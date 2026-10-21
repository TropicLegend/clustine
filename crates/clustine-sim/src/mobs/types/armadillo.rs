//! Armadillos (`Armadillo`; `docs/groundwork/n7-brief.md`, section 7.8): an animal of
//! spider eyes that rolls up when something living hurts it (and takes half a hit less
//! one rolled up), unrolls when fire, lava or the like hurts it instead, gives a scute
//! to a brush and sheds one every 6000 to 11999 ticks; rolled up it is not fed nor
//! bred, makes no ambient sound and its hurt sound is the reduced one.
//!
//! Its state (`ArmadilloState`: idle, rolling, scared, unrolling) is synced by a
//! serializer of its own; how long it has been in it (`inStateTicks`, which counts every
//! tick and is neither saved nor sent) is the moment it began, and its scute's timer
//! (`scuteTime`, saved) the moment it runs out, so that an armadillo at rest writes
//! nothing.
//!
//! Its mind is a brain (`ArmadilloAi`, [`PROVIDER`]): it wanders, follows spider eyes and
//! adults, breeds and looks about; its scare sensor, every five ticks, remembers danger
//! for 80 ticks where something of its `mobs` is undead, last hurt it, or is a player
//! sprinting or riding within 7, 2 and 7 of it, and a hurt by something living does the
//! same; with danger remembered it panics (`PANIC`, [`BallUp`]): rolls up, is scared ten
//! ticks on with the landing's sound, peeks now and then while danger is near, starts to
//! unroll 30 ticks before the memory runs out and is scared again if danger comes back,
//! and the core's rolling out unrolls it the tick the memory is gone.
//!
//! What reads across a line (the plan, section 3): its scare reads the `mobs` its living
//! sensor took of the copies' entities and the faces' players, a face's sprint among what
//! it shows; an entity of that list the view no longer has is passed over, as one out of
//! sight (the shared scare sensor's reading of "unknown", plan section 8 item 8), so that
//! it takes no danger of what it cannot see and forgets none it remembers; nothing else
//! of its own reads another entity.
//!
//! Named differences: its panic (`ArmadilloPanic`, of `#panic_environmental_causes`) does
//! not start yet, the brain's hurt sensor seeing no hurt of no living cause until the body
//! keeps its last damage's source (the lead's, plan section 8 item 2); a hurt by something
//! living taken in its own tick after its counters (a living attacker's hit comes between
//! its ticks, so none does yet) would remember its danger a tick longer than the game's,
//! its moment then being the next tick's; a peek's count goes on
//! through ticks its AI does not run, as every count of its brain does (the plan's
//! section 2.6); a baby sheds no scute (`shouldDropLoot` is false for a baby, which the
//! brief has otherwise) and its timer waits below nothing until it grows up; the
//! timer counts the ticks its AI does not run (`NoAI`), as a stamp does; the shed and
//! brush tables, which the extraction does not compile yet (the lead's N7j1), are their
//! one scute and draw nothing; a brush wears nothing, every player being creative.

use clustine_data::{items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Used};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::ai::{Ai, HeadLimits, Setup};
use crate::mobs::body::hurt::{DamageSource, GameEvent, Struck, voice_pitch};
use crate::mobs::body::{Event, loot};
use crate::mobs::brain::behaviours::looking::{self, LookAt};
use crate::mobs::brain::behaviours::panic::{self, AnimalPanic};
use crate::mobs::brain::behaviours::social::{self, AnimalMakeLove, FollowTemptation};
use crate::mobs::brain::behaviours::walking::{self, MoveToTargetSink};
use crate::mobs::brain::behaviours::{Counter, Speed, cooldowns};
use crate::mobs::brain::sensors::{self, MobSensor, TemptingSensor};
use crate::mobs::brain::{
    self as machinery, Activity, ActivityData, Cx, DoNothing, Gate, HeldShot, HeldTimed,
    MemoryKind, Order, Provider, Running, Shot, Status, Timed, Value,
};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::{DataShown, Serialiser};
use clustine_world::{BlockPos, ItemStack};

/// `#armadillo_food`.
pub(crate) const FOOD: Items = Items::Tag("minecraft:armadillo_food");

/// The synced state's index.
const STATE_INDEX: u8 = 18;

/// The shed's and the brush's tables.
const SHED_TABLE: &str = "minecraft:gameplay/armadillo_shed";
const BRUSH_TABLE: &str = "minecraft:brush/armadillo";

/// The entity event of a peek, whose sound and look are the client's.
pub(crate) const PEEK: u8 = 64;

/// `ArmadilloState`, by its id (`ByIdMap.continuous`) and name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Phase {
    #[default]
    Idle,
    Rolling,
    Scared,
    Unrolling,
}

impl Phase {
    /// Its id, which clients are sent.
    pub(crate) fn id(self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::Rolling => 1,
            Self::Scared => 2,
            Self::Unrolling => 3,
        }
    }

    /// `isThreatened()`: all but idle.
    pub(crate) fn threatened(self) -> bool {
        self != Self::Idle
    }

    /// `animationDuration()`.
    pub(crate) fn animation_duration(self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::Rolling => 10,
            Self::Scared => 50,
            Self::Unrolling => 30,
        }
    }
}

/// What an armadillo keeps beyond an animal: its state and the moment it began
/// (`inStateTicks` is the moments since), and the moment its scute's timer comes to
/// nothing (`scuteTime` is that less now).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArmadilloState {
    pub(crate) phase: Phase,
    pub(crate) phase_since: u32,
    pub(crate) scute_at: u32,
}

impl Own for ArmadilloState {}

impl ArmadilloState {
    /// `inStateTicks` as read at the moment `now`.
    pub(crate) fn in_state_ticks(&self, now: u32) -> i64 {
        (i64::from(now) - i64::from(self.phase_since)).max(0)
    }

    /// `scuteTime` at the moment `now`, below nothing where it waits.
    pub(crate) fn scute_time(&self, now: u32) -> i64 {
        i64::from(self.scute_at) - i64::from(now)
    }

    /// `switchToState(state)` at the moment `now`: its count begins again only where the
    /// state changes (`onSyncedDataUpdated` is called of a new value alone).
    pub(crate) fn switch_to(&mut self, phase: Phase, now: u32) {
        if phase != self.phase {
            self.phase = phase;
            self.phase_since = now;
        }
    }
}

/// The armadillo's own state of `mob`.
pub(crate) fn state_of(mob: &Mob) -> ArmadilloState {
    animal::read::<ArmadilloState>(&mob.state).1
}

/// Changes the armadillo's own state of `mob`.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut ArmadilloState)) {
    let (shared, mut own) = animal::read::<ArmadilloState>(&mob.state);
    let before = own;
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `isScared()`: any state but idle.
pub(crate) fn is_scared(mob: &Mob) -> bool {
    state_of(mob).phase.threatened()
}

/// `pickNextScuteDropTime()`: 6000 to 11999 ticks, of its own draw.
fn next_scute_time(draws: &mut Random) -> u32 {
    draws.next_int(6000) + 6000
}

/// The stacks of the table `name`, which gives one scute and draws nothing where the
/// extraction does not compile it yet.
fn scute_drops(name: &str, kind: u16, draws: &mut Random) -> Vec<ItemStack> {
    if clustine_data::loot::table_by_name(name).is_none() {
        return vec![ItemStack {
            item: items::ARMADILLO_SCUTE,
            count: 1,
        }];
    }
    loot::named_drops(name, kind, false, loot::Facts::default(), draws)
}

/// `canStayRolledUp()`: not panicking, out of liquids, on no lead, riding nothing and
/// carrying no one.
pub(crate) fn can_stay_rolled_up(
    panicking: bool,
    in_liquid: bool,
    leashed: bool,
    passenger: bool,
    vehicle: bool,
) -> bool {
    !panicking && !in_liquid && !leashed && !passenger && !vehicle
}

/// What a rolling up or out shows: its game event and its sound (`makeSound`, the
/// voice's two draws), in the order made.
fn act(mob: &Mob, sound: i32, draws: &mut Random, events: &mut Vec<Event>) {
    events.push(Event::GameEvent(GameEvent::EntityAction));
    let pitch = voice_pitch(draws, animal::is_baby(mob));
    events.push(Event::Sound {
        sound,
        volume: 1.0,
        pitch,
    });
}

/// `rollUp()` of an armadillo not scared, at the moment `now`: it stops where it is
/// (`stopInPlace`: its path, its inputs, its motion), its love ends, it acts with the
/// roll's sound, and it is rolling.
fn roll_up(
    mob: &mut Mob,
    motion: &mut Vec3,
    now: u32,
    draws: &mut Random,
    events: &mut Vec<Event>,
) {
    if is_scared(mob) {
        return;
    }
    mob.body.path.followed = None;
    let inputs = &mut mob.body.inputs;
    inputs.xxa = 0.0;
    inputs.yya = 0.0;
    inputs.zza = 0.0;
    inputs.speed = 0.0;
    *motion = [0.0; 3];
    animal::update(mob, animal::Animal::reset_love);
    act(mob, sounds::ENTITY_ARMADILLO_ROLL, draws, events);
    update(mob, |own| own.switch_to(Phase::Rolling, now));
}

/// `rollOut()` of a scared armadillo, at the moment `now`: it acts with the unroll's
/// sound and is idle.
fn roll_out(mob: &mut Mob, now: u32, draws: &mut Random, events: &mut Vec<Event>) {
    if !is_scared(mob) {
        return;
    }
    act(mob, sounds::ENTITY_ARMADILLO_UNROLL_FINISH, draws, events);
    update(mob, |own| own.switch_to(Phase::Idle, now));
}

/// The armadillo.
pub(crate) struct Armadillo;

/// What the brain calls of an armadillo: its own behaviours' bodies (`ArmadilloBallUp`,
/// `ArmadilloPanic`'s start, the core's rolling out, the walk sink that waits while it is
/// scared), the scare sensor's test, and the memory it sets when hurt.
pub(crate) mod brain {
    use super::*;

    /// `DANGER_DETECTED_RECENTLY`'s expiry where it is set, and the times to it the
    /// rolled-up armadillo compares (75 for danger about, `UNROLLING`'s 30).
    pub(crate) const DANGER_TICKS: i64 = 80;
    pub(crate) const DANGER_ABOUT: i64 = 75;

    /// `ArmadilloBallUp`'s length, `5 × 60 × 20`.
    pub(crate) const BALL_UP_TICKS: i64 = 6000;

    /// `FollowTemptation`'s speed, and its distance: a baby's 1, else 2.
    pub(crate) const TEMPTED_SPEED: f32 = 1.25;
    pub(crate) fn tempted_distance(mob: &Mob) -> f64 {
        if animal::is_baby(mob) { 1.0 } else { 2.0 }
    }

    /// A hurt by something living at the moment `moment` (`actuallyHurt`, outside the
    /// brain's tick): `DANGER_DETECTED_RECENTLY` set with an expiry of 80.
    pub(crate) fn remember_danger(mob: &mut Mob, moment: u32) {
        let mut memories = machinery::memories_of(&PROVIDER, mob, moment);
        memories.set_with_expiry(
            MemoryKind::DangerDetectedRecently,
            Value::Bool(true),
            DANGER_TICKS,
        );
        machinery::write_memories(&PROVIDER, mob, memories);
    }

    /// `isPanicking()` outside the brain's tick, at the moment `moment`: the brain's
    /// `IS_PANICKING`.
    pub(crate) fn panicking(mob: &Mob, moment: u32) -> bool {
        machinery::memories_of(&PROVIDER, mob, moment)
            .check(MemoryKind::IsPanicking, Status::Present)
    }

    /// `ArmadilloBallUp`'s fields: its next peek's countdown and whether danger was about
    /// at its last tick.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
    pub(crate) struct BallUp {
        pub(crate) next_peek_timer: i32,
        pub(crate) danger_was_around: bool,
    }

    /// `pickNextPeekTimer`: `SCARED`'s 50 and 100 to 400 more, of its own draw.
    pub(crate) fn next_peek_timer(draws: &mut Random) -> i32 {
        Phase::Scared.animation_duration() + draws.next_int_between(100, 400)
    }

    /// `ArmadilloBallUp.checkExtraStartConditions`: on the ground, out of liquids.
    pub(crate) fn ball_up_may_start(tick: &MobTick<'_>) -> bool {
        tick.living.body.on_ground && !(tick.living.body.in_water || tick.living.in_lava())
    }

    /// `ArmadilloBallUp.start`: it rolls up.
    pub(crate) fn ball_up_start(tick: &mut MobTick<'_>) {
        let now = tick.lived;
        roll_up(
            &mut tick.mob,
            &mut tick.living.body.motion,
            now,
            tick.draws,
            &mut tick.out.events,
        );
    }

    /// `ArmadilloBallUp.canStillUse`: while it is threatened.
    pub(crate) fn ball_up_may_go_on(mob: &Mob) -> bool {
        state_of(mob).phase.threatened()
    }

    /// `ArmadilloBallUp.stop`: one that may not stay rolled up rolls out; `panicking`
    /// the brain's `IS_PANICKING`.
    pub(crate) fn ball_up_stop(tick: &mut MobTick<'_>, panicking: bool) {
        let stays = can_stay_rolled_up(
            panicking,
            tick.living.body.in_water || tick.living.in_lava(),
            tick.leashed,
            tick.vehicle.is_some(),
            tick.carries,
        );
        if !stays {
            let now = tick.lived;
            roll_out(&mut tick.mob, now, tick.draws, &mut tick.out.events);
        }
    }

    /// `ArmadilloBallUp.tick`, `ttl` the time to the expiry of its danger memory
    /// (`i64::MAX` where it has none): the peek's countdown; a roll of more than ten
    /// ticks becomes scared, with the landing's sound on the ground, and nothing more
    /// that tick; else a change of danger about draws the next peek, a scared one peeks
    /// when its countdown is out and danger is about and unrolls when the danger is 30
    /// from its end, an unrolling one is scared again when danger comes back.
    pub(crate) fn ball_up_tick(tick: &mut MobTick<'_>, ball: &mut BallUp, ttl: i64) {
        let now = tick.lived;
        if ball.next_peek_timer > 0 {
            ball.next_peek_timer -= 1;
        }
        let own = state_of(&tick.mob);
        // `shouldSwitchToScaredState()`: rolling for more than ten ticks.
        if own.phase == Phase::Rolling && own.in_state_ticks(now) > 10 {
            update(&mut tick.mob, |own| own.switch_to(Phase::Scared, now));
            if tick.living.body.on_ground {
                sound(tick, sounds::ENTITY_ARMADILLO_LAND);
            }
            return;
        }
        let danger = ttl > DANGER_ABOUT;
        if danger != ball.danger_was_around {
            ball.next_peek_timer = next_peek_timer(tick.draws);
        }
        ball.danger_was_around = danger;
        let unrolling = i64::from(Phase::Unrolling.animation_duration());
        match own.phase {
            Phase::Scared => {
                if ball.next_peek_timer == 0 && tick.living.body.on_ground && danger {
                    tick.out.events.push(Event::Entity(PEEK));
                    ball.next_peek_timer = next_peek_timer(tick.draws);
                }
                if ttl < unrolling {
                    sound(tick, sounds::ENTITY_ARMADILLO_UNROLL_START);
                    update(&mut tick.mob, |own| own.switch_to(Phase::Unrolling, now));
                }
            }
            Phase::Unrolling if ttl > unrolling => {
                update(&mut tick.mob, |own| own.switch_to(Phase::Scared, now));
            }
            _ => {}
        }
    }

    /// `Entity.playSound(sound)`: at full volume and pitch, no draw.
    fn sound(tick: &mut MobTick<'_>, sound: i32) {
        tick.out.events.push(Event::Sound {
            sound,
            volume: 1.0,
            pitch: 1.0,
        });
    }

    /// `ARMADILLO_ROLLING_OUT`, of a brain without its danger memory: a scared armadillo
    /// rolls out. Whether it did.
    pub(crate) fn rolling_out(tick: &mut MobTick<'_>) -> bool {
        if !is_scared(&tick.mob) {
            return false;
        }
        let now = tick.lived;
        roll_out(&mut tick.mob, now, tick.draws, &mut tick.out.events);
        true
    }

    /// `ArmadilloPanic.start` before `AnimalPanic`'s: it rolls out.
    pub(crate) fn panic_start(tick: &mut MobTick<'_>) {
        let now = tick.lived;
        roll_out(&mut tick.mob, now, tick.draws, &mut tick.out.events);
    }

    /// `ArmadilloAi$1`, the walk sink, starts only where the armadillo is not scared.
    pub(crate) fn walk_sink_may_start(mob: &Mob) -> bool {
        !is_scared(mob)
    }

    /// What `isScaredBy` reads of a living entity near.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(crate) struct Near {
        pub(crate) bounds: crate::physics::aabb::Aabb,
        /// Its entity type, by id.
        pub(crate) kind: u16,
        /// Whether it is the armadillo's `getLastHurtByMob()`.
        pub(crate) hurt_it_last: bool,
        /// A player's: spectating, sprinting, riding.
        pub(crate) player: Option<(bool, bool, bool)>,
    }

    /// `isScaredBy(entity)`, the scare sensor's test of the armadillo whose box is
    /// `own`: within 7, 2 and 7 of it, the undead, what last hurt it, and a player who
    /// sprints or rides (never one who spectates).
    pub(crate) fn is_scared_by(own: crate::physics::aabb::Aabb, near: &Near) -> bool {
        if !own.inflate([7.0, 2.0, 7.0]).intersects(near.bounds) {
            return false;
        }
        if clustine_blocks::data::entity_in_tag(i32::from(near.kind), "minecraft:undead") {
            return true;
        }
        if near.hurt_it_last {
            return true;
        }
        match near.player {
            Some((spectator, sprinting, riding)) => !spectator && (sprinting || riding),
            None => false,
        }
    }
}

/// What a fresh load of an armadillo resets (`inStateTicks` is not saved; the plan's
/// 2.10), at the moment `now`: for the brain's `fresh_load` hook when it comes.
#[allow(
    dead_code,
    reason = "the fresh load of N7j1's contracts, not built yet"
)]
pub(crate) fn fresh_load(mob: &mut Mob, now: u32) {
    update(mob, |own| own.phase_since = now);
}

/// `FOOD_TEMPTATIONS` of an armadillo: spider eyes.
static TEMPTATIONS: TemptingSensor = TemptingSensor {
    tempts: |_, stack| FOOD.test(Some(stack)),
};

/// `ARMADILLO_SCARE_DETECTED` (`SensorType`'s `MobSensor(5, isScaredBy, canStayRolledUp,
/// DANGER_DETECTED_RECENTLY, 80)`, its two method references **Inferred**, the brief's
/// 7.8.3).
static SCARE: MobSensor = MobSensor {
    rate: 5,
    mob_test: scared_by,
    ready: may_stay_rolled_up,
    to_set: MemoryKind::DangerDetectedRecently,
    ttl: brain::DANGER_TICKS,
};

/// `Armadillo.BRAIN_PROVIDER`: its sensors in the game's order, and `ArmadilloAi`'s
/// activities.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::HURT_BY,
        &TEMPTATIONS,
        &sensors::NEAREST_ADULT,
        &SCARE,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `ArmadilloAi.updateActivity`: panic where danger is remembered and it does not panic
/// already, else idle.
pub(crate) const ACTIVITIES: [Activity; 2] = [Activity::Panic, Activity::Idle];

/// The armadillo's entity type, as the breeding's partner type.
const ARMADILLO_KIND: u16 = clustine_data::entity_types::ARMADILLO as u16;

/// `ArmadilloAi.getActivities`.
fn activities() -> Vec<ActivityData> {
    use MemoryKind::{
        DangerDetectedRecently, GazeCooldownTicks, IsPanicking, TemptationCooldownTicks, WalkTarget,
    };
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            panic::swim(0.8),
            AnimalPanic {
                causes: environmental_causes,
                on_start: roll_out_for_the_panic,
                ..AnimalPanic::new(2.0)
            }
            .boxed(),
            walking::look_sink(45, 90),
            Box::new(MoveToTargetSink::refusing(150, 250, not_scared)),
            cooldowns::count_down(TemptationCooldownTicks),
            cooldowns::count_down(GazeCooldownTicks),
            Box::new(HeldShot::new(RollingOut)),
        ],
    );
    let tempted = FollowTemptation {
        close_enough: tempted_distance,
        ..FollowTemptation::new(Speed::Fixed(brain::TEMPTED_SPEED))
    };
    let idle = ActivityData {
        activity: Activity::Idle,
        behaviours: vec![
            (0, looking::look_sometimes(LookAt::Player, 6.0, (30, 60))),
            (1, AnimalMakeLove::at(ARMADILLO_KIND, 1.0, 1).boxed()),
            (
                2,
                Box::new(Gate::run_one(vec![
                    (tempted.boxed(), 1),
                    (social::follow_adult((5, 16), 1.25), 1),
                ])),
            ),
            (3, looking::look_around((150, 250), 30.0, 0.0, 0.0)),
            (
                4,
                Box::new(Gate::new(
                    vec![(WalkTarget, Status::Absent)],
                    Vec::new(),
                    Order::Shuffled,
                    Running::RunOne,
                    vec![
                        (walking::stroll(1.0), 1),
                        (walking::walk_to_look_target(1.0, 3), 1),
                        (Box::new(HeldTimed::new(DoNothing { min: 30, max: 60 })), 1),
                    ],
                )),
            ),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    };
    let scared = ActivityData {
        activity: Activity::Panic,
        behaviours: vec![(0, Box::new(HeldTimed::new(BallUp)))],
        conditions: vec![
            (DangerDetectedRecently, Status::Present),
            (IsPanicking, Status::Absent),
        ],
        erase: Vec::new(),
    };
    vec![core, idle, scared]
}

/// `ArmadilloPanic`'s causes: `#panic_environmental_causes`.
fn environmental_causes(_: &Cx<'_, '_, '_>) -> &'static str {
    "minecraft:panic_environmental_causes"
}

/// `ArmadilloPanic.start` before the panic's own: it rolls out.
fn roll_out_for_the_panic(cx: &mut Cx<'_, '_, '_>) {
    brain::panic_start(cx.ai.tick);
}

/// `ArmadilloAi$1.checkExtraStartConditions` before the sink's own: not scared.
fn not_scared(cx: &mut Cx<'_, '_, '_>) -> bool {
    brain::walk_sink_may_start(&cx.ai.tick.mob)
}

/// `FollowTemptation`'s distance: 1 for a pup, 2 grown.
fn tempted_distance(cx: &Cx<'_, '_, '_>) -> f64 {
    brain::tempted_distance(&cx.ai.tick.mob)
}

/// `canStayRolledUp()` in the brain's tick: its own memory of panicking.
fn may_stay_rolled_up(cx: &Cx<'_, '_, '_>) -> bool {
    let tick = &cx.ai.tick;
    can_stay_rolled_up(
        cx.memories.check(MemoryKind::IsPanicking, Status::Present),
        cx.ai.in_liquid(),
        tick.leashed,
        tick.vehicle.is_some(),
        tick.carries,
    )
}

/// `isScaredBy(entity)` of a thing of the armadillo's `mobs`: a player's sprint and
/// riding as their face shows them, what last hurt it as its body remembers.
fn scared_by(cx: &Cx<'_, '_, '_>, looked: &Looked) -> bool {
    let tick = &cx.ai.tick;
    let player = looked.is_player().then(|| {
        let sprinting = tick
            .world
            .players()
            .iter()
            .find(|player| player.key == looked.key)
            .is_some_and(|player| player.sprinting);
        (looked.spectator, sprinting, looked.vehicle.is_some())
    });
    let near = brain::Near {
        bounds: looked.bounds,
        kind: looked
            .kind
            .unwrap_or(clustine_data::entity_types::PLAYER as u16),
        hurt_it_last: tick
            .mob
            .body
            .combat
            .last_hurt_by
            .is_some_and(|(key, _)| key == looked.key),
        player,
    };
    brain::is_scared_by(tick.living.body.bounds(), &near)
}

/// `ARMADILLO_ROLLING_OUT`: where the danger memory is gone and the armadillo is scared,
/// it rolls out.
struct RollingOut;

impl Shot for RollingOut {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::DangerDetectedRecently]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories
            .check(MemoryKind::DangerDetectedRecently, Status::Absent)
            && brain::rolling_out(cx.ai.tick)
    }
}

/// `ArmadilloAi$ArmadilloBallUp`: a `Behavior` of no memories and 6000 ticks (its start
/// draws `nextInt(1)`), started on the ground out of liquids, running while the
/// armadillo is threatened, its tick [`brain::ball_up_tick`].
pub(crate) struct BallUp;

/// What [`BallUp`] keeps: `nextPeekTimer`, which its ticks lower while it is above
/// nothing and which waits while it does not run (kept as a [`Counter`], written only
/// where it is drawn again or its lowering pauses), and `dangerWasAround`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BallUpState {
    pub(crate) peek: Counter,
    pub(crate) danger_was_around: bool,
}

impl Timed for BallUp {
    type State = BallUpState;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[]
    }

    fn durations(&self) -> (i64, i64) {
        (brain::BALL_UP_TICKS, brain::BALL_UP_TICKS)
    }

    fn extra_start(&self, _: &mut BallUpState, cx: &mut Cx<'_, '_, '_>) -> bool {
        brain::ball_up_may_start(cx.ai.tick)
    }

    fn can_still_use(&self, _: &mut BallUpState, cx: &mut Cx<'_, '_, '_>) -> bool {
        brain::ball_up_may_go_on(&cx.ai.tick.mob)
    }

    fn start(&self, _: &mut BallUpState, cx: &mut Cx<'_, '_, '_>) {
        brain::ball_up_start(cx.ai.tick);
    }

    fn tick(&self, state: &mut BallUpState, cx: &mut Cx<'_, '_, '_>) {
        let now = cx.memories.now;
        let before = state.peek.at(now);
        let mut ball = brain::BallUp {
            next_peek_timer: before,
            danger_was_around: state.danger_was_around,
        };
        let ttl = cx.memories.expires_in(MemoryKind::DangerDetectedRecently);
        brain::ball_up_tick(cx.ai.tick, &mut ball, ttl);
        let lowered = (before - 1).max(0);
        if ball.next_peek_timer == lowered {
            if before > 0 {
                state.peek.lower(now);
            }
        } else {
            // Drawn again in this tick: its count goes on from the next.
            state.peek.set(ball.next_peek_timer, now, true);
        }
        state.danger_was_around = ball.danger_was_around;
    }

    fn stop(&self, state: &mut BallUpState, cx: &mut Cx<'_, '_, '_>) {
        // Not lowered in the tick it stops, nor until it runs again.
        state.peek.pause(cx.memories.now);
        let panicking = cx.memories.check(MemoryKind::IsPanicking, Status::Present);
        brain::ball_up_stop(cx.ai.tick, panicking);
    }
}

impl MobType for Armadillo {
    /// The constructor's: `Animal`'s maluses and floating. Its goals are its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        super::cow::animal_maluses(setup);
        setup.config.navigation.flags.can_float = true;
    }

    /// `getMaxHeadYRot()`: 32, and none while scared, which its tick's clamp holds.
    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 32,
            ..HeadLimits::default()
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    /// The constructor's draw after the brain's: the scute's first timer.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let time = next_scute_time(own);
        update(mob, |state| state.scute_at = time);
    }

    /// The body control turns nothing while it is scared.
    fn body_rotation_ticks(&self, tick: &MobTick<'_>) -> bool {
        !is_scared(&tick.mob)
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `brain.tick` and `ArmadilloAi.updateActivity`; then, alive, its scute's timer: run
    /// out, a grown one sheds a scute, with its sound (two draws) and the place's game
    /// event, and draws its next; then `Animal`'s.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        machinery::run(tick, |brain, _| {
            brain.set_active_to_first_valid(&ACTIVITIES)
        });
        if !tick.mob.body.vitals.dying() {
            let next = tick.lived.saturating_add(1);
            let own = state_of(&tick.mob);
            if own.scute_time(next) <= 0 && !animal::is_baby(&tick.mob) {
                let at = tick.living.body.position;
                let mut shed = false;
                for stack in scute_drops(SHED_TABLE, tick.kind, tick.draws) {
                    if stack.count > 0 {
                        shed = true;
                        tick.out.local.push(Local::Drop {
                            stack,
                            at,
                            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                        });
                    }
                }
                if shed {
                    let pitch = (tick.draws.next_float() - tick.draws.next_float()) * 0.2 + 1.0;
                    tick.out.events.push(Event::Sound {
                        sound: sounds::ENTITY_ARMADILLO_SCUTE_DROP,
                        volume: 1.0,
                        pitch,
                    });
                    tick.out
                        .events
                        .push(Event::GameEvent(GameEvent::EntityPlace));
                }
                let time = next_scute_time(tick.draws);
                update(&mut tick.mob, |own| {
                    own.scute_at = next.saturating_add(time)
                });
            }
        }
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Armadillo.tick` after the animal's: a scared armadillo's head is held to its
    /// body. Its `inStateTicks` counts by itself.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        if is_scared(&tick.mob) {
            super::camel::clamp_head_to_body(&mut tick.mob, 0.0);
        }
    }

    /// `Armadillo.mobInteract`: a brush on a grown one brushes a scute off (its table,
    /// its sound, the interaction's game event; the brush worn by 16, which a creative
    /// player's is not); a scared one is not fed (`FAIL`, which passes on as nothing
    /// done); else an animal's use, with its eating sound.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if animal::is_item(used.held, items::BRUSH) && !animal::is_baby(used.mob) {
            for stack in scute_drops(BRUSH_TABLE, used.kind, used.draws) {
                if stack.count > 0 {
                    used.did.push(Did::Drop {
                        stack,
                        at: used.position,
                        pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                        push: None,
                    });
                }
            }
            used.did.push(Did::Sound {
                sound: sounds::ENTITY_ARMADILLO_BRUSH,
                source: SOUND_SOURCE_NEUTRAL,
                at: used.position,
                volume: 1.0,
                pitch: 1.0,
                but_the_player: false,
            });
            used.did
                .push(Did::GameEvent(GameEvent::EntityInteract.name()));
            return Interaction::Success;
        }
        if is_scared(used.mob) {
            return Interaction::Pass;
        }
        animal::mob_interact(used, FOOD, Some(sounds::ENTITY_ARMADILLO_EAT))
    }

    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    /// `Armadillo.hurtServer`: a scared one takes `(amount − 1) / 2`.
    fn hurt_amount(&self, mob: &Mob, amount: f32) -> f32 {
        if is_scared(mob) {
            (amount - 1.0) / 2.0
        } else {
            amount
        }
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    /// `Armadillo.actuallyHurt` after `Animal`'s: of a living mob with AI, something
    /// living that hurt it is a danger it remembers, and it rolls up where it may stay
    /// so; fire, lava and the like of `#panic_environmental_causes` roll it out.
    fn after_actually_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        if struck.mob.body.flags.no_ai || struck.dying() {
            return;
        }
        let now = struck.moment;
        if source.cause.as_ref().is_some_and(|cause| cause.living) {
            brain::remember_danger(struck.mob, now);
            let stays = can_stay_rolled_up(
                brain::panicking(struck.mob, now),
                struck.in_liquid,
                struck.leashed,
                struck.vehicle.is_some(),
                struck.carries,
            );
            if stays {
                roll_up(
                    struck.mob,
                    struck.motion,
                    now,
                    struck.draws,
                    &mut struck.out.events,
                );
            }
        } else if source.kind.is("minecraft:panic_environmental_causes") {
            roll_out(struck.mob, now, struck.draws, &mut struck.out.events);
        }
    }

    /// A baby's size: the type's scaled by 0.6, its eye at 0.21875.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        if !animal::is_baby(mob) {
            return None;
        }
        let info = clustine_data::EntityTypeInfo::of(clustine_data::entity_types::ARMADILLO);
        Some(Dimensions {
            width: info.width * 0.6,
            height: info.height * 0.6,
            eye_height: 0.218_75,
        })
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        (!is_scared(mob)).then_some(sounds::ENTITY_ARMADILLO_AMBIENT)
    }

    /// `playStepSound`: its own at 0.15 and 1.
    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ARMADILLO_STEP)
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(if is_scared(mob) {
            sounds::ENTITY_ARMADILLO_HURT_REDUCED
        } else {
            sounds::ENTITY_ARMADILLO_HURT
        })
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ARMADILLO_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let phase = state_of(mob).phase;
        let mut values = animal::shown(mob);
        values.push((
            STATE_INDEX,
            DataShown::Serialised(Serialiser::ArmadilloState, phase.id()),
            phase == Phase::Idle,
        ));
        values
    }
}
