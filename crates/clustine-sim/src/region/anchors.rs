//! Respawn anchors (`RespawnAnchorBlock`; `docs/groundwork/p-brief.md`, section 5.4):
//! charged by glowstone, from a hand or a dispenser, in every dimension; used with a
//! charge, it sets the spawn where anchors work (the Nether) and explodes elsewhere.
//! Family C of `docs/groundwork/p-plan.md`, section 17, builds the use by hand.

use clustine_blocks::data::{number, set};
use clustine_data::{BlockState, Face, sounds};
use clustine_world::{BlockPos, PlayerId};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `RespawnAnchorBlock.charge`: one charge more, with every update, and its sound.
    /// The anchor `state` at `pos` is not full.
    pub(super) fn charge_anchor(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let charge = number(state, "charges").unwrap_or(0);
        self.set_block(
            pos,
            set(state, "charges", &(charge + 1).to_string()),
            output,
        );
        Region::block_sound(pos, sounds::BLOCK_RESPAWN_ANCHOR_CHARGE, 1.0, 1.0, output);
    }

    /// A player's click that the anchor at `pos` took (`placement::anchor::takes`).
    /// Until family C builds it, nothing comes of it.
    pub(super) fn use_anchor(
        &mut self,
        pos: BlockPos,
        face: Face,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        let _ = (pos, face, player, output);
    }
}
