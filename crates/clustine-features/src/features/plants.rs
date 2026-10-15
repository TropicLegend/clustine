//! The kinds of configured features that step F4 of `docs/groundwork/f-plan.md` builds
//! (`crate::dispatch::KINDS`): patches of moss and clay with what grows on them,
//! bamboo, vines, glow lichen and sculk veins, sculk patches, coral and the basalt
//! pillars' patchy squares. Each kind is a file of `plants/`; `multiface.rs` is the
//! game's `MultifaceSpreader`, which lichen, veins and sculk share.
//!
//! Kelp, cactus and sugar cane are `block_column`, sea pickles `simple_block`,
//! seagrass and the coral reefs selectors and overlays: all of step F1b.

use clustine_data::Face;
use clustine_noise::Random;

use crate::feature::Feature;
use crate::reader::{Read, Reader};

pub mod bamboo;
pub mod coral;
pub mod multiface;
pub mod multiface_growth;
pub mod projected_random_patchy_square;
pub mod sculk_patch;
pub mod vegetation_patch;
pub mod vines;

/// Reads a configured feature of one of this step's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    Ok(match kind {
        "minecraft:vegetation_patch" => {
            Box::new(vegetation_patch::VegetationPatch::read(reader, false)?)
        }
        "minecraft:waterlogged_vegetation_patch" => {
            Box::new(vegetation_patch::VegetationPatch::read(reader, true)?)
        }
        "minecraft:bamboo" => Box::new(bamboo::Bamboo::read(reader)?),
        "minecraft:vines" => Box::new(vines::Vines),
        "minecraft:multiface_growth" => Box::new(multiface_growth::MultifaceGrowth::read(reader)?),
        "minecraft:sculk_patch" => Box::new(sculk_patch::SculkPatch::read(reader)?),
        "minecraft:coral_claw" => Box::new(coral::Coral::read(reader, coral::Shape::Claw)?),
        "minecraft:coral_tree" => Box::new(coral::Coral::read(reader, coral::Shape::Tree)?),
        "minecraft:projected_random_patchy_square" => {
            Box::new(projected_random_patchy_square::ProjectedRandomPatchySquare::read(reader)?)
        }
        other => return Err(reader.error(format!("{other} is not a kind of step F4"))),
    })
}

/// Shuffles `items` in place as Java's `Util.shuffle` and `Util.shuffledCopy` do: from
/// the last item to the second, each swapped with one drawn from those up to it.
pub fn shuffle<T, R: Random + ?Sized>(random: &mut R, items: &mut [T]) {
    for i in (2..=items.len()).rev() {
        let j = random.next_i32_bounded(i as i32) as usize;
        items.swap(i - 1, j);
    }
}

/// `Direction.allShuffled`: the six directions in the game's order, shuffled.
pub fn all_shuffled<R: Random + ?Sized>(random: &mut R) -> [Face; 6] {
    let mut directions = Face::ALL;
    shuffle(random, &mut directions);
    directions
}

/// The direction a quarter turn clockwise, seen from above (`getClockWise`), of a
/// horizontal direction.
pub fn clockwise(face: Face) -> Face {
    match face {
        Face::North => Face::East,
        Face::East => Face::South,
        Face::South => Face::West,
        Face::West => Face::North,
        vertical => vertical,
    }
}

/// The direction a quarter turn anticlockwise (`getCounterClockWise`).
pub fn anticlockwise(face: Face) -> Face {
    match face {
        Face::North => Face::West,
        Face::West => Face::South,
        Face::South => Face::East,
        Face::East => Face::North,
        vertical => vertical,
    }
}

/// Whether two directions lie on one axis.
pub fn same_axis(a: Face, b: Face) -> bool {
    a == b || a == b.opposite()
}

#[cfg(test)]
mod tests {
    use clustine_noise::WorldgenRandom;

    use super::*;
    use crate::dispatch::{Batch, KINDS};

    #[test]
    fn every_kind_of_this_step_is_read_by_it() {
        use clustine_worldgen_data::feature::Data;
        static EMPTY: Data = Data::Object(&[]);
        for (kind, batch) in KINDS {
            if batch != Batch::Plants {
                continue;
            }
            let reader = Reader::new(&EMPTY, kind);
            // Each kind is known here: it fails for a missing member, never for its
            // kind (vines have none to miss).
            if let Err(error) = super::read(kind, &reader) {
                assert!(!error.what.contains("is not a kind"), "{kind}: {error}");
            }
        }
    }

    #[test]
    fn a_shuffle_swaps_from_the_end_as_java_does() {
        // Java's `Util.shuffle` draws `nextInt(i)` for i from the length down to 2;
        // the same draws taken by hand give the same order.
        let mut random = WorldgenRandom::from_seed(42);
        let mut by_hand = WorldgenRandom::from_seed(42);
        let mut items = [0, 1, 2, 3, 4, 5];
        shuffle(&mut random, &mut items);
        let mut expected = [0, 1, 2, 3, 4, 5];
        for i in (2..=6).rev() {
            let j = by_hand.next_i32_bounded(i) as usize;
            expected.swap(i as usize - 1, j);
        }
        assert_eq!(items, expected);
        assert_eq!(random.next_i32(), by_hand.next_i32());
    }

    #[test]
    fn turns_go_round_the_compass() {
        for face in crate::blocks::HORIZONTAL {
            assert_eq!(anticlockwise(clockwise(face)), face);
            assert_eq!(clockwise(clockwise(face)), face.opposite());
        }
        assert!(same_axis(Face::Up, Face::Down));
        assert!(!same_axis(Face::Up, Face::North));
    }
}
