//! A dispenser's projectiles (`ProjectileDispenseBehavior`; `docs/groundwork/n9-plan.md`,
//! section 4.9; part S5's): arrows, eggs, snowballs, the potions, the experience bottle
//! and rockets, made in front along the facing at the configuration's power and
//! uncertainty, their draws their own.
//!
//! Until S5 fills it, a projectile's item is dropped as before.

use super::super::Region;
use super::Dispense;
use crate::api::{ItemStack, TickOutput};

impl Region {
    /// `ProjectileDispenseBehavior.execute` of the dispensed stack, a projectile of the
    /// entity type `kind`. What the slot is left with.
    pub(crate) fn dispense_projectile(
        &mut self,
        dispense: &mut Dispense,
        _kind: i32,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        self.default_dispense(dispense, output)
    }
}
