//! Tests of how the edge shows items, written from `docs/adr/0028-entities.md`
//! (sections 1 and 6) and `docs/groundwork/n2-plan.md` (sections 8 and 11) by someone
//! who did not write the code: wire ids are the edge's own, given in the order
//! entities are first shown and never twice; an item's data follows its spawn for
//! every client; an item in a chunk no client views is shown to nobody; and the word
//! with the most changes of holder behind it is the one shown, whether it comes as a
//! spawn, a move, a snapshot or a removal.

use super::*;

const STACK: ItemStack = ItemStack { item: 1, count: 2 };

/// An item of the key `key` at `x` in the spawn's row of chunks, with `hops` changes
/// of holder behind it.
fn thrown(key: u64, x: f64, hops: u32) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: entity_types::ITEM as u16,
            uuid: Uuid::from_u128(u128::from(key) | (9 << 100)),
            motion: [0.0, 0.2, 0.1],
            hops,
            data: EntityData::Item {
                stack: STACK,
                pick_up_delay: 40,
                thrower: None,
                health: 5,
            },
        },
        pose: Pose {
            position: Vec3::new(x, -58.68, 0.5),
            yaw: 10.0,
            pitch: 0.0,
            on_ground: false,
        },
    }
}

fn player_at_home(number: u128, entity: EntityId) -> EntityState {
    EntityState {
        entity,
        kind: EntityKind::Player {
            player: player(number),
            name: format!("Player{number}"),
        },
        pose: Pose::at(SPAWN),
    }
}

fn in_a_tick(events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick: 3, events }
}

/// Two players at the spawn, both shown `HOME` from the west with each other in it.
async fn two_watching() -> (Harness, Client, Client) {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(11)).await;
    let mut second = edge.joined(player(2), EntityId(12)).await;
    edge.quiet().await;
    let ask = edge.ask(WEST, HOME);
    let there = vec![
        player_at_home(1, EntityId(11)),
        player_at_home(2, EntityId(12)),
    ];
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(edge.shows(&mut second, WEST, HOME).await);
    (edge, first, second)
}

async fn settled(edge: &mut Harness, region: RegionId, clients: &mut [&mut Client]) {
    edge.settle(region).await;
    for client in clients.iter_mut() {
        edge.sync(client).await;
    }
}

/// The wire id the client was shown the entity `key` under, if it was spawned.
fn spawned_as(client: &Client, key: u64) -> Option<i32> {
    client
        .naming(key)
        .iter()
        .find(|(naming, _)| *naming == Naming::Spawn)
        .map(|(_, id)| *id)
}

/// ADR-0028, section 1: the edge gives wire ids of its own, in increasing order, as
/// it first shows an entity, whatever its key; a key is shown under one wire id to
/// every client, and a wire id is not given twice in the edge's life, not after its
/// entity is removed either.
#[tokio::test]
async fn wire_ids_are_the_edge_s_own_in_the_order_entities_are_first_shown_and_never_given_twice() {
    let (mut edge, mut first, mut second) = two_watching().await;
    let (high, low) = ((1 << 40) + 9, 3);
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntitySpawned(thrown(high, 2.5, 0))]),
    );
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntitySpawned(thrown(low, 4.5, 0))]),
    );
    settled(&mut edge, WEST, &mut [&mut first, &mut second]).await;
    let ids = |client: &Client| {
        (
            spawned_as(client, high).expect("shown the first"),
            spawned_as(client, low).expect("shown the second"),
        )
    };
    let (high_id, low_id) = ids(&first);
    assert_eq!(
        ids(&second),
        (high_id, low_id),
        "one wire id for every client"
    );
    assert!(high_id < low_id, "in the order shown, not by key");
    for client in [&first, &second] {
        assert_eq!(client.key(high_id), high);
        assert_eq!(client.key(low_id), low);
        assert_ne!(high_id, client.own_id());
        assert_ne!(low_id, client.own_id());
    }
    let players: Vec<i32> = [&first, &second]
        .iter()
        .map(|client| client.own_id())
        .collect();
    assert!(
        players.iter().all(|player| *player < high_id),
        "{players:?}"
    );

    // Removed, and another one thrown: the new one has an id never given before.
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntityRemoved {
            entity: EntityId(low),
            chunk: HOME,
        }]),
    );
    let newer = (1 << 41) + 1;
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntitySpawned(thrown(newer, 6.5, 0))]),
    );
    settled(&mut edge, WEST, &mut [&mut first, &mut second]).await;
    let newer_id = spawned_as(&first, newer).expect("shown the newer");
    assert!(newer_id > low_id, "{newer_id} after {low_id}");
    assert!(!first.entities.contains(&low));
    edge.end().await;
}

/// The plan, section 11: every client in view is sent the item's `SpawnEntity` and
/// then, as the next packet that names an entity, its `SetEntityData`; once.
#[tokio::test]
async fn every_client_in_view_is_sent_an_item_s_data_right_after_its_spawn_once() {
    let (mut edge, mut first, mut second) = two_watching().await;
    let key = 1 << 33;
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntitySpawned(thrown(key, 3.5, 0))]),
    );
    settled(&mut edge, WEST, &mut [&mut first, &mut second]).await;
    let mut third = edge.joined(player(3), EntityId(13)).await;
    edge.quiet().await;
    edge.sync(&mut third).await;
    for client in [&first, &second, &third] {
        let id = spawned_as(client, key).expect("shown the item");
        let at = client
            .named
            .iter()
            .position(|named| *named == (Naming::Spawn, id))
            .expect("spawned");
        assert_eq!(
            client.named.get(at + 1),
            Some(&(Naming::Data, id)),
            "the data next: {:?}",
            client.named
        );
        assert_eq!(
            client
                .named
                .iter()
                .filter(|(naming, named)| *named == id && *naming == Naming::Spawn)
                .count(),
            1
        );
        let data: Vec<&SetEntityData> = client
            .entity_data
            .iter()
            .filter(|data| data.entity_id == id)
            .collect();
        assert_eq!(data.len(), 1, "{data:?}");
        assert_eq!(data[0].values.len(), 1);
        assert_eq!(data[0].values[0].0, 8, "the item entity's data index");
    }
    edge.end().await;
}

/// An item reported in a chunk that no client of the edge views is shown to nobody.
#[tokio::test]
async fn an_item_in_a_chunk_nobody_views_is_shown_to_nobody() {
    let (mut edge, mut first, mut second) = two_watching().await;
    let key = (1 << 34) + 5;
    let far = f64::from(VIEW + 4) * 16.0 + 0.5;
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntitySpawned(thrown(key, far, 0))]),
    );
    settled(&mut edge, WEST, &mut [&mut first, &mut second]).await;
    assert_eq!(spawned_as(&first, key), None);
    assert_eq!(spawned_as(&second, key), None);
    edge.end().await;
}

/// ADR-0028, section 6: once a report with more changes of holder has taken an item
/// over, the region it was shown from before is passed over in all it says of it, its
/// moves, its snapshot without it and its removal; the region that took it over moves
/// and removes it; and a client that comes in between is shown it where that region
/// has it, once.
#[tokio::test]
async fn after_a_higher_hops_takes_an_item_over_the_old_holder_s_moves_snapshots_and_removals_are_passed_over()
 {
    let (mut edge, mut first, mut second) = two_watching().await;
    let key = (1 << 35) + 2;
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntitySpawned(thrown(key, 2.5, 0))]),
    );
    settled(&mut edge, WEST, &mut [&mut first, &mut second]).await;
    let id = spawned_as(&first, key).expect("shown");

    // The east takes it in, with one more hop, somewhat further on.
    edge.tell(
        EAST,
        in_a_tick(vec![RegionEvent::EntitySpawned(thrown(key, 5.5, 1))]),
    );
    settled(&mut edge, EAST, &mut [&mut first, &mut second]).await;
    let moves_before = first
        .naming(key)
        .iter()
        .filter(|(naming, _)| *naming == Naming::Position)
        .count();
    assert!(moves_before >= 1, "moved to where the east has it");

    // The west's word, with fewer: a move, a snapshot without it, a removal.
    let moved = thrown(key, 1.5, 0).pose;
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntityMoved {
            entity: EntityId(key),
            pose: moved,
            previous_chunk: HOME,
        }]),
    );
    let ask = edge.ask(WEST, HOME);
    let there = vec![
        player_at_home(1, EntityId(11)),
        player_at_home(2, EntityId(12)),
    ];
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    edge.tell(
        WEST,
        in_a_tick(vec![RegionEvent::EntityRemoved {
            entity: EntityId(key),
            chunk: HOME,
        }]),
    );
    settled(&mut edge, WEST, &mut [&mut first, &mut second]).await;
    for client in [&first, &second] {
        assert!(client.entities.contains(&key), "the west took it away");
        let named = client.naming(key);
        assert!(
            !named.iter().any(|(naming, _)| *naming == Naming::Removal),
            "{named:?}"
        );
        assert_eq!(
            named
                .iter()
                .filter(|(naming, _)| *naming == Naming::Position)
                .count(),
            moves_before,
            "the west's move was shown: {named:?}"
        );
    }

    // Someone who comes now is shown it once, where the east has it.
    let mut third = edge.joined(player(3), EntityId(13)).await;
    edge.quiet().await;
    edge.sync(&mut third).await;
    let spawns: Vec<&SpawnEntity> = third
        .spawns
        .iter()
        .filter(|spawn| third.key(spawn.entity_id) == key)
        .collect();
    let [spawn] = spawns[..] else {
        panic!("one spawn: {spawns:?}");
    };
    assert_eq!((spawn.entity_id, spawn.x), (id, 5.5));

    // The east's removal takes it away.
    edge.tell(
        EAST,
        in_a_tick(vec![RegionEvent::EntityRemoved {
            entity: EntityId(key),
            chunk: HOME,
        }]),
    );
    settled(&mut edge, EAST, &mut [&mut first, &mut second, &mut third]).await;
    for client in [&first, &second, &third] {
        assert!(!client.entities.contains(&key));
    }
    edge.end().await;
}
