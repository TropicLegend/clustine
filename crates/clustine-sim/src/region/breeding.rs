//! Breeding's protocol (`docs/groundwork/n7-plan.md`, sections 8.5, 9.4 and 10.7): an
//! animal in love that has found its partner asks it to breed, by a write of its tick
//! (`Local::BreedAsk`). The partner's region breeds, as the game's `this` of `breed`, if
//! the partner can, and answers; an accepted answer gives the asker its part (its age
//! and its love reset). Where the partner is the region's own, the request and the answer
//! are taken at once; where only a copy shows it, they go as effects on the entity
//! across the line (`Effected::OnEntity`), and the answer comes back the same way.
//!
//! The decisions are the animals' (`crate::mobs::types::animal`): only the higher key
//! asks across a line, an asker is marked asked until it is answered or 40 ticks pass,
//! an asked animal counts as not in love for others and refuses every request. Across a
//! line the partner breeds as the game's `this`: the child, the orb and the hearts are
//! made at the partner, from its stream. Within one region the asker is `this`, as the
//! first of the two whose goal reached its count, which the game's order makes the
//! breeding one.
//!
//! Named differences: the child of a breeding across a line appears at the partner;
//! the asker's love is taken as it was when it asked; an answer later than 40 ticks
//! lets the asker breed again in between; the parents' cooldowns start a tick after
//! the game's, the request being carried out after the asker's tick.

use clustine_world::{ChunkPos, Effect, Effected, Entity, EntityId, EntityUse};

use super::Region;
use super::uses::block_in;
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::types::{self, Breeding, animal};
use crate::mobs::{Mob, ai};

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

    /// `partner` breeds with `asker` if it can (`Animal.canMate`: of one type, both in
    /// love, the partner not waiting on an answer of its own nor panicking), then
    /// `spawnChildFromBreeding` of `this`: the asker where the region holds it, else the
    /// partner. Whether it did.
    pub(crate) fn breed_with(
        &mut self,
        partner: EntityId,
        asker: EntityId,
        output: &mut TickOutput,
    ) -> bool {
        let Some((partner_chunk, partner_record)) = self.own_record(partner) else {
            return false;
        };
        let Some(partner_mob) = breeder(&partner_record) else {
            return false;
        };
        let partner_animal = animal::animal_of(&partner_mob.state);
        if !partner_animal.in_love_for_others(partner_record.lived)
            || ai::panicking_record(&partner_record)
        {
            return false;
        }
        if let Some((asker_chunk, asker_record)) = self.own_record(asker) {
            let Some(asker_mob) = breeder(&asker_record) else {
                return false;
            };
            let asker_animal = animal::animal_of(&asker_mob.state);
            if asker_record.kind != partner_record.kind || !asker_animal.in_love(asker_record.lived)
            {
                return false;
            }
            let _ = (asker_chunk, partner_chunk);
            return self.spawn_child_from_breeding(
                &asker_record,
                &asker_mob,
                partner,
                &partner_mob,
                output,
            );
        }
        // Across a line: the asker as its copy shows it, its love as of its request.
        let Some((_, asker_record)) = self.copied_entity(asker) else {
            return false;
        };
        if asker_record.kind != partner_record.kind {
            return false;
        }
        let Some(asker_mob) = breeder(&asker_record) else {
            return false;
        };
        self.spawn_child_from_breeding(&partner_record, &partner_mob, asker, &asker_mob, output)
    }

    /// `Animal.spawnChildFromBreeding(level, partner)` of the region's animal `this`
    /// with `partner` (the region's own, or another's that only a copy shows): the child
    /// its type makes of the two from its stream, a baby at its feet facing south; then
    /// `finalizeSpawnChildFromBreeding`: both cooldowns of 6000 and no more love (the
    /// partner's here where the region holds it, else by the answer), the hearts at
    /// `this`, and an orb of one to seven; the child added last. Whether a child came.
    fn spawn_child_from_breeding(
        &mut self,
        this: &Entity,
        this_mob: &Mob,
        partner: EntityId,
        partner_mob: &Mob,
        output: &mut TickOutput,
    ) -> bool {
        let mut stream = self.entity_stream(this.key).random;
        let start = stream.clone();
        if let Some(state) = types::of(this.kind).bred_without_child(this_mob) {
            // A turtle's breeding (`TurtleBreedGoal.breed`): an egg for `this`, the
            // cooldowns and the love as ever, an orb, and no child nor hearts.
            let value = stream.next_int(7) as i32 + 1;
            self.entity_drew(this.key, stream.draws_since(&start));
            if let Some((chunk, record)) = self.own_record(this.key)
                && let Ok(mut mob) = Mob::decode(&record.data)
            {
                mob.state = state;
                let now = record.lived;
                animal::update(&mut mob, |shared| shared.bred(now));
                self.commit_mob(&record, chunk, &mob, output);
            }
            if self.find_entity(partner).is_some() {
                self.update_animal(partner, |shared, now| shared.bred(now), output);
            }
            self.add_orb(this.position, value, output);
            return true;
        }
        let biome = self.biome_at(crate::physics::containing(this.position));
        let child = types::of(this.kind).breed_offspring(&mut Breeding {
            this: this_mob,
            partner: partner_mob,
            draws: &mut stream,
            biome,
        });
        let Some(mut child) = child else {
            self.entity_drew(this.key, stream.draws_since(&start));
            return false;
        };
        animal::made_baby(&mut child);
        // `getRandom().nextInt(7) + 1`, the parent's own.
        let value = stream.next_int(7) as i32 + 1;
        self.entity_drew(this.key, stream.draws_since(&start));
        self.update_animal(this.key, |shared, now| shared.bred(now), output);
        if self.find_entity(partner).is_some() {
            self.update_animal(partner, |shared, now| shared.bred(now), output);
        }
        if let Some((chunk, _)) = self.find_entity(this.key) {
            output.events.push(RegionEvent::EntityEvent {
                entity: this.key,
                chunk,
                event: 18,
            });
        }
        self.add_orb(this.position, value, output);
        if let Some(made) = self.new_mob(this.kind, child, this.position, 0.0, 0.0) {
            self.add_made_mob(made, output);
        }
        true
    }

    /// The region's animal `asker` hears the answer to its request `number` to
    /// `partner`: it ends its mark, and where accepted takes its part of the breeding.
    /// An answer to a request it no longer waits on changes nothing.
    pub(crate) fn bred(
        &mut self,
        asker: EntityId,
        partner: EntityId,
        number: u32,
        accepted: bool,
        output: &mut TickOutput,
    ) {
        self.update_animal(
            asker,
            |shared, now| {
                let answered = shared
                    .asked
                    .is_some_and(|asked| asked.number == number && asked.partner == partner);
                if !answered {
                    return;
                }
                shared.asked = None;
                if accepted {
                    shared.bred(now);
                }
            },
            output,
        );
    }

    /// The chunk and record of the region's own entity `key`.
    fn own_record(&self, key: EntityId) -> Option<(ChunkPos, Entity)> {
        self.find_entity(key)
            .map(|(chunk, entity)| (chunk, entity.clone()))
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

/// The mob of a farm animal's record that is alive to breed, if it is one.
fn breeder(entity: &Entity) -> Option<Mob> {
    animal::of_record(entity)?;
    let mob = Mob::decode(&entity.data).ok()?;
    (!mob.body.vitals.dying()).then_some(mob)
}
