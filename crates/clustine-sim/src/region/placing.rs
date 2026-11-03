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
        // An eye set in an end portal frame (`EnderEyeItem.useOn`, `eye_of_ender.rs`).
        if self.ender_eye_used_on(click, player, output) {
            return true;
        }
        // TNT lit by a flint and steel or a fire charge (`TntBlock.useItemOn`), then
        // fire either sets (`FlintAndSteelItem.useOn`, `FireChargeItem.useOn`).
        if self.light_tnt(click, player, output) || self.fire_item_used_on(click, player, output) {
            return true;
        }
        // A rocket used on a block flies from it (`FireworkRocketItem.useOn`, `throws.rs`).
        if self.rocket_used_on(click, player, output) {
            return true;
        }
        // Bone meal on what takes it (`BoneMealItem.useOn`).
        if self.bone_meal_used_on(click, player, output) {
            return true;
        }
        // An end crystal on obsidian or bedrock (`EndCrystalItem.useOn`,
        // `end_crystal_item.rs`), which makes its own game event.
        if self.place_end_crystal(click, player, output) {
            // `itemStack.shrink(1)`, put back for a creative player (`materials.rs`).
            self.consume_used(click, player, output);
            return true;
        }
        let spawned = output.events.len();
        let made = self.place_minecart(click, player, output)
            || self.place_armor_stand(click, player, output)
            || self.place_cushion(click, player, output);
        if made {
            // Each item's `itemStack.shrink(1)`, put back for a creative player by
            // `ServerPlayerGameMode.useItemOn` (`materials.rs`).
            self.consume_used(click, player, output);
        }
        // A painting or a frame, which takes a click it does not hang, and an egg use
        // themselves up (`hanging.rs`, `spawn_eggs.rs`).
        let placed = made
            || self.place_hanging(click, player, output)
            || self.use_spawn_egg_on(click, player, output);
        if placed {
            self.entity_placed(player, spawned, output);
            return true;
        }
        self.lead_used_on(click, player, output)
    }

    /// One of the item of `click` used up by `player`, where the region has them and
    /// they have no infinite materials.
    fn consume_used(&mut self, click: &Click, player: PlayerId, output: &mut TickOutput) {
        if let Some(item) = click.held {
            self.consume_item(player, item, 1, output);
        }
    }

    /// `gameEvent(player, ENTITY_PLACE, …)` of the item that placed an entity: at the
    /// entity made since the event `since` of the tick's output, by the player
    /// (`docs/groundwork/n6-brief.md`, section 8.1). One sent across a line as it was
    /// made is heard by nothing here.
    fn entity_placed(&mut self, player: PlayerId, since: usize, output: &TickOutput) {
        let made = output.events[since..]
            .iter()
            .rev()
            .find_map(|event| match event {
                crate::api::RegionEvent::EntitySpawned(state) => Some(state.pose.position),
                _ => None,
            });
        let Some(at) = made else {
            return;
        };
        let source = self
            .players
            .get(&player)
            .map(|own| super::vibrations::Source::of(own.entity_id));
        let context = super::vibrations::Context::of(source, None);
        self.vibrate(
            super::vibrations::Vibration::ENTITY_PLACE,
            [at.x, at.y, at.z],
            context,
        );
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
        // An eye of ender thrown (`EnderEyeItem.use`, `eye_of_ender.rs`).
        if self.ender_eye_used(used, player, output) {
            return true;
        }
        let spawned = output.events.len();
        let boat = self.place_boat(used, player, output);
        if boat && let Some(item) = used.held {
            // `BoatItem.use`'s `itemStack.consume(1, player)` (`materials.rs`).
            self.consume_item(player, item, 1, output);
        }
        // An egg and a bucket of a mob spend themselves (`spawn_eggs.rs`,
        // `water_creatures.rs`).
        let placed = boat
            || self.use_spawn_egg(used, player, output)
            || self.use_mob_bucket(used, player, output);
        if placed {
            self.entity_placed(player, spawned, output);
        }
        placed
    }
}
