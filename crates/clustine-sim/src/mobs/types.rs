//! What each type of mob adds to the body and the mind: its goals, its own step, its
//! interactions, its spawn and its babies (`docs/groundwork/n7-plan.md`, section 11.3,
//! item 8). Each type is a module of its own that the parts from N7e on add; [`of`] is
//! the one place a type id finds its module.
//!
//! The farm animals (part N7e: cows, mooshrooms, pigs, sheep, chickens and rabbits) share
//! what `AgeableMob` and `Animal` add, which [`animal`] holds; each has a module of its
//! own beside it.

pub(crate) mod animal;
pub(crate) mod chicken;
pub(crate) mod cow;
pub(crate) mod mooshroom;
pub(crate) mod pig;
pub(crate) mod rabbit;
pub(crate) mod sheep;
#[cfg(test)]
mod tests;

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, Entity, EntityData, EntityId, ItemStack};

use super::Mob;
use super::ai::{Ai, HeadLimits, Setup, controls};
use super::body::hurt::DamageSource;
use super::body::{ladder, loot};
use super::tick::MobTick;
use crate::physics::Vec3;
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

    /// `ItemStack.interactLivingEntity` of the item used on a mob of the type, once the
    /// mob's own use passed: a dye on a sheep. `Item`'s passes.
    fn item_interact(&self, used: &mut Used<'_>) -> Interaction {
        let _ = used;
        Interaction::Pass
    }

    /// What the type's constructor sets and draws beyond the body's (a chicken's egg
    /// timer, a rabbit's idle timer and its wanted position), from the new mob's own
    /// stream: every mob of the type is made so, by an egg, a breeding or a conversion.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let _ = (mob, own);
    }

    /// The type's own `finalizeSpawn`, before `Mob`'s (the body's: the follow range's
    /// bonus and the hand), with the level's draws on the mob's behalf: its variant,
    /// its colour, a baby of a group. `Mob`'s adds nothing.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let _ = spawn;
    }

    /// `getBreedOffspring(level, partner)`: the child of a breeding (or of a spawn egg
    /// used on the mob, its own partner), as `EntityType.create` makes it, with what it
    /// takes of its parents; `None` where the type has none.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let _ = breeding;
        None
    }

    /// What the type's `aiStep` does after `LivingEntity.aiStep` (its travel and its
    /// pushes): an ageable mob's growing, an animal's love, a chicken's slow fall and
    /// eggs, a rabbit's hop ending.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// What the type's `actuallyHurt` does before the body's (an animal's love ends),
    /// at the moment `moment`.
    fn actually_hurt(&self, mob: &mut Mob, moment: u32) {
        let _ = (mob, moment);
    }

    /// `getDefaultDimensions(pose)` where it is not the type's (a baby animal's own
    /// size): its width, height and eye height.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let _ = mob;
        None
    }

    /// `moveControl.tick()` of the type's move control (a rabbit's sets its speed first).
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        controls::tick_move(ai);
    }

    /// `jumpControl.tick()` of the type's jump control (a rabbit's starts a hop).
    fn tick_jump_control(&self, ai: &mut Ai<'_, '_>) {
        controls::tick_jump(ai);
    }

    /// `getJumpPower()` where the type has its own (a rabbit's), given the jump factor of
    /// the block it stands on; `None` for `LivingEntity`'s.
    fn jump_power(&self, tick: &MobTick<'_>, block_factor: f32) -> Option<f32> {
        let _ = (tick, block_factor);
        None
    }

    /// What the type's `jumpFromGround` does after `LivingEntity`'s (a rabbit's push
    /// ahead and its hop shown).
    fn jumped_from_ground(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
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

    /// The sound of its `playStepSound(pos, state)` where the type has its own (played
    /// at 0.15 and 1); `None` for `Entity`'s, the step of the block stepped on.
    fn step_sound(&self, mob: &Mob) -> Option<i32> {
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

    /// The type's own synced values (`defineSynchedData`), each with its index and
    /// whether it is the default: an ageable type's baby (16), a sheep's wool (18) and
    /// the like (`crate::shown`). None for a type that adds none.
    fn shown(&self, mob: &Mob) -> Vec<(u8, crate::shown::DataShown, bool)> {
        let _ = mob;
        Vec::new()
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
#[allow(
    dead_code,
    reason = "what the uses of later parts read: a tamer's key, a sneaking rider's"
)]
pub(crate) struct Used<'a> {
    pub(crate) key: EntityId,
    /// The mob's entity type, by id.
    pub(crate) kind: u16,
    pub(crate) mob: &'a mut Mob,
    /// The player's entity.
    pub(crate) player: EntityId,
    /// What the hand used holds.
    pub(crate) held: Option<ItemStack>,
    /// Whether the player is a creative one (every player is: what they use is kept).
    pub(crate) creative: bool,
    pub(crate) sneaking: bool,
    /// The player's feet.
    pub(crate) from: Vec3,
    /// The mob's feet.
    pub(crate) position: Vec3,
    /// The mob's `lived`: the moment its counters see the use at.
    pub(crate) moment: u32,
    /// The mob's own stream, aside from its tick, on from what it drew in it.
    pub(crate) draws: &'a mut Random,
    /// What the use did beyond the mob, for the region to do in order.
    pub(crate) did: Vec<Did>,
}

/// What a player's use of a mob does beyond the mob itself, which the region carries
/// out in the order given.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Did {
    /// `playSound` at `at` from `source`, heard by everyone, or by everyone but the
    /// player who used it (`Player.playSound`, whose own client plays it).
    Sound {
        sound: i32,
        source: i32,
        at: Vec3,
        volume: f32,
        pitch: f32,
        but_the_player: bool,
    },
    /// `broadcastEntityEvent(mob, event)`.
    Event(u8),
    /// `gameEvent(event, player)` at the mob, by its name.
    GameEvent(&'static str),
    /// `ItemUtils.createFilledResult`: the stack goes to the player.
    Give {
        stack: ItemStack,
        no_duplicates: bool,
    },
    /// An item entity of `stack` at `at` (`spawnAtLocation`, a shearing's drops), with
    /// its pick-up delay, and a push added to its own motion.
    Drop {
        stack: ItemStack,
        at: Vec3,
        pick_up_delay: u16,
        push: Option<Vec3>,
    },
    /// `convertTo(kind, single)`: the mob made a mob of `kind` where it stands, with
    /// `drops` laid at its top before the new mob is added, and itself gone.
    Convert { kind: u16, drops: Vec<ItemStack> },
}

/// A mob being made by an egg (`finalizeSpawn`): its type, the mob, the level's random
/// on its behalf, and the biome at its block.
pub(crate) struct Spawning<'a> {
    pub(crate) mob: &'a mut Mob,
    pub(crate) level: &'a mut Random,
    /// The name of the biome at the mob's block, where its chunk is read.
    pub(crate) biome: Option<&'static str>,
}

/// A breeding as `getBreedOffspring` reads it: the parent that breeds (`this`) and its
/// partner, the draws, and the biome at the parent's block.
pub(crate) struct Breeding<'a> {
    pub(crate) this: &'a Mob,
    pub(crate) partner: &'a Mob,
    /// The parent's own stream: its random's draws, and the level's on its behalf at
    /// their place among them (the plan's 7.2.8).
    pub(crate) draws: &'a mut Random,
    /// The name of the biome at the parent's block, where its chunk is read.
    pub(crate) biome: Option<&'static str>,
}

/// An entity's size (`EntityDimensions`): its width, height and eye height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Dimensions {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) eye_height: f32,
}

/// `InteractionResult` as a use of a mob answers.
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

/// Whether a type has its module, so that an egg makes it: a mob of a type without its
/// AI would stand like a statue, so until its part brings it, its egg makes nothing
/// (`docs/groundwork/n7-plan.md`, section 11.6).
pub(crate) fn is_built(kind: u16) -> bool {
    built(kind).is_some()
}

/// The module of the entity type `kind`, by its id in `clustine_data::ENTITY_TYPES`.
pub(crate) fn of(kind: u16) -> &'static dyn MobType {
    built(kind).unwrap_or(&Plain)
}

/// The module of a type some part has built: one arm a type, as each part brings it.
fn built(kind: u16) -> Option<&'static dyn MobType> {
    let found: &'static dyn MobType = match i32::from(kind) {
        entity_types::COW => &cow::Cow,
        entity_types::MOOSHROOM => &mooshroom::Mooshroom,
        entity_types::PIG => &pig::Pig,
        entity_types::SHEEP => &sheep::Sheep,
        entity_types::CHICKEN => &chicken::Chicken,
        entity_types::RABBIT => &rabbit::Rabbit,
        _ => return None,
    };
    Some(found)
}

/// The size of `entity` where its type's module gives it one of its own (a baby
/// animal's), else `None`: the type's.
pub(crate) fn own_size(entity: &Entity) -> Option<Dimensions> {
    let module = built(entity.kind)?;
    let EntityData::Mob { state, .. } = &entity.data else {
        return None;
    };
    // Only the type's state decides a baby's size: the body is not decoded for it.
    let mob = Mob {
        body: Default::default(),
        mind: Default::default(),
        state: state.clone(),
    };
    module.dimensions(&mob)
}
