//! Family E3 of P12 from the game's rules (`docs/groundwork/p-plan.md`, section 18.3;
//! the brief's section 5.9; d `Blaze`, `Blaze$BlazeAttackGoal`, `WitherSkeleton`), not
//! from the code: the blaze's volley (its charge, its three shots six ticks apart with
//! their level event, where they start and their spread, its rest), its melee hit, its
//! climb and its height's draw, its slowed fall, its hurt by water, its goals, maluses,
//! sounds, experience and what it shows; the wither skeleton's sword and attack at its
//! spawn, its goals with the piglins, its hit's wither, its immunity to wither, its
//! sounds; both types' spawn rules.

use clustine_data::{EntityTypeInfo, PathType, attributes as ids, blocks, entity_types, items};
use clustine_world::generation::Draws;
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, EntityId};

use super::{BlazeAttackGoal, FLAGS_INDEX, SHOOT_EVENT, Volley, change, state_of};
use crate::living::{Kind, Living, MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::{Ai, Operation, Setup};
use crate::mobs::body::attribute;
use crate::mobs::body::effects::{Effect, can_be_affected};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::wither_skeleton::{self, PIGLINS, SPAWNED_ATTACK, WITHER_SKELETON};
use crate::mobs::types::{SpawnReason, Spawning, of};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::thrown::triangle;
use crate::physics::{self, Vec3, World};
use crate::region::Random;
use crate::shown::DataShown;
use crate::spawning::rules::{Light, SpawnWorld, check_spawn_rules};

const BLAZE: u16 = entity_types::BLAZE as u16;
const KEY: EntityId = EntityId(40);
const TARGET: EntityId = EntityId(77);

/// Open air above stone below y = 0, with the entities a test puts in it; `wall` a
/// block of stone at x = 5 for every y and z where it is set.
#[derive(Default)]
struct Field {
    entities: Vec<(Entity, Aabb)>,
    wall: bool,
}

impl World for Field {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        Some(if pos.y < 0 || (self.wall && pos.x == 5) {
            blocks::STONE
        } else {
            blocks::AIR
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
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        Some(LightAt { sky: 0, block: 0 })
    }
}

impl Field {
    /// A field with a zombie of 20 health standing at `feet`, the blaze's target.
    fn zombie_at(feet: Vec3) -> Self {
        let id = entity_types::ZOMBIE;
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 20.0;
        let info = EntityTypeInfo::of(id);
        let entity = Entity {
            portal: Default::default(),
            key: TARGET,
            kind: id as u16,
            uuid: uuid::Uuid::from_u128(77),
            position: feet,
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
        };
        Self {
            entities: vec![(entity, Aabb::of_entity(feet, info.width, info.height))],
            wall: false,
        }
    }
}

/// A body of the type `kind` at rest at `at`, its first tick behind it.
fn body(kind: u16, at: Vec3, on_ground: bool) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let body = Moving {
        stands_on_lava: false,
        position: at,
        motion: [0.0; 3],
        on_ground,
        fall_distance: 0.0,
        width: info.width,
        height: info.height,
        old_position: at,
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
    living
}

/// A mob of the type `kind` ticked again and again, its stream carried from one tick to
/// the next.
struct Run {
    kind: u16,
    mob: Mob,
    living: Living,
    yaw: f32,
    pitch: f32,
    lived: u32,
    stream: Random,
}

impl Run {
    /// A mob of 20 health at `at` whose target is the field's.
    fn at(kind: u16, at: Vec3, on_ground: bool) -> Self {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 20.0;
        mob.body.vitals.air = MAX_AIR;
        mob.body.combat.target = Some(TARGET);
        Self {
            kind,
            mob,
            living: body(kind, at, on_ground),
            yaw: 0.0,
            pitch: 0.0,
            lived: 100,
            stream: Random::of_entity(7, KEY, 1),
        }
    }

    fn tick<R>(&mut self, world: &Field, f: impl FnOnce(&mut MobTick<'_>) -> R) -> (R, MobOutput) {
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: std::mem::replace(&mut self.mob, Mob::fresh()),
            living: self.living.clone(),
            yaw: self.yaw,
            pitch: self.pitch,
            mob_type: of(self.kind),
            world,
            draws: &mut self.stream,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let given = f(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.pitch = tick.pitch;
        (given, tick.out)
    }

    /// `f` in the mob's AI step; what it gave and what the step made.
    fn ai<R>(&mut self, world: &Field, f: impl FnOnce(&mut Ai<'_, '_>) -> R) -> (R, MobOutput) {
        let mut setup = Setup::default();
        of(self.kind).register_goals_of(&self.mob, &mut setup);
        let config = setup.config;
        self.tick(world, |tick| {
            let mut ai = Ai {
                tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: false,
            };
            f(&mut ai)
        })
    }

    /// One tick of the blaze's attack goal.
    fn volley_tick(&mut self, world: &Field, state: &mut Volley) -> MobOutput {
        let ((), out) = self.ai(world, |ai| BlazeAttackGoal.tick(state, ai));
        self.lived += 1;
        out
    }

    fn flags_shown(&self) -> DataShown {
        let shown = of(BLAZE).shown(&self.mob);
        let [(index, value, _)] = &shown[..] else {
            panic!("one value shown: {shown:?}");
        };
        assert_eq!(*index, FLAGS_INDEX);
        value.clone()
    }
}

fn level_events(out: &MobOutput) -> Vec<i32> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::LevelEvent { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect()
}

fn fireballs(out: &MobOutput) -> Vec<(u16, Vec3, Vec3, u8)> {
    out.local
        .iter()
        .filter_map(|local| match *local {
            Local::ShootFireball {
                kind,
                at,
                direction,
                explosion_power,
            } => Some((kind, at, direction, explosion_power)),
            _ => None,
        })
        .collect()
}

fn strikes(out: &MobOutput) -> Vec<(EntityId, clustine_world::HurtBy)> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::Strike { key, hurt } => Some((*key, (**hurt).clone())),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------
// The blaze's volley.

/// From a fresh goal: the first tick charges it (60 ticks, alight); at the 61st, 67th
/// and 73rd one small fireball each with level event 1018; at the 79th it rests unlit
/// for 100; at the 179th it charges again.
#[test]
fn a_blaze_charges_for_sixty_ticks_then_shoots_three_small_fireballs_six_apart_and_rests() {
    let world = Field::zombie_at([10.5, 0.0, 0.5]);
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    let mut state = Volley::default();
    assert!(
        blaze
            .ai(&world, |ai| BlazeAttackGoal.can_use(&mut state, ai))
            .0
    );
    blaze.ai(&world, |ai| BlazeAttackGoal.start(&mut state, ai));
    let mut shots = Vec::new();
    let mut lit = Vec::new();
    for tick in 1..=180 {
        let out = blaze.volley_tick(&world, &mut state);
        let thrown = fireballs(&out);
        let events = level_events(&out);
        if thrown.is_empty() {
            assert!(events.is_empty(), "{tick}: {events:?}");
        } else {
            assert_eq!(thrown.len(), 1, "one fireball a step");
            assert_eq!(events, [SHOOT_EVENT], "{tick}");
            let (kind, _, _, power) = thrown[0];
            assert_eq!(i32::from(kind), entity_types::SMALL_FIREBALL);
            assert_eq!(power, 0);
            shots.push(tick);
        }
        if state_of(&blaze.mob).charged {
            lit.push(tick);
        }
    }
    assert_eq!(shots, [61, 67, 73]);
    // Lit from its first tick until its rest begins at the 79th, and again at the 179th.
    let expected: Vec<i32> = (1..79).chain(179..=180).collect();
    assert_eq!(lit, expected);
    assert_eq!(state.attack_step, 1);
}

/// A shot starts at the blaze's middle half a block up, and goes along the two
/// triangles' spread across, drawn of its own stream, x first, and the height between
/// the middles.
#[test]
fn a_blaze_s_fireball_starts_above_its_middle_and_spreads_by_the_fourth_root_of_the_distance() {
    let world = Field::zombie_at([10.5, 0.0, 4.5]);
    let mut blaze = Run::at(BLAZE, [0.5, 3.0, 0.5], false);
    // Its next step is a shot.
    let mut state = Volley {
        attack_step: 1,
        attack_time: 1,
        last_seen: 0,
    };
    let mut replay = blaze.stream.clone();
    let out = blaze.volley_tick(&world, &mut state);
    let [(_, at, direction, _)] = fireballs(&out)[..] else {
        panic!("one fireball: {:?}", out.local);
    };
    let height = f64::from(EntityTypeInfo::of(entity_types::BLAZE).height);
    let zombie = f64::from(EntityTypeInfo::of(entity_types::ZOMBIE).height);
    let middle = 3.0 + height * 0.5;
    assert_eq!(
        at.map(f64::to_bits),
        [0.5, middle + 0.5, 0.5].map(f64::to_bits)
    );
    let (dx, dy, dz) = (10.0, zombie * 0.5 - middle, 4.0);
    let distance: f64 = 10.0 * 10.0 + 3.0 * 3.0 + 4.0 * 4.0;
    let spread = distance.sqrt().sqrt() * 0.5;
    let across = triangle(&mut replay, dx, 2.297 * spread);
    let along = triangle(&mut replay, dz, 2.297 * spread);
    let expected = physics::normalize([across, dy, along]);
    assert_eq!(direction.map(f64::to_bits), expected.map(f64::to_bits));
    assert_eq!(
        blaze.stream.draws_since(&replay),
        0,
        "four doubles, no more"
    );
    assert_eq!(state.attack_time, 6);
    // It looks at its target meanwhile, at ten degrees a step either way.
    let look = &blaze.mob.body.controls.look;
    assert_eq!((look.y_max_rot_speed, look.x_max_rot_angle), (10.0, 10.0));
}

/// Out of sight it neither charges nor shoots; it moves onto its target for the first
/// four ticks unseen and then no more.
#[test]
fn a_blaze_that_cannot_see_its_target_holds_its_fire_and_soon_stops_following() {
    let mut world = Field::zombie_at([10.5, 0.0, 0.5]);
    world.wall = true;
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    let mut state = Volley::default();
    let mut moved = Vec::new();
    for tick in 1..=10 {
        blaze.mob.body.controls.movement.operation = Operation::Wait;
        let out = blaze.volley_tick(&world, &mut state);
        assert!(fireballs(&out).is_empty());
        if blaze.mob.body.controls.movement.operation == Operation::MoveTo {
            assert_eq!(blaze.mob.body.controls.movement.wanted, [10.5, 0.0, 0.5]);
            moved.push(tick);
        }
    }
    assert_eq!(moved, [1, 2, 3, 4]);
    assert_eq!(state.attack_step, 0);
    assert!(!state_of(&blaze.mob).charged);
}

/// Within two blocks and seen: a hit of its attack (6) at once, the next 20 ticks
/// after, and a move onto the target every tick; no volley.
#[test]
fn a_blaze_within_two_blocks_hits_every_twenty_ticks_for_six() {
    let world = Field::zombie_at([1.5, 0.0, 0.5]);
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    let mut state = Volley::default();
    let mut hits = Vec::new();
    for tick in 1..=41 {
        let out = blaze.volley_tick(&world, &mut state);
        assert!(fireballs(&out).is_empty());
        assert_eq!(blaze.mob.body.controls.movement.wanted, [1.5, 0.0, 0.5]);
        for (key, hurt) in strikes(&out) {
            assert_eq!(key, TARGET);
            assert_eq!(hurt.damage_type, "minecraft:mob_attack");
            assert_eq!(hurt.amount.0, 6.0);
            assert!(hurt.effects.is_empty());
            hits.push(tick);
        }
    }
    assert_eq!(hits, [1, 21, 41]);
    assert!(!state_of(&blaze.mob).charged);
}

/// Its goal's end puts out its charge; it is used only with a target.
#[test]
fn a_blaze_s_charge_goes_out_when_its_goal_stops() {
    let world = Field::zombie_at([10.5, 0.0, 0.5]);
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    change(&mut blaze.mob, |own| own.charged = true);
    assert_eq!(blaze.flags_shown(), DataShown::Byte(1));
    let mut state = Volley {
        last_seen: 3,
        ..Volley::default()
    };
    blaze.ai(&world, |ai| BlazeAttackGoal.stop(&mut state, ai));
    assert_eq!(blaze.flags_shown(), DataShown::Byte(0));
    assert_eq!(state.last_seen, 0);
    blaze.mob.body.combat.target = None;
    assert!(
        !blaze
            .ai(&world, |ai| BlazeAttackGoal.can_use(&mut state, ai))
            .0
    );
}

// ---------------------------------------------------------------------------------
// The blaze's body.

/// Its first step draws the height it keeps (`triangle(0.5, 6.891)`), the next 99 draw
/// nothing, the 100th draws again.
#[test]
fn a_blaze_draws_the_height_it_keeps_above_its_target_every_hundred_ticks() {
    let world = Field::default();
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    blaze.mob.body.combat.target = None;
    assert_eq!(state_of(&blaze.mob).allowed_height_offset, 0.5);
    let mut replay = blaze.stream.clone();
    let mut drawn = Vec::new();
    for step in 1..=201 {
        let start = blaze.stream.clone();
        blaze.tick(&world, super::custom_server_ai_step);
        if blaze.stream.draws_since(&start) > 0 {
            assert_eq!(blaze.stream.draws_since(&start), 2, "two doubles");
            let offset = triangle(&mut replay, 0.5, 6.891) as f32;
            assert_eq!(state_of(&blaze.mob).allowed_height_offset, offset);
            drawn.push(step);
        }
    }
    assert_eq!(drawn, [1, 101, 201]);
}

/// With its target's eyes above its own by more than its offset, its vertical motion
/// goes three tenths of the way to 0.3 and is sent; level with it, nothing.
#[test]
fn a_blaze_climbs_towards_a_target_above_it_and_not_one_level_with_it() {
    let world = Field::zombie_at([4.5, 10.0, 0.5]);
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    change(&mut blaze.mob, |own| {
        own.next_height_offset_change_tick = 50
    });
    blaze.living.body.motion = [0.0, -0.2, 0.0];
    let ((), out) = blaze.tick(&world, super::custom_server_ai_step);
    let tenths = f64::from(0.3_f32);
    assert_eq!(
        blaze.living.body.motion[1].to_bits(),
        (-0.2 + (tenths + 0.2) * tenths).to_bits()
    );
    assert!(out.needs_sync);
    let level = Field::zombie_at([4.5, 0.0, 0.5]);
    blaze.living.body.motion = [0.0; 3];
    let ((), out) = blaze.tick(&level, super::custom_server_ai_step);
    assert_eq!(blaze.living.body.motion, [0.0; 3]);
    assert!(!out.needs_sync);
}

/// Off the ground and falling its fall is six tenths; rising, or on the ground, as it
/// was.
#[test]
fn a_blaze_falls_slowly_and_rises_as_it_was_pushed() {
    let world = Field::default();
    let mut blaze = Run::at(BLAZE, [0.5, 5.0, 0.5], false);
    blaze.living.body.motion = [0.1, -0.5, 0.1];
    blaze.tick(&world, |tick| tick.mob_type.before_ai_step(tick));
    assert_eq!(blaze.living.body.motion, [0.1, -0.5 * 0.6, 0.1]);
    blaze.living.body.motion = [0.0, 0.25, 0.0];
    blaze.tick(&world, |tick| tick.mob_type.before_ai_step(tick));
    assert_eq!(blaze.living.body.motion, [0.0, 0.25, 0.0]);
    let mut grounded = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    grounded.living.body.motion = [0.0, -0.0784, 0.0];
    grounded.tick(&world, |tick| tick.mob_type.before_ai_step(tick));
    assert_eq!(grounded.living.body.motion, [0.0, -0.0784, 0.0]);
}

/// Its light-dependent value is 1, so its idle time counts two more in the dark too.
#[test]
fn a_blaze_idles_twice_as_fast_in_the_dark() {
    let world = Field::default();
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    blaze.mob.body.idle.no_action_since = Some(0);
    let before = blaze.mob.body.idle_gained.gained;
    blaze.tick(&world, |tick| {
        tick.mob_type.before_ai_step(tick);
        crate::mobs::body::burning::monster_idles(tick);
    });
    assert_eq!(blaze.mob.body.idle_gained.gained, before + 2);
}

/// In water it is hurt by drowning's 1 each step; out of it, not.
#[test]
fn a_blaze_in_water_is_hurt_by_one_each_step() {
    let world = Field::default();
    let mut blaze = Run::at(BLAZE, [0.5, 0.0, 0.5], true);
    blaze.tick(&world, |tick| tick.mob_type.after_ai_step(tick));
    assert_eq!(blaze.mob.body.vitals.health, 20.0);
    blaze.living.fluids.water_height = 0.5;
    blaze.tick(&world, |tick| tick.mob_type.after_ai_step(tick));
    assert_eq!(blaze.mob.body.vitals.health, 19.0);
}

/// Its goals (the attack at 4, the way home at 5, the stroll at 7, the looks at 8) and
/// targets (who hurt it, alerting blazes, at 1; players at 2); its maluses: water −1,
/// lava 8, fire none.
#[test]
fn a_blaze_s_goals_targets_and_maluses_are_its_own() {
    let mut setup = Setup::default();
    of(BLAZE).register_goals(&mut setup);
    let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
    assert_eq!(goals, [4, 5, 7, 8, 8]);
    let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
    assert_eq!(targets, [1, 2]);
    let malus = |kind: PathType| setup.config.malus[kind as usize];
    assert_eq!(malus(PathType::Water), Some(-1.0));
    assert_eq!(malus(PathType::Lava), Some(8.0));
    assert_eq!(malus(PathType::FireInNeighbor), Some(0.0));
    assert_eq!(malus(PathType::Fire), Some(0.0));
    assert!(crate::mobs::ai::targets::comes_to_alert(BLAZE, BLAZE));
}

/// Immune to fire; its attributes; its sounds; 10 experience; unlit as it is made.
#[test]
fn a_blaze_is_fire_immune_of_its_attributes_sounds_and_ten_experience() {
    use clustine_data::sounds;
    let info = EntityTypeInfo::of(entity_types::BLAZE);
    assert!(info.fire_immune);
    let supplied = |id: i32| crate::mobs::body::attributes::supplied(BLAZE, attribute(id)).unwrap();
    assert_eq!(supplied(ids::ATTACK_DAMAGE), 6.0);
    assert_eq!(supplied(ids::MOVEMENT_SPEED), f64::from(0.23_f32));
    assert_eq!(supplied(ids::FOLLOW_RANGE), 48.0);
    let mob = Mob::fresh();
    let blaze = of(BLAZE);
    assert_eq!(
        blaze.ambient_sound(&mob),
        Some(sounds::ENTITY_BLAZE_AMBIENT)
    );
    let fall = DamageSource::of(DamageType::FALL);
    assert_eq!(
        blaze.hurt_sound(&mob, &fall),
        Some(sounds::ENTITY_BLAZE_HURT)
    );
    assert_eq!(blaze.death_sound(&mob), Some(sounds::ENTITY_BLAZE_DEATH));
    let mut draws = Random::of_entity(1, KEY, 0);
    assert_eq!(blaze.experience_reward(BLAZE, &mob, &mut draws), 10);
    assert_eq!(blaze.shown(&mob), [(FLAGS_INDEX, DataShown::Byte(0), true)]);
}

// ---------------------------------------------------------------------------------
// The wither skeleton.

/// A level's random of fixed values, which notes how many floats were drawn.
struct Level {
    random: Random,
    floats: u32,
}

impl Draws for Level {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.random.next_int(bound)
    }

    fn next_float(&mut self) -> f32 {
        self.floats += 1;
        self.random.next_float()
    }

    fn next_double(&mut self) -> f64 {
        self.random.next_double()
    }

    fn next_bool(&mut self) -> bool {
        self.random.next_bool()
    }
}

/// A mob of the type `kind` made by an egg's `finalizeSpawn` from `level` at
/// `difficulty`.
fn made_by_egg(kind: u16, level: &mut dyn Draws, difficulty: (f32, f32)) -> Mob {
    let mut own = Random::of_entity(3, KEY, 0);
    let (_, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome: None,
            at: BlockPos::new(0, 0, 0),
            day: 0,
            game_time: 0,
            reason: SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty,
            companions: None,
        },
    );
    mob
}

/// An egg's wither skeleton holds a stone sword and wears nothing; its attack's base is
/// 4; its pick-up is drawn against `0.55 × special` as a skeleton's is.
#[test]
fn an_egg_s_wither_skeleton_holds_a_stone_sword_wears_nothing_and_attacks_from_four() {
    for special in [0.0_f32, 1.0] {
        let mut level = Level {
            random: Random::of_entity(5, EntityId(1), 0),
            floats: 0,
        };
        let mob = made_by_egg(WITHER_SKELETON, &mut level, (1.0, special));
        let equipment = &mob.body.equipment.slots;
        let hand = equipment[usize::from(slots::MAIN_HAND)].as_ref();
        assert_eq!(hand.map(|stack| stack.item), Some(items::STONE_SWORD));
        for slot in [slots::HEAD, slots::CHEST, slots::LEGS, slots::FEET] {
            assert!(equipment[usize::from(slot)].is_none(), "{slot}");
        }
        assert!(equipment[usize::from(slots::OFF_HAND)].is_none());
        let base = mob
            .body
            .attributes
            .made(attribute(ids::ATTACK_DAMAGE))
            .map(|instance| instance.base);
        assert_eq!(base, Some(SPAWNED_ATTACK));
        if special == 0.0 {
            assert!(!mob.body.flags.picks_up_loot, "never at no special");
        }
    }
    // Before its spawn, its attack is the type's 2.
    let supplied =
        crate::mobs::body::attributes::supplied(WITHER_SKELETON, attribute(ids::ATTACK_DAMAGE));
    assert_eq!(supplied, Some(2.0));
}

/// Its targets: the piglins it sees at 3 first, then the skeleton's (who hurt it at 1,
/// players at 2, iron golems and baby turtles at 3); the skeleton's goals; a malus of 8
/// on lava.
#[test]
fn a_wither_skeleton_targets_piglins_first_and_then_as_a_skeleton() {
    let mut setup = Setup::default();
    of(WITHER_SKELETON).register_goals(&mut setup);
    let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
    assert_eq!(targets, [3, 1, 2, 3, 3]);
    let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
    assert_eq!(goals, [2, 3, 3, 5, 6, 6, 4]);
    assert_eq!(setup.config.malus[PathType::Lava as usize], Some(8.0));
    assert_eq!(
        PIGLINS,
        [
            entity_types::PIGLIN as u16,
            entity_types::PIGLIN_BRUTE as u16
        ]
    );
}

/// Its hit carries wither for 200 ticks, of amplifier 0, shown; wither does not take on
/// it.
#[test]
fn a_wither_skeleton_s_hit_withers_for_two_hundred_ticks_and_wither_spares_it() {
    let world = Field::zombie_at([1.5, 0.0, 0.5]);
    let mut skeleton = Run::at(WITHER_SKELETON, [0.5, 0.0, 0.5], true);
    let target = crate::mobs::ai::sensing::Looked::of_seen(&world.entity(TARGET).unwrap());
    let ((), out) = skeleton.ai(&world, |ai| {
        let mob_type = ai.tick.mob_type;
        mob_type.do_hurt_target(ai, &target);
    });
    let [(key, hurt)] = &strikes(&out)[..] else {
        panic!("one hit: {:?}", out.local);
    };
    assert_eq!(*key, TARGET);
    let [effect] = &hurt.effects[..] else {
        panic!("one effect: {:?}", hurt.effects);
    };
    assert_eq!(effect.effect, "minecraft:wither");
    assert_eq!(
        (effect.amplifier, effect.duration),
        (0, wither_skeleton::WITHER_TICKS)
    );
    assert!(effect.visible && effect.show_icon && !effect.ambient);
    assert!(!can_be_affected(WITHER_SKELETON, Effect::WITHER));
    assert!(can_be_affected(
        entity_types::SKELETON as u16,
        Effect::WITHER
    ));
}

/// Its own sounds, 5 experience with its sword's 1 to 3, its size.
#[test]
fn a_wither_skeleton_has_its_own_sounds_size_and_five_experience() {
    use clustine_data::sounds;
    let info = EntityTypeInfo::of(entity_types::WITHER_SKELETON);
    assert_eq!((info.width, info.height), (0.7, 2.4));
    assert!(info.fire_immune);
    let mut mob = Mob::fresh();
    let skeleton = of(WITHER_SKELETON);
    assert_eq!(
        skeleton.ambient_sound(&mob),
        Some(sounds::ENTITY_WITHER_SKELETON_AMBIENT)
    );
    let fall = DamageSource::of(DamageType::FALL);
    assert_eq!(
        skeleton.hurt_sound(&mob, &fall),
        Some(sounds::ENTITY_WITHER_SKELETON_HURT)
    );
    assert_eq!(
        skeleton.death_sound(&mob),
        Some(sounds::ENTITY_WITHER_SKELETON_DEATH)
    );
    assert_eq!(
        skeleton.step_sound(&mob),
        Some(sounds::ENTITY_WITHER_SKELETON_STEP)
    );
    let mut draws = Random::of_entity(1, KEY, 0);
    assert_eq!(
        skeleton.experience_reward(WITHER_SKELETON, &mob, &mut draws),
        5
    );
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(clustine_world::ItemStack::new(items::STONE_SWORD, 1));
    let mut replay = Random::of_entity(1, KEY, 0);
    let mut draws = replay.clone();
    let sword = 1 + replay.next_int(3) as i32;
    assert_eq!(
        skeleton.experience_reward(WITHER_SKELETON, &mob, &mut draws),
        5 + sword
    );
}

// ---------------------------------------------------------------------------------
// Spawn rules.

/// A field for the spawn rules: one block at y 63 over air, one light everywhere.
struct Rules {
    ground: clustine_data::BlockState,
    light: Light,
}

impl SpawnWorld for Rules {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        Some(if pos == BlockPos::new(0, 63, 0) {
            self.ground
        } else {
            blocks::AIR
        })
    }

    fn light(&self, _: BlockPos) -> Option<Light> {
        Some(self.light)
    }

    fn sky_darken(&self) -> i32 {
        0
    }

    fn biome(&self, _: BlockPos) -> Option<Biome> {
        None
    }

    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        Some(64)
    }

    fn game_seed(&self) -> i64 {
        0
    }

    fn day(&self) -> u64 {
        6_000
    }

    fn dimension(&self) -> clustine_world::Dimension {
        clustine_world::Dimension::Nether
    }
}

/// The blaze's rule is the mob rules alone, whatever the light (a valid block below);
/// the wither skeleton's a monster's, which the light of 15 refuses.
#[test]
fn a_blaze_spawns_in_any_light_on_bricks_and_a_wither_skeleton_only_in_the_dark() {
    let pos = BlockPos::new(0, 64, 0);
    let bright = Rules {
        ground: blocks::NETHER_BRICKS,
        light: Light { sky: 15, block: 15 },
    };
    let dark = Rules {
        ground: blocks::NETHER_BRICKS,
        light: Light { sky: 0, block: 0 },
    };
    let mut draws = Random::of_entity(9, KEY, 0);
    assert!(check_spawn_rules(BLAZE, &bright, pos, &mut draws));
    assert!(check_spawn_rules(BLAZE, &dark, pos, &mut draws));
    let air = Rules {
        ground: blocks::AIR,
        light: Light { sky: 0, block: 0 },
    };
    assert!(!check_spawn_rules(BLAZE, &air, pos, &mut draws));
    assert!(check_spawn_rules(WITHER_SKELETON, &dark, pos, &mut draws));
    assert!(!check_spawn_rules(
        WITHER_SKELETON,
        &bright,
        pos,
        &mut draws
    ));
    // Both are built, so that an egg and the fortress's list make them.
    assert!(crate::mobs::types::is_built(BLAZE));
    assert!(crate::mobs::types::is_built(WITHER_SKELETON));
}
