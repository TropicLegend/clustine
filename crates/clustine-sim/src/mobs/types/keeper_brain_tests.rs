//! The keepers' brains from the game's rules (`docs/groundwork/n7-brief.md`, sections
//! 7.12, 7.13 and 10.7.5, and the bytecode): each type's memories, sensors and
//! activities as its provider names them, its activities' switching, the nautilus's
//! search for a fight and its charge with their draws in order, what angers it, and the
//! copper golem's sorting at a chest, each on a small world of its own, the brain ticked
//! at its type's `customServerAiStep`.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{
    BlockState, EntityTypeInfo, attributes as ids, blocks, entity_types, items, sounds,
};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, ItemStack};

use super::animal::{self, Animal};
use super::golem_transport::{self, Reached, Transporting, Way};
use super::nautilus::{self, NautilusState};
use super::tamable::Tame;
use super::{copper_golem, of, zombie_nautilus};
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::body::Event;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::body::hurt::{self, Causer, DamageSource, DamageType, Struck};
use crate::mobs::brain::{
    self, Activity, Brain, Memories, MemoryKind, Provider, Sensed, Status, Tracker, Value,
};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::{Random, golem_chests};

const NAUTILUS: u16 = entity_types::NAUTILUS as u16;
const ZOMBIE: u16 = entity_types::ZOMBIE_NAUTILUS as u16;
const GOLEM: u16 = entity_types::COPPER_GOLEM as u16;
const PUFFERFISH: u16 = entity_types::PUFFERFISH as u16;
const COD: u16 = entity_types::COD as u16;

/// The key of the mob the tests tick.
const KEY: EntityId = EntityId(40);

// ------------------------------------------------------------------------------------
// A world of its own.

/// Stone below y = 0 and air above, with what a test puts on it: blocks, chests' data,
/// players and entities.
#[derive(Default)]
struct Place {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    block_entities: BTreeMap<(i32, i32, i32), Vec<u8>>,
    players: Vec<SeenPlayer>,
    entities: Vec<(Entity, Aabb)>,
}

impl Place {
    /// Water from x and z of -10 to 10 and y of 0 to 5.
    fn pool() -> Self {
        let mut place = Self::default();
        for x in -10..=10 {
            for z in -10..=10 {
                for y in 0..=5 {
                    place.set(BlockPos::new(x, y, z), blocks::WATER);
                }
            }
        }
        place
    }

    fn set(&mut self, pos: BlockPos, state: BlockState) {
        self.blocks.insert((pos.x, pos.y, pos.z), state);
    }

    /// A chest of `state` at `pos` holding `slots`, with `openers`.
    fn chest(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        slots: &[Option<ItemStack>],
        openers: u32,
    ) {
        self.set(pos, state);
        let mut all = slots.to_vec();
        all.resize(27, None);
        self.block_entities.insert(
            (pos.x, pos.y, pos.z),
            golem_chests::chest_data(state, &all, openers),
        );
    }

    /// `entity` laid on the place, replacing what had its key.
    fn put(&mut self, entity: Entity) {
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        self.entities.retain(|(other, _)| other.key != entity.key);
        self.entities.push((entity, bounds));
    }
}

impl World for Place {
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

impl Holds for Place {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Place {
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

    fn block_entity_data(&self, pos: BlockPos) -> Option<&[u8]> {
        self.block_entities
            .get(&(pos.x, pos.y, pos.z))
            .map(Vec::as_slice)
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }
}

/// A creative player at `position` holding `held`.
fn player(key: u64, position: Vec3, held: Option<i32>) -> SeenPlayer {
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

/// A mob of the type `kind` at its full health with the type state `state`.
fn healthy(kind: u16, state: clustine_world::TypeState) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = supplied(kind, attribute(ids::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = state;
    mob
}

/// The record of a mob of the type `kind` at `position`.
fn record(key: EntityId, kind: u16, position: Vec3, mob: &Mob) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

/// A pufferfish of the place's at `position`.
fn pufferfish(key: u64, position: Vec3) -> Entity {
    record(
        EntityId(key),
        PUFFERFISH,
        position,
        &healthy(PUFFERFISH, Default::default()),
    )
}

/// The body of a mob of the type `kind` at `position`, its fluids as `place` has them.
fn body_at(place: &Place, kind: u16, position: Vec3) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let body = Body {
        stands_on_lava: false,
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
            kind,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living.fluids = crate::physics::fluids::update(place, &living.body.bounds());
    living.body.in_water = living.fluids.in_water();
    living
}

/// A keeper ticked on a place, its record and its region's sensed memories carried
/// between ticks as a region would; it does not move (no travel), so that what its
/// brain does is all there is.
struct Keeper {
    kind: u16,
    mob: Mob,
    living: Living,
    sensed: Option<Sensed>,
    lived: u32,
    yaw: f32,
    seed: u64,
    /// How many draws its last tick made.
    drew: u64,
}

impl Keeper {
    fn new(place: &Place, kind: u16, mob: Mob, at: Vec3) -> Self {
        Self {
            kind,
            mob,
            living: body_at(place, kind, at),
            sensed: Some(Sensed::new()),
            lived: 0,
            yaw: 0.0,
            seed: 7,
            drew: 0,
        }
    }

    /// Its stream in its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(self.seed, KEY, self.lived)
    }

    /// One `customServerAiStep` (its brain's tick and its activity's update, and its
    /// type's own after): what it put out.
    fn tick(&mut self, place: &Place) -> MobOutput {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = MobTick {
            sensed: self.sensed.take(),
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: self.mob.clone(),
            living: self.living.clone(),
            yaw: self.yaw,
            pitch: 0.0,
            mob_type: of(self.kind),
            world: place,
            draws: &mut draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        of(self.kind).custom_server_ai_step(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.sensed = tick.sensed;
        self.lived += 1;
        let out = tick.out;
        self.drew = draws.draws_since(&start);
        out
    }

    fn provider(&self) -> &'static Provider {
        of(self.kind).brain().expect("a keeper has a brain")
    }

    /// Its memories as code between its ticks reads them.
    fn memories(&self) -> Memories {
        brain::memories_of(self.provider(), &self.mob, self.lived)
    }

    /// `change` made to its memories between its ticks.
    fn remember(&mut self, change: impl FnOnce(&mut Memories)) {
        let provider = self.provider();
        let mut memories = brain::memories_of(provider, &self.mob, self.lived);
        change(&mut memories);
        brain::write_memories(provider, &mut self.mob, memories);
    }

    /// Its active activity beside its core.
    fn activity(&self) -> Option<Activity> {
        Brain::read(
            self.provider(),
            &self.mob.mind.brain,
            Some(&Sensed::new()),
            self.lived,
        )
        .active_non_core()
    }
}

/// The memories a brain of `provider` has registered.
fn registered(provider: &'static Provider) -> BTreeSet<MemoryKind> {
    Brain::read(provider, &[], Some(&Sensed::new()), 0)
        .memories
        .registered
}

// ------------------------------------------------------------------------------------
// The nautiluses.

/// The memories the nautilus's provider names, with its sensors' and its behaviours'
/// (`Nautilus.BRAIN_PROVIDER`; `AnimalPanic`, `LookAtTargetSink`, `MoveToTargetSink`,
/// `CountDownCooldownTicks`, `AnimalMakeLove`, `FollowTemptation`, `StartAttacking`,
/// the gate of `RandomStroll.swim` and `SetWalkTargetFromLookTarget`, `ChargeAttack`).
const NAUTILUS_MEMORIES: [MemoryKind; 21] = [
    MemoryKind::AngryAt,
    MemoryKind::AttackTargetCooldown,
    MemoryKind::Mobs,
    MemoryKind::VisibleMobs,
    MemoryKind::NearestVisibleAdult,
    MemoryKind::NearestPlayers,
    MemoryKind::NearestVisiblePlayer,
    MemoryKind::NearestVisibleTargetablePlayer,
    MemoryKind::NearestVisibleTargetablePlayers,
    MemoryKind::HurtBy,
    MemoryKind::HurtByEntity,
    MemoryKind::TemptingPlayer,
    MemoryKind::IsPanicking,
    MemoryKind::LookTarget,
    MemoryKind::WalkTarget,
    MemoryKind::CantReachWalkTargetSince,
    MemoryKind::Path,
    MemoryKind::TemptationCooldownTicks,
    MemoryKind::ChargeCooldownTicks,
    MemoryKind::BreedTarget,
    MemoryKind::AttackTarget,
];

#[test]
fn the_nautiluses_register_what_their_providers_sensors_and_behaviours_name() {
    let expected: BTreeSet<MemoryKind> = NAUTILUS_MEMORIES.into_iter().collect();
    assert_eq!(registered(&nautilus::PROVIDER), expected);
    // The zombie lacks the panic and the breeding, whose memories the temptation names.
    assert_eq!(registered(&zombie_nautilus::PROVIDER), expected);
    // The three-argument provider: the anger and the cooldown are registered first.
    assert_eq!(
        nautilus::PROVIDER.memories,
        [MemoryKind::AngryAt, MemoryKind::AttackTargetCooldown]
    );
    assert_eq!(nautilus::PROVIDER.sensors.len(), 5);
    assert_eq!(nautilus::PROVIDER.core, [Activity::Core]);
    assert_eq!(nautilus::PROVIDER.default, Activity::Idle);
}

/// Each activity's behaviours by their priorities, and its conditions.
fn shape(provider: &'static Provider) -> Vec<(Activity, Vec<i32>, usize)> {
    (provider.activities)()
        .into_iter()
        .map(|data| {
            (
                data.activity,
                data.behaviours
                    .iter()
                    .map(|(priority, _)| *priority)
                    .collect(),
                data.conditions.len(),
            )
        })
        .collect()
}

#[test]
fn the_nautilus_s_activities_are_core_idle_and_fight_by_the_game_s_priorities() {
    assert_eq!(
        shape(&nautilus::PROVIDER),
        [
            (Activity::Core, vec![0, 1, 2, 3, 4, 5], 0),
            (Activity::Idle, vec![1, 2, 3, 4], 0),
            (Activity::Fight, vec![0], 4),
        ]
    );
    // The zombie's core has no panic and its idle no breeding.
    assert_eq!(
        shape(&zombie_nautilus::PROVIDER),
        [
            (Activity::Core, vec![0, 1, 2, 3, 4], 0),
            (Activity::Idle, vec![1, 2, 3], 0),
            (Activity::Fight, vec![0], 4),
        ]
    );
    let fight = (nautilus::PROVIDER.activities)().remove(2);
    assert_eq!(
        fight.conditions,
        [
            (MemoryKind::AttackTarget, Status::Present),
            (MemoryKind::TemptingPlayer, Status::Absent),
            (MemoryKind::BreedTarget, Status::Absent),
            (MemoryKind::ChargeCooldownTicks, Status::Absent),
        ]
    );
    assert!(fight.erase.is_empty());
}

/// A wild adult nautilus in the pool at its middle.
fn wild_in_pool(place: &Place, kind: u16) -> Keeper {
    let mob = healthy(
        kind,
        animal::write(&Animal::default(), &NautilusState::default()),
    );
    Keeper::new(place, kind, mob, [0.5, 1.0, 0.5])
}

#[test]
fn a_wild_adult_in_water_draws_its_attack_cooldown_then_a_coin_and_on_heads_takes_a_pufferfish() {
    let mut heads = 0;
    let mut tails = 0;
    for seed in 0..40 {
        let mut place = Place::pool();
        place.put(pufferfish(9, [3.5, 1.0, 0.5]));
        let mut keeper = wild_in_pool(&place, NAUTILUS);
        keeper.seed = seed;
        // The first draws of its brain's first tick are `StartAttacking`'s finder's: no
        // behaviour before it at priorities 0 to 3 starts.
        let mut replay = keeper.stream();
        let cooldown = 2400 + replay.next_int(1201) as i32;
        let heads_up = replay.next_float() >= 0.5;
        keeper.tick(&place);
        let memories = keeper.memories();
        // Its core began counting it down in the same tick.
        assert_eq!(
            memories.int(MemoryKind::AttackTargetCooldown),
            Some(cooldown - 1)
        );
        if heads_up {
            heads += 1;
            assert_eq!(
                memories.get(MemoryKind::AttackTarget),
                Some(Value::Entity(EntityId(9)))
            );
            assert_eq!(keeper.activity(), Some(Activity::Fight));
        } else {
            tails += 1;
            assert!(!memories.check(MemoryKind::AttackTarget, Status::Present));
            assert_eq!(keeper.activity(), Some(Activity::Idle));
        }
    }
    assert!(heads > 0 && tails > 0, "{heads} {tails}");
}

#[test]
fn the_search_for_a_fight_draws_nothing_while_its_cooldown_runs_and_takes_no_cod() {
    let mut place = Place::pool();
    place.put(record(
        EntityId(9),
        COD,
        [3.5, 1.0, 0.5],
        &healthy(COD, Default::default()),
    ));
    let mut keeper = wild_in_pool(&place, NAUTILUS);
    for _ in 0..30 {
        keeper.tick(&place);
        assert!(
            !keeper
                .memories()
                .check(MemoryKind::AttackTarget, Status::Present)
        );
    }
    // One cooldown drawn in the first tick, counted down since.
    let cooldown = keeper
        .memories()
        .int(MemoryKind::AttackTargetCooldown)
        .unwrap();
    let mut replay = Random::of_entity(7, KEY, 0);
    assert_eq!(cooldown, 2400 + replay.next_int(1201) as i32 - 30);
}

#[test]
fn a_baby_a_tamed_one_and_one_out_of_water_pick_no_fight() {
    let check = |keeper: &mut Keeper, place: &Place| {
        for _ in 0..3 {
            keeper.tick(place);
        }
        let memories = keeper.memories();
        assert!(!memories.check(MemoryKind::AttackTargetCooldown, Status::Present));
        assert!(!memories.check(MemoryKind::AttackTarget, Status::Present));
    };
    let mut place = Place::pool();
    place.put(pufferfish(9, [3.5, 1.0, 0.5]));
    let baby = Animal {
        age: animal::BABY_START_AGE,
        ..Animal::default()
    };
    let mut keeper = Keeper::new(
        &place,
        NAUTILUS,
        healthy(NAUTILUS, animal::write(&baby, &NautilusState::default())),
        [0.5, 1.0, 0.5],
    );
    check(&mut keeper, &place);
    let mut tame = Tame::default();
    tame.set_tame_flag(true);
    let tamed = NautilusState {
        tame,
        ..NautilusState::default()
    };
    let mut keeper = Keeper::new(
        &place,
        NAUTILUS,
        healthy(NAUTILUS, animal::write(&Animal::default(), &tamed)),
        [0.5, 1.0, 0.5],
    );
    check(&mut keeper, &place);
    let dry = Place::default();
    let mut keeper = wild_in_pool(&dry, NAUTILUS);
    check(&mut keeper, &dry);
}

/// A keeper's memories with `kind` set to the entity `key`, between its ticks.
fn targeting(keeper: &mut Keeper, key: EntityId) {
    keeper.remember(|memories| {
        memories.set(MemoryKind::AttackTarget, Value::Entity(key));
    });
}

/// The sounds a tick made.
fn sounds_of(out: &MobOutput) -> Vec<i32> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

/// The hurts a tick made.
fn hurts(out: &MobOutput) -> Vec<(EntityId, f32, Option<[f64; 3]>)> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::Hurt {
                key,
                amount,
                knockback,
                fling: None,
                ..
            } => Some((*key, *amount, *knockback)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_charge_dashes_at_its_target_with_its_sound_and_hits_what_its_box_meets() {
    for (kind, speed, dash) in [
        (NAUTILUS, 0.6_f32, sounds::ENTITY_NAUTILUS_DASH),
        (ZOMBIE, 0.5_f32, sounds::ENTITY_ZOMBIE_NAUTILUS_DASH),
    ] {
        let mut place = Place::pool();
        // Its target, 5 blocks east; in its way, a pufferfish where its box meets it.
        place.put(pufferfish(9, [5.5, 1.0, 0.5]));
        place.put(pufferfish(10, [0.8, 1.0, 0.5]));
        let mut keeper = wild_in_pool(&place, kind);
        keeper.remember(|memories| {
            memories.set(MemoryKind::AttackTargetCooldown, Value::Int(3000));
        });
        targeting(&mut keeper, EntityId(9));
        // Its first tick finds it with an attack target: the fight is its activity next.
        keeper.tick(&place);
        assert_eq!(keeper.activity(), Some(Activity::Fight));
        let mut replay = keeper.stream();
        let out = keeper.tick(&place);
        // The charge's start draws its duration first (`nextInt(1)`, at priority 0),
        // then the walk its first tick's swim chose starts (`MoveToTargetSink`'s 150 to
        // 250, at priority 2): two draws, in that order.
        assert_eq!(keeper.drew, 2, "{kind}");
        assert_eq!(replay.next_int(1), 0);
        let walk = replay.next_int(101);
        assert!(walk <= 100);
        // The dash's sound as it starts, its velocity towards the target at its speed.
        assert_eq!(sounds_of(&out), [dash], "{kind}");
        // It hit the pufferfish in its way, not its target: its attack damage, and a
        // push of `clamp(speed × movement speed, 0.2, 2) × 2` from where it faces.
        let movement = supplied(kind, attribute(ids::MOVEMENT_SPEED)).unwrap() as f32;
        let strength = (speed * movement).clamp(0.2, 2.0) * 2.0;
        let yaw = keeper.yaw * 0.017_453_292;
        let push = [
            f64::from(strength),
            f64::from(crate::mobs::ai::maths::table_sin(yaw)),
            f64::from(-crate::mobs::ai::maths::table_cos(yaw)),
        ];
        assert_eq!(hurts(&out), [(EntityId(10), 3.0, Some(push))], "{kind}");
        // Facing its target east: a yaw of -90.
        assert!((keeper.yaw + 90.0).abs() < 1.0e-3, "{}", keeper.yaw);
        // Its motion: the charge's velocity, slowed across by the push's own rule.
        let motion = keeper.living.body.motion;
        assert!(
            (motion[0] - f64::from(speed) * 0.6).abs() < 1.0e-6,
            "{motion:?}"
        );
        // Stopped at once: its cooldown of 80, its target forgotten, and idle again.
        let memories = keeper.memories();
        assert_eq!(memories.int(MemoryKind::ChargeCooldownTicks), Some(80));
        assert!(!memories.check(MemoryKind::AttackTarget, Status::Present));
        assert_eq!(keeper.activity(), Some(Activity::Idle));
        // Its stop again at its next running pass sets the cooldown again, which its
        // core then counts down.
        let out = keeper.tick(&place);
        assert!(hurts(&out).is_empty());
        assert_eq!(
            keeper.memories().int(MemoryKind::ChargeCooldownTicks),
            Some(79)
        );
    }
}

#[test]
fn a_charge_at_a_target_beyond_eleven_ends_at_once_silent_with_its_cooldown() {
    let mut place = Place::pool();
    place.put(pufferfish(9, [9.5, 1.0, 9.5]));
    let mut keeper = wild_in_pool(&place, NAUTILUS);
    keeper.remember(|memories| {
        memories.set(MemoryKind::AttackTargetCooldown, Value::Int(3000));
    });
    targeting(&mut keeper, EntityId(9));
    keeper.tick(&place);
    assert_eq!(keeper.activity(), Some(Activity::Fight));
    let out = keeper.tick(&place);
    assert!(sounds_of(&out).is_empty());
    assert!(hurts(&out).is_empty());
    let memories = keeper.memories();
    assert_eq!(memories.int(MemoryKind::ChargeCooldownTicks), Some(80));
    assert!(!memories.check(MemoryKind::AttackTarget, Status::Present));
}

#[test]
fn an_attack_target_the_view_has_not_is_beyond_the_charge_and_ends_it() {
    let place = Place::pool();
    let mut keeper = wild_in_pool(&place, NAUTILUS);
    keeper.remember(|memories| {
        memories.set(MemoryKind::AttackTargetCooldown, Value::Int(3000));
    });
    targeting(&mut keeper, EntityId(77));
    keeper.tick(&place);
    let out = keeper.tick(&place);
    assert!(sounds_of(&out).is_empty());
    assert_eq!(
        keeper.memories().int(MemoryKind::ChargeCooldownTicks),
        Some(80)
    );
}

#[test]
fn a_tempting_player_or_a_breed_target_keeps_it_from_its_fight() {
    let mut place = Place::pool();
    place.put(pufferfish(9, [3.5, 1.0, 0.5]));
    place
        .players
        .push(player(2, [2.5, 1.0, 2.5], Some(items::COD)));
    let mut keeper = wild_in_pool(&place, NAUTILUS);
    keeper.remember(|memories| {
        memories.set(MemoryKind::AttackTargetCooldown, Value::Int(3000));
    });
    targeting(&mut keeper, EntityId(9));
    keeper.tick(&place);
    // Sensed, so kept beside its record.
    let sensed = keeper.sensed.as_ref().expect("sensed");
    assert!(sensed.contains_key(&MemoryKind::TemptingPlayer));
    assert_eq!(keeper.activity(), Some(Activity::Idle));
}

/// A hurt of 1 by `source` of `mob` (of the type `kind`) at its feet `at`, between its
/// ticks at the moment `moment`; whether it took.
fn hurt_between_ticks(
    kind: u16,
    mob: &mut Mob,
    at: Vec3,
    source: &DamageSource,
    moment: u32,
) -> bool {
    let mut stream = Random::of_entity(3, KEY, moment);
    let mut out = MobOutput::default();
    let mut motion = [0.0; 3];
    let mut struck = Struck {
        kind,
        mob,
        mob_type: of(kind),
        position: at,
        motion: &mut motion,
        on_ground: false,
        on_fire: false,
        vehicle: None,
        carries: false,
        leashed: false,
        in_liquid: true,
        moment,
        game_time: 0,
        draws: &mut stream,
        out: &mut out,
    };
    hurt::hurt_server(&mut struck, source, 1.0)
}

fn by(kind: u16, key: u64, position: Vec3, player: Option<u128>) -> DamageSource {
    DamageSource::by(
        if player.is_some() {
            DamageType::PLAYER_ATTACK
        } else {
            DamageType::MOB_ATTACK
        },
        Causer {
            key: EntityId(key),
            kind,
            position,
            player,
            living: true,
        },
    )
}

#[test]
fn a_hurt_angers_a_nautilus_for_400_ticks_at_its_next_tick_but_not_by_a_creative_player() {
    for kind in [NAUTILUS, ZOMBIE] {
        let mut place = Place::pool();
        place.put(pufferfish(9, [3.5, 1.0, 0.5]));
        let mut survival = player(2, [2.5, 1.0, 2.5], None);
        survival.creative = false;
        place.players.push(survival);
        place.players.push(player(3, [-2.5, 1.0, 2.5], None));
        place.put(pufferfish(11, [-15.5, 1.0, 9.5]));
        let angry = |source: &DamageSource| {
            let mut keeper = wild_in_pool(&place, kind);
            keeper.remember(|memories| {
                memories.set(MemoryKind::AttackTargetCooldown, Value::Int(3000));
                memories.set(MemoryKind::CantReachWalkTargetSince, Value::Long(3));
            });
            keeper.tick(&place);
            let at = keeper.living.body.position;
            let moment = keeper.lived;
            assert!(hurt_between_ticks(
                kind,
                &mut keeper.mob,
                at,
                source,
                moment
            ));
            // Kept for its next tick, which reads the world.
            let pending = nautilus::state_of(&keeper.mob).1.angered_by;
            assert_eq!(pending, source.cause.as_ref().map(|cause| cause.key));
            keeper.tick(&place);
            assert_eq!(nautilus::state_of(&keeper.mob).1.angered_by, None);
            let memories = keeper.memories();
            let anger = memories.get(MemoryKind::AngryAt);
            if anger.is_some() {
                // Set as between its ticks: 400, less its forget pass of this tick.
                assert_eq!(memories.expires_in(MemoryKind::AngryAt), 399);
                assert!(!memories.check(MemoryKind::CantReachWalkTargetSince, Status::Present));
            }
            anger
        };
        // A mob in reach: by its key.
        assert_eq!(
            angry(&by(PUFFERFISH, 9, [3.5, 1.0, 0.5], None)),
            Some(Value::Entity(EntityId(9))),
            "{kind}"
        );
        // A survival player: by their UUID.
        assert_eq!(
            angry(&by(
                entity_types::PLAYER as u16,
                2,
                [2.5, 1.0, 2.5],
                Some(2)
            )),
            Some(Value::Uuid(2))
        );
        // A creative player, whom combat refuses, and a mob beyond its follow range of
        // 16: no anger.
        assert_eq!(
            angry(&by(
                entity_types::PLAYER as u16,
                3,
                [-2.5, 1.0, 2.5],
                Some(3)
            )),
            None
        );
        assert_eq!(angry(&by(PUFFERFISH, 11, [-15.5, 1.0, 9.5], None)), None);
    }
}

#[test]
fn an_angry_nautilus_or_one_with_an_attack_target_takes_no_lead() {
    for kind in [NAUTILUS, ZOMBIE] {
        let place = Place::pool();
        let mut keeper = wild_in_pool(&place, kind);
        assert!(of(kind).can_be_leashed(kind, &keeper.mob));
        keeper.remember(|memories| {
            memories.set_with_expiry(MemoryKind::AngryAt, Value::Entity(EntityId(9)), 400);
        });
        assert!(!of(kind).can_be_leashed(kind, &keeper.mob));
        let mut keeper = wild_in_pool(&place, kind);
        targeting(&mut keeper, EntityId(9));
        assert!(!of(kind).can_be_leashed(kind, &keeper.mob));
    }
}

#[test]
fn what_angered_it_in_water_is_its_attack_target_before_any_cooldown() {
    let mut place = Place::pool();
    place.put(pufferfish(9, [3.5, 1.0, 0.5]));
    let mut keeper = wild_in_pool(&place, ZOMBIE);
    keeper.remember(|memories| {
        memories.set(MemoryKind::AttackTargetCooldown, Value::Int(3000));
        memories.set_with_expiry(MemoryKind::AngryAt, Value::Entity(EntityId(9)), 400);
    });
    let replay = keeper.stream();
    keeper.tick(&place);
    let memories = keeper.memories();
    assert_eq!(
        memories.get(MemoryKind::AttackTarget),
        Some(Value::Entity(EntityId(9)))
    );
    // The anger's way draws nothing of its own: the cooldown ran on.
    assert_eq!(memories.int(MemoryKind::AttackTargetCooldown), Some(2999));
    let _ = replay;
    assert_eq!(keeper.activity(), Some(Activity::Fight));
}

#[test]
fn a_tempted_nautilus_walks_to_the_player_and_its_temptation_cools_down_after() {
    let mut place = Place::pool();
    place
        .players
        .push(player(2, [6.5, 1.0, 0.5], Some(items::TROPICAL_FISH)));
    let mut keeper = wild_in_pool(&place, NAUTILUS);
    keeper.remember(|memories| {
        memories.set(MemoryKind::AttackTargetCooldown, Value::Int(3000));
    });
    keeper.tick(&place);
    // Sensed this tick; followed from the next.
    keeper.tick(&place);
    let memories = keeper.memories();
    assert_eq!(
        memories.get(MemoryKind::LookTarget),
        Some(Value::Tracker(Tracker::Entity {
            key: EntityId(2),
            eyes: true,
            eyes_block: false
        }))
    );
    assert!(matches!(
        memories.get(MemoryKind::WalkTarget),
        Some(Value::Walk { speed, close_enough: 2, .. }) if speed == 1.3
    ));
    // The player puts the fish away: at its next sensing it stops, and cools down 100.
    place.players[0].main_hand = None;
    for _ in 0..20 {
        keeper.tick(&place);
        if keeper
            .memories()
            .check(MemoryKind::TemptationCooldownTicks, Status::Present)
        {
            break;
        }
    }
    let memories = keeper.memories();
    let cooldown = memories.int(MemoryKind::TemptationCooldownTicks).unwrap();
    assert!((98..=100).contains(&cooldown), "{cooldown}");
}

// ------------------------------------------------------------------------------------
// The copper golem.

#[test]
fn the_copper_golem_registers_what_its_sensors_and_behaviours_name() {
    let expected: BTreeSet<MemoryKind> = [
        MemoryKind::Mobs,
        MemoryKind::VisibleMobs,
        MemoryKind::HurtBy,
        MemoryKind::HurtByEntity,
        MemoryKind::IsPanicking,
        MemoryKind::LookTarget,
        MemoryKind::WalkTarget,
        MemoryKind::CantReachWalkTargetSince,
        MemoryKind::Path,
        MemoryKind::DoorsToClose,
        MemoryKind::GazeCooldownTicks,
        MemoryKind::TransportItemsCooldownTicks,
        MemoryKind::VisitedBlockPositions,
        MemoryKind::UnreachableTransportBlockPositions,
    ]
    .into_iter()
    .collect();
    assert_eq!(registered(&copper_golem::PROVIDER), expected);
    assert!(copper_golem::PROVIDER.memories.is_empty());
    assert_eq!(copper_golem::PROVIDER.sensors.len(), 2);
    assert_eq!(
        shape(&copper_golem::PROVIDER),
        [
            (Activity::Core, vec![0, 1, 2, 3, 4, 5], 0),
            (Activity::Idle, vec![0, 1, 2], 0),
        ]
    );
}

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

#[test]
fn a_golem_takes_up_to_16_of_the_first_stack_there_is() {
    let mut slots = vec![None; 27];
    assert_eq!(golem_transport::pickup(&slots), None);
    slots[3] = stack(items::COBBLESTONE, 40);
    slots[5] = stack(items::DIRT, 2);
    assert_eq!(golem_transport::pickup(&slots), Some((3, 16)));
    slots[3] = stack(items::COBBLESTONE, 5);
    assert_eq!(golem_transport::pickup(&slots), Some((3, 5)));
}

#[test]
fn a_golem_tops_up_stacks_of_its_item_on_its_way_to_the_first_empty_slot() {
    // A stack of its item with room before the first empty slot is topped up first; a
    // full one and another item are passed.
    let mut slots = vec![None; 27];
    slots[0] = stack(items::DIRT, 64);
    slots[1] = stack(items::COBBLESTONE, 64);
    slots[2] = stack(items::COBBLESTONE, 60);
    let rest = golem_transport::add_items(&mut slots, ItemStack::new(items::COBBLESTONE, 16));
    assert_eq!(rest, None);
    // The game takes the room (4) from the hand and leaves the rest of it whole in the
    // first empty slot.
    assert_eq!(slots[2], stack(items::COBBLESTONE, 64));
    assert_eq!(slots[3], stack(items::COBBLESTONE, 12));
    // A hand smaller than the room is all moved, and nothing is left.
    let mut slots = vec![stack(items::COBBLESTONE, 10)];
    let rest = golem_transport::add_items(&mut slots, ItemStack::new(items::COBBLESTONE, 3));
    assert_eq!(
        (rest, slots[0].clone()),
        (None, stack(items::COBBLESTONE, 13))
    );
    // A chest full of others gives it all back.
    let mut slots = vec![stack(items::DIRT, 64); 27];
    let hand = ItemStack::new(items::COBBLESTONE, 16);
    assert_eq!(
        golem_transport::add_items(&mut slots, hand.clone()),
        Some(hand)
    );
}

/// The golem's sorting's state, read from its brain's parts (its part is the 7th, after
/// its core's six).
fn transporting(keeper: &Keeper) -> Transporting {
    let id = brain::ids::FIRST_BEHAVIOUR + 6;
    keeper
        .mob
        .mind
        .brain
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| {
            postcard::from_bytes::<(bool, i64, Transporting)>(&entry.bytes)
                .expect("a behaviour's part reads back")
                .2
        })
        .unwrap_or_default()
}

/// A golem standing on the stone at `at`, its first cooldown spent.
fn golem_at(place: &Place, at: Vec3) -> Keeper {
    Keeper::new(place, GOLEM, healthy(GOLEM, Default::default()), at)
}

#[test]
fn a_golem_with_an_empty_hand_takes_the_nearest_copper_chest_it_has_not_visited() {
    let mut place = Place::default();
    let near = BlockPos::new(3, 0, 0);
    let far = BlockPos::new(-6, 0, 0);
    let plain = BlockPos::new(1, 0, 2);
    for pos in [near, far] {
        place.chest(pos, blocks::COPPER_CHEST, &[stack(items::DIRT, 3)], 0);
    }
    place.chest(plain, blocks::CHEST, &[stack(items::DIRT, 3)], 0);
    let mut keeper = golem_at(&place, [0.5, 0.0, 0.5]);
    keeper.tick(&place);
    // Its sorting's start draws its duration (`nextInt(1)`); nothing else draws, as no
    // player is near to look at; its one activity is IDLE.
    assert_eq!(keeper.drew, 1);
    assert_eq!(keeper.activity(), Some(Activity::Idle));
    let state = transporting(&keeper);
    assert_eq!(state.target.map(|target| target.pos), Some(near));
    assert_eq!(state.way, Way::Travelling);
    assert_eq!(
        keeper.memories().get(MemoryKind::VisitedBlockPositions),
        Some(Value::Blocks(vec![near]))
    );
    // With the near one visited, the far one.
    let mut keeper = golem_at(&place, [0.5, 0.0, 0.5]);
    keeper.remember(|memories| {
        memories.set_with_expiry(
            MemoryKind::VisitedBlockPositions,
            Value::Blocks(vec![near]),
            6000,
        );
    });
    keeper.tick(&place);
    assert_eq!(
        transporting(&keeper).target.map(|target| target.pos),
        Some(far)
    );
    // A copper chest under a block that conducts is blocked, and passed over.
    place.set(near.offset(0, 1, 0), blocks::STONE);
    let mut keeper = golem_at(&place, [0.5, 0.0, 0.5]);
    keeper.tick(&place);
    assert_eq!(
        transporting(&keeper).target.map(|target| target.pos),
        Some(far)
    );
}

#[test]
fn a_golem_holding_something_takes_the_nearest_chest_or_trapped_chest() {
    let mut place = Place::default();
    place.chest(BlockPos::new(2, 0, 0), blocks::COPPER_CHEST, &[], 0);
    place.chest(BlockPos::new(-4, 0, 0), blocks::TRAPPED_CHEST, &[], 0);
    place.chest(BlockPos::new(6, 0, 0), blocks::CHEST, &[], 0);
    let mut mob = healthy(GOLEM, Default::default());
    mob.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)] = stack(items::DIRT, 4);
    let mut keeper = Keeper::new(&place, GOLEM, mob, [0.5, 0.0, 0.5]);
    keeper.tick(&place);
    assert_eq!(
        transporting(&keeper).target.map(|target| target.pos),
        Some(BlockPos::new(-4, 0, 0))
    );
}

#[test]
fn a_golem_that_finds_nothing_rests_140_ticks_and_strolls_or_stands_meanwhile() {
    let place = Place::default();
    let mut keeper = golem_at(&place, [0.5, 0.0, 0.5]);
    keeper.tick(&place);
    assert_eq!(
        keeper
            .memories()
            .int(MemoryKind::TransportItemsCooldownTicks),
        Some(140)
    );
    assert_eq!(transporting(&keeper).target, None);
    // Its core counts it down from the next tick, while its rest's `RunOne` runs.
    keeper.tick(&place);
    assert_eq!(
        keeper
            .memories()
            .int(MemoryKind::TransportItemsCooldownTicks),
        Some(139)
    );
}

/// The steps at chests a tick put out.
fn chest_steps(out: &MobOutput) -> Vec<(bool, BlockPos, copper_golem::ChestStep)> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::ContainerTake { chest, step } => Some((true, *chest, *step)),
            Local::ContainerPut { chest, step } => Some((false, *chest, *step)),
            _ => None,
        })
        .collect()
}

#[test]
fn at_a_chest_a_golem_opens_it_at_once_sounds_at_the_ninth_tick_and_takes_at_the_60th() {
    use copper_golem::ChestStep::{Close, Move, Open};
    let mut place = Place::default();
    let chest = BlockPos::new(3, 0, 0);
    place.chest(chest, blocks::COPPER_CHEST, &[stack(items::DIRT, 20)], 0);
    // Beside the chest's west side, within half a block of its shape.
    let mut keeper = golem_at(&place, [2.4, 0.0, 0.5]);
    // Its first tick finds the chest; its second reaches it and opens it.
    let out = keeper.tick(&place);
    assert!(chest_steps(&out).is_empty());
    let out = keeper.tick(&place);
    assert_eq!(chest_steps(&out), [(true, chest, Open)]);
    let state = transporting(&keeper);
    assert_eq!(state.way, Way::Interacting);
    assert_eq!(state.interaction, Some(Reached::Pickup));
    let own = copper_golem::state_of(&keeper.mob);
    assert_eq!(own.opened_chest, Some(chest));
    assert_eq!(own.state, copper_golem::GETTING_ITEM);
    assert_eq!(
        keeper.memories().get(MemoryKind::LookTarget),
        Some(Value::Tracker(Tracker::Block(chest)))
    );
    for at_chest in 2..=60 {
        let out = keeper.tick(&place);
        let sounds = sounds_of(&out);
        if at_chest == 9 {
            assert_eq!(sounds, [sounds::ENTITY_COPPER_GOLEM_NO_ITEM_GET]);
        } else {
            assert!(sounds.is_empty(), "{at_chest}: {sounds:?}");
        }
        if at_chest == 60 {
            // Closed, then the stack moved (by the region), then on its way with its
            // memories of chests forgotten.
            assert_eq!(
                chest_steps(&out),
                [(true, chest, Close), (true, chest, Move)]
            );
        } else {
            assert!(chest_steps(&out).is_empty(), "{at_chest}");
        }
    }
    let own = copper_golem::state_of(&keeper.mob);
    assert_eq!((own.opened_chest, own.state), (None, copper_golem::IDLE));
    let state = transporting(&keeper);
    assert_eq!(
        (state.target, state.way, state.interaction),
        (None, Way::Travelling, None)
    );
    assert!(
        !keeper
            .memories()
            .check(MemoryKind::VisitedBlockPositions, Status::Present)
    );
}

#[test]
fn at_an_empty_copper_chest_it_shows_no_item_and_gives_the_chest_up() {
    use copper_golem::ChestStep::{Close, Open};
    let mut place = Place::default();
    let chest = BlockPos::new(3, 0, 0);
    place.chest(chest, blocks::COPPER_CHEST, &[], 0);
    let mut keeper = golem_at(&place, [2.4, 0.0, 0.5]);
    keeper.tick(&place);
    let out = keeper.tick(&place);
    assert_eq!(chest_steps(&out), [(true, chest, Open)]);
    assert_eq!(
        copper_golem::state_of(&keeper.mob).state,
        copper_golem::GETTING_NO_ITEM
    );
    let mut closing = Vec::new();
    for _ in 2..=60 {
        closing.extend(chest_steps(&keeper.tick(&place)));
    }
    assert_eq!(closing, [(true, chest, Close)]);
    // Given up but remembered: its next search passes it over, and finds nothing.
    assert_eq!(
        keeper.memories().get(MemoryKind::VisitedBlockPositions),
        Some(Value::Blocks(vec![chest]))
    );
    keeper.tick(&place);
    assert_eq!(
        keeper
            .memories()
            .int(MemoryKind::TransportItemsCooldownTicks),
        Some(140)
    );
}

#[test]
fn a_golem_queues_by_a_chest_someone_has_open_and_goes_on_once_nobody_has() {
    use copper_golem::ChestStep::Open;
    let mut place = Place::default();
    let chest = BlockPos::new(3, 0, 0);
    place.chest(chest, blocks::COPPER_CHEST, &[stack(items::DIRT, 2)], 1);
    let mut keeper = golem_at(&place, [2.4, 0.0, 0.5]);
    for _ in 0..5 {
        let out = keeper.tick(&place);
        assert!(chest_steps(&out).is_empty());
    }
    assert_eq!(transporting(&keeper).way, Way::Queuing);
    place.chest(chest, blocks::COPPER_CHEST, &[stack(items::DIRT, 2)], 0);
    let out = keeper.tick(&place);
    assert_eq!(chest_steps(&out), [(true, chest, Open)]);
}

#[test]
fn a_golem_puts_into_a_chest_with_its_item_and_shows_dropping_it() {
    use copper_golem::ChestStep::{Close, Move, Open};
    let mut place = Place::default();
    let chest = BlockPos::new(3, 0, 0);
    place.chest(chest, blocks::CHEST, &[stack(items::DIRT, 2)], 0);
    let mut mob = healthy(GOLEM, Default::default());
    mob.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)] = stack(items::DIRT, 16);
    let mut keeper = Keeper::new(&place, GOLEM, mob, [2.4, 0.0, 0.5]);
    keeper.tick(&place);
    let out = keeper.tick(&place);
    assert_eq!(chest_steps(&out), [(false, chest, Open)]);
    assert_eq!(
        copper_golem::state_of(&keeper.mob).state,
        copper_golem::DROPPING_ITEM
    );
    let mut steps = Vec::new();
    let mut heard = Vec::new();
    for _ in 2..=60 {
        let out = keeper.tick(&place);
        steps.extend(chest_steps(&out));
        heard.extend(sounds_of(&out));
    }
    assert_eq!(steps, [(false, chest, Close), (false, chest, Move)]);
    assert_eq!(heard, [sounds::ENTITY_COPPER_GOLEM_ITEM_DROP]);
}

#[test]
fn a_fresh_load_of_a_golem_lets_go_of_its_chest_and_is_idle() {
    let mut mob = healthy(GOLEM, Default::default());
    copper_golem::update(&mut mob, |own| {
        own.opened_chest = Some(BlockPos::new(1, 2, 3));
        own.state = copper_golem::DROPPING_ITEM;
        own.weather = copper_golem::WEATHERED;
    });
    copper_golem::fresh_load(&mut mob);
    let own = copper_golem::state_of(&mob);
    assert_eq!(
        (own.opened_chest, own.state, own.weather),
        (None, copper_golem::IDLE, copper_golem::WEATHERED)
    );
}

#[test]
fn a_fresh_load_of_a_nautilus_ends_its_dash() {
    let mut mob = healthy(
        NAUTILUS,
        animal::write(
            &Animal::default(),
            &NautilusState {
                dashing: true,
                dash_ready_at: 90,
                ..NautilusState::default()
            },
        ),
    );
    nautilus::fresh_load(&mut mob);
    let own = nautilus::state_of(&mob).1;
    assert!(!own.dashing);
    assert_eq!(own.dash_ready_at, 0);
}
