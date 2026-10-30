//! The spawning of a chunk (`NaturalSpawner.spawnMobsForChunkGeneration`,
//! `docs/groundwork/n7-brief.md`, sections 8.3 to 8.6, and `docs/groundwork/n7l-plan.md`,
//! section 3.2): the animals a chunk is made with, from the spawn's random W and the
//! region random R, against a [`SpawnView`] of the lit chunk and the nine around it.
//!
//! Every draw of W is the game's, in the game's order, with the quirk of the brief's
//! 8.0 item 6 kept: an attempt that fails at the box, at the type's rule or at the
//! making tries the same column again without a step. None of the rules draws; every
//! `finalizeSpawn` draws from R.

use clustine_blocks::data::{has_lava, has_water, in_tag, is_block};
use clustine_data::spawning::{self, Heightmap, Placement};
use clustine_data::{BIOME_SPAWNS, BlockState, ENTITY_TYPE_INFO, PathComputation, blocks};
use clustine_data::{entity_types, spawns::Spawner};
use clustine_noise::{LegacyRandom, Random as _};
use clustine_world::generation::{Draws, FinalHeightmap, FromNoise};
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, Generated};

use super::{SpawnView, made_mob};
use crate::mobs::types::{SpawnGroup, SpawnReason};
use crate::physics::aabb::Aabb;
use crate::physics::collide::{self, Context};
use crate::physics::{self, World};

/// `CollisionContext.empty()`: a region's `noCollision(box)` asks for no entity.
const NO_ENTITY: Context = Context {
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// The default world border's half size: what `WorldBorder.isWithinBounds` holds a
/// spawn's block within.
const BORDER: f64 = 29_999_984.0;

/// The spawning of the chunk of `view`, in the dimension numbered `dimension` of the
/// world `world` (whose streams key each mob's own draws): W is `spawn`, R `region`.
pub(crate) fn spawn_chunk(
    view: &dyn SpawnView,
    world: u64,
    dimension: u8,
    spawn: &mut dyn Draws,
    region: &mut dyn Draws,
) -> Vec<Entity> {
    let chunk = view.position();
    let (min_x, min_z) = (chunk.x * 16, chunk.z * 16);
    // `getWorldPosition().atY(getMaxY())`, read at the block's centre: the list from
    // the zoomed biome there, the probability from the noise biome (B 8.0 item 4).
    let corner = BlockPos::new(min_x, view.max_y(), min_z);
    let Some(list) = spawns_of(view.zoomed_biome(corner)).and_then(|biome| biome.of("creature"))
    else {
        return Vec::new();
    };
    if list.is_empty() {
        return Vec::new();
    }
    let probability =
        spawns_of(view.noise_biome(corner)).map_or(0.1, |biome| biome.creature_probability);
    let total: i32 = list.iter().map(|entry| entry.weight).sum();
    let mut made = Vec::new();
    while spawn.next_float() < probability {
        // `WeightedList.getRandom`: nothing, and no draw, for a list of no weight.
        let Some(entry) = pick(list, total, spawn) else {
            continue;
        };
        let Ok(kind) = u16::try_from(entry.kind) else {
            continue;
        };
        // `IntProvider.sample`: a uniform count draws, a constant does not. The data
        // keeps a count as its two ends; no biome has a uniform one of equal ends.
        let count = if entry.min_count == entry.max_count {
            entry.min_count
        } else {
            spawn.next_int_between(entry.min_count, entry.max_count)
        };
        let mut group = None;
        let mut x = min_x + spawn.next_int(16) as i32;
        let mut z = min_z + spawn.next_int(16) as i32;
        let (first_x, first_z) = (x, z);
        for _ in 0..count {
            let mut spawned = false;
            let mut attempt = 0;
            while !spawned && attempt < 4 {
                attempt += 1;
                let making = Generated {
                    dimension,
                    chunk,
                    structure: false,
                    index: u16::try_from(made.len()).unwrap_or(u16::MAX),
                };
                let tried = Attempt {
                    view,
                    world,
                    kind,
                    making,
                    min_x,
                    min_z,
                }
                .run(x, z, spawn, region, &mut group);
                match tried {
                    // Failed at the box, the type's rule or the making: the same column
                    // again, with no step (NaturalSpawner.txt 969–1012 to 1112).
                    Tried::Again => continue,
                    Tried::Stepped => {}
                    Tried::Made(entity) => {
                        made.push(*entity);
                        spawned = true;
                    }
                }
                x += spawn.next_int(5) as i32 - spawn.next_int(5) as i32;
                z += spawn.next_int(5) as i32 - spawn.next_int(5) as i32;
                while !(min_x <= x && x < min_x + 16 && min_z <= z && z < min_z + 16) {
                    x = first_x + spawn.next_int(5) as i32 - spawn.next_int(5) as i32;
                    z = first_z + spawn.next_int(5) as i32 - spawn.next_int(5) as i32;
                }
            }
        }
    }
    made
}

/// W, the spawn's random: the game's `WorldgenRandom` over a *legacy* source
/// (`NoiseBasedChunkGenerator.spawnOriginalMobs`, NoiseBasedChunkGenerator.txt
/// 1057–1063), seeded with `setDecorationSeed(seed, minX, minZ)`. A `WorldgenRandom`
/// passes every draw to the source it wraps, so it draws as `java.util.Random`. It is
/// not the features' generator, which wraps a xoroshiro source
/// (`clustine_noise::WorldgenRandom`, `applyBiomeDecoration`).
pub struct SpawnRandom(LegacyRandom);

impl SpawnRandom {
    /// W of the chunk `chunk` in the world of `seed`.
    pub fn new(seed: i64, chunk: ChunkPos) -> Self {
        let mut random = LegacyRandom::from_seed(0);
        random.set_seed(seed);
        let x_scale = random.next_i64() | 1;
        let z_scale = random.next_i64() | 1;
        let decoration = i64::from(chunk.x * 16)
            .wrapping_mul(x_scale)
            .wrapping_add(i64::from(chunk.z * 16).wrapping_mul(z_scale))
            ^ seed;
        random.set_seed(decoration);
        Self(random)
    }
}

impl Draws for SpawnRandom {
    fn next_int(&mut self, bound: u32) -> u32 {
        FromNoise(&mut self.0).next_int(bound)
    }

    fn next_float(&mut self) -> f32 {
        self.0.next_f32()
    }

    fn next_double(&mut self) -> f64 {
        self.0.next_f64()
    }

    fn next_bool(&mut self) -> bool {
        self.0.next_bool()
    }
}

/// What a biome makes, by its number.
fn spawns_of(biome: Biome) -> Option<&'static clustine_data::spawns::BiomeSpawns> {
    BIOME_SPAWNS.get(usize::from(biome.0))
}

/// `WeightedList.getRandom`: one `nextInt(total)`, the first entry whose weights so far
/// pass it.
fn pick<'a>(list: &'a [Spawner], total: i32, spawn: &mut dyn Draws) -> Option<&'a Spawner> {
    let total = u32::try_from(total).ok().filter(|total| *total > 0)?;
    let mut left = spawn.next_int(total) as i32;
    list.iter().find(|entry| {
        left -= entry.weight;
        left < 0
    })
}

/// How an attempt at one member ended.
enum Tried {
    /// Failed before the step: the same column again.
    Again,
    /// Failed after the placement or the mob's own rules: the step follows.
    Stepped,
    /// Made: the step follows too.
    Made(Box<Entity>),
}

/// One attempt at a member of a group.
struct Attempt<'v> {
    view: &'v dyn SpawnView,
    world: u64,
    kind: u16,
    making: Generated,
    min_x: i32,
    min_z: i32,
}

impl Attempt<'_> {
    fn run(
        &self,
        x: i32,
        z: i32,
        spawn: &mut dyn Draws,
        region: &mut dyn Draws,
        group: &mut Option<SpawnGroup>,
    ) -> Tried {
        let view = self.view;
        let kind = self.kind;
        let at = top_non_colliding_pos(view, kind, x, z);
        // `canSummon` holds for every creature.
        if !is_spawn_position_ok(view, kind, at) {
            return Tried::Stepped;
        }
        // The type's width, a float widened; the column's corner moved in by it where
        // it is at the chunk's edge (B 8.0 item 7).
        let width = f64::from(info(kind).width);
        let clamp = |value: i32, min: i32| {
            let (low, high) = (f64::from(min) + width, f64::from(min) + 16.0 - width);
            let value = f64::from(value);
            // `Mth.clamp(double, double, double)`.
            if value < low { low } else { value.min(high) }
        };
        let position = [clamp(x, self.min_x), f64::from(at.y), clamp(z, self.min_z)];
        if !no_collision(view, &spawn_box(kind, position)) {
            return Tried::Again;
        }
        let pos = physics::containing(position);
        if !check_spawn_rules(view, kind, pos) {
            return Tried::Again;
        }
        // `EntityType.create`: a type Clustine has not built, or a making beyond the
        // border, makes nothing, as the game's null.
        if !crate::mobs::types::is_built(kind) || self.making.key().is_none() {
            return Tried::Again;
        }
        let yaw = spawn.next_float() * 360.0;
        if !own_rules(view, kind, pos, position) {
            return Tried::Stepped;
        }
        let biome = spawns_of(view.zoomed_biome(pos)).map(|biome| biome.name);
        match made_mob(
            self.world,
            self.making,
            kind,
            SpawnReason::ChunkGeneration,
            group,
            None,
            biome,
            position,
            yaw,
            region,
        ) {
            Some(entity) => Tried::Made(Box::new(entity)),
            // Not reached: the making's failures are tested above.
            None => Tried::Stepped,
        }
    }
}

fn info(kind: u16) -> &'static clustine_data::EntityTypeInfo {
    &ENTITY_TYPE_INFO[usize::from(kind)]
}

/// `getTopNonCollidingPos`: the top of the type's height map at the column, moved by
/// its placement. The overworld has no ceiling.
pub(crate) fn top_non_colliding_pos(view: &dyn SpawnView, kind: u16, x: i32, z: i32) -> BlockPos {
    let (placement, heightmap) = spawning::placement(kind);
    let map = match heightmap {
        Heightmap::MotionBlocking => FinalHeightmap::MotionBlocking,
        Heightmap::MotionBlockingNoLeaves => FinalHeightmap::MotionBlockingNoLeaves,
        Heightmap::WorldSurface => FinalHeightmap::WorldSurface,
        Heightmap::OceanFloor => FinalHeightmap::OceanFloor,
    };
    let pos = BlockPos::new(x, view.height(map, x, z), z);
    adjust_spawn_position(view, placement, pos)
}

/// `SpawnPlacementType.adjustSpawnPosition`: on the ground, into the block below where
/// a mob may walk through it (snow below five layers, an open trapdoor; not a slab).
pub(crate) fn adjust_spawn_position(
    view: &dyn SpawnView,
    placement: Placement,
    pos: BlockPos,
) -> BlockPos {
    let below = pos.offset(0, -1, 0);
    if placement == Placement::OnGround && view.block(below).is_pathfindable(PathComputation::Land)
    {
        below
    } else {
        pos
    }
}

/// `WorldBorder.isWithinBounds(BlockPos)`.
fn within_border(pos: BlockPos) -> bool {
    let (x, z) = (f64::from(pos.x), f64::from(pos.z));
    (-BORDER..BORDER).contains(&x) && (-BORDER..BORDER).contains(&z)
}

/// `SpawnPlacements.isSpawnPositionOk`: the placement's test of the position.
pub(crate) fn is_spawn_position_ok(view: &dyn SpawnView, kind: u16, pos: BlockPos) -> bool {
    match spawning::placement(kind).0 {
        Placement::NoRestrictions => true,
        Placement::InWater => {
            within_border(pos)
                && has_water(view.block(pos))
                && !view
                    .block(pos.offset(0, 1, 0))
                    .props()
                    .is_redstone_conductor()
        }
        Placement::InLava => within_border(pos) && has_lava(view.block(pos)),
        Placement::OnGround => {
            within_border(pos)
                && spawning::is_valid_spawn(u32::from(view.block(pos.offset(0, -1, 0)).0), kind)
                && is_valid_empty_spawn_block(view.block(pos), kind)
                && is_valid_empty_spawn_block(view.block(pos.offset(0, 1, 0)), kind)
        }
    }
}

/// `NaturalSpawner.isValidEmptySpawnBlock`: no full block's collision, no signal, no
/// fluid, no rail and nothing dangerous to the type.
pub(crate) fn is_valid_empty_spawn_block(state: BlockState, kind: u16) -> bool {
    !state.voxel_collision_shape().is_block()
        && !state.props().is_signal_source()
        && state.props().fluid().is_none()
        && !in_tag(state, "minecraft:prevent_mob_spawning_inside")
        && !spawning::is_block_dangerous(kind, u32::from(state.0))
}

/// `getSpawnAABB(x, y, z)`: the type's box, its half width a float, at its feet. Every
/// creature's spawn scale is 1.
fn spawn_box(kind: u16, position: [f64; 3]) -> Aabb {
    let info = info(kind);
    Aabb::of_entity(position, info.width, info.height)
}

/// The view as the collider reads a world: every block the view has, no entities.
struct Blocks<'v>(&'v dyn SpawnView);

impl World for Blocks<'_> {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(self.0.block(pos))
    }

    fn has_chunk(&self, _chunk: ChunkPos) -> bool {
        true
    }
}

/// `noCollision(box)` of a region: no block's shape in the box (a region has no
/// entities). The border's shape is left out: no chunk beyond it is made.
fn no_collision(view: &dyn SpawnView, bounds: &Aabb) -> bool {
    collide::block_collisions(&Blocks(view), NO_ENTITY, bounds).is_empty()
}

/// `containsAnyLiquid(box)`. **Inferred** as elsewhere (`cushions.rs`): `findBlocksIn`
/// visits the blocks from the floor of the box's minimum to the floor of its maximum.
fn contains_any_liquid(view: &dyn SpawnView, bounds: &Aabb) -> bool {
    let low = physics::containing(bounds.min);
    let high = physics::containing(bounds.max);
    (low.x..=high.x).any(|x| {
        (low.y..=high.y).any(|y| {
            (low.z..=high.z).any(|z| view.block(BlockPos::new(x, y, z)).props().fluid().is_some())
        })
    })
}

/// `getRawBrightness(pos, 0)`: the greater of the blocks' light and the sky's, the
/// region's sky not darkened.
fn raw_brightness(view: &dyn SpawnView, pos: BlockPos) -> u8 {
    view.block_light(pos).max(view.sky_light(pos))
}

/// `Animal.isBrightEnoughToSpawn`.
fn bright(view: &dyn SpawnView, pos: BlockPos) -> bool {
    raw_brightness(view, pos) > 8
}

/// The type's spawn rule (`SpawnPlacements.checkSpawnRules` at `CHUNK_GENERATION`),
/// the brief's 8.6: none draws. `pos` is the block the mob's feet are in.
pub(crate) fn check_spawn_rules(view: &dyn SpawnView, kind: u16, pos: BlockPos) -> bool {
    use entity_types::*;
    let below = view.block(pos.offset(0, -1, 0));
    let on = |tag: &str| in_tag(below, tag) && bright(view, pos);
    let animal = || in_tag(below, "minecraft:animals_spawnable_on") && bright(view, pos);
    match i32::from(kind) {
        SHEEP | PIG | COW | CHICKEN | HORSE | DONKEY | MULE | LLAMA | CAT | HAPPY_GHAST | PANDA
        | TRADER_LLAMA => animal(),
        ARMADILLO => on("minecraft:armadillo_spawnable_on"),
        CAMEL => on("minecraft:camels_spawnable_on"),
        FROG => on("minecraft:frogs_spawnable_on"),
        GOAT => on("minecraft:goats_spawnable_on"),
        MOOSHROOM => on("minecraft:mooshrooms_spawnable_on"),
        PARROT => on("minecraft:parrots_spawnable_on"),
        RABBIT => on("minecraft:rabbits_spawnable_on"),
        WOLF => on("minecraft:wolves_spawnable_on"),
        FOX => on("minecraft:foxes_spawnable_on"),
        POLAR_BEAR => {
            let alternate = biome_in_tag(
                view.zoomed_biome(pos),
                "minecraft:polar_bears_spawn_on_alternate_blocks",
            );
            if alternate {
                bright(view, pos) && in_tag(below, "minecraft:polar_bears_spawnable_on_alternate")
            } else {
                animal()
            }
        }
        TURTLE => {
            pos.y < view.sea_level() + 4 && in_tag(below, "minecraft:sand") && bright(view, pos)
        }
        // `Mob.checkMobSpawnRules`: not a spawner's reason, so the block below.
        IRON_GOLEM | SNOW_GOLEM | VILLAGER | WANDERING_TRADER => {
            spawning::is_valid_spawn(u32::from(below.0), kind)
        }
        // No overworld biome makes any other creature with a chunk (a test holds it);
        // a type not registered passes.
        _ => true,
    }
}

/// Whether `biome` is in the biome tag `name` (`Holder.is(tag)`).
pub(crate) fn biome_in_tag(biome: Biome, name: &str) -> bool {
    clustine_data::TAGS
        .iter()
        .find(|tags| tags.registry == "minecraft:worldgen/biome")
        .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
        .is_some_and(|tag| tag.entries.contains(&i32::from(biome.0)))
}

/// The mob's own rules, made and snapped at `position`: `PathfinderMob.checkSpawnRules`
/// (its walk target value at its block at least 0) and `checkSpawnObstruction` (no
/// liquid in its box; a region has no entities to obstruct it).
pub(crate) fn own_rules(
    view: &dyn SpawnView,
    kind: u16,
    pos: BlockPos,
    position: [f64; 3],
) -> bool {
    walk_target_value(view, kind, pos) >= 0.0
        && !contains_any_liquid(view, &spawn_box(kind, position))
}

/// `getWalkTargetValue(pos)` of a fresh creature: 10 on what it prefers (grass for an
/// animal, mycelium for a mooshroom; water or sand for a turtle not going home), else
/// the light's worth.
pub(crate) fn walk_target_value(view: &dyn SpawnView, kind: u16, pos: BlockPos) -> f32 {
    let below = view.block(pos.offset(0, -1, 0));
    let preferred = match i32::from(kind) {
        entity_types::MOOSHROOM => is_block(below, blocks::MYCELIUM),
        entity_types::TURTLE => has_water(view.block(pos)) || in_tag(below, "minecraft:sand"),
        _ => is_block(below, blocks::GRASS_BLOCK),
    };
    if preferred {
        return 10.0;
    }
    pathfinding_cost_from_light_levels(view, pos)
}

/// `LevelReader.getPathfindingCostFromLightLevels`: the light's magic value less a
/// half, all in `f32` as the game computes it (11 gives −0.0926, 12 gives
/// 0.50000006 − 0.5 ≥ 0).
pub(crate) fn pathfinding_cost_from_light_levels(view: &dyn SpawnView, pos: BlockPos) -> f32 {
    let far =
        !(-30_000_000..30_000_000).contains(&pos.x) || !(-30_000_000..30_000_000).contains(&pos.z);
    let brightness = if far { 15 } else { raw_brightness(view, pos) };
    magic_value(view.dimension(), brightness) - 0.5
}

/// `getLightLevelDependentMagicValue` of a raw brightness in a level of `dimension`:
/// `Mth.lerp(ambient, g, 1)` with the dimension's ambient light (the overworld's 0, the
/// Nether's 0.1, the End's 0.25; `docs/groundwork/p-plan.md`, 18.5 and 18.6).
pub(crate) fn magic_value(dimension: clustine_world::Dimension, brightness: u8) -> f32 {
    let f = f32::from(brightness) / 15.0;
    let g = f / (4.0 - 3.0 * f);
    let ambient = dimension.rules().ambient_light;
    g + ambient * (1.0 - g)
}

#[cfg(test)]
#[path = "spawning_tests.rs"]
mod tests;
