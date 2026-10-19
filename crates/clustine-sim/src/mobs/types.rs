//! What each type of mob adds to the body and the mind: its goals, its own step, its
//! interactions, its spawn and its babies (`docs/groundwork/n7-plan.md`, section 11.3,
//! item 8). Each type is a module of its own that the parts from N7e on add; [`of`] is
//! the one place a type id finds its module.

use clustine_world::BlockPos;

use super::ai::{Ai, HeadLimits, Setup};
use super::tick::MobTick;

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
}

/// A type that adds nothing: a mob whose type no part has built yet is a body of the
/// game's defaults.
pub(crate) struct Plain;

impl MobType for Plain {}

/// The module of the entity type `kind`, by its id in `clustine_data::ENTITY_TYPES`.
pub(crate) fn of(kind: u16) -> &'static dyn MobType {
    // One arm a type, as each part brings it (N7e on).
    let _ = kind;
    &Plain
}
