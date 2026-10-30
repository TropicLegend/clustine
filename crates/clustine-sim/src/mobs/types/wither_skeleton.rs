//! The wither skeleton (`WitherSkeleton`, d javap, the same as n7k's; `docs/groundwork/
//! p-brief.md`, section 5.9; family E3 of P12, `docs/groundwork/p-plan.md`, sections
//! 18.3, 18.5 and 18.6): a skeleton of [`super::skeletons`]' kind, made by natural
//! spawning where a Nether fortress's list names it, and by its egg.
//!
//! Its goals are every skeleton's, with the piglins (`AbstractPiglin`: the piglin and
//! the brute) a target it sees at 3, added before the skeleton's own (WitherSkeleton.txt
//! 15–29); it dislikes lava (a malus of 8). At its spawn (`finalizeSpawn` 86–103) it
//! holds a stone sword and wears nothing (`populateDefaultEquipmentSlots`, which does not
//! call the skeleton's: no armour's draws, and no bow), draws no enchantment, draws its
//! loot pick-up against `0.55 × special` as every skeleton does, and its attack's base
//! becomes 4. Holding no bow, its weapon goal is the melee one. Its hit, where it took,
//! withers what it struck for 200 ticks (`doHurtTarget` 105–130, through
//! [`super::MobType::hit_effects`]). Wither does not take on it (`canBeAffected`
//! 146–157; `body/effects.rs`' table). Its sounds are its own; 5 experience with what
//! its equipment adds; its loot is its table's (coal, bones, and its skull at a
//! player's kill by chance).
//!
//! Named differences:
//! - Its arrows are set alight for 100 seconds (`getArrow` 132–144), but a mob's shot
//!   (`Local::Shoot`) has no fire of its own to carry it, and a wither skeleton holds a
//!   bow only where a template or a command gave it one (its loot pick-up refuses
//!   `#wither_skeleton_disliked_weapons`, and monsters pick nothing up yet): its arrows
//!   are plain.
//! - The skull a charged creeper's blast gives (`charged_creeper/wither_skeleton`)
//!   never comes, as no creeper is charged without lightning.
//! - Halloween's pumpkins are never drawn for (as every skeleton's).

use clustine_data::{PathType, attributes, entity_types, items, sounds};
use clustine_world::components::EffectInstance;
use clustine_world::{BlockPos, ItemStack};

use super::skeletons;
use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::targets::NearestTarget;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::attribute;
use crate::mobs::body::hurt::DamageSource;
use crate::region::Random;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The wither skeleton's entity type, by id.
pub(crate) const WITHER_SKELETON: u16 = entity_types::WITHER_SKELETON as u16;

/// The class `AbstractPiglin`: the piglin and the piglin brute.
pub(crate) const PIGLINS: &[u16] = &[
    entity_types::PIGLIN as u16,
    entity_types::PIGLIN_BRUTE as u16,
];

/// How long its hit withers: 200 ticks.
pub(crate) const WITHER_TICKS: i32 = 200;

/// Its attack's base after its spawn.
pub(crate) const SPAWNED_ATTACK: f64 = 4.0;

/// `WitherSkeleton.registerGoals` and its constructor's malus: the piglins at 3 first,
/// then every skeleton's goals and targets for what `mob` holds.
fn register(mob: Option<&Mob>, setup: &mut Setup) {
    setup.set_malus(PathType::Lava, 8.0);
    setup.add_target(3, NearestTarget::mobs(PIGLINS, true));
    skeletons::register_goals(WITHER_SKELETON, mob, setup);
}

/// The wither skeleton.
pub(crate) struct WitherSkeleton;

impl MobType for WitherSkeleton {
    fn register_goals(&self, setup: &mut Setup) {
        register(None, setup);
    }

    /// With `reassessWeaponGoal`'s goal for what it holds.
    fn register_goals_of(&self, mob: &Mob, setup: &mut Setup) {
        register(Some(mob), setup);
    }

    /// `AbstractSkeleton.finalizeSpawn` after `Monster`'s with the wither skeleton's own
    /// equipment (a stone sword in the main hand, nothing else, no enchantment), the
    /// loot pick-up's draw, then its attack's base of 4.
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let special = spawn.difficulty.1;
        spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
            Some(ItemStack::new(items::STONE_SWORD, 1));
        spawn.mob.body.flags.picks_up_loot = spawn.level.next_float() < 0.55 * special;
        if let Some(attack) = spawn
            .mob
            .body
            .attributes
            .instance(WITHER_SKELETON, attribute(attributes::ATTACK_DAMAGE))
        {
            attack.base = SPAWNED_ATTACK;
        }
    }

    /// `doHurtTarget`'s wither, where the hit took: `MobEffectInstance(WITHER, 200)`.
    fn hit_effects(&self, _: &Ai<'_, '_>) -> Vec<EffectInstance> {
        vec![EffectInstance {
            effect: "minecraft:wither".to_owned(),
            amplifier: 0,
            duration: WITHER_TICKS,
            ambient: false,
            visible: true,
            show_icon: true,
        }]
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        skeletons::walk_target_value(ai, pos)
    }

    fn avoids_sun(&self, mob: &Mob) -> bool {
        skeletons::avoids_sun(mob)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_WITHER_SKELETON_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_WITHER_SKELETON_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_WITHER_SKELETON_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_WITHER_SKELETON_STEP)
    }

    /// `Monster`'s `xpReward` 5, with what its equipment adds.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(5, mob, draws)
    }
}
