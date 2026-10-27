//! The brain (`ai.Brain`, the brief's 7.1; `docs/groundwork/n7j-plan.md`, section 2): a
//! type's provider names its memories, sensors and activities; each mob's brain is its
//! memories, its sensors' next scans, its active activity and each behaviour's state, all
//! in its record but the memories its sensors rewrite whole, which the region keeps and
//! senses again after a take-in. Its tick is the game's, in order: the forget pass, the
//! sensors in their order, the start pass over the active activities by priority, their
//! buckets and their order of adding, and the running pass over every running behaviour,
//! active or not.
//!
//! As the goals' are, a behaviour's code is its type's and its state the record's: the
//! provider makes the behaviours afresh for each tick (`createActivities`, which the game
//! calls for each brain), each is numbered in the order it was made, and its state is read
//! before the tick and written after it, a behaviour at rest as a fresh one writing
//! nothing.

pub(crate) mod activity;
#[allow(
    dead_code,
    reason = "the shared behaviours the types of N7j's parts build their brains of"
)]
pub(crate) mod behaviours;
#[cfg(test)]
mod behaviours_tests;
pub(crate) mod memory;
#[allow(
    dead_code,
    reason = "the sensors the types of N7j's parts build their brains of"
)]
pub(crate) mod sensors;
#[cfg(test)]
mod sensors_tests;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::Entry;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub(crate) use activity::Activity;
pub(crate) use memory::{Countdown, Memories, MemoryKind, Slot, Status, Tracker, Value};

use super::ai::Ai;
use super::part;

/// The ids of the brain's parts in the mind, above every goal's.
pub(crate) mod ids {
    /// The brain's own state: its sensors' next scans and its active activity.
    pub(crate) const STATE: u16 = 0x4000;
    /// A memory's part is this plus its kind's number.
    pub(crate) const FIRST_MEMORY: u16 = 0x4100;
    /// A behaviour's part is this plus its number among the provider's behaviours.
    pub(crate) const FIRST_BEHAVIOUR: u16 = 0x4800;
}

/// The memories a sensor's scan rewrites whole, as the region keeps them beside the
/// record (`region/items.rs`'s `Memory`); none until a brain has ticked since the
/// entity was taken in, which then senses them again before its first start pass.
pub(crate) type Sensed = BTreeMap<MemoryKind, Slot>;

/// What a behaviour or a sensor works with: the mob's AI step, its memories, and the game
/// time its behaviours count in (`level.getGameTime()`).
pub(crate) struct Cx<'c, 't, 'w> {
    pub(crate) ai: &'c mut Ai<'t, 'w>,
    pub(crate) memories: &'c mut Memories,
    pub(crate) time: i64,
}

/// A sensor (`ai.sensing.Sensor`): its scan rate, the memories it requires (which it
/// registers), and its scan.
pub(crate) trait Sensor: Sync {
    /// `scanRate`.
    fn rate(&self) -> u32 {
        20
    }

    /// `requires()`.
    fn requires(&self) -> &'static [MemoryKind];

    /// `doTick`: its scan, which writes what it senses. A scan draws nothing.
    fn scan(&self, cx: &mut Cx<'_, '_, '_>);

    /// Its scan again after a take-in (the plan, section 8 item 1): what it senses and
    /// nothing else, nothing kept written; its phase does not move. By default its scan;
    /// a sensor that also writes kept memories writes only its sensed ones here, and one
    /// that senses nothing does nothing.
    fn resense(&self, cx: &mut Cx<'_, '_, '_>) {
        self.scan(cx);
    }
}

/// A behaviour as the brain holds it (`BehaviorControl`), whatever its kind.
pub(crate) trait Behaviour {
    /// `getRequiredMemories()`, added to `into`.
    fn required(&self, into: &mut BTreeSet<MemoryKind>);
    /// `getStatus() == RUNNING`.
    fn running(&self) -> bool;
    /// `tryStart`.
    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool;
    /// `tickOrStop`.
    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>);
    /// `doStop`.
    fn do_stop(&mut self, cx: &mut Cx<'_, '_, '_>);
    /// How many numbers it takes: one, and a gate's children's.
    fn count(&self) -> usize {
        1
    }
    /// Its state from the brain's parts, numbered from `first`; fresh where there is
    /// none or it cannot be read.
    fn load(&mut self, first: u16, parts: &BTreeMap<u16, Entry>);
    /// Its parts, numbered from `first`, each only where it is not as a fresh one is.
    fn save(&self, first: u16, into: &mut Vec<Entry>);
    /// Its activity is no longer active after the brain's tick at `lived` `now` and game
    /// time `time`: the game does not ask it again until its activity is active once
    /// more, so what it counts by the ticks it is asked (a ticker, a search's wait)
    /// pauses here. Nothing for a behaviour that counts nothing.
    fn left(&mut self, now: u32, time: i64) {
        let _ = (now, time);
    }
}

/// `Behavior` (the brief's 7.1.6): a behaviour with entry conditions on memories, a
/// duration drawn at each start, and the game's start, tick and stop.
pub(crate) trait Timed: 'static {
    /// The fields of its own that it keeps between ticks.
    type State: Serialize + DeserializeOwned + Default + PartialEq;
    /// The version of [`Timed::State`]'s encoding.
    const VERSION: u8 = 1;

    /// `entryCondition`.
    fn entry(&self) -> &[(MemoryKind, Status)];

    /// `minDuration` and `maxDuration`: 60 and 60 by default.
    fn durations(&self) -> (i64, i64) {
        (60, 60)
    }

    /// `checkExtraStartConditions`.
    fn extra_start(&self, state: &mut Self::State, cx: &mut Cx<'_, '_, '_>) -> bool {
        let _ = (state, cx);
        true
    }

    /// `canStillUse`: false by default, so that it starts and stops in one tick.
    fn can_still_use(&self, state: &mut Self::State, cx: &mut Cx<'_, '_, '_>) -> bool {
        let _ = (state, cx);
        false
    }

    /// `timedOut(time)`: the game time past its end.
    fn timed_out(&self, state: &Self::State, cx: &Cx<'_, '_, '_>, end: i64) -> bool {
        let _ = state;
        cx.time > end
    }

    fn start(&self, state: &mut Self::State, cx: &mut Cx<'_, '_, '_>) {
        let _ = (state, cx);
    }

    fn tick(&self, state: &mut Self::State, cx: &mut Cx<'_, '_, '_>) {
        let _ = (state, cx);
    }

    fn stop(&self, state: &mut Self::State, cx: &mut Cx<'_, '_, '_>) {
        let _ = (state, cx);
    }
}

/// What a [`Timed`] behaviour keeps: whether it runs, its end, and its own fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct TimedState<S> {
    running: bool,
    end: i64,
    own: S,
}

/// A [`Timed`] behaviour and its state.
pub(crate) struct HeldTimed<B: Timed> {
    behaviour: B,
    state: TimedState<B::State>,
}

impl<B: Timed> HeldTimed<B> {
    pub(crate) fn new(behaviour: B) -> Self {
        Self {
            behaviour,
            state: TimedState::default(),
        }
    }
}

impl<B: Timed> Behaviour for HeldTimed<B> {
    fn required(&self, into: &mut BTreeSet<MemoryKind>) {
        into.extend(self.behaviour.entry().iter().map(|(kind, _)| *kind));
    }

    fn running(&self) -> bool {
        self.state.running
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let met = self
            .behaviour
            .entry()
            .iter()
            .all(|(kind, status)| cx.memories.check(*kind, *status));
        if !met || !self.behaviour.extra_start(&mut self.state.own, cx) {
            return false;
        }
        self.state.running = true;
        // One draw at every start, also where the two durations are the same.
        let (min, max) = self.behaviour.durations();
        let bound = u32::try_from(max + 1 - min).unwrap_or(1).max(1);
        let drawn = i64::from(cx.ai.tick.draws.next_int(bound));
        self.state.end = cx.time + min + drawn;
        self.behaviour.start(&mut self.state.own, cx);
        true
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        let end = self.state.end;
        if !self.behaviour.timed_out(&self.state.own, cx, end)
            && self.behaviour.can_still_use(&mut self.state.own, cx)
        {
            self.behaviour.tick(&mut self.state.own, cx);
        } else {
            self.do_stop(cx);
        }
    }

    fn do_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        self.state.running = false;
        // Its end is read only while it runs, and set again at its next start: one at
        // rest keeps none, so that it writes nothing but its own fields.
        self.state.end = 0;
        self.behaviour.stop(&mut self.state.own, cx);
    }

    fn load(&mut self, first: u16, parts: &BTreeMap<u16, Entry>) {
        self.state = parts
            .get(&first)
            .filter(|entry| entry.version == B::VERSION)
            .and_then(|entry| postcard::from_bytes(&entry.bytes).ok())
            .unwrap_or_default();
    }

    fn save(&self, first: u16, into: &mut Vec<Entry>) {
        if self.state != TimedState::default() {
            into.push(entry(first, B::VERSION, &self.state));
        }
    }
}

/// `OneShot` (and every `BehaviorBuilder` behaviour, which is one): a trigger that, where
/// it fires, runs for the rest of the tick and is stopped by its running pass. It keeps
/// nothing.
pub(crate) trait Shot: 'static {
    /// The memories its conditions name.
    fn required(&self) -> &[MemoryKind];
    /// `trigger`: its conditions, left to right, and its body where they hold.
    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool;
}

/// A behaviour that never starts and draws nothing: what the contract step of N10 gives
/// for a behaviour another part builds, so that a package that names it compiles before
/// that part lands (`docs/groundwork/n10-plan.md`, section 4.1).
struct Inert;

impl Shot for Inert {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, _: &mut Cx<'_, '_, '_>) -> bool {
        false
    }
}

/// [`Inert`], boxed.
#[allow(
    dead_code,
    reason = "the stubs of N10's parts V2 and V3 until they land"
)]
pub(crate) fn inert() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(Inert))
}

/// A [`Shot`] and whether it runs, which no tick ends with.
pub(crate) struct HeldShot<S: Shot> {
    shot: S,
    running: bool,
}

impl<S: Shot> HeldShot<S> {
    pub(crate) fn new(shot: S) -> Self {
        Self {
            shot,
            running: false,
        }
    }
}

impl<S: Shot> Behaviour for HeldShot<S> {
    fn required(&self, into: &mut BTreeSet<MemoryKind>) {
        into.extend(self.shot.required().iter().copied());
    }

    fn running(&self) -> bool {
        self.running
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        self.running = self.shot.trigger(cx);
        self.running
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        self.do_stop(cx);
    }

    fn do_stop(&mut self, _: &mut Cx<'_, '_, '_>) {
        self.running = false;
    }

    // A one-shot is stopped in the tick it fires: there is nothing to keep.
    fn load(&mut self, _: u16, _: &BTreeMap<u16, Entry>) {
        self.running = false;
    }

    fn save(&self, _: u16, _: &mut Vec<Entry>) {}
}

/// `DoNothing(min, max)`: always starts, for a drawn time.
pub(crate) struct DoNothing {
    pub(crate) min: i64,
    pub(crate) max: i64,
}

impl Timed for DoNothing {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[]
    }

    fn durations(&self) -> (i64, i64) {
        (self.min, self.max)
    }

    // `DoNothing.tickOrStop` stops only once its time is out.
    fn can_still_use(&self, _: &mut (), _: &mut Cx<'_, '_, '_>) -> bool {
        true
    }
}

/// `GateBehavior.OrderPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    Ordered,
    /// The children shuffled at each start, by weight (`ShufflingList.shuffle`).
    Shuffled,
}

/// `GateBehavior.RunningPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Running {
    /// The first child in order that starts; the rest are not tried.
    RunOne,
    /// Every stopped child is tried.
    TryAll,
}

/// What a gate keeps: whether it runs, and its children's order since its last shuffle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct GateState {
    running: bool,
    order: Vec<u16>,
}

/// `GateBehavior` (and `RunOne`, a shuffled gate that runs one): an entry condition,
/// memories erased as it stops, its policies, and its children with their weights.
pub(crate) struct Gate {
    pub(crate) entry: Vec<(MemoryKind, Status)>,
    pub(crate) erase: Vec<MemoryKind>,
    pub(crate) order: Order,
    pub(crate) running: Running,
    pub(crate) children: Vec<(Box<dyn Behaviour>, u32)>,
    state: GateState,
}

impl Gate {
    pub(crate) fn new(
        entry: Vec<(MemoryKind, Status)>,
        erase: Vec<MemoryKind>,
        order: Order,
        running: Running,
        children: Vec<(Box<dyn Behaviour>, u32)>,
    ) -> Self {
        Self {
            entry,
            erase,
            order,
            running,
            children,
            state: GateState::default(),
        }
    }

    /// `RunOne(children)`: no entry condition, shuffled, running one.
    pub(crate) fn run_one(children: Vec<(Box<dyn Behaviour>, u32)>) -> Self {
        Self::new(
            Vec::new(),
            Vec::new(),
            Order::Shuffled,
            Running::RunOne,
            children,
        )
    }

    /// The children's indices in their present order: as given until the first shuffle.
    fn order(&self) -> Vec<usize> {
        if self.state.order.len() == self.children.len() {
            self.state
                .order
                .iter()
                .map(|index| usize::from(*index))
                .collect()
        } else {
            (0..self.children.len()).collect()
        }
    }

    /// `ShufflingList.shuffle`: one `nextFloat` per child in its present order, its
    /// sort key `-pow(r, 1 / weight)` with the exponent a float widened, then a stable
    /// sort by the keys ascending.
    fn shuffle(&mut self, cx: &mut Cx<'_, '_, '_>) {
        let order = self.order();
        let mut keyed: Vec<(f64, usize)> = order
            .into_iter()
            .map(|index| {
                let weight = self.children[index].1 as f32;
                let r = f64::from(cx.ai.tick.draws.next_float());
                let exponent = f64::from(1.0_f32 / weight);
                (-libm::pow(r, exponent), index)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.state.order = keyed
            .into_iter()
            .map(|(_, index)| u16::try_from(index).expect("a gate has fewer than 65536 children"))
            .collect();
    }
}

impl Behaviour for Gate {
    fn required(&self, into: &mut BTreeSet<MemoryKind>) {
        into.extend(self.entry.iter().map(|(kind, _)| *kind));
        for (child, _) in &self.children {
            child.required(into);
        }
    }

    fn running(&self) -> bool {
        self.state.running
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !self
            .entry
            .iter()
            .all(|(kind, status)| cx.memories.check(*kind, *status))
        {
            return false;
        }
        self.state.running = true;
        if self.order == Order::Shuffled {
            self.shuffle(cx);
        }
        for index in self.order() {
            let child = &mut self.children[index].0;
            if child.running() {
                continue;
            }
            if child.try_start(cx) && self.running == Running::RunOne {
                break;
            }
        }
        // True even where no child started: the running pass stops it in this tick.
        true
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        for index in self.order() {
            let child = &mut self.children[index].0;
            if child.running() {
                child.tick_or_stop(cx);
            }
        }
        if !self.children.iter().any(|(child, _)| child.running()) {
            self.do_stop(cx);
        }
    }

    fn do_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        self.state.running = false;
        for index in self.order() {
            let child = &mut self.children[index].0;
            if child.running() {
                child.do_stop(cx);
            }
        }
        for kind in &self.erase {
            cx.memories.erase(*kind);
        }
    }

    fn left(&mut self, now: u32, time: i64) {
        for (child, _) in &mut self.children {
            child.left(now, time);
        }
    }

    fn count(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(|(child, _)| child.count())
            .sum::<usize>()
    }

    fn load(&mut self, first: u16, parts: &BTreeMap<u16, Entry>) {
        self.state = parts
            .get(&first)
            .and_then(|entry| postcard::from_bytes(&entry.bytes).ok())
            .unwrap_or_default();
        let mut next = first + 1;
        for (child, _) in &mut self.children {
            child.load(next, parts);
            next += child.count() as u16;
        }
    }

    fn save(&self, first: u16, into: &mut Vec<Entry>) {
        if self.state != GateState::default() {
            into.push(entry(first, 1, &self.state));
        }
        let mut next = first + 1;
        for (child, _) in &self.children {
            child.save(next, into);
            next += child.count() as u16;
        }
    }
}

/// `ActivityData`: an activity's behaviours with their priorities, the memories it
/// needs to be set, and those erased when it stops being active.
pub(crate) struct ActivityData {
    pub(crate) activity: Activity,
    pub(crate) behaviours: Vec<(i32, Box<dyn Behaviour>)>,
    pub(crate) conditions: Vec<(MemoryKind, Status)>,
    pub(crate) erase: Vec<MemoryKind>,
}

impl ActivityData {
    /// `ActivityData.create(activity, start, list)`: priorities from `start` up in list
    /// order, no conditions, nothing erased.
    pub(crate) fn numbered(
        activity: Activity,
        start: i32,
        behaviours: Vec<Box<dyn Behaviour>>,
    ) -> Self {
        Self {
            activity,
            behaviours: behaviours
                .into_iter()
                .zip(start..)
                .map(|(behaviour, priority)| (priority, behaviour))
                .collect(),
            conditions: Vec::new(),
            erase: Vec::new(),
        }
    }

    /// `create(activity, start, list, memory)`: as [`ActivityData::numbered`], needing
    /// `memory` present and erasing it when it stops.
    pub(crate) fn while_present(
        activity: Activity,
        start: i32,
        behaviours: Vec<Box<dyn Behaviour>>,
        memory: MemoryKind,
    ) -> Self {
        Self {
            conditions: vec![(memory, Status::Present)],
            erase: vec![memory],
            ..Self::numbered(activity, start, behaviours)
        }
    }
}

/// `Brain.Provider`: the memories it names (the three-argument `Brain.provider`), its
/// sensors in their order, and what makes its activities for a tick.
pub(crate) struct Provider {
    pub(crate) memories: &'static [MemoryKind],
    pub(crate) sensors: &'static [&'static dyn Sensor],
    pub(crate) activities: fn() -> Vec<ActivityData>,
    /// The activities `setCoreActivities` makes core: `{CORE}`.
    pub(crate) core: &'static [Activity],
    /// `defaultActivity`: `IDLE` unless the type sets another.
    pub(crate) default: Activity,
}

/// The brain's own state in the record: when it was made, each sensor's next scan, and
/// its active activity beside the core ones.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BrainState {
    /// The `lived` of each sensor's next scan, in the provider's order.
    pub(crate) next_scans: Vec<u32>,
    /// The active activity that is not core; none until the brain's first tick has
    /// made the default active.
    pub(crate) active: Option<Activity>,
}

impl part::Part for BrainState {
    const ID: u16 = ids::STATE;
    const VERSION: u8 = 1;
}

/// `Brain.<init>`'s draws for a brain made with a mob whose first tick will be at
/// `first` (its construction, not a load: the record carries the brain whole through
/// every take-in): one `nextInt(scanRate)` of the mob's own random per sensor in order
/// (`randomlyDelayStart`). A sensor whose delay is `k` scans at the brain's `k`-th tick,
/// the first where `k` is 0 or 1 (`Sensor.tick` lowers it before it looks).
pub(crate) fn made(provider: &Provider, first: u32, own: &mut crate::region::Random) -> BrainState {
    let next_scans = provider
        .sensors
        .iter()
        .map(|sensor| {
            let delay = own.next_int(sensor.rate().max(1));
            first + delay.max(1) - 1
        })
        .collect();
    BrainState {
        next_scans,
        active: None,
    }
}

/// A new mob's construction (`LivingEntity`'s constructor, then the type's): its brain
/// made, where its type has one, with the draws of its sensors' delays from the mob's own
/// stream, then the type's own draws.
pub(crate) fn construct(
    mob_type: &'static dyn super::types::MobType,
    mob: &mut super::Mob,
    own: &mut crate::region::Random,
) {
    if let Some(provider) = mob_type.brain() {
        let state = made(provider, 0, own);
        part::put(&mut mob.mind.brain, &state);
    }
    mob_type.construct(mob, own);
}

/// The brain's tick at a type's `customServerAiStep` (`brain.tick(level, this)`), and
/// `then` after it with the brain (the type's activity update, `XAi.updateActivity`):
/// read from the mob's record and the region's sensed memories, ticked, and written
/// back to both. Nothing for a type without a brain.
///
/// Where a behaviour asked for the brain to be made again ([`Memories::refresh`],
/// `refreshBrain`), the brain is then the new one: every behaviour still running stopped
/// (`stopAll`), and of the old one only the memories the game saves (`Brain.pack`),
/// beside the new brain's state drawn as the behaviour asked. Whether it was so made
/// again, so that the type does what its `refreshBrain` does after (a villager's
/// schedule). **Named**: the game stops what runs at the moment it asks and drops what
/// the rest of that tick writes into the old brain; here what runs at the end of the
/// tick is stopped, and what the rest of the tick wrote into the saved memories is kept.
pub(crate) fn run(
    tick: &mut super::tick::MobTick<'_>,
    then: impl FnOnce(&mut Brain, &mut Ai<'_, '_>),
) -> bool {
    let Some(provider) = tick.mob_type.brain() else {
        return false;
    };
    let now = tick.lived;
    let time = tick.world.game_time();
    let mut brain = Brain::read(provider, &tick.mob.mind.brain, tick.sensed.as_ref(), now);
    let mut setup = super::ai::Setup::default();
    tick.mob_type.register_goals(&mut setup);
    let config = setup.config;
    let (entries, sensed) = {
        let mut ai = Ai {
            tick,
            config: &config,
            sensing: super::ai::sensing::Sensing::default(),
            navigation_ticked: true,
        };
        brain.tick(&mut ai, now, time);
        then(&mut brain, &mut ai);
        if brain.memories.refresh.is_some() {
            brain.stop_all(&mut ai, time);
        }
        brain.write()
    };
    match brain.memories.refresh.take() {
        Some(state) => {
            let mut kept: Vec<Entry> = entries
                .into_iter()
                .filter(|entry| {
                    entry
                        .id
                        .checked_sub(ids::FIRST_MEMORY)
                        .filter(|_| entry.id < ids::FIRST_BEHAVIOUR)
                        .and_then(MemoryKind::of_id)
                        .is_some_and(MemoryKind::saved_by_game)
                })
                .collect();
            part::put(&mut kept, &state);
            tick.mob.mind.brain = kept;
            tick.sensed = Some(sensed);
            true
        }
        None => {
            tick.mob.mind.brain = entries;
            tick.sensed = Some(sensed);
            false
        }
    }
}

/// `refreshBrain` of a mob in its tick (a villager grown up, `ageBoundaryReached`): every
/// running behaviour stopped (`stopAll`), then a new brain of the same memories
/// (`pack`, `makeBrain`): every behaviour at rest, the default activity, and its sensors'
/// delays drawn again from the mob's own stream, for a first tick at its next `lived`.
/// Nothing for a type without a brain.
pub(crate) fn remake(tick: &mut super::tick::MobTick<'_>) {
    let Some(provider) = tick.mob_type.brain() else {
        return;
    };
    let now = tick.lived;
    let time = tick.world.game_time();
    let mut brain = Brain::read(provider, &tick.mob.mind.brain, tick.sensed.as_ref(), now);
    let mut setup = super::ai::Setup::default();
    tick.mob_type.register_goals(&mut setup);
    let config = setup.config;
    let (entries, sensed) = {
        let mut ai = Ai {
            tick,
            config: &config,
            sensing: super::ai::sensing::Sensing::default(),
            navigation_ticked: true,
        };
        brain.stop_all(&mut ai, time);
        brain.write()
    };
    // Of the old brain only its memories go on.
    let mut kept: Vec<Entry> = entries
        .into_iter()
        .filter(|entry| (ids::FIRST_MEMORY..ids::FIRST_BEHAVIOUR).contains(&entry.id))
        .collect();
    let state = made(provider, now.saturating_add(1), tick.draws);
    part::put(&mut kept, &state);
    tick.mob.mind.brain = kept;
    tick.sensed = Some(sensed);
}

/// The activity beside the core ones of a mob's brain as its record has it, set to
/// `activity` (a villager's schedule as its brain is made, `registerBrainGoals`).
pub(crate) fn set_recorded_activity(mob: &mut super::Mob, activity: Activity) {
    let mut state: BrainState = part::decode(&mob.mind.brain).unwrap_or_default();
    state.active = Some(activity);
    part::put(&mut mob.mind.brain, &state);
}

/// A write of one memory of another mob, which a mob's tick makes and its region applies
/// after it ([`super::tick::Local::SetMemories`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MemoryWrite {
    /// `setMemory(kind, value)`.
    Set(MemoryKind, Value),
    /// `setMemoryWithExpiry(kind, value, ttl)`.
    SetFor(MemoryKind, Value, i64),
    /// `eraseMemory(kind)`.
    Erase(MemoryKind),
}

impl MemoryWrite {
    /// The memory it writes.
    pub(crate) fn kind(&self) -> MemoryKind {
        match self {
            Self::Set(kind, _) | Self::SetFor(kind, _, _) | Self::Erase(kind) => *kind,
        }
    }

    /// The write made to `memories`, by the brain's own rules (an unregistered kind takes
    /// nothing).
    pub(crate) fn apply(self, memories: &mut Memories) {
        match self {
            Self::Set(kind, value) => memories.set(kind, value),
            Self::SetFor(kind, value, ttl) => memories.set_with_expiry(kind, value, ttl),
            Self::Erase(kind) => memories.erase(kind),
        }
    }
}

/// The memories of a mob of a type with a brain, as code outside its tick reads and
/// writes them (a hurt, a use, another mob's tick): as of its last brain tick, the one
/// at `lived − 1` of its record, every countdown after that tick's lowering. Its sensed
/// memories are not among them.
pub(crate) fn memories_of(provider: &'static Provider, mob: &super::Mob, lived: u32) -> Memories {
    let brain = Brain::read(
        provider,
        &mob.mind.brain,
        Some(&Sensed::new()),
        lived.saturating_sub(1),
    );
    brain.memories
}

/// The memories `memories` written back into a mob's record after code outside its tick
/// changed them.
pub(crate) fn write_memories(
    provider: &'static Provider,
    mob: &mut super::Mob,
    memories: Memories,
) {
    let lived = memories.now;
    let mut brain = Brain::read(provider, &mob.mind.brain, Some(&Sensed::new()), lived);
    brain.memories.slots = memories.slots;
    brain.memories.changed = memories.changed;
    mob.mind.brain = brain.write().0;
}

/// A brain for one tick: its memories, its state and its behaviours, as the record and
/// the region's sensed memories have them.
pub(crate) struct Brain {
    provider: &'static Provider,
    pub(crate) memories: Memories,
    pub(crate) state: BrainState,
    activities: Vec<ActivityData>,
    /// The activities' requirements and their memories to erase, by activity.
    requirements: BTreeMap<Activity, Requirement>,
    /// Whether the region had nothing of its sensed memories: the brain senses again
    /// before its first start pass.
    resense: bool,
    /// The game time of its tick, which the behaviours of an activity it leaves are
    /// told of.
    time: i64,
    /// The order of the start and running passes: (activity's index, behaviour's index).
    order: Vec<(usize, usize)>,
}

/// An activity's requirements, and the memories its end erases.
type Requirement = (Vec<(MemoryKind, Status)>, Vec<MemoryKind>);

impl Brain {
    /// The brain of `provider` from the mind's `entries`, with the region's `sensed`
    /// memories (none where it has not kept any since a take-in), at the mob's `lived`.
    pub(crate) fn read(
        provider: &'static Provider,
        entries: &[Entry],
        sensed: Option<&Sensed>,
        lived: u32,
    ) -> Self {
        let parts: BTreeMap<u16, Entry> = entries
            .iter()
            .map(|entry| (entry.id, entry.clone()))
            .collect();
        let state: BrainState = part::decode(entries).unwrap_or_default();
        let mut activities = (provider.activities)();
        let mut next = ids::FIRST_BEHAVIOUR;
        for data in &mut activities {
            for (_, behaviour) in &mut data.behaviours {
                behaviour.load(next, &parts);
                next += behaviour.count() as u16;
            }
        }
        // `Brain.<init>`: the provider's memories, the sensors' and the behaviours'
        // entry conditions'.
        let mut registered: BTreeSet<MemoryKind> = provider.memories.iter().copied().collect();
        for sensor in provider.sensors {
            registered.extend(sensor.requires().iter().copied());
        }
        let mut requirements = BTreeMap::new();
        for data in &activities {
            for (_, behaviour) in &data.behaviours {
                behaviour.required(&mut registered);
            }
            requirements.insert(data.activity, (data.conditions.clone(), data.erase.clone()));
        }
        let mut slots = BTreeMap::new();
        for entry in entries {
            let Some(kind) = entry
                .id
                .checked_sub(ids::FIRST_MEMORY)
                .filter(|_| entry.id < ids::FIRST_BEHAVIOUR)
                .and_then(MemoryKind::of_id)
            else {
                continue;
            };
            if let Ok(slot) = postcard::from_bytes::<Slot>(&entry.bytes) {
                slots.insert(kind, slot);
            }
        }
        if let Some(sensed) = sensed {
            slots.extend(sensed.iter().map(|(kind, slot)| (*kind, slot.clone())));
        }
        let order = pass_order(&activities);
        Self {
            provider,
            memories: Memories {
                registered,
                slots,
                now: lived,
                lowered: None,
                changed: BTreeSet::new(),
                active: BTreeSet::new(),
                requested: Vec::new(),
                refresh: None,
            },
            state,
            activities,
            requirements,
            resense: sensed.is_none(),
            time: 0,
            order,
        }
    }

    /// `Brain.tick` at the mob's `lived` `now` and the game time `time` (the brief's
    /// 7.1.5): the forget pass, the sensors (after a take-in every sensor senses again
    /// first), the start pass and the running pass. An activity a behaviour asks for
    /// ([`Memories::requested`], a villager's panic or schedule) is made active as soon
    /// as that behaviour returns, so that the behaviours after it in the passes see it
    /// as the game's do, which ask `activeActivities` afresh as they visit each activity.
    pub(crate) fn tick(&mut self, ai: &mut Ai<'_, '_>, now: u32, time: i64) {
        // A brain read before its first tick makes the default active, as its
        // construction's `useDefaultActivity` does.
        if self.state.active.is_none() {
            self.state.active = Some(self.provider.default);
        }
        self.memories.forget(now);
        self.time = time;
        self.memories.active = self.active();
        self.memories.requested.clear();
        let resense = std::mem::take(&mut self.resense);
        {
            let mut cx = Cx {
                ai: &mut *ai,
                memories: &mut self.memories,
                time,
            };
            if resense {
                for sensor in self.provider.sensors {
                    sensor.resense(&mut cx);
                }
            }
            if self.state.next_scans.len() != self.provider.sensors.len() {
                // A provider whose sensors changed: those it has now scan at once.
                self.state.next_scans = vec![now; self.provider.sensors.len()];
            }
            for (index, sensor) in self.provider.sensors.iter().enumerate() {
                let next = &mut self.state.next_scans[index];
                if now >= *next {
                    *next = now + sensor.rate().max(1);
                    sensor.scan(&mut cx);
                }
            }
        }
        for at in 0..self.order.len() {
            let (activity, behaviour) = self.order[at];
            let data = &mut self.activities[activity];
            if !self.memories.active.contains(&data.activity) {
                continue;
            }
            let behaviour = &mut data.behaviours[behaviour].1;
            if !behaviour.running() {
                let mut cx = Cx {
                    ai: &mut *ai,
                    memories: &mut self.memories,
                    time,
                };
                behaviour.try_start(&mut cx);
                self.take_requests();
            }
        }
        // Every running behaviour of every activity, listed before any ticks.
        let running: Vec<(usize, usize)> = self
            .order
            .iter()
            .copied()
            .filter(|(activity, behaviour)| {
                self.activities[*activity].behaviours[*behaviour]
                    .1
                    .running()
            })
            .collect();
        for (activity, behaviour) in running {
            let mut cx = Cx {
                ai: &mut *ai,
                memories: &mut self.memories,
                time,
            };
            self.activities[activity].behaviours[behaviour]
                .1
                .tick_or_stop(&mut cx);
            self.take_requests();
        }
        self.memories.end_tick();
    }

    /// The activities the behaviour that just returned asked for, each made active
    /// where it may be (`setActiveActivityIfPossible`), in the order asked.
    fn take_requests(&mut self) {
        if self.memories.requested.is_empty() {
            return;
        }
        for activity in std::mem::take(&mut self.memories.requested) {
            self.set_active_if_possible(activity);
        }
        self.memories.active = self.active();
    }

    /// The active activities: the core ones and the one beside them.
    pub(crate) fn active(&self) -> BTreeSet<Activity> {
        self.provider
            .core
            .iter()
            .copied()
            .chain(self.state.active)
            .collect()
    }

    /// `getActiveNonCoreActivity()`.
    pub(crate) fn active_non_core(&self) -> Option<Activity> {
        self.state
            .active
            .filter(|activity| !self.provider.core.contains(activity))
    }

    /// `activityRequirementsAreMet(activity)`: false for one never added.
    pub(crate) fn requirements_met(&self, activity: Activity) -> bool {
        self.requirements
            .get(&activity)
            .is_some_and(|(conditions, _)| {
                conditions
                    .iter()
                    .all(|(kind, status)| self.memories.check(*kind, *status))
            })
    }

    /// `setActiveActivity(activity)`: where it is not active already, the memories each
    /// active one other than it erases when stopped are erased, and it is active beside
    /// the core ones.
    pub(crate) fn set_active(&mut self, activity: Activity) {
        if self.active().contains(&activity) {
            return;
        }
        for other in self.active() {
            if other == activity {
                continue;
            }
            if let Some((_, erase)) = self.requirements.get(&other) {
                for kind in erase.clone() {
                    self.memories.erase(kind);
                }
            }
            // Its behaviours are not asked from the next tick on.
            if !self.provider.core.contains(&other) {
                let (now, time) = (self.memories.now, self.time);
                for data in &mut self.activities {
                    if data.activity == other {
                        for (_, behaviour) in &mut data.behaviours {
                            behaviour.left(now, time);
                        }
                    }
                }
            }
        }
        self.state.active = Some(activity);
    }

    /// `setActiveActivityIfPossible(activity)`: it where its requirements are met, else
    /// the default.
    pub(crate) fn set_active_if_possible(&mut self, activity: Activity) {
        if self.requirements_met(activity) {
            self.set_active(activity);
        } else {
            self.set_active(self.provider.default);
        }
    }

    /// `setActiveActivityToFirstValid(list)`: the first whose requirements are met; none
    /// changes nothing.
    pub(crate) fn set_active_to_first_valid(&mut self, list: &[Activity]) {
        if let Some(activity) = list
            .iter()
            .find(|activity| self.requirements_met(**activity))
        {
            self.set_active(*activity);
        }
    }

    /// `stopAll`: every running behaviour stopped, in the running pass's order.
    pub(crate) fn stop_all(&mut self, ai: &mut Ai<'_, '_>, time: i64) {
        let mut cx = Cx {
            ai,
            memories: &mut self.memories,
            time,
        };
        for &(activity, behaviour) in &self.order {
            let behaviour = &mut self.activities[activity].behaviours[behaviour].1;
            if behaviour.running() {
                behaviour.do_stop(&mut cx);
            }
        }
    }

    /// The brain's parts for the mind, and the sensed memories for the region.
    pub(crate) fn write(&self) -> (Vec<Entry>, Sensed) {
        let mut entries = Vec::new();
        part::put(&mut entries, &self.state);
        let mut sensed = Sensed::new();
        for (kind, slot) in &self.memories.slots {
            if kind.sensed() {
                sensed.insert(*kind, slot.clone());
            } else {
                entries.push(entry(ids::FIRST_MEMORY + kind.id(), 1, slot));
            }
        }
        let mut next = ids::FIRST_BEHAVIOUR;
        for data in &self.activities {
            for (_, behaviour) in &data.behaviours {
                behaviour.save(next, &mut entries);
                next += behaviour.count() as u16;
            }
        }
        entries.sort_by_key(|entry| entry.id);
        (entries, sensed)
    }
}

/// The order of the start and running passes (the brief's 7.1.5 and finding 6): by
/// priority ascending; at one priority the activities by their buckets, and those of one
/// bucket in the order they were first added at that priority; and an activity's
/// behaviours at that priority in the order they were added, each once.
fn pass_order(activities: &[ActivityData]) -> Vec<(usize, usize)> {
    let mut priorities: BTreeSet<i32> = BTreeSet::new();
    for data in activities {
        priorities.extend(data.behaviours.iter().map(|(priority, _)| *priority));
    }
    let mut order = Vec::new();
    for priority in priorities {
        let mut at: Vec<(u8, usize, usize)> = Vec::new();
        for (index, data) in activities.iter().enumerate() {
            if data.behaviours.iter().any(|(p, _)| *p == priority)
                && !at
                    .iter()
                    .any(|(_, _, seen)| activities[*seen].activity == data.activity)
            {
                at.push((data.activity.bucket(), at.len(), index));
            }
        }
        at.sort();
        for (_, _, first) in at {
            let activity = activities[first].activity;
            // Every activity data of this activity, in order: an activity added twice
            // has one set at a priority.
            for (index, data) in activities.iter().enumerate() {
                if data.activity != activity {
                    continue;
                }
                for (behaviour, (p, _)) in data.behaviours.iter().enumerate() {
                    if *p == priority {
                        order.push((index, behaviour));
                    }
                }
            }
        }
    }
    order
}

/// A part of the brain in the mind.
fn entry<T: Serialize>(id: u16, version: u8, value: &T) -> Entry {
    Entry {
        id,
        version,
        bytes: postcard::to_allocvec(value).expect("a brain's part is serialisable"),
    }
}
