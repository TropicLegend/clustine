//! Trader llamas (`TraderLlama`, a `Llama`; `docs/groundwork/n7-brief.md`, section 5.16):
//! a llama that panics faster, takes zombies and illagers as its targets, and leaves the
//! world when its 47,999 ticks run out, while nothing keeps it (taming, a lead, a player
//! on it, the age lock, persistence). A foal of a trader llama is a persistent trader
//! llama.
//!
//! Led by a wandering trader (the trader spawner leashes two to each trader it makes,
//! `region/custom_spawners.rs`), it defends the trader against what last hurt it
//! (`TraderLlamaDefendWanderingTraderGoal`) and leaves when the trader does, its delay the
//! trader's less one.
//!
//! Named differences: a trader leading it across a line beyond its region's copies is not
//! seen, and the llama counts as led by something else (it neither defends nor leaves);
//! a player may mount one led by a trader (`doPlayerRide` refuses it in the game).

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack, LeashHolder};

use super::equine::{self, Equine};
use super::horse::common;
use super::llama::{self, common_llama};
use super::{
    Breeding, Dimensions, Interaction, Mate, MobType, Spawning, Used, animal, wandering_trader,
};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{NearestAttackableTargetGoal, PanicGoal, TargetRules, Targeting};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions, Looked};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::Removed;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::mobs::view::MobWorld;
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

/// The wandering trader whose leash holds the llama `key` (`getLeashHolder()` being a
/// `WanderingTrader`), where the view has both: its key and its mob.
pub(crate) fn trader_holding(world: &dyn MobWorld, key: EntityId) -> Option<(EntityId, Mob)> {
    let me = world.entity(key)?;
    let LeashHolder::Entity(holder) = me.entity.riding.leash.as_ref()?.holder else {
        return None;
    };
    let trader = world.entity(holder)?;
    if i32::from(trader.entity.kind) != entity_types::WANDERING_TRADER {
        return None;
    }
    Some((holder, Mob::decode(&trader.entity.data).ok()?))
}

/// `TraderLlama$TraderLlamaDefendWanderingTraderGoal(llama)` (`TargetGoal(mob, false)`):
/// a llama led by a wandering trader takes what last hurt the trader as its target, once
/// for each hurt, where it may attack it. No draws.
pub(crate) struct DefendWanderingTraderGoal;

impl DefendWanderingTraderGoal {
    const RULES: TargetRules = TargetRules {
        must_see: false,
        must_reach: false,
    };
}

impl Goal for DefendWanderingTraderGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        if !ai.tick.leashed {
            return false;
        }
        let Some((_, trader)) = trader_holding(ai.tick.world, ai.tick.key) else {
            return false;
        };
        let last = trader.body.combat.last_hurt_by;
        state.found = last.map(|(attacker, _)| attacker);
        if last.map(|(_, at)| at) == state.timestamp {
            return false;
        }
        let attacker = state.found.and_then(|key| sensing::find(ai, key));
        Self::RULES.can_attack(state, ai, attacker.as_ref(), &Conditions::combat(), &|_| {
            true
        })
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::RULES.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let attacker = state.found.and_then(|key| sensing::find(ai, key));
        sensing::set_target(ai, attacker.as_ref());
        if let Some((_, trader)) = trader_holding(ai.tick.world, ai.tick.key) {
            state.timestamp = trader.body.combat.last_hurt_by.map(|(_, at)| at);
        }
        TargetRules::start(state);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(state, ai);
    }
}

/// The trader llama.
pub(crate) struct TraderLlama;

/// `maybeDespawn()` after the llama's `aiStep`: while despawnable its delay runs down
/// by one a tick, or is its trader's less one while a trader leads it, and at nothing
/// it leaves the world.
fn maybe_despawn(tick: &mut MobTick<'_>) {
    let now = tick.lived;
    let (shared, horse) = equine::read(&tick.mob);
    let one_player = tick
        .world
        .entity(tick.key)
        .is_some_and(|me| matches!(&me.entity.riding.passengers[..], [seat] if seat.player));
    // `isLeashedToWanderingTrader()`: the trader's delay, which the llama's follows.
    let trader_delay = if tick.leashed {
        trader_holding(tick.world, tick.key)
            .map(|(_, trader)| wandering_trader::state_of(&trader).1.despawn_delay)
    } else {
        None
    };
    let led_by_another = tick.leashed && trader_delay.is_none();
    let despawnable = !horse.tame
        && !led_by_another
        && !one_player
        && !shared.age_locked
        && !tick.mob.body.flags.persistent;
    let mut gone = false;
    equine::update(&mut tick.mob, |horse| {
        if let (true, Some(delay)) = (despawnable, trader_delay) {
            // Kept as what is spent of its own delay, so that it counts on from there
            // when the trader lets it go.
            let left = delay - 1;
            horse.despawn_since = None;
            horse.despawn_spent = DESPAWN_DELAY.saturating_sub(left);
            gone = left <= 0;
        } else if despawnable {
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
