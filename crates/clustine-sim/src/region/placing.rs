//! Items that make entities (`docs/groundwork/n5-plan.md`, section 7, item 10): used on a
//! block, a minecart's on a rail, an armour stand's, a frame's, a painting's, a cushion's
//! and a lead on a fence (`MinecartItem`, `ArmorStandItem`, `ItemFrameItem` and
//! `HangingEntityItem`, `CushionItem`, `LeadItem`), and used in the air, a boat's
//! (`BoatItem`). The game asks the item only once the clicked block has done nothing
//! with the click, as here. Each kind's step fills its own function; one that is not its
//! item answers that it did nothing.

use clustine_world::PlayerId;

use super::Region;
use crate::api::TickOutput;
use crate::placement::{Click, ItemUse};

impl Region {
    /// `Item.useOn` of the held item on the clicked block of `click`, which the region
    /// holds, by the player `player`: whether an entity's item did something with it.
    pub(crate) fn entity_item_used_on(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if click.held.is_none() {
            return false;
        }
        self.place_minecart(click, player, output)
            || self.place_armor_stand(click, player, output)
            || self.place_hanging(click, player, output)
            || self.place_cushion(click, player, output)
            || self.lead_used_on(click, player, output)
    }

    /// `Item.use` of the held item in the air by the player `player`: whether an entity's
    /// item did something with it (a boat placed by its ray).
    pub(crate) fn entity_item_used(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if used.held.is_none() {
            return false;
        }
        self.place_boat(used, player, output)
    }
}
