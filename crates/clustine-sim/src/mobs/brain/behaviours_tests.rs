//! Tests of the shared behaviours from the game's rules (the brief's 7.3, the plan's
//! sections 2 and 8), each on a small brain on a field of its own: the expected numbers
//! are the game's formulas and the dumps', drawn where a draw decides them from the same
//! stream the mob's tick draws from, at the place the game draws.
//!
//! The field ([`Field`]) and the ticked mob ([`Run`]) serve the sensors' tests too.

use std::cell::RefCell;
use std::collections::BTreeMap;

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, Entry, ItemStack};

use super::behaviours::cooldowns::count_down;
use super::behaviours::jumping::{self, PossibleJump};
use super::behaviours::liquids::{Wait, find_liquid};
use super::behaviours::looking::{LookAt, look_around, look_sometimes};
use super::behaviours::panic::{AnimalPanic, swim};
use super::behaviours::ramming::{PrepareRamNearestTarget, edge_of_block};
use super::behaviours::social::{AnimalMakeLove, FollowTemptation};
use super::behaviours::walking::{MoveToTargetSink, SinkState, look_sink};
use super::behaviours::{Counter, Speed, walk};
use super::sensors::NEAREST_LIVING_ENTITIES;
use super::*;
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::{MobType, animal};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;

/// The key of the mob the tests tick.
pub(super) const KEY: EntityId = EntityId(40);
/// The world's seed of its stream.
pub(super) const SEED: u64 = 7;
pub(super) const COW: u16 = entity_types::COW as u16;

/// An animal of no type of its own but its age: what the shared behaviours ask of a mob.
pub(super) struct Beast;

impl MobType for Beast {
    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }
}

pub(super) static BEAST: Beast = Beast;

/// Land of stone below y = 0 and air above, with what a test puts on it.
#[derive(Default)]
pub(super) struct Field {
    pub(super) blocks: BTreeMap<(i32, i32, i32), BlockState>,
    pub(super) players: Vec<SeenPlayer>,
    /// The entities, their boxes, and whether each is in a copy.
    pub(super) entities: Vec<(Entity, Aabb, bool)>,
}

impl Field {
    pub(super) fn set(&mut self, pos: BlockPos, state: BlockState) {
        self.blocks.insert((pos.x, pos.y, pos.z), state);
    }

    /// `entity` laid on the field (where it is already, replaced).
    pub(super) fn put(&mut self, entity: Entity, copied: bool) {
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        self.entities
            .retain(|(other, _, _)| other.key != entity.key);
        self.entities.push((entity, bounds, copied));
    }

    pub(super) fn remove(&mut self, key: EntityId) {
        self.entities.retain(|(entity, _, _)| entity.key != key);
    }
}

impl World for Field {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if pos.y < -64 || pos.y >= 320 {
            return None;
        }
        Some(match self.blocks.get(&(pos.x, pos.y, pos.z)) {
            Some(state) => *state,
            None if pos.y < 0 => blocks::STONE,
            None => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Field {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Field {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, bounds: &Aabb) -> Vec<Seen<'_>> {
        self.entities
            .iter()
            .filter(|(_, own, _)| own.intersects(*bounds))
            .map(|(entity, own, copied)| Seen {
                entity,
                bounds: *own,
                copied: *copied,
            })
            .collect()
    }

    fn entity(&self, key: EntityId) -> Option<Seen<'_>> {
        self.entities
            .iter()
            .find(|(entity, _, _)| entity.key == key)
            .map(|(entity, own, copied)| Seen {
                entity,
                bounds: *own,
                copied: *copied,
            })
    }

    fn players(&self) -> Vec<SeenPlayer> {
        self.players.clone()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }
}

/// A creative player standing at `position`, holding `held` in the main hand.
pub(super) fn player(key: u64, position: Vec3, held: Option<i32>) -> SeenPlayer {
    SeenPlayer {
        key: EntityId(key),
        uuid: u128::from(key),
        position,
        eye_height: 1.62,
        yaw: 0.0,
        pitch: 0.0,
        main_hand: held.map(|item| ItemStack::new(item, 1)),
        off_hand: None,
        sneaking: false,
        creative: true,
        spectator: false,
        vehicle: None,
        moved_horizontally: false,
        sprinting: false,
        alive: true,
        sleeping: None,
        last_hurt_mob: None,
        last_hurt_by: None,
    }
}

/// A cow of full health.
pub(super) fn healthy() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = MAX_AIR;
    mob
}

/// The record of a mob of `kind` at `position`.
pub(super) fn record(key: EntityId, kind: u16, position: Vec3, mob: &Mob) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

/// A cow's body at `position`, on the ground, at rest, with its fluids as the field has
/// them.
fn body_at(field: &Field, position: Vec3) -> Living {
    let info = EntityTypeInfo::of(entity_types::COW);
    let body = Body {
        position,
        motion: [0.0; 3],
        on_ground: true,
        fall_distance: 0.0,
        width: info.width,
        height: info.height,
        old_position: position,
        horizontal_collision: false,
        vertical_collision: false,
        vertical_collision_below: false,
        supporting: None,
        on_ground_no_blocks: false,
        stuck_speed: [0.0; 3],
        movements: Vec::new(),
        in_water: false,
        sync_position: false,
        falling_block: false,
        item: false,
        fell_on: None,
        noclip: None,
        mover: Mover {
            key: KEY,
            root: KEY,
            like_a_vehicle: false,
            happy_ghast: false,
        },
        steps: None,
    };
    let mut living = Living::new(
        body,
        Kind::Mob {
            kind: COW,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living.fluids = crate::physics::fluids::update(field, &living.body.bounds());
    living.body.in_water = living.fluids.in_water();
    living
}

/// One brain ticked on a field, its record carried between ticks as a region would:
/// the game time is its `lived`.
pub(super) struct Run {
    pub(super) field: Field,
    pub(super) provider: &'static Provider,
    pub(super) entries: Vec<Entry>,
    pub(super) sensed: Option<Sensed>,
    pub(super) mob: Mob,
    pub(super) living: Living,
    pub(super) yaw: f32,
    pub(super) lived: u32,
    /// Whether its own record is laid on the field before each tick (what reads it:
    /// a breeding's `canMate`).
    pub(super) laid: bool,
}

/// What a tick left: the brain as it was before it was written, what the mob put out,
/// how many draws it made, and the brain's parts that changed.
pub(super) struct Ticked {
    pub(super) brain: Brain,
    pub(super) out: MobOutput,
    pub(super) draws: u64,
    pub(super) changed: Vec<u16>,
}

impl Run {
    pub(super) fn new(provider: &'static Provider, at: Vec3) -> Self {
        Self::on(Field::default(), provider, at)
    }

    pub(super) fn on(field: Field, provider: &'static Provider, at: Vec3) -> Self {
        let living = body_at(&field, at);
        Self {
            field,
            provider,
            entries: Vec::new(),
            sensed: Some(Sensed::new()),
            mob: healthy(),
            living,
            yaw: 0.0,
            lived: 0,
            laid: false,
        }
    }

    /// The mob's stream in its next tick.
    pub(super) fn stream(&self) -> Random {
        Random::of_entity(SEED, KEY, self.lived)
    }

    /// The mob's feet moved to `at`, its fluids with them.
    pub(super) fn move_to(&mut self, at: Vec3) {
        self.living.body.position = at;
        self.living.fluids =
            crate::physics::fluids::update(&self.field, &self.living.body.bounds());
        self.living.body.in_water = self.living.fluids.in_water();
    }

    /// One brain tick, `before` on its memories ahead of it (as a hurt or a use between
    /// ticks), `after` after it (as a type's activity update).
    pub(super) fn tick_around(
        &mut self,
        before: impl FnOnce(&mut Brain),
        after: impl FnOnce(&mut Brain, &mut Ai<'_, '_>),
    ) -> Ticked {
        if self.laid {
            let mut own = record(KEY, COW, self.living.body.position, &self.mob);
            own.lived = self.lived;
            self.field.put(own, false);
        }
        let now = self.lived;
        let time = i64::from(now);
        let mut brain = Brain::read(self.provider, &self.entries, self.sensed.as_ref(), now);
        before(&mut brain);
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: COW,
            lived: now,
            tick_count: now,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: self.mob.clone(),
            living: self.living.clone(),
            yaw: self.yaw,
            pitch: 0.0,
            mob_type: &BEAST,
            world: &self.field,
            draws: &mut draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let mut setup = Setup::default();
        BEAST.register_goals(&mut setup);
        let config = setup.config;
        let (entries, sensed) = {
            let mut ai = Ai {
                tick: &mut tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: true,
            };
            brain.tick(&mut ai, now, time);
            after(&mut brain, &mut ai);
            brain.write()
        };
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        let out = tick.out;
        let changed = changed_parts(&self.entries, &entries);
        self.entries = entries;
        self.sensed = Some(sensed);
        self.lived += 1;
        Ticked {
            brain,
            out,
            draws: draws.draws_since(&start),
            changed,
        }
    }

    pub(super) fn tick(&mut self) -> Ticked {
        self.tick_around(|_| {}, |_, _| {})
    }

    /// A tick with `f` on its memories after it.
    pub(super) fn tick_then(&mut self, f: impl FnOnce(&mut Memories)) -> Ticked {
        self.tick_around(|_| {}, |brain, _| f(&mut brain.memories))
    }
}

/// The ids of the parts that differ between two writes of a brain.
fn changed_parts(before: &[Entry], after: &[Entry]) -> Vec<u16> {
    let mut ids: Vec<u16> = before.iter().chain(after).map(|entry| entry.id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter(|id| {
            before.iter().find(|entry| entry.id == *id)
                != after.iter().find(|entry| entry.id == *id)
        })
        .collect()
}

/// The part of the behaviour numbered `n` in the provider.
fn behaviour_part(n: u16) -> u16 {
    ids::FIRST_BEHAVIOUR + n
}

/// The part of the memory `kind`.
fn memory_part(kind: MemoryKind) -> u16 {
    ids::FIRST_MEMORY + kind.id()
}

thread_local! {
    static SEEN: RefCell<Vec<(&'static str, Option<i32>)>> = const { RefCell::new(Vec::new()) };
}

fn take_seen() -> Vec<(&'static str, Option<i32>)> {
    SEEN.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
}

/// A behaviour that runs for good and notes, at its place in each running pass, the
/// value of an int memory.
struct Reader {
    name: &'static str,
    kind: MemoryKind,
}

impl Timed for Reader {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[]
    }

    fn timed_out(&self, _: &(), _: &Cx<'_, '_, '_>, _: i64) -> bool {
        false
    }

    fn can_still_use(&self, _: &mut (), _: &mut Cx<'_, '_, '_>) -> bool {
        true
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        let value = cx.memories.int(self.kind);
        SEEN.with(|seen| seen.borrow_mut().push((self.name, value)));
    }
}

fn reader(name: &'static str, kind: MemoryKind) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(Reader { name, kind }))
}

// ------------------------------------------------------------------------------------
// CountDownCooldownTicks (7.3.12; the plan, section 2.2 item 3).

const COOLDOWN: MemoryKind = MemoryKind::TemptationCooldownTicks;

static COUNTING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![
                reader("before", COOLDOWN),
                count_down(COOLDOWN),
                reader("after", COOLDOWN),
            ],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A literal port of the game's lowering: a cooldown of `n` set after the start pass of
/// tick `set_at`; each tick the stopped behaviour starts where the memory is there, and
/// the running one ticks (`- 1`) while it is above nothing, else stops and erases. The
/// value read before its place and after it, tick by tick.
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
                Some(left) if left > 0 => value = Some(left - 1),
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

/// A cooldown set after a tick's start pass is lowered from the next tick, at the
/// behaviour's place in the running pass: a reader before it sees one more than a
/// reader after it, every tick, as the game's tick-by-tick lowering does, until it is
/// erased in the tick after it reached nothing; and nothing of it is written between
/// its lowering's beginning and its erasure.
#[test]
fn count_down_cooldown_ticks_reads_as_the_games_lowering_at_every_place() {
    for n in [0, 1, 4] {
        let mut run = Run::new(&COUNTING, [0.5, 0.0, 0.5]);
        run.tick_then(|memories| memories.set(COOLDOWN, Value::Int(n)));
        take_seen();
        let mut writes = 0;
        for (now, before, after) in ported(n, 0, n as u32 + 3) {
            let ticked = run.tick();
            assert_eq!(
                take_seen(),
                [("before", before), ("after", after)],
                "n {n} at {now}"
            );
            if ticked.changed.contains(&memory_part(COOLDOWN)) {
                writes += 1;
            }
        }
        // Its lowering begun, then its erasure.
        assert!(writes <= 2, "n {n}: {writes} writes of the cooldown");
    }
}

// ------------------------------------------------------------------------------------
// A behaviour outliving its activity (the brief's 7.1.1 finding 7).

static LOOKING: Provider = Provider {
    memories: &[MemoryKind::IsPanicking],
    sensors: &[],
    activities: || {
        vec![
            ActivityData::numbered(Activity::Idle, 0, vec![look_sink(45, 90)]),
            ActivityData::while_present(Activity::Panic, 0, Vec::new(), MemoryKind::IsPanicking),
        ]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `LookAtTargetSink` started in IDLE runs on after PANIC takes over, to its drawn end
/// (45 + `nextInt(46)` ticks after its start, inclusive), and then forgets its target.
#[test]
fn a_look_outlives_the_activity_it_started_in() {
    let mut run = Run::new(&LOOKING, [0.5, 0.0, 0.5]);
    let block = BlockPos::new(3, 1, 0);
    run.tick_then(|memories| {
        memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(Tracker::Block(block)),
        )
    });
    // Its start is the only draw of its tick.
    let drawn = i64::from(run.stream().next_int(46));
    let started = i64::from(run.lived);
    run.tick_around(
        |_| {},
        |brain, _| {
            brain.memories.set(MemoryKind::IsPanicking, Value::Unit);
            brain.set_active(Activity::Panic);
        },
    );
    let end = started + 45 + drawn;
    while i64::from(run.lived) <= end {
        let ticked = run.tick();
        assert_eq!(ticked.brain.active_non_core(), Some(Activity::Panic));
        assert!(
            ticked
                .brain
                .memories
                .check(MemoryKind::LookTarget, Status::Present),
            "looking at {}",
            run.lived - 1
        );
    }
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::LookTarget, Status::Absent)
    );
}

// ------------------------------------------------------------------------------------
// MoveToTargetSink (7.3.5).

static SINKING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![super::behaviours::walking::sink()],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

fn walk_to(pos: BlockPos, close_enough: i32) -> Value {
    walk(Tracker::Block(pos), 1.0, close_enough)
}

/// A walk target within its distance is reached: erased, with
/// `cant_reach_walk_target_since`, and nothing started.
#[test]
fn the_sink_erases_a_walk_target_already_reached() {
    let mut run = Run::new(&SINKING, [0.5, 0.0, 0.5]);
    run.tick_then(|memories| {
        memories.set(MemoryKind::WalkTarget, walk_to(BlockPos::new(2, 0, 0), 2));
        memories.set(MemoryKind::CantReachWalkTargetSince, Value::Long(-5));
    });
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::WalkTarget, Status::Absent)
    );
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::CantReachWalkTargetSince, Status::Absent)
    );
    assert_eq!(ticked.draws, 0, "no start, no draw");
}

/// A walk target walled in where no path reaches it: `cant_reach_walk_target_since` is
/// the game time of the first failure and stays at it through later failures; a path
/// that reaches erases it.
#[test]
fn cant_reach_walk_target_since_holds_the_first_failure() {
    let mut field = Field::default();
    // A cell at (5, 0, 0) walled in on every side and roofed.
    for (x, y, z) in [
        (4, 0, 0),
        (6, 0, 0),
        (5, 0, -1),
        (5, 0, 1),
        (4, 1, 0),
        (6, 1, 0),
        (5, 1, -1),
        (5, 1, 1),
        (5, 1, 0),
    ] {
        field.set(BlockPos::new(x, y, z), blocks::STONE);
    }
    let mut run = Run::on(field, &SINKING, [0.5, 0.0, 0.5]);
    let target = BlockPos::new(5, 0, 0);
    run.tick_then(|memories| memories.set(MemoryKind::WalkTarget, walk_to(target, 0)));
    let first = i64::from(run.lived);
    let ticked = run.tick();
    assert_eq!(
        ticked
            .brain
            .memories
            .get(MemoryKind::CantReachWalkTargetSince),
        Some(Value::Long(first))
    );
    // It runs along the path that gets nearest; set again later, the moment stays.
    for _ in 0..3 {
        let ticked = run.tick_then(|memories| {
            memories.erase(MemoryKind::Path);
        });
        assert_eq!(
            ticked
                .brain
                .memories
                .get(MemoryKind::CantReachWalkTargetSince),
            Some(Value::Long(first))
        );
    }
}

/// The retry cooldown is lowered only by the start attempts a walk target lets it make:
/// with 3 left, the fourth attempt starts; a tick with no walk target keeps it as it
/// was; and it is written only as its lowering begins, pauses and ends, never each
/// tick.
#[test]
fn the_sinks_cooldown_counts_only_its_attempts_and_writes_only_when_it_pauses_or_resumes() {
    let mut run = Run::new(&SINKING, [0.5, 0.0, 0.5]);
    let state = SinkState {
        cooldown: Counter::at_rest(3),
        ..SinkState::default()
    };
    run.entries
        .push(super::behaviours::entry(behaviour_part(0), 1, &state));
    let target = BlockPos::new(6, 0, 0);
    let sink = behaviour_part(0);
    // Attempt one (3 left, now 2).
    let ticked = run.tick_around(
        |brain| {
            brain
                .memories
                .set(MemoryKind::WalkTarget, walk_to(target, 0))
        },
        |_, _| {},
    );
    assert!(ticked.changed.contains(&sink), "its lowering resumes");
    // Attempt two (2, now 1): nothing written.
    let ticked = run.tick();
    assert!(!ticked.changed.contains(&sink));
    // No walk target: paused at 1.
    let ticked = run.tick_around(
        |brain| brain.memories.erase(MemoryKind::WalkTarget),
        |_, _| {},
    );
    assert!(ticked.changed.contains(&sink), "paused");
    let ticked = run.tick();
    assert!(!ticked.changed.contains(&sink));
    // Attempt three (1, now 0), then the fourth starts.
    let ticked = run.tick_around(
        |brain| {
            brain
                .memories
                .set(MemoryKind::WalkTarget, walk_to(target, 0))
        },
        |_, _| {},
    );
    assert!(ticked.changed.contains(&sink));
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::Path, Status::Absent),
        "not yet"
    );
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::Path, Status::Present),
        "started"
    );
    assert!(run.mob.body.path.followed.is_some());
}

/// The counter itself: its value at a moment, lowered from where it resumed, kept where
/// it pauses, never below nothing.
#[test]
fn a_counter_lowered_on_some_ticks_reads_its_value_at_each() {
    let mut counter = Counter::at_rest(5);
    counter.lower(10);
    assert_eq!(counter.at(10), 5);
    assert_eq!(counter.at(12), 3);
    counter.pause(12);
    assert_eq!(counter, Counter::at_rest(3));
    assert_eq!(counter.at(40), 3);
    counter.lower(40);
    assert_eq!(counter.at(50), 0, "never below nothing");
    counter.set(7, 60, true);
    assert_eq!((counter.at(60), counter.at(61), counter.at(63)), (7, 7, 5));
}

// ------------------------------------------------------------------------------------
// AnimalPanic (7.3.13).

static PANICKING: Provider = Provider {
    memories: &[MemoryKind::WalkTarget],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![AnimalPanic::new(2.0).boxed()],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A hurt of `#panic_causes` panics the mob for 100 + `nextInt(21)` ticks, setting
/// `is_panicking`; it erases `is_panicking` as it stops and not `hurt_by`, so that it
/// starts again in the next tick.
#[test]
fn a_panic_runs_its_drawn_time_and_starts_again_while_the_hurt_is_remembered() {
    let mut run = Run::new(&PANICKING, [0.5, 0.0, 0.5]);
    let attack = crate::mobs::body::hurt::DamageType::PLAYER_ATTACK
        .id()
        .expect("a damage type of the registry");
    let hurt = |brain: &mut Brain| brain.memories.set(MemoryKind::HurtBy, Value::Int(attack));
    // The start's draw is the first of its tick.
    let drawn = i64::from(run.stream().next_int(21));
    let started = i64::from(run.lived);
    let ticked = run.tick_around(hurt, |_, _| {});
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::IsPanicking, Status::Present)
    );
    let end = started + 100 + drawn;
    while i64::from(run.lived) <= end {
        let ticked = run.tick_around(hurt, |_, _| {});
        assert!(
            ticked
                .brain
                .memories
                .check(MemoryKind::IsPanicking, Status::Present)
        );
    }
    let ticked = run.tick_around(hurt, |_, _| {});
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::IsPanicking, Status::Absent),
        "stopped at {}",
        end + 1
    );
    let ticked = run.tick_around(hurt, |_, _| {});
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::IsPanicking, Status::Present),
        "started again"
    );
}

/// A hurt not of the causes (a fall) does not panic it; `is_panicking` set alone does.
#[test]
fn a_panic_needs_a_panicking_hurt_or_the_flag() {
    let mut run = Run::new(&PANICKING, [0.5, 0.0, 0.5]);
    let fall = crate::mobs::body::hurt::DamageType::FALL
        .id()
        .expect("fall");
    let ticked = run.tick_around(
        |brain| brain.memories.set(MemoryKind::HurtBy, Value::Int(fall)),
        |_, _| {},
    );
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::IsPanicking, Status::Absent)
    );
    let ticked = run.tick_around(
        |brain| {
            brain
                .memories
                .set(MemoryKind::IsPanicking, Value::Bool(true))
        },
        |_, _| {},
    );
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::IsPanicking, Status::Present)
    );
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::WalkTarget, Status::Present)
    );
}

// ------------------------------------------------------------------------------------
// TryFindLiquid (7.3.15).

static FINDING_WATER: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![find_liquid(8, 1.0, "minecraft:water")],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The search takes the nearest water by Manhattan distance with air above it, and in
/// one distance the first of the order (**Inferred**: x from low to high, then y, then
/// z after its mirror): of water at (3, -1, 0) and (-3, -1, 0), both 4 away, the one of
/// the lower x.
#[test]
fn the_liquid_search_goes_to_the_nearest_water_in_the_manhattan_order() {
    let mut field = Field::default();
    field.set(BlockPos::new(3, -1, 0), blocks::WATER);
    field.set(BlockPos::new(-3, -1, 0), blocks::WATER);
    let mut run = Run::on(field, &FINDING_WATER, [0.5, 0.0, 0.5]);
    let ticked = run.tick();
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::LookTarget),
        Some(Value::Tracker(Tracker::Block(BlockPos::new(-3, -1, 0))))
    );
    assert_eq!(ticked.draws, 0, "the search draws nothing");
}

/// After a search the next waits 40 ticks; each call within the wait sets it again
/// from that tick (22 on), so that called every tick it waits on; a tick that does not
/// reach the wait (a walk target) leaves it as the last call set it, and it is written
/// only then.
#[test]
fn the_liquid_search_waits_and_its_wait_is_written_only_as_it_rests() {
    let mut run = Run::new(&FINDING_WATER, [0.5, 0.0, 0.5]);
    let part = behaviour_part(0);
    // Tick 0: a search (nothing found), the wait to 40.
    let ticked = run.tick();
    assert!(ticked.changed.contains(&part));
    assert_eq!(
        read_wait(&run),
        Wait {
            next: 40,
            again_since: None
        }
    );
    // Tick 1: within the wait, set again from now on.
    run.tick();
    assert_eq!(read_wait(&run).again_since, Some(1));
    for _ in 2..30 {
        let ticked = run.tick();
        assert!(
            !ticked.changed.contains(&part),
            "no write at {}",
            run.lived - 1
        );
    }
    // Tick 30 does not reach it: it rests at 29 + 22.
    run.tick_around(
        |brain| {
            brain
                .memories
                .set(MemoryKind::WalkTarget, walk_to(BlockPos::new(9, 0, 9), 0))
        },
        |brain, _| brain.memories.erase(MemoryKind::WalkTarget),
    );
    assert_eq!(
        read_wait(&run),
        Wait {
            next: 51,
            again_since: None
        }
    );
}

fn read_wait(run: &Run) -> Wait {
    let parts: BTreeMap<u16, Entry> = run
        .entries
        .iter()
        .map(|entry| (entry.id, entry.clone()))
        .collect();
    super::behaviours::read_entry(behaviour_part(0), 1, &parts)
}

// ------------------------------------------------------------------------------------
// The long jump (7.3.16).

/// The candidates are every block of the box about the start block but its own, in
/// `betweenClosed`'s order (x, then y, then z), each weighted by the ceiling of its
/// distance squared.
#[test]
fn the_long_jumps_candidates_derive_from_the_start_block() {
    let origin = BlockPos::new(10, 64, -3);
    let list = jumping::candidates(origin, 1, 1);
    assert_eq!(list.len(), 26);
    assert_eq!(
        list[0],
        PossibleJump {
            target: BlockPos::new(9, 63, -4),
            weight: 3
        }
    );
    assert_eq!(list[1].target, BlockPos::new(10, 63, -4));
    assert_eq!(list[3].target, BlockPos::new(9, 64, -4));
    assert_eq!(
        list[12],
        PossibleJump {
            target: BlockPos::new(9, 64, -3),
            weight: 1
        }
    );
    assert_eq!(
        list[13].target,
        BlockPos::new(11, 64, -3),
        "the start left out"
    );
    let goat = jumping::candidates(origin, 5, 5);
    assert_eq!(goat.len(), 11 * 11 * 11 - 1);
    assert_eq!(goat[0].weight, 75);
}

/// `WeightedRandom.getRandomItem`: one `nextInt(total)`, walked down the list; the
/// chosen one removed.
#[test]
fn a_weighted_pick_draws_once_and_walks_the_weights() {
    let mut list = vec![
        PossibleJump {
            target: BlockPos::new(0, 0, 0),
            weight: 2,
        },
        PossibleJump {
            target: BlockPos::new(1, 0, 0),
            weight: 5,
        },
        PossibleJump {
            target: BlockPos::new(2, 0, 0),
            weight: 3,
        },
    ];
    let mut draws = Random::of_entity(SEED, KEY, 3);
    let mut copy = draws.clone();
    let i = copy.next_int(10) as i32;
    let expected = if i < 2 {
        0
    } else if i < 7 {
        1
    } else {
        2
    };
    let picked = jumping::take_weighted(&mut draws, &mut list).expect("a weight");
    assert_eq!(picked.target.x, expected);
    assert_eq!(list.len(), 2);
    assert_eq!(draws.draws_since(&Random::of_entity(SEED, KEY, 3)), 1);
}

/// `Collections.shuffle` of four: `nextInt(4)`, `nextInt(3)`, `nextInt(2)`, each a swap
/// of the last unshuffled place with the drawn one.
#[test]
fn the_jdk_shuffle_swaps_from_the_end() {
    for lived in 0..20 {
        let mut draws = Random::of_entity(SEED, KEY, lived);
        let mut copy = draws.clone();
        let mut expected = [65, 70, 75, 80];
        let (a, b, c) = (copy.next_int(4), copy.next_int(3), copy.next_int(2));
        expected.swap(3, a as usize);
        expected.swap(2, b as usize);
        expected.swap(1, c as usize);
        let mut angles = [65, 70, 75, 80];
        jumping::jdk_shuffle(&mut draws, &mut angles);
        assert_eq!(angles, expected);
        assert_eq!(draws.draws_since(&Random::of_entity(SEED, KEY, lived)), 3);
    }
}

struct Hopper;

impl jumping::Leaper for Hopper {
    fn jump_sound(&self, _: &Mob) -> i32 {
        1
    }
}

static HOPPER: Hopper = Hopper;

static JUMPING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![
                jumping::LongJumpToRandomPos {
                    interval: (600, 1200),
                    max_height: 2,
                    max_width: 2,
                    max_velocity_multiplier: 3.5,
                    leaper: &HOPPER,
                    preferred: None,
                }
                .boxed(),
            ],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// On open flat ground every candidate is reached on foot, so none is chosen: the jump
/// runs its first tick, stops in the next and leaves half a drawn interval as its
/// cooldown, its look target forgotten; at rest it keeps nothing.
#[test]
fn a_long_jump_with_every_landing_in_walking_reach_cools_down_by_half() {
    let mut run = Run::new(&JUMPING, [0.5, 0.0, 0.5]);
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::LongJumpCoolingDown, Status::Absent)
    );
    let ticked = run.tick();
    let cooldown = ticked
        .brain
        .memories
        .int(MemoryKind::LongJumpCoolingDown)
        .expect("a cooldown");
    assert!((300..=600).contains(&cooldown), "{cooldown}");
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::LookTarget, Status::Absent)
    );
    assert!(
        !run.entries
            .iter()
            .any(|entry| entry.id == behaviour_part(0))
    );
}

/// Off the ground it does not start, and leaves half a drawn interval as its cooldown.
#[test]
fn a_long_jump_off_the_ground_cools_down_by_half_without_starting() {
    let mut run = Run::new(&JUMPING, [0.5, 0.0, 0.5]);
    run.living.body.on_ground = false;
    let mut copy = run.stream();
    let expected = (copy.next_int(601) as i32 + 600) / 2;
    let ticked = run.tick();
    assert_eq!(
        ticked.brain.memories.int(MemoryKind::LongJumpCoolingDown),
        Some(expected)
    );
    assert_eq!(ticked.draws, 1);
}

/// A jump's arc to a block 3 east on the level: its speed within the most the mob may
/// give, its way east, and up.
#[test]
fn a_jump_vector_points_at_its_landing() {
    let mut run = Run::new(&JUMPING, [0.5, 0.0, 0.5]);
    run.tick_around(
        |_| {},
        |brain, ai| {
            let cx = Cx {
                ai,
                memories: &mut brain.memories,
                time: 0,
            };
            let vector = jumping::jump_vector_for_angle(&cx, &HOPPER, [3.5, 0.5, 0.5], 2.0, 65)
                .expect("an arc");
            assert!(vector[0] > 0.0 && vector[1] > 0.0 && vector[2].abs() < 1e-9);
            assert!(crate::physics::length_sqr(vector).sqrt() <= 2.0);
        },
    );
}

// ------------------------------------------------------------------------------------
// The ram's start (7.3.18).

static RAMMING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

fn preparation() -> PrepareRamNearestTarget {
    PrepareRamNearestTarget {
        cooldown_on_fail: |_| 600,
        min_distance: 4,
        max_distance: 7,
        walk_speed: 1.25,
        targets: |_| true,
        prepare_time: 20,
        sound: |_| 1,
    }
}

/// From a target at (0, 0, 10), the starts are 7 blocks out north, east, south and west;
/// the nearest the mob (at the origin) that a path reaches is north's, (0, 0, 3). A wall
/// at (0, 0, 6) stops north's at (0, 0, 7), 3 from the target, too near: of east's and
/// west's, as near the mob, east's comes first in the plane's order.
#[test]
fn the_rams_start_is_the_nearest_reachable_of_the_four_directions() {
    let mut run = Run::new(&RAMMING, [0.5, 0.0, 0.5]);
    let target = BlockPos::new(0, 0, 10);
    run.tick_around(
        |_| {},
        |brain, ai| {
            let mut cx = Cx {
                ai,
                memories: &mut brain.memories,
                time: 0,
            };
            assert_eq!(
                preparation().start_position(&mut cx, target),
                Some(BlockPos::new(0, 0, 3))
            );
        },
    );
    run.field.set(BlockPos::new(0, 0, 6), blocks::STONE);
    run.field.set(BlockPos::new(0, 1, 6), blocks::STONE);
    run.tick_around(
        |_| {},
        |brain, ai| {
            let mut cx = Cx {
                ai,
                memories: &mut brain.memories,
                time: 0,
            };
            assert_eq!(
                preparation().start_position(&mut cx, target),
                Some(BlockPos::new(7, 0, 10))
            );
        },
    );
}

/// The ram's target is the edge of the target's block towards the start.
#[test]
fn the_ram_aims_at_the_edge_of_the_targets_block() {
    assert_eq!(
        edge_of_block(BlockPos::new(0, 0, 3), BlockPos::new(0, 0, 10)),
        [0.5, 0.0, 10.0]
    );
    assert_eq!(
        edge_of_block(BlockPos::new(7, 0, 10), BlockPos::new(0, 0, 10)),
        [1.0, 0.0, 10.5]
    );
}

// ------------------------------------------------------------------------------------
// Looking (7.3.7, 7.3.22).

static GLANCING: Provider = Provider {
    memories: &[],
    sensors: &[&NEAREST_LIVING_ENTITIES],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![look_sometimes(LookAt::Player, 6.0, (2, 4))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The ticker: at nothing it draws `interval − 1` and does not fire; then one off each
/// tick a player is in sight; at nothing again it fires and sets the look. Its state is
/// written as it is drawn and as it fires, not on the ticks between.
#[test]
fn the_look_tickers_count_and_its_writes() {
    let mut field = Field::default();
    field.players.push(player(77, [3.5, 0.0, 0.5], None));
    let mut run = Run::on(field, &GLANCING, [0.5, 0.0, 0.5]);
    let drawn = run.stream().next_int(3) as i32 + 2;
    let ticked = run.tick();
    assert!(ticked.changed.contains(&behaviour_part(0)), "drawn");
    assert_eq!(ticked.draws, 1);
    let mut fired = None;
    for _ in 0..10 {
        let at = run.lived;
        let ticked = run.tick();
        if ticked
            .brain
            .memories
            .check(MemoryKind::LookTarget, Status::Present)
        {
            fired = Some(at);
            assert!(
                ticked.changed.contains(&behaviour_part(0)),
                "at rest at nothing"
            );
            break;
        }
        assert!(
            !ticked.changed.contains(&behaviour_part(0)),
            "counting at {at}"
        );
    }
    assert_eq!(fired, Some(drawn as u32 - 1), "drawn {drawn}");
}

static LOOKING_AROUND: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![look_around((150, 250), 30.0, 0.0, 0.0)],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `RandomLookAround`: its start's draw, then the pitch's and yaw's floats and the
/// cooldown's sample, all of the mob's stream; a look at the point a block from its eyes,
/// and the cooldown drawn.
#[test]
fn a_random_look_draws_four_and_looks_a_block_from_the_eyes() {
    let mut run = Run::new(&LOOKING_AROUND, [0.5, 0.0, 0.5]);
    let mut copy = run.stream();
    copy.next_int(1);
    copy.next_float();
    let yaw_draw = copy.next_float();
    let cooldown = copy.next_int(101) as i32 + 150;
    let ticked = run.tick();
    assert_eq!(ticked.draws, 4);
    assert_eq!(
        ticked.brain.memories.int(MemoryKind::GazeCooldownTicks),
        Some(cooldown)
    );
    let yaw = crate::mobs::ai::maths::wrap_degrees(2.0 * yaw_draw * 30.0 - 30.0);
    let way = super::behaviours::looking::direction_from_rotation(0.0, yaw);
    let eye = EntityTypeInfo::of(entity_types::COW).eye_height;
    let expected = [0.5 + way[0], f64::from(eye) + way[1], 0.5 + way[2]];
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::LookTarget),
        Some(Value::Tracker(Tracker::Point(expected)))
    );
}

// ------------------------------------------------------------------------------------
// Swim (7.3.14).

static SWIMMING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || vec![ActivityData::numbered(Activity::Core, 0, vec![swim(0.8)])],
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// Afloat, a `Swim` starts (one draw) and draws one float a tick to jump; on dry land it
/// does not start.
#[test]
fn swimming_draws_a_float_a_tick_while_afloat() {
    let mut run = Run::new(&SWIMMING, [0.5, 0.0, 0.5]);
    assert_eq!(run.tick().draws, 0);
    let mut field = Field::default();
    for x in -2..=2 {
        for z in -2..=2 {
            field.set(BlockPos::new(x, 0, z), blocks::WATER);
            field.set(BlockPos::new(x, 1, z), blocks::WATER);
        }
    }
    let mut run = Run::on(field, &SWIMMING, [0.5, 0.0, 0.5]);
    assert_eq!(run.tick().draws, 2, "the start and the first tick's float");
    assert_eq!(run.tick().draws, 1);
}

// ------------------------------------------------------------------------------------
// Temptation and breeding (7.3.9, 7.3.11).

static TEMPTED: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![
            ActivityData::numbered(
                Activity::Core,
                0,
                vec![count_down(MemoryKind::TemptationCooldownTicks)],
            ),
            ActivityData::numbered(
                Activity::Idle,
                0,
                vec![FollowTemptation::new(Speed::Fixed(1.25)).boxed()],
            ),
        ]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A tempted mob follows the player to within 2 while beyond 2.5, and, the player gone,
/// stops with a cooldown of 100, which counts down to its erasure 101 ticks on.
#[test]
fn following_a_temptation_ends_with_its_cooldown() {
    let mut field = Field::default();
    field.players.push(player(77, [6.5, 0.0, 0.5], None));
    let mut run = Run::on(field, &TEMPTED, [0.5, 0.0, 0.5]);
    let ticked = run.tick_around(
        |brain| {
            brain
                .memories
                .set(MemoryKind::TemptingPlayer, Value::Entity(EntityId(77)))
        },
        |_, _| {},
    );
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::WalkTarget),
        Some(walk(
            Tracker::Entity {
                key: EntityId(77),
                eyes: false,
                eyes_block: false
            },
            1.25,
            2
        ))
    );
    let ticked = run.tick_around(
        |brain| brain.memories.erase(MemoryKind::TemptingPlayer),
        |_, _| {},
    );
    assert_eq!(
        ticked
            .brain
            .memories
            .int(MemoryKind::TemptationCooldownTicks),
        Some(100)
    );
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::WalkTarget, Status::Absent)
    );
    let mut gone_at = None;
    for _ in 0..105 {
        let at = run.lived;
        if run
            .tick()
            .brain
            .memories
            .check(MemoryKind::TemptationCooldownTicks, Status::Absent)
        {
            gone_at = Some(at);
            break;
        }
    }
    // Set in tick 1, lowered from tick 2 to nothing at tick 101, erased in tick 102.
    assert_eq!(gone_at, Some(102));
}

static COURTING: Provider = Provider {
    memories: &[],
    sensors: &[&NEAREST_LIVING_ENTITIES],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![AnimalMakeLove::new(COW).boxed()],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

fn in_love(mob: &mut Mob) {
    animal::update(mob, |animal| animal.set_in_love(0));
}

/// Two cows in love: the one that starts writes its partner's breed, look and walk
/// targets each tick by `Local::SetMemories`, and draws the moment of the child (60 +
/// `nextInt(50)` of its own stream after its start's draw); within 3 blocks at that
/// moment, it asks the region to breed and forgets the partner.
#[test]
fn making_love_writes_the_partner_and_asks_to_breed_at_the_drawn_moment() {
    let mut field = Field::default();
    let mut partner = healthy();
    in_love(&mut partner);
    field.put(record(EntityId(42), COW, [2.5, 0.0, 0.5], &partner), false);
    let mut run = Run::on(field, &COURTING, [0.5, 0.0, 0.5]);
    run.laid = true;
    in_love(&mut run.mob);
    let mut copy = run.stream();
    copy.next_int(1);
    let spawn_at = 60 + i64::from(copy.next_int(50));
    let ticked = run.tick();
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::BreedTarget),
        Some(Value::Entity(EntityId(42)))
    );
    let writes: Vec<&MemoryWrite> = ticked
        .out
        .local
        .iter()
        .filter_map(|local| match local {
            Local::SetMemories { key, writes } if *key == EntityId(42) => Some(writes),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(writes.contains(&&MemoryWrite::Set(
        MemoryKind::BreedTarget,
        Value::Entity(KEY)
    )));
    assert!(
        writes
            .iter()
            .any(|write| write.kind() == MemoryKind::WalkTarget)
    );
    let mut asked = None;
    for _ in 0..120 {
        let at = i64::from(run.lived);
        let ticked = run.tick();
        if ticked.out.local.iter().any(
            |local| matches!(local, Local::BreedAsk { partner, .. } if *partner == EntityId(42)),
        ) {
            asked = Some(at);
            assert!(
                ticked
                    .brain
                    .memories
                    .check(MemoryKind::BreedTarget, Status::Absent)
            );
            break;
        }
    }
    assert_eq!(asked, Some(spawn_at));
}

// ------------------------------------------------------------------------------------
// Doors (7.3.21).

static DOORS: Provider = Provider {
    memories: &[MemoryKind::Path],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![super::behaviours::doors::interact_with_door()],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A closed oak door at the path's next node is opened (`Local::OpenDoor`, its pitch of
/// one draw) and remembered.
#[test]
fn a_door_on_the_path_is_opened_and_remembered() {
    use crate::mobs::ai::pathfinder::{FoundPath, PathNode};
    let mut field = Field::default();
    let door = BlockPos::new(1, 0, 0);
    let state = blocks::OAK_DOOR;
    field.set(door, state);
    let mut run = Run::on(field, &DOORS, [0.5, 0.0, 0.5]);
    let node = |x| PathNode {
        x,
        y: 0,
        z: 0,
        kind: 0,
    };
    run.mob.body.path.followed = Some(FoundPath {
        nodes: vec![node(0), node(1), node(2)],
        target: BlockPos::new(2, 0, 0),
        dist_to_target: 0.0,
        reached: true,
    });
    run.mob.body.progress.next = 1;
    let ticked = run.tick_around(
        |brain| brain.memories.set(MemoryKind::Path, Value::Path),
        |_, _| {},
    );
    let opened = ticked
        .out
        .local
        .iter()
        .any(|local| matches!(local, Local::OpenDoor { pos, open: true, .. } if *pos == door));
    let is_door = state.property("hinge").is_some() && !clustine_blocks::data::flag(state, "open");
    assert_eq!(opened, is_door, "the default oak door is a closed door");
    if is_door {
        assert_eq!(ticked.draws, 1);
        assert_eq!(
            ticked.brain.memories.get(MemoryKind::DoorsToClose),
            Some(Value::Blocks(vec![door]))
        );
    }
}

// ------------------------------------------------------------------------------------
// Attacking (7.3.20): a one-shot, and a memory's expiry to the tick.

static FIGHTING: Provider = Provider {
    memories: &[],
    sensors: &[&NEAREST_LIVING_ENTITIES],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![super::behaviours::attacking::melee(20)],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A seen attack target within reach is hit once (a one-shot, stopped in its tick) and
/// the attack cools down: `attack_cooling_down`, set with an expiry of 20 in tick T, is
/// there through tick T + 20 and gone in the forget pass of T + 21, when the next hit
/// comes.
#[test]
fn a_melee_hit_cools_down_to_the_tick_of_its_expiry() {
    let mut field = Field::default();
    let target = EntityId(50);
    field.put(record(target, COW, [1.7, 0.0, 0.5], &healthy()), false);
    let mut run = Run::on(field, &FIGHTING, [0.5, 0.0, 0.5]);
    let hits = |ticked: &Ticked| {
        ticked
            .out
            .local
            .iter()
            .filter(|local| matches!(local, Local::Strike { key, .. } if *key == target))
            .count()
    };
    let ticked = run.tick_around(
        |brain| {
            brain
                .memories
                .set(MemoryKind::AttackTarget, Value::Entity(target))
        },
        |_, _| {},
    );
    assert_eq!(hits(&ticked), 1);
    assert!(
        run.entries
            .iter()
            .all(|entry| entry.id != behaviour_part(0)),
        "a one-shot keeps nothing"
    );
    for now in 1..=20 {
        let ticked = run.tick();
        assert_eq!(hits(&ticked), 0, "cooling down at {now}");
        assert!(
            ticked
                .brain
                .memories
                .check(MemoryKind::AttackCoolingDown, Status::Present)
        );
    }
    assert_eq!(hits(&run.tick()), 1, "hit again at 21");
}
