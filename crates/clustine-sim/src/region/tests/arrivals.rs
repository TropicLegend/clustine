//! Tests of entities crossing a line (step N3d, `docs/groundwork/n3-plan.md`, with its
//! sections 5 and 6; `docs/adr/0028-entities.md`, section 5): an entity whose move ends
//! in a chunk the region does not have loaded is sent, `Gone` here and whole in the
//! tick's arrivals with the `hops` it had, and reported handed on; a throw there is
//! sent as it is made; arrivals handed in are taken with one more hop and tick in the
//! tick that takes them; a chunk an arrival waits for is loaded with an arrival ticket,
//! and a chunk held for an arrival alone is given back when its ticket ends.
//!
//! Player 1 stands at the spawn, (0.5, -60, 0.5), on a stone floor at y = -61. The
//! items are never picked up.

use clustine_world::{
    Arrival, Due, EffectId, Entity, EntityChange, EntityData, MAX_CHUNK_ENTITIES,
};

use super::*;

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);

/// The game's `0.98f`, widened: the air's drag.
fn drag() -> f64 {
    f64::from(0.98_f32)
}

/// An item of dirt at `position` with `motion`, on the ground or not, with `hops`
/// changes of holder behind it, never picked up.
fn item(key: u64, position: [f64; 3], motion: [f64; 3], on_ground: bool, hops: u32) -> Entity {
    Entity {
        key: EntityId((1 << 40) + key),
        kind: clustine_data::entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key)),
        position,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
        hops,
        age: 1,
        data: EntityData::Item {
            stack: DIRT,
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// Lays `entity` in its loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// The record of the entity `key`, wherever the region has it.
fn record(region: &Region, key: EntityId) -> Option<Entity> {
    let chunk = region.entity_chunks.get(&key)?;
    region
        .chunk(*chunk)?
        .entities()
        .iter()
        .find(|entity| entity.key == key)
        .cloned()
}

/// Whether any loaded chunk of `region` has an entity of `key`.
fn anywhere(region: &Region, key: EntityId) -> bool {
    region
        .chunks
        .values()
        .any(|chunk| chunk.entities().iter().any(|entity| entity.key == key))
}

/// What the edges are told of the entity `key` in `output`, beyond block changes.
fn reports_of(output: &TickOutput, key: EntityId) -> Vec<RegionEvent> {
    output
        .events
        .iter()
        .filter(|event| match event {
            RegionEvent::EntityTracked(tracked) => tracked.entity == key,
            RegionEvent::EntitySpawned(state) => state.entity == key,
            RegionEvent::EntityGone { entity, .. } | RegionEvent::HandedOn { entity, .. } => {
                *entity == key
            }
            _ => false,
        })
        .cloned()
        .collect()
}

/// A region pinned to `area`, which player 1 has joined, holding chunks x = -1 to 0
/// (z = -1 to 1) loaded, with `blocks` in chunk (0, 0), and copies of the column of
/// chunks at x = 1 beyond: the origin ticks, and nothing at x = 1 is loaded. Pinned
/// everywhere, the region holds the chunks at x = 1 and has not loaded them; pinned
/// west of x = 1, it knows them another region's.
pub(super) fn at_a_line(area: ChunkArea, blocks: &[(BlockPos, BlockState)]) -> Region {
    let mut region = joined_in(area, &[1]);
    let ours: Vec<ChunkPos> = (-1..=0)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    checked(&mut region, &tickets(ours.clone(), vec![]));
    let chunks = ours
        .iter()
        .map(|position| {
            let mut chunk = floor();
            if *position == ORIGIN {
                for (at, state) in blocks {
                    chunk.set(at.x as usize, at.y, at.z as usize, *state);
                }
            }
            (*position, chunk)
        })
        .collect();
    let copies = (-1..=1).map(|z| (ChunkPos::new(1, z), floor())).collect();
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks,
            copy_bases: copies,
            ..TickInputs::default()
        },
    );
    assert!(region.ticks_chunk(ORIGIN));
    assert!(!region.chunks.contains_key(&EAST));
    region
}

/// Pinned west of chunk x = 1.
const WEST_OF_THE_LINE: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};

/// The arrival numbered `index` of `entity` into its chunk.
fn arrival(index: u32, entity: &Entity) -> Arrival {
    Arrival {
        id: EffectId { sequence: 7, index },
        chunk: entity.chunk(),
        entity: entity.clone(),
        due: Due::FromTaking,
    }
}

fn arriving(arrivals: Vec<Arrival>) -> TickInputs {
    TickInputs {
        arrivals,
        ..TickInputs::default()
    }
}

#[test]
fn an_item_that_slides_across_a_line_is_gone_here_and_in_the_arrivals_whole_with_its_hops() {
    let mut region = at_a_line(ChunkArea::EVERYWHERE, &[]);
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.0], true, 3);
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    // It moved on as it would within the region: by its motion, the ground's friction
    // taking from it after the move.
    let [(to, sent)] = &output.arrivals[..] else {
        panic!("{:?}", output.arrivals);
    };
    assert_eq!(*to, EAST);
    assert_eq!(sent.key, sliding.key);
    assert_eq!(sent.hops, 3, "the hops it had: the taking raises them");
    assert_eq!(sent.position[0].to_bits(), (15.95 + 0.2_f64).to_bits());
    let ground = f64::from(0.98_f32 * 0.6);
    assert_eq!(sent.motion[0].to_bits(), (0.2 * ground).to_bits());
    assert_eq!(sent.data, sliding.data);
    // Its chunk commits it gone, and nothing else of it.
    assert_eq!(
        output.entity_changes,
        [(ORIGIN, EntityChange::Gone(sliding.key))]
    );
    assert_eq!(
        reports_of(&output, sliding.key),
        [RegionEvent::HandedOn {
            entity: sliding.key,
            chunk: ORIGIN,
            hops: 3,
            position: sent.position,
            motion: sent.motion,
        }]
    );
    assert!(!anywhere(&region, sliding.key));
    assert!(!region.entity_chunks.contains_key(&sliding.key));
    assert!(!region.entity_memory.contains_key(&sliding.key));
    assert!(region.copy(EAST).unwrap().entities().is_empty());
    // It is sent once.
    for _ in 0..20 {
        let output = idle(&mut region);
        assert!(output.arrivals.is_empty());
        assert!(reports_of(&output, sliding.key).is_empty());
    }
}

#[test]
fn an_item_sent_into_another_regions_chunk_counts_at_that_line_for_half_a_minute() {
    let mut region = at_a_line(WEST_OF_THE_LINE, &[]);
    let Knowledge::Foreign(neighbour) = region.knowledge(EAST) else {
        panic!("{:?}", region.knowledge(EAST));
    };
    assert!(region.entities_at_line().is_empty());
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.0], true, 0);
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    assert_eq!(output.arrivals.len(), 1);
    // Sent at x = 16.15, z = 8.5: the chunks within four blocks are those on either
    // side of the line, by the region it went to.
    assert_eq!(
        region.entities_at_line(),
        [(ORIGIN, Some(neighbour)), (EAST, Some(neighbour))]
    );
    for _ in 1..600 {
        idle(&mut region);
        assert_eq!(region.entities_at_line().len(), 2);
    }
    idle(&mut region);
    assert!(region.entities_at_line().is_empty());
}

#[test]
fn a_falling_block_that_falls_across_a_line_is_sent_and_does_not_land_here() {
    let mut region = at_a_line(ChunkArea::EVERYWHERE, &[]);
    let sand = Entity {
        kind: clustine_data::entity_types::FALLING_BLOCK as u16,
        data: EntityData::FallingBlock {
            state: blocks::SAND.0,
            time: 3,
            drop_item: true,
            cancel_drop: false,
        },
        ..item(2, [15.6, -59.0, 8.5], [0.5, -0.2, 0.0], false, 1)
    };
    lay(&mut region, &sand);
    let output = idle(&mut region);
    let [(to, sent)] = &output.arrivals[..] else {
        panic!("{:?}", output.arrivals);
    };
    assert_eq!(*to, EAST);
    // Gravity, the move, then the air's drag, as its tick would leave it; it has
    // fallen a tick more.
    let fall: f64 = -0.2 - 0.04;
    assert_eq!(sent.position[0].to_bits(), (15.6 + 0.5_f64).to_bits());
    assert_eq!(sent.position[1].to_bits(), (-59.0 + fall).to_bits());
    assert_eq!(sent.motion[1].to_bits(), (fall * drag()).to_bits());
    assert_eq!(
        sent.data,
        EntityData::FallingBlock {
            state: blocks::SAND.0,
            time: 4,
            drop_item: true,
            cancel_drop: false,
        }
    );
    assert_eq!((sent.hops, sent.age), (1, 2));
    assert_eq!(
        output.entity_changes,
        [(ORIGIN, EntityChange::Gone(sand.key))]
    );
    assert!(matches!(
        &reports_of(&output, sand.key)[..],
        [RegionEvent::HandedOn {
            hops: 1,
            chunk: ORIGIN,
            ..
        }]
    ));
    // Nothing of it lands in the region, now or later.
    for _ in 0..40 {
        let output = idle(&mut region);
        assert!(output.events.iter().all(|event| !matches!(
            event,
            RegionEvent::TransientBlock { .. } | RegionEvent::BlockChanged { .. }
        )));
    }
    assert!(!anywhere(&region, sand.key));
}

#[test]
fn an_item_that_slides_into_a_loaded_chunk_that_does_not_tick_moves_in_and_stands_still() {
    // Replaces N4's test that an item stops at the edge of the chunks the region ticks
    // (`item_physics.rs`): the plan's section 5 has it move into a loaded chunk of the
    // region's own and stop ticking, as the game's entities do.
    let mut region = on_ticking_floor_with(&[]);
    assert!(!region.ticks_chunk(EAST) && region.chunks.contains_key(&EAST));
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.1], true, 0);
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    let [
        (from, EntityChange::Gone(gone)),
        (to, EntityChange::Whole(whole)),
    ] = &output.entity_changes[..]
    else {
        panic!("{:?}", output.entity_changes);
    };
    assert_eq!((*from, *gone, *to), (ORIGIN, sliding.key, EAST));
    assert_eq!(whole.position[0].to_bits(), (15.95 + 0.2_f64).to_bits());
    assert_eq!(whole.position[2].to_bits(), (8.5 + 0.1_f64).to_bits());
    assert!(output.arrivals.is_empty());
    assert_eq!(region.entity_chunks[&sliding.key], EAST);
    let moved_in = record(&region, sliding.key).unwrap();
    for _ in 0..20 {
        let output = idle(&mut region);
        assert!(output.entity_changes.is_empty() && output.arrivals.is_empty());
    }
    assert_eq!(record(&region, sliding.key).unwrap(), moved_in);
}

#[test]
fn the_guard_holds_an_item_back_from_a_full_chunk_of_the_regions_own() {
    let mut region = on_ticking_floor_with(&[]);
    let crowd: Vec<Entity> = (0..MAX_CHUNK_ENTITIES as u64)
        .map(|n| item(100 + n, [24.5, -60.0, 8.5], [0.0; 3], true, 0))
        .collect();
    region.chunks.get_mut(&EAST).unwrap().set_entities(crowd);
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.1], true, 0);
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    let stopped = record(&region, sliding.key).unwrap();
    // It keeps its x and loses its motion along x, and goes on along z.
    assert_eq!(stopped.position[0], 15.95);
    assert_eq!(stopped.motion[0], 0.0);
    assert_eq!(stopped.position[2].to_bits(), (8.5 + 0.1_f64).to_bits());
    assert_eq!(region.entity_chunks[&sliding.key], ORIGIN);
    assert!(output.arrivals.is_empty());
    for _ in 0..20 {
        assert!(idle(&mut region).arrivals.is_empty());
    }
    assert_eq!(record(&region, sliding.key).unwrap().position[0], 15.95);
}

#[test]
fn taking_an_arrival_adds_it_with_one_more_hop_and_ticks_it_in_that_tick() {
    let mut region = on_ticking_floor_with(&[]);
    let falling = item(1, [8.5, -58.0, 8.5], [0.0; 3], false, 2);
    let output = checked(&mut region, &arriving(vec![arrival(4, &falling)]));
    assert_eq!(output.arrivals_taken, [arrival(4, &falling).id]);
    // Whole with one more hop, as it came, then its tick's move in the same tick.
    let raised = Entity {
        hops: 3,
        ..falling.clone()
    };
    assert_eq!(
        output.entity_changes.first(),
        Some(&(ORIGIN, EntityChange::Whole(raised.clone())))
    );
    let now = record(&region, falling.key).unwrap();
    assert_eq!(now.hops, 3);
    assert_eq!(now.position[1].to_bits(), (-58.0 - 0.04_f64).to_bits());
    assert!(
        output
            .entity_changes
            .iter()
            .any(|(chunk, change)| *chunk == ORIGIN
                && matches!(change, EntityChange::Set(key, _) if *key == falling.key))
    );
    // The edges hear of it spawned before its move.
    let reports = reports_of(&output, falling.key);
    let [
        RegionEvent::EntitySpawned(spawned),
        RegionEvent::EntityTracked(moved),
    ] = &reports[..]
    else {
        panic!("{reports:?}");
    };
    assert_eq!(spawned.entity, falling.key);
    assert!(matches!(spawned.kind, EntityKind::Other { hops: 3, .. }));
    assert_eq!(moved.hops, 3);
    assert_eq!(moved.position, now.position);
}

#[test]
fn an_arrival_is_taken_into_a_full_chunk_all_the_same() {
    let mut region = on_ticking_floor_with(&[]);
    let crowd: Vec<Entity> = (0..MAX_CHUNK_ENTITIES as u64)
        .map(|n| item(100 + n, [4.5, -60.0, 4.5], [0.0; 3], true, 0))
        .collect();
    region.chunks.get_mut(&ORIGIN).unwrap().set_entities(crowd);
    let coming = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, 0);
    let output = checked(&mut region, &arriving(vec![arrival(0, &coming)]));
    assert_eq!(output.arrivals_taken.len(), 1);
    assert_eq!(
        region.chunk(ORIGIN).unwrap().entities().len(),
        MAX_CHUNK_ENTITIES + 1
    );
    assert_eq!(record(&region, coming.key).unwrap().hops, 1);
}

#[test]
fn an_arrival_whose_key_the_region_has_already_is_counted_taken_and_its_own_entity_kept() {
    let mut region = on_ticking_floor_with(&[]);
    let own = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, 4);
    lay(&mut region, &own);
    let other = Entity {
        position: [4.5, -60.0, 4.5],
        hops: 9,
        ..own.clone()
    };
    let output = checked(&mut region, &arriving(vec![arrival(2, &other)]));
    assert_eq!(output.arrivals_taken, [arrival(2, &other).id]);
    assert!(
        output
            .entity_changes
            .iter()
            .all(|(_, change)| !matches!(change, EntityChange::Whole(_)))
    );
    assert!(
        reports_of(&output, own.key)
            .iter()
            .all(|event| !matches!(event, RegionEvent::EntitySpawned(_)))
    );
    let kept = record(&region, own.key).unwrap();
    assert_eq!((kept.position, kept.hops), (own.position, 4));
    assert_eq!(region.chunk(ORIGIN).unwrap().entities().len(), 1);
}

#[test]
fn an_arrival_into_a_chunk_not_loaded_waits_and_its_ticket_loads_the_chunk_until_it_is_taken() {
    // The region holds (2, 0), pinned everywhere, and has not loaded it.
    let mut region = on_ticking_floor_with(&[]);
    let far = ChunkPos::new(2, 0);
    assert_eq!(region.knowledge(far), Knowledge::Held);
    let coming = item(1, [40.5, -60.0, 8.5], [0.0; 3], true, 5);
    let handed = arrival(1, &coming);
    // Handed in before its chunk is loaded, it is not taken.
    let output = checked(&mut region, &arriving(vec![handed.clone()]));
    assert!(output.arrivals_taken.is_empty() && output.entity_changes.is_empty());
    assert!(output.chunk_requests.is_empty());
    // Waiting for it, the chunk is ticketed and asked of storage, once.
    let waiting = TickInputs {
        arrivals_waiting: vec![far],
        ..TickInputs::default()
    };
    let output = checked(&mut region, &waiting);
    assert_eq!(output.chunk_requests, [far]);
    assert!(checked(&mut region, &waiting).chunk_requests.is_empty());
    let output = checked(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(far, floor())],
            arrivals_waiting: vec![far],
            ..TickInputs::default()
        },
    );
    assert!(output.arrivals_taken.is_empty());
    assert!(region.chunks.contains_key(&far));
    let output = checked(&mut region, &arriving(vec![handed.clone()]));
    assert_eq!(output.arrivals_taken, [handed.id]);
    assert_eq!(record(&region, coming.key).unwrap().hops, 6);
    // The ticket ends a hundred ticks after the taking: the chunk is let go of then,
    // saved as it is, and dropped at the start of the next tick. The region keeps
    // what it is pinned to, and gives nothing back.
    for tick in 0..100 {
        let output = idle(&mut region);
        assert!(region.letting_go().is_empty(), "tick {tick}");
        assert!(output.returns.is_empty());
        assert!(region.chunks.contains_key(&far), "tick {tick}");
    }
    let output = idle(&mut region);
    assert_eq!(region.letting_go(), [far]);
    assert!(output.returns.is_empty());
    idle(&mut region);
    assert!(!region.chunks.contains_key(&far));
    assert_eq!(region.knowledge(far), Knowledge::Held);
}

#[test]
fn an_arrival_ticket_on_pinned_land_not_held_claims_the_chunk() {
    let mut region = asking(&[], &[ChunkArea::EVERYWHERE]);
    let far = ChunkPos::new(5, 5);
    assert_eq!(region.knowledge(far), Knowledge::Unknown);
    let output = checked(
        &mut region,
        &TickInputs {
            arrivals_waiting: vec![far],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.claims, [far]);
    let output = checked(
        &mut region,
        &TickInputs {
            granted: vec![far],
            arrivals_waiting: vec![far],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, [far]);
}

#[test]
fn a_chunk_held_only_for_an_arrival_is_given_back_when_its_ticket_ends() {
    let config = RegionConfig {
        return_after: 10_000,
        ..config()
    };
    let mut region = asking_with(config, &[], &[]);
    let granted = ChunkPos::new(5, 5);
    // The store grants the chunk with the arrival into it, unasked.
    let output = checked(
        &mut region,
        &TickInputs {
            granted: vec![granted],
            arrivals_waiting: vec![granted],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, [granted]);
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(granted, floor())],
            arrivals_waiting: vec![granted],
            ..TickInputs::default()
        },
    );
    let coming = item(1, [88.5, -60.0, 88.5], [0.0; 3], true, 0);
    let output = checked(&mut region, &arriving(vec![arrival(0, &coming)]));
    assert_eq!(output.arrivals_taken.len(), 1);
    for tick in 0..100 {
        assert!(idle(&mut region).returns.is_empty(), "tick {tick}");
    }
    // Given back at once, not after the ten thousand ticks a chunk is otherwise kept
    // unused: saved as it is, and dropped at the start of the next tick.
    let output = idle(&mut region);
    assert_eq!(output.returns, [granted]);
    assert_eq!(region.letting_go(), [granted]);
    assert_eq!(region.knowledge(granted), Knowledge::Unknown);
    idle(&mut region);
    assert!(!region.chunks.contains_key(&granted));
}

#[test]
fn a_throw_into_another_regions_chunk_is_sent_as_it_is_made() {
    // East of the pinned line, beyond the chunks the region has been told of: a player
    // who walks there stays until the store answers who holds it, and what they throw
    // meanwhile goes as an arrival into that chunk, which the store gives its holder.
    // (A player who stands in a chunk the region knows another's is let go, and what
    // they do there is the other region's to do.)
    let mut region = at_a_line(WEST_OF_THE_LINE, &[]);
    let beyond = ChunkPos::new(6, 0);
    assert_eq!(region.knowledge(beyond), Knowledge::Unknown);
    let output = checked(
        &mut region,
        &moves(vec![
            walk(1, 100.5, 8.5),
            numbered(
                1,
                PlayerInput::Drop {
                    all: false,
                    crouching: false,
                },
            ),
        ]),
    );
    let [(to, thrown)] = &output.arrivals[..] else {
        panic!("{:?}", output.arrivals);
    };
    assert_eq!(*to, beyond);
    assert_eq!(thrown.hops, 0);
    assert_eq!(thrown.chunk(), beyond);
    let EntityData::Item { stack, thrower, .. } = thrown.data else {
        panic!("{thrown:?}");
    };
    assert_eq!(stack.count, 1);
    assert_eq!(thrower, Some(player(1).0));
    // It appears where it is taken: here it is neither committed nor shown, and nothing
    // is said of it being handed on.
    assert!(
        output
            .entity_changes
            .iter()
            .all(|(_, change)| !matches!(change, EntityChange::Whole(_)))
    );
    assert!(reports_of(&output, thrown.key).is_empty());
    // The slot is not put back, and the arm swings.
    assert!(
        !told(&output)
            .iter()
            .any(|(_, event)| matches!(event, PlayerEvent::InventorySlot { slot: 0, .. }))
    );
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::ArmSwung { .. }))
    );
    assert_eq!(output.claims, [beyond]);
}

#[test]
fn a_region_and_its_restore_agree_tick_for_tick_through_a_sending_and_a_taking() {
    let mut original = at_a_line(ChunkArea::EVERYWHERE, &[]);
    let sliding = item(1, [15.0, -60.0, 8.5], [0.5, 0.0, 0.0], true, 0);
    let lying = item(2, [8.5, -60.0, 4.5], [0.0; 3], true, 0);
    for entity in [&sliding, &lying] {
        lay(&mut original, entity);
    }
    let coming = item(3, [12.5, -57.0, 12.5], [-0.1, 0.0, 0.0], false, 1);
    // The arrival is handed in at the eighth tick, and again at the ninth, as a runner
    // that has not yet seen it taken might.
    let inputs = |tick: u64| {
        if tick == 8 || tick == 9 {
            arriving(vec![arrival(0, &coming)])
        } else {
            TickInputs::default()
        }
    };
    let mut sent_at = None;
    let mut taken_at = None;
    for restore_after in [1, 3, 6, 8, 10] {
        let mut region = original.clone();
        for tick in 0..restore_after {
            checked(&mut region, &inputs(tick));
        }
        let mut copy = super::scheduled_ticks::restored(&region);
        for tick in restore_after..40 {
            let (a, b) = (
                checked(&mut region, &inputs(tick)),
                checked(&mut copy, &inputs(tick)),
            );
            let case = format!("restored after {restore_after}, tick {tick}");
            assert_eq!(a.entity_changes, b.entity_changes, "{case}");
            assert_eq!(a.arrivals, b.arrivals, "{case}");
            assert_eq!(a.arrivals_taken, b.arrivals_taken, "{case}");
            for key in [sliding.key, lying.key, coming.key] {
                assert_eq!(reports_of(&a, key), reports_of(&b, key), "{case}");
                assert_eq!(record(&region, key), record(&copy, key), "{case}");
            }
            if !a.arrivals.is_empty() {
                sent_at.get_or_insert(tick);
            }
            if !a.arrivals_taken.is_empty() {
                taken_at.get_or_insert(tick);
            }
        }
    }
    // The sliding item crossed early on, and the arrival was taken once, as handed in
    // first.
    assert!(sent_at.is_some_and(|tick| tick < 8), "{sent_at:?}");
    assert_eq!(taken_at, Some(8));
}
