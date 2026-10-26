//! What grows by light (part D2 of N8, `docs/groundwork/n8-plan.md`, sections 6.2 and
//! 6.3), from the class dumps: crops, stems, berries, beetroot, the torchflower, grass and
//! mycelium, bamboo and its shoot, mushrooms, seeds planted only in light, and a grass
//! block's spread across a pinned line.
//!
//! Most tests drive one rule at a time, on the 3×3 of stone floors about the origin (the
//! floor at y = −61), lit by the region's own light step, with a stream of their own for
//! each seed. Where a rule draws, what it drew is checked against a stream that draws what
//! the game's rule draws, in its order; where it decides by chance, many streams are
//! tried and a count is held within wide margins of the game's chance. A dark room is the
//! 3×3 under a stone roof at y = −57: the chunks beyond are absent, which the light
//! takes as walls, so no sky reaches under it.

use clustine_data::Face;
use clustine_world::Effected;

use super::*;
use crate::region::Random;

const NOON: u64 = 6_000;
const MIDNIGHT: u64 = 18_000;

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// A region with the 3×3 of floors about the origin, `placed` put there without any
/// update, lit, at noon.
fn scene(placed: &[(BlockPos, BlockState)]) -> Region {
    let mut region = region();
    for x in -1..=1 {
        for z in -1..=1 {
            region.chunks.insert(ChunkPos::new(x, z), floor());
        }
    }
    for (pos, placed) in placed {
        put(&mut region, *pos, *placed);
    }
    region.clock.day = NOON;
    region.light_step();
    region
}

/// [`scene`] under a stone roof at y = −57 over the whole 3×3.
fn dark_scene(placed: &[(BlockPos, BlockState)]) -> Region {
    let mut all: Vec<(BlockPos, BlockState)> = (-16..32)
        .flat_map(|x| (-16..32).map(move |z| (at(x, -57, z), blocks::STONE)))
        .collect();
    all.extend_from_slice(placed);
    scene(&all)
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

/// The random tick of what stands at `pos` by the growth's rules, drawn from the stream
/// of `seed`; the stream as the rule left it.
fn grow(region: &mut Region, pos: BlockPos, seed: u64) -> Random {
    let state = read(region, pos);
    let mut random = stream(seed);
    let mut output = TickOutput::default();
    assert!(
        region.growth_random_tick(pos, state, &mut random, &mut output),
        "{state:?} grows by light"
    );
    random
}

/// The stream of `seed` after `draws`, each `nextInt` of its bound.
fn drawn(seed: u64, draws: &[u32]) -> Random {
    let mut random = stream(seed);
    for bound in draws {
        random.next_int(*bound);
    }
    random
}

/// The first `nextInt(bound)` of the stream of `seed`.
fn first(seed: u64, bound: u32) -> u32 {
    stream(seed).next_int(bound)
}

fn torch() -> BlockState {
    blocks::TORCH
}

/// Wheat of age 0 at the origin on dry farmland, with a torch `away` blocks east of it,
/// in the dark room.
fn wheat_by_a_torch(away: i32) -> (Region, BlockPos) {
    let wheat = at(0, -60, 0);
    let region = dark_scene(&[
        (at(0, -61, 0), blocks::FARMLAND),
        (wheat, blocks::WHEAT),
        (at(away, -60, 0), torch()),
    ]);
    (region, wheat)
}

#[test]
fn wheat_grows_by_its_chance_in_light_9_and_draws_nothing_in_light_8() {
    let (mut lit, wheat) = wheat_by_a_torch(5);
    assert_eq!(lit.raw_brightness(wheat, 0), Some(9));
    // One for the dry farmland below, nothing for the stone about it: 25 / 2 + 1 = 13.
    assert_eq!(lit.growth_speed(blocks::WHEAT, wheat), 2.0);
    let mut grown = 0;
    for seed in 0..2600 {
        put(&mut lit, wheat, blocks::WHEAT);
        let random = grow(&mut lit, wheat, seed);
        assert_eq!(random, drawn(seed, &[13]), "one draw of 13, seed {seed}");
        let now = read(&lit, wheat);
        if first(seed, 13) == 0 {
            assert_eq!(now, state("minecraft:wheat[age=1]"));
            grown += 1;
        } else {
            assert_eq!(now, blocks::WHEAT);
        }
    }
    assert!((140..260).contains(&grown), "{grown} of 2600, one in 13");

    let (mut dark, wheat) = wheat_by_a_torch(6);
    assert_eq!(dark.raw_brightness(wheat, 0), Some(8));
    for seed in 0..200 {
        let random = grow(&mut dark, wheat, seed);
        assert_eq!(random, stream(seed), "the light is read first");
        assert_eq!(read(&dark, wheat), blocks::WHEAT);
    }
}

#[test]
fn a_crop_at_its_oldest_draws_nothing() {
    let wheat = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::FARMLAND),
        (wheat, state("minecraft:wheat[age=7]")),
    ]);
    for seed in 0..50 {
        assert_eq!(grow(&mut region, wheat, seed), stream(seed));
    }
}

#[test]
fn moist_farmland_and_a_row_of_the_same_crop_change_the_chance_as_the_game_s() {
    let wheat = at(0, -60, 0);
    let mut moist: Vec<(BlockPos, BlockState)> = Vec::new();
    for x in -1..=1 {
        for z in -1..=1 {
            moist.push((at(x, -61, z), state("minecraft:farmland[moisture=7]")));
        }
    }
    let mut region = scene(&moist);
    // 1 + 3 + 8 × 3 / 4 = 10.
    assert_eq!(region.growth_speed(blocks::WHEAT, wheat), 10.0);
    // Wheat on both sides along x and along z halves it.
    put(&mut region, at(1, -60, 0), blocks::WHEAT);
    put(&mut region, at(0, -60, 1), blocks::WHEAT);
    assert_eq!(region.growth_speed(blocks::WHEAT, wheat), 5.0);
    // Along one way only, it is as it was; a diagonal alone halves it.
    put(&mut region, at(0, -60, 1), blocks::AIR);
    assert_eq!(region.growth_speed(blocks::WHEAT, wheat), 10.0);
    put(&mut region, at(1, -60, 0), blocks::AIR);
    put(&mut region, at(1, -60, 1), blocks::WHEAT);
    assert_eq!(region.growth_speed(blocks::WHEAT, wheat), 5.0);
    // Carrots beside are no wheat.
    put(&mut region, at(1, -60, 1), blocks::CARROTS);
    assert_eq!(region.growth_speed(blocks::WHEAT, wheat), 10.0);
}

#[test]
fn beetroot_skips_one_time_in_three_before_it_reads_the_light() {
    let beetroot = at(0, -60, 0);
    let placed = [
        (at(0, -61, 0), blocks::FARMLAND),
        (beetroot, blocks::BEETROOTS),
    ];
    let mut dark = dark_scene(&placed);
    for seed in 0..100 {
        assert_eq!(grow(&mut dark, beetroot, seed), drawn(seed, &[3]));
        assert_eq!(read(&dark, beetroot), blocks::BEETROOTS);
    }
    let mut lit = scene(&placed);
    let mut grown = 0;
    for seed in 0..3000 {
        put(&mut lit, beetroot, blocks::BEETROOTS);
        let random = grow(&mut lit, beetroot, seed);
        let mut expected = stream(seed);
        let skips = expected.next_int(3) == 0;
        let grows = !skips && expected.next_int(13) == 0;
        assert_eq!(random, expected, "seed {seed}");
        if grows {
            assert_eq!(read(&lit, beetroot), state("minecraft:beetroots[age=1]"));
            grown += 1;
        } else {
            assert_eq!(read(&lit, beetroot), blocks::BEETROOTS);
        }
    }
    // Two in three times one in 13: about 154.
    assert!((100..210).contains(&grown), "{grown}");
}

#[test]
fn a_torchflower_crop_grows_to_its_last_age_and_then_flowers_by_random_ticks() {
    let crop = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::FARMLAND),
        (crop, blocks::TORCHFLOWER_CROP),
    ]);
    // Its own ages are 0 and 1, and its oldest (`getMaxAge`) is 2: the age past 1 is
    // the torchflower (n7 TorchflowerCropBlock 45–66), by random tick as by bone meal.
    for (from, to) in [
        (
            blocks::TORCHFLOWER_CROP,
            state("minecraft:torchflower_crop[age=1]"),
        ),
        (
            state("minecraft:torchflower_crop[age=1]"),
            blocks::TORCHFLOWER,
        ),
    ] {
        let mut grew = false;
        for seed in 0..200 {
            put(&mut region, crop, from);
            let random = grow(&mut region, crop, seed);
            let mut expected = stream(seed);
            if expected.next_int(3) != 0 {
                grew |= expected.next_int(13) == 0;
            }
            assert_eq!(random, expected);
            if grew {
                assert_eq!(read(&region, crop), to);
                break;
            }
            assert_eq!(read(&region, crop), from);
        }
        assert!(grew);
    }
}

#[test]
fn the_pitcher_crop_draws_its_chance_before_it_reads_the_light_and_grows_two_tall_from_age_3() {
    let pod = at(0, -60, 0);
    let placed = [
        (at(0, -61, 0), blocks::FARMLAND),
        (pod, state("minecraft:pitcher_crop[age=2,half=lower]")),
    ];
    let mut dark = dark_scene(&placed);
    for seed in 0..100 {
        assert_eq!(grow(&mut dark, pod, seed), drawn(seed, &[13]));
        assert_eq!(read(&dark, pod), placed[1].1);
    }
    let mut lit = scene(&placed);
    let seed = (0..)
        .find(|seed| first(*seed, 13) == 0)
        .expect("a stream that grows");
    grow(&mut lit, pod, seed);
    assert_eq!(
        read(&lit, pod),
        state("minecraft:pitcher_crop[age=3,half=lower]")
    );
    assert_eq!(
        read(&lit, at(0, -59, 0)),
        state("minecraft:pitcher_crop[age=3,half=upper]")
    );
}

/// A melon stem of age 7 at the origin on farmland, with dirt under its four sides.
fn melon_patch() -> (Region, BlockPos) {
    let stem = at(0, -60, 0);
    let mut placed = vec![
        (at(0, -61, 0), blocks::FARMLAND),
        (stem, state("minecraft:melon_stem[age=7]")),
    ];
    for (x, z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        placed.push((at(x, -61, z), blocks::DIRT));
    }
    (scene(&placed), stem)
}

/// Random ticks of the stem at `stem` from the seeds of `seeds` until it is attached; the
/// side its fruit is on.
fn fruit_of(region: &mut Region, stem: BlockPos, seeds: std::ops::Range<u64>) -> Face {
    for seed in seeds {
        grow(region, stem, seed);
        region.light_step();
        let now = read(region, stem);
        if clustine_blocks::data::is_block(now, blocks::ATTACHED_MELON_STEM) {
            return clustine_blocks::data::facing(now).expect("an attached stem faces");
        }
        assert_eq!(now, state("minecraft:melon_stem[age=7]"));
    }
    panic!("no fruit");
}

#[test]
fn a_melon_stem_fruits_turns_back_when_its_fruit_is_harvested_and_fruits_again() {
    let (mut region, stem) = melon_patch();
    // The draw of a side follows the draw of the chance, the game's order.
    let seed = (0..)
        .find(|seed| first(*seed, 13) == 0)
        .expect("a stream that fruits");
    let random = grow(&mut region, stem, seed);
    assert_eq!(random, drawn(seed, &[13, 4]));
    let side =
        [Face::North, Face::East, Face::South, Face::West][drawn(seed, &[13]).next_int(4) as usize];
    let fruit = clustine_blocks::data::relative(stem, side);
    assert_eq!(read(&region, fruit), blocks::MELON);
    assert_eq!(
        read(&region, stem),
        state(&format!(
            "minecraft:attached_melon_stem[facing={}]",
            clustine_blocks::data::face_name(side)
        ))
    );
    region.light_step();

    // Harvested: the stem's shape update from the side it faces turns it back.
    let mut output = TickOutput::default();
    region.set_block(fruit, blocks::AIR, &mut output);
    assert_eq!(read(&region, stem), state("minecraft:melon_stem[age=7]"));
    region.light_step();

    // And it fruits again.
    fruit_of(&mut region, stem, 0..400);

    // A change on another side of an attached stem leaves it attached.
    let side = clustine_blocks::data::facing(read(&region, stem)).expect("attached");
    let other = clustine_blocks::data::relative(stem, side.opposite());
    region.set_block(other, blocks::STONE, &mut output);
    assert!(clustine_blocks::data::is_block(
        read(&region, stem),
        blocks::ATTACHED_MELON_STEM
    ));
}

#[test]
fn a_stem_below_age_7_ages_and_one_in_the_dark_draws_nothing() {
    let stem = at(0, -60, 0);
    let placed = [
        (at(0, -61, 0), blocks::FARMLAND),
        (stem, blocks::PUMPKIN_STEM),
    ];
    let mut dark = dark_scene(&placed);
    for seed in 0..50 {
        assert_eq!(grow(&mut dark, stem, seed), stream(seed));
    }
    let mut lit = scene(&placed);
    let seed = (0..).find(|seed| first(*seed, 13) == 0).expect("a stream");
    assert_eq!(grow(&mut lit, stem, seed), drawn(seed, &[13]));
    assert_eq!(read(&lit, stem), state("minecraft:pumpkin_stem[age=1]"));
}

#[test]
fn sweet_berries_draw_their_chance_first_and_age_only_in_light_above_them() {
    let bush = at(0, -60, 0);
    let placed = [
        (at(0, -61, 0), blocks::GRASS_BLOCK),
        (bush, blocks::SWEET_BERRY_BUSH),
    ];
    let mut dark = dark_scene(&placed);
    for seed in 0..100 {
        assert_eq!(grow(&mut dark, bush, seed), drawn(seed, &[5]));
        assert_eq!(read(&dark, bush), blocks::SWEET_BERRY_BUSH);
    }
    let mut lit = scene(&placed);
    let mut grown = 0;
    for seed in 0..1000 {
        put(&mut lit, bush, blocks::SWEET_BERRY_BUSH);
        assert_eq!(grow(&mut lit, bush, seed), drawn(seed, &[5]));
        if first(seed, 5) == 0 {
            assert_eq!(read(&lit, bush), state("minecraft:sweet_berry_bush[age=1]"));
            grown += 1;
        }
    }
    assert!((150..250).contains(&grown), "{grown}");
    // Grown out, it draws nothing.
    put(&mut lit, bush, state("minecraft:sweet_berry_bush[age=3]"));
    assert_eq!(grow(&mut lit, bush, 1), stream(1));
}

/// A grass block at `GRASS`, high above the floor so that its targets can be set apart.
const GRASS: BlockPos = BlockPos::new(0, -50, 0);

#[test]
fn grass_spreads_by_day_to_dirt_one_across_three_down_and_one_up_and_no_further() {
    let reached = [at(1, -50, 0), at(-1, -49, 1), at(1, -53, -1), at(0, -53, 1)];
    let beyond = [at(2, -50, 0), at(0, -48, 1), at(-1, -54, 0), at(0, -50, -2)];
    let mut placed = vec![(GRASS, blocks::GRASS_BLOCK)];
    placed.extend(
        reached
            .iter()
            .chain(&beyond)
            .map(|pos| (*pos, blocks::DIRT)),
    );
    let mut region = scene(&placed);
    for seed in 0..600 {
        let random = grow(&mut region, GRASS, seed);
        assert_eq!(random, drawn(seed, &[3, 5, 3, 3, 5, 3, 3, 5, 3, 3, 5, 3]));
    }
    for pos in reached {
        assert_eq!(
            read(&region, pos),
            state("minecraft:grass_block[snowy=false]"),
            "{pos:?}"
        );
    }
    for pos in beyond {
        assert_eq!(read(&region, pos), blocks::DIRT, "{pos:?}");
    }
}

#[test]
fn grass_spreads_at_night_only_by_a_torch_and_draws_nothing_in_the_dark() {
    let target = at(1, -50, 0);
    let placed = [(GRASS, blocks::GRASS_BLOCK), (target, blocks::DIRT)];
    let mut region = scene(&placed);
    region.clock.day = MIDNIGHT;
    let above = at(0, -49, 0);
    assert_eq!(region.raw_brightness(above, 0), Some(15));
    assert!(region.max_local_raw_brightness(above) < Some(9));
    for seed in 0..300 {
        assert_eq!(grow(&mut region, GRASS, seed), stream(seed));
    }
    assert_eq!(read(&region, target), blocks::DIRT);

    // A torch five blocks from the block above it lights it to 9, night or day.
    put(&mut region, at(0, -49, 5), torch());
    region.light_step();
    assert_eq!(region.max_local_raw_brightness(above), Some(9));
    for seed in 0..300 {
        grow(&mut region, GRASS, seed);
    }
    assert_eq!(
        read(&region, target),
        state("minecraft:grass_block[snowy=false]")
    );
}

#[test]
fn grass_dies_under_a_full_block_a_bottom_slab_or_a_full_fluid_and_lives_under_a_top_slab_or_snow()
{
    let above = at(0, -49, 0);
    let dies = [
        blocks::STONE,
        state("minecraft:oak_slab[type=bottom,waterlogged=false]"),
        blocks::WATER,
        blocks::LAVA,
    ];
    for over in dies {
        let mut region = scene(&[(GRASS, blocks::GRASS_BLOCK), (above, over)]);
        assert_eq!(grow(&mut region, GRASS, 7), stream(7), "{over:?}: no draw");
        assert_eq!(read(&region, GRASS), blocks::DIRT, "under {over:?}");
    }
    let lives = [
        state("minecraft:oak_slab[type=top,waterlogged=false]"),
        state("minecraft:snow[layers=1]"),
        blocks::GLASS,
    ];
    for over in lives {
        let mut region = scene(&[(GRASS, blocks::GRASS_BLOCK), (above, over)]);
        grow(&mut region, GRASS, 7);
        assert_eq!(read(&region, GRASS), blocks::GRASS_BLOCK, "under {over:?}");
    }
}

#[test]
fn grass_spreads_snowy_under_snow_and_not_to_dirt_under_water_or_a_full_block() {
    let snowy = at(1, -50, 0);
    let wet = at(-1, -50, 0);
    let covered = at(0, -50, 1);
    let mut region = scene(&[
        (GRASS, blocks::GRASS_BLOCK),
        (snowy, blocks::DIRT),
        (at(1, -49, 0), state("minecraft:snow[layers=1]")),
        (wet, blocks::DIRT),
        (at(-1, -49, 0), state("minecraft:water[level=3]")),
        (covered, blocks::DIRT),
        (at(0, -49, 1), blocks::STONE),
    ]);
    for seed in 0..600 {
        grow(&mut region, GRASS, seed);
    }
    assert_eq!(
        read(&region, snowy),
        state("minecraft:grass_block[snowy=true]")
    );
    assert_eq!(read(&region, wet), blocks::DIRT);
    assert_eq!(read(&region, covered), blocks::DIRT);
}

#[test]
fn mycelium_spreads_to_dirt_as_grass_does() {
    let target = at(0, -50, -1);
    let mut region = scene(&[(GRASS, blocks::MYCELIUM), (target, blocks::DIRT)]);
    for seed in 0..300 {
        grow(&mut region, GRASS, seed);
    }
    assert_eq!(
        read(&region, target),
        state("minecraft:mycelium[snowy=false]")
    );
    // Under a block it dies to dirt too.
    put(&mut region, at(0, -49, 0), blocks::STONE);
    grow(&mut region, GRASS, 1);
    assert_eq!(read(&region, GRASS), blocks::DIRT);
}

fn bamboo(age: u8, leaves: &str, stage: u8) -> BlockState {
    state(&format!(
        "minecraft:bamboo[age={age},leaves={leaves},stage={stage}]"
    ))
}

/// A column of `height` stalks of bamboo of stage 0 on dirt at the origin; the top's
/// position.
fn bamboo_column(height: i32) -> (Region, BlockPos) {
    let mut placed = vec![(at(0, -61, 0), blocks::DIRT)];
    for y in 0..height {
        placed.push((at(0, -60 + y, 0), bamboo(0, "none", 0)));
    }
    (scene(&placed), at(0, -61 + height, 0))
}

#[test]
fn bamboo_grows_one_try_in_three_with_small_leaves_over_a_stalk_without() {
    let (mut region, top) = bamboo_column(1);
    let seed = (0..).find(|seed| first(*seed, 3) == 0).expect("a stream");
    assert_eq!(grow(&mut region, top, seed), drawn(seed, &[3]));
    assert_eq!(read(&region, at(0, -59, 0)), bamboo(0, "small", 0));
    let (mut region, top) = bamboo_column(1);
    let seed = (0..).find(|seed| first(*seed, 3) != 0).expect("a stream");
    assert_eq!(grow(&mut region, top, seed), drawn(seed, &[3]));
    assert_eq!(read(&region, at(0, -59, 0)), blocks::AIR);
}

#[test]
fn bamboo_over_a_leafy_stalk_takes_large_leaves_and_thins_the_two_below_it() {
    let mut placed = vec![(at(0, -61, 0), blocks::DIRT)];
    placed.push((at(0, -60, 0), bamboo(0, "small", 0)));
    placed.push((at(0, -59, 0), bamboo(0, "large", 0)));
    placed.push((at(0, -58, 0), bamboo(0, "large", 0)));
    let mut region = scene(&placed);
    let top = at(0, -58, 0);
    let seed = (0..).find(|seed| first(*seed, 3) == 0).expect("a stream");
    grow(&mut region, top, seed);
    // As the game's: the stalk below the growing one takes small leaves and the one
    // below that none, the growing one keeping its own; two stalks below make the new
    // one of age 1, which the stalks below then take from it by their shape updates.
    let leaves = |region: &Region, y: i32| {
        read(region, at(0, y, 0))
            .property("leaves")
            .map(str::to_owned)
    };
    assert_eq!(read(&region, at(0, -57, 0)), bamboo(1, "large", 0));
    assert_eq!(leaves(&region, -58).as_deref(), Some("large"));
    assert_eq!(leaves(&region, -59).as_deref(), Some("small"));
    assert_eq!(leaves(&region, -60).as_deref(), Some("none"));
}

#[test]
fn bamboo_draws_its_float_only_from_height_11_and_stops_at_16() {
    // The top of a column of ten is at height 10: no float.
    let seed = (0..).find(|seed| first(*seed, 3) == 0).expect("a stream");
    let (mut region, top) = bamboo_column(10);
    assert_eq!(grow(&mut region, top, seed), drawn(seed, &[3]));
    assert_eq!(
        number_of(read(&region, top.offset(0, 1, 0)), "stage"),
        Some(0)
    );
    // Of eleven: a float, stage 1 at a chance of one in four.
    let (mut region, top) = bamboo_column(11);
    let random = grow(&mut region, top, seed);
    let mut expected = drawn(seed, &[3]);
    let grown_out = expected.next_float() < 0.25;
    assert_eq!(random, expected);
    assert_eq!(
        number_of(read(&region, top.offset(0, 1, 0)), "stage"),
        Some(u8::from(grown_out))
    );
    // Of fifteen: the new one is the sixteenth, grown out whatever the float.
    let (mut region, top) = bamboo_column(15);
    let random = grow(&mut region, top, seed);
    let mut expected = drawn(seed, &[3]);
    expected.next_float();
    assert_eq!(random, expected);
    assert_eq!(
        number_of(read(&region, top.offset(0, 1, 0)), "stage"),
        Some(1)
    );
    // Of sixteen: it draws its one in three and grows no more.
    let (mut region, top) = bamboo_column(16);
    for seed in 0..50 {
        assert_eq!(grow(&mut region, top, seed), drawn(seed, &[3]));
    }
    assert_eq!(read(&region, top.offset(0, 1, 0)), blocks::AIR);
    // A stalk grown out draws nothing.
    let (mut region, top) = bamboo_column(1);
    put(&mut region, top, bamboo(0, "none", 1));
    assert_eq!(grow(&mut region, top, seed), stream(seed));
}

fn number_of(state: BlockState, name: &str) -> Option<u8> {
    clustine_blocks::data::number(state, name)
}

#[test]
fn bamboo_in_the_dark_draws_its_one_in_three_and_does_not_grow() {
    let mut placed = vec![
        (at(0, -61, 0), blocks::DIRT),
        (at(0, -60, 0), bamboo(0, "none", 0)),
    ];
    placed.push((at(0, -61, 1), blocks::DIRT));
    let mut region = dark_scene(&placed);
    for seed in 0..60 {
        assert_eq!(grow(&mut region, at(0, -60, 0), seed), drawn(seed, &[3]));
    }
    assert_eq!(read(&region, at(0, -59, 0)), blocks::AIR);
}

#[test]
fn a_bamboo_shoot_grows_bamboo_and_becomes_bamboo_under_it() {
    let shoot = at(0, -60, 0);
    let mut region = scene(&[
        (at(0, -61, 0), blocks::DIRT),
        (shoot, blocks::BAMBOO_SAPLING),
    ]);
    let seed = (0..).find(|seed| first(*seed, 3) == 0).expect("a stream");
    assert_eq!(grow(&mut region, shoot, seed), drawn(seed, &[3]));
    assert_eq!(read(&region, at(0, -59, 0)), bamboo(0, "small", 0));
    assert_eq!(read(&region, shoot), bamboo(0, "none", 0));
    // And it grows on as a stalk.
    region.light_step();
    let seed = (0..).find(|seed| first(*seed, 3) == 0).expect("a stream");
    grow(&mut region, at(0, -59, 0), seed);
    assert!(clustine_blocks::data::is_block(
        read(&region, at(0, -58, 0)),
        blocks::BAMBOO
    ));
}

#[test]
fn a_bamboo_shoot_under_a_block_or_in_the_dark_does_not_grow() {
    let shoot = at(0, -60, 0);
    let mut covered = scene(&[
        (at(0, -61, 0), blocks::DIRT),
        (shoot, blocks::BAMBOO_SAPLING),
        (at(0, -59, 0), blocks::STONE),
    ]);
    let mut dark = dark_scene(&[
        (at(0, -61, 0), blocks::DIRT),
        (shoot, blocks::BAMBOO_SAPLING),
    ]);
    for seed in 0..60 {
        assert_eq!(grow(&mut covered, shoot, seed), drawn(seed, &[3]));
        assert_eq!(grow(&mut dark, shoot, seed), drawn(seed, &[3]));
    }
    assert_eq!(read(&covered, shoot), blocks::BAMBOO_SAPLING);
    assert_eq!(read(&dark, shoot), blocks::BAMBOO_SAPLING);
    assert_eq!(read(&dark, at(0, -59, 0)), blocks::AIR);
}

/// The draws of a mushroom's random tick where it spreads: its chance, then five
/// targets of four draws each.
fn mushroom_draws() -> Vec<u32> {
    let mut draws = vec![25];
    for _ in 0..5 {
        draws.extend_from_slice(&[3, 2, 2, 3]);
    }
    draws
}

#[test]
fn mushrooms_spread_in_the_dark_by_a_walk_that_draws_every_target() {
    let mushroom = at(0, -60, 0);
    let mut region = dark_scene(&[(mushroom, blocks::BROWN_MUSHROOM)]);
    let spreading: Vec<u64> = (0..2000).filter(|seed| first(*seed, 25) == 0).collect();
    assert!(spreading.len() > 40);
    let mut crowded = false;
    for seed in &spreading {
        let about = (-4..=4)
            .flat_map(|x| (-4..=4).map(move |z| at(x, -60, z)))
            .filter(|pos| read(&region, *pos) == blocks::BROWN_MUSHROOM)
            .count();
        let random = grow(&mut region, mushroom, *seed);
        if about <= 4 {
            assert_eq!(random, drawn(*seed, &mushroom_draws()), "{about} about it");
        } else {
            // Where five stand about it, it draws only its chance.
            assert_eq!(random, drawn(*seed, &[25]), "{about} about it");
            crowded = true;
        }
    }
    assert!(crowded, "the room filled");
    let spread: Vec<BlockPos> = (-6..=6)
        .flat_map(|x| (-6..=6).map(move |z| at(x, -60, z)))
        .filter(|pos| *pos != mushroom && read(&region, *pos) == blocks::BROWN_MUSHROOM)
        .collect();
    assert!(!spread.is_empty());
    // A walk of five steps of one: within five blocks each way.
    assert!(
        spread
            .iter()
            .all(|pos| pos.x.abs() <= 5 && pos.z.abs() <= 5)
    );
}

#[test]
fn mushrooms_draw_only_their_chance_where_five_stand_about_them() {
    let mushroom = at(0, -60, 0);
    let mut placed = vec![(mushroom, blocks::RED_MUSHROOM)];
    for x in [-4, -2, 2, 4] {
        placed.push((at(x, -60, 4), blocks::RED_MUSHROOM));
    }
    let mut region = dark_scene(&placed);
    let seed = (0..).find(|seed| first(*seed, 25) == 0).expect("a stream");
    assert_eq!(grow(&mut region, mushroom, seed), drawn(seed, &[25]));
    // Four about it, itself counted, and it walks.
    put(&mut region, at(4, -60, 4), blocks::AIR);
    assert_eq!(
        grow(&mut region, mushroom, seed),
        drawn(seed, &mushroom_draws())
    );
}

#[test]
fn a_mushroom_stays_in_light_12_and_not_in_13_and_does_not_spread_under_the_sky() {
    // A torch two blocks away gives 12, one away 13.
    let near = at(1, -60, 0);
    let far = at(2, -60, 0);
    let mut region = dark_scene(&[(at(0, -60, 0), torch())]);
    assert_eq!(region.raw_brightness(near, 0), Some(13));
    assert_eq!(region.raw_brightness(far, 0), Some(12));
    let mut view = crate::region::updates::RegionView(&mut region);
    assert!(!clustine_blocks::can_survive(
        blocks::BROWN_MUSHROOM,
        near,
        &mut view
    ));
    assert!(clustine_blocks::can_survive(
        blocks::BROWN_MUSHROOM,
        far,
        &mut view
    ));
    // On mycelium, which overrides its need of the dark, it stays in any light.
    put(&mut region, at(1, -61, 0), blocks::MYCELIUM);
    let mut view = crate::region::updates::RegionView(&mut region);
    assert!(clustine_blocks::can_survive(
        blocks::BROWN_MUSHROOM,
        near,
        &mut view
    ));

    let mushroom = at(0, -60, 0);
    let mut open = scene(&[(mushroom, blocks::BROWN_MUSHROOM)]);
    for seed in (0..3000).filter(|seed| first(*seed, 25) == 0) {
        grow(&mut open, mushroom, seed);
    }
    let others = (-6..=6)
        .flat_map(|x| (-6..=6).map(move |z| at(x, -60, z)))
        .filter(|pos| *pos != mushroom && read(&open, *pos) == blocks::BROWN_MUSHROOM)
        .count();
    assert_eq!(others, 0);
}

#[test]
fn a_crop_breaks_at_a_neighbour_s_change_in_the_dark_and_not_at_a_change_of_light() {
    let wheat = at(0, -60, 0);
    let lamp = at(3, -60, 0);
    let mut region = dark_scene(&[
        (at(0, -61, 0), blocks::FARMLAND),
        (wheat, blocks::WHEAT),
        (lamp, torch()),
    ]);
    assert_eq!(region.raw_brightness(wheat, 0), Some(11));
    // The torch goes: dark, but nothing beside the wheat changed, and it stays.
    let mut output = TickOutput::default();
    region.set_block(lamp, blocks::AIR, &mut output);
    region.light_step();
    assert_eq!(region.raw_brightness(wheat, 0), Some(0));
    assert_eq!(read(&region, wheat), blocks::WHEAT);
    // A block set beside it asks its shape again: it breaks.
    region.set_block(at(1, -60, 0), blocks::STONE, &mut output);
    assert_eq!(read(&region, wheat), blocks::AIR);
}

/// A region that ticks the 5×5 about the origin, player 1 at the spawn holding
/// `item`, with `placed` laid in its chunks.
fn ticking_holding(placed: &[(BlockPos, BlockState)], item: i32) -> Region {
    let mut region = super::redstone::ticking_with(placed);
    checked(&mut region, &super::redstone::holding(item));
    region.clock.day = NOON;
    region
}

#[test]
fn seeds_are_refused_in_the_dark_and_planted_in_light_of_8() {
    let ground = at(3, -61, 0);
    let wheat = at(3, -60, 0);
    // A roof over the whole 5×5 at y = −57, and a torch seven or six blocks from where
    // the seeds go.
    let roof: Vec<(BlockPos, BlockState)> = (-32..48)
        .flat_map(|x| (-32..48).map(move |z| (at(x, -57, z), blocks::STONE)))
        .collect();
    for (away, light, taken) in [(7, 7, false), (6, 8, true)] {
        let mut placed = roof.clone();
        placed.push((ground, blocks::FARMLAND));
        placed.push((at(3 + away, -60, 0), torch()));
        let mut region = ticking_holding(&placed, items::WHEAT_SEEDS);
        assert_eq!(region.raw_brightness(wheat, 0), Some(light));
        checked(&mut region, &super::redstone::click(ground));
        let expected = if taken { blocks::WHEAT } else { blocks::AIR };
        assert_eq!(read(&region, wheat), expected, "in light {light}");
    }
    // Under the open sky, by night as by day: the sky's light is not darkened here.
    let mut region = ticking_holding(&[(ground, blocks::FARMLAND)], items::WHEAT_SEEDS);
    region.clock.day = MIDNIGHT;
    checked(&mut region, &super::redstone::click(ground));
    assert_eq!(read(&region, wheat), blocks::WHEAT);
}

/// A region holding the chunks west of x = 32, with the column of chunks at x = 2
/// another region's and copied, `here` and `beyond` laid in the two sides' floors; at
/// noon.
fn at_a_line(here: &[(BlockPos, BlockState)], beyond: &[(BlockPos, BlockState)]) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let laid = |position: ChunkPos, blocks: &[(BlockPos, BlockState)]| {
        let mut chunk = floor();
        for (pos, state) in blocks.iter().filter(|(pos, _)| pos.chunk() == position) {
            let (x, z) = pos.in_chunk();
            chunk.set(x, pos.y, z, *state);
        }
        chunk
    };
    region.tick(&TickInputs {
        chunks_loaded: around
            .iter()
            .map(|position| (*position, laid(*position, here)))
            .collect(),
        ..TickInputs::default()
    });
    let copies: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    region.tick(&TickInputs {
        foreign: copies
            .iter()
            .map(|position| (*position, RegionId(1)))
            .collect(),
        copy_bases: copies
            .iter()
            .map(|position| (*position, laid(*position, beyond)))
            .collect(),
        ..TickInputs::default()
    });
    region.clock.day = NOON;
    region.light_step();
    region
}

#[test]
fn grass_spreading_across_a_pinned_line_is_an_effect_that_expects_dirt() {
    let grass = at(31, -61, 4);
    let beyond = at(32, -61, 4);
    let mut region = at_a_line(&[(grass, blocks::GRASS_BLOCK)], &[(beyond, blocks::DIRT)]);
    assert!(region.copies.contains_key(&beyond.chunk()));
    assert!(region.light_at(beyond).is_some(), "the copy is lit");
    let mut sent = Vec::new();
    for seed in 0..400 {
        region.made.clear();
        grow(&mut region, grass, seed);
        sent.extend(region.made.iter().filter_map(|effect| match effect.what {
            Effected::SetIf {
                expected, state, ..
            } => Some((effect.at, expected, state)),
            _ => None,
        }));
    }
    assert!(!sent.is_empty());
    assert!(sent.iter().all(|sent| *sent
        == (
            beyond,
            blocks::DIRT,
            state("minecraft:grass_block[snowy=false]")
        )));
    // The copy itself is the holder's to change.
    assert_eq!(read(&region, beyond), blocks::DIRT);
}
