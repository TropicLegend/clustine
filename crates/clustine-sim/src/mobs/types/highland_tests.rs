//! The highland animals' own rules, from the brief's sections 7.6 to 7.8 with 9.7.3 and
//! 10.7.3, and the bytecode where the brief is silent or wrong: the goat's scream, horns
//! and draws, the camel's pose, sizes, seats and dash, the camel husk's differences, the
//! armadillo's state, scute and hurts, and what clients are shown of each; and their
//! brains: what each registers, its sensors' delays, its activities and their switching,
//! its own behaviours (a camel's random sitting, an armadillo's balling up and rolling
//! out) and sensor (the armadillo's scare) with their draws in order, and that a tick of
//! each is a function of its inputs.

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ChunkPos, EntityId, ItemStack, TypeState};

use super::animal::{self, Animal, BABY_START_AGE};
use super::armadillo::{self, ArmadilloState, Phase};
use super::camel::{self, CamelState};
use super::equine::{self, HorseState};
use super::goat::{self, GoatState, brain as goat_brain};
use super::{Breeding, Dimensions, Spawning, of};
use crate::living::{Kind, Landing, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::body::Event;
use crate::mobs::body::hurt::{self, Causer, DamageSource, DamageType, GameEvent, Struck};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::view::{MobWorld, Seen};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;
use crate::shown::{DataShown, Serialiser};

const GOAT: u16 = entity_types::GOAT as u16;
const CAMEL: u16 = entity_types::CAMEL as u16;
const HUSK: u16 = entity_types::CAMEL_HUSK as u16;
const ARMADILLO: u16 = entity_types::ARMADILLO as u16;
const KEY: EntityId = EntityId(40);

fn draws(seed: u64) -> Random {
    Random::of_entity(seed, EntityId(5), 9)
}

/// Land of stone below y = 0 and air above, at a game time of its own, with the players
/// and entities a test puts on it.
struct Flat {
    game_time: i64,
    blocks: Vec<(BlockPos, BlockState)>,
    players: Vec<crate::mobs::view::SeenPlayer>,
    /// The entities and their boxes.
    entities: Vec<(clustine_world::Entity, Aabb)>,
}

impl Flat {
    fn at(game_time: i64) -> Self {
        Self {
            game_time,
            blocks: Vec::new(),
            players: Vec::new(),
            entities: Vec::new(),
        }
    }

    /// `entity` laid on the land.
    fn put(&mut self, entity: clustine_world::Entity) {
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        self.entities.retain(|(other, _)| other.key != entity.key);
        self.entities.push((entity, bounds));
    }
}

impl World for Flat {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if let Some((_, state)) = self.blocks.iter().find(|(at, _)| *at == pos) {
            return Some(*state);
        }
        Some(if pos.y < 0 {
            blocks::STONE
        } else {
            blocks::AIR
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Flat {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Flat {
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

    fn players(&self) -> Vec<crate::mobs::view::SeenPlayer> {
        self.players.clone()
    }

    fn light(&self, _: BlockPos) -> Option<crate::mobs::view::LightAt> {
        None
    }

    fn game_time(&self) -> i64 {
        self.game_time
    }
}

/// The body of a mob of the type `kind` standing at `position`.
fn living(kind: u16, position: Vec3) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
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

/// A mob of the type `kind` at its full health, with `state` as its type state.
fn healthy(kind: u16, state: TypeState) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = state;
    mob
}

/// `what` done in a tick of `mob` (of the type `kind`) on `world` at the moment
/// `lived`, with the stream `stream`: the mob as it left it, and what it showed.
fn in_tick(
    world: &Flat,
    kind: u16,
    mob: Mob,
    lived: u32,
    stream: &mut Random,
    what: impl FnOnce(&mut MobTick<'_>),
) -> (Mob, MobOutput) {
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: KEY,
        kind,
        lived,
        tick_count: lived,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: false,
        mob,
        living: living(kind, [0.5, 0.0, 0.5]),
        yaw: 0.0,
        pitch: 0.0,
        mob_type: of(kind),
        world,
        draws: stream,
        out: MobOutput::default(),
    };
    what(&mut tick);
    (tick.mob, tick.out)
}

/// A hurt of `amount` by `source` of `mob` (of the type `kind`) between its ticks at the
/// moment `moment` and the game time `game_time`; whether it took, and what it showed.
fn hurt(
    kind: u16,
    mob: &mut Mob,
    source: &DamageSource,
    amount: f32,
    moment: u32,
    game_time: i64,
    stream: &mut Random,
) -> (bool, MobOutput) {
    let mut out = MobOutput::default();
    let mut motion = [0.0; 3];
    let took = {
        let mut struck = Struck {
            kind,
            mob,
            mob_type: of(kind),
            position: [0.5, 0.0, 0.5],
            motion: &mut motion,
            on_ground: true,
            on_fire: false,
            vehicle: None,
            carries: false,
            leashed: false,
            in_liquid: false,
            moment,
            game_time,
            draws: stream,
            out: &mut out,
        };
        hurt::hurt_server(&mut struck, source, amount)
    };
    (took, out)
}

fn by_player() -> DamageSource {
    DamageSource::by(
        DamageType::PLAYER_ATTACK,
        Causer {
            key: EntityId(7),
            kind: entity_types::PLAYER as u16,
            position: [3.5, 0.0, 0.5],
            player: Some(7),
            living: true,
        },
    )
}

fn sounds_of(out: &MobOutput) -> Vec<i32> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

fn spawned(kind: u16, mob: &mut Mob, level: &mut Random, game_time: i64) {
    of(kind).finalize_spawn(&mut Spawning {
        mob,
        level,
        own: &mut draws(99),
        biome: None,
        at: BlockPos::new(0, 0, 0),
        day: 0,
        game_time,
    });
}

fn goat_with(own: &GoatState, shared: &Animal) -> Mob {
    healthy(GOAT, animal::write(shared, own))
}

fn baby() -> Animal {
    let mut shared = Animal::default();
    shared.set_age(BABY_START_AGE, 0);
    shared
}

#[test]
fn every_highland_animal_is_built_of_the_animals_and_the_camel_husk_alone_despawns() {
    for kind in [GOAT, CAMEL, HUSK, ARMADILLO] {
        assert!(super::is_built(kind), "{kind}");
        assert_eq!(
            crate::mobs::body::ladder::of(kind),
            crate::mobs::body::ladder::Family::Animal
        );
        let far = of(kind).remove_when_far_away(kind, &Mob::fresh(), 0, 200.0);
        assert_eq!(far, kind == HUSK, "{kind}");
    }
}

#[test]
fn the_three_bodies_have_the_game_s_attributes() {
    let supplied =
        |kind: u16, id: i32| crate::mobs::body::attributes::supplied(kind, id as u16).unwrap();
    use clustine_data::attributes as ids;
    assert_eq!(supplied(GOAT, ids::MAX_HEALTH), 10.0);
    assert_eq!(supplied(GOAT, ids::MOVEMENT_SPEED), f64::from(0.2_f32));
    assert_eq!(supplied(GOAT, ids::ATTACK_DAMAGE), 2.0);
    for kind in [CAMEL, HUSK] {
        assert_eq!(supplied(kind, ids::MAX_HEALTH), 32.0);
        assert_eq!(
            supplied(kind, ids::MOVEMENT_SPEED),
            0.090_000_003_576_278_69
        );
        assert_eq!(supplied(kind, ids::JUMP_STRENGTH), 0.419_999_986_886_978_15);
        assert_eq!(supplied(kind, ids::STEP_HEIGHT), 1.5);
    }
    assert_eq!(supplied(ARMADILLO, ids::MAX_HEALTH), 12.0);
    assert_eq!(supplied(ARMADILLO, ids::MOVEMENT_SPEED), 0.14);
}

#[test]
fn a_fresh_goat_keeps_nothing_and_its_scream_and_horns_survive_its_record() {
    assert_eq!(goat::state_of(&Mob::fresh()), GoatState::default());
    assert_eq!(
        animal::write(&Animal::default(), &GoatState::default()),
        TypeState::default()
    );
    let own = GoatState {
        screaming: true,
        left_horn: false,
        right_horn: true,
        ..GoatState::default()
    };
    let mob = goat_with(&own, &Animal::default());
    assert_eq!(mob.state.version, 1);
    let read = Mob::decode(&mob.encode()).unwrap();
    assert_eq!(goat::state_of(&read), own);
}

#[test]
fn a_goat_shows_its_scream_and_its_horns_at_18_19_and_20_after_the_age_s() {
    let fresh = of(GOAT).shown(&Mob::fresh());
    assert_eq!(
        fresh,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (6, DataShown::Pose(0), true),
            (18, DataShown::Boolean(false), true),
            (19, DataShown::Boolean(true), true),
            (20, DataShown::Boolean(true), true),
        ]
    );
    let shown = of(GOAT).shown(&goat_with(
        &GoatState {
            screaming: true,
            left_horn: true,
            right_horn: false,
            long_jumping: true,
            ..GoatState::default()
        },
        &baby(),
    ));
    // In a long jump, its pose (`LONG_JUMPING`, 6) is shown too.
    assert_eq!(
        shown,
        [
            (16, DataShown::Boolean(true), false),
            (17, DataShown::Boolean(false), true),
            (6, DataShown::Pose(6), false),
            (18, DataShown::Boolean(true), false),
            (19, DataShown::Boolean(true), true),
            (20, DataShown::Boolean(false), false),
        ]
    );
}

#[test]
fn a_goat_s_spawn_draws_its_two_cooldowns_its_scream_and_a_horn_in_the_game_s_order() {
    let (mut screamers, mut one_horned) = (0, 0);
    for seed in 0..2000 {
        let mut mob = Mob::fresh();
        let mut level = draws(seed);
        let mut replay = level.clone();
        spawned(GOAT, &mut mob, &mut level, 0);
        // `initMemories`: the long jump's 600 to 1200, the ram's 600 to 6000.
        let long_jump = replay.next_int(601) as i32 + 600;
        let ram = replay.next_int(5401) as i32 + 600;
        assert!((600..=1200).contains(&long_jump) && (600..=6000).contains(&ram));
        let screaming = replay.next_double() < 0.02;
        let mut expected = GoatState {
            screaming,
            ..GoatState::default()
        };
        if f64::from(replay.next_float()) < 0.100_000_001_490_116_12 {
            if replay.next_bool() {
                expected.left_horn = false;
            } else {
                expected.right_horn = false;
            }
            one_horned += 1;
        }
        screamers += usize::from(screaming);
        assert_eq!(goat::state_of(&mob), expected, "{seed}");
        assert_eq!(level, replay, "{seed}: the draws' count");
    }
    // One in fifty screams, one in ten loses a horn.
    assert!((15..=70).contains(&screamers), "{screamers}");
    assert!((140..=260).contains(&one_horned), "{one_horned}");
    // The cooldowns as `initMemories` draws them.
    let mut level = draws(3);
    let mut replay = level.clone();
    let cooldowns = goat_brain::init_memories(&mut level);
    assert_eq!(
        cooldowns,
        goat_brain::Cooldowns {
            long_jump: replay.next_int(601) as i32 + 600,
            ram: replay.next_int(5401) as i32 + 600,
        }
    );
}

#[test]
fn a_bred_kid_screams_as_the_parent_a_coin_picks_or_one_time_in_fifty() {
    let screaming = goat_with(
        &GoatState {
            screaming: true,
            ..GoatState::default()
        },
        &Animal::default(),
    );
    let quiet = goat_with(&GoatState::default(), &Animal::default());
    let mut chosen_screamer = 0;
    for seed in 0..400 {
        let mut stream = draws(seed);
        let mut replay = stream.clone();
        let child = of(GOAT)
            .breed_offspring(&mut Breeding {
                this: &screaming,
                partner: &quiet,
                partner_kind: GOAT,
                child_kind: None,
                draws: &mut stream,
                biome: None,
                egg_user: None,
            })
            .expect("a kid");
        replay.next_int(601);
        replay.next_int(5401);
        let this = replay.next_bool();
        let expected = if this {
            chosen_screamer += 1;
            true
        } else {
            // Drawn only where the chosen parent does not scream.
            replay.next_double() < 0.02
        };
        let own = goat::state_of(&child);
        assert_eq!(own.screaming, expected, "{seed}");
        assert!(own.left_horn && own.right_horn);
        assert_eq!(stream, replay, "{seed}");
    }
    assert!((150..=250).contains(&chosen_screamer), "{chosen_screamer}");
}

#[test]
fn a_screaming_goat_has_its_own_sounds_and_rams_sooner_but_steps_as_any_goat() {
    let plain = goat_with(&GoatState::default(), &Animal::default());
    let screaming = goat_with(
        &GoatState {
            screaming: true,
            ..GoatState::default()
        },
        &Animal::default(),
    );
    let goat = of(GOAT);
    let source = DamageSource::of(DamageType::FALL);
    assert_eq!(
        goat.ambient_sound(&plain),
        Some(sounds::ENTITY_GOAT_AMBIENT)
    );
    assert_eq!(
        goat.ambient_sound(&screaming),
        Some(sounds::ENTITY_GOAT_SCREAMING_AMBIENT)
    );
    assert_eq!(
        goat.hurt_sound(&plain, &source),
        Some(sounds::ENTITY_GOAT_HURT)
    );
    assert_eq!(
        goat.hurt_sound(&screaming, &source),
        Some(sounds::ENTITY_GOAT_SCREAMING_HURT)
    );
    assert_eq!(goat.death_sound(&plain), Some(sounds::ENTITY_GOAT_DEATH));
    assert_eq!(
        goat.death_sound(&screaming),
        Some(sounds::ENTITY_GOAT_SCREAMING_DEATH)
    );
    assert_eq!(goat::milking_sound(&plain), sounds::ENTITY_GOAT_MILK);
    assert_eq!(
        goat::milking_sound(&screaming),
        sounds::ENTITY_GOAT_SCREAMING_MILK
    );
    for mob in [&plain, &screaming] {
        assert_eq!(
            goat.step_sound_on(mob, blocks::STONE),
            Some((sounds::ENTITY_GOAT_STEP, 0.15, 1.0))
        );
    }
    assert_eq!(goat.ambient_interval(), 80);
    assert_eq!(goat.sound_volume(), 1.0);
    assert_eq!(goat_brain::time_between_rams(&plain), (600, 6000));
    assert_eq!(goat_brain::time_between_rams(&screaming), (100, 300));
    assert_eq!(goat_brain::least_ram_cooldown(&screaming), 100);
    assert_eq!(goat_brain::least_ram_cooldown(&plain), 600);
    assert_eq!(
        goat_brain::ram_impact_sound(&screaming),
        sounds::ENTITY_GOAT_SCREAMING_RAM_IMPACT
    );
    assert_eq!(
        goat_brain::prepare_ram_sound(&plain),
        sounds::ENTITY_GOAT_PREPARE_RAM
    );
    assert_eq!(
        goat_brain::long_jump_sound(&screaming),
        sounds::ENTITY_GOAT_SCREAMING_LONG_JUMP
    );
    assert!(!goat_brain::rams(GOAT) && goat_brain::rams(entity_types::ARMOR_STAND as u16));
}

#[test]
fn a_kid_is_smaller_rams_softer_and_a_long_jump_shrinks_a_goat_by_three_tenths() {
    let kid = goat_with(&GoatState::default(), &baby());
    let adult = goat_with(&GoatState::default(), &Animal::default());
    assert_eq!(of(GOAT).dimensions(&kid), Some(goat::BABY));
    assert_eq!(of(GOAT).dimensions(&adult), None);
    assert_eq!(goat_brain::ram_knockback(&kid), 1.0);
    assert_eq!(goat_brain::ram_knockback(&adult), 2.5);
    assert_eq!(goat_brain::attack_damage(&kid), 1.0);
    assert_eq!(goat_brain::attack_damage(&adult), 2.0);
    let info = EntityTypeInfo::of(entity_types::GOAT);
    assert_eq!((info.width, info.height), (0.9, 1.3));
    assert_eq!(
        goat_brain::long_jumping_dimensions(&adult),
        Dimensions {
            width: 0.9 * 0.7,
            height: 1.3 * 0.7,
            eye_height: info.eye_height * 0.7,
        }
    );
    assert_eq!(
        goat_brain::long_jumping_dimensions(&kid),
        Dimensions {
            width: 0.45 * 0.7,
            height: 0.65 * 0.7,
            eye_height: 0.593_75 * 0.7,
        }
    );
    assert_eq!(of(GOAT).head().max_y_rot, 15);
    assert_eq!(of(GOAT).landing(&adult), Some(Landing::Lowered { by: 10 }));
}

#[test]
fn a_horn_comes_off_one_at_a_time_by_a_coin_and_a_kid_or_a_hornless_goat_drops_none() {
    let mut kid = goat_with(&GoatState::default(), &baby());
    assert!(goat_brain::drop_horn(&mut kid, &mut draws(1)).is_none());
    for seed in 0..50 {
        let mut mob = goat_with(&GoatState::default(), &Animal::default());
        let mut stream = draws(seed);
        let mut replay = stream.clone();
        let (horn, motion) = goat_brain::drop_horn(&mut mob, &mut stream).expect("a horn");
        assert_eq!(
            horn,
            ItemStack {
                item: items::GOAT_HORN,
                count: 1
            }
        );
        let left = replay.next_bool();
        let own = goat::state_of(&mob);
        assert_eq!((own.left_horn, own.right_horn), (!left, left));
        let mut between = |min: f32, max: f32| f64::from(replay.next_float() * (max - min) + min);
        assert_eq!(
            motion,
            [between(-0.2, 0.2), between(0.3, 0.7), between(-0.2, 0.2)]
        );
        assert_eq!(stream, replay);
        // The other horn next, with no coin.
        let mut stream = draws(seed + 100);
        let mut replay = stream.clone();
        goat_brain::drop_horn(&mut mob, &mut stream).expect("the other");
        for _ in 0..3 {
            replay.next_float();
        }
        assert_eq!(stream, replay);
        let own = goat::state_of(&mob);
        assert!(!own.left_horn && !own.right_horn);
        assert!(goat_brain::drop_horn(&mut mob, &mut draws(seed)).is_none());
    }
}

#[test]
fn a_goat_s_horn_is_the_instrument_its_uuid_s_hash_picks_with_java_s_generator() {
    // Java's `new Random(42).nextInt()` and `nextInt(10)`.
    let mut java = goat_brain::JavaRandom::new(42);
    assert_eq!(java.next_int(10), 0);
    // `nextInt(4)` of a power of two: the top bits of `next(31)`, of seed 42 the third.
    assert_eq!(goat_brain::JavaRandom::new(42).next_int(4), 2);
    // `UUID.hashCode`.
    assert_eq!(goat_brain::uuid_hash((1_u128 << 64) | 2), 3);
    assert_eq!(goat_brain::uuid_hash(0xFFFF_FFFF_0000_0000_u128 << 64), -1);
    // Seed 42 is the hash of the UUID 0-42.
    assert_eq!(goat_brain::uuid_hash(42), 42);
    assert_eq!(
        goat_brain::horn_instrument(42, false),
        "minecraft:seek_goat_horn"
    );
    assert_eq!(
        goat_brain::horn_instrument(42, true),
        "minecraft:yearn_goat_horn"
    );
}

/// A camel of `shared`'s age with the camel's own part `own`.
fn camel_with(kind: u16, shared: &Animal, own: CamelState) -> Mob {
    let mut mob = healthy(kind, TypeState::default());
    mob.state = animal::write(shared, &HorseState::default());
    camel::update(&mut mob, |camel| *camel = own);
    mob
}

fn sitting_since(game_time: i64) -> CamelState {
    CamelState {
        pose_tick: -game_time,
        ..CamelState::default()
    }
}

#[test]
fn a_camel_s_own_part_follows_the_family_s_in_its_record_and_survives_the_family_s_writes() {
    let own = CamelState {
        pose_tick: -1234,
        dashing: true,
        dash_until: 70,
    };
    let mut mob = camel_with(CAMEL, &Animal::default(), own);
    assert_eq!(camel::camel_of(&mob), own);
    assert_eq!(equine::horse_of(&mob), HorseState::default());
    // What the family's code writes keeps the camel's part.
    of(CAMEL).hurt_taken(&mut mob, &mut draws(1), 5);
    of(CAMEL).drop_equipment(&mut mob);
    assert_eq!(camel::camel_of(&mob), own);
    let read = Mob::decode(&mob.encode()).unwrap();
    assert_eq!(camel::camel_of(&read), own);
    // A fresh camel keeps nothing of its own.
    let mut fresh = camel_with(CAMEL, &Animal::default(), CamelState::default());
    assert_eq!(fresh.state, TypeState::default());
    camel::update(&mut fresh, |camel| camel.pose_tick = 3);
    // The family's state is written whole before the camel's, version byte first.
    let rest = animal::own_bytes(&fresh.state);
    let (_, tail) = postcard::take_from_bytes::<HorseState>(rest).unwrap();
    assert_eq!(tail[0], 1);
}

#[test]
fn a_camel_sits_for_40_ticks_stands_for_52_and_refuses_to_move_meanwhile() {
    let now = 10_000;
    let standing = camel_with(CAMEL, &Animal::default(), CamelState::default());
    assert!(!camel::is_sitting(&standing));
    assert!(!camel::refuse_to_move(&standing, now));
    let sat = camel_with(CAMEL, &Animal::default(), sitting_since(now));
    assert!(camel::is_sitting(&sat));
    assert_eq!(camel::pose_time(&sat, now + 39), 39);
    assert!(camel::in_pose_transition(&sat, now + 39));
    assert!(!camel::in_pose_transition(&sat, now + 40));
    assert!(camel::refuse_to_move(&sat, now + 1000), "sitting");
    let stood = camel_with(
        CAMEL,
        &Animal::default(),
        CamelState {
            pose_tick: now,
            ..CamelState::default()
        },
    );
    assert!(camel::in_pose_transition(&stood, now + 51));
    assert!(!camel::in_pose_transition(&stood, now + 52));
    assert!(!camel::refuse_to_move(&stood, now + 52));
    // Standing up at once is as if 53 ticks ago, never below nothing.
    let mut mob = sat.clone();
    camel::stand_up_instantly(&mut mob, now);
    assert_eq!(camel::camel_of(&mob).pose_tick, now - 53);
    assert!(!camel::refuse_to_move(&mob, now));
    camel::stand_up_instantly(&mut mob, 20);
    assert_eq!(camel::camel_of(&mob).pose_tick, 0);
}

#[test]
fn sitting_down_and_standing_up_sound_and_stamp_the_game_time_in_its_tick() {
    let world = Flat::at(5000);
    let mob = camel_with(CAMEL, &Animal::default(), CamelState::default());
    let mut stream = draws(4);
    let mut replay = stream.clone();
    let (mob, out) = in_tick(&world, CAMEL, mob, 3, &mut stream, camel::sit_down);
    assert_eq!(camel::camel_of(&mob).pose_tick, -5000);
    let pitch = hurt::voice_pitch(&mut replay, false);
    assert_eq!(
        out.events,
        [
            Event::Sound {
                sound: sounds::ENTITY_CAMEL_SIT,
                volume: 0.8,
                pitch,
            },
            Event::GameEvent(GameEvent::EntityAction),
        ]
    );
    assert_eq!(stream, replay);
    // Sitting again does nothing.
    let (mob, out) = in_tick(&world, CAMEL, mob, 4, &mut stream, camel::sit_down);
    assert!(out.events.is_empty());
    let world = Flat::at(5100);
    let (mob, out) = in_tick(&world, CAMEL, mob, 5, &mut stream, camel::stand_up);
    assert_eq!(camel::camel_of(&mob).pose_tick, 5100);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_CAMEL_STAND]);
    // A husk's own.
    let husk = camel_with(HUSK, &Animal::default(), CamelState::default());
    let (_, out) = in_tick(&world, HUSK, husk, 3, &mut stream, camel::sit_down);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_CAMEL_HUSK_SIT]);
}

#[test]
fn a_camel_s_size_is_its_pose_s_and_its_age_s() {
    let adult = Animal::default();
    let calf = baby();
    let cases = [
        (&adult, CamelState::default(), None),
        (&adult, sitting_since(100), Some(camel::ADULT_SITTING)),
        (&calf, CamelState::default(), Some(camel::BABY_STANDING)),
        (&calf, sitting_since(100), Some(camel::BABY_SITTING)),
    ];
    for (shared, own, expected) in cases {
        assert_eq!(
            of(CAMEL).dimensions(&camel_with(CAMEL, shared, own)),
            expected
        );
    }
    assert_eq!(camel::ADULT_SITTING.height, 2.375_f32 - 1.43_f32);
    assert_eq!(camel::ADULT_SITTING.eye_height, 0.845);
    assert_eq!(
        (
            camel::BABY_STANDING.width,
            camel::BABY_STANDING.height,
            camel::BABY_STANDING.eye_height
        ),
        (0.95, 1.4, 1.38)
    );
    assert_eq!(
        (camel::BABY_SITTING.height, camel::BABY_SITTING.eye_height),
        (0.425, 0.41)
    );
    let info = EntityTypeInfo::of(entity_types::CAMEL);
    assert_eq!(
        (info.width, info.height, info.eye_height),
        (1.7, 2.375, 2.275)
    );
}

#[test]
fn a_camel_shows_its_family_s_flags_its_pose_its_dash_and_its_pose_s_tick() {
    let shown = of(CAMEL).shown(&Mob::fresh());
    assert_eq!(
        shown,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Byte(0), true),
            (6, DataShown::Pose(0), true),
            (19, DataShown::Boolean(false), true),
            (20, DataShown::Long(0), true),
        ]
    );
    let sat = camel_with(
        CAMEL,
        &Animal::default(),
        CamelState {
            pose_tick: -77,
            dashing: true,
            dash_until: 3,
        },
    );
    let shown = of(CAMEL).shown(&sat);
    assert!(shown.contains(&(6, DataShown::Pose(10), false)));
    assert!(shown.contains(&(19, DataShown::Boolean(true), false)));
    assert!(shown.contains(&(20, DataShown::Long(-77), false)));
    // Never the tame bit: `isTamed` is the camel's own.
    assert!(shown.contains(&(18, DataShown::Byte(0), true)));
}

#[test]
fn a_camel_s_spawn_stands_it_long_since_and_draws_nothing() {
    let mut mob = Mob::fresh();
    let mut level = draws(8);
    let before = level.clone();
    spawned(CAMEL, &mut mob, &mut level, 24_000);
    assert_eq!(level, before);
    assert_eq!(camel::camel_of(&mob).pose_tick, 24_000 - 53);
    let mut mob = Mob::fresh();
    spawned(HUSK, &mut mob, &mut level, 30);
    assert_eq!(camel::camel_of(&mob).pose_tick, 0);
}

/// The riders' height by `getBodyAnchorAnimationYOffset`'s formula, written again from
/// the bytecode for the test.
fn anchor(
    baby: bool,
    height: f32,
    scale: f32,
    sitting: bool,
    pose_time: Option<f32>,
    first: bool,
) -> f64 {
    let mut y = f64::from(height) - if baby { 0.093_75 } else { 0.375 };
    let sit = scale * 1.43_f32;
    let mid0 = sit - scale * 0.2_f32;
    let low = sit - mid0;
    if let Some(time) = pose_time {
        let total: f32 = if sitting { 40.0 } else { 52.0 };
        let split: f32 = match (sitting, first) {
            (true, _) => 28.0,
            (false, true) => 24.0,
            (false, false) => 32.0,
        };
        let k: f32 = match (sitting, first) {
            (true, true) => 0.5,
            (true, false) => 0.1,
            (false, true) => 0.6,
            (false, false) => 0.35,
        };
        let t = time.clamp(0.0, total);
        let early = t < split;
        let p = if early {
            t / split
        } else {
            (t - split) / (total - split)
        };
        let mid = sit - k * mid0;
        let (a, b) = match (sitting, early) {
            (true, true) => (sit, mid),
            (true, false) => (mid, low),
            (false, true) => (low - sit, low - mid),
            (false, false) => (low - mid, 0.0),
        };
        y += f64::from(a + p * (b - a));
    } else if sitting {
        y += f64::from(low);
    }
    y
}

#[test]
fn a_camel_s_riders_sit_as_high_as_its_pose_s_animation_has_them() {
    let now = 50_000;
    let standing = camel_with(CAMEL, &Animal::default(), CamelState::default());
    let y = camel::riding::body_anchor_y_offset(&standing, now, true, 0.0, 2.375, 1.0);
    assert_eq!(y, anchor(false, 2.375, 1.0, false, None, true));
    assert_eq!(y, 2.375_f32 as f64 - 0.375);
    let sat = camel_with(CAMEL, &Animal::default(), sitting_since(now - 1000));
    let height = camel::ADULT_SITTING.height;
    let y = camel::riding::body_anchor_y_offset(&sat, now, true, 0.0, height, 1.0);
    assert_eq!(y, anchor(false, height, 1.0, true, None, true));
    // Sitting down, 14 ticks in: the first seat halfway to its middle height.
    let sitting = camel_with(CAMEL, &Animal::default(), sitting_since(now - 14));
    for first in [true, false] {
        let y = camel::riding::body_anchor_y_offset(&sitting, now, first, 0.5, height, 1.0);
        assert_eq!(
            y,
            anchor(false, height, 1.0, true, Some(14.5), first),
            "{first}"
        );
    }
    // Standing up, 30 ticks in, a calf's of its scale.
    let rising = camel_with(
        CAMEL,
        &baby(),
        CamelState {
            pose_tick: now - 30,
            ..CamelState::default()
        },
    );
    for first in [true, false] {
        let y = camel::riding::body_anchor_y_offset(&rising, now, first, 0.0, 1.4, 0.6);
        assert_eq!(
            y,
            anchor(true, 1.4, 0.6, false, Some(30.0), first),
            "{first}"
        );
    }
}

#[test]
fn a_camel_seats_a_second_rider_behind_and_an_animal_further_forward_turned_with_it() {
    let now = 50_000;
    let mob = camel_with(CAMEL, &Animal::default(), CamelState::default());
    let y = camel::riding::body_anchor_y_offset(&mob, now, true, 0.0, 2.375, 1.0) as f32;
    let alone = camel::riding::seat(&mob, now, 0, 1, false, 2.375, 1.0, 0.0, false);
    assert_eq!(alone, [0.0, f64::from(y), 0.5]);
    let behind = camel::riding::seat(&mob, now, 1, 2, false, 2.375, 1.0, 0.0, false);
    assert_eq!(behind[2], f64::from(-0.7_f32));
    let animal_behind = camel::riding::seat(&mob, now, 1, 2, true, 2.375, 1.0, 0.0, false);
    assert_eq!(animal_behind[2], f64::from(-0.7_f32 + 0.2_f32));
    // An animal alone has no shift.
    let animal_alone = camel::riding::seat(&mob, now, 0, 1, true, 2.375, 1.0, 0.0, false);
    assert_eq!(animal_alone[2], 0.5);
    // Turned a quarter: the seat ahead is to the camel's west.
    let turned = camel::riding::seat(&mob, now, 0, 1, false, 2.375, 1.0, 90.0, false);
    assert!(
        (turned[0] + 0.5).abs() < 1e-6 && turned[2].abs() < 1e-6,
        "{turned:?}"
    );
    // A removed camel's riders sit at a hundredth.
    let removed = camel::riding::seat(&mob, now, 0, 1, false, 2.375, 1.0, 0.0, true);
    assert_eq!(removed[1], f64::from(0.01_f32));
    assert!(camel::riding::can_add_passenger(2) && !camel::riding::can_add_passenger(3));
    assert!(camel::riding::click_mounts(&mob, 1) && !camel::riding::click_mounts(&mob, 2));
    assert_eq!(
        camel::riding::age_scale(&camel_with(CAMEL, &baby(), CamelState::default())),
        0.6
    );
}

#[test]
fn a_dash_pushes_a_camel_along_its_look_and_up_and_waits_55_ticks() {
    let mut mob = camel_with(CAMEL, &Animal::default(), CamelState::default());
    let mut saddled = mob.clone();
    saddled.body.equipment.slots[usize::from(crate::living::slots::SADDLE)] = Some(ItemStack {
        item: items::SADDLE,
        count: 1,
    });
    assert!(!camel::riding::may_dash(&mob, true, 0), "no saddle");
    assert!(camel::riding::may_dash(&saddled, true, 0));
    assert!(!camel::riding::may_dash(&saddled, false, 0), "in the air");
    assert!(camel::riding::can_jump(&saddled, 100_000));
    let speed = 0.090_000_003_576_278_69;
    let push = camel::riding::execute_riders_jump(&mut mob, 10, 1.0, (0.0, 30.0), speed, 1.0, 0.42);
    let by = f64::from(22.2222_f32) * speed;
    assert!((push[0]).abs() < 1e-12);
    assert!((push[2] - by).abs() < 1e-9, "{push:?}");
    assert_eq!(push[1], f64::from(1.4285_f32) * f64::from(0.42_f32));
    assert_eq!(camel::riding::jump_cooldown(&mob, 10), 55);
    assert_eq!(camel::riding::jump_cooldown(&mob, 60), 5);
    assert!(camel::camel_of(&mob).dashing);
    // A sprinting rider is faster only while the dash is ready.
    let base = 0.090_000_003_576_278_69_f64 as f32;
    assert_eq!(camel::riding::ridden_speed(CAMEL, &mob, true, 20), base);
    assert_eq!(
        camel::riding::ridden_speed(CAMEL, &mob, true, 65),
        base + 0.1
    );
    assert_eq!(camel::riding::ridden_speed(CAMEL, &mob, false, 65), base);
    // A sitting camel keeps its own rotation and takes no input.
    let sat = camel_with(CAMEL, &Animal::default(), sitting_since(100));
    assert_eq!(
        camel::riding::ridden_rotation(&sat, 1000, (10.0, 2.0), (90.0, 40.0)),
        (10.0, 2.0)
    );
    assert!(camel::riding::refuses_ridden_input(&sat, 1000));
    assert_eq!(
        camel::riding::ridden_rotation(&mob, 1000, (10.0, 2.0), (90.0, 40.0)),
        (90.0, 20.0)
    );
}

#[test]
fn a_landed_dash_ends_under_50_and_its_cooldown_sounds_ready_at_nothing() {
    let world = Flat::at(1000);
    let mob = camel_with(
        CAMEL,
        &Animal::default(),
        CamelState {
            pose_tick: 0,
            dashing: true,
            dash_until: 100,
        },
    );
    let mut stream = draws(2);
    // At 51 left it is still dashing; at 49 it ends on the ground.
    let (mob, out) = in_tick(&world, CAMEL, mob, 49, &mut stream, |tick| {
        of(CAMEL).after_tick(tick);
    });
    assert!(camel::camel_of(&mob).dashing && out.local.is_empty());
    let (mob, _) = in_tick(&world, CAMEL, mob, 51, &mut stream, |tick| {
        of(CAMEL).after_tick(tick);
    });
    assert!(!camel::camel_of(&mob).dashing);
    let (mob, out) = in_tick(&world, CAMEL, mob, 98, &mut stream, |tick| {
        of(CAMEL).after_tick(tick);
    });
    assert!(out.local.is_empty());
    let (mob, out) = in_tick(&world, CAMEL, mob, 99, &mut stream, |tick| {
        of(CAMEL).after_tick(tick);
    });
    assert_eq!(
        out.local,
        [Local::Sound {
            sound: sounds::ENTITY_CAMEL_DASH_READY,
            source: crate::physics::item::SOUND_SOURCE_NEUTRAL,
            at: [0.5, 0.5, 0.5],
            volume: 1.0,
            pitch: 1.0,
        }]
    );
    assert_eq!(camel::camel_of(&mob).dash_until, 0);
}

#[test]
fn a_sitting_camel_in_water_stands_up_at_once_and_holds_its_head_to_its_body() {
    let world = Flat::at(2000);
    let mut mob = camel_with(CAMEL, &Animal::default(), sitting_since(1500));
    mob.body.head.body_yaw = 0.0;
    mob.body.head.head_yaw = 50.0;
    let mut stream = draws(3);
    let (mob, out) = in_tick(&world, CAMEL, mob, 1, &mut stream, |tick| {
        of(CAMEL).after_tick(tick);
    });
    assert_eq!(mob.body.head.head_yaw, 30.0);
    assert!(camel::is_sitting(&mob) && out.events.is_empty());
    let (mob, out) = in_tick(&world, CAMEL, mob, 2, &mut stream, |tick| {
        tick.living.body.in_water = true;
        of(CAMEL).after_tick(tick);
    });
    assert_eq!(camel::camel_of(&mob).pose_tick, 2000 - 53);
    assert_eq!(out.events, [Event::GameEvent(GameEvent::EntityAction)]);
}

#[test]
fn a_hurt_camel_stands_up_at_once_before_the_hurt_is_heard() {
    let mut mob = camel_with(CAMEL, &Animal::default(), sitting_since(1000));
    let (took, out) = hurt(CAMEL, &mut mob, &by_player(), 2.0, 5, 3000, &mut draws(1));
    assert!(took);
    assert_eq!(camel::camel_of(&mob).pose_tick, 3000 - 53);
    let action = out
        .events
        .iter()
        .position(|event| *event == Event::GameEvent(GameEvent::EntityAction))
        .expect("acted");
    let damage = out
        .events
        .iter()
        .position(|event| *event == Event::GameEvent(GameEvent::EntityDamage))
        .expect("damaged");
    assert!(action < damage);
    assert!(sounds_of(&out).contains(&sounds::ENTITY_CAMEL_HURT));
}

#[test]
fn a_camel_steps_softly_on_sand_and_its_husk_at_four_tenths() {
    let camel = of(CAMEL);
    let mob = Mob::fresh();
    assert_eq!(
        camel.step_sound_on(&mob, blocks::SAND),
        Some((sounds::ENTITY_CAMEL_STEP_SAND, 1.0, 1.0))
    );
    assert_eq!(
        camel.step_sound_on(&mob, blocks::RED_SAND),
        Some((sounds::ENTITY_CAMEL_STEP_SAND, 1.0, 1.0))
    );
    assert_eq!(
        camel.step_sound_on(&mob, blocks::STONE),
        Some((sounds::ENTITY_CAMEL_STEP, 1.0, 1.0))
    );
    assert_eq!(
        of(HUSK).step_sound_on(&mob, blocks::SAND),
        Some((sounds::ENTITY_CAMEL_HUSK_STEP_SAND, 0.4, 1.0))
    );
    let source = DamageSource::of(DamageType::FALL);
    assert_eq!(
        camel.ambient_sound(&mob),
        Some(sounds::ENTITY_CAMEL_AMBIENT)
    );
    assert_eq!(
        of(HUSK).ambient_sound(&mob),
        Some(sounds::ENTITY_CAMEL_HUSK_AMBIENT)
    );
    assert_eq!(
        of(HUSK).hurt_sound(&mob, &source),
        Some(sounds::ENTITY_CAMEL_HUSK_HURT)
    );
    assert_eq!(
        of(HUSK).death_sound(&mob),
        Some(sounds::ENTITY_CAMEL_HUSK_DEATH)
    );
    assert_eq!((camel.ambient_interval(), camel.sound_volume()), (400, 0.8));
    assert_eq!(camel.head().max_y_rot, 30);
    assert_eq!(camel.landing(&mob), Some(Landing::Horse { baby: false }));
}

#[test]
fn camels_breed_with_camels_and_a_camel_husk_with_nothing() {
    let entity = |key: u64, kind: u16, mob: &Mob| clustine_world::Entity {
        key: EntityId(key),
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key)),
        position: [0.0; 3],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 10,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    };
    let mut in_love = Animal::default();
    in_love.set_in_love(0);
    let a = camel_with(CAMEL, &in_love, CamelState::default());
    let b = camel_with(CAMEL, &in_love, CamelState::default());
    let husk = camel_with(HUSK, &in_love, CamelState::default());
    let (ea, eb, eh) = (
        entity(1, CAMEL, &a),
        entity(2, CAMEL, &b),
        entity(3, HUSK, &husk),
    );
    let mate = |entity, mob| super::Mate { entity, mob };
    assert!(of(CAMEL).can_mate(mate(&ea, &a), mate(&eb, &b)));
    assert!(!of(CAMEL).can_mate(mate(&ea, &a), mate(&ea, &a)), "itself");
    assert!(!of(CAMEL).can_mate(mate(&ea, &a), mate(&eh, &husk)));
    assert!(!of(HUSK).can_mate(mate(&eh, &husk), mate(&ea, &a)));
    // Hurt, it may not parent.
    let mut hurt_camel = b.clone();
    hurt_camel.body.vitals.health = 10.0;
    let ehc = entity(2, CAMEL, &hurt_camel);
    assert!(!of(CAMEL).can_mate(mate(&ea, &a), mate(&ehc, &hurt_camel)));
    let mut breeding = Breeding {
        this: &a,
        partner: &b,
        partner_kind: CAMEL,
        child_kind: None,
        draws: &mut draws(1),
        biome: None,
        egg_user: None,
    };
    assert!(of(CAMEL).breed_offspring(&mut breeding).is_some());
    assert!(of(HUSK).breed_offspring(&mut breeding).is_none());
}

fn armadillo_with(own: ArmadilloState, shared: &Animal) -> Mob {
    healthy(ARMADILLO, animal::write(shared, &own))
}

#[test]
fn an_armadillo_s_states_are_sent_by_their_ids_and_shown_by_their_own_serializer() {
    let ids: Vec<_> = [Phase::Idle, Phase::Rolling, Phase::Scared, Phase::Unrolling]
        .iter()
        .map(|phase| (phase.id(), phase.animation_duration(), phase.threatened()))
        .collect();
    assert_eq!(
        ids,
        [(0, 0, false), (1, 10, true), (2, 50, true), (3, 30, true)]
    );
    let shown = of(ARMADILLO).shown(&Mob::fresh());
    assert_eq!(
        shown.last(),
        Some(&(
            18,
            DataShown::Serialised(Serialiser::ArmadilloState, 0),
            true
        ))
    );
    let scared = armadillo_with(
        ArmadilloState {
            phase: Phase::Scared,
            ..ArmadilloState::default()
        },
        &Animal::default(),
    );
    assert_eq!(
        of(ARMADILLO).shown(&scared).last(),
        Some(&(
            18,
            DataShown::Serialised(Serialiser::ArmadilloState, 2),
            false
        ))
    );
}

#[test]
fn an_armadillo_s_count_in_its_state_begins_again_only_where_the_state_changes() {
    let mut own = ArmadilloState::default();
    own.switch_to(Phase::Rolling, 7);
    assert_eq!(own.in_state_ticks(18), 11);
    own.switch_to(Phase::Rolling, 15);
    assert_eq!(own.in_state_ticks(18), 11, "the same state again");
    own.switch_to(Phase::Scared, 18);
    assert_eq!(own.in_state_ticks(18), 0);
}

#[test]
fn an_armadillo_draws_its_first_scute_time_after_the_brain_s_draws_in_its_constructor() {
    let mut mob = Mob::fresh();
    let mut own = draws(5);
    let mut replay = own.clone();
    of(ARMADILLO).construct(&mut mob, &mut own);
    let time = replay.next_int(6000) + 6000;
    assert_eq!(armadillo::state_of(&mob).scute_at, time);
    assert_eq!(armadillo::state_of(&mob).scute_time(0), i64::from(time));
    assert_eq!(own, replay);
}

#[test]
fn a_living_attacker_rolls_an_armadillo_up_and_then_it_takes_half_a_hit_less_one() {
    let mut mob = armadillo_with(ArmadilloState::default(), &Animal::default());
    mob.body.inputs.zza = 1.0;
    mob.body.inputs.speed = 0.14;
    let (took, out) = hurt(ARMADILLO, &mut mob, &by_player(), 4.0, 30, 0, &mut draws(1));
    assert!(took);
    assert_eq!(mob.body.vitals.health, 8.0);
    let own = armadillo::state_of(&mob);
    assert_eq!((own.phase, own.phase_since), (Phase::Rolling, 30));
    assert_eq!((mob.body.inputs.zza, mob.body.inputs.speed), (0.0, 0.0));
    // The hurt's sound comes after the roll's; the roll acts before.
    assert_eq!(
        sounds_of(&out),
        [
            sounds::ENTITY_ARMADILLO_ROLL,
            sounds::ENTITY_ARMADILLO_HURT_REDUCED
        ]
    );
    assert!(
        out.events
            .contains(&Event::GameEvent(GameEvent::EntityAction))
    );
    // Rolled up, a hit of 6 takes (6 − 1) / 2, after its cooldown.
    let (_, out) = hurt(ARMADILLO, &mut mob, &by_player(), 6.0, 60, 0, &mut draws(2));
    assert_eq!(mob.body.vitals.health, 8.0 - 2.5);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_ARMADILLO_HURT_REDUCED]);
    // A hit under one heals nothing.
    let before = mob.body.vitals.health;
    hurt(ARMADILLO, &mut mob, &by_player(), 0.5, 90, 0, &mut draws(3));
    assert_eq!(mob.body.vitals.health, before);
    assert_eq!(of(ARMADILLO).hurt_amount(&mob, 0.5), -0.25);
}

#[test]
fn lava_rolls_an_armadillo_out_and_one_in_a_liquid_or_without_ai_does_not_roll_up() {
    let scared = ArmadilloState {
        phase: Phase::Scared,
        ..ArmadilloState::default()
    };
    let mut mob = armadillo_with(scared, &Animal::default());
    let (_, out) = hurt(
        ARMADILLO,
        &mut mob,
        &DamageSource::of(DamageType::LAVA),
        4.0,
        30,
        0,
        &mut draws(1),
    );
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Idle);
    assert!(sounds_of(&out).contains(&sounds::ENTITY_ARMADILLO_UNROLL_FINISH));
    // A fall hurts without rolling either way.
    let mut mob = armadillo_with(ArmadilloState::default(), &Animal::default());
    hurt(
        ARMADILLO,
        &mut mob,
        &DamageSource::of(DamageType::FALL),
        2.0,
        30,
        0,
        &mut draws(1),
    );
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Idle);
    let mut mob = armadillo_with(ArmadilloState::default(), &Animal::default());
    mob.body.flags.no_ai = true;
    hurt(ARMADILLO, &mut mob, &by_player(), 2.0, 30, 0, &mut draws(1));
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Idle);
    assert!(!armadillo::can_stay_rolled_up(
        false, true, false, false, false
    ));
    assert!(!armadillo::can_stay_rolled_up(
        false, false, true, false, false
    ));
    assert!(armadillo::can_stay_rolled_up(
        false, false, false, false, false
    ));
}

#[test]
fn a_fresh_load_puts_a_camel_s_dash_cooldown_and_an_armadillo_s_count_in_its_state_back() {
    // Neither is saved by the game, so a reload begins them again.
    let mut mob = camel_with(
        CAMEL,
        &Animal::default(),
        CamelState {
            pose_tick: -40,
            dashing: true,
            dash_until: 90,
        },
    );
    camel::fresh_load(&mut mob);
    assert_eq!(
        camel::camel_of(&mob),
        CamelState {
            pose_tick: -40,
            dashing: true,
            dash_until: 0,
        }
    );
    let mut mob = armadillo_with(
        ArmadilloState {
            phase: Phase::Scared,
            phase_since: 3,
            scute_at: 7000,
        },
        &Animal::default(),
    );
    armadillo::fresh_load(&mut mob, 500);
    let own = armadillo::state_of(&mob);
    assert_eq!(
        (own.phase, own.in_state_ticks(500), own.scute_at),
        (Phase::Scared, 0, 7000)
    );
}

#[test]
fn a_rolled_up_armadillo_is_silent_its_hurt_reduced_and_a_baby_s_size_six_tenths() {
    let scared = armadillo_with(
        ArmadilloState {
            phase: Phase::Unrolling,
            ..ArmadilloState::default()
        },
        &Animal::default(),
    );
    let idle = armadillo_with(ArmadilloState::default(), &Animal::default());
    let source = DamageSource::of(DamageType::FALL);
    let armadillo = of(ARMADILLO);
    assert_eq!(armadillo.ambient_sound(&scared), None);
    assert_eq!(
        armadillo.ambient_sound(&idle),
        Some(sounds::ENTITY_ARMADILLO_AMBIENT)
    );
    assert_eq!(
        armadillo.hurt_sound(&scared, &source),
        Some(sounds::ENTITY_ARMADILLO_HURT_REDUCED)
    );
    assert_eq!(
        armadillo.hurt_sound(&idle, &source),
        Some(sounds::ENTITY_ARMADILLO_HURT)
    );
    assert_eq!(
        armadillo.death_sound(&idle),
        Some(sounds::ENTITY_ARMADILLO_DEATH)
    );
    assert_eq!(
        armadillo.step_sound_on(&idle, blocks::STONE),
        Some((sounds::ENTITY_ARMADILLO_STEP, 0.15, 1.0))
    );
    let pup = armadillo_with(ArmadilloState::default(), &baby());
    assert_eq!(
        armadillo.dimensions(&pup),
        Some(Dimensions {
            width: 0.7 * 0.6,
            height: 0.65 * 0.6,
            eye_height: 0.218_75,
        })
    );
    assert_eq!(armadillo.dimensions(&idle), None);
}

#[test]
fn rolling_up_turns_scared_after_ten_ticks_with_nothing_more_that_tick_then_peeks_and_unrolls() {
    let world = Flat::at(0);
    let mut stream = draws(6);
    let mob = armadillo_with(ArmadilloState::default(), &Animal::default());
    let mut ball = armadillo::brain::BallUp::default();
    // Started at 100: rolling up, and its first tick in the same brain tick.
    let (mob, out) = in_tick(&world, ARMADILLO, mob, 100, &mut stream, |tick| {
        armadillo::brain::ball_up_start(tick);
        armadillo::brain::ball_up_tick(tick, &mut ball, 80);
    });
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Rolling);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_ARMADILLO_ROLL]);
    // Danger about at its first tick drew a peek's time.
    assert!(ball.danger_was_around && (150..=450).contains(&ball.next_peek_timer));
    let peek = ball.next_peek_timer;
    let mut mob = mob;
    for lived in 101..=110 {
        let (after, _) = in_tick(&world, ARMADILLO, mob, lived, &mut stream, |tick| {
            armadillo::brain::ball_up_tick(tick, &mut ball, 79);
        });
        mob = after;
        assert_eq!(armadillo::state_of(&mob).phase, Phase::Rolling, "{lived}");
    }
    // Its eleventh: scared, the landing's sound, and the tick ends there though the
    // danger is near its end.
    let (mob, out) = in_tick(&world, ARMADILLO, mob, 111, &mut stream, |tick| {
        armadillo::brain::ball_up_tick(tick, &mut ball, 10);
    });
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Scared);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_ARMADILLO_LAND]);
    assert_eq!(ball.next_peek_timer, peek - 11);
    // Danger near its end: unrolling, with its sound; danger back: scared again.
    let (mob, out) = in_tick(&world, ARMADILLO, mob, 112, &mut stream, |tick| {
        armadillo::brain::ball_up_tick(tick, &mut ball, 29);
    });
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Unrolling);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_ARMADILLO_UNROLL_START]);
    assert!(!ball.danger_was_around);
    let (mob, _) = in_tick(&world, ARMADILLO, mob, 113, &mut stream, |tick| {
        armadillo::brain::ball_up_tick(tick, &mut ball, 31);
    });
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Scared);
    // A peek when its count is out, on the ground, with danger about.
    ball.next_peek_timer = 1;
    ball.danger_was_around = true;
    let (mob, out) = in_tick(&world, ARMADILLO, mob, 114, &mut stream, |tick| {
        armadillo::brain::ball_up_tick(tick, &mut ball, 78);
    });
    assert!(out.events.contains(&Event::Entity(armadillo::PEEK)));
    assert!((150..=450).contains(&ball.next_peek_timer));
    // The danger gone: the core's rolling out, idle with its sound.
    let (mob, out) = in_tick(&world, ARMADILLO, mob, 115, &mut stream, |tick| {
        assert!(armadillo::brain::rolling_out(tick));
    });
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Idle);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_ARMADILLO_UNROLL_FINISH]);
    assert!(!armadillo::brain::ball_up_may_go_on(&mob));
}

#[test]
fn an_armadillo_is_scared_near_by_the_undead_what_hurt_it_and_a_sprinting_or_riding_player() {
    use armadillo::brain::{Near, is_scared_by};
    let own = Aabb::of_entity([0.5, 0.0, 0.5], 0.7, 0.65);
    let near = |x: f64, kind: i32, hurt_it_last: bool, player| Near {
        bounds: Aabb::of_entity([x, 0.0, 0.5], 0.6, 1.8),
        kind: kind as u16,
        hurt_it_last,
        player,
    };
    let player = entity_types::PLAYER;
    assert!(is_scared_by(
        own,
        &near(5.0, player, false, Some((false, true, false)))
    ));
    assert!(is_scared_by(
        own,
        &near(5.0, player, false, Some((false, false, true)))
    ));
    assert!(!is_scared_by(
        own,
        &near(5.0, player, false, Some((false, false, false)))
    ));
    assert!(
        !is_scared_by(own, &near(5.0, player, false, Some((true, true, false)))),
        "spectating"
    );
    assert!(
        !is_scared_by(own, &near(9.0, player, false, Some((false, true, false)))),
        "beyond 7"
    );
    assert!(
        is_scared_by(own, &near(5.0, entity_types::CAMEL_HUSK, false, None)),
        "undead"
    );
    assert!(is_scared_by(
        own,
        &near(5.0, entity_types::ZOMBIE, false, None)
    ));
    assert!(!is_scared_by(
        own,
        &near(5.0, entity_types::COW, false, None)
    ));
    assert!(
        is_scared_by(own, &near(5.0, entity_types::COW, true, None)),
        "what hurt it"
    );
}

// ------------------------------------------------------------------------------------
// Their brains (the brief's 7.6.3, 7.7.3, 7.8.3 and 10.7.3; `docs/groundwork/
// n7j-plan.md`, sections 2 and 8): what each registers, its sensors' delays, its
// activities and their switching, its own behaviours and sensor, and their draws.

use crate::mobs::ai::{Ai, Setup};
use crate::mobs::brain::{
    self as machinery, Activity, ActivityData, Brain, BrainState, HeldTimed, Memories, MemoryKind,
    Provider, Sensed, Status, Value,
};
use crate::mobs::view::SeenPlayer;

/// The world's seed of the streams the ticked mobs draw from.
const SEED: u64 = 9;

/// A creative player standing at `position`, holding `held`, sprinting or not.
fn seen_player(key: u64, position: Vec3, held: Option<i32>, sprinting: bool) -> SeenPlayer {
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
        sprinting,
        alive: true,
        sleeping: None,
        last_hurt_mob: None,
        last_hurt_by: None,
    }
}

/// The record of the mob `mob` of the type `kind` at `position`.
fn entity_of(key: EntityId, kind: u16, position: Vec3, mob: &Mob) -> clustine_world::Entity {
    clustine_world::Entity {
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

/// A mob of the type `kind` that thinks: ticked by its own `customServerAiStep` (its
/// brain, its activity's update, the rest of its step), its record and the memories the
/// region keeps beside it carried between ticks as a region carries them, its game time
/// one on at each tick.
struct Thinking {
    world: Flat,
    kind: u16,
    mob: Mob,
    living: Living,
    sensed: Option<Sensed>,
    lived: u32,
}

impl Thinking {
    /// A mob made as an egg makes it (its brain's delays and its type's own draws from
    /// its stream), with `state` as its type state, standing at the land's corner.
    fn new(kind: u16, state: TypeState) -> Self {
        let mut mob = healthy(kind, state);
        machinery::construct(of(kind), &mut mob, &mut draws(1));
        Self {
            world: Flat::at(10_000),
            kind,
            mob,
            living: living(kind, [0.5, 0.0, 0.5]),
            sensed: None,
            lived: 0,
        }
    }

    fn provider(&self) -> &'static Provider {
        of(self.kind).brain().expect("a brain")
    }

    /// The mob's stream in its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(SEED, KEY, self.lived)
    }

    /// One tick of `what`: what it showed and how many draws it made.
    fn step(&mut self, what: impl FnOnce(&mut MobTick<'_>)) -> (MobOutput, u64) {
        let mut stream = self.stream();
        let start = stream.clone();
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
            yaw: 0.0,
            pitch: 0.0,
            mob_type: of(self.kind),
            world: &self.world,
            draws: &mut stream,
            out: MobOutput::default(),
        };
        what(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.sensed = tick.sensed;
        let out = tick.out;
        self.lived += 1;
        self.world.game_time += 1;
        (out, stream.draws_since(&start))
    }

    /// One tick of its own `customServerAiStep`.
    fn think(&mut self) -> MobOutput {
        self.step(|tick| tick.mob_type.custom_server_ai_step(tick))
            .0
    }

    /// One tick of `provider`'s brain alone in its place, `before` on its brain ahead of
    /// the tick: what it showed and how many draws it made.
    fn alone(
        &mut self,
        provider: &'static Provider,
        before: impl FnOnce(&mut Brain),
    ) -> (MobOutput, u64) {
        self.step(|tick| {
            let now = tick.lived;
            let time = tick.world.game_time();
            let mut brain = Brain::read(provider, &tick.mob.mind.brain, tick.sensed.as_ref(), now);
            before(&mut brain);
            let mut setup = Setup::default();
            tick.mob_type.register_goals(&mut setup);
            let config = setup.config;
            let (entries, sensed) = {
                let mut ai = Ai {
                    tick,
                    config: &config,
                    sensing: Default::default(),
                    navigation_ticked: true,
                };
                brain.tick(&mut ai, now, time);
                brain.write()
            };
            tick.mob.mind.brain = entries;
            tick.sensed = Some(sensed);
        })
    }

    /// Its brain as its last tick left it.
    fn brain(&self) -> Brain {
        Brain::read(
            self.provider(),
            &self.mob.mind.brain,
            self.sensed.as_ref(),
            self.lived.saturating_sub(1),
        )
    }

    /// Its memories as its last tick left them, every countdown after its lowering.
    fn memories(&self) -> Memories {
        let mut memories = self.brain().memories;
        memories.end_tick();
        memories
    }

    /// What `write` does to its memories between ticks.
    fn remember(&mut self, write: impl FnOnce(&mut Memories)) {
        let provider = self.provider();
        let mut memories = machinery::memories_of(provider, &self.mob, self.lived);
        write(&mut memories);
        machinery::write_memories(provider, &mut self.mob, memories);
    }
}

/// The memories the brain of the type `kind` registers.
fn registered(kind: u16) -> Vec<&'static str> {
    let provider = of(kind).brain().expect("a brain");
    let brain = Brain::read(provider, &[], Some(&Sensed::new()), 0);
    brain
        .memories
        .registered
        .iter()
        .map(|kind| kind.name())
        .collect()
}

fn sorted(mut names: Vec<&'static str>) -> Vec<&'static str> {
    names.sort_unstable();
    names
}

#[test]
fn each_highland_brain_registers_the_memories_its_sensors_and_behaviours_name() {
    // The game's: the sensors' `requires`, then every behaviour's entry conditions
    // (`Brain.provider(sensors, activities)`, which names no memory of its own).
    let goat = [
        // NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS, NEAREST_ITEMS, NEAREST_ADULT,
        // HURT_BY, FOOD_TEMPTATIONS.
        "mobs",
        "visible_mobs",
        "nearest_players",
        "nearest_visible_player",
        "nearest_visible_targetable_player",
        "nearest_visible_targetable_players",
        "nearest_visible_wanted_item",
        "nearest_visible_adult",
        "hurt_by",
        "hurt_by_entity",
        "tempting_player",
        // AnimalPanic, LookAtTargetSink, MoveToTargetSink, the three countdowns.
        "is_panicking",
        "look_target",
        "cant_reach_walk_target_since",
        "path",
        "walk_target",
        "temptation_cooldown_ticks",
        "long_jump_cooling_down",
        "ram_cooldown_ticks",
        // AnimalMakeLove, the long jump's two, the ram's two.
        "breed_target",
        "long_jump_mid_jump",
        "ram_target",
    ];
    assert_eq!(sorted(registered(GOAT)), sorted(goat.to_vec()));
    let camel = [
        "mobs",
        "visible_mobs",
        "hurt_by",
        "hurt_by_entity",
        "tempting_player",
        "nearest_visible_adult",
        "is_panicking",
        "look_target",
        "cant_reach_walk_target_since",
        "path",
        "walk_target",
        "temptation_cooldown_ticks",
        "gaze_cooldown_ticks",
        "breed_target",
    ];
    assert_eq!(sorted(registered(CAMEL)), sorted(camel.to_vec()));
    assert_eq!(sorted(registered(HUSK)), sorted(camel.to_vec()));
    // The armadillo's scare sensor requires `mobs`; its rolling out names its danger.
    let mut armadillo = camel.to_vec();
    armadillo.push("danger_detected_recently");
    assert_eq!(sorted(registered(ARMADILLO)), sorted(armadillo));
}

#[test]
fn a_highland_brain_draws_its_sensors_delays_in_their_order_before_the_type_s_own_draws() {
    for (kind, rates) in [
        (GOAT, vec![20, 20, 20, 20, 20, 20]),
        (CAMEL, vec![20, 20, 20, 20]),
        (HUSK, vec![20, 20, 20, 20]),
        (ARMADILLO, vec![20, 20, 20, 20, 5]),
    ] {
        let mut mob = Mob::fresh();
        let mut own = draws(17);
        let mut replay = own.clone();
        machinery::construct(of(kind), &mut mob, &mut own);
        // Each `nextInt(scanRate)`; a delay of 0 or 1 scans at the first tick.
        let expected: Vec<u32> = rates
            .iter()
            .map(|rate| replay.next_int(*rate).max(1) - 1)
            .collect();
        let state: BrainState = crate::mobs::part::decode(&mob.mind.brain).unwrap();
        assert_eq!(state.next_scans, expected, "{kind}");
        if kind == ARMADILLO {
            // Then its constructor's scute time.
            let time = replay.next_int(6000) + 6000;
            assert_eq!(armadillo::state_of(&mob).scute_at, time);
        }
        assert_eq!(own, replay, "{kind}: the draws' count");
    }
}

/// A brain of `provider` at rest with the memories `write` gives it, switched as the
/// type's update switches it by `update`, from the active activity `from`.
fn switched(
    provider: &'static Provider,
    from: Activity,
    update: &[Activity],
    write: impl FnOnce(&mut Memories),
) -> Option<Activity> {
    let mut brain = Brain::read(provider, &[], Some(&Sensed::new()), 5);
    brain.state.active = Some(from);
    write(&mut brain.memories);
    brain.set_active_to_first_valid(update);
    brain.state.active
}

#[test]
fn a_goat_rams_before_it_jumps_jumps_before_it_idles_and_where_none_may_keeps_its_activity() {
    let provider = &goat::PROVIDER;
    let update = &goat::ACTIVITIES;
    let ram = |memories: &mut Memories| memories.set(MemoryKind::RamCooldownTicks, Value::Int(9));
    let jump =
        |memories: &mut Memories| memories.set(MemoryKind::LongJumpCoolingDown, Value::Int(9));
    // Neither cooldown (a goat of no spawn): it rams.
    assert_eq!(
        switched(provider, Activity::Idle, update, |_| {}),
        Some(Activity::Ram)
    );
    // Its ram cooling down: it jumps.
    assert_eq!(
        switched(provider, Activity::Idle, update, ram),
        Some(Activity::LongJump)
    );
    // Both: it idles.
    assert_eq!(
        switched(provider, Activity::Ram, update, |memories| {
            ram(memories);
            jump(memories);
        }),
        Some(Activity::Idle)
    );
    // Its ram's out but a player tempts it: neither the ram nor, its jump cooling down,
    // the jump nor the idling may be, and it stays as it was (the brief's 7.0.3.5).
    let tempted = |memories: &mut Memories| {
        jump(memories);
        memories.set(MemoryKind::TemptingPlayer, Value::Entity(EntityId(7)));
    };
    assert_eq!(
        switched(provider, Activity::Idle, update, tempted),
        Some(Activity::Idle)
    );
    assert_eq!(
        switched(provider, Activity::LongJump, update, tempted),
        Some(Activity::LongJump)
    );
    // A walk target keeps it from jumping; a breed target from either.
    assert_eq!(
        switched(provider, Activity::Ram, update, |memories| {
            ram(memories);
            memories.set(
                MemoryKind::WalkTarget,
                crate::mobs::brain::behaviours::walk(
                    crate::mobs::brain::Tracker::Block(BlockPos::new(0, 0, 0)),
                    1.0,
                    0,
                ),
            );
        }),
        Some(Activity::Ram)
    );
    assert_eq!(
        switched(provider, Activity::Idle, update, |memories| {
            memories.set(MemoryKind::BreedTarget, Value::Entity(EntityId(7)));
        }),
        Some(Activity::Idle)
    );
}

#[test]
fn a_camel_idles_and_an_armadillo_panics_while_danger_is_remembered_and_it_does_not_panic() {
    for provider in [&camel::PROVIDER, &super::camel_husk::PROVIDER] {
        assert_eq!(
            switched(provider, Activity::Idle, &camel::ACTIVITIES, |_| {}),
            Some(Activity::Idle)
        );
    }
    let provider = &armadillo::PROVIDER;
    let update = &armadillo::ACTIVITIES;
    let danger = |memories: &mut Memories| {
        memories.set_with_expiry(MemoryKind::DangerDetectedRecently, Value::Bool(true), 80);
    };
    assert_eq!(
        switched(provider, Activity::Idle, update, danger),
        Some(Activity::Panic)
    );
    assert_eq!(
        switched(provider, Activity::Panic, update, |memories| {
            danger(memories);
            memories.set(MemoryKind::IsPanicking, Value::Bool(true));
        }),
        Some(Activity::Idle)
    );
    assert_eq!(
        switched(provider, Activity::Panic, update, |_| {}),
        Some(Activity::Idle)
    );
}

#[test]
fn a_goat_s_spawn_makes_its_two_cooldowns_memories_at_rest_that_its_core_counts_down() {
    let mut level = draws(4);
    let mut replay = level.clone();
    let mut goat = Thinking::new(GOAT, TypeState::default());
    spawned(GOAT, &mut goat.mob, &mut level, 0);
    let long_jump = replay.next_int(601) as i32 + 600;
    let ram = replay.next_int(5401) as i32 + 600;
    let memories = machinery::memories_of(&goat::PROVIDER, &goat.mob, 0);
    assert_eq!(
        memories.int(MemoryKind::LongJumpCoolingDown),
        Some(long_jump)
    );
    assert_eq!(memories.int(MemoryKind::RamCooldownTicks), Some(ram));
    // Its first tick idles, its countdowns lowering them from then on by one a tick.
    goat.think();
    assert_eq!(goat.brain().state.active, Some(Activity::Idle));
    assert_eq!(
        goat.memories().int(MemoryKind::LongJumpCoolingDown),
        Some(long_jump - 1)
    );
    for _ in 0..9 {
        goat.think();
    }
    assert_eq!(
        goat.memories().int(MemoryKind::RamCooldownTicks),
        Some(ram - 10)
    );
    // A kid of a breeding has them too.
    let goat_parent = goat_with(&GoatState::default(), &Animal::default());
    let mut breeding = Breeding {
        this: &goat_parent,
        partner: &goat_parent,
        partner_kind: GOAT,
        child_kind: None,
        draws: &mut draws(8),
        biome: None,
        egg_user: None,
    };
    let kid = of(GOAT).breed_offspring(&mut breeding).unwrap();
    let memories = machinery::memories_of(&goat::PROVIDER, &kid, 0);
    assert!(memories.int(MemoryKind::LongJumpCoolingDown).is_some());
    assert!(memories.int(MemoryKind::RamCooldownTicks).is_some());
}

#[test]
fn a_kid_s_attack_damage_is_one_until_it_grows_up_and_an_adult_s_its_type_s_two() {
    let mut kid = Thinking::new(GOAT, animal::write(&baby(), &GoatState::default()));
    kid.remember(|memories| {
        memories.set(MemoryKind::LongJumpCoolingDown, Value::Int(10_000));
        memories.set(MemoryKind::RamCooldownTicks, Value::Int(10_000));
    });
    let damage = crate::mobs::body::attribute(clustine_data::attributes::ATTACK_DAMAGE);
    kid.think();
    assert_eq!(kid.mob.body.attributes.value(GOAT, damage), 1.0);
    // Grown, its base is its type's again.
    animal::update(&mut kid.mob, |shared| shared.set_age(0, 0));
    kid.think();
    assert_eq!(kid.mob.body.attributes.value(GOAT, damage), 2.0);
    // An adult makes no instance of it.
    let mut adult = Thinking::new(GOAT, TypeState::default());
    adult.think();
    assert!(adult.mob.body.attributes.made(damage).is_none());
}

#[test]
fn a_goat_with_nothing_to_ram_gives_up_with_its_head_raised_for_the_least_of_its_cooldown() {
    for (screaming, least) in [(false, 600), (true, 100)] {
        let own = GoatState {
            screaming,
            ..GoatState::default()
        };
        let mut goat = Thinking::new(GOAT, animal::write(&Animal::default(), &own));
        goat.remember(|memories| {
            memories.set(MemoryKind::LongJumpCoolingDown, Value::Int(10_000));
        });
        // A creative player near is no target.
        goat.world
            .players
            .push(seen_player(7, [5.5, 0.0, 0.5], None, false));
        let mut events = Vec::new();
        for _ in 0..3 {
            events.extend(goat.think().events);
        }
        assert_eq!(
            goat.brain().state.active,
            Some(Activity::Idle),
            "{screaming}"
        );
        assert!(events.contains(&Event::Entity(59)));
        assert!(!events.contains(&Event::Entity(58)));
        // Set once as it gave up, then lowered.
        let left = goat.memories().int(MemoryKind::RamCooldownTicks).unwrap();
        assert!((least - 3..least).contains(&left), "{left}");
    }
}

#[test]
fn a_goat_s_jump_puts_it_in_its_pose_unslowed_and_its_ram_throws_a_horn_with_a_push() {
    use crate::mobs::brain::Cx;
    use crate::mobs::brain::behaviours::jumping::Leaper;
    use crate::mobs::brain::behaviours::ramming::Rammer;
    let mut goat = Thinking::new(GOAT, TypeState::default());
    let mut replay = goat.stream();
    let (out, _) = goat.step(|tick| {
        let mut memories = Memories::default();
        let config = Setup::default().config;
        let mut ai = Ai {
            tick,
            config: &config,
            sensing: Default::default(),
            navigation_ticked: true,
        };
        let mut cx = Cx {
            ai: &mut ai,
            memories: &mut memories,
            time: 0,
        };
        goat::LEAPER.set_long_jumping(&mut cx, true);
        goat::LEAPER.set_discard_friction(&mut cx, true);
        assert_eq!(goat::LEAPER.long_jumping_size(&cx), (0.9 * 0.7, 1.3 * 0.7));
        assert_eq!(
            goat::LEAPER.jump_sound(&cx.ai.tick.mob),
            sounds::ENTITY_GOAT_LONG_JUMP
        );
        assert!(goat::RAMMER.drop_horn(&mut cx));
    });
    let own = goat::state_of(&goat.mob);
    assert!(own.long_jumping && own.discards_friction);
    assert_eq!(
        of(GOAT).dimensions(&goat.mob).map(|size| size.height),
        Some(1.3 * 0.7)
    );
    // The horn: a coin for which, three floats for the push, thrown at its feet of no
    // pick-up delay.
    let left = replay.next_bool();
    let mut between = |min: f32, max: f32| f64::from(replay.next_float() * (max - min) + min);
    let motion = [between(-0.2, 0.2), between(0.3, 0.7), between(-0.2, 0.2)];
    assert_eq!(
        out.local,
        [Local::Throw {
            stack: ItemStack {
                item: items::GOAT_HORN,
                count: 1,
            },
            at: [0.5, 0.0, 0.5],
            pick_up_delay: 0,
            motion,
        }]
    );
    assert_eq!(own.left_horn, !left);
    // Its travel keeps its motion whole: neither the air's drag nor the block's friction.
    let (_, _) = goat.step(|tick| {
        tick.mob_type.before_travel(tick);
        assert_eq!(tick.living.attributes.air_drag_modifier, 0.0);
        assert_eq!(tick.living.attributes.friction_modifier, 0.0);
    });
    goat::fresh_load(&mut goat.mob);
    let own = goat::state_of(&goat.mob);
    assert!(!own.long_jumping && !own.discards_friction);
}

/// A brain of one behaviour in its core, `RandomSitting`.
static SITTING: Provider = Provider {
    memories: &[MemoryKind::IsPanicking],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(HeldTimed::new(camel::RandomSitting))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A camel standing since the game time `since`.
fn standing_since(since: i64) -> CamelState {
    CamelState {
        pose_tick: since,
        ..CamelState::default()
    }
}

/// A camel of the type `kind` thinking, its own state `own`.
fn thinking_camel(kind: u16, own: CamelState) -> Thinking {
    let mut camel = Thinking::new(
        kind,
        animal::write(&Animal::default(), &HorseState::default()),
    );
    camel::update(&mut camel.mob, |camel| *camel = own);
    camel
}

#[test]
fn a_camel_400_ticks_in_its_pose_sits_or_stands_at_random_sitting_s_start_of_one_draw() {
    // 400 ticks standing: it sits, of the start's `nextInt(1)` and its sound's two.
    let mut camel = thinking_camel(CAMEL, standing_since(10_000 - 400));
    let mut replay = camel.stream();
    let (out, drawn) = camel.alone(&SITTING, |_| {});
    assert!(camel::is_sitting(&camel.mob));
    assert_eq!(camel::camel_of(&camel.mob).pose_tick, -10_000);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_CAMEL_SIT]);
    assert_eq!(drawn, 3);
    replay.next_int(1);
    let pitch = (replay.next_float() - replay.next_float()) * 0.2 + 1.0;
    assert!(matches!(
        out.events.as_slice(),
        [Event::Sound { pitch: heard, .. }, Event::GameEvent(GameEvent::EntityAction)] if *heard == pitch
    ));
    // It starts and stops in that tick: nothing kept.
    assert!(
        camel
            .mob
            .mind
            .brain
            .iter()
            .all(|entry| entry.id < machinery::ids::FIRST_BEHAVIOUR)
    );
    // 399 ticks: nothing, and no draw.
    let mut camel = thinking_camel(CAMEL, standing_since(10_000 - 399));
    let (out, drawn) = camel.alone(&SITTING, |_| {});
    assert!(!camel::is_sitting(&camel.mob));
    assert!(out.events.is_empty());
    assert_eq!(drawn, 0);
    // Sitting 400: it stands up, with its sound.
    let mut camel = thinking_camel(CAMEL, standing_since(-(10_000 - 400)));
    let (out, _) = camel.alone(&SITTING, |_| {});
    assert!(!camel::is_sitting(&camel.mob));
    assert_eq!(camel::camel_of(&camel.mob).pose_tick, 10_000);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_CAMEL_STAND]);
    // Panicking, a standing one starts but sits not; in water neither starts.
    let mut camel = thinking_camel(CAMEL, standing_since(0));
    let (out, drawn) = camel.alone(&SITTING, |brain| {
        brain
            .memories
            .set(MemoryKind::IsPanicking, Value::Bool(true));
    });
    assert!(!camel::is_sitting(&camel.mob) && out.events.is_empty());
    assert_eq!(drawn, 1);
    let mut camel = thinking_camel(CAMEL, standing_since(0));
    camel.living.body.in_water = true;
    let (_, drawn) = camel.alone(&SITTING, |_| {});
    assert_eq!(drawn, 0);
    // The husk's sounds are its own.
    let mut husk = thinking_camel(HUSK, standing_since(0));
    let (out, _) = husk.alone(&SITTING, |_| {});
    assert_eq!(sounds_of(&out), [sounds::ENTITY_CAMEL_HUSK_SIT]);
}

/// A brain of one behaviour in its core: a stroll that only a camel free to move tries
/// (`triggerIf(not(refuseToMove), RandomStroll.stroll(2.0))`).
static STROLLING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![crate::mobs::brain::behaviours::trigger_if(
                |cx| camel::brain::may_stroll(cx.ai.tick),
                crate::mobs::brain::behaviours::walking::stroll(2.0),
            )],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

#[test]
fn a_sitting_or_rising_camel_tries_no_stroll_and_draws_nothing_for_one() {
    for (own, refuses) in [
        (standing_since(-5000), true),
        (standing_since(10_000 - 51), true),
        (standing_since(10_000 - 52), false),
    ] {
        let mut camel = thinking_camel(CAMEL, own);
        let (_, drawn) = camel.alone(&STROLLING, |_| {});
        assert_eq!(drawn == 0, refuses, "{own:?}");
        let walks = camel.memories().get(MemoryKind::WalkTarget).is_some();
        assert_eq!(walks, !refuses, "{own:?}");
    }
}

#[test]
fn a_hurt_camel_panics_standing_up_at_once_and_runs_at_four_times_its_speed() {
    let mut camel = thinking_camel(CAMEL, standing_since(-5000));
    // Hit by a player at its last tick: its hurt sensor sees it at its first scan.
    camel.mob.body.combat.last_hurt_by = Some((EntityId(7), 0));
    camel.mob.body.combat.last_hurt_by_player = Some((7, 0));
    camel.mob.body.last_damage.last = Some((
        DamageType::PLAYER_ATTACK.id().expect("a damage type"),
        Some(EntityId(7)),
        camel.world.game_time(),
    ));
    let out = camel.think();
    assert!(
        camel
            .memories()
            .check(MemoryKind::IsPanicking, Status::Present)
    );
    // `standUpInstantly`: no sound, standing as if 53 ticks ago.
    assert_eq!(camel::camel_of(&camel.mob).pose_tick, 10_000 - 53);
    assert!(sounds_of(&out).is_empty());
    assert!(
        out.events
            .contains(&Event::GameEvent(GameEvent::EntityAction))
    );
    let walk = camel.memories().get(MemoryKind::WalkTarget);
    assert!(
        matches!(walk, Some(Value::Walk { speed, .. }) if speed == 4.0),
        "{walk:?}"
    );
}

#[test]
fn a_camel_is_tempted_by_cactus_and_its_husk_by_rabbit_feet_alone() {
    for (kind, food, other) in [
        (CAMEL, items::CACTUS, items::RABBIT_FOOT),
        (HUSK, items::RABBIT_FOOT, items::CACTUS),
    ] {
        for (held, tempted) in [(food, true), (other, false)] {
            let mut camel = thinking_camel(kind, standing_since(0));
            camel
                .world
                .players
                .push(seen_player(7, [4.5, 0.0, 0.5], Some(held), false));
            camel.think();
            let brain = camel.brain();
            assert_eq!(
                brain.memories.get(MemoryKind::TemptingPlayer).is_some(),
                tempted,
                "{kind} {held}"
            );
        }
    }
}

/// An armadillo thinking, its scute far off.
fn thinking_armadillo() -> Thinking {
    let mut armadillo = Thinking::new(ARMADILLO, TypeState::default());
    armadillo::update(&mut armadillo.mob, |own| own.scute_at = 1_000_000);
    armadillo
}

/// The ticks of the first `ticks` at which the armadillo's scare sensor scans: from its
/// delay on, every five.
fn scare_scans(armadillo: &Thinking, ticks: u32) -> Vec<u32> {
    let state: BrainState = crate::mobs::part::decode(&armadillo.mob.mind.brain).unwrap();
    let first = state.next_scans[4];
    (0..ticks)
        .filter(|tick| *tick >= first && (tick - first).is_multiple_of(5))
        .collect()
}

#[test]
fn an_armadillo_remembers_danger_for_80_ticks_at_each_scan_that_sees_a_scare_within_7_2_7() {
    let danger = |armadillo: &Thinking| {
        armadillo
            .memories()
            .slots
            .get(&MemoryKind::DangerDetectedRecently)
            .cloned()
    };
    // A sprinting player 5 blocks off: set at the scans, each for 80 ticks.
    let mut armadillo = thinking_armadillo();
    armadillo
        .world
        .players
        .push(seen_player(7, [5.5, 0.0, 0.5], None, true));
    let scans = scare_scans(&armadillo, 30);
    let mut set_at = Vec::new();
    for _ in 0..30 {
        let before = danger(&armadillo);
        let now = armadillo.lived;
        armadillo.think();
        if danger(&armadillo) != before {
            set_at.push(now);
            assert_eq!(
                danger(&armadillo),
                Some(crate::mobs::brain::Slot::Kept {
                    value: Value::Bool(true),
                    expires: Some(now + 80),
                })
            );
        }
    }
    assert_eq!(set_at, scans);
    // A walking player, a sprinting spectator, a sprinting player 8 blocks off: none.
    let mut walking = seen_player(7, [5.5, 0.0, 0.5], None, false);
    let mut spectator = seen_player(7, [5.5, 0.0, 0.5], None, true);
    spectator.spectator = true;
    let far = seen_player(7, [8.6, 0.0, 0.5], None, true);
    for player in [walking.clone(), spectator, far] {
        let mut armadillo = thinking_armadillo();
        armadillo.world.players.push(player.clone());
        for _ in 0..30 {
            armadillo.think();
        }
        assert_eq!(danger(&armadillo), None, "{player:?}");
    }
    // A player riding, an undead mob, and what last hurt it: all scare it.
    walking.vehicle = Some(EntityId(99));
    let zombie = entity_of(
        EntityId(8),
        entity_types::ZOMBIE as u16,
        [4.5, 0.0, 0.5],
        &healthy(entity_types::ZOMBIE as u16, TypeState::default()),
    );
    let cow = entity_of(
        EntityId(8),
        entity_types::COW as u16,
        [4.5, 0.0, 0.5],
        &healthy(entity_types::COW as u16, TypeState::default()),
    );
    for case in 0..3 {
        let mut armadillo = thinking_armadillo();
        match case {
            0 => armadillo.world.players.push(walking.clone()),
            1 => armadillo.world.put(zombie.clone()),
            _ => {
                armadillo.world.put(cow.clone());
                armadillo.mob.body.combat.last_hurt_by = Some((EntityId(8), 0));
                armadillo.mob.body.last_damage.last = Some((
                    DamageType::MOB_ATTACK.id().expect("a damage type"),
                    Some(EntityId(8)),
                    armadillo.world.game_time(),
                ));
            }
        }
        for _ in 0..30 {
            armadillo.think();
        }
        assert!(danger(&armadillo).is_some(), "case {case}");
    }
    // In water it may not stay rolled up: the scan forgets the danger.
    let mut armadillo = thinking_armadillo();
    armadillo
        .world
        .players
        .push(seen_player(7, [5.5, 0.0, 0.5], None, true));
    for _ in 0..10 {
        armadillo.think();
    }
    assert!(danger(&armadillo).is_some());
    armadillo
        .world
        .blocks
        .push((BlockPos::new(0, 0, 0), blocks::WATER));
    armadillo.living.fluids =
        crate::physics::fluids::update(&armadillo.world, &armadillo.living.body.bounds());
    armadillo.living.body.in_water = armadillo.living.fluids.in_water();
    assert!(armadillo.living.body.in_water);
    for _ in 0..5 {
        armadillo.think();
    }
    assert_eq!(danger(&armadillo), None);
}

/// A brain of one behaviour of the armadillo's panic, `ArmadilloBallUp`, in its core,
/// and the memory it reads.
static BALLING: Provider = Provider {
    memories: &[MemoryKind::DangerDetectedRecently, MemoryKind::IsPanicking],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(HeldTimed::new(armadillo::BallUp))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

#[test]
fn balling_up_draws_its_start_its_roll_s_voice_and_a_peek_time_as_danger_comes_in_that_order() {
    let mut armadillo = thinking_armadillo();
    let mut replay = armadillo.stream();
    let (out, drawn) = armadillo.alone(&BALLING, |_| {});
    // `nextInt(1)` of its start, the roll's two of its voice, then its first tick: no
    // memory is "danger" (the longest time to expire), which was not about: a peek time.
    replay.next_int(1);
    replay.next_float();
    replay.next_float();
    replay.next_int_between(100, 400);
    assert_eq!(drawn, 4);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_ARMADILLO_ROLL]);
    assert_eq!(armadillo::state_of(&armadillo.mob).phase, Phase::Rolling);
    // Its next tick, rolling on with the danger as it was, draws nothing.
    let (_, drawn) = armadillo.alone(&BALLING, |_| {});
    assert_eq!(drawn, 0);
}

/// The game's `ArmadilloBallUp` as a plain loop over its ticks: its fields kept as the
/// game keeps them, its tick [`armadillo::brain::ball_up_tick`] of an int lowered every
/// tick it runs.
#[test]
fn balling_up_keeps_its_peek_count_as_the_game_s_and_writes_it_only_where_it_is_drawn() {
    // Danger seen every five ticks until tick 300, then no more.
    let ttl_at = |lived: u32, last: u32| -> i64 {
        if lived <= last {
            let seen = lived - lived % 5;
            i64::from(seen + 80) - i64::from(lived)
        } else {
            i64::from(last - last % 5 + 80) - i64::from(lived)
        }
    };
    let mut armadillo = thinking_armadillo();
    let mut port = thinking_armadillo();
    let mut ball = armadillo::brain::BallUp::default();
    let mut running = false;
    // Up to the danger's last tick (300's 80 later).
    for _ in 0..=380 {
        let lived = armadillo.lived;
        let ttl = ttl_at(lived, 300);
        let expiry = u32::try_from(i64::from(lived) + ttl).unwrap();
        let before: Vec<_> = armadillo
            .mob
            .mind
            .brain
            .iter()
            .filter(|entry| entry.id >= machinery::ids::FIRST_BEHAVIOUR)
            .cloned()
            .collect();
        let (out, _) = armadillo.alone(&BALLING, |brain| {
            if ttl >= 0 {
                brain.memories.slots.insert(
                    MemoryKind::DangerDetectedRecently,
                    crate::mobs::brain::Slot::Kept {
                        value: Value::Bool(true),
                        expires: Some(expiry),
                    },
                );
            } else {
                brain.memories.erase(MemoryKind::DangerDetectedRecently);
            }
        });
        // The port, of the same stream at the same places: whether its fields changed
        // but by the count's lowering.
        let read = if ttl >= 0 { ttl } else { i64::MAX };
        let mut moved = false;
        let (expected, _) = port.step(|tick| {
            if !running {
                if !armadillo::brain::ball_up_may_start(tick) {
                    return;
                }
                tick.draws.next_int(1);
                armadillo::brain::ball_up_start(tick);
                running = true;
                moved = true;
            } else if !armadillo::brain::ball_up_may_go_on(&tick.mob) {
                armadillo::brain::ball_up_stop(tick, false);
                running = false;
                moved = true;
                return;
            }
            let was = ball;
            armadillo::brain::ball_up_tick(tick, &mut ball, read);
            moved |= ball.danger_was_around != was.danger_was_around
                || ball.next_peek_timer != (was.next_peek_timer - 1).max(0);
        });
        assert_eq!(out.events, expected.events, "{lived}");
        assert_eq!(
            armadillo::state_of(&armadillo.mob),
            armadillo::state_of(&port.mob),
            "{lived}"
        );
        // Its part changes only where it starts, draws a peek time or stops.
        let after: Vec<_> = armadillo
            .mob
            .mind
            .brain
            .iter()
            .filter(|entry| entry.id >= machinery::ids::FIRST_BEHAVIOUR)
            .cloned()
            .collect();
        if before != after {
            assert!(moved, "{lived}: written as its count only went down");
        }
    }
    // Over by then: unrolling as its danger ran low, it never rolled out itself, which
    // its brain's rolling out does once the memory is gone.
    assert_eq!(armadillo::state_of(&armadillo.mob).phase, Phase::Unrolling);
}

#[test]
fn a_rolled_up_armadillo_unrolls_by_its_core_the_tick_its_danger_is_gone_and_stops_balling() {
    let mut armadillo = thinking_armadillo();
    armadillo::update(&mut armadillo.mob, |own| {
        own.switch_to(Phase::Unrolling, 0);
    });
    // Its danger's last tick is its next.
    armadillo.remember(|memories| {
        memories.set_with_expiry(MemoryKind::DangerDetectedRecently, Value::Bool(true), 1);
    });
    armadillo.think();
    assert_eq!(armadillo.brain().state.active, Some(Activity::Panic));
    let out = armadillo.think();
    assert_eq!(armadillo::state_of(&armadillo.mob).phase, Phase::Unrolling);
    assert!(!sounds_of(&out).contains(&sounds::ENTITY_ARMADILLO_UNROLL_FINISH));
    // Gone: the core's rolling out, with its sound once; idle again.
    let out = armadillo.think();
    assert_eq!(armadillo::state_of(&armadillo.mob).phase, Phase::Idle);
    assert_eq!(
        sounds_of(&out)
            .iter()
            .filter(|sound| **sound == sounds::ENTITY_ARMADILLO_UNROLL_FINISH)
            .count(),
        1
    );
    assert_eq!(armadillo.brain().state.active, Some(Activity::Idle));
    // Balling up stopped with it, its stop rolling nothing out again.
    let out = armadillo.think();
    assert!(!sounds_of(&out).contains(&sounds::ENTITY_ARMADILLO_UNROLL_FINISH));
}

#[test]
fn a_hurt_by_something_living_is_a_danger_an_armadillo_remembers_for_80_ticks() {
    let mut mob = armadillo_with(ArmadilloState::default(), &Animal::default());
    machinery::construct(of(ARMADILLO), &mut mob, &mut draws(1));
    hurt(ARMADILLO, &mut mob, &by_player(), 2.0, 30, 0, &mut draws(1));
    let memories = machinery::memories_of(&armadillo::PROVIDER, &mob, 30);
    // As if set before its forget pass at 30: there through 109, gone at 110.
    assert_eq!(
        memories.slots.get(&MemoryKind::DangerDetectedRecently),
        Some(&crate::mobs::brain::Slot::Kept {
            value: Value::Bool(true),
            expires: Some(29 + 80),
        })
    );
    // A panicking armadillo hurt rolls not up.
    let mut mob = armadillo_with(ArmadilloState::default(), &Animal::default());
    let mut memories = machinery::memories_of(&armadillo::PROVIDER, &mob, 30);
    memories.set(MemoryKind::IsPanicking, Value::Bool(true));
    machinery::write_memories(&armadillo::PROVIDER, &mut mob, memories);
    hurt(ARMADILLO, &mut mob, &by_player(), 2.0, 30, 0, &mut draws(1));
    assert_eq!(armadillo::state_of(&mob).phase, Phase::Idle);
}

#[test]
fn a_tick_of_each_highland_brain_is_a_function_of_its_inputs() {
    for kind in [GOAT, CAMEL, HUSK, ARMADILLO] {
        let run = || {
            let mut mob = Thinking::new(kind, TypeState::default());
            if kind == CAMEL || kind == HUSK {
                mob = thinking_camel(kind, standing_since(0));
            }
            mob.world
                .players
                .push(seen_player(7, [4.5, 0.0, 2.5], Some(items::WHEAT), true));
            let mut seen = Vec::new();
            for _ in 0..300 {
                let out = mob.think();
                seen.push((out, mob.mob.encode(), mob.sensed.clone()));
            }
            seen
        };
        assert_eq!(run(), run(), "{kind}");
    }
}
