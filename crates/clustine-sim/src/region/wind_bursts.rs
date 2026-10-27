//! The wind charged effect's burst (`WindChargedMobEffect.onMobRemoved`'s explosion;
//! `docs/groundwork/n9-plan.md`, section 5.2): part S1's.
//!
//! Until S1 fills it, nothing bursts.

use super::Region;
use crate::api::TickOutput;
use crate::physics::Vec3;

impl Region {
    /// The burst of wind of `radius` at `at`: what it pushes and the blocks it
    /// triggers.
    #[expect(
        dead_code,
        clippy::unused_self,
        reason = "part S1 fills it and calls it (docs/groundwork/n9-plan.md, section 5.2)"
    )]
    pub(crate) fn wind_burst(&mut self, _at: Vec3, _radius: f32, _output: &mut TickOutput) {}
}
