//! The witch and the illagers from the game's rules (`n7k-brief.md` 3.1, 3.6, 3.7 and
//! 5; `Witch`, `Raider`, `PatrollingMonster`, `Pillager`, `Vindicator`, `Evoker`), not
//! from the code: each egg's draws in order with the leader's 0.06 and the pillager's
//! one in 300, what each holds and shows, the witch's draws for a potion a tick in the
//! game's order, its drinking, and what a drunk potion does.

use std::collections::VecDeque;

use clustine_data::sounds;
use clustine_data::{BlockState, EntityTypeInfo, attributes as ids, blocks, entity_types, items};
use clustine_world::{BlockPos, ChunkPos, EntityId, ItemStack};

use super::illagers::{self, ArmPose, Potion, RaiderState};
use super::template::build::{self as nbt};
use super::{SpawnReason, Spawning, of};
use crate::living::{Kind, Living, MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect;
use crate::mobs::body::{Event, attribute};
use crate::mobs::tick::{self, Local, MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen};
use crate::physics::World;
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::region::Random;
use crate::shown::DataShown;

const WITCH: u16 = entity_types::WITCH as u16;
const PILLAGER: u16 = entity_types::PILLAGER as u16;
const VINDICATOR: u16 = entity_types::VINDICATOR as u16;
const EVOKER: u16 = entity_types::EVOKER as u16;
const KEY: EntityId = EntityId(40);

/// One call of the level's random, with its bound where it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Int(u32),
    Float,
    Double,
}

use Call::{Double, Float, Int};

/// The level's random of a test: every call recorded, each answered from its script
/// where one is given, else with a middling value.
#[derive(Default)]
struct Counting {
    calls: Vec<Call>,
    ints: VecDeque<u32>,
    floats: VecDeque<f32>,
}

impl clustine_world::generation::Draws for Counting {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push(Int(bound));
        self.ints.pop_front().unwrap_or(bound / 2)
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push(Float);
        self.floats.pop_front().unwrap_or(0.5)
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push(Double);
        0.5
    }

    fn next_bool(&mut self) -> bool {
        unreachable!("no raider draws a boolean")
    }
}

/// `Mob.finalizeSpawn`'s tail: the follow range's triangle, then the hand.
const TAIL: [Call; 3] = [Double, Double, Float];

/// A mob of `kind` made for `reason` and finalised with `level`.
fn made(kind: u16, reason: SpawnReason, level: &mut Counting) -> Mob {
    let mut own = Random::of_entity(7, KEY, 0);
    let (_, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome: Some("minecraft:plains"),
            at: BlockPos::new(0, 64, 0),
            day: 0,
            game_time: 0,
            reason,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
        },
    );
    mob
}

fn held(mob: &Mob, slot: u8) -> Option<i32> {
    mob.body.equipment.slots[usize::from(slot)]
        .as_ref()
        .map(|stack| stack.item)
}

#[test]
fn a_pillager_s_egg_draws_its_crossbow_s_enchantments_the_one_in_300_the_leader_then_the_tail() {
    let mut level = Counting::default();
    let mob = made(PILLAGER, SpawnReason::SpawnEgg, &mut level);
    assert_eq!(
        level.calls,
        [&[Float, Int(300), Float][..], &TAIL].concat(),
        "the hand's enchantment, Piercing's one in 300, the leader's 0.06, then `Mob`'s"
    );
    assert_eq!(held(&mob, slots::MAIN_HAND), Some(items::CROSSBOW));
    assert_eq!(held(&mob, slots::HEAD), None, "no leader at 0.5");
    let own = illagers::state_of(&mob);
    assert!(own.can_join_raid && !own.patrol_leader && !own.patrolling);

    // The one in 300 passing changes nothing a stack can hold.
    let mut level = Counting {
        ints: VecDeque::from([0]),
        ..Counting::default()
    };
    let lucky = made(PILLAGER, SpawnReason::SpawnEgg, &mut level);
    assert_eq!(held(&lucky, slots::MAIN_HAND), Some(items::CROSSBOW));
}

#[test]
fn a_pillager_under_0_06_leads_with_a_banner_it_always_drops() {
    let mut level = Counting {
        floats: VecDeque::from([0.5, 0.059]),
        ..Counting::default()
    };
    let mob = made(PILLAGER, SpawnReason::SpawnEgg, &mut level);
    assert!(illagers::state_of(&mob).patrol_leader);
    assert_eq!(held(&mob, slots::HEAD), Some(items::WHITE_BANNER));
    assert_eq!(
        mob.body.equipment.drop_chance(usize::from(slots::HEAD)),
        2.0
    );
    assert_eq!(
        mob.body
            .equipment
            .drop_chance(usize::from(slots::MAIN_HAND)),
        0.085
    );
    // Exactly 0.06 is not under it.
    let mut level = Counting {
        floats: VecDeque::from([0.5, 0.06]),
        ..Counting::default()
    };
    assert!(!illagers::state_of(&made(PILLAGER, SpawnReason::SpawnEgg, &mut level)).patrol_leader);
}

#[test]
fn a_structure_s_raider_draws_no_leader() {
    let mut level = Counting::default();
    made(PILLAGER, SpawnReason::Structure, &mut level);
    assert_eq!(level.calls, [&[Float, Int(300)][..], &TAIL].concat());
    let mut level = Counting::default();
    made(EVOKER, SpawnReason::Structure, &mut level);
    assert_eq!(level.calls, TAIL);
    let mut level = Counting::default();
    let vindicator = made(VINDICATOR, SpawnReason::Structure, &mut level);
    assert_eq!(level.calls, [&TAIL[..], &[Float]].concat());
    assert_eq!(held(&vindicator, slots::MAIN_HAND), Some(items::IRON_AXE));
}

#[test]
fn a_vindicator_draws_the_leader_and_the_tail_then_holds_its_axe_and_draws_its_enchantment() {
    let mut level = Counting::default();
    let mob = made(VINDICATOR, SpawnReason::SpawnEgg, &mut level);
    assert_eq!(level.calls, [&[Float][..], &TAIL, &[Float]].concat());
    assert_eq!(held(&mob, slots::MAIN_HAND), Some(items::IRON_AXE));
    assert!(mob.body.equipment.slots[usize::from(slots::HEAD)].is_none());
}

#[test]
fn an_evoker_draws_the_leader_and_the_tail_and_holds_nothing() {
    let mut level = Counting {
        floats: VecDeque::from([0.01]),
        ..Counting::default()
    };
    let mob = made(EVOKER, SpawnReason::SpawnEgg, &mut level);
    assert_eq!(level.calls, [&[Float][..], &TAIL].concat());
    assert!(illagers::state_of(&mob).patrol_leader, "an evoker can lead");
    assert_eq!(held(&mob, slots::MAIN_HAND), None);
}

#[test]
fn a_witch_draws_the_leader_s_0_06_and_never_leads() {
    let mut level = Counting {
        floats: VecDeque::from([0.0]),
        ..Counting::default()
    };
    let mob = made(WITCH, SpawnReason::SpawnEgg, &mut level);
    assert_eq!(level.calls, [&[Float][..], &TAIL].concat());
    let own = illagers::state_of(&mob);
    assert!(!own.patrol_leader && own.can_join_raid);
    assert!(mob.body.equipment.slots.iter().all(Option::is_none));
    // A naturally spawned witch may not join a raid.
    let natural = made(WITCH, SpawnReason::Natural, &mut Counting::default());
    assert!(!illagers::state_of(&natural).can_join_raid);
}

#[test]
fn each_raider_shows_its_celebration_at_16_and_its_own_at_17() {
    let fresh = Mob::fresh();
    let celebrating = (16, DataShown::Boolean(false), true);
    assert_eq!(
        of(WITCH).shown(&fresh),
        [celebrating, (17, DataShown::Boolean(false), true)]
    );
    assert_eq!(
        of(PILLAGER).shown(&fresh),
        [celebrating, (17, DataShown::Boolean(false), true)]
    );
    assert_eq!(of(VINDICATOR).shown(&fresh), [celebrating]);
    assert_eq!(
        of(EVOKER).shown(&fresh),
        [celebrating, (17, DataShown::Byte(0), true)]
    );
    let mut drinking = Mob::fresh();
    illagers::put_state(
        &mut drinking,
        &RaiderState {
            drinking: Some(Potion::Healing),
            using_time: 12,
            ..RaiderState::default()
        },
    );
    assert_eq!(
        of(WITCH).shown(&drinking)[1],
        (17, DataShown::Boolean(true), false)
    );
}

#[test]
fn the_illagers_arms_are_crossed_but_the_pillager_s_holding_its_crossbow() {
    let mut pillager = Mob::fresh();
    assert_eq!(
        illagers::arm_pose(PILLAGER, &pillager, false),
        ArmPose::Neutral
    );
    pillager.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(ItemStack::new(items::CROSSBOW, 1));
    assert_eq!(
        illagers::arm_pose(PILLAGER, &pillager, false),
        ArmPose::CrossbowHold
    );
    let fresh = Mob::fresh();
    assert_eq!(
        illagers::arm_pose(VINDICATOR, &fresh, false),
        ArmPose::Crossed
    );
    assert_eq!(
        illagers::arm_pose(VINDICATOR, &fresh, true),
        ArmPose::Attacking
    );
    assert_eq!(illagers::arm_pose(EVOKER, &fresh, false), ArmPose::Crossed);
}

#[test]
fn a_raider_not_patrolling_may_despawn_anywhere_and_a_patrolling_one_only_beyond_128() {
    let mut mob = Mob::fresh();
    for kind in [WITCH, PILLAGER, VINDICATOR, EVOKER] {
        assert!(of(kind).remove_when_far_away(kind, &mob, 0, 100.0));
    }
    illagers::update(&mut mob, |own| own.patrolling = true);
    assert!(!of(PILLAGER).remove_when_far_away(PILLAGER, &mob, 0, 16_384.0));
    assert!(of(PILLAGER).remove_when_far_away(PILLAGER, &mob, 0, 16_385.0));
}

#[test]
fn a_template_s_raider_keeps_its_patrol_its_wave_and_a_vindicator_s_johnny() {
    let tag = nbt::compound(&[
        nbt::byte("PatrolLeader", 1),
        nbt::byte("Patrolling", 1),
        nbt::int("Wave", 3),
        nbt::byte("CanJoinRaid", 1),
        nbt::byte("Johnny", 1),
        nbt::int("SpellTicks", 7),
    ]);
    let mut vindicator = Mob::fresh();
    of(VINDICATOR).from_template(&mut vindicator, &tag);
    let own = illagers::state_of(&vindicator);
    assert!(own.patrol_leader && own.patrolling && own.can_join_raid && own.johnny);
    assert_eq!(own.wave, 3);
    let mut evoker = Mob::fresh();
    of(EVOKER).from_template(&mut evoker, &tag);
    assert_eq!(illagers::state_of(&evoker).spell_ticks, 7);
    assert!(
        !illagers::state_of(&evoker).johnny,
        "only a vindicator is Johnny"
    );
}

#[test]
fn a_raider_s_experience_is_five_and_more_for_what_it_holds_and_may_not_keep() {
    let mut mob = Mob::fresh();
    let mut draws = Random::of_entity(3, KEY, 0);
    assert_eq!(of(WITCH).experience_reward(WITCH, &mob, &mut draws), 5);
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(ItemStack::new(items::CROSSBOW, 1));
    let mut replay = draws.clone();
    let extra = 1 + replay.next_int(3) as i32;
    assert_eq!(
        of(PILLAGER).experience_reward(PILLAGER, &mob, &mut draws),
        5 + extra
    );
    // A banner it always drops adds nothing.
    let mut banner = Mob::fresh();
    banner.body.equipment.slots[usize::from(slots::HEAD)] =
        Some(ItemStack::new(items::WHITE_BANNER, 1));
    let mut chances = [0.085; 8];
    chances[usize::from(slots::HEAD)] = 2.0;
    banner.body.equipment.drop_chances = Some(chances);
    assert_eq!(of(EVOKER).experience_reward(EVOKER, &banner, &mut draws), 5);
}

// The witch's drinking, on a body of its own.

/// A floor of stone with three blocks of `fluid` on it (air for none), at night.
struct Pool {
    fluid: BlockState,
}

impl World for Pool {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(match pos.y {
            ..0 => blocks::STONE,
            0..3 => self.fluid,
            _ => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Pool {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Pool {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, _: &Aabb) -> Vec<Seen<'_>> {
        Vec::new()
    }

    fn entity(&self, _: EntityId) -> Option<Seen<'_>> {
        None
    }

    fn players(&self) -> Vec<crate::mobs::view::SeenPlayer> {
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        Some(LightAt { sky: 15, block: 0 })
    }

    fn day(&self) -> u64 {
        18_000
    }
}

/// A witch's body at rest on the floor (in the fluid, if any), its first tick behind it.
fn living(fluid: BlockState) -> Living {
    let info = EntityTypeInfo::of(entity_types::WITCH);
    let position = [0.5, 0.0, 0.5];
    let body = Moving {
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
        in_water: fluid == blocks::WATER,
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
            kind: WITCH,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living
}

fn max_health() -> f32 {
    crate::mobs::body::attributes::supplied(WITCH, attribute(ids::MAX_HEALTH)).unwrap() as f32
}

fn speed(mob: &Mob) -> f64 {
    mob.body
        .attributes
        .value(WITCH, attribute(ids::MOVEMENT_SPEED))
}

/// A witch of `health`, without AI so that it stays where it is put.
fn witch(health: f32) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = health;
    mob.body.vitals.air = MAX_AIR;
    mob.body.flags.no_ai = true;
    mob
}

/// The witch's own `aiStep` part once, on `mob` in `fluid` (burning for `fire` ticks),
/// with `stream`: the mob and what it did.
fn ai_step(mob: Mob, fluid: BlockState, fire: i32, stream: &mut Random) -> (Mob, MobOutput) {
    let world = Pool { fluid };
    let mut living = living(fluid);
    living.fire_ticks = fire;
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: KEY,
        kind: WITCH,
        lived: 100,
        tick_count: 100,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: true,
        mob,
        living,
        yaw: 0.0,
        pitch: 0.0,
        mob_type: of(WITCH),
        world: &world,
        draws: stream,
        out: MobOutput::default(),
    };
    of(WITCH).before_ai_step(&mut tick);
    (tick.mob, tick.out)
}

/// The first stream from seed 0 up whose floats pass `wanted`.
fn stream_where(wanted: impl Fn(&mut Random) -> bool) -> Random {
    (0..)
        .map(|seed| Random::of_entity(seed, KEY, 1))
        .find(|stream| wanted(&mut stream.clone()))
        .expect("some stream draws so")
}

#[test]
fn a_hurt_witch_draws_for_each_need_in_turn_and_takes_up_healing_with_its_drink_s_pitch() {
    let mut stream = stream_where(|r| {
        let (_breath, _fire, heal) = (r.next_float(), r.next_float(), r.next_float());
        heal < 0.05
    });
    let mut replay = stream.clone();
    let (mob, out) = ai_step(witch(10.0), blocks::AIR, 0, &mut stream);
    // Water breathing's and fire resistance's draws are made though neither is needed,
    // then healing's passes; then the drink's pitch, then the particles'.
    for _ in 0..3 {
        replay.next_float();
    }
    let pitch = 0.8 + replay.next_float() * 0.4;
    let particles = replay.next_float() < 7.5e-4;
    assert_eq!(stream, replay, "five draws in all");
    let own = illagers::state_of(&mob);
    assert_eq!(own.drinking, Some(Potion::Healing));
    assert_eq!(own.using_time, 32);
    assert_eq!(held(&mob, slots::MAIN_HAND), Some(items::POTION));
    let mut expected = vec![Event::Sound {
        sound: sounds::ENTITY_WITCH_DRINK,
        volume: 1.0,
        pitch,
    }];
    if particles {
        expected.push(Event::Entity(15));
    }
    assert_eq!(out.events, expected);
    // Its slowness while it drinks: a quarter off a quarter.
    assert_eq!(speed(&mob), 0.0);
}

#[test]
fn a_witch_in_no_need_draws_four_floats_and_the_particles_and_shows_them_under_7_5e_4() {
    let mut stream = stream_where(|r| {
        for _ in 0..4 {
            r.next_float();
        }
        r.next_float() < 7.5e-4
    });
    let mut replay = stream.clone();
    let (mob, out) = ai_step(witch(max_health()), blocks::AIR, 0, &mut stream);
    for _ in 0..5 {
        replay.next_float();
    }
    assert_eq!(stream, replay);
    assert_eq!(illagers::state_of(&mob), RaiderState::default());
    assert_eq!(out.events, [Event::Entity(15)]);
}

#[test]
fn a_witch_under_water_takes_up_water_breathing_and_a_burning_one_fire_resistance() {
    let mut stream = stream_where(|r| r.next_float() < 0.15);
    let (mob, _) = ai_step(witch(max_health()), blocks::WATER, 0, &mut stream);
    assert_eq!(
        illagers::state_of(&mob).drinking,
        Some(Potion::WaterBreathing)
    );

    // On fire: the first draw is wasted on dry land, the second passes.
    let mut stream = stream_where(|r| {
        r.next_float();
        r.next_float() < 0.15
    });
    let (mob, _) = ai_step(witch(max_health()), blocks::AIR, 100, &mut stream);
    assert_eq!(
        illagers::state_of(&mob).drinking,
        Some(Potion::FireResistance)
    );
}

#[test]
fn a_drinking_witch_counts_down_and_draws_only_for_its_particles() {
    let mut mob = witch(10.0);
    illagers::put_state(
        &mut mob,
        &RaiderState {
            drinking: Some(Potion::Healing),
            using_time: 5,
            ..RaiderState::default()
        },
    );
    let mut stream = Random::of_entity(9, KEY, 0);
    let mut replay = stream.clone();
    let (mob, _) = ai_step(mob, blocks::AIR, 0, &mut stream);
    replay.next_float();
    assert_eq!(stream, replay, "the particles' draw only");
    assert_eq!(illagers::state_of(&mob).using_time, 4);
}

/// A witch ticking whole in a world, its body carried from one tick to the next.
struct Run {
    world: Pool,
    mob: Mob,
    living: Living,
    lived: u32,
    stream: Random,
    out: Vec<MobOutput>,
}

impl Run {
    fn new(mob: Mob) -> Self {
        Self {
            world: Pool { fluid: blocks::AIR },
            mob,
            living: living(blocks::AIR),
            lived: 100,
            stream: Random::of_entity(7, KEY, 1),
            out: Vec::new(),
        }
    }

    fn tick(&mut self) {
        self.living.air = self.mob.body.vitals.air;
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: WITCH,
            lived: self.lived,
            tick_count: self.lived,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: std::mem::replace(&mut self.mob, Mob::fresh()),
            living: self.living.clone(),
            yaw: 0.0,
            pitch: 0.0,
            mob_type: of(WITCH),
            world: &self.world,
            draws: &mut self.stream,
            out: MobOutput::default(),
        };
        tick::tick(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.out.push(tick.out);
        self.mob.body.vitals.air = self.living.air;
        self.lived += 1;
    }

    fn health(&self) -> f32 {
        self.mob.body.vitals.health
    }
}

/// A witch of `health` drinking `potion` with `left` ticks of it to go.
fn drinking(health: f32, potion: Potion, left: i32) -> Mob {
    let mut mob = witch(health);
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(ItemStack::new(items::POTION, 1));
    illagers::put_state(
        &mut mob,
        &RaiderState {
            drinking: Some(potion),
            using_time: left,
            ..RaiderState::default()
        },
    );
    mob
}

#[test]
fn a_hurt_witch_is_healed_the_tick_after_its_drink_of_healing_ends() {
    let mut run = Run::new(drinking(10.0, Potion::Healing, 2));
    // `usingTime--` is compared before it is lowered: 2, 1, then 0 ends it.
    run.tick();
    run.tick();
    assert!(illagers::state_of(&run.mob).drinking.is_some());
    run.tick();
    assert_eq!(
        illagers::state_of(&run.mob).drinking,
        None,
        "the drink ended"
    );
    assert_eq!(held(&run.mob, slots::MAIN_HAND), None, "the bottle is gone");
    assert_eq!(run.health(), 10.0, "not healed yet");
    assert!(
        run.out[2].local.iter().any(|local| matches!(
            local,
            Local::GameEventAt {
                name: "minecraft:drink",
                ..
            }
        )),
        "the game event `drink`"
    );
    run.tick();
    assert_eq!(run.health(), 14.0, "four back at the next tick's effects");
    assert!(
        !run.mob.body.effects.has(Effect::INSTANT_HEALTH),
        "an instant effect leaves nothing"
    );
}

#[test]
fn a_witch_moves_again_once_its_drink_ends() {
    let mut mob = drinking(max_health(), Potion::Swiftness, 0);
    // As its drink began: its slowness on.
    let mut stream = stream_where(|r| {
        r.next_float();
        r.next_float();
        r.next_float() < 0.05
    });
    let (begun, _) = ai_step(witch(10.0), blocks::AIR, 0, &mut stream);
    mob.body.attributes = begun.body.attributes.clone();
    assert_eq!(speed(&mob), 0.0);
    let mut run = Run::new(mob);
    run.tick();
    // Its slowness off, and swiftness's fifth on.
    let base = 0.25_f64;
    assert!(run.mob.body.effects.has(Effect::SPEED));
    assert_eq!(speed(&run.mob), base * (1.0 + f64::from(0.2_f32)));
}

#[test]
fn a_burning_witch_that_drank_fire_resistance_takes_no_fire_hurt_after() {
    let full = max_health();
    let mut run = Run::new(drinking(full, Potion::FireResistance, 0));
    run.living.fire_ticks = 210;
    for _ in 0..60 {
        run.tick();
    }
    assert!(run.mob.body.effects.has(Effect::FIRE_RESISTANCE));
    assert_eq!(run.health(), full, "no fire hurt with fire resistance");

    // Without it, the fire's hurt at 200 comes before any drink of its own can end.
    let mut run = Run::new(witch(full));
    run.living.fire_ticks = 210;
    for _ in 0..20 {
        run.tick();
    }
    assert!(run.health() < full);
}

#[test]
fn magic_hurts_a_witch_less_and_other_hurts_as_much() {
    use crate::mobs::body::hurt::{DamageSource, DamageType};
    let mob = Mob::fresh();
    let witch = of(WITCH);
    let magic = DamageSource::of(DamageType::MAGIC);
    assert_eq!(witch.magic_absorbed(&mob, &magic, 10.0), 10.0 * 0.15);
    let thorns = DamageSource::of(DamageType::THORNS);
    assert_eq!(witch.magic_absorbed(&mob, &thorns, 2.0), 2.0 * 0.15);
    let fall = DamageSource::of(DamageType::FALL);
    assert_eq!(witch.magic_absorbed(&mob, &fall, 4.0), 4.0);
    assert_eq!(of(PILLAGER).magic_absorbed(&mob, &magic, 10.0), 10.0);
}
