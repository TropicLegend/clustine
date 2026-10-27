//! The region's side of a mob's tick (`crate::mobs`): the mob taken from its chunk and
//! decoded, its tick run on a view of the region, and what it changed committed, only
//! the parts of its record that changed among it (`docs/groundwork/n7-plan.md`, section
//! 11.3).

use clustine_data::EntityTypeInfo;
use clustine_world::{BlockPos, ChunkPos, Entity, EntityChange, EntityField, EntityId, PlayerId};

use super::Region;
use super::items::{Memory, View, tracked};
use super::riding::Relocated;
use crate::api::{PlayerFace, RegionEvent, TickOutput, Tracked};
use crate::living::{Kind, Living};
use crate::mobs::tick::{MobOutput, MobTick, Ridden};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::mobs::{Mob, tick, types};
use crate::physics::body::{Movement, Steps};
use crate::physics::{self, aabb::Aabb};

impl MobWorld for View<'_> {
    fn is_loaded(&self, pos: BlockPos) -> bool {
        self.region.readable(pos.chunk())
    }

    fn entities_in(&self, bounds: &Aabb) -> Vec<Seen<'_>> {
        // As `colliders_in` gathers them: a box reaches the chunks its corners and two
        // blocks beyond touch, since a box is reckoned by its entity's chunk.
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut found = Vec::new();
        for copied in [false, true] {
            for x in low.x..=high.x {
                for z in low.z..=high.z {
                    let position = ChunkPos::new(x, z);
                    let chunk = if copied {
                        self.region.copies.get(&position)
                    } else {
                        self.region.chunks.get(&position)
                    };
                    let Some(chunk) = chunk else {
                        continue;
                    };
                    let mut here: Vec<(i32, Seen<'_>)> = chunk
                        .entities()
                        .iter()
                        .filter_map(|entity| {
                            let own = self.region.entity_box(entity);
                            own.intersects(*bounds).then_some((
                                physics::floor(entity.position[1]) >> 4,
                                Seen {
                                    entity,
                                    bounds: own,
                                    copied,
                                },
                            ))
                        })
                        .collect();
                    here.sort_by_key(|(section, seen)| (*section, seen.entity.key));
                    found.extend(here.into_iter().map(|(_, seen)| seen));
                }
            }
        }
        found
    }

    fn entity(&self, key: EntityId) -> Option<Seen<'_>> {
        if let Some((_, entity)) = self.region.find_entity(key) {
            return Some(Seen {
                entity,
                bounds: self.region.entity_box(entity),
                copied: false,
            });
        }
        // A copy's entities are not indexed by key: a target across the line is looked
        // for among them.
        self.region.copies.values().find_map(|chunk| {
            let entity = chunk.entities().iter().find(|entity| entity.key == key)?;
            Some(Seen {
                entity,
                bounds: self.region.entity_box(entity),
                copied: true,
            })
        })
    }

    /// The players the region holds, by their keys, then the faces of others it keeps,
    /// by the player (`docs/groundwork/n7-plan.md`, 10.11).
    fn players(&self) -> Vec<SeenPlayer> {
        let mut players: Vec<SeenPlayer> = self
            .region
            .players
            .iter()
            .map(|(id, player)| seen(*id, player.entity_id, &player.face()))
            .collect();
        players.sort_by_key(|player| player.key);
        let region = self.region;
        let holds = |chunk| region.land.holds(chunk) || region.copies.contains_key(&chunk);
        let faces = region.faces.seen(holds);
        players.extend(faces.map(|(id, kept)| seen(*id, kept.entity, &kept.face)));
        players
    }

    fn sky_darken(&self) -> i32 {
        crate::time::sky_darken(self.region.clock.day)
    }

    fn game_time(&self) -> i64 {
        i64::try_from(self.region.clock.game_time).unwrap_or(i64::MAX)
    }

    fn day(&self) -> u64 {
        self.region.clock.day
    }

    fn biome(&self, pos: BlockPos) -> Option<&'static str> {
        self.region.biome_at(pos)
    }

    fn block_entity_data(&self, pos: BlockPos) -> Option<&[u8]> {
        self.region.block_entity_data(pos)
    }

    /// Those the mob reads first, then the world's players as the store last published
    /// them: where they stand and their game mode, holding nothing a mob reads.
    fn player_anywhere(&self, uuid: u128) -> Option<SeenPlayer> {
        if let Some(found) = self
            .players()
            .into_iter()
            .find(|player| player.uuid == uuid)
        {
            return Some(found);
        }
        let known = self
            .region
            .faces
            .world
            .iter()
            .flatten()
            .find(|player| player.player.0.as_u128() == uuid)?;
        let at = clustine_world::Vec3::new(known.position[0], known.position[1], known.position[2]);
        let mut face = PlayerFace::of(
            &crate::api::Pose::at(at),
            false,
            false,
            &clustine_world::Inventory::default(),
            0,
            None,
        );
        face.game_mode = known.game_mode;
        // Their sleep's counter as of the store's last word, as the time they lay down.
        let now = self.region.clock.game_time;
        face.sleeping = known
            .sleeping
            .map(|counter| now.saturating_sub(u64::from(counter)));
        Some(seen(known.player, known.entity, &face))
    }

    /// Those the mob reads, then the world's players beyond them as the store last
    /// published them; none until it has published them once.
    fn despawn_players(&self) -> Option<Vec<[f64; 3]>> {
        let world = self.region.faces.world.as_ref()?;
        let seen = self.players();
        let mut positions: Vec<[f64; 3]> = seen
            .iter()
            .filter(|player| !player.spectator)
            .map(|player| player.position)
            .collect();
        positions.extend(
            world
                .iter()
                .filter(|other| other.game_mode != 3)
                .filter(|other| {
                    !seen
                        .iter()
                        .any(|player| player.uuid == other.player.0.as_u128())
                })
                .map(|other| other.position),
        );
        Some(positions)
    }

    fn motion_blocking_height(&self, x: i32, z: i32) -> Option<i32> {
        let pos = BlockPos::new(x, 0, z);
        let chunk = self.region.readable_chunk(pos.chunk())?;
        let (x, z) = pos.in_chunk();
        Some(chunk.min_y() + i32::from(chunk.heights().motion_blocking[z << 4 | x]))
    }

    fn motion_blocking_no_leaves_height(&self, x: i32, z: i32) -> Option<i32> {
        self.region.motion_blocking_no_leaves_height(x, z)
    }

    fn light(&self, pos: BlockPos) -> Option<LightAt> {
        // The region's light as of its step (ADR-0029); `None` where the chunk is not lit.
        self.region.light_at(pos).map(|light| LightAt {
            sky: light.sky,
            block: light.block,
        })
    }
}

impl Region {
    /// The tick of the mob `entity`, of the loaded chunk `position`.
    pub(crate) fn tick_mob(&mut self, entity: Entity, position: ChunkPos, output: &mut TickOutput) {
        let key = entity.key;
        let Ok(mut mob) = Mob::decode(&entity.data) else {
            output.unreadable_mobs.push(key);
            return;
        };
        // Its leash, before its tick (`mob_bodies.rs`), which may move it, turn it,
        // keep it home or let it go: its record is read again after it.
        let pull = self.mob_tick_leash(&entity, &mut mob, position, output);
        let Some((position, entity)) = self
            .find_entity(key)
            .map(|(position, entity)| (position, entity.clone()))
        else {
            return;
        };
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        // A baby animal's size is its own, as its type state has it at the tick's start.
        let (width, height, eye_height) = types::own_size(&entity)
            .map_or((info.width, info.height, info.eye_height), |size| {
                (size.width, size.height, size.eye_height)
            });
        let mut body = self.living_body(&entity, width, height, position);
        // What its last move left of its collision, which a fish's flop reads.
        body.vertical_collision = memory_vertical(self, key);
        let kind = Kind::Mob {
            kind: entity.kind,
            hurts: Vec::new(),
        };
        let memory = self.entity_memory.get(&key);
        let mut living = Living::new(body, kind, eye_height);
        // A passenger's moves are its vehicle's and make no steps of its own
        // (`Entity.move`'s `isPassenger`).
        if entity.riding.vehicle.is_none() {
            living.body.steps = Some(Steps {
                count: memory.map(|memory| memory.steps).unwrap_or_default(),
                taken: Vec::new(),
            });
        }
        let mut yaw = entity.yaw;
        if let Some(pull) = pull {
            living.body.motion = pull.motion;
            living.body.fall_distance = pull.fall_distance;
            yaw = pull.yaw;
        }
        // A mob without AI does not travel at all (the brief's 1.0.13).
        living.travels = !mob.body.flags.no_ai;
        living.fire_ticks = entity.fire_ticks;
        living.air = mob.body.vitals.air;
        living.frozen = mob.body.vitals.frozen;
        living.first_tick = memory.is_none() && entity.tick_count() == 0;
        living.last_movements = memory.map_or_else(
            || {
                vec![Movement {
                    from: entity.position,
                    to: entity.position,
                    asked: None,
                }]
            },
            |memory| memory.last_movements.clone(),
        );
        let mut stream = self.entity_stream(key).random;
        let start = stream.clone();
        // The vehicle it steers, and the vehicle's stream for what its navigation
        // draws in its rider's tick (`docs/groundwork/n9-plan.md`, section 6).
        let steered = self.steered_vehicle(&entity);
        let mut vehicle_stream = steered
            .as_ref()
            .map(|(_, vehicle, _)| self.entity_stream(vehicle.key).random);
        let vehicle_start = vehicle_stream.clone();
        let controlled_by_mob = self.controlling_rider(&entity, &mob).is_some();
        let view = View {
            region: self,
            home: position,
        };
        let vehicle = entity
            .riding
            .vehicle
            .and_then(|vehicle| self.find_entity(vehicle))
            .map(|(_, vehicle)| vehicle.kind);
        let ridden = self.ridden(&entity, &mob);
        let controlled =
            match (&steered, vehicle_stream.as_mut()) {
                (Some((chunk, vehicle, vehicle_mob)), Some(draws)) => Some(Box::new(
                    self.controlled(*chunk, vehicle, vehicle_mob.clone(), &view, draws),
                )),
                _ => None,
            };
        let mut ticking = MobTick {
            ridden,
            key,
            kind: entity.kind,
            lived: entity.lived,
            tick_count: entity.tick_count(),
            yaw,
            pitch: entity.pitch,
            vehicle,
            carries: !entity.riding.passengers.is_empty(),
            leashed: entity.riding.leash.is_some(),
            counted: false,
            mob,
            living,
            mob_type: types::of(entity.kind),
            world: &view,
            draws: &mut stream,
            out: MobOutput::default(),
            sensed: memory.and_then(|memory| memory.sensed.clone()),
            controlled,
            controlled_by_mob,
        };
        tick::tick(&mut ticking);
        let MobTick {
            mut mob,
            mut living,
            yaw,
            pitch,
            mut out,
            mob_type,
            sensed,
            controlled,
            ..
        } = ticking;
        // The vehicle's navigation and move control as its rider's goals left them,
        // written back before the rider is put in its seat.
        if let (Some(controlled), Some((chunk, vehicle, _)), Some(from)) =
            (controlled, &steered, &vehicle_start)
        {
            let drawn = controlled.tick.draws.draws_since(from);
            let mob = controlled.tick.mob;
            self.write_steered(*chunk, vehicle, &mob, drawn, output);
        }
        if sensed.is_some() {
            self.entity_memory.entry(key).or_default().sensed = sensed;
        }
        // What the type's tick does with the time a quad leashee keeps it a holder (a
        // happy ghast's `leashHolderTime`, kept in the region's memory).
        let mut holder_time = self
            .entity_memory
            .get(&key)
            .map_or(0, |memory| memory.leash_holder_time);
        mob_type.leash_holder_tick(&mut mob, &mut holder_time);
        if holder_time != 0 || self.entity_memory.contains_key(&key) {
            self.entity_memory.entry(key).or_default().leash_holder_time = holder_time;
        }
        // Its writes to others come after its own record is written, so that what they
        // do to it (a breeding it asked for) is not written over.
        let locals = std::mem::take(&mut out.local);
        self.entity_drew(key, stream.draws_since(&start));
        self.living_aftermath(key, &mut living, output);
        mob.body.vitals.air = living.air;
        mob.body.vitals.frozen = living.frozen;
        let mut flags = 0;
        if out.needs_sync {
            flags |= Tracked::NEEDS_SYNC;
        }
        // Sent whole while its type says (a still happy ghast), and at once in the tick
        // it begins to be, as `setServerStillTimeout` sends its position then.
        if mob_type.requires_precise_position(&mob) {
            flags |= Tracked::PRECISE;
            let was =
                Mob::decode(&entity.data).is_ok_and(|was| mob_type.requires_precise_position(&was));
            if !was {
                flags |= Tracked::NEEDS_SYNC;
            }
        }
        if out.sync_velocity {
            flags |= Tracked::SYNC_VELOCITY;
        }
        // What it showed, dropped and did to others (`mob_bodies.rs`), then its record,
        // unless it left the world.
        let removed = out.removed;
        self.mob_ticked(&entity, living.body.position, out, position, output);
        if removed.is_none() {
            let shown = Shown { yaw, pitch, flags };
            self.write_mob(&entity, &mob, &living, shown, position, output);
        }
        self.apply_locals(key, locals, output);
    }

    /// The vehicle the mob `rider` steers, where it rides first a mob of the region that
    /// it controls (`getControlledVehicle`): its chunk, its record and its mob.
    fn steered_vehicle(&self, rider: &Entity) -> Option<(ChunkPos, Entity, Mob)> {
        let (chunk, vehicle) = self.find_entity(rider.riding.vehicle?)?;
        let vehicle_mob = Mob::decode(&vehicle.data).ok()?;
        (self.controlling_rider(vehicle, &vehicle_mob) == Some(rider.key))
            .then(|| (chunk, vehicle.clone(), vehicle_mob))
    }

    /// `Mob.getControllingPassenger()` of the mob `vehicle`: its first passenger, where
    /// that is a mob of the region not of `#non_controlling_rider` and the vehicle has
    /// AI. A player's steering is `ridden`'s.
    pub(crate) fn controlling_rider(
        &self,
        vehicle: &Entity,
        vehicle_mob: &Mob,
    ) -> Option<EntityId> {
        if vehicle_mob.body.flags.no_ai {
            return None;
        }
        let seat = vehicle.riding.passengers.first()?;
        if seat.player {
            return None;
        }
        let (_, rider) = self.find_entity(seat.key)?;
        let clustine_world::EntityData::Mob { .. } = rider.data else {
            return None;
        };
        let leads = !clustine_blocks::data::entity_in_tag(
            i32::from(rider.kind),
            "minecraft:non_controlling_rider",
        );
        leads.then_some(seat.key)
    }

    /// The vehicle `vehicle` of the chunk `chunk` as its steering rider's tick sees it:
    /// its own tick, reading `view`, drawing from `draws`, and its navigation's
    /// configuration.
    fn controlled<'w>(
        &self,
        chunk: ChunkPos,
        vehicle: &Entity,
        vehicle_mob: Mob,
        view: &'w View<'_>,
        draws: &'w mut super::Random,
    ) -> tick::Controlled<'w> {
        let info = EntityTypeInfo::of(i32::from(vehicle.kind));
        let (width, height, eye_height) = types::own_size(vehicle)
            .map_or((info.width, info.height, info.eye_height), |size| {
                (size.width, size.height, size.eye_height)
            });
        let body = self.living_body(vehicle, width, height, chunk);
        let kind = Kind::Mob {
            kind: vehicle.kind,
            hurts: Vec::new(),
        };
        let mut living = Living::new(body, kind, eye_height);
        living.travels = !vehicle_mob.body.flags.no_ai;
        living.fire_ticks = vehicle.fire_ticks;
        let tick = MobTick {
            ridden: Ridden::Not,
            key: vehicle.key,
            kind: vehicle.kind,
            lived: vehicle.lived,
            tick_count: vehicle.tick_count(),
            yaw: vehicle.yaw,
            pitch: vehicle.pitch,
            vehicle: None,
            carries: true,
            leashed: vehicle.riding.leash.is_some(),
            counted: false,
            mob: vehicle_mob,
            living,
            mob_type: types::of(vehicle.kind),
            world: view,
            draws,
            out: MobOutput::default(),
            sensed: None,
            controlled: None,
            controlled_by_mob: true,
        };
        let config = crate::mobs::ai::config_of(&tick);
        tick::Controlled { tick, config }
    }

    /// Writes the steered vehicle `before` of the chunk `chunk` back with its mob as its
    /// rider's tick left it, `drawn` the draws its navigation made: what changed is
    /// committed with the vehicle's `lived` as it stands.
    fn write_steered(
        &mut self,
        chunk: ChunkPos,
        before: &Entity,
        mob: &Mob,
        drawn: u64,
        output: &mut TickOutput,
    ) {
        self.entity_drew(before.key, drawn);
        let Some((now, current)) = self.find_entity(before.key) else {
            return;
        };
        if now != chunk {
            return;
        }
        let mut fields = mob.changes_from(&current.data);
        if fields.is_empty() {
            return;
        }
        let after = Entity {
            data: mob.encode(),
            ..current.clone()
        };
        fields.push(EntityField::Lived(after.lived));
        self.commit_entity(chunk, before.key, fields, output);
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&EntityChange::Whole(after));
        }
    }

    /// What the tick leaves of the mob `before` of the chunk `position`: its record, its
    /// commit where it changed what is committed (with its `lived`, so that what is
    /// stamped against it is never ahead of it: the plan's 7.2.4), and its report.
    fn write_mob(
        &mut self,
        before: &Entity,
        mob: &Mob,
        living: &Living,
        shown: Shown,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        let mut after = Entity {
            position: living.body.position,
            motion: living.body.motion,
            yaw: shown.yaw,
            pitch: shown.pitch,
            on_ground: living.body.on_ground,
            fall_distance: living.body.fall_distance,
            fire_ticks: living.fire_ticks,
            lived: before.lived.saturating_add(1),
            data: mob.encode(),
            ..before.clone()
        };
        // Its leash's memory outlives its tick (`leashes.rs`).
        let kept = self.entity_memory.get(&key);
        let boat = kept.map(|memory| memory.boat.clone()).unwrap_or_default();
        // A passenger's count waits for it to walk again.
        let steps = living.body.steps.as_ref().map_or_else(
            || kept.map(|memory| memory.steps).unwrap_or_default(),
            |steps| steps.count,
        );
        self.entity_memory.insert(
            key,
            Memory {
                sensed: kept.and_then(|memory| memory.sensed.clone()),
                supporting: living.body.supporting,
                on_ground_no_blocks: living.body.on_ground_no_blocks,
                last_movements: living.last_movements.clone(),
                stuck_speed: living.body.stuck_speed,
                no_physics: living.no_physics,
                vertical_collision: living.body.vertical_collision,
                ticks_since_check: 0,
                cart: Default::default(),
                boat,
                steps,
                leash_holder_time: kept.map_or(0, |memory| memory.leash_holder_time),
            },
        );
        let to = match self.relocate(position, before.position, &mut after, output) {
            Relocated::Sent => return,
            Relocated::Moved(to) => to,
            Relocated::Stays => position,
        };
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        let parts = mob.changes_from(&before.data);
        let moved = bits(after.position) != bits(before.position);
        let mut committed = false;
        if to == position {
            let mut fields = Vec::new();
            if moved {
                fields.push(EntityField::Position(after.position));
            }
            if bits(after.motion) != bits(before.motion) {
                fields.push(EntityField::Motion(after.motion));
            }
            if after.on_ground != before.on_ground {
                fields.push(EntityField::OnGround(after.on_ground));
            }
            if after.yaw.to_bits() != before.yaw.to_bits()
                || after.pitch.to_bits() != before.pitch.to_bits()
            {
                fields.push(EntityField::Rotation {
                    yaw: after.yaw,
                    pitch: after.pitch,
                });
            }
            if after.fall_distance.to_bits() != before.fall_distance.to_bits() {
                fields.push(EntityField::FallDistance(after.fall_distance));
            }
            if after.fire_ticks != before.fire_ticks {
                fields.push(EntityField::FireTicks(after.fire_ticks));
            }
            fields.extend(parts.iter().cloned());
            if !fields.is_empty() {
                committed = true;
                fields.push(EntityField::Lived(after.lived));
                self.commit_entity(position, key, fields, output);
            }
            // Its counters, committed or not, as the tick leaves them.
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        // The attributes a client is sent (`UpdateAttributes`), where they changed, and
        // at its first tick, which makes its `movement_speed`; its equipment
        // (`SetEquipment`), where it changed.
        self.mob_shown_changes(before, &after, to, output);
        let mut flags = shown.flags;
        if living.body.horizontal_collision {
            flags |= Tracked::HORIZONTAL_COLLISION;
        }
        if living.body.vertical_collision {
            flags |= Tracked::VERTICAL_COLLISION;
        }
        if living.body.sync_position {
            flags |= Tracked::SYNC_POSITION;
        }
        if living.on_fire_flag {
            flags |= Tracked::ON_FIRE;
        }
        let burning_shown = living.on_fire_flag != (before.fire_ticks > 0);
        if committed
            || burning_shown
            || to != position
            || living.body.sync_position
            || shown.flags != 0
        {
            let mut report = tracked(&after, flags, !parts.is_empty());
            report.chunk = to;
            output.events.push(RegionEvent::EntityTracked(report));
        }
    }
}

impl Region {
    /// How the mob `entity` is ridden as its tick begins: its first seat a player's on
    /// their way to it, kept still; or a player of the region its type lets steer it as
    /// they hold what they hold now (the region's own players only, never a face).
    pub(crate) fn ridden(&self, entity: &Entity, mob: &crate::mobs::Mob) -> Ridden {
        let Some(seat) = entity.riding.passengers.first() else {
            return Ridden::Not;
        };
        if !seat.player {
            return Ridden::Not;
        }
        if seat.pending.is_some() {
            return Ridden::Pending;
        }
        let Some(id) = self.player_with_entity(seat.key) else {
            return Ridden::Not;
        };
        let player = &self.players[&id];
        let hands = [
            player.inventory.0[usize::from(player.selected_slot)].clone(),
            player.inventory.0[clustine_world::OFF_HAND].clone(),
        ];
        if types::of(entity.kind).steered_by(mob, hands) {
            Ridden::Driven {
                yaw: player.pose.yaw,
                pitch: player.pose.pitch,
            }
        } else {
            Ridden::Not
        }
    }
}

/// `verticalCollision` as the mob `key`'s last move in the region left it.
fn memory_vertical(region: &Region, key: EntityId) -> bool {
    region
        .entity_memory
        .get(&key)
        .is_some_and(|memory| memory.vertical_collision)
}

/// What a mob reads of a player, from their face.
fn seen(id: PlayerId, key: EntityId, face: &PlayerFace) -> SeenPlayer {
    SeenPlayer {
        key,
        uuid: id.0.as_u128(),
        position: face.position,
        eye_height: f64::from(face.eye_height),
        yaw: face.yaw,
        pitch: face.pitch,
        main_hand: face.main_hand.clone(),
        off_hand: face.off_hand.clone(),
        sneaking: face.sneaking,
        creative: face.game_mode == 1,
        spectator: face.game_mode == 3,
        vehicle: face.vehicle,
        moved_horizontally: face.moved_horizontally,
        sprinting: face.sprinting,
        alive: face.alive,
        sleeping: face.sleeping,
        last_hurt_mob: face.last_hurt_mob,
        last_hurt_by: face.last_hurt_by,
    }
}

/// What a mob's tick leaves of it beyond its body and record: its rotation, and the
/// flags its report carries (`needsSync`, `syncVelocity`).
#[derive(Debug, Clone, Copy)]
struct Shown {
    yaw: f32,
    pitch: f32,
    flags: u8,
}
