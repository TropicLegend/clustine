//! What a mob's effects do as it is removed killed and as it is hurt
//! (`MobEffect.onMobRemoved`: oozing, weaving and wind charged; `onMobHurt`: infested;
//! `docs/groundwork/n9-plan.md`, section 5.2): part S1's.
//!
//! Until S1 fills it, a mob's effects do nothing at its removal or its hurt.

use clustine_world::EntityId;

use super::Region;
use crate::api::TickOutput;
use crate::mobs::body::effects::Effect;
use crate::physics::Vec3;

impl Region {
    /// `triggerOnDeathMobEffects`: `onMobRemoved` of each effect, with its amplifier,
    /// that the region's mob `key` held as it was removed killed at `at`
    /// (`LivingEntity.remove(KILLED)` at the end of its death, after its drops and orbs,
    /// before it leaves the world).
    #[expect(
        clippy::unused_self,
        reason = "part S1 fills it (docs/groundwork/n9-plan.md, section 5.2)"
    )]
    pub(crate) fn effects_at_removal(
        &mut self,
        _key: EntityId,
        _at: Vec3,
        _effects: &[(Effect, u8)],
        _output: &mut TickOutput,
    ) {
    }

    /// `onMobHurt` of each effect, with its amplifier, that the region's mob `key` at
    /// `at` holds after a hurt that took (the end of `LivingEntity.hurtServer`).
    #[expect(
        clippy::unused_self,
        reason = "part S1 fills it (docs/groundwork/n9-plan.md, section 5.2)"
    )]
    pub(crate) fn effects_at_hurt(
        &mut self,
        _key: EntityId,
        _at: Vec3,
        _effects: &[(Effect, u8)],
        _output: &mut TickOutput,
    ) {
    }
}
