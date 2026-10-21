//! The sniffer (`Sniffer`, `Sniffer$State`; `docs/groundwork/n7-brief.md`, section 7.9):
//! a big, slow animal whose whole life is a state machine in its synced data (idle,
//! happy, scenting, sniffing, searching, digging, rising), which its brain's seven
//! behaviours drive. Digging, it drops a torchflower seed or a pitcher pod 120 ticks in;
//! bred with torchflower seeds it lays a sniffer egg (an item, never a baby), whose block
//! (`region/sniffer_eggs.rs`) hatches a snifflet that grows for 48000 ticks.
//!
//! Here are the body, the state and its transitions with their sounds and the entity
//! event, the seed's tick, the dig's game event, the box made lower while it digs,
//! feeding, mating's rule, the egg its breeding lays, the quad leash, and its brain
//! (`SnifferAi`, B 7.9.3 and 7.9.4): the provider of the game's memories, sensors and four
//! activities (core, idle, sniff, dig), and the six behaviours of its own, which
//! [`brain_seams`] serve.
//!
//! **Across a line** (`docs/groundwork/n7j-plan.md`, section 3): its sensors read the
//! region's land, copies and faces (items 1); a place to dig and the path to it are
//! judged on the copies' blocks, and a block beyond them reads as unloaded, so not
//! diggable and the end of a path (item 8: a sniffer near a line finds fewer places);
//! its explored positions are absolute and travel with it; a partner only a copy shows
//! breeds by the region's protocol (item 4), the partner's region laying the egg.
//!
//! Named differences: the seed's draw comes from the sniffer's own stream, not from the
//! world's random sequence of `gameplay/sniffer_digging` (which is not extracted yet);
//! the water's malus is not lifted while it is in water or burning
//! (`onPathfindingStart`), which only its brain's paths read; `canDig` asks nothing of
//! the world's border, which every place is within; the digging animation's clock, which
//! the game starts again only at a state's change after a load, is not kept, as the
//! particles it times are the client's.

use std::collections::{BTreeMap, BTreeSet};

use clustine_blocks::data::in_tag;
use clustine_data::{PathType, entity_types, items, sounds};
use clustine_world::{BlockPos, Entry, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, Laid, Mate, MobType, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::maths::{self, table_cos, table_sin};
use crate::mobs::ai::{Ai, HeadLimits, Setup, navigation, random_pos};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::brain::behaviours::cooldowns::count_down;
use crate::mobs::brain::behaviours::looking::{LookAt, look_at};
use crate::mobs::brain::behaviours::panic::{self, AnimalPanic};
use crate::mobs::brain::behaviours::social::{AnimalMakeLove, FollowTemptation};
use crate::mobs::brain::behaviours::walking::{
    MoveToTargetSink, look_sink, stroll, walk_to_look_target,
};
use crate::mobs::brain::behaviours::{Speed, block_of, entry, read_entry, walk, walk_target};
use crate::mobs::brain::sensors::{self, TemptingSensor};
use crate::mobs::brain::{
    self, Activity, ActivityData, Behaviour, Brain, Cx, DoNothing, Gate, HeldTimed, Memories,
    MemoryKind, Provider, Status, Tracker, Value,
};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::{self, Vec3};
use crate::region::{DEFAULT_PICK_UP_DELAY, Random};
use crate::shown::{DataShown, Serialiser};

/// `#sniffer_food`: torchflower seeds.
pub(crate) const FOOD: Items = Items::Tag("minecraft:sniffer_food");

/// The synced data's indices: the state (`SNIFFER_STATE`) and the seed's tick.
pub(crate) const STATE_INDEX: u8 = 18;
pub(crate) const DROP_SEED_INDEX: u8 = 19;

/// `SNIFFER_BABY_START_AGE`: a snifflet grows for twice an animal's time.
pub(crate) const BABY_START_AGE: i32 = -48_000;

/// `DIGGING_DROP_SEED_OFFSET_TICKS`.
pub(crate) const DROP_SEED_AFTER: i32 = 120;

/// `DIGGING_BB_HEIGHT_OFFSET` and the digging box's eye height.
const DIGGING_LOWER_BY: f32 = 0.4;
const DIGGING_EYE_HEIGHT: f32 = 0.81;

/// How far ahead of its feet its nose is (`getHeadPosition`).
const HEAD_REACH: f64 = 2.25;

/// How many explored positions it keeps beyond the newest (`storeExploredPosition`).
const EXPLORED_KEPT: usize = 20;

/// `Sniffer$State`, by its ordinal, which is its id on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum State {
    #[default]
    Idling,
    FeelingHappy,
    Scenting,
    Sniffing,
    Searching,
    Digging,
    Rising,
}

impl State {
    /// Its id (`Sniffer$State.id`, its ordinal).
    pub(crate) fn id(self) -> i32 {
        self as i32
    }
}

/// What a sniffer keeps beyond an animal: its state (`DATA_STATE`) and the tick its seed
/// drops at (`DATA_DROP_SEED_AT_TICK`, in the game's `tickCount`). Neither is saved by the
/// game, which loads a sniffer idle with no seed tick.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SnifferState {
    pub(crate) state: State,
    pub(crate) drop_seed_at: i32,
}

impl Own for SnifferState {}

/// A sniffer's own state.
pub(crate) fn read(mob: &Mob) -> SnifferState {
    animal::read::<SnifferState>(&mob.state).1
}

/// Changes a sniffer's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut SnifferState)) {
    let (shared, mut own) = animal::read::<SnifferState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// The game's `tickCount` within the mob's tick, counted at its start.
fn game_tick_count(tick: &MobTick<'_>) -> i32 {
    tick.tick_count.wrapping_add(1) as i32
}

/// `transitionTo(state)`: the state set, with what each brings: the happy sound, the
/// scenting sound (higher for a baby), the sniffing sound, the seed's tick and entity
/// event 63 as digging starts, the sound of digging's end as it rises.
pub(crate) fn transition_to(tick: &mut MobTick<'_>, to: State) {
    let sound = |sound: i32, pitch: f32| Event::Sound {
        sound,
        volume: 1.0,
        pitch,
    };
    match to {
        State::FeelingHappy => tick
            .out
            .events
            .push(sound(sounds::ENTITY_SNIFFER_HAPPY, 1.0)),
        State::Sniffing => tick
            .out
            .events
            .push(sound(sounds::ENTITY_SNIFFER_SNIFFING, 1.0)),
        State::Rising => tick
            .out
            .events
            .push(sound(sounds::ENTITY_SNIFFER_DIGGING_STOP, 1.0)),
        _ => {}
    }
    update(&mut tick.mob, |own| own.state = to);
    match to {
        State::Scenting => {
            let pitch = if animal::is_baby(&tick.mob) { 1.3 } else { 1.0 };
            tick.out
                .events
                .push(sound(sounds::ENTITY_SNIFFER_SCENTING, pitch));
        }
        State::Digging => {
            let at = game_tick_count(tick).wrapping_add(DROP_SEED_AFTER);
            update(&mut tick.mob, |own| own.drop_seed_at = at);
            tick.out.events.push(Event::Entity(63));
        }
        _ => {}
    }
}

/// `getHeadPosition()`: its feet moved 2.25 along its look (`getForward`, the game's
/// table of sines).
pub(crate) fn head_position(position: Vec3, yaw: f32, pitch: f32) -> Vec3 {
    let f = pitch * maths::RADIANS;
    let g = -yaw * maths::RADIANS;
    let (h, i) = (table_cos(g), table_sin(g));
    let (j, k) = (table_cos(f), table_sin(f));
    let forward = [f64::from(i * j), f64::from(-k), f64::from(h * j)];
    physics::add(position, physics::scale(forward, HEAD_REACH))
}

/// `getHeadBlock()`: the block of its nose, at its feet's height and a fifth.
pub(crate) fn head_block(position: Vec3, yaw: f32, pitch: f32) -> BlockPos {
    let head = head_position(position, yaw, pitch);
    physics::containing([head[0], position[1] + 0.2, head[2]])
}

/// `#sniffer_diggable_block`: dirt, mud, moss, grass and podzol.
pub(crate) fn diggable(state: clustine_data::BlockState) -> bool {
    in_tag(state, "minecraft:sniffer_diggable_block")
}

/// `storeExploredPosition(pos)`: the newest first, the twenty before it kept.
pub(crate) fn store_explored(explored: &[BlockPos], pos: BlockPos) -> Vec<BlockPos> {
    let mut kept = Vec::with_capacity(EXPLORED_KEPT + 1);
    kept.push(pos);
    kept.extend(explored.iter().take(EXPLORED_KEPT).copied());
    kept
}

/// What the brain's behaviours call (`SnifferAi`'s, B 7.9.3 and 7.9.4): the predicates
/// they read and the search for a place to dig. Its memories (`sniffer_explored_positions`,
/// the digging and sniffing marks, `sniff_cooldown`) are the brain's; these take what they
/// read of them.
pub(crate) mod brain_seams {
    use super::*;

    /// What the brain's memories say of the sniffer, as its predicates read them.
    #[derive(Debug, Clone, Copy, Default)]
    pub(crate) struct Marks {
        /// `TEMPTING_PLAYER` present (`isTempted`).
        pub(crate) tempted: bool,
        /// `IS_PANICKING` present (`isPanicking`).
        pub(crate) panicking: bool,
    }

    impl Marks {
        /// The marks as `memories` hold them.
        pub(crate) fn of(memories: &Memories) -> Self {
            Self {
                tempted: memories.check(MemoryKind::TemptingPlayer, Status::Present),
                panicking: memories.check(MemoryKind::IsPanicking, Status::Present),
            }
        }
    }

    /// `getExploredPositions()`: the explored list, newest first, empty where it has none.
    pub(crate) fn explored(memories: &Memories) -> Vec<BlockPos> {
        match memories.get(MemoryKind::SnifferExploredPositions) {
            Some(Value::Blocks(list)) => list,
            _ => Vec::new(),
        }
    }

    /// `canSniff()`: not tempted, not panicking, not in water, not in love, on the
    /// ground, riding nothing and on no lead.
    pub(crate) fn can_sniff(ai: &Ai<'_, '_>, marks: Marks) -> bool {
        let now = ai.tick.lived;
        !marks.tempted
            && !marks.panicking
            && !ai.in_water()
            && !animal::animal_of(&ai.tick.mob.state).in_love(now)
            && ai.on_ground()
            && ai.tick.vehicle.is_none()
            && !ai.tick.leashed
    }

    /// `canDig(pos)`: a diggable block not among the explored, to which a path of reach
    /// one reaches (one path search each time, as the game's).
    pub(crate) fn can_dig_at(ai: &mut Ai<'_, '_>, pos: BlockPos, explored: &[BlockPos]) -> bool {
        if !diggable(ai.tick.world.state(pos)) || explored.contains(&pos) {
            return false;
        }
        navigation::create_path(ai, pos, 1)
            .is_some_and(|made| made.path(ai).is_some_and(|path| path.reached))
    }

    /// `canDig()`: not panicking, not tempted, grown, not in water, on the ground,
    /// riding nothing, and the block below its nose diggable and reached.
    pub(crate) fn can_dig(ai: &mut Ai<'_, '_>, marks: Marks, explored: &[BlockPos]) -> bool {
        if marks.panicking
            || marks.tempted
            || animal::is_baby(&ai.tick.mob)
            || ai.in_water()
            || !ai.on_ground()
            || ai.tick.vehicle.is_some()
        {
            return false;
        }
        let head = head_block(ai.position(), ai.tick.yaw, ai.tick.pitch);
        can_dig_at(ai, head.offset(0, -1, 0), explored)
    }

    /// `calculateDigPosition()`: up to five random places on land, 10, 12, 14, 16 and 18
    /// across and 3 up or down, the first whose ground it can dig (within the world's
    /// border, which is not kept: every place is within it).
    pub(crate) fn calculate_dig_position(
        ai: &mut Ai<'_, '_>,
        explored: &[BlockPos],
    ) -> Option<BlockPos> {
        for i in 0..5 {
            let Some(found) = random_pos::land_pos(ai, 10 + 2 * i, 3) else {
                continue;
            };
            let ground = physics::containing(found).offset(0, -1, 0);
            if can_dig_at(ai, ground, explored) {
                return Some(ground);
            }
        }
        None
    }

    /// `onDiggingComplete(timedOut)`: a dig run its whole length explores the block it
    /// stood on (`getOnPos`), as the explored list's newest.
    pub(crate) fn on_digging_complete(
        explored: &[BlockPos],
        on_pos: BlockPos,
        timed_out: bool,
    ) -> Option<Vec<BlockPos>> {
        timed_out.then(|| store_explored(explored, on_pos))
    }

    /// `canMate`'s state test of one sniffer: idle, scenting or happy.
    pub(crate) fn may_mate_in(state: State) -> bool {
        matches!(state, State::Idling | State::Scenting | State::FeelingHappy)
    }
}

// ------------------------------------------------------------------------------------
// The brain (`Sniffer.brainProvider`, `SnifferAi`).

/// `SNIFFING_COOLDOWN_TICKS`: how long after a dig it does not sniff again.
pub(crate) const SNIFF_COOLDOWN: i64 = 9600;

/// `FOOD_TEMPTATIONS` of a sniffer (`TemptingSensor.forAnimal`: `isFood`).
static TEMPTATIONS: TemptingSensor = TemptingSensor {
    tempts: |_, stack| FOOD.test(Some(stack)),
};

/// `Sniffer.brainProvider()` (Sniffer.txt 1104–1115): its explored positions named, and
/// its four sensors in the game's order.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[MemoryKind::SnifferExploredPositions],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::HURT_BY,
        &sensors::NEAREST_PLAYERS,
        &TEMPTATIONS,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `SnifferAi.getActivities()`: core, idle, sniff and dig, in that order.
fn activities() -> Vec<ActivityData> {
    let mut panic = AnimalPanic::new(2.0);
    panic.on_start = reset_sniffing;
    let mut love = AnimalMakeLove::new(entity_types::SNIFFER as u16);
    love.on_start = reset_sniffing;
    let mut tempted = FollowTemptation::new(Speed::Fixed(1.25));
    tempted.close_enough = |cx| {
        if animal::is_baby(&cx.ai.tick.mob) {
            2.5
        } else {
            3.5
        }
    };
    tempted.on_start = reset_sniffing;
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            panic::swim(0.8),
            panic.boxed(),
            Box::new(MoveToTargetSink::new(500, 700)),
            count_down(MemoryKind::TemptationCooldownTicks),
        ],
    );
    let mut idle = ActivityData::numbered(
        Activity::Idle,
        0,
        vec![
            love.boxed(),
            tempted.boxed(),
            look_sink(45, 90),
            held(FeelingHappy),
            Box::new(Gate::run_one(vec![
                (walk_to_look_target(1.0, 3), 2),
                (held(Scenting), 1),
                (held(Sniffing), 1),
                (look_at(LookAt::Player, 6.0), 1),
                (stroll(1.0), 1),
                (Box::new(HeldTimed::new(DoNothing { min: 5, max: 20 })), 2),
            ])),
        ],
    );
    idle.conditions = vec![(MemoryKind::SnifferDigging, Status::Absent)];
    let mut sniff = ActivityData::numbered(Activity::Sniff, 0, vec![held(Searching)]);
    sniff.conditions = vec![
        (MemoryKind::IsPanicking, Status::Absent),
        (MemoryKind::SnifferSniffingTarget, Status::Present),
        (MemoryKind::WalkTarget, Status::Present),
    ];
    // Both at priority 0 (explicit pairs).
    let dig = ActivityData {
        activity: Activity::Dig,
        behaviours: vec![(0, held(Digging)), (0, held(FinishedDigging))],
        conditions: vec![
            (MemoryKind::IsPanicking, Status::Absent),
            (MemoryKind::WalkTarget, Status::Absent),
            (MemoryKind::SnifferDigging, Status::Present),
        ],
        erase: Vec::new(),
    };
    vec![core, idle, sniff, dig]
}

/// What a fresh load of a sniffer resets (the plan's 2.10, the brief's 7.9.8): its state
/// and its seed's tick, synced data the game does not save, so that it is idle with no
/// seed to drop; the brain's fresh load forgets its dig, its place to sniff and its
/// happiness (memories of no codec), so that it does not finish that dig. For the brain's
/// `fresh_load` when it comes.
#[allow(
    dead_code,
    reason = "the fresh load of N7j1's contracts, not built yet"
)]
pub(crate) fn fresh_load(mob: &mut Mob) {
    update(mob, |own| *own = SnifferState::default());
}

/// `SnifferAi.updateActivity`: dig, else sniff, else idle, the first whose conditions
/// hold (none changes nothing: a dig with a walk target and no sniffing target keeps the
/// activity it had).
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_to_first_valid(&[Activity::Dig, Activity::Sniff, Activity::Idle]);
}

/// `SnifferAi.resetSniffing`: no dig and no place to sniff, and idle again.
fn reset_sniffing(cx: &mut Cx<'_, '_, '_>) {
    cx.memories.erase(MemoryKind::SnifferDigging);
    cx.memories.erase(MemoryKind::SnifferSniffingTarget);
    transition_to(cx.ai.tick, State::Idling);
}

/// `canSniff()` of the sniffer in its brain's tick.
fn can_sniff(cx: &Cx<'_, '_, '_>) -> bool {
    brain_seams::can_sniff(cx.ai, brain_seams::Marks::of(cx.memories))
}

/// `canDig()` of the sniffer in its brain's tick: a path search.
fn can_dig(cx: &mut Cx<'_, '_, '_>) -> bool {
    let marks = brain_seams::Marks::of(cx.memories);
    let explored = brain_seams::explored(cx.memories);
    brain_seams::can_dig(cx.ai, marks, &explored)
}

/// A `Behavior` of the sniffer's own: the game's start, tick and stop, with its stop told
/// whether it ran its whole time (`timedOut(time)` at the stop, which three of them read
/// and which [`brain::Timed`] does not pass on).
trait SnifferBehaviour: Sync + 'static {
    /// The entry condition (`Map.of(...)`, a conjunction).
    fn entry(&self) -> &'static [(MemoryKind, Status)];
    /// `minDuration` and `maxDuration`.
    fn durations(&self) -> (i64, i64);
    /// `checkExtraStartConditions`.
    fn extra_start(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let _ = cx;
        true
    }
    /// `canStillUse`.
    fn can_still_use(&self, cx: &mut Cx<'_, '_, '_>) -> bool;
    fn start(&self, cx: &mut Cx<'_, '_, '_>);
    /// `stop`, with `timedOut(time)`.
    fn stop(&self, cx: &mut Cx<'_, '_, '_>, timed_out: bool);
}

/// What a sniffer's behaviour keeps: whether it runs, and its end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
struct Run {
    running: bool,
    end: i64,
}

/// A [`SnifferBehaviour`] and its run.
struct Held<B: SnifferBehaviour> {
    behaviour: B,
    run: Run,
}

fn held<B: SnifferBehaviour>(behaviour: B) -> Box<dyn Behaviour> {
    Box::new(Held {
        behaviour,
        run: Run::default(),
    })
}

impl<B: SnifferBehaviour> Behaviour for Held<B> {
    fn required(&self, into: &mut BTreeSet<MemoryKind>) {
        into.extend(self.behaviour.entry().iter().map(|(kind, _)| *kind));
    }

    fn running(&self) -> bool {
        self.run.running
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let met = self
            .behaviour
            .entry()
            .iter()
            .all(|(kind, status)| cx.memories.check(*kind, *status));
        if !met || !self.behaviour.extra_start(cx) {
            return false;
        }
        self.run.running = true;
        // `Behavior.tryStart`: one draw of the level's at every start, also where the two
        // durations are the same.
        let (min, max) = self.behaviour.durations();
        let bound = u32::try_from(max + 1 - min).unwrap_or(1).max(1);
        self.run.end = cx.time + min + i64::from(cx.ai.tick.draws.next_int(bound));
        self.behaviour.start(cx);
        true
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        // None of the sniffer's own ticks.
        if cx.time > self.run.end || !self.behaviour.can_still_use(cx) {
            self.do_stop(cx);
        }
    }

    fn do_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        let timed_out = cx.time > self.run.end;
        self.run = Run::default();
        self.behaviour.stop(cx, timed_out);
    }

    fn load(&mut self, first: u16, parts: &BTreeMap<u16, Entry>) {
        self.run = read_entry(first, 1, parts);
    }

    fn save(&self, first: u16, into: &mut Vec<Entry>) {
        if self.run != Run::default() {
            into.push(entry(first, 1, &self.run));
        }
    }
}

/// `SnifferAi$Scenting(40, 80)`: idle and untempted, it scents for its whole time.
struct Scenting;

impl SnifferBehaviour for Scenting {
    fn entry(&self) -> &'static [(MemoryKind, Status)] {
        &[
            (MemoryKind::IsPanicking, Status::Absent),
            (MemoryKind::SnifferDigging, Status::Absent),
            (MemoryKind::SnifferSniffingTarget, Status::Absent),
            (MemoryKind::SnifferHappy, Status::Absent),
            (MemoryKind::BreedTarget, Status::Absent),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (40, 80)
    }

    fn extra_start(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        !brain_seams::Marks::of(cx.memories).tempted
    }

    fn can_still_use(&self, _: &mut Cx<'_, '_, '_>) -> bool {
        true
    }

    fn start(&self, cx: &mut Cx<'_, '_, '_>) {
        transition_to(cx.ai.tick, State::Scenting);
    }

    fn stop(&self, cx: &mut Cx<'_, '_, '_>, _: bool) {
        transition_to(cx.ai.tick, State::Idling);
    }
}

/// `SnifferAi$Sniffing(40, 80)`: a grown sniffer that may sniff sniffs; sniffed its whole
/// time, it chooses a place to dig and walks to it.
struct Sniffing;

impl SnifferBehaviour for Sniffing {
    fn entry(&self) -> &'static [(MemoryKind, Status)] {
        &[
            (MemoryKind::WalkTarget, Status::Absent),
            (MemoryKind::SnifferSniffingTarget, Status::Absent),
            (MemoryKind::SniffCooldown, Status::Absent),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (40, 80)
    }

    fn extra_start(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        !animal::is_baby(&cx.ai.tick.mob) && can_sniff(cx)
    }

    fn can_still_use(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        can_sniff(cx)
    }

    fn start(&self, cx: &mut Cx<'_, '_, '_>) {
        transition_to(cx.ai.tick, State::Sniffing);
    }

    fn stop(&self, cx: &mut Cx<'_, '_, '_>, timed_out: bool) {
        transition_to(cx.ai.tick, State::Idling);
        if !timed_out {
            return;
        }
        let explored = brain_seams::explored(cx.memories);
        if let Some(pos) = brain_seams::calculate_dig_position(cx.ai, &explored) {
            cx.memories
                .set(MemoryKind::SnifferSniffingTarget, Value::Block(pos));
            cx.memories
                .set(MemoryKind::WalkTarget, walk(Tracker::Block(pos), 1.25, 0));
        }
    }
}

/// `SnifferAi$Searching` (600): it walks to its place, searching; where it arrives and
/// may dig there (its nose's block), it digs.
struct Searching;

impl SnifferBehaviour for Searching {
    fn entry(&self) -> &'static [(MemoryKind, Status)] {
        &[
            (MemoryKind::WalkTarget, Status::Present),
            (MemoryKind::IsPanicking, Status::Absent),
            (MemoryKind::SnifferSniffingTarget, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (600, 600)
    }

    fn extra_start(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        can_sniff(cx)
    }

    fn can_still_use(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !can_sniff(cx) {
            transition_to(cx.ai.tick, State::Idling);
            return false;
        }
        // `WalkTarget.getTarget().currentBlockPosition()`: a block's own, never unknown.
        let walking = walk_target(cx).and_then(|walk| block_of(cx, &walk.to));
        let target = match cx.memories.get(MemoryKind::SnifferSniffingTarget) {
            Some(Value::Block(pos)) => Some(pos),
            _ => None,
        };
        walking.is_some() && walking == target
    }

    fn start(&self, cx: &mut Cx<'_, '_, '_>) {
        transition_to(cx.ai.tick, State::Searching);
    }

    fn stop(&self, cx: &mut Cx<'_, '_, '_>, _: bool) {
        if can_dig(cx) && can_sniff(cx) {
            cx.memories
                .set(MemoryKind::SnifferDigging, Value::Bool(true));
        }
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::SnifferSniffingTarget);
    }
}

/// `SnifferAi$Digging(160, 180)`: it digs while it may, its seed 120 ticks in; dug its
/// whole time, it does not sniff for 9600 ticks; cut short, it is idle again.
struct Digging;

impl SnifferBehaviour for Digging {
    fn entry(&self) -> &'static [(MemoryKind, Status)] {
        &[
            (MemoryKind::IsPanicking, Status::Absent),
            (MemoryKind::WalkTarget, Status::Absent),
            (MemoryKind::SnifferDigging, Status::Present),
            (MemoryKind::SniffCooldown, Status::Absent),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (160, 180)
    }

    fn extra_start(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        can_sniff(cx)
    }

    fn can_still_use(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let now = cx.ai.tick.lived;
        cx.memories
            .check(MemoryKind::SnifferDigging, Status::Present)
            && can_dig(cx)
            && !animal::animal_of(&cx.ai.tick.mob.state).in_love(now)
    }

    fn start(&self, cx: &mut Cx<'_, '_, '_>) {
        transition_to(cx.ai.tick, State::Digging);
    }

    fn stop(&self, cx: &mut Cx<'_, '_, '_>, timed_out: bool) {
        if timed_out {
            cx.memories
                .set_with_expiry(MemoryKind::SniffCooldown, Value::Unit, SNIFF_COOLDOWN);
        } else {
            reset_sniffing(cx);
        }
    }
}

/// `SnifferAi$FinishedDigging(40)`: after a whole dig, it rises for 40 ticks, then
/// remembers where it stood and is happy.
struct FinishedDigging;

impl SnifferBehaviour for FinishedDigging {
    fn entry(&self) -> &'static [(MemoryKind, Status)] {
        &[
            (MemoryKind::IsPanicking, Status::Absent),
            (MemoryKind::WalkTarget, Status::Absent),
            (MemoryKind::SnifferDigging, Status::Present),
            (MemoryKind::SniffCooldown, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (40, 40)
    }

    fn can_still_use(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories
            .check(MemoryKind::SnifferDigging, Status::Present)
    }

    fn start(&self, cx: &mut Cx<'_, '_, '_>) {
        transition_to(cx.ai.tick, State::Rising);
    }

    fn stop(&self, cx: &mut Cx<'_, '_, '_>, timed_out: bool) {
        transition_to(cx.ai.tick, State::Idling);
        // `onDiggingComplete(timedOut)`: the block it stands on (`getOnPos()`).
        let on_pos = cx.ai.tick.living.body.on_pos(cx.ai.tick.world, 1.0e-5);
        let explored = brain_seams::explored(cx.memories);
        if let Some(list) = brain_seams::on_digging_complete(&explored, on_pos, timed_out) {
            cx.memories
                .set(MemoryKind::SnifferExploredPositions, Value::Blocks(list));
        }
        cx.memories.erase(MemoryKind::SnifferDigging);
        cx.memories.set(MemoryKind::SnifferHappy, Value::Bool(true));
    }
}

/// `SnifferAi$FeelingHappy(40, 100)`: happy for its whole time after a dig.
struct FeelingHappy;

impl SnifferBehaviour for FeelingHappy {
    fn entry(&self) -> &'static [(MemoryKind, Status)] {
        &[(MemoryKind::SnifferHappy, Status::Present)]
    }

    fn durations(&self) -> (i64, i64) {
        (40, 100)
    }

    fn can_still_use(&self, _: &mut Cx<'_, '_, '_>) -> bool {
        true
    }

    fn start(&self, cx: &mut Cx<'_, '_, '_>) {
        transition_to(cx.ai.tick, State::FeelingHappy);
    }

    fn stop(&self, cx: &mut Cx<'_, '_, '_>, _: bool) {
        transition_to(cx.ai.tick, State::Idling);
        cx.memories.erase(MemoryKind::SnifferHappy);
    }
}

/// `dropSeed()`: at its tick, a seed (a torchflower's or a pitcher pod, one draw of two)
/// at the corner of its nose's block, with its sound.
fn drop_seed(tick: &mut MobTick<'_>) {
    let own = read(&tick.mob);
    if own.drop_seed_at != game_tick_count(tick) {
        return;
    }
    let head = head_block(tick.living.body.position, tick.yaw, tick.pitch);
    tick.out.local.push(Local::Drop {
        stack: seed(tick.draws),
        at: [f64::from(head.x), f64::from(head.y), f64::from(head.z)],
        pick_up_delay: DEFAULT_PICK_UP_DELAY,
    });
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_SNIFFER_DROP_SEED,
        volume: 1.0,
        pitch: 1.0,
    });
}

/// `gameplay/sniffer_digging`: one roll of two entries of one weight each.
pub(crate) fn seed(draws: &mut Random) -> ItemStack {
    let item = if draws.next_int(2) == 0 {
        items::TORCHFLOWER_SEEDS
    } else {
        items::PITCHER_POD
    };
    ItemStack { item, count: 1 }
}

/// `emitDiggingParticles`' part the server's world hears: every tenth tick while it
/// digs, `ENTITY_ACTION` at its nose's block, in the window of the particles or not (the
/// particles and the hit sounds are the client's).
fn dig_game_event(tick: &mut MobTick<'_>) {
    if game_tick_count(tick) % 10 != 0 {
        return;
    }
    let head = head_block(tick.living.body.position, tick.yaw, tick.pitch);
    tick.out.local.push(Local::GameEvent {
        name: "minecraft:entity_action",
        pos: head,
    });
}

/// The sniffer.
pub(crate) struct Sniffer;

impl MobType for Sniffer {
    /// `Animal.finalizeSpawn` (`Sniffer` adds none): the group's babies one in twenty,
    /// a snifflet at its own start age (`getBabyStartAge`).
    fn finalize_spawn(&self, spawn: &mut super::Spawning<'_>) {
        let group = super::SpawnGroup::Ageable(super::AgeableGroup::new(true));
        if animal::ageable_draw(spawn, group) {
            animal::update(spawn.mob, |shared| shared.set_age(BABY_START_AGE, 0));
        }
    }

    /// The constructor's navigation and maluses (and `Animal`'s): no goals, as its mind is
    /// its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.config.navigation.set_can_float(true);
        setup.set_malus(PathType::Water, -1.0);
        setup.set_malus(PathType::OnTopOfPowderSnow, -1.0);
        setup.set_malus(PathType::DamageCautious, -1.0);
    }

    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 50,
            ..HeadLimits::default()
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `Sniffer.customServerAiStep`: the brain's tick and its activity's update, then the
    /// animal's.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        brain::run(tick, |brain, _| update_activity(brain));
        animal::custom_server_ai_step(tick);
    }

    /// `Sniffer.tick()` before the animal's: digging, its game event and its seed.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        if read(&tick.mob).state == State::Digging {
            dig_game_event(tick);
            drop_seed(tick);
        }
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Sniffer.jumpFromGround()` after the living one: a sniffer stuck at a step while
    /// it means to move is pushed a tenth ahead.
    fn jumped_from_ground(&self, tick: &mut MobTick<'_>) {
        let motion = tick.living.body.motion;
        if tick.mob.body.controls.movement.speed > 0.0
            && motion[0] * motion[0] + motion[2] * motion[2] < 0.01
        {
            let push = crate::physics::minecart::input_vector([0.0, 0.0, 1.0], 0.1, tick.yaw);
            tick.living.body.motion = physics::add(motion, push);
        }
    }

    /// `Sniffer.mobInteract`: the animal's feeding with torchflower seeds, after which
    /// it eats once more (`playEatingSound` is called by `Animal.mobInteract` and again
    /// by the sniffer's own: two sounds, each a draw of the level's random).
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let food = FOOD.test(used.held);
        let result = animal::mob_interact(used, FOOD, None);
        if food && result != Interaction::Pass {
            for _ in 0..2 {
                // `Mth.randomBetween(random, 0.8, 1.2)`.
                let pitch = used.draws.next_float() * (1.2 - 0.8) + 0.8;
                used.did.push(Did::Sound {
                    sound: sounds::ENTITY_SNIFFER_EAT,
                    source: SOUND_SOURCE_NEUTRAL,
                    at: used.position,
                    volume: 1.0,
                    pitch,
                    but_the_player: false,
                });
            }
        }
        result
    }

    /// `getBreedOffspring`: a sniffer (what an egg used on one makes), a baby of its
    /// own start age.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(hatchling())
    }

    /// `spawnChildFromBreeding`: an egg laid at its feet, and no child.
    fn laid_from_breeding(&self) -> Option<Laid> {
        Some(Laid {
            stack: ItemStack {
                item: items::SNIFFER_EGG,
                count: 1,
            },
            sound: sounds::BLOCK_SNIFFER_EGG_PLOP,
            pitch_base: 0.5,
        })
    }

    /// `Sniffer.canMate`: both sniffers, each idle, scenting or happy.
    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        this.entity.kind == partner.entity.kind
            && brain_seams::may_mate_in(read(this.mob).state)
            && brain_seams::may_mate_in(read(partner.mob).state)
    }

    /// `Sniffer.die`: idle again before it dies.
    fn before_die(&self, mob: &mut Mob) {
        update(mob, |own| own.state = State::Idling);
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    /// `getDefaultDimensions`: lower by 0.4 while it digs (scaled as a baby is), else
    /// the animal's.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let info = clustine_data::EntityTypeInfo::of(clustine_data::entity_types::SNIFFER);
        let scale = if animal::is_baby(mob) { 0.5 } else { 1.0 };
        if read(mob).state == State::Digging {
            return Some(Dimensions {
                width: info.width * scale,
                height: (info.height - DIGGING_LOWER_BY) * scale,
                eye_height: DIGGING_EYE_HEIGHT * scale,
            });
        }
        animal::dimensions(
            mob,
            super::water::scaled(info.width, info.height, info.eye_height, 0.5),
        )
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// `isFood` and `supportQuadLeash`: a lead holds it by four.
    fn quad_leash(&self, _: u16) -> bool {
        true
    }

    /// `getAmbientSound`: none while it digs or searches.
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        match read(mob).state {
            State::Digging | State::Searching => None,
            _ => Some(sounds::ENTITY_SNIFFER_IDLE),
        }
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SNIFFER_STEP)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SNIFFER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SNIFFER_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        let mut values = animal::shown(mob);
        values.push((
            STATE_INDEX,
            DataShown::Serialised(Serialiser::SnifferState, own.state.id()),
            own.state == State::Idling,
        ));
        values.push((
            DROP_SEED_INDEX,
            DataShown::Int(own.drop_seed_at),
            own.drop_seed_at == 0,
        ));
        values
    }
}

/// A snifflet hatched from an egg (`SnifferEggBlock`'s hatching): `EntityType.create(level,
/// BREEDING)` with no `finalizeSpawn`, `setBaby(true)` of a sniffer's own start age.
pub(crate) fn hatchling() -> Mob {
    let mut mob = Mob::fresh();
    animal::update(&mut mob, |shared| shared.set_age(BABY_START_AGE, 0));
    mob
}
