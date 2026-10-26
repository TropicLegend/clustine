//! Saplings and their trees (part D4a of N8, `docs/groundwork/n8-plan.md`, sections 1.6
//! and 7), worked from the class dumps: n8 SaplingBlock, MangrovePropaguleBlock,
//! TreeGrower and WeightedList.
//!
//! The scene is the 3×3 of stone floors about the origin (the floor at y = −61), with
//! dirt under each sapling, lit by the region's own light step, and given the world's
//! features of the seed 13579 as a server's regions are. What a rule draws is checked
//! against a stream that draws what the game's rule draws from the level's random, in its
//! order: the sapling's chance and the grower's lists, then one long for the feature's
//! own random (the plan's 7.3).

use std::sync::{Arc, OnceLock};

use clustine_blocks::data::is_block;
use clustine_world::Effected;

use super::*;
use crate::region::Random;
use crate::region::trees::{self, TreeGrower};

const NOON: u64 = 6_000;
const MIDNIGHT: u64 = 18_000;
const GROUND: i32 = -60;

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// The world's features of the seed 13579, read once for all the tests.
fn features() -> crate::Features {
    static SETTINGS: OnceLock<Arc<clustine_features::FeatureSettings>> = OnceLock::new();
    let settings = SETTINGS.get_or_init(|| {
        Arc::new(
            clustine_features::FeatureSettings::new(13579, clustine_features::Dimension::Overworld)
                .expect("the pinned jar's features are read whole"),
        )
    });
    crate::Features(Some(Arc::clone(settings)))
}

/// A sapling of `block` at `stage`.
fn sapling(block: &str, stage: u8) -> BlockState {
    state(&format!("minecraft:{block}[stage={stage}]"))
}

/// A region with the 3×3 of floors about the origin, dirt under each of `saplings` and
/// they on it, `placed` put there too, without any update; given the world's features,
/// lit, at `day`.
fn scene(
    saplings: &[(BlockPos, BlockState)],
    placed: &[(BlockPos, BlockState)],
    day: u64,
) -> Region {
    let mut region = region();
    region.config.features = features();
    for x in -1..=1 {
        for z in -1..=1 {
            region.chunks.insert(ChunkPos::new(x, z), floor());
        }
    }
    for (pos, sapling) in saplings {
        put(&mut region, pos.offset(0, -1, 0), blocks::DIRT);
        put(&mut region, *pos, *sapling);
    }
    for (pos, placed) in placed {
        put(&mut region, *pos, *placed);
    }
    region.clock.day = day;
    region.light_step();
    region
}

/// Puts `state` at `pos` without any update, as a test sets the scene.
fn put(region: &mut Region, pos: BlockPos, state: BlockState) {
    let (x, z) = pos.in_chunk();
    region
        .chunks
        .get_mut(&pos.chunk())
        .expect("a loaded chunk")
        .set(x, pos.y, z, state);
}

fn read(region: &Region, pos: BlockPos) -> BlockState {
    region.block(pos).expect("a loaded block")
}

/// A stream of its own for each `seed`.
fn stream(seed: u64) -> Random {
    Random::of_section(seed, ChunkPos::new(0, 0), 0, seed.wrapping_mul(31))
}

/// The random tick of what stands at `pos`, drawn from the stream of `seed`; the stream
/// as the rule left it.
fn random_tick(region: &mut Region, pos: BlockPos, seed: u64) -> (Random, TickOutput) {
    let state = read(region, pos);
    let mut random = stream(seed);
    let mut output = TickOutput::default();
    assert!(
        region.growth_random_tick(pos, state, &mut random, &mut output),
        "{state:?} grows by light"
    );
    (random, output)
}

/// `advanceTree` of what stands at `pos`, drawn from the stream of `seed`.
fn advance(region: &mut Region, pos: BlockPos, seed: u64) -> (Random, TickOutput) {
    let state = read(region, pos);
    let mut random = stream(seed);
    let mut output = TickOutput::default();
    region.advance_tree(pos, state, &mut random, &mut output);
    (random, output)
}

/// The stream of `seed` after `bounds`, each a `nextInt` of its bound, and then
/// `longs` longs.
fn drawn(seed: u64, bounds: &[u32], longs: usize) -> Random {
    let mut random = stream(seed);
    for bound in bounds {
        random.next_int(*bound);
    }
    for _ in 0..longs {
        random.next_u64();
    }
    random
}

/// The first seed from `from` whose stream's first `nextInt(7)` is 0: a sapling's
/// success.
fn success(from: u64) -> u64 {
    (from..)
        .find(|seed| stream(*seed).next_int(7) == 0)
        .expect("one in seven")
}

/// Whether a block of `block` stands in the box from `low` to `high`.
fn stands(region: &Region, block: BlockState, low: BlockPos, high: BlockPos) -> bool {
    (low.x..=high.x).any(|x| {
        (low.y..=high.y)
            .any(|y| (low.z..=high.z).any(|z| is_block(read(region, at(x, y, z)), block)))
    })
}

/// The blocks changed in `output`, in order.
fn changes(output: &TickOutput) -> Vec<(BlockPos, BlockState)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } => Some((*position, *state)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_oak_sapling_in_the_open_by_day_takes_stage_1_at_a_success_and_grows_a_tree_at_the_next() {
    let pos = at(0, GROUND, 0);
    let mut region = scene(&[(pos, sapling("oak_sapling", 0))], &[], NOON);
    assert!(region.max_local_raw_brightness(pos.offset(0, 1, 0)) >= Some(9));

    // A failure draws its chance and nothing more.
    let failure = (0..).find(|seed| stream(*seed).next_int(7) != 0).unwrap();
    let (random, _) = random_tick(&mut region, pos, failure);
    assert_eq!(random, drawn(failure, &[7], 0));
    assert_eq!(read(&region, pos), sapling("oak_sapling", 0));

    let first = success(0);
    let (random, output) = random_tick(&mut region, pos, first);
    assert_eq!(
        random,
        drawn(first, &[7], 0),
        "the stage draws nothing more"
    );
    assert_eq!(read(&region, pos), sapling("oak_sapling", 1));
    assert_eq!(changes(&output), [(pos, sapling("oak_sapling", 1))]);

    let second = success(first + 1);
    let (random, _) = random_tick(&mut region, pos, second);
    // The chance, oak's plain list of weights 9 and 1 (its mega list is empty and draws
    // nothing), and the feature's seed.
    assert_eq!(random, drawn(second, &[7, 10], 1));
    assert_eq!(random.draws_since(&stream(second)), 3);
    assert!(is_block(read(&region, pos), blocks::OAK_LOG));
    assert!(stands(
        &region,
        blocks::OAK_LEAVES,
        at(-3, GROUND, -3),
        at(3, GROUND + 8, 3)
    ));
}

#[test]
fn a_sapling_in_the_open_at_midnight_draws_nothing_and_grows_nothing() {
    let pos = at(0, GROUND, 0);
    let mut region = scene(&[(pos, sapling("oak_sapling", 1))], &[], MIDNIGHT);
    assert!(region.max_local_raw_brightness(pos.offset(0, 1, 0)) < Some(9));
    for seed in 0..64 {
        let (random, output) = random_tick(&mut region, pos, seed);
        assert_eq!(random, stream(seed), "the light is read first, seed {seed}");
        assert!(changes(&output).is_empty());
    }
    assert_eq!(read(&region, pos), sapling("oak_sapling", 1));
}

#[test]
fn a_sapling_under_a_torch_giving_9_at_midnight_grows() {
    let pos = at(0, GROUND, 0);
    // The torch is four across and one down from the block above the sapling: 14 − 5.
    let mut region = scene(
        &[(pos, sapling("birch_sapling", 0))],
        &[(at(4, GROUND, 0), blocks::TORCH)],
        MIDNIGHT,
    );
    assert_eq!(
        region.max_local_raw_brightness(pos.offset(0, 1, 0)),
        Some(9)
    );
    let seed = success(0);
    let (random, _) = random_tick(&mut region, pos, seed);
    assert_eq!(random, drawn(seed, &[7], 0));
    assert_eq!(read(&region, pos), sapling("birch_sapling", 1));
}

#[test]
fn four_spruce_saplings_in_a_square_grow_a_mega_spruce_and_a_lone_one_a_spruce() {
    let square = [
        at(0, GROUND, 0),
        at(1, GROUND, 0),
        at(0, GROUND, 1),
        at(1, GROUND, 1),
    ];
    let spruce = sapling("spruce_sapling", 1);
    let saplings: Vec<(BlockPos, BlockState)> = square.iter().map(|pos| (*pos, spruce)).collect();
    // From the square's far corner, (1, 1): the first square of the four searched that
    // holds four is the one at (−1, −1) from it.
    let pos = square[3];
    for seed in 0..8 {
        let mut region = scene(&saplings, &[], NOON);
        let chosen = region
            .choose_tree(&trees::SPRUCE, pos, spruce, &mut stream(seed))
            .expect("a tree");
        let mega =
            ["minecraft:mega_spruce", "minecraft:mega_pine"][stream(seed).next_int(2) as usize];
        assert_eq!(chosen.feature, mega);
        assert_eq!(chosen.at, square[0]);
        let expected: Vec<(BlockState, BlockPos)> = square.iter().map(|at| (spruce, *at)).collect();
        assert_eq!(chosen.saplings, expected);

        let (random, _) = advance(&mut region, pos, seed);
        assert_eq!(random, drawn(seed, &[2], 1), "the mega list, then the seed");
        for at in square {
            assert!(
                is_block(read(&region, at), blocks::SPRUCE_LOG),
                "{at:?} is the trunk, seed {seed}"
            );
        }
    }

    let alone = at(0, GROUND, 0);
    let mut region = scene(&[(alone, spruce)], &[], NOON);
    let chosen = region
        .choose_tree(&trees::SPRUCE, alone, spruce, &mut stream(3))
        .expect("a tree");
    assert_eq!(chosen.feature, "minecraft:spruce");
    assert_eq!(chosen.saplings, [(spruce, alone)]);
    let (random, _) = advance(&mut region, alone, 3);
    // The mega list is drawn first all the same, then the plain list of one.
    assert_eq!(random, drawn(3, &[2, 1], 1));
    assert!(is_block(read(&region, alone), blocks::SPRUCE_LOG));
    assert!(!is_block(
        read(&region, at(1, GROUND, 0)),
        blocks::SPRUCE_LOG
    ));
}

#[test]
fn a_sapling_without_room_is_taken_away_and_put_back_at_stage_1() {
    let pos = at(0, GROUND, 0);
    // Stone three above: an oak's trunk is at least four.
    let ceiling = at(0, GROUND + 3, 0);
    let mut region = scene(
        &[(pos, sapling("oak_sapling", 1))],
        &[(ceiling, blocks::STONE)],
        NOON,
    );
    assert_eq!(
        TreeGrower::of(sapling("oak_sapling", 1))
            .unwrap()
            .minimum_height(),
        Some(4)
    );
    for seed in 0..16 {
        let (random, output) = advance(&mut region, pos, seed);
        assert_eq!(random, drawn(seed, &[10], 1));
        assert_eq!(read(&region, pos), sapling("oak_sapling", 1), "seed {seed}");
        assert_eq!(
            changes(&output),
            [(pos, blocks::AIR), (pos, sapling("oak_sapling", 1))],
            "taken away to its fluid's block, and put back"
        );
        // Flags 818 tell the clients; 260 do not, and Clustine tells them anyway.
        let seen: Vec<bool> = output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::BlockChanged { seen, .. } => Some(*seen),
                _ => None,
            })
            .collect();
        assert_eq!(seen, [true, false]);
    }
}

#[test]
fn an_oak_beside_a_flower_draws_from_its_flower_list_and_may_bear_a_bee_nest() {
    let pos = at(0, GROUND, 0);
    let flower = at(2, GROUND, -2);
    let oak = sapling("oak_sapling", 1);
    let region = scene(&[(pos, oak)], &[(flower, blocks::POPPY)], NOON);
    for seed in 0..32 {
        let chosen = region
            .choose_tree(&trees::OAK, pos, oak, &mut stream(seed))
            .expect("a tree");
        let expected = if stream(seed).next_int(10) < 9 {
            "minecraft:oak_bees_005"
        } else {
            "minecraft:fancy_oak_bees_005"
        };
        assert_eq!(chosen.feature, expected, "seed {seed}");
    }
    // Three out, the flower is not near.
    let far = scene(&[(pos, oak)], &[(at(3, GROUND, 0), blocks::POPPY)], NOON);
    let chosen = far
        .choose_tree(&trees::OAK, pos, oak, &mut stream(0))
        .unwrap();
    assert!(!chosen.feature.contains("bees"));

    // One tree in twenty of those bears a nest.
    let nest = (0..400).find(|seed| {
        let mut region = scene(&[(pos, oak)], &[(flower, blocks::POPPY)], NOON);
        advance(&mut region, pos, *seed);
        stands(
            &region,
            blocks::BEE_NEST,
            at(-2, GROUND, -2),
            at(2, GROUND + 8, 2),
        )
    });
    assert!(nest.is_some(), "a bee nest within 400 trees");
}

#[test]
fn a_mangrove_propagule_on_the_ground_grows_in_the_dark() {
    let pos = at(0, GROUND, 0);
    let propagule =
        state("minecraft:mangrove_propagule[stage=0,age=0,hanging=false,waterlogged=false]");
    let mut region = scene(&[(pos, propagule)], &[], MIDNIGHT);
    assert!(region.max_local_raw_brightness(pos.offset(0, 1, 0)) < Some(9));
    let first = success(0);
    let (random, _) = random_tick(&mut region, pos, first);
    assert_eq!(random, drawn(first, &[7], 0), "no light read");
    assert_eq!(
        read(&region, pos),
        state("minecraft:mangrove_propagule[stage=1,age=0,hanging=false,waterlogged=false]")
    );
    let second = success(first + 1);
    let (random, _) = random_tick(&mut region, pos, second);
    // The chance, the plain list of weights 15 and 85, the seed.
    assert_eq!(random, drawn(second, &[7, 100], 1));
    assert!(stands(
        &region,
        blocks::MANGROVE_LOG,
        at(-3, GROUND, -3),
        at(3, GROUND + 12, 3)
    ));
}

#[test]
fn a_dark_oak_sapling_alone_never_grows_and_draws_its_mega_list_only() {
    let pos = at(0, GROUND, 0);
    let dark_oak = sapling("dark_oak_sapling", 1);
    let mut region = scene(&[(pos, dark_oak)], &[], NOON);
    for seed in 0..32 {
        let (random, output) = advance(&mut region, pos, seed);
        assert_eq!(random, drawn(seed, &[1], 0), "seed {seed}");
        assert!(changes(&output).is_empty(), "never taken away");
        assert_eq!(read(&region, pos), dark_oak);
    }
    assert_eq!(TreeGrower::of(dark_oak).unwrap().minimum_height(), None);
}

#[test]
fn a_weighted_list_of_one_draws_and_an_empty_one_does_not() {
    for seed in 0..16 {
        let mut random = stream(seed);
        assert_eq!(
            trees::weighted(&[("minecraft:spruce", 1)], &mut random),
            Some("minecraft:spruce")
        );
        assert_eq!(random, drawn(seed, &[1], 0));
        let mut random = stream(seed);
        assert_eq!(trees::weighted(&[], &mut random), None);
        assert_eq!(random, stream(seed));
        let mut random = stream(seed);
        const LIST: &[(&str, u32)] = &[("a", 15), ("b", 85)];
        let expected = if stream(seed).next_int(100) < 15 {
            "a"
        } else {
            "b"
        };
        assert_eq!(trees::weighted(LIST, &mut random), Some(expected));
    }
}

#[test]
fn can_grow_draws_the_plain_list_then_the_mega_list_and_wants_a_square_for_a_dark_oak() {
    let pos = at(0, GROUND, 0);
    let dark_oak = sapling("dark_oak_sapling", 0);
    let alone = scene(&[(pos, dark_oak)], &[], NOON);
    let mut random = stream(5);
    assert!(!alone.can_grow(&trees::DARK_OAK, pos, dark_oak, &mut random));
    assert_eq!(random, drawn(5, &[1], 0));
    let square: Vec<(BlockPos, BlockState)> = [(0, 0), (1, 0), (0, 1), (1, 1)]
        .into_iter()
        .map(|(x, z)| (at(x, GROUND, z), dark_oak))
        .collect();
    let four = scene(&square, &[], NOON);
    assert!(four.can_grow(&trees::DARK_OAK, pos, dark_oak, &mut stream(5)));
    let oak = sapling("oak_sapling", 0);
    let mut random = stream(5);
    assert!(alone.can_grow(&trees::OAK, pos, oak, &mut random));
    assert_eq!(
        random,
        drawn(5, &[10], 0),
        "oak's plain list; its mega list is empty"
    );
}

#[test]
fn a_region_without_features_takes_the_stage_and_draws_the_lists_but_grows_nothing() {
    let pos = at(0, GROUND, 0);
    let mut region = scene(&[(pos, sapling("oak_sapling", 0))], &[], NOON);
    region.config.features = crate::Features::default();
    let (random, _) = advance(&mut region, pos, 1);
    assert_eq!(random, stream(1));
    assert_eq!(read(&region, pos), sapling("oak_sapling", 1));
    for seed in 0..16 {
        let (random, output) = advance(&mut region, pos, seed);
        assert_eq!(random, drawn(seed, &[10], 0), "the list, and no seed");
        assert!(changes(&output).is_empty());
        assert_eq!(read(&region, pos), sapling("oak_sapling", 1));
    }
}

#[test]
fn a_tree_beside_a_pinned_line_sends_its_leaves_across_as_conditional_sets() {
    const WESTERN: ChunkArea = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };
    let pos = at(15, GROUND, 8);
    let oak = sapling("oak_sapling", 1);
    let mut land: Vec<(ChunkPos, Chunk)> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| (ChunkPos::new(x, z), floor())))
        .collect();
    for (position, chunk) in &mut land {
        if *position == pos.chunk() {
            chunk.set(15, GROUND - 1, 8, blocks::DIRT);
            chunk.set(15, GROUND, 8, oak);
        }
    }
    let (chunks, copies): (Vec<_>, Vec<_>) = land
        .into_iter()
        .partition(|(position, _)| WESTERN.contains(*position));
    let tickets: Vec<(ChunkPos, Ticket)> = chunks
        .iter()
        .map(|(position, _)| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: chunks.iter().map(|(position, _)| *position).collect(),
        pinned: vec![WESTERN],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let state = RegionState::new(EntityIds::block(1).expect("a block of ids"));
    let mut config = config();
    config.features = features();
    let mut west = Region::restore_loaded(config, state, holdings, &tickets, chunks, copies);

    let (_, _) = advance(&mut west, pos, 7);
    assert!(is_block(read(&west, pos), blocks::OAK_LOG));
    let across: Vec<&Effect> = west
        .made
        .iter()
        .filter(|effect| matches!(effect.what, Effected::SetIf { .. }))
        .collect();
    assert!(!across.is_empty(), "the leaves beyond the line");
    for effect in across {
        assert_eq!(effect.at.chunk().x, 1, "{:?} is beyond the line", effect.at);
        let Effected::SetIf {
            expected,
            state,
            flags,
        } = effect.what
        else {
            unreachable!()
        };
        assert_eq!(expected, blocks::AIR);
        assert!(is_block(state, blocks::OAK_LEAVES), "{state:?}");
        assert_eq!(flags, 19);
    }
}
