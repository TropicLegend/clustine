//! Natural spawning's counts and caps in a region, tested from the plan
//! (`docs/groundwork/n9-plan.md`, sections 9.1, 9.2 and 14, and decision 7 of 18.1) by
//! someone who did not write them: what the region reports of its mobs by category
//! (what the game's `createState` counts, every loaded chunk after a restore, a chunk let
//! go as nothing), and the caps a region spawns by: nothing before the store's first
//! publication, the global cap filtered once at the phase's start, a player's local cap
//! from the others' published sums and the region's own spawns.
//!
//! **The per-chunk spawner (part S10) is being built**: the tests that need mobs to
//! spawn are written to the plan and ignored until it lands.

use clustine_data::entity_types;
use clustine_world::{Chunk, Entity, EntityData, EntityId};

use super::n9_world::*;
use super::*;
use crate::api::{CountsReport, PublishedCounts, SPAWN_CATEGORIES, SpawnCounts, WorldClock};
use crate::mobs::Mob;

const MONSTER: usize = 0;
const CREATURE: usize = 1;
const WATER_CREATURE: usize = 5;

fn counts(monsters: u32, creatures: u32, water: u32) -> SpawnCounts {
    let mut counts = [0; SPAWN_CATEGORIES];
    counts[MONSTER] = monsters;
    counts[CREATURE] = creatures;
    counts[WATER_CREATURE] = water;
    counts
}

/// A mob of the type `kind` at `feet`, with no AI; kept from despawning and from the
/// counts where `persistent`.
fn mob_at(key: u64, kind: i32, feet: [f64; 3], persistent: bool) -> Entity {
    let mut entity = super::thrown::cow(EntityId(key), feet);
    entity.kind = u16::try_from(kind).expect("a type");
    let mut mob = Mob::decode(&entity.data).expect("a mob's record");
    mob.body.flags.persistent = persistent;
    entity.data = mob.encode();
    entity
}

/// An item lying at `feet`: of the category `MISC`, never counted.
fn item_at(key: u64, feet: [f64; 3]) -> Entity {
    Entity {
        kind: u16::try_from(entity_types::ITEM).expect("a type"),
        data: EntityData::Item {
            stack: ItemStack::new(clustine_data::items::STONE, 1),
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        ..super::thrown::cow(EntityId(key), feet)
    }
}

/// The 5 by 5 chunks about the origin held by one region restored just now, a stone
/// floor in each and `entities` in theirs, its clock at `clock`; not ticked yet.
fn restored_with(entities: Vec<Entity>, clock: WorldClock) -> Region {
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let mut state = knowing_the_edges(EntityIds::block(0).expect("a block of ids"));
    state.clock = clock;
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = floor();
            chunk.set_entities(
                entities
                    .iter()
                    .filter(|entity| entity.chunk() == *position)
                    .cloned()
                    .collect(),
            );
            (*position, chunk)
        })
        .collect();
    Region::restore_loaded(
        RegionConfig {
            return_after: u64::MAX,
            seed: SEED,
            ..config()
        },
        state,
        Holdings {
            held,
            pinned: vec![ChunkArea::EVERYWHERE],
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
        &tickets,
        chunks,
        Vec::new(),
    )
}

fn reported(report: &CountsReport, chunk: ChunkPos) -> Option<SpawnCounts> {
    report
        .chunks
        .iter()
        .find(|(at, _)| *at == chunk)
        .map(|(_, counts)| *counts)
}

/// Section 9.2: a region's first tick after it is made or restored reports every loaded
/// chunk's counts whole; each counts what the game's `createState` does: a mob by its
/// category, not one kept (`isPersistenceRequired`), nothing of `MISC`.
#[test]
fn a_restored_region_reports_every_loaded_chunk_counting_what_create_state_counts() {
    let mut region = restored_with(
        vec![
            mob_at(1, entity_types::ZOMBIE, [3.5, -60.0, 3.5], false),
            mob_at(2, entity_types::ZOMBIE, [4.5, -60.0, 3.5], true),
            mob_at(3, entity_types::SKELETON, [5.5, -60.0, 3.5], false),
            mob_at(4, entity_types::COW, [6.5, -60.0, 3.5], false),
            mob_at(5, entity_types::SQUID, [7.5, -60.0, 3.5], false),
            item_at(6, [8.5, -60.0, 3.5]),
            mob_at(7, entity_types::CREEPER, [20.5, -60.0, 3.5], false),
        ],
        WorldClock::default(),
    );
    let output = region.tick(&TickInputs::default());
    let report = output.counts.expect("a report");
    assert!(report.full, "the first tick's is whole");
    assert_eq!(report.chunks.len(), 25, "every loaded chunk");
    assert_eq!(
        reported(&report, ChunkPos::new(0, 0)),
        Some(counts(2, 1, 1))
    );
    assert_eq!(
        reported(&report, ChunkPos::new(1, 0)),
        Some(counts(1, 0, 0))
    );
    assert_eq!(
        reported(&report, ChunkPos::new(-2, 2)),
        Some(counts(0, 0, 0))
    );
}

/// Section 9.2: after the first, a tick reports only the chunks whose counts changed,
/// whole, and a chunk let go as nothing; a tick that changes none reports none.
#[test]
fn a_tick_reports_the_chunks_whose_counts_changed_and_a_chunk_let_go_as_nothing() {
    let zombie = mob_at(1, entity_types::ZOMBIE, [3.5, -60.0, 3.5], false);
    let mut region = restored_with(
        vec![
            zombie.clone(),
            mob_at(2, entity_types::CREEPER, [40.5, -60.0, 40.5], false),
        ],
        WorldClock::default(),
    );
    region.tick(&TickInputs::default());
    let output = region.tick(&TickInputs::default());
    assert_eq!(output.counts, None, "nothing changed");

    // The chunk with the creeper is let go.
    let output = region.tick(&TickInputs {
        tickets_removed: vec![(ChunkPos::new(2, 2), Ticket::Viewer)],
        ..TickInputs::default()
    });
    let changed = |output: &TickOutput, chunk| {
        output
            .counts
            .as_ref()
            .and_then(|report| reported(report, chunk))
    };
    let report = output.counts.clone().expect("a report");
    assert!(!report.full);
    assert_eq!(
        report.chunks,
        [(ChunkPos::new(2, 2), counts(0, 0, 0))],
        "the chunk let go, as nothing, and no other"
    );

    // The zombie is killed by a hurt filed with the region, and goes when its death is
    // over: its chunk is reported again, with nothing in it.
    let killing = clustine_world::HurtBy {
        damage_type: "minecraft:generic".to_owned(),
        cause: None,
        direct: None,
        position: None,
        projectile_motion: None,
        amount: clustine_world::Float(1_000.0),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras: Vec::new(),
        sound: None,
        hit: None,
    };
    let mut inputs = TickInputs {
        effects: filed(
            vec![clustine_world::Effect {
                at: BlockPos::new(3, -60, 3),
                what: clustine_world::Effected::OnEntity {
                    key: zombie.key,
                    by: clustine_world::EntityUse::Hurt(Box::new(killing)),
                },
            }],
            1,
        ),
        ..TickInputs::default()
    };
    let mut zeroed = None;
    for _ in 0..40 {
        let output = region.tick(&inputs);
        inputs = TickInputs::default();
        if region.find_entity(zombie.key).is_some() {
            assert_eq!(changed(&output, ChunkPos::new(0, 0)), None, "still counted");
        } else if zeroed.is_none() {
            zeroed = changed(&output, ChunkPos::new(0, 0));
            break;
        }
    }
    assert_eq!(
        zeroed,
        Some(counts(0, 0, 0)),
        "the zombie's chunk, as it went"
    );
}

/// The monsters a tick made.
fn monsters_made(output: &TickOutput) -> usize {
    output
        .events
        .iter()
        .filter(|event| {
            matches!(event, RegionEvent::EntitySpawned(state) if matches!(
                state.kind,
                crate::api::EntityKind::Other { kind, .. } if is_monster(i32::from(kind))
            ))
        })
        .count()
}

/// Whether the entity type `kind` is of the monster category.
fn is_monster(kind: i32) -> bool {
    clustine_data::EntityTypeInfo::of(kind).category == clustine_data::MobCategory::Monster
}

/// A region alone at midnight with player 1 at the origin: dark enough for monsters on
/// the floor's stone.
fn at_midnight() -> Region {
    let mut region = alone(0, clock_at(18_000));
    enter_with(
        &mut region,
        1,
        entering_place(Vec3::new(0.5, -60.0, 0.5), Vec::new(), 0),
    );
    region
}

/// The store's publication of the others' counts: `totals` of monsters in all, and
/// `near` of them within 128 blocks of player 1.
fn published(totals: u32, near: u32) -> PublishedCounts {
    PublishedCounts {
        totals: counts(totals, 0, 0),
        players: vec![(player(1), counts(near, 0, 0))],
    }
}

/// Section 9.2: before the store's first publication of the others' counts a region
/// spawns nothing; once one comes, it spawns.
#[test]
#[ignore = "waits for S10's spawner"]
fn a_region_spawns_nothing_before_the_first_publication_and_spawns_after_it() {
    let mut region = at_midnight();
    for _ in 0..400 {
        let output = region.tick(&TickInputs::default());
        assert_eq!(monsters_made(&output), 0, "nothing before a publication");
    }
    let mut made = 0;
    let mut inputs = TickInputs {
        counts: Some(published(0, 0)),
        ..TickInputs::default()
    };
    for _ in 0..400 {
        made += monsters_made(&region.tick(&inputs));
        inputs = TickInputs::default();
    }
    assert!(made > 0, "monsters spawn once the counts are published");
}

/// Section 9.1 (finding 2): the categories are filtered by the global cap once, at the
/// start of the phase, and the region's own spawns in it do not close it again: with
/// the others at 69 monsters of a cap of 70 (one player, 289 spawnable chunks), the
/// first tick that spawns may spawn several, and the next ticks, which count them, none.
#[test]
#[ignore = "waits for S10's spawner"]
fn the_global_cap_is_filtered_once_at_the_phase_s_start_and_not_closed_by_its_own_spawns() {
    let mut region = at_midnight();
    let mut per_tick = Vec::new();
    let mut inputs = TickInputs {
        counts: Some(published(69, 0)),
        ..TickInputs::default()
    };
    for _ in 0..600 {
        per_tick.push(monsters_made(&region.tick(&inputs)));
        inputs = TickInputs::default();
    }
    let first = per_tick
        .iter()
        .position(|made| *made > 0)
        .expect("monsters spawn while the cap is open");
    assert!(
        per_tick[first] >= 2,
        "the cap stays open through the phase: {per_tick:?}"
    );
    assert!(
        per_tick[first + 1..].iter().all(|made| *made == 0),
        "closed at the next phase's start: {per_tick:?}"
    );
}

/// Section 9.2: a player's local cap is their count of the others' chunks within 128
/// blocks as published plus the region's own now, raised by its spawns in the phase
/// (`afterSpawn`). Published at 70 near the only player, nothing spawns whatever the
/// global cap; published at 66, the region spawns until its own bring them to 70, in the
/// tick it does so, and spawns nothing after.
#[test]
#[ignore = "waits for S10's spawner"]
fn a_player_s_local_cap_counts_the_others_published_sums_and_the_region_s_own_spawns() {
    let mut full = at_midnight();
    let mut inputs = TickInputs {
        counts: Some(published(0, 70)),
        ..TickInputs::default()
    };
    for _ in 0..400 {
        assert_eq!(monsters_made(&full.tick(&inputs)), 0);
        inputs = TickInputs::default();
    }

    let mut region = at_midnight();
    let mut made = 0;
    let mut inputs = TickInputs {
        counts: Some(published(0, 66)),
        ..TickInputs::default()
    };
    for _ in 0..600 {
        made += monsters_made(&region.tick(&inputs));
        inputs = TickInputs::default();
    }
    // Four reach 70; a pack begun below the cap may carry it past by its last members.
    assert!(
        (4..=7).contains(&made),
        "spawned {made} near a player published at 66"
    );
}

/// Section 9.3 and 14: a pack's members are placed only where the region may make them;
/// where a member's position is in another region's land across a pinned line, it is
/// refused there, and nothing appears in the other region's land.
#[test]
#[ignore = "waits for S10's spawner"]
fn a_pack_s_member_across_a_pinned_line_is_refused() {
    let mut region = west(0, clock_at(18_000));
    enter_with(
        &mut region,
        1,
        entering_place(Vec3::new(14.5, -60.0, 0.5), Vec::new(), 0),
    );
    let mut inputs = TickInputs {
        counts: Some(published(0, 0)),
        ..TickInputs::default()
    };
    let mut made = 0;
    for _ in 0..1_200 {
        let output = region.tick(&inputs);
        inputs = TickInputs::default();
        made += monsters_made(&output);
        for event in &output.events {
            if let RegionEvent::EntitySpawned(state) = event {
                assert!(
                    state.pose.position.x < 16.0,
                    "made in the west's land alone: {state:?}"
                );
            }
        }
        assert!(
            output.arrivals.is_empty(),
            "nothing sent across the line: {:?}",
            output.arrivals
        );
    }
    assert!(made > 0, "the west spawns in its own land");
}
