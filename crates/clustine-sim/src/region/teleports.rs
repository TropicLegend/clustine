//! `randomTeleport` and what comes of a teleport (`docs/groundwork/n9-plan.md`, sections
//! 4.4 and 9; part S9's): the enderman's, the chorus fruit's, and the endermite of an
//! ender pearl.
//!
//! Until S9 fills it, no endermite comes of a pearl.

use clustine_world::PlayerId;

use super::{Random, Region};
use crate::api::TickOutput;

impl Region {
    /// The endermite an ender pearl of the player `id` made, at their feet and with their
    /// look, before they are moved (`ThrownEnderpearl.onHit`).
    #[expect(
        clippy::unused_self,
        reason = "part S9 fills it (docs/groundwork/n9-plan.md, section 4.4)"
    )]
    pub(crate) fn pearl_endermite(&mut self, _id: PlayerId, _output: &mut TickOutput) {}

    /// The chorus fruit's `teleport_randomly` of the player `id`
    /// (`TeleportRandomlyConsumeEffect.apply`): up to 16 tries of `randomTeleport` within
    /// `diameter` of their feet, each try's draws from `random` (the player's stream
    /// aside), the first that lands moving them with the teleport's sound and the
    /// player's fall reset. Whether they were moved. Part S9's; S1's eating calls it.
    #[expect(
        dead_code,
        clippy::unused_self,
        reason = "part S9 fills it and S1 calls it (docs/groundwork/n9-plan.md, sections 5.5 and 8.4)"
    )]
    pub(crate) fn chorus_teleport(
        &mut self,
        _id: PlayerId,
        _diameter: f64,
        _random: &mut Random,
        _output: &mut TickOutput,
    ) -> bool {
        false
    }
}
