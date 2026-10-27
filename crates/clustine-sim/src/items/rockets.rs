//! A firework rocket used on a block (`FireworkRocketItem.useOn`;
//! `docs/groundwork/n9-plan.md`, section 4.7; part S4's): the rocket made at the clicked
//! point moved 0.15 along the face (`Region::make_firework`).
//!
//! `useOn` (n9 FireworkRocketItem 14–84): nothing for a player who glides (none does in
//! Clustine); on the server `new FireworkRocketEntity(level, player, x, y, z, stack)` at
//! the clicked point plus 0.15 of the face's step, added by `spawnProjectile` without a
//! shot, so it flies up as one launched by hand; the stack shrunk by one, which the
//! game's creative mode puts back. The rocket in the air (`use`) only boosts a glide, so
//! it does nothing here.

use crate::api::Face;
use crate::physics::Vec3;

/// `ROCKET_PLACEMENT_OFFSET`.
pub(crate) const PLACEMENT_OFFSET: f64 = 0.15;

/// Where a rocket used on the face `face` at the clicked point `clicked` starts.
pub(crate) fn placed_at(clicked: Vec3, face: Face) -> Vec3 {
    let step = match face {
        Face::Bottom => [0, -1, 0],
        Face::Top => [0, 1, 0],
        Face::North => [0, 0, -1],
        Face::South => [0, 0, 1],
        Face::West => [-1, 0, 0],
        Face::East => [1, 0, 0],
    };
    [
        clicked[0] + f64::from(step[0]) * PLACEMENT_OFFSET,
        clicked[1] + f64::from(step[1]) * PLACEMENT_OFFSET,
        clicked[2] + f64::from(step[2]) * PLACEMENT_OFFSET,
    ]
}
