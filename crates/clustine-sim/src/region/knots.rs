//! The knot of a leash tied to a fence (`LeashFenceKnotEntity`,
//! `docs/groundwork/n5-brief.md`, section 7.6): the hooks the shared parts call for it,
//! which step N5g fills (`docs/groundwork/n5-plan.md`). Until then a knot holds wherever
//! it is and drops nothing, and a hit breaks it as any block-attached entity.

use clustine_world::{ChunkPos, Entity, EntityUse};

use super::Region;
use super::attached::Breaker;
use crate::api::TickOutput;

impl Region {
    /// `LeadItem.useOn`: the player's leashed boats tied to the clicked fence (N5g). Filled by its step; until then it does nothing.
    pub(crate) fn lead_used_on(
        &mut self,
        click: &crate::placement::Click,
        player: clustine_world::PlayerId,
        output: &mut crate::api::TickOutput,
    ) -> bool {
        let _ = (click, player, output);
        false
    }

    /// `LeashFenceKnotEntity.survives()`, a fence at its block: N5g's.
    pub(crate) fn knot_survives(&self, entity: &Entity, position: ChunkPos) -> bool {
        let _ = (entity, position);
        true
    }

    /// `LeashFenceKnotEntity.dropItem`: N5g's.
    pub(crate) fn knot_drop(&mut self, entity: &Entity, breaker: Breaker, output: &mut TickOutput) {
        let _ = (entity, breaker, output);
    }

    /// A use of a knot by another region's player: a hit breaks it; tying and untying
    /// are N5g's.
    pub(crate) fn knot_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        if let EntityUse::Attack { creative, .. } = by {
            self.hurt_attached(entity.key, Breaker::Player { creative }, output);
        }
    }
}
