//! The squid (`Squid` with `$SquidRandomMovementGoal` and `$SquidFleeGoal`;
//! `docs/groundwork/n7-brief.md`, sections 6.8.1 to 6.8.5): an ageable water creature
//! with no navigation and no move control. Its two goals set a movement vector that its
//! `aiStep` copies into its motion in the last quarter of the first half of each stroke
//! of its tentacles, and lets coast at nine tenths a tick in the second; its `travel` is
//! a bare move of its motion. Out of water it falls straight down. Hurt by a living
//! thing, it squirts its ink. The glow squid's module builds on this.
//!
//! Its tentacles' phase counts by its speed every tick, so it is kept as the phase at a
//! stroke's start and the tick from which it counts: the stroke's ticks are added again
//! one by one in floats, as the game adds them, and the record changes only at a stroke's
//! end, when the speed may be drawn again.
//!
//! Named differences: the body's pitch and roll and the tentacles' angle, which only a
//! client draws by and the ink's particles would be turned by, are not kept (the server
//! sends neither, so nothing of them is seen); the ink's particles are drawn and not
//! sent (Clustine sends no particles yet); levitation, which would lift a squid on land,
//! is not ported (no mob effects until N9).

use clustine_data::{EntityTypeInfo, entity_types, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Dimensions, Interaction, MobType, Used, water};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{Items, no_action_time};
use crate::mobs::ai::maths::{reduced_tick_delay, table_cos, table_sin};
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::tick::MobTick;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// A baby squid's size, a glow squid's too (`BABY_DIMENSIONS`).
pub(crate) const BABY: Dimensions = Dimensions {
    width: 0.5,
    height: 0.5,
    eye_height: 0.37,
};

/// The float of two pi the strokes count to and take off (`6.2831855f`), and pi.
const TWO_PI: f32 = 6.283_185_5;
const PI: f32 = std::f32::consts::PI;

/// The synced index of a glow squid's dark ticks.
const DARK_TICKS_INDEX: u8 = 18;

/// What a squid keeps beyond an ageable mob: its tentacles' phase (`tentacleMovement`)
/// as the tick of `phase_from` finds it, counted on by `tentacle_speed` a tick since; its
/// movement vector; and a glow squid's dark ticks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SquidState {
    pub(crate) phase: f32,
    pub(crate) phase_from: u32,
    pub(crate) tentacle_speed: f32,
    pub(crate) movement: Vec3,
    pub(crate) dark_ticks: i32,
}

impl Own for SquidState {}

impl SquidState {
    /// `tentacleMovement` as the tick that begins at `lived` finds it: the phase with
    /// the speed added once for each tick from `phase_from` up to that one, in floats.
    pub(crate) fn tentacle_movement(&self, lived: u32) -> f32 {
        let steps = (i64::from(lived) - i64::from(self.phase_from)).max(0);
        let mut phase = self.phase;
        for _ in 0..steps {
            phase += self.tentacle_speed;
        }
        phase
    }
}

/// `1.0f / (nextFloat() + 1.0f) * 0.2f`: a stroke's speed, between a tenth and a fifth.
fn tentacle_speed(draws: &mut Random) -> f32 {
    1.0 / (draws.next_float() + 1.0) * 0.2
}

/// A squid's or a glow squid's state.
pub(crate) fn read(mob: &Mob) -> (animal::Animal, SquidState) {
    animal::read::<SquidState>(&mob.state)
}

fn write(mob: &mut Mob, shared: &animal::Animal, own: &SquidState) {
    mob.state = animal::write(shared, own);
}

/// Changes a squid's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut SquidState)) {
    let (shared, mut own) = read(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        write(mob, &shared, &own);
    }
}

/// `Squid`'s constructor: its first stroke's speed, drawn after `Mob`'s constructor.
pub(crate) fn construct(mob: &mut Mob, own: &mut Random) {
    let speed = tentacle_speed(own);
    update(mob, |squid| squid.tentacle_speed = speed);
}

/// `Squid.registerGoals`: the random movement at 0 and the flight at 1, no flags on
/// either; its ground navigation is never asked anything.
pub(crate) fn register_goals(setup: &mut Setup) {
    setup.add_goal(0, RandomMovementGoal);
    setup.add_goal(1, FleeGoal);
}

/// `Vec3.normalize()`: nothing for a vector shorter than `1.0E-5`.
fn normalize(v: Vec3) -> Vec3 {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length < 1.0e-5 {
        [0.0; 3]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

/// `SquidRandomMovementGoal`: idle for 100 ticks it stops; else, one time in 25 or
/// whenever it is out of water or still, a new way to swim. Never ends.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RandomMovementGoal;

impl Goal for RandomMovementGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        true
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if no_action_time(ai) > 100 {
            update(&mut ai.tick.mob, |squid| squid.movement = [0.0; 3]);
            return;
        }
        let draws = &mut *ai.tick.draws;
        let chance = draws.next_int(reduced_tick_delay(50) as u32) == 0;
        let (_, own) = read(&ai.tick.mob);
        let still = physics::length_sqr(own.movement) <= 9.999_999_747_378_752e-6;
        if chance || !ai.tick.living.body.in_water || still {
            let draws = &mut *ai.tick.draws;
            let angle = draws.next_float() * TWO_PI;
            let x = table_cos(angle) * 0.2;
            let y = -0.1 + draws.next_float() * 0.2;
            let z = table_sin(angle) * 0.2;
            let movement = [f64::from(x), f64::from(y), f64::from(z)];
            update(&mut ai.tick.mob, |squid| squid.movement = movement);
        }
    }
}

/// `SquidFleeGoal`'s state: its ticks of fleeing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Fleeing {
    pub(crate) ticks: i32,
}

/// `SquidFleeGoal`: in water, from what last hurt it while it is within ten blocks, away
/// at up to three blocks a second, never up into the air. Every tick.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FleeGoal;

impl FleeGoal {
    fn attacker(ai: &Ai<'_, '_>) -> Option<sensing::Looked> {
        let (key, _) = ai.tick.mob.body.combat.last_hurt_by?;
        sensing::find(ai, key)
    }
}

impl Goal for FleeGoal {
    type State = Fleeing;

    fn flags(&self) -> u8 {
        0
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut Fleeing, ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.living.body.in_water
            && Self::attacker(ai)
                .is_some_and(|attacker| attacker.distance_sqr(ai.position()) < 100.0)
    }

    fn start(&self, state: &mut Fleeing, _: &mut Ai<'_, '_>) {
        state.ticks = 0;
    }

    fn tick(&self, state: &mut Fleeing, ai: &mut Ai<'_, '_>) {
        state.ticks += 1;
        let Some(attacker) = Self::attacker(ai) else {
            return;
        };
        let at = ai.position();
        let mut away = physics::sub(at, attacker.position);
        let ahead = physics::containing(physics::add(at, away));
        let world = ai.tick.world;
        let block = world.state(ahead);
        let water = clustine_blocks::data::fluid_in_tag(block, "minecraft:water");
        let air = block.props().is_air();
        if water || air {
            let length = (away[0] * away[0] + away[1] * away[1] + away[2] * away[2]).sqrt();
            if length > 0.0 {
                away = normalize(away);
                let mut speed = 3.0;
                if length > 5.0 {
                    speed -= (length - 5.0) / 5.0;
                }
                if speed > 0.0 {
                    away = physics::scale(away, speed);
                }
            }
            if air {
                away[1] = 0.0;
            }
            let movement = [away[0] / 20.0, away[1] / 20.0, away[2] / 20.0];
            update(&mut ai.tick.mob, |squid| squid.movement = movement);
        }
        // The bubbles of every tenth tick are a client's (`level.addParticle`, which
        // draws nothing).
    }
}

/// `Squid.aiStep()` after the ageable mob's: the tentacles stroke on, and at a stroke's
/// end draw whether to change their speed and show the stroke (entity event 19); in
/// water the movement vector is taken on in the stroke's last quarter of its first half
/// and the motion slows by nine tenths in its second, the body facing where it goes; out
/// of water it falls straight down.
pub(crate) fn stroke(tick: &mut MobTick<'_>) {
    let lived = tick.lived;
    let (shared, mut own) = read(&tick.mob);
    let before = own.clone();
    let mut tentacle = own.tentacle_movement(lived) + own.tentacle_speed;
    // `6.283185307179586`, two pi as a double.
    if f64::from(tentacle) > std::f64::consts::TAU {
        tentacle -= TWO_PI;
        if tick.draws.next_int(10) == 0 {
            own.tentacle_speed = tentacle_speed(tick.draws);
        }
        tick.out.events.push(Event::Entity(19));
        own.phase = tentacle;
        own.phase_from = lived.saturating_add(1);
    }
    if tick.living.body.in_water {
        if tentacle < PI {
            let f = tentacle / PI;
            if f64::from(f) > 0.75 {
                tick.living.body.motion = own.movement;
            }
        } else {
            tick.living.body.motion = physics::scale(tick.living.body.motion, 0.9);
        }
        let motion = tick.living.body.motion;
        let facing = -(crate::mobs::ai::maths::atan2(motion[0], motion[2]) as f32) * 57.295_776;
        let body = &mut tick.mob.body.head.body_yaw;
        *body += (facing - *body) * 0.1;
        tick.yaw = *body;
    } else {
        let y = tick.living.body.motion[1] - tick.living.attributes.gravity;
        let drag = f64::from(tick.living.air_drag());
        tick.living.body.motion = [0.0, y * drag, 0.0];
    }
    if own != before {
        write(&mut tick.mob, &shared, &own);
    }
}

/// `Squid.hurtServer` after the living one: hurt by a living thing, it squirts its ink
/// (`spawnInk`): its squirt's sound (two draws), and thirty particles of three draws
/// each, drawn and not sent.
pub(crate) fn squirt(struck: &mut Struck<'_>, took: bool, sound: i32) {
    if !took || struck.mob.body.combat.last_hurt_by.is_none() {
        return;
    }
    struck.make_sound(Some(sound));
    for _ in 0..30 {
        struck.draws.next_float();
        struck.draws.next_float();
        struck.draws.next_float();
    }
}

/// `getBreedOffspring`: a new one of its kind, which nothing but an egg makes.
pub(crate) fn offspring() -> Mob {
    Mob::fresh()
}

/// What an ageable water creature does of a use: `AgeableMob.mobInteract`, a golden
/// dandelion on a baby; no food makes it breed.
pub(crate) fn mob_interact(used: &mut Used<'_>) -> Interaction {
    animal::mob_interact(used, Items::Of(&[]), None)
}

/// The synced values of a squid: the ageable mob's, and a glow squid's dark ticks.
pub(crate) fn shown(mob: &Mob, glows: bool) -> Vec<(u8, DataShown, bool)> {
    let mut values = animal::shown(mob);
    if glows {
        let (_, own) = read(mob);
        values.push((
            DARK_TICKS_INDEX,
            DataShown::Int(own.dark_ticks),
            own.dark_ticks == 0,
        ));
    }
    values
}

/// The squid.
pub(crate) struct Squid;

impl MobType for Squid {
    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup);
    }

    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        construct(mob, own);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        water::handle_air_supply(tick, air);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        stroke(tick);
    }

    fn travel(&self, _: &MobTick<'_>) -> Travel {
        Travel::Bare
    }

    fn after_hurt(&self, struck: &mut Struck<'_>, _: &DamageSource, took: bool) {
        squirt(struck, took, sounds::ENTITY_SQUID_SQUIRT);
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(offspring())
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SQUID_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SQUID_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SQUID_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        shown(mob, false)
    }
}

/// The squid's size as a grown one: its type's.
#[allow(dead_code, reason = "the tests' measure of a grown squid")]
pub(crate) fn grown() -> Dimensions {
    let info = EntityTypeInfo::of(entity_types::SQUID);
    Dimensions {
        width: info.width,
        height: info.height,
        eye_height: info.eye_height,
    }
}
