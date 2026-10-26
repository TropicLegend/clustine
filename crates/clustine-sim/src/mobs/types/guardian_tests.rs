//! The guardians from the game's rules (`n7k-brief.md` 3.8 and 5; `Guardian`,
//! `GuardianMoveControl`, `ElderGuardian`), not from the code: the constructor's draw,
//! the flop on land by its three draws, the air kept full in water, the spikes, the
//! sounds of water and land, the elder's persistence, home and curse.

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, sounds};
use clustine_world::{BlockPos, ChunkPos, EntityId};

use super::guardians::{self, GuardianState};
use super::{SpawnReason, Spawning, elder_guardian, of};
use crate::living::{Kind, Living, Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType, Struck};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::World;
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::region::Random;
use crate::shown::DataShown;

const GUARDIAN: u16 = entity_types::GUARDIAN as u16;
const ELDER: u16 = entity_types::ELDER_GUARDIAN as u16;
const COW: u16 = entity_types::COW as u16;
const KEY: EntityId = EntityId(40);

/// A floor of stone, with water over it where `water`, and the players `players`.
struct Shore {
    water: bool,
    players: Vec<SeenPlayer>,
}

impl World for Shore {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(match pos.y {
            ..0 => blocks::STONE,
            0..3 if self.water => blocks::WATER,
            _ => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Shore {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Shore {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, _: &Aabb) -> Vec<Seen<'_>> {
        Vec::new()
    }

    fn entity(&self, _: EntityId) -> Option<Seen<'_>> {
        None
    }

    fn players(&self) -> Vec<SeenPlayer> {
        self.players.clone()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        Some(LightAt { sky: 15, block: 0 })
    }

    fn day(&self) -> u64 {
        18_000
    }
}

/// A guardian's body at rest on the floor, in water where `in_water`.
fn living(kind: u16, in_water: bool) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
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
        in_water,
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
    living.air = 120;
    living
}

/// `f` on a tick of a guardian of `kind` in `world`, in water where `in_water`, at the
/// `tickCount` `tick_count`.
fn with_tick<R>(
    kind: u16,
    world: &Shore,
    in_water: bool,
    tick_count: u32,
    mob: Mob,
    stream: &mut Random,
    f: impl FnOnce(&mut MobTick<'_>) -> R,
) -> R {
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: KEY,
        kind,
        lived: tick_count,
        tick_count,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: true,
        mob,
        living: living(kind, in_water),
        yaw: 0.0,
        pitch: 0.0,
        mob_type: of(kind),
        world,
        draws: stream,
        out: MobOutput::default(),
    };
    f(&mut tick)
}

/// A guardian of full health.
fn alive() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 30.0;
    mob
}

#[test]
fn a_guardian_s_constructor_draws_its_tail_after_the_mob_s_draws() {
    for kind in [GUARDIAN, ELDER] {
        let mut own = Random::of_entity(3, KEY, 0);
        let mut replay = own.clone();
        let (_, mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
        replay.next_u64();
        replay.next_u64();
        replay.next_float();
        replay.next_float();
        assert_eq!(own, replay, "the UUID, the yaw, then the tail");
        assert_eq!(
            mob.body.flags.persistent,
            kind == ELDER,
            "the elder is kept"
        );
    }
}

#[test]
fn a_guardian_s_egg_draws_only_the_mob_s_tail() {
    let mut level = Random::of_entity(5, KEY, 0);
    let mut replay = level.clone();
    let mut own = Random::of_entity(7, KEY, 0);
    let (_, mut mob) = crate::mobs::making::construct(GUARDIAN, &mut own, 0.0);
    crate::mobs::making::finalize(
        GUARDIAN,
        &mut Spawning {
            mob: &mut mob,
            level: &mut level,
            own: &mut own,
            biome: None,
            at: BlockPos::new(0, 64, 0),
            day: 0,
            game_time: 0,
            reason: SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
        },
    );
    replay.next_double();
    replay.next_double();
    replay.next_float();
    assert_eq!(level, replay);
}

#[test]
fn a_guardian_on_land_flops_by_three_draws_of_its_own() {
    let world = Shore {
        water: false,
        players: Vec::new(),
    };
    let mut stream = Random::of_entity(11, KEY, 0);
    let mut replay = stream.clone();
    let (motion, yaw, on_ground, needs_sync, air) =
        with_tick(GUARDIAN, &world, false, 100, alive(), &mut stream, |tick| {
            of(GUARDIAN).before_ai_step(tick);
            (
                tick.living.body.motion,
                tick.yaw,
                tick.living.body.on_ground,
                tick.out.needs_sync,
                tick.living.air,
            )
        });
    let x = f64::from((replay.next_float() * 2.0 - 1.0) * 0.4);
    let z = f64::from((replay.next_float() * 2.0 - 1.0) * 0.4);
    let turned = replay.next_float() * 360.0;
    assert_eq!(stream, replay, "three draws");
    assert_eq!(motion, [x, 0.5, z]);
    assert_eq!(yaw, turned);
    assert!(!on_ground && needs_sync);
    assert_eq!(air, 120, "no air is given or taken on land");
}

#[test]
fn a_guardian_in_water_breathes_its_fill_and_does_not_flop() {
    let world = Shore {
        water: true,
        players: Vec::new(),
    };
    let mut stream = Random::of_entity(11, KEY, 0);
    let before = stream.clone();
    let (motion, air, mob) = with_tick(GUARDIAN, &world, true, 100, alive(), &mut stream, |tick| {
        of(GUARDIAN).before_ai_step(tick);
        (tick.living.body.motion, tick.living.air, tick.mob.clone())
    });
    assert_eq!(stream, before, "no draw");
    assert_eq!(motion, [0.0; 3]);
    assert_eq!(air, 300);
    assert!(guardians::state_of(&mob).in_water);
}

#[test]
fn a_still_guardian_sinks_in_water_and_a_moving_one_does_not() {
    let world = Shore {
        water: true,
        players: Vec::new(),
    };
    let mut stream = Random::of_entity(1, KEY, 0);
    let still = with_tick(
        GUARDIAN,
        &world,
        true,
        1,
        Mob::fresh(),
        &mut stream,
        |tick| of(GUARDIAN).travel(tick),
    );
    assert_eq!(
        still,
        Travel::Swims {
            speed: 0.1,
            sinks: Sinks::Always
        }
    );
    let mut moving = Mob::fresh();
    guardians::update(&mut moving, |own| own.moving = true);
    let going = with_tick(GUARDIAN, &world, true, 1, moving, &mut stream, |tick| {
        of(GUARDIAN).travel(tick)
    });
    assert_eq!(
        going,
        Travel::Swims {
            speed: 0.1,
            sinks: Sinks::Never
        }
    );
}

/// `f` on a guardian of `own` struck between ticks.
fn struck<R>(own: &GuardianState, f: impl FnOnce(&mut Struck<'_>) -> R) -> (R, Mob, MobOutput) {
    let mut mob = Mob::fresh();
    guardians::update(&mut mob, |state| *state = own.clone());
    let mut motion = [0.0; 3];
    let mut draws = Random::of_entity(1, KEY, 0);
    let mut out = MobOutput::default();
    let result = {
        let mut struck = Struck {
            kind: GUARDIAN,
            mob: &mut mob,
            mob_type: of(GUARDIAN),
            position: [1.5, 0.0, 1.5],
            motion: &mut motion,
            on_ground: false,
            on_fire: false,
            vehicle: None,
            carries: false,
            leashed: false,
            in_liquid: true,
            moment: 10,
            game_time: 10,
            draws: &mut draws,
            out: &mut out,
        };
        f(&mut struck)
    };
    (result, mob, out)
}

fn by_a_cow(kind: DamageType) -> DamageSource {
    DamageSource::by(
        kind,
        Causer {
            key: EntityId(77),
            kind: COW,
            position: [3.5, 0.0, 1.5],
            player: None,
            living: true,
        },
    )
}

fn spikes(out: &MobOutput) -> Vec<Local> {
    out.local
        .iter()
        .filter(|local| matches!(local, Local::Thorns { .. }))
        .cloned()
        .collect()
}

#[test]
fn a_still_guardian_s_spikes_hurt_a_living_attacker_by_two_of_thorns() {
    let (_, mob, out) = struck(&GuardianState::default(), |struck| {
        of(GUARDIAN).before_hurt_server(struck, &by_a_cow(DamageType::MOB_ATTACK), 3.0);
    });
    assert_eq!(
        spikes(&out),
        [Local::Thorns {
            key: EntityId(77),
            kind: GUARDIAN,
            from: [1.5, 0.0, 1.5],
            amount: 2.0,
        }]
    );
    assert!(
        guardians::state_of(&mob).stroll_triggered,
        "its stroll is triggered"
    );
}

#[test]
fn no_spikes_while_it_moves_nor_against_thorns_magic_a_player_or_what_is_not_alive() {
    let moving = GuardianState {
        moving: true,
        ..GuardianState::default()
    };
    let cases: Vec<(GuardianState, DamageSource)> = vec![
        (moving, by_a_cow(DamageType::MOB_ATTACK)),
        (GuardianState::default(), by_a_cow(DamageType::THORNS)),
        (GuardianState::default(), by_a_cow(DamageType::MAGIC)),
        (
            GuardianState::default(),
            DamageSource::by(
                DamageType::PLAYER_ATTACK,
                Causer {
                    key: EntityId(5),
                    kind: entity_types::PLAYER as u16,
                    position: [3.5, 0.0, 1.5],
                    player: Some(9),
                    living: true,
                },
            ),
        ),
        (GuardianState::default(), DamageSource::of(DamageType::FALL)),
    ];
    for (own, source) in cases {
        let (_, mob, out) = struck(&own, |struck| {
            of(GUARDIAN).before_hurt_server(struck, &source, 3.0);
        });
        assert!(spikes(&out).is_empty(), "{source:?}");
        assert!(guardians::state_of(&mob).stroll_triggered);
    }
}

#[test]
fn a_guardian_sounds_of_water_or_land_by_where_it_last_was() {
    let mut mob = Mob::fresh();
    let guardian = of(GUARDIAN);
    let fall = DamageSource::of(DamageType::FALL);
    assert_eq!(
        guardian.hurt_sound(&mob, &fall),
        Some(sounds::ENTITY_GUARDIAN_HURT_LAND)
    );
    assert_eq!(
        guardian.ambient_sound(&mob),
        Some(sounds::ENTITY_GUARDIAN_AMBIENT_LAND)
    );
    guardians::update(&mut mob, |own| own.in_water = true);
    assert_eq!(
        guardian.hurt_sound(&mob, &fall),
        Some(sounds::ENTITY_GUARDIAN_HURT)
    );
    assert_eq!(
        guardian.death_sound(&mob),
        Some(sounds::ENTITY_GUARDIAN_DEATH)
    );
    assert_eq!(
        of(ELDER).death_sound(&mob),
        Some(sounds::ENTITY_ELDER_GUARDIAN_DEATH)
    );
    assert_eq!(
        of(ELDER).ambient_sound_at(&mob, false, true),
        Some(sounds::ENTITY_ELDER_GUARDIAN_AMBIENT_LAND)
    );
    assert_eq!(guardian.ambient_interval(), 160);
}

#[test]
fn a_guardian_shows_whether_it_moves_and_no_beam() {
    let mut mob = Mob::fresh();
    assert_eq!(
        of(GUARDIAN).shown(&mob),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Int(0), true),
        ]
    );
    guardians::update(&mut mob, |own| own.moving = true);
    assert_eq!(
        of(ELDER).shown(&mob)[0],
        (16, DataShown::Boolean(true), false)
    );
}

fn player(key: u64, at: [f64; 3], creative: bool) -> SeenPlayer {
    SeenPlayer {
        key: EntityId(key),
        uuid: u128::from(key),
        position: at,
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

#[test]
fn the_elder_curses_no_creative_player_and_would_find_a_survival_one_within_50() {
    let world = Shore {
        water: true,
        players: vec![
            player(1, [10.5, 0.0, 0.5], true),
            player(2, [40.5, 0.0, 0.5], false),
            player(3, [60.5, 0.0, 0.5], false),
        ],
    };
    let mut stream = Random::of_entity(1, KEY, 0);
    let found = with_tick(ELDER, &world, true, 0, Mob::fresh(), &mut stream, |tick| {
        elder_guardian::cursed_players(tick)
    });
    assert_eq!(found, [EntityId(2)], "only the one in survival within 50");
    let creative_only = Shore {
        water: true,
        players: vec![player(1, [10.5, 0.0, 0.5], true)],
    };
    let found = with_tick(
        ELDER,
        &creative_only,
        true,
        0,
        Mob::fresh(),
        &mut stream,
        |tick| elder_guardian::cursed_players(tick),
    );
    assert!(found.is_empty());
}

#[test]
fn the_elder_curses_every_1200_ticks_by_its_tick_count_and_its_id_and_keeps_a_home_of_16() {
    let world = Shore {
        water: true,
        players: Vec::new(),
    };
    let mut stream = Random::of_entity(1, KEY, 0);
    // Its id is 40: the curse comes at tick counts 1160, 2360, …
    for (count, curses) in [(1160, true), (1161, false), (2360, true), (0, false)] {
        let now = with_tick(
            ELDER,
            &world,
            true,
            count,
            Mob::fresh(),
            &mut stream,
            |tick| elder_guardian::curses_now(tick),
        );
        assert_eq!(now, curses, "{count}");
    }
    let home = with_tick(ELDER, &world, true, 5, Mob::fresh(), &mut stream, |tick| {
        of(ELDER).custom_server_ai_step(tick);
        tick.mob.body.home.clone()
    });
    assert_eq!(home.centre, Some(BlockPos::new(0, 0, 0)));
    assert_eq!(home.radius, 16);
    // The guardian has none.
    let home = with_tick(
        GUARDIAN,
        &world,
        true,
        5,
        Mob::fresh(),
        &mut stream,
        |tick| {
            of(GUARDIAN).custom_server_ai_step(tick);
            tick.mob.body.home.clone()
        },
    );
    assert_eq!(home.centre, None);
}
