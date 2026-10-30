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
//! The spawning of a chunk is in `spawning`; the structures' entities, from their
//! templates' tags and their pieces' code, in `template`.

use clustine_data::BlockState;
use clustine_world::generation::{Draws, FinalHeightmap, Maker, Placed};
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, EntityId, Generated};

use crate::mobs::types::{SpawnGroup, SpawnReason, Spawning};

use crate::region::Random;

pub(crate) mod spawning;
mod template;
#[cfg(test)]
mod template_tests;

pub use spawning::SpawnRandom;

/// Raised whenever the set of types made with chunks or by structures changes, or what
/// any of them is made with (section 4): a world made while fewer were made is refused
/// rather than continued with a seam. 0 while none was made; 1 since a chunk is made
/// with the overworld's creatures (N7l); 2 since the structures make theirs, by every
/// type built (N7l's D3); 3 since a chunk's wolves are made with the most health their
/// load from the chunk gives them (2026-10-29).
pub const ENTITIES_GENERATION: u16 = 3;

/// What the spawning of a chunk reads (section 3.2): the lit chunk's blocks, its two
/// motion height maps and its light, and the biomes of the nine around it, which the
/// zoom at the corner looks at. The generator implements it over the lit centre and
/// the nine finished chunks.
pub trait SpawnView {
    /// The chunk being spawned in.
    fn position(&self) -> ChunkPos;
    /// The highest block's y (`getMaxY()`): the spawn reads the biomes at the top of
    /// the chunk's corner.
    fn max_y(&self) -> i32;
    /// The dimension's sea level, below which and four more a turtle may spawn.
    fn sea_level(&self) -> i32;
    /// The level's dimension, whose ambient light the light's worth reads: the
    /// overworld where a view knows no other.
    fn dimension(&self) -> clustine_world::Dimension {
        clustine_world::Dimension::Overworld
    }
    /// The block at `at`, in the centre or the eight around it (the box of a mob at the
    /// chunk's edge meets the shapes of blocks beside it); air beyond them or the
    /// dimension's height.
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

/// The spawning of a chunk (`NaturalSpawner.spawnMobsForChunkGeneration`, section
/// 3.2): the mobs made, without their keys, in the order the scheduler keys them as the
/// spawn's. `spawn` is W, the spawn's `WorldgenRandom` over a legacy source seeded with
/// `setDecorationSeed`, which draws the groups, the columns and the yaws; `region` is
/// R, the region random, which every `finalizeSpawn` draws from. Each mob's own stream
/// is keyed by `world` and its making in the dimension numbered `dimension`
/// ([`making_stream`]). Whether the dimension makes mobs at all is the caller's.
pub fn spawn_chunk(
    view: &dyn SpawnView,
    world: u64,
    dimension: u8,
    spawn: &mut dyn Draws,
    region: &mut dyn Draws,
) -> Vec<Entity> {
    spawning::spawn_chunk(view, world, dimension, spawn, region)
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

/// A mob of the type `kind` a structure's code makes (the swamp hut's witch, the
/// monument's elders), made as an egg's is ([`crate::mobs::making`]), kept from
/// despawning where `persistent`, and finalised for the reason `STRUCTURE` with the
/// level's draws `placed` hands (the run's region random), in the world `world`. Its
/// unit, the vehicle first, each keyed by its making; nothing, having drawn nothing,
/// for a type not built.
pub fn mob(world: u64, kind: u16, persistent: bool, placed: Placed<'_>) -> Vec<Entity> {
    template::structure_mob(world, kind, persistent, placed)
}

/// An entity that is no mob a structure's code makes (a mineshaft's chest minecart),
/// keyed by its making; `None` for a type not made so.
pub fn entity(world: u64, kind: u16, placed: Placed<'_>) -> Option<Entity> {
    template::structure_entity(world, kind, placed)
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
            companions: None,
        },
    );
    loaded_from_its_chunk(kind, &mut mob);
    Some(Entity {
        portal: Default::default(),
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

/// What a mob a chunk is made with keeps of its round trip through the chunk: the game's
/// `WorldGenRegion.addFreshEntity` saves it into the proto chunk (`ProtoChunk.addEntity`
/// writes its tag) and the chunk loads it from that tag as it is promoted to a full one,
/// so its `readAdditionalSaveData` runs after its `finalizeSpawn`. Of the creatures a
/// chunk is made with, only the wolf's load changes what a client is shown:
/// `TamableAnimal`'s, finding no owner, calls `setTame(false, true)`, whose
/// `applyTamingSideEffects` makes the wolf's `max_health` instance (base 8), which is
/// sent with the wolf's spawn. The other types' loads set again what their tags hold.
fn loaded_from_its_chunk(kind: u16, mob: &mut crate::mobs::Mob) {
    if i32::from(kind) == clustine_data::entity_types::WOLF {
        crate::mobs::types::wolf::taming_side_effects(mob, false);
    }
}

/// An entity of a template from its tag (a compound with its `id`, in the binary form
/// of NBT without a name), as `placeEntities` makes it, finalised for the reason
/// `STRUCTURE` with the level's draws where `finalise` and it is a mob, in the world
/// `world`. Its unit, the vehicle first, each keyed by its making; nothing, having
/// drawn nothing, for what is not made so.
pub fn template_entity(world: u64, tag: &[u8], finalise: bool, placed: Placed<'_>) -> Vec<Entity> {
    template::template_entity(world, tag, finalise, placed)
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

    fn mob(&self, kind: u16, persistent: bool, placed: Placed<'_>) -> Vec<Entity> {
        mob(self.world, kind, persistent, placed)
    }

    fn entity(&self, kind: u16, placed: Placed<'_>) -> Option<Entity> {
        entity(self.world, kind, placed)
    }

    fn template_entity(&self, tag: &[u8], finalise: bool, placed: Placed<'_>) -> Vec<Entity> {
        template_entity(self.world, tag, finalise, placed)
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
            portal: Default::default(),
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
            let mut makings = clustine_world::generation::Makings::new(0, ChunkPos::new(4, -2));
            for _ in 0..index {
                makings.next();
            }
            let mut made = mob(
                world,
                clustine_data::entity_types::COW as u16,
                false,
                Placed {
                    position: [64.5, 70.0, -30.5],
                    yaw: 90.0,
                    pitch: 0.0,
                    structure: None,
                    biome: clustine_world::Biome(0),
                    makings: &mut makings,
                    level: &mut level,
                },
            );
            assert_eq!(made.len(), 1, "a cow is made alone");
            assert_eq!(
                made[0].key,
                making(index).key().unwrap(),
                "named by its making"
            );
            made.remove(0)
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
            stack: clustine_world::ItemStack::new(1, 1),
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
