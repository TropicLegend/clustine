//! Bone meal (part D4b of N8, `docs/groundwork/n8-plan.md`, section 7.5), from the class
//! dumps: `BoneMealItem.growCrop` and each block's `isValidBonemealTarget`,
//! `isBonemealSuccess` and `performBonemeal`, a player's use and its sparks, and nothing
//! in a chunk that does not tick.
//!
//! The tests drive `grow_crop` on the stone floor of the 5×5 chunks about the origin (the
//! middle 3×3 ticking), at noon, with a stream of their own for each seed; what it drew
//! is checked against a stream that draws what the game's blocks draw, in their order.
//! The features (grass's flowers, the huge mushrooms) are the world's of the seed 13579.

use std::collections::BTreeSet;

use clustine_blocks::data::{HORIZONTAL, face_name, in_tag, is_block, relative};
use clustine_world::Effected;

use super::*;
use crate::region::Random;

const NOON: u64 = 6_000;

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// A region that ticks the 3×3 about the origin, `placed` laid in its chunks, lit, at
/// noon, player 1 at the spawn.
fn scene(placed: &[(BlockPos, BlockState)]) -> Region {
    let mut region = super::redstone::ticking_with(placed);
    region.clock.day = NOON;
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
    region.block(pos).expect("a readable block")
}

/// A stream of its own for each `seed`.
fn stream(seed: u64) -> Random {
    Random::of_section(seed, ChunkPos::new(0, 0), 0, seed.wrapping_mul(31))
}

/// Bone meal on the block at `pos`, drawn from the stream of `seed`: whether the block
/// took it, the stream as it was left, and what the tick sent.
fn bone(region: &mut Region, pos: BlockPos, seed: u64) -> (bool, Random, TickOutput) {
    let mut random = stream(seed);
    let mut output = TickOutput::default();
    let used = region.grow_crop(pos, &mut random, &mut output);
    (used, random, output)
}

/// The world's features of the seed 13579, as a server's regions are given them.
fn features() -> crate::Features {
    crate::Features(Some(std::sync::Arc::new(
        clustine_features::FeatureSettings::new(13579, clustine_features::Dimension::Overworld)
            .expect("the pinned jar's features are read whole"),
    )))
}

fn aged(block: &str, age: u8) -> BlockState {
    state(&format!("minecraft:{block}[age={age}]"))
}

#[test]
fn wheat_given_bone_meal_ages_by_two_to_five_and_never_past_its_oldest() {
    let wheat = at(0, -60, 0);
    let mut region = scene(&[(at(0, -61, 0), blocks::FARMLAND), (wheat, blocks::WHEAT)]);
    let mut increases = BTreeSet::new();
    for seed in 0..400 {
        let age = (seed % 8) as u8;
        put(&mut region, wheat, aged("wheat", age));
        let (used, random, _) = bone(&mut region, wheat, seed);
        let mut expected = stream(seed);
        if age == 7 {
            // Grown out: no target, nothing drawn, nothing used.
            assert!(!used);
            assert_eq!(random, expected);
            assert_eq!(read(&region, wheat), aged("wheat", 7));
            continue;
        }
        // `Mth.nextInt(random, 2, 5)`, drawn once.
        let increase = expected.next_int(4) as u8 + 2;
        increases.insert(increase);
        assert!(used);
        assert_eq!(random.draws_since(&stream(seed)), 1);
        assert_eq!(random, expected, "seed {seed}");
        assert_eq!(read(&region, wheat), aged("wheat", (age + increase).min(7)));
    }
    assert_eq!(increases, BTreeSet::from([2, 3, 4, 5]));
}

#[test]
fn beetroot_given_bone_meal_ages_by_a_third_of_two_to_five() {
    let beetroot = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::FARMLAND),
        (beetroot, blocks::BEETROOTS),
    ]);
    let mut stayed = 0;
    for seed in 0..400 {
        let age = (seed % 4) as u8;
        put(&mut region, beetroot, aged("beetroots", age));
        let (used, random, _) = bone(&mut region, beetroot, seed);
        let mut expected = stream(seed);
        if age == 3 {
            assert!(!used);
            assert_eq!(random, expected);
            continue;
        }
        // `Mth.nextInt(2, 5)`, then a third of it.
        let drawn = expected.next_int(4) as u8 + 2;
        let increase = drawn / 3;
        stayed += usize::from(increase == 0);
        assert!(used);
        assert_eq!(random, expected);
        assert_eq!(read(&region, beetroot), aged("beetroots", age + increase));
    }
    // One draw in four is a 2, which a third makes nought: the bone meal is used and
    // the beetroot does not grow. About 75 of 300.
    assert!((40..115).contains(&stayed), "{stayed}");
}

#[test]
fn the_torchflower_crop_flowers_at_its_second_bone_meal_drawing_nothing() {
    let crop = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::FARMLAND),
        (crop, blocks::TORCHFLOWER_CROP),
    ]);
    let (used, random, _) = bone(&mut region, crop, 1);
    assert!(used);
    assert_eq!(random, stream(1));
    assert_eq!(read(&region, crop), aged("torchflower_crop", 1));
    let (used, random, _) = bone(&mut region, crop, 2);
    assert!(used);
    assert_eq!(random, stream(2));
    assert_eq!(read(&region, crop), blocks::TORCHFLOWER);
    // The flower takes no bone meal.
    assert!(!bone(&mut region, crop, 3).0);
}

#[test]
fn the_pitcher_crop_grows_one_age_a_bone_meal_and_two_tall_from_age_3() {
    let pod = at(0, -60, 0);
    let above = at(0, -59, 0);
    let pitcher =
        |age: u8, half: &str| state(&format!("minecraft:pitcher_crop[age={age},half={half}]"));
    let mut region = scene(&[
        (at(0, -61, 0), blocks::FARMLAND),
        (pod, pitcher(0, "lower")),
    ]);
    for age in 1..=3 {
        let (used, random, _) = bone(&mut region, pod, u64::from(age));
        assert!(used);
        assert_eq!(random, stream(u64::from(age)), "it draws nothing");
        assert_eq!(read(&region, pod), pitcher(age, "lower"));
    }
    assert_eq!(read(&region, above), pitcher(3, "upper"));
    // The upper half takes bone meal for the lower.
    assert!(bone(&mut region, above, 4).0);
    assert_eq!(read(&region, pod), pitcher(4, "lower"));
    assert_eq!(read(&region, above), pitcher(4, "upper"));
    // Grown out, neither half is a target.
    assert!(!bone(&mut region, pod, 5).0);
    assert!(!bone(&mut region, above, 6).0);
}

#[test]
fn a_stem_given_bone_meal_reaches_7_and_then_takes_its_random_tick_which_may_fruit() {
    let stem = at(0, -60, 0);
    let mut placed = vec![(at(0, -61, 0), blocks::FARMLAND)];
    for face in HORIZONTAL {
        placed.push((relative(at(0, -61, 0), face), blocks::DIRT));
    }
    let mut region = scene(&placed);
    let mut fruited = 0;
    for seed in 0..400 {
        for face in HORIZONTAL {
            put(&mut region, relative(stem, face), blocks::AIR);
        }
        put(&mut region, stem, aged("pumpkin_stem", 5));
        let (used, random, _) = bone(&mut region, stem, seed);
        assert!(used);
        let mut expected = stream(seed);
        // `Mth.nextInt(2, 5)`: from 5 it is always 7, and the random tick follows.
        let _ = expected.next_int(4);
        let speed = region.growth_speed(aged("pumpkin_stem", 7), stem);
        let bound = (25.0_f32 / speed) as u32 + 1;
        if expected.next_int(bound) == 0 {
            let face = HORIZONTAL[expected.next_int(4) as usize];
            assert_eq!(read(&region, relative(stem, face)), blocks::PUMPKIN);
            assert_eq!(
                read(&region, stem),
                state(&format!(
                    "minecraft:attached_pumpkin_stem[facing={}]",
                    face_name(face)
                ))
            );
            fruited += 1;
        } else {
            assert_eq!(read(&region, stem), aged("pumpkin_stem", 7));
        }
        assert_eq!(random, expected, "seed {seed}");
    }
    // One in 13 (dry farmland below, dirt about it): about 31 of 400.
    assert!((12..60).contains(&fruited), "{fruited}");
    // At 7 the stem is no target.
    put(&mut region, stem, aged("pumpkin_stem", 7));
    let (used, random, _) = bone(&mut region, stem, 9);
    assert!(!used);
    assert_eq!(random, stream(9));
}

#[test]
fn sweet_berries_and_cocoa_age_by_one_until_they_are_ripe() {
    let bush = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::DIRT),
        (bush, blocks::SWEET_BERRY_BUSH),
    ]);
    for age in 1..=3 {
        let (used, random, _) = bone(&mut region, bush, u64::from(age));
        assert!(used);
        assert_eq!(random, stream(u64::from(age)));
        assert_eq!(read(&region, bush), aged("sweet_berry_bush", age));
    }
    assert!(!bone(&mut region, bush, 4).0);
    // Cocoa on a jungle log: by one to 2, its oldest.
    let pod = at(2, -60, 0);
    let mut region = scene(&[
        (at(2, -60, 1), blocks::JUNGLE_LOG),
        (pod, state("minecraft:cocoa[age=0,facing=south]")),
    ]);
    for age in 1..=2_u8 {
        let (used, random, _) = bone(&mut region, pod, u64::from(age));
        assert!(used);
        assert_eq!(random, stream(u64::from(age)));
        assert_eq!(
            read(&region, pod),
            state(&format!("minecraft:cocoa[age={age},facing=south]"))
        );
    }
    assert!(!bone(&mut region, pod, 3).0);
}

#[test]
fn bamboo_grows_one_or_two_stalks_and_its_shoot_grows_bamboo() {
    let stalk = at(0, -60, 0);
    let mut grew = BTreeSet::new();
    for seed in 0..40 {
        let mut region = scene(&[(at(0, -61, 0), blocks::DIRT), (stalk, blocks::BAMBOO)]);
        let (used, random, _) = bone(&mut region, stalk, seed);
        assert!(used);
        let mut expected = stream(seed);
        // `1 + nextInt(2)` new stalks; below height 11 `growBamboo` draws nothing.
        let stalks = 1 + expected.next_int(2) as i32;
        assert_eq!(random, expected);
        for up in 1..=stalks {
            assert!(is_block(
                read(&region, stalk.offset(0, up, 0)),
                blocks::BAMBOO
            ));
        }
        assert_eq!(read(&region, stalk.offset(0, stalks + 1, 0)), blocks::AIR);
        grew.insert(stalks);
    }
    assert_eq!(grew, BTreeSet::from([1, 2]));
    // The shoot grows a stalk with small leaves above it, and becomes bamboo.
    let shoot = at(5, -60, 5);
    let mut region = scene(&[
        (at(5, -61, 5), blocks::DIRT),
        (shoot, blocks::BAMBOO_SAPLING),
    ]);
    let (used, random, _) = bone(&mut region, shoot, 1);
    assert!(used);
    assert_eq!(random, stream(1));
    let above = read(&region, at(5, -59, 5));
    assert!(is_block(above, blocks::BAMBOO));
    assert_eq!(above.property("leaves"), Some("small"));
    assert!(is_block(read(&region, shoot), blocks::BAMBOO));
}

/// A field of grass blocks at y = −61, eleven across about `centre`.
fn grass_field(centre: BlockPos) -> Vec<(BlockPos, BlockState)> {
    (-10..=10)
        .flat_map(|dx| (-10..=10).map(move |dz| (centre.offset(dx, 0, dz), blocks::GRASS_BLOCK)))
        .collect()
}

#[test]
fn grass_given_bone_meal_makes_short_grass_about_it_within_its_walks() {
    let grass = at(0, -61, 0);
    let mut region = scene(&grass_field(grass));
    region.config.features = features();
    let (used, random, _) = bone(&mut region, grass, 3);
    assert!(used);
    // 128 walks, each of which ends in a draw at least (its one in eight).
    assert!(random.draws_since(&stream(3)) > 128);
    let mut short = 0;
    for dx in -10..=10 {
        for dz in -10..=10 {
            let here = read(&region, at(dx, -60, dz));
            if here.props().is_air() {
                continue;
            }
            // The walks take at most seven steps of one.
            assert!(dx.abs() <= 7 && dz.abs() <= 7, "{here:?} at {dx}, {dz}");
            short += usize::from(is_block(here, blocks::SHORT_GRASS));
        }
    }
    assert!(short > 20, "{short}");
    // With air above it is a target again; under a block it is not.
    put(&mut region, at(0, -60, 0), blocks::STONE);
    assert!(!bone(&mut region, grass, 4).0);
}

/// The game's walks of grass's bone meal on a flat field of grass under open air, drawn
/// from `random`: the blocks above the field each walk's end leaves (short grass, or the
/// lower half of tall grass), in a biome with no bone meal features.
fn walks_on_a_field(
    start: BlockPos,
    random: &mut Random,
) -> std::collections::BTreeMap<BlockPos, &'static str> {
    let mut grown = std::collections::BTreeMap::new();
    'walks: for walk in 0..128 {
        let mut at = start;
        for _ in 0..walk / 16 {
            let dx = random.next_int_between(-1, 1);
            let dy = random.next_int_between(-1, 1) * random.next_int(3) as i32 / 2;
            let dz = random.next_int_between(-1, 1);
            at = at.offset(dx, dy, dz);
            // Above the field only y = −60 stands on grass and is not a full block.
            if at.y != start.y {
                continue 'walks;
            }
        }
        match grown.get(&at).copied() {
            Some("short") => {
                if random.next_float() < 0.1 {
                    grown.insert(at, "tall");
                }
            }
            Some(_) => {}
            None => {
                // One in eight the biome's list, which is empty here and draws no more.
                if random.next_float() >= 0.125 {
                    let _ = random.next_u64();
                    grown.insert(at, "short");
                }
            }
        }
    }
    grown
}

#[test]
fn grass_s_bone_meal_draws_the_game_s_walks() {
    let grass = at(0, -61, 0);
    for seed in 0..20 {
        let mut region = scene(&grass_field(grass));
        region.config.features = features();
        let biome = region.biome_at(at(0, -60, 0)).expect("a biome");
        let settings = region.config.features.0.clone().expect("features");
        assert!(settings.bone_meal_features(biome).is_empty(), "{biome}");
        let (used, random, _) = bone(&mut region, grass, seed);
        assert!(used);
        let mut expected = stream(seed);
        let grown = walks_on_a_field(at(0, -60, 0), &mut expected);
        assert_eq!(random, expected, "seed {seed}");
        for dx in -10..=10 {
            for dz in -10..=10 {
                let pos = at(dx, -60, dz);
                let here = read(&region, pos);
                match grown.get(&pos).copied() {
                    Some("short") => assert_eq!(here, blocks::SHORT_GRASS, "{pos:?}"),
                    Some(_) => assert_eq!(here, state("minecraft:tall_grass[half=lower]")),
                    None => assert_eq!(here, blocks::AIR, "{pos:?}"),
                }
            }
        }
    }
}

#[test]
fn grass_in_the_plains_also_grows_the_plains_flowers() {
    let settings = features();
    let plains = settings.0.as_ref().expect("features");
    assert_eq!(
        plains.bone_meal_features("minecraft:plains"),
        ["minecraft:flower_plain"]
    );
    assert_eq!(
        plains.bone_meal_features("minecraft:birch_forest"),
        ["minecraft:wildflower", "minecraft:flower_default"]
    );
    assert!(plains.bone_meal_features("minecraft:badlands").is_empty());
    // A region whose chunks are plains.
    let biome = clustine_data::synced_registry("minecraft:worldgen/biome")
        .and_then(|biomes| {
            biomes
                .entries
                .iter()
                .position(|name| *name == "minecraft:plains")
        })
        .expect("the plains");
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("the overworld");
    let grass = at(0, -61, 0);
    let field = grass_field(grass);
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    checked(&mut region, &tickets(around.clone(), vec![]));
    let chunks = around
        .into_iter()
        .map(|position| {
            let mut chunk = clustine_world::Chunk::empty(overworld, Biome(biome as u16));
            for (pos, state) in field.iter().filter(|(pos, _)| pos.chunk() == position) {
                let (x, z) = pos.in_chunk();
                chunk.set(x, pos.y, z, *state);
            }
            (position, chunk)
        })
        .collect();
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        },
    );
    region.config.features = features();
    assert_eq!(region.biome_at(at(0, -60, 0)), Some("minecraft:plains"));
    let mut flowers = 0;
    for seed in 0..4 {
        // What the last use grew right above it is cleared, so that it takes more.
        put(&mut region, at(0, -60, 0), blocks::AIR);
        put(&mut region, at(0, -59, 0), blocks::AIR);
        assert!(bone(&mut region, grass, seed).0);
    }
    for dx in -10..=10 {
        for dz in -10..=10 {
            flowers += usize::from(in_tag(read(&region, at(dx, -60, dz)), "minecraft:flowers"));
        }
    }
    // One walk's end in eight, of those that end in air: some dozens over four uses.
    assert!(flowers > 0, "{flowers}");
}

#[test]
fn grass_beside_a_pinned_line_sends_what_it_grows_beyond_as_conditional_sets() {
    let grass = at(15, -61, 0);
    let mut region = crate::region::tnt::tests::west_of_a_line(5, |pos| {
        (pos.y == -61 && (5..=25).contains(&pos.x) && (-10..=10).contains(&pos.z))
            .then_some(blocks::GRASS_BLOCK)
    });
    region.config.features = features();
    assert!(region.ticks_chunk(grass.chunk()));
    let mut sent = Vec::new();
    for seed in 0..4 {
        region.made.clear();
        put(&mut region, at(15, -60, 0), blocks::AIR);
        put(&mut region, at(15, -59, 0), blocks::AIR);
        assert!(bone(&mut region, grass, seed).0);
        sent.extend(region.made.iter().filter_map(|effect| match effect.what {
            Effected::SetIf {
                expected, state, ..
            } => Some((effect.at, expected, state)),
            _ => None,
        }));
    }
    assert!(!sent.is_empty());
    for (pos, expected, state) in &sent {
        assert!(pos.x >= 16, "{pos:?} is beyond the line");
        assert_eq!(*expected, blocks::AIR);
        assert!(
            is_block(*state, blocks::SHORT_GRASS) || is_block(*state, blocks::TALL_GRASS),
            "{state:?}"
        );
        // The copy itself is the holder's to change.
        assert_eq!(read(&region, *pos), blocks::AIR);
    }
}

#[test]
fn a_mushroom_becomes_a_huge_mushroom_at_its_chance_and_is_put_back_where_it_fails() {
    let mushroom = at(0, -60, 0);
    let placed = [
        (at(0, -61, 0), blocks::MYCELIUM),
        (mushroom, blocks::BROWN_MUSHROOM),
    ];
    // Without the world's features the mushroom is no target.
    let mut bare = scene(&placed);
    assert!(!bone(&mut bare, mushroom, 0).0);
    let mut grown = 0;
    let mut failed = 0;
    for seed in 0..40 {
        let mut region = scene(&placed);
        region.config.features = features();
        let (used, random, _) = bone(&mut region, mushroom, seed);
        assert!(used);
        let mut expected = stream(seed);
        if f64::from(expected.next_float()) < 0.4 {
            // The feature's random is seeded from the stream's next long.
            let _ = expected.next_u64();
            assert!(is_block(read(&region, mushroom), blocks::MUSHROOM_STEM));
            let cap = (1..=8).any(|up| {
                (-3..=3).any(|dx| {
                    (-3..=3).any(|dz| {
                        is_block(
                            read(&region, mushroom.offset(dx, up, dz)),
                            blocks::BROWN_MUSHROOM_BLOCK,
                        )
                    })
                })
            });
            assert!(cap, "seed {seed}");
            grown += 1;
        } else {
            assert_eq!(read(&region, mushroom), blocks::BROWN_MUSHROOM);
            failed += 1;
        }
        assert_eq!(random, expected, "seed {seed}");
    }
    assert!(grown > 5 && failed > 5, "{grown}, {failed}");
    // A block two above: the feature fails, and the mushroom is put back.
    let mut put_back = 0;
    for seed in 0..40 {
        let mut region = scene(&[placed[0], placed[1], (at(0, -58, 0), blocks::STONE)]);
        region.config.features = features();
        assert!(bone(&mut region, mushroom, seed).0);
        assert_eq!(read(&region, mushroom), blocks::BROWN_MUSHROOM);
        assert_eq!(read(&region, at(0, -58, 0)), blocks::STONE);
        let mut expected = stream(seed);
        put_back += usize::from(f64::from(expected.next_float()) < 0.4);
    }
    assert!(put_back > 5, "{put_back}");
}

/// Part D4a's `advance_tree` is a stub beside this: these check bone meal's part (the
/// target, the draw of 0.45, the bone meal used), with the grower's `canGrow` drawing
/// the oak's plain list (weights 9 and 1) before the chance, its mega list being empty.
#[test]
fn a_sapling_is_a_target_and_succeeds_at_0_45() {
    let sapling = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::DIRT),
        (sapling, blocks::OAK_SAPLING),
    ]);
    let mut successes = 0;
    for seed in 0..1000 {
        put(&mut region, sapling, blocks::OAK_SAPLING);
        let (used, random, _) = bone(&mut region, sapling, seed);
        assert!(used);
        let mut expected = stream(seed);
        let _list = expected.next_int(10);
        let success = f64::from(expected.next_float()) < 0.45;
        successes += usize::from(success);
        // Stage 0 takes stage 1 at a success, which draws nothing more.
        assert_eq!(random, expected);
        let stage = if success { "1" } else { "0" };
        assert_eq!(read(&region, sapling).property("stage"), Some(stage));
    }
    assert!((390..510).contains(&successes), "{successes}");
}

/// The plan of N8, 7.5: a sapling given bone meal takes its stage at a first success and
/// grows its tree at the next, not at once.
#[test]
fn a_sapling_given_bone_meal_takes_its_stage_and_then_its_tree() {
    let sapling = at(8, -60, 8);
    let mut region = scene(&[
        (at(8, -61, 8), blocks::DIRT),
        (sapling, blocks::OAK_SAPLING),
    ]);
    region.config.features = features();
    let mut successes = 0;
    for seed in 0..200 {
        let before = read(&region, sapling);
        bone(&mut region, sapling, seed);
        let after = read(&region, sapling);
        if after == before {
            continue;
        }
        successes += 1;
        if successes == 1 {
            assert_eq!(
                after.property("stage"),
                Some("1"),
                "the first success's stage"
            );
        } else {
            assert!(
                is_block(after, blocks::OAK_LOG),
                "the tree's trunk: {after:?}"
            );
            return;
        }
    }
    panic!("the sapling grew no tree in 200 uses");
}

#[test]
fn a_propagule_on_the_ground_is_a_sapling_and_a_hanging_one_ages_until_grown() {
    let propagule = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::MUD),
        (
            propagule,
            state("minecraft:mangrove_propagule[hanging=false,stage=0]"),
        ),
    ]);
    let (used, random, _) = bone(&mut region, propagule, 7);
    assert!(used);
    assert_eq!(random.draws_since(&stream(7)), 1);
    // A hanging one: its age by one, drawing nothing, until 4.
    let hanging = at(3, -58, 3);
    let mut region = scene(&[
        (at(3, -57, 3), blocks::MANGROVE_LEAVES),
        (
            hanging,
            state("minecraft:mangrove_propagule[hanging=true,age=0]"),
        ),
    ]);
    for age in 1..=4_u8 {
        let (used, random, _) = bone(&mut region, hanging, u64::from(age));
        assert!(used);
        assert_eq!(random, stream(u64::from(age)));
        assert_eq!(
            read(&region, hanging).property("age"),
            Some(age.to_string().as_str())
        );
    }
    assert!(!bone(&mut region, hanging, 9).0);
}

#[test]
fn an_azalea_is_a_target_only_with_no_fluid_above_and_succeeds_at_0_45() {
    let azalea = at(0, -60, 0);
    let mut region = scene(&[(at(0, -61, 0), blocks::DIRT), (azalea, blocks::AZALEA)]);
    let (used, random, _) = bone(&mut region, azalea, 2);
    assert!(used);
    assert_eq!(random.draws_since(&stream(2)), 1);
    put(&mut region, at(0, -59, 0), blocks::WATER);
    let (used, random, _) = bone(&mut region, azalea, 3);
    assert!(!used);
    assert_eq!(random, stream(3));
}

#[test]
fn short_grass_grows_tall_with_air_above() {
    let grass = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::GRASS_BLOCK),
        (grass, blocks::SHORT_GRASS),
    ]);
    let (used, random, _) = bone(&mut region, grass, 1);
    assert!(used);
    assert_eq!(random, stream(1));
    assert_eq!(
        read(&region, grass),
        state("minecraft:tall_grass[half=lower]")
    );
    assert_eq!(
        read(&region, at(0, -59, 0)),
        state("minecraft:tall_grass[half=upper]")
    );
}

#[test]
fn a_player_s_bone_meal_sparks_where_it_grows_keeps_the_creative_stack_and_does_nothing_on_a_grown_crop()
 {
    let wheat = at(1, -60, 1);
    let mut region = scene(&[(at(1, -61, 1), blocks::FARMLAND), (wheat, blocks::WHEAT)]);
    checked(&mut region, &super::redstone::holding(items::BONE_MEAL));
    let before = region.state().players[&player(1)].inventory.clone();
    let output = checked(&mut region, &super::redstone::click(wheat));
    assert!(
        super::r7::level_events(&output).contains(&(1505, wheat, 15)),
        "{:?}",
        super::r7::level_events(&output)
    );
    let age = read(&region, wheat).property("age").map(str::to_owned);
    assert!(
        matches!(age.as_deref(), Some("2" | "3" | "4" | "5")),
        "{age:?}"
    );
    assert_eq!(region.state().players[&player(1)].inventory, before);
    // A grown crop takes none: no sparks, nothing changed.
    put(&mut region, wheat, aged("wheat", 7));
    let output = checked(&mut region, &super::redstone::click(wheat));
    assert!(
        !super::r7::level_events(&output)
            .iter()
            .any(|(kind, ..)| *kind == 1505)
    );
    assert_eq!(read(&region, wheat), aged("wheat", 7));
}

#[test]
fn nothing_grows_in_a_chunk_the_region_does_not_tick() {
    // The chunk at x = 2 is loaded and does not tick.
    let wheat = at(40, -60, 0);
    let mut region = scene(&[(at(40, -61, 0), blocks::FARMLAND), (wheat, blocks::WHEAT)]);
    assert!(!region.ticks_chunk(wheat.chunk()));
    let (used, random, _) = bone(&mut region, wheat, 1);
    assert!(!used);
    assert_eq!(random, stream(1));
    assert_eq!(read(&region, wheat), blocks::WHEAT);
}

#[test]
fn a_block_that_takes_no_bone_meal_draws_nothing() {
    let mut region = scene(&[(at(0, -60, 0), blocks::STONE)]);
    for pos in [at(0, -60, 0), at(0, -59, 0)] {
        let (used, random, output) = bone(&mut region, pos, 1);
        assert!(!used);
        assert_eq!(random, stream(1));
        assert!(output.events.is_empty());
    }
}
