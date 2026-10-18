//! Entities other than players (`docs/adr/0028-entities.md`, sections 2 and 4;
//! `docs/groundwork/n2-plan.md`, sections 7 and 11): they live in their chunks, the
//! region keeps which chunk each is in, they tick after the block events in ascending
//! key, and what a tick does to them goes into its output to be committed.
//!
//! The one entity so far is the item a player throws. It lies where it was thrown until
//! N4 brings physics: its tick only counts its age and its pick-up delay down, which
//! the region keeps in the chunk and commits nothing for, so that an item at rest costs
//! the log nothing. Its counters are kept as counts and not as ticks of the region, so
//! that what a chunk holds is always as of its last tick and nothing needs rebasing
//! when another region takes it in; while its chunk does not tick they stand still, as
//! the game's do. A crash loses what they counted since the item's last record: as if
//! its chunk had not been loaded for those ticks, a difference the plan names.

use clustine_data::{ITEMS, entity_types};
use clustine_noise::trig;
use clustine_world::{
    ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId, MAX_CHUNK_ENTITIES, PlayerId,
};
use uuid::Uuid;

use super::Region;
use super::random_ticks::Random;
use crate::api::{EntityKind, EntityState, ItemStack, Pose, RegionEvent, TickOutput};

/// How far above a player's feet their eyes are, standing and crouching: the game's
/// `Avatar` dimensions, which it keeps as floats.
const STANDING_EYES: f32 = 1.62;
const CROUCHING_EYES: f32 = 1.27;

/// How far below the eyes an item leaves from (`LivingEntity.createItemStackToDrop`).
const BELOW_THE_EYES: f64 = 0.300_000_011_920_928_96;

/// The ticks before a thrown item may be picked up.
const THROWN_PICK_UP_DELAY: u16 = 40;

/// The pick-up delay the game reads as never, which does not count down.
const NEVER_PICKED_UP: u16 = 32_767;

/// The game's `Mth.DEG_TO_RAD`, a float.
const DEGREES_TO_RADIANS: f32 = 0.017_453_292;

/// How many draws a throw takes from the thrower's stream.
const THROW_DRAWS: usize = 4;

/// The motion the game gives an item a player throws, `LivingEntity.createItemStackToDrop`
/// without its scattering, from the thrower's yaw and pitch in degrees and the four
/// floats it draws from the thrower's random, in the order drawn: straight ahead at 0.3
/// and a little up, and off that in a direction of the first draw by up to 0.02 by the
/// second; the last two lift it or drop it a little. The yaw and the pitch go through
/// `Mth`'s table of sines and the direction through `StrictMath`'s sine and cosine (the
/// game's `Math`, which the plan takes as `StrictMath`, ADR-0028 section 4), each in the
/// game's widths and order.
pub(crate) fn throw_motion(yaw: f32, pitch: f32, draws: [f32; THROW_DRAWS]) -> [f64; 3] {
    let sin_pitch = trig::sin(f64::from(pitch * DEGREES_TO_RADIANS));
    let cos_pitch = trig::cos(f64::from(pitch * DEGREES_TO_RADIANS));
    let sin_yaw = trig::sin(f64::from(yaw * DEGREES_TO_RADIANS));
    let cos_yaw = trig::cos(f64::from(yaw * DEGREES_TO_RADIANS));
    let direction = f64::from(draws[0] * 6.283_185_5);
    let spread = f64::from(0.02 * draws[1]);
    [
        f64::from(-sin_yaw * cos_pitch * 0.3) + libm::cos(direction) * spread,
        f64::from(-sin_pitch * 0.3 + 0.1 + (draws[2] - draws[3]) * 0.1),
        f64::from(cos_yaw * cos_pitch * 0.3) + libm::sin(direction) * spread,
    ]
}

/// A UUID of version 4 from two draws, as the game's `Mth.createInsecureUUID` makes one
/// of two longs.
fn insecure_uuid(most: u64, least: u64) -> Uuid {
    Uuid::from_u64_pair(
        most & !0xF000 | 0x4000,
        least & 0x3FFF_FFFF_FFFF_FFFF | 0x8000_0000_0000_0000,
    )
}

/// The state an edge is shown of `entity`.
fn state_of(entity: &Entity) -> EntityState {
    EntityState {
        entity: entity.key,
        kind: EntityKind::Other {
            kind: entity.kind,
            uuid: entity.uuid,
            motion: entity.motion,
            hops: entity.hops,
            data: entity.data.clone(),
        },
        pose: Pose {
            position: clustine_world::Vec3::new(
                entity.position[0],
                entity.position[1],
                entity.position[2],
            ),
            yaw: entity.yaw,
            pitch: entity.pitch,
            on_ground: entity.on_ground,
        },
    }
}

/// What the region's tick reads and writes of one thing a player threw.
pub(crate) struct Thrown {
    pub(crate) player: PlayerId,
    pub(crate) stack: ItemStack,
    pub(crate) crouching: bool,
}

impl Region {
    /// The entities other than players of the loaded chunks, in ascending key.
    pub(crate) fn other_entities(&self) -> impl Iterator<Item = EntityState> + '_ {
        self.entity_chunks.iter().filter_map(|(key, chunk)| {
            let chunk = self.chunks.get(chunk)?;
            let entities = chunk.entities();
            let at = entities
                .binary_search_by_key(key, |entity| entity.key)
                .ok()?;
            Some(state_of(&entities[at]))
        })
    }

    /// The entity other than a player with the key `key`, if a loaded chunk has it.
    pub(crate) fn other_entity(&self, key: EntityId) -> Option<EntityState> {
        let chunk = self.chunks.get(self.entity_chunks.get(&key)?)?;
        let entities = chunk.entities();
        let at = entities
            .binary_search_by_key(&key, |entity| entity.key)
            .ok()?;
        Some(state_of(&entities[at]))
    }

    /// Takes in the entities of the chunk at `position`, loaded: they are indexed, and,
    /// unless `continued`, taken in by this region, which raises each one's changes of
    /// holder and commits it whole (ADR-0028, section 6). Every load from the store is
    /// a take-in, a restore's among them, and so is the load of a warm chunk after a
    /// merge or a split: a region that a merge or a split made is what a restore makes
    /// of the store's record (ADR-0014), and cannot tell the one from the other. The
    /// plan would have a merge raise nothing; that it does is one record of each of
    /// its entities more, and the edge takes the higher word as it would the equal.
    /// What continues is a region made with its chunks loaded
    /// ([`Region::restore_loaded`]).
    pub(crate) fn take_up_entities(
        &mut self,
        position: ChunkPos,
        continued: bool,
        output: Option<&mut TickOutput>,
    ) {
        let Some(chunk) = self.chunks.get_mut(&position) else {
            return;
        };
        let keys: Vec<EntityId> = chunk.entities().iter().map(|entity| entity.key).collect();
        if !continued && let Some(output) = output {
            let raised: Vec<Entity> = chunk
                .entities()
                .iter()
                .map(|entity| Entity {
                    hops: entity.hops.saturating_add(1),
                    ..entity.clone()
                })
                .collect();
            for entity in raised {
                let change = EntityChange::Whole(entity);
                chunk.apply_entity(&change);
                output.entity_changes.push((position, change));
            }
        }
        for key in keys {
            self.entity_chunks.insert(key, position);
        }
    }

    /// Forgets the entities of the chunk at `position`, which is no longer loaded.
    pub(crate) fn let_go_entities(&mut self, position: ChunkPos) {
        self.entity_chunks.retain(|_, chunk| *chunk != position);
    }

    /// Applies what later commits did to the entities of copied chunks. A whole entity
    /// outside the chunk it is for is passed over: the holder's chunk would not have it.
    pub(crate) fn update_copied_entities(&mut self, updates: &[(ChunkPos, Vec<EntityChange>)]) {
        for (position, changes) in updates {
            let Some(copy) = self.copies.get_mut(position) else {
                continue;
            };
            for change in changes {
                if let EntityChange::Whole(entity) = change
                    && entity.chunk() != *position
                {
                    continue;
                }
                copy.apply_entity(change);
            }
        }
    }

    /// The entities' phase of the tick (ADR-0028, section 2): every entity whose chunk
    /// ticks, in ascending key. An item counts its age up and its pick-up delay down
    /// (`ItemEntity.tick`); it does nothing else until N4, and commits nothing.
    pub(crate) fn run_entities(&mut self) {
        let ticking: Vec<(EntityId, ChunkPos)> = self
            .entity_chunks
            .iter()
            .filter(|(_, chunk)| self.ticks_chunk(**chunk))
            .map(|(key, chunk)| (*key, *chunk))
            .collect();
        for (key, position) in ticking {
            let Some(chunk) = self.chunks.get_mut(&position) else {
                continue;
            };
            let entities = chunk.entities();
            let Ok(at) = entities.binary_search_by_key(&key, |entity| entity.key) else {
                continue;
            };
            let entity = &entities[at];
            let mut fields = vec![EntityField::Age(entity.age.saturating_add(1))];
            match &entity.data {
                EntityData::Item {
                    stack,
                    pick_up_delay,
                    thrower,
                } => {
                    if *pick_up_delay > 0 && *pick_up_delay != NEVER_PICKED_UP {
                        fields.push(EntityField::Data(EntityData::Item {
                            stack: *stack,
                            pick_up_delay: pick_up_delay - 1,
                            thrower: *thrower,
                        }));
                    }
                }
            }
            chunk.apply_entity(&EntityChange::Set(key, fields));
        }
    }

    /// `count` floats from the stream of the entity `key` in this tick, on from what it
    /// has drawn in the tick already.
    fn draw_floats<const N: usize>(&mut self, key: EntityId) -> [f32; N] {
        let drawn = self.entity_draws.entry(key).or_default();
        let mut random = Random::of_entity(self.config.seed, key, self.tick);
        for _ in 0..*drawn {
            random.next_u64();
        }
        *drawn += N as u64;
        std::array::from_fn(|_| random.next_float())
    }

    /// A player throws `stack`, as the game's `LivingEntity.drop` and
    /// `createItemStackToDrop` do: an item at their x and z and a little below their
    /// eyes, with the motion of [`throw_motion`], the pick-up delay of a throw and the
    /// player as its thrower, under the region's next key; and their arm swings for
    /// everyone else. False, and nothing made, where the region cannot keep the item:
    /// its chunk is not loaded or does not tick (as when it is being handed over or
    /// lent), it is full ([`MAX_CHUNK_ENTITIES`]), or the region has no key left
    /// (ADR-0028, section 1).
    pub(crate) fn throw(&mut self, thrown: Thrown, output: &mut TickOutput) -> bool {
        let Thrown {
            player,
            stack,
            crouching,
        } = thrown;
        let Some(thrower) = self.players.get(&player) else {
            return false;
        };
        // Only stacks of items that exist are thrown.
        let known =
            stack.count > 0 && usize::try_from(stack.item).is_ok_and(|item| item < ITEMS.len());
        if !known {
            return false;
        }
        let pose = thrower.pose;
        let thrower_key = thrower.entity_id;
        let thrower_chunk = thrower.chunk();
        // A player who flies does not crouch, whatever the key says.
        let eyes = if crouching && !thrower.flying {
            CROUCHING_EYES
        } else {
            STANDING_EYES
        };
        let position = [
            pose.position.x,
            pose.position.y + f64::from(eyes) - BELOW_THE_EYES,
            pose.position.z,
        ];
        let chunk = ChunkPos::containing(position[0], position[2]);
        let room = self
            .chunks
            .get(&chunk)
            .is_some_and(|loaded| loaded.entities().len() < MAX_CHUNK_ENTITIES);
        let key = self.next_entity_id;
        if !room || !self.ticks_chunk(chunk) || !self.entity_ids.contains(key) {
            return false;
        }
        self.next_entity_id = EntityId(key.0 + 1);
        self.journal.next_entity_id = true;

        let motion = throw_motion(pose.yaw, pose.pitch, self.draw_floats(thrower_key));
        // The item's own draws at its birth: its UUID (the game's two longs), the bob
        // of its model, which only clients show, and its yaw (`ItemEntity`'s
        // constructors).
        let mut own = Random::of_entity(self.config.seed, key, self.tick);
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _bob = own.next_float();
        let yaw = own.next_float() * 360.0;
        self.entity_draws.insert(key, 4);
        let entity = Entity {
            key,
            kind: u16::try_from(entity_types::ITEM).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion,
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            age: 0,
            data: EntityData::Item {
                stack,
                pick_up_delay: THROWN_PICK_UP_DELAY,
                thrower: Some(player.0),
            },
        };
        output.events.push(RegionEvent::ArmSwung {
            entity: thrower_key,
            chunk: thrower_chunk,
        });
        output
            .events
            .push(RegionEvent::EntitySpawned(state_of(&entity)));
        let change = EntityChange::Whole(entity);
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&change);
        }
        self.entity_chunks.insert(key, chunk);
        output.entity_changes.push((chunk, change));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's sine table at an angle, worked out here as the game fills the table
    /// (`Mth.SIN`, the sine of each 65,536th of a turn) and looks it up, without the
    /// table of the noise crate.
    fn table_sin(radians: f32) -> f32 {
        let index = (f64::from(radians) * 10_430.378_350_470_453) as i64 & 0xFFFF;
        libm::sin(index as f64 / 10_430.378_350_470_453) as f32
    }

    fn table_cos(radians: f32) -> f32 {
        let index = (f64::from(radians) * 10_430.378_350_470_453 + 16_384.0) as i64 & 0xFFFF;
        libm::sin(index as f64 / 10_430.378_350_470_453) as f32
    }

    #[test]
    fn an_item_thrown_looking_ahead_flies_ahead_at_three_tenths_scattered_by_its_draws() {
        // Looking along positive z, level: the sine of 0 is 0 and its cosine 1, so the
        // throw is 0.3 along z and 0.1 up, and the draws put it off that.
        let draws = [0.25, 0.5, 0.75, 0.25];
        let motion = throw_motion(0.0, 0.0, draws);
        let direction = f64::from(0.25_f32 * 6.283_185_5_f32);
        let spread = f64::from(0.02_f32 * 0.5_f32);
        assert_eq!(
            motion[0].to_bits(),
            (0.0 + libm::cos(direction) * spread).to_bits()
        );
        let lift = 0.0_f32 * 0.3 + 0.1 + (0.75_f32 - 0.25) * 0.1;
        assert_eq!(motion[1].to_bits(), f64::from(lift).to_bits());
        assert_eq!(
            motion[2].to_bits(),
            (f64::from(0.3_f32) + libm::sin(direction) * spread).to_bits()
        );
        // The first draw's quarter turn scatters it along z too: 0.31 ahead.
        assert!((motion[0] - 0.0).abs() < 1e-6, "{motion:?}");
        assert!((motion[2] - 0.31).abs() < 1e-6, "{motion:?}");
    }

    #[test]
    fn an_item_thrown_to_the_side_and_upwards_follows_the_game_s_sine_table() {
        // Facing east (yaw -90, towards positive x) and looking up at 45 degrees: each
        // of the game's steps by hand.
        let (yaw, pitch) = (-90.0_f32, -45.0_f32);
        let draws = [0.0, 1.0, 0.0, 0.0];
        let sin_pitch = table_sin(pitch * 0.017_453_292);
        let cos_pitch = table_cos(pitch * 0.017_453_292);
        let sin_yaw = table_sin(yaw * 0.017_453_292);
        let cos_yaw = table_cos(yaw * 0.017_453_292);
        // The first draw 0 points the scatter along x, by the second's 0.02.
        let expected = [
            f64::from(-sin_yaw * cos_pitch * 0.3_f32) + 1.0 * f64::from(0.02_f32),
            f64::from(-sin_pitch * 0.3_f32 + 0.1_f32 + 0.0 * 0.1_f32),
            f64::from(cos_yaw * cos_pitch * 0.3_f32) + 0.0,
        ];
        let motion = throw_motion(yaw, pitch, draws);
        assert_eq!(motion.map(f64::to_bits), expected.map(f64::to_bits));
        // East and up: 0.21 along x and the scatter's 0.02, 0.31 up, nothing along z.
        assert!((motion[0] - 0.232).abs() < 1e-3, "{motion:?}");
        assert!((motion[1] - 0.312).abs() < 1e-3, "{motion:?}");
        assert!(motion[2].abs() < 1e-6, "{motion:?}");
    }

    #[test]
    fn an_insecure_uuid_is_of_version_4_and_the_ietf_variant() {
        let uuid = insecure_uuid(u64::MAX, u64::MAX);
        assert_eq!(uuid.get_version_num(), 4);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
        assert_eq!(
            insecure_uuid(0, 0),
            Uuid::from_u64_pair(0x4000, 0x8000_0000_0000_0000)
        );
    }
}
