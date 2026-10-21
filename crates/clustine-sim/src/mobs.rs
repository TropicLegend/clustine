//! Living mobs: the record of a mob's body and mind, the order of its tick, and what
//! each type of mob adds (`docs/groundwork/n7-plan.md`, section 11.3; the brief
//! `docs/groundwork/n7-brief.md`, sections 1 to 3).
//!
//! A mob's chunk keeps it as parts ([`clustine_world::Entry`]): the body's, the mind's
//! and its type's own state, each the encoding of the code here that keeps it, under an
//! id that code gives it and keeps across builds ([`part`]). A tick decodes the parts it
//! needs, runs the game's order ([`tick`]) on them, and commits only the parts that
//! changed. The body is [`body`]'s (part N7b of the plan), the mind and the controls
//! [`ai`]'s (N7c), each type's own [`types`]' (N7e and on); this module and [`tick`]
//! hold them together, so that each part is written in files of its own.
//!
//! As the physics are, the arithmetic is the game's to the bit, and nothing here reads
//! anything but what a [`view::MobWorld`] gives.

pub(crate) mod ai;
pub(crate) mod body;
// The brain's engine (N7j1) is the contract its sensors, behaviours and types are
// built on next; until the first type with a brain calls it, only its tests do.
#[allow(dead_code, unused_imports)]
pub(crate) mod brain;
pub(crate) mod part;
pub(crate) mod tick;
pub(crate) mod types;
pub(crate) mod view;

use clustine_world::{EntityData, EntityField, Entry, TypeState};

use part::PartError;

/// A mob as its tick has it: the parts of its record decoded, and its type's state as
/// its type's code reads it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Mob {
    pub(crate) body: body::Body,
    pub(crate) mind: ai::Mind,
    /// Its type's own state, in the encoding of [`types`]' module for its type.
    pub(crate) state: TypeState,
}

impl Mob {
    /// A mob of nothing yet: every part as a fresh body has it, which the code that
    /// makes a mob (a spawn egg, generation) then fills.
    #[allow(dead_code, reason = "what the spawn eggs of N7b make mobs from")]
    pub(crate) fn fresh() -> Self {
        Self {
            body: body::Body::default(),
            mind: ai::Mind::default(),
            state: TypeState::default(),
        }
    }

    /// The mob a record holds: each part decoded, a part the record lacks as it is
    /// fresh. A part this build cannot read is an error, and the mob is left as it is.
    pub(crate) fn decode(data: &EntityData) -> Result<Self, PartError> {
        let EntityData::Mob { body, mind, state } = data else {
            return Err(PartError::NotAMob);
        };
        Ok(Self {
            body: body::Body::decode(body)?,
            mind: ai::Mind::decode(mind)?,
            state: state.clone(),
        })
    }

    /// The record of the mob.
    pub(crate) fn encode(&self) -> EntityData {
        EntityData::Mob {
            body: self.body.encode(),
            mind: self.mind.encode(),
            state: self.state.clone(),
        }
    }

    /// What changed of its record from `before`: a field for each part set or gone, and
    /// its type's state where it changed; nothing for what did not change.
    pub(crate) fn changes_from(&self, before: &EntityData) -> Vec<EntityField> {
        let EntityData::Mob { body, mind, state } = before else {
            return vec![EntityField::Data(self.encode())];
        };
        let mut fields = Vec::new();
        for (entries, after, set, gone) in [
            (
                body,
                self.body.encode(),
                EntityField::BodyEntry as fn(Entry) -> EntityField,
                EntityField::BodyGone as fn(u16) -> EntityField,
            ),
            (
                mind,
                self.mind.encode(),
                EntityField::MindEntry as fn(Entry) -> EntityField,
                EntityField::MindGone as fn(u16) -> EntityField,
            ),
        ] {
            part::diff(entries, &after, &mut fields, set, gone);
        }
        if *state != self.state {
            fields.push(EntityField::TypeState(self.state.clone()));
        }
        fields
    }
}

#[cfg(test)]
mod tests;
