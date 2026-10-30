//! Part V3's social life from the game's rules (`docs/groundwork/n10-brief.md`, section
//! 4; `n10-dumps.md`, part B, sections 1–3; `n10-plan.md`, section 7), each behaviour on
//! a villager of its own on a field, before the villager's packages (part V1) name them:
//! gossip and its draws, decay and the reputation events, trade between villagers,
//! love with and without a bed, a golem summoned by villagers that slept, hiding from a
//! bell, the trades shown to a player, a hero's gift, and a murder's witnesses. The
//! numbers are the game's formulas, drawn where a draw decides them from a copy of the
//! stream the behaviour draws from.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, items};
use clustine_world::poi::PoiType;
use clustine_world::trading::{ExactPredicate, ItemCost, Offer, Offers};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, ItemStack, PlayerId, VillagerUse};
use uuid::Uuid;

use super::animal::{self, Animal};
use super::gossip::{GossipType, Gossips, ReputationEvent};
use super::of;
use super::villager::{self, VillagerState};
use crate::api::Omens;
use crate::living::{Kind, Living};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::Event;
use crate::mobs::brain::behaviours::social_villagers::{self as social};
use crate::mobs::brain::{Activity, Behaviour, Cx, Memories, MemoryKind, Tracker, Value};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;
use crate::region::pois::{Occupancy, PoiRecord};

const KEY: EntityId = EntityId(40);
const PARTNER: EntityId = EntityId(42);
const THIRD: EntityId = EntityId(44);
const VILLAGER: u16 = entity_types::VILLAGER as u16;
const SEED: u64 = 11;

/// Stone below y 0 and air above, with what a test puts on it: blocks, players,
/// entities, points of interest of the land, the effects players have, the game time
/// and the day.
#[derive(Default)]
struct Field {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    players: Vec<SeenPlayer>,
    entities: Vec<(Entity, Aabb)>,
    pois: Vec<PoiRecord>,
    omens: BTreeMap<EntityId, Omens>,
    time: i64,
}

impl Field {
    fn set(&mut self, pos: BlockPos, state: BlockState) {
        self.blocks.insert((pos.x, pos.y, pos.z), state);
    }

    fn put(&mut self, entity: Entity) {
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        self.entities.retain(|(other, _)| other.key != entity.key);
        self.entities.push((entity, bounds));
    }

    fn home(&mut self, pos: BlockPos) {
        self.pois.push(PoiRecord {
            pos,
            kind: PoiType::Home,
            free: 1,
            own: true,
        });
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

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }

    fn pois_in_range(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: &dyn Fn(PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<PoiRecord> {
        let most = f64::from(r) * f64::from(r);
        self.pois
            .iter()
            .filter(|record| {
                wanted(record.kind)
                    && record.dist_sqr(centre) <= most
                    && match occupancy {
                        Occupancy::HasSpace => record.own && record.free > 0,
                        Occupancy::IsOccupied => record.free != record.kind.max_tickets(),
                        Occupancy::Any => true,
                    }
            })
            .copied()
            .collect()
    }

    fn poi_type_at(&self, pos: BlockPos) -> Option<PoiType> {
        self.pois
            .iter()
            .find(|record| record.pos == pos)
            .map(|record| record.kind)
    }

    fn game_time(&self) -> i64 {
        self.time
    }

    fn player_omens(&self, key: EntityId) -> Omens {
        self.omens.get(&key).copied().unwrap_or_default()
    }
}

/// A villager of `shared` and `own`, standing and healthy.
fn villager_mob(shared: &Animal, own: &VillagerState) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.state = animal::write(shared, own);
    mob
}

/// The record of the villager `key` at `feet`, of `mob`.
fn record_of(key: EntityId, feet: Vec3, mob: &Mob) -> Entity {
    let mut entity = crate::mobs::making::fresh_record(
        key,
        VILLAGER,
        Uuid::from_u128(u128::from(key.0)),
        feet,
        0.0,
        mob,
    );
    entity.on_ground = true;
    entity
}

/// A villager's body at `feet`, on the ground.
fn living(feet: Vec3) -> Living {
    let info = EntityTypeInfo::of(entity_types::VILLAGER);
    let body = Moving {
        stands_on_lava: false,
        position: feet,
        motion: [0.0; 3],
        on_ground: true,
        fall_distance: 0.0,
        width: info.width,
        height: info.height,
        old_position: feet,
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
            kind: VILLAGER,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = crate::living::MAX_AIR;
    living
}

/// The villager a test runs behaviours of, on its field: its record (laid on the field
/// before each step), its body, its memories, and its `lived`.
struct Bench {
    field: Field,
    mob: Mob,
    feet: Vec3,
    memories: Memories,
    lived: u32,
}

impl Bench {
    fn new(field: Field, feet: Vec3, own: VillagerState) -> Self {
        let memories = Memories {
            registered: MemoryKind::ALL.iter().copied().collect(),
            ..Memories::default()
        };
        Self {
            field,
            mob: villager_mob(&Animal::default(), &own),
            feet,
            memories,
            lived: 100,
        }
    }

    /// Its stream in its next step.
    fn stream(&self) -> Random {
        Random::of_entity(SEED, KEY, self.lived)
    }

    fn own(&self) -> VillagerState {
        villager::state_of(&self.mob).1
    }

    /// `f` on its brain at the game time `time`, kept; what it put out.
    fn step(&mut self, time: i64, f: impl FnOnce(&mut Cx<'_, '_, '_>)) -> MobOutput {
        self.field.put(record_of(KEY, self.feet, &self.mob));
        self.field.time = time;
        let mut draws = self.stream();
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: VILLAGER,
            lived: self.lived,
            tick_count: self.lived,
            yaw: 0.0,
            pitch: 0.0,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: true,
            mob: self.mob.clone(),
            living: living(self.feet),
            mob_type: of(VILLAGER),
            world: &self.field,
            draws: &mut draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let mut setup = Setup::default();
        of(VILLAGER).register_goals(&mut setup);
        let config = setup.config;
        self.memories.now = self.lived;
        self.memories.lowered = Some(BTreeSet::new());
        {
            let mut ai = Ai {
                tick: &mut tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: true,
            };
            let mut cx = Cx {
                ai: &mut ai,
                memories: &mut self.memories,
                time,
            };
            f(&mut cx);
        }
        self.memories.lowered = None;
        self.mob = tick.mob;
        self.lived += 1;
        tick.out
    }

    /// One try to start `behaviour` and, where it starts, its running tick, at `time`.
    fn start(&mut self, behaviour: &mut dyn Behaviour, time: i64) -> (bool, MobOutput) {
        let mut started = false;
        let out = self.step(time, |cx| {
            started = behaviour.try_start(cx);
            if started && behaviour.running() {
                behaviour.tick_or_stop(cx);
            }
        });
        (started, out)
    }

    /// The running tick of `behaviour` at `time`.
    fn run(&mut self, behaviour: &mut dyn Behaviour, time: i64) -> MobOutput {
        self.step(time, |cx| behaviour.tick_or_stop(cx))
    }
}

/// `n` breads.
fn bread(count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(items::BREAD, count))
}

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

// ------------------------------------------------------------------------------------
// Gossip.

#[test]
fn gossip_passes_by_ten_draws_of_the_receiver_s_stream_weighted_by_value_and_weight() {
    let mut sender = Gossips::default();
    sender.add(uuid(1), GossipType::MajorNegative, 30); // |30 · −5| = 150
    sender.add(uuid(2), GossipType::Trading, 25); // 25
    sender.add(uuid(3), GossipType::MajorPositive, 20); // 100
    let mut receiver = Gossips::default();
    receiver.add(uuid(2), GossipType::Trading, 3);
    let mut draws = Random::of_entity(SEED, KEY, 7);
    let mut expected = draws.clone();
    let picked = receiver.transfer_from(&sender, &mut draws, 10);
    // The cumulative weights less one: 149, 174, 274; a draw finds the first at or
    // above it.
    let mut chosen = BTreeSet::new();
    for _ in 0..10 {
        let r = expected.next_int(275);
        chosen.insert(if r <= 149 {
            0
        } else if r <= 174 {
            1
        } else {
            2
        });
    }
    assert_eq!(draws.draws_since(&Random::of_entity(SEED, KEY, 7)), 10);
    assert_eq!(picked as usize, chosen.len());
    let mut wanted = Vec::new();
    if chosen.contains(&0) {
        // 30 less a transfer's 10.
        wanted.push((uuid(1), GossipType::MajorNegative, 20));
    }
    // 25 less 20 is 5, above the receiver's 3.
    let trading = if chosen.contains(&1) { 5 } else { 3 };
    wanted.push((uuid(2), GossipType::Trading, trading));
    // Major positive never passes on: 20 less 20 is nothing.
    assert_eq!(receiver.entries, wanted);
}

#[test]
fn gossip_from_nobody_draws_nothing_and_what_arrives_never_lowers_what_was_there() {
    let mut draws = Random::of_entity(SEED, KEY, 7);
    let start = draws.clone();
    let mut receiver = Gossips::default();
    receiver.add(uuid(1), GossipType::MinorNegative, 150);
    assert_eq!(
        receiver.transfer_from(&Gossips::default(), &mut draws, 10),
        0
    );
    assert_eq!(draws.draws_since(&start), 0);
    let mut sender = Gossips::default();
    sender.add(uuid(1), GossipType::MinorNegative, 100);
    assert_eq!(receiver.transfer_from(&sender, &mut draws, 10), 1);
    assert_eq!(
        receiver.entries,
        [(uuid(1), GossipType::MinorNegative, 150)],
        "80 arrives, below the 150 it had"
    );
}

#[test]
fn a_day_s_decay_takes_each_type_s_due_and_drops_what_falls_below_two() {
    let mut gossips = Gossips::default();
    gossips.add(uuid(1), GossipType::MajorNegative, 15);
    gossips.add(uuid(1), GossipType::MinorNegative, 21);
    gossips.add(uuid(1), GossipType::MinorPositive, 25);
    gossips.add(uuid(1), GossipType::MajorPositive, 20);
    gossips.add(uuid(2), GossipType::Trading, 3);
    gossips.decay();
    assert_eq!(
        gossips.entries,
        [
            (uuid(1), GossipType::MajorNegative, 5),
            (uuid(1), GossipType::MinorPositive, 24),
            (uuid(1), GossipType::MajorPositive, 20),
        ]
    );
}

#[test]
fn an_addition_to_a_value_above_its_most_brings_it_down_to_the_most() {
    let mut gossips = Gossips {
        entries: vec![(uuid(1), GossipType::Trading, 40)],
    };
    gossips.add(uuid(1), GossipType::Trading, 1);
    assert_eq!(gossips.entries, [(uuid(1), GossipType::Trading, 25)]);
}

#[test]
fn each_reputation_event_writes_the_game_s_gossip_and_a_golem_s_death_nothing() {
    let player = uuid(5);
    let mut own = VillagerState::default();
    assert!(!social::reputation_event(
        &mut own,
        ReputationEvent::ZombieVillagerCured,
        player
    ));
    assert_eq!(
        own.gossips.entries,
        [
            (player, GossipType::MinorPositive, 25),
            (player, GossipType::MajorPositive, 20),
        ]
    );
    let mut own = VillagerState::default();
    social::reputation_event(&mut own, ReputationEvent::Trade, player);
    assert_eq!(own.gossips.entries, [(player, GossipType::Trading, 2)]);
    let mut own = VillagerState::default();
    social::reputation_event(&mut own, ReputationEvent::VillagerHurt, player);
    assert_eq!(
        own.gossips.entries,
        [(player, GossipType::MinorNegative, 25)]
    );
    let mut own = VillagerState::default();
    social::reputation_event(&mut own, ReputationEvent::VillagerKilled, player);
    assert_eq!(
        own.gossips.entries,
        [(player, GossipType::MajorNegative, 25)]
    );
    assert_eq!(own.gossips.reputation(player), -125);
    let mut own = VillagerState::default();
    social::reputation_event(&mut own, ReputationEvent::GolemKilled, player);
    assert!(own.gossips.entries.is_empty());
    // About the player it trades with: the caller works out their prices again.
    own.trading_player = Some(PlayerId(player));
    assert!(social::reputation_event(
        &mut own,
        ReputationEvent::Trade,
        player
    ));
}

#[test]
fn a_partner_s_birth_eats_until_full_digests_twelve_and_sets_its_age() {
    let mut own = VillagerState {
        food_level: 5,
        ..VillagerState::default()
    };
    own.inventory[0] = Some(ItemStack::new(items::POTATO, 10));
    own.inventory[1] = bread(2);
    let mut mob = villager_mob(&Animal::default(), &own);
    social::born_as_partner(&mut mob, 77);
    let (shared, own) = villager::state_of(&mob);
    // Seven potatoes take it from 5 to 12, then 12 is digested.
    assert_eq!(own.inventory[0], Some(ItemStack::new(items::POTATO, 3)));
    assert_eq!(own.inventory[1], bread(2));
    assert_eq!(own.food_level, 0);
    assert_eq!(shared.age(77), 6000);
    assert_eq!(shared.age(78), 5999);
}

// ------------------------------------------------------------------------------------
// `TradeWithVillager`.

#[test]
fn trading_with_a_villager_takes_its_gossip_and_throws_bread_it_wants() {
    let mut field = Field::default();
    let mut theirs = VillagerState::default();
    theirs.gossips.add(uuid(9), GossipType::MinorNegative, 100);
    let partner_mob = villager_mob(&Animal::default(), &theirs);
    field.put(record_of(PARTNER, [1.5, 0.0, 0.5], &partner_mob));
    let mut own = VillagerState::default();
    own.inventory[0] = bread(40); // 160 points: excess
    let mut bench = Bench::new(field, [0.5, 0.0, 0.5], own);
    bench
        .memories
        .set(MemoryKind::InteractionTarget, Value::Entity(PARTNER));
    bench
        .memories
        .set(MemoryKind::VisibleMobs, Value::Entities(vec![PARTNER]));
    let mut behaviour = social::trade_with_villager();
    let mut expected = bench.stream();
    let time = 5000;
    let (started, out) = bench.start(behaviour.as_mut(), time);
    assert!(started);
    // The start's duration (`nextInt(1)`), then the gossip's ten draws of 100.
    let _ = expected.next_int(1);
    for _ in 0..10 {
        let _ = expected.next_int(100);
    }
    let own = bench.own();
    assert_eq!(
        own.gossips.entries,
        [(uuid(9), GossipType::MinorNegative, 80)]
    );
    assert_eq!(own.last_gossip_time, Some(time));
    // Half its bread (40 is above 64 / 2), thrown at the partner.
    assert_eq!(own.inventory[0], bread(20));
    let throws: Vec<&ItemStack> = out
        .local
        .iter()
        .filter_map(|local| match local {
            Local::Throw {
                stack,
                pick_up_delay: 10,
                motion,
                ..
            } => {
                assert!(motion[0] > 0.29 && motion[0] < 0.31, "{motion:?}");
                Some(stack)
            }
            _ => None,
        })
        .collect();
    assert_eq!(throws, [&ItemStack::new(items::BREAD, 20)]);
    // The partner is stamped and looks back, twice (the start's and the tick's).
    assert!(out.local.contains(&Local::Villager {
        key: PARTNER,
        used: VillagerUse::Gossiped { at: 5000 },
    }));
    let gazes = out
        .local
        .iter()
        .filter(|local| {
            matches!(
                local,
                Local::Villager {
                    key: PARTNER,
                    used: VillagerUse::Gaze {
                        to: KEY,
                        close: 2,
                        ..
                    }
                }
            )
        })
        .count();
    assert_eq!(gazes, 2);
    assert_eq!(
        bench.memories.get(MemoryKind::LookTarget),
        Some(Value::Tracker(Tracker::Entity {
            key: PARTNER,
            eyes: true,
            eyes_block: false,
        }))
    );
    // Within 1200 ticks the two gossip no more; 20 breads are no excess to throw.
    let out = bench.run(behaviour.as_mut(), time + 1);
    assert!(!out.local.iter().any(|local| matches!(
        local,
        Local::Villager {
            used: VillagerUse::Gossiped { .. },
            ..
        }
    )));
    assert!(
        !out.local
            .iter()
            .any(|local| matches!(local, Local::Throw { .. }))
    );
}

// ------------------------------------------------------------------------------------
// Love.

/// Two villagers of 12 points of bread each, of age, the second the first's breed
/// target and seen.
fn lovers(field: Field) -> Bench {
    let mut field = field;
    let mut theirs = VillagerState::default();
    theirs.inventory[0] = bread(3);
    field.put(record_of(
        PARTNER,
        [1.5, 0.0, 0.5],
        &villager_mob(&Animal::default(), &theirs),
    ));
    let mut own = VillagerState::default();
    own.inventory[0] = bread(3);
    let mut bench = Bench::new(field, [0.5, 0.0, 0.5], own);
    bench
        .memories
        .set(MemoryKind::BreedTarget, Value::Entity(PARTNER));
    bench
        .memories
        .set(MemoryKind::VisibleMobs, Value::Entities(vec![PARTNER]));
    bench
}

/// `VillagerMakeLove` started at 1000 and run to its birth; the output of the birth.
fn love_to_birth(bench: &mut Bench) -> MobOutput {
    let mut behaviour = social::villager_make_love();
    let mut stream = bench.stream();
    let (started, out) = bench.start(behaviour.as_mut(), 1000);
    assert!(started);
    // Hearts on both, the partner's first.
    assert!(out.local.contains(&Local::EntityEvent {
        key: PARTNER,
        event: 18
    }));
    assert!(out.events.contains(&Event::Entity(18)));
    // Its duration's draw, then the birth's 275 + `nextInt(50)`, its own.
    let _ = stream.next_int(1);
    let birth = 1000 + 275 + i64::from(stream.next_int(50));
    bench.run(behaviour.as_mut(), birth)
}

#[test]
fn love_with_a_free_bed_it_can_reach_makes_a_baby_given_the_bed() {
    let mut field = Field::default();
    let bed = BlockPos::new(4, 0, 0);
    field.home(bed);
    let mut bench = lovers(field);
    let out = love_to_birth(&mut bench);
    assert!(out.local.contains(&Local::TakePoi { pos: bed }));
    assert!(out.local.contains(&Local::Villager {
        key: PARTNER,
        used: VillagerUse::Born,
    }));
    let (shared, own) = villager::state_of(&bench.mob);
    assert_eq!(shared.age(bench.lived - 1), 6000);
    assert_eq!(own.inventory[0], None, "three breads eaten, then digested");
    assert_eq!(own.food_level, 0);
    let child = out
        .local
        .iter()
        .find_map(|local| match local {
            Local::MakeMob {
                kind: VILLAGER,
                mob,
                at,
            } => Some((mob, *at)),
            _ => None,
        })
        .expect("a baby");
    assert_eq!(child.1, [0.5, 0.0, 0.5], "at its parent's feet");
    assert!(animal::is_baby(child.0));
    let provider = of(VILLAGER).brain().unwrap();
    let memories = crate::mobs::brain::memories_of(provider, child.0, 1);
    assert_eq!(memories.get(MemoryKind::Home), Some(Value::Block(bed)));
}

#[test]
fn love_without_a_bed_makes_both_angry_and_no_baby() {
    let mut bench = lovers(Field::default());
    let out = love_to_birth(&mut bench);
    assert!(out.local.contains(&Local::EntityEvent {
        key: PARTNER,
        event: 13
    }));
    assert!(out.events.contains(&Event::Entity(13)));
    assert!(
        !out.local
            .iter()
            .any(|local| matches!(local, Local::MakeMob { .. } | Local::TakePoi { .. }))
    );
    // It ate and digested anyway.
    assert_eq!(bench.own().food_level, 0);
}

#[test]
fn a_villager_with_too_little_food_does_not_start_love() {
    let mut bench = lovers(Field::default());
    villager::update(&mut bench.mob, |own| own.inventory[0] = bread(2));
    let mut behaviour = social::villager_make_love();
    let (started, _) = bench.start(behaviour.as_mut(), 1000);
    assert!(!started);
}

// ------------------------------------------------------------------------------------
// Golems.

/// The villager `key` at `feet` that slept at `slept` (as it shows it).
fn sleeper(key: EntityId, feet: Vec3, slept: i64) -> Entity {
    let mut own = VillagerState::default();
    own.shown.last_slept = Some(slept);
    record_of(key, feet, &villager_mob(&Animal::default(), &own))
}

#[test]
fn three_villagers_that_slept_summon_a_golem_where_the_draws_put_it() {
    let time = 30_000;
    let mut field = Field::default();
    field.put(sleeper(PARTNER, [-3.5, 0.0, -3.5], 10_000));
    field.put(sleeper(THIRD, [-3.5, 0.0, -1.5], 20_000));
    let mut bench = Bench::new(field, [-1.5, 0.0, -3.5], VillagerState::default());
    bench
        .memories
        .set(MemoryKind::LastSlept, Value::Long(25_000));
    let mut expected = bench.stream();
    let out = bench.step(time, |cx| social::spawn_golem_if_needed(cx, time, 3));
    // The first try's column (two `randomBetweenInclusive(-8, 8)`); on the flat stone
    // the spot is on top of it, y 0, unless a villager stands in the golem's box.
    let dx = expected.next_int(17) as i32 - 8;
    let dz = expected.next_int(17) as i32 - 8;
    let at = [f64::from(-2 + dx) + 0.5, 0.0, f64::from(-4 + dz) + 0.5];
    let golem_box = Aabb::of_entity(at, 1.4, 2.7);
    let crowded = bench
        .field
        .entities
        .iter()
        .any(|(_, own)| own.intersects(golem_box));
    assert!(!crowded, "the seed's first spot is clear: {at:?}");
    let yaw = crate::mobs::ai::maths::wrap_degrees(expected.next_float() * 360.0);
    let bonus = 0.114_850_000_000_000_01 * (expected.next_double() - expected.next_double());
    let left = expected.next_float() < 0.05;
    let (golem, made_at) = out
        .local
        .iter()
        .find_map(|local| match local {
            Local::MakeMob { kind, mob, at } if i32::from(*kind) == entity_types::IRON_GOLEM => {
                Some((mob, *at))
            }
            _ => None,
        })
        .expect("a golem");
    assert_eq!(made_at, at);
    assert_eq!(golem.body.head.head_yaw, yaw);
    assert_eq!(golem.body.flags.left_handed, left);
    let follow = golem
        .body
        .attributes
        .made(clustine_data::attributes::FOLLOW_RANGE as u16)
        .unwrap();
    assert_eq!(follow.modifiers[0].amount.to_bits(), bonus.to_bits());
    assert!(
        !super::iron_golem::state_of(golem).player_created,
        "a summoned golem defends against players"
    );
    // Every villager in the box remembers it: itself, and the two by their holders.
    assert_eq!(
        bench.memories.get(MemoryKind::GolemDetectedRecently),
        Some(Value::Bool(true))
    );
    for key in [PARTNER, THIRD] {
        assert!(out.local.contains(&Local::Villager {
            key,
            used: VillagerUse::GolemDetected,
        }));
    }
}

#[test]
fn two_that_slept_and_one_that_did_not_lately_summon_nothing_and_draw_nothing() {
    let time = 30_000;
    let mut field = Field::default();
    field.put(sleeper(PARTNER, [-3.5, 0.0, -3.5], 10_000));
    // Slept more than 24000 ticks ago.
    field.put(sleeper(THIRD, [-3.5, 0.0, -1.5], 5_000));
    let mut bench = Bench::new(field, [-1.5, 0.0, -3.5], VillagerState::default());
    bench
        .memories
        .set(MemoryKind::LastSlept, Value::Long(25_000));
    let mut drawn = 0;
    let out = bench.step(time, |cx| {
        let before = cx.ai.tick.draws.clone();
        social::spawn_golem_if_needed(cx, time, 3);
        drawn = cx.ai.tick.draws.draws_since(&before);
    });
    assert_eq!(drawn, 0);
    assert!(out.local.is_empty(), "{:?}", out.local);
    // And a villager that saw a golem lately wants none.
    let mut bench = Bench::new(Field::default(), [0.5, 0.0, 0.5], VillagerState::default());
    bench
        .memories
        .set(MemoryKind::LastSlept, Value::Long(25_000));
    bench
        .memories
        .set_with_expiry(MemoryKind::GolemDetectedRecently, Value::Bool(true), 599);
    let out = bench.step(time, |cx| social::spawn_golem_if_needed(cx, time, 1));
    assert!(out.local.is_empty());
}

// ------------------------------------------------------------------------------------
// Bells and hiding.

#[test]
fn a_villager_that_heard_a_bell_with_no_raid_asks_to_hide() {
    let mut bench = Bench::new(Field::default(), [0.5, 0.0, 0.5], VillagerState::default());
    let mut behaviour = social::react_to_bell();
    let mut requested = Vec::new();
    bench.step(10, |cx| {
        assert!(!behaviour.try_start(cx), "nothing heard");
        cx.memories.set(MemoryKind::HeardBellTime, Value::Long(5));
        assert!(behaviour.try_start(cx));
        requested = cx.memories.requested.clone();
    });
    assert_eq!(requested, [Activity::Hide]);
}

#[test]
fn a_hiding_place_is_the_first_home_within_reach_else_one_at_random_by_its_own_stream() {
    // A home a block off: within its close distance of 2, so no walk.
    let mut field = Field::default();
    field.home(BlockPos::new(1, 0, 0));
    let mut bench = Bench::new(field, [0.5, 0.0, 0.5], VillagerState::default());
    bench
        .memories
        .set(MemoryKind::BreedTarget, Value::Entity(PARTNER));
    let mut behaviour = social::locate_hiding_place(32, 0.625, 2);
    let mut drawn = 0;
    bench.step(10, |cx| {
        let before = cx.ai.tick.draws.clone();
        assert!(behaviour.try_start(cx));
        drawn = cx.ai.tick.draws.draws_since(&before);
    });
    assert_eq!(drawn, 0);
    assert_eq!(
        bench.memories.get(MemoryKind::HidingPlace),
        Some(Value::Block(BlockPos::new(1, 0, 0)))
    );
    assert_eq!(bench.memories.get(MemoryKind::BreedTarget), None);
    assert_eq!(bench.memories.get(MemoryKind::WalkTarget), None);
    // Three homes far off: shuffled by two of its own draws, the first walked to.
    let homes = [
        BlockPos::new(10, 0, 0),
        BlockPos::new(0, 0, 12),
        BlockPos::new(-9, 0, -9),
    ];
    let mut field = Field::default();
    for home in homes {
        field.home(home);
    }
    let mut bench = Bench::new(field, [0.5, 0.0, 0.5], VillagerState::default());
    let mut expected = bench.stream();
    let mut order = homes.to_vec();
    for i in (2..=order.len()).rev() {
        let j = expected.next_int(i as u32) as usize;
        order.swap(i - 1, j);
    }
    let mut behaviour = social::locate_hiding_place(32, 0.625, 2);
    bench.step(10, |cx| assert!(behaviour.try_start(cx)));
    assert_eq!(
        bench.memories.get(MemoryKind::HidingPlace),
        Some(Value::Block(order[0]))
    );
    assert_eq!(
        bench.memories.get(MemoryKind::WalkTarget),
        Some(Value::Walk {
            to: Tracker::Block(order[0]),
            speed: 0.625,
            close_enough: 2,
        })
    );
}

#[test]
fn hidden_it_counts_its_ticks_at_the_place_and_comes_out_after_the_bell_s_300() {
    let mut bench = Bench::new(Field::default(), [0.5, 0.0, 0.5], VillagerState::default());
    let mut behaviour = social::set_hidden_state(15, 3);
    bench.memories.set(
        MemoryKind::HidingPlace,
        Value::Block(BlockPos::new(0, 0, 0)),
    );
    bench
        .memories
        .set(MemoryKind::HeardBellTime, Value::Long(100));
    for time in 101..400 {
        bench.step(time, |cx| assert!(behaviour.try_start(cx)));
        assert!(
            bench.memories.get(MemoryKind::HidingPlace).is_some(),
            "still hidden at {time}"
        );
    }
    // At 400, 300 after the bell, it comes out.
    bench.step(400, |cx| assert!(behaviour.try_start(cx)));
    assert_eq!(bench.memories.get(MemoryKind::HidingPlace), None);
    assert_eq!(bench.memories.get(MemoryKind::HeardBellTime), None);
}

#[test]
fn hidden_more_than_its_seconds_at_the_place_it_comes_out_before_the_bell_s_time() {
    let mut bench = Bench::new(Field::default(), [0.5, 0.0, 0.5], VillagerState::default());
    // One second: 20 ticks.
    let mut behaviour = social::set_hidden_state(1, 3);
    bench.memories.set(
        MemoryKind::HidingPlace,
        Value::Block(BlockPos::new(0, 0, 0)),
    );
    bench
        .memories
        .set(MemoryKind::HeardBellTime, Value::Long(100));
    // 21 ticks counted (to 21, above 20), then the next comes out.
    for time in 101..=121 {
        bench.step(time, |cx| assert!(behaviour.try_start(cx)));
    }
    assert!(bench.memories.get(MemoryKind::HidingPlace).is_some());
    bench.step(122, |cx| assert!(behaviour.try_start(cx)));
    assert_eq!(bench.memories.get(MemoryKind::HidingPlace), None);
}

// ------------------------------------------------------------------------------------
// Players.

/// A player at `feet` holding `held`.
fn player_at(key: u64, feet: Vec3, held: Option<i32>) -> SeenPlayer {
    SeenPlayer {
        key: EntityId(key),
        uuid: u128::from(key),
        position: feet,
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

fn offer(cost: i32, result: i32) -> Offer {
    Offer::new(
        ItemCost {
            item: cost,
            count: 1,
            components: ExactPredicate::of(Vec::new()),
        },
        None,
        ItemStack::new(result, 1),
        12,
        1,
        0.05,
    )
}

#[test]
fn a_player_holding_an_offer_s_cost_is_shown_its_result_cycling_every_forty_ticks() {
    let mut field = Field::default();
    let player = EntityId(500);
    field
        .players
        .push(player_at(500, [2.5, 0.0, 0.5], Some(items::EMERALD)));
    let mut own = VillagerState::default();
    let mut sold_out = offer(items::EMERALD, items::BELL);
    sold_out.uses = 12;
    own.offers = Some(Offers(vec![
        offer(items::EMERALD, items::BREAD),
        offer(items::WHEAT, items::EMERALD),
        sold_out,
        offer(items::EMERALD, items::APPLE),
    ]));
    let mut bench = Bench::new(field, [0.5, 0.0, 0.5], own);
    bench
        .memories
        .set(MemoryKind::InteractionTarget, Value::Entity(player));
    let mut behaviour = social::show_trades_to_player(400, 1600);
    let main_hand = |bench: &Bench| {
        bench.mob.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)].clone()
    };
    let (started, _) = bench.start(behaviour.as_mut(), 10);
    assert!(started);
    assert_eq!(main_hand(&bench), Some(ItemStack::new(items::BREAD, 1)));
    // It holds the first for forty ticks of cycling (its first tick the first), then
    // the other in stock.
    for time in 11..49 {
        bench.run(behaviour.as_mut(), time);
        assert_eq!(
            main_hand(&bench),
            Some(ItemStack::new(items::BREAD, 1)),
            "{time}"
        );
    }
    bench.run(behaviour.as_mut(), 49);
    assert_eq!(main_hand(&bench), Some(ItemStack::new(items::APPLE, 1)));
    assert_eq!(
        bench
            .mob
            .body
            .equipment
            .drop_chance(usize::from(crate::living::slots::MAIN_HAND)),
        0.0,
        "what it shows never drops"
    );
    // The player gone, it stops: its hand empty, its interaction target forgotten.
    bench.field.players.clear();
    bench.run(behaviour.as_mut(), 51);
    assert!(!behaviour.running());
    assert_eq!(main_hand(&bench), None);
    assert_eq!(bench.memories.get(MemoryKind::InteractionTarget), None);
}

#[test]
fn a_hero_seen_is_given_a_gift_of_its_profession_s_table_after_the_wait() {
    let mut field = Field::default();
    let hero = EntityId(500);
    field.players.push(player_at(500, [2.5, 0.0, 0.5], None));
    field.omens.insert(
        hero,
        Omens {
            hero: Some(0),
            ..Omens::default()
        },
    );
    let mut own = VillagerState::default();
    own.data.profession = clustine_data::villagers::professions::ARMORER as u8;
    let mut bench = Bench::new(field, [0.5, 0.0, 0.5], own);
    bench
        .memories
        .set(MemoryKind::NearestVisiblePlayer, Value::Entity(hero));
    let mut behaviour = social::give_gift_to_hero(100);
    // Its first wait of 600 counts down only while the hero is seen.
    for time in 0..600 {
        let (started, _) = bench.start(behaviour.as_mut(), time);
        assert!(!started, "{time}");
    }
    let (started, _) = bench.start(behaviour.as_mut(), 600);
    assert!(started);
    assert_eq!(
        bench.memories.get(MemoryKind::InteractionTarget),
        Some(Value::Entity(hero))
    );
    // Within 5 and twenty ticks after its start, it throws one piece of chainmail.
    let mut thrown = Vec::new();
    for time in 601..=621 {
        let out = bench.run(behaviour.as_mut(), time);
        thrown.extend(out.local.into_iter().filter_map(|local| match local {
            Local::Throw { stack, .. } => Some(stack),
            _ => None,
        }));
    }
    assert_eq!(thrown.len(), 1, "{thrown:?}");
    assert!(
        [
            items::CHAINMAIL_HELMET,
            items::CHAINMAIL_CHESTPLATE,
            items::CHAINMAIL_LEGGINGS,
            items::CHAINMAIL_BOOTS
        ]
        .contains(&thrown[0].item),
        "{thrown:?}"
    );
    // Given, it stops: the next wait is drawn, 600 to 6600.
    let mut expected = bench.stream();
    bench.run(behaviour.as_mut(), 622);
    assert!(!behaviour.running());
    let wait = 600 + expected.next_int(6001) as i64;
    for time in 623..623 + wait {
        let (started, _) = bench.start(behaviour.as_mut(), time);
        assert!(!started, "{time}");
    }
    let (started, _) = bench.start(behaviour.as_mut(), 623 + wait);
    assert!(started);
}

// ------------------------------------------------------------------------------------
// A murder.

#[test]
fn a_murder_s_witnesses_are_the_villagers_it_saw_that_see_it_now() {
    let mut field = Field::default();
    let dying = record_of(
        KEY,
        [0.5, 0.0, 0.5],
        &villager_mob(&Animal::default(), &VillagerState::default()),
    );
    field.put(dying.clone());
    let plain = villager_mob(&Animal::default(), &VillagerState::default());
    field.put(record_of(PARTNER, [4.5, 0.0, 0.5], &plain));
    // Behind a wall of stone.
    field.put(record_of(THIRD, [0.5, 0.0, 6.5], &plain));
    for x in -3..=3 {
        for y in 0..=3 {
            field.set(BlockPos::new(x, y, 3), blocks::STONE);
        }
    }
    let far = EntityId(46);
    field.put(record_of(far, [60.5, 0.0, 0.5], &plain));
    let visible = [PARTNER, THIRD, far, EntityId(999)];
    assert_eq!(social::witnesses(&field, &dying, &visible), [PARTNER]);
}

#[test]
fn a_dying_villager_gives_back_the_points_it_held_that_are_still_there() {
    let mut field = Field::default();
    let home = BlockPos::new(3, 0, 0);
    let meeting = BlockPos::new(5, 0, 0);
    let gone = BlockPos::new(7, 0, 0);
    field.home(home);
    field.pois.push(PoiRecord {
        pos: meeting,
        kind: PoiType::Meeting,
        free: 31,
        own: true,
    });
    let mut mob = villager_mob(&Animal::default(), &VillagerState::default());
    let provider = of(VILLAGER).brain().unwrap();
    let mut memories = crate::mobs::brain::memories_of(provider, &mob, 0);
    memories
        .registered
        .extend([MemoryKind::MeetingPoint, MemoryKind::JobSite]);
    memories.set(MemoryKind::Home, Value::Block(home));
    memories.set(MemoryKind::MeetingPoint, Value::Block(meeting));
    memories.set(MemoryKind::JobSite, Value::Block(gone));
    crate::mobs::brain::write_memories(provider, &mut mob, memories);
    let dying = record_of(KEY, [0.5, 0.0, 0.5], &mob);
    let released = social::points_to_release(&field, &dying);
    // Its home always; the meeting point and the job site where the provider registers
    // them (part V1's), and the job site not where no point is any more.
    assert!(released.contains(&(home, PoiType::Home)), "{released:?}");
    assert!(!released.iter().any(|(pos, _)| *pos == gone));
}
