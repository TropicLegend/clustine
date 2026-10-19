//! What each type of mob adds to the body and the mind: its goals, its own step, its
//! interactions, its spawn and its babies (`docs/groundwork/n7-plan.md`, section 11.3,
//! item 8). Each type is a module of its own that the parts from N7e on add; [`of`] is
//! the one place a type id finds its module.

use clustine_world::BlockPos;

use super::ai::{Ai, HeadLimits, Setup};
use clustine_data::sounds;
use clustine_world::{EntityId, ItemStack};

use super::Mob;
use super::body::hurt::DamageSource;
use super::body::{ladder, loot};
use super::tick::MobTick;
use crate::region::Random;

/// What a type of mob does beyond what every mob does. Every hook has the game's
/// default, which a type that does nothing more keeps.
pub(crate) trait MobType: Sync {
    /// `registerGoals`, `createNavigation` and what the constructor sets of the AI: the
    /// goals and target goals in the order the game adds them (each slot's part is its
    /// place in that order, `mind_ids::FIRST_GOAL` on, the target goals after the
    /// goals), the navigation, the maluses, the controls. Called in every AI step, as
    /// nothing of it is kept.
    fn register_goals(&self, setup: &mut Setup) {
        let _ = setup;
    }

    /// `getMaxHeadXRot`, `getMaxHeadYRot`, `getHeadRotSpeed`.
    fn head(&self) -> HeadLimits {
        HeadLimits::default()
    }

    /// `PathfinderMob.getWalkTargetValue(pos)`: how much the mob likes a place it might
    /// wander to (0 for every place, unless the type says otherwise: an animal's
    /// prefers grass and light).
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        let _ = (ai, pos);
        0.0
    }

    /// `customServerAiStep`: what the type does after its navigation and before its
    /// controls.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// `mobInteract(player, hand)`: what a player's use of the mob does that is the
    /// type's own (feeding, milking, mounting). `Mob`'s passes.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let _ = used;
        Interaction::Pass
    }

    /// The type's own `finalizeSpawn`, before `Mob`'s (the body's: the follow range's
    /// bonus and the hand), with the level's draws on the mob's behalf: its variant,
    /// its colour, a baby of a group. `Mob`'s adds nothing.
    fn finalize_spawn(&self, mob: &mut Mob, level: &mut Random) {
        let _ = (mob, level);
    }

    /// `removeWhenFarAway(distance)`: whether, far from every player, it may despawn.
    /// Its class decides by default ([`ladder::remove_when_far_away`]).
    fn remove_when_far_away(&self, kind: u16, mob: &Mob, tick_count: u32, distance: f64) -> bool {
        let _ = (mob, distance);
        ladder::remove_when_far_away(kind, tick_count)
    }

    /// `getBaseExperienceReward`: the experience of its death, drawn from its own
    /// stream. Its class decides by default
    /// ([`super::body::hurt::base_experience_reward`]).
    fn experience_reward(&self, kind: u16, mob: &Mob, draws: &mut Random) -> i32 {
        let _ = mob;
        super::body::hurt::base_experience_reward(kind, draws)
    }

    /// `getAmbientSound()`: none in `Mob`.
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        let _ = mob;
        None
    }

    /// `getAmbientSoundInterval()`: 80 in `Mob`.
    fn ambient_interval(&self) -> u32 {
        80
    }

    /// `getHurtSound(source)`: `generic.hurt` in `LivingEntity`.
    fn hurt_sound(&self, mob: &Mob, source: &DamageSource) -> Option<i32> {
        let _ = (mob, source);
        Some(sounds::ENTITY_GENERIC_HURT)
    }

    /// `getDeathSound()`: `generic.death` in `LivingEntity`.
    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        let _ = mob;
        Some(sounds::ENTITY_GENERIC_DEATH)
    }

    /// `getSoundVolume()`: 1 in `LivingEntity`.
    fn sound_volume(&self) -> f32 {
        1.0
    }

    /// `isBaby()`: false but for an ageable mob of a negative age (N7e's).
    fn is_baby(&self, mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// What its loot table asks of it that is its type's (a sheep's colour).
    fn loot_facts(&self, mob: &Mob) -> loot::Facts {
        let _ = mob;
        loot::Facts::default()
    }

    /// `canBeLeashed()`: its class decides by default ([`ladder::can_be_leashed`]).
    fn can_be_leashed(&self, kind: u16, mob: &Mob) -> bool {
        let _ = mob;
        ladder::can_be_leashed(kind)
    }
}

/// A player's use of a mob, as `mobInteract` reads it.
#[allow(dead_code, reason = "what the types' own uses read, N7e on")]
pub(crate) struct Used<'a> {
    pub(crate) key: EntityId,
    /// The mob's entity type, by id.
    pub(crate) kind: u16,
    pub(crate) mob: &'a mut Mob,
    /// The player's entity.
    pub(crate) player: EntityId,
    /// What the hand used holds.
    pub(crate) held: Option<ItemStack>,
    pub(crate) creative: bool,
    pub(crate) sneaking: bool,
}

/// `InteractionResult` as a use of a mob answers.
#[allow(dead_code, reason = "what the types' own uses answer, N7e on")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Interaction {
    /// Nothing happened; the item's own use of an entity may follow.
    Pass,
    /// Done, the arm swinging for the trackers.
    Success,
    /// Done, the arm swinging for the player too.
    SuccessServer,
    /// Done, no arm swinging.
    Consume,
}

/// A type that adds nothing: a mob whose type no part has built yet is a body of the
/// game's defaults.
pub(crate) struct Plain;

impl MobType for Plain {}

/// The module of the entity type `kind`, by its id in `clustine_data::ENTITY_TYPES`.
/// Whether a type has its module, so that an egg makes it: a mob of a type without its
/// AI would stand like a statue, so until its part brings it, its egg makes nothing
/// (`docs/groundwork/n7-plan.md`, section 11.6).
pub(crate) fn is_built(kind: u16) -> bool {
    // One arm a type, as each part brings it (N7e on).
    let _ = kind;
    false
}

pub(crate) fn of(kind: u16) -> &'static dyn MobType {
    // One arm a type, as each part brings it (N7e on).
    let _ = kind;
    &Plain
}
