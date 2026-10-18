//! Tests of what the edge sends of leashes (step N5g, `docs/groundwork/n5-plan.md`;
//! `docs/groundwork/n5-brief.md`, sections 7.5, 7.6 and 8.3): every client that shows a
//! leashed entity is told who holds its leash on every change, 0 when nobody does, and a
//! client newly shown it is told after its spawn; a knot is spawned at its fence's
//! corner with no data.

use clustine_protocol::packet_ids::play::clientbound;
use clustine_world::{Attached, Float, Hurt};

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

/// An oak boat of the key `key` beside the spawn, in `HOME`.
fn boat(key: u64) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: entity_types::OAK_BOAT as u16,
            uuid: Uuid::from_u128(u128::from(key) + 9_000),
            motion: [0.0; 3],
            hops: 0,
            data: EntityData::Boat {
                hurt: Hurt::default(),
                paddles: [false; 2],
                bubble_time: 0,
                out_of_control: Float(0.0),
                turning: Float(0.0),
                status: [3; 2],
                contents: Vec::new(),
            },
            riding: Default::default(),
        },
        pose: Pose::at(Vec3::new(3.5, -60.0, 0.5)),
    }
}

fn leashed(entity: u64, holder: Option<u64>) -> RegionEvent {
    RegionEvent::Leashed {
        entity: EntityId(entity),
        chunk: HOME,
        holder: holder.map(EntityId),
    }
}

fn ticked(events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick: 2, events }
}

fn links(client: &Client) -> Vec<(i32, i32)> {
    client
        .links
        .iter()
        .map(|link| (link.leashed_id, link.holder_id))
        .collect()
}

/// A boat tied to player 1 and let go again: both players, who see the boat, are told
/// each time, the holder by its id, and 0 once nobody holds it.
#[tokio::test]
async fn every_client_that_shows_a_leashed_boat_is_told_who_holds_it_on_each_change() {
    let mut edge = Harness::start().await;
    let mut holder = edge.joined(player(1), EntityId(5)).await;
    let mut watcher = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let key = 1 << 32;
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6)), boat(key)];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut holder, WEST, HOME).await);
    assert!(edge.shows(&mut watcher, WEST, HOME).await);

    edge.tell(WEST, ticked(vec![leashed(key, Some(5))]));
    edge.quiet().await;
    holder.drain();
    watcher.drain();
    let boat_id = watcher.ledger_id(key);
    let holder_id = watcher.ledger_id(5);
    assert_eq!(links(&watcher), [(boat_id, holder_id)]);
    assert_eq!(links(&holder), [(boat_id, holder_id)]);

    edge.tell(WEST, ticked(vec![leashed(key, None)]));
    edge.quiet().await;
    holder.drain();
    watcher.drain();
    assert_eq!(links(&watcher), [(boat_id, holder_id), (boat_id, 0)]);
    assert_eq!(links(&holder), [(boat_id, holder_id), (boat_id, 0)]);
    edge.end().await;
}

/// A client shown a boat after it was tied is told who holds it, after the boat's
/// spawn, as `sendPairingData` sends the link last; one shown it after it was let go
/// is told nothing.
#[tokio::test]
async fn a_client_newly_shown_a_leashed_boat_is_told_who_holds_it_after_its_spawn() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let key = 1 << 32;
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(
            HOME,
            ask,
            empty_chunk(),
            vec![own(1, EntityId(5)), boat(key)],
        ),
    );
    assert!(edge.shows(&mut first, WEST, HOME).await);
    edge.tell(WEST, ticked(vec![leashed(key, Some(5))]));
    edge.quiet().await;

    let mut late = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    assert!(edge.shows(&mut late, WEST, HOME).await);
    late.drain();
    let boat_id = late.ledger_id(key);
    let holder_id = late.ledger_id(5);
    assert_eq!(links(&late), [(boat_id, holder_id)]);
    let spawned_at = late
        .ids
        .iter()
        .position(|id| *id == clientbound::ADD_ENTITY)
        .expect("the boat's spawn");
    let linked_at = late
        .ids
        .iter()
        .rposition(|id| *id == clientbound::SET_ENTITY_LINK)
        .expect("the link");
    assert!(spawned_at < linked_at);

    edge.tell(WEST, ticked(vec![leashed(key, None)]));
    edge.quiet().await;
    let mut later = edge.joined(player(3), EntityId(7)).await;
    edge.quiet().await;
    assert!(edge.shows(&mut later, WEST, HOME).await);
    later.drain();
    assert!(links(&later).is_empty(), "{:?}", links(&later));
    edge.end().await;
}

/// A knot is spawned at its fence block's corner with no data: the client puts it at
/// the block's middle itself.
#[tokio::test]
async fn a_knot_is_spawned_at_its_fences_corner_with_no_data() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, empty_chunk(), vec![own(1, EntityId(5))]),
    );
    assert!(edge.shows(&mut first, WEST, HOME).await);
    let key = 1 << 32;
    let knot = EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: entity_types::LEASH_KNOT as u16,
            uuid: Uuid::from_u128(77),
            motion: [0.0; 3],
            hops: 0,
            data: EntityData::LeashKnot {
                attached: Attached {
                    block: BlockPos::new(2, -60, 3),
                    direction: 0,
                },
            },
            riding: Default::default(),
        },
        pose: Pose::at(Vec3::new(2.5, -59.625, 3.5)),
    };
    edge.tell(WEST, ticked(vec![RegionEvent::EntitySpawned(knot)]));
    edge.quiet().await;
    first.drain();
    let id = first.ledger_id(key);
    let spawn = first
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .expect("the knot's spawn");
    assert_eq!(spawn.kind, entity_types::LEASH_KNOT);
    assert_eq!((spawn.x, spawn.y, spawn.z), (2.0, -60.0, 3.0));
    assert_eq!(spawn.data, 0);
    edge.end().await;
}
