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
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, EntityId, Generated};

use crate::mobs::types::{SpawnGroup, SpawnReason, Spawning};

use crate::region::Random;

/// Raised whenever the set of types made with chunks or by structures changes, or what
/// any of them is made with (section 4): a world made while fewer were made is refused
/// rather than continued with a seam. 0 while none is made.
pub const ENTITIES_GENERATION: u16 = 0;

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

/// The making of a generated entity, which keys its own stream
/// (`docs/groundwork/n7l-plan.md`, section 2.2): the run's centre (the spawn's for the
/// spawn's), its class and its place among what that run or spawn made, counting from
/// 0, in the parts of a key ([`Generated`]). Known when the entity is made, so that its
/// constructor, `construct` and `finalizeSpawn` draw from its own random in the game's
/// order there; its key is the chunk's it lands in, set by [`record`] (section 9 item 1).
pub(crate) fn making_stream(world: u64, making: Generated) -> Option<Random> {
    Some(Random::of_generated(world, making.key()?))
}

/// A mob of the type `kind` a structure's code makes at `position` facing `yaw` (the
/// swamp hut's witch, the monument's elders), made as an egg's is
/// ([`crate::mobs::making`]) and finalised for the reason `STRUCTURE`, with the biome
/// `biome` at its block and the level's `level` (the run's region random). Without its
/// key. `None` for a type that is no mob, or a making beyond the border.
#[expect(
    clippy::too_many_arguments,
    reason = "what a structure's code knows of its mob"
)]
pub fn mob(
    world: u64,
    making: Generated,
    kind: u16,
    structure: Option<&'static str>,
    biome: Option<&'static str>,
    position: [f64; 3],
    yaw: f32,
    level: &mut dyn Draws,
) -> Option<Entity> {
    let mut group = None;
    made_mob(
        world,
        making,
        kind,
        SpawnReason::Structure,
        &mut group,
        structure,
        biome,
        position,
        yaw,
        level,
    )
}

/// A mob made for `reason` with the group `group` of its spawn (none for the first of a
/// group, which leaves it what its `finalizeSpawn` returns), as [`mob`] makes one.
#[expect(
    clippy::too_many_arguments,
    reason = "what a generated mob's making reads"
)]
pub(crate) fn made_mob(
    world: u64,
    making: Generated,
    kind: u16,
    reason: SpawnReason,
    group: &mut Option<SpawnGroup>,
    structure: Option<&'static str>,
    biome: Option<&'static str>,
    position: [f64; 3],
    yaw: f32,
    level: &mut dyn Draws,
) -> Option<Entity> {
    // A type whose making is not built yet makes nothing, as its egg makes nothing.
    if !crate::mobs::types::is_built(kind) {
        return None;
    }
    let mut own = making_stream(world, making)?;
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, yaw);
    let at = crate::physics::containing(position);
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome,
            at,
            // A `WorldGenRegion` has no time layer: its day is the first, under a full
            // moon (`docs/groundwork/n7l-plan.md`, section 2.1), and its game time is
            // set at the first take (`MobType::first_take`).
            day: 0,
            game_time: 0,
            reason,
            group,
            structure,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
        },
    );
    Some(Entity {
        key: EntityId(0),
        kind,
        uuid,
        position,
        motion: [0.0; 3],
        yaw,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    })
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

/// The record of an entity a run or a spawn made, now that its key is `key`, the chunk's
/// it lands in: the key set. Its own draws were made at its making, from the stream of
/// its making ([`making_stream`]).
pub fn record(key: EntityId, mut entity: Entity) -> Entity {
    entity.key = key;
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
        record(key, entity)
    }

    fn mob(
        &self,
        kind: u16,
        making: Generated,
        biome: Option<&'static str>,
        position: [f64; 3],
        yaw: f32,
        draws: &mut dyn Draws,
    ) -> Option<Entity> {
        mob(self.world, making, kind, None, biome, position, yaw, draws)
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

    /// A generated mob is made from the stream of its making, as an egg's is from its
    /// own: the same making makes the same mob however often the chunk is made, another
    /// making another UUID; its record then only sets its key.
    #[test]
    fn a_generated_mob_is_made_from_its_makings_stream_and_its_record_sets_its_key() {
        let making = |index| clustine_world::Generated {
            dimension: 0,
            chunk: ChunkPos::new(4, -2),
            structure: true,
            index,
        };
        let make = |world, index| {
            let mut level = Random::of_entity(3, EntityId(5), 0);
            mob(
                world,
                making(index),
                clustine_data::entity_types::COW as u16,
                None,
                Some("minecraft:plains"),
                [64.5, 70.0, -30.5],
                90.0,
                &mut level,
            )
            .expect("a cow is made")
        };
        let made = make(11, 0);
        assert_eq!(make(11, 0), made);
        assert_ne!(make(11, 1).uuid, made.uuid);
        assert_ne!(make(12, 0).uuid, made.uuid);
        let mut own = making_stream(11, making(0)).expect("within the border");
        let (uuid, _) = crate::mobs::making::construct(made.kind, &mut own, 90.0);
        assert_eq!(made.uuid, uuid);
        assert_eq!(
            record(generated(7), made.clone()),
            Entity {
                key: generated(7),
                ..made
            }
        );
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
        let made = record(generated(3), item.clone());
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
