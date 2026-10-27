//! Ocelots (`Ocelot`, `OcelotTemptGoal`, `OcelotAvoidEntityGoal`, `OcelotAttackGoal`;
//! `docs/groundwork/n7-brief.md`, section 5.7): an animal of fish that runs from players
//! until it trusts one, which a fish held out within three blocks while it is tempted wins
//! one time in three; it hunts chickens and baby turtles, crouches while it creeps after
//! food, and breeds. What it shares with the cat (its attack, its crouch, its avoiding of
//! players) is here, and the cat's module uses it.
//!
//! The avoiding of players is the game's goal removed and added again as trust changes
//! (`reassessTrustingGoals`), so last among its goals while it is not trusting: here it
//! is always last and does nothing while trusting, and trust given stops it at once, as
//! its removal does.
//!
//! Named differences: its sprint (the shared flag of sprinting at 1.33) is not shown,
//! only its crouch.

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, Entry};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Used, cow, tamable};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, Avoiding, FloatGoal, Items, LeapAtTargetGoal, LookAtPlayerGoal,
    NearestAttackableTargetGoal, RandomStrollGoal, Stroll, TemptGoal, Tempted,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Looked};
use crate::mobs::ai::{Ai, Operation, Setup, controls, navigation};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::part::mind_ids;
use crate::mobs::tick::{Local, MobTick};
use crate::shown::DataShown;

/// `#ocelot_food`.
const FOOD: Items = Items::Tag("minecraft:ocelot_food");

/// A kitten's size, a cat's and an ocelot's.
pub(crate) const BABY: Dimensions = Dimensions {
    width: 0.3,
    height: 0.35,
    eye_height: 0.343_75,
};

/// The synced data's index of its trust.
const TRUSTING_INDEX: u8 = 18;

/// `Entity.DATA_POSE`'s index, and the ordinals of the poses a feline takes.
pub(crate) const POSE_INDEX: u8 = 6;
const STANDING: i32 = 0;
const CROUCHING: i32 = 5;

/// The speeds a feline crouches and sprints at (`CROUCH_SPEED_MOD`, `SPRINT_SPEED_MOD`).
const CROUCH_SPEED: f64 = 0.6;
const SPRINT_SPEED: f64 = 1.33;

/// The place of its tempt goal among its goals, whose state an interaction reads.
const TEMPT_SLOT: u16 = 1;

/// What an ocelot keeps beyond an animal: whether it trusts players, and whether it
/// crouches (its pose, not saved by the game).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct OcelotState {
    pub(crate) trusting: bool,
    pub(crate) crouching: bool,
}

impl Own for OcelotState {}

/// The ocelot.
pub(crate) struct Ocelot;

pub(crate) fn state_of(mob: &Mob) -> (animal::Animal, OcelotState) {
    animal::read::<OcelotState>(&mob.state)
}

/// `customServerAiStep` of a cat or an ocelot: where it wants to move, crouching at the
/// crouch's speed, standing at any other; standing where it wants nothing. Whether it
/// crouches.
pub(crate) fn crouches(tick: &MobTick<'_>) -> bool {
    let movement = &tick.mob.body.controls.movement;
    movement.operation == Operation::MoveTo && movement.speed == CROUCH_SPEED
}

/// The pose shown of a feline crouching or not.
pub(crate) fn pose_shown(crouching: bool) -> (u8, DataShown, bool) {
    let pose = if crouching { CROUCHING } else { STANDING };
    (POSE_INDEX, DataShown::Pose(pose), !crouching)
}

/// A goal's state as the mob's mind keeps it, by the goal's place among its goals.
pub(crate) fn goal_state<T: DeserializeOwned + Default>(mob: &Mob, slot: u16) -> T {
    let id = mind_ids::FIRST_GOAL + slot;
    mob.mind
        .slots
        .iter()
        .find(|entry: &&Entry| entry.id == id && entry.version == 1)
        .and_then(|entry| postcard::from_bytes(&entry.bytes).ok())
        .unwrap_or_default()
}

/// `GoalSelector.removeGoal(goal)` of the goal at `slot` of the goal selector: stopped
/// where it runs, which for an `AvoidEntityGoal` forgets what it ran from; its flags let
/// go.
pub(crate) fn remove_goal(mob: &mut Mob, slot: u16) {
    let selector = &mut mob.mind.goals.0;
    if let Ok(at) = selector.running.binary_search(&slot) {
        selector.running.remove(at);
        let id = mind_ids::FIRST_GOAL + slot;
        mob.mind.slots.retain(|entry| entry.id != id);
    }
    for lock in &mut selector.locks {
        if *lock == Some(slot) {
            *lock = None;
        }
    }
}

/// `OcelotAttackGoal(mob)`: after its target at a crouch, a walk or a sprint by how far,
/// and a hit within reach once a second.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OcelotAttackGoal;

/// `OcelotAttackGoal`'s state: its target and the ticks to its next hit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pouncing {
    pub(crate) target: Option<clustine_world::EntityId>,
    pub(crate) attack_time: i32,
}

impl Goal for OcelotAttackGoal {
    type State = Pouncing;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Pouncing, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        state.target = Some(target.key);
        true
    }

    fn can_continue(&self, state: &mut Pouncing, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = state
            .target
            .and_then(|key| tamable::find(ai.tick.world, key))
        else {
            return false;
        };
        if !target.alive {
            return false;
        }
        if target.distance_sqr(ai.position()) > 225.0 {
            return false;
        }
        !navigation::is_done(ai) || self.can_use(state, ai)
    }

    fn stop(&self, state: &mut Pouncing, ai: &mut Ai<'_, '_>) {
        state.target = None;
        navigation::stop(ai);
    }

    fn tick(&self, state: &mut Pouncing, ai: &mut Ai<'_, '_>) {
        let Some(target) = state
            .target
            .and_then(|key| tamable::find(ai.tick.world, key))
        else {
            return;
        };
        controls::set_look_at(
            ai,
            [target.position[0], target.eye_y, target.position[2]],
            30.0,
            30.0,
        );
        let width = ai.width() * 2.0;
        let reach = f64::from(width * width);
        let distance = target.distance_sqr(ai.position());
        let speed = if distance > reach && distance < 16.0 {
            SPRINT_SPEED
        } else if distance < 225.0 {
            CROUCH_SPEED
        } else {
            0.8
        };
        navigation::move_to_entity(ai, target.block_position(), speed);
        state.attack_time = (state.attack_time - 1).max(0);
        if distance > reach || state.attack_time > 0 {
            return;
        }
        state.attack_time = 20;
        let amount = ai.attribute(clustine_data::attributes::ATTACK_DAMAGE) as f32;
        ai.tick.out.local.push(Local::Hurt {
            key: target.key,
            kind: ai.tick.kind,
            from: ai.position(),
            amount,
            sound: None,
            knockback: None,
            fling: None,
        });
    }
}

/// An avoiding of players that a feline's state switches off (`CatAvoidEntityGoal`,
/// `OcelotAvoidEntityGoal`): `NO_CREATIVE_OR_SPECTATOR` players within 16, walked from at
/// 0.8 and sprinted from at 1.33, while `active` says so.
#[derive(Clone, Copy)]
pub(crate) struct AvoidPlayersGoal {
    pub(crate) active: fn(&Mob) -> bool,
}

const AVOID_PLAYERS: AvoidEntityGoal = AvoidEntityGoal {
    class: Class::Player,
    max_distance: 16.0,
    walk_speed: 0.8,
    sprint_speed: SPRINT_SPEED,
};

impl Goal for AvoidPlayersGoal {
    type State = Avoiding;

    fn flags(&self) -> u8 {
        AVOID_PLAYERS.flags()
    }

    fn can_use(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        (self.active)(&ai.tick.mob) && AVOID_PLAYERS.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        (self.active)(&ai.tick.mob) && AVOID_PLAYERS.can_continue(state, ai)
    }

    fn start(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        AVOID_PLAYERS.start(state, ai);
    }

    fn stop(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        AVOID_PLAYERS.stop(state, ai);
    }

    fn tick(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        AVOID_PLAYERS.tick(state, ai);
    }
}

/// `OcelotTemptGoal(ocelot, 0.6, food, true)`: a tempt that scares only an ocelot not
/// trusting.
#[derive(Clone, Copy)]
struct OcelotTemptGoal;

const TEMPT: TemptGoal = TemptGoal {
    speed: CROUCH_SPEED,
    items: FOOD,
    can_scare: true,
    stop_distance: 2.5,
};

impl Goal for OcelotTemptGoal {
    type State = Tempted;

    fn flags(&self) -> u8 {
        TEMPT.flags()
    }

    fn can_use(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) -> bool {
        TEMPT.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) -> bool {
        let trusting = state_of(&ai.tick.mob).1.trusting;
        TemptGoal {
            can_scare: !trusting,
            ..TEMPT
        }
        .can_continue(state, ai)
    }

    fn start(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        TEMPT.start(state, ai);
    }

    fn stop(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        TEMPT.stop(state, ai);
    }

    fn tick(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        TEMPT.tick(state, ai);
    }
}

/// `WaterAvoidingRandomStrollGoal(mob, 0.8, 1.0000001E-5)`: a feline's stroll.
pub(crate) fn feline_stroll() -> RandomStrollGoal {
    RandomStrollGoal {
        kind: Stroll::AvoidingWater {
            probability: 1.000_000_1e-5,
        },
        ..RandomStrollGoal::new(0.8)
    }
}

/// Whether an ocelot runs from players: while it does not trust them.
fn wary(mob: &Mob) -> bool {
    !state_of(mob).1.trusting
}

/// `spawnTrustingParticles(trusting)`: seven particles drawn and not sent (the server
/// sends none; the event shows them).
fn trusting_particle_draws(used: &mut Used<'_>) {
    for _ in 0..7 {
        for _ in 0..3 {
            used.draws.next_gaussian();
        }
        for _ in 0..3 {
            used.draws.next_double();
        }
    }
}

/// The place of its avoiding of players among its goals: after its seven.
const AVOID_SLOT: u16 = 7;

impl MobType for Ocelot {
    /// `Ocelot.finalizeSpawn`: a group every later one of which is a kitten
    /// (`AgeableMobGroupData(1.0f)`) where it has none.
    fn finalize_spawn(&self, spawn: &mut super::Spawning<'_>) {
        let group = super::SpawnGroup::Ageable(super::AgeableGroup::with_chance(1.0));
        animal::finalize_ageable(spawn, group);
    }

    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.add_goal(1, FloatGoal);
        setup.add_goal(3, OcelotTemptGoal);
        setup.add_goal(7, LeapAtTargetGoal { yd: 0.3 });
        setup.add_goal(8, OcelotAttackGoal);
        setup.add_goal(9, animal::BreedGoal { speed: 0.8 });
        setup.add_goal(10, feline_stroll());
        setup.add_goal(11, LookAtPlayerGoal::new(Class::Player, 10.0));
        setup.add_goal(4, AvoidPlayersGoal { active: wary });
        setup.add_target(
            1,
            NearestAttackableTargetGoal::new(Class::Kinds(&[entity_types::CHICKEN as u16]), false),
        );
        setup.add_target(1, BabyTurtleTargetGoal);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let crouching = crouches(tick);
        let (shared, mut own) = state_of(&tick.mob);
        if own.crouching != crouching {
            own.crouching = crouching;
            tick.mob.state = animal::write(&shared, &own);
        }
        // Its own replaces `Animal`'s, whose end of love does not come here.
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Ocelot.mobInteract`: its food held out within three blocks while it is tempted
    /// (or has no tempt goal) wins its trust one time in three; else an animal's use.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let (shared, mut own) = state_of(used.mob);
        let tempted = goal_state::<Tempted>(used.mob, TEMPT_SLOT).running;
        let near = tamable::distance_sqr(used.from, used.position) < 9.0;
        if tempted && !own.trusting && FOOD.test(used.held.as_ref()) && near {
            if used.draws.next_int(3) == 0 {
                own.trusting = true;
                used.mob.state = animal::write(&shared, &own);
                remove_goal(used.mob, AVOID_SLOT);
                trusting_particle_draws(used);
                used.did.push(Did::Event(41));
            } else {
                trusting_particle_draws(used);
                used.did.push(Did::Event(40));
            }
            return Interaction::Success;
        }
        animal::mob_interact(used, FOOD, None)
    }

    /// `Ocelot.getBreedOffspring`: a plain ocelot.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_OCELOT_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        900
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_OCELOT_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_OCELOT_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// `removeWhenFarAway`: one not trusting, after 2400 ticks.
    fn remove_when_far_away(&self, _: u16, mob: &Mob, tick_count: u32, _: f64) -> bool {
        !state_of(mob).1.trusting && tick_count > 2400
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = state_of(mob);
        let mut values = animal::shown(mob);
        values.push(pose_shown(own.crouching));
        values.push((
            TRUSTING_INDEX,
            DataShown::Boolean(own.trusting),
            !own.trusting,
        ));
        values
    }
}

/// `NearestAttackableTargetGoal(mob, Turtle, 10, false, false, BABY_ON_LAND_SELECTOR)`.
#[derive(Clone, Copy)]
pub(crate) struct BabyTurtleTargetGoal;

const TURTLES: NearestAttackableTargetGoal = NearestAttackableTargetGoal {
    class: Class::Kinds(&[entity_types::TURTLE as u16]),
    interval: 10,
    must_see: false,
    must_reach: false,
    selector: None,
};

impl Goal for BabyTurtleTargetGoal {
    type State = crate::mobs::ai::goals::Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) -> bool {
        let interval = crate::mobs::ai::maths::reduced_tick_delay(10);
        if ai.tick.draws.next_int(interval as u32) != 0 {
            return false;
        }
        let world = ai.tick.world;
        let found = tamable::nearest_target(ai, TURTLES.class, &|looked: &Looked| {
            tamable::baby_turtle_on_land(world, looked)
        });
        state.found = found.map(|found| found.key);
        state.found.is_some()
    }

    fn can_continue(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) -> bool {
        TURTLES.can_continue(state, ai)
    }

    fn start(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        TURTLES.start(state, ai);
    }

    fn stop(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        TURTLES.stop(state, ai);
    }
}
