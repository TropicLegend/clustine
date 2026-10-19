//! The order of a mob's tick, as the game's (`LivingEntity.tick`, `Mob.baseTick`,
//! `aiStep`, `serverAiStep`; the brief's 2.0.12), with each step in the file of the part
//! it belongs to: the body's in [`super::body`], the mind's and controls' in
//! [`super::ai`], the type's in [`super::types`].
//!
//! A tick borrows the mob alone mutably and the rest of the world as it was when the
//! tick began ([`MobWorld`]); what it does to other entities and blocks it writes into
//! [`MobOutput::local`], which the region applies after the mob's tick and before the
//! next entity's, as the messages of the effects across a line are applied.

use clustine_world::EntityId;

use super::types::MobType;
use super::view::MobWorld;
use super::{Mob, ai, body};
use crate::living::Living;
use crate::region::Random;

/// What a mob's tick does to other things than itself, applied in order after it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MobOutput {
    /// Writes to other entities and blocks, in the order made. None is made yet.
    pub(crate) local: Vec<Local>,
}

/// A write a mob's tick makes to something other than itself. Each part adds what it
/// needs, as the same messages as the effects across a line (`docs/groundwork/
/// n7-plan.md`, section 10, item 12).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Local {}

/// A mob in its tick: its record decoded, its body as the physics have it, its type,
/// what it reads and what it draws from.
#[allow(
    dead_code,
    reason = "what the living body's and the AI's parts (N7b, N7c) read of the mob"
)]
pub(crate) struct MobTick<'w> {
    pub(crate) key: EntityId,
    /// Its entity type, by id.
    pub(crate) kind: u16,
    /// The `lived` its tick began with, which its counters are stamped against.
    pub(crate) lived: u32,
    /// The game's `tickCount`: its ticks since it was last loaded fresh.
    pub(crate) tick_count: u32,
    pub(crate) mob: Mob,
    pub(crate) living: Living,
    pub(crate) mob_type: &'static dyn MobType,
    pub(crate) world: &'w dyn MobWorld,
    /// Its own stream in this tick, on from what it has drawn: every draw of its tick,
    /// the level's on its behalf among them, at their place (the plan's 7.2.8).
    pub(crate) draws: &'w mut Random,
    pub(crate) out: MobOutput,
}

impl MobTick<'_> {
    /// `isEffectiveAi`: its AI runs unless it has none.
    pub(crate) fn effective_ai(&self) -> bool {
        !self.mob.body.flags.no_ai
    }
}

/// The tick of a mob, in the game's order.
pub(crate) fn tick(tick: &mut MobTick<'_>) {
    body::base_tick(tick);
    take_hurts(tick);
    if !tick.living.alive() || tick.mob.body.vitals.died_at.is_some() {
        body::tick_death(tick);
        return;
    }
    // `aiStep`.
    body::start_ai_step(tick);
    if tick.effective_ai() {
        ai::server_ai_step(tick);
    }
    body::jump(tick);
    body::travel(tick);
    take_hurts(tick);
    body::push_entities(tick);
    // After `aiStep`, in `LivingEntity.tick`.
    ai::rotate_body(tick);
}

/// The hurts the body met of itself so far in the tick, taken in the order met
/// (`hurtServer` of each, as the game calls it where it meets it).
fn take_hurts(tick: &mut MobTick<'_>) {
    let hurts = match &mut tick.living.kind {
        crate::living::Kind::Mob { hurts, .. } => std::mem::take(hurts),
        crate::living::Kind::ArmorStand { .. } => return,
    };
    for (damage, amount) in hurts {
        body::hurt_server(tick, damage, amount);
    }
}
