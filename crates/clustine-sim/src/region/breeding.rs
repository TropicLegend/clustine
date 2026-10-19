//! Breeding's protocol (`docs/groundwork/n7-plan.md`, sections 8.5, 9.4 and 10.7): an
//! animal in love that has found its partner asks it to breed, by a write of its tick
//! (`Local::BreedAsk`). The partner's region breeds, as the game's `this` of `breed`, if
//! the partner can, and answers; an accepted answer gives the asker its part (its age
//! and its love reset). Where the partner is the region's own, the request and the answer
//! are taken at once; where only a copy shows it, they go as effects on the entity
//! across the line (`Effected::OnEntity`), and the answer comes back the same way.
//!
//! The decisions are the animals' (step N7e fills [`Region::breed_with`] and
//! [`Region::bred`]): only the higher key asks across a line, an asker is marked asked
//! until it is answered or 40 ticks pass, an asked animal counts as not in love for
//! others and refuses every request, and the child, the orb and the hearts are made at
//! the partner. Until then every request is refused.

use clustine_world::{Effect, Effected, EntityId, EntityUse};

use super::Region;
use super::uses::block_in;
use crate::api::TickOutput;

impl Region {
    /// The region's animal `asker` asks `partner` to breed, its request `number`.
    pub(crate) fn ask_to_breed(
        &mut self,
        asker: EntityId,
        partner: EntityId,
        number: u32,
        output: &mut TickOutput,
    ) {
        if self.find_entity(partner).is_some() {
            self.breed_asked(partner, asker, number, output);
            return;
        }
        if self.send_on_entity(partner, EntityUse::BreedAsk { asker, number }) {
            return;
        }
        // Nobody to ask: refused at once, so that the asker is free again.
        self.bred(asker, partner, number, false, output);
    }

    /// The region's animal `partner` is asked by `asker`: it breeds if it can, and the
    /// asker is answered.
    pub(crate) fn breed_asked(
        &mut self,
        partner: EntityId,
        asker: EntityId,
        number: u32,
        output: &mut TickOutput,
    ) {
        let accepted = self.breed_with(partner, asker, output);
        if self.find_entity(asker).is_some() {
            self.bred(asker, partner, number, accepted, output);
            return;
        }
        let answer = EntityUse::BreedAnswer {
            partner,
            number,
            accepted,
        };
        // An asker beyond the copies hears nothing, and asks again once its mark ends.
        self.send_on_entity(asker, answer);
    }

    /// `partner` breeds with `asker` if it can (`Animal.canMate`, then `spawnChildFromBreeding`
    /// at the partner): whether it did. The animals' (N7e); nothing breeds yet.
    pub(crate) fn breed_with(
        &mut self,
        partner: EntityId,
        asker: EntityId,
        output: &mut TickOutput,
    ) -> bool {
        let _ = (partner, asker, output);
        false
    }

    /// The region's animal `asker` hears the answer to its request `number` to
    /// `partner`: it ends its mark, and where accepted takes its part of the breeding.
    /// The animals' (N7e).
    pub(crate) fn bred(
        &mut self,
        asker: EntityId,
        partner: EntityId,
        number: u32,
        accepted: bool,
        output: &mut TickOutput,
    ) {
        let _ = (asker, partner, number, accepted, output);
    }

    /// Sends `by` to the entity `key` that a copy of the region shows, as an effect at
    /// its block: whether it was sent.
    fn send_on_entity(&mut self, key: EntityId, by: EntityUse) -> bool {
        let Some((chunk, entity)) = self.copied_entity(key) else {
            return false;
        };
        if !self.sends_to(chunk) {
            return false;
        }
        self.made.push(Effect {
            at: block_in(chunk, &entity),
            what: Effected::OnEntity { key, by },
        });
        true
    }
}
