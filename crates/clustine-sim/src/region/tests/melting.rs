//! What melts by the blocks' light (part D2 of N8, `docs/groundwork/n8-plan.md`, section
//! 6.2), from the class dumps: snow layers in block light above 11 (two blocks from a
//! torch, three from glowstone, not one more), ice above 11 less its dampening, and
//! neither by the sun. Neither rule draws.
//!
//! Each test drives one random tick on the 3×3 of stone floors about the origin (the
//! floor at y = −61), lit by the region's own light step, at noon under the open sky.

use super::*;

use crate::region::Random;

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
            let mut chunk = floor();
            for (pos, state) in placed
                .iter()
                .filter(|(pos, _)| pos.chunk() == ChunkPos::new(x, z))
            {
                let (bx, bz) = pos.in_chunk();
                chunk.set(bx, pos.y, bz, *state);
            }
            region.chunks.insert(ChunkPos::new(x, z), chunk);
        }
    }
    region.clock.day = 6_000;
    region.light_step();
    region
}

/// The random tick of what stands at `pos` by the melting's rules; that it was one, and
/// that it drew nothing.
fn melt(region: &mut Region, pos: BlockPos) -> TickOutput {
    let state = region.block(pos).expect("a loaded block");
    let start = Random::of_section(1, ChunkPos::new(0, 0), 0, 1);
    let mut random = start.clone();
    let mut output = TickOutput::default();
    assert!(region.melting_random_tick(pos, state, &mut random, &mut output));
    assert_eq!(random, start, "melting draws nothing");
    output
}

fn snow() -> BlockState {
    state("minecraft:snow[layers=1]")
}

/// Whether a layer of snow `away` blocks along the floor from a block of `light` melts.
fn snow_melts(light: BlockState, away: i32) -> (bool, u8) {
    let layer = at(0, -60, 0);
    let mut region = scene(&[(layer, snow()), (at(away, -60, 0), light)]);
    let brightness = region.block_brightness(layer).expect("lit");
    melt(&mut region, layer);
    (region.block(layer) == Some(blocks::AIR), brightness)
}

#[test]
fn snow_melts_within_two_blocks_of_a_torch_and_not_three() {
    assert_eq!(snow_melts(blocks::TORCH, 1), (true, 13));
    assert_eq!(snow_melts(blocks::TORCH, 2), (true, 12));
    assert_eq!(snow_melts(blocks::TORCH, 3), (false, 11));
}

#[test]
fn snow_melts_within_three_blocks_of_glowstone_and_not_four() {
    assert_eq!(snow_melts(blocks::GLOWSTONE, 3), (true, 12));
    assert_eq!(snow_melts(blocks::GLOWSTONE, 4), (false, 11));
}

#[test]
fn the_sun_melts_no_snow_and_melting_snow_is_seen_to_go() {
    let layer = at(0, -60, 0);
    let mut region = scene(&[(layer, snow())]);
    assert_eq!(region.raw_brightness(layer, 0), Some(15));
    melt(&mut region, layer);
    assert_eq!(region.block(layer), Some(snow()));

    let mut region = scene(&[(layer, snow()), (at(1, -60, 0), blocks::TORCH)]);
    let output = melt(&mut region, layer);
    assert_eq!(region.block(layer), Some(blocks::AIR));
    assert!(output.events.iter().any(
        |event| matches!(event, RegionEvent::BlockChanged { position, .. } if *position == layer)
    ));
}

#[test]
fn ice_beside_a_torch_melts_to_water_and_ice_far_from_one_or_in_the_sun_stays() {
    let ice = at(0, -60, 0);
    let dampening = i32::from(blocks::ICE.props().light_dampening());
    let mut region = scene(&[(ice, blocks::ICE), (at(1, -60, 0), blocks::TORCH)]);
    assert!(i32::from(region.block_brightness(ice).expect("lit")) > 11 - dampening);
    melt(&mut region, ice);
    assert_eq!(region.block(ice), Some(blocks::WATER));

    let mut region = scene(&[(ice, blocks::ICE), (at(6, -60, 0), blocks::TORCH)]);
    assert!(i32::from(region.block_brightness(ice).expect("lit")) <= 11 - dampening);
    melt(&mut region, ice);
    assert_eq!(region.block(ice), Some(blocks::ICE));

    let mut region = scene(&[(ice, blocks::ICE)]);
    melt(&mut region, ice);
    assert_eq!(region.block(ice), Some(blocks::ICE));
}

#[test]
fn frosted_ice_placed_by_a_command_melts_by_the_blocks_light_as_ice_does() {
    let ice = at(0, -60, 0);
    let frosted = state("minecraft:frosted_ice[age=0]");
    let mut region = scene(&[(ice, frosted), (at(1, -60, 0), blocks::TORCH)]);
    melt(&mut region, ice);
    assert_eq!(region.block(ice), Some(blocks::WATER));
    let mut region = scene(&[(ice, frosted)]);
    melt(&mut region, ice);
    assert_eq!(region.block(ice), Some(frosted));
}

#[test]
fn packed_and_blue_ice_are_no_rule_of_melting() {
    let ice = at(0, -60, 0);
    for block in [blocks::PACKED_ICE, blocks::BLUE_ICE] {
        let mut region = scene(&[(ice, block), (at(1, -60, 0), blocks::TORCH)]);
        let mut output = TickOutput::default();
        let mut random = Random::of_section(1, ChunkPos::new(0, 0), 0, 1);
        assert!(!region.melting_random_tick(ice, block, &mut random, &mut output));
        assert_eq!(region.block(ice), Some(block));
    }
}
