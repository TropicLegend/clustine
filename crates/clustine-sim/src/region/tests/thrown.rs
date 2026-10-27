//! Thrown items from the game's rules (`ThrowableProjectile.tick`, `Projectile`,
//! `ProjectileUtil`, `Snowball`; `docs/groundwork/n7k-brief.md`, section 11.10, and the
//! plan's section 10): a snowball's curve worked out by hand tick by tick, its stop at a
//! wall, its hit on a cow and the cow's push, its owner left alone in its first tick, its
//! hit across a line as an effect, a player's throw and its draws, and its flight on
//! across a line.

use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::{
    Arrival, Due, EffectId, Effected, Entity, EntityChange, EntityData, EntityId, EntityUse,
    ItemStack,
};

use super::*;
use crate::mobs::Mob;
use crate::mobs::tick::Local;
use crate::physics::thrown::{self as flight, AIR_DRAG, GRAVITY};
use crate::region::projectiles::{Aim, Making};
use crate::region::random_ticks::Random;
use crate::region::tnt::tests::{nothing, records, west_of_a_line, world};

const SEED: u64 = 0x05E0_BA11;

/// One of the snowball, as a throw makes it.
const SNOWBALL: ItemStack = ItemStack::new(items::SNOWBALL, 1);

/// An owner no region has: a snowball of it has none.
const NOBODY: EntityId = EntityId(999_999);

/// `0.4f` widened: the push of a hurt (`dealDefaultKnockback`).
const KNOCKBACK: f64 = 0.400_000_005_960_464_5;

/// A cow resting on the floor at `feet`, of full health, with no AI, so that its own
/// tick leaves its motion as it is: the resting motion of a mob on the ground.
pub(super) fn cow(key: EntityId, feet: [f64; 3]) -> Entity {
    let kind = entity_types::COW as u16;
    let mut mob = Mob::fresh();
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(
        kind,
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a cow has a health") as f32;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.no_ai = true;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 1),
        position: feet,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
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

/// Lays `entity` in the region's loaded chunk, as a load would have it.
pub(super) fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

pub(super) fn mob(region: &Region, key: EntityId) -> Mob {
    let (_, record) = region.find_entity(key).expect("the mob is here");
    Mob::decode(&record.data).expect("a mob's record")
}

pub(super) fn health(region: &Region, key: EntityId) -> f32 {
    mob(region, key).body.vitals.health
}

/// The snowballs of the region's loaded chunks.
fn snowballs(region: &Region) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| matches!(entity.data, EntityData::Thrown { .. }))
        .collect()
}

fn the_snowball(region: &Region) -> Entity {
    let [snowball] = &snowballs(region)[..] else {
        panic!("one snowball: {:?}", snowballs(region));
    };
    snowball.clone()
}

/// A snowball of `owner` at `at`, shot along `direction` at `power` with no uncertainty,
/// so that its motion is the direction made of length `power`: a test's own shot.
fn shoot(region: &mut Region, owner: EntityId, at: [f64; 3], direction: [f64; 3], power: f32) {
    let making = Making {
        kind: u16::try_from(entity_types::SNOWBALL).unwrap(),
        at,
        data: EntityData::Thrown {
            stack: SNOWBALL,
            owner: Some(owner),
            left_owner: false,
            has_been_shot: false,
        },
        aim: Aim::Direction(direction),
        power,
        uncertainty: 0.0,
    };
    let made = region.make_projectile(making, |_, _| {}, &mut TickOutput::default());
    assert!(made.is_some());
}

/// One tick of `ThrowableProjectile`'s motion by hand: gravity, `Vec3.add(0, -0.03, 0)`,
/// then the air's drag, `scale(0.99f)`.
fn fly(motion: [f64; 3]) -> [f64; 3] {
    let drag = f64::from(AIR_DRAG);
    [
        (motion[0] + 0.0) * drag,
        (motion[1] + -GRAVITY) * drag,
        (motion[2] + 0.0) * drag,
    ]
}

/// `LivingEntity.knockback(0.4, -x, -z)` of a mob of no resistance that moved by `old`
/// and stood on the ground, pushed along a projectile's `motion`.
fn knocked(old: [f64; 3], motion: [f64; 3]) -> [f64; 3] {
    let (x, z) = (-motion[0], -motion[2]);
    let length = (x * x + z * z).sqrt();
    let push = [x / length * KNOCKBACK, 0.0, z / length * KNOCKBACK];
    [
        old[0] / 2.0 - push[0],
        (old[1] / 2.0 + KNOCKBACK).min(0.4),
        old[2] / 2.0 - push[2],
    ]
}

/// The entity events of `output` of `key`.
fn events_of(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
            _ => None,
        })
        .collect()
}

fn gone(output: &TickOutput, key: EntityId) -> bool {
    output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key))
}

#[test]
fn a_snowball_thrown_straight_ahead_falls_along_the_games_curve() {
    let mut region = world(SEED, nothing);
    let start = [0.5, -55.0, 0.5];
    shoot(&mut region, NOBODY, start, [0.0, 0.0, 1.0], 1.5);
    let made = the_snowball(&region);
    // Shot along z at 1.5 with no uncertainty: of length one, nothing added, scaled.
    assert_eq!(made.motion, [0.0, 0.0, 1.5]);
    assert_eq!((made.yaw, made.pitch), (0.0, 0.0));
    let mut position = start;
    let mut motion = made.motion;
    for tick in 1..=4 {
        idle(&mut region);
        motion = fly(motion);
        position = [
            position[0] + motion[0],
            position[1] + motion[1],
            position[2] + motion[2],
        ];
        let now = the_snowball(&region);
        assert_eq!(
            now.position.map(f64::to_bits),
            position.map(f64::to_bits),
            "tick {tick}: {:?} against {position:?}",
            now.position
        );
        assert_eq!(now.motion.map(f64::to_bits), motion.map(f64::to_bits));
    }
    // The fourth tick by hand: 1.5 along z kept at 0.99 a tick, and 0.03 down a tick
    // gathered and kept likewise.
    let drag = f64::from(0.99_f32);
    let along = 1.5 * drag * drag * drag * drag;
    let down = ((((-0.03 * drag) - 0.03) * drag - 0.03) * drag - 0.03) * drag;
    assert!((motion[2] - along).abs() < 1.0e-12, "{motion:?}");
    assert!((motion[1] - down).abs() < 1.0e-12, "{motion:?}");
    // 0.5, then 1.485, 1.470, 1.455 and 1.441 on: 6.3515.
    assert!((position[2] - 6.3515).abs() < 1.0e-4, "{position:?}");
    // It turns a fifth of the way towards its motion each tick: down a little.
    let now = the_snowball(&region);
    assert!(now.pitch < 0.0 && now.pitch > -2.0, "{}", now.pitch);
    assert_eq!(now.yaw, 0.0);
}

#[test]
fn a_snowball_stops_at_a_wall_with_its_particles_and_is_gone() {
    let wall = |pos: clustine_world::BlockPos| (pos.z == 5 && pos.y > -61).then_some(blocks::STONE);
    let mut region = world(SEED, wall);
    shoot(&mut region, NOBODY, [0.5, -55.0, 0.5], [0.0, 0.0, 1.0], 1.5);
    let key = the_snowball(&region).key;
    // At 0.5, 1.985, 3.455 and 4.910: three ticks short of the wall's face at z = 5.
    for _ in 0..3 {
        let output = idle(&mut region);
        assert!(events_of(&output, key).is_empty());
        assert!(!gone(&output, key));
    }
    assert!((the_snowball(&region).position[2] - 4.910).abs() < 1.0e-3);
    // The fourth tick's move would take it to 6.36: the wall stops it.
    let output = idle(&mut region);
    assert_eq!(events_of(&output, key), [3]);
    assert!(gone(&output, key));
    assert!(snowballs(&region).is_empty());
    // Its particles are shown before it is gone, as the game's `onHit` does.
    let event_at = output.events.iter().position(
        |event| matches!(event, RegionEvent::EntityEvent { entity, .. } if *entity == key),
    );
    let gone_at = output.events.iter().position(
        |event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key),
    );
    assert!(event_at < gone_at);
}

#[test]
fn a_snowball_hurts_a_cow_by_nothing_and_pushes_it_along_its_motion() {
    let mut region = world(SEED, nothing);
    let key = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
    lay(&mut region, &cow(key, [3.5, -60.0, 0.5]));
    let full = health(&region, key);
    // A snowball of nobody's along x: at 1.985 after a tick, and its second tick's move
    // to 3.455 enters the cow's box at 3.05.
    shoot(&mut region, NOBODY, [0.5, -59.0, 0.5], [1.0, 0.0, 0.0], 1.5);
    let snowball = the_snowball(&region).key;
    assert!(snowball > key, "the cow ticks first");
    idle(&mut region);
    let before = region.find_entity(key).expect("the cow").1.motion;
    assert!(mob(&region, key).body.vitals.hurt_from.is_none());
    let output = idle(&mut region);
    assert_eq!(events_of(&output, snowball), [3]);
    assert!(gone(&output, snowball));
    // Hurt by nothing: its health whole, but the hurt taken, which pushes it along the
    // snowball's motion in the tick it hit, and lifts it.
    assert_eq!(health(&region, key), full);
    assert!(mob(&region, key).body.vitals.hurt_from.is_some());
    let motion = fly(fly([1.5, 0.0, 0.0]));
    let after = region.find_entity(key).expect("the cow").1.motion;
    assert_eq!(
        after.map(f64::to_bits),
        knocked(before, motion).map(f64::to_bits),
        "{after:?}"
    );
    assert!(after[0] > 0.0 && after[1] > 0.0, "{after:?}");
}

#[test]
fn a_mobs_snowball_passes_its_thrower_in_its_first_tick_where_anyone_elses_hits() {
    // The cow throws (`Local::Shoot`) from beside itself through its own box: in its
    // first tick its snowball's move crosses the cow, and leaves it unhurt.
    let mut region = world(SEED, nothing);
    let key = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
    lay(&mut region, &cow(key, [3.5, -60.0, 0.5]));
    let shot = Local::Shoot(crate::mobs::tick::Shot {
        kind: u16::try_from(entity_types::SNOWBALL).unwrap(),
        stack: SNOWBALL,
        weapon: None,
        at: [2.5, -59.0, 0.5],
        direction: [1.0, 0.0, 0.0],
        power: flight::GOLEM_POWER,
        uncertainty: 0.0,
        from_mob: None,
    });
    region.apply_locals(key, vec![shot], &mut TickOutput::default());
    let made = the_snowball(&region);
    assert!(matches!(
        made.data,
        EntityData::Thrown { owner: Some(owner), left_owner: false, has_been_shot: false, .. }
            if owner == key
    ));
    let output = idle(&mut region);
    assert!(events_of(&output, made.key).is_empty());
    assert!(mob(&region, key).body.vitals.hurt_from.is_none());
    // It is within a block of its thrower still, so it has not left it.
    assert!(matches!(
        the_snowball(&region).data,
        EntityData::Thrown {
            left_owner: false,
            has_been_shot: true,
            ..
        }
    ));
    // The same shot of nobody's hits the cow in that tick.
    let mut region = world(SEED, nothing);
    let key = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
    lay(&mut region, &cow(key, [3.5, -60.0, 0.5]));
    shoot(
        &mut region,
        NOBODY,
        [2.5, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        flight::GOLEM_POWER,
    );
    let other = the_snowball(&region).key;
    let output = idle(&mut region);
    assert_eq!(events_of(&output, other), [3]);
    assert!(mob(&region, key).body.vitals.hurt_from.is_some());
}

#[test]
fn a_snowball_hits_a_cow_across_a_pinned_line_through_its_holder() {
    // The cow is in the copy of the column east of the line, x = 16 on.
    let mut region = west_of_a_line(SEED, nothing);
    let key = EntityId(77_777);
    let across = cow(key, [17.0, -60.0, 0.5]);
    region
        .copies
        .get_mut(&across.chunk())
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(across.clone()));
    // From 14 along x: at 15.485 after a tick, still west of the line; the second
    // tick's move to 16.955 enters the cow's box at 16.55.
    shoot(
        &mut region,
        NOBODY,
        [14.0, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        1.5,
    );
    let snowball = the_snowball(&region).key;
    let output = idle(&mut region);
    assert!(output.effects.is_empty(), "{:?}", output.effects);
    let output = idle(&mut region);
    assert_eq!(events_of(&output, snowball), [3]);
    assert!(gone(&output, snowball));
    let motion = fly(fly([1.5, 0.0, 0.0]));
    let hits: Vec<EntityUse> = output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::OnEntity { key: hit, ref by } if hit == key => Some(by.clone()),
            _ => None,
        })
        .collect();
    let [ref by] = hits[..] else {
        panic!("one hit on the cow: {:?}", output.effects);
    };
    let EntityUse::Thrown {
        projectile,
        kind,
        motion: sent,
        damage,
        owner,
        ..
    } = by.clone()
    else {
        panic!("a thrown item's hit: {by:?}");
    };
    assert_eq!(projectile, snowball);
    assert_eq!(i32::from(kind), entity_types::SNOWBALL);
    assert_eq!(
        sent.map(|value| value.0.to_bits()),
        motion.map(f64::to_bits)
    );
    assert_eq!(damage.0, 0.0);
    assert_eq!(owner, None);
    // The holder takes it as a hit of its own: the cow is hurt by nothing and pushed.
    let mut holder = world(SEED, nothing);
    lay(&mut holder, &across);
    let full = health(&holder, key);
    holder.entity_used(key, by.clone(), &mut TickOutput::default());
    assert_eq!(health(&holder, key), full);
    assert!(mob(&holder, key).body.vitals.hurt_from.is_some());
    let after = holder.find_entity(key).expect("the cow").1.motion;
    assert_eq!(
        after.map(f64::to_bits),
        knocked(across.motion, motion).map(f64::to_bits)
    );
}

#[test]
fn a_players_throw_draws_its_pitch_then_the_snowballs_uuid_and_shot_and_keeps_the_stack() {
    let mut region = on_ticking_floor_with(&[]);
    let stack = clustine_world::ItemStack::new(items::SNOWBALL, 16);
    let thrower = region.players.get_mut(&player(1)).expect("player 1");
    thrower.inventory.set(0, Some(stack.clone()));
    thrower.selected_slot = 0;
    let feet = thrower.pose.position;
    let use_item = PlayerInput::UseItem {
        rotation: (30.0, -20.0),
        sneaking: false,
        sequence: 1,
    };
    let output = checked(&mut region, &moves(vec![numbered(1, use_item)]));
    let tick = region.tick;
    let seed = region.config.seed;
    // The sound first, of the level's random: its first draw in the tick.
    let thrown: Vec<(clustine_world::Vec3, f32, f32)> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                position,
                volume,
                pitch,
                ..
            } if *sound == sounds::ENTITY_SNOWBALL_THROW => Some((*position, *volume, *pitch)),
            _ => None,
        })
        .collect();
    let mut level = Random::of_section(seed, ChunkPos::new(0, 0), 1, tick);
    let pitch = 0.4 / (level.next_float() * 0.4 + 0.8);
    assert_eq!(thrown, [(feet, 0.5, pitch)]);
    // The snowball's own stream at its birth: its UUID's two longs, then the shot's
    // three triangles, each two doubles, along the look and at 1.5.
    let snowball = the_snowball(&region);
    let mut own = Random::of_entity_aside(seed, snowball.key, 0, tick);
    let uuid = crate::region::entities::insecure_uuid(own.next_u64(), own.next_u64());
    assert_eq!(snowball.uuid, uuid);
    let shot = flight::shoot_from_rotation(30.0, -20.0, 1.5, 1.0, [0.0; 3], true, &mut own);
    // Made before the entities' phase, it flew once in the tick of its throw.
    let motion = fly(shot.motion);
    let start = [
        feet.x,
        feet.y + f64::from(1.62_f32) - 0.100_000_001_490_116_12,
        feet.z,
    ];
    let position = [
        start[0] + motion[0],
        start[1] + motion[1],
        start[2] + motion[2],
    ];
    assert_eq!(snowball.motion.map(f64::to_bits), motion.map(f64::to_bits));
    assert_eq!(
        snowball.position.map(f64::to_bits),
        position.map(f64::to_bits)
    );
    let [yaw, pitch] = flight::update_rotation(shot.yaw, shot.pitch, motion);
    assert_eq!((snowball.yaw, snowball.pitch), (yaw, pitch));
    // Looking 30 degrees to the west of south and up: along +z, -x and up.
    assert!(shot.motion[0] < 0.0 && shot.motion[1] > 0.0 && shot.motion[2] > 0.0);
    assert!(matches!(
        snowball.data,
        EntityData::Thrown {
            owner: Some(EntityId(1)),
            stack: SNOWBALL,
            ..
        }
    ));
    // A creative player keeps the stack.
    let thrower = region.players.get(&player(1)).expect("player 1");
    assert_eq!(thrower.inventory.0[0], Some(stack));
}

#[test]
fn a_snowball_crosses_a_line_in_flight_and_flies_on_there() {
    let mut west = west_of_a_line(SEED, nothing);
    shoot(&mut west, NOBODY, [15.0, -55.0, 0.5], [1.0, 0.0, 0.0], 1.5);
    let made = the_snowball(&west);
    let output = idle(&mut west);
    let first = fly([1.5, 0.0, 0.0]);
    let crossed = [15.0 + first[0], -55.0 + first[1], 0.5 + first[2]];
    assert!(snowballs(&west).is_empty());
    let [(chunk, sent)] = &output.arrivals[..] else {
        panic!("one arrival: {:?}", output.arrivals);
    };
    assert_eq!(*chunk, ChunkPos::new(1, 0));
    assert_eq!(sent.key, made.key);
    assert_eq!(sent.position.map(f64::to_bits), crossed.map(f64::to_bits));
    assert_eq!(sent.motion.map(f64::to_bits), first.map(f64::to_bits));
    // The region east of the line takes it and it flies on.
    let mut east = world(SEED, nothing);
    let taken = TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 3,
                index: 0,
            },
            chunk: *chunk,
            entity: sent.clone(),
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    east.tick(&taken);
    let second = fly(first);
    let on = the_snowball(&east);
    assert_eq!(on.key, made.key);
    assert_eq!(
        on.position.map(f64::to_bits),
        [
            crossed[0] + second[0],
            crossed[1] + second[1],
            crossed[2] + second[2]
        ]
        .map(f64::to_bits)
    );
    assert!(matches!(
        on.data,
        EntityData::Thrown {
            has_been_shot: true,
            ..
        }
    ));
}

/// The face of a player of another region, standing or crouching with their feet at
/// `feet`, as the store streams it.
fn face_at(feet: [f64; 3], sneaking: bool) -> crate::api::FaceChange {
    crate::api::FaceChange::Face {
        player: PlayerId(uuid::Uuid::from_u128(9)),
        entity: EntityId(500),
        hops: 0,
        seq: crate::api::FaceSeq(1, 0),
        face: crate::api::PlayerFace::of(
            &crate::api::Pose::at(clustine_world::Vec3::new(feet[0], feet[1], feet[2])),
            sneaking,
            false,
            &clustine_world::Inventory::default(),
            0,
            None,
        ),
    }
}

/// Section 4.1 of the N9 plan: another region's player, whose face the region keeps in
/// the copy east of the line, is a target the hit test finds after the region's own,
/// 1.8 tall standing and 1.5 crouching.
#[test]
fn the_face_of_another_region_s_player_is_a_target_of_their_height() {
    for (sneaking, height) in [(false, 1.8), (true, 1.5)] {
        let mut region = west_of_a_line(SEED, nothing);
        let inputs = TickInputs {
            faces: vec![face_at([17.5, -60.0, 0.5], sneaking)],
            ..TickInputs::default()
        };
        region.tick(&inputs);
        let around = crate::physics::aabb::Aabb::of_entity([17.5, -60.0, 0.5], 2.0, 3.0);
        let targets = region.targets(&around, NOBODY);
        let [face] = &targets[..] else {
            panic!("the face alone: {targets:?}");
        };
        assert_eq!(face.key, EntityId(500));
        assert_eq!(face.kind, crate::region::projectiles::TargetKind::Face);
        assert!((face.bounds.max[1] - face.bounds.min[1] - height).abs() < 1e-6);
    }
}

/// Section 4.1: a snowball thrown past another region's player turns back from their
/// face as from a creative player's: it is gone at them, and nothing is sent to anyone.
#[test]
fn a_snowball_that_hits_another_region_s_player_is_gone_and_sends_nothing() {
    let mut region = west_of_a_line(SEED, nothing);
    let inputs = TickInputs {
        faces: vec![face_at([17.0, -60.0, 0.5], false)],
        ..TickInputs::default()
    };
    region.tick(&inputs);
    shoot(
        &mut region,
        NOBODY,
        [14.0, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        1.5,
    );
    let snowball = the_snowball(&region).key;
    let output = idle(&mut region);
    assert!(!gone(&output, snowball));
    let output = idle(&mut region);
    assert_eq!(events_of(&output, snowball), [3]);
    assert!(gone(&output, snowball));
    assert!(output.effects.is_empty(), "{:?}", output.effects);
}

/// Section 4.1's rule for a making: where the region can read nothing, a projectile is
/// not made, and no key is taken.
#[test]
fn a_projectile_is_not_made_where_the_region_can_read_nothing() {
    let mut region = west_of_a_line(SEED, nothing);
    let making = Making {
        kind: u16::try_from(entity_types::SNOWBALL).unwrap(),
        at: [200.0, -59.0, 0.5],
        data: EntityData::Thrown {
            stack: SNOWBALL,
            owner: None,
            left_owner: false,
            has_been_shot: false,
        },
        aim: Aim::Direction([1.0, 0.0, 0.0]),
        power: 1.5,
        uncertainty: 0.0,
    };
    let mut output = TickOutput::default();
    assert_eq!(region.make_projectile(making, |_, _| {}, &mut output), None);
    assert!(
        output.events.is_empty() && output.effects.is_empty(),
        "{output:?}"
    );
}

/// `AbstractArrow.<init>`: an arrow made of intangible ammunition may be picked up by a
/// creative player only; any other made by a shot by nobody.
#[test]
fn an_arrow_of_intangible_ammunition_is_for_creative_players_only() {
    use clustine_world::components::Component;
    let mut stack = ItemStack::new(items::ARROW, 1);
    let shot = |stack: ItemStack| crate::mobs::tick::Shot {
        kind: u16::try_from(entity_types::ARROW).unwrap(),
        stack,
        weapon: None,
        at: [0.0; 3],
        direction: [1.0, 0.0, 0.0],
        power: 1.0,
        uncertainty: 0.0,
        from_mob: None,
    };
    let pickup = |data: Option<EntityData>| match data {
        Some(EntityData::Arrow(arrow)) => arrow.pickup,
        other => panic!("{other:?} is no arrow"),
    };
    let record = crate::region::projectiles::shot_record(&shot(stack.clone()), None);
    assert_eq!(pickup(record), clustine_world::Pickup::Disallowed);
    stack.components.set(Component::IntangibleProjectile);
    let record = crate::region::projectiles::shot_record(&shot(stack), None);
    assert_eq!(pickup(record), clustine_world::Pickup::CreativeOnly);
}

/// A llama's shot is its spit, owned by the llama (`Llama.spit`); a trident's record has
/// the trident's own hit sound (`ThrownTrident.getDefaultHitGroundSoundEvent`).
#[test]
fn a_llama_s_shot_makes_its_spit_and_a_trident_s_its_own_hit_sound() {
    let shot = |kind: i32, item| crate::mobs::tick::Shot {
        kind: u16::try_from(kind).unwrap(),
        stack: ItemStack::new(item, 1),
        weapon: None,
        at: [0.0; 3],
        direction: [1.0, 0.0, 0.0],
        power: 1.5,
        uncertainty: 10.0,
        from_mob: Some(1.0),
    };
    let llama = EntityId(17);
    let record = crate::region::projectiles::shot_record(
        &shot(entity_types::LLAMA_SPIT, items::AIR),
        Some(llama),
    );
    assert_eq!(
        record,
        Some(EntityData::Spit(clustine_world::projectiles::Spit {
            owner: Some(llama),
            left_owner: false,
            has_been_shot: false,
        }))
    );
    let trident =
        crate::region::projectiles::shot_record(&shot(entity_types::TRIDENT, items::TRIDENT), None);
    let Some(EntityData::Arrow(arrow)) = trident else {
        panic!("{trident:?} is no trident");
    };
    assert_eq!(arrow.hit_sound, "minecraft:item.trident.hit_ground");
}
