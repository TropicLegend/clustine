//! Striders in a region (d Strider.txt; the brief's section 5.9; family E4 of P12,
//! `docs/groundwork/p-plan.md`, sections 18.3, 18.5 and 18.6): one walks over a lava
//! sea to a player tempting it without sinking, here and over a pinned line; one out of
//! lava is cold; a saddled one is ridden, steered and boosted by the warped fungus on a
//! stick; and they spawn in a lava sea of the Nether's wastes. The numbers each test
//! expects are the game's rules, not what the region computes.
//!
//! The regions but the Nether's are those of the tests of redstone (`redstone.rs`):
//! player 1 at the spawn, (0.5, -60, 0.5), on a stone floor whose top is y = -60, with
//! lava laid on it.

use clustine_data::{attributes as ids, blocks, entity_types, items};
use clustine_world::{Biome, Entity};

use super::redstone::{holding, ticking_with};
use super::*;
use crate::api::{PublishedCounts, VehicleMove};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::types::strider;

const STRIDER: u16 = entity_types::STRIDER as u16;
const KEY: EntityId = EntityId(40);

/// The height a strider stands at on a lava source laid on the floor: the half block's
/// top over y = -60.
const ON_LAVA: f64 = -59.5;

/// Lava sources over `x` from `low` to `high` and z from -6 to 6 at y = -60.
fn lava_sea(low: i32, high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut sea = Vec::new();
    for x in low..=high {
        for z in -6..=6 {
            sea.push((BlockPos::new(x, -60, z), blocks::LAVA));
        }
    }
    sea
}

/// A grown strider at `position` of full health, saddled where `saddled`.
fn strider_at(key: EntityId, position: [f64; 3], saddled: bool) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.vitals.air = crate::living::MAX_AIR;
    if saddled {
        mob.body.equipment.slots[usize::from(slots::SADDLE)] =
            Some(ItemStack::new(items::SADDLE, 1));
    }
    Entity {
        portal: Default::default(),
        key,
        kind: STRIDER,
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

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("a loaded chunk")
        .apply_entity(&clustine_world::EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the strider is here").data).expect("a mob")
}

fn put_player(region: &mut Region, at: [f64; 3]) {
    let rider = region.players.get_mut(&player(1)).expect("player 1");
    rider.pose.position = Vec3::new(at[0], at[1], at[2]);
}

/// Whether the region shows the strider `key` cold (`DATA_SUFFOCATING`, index 19).
fn shown_cold(region: &Region, key: EntityId) -> bool {
    let shown = crate::shown::mob_values(STRIDER, &record(region, key).unwrap().data);
    shown
        .iter()
        .any(|(index, value, _)| *index == 19 && *value == crate::shown::DataShown::Boolean(true))
}

fn speed(region: &Region, key: EntityId) -> f64 {
    mob(region, key)
        .body
        .attributes
        .value(STRIDER, attribute(ids::MOVEMENT_SPEED))
}

/// `StriderPathNavigation` and `canStandOnFluid`: tempted by a player holding warped
/// fungus ten blocks off, a strider walks across a lava sea to its shore standing on the
/// lava's half block all the way, and is warm there.
#[test]
fn a_strider_walks_over_a_lava_sea_to_a_player_tempting_it_and_never_sinks() {
    let mut region = ticking_with(&lava_sea(-8, 8));
    lay(&mut region, &strider_at(KEY, [-1.5, ON_LAVA, 0.5], false));
    put_player(&mut region, [10.5, -60.0, 0.5]);
    checked(&mut region, &holding(items::WARPED_FUNGUS));
    let mut lowest = f64::MAX;
    let mut arrived = false;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        let strider = record(&region, KEY).expect("the strider is here");
        if strider.position[0] < 8.0 {
            lowest = lowest.min(strider.position[1]);
            assert!(!shown_cold(&region, KEY), "warm on lava");
        }
        if strider.position[0] > 6.5 {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "it walked to the shore");
    assert_eq!(lowest, ON_LAVA, "on the half block all the way");
}

/// `Strider.tick`'s warmth: on stone out of lava it is cold, shown so and 34% slower
/// (`suffocating`, −0.34 of its base); back on lava it is warm and its speed whole.
#[test]
fn a_strider_on_stone_is_cold_and_slower_and_warm_again_on_lava() {
    let mut region = ticking_with(&lava_sea(-8, 0));
    lay(&mut region, &strider_at(KEY, [6.5, -60.0, 0.5], false));
    checked(&mut region, &TickInputs::default());
    assert!(shown_cold(&region, KEY));
    let base = 0.174_999_997_019_767_76;
    assert_eq!(speed(&region, KEY), base + base * -0.340_000_003_576_278_7);
    // Laid on the lava, it is warm at its next tick.
    let mut warm = record(&region, KEY).unwrap();
    warm.position = [-3.5, ON_LAVA, 0.5];
    lay(&mut region, &warm);
    region.entity_chunks.insert(KEY, warm.chunk());
    checked(&mut region, &TickInputs::default());
    assert!(!shown_cold(&region, KEY));
    assert_eq!(speed(&region, KEY), base);
}

/// `floatStrider`: a strider deep in lava, whose travel is the air's there (it stands on
/// the lava at its feet), rises through it, its motion halved and 0.05 up each tick it
/// has lava over its block or is not above the half block, until it stands on the top source's half
/// block, where it stays.
#[test]
fn a_strider_deep_in_lava_rises_to_stand_on_its_top() {
    let mut deep = Vec::new();
    for y in -60..=-56 {
        for x in -4..=4 {
            for z in -4..=4 {
                deep.push((BlockPos::new(x, y, z), blocks::LAVA));
            }
        }
    }
    let mut region = ticking_with(&deep);
    let mut sunk = strider_at(KEY, [0.5, -60.0, 0.5], false);
    sunk.on_ground = false;
    lay(&mut region, &sunk);
    let top = -55.5;
    let mut rose = false;
    // Some 0.021 a tick at its steadiest: `((v - 0.08) × 0.98) × 0.5 + 0.05`.
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        if record(&region, KEY).unwrap().position[1] == top {
            rose = true;
            break;
        }
    }
    assert!(rose, "{:?}", record(&region, KEY).unwrap().position);
    for _ in 0..40 {
        checked(&mut region, &TickInputs::default());
        let strider = record(&region, KEY).unwrap();
        assert_eq!(strider.position[1], top);
        assert!(strider.on_ground);
    }
}

// ---------------------------------------------------------------------------------
// Over a pinned line.

/// A region pinned west of x = 32 holding chunks -1 to 1 each way, the copies of chunk
/// x = 2 beyond the line, both with `blocks`, and `own` laid in it.
fn at_a_line(blocks: &[(BlockPos, BlockState)], own: &[Entity]) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let set = |chunk: &mut Chunk, position: ChunkPos| {
        for (at, state) in blocks {
            if at.chunk() == position {
                chunk.set(
                    at.x.rem_euclid(16) as usize,
                    at.y,
                    at.z.rem_euclid(16) as usize,
                    *state,
                );
            }
        }
    };
    let chunks = around
        .iter()
        .map(|position| {
            let mut chunk = floor();
            set(&mut chunk, *position);
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    for entity in own {
        lay(&mut region, entity);
    }
    let copies: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    let bases = copies
        .iter()
        .map(|position| {
            let mut base = floor();
            set(&mut base, *position);
            (*position, base)
        })
        .collect();
    region.tick(&TickInputs {
        foreign: copies
            .iter()
            .map(|position| (*position, RegionId(1)))
            .collect(),
        copy_bases: bases,
        ..TickInputs::default()
    });
    region
}

/// A player of the other region at `x` beyond the line, holding `held`.
fn face_holding(x: f64, held: i32) -> crate::api::FaceChange {
    let mut face = crate::api::PlayerFace::of(
        &crate::api::Pose::at(Vec3::new(x, -60.0, 8.5)),
        false,
        false,
        &Default::default(),
        0,
        None,
    );
    face.main_hand = Some(ItemStack::new(held, 1));
    crate::api::FaceChange::Face {
        player: player(9),
        entity: EntityId(900),
        hops: 0,
        seq: crate::api::FaceSeq(1, 0),
        face,
    }
}

/// Lava from x 16 to 47 and z 2 to 14 at y = -60, on both sides of the line.
fn lava_across() -> Vec<(BlockPos, BlockState)> {
    let mut sea = Vec::new();
    for x in 16..48 {
        for z in 2..15 {
            sea.push((BlockPos::new(x, -60, z), blocks::LAVA));
        }
    }
    sea
}

/// Tempted by a player of the other region beyond the line, a strider walks over the
/// lava across the line and is sent standing on the lava's half block; taken in a region
/// of lava, it stands on it there.
#[test]
fn a_strider_walks_over_lava_across_a_pinned_line_and_arrives_standing_on_it() {
    let mut region = at_a_line(
        &lava_across(),
        &[strider_at(KEY, [29.5, ON_LAVA, 8.5], false)],
    );
    checked(
        &mut region,
        &TickInputs {
            faces: vec![face_holding(37.5, items::WARPED_FUNGUS)],
            ..TickInputs::default()
        },
    );
    let mut sent = None;
    for _ in 0..600 {
        let output = checked(&mut region, &TickInputs::default());
        if let Some((_, entity)) = output.arrivals.iter().find(|(_, entity)| entity.key == KEY) {
            sent = Some(entity.clone());
            break;
        }
        let here = record(&region, KEY).expect("here until it is sent");
        assert_eq!(here.position[1], ON_LAVA, "it never sinks");
    }
    let sent = sent.expect("sent over the line");
    assert!(sent.position[0] >= 32.0);
    assert_eq!(sent.position[1], ON_LAVA);
    // Taken where it lands on lava again, it stands on it.
    let mut beyond = ticking_with(&lava_sea(-8, 8));
    let mut arriving = sent.clone();
    arriving.position[0] -= 32.0;
    arriving.position[2] -= 8.0;
    let output = checked(
        &mut beyond,
        &TickInputs {
            arrivals: vec![clustine_world::Arrival {
                id: clustine_world::EffectId {
                    sequence: 7,
                    index: 0,
                },
                chunk: arriving.chunk(),
                entity: arriving,
                due: clustine_world::Due::FromTaking,
            }],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.arrivals_taken.len(), 1);
    for _ in 0..40 {
        checked(&mut beyond, &TickInputs::default());
        let strider = record(&beyond, KEY).expect("taken");
        assert_eq!(strider.position[1], ON_LAVA);
        assert!(!shown_cold(&beyond, KEY));
    }
}

// ---------------------------------------------------------------------------------
// Riding.

fn interact(key: EntityId) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: key,
            location: [0.0, 0.5, 0.0],
            sneaking: false,
        },
    )])
}

fn use_stick() -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::UseItem {
            rotation: (0.0, 0.0),
            sneaking: false,
            sequence: 1,
        },
    )])
}

fn move_vehicle(position: [f64; 3], yaw: f32) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::MoveVehicle(VehicleMove {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw,
            pitch: 0.0,
            on_ground: true,
        }),
    )])
}

fn riding(region: &Region) -> Option<EntityId> {
    region.players[&player(1)]
        .riding
        .map(|riding| riding.vehicle)
}

fn corrected(output: &TickOutput) -> bool {
    output
        .player_events
        .iter()
        .any(|(_, event)| matches!(event, PlayerEvent::VehicleCorrected { .. }))
}

/// `Strider.getControllingPassenger`, `tickRidden` and `ItemBasedSteering`: a saddled
/// strider on lava takes on a player who clicks it; holding a warped fungus on a stick
/// they drive it over the lava where their client moves it, it turns to their way, and
/// a use of the stick boosts it, shown at index 18; with the stick put away it is its
/// own again and their move does not move it.
#[test]
fn a_saddled_strider_is_ridden_over_lava_steered_and_boosted_by_the_warped_fungus_on_a_stick() {
    let mut region = ticking_with(&lava_sea(-8, 8));
    lay(&mut region, &strider_at(KEY, [2.5, ON_LAVA, 0.5], true));
    put_player(&mut region, [1.5, -60.0, 0.5]);
    checked(&mut region, &holding(items::WARPED_FUNGUS_ON_A_STICK));
    checked(&mut region, &interact(KEY));
    assert_eq!(riding(&region), Some(KEY));
    let mut to = [2.5, ON_LAVA, 0.5];
    for step in 0..4 {
        to = [to[0] + 0.25, ON_LAVA, to[2] + 0.125];
        let output = checked(&mut region, &move_vehicle(to, 270.0));
        assert!(
            !corrected(&output),
            "step {step}: {:?}",
            output.player_events
        );
        assert_eq!(
            record(&region, KEY).unwrap().position.map(f64::to_bits),
            to.map(f64::to_bits)
        );
    }
    {
        let rider = region.players.get_mut(&player(1)).expect("player 1");
        rider.pose.yaw = 90.0;
        rider.pose.pitch = 30.0;
    }
    idle(&mut region);
    let driven = record(&region, KEY).unwrap();
    assert_eq!((driven.yaw, driven.pitch), (90.0, 15.0));
    assert_eq!(driven.motion, [0.0; 3]);

    checked(&mut region, &use_stick());
    let boost = strider::state_of(&mob(&region, KEY)).boost;
    assert!(boost.boosting);
    assert!((140..=980).contains(&boost.total), "{boost:?}");
    let shown = crate::shown::mob_values(STRIDER, &record(&region, KEY).unwrap().data);
    assert!(
        shown.iter().any(|(index, value, default)| *index == 18
            && *value == crate::shown::DataShown::Int(boost.total)
            && !default),
        "{shown:?}"
    );
    checked(&mut region, &use_stick());
    assert_eq!(
        strider::state_of(&mob(&region, KEY)).boost.total,
        boost.total,
        "refused while it boosts"
    );

    // A carrot on a stick steers no strider.
    checked(&mut region, &holding(items::CARROT_ON_A_STICK));
    let away = [to[0] + 1.0, ON_LAVA, to[2]];
    checked(&mut region, &move_vehicle(away, 270.0));
    assert_ne!(
        record(&region, KEY).unwrap().position.map(f64::to_bits),
        away.map(f64::to_bits)
    );
}

// ---------------------------------------------------------------------------------
// Spawning in the Nether.

/// The Nether's wastes' lava sea: netherrack to y 20, lava from 21 to 31, air above.
const SEA_TOP: i32 = 31;

fn nether_chunk(biome: Biome) -> Chunk {
    let mut chunk = Chunk::empty(clustine_world::Dimension::Nether.rules(), biome);
    for z in 0..16 {
        for x in 0..16 {
            for y in 0..=20 {
                chunk.set(x, y, z, blocks::NETHERRACK);
            }
            for y in 21..=SEA_TOP {
                chunk.set(x, y, z, blocks::LAVA);
            }
        }
    }
    chunk
}

/// A Nether region of the 5×5 chunks about the origin's, all held and loaded, the middle
/// 3×3 ticking, a lava sea of the wastes; ticked once.
fn nether_sea() -> Region {
    let biome = crate::spawning::rules::biome_named("minecraft:nether_wastes").expect("a biome");
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| (*position, nether_chunk(biome)))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held,
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let rules = clustine_world::Dimension::Nether.rules();
    let config = RegionConfig {
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension: clustine_world::Dimension::Nether,
        spawn: Some(Vec3::new(0.5, f64::from(SEA_TOP + 1), 0.5)),
        starting_inventory: Default::default(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 0x571d,
        game_seed: 0,
        sea_level: 32,
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = crate::state::RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, Vec::new());
    region.tick(&TickInputs::default());
    region
}

/// The mobs a tick made, by their records as first committed.
fn made(output: &TickOutput) -> Vec<Entity> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            clustine_world::EntityChange::Whole(entity)
                if matches!(entity.data, clustine_world::EntityData::Mob { .. }) =>
            {
                Some(entity.clone())
            }
            _ => None,
        })
        .collect()
}

/// `NaturalSpawner` in the Nether's wastes, whose creatures are the striders alone:
/// on the ticks of the creatures (game time a multiple of 400) striders spawn in the lava
/// sea (`IN_LAVA`, `checkStriderSpawnRules`: up through the lava to the air), each in a
/// lava block of it; a rider made with one rides it.
#[test]
fn striders_spawn_in_a_lava_sea_of_the_nether_s_wastes() {
    let mut region = nether_sea();
    super::super::tnt::tests::add_player(
        &mut region,
        1,
        [30.5, f64::from(SEA_TOP) + 0.5, 0.5],
        true,
    );
    let mut spawned = Vec::new();
    for round in 1..=60_u64 {
        region.clock = crate::api::WorldClock {
            game_time: round * 400 - 1,
            day: 18_000 + round,
        };
        let output = region.tick(&TickInputs {
            counts: Some(PublishedCounts::default()),
            ..TickInputs::default()
        });
        spawned.extend(made(&output));
        if spawned.iter().filter(|mob| mob.kind == STRIDER).count() >= 4 {
            break;
        }
    }
    let striders: Vec<&Entity> = spawned.iter().filter(|mob| mob.kind == STRIDER).collect();
    assert!(striders.len() >= 4, "striders came: {}", striders.len());
    for mob in &spawned {
        let at = crate::physics::containing(mob.position);
        if mob.kind == STRIDER {
            assert_eq!(
                region.block(at).map(|state| state.block_id()),
                Some(blocks::LAVA.block_id()),
                "in the lava: {:?}",
                mob.position
            );
            assert!((21..=SEA_TOP).contains(&at.y), "{:?}", mob.position);
        } else {
            // A jockey's zombified piglin.
            assert_eq!(i32::from(mob.kind), entity_types::ZOMBIFIED_PIGLIN);
        }
    }
}
