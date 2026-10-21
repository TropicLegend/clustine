//! The shared behaviours (`ai.behavior`, the brief's 7.3), a file each or a few
//! together, with what they share: one-shots that keep state between ticks (the game's
//! closures' `MutableLong` and the like, which the contract's [`super::Shot`] does not
//! keep), counters the game lowers on some ticks only, and the readers of trackers and
//! walk targets.
//!
//! Every draw is the mob's stream's (`cx.ai.tick.draws`) at the game's place: the level's
//! on the mob's behalf, the mob's own, a shuffled list's and the JDK's.
//!
//! **Where types extend them** (the plan, section 8 item 7b): every behaviour a type's own
//! subclass changes takes what it changes as a plain function among its fields, which a
//! type sets in its provider without editing these files:
//!
//! - [`panic::AnimalPanic`]: `may_start` (the camel's `!isMobControlled`), `on_start`
//!   (the camel's `standUpInstantly`, the armadillo's `rollOut`, the sniffer's
//!   `resetSniffing`, before the panic's own start), `causes` (the armadillo's
//!   environmental causes) and `position` (the flying and swimming form).
//! - [`walking::MoveToTargetSink`]: `may_start` (the armadillo's "not while scared",
//!   asked before the sink's own conditions, so that its cooldown waits too).
//! - [`jumping::LongJumpToRandomPos`]: a [`jumping::Leaper`] (sounds, pose, friction,
//!   the landing test, the size in the pose) and `preferred` (the frog's
//!   `LongJumpToPreferredBlock`: its block tag and chance).
//! - [`social::AnimalMakeLove`] and [`social::FollowTemptation`]: `on_start` (the
//!   sniffer's `resetSniffing`).
//! - [`ramming::RamTarget`]: a [`ramming::Rammer`] (the goat's cooldowns, knockback,
//!   sounds and horn).

pub(crate) mod attacking;
pub(crate) mod cooldowns;
pub(crate) mod doors;
pub(crate) mod jumping;
pub(crate) mod liquids;
pub(crate) mod looking;
pub(crate) mod panic;
pub(crate) mod ramming;
pub(crate) mod social;
pub(crate) mod walking;

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::{BlockPos, EntityId, Entry};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::sensors::{self, find};
use super::{Behaviour, Cx, MemoryKind, Tracker, Value};
use crate::physics::{self, Vec3};
use crate::region::Random;

/// A part of the brain in the mind, as the contract writes its own.
pub(crate) fn entry<T: Serialize>(id: u16, version: u8, value: &T) -> Entry {
    Entry {
        id,
        version,
        bytes: postcard::to_allocvec(value).expect("a brain's part is serialisable"),
    }
}

/// A part read back: none where there is none, it is of another version or it cannot
/// be read.
pub(crate) fn read_entry<T: DeserializeOwned + Default>(
    first: u16,
    version: u8,
    parts: &BTreeMap<u16, Entry>,
) -> T {
    parts
        .get(&first)
        .filter(|entry| entry.version == version)
        .and_then(|entry| postcard::from_bytes(&entry.bytes).ok())
        .unwrap_or_default()
}

/// A one-shot (`BehaviorBuilder`) whose trigger keeps state between ticks, as the game's
/// closures do (`TryFindLiquid`'s `next`, `SetEntityLookTargetSometimes`' ticker,
/// `InteractWithDoor`'s last node): as [`super::Shot`], with its state.
pub(crate) trait KeptShot: 'static {
    type State: Serialize + DeserializeOwned + Default + PartialEq;
    const VERSION: u8 = 1;

    /// The memories its conditions name.
    fn required(&self) -> &[MemoryKind];

    /// `trigger`: its conditions, left to right, and its body where they hold.
    fn trigger(&self, state: &mut Self::State, cx: &mut Cx<'_, '_, '_>) -> bool;

    /// [`Behaviour::left`]: what it counts by its calls pauses.
    fn left(&self, state: &mut Self::State, now: u32, time: i64) {
        let _ = (state, now, time);
    }
}

/// A [`KeptShot`], its state and whether it runs, which no tick ends with.
pub(crate) struct HeldKept<S: KeptShot> {
    shot: S,
    state: S::State,
    running: bool,
}

impl<S: KeptShot> HeldKept<S> {
    pub(crate) fn new(shot: S) -> Self {
        Self {
            shot,
            state: S::State::default(),
            running: false,
        }
    }
}

impl<S: KeptShot> Behaviour for HeldKept<S> {
    fn required(&self, into: &mut BTreeSet<MemoryKind>) {
        into.extend(self.shot.required().iter().copied());
    }

    fn running(&self) -> bool {
        self.running
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        self.running = self.shot.trigger(&mut self.state, cx);
        self.running
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        self.do_stop(cx);
    }

    fn do_stop(&mut self, _: &mut Cx<'_, '_, '_>) {
        self.running = false;
    }

    fn load(&mut self, first: u16, parts: &BTreeMap<u16, Entry>) {
        self.state = read_entry(first, S::VERSION, parts);
        self.running = false;
    }

    fn save(&self, first: u16, into: &mut Vec<Entry>) {
        if self.state != S::State::default() {
            into.push(entry(first, S::VERSION, &self.state));
        }
    }

    fn left(&mut self, now: u32, time: i64) {
        self.shot.left(&mut self.state, now, time);
    }
}

/// A counter the game lowers by one on some ticks only (the plan, section 8 item 4): its
/// value, and the `lived` from which every tick has lowered it, where it is being
/// lowered. It is written only where its lowering pauses or resumes, or it is set; one
/// that is lowered tick after tick writes nothing.
///
/// A pause is seen at the first tick its owner is asked and does not lower it, and where
/// its owner's activity ends ([`Behaviour::left`]), after which the game does not ask
/// it. **Named**: a tick a gate does not ask its child is not seen, and is counted as a
/// lowering; a value that would so fall below nothing reads as nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Counter {
    pub(crate) value: i32,
    pub(crate) since: Option<u32>,
}

impl Counter {
    /// A counter at `value`, not being lowered.
    pub(crate) fn at_rest(value: i32) -> Self {
        Self { value, since: None }
    }

    /// Its value at `now`, before that tick's lowering.
    pub(crate) fn at(self, now: u32) -> i32 {
        match self.since {
            None => self.value,
            Some(since) => {
                let gone = i64::from(now) - i64::from(since);
                (i64::from(self.value) - gone.max(0)).max(0) as i32
            }
        }
    }

    /// The tick `now` lowers it by one: it is lowered from here on.
    pub(crate) fn lower(&mut self, now: u32) {
        if self.since.is_none() {
            self.since = Some(now);
        }
    }

    /// The tick `now` does not lower it: its value is kept as it is now.
    pub(crate) fn pause(&mut self, now: u32) {
        if self.since.is_some() {
            *self = Self::at_rest(self.at(now));
        }
    }

    /// Set to `value` in the tick `now`, after its lowering: lowered on from the next
    /// tick where `lowering`.
    pub(crate) fn set(&mut self, value: i32, now: u32, lowering: bool) {
        *self = Self {
            value,
            since: lowering.then_some(now + 1),
        };
    }
}

/// A speed a behaviour walks at: a constant, or a function of the mob (`e -> 1.25f`).
#[derive(Clone, Copy)]
pub(crate) enum Speed {
    Fixed(f32),
    Of(fn(&Cx<'_, '_, '_>) -> f32),
}

impl Speed {
    pub(crate) fn of(self, cx: &Cx<'_, '_, '_>) -> f32 {
        match self {
            Speed::Fixed(speed) => speed,
            Speed::Of(speed) => speed(cx),
        }
    }
}

/// `UniformInt.of(min, max).sample(random)`: `nextInt(max − min + 1) + min`, one draw.
pub(crate) fn sample(draws: &mut Random, (min, max): (i32, i32)) -> i32 {
    let bound = u32::try_from(i64::from(max) - i64::from(min) + 1)
        .unwrap_or(1)
        .max(1);
    draws.next_int(bound) as i32 + min
}

/// `EntityTracker(entity, trackEyeHeight)`: its block its feet's.
pub(crate) fn tracking(key: EntityId, eyes: bool) -> Tracker {
    Tracker::Entity {
        key,
        eyes,
        eyes_block: false,
    }
}

/// `PositionTracker.currentPosition()`: an entity's feet (or eyes), a block's centre, a
/// point; none for an entity the view has not ("unknown").
pub(crate) fn position_of(cx: &Cx<'_, '_, '_>, tracker: &Tracker) -> Option<Vec3> {
    match *tracker {
        Tracker::Entity { key, eyes, .. } => {
            let looked = find(cx.ai, key)?;
            Some(if eyes {
                [looked.position[0], looked.eye_y, looked.position[2]]
            } else {
                looked.position
            })
        }
        Tracker::Block(pos) => Some([
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ]),
        Tracker::Point(point) => Some(point),
    }
}

/// `PositionTracker.currentBlockPosition()`: an entity's block (or its eyes' block), a
/// block, a point's block; none for an entity the view has not.
pub(crate) fn block_of(cx: &Cx<'_, '_, '_>, tracker: &Tracker) -> Option<BlockPos> {
    match *tracker {
        Tracker::Entity {
            key, eyes_block, ..
        } => {
            let looked = find(cx.ai, key)?;
            Some(if eyes_block {
                physics::containing([looked.position[0], looked.eye_y, looked.position[2]])
            } else {
                looked.block_position()
            })
        }
        Tracker::Block(pos) => Some(pos),
        Tracker::Point(point) => Some(physics::containing(point)),
    }
}

/// `PositionTracker.isVisibleBy(mob)`: a block or a point always; a living thing while it
/// lives and the mob's `visible_mobs` holds it seen; another entity always. One the view
/// has not is taken as out of sight (the game's answer for one beyond its sight).
pub(crate) fn visible_by(cx: &mut Cx<'_, '_, '_>, tracker: &Tracker) -> bool {
    let Tracker::Entity { key, .. } = *tracker else {
        return true;
    };
    let Some(looked) = find(cx.ai, key) else {
        return false;
    };
    if !sensors::is_living(&looked) {
        return true;
    }
    looked.alive && sensors::visible_contains(cx, key)
}

/// A walk target as the memory holds it: where to, how fast, how close is close enough.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Walk {
    pub(crate) to: Tracker,
    pub(crate) speed: f32,
    pub(crate) close_enough: i32,
}

/// The mob's `walk_target`, if it has one.
pub(crate) fn walk_target(cx: &Cx<'_, '_, '_>) -> Option<Walk> {
    match cx.memories.get(MemoryKind::WalkTarget)? {
        Value::Walk {
            to,
            speed,
            close_enough,
        } => Some(Walk {
            to,
            speed,
            close_enough,
        }),
        _ => None,
    }
}

/// A walk target's value.
pub(crate) fn walk(to: Tracker, speed: f32, close_enough: i32) -> Value {
    Value::Walk {
        to,
        speed,
        close_enough,
    }
}

/// `new WalkTarget(Vec3, speed, closeEnough)`: to the block the point is in.
pub(crate) fn walk_to_point(point: Vec3, speed: f32, close_enough: i32) -> Value {
    walk(
        Tracker::Block(physics::containing(point)),
        speed,
        close_enough,
    )
}

/// The mob's `look_target`, if it has one.
pub(crate) fn look_target(cx: &Cx<'_, '_, '_>) -> Option<Tracker> {
    match cx.memories.get(MemoryKind::LookTarget)? {
        Value::Tracker(tracker) => Some(tracker),
        _ => None,
    }
}

/// An entity a memory names.
pub(crate) fn entity_of(cx: &Cx<'_, '_, '_>, kind: MemoryKind) -> Option<EntityId> {
    match cx.memories.get(kind)? {
        Value::Entity(key) => Some(key),
        _ => None,
    }
}

/// `Vec3.atBottomCenterOf(pos)`.
pub(crate) fn bottom_centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// `Vec3.atCenterOf(pos)`.
pub(crate) fn centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// `Vec3i.distManhattan`.
pub(crate) fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.z - b.z).abs()
}

/// `BehaviorBuilder.triggerIf(predicate, oneShot)`: the one-shot tried only while the
/// predicate holds (the camel's strolls that refuse while it sits).
pub(crate) struct TriggerIf {
    pub(crate) predicate: fn(&mut Cx<'_, '_, '_>) -> bool,
    pub(crate) inner: Box<dyn Behaviour>,
}

impl Behaviour for TriggerIf {
    fn required(&self, into: &mut BTreeSet<MemoryKind>) {
        self.inner.required(into);
    }

    fn running(&self) -> bool {
        self.inner.running()
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        (self.predicate)(cx) && self.inner.try_start(cx)
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        self.inner.tick_or_stop(cx);
    }

    fn do_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        self.inner.do_stop(cx);
    }

    fn count(&self) -> usize {
        self.inner.count()
    }

    fn load(&mut self, first: u16, parts: &BTreeMap<u16, Entry>) {
        self.inner.load(first, parts);
    }

    fn save(&self, first: u16, into: &mut Vec<Entry>) {
        self.inner.save(first, into);
    }
}

/// `TriggerIf` boxed.
pub(crate) fn trigger_if(
    predicate: fn(&mut Cx<'_, '_, '_>) -> bool,
    inner: Box<dyn Behaviour>,
) -> Box<dyn Behaviour> {
    Box::new(TriggerIf { predicate, inner })
}
