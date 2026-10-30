//! The magma cube (`MagmaCube` on `AbstractCubeMob`; `docs/groundwork/p-plan.md`,
//! sections 18.3, 18.5 and 18.6, and `docs/groundwork/p-brief.md`, section 5.9; d
//! MagmaCube), on the cube's layer in [`super::cube_mob`] as the slime is.
//!
//! What it does as the slime does: its size (1, 2 or 4 from an egg or a spawn), health
//! `n²`, speed `0.2 + 0.1n`, attack `n`, its box `n` times its type's; its four goals and
//! two targets (players within 4 blocks up or down, iron golems); its landing's squish;
//! its experience of its size; its split at death into two to four of half its size
//! (`region/splits.rs`). What is its own (MagmaCube.txt):
//! - armour `3n` (`setSize`, 70–101);
//! - four times the slime's delay between its jumps (`getJumpDelay`, 117–124), and a jump
//!   of `0.42 + 0.1n` (`jumpFromGround`, 135–161, the jump strength's 0.42 and a tenth of
//!   its size);
//! - in lava, a jump of `0.22 + 0.05n` (`jumpInLiquid(LAVA)`, 162–192), so that it leaps
//!   out of a lake where the slime would bob;
//! - it means harm at any size (`canDealDamage`, 193–198);
//! - its sounds, the small ones when it is tiny but for one jump sound for all;
//! - its spawn rule: anything but peaceful (`checkMagmaCubeSpawnRules`, 59–69).
//!
//! It is immune to fire (its type's `fireImmune`), so it is never "on fire" (`isOnFire`,
//! 112–116, the client's own test for a type immune to fire) and lava and fire do not
//! hurt it. Its landing's particles are flames where the slime's are slime, which only
//! a client draws; the server draws the same floats for them.
//!
//! Named differences: its `jumpFromGround` keeps an upward motion above its jump's, as
//! the slime's does in Clustine (`cube_mob`); `getAttackDamage`'s two more and
//! `getLightLevelDependentMagicValue`'s 1 are read by nothing Clustine has (no cube hurts
//! what touches it: every player is creative, and the push of an iron golem is not told
//! to the cube); its `decreaseSquish` is the client's.

use clustine_data::{attributes as ids, entity_types, sounds};
use clustine_world::{BlockPos, Difficulty};

use super::cube_mob::{self, CubeMobAttackGoal, CubeSounds, Sounds};
use super::{Dimensions, MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::targets;
use crate::mobs::ai::{Ai, HeadLimits, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::{attribute, effects, loot};
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;
use crate::spawning::rules::SpawnWorld;
use clustine_world::generation::Draws;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The magma cube's sounds, tiny and larger: one jump sound for both
/// (`getJumpSound`, MagmaCube.txt 237–241).
pub(crate) const SOUNDS: CubeSounds = CubeSounds {
    small: Sounds {
        jump: sounds::ENTITY_MAGMA_CUBE_JUMP,
        squish: sounds::ENTITY_MAGMA_CUBE_SQUISH_SMALL,
        hurt: sounds::ENTITY_MAGMA_CUBE_HURT_SMALL,
        death: sounds::ENTITY_MAGMA_CUBE_DEATH_SMALL,
    },
    large: Sounds {
        jump: sounds::ENTITY_MAGMA_CUBE_JUMP,
        squish: sounds::ENTITY_MAGMA_CUBE_SQUISH,
        hurt: sounds::ENTITY_MAGMA_CUBE_HURT,
        death: sounds::ENTITY_MAGMA_CUBE_DEATH,
    },
};

/// The entity type's id.
const fn kind() -> u16 {
    entity_types::MAGMA_CUBE as u16
}

/// Whether the cube type `kind` is the magma cube, whose rules `cube_mob` asks.
pub(crate) fn is_magma_cube(kind: u16) -> bool {
    i32::from(kind) == entity_types::MAGMA_CUBE
}

/// `getJumpDelay()`'s factor over `AbstractCubeMob`'s: four for the magma cube
/// (MagmaCube.txt 117–124), one for every other cube.
pub(crate) fn jump_delay_factor(kind: u16) -> i32 {
    if is_magma_cube(kind) { 4 } else { 1 }
}

/// `MagmaCube.setSize(size, resetHealth)` after its parent's (MagmaCube.txt 70–101): its
/// armour `3 · size` (the attack and the experience its parent's already are). Nothing
/// for another cube.
pub(crate) fn set_armour(kind: u16, mob: &mut Mob, size: i32) {
    if !is_magma_cube(kind) {
        return;
    }
    if let Some(armour) = mob.body.attributes.instance(kind, attribute(ids::ARMOR)) {
        armour.base = f64::from(size * 3);
    }
}

/// `MagmaCube.jumpInLiquid(LAVA)` (MagmaCube.txt 162–192): where the living body's jump
/// went up through lava this tick (`LivingEntity.aiStep`'s second branch, after the
/// water's), its upward motion set to `0.22 + 0.05 · size` in place of the `0.04` the
/// body gave it, and its move sent. The body's jump knows no type's own liquid jump, so
/// the magma cube's is made here, before its travel and after the jump, where nothing
/// else has touched its motion.
fn jump_in_lava(tick: &mut MobTick<'_>) {
    if !tick.mob.body.inputs.jumping {
        return;
    }
    let living = &tick.living;
    let threshold = if living.eye_height < 0.4 { 0.0 } else { 0.4 };
    let on_ground = living.body.on_ground;
    let water = living.fluids.water_height;
    let in_water = living.body.in_water && water > 0.0;
    if in_water && (!on_ground || water > threshold) {
        return;
    }
    if !(living.in_lava() && (!on_ground || living.fluids.lava_height > threshold)) {
        return;
    }
    let size = cube_mob::size(&tick.mob) as f32;
    tick.living.body.motion[1] = f64::from(0.22_f32 + size * 0.05_f32);
    tick.out.needs_sync = true;
}

/// The magma cube's spawn rule at a natural reason (`checkMagmaCubeSpawnRules`,
/// MagmaCube.txt 59–69): anything but peaceful, wherever it is placed; nothing drawn.
pub(crate) fn check_spawn_rules(
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    let _ = (world, pos, draws);
    Difficulty::WORLD != Difficulty::Peaceful
}

/// The magma cube.
pub(crate) struct MagmaCube;

impl MobType for MagmaCube {
    /// `AbstractCubeMob.registerGoals` and the magma cube's own (MagmaCube.txt 11–47): its
    /// attack (2) after the cube's three, then the slime's two targets.
    fn register_goals(&self, setup: &mut Setup) {
        cube_mob::register_goals(setup);
        setup.add_goal(2, CubeMobAttackGoal);
        targets::register(setup, super::slime::TARGETS);
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        cube_mob::tick_move_control(ai);
    }

    fn head(&self) -> HeadLimits {
        cube_mob::head()
    }

    /// `jumpFromGround`'s `getJumpPower() + 0.1 · size` (MagmaCube.txt 135–161): the jump
    /// strength by the block's factor, jump boost's power, and a tenth of its size.
    fn jump_power(&self, tick: &MobTick<'_>, block_factor: f32) -> Option<f32> {
        let strength = tick
            .mob
            .body
            .attributes
            .value(kind(), attribute(ids::JUMP_STRENGTH)) as f32;
        let power =
            strength * 1.0 * block_factor + effects::jump_boost_power(&tick.mob.body.effects);
        Some(power + cube_mob::size(&tick.mob) as f32 * 0.1_f32)
    }

    fn before_travel(&self, tick: &mut MobTick<'_>) {
        jump_in_lava(tick);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        cube_mob::after_tick(tick);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        Some(cube_mob::dimensions(kind(), mob))
    }

    /// `MagmaCube.finalizeSpawn`: an `AgeableMobGroupData(false)` where none is given,
    /// then the cube's.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        cube_mob::finalize_spawn(spawn);
    }

    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        cube_mob::set_spawn_size(kind(), spawn);
    }

    fn can_be_a_baby(&self) -> bool {
        false
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        cube_mob::experience_reward(mob, draws)
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(SOUNDS.of(mob).hurt)
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(SOUNDS.of(mob).death)
    }

    fn sound_volume_of(&self, mob: &Mob) -> f32 {
        cube_mob::sound_volume(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        cube_mob::shown(mob)
    }

    /// `entities/magma_cube`: its magma cream (and the frog's froglights) by its size.
    fn loot_facts(&self, mob: &Mob) -> loot::Facts {
        loot::Facts {
            cube_size: Some(cube_mob::size(mob)),
            ..loot::Facts::default()
        }
    }
}
