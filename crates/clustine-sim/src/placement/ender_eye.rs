//! The eye of ender's rules that read only blocks (`docs/groundwork/p-brief.md`,
//! section 4.3; `docs/groundwork/p-plan.md`, section 19.2 P10b): whether a use in the
//! air aims at an end portal frame, which makes it pass (d-javap `EnderEyeItem.use`
//! 7–49), and the search of the 5 × 5 ring of frames with eyes that opens a portal
//! (`EnderEyeItem.useOn` 121–133, `EndPortalFrameBlock.getOrCreatePortalShape` 131–222,
//! n7k `BlockPattern.find`). What the region does with them is
//! `region/eye_of_ender.rs`.
//!
//! The ring is searched as the game's `BlockPattern.find` searches: every corner cell
//! of the cube `at + (0..5)³` in `BlockPos.betweenClosed`'s order (x fastest, then y,
//! then z), at each the 24 pairs of `forwards` and `up` in `Direction.values()`' order
//! (`up` neither `forwards` nor its opposite), and in each the cells column by column
//! and row by row. A cell the reader cannot read (`None`) fails the key that wants it,
//! as the game's `BlockStatePredicate` says false of the null state of an unloaded
//! chunk; the corners of the ring, `?`, are never read.

use clustine_blocks::data::{relative, set, set_flag};
use clustine_data::{BlockState, Face, blocks};
use clustine_world::BlockPos;

use super::{Blocks, ItemUse};

/// The one aisle of `getOrCreatePortalShape`, its rows from the top: `?` anything, and
/// each other key a frame with its eye that faces as [`facing_of`] says.
const RING: [&str; 5] = ["?vvv?", ">???<", ">???<", ">???<", "?^^^?"];

/// The pattern's size, the largest of its width, height and depth (`BlockPattern.find`).
const SIZE: i32 = 5;

/// The facing a key of [`RING`] asks of its frame (`where(FACING, …)`): `^` south, `>`
/// west, `v` north and `<` east; `None` for `?`.
fn facing_of(key: u8) -> Option<&'static str> {
    match key {
        b'^' => Some("south"),
        b'>' => Some("west"),
        b'v' => Some("north"),
        b'<' => Some("east"),
        _ => None,
    }
}

/// `BlockStatePredicate.forBlock(END_PORTAL_FRAME).where(HAS_EYE, true).where(FACING,
/// facing)`: the frame's two properties are both named, so one state.
fn frame_with_eye(facing: &str) -> BlockState {
    set(
        set_flag(blocks::END_PORTAL_FRAME, "eye", true),
        "facing",
        facing,
    )
}

/// Whether a use of the eye in the air aims at an end portal frame
/// (`getPlayerPOVHitResult(level, player, Fluid.NONE)` meets a block that `is
/// (END_PORTAL_FRAME)`), with or without its eye: the game's `use` then passes, so that
/// a click on a frame that has its eye throws nothing. A ray that reaches a block the
/// caller cannot see first aims at no frame.
pub(crate) fn aims_at_frame(item_use: &ItemUse, blocks: &dyn Blocks) -> bool {
    let Ok(Some(pos)) = super::bucket::block_hit(item_use, blocks) else {
        return false;
    };
    blocks.block(pos).is_some_and(|state| {
        clustine_blocks::data::is_block(state, clustine_data::blocks::END_PORTAL_FRAME)
    })
}

/// The 24 pairs of `forwards` and `up` that `BlockPattern.find` tries at each corner,
/// in its order.
fn orientations() -> impl Iterator<Item = (Face, Face)> {
    Face::ALL.into_iter().flat_map(|forwards| {
        Face::ALL
            .into_iter()
            .filter(move |up| *up != forwards && *up != forwards.opposite())
            .map(move |up| (forwards, up))
    })
}

/// A side's step as a vector.
fn step(face: Face) -> [i32; 3] {
    let at = relative(BlockPos::new(0, 0, 0), face);
    [at.x, at.y, at.z]
}

/// `BlockPattern.translateAndRotate`: column `x` along `forwards × up`, row `y` down
/// `up`, aisle 0.
fn translate(origin: BlockPos, forwards: Face, up: Face, x: i32, y: i32) -> BlockPos {
    let (f, u) = (step(forwards), step(up));
    let l = [
        f[1] * u[2] - f[2] * u[1],
        f[2] * u[0] - f[0] * u[2],
        f[0] * u[1] - f[1] * u[0],
    ];
    origin.offset(
        -u[0] * y + l[0] * x,
        -u[1] * y + l[1] * x,
        -u[2] * y + l[2] * x,
    )
}

/// `getOrCreatePortalShape().find(level, at)`: the front top left of the first match of
/// the ring (`BlockPatternMatch.getFrontTopLeft`), or `None`.
pub(crate) fn find_portal_ring(
    at: BlockPos,
    block: impl Fn(BlockPos) -> Option<BlockState>,
) -> Option<BlockPos> {
    for index in 0..SIZE * SIZE * SIZE {
        let corner = at.offset(index % SIZE, index / SIZE % SIZE, index / SIZE / SIZE);
        for (forwards, up) in orientations() {
            if ring_matches(corner, forwards, up, &block) {
                return Some(corner);
            }
        }
    }
    None
}

/// `BlockPattern.matches(origin, forwards, up, cache)` of the ring: column by column,
/// then row by row, the first key that fails ends it.
fn ring_matches(
    origin: BlockPos,
    forwards: Face,
    up: Face,
    block: &impl Fn(BlockPos) -> Option<BlockState>,
) -> bool {
    for x in 0..SIZE {
        for (y, row) in RING.iter().enumerate() {
            let key = row.as_bytes()[x as usize];
            let Some(facing) = facing_of(key) else {
                continue;
            };
            let y = i32::try_from(y).expect("five rows");
            let cell = translate(origin, forwards, up, x, y);
            if block(cell) != Some(frame_with_eye(facing)) {
                return false;
            }
        }
    }
    true
}

/// The inner 3 × 3 of a ring found with its front top left at `front_top_left`, in the
/// game's order (`frontTopLeft.offset(-3, 0, -3)`, then `offset(i, 0, j)` for `i` and
/// then `j` from 0 to 2), and its centre, where the portal's opening is heard.
pub(crate) fn portal_inside(front_top_left: BlockPos) -> ([BlockPos; 9], BlockPos) {
    let corner = front_top_left.offset(-3, 0, -3);
    let cells = std::array::from_fn(|index| {
        let (i, j) = (index / 3, index % 3);
        corner.offset(i as i32, 0, j as i32)
    });
    (cells, corner.offset(1, 0, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring of frames with eyes around the 3 × 3 whose lowest corner is `low`, at its
    /// height, each facing inwards as a stronghold's portal room lays them; `eyes` says
    /// which have their eye, by the frame's place.
    fn ring(low: BlockPos, eyes: impl Fn(BlockPos) -> bool) -> Vec<(BlockPos, BlockState)> {
        let mut frames = Vec::new();
        for i in 0..3 {
            frames.push((low.offset(i, 0, -1), "south"));
            frames.push((low.offset(i, 0, 3), "north"));
            frames.push((low.offset(-1, 0, i), "east"));
            frames.push((low.offset(3, 0, i), "west"));
        }
        frames
            .into_iter()
            .map(|(pos, facing)| {
                let state = set(
                    set_flag(blocks::END_PORTAL_FRAME, "eye", eyes(pos)),
                    "facing",
                    facing,
                );
                (pos, state)
            })
            .collect()
    }

    fn reader(frames: Vec<(BlockPos, BlockState)>) -> impl Fn(BlockPos) -> Option<BlockState> {
        move |pos| {
            Some(
                frames
                    .iter()
                    .find(|(at, _)| *at == pos)
                    .map_or(blocks::AIR, |(_, state)| *state),
            )
        }
    }

    /// The pattern's rows lie south to north and its columns east to west, so the
    /// front top left is the ring's south-east corner, and the inside the 3 × 3 from it
    /// three back each way: found from any frame of the ring.
    #[test]
    fn a_full_ring_is_found_from_each_of_its_frames_with_its_south_east_corner() {
        let low = BlockPos::new(10, 30, -7);
        let frames = ring(low, |_| true);
        let read = reader(frames.clone());
        for (pos, _) in &frames {
            assert_eq!(
                find_portal_ring(*pos, &read),
                Some(low.offset(3, 0, 3)),
                "{pos:?}"
            );
        }
        let (cells, centre) = portal_inside(low.offset(3, 0, 3));
        assert_eq!(cells[0], low);
        assert_eq!(cells[1], low.offset(0, 0, 1));
        assert_eq!(cells[3], low.offset(1, 0, 0));
        assert_eq!(cells[8], low.offset(2, 0, 2));
        assert_eq!(centre, low.offset(1, 0, 1));
    }

    /// One frame without its eye, one turned outwards, or one the reader cannot see,
    /// and there is no ring.
    #[test]
    fn a_ring_with_a_frame_missing_its_eye_turned_or_unseen_is_not_found() {
        let low = BlockPos::new(0, 0, 0);
        let missing = low.offset(1, 0, -1);
        let frames = ring(low, |pos| pos != missing);
        assert_eq!(find_portal_ring(low.offset(0, 0, -1), reader(frames)), None);
        let mut turned = ring(low, |_| true);
        turned[0].1 = set(turned[0].1, "facing", "north");
        assert_eq!(find_portal_ring(low.offset(0, 0, -1), reader(turned)), None);
        let full = reader(ring(low, |_| true));
        let unseen = move |pos: BlockPos| (pos != low.offset(3, 0, 1)).then(|| full(pos)).flatten();
        assert_eq!(find_portal_ring(low.offset(0, 0, -1), unseen), None);
    }
}
