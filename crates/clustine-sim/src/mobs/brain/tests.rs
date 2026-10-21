//! Tests of the brain's machinery from the brief's rules (7.1) and the plan (sections 2.2
//! to 2.6), each number worked out from the game's own rules: memories' expiry to the
//! tick, a countdown against a literal port of the game's lowering at every reader's
//! place, the activities' buckets, the passes' order, each kind of behaviour, the gates,
//! the sensors' phases and their sensing again after a take-in, and the record.

use std::cell::RefCell;
use std::collections::BTreeSet;

use super::*;
use crate::mobs::ai::tests::{Plot, Ticked};
use crate::mobs::types::MobType;
use crate::physics::Vec3;
use clustine_world::EntityId;

/// A type with nothing of its own, for the brain alone.
struct Plain;

impl MobType for Plain {}

static PLAIN: Plain = Plain;

fn memories(registered: &[MemoryKind], now: u32) -> Memories {
    Memories {
        registered: registered.iter().copied().collect(),
        now,
        ..Memories::default()
    }
}

// ------------------------------------------------------------------------------------
// Memories (the brief's 7.1.2; the plan, section 2.2).

/// "A memory set with expiry `n` during tick T is present for the behaviours of ticks T
/// to T+n and cleared in the forget pass of T+n+1. A memory set with expiry 0 lives
/// through the rest of its tick and the next whole tick."
#[test]
fn a_memory_set_with_an_expiry_is_there_through_that_many_ticks_and_gone_after() {
    for ttl in [0_i64, 1, 5] {
        let mut memories = memories(&[MemoryKind::IsPanicking], 10);
        memories.forget(10);
        memories.set_with_expiry(MemoryKind::IsPanicking, Value::Bool(true), ttl);
        for now in 11..=(10 + ttl as u32) {
            memories.forget(now);
            assert!(
                memories.get(MemoryKind::IsPanicking).is_some(),
                "ttl {ttl}: there at {now}"
            );
            assert_eq!(
                memories.expires_in(MemoryKind::IsPanicking),
                10 + ttl - i64::from(now)
            );
        }
        memories.forget(11 + ttl as u32);
        assert!(
            memories.get(MemoryKind::IsPanicking).is_none(),
            "ttl {ttl}: gone"
        );
    }
}

/// `setMemory` without an expiry replaces the slot's; an empty collection is none; a
/// kind not registered takes nothing and checks false; an absent memory's time to
/// expiry is `Long.MAX_VALUE`.
#[test]
fn set_replaces_an_expiry_an_empty_list_erases_and_an_unregistered_kind_takes_nothing() {
    let mut memories = memories(&[MemoryKind::IsPanicking, MemoryKind::Mobs], 3);
    memories.set_with_expiry(MemoryKind::IsPanicking, Value::Unit, 2);
    memories.set(MemoryKind::IsPanicking, Value::Unit);
    assert_eq!(memories.expires_in(MemoryKind::IsPanicking), i64::MAX);
    memories.forget(100);
    assert!(memories.check(MemoryKind::IsPanicking, Status::Present));

    memories.set(MemoryKind::Mobs, Value::Entities(vec![EntityId(4)]));
    memories.set(MemoryKind::Mobs, Value::Entities(Vec::new()));
    assert!(memories.check(MemoryKind::Mobs, Status::Absent));
    assert_eq!(memories.expires_in(MemoryKind::Mobs), i64::MAX);

    memories.set(MemoryKind::BreedTarget, Value::Entity(EntityId(9)));
    assert!(memories.get(MemoryKind::BreedTarget).is_none());
    for status in [Status::Registered, Status::Present, Status::Absent] {
        assert!(!memories.check(MemoryKind::BreedTarget, status));
    }
    assert!(memories.check(MemoryKind::Mobs, Status::Registered));
}

/// A literal port of `CountDownCooldownTicks` (the brief's 7.3.12) on a plain int: a
/// cooldown of `n` set after the start pass of tick `set_at`; each tick the behaviour,
/// stopped, starts where the memory is present, and running, ticks (`- 1`) while it is
/// present and above 0, else stops and erases. Returns, for each tick, the value a
/// reader sees before the lowering's place and after it.
fn ported(n: i32, set_at: u32, until: u32) -> Vec<(u32, Option<i32>, Option<i32>)> {
    let mut value = Some(n);
    let mut running = false;
    let mut seen = Vec::new();
    for now in set_at + 1..=until {
        if !running && value.is_some() {
            running = true;
        }
        let before = value;
        if running {
            match value {
                Some(v) if v > 0 => value = Some(v - 1),
                _ => {
                    running = false;
                    value = None;
                }
            }
        }
        seen.push((now, before, value));
    }
    seen
}

/// The plan, section 2.2 item 3: a countdown kept as its value and the tick its lowering
/// began reads, at every reader's place, as the game's per-tick decrement does, and is
/// erased in the tick the game erases it; it is written only when set and when its
/// lowering begins.
#[test]
fn a_countdown_reads_at_every_place_as_the_games_tick_by_tick_lowering_does() {
    let kind = MemoryKind::TemptationCooldownTicks;
    for n in [0, 1, 3, 100] {
        let mut memories = memories(&[kind], 5);
        memories.forget(5);
        memories.set(kind, Value::Int(n));
        let mut running = false;
        let mut writes = 0;
        for (now, before, after) in ported(n, 5, 5 + n as u32 + 3) {
            memories.forget(now);
            memories.changed.clear();
            // The start pass: the lowering behaviour starts where it is present.
            if !running && memories.get(kind).is_some() {
                running = true;
                memories.begin_lowering(kind);
            }
            assert_eq!(memories.int(kind), before, "n {n}: before at {now}");
            // The running pass: it stops at nothing, else lowers.
            if running {
                if memories.int(kind).is_some_and(|value| value > 0) {
                    memories.lower(kind);
                } else {
                    running = false;
                    memories.erase(kind);
                }
            }
            assert_eq!(memories.int(kind), after, "n {n}: after at {now}");
            writes += memories.changed.len();
            memories.end_tick();
            assert_eq!(
                memories.int(kind),
                after,
                "n {n}: outside the tick at {now}"
            );
        }
        // Set, its lowering begun, its erasure: no write a tick between.
        assert!(writes <= 2, "n {n}: {writes} writes");
    }
}

/// A countdown set again while its lowering runs goes on being lowered from its new
/// value: in this tick where this tick's lowering has not come yet, from the next where
/// it has.
#[test]
fn a_countdown_set_again_while_it_is_lowered_is_lowered_from_its_new_value() {
    let kind = MemoryKind::RamCooldownTicks;
    let mut memories = memories(&[kind], 0);
    memories.forget(1);
    memories.set(kind, Value::Int(10));
    memories.begin_lowering(kind);
    memories.lower(kind);
    // Set after this tick's lowering: 7 now and in the next tick's start pass.
    memories.set(kind, Value::Int(7));
    assert_eq!(memories.int(kind), Some(7));
    memories.forget(2);
    assert_eq!(memories.int(kind), Some(7));
    memories.lower(kind);
    assert_eq!(memories.int(kind), Some(6));
    // Set before this tick's lowering: lowered in this tick.
    memories.forget(3);
    memories.set(kind, Value::Int(20));
    assert_eq!(memories.int(kind), Some(20));
    memories.lower(kind);
    assert_eq!(memories.int(kind), Some(19));
}

/// Every memory kind of the table is the registry's name, and the sensed ones are kept
/// apart from those the game saves but for the frog's, which a sensor writes and the
/// game keeps (the plan, section 2.2 item 7).
#[test]
fn the_memory_table_names_its_kinds_and_numbers_them_for_good() {
    assert_eq!(MemoryKind::ALL.len(), 115);
    for (index, kind) in MemoryKind::ALL.iter().enumerate() {
        assert_eq!(usize::from(kind.id()), index);
        assert_eq!(MemoryKind::of_id(kind.id()), Some(*kind));
    }
    assert_eq!(MemoryKind::TemptingPlayer.name(), "tempting_player");
    assert_eq!(
        MemoryKind::LongJumpCoolingDown.name(),
        "long_jump_cooling_down"
    );
    assert!(MemoryKind::TemptingPlayer.sensed() && !MemoryKind::TemptingPlayer.saved_by_game());
    assert!(MemoryKind::HurtByEntity.saved_by_game() || !MemoryKind::HurtByEntity.sensed());
    assert!(MemoryKind::PlayDeadTicks.counts_down());
}

// ------------------------------------------------------------------------------------
// Activities and the passes' order (the brief's 7.1.1 finding 6, 7.1.5).

/// The buckets of the game's activities in a `HashMap` of capacity 16, as the brief
/// lists them: `play` 0; `core`, `rest` 1; `hide`, `admire_item` 2; `dig` 3; `meet` 4;
/// `idle`, `pre_raid`, `celebrate`, `tongue` 5; `work`, `sniff` 6; `lay_spawn` 8; `emerge`
/// 10; `avoid`, `investigate`, `roar` 11; `ride` 13; `fight`, `swim` 14; `panic`, `raid`,
/// `play_dead`, `long_jump`, `ram` 15.
#[test]
fn each_activity_is_in_the_bucket_the_games_hash_map_puts_it_in() {
    use Activity::*;
    let expected = [
        (Play, 0),
        (Core, 1),
        (Rest, 1),
        (Hide, 2),
        (AdmireItem, 2),
        (Dig, 3),
        (Meet, 4),
        (Idle, 5),
        (PreRaid, 5),
        (Celebrate, 5),
        (Tongue, 5),
        (Work, 6),
        (Sniff, 6),
        (LaySpawn, 8),
        (Emerge, 10),
        (Avoid, 11),
        (Investigate, 11),
        (Roar, 11),
        (Ride, 13),
        (Fight, 14),
        (Swim, 14),
        (Panic, 15),
        (Raid, 15),
        (PlayDead, 15),
        (LongJump, 15),
        (Ram, 15),
    ];
    for (activity, bucket) in expected {
        assert_eq!(activity.bucket(), bucket, "{}", activity.name());
    }
    assert_eq!(activity::java_string_hash("idle"), 3_227_604);
}

// What the behaviours of the order's tests write: who, in which pass, in order.
thread_local! {
    static LOG: RefCell<Vec<(&'static str, &'static str)>> = const { RefCell::new(Vec::new()) };
}

fn logged(name: &'static str, what: &'static str) {
    LOG.with(|log| log.borrow_mut().push((name, what)));
}

fn take_log() -> Vec<(&'static str, &'static str)> {
    LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

/// A behaviour that logs its start, its ticks and its stop, runs `runs` ticks of its
/// duration (`min == max == runs`), and keeps going while `still`.
struct Noted {
    name: &'static str,
    runs: i64,
    still: bool,
    entry: Vec<(MemoryKind, Status)>,
}

impl Timed for Noted {
    type State = u8;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &self.entry
    }

    fn durations(&self) -> (i64, i64) {
        (self.runs, self.runs)
    }

    fn can_still_use(&self, _: &mut u8, _: &mut Cx<'_, '_, '_>) -> bool {
        self.still
    }

    fn start(&self, state: &mut u8, _: &mut Cx<'_, '_, '_>) {
        *state = state.wrapping_add(1);
        logged(self.name, "start");
    }

    fn tick(&self, _: &mut u8, _: &mut Cx<'_, '_, '_>) {
        logged(self.name, "tick");
    }

    fn stop(&self, _: &mut u8, _: &mut Cx<'_, '_, '_>) {
        logged(self.name, "stop");
    }
}

fn noted(name: &'static str, runs: i64, still: bool) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(Noted {
        name,
        runs,
        still,
        entry: Vec::new(),
    }))
}

/// One brain ticked on a plot, its record carried between ticks as a region would.
struct Run {
    plot: Plot,
    ticked: Ticked,
    provider: &'static Provider,
    entries: Vec<Entry>,
    sensed: Option<Sensed>,
}

impl Run {
    fn new(provider: &'static Provider) -> Self {
        let plot = Plot::flat();
        let ticked = Ticked::new(&plot, &PLAIN, Vec3::default());
        Self {
            plot,
            ticked,
            provider,
            entries: Vec::new(),
            sensed: Some(Sensed::new()),
        }
    }

    /// One brain tick, `f` after it, as a type's activity update; the brain as it was
    /// left, before it was written.
    fn tick_with(&mut self, f: impl FnOnce(&mut Brain)) -> Brain {
        let now = self.ticked.lived;
        let time = i64::from(now);
        let mut brain = Brain::read(self.provider, &self.entries, self.sensed.as_ref(), now);
        let (entries, sensed) = self.ticked.with_ai(&self.plot, |ai| {
            brain.tick(ai, now, time);
            f(&mut brain);
            brain.write()
        });
        self.entries = entries;
        self.sensed = Some(sensed);
        self.ticked.lived += 1;
        brain
    }

    fn tick(&mut self) -> Brain {
        self.tick_with(|_| {})
    }
}

static ORDERED: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![
            ActivityData::numbered(Activity::Idle, 0, vec![noted("idle 0", 0, false)]),
            ActivityData::numbered(
                Activity::Core,
                0,
                vec![noted("core 0", 0, false), noted("core 1", 0, false)],
            ),
            ActivityData::numbered(Activity::Play, 0, vec![noted("play 0", 0, false)]),
        ]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The start pass by priority ascending, and at one priority by the activities' buckets
/// (`play` 0, `core` 1, `idle` 5) whatever order they were added in, over the active
/// activities only (`play` is not active here); then the running pass.
#[test]
fn behaviours_start_by_priority_then_by_their_activitys_bucket_in_active_activities() {
    let mut run = Run::new(&ORDERED);
    take_log();
    run.tick();
    assert_eq!(
        take_log(),
        [
            ("core 0", "start"),
            ("idle 0", "start"),
            ("core 1", "start"),
            ("core 0", "stop"),
            ("idle 0", "stop"),
            ("core 1", "stop"),
        ]
    );
}

static TIMED: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![noted("runner", 2, true)],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `Behavior`: a behaviour that can still be used starts and ticks in its first tick,
/// runs to its end inclusive (`d + 1` ticks), is stopped in the tick after, and starts
/// again only in the tick after that; every start draws one number, also where the two
/// durations are the same.
#[test]
fn a_timed_behaviour_runs_to_its_end_inclusive_stops_after_and_draws_at_each_start() {
    let mut run = Run::new(&TIMED);
    take_log();
    for _ in 0..5 {
        run.tick();
    }
    assert_eq!(
        take_log(),
        [
            ("runner", "start"),
            ("runner", "tick"),
            ("runner", "tick"),
            ("runner", "tick"),
            ("runner", "stop"),
            ("runner", "start"),
            ("runner", "tick"),
        ]
    );
}

/// The draw of a start: `nextInt(max + 1 - min)` of the mob's stream, one each start.
#[test]
fn a_start_draws_one_number_of_the_mobs_stream() {
    let plot = Plot::flat();
    let mut ticked = Ticked::new(&plot, &PLAIN, Vec3::default());
    let mut brain = Brain::read(&TIMED, &[], Some(&Sensed::new()), 0);
    let before = ticked.stream();
    let drawn = ticked.with_ai(&plot, |ai| {
        let start = ai.tick.draws.clone();
        brain.tick(ai, 0, 0);
        ai.tick.draws.draws_since(&start)
    });
    assert_eq!(drawn, 1, "{before:?}");
}

/// A [`Shot`] that fires while `present` is.
struct Fires;

impl Shot for Fires {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::IsPanicking]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let fires = cx.memories.check(MemoryKind::IsPanicking, Status::Present);
        if fires {
            logged("shot", "fired");
        }
        fires
    }
}

static SHOTS: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(HeldShot::new(Fires))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `OneShot`: fired, it is running for the rest of the tick and stopped by the running
/// pass, so that it fires again the next tick; and a kind it names is registered.
#[test]
fn a_one_shot_fires_each_tick_its_conditions_hold_and_keeps_nothing() {
    let mut run = Run::new(&SHOTS);
    take_log();
    let brain = run.tick_with(|brain| {
        brain.memories.set(MemoryKind::IsPanicking, Value::Unit);
    });
    assert!(brain.memories.registered(MemoryKind::IsPanicking));
    assert!(
        take_log().is_empty(),
        "nothing to fire on before it was set"
    );
    run.tick();
    run.tick();
    assert_eq!(take_log(), [("shot", "fired"), ("shot", "fired")]);
    assert!(
        run.entries
            .iter()
            .all(|entry| entry.id < ids::FIRST_BEHAVIOUR),
        "a one-shot keeps nothing"
    );
}

static GATED: Provider = Provider {
    memories: &[MemoryKind::IsPanicking],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(Gate::new(
                Vec::new(),
                vec![MemoryKind::IsPanicking],
                Order::Ordered,
                Running::RunOne,
                vec![
                    (
                        Box::new(HeldTimed::new(Noted {
                            name: "refused",
                            runs: 0,
                            still: false,
                            entry: vec![(MemoryKind::BreedTarget, Status::Present)],
                        })),
                        1,
                    ),
                    (noted("first", 1, true), 1),
                    (noted("second", 1, true), 1),
                ],
            ))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `GateBehavior` ordered, running one: the first child in order that starts runs, those
/// after it are not tried, and while it runs the gate runs and is not tried again; when
/// it stops, the gate stops in that tick and erases its exit memories.
#[test]
fn an_ordered_gate_runs_its_first_child_that_starts_and_stops_with_it() {
    let mut run = Run::new(&GATED);
    take_log();
    run.tick_with(|brain| brain.memories.set(MemoryKind::IsPanicking, Value::Unit));
    run.tick();
    let brain = run.tick();
    assert_eq!(
        take_log(),
        [
            ("first", "start"),
            ("first", "tick"),
            ("first", "tick"),
            ("first", "stop"),
        ]
    );
    assert!(
        brain
            .memories
            .check(MemoryKind::IsPanicking, Status::Absent)
    );
}

static SHUFFLED: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(Gate::run_one(vec![
                (noted("a", 0, false), 1),
                (noted("b", 0, false), 3),
                (noted("c", 0, false), 2),
            ]))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `RunOne`: its children shuffled at each start by one `nextFloat` each, and the start's
/// own draw of the child that starts; across many starts each child comes first about as
/// often as its weight's share (`-pow(r, 1 / w)` keys: 1/6, 3/6 and 2/6).
#[test]
fn a_shuffled_gate_draws_one_float_a_child_and_puts_them_first_by_weight() {
    let mut run = Run::new(&SHUFFLED);
    let mut first = [0_u32; 3];
    let tries = 6000;
    for _ in 0..tries {
        take_log();
        run.tick();
        let started = take_log()
            .into_iter()
            .find(|(_, what)| *what == "start")
            .expect("one child starts")
            .0;
        first[["a", "b", "c"]
            .iter()
            .position(|name| *name == started)
            .expect("a child")] += 1;
    }
    for (count, weight) in first.into_iter().zip([1.0, 3.0, 2.0]) {
        let share = f64::from(count) / f64::from(tries);
        assert!((share - weight / 6.0).abs() < 0.03, "{first:?}");
    }
    let plot = Plot::flat();
    let mut ticked = Ticked::new(&plot, &PLAIN, Vec3::default());
    let mut brain = Brain::read(&SHUFFLED, &[], Some(&Sensed::new()), 0);
    let drawn = ticked.with_ai(&plot, |ai| {
        let start = ai.tick.draws.clone();
        brain.tick(ai, 0, 0);
        ai.tick.draws.draws_since(&start)
    });
    assert_eq!(drawn, 3 + 1, "three floats and the start's number");
}

// ------------------------------------------------------------------------------------
// Sensors (the brief's 7.1.3; the plan, sections 2.3 and 8 item 1).

/// A sensor that notes each scan and each sensing again, and senses the mob's lived.
struct Counting;

impl Sensor for Counting {
    fn rate(&self) -> u32 {
        5
    }

    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::Mobs]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        logged("sensor", "scan");
        cx.memories
            .set(MemoryKind::Mobs, Value::Entities(vec![EntityId(7)]));
    }

    fn resense(&self, cx: &mut Cx<'_, '_, '_>) {
        logged("sensor", "resense");
        cx.memories
            .set(MemoryKind::Mobs, Value::Entities(vec![EntityId(7)]));
    }
}

static SENSING: Provider = Provider {
    memories: &[],
    sensors: &[&Counting],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A brain made for a mob whose first tick is at `lived` 0 draws its sensor's delay `d`
/// from the mob's own random; the first scan is at the brain's `max(d, 1)`-th tick, `lived`
/// `max(d, 1) - 1`, and every fifth tick after; the sensed memory is
/// kept by the region and not in the record; and after a take-in (nothing sensed kept)
/// the sensor senses again before the start pass, without moving its phase.
#[test]
fn a_sensor_scans_first_at_its_drawn_delay_then_at_its_rate_and_senses_again_after_a_take_in() {
    let mut own = crate::region::Random::of_entity(7, EntityId(40), 0);
    let mut drawn = own.clone();
    let delay = drawn.next_int(5).max(1) - 1;
    let mut run = Run::new(&SENSING);
    let state = made(&SENSING, 0, &mut own);
    assert_eq!(state.next_scans, [delay]);
    run.entries = Vec::new();
    part::put(&mut run.entries, &state);
    take_log();
    let mut scans = Vec::new();
    for _ in 0..=delay + 10 {
        let now = run.ticked.lived;
        run.tick();
        if take_log().contains(&("sensor", "scan")) {
            scans.push(now);
        }
    }
    assert_eq!(scans, [delay, delay + 5, delay + 10]);
    assert!(
        run.entries
            .iter()
            .all(|entry| entry.id != ids::FIRST_MEMORY + MemoryKind::Mobs.id()),
        "sensed memories are the region's"
    );
    assert!(
        run.sensed
            .as_ref()
            .is_some_and(|sensed| sensed.contains_key(&MemoryKind::Mobs))
    );

    // Taken in: nothing sensed is kept, and the sensor senses again at once.
    run.sensed = None;
    let now = run.ticked.lived;
    let brain = run.tick();
    let log = take_log();
    assert_eq!(
        log.first(),
        Some(&("sensor", "resense")),
        "{log:?} at {now}"
    );
    assert!(brain.memories.check(MemoryKind::Mobs, Status::Present));
    assert_eq!(brain.state.next_scans, [delay + 15], "its phase as it was");
}

// ------------------------------------------------------------------------------------
// Activities and the record.

static IN_ACTIVITIES: Provider = Provider {
    memories: &[MemoryKind::IsPanicking, MemoryKind::BreedTarget],
    sensors: &[],
    activities: || {
        vec![
            ActivityData::numbered(Activity::Core, 0, Vec::new()),
            ActivityData::numbered(Activity::Idle, 0, Vec::new()),
            ActivityData::while_present(Activity::Panic, 0, Vec::new(), MemoryKind::IsPanicking),
        ]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `setActiveActivityToFirstValid`: the first whose requirements are met; none changes
/// nothing. `setActiveActivity` erases what the activity it ends erases when stopped.
#[test]
fn the_first_valid_activity_is_set_and_ending_one_erases_its_memories() {
    let mut run = Run::new(&IN_ACTIVITIES);
    let brain = run.tick();
    assert_eq!(brain.active_non_core(), Some(Activity::Idle));
    let brain = run.tick_with(|brain| {
        brain.set_active_to_first_valid(&[Activity::Panic]);
        assert_eq!(
            brain.active_non_core(),
            Some(Activity::Idle),
            "not valid yet"
        );
        brain.memories.set(MemoryKind::IsPanicking, Value::Unit);
        brain.set_active_to_first_valid(&[Activity::Panic, Activity::Idle]);
    });
    assert_eq!(brain.active_non_core(), Some(Activity::Panic));
    let brain = run.tick_with(|brain| brain.set_active(Activity::Idle));
    assert_eq!(brain.active_non_core(), Some(Activity::Idle));
    assert!(
        brain
            .memories
            .check(MemoryKind::IsPanicking, Status::Absent)
    );
    assert!(!brain.requirements_met(Activity::Fight), "never added");
}

/// The record: what the brain writes, read again, is the same brain; a behaviour at rest
/// writes nothing; the brain's entries are kept apart from the goals' in the mind.
#[test]
fn a_brain_written_and_read_again_is_the_same_and_one_at_rest_writes_little() {
    let mut run = Run::new(&TIMED);
    run.tick();
    let running = run.entries.clone();
    assert!(
        running.iter().any(|entry| entry.id == ids::FIRST_BEHAVIOUR),
        "a running behaviour is kept: {running:?}"
    );
    let brain = Brain::read(&TIMED, &running, run.sensed.as_ref(), run.ticked.lived);
    assert_eq!(brain.write().0, running);
    let mind = crate::mobs::ai::Mind {
        brain: running.clone(),
        ..Default::default()
    };
    let read = crate::mobs::ai::Mind::decode(&mind.encode()).expect("a mind");
    assert_eq!(read.brain, running);
    assert!(read.slots.is_empty());

    let mut resting = Run::new(&RESTING);
    resting.tick();
    let rest = resting.tick();
    let ids: BTreeSet<u16> = rest.write().0.iter().map(|entry| entry.id).collect();
    assert_eq!(
        ids,
        BTreeSet::from([ids::STATE]),
        "only the active activity"
    );
}

static RESTING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Idle,
            0,
            vec![Box::new(HeldTimed::new(DoNothing { min: 0, max: 0 }))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};
