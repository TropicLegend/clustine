//! Mushrooms, grass and mycelium, bamboo and its shoot, and stems with their fruit:
//! the walks, tries and heights of the dumps (n8 MushroomBlock 26–137,
//! SpreadingSnowyBlock 15–167, BambooStalkBlock 227–270 and 454–572, BambooSaplingBlock
//! 21–85, StemBlock 71–256, AttachedStemBlock 54–106), predicted seed for seed.

use std::collections::BTreeSet;

use super::*;

/// The first seeds from 0 whose stream's first `nextInt(bound)` is 0.
pub(super) fn successes(bound: u32, count: usize) -> Vec<u64> {
    (0..)
        .filter(|seed| stream(*seed).next_int(bound) == 0)
        .take(count)
        .collect()
}

/// Where a brown mushroom at `from` in a dark room on the stone floor spreads to, by
/// the game's walk, with the draws a replay of `seed` makes (its first `nextInt(25)`
/// taken already): the ground (y = −60) is where one may stand, being empty, over stone
/// and dark; `taken` are the blocks that are not empty there.
pub(super) fn walk(
    replay: &mut Replay,
    from: BlockPos,
    taken: &BTreeSet<BlockPos>,
) -> Option<BlockPos> {
    let may_stand = |pos: BlockPos| pos.y == GROUND && !taken.contains(&pos);
    let next = |replay: &mut Replay, from: BlockPos| {
        let dx = replay.int(3) as i32 - 1;
        let dy = replay.int(2) as i32 - replay.int(2) as i32;
        let dz = replay.int(3) as i32 - 1;
        from.offset(dx, dy, dz)
    };
    let mut standing = from;
    let mut target = next(replay, standing);
    for _ in 0..4 {
        if may_stand(target) {
            standing = target;
        }
        // Drawn whether or not the step was taken.
        target = next(replay, standing);
    }
    may_stand(target).then_some(target)
}

/// The brown mushrooms in the box of the room about the origin.
fn mushrooms(region: &Region) -> Vec<BlockPos> {
    (-8..=8)
        .flat_map(|x| {
            (GROUND - 1..=GROUND + 1).flat_map(move |y| (-8..=8).map(move |z| at(x, y, z)))
        })
        .filter(|pos| read(region, *pos) == blocks::BROWN_MUSHROOM)
        .collect()
}

#[test]
fn a_mushroom_walks_four_steps_drawing_every_target_and_lands_where_the_walk_ends() {
    let origin = at(0, GROUND, 0);
    let mut region = roofed(&[(origin, blocks::BROWN_MUSHROOM)]);
    let taken = BTreeSet::from([origin]);
    let mut landed_away = 0;
    for seed in successes(25, 150) {
        let drawn = tick_at(&mut region, origin, seed);
        let mut replay = Replay::of(seed);
        replay.int(25);
        let landing = walk(&mut replay, origin, &taken);
        replay.matches(&drawn, "a mushroom's walk");
        let mut expected = vec![origin];
        if let Some(landing) = landing {
            expected.push(landing);
            expected.sort();
            let away = (landing.x - origin.x)
                .abs()
                .max((landing.z - origin.z).abs());
            landed_away += u32::from(away > 1);
        }
        assert_eq!(mushrooms(&region), expected, "seed {seed}");
        if let Some(landing) = landing {
            put(&mut region, landing, blocks::AIR);
        }
    }
    // A walk takes it beyond its first ring now and then, as no single step could.
    assert!(landed_away > 0);
}

#[test]
fn a_mushroom_draws_only_its_chance_where_five_of_its_kind_stand_in_the_nine_by_three_by_nine() {
    let origin = at(0, GROUND, 0);
    let seed = successes(25, 1)[0];
    // Itself and three more: four, which is at most four.
    let near = [
        at(4, GROUND, 4),
        at(-4, GROUND + 1, -4),
        at(4, GROUND - 1, -4),
    ];
    let mut placed = vec![(origin, blocks::BROWN_MUSHROOM)];
    placed.extend(near.iter().map(|pos| (*pos, blocks::BROWN_MUSHROOM)));
    let spreads = |placed: &[(BlockPos, BlockState)]| {
        let mut region = roofed(placed);
        let drawn = tick_at(&mut region, origin, seed);
        drawn.draws_since(&stream(seed))
    };
    let mut replay = Replay::of(seed);
    replay.int(25);
    let taken: BTreeSet<BlockPos> = placed.iter().map(|(pos, _)| *pos).collect();
    walk(&mut replay, origin, &taken);
    let walked = replay.random.draws_since(&stream(seed));
    assert_eq!(spreads(&placed), walked, "four of its kind: it spreads");
    // A fifth, on the box's edge: one draw, its chance, and no walk.
    let mut five = placed.clone();
    five.push((at(-4, GROUND, 4), blocks::BROWN_MUSHROOM));
    assert_eq!(spreads(&five), 1);
    // The fifth one block beyond the box is not counted.
    let mut beyond = placed.clone();
    beyond.push((at(-5, GROUND, 4), blocks::BROWN_MUSHROOM));
    assert!(spreads(&beyond) > 1);
    let mut above = placed.clone();
    above.push((at(0, GROUND + 2, 0), blocks::BROWN_MUSHROOM));
    assert!(spreads(&above) > 1);
    // Nor are mushrooms of the other kind.
    let mut red = placed.clone();
    red.push((at(-4, GROUND, 4), blocks::RED_MUSHROOM));
    red.push((at(3, GROUND, 3), blocks::RED_MUSHROOM));
    assert!(spreads(&red) > 1);
}

/// Grass on the ground at the origin, and the floor beneath (1, 0) dirt: the one block
/// it can spread to, one east and one down.
const GRASS: BlockPos = BlockPos::new(0, GROUND, 0);
const DIRT_AT: BlockPos = BlockPos::new(1, GROUND - 1, 0);

#[test]
fn grass_tries_four_targets_drawn_x_then_y_then_z_and_takes_the_one_dirt_block_it_hits() {
    let mut region = open(&[(GRASS, blocks::GRASS_BLOCK), (DIRT_AT, blocks::DIRT)]);
    let mut spread = 0;
    for seed in 0..600 {
        put(&mut region, DIRT_AT, blocks::DIRT);
        let drawn = tick_at(&mut region, GRASS, seed);
        let mut replay = Replay::of(seed);
        let mut hit = false;
        for _ in 0..4 {
            let dx = replay.int(3) as i32 - 1;
            let dy = replay.int(5) as i32 - 3;
            let dz = replay.int(3) as i32 - 1;
            hit |= GRASS.offset(dx, dy, dz) == DIRT_AT;
        }
        replay.matches(&drawn, "grass");
        let expected = if hit {
            state("minecraft:grass_block[snowy=false]")
        } else {
            blocks::DIRT
        };
        assert_eq!(read(&region, DIRT_AT), expected, "seed {seed}");
        spread += u32::from(hit);
    }
    assert!(spread > 10, "{spread}");
}

#[test]
fn mycelium_spreads_onto_dirt_by_the_same_tries() {
    let mut region = open(&[(GRASS, blocks::MYCELIUM), (DIRT_AT, blocks::DIRT)]);
    for seed in 0..200 {
        put(&mut region, DIRT_AT, blocks::DIRT);
        let drawn = tick_at(&mut region, GRASS, seed);
        let mut replay = Replay::of(seed);
        let mut hit = false;
        for _ in 0..4 {
            let dx = replay.int(3) as i32 - 1;
            let dy = replay.int(5) as i32 - 3;
            let dz = replay.int(3) as i32 - 1;
            hit |= GRASS.offset(dx, dy, dz) == DIRT_AT;
        }
        replay.matches(&drawn, "mycelium");
        let expected = if hit {
            state("minecraft:mycelium[snowy=false]")
        } else {
            blocks::DIRT
        };
        assert_eq!(read(&region, DIRT_AT), expected, "seed {seed}");
    }
}

#[test]
fn grass_dies_under_a_bottom_slab_or_two_layers_of_snow_and_lives_under_a_top_slab_or_one() {
    let above = GRASS.offset(0, 1, 0);
    for (cover, dies) in [
        ("minecraft:stone_slab[type=bottom]", true),
        ("minecraft:snow[layers=2]", true),
        ("minecraft:water", true),
        ("minecraft:stone", true),
        ("minecraft:stone_slab[type=top]", false),
        ("minecraft:snow[layers=1]", false),
        ("minecraft:glass", false),
        ("minecraft:oak_leaves[persistent=true]", false),
    ] {
        let mut region = open(&[(GRASS, blocks::GRASS_BLOCK), (above, state(cover))]);
        let drawn = tick_at(&mut region, GRASS, 1);
        let now = read(&region, GRASS);
        if dies {
            // Dying draws nothing.
            Replay::of(1).matches(&drawn, cover);
            assert_eq!(now, blocks::DIRT, "under {cover}");
        } else {
            assert!(
                now == blocks::GRASS_BLOCK || now == state("minecraft:grass_block[snowy=true]"),
                "under {cover}: {now:?}"
            );
        }
    }
}

#[test]
fn grass_does_not_spread_to_dirt_under_water_or_that_would_die() {
    for (over_dirt, takes) in [
        (blocks::AIR, true),
        (blocks::WATER, false),
        (state("minecraft:stone_slab[type=bottom]"), false),
        (state("minecraft:snow[layers=1]"), true),
    ] {
        let mut region = open(&[
            (GRASS, blocks::GRASS_BLOCK),
            (DIRT_AT, blocks::DIRT),
            (DIRT_AT.offset(0, 1, 0), over_dirt),
        ]);
        let mut taken = false;
        for seed in 0..200 {
            tick_at(&mut region, GRASS, seed);
            let now = read(&region, DIRT_AT);
            if now != blocks::DIRT {
                taken = true;
                // Snowy from the snow above it.
                let snowy = over_dirt == state("minecraft:snow[layers=1]");
                let expected = state(&format!("minecraft:grass_block[snowy={snowy}]"));
                assert_eq!(now, expected);
                break;
            }
        }
        assert_eq!(taken, takes, "under {over_dirt:?}");
    }
}

/// A stalk of `height` bamboo on dirt at the origin, its top of stage `stage`; the top.
fn stalk(height: i32, stage: u8) -> (Region, BlockPos) {
    let mut placed = vec![(at(0, GROUND - 1, 0), blocks::DIRT)];
    for y in 0..height {
        let top = y == height - 1;
        let stage = if top { stage } else { 1 };
        placed.push((
            at(0, GROUND + y, 0),
            state(&format!(
                "minecraft:bamboo[age=0,leaves=none,stage={stage}]"
            )),
        ));
    }
    (open(&placed), at(0, GROUND + height - 1, 0))
}

fn is_bamboo(state: BlockState) -> bool {
    clustine_blocks::data::is_block(state, blocks::BAMBOO)
}

fn stage_of(state: BlockState) -> Option<&'static str> {
    state
        .properties()
        .find(|(name, _)| *name == "stage")
        .map(|(_, value)| value)
}

#[test]
fn bamboo_draws_its_float_only_from_a_stalk_of_11_takes_stage_1_at_15_and_stops_at_16() {
    for (height, floats) in [(1, false), (10, false), (11, true), (14, true), (15, true)] {
        let mut grown = 0;
        for seed in successes(3, 60) {
            let (mut region, top) = stalk(height, 0);
            let drawn = tick_at(&mut region, top, seed);
            let mut replay = Replay::of(seed);
            replay.int(3);
            let quick = floats && replay.float() < 0.25;
            replay.matches(&drawn, &format!("a stalk of {height}"));
            let new = read(&region, top.offset(0, 1, 0));
            assert!(is_bamboo(new), "{height}: {new:?}");
            let stage = if quick || height == 15 { "1" } else { "0" };
            assert_eq!(
                stage_of(new),
                Some(stage),
                "a stalk of {height}, seed {seed}"
            );
            grown += 1;
        }
        assert_eq!(grown, 60);
    }
    // A stalk of 16 whose top is still of stage 0 draws its one in three and grows no
    // more; a top of stage 1 draws nothing.
    for seed in successes(3, 20) {
        let (mut region, top) = stalk(16, 0);
        let drawn = tick_at(&mut region, top, seed);
        let mut replay = Replay::of(seed);
        replay.int(3);
        replay.matches(&drawn, "a stalk of 16");
        assert_eq!(read(&region, top.offset(0, 1, 0)), blocks::AIR);
        let (mut region, top) = stalk(5, 1);
        let drawn = tick_at(&mut region, top, seed);
        Replay::of(seed).matches(&drawn, "a top of stage 1");
        assert_eq!(read(&region, top.offset(0, 1, 0)), blocks::AIR);
    }
}

#[test]
fn bamboo_and_its_shoot_draw_one_in_three_and_grow_only_into_air_in_light_9() {
    for shoot in [false, true] {
        let block = if shoot {
            blocks::BAMBOO_SAPLING
        } else {
            state("minecraft:bamboo[age=0,leaves=none,stage=0]")
        };
        let base = at(0, GROUND, 0);
        // A torch five blocks east and level with the air above: 9 there in the dark;
        // six blocks: 8.
        for (away, cover, grows) in [
            (5, blocks::AIR, true),
            (6, blocks::AIR, false),
            (5, blocks::STONE, false),
        ] {
            let mut region = roofed(&[
                (at(0, GROUND - 1, 0), blocks::DIRT),
                (base, block),
                (base.offset(0, 1, 0), cover),
                (at(away, GROUND + 1, 0), blocks::TORCH),
                (at(away, GROUND, 0), blocks::STONE),
            ]);
            if cover == blocks::AIR {
                assert_eq!(
                    region.raw_brightness(base.offset(0, 1, 0), 0),
                    Some((14 - away) as u8)
                );
            }
            for seed in 0..60 {
                put(&mut region, base, block);
                put(&mut region, base.offset(0, 1, 0), cover);
                let drawn = tick_at(&mut region, base, seed);
                let mut replay = Replay::of(seed);
                let grew = replay.int(3) == 0 && grows;
                replay.matches(&drawn, "bamboo");
                let above = read(&region, base.offset(0, 1, 0));
                assert_eq!(
                    is_bamboo(above),
                    grew,
                    "shoot {shoot}, {away} away, seed {seed}"
                );
                if grew && shoot {
                    // Small leaves above, and the shoot turned bamboo by its shape update
                    // from above.
                    assert_eq!(above, state("minecraft:bamboo[leaves=small]"));
                    assert_eq!(read(&region, base), blocks::BAMBOO);
                }
            }
        }
    }
}

#[test]
fn a_stem_turns_back_only_when_the_fruit_it_faces_goes_and_then_fruits_again() {
    let stem = at(0, GROUND, 0);
    let mut placed = vec![(at(0, GROUND - 1, 0), blocks::FARMLAND)];
    for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        placed.push((at(dx, GROUND - 1, dz), blocks::DIRT));
    }
    placed.push((stem, state("minecraft:melon_stem[age=7]")));
    let mut region = open(&placed);
    // A seed whose chance succeeds and whose side is east: `nextInt(4)` of 1.
    let east = (0..)
        .find(|seed| {
            let mut random = stream(*seed);
            random.next_int(13) == 0 && random.next_int(4) == 1
        })
        .expect("one in fifty-two");
    tick_at(&mut region, stem, east);
    let fruit = at(1, GROUND, 0);
    assert_eq!(read(&region, fruit), blocks::MELON);
    let attached = state("minecraft:attached_melon_stem[facing=east]");
    assert_eq!(read(&region, stem), attached);
    // A block set on another side is no change of its fruit: it stays attached.
    set_updated(&mut region, at(-1, GROUND, 0), blocks::STONE);
    assert_eq!(read(&region, stem), attached);
    // The fruit harvested: a stem of age 7 again.
    set_updated(&mut region, fruit, blocks::AIR);
    assert_eq!(read(&region, stem), state("minecraft:melon_stem[age=7]"));
    // And it fruits again, now east or on the two sides still open.
    let mut again = None;
    for seed in successes(13, 40) {
        tick_at(&mut region, stem, seed);
        if read(&region, stem) != state("minecraft:melon_stem[age=7]") {
            again = Some(seed);
            break;
        }
    }
    assert!(again.is_some(), "it fruited again");
    let sides = [at(0, GROUND, -1), fruit, at(0, GROUND, 1)];
    let fruits = sides
        .iter()
        .filter(|pos| read(&region, **pos) == blocks::MELON)
        .count();
    assert_eq!(fruits, 1);
    assert_eq!(read(&region, at(-1, GROUND, 0)), blocks::STONE);
}
