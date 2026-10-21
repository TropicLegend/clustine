//! Polar bears (`PolarBear` and its nested classes; `docs/groundwork/n7-brief.md`,
//! section 6.3): a neutral animal that no food breeds, whose adults hunt foxes and
//! defend cubs, standing up before a bite. Its anger is `NeutralMob`'s ([`neutral`]),
//! drawn again every tick it has a target (finding F3).
//!
//! Named differences: a hurt cub's alert of the adults near it (`alertOthers`) writes
//! their targets, which is not built, so a cub's attacker is not set on; no player is
//! ever a bear's target (every player is creative, finding F4), so its player goals only
//! draw; the standing box is the client's (finding F9), as on the game's server; the
//! fire, lava and the like of `#panic_environmental_causes` are not seen, so a grown
//! bear never panics.

use clustine_data::{entity_types, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::neutral::{self, Anger};
use super::tamable::Idle;
use super::{Breeding, Dimensions, Interaction, MobType, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    Attacking, FloatGoal, HurtByTargetGoal, Items, LookAtPlayerGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, Panic, PanicGoal, RandomLookAroundGoal, RandomStrollGoal,
    Targeting, ai_bounds, melee_hit,
};
use crate::mobs::ai::maths::reduced_tick_delay;
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::{self, Class};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;
use clustine_world::BlockPos;

/// A cub's size (`BABY_DIMENSIONS`).
const BABY: Dimensions = Dimensions {
    width: 0.7,
    height: 0.7,
    eye_height: 0.343_75,
};

/// The synced data's index of its standing (`DATA_STANDING_ID`).
const STANDING_INDEX: u8 = 18;

/// What a polar bear keeps beyond an animal: whether it stands, its warning's timer,
/// and its anger.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BearState {
    pub(crate) standing: bool,
    /// `warningSoundTicks`.
    pub(crate) warning_ticks: i32,
    pub(crate) anger: Anger,
}

impl Own for BearState {}

/// The polar bear.
pub(crate) struct PolarBear;

pub(crate) fn state_of(mob: &Mob) -> BearState {
    animal::read::<BearState>(&mob.state).1
}

fn put_state(mob: &mut Mob, own: &BearState) {
    let animal = animal::animal_of(&mob.state);
    mob.state = animal::write(&animal, own);
}

/// `PolarBearMeleeAttackGoal`: a melee attack at 1.25 that stands up and warns before
/// the bite and sits down after it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BearAttackGoal;

impl BearAttackGoal {
    const MELEE: MeleeAttackGoal = MeleeAttackGoal {
        speed: 1.25,
        following_even_if_not_seen: true,
    };
}

/// `setStanding(standing)`.
fn set_standing(ai: &mut Ai<'_, '_>, standing: bool) {
    let mut own = state_of(&ai.tick.mob);
    if own.standing != standing {
        own.standing = standing;
        put_state(&mut ai.tick.mob, &own);
    }
}

/// `playWarningSound()`: at most every 40 ticks, at its volume and voice.
fn play_warning_sound(ai: &mut Ai<'_, '_>) {
    let mut own = state_of(&ai.tick.mob);
    if own.warning_ticks > 0 {
        return;
    }
    let pitch = crate::mobs::body::hurt::voice_pitch(ai.tick.draws, animal::is_baby(&ai.tick.mob));
    ai.tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_POLAR_BEAR_WARNING,
        volume: 1.0,
        pitch,
    });
    own.warning_ticks = 40;
    put_state(&mut ai.tick.mob, &own);
}

impl Goal for BearAttackGoal {
    type State = Attacking;

    fn flags(&self) -> u8 {
        Self::MELEE.flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::MELEE.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::MELEE.can_continue(state, ai)
    }

    fn start(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        Self::MELEE.start(state, ai);
    }

    fn stop(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        set_standing(ai, false);
        Self::MELEE.stop(state, ai);
    }

    fn tick(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = Self::MELEE.chase(state, ai) else {
            return;
        };
        // `checkAndPerformAttack`.
        if MeleeAttackGoal::can_perform_attack(state, ai, &target) {
            Self::MELEE.reset_attack_cooldown(state);
            melee_hit(ai, &target);
            set_standing(ai, false);
            return;
        }
        let width = (target.bounds.max[0] - target.bounds.min[0]) as f32;
        let near = (width + 3.0) * (width + 3.0);
        if target.distance_sqr(ai.position()) < f64::from(near) {
            if state.attack_cooldown <= 0 {
                set_standing(ai, false);
                Self::MELEE.reset_attack_cooldown(state);
            }
            if state.attack_cooldown <= 10 {
                set_standing(ai, true);
                play_warning_sound(ai);
            }
        } else {
            Self::MELEE.reset_attack_cooldown(state);
            set_standing(ai, false);
        }
    }
}

/// `PanicGoal(bear, 2.0, mob -> isBaby ? #panic_causes : #panic_environmental_causes)`:
/// a cub panics at a hit; a grown bear only at what the body does not see yet.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BearPanicGoal;

impl BearPanicGoal {
    const PANIC: PanicGoal = PanicGoal { speed: 2.0 };
}

impl Goal for BearPanicGoal {
    type State = Panic;

    fn flags(&self) -> u8 {
        Self::PANIC.flags()
    }

    fn panics(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        animal::is_baby(&ai.tick.mob) && Self::PANIC.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        Self::PANIC.can_continue(state, ai)
    }

    fn start(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        Self::PANIC.start(state, ai);
    }

    fn stop(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        Self::PANIC.stop(state, ai);
    }
}

/// `PolarBearHurtByTargetGoal`: who hurt it, but a cub stops at once once it has (its
/// alert of the grown bears is named).
#[derive(Debug, Clone, Copy)]
pub(crate) struct BearHurtByTargetGoal;

impl BearHurtByTargetGoal {
    const HURT_BY: HurtByTargetGoal = HurtByTargetGoal { ignore: &[] };
}

impl Goal for BearHurtByTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        Self::HURT_BY.flags()
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::HURT_BY.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::HURT_BY.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::HURT_BY.start(state, ai);
        if animal::is_baby(&ai.tick.mob) {
            Self::HURT_BY.stop(state, ai);
        }
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::HURT_BY.stop(state, ai);
    }
}

/// `PolarBearAttackPlayersGoal`: a grown bear goes for a player near a cub. Every player
/// is creative, so it only draws.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AttackPlayersGoal;

impl AttackPlayersGoal {
    const NEAREST: NearestAttackableTargetGoal = NearestAttackableTargetGoal {
        class: Class::Player,
        interval: 20,
        must_see: true,
        must_reach: true,
        selector: None,
    };
}

impl Goal for AttackPlayersGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        Self::NEAREST.flags()
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        if animal::is_baby(&ai.tick.mob) || !Self::NEAREST.can_use(state, ai) {
            return false;
        }
        let bounds = ai_bounds(ai).inflate([8.0, 4.0, 8.0]);
        let bear = &[entity_types::POLAR_BEAR as u16][..];
        sensing::things_in(ai, bounds, Class::Kinds(bear))
            .iter()
            .any(|other| {
                ai.tick
                    .world
                    .entity(other.key)
                    .and_then(|seen| match &seen.entity.data {
                        clustine_world::EntityData::Mob { state, .. } => {
                            Some(animal::animal_of(state).is_baby())
                        }
                        _ => None,
                    })
                    .unwrap_or(false)
            })
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::NEAREST.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::NEAREST.start(state, ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::NEAREST.stop(state, ai);
    }
}

/// `NearestAttackableTargetGoal<Fox>(bear, 10, true, true, (fox, level) -> !isBaby())`: a
/// grown bear hunts the nearest fox it can reach; a cub's selector refuses every fox, so
/// it draws its chance and finds none.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HuntFoxesGoal;

impl HuntFoxesGoal {
    const NEAREST: NearestAttackableTargetGoal = NearestAttackableTargetGoal {
        class: Class::Kinds(&[entity_types::FOX as u16]),
        interval: 10,
        must_see: true,
        must_reach: true,
        selector: None,
    };
}

impl Goal for HuntFoxesGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        Self::NEAREST.flags()
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        if animal::is_baby(&ai.tick.mob) {
            let interval = reduced_tick_delay(Self::NEAREST.interval);
            if interval > 0 {
                ai.tick.draws.next_int(interval as u32);
            }
            state.found = None;
            return false;
        }
        Self::NEAREST.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::NEAREST.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::NEAREST.start(state, ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::NEAREST.stop(state, ai);
    }
}

/// `TimeUtil.rangeOfSeconds(20, 39).sample(random)`: anger of 400 to 780 ticks, one draw.
pub(crate) fn anger_time(draws: &mut Random) -> i64 {
    400 + i64::from(draws.next_int(381))
}

impl MobType for PolarBear {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, BearAttackGoal);
        setup.add_goal(1, BearPanicGoal);
        setup.add_goal(4, animal::FollowParentGoal { speed: 1.25 });
        setup.add_goal(5, RandomStrollGoal::new(1.0));
        setup.add_goal(6, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(7, RandomLookAroundGoal);
        setup.add_target(1, BearHurtByTargetGoal);
        setup.add_target(2, AttackPlayersGoal);
        // `this::isAngryAt`: no player is ever one.
        setup.add_target(3, NearestAttackableTargetGoal::new(Class::Player, true));
        setup.add_target(4, HuntFoxesGoal);
        // `ResetUniversalAngerTargetGoal(false)`: only with `universal_anger`, which is off.
        setup.add_target(5, Idle { flags: 0 });
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    /// `Animal.aiStep`, then `PolarBear.tick`'s own after the living tick: the
    /// warning's timer down, and the anger brought up to date, its timer drawn again
    /// every tick it has a target.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        let mut own = state_of(&tick.mob);
        let before = own.clone();
        if own.warning_ticks > 0 {
            own.warning_ticks -= 1;
        }
        let now = tick.world.game_time();
        neutral::update_persistent_anger(
            &mut tick.mob,
            &mut own.anger,
            tick.world,
            now,
            true,
            tick.draws,
            anger_time,
        );
        if own != before {
            put_state(&mut tick.mob, &own);
        }
    }

    /// No food breeds it (`isFood` is false); a golden dandelion still locks a cub.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        animal::mob_interact(used, Items::Of(&[]), None)
    }

    /// `getBreedOffspring`: a cub, as an egg used on a bear makes one.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn water_slow_down(&self) -> f32 {
        0.98
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_POLAR_BEAR_AMBIENT_BABY
        } else {
            sounds::ENTITY_POLAR_BEAR_AMBIENT
        })
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_POLAR_BEAR_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_POLAR_BEAR_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((
            STANDING_INDEX,
            DataShown::Boolean(own.standing),
            !own.standing,
        ));
        values
    }
}
