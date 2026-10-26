//! The thresholds of light at their edges: snow by block light 12, ice by 11 less its
//! dampening, mushrooms below 13, crops and the pitcher kept from 8, seeds planted from
//! 8, saplings and grass from 9 with the night's darkening.

use super::*;
use crate::time::sky_darken;

/// Snow and ice melt by the blocks' light at them alone, whatever the sky says.
fn melts(block: BlockState, source: BlockState, offset: (i32, i32)) -> (bool, u8) {
    let pos = at(0, GROUND, 0);
    let mut region = roofed(&[(pos, block), (at(offset.0, GROUND, offset.1), source)]);
    let light = region.block_brightness(pos).expect("lit");
    tick_at(&mut region, pos, 0);
    (read(&region, pos) != block, light)
}

#[test]
fn snow_melts_two_steps_from_a_torch_and_three_from_glowstone_and_not_one_step_farther() {
    // Steps are counted as light walks them: across and along.
    for (source, offset, light, melted) in [
        (blocks::TORCH, (2, 0), 12, true),
        (blocks::TORCH, (1, 1), 12, true),
        (blocks::TORCH, (3, 0), 11, false),
        (blocks::TORCH, (2, 1), 11, false),
        (blocks::GLOWSTONE, (3, 0), 12, true),
        (blocks::GLOWSTONE, (2, 1), 12, true),
        (blocks::GLOWSTONE, (4, 0), 11, false),
        (state("minecraft:lantern"), (3, 0), 12, true),
        (state("minecraft:lantern"), (2, 2), 11, false),
    ] {
        let (gone, at_snow) = melts(blocks::SNOW, source, offset);
        assert_eq!(at_snow, light, "{source:?} at {offset:?}");
        assert_eq!(gone, melted, "{source:?} at {offset:?}, light {light}");
    }
}

#[test]
fn ice_and_frosted_ice_melt_above_11_less_their_dampening() {
    for block in [blocks::ICE, blocks::FROSTED_ICE] {
        let dampening = i32::from(block.props().light_dampening());
        for away in 1..=7 {
            let (gone, light) = melts(block, blocks::TORCH, (away, 0));
            assert_eq!(
                gone,
                i32::from(light) > 11 - dampening,
                "{block:?} in block light {light}, dampening {dampening}"
            );
        }
        // Whichever the dampening, one torch melts it at some distance and not at
        // another.
        assert!(melts(block, blocks::TORCH, (1, 0)).0);
        assert!(!melts(block, blocks::TORCH, (7, 0)).0);
    }
}

#[test]
fn the_noon_sun_melts_neither_snow_nor_ice() {
    for block in [blocks::SNOW, blocks::ICE] {
        let pos = at(0, GROUND, 0);
        let mut region = open(&[(pos, block)]);
        assert_eq!(levels(&region, pos).map(|(_, block)| block), Some(0));
        tick_at(&mut region, pos, 0);
        assert_eq!(read(&region, pos), block);
    }
}

/// A brown mushroom at the origin on `ground` with glowstone `away` blocks east, after a
/// block is set beside it on the west: whether it is still there, and its light.
fn mushroom_after_a_neighbour_changes(ground: BlockState, away: i32, roofed: bool) -> (bool, u8) {
    let pos = at(0, GROUND, 0);
    let placed = [
        (pos.offset(0, -1, 0), ground),
        (pos, blocks::BROWN_MUSHROOM),
        (at(away, GROUND, 0), blocks::GLOWSTONE),
    ];
    let mut region = if roofed {
        super::roofed(&placed)
    } else {
        open(&placed)
    };
    let light = region.raw_brightness(pos, 0).expect("lit");
    set_updated(&mut region, at(-1, GROUND, 0), blocks::STONE);
    (read(&region, pos) == blocks::BROWN_MUSHROOM, light)
}

#[test]
fn a_mushroom_on_stone_stays_in_light_12_and_goes_in_13_at_its_next_shape_update() {
    assert_eq!(
        mushroom_after_a_neighbour_changes(blocks::STONE, 3, true),
        (true, 12)
    );
    assert_eq!(
        mushroom_after_a_neighbour_changes(blocks::STONE, 2, true),
        (false, 13)
    );
    // Under the open sky by day it cannot stay on stone; on mycelium or podzol it stays
    // in any light.
    assert_eq!(
        mushroom_after_a_neighbour_changes(blocks::STONE, 8, false),
        (false, 15)
    );
    for ground in [blocks::MYCELIUM, blocks::PODZOL] {
        assert_eq!(
            mushroom_after_a_neighbour_changes(ground, 8, false),
            (true, 15)
        );
    }
}

/// Carrots at the origin on farmland in a dark room, a torch `away` blocks east.
fn carrots_by_a_torch(away: i32) -> (Region, BlockPos) {
    let crop = at(0, GROUND, 0);
    let region = roofed(&[
        (at(0, GROUND - 1, 0), blocks::FARMLAND),
        (crop, blocks::CARROTS),
        (at(away, GROUND, 0), blocks::TORCH),
    ]);
    (region, crop)
}

#[test]
fn a_crop_in_light_7_breaks_at_a_neighbour_s_change_and_in_light_8_stays() {
    for (away, light, stays) in [(6, 8, true), (7, 7, false)] {
        let (mut region, crop) = carrots_by_a_torch(away);
        assert_eq!(region.raw_brightness(crop, 0), Some(light));
        set_updated(&mut region, at(0, GROUND, 1), blocks::STONE);
        let expected = if stays { blocks::CARROTS } else { blocks::AIR };
        assert_eq!(read(&region, crop), expected, "light {light}");
    }
}

#[test]
fn a_crop_gone_dark_stays_through_the_change_of_light_and_breaks_at_the_next_neighbour() {
    let (mut region, crop) = carrots_by_a_torch(3);
    let torch = at(3, GROUND, 0);
    set_updated(&mut region, torch, blocks::AIR);
    region.light_step();
    assert_eq!(region.raw_brightness(crop, 0), Some(0));
    // The light changed, and nothing beside the crop: it stays, step after step.
    for _ in 0..3 {
        region.light_step();
    }
    assert_eq!(read(&region, crop), blocks::CARROTS);
    // A block two away is no neighbour either.
    set_updated(&mut region, at(2, GROUND, 0), blocks::STONE);
    assert_eq!(read(&region, crop), blocks::CARROTS);
    // A block above it is.
    set_updated(&mut region, crop.offset(0, 1, 0), blocks::STONE);
    assert_eq!(read(&region, crop), blocks::AIR);
}

#[test]
fn the_pitcher_s_lower_half_stays_in_light_8_and_breaks_in_7() {
    for (away, stays) in [(6, true), (7, false)] {
        let crop = at(0, GROUND, 0);
        let lower = state("minecraft:pitcher_crop[age=0,half=lower]");
        let mut region = roofed(&[
            (at(0, GROUND - 1, 0), blocks::FARMLAND),
            (crop, lower),
            (at(away, GROUND, 0), blocks::TORCH),
        ]);
        set_updated(&mut region, at(0, GROUND, 1), blocks::STONE);
        assert_eq!(read(&region, crop) == lower, stays, "{away} away");
    }
}

/// A region that ticks the 5×5 about the origin, roofed over all of it, player 1 at the
/// spawn holding `item`, with `placed` laid in its chunks.
fn roofed_and_holding(placed: &[(BlockPos, BlockState)], item: i32) -> Region {
    let mut all = roof(2);
    all.extend_from_slice(placed);
    let mut region = super::super::redstone::ticking_with(&all);
    checked(&mut region, &super::super::redstone::holding(item));
    region
}

#[test]
fn every_seed_is_refused_in_light_7_and_planted_in_light_8() {
    let ground = at(3, GROUND - 1, 0);
    let planted_at = at(3, GROUND, 0);
    for (item, planted) in [
        (items::WHEAT_SEEDS, blocks::WHEAT),
        (items::CARROT, blocks::CARROTS),
        (items::POTATO, blocks::POTATOES),
        (items::BEETROOT_SEEDS, blocks::BEETROOTS),
        (items::TORCHFLOWER_SEEDS, blocks::TORCHFLOWER_CROP),
        (
            items::PITCHER_POD,
            state("minecraft:pitcher_crop[age=0,half=lower]"),
        ),
    ] {
        // Glowstone seven blocks off: 8; eight: 7.
        for (away, light) in [(7, 8), (8, 7)] {
            let mut region = roofed_and_holding(
                &[
                    (ground, blocks::FARMLAND),
                    (at(3 + away, GROUND, 0), blocks::GLOWSTONE),
                ],
                item,
            );
            assert_eq!(region.raw_brightness(planted_at, 0), Some(light));
            checked(&mut region, &super::super::redstone::click(ground));
            let expected = if light >= 8 { planted } else { blocks::AIR };
            assert_eq!(
                read(&region, planted_at),
                expected,
                "item {item}, light {light}"
            );
        }
    }
}

/// A grass block's four tries at a target, x, y and z each.
fn four_tries(replay: &mut Replay) {
    for _ in 0..4 {
        replay.int(3);
        replay.int(5);
        replay.int(3);
    }
}

/// The first day time from noon on, towards dusk, at which the night darkens the sky's
/// light by `darken`.
fn darkened_by(darken: i32) -> u64 {
    (NOON..NOON + 12_000)
        .find(|day| sky_darken(*day) == darken)
        .expect("the sky darkens by every step from noon to midnight")
}

#[test]
fn grass_spreads_in_the_open_until_the_night_darkens_the_sky_below_9() {
    // A field of dirt about the grass, so that a spread is likely to hit.
    let grass = at(0, GROUND, 0);
    let mut placed: Vec<(BlockPos, BlockState)> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| (at(x, GROUND - 1, z), blocks::DIRT)))
        .collect();
    placed.push((at(0, GROUND - 1, 0), blocks::STONE));
    placed.push((grass, blocks::GRASS_BLOCK));
    for (darken, spreads) in [(6, true), (7, false)] {
        let day = darkened_by(darken);
        let mut region = scene_at(day, &placed);
        assert_eq!(
            region.max_local_raw_brightness(grass.offset(0, 1, 0)),
            Some((15 - darken) as u8)
        );
        let drawn = tick_at(&mut region, grass, 5);
        let mut replay = Replay::of(5);
        if spreads {
            four_tries(&mut replay);
        }
        replay.matches(&drawn, &format!("grass darkened by {darken}"));
    }
}

#[test]
fn a_sapling_and_grass_by_a_torch_at_midnight_take_light_9_and_not_8() {
    let pos = at(0, GROUND, 0);
    for (away, light, reads) in [(5, 9, true), (6, 8, false)] {
        for block in [blocks::OAK_SAPLING, blocks::GRASS_BLOCK] {
            // The torch level with the block above, on a stone pillar.
            let mut region = roofed_at(
                MIDNIGHT,
                &[
                    (at(0, GROUND - 1, 0), blocks::DIRT),
                    (pos, block),
                    (at(away, GROUND, 0), blocks::STONE),
                    (at(away, GROUND + 1, 0), blocks::TORCH),
                ],
            );
            assert_eq!(
                region.max_local_raw_brightness(pos.offset(0, 1, 0)),
                Some(light)
            );
            let drawn = tick_at(&mut region, pos, 11);
            let mut replay = Replay::of(11);
            match (reads, block == blocks::OAK_SAPLING) {
                (false, _) => {}
                // Its one in seven.
                (true, true) => {
                    replay.int(7);
                }
                (true, false) => four_tries(&mut replay),
            }
            replay.matches(&drawn, &format!("{block:?} in light {light}"));
        }
    }
}
