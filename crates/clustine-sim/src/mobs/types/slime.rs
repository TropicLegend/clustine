//! The slime (`Slime` on `AbstractCubeMob`; `docs/groundwork/n9-plan.md`, section 8.4;
//! n9 Slime), on the cube's layer in [`super::cube_mob`].
//!
//! It jumps about, its way drawn now and then; at a player (whom it never takes, every
//! player being creative) or an iron golem it sees within 4 blocks up or down (its
//! players' selector, `|dy| <= 4`) it turns and jumps at it, meaning harm unless it is
//! tiny. Its hurt and death sounds are the small ones when it is tiny. Its experience is
//! its size. At death a slime larger than tiny splits into two to four of half its size
//! (`region/splits.rs`); an egg's slime is of size 1, 2 or 4.
//!
//! Named: what a slime does to an iron golem that pushes it is not done (see
//! [`super::cube_mob`]).

use clustine_data::{entity_types, sounds};

use super::cube_mob::{self, CubeMobAttackGoal, CubeSounds, Sounds};
use super::{Dimensions, MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::ai::targets::{self, Among, NearestTarget, Target};
use crate::mobs::ai::{Ai, HeadLimits, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::loot;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The slime's sounds, tiny and larger.
pub(crate) const SOUNDS: CubeSounds = CubeSounds {
    small: Sounds {
        jump: sounds::ENTITY_SLIME_JUMP_SMALL,
        squish: sounds::ENTITY_SLIME_SQUISH_SMALL,
        hurt: sounds::ENTITY_SLIME_HURT_SMALL,
        death: sounds::ENTITY_SLIME_DEATH_SMALL,
    },
    large: Sounds {
        jump: sounds::ENTITY_SLIME_JUMP,
        squish: sounds::ENTITY_SLIME_SQUISH,
        hurt: sounds::ENTITY_SLIME_HURT,
        death: sounds::ENTITY_SLIME_DEATH,
    },
};

/// The slime's players' selector (`lambda$addTargetingGoals$0`): within 4 blocks up or
/// down of its feet.
pub(crate) fn within_height(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    (looked.position[1] - ai.position()[1]).abs() <= 4.0
}

/// `Slime.addTargetingGoals`: players it sees within 4 up or down (interval 10), then iron
/// golems it sees.
pub(crate) const TARGETS: &[(i32, Target)] = &[
    (
        1,
        Target::Nearest(NearestTarget {
            selector: Some(within_height),
            ..NearestTarget::new(Among::Players, true)
        }),
    ),
    (
        3,
        Target::Nearest(NearestTarget::mobs(targets::IRON_GOLEMS, true)),
    ),
];

/// The entity type's id.
const fn kind() -> u16 {
    entity_types::SLIME as u16
}

/// The slime.
pub(crate) struct Slime;

impl MobType for Slime {
    /// `AbstractCubeMob.registerGoals` and the slime's own: its attack (2) after the
    /// cube's three, then its targets.
    fn register_goals(&self, setup: &mut Setup) {
        cube_mob::register_goals(setup);
        setup.add_goal(2, CubeMobAttackGoal);
        targets::register(setup, TARGETS);
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        cube_mob::tick_move_control(ai);
    }

    fn head(&self) -> HeadLimits {
        cube_mob::head()
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        cube_mob::after_tick(tick);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        Some(cube_mob::dimensions(kind(), mob))
    }

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

    fn loot_facts(&self, mob: &Mob) -> loot::Facts {
        loot::Facts {
            cube_size: Some(cube_mob::size(mob)),
            ..loot::Facts::default()
        }
    }
}
