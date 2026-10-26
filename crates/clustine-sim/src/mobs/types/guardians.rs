//! What the guardian and the elder guardian share (`Guardian`, its move control and
//! goals; `n7k-brief.md` 3.8), part K5: a monster of the water that swims where its
//! water-bound navigation takes it, keeps near its home, looks about, flops on land, and
//! whose spikes hurt what hits it while it keeps still.
//!
//! Named differences: its attack, its beam and its target are N9's (the attack's goal
//! holds its place; the beam's synced target stays 0, so its body is never turned to its
//! head for one); the tail's and the spikes' animations, its bubbles and the flop's
//! sound are its clients' own (the flop is heard by them, `playLocalSound`); and the
//! spikes hurt their attacker in the region's order of things, after the guardian's own
//! hurt, where the game hurts the attacker first.

use clustine_blocks::data::fluid_in_tag;
use clustine_data::{PathType, attributes as ids, entity_types};
use clustine_world::{BlockPos, TypeState};
use serde::{Deserialize, Serialize};

use crate::living::{Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    LookAtPlayerGoal, MoveTowardsRestrictionGoal, RandomLookAroundGoal, RandomStrollGoal,
    Returning, Strolling,
};
use crate::mobs::ai::maths::{hotspot_cos, hotspot_sin, lerp, move_rotlerp, yaw_towards};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, HeadLimits, Operation, Setup, controls};
use crate::mobs::body::hurt::{DamageSource, DamageType, Struck};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::types::tamable::Idle;
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a guardian's type state.
const VERSION: u8 = 1;

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
/// last tick (`isInWater`, which its sounds ask between ticks), and whether a hurt has
/// triggered its stroll (`RandomStrollGoal.trigger`), which the stroll takes at its next
/// asking.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GuardianState {
    pub(crate) moving: bool,
    pub(crate) in_water: bool,
    pub(crate) stroll_triggered: bool,
}

/// A guardian's state from its type state; a record of nothing, or one this build
/// cannot read, as a fresh guardian's.
pub(crate) fn state_of(mob: &Mob) -> GuardianState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return GuardianState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
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

/// `Guardian.registerGoals`, the constructor's water malus and `createNavigation`: the
/// stroll's interval 80, the elder's 400.
pub(crate) fn register_goals(setup: &mut Setup, interval: i32) {
    setup.config.navigation = Settings::of(Kind::WaterBound { breach: false });
    setup.set_malus(PathType::Water, 0.0);
    // `GuardianAttackGoal`: a target's (N9).
    setup.add_goal(
        4,
        Idle {
            flags: flags::MOVE | flags::LOOK,
        },
    );
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
    // `NearestAttackableTargetGoal(LivingEntity, 10, true, false, GuardianAttackSelector)`.
    setup.add_target(
        1,
        Idle {
            flags: flags::TARGET,
        },
    );
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
/// off the ground. Where it is in water is kept for its sounds.
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
}

/// `travelInWater`: `moveRelative(0.1)`, its move, its motion times 0.9, and a sink of
/// 0.005 unless it moves or has a target (none before N9).
pub(crate) fn travel(tick: &MobTick<'_>) -> Travel {
    let sinks = if state_of(&tick.mob).moving {
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

/// `isMoving()` and the beam's target, as clients are sent them.
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let moving = state_of(mob).moving;
    vec![
        (MOVING_INDEX, DataShown::Boolean(moving), !moving),
        (ATTACK_TARGET_INDEX, DataShown::Int(0), true),
    ]
}

/// `getMaxHeadXRot()`: 180.
pub(crate) fn head() -> HeadLimits {
    HeadLimits {
        max_x_rot: 180,
        ..HeadLimits::default()
    }
}
