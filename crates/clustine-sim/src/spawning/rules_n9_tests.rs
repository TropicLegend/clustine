//! Natural spawning's rules and the per-chunk spawner, from the game's rules
//! (`docs/groundwork/n9-plan.md`, sections 9.3 and 9.4; D9 5.5 and 5.7): each rule's
//! outcome and draws against a scripted random, the slime chunks against the formula,
//! and the spawner through a region. Declared in `region/natural_spawning.rs`, so that
//! the region's tests' helpers reach it.

use std::collections::{BTreeMap, VecDeque};

use clustine_data::{BlockState, DIMENSION_TYPES, blocks, entity_types};
use clustine_world::generation::Draws;
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, EntityChange, EntityIds, KeptBox, KeptKind, KeptStructure,
    PlayerId, Vec3,
};

use super::super::spawn_caps::{NearPlayer, SPAWNING, SpawnState};
use super::super::tnt::tests::add_player;
use super::super::{Holdings, Random, Region, RegionConfig};
use super::{Ended, spawn_box};
use crate::api::{PublishedCounts, SPAWN_CATEGORIES, TickInputs, TickOutput, Ticket, WorldClock};
use crate::spawning::rules::{self, Light, SpawnWorld};
use crate::state::RegionState;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

fn biome(name: &str) -> Biome {
    rules::biome_named(name).unwrap_or_else(|| panic!("{name} is a biome"))
}

// ---------------------------------------------------------------------------------
// A scripted random and a field of blocks.

/// What a rule drew, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drawn {
    Int(u32),
    Float,
    Bool,
}

/// A value a scripted draw gives.
#[derive(Debug, Clone, Copy)]
enum Value {
    Int(u32),
    Float(f32),
    Bool(bool),
}

/// A random that gives what it is told, and notes what was asked of it.
#[derive(Default)]
struct Script {
    values: VecDeque<Value>,
    drawn: Vec<Drawn>,
}

impl Script {
    fn of(values: &[Value]) -> Self {
        Self {
            values: values.iter().copied().collect(),
            drawn: Vec::new(),
        }
    }
}

impl Draws for Script {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.drawn.push(Drawn::Int(bound));
        match self.values.pop_front() {
            Some(Value::Int(value)) => value.min(bound - 1),
            other => panic!("an int of {bound} was drawn where the script had {other:?}"),
        }
    }

    fn next_float(&mut self) -> f32 {
        self.drawn.push(Drawn::Float);
        match self.values.pop_front() {
            Some(Value::Float(value)) => value,
            other => panic!("a float was drawn where the script had {other:?}"),
        }
    }

    fn next_double(&mut self) -> f64 {
        panic!("no rule draws a double")
    }

    fn next_bool(&mut self) -> bool {
        self.drawn.push(Drawn::Bool);
        match self.values.pop_front() {
            Some(Value::Bool(value)) => value,
            other => panic!("a boolean was drawn where the script had {other:?}"),
        }
    }
}

/// A field for the rules: blocks set one by one over air, one light everywhere (none
/// where unlit), one biome, a surface, a seed and a day.
struct Field {
    blocks: BTreeMap<BlockPos, BlockState>,
    light: Option<Light>,
    darken: i32,
    biome: Option<Biome>,
    surface: i32,
    seed: i64,
    day: u64,
}

impl Field {
    /// Plains under the night sky (darkened by 11), lit by the sky alone.
    fn night() -> Self {
        Self {
            blocks: BTreeMap::new(),
            light: Some(Light { sky: 15, block: 0 }),
            darken: 11,
            biome: Some(biome("minecraft:plains")),
            surface: 64,
            seed: 0,
            day: 18_000,
        }
    }

    fn with(mut self, pos: BlockPos, block: BlockState) -> Self {
        self.blocks.insert(pos, block);
        self
    }

    fn lit(mut self, sky: u8, block: u8) -> Self {
        self.light = Some(Light { sky, block });
        self
    }
}

impl SpawnWorld for Field {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(self.blocks.get(&pos).copied().unwrap_or(blocks::AIR))
    }

    fn light(&self, _: BlockPos) -> Option<Light> {
        self.light
    }

    fn sky_darken(&self) -> i32 {
        self.darken
    }

    fn biome(&self, _: BlockPos) -> Option<Biome> {
        self.biome
    }

    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        Some(self.surface)
    }

    fn game_seed(&self) -> i64 {
        self.seed
    }

    fn day(&self) -> u64 {
        self.day
    }
}

/// Where the rules' mobs stand: on the block below at y 63.
const AT: BlockPos = BlockPos::new(0, 64, 0);

fn below(pos: BlockPos) -> BlockPos {
    pos.offset(0, -1, 0)
}

/// A field of `floor` under `AT`.
fn on(floor: BlockState) -> Field {
    Field::night().with(below(AT), floor)
}

fn check(id: i32, field: &Field, at: BlockPos, values: &[Value]) -> (bool, Vec<Drawn>) {
    let mut script = Script::of(values);
    let passed = rules::check_spawn_rules(kind(id), field, at, &mut script);
    assert!(
        script.values.is_empty(),
        "the rule drew fewer than scripted: {:?} left",
        script.values
    );
    (passed, script.drawn)
}

// ---------------------------------------------------------------------------------
// `Monster.checkMonsterSpawnRules` and its kin.

#[test]
fn a_monster_draws_against_the_sky_s_light_first_and_always() {
    // Full sky light against 14: refused at the first draw.
    let field = on(blocks::STONE);
    assert_eq!(
        check(entity_types::ZOMBIE, &field, AT, &[Value::Int(14)]),
        (false, vec![Drawn::Int(32)])
    );
    // Against 15 it passes; the darkened light (15 − 11 = 4) against `nextInt(8)`.
    assert_eq!(
        check(
            entity_types::ZOMBIE,
            &field,
            AT,
            &[Value::Int(15), Value::Int(4)]
        ),
        (true, vec![Drawn::Int(32), Drawn::Int(8)])
    );
    assert_eq!(
        check(
            entity_types::ZOMBIE,
            &field,
            AT,
            &[Value::Int(15), Value::Int(3)]
        ),
        (false, vec![Drawn::Int(32), Drawn::Int(8)])
    );
}

#[test]
fn any_block_light_refuses_a_monster_after_the_sky_s_draw_alone() {
    let field = on(blocks::STONE).lit(0, 1);
    assert_eq!(
        check(entity_types::SKELETON, &field, AT, &[Value::Int(0)]),
        (false, vec![Drawn::Int(32)])
    );
}

#[test]
fn a_monster_needs_a_block_to_spawn_on_after_its_draws() {
    let dark = Field::night()
        .lit(0, 0)
        .with(below(AT), state("minecraft:glass"));
    assert_eq!(
        check(
            entity_types::CREEPER,
            &dark,
            AT,
            &[Value::Int(0), Value::Int(0)]
        ),
        (false, vec![Drawn::Int(32), Drawn::Int(8)])
    );
}

#[test]
fn an_unlit_chunk_refuses_a_rule_that_reads_light_before_any_draw() {
    let mut field = on(blocks::STONE);
    field.light = None;
    for id in [
        entity_types::ZOMBIE,
        entity_types::HUSK,
        entity_types::COW,
        entity_types::PILLAGER,
        entity_types::GLOW_SQUID,
    ] {
        assert_eq!(check(id, &field, AT, &[]), (false, Vec::new()), "{id}");
    }
}

#[test]
fn a_husk_needs_the_sky_and_a_zombie_does_not() {
    // Under a roof the sky's light is 14 at most.
    let roofed = on(blocks::SAND).lit(14, 0);
    let draws = [Value::Int(31), Value::Int(7)];
    assert!(check(entity_types::ZOMBIE, &roofed, AT, &draws).0);
    assert_eq!(
        check(entity_types::HUSK, &roofed, AT, &draws),
        (false, vec![Drawn::Int(32), Drawn::Int(8)])
    );
    let open = on(blocks::SAND);
    assert!(
        check(
            entity_types::HUSK,
            &open,
            AT,
            &[Value::Int(31), Value::Int(7)]
        )
        .0
    );
}

#[test]
fn a_stray_looks_for_the_sky_from_the_topmost_powder_snow_above_it() {
    let snow = state("minecraft:powder_snow");
    let field = on(blocks::STONE)
        .with(AT.offset(0, 1, 0), snow)
        .with(AT.offset(0, 2, 0), snow);
    assert_eq!(
        check(
            entity_types::STRAY,
            &field,
            AT,
            &[Value::Int(31), Value::Int(7)]
        ),
        (true, vec![Drawn::Int(32), Drawn::Int(8)])
    );
    // The sky refused: the monster's draws are made all the same.
    let shaded = on(blocks::STONE).lit(14, 0);
    assert_eq!(
        check(
            entity_types::STRAY,
            &shaded,
            AT,
            &[Value::Int(31), Value::Int(7)]
        ),
        (false, vec![Drawn::Int(32), Drawn::Int(8)])
    );
}

#[test]
fn a_pillager_minds_only_the_blocks_light_above_8_and_draws_nothing() {
    let field = on(blocks::GRASS_BLOCK).lit(15, 8);
    assert_eq!(
        check(entity_types::PILLAGER, &field, AT, &[]),
        (true, vec![])
    );
    let lamp = on(blocks::GRASS_BLOCK).lit(0, 9);
    assert_eq!(
        check(entity_types::PILLAGER, &lamp, AT, &[]),
        (false, vec![])
    );
}

// ---------------------------------------------------------------------------------
// The water's monsters.

fn water() -> BlockState {
    blocks::WATER
}

/// Water from `low` to `high` over the column of `AT`, a floor of stone below.
fn pool(mut field: Field, low: i32, high: i32) -> Field {
    for y in low..=high {
        field = field.with(BlockPos::new(AT.x, y, AT.z), water());
    }
    field.with(BlockPos::new(AT.x, low - 1, AT.z), blocks::STONE)
}

#[test]
fn a_drowned_draws_nothing_without_water_below() {
    let dry = on(blocks::STONE);
    assert_eq!(check(entity_types::DROWNED, &dry, AT, &[]), (false, vec![]));
}

#[test]
fn a_drowned_in_a_river_draws_one_in_15_whatever_the_light_said() {
    let mut river = pool(Field::night(), 40, 62);
    river.biome = Some(biome("minecraft:river"));
    let at = BlockPos::new(0, 50, 0);
    // Too bright (the sky's 15 above 3): its draw is made all the same.
    assert_eq!(
        check(
            entity_types::DROWNED,
            &river,
            at,
            &[Value::Int(3), Value::Int(0)]
        ),
        (false, vec![Drawn::Int(32), Drawn::Int(15)])
    );
    let dark = pool(Field::night().lit(0, 0), 40, 62);
    let mut dark = dark;
    dark.biome = Some(biome("minecraft:river"));
    assert_eq!(
        check(
            entity_types::DROWNED,
            &dark,
            at,
            &[Value::Int(0), Value::Int(0), Value::Int(0)]
        ),
        (true, vec![Drawn::Int(32), Drawn::Int(8), Drawn::Int(15)])
    );
}

#[test]
fn a_drowned_in_the_ocean_draws_one_in_40_and_wants_depth() {
    let mut ocean = pool(Field::night().lit(0, 0), 30, 62);
    ocean.biome = Some(biome("minecraft:ocean"));
    let deep = BlockPos::new(0, 57, 0);
    assert_eq!(
        check(
            entity_types::DROWNED,
            &ocean,
            deep,
            &[Value::Int(0), Value::Int(0), Value::Int(0)]
        ),
        (true, vec![Drawn::Int(32), Drawn::Int(8), Drawn::Int(40)])
    );
    // At the sea's level less five it is not deep enough.
    let shallow = BlockPos::new(0, 58, 0);
    assert!(
        !check(
            entity_types::DROWNED,
            &ocean,
            shallow,
            &[Value::Int(0), Value::Int(0), Value::Int(0)]
        )
        .0
    );
}

#[test]
fn a_guardian_draws_one_in_20_first_and_then_wants_the_sky_hidden_and_water() {
    let at = BlockPos::new(0, 50, 0);
    // Open water under the sky: only its lucky draw lets it through.
    let open = pool(Field::night(), 40, 62);
    assert_eq!(
        check(entity_types::GUARDIAN, &open, at, &[Value::Int(1)]),
        (false, vec![Drawn::Int(20)])
    );
    assert_eq!(
        check(entity_types::GUARDIAN, &open, at, &[Value::Int(0)]),
        (true, vec![Drawn::Int(20)])
    );
    // Under a roof of stone in the water, the sky is hidden.
    let roofed =
        pool(Field::night().lit(0, 0), 40, 62).with(BlockPos::new(0, 55, 0), blocks::STONE);
    assert_eq!(
        check(entity_types::GUARDIAN, &roofed, at, &[Value::Int(7)]),
        (true, vec![Drawn::Int(20)])
    );
}

// ---------------------------------------------------------------------------------
// The slime.

#[test]
fn a_slime_out_of_a_swamp_draws_its_slime_chunk_ten_always() {
    let field = on(blocks::STONE);
    // Chunk (0, 0) of seed 0 is no slime chunk: drawn and refused.
    assert_eq!(
        check(
            entity_types::SLIME,
            &field,
            BlockPos::new(0, 30, 0),
            &[Value::Int(0)]
        ),
        (false, vec![Drawn::Int(10)])
    );
    // Chunk (−2, 0) is one: below 40 a 0 lets it through, above 40 not.
    let slimy = BlockPos::new(-30, 30, 4);
    let field = Field::night().with(below(slimy), blocks::STONE);
    assert_eq!(
        check(entity_types::SLIME, &field, slimy, &[Value::Int(0)]),
        (true, vec![Drawn::Int(10)])
    );
    assert!(!check(entity_types::SLIME, &field, slimy, &[Value::Int(1)]).0);
    let high = BlockPos::new(-30, 40, 4);
    let field = Field::night().with(below(high), blocks::STONE);
    assert_eq!(
        check(entity_types::SLIME, &field, high, &[Value::Int(0)]),
        (false, vec![Drawn::Int(10)])
    );
}

fn swamp(day: u64) -> Field {
    let mut field = Field::night().with(below(AT), blocks::GRASS_BLOCK);
    field.biome = Some(biome("minecraft:swamp"));
    field.day = day;
    field.light = Some(Light { sky: 15, block: 0 });
    field
}

#[test]
fn a_slime_in_a_swamp_draws_the_moon_s_chance_then_the_light_and_returns_its_ground_s_answer() {
    // Full moon (day 0 to 24 000): a chance of 0.5. A float of 0.49 passes; the
    // darkened light (15 − 11) at most 4 passes; the grass below decides.
    let field = swamp(18_000);
    assert_eq!(
        check(
            entity_types::SLIME,
            &field,
            AT,
            &[Value::Float(0.49), Value::Int(4)]
        ),
        (true, vec![Drawn::Float, Drawn::Int(8)])
    );
    // The light fails: falls through to the slime chunk's draw.
    assert_eq!(
        check(
            entity_types::SLIME,
            &field,
            AT,
            &[Value::Float(0.49), Value::Int(3), Value::Int(0)]
        ),
        (false, vec![Drawn::Float, Drawn::Int(8), Drawn::Int(10)])
    );
    // The float fails: no light drawn, the slime chunk's ten drawn.
    assert_eq!(
        check(
            entity_types::SLIME,
            &field,
            AT,
            &[Value::Float(0.5), Value::Int(0)]
        ),
        (false, vec![Drawn::Float, Drawn::Int(10)])
    );
    // The ground refuses after the float and the light passed: false at once, no
    // fall through (n9 Slime 120–129 returns `checkMobSpawnRules`).
    let glass = swamp(18_000).with(below(AT), state("minecraft:glass"));
    assert_eq!(
        check(
            entity_types::SLIME,
            &glass,
            AT,
            &[Value::Float(0.0), Value::Int(7)]
        ),
        (false, vec![Drawn::Float, Drawn::Int(8)])
    );
    // A new moon (day 4 · 24 000): a chance of 0, so no float passes.
    let new_moon = swamp(4 * 24_000 + 18_000);
    assert!(
        !check(
            entity_types::SLIME,
            &new_moon,
            AT,
            &[Value::Float(0.0), Value::Int(0)]
        )
        .0
    );
    // Out of 50 < y < 70: straight to the slime chunk.
    let low = BlockPos::new(0, 50, 0);
    let field = swamp(18_000).with(below(low), blocks::GRASS_BLOCK);
    assert_eq!(
        check(entity_types::SLIME, &field, low, &[Value::Int(5)]),
        (false, vec![Drawn::Int(10)])
    );
}

#[test]
fn the_surface_slime_chance_follows_the_moon_timeline() {
    let chances: Vec<f32> = (0..8)
        .map(|phase| rules::surface_slime_spawn_chance(phase * 24_000 + 100))
        .collect();
    assert_eq!(chances, [0.5, 0.375, 0.25, 0.125, 0.0, 0.125, 0.25, 0.375]);
    assert_eq!(rules::surface_slime_spawn_chance(192_000), 0.5);
}

/// The slime chunks of seeds 0 and 12345 within four chunks of the origin, and of a few
/// chunks far out where the `int` products overflow, as an independent port of
/// `java.util.Random` with the formula of n9 WorldgenRandom 180–210 computes them
/// (Python, the products wrapped to 32 bits before they are widened, `z·z` widened
/// before its `long` product, the salt xored last).
#[test]
fn the_slime_chunks_are_the_formula_s() {
    let near = |seed: i64| -> Vec<(i32, i32)> {
        let mut found = Vec::new();
        for x in -4..=4 {
            for z in -4..=4 {
                if rules::is_slime_chunk(x, z, seed) {
                    found.push((x, z));
                }
            }
        }
        found
    };
    assert_eq!(near(0), [(-2, 0), (1, -3), (2, -3), (2, 2), (2, 4), (4, 2)]);
    assert_eq!(
        near(12345),
        [
            (-4, 0),
            (-2, -4),
            (-2, 1),
            (-2, 4),
            (-1, 2),
            (0, -2),
            (3, 0),
            (4, -3),
            (4, 1)
        ]
    );
    assert_eq!(
        near(-4_172_144_997_902_289_642),
        [
            (-4, -4),
            (-3, -4),
            (-2, 3),
            (1, 0),
            (1, 1),
            (3, 0),
            (4, -4),
            (4, -2),
            (4, 3)
        ]
    );
    assert!(rules::is_slime_chunk(100_000, -100_000, 0));
    assert!(!rules::is_slime_chunk(100_000, -100_000, 12345));
    assert!(!rules::is_slime_chunk(46_341, 46_341, 0));
    assert!(!rules::is_slime_chunk(-30_000, 70_000, 0));
}

// ---------------------------------------------------------------------------------
// The animals, the water's and the rest.

#[test]
fn an_animal_wants_its_ground_and_the_undarkened_light_above_8() {
    // At night the darkened light is 4, the undarkened 15: animals spawn by the latter.
    let grass = on(blocks::GRASS_BLOCK);
    assert_eq!(check(entity_types::COW, &grass, AT, &[]), (true, vec![]));
    let dim = on(blocks::GRASS_BLOCK).lit(8, 0);
    assert!(!check(entity_types::COW, &dim, AT, &[]).0);
    let stone = on(blocks::STONE);
    assert!(!check(entity_types::SHEEP, &stone, AT, &[]).0);
    // Their own tags.
    assert!(
        check(
            entity_types::MOOSHROOM,
            &on(state("minecraft:mycelium")),
            AT,
            &[]
        )
        .0
    );
    assert!(!check(entity_types::MOOSHROOM, &grass, AT, &[]).0);
    assert!(check(entity_types::RABBIT, &on(blocks::SAND), AT, &[]).0);
    assert!(check(entity_types::GOAT, &on(blocks::STONE), AT, &[]).0);
}

#[test]
fn an_ocelot_draws_two_in_three_and_its_place_is_its_obstruction_s() {
    let field = on(blocks::STONE);
    assert_eq!(
        check(entity_types::OCELOT, &field, AT, &[Value::Int(1)]),
        (true, vec![Drawn::Int(3)])
    );
    assert_eq!(
        check(entity_types::OCELOT, &field, AT, &[Value::Int(0)]),
        (false, vec![Drawn::Int(3)])
    );
    // On grass at or above the sea, with no liquid in its box.
    let bounds = (AT, AT.offset(0, 1, 0));
    let ocelot = kind(entity_types::OCELOT);
    assert!(rules::obstruction_by_blocks(
        ocelot,
        &on(blocks::GRASS_BLOCK),
        AT,
        bounds
    ));
    assert!(rules::obstruction_by_blocks(
        ocelot,
        &on(state("minecraft:jungle_leaves[persistent=true]")),
        AT,
        bounds
    ));
    assert!(!rules::obstruction_by_blocks(
        ocelot,
        &on(blocks::STONE),
        AT,
        bounds
    ));
    let low = BlockPos::new(0, 62, 0);
    let field = Field::night().with(below(low), blocks::GRASS_BLOCK);
    assert!(!rules::obstruction_by_blocks(
        ocelot,
        &field,
        low,
        (low, low.offset(0, 1, 0))
    ));
}

#[test]
fn a_parrot_wants_its_own_ground_and_light() {
    let leaves = on(state("minecraft:oak_leaves[persistent=true]"));
    assert!(check(entity_types::PARROT, &leaves, AT, &[]).0);
    assert!(!check(entity_types::PARROT, &on(blocks::STONE), AT, &[]).0);
}

#[test]
fn a_polar_bear_on_ice_wants_its_biome_and_on_grass_is_an_animal() {
    let mut ice = on(state("minecraft:ice"));
    ice.biome = Some(biome("minecraft:frozen_ocean"));
    assert!(check(entity_types::POLAR_BEAR, &ice, AT, &[]).0);
    let mut plains_ice = on(state("minecraft:ice"));
    plains_ice.biome = Some(biome("minecraft:plains"));
    assert!(!check(entity_types::POLAR_BEAR, &plains_ice, AT, &[]).0);
}

#[test]
fn a_turtle_wants_sand_below_four_above_the_sea() {
    let sand = Field::night().with(below(BlockPos::new(0, 66, 0)), blocks::SAND);
    assert!(check(entity_types::TURTLE, &sand, BlockPos::new(0, 66, 0), &[]).0);
    let high = Field::night().with(below(BlockPos::new(0, 67, 0)), blocks::SAND);
    assert!(!check(entity_types::TURTLE, &high, BlockPos::new(0, 67, 0), &[]).0);
}

#[test]
fn a_bat_wants_to_be_below_the_surface_a_coin_and_the_dark() {
    let cave = Field::night().lit(0, 0).with(below(AT), blocks::STONE);
    let mut cave = cave;
    cave.surface = 70;
    // Below the surface: the coin false, the light 0 against 0, stone below.
    assert_eq!(
        check(
            entity_types::BAT,
            &cave,
            AT,
            &[Value::Bool(false), Value::Int(0)]
        ),
        (true, vec![Drawn::Bool, Drawn::Int(4)])
    );
    // The coin true refuses at once.
    assert_eq!(
        check(entity_types::BAT, &cave, AT, &[Value::Bool(true)]),
        (false, vec![Drawn::Bool])
    );
    // At the surface: no draw.
    cave.surface = 64;
    assert_eq!(check(entity_types::BAT, &cave, AT, &[]), (false, vec![]));
}

#[test]
fn the_water_s_animals_keep_to_their_depths() {
    let sea = pool(Field::night(), 40, 62);
    for (id, y, passes) in [
        (entity_types::COD, 50, true),
        (entity_types::COD, 49, false),
        (entity_types::SQUID, 61, true),
        (entity_types::DOLPHIN, 55, true),
        (entity_types::NAUTILUS, 38 + 3, true),
        (entity_types::NAUTILUS, 58, true),
        (entity_types::NAUTILUS, 59, false),
    ] {
        let at = BlockPos::new(0, y, 0);
        assert_eq!(check(id, &sea, at, &[]).0, passes, "{id} at {y}");
    }
    // A tropical fish at any height in a lush cave, near the surface elsewhere.
    let mut lush = pool(Field::night(), 10, 62);
    lush.biome = Some(biome("minecraft:lush_caves"));
    assert!(
        check(
            entity_types::TROPICAL_FISH,
            &lush,
            BlockPos::new(0, 20, 0),
            &[]
        )
        .0
    );
    let mut warm = pool(Field::night(), 10, 62);
    warm.biome = Some(biome("minecraft:warm_ocean"));
    assert!(
        !check(
            entity_types::TROPICAL_FISH,
            &warm,
            BlockPos::new(0, 20, 0),
            &[]
        )
        .0
    );
    assert!(
        check(
            entity_types::TROPICAL_FISH,
            &warm,
            BlockPos::new(0, 55, 0),
            &[]
        )
        .0
    );
}

#[test]
fn a_glow_squid_wants_the_deep_and_the_utter_dark() {
    let deep = pool(Field::night().lit(0, 0), -10, 40);
    assert!(
        check(
            entity_types::GLOW_SQUID,
            &deep,
            BlockPos::new(0, 30, 0),
            &[]
        )
        .0
    );
    assert!(
        !check(
            entity_types::GLOW_SQUID,
            &deep,
            BlockPos::new(0, 31, 0),
            &[]
        )
        .0
    );
    let lit = pool(Field::night().lit(0, 1), -10, 40);
    assert!(!check(entity_types::GLOW_SQUID, &lit, BlockPos::new(0, 30, 0), &[]).0);
}

#[test]
fn an_axolotl_wants_clay_and_its_kin() {
    let clay = on(state("minecraft:clay"));
    assert!(check(entity_types::AXOLOTL, &clay, AT, &[]).0);
    assert!(!check(entity_types::AXOLOTL, &on(blocks::STONE), AT, &[]).0);
}

#[test]
fn the_sulfur_cube_is_placed_on_the_ground_by_a_box_twice_its_size_and_its_rule_passes() {
    let cube = kind(entity_types::SULFUR_CUBE);
    assert_eq!(
        clustine_data::spawning::placement(cube),
        (
            clustine_data::spawning::Placement::OnGround,
            clustine_data::spawning::Heightmap::MotionBlockingNoLeaves
        )
    );
    assert_eq!(
        check(entity_types::SULFUR_CUBE, &on(blocks::STONE), AT, &[]),
        (true, vec![])
    );
    // 0.49 by 0.49, spawned in a box of 0.98 by 0.98.
    let bounds = spawn_box(cube, [0.5, 64.0, 0.5]);
    assert!((bounds.max[0] - bounds.min[0] - 0.98).abs() < 1e-6);
    assert!((bounds.max[1] - bounds.min[1] - 0.98).abs() < 1e-6);
    // The slime's four times its own.
    let slime = spawn_box(kind(entity_types::SLIME), [0.5, 64.0, 0.5]);
    assert!((slime.max[0] - slime.min[0] - 2.08).abs() < 1e-6);
    let zombie = spawn_box(kind(entity_types::ZOMBIE), [0.5, 64.0, 0.5]);
    assert!((zombie.max[0] - zombie.min[0] - 0.6).abs() < 1e-6);
}

#[test]
fn the_placements_test_ground_water_and_nothing() {
    let zombie = kind(entity_types::ZOMBIE);
    assert!(rules::is_spawn_position_ok(zombie, &on(blocks::STONE), AT));
    assert!(!rules::is_spawn_position_ok(
        zombie,
        &on(state("minecraft:glass")),
        AT
    ));
    let headroom = on(blocks::STONE).with(AT.offset(0, 1, 0), blocks::STONE);
    assert!(!rules::is_spawn_position_ok(zombie, &headroom, AT));
    let cod = kind(entity_types::COD);
    let sea = pool(Field::night(), 40, 62);
    assert!(rules::is_spawn_position_ok(
        cod,
        &sea,
        BlockPos::new(0, 50, 0)
    ));
    assert!(!rules::is_spawn_position_ok(
        cod,
        &sea,
        BlockPos::new(0, 63, 0)
    ));
    // The evoker spawns anywhere.
    assert!(rules::is_spawn_position_ok(
        kind(entity_types::EVOKER),
        &Field::night(),
        AT
    ));
}

#[test]
fn a_fresh_monster_wants_the_dark_by_its_walk_target_value_and_an_animal_its_grass() {
    let zombie = kind(entity_types::ZOMBIE);
    // Darkened light 11 passes, 12 is worth just over a half and fails.
    let eleven = on(blocks::STONE).lit(0, 11);
    assert!(rules::mob_check_spawn_rules(zombie, &eleven, AT));
    let twelve = on(blocks::STONE).lit(0, 12);
    assert!(!rules::mob_check_spawn_rules(zombie, &twelve, AT));
    // In daylight a monster's place is worth nothing to it.
    let mut day = on(blocks::STONE);
    day.darken = 0;
    assert!(!rules::mob_check_spawn_rules(zombie, &day, AT));
    // The enderman and the pillager like every place alike.
    assert!(rules::mob_check_spawn_rules(
        kind(entity_types::ENDERMAN),
        &day,
        AT
    ));
    assert!(rules::mob_check_spawn_rules(
        kind(entity_types::PILLAGER),
        &day,
        AT
    ));
    // A cow on grass in the dark; off grass it wants light worth a half.
    let cow = kind(entity_types::COW);
    let dark = Field::night().lit(0, 0);
    assert!(rules::mob_check_spawn_rules(
        cow,
        &dark.with(below(AT), blocks::GRASS_BLOCK),
        AT
    ));
    let dark = Field::night().lit(0, 0);
    assert!(!rules::mob_check_spawn_rules(
        cow,
        &dark.with(below(AT), blocks::SAND),
        AT
    ));
    // A bat is no pathfinder.
    assert!(rules::mob_check_spawn_rules(
        kind(entity_types::BAT),
        &Field::night().lit(0, 15),
        AT
    ));
}

#[test]
fn the_cluster_sizes_are_the_types() {
    let size = |id| rules::max_spawn_cluster_size(kind(id));
    assert_eq!(size(entity_types::ZOMBIE), 4);
    assert_eq!(size(entity_types::PILLAGER), 1);
    assert_eq!(size(entity_types::WOLF), 8);
    assert_eq!(size(entity_types::HORSE), 6);
    assert_eq!(size(entity_types::LLAMA), 6);
    assert_eq!(size(entity_types::CAMEL), 6);
    assert_eq!(size(entity_types::COD), 8);
    assert_eq!(size(entity_types::SALMON), 5);
    assert_eq!(size(entity_types::PUFFERFISH), 8);
    assert_eq!(size(entity_types::SHEEP), 4);
}

#[test]
fn every_type_of_the_overworld_s_lists_is_built_but_the_sulfur_cube() {
    let overworld_names: Vec<&str> = clustine_data::BIOME_SPAWNS
        .iter()
        .map(|biome| biome.name)
        .filter(|name| {
            !name.contains("nether")
                && !name.contains("end")
                && !name.contains("soul_sand")
                && !name.contains("crimson")
                && !name.contains("warped")
                && !name.contains("basalt")
                && *name != "minecraft:small_end_islands"
                && *name != "minecraft:the_void"
        })
        .collect();
    let mut unbuilt = std::collections::BTreeSet::new();
    for biome in clustine_data::BIOME_SPAWNS
        .iter()
        .filter(|biome| overworld_names.contains(&biome.name))
    {
        for (_, list) in biome.categories {
            for entry in *list {
                let id = u16::try_from(entry.kind).expect("a type");
                if !crate::mobs::types::is_built(id) {
                    unbuilt.insert(clustine_data::ENTITY_TYPES[entry.kind]);
                }
            }
        }
    }
    assert_eq!(
        unbuilt.into_iter().collect::<Vec<_>>(),
        ["minecraft:sulfur_cube"]
    );
}

// ---------------------------------------------------------------------------------
// The spawner through a region.

/// The floor's top: a test's mobs stand at y −60.
const FLOOR: i32 = -61;
const STANDING: i32 = FLOOR + 1;

fn overworld_chunk(position: ChunkPos, biome: Biome, floor: BlockState) -> Chunk {
    let overworld = DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("the game has the overworld");
    let mut chunk = Chunk::empty(overworld, biome);
    for z in 0..16 {
        for x in 0..16 {
            for y in -64..FLOOR {
                chunk.set(x, y, z, blocks::STONE);
            }
            chunk.set(x, FLOOR, z, floor);
        }
    }
    let _ = position;
    chunk
}

/// A region of the 5×5 chunks about the origin's, all held and loaded, the middle 3×3
/// ticking, of one biome over a floor of `floor`, the structures `structures` kept by
/// chunk (1, 1); ticked once.
fn field_region(biome: Biome, floor: BlockState, structures: Vec<KeptStructure>) -> Region {
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = overworld_chunk(*position, biome, floor);
            if *position == ChunkPos::new(1, 1) {
                chunk.set_structures(structures.clone());
            }
            (*position, chunk)
        })
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: held.clone(),
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let config = RegionConfig {
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension: Default::default(),
        spawn: Some(Vec3::new(0.5, f64::from(STANDING), 0.5)),
        starting_inventory: Default::default(),
        return_after: u64::MAX,
        lowest_y: -64,
        seed: 0x5_10,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, Vec::new());
    region.tick(&TickInputs::default());
    region
}

/// The player 30 blocks east of the world's spawn.
const PLAYER_AT: [f64; 3] = [30.5, STANDING as f64, 0.5];

/// Sets the clock so that the next tick runs at `game_time` and `day`.
fn set_clock(region: &mut Region, game_time: u64, day: u64) {
    region.clock = WorldClock {
        game_time: game_time - 1,
        day: day - 1,
    };
}

/// The mobs a tick made, by their records as first committed.
fn made(output: &TickOutput) -> Vec<clustine_world::Entity> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Whole(entity)
                if matches!(entity.data, clustine_world::EntityData::Mob { .. }) =>
            {
                Some(entity.clone())
            }
            _ => None,
        })
        .collect()
}

/// Runs ticks at night (day 18 000 on), never at a creatures' tick, with the others'
/// counts `published`, until `enough` mobs were made or 400 ticks went by; what was
/// made.
fn run_nights(
    region: &mut Region,
    published: &PublishedCounts,
    enough: usize,
) -> Vec<clustine_world::Entity> {
    let mut all = Vec::new();
    for tick in 0..400_u64 {
        set_clock(region, 1_000 + tick * 2 + 1, 18_000 + tick);
        let inputs = TickInputs {
            counts: Some(published.clone()),
            ..TickInputs::default()
        };
        let output = region.tick(&inputs);
        all.extend(made(&output));
        if all.len() >= enough {
            break;
        }
    }
    all
}

fn distance_sqr(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|axis| (a[axis] - b[axis]).powi(2)).sum()
}

#[test]
fn nothing_spawns_with_the_game_rule_spawn_mobs_off() {
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    region.config.spawn_mobs = false;
    add_player(&mut region, 1, PLAYER_AT, false);
    let spawned = run_nights(&mut region, &PublishedCounts::default(), 6);
    assert!(spawned.is_empty(), "{} came", spawned.len());
}

#[test]
fn monsters_spawn_at_night_on_a_dark_floor_and_none_within_24_blocks_of_the_player_or_the_spawn() {
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    add_player(&mut region, 1, PLAYER_AT, false);
    let spawned = run_nights(&mut region, &PublishedCounts::default(), 6);
    assert!(
        spawned.len() >= 6,
        "monsters came at night: {}",
        spawned.len()
    );
    let monsters = [
        entity_types::SPIDER,
        entity_types::ZOMBIE,
        entity_types::ZOMBIE_VILLAGER,
        entity_types::SKELETON,
        entity_types::CREEPER,
        entity_types::SLIME,
        entity_types::ENDERMAN,
        entity_types::WITCH,
        // A spider's jockey, a zombie's chicken.
        entity_types::CHICKEN,
    ];
    for mob in &spawned {
        assert!(
            monsters.contains(&i32::from(mob.kind)),
            "a {} of the badlands' monsters",
            clustine_data::ENTITY_TYPES[usize::from(mob.kind)]
        );
        assert_eq!(mob.position[1], f64::from(STANDING), "on the floor");
        assert!(
            distance_sqr(mob.position, PLAYER_AT) > 576.0,
            "{:?}",
            mob.position
        );
        let spawn = [0.5, f64::from(STANDING) + 0.5, 0.5];
        assert!(
            distance_sqr(mob.position, spawn) >= 576.0,
            "{:?}",
            mob.position
        );
        // On land the region ticks.
        assert!(region.ticks_chunk(mob.chunk()));
    }
}

#[test]
fn no_monster_spawns_in_daylight_on_grass() {
    let mut region = field_region(biome("minecraft:badlands"), blocks::GRASS_BLOCK, Vec::new());
    add_player(&mut region, 1, PLAYER_AT, false);
    let mut spawned = Vec::new();
    for tick in 0..100_u64 {
        // Noon, and never a tick of the creatures.
        set_clock(&mut region, 1_001 + tick, 6_000 + tick);
        let inputs = TickInputs {
            counts: Some(PublishedCounts::default()),
            ..TickInputs::default()
        };
        spawned.extend(made(&region.tick(&inputs)));
    }
    assert_eq!(spawned.len(), 0);
}

#[test]
fn animals_spawn_only_on_a_tick_of_game_time_divisible_by_400() {
    let mut region = field_region(biome("minecraft:plains"), blocks::GRASS_BLOCK, Vec::new());
    add_player(&mut region, 1, PLAYER_AT, false);
    let mut on_the_tick = Vec::new();
    for round in 1..=40_u64 {
        let inputs = TickInputs {
            counts: Some(PublishedCounts::default()),
            ..TickInputs::default()
        };
        // A tick just before, at noon: no creature.
        set_clock(&mut region, round * 400 - 1, 6_000 + round * 2);
        assert_eq!(
            made(&region.tick(&inputs)),
            Vec::new(),
            "round {round}, before"
        );
        // The tick of game time divisible by 400.
        let output = region.tick(&inputs);
        on_the_tick.extend(made(&output));
        if on_the_tick.len() >= 4 {
            break;
        }
    }
    assert!(!on_the_tick.is_empty(), "animals came on the 400th ticks");
    let animals = [
        entity_types::SHEEP,
        entity_types::PIG,
        entity_types::CHICKEN,
        entity_types::COW,
        entity_types::HORSE,
        entity_types::DONKEY,
    ];
    for mob in &on_the_tick {
        assert!(animals.contains(&i32::from(mob.kind)), "{}", mob.kind);
    }
}

#[test]
fn the_local_cap_of_a_full_player_stops_their_chunks() {
    let full = PublishedCounts {
        totals: [0; SPAWN_CATEGORIES],
        players: vec![(PlayerId(uuid::Uuid::from_u128(1)), [70, 0, 0, 0, 0, 0, 0])],
    };
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    add_player(&mut region, 1, PLAYER_AT, false);
    let mut spawned = Vec::new();
    for tick in 0..100_u64 {
        set_clock(&mut region, 1_001 + tick * 2, 18_000 + tick);
        let inputs = TickInputs {
            counts: Some(full.clone()),
            ..TickInputs::default()
        };
        spawned.extend(made(&region.tick(&inputs)));
    }
    assert_eq!(
        spawned.len(),
        0,
        "the player's 70 monsters hold their chunks"
    );
    // One fewer counted, and the chunks spawn again.
    let mut room = full.clone();
    room.players[0].1[0] = 69;
    assert!(!run_nights(&mut region, &room, 1).is_empty());
}

/// A swamp hut whose one piece covers chunk (1, 1) from y −64 to −40.
fn hut() -> KeptStructure {
    let piece = KeptBox {
        min: BlockPos::new(16, -64, 16),
        max: BlockPos::new(31, -40, 31),
    };
    KeptStructure {
        kind: KeptKind::SwampHut,
        source: ChunkPos::new(1, 1),
        bounds: piece,
        pieces: vec![piece],
    }
}

#[test]
fn a_swamp_hut_s_piece_spawns_its_witches_and_its_cats() {
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, vec![hut()]);
    let inside = BlockPos::new(20, STANDING, 20);
    let outside = BlockPos::new(8, STANDING, 20);
    let witch = clustine_data::spawns::Spawner {
        kind: entity_types::WITCH as usize,
        weight: 1,
        min_count: 1,
        max_count: 1,
    };
    let cat = clustine_data::spawns::Spawner {
        kind: entity_types::CAT as usize,
        ..witch
    };
    assert_eq!(region.mobs_at(0, inside), [witch]);
    assert_eq!(region.mobs_at(1, inside), [cat]);
    assert_ne!(region.mobs_at(0, outside), [witch]);
    // Ambient keeps the biome's bats.
    assert_eq!(region.mobs_at(2, inside), region.mobs_at(2, outside));
    // At night, every monster made in the hut's chunk is a witch.
    add_player(&mut region, 1, PLAYER_AT, false);
    let spawned = run_nights(&mut region, &PublishedCounts::default(), 40);
    let in_hut: Vec<u16> = spawned
        .iter()
        .filter(|mob| mob.chunk() == ChunkPos::new(1, 1))
        .map(|mob| mob.kind)
        .collect();
    assert!(!in_hut.is_empty(), "witches came to the hut");
    assert!(
        in_hut
            .iter()
            .all(|kind| i32::from(*kind) == entity_types::WITCH)
    );
}

#[test]
fn an_ancient_city_spawns_nothing_and_a_monument_its_guardians() {
    let region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    let mut region = region;
    let city = KeptStructure {
        kind: KeptKind::AncientCity,
        source: ChunkPos::new(1, 1),
        bounds: KeptBox {
            min: BlockPos::new(16, -64, 16),
            max: BlockPos::new(31, -50, 31),
        },
        pieces: Vec::new(),
    };
    region
        .chunks
        .get_mut(&ChunkPos::new(1, 1))
        .expect("loaded")
        .set_structures(vec![city]);
    let inside = BlockPos::new(20, STANDING, 20);
    for category in 0..SPAWN_CATEGORIES {
        assert!(region.mobs_at(category, inside).is_empty(), "{category}");
    }
    let monument = KeptStructure {
        kind: KeptKind::Monument,
        source: ChunkPos::new(1, 1),
        bounds: KeptBox {
            min: BlockPos::new(16, -64, 16),
            max: BlockPos::new(31, -50, 31),
        },
        pieces: Vec::new(),
    };
    region
        .chunks
        .get_mut(&ChunkPos::new(1, 1))
        .expect("loaded")
        .set_structures(vec![monument]);
    let guardians = region.mobs_at(0, inside);
    assert_eq!(guardians.len(), 1);
    assert_eq!(guardians[0].kind, entity_types::GUARDIAN as usize);
    assert_eq!((guardians[0].min_count, guardians[0].max_count), (2, 4));
    // Its axolotls none; its water creatures the biome's.
    assert!(region.mobs_at(3, inside).is_empty());
    assert_eq!(
        region.mobs_at(5, inside),
        region.mobs_at(5, BlockPos::new(8, STANDING, 8))
    );
}

/// A spawning state of the players `players` (number, feet), nothing counted.
fn spawning_state(players: &[(u128, [f64; 3])]) -> SpawnState {
    let near: Vec<NearPlayer> = players
        .iter()
        .map(|(number, feet)| NearPlayer {
            player: PlayerId(uuid::Uuid::from_u128(*number)),
            position: *feet,
            spectator: false,
        })
        .collect();
    SpawnState::new(near, &BTreeMap::new(), &PublishedCounts::default(), true)
}

#[test]
fn an_unbuilt_type_picked_and_passing_its_checks_ends_its_whole_position() {
    // The sulfur caves: the sulfur cube 100 of 311 of the monsters, and its rule
    // `true`. Over many ticks of chunk (−1, 1), its picks end their positions with
    // nothing made after, and none is ever made.
    let mut region = field_region(biome("minecraft:sulfur_caves"), blocks::STONE, Vec::new());
    set_clock(&mut region, 1_001, 18_000);
    let chunk = ChunkPos::new(-1, 1);
    let mut ended = 0;
    let mut output = TickOutput::default();
    for tick in 0..400 {
        region.tick = 10_000 + tick;
        let mut state = spawning_state(&[(1, PLAYER_AT)]);
        let mut draws = Random::of_chunk(region.config.seed, chunk, region.tick, SPAWNING);
        let spawned =
            region.spawn_category_for_chunk(0, chunk, &mut state, &mut draws, &mut output);
        assert!(
            spawned
                .made
                .iter()
                .all(|(kind, _)| i32::from(*kind) != entity_types::SULFUR_CUBE)
        );
        if let Ended::Unbuilt(kind) = spawned.ended {
            assert_eq!(i32::from(kind), entity_types::SULFUR_CUBE);
            ended += 1;
        }
    }
    assert!(ended > 0, "some positions picked the sulfur cube");
}

/// The draws a position takes over the test's floor of stone (its top at `top`, every
/// block below a redstone conductor) where no attempt finds a player to spawn by: the
/// position's three, and nothing more where it is the lowest block or in the stone;
/// else for each of three groups the pack size and four steps an attempt.
fn draws_without_a_pick(start: &Random, min_y: i32, top: i32) -> u64 {
    let mut replay = start.clone();
    let _x = replay.next_int(16);
    let _z = replay.next_int(16);
    let y = replay.next_int(u32::try_from(top - min_y + 1).expect("a span")) as i32 + min_y;
    if y < top {
        return replay.draws_since(start);
    }
    for _ in 0..3 {
        let pack = super::ceil(replay.next_float() * 4.0);
        for _ in 0..pack * 4 {
            replay.next_int(6);
        }
    }
    replay.draws_since(start)
}

#[test]
fn an_attempt_without_a_player_or_too_near_one_draws_its_four_steps_alone() {
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    set_clock(&mut region, 1_001, 18_000);
    let chunk = ChunkPos::new(0, 1);
    let mut output = TickOutput::default();
    // Players every eight blocks about the chunk: one within 24 of every step.
    let crowd: Vec<(u128, [f64; 3])> = (0..13)
        .flat_map(|i| (0..13).map(move |j| (i, j)))
        .map(|(i, j)| {
            let number = u128::try_from(i * 13 + j + 1).expect("few");
            let x = f64::from(-40 + 8 * i);
            let z = f64::from(-24 + 8 * j);
            (number, [x, f64::from(STANDING), z])
        })
        .collect();
    for players in [
        // Nobody to spawn by.
        Vec::new(),
        crowd,
    ] {
        for tick in 0..20 {
            region.tick = 20_000 + tick;
            let mut state = spawning_state(&players);
            let start = Random::of_chunk(region.config.seed, chunk, region.tick, SPAWNING);
            let mut draws = start.clone();
            let spawned =
                region.spawn_category_for_chunk(0, chunk, &mut state, &mut draws, &mut output);
            assert!(spawned.made.is_empty());
            assert_eq!(
                draws.draws_since(&start),
                draws_without_a_pick(&start, -64, STANDING),
                "tick {tick}, {} players",
                players.len()
            );
        }
    }
}

#[test]
fn every_category_of_a_chunk_draws_from_one_stream_in_turn() {
    // With nobody near, each category open draws what `draws_without_a_pick` says, one
    // after the other on the chunk's one stream.
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    let chunk = ChunkPos::new(0, 1);
    let start = Random::of_chunk(region.config.seed, chunk, region.tick, SPAWNING);
    let mut draws = start.clone();
    let mut output = TickOutput::default();
    let mut state = spawning_state(&[]);
    let mut expected = start.clone();
    for category in [0, 2] {
        region.spawn_category_for_chunk(category, chunk, &mut state, &mut draws, &mut output);
        let used = draws_without_a_pick(&expected, -64, STANDING);
        for _ in 0..used {
            expected.next_u64();
        }
    }
    assert_eq!(draws, expected);
}

#[test]
fn a_redstone_conductor_at_the_position_ends_it_after_the_three_draws() {
    // A floor of stone up to the surface: every position the column draws is in stone
    // or at the floor's top, where stone is a conductor.
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    let chunk = ChunkPos::new(0, 1);
    let mut output = TickOutput::default();
    let mut conductors = 0;
    for tick in 0..30 {
        region.tick = 30_000 + tick;
        let start = Random::of_chunk(region.config.seed, chunk, region.tick, SPAWNING);
        let mut draws = start.clone();
        let mut state = spawning_state(&[(1, PLAYER_AT)]);
        let spawned =
            region.spawn_category_for_chunk(0, chunk, &mut state, &mut draws, &mut output);
        if spawned.ended == Ended::Conductor {
            conductors += 1;
            assert_eq!(draws.draws_since(&start), 3);
            assert_eq!(spawned.groups, 0);
        }
    }
    assert!(conductors > 0);
}

#[test]
fn a_spawned_mob_is_counted_for_the_players_near_it_at_once() {
    let mut region = field_region(biome("minecraft:badlands"), blocks::STONE, Vec::new());
    set_clock(&mut region, 1_001, 18_000);
    let chunk = ChunkPos::new(-1, 1);
    let mut output = TickOutput::default();
    let mut counted = None;
    for tick in 0..400 {
        region.tick = 40_000 + tick;
        let mut state = spawning_state(&[(1, PLAYER_AT)]);
        let mut draws = Random::of_chunk(region.config.seed, chunk, region.tick, SPAWNING);
        let spawned =
            region.spawn_category_for_chunk(0, chunk, &mut state, &mut draws, &mut output);
        if !spawned.made.is_empty() {
            counted = Some((spawned.made.len(), state));
            break;
        }
    }
    let (made, mut state) = counted.expect("a monster came");
    // The player had none counted: each one made raised their count, the monsters'
    // cap of 70 less what was made still open, and full at 70.
    let made = u32::try_from(made).expect("few");
    for _ in made..70 {
        assert!(state.can_spawn_local(0, chunk));
        state.after_spawn(0, chunk);
    }
    assert!(!state.can_spawn_local(0, chunk));
}

#[test]
fn the_monsters_of_the_plains_spawn_in_the_shares_their_weights_give() {
    // The spawning oracle (`bin/clustine/tests/spawning_oracle.rs`) showed Clustine more
    // zombies and fewer of the other monsters than the official server. The type is the
    // weighted pick of the plains' list, once a group, and the zombie, the skeleton, the
    // creeper and the spider pass the same rule on a dark floor of grass, so each
    // position's first type comes in the share of its weight among the types that can
    // spawn there (the slime's rule fails out of a slime chunk, and none of these is
    // one). Fresh fields keep the crowd of what was made from obstructing the wide
    // spider more than the game would.
    let plains = biome("minecraft:plains");
    let list = clustine_data::BIOME_SPAWNS[usize::from(plains.0)]
        .of("monster")
        .expect("the plains have monsters");
    let weight = |id: i32| -> i32 {
        list.iter()
            .filter(|entry| entry.kind == usize::try_from(id).expect("an id"))
            .map(|entry| entry.weight)
            .sum()
    };
    let total = list.iter().map(|entry| entry.weight).sum::<i32>() - weight(entity_types::SLIME);
    let mut positions: BTreeMap<u16, u32> = BTreeMap::new();
    let mut mobs: BTreeMap<u16, u32> = BTreeMap::new();
    let mut output = TickOutput::default();
    for round in 0..80_u64 {
        let mut region = field_region(plains, blocks::GRASS_BLOCK, Vec::new());
        set_clock(&mut region, 1_001, 18_000);
        for tick in 0..50 {
            region.tick = 60_000 + round * 1_000 + tick;
            for chunk in (-1..=1).flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z))) {
                assert!(!rules::is_slime_chunk(chunk.x, chunk.z, 0));
                let mut state = spawning_state(&[(1, PLAYER_AT)]);
                let mut draws = Random::of_chunk(region.config.seed, chunk, region.tick, SPAWNING);
                let spawned =
                    region.spawn_category_for_chunk(0, chunk, &mut state, &mut draws, &mut output);
                if let Some((kind, _)) = spawned.made.first() {
                    *positions.entry(*kind).or_default() += 1;
                }
                for (kind, _) in &spawned.made {
                    *mobs.entry(*kind).or_default() += 1;
                }
            }
        }
    }
    let all_positions: u32 = positions.values().sum();
    let all_mobs: u32 = mobs.values().sum();
    assert!(all_positions > 1_000, "{all_positions} positions made mobs");
    assert_eq!(mobs.get(&kind(entity_types::SLIME)), None);
    for id in [
        entity_types::ZOMBIE,
        entity_types::SKELETON,
        entity_types::CREEPER,
        entity_types::SPIDER,
    ] {
        let expected = f64::from(weight(id)) / f64::from(total);
        let of_positions =
            f64::from(positions.get(&kind(id)).copied().unwrap_or(0)) / f64::from(all_positions);
        let of_mobs = f64::from(mobs.get(&kind(id)).copied().unwrap_or(0)) / f64::from(all_mobs);
        let name = clustine_data::ENTITY_TYPES[usize::try_from(id).expect("an id")];
        assert!(
            (of_positions - expected).abs() < 0.03,
            "{name}: {of_positions:.3} of the positions, {expected:.3} by its weight \
             ({positions:?})"
        );
        assert!(
            (of_mobs - expected).abs() < 0.04,
            "{name}: {of_mobs:.3} of the mobs, {expected:.3} by its weight ({mobs:?})"
        );
    }
}
