//! Tests of how the edge shows riding (step N5b, `docs/groundwork/n5-plan.md`;
//! `docs/groundwork/n5-brief.md`, sections 0.11 and 1.11): the rider's own client is told
//! where they are when they get on and off, as by a teleport whose moves before its
//! confirmation are passed over; every client that shows the vehicle, or rides it, is
//! told whom it carries on every change and when it is first shown; and a rider is
//! sent no position, only which way they look.

use super::*;

fn own(number: u128, entity: EntityId) -> EntityState {
    EntityState {
        entity,
        kind: EntityKind::Player {
            player: player(number),
            name: format!("Player{number}"),
        },
        pose: Pose::at(SPAWN),
    }
}

/// A cushion of the key `key` beside the spawn, in `HOME`.
fn cushion(key: u64) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: entity_types::CUSHION as u16,
            uuid: Uuid::from_u128(u128::from(key) + 9_000),
            motion: [0.0; 3],
            hops: 0,
            data: EntityData::Cushion {
                attached: clustine_world::Attached {
                    block: block_of(HOME),
                    direction: 0,
                },
                colour: 0,
            },
            riding: Default::default(),
        },
        pose: Pose::at(SPAWN),
    }
}

fn ticked(events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick: 2, events }
}

fn passengers(vehicle: u64, riders: &[u64]) -> RegionEvent {
    RegionEvent::Passengers {
        vehicle: EntityId(vehicle),
        chunk: HOME,
        hops: 0,
        passengers: riders.iter().map(|key| EntityId(*key)).collect(),
    }
}

#[tokio::test]
async fn a_rider_s_client_is_teleported_on_and_off_and_every_client_is_told_who_rides() {
    let mut edge = Harness::start().await;
    let mut rider = edge.joined(player(1), EntityId(5)).await;
    let mut watcher = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let seat = cushion(1 << 32);
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6)), seat.clone()];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut rider, WEST, HOME).await);
    assert!(edge.shows(&mut watcher, WEST, HOME).await);
    let teleports_before = rider.teleports.len();
    let told_before = (rider.passengers.len(), watcher.passengers.len());

    edge.tell(WEST, ticked(vec![passengers(1 << 32, &[5])]));
    let sat = Pose::at(Vec3::new(0.5, -60.35, 0.5));
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Rides {
                vehicle: Some(EntityId(1 << 32)),
                pose: sat,
            },
        },
    );
    edge.quiet().await;
    rider.drain();
    watcher.drain();
    let seat_id = rider.ledger_id(1 << 32);
    let own_id = rider.ledger_id(5);
    // The vehicle's word and the rider's agree: one change of the list, told once, as
    // the official server tells it (`minecarts_oracle.rs`).
    assert_eq!(
        (rider.passengers.len(), watcher.passengers.len()),
        (told_before.0 + 1, told_before.1 + 1)
    );
    assert_eq!(
        rider
            .passengers
            .last()
            .map(|told| (told.vehicle_id, told.passengers.clone())),
        Some((seat_id, vec![own_id]))
    );
    assert_eq!(
        watcher
            .passengers
            .last()
            .map(|told| (told.vehicle_id, told.passengers.clone())),
        Some((seat_id, vec![own_id]))
    );
    let teleport = rider.teleports.last().expect("a teleport onto the seat");
    assert_eq!(rider.teleports.len(), teleports_before + 1);
    assert_eq!(teleport.teleport_id, 2, "the login's was the first");
    assert_eq!((teleport.x, teleport.y, teleport.z), (0.5, -60.35, 0.5));
    // As `ServerPlayer.startRiding`: the look relative by nothing, so that the client
    // keeps its own, which it confirms with.
    assert_eq!(
        (teleport.yaw, teleport.pitch, teleport.relative_flags),
        (0.0, 0.0, relative::ROTATION)
    );

    // A rider's move is no position for others: they place a rider themselves.
    let moved_before = watcher.named.len();
    let mut turned = own(1, EntityId(5));
    turned.pose = Pose { yaw: 90.0, ..sat };
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::EntityMoved {
            entity: EntityId(5),
            pose: turned.pose,
            previous_chunk: HOME,
        }]),
    );
    edge.quiet().await;
    watcher.drain();
    let since: Vec<Naming> = watcher.named[moved_before..]
        .iter()
        .filter(|(_, id)| *id == own_id)
        .map(|(naming, _)| *naming)
        .collect();
    // The body's rotation (`move_entity_rot`, which the client counts with moves) and
    // the head's, and no position.
    assert_eq!(since, [Naming::Position, Naming::HeadRotation]);
    assert!(watcher.tracking.iter().any(|packet| matches!(
        packet,
        ClientboundPlay::MoveEntityRot(rotation) if rotation.entity_id == own_id
    )));
    assert!(!watcher.tracking.iter().any(|packet| matches!(
        packet,
        ClientboundPlay::SyncEntityPosition(moved) if moved.entity_id == own_id
    )));

    // Off again: the seat is empty for everyone, and the rider teleported once more.
    let told_before = watcher.passengers.len();
    edge.tell(WEST, ticked(vec![passengers(1 << 32, &[])]));
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Rides {
                vehicle: None,
                pose: Pose::at(Vec3::new(0.5, -59.75, 0.5)),
            },
        },
    );
    edge.quiet().await;
    rider.drain();
    watcher.drain();
    assert_eq!(
        watcher
            .passengers
            .last()
            .map(|told| told.passengers.clone()),
        Some(Vec::new())
    );
    assert_eq!(watcher.passengers.len(), told_before + 1, "told once");
    assert_eq!(
        rider
            .teleports
            .last()
            .map(|told| (told.teleport_id, told.relative_flags)),
        Some((3, relative::ROTATION))
    );
}

/// The rider's own region may speak before the vehicle's: its word seats them at once,
/// and the vehicle's naming them after changes nothing a client is told again.
#[tokio::test]
async fn a_rider_s_word_before_the_vehicle_s_is_told_once() {
    let mut edge = Harness::start().await;
    let mut rider = edge.joined(player(1), EntityId(5)).await;
    let mut watcher = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let seat = cushion(1 << 32);
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6)), seat.clone()];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut rider, WEST, HOME).await);
    assert!(edge.shows(&mut watcher, WEST, HOME).await);
    let told_before = watcher.passengers.len();
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Rides {
                vehicle: Some(EntityId(1 << 32)),
                pose: Pose::at(Vec3::new(0.5, -60.35, 0.5)),
            },
        },
    );
    edge.tell(WEST, ticked(vec![passengers(1 << 32, &[5])]));
    edge.quiet().await;
    watcher.drain();
    let own_id = watcher.ledger_id(5);
    assert_eq!(watcher.passengers.len(), told_before + 1, "told once");
    assert_eq!(
        watcher
            .passengers
            .last()
            .map(|told| told.passengers.clone()),
        Some(vec![own_id])
    );
}

#[tokio::test]
async fn a_client_newly_shown_a_vehicle_is_told_who_rides_it() {
    let mut edge = Harness::start().await;
    let mut rider = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let seat = cushion(1 << 32);
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, empty_chunk(), vec![own(1, EntityId(5)), seat]),
    );
    assert!(edge.shows(&mut rider, WEST, HOME).await);
    edge.tell(WEST, ticked(vec![passengers(1 << 32, &[5])]));
    edge.quiet().await;
    let mut late = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    assert!(edge.shows(&mut late, WEST, HOME).await);
    late.drain();
    let seat_id = late.ledger_id(1 << 32);
    let rider_id = late.ledger_id(5);
    assert!(
        late.passengers
            .iter()
            .any(|told| told.vehicle_id == seat_id && told.passengers == [rider_id]),
        "{:?}",
        late.passengers
    );
}

#[tokio::test]
async fn a_snapshot_of_a_ridden_vehicle_tells_a_client_who_rides_it() {
    let mut edge = Harness::start().await;
    let mut rider = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let mut seat = cushion(1 << 32);
    if let EntityKind::Other { riding, .. } = &mut seat.kind {
        riding.passengers.push(clustine_world::Seat {
            key: EntityId(5),
            player: true,
            pending: None,
        });
    }
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, empty_chunk(), vec![own(1, EntityId(5)), seat]),
    );
    assert!(edge.shows(&mut rider, WEST, HOME).await);
    let mut late = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    assert!(edge.shows(&mut late, WEST, HOME).await);
    late.drain();
    let seat_id = late.ledger_id(1 << 32);
    let rider_id = late.ledger_id(5);
    assert!(
        late.passengers
            .iter()
            .any(|told| told.vehicle_id == seat_id && told.passengers == [rider_id]),
        "{:?}",
        late.passengers
    );
}
