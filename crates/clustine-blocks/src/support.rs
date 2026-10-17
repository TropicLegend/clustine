//! Blocks that go when what holds them goes: the part of each class's `updateShape`
//! that, told of a change in the one direction its support lies in, gives air where
//! the block can no longer stay. Read from the 26.3 jar's classes.
//!
//! What the classes check first, before anything else of their `updateShape` (a sign or
//! a ladder schedules its water only where it stays):
//!
//! - down: standing torches of every kind (`BaseTorchBlock`), standing signs, banners
//!   and pressure plates;
//! - the wall behind, which the block faces away from: wall torches, ladders, wall
//!   signs, wall banners and tripwire hooks;
//! - up: hanging signs under a ceiling;
//! - the direction it hangs or stands by: buttons, levers (`getConnectedDirection` of
//!   `FaceAttachedHorizontalDirectionalBlock`) and lanterns.
//!
//! Candles, rails and repeaters have no such check in their `updateShape`: rails and
//! repeaters go by `neighborChanged`, and candles stay. A wall hanging sign checks the
//! sides along its board, by a `canSurvive` that is not ported, and stays.

use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::data::{face_named, is_class};
use crate::survive::can_survive;

/// What `state` becomes when its neighbour towards `direction` changed, if it is a
/// block that then goes for want of support: air, or `None` where this rule says
/// nothing (the block stays, or its support is not that way).
pub fn without_support<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> Option<BlockState> {
    let support = support_direction(state)?;
    (support == direction && !can_survive(state, pos, view)).then_some(blocks::AIR)
}

/// The direction a block of these classes rests on.
fn support_direction(state: BlockState) -> Option<Face> {
    use BlockClass as C;
    let facing = || state.property("facing").and_then(face_named);
    let class = |of| is_class(state, of);
    if class(C::WallTorchBlock)
        || class(C::RedstoneWallTorchBlock)
        || class(C::LadderBlock)
        || class(C::WallSignBlock)
        || class(C::WallBannerBlock)
        || class(C::TripWireHookBlock)
    {
        return facing().map(opposite);
    }
    if class(C::BaseTorchBlock)
        || class(C::StandingSignBlock)
        || class(C::BannerBlock)
        || class(C::BasePressurePlateBlock)
    {
        return Some(Face::Down);
    }
    if class(C::CeilingHangingSignBlock) {
        return Some(Face::Up);
    }
    if class(C::LanternBlock) {
        // `getConnectedDirection`: down for one that hangs, up for one that stands;
        // the support is the other way.
        return Some(if state.property("hanging") == Some("true") {
            Face::Up
        } else {
            Face::Down
        });
    }
    if class(C::FaceAttachedHorizontalDirectionalBlock) {
        // `getConnectedDirection`: down on a ceiling, up on a floor, the facing on a
        // wall; the support is the other way.
        return match state.property("face") {
            Some("ceiling") => Some(Face::Up),
            Some("floor") => Some(Face::Down),
            _ => facing().map(opposite),
        };
    }
    None
}

fn opposite(face: Face) -> Face {
    match face {
        Face::Down => Face::Up,
        Face::Up => Face::Down,
        Face::North => Face::South,
        Face::South => Face::North,
        Face::West => Face::East,
        Face::East => Face::West,
    }
}
