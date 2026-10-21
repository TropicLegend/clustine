//! What generation calls of the simulation to make the entities of a chunk
//! (`docs/groundwork/n7l-plan.md`, sections 2.1 and 2.2): the mobs a chunk's spawn makes
//! and those its structures make, by the types' own code, called as pure functions. No
//! region, no clock and no I/O: generation stays a function of the seed and a chunk's
//! inputs (`docs/adr/0021-generation-is-a-function.md`).
//!
//! The level's draws reach the hooks through [`Draws`], which the simulation's own
//! stream implements as well, so that the same hook draws from a player's stream for an
//! egg and from the game's random sources for generation.
//!
//! What is here is the contract of the step's first part. The bodies of the creatures'
//! `finalizeSpawn` by reason and group, of the spawning of a chunk and of a template's
//! entities are the plan's later parts (sections 5.2 to 5.4): until then they make
//! nothing and draw nothing, and say so where they stand.

use clustine_data::BlockState;
use clustine_world::generation::{Draws, FinalHeightmap, Maker};
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, EntityId};

use crate::region::Random;

/// Raised whenever the set of types made with chunks or by structures changes, or what
/// any of them is made with (section 4): a world made while fewer were made is refused
/// rather than continued with a seam. 0 while none is made.
pub const ENTITIES_GENERATION: u16 = 0;

/// Why a mob is made, as the game's `EntitySpawnReason` says it to `finalizeSpawn`, of
/// the reasons generation has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// `CHUNK_GENERATION`: by the spawn of a chunk.
    ChunkGeneration,
    /// `STRUCTURE`: by a structure's piece, from its code or its template.
    Structure,
}

/// What the members of one group of the spawn share (`SpawnGroupData`): what the first
/// member's `finalizeSpawn` returned, which the later members are handed. Each type's
/// own (a herd's variant, a pack's) comes with the creatures' hooks (section 5.2).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Group {
    /// `AgeableMobGroupData`: how many of the group have been made, and the chance that
    /// a later one is a baby.
    Ageable { made: u32, baby_chance: f32 },
}

/// What the spawning of a chunk reads (section 3.2): the lit chunk's blocks, its two
/// motion height maps and its light, and the biomes of the nine around it, which the
/// zoom at the corner looks at. The generator implements it over the lit centre and
/// the nine finished chunks.
pub trait SpawnView {
    /// The chunk being spawned in.
    fn position(&self) -> ChunkPos;
    fn block(&self, at: BlockPos) -> BlockState;
    /// One of the final height maps (the spawning reads the two of motion): the y above
    /// the highest block that counts.
    fn height(&self, map: FinalHeightmap, x: i32, z: i32) -> i32;
    /// The sky's light at `at`, with the game's meaning of a section left out.
    fn sky_light(&self, at: BlockPos) -> u8;
    /// The blocks' light at `at`, with the game's meaning of a section left out.
    fn block_light(&self, at: BlockPos) -> u8;
    /// The biome at `at` as the game's zoom finds it (`BiomeManager.getBiome`).
    fn zoomed_biome(&self, at: BlockPos) -> Biome;
    /// The noise biome of the cell of `at` (`getNoiseBiomeAtPosition`).
    fn noise_biome(&self, at: BlockPos) -> Biome;
}

/// The spawning of a chunk (`NaturalSpawner.spawnMobsForChunkGeneration`): the mobs
/// made, without their keys, drawing the spawn's own random from `seed` and every
/// `finalizeSpawn` from the region random `region`.
///
/// Section 5.3's: until then it makes none and draws nothing.
pub fn spawn_chunk(view: &dyn SpawnView, seed: i64, region: &mut dyn Draws) -> Vec<Entity> {
    let _ = (view, seed, region);
    Vec::new()
}

/// A mob of the type `kind` made for `reason` at `position` facing `yaw`, finalised
/// with the level's `draws`, as a member of `group` where it is one: its record without
/// its key and without what it draws from its own stream, which [`record`] adds. `None`
/// for a type that is not made so, having drawn nothing.
///
/// The creatures' `finalizeSpawn` by reason and group are section 5.2's: until then it
/// makes none.
pub fn mob(
    kind: u16,
    reason: Reason,
    group: Option<&mut Group>,
    position: [f64; 3],
    yaw: f32,
    draws: &mut dyn Draws,
) -> Option<Entity> {
    let _ = (kind, reason, group, position, yaw, draws);
    None
}

/// An entity of a template from its tag (a compound with its `id`, in the binary form
/// of NBT without a name), at `position` facing `yaw`, finalised for the reason
/// `STRUCTURE` with `draws` where `finalise` and it is a mob. Without its key. `None`
/// for what is not made so, having drawn nothing.
///
/// Section 5.4's: until then it makes none.
pub fn template_entity(
    tag: &[u8],
    position: [f64; 3],
    yaw: f32,
    finalise: bool,
    draws: &mut dyn Draws,
) -> Option<Entity> {
    let _ = (tag, position, yaw, finalise, draws);
    None
}

/// The record of an entity a run or a spawn made, now that its key is `key`: the key
/// set, and what the entity draws from its own stream at its making drawn from the
/// stream of the world `world` and `key` (section 2.2).
pub fn record(world: u64, key: EntityId, mut entity: Entity) -> Entity {
    entity.key = key;
    let Ok(mut mob) = crate::mobs::Mob::decode(&entity.data) else {
        return entity;
    };
    // What a mob's making draws from its own random, as `Region::new_mob` does for an
    // egg's: its UUID, its constructor's yaw, then its type's `construct` (a brain's
    // sensors' delays, a squid's strokes), from a stream of its own that no region
    // counts (`docs/groundwork/n7l-plan.md`, sections 2.2 and 9 item 10).
    let mut own = Random::of_generated(world, key);
    entity.uuid = crate::region::insecure_uuid(own.next_u64(), own.next_u64());
    let _constructor_yaw = own.next_float();
    crate::mobs::brain::construct(crate::mobs::types::of(entity.kind), &mut mob, &mut own);
    entity.data = mob.encode();
    entity
}

/// The simulation's maker of generated entities, for the world `world`: what
/// generation hands its runs and keys their entities with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation {
    pub world: u64,
}

impl Maker for Generation {
    fn record(&self, key: EntityId, entity: Entity) -> Entity {
        record(self.world, key, entity)
    }

    fn mob(
        &self,
        kind: u16,
        position: [f64; 3],
        yaw: f32,
        draws: &mut dyn Draws,
    ) -> Option<Entity> {
        mob(kind, Reason::Structure, None, position, yaw, draws)
    }

    fn template_entity(
        &self,
        tag: &[u8],
        position: [f64; 3],
        yaw: f32,
        finalise: bool,
        draws: &mut dyn Draws,
    ) -> Option<Entity> {
        template_entity(tag, position, yaw, finalise, draws)
    }
}

/// The simulation's own stream draws as it always does: an egg's mob draws its
/// `finalizeSpawn` from it as before.
impl Draws for Random {
    fn next_int(&mut self, bound: u32) -> u32 {
        Random::next_int(self, bound)
    }

    fn next_float(&mut self) -> f32 {
        Random::next_float(self)
    }

    fn next_double(&mut self) -> f64 {
        Random::next_double(self)
    }

    fn next_bool(&mut self) -> bool {
        Random::next_bool(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated(index: u16) -> EntityId {
        clustine_world::Generated {
            dimension: 0,
            chunk: ChunkPos::new(4, -2),
            structure: false,
            index,
        }
        .key()
        .expect("within the border")
    }

    fn cow() -> Entity {
        Entity {
            key: EntityId(0),
            kind: clustine_data::entity_types::COW as u16,
            uuid: uuid::Uuid::nil(),
            position: [64.5, 70.0, -30.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: crate::mobs::Mob::fresh().encode(),
        }
    }

    /// A generated mob's record: its key, and a UUID drawn from a stream of its own
    /// keyed by it, the same however often the chunk is made and another for another
    /// key; the stream is no egg's (`Random::of_entity`).
    #[test]
    fn a_generated_mobs_record_is_keyed_and_its_uuid_drawn_from_its_own_stream() {
        let made = record(11, generated(0), cow());
        assert_eq!(made.key, generated(0));
        assert_ne!(made.uuid, uuid::Uuid::nil());
        assert_eq!(record(11, generated(0), cow()), made);
        assert_ne!(record(11, generated(1), cow()).uuid, made.uuid);
        assert_ne!(record(12, generated(0), cow()).uuid, made.uuid);
        let mut egg = Random::of_entity(11, generated(0), 0);
        let egg_uuid = crate::region::insecure_uuid(egg.next_u64(), egg.next_u64());
        assert_ne!(egg_uuid, made.uuid);
    }

    /// An entity that is no mob is given its key only.
    #[test]
    fn a_record_of_no_mob_is_given_its_key_only() {
        let mut item = cow();
        item.data = clustine_world::EntityData::Item {
            stack: clustine_world::ItemStack { item: 1, count: 1 },
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        };
        let made = record(11, generated(3), item.clone());
        assert_eq!(
            made,
            Entity {
                key: generated(3),
                ..item
            }
        );
    }

    #[test]
    fn the_simulation_s_stream_draws_the_same_through_draws() {
        let mut through = Random::of_entity(7, EntityId(9), 3);
        let mut direct = through.clone();
        let draws: &mut dyn Draws = &mut through;
        assert_eq!(draws.next_int(13), direct.next_int(13));
        assert_eq!(draws.next_float(), direct.next_float());
        assert_eq!(draws.next_double(), direct.next_double());
        assert_eq!(draws.next_bool(), direct.next_bool());
    }
}
