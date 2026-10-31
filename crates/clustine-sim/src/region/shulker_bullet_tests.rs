//! The shulker and its bullet in a region, from the game's rules (`Shulker` and its
//! goals, `ShulkerBullet`, d javap; `docs/groundwork/p-plan.md`, sections 19.2 and 19.5),
//! not from the code: a shulker shooting at the mob it targets, the bullet's flight to it,
//! its hurt and the levitation; the bullet's turns worked out by hand; a bullet hurt by a
//! player's hit and gone; a shulker's peek and its armour; its teleport; the duplication of
//! an open shulker a bullet hits, and none of a closed one; a bullet crossing a line.

use clustine_data::{blocks, entity_types, sounds};
use clustine_world::{
    Arrival, BlockPos, Double, EdgeId, EffectId, Entity, EntityChange, EntityData, EntityId,
    ShulkerBullet, projectiles::ProjectileOwner,
};

use super::{Aimed, Course, rotate_towards_movement, select_next_move_direction};
use crate::api::{PlayerInput, RegionEvent, TickInputs, TickOutput};
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::effects::Effect;
use crate::mobs::types::shulker::{
    COVERED, SHULKER, ShulkerState, TELEPORT_EVENT, change, pack_difference, state_of,
};
use crate::region::Random;
use crate::region::Region;
use crate::region::tnt::tests::{FLOOR, add_player, idle, nothing, records, west_of_a_line, world};

const SEED: u64 = 0x05B0_11E7;

const COW: EntityId = EntityId(30);
const SHOOTER: EntityId = EntityId(40);

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// A mob of the type `kind` at `feet`, of `health`, whose type state is `mob`'s.
fn mob_at(key: EntityId, kind: u16, feet: [f64; 3], health: f32, mut mob: Mob) -> Entity {
    mob.body.vitals.health = health;
    mob.body.vitals.air = crate::living::MAX_AIR;
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 11),
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
    }
}

/// A cow with no mind of its own, of full health.
fn still_cow(feet: [f64; 3]) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.flags.no_ai = true;
    mob_at(COW, kind(entity_types::COW), feet, 10.0, mob)
}

/// A shulker of 30 health at `feet` whose state `own` gives and whose target is
/// `target`.
fn shulker(key: EntityId, feet: [f64; 3], own: ShulkerState, target: Option<EntityId>) -> Entity {
    let mut mob = Mob::fresh();
    change(&mut mob, |state| *state = own);
    mob.body.combat.target = target;
    mob_at(key, SHULKER, feet, 30.0, mob)
}

/// A bullet at `at` moving by `motion`, flying at `target`, owned by `owner`.
fn bullet(
    key: EntityId,
    at: [f64; 3],
    motion: [f64; 3],
    target: Option<EntityId>,
    owner: Option<EntityId>,
) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::SHULKER_BULLET),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 7),
        position: at,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 3,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::ShulkerBullet(Box::new(ShulkerBullet {
            owner: owner.map(|key| ProjectileOwner {
                key,
                kind: SHULKER,
                player: None,
            }),
            target,
            steps: 40,
            direction: None,
            target_delta: motion.map(Double),
        })),
    }
}

/// Lays `entity` in the region's loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// Lays `entity` in the region's copy of its chunk.
fn lay_in_copy(region: &mut Region, entity: &Entity) {
    region
        .copies
        .get_mut(&entity.chunk())
        .expect("the chunk is copied")
        .apply_entity(&EntityChange::Whole(entity.clone()));
}

fn bullets(region: &Region) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| matches!(entity.data, EntityData::ShulkerBullet(_)))
        .collect()
}

fn shulkers(region: &Region) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| entity.kind == SHULKER)
        .collect()
}

fn data_of(entity: &Entity) -> ShulkerBullet {
    let EntityData::ShulkerBullet(bullet) = &entity.data else {
        panic!("a bullet: {entity:?}");
    };
    (**bullet).clone()
}

fn mob(region: &Region, key: EntityId) -> Mob {
    let (_, record) = region.find_entity(key).expect("the mob is here");
    Mob::decode(&record.data).expect("a mob's record")
}

fn gone(output: &TickOutput, key: EntityId) -> bool {
    output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key))
}

/// The sounds of `sound` a tick played, with their volumes.
fn heard(output: &TickOutput, sound: i32) -> Vec<(clustine_world::Vec3, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: heard,
                position,
                volume,
                ..
            } if *heard == sound => Some((*position, *volume)),
            _ => None,
        })
        .collect()
}

/// The bullets a tick made, as first committed.
fn made_bullets(output: &TickOutput) -> Vec<Entity> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Whole(entity) if matches!(entity.data, EntityData::ShulkerBullet(_)) => {
                Some(entity.clone())
            }
            _ => None,
        })
        .collect()
}

/// Stone walls along every x a multiple of three, from the floor up, so that every
/// block of air between them has a wall beside it to cling to with a block of air
/// beyond it to open into.
fn walls(pos: BlockPos) -> Option<clustine_data::BlockState> {
    (pos.x.rem_euclid(3) == 0 && pos.y > FLOOR).then_some(blocks::STONE)
}

/// The middle of a block's bottom.
fn bottom_of(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

#[test]
fn a_shulker_opens_and_shoots_a_bullet_at_the_mob_it_targets_twenty_ticks_later() {
    let mut region = world(SEED, nothing);
    lay(&mut region, &still_cow([0.5, -60.0, 8.5]));
    lay(
        &mut region,
        &shulker(
            SHOOTER,
            [0.5, -60.0, 0.5],
            ShulkerState::default(),
            Some(COW),
        ),
    );
    // The attack goal starts: a peek of 100, with the open sound.
    let mut started = None;
    for tick in 0..10 {
        let output = idle(&mut region);
        if state_of(&mob(&region, SHOOTER)).peek == 100 {
            assert_eq!(heard(&output, sounds::ENTITY_SHULKER_OPEN).len(), 1);
            assert!(made_bullets(&output).is_empty());
            started = Some(tick);
            break;
        }
    }
    let started = started.expect("the attack starts within ten ticks");
    // Its timer of 20, counted down from the tick it started: the bullet comes in the
    // nineteenth tick after.
    let mut shot = None;
    for tick in started + 1..started + 30 {
        let output = idle(&mut region);
        let made = made_bullets(&output);
        if !made.is_empty() {
            let shoot = heard(&output, sounds::ENTITY_SHULKER_SHOOT);
            assert_eq!(shoot.len(), 1);
            assert_eq!(shoot[0].1, 2.0);
            shot = Some((tick, made));
            break;
        }
    }
    let (tick, made) = shot.expect("a bullet within thirty ticks");
    assert_eq!(tick, started + 19);
    let [made] = &made[..] else {
        panic!("one bullet: {made:?}");
    };
    let data = data_of(made);
    assert_eq!(data.owner.map(|owner| owner.key), Some(SHOOTER));
    assert_eq!(data.owner.map(|owner| owner.kind), Some(SHULKER));
    assert_eq!(data.target, Some(COW));
    assert!((10..=50).contains(&data.steps), "{}", data.steps);
    // At the middle of its box, the lid nineteen steps of 0.05 open: up by half of one
    // and the physical peek of 0.95.
    let peek = 0.5 - ((0.5 + 0.95) * std::f64::consts::PI).sin() * 0.5;
    assert_eq!(made.position[0], 0.5);
    assert_eq!(made.position[2], 0.5);
    assert!(
        (made.position[1] - (-60.0 + (1.0 + peek) / 2.0)).abs() < 1e-3,
        "{:?}",
        made.position
    );
    assert_eq!(made.motion, [0.0; 3]);
    // Facing down, its first turn is not up or down: along z, towards the cow.
    assert_eq!(data.direction, Some(3));
}

#[test]
fn a_bullet_flies_to_the_mob_it_targets_hurts_it_by_four_and_makes_it_levitate() {
    let mut region = world(SEED, nothing);
    lay(&mut region, &still_cow([0.5, -60.0, 8.5]));
    lay(
        &mut region,
        &shulker(
            SHOOTER,
            [0.5, -60.0, 0.5],
            ShulkerState::default(),
            Some(COW),
        ),
    );
    let mut hurt = None;
    for tick in 0..200 {
        idle(&mut region);
        let cow = mob(&region, COW);
        if cow.body.vitals.health < 10.0 {
            hurt = Some((tick, cow));
            break;
        }
    }
    let (_, cow) = hurt.expect("the cow is hurt within 200 ticks");
    assert_eq!(cow.body.vitals.health, 6.0);
    let levitation = cow
        .body
        .effects
        .active
        .iter()
        .find(|active| active.effect == Effect::LEVITATION)
        .expect("the cow levitates");
    assert_eq!(levitation.top().amplifier, 0);
    // The bullet that hit is gone.
    assert!(
        bullets(&region)
            .iter()
            .all(|bullet| data_of(bullet).target == Some(COW))
    );
    // A shulker never targets a creative player, and its bullet never levitates one: a
    // player beside the cow takes nothing (their effects are their own).
}

#[test]
fn a_bullet_far_from_its_target_turns_along_the_one_axis_that_brings_it_nearer() {
    // At (0.5, 0.5, 0.5), its target's middle at x = 6.5, all air: x is the only axis
    // with a step nearer (y and z are level), y being the axis it came along.
    let target = Aimed {
        position: [6.5, -0.4, 0.5],
        height: 1.8,
        alive: true,
    };
    let mut own = Random::of_entity(SEED, EntityId(1), 42);
    let mut expected = own.clone();
    let course =
        select_next_move_direction([0.5, 0.5, 0.5], Some(1), Some(&target), &|_| true, &mut own);
    // `getRandom` first, then the one of the list, then the steps.
    let _first = expected.next_int(6);
    assert_eq!(expected.next_int(1), 0);
    let steps = 10 + expected.next_int(5) as i32 * 10;
    assert_eq!(
        course,
        Course {
            direction: Some(5),
            target_delta: [0.15, 0.0, 0.0],
            steps,
        }
    );
    assert_eq!(own, expected);
}

#[test]
fn a_bullet_near_its_target_aims_at_its_middle_and_draws_only_its_steps() {
    // The target's middle block is (1, 0, 0), whose middle is within 2 of the bullet.
    let target = Aimed {
        position: [1.5, -0.5, 0.5],
        height: 1.0,
        alive: true,
    };
    let mut own = Random::of_entity(SEED, EntityId(1), 7);
    let mut expected = own.clone();
    let course =
        select_next_move_direction([0.5, 0.5, 0.5], None, Some(&target), &|_| true, &mut own);
    // It wants (1.5, 0.5, 0.5), the block's middle across and half the height up.
    assert_eq!(course.direction, None);
    assert_eq!(course.target_delta, [0.15, 0.0, 0.0]);
    let _steps = expected.next_int(5);
    assert_eq!(own, expected);
}

#[test]
fn a_bullet_with_no_way_nearer_draws_a_direction_until_one_is_air_five_times_at_most() {
    // Every block about it is solid but the one above: no step is nearer, so it draws
    // directions until it finds up, or gives up after five more draws.
    let target = Aimed {
        position: [8.5, 0.0, 0.5],
        height: 1.0,
        alive: true,
    };
    let empty = |pos: BlockPos| pos == BlockPos::new(0, 1, 0);
    for seed in 0..40 {
        let mut own = Random::of_entity(SEED, EntityId(1), seed);
        let mut expected = own.clone();
        let course =
            select_next_move_direction([0.5, 0.5, 0.5], None, Some(&target), &empty, &mut own);
        let mut chosen = expected.next_int(6);
        let mut tries = 5;
        while chosen != 1 && tries > 0 {
            chosen = expected.next_int(6);
            tries -= 1;
        }
        let steps = 10 + expected.next_int(5) as i32 * 10;
        assert_eq!(course.direction, Some(chosen as u8));
        assert_eq!(course.steps, steps);
        assert_eq!(own, expected);
    }
}

#[test]
fn a_bullet_without_a_target_falls_and_turns_half_way_towards_its_motion() {
    let mut region = world(SEED, nothing);
    let key = EntityId(50);
    lay(
        &mut region,
        &bullet(key, [0.5, -50.0, 0.5], [0.1, 0.0, 0.0], None, None),
    );
    idle(&mut region);
    let [now] = &bullets(&region)[..] else {
        panic!("one bullet");
    };
    let motion = [0.1, -0.04, 0.0];
    assert_eq!(now.motion, motion);
    assert_eq!(now.position, [0.6, -50.04, 0.5]);
    assert_eq!(
        [now.yaw, now.pitch],
        rotate_towards_movement(0.0, 0.0, motion)
    );
}

#[test]
fn a_player_s_hit_destroys_a_bullet_with_its_hurt_sound_and_fifteen_crits() {
    let mut region = world(SEED, nothing);
    let (id, player) = add_player(&mut region, 1, [0.5, -60.0, 0.5], false);
    let key = EntityId(51);
    lay(
        &mut region,
        &bullet(key, [0.5, -58.5, 2.0], [0.0; 3], None, None),
    );
    let attack = TickInputs {
        inputs: vec![(
            EdgeId(1),
            id,
            player,
            1,
            PlayerInput::Attack {
                part: None,
                entity: key,
            },
        )],
        ..TickInputs::default()
    };
    let output = region.tick(&attack);
    assert!(gone(&output, key));
    assert!(bullets(&region).is_empty());
    assert_eq!(heard(&output, sounds::ENTITY_SHULKER_BULLET_HURT).len(), 1);
    let crits: Vec<i32> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Particles {
                particle: 13,
                count,
                ..
            } => Some(*count),
            _ => None,
        })
        .collect();
    assert_eq!(crits, [15]);
}

#[test]
fn a_bullet_that_meets_a_block_is_gone_with_two_explosions_and_its_hit_sound() {
    let mut region = world(SEED, |pos| {
        (pos == BlockPos::new(0, -55, 2)).then_some(blocks::STONE)
    });
    let key = EntityId(52);
    lay(
        &mut region,
        &bullet(key, [0.5, -54.5, 1.8], [0.0, 0.0, 0.3], None, None),
    );
    let output = idle(&mut region);
    assert!(gone(&output, key));
    assert_eq!(heard(&output, sounds::ENTITY_SHULKER_BULLET_HIT).len(), 1);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Particles {
            particle: 30,
            count: 2,
            ..
        }
    )));
}

#[test]
fn a_shulker_whose_target_is_out_of_reach_closes_with_twenty_armour() {
    let mut region = world(SEED, nothing);
    // 21 blocks off: the attack starts, and on its first tick lets the target go.
    lay(&mut region, &still_cow([0.5, -60.0, 21.5]));
    lay(
        &mut region,
        &shulker(
            SHOOTER,
            [0.5, -60.0, 0.5],
            ShulkerState::default(),
            Some(COW),
        ),
    );
    let armour = |region: &Region| {
        mob(region, SHOOTER)
            .body
            .attributes
            .value(SHULKER, attribute(clustine_data::attributes::ARMOR))
    };
    // A shulker made closed wears no armour of its being closed until a goal sets its
    // peek (`defineSynchedData` sets the peek, not `setRawPeekAmount`).
    assert_eq!(armour(&region), 0.0);
    let mut opened = false;
    let mut closed = false;
    for _ in 0..20 {
        let output = idle(&mut region);
        let own = state_of(&mob(&region, SHOOTER));
        if own.peek == 100 {
            opened = true;
            assert_eq!(armour(&region), 0.0);
        }
        if opened && own.peek == 0 {
            assert_eq!(heard(&output, sounds::ENTITY_SHULKER_CLOSE).len(), 1);
            closed = true;
            break;
        }
    }
    assert!(opened && closed);
    assert_eq!(mob(&region, SHOOTER).body.combat.target, None);
    assert_eq!(armour(&region), 20.0);
    let mob = mob(&region, SHOOTER);
    let instance = mob
        .body
        .attributes
        .made(attribute(clustine_data::attributes::ARMOR))
        .expect("an armour instance");
    assert!(instance.has_modifier(COVERED));
}

#[test]
fn a_shulker_asked_to_teleport_goes_within_eight_to_a_face_it_may_cling_to() {
    let mut region = world(SEED, walls);
    // Between the walls of x = 0 and x = 3, on the floor.
    let from = BlockPos::new(1, -60, 4);
    let own = ShulkerState {
        pending: Some(crate::mobs::types::shulker::Pending::Teleport),
        peek: 30,
        ..ShulkerState::default()
    };
    lay(
        &mut region,
        &shulker(SHOOTER, bottom_of(from), own, Some(COW)),
    );
    let output = idle(&mut region);
    let (_, record) = region.find_entity(SHOOTER).expect("the shulker is here");
    let to = crate::physics::containing(record.position);
    assert_ne!(to, from);
    assert_eq!(record.position, bottom_of(to));
    for (a, b) in [(to.x, from.x), (to.y, from.y), (to.z, from.z)] {
        assert!((a - b).abs() <= 8, "{to:?} from {from:?}");
    }
    let mob = Mob::decode(&record.data).expect("a mob");
    let own = state_of(&mob);
    // It clings to a face of stone, its peek 0 and its target dropped.
    let behind = crate::mobs::types::shulker::relative(to, own.attach_face);
    assert_eq!(region.block(behind), Some(blocks::STONE));
    assert_eq!(own.peek, 0);
    assert_eq!(own.pending, None);
    assert_eq!(mob.body.combat.target, None);
    let events: Vec<(BlockPos, i32)> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } if *kind == TELEPORT_EVENT => Some((*position, *data)),
            _ => None,
        })
        .collect();
    assert_eq!(events, [(from, pack_difference(from, to))]);
    let teleported = heard(&output, sounds::ENTITY_SHULKER_TELEPORT);
    assert_eq!(
        teleported.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
        [clustine_world::Vec3::new(1.5, -60.0, 4.5)]
    );
}

#[test]
fn the_difference_of_a_teleport_is_packed_eight_up_a_byte_each_x_highest() {
    let from = BlockPos::new(10, 64, -3);
    assert_eq!(pack_difference(from, from), 0x08_08_08);
    assert_eq!(
        pack_difference(from, BlockPos::new(2, 72, 2)),
        (16 << 8) | 13
    );
}

/// An open shulker a bullet hits teleports and, alone within its box grown by 8, makes
/// another of its colour where it was (`(1 - 1) / 5` is no chance of not).
#[test]
fn an_open_shulker_a_bullet_hits_teleports_and_leaves_another_of_its_colour_behind() {
    let mut region = world(SEED, walls);
    let from = BlockPos::new(1, -60, 4);
    let own = ShulkerState {
        peek: 100,
        opening: 1.0,
        colour: 5,
        ..ShulkerState::default()
    };
    let hit = EntityId(20);
    lay(&mut region, &shulker(hit, bottom_of(from), own, None));
    let key = EntityId(53);
    lay(
        &mut region,
        &bullet(key, [1.5, -59.0, 2.6], [0.0, 0.0, 0.4], Some(hit), None),
    );
    let mut outputs = Vec::new();
    for _ in 0..8 {
        outputs.push(idle(&mut region));
    }
    assert!(outputs.iter().any(|output| gone(output, key)));
    assert_eq!(mob(&region, hit).body.vitals.health, 26.0);
    let all = shulkers(&region);
    assert_eq!(all.len(), 2, "{all:?}");
    let (_, moved) = region.find_entity(hit).expect("the shulker is here");
    assert_ne!(crate::physics::containing(moved.position), from);
    let made = all
        .iter()
        .find(|shulker| shulker.key != hit)
        .expect("a new shulker");
    assert_eq!(made.position, bottom_of(from));
    let made = Mob::decode(&made.data).expect("a mob");
    assert_eq!(state_of(&made).colour, 5);
}

#[test]
fn a_closed_shulker_a_bullet_hits_neither_teleports_nor_makes_another() {
    let mut region = world(SEED, walls);
    let from = BlockPos::new(1, -60, 4);
    let hit = EntityId(20);
    lay(
        &mut region,
        &shulker(hit, bottom_of(from), ShulkerState::default(), None),
    );
    let key = EntityId(53);
    lay(
        &mut region,
        &bullet(key, [1.5, -59.5, 2.6], [0.0, 0.0, 0.4], Some(hit), None),
    );
    for _ in 0..8 {
        idle(&mut region);
    }
    assert!(bullets(&region).is_empty());
    assert!(mob(&region, hit).body.vitals.health < 30.0);
    assert_eq!(shulkers(&region).len(), 1);
    let (_, record) = region.find_entity(hit).expect("the shulker is here");
    assert_eq!(record.position, bottom_of(from));
}

#[test]
fn a_bullet_crosses_a_line_and_flies_on_at_its_target_in_the_region_that_takes_it() {
    let cow = still_cow([24.5, -60.0, 0.5]);
    let mut west = west_of_a_line(SEED, nothing);
    lay_in_copy(&mut west, &cow);
    let key = EntityId(54);
    let flying = bullet(
        key,
        [15.8, -59.0, 0.5],
        [0.3, 0.0, 0.0],
        Some(COW),
        Some(SHOOTER),
    );
    lay(&mut west, &flying);
    let output = idle(&mut west);
    assert!(bullets(&west).is_empty());
    let [(chunk, sent)] = &output.arrivals[..] else {
        panic!("one arrival: {:?}", output.arrivals);
    };
    assert_eq!(sent.key, key);
    let data = data_of(sent);
    assert_eq!(data.target, Some(COW));
    assert_eq!(data.owner.map(|owner| owner.key), Some(SHOOTER));
    // Its wanted motion grew by 1.025 as it flew at the cow it saw in the copy.
    assert_eq!(data.target_delta[0].0, 0.3 * 1.025);
    // The region east of the line, which has the cow, takes it, and it flies on at it.
    let mut east = world(SEED, nothing);
    lay(&mut east, &cow);
    let taken = TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 3,
                index: 0,
            },
            chunk: *chunk,
            entity: sent.clone(),
            due: clustine_world::Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    east.tick(&taken);
    let [now] = &bullets(&east)[..] else {
        panic!("one bullet in the east");
    };
    let now = data_of(now);
    assert_eq!(now.target, Some(COW));
    assert_eq!(now.target_delta[0].0, 0.3 * 1.025 * 1.025);
}

#[test]
fn a_shulker_s_egg_makes_one_on_the_floor_its_head_turned_south_that_stays_where_it_is() {
    let mut region = world(SEED, nothing);
    let (id, _) = add_player(&mut region, 1, [5.5, -60.0, 5.5], false);
    let key = region
        .spawn_mob(
            SHULKER,
            id,
            BlockPos::new(2, -60, 2),
            true,
            false,
            &mut TickOutput::default(),
        )
        .expect("a shulker is made");
    let made = mob(&region, key);
    assert_eq!(made.body.head.head_yaw, 0.0);
    assert_eq!(state_of(&made), ShulkerState::default());
    for _ in 0..40 {
        idle(&mut region);
    }
    let (_, record) = region.find_entity(key).expect("the shulker is here");
    assert_eq!(record.position, [2.5, -60.0, 2.5]);
    assert_eq!(record.motion, [0.0; 3]);
    assert_eq!(state_of(&mob(&region, key)).attach_face, 0);
}

#[test]
fn a_shulker_dies_with_a_shell_by_its_table_s_chance_of_a_half_and_nothing_else() {
    let mut region = world(SEED, nothing);
    let count = 24;
    for index in 0..count {
        let key = EntityId(100 + index);
        let feet = [
            f64::from(index as i32 % 6) * 3.0 - 7.5,
            -60.0,
            f64::from(index as i32 / 6) * 3.0 - 7.5,
        ];
        lay(
            &mut region,
            &shulker(key, feet, ShulkerState::default(), None),
        );
    }
    let generic = crate::mobs::body::hurt::DamageType::named("minecraft:generic")
        .expect("the generic damage type");
    let source = crate::mobs::body::hurt::DamageSource::of(generic);
    for index in 0..count {
        let mut output = TickOutput::default();
        assert!(region.hurt_mob(EntityId(100 + index), &source, 100.0, &mut output));
    }
    for _ in 0..30 {
        idle(&mut region);
    }
    let drops: Vec<i32> = records(&region)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some(stack.item),
            _ => None,
        })
        .collect();
    assert!(
        drops
            .iter()
            .all(|item| *item == clustine_data::items::SHULKER_SHELL),
        "{drops:?}"
    );
    assert!((4..=20).contains(&drops.len()), "{}", drops.len());
}

#[test]
fn a_shulker_without_a_target_peeks_out_at_thirty_now_and_then_and_closes_again() {
    let mut region = world(SEED, nothing);
    lay(
        &mut region,
        &shulker(SHOOTER, [0.5, -60.0, 0.5], ShulkerState::default(), None),
    );
    let mut opened = None;
    let mut closed = None;
    for tick in 0..2000 {
        let output = idle(&mut region);
        let peek = state_of(&mob(&region, SHOOTER)).peek;
        if opened.is_none() && peek == 30 {
            assert_eq!(heard(&output, sounds::ENTITY_SHULKER_OPEN).len(), 1);
            opened = Some(tick);
        }
        if opened.is_some() && peek == 0 {
            assert_eq!(heard(&output, sounds::ENTITY_SHULKER_CLOSE).len(), 1);
            closed = Some(tick);
            break;
        }
    }
    let (opened, closed) = (
        opened.expect("it peeks within 2000 ticks"),
        closed.expect("and closes"),
    );
    // `adjustedTickDelay(20 · (1 + nextInt(3)))` of a goal ticked every other tick: 10,
    // 20 or 30 of its ticks, each two of the mob's.
    let open = closed - opened;
    assert!((18..=62).contains(&open), "{open}");
    // Its lid went towards 0.3 and no further.
    assert!(state_of(&mob(&region, SHOOTER)).opening <= 30.0_f32 * 0.01);
    assert_eq!(
        mob(&region, SHOOTER)
            .body
            .attributes
            .value(SHULKER, attribute(clustine_data::attributes::ARMOR)),
        20.0
    );
}
