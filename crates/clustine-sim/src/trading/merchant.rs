//! A merchant's rules (`AbstractVillager`, `Villager`'s trading, the wandering trader's;
//! `docs/groundwork/n10-plan.md`, section 8.2). Part T2 fills it; the contract step N10c
//! moved the villager's click here, as it was, so that T2 changes it in its own file.

use clustine_data::items;

use crate::mobs::ai::goals::Items;
use crate::mobs::types::{Interaction, Used, animal, villager};

/// `Villager.mobInteract`: with anything but its egg, a living villager shakes its head
/// (a baby always; an adult while it has no offers) or trades; else `AgeableMob`'s.
pub(crate) fn villager_interact(used: &mut Used<'_>) -> Interaction {
    let egg = used
        .held
        .as_ref()
        .is_some_and(|stack| stack.count > 0 && stack.item == items::VILLAGER_SPAWN_EGG);
    if !egg && !used.mob.body.vitals.dying() {
        villager::set_unhappy(used);
        if animal::is_baby(used.mob) {
            return Interaction::Success;
        }
        return Interaction::Consume;
    }
    animal::mob_interact(used, Items::Of(&[]), None)
}
