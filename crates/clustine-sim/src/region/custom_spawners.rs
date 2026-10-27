//! The custom spawners (`docs/groundwork/n9-plan.md`, section 9.7) and patrols (8.6):
//! part S11's.
//!
//! Until S11 fills it, nothing is spawned and a patrol's target is not taken.

use clustine_world::{BlockPos, EntityId};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `LongDistancePatrolGoal`'s word on the region's mob `key`: a mob that can join the
    /// patrol takes `pos` as its patrol target.
    #[expect(
        clippy::unused_self,
        reason = "part S11 fills it (docs/groundwork/n9-plan.md, section 8.6)"
    )]
    pub(crate) fn patrol_target_given(
        &mut self,
        _key: EntityId,
        _pos: BlockPos,
        _output: &mut TickOutput,
    ) {
    }
}
