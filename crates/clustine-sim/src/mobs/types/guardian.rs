//! The guardian (`Guardian`; `n7k-brief.md` 3.8), part K5: what [`guardians`] holds,
//! with the guardian's stroll every 80 ticks or so and its sounds.

use clustine_data::sounds;

use super::{MobType, guardians};
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::region::Random;

/// The guardian.
pub(crate) struct Guardian;

/// What both guardians are as types but for their constructors, their goals' stroll and
/// their sounds, which each type gives (`fn sound(&self, in_water, which) -> i32`).
macro_rules! guardian_body {
    () => {
        fn before_ai_step(&self, tick: &mut $crate::mobs::tick::MobTick<'_>) {
            $crate::mobs::types::guardians::before_ai_step(tick);
        }

        fn tick_move_control(&self, ai: &mut $crate::mobs::ai::Ai<'_, '_>) {
            $crate::mobs::types::guardians::tick_move_control(ai);
        }

        fn travel(&self, tick: &$crate::mobs::tick::MobTick<'_>) -> $crate::living::Travel {
            $crate::mobs::types::guardians::travel(tick)
        }

        fn walk_target_value(
            &self,
            ai: &$crate::mobs::ai::Ai<'_, '_>,
            pos: clustine_world::BlockPos,
        ) -> f32 {
            $crate::mobs::types::guardians::walk_target_value(ai, pos)
        }

        fn before_hurt_server(
            &self,
            struck: &mut $crate::mobs::body::hurt::Struck<'_>,
            source: &$crate::mobs::body::hurt::DamageSource,
            _: f32,
        ) {
            $crate::mobs::types::guardians::before_hurt_server(struck, source);
        }

        fn head(&self) -> $crate::mobs::ai::HeadLimits {
            $crate::mobs::types::guardians::head()
        }

        /// `xpReward` 10.
        fn experience_reward(
            &self,
            _: u16,
            mob: &$crate::mobs::Mob,
            draws: &mut $crate::region::Random,
        ) -> i32 {
            $crate::mobs::types::illagers::experience(10, mob, draws)
        }

        /// `getAmbientSoundInterval()`: 160.
        fn ambient_interval(&self) -> u32 {
            160
        }

        /// The ambient sound as an egg's mob makes it, of where it last was.
        fn ambient_sound(&self, mob: &$crate::mobs::Mob) -> Option<i32> {
            let in_water = $crate::mobs::types::guardians::state_of(mob).in_water;
            Some(self.sound(in_water, $crate::mobs::types::guardian::Sound::Ambient))
        }

        fn ambient_sound_at(&self, _: &$crate::mobs::Mob, in_water: bool, _: bool) -> Option<i32> {
            Some(self.sound(in_water, $crate::mobs::types::guardian::Sound::Ambient))
        }

        fn hurt_sound(
            &self,
            mob: &$crate::mobs::Mob,
            _: &$crate::mobs::body::hurt::DamageSource,
        ) -> Option<i32> {
            let in_water = $crate::mobs::types::guardians::state_of(mob).in_water;
            Some(self.sound(in_water, $crate::mobs::types::guardian::Sound::Hurt))
        }

        fn death_sound(&self, mob: &$crate::mobs::Mob) -> Option<i32> {
            let in_water = $crate::mobs::types::guardians::state_of(mob).in_water;
            Some(self.sound(in_water, $crate::mobs::types::guardian::Sound::Death))
        }

        fn shown(&self, mob: &$crate::mobs::Mob) -> Vec<(u8, $crate::shown::DataShown, bool)> {
            $crate::mobs::types::guardians::shown(mob)
        }
    };
}

pub(crate) use guardian_body;

/// Which of a guardian's sounds: each is its own in water, another on land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sound {
    Ambient,
    Hurt,
    Death,
}

impl Guardian {
    /// Its sound of `which`, in water or on land.
    fn sound(&self, in_water: bool, which: Sound) -> i32 {
        match (which, in_water) {
            (Sound::Ambient, true) => sounds::ENTITY_GUARDIAN_AMBIENT,
            (Sound::Ambient, false) => sounds::ENTITY_GUARDIAN_AMBIENT_LAND,
            (Sound::Hurt, true) => sounds::ENTITY_GUARDIAN_HURT,
            (Sound::Hurt, false) => sounds::ENTITY_GUARDIAN_HURT_LAND,
            (Sound::Death, true) => sounds::ENTITY_GUARDIAN_DEATH,
            (Sound::Death, false) => sounds::ENTITY_GUARDIAN_DEATH_LAND,
        }
    }
}

impl MobType for Guardian {
    fn register_goals(&self, setup: &mut Setup) {
        guardians::register_goals(setup, 80);
    }

    /// `Guardian`'s constructor's draw.
    fn construct(&self, _: &mut Mob, own: &mut Random) {
        guardians::construct(own);
    }

    guardian_body!();
}
