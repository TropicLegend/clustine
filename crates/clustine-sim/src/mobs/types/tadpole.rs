//! The tadpole (`Tadpole`; `docs/groundwork/n7-brief.md`, section 7.5.8): a fish of
//! `AbstractFish`'s goals and body (it panics, flees players, swims about, flops and
//! drowns on land), never despawning, that counts its own age up a tick at a time and at
//! 24000 is a frog of the biome it is in; a slime ball takes a tenth of what it has
//! left, a golden dandelion stops its age, and a water bucket takes it.
//!
//! Beside its goals it has a brain (`TadpoleAi`, [`super::tadpole_ai`]), which ticks
//! after them in its `customServerAiStep`.
//!
//! Its type state is its own encoding ([`TadpoleState`]), under a version of its own
//! that no animal's or fish's reading takes for theirs. Its age is a stamp against
//! `lived`, as an animal's is (`docs/groundwork/n7-plan.md`, section 11.2): a tadpole
//! at rest commits nothing for its growing until it grows up.
//!
//! Named differences: a bucket keeps nothing of it (the plan's question 4), so one
//! poured is a newborn; the particles of its age lock are drawn and not sent, as an
//! animal's are.

use clustine_blocks::data::entity_in_tag;
use clustine_data::{PathType, items, sounds};
use clustine_world::TypeState;
use serde::{Deserialize, Serialize};

use super::animal::{self, SOUND_SOURCE_PLAYERS};
use super::fish::FishSwimGoal;
use super::{Did, Interaction, MobType, Used, fish, water};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{AvoidEntityGoal, PanicGoal};
use crate::mobs::ai::navigation::{Kind, Settings};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{LookKind, MoveKind, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::physics;
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a tadpole's type state: high, so that it is never an animal's, and
/// apart from a fish's 129.
const VERSION: u8 = 130;

/// `ticksToBeFrog`: `|-24000|`.
pub(crate) const TICKS_TO_BE_FROG: i32 = 24_000;

/// `AGE_LOCK_COOLDOWN_TICKS`: how long its lock's particles last.
const LOCK_TICKS: u32 = 40;

/// The synced indices of `AbstractFish.FROM_BUCKET` (never set for a tadpole) and its
/// own `AGE_LOCKED`.
pub(crate) const FROM_BUCKET_INDEX: u8 = 16;
pub(crate) const AGE_LOCKED_INDEX: u8 = 17;

/// What a tadpole keeps: its age (`age`, saved as `Age`) at the moment `age_at`, which
/// it counts up from by one a tick unless locked; its lock (`AGE_LOCKED`); and the
/// moment its lock's particle timer (`ageLockParticleTimer`) comes to nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TadpoleState {
    pub(crate) age: i32,
    pub(crate) age_at: u32,
    pub(crate) age_locked: bool,
    pub(crate) lock_until: u32,
}

/// The ticks from `from` to `now`, none where `now` is earlier (a crash's restore lowers
/// `lived`).
fn since(from: u32, now: u32) -> i64 {
    (i64::from(now) - i64::from(from)).max(0)
}

/// What is left at `now` of a timer that ends at `until`.
fn left(until: u32, now: u32) -> i32 {
    (i64::from(until) - i64::from(now)).clamp(0, i64::from(i32::MAX)) as i32
}

impl TadpoleState {
    /// Its age at the moment `now`: counted up a tick at a time unless locked.
    pub(crate) fn age(&self, now: u32) -> i32 {
        if self.age_locked {
            return self.age;
        }
        (i64::from(self.age) + since(self.age_at, now)).min(i64::from(i32::MAX)) as i32
    }

    /// `setAge(age)` at the moment `now`, which says whether it grows up for it
    /// (`age >= ticksToBeFrog`: `ageUp()`).
    pub(crate) fn set_age(&mut self, age: i32, now: u32) -> bool {
        self.age = age;
        self.age_at = now;
        age >= TICKS_TO_BE_FROG
    }

    /// `getTicksLeftUntilAdult()` at `now`.
    pub(crate) fn ticks_left(&self, now: u32) -> i32 {
        (TICKS_TO_BE_FROG - self.age(now)).max(0)
    }
}

/// A tadpole's state from its type state: a record of nothing, or one this build cannot
/// read, as a newborn's.
pub(crate) fn read(state: &TypeState) -> TadpoleState {
    if state.version != VERSION || state.bytes.is_empty() {
        return TadpoleState::default();
    }
    postcard::from_bytes(&state.bytes).unwrap_or_default()
}

/// The type state of `tadpole`: nothing for a newborn's.
pub(crate) fn write(tadpole: &TadpoleState) -> TypeState {
    if *tadpole == TadpoleState::default() {
        return TypeState::default();
    }
    TypeState {
        version: VERSION,
        bytes: postcard::to_allocvec(tadpole).expect("a type state is encoded into memory"),
    }
}

/// Changes a tadpole's state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut TadpoleState)) {
    let mut tadpole = read(&mob.state);
    let before = tadpole.clone();
    change(&mut tadpole);
    if tadpole != before {
        mob.state = write(&tadpole);
    }
}

/// A tadpole hatched from frogspawn (`FrogspawnBlock.spawnTadpoles`): `EntityType.create`
/// with no `finalizeSpawn`, kept for good.
pub(crate) fn hatchling() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.flags.persistent = true;
    mob
}

/// `getRandomX(1)`, `getRandomY() + 0.5` and `getRandomZ(1)` of a particle's place,
/// drawn and not sent (`Level.addParticle` is nothing on a server).
fn particle_draws(draws: &mut Random) {
    for _ in 0..3 {
        draws.next_double();
    }
}

/// `Tadpole.mobInteract`: a slime ball, unless its age is locked, takes a tenth of its
/// time left (in whole seconds) and may grow it up there and then, with its particle's
/// draws after; a golden dandelion, once its last use's 40 ticks are over, locks its age
/// (or unlocks it) at nothing, with its sound at its block; else a water bucket takes
/// it, with its own filling sound.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let now = used.moment;
    let mut tadpole = read(&used.mob.state);
    if super::frog::FOOD.test(used.held.as_ref()) && !tadpole.age_locked {
        // `usePlayerItem`: a creative player keeps the ball.
        let gained = animal::speed_up_seconds(tadpole.ticks_left(now)) * 20;
        let grows = tadpole.set_age(tadpole.age(now).wrapping_add(gained), now);
        used.mob.state = write(&tadpole);
        if grows {
            used.did.push(Did::GrowUp);
        }
        particle_draws(used.draws);
        return Interaction::Success;
    }
    let dandelion = animal::is_item(used.held.as_ref(), items::GOLDEN_DANDELION)
        && left(tadpole.lock_until, now) == 0
        && !entity_in_tag(i32::from(used.kind), "minecraft:cannot_be_age_locked");
    if dandelion {
        tadpole.age_locked = !tadpole.age_locked;
        tadpole.set_age(0, now);
        tadpole.lock_until = now + LOCK_TICKS;
        used.mob.state = write(&tadpole);
        if tadpole.age_locked {
            used.mob.body.flags.persistent = true;
        }
        let block = physics::containing(used.position);
        used.did.push(Did::Sound {
            sound: if tadpole.age_locked {
                sounds::ITEM_GOLDEN_DANDELION_USE
            } else {
                sounds::ITEM_GOLDEN_DANDELION_UNUSE
            },
            source: SOUND_SOURCE_PLAYERS,
            at: [
                f64::from(block.x) + 0.5,
                f64::from(block.y) + 0.5,
                f64::from(block.z) + 0.5,
            ],
            volume: 1.0,
            pitch: 1.0,
            but_the_player: false,
        });
        return Interaction::Success;
    }
    water::bucket_pickup_sounding(used, sounds::ITEM_BUCKET_FILL_TADPOLE)
        .unwrap_or(Interaction::Pass)
}

/// `Tadpole.aiStep` after `AbstractFish`'s: a tick older unless locked, grown up at
/// 24000, then its lock's particles every second tick of their timer (three draws).
fn after_ai_step(tick: &mut MobTick<'_>) {
    let now = tick.lived;
    let next = now.saturating_add(1);
    let tadpole = read(&tick.mob.state);
    if !tadpole.age_locked && tadpole.age(next) >= TICKS_TO_BE_FROG {
        // `setAge(age + 1)` at the boundary, which the record keeps.
        update(&mut tick.mob, |tadpole| {
            let age = tadpole.age(next);
            tadpole.set_age(age, next);
        });
        tick.out.local.push(Local::GrowUp);
    }
    let lock = left(tadpole.lock_until, now);
    if lock > 0 && lock % 2 == 0 {
        for _ in 0..3 {
            tick.draws.next_double();
        }
    }
}

/// The tadpole.
pub(crate) struct Tadpole;

impl MobType for Tadpole {
    /// `AbstractFish.registerGoals` (it schools with nobody), its navigation, its malus
    /// and its constructor's controls (`SmoothSwimmingMoveControl(85, 10, 0.02, 0.1,
    /// true)`, `SmoothSwimmingLookControl(10)`).
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::WaterBound { breach: false });
        setup.set_malus(PathType::Water, 0.0);
        setup.config.move_kind = MoveKind::SmoothSwimming {
            max_turn_x: 85,
            max_turn_y: 10,
            in_water: 0.02,
            outside_water: 0.1,
            gravity: true,
        };
        setup.config.look_kind = LookKind::SmoothSwimming {
            max_y_rot_from_center: 10,
        };
        setup.add_goal(0, PanicGoal { speed: 1.25 });
        setup.add_goal(
            2,
            AvoidEntityGoal {
                class: Class::Player,
                max_distance: 8.0,
                walk_speed: 1.6,
                sprint_speed: 1.4,
            },
        );
        setup.add_goal(4, FishSwimGoal);
    }

    fn brain(&self) -> Option<&'static crate::mobs::brain::Provider> {
        Some(&super::tadpole_ai::PROVIDER)
    }

    /// `customServerAiStep`: its brain, then `AbstractFish`'s, which is nothing.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        super::tadpole_ai::step(tick);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        water::handle_air_supply(tick, air);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        fish::flop(tick, sounds::ENTITY_TADPOLE_FLOP);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        after_ai_step(tick);
    }

    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        fish::travel(tick)
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// `setFromBucket` does nothing: `fromBucket()` is always true.
    fn poured_from_bucket(&self, _: u16, _: &mut Mob) {}

    /// Never: `fromBucket()` is always true.
    fn remove_when_far_away(&self, _: u16, _: &Mob, _: u32, _: f64) -> bool {
        false
    }

    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        false
    }

    /// `shouldDropExperience()`: none, and nothing drawn for it.
    fn experience_reward(&self, _: u16, _: &Mob, _: &mut Random) -> i32 {
        0
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_TADPOLE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_TADPOLE_DEATH)
    }

    /// `FROM_BUCKET`, never set, and its age lock.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let locked = read(&mob.state).age_locked;
        vec![
            (FROM_BUCKET_INDEX, DataShown::Boolean(false), true),
            (AGE_LOCKED_INDEX, DataShown::Boolean(locked), !locked),
        ]
    }
}
