//! What the skeletons, the creeper and the spiders (part K4) share of `Monster` and of
//! the goals that only a target starts (`docs/groundwork/n7k-plan.md`, section 10:
//! attacks and targets of monsters are N9's).
//!
//! A goal of an attack or of a target holds its slot, so that its type's later goals keep
//! their parts when N9 builds it, and makes the draws the game's makes without a target
//! to start with: `NearestAttackableTargetGoal`'s `nextInt` of its interval, after the
//! spider's light test. Every player is a creative one, whom no target goal of the game
//! takes, so what the placeholders draw is the game's; what they never take (an iron
//! golem, a baby turtle, a mob that hurt them) is N9's, named.

use clustine_world::BlockPos;

use crate::mobs::ai::Ai;
use crate::mobs::ai::maths::reduced_tick_delay;
use crate::mobs::ai::selector::Goal;

/// `Monster.getWalkTargetValue(pos)`: less the light's worth (`getPathfindingCostFromLightLevels`),
/// so that a monster likes the dark and dislikes a place lit above a half.
pub(crate) fn walk_target_value(ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
    -super::animal::light_cost(ai, pos)
}

/// `Level.isBrightOutside()`: the sky darkened by less than four.
pub(crate) fn bright_outside(ai: &Ai<'_, '_>) -> bool {
    ai.tick.world.sky_darken() < 4
}

/// `Entity.getLightLevelDependentMagicValue()`: the light's worth at the mob's eyes, the
/// overworld's ambient light being 0; nothing where the view has no light.
pub(crate) fn magic_value_at_eyes(ai: &Ai<'_, '_>) -> f32 {
    let [x, _, z] = ai.position();
    let eyes = crate::physics::containing([x, ai.eye_y(), z]);
    let Some(light) = ai.tick.world.light(eyes) else {
        return 0.0;
    };
    let raw = (i32::from(light.sky) - ai.tick.world.sky_darken()).max(i32::from(light.block));
    let f = raw as f32 / 15.0;
    f / (4.0 - 3.0 * f)
}

/// A goal of an attack (`MeleeAttackGoal`, a skeleton's bow or melee goal, the spider's),
/// which only a target starts: it holds its slot and its flags, and is never used, as the
/// game's is not without a target (none of them draws before it asks for one).
#[derive(Debug, Clone, Copy)]
pub(crate) struct AttackOfN9 {
    pub(crate) flags: u8,
}

impl Goal for AttackOfN9 {
    type State = ();

    fn flags(&self) -> u8 {
        self.flags
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }
}

/// A target goal: `HurtByTargetGoal` (no interval, no draw), a
/// `NearestAttackableTargetGoal` of the interval `interval` (its `nextInt`, reduced as its
/// constructor reduces it), or a spider's `SpiderTargetGoal`, which asks nothing in light
/// worth a half or more (`dark_only`). It takes no target (N9).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetOfN9 {
    pub(crate) interval: Option<i32>,
    pub(crate) dark_only: bool,
}

impl TargetOfN9 {
    /// `HurtByTargetGoal(mob)`.
    pub(crate) const HURT_BY: Self = Self {
        interval: None,
        dark_only: false,
    };

    /// `NearestAttackableTargetGoal(mob, class, mustSee)`, of the interval 10.
    pub(crate) const NEAREST: Self = Self {
        interval: Some(10),
        dark_only: false,
    };
}

impl Goal for TargetOfN9 {
    type State = ();

    fn flags(&self) -> u8 {
        crate::mobs::ai::selector::flags::TARGET
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if self.dark_only && magic_value_at_eyes(ai) >= 0.5 {
            return false;
        }
        if let Some(interval) = self.interval {
            let interval = reduced_tick_delay(interval);
            if interval > 0 {
                let _found_only_if_zero = ai.tick.draws.next_int(interval as u32);
            }
        }
        false
    }
}
