//! What the guardian and the elder guardian share (`Guardian`, its move control and
//! goals; `n7k-brief.md` 3.8, `docs/groundwork/n9-plan.md` 8.4), parts K5 and N9's S7: a
//! monster of the water that swims where its water-bound navigation takes it, keeps near
//! its home, looks about, flops on land, and whose spikes hurt what hits it while it
//! keeps still. To a target it keeps still and turns its body to its head, and its beam
//! (`GuardianAttackGoal`, [`GuardianAttackGoal`]) shows from ten ticks on (the target in
//! `DATA_ID_ATTACK_TARGET`, data 17, and entity event 21), and strikes at 80 (the
//! elder's at 60): magic of 1 (the elder's 3), then its bite.
//!
//! Named differences: the tail's and the spikes' animations, its bubbles and the flop's
//! sound are its clients' own (the flop is heard by them, `playLocalSound`); and the
//! spikes hurt their attacker in the region's order of things, after the guardian's own
//! hurt, where the game hurts the attacker first.

use clustine_blocks::data::fluid_in_tag;
use clustine_data::{PathType, attributes as ids, entity_types};
use clustine_world::{BlockPos, EntityId, TypeState};
use serde::{Deserialize, Serialize};

use crate::living::{Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::ai::goals::look_at_thing;
use crate::mobs::ai::goals::{
    LookAtPlayerGoal, MoveTowardsRestrictionGoal, RandomLookAroundGoal, RandomStrollGoal,
    Returning, Strolling,
};
use crate::mobs::ai::maths::{hotspot_cos, hotspot_sin, lerp, move_rotlerp, yaw_towards};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class};
use crate::mobs::ai::{Ai, HeadLimits, Operation, Setup, controls};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::{DamageSource, DamageType, Struck};
use crate::mobs::tick::{Local, MobTick};
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a guardian's type state: 2 with the beam's target.
const VERSION: u8 = 2;

/// The synced data's index of `DATA_ID_MOVING`.
pub(crate) const MOVING_INDEX: u8 = 16;
/// The synced data's index of `DATA_ID_ATTACK_TARGET`: the beam's target's id, 0 for
/// none.
pub(crate) const ATTACK_TARGET_INDEX: u8 = 17;

/// The guardians, the class `Guardian` takes in (the elder is one).
const GUARDIANS: [u16; 2] = [
    entity_types::GUARDIAN as u16,
    entity_types::ELDER_GUARDIAN as u16,
];

/// What a guardian keeps: whether it moves (synced), whether it was in water at its
/// last tick (`isInWater`, which its sounds ask between ticks), whether a hurt has
/// triggered its stroll (`RandomStrollGoal.trigger`), which the stroll takes at its next
/// asking, and the beam's target (`DATA_ID_ATTACK_TARGET`, synced, none for 0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GuardianState {
    pub(crate) moving: bool,
    pub(crate) in_water: bool,
    pub(crate) stroll_triggered: bool,
    pub(crate) beam: Option<EntityId>,
}

/// [`GuardianState`] as version 1 wrote it, before the beam.
#[derive(Deserialize)]
struct Version1 {
    moving: bool,
    in_water: bool,
    stroll_triggered: bool,
}

/// A guardian's state from its type state; a record of nothing, or one this build
/// cannot read, as a fresh guardian's; one of version 1 with no beam.
pub(crate) fn state_of(mob: &Mob) -> GuardianState {
    if mob.state.bytes.is_empty() {
        return GuardianState::default();
    }
    match mob.state.version {
        VERSION => postcard::from_bytes(&mob.state.bytes).unwrap_or_default(),
        1 => postcard::from_bytes::<Version1>(&mob.state.bytes).map_or_else(
            |_| GuardianState::default(),
            |old| GuardianState {
                moving: old.moving,
                in_water: old.in_water,
                stroll_triggered: old.stroll_triggered,
                beam: None,
            },
        ),
        _ => GuardianState::default(),
    }
}

/// Changes a guardian's state by `change`, writing it back only where it changed.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut GuardianState)) {
    let before = state_of(mob);
    let mut own = before.clone();
    change(&mut own);
    if own == before {
        return;
    }
    mob.state = if own == GuardianState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
        }
    };
}

/// `Guardian`'s constructor's own draw: the client's tail's first frame
/// (`clientSideTailAnimation`), drawn and not kept.
pub(crate) fn construct(own: &mut Random) {
    let _tail = own.next_float();
}

/// The guardian's `randomStrollGoal`: `RandomStrollGoal(1.0, interval)` that moves and
/// looks, and that a hurt triggers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GuardianStrollGoal {
    pub(crate) interval: i32,
}

impl GuardianStrollGoal {
    fn stroll(self) -> RandomStrollGoal {
        RandomStrollGoal {
            interval: self.interval,
            ..RandomStrollGoal::new(1.0)
        }
    }
}

impl Goal for GuardianStrollGoal {
    type State = Strolling;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        if state_of(&ai.tick.mob).stroll_triggered {
            update(&mut ai.tick.mob, |own| own.stroll_triggered = false);
            state.forced = true;
        }
        self.stroll().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        self.stroll().can_continue(state, ai)
    }

    fn start(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        self.stroll().start(state, ai);
    }

    fn stop(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        self.stroll().stop(state, ai);
    }
}

/// `MoveTowardsRestrictionGoal(1.0)` that moves and looks.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GuardianReturnGoal;

impl GuardianReturnGoal {
    const RETURN: MoveTowardsRestrictionGoal = MoveTowardsRestrictionGoal { speed: 1.0 };
}

impl Goal for GuardianReturnGoal {
    type State = Returning;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Returning, ai: &mut Ai<'_, '_>) -> bool {
        Self::RETURN.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Returning, ai: &mut Ai<'_, '_>) -> bool {
        Self::RETURN.can_continue(state, ai)
    }

    fn start(&self, state: &mut Returning, ai: &mut Ai<'_, '_>) {
        Self::RETURN.start(state, ai);
    }
}

/// `GuardianAttackGoal`'s state: `attackTime`, which counts from −10 at its start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Beaming {
    pub(crate) attack_time: i32,
}

/// `Guardian$GuardianAttackGoal(guardian)` (n7 Guardian$GuardianAttackGoal): with a
/// living target (the guardian's within 3 blocks no longer), it keeps still and looks at
/// it; losing sight of it, it drops it; ten ticks on it shows its beam at it
/// (`setActiveAttackTarget`, entity event 21), and at `getAttackDuration()` (80, the
/// elder's 60) it strikes with magic (1, 2 more for the elder and 2 more on hard), then
/// bites (`doHurtTarget`) and drops the target. At its end the beam goes, the target is
/// dropped and its stroll triggered.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GuardianAttackGoal {
    pub(crate) elder: bool,
}

impl GuardianAttackGoal {
    /// `getAttackDuration()`.
    pub(crate) fn attack_duration(self) -> i32 {
        if self.elder { 60 } else { 80 }
    }
}

impl Goal for GuardianAttackGoal {
    type State = Beaming;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut Beaming, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some_and(|target| target.alive)
    }

    fn can_continue(&self, state: &mut Beaming, ai: &mut Ai<'_, '_>) -> bool {
        if !self.can_use(state, ai) {
            return false;
        }
        self.elder
            || sensing::target(ai).is_some_and(|target| target.distance_sqr(ai.position()) > 9.0)
    }

    fn start(&self, state: &mut Beaming, ai: &mut Ai<'_, '_>) {
        state.attack_time = -10;
        navigation::stop(ai);
        if let Some(target) = sensing::target(ai) {
            look_at_thing(ai, &target, 90.0, 90.0);
        }
        ai.tick.out.needs_sync = true;
    }

    fn stop(&self, _: &mut Beaming, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| {
            own.beam = None;
            own.stroll_triggered = true;
        });
        sensing::set_target(ai, None);
    }

    fn tick(&self, state: &mut Beaming, ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        navigation::stop(ai);
        look_at_thing(ai, &target, 90.0, 90.0);
        if !sensing::has_line_of_sight(ai, &target) {
            sensing::set_target(ai, None);
            return;
        }
        state.attack_time += 1;
        if state.attack_time == 0 {
            update(&mut ai.tick.mob, |own| own.beam = Some(target.key));
            // No mob is silent.
            ai.tick.out.events.push(Event::Entity(BEAM_EVENT));
        } else if state.attack_time >= self.attack_duration() {
            let mut amount = 1.0_f32;
            if clustine_world::Difficulty::WORLD == clustine_world::Difficulty::Hard {
                amount += 2.0;
            }
            if self.elder {
                amount += 2.0;
            }
            // `indirectMagic(guardian, guardian)`, then `doHurtTarget`: both land on the
            // target after the guardian's tick, in this order.
            let guardian = clustine_world::Causer {
                key: ai.tick.key,
                kind: ai.tick.kind,
                position: ai.position().map(clustine_world::Double),
                player: None,
                living: true,
            };
            let hurt = clustine_world::HurtBy {
                damage_type: "minecraft:indirect_magic".to_owned(),
                cause: Some(guardian.clone()),
                direct: Some(guardian),
                position: None,
                projectile_motion: None,
                amount: clustine_world::Float(amount),
                knockback: None,
                fire_before: 0,
                fire_after: 0,
                effects: Vec::new(),
                arrow: false,
                extras: Vec::new(),
                sound: None,
                hit: None,
            };
            ai.tick.out.local.push(Local::Strike {
                key: target.key,
                hurt: Box::new(hurt),
            });
            let mob_type = ai.tick.mob_type;
            mob_type.do_hurt_target(ai, &target);
            sensing::set_target(ai, None);
        }
    }
}

/// The entity event that starts a client's beam (`handleEntityEvent(21)`).
pub(crate) const BEAM_EVENT: u8 = 21;

/// `Guardian.registerGoals`, the constructor's water malus and `createNavigation`: the
/// stroll's interval 80, the elder's 400.
pub(crate) fn register_goals(setup: &mut Setup, interval: i32, elder: bool) {
    setup.config.navigation = Settings::of(Kind::WaterBound { breach: false });
    setup.set_malus(PathType::Water, 0.0);
    setup.add_goal(4, GuardianAttackGoal { elder });
    setup.add_goal(5, GuardianReturnGoal);
    setup.add_goal(7, GuardianStrollGoal { interval });
    setup.add_goal(8, LookAtPlayerGoal::new(Class::Player, 8.0));
    setup.add_goal(
        8,
        LookAtPlayerGoal {
            probability: 0.01,
            ..LookAtPlayerGoal::new(Class::Kinds(&GUARDIANS), 12.0)
        },
    );
    setup.add_goal(9, RandomLookAroundGoal);
    crate::mobs::ai::targets::register(setup, crate::mobs::ai::targets::GUARDIAN);
}

/// `Mob.setSpeed(speed)`, which sets `zza` too.
fn set_speed(ai: &mut Ai<'_, '_>, speed: f32) {
    let inputs = &mut ai.tick.mob.body.inputs;
    inputs.speed = speed;
    inputs.zza = speed;
}

/// `GuardianMoveControl.tick()`: towards what it wants while its path lasts, turned up
/// to 90 a tick, its speed eased an eighth of the way, its motion swayed by the
/// `tickCount` and its id, its look eased an eighth towards two blocks ahead; else
/// still. Its operation is left as it was; it moves as long as it goes.
pub(crate) fn tick_move_control(ai: &mut Ai<'_, '_>) {
    let movement = ai.tick.mob.body.controls.movement.clone();
    if movement.operation != Operation::MoveTo || navigation::is_done(ai) {
        set_speed(ai, 0.0);
        update(&mut ai.tick.mob, |own| own.moving = false);
        return;
    }
    let [x, y, z] = ai.position();
    let (dx, dy, dz) = (
        movement.wanted[0] - x,
        movement.wanted[1] - y,
        movement.wanted[2] - z,
    );
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    let (nx, ny, nz) = (dx / length, dy / length, dz / length);
    let yaw = yaw_towards(dx, dz);
    ai.tick.yaw = move_rotlerp(ai.tick.yaw, yaw, 90.0);
    ai.tick.mob.body.head.body_yaw = ai.tick.yaw;
    let wanted_speed = (movement.speed * ai.attribute(ids::MOVEMENT_SPEED)) as f32;
    let current = ai.tick.mob.body.inputs.speed;
    // `Mth.lerp(0.125f, speed, wanted)`.
    let speed = current + 0.125 * (wanted_speed - current);
    set_speed(ai, speed);
    let phase = f64::from((ai.tick.tick_count as i32).wrapping_add(ai.id()));
    let sway = hotspot_sin(phase * 0.5) * 0.05;
    let cos = hotspot_cos(f64::from(ai.tick.yaw * 0.017_453_292));
    let sin = hotspot_sin(f64::from(ai.tick.yaw * 0.017_453_292));
    let bob = hotspot_sin(phase * 0.75) * 0.05;
    let motion = &mut ai.tick.living.body.motion;
    motion[0] += sway * cos;
    motion[1] += bob * (sin + cos) * 0.25 + f64::from(speed) * ny * 0.1;
    motion[2] += sway * sin;
    let ahead = [x + nx * 2.0, ai.eye_y() + ny / length, z + nz * 2.0];
    let wanted = if controls::is_looking(ai) {
        ai.tick.mob.body.controls.look.wanted
    } else {
        ahead
    };
    controls::set_look_at(
        ai,
        [
            lerp(0.125, wanted[0], ahead[0]),
            lerp(0.125, wanted[1], ahead[1]),
            lerp(0.125, wanted[2], ahead[2]),
        ],
        10.0,
        40.0,
    );
    update(&mut ai.tick.mob, |own| own.moving = true);
}

/// `Guardian.aiStep` before `Monster`'s, on a living guardian: in water its air full;
/// else on the ground the flop, three draws of its own: a push about and up, a yaw,
/// off the ground; then, while its beam has a target, its body's yaw its head's. Where
/// it is in water is kept for its sounds.
pub(crate) fn before_ai_step(tick: &mut MobTick<'_>) {
    if tick.mob.body.vitals.dying() {
        return;
    }
    let in_water = tick.living.body.in_water;
    update(&mut tick.mob, |own| own.in_water = in_water);
    if in_water {
        tick.living.air = 300;
    } else if tick.living.body.on_ground {
        let x = f64::from((tick.draws.next_float() * 2.0 - 1.0) * 0.4);
        let z = f64::from((tick.draws.next_float() * 2.0 - 1.0) * 0.4);
        let motion = &mut tick.living.body.motion;
        motion[0] += x;
        motion[1] += 0.5;
        motion[2] += z;
        tick.yaw = tick.draws.next_float() * 360.0;
        tick.living.body.on_ground = false;
        tick.out.needs_sync = true;
    }
    // `hasActiveAttackTarget()`: `setYRot(yHeadRot)`.
    if state_of(&tick.mob).beam.is_some() {
        tick.yaw = tick.mob.body.head.head_yaw;
    }
}

/// `travelInWater`: `moveRelative(0.1)`, its move, its motion times 0.9, and a sink of
/// 0.005 unless it moves or has a target.
pub(crate) fn travel(tick: &MobTick<'_>) -> Travel {
    let sinks = if state_of(&tick.mob).moving || tick.mob.body.combat.target.is_some() {
        Sinks::Never
    } else {
        Sinks::Always
    };
    Travel::Swims { speed: 0.1, sinks }
}

/// `Guardian.getWalkTargetValue`: water is worth ten and its light's worth, the rest
/// `Monster`'s, darker better.
pub(crate) fn walk_target_value(ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
    let cost = super::animal::light_cost(ai, pos);
    if fluid_in_tag(ai.tick.world.state(pos), "minecraft:water") {
        10.0 + cost
    } else {
        -cost
    }
}

/// `Guardian.hurtServer` before `Monster`'s: while it keeps still, what hit it with its
/// own living body (not a player, whom no hurt reaches as every player is creative) is
/// hurt by its spikes, `thorns` of 2, unless the damage is one of
/// `#avoids_guardian_thorns` or thorns itself; and its stroll is triggered.
pub(crate) fn before_hurt_server(struck: &mut Struck<'_>, source: &DamageSource) {
    let own = state_of(struck.mob);
    if !own.moving
        && !source.kind.is("minecraft:avoids_guardian_thorns")
        && source.kind != DamageType::THORNS
        && let Some(direct) = source.direct.as_ref()
        && direct.living
        && direct.player.is_none()
    {
        struck.out.local.push(Local::Thorns {
            key: direct.key,
            kind: struck.kind,
            from: struck.position,
            amount: 2.0,
        });
    }
    update(struck.mob, |own| own.stroll_triggered = true);
}

/// `isMoving()` and the beam's target, as clients are sent them: the target by its key,
/// which the edge writes as its id (an `INT`, 0 for none).
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let own = state_of(mob);
    vec![
        (MOVING_INDEX, DataShown::Boolean(own.moving), !own.moving),
        (
            ATTACK_TARGET_INDEX,
            DataShown::Entity(own.beam),
            own.beam.is_none(),
        ),
    ]
}

/// `getMaxHeadXRot()`: 180.
pub(crate) fn head() -> HeadLimits {
    HeadLimits {
        max_x_rot: 180,
        ..HeadLimits::default()
    }
}
