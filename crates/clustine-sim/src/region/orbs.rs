//! Experience orbs (`ExperienceOrb`, `docs/groundwork/n7-brief.md`, section 1.7): what a
//! mob a player killed leaves of its experience, merged into an orb of the same worth
//! near by, or a new orb thrown up a little; each tick an orb falls (rising in water,
//! tossed in lava), merges every twentieth tick with those of its worth whose id is the
//! same modulo 40, follows the nearest player within 8 blocks, and is gone at 6000
//! ticks; a player in its reach takes one orb every other tick (`takeXpDelay`), a
//! creative player too.
//!
//! The game draws the merge's `nextInt(40)` from the level's random: it is drawn at its
//! place in the dying mob's stream (`crate::mobs::body::Death`). An orb's own draws (its
//! UUID, its yaw, its motion; lava's toss) are its key's stream's.
//!
//! Named differences: an orb is neither burnt by fire or lava nor hurt by anything but
//! an explosion (`applyEffectsFromBlocks` is left out), and one stuck in a block is not
//! eased out of it (`unstuckIfPossible`, `moveTowardsClosestSpace`); a player's delay
//! between orbs is the region's memory, which another region's does not share; the
//! player's experience is theirs ([`Region::add_experience`]).

use clustine_blocks::fluid::FluidState;
use clustine_data::{EntityTypeInfo, entity_types};
use clustine_world::{ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId};

use super::Region;
use super::entities::insecure_uuid;
use super::items::{Memory, View, tracked};
use super::riding::Relocated;
use crate::api::{RegionEvent, TickOutput};
use crate::physics::aabb::Aabb;
use crate::physics::body::{Bounce, physics_of};
use crate::physics::{self, Vec3, World, collide, fluids};

/// `getGravity()` of an orb.
const GRAVITY: f64 = 0.03;

/// How long an orb lasts (`age` of 6000).
const LIFETIME: u32 = 6000;

/// An orb's own bounce: its gravity, its air drag of `0.98f`, nothing of itself.
const BOUNCE: Bounce = Bounce {
    gravity: GRAVITY,
    air_drag: 0.98,
    own: 0.0,
    living: false,
    rails_keep_speed: false,
    max_up_step: 0.0,
};

/// The orb's type, by id.
fn orb_kind() -> u16 {
    u16::try_from(entity_types::EXPERIENCE_ORB).expect("entity type ids fit 16 bits")
}

/// `getId()` of the entity `key`: its key's low 32 bits as a Java `int`.
fn java_id(key: EntityId) -> i32 {
    key.0 as u32 as i32
}

/// `canMerge(orb, id, value)`: of the same worth, and of an id the same as `id` modulo
/// 40 (Java's `%`, which keeps the sign: only a difference of a multiple of 40 is 0).
fn merges(orb: &Entity, id: i32, worth: i32) -> bool {
    let EntityData::ExperienceOrb { value, .. } = orb.data else {
        return false;
    };
    value == worth && java_id(orb.key).wrapping_sub(id) % 40 == 0
}

/// An orb's age (`age`): its ticks lived less what a merge with a younger one took off.
fn orb_age(orb: &Entity) -> u32 {
    match orb.data {
        EntityData::ExperienceOrb { age_offset, .. } => orb.lived.saturating_sub(age_offset),
        _ => orb.lived,
    }
}

impl Region {
    /// `ExperienceOrb.award`'s one orb of `value` at `position`, with the level's draw
    /// `draw` of `tryMergeToExisting`: the first orb of the region's in the box of 1
    /// about the position that merges with it takes it in (one more, and young again);
    /// else a new orb, whose own draws turn it and throw it a little.
    pub(crate) fn award_orb(
        &mut self,
        position: Vec3,
        value: i32,
        draw: u32,
        output: &mut TickOutput,
    ) {
        let around = Aabb::new(
            [position[0] - 0.5, position[1] - 0.5, position[2] - 0.5],
            [position[0] + 0.5, position[1] + 0.5, position[2] + 0.5],
        );
        let draw = i32::try_from(draw).expect("a draw below 40");
        let found = self.entities_in(&around).into_iter().find(|&(key, chunk)| {
            self.entity_record(key, chunk)
                .is_some_and(|orb| merges(orb, draw, value))
        });
        if let Some((key, chunk)) = found
            && let Some(orb) = self.entity_record(key, chunk).cloned()
            && let EntityData::ExperienceOrb {
                value,
                count,
                health,
                following,
                ..
            } = orb.data
        {
            let data = EntityData::ExperienceOrb {
                value,
                count: count.saturating_add(1),
                health,
                age_offset: orb.lived,
                following,
            };
            self.commit_entity(chunk, key, vec![EntityField::Data(data)], output);
            return;
        }
        self.add_orb(position, value, output);
    }

    /// `new ExperienceOrb(level, x, y, z, value)` added (a breeding's): an orb of its own,
    /// whose own draws turn it and throw it a little.
    pub(crate) fn add_orb(&mut self, position: Vec3, value: i32, output: &mut TickOutput) {
        let chunk = ChunkPos::containing(position[0], position[2]);
        let here = self.ticks_chunk(chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return;
        };
        // `Entity.<init>`'s UUID, then the orb's own turn and throw.
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let yaw = own.next_float() * 360.0;
        let x = (own.next_double() * 0.2 - 0.1) * 2.0;
        let y = own.next_double() * 0.2 * 2.0;
        let z = (own.next_double() * 0.2 - 0.1) * 2.0;
        let entity = Entity {
            key,
            kind: orb_kind(),
            uuid,
            position,
            motion: [x, y, z],
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::ExperienceOrb {
                value,
                count: 1,
                health: 5,
                age_offset: 0,
                following: None,
            },
        };
        if here {
            self.entity_drew(key, own.draws_since(&start));
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
    }

    /// `ExperienceOrb.tick()` of the orb `entity` of the loaded chunk `position`.
    pub(crate) fn tick_orb(&mut self, entity: Entity, position: ChunkPos, output: &mut TickOutput) {
        let EntityData::ExperienceOrb {
            value,
            count,
            health,
            mut following,
            ..
        } = entity.data
        else {
            return;
        };
        let key = entity.key;
        let info = EntityTypeInfo::of(entity_types::EXPERIENCE_ORB);
        let mut body = self.living_body(&entity, info.width, info.height, position);
        let mut stream = self.entity_stream(key).random;
        let start = stream.clone();
        let view = View {
            region: self,
            home: position,
        };
        // `Entity.baseTick`: the fluids it is in and their push; far below the world it
        // is discarded.
        let found = fluids::update(&view, &body.bounds());
        body.in_water = found.in_water();
        if found.in_water() {
            body.fall_distance = 0.0;
        }
        body.motion = fluids::push(&found, body.motion);
        if body.position[1] < f64::from(item_min_y(&view) - 64) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let stuck = !collide::no_collision(&view, body.context(), &body.mover, &body.bounds());
        let eyes = [
            body.position[0],
            body.position[1] + f64::from(info.eye_height),
            body.position[2],
        ];
        if eye_in_water(&view, eyes) {
            // `setUnderwaterMovement`.
            let [x, y, z] = body.motion;
            body.motion = [
                x * f64::from(0.99_f32),
                (y + f64::from(5.0e-4_f32)).min(f64::from(0.06_f32)),
                z * f64::from(0.99_f32),
            ];
        } else if !stuck {
            body.motion[1] -= GRAVITY;
        }
        let at = view.state(body.block_position());
        if FluidState::of(at).kind() == Some(clustine_blocks::fluid::Kind::Lava) {
            let x = (stream.next_float() - stream.next_float()) * 0.2;
            let z = (stream.next_float() - stream.next_float()) * 0.2;
            body.motion = [f64::from(x), 0.200_000_002_980_232_24, f64::from(z)];
        }
        let tick_count = entity.tick_count().wrapping_add(1);
        let mut merged: Vec<(EntityId, ChunkPos, u32, i32)> = Vec::new();
        if tick_count % 20 == 1 {
            // `scanForMerges`: the orbs of the region's in its box grown by half a block.
            let reach = body.bounds().inflate([0.5; 3]);
            let id = java_id(key);
            for (other, chunk) in self.entities_in(&reach) {
                if other == key {
                    continue;
                }
                if let Some(orb) = self.entity_record(other, chunk)
                    && merges(orb, id, value)
                    && let EntityData::ExperienceOrb { count, .. } = orb.data
                {
                    merged.push((other, chunk, orb_age(orb), count));
                }
            }
        }
        following = self.followed(&body.position, following);
        if let Some(player) = following.and_then(|player| self.player_feet_of(player)) {
            let (feet, eye) = player;
            let to = [
                feet[0] - body.position[0],
                feet[1] + eye / 2.0 - body.position[1],
                feet[2] - body.position[2],
            ];
            let length = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
            let closeness = 1.0 - length / 8.0;
            let pull = crate::mobs::body::hurt::normalize(to);
            let by = closeness * closeness * 0.1;
            body.motion = physics::add(body.motion, [pull[0] * by, pull[1] * by, pull[2] * by]);
        }
        let view = View {
            region: self,
            home: position,
        };
        let before_y = body.motion[1];
        let motion = body.motion;
        body.move_with(&view, false, motion, None, BOUNCE, &mut |_| {});
        let mut friction = 0.98_f32;
        if body.on_ground {
            let below = view.state(body.on_pos(&view, 0.999_999));
            friction *= physics_of(below).map_or(0.6, |physics| physics.friction);
        }
        body.motion = physics::scale(body.motion, f64::from(friction));
        if body.vertical_collision_below && before_y < -GRAVITY {
            body.motion = [body.motion[0], -before_y * 0.4, body.motion[2]];
        }
        self.entity_drew(key, stream.draws_since(&start));
        let mut total = count;
        let mut youngest = orb_age(&entity);
        for (other, chunk, age, count) in merged {
            if self.entity_record(other, chunk).is_none() {
                continue;
            }
            total = total.saturating_add(count);
            youngest = youngest.min(age);
            let hops = self.entity_record(other, chunk).map_or(0, |orb| orb.hops);
            self.remove_entity(other, hops, chunk, output);
        }
        let lived = entity.lived.saturating_add(1);
        let age = youngest.saturating_add(1);
        if age >= LIFETIME {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let after = Entity {
            position: body.position,
            motion: body.motion,
            on_ground: body.on_ground,
            fall_distance: body.fall_distance,
            lived,
            data: EntityData::ExperienceOrb {
                value,
                count: total,
                health,
                age_offset: lived.saturating_sub(age),
                following,
            },
            ..entity.clone()
        };
        self.entity_memory.insert(
            key,
            Memory {
                supporting: body.supporting,
                on_ground_no_blocks: body.on_ground_no_blocks,
                last_movements: Vec::new(),
                stuck_speed: body.stuck_speed,
                no_physics: false,
                ticks_since_check: 0,
                cart: Default::default(),
                boat: Default::default(),
                steps: Default::default(),
            },
        );
        self.write_orb(&entity, after, position, output);
    }

    /// What an orb's tick leaves: its record, committed where it moved or changed (its
    /// age is its counter, kept in place), and its report.
    fn write_orb(
        &mut self,
        before: &Entity,
        mut after: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        let to = match self.relocate(position, before.position, &mut after, output) {
            Relocated::Sent => return,
            Relocated::Moved(to) => to,
            Relocated::Stays => position,
        };
        let bits = |values: Vec3| values.map(f64::to_bits);
        let moved = bits(after.position) != bits(before.position);
        let mut fields = Vec::new();
        if to == position {
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
            let counted = |data: &EntityData| match data {
                EntityData::ExperienceOrb {
                    count, following, ..
                } => Some((*count, *following)),
                _ => None,
            };
            if counted(&after.data) != counted(&before.data) {
                fields.push(EntityField::Data(after.data.clone()));
            }
            if !fields.is_empty() {
                fields.push(EntityField::Lived(after.lived));
                self.commit_entity(position, key, fields.clone(), output);
            }
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        if !fields.is_empty() || to != position {
            let mut report = tracked(&after, 0, false);
            report.chunk = to;
            output.events.push(RegionEvent::EntityTracked(report));
        }
    }

    /// `followNearbyPlayer`'s choice: the player followed, kept while within 8 blocks;
    /// else the nearest of the region's players within 8 (`getNearestPlayer`, the first
    /// of the nearest in the region's order), if any.
    fn followed(&self, at: &Vec3, following: Option<EntityId>) -> Option<EntityId> {
        let distance = |feet: Vec3| physics::distance_sqr(feet, *at);
        if let Some(player) = following
            && let Some((feet, _)) = self.player_feet_of(player)
            && distance(feet) <= 64.0
        {
            return Some(player);
        }
        let mut nearest: Option<(f64, EntityId)> = None;
        for player in self.players.values() {
            let feet = [
                player.pose.position.x,
                player.pose.position.y,
                player.pose.position.z,
            ];
            let d = distance(feet);
            if d <= 64.0 && nearest.is_none_or(|(least, _)| d < least) {
                nearest = Some((d, player.entity_id));
            }
        }
        nearest.map(|(_, player)| player)
    }

    /// The feet and the eyes' height of the region's player whose entity is `player`.
    fn player_feet_of(&self, player: EntityId) -> Option<(Vec3, f64)> {
        let record = self
            .players
            .values()
            .find(|record| record.entity_id == player)?;
        let at = record.pose.position;
        Some(([at.x, at.y, at.z], super::EYE_HEIGHT))
    }

    /// The players' touch of orbs (`Player.aiStep` → `ExperienceOrb.playerTouch`), after
    /// their items', in the order of the players: each takes, once in two ticks, one orb
    /// of the region's in their box grown by a block sideways and half a block up and
    /// down, and is given its worth; clients see the take; an orb whose count is spent
    /// is gone.
    pub(crate) fn players_touch_orbs(&mut self, output: &mut TickOutput) {
        let players: Vec<(clustine_world::PlayerId, EntityId, Vec3)> = self
            .players
            .iter()
            .map(|(id, player)| {
                let at = player.pose.position;
                (*id, player.entity_id, [at.x, at.y, at.z])
            })
            .collect();
        for (id, collector, feet) in players {
            let reach = Aabb::of_entity(feet, 0.6, 1.8).inflate([1.0, 0.5, 1.0]);
            for (key, chunk) in self.entities_in(&reach) {
                if self
                    .orbs_taken
                    .get(&id)
                    .is_some_and(|&taken| self.tick < taken.saturating_add(2))
                {
                    break;
                }
                let Some(record) = self.entity_record(key, chunk).cloned() else {
                    continue;
                };
                let EntityData::ExperienceOrb {
                    value,
                    count,
                    health,
                    age_offset,
                    following,
                } = record.data
                else {
                    continue;
                };
                self.orbs_taken.insert(id, self.tick);
                output.events.push(RegionEvent::ItemTaken {
                    item: key,
                    collector,
                    count: 1,
                    chunk,
                });
                // `repairPlayerItems`: nothing a player holds has mending.
                self.add_experience(id, value, output);
                let left = count - 1;
                if left <= 0 {
                    self.remove_entity(key, record.hops, chunk, output);
                } else {
                    let data = EntityData::ExperienceOrb {
                        value,
                        count: left,
                        health,
                        age_offset,
                        following,
                    };
                    self.commit_entity(chunk, key, vec![EntityField::Data(data)], output);
                }
            }
        }
    }

    /// `ExperienceOrb.hurtServer(source, amount)` of an explosion's: its health down by
    /// the damage, gone at nothing. Whether it is gone.
    pub(crate) fn hurt_orb(&mut self, key: EntityId, damage: f32, output: &mut TickOutput) -> bool {
        let Some((chunk, record)) = self.find_entity(key) else {
            return true;
        };
        let record = record.clone();
        let EntityData::ExperienceOrb {
            value,
            count,
            health,
            age_offset,
            following,
        } = record.data
        else {
            return false;
        };
        let left = (f32::from(health) - damage) as i32;
        if left <= 0 {
            self.remove_entity(key, record.hops, chunk, output);
            return true;
        }
        let data = EntityData::ExperienceOrb {
            value,
            count,
            health: i16::try_from(left).unwrap_or(i16::MAX),
            age_offset,
            following,
        };
        self.commit_entity(chunk, key, vec![EntityField::Data(data)], output);
        false
    }
}

/// The world's lowest block for an orb's void test.
fn item_min_y(view: &View<'_>) -> i32 {
    crate::physics::item::Region::min_y(view)
}

/// `isEyeInFluid(#water)`: water at its eyes, as deep as their height.
fn eye_in_water(world: &View<'_>, eyes: Vec3) -> bool {
    let pos = physics::containing(eyes);
    let state = world.state(pos);
    if !clustine_blocks::data::has_water(state) {
        return false;
    }
    let fluid = FluidState::of(state);
    let height = clustine_blocks::fluid::height(&mut physics::Rules(world), fluid, pos);
    eyes[1] <= f64::from(pos.y) + f64::from(height)
}
