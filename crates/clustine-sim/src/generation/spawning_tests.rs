//! The spawning of a chunk against chunks built by hand, with W scripted: each test's
//! script is a hand-run of the brief's listing (`docs/groundwork/n7-brief.md`, 8.4 and
//! 10.8), every draw W is asked for, in order, with what it answers. A draw the code
//! asks for that the script does not hold, or one the script holds that the code does
//! not ask for, fails the test.

use std::collections::{BTreeMap, VecDeque};

use clustine_data::{BIOME_SPAWNS, DIMENSION_TYPES, blocks, entity_types};
use clustine_world::{Chunk, EntityId};

use super::*;
use crate::region::Random;

/// One draw of W as the listing asks for it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum W {
    /// `nextFloat()`, answering this.
    Float(f32),
    /// `nextInt(bound)`, answering the second.
    Int(u32, u32),
}

use W::{Float, Int};

/// W, scripted.
struct Script(VecDeque<W>);

impl Script {
    fn new(draws: &[W]) -> Self {
        Self(draws.iter().copied().collect())
    }
}

impl Draws for Script {
    fn next_int(&mut self, bound: u32) -> u32 {
        match self.0.pop_front() {
            Some(Int(expected, value)) if expected == bound => value,
            other => panic!("W was asked for nextInt({bound}), the script has {other:?}"),
        }
    }

    fn next_float(&mut self) -> f32 {
        match self.0.pop_front() {
            Some(Float(value)) => value,
            other => panic!("W was asked for nextFloat(), the script has {other:?}"),
        }
    }

    fn next_double(&mut self) -> f64 {
        panic!("W is never asked for nextDouble()")
    }

    fn next_bool(&mut self) -> bool {
        panic!("W is never asked for nextBoolean()")
    }
}

const CENTRE: ChunkPos = ChunkPos { x: 2, z: 3 };
const MIN_X: i32 = 32;
const MIN_Z: i32 = 48;

fn biome(name: &str) -> Biome {
    let index = BIOME_SPAWNS
        .iter()
        .position(|biome| biome.name == name)
        .expect("a biome");
    Biome(u16::try_from(index).expect("few biomes"))
}

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type")
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).expect("a state")
}

/// The centre chunk built by hand, air around it, its light and biomes given.
struct View {
    chunk: Chunk,
    /// The sky's light at every block at or above the ground; under it 0.
    sky: u8,
    /// The sky's light at single blocks, where it is not `sky`.
    sky_at: BTreeMap<BlockPos, u8>,
    zoomed: Biome,
    noise: Biome,
}

impl View {
    /// A chunk of `ground` at y 64, everything above it air, under the sky's 15, in
    /// `zoomed`, whose noise biome at the corner's top is `noise`.
    fn new(ground: BlockState, zoomed: &str, noise: &str) -> Self {
        let overworld = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .expect("the overworld");
        let mut chunk = Chunk::empty(overworld, biome(zoomed));
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, 64, z, ground);
            }
        }
        Self {
            chunk,
            sky: 15,
            sky_at: BTreeMap::new(),
            zoomed: biome(zoomed),
            noise: biome(noise),
        }
    }

    fn set(&mut self, at: BlockPos, state: BlockState) {
        let (x, z) = ((at.x - MIN_X) as usize, (at.z - MIN_Z) as usize);
        self.chunk.set(x, at.y, z, state);
    }
}

impl SpawnView for View {
    fn position(&self) -> ChunkPos {
        CENTRE
    }

    fn max_y(&self) -> i32 {
        319
    }

    fn sea_level(&self) -> i32 {
        63
    }

    fn block(&self, at: BlockPos) -> BlockState {
        if at.chunk() != CENTRE {
            return blocks::AIR;
        }
        let (x, z) = ((at.x - MIN_X) as usize, (at.z - MIN_Z) as usize);
        self.chunk.get(x, at.y, z).unwrap_or(blocks::AIR)
    }

    fn height(&self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        let heights = self.chunk.heights();
        let column = ((z - MIN_Z) << 4 | (x - MIN_X)) as usize;
        let above_bottom = match map {
            FinalHeightmap::WorldSurface => heights.world_surface[column],
            FinalHeightmap::OceanFloor => heights.ocean_floor[column],
            FinalHeightmap::MotionBlocking => heights.motion_blocking[column],
            FinalHeightmap::MotionBlockingNoLeaves => heights.motion_blocking_no_leaves[column],
        };
        self.chunk.min_y() + i32::from(above_bottom)
    }

    fn sky_light(&self, at: BlockPos) -> u8 {
        if let Some(level) = self.sky_at.get(&at) {
            return *level;
        }
        if at.y > 64 { self.sky } else { 0 }
    }

    fn block_light(&self, _at: BlockPos) -> u8 {
        0
    }

    fn zoomed_biome(&self, _at: BlockPos) -> Biome {
        self.zoomed
    }

    fn noise_biome(&self, _at: BlockPos) -> Biome {
        self.noise
    }
}

/// The spawn of `view` with W scripted by `draws`, all of which it must ask for.
fn spawn(view: &View, draws: &[W]) -> Vec<Entity> {
    let mut w = Script::new(draws);
    let mut r = Random::of_entity(5, EntityId(1), 0);
    let made = spawn_chunk(view, 7, 0, &mut w, &mut r);
    assert!(w.0.is_empty(), "W was not asked for {:?}", w.0);
    made
}

/// The kinds, positions and yaws of what was made.
fn placed(made: &[Entity]) -> Vec<(u16, [f64; 3], f32)> {
    made.iter()
        .map(|entity| (entity.kind, entity.position, entity.yaw))
        .collect()
}

/// A step that leaves the column where it is: `nextInt(5)` four times, each 0.
const STAY: [W; 4] = [Int(5, 0), Int(5, 0), Int(5, 0), Int(5, 0)];

fn with(parts: &[&[W]]) -> Vec<W> {
    parts.concat()
}

/// The plains' list (sheep 12, pig 10, chicken 10, cow 8, horse 5, donkey 1: 46): a
/// group of four sheep at (37, 55), each made at its column's top on the block's corner
/// with the yaw W gives, each followed by its step, and a draw of exactly the
/// probability ending the chunk.
#[test]
fn a_chunks_draws_place_a_group_of_sheep_step_by_step() {
    let view = View::new(blocks::GRASS_BLOCK, "minecraft:plains", "minecraft:plains");
    let made = spawn(
        &view,
        &[
            Float(0.05),
            Int(46, 0),
            Int(16, 5),
            Int(16, 7),
            // (37, 55): made, then x + 3 − 1, z + 0 − 0.
            Float(0.25),
            Int(5, 3),
            Int(5, 1),
            Int(5, 0),
            Int(5, 0),
            // (39, 55): made, then x + 1 − 4, z + 2 − 0.
            Float(0.5),
            Int(5, 1),
            Int(5, 4),
            Int(5, 2),
            Int(5, 0),
            // (36, 57) twice.
            Float(0.0),
            Int(5, 0),
            Int(5, 0),
            Int(5, 0),
            Int(5, 0),
            Float(0.75),
            Int(5, 0),
            Int(5, 0),
            Int(5, 0),
            Int(5, 0),
            // 0.1 is not below the plains' 0.1.
            Float(0.1),
        ],
    );
    let sheep = kind(entity_types::SHEEP);
    assert_eq!(
        placed(&made),
        [
            (sheep, [37.0, 65.0, 55.0], 90.0),
            (sheep, [39.0, 65.0, 55.0], 180.0),
            (sheep, [36.0, 65.0, 57.0], 0.0),
            (sheep, [36.0, 65.0, 57.0], 270.0),
        ]
    );
}

/// A column whose box meets a block (stone beside it at the feet) fails at the box:
/// each member's four attempts are spent on the same column without a step, so W is
/// asked for nothing more until the next group's float.
#[test]
fn a_blocked_column_spends_every_attempt_on_itself() {
    let mut view = View::new(blocks::GRASS_BLOCK, "minecraft:plains", "minecraft:plains");
    view.set(BlockPos::new(36, 65, 55), blocks::STONE);
    let made = spawn(
        &view,
        &[Float(0.05), Int(46, 0), Int(16, 5), Int(16, 7), Float(0.5)],
    );
    assert!(made.is_empty());
}

/// A step out of the chunk draws the column again from the group's first, x then z,
/// until it is inside; the sheep at the edge column stands the width in from it.
#[test]
fn a_column_stepping_out_of_the_chunk_is_drawn_again_from_the_first() {
    let view = View::new(blocks::GRASS_BLOCK, "minecraft:plains", "minecraft:plains");
    let made = spawn(
        &view,
        &with(&[
            &[Float(0.05), Int(46, 0), Int(16, 0), Int(16, 7)],
            // (32, 55): made; x + 0 − 2 = 30 is outside.
            &[Float(0.0), Int(5, 0), Int(5, 2), Int(5, 1), Int(5, 1)],
            // Again from (32, 55): x + 0 − 1 = 31 still outside.
            &[Int(5, 0), Int(5, 1), Int(5, 2), Int(5, 2)],
            // Again: x + 3 − 0 = 35, z + 0 − 1 = 54.
            &[Int(5, 3), Int(5, 0), Int(5, 0), Int(5, 1)],
            &[Float(0.0)],
            &STAY,
            &[Float(0.0)],
            &STAY,
            &[Float(0.0)],
            &STAY,
            &[Float(0.5)],
        ]),
    );
    let sheep = kind(entity_types::SHEEP);
    assert_eq!(ENTITY_TYPE_INFO[usize::from(sheep)].width, 0.9);
    let edge = 32.0 + f64::from(0.9_f32);
    assert_eq!(
        placed(&made),
        [
            (sheep, [edge, 65.0, 55.0], 0.0),
            (sheep, [35.0, 65.0, 54.0], 0.0),
            (sheep, [35.0, 65.0, 54.0], 0.0),
            (sheep, [35.0, 65.0, 54.0], 0.0),
        ]
    );
}

/// The clamp at the edge (B 8.0 item 7): a cow at the lowest x column is moved in by
/// its width, 0.9 as a float widened, and keeps the highest z column's corner; a horse at the highest corner by its
/// 1.3964844, so far that its block is the column beside.
#[test]
fn a_cow_and_a_horse_at_an_edge_column_are_moved_in_by_their_width() {
    let view = View::new(blocks::GRASS_BLOCK, "minecraft:plains", "minecraft:plains");
    let mut draws = vec![Float(0.05), Int(46, 32), Int(16, 0), Int(16, 15)];
    for _ in 0..4 {
        draws.push(Float(0.0));
        draws.extend(STAY);
    }
    // A horse, a group of 2 + 0.
    draws.extend([
        Float(0.05),
        Int(46, 40),
        Int(5, 0),
        Int(16, 15),
        Int(16, 15),
    ]);
    for _ in 0..2 {
        draws.push(Float(0.0));
        draws.extend(STAY);
    }
    draws.push(Float(0.5));
    let made = spawn(&view, &draws);
    let (cow, horse) = (kind(entity_types::COW), kind(entity_types::HORSE));
    assert_eq!(ENTITY_TYPE_INFO[usize::from(horse)].width, 1.396_484_4);
    // Narrower than a block, the cow keeps the corner of the highest column: 63 is
    // within 64 − 0.9.
    let cow_at = [32.0 + f64::from(0.9_f32), 65.0, 63.0];
    let horse_edge = 48.0 - f64::from(1.396_484_4_f32);
    let horse_at = [horse_edge, 65.0, 64.0 - f64::from(1.396_484_4_f32)];
    assert_eq!(
        placed(&made),
        [
            (cow, cow_at, 0.0),
            (cow, cow_at, 0.0),
            (cow, cow_at, 0.0),
            (cow, cow_at, 0.0),
            (horse, horse_at, 0.0),
            (horse, horse_at, 0.0),
        ]
    );
    assert_eq!(physics::containing(horse_at), BlockPos::new(46, 65, 62));
}

/// The jungle's list (91, the chicken twice): a parrot goes by the height map with
/// leaves and is made on top of them.
#[test]
fn a_parrot_is_made_on_leaves() {
    let mut view = View::new(blocks::GRASS_BLOCK, "minecraft:jungle", "minecraft:jungle");
    view.set(BlockPos::new(37, 65, 55), blocks::OAK_LEAVES);
    let made = spawn(
        &view,
        &with(&[
            &[Float(0.05), Int(91, 50), Int(2, 0), Int(16, 5), Int(16, 7)],
            &[Float(0.5)],
            &STAY,
            &[Float(0.5)],
        ]),
    );
    assert_eq!(
        placed(&made),
        [(kind(entity_types::PARROT), [37.0, 66.0, 55.0], 180.0)]
    );
}

/// A fox is placed without restriction: no test of the ground. In a layer of snow
/// under leaves that hang just above it, the ground test of a mob placed on the ground
/// fails (the leaves above are no empty block), and a cow is refused there; the fox is
/// made in the snow, which its column's top is (snow does not block motion).
#[test]
fn a_fox_stands_in_snow_with_no_ground_test() {
    let mut view = View::new(blocks::GRASS_BLOCK, "minecraft:taiga", "minecraft:taiga");
    view.set(BlockPos::new(37, 65, 55), state("minecraft:snow[layers=1]"));
    view.set(
        BlockPos::new(37, 66, 55),
        state("minecraft:oak_leaves[persistent=true]"),
    );
    let top = top_non_colliding_pos(&view, kind(entity_types::FOX), 37, 55);
    assert_eq!(top, BlockPos::new(37, 65, 55), "in the snow");
    assert_eq!(
        top_non_colliding_pos(&view, kind(entity_types::COW), 37, 55),
        top
    );
    assert!(is_spawn_position_ok(&view, kind(entity_types::FOX), top));
    assert!(!is_spawn_position_ok(&view, kind(entity_types::COW), top));
    // The taiga's list: sheep 12, pig 10, chicken 10, cow 8, wolf 8, rabbit 4, fox 8.
    let made = spawn(
        &view,
        &with(&[
            &[Float(0.05), Int(60, 52), Int(3, 0), Int(16, 5), Int(16, 7)],
            &[Float(0.0)],
            &STAY,
            &[Float(0.0)],
            &STAY,
            &[Float(0.5)],
        ]),
    );
    let fox = kind(entity_types::FOX);
    assert_eq!(
        placed(&made),
        [
            (fox, [37.0, 65.0, 55.0], 0.0),
            (fox, [37.0, 65.0, 55.0], 0.0)
        ]
    );
}

/// `adjustSpawnPosition` on the ground: not into a slab below (a slab cannot be walked
/// through), into snow of two layers.
#[test]
fn a_cow_is_not_lowered_above_a_slab_and_is_lowered_into_two_layers_of_snow() {
    let mut view = View::new(blocks::GRASS_BLOCK, "minecraft:plains", "minecraft:plains");
    let above = BlockPos::new(37, 66, 55);
    view.set(
        above.offset(0, -1, 0),
        state("minecraft:stone_slab[type=top]"),
    );
    assert_eq!(
        adjust_spawn_position(&view, Placement::OnGround, above),
        above
    );
    view.set(above.offset(0, -1, 0), state("minecraft:snow[layers=2]"));
    assert_eq!(
        adjust_spawn_position(&view, Placement::OnGround, above),
        above.offset(0, -1, 0)
    );
    // A fox's placement lowers nothing.
    assert_eq!(
        adjust_spawn_position(&view, Placement::NoRestrictions, above),
        above
    );
}

/// On grass a cow needs a raw brightness above 8 by its rule: at 8 the rule fails, and
/// so the column is tried again with no yaw and no step; at 9 it is made, grass being
/// worth 10 to it whatever the light.
#[test]
fn light_8_is_refused_and_9_taken_on_grass() {
    let mut view = View::new(blocks::GRASS_BLOCK, "minecraft:plains", "minecraft:plains");
    view.sky = 8;
    let made = spawn(
        &view,
        &[Float(0.05), Int(46, 32), Int(16, 5), Int(16, 7), Float(0.5)],
    );
    assert!(made.is_empty());
    view.sky = 9;
    let mut draws = vec![Float(0.05), Int(46, 32), Int(16, 5), Int(16, 7)];
    for _ in 0..4 {
        draws.push(Float(0.0));
        draws.extend(STAY);
    }
    draws.push(Float(0.5));
    assert_eq!(spawn(&view, &draws).len(), 4);
}

/// On sand a rabbit passes its rule at 11, is made and given its yaw, but its walk
/// target value is the light's, below 0 at 11: refused after the yaw, the step drawn.
/// At 12 the value is 0.50000006 − 0.5, at least 0, and it is made.
#[test]
fn light_11_is_refused_and_12_taken_on_sand_for_a_rabbit() {
    // The desert's list: rabbit 12 (2–3), camel 1.
    let mut view = View::new(blocks::SAND, "minecraft:desert", "minecraft:desert");
    let group = [Float(0.05), Int(13, 0), Int(2, 0), Int(16, 5), Int(16, 7)];
    // Refused, each of the two spends its four attempts, each with a yaw and a step.
    let mut refused = group.to_vec();
    for _ in 0..8 {
        refused.push(Float(0.0));
        refused.extend(STAY);
    }
    refused.push(Float(0.5));
    view.sky = 11;
    assert!(spawn(&view, &refused).is_empty());
    let taken = with(&[
        &group,
        &[Float(0.0)],
        &STAY,
        &[Float(0.0)],
        &STAY,
        &[Float(0.5)],
    ]);
    view.sky = 12;
    assert_eq!(spawn(&view, &taken).len(), 2);
    assert!(magic_value(11) - 0.5 < 0.0);
    assert_eq!(magic_value(12), 0.500_000_06);
}

/// A polar bear on ice where the biome at its block is a frozen ocean; elsewhere it
/// needs grass like any animal.
#[test]
fn a_polar_bear_stands_on_ice_only_in_a_frozen_ocean() {
    let bear = kind(entity_types::POLAR_BEAR);
    let at = BlockPos::new(37, 65, 55);
    let ocean = View::new(
        state("minecraft:ice"),
        "minecraft:frozen_ocean",
        "minecraft:frozen_ocean",
    );
    assert!(check_spawn_rules(&ocean, bear, at));
    let plains = View::new(
        state("minecraft:ice"),
        "minecraft:snowy_plains",
        "minecraft:snowy_plains",
    );
    assert!(!check_spawn_rules(&plains, bear, at));
    let grassy_ocean = View::new(
        blocks::GRASS_BLOCK,
        "minecraft:frozen_ocean",
        "minecraft:frozen_ocean",
    );
    assert!(!check_spawn_rules(&grassy_ocean, bear, at));
    let grassy_plains = View::new(
        blocks::GRASS_BLOCK,
        "minecraft:snowy_plains",
        "minecraft:snowy_plains",
    );
    assert!(check_spawn_rules(&grassy_plains, bear, at));
}

/// A turtle on sand only below the sea level and four more: 66, not 67.
#[test]
fn a_turtle_spawns_only_below_sea_level_and_four() {
    let turtle = kind(entity_types::TURTLE);
    let mut view = View::new(blocks::SAND, "minecraft:beach", "minecraft:beach");
    for y in 65..=66 {
        view.set(BlockPos::new(37, y, 55), blocks::SAND);
    }
    view.set(BlockPos::new(38, 65, 55), blocks::SAND);
    assert!(check_spawn_rules(&view, turtle, BlockPos::new(38, 66, 55)));
    assert!(!check_spawn_rules(&view, turtle, BlockPos::new(37, 67, 55)));
    // Not on sand.
    view.set(BlockPos::new(39, 64, 55), blocks::GRASS_BLOCK);
    assert!(!check_spawn_rules(&view, turtle, BlockPos::new(39, 65, 55)));
}

/// At a biome's edge the list is the zoomed biome's and the probability the noise
/// biome's: snowy plains' list (rabbit 10, polar bear 1) at the plains' 0.1 makes
/// rabbits on a draw of 0.08, which the snowy plains' 0.07 would have stopped at; the
/// other way round, 0.08 stops before any other draw.
#[test]
fn the_list_is_one_biomes_and_the_probability_anothers_at_a_biomes_edge() {
    let edge = View::new(
        blocks::GRASS_BLOCK,
        "minecraft:snowy_plains",
        "minecraft:plains",
    );
    let made = spawn(
        &edge,
        &with(&[
            &[Float(0.08), Int(11, 0), Int(2, 0), Int(16, 5), Int(16, 7)],
            &[Float(0.0)],
            &STAY,
            &[Float(0.0)],
            &STAY,
            &[Float(0.5)],
        ]),
    );
    assert_eq!(made.len(), 2);
    assert!(
        made.iter()
            .all(|made| made.kind == kind(entity_types::RABBIT))
    );
    let other = View::new(
        blocks::GRASS_BLOCK,
        "minecraft:plains",
        "minecraft:snowy_plains",
    );
    assert!(spawn(&other, &[Float(0.08)]).is_empty());
}

/// A biome without creatures draws nothing at all.
#[test]
fn a_biome_without_creatures_draws_nothing() {
    let view = View::new(blocks::GRASS_BLOCK, "minecraft:river", "minecraft:river");
    assert!(spawn(&view, &[]).is_empty());
}

/// The same chunk made twice makes the same mobs, R drawn alike.
#[test]
fn the_same_chunk_made_twice_makes_the_same_mobs() {
    let view = View::new(blocks::GRASS_BLOCK, "minecraft:plains", "minecraft:plains");
    let draws = with(&[
        &[Float(0.05), Int(46, 40), Int(5, 1), Int(16, 5), Int(16, 7)],
        &[Float(0.1)],
        &[Int(5, 1), Int(5, 0), Int(5, 0), Int(5, 0)],
        &[Float(0.2)],
        &STAY,
        &[Float(0.3)],
        &STAY,
        &[Float(0.5)],
    ]);
    let first = spawn(&view, &draws);
    assert_eq!(first.len(), 3);
    assert_eq!(first, spawn(&view, &draws));
    assert_ne!(first[0].uuid, first[1].uuid);
}

/// W is `setDecorationSeed` over `java.util.Random`, worked by hand: the seed
/// scrambled, two longs, the chunk's corner by them, the result scrambled again; then
/// a float is the top 24 bits of the next state and `nextInt(16)` the top 4 of 31.
#[test]
fn the_spawns_random_is_a_legacy_one_seeded_for_the_chunks_decoration() {
    const MULTIPLIER: u64 = 0x5_DEEC_E66D;
    const MASK: u64 = (1 << 48) - 1;
    fn next(state: &mut u64, bits: u32) -> i32 {
        *state = state.wrapping_mul(MULTIPLIER).wrapping_add(11) & MASK;
        (*state >> (48 - bits)) as i32
    }
    fn long(state: &mut u64) -> i64 {
        let high = i64::from(next(state, 32));
        (high << 32).wrapping_add(i64::from(next(state, 32)))
    }
    let (seed, chunk) = (12_345_i64, ChunkPos::new(-3, 7));
    let mut state = (seed as u64 ^ MULTIPLIER) & MASK;
    let a = long(&mut state) | 1;
    let b = long(&mut state) | 1;
    let decoration = (-48_i64)
        .wrapping_mul(a)
        .wrapping_add(112_i64.wrapping_mul(b))
        ^ seed;
    let mut state = (decoration as u64 ^ MULTIPLIER) & MASK;
    let float = next(&mut state, 24) as f32 / (1 << 24) as f32;
    let int = next(&mut state, 31);
    let mut w = SpawnRandom::new(seed, chunk);
    assert_eq!(w.next_float(), float);
    assert_eq!(i64::from(w.next_int(16)), (16 * i64::from(int)) >> 31);
}

/// Every creature an overworld biome makes with a chunk has its rule ported and its
/// type built, so that none is passed over where the game makes it; and every tag the
/// rules name is the game's.
#[test]
fn every_creature_of_an_overworld_list_has_its_rule_and_its_type() {
    use entity_types::*;
    let ported = [
        SHEEP,
        PIG,
        COW,
        CHICKEN,
        HORSE,
        DONKEY,
        MULE,
        LLAMA,
        CAT,
        PANDA,
        TRADER_LLAMA,
        ARMADILLO,
        CAMEL,
        FROG,
        GOAT,
        MOOSHROOM,
        PARROT,
        RABBIT,
        WOLF,
        FOX,
        POLAR_BEAR,
        TURTLE,
    ];
    let elsewhere = [
        "minecraft:basalt_deltas",
        "minecraft:crimson_forest",
        "minecraft:nether_wastes",
        "minecraft:soul_sand_valley",
        "minecraft:warped_forest",
    ];
    for biome in BIOME_SPAWNS
        .iter()
        .filter(|biome| !elsewhere.contains(&biome.name))
    {
        for spawner in biome.of("creature").unwrap_or(&[]) {
            let id = i32::try_from(spawner.kind).expect("a type");
            assert!(ported.contains(&id), "{} makes {id}", biome.name);
            assert!(
                crate::mobs::types::is_built(kind(id)),
                "{} makes {id}, not built",
                biome.name
            );
        }
    }
    let block_tags = clustine_data::TAGS
        .iter()
        .find(|tags| tags.registry == "minecraft:block")
        .expect("block tags");
    for tag in [
        "minecraft:animals_spawnable_on",
        "minecraft:armadillo_spawnable_on",
        "minecraft:camels_spawnable_on",
        "minecraft:frogs_spawnable_on",
        "minecraft:goats_spawnable_on",
        "minecraft:mooshrooms_spawnable_on",
        "minecraft:parrots_spawnable_on",
        "minecraft:rabbits_spawnable_on",
        "minecraft:wolves_spawnable_on",
        "minecraft:foxes_spawnable_on",
        "minecraft:polar_bears_spawnable_on_alternate",
        "minecraft:sand",
        "minecraft:prevent_mob_spawning_inside",
    ] {
        assert!(
            block_tags.tags.iter().any(|known| known.name == tag),
            "{tag}"
        );
    }
    assert!(biome_in_tag(
        biome("minecraft:deep_frozen_ocean"),
        "minecraft:polar_bears_spawn_on_alternate_blocks"
    ));
}

/// A wolf a chunk is made with is saved into its chunk and loaded from it, as the game's
/// is, and the load (`setTame(false, true)` of a wolf without an owner) makes its
/// `max_health` instance at 8: its spawn shows that attribute, and only that, before it
/// has ticked, as the official server's frozen wolves showed it.
#[test]
fn a_wolf_a_chunk_is_made_with_shows_the_most_health_its_load_made() {
    let making = Generated {
        dimension: 0,
        chunk: CENTRE,
        structure: false,
        index: 0,
    };
    let mut r = Random::of_entity(5, EntityId(1), 0);
    let wolf = kind(entity_types::WOLF);
    let made = made_mob(
        7,
        making,
        wolf,
        SpawnReason::ChunkGeneration,
        &mut None,
        None,
        Some("minecraft:taiga"),
        [40.0, 65.0, 50.0],
        90.0,
        &mut r,
    )
    .expect("a wolf is built");
    let mob = crate::mobs::Mob::decode(&made.data).expect("a mob's record");
    let shown: Vec<(u16, f64)> = mob
        .body
        .attributes
        .syncable(wolf, false)
        .into_iter()
        .map(|instance| (instance.attribute, instance.base))
        .collect();
    let max_health = u16::try_from(clustine_data::attributes::MAX_HEALTH).expect("ids fit");
    assert_eq!(shown, [(max_health, 8.0)]);
}
