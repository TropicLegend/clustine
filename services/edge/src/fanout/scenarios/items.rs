//! Tests of how the edge shows an entity other than a player, the item a player
//! throws (step N2c, `docs/groundwork/n2-plan.md`, sections 8 and 11, and
//! `docs/adr/0028-entities.md`, section 6): every client that has its chunk in view is
//! shown it once, under the one wire id the edge gives its key, with `SpawnEntity`
//! (its type, UUID, position, motion, pitch 0, its yaw, head yaw 0, data 0) and then
//! `SetEntityData` with its stack; it is shown from the region whose word has the most
//! changes of holder behind it; and the thrower's arm swings for the others.

use clustine_protocol::packets::play::entity_data_serializer;

use super::*;

/// One of the edge's own players, standing where they entered the world.
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

const STACK: ItemStack = ItemStack { item: 1, count: 3 };

/// An item of the key `key` in `chunk` with `hops` changes of holder behind it, which
/// player 1 threw.
fn item(key: u64, chunk: ChunkPos, hops: u32) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: entity_types::ITEM as u16,
            uuid: Uuid::from_u128(u128::from(key) + 7_000),
            motion: [0.1, 0.25, -0.2],
            hops,
            data: EntityData::Item {
                stack: STACK,
                pick_up_delay: 40,
                thrower: Some(player(1).0),
                health: 5,
            },
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(
                f64::from(chunk.x) * 16.0 + 0.5,
                -58.68,
                f64::from(chunk.z) * 16.0 + 0.5,
            ),
            yaw: 200.0,
            pitch: 0.0,
            on_ground: false,
        },
    }
}

fn ticked(events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick: 2, events }
}

/// Two players at the spawn, both shown `HOME` with each other in it.
async fn two_at_home() -> (Harness, Client, Client) {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(edge.shows(&mut second, WEST, HOME).await);
    (edge, first, second)
}

/// The wire ids the client was shown the item `key` by, with the packets that named
/// it, after it was shown everything else these tests set up.
fn of_item(client: &Client, key: u64) -> Vec<(Naming, i32)> {
    client.naming(key)
}

/// Checks that the client was shown the item `state` once, as the plan has it, and
/// returns the wire id. What its tracker sent after, the update of the tick it was
/// thrown in (a move of nothing: `tests::tracking`), is no showing.
fn shown_once(client: &Client, state: &EntityState) -> i32 {
    let key = state.entity.0;
    let named = of_item(client, key);
    let [
        (Naming::Spawn, id),
        (Naming::Data, data_id),
        ref updates @ ..,
    ] = named[..]
    else {
        panic!("a spawn and its data: {named:?}");
    };
    assert!(
        updates
            .iter()
            .all(|(naming, _)| *naming == Naming::Position),
        "{named:?}"
    );
    assert_eq!(id, data_id);
    let spawns: Vec<&SpawnEntity> = client
        .spawns
        .iter()
        .filter(|spawn| spawn.entity_id == id)
        .collect();
    let [spawn] = spawns[..] else {
        panic!("one spawn: {spawns:?}");
    };
    let EntityKind::Other {
        kind, uuid, motion, ..
    } = &state.kind
    else {
        panic!("an item");
    };
    assert_eq!(spawn.kind, i32::from(*kind));
    assert_eq!(spawn.kind, entity_types::ITEM);
    assert_eq!(spawn.uuid, *uuid);
    let position = state.pose.position;
    assert_eq!(
        (spawn.x, spawn.y, spawn.z),
        (position.x, position.y, position.z)
    );
    assert_eq!(
        (spawn.pitch, spawn.yaw, spawn.head_yaw, spawn.data),
        (0, angle(200.0), 0, 0)
    );
    // The motion as the protocol carries it: to a step of 1/16383 of the largest
    // component rounded up.
    for (sent, made) in spawn.velocity.iter().zip(motion) {
        assert!(
            (sent - made).abs() <= 1.0 / 32766.0,
            "{:?} for {motion:?}",
            spawn.velocity
        );
    }
    let data: Vec<&SetEntityData> = client
        .entity_data
        .iter()
        .filter(|data| data.entity_id == id)
        .collect();
    let stack = clustine_protocol::item::ItemStack {
        item: STACK.item,
        count: STACK.count,
    };
    assert_eq!(
        data,
        [&SetEntityData {
            entity_id: id,
            values: vec![(
                entity_data::ITEM_STACK,
                EntityDataValue::ItemStack(Some(stack))
            )],
        }]
    );
    id
}

#[tokio::test]
async fn a_thrown_item_is_shown_to_every_client_in_view_once_by_one_wire_id_and_the_thrower_s_arm_swings_for_others()
 {
    let (mut edge, mut first, mut second) = two_at_home().await;
    let thrown = item(1 << 32, HOME, 0);
    let swung = RegionEvent::ArmSwung {
        entity: EntityId(5),
        chunk: HOME,
    };
    edge.tell(
        WEST,
        ticked(vec![swung, RegionEvent::EntitySpawned(thrown.clone())]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    // The thrower is shown it too: their client does not make the item itself.
    let id = shown_once(&first, &thrown);
    assert_eq!(shown_once(&second, &thrown), id);
    assert_eq!(first.key(id), 1 << 32);
    // The thrower's arm, by the wire id of their entity, for the other one only.
    let thrower = second.naming(5);
    assert!(
        thrower.contains(&(Naming::Swing, thrower[0].1)),
        "{thrower:?}"
    );
    assert_eq!(
        second.swings,
        [Animate {
            entity_id: thrower[0].1,
            action: animation::SWING_MAIN_HAND,
        }]
    );
    assert_eq!(first.swings, []);

    // A snapshot of the chunk that has it, as the worker sends after the tick, shows
    // nothing again; nor does the same word twice.
    let ask = edge.ask(WEST, HOME);
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6)), thrown.clone()];
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::EntitySpawned(thrown.clone())]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    shown_once(&first, &thrown);
    shown_once(&second, &thrown);

    // Someone who comes later is shown it as it is, under the same id.
    let mut third = edge.joined(player(3), EntityId(7)).await;
    edge.quiet().await;
    edge.sync(&mut third).await;
    assert_eq!(shown_once(&third, &thrown), id);
    edge.end().await;
}

/// ADR-0028, section 6: a report with more changes of holder behind it takes over, one
/// with fewer is passed over, and one with as many counts only from the region the edge
/// shows the entity from.
#[tokio::test]
async fn an_item_is_shown_from_the_word_with_the_most_changes_of_holder() {
    let (mut edge, mut first, _second) = two_at_home().await;
    let key = 1 << 32;
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::EntitySpawned(item(key, HOME, 0))]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    let moves = |client: &Client| {
        let named = client.named.iter();
        named
            .filter(|(naming, _)| *naming == Naming::Position)
            .count()
    };
    // The update of the tick of its spawn: a move of nothing.
    let updated = moves(&first);
    assert_eq!(updated, 1);

    // The east says the same with as many: passed over, so the west's snapshot without
    // it still removes it.
    let mut moved = item(key, HOME, 0);
    moved.pose.position.x += 2.0;
    edge.tell(
        EAST,
        ticked(vec![RegionEvent::EntitySpawned(moved.clone())]),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert_eq!(
        first
            .spawns
            .iter()
            .filter(|spawn| spawn.uuid == Uuid::from_u128(u128::from(key) + 7_000))
            .count(),
        1
    );
    assert_eq!(moves(&first), updated);

    // With one more, the east takes over: the client is shown it where the east says,
    // under the id it has, with no spawn.
    let mut taken = item(key, HOME, 1);
    taken.pose.position.x += 3.0;
    edge.tell(EAST, ticked(vec![RegionEvent::EntitySpawned(taken)]));
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    let named = of_item(&first, key);
    let id = named[0].1;
    assert_eq!(
        named,
        [
            (Naming::Spawn, id),
            (Naming::Data, id),
            (Naming::Position, id),
            (Naming::Position, id)
        ]
    );

    // The west's word, with fewer, is passed over: its snapshot without the item does
    // not take it away, and its word with it does not move it back.
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::EntitySpawned(item(key, HOME, 0))]),
    );
    let ask = edge.ask(WEST, HOME);
    let mut chunk = empty_chunk();
    chunk.set(1, -61, 1, STONE);
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
    edge.tell(WEST, snapshot_of(HOME, ask, chunk, there));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert_eq!(of_item(&first, key).len(), 4, "{:?}", of_item(&first, key));
    assert!(first.entities.contains(&key));

    // The east's removal of it takes it away.
    edge.tell(
        EAST,
        ticked(vec![RegionEvent::EntityRemoved {
            entity: EntityId(key),
            chunk: HOME,
        }]),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&key));
    assert_eq!(of_item(&first, key).last(), Some(&(Naming::Removal, id)));
    edge.end().await;
}

#[test]
fn an_item_s_data_is_its_stack_by_the_game_s_index_and_serializer() {
    let data = entity_data_packet(
        9,
        &EntityData::Item {
            stack: STACK,
            pick_up_delay: 0,
            thrower: None,
            health: 5,
        },
    );
    let bytes = packets::encode(&data);
    // The packet's id, the wire id, `ItemEntity.DATA_ITEM`'s index 8, the serializer
    // `ITEM_STACK`, 7, the stack and the end.
    assert_eq!(
        bytes[1..4],
        [
            9,
            entity_data::ITEM_STACK,
            entity_data_serializer::ITEM_STACK as u8
        ]
    );
    assert_eq!(bytes[1..4], [9, 8, 7]);
    assert_eq!(bytes.last(), Some(&0xFF));
}

/// An item its region sends across a line goes on being shown where it was sent, by
/// its wire id, until the region it went to reports it with one more change of holder,
/// which continues it with no new spawn; an item the edge does not show is not
/// introduced by a word that it was handed on (`docs/adr/0028-entities.md`, section 6;
/// `docs/groundwork/n3-plan.md`, step N3e).
#[tokio::test]
async fn an_item_handed_on_across_a_line_is_shown_where_it_went_until_its_new_holder_reports_it() {
    let (mut edge, mut first, _second) = two_at_home().await;
    let key = 1 << 32;
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::EntitySpawned(item(key, HOME, 0))]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    let id = of_item(&first, key)[0].1;

    // Sent to the east with no change of holder yet: shown there, moved, not removed.
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::HandedOn {
            entity: EntityId(key),
            chunk: HOME,
            hops: 0,
            position: [2.5, -58.68, 0.5],
            motion: [0.1, 0.0, 0.0],
        }]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&key));
    assert!(
        of_item(&first, key)
            .iter()
            .all(|(naming, named)| *naming != Naming::Removal && *named == id)
    );

    // The east takes it, with one more change of holder: no new spawn, the same id.
    let mut taken = item(key, HOME, 1);
    taken.pose.position.x = 2.5;
    edge.tell(EAST, ticked(vec![RegionEvent::EntitySpawned(taken)]));
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    let spawns = of_item(&first, key)
        .iter()
        .filter(|(naming, _)| *naming == Naming::Spawn)
        .count();
    assert_eq!(spawns, 1);
    assert!(first.entities.contains(&key));

    // A word that an entity the edge does not show was handed on shows nothing.
    let other = (1 << 32) + 1;
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::HandedOn {
            entity: EntityId(other),
            chunk: HOME,
            hops: 0,
            position: [3.5, -58.68, 0.5],
            motion: [0.0; 3],
        }]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&other));
    assert!(of_item(&first, other).is_empty());
    edge.end().await;
}

/// The sender's own snapshot of the chunk it handed an item on from does not take the
/// item off: it is no longer the sender's to show or to take off (the plan, section 5,
/// G9). That another region's snapshot without it takes it off is tested end to end.
#[tokio::test]
async fn an_item_handed_on_is_not_taken_off_by_its_sender_s_own_snapshot() {
    let (mut edge, mut first, _second) = two_at_home().await;
    let key = 1 << 32;
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::EntitySpawned(item(key, HOME, 0))]),
    );
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::HandedOn {
            entity: EntityId(key),
            chunk: HOME,
            hops: 0,
            position: [2.5, -58.68, 0.5],
            motion: [0.0; 3],
        }]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&key));

    // The sender's snapshot of the chunk, without the item: kept.
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
    let ask = edge.ask(WEST, HOME);
    let mut chunk = empty_chunk();
    chunk.set(1, -61, 1, STONE);
    edge.tell(WEST, snapshot_of(HOME, ask, chunk.clone(), there.clone()));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&key));

    edge.end().await;
}
