//! Each spawned type's rule (`docs/groundwork/n9-plan.md`, section 9.4; D9 5.7's
//! table): `Monster.checkMonsterSpawnRules`, the animals', the water creatures', the
//! slime's and the rest, each with its draws on the chunk's stream in the game's order,
//! dispatched by D9 5.7's index; and what the per-chunk spawner asks of a position and a
//! type besides (`SpawnPlacements.isSpawnPositionOk`, a fresh mob's `checkSpawnRules`
//! and the liquid part of its `checkSpawnObstruction`).
//!
//! Every rule reads the world through [`SpawnWorld`], so that it is a function of what
//! it reads and its draws. A rule that reads light where the light is not known yet (a
//! chunk N8's step has not lit) fails, before any draw of its own (finding 43): the
//! attempt goes on to the next, as a failed rule's would, for the few ticks before the
//! chunk is lit. A block the world cannot read fails the rule likewise.
//!
//! Named differences: the overworld is never thundering in Clustine, so the monsters'
//! darkness is never read with the thunder's reach of 10; the world is never peaceful
//! (`Difficulty::WORLD` is easy), so peaceful's refusals are written and never taken;
//! the types of the Nether's lists (D9 5.7's #9, #14, #17, #20, #26, #27, #31, #32,
//! #35 and #38 but the any-light monsters) are refused here, since no overworld list or
//! structure names them.

use clustine_blocks::data::{fluid_in_tag, has_lava, in_tag, is_block};
use clustine_data::spawning::{self, Placement};
use clustine_data::{BIOME_SPAWNS, BlockState, blocks, entity_types};
use clustine_noise::LegacyRandom;
use clustine_world::generation::{Draws, FromNoise};
use clustine_world::{Biome, BlockPos, Difficulty};

pub(crate) use crate::generation::spawning::biome_in_tag;
use crate::generation::spawning::{is_valid_empty_spawn_block, magic_value};
use crate::mobs::body::ladder::{self, Family};

/// The light at a block as the last light step left it: the sky's, undarkened, and the
/// blocks'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Light {
    pub(crate) sky: u8,
    pub(crate) block: u8,
}

/// What a rule reads of the world: blocks, light, the biome, the surface and the clock.
/// The region's is `region/natural_spawning.rs`'; the tests' is a field of their own.
pub(crate) trait SpawnWorld {
    /// The block at `pos`, where the world can read it.
    fn block(&self, pos: BlockPos) -> Option<BlockState>;
    /// The light at `pos`; `None` where its chunk is not lit yet.
    fn light(&self, pos: BlockPos) -> Option<Light>;
    /// `getSkyDarken()`: how much the night takes from the sky's light now.
    fn sky_darken(&self) -> i32;
    /// `ServerLevel.getBiome(pos)`: the game's zoomed biome.
    fn biome(&self, pos: BlockPos) -> Option<Biome>;
    /// The `WORLD_SURFACE` heightmap at the column (`getHeightmapPos(...).getY()`): the
    /// first free height above the highest block that is not air.
    fn surface(&self, x: i32, z: i32) -> Option<i32>;
    /// The world's game seed (`WorldGenLevel.getSeed()`), which the slime chunks read.
    fn game_seed(&self) -> i64;
    /// The overworld clock's time (`world_clock`), which the `moon` timeline reads.
    fn day(&self) -> u64;
    /// `getSeaLevel()`: the world's (a region's `RegionConfig::sea_level`), the
    /// overworld's 63 where a reader knows no other.
    fn sea_level(&self) -> i32 {
        SEA_LEVEL
    }
    /// The level's dimension: the overworld where a reader knows no other.
    fn dimension(&self) -> clustine_world::Dimension {
        clustine_world::Dimension::Overworld
    }
}

/// The overworld's sea level.
const SEA_LEVEL: i32 = 63;

/// `MobCategory.getDespawnDistance()` squared, by the counted category's index: 64
/// blocks for the water's ambient, 128 for every other.
pub(crate) fn despawn_distance_sqr(category: usize) -> f64 {
    const WATER_AMBIENT: usize = 6;
    if category == WATER_AMBIENT {
        64.0 * 64.0
    } else {
        128.0 * 128.0
    }
}

/// `EntityType.canSpawnFarFromPlayer()`: the pillager's and the shulker's builders say
/// so (n7 EntityTypes 2278–2282, 2454–2459).
pub(crate) fn can_spawn_far_from_player(kind: u16) -> bool {
    matches!(
        i32::from(kind),
        entity_types::PILLAGER | entity_types::SHULKER
    )
}

/// `getMaxSpawnClusterSize()`: how many of one position's groups together end it.
/// `AbstractFish`'s 8 and the schooling fish's `getMaxSchoolSize()` (the salmon's 5),
/// `AbstractHorse`'s 6 (the llamas and the camel are horses), the wolf's 8, the
/// pillager's and the ghast's 1, `Mob`'s 4 for every other.
pub(crate) fn max_spawn_cluster_size(kind: u16) -> u32 {
    use entity_types::*;
    match i32::from(kind) {
        SALMON => 5,
        COD | PUFFERFISH | TROPICAL_FISH | WOLF => 8,
        HORSE | DONKEY | MULE | LLAMA | TRADER_LLAMA | CAMEL | CAMEL_HUSK | SKELETON_HORSE
        | ZOMBIE_HORSE => 6,
        PILLAGER | GHAST => 1,
        _ => 4,
    }
}

/// `SpawnPlacements.isSpawnPositionOk(type, level, pos)`: the type's placement's test
/// (n7 SpawnPlacementTypes), on the ground, in water, in lava or anywhere. A block the
/// world cannot read fails it.
pub(crate) fn is_spawn_position_ok(kind: u16, world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    let at = |pos: BlockPos| world.block(pos);
    match spawning::placement(kind).0 {
        Placement::NoRestrictions => true,
        Placement::InWater => {
            within_border(pos)
                && at(pos).is_some_and(has_water_fluid)
                && at(pos.offset(0, 1, 0))
                    .is_some_and(|above| !above.props().is_redstone_conductor())
        }
        Placement::InLava => within_border(pos) && at(pos).is_some_and(has_lava),
        Placement::OnGround => {
            within_border(pos)
                && at(pos.offset(0, -1, 0))
                    .is_some_and(|below| spawning::is_valid_spawn(u32::from(below.0), kind))
                && at(pos).is_some_and(|here| is_valid_empty_spawn_block(here, kind))
                && at(pos.offset(0, 1, 0))
                    .is_some_and(|above| is_valid_empty_spawn_block(above, kind))
        }
    }
}

/// `WorldBorder.isWithinBounds(BlockPos)` of the default border.
fn within_border(pos: BlockPos) -> bool {
    const BORDER: i32 = 29_999_984;
    (-BORDER..BORDER).contains(&pos.x) && (-BORDER..BORDER).contains(&pos.z)
}

/// The fluid at a state is water (`getFluidState(pos).is(FluidTags.WATER)`).
fn has_water_fluid(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:water")
}

/// The fluid at a state is lava (`is(FluidTags.LAVA)`).
fn has_lava_fluid(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:lava")
}

/// The type's rule (`SpawnPlacements.checkSpawnRules(type, level, NATURAL, pos,
/// random)`, n9 SpawnPlacements 77–108): peaceful refuses a type not allowed in it; a
/// type with no registration passes; else its predicate, by D9 5.7's index, with its
/// draws from `draws` in the game's order.
pub(crate) fn check_spawn_rules(
    kind: u16,
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    use entity_types::*;
    let id = i32::from(kind);
    if Difficulty::WORLD == Difficulty::Peaceful && !allowed_in_peaceful(kind) {
        return false;
    }
    match id {
        // #1
        AXOLOTL => below_in_tag(world, pos, "minecraft:axolotls_spawnable_on"),
        // #2, #3
        COD | PUFFERFISH | SALMON | DOLPHIN | SQUID => surface_water_animal(world, pos),
        // #4
        DROWNED => drowned(world, pos, draws),
        // #5
        GUARDIAN | ELDER_GUARDIAN => guardian(world, pos, draws),
        // #6
        TROPICAL_FISH => tropical_fish(world, pos),
        // #7, #11, #16, #19, #22, #25, #30, #37, #39: their own block tag and the light.
        ARMADILLO => on_and_bright(world, pos, "minecraft:armadillo_spawnable_on"),
        CAMEL => on_and_bright(world, pos, "minecraft:camels_spawnable_on"),
        FROG => on_and_bright(world, pos, "minecraft:frogs_spawnable_on"),
        GOAT => on_and_bright(world, pos, "minecraft:goats_spawnable_on"),
        MOOSHROOM => on_and_bright(world, pos, "minecraft:mooshrooms_spawnable_on"),
        PARROT => on_and_bright(world, pos, "minecraft:parrots_spawnable_on"),
        RABBIT => on_and_bright(world, pos, "minecraft:rabbits_spawnable_on"),
        WOLF => on_and_bright(world, pos, "minecraft:wolves_spawnable_on"),
        FOX => on_and_bright(world, pos, "minecraft:foxes_spawnable_on"),
        // #8
        BAT => bat(world, pos, draws),
        // #9: `checkAnyLightMonsterSpawnRules`.
        BLAZE | BREEZE | ZOGLIN => check_mob_spawn_rules(kind, world, pos),
        // #10
        BOGGED | CAVE_SPIDER | CREEPER | ENDERMAN | GIANT | SKELETON | SPIDER | WITCH | WITHER
        | WITHER_SKELETON | CREAKING | ZOMBIE | ZOMBIE_HORSE | ZOMBIE_VILLAGER | RAVAGER
        | EVOKER | ILLUSIONER | VEX | VINDICATOR | WARDEN => monster(kind, world, pos, draws),
        // #12
        CAMEL_HUSK | HUSK | PARCHED => surface_monster(kind, world, pos, draws),
        // #13
        CHICKEN | COW | DONKEY | HAPPY_GHAST | HORSE | LLAMA | MULE | PIG | SHEEP | CAT | PANDA
        | TRADER_LLAMA => animal(world, pos),
        // #15
        ENDER_DRAGON | IRON_GOLEM | SNOW_GOLEM | VILLAGER | WANDERING_TRADER | PHANTOM
        | SHULKER => check_mob_spawn_rules(kind, world, pos),
        // #18
        GLOW_SQUID => glow_squid(world, pos),
        // #21: `checkSulfurCubeSpawnRules` is `true` (n9 SulfurCube).
        SULFUR_CUBE => true,
        // #23
        NAUTILUS => nautilus(world, pos),
        // #24
        OCELOT => ocelot(draws),
        // #28
        PILLAGER => patrolling_monster(kind, world, pos),
        // #29
        POLAR_BEAR => polar_bear(world, pos),
        // #33
        SLIME => slime(kind, world, pos, draws),
        // #34
        STRAY => stray(kind, world, pos, draws),
        // #36
        TURTLE => turtle(world, pos),
        // The Nether's, each its family's (`docs/groundwork/p-plan.md`, section 18.2).
        GHAST => crate::mobs::types::ghast::check_spawn_rules(world, pos, draws),
        MAGMA_CUBE => crate::mobs::types::magma_cube::check_spawn_rules(world, pos, draws),
        HOGLIN => crate::mobs::types::hoglin::check_spawn_rules(world, pos, draws),
        PIGLIN => crate::mobs::types::piglin::check_spawn_rules(world, pos, draws),
        STRIDER => crate::mobs::types::strider::check_spawn_rules(world, pos, draws),
        ZOMBIFIED_PIGLIN => {
            crate::mobs::types::zombified_piglin::check_spawn_rules(world, pos, draws)
        }
        // The End's, and those no list names (named above).
        ENDERMITE | SILVERFISH | SKELETON_HORSE => false,
        // A type with no registration (`SpawnPlacements.DATA_BY_TYPE` has none).
        _ => true,
    }
}

/// `EntityType.isAllowedInPeaceful()`: false of the monsters' category. Clustine's
/// world is never peaceful, so nothing asks it but the rule's first line.
fn allowed_in_peaceful(kind: u16) -> bool {
    ladder::of(kind) != Family::Monster
}

/// `Mob.checkMobSpawnRules` at a natural reason: the block below is valid to spawn on
/// for the type (`isValidSpawn`).
pub(crate) fn check_mob_spawn_rules(kind: u16, world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    world
        .block(pos.offset(0, -1, 0))
        .is_some_and(|below| spawning::is_valid_spawn(u32::from(below.0), kind))
}

/// `getRawBrightness(pos, 0)`: the greater of the sky's light and the blocks',
/// undarkened.
fn raw_brightness(world: &dyn SpawnWorld, pos: BlockPos) -> Option<u8> {
    let light = world.light(pos)?;
    Some(light.sky.max(light.block))
}

/// `getMaxLocalRawBrightness(pos)`: the sky's light darkened by the night, or the
/// blocks', the greater; 15 beyond the world's border of ±30,000,000.
pub(crate) fn max_local_raw_brightness(world: &dyn SpawnWorld, pos: BlockPos) -> Option<u8> {
    const BORDER: i32 = 30_000_000;
    if !(-BORDER..BORDER).contains(&pos.x) || !(-BORDER..BORDER).contains(&pos.z) {
        return Some(15);
    }
    let light = world.light(pos)?;
    let darkened = (i32::from(light.sky) - world.sky_darken()).max(i32::from(light.block));
    Some(u8::try_from(darkened.clamp(0, 15)).unwrap_or(15))
}

/// `canSeeSky(pos)`: the sky's light there is full.
fn can_see_sky(world: &dyn SpawnWorld, pos: BlockPos) -> Option<bool> {
    Some(world.light(pos)?.sky >= 15)
}

/// `Monster.isDarkEnoughToSpawn` (n7 Monster 81–132): the sky's undarkened light above
/// `nextInt(32)` refuses (always drawn); then, where the dimension's
/// `monster_spawn_block_light_limit` is under 15, the blocks' light above it refuses (the
/// overworld's and the End's 0; the Nether's 15 is never compared, lines 30–58); then
/// the darkened light at most `monster_spawn_light_level`'s sample (lines 93–105). `None`
/// where the light is not known, before any draw.
fn is_dark_enough(world: &dyn SpawnWorld, pos: BlockPos, draws: &mut dyn Draws) -> Option<bool> {
    let light = world.light(pos)?;
    let darkened = max_local_raw_brightness(world, pos)?;
    if u32::from(light.sky) > draws.next_int(32) {
        return Some(false);
    }
    let rules = world.dimension().rules();
    let limit = rules.monster_spawn_block_light_limit;
    if limit < 15 && i32::from(light.block) > limit {
        return Some(false);
    }
    Some(i32::from(darkened) <= sample_spawn_light(rules.monster_spawn_light_level, draws))
}

/// `monsterSpawnLightTest().sample(random)`: the overworld's `UniformInt` of 0 to 7
/// draws `Mth.randomBetweenInclusive` (`nextInt(max − min + 1) + min`, n7 UniformInt
/// `sample`); the Nether's 7 and the End's 15 are `ConstantInt`s, which draw nothing.
/// The data keeps a constant as equal bounds (the dimension types' JSON writes it as a
/// bare number), so equal bounds are sampled without a draw.
fn sample_spawn_light((low, high): (i32, i32), draws: &mut dyn Draws) -> i32 {
    if low == high {
        return low;
    }
    let span = u32::try_from(high - low + 1).expect("a uniform light's bounds are in order");
    i32::try_from(draws.next_int(span)).expect("a light level fits an i32") + low
}

/// `Monster.checkMonsterSpawnRules`: dark enough, then `checkMobSpawnRules`.
fn monster(kind: u16, world: &dyn SpawnWorld, pos: BlockPos, draws: &mut dyn Draws) -> bool {
    is_dark_enough(world, pos, draws).unwrap_or(false) && check_mob_spawn_rules(kind, world, pos)
}

/// `Monster.checkSurfaceMonstersSpawnRules`: a monster's, then the sky seen.
fn surface_monster(
    kind: u16,
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    monster(kind, world, pos, draws) && can_see_sky(world, pos).unwrap_or(false)
}

/// `Stray.checkStraySpawnRules` (n7k Stray 11–42): up through any powder snow above the
/// position; a monster's rule at the position; then the sky seen from the topmost
/// powder snow (or the position itself where none is above it).
fn stray(kind: u16, world: &dyn SpawnWorld, pos: BlockPos, draws: &mut dyn Draws) -> bool {
    let mut above = pos;
    loop {
        above = above.offset(0, 1, 0);
        let Some(state) = world.block(above) else {
            return false;
        };
        if !is_block(state, blocks::POWDER_SNOW) {
            break;
        }
    }
    monster(kind, world, pos, draws) && can_see_sky(world, above.offset(0, -1, 0)).unwrap_or(false)
}

/// `Drowned.checkDrownedSpawnRules` (n7k Drowned 296–390): water below, or no draw at
/// all; a monster's darkness (its draws) and water at the position; then in
/// `#more_frequent_drowned_spawns` `nextInt(15) == 0`, elsewhere `nextInt(40) == 0` and
/// deeper than five below the sea, either drawn whatever the light said.
fn drowned(world: &dyn SpawnWorld, pos: BlockPos, draws: &mut dyn Draws) -> bool {
    if !world
        .block(pos.offset(0, -1, 0))
        .is_some_and(has_water_fluid)
    {
        return false;
    }
    let biome = world.biome(pos);
    let Some(dark) = is_dark_enough(world, pos, draws) else {
        return false;
    };
    let ok = dark && world.block(pos).is_some_and(has_water_fluid);
    if biome.is_some_and(|biome| biome_in_tag(biome, "minecraft:more_frequent_drowned_spawns")) {
        return draws.next_int(15) == 0 && ok;
    }
    draws.next_int(40) == 0 && pos.y < world.sea_level() - 5 && ok
}

/// `Guardian.checkGuardianSpawnRules` (n7 Guardian 796–829): `nextInt(20) == 0` (drawn
/// first, always) or the sky not seen from below the water; then water at the position
/// and below it.
fn guardian(world: &dyn SpawnWorld, pos: BlockPos, draws: &mut dyn Draws) -> bool {
    let lucky = draws.next_int(20) == 0;
    if !lucky && can_see_sky_from_below_water(world, pos).unwrap_or(true) {
        return false;
    }
    world.block(pos).is_some_and(has_water_fluid)
        && world
            .block(pos.offset(0, -1, 0))
            .is_some_and(has_water_fluid)
}

/// `LevelReader.canSeeSkyFromBelowWater(pos)` (n7 LevelReader 148–200): at or above the
/// sea, `canSeeSky`; below it, the sky seen at the sea's height and nothing between
/// that dims the light but liquid.
fn can_see_sky_from_below_water(world: &dyn SpawnWorld, pos: BlockPos) -> Option<bool> {
    let sea = world.sea_level();
    if pos.y >= sea {
        return can_see_sky(world, pos);
    }
    let mut at = BlockPos::new(pos.x, sea, pos.z);
    if !can_see_sky(world, at)? {
        return Some(false);
    }
    at = at.offset(0, -1, 0);
    while at.y > pos.y {
        let state = world.block(at)?;
        if state.props().light_dampening() > 0 && !state.props().is_liquid() {
            return Some(false);
        }
        at = at.offset(0, -1, 0);
    }
    Some(true)
}

/// `Animal.isBrightEnoughToSpawn`: undarkened light above 8.
fn bright(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    raw_brightness(world, pos).is_some_and(|light| light > 8)
}

/// Whether the block below `pos` is in the block tag `tag`.
fn below_in_tag(world: &dyn SpawnWorld, pos: BlockPos, tag: &str) -> bool {
    world
        .block(pos.offset(0, -1, 0))
        .is_some_and(|below| in_tag(below, tag))
}

/// `Animal.checkAnimalSpawnRules` (n7 Animal 174–212): bright enough, and on
/// `#animals_spawnable_on`.
fn animal(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    let bright = bright(world, pos);
    below_in_tag(world, pos, "minecraft:animals_spawnable_on") && bright
}

/// The animals with a tag of their own (the armadillo's, the camel's, the frog's, the
/// goat's, the mooshroom's, the parrot's, the rabbit's, the wolf's and the fox's): on
/// it, and bright enough.
fn on_and_bright(world: &dyn SpawnWorld, pos: BlockPos, tag: &str) -> bool {
    below_in_tag(world, pos, tag) && bright(world, pos)
}

/// `PolarBear.checkPolarBearSpawnRules`: in `#polar_bears_spawn_on_alternate_blocks`,
/// bright enough and on `#polar_bears_spawnable_on_alternate`; elsewhere an animal's.
fn polar_bear(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    let alternate = world.biome(pos).is_some_and(|biome| {
        biome_in_tag(biome, "minecraft:polar_bears_spawn_on_alternate_blocks")
    });
    if alternate {
        bright(world, pos)
            && below_in_tag(world, pos, "minecraft:polar_bears_spawnable_on_alternate")
    } else {
        animal(world, pos)
    }
}

/// `Turtle.checkTurtleSpawnRules`: below four above the sea, on sand
/// (`TurtleEggBlock.onSand`), bright enough.
fn turtle(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    pos.y < world.sea_level() + 4
        && below_in_tag(world, pos, "minecraft:sand")
        && bright(world, pos)
}

/// `Ocelot.checkOcelotSpawnRules` (n7 Ocelot 480–489): two times in three
/// (`nextInt(3) != 0`), whatever the place; its `checkSpawnObstruction` asks the rest.
fn ocelot(draws: &mut dyn Draws) -> bool {
    draws.next_int(3) != 0
}

/// Water at the position's foot and a water block above it.
fn water_below_and_above(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    world
        .block(pos.offset(0, -1, 0))
        .is_some_and(has_water_fluid)
        && world
            .block(pos.offset(0, 1, 0))
            .is_some_and(|above| is_block(above, blocks::WATER))
}

/// `WaterAnimal.checkSurfaceWaterAnimalSpawnRules` and
/// `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules` (the same, n7
/// WaterAnimal 104–138): from 13 below the sea up to it, in water.
fn surface_water_animal(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    let sea = world.sea_level();
    (sea - 13..=sea).contains(&pos.y) && water_below_and_above(world, pos)
}

/// `TropicalFish.checkTropicalFishSpawnRules` (n7 TropicalFish 452–484): in water, at
/// any height in `#allows_tropical_fish_spawns_at_any_height`, else near the surface.
fn tropical_fish(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    water_below_and_above(world, pos)
        && (world.biome(pos).is_some_and(|biome| {
            biome_in_tag(biome, "minecraft:allows_tropical_fish_spawns_at_any_height")
        }) || surface_water_animal(world, pos))
}

/// `AbstractNautilus.checkNautilusSpawnRules` (n7 AbstractNautilus 165–201): from 25
/// below the sea to 5 below it, in water.
fn nautilus(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    let sea = world.sea_level();
    (sea - 25..=sea - 5).contains(&pos.y) && water_below_and_above(world, pos)
}

/// `GlowSquid.checkGlowSquidSpawnRules` (n7 GlowSquid 154–177): 33 or more below the
/// sea, no light at all, in a water block.
fn glow_squid(world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    pos.y <= world.sea_level() - 33
        && raw_brightness(world, pos) == Some(0)
        && world
            .block(pos)
            .is_some_and(|state| is_block(state, blocks::WATER))
}

/// `Bat.checkBatSpawnRules` (n7 Bat 537–578): below the surface; one time in two
/// (`nextBoolean()`); the darkened light at most `nextInt(4)` (no Halloween: Clustine's
/// calendar has none, P7k 10); on `#bats_spawnable_on`; `checkMobSpawnRules`.
fn bat(world: &dyn SpawnWorld, pos: BlockPos, draws: &mut dyn Draws) -> bool {
    let Some(surface) = world.surface(pos.x, pos.z) else {
        return false;
    };
    if pos.y >= surface {
        return false;
    }
    if draws.next_bool() {
        return false;
    }
    let Some(light) = max_local_raw_brightness(world, pos) else {
        return false;
    };
    if u32::from(light) > draws.next_int(4) {
        return false;
    }
    below_in_tag(world, pos, "minecraft:bats_spawnable_on")
        && check_mob_spawn_rules(entity_types::BAT as u16, world, pos)
}

/// `PatrollingMonster.checkPatrollingMonsterSpawnRules` (n9 PatrollingMonster 151–167):
/// the blocks' light above 8 refuses; else any light's (`checkMobSpawnRules`).
fn patrolling_monster(kind: u16, world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    let Some(light) = world.light(pos) else {
        return false;
    };
    light.block <= 8 && check_mob_spawn_rules(kind, world, pos)
}

/// `WorldgenRandom.seedSlimeChunk(x, z, seed, salt)` (n9 WorldgenRandom 180–210): a
/// legacy random of the sum, its `int` products overflowing as `int` before they are
/// widened (`z·z` widened first, then times a `long`), the salt mixed in last.
pub(crate) fn seed_slime_chunk(x: i32, z: i32, seed: i64, salt: i64) -> LegacyRandom {
    let sum = seed
        .wrapping_add(i64::from(x.wrapping_mul(x).wrapping_mul(4_987_142)))
        .wrapping_add(i64::from(x.wrapping_mul(5_947_611)))
        .wrapping_add(i64::from(z.wrapping_mul(z)).wrapping_mul(4_392_871))
        .wrapping_add(i64::from(z.wrapping_mul(389_711)));
    LegacyRandom::from_seed((sum ^ salt) as u64)
}

/// The slime chunks' salt.
const SLIME_SALT: i64 = 987_234_911;

/// Whether the chunk at `x`, `z` is a slime chunk of the world of `seed`: its own random's
/// `nextInt(10)` is 0.
pub(crate) fn is_slime_chunk(x: i32, z: i32, seed: i64) -> bool {
    let mut random = seed_slime_chunk(x, z, seed, SLIME_SALT);
    FromNoise(&mut random).next_int(10) == 0
}

/// `SURFACE_SLIME_SPAWN_CHANCE` at the overworld clock's `day`: the `moon` timeline's
/// track (period 192 000, constant ease, `maximum` over the default 0), half the moon's
/// brightness: 0.5 at full moon down to 0 at the new.
pub(crate) fn surface_slime_spawn_chance(day: u64) -> f32 {
    crate::mobs::making::moon_brightness(day) * 0.5
}

/// `Slime.checkSlimeSpawnRules` (n9 Slime 96–198). In `#allows_surface_slime_spawns`
/// between 50 and 70 (both out): `nextFloat()` under the surface chance, then the
/// darkened light at most `nextInt(8)` (drawn only after the float passed), and then
/// `checkMobSpawnRules` decides; a float or a light that fails falls through to the
/// slime chunk. There `nextInt(10)` is drawn always, and 0 of it in a slime chunk below
/// 40 passes `checkMobSpawnRules`.
fn slime(kind: u16, world: &dyn SpawnWorld, pos: BlockPos, draws: &mut dyn Draws) -> bool {
    let surface = world
        .biome(pos)
        .is_some_and(|biome| biome_in_tag(biome, "minecraft:allows_surface_slime_spawns"));
    if surface && pos.y > 50 && pos.y < 70 {
        let chance = surface_slime_spawn_chance(world.day());
        if draws.next_float() < chance {
            let Some(light) = max_local_raw_brightness(world, pos) else {
                return false;
            };
            if u32::from(light) <= draws.next_int(8) {
                return check_mob_spawn_rules(kind, world, pos);
            }
        }
    }
    let chunk = pos.chunk();
    let slime_chunk = is_slime_chunk(chunk.x, chunk.z, world.game_seed());
    draws.next_int(10) == 0 && slime_chunk && pos.y < 40 && check_mob_spawn_rules(kind, world, pos)
}

/// A fresh mob's `checkSpawnRules(level, NATURAL)` at its block: a `PathfinderMob`'s
/// walk target value at least 0 (n7 PathfinderMob); `Mob`'s `true` for the bat, the
/// phantom and the other mobs that are no pathfinders.
pub(crate) fn mob_check_spawn_rules(kind: u16, world: &dyn SpawnWorld, pos: BlockPos) -> bool {
    match walk_target_value(kind, world, pos) {
        Some(value) => value >= 0.0,
        None => !is_pathfinder(kind),
    }
}

/// Whether the type is a `PathfinderMob`: every spawned type but the bat
/// (`AmbientCreature`), the phantom, the ghast and the dragon (`Mob`'s).
fn is_pathfinder(kind: u16) -> bool {
    use entity_types::*;
    !matches!(
        i32::from(kind),
        BAT | PHANTOM | GHAST | ENDER_DRAGON | SHULKER
    )
}

/// `getWalkTargetValue(pos, level)` of a fresh mob of the type `kind`, as the spawn
/// asks it: `Monster`'s less the light's worth (`getPathfindingCostFromLightLevels`);
/// the guardian's 10 more in water; `Animal`'s 10 on grass (a mooshroom's mycelium; a
/// turtle's water or sand, being not on its way home); the enderman's, the pillager's,
/// the axolotl's and `PathfinderMob`'s 0 for the rest. `None` for a mob that is no
/// pathfinder, or where the light is needed and not known.
fn walk_target_value(kind: u16, world: &dyn SpawnWorld, pos: BlockPos) -> Option<f32> {
    use entity_types::*;
    if !is_pathfinder(kind) {
        return None;
    }
    let cost = || -> Option<f32> {
        Some(magic_value(world.dimension(), max_local_raw_brightness(world, pos)?) - 0.5)
    };
    let below = || world.block(pos.offset(0, -1, 0));
    let id = i32::from(kind);
    match id {
        ENDERMAN | PILLAGER | AXOLOTL => Some(0.0),
        GUARDIAN | ELDER_GUARDIAN => {
            let cost = cost()?;
            if world.block(pos).is_some_and(has_water_fluid) {
                Some(10.0 + cost)
            } else {
                Some(-cost)
            }
        }
        MOOSHROOM => {
            if below().is_some_and(|below| is_block(below, blocks::MYCELIUM)) {
                Some(10.0)
            } else {
                cost()
            }
        }
        TURTLE => {
            let preferred = world.block(pos).is_some_and(has_water_fluid)
                || below().is_some_and(|below| in_tag(below, "minecraft:sand"));
            if preferred { Some(10.0) } else { cost() }
        }
        // `Strider.getWalkTargetValue`: 10 in lava; a fresh strider is in none, so 0
        // elsewhere (n7 Strider.txt 925–941).
        STRIDER => {
            let lava = world.block(pos).is_some_and(has_lava_fluid);
            Some(if lava { 10.0 } else { 0.0 })
        }
        // `Hoglin.getWalkTargetValue`: 10 over crimson nylium, else 0; a fresh hoglin
        // remembers no repellent (n7 Hoglin.txt 294–316).
        HOGLIN => {
            let nylium = below().is_some_and(|below| is_block(below, blocks::CRIMSON_NYLIUM));
            Some(if nylium { 10.0 } else { 0.0 })
        }
        _ => match ladder::of(kind) {
            Family::Monster => Some(-cost()?),
            Family::Animal => {
                if below().is_some_and(|below| is_block(below, blocks::GRASS_BLOCK)) {
                    Some(10.0)
                } else {
                    cost()
                }
            }
            _ => Some(0.0),
        },
    }
}

/// `checkSpawnObstruction`'s part that is not the entities': whether the type minds
/// liquid in its box, and the ocelot's own test of where it stands. The water's mobs
/// (`WaterAnimal`, `AgeableWaterCreature`, the axolotl, the guardians, the drowned, the
/// nautili) ask only `isUnobstructed`; the ocelot asks no liquid, at or above the sea,
/// on grass or leaves; every other no liquid in its box.
pub(crate) fn obstruction_by_blocks(
    kind: u16,
    world: &dyn SpawnWorld,
    pos: BlockPos,
    bounds: (BlockPos, BlockPos),
) -> bool {
    use entity_types::*;
    let id = i32::from(kind);
    // The strider asks only `isUnobstructed` too, so that it spawns in lava (n7
    // Strider.txt 358–363).
    let water_mob = ladder::of(kind) == Family::WaterAnimal
        || matches!(
            id,
            AXOLOTL | GUARDIAN | ELDER_GUARDIAN | DROWNED | NAUTILUS | ZOMBIE_NAUTILUS | STRIDER
        );
    if water_mob {
        return true;
    }
    if contains_any_liquid(world, bounds) {
        return false;
    }
    if id == OCELOT {
        return pos.y >= world.sea_level()
            && world.block(pos.offset(0, -1, 0)).is_some_and(|below| {
                is_block(below, blocks::GRASS_BLOCK) || in_tag(below, "minecraft:leaves")
            });
    }
    true
}

/// `containsAnyLiquid(box)` over the blocks from `low` to `high`, both in (generation's
/// reading of `findBlocksIn`, `generation/spawning.rs`); a block the world cannot read
/// counts as liquid, so that the spawn is refused.
fn contains_any_liquid(world: &dyn SpawnWorld, (low, high): (BlockPos, BlockPos)) -> bool {
    (low.x..=high.x).any(|x| {
        (low.y..=high.y).any(|y| {
            (low.z..=high.z).any(|z| {
                world
                    .block(BlockPos::new(x, y, z))
                    .is_none_or(|state| state.props().fluid().is_some())
            })
        })
    })
}

/// The biome named `name`, by its number (`BIOME_SPAWNS` keeps the biomes in the order
/// of their numbers).
pub(crate) fn biome_named(name: &str) -> Option<Biome> {
    let index = BIOME_SPAWNS.iter().position(|biome| biome.name == name)?;
    Some(Biome(u16::try_from(index).ok()?))
}

#[cfg(test)]
#[path = "rules_dimension_tests.rs"]
mod rules_dimension_tests;
