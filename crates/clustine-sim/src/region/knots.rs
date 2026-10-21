//! The knot of a leash tied to a fence (`LeashFenceKnotEntity`, `LeadItem.useOn` and
//! `FenceBlock.useWithoutItem`; `docs/groundwork/n5-brief.md`, sections 0.36 and 7.6;
//! step N5g of `docs/groundwork/n5-plan.md`): a lead used on a fence, or a fence used
//! with anything in the main hand or nothing, ties the player's leashed boats to the
//! knot there (`leashes.rs`); a knot holds while its block is a fence, at its check
//! every 101 ticks; it breaks to one hit of anything, playing its sound and dropping
//! nothing, and its leashees each drop their own lead on their next tick, whoever broke
//! it. What a knot is used for is `LeashFenceKnotEntity.interact`.

use clustine_data::{items, sounds};
use clustine_world::{ChunkPos, Entity, EntityData, EntityUse, ItemStack};

use super::Region;
use super::attached::Breaker;
use crate::api::TickOutput;

/// `getPickResult()`: a knot gives a lead.
pub(crate) fn knot_pick(entity: &Entity) -> Option<ItemStack> {
    matches!(entity.data, EntityData::LeashKnot { .. }).then_some(ItemStack {
        item: items::LEAD,
        count: 1,
    })
}

impl Region {
    /// `LeadItem.useOn`: a lead used on a fence binds the player's leashed boats to it.
    /// Whether it bound any.
    pub(crate) fn lead_used_on(
        &mut self,
        click: &crate::placement::Click,
        player: clustine_world::PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if click.held != Some(items::LEAD) {
            return false;
        }
        if !self.block(click.position).is_some_and(Region::is_fence) {
            return false;
        }
        self.bind_player_mobs(player, click.position, output)
    }

    /// `FenceBlock.useWithoutItem`, which the game asks of a fence clicked with the main
    /// hand, with any item or none, before the item (`ServerPlayerGameMode.useItemOn`,
    /// the block's `useItemOn` answering `TRY_WITH_EMPTY_HAND`), unless the player
    /// sneaks with something in hand: the player's leashed boats are bound to it.
    /// Whether that took the click.
    pub(crate) fn fence_used(
        &mut self,
        click: &crate::placement::Click,
        player: clustine_world::PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if click.sneaking && click.held.is_some() {
            return false;
        }
        if !self.block(click.position).is_some_and(Region::is_fence) {
            return false;
        }
        self.bind_player_mobs(player, click.position, output)
    }

    /// `LeashFenceKnotEntity.survives()`: its block is a fence.
    pub(crate) fn knot_survives(&self, entity: &Entity, position: ChunkPos) -> bool {
        let _ = position;
        let EntityData::LeashKnot { attached } = entity.data else {
            return true;
        };
        self.block(attached.block).is_some_and(Region::is_fence)
    }

    /// `LeashFenceKnotEntity.dropItem`: only the sound of a lead untied, whoever broke
    /// it; its leashees drop their leads themselves.
    pub(crate) fn knot_drop(&mut self, entity: &Entity, breaker: Breaker, output: &mut TickOutput) {
        let _ = breaker;
        self.leash_sound(sounds::ITEM_LEAD_UNTIED, entity.position, output);
    }

    /// A use of a knot by a player of this region or another: a hit breaks it; a click
    /// ties and unties as `LeashFenceKnotEntity.interact` does (`leashes.rs`).
    pub(crate) fn knot_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        match by {
            EntityUse::Attack { creative, .. } => {
                self.hurt_attached(entity.key, Breaker::Player { creative }, output);
            }
            EntityUse::Interact { .. } => self.knot_interact(entity, by, output),
            // An explosion's hit across a line is step N6g's (`docs/groundwork/n6-plan.md`).
            // Only animals breed.
            EntityUse::Blasted { .. }
            | EntityUse::BreedAsk { .. }
            | EntityUse::MobHit { .. }
            | EntityUse::SeatDeclined { .. }
            | EntityUse::BreedAnswer { .. } => {}
        }
    }
}
