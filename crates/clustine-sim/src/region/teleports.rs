//! `randomTeleport` and what comes of a teleport (`docs/groundwork/n9-plan.md`, sections
//! 4.4 and 9; part S9's): the enderman's, the chorus fruit's, and the endermite of an
//! ender pearl.
//!
//! Until S9 fills it, no endermite comes of a pearl.

use clustine_world::PlayerId;

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// The endermite an ender pearl of the player `id` made, at their feet and with their
    /// look, before they are moved (`ThrownEnderpearl.onHit`).
    #[expect(
        clippy::unused_self,
        reason = "part S9 fills it (docs/groundwork/n9-plan.md, section 4.4)"
    )]
    pub(crate) fn pearl_endermite(&mut self, _id: PlayerId, _output: &mut TickOutput) {}
}
