//! Part S9's types from the game's rules (`docs/groundwork/n9-plan.md`, sections 8.1 and
//! 8.4; the dumps of `Slime`, `AbstractCubeMob` and its goals, `Enderman`, `Silverfish`,
//! `Endermite`, `Phantom`), not from the code: each type's goals and what they draw, the
//! slime's sizes and split, the enderman's teleports, refusals and blocks, the
//! silverfish's merge and wake, the endermite's life, the phantom's circling and burning.

use std::collections::BTreeMap;

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, sounds};
use clustine_world::generation::Draws;
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId};

use super::{SpawnReason, Spawning, of};
use crate::living::{Kind, Living};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup, server_ai_step};
use crate::mobs::body::Event;
use crate::mobs::tick::{MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;
use crate::shown::DataShown;

const KEY: EntityId = EntityId(40);

const fn kind(id: i32) -> u16 {
    id as u16
}

const SLIME: u16 = kind(entity_types::SLIME);

/// A floor of stone below y 0 and air above, with the blocks a test sets, its light, the
/// sky's darkening, the day, and the players and entities about.
struct Field {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    light: Option<LightAt>,
    darken: i32,
    day: u64,
    players: Vec<SeenPlayer>,
    entities: Vec<(Entity, Aabb)>,
    /// What is below y 0.
    floor: BlockState,
}

impl Field {
    fn new() -> Self {
        Self {
            floor: blocks::STONE,
            blocks: BTreeMap::new(),
            light: Some(LightAt { sky: 0, block: 0 }),
            darken: 11,
            day: 18_000,
            players: Vec::new(),
            entities: Vec::new(),
        }
    }

    fn set(&mut self, pos: BlockPos, state: BlockState) {
        self.blocks.insert((pos.x, pos.y, pos.z), state);
    }
}

impl World for Field {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if pos.y < -64 || pos.y >= 320 {
            return None;
        }
        Some(match self.blocks.get(&(pos.x, pos.y, pos.z)) {
            Some(state) => *state,
            None if pos.y < 0 => self.floor,
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
        self.light
    }

    fn sky_darken(&self) -> i32 {
        self.darken
    }

    fn day(&self) -> u64 {
        self.day
    }
}

/// A player standing at `feet`, creative where `creative`.
fn player(key: u64, feet: Vec3, creative: bool) -> SeenPlayer {
    SeenPlayer {
        key: EntityId(key),
        uuid: u128::from(key),
        position: feet,
        eye_height: 1.62,
        yaw: 0.0,
        pitch: 0.0,
        main_hand: None,
        off_hand: None,
        sneaking: false,
        creative,
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

/// A mob of the type `id` standing at `feet` as a record and its box, of `mob`.
fn record(key: u64, id: u16, feet: Vec3, mob: &Mob) -> (Entity, Aabb) {
    let info = EntityTypeInfo::of(i32::from(id));
    let entity = crate::mobs::making::fresh_record(
        EntityId(key),
        id,
        uuid::Uuid::from_u128(u128::from(key)),
        feet,
        0.0,
        mob,
    );
    let size =
        super::own_size(&entity).map_or((info.width, info.height), |own| (own.width, own.height));
    (entity, Aabb::of_entity(feet, size.0, size.1))
}

/// A body of the type `kind` of `mob`'s size at `feet`, on the ground where `on_ground`.
fn living(kind: u16, mob: &Mob, feet: Vec3, on_ground: bool) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let size = of(kind).dimensions(mob);
    let (width, height, eye) = size.map_or((info.width, info.height, info.eye_height), |own| {
        (own.width, own.height, own.eye_height)
    });
    let body = Moving {
        position: feet,
        motion: [0.0; 3],
        on_ground,
        fall_distance: 0.0,
        width,
        height,
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
            kind,
            hurts: Vec::new(),
        },
        eye,
    );
    living.first_tick = false;
    living.air = crate::living::MAX_AIR;
    living
}

/// One mob ticked step by step in a field, as a region would: what it keeps between
/// steps is its record and its body.
struct Run {
    kind: u16,
    mob: Mob,
    living: Living,
    yaw: f32,
    pitch: f32,
    lived: u32,
    seed: u64,
}

impl Run {
    fn new(kind: u16, mob: Mob, feet: Vec3, on_ground: bool) -> Self {
        let living = living(kind, &mob, feet, on_ground);
        Self {
            kind,
            mob,
            living,
            yaw: 0.0,
            pitch: 0.0,
            lived: 0,
            seed: 7,
        }
    }

    /// Its stream in its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(self.seed, KEY, self.lived)
    }

    fn tick<'w>(&self, world: &'w Field, draws: &'w mut Random) -> MobTick<'w> {
        MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            yaw: self.yaw,
            pitch: self.pitch,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: true,
            mob: self.mob.clone(),
            living: self.living.clone(),
            mob_type: of(self.kind),
            world,
            draws,
            out: MobOutput::default(),
        }
    }

    /// `f` on its next tick, kept; the draws `f` made and what the tick put out.
    fn with_tick(&mut self, world: &Field, f: impl FnOnce(&mut MobTick<'_>)) -> (u64, MobOutput) {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = self.tick(world, &mut draws);
        f(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.pitch = tick.pitch;
        let out = tick.out;
        self.lived += 1;
        (draws.draws_since(&start), out)
    }

    /// One AI step (`serverAiStep`): its draws and output.
    fn step(&mut self, world: &Field) -> (u64, MobOutput) {
        self.with_tick(world, server_ai_step)
    }

    /// `f` on the mob's AI as its next step would have it, kept; the draws `f` made.
    fn with_ai<R>(&mut self, world: &Field, f: impl FnOnce(&mut Ai<'_, '_>) -> R) -> (u64, R) {
        let (drawn, result, _) = self.with_ai_out(world, f);
        (drawn, result)
    }

    /// [`Run::with_ai`] with what the tick put out.
    fn with_ai_out<R>(
        &mut self,
        world: &Field,
        f: impl FnOnce(&mut Ai<'_, '_>) -> R,
    ) -> (u64, R, MobOutput) {
        let mut result = None;
        let (drawn, out) = self.with_tick(world, |tick| {
            let mut setup = Setup::default();
            tick.mob_type
                .register_goals_of(&tick.mob.clone(), &mut setup);
            let config = setup.config;
            let mut ai = Ai {
                tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: false,
            };
            result = Some(f(&mut ai));
        });
        self.lived -= 1;
        (drawn, result.expect("ran"), out)
    }
}

/// A level's random that records what is drawn of it, from a stream.
struct Recorded {
    random: Random,
    calls: Vec<&'static str>,
}

impl Recorded {
    fn new(seed: u64) -> Self {
        Self {
            random: Random::of_entity(seed, EntityId(1), 0),
            calls: Vec::new(),
        }
    }
}

impl Draws for Recorded {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push("int");
        self.random.next_int(bound)
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push("float");
        self.random.next_float()
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push("double");
        self.random.next_double()
    }

    fn next_bool(&mut self) -> bool {
        self.calls.push("bool");
        self.random.next_bool()
    }
}

/// A mob of the type `kind` made by an egg's `finalizeSpawn` from `level`.
fn made_by_egg(kind: u16, level: &mut dyn Draws, special: f32) -> Mob {
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
            difficulty: (0.0, special),
            companions: None,
        },
    );
    mob
}

/// The sounds a tick put out, by id.
fn sounds_of(out: &MobOutput) -> Vec<i32> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

mod slime {
    use super::*;
    use crate::mobs::types::cube_mob;

    fn slime(size: i32) -> Mob {
        let mut mob = Mob::fresh();
        cube_mob::set_size(SLIME, &mut mob, size, true);
        mob
    }

    fn attribute(mob: &Mob, id: i32) -> f64 {
        mob.body
            .attributes
            .value(SLIME, crate::mobs::body::attribute(id))
    }

    #[test]
    fn a_slime_s_size_sets_its_health_speed_attack_box_and_data() {
        use clustine_data::attributes as ids;
        let mob = slime(3);
        assert_eq!(cube_mob::size(&mob), 3);
        assert_eq!(attribute(&mob, ids::MAX_HEALTH), 9.0);
        assert_eq!(
            mob.body.vitals.health, 9.0,
            "its health reset to its maximum"
        );
        assert_eq!(
            attribute(&mob, ids::MOVEMENT_SPEED),
            f64::from(0.2_f32 + 0.3_f32)
        );
        assert_eq!(attribute(&mob, ids::ATTACK_DAMAGE), 3.0);
        let size = of(SLIME).dimensions(&mob).expect("a cube's own size");
        assert_eq!(size.width, 0.52 * 3.0);
        assert_eq!(
            of(SLIME).shown(&mob)[2],
            (18, DataShown::Int(3), false),
            "the size at 18"
        );
        assert_eq!(
            of(SLIME).shown(&Mob::fresh())[2],
            (18, DataShown::Int(1), true)
        );
        // Held to 1 to 127.
        assert_eq!(cube_mob::size(&slime(400)), 127);
        assert_eq!(cube_mob::size(&slime(0)), 1);
    }

    #[test]
    fn an_egg_s_slime_draws_its_size_after_mob_s_finalize_spawn() {
        let mut level = Recorded::new(11);
        let mut replay = level.random.clone();
        let mob = made_by_egg(SLIME, &mut level, 0.0);
        // `Mob.finalizeSpawn`: the follow range's two doubles and the left hand's float;
        // then `setSpawnSize`'s `nextInt(3)`, and a float where that is under 2.
        replay.next_double();
        replay.next_double();
        replay.next_float();
        let power = replay.next_int(3);
        let mut expected = vec!["double", "double", "float", "int"];
        if power < 2 {
            replay.next_float();
            expected.push("float");
        }
        assert_eq!(level.calls, expected);
        assert_eq!(cube_mob::size(&mob), 1 << power);
        assert_eq!(mob.body.vitals.health, ((1 << power) * (1 << power)) as f32);
    }

    #[test]
    fn a_special_multiplier_may_double_an_egg_s_slime() {
        // Of every seed whose `nextInt(3)` is 0, the float under half the multiplier of 2
        // (any float) makes it of size 2.
        for seed in 0..40 {
            let mut level = Recorded::new(seed);
            let mut replay = level.random.clone();
            replay.next_double();
            replay.next_double();
            replay.next_float();
            let power = replay.next_int(3);
            let mob = made_by_egg(SLIME, &mut level, 2.0);
            let expected = if power < 2 { power + 1 } else { power };
            assert_eq!(cube_mob::size(&mob), 1 << expected, "seed {seed}");
        }
    }

    #[test]
    fn a_split_draws_its_count_then_each_new_cube_s_yaw_of_the_parent() {
        let parent = slime(4);
        let mut draws = Random::of_entity(9, KEY, 3);
        let mut replay = draws.clone();
        let made = cube_mob::split(SLIME, &parent, &mut draws);
        let count = 2 + replay.next_int(3) as usize;
        assert_eq!(made.len(), count);
        let half_width = 0.52 * 4.0 / 2.0;
        for (i, split) in made.iter().enumerate() {
            assert_eq!(split.size, 2);
            let i = i as i32;
            assert_eq!(
                split.offset,
                [
                    ((i % 2) as f32 - 0.5) * half_width,
                    ((i / 2) as f32 - 0.5) * half_width
                ]
            );
            assert_eq!(split.yaw, replay.next_float() * 360.0);
        }
        assert_eq!(draws, replay);
    }

    #[test]
    fn a_tiny_slime_does_not_split_and_draws_nothing() {
        let mut draws = Random::of_entity(9, KEY, 3);
        let before = draws.clone();
        assert!(cube_mob::split(SLIME, &slime(1), &mut draws).is_empty());
        assert_eq!(draws, before);
        // Size 3 splits into cubes of 1.
        let made = cube_mob::split(SLIME, &slime(3), &mut draws);
        assert!(made.iter().all(|split| split.size == 1));
    }

    #[test]
    fn the_slime_s_goals_and_targets_are_the_cube_s_three_its_attack_and_two_targets() {
        let mut setup = Setup::default();
        of(SLIME).register_goals(&mut setup);
        let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
        assert_eq!(goals, [1, 4, 5, 2]);
        let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
        assert_eq!(targets, [1, 3]);
        assert!(setup.config.navigation.flags.can_float);
    }

    #[test]
    fn on_the_ground_it_jumps_once_its_delay_is_over_with_the_jump_s_sound() {
        let world = Field::new();
        let mut run = Run::new(SLIME, slime(2), [0.5, 0.0, 0.5], true);
        // The first step is a full one: the random way (its delay and its way drawn,
        // `nextInt(60)` and `nextInt(360)`) and the keeping on jumping start and tick; the
        // move control turns, finds its delay at 0, draws `nextInt(20)` for the next and
        // jumps with its sound, two floats.
        let mut replay = run.stream();
        let (drawn, out) = run.step(&world);
        // Ambient look-around goals: none. The target goals draw their interval first:
        // the players' and the golems' `nextInt(5)` each.
        replay.next_int(5);
        replay.next_int(5);
        let _delay = replay.next_int(60);
        let way = replay.next_int(360) as f32;
        let jump_delay = replay.next_int(20) as i32 + 10;
        let a = replay.next_float();
        let b = replay.next_float();
        assert_eq!(drawn, replay.draws_since(&run_stream_at(7, 0)));
        assert!(run.mob.body.inputs.jumping, "it jumps");
        assert_eq!(cube_mob::state_of(&run.mob).jump_delay, jump_delay);
        assert_eq!(cube_mob::state_of(&run.mob).direction, Some(way));
        assert_eq!(sounds_of(&out), [sounds::ENTITY_SLIME_JUMP]);
        let Event::Sound { volume, pitch, .. } = out.events[0] else {
            unreachable!()
        };
        assert_eq!(volume, 0.8, "0.4 times its size");
        assert_eq!(pitch, ((a - b) * 0.2 + 1.0) * 0.8);
        // It turns towards its way by at most 90.
        let expected = crate::mobs::ai::maths::move_rotlerp(0.0, way, 90.0);
        assert_eq!(run.yaw, expected);
        // The next step is a full one too (`(tickCount + id) % 2`): asked on, its delay
        // counted down, no jump.
        let (_, out) = run.step(&world);
        assert!(sounds_of(&out).is_empty());
        assert!(!run.mob.body.inputs.jumping);
        assert_eq!(cube_mob::state_of(&run.mob).jump_delay, jump_delay - 1);
        // The third is a half one: the keeping on jumping does not tick, so nothing asks
        // it on and it stands, its delay not counted.
        let (_, out) = run.step(&world);
        assert!(sounds_of(&out).is_empty());
        assert_eq!(run.mob.body.inputs.zza, 0.0);
        assert_eq!(cube_mob::state_of(&run.mob).jump_delay, jump_delay - 1);
    }

    fn run_stream_at(seed: u64, lived: u32) -> Random {
        Random::of_entity(seed, KEY, lived)
    }

    #[test]
    fn a_landing_draws_two_floats_for_each_particle_then_squishes() {
        let world = Field::new();
        for (size, particles) in [(1, 17), (2, 34), (4, 67)] {
            let mut run = Run::new(SLIME, slime(size), [0.5, 0.0, 0.5], true);
            let (drawn, out) = run.with_tick(&world, cube_mob::after_tick);
            assert_eq!(drawn, particles * 2 + 2, "size {size}");
            let squish = if size == 1 {
                sounds::ENTITY_SLIME_SQUISH_SMALL
            } else {
                sounds::ENTITY_SLIME_SQUISH
            };
            assert_eq!(sounds_of(&out), [squish]);
            assert!(cube_mob::state_of(&run.mob).was_on_ground);
            // On the ground still: nothing more.
            let (drawn, out) = run.with_tick(&world, cube_mob::after_tick);
            assert_eq!(drawn, 0);
            assert!(out.events.is_empty());
        }
    }

    #[test]
    fn the_slime_s_players_must_be_within_four_blocks_up_or_down() {
        let mut world = Field::new();
        world.players = vec![player(9, [3.5, 5.0, 0.5], false)];
        let mut run = Run::new(SLIME, slime(2), [0.5, 0.0, 0.5], true);
        let looked = crate::mobs::ai::sensing::Looked::of_player(&world.players[0]);
        let (_, far) = run.with_ai(&world, |ai| {
            crate::mobs::types::slime::within_height(ai, &looked)
        });
        assert!(!far, "five up");
        world.players = vec![player(9, [3.5, 4.0, 0.5], false)];
        let looked = crate::mobs::ai::sensing::Looked::of_player(&world.players[0]);
        let (_, near) = run.with_ai(&world, |ai| {
            crate::mobs::types::slime::within_height(ai, &looked)
        });
        assert!(near, "four up");
    }

    #[test]
    fn the_attack_goal_turns_it_to_its_target_and_means_harm_unless_it_is_tiny() {
        let mut world = Field::new();
        let mut golem = Mob::fresh();
        golem.body.vitals.health = 100.0;
        world.entities.push(record(
            5,
            kind(entity_types::IRON_GOLEM),
            [4.5, 0.0, 0.5],
            &golem,
        ));
        for (size, harm) in [(1, false), (2, true)] {
            let mut mob = slime(size);
            mob.body.combat.target = Some(EntityId(5));
            let mut run = Run::new(SLIME, mob, [0.5, 0.0, 0.5], true);
            run.step(&world);
            let own = cube_mob::state_of(&run.mob);
            assert_eq!(own.aggressive, harm, "size {size}");
            // Turned towards the golem (east, yaw -90) by at most 10, then the move
            // control towards that by at most 90.
            assert_eq!(own.direction, Some(-10.0));
        }
    }

    #[test]
    fn a_slime_s_experience_is_its_size() {
        let mut draws = Random::of_entity(1, KEY, 0);
        assert_eq!(of(SLIME).experience_reward(SLIME, &slime(4), &mut draws), 4);
        assert_eq!(of(SLIME).experience_reward(SLIME, &slime(1), &mut draws), 1);
    }
}

mod enderman {
    use super::*;
    use crate::mobs::ai::selector::Goal;
    use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
    use crate::mobs::tick::Local;
    use crate::mobs::types::enderman::{self, Pending};

    const ENDERMAN: u16 = kind(entity_types::ENDERMAN);

    fn enderman_mob() -> Mob {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 40.0;
        mob
    }

    fn run_at_seed(seed: u64) -> Run {
        let mut run = Run::new(ENDERMAN, enderman_mob(), [0.5, 0.0, 0.5], true);
        run.seed = seed;
        run
    }

    fn change_pending(run: &mut Run, pending: Pending) {
        enderman::change(&mut run.mob, |own| own.pending = Some(pending));
    }

    /// The place `teleport()` draws from `replay`, from feet at `feet`.
    fn drawn_place(replay: &mut Random, feet: Vec3) -> Vec3 {
        let x = feet[0] + (replay.next_double() - 0.5) * 64.0;
        let y = feet[1] + f64::from(replay.next_int(64) as i32 - 32);
        let z = feet[2] + (replay.next_double() - 0.5) * 64.0;
        [x, y, z]
    }

    fn mover() -> crate::physics::entities::Mover {
        crate::physics::entities::Mover {
            key: KEY,
            root: KEY,
            like_a_vehicle: false,
            happy_ghast: false,
        }
    }

    #[test]
    fn the_enderman_s_goals_and_targets_are_the_game_s_by_priority() {
        let mut setup = Setup::default();
        of(ENDERMAN).register_goals(&mut setup);
        let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
        assert_eq!(goals, [0, 1, 2, 7, 8, 8, 10, 11]);
        let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
        assert_eq!(targets, [1, 2, 3, 4]);
        assert_eq!(
            setup.config.malus[clustine_data::PathType::Water as usize],
            Some(-1.0),
            "it keeps out of water"
        );
    }

    #[test]
    fn a_teleport_lands_its_feet_on_the_first_block_below_its_drawn_place() {
        let world = Field::new();
        let mut landed = 0;
        for seed in 0..24 {
            let mut run = run_at_seed(seed);
            let mut replay = run.stream();
            let (drawn, out) = run.with_tick(&world, |tick| {
                enderman::teleport(tick);
            });
            let [x, y, z] = drawn_place(&mut replay, [0.5, 0.0, 0.5]);
            assert_eq!(drawn, 3, "two doubles and an int, nothing more");
            if y >= 0.0 {
                landed += 1;
                // Down from its block to the stone below the floor, its feet as many
                // blocks lower as it went: at 0.
                assert_eq!(run.living.body.position, [x, 0.0, z], "seed {seed}");
                assert!(out.events.contains(&Event::Entity(46)));
                assert_eq!(sounds_of(&out), [sounds::ENTITY_ENDERMAN_TELEPORT]);
                assert!(out.local.iter().any(|local| matches!(
                    local,
                    Local::Sound { sound, .. } if *sound == sounds::ENTITY_ENDERMAN_TELEPORT
                )));
                assert!(out.local.iter().any(|local| matches!(
                    local,
                    Local::GameEventAt { name: "minecraft:teleport", at, .. }
                        if *at == [0.5, 0.0, 0.5]
                )));
            } else {
                // Inside the stone: its box there meets a block.
                assert_eq!(run.living.body.position, [0.5, 0.0, 0.5], "seed {seed}");
                assert!(out.events.is_empty());
            }
        }
        assert!(landed > 0);
    }

    #[test]
    fn random_teleport_refuses_water_below_and_finds_the_floor_through_air() {
        let mover = mover();
        let who = crate::region::teleports::enderman(&mover);
        // Water under the whole field: no block below takes it before the floor.
        let mut world = Field::new();
        world.floor = blocks::WATER;
        assert_eq!(
            crate::region::teleports::random_teleport(&world, [3.5, 5.0, 3.5], &who),
            None
        );
        // A block of water on the stone: its feet would stand in it.
        let mut world = Field::new();
        world.set(BlockPos::new(3, 0, 3), blocks::WATER);
        assert_eq!(
            crate::region::teleports::random_teleport(&world, [3.5, 2.0, 3.5], &who),
            None,
            "water in its box"
        );
        assert_eq!(
            crate::region::teleports::random_teleport(&world, [5.5, 2.25, 3.5], &who),
            Some([5.5, 0.25, 3.5]),
            "the fraction of its height kept"
        );
    }

    #[test]
    fn a_projectile_is_refused_and_teleports_it_up_to_sixty_four_times() {
        // Every try fails over water.
        let mut world = Field::new();
        world.floor = blocks::WATER;
        let mut run = run_at_seed(7);
        let arrow = Causer {
            key: EntityId(77),
            kind: kind(entity_types::ARROW),
            position: [3.0, 1.0, 0.5],
            player: None,
            living: false,
        };
        let source = DamageSource::thrown(arrow, [-1.0, 0.0, 0.0], None);
        run.with_tick(&world, |tick| {
            let took = crate::mobs::body::hurt_by(tick, &source, 4.0);
            assert!(!took, "refused");
        });
        assert_eq!(run.mob.body.vitals.health, 40.0);
        assert_eq!(
            enderman::state_of(&run.mob).pending,
            Some(Pending::Repeatedly)
        );
        let (drawn, _) = run.with_tick(&world, |tick| of(ENDERMAN).after_tick(tick));
        assert_eq!(drawn, 64 * 3, "64 tries of three draws");
        assert_eq!(enderman::state_of(&run.mob).pending, None);
        // Over stone the first try that lands ends them.
        let world = Field::new();
        let mut run = run_at_seed(3);
        change_pending(&mut run, Pending::Repeatedly);
        let mut replay = run.stream();
        let mut tries = 0;
        loop {
            tries += 1;
            let [_, y, _] = drawn_place(&mut replay, [0.5, 0.0, 0.5]);
            if y >= 0.0 {
                break;
            }
        }
        let (drawn, _) = run.with_tick(&world, |tick| of(ENDERMAN).after_tick(tick));
        assert_eq!(drawn, tries * 3);
    }

    #[test]
    fn a_hurt_no_living_thing_caused_asks_a_teleport_nine_times_in_ten() {
        let world = Field::new();
        let mut once = 0;
        for seed in 0..40 {
            let mut run = run_at_seed(seed);
            let mut asked = None;
            run.with_tick(&world, |tick| {
                crate::mobs::body::hurt_by(tick, &DamageSource::of(DamageType::IN_WALL), 1.0);
                asked = enderman::state_of(&tick.mob).pending;
            });
            if asked == Some(Pending::Once) {
                once += 1;
            }
        }
        assert!((30..40).contains(&once), "nine in ten, {once} of 40");
        // A living thing's hurt asks nothing.
        let mut run = run_at_seed(1);
        let zombie = Causer {
            key: EntityId(5),
            kind: kind(entity_types::ZOMBIE),
            position: [1.5, 0.0, 0.5],
            player: None,
            living: true,
        };
        run.with_tick(&world, |tick| {
            crate::mobs::body::hurt_by(
                tick,
                &DamageSource::by(DamageType::MOB_ATTACK, zombie),
                1.0,
            );
        });
        assert_eq!(enderman::state_of(&run.mob).pending, None);
        assert_eq!(run.mob.body.vitals.health, 39.0);
    }

    #[test]
    fn water_hurts_it_by_one() {
        let mut world = Field::new();
        world.set(BlockPos::new(0, 0, 0), blocks::WATER);
        world.set(BlockPos::new(0, 1, 0), blocks::WATER);
        let mut run = run_at_seed(7);
        run.living.fluids = crate::physics::fluids::update(&world, &run.living.body.bounds());
        assert!(run.living.fluids.in_water());
        run.with_tick(&world, |tick| of(ENDERMAN).after_ai_step(tick));
        assert_eq!(run.mob.body.vitals.health, 39.0, "drowning's 1");
    }

    /// The block `EndermanTakeBlockGoal.tick` draws of `run`'s next stream.
    fn take_place(run: &Run) -> BlockPos {
        let mut replay = run.stream();
        let x = crate::physics::floor(0.5 - 2.0 + replay.next_double() * 4.0);
        let y = crate::physics::floor(replay.next_double() * 3.0);
        let z = crate::physics::floor(0.5 - 2.0 + replay.next_double() * 4.0);
        BlockPos::new(x, y, z)
    }

    #[test]
    fn it_takes_a_holdable_block_its_three_doubles_find_and_carries_it() {
        let mut world = Field::new();
        let mut run = run_at_seed(7);
        let pos = take_place(&run);
        world.set(pos, blocks::GRASS_BLOCK);
        let (drawn, (), out) = run.with_ai_out(&world, |ai| {
            enderman::EndermanTakeBlockGoal.tick(&mut (), ai);
        });
        assert_eq!(drawn, 3);
        assert_eq!(enderman::carried(&run.mob), Some(blocks::GRASS_BLOCK));
        assert_eq!(
            out.local[0],
            Local::SetBlock {
                pos,
                state: blocks::AIR,
                flags: 3
            }
        );
        assert!(matches!(
            out.local[1],
            Local::GameEventAt {
                name: "minecraft:block_destroy",
                affected: Some(blocks::GRASS_BLOCK),
                ..
            }
        ));
        assert_eq!(
            of(ENDERMAN).shown(&run.mob)[0],
            (
                16,
                DataShown::OptionalBlockState(Some(blocks::GRASS_BLOCK.0)),
                false
            )
        );
        assert!(of(ENDERMAN).requires_custom_persistence(&run.mob));
        // Carrying, it asks no more; its putting down asks one in a thousand.
        let (drawn, used) = run.with_ai(&world, |ai| {
            enderman::EndermanTakeBlockGoal.can_use(&mut (), ai)
        });
        assert!(!used);
        assert_eq!(drawn, 0);
        let mut replay = run.stream();
        let (drawn, used) = run.with_ai(&world, |ai| {
            enderman::EndermanLeaveBlockGoal.can_use(&mut (), ai)
        });
        assert_eq!(drawn, 1);
        assert_eq!(used, replay.next_int(1000) == 0);
    }

    #[test]
    fn a_block_not_holdable_is_not_taken() {
        let mut world = Field::new();
        let mut run = run_at_seed(7);
        let pos = take_place(&run);
        world.set(pos, blocks::STONE);
        let (_, (), out) = run.with_ai_out(&world, |ai| {
            enderman::EndermanTakeBlockGoal.tick(&mut (), ai);
        });
        assert!(out.local.is_empty());
        assert_eq!(enderman::carried(&run.mob), None);
    }

    #[test]
    fn it_puts_its_block_down_only_on_the_floor() {
        for seed in 0..16 {
            let world = Field::new();
            let mut run = run_at_seed(seed);
            enderman::change(&mut run.mob, |own| own.carried = Some(blocks::DIRT.0));
            let mut replay = run.stream();
            let x = crate::physics::floor(0.5 - 1.0 + replay.next_double() * 2.0);
            let y = crate::physics::floor(replay.next_double() * 2.0);
            let z = crate::physics::floor(0.5 - 1.0 + replay.next_double() * 2.0);
            let (_, (), out) = run.with_ai_out(&world, |ai| {
                enderman::EndermanLeaveBlockGoal.tick(&mut (), ai);
            });
            if y == 0 {
                assert_eq!(
                    out.local,
                    [Local::PlaceBlock {
                        pos: BlockPos::new(x, 0, z),
                        state: blocks::DIRT
                    }],
                    "seed {seed}"
                );
                assert_eq!(enderman::carried(&run.mob), None);
            } else {
                assert!(out.local.is_empty(), "seed {seed}: on air");
                assert_eq!(enderman::carried(&run.mob), Some(blocks::DIRT));
            }
        }
    }

    #[test]
    fn its_block_is_not_put_down_where_another_stands() {
        for seed in 0..16 {
            let mut world = Field::new();
            let mut run = run_at_seed(seed);
            enderman::change(&mut run.mob, |own| own.carried = Some(blocks::DIRT.0));
            let mut replay = run.stream();
            let x = crate::physics::floor(0.5 - 1.0 + replay.next_double() * 2.0);
            let _ = replay.next_double();
            let z = crate::physics::floor(0.5 - 1.0 + replay.next_double() * 2.0);
            world.players = vec![player(
                9,
                [f64::from(x) + 0.5, 0.0, f64::from(z) + 0.5],
                true,
            )];
            let (_, (), out) = run.with_ai_out(&world, |ai| {
                enderman::EndermanLeaveBlockGoal.tick(&mut (), ai);
            });
            assert!(out.local.is_empty(), "seed {seed}");
        }
    }

    #[test]
    fn its_carried_block_drops_at_its_death_as_its_item() {
        let mut mob = enderman_mob();
        enderman::change(&mut mob, |own| own.carried = Some(blocks::DIRT.0));
        let dropped = of(ENDERMAN).drop_equipment(&mut mob);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].item, clustine_data::items::DIRT);
        assert_eq!(enderman::carried(&mob), None);
    }

    #[test]
    fn a_target_makes_it_scream_and_quicker_and_its_loss_undoes_both() {
        let world = Field::new();
        let mut run = run_at_seed(7);
        let speed = |mob: &Mob| {
            mob.body.attributes.value(
                ENDERMAN,
                crate::mobs::body::attribute(clustine_data::attributes::MOVEMENT_SPEED),
            )
        };
        run.mob.body.combat.target = Some(EntityId(6));
        run.with_tick(&world, |tick| of(ENDERMAN).custom_server_ai_step(tick));
        let own = enderman::state_of(&run.mob);
        assert!(own.creepy);
        assert_eq!(own.target_change_time, 1);
        assert!(
            (speed(&run.mob) - (0.300_000_011_920_928_96 + 0.150_000_005_960_464_48)).abs() < 1e-9
        );
        assert_eq!(
            of(ENDERMAN).ambient_sound(&run.mob),
            Some(sounds::ENTITY_ENDERMAN_SCREAM)
        );
        assert_eq!(
            of(ENDERMAN).shown(&run.mob)[1],
            (17, DataShown::Boolean(true), false)
        );
        run.mob.body.combat.target = None;
        run.with_tick(&world, |tick| of(ENDERMAN).custom_server_ai_step(tick));
        let own = enderman::state_of(&run.mob);
        assert!(!own.creepy);
        assert_eq!(own.target_change_time, 0);
        assert!((speed(&run.mob) - 0.300_000_011_920_928_96).abs() < 1e-9);
    }

    #[test]
    fn by_day_in_the_open_it_draws_to_teleport_once_ten_seconds_have_passed() {
        let mut world = Field::new();
        world.darken = 0;
        world.light = Some(LightAt { sky: 15, block: 0 });
        // Before its tick count reaches 600, nothing is drawn.
        let mut run = run_at_seed(7);
        run.lived = 598;
        let (drawn, _) = run.with_tick(&world, |tick| of(ENDERMAN).custom_server_ai_step(tick));
        assert_eq!(drawn, 0);
        // From then on, a float; under 1.2 of 30 it teleports (three more draws).
        let mut teleported = 0;
        for seed in 0..60 {
            let mut run = run_at_seed(seed);
            run.lived = 599;
            let mut replay = run.stream();
            let (drawn, _) = run.with_tick(&world, |tick| of(ENDERMAN).custom_server_ai_step(tick));
            if replay.next_float() * 30.0 < 1.2 {
                teleported += 1;
                assert_eq!(drawn, 4, "seed {seed}");
            } else {
                assert_eq!(drawn, 1, "seed {seed}");
            }
        }
        assert!(teleported < 12);
        // In the dark it draws nothing.
        world.darken = 11;
        let mut run = run_at_seed(7);
        run.lived = 700;
        let (drawn, _) = run.with_tick(&world, |tick| of(ENDERMAN).custom_server_ai_step(tick));
        assert_eq!(drawn, 0);
    }

    #[test]
    fn a_dying_enderman_draws_nothing_to_teleport() {
        let world = Field::new();
        let mut run = run_at_seed(7);
        run.mob.body.vitals.health = 0.0;
        change_pending(&mut run, Pending::Repeatedly);
        let (drawn, _) = run.with_tick(&world, |tick| of(ENDERMAN).after_tick(tick));
        assert_eq!(drawn, 0);
    }
}

mod silverfish {
    use super::*;
    use crate::mobs::ai::selector::Goal;
    use crate::mobs::body::Removed;
    use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
    use crate::mobs::tick::Local;
    use crate::mobs::types::silverfish::{self, Merging, Waking};

    const SILVERFISH: u16 = kind(entity_types::SILVERFISH);

    fn run_at_seed(seed: u64) -> Run {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 8.0;
        let mut run = Run::new(SILVERFISH, mob, [0.5, 0.0, 0.5], true);
        run.seed = seed;
        run
    }

    /// The waking goal's state as the mind keeps it.
    fn waking(mob: &Mob) -> Waking {
        let id = crate::mobs::part::mind_ids::FIRST_GOAL + 2;
        mob.mind
            .slots
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| postcard::from_bytes(&entry.bytes).expect("a waking's state"))
            .unwrap_or_default()
    }

    #[test]
    fn the_silverfish_s_goals_and_targets_are_the_game_s_by_priority() {
        let mut setup = Setup::default();
        of(SILVERFISH).register_goals(&mut setup);
        let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
        assert_eq!(goals, [1, 1, 3, 4, 5]);
        let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
        assert_eq!(targets, [1, 2]);
        assert!(!of(SILVERFISH).plays_step_sounds());
    }

    #[test]
    fn hosts_and_their_infested_blocks_go_both_ways_keeping_the_axis() {
        assert!(silverfish::is_host(blocks::STONE));
        assert!(silverfish::is_host(blocks::DEEPSLATE));
        assert!(!silverfish::is_host(blocks::DIRT));
        assert!(silverfish::is_infested(blocks::INFESTED_COBBLESTONE));
        assert_eq!(
            silverfish::infested_by_host(blocks::MOSSY_STONE_BRICKS),
            blocks::INFESTED_MOSSY_STONE_BRICKS
        );
        let lying = blocks::DEEPSLATE
            .with_property("axis", "x")
            .expect("an axis");
        let infested = silverfish::infested_by_host(lying);
        assert!(silverfish::is_infested(infested));
        assert_eq!(infested.property("axis"), Some("x"));
    }

    #[test]
    fn with_its_path_done_one_ask_in_five_it_merges_into_a_host_beside_it() {
        let world = Field::new();
        let mut merged = 0;
        for seed in 0..200 {
            let mut run = run_at_seed(seed);
            let mut replay = run.stream();
            let mut state = Merging::default();
            let (_, used, out) = run.with_ai_out(&world, |ai| {
                let used = silverfish::SilverfishMergeWithStoneGoal.can_use(&mut state, ai);
                if used && state.do_merge {
                    silverfish::SilverfishMergeWithStoneGoal.start(&mut state, ai);
                }
                used
            });
            if replay.next_int(5) == 0 && replay.next_int(6) == 0 {
                // Down: the stone below its feet.
                merged += 1;
                assert!(used && state.do_merge, "seed {seed}");
                assert_eq!(
                    out.local,
                    [Local::SetBlock {
                        pos: BlockPos::new(0, -1, 0),
                        state: blocks::INFESTED_STONE,
                        flags: 3
                    }]
                );
                assert_eq!(out.events, [Event::Entity(20)]);
                assert_eq!(out.removed, Some(Removed::Discarded));
            } else {
                assert!(!state.do_merge, "seed {seed}");
                assert!(out.local.is_empty());
                assert_eq!(out.removed, None);
            }
        }
        assert!(merged > 0);
    }

    #[test]
    fn with_a_target_it_does_not_merge_and_draws_nothing() {
        let world = Field::new();
        let mut run = run_at_seed(1);
        run.mob.body.combat.target = Some(EntityId(9));
        let (drawn, used) = run.with_ai(&world, |ai| {
            silverfish::SilverfishMergeWithStoneGoal.can_use(&mut Merging::default(), ai)
        });
        assert!(!used);
        assert_eq!(drawn, 0);
    }

    #[test]
    fn a_hurt_by_something_wakes_its_friends_ten_goal_ticks_later() {
        let world = Field::new();
        let mut run = run_at_seed(1);
        let zombie = Causer {
            key: EntityId(5),
            kind: kind(entity_types::ZOMBIE),
            position: [1.5, 0.0, 0.5],
            player: None,
            living: true,
        };
        run.with_tick(&world, |tick| {
            crate::mobs::body::hurt_by(
                tick,
                &DamageSource::by(DamageType::MOB_ATTACK, zombie.clone()),
                1.0,
            );
        });
        assert_eq!(waking(&run.mob).look_for_friends, 10);
        // Counting already, a second hurt does not start it again.
        let mut counting = run.mob.clone();
        let id = crate::mobs::part::mind_ids::FIRST_GOAL + 2;
        for entry in &mut counting.mind.slots {
            if entry.id == id {
                entry.bytes = postcard::to_allocvec(&Waking {
                    look_for_friends: 4,
                })
                .expect("encoded");
            }
        }
        run.mob = counting;
        run.with_tick(&world, |tick| {
            crate::mobs::body::hurt_by(
                tick,
                &DamageSource::by(DamageType::MOB_ATTACK, zombie),
                1.0,
            );
        });
        assert_eq!(waking(&run.mob).look_for_friends, 4);
        // A fall wakes nobody.
        let mut run = run_at_seed(1);
        run.with_tick(&world, |tick| {
            crate::mobs::body::hurt_by(tick, &DamageSource::of(DamageType::FALL), 1.0);
        });
        assert_eq!(waking(&run.mob).look_for_friends, 0);
    }

    #[test]
    fn waking_breaks_infested_blocks_nearest_first_until_a_boolean_stops_it() {
        let mut world = Field::new();
        // Its own block's column first (y 0, then 1, −1 …), x and z from 0 outwards.
        let first = BlockPos::new(0, 1, 0);
        let second = BlockPos::new(1, 1, 0);
        let third = BlockPos::new(0, -1, 0);
        for pos in [first, second, third] {
            world.set(pos, blocks::INFESTED_STONE);
        }
        for seed in 0..12 {
            let mut run = run_at_seed(seed);
            let mut replay = run.stream();
            let mut state = Waking {
                look_for_friends: 1,
            };
            let (drawn, (), out) = run.with_ai_out(&world, |ai| {
                silverfish::SilverfishWakeUpFriendsGoal.tick(&mut state, ai);
            });
            // The order of the walk: y 0, 1, −1 …; at each y, x 0, 1, −1 …; z likewise.
            let mut expected = Vec::new();
            for pos in [first, second, third] {
                expected.push(Local::BreakBlock { pos });
                if replay.next_bool() {
                    break;
                }
            }
            assert_eq!(out.local, expected, "seed {seed}");
            assert_eq!(drawn, expected.len() as u64);
        }
        // Not yet counted down: nothing.
        let mut run = run_at_seed(1);
        let mut state = Waking {
            look_for_friends: 2,
        };
        let (drawn, (), out) = run.with_ai_out(&world, |ai| {
            silverfish::SilverfishWakeUpFriendsGoal.tick(&mut state, ai);
        });
        assert_eq!(state.look_for_friends, 1);
        assert!(out.local.is_empty());
        assert_eq!(drawn, 0);
    }
}

mod endermite {
    use super::*;
    use crate::mobs::body::Removed;

    const ENDERMITE: u16 = kind(entity_types::ENDERMITE);

    fn run_at(lived: u32) -> Run {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 8.0;
        let mut run = Run::new(ENDERMITE, mob, [0.5, 0.0, 0.5], true);
        run.lived = lived;
        run
    }

    #[test]
    fn the_endermite_s_goals_and_targets_are_the_game_s_by_priority() {
        let mut setup = Setup::default();
        of(ENDERMITE).register_goals(&mut setup);
        let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
        assert_eq!(goals, [1, 1, 2, 3, 7, 8]);
        let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
        assert_eq!(targets, [1, 2]);
        assert!(!of(ENDERMITE).plays_step_sounds());
    }

    #[test]
    fn it_is_gone_at_its_two_thousand_four_hundredth_tick_unless_kept() {
        let world = Field::new();
        let mut run = run_at(2398);
        let (_, out) = run.with_tick(&world, |tick| of(ENDERMITE).after_ai_step(tick));
        assert_eq!(out.removed, None, "its 2399th tick");
        let (_, out) = run.with_tick(&world, |tick| of(ENDERMITE).after_ai_step(tick));
        assert_eq!(out.removed, Some(Removed::Discarded), "its 2400th");
        let mut kept = run_at(5000);
        kept.mob.body.flags.persistent = true;
        let (_, out) = kept.with_tick(&world, |tick| of(ENDERMITE).after_ai_step(tick));
        assert_eq!(out.removed, None);
    }

    #[test]
    fn its_body_faces_its_yaw_and_its_yaw_follows_its_body() {
        let world = Field::new();
        let mut run = run_at(0);
        run.yaw = 30.0;
        run.with_tick(&world, |tick| of(ENDERMITE).before_tick(tick));
        assert_eq!(run.mob.body.head.body_yaw, 30.0);
        run.mob.body.head.body_yaw = 45.0;
        run.with_tick(&world, |tick| of(ENDERMITE).after_tick(tick));
        assert_eq!(run.yaw, 45.0);
    }

    #[test]
    fn its_experience_is_three_and_what_it_wears_adds() {
        let mut draws = Random::of_entity(1, KEY, 0);
        let before = draws.clone();
        assert_eq!(
            of(ENDERMITE).experience_reward(ENDERMITE, &Mob::fresh(), &mut draws),
            3
        );
        assert_eq!(draws, before);
    }
}

mod phantom {
    use super::*;
    use crate::mobs::ai::selector::Goal;
    use crate::mobs::types::phantom::{self, AttackPhase, Circling, Scanning};

    const PHANTOM: u16 = kind(entity_types::PHANTOM);

    fn run_at_seed(seed: u64, feet: Vec3) -> Run {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 20.0;
        phantom::change(&mut mob, |own| own.anchor = Some(BlockPos::new(0, 20, 0)));
        let mut run = Run::new(PHANTOM, mob, feet, false);
        run.seed = seed;
        run
    }

    #[test]
    fn the_phantom_s_goals_and_target_are_the_game_s_by_priority() {
        let mut setup = Setup::default();
        of(PHANTOM).register_goals(&mut setup);
        let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
        assert_eq!(goals, [1, 2, 3]);
        let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
        assert_eq!(targets, [1]);
    }

    #[test]
    fn an_egg_s_phantom_anchors_five_above_its_block_and_is_of_size_naught() {
        let mut level = Recorded::new(4);
        let mob = made_by_egg(PHANTOM, &mut level, 0.0);
        assert_eq!(level.calls, ["double", "double", "float"], "Mob's alone");
        let own = phantom::state_of(&mob);
        assert_eq!(own.anchor, Some(BlockPos::new(0, 5, 0)));
        assert_eq!(own.size, 0);
        assert_eq!(of(PHANTOM).shown(&mob), [(16, DataShown::Int(0), true)]);
        let mut big = Mob::fresh();
        phantom::set_size(&mut big, 3);
        let size = of(PHANTOM).dimensions(&big).expect("its own size");
        let info = EntityTypeInfo::of(entity_types::PHANTOM);
        assert_eq!(size.width, info.width * 1.45);
        assert_eq!(
            big.body.attributes.value(
                PHANTOM,
                crate::mobs::body::attribute(clustine_data::attributes::ATTACK_DAMAGE)
            ),
            9.0
        );
    }

    #[test]
    fn circling_starts_at_a_drawn_distance_height_and_way_and_aims_fifteen_degrees_on() {
        let world = Field::new();
        let mut run = run_at_seed(5, [0.5, 30.0, 0.5]);
        let mut replay = run.stream();
        let mut state = Circling::default();
        let (drawn, ()) = run.with_ai(&world, |ai| {
            phantom::PhantomCircleAroundAnchorGoal.start(&mut state, ai);
        });
        assert_eq!(drawn, 3);
        let distance = 5.0 + replay.next_float() * 10.0;
        let height = -4.0 + replay.next_float() * 9.0;
        let clockwise: f32 = if replay.next_bool() { 1.0 } else { -1.0 };
        assert_eq!(
            (state.distance, state.height, state.clockwise),
            (distance, height, clockwise)
        );
        let angle = clockwise * 15.0 * 0.017_453_292;
        assert_eq!(state.angle, angle);
        let target = phantom::state_of(&run.mob).move_target;
        let cos = clustine_noise::trig::cos(f64::from(angle));
        let sin = clustine_noise::trig::sin(f64::from(angle));
        assert_eq!(
            target,
            [
                f64::from(distance * cos),
                20.0 + f64::from(-4.0 + height),
                f64::from(distance * sin)
            ]
        );
    }

    #[test]
    fn a_circling_tick_draws_its_three_chances_in_order() {
        let world = Field::new();
        let mut run = run_at_seed(9, [30.5, 60.0, 30.5]);
        // Far from its move target and in open air: only the three chances.
        phantom::change(&mut run.mob, |own| own.move_target = [0.5, 60.0, 0.5]);
        let mut replay = run.stream();
        let mut state = Circling {
            angle: 0.0,
            distance: 10.0,
            height: 0.0,
            clockwise: 1.0,
        };
        let (drawn, ()) = run.with_ai(&world, |ai| {
            phantom::PhantomCircleAroundAnchorGoal.tick(&mut state, ai);
        });
        let mut expected = 0;
        expected += 1;
        if replay.next_int(175) == 0 {
            replay.next_float();
            expected += 1;
        }
        expected += 1;
        replay.next_int(125);
        expected += 1;
        if replay.next_int(225) == 0 {
            expected += 1;
        }
        assert_eq!(drawn, expected);
    }

    #[test]
    fn its_move_control_turns_at_most_four_and_eases_its_speed_and_motion() {
        let world = Field::new();
        let mut run = run_at_seed(1, [0.5, 30.0, 0.5]);
        // Its move target straight ahead (yaw 0 faces south, +z).
        phantom::change(&mut run.mob, |own| own.move_target = [0.5, 30.0, 20.5]);
        run.with_ai(&world, phantom::tick_move_control);
        assert_eq!(run.yaw, 0.0, "already facing it");
        let speed = phantom::state_of(&run.mob).speed.expect("its speed");
        assert!((speed - (0.1 + 0.005 * 18.0)).abs() < 1e-6, "{speed}");
        assert!(run.living.body.motion[2] > 0.0);
        // A target behind it: it turns by 4.
        let mut run = run_at_seed(1, [0.5, 30.0, 0.5]);
        phantom::change(&mut run.mob, |own| own.move_target = [0.5, 30.0, -20.5]);
        run.with_ai(&world, phantom::tick_move_control);
        assert_eq!(crate::mobs::ai::maths::wrap_degrees(run.yaw).abs(), 4.0);
        let speed = phantom::state_of(&run.mob).speed.expect("its speed");
        assert!((speed - 0.125).abs() < 1e-6, "towards 0.2 by 0.025");
    }

    #[test]
    fn it_scans_for_players_every_thirty_goal_ticks_and_takes_no_creative_one() {
        let mut world = Field::new();
        world.players = vec![player(9, [3.5, 10.0, 3.5], true)];
        let mut run = run_at_seed(1, [0.5, 30.0, 0.5]);
        let mut state = Scanning::default();
        let (drawn, found) = run.with_ai(&world, |ai| {
            phantom::PhantomAttackPlayerTargetGoal.can_use(&mut state, ai)
        });
        assert!(!found);
        assert_eq!(drawn, 0);
        assert_eq!(state.next_scan_tick, 30);
        // A survivor below is taken at the next scan, not before.
        world.players = vec![player(9, [3.5, 10.0, 3.5], false)];
        for _ in 0..30 {
            let (_, found) = run.with_ai(&world, |ai| {
                phantom::PhantomAttackPlayerTargetGoal.can_use(&mut state, ai)
            });
            assert!(!found);
        }
        let (_, found) = run.with_ai(&world, |ai| {
            phantom::PhantomAttackPlayerTargetGoal.can_use(&mut state, ai)
        });
        assert!(found);
        assert_eq!(run.mob.body.combat.target, Some(EntityId(9)));
        // With a target it swoops by turns.
        let mut strategy = phantom::Striking::default();
        let (_, ()) = run.with_ai(&world, |ai| {
            phantom::PhantomAttackStrategyGoal.start(&mut strategy, ai);
        });
        assert_eq!(strategy.next_sweep_tick, 5);
        assert_eq!(phantom::state_of(&run.mob).phase, AttackPhase::Circle);
        let anchor = phantom::state_of(&run.mob).anchor.expect("an anchor");
        assert!((64..84).contains(&anchor.y), "above the sea: {anchor:?}");
    }

    #[test]
    fn it_burns_in_daylight() {
        let mut world = Field::new();
        world.darken = 0;
        world.day = 6000;
        world.light = Some(LightAt { sky: 15, block: 0 });
        let mut burned = 0;
        for seed in 0..200 {
            let mut run = run_at_seed(seed, [0.5, 30.0, 0.5]);
            run.with_tick(&world, crate::mobs::body::burning::burn_undead);
            if run.living.fire_ticks > 0 {
                burned += 1;
            }
        }
        assert!(burned > 0);
    }
}

mod frog {
    use super::*;
    use crate::mobs::types::{cube_mob, frog_ai};

    #[test]
    fn a_frog_eats_only_a_slime_of_size_one() {
        let mut world = Field::new();
        for (key, size) in [(5, 1), (6, 2)] {
            let mut mob = Mob::fresh();
            cube_mob::set_size(SLIME, &mut mob, size, true);
            world
                .entities
                .push(record(key, SLIME, [2.5, 0.0, 0.5], &mob));
        }
        let small = crate::mobs::ai::sensing::Looked::of_seen(&world.entity(EntityId(5)).unwrap());
        let large = crate::mobs::ai::sensing::Looked::of_seen(&world.entity(EntityId(6)).unwrap());
        assert!(frog_ai::can_eat(&world, &small));
        assert!(!frog_ai::can_eat(&world, &large));
    }
}
