//! Trader llamas (`TraderLlama`, a `Llama`; `docs/groundwork/n7-brief.md`, section 5.16):
//! a llama that panics faster, takes zombies and illagers as its targets, and leaves the
//! world when its 47,999 ticks run out, while nothing keeps it (taming, a lead, a player
//! on it, the age lock, persistence). A foal of a trader llama is a persistent trader
//! llama.
//!
//! Named differences: wandering traders are N10's, so no trader llama is leashed to one,
//! defends one or takes its despawn delay; the goal of defending one is registered and
//! never runs, so that its place among the goals stays the game's.

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, ItemStack};

use super::equine::{self, Equine};
use super::horse::common;
use super::llama::{self, common_llama};
use super::{Breeding, Dimensions, Interaction, Mate, MobType, Spawning, Used, animal};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{NearestAttackableTargetGoal, PanicGoal, Targeting};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{Class, Looked};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::Removed;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// `DEFAULT_DESPAWN_DELAY`.
pub(crate) const DESPAWN_DELAY: i32 = 47_999;

pub(crate) const EQUINE: Equine = Equine {
    kind: entity_types::TRADER_LLAMA,
    ..llama::EQUINE
};

/// The zombies' class (`Zombie`): the zombie, the husk, the drowned, the zombie villager
/// and the zombified piglin, which the trader llama's selector leaves out.
const ZOMBIES: [u16; 5] = [
    entity_types::ZOMBIE as u16,
    entity_types::HUSK as u16,
    entity_types::DROWNED as u16,
    entity_types::ZOMBIE_VILLAGER as u16,
    entity_types::ZOMBIFIED_PIGLIN as u16,
];

/// The illagers' class (`AbstractIllager`).
const ILLAGERS: [u16; 4] = [
    entity_types::PILLAGER as u16,
    entity_types::VINDICATOR as u16,
    entity_types::EVOKER as u16,
    entity_types::ILLUSIONER as u16,
];

/// Not a zombified piglin.
fn not_zombified_piglin(looked: &Looked) -> bool {
    looked.kind != Some(entity_types::ZOMBIFIED_PIGLIN as u16)
}

/// A trader llama's partner class: its own.
fn trader_llama(kind: u16) -> bool {
    i32::from(kind) == entity_types::TRADER_LLAMA
}

/// `TraderLlama$TraderLlamaDefendWanderingTraderGoal`, which needs a wandering trader
/// leashing it: none is (N10), so it never runs.
pub(crate) struct DefendWanderingTraderGoal;

impl Goal for DefendWanderingTraderGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, _: &mut Targeting, _: &mut Ai<'_, '_>) -> bool {
        false
    }
}

/// The trader llama.
pub(crate) struct TraderLlama;

/// `maybeDespawn()` after the llama's `aiStep`: while despawnable its delay runs down
/// by one a tick, and at nothing it leaves the world.
fn maybe_despawn(tick: &mut MobTick<'_>) {
    let now = tick.lived;
    let (shared, horse) = equine::read(&tick.mob);
    let one_player = tick
        .world
        .entity(tick.key)
        .is_some_and(|me| matches!(&me.entity.riding.passengers[..], [seat] if seat.player));
    let despawnable = !horse.tame
        && !tick.leashed
        && !one_player
        && !shared.age_locked
        && !tick.mob.body.flags.persistent;
    let mut gone = false;
    equine::update(&mut tick.mob, |horse| {
        if despawnable {
            let since = *horse.despawn_since.get_or_insert(now);
            let counted = (i64::from(now) - i64::from(since) + 1).max(0);
            let spent = i64::from(horse.despawn_spent) + counted;
            gone = i64::from(DESPAWN_DELAY) - spent <= 0;
        } else if let Some(since) = horse.despawn_since.take() {
            let counted = (i64::from(now) - i64::from(since)).max(0);
            horse.despawn_spent =
                (i64::from(horse.despawn_spent) + counted).min(i64::from(DESPAWN_DELAY)) as i32;
        }
    });
    if gone {
        tick.out.removed = Some(Removed::Discarded);
    }
}

impl MobType for TraderLlama {
    /// The llama's goals, then a faster panic, and the trader's target goals.
    fn register_goals(&self, setup: &mut Setup) {
        llama::register_goals(setup, &EQUINE, trader_llama);
        setup.add_goal(1, PanicGoal { speed: 2.0 });
        setup.add_target(1, DefendWanderingTraderGoal);
        setup.add_target(
            2,
            NearestAttackableTargetGoal {
                selector: Some(not_zombified_piglin),
                ..NearestAttackableTargetGoal::new(Class::Kinds(&ZOMBIES), true)
            },
        );
        setup.add_target(
            2,
            NearestAttackableTargetGoal::new(Class::Kinds(&ILLAGERS), true),
        );
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        common::walk_target_value(ai, pos)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        equine::before_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step_growing(tick, EQUINE.grows);
        equine::ai_step(tick, &EQUINE);
        maybe_despawn(tick);
        if tick.out.removed.is_some() {
            return;
        }
        equine::tick_end(tick);
    }

    fn immobile(&self, tick: &MobTick<'_>) -> bool {
        equine::immobile(tick, &EQUINE)
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        equine::mob_interact(used, &EQUINE, true)
    }

    /// `TraderLlama.finalizeSpawn`: a group that wants no babies where it has none,
    /// which the llama's then takes for no herd of its own and replaces with one.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        if spawn.group.is_none() {
            *spawn.group = Some(super::SpawnGroup::Ageable(super::AgeableGroup::new(false)));
        }
        llama::finalize_spawn(spawn, entity_types::TRADER_LLAMA as u16);
    }

    /// `makeNewLlama()`: a trader llama, persistent.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let mut child = llama::breed_offspring(breeding, entity_types::TRADER_LLAMA as u16);
        child.body.flags.persistent = true;
        Some(child)
    }

    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        equine::llama_mates(this, partner)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        equine::hurt_taken(mob, entity_types::TRADER_LLAMA as u16, draws, moment);
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        equine::drop_equipment(mob)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        equine::dimensions(mob, &EQUINE)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_LLAMA_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_LLAMA_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_LLAMA_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.8
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        common_llama::shown(mob)
    }
}
