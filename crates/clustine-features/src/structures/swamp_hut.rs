// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/swamp_hut.rs`
// (see NOTICE in this crate), which follows the game's `SwampHutPiece`.

//! The swamp hut (`SwampHutPiece`): a hut of spruce on four oak stilts that reach down
//! to the ground, with a potted red mushroom, a crafting table and a cauldron.
//!
//! The game makes a witch and a black cat in the hut, by the run of the chunk of the
//! block where they stand, and keeps that they were made. Clustine makes them there
//! too, finalised from the region's random source; a type its maker does not make yet
//! is drawn for as the game's mob would be ([`finalize_mob_spawn`]), since what is
//! placed after it draws from the same source.
//!
//! The piece settles on the mean height of the ground under its columns in the first
//! chunk that places it.

use clustine_data::{BlockState, blocks, entity_types};
use clustine_noise::Random;
use clustine_structures::piece::StructurePiece;
use clustine_world::generation::PieceState;

use super::Placing;
use super::scattered::{
    Piece, finalize_mob_spawn, inside, region_random, settle_on_average, settled_box, with,
};

/// What the piece keeps of its mobs, as bits of `PieceState::flags`.
const WITCH: u16 = 1;
const CAT: u16 = 1 << 1;

/// The number of the game's cat sound variants (`minecraft:cat_sound_variant`), of
/// which a cat draws one.
const CAT_SOUND_VARIANTS: i32 = 2;

fn stairs(facing: &str) -> BlockState {
    with(blocks::SPRUCE_STAIRS, "facing", facing)
}

/// `SwampHutPiece.postProcess`. Returns the piece's state: where it settled, and
/// whether its witch and cat were made.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    state: PieceState,
) -> PieceState {
    let Some(mut state) = settle_on_average(placing, piece.bounding_box, state, 0) else {
        return state;
    };
    let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
    let p = placing;
    let planks = blocks::SPRUCE_PLANKS;
    let log = blocks::OAK_LOG;
    let fence = blocks::OAK_FENCE;
    let air = blocks::AIR;

    at.fill(p, [1, 1, 1], [5, 1, 7], planks);
    at.fill(p, [1, 4, 2], [5, 4, 7], planks);
    at.fill(p, [2, 1, 0], [4, 1, 0], planks);
    at.fill(p, [2, 2, 2], [3, 3, 2], planks);
    at.fill(p, [1, 2, 3], [1, 3, 6], planks);
    at.fill(p, [5, 2, 3], [5, 3, 6], planks);
    at.fill(p, [2, 2, 7], [4, 3, 7], planks);
    at.fill(p, [1, 0, 2], [1, 3, 2], log);
    at.fill(p, [5, 0, 2], [5, 3, 2], log);
    at.fill(p, [1, 0, 7], [1, 3, 7], log);
    at.fill(p, [5, 0, 7], [5, 3, 7], log);
    at.place_block(p, fence, 2, 3, 2);
    at.place_block(p, fence, 3, 3, 7);
    at.place_block(p, air, 1, 3, 4);
    at.place_block(p, air, 5, 3, 4);
    at.place_block(p, air, 5, 3, 5);
    at.place_block(p, blocks::POTTED_RED_MUSHROOM, 1, 3, 5);
    at.place_block(p, blocks::CRAFTING_TABLE, 3, 2, 6);
    at.place_block(p, blocks::CAULDRON, 4, 2, 6);
    at.place_block(p, fence, 1, 2, 1);
    at.place_block(p, fence, 5, 2, 1);

    let north = stairs("north");
    let east = stairs("east");
    let west = stairs("west");
    let south = stairs("south");
    at.fill(p, [0, 4, 1], [6, 4, 1], north);
    at.fill(p, [0, 4, 2], [0, 4, 7], east);
    at.fill(p, [6, 4, 2], [6, 4, 7], west);
    at.fill(p, [0, 4, 8], [6, 4, 8], south);
    at.place_block(p, with(north, "shape", "outer_right"), 0, 4, 1);
    at.place_block(p, with(north, "shape", "outer_left"), 6, 4, 1);
    at.place_block(p, with(south, "shape", "outer_left"), 0, 4, 8);
    at.place_block(p, with(south, "shape", "outer_right"), 6, 4, 8);

    for z in [2, 7] {
        for x in [1, 5] {
            at.fill_column_down(p, log, x, -1, z);
        }
    }

    // `spawnWitch`, then `spawnCat`, both where the witch stands, each kept from
    // despawning and put at the block's bottom centre, facing nowhere.
    let home = at.world_pos(2, 2, 5);
    let position = [
        f64::from(home.x) + 0.5,
        f64::from(home.y),
        f64::from(home.z) + 0.5,
    ];
    let settings = p.settings;
    if state.flags & WITCH == 0 && inside(p.chunk_box, home) {
        state.flags |= WITCH;
        if !p.level.make_mob(
            settings,
            entity_types::WITCH,
            true,
            position,
            Some(STRUCTURE),
        ) {
            finalize_mob_spawn(p);
        }
    }
    if state.flags & CAT == 0 && inside(p.chunk_box, home) {
        state.flags |= CAT;
        if !p
            .level
            .make_mob(settings, entity_types::CAT, true, position, Some(STRUCTURE))
        {
            finalize_mob_spawn(p);
            // `Cat.finalizeSpawn` then picks its variant by the variants' spawn
            // conditions (in a swamp hut the black cat's structure condition outranks
            // the rest, leaving one to choose from) and its sound variant, one draw
            // each.
            region_random(p).next_i32_bounded(1);
            region_random(p).next_i32_bounded(CAT_SOUND_VARIANTS);
        }
    }
    state
}

/// The structure the hut's mobs are made in: its cat is black.
const STRUCTURE: &str = "minecraft:swamp_hut";

#[cfg(test)]
mod tests {
    use clustine_structures::geometry::{BoundingBox, Direction, IVec3};
    use clustine_structures::piece::{ProceduralPieceData, StructurePiecePayload};
    use clustine_structures::swamp_hut::SwampHutPieceData;
    use clustine_world::BlockPos;

    use super::*;
    use crate::structures::scattered::testing::{Run, area, chunk, fresh_region_random, piece};
    use crate::testing::World;

    fn hut(orientation: Direction) -> StructurePiece {
        piece(
            BoundingBox::new(IVec3::new(0, 64, 0), IVec3::new(6, 70, 8)),
            orientation,
            StructurePiecePayload::Procedural(ProceduralPieceData::SwampHut(
                SwampHutPieceData::new(),
            )),
        )
    }

    /// Settled with its floor at y 61, three blocks below where it was made.
    const SETTLED: PieceState = PieceState {
        shift_y: -3,
        settled: true,
        flags: 0,
    };

    #[test]
    fn the_hut_stands_on_stilts_down_to_the_ground_with_its_furniture() {
        let mut run = Run::new(World::flat(blocks::DIRT, 55), 2);
        let piece = hut(Direction::North);
        let state = run.place(area(), |p| place(p, &piece, SETTLED));
        let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
        for (x, z) in [(1, 2), (5, 2), (1, 7), (5, 7)] {
            let foot = at.world_pos(x, 0, z);
            for y in 56..=foot.y {
                assert_eq!(run.get(BlockPos::new(foot.x, y, foot.z)), blocks::OAK_LOG);
            }
        }
        assert_eq!(run.get(at.world_pos(1, 3, 5)), blocks::POTTED_RED_MUSHROOM);
        assert_eq!(run.get(at.world_pos(3, 2, 6)), blocks::CRAFTING_TABLE);
        assert_eq!(run.get(at.world_pos(4, 2, 6)), blocks::CAULDRON);
        // The fences are left to take their shape when the chunk is finished.
        assert!(run.world.world.marks.contains(&at.world_pos(2, 3, 2)));
    }

    #[test]
    fn the_witch_and_the_cat_draw_from_the_region_once_in_the_chunk_of_their_home() {
        let mut run = Run::new(World::flat(blocks::DIRT, 55), 2);
        let piece = hut(Direction::South);
        let (state, after) = run.place(area(), |p| {
            let state = place(p, &piece, SETTLED);
            (state, region_random(p).next_i64())
        });
        assert_eq!(state.flags, WITCH | CAT);
        let mut expected = fresh_region_random();
        for _ in 0..2 {
            expected.next_f64();
            expected.next_f64();
            expected.next_f32();
        }
        expected.next_i32_bounded(1);
        expected.next_i32_bounded(CAT_SOUND_VARIANTS);
        assert_eq!(after, expected.next_i64());

        // Placed again, and in a chunk away from their home, they draw nothing.
        let away = run.place(chunk(-1, 0), |p| {
            let again = place(p, &piece, SETTLED);
            (again, region_random(p).next_i64())
        });
        assert_eq!(away, (SETTLED, fresh_region_random().next_i64()));
        let again = run.place(area(), |p| {
            let again = place(p, &piece, state);
            (again, region_random(p).next_i64())
        });
        assert_eq!(again, (state, fresh_region_random().next_i64()));
    }

    #[test]
    fn the_witch_and_the_black_cat_are_made_once_kept_in_the_chunk_of_their_home() {
        // A maker that makes cats and no witch yet: the witch is drawn for instead.
        let maker = crate::testing::ToyMaker {
            mobs: &[entity_types::CAT],
            draws: false,
        };
        let mut run = Run::new(World::flat(blocks::DIRT, 55), 2);
        let piece = hut(Direction::West);
        let (state, after) = run.place_making(&maker, area(), |p| {
            let state = place(p, &piece, SETTLED);
            (state, region_random(p).next_i64())
        });
        assert_eq!(state.flags, WITCH | CAT);
        let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
        let home = at.world_pos(2, 2, 5);
        let made = &run.world.world.made;
        assert_eq!(made.len(), 1);
        let cat = &made[0];
        assert_eq!(cat.kind, entity_types::CAT as u16);
        assert_eq!(
            cat.position,
            [
                f64::from(home.x) + 0.5,
                f64::from(home.y),
                f64::from(home.z) + 0.5
            ]
        );
        assert_eq!((cat.yaw, cat.pitch), (0.0, 0.0));
        // Kept from despawning, and made in the swamp hut, which makes it black.
        let (structure, persistent) = crate::testing::ToyMaker::handed(cat);
        assert_eq!(structure, STRUCTURE.as_bytes());
        assert!(persistent);
        // The witch's draws alone: the toy cat draws nothing.
        let mut expected = fresh_region_random();
        expected.next_f64();
        expected.next_f64();
        expected.next_f32();
        assert_eq!(after, expected.next_i64());

        // Placed again, or in a chunk away from their home, nothing more is made.
        run.place_making(&maker, area(), |p| place(p, &piece, state));
        run.place_making(&maker, chunk(-1, 0), |p| place(p, &piece, SETTLED));
        assert_eq!(run.world.world.made.len(), 1);
    }

    #[test]
    fn the_same_hut_made_twice_makes_the_same_mobs() {
        let maker = crate::testing::ToyMaker {
            mobs: &[entity_types::CAT, entity_types::WITCH],
            draws: true,
        };
        let made = || {
            let mut run = Run::new(World::flat(blocks::DIRT, 55), 2);
            let piece = hut(Direction::North);
            run.place_making(&maker, area(), |p| place(p, &piece, SETTLED));
            run.world.world.made
        };
        let first = made();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].kind, entity_types::WITCH as u16);
        assert_eq!(first[1].kind, entity_types::CAT as u16);
        assert_ne!(first[0].key, first[1].key, "each its own making");
        assert_eq!(made(), first);
    }
}
