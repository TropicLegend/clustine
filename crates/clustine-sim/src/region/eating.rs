//! A player's eating and drinking (`docs/groundwork/n9-plan.md`, section 5.5; part S1's):
//! the mouthfuls while the use lasts (`Consumable.emitParticlesAndSounds`), and what the
//! meal or the drink does at its end (`Consumable.onConsume`: its listeners, then every
//! `on_consume_effects` entry of the data), with what it leaves in the hand. The use
//! itself is `using.rs`'.
//!
//! Until S1 fills it, a meal ends with nothing done.

use clustine_world::PlayerId;

use super::Region;
use crate::api::{TickOutput, Using};

impl Region {
    /// `Consumable`'s part of `onUseTick` of the player `id`'s meal: crumbs and a
    /// mouthful's sound where `shouldEmitParticlesAndSounds` holds.
    #[expect(
        clippy::unused_self,
        reason = "part S1 fills it (docs/groundwork/n9-plan.md, section 5.5)"
    )]
    pub(crate) fn consume_tick(&mut self, _id: PlayerId, _using: Using, _output: &mut TickOutput) {}

    /// `finishUsingItem` of the player `id`'s meal: the last mouthful, `onConsume`, and
    /// what is left in the hand (nothing consumed in creative).
    #[expect(
        clippy::unused_self,
        reason = "part S1 fills it (docs/groundwork/n9-plan.md, section 5.5)"
    )]
    pub(crate) fn finish_consuming(
        &mut self,
        _id: PlayerId,
        _using: Using,
        _output: &mut TickOutput,
    ) {
    }
}
