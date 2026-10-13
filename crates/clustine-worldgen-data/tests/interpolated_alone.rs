//! The final density at single positions where the game's two ways of multiplying a
//! noise's coordinates part.
//!
//! At a position that is no corner of a cell the game computes an `interpolated`
//! function by filling a volume of the cell's eight corners, and in a volume a noise
//! that is not shifted multiplies its scale by a layer's frequency before the
//! coordinate (`density::noise3` and `clustine_noise::NormalNoise::get_in_volume` say
//! more). Almost everywhere that gives the bits of the corner asked alone. In the
//! chunk these positions are in, at the blocks 10016 to 10031 and 22784 to 22799 of
//! the overworld of seed 13579, it does not: a noise of the noodle caves differs in
//! its last bits over the corners at z 22788, and with it the final density at the
//! eighteen blocks that noise reaches.
//!
//! The values are `RandomState.sampleBlockValueUncached` of the pinned jar's own
//! classes for the game's final density, asked without a server: Mojang's data, not
//! under the licence of this file (see NOTICE.md at the root of the repository). All
//! 98,304 positions of the chunk were compared on 2026-10-13 and were equal; with the
//! corners each computed alone, these were not.

use clustine_worldgen_data::{NoiseRouter, OverworldRouter, RouterEntry};

/// A block position, the bits of the game's final density there, and the bits that
/// interpolating between corners asked alone would give.
const KNOWN: [(i32, i32, i32, u32, u32); 8] = [
    (10022, 46, 22785, 0x3df4_ce60, 0x3df4_ce62),
    (10016, 18, 22787, 0x3d47_2189, 0x3d47_218b),
    (10016, 17, 22788, 0x3cf0_dd0a, 0x3cf0_dd0e),
    (10022, 11, 22788, 0x3c85_f3d8, 0x3c85_f3e0),
    (10016, 16, 22790, 0x3b40_9b00, 0x3b40_9b20),
    (10025, 7, 22790, 0x3c8a_f504, 0x3c8a_f510),
    (10022, 9, 22791, 0xbb67_2530, 0xbb67_2520),
    (10023, 8, 22791, 0xb9a4_d200, 0xb9a4_d000),
];

#[test]
fn the_corners_of_a_cell_are_computed_as_the_game_computes_a_volume() {
    let router = OverworldRouter::new(13579);
    let mut memory = Default::default();
    for (x, y, z, theirs, alone) in KNOWN {
        let ours = router.sample(&mut memory, RouterEntry::FinalDensity, x, y, z);
        assert_eq!(
            ours.to_bits(),
            theirs,
            "at {x} {y} {z}: {ours:e}, the game's {:e}",
            f32::from_bits(theirs)
        );
        assert_ne!(theirs, alone);
    }
}

#[test]
fn what_is_remembered_for_a_volume_is_not_given_to_one_who_asks_alone() {
    // The same position asked both ways through one memory, in both orders: a value
    // remembered for the one must not be handed to the other.
    use clustine_worldgen_data::Give;
    let router = OverworldRouter::new(13579);
    let numbers: Vec<usize> = (0..router.interpolated().len()).collect();
    let ask = |memory: &mut <OverworldRouter as NoiseRouter>::Memory, in_volume: bool| {
        let before = memory.in_volume(in_volume);
        let values: Vec<u32> = numbers
            .iter()
            .map(|number| {
                router
                    .interpolated_input(memory, *number, 10016, 16, 22788)
                    .to_bits()
            })
            .collect();
        memory.in_volume(before);
        values
    };
    let alone = ask(&mut Default::default(), false);
    let in_volume = ask(&mut Default::default(), true);
    assert_ne!(alone, in_volume, "the two ways part at this corner");

    let mut memory = Default::default();
    assert_eq!(ask(&mut memory, true), in_volume);
    assert_eq!(ask(&mut memory, false), alone);
    assert_eq!(ask(&mut memory, true), in_volume);
}
