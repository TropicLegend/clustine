//! Each rule's draws in the order of the plan's 6.2 and the dumps, against a replay of
//! the game's, with the outcome the replay predicts, seed for seed.

use super::*;

/// The age `age` of the crop or plant `name`, its other properties as by default.
fn aged(name: &str, age: u8) -> BlockState {
    state(&format!("minecraft:{name}[age={age}]"))
}

/// A crop of `name` at `age` at the origin on dry farmland, in a dark room with a torch
/// `away` blocks east of it: light `14 - away` at the crop.
fn crop_by_a_torch(name: &str, age: u8, away: i32) -> (Region, BlockPos) {
    let crop = at(0, GROUND, 0);
    let region = roofed(&[
        (at(0, GROUND - 1, 0), blocks::FARMLAND),
        (crop, aged(name, age)),
        (at(away, GROUND, 0), blocks::TORCH),
    ]);
    assert_eq!(region.raw_brightness(crop, 0), Some((14 - away) as u8));
    (region, crop)
}

/// The bound a crop alone on dry farmland draws its chance with: `getGrowthSpeed` is 1,
/// and 1 for the dry farmland under it, so `(int)(25 / 2) + 1`.
const ALONE_ON_DRY: u32 = 13;

#[test]
fn wheat_carrots_and_potatoes_read_the_light_first_and_draw_their_chance_only_in_9_or_more() {
    for name in ["wheat", "carrots", "potatoes"] {
        for (away, lit) in [(5, true), (6, false)] {
            let (mut region, crop) = crop_by_a_torch(name, 0, away);
            for seed in 0..150 {
                put(&mut region, crop, aged(name, 0));
                let drawn = tick_at(&mut region, crop, seed);
                let mut replay = Replay::of(seed);
                let grows = lit && replay.int(ALONE_ON_DRY) == 0;
                replay.matches(&drawn, name);
                let age = u8::from(grows);
                assert_eq!(read(&region, crop), aged(name, age), "{name}, seed {seed}");
            }
        }
    }
}

#[test]
fn a_crop_at_its_oldest_reads_the_light_and_draws_nothing() {
    for (name, oldest) in [
        ("wheat", 7),
        ("carrots", 7),
        ("potatoes", 7),
        ("beetroots", 3),
    ] {
        let (mut region, crop) = crop_by_a_torch(name, oldest, 1);
        for seed in 0..40 {
            let drawn = tick_at(&mut region, crop, seed);
            let mut replay = Replay::of(seed);
            if name == "beetroots" {
                // The beetroot's one in three comes before the crop's rule.
                replay.int(3);
            }
            replay.matches(&drawn, name);
            assert_eq!(read(&region, crop), aged(name, oldest));
        }
    }
}

#[test]
fn wheat_under_the_open_sky_grows_at_midnight_since_crops_read_the_sky_undarkened() {
    let crop = at(0, GROUND, 0);
    let mut region = scene_at(
        MIDNIGHT,
        &[
            (at(0, GROUND - 1, 0), blocks::FARMLAND),
            (crop, blocks::WHEAT),
        ],
    );
    assert_eq!(region.raw_brightness(crop, 0), Some(15));
    assert!(
        region.max_local_raw_brightness(crop) < Some(9),
        "dark for grass"
    );
    let mut grown = 0;
    for seed in 0..200 {
        put(&mut region, crop, blocks::WHEAT);
        let drawn = tick_at(&mut region, crop, seed);
        let mut replay = Replay::of(seed);
        let grows = replay.int(ALONE_ON_DRY) == 0;
        replay.matches(&drawn, "wheat at midnight");
        grown += u32::from(grows);
        assert_eq!(read(&region, crop), aged("wheat", u8::from(grows)));
    }
    assert!(grown > 0);
}

#[test]
fn a_diagonal_neighbour_of_the_same_crop_halves_the_speed_of_moist_farmland() {
    // Moist farmland under the crop and the eight around it: 1 + 3 + 8 · 3/4 = 10, so
    // 25 / 10 + 1 = 3. A wheat east of it is a row along x only, which halves nothing;
    // one more at the south-east corner is a diagonal, which halves the speed to 5:
    // 25 / 5 + 1 = 6 (n7 CropBlock 174–326).
    let moist = state("minecraft:farmland[moisture=7]");
    let crop = at(0, GROUND, 0);
    for (diagonal, bound) in [(false, 3), (true, 6)] {
        let mut placed: Vec<(BlockPos, BlockState)> = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| (at(x, GROUND - 1, z), moist)))
            .collect();
        placed.push((crop, blocks::WHEAT));
        placed.push((at(1, GROUND, 0), blocks::WHEAT));
        if diagonal {
            placed.push((at(1, GROUND, 1), blocks::WHEAT));
        }
        let mut region = open(&placed);
        for seed in 0..100 {
            put(&mut region, crop, blocks::WHEAT);
            let drawn = tick_at(&mut region, crop, seed);
            let mut replay = Replay::of(seed);
            let grows = replay.int(bound) == 0;
            replay.matches(&drawn, &format!("a bound of {bound}"));
            assert_eq!(read(&region, crop), aged("wheat", u8::from(grows)));
        }
    }
}

#[test]
fn beetroots_and_the_torchflower_crop_skip_at_a_zero_of_three_before_the_crops_rule() {
    for name in ["beetroots", "torchflower_crop"] {
        for (away, lit) in [(5, true), (6, false)] {
            let (mut region, crop) = crop_by_a_torch(name, 0, away);
            for seed in 0..150 {
                put(&mut region, crop, aged(name, 0));
                let drawn = tick_at(&mut region, crop, seed);
                let mut replay = Replay::of(seed);
                let goes_on = replay.int(3) != 0;
                let grows = goes_on && lit && replay.int(ALONE_ON_DRY) == 0;
                replay.matches(&drawn, name);
                assert_eq!(read(&region, crop), aged(name, u8::from(grows)), "{name}");
            }
        }
    }
}

#[test]
fn the_torchflower_crop_at_age_1_becomes_the_torchflower() {
    let (mut region, crop) = crop_by_a_torch("torchflower_crop", 1, 1);
    let mut flowered = 0;
    for seed in 0..200 {
        put(&mut region, crop, aged("torchflower_crop", 1));
        let drawn = tick_at(&mut region, crop, seed);
        let mut replay = Replay::of(seed);
        let grows = replay.int(3) != 0 && replay.int(ALONE_ON_DRY) == 0;
        replay.matches(&drawn, "torchflower crop");
        let expected = if grows {
            blocks::TORCHFLOWER
        } else {
            aged("torchflower_crop", 1)
        };
        assert_eq!(read(&region, crop), expected, "seed {seed}");
        flowered += u32::from(grows);
    }
    assert!(flowered > 0);
}

/// The pitcher crop's lower half at `age`.
fn pitcher(age: u8, half: &str) -> BlockState {
    state(&format!("minecraft:pitcher_crop[age={age},half={half}]"))
}

#[test]
fn the_pitcher_crop_draws_its_chance_even_in_the_dark_and_grows_from_light_8() {
    for (away, lit) in [(6, true), (7, false)] {
        let crop = at(0, GROUND, 0);
        let mut region = roofed(&[
            (at(0, GROUND - 1, 0), blocks::FARMLAND),
            (crop, pitcher(0, "lower")),
            (at(away, GROUND, 0), blocks::TORCH),
        ]);
        assert_eq!(region.raw_brightness(crop, 0), Some((14 - away) as u8));
        for seed in 0..150 {
            put(&mut region, crop, pitcher(0, "lower"));
            let drawn = tick_at(&mut region, crop, seed);
            let mut replay = Replay::of(seed);
            let grows = replay.int(ALONE_ON_DRY) == 0 && lit;
            replay.matches(&drawn, "pitcher crop");
            assert_eq!(read(&region, crop), pitcher(u8::from(grows), "lower"));
        }
    }
}

#[test]
fn the_pitcher_crop_needs_room_above_only_to_become_two_tall_at_age_3() {
    let crop = at(0, GROUND, 0);
    let above = at(0, GROUND + 1, 0);
    let mut region = open(&[(at(0, GROUND - 1, 0), blocks::FARMLAND)]);
    let success = (0..)
        .find(|seed| stream(*seed).next_int(ALONE_ON_DRY) == 0)
        .expect("one in thirteen");
    // A block above: age 0 still grows to 1, age 2 cannot become two tall.
    put(&mut region, above, blocks::STONE);
    put(&mut region, crop, pitcher(0, "lower"));
    region.light_step();
    tick_at(&mut region, crop, success);
    assert_eq!(read(&region, crop), pitcher(1, "lower"));
    put(&mut region, crop, pitcher(2, "lower"));
    tick_at(&mut region, crop, success);
    assert_eq!(read(&region, crop), pitcher(2, "lower"));
    // Air above: age 2 becomes age 3 and its upper half.
    put(&mut region, above, blocks::AIR);
    region.light_step();
    tick_at(&mut region, crop, success);
    assert_eq!(read(&region, crop), pitcher(3, "lower"));
    assert_eq!(read(&region, above), pitcher(3, "upper"));
}

#[test]
fn a_stem_reads_the_light_first_and_at_age_7_draws_the_side_its_fruit_goes() {
    // `Direction.Plane.HORIZONTAL` is north, east, south, west, and `getRandomDirection`
    // takes `nextInt(4)` of it (n6 Direction$Plane, Util.getRandom).
    const SIDES: [(i32, i32, &str); 4] = [
        (0, -1, "north"),
        (1, 0, "east"),
        (0, 1, "south"),
        (-1, 0, "west"),
    ];
    let stem = at(0, GROUND, 0);
    let mut placed = vec![(at(0, GROUND - 1, 0), blocks::FARMLAND)];
    for (dx, dz, _) in SIDES {
        placed.push((at(dx, GROUND - 1, dz), blocks::DIRT));
    }
    for (name, fruit, attached) in [
        ("melon_stem", blocks::MELON, "attached_melon_stem"),
        ("pumpkin_stem", blocks::PUMPKIN, "attached_pumpkin_stem"),
    ] {
        let mut region = open(&placed);
        let mut fruited = 0;
        for seed in 0..300 {
            put(&mut region, stem, aged(name, 7));
            for (dx, dz, _) in SIDES {
                put(&mut region, at(dx, GROUND, dz), blocks::AIR);
            }
            let drawn = tick_at(&mut region, stem, seed);
            let mut replay = Replay::of(seed);
            let side = (replay.int(ALONE_ON_DRY) == 0).then(|| SIDES[replay.int(4) as usize]);
            replay.matches(&drawn, name);
            match side {
                Some((dx, dz, facing)) => {
                    fruited += 1;
                    assert_eq!(read(&region, at(dx, GROUND, dz)), fruit, "seed {seed}");
                    let turned = state(&format!("minecraft:{attached}[facing={facing}]"));
                    assert_eq!(read(&region, stem), turned, "seed {seed}");
                }
                None => assert_eq!(read(&region, stem), aged(name, 7), "seed {seed}"),
            }
        }
        assert!(fruited > 5, "{fruited}");
        // In the dark, nothing is drawn.
        let mut dark = roofed(&placed);
        put(&mut dark, stem, aged(name, 3));
        dark.light_step();
        for seed in 0..20 {
            let drawn = tick_at(&mut dark, stem, seed);
            Replay::of(seed).matches(&drawn, "a stem in the dark");
            assert_eq!(read(&dark, stem), aged(name, 3));
        }
    }
}

#[test]
fn sweet_berries_draw_their_one_in_five_before_the_light_above_them() {
    let bush = at(0, GROUND, 0);
    for (away, lit) in [(5, true), (6, false)] {
        let mut region = roofed(&[
            (at(0, GROUND - 1, 0), blocks::GRASS_BLOCK),
            (bush, aged("sweet_berry_bush", 0)),
            // Level with the air above the bush, so the light there is 14 - away.
            (at(away, GROUND + 1, 0), blocks::TORCH),
            (at(away, GROUND, 0), blocks::STONE),
        ]);
        assert_eq!(
            region.raw_brightness(bush.offset(0, 1, 0), 0),
            Some((14 - away) as u8)
        );
        for seed in 0..150 {
            put(&mut region, bush, aged("sweet_berry_bush", 0));
            let drawn = tick_at(&mut region, bush, seed);
            let mut replay = Replay::of(seed);
            let grows = replay.int(5) == 0 && lit;
            replay.matches(&drawn, "sweet berries");
            assert_eq!(
                read(&region, bush),
                aged("sweet_berry_bush", u8::from(grows))
            );
        }
    }
    // Ripe, it draws nothing.
    let mut region = open(&[
        (at(0, GROUND - 1, 0), blocks::GRASS_BLOCK),
        (bush, aged("sweet_berry_bush", 3)),
    ]);
    let drawn = tick_at(&mut region, bush, 9);
    Replay::of(9).matches(&drawn, "a ripe bush");
}

#[test]
fn snow_ice_and_frosted_ice_draw_nothing_whether_they_melt_or_not() {
    let pos = at(0, GROUND, 0);
    for (melted, away) in [(true, 1), (false, 6)] {
        for (block, gone) in [
            (blocks::SNOW, blocks::AIR),
            (blocks::ICE, blocks::WATER),
            (blocks::FROSTED_ICE, blocks::WATER),
        ] {
            let mut region = roofed(&[(pos, block), (at(away, GROUND, 0), blocks::GLOWSTONE)]);
            let drawn = tick_at(&mut region, pos, 3);
            Replay::of(3).matches(&drawn, "melting");
            let expected = if melted { gone } else { block };
            assert_eq!(read(&region, pos), expected, "{block:?} {away} away");
        }
    }
}

#[test]
fn a_sapling_by_day_draws_one_in_seven_and_at_stage_0_takes_stage_1_drawing_no_more() {
    let sapling = at(0, GROUND, 0);
    let mut region = open(&[
        (at(0, GROUND - 1, 0), blocks::DIRT),
        (sapling, blocks::BIRCH_SAPLING),
    ]);
    let mut staged = 0;
    for seed in 0..120 {
        put(&mut region, sapling, blocks::BIRCH_SAPLING);
        let drawn = tick_at(&mut region, sapling, seed);
        let mut replay = Replay::of(seed);
        let advances = replay.int(7) == 0;
        replay.matches(&drawn, "a sapling");
        let stage = u8::from(advances);
        let expected = state(&format!("minecraft:birch_sapling[stage={stage}]"));
        assert_eq!(read(&region, sapling), expected);
        staged += u32::from(advances);
    }
    assert!(staged > 0);
}

#[test]
fn a_sapling_in_the_open_at_midnight_and_a_propagule_in_the_dark_differ_in_what_they_read() {
    // The sapling reads the light above it darkened by the night: 4 at midnight under
    // the open sky, so nothing is drawn.
    let sapling = at(0, GROUND, 0);
    let mut night = scene_at(
        MIDNIGHT,
        &[
            (at(0, GROUND - 1, 0), blocks::DIRT),
            (sapling, blocks::OAK_SAPLING),
        ],
    );
    assert_eq!(
        night.max_local_raw_brightness(sapling.offset(0, 1, 0)),
        Some(4)
    );
    for seed in 0..30 {
        let drawn = tick_at(&mut night, sapling, seed);
        Replay::of(seed).matches(&drawn, "a sapling at midnight");
        assert_eq!(read(&night, sapling), blocks::OAK_SAPLING);
    }
    // A propagule on the ground reads none: its one in seven in a dark room.
    let propagule = state("minecraft:mangrove_propagule[hanging=false,stage=0]");
    let mut dark = roofed(&[(at(0, GROUND - 1, 0), blocks::MUD), (sapling, propagule)]);
    assert_eq!(dark.raw_brightness(sapling, 0), Some(0));
    let mut staged = 0;
    for seed in 0..120 {
        put(&mut dark, sapling, propagule);
        let drawn = tick_at(&mut dark, sapling, seed);
        let mut replay = Replay::of(seed);
        let advances = replay.int(7) == 0;
        replay.matches(&drawn, "a propagule in the dark");
        staged += u32::from(advances);
        let stage = u8::from(advances);
        let expected = state(&format!(
            "minecraft:mangrove_propagule[hanging=false,stage={stage}]"
        ));
        assert_eq!(read(&dark, sapling), expected, "seed {seed}");
    }
    assert!(staged > 0);
}
