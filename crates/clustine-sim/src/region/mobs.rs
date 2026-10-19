//! The region's side of a mob's tick (`crate::mobs`): the mob taken from its chunk and
//! decoded, its tick run on a view of the region, and what it changed committed, only
//! the parts of its record that changed among it (`docs/groundwork/n7-plan.md`, section
//! 11.3).

use clustine_data::EntityTypeInfo;
use clustine_world::{
    BlockPos, ChunkPos, Entity, EntityChange, EntityField, EntityId, PlayerId, SectionLight,
};

use super::Region;
use super::items::{Memory, View, tracked};
use super::riding::Relocated;
use crate::api::{PlayerFace, RegionEvent, TickOutput, Tracked};
use crate::living::{Kind, Living};
use crate::mobs::tick::{MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::mobs::{Mob, tick, types};
use crate::physics::body::Movement;
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

    fn light(&self, pos: BlockPos) -> Option<LightAt> {
        let chunk = self.region.readable_chunk(pos.chunk())?;
        let (x, z) = pos.in_chunk();
        let section = (pos.y - chunk.min_y()).div_euclid(16);
        if let Some(light) = chunk.light() {
            // One entry below the column, then one a section, then one above.
            let index = usize::try_from(section + 1).ok()?;
            let level = |layer: &[SectionLight], absent: u8| match layer.get(index) {
                None | Some(SectionLight::Absent) => absent,
                Some(SectionLight::Dark) => 0,
                Some(SectionLight::Full) => 15,
                Some(SectionLight::Levels(levels)) => {
                    let at = (pos.y.rem_euclid(16) as usize) << 8 | z << 4 | x;
                    (levels[at >> 1] >> ((at & 1) * 4)) & 0x0F
                }
            };
            return Some(LightAt {
                sky: level(&light.sky, 15),
                block: level(&light.block, 0),
            });
        }
        let top = chunk.min_y() + i32::from(chunk.heights().motion_blocking[z << 4 | x]);
        Some(LightAt {
            sky: if pos.y >= top { 15 } else { 0 },
            block: 0,
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
        let body = self.living_body(&entity, info.width, info.height, position);
        let kind = Kind::Mob {
            kind: entity.kind,
            hurts: Vec::new(),
        };
        let memory = self.entity_memory.get(&key);
        let mut living = Living::new(body, kind, info.eye_height);
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
        let view = View {
            region: self,
            home: position,
        };
        let vehicle = entity
            .riding
            .vehicle
            .and_then(|vehicle| self.find_entity(vehicle))
            .map(|(_, vehicle)| vehicle.kind);
        let mut ticking = MobTick {
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
        };
        tick::tick(&mut ticking);
        let MobTick {
            mut mob,
            mut living,
            yaw,
            pitch,
            out,
            ..
        } = ticking;
        self.entity_drew(key, stream.draws_since(&start));
        self.living_aftermath(key, &mut living, output);
        mob.body.vitals.air = living.air;
        mob.body.vitals.frozen = living.frozen;
        let mut flags = 0;
        if out.needs_sync {
            flags |= Tracked::NEEDS_SYNC;
        }
        if out.sync_velocity {
            flags |= Tracked::SYNC_VELOCITY;
        }
        // What it showed, dropped and did to others (`mob_bodies.rs`), then its record,
        // unless it left the world.
        let removed = out.removed;
        self.mob_ticked(&entity, living.body.position, out, position, output);
        if removed.is_some() {
            return;
        }
        let shown = Shown { yaw, pitch, flags };
        self.write_mob(&entity, &mob, &living, shown, position, output);
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
        let boat = self
            .entity_memory
            .get(&key)
            .map(|memory| memory.boat.clone())
            .unwrap_or_default();
        self.entity_memory.insert(
            key,
            Memory {
                supporting: living.body.supporting,
                on_ground_no_blocks: living.body.on_ground_no_blocks,
                last_movements: living.last_movements.clone(),
                stuck_speed: living.body.stuck_speed,
                no_physics: living.no_physics,
                ticks_since_check: 0,
                cart: Default::default(),
                boat,
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

/// What a mob reads of a player, from their face.
fn seen(id: PlayerId, key: EntityId, face: &PlayerFace) -> SeenPlayer {
    SeenPlayer {
        key,
        uuid: id.0.as_u128(),
        position: face.position,
        eye_height: f64::from(face.eye_height),
        yaw: face.yaw,
        pitch: face.pitch,
        main_hand: face.main_hand,
        off_hand: face.off_hand,
        sneaking: face.sneaking,
        creative: face.game_mode == 1,
        spectator: face.game_mode == 3,
        vehicle: face.vehicle,
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
