//! The brains of the sniffer, the allay and the ghastling, from the game's rules (the
//! brief's 7.9.3 to 7.9.5, 7.10.2 to 7.10.4 and 7.11.3, the dumps of `SnifferAi`,
//! `AllayAi`, `HappyGhastAi` and their behaviours): what each registers, its activities
//! and their switching, each behaviour of its own, and their draws, on a pen of grass with
//! the mob's brain ticked at its `customServerAiStep` as a region would tick it. The mob
//! does not move here (its moves are the region's tests'); what it means to do is read in
//! its memories and its state.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityData, EntityId, ItemStack};

use super::{ALLAY, GHAST, SNIFFER, body_of, ghast_with};
use crate::living::Living;
use crate::mobs::Mob;
use crate::mobs::body::Event;
use crate::mobs::brain::{
    self, Activity, Brain, Memories, MemoryKind, Provider, Sensed, Status, Tracker, Value,
};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::allay;
use crate::mobs::types::happy_ghast::{self, HappyGhastState};
use crate::mobs::types::sniffer::{self, State};
use crate::mobs::types::{animal, of};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;

/// The key of the mob ticked, and the world's seed of its stream.
const KEY: EntityId = EntityId(5);
const SEED: u64 = 11;

/// Grass on dirt below y = 0 and air above, with what a test puts on it; the chunks from
/// `beyond` east are not the region's (neither land nor copy).
#[derive(Default)]
struct Pen {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    players: Vec<SeenPlayer>,
    entities: Vec<(Entity, Aabb)>,
    /// The game time of the tick (its `lived`, as the tests take it).
    time: i64,
    /// What the game time is ahead of the mob's `lived`.
    ahead: i64,
    /// The world's players the store publishes, whom the view does not show.
    anywhere: Vec<SeenPlayer>,
    beyond: Option<i32>,
}

impl Pen {
    fn set(&mut self, pos: BlockPos, state: BlockState) {
        self.blocks.insert((pos.x, pos.y, pos.z), state);
    }

    fn put(&mut self, entity: Entity) {
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        self.entities.retain(|(other, _)| other.key != entity.key);
        self.entities.push((entity, bounds));
    }

    fn held(&self, chunk: ChunkPos) -> bool {
        self.beyond.is_none_or(|x| chunk.x < x)
    }
}

impl World for Pen {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if pos.y < -64 || pos.y >= 320 || !self.held(pos.chunk()) {
            return None;
        }
        Some(match self.blocks.get(&(pos.x, pos.y, pos.z)) {
            Some(state) => *state,
            None if pos.y < -1 => blocks::DIRT,
            None if pos.y == -1 => blocks::GRASS_BLOCK,
            None => blocks::AIR,
        })
    }

    fn has_chunk(&self, chunk: ChunkPos) -> bool {
        self.held(chunk)
    }
}

impl Holds for Pen {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Pen {
    fn is_loaded(&self, pos: BlockPos) -> bool {
        self.held(pos.chunk())
    }

    fn entities_in(&self, bounds: &Aabb) -> Vec<Seen<'_>> {
        self.entities
            .iter()
            .filter(|(_, own)| own.intersects(*bounds))
            .map(|(entity, own)| Seen {
                entity,
                bounds: *own,
                copied: false,
            })
            .collect()
    }

    fn entity(&self, key: EntityId) -> Option<Seen<'_>> {
        self.entities
            .iter()
            .find(|(entity, _)| entity.key == key)
            .map(|(entity, own)| Seen {
                entity,
                bounds: *own,
                copied: false,
            })
    }

    fn players(&self) -> Vec<SeenPlayer> {
        self.players.clone()
    }

    fn game_time(&self) -> i64 {
        self.time
    }

    fn player_anywhere(&self, uuid: u128) -> Option<SeenPlayer> {
        self.players
            .iter()
            .chain(&self.anywhere)
            .find(|player| player.uuid == uuid)
            .cloned()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }
}

/// A creative player at `position` holding `held`, of the key and UUID `key`.
fn player(key: u64, position: Vec3, held: Option<i32>) -> SeenPlayer {
    SeenPlayer {
        key: EntityId(key),
        uuid: u128::from(key),
        position,
        eye_height: 1.62,
        yaw: 0.0,
        pitch: 0.0,
        main_hand: held.map(|item| ItemStack { item, count: 1 }),
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

/// An item entity of `stack` at `position`, of the key `key`.
fn item(key: u64, stack: ItemStack, position: Vec3) -> Entity {
    Entity {
        key: EntityId(key),
        kind: entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key)),
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
        data: EntityData::Item {
            stack,
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

/// A mob whose brain ticks on a pen, its record carried between ticks as a region would.
struct Brainy {
    kind: u16,
    mob: Mob,
    living: Living,
    lived: u32,
    sensed: Option<Sensed>,
    yaw: f32,
}

/// What one tick left: what the mob put out and how many draws it made.
struct Stepped {
    out: MobOutput,
    draws: u64,
}

impl Brainy {
    fn new(kind: u16, mut mob: Mob, at: Vec3) -> Self {
        mob.body.vitals.health = 14.0;
        let mut living = body_of(kind, at);
        living.first_tick = false;
        let mut made = Self {
            kind,
            mob,
            living,
            lived: 0,
            sensed: Some(Sensed::new()),
            yaw: 0.0,
        };
        // Made as an egg makes it: its sensors' delays drawn.
        let mut own = Random::of_entity(SEED, KEY, 0);
        brain::construct(of(kind), &mut made.mob, &mut own);
        made
    }

    fn provider(&self) -> &'static Provider {
        of(self.kind).brain().expect("a type with a brain")
    }

    /// The mob's stream in its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(SEED, KEY, self.lived)
    }

    /// One tick of its `customServerAiStep`, where its brain ticks.
    fn step(&mut self, pen: &mut Pen) -> Stepped {
        pen.time = i64::from(self.lived) + pen.ahead;
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = MobTick {
            sensed: self.sensed.take(),
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            yaw: self.yaw,
            pitch: 0.0,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: self.mob.clone(),
            living: self.living.clone(),
            mob_type: of(self.kind),
            world: &*pen,
            draws: &mut draws,
            out: MobOutput::default(),
        };
        of(self.kind).custom_server_ai_step(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.sensed = tick.sensed;
        let out = tick.out;
        self.lived += 1;
        Stepped {
            out,
            draws: draws.draws_since(&start),
        }
    }

    /// Its brain as its next tick reads it.
    fn brain(&self) -> Brain {
        Brain::read(
            self.provider(),
            &self.mob.mind.brain,
            self.sensed.as_ref(),
            self.lived,
        )
    }

    /// Its memories as of its last tick, its sensed ones with them.
    fn memories(&self) -> Memories {
        self.brain().memories
    }

    /// `f` done to its brain between two ticks (as a hurt or a use would).
    fn change(&mut self, f: impl FnOnce(&mut Brain)) {
        let mut brain = self.brain();
        f(&mut brain);
        let (entries, sensed) = brain.write();
        self.mob.mind.brain = entries;
        self.sensed = Some(sensed);
    }

    /// Every sensor scans in its next tick (as one made with delays of nothing).
    fn scan_now(&mut self) {
        let sensors = self.provider().sensors.len();
        self.change(|brain| brain.state.next_scans = vec![0; sensors]);
    }

    fn state(&self) -> State {
        sniffer::read(&self.mob).state
    }

    fn active(&self) -> Option<Activity> {
        self.brain().active_non_core()
    }

    /// Ticks until `done` holds, at most `most`; how many it took.
    fn until(&mut self, pen: &mut Pen, most: u32, mut done: impl FnMut(&Self) -> bool) -> u32 {
        for n in 1..=most {
            self.step(pen);
            if done(self) {
                return n;
            }
        }
        panic!("not within {most} ticks");
    }
}

/// The memories a brain of `provider` registers.
fn registered(provider: &'static Provider) -> BTreeSet<MemoryKind> {
    Brain::read(provider, &[], Some(&Sensed::new()), 0)
        .memories
        .registered
}

fn sounds(out: &MobOutput) -> Vec<i32> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------
// What each registers.

#[test]
fn each_brain_registers_the_memories_its_provider_sensors_and_behaviours_name() {
    use MemoryKind as M;
    let sensed_by_all = [
        M::Mobs,
        M::VisibleMobs,
        M::HurtBy,
        M::HurtByEntity,
        M::NearestPlayers,
        M::NearestVisiblePlayer,
        M::NearestVisibleTargetablePlayer,
        M::NearestVisibleTargetablePlayers,
    ];
    let moved = [
        M::IsPanicking,
        M::LookTarget,
        M::WalkTarget,
        M::Path,
        M::CantReachWalkTargetSince,
    ];
    let set = |extra: &[M]| -> BTreeSet<M> {
        sensed_by_all
            .iter()
            .chain(moved.iter())
            .chain(extra)
            .copied()
            .collect()
    };
    // `Sniffer.brainProvider`: its explored positions; its food's temptation; love,
    // temptation and its own six behaviours' memories.
    assert_eq!(
        registered(&sniffer::PROVIDER),
        set(&[
            M::SnifferExploredPositions,
            M::TemptingPlayer,
            M::TemptationCooldownTicks,
            M::BreedTarget,
            M::SnifferHappy,
            M::SnifferDigging,
            M::SnifferSniffingTarget,
            M::SniffCooldown,
        ])
    );
    // `Allay.brainProvider`: the player and the note block it likes and the note block's
    // cooldown; the item it wants; the pick-up's cooldown.
    assert_eq!(
        registered(&allay::PROVIDER),
        set(&[
            M::LikedPlayer,
            M::LikedNoteblock,
            M::LikedNoteblockCooldownTicks,
            M::NearestVisibleWantedItem,
            M::ItemPickupCooldownTicks,
        ])
    );
    // `HappyGhast.brainProvider`: nothing of its own; snowballs tempt it, it follows
    // adults of any followable kind.
    assert_eq!(
        registered(&happy_ghast::PROVIDER),
        set(&[
            M::TemptingPlayer,
            M::TemptationCooldownTicks,
            M::BreedTarget,
            M::NearestVisibleAdult,
        ])
    );
    // A kind not registered is never there, as the game's `checkMemory`.
    let brain = Brain::read(&allay::PROVIDER, &[], Some(&Sensed::new()), 0);
    assert!(!brain.memories.check(M::SnifferDigging, Status::Absent));
}

#[test]
fn each_is_made_with_one_draw_of_twenty_per_sensor_in_the_provider_s_order() {
    for (kind, sensors) in [(SNIFFER, 4), (ALLAY, 4), (GHAST, 5)] {
        let mut mob = Mob::fresh();
        let mut own = Random::of_entity(SEED, KEY, 0);
        let start = own.clone();
        brain::construct(of(kind), &mut mob, &mut own);
        assert_eq!(own.draws_since(&start), sensors, "{kind}");
        let mut expected = start.clone();
        let state = Brain::read(of(kind).brain().unwrap(), &mob.mind.brain, None, 0).state;
        let delays: Vec<u32> = (0..sensors).map(|_| expected.next_int(20)).collect();
        // A sensor of delay `k` scans at the `k`-th tick, the first where it is 0 or 1.
        let scans: Vec<u32> = delays.iter().map(|delay| delay.max(&1) - 1).collect();
        assert_eq!(state.next_scans, scans, "{kind}");
    }
    // A grown happy ghast's brain is made too, though it never ticks.
    let mut adult = ghast_with(0, &HappyGhastState::default());
    let mut own = Random::of_entity(SEED, KEY, 0);
    brain::construct(of(GHAST), &mut adult, &mut own);
    assert!(!adult.mind.brain.is_empty());
}

// ------------------------------------------------------------------------------------
// The sniffer.

fn sniffer_at(at: Vec3) -> Brainy {
    Brainy::new(SNIFFER, Mob::fresh(), at)
}

#[test]
fn a_sniffer_digs_where_it_has_a_dig_then_sniffs_where_it_has_a_place_and_else_idles() {
    use MemoryKind as M;
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    let update = |brain: &mut Brain| sniffer::update_activity(brain);
    let mut brain = brainy.brain();
    brain.state.active = Some(Activity::Idle);
    update(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Idle));
    // A place to sniff and a walk there: sniff.
    let place = BlockPos::new(4, -1, 9);
    brain
        .memories
        .set(M::SnifferSniffingTarget, Value::Block(place));
    brain.memories.set(
        M::WalkTarget,
        Value::Walk {
            to: Tracker::Block(place),
            speed: 1.25,
            close_enough: 0,
        },
    );
    update(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Sniff));
    // Panicking, it does not sniff: idle again (digging is absent).
    brain.memories.set(M::IsPanicking, Value::Bool(true));
    update(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Idle));
    brain.memories.erase(M::IsPanicking);
    // A dig with no walk: dig, first of the three.
    brain.memories.erase(M::WalkTarget);
    brain.memories.set(M::SnifferDigging, Value::Bool(true));
    update(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Dig));
    // A dig with a walk target and no place to sniff: none qualifies, and the activity
    // it had stays (`setActiveActivityToFirstValid` changes nothing).
    brain.memories.erase(M::SnifferSniffingTarget);
    brain.memories.set(
        M::WalkTarget,
        Value::Walk {
            to: Tracker::Block(place),
            speed: 1.0,
            close_enough: 0,
        },
    );
    update(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Dig));
    brainy.change(|kept| kept.state.active = Some(Activity::Sniff));
    let mut brain = brainy.brain();
    brain.memories.set(M::SnifferDigging, Value::Bool(true));
    brain.memories.set(
        M::WalkTarget,
        Value::Walk {
            to: Tracker::Block(place),
            speed: 1.0,
            close_enough: 0,
        },
    );
    update(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Sniff));
}

#[test]
fn a_sniffer_with_a_dig_digs_its_whole_time_rises_for_40_and_is_happy_after() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    // Facing south, its nose is over the grass at z = 2.
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    brainy.change(|brain| {
        brain.state.active = Some(Activity::Dig);
        brain.memories.set(M::SnifferDigging, Value::Bool(true));
    });
    // The first tick: its only draw is `Digging`'s start, `nextInt(21)` of 160 to 180.
    let mut expected = brainy.stream();
    let drawn = i64::from(expected.next_int(21));
    let started = brainy.lived;
    let first = brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Digging);
    assert_eq!(first.draws, 1);
    assert!(first.out.events.contains(&Event::Entity(63)));
    // Its seed's tick: the game's `tickCount` of this tick and 120.
    assert_eq!(
        sniffer::read(&brainy.mob).drop_seed_at,
        started as i32 + 1 + sniffer::DROP_SEED_AFTER
    );
    // It digs until the game time is past its end, 160 and the draw after its start, and
    // stops with its cooldown of 9600 (9599 as the next tick reads it).
    let n = brainy.until(&mut pen, 400, |b| {
        b.memories().check(M::SniffCooldown, Status::Present)
    });
    assert_eq!(i64::from(n), 160 + drawn + 1);
    let memories = brainy.memories();
    assert_eq!(
        memories.expires_in(M::SniffCooldown),
        sniffer::SNIFF_COOLDOWN - 1
    );
    assert!(memories.check(M::SnifferDigging, Status::Present));
    // In the tick it stopped (a whole dig changes no state), the cooldown came too late
    // for `FinishedDigging`; it rises in the next, with its sound.
    assert_eq!(brainy.state(), State::Digging);
    let rising = brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Rising);
    assert_eq!(sounds(&rising.out), [sounds::ENTITY_SNIFFER_DIGGING_STOP]);
    let n = brainy.until(&mut pen, 100, |b| b.state() != State::Rising);
    assert_eq!(n, 41);
    // Done: where it stood is explored, it is happy and idle again.
    let memories = brainy.memories();
    assert_eq!(
        memories.get(M::SnifferExploredPositions),
        Some(Value::Blocks(vec![BlockPos::new(0, -1, 0)]))
    );
    assert!(memories.check(M::SnifferHappy, Status::Present));
    assert!(memories.check(M::SnifferDigging, Status::Absent));
    assert_eq!(brainy.active(), Some(Activity::Idle));
    let happy = brainy.until(&mut pen, 5, |b| b.state() == State::FeelingHappy);
    assert_eq!(happy, 1);
    let n = brainy.until(&mut pen, 200, |b| b.state() != State::FeelingHappy);
    assert!((41..=101).contains(&n), "{n}");
    assert!(brainy.memories().check(M::SnifferHappy, Status::Absent));
    // For 9600 ticks from its dig's end it does not sniff (`sniff_cooldown`).
    assert!(brainy.memories().check(M::SniffCooldown, Status::Present));
}

#[test]
fn a_dig_cut_short_by_a_tempting_player_leaves_no_cooldown_and_the_sniffer_idle() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    brainy.change(|brain| {
        brain.state.active = Some(Activity::Dig);
        brain.memories.set(M::SnifferDigging, Value::Bool(true));
    });
    brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Digging);
    // A player with torchflower seeds within its tempt range of 10: once its sensor sees
    // them, it may dig no more.
    pen.players
        .push(player(77, [4.5, 0.0, 0.5], Some(items::TORCHFLOWER_SEEDS)));
    brainy.until(&mut pen, 40, |b| b.state() != State::Digging);
    let memories = brainy.memories();
    assert!(memories.check(M::SniffCooldown, Status::Absent));
    assert!(memories.check(M::SnifferDigging, Status::Absent));
    assert_eq!(brainy.state(), State::Idling);
    assert_eq!(brainy.active(), Some(Activity::Idle));
}

#[test]
fn a_sniffer_does_not_dig_where_its_nose_is_over_stone_or_ground_it_has_explored() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    pen.set(BlockPos::new(0, -1, 2), blocks::STONE);
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    brainy.change(|brain| {
        brain.state.active = Some(Activity::Dig);
        brain.memories.set(M::SnifferDigging, Value::Bool(true));
    });
    brainy.step(&mut pen);
    // It starts (its start asks only `canSniff`) and stops at once, cut short.
    assert_eq!(brainy.state(), State::Idling);
    assert!(brainy.memories().check(M::SnifferDigging, Status::Absent));
    // Grass it has explored is no better.
    let mut pen = Pen::default();
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    brainy.change(|brain| {
        brain.state.active = Some(Activity::Dig);
        brain.memories.set(M::SnifferDigging, Value::Bool(true));
        brain.memories.set(
            M::SnifferExploredPositions,
            Value::Blocks(vec![BlockPos::new(0, -1, 2)]),
        );
    });
    brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Idling);
}

#[test]
fn a_sniffer_keeps_the_newest_21_places_it_explored_newest_first() {
    let mut explored: Vec<BlockPos> = Vec::new();
    for x in 0..30 {
        explored = sniffer::store_explored(&explored, BlockPos::new(x, -1, 0));
    }
    assert_eq!(explored.len(), 21);
    assert_eq!(explored[0], BlockPos::new(29, -1, 0));
    assert_eq!(explored[20], BlockPos::new(9, -1, 0));
}

#[test]
fn an_idle_sniffer_scents_and_sniffs_and_a_whole_sniff_chooses_a_grass_place_within_18() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    let mut scented = false;
    let mut began = None;
    let mut was = State::Idling;
    for _ in 0..20_000 {
        let stepped = brainy.step(&mut pen);
        if brainy.state() == State::Scenting && was != State::Scenting {
            scented = true;
            assert!(sounds(&stepped.out).contains(&sounds::ENTITY_SNIFFER_SCENTING));
        }
        was = brainy.state();
        if brainy.state() == State::Sniffing && began.is_none() {
            assert!(sounds(&stepped.out).contains(&sounds::ENTITY_SNIFFER_SNIFFING));
            began = Some(brainy.lived);
        }
        if began.is_some() && brainy.state() != State::Sniffing {
            break;
        }
    }
    let began = began.expect("it sniffs");
    let length = brainy.lived - began;
    // A whole sniff (it is never cut short on its pen): 41 to 81 ticks.
    assert!((41..=81).contains(&length), "{length}");
    let memories = brainy.memories();
    let Some(Value::Block(place)) = memories.get(M::SnifferSniffingTarget) else {
        panic!("a place: {:?}", memories.slots);
    };
    assert_eq!(place.y, -1, "the ground under a place on land");
    assert!(place.x.abs() <= 19 && place.z.abs() <= 19, "{place:?}");
    assert_eq!(
        memories.get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Block(place),
            speed: 1.25,
            close_enough: 0,
        })
    );
    // Then it searches its way there, in the sniff activity.
    assert_eq!(brainy.active(), Some(Activity::Sniff));
    brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Searching);
    assert!(scented, "it scented on its way");
}

#[test]
fn a_search_ends_with_its_walk_and_digs_where_the_nose_then_is() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    let place = BlockPos::new(0, -1, 9);
    let walk = Value::Walk {
        to: Tracker::Block(place),
        speed: 1.25,
        close_enough: 0,
    };
    brainy.change(|brain| {
        brain.state.active = Some(Activity::Sniff);
        brain
            .memories
            .set(M::SnifferSniffingTarget, Value::Block(place));
        brain.memories.set(M::WalkTarget, walk.clone());
    });
    brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Searching);
    // Its walk ends (the sink erases it on arrival): it digs at its nose, over grass.
    brainy.change(|brain| brain.memories.erase(M::WalkTarget));
    brainy.step(&mut pen);
    let memories = brainy.memories();
    assert!(memories.check(M::SnifferDigging, Status::Present));
    assert!(memories.check(M::SnifferSniffingTarget, Status::Absent));
    assert_eq!(brainy.active(), Some(Activity::Dig));
    // A walk elsewhere ends a search too; over stone it does not dig.
    let mut pen = Pen::default();
    pen.set(BlockPos::new(0, -1, 2), blocks::STONE);
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    brainy.change(|brain| {
        brain.state.active = Some(Activity::Sniff);
        brain
            .memories
            .set(M::SnifferSniffingTarget, Value::Block(place));
        brain.memories.set(M::WalkTarget, walk.clone());
    });
    brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Searching);
    brainy.change(|brain| {
        brain.memories.set(
            M::WalkTarget,
            Value::Walk {
                to: Tracker::Block(BlockPos::new(5, -1, 5)),
                speed: 1.0,
                close_enough: 0,
            },
        );
    });
    brainy.step(&mut pen);
    let memories = brainy.memories();
    assert!(memories.check(M::SnifferDigging, Status::Absent));
    assert!(memories.check(M::WalkTarget, Status::Absent));
    assert!(memories.check(M::SnifferSniffingTarget, Status::Absent));
}

#[test]
fn panic_love_and_temptation_each_reset_a_sniff_before_they_start() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    let mut brainy = sniffer_at([0.5, 0.0, 0.5]);
    let place = BlockPos::new(0, -1, 9);
    brainy.change(|brain| {
        brain.state.active = Some(Activity::Sniff);
        brain
            .memories
            .set(M::SnifferSniffingTarget, Value::Block(place));
        brain.memories.set(
            M::WalkTarget,
            Value::Walk {
                to: Tracker::Block(place),
                speed: 1.25,
                close_enough: 0,
            },
        );
    });
    brainy.step(&mut pen);
    assert_eq!(brainy.state(), State::Searching);
    // A panic (the core's, at any activity): its sniffing target and dig are forgotten,
    // it is idle, then it panics.
    brainy.change(|brain| brain.memories.set(M::IsPanicking, Value::Bool(true)));
    brainy.step(&mut pen);
    let memories = brainy.memories();
    assert!(memories.check(M::SnifferSniffingTarget, Status::Absent));
    assert!(memories.check(M::IsPanicking, Status::Present));
    assert_eq!(brainy.state(), State::Idling);
}

// ------------------------------------------------------------------------------------
// The allay.

fn allay_holding(item: i32, at: Vec3) -> Brainy {
    let mut mob = Mob::fresh();
    mob.body.equipment.slots[0] = Some(ItemStack { item, count: 1 });
    let mut brainy = Brainy::new(ALLAY, mob, at);
    brainy.scan_now();
    brainy
}

fn likes(brainy: &mut Brainy, uuid: u128) {
    brainy.change(|brain| {
        brain
            .memories
            .set(MemoryKind::LikedPlayer, Value::Uuid(uuid));
    });
}

#[test]
fn an_allay_holding_a_kind_goes_for_the_nearest_it_sees_within_32_and_not_while_it_waits() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    let diamonds = ItemStack {
        item: items::DIAMOND,
        count: 2,
    };
    pen.put(item(70, one_of(items::DIRT), [3.5, 0.0, 0.5]));
    pen.put(item(71, diamonds, [10.5, 0.0, 0.5]));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    brainy.until(&mut pen, 40, |b| {
        b.memories()
            .check(M::NearestVisibleWantedItem, Status::Present)
    });
    // The item sensor saw the diamonds, not the nearer dirt; the next tick it goes.
    brainy.until(&mut pen, 2, |b| {
        b.memories().check(M::WalkTarget, Status::Present)
    });
    let memories = brainy.memories();
    assert_eq!(
        memories.get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Entity {
                key: EntityId(71),
                eyes: false,
                eyes_block: false,
            },
            speed: 1.75,
            close_enough: 0,
        })
    );
    // Waiting after a throw, it does not.
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    brainy.change(|brain| {
        brain
            .memories
            .set(M::ItemPickupCooldownTicks, Value::Int(60));
    });
    for _ in 0..40 {
        brainy.step(&mut pen);
        let walking = brainy.memories().get(M::WalkTarget);
        assert!(
            !matches!(
                walking,
                Some(Value::Walk {
                    to: Tracker::Entity {
                        key: EntityId(71),
                        ..
                    },
                    ..
                })
            ),
            "{walking:?}"
        );
    }
    // And holding nothing, it wants nothing.
    let mut brainy = Brainy::new(ALLAY, Mob::fresh(), [0.5, 1.0, 0.5]);
    for _ in 0..40 {
        brainy.step(&mut pen);
        assert!(
            brainy
                .memories()
                .check(M::NearestVisibleWantedItem, Status::Absent)
        );
    }
}

fn one_of(item: i32) -> ItemStack {
    ItemStack { item, count: 1 }
}

#[test]
fn an_allay_with_items_flies_to_the_player_it_likes_and_within_3_throws_one_a_tick() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    pen.players.push(player(77, [6.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    allay::update(&mut brainy.mob, |own| {
        own.inventory = Some(ItemStack {
            item: items::DIAMOND,
            count: 3,
        });
    });
    // Too far to throw: it flies there at 2.25, its look on the player's eyes.
    brainy.step(&mut pen);
    let memories = brainy.memories();
    let to = Tracker::Entity {
        key: EntityId(77),
        eyes: true,
        eyes_block: false,
    };
    assert_eq!(memories.get(M::LookTarget), Some(Value::Tracker(to)));
    assert_eq!(
        memories.get(M::WalkTarget),
        Some(Value::Walk {
            to,
            speed: 2.25,
            close_enough: 3,
        })
    );
    assert!(allay::read(&brainy.mob).inventory.is_some());
    // Within 3 of their eyes (from its own): one item a tick, aimed a block above their
    // eyes, each starting the 60 ticks it waits before it picks up again.
    brainy.living.body.position = [5.0, 1.0, 0.5];
    let mut thrown = Vec::new();
    for _ in 0..5 {
        let stepped = brainy.step(&mut pen);
        thrown.extend(
            stepped
                .out
                .local
                .into_iter()
                .filter_map(|local| match local {
                    Local::Throw {
                        stack, at, motion, ..
                    } => Some((stack, at, motion)),
                    _ => None,
                }),
        );
    }
    assert_eq!(thrown.len(), 3);
    let eye = f64::from(EntityTypeInfo::of(entity_types::ALLAY).eye_height);
    let (stack, at, motion) = thrown[0];
    assert_eq!(stack, one_of(items::DIAMOND));
    assert_eq!(at, [5.0, 1.0 + eye - f64::from(0.2_f32), 0.5]);
    let aim = crate::physics::normalize([6.5 - 5.0, 1.62 + 1.0 - 1.0, 0.0]);
    assert_eq!(
        motion,
        [
            aim[0] * f64::from(0.2_f32),
            aim[1] * f64::from(0.3_f32),
            aim[2] * f64::from(0.2_f32),
        ]
    );
    assert_eq!(allay::read(&brainy.mob).inventory, None);
    let memories = brainy.memories();
    assert!(memories.check(M::ItemPickupCooldownTicks, Status::Present));
}

#[test]
fn an_allay_s_throw_is_heard_only_on_a_seventh_tick_with_two_more_draws() {
    // One tick of a throw at game time `time`, of the same stream: its draws, and whether
    // it was heard.
    let throw_at = |time: i64| -> (u64, bool) {
        let mut pen = Pen {
            ahead: time - 8,
            ..Pen::default()
        };
        pen.players.push(player(77, [2.5, 0.0, 0.5], None));
        let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
        likes(&mut brainy, 77);
        allay::update(&mut brainy.mob, |own| {
            own.inventory = Some(ItemStack {
                item: items::DIAMOND,
                count: 64,
            });
        });
        brainy.lived = 8;
        // Its sensors out of the way: a tick that starts the giving, and throws.
        brainy.change(|brain| {
            brain.state.active = Some(Activity::Idle);
            brain.state.next_scans = vec![u32::MAX; 4];
        });
        let stepped = brainy.step(&mut pen);
        let heard = sounds(&stepped.out).contains(&sounds::ENTITY_ALLAY_ITEM_THROWN);
        (stepped.draws, heard)
    };
    let (quiet, heard) = throw_at(8);
    assert!(!heard);
    let (loud, _) = throw_at(14);
    // On the seventh: `nextDouble`, and where below 0.9, `nextInt(16)` for its pitch.
    assert!(loud == quiet + 1 || loud == quiet + 2, "{quiet} {loud}");
}

#[test]
fn an_allay_takes_its_items_above_a_note_block_it_likes_and_forgets_one_that_is_not() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    let note = BlockPos::new(8, 0, 0);
    pen.set(note, blocks::NOTE_BLOCK);
    pen.players.push(player(77, [-6.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    allay::update(&mut brainy.mob, |own| {
        own.inventory = Some(one_of(items::DIAMOND))
    });
    brainy.change(|brain| {
        allay::brain_seams::hear_noteblock(&mut brain.memories, note);
    });
    brainy.step(&mut pen);
    assert_eq!(
        brainy.memories().get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Block(note.offset(0, 1, 0)),
            speed: 2.25,
            close_enough: 3,
        })
    );
    // Its block no longer a note block: forgotten, and the player taken instead.
    pen.set(note, blocks::STONE);
    brainy.change(|brain| brain.memories.erase(M::WalkTarget));
    brainy.until(&mut pen, 30, |b| {
        b.memories().check(M::WalkTarget, Status::Present)
    });
    let memories = brainy.memories();
    assert!(memories.check(M::LikedNoteblock, Status::Absent));
    assert!(matches!(
        memories.get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Entity {
                key: EntityId(77),
                ..
            },
            ..
        })
    ));
}

#[test]
fn a_liked_note_block_beyond_the_region_is_kept_and_the_player_taken_for_the_while() {
    use MemoryKind as M;
    let mut pen = Pen {
        beyond: Some(1),
        ..Pen::default()
    };
    let note = BlockPos::new(20, 0, 0);
    pen.players.push(player(77, [-6.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    allay::update(&mut brainy.mob, |own| {
        own.inventory = Some(one_of(items::DIAMOND))
    });
    brainy.change(|brain| allay::brain_seams::hear_noteblock(&mut brain.memories, note));
    brainy.step(&mut pen);
    let memories = brainy.memories();
    assert_eq!(memories.get(M::LikedNoteblock), Some(Value::Block(note)));
    assert!(matches!(
        memories.get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Entity {
                key: EntityId(77),
                ..
            },
            ..
        })
    ));
}

#[test]
fn a_note_block_is_liked_for_600_ticks_from_its_last_hearing_and_another_is_passed_over() {
    use MemoryKind as M;
    let mut brain = Brain::read(&allay::PROVIDER, &[], Some(&Sensed::new()), 100);
    let first = BlockPos::new(1, 2, 3);
    allay::brain_seams::hear_noteblock(&mut brain.memories, first);
    assert_eq!(
        brain.memories.get(M::LikedNoteblock),
        Some(Value::Block(first))
    );
    assert_eq!(
        brain.memories.int(M::LikedNoteblockCooldownTicks),
        Some(600)
    );
    // Another is not liked, nor heard by its listener.
    let other = BlockPos::new(9, 2, 3);
    allay::brain_seams::hear_noteblock(&mut brain.memories, other);
    assert_eq!(
        brain.memories.get(M::LikedNoteblock),
        Some(Value::Block(first))
    );
    let at = BlockPos::new(0, 0, 0);
    assert!(allay::brain_seams::can_receive_vibration(
        &brain.memories,
        at,
        first,
        false
    ));
    assert!(!allay::brain_seams::can_receive_vibration(
        &brain.memories,
        at,
        other,
        false
    ));
    assert!(!allay::brain_seams::can_receive_vibration(
        &brain.memories,
        at,
        first,
        true
    ));
    // Liking none, it hears any.
    let fresh = Brain::read(&allay::PROVIDER, &[], Some(&Sensed::new()), 100);
    assert!(allay::brain_seams::can_receive_vibration(
        &fresh.memories,
        at,
        other,
        false
    ));
}

#[test]
fn an_allay_s_player_only_the_world_s_players_show_is_flown_to_where_they_were_last_told() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    // Beyond the region's view (another region, past its copies), 30 away.
    pen.anywhere.push(player(77, [30.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    brainy.step(&mut pen);
    assert_eq!(
        brainy.memories().get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Point([30.5, 1.62, 0.5]),
            speed: 2.25,
            close_enough: 4,
        })
    );
    // Beyond 64, nobody.
    let mut pen = Pen::default();
    pen.anywhere.push(player(77, [80.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    brainy.step(&mut pen);
    assert!(!matches!(
        brainy.memories().get(M::WalkTarget),
        Some(Value::Walk {
            close_enough: 4,
            ..
        })
    ));
}

#[test]
fn a_fresh_load_resets_what_the_game_does_not_save_of_each() {
    // A sniffer: idle, with no seed to drop.
    let mut digging = Mob::fresh();
    sniffer::update(&mut digging, |own| {
        own.state = State::Digging;
        own.drop_seed_at = 130;
    });
    sniffer::fresh_load(&mut digging);
    assert_eq!(sniffer::read(&digging), sniffer::SnifferState::default());
    // An allay: not dancing, but its cooldown and its items kept.
    let mut dancing = Mob::fresh();
    allay::update(&mut dancing, |own| {
        own.dancing = true;
        own.jukebox = Some(BlockPos::new(1, 2, 3));
        own.cooldown_until = Some(900);
        own.inventory = Some(one_of(items::DIAMOND));
    });
    allay::fresh_load(&mut dancing);
    let own = allay::read(&dancing);
    assert!(!own.dancing && own.jukebox.is_none());
    assert_eq!(own.cooldown_until, Some(900));
    assert_eq!(own.inventory, Some(one_of(items::DIAMOND)));
    // A happy ghast: still for its saved timeout, flagged again; no leash held.
    let mut still = ghast_with(
        0,
        &HappyGhastState {
            still_timeout: 7,
            stays_still: false,
            leash_holder: true,
            impulse_at: 900,
        },
    );
    happy_ghast::fresh_load(&mut still);
    assert_eq!(
        happy_ghast::read(&still),
        HappyGhastState {
            still_timeout: 7,
            stays_still: true,
            leash_holder: false,
            impulse_at: 0,
        }
    );
}

#[test]
fn an_allay_seeing_nothing_it_wants_comes_back_within_4_of_its_player_from_16_or_more() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    pen.players.push(player(77, [20.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    brainy.step(&mut pen);
    let memories = brainy.memories();
    assert_eq!(
        memories.get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Entity {
                key: EntityId(77),
                eyes: true,
                eyes_block: false,
            },
            speed: 2.25,
            close_enough: 4,
        })
    );
    // Within 16 it stays where it is.
    let mut pen = Pen::default();
    pen.players.push(player(77, [10.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    brainy.step(&mut pen);
    assert!(!matches!(
        brainy.memories().get(M::WalkTarget),
        Some(Value::Walk {
            close_enough: 4,
            ..
        })
    ));
    // A player beyond 64 is not liked any more as a target.
    let mut pen = Pen::default();
    pen.players.push(player(77, [70.5, 0.0, 0.5], None));
    let mut brainy = allay_holding(items::DIAMOND, [0.5, 1.0, 0.5]);
    likes(&mut brainy, 77);
    brainy.step(&mut pen);
    assert!(!matches!(
        brainy.memories().get(M::WalkTarget),
        Some(Value::Walk {
            close_enough: 4,
            ..
        })
    ));
}

#[test]
fn a_panicking_allay_stops_dancing() {
    let mut pen = Pen::default();
    let mut brainy = Brainy::new(ALLAY, Mob::fresh(), [0.5, 1.0, 0.5]);
    allay::update(&mut brainy.mob, |own| own.dancing = true);
    brainy.change(|brain| {
        brain
            .memories
            .set(MemoryKind::IsPanicking, Value::Bool(true));
    });
    brainy.step(&mut pen);
    let tick_after = |brainy: &mut Brainy| {
        let mut draws = brainy.stream();
        let mut tick = MobTick {
            sensed: brainy.sensed.take(),
            ridden: Default::default(),
            key: KEY,
            kind: ALLAY,
            lived: brainy.lived - 1,
            tick_count: brainy.lived - 1,
            yaw: 0.0,
            pitch: 0.0,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: brainy.mob.clone(),
            living: brainy.living.clone(),
            mob_type: of(ALLAY),
            world: &pen,
            draws: &mut draws,
            out: MobOutput::default(),
        };
        of(ALLAY).after_tick(&mut tick);
        brainy.mob = tick.mob;
        brainy.sensed = tick.sensed;
    };
    tick_after(&mut brainy);
    assert!(!allay::read(&brainy.mob).dancing);
}

// ------------------------------------------------------------------------------------
// The ghastling.

fn ghastling_at(at: Vec3) -> Brainy {
    let mut brainy = Brainy::new(GHAST, ghast_with(-24_000, &HappyGhastState::default()), at);
    brainy.scan_now();
    brainy
}

#[test]
fn a_ghastling_panics_in_its_panic_activity_and_idles_else() {
    let mut brainy = ghastling_at([0.5, 4.0, 0.5]);
    let mut brain = brainy.brain();
    brain.state.active = Some(Activity::Idle);
    happy_ghast::update_activity(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Idle));
    brain
        .memories
        .set(MemoryKind::IsPanicking, Value::Bool(true));
    happy_ghast::update_activity(&mut brain);
    assert_eq!(brain.active_non_core(), Some(Activity::Panic));
    // A hurt by a player panics it, in its next tick.
    let mut pen = Pen::default();
    brainy.mob.body.combat.last_hurt_by = Some((EntityId(77), 0));
    brainy.mob.body.combat.last_hurt_by_player = Some((77, 0));
    brainy.mob.body.last_damage.last = Some((
        crate::mobs::body::hurt::DamageType::PLAYER_ATTACK
            .id()
            .expect("a damage type"),
        Some(EntityId(77)),
        i64::from(brainy.lived) + pen.ahead,
    ));
    pen.players.push(player(77, [3.5, 4.0, 0.5], None));
    brainy.until(&mut pen, 25, |b| b.active() == Some(Activity::Panic));
}

#[test]
fn a_ghastling_follows_the_nearest_player_it_sees_from_within_17_to_within_2() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    pen.players.push(player(77, [9.5, 4.0, 0.5], None));
    let mut brainy = ghastling_at([0.5, 4.0, 0.5]);
    brainy.until(&mut pen, 40, |b| {
        b.memories().check(M::WalkTarget, Status::Present)
    });
    let memories = brainy.memories();
    let following = Tracker::Entity {
        key: EntityId(77),
        eyes: true,
        eyes_block: true,
    };
    assert_eq!(
        memories.get(M::WalkTarget),
        Some(Value::Walk {
            to: following,
            speed: 1.1,
            close_enough: 2,
        })
    );
    // Nearer than 3, it does not follow.
    let mut pen = Pen::default();
    pen.players.push(player(77, [2.5, 4.0, 0.5], None));
    let mut brainy = ghastling_at([0.5, 4.0, 0.5]);
    for _ in 0..40 {
        brainy.step(&mut pen);
        assert!(!matches!(
            brainy.memories().get(M::WalkTarget),
            Some(Value::Walk { speed: 1.1, .. })
        ));
    }
}

#[test]
fn a_ghastling_is_tempted_by_a_snowball_within_16_to_within_3_of_the_player() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    pen.players
        .push(player(77, [12.5, 4.0, 0.5], Some(items::SNOWBALL)));
    let mut brainy = ghastling_at([0.5, 4.0, 0.5]);
    brainy.until(&mut pen, 40, |b| {
        matches!(
            b.memories().get(M::WalkTarget),
            Some(Value::Walk { speed: 1.25, .. })
        )
    });
    assert_eq!(
        brainy.memories().get(M::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Entity {
                key: EntityId(77),
                eyes: true,
                eyes_block: true,
            },
            speed: 1.25,
            close_enough: 2,
        })
    );
    // The player puts the snowball away: at the next scan it ends, with its cooldown of
    // 100.
    pen.players[0].main_hand = None;
    brainy.until(&mut pen, 25, |b| {
        b.memories()
            .check(M::TemptationCooldownTicks, Status::Present)
    });
}

#[test]
fn a_ghastling_grown_up_forgets_its_brain_and_a_grown_one_never_ticks_its_own() {
    use MemoryKind as M;
    let mut pen = Pen::default();
    pen.players.push(player(77, [9.5, 4.0, 0.5], None));
    let mut brainy = ghastling_at([0.5, 4.0, 0.5]);
    brainy.until(&mut pen, 40, |b| {
        b.memories().check(M::WalkTarget, Status::Present)
    });
    // Grown by a snowball (a use, between ticks): at its next AI step its brain is
    // stopped and forgotten, but for the brain's own state.
    animal::update(&mut brainy.mob, |shared| shared.set_age(0, 0));
    brainy.step(&mut pen);
    assert!(
        brainy
            .mob
            .mind
            .brain
            .iter()
            .all(|entry| entry.id == brain::ids::STATE)
    );
    assert!(brainy.memories().slots.is_empty());
    // A grown one's brain does not tick: nothing is sensed, its scans do not move.
    let before = brainy.mob.mind.brain.clone();
    for _ in 0..40 {
        let stepped = brainy.step(&mut pen);
        assert_eq!(stepped.draws, 0);
    }
    assert_eq!(brainy.mob.mind.brain, before);
}
