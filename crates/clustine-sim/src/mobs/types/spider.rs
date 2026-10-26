//! The spider (`Spider`; `docs/groundwork/n7k-brief.md`, section 11.4), and what the cave
//! spider takes of it ([`super::cave_spider`]).
//!
//! It climbs: after each tick its synced flag (`DATA_FLAGS_ID`, bit 0) is whether its move
//! ran into something sideways, and that flag is its `onClimbable()` in the next tick's
//! travel (a climb of 0.2 a tick against a wall, a fall of no more than 0.15 while it
//! touches one) and pushing (a climbing spider is not pushed). Its navigation is a wall
//! climber's (`WallClimberNavigation`): once a path is done short of where it was sent, it
//! walks straight at it, up a wall if one is in the way. Cobwebs do not hold it.
//!
//! It floats, runs from an armadillo that is not rolled up, strolls and looks about; its
//! leap and its attack want a target, which is N9's (`monster_ai.rs`). An egg's spider is
//! ridden by a skeleton one time in a hundred (`finalizeSpawn`'s jockey, a skeleton of its
//! own draws, added after it as its passenger); its effect is drawn only on hard, never in
//! Clustine's world, which is easy.
//!
//! Named differences: the effect it would be given on hard is drawn for and not given
//! (mob effects are few, `n7k-plan.md` section 10), and a later member of a group is not
//! handed its group's effect; poison not affecting it is not needed until poison comes;
//! its own seat on a narrower vehicle (`getVehicleAttachmentPoint`) is the type's
//! extracted one; a skeleton riding it walks by its own goals (`n7k-plan.md`, section
//! 10, until N9).

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, TypeState};
use serde::{Deserialize, Serialize};

use super::monster_ai::{self, AttackOfN9, TargetOfN9};
use super::{CompanionSeat, MobType, SpawnReason, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, AvoidEntityGoalIf, FloatGoal, LeapAtTargetGoal, LookAtPlayerGoal,
    RandomLookAroundGoal, RandomStrollGoal,
};
use crate::mobs::ai::selector::flags;
use crate::mobs::ai::sensing::{Class, Looked};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::making;
use crate::mobs::tick::MobTick;
use crate::shown::DataShown;

/// The version of a spider's type state.
const VERSION: u8 = 1;

/// `DATA_FLAGS_ID`'s index, and its bit of climbing.
const FLAGS_INDEX: u8 = 16;
const CLIMBING: u8 = 1;

/// What a spider (or a cave spider) keeps: whether it climbs, and where its wall
/// climber's navigation was last sent (`WallClimberNavigation.pathToPosition`, which the
/// game keeps in memory only).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpiderState {
    pub(crate) climbing: bool,
    pub(crate) path_to: Option<BlockPos>,
}

/// A spider's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh spider's.
pub(crate) fn state_of(mob: &Mob) -> SpiderState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return SpiderState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

fn change(mob: &mut Mob, change: impl FnOnce(&mut SpiderState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = if own == SpiderState::default() {
            TypeState::default()
        } else {
            TypeState {
                version: VERSION,
                bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
            }
        };
    }
}

/// Where the wall climber's navigation was last sent (`pathToPosition`).
pub(crate) fn path_to(mob: &Mob) -> Option<BlockPos> {
    state_of(mob).path_to
}

/// `pathToPosition` set, or forgotten.
pub(crate) fn set_path_to(mob: &mut Mob, to: Option<BlockPos>) {
    change(mob, |own| own.path_to = to);
}

/// The armadillo, which a spider runs from while it is not rolled up.
const ARMADILLOS: &[u16] = &[entity_types::ARMADILLO as u16];

/// `!armadillo.isScared()`: an armadillo of the view that is not rolled up.
fn not_rolled_up(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    let Some(seen) = ai.tick.world.entity(looked.key) else {
        return true;
    };
    let clustine_world::EntityData::Mob { state, .. } = &seen.entity.data else {
        return true;
    };
    let armadillo = Mob {
        body: Default::default(),
        mind: Default::default(),
        state: state.clone(),
    };
    !super::armadillo::is_scared(&armadillo)
}

/// `Spider.registerGoals` and `createNavigation`, the cave spider's too.
pub(crate) fn register_goals(setup: &mut Setup) {
    setup.config.navigation.wall_climber = true;
    setup.add_goal(1, FloatGoal);
    setup.add_goal(
        2,
        AvoidEntityGoalIf {
            goal: AvoidEntityGoal {
                class: Class::Kinds(ARMADILLOS),
                max_distance: 6.0,
                walk_speed: 1.0,
                sprint_speed: 1.2,
            },
            avoids: not_rolled_up,
        },
    );
    setup.add_goal(3, LeapAtTargetGoal { yd: 0.4 });
    // `SpiderAttackGoal`, a melee attack: N9's.
    setup.add_goal(
        4,
        AttackOfN9 {
            flags: flags::MOVE | flags::LOOK,
        },
    );
    setup.add_goal(5, RandomStrollGoal::avoiding_water(0.8));
    setup.add_goal(6, LookAtPlayerGoal::new(Class::Player, 8.0));
    setup.add_goal(6, RandomLookAroundGoal);
    // Who last hurt it; `SpiderTargetGoal`s of players and iron golems, which ask nothing
    // in light worth a half or more.
    setup.add_target(1, TargetOfN9::HURT_BY);
    let in_the_dark = TargetOfN9 {
        interval: Some(10),
        dark_only: true,
    };
    setup.add_target(2, in_the_dark);
    setup.add_target(3, in_the_dark);
}

/// `Spider.tick` after `Monster`'s: climbing is whether its move ran into something
/// sideways.
pub(crate) fn after_tick(tick: &mut MobTick<'_>) {
    let collided = tick.living.body.horizontal_collision;
    change(&mut tick.mob, |own| own.climbing = collided);
}

/// `onClimbable()`: its flag.
pub(crate) fn on_climbable(mob: &Mob) -> Option<bool> {
    Some(state_of(mob).climbing)
}

/// The synced flags.
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let flags = if state_of(mob).climbing { CLIMBING } else { 0 };
    vec![(FLAGS_INDEX, DataShown::Byte(flags), flags == 0)]
}

/// The spider.
pub(crate) struct Spider;

impl MobType for Spider {
    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        monster_ai::walk_target_value(ai, pos)
    }

    /// `Spider.finalizeSpawn` after `Monster`'s: one time in a hundred a skeleton made
    /// (`JOCKEY`) at the spider, facing its way, finalised of the same random, to ride
    /// it once both are added; then, of a group of its own, an effect drawn for on hard
    /// alone.
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        if spawn.level.next_int(100) == 0 {
            let position = [
                f64::from(spawn.at.x) + 0.5,
                f64::from(spawn.at.y),
                f64::from(spawn.at.z) + 0.5,
            ];
            let yaw = spawn.mob.body.head.body_yaw;
            making::companion(
                spawn,
                entity_types::SKELETON as u16,
                position,
                yaw,
                (SpawnReason::Jockey, CompanionSeat::Passenger),
                |_| {},
            );
        }
        // `SpiderEffectsGroupData`: on hard, one time in `0.1 × special`, one of five
        // effects for ever (speed twice, strength, regeneration, invisibility).
        if clustine_world::Difficulty::WORLD == clustine_world::Difficulty::Hard
            && spawn.level.next_float() < 0.1 * spawn.difficulty.1
        {
            let _effect = spawn.level.next_int(5);
        }
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        after_tick(tick);
    }

    fn on_climbable(&self, mob: &Mob) -> Option<bool> {
        on_climbable(mob)
    }

    fn stuck_in_webs(&self) -> bool {
        false
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        shown(mob)
    }
}
