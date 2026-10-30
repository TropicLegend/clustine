//! The ghast from the game's rules (`Ghast`, `Ghast$GhastShootFireballGoal`,
//! `Ghast$GhastMoveControl`, `Ghast$RandomFloatAroundGoal`, `Ghast$GhastLookGoal`, d
//! javap; `docs/groundwork/p-brief.md`, section 5.9), not from the code: the charge's
//! rhythm and its level events, where and along what its fireball is shot, the charge
//! counted down out of sight, its look, its float and its impulses, its hurt by a
//! fireball a player sent back and its immunity to any other, its spawn rule's draw, and
//! what it shows.

use clustine_data::{EntityTypeInfo, entity_types};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId};

use super::{
    Charging, GHAST, GhastLookGoal, GhastShootFireballGoal, RandomFloatAroundGoal, SHOOT_EVENT,
    WARN_EVENT, change, check_spawn_rules, state_of, tick_move_control, view_vector,
};
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::{Ai, Operation, Setup};
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::of;
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{self, Vec3, World};
use crate::region::Random;
use crate::shown::DataShown;

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
            clustine_data::blocks::STONE
        } else {
            clustine_data::blocks::AIR
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
        None
    }
}

impl Field {
    /// A field with a zombie of 20 health standing at `feet`, the ghast's target.
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

/// A ghast's body at rest at `at`, its first tick behind it.
fn body(at: Vec3) -> Living {
    let info = EntityTypeInfo::of(entity_types::GHAST);
    let body = Moving {
        stands_on_lava: false,
        position: at,
        motion: [0.0; 3],
        on_ground: false,
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
            kind: GHAST,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living
}

/// A ghast ticked again and again, its stream carried from one tick to the next.
struct Ghast {
    mob: Mob,
    living: Living,
    yaw: f32,
    pitch: f32,
    lived: u32,
    stream: Random,
}

impl Ghast {
    /// A ghast of full health at `at` whose target is the field's.
    fn at(at: Vec3) -> Self {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 10.0;
        mob.body.vitals.air = MAX_AIR;
        mob.body.combat.target = Some(TARGET);
        Self {
            mob,
            living: body(at),
            yaw: 0.0,
            pitch: 0.0,
            lived: 100,
            stream: Random::of_entity(7, KEY, 1),
        }
    }

    /// `f` in the ghast's AI step; what it gave, what the step made and the draws taken.
    fn ai<R>(
        &mut self,
        world: &Field,
        f: impl FnOnce(&mut Ai<'_, '_>) -> R,
    ) -> (R, MobOutput, u64) {
        let mut setup = Setup::default();
        of(GHAST).register_goals_of(&self.mob, &mut setup);
        let config = setup.config;
        let start = self.stream.clone();
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: GHAST,
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
            mob_type: of(GHAST),
            world,
            draws: &mut self.stream,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let given = {
            let mut ai = Ai {
                tick: &mut tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: false,
            };
            f(&mut ai)
        };
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.pitch = tick.pitch;
        let out = tick.out;
        (given, out, self.stream.draws_since(&start))
    }

    /// One tick of the shooting goal.
    fn shoot_tick(&mut self, world: &Field, state: &mut Charging) -> MobOutput {
        let ((), out, _) = self.ai(world, |ai| GhastShootFireballGoal.tick(state, ai));
        self.lived += 1;
        out
    }

    fn charging_shown(&self) -> DataShown {
        let shown = of(GHAST).shown(&self.mob);
        let [(index, value, _)] = &shown[..] else {
            panic!("one value shown: {shown:?}");
        };
        assert_eq!(*index, 16);
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

#[test]
fn the_ghast_warns_at_its_tenth_charge_and_shoots_a_large_fireball_at_its_twentieth() {
    let world = Field::zombie_at([10.5, 10.0, 0.5]);
    let mut ghast = Ghast::at([0.5, 10.0, 0.5]);
    let mut state = Charging::default();
    assert!(
        ghast
            .ai(&world, |ai| GhastShootFireballGoal.can_use(&mut state, ai))
            .0
    );
    ghast.ai(&world, |ai| GhastShootFireballGoal.start(&mut state, ai));
    for charge in 1..=20 {
        let out = ghast.shoot_tick(&world, &mut state);
        let events = level_events(&out);
        match charge {
            10 => assert_eq!(events, [WARN_EVENT]),
            20 => assert_eq!(events, [SHOOT_EVENT]),
            _ => assert!(events.is_empty(), "{charge}: {events:?}"),
        }
        // It shows itself charging above 10, and after the shot not.
        let charging = (11..20).contains(&charge);
        assert_eq!(
            ghast.charging_shown(),
            DataShown::Boolean(charging),
            "{charge}"
        );
        if charge < 20 {
            assert_eq!(state.charge_time, charge);
            assert!(fireballs(&out).is_empty());
            continue;
        }
        assert_eq!(state.charge_time, -40);
        // Four blocks ahead along its look (south, at yaw 0) and half a block above its
        // middle, towards the zombie's middle.
        let height = f64::from(EntityTypeInfo::of(entity_types::GHAST).height);
        let zombie = f64::from(EntityTypeInfo::of(entity_types::ZOMBIE).height);
        let at = [0.5, 10.0 + height * 0.5 + 0.5, 0.5 + 4.0];
        let towards = [10.5 - at[0], 10.0 + zombie * 0.5 - at[1], 0.5 - at[2]];
        let length = physics::length_sqr(towards).sqrt();
        let direction = towards.map(|part| part / length);
        let [(kind, from, along, power)] = fireballs(&out)[..] else {
            panic!("one fireball: {:?}", out.local);
        };
        assert_eq!(i32::from(kind), entity_types::FIREBALL);
        assert_eq!(from.map(f64::to_bits), at.map(f64::to_bits));
        for (got, want) in along.iter().zip(direction) {
            assert!(
                (got - want).abs() < 1e-12,
                "{along:?} against {direction:?}"
            );
        }
        assert_eq!(power, 1);
    }
    // Then forty ticks and twenty more of charging before the next.
    let mut shots = Vec::new();
    for tick in 0..60 {
        if !fireballs(&ghast.shoot_tick(&world, &mut state)).is_empty() {
            shots.push(tick);
        }
    }
    assert_eq!(shots, [59]);
}

#[test]
fn the_ghast_s_charge_counts_down_while_its_target_is_out_of_sight_or_reach() {
    // Behind a wall at x = 5.
    let mut world = Field::zombie_at([10.5, 10.0, 0.5]);
    let mut ghast = Ghast::at([0.5, 10.0, 0.5]);
    let mut state = Charging { charge_time: 15 };
    world.wall = true;
    let out = ghast.shoot_tick(&world, &mut state);
    assert_eq!(state.charge_time, 14);
    assert!(level_events(&out).is_empty());
    assert_eq!(ghast.charging_shown(), DataShown::Boolean(true));
    for _ in 0..20 {
        ghast.shoot_tick(&world, &mut state);
    }
    assert_eq!(state.charge_time, 0);
    assert_eq!(ghast.charging_shown(), DataShown::Boolean(false));
    // 64 blocks away is out of reach, in sight or not.
    let far = Field::zombie_at([64.5, 10.0, 0.5]);
    let mut state = Charging { charge_time: 5 };
    ghast.shoot_tick(&far, &mut state);
    assert_eq!(state.charge_time, 4);
}

#[test]
fn the_ghast_stops_showing_its_charge_when_its_goal_stops() {
    let world = Field::zombie_at([10.5, 10.0, 0.5]);
    let mut ghast = Ghast::at([0.5, 10.0, 0.5]);
    change(&mut ghast.mob, |own| own.charging = true);
    let mut state = Charging { charge_time: 12 };
    ghast.ai(&world, |ai| GhastShootFireballGoal.stop(&mut state, ai));
    assert_eq!(ghast.charging_shown(), DataShown::Boolean(false));
    // Its goal is used only while it has a target.
    ghast.mob.body.combat.target = None;
    assert!(
        !ghast
            .ai(&world, |ai| GhastShootFireballGoal.can_use(&mut state, ai))
            .0
    );
}

#[test]
fn the_ghast_shoots_along_its_head_s_yaw_four_blocks_ahead() {
    let world = Field::zombie_at([10.5, 10.0, 0.5]);
    let mut ghast = Ghast::at([0.5, 10.0, 0.5]);
    ghast.mob.body.head.head_yaw = -90.0;
    let mut state = Charging { charge_time: 19 };
    let out = ghast.shoot_tick(&world, &mut state);
    let [(_, from, _, _)] = fireballs(&out)[..] else {
        panic!("one fireball: {:?}", out.local);
    };
    let view = view_vector(-90.0, 0.0);
    // Yaw −90 looks east, along +x.
    assert!(
        (view[0] - 1.0).abs() < 1e-6 && view[2].abs() < 1e-6,
        "{view:?}"
    );
    assert_eq!(from[0].to_bits(), (0.5 + view[0] * 4.0).to_bits());
    assert_eq!(from[2].to_bits(), (0.5 + view[2] * 4.0).to_bits());
}

#[test]
fn the_ghast_faces_its_target_within_64_and_else_the_way_it_drifts() {
    let world = Field::zombie_at([10.5, 10.0, 0.5]);
    let mut ghast = Ghast::at([0.5, 10.0, 0.5]);
    ghast.ai(&world, |ai| GhastLookGoal.tick(&mut (), ai));
    // Towards +x: yaw −90.
    assert!((ghast.yaw + 90.0).abs() < 1e-4, "{}", ghast.yaw);
    assert_eq!(ghast.mob.body.head.body_yaw, ghast.yaw);
    // No target: along its motion, here towards −z, a half turn.
    ghast.mob.body.combat.target = None;
    ghast.living.body.motion = [0.0, 0.0, -0.1];
    ghast.ai(&world, |ai| GhastLookGoal.tick(&mut (), ai));
    assert!((ghast.yaw.abs() - 180.0).abs() < 1e-4, "{}", ghast.yaw);
    // A target 64 away or more: as it is.
    let far = Field::zombie_at([70.5, 10.0, 0.5]);
    ghast.mob.body.combat.target = Some(TARGET);
    ghast.yaw = 12.0;
    ghast.ai(&far, |ai| GhastLookGoal.tick(&mut (), ai));
    assert_eq!(ghast.yaw, 12.0);
}

#[test]
fn the_ghast_floats_to_a_point_within_16_each_way_of_three_draws() {
    let world = Field::default();
    let mut ghast = Ghast::at([0.5, 30.0, 0.5]);
    ghast.mob.body.combat.target = None;
    assert!(
        ghast
            .ai(&world, |ai| RandomFloatAroundGoal.can_use(&mut (), ai))
            .0
    );
    let mut expected = ghast.stream.clone();
    let parts: Vec<f64> = (0..3)
        .map(|_| f64::from((expected.next_float() * 2.0 - 1.0) * 16.0))
        .collect();
    let ((), _, drawn) = ghast.ai(&world, |ai| RandomFloatAroundGoal.start(&mut (), ai));
    assert_eq!(drawn, 3);
    let movement = &ghast.mob.body.controls.movement;
    assert_eq!(movement.operation, Operation::MoveTo);
    assert_eq!(
        movement.wanted.map(f64::to_bits),
        [0.5 + parts[0], 30.0 + parts[1], 0.5 + parts[2]].map(f64::to_bits)
    );
    // A point wanted within a block to 60 holds it; one nearer or further does not.
    for (wanted, used) in [
        ([0.5, 30.0, 10.5], false),
        ([0.5, 30.0, 0.9], true),
        ([0.5, 30.0, 70.5], true),
    ] {
        ghast.mob.body.controls.movement.wanted = wanted;
        let can = ghast
            .ai(&world, |ai| RandomFloatAroundGoal.can_use(&mut (), ai))
            .0;
        assert_eq!(can, used, "{wanted:?}");
    }
}

#[test]
fn the_ghast_s_move_control_pushes_it_at_five_thirds_of_its_flying_speed_every_few_ticks() {
    let world = Field::default();
    let mut ghast = Ghast::at([0.5, 30.0, 0.5]);
    ghast.mob.body.controls.movement.operation = Operation::MoveTo;
    ghast.mob.body.controls.movement.wanted = [0.5, 30.0, 10.5];
    let mut expected = ghast.stream.clone();
    let wait = expected.next_int(5) as i32 + 2;
    let ((), _, drawn) = ghast.ai(&world, tick_move_control);
    assert_eq!(drawn, 1);
    let push = 0.06 * 5.0 / 3.0;
    assert_eq!(
        ghast.living.body.motion.map(f64::to_bits),
        [0.0, 0.0, push].map(f64::to_bits)
    );
    // `floatDuration` was 0: counted to −1, then the draw added.
    assert_eq!(state_of(&ghast.mob).float_duration, wait - 1);
    // Until it runs out, nothing more.
    for _ in 0..wait - 1 {
        let ((), _, drawn) = ghast.ai(&world, tick_move_control);
        assert_eq!(drawn, 0);
    }
    let ((), _, drawn) = ghast.ai(&world, tick_move_control);
    assert_eq!(drawn, 1);
    assert_eq!(
        ghast.living.body.motion[2].to_bits(),
        (push + push).to_bits()
    );
    // Where its box cannot sweep there, it waits.
    let walled = Field {
        wall: true,
        ..Field::default()
    };
    let mut ghast = Ghast::at([0.5, 30.0, 0.5]);
    ghast.mob.body.controls.movement.operation = Operation::MoveTo;
    ghast.mob.body.controls.movement.wanted = [10.5, 30.0, 0.5];
    ghast.ai(&walled, tick_move_control);
    assert_eq!(ghast.living.body.motion, [0.0; 3]);
    assert_eq!(ghast.mob.body.controls.movement.operation, Operation::Wait);
}

fn source(kind: &'static str, direct: i32, player: Option<u128>) -> DamageSource {
    DamageSource {
        kind: DamageType(kind),
        cause: Some(Causer {
            key: EntityId(5),
            kind: if player.is_some() {
                entity_types::PLAYER as u16
            } else {
                GHAST
            },
            position: [0.0; 3],
            player,
            living: true,
        }),
        direct: Some(Causer {
            key: EntityId(6),
            kind: direct as u16,
            position: [0.0; 3],
            player: None,
            living: false,
        }),
        position: None,
        projectile_motion: Some([1.0, 0.0, 0.0]),
        clean_water: false,
    }
}

/// Whether a ghast of full health takes the hurt `source` of `amount`, and its health
/// after.
fn hurt(source: &DamageSource, amount: f32) -> (bool, f32) {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    let mut motion = [0.0; 3];
    let mut draws = Random::of_entity(1, KEY, 0);
    let mut out = MobOutput::default();
    let took = {
        let mut struck = crate::mobs::body::hurt::Struck {
            kind: GHAST,
            mob: &mut mob,
            mob_type: of(GHAST),
            position: [0.5, 10.0, 0.5],
            motion: &mut motion,
            on_ground: false,
            on_fire: false,
            vehicle: None,
            carries: false,
            leashed: false,
            in_liquid: false,
            moment: 10,
            game_time: 10,
            draws: &mut draws,
            out: &mut out,
        };
        crate::mobs::body::hurt::hurt_server(&mut struck, source, amount)
    };
    (took, mob.body.vitals.health)
}

#[test]
fn a_large_fireball_a_player_sent_back_hurts_a_ghast_by_1000_and_any_other_fireball_not() {
    let sent_back = source("minecraft:fireball", entity_types::FIREBALL, Some(9));
    assert_eq!(hurt(&sent_back, 6.0), (true, 0.0));
    // Its own, or another ghast's: fire, to which it is immune.
    let own = source("minecraft:fireball", entity_types::FIREBALL, None);
    assert_eq!(hurt(&own, 6.0), (false, 10.0));
    // A player's small fireball is no large one.
    let small = source("minecraft:fireball", entity_types::SMALL_FIREBALL, Some(9));
    assert_eq!(hurt(&small, 5.0), (false, 10.0));
    // Anything else hurts it as it would.
    let arrow = source("minecraft:arrow", entity_types::ARROW, Some(9));
    assert_eq!(hurt(&arrow, 4.0), (true, 6.0));
}

/// The draws of a spawn rule: a fixed `nextInt`.
struct Fixed(u32);

impl clustine_world::generation::Draws for Fixed {
    fn next_int(&mut self, _: u32) -> u32 {
        self.0
    }
    fn next_float(&mut self) -> f32 {
        0.0
    }
    fn next_double(&mut self) -> f64 {
        0.0
    }
    fn next_bool(&mut self) -> bool {
        false
    }
}

/// The Nether's floor for a spawn rule: netherrack below y = 64, air above.
struct Netherrack;

impl crate::spawning::rules::SpawnWorld for Netherrack {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        Some(if pos.y < 64 {
            clustine_data::BlockState::parse("minecraft:netherrack").expect("netherrack")
        } else {
            clustine_data::blocks::AIR
        })
    }
    fn light(&self, _: BlockPos) -> Option<crate::spawning::rules::Light> {
        None
    }
    fn sky_darken(&self) -> i32 {
        0
    }
    fn biome(&self, _: BlockPos) -> Option<clustine_world::Biome> {
        None
    }
    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        Some(64)
    }
    fn game_seed(&self) -> i64 {
        0
    }
    fn day(&self) -> u64 {
        0
    }
}

#[test]
fn a_ghast_spawns_on_a_valid_block_one_draw_in_twenty() {
    let on = BlockPos::new(0, 64, 0);
    assert!(check_spawn_rules(&Netherrack, on, &mut Fixed(0)));
    assert!(!check_spawn_rules(&Netherrack, on, &mut Fixed(1)));
    assert!(!check_spawn_rules(&Netherrack, on, &mut Fixed(19)));
    // And on nothing it can spawn on.
    assert!(!check_spawn_rules(
        &Netherrack,
        BlockPos::new(0, 70, 0),
        &mut Fixed(0)
    ));
}

#[test]
fn the_ghast_registers_its_float_its_look_its_shot_and_its_target() {
    let mut setup = Setup::default();
    of(GHAST).register_goals_of(&Mob::fresh(), &mut setup);
    let priorities: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
    assert_eq!(priorities, [5, 7, 7]);
    let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
    assert_eq!(targets, [1]);
    assert!(super::super::is_built(GHAST));
}
