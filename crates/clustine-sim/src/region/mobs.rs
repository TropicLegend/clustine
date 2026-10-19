//! The region's side of a mob's tick (`crate::mobs`): the mob taken from its chunk and
//! decoded, its tick run on a view of the region, and what it changed committed, only
//! the parts of its record that changed among it (`docs/groundwork/n7-plan.md`, section
//! 11.3).

use clustine_data::EntityTypeInfo;
use clustine_world::{
    BlockPos, ChunkPos, Entity, EntityChange, EntityField, OFF_HAND, SectionLight,
};

use super::entities::CROUCHING_EYES;
use super::items::{Memory, View, tracked};
use super::riding::Relocated;
use super::{EYE_HEIGHT, Region};
use crate::api::{RegionEvent, TickOutput, Tracked};
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

    fn players(&self) -> Vec<SeenPlayer> {
        let mut players: Vec<SeenPlayer> = self
            .region
            .players
            .iter()
            .map(|(id, player)| SeenPlayer {
                key: player.entity_id,
                uuid: id.0.as_u128(),
                position: [
                    player.pose.position.x,
                    player.pose.position.y,
                    player.pose.position.z,
                ],
                eye_height: if player.keys.sneak && !player.flying {
                    f64::from(CROUCHING_EYES)
                } else {
                    EYE_HEIGHT
                },
                yaw: player.pose.yaw,
                pitch: player.pose.pitch,
                main_hand: player.inventory.0[usize::from(player.selected_slot)],
                off_hand: player.inventory.0[OFF_HAND],
                sneaking: player.keys.sneak,
                creative: true,
                spectator: false,
                vehicle: player.riding.map(|riding| riding.vehicle),
            })
            .collect();
        players.sort_by_key(|player| player.key);
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
        let Ok(mob) = Mob::decode(&entity.data) else {
            output.unreadable_mobs.push(key);
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
        let mut ticking = MobTick {
            key,
            kind: entity.kind,
            lived: entity.lived,
            tick_count: entity.tick_count(),
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
            out,
            ..
        } = ticking;
        self.entity_drew(key, stream.draws_since(&start));
        debug_assert!(out.local.is_empty(), "no write to another is made yet");
        self.living_aftermath(key, &mut living, output);
        mob.body.vitals.air = living.air;
        mob.body.vitals.frozen = living.frozen;
        self.write_mob(&entity, &mob, &living, position, output);
    }

    /// What the tick leaves of the mob `before` of the chunk `position`: its record, its
    /// commit where it changed what is committed (with its `lived`, so that what is
    /// stamped against it is never ahead of it: the plan's 7.2.4), and its report.
    fn write_mob(
        &mut self,
        before: &Entity,
        mob: &Mob,
        living: &Living,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        let mut after = Entity {
            position: living.body.position,
            motion: living.body.motion,
            on_ground: living.body.on_ground,
            fall_distance: living.body.fall_distance,
            fire_ticks: living.fire_ticks,
            lived: before.lived.saturating_add(1),
            data: mob.encode(),
            ..before.clone()
        };
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
                boat: Default::default(),
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
        let mut flags = 0;
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
        if committed || burning_shown || to != position || living.body.sync_position {
            let mut report = tracked(&after, flags, !parts.is_empty());
            report.chunk = to;
            output.events.push(RegionEvent::EntityTracked(report));
        }
    }
}
