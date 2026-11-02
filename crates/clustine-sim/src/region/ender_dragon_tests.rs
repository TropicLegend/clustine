//! The ender dragon in a region, from the game's rules (d EnderDragon.txt and its
//! phases, DragonFireball.txt; `docs/groundwork/p-brief.md`, section 6.4), not from the
//! code: its parts placed after a tick, a hit on its head and on a wing, its healing by
//! a crystal, the first node of its holding pattern and its draws, the landing approach
//! one draw in three without crystals, the sitting phases' order and the flame's cloud,
//! the strafe's fireball and its burst, its death's 200 ticks with its experience and
//! the global event 1028, its wings' hurt of a mob and of no creative player, and the
//! walls it breaks and the ones that hold it.
//!
//! The test world is the overworld's floor of stone at y = -61 over 5 × 5 chunks about
//! the origin, every node of the dragon's graph at y 73 over it.

use clustine_data::{BlockState, blocks, entity_types};
use clustine_world::{
    BlockPos, Dimension, EdgeId, EndCrystal, Entity, EntityChange, EntityData, EntityId, PlayerId,
};

use crate::api::{PlayerInput, RegionEvent, TickInputs, TickOutput};
use crate::mobs::Mob;
use crate::mobs::types::ender_dragon::{
    DRAGON, DRAGON_FIREBALL, DragonState, change, nodes, parts, phases, state_of,
};
use crate::physics::aabb::Aabb;
use crate::region::tnt::tests::{add_player, idle, nothing, records, world};
use crate::region::{Random, Region};

const SEED: u64 = 0xE9_D2_A6;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
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

/// A dragon of full health under the key `key` at `at`, facing `yaw`, its state as
/// `set` leaves a new one's.
fn dragon(key: EntityId, at: [f64; 3], yaw: f32, set: impl FnOnce(&mut DragonState)) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 200.0;
    mob.body.vitals.air = 300;
    change(&mut mob, set);
    Entity {
        portal: Default::default(),
        key,
        kind: DRAGON,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 13),
        position: at,
        motion: [0.0; 3],
        yaw,
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

/// A cow of full health at `feet` with no AI, ten ticks old.
fn cow(key: EntityId, feet: [f64; 3]) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.no_ai = true;
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::COW),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 1),
        position: feet,
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
    }
}

/// An end crystal at `at`, its base shown.
fn crystal(key: EntityId, at: [f64; 3]) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::END_CRYSTAL),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 11),
        position: at,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 3,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::EndCrystal(EndCrystal {
            beam_target: None,
            show_bottom: true,
        }),
    }
}

fn record_of(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .map(|(_, record)| record.clone())
        .expect("the entity is there")
}

fn mob_of(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record_of(region, key).data).expect("a mob's record")
}

fn own(region: &Region, key: EntityId) -> DragonState {
    state_of(&mob_of(region, key))
}

/// The dragon's own stream in its next tick, as the region will give it.
fn replay(region: &Region, key: EntityId) -> Random {
    Random::of_entity(region.seed(), key, record_of(region, key).lived)
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1.0e-6)
}

fn near_box(a: Aabb, b: Aabb) -> bool {
    near(a.min, b.min) && near(a.max, b.max)
}

/// Player 1's hit on `target` at its part `part`, their input `number`.
fn attack(
    region: &mut Region,
    (id, entity): (PlayerId, EntityId),
    number: u64,
    target: EntityId,
    part: Option<u8>,
) -> TickOutput {
    region.tick(&TickInputs {
        inputs: vec![(
            EdgeId(1),
            id,
            entity,
            number,
            PlayerInput::Attack {
                entity: target,
                part,
            },
        )],
        ..TickInputs::default()
    })
}

// Its parts.

/// Hovering at (10.5, -40, 10.5) facing south (yaw 0) with nothing behind it: its
/// target is where it is, so it is pushed `0.06 · (⅓ · 2 − 1)` along its facing's
/// other way, 0.02 south; its history is its one sample, flat; it sits, so its head is
/// 1 down; its yaw's sine is 0 and its cosine 1. So the body is half a block north,
/// the wings 4.5 east and west and 2 up, the head 6.5 and the neck 5.5 north and 1 down,
/// the tail 3.5, 5.5 and 7.5 south and 1.5 up, each a box of its own size at its feet.
#[test]
fn its_parts_stand_about_it_after_a_tick_as_aistep_places_them() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    lay(&mut region, &dragon(key, [10.5, -40.0, 10.5], 0.0, |_| {}));
    idle(&mut region);
    let record = record_of(&region, key);
    assert!(
        near(record.position, [10.5, -40.0, 10.52]),
        "{:?}",
        record.position
    );
    let [x, y, z] = record.position;
    let own = own(&region, key);
    let expected = [
        (parts::HEAD, [x, y - 1.0, z - 6.5]),
        (parts::NECK, [x, y - 1.0, z - 5.5]),
        (parts::BODY, [x, y, z - 0.5]),
        (parts::TAIL[0], [x, y + 1.5, z + 3.5]),
        (parts::TAIL[1], [x, y + 1.5, z + 5.5]),
        (parts::TAIL[2], [x, y + 1.5, z + 7.5]),
        (parts::WING[0], [x + 4.5, y + 2.0, z]),
        (parts::WING[1], [x - 4.5, y + 2.0, z]),
    ];
    for (part, at) in expected {
        assert!(
            near(own.parts[usize::from(part)], at),
            "part {part}: {:?}",
            own.parts[usize::from(part)]
        );
    }
    let boxes = crate::mobs::types::ender_dragon::part_boxes(&record).expect("a dragon");
    assert!(near_box(
        boxes[usize::from(parts::HEAD)],
        Aabb::new([x - 0.5, y - 1.0, z - 7.0], [x + 0.5, y, z - 6.0])
    ));
    assert!(near_box(
        boxes[usize::from(parts::BODY)],
        Aabb::new([x - 2.5, y, z - 3.0], [x + 2.5, y + 3.0, z + 2.0])
    ));
    assert!(near_box(
        boxes[usize::from(parts::WING[0])],
        Aabb::new([x + 2.5, y + 2.0, z - 2.0], [x + 6.5, y + 4.0, z + 2.0])
    ));
}

// Its hurt.

/// A dragon without AI whose parts stand by the player: the head takes the hit whole
/// (an empty hand's 1), a wing `1 / 4 + 1`.
#[test]
fn a_hit_on_its_head_takes_the_whole_and_on_a_wing_a_quarter_and_one() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let mut record = dragon(key, [10.5, -55.0, 2.5], 0.0, |own| {
        own.parts[usize::from(parts::HEAD)] = [4.5, -59.0, 2.5];
        own.parts[usize::from(parts::WING[0])] = [5.5, -60.0, 5.5];
    });
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.flags.no_ai = true;
    record.data = mob.encode();
    lay(&mut region, &record);
    let player = add_player(&mut region, 1, [2.5, -60.0, 2.5], false);
    attack(&mut region, player, 1, key, Some(parts::HEAD));
    assert_eq!(mob_of(&region, key).body.vitals.health, 199.0);
    for _ in 0..20 {
        idle(&mut region);
    }
    attack(&mut region, player, 2, key, Some(parts::WING[0]));
    assert_eq!(mob_of(&region, key).body.vitals.health, 197.75);
    // A part out of reach (the far wing, by the origin) is not hit.
    for _ in 0..20 {
        idle(&mut region);
    }
    attack(&mut region, player, 3, key, Some(parts::WING[1]));
    assert_eq!(mob_of(&region, key).body.vitals.health, 197.75);
}

/// A hit by a falling player is critical against a living target only
/// (`canCriticalAttack`'s `instanceof LivingEntity`): a dragon's part is not one, so
/// the head takes the hand's 1, not a critical 1.5, and no critical sound is made.
#[test]
fn a_falling_player_s_hit_on_its_head_is_not_critical() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let mut record = dragon(key, [10.5, -55.0, 2.5], 0.0, |own| {
        own.parts[usize::from(parts::HEAD)] = [4.5, -59.0, 2.5];
    });
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.flags.no_ai = true;
    record.data = mob.encode();
    lay(&mut region, &record);
    let (id, entity) = add_player(&mut region, 1, [2.5, -60.0, 2.5], false);
    // Up a block and half of it back down, off the ground: falling.
    for (number, y) in [(1, -59.0), (2, -59.5)] {
        region.tick(&TickInputs {
            inputs: vec![(
                EdgeId(1),
                id,
                entity,
                number,
                PlayerInput::Move {
                    position: Some(clustine_world::Vec3::new(2.5, y, 2.5)),
                    rotation: None,
                    on_ground: false,
                },
            )],
            ..TickInputs::default()
        });
    }
    for _ in 0..20 {
        idle(&mut region);
    }
    let output = attack(&mut region, (id, entity), 3, key, Some(parts::HEAD));
    assert_eq!(mob_of(&region, key).body.vitals.health, 199.0);
    let crit_sound = output.events.iter().any(|event| {
        matches!(event, RegionEvent::Sound { sound, .. }
            if *sound == clustine_data::sounds::ENTITY_PLAYER_ATTACK_CRIT)
    });
    assert!(!crit_sound, "no critical sound: {:?}", output.events);
}

// Its healing.

/// A crystal 10 blocks off: one tick in ten it is chosen as the nearest, and from then
/// the dragon heals by one every tenth tick.
#[test]
fn the_nearest_crystal_heals_it_one_every_ten_ticks() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let mut record = dragon(key, [10.5, -50.0, 10.5], 0.0, |_| {});
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.vitals.health = 100.0;
    mob.body.flags.no_ai = true;
    record.data = mob.encode();
    lay(&mut region, &record);
    let crystal_key = EntityId(701);
    lay(&mut region, &crystal(crystal_key, [20.5, -50.0, 10.5]));
    let mut ticks = 0;
    while own(&region, key).nearest_crystal.is_none() {
        assert!(ticks < 200, "a crystal is chosen one tick in ten");
        idle(&mut region);
        ticks += 1;
    }
    assert_eq!(own(&region, key).nearest_crystal, Some(crystal_key));
    let start = mob_of(&region, key).body.vitals.health;
    for _ in 0..30 {
        idle(&mut region);
    }
    assert_eq!(mob_of(&region, key).body.vitals.health, start + 3.0);
}

// Its holding pattern.

/// At (20.5, 80, 0.5) with no fight, the nodes looked at are the inner rings', the
/// nearest node 20 at (20, 73, 0); a turn one draw in eight or not, the next is 19 at
/// (28, 73, -29) (`(19 − 12) & 7 + 12` or `(20 + 6 + 1 − 12) & 7 + 12`), a neighbour
/// of 20; the target is that node raised by the draw that follows. The draws in
/// the tick before it: the ambient sound's chance (`nextInt(1000)`) and the crystals'
/// (`nextInt(10)`).
#[test]
fn the_holding_patterns_first_target_is_the_next_node_on_the_middle_ring_raised_by_a_draw() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    lay(
        &mut region,
        &dragon(key, [20.5, 80.0, 0.5], 0.0, |own| {
            own.phase = phases::HOLDING_PATTERN;
        }),
    );
    let mut draws = replay(&region, key);
    assert!(
        draws.next_int(1000) >= 1,
        "no ambient sound at its first tick"
    );
    draws.next_int(10);
    let turned = draws.next_int(8) == 0;
    let raise = draws.next_float() * 20.0;
    idle(&mut region);
    let own = own(&region, key);
    assert_eq!(own.phase, phases::HOLDING_PATTERN);
    assert_eq!(own.phases.holding.clockwise, turned);
    let path = own.phases.holding.path.expect("a path");
    assert_eq!(path.nodes, vec![[20, 73, 0], [28, 73, -29]]);
    assert_eq!(path.next, 2);
    assert_eq!(
        own.phases.holding.target,
        Some([28.0, f64::from(73.0_f32 + raise), -29.0])
    );
    let made = own.nodes.expect("its nodes");
    assert_eq!(made.len(), nodes::NODE_COUNT);
    assert_eq!(made[12], [40, 73, 0]);
}

/// At the end of a path, with no fight and so no crystals, the landing approach comes
/// one draw in three (`nextInt(0 + 3)`); then its own first tick in the same step
/// makes a path to the podium's top, (0, -60, 0) here. Tried for dragons of several
/// keys until each way has come.
#[test]
fn without_crystals_the_holding_pattern_turns_to_the_landing_approach_one_draw_in_three() {
    let (mut landed, mut held) = (false, false);
    for key in (700..760).map(EntityId) {
        let mut region = world(SEED, nothing);
        let at = [20.5, 80.0, 0.5];
        lay(
            &mut region,
            &dragon(key, at, 0.0, |own| {
                own.phase = phases::HOLDING_PATTERN;
                own.phases.holding.target = Some(at);
                own.phases.holding.path = Some(nodes::Path {
                    nodes: vec![[20, 73, 0]],
                    next: 1,
                });
            }),
        );
        let mut draws = replay(&region, key);
        draws.next_int(1000);
        draws.next_int(10);
        let lands = draws.next_int(3) == 0;
        idle(&mut region);
        let own = own(&region, key);
        if lands {
            assert_eq!(own.phase, phases::LANDING_APPROACH);
            let path = own.phases.approach.path.expect("a path to the podium");
            assert_eq!(path.nodes.last(), Some(&[0, -60, 0]));
            landed = true;
        } else {
            assert_eq!(own.phase, phases::HOLDING_PATTERN);
            held = true;
        }
        if landed && held {
            return;
        }
    }
    panic!("both ways came: landed {landed}, held {held}");
}

// The sitting phases.

/// Within a block of the podium's top (0.5, -60, 0.5) the landing sits to scan, the
/// flames counted again from none; the scan, with only a creative player by it, takes
/// off at its hundredth tick; the roar flames at its fortieth, its flame counted.
#[test]
fn the_landing_sits_to_scan_which_takes_off_after_100_ticks_with_only_creative_players_by() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    lay(
        &mut region,
        &dragon(key, [0.5, -59.5, 0.5], 0.0, |own| {
            own.phase = phases::LANDING;
            own.phases.flaming.count = 3;
        }),
    );
    add_player(&mut region, 1, [5.5, -60.0, 0.5], false);
    idle(&mut region);
    let state = own(&region, key);
    assert_eq!(state.phase, phases::SITTING_SCANNING);
    assert_eq!(state.phases.flaming.count, 0);
    // The scan ticked once already, in the step it began (`aiStep` ticks the phase it
    // went to): 99 more make its hundredth.
    assert_eq!(state.phases.scanning, 1);
    let mut ticks = 0;
    while own(&region, key).phase == phases::SITTING_SCANNING {
        assert!(ticks < 150);
        idle(&mut region);
        ticks += 1;
    }
    assert_eq!(ticks, 99);
    assert_eq!(own(&region, key).phase, phases::TAKEOFF);

    let other = EntityId(701);
    lay(
        &mut region,
        &dragon(other, [-10.5, -50.0, 20.5], 0.0, |own| {
            own.phase = phases::SITTING_ATTACKING;
        }),
    );
    let mut ticks = 0;
    while own(&region, other).phase == phases::SITTING_ATTACKING {
        assert!(ticks < 60);
        idle(&mut region);
        ticks += 1;
    }
    // `attackingTicks++ >= 40` compares the count before it rises: the roar lasts 41.
    assert_eq!(ticks, 41);
    let state = own(&region, other);
    assert_eq!(state.phase, phases::SITTING_FLAMING);
    assert_eq!(state.phases.flaming.count, 1);
}

fn clouds(region: &Region) -> Vec<(Entity, clustine_world::projectiles::Cloud)> {
    records(region)
        .into_iter()
        .filter_map(|entity| match &entity.data {
            EntityData::Cloud(cloud) => Some((entity.clone(), (**cloud).clone())),
            _ => None,
        })
        .collect()
}

/// Flaming at (10.5, -50, 10.5) facing south, its head 6.5 north and 1 down: at its
/// tenth tick a cloud of breath 2.5 further out along its neck, at (10.5, z 1.5), on
/// the block below the head's middle (stone at (10, -51, 1): y -50), radius 5 for 200
/// ticks, instant damage, the dragon's; at its 200th tick, its fourth flame, it takes
/// off and the cloud is gone.
#[test]
fn the_flame_breathes_a_cloud_at_its_tenth_tick_ahead_of_its_head_and_ends_with_it() {
    let stone = |pos: BlockPos| -> Option<BlockState> {
        (pos == BlockPos::new(10, -51, 1)).then_some(blocks::STONE)
    };
    let mut region = world(SEED, stone);
    let key = EntityId(700);
    lay(
        &mut region,
        &dragon(key, [10.5, -50.0, 10.5], 0.0, |own| {
            own.phase = phases::SITTING_FLAMING;
            own.phases.flaming.count = 4;
            own.parts[usize::from(parts::HEAD)] = [10.5, -51.0, 4.0];
        }),
    );
    for _ in 0..9 {
        idle(&mut region);
    }
    assert!(clouds(&region).is_empty());
    idle(&mut region);
    let [(made, cloud)] = &clouds(&region)[..] else {
        panic!("one cloud: {:?}", clouds(&region));
    };
    assert!(
        near(made.position, [10.5, -50.0, 1.5]),
        "{:?}",
        made.position
    );
    assert_eq!(cloud.radius.0, 5.0);
    assert_eq!(cloud.duration, 200);
    assert_eq!(cloud.owner, Some(key));
    assert_eq!(cloud.duration_scale.0, 0.25);
    assert_eq!(
        cloud.potion.custom_effects[0].effect,
        "minecraft:instant_damage"
    );
    assert_eq!(cloud.potion.custom_effects[0].amplifier, 0);
    assert!(super::is_breath(cloud));
    let mut ticks = 10;
    while own(&region, key).phase == phases::SITTING_FLAMING {
        assert!(ticks < 210);
        idle(&mut region);
        ticks += 1;
    }
    assert_eq!(ticks, 200);
    assert_eq!(own(&region, key).phase, phases::TAKEOFF);
    assert!(clouds(&region).is_empty());
}

// The strafe.

/// Strafing a cow 20 blocks south of it, facing it, its charge at 4: the fifth shoots
/// (level event 1017 at its block) a fireball of its own from before its head towards
/// the cow's middle, and it goes back to its holding pattern. The fireball bursts on
/// the cow: level event 2006 and a cloud of breath where the cow stands, radius 3
/// growing over 600 ticks, instant damage II, the dragon's.
#[test]
fn the_strafe_shoots_a_fireball_that_bursts_into_a_cloud_of_breath() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let target = EntityId(701);
    let cow_at = [10.5, -60.0, 30.5];
    lay(&mut region, &cow(target, cow_at));
    lay(
        &mut region,
        &dragon(key, [10.5, -58.0, 10.5], 180.0, |own| {
            own.phase = phases::STRAFE_PLAYER;
            own.phases.strafe.attack = Some(target);
            own.phases.strafe.fireball_charge = 4;
            own.parts[usize::from(parts::HEAD)] = [10.5, -59.0, 17.0];
        }),
    );
    let output = idle(&mut region);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::LevelEvent { kind: 1017, position, .. }
            if *position == BlockPos::new(10, -58, 10)
    )));
    assert_eq!(own(&region, key).phase, phases::HOLDING_PATTERN);
    let balls: Vec<Entity> = records(&region)
        .into_iter()
        .filter(|entity| entity.kind == DRAGON_FIREBALL)
        .collect();
    let [ball] = &balls[..] else {
        panic!("one fireball: {balls:?}");
    };
    assert!(ball.motion[2] > 0.0, "towards the cow: {:?}", ball.motion);
    let mut burst = false;
    for _ in 0..100 {
        let output = idle(&mut region);
        if output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::LevelEvent { kind: 2006, .. }))
        {
            burst = true;
            break;
        }
    }
    assert!(burst, "the fireball burst");
    assert!(
        records(&region)
            .iter()
            .all(|entity| entity.kind != DRAGON_FIREBALL)
    );
    let [(made, cloud)] = &clouds(&region)[..] else {
        panic!("one cloud: {:?}", clouds(&region));
    };
    assert!(near(made.position, cow_at), "{:?}", made.position);
    assert_eq!(cloud.radius.0, 3.0);
    assert_eq!(cloud.duration, 600);
    assert_eq!(cloud.radius_per_tick.0, 4.0 / 600.0);
    assert_eq!(cloud.owner, Some(key));
    assert_eq!(cloud.potion.custom_effects[0].amplifier, 1);
}

// Its death.

fn experience(region: &Region) -> i32 {
    records(region)
        .iter()
        .map(|entity| match entity.data {
            EntityData::ExperienceOrb { value, count, .. } => value * count,
            _ => 0,
        })
        .sum()
}

/// Dead in its fight in the End, never killed before: 200 ticks, the global event 1028
/// at its block at the first, 12000 experience in all (a twelfth and a half every fifth
/// tick after 150, a fifth at the last), rising 0.1 a tick, and gone at the 200th.
#[test]
fn its_death_lasts_200_ticks_with_the_global_event_1028_and_12000_experience() {
    let mut region = world(SEED, nothing);
    region.config.dimension = Dimension::End;
    let key = EntityId(700);
    let mut record = dragon(key, [10.5, -50.0, 10.5], 0.0, |own| {
        own.phase = phases::DYING
    });
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.vitals.health = 0.0;
    record.data = mob.encode();
    lay(&mut region, &record);
    region.dragon_fight = Some(Box::new(clustine_world::dragon::DragonFight {
        dragon: Some(key),
        ..Default::default()
    }));
    let output = idle(&mut region);
    let events = output.village.expect("a report").global_events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, 1028);
    assert_eq!(events[0].position, BlockPos::new(10, -50, 10));
    assert_eq!(events[0].dimension, Dimension::End);
    for _ in 1..199 {
        idle(&mut region);
    }
    assert!(region.find_entity(key).is_some(), "there after 199 ticks");
    assert!((record_of(&region, key).position[1] - (-50.0 + 19.9)).abs() < 1.0e-3);
    assert_eq!(own(&region, key).death_time, 199);
    assert_eq!(experience(&region), 9600 - 960);
    idle(&mut region);
    assert!(region.find_entity(key).is_none(), "gone at the 200th");
    assert_eq!(experience(&region), 12_000);
}

/// A dragon without a fight (here in the overworld) dies as one killed before: 500.
#[test]
fn a_dragon_without_a_fight_leaves_500_experience() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let mut record = dragon(key, [10.5, -50.0, 10.5], 0.0, |own| {
        own.phase = phases::DYING
    });
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.vitals.health = 0.0;
    record.data = mob.encode();
    lay(&mut region, &record);
    for _ in 0..200 {
        idle(&mut region);
    }
    assert!(region.find_entity(key).is_none());
    assert_eq!(experience(&region), 500);
}

// Its wings and walls.

/// Flying north with its target far ahead: the cow by its east wing is pushed east and
/// hurt by 5 (`mobAttack`); the creative player by its west wing is neither.
#[test]
fn its_wings_hurt_a_mob_by_5_and_push_it_away_and_leave_a_creative_player_be() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let at = [10.5, -60.0, 10.5];
    lay(
        &mut region,
        &dragon(key, at, 0.0, |own| {
            own.phase = phases::HOLDING_PATTERN;
            own.phases.holding.target = Some([10.5, -60.0, -39.5]);
        }),
    );
    let target = EntityId(701);
    lay(&mut region, &cow(target, [15.5, -60.0, 10.5]));
    let (id, _) = add_player(&mut region, 1, [5.5, -60.0, 10.5], false);
    idle(&mut region);
    let hit = mob_of(&region, target);
    assert_eq!(hit.body.vitals.health, 5.0);
    assert!(record_of(&region, target).motion[0] > 0.0);
    let player = region.players.get(&id).expect("player 1");
    assert_eq!(player.health, 20.0);
}

/// Its body box over stone, obsidian and end stone: the stone is broken, with level
/// event 2008 in the box; the obsidian and the end stone hold, and it is in a wall.
#[test]
fn it_breaks_stone_in_its_way_but_not_obsidian_or_end_stone_which_hold_it_in_a_wall() {
    let walls = |pos: BlockPos| -> Option<BlockState> {
        match (pos.x, pos.y, pos.z) {
            (10, -49, 10) => Some(blocks::STONE),
            (11, -49, 10) => Some(blocks::OBSIDIAN),
            (9, -49, 10) => Some(blocks::END_STONE),
            _ => None,
        }
    };
    let mut region = world(SEED, walls);
    let key = EntityId(700);
    lay(&mut region, &dragon(key, [10.5, -50.0, 10.5], 0.0, |_| {}));
    let output = idle(&mut region);
    assert_eq!(region.block(BlockPos::new(10, -49, 10)), Some(blocks::AIR));
    assert_eq!(
        region.block(BlockPos::new(11, -49, 10)),
        Some(blocks::OBSIDIAN)
    );
    assert_eq!(
        region.block(BlockPos::new(9, -49, 10)),
        Some(blocks::END_STONE)
    );
    let body = Aabb::new([8.0, -50.0, 7.0], [14.0, -46.0, 13.0]);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::LevelEvent { kind: 2008, position, .. }
            if body.contains([f64::from(position.x), f64::from(position.y), f64::from(position.z)])
    )));
    assert!(own(&region, key).in_wall);
}

// A crystal's destruction, which the fight asks of its dragon.

/// Its nearest crystal destroyed by a player hurts its head by 10 (`player_explosion`);
/// with nobody's hand in it, by 10 of `explosion`, which hurts a dragon too; another
/// crystal, nothing. The crystal stands still, so that its dragon keeps it nearest.
#[test]
fn the_destruction_of_its_nearest_crystal_hurts_its_head_by_10() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let crystal_key = EntityId(701);
    let mut record = dragon(key, [10.5, -50.0, 10.5], 0.0, |own| {
        own.nearest_crystal = Some(crystal_key);
    });
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.flags.no_ai = true;
    record.data = mob.encode();
    lay(&mut region, &record);
    lay(&mut region, &crystal(crystal_key, [20.5, -50.0, 10.5]));
    let (id, entity) = add_player(&mut region, 1, [5.5, -60.0, 5.5], false);
    let at = BlockPos::new(20, -50, 10);
    let mut output = TickOutput::default();
    region.dragon_crystal_destroyed(key, EntityId(702), at, Some((entity, id.0)), &mut output);
    assert_eq!(mob_of(&region, key).body.vitals.health, 200.0);
    region.dragon_crystal_destroyed(key, crystal_key, at, Some((entity, id.0)), &mut output);
    assert_eq!(mob_of(&region, key).body.vitals.health, 190.0);
    // Past its hurt's cooldown, the crystal healing it meanwhile.
    for _ in 0..20 {
        idle(&mut region);
    }
    let before = mob_of(&region, key).body.vitals.health;
    region.dragon_crystal_destroyed(key, crystal_key, at, None, &mut output);
    assert_eq!(mob_of(&region, key).body.vitals.health, before - 10.0);
}

/// The blow that would kill it while it flies leaves it at 1 and dying
/// (`handleKillingBlow`), and it refuses every hurt after; it flies to the podium's top
/// (0.5, -60, 0.5), within ten blocks of which its health is 0 and its death's ticks
/// count. While it sits, the same blow kills it where it is.
#[test]
fn a_killing_blow_in_flight_leaves_it_at_1_dying_at_the_podium_and_one_while_sitting_kills() {
    let mut region = world(SEED, nothing);
    let key = EntityId(700);
    let mut record = dragon(key, [10.5, -55.0, 2.5], 0.0, |own| {
        own.phase = phases::HOLDING_PATTERN;
        own.phases.holding.target = Some([10.5, -55.0, -47.5]);
        own.parts[usize::from(parts::HEAD)] = [4.5, -59.0, 2.5];
    });
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.vitals.health = 1.0;
    record.data = mob.encode();
    lay(&mut region, &record);
    let player = add_player(&mut region, 1, [2.5, -60.0, 2.5], false);
    attack(&mut region, player, 1, key, Some(parts::HEAD));
    assert_eq!(mob_of(&region, key).body.vitals.health, 1.0);
    assert_eq!(own(&region, key).phase, phases::DYING);
    let mut ticks = 0;
    while mob_of(&region, key).body.vitals.health > 0.0 {
        assert!(ticks < 100, "it reaches the podium");
        idle(&mut region);
        ticks += 1;
    }
    let at = record_of(&region, key).position;
    let d = (at[0] - 0.5).powi(2) + (at[1] + 60.0).powi(2) + (at[2] - 0.5).powi(2);
    assert!(d < 100.0, "{at:?}");
    idle(&mut region);
    assert!(own(&region, key).death_time >= 1);

    let other = EntityId(701);
    let mut record = dragon(other, [10.5, -55.0, 2.5], 0.0, |own| {
        own.phase = phases::SITTING_SCANNING;
        own.parts[usize::from(parts::HEAD)] = [4.5, -59.0, 2.5];
    });
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.vitals.health = 1.0;
    mob.body.flags.no_ai = true;
    record.data = mob.encode();
    lay(&mut region, &record);
    let player = add_player(&mut region, 2, [2.5, -60.0, 2.5], false);
    attack(&mut region, player, 1, other, Some(parts::HEAD));
    assert_eq!(mob_of(&region, other).body.vitals.health, 0.0);
    assert_eq!(own(&region, other).phase, phases::SITTING_SCANNING);
}
