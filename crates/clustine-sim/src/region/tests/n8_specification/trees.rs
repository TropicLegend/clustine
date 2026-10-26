//! Saplings and their trees (N8f; the plan's section 7, n8 TreeGrower 101–743,
//! WeightedList 149–166, SaplingBlock 39–146): each grower's lists in the game's order of
//! draws, the square of four found from each of its corners, a failed tree's saplings
//! put back as they were, bone meal's stage and then tree, and a tree beside a pinned
//! line arriving on the other side the tick after, with a gap where its block is refused.

use std::sync::{Arc, OnceLock};

use clustine_world::{Due, EffectId, Effected, FiledEffect};

use super::*;
use crate::region::trees::TreeGrower;

/// The world's features for the seed 2468, read once.
fn features() -> crate::Features {
    static SETTINGS: OnceLock<Arc<clustine_features::FeatureSettings>> = OnceLock::new();
    let settings = SETTINGS.get_or_init(|| {
        Arc::new(
            clustine_features::FeatureSettings::new(2468, clustine_features::Dimension::Overworld)
                .expect("the pinned jar's features"),
        )
    });
    crate::Features(Some(Arc::clone(settings)))
}

/// [`open`] with dirt under each of `saplings` and the world's features.
fn grove(saplings: &[(BlockPos, BlockState)], placed: &[(BlockPos, BlockState)]) -> Region {
    let mut all: Vec<(BlockPos, BlockState)> = saplings
        .iter()
        .map(|(pos, _)| (pos.offset(0, -1, 0), blocks::DIRT))
        .collect();
    all.extend_from_slice(saplings);
    all.extend_from_slice(placed);
    let mut region = open(&all);
    region.config.features = features();
    region
}

fn name(feature: &str) -> &str {
    feature.strip_prefix("minecraft:").unwrap_or(feature)
}

/// What `choose_tree` gives for the sapling at `pos`, drawn from the stream of `seed`:
/// the feature, where, and the stream after.
fn choose(region: &Region, pos: BlockPos, seed: u64) -> (Option<(String, BlockPos)>, Random) {
    let sapling = read(region, pos);
    let grower = TreeGrower::of(sapling).expect("a sapling's grower");
    let mut random = stream(seed);
    let chosen = region.choose_tree(grower, pos, sapling, &mut random);
    (
        chosen.map(|chosen| (name(chosen.feature).to_owned(), chosen.at)),
        random,
    )
}

/// A grower's lists as n8 TreeGrower 514–743 builds them: the plain list, the mega
/// list and the flower list, each of names and weights.
type Lists = (
    &'static [(&'static str, u32)],
    &'static [(&'static str, u32)],
    &'static [(&'static str, u32)],
);

const GROWERS: [(&str, Lists); 11] = [
    (
        "oak_sapling",
        (
            &[("oak", 9), ("fancy_oak", 1)],
            &[],
            &[("oak_bees_005", 9), ("fancy_oak_bees_005", 1)],
        ),
    ),
    (
        "spruce_sapling",
        (
            &[("spruce", 1)],
            &[("mega_spruce", 1), ("mega_pine", 1)],
            &[],
        ),
    ),
    (
        "mangrove_propagule",
        (&[("mangrove", 15), ("tall_mangrove", 85)], &[], &[]),
    ),
    ("azalea", (&[("azalea_tree", 1)], &[], &[])),
    (
        "birch_sapling",
        (&[("birch", 1)], &[], &[("birch_bees_005", 1)]),
    ),
    (
        "jungle_sapling",
        (
            &[("jungle_tree_no_vine", 1)],
            &[("mega_jungle_tree", 1)],
            &[],
        ),
    ),
    ("acacia_sapling", (&[("acacia", 1)], &[], &[])),
    (
        "cherry_sapling",
        (&[("cherry", 1)], &[], &[("cherry_bees_005", 1)]),
    ),
    ("dark_oak_sapling", (&[], &[("dark_oak", 1)], &[])),
    ("pale_oak_sapling", (&[], &[("pale_oak_bonemeal", 1)], &[])),
    (
        "poplar_sapling",
        (
            &[
                ("red_poplar", 1),
                ("orange_poplar", 1),
                ("yellow_poplar", 1),
            ],
            &[],
            &[],
        ),
    ),
];

/// `WeightedList.getRandom` replayed: nothing and no draw for an empty list, else
/// `nextInt` of the total, a list of one too.
fn pick(list: &[(&'static str, u32)], replay: &mut Replay) -> Option<&'static str> {
    if list.is_empty() {
        return None;
    }
    let total = list.iter().map(|(_, weight)| weight).sum();
    let mut drawn = replay.int(total);
    for (name, weight) in list {
        if drawn < *weight {
            return Some(name);
        }
        drawn -= weight;
    }
    unreachable!("the draw is below the total")
}

#[test]
fn a_lone_sapling_draws_its_mega_list_first_and_then_its_flower_or_plain_list() {
    let pos = at(0, GROUND, 0);
    for flowered in [false, true] {
        for (block, (plain, mega, flowers)) in GROWERS {
            let sapling = state(&format!("minecraft:{block}"));
            let placed = if flowered {
                vec![(at(2, GROUND + 1, -2), blocks::POPPY)]
            } else {
                Vec::new()
            };
            let region = grove(&[(pos, sapling)], &placed);
            for seed in 0..40 {
                let (chosen, drawn) = choose(&region, pos, seed);
                let mut replay = Replay::of(seed);
                // The mega list is drawn whatever stands about: no square here.
                pick(mega, &mut replay);
                let list = if flowered && !flowers.is_empty() {
                    flowers
                } else {
                    plain
                };
                let expected = pick(list, &mut replay);
                replay.matches(&drawn, block);
                assert_eq!(
                    chosen,
                    expected.map(|name| (name.to_owned(), pos)),
                    "{block}, flowers {flowered}, seed {seed}"
                );
            }
        }
    }
}

#[test]
fn a_flower_counts_within_two_across_and_one_up_or_down_and_no_farther() {
    let pos = at(0, GROUND, 0);
    for (flower, counts) in [
        (at(2, GROUND + 1, 2), true),
        (at(-2, GROUND - 1, -2), true),
        (at(0, GROUND, 1), true),
        (at(3, GROUND, 0), false),
        (at(0, GROUND + 2, 0), false),
        (at(-2, GROUND, -3), false),
    ] {
        let mut placed = vec![(flower, blocks::DANDELION)];
        if flower.y == GROUND - 1 {
            // A flower sunk in the floor: what matters is where it is.
            placed.push((flower.offset(0, -1, 0), blocks::DIRT));
        }
        let region = grove(&[(pos, blocks::OAK_SAPLING)], &placed);
        let (chosen, _) = choose(&region, pos, 3);
        let name = chosen.expect("an oak").0;
        assert_eq!(name.ends_with("_bees_005"), counts, "{flower:?}: {name}");
    }
}

/// The saplings of a square of `block` from `low`: `(dx, dz)` from it, with stages.
fn square_of(block: &str, low: BlockPos, cells: &[(i32, i32, u8)]) -> Vec<(BlockPos, BlockState)> {
    cells
        .iter()
        .map(|(dx, dz, stage)| {
            (
                low.offset(*dx, 0, *dz),
                state(&format!("minecraft:{block}[stage={stage}]")),
            )
        })
        .collect()
}

#[test]
fn a_square_of_four_is_found_from_each_of_its_corners_and_placed_at_its_north_west() {
    let low = at(0, GROUND, 0);
    let saplings = square_of(
        "spruce_sapling",
        low,
        &[(0, 0, 1), (1, 0, 0), (0, 1, 1), (1, 1, 0)],
    );
    let region = grove(&saplings, &[]);
    for (corner, _) in &saplings {
        let sapling = read(&region, *corner);
        let grower = TreeGrower::of(sapling).expect("spruce");
        for seed in 0..20 {
            let mut random = stream(seed);
            let chosen = region
                .choose_tree(grower, *corner, sapling, &mut random)
                .expect("a mega tree");
            let mut replay = Replay::of(seed);
            let mega = pick(&[("mega_spruce", 1), ("mega_pine", 1)], &mut replay);
            // Found: no other list is drawn.
            replay.matches(&random, "a square");
            assert_eq!(Some(name(chosen.feature)), mega);
            assert_eq!(chosen.at, low, "from {corner:?}");
            // Each sapling with the state it is put back as, in the game's order:
            // the square's own corner, east, south, south-east.
            let order: Vec<BlockPos> = chosen.saplings.iter().map(|(_, at)| *at).collect();
            assert_eq!(
                order,
                [
                    low,
                    low.offset(1, 0, 0),
                    low.offset(0, 0, 1),
                    low.offset(1, 0, 1)
                ]
            );
            for (put_back, at) in &chosen.saplings {
                assert_eq!(*put_back, read(&region, *at));
            }
        }
    }
}

#[test]
fn in_a_three_by_three_the_square_is_tried_with_the_sapling_at_its_corners_in_order() {
    let mut cells = Vec::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            cells.push((dx, dz, 0));
        }
    }
    let saplings = square_of("dark_oak_sapling", at(0, GROUND, 0), &cells);
    let region = grove(&saplings, &[]);
    // From the centre the first offset, (0, 0), is a square; from the south-east
    // corner only the last, (−1, −1); from the north-west the first again.
    for (from, placed_at) in [
        (at(0, GROUND, 0), at(0, GROUND, 0)),
        (at(1, GROUND, 1), at(0, GROUND, 0)),
        (at(-1, GROUND, -1), at(-1, GROUND, -1)),
        (at(1, GROUND, -1), at(0, GROUND, -1)),
        (at(-1, GROUND, 1), at(-1, GROUND, 0)),
    ] {
        let (chosen, _) = choose(&region, from, 1);
        assert_eq!(chosen, Some(("dark_oak".to_owned(), placed_at)), "{from:?}");
    }
}

#[test]
fn three_saplings_and_one_of_another_kind_are_no_square() {
    let low = at(0, GROUND, 0);
    let mut saplings = square_of("spruce_sapling", low, &[(0, 0, 0), (1, 0, 0), (0, 1, 0)]);
    saplings.push((low.offset(1, 0, 1), blocks::BIRCH_SAPLING));
    let region = grove(&saplings, &[]);
    let (chosen, drawn) = choose(&region, low, 4);
    let mut replay = Replay::of(4);
    pick(&[("mega_spruce", 1), ("mega_pine", 1)], &mut replay);
    pick(&[("spruce", 1)], &mut replay);
    replay.matches(&drawn, "no square");
    assert_eq!(chosen, Some(("spruce".to_owned(), low)));
    // A dark oak without its square grows nothing, its mega list drawn.
    let region = grove(&[(low, blocks::DARK_OAK_SAPLING)], &[]);
    let (chosen, drawn) = choose(&region, low, 4);
    let mut replay = Replay::of(4);
    replay.int(1);
    replay.matches(&drawn, "a lone dark oak");
    assert_eq!(chosen, None);
}

#[test]
fn a_mega_tree_without_room_puts_all_four_saplings_back_as_they_were_and_tries_no_other() {
    let low = at(0, GROUND, 0);
    let saplings = square_of(
        "spruce_sapling",
        low,
        &[(0, 0, 1), (1, 0, 0), (0, 1, 1), (1, 1, 1)],
    );
    // A ceiling two blocks up over the square and about it.
    let ceiling: Vec<(BlockPos, BlockState)> = (-3..=4)
        .flat_map(|x| (-3..=4).map(move |z| (at(x, GROUND + 2, z), blocks::STONE)))
        .collect();
    let mut region = grove(&saplings, &ceiling);
    for seed in 0..10 {
        let mut random = stream(seed);
        let mut output = TickOutput::default();
        let sapling = read(&region, low);
        region.advance_tree(low, sapling, &mut random, &mut output);
        let mut replay = Replay::of(seed);
        pick(&[("mega_spruce", 1), ("mega_pine", 1)], &mut replay);
        // The feature's own random's seed; then nothing: no fall back to a lone spruce.
        replay.long();
        replay.matches(&random, "a mega tree that fails");
        for (at, put_back) in &saplings {
            assert_eq!(read(&region, *at), *put_back, "seed {seed}");
        }
        assert!(
            !(-3..=4).any(|x| (-3..=4).any(|z| (GROUND..GROUND + 2).any(|y| {
                clustine_blocks::data::is_block(read(&region, at(x, y, z)), blocks::SPRUCE_LOG)
            }))),
            "no tree"
        );
    }
}

#[test]
fn bone_meal_gives_a_sapling_its_stage_and_then_its_tree_never_both_at_once() {
    let sapling = at(3, GROUND, 3);
    let placed = vec![
        (sapling.offset(0, -1, 0), blocks::DIRT),
        (sapling, blocks::BIRCH_SAPLING),
    ];
    let mut region = super::super::redstone::ticking_with(&placed);
    region.config.features = features();
    let staged = state("minecraft:birch_sapling[stage=1]");
    let mut clicks = 0;
    let mut stage_at = None;
    let mut tree_at = None;
    while tree_at.is_none() && clicks < 200 {
        checked(
            &mut region,
            &super::super::redstone::holding(items::BONE_MEAL),
        );
        checked(&mut region, &super::super::redstone::click(sapling));
        clicks += 1;
        let now = read(&region, sapling);
        if now == staged && stage_at.is_none() {
            stage_at = Some(clicks);
        } else if clustine_blocks::data::is_block(now, blocks::BIRCH_LOG) {
            tree_at = Some(clicks);
        } else {
            assert!(
                now == blocks::BIRCH_SAPLING || now == staged,
                "click {clicks}: {now:?}"
            );
        }
    }
    let (stage_at, tree_at) = (stage_at.expect("staged"), tree_at.expect("a tree"));
    assert!(
        stage_at < tree_at,
        "the stage at {stage_at}, the tree at {tree_at}"
    );
}

#[test]
fn a_tree_beside_a_pinned_line_arrives_beyond_it_a_tick_later_with_a_gap_where_refused() {
    const LINE: i32 = 1;
    let west_area = ChunkArea {
        min_x: None,
        max_x: Some(LINE),
    };
    let east_area = ChunkArea {
        min_x: Some(LINE),
        max_x: None,
    };
    let pos = at(15, GROUND, 8);
    let land: Vec<(ChunkPos, Chunk)> = (-1..=2)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .map(|position| {
            let mut chunk = floor();
            if position == pos.chunk() {
                chunk.set(15, GROUND - 1, 8, blocks::DIRT);
                chunk.set(15, GROUND, 8, state("minecraft:oak_sapling[stage=1]"));
            }
            (position, chunk)
        })
        .collect();
    // A region holds the land of its area and copies the column beside it across the
    // line: the chunks within one of `beside` along x.
    let region_of = |area: ChunkArea, ids: u32, beside: i32| {
        let (chunks, copies): (Vec<_>, Vec<_>) = land
            .iter()
            .filter(|(position, _)| (position.x - beside).abs() <= 1 || area.contains(*position))
            .cloned()
            .partition(|(position, _)| area.contains(*position));
        let tickets: Vec<(ChunkPos, Ticket)> = chunks
            .iter()
            .map(|(position, _)| (*position, Ticket::Viewer))
            .collect();
        let holdings = Holdings {
            held: chunks.iter().map(|(position, _)| *position).collect(),
            pinned: vec![area],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let state = RegionState::new(EntityIds::block(ids).expect("ids"));
        let mut config = config();
        config.features = features();
        Region::restore_loaded(config, state, holdings, &tickets, chunks, copies)
    };
    let mut west = region_of(west_area, 1, 0);
    let mut east = region_of(east_area, 2, 1);
    idle(&mut west);
    idle(&mut east);
    // A seed whose oak grows: the plain list's draw, then the feature's own random.
    let mut grown = None;
    for seed in 0..200 {
        let mut trial = west.clone();
        trial.light_step();
        trial.made.clear();
        let mut random = stream(seed);
        let mut output = TickOutput::default();
        let sapling = read(&trial, pos);
        trial.advance_tree(pos, sapling, &mut random, &mut output);
        if clustine_blocks::data::is_block(read(&trial, pos), blocks::OAK_LOG)
            && trial.made.iter().any(|effect| effect.at.x >= 16)
        {
            grown = Some(trial);
            break;
        }
    }
    let west = grown.expect("an oak that reaches across the line");
    let across: Vec<(BlockPos, BlockState)> = west
        .made
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::SetIf {
                expected, state, ..
            } => {
                assert_eq!(expected, blocks::AIR, "{:?}", effect.at);
                Some((effect.at, state))
            }
            _ => None,
        })
        .collect();
    assert!(!across.is_empty());
    assert!(across.iter().all(|(at, _)| at.chunk().x >= LINE));
    // Before the effects come, nothing beyond the line has changed.
    for (at, _) in &across {
        assert_eq!(read(&east, *at), blocks::AIR);
    }
    // A player builds where one leaf would go, in the holder, before the tree's sets
    // reach it.
    let refused = across[0].0;
    put(&mut east, refused, blocks::COBBLESTONE);
    let made_in = west.tick;
    let effects = west
        .made
        .iter()
        .filter(|effect| east.chunk(effect.at.chunk()).is_some())
        .enumerate()
        .map(|(index, effect)| FiledEffect {
            id: EffectId {
                sequence: 1,
                index: u32::try_from(index).expect("few"),
            },
            effect: *effect,
            due: Due::At { tick: made_in },
        })
        .collect();
    checked(
        &mut east,
        &TickInputs {
            effects,
            ..TickInputs::default()
        },
    );
    for (at, state) in &across {
        if *at == refused {
            assert_eq!(read(&east, *at), blocks::COBBLESTONE, "the gap");
        } else {
            let now = read(&east, *at);
            assert!(
                clustine_blocks::data::is_block(now, *state),
                "{at:?}: {now:?} where {state:?} was sent"
            );
        }
    }
}
