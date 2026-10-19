//! Tests of what the edge shows of an entity other than a player that crosses a line
//! between regions, written from `docs/adr/0028-entities.md`, section 6, and
//! `docs/groundwork/n3-plan.md` (step N3e and section 5, G9) by someone who did not
//! write the edge's handling of it: an entity is shown by its key from the report with
//! the most changes of holder (`hops`); a sent entity is reported "handed on" with the
//! `hops` it had and stays shown, at its pose at the line, until the receiver's taking
//! (at one more) continues it on the wire with no removal and no new spawn; a "handed
//! on" never introduces; a snapshot of a chunk removes what was handed on into it and
//! is not there, keeping the key at the handed-on `hops`; a sender's own snapshot does
//! not; an entity reported in a chunk outside the replica is removed; and with several
//! edges the order of two regions' reports does not matter.

use clustine_protocol::packets::play::{MoveEntityPos, VecDelta};
use clustine_sim::api::Tracked;

use super::*;

const STACK: ItemStack = ItemStack { item: 1, count: 2 };

/// The chunk east of `HOME`, which the eastern region holds in these tests: the line
/// between the two regions is x = 16.
const EASTERN: ChunkPos = ChunkPos::new(1, 0);

/// A chunk far from every viewer: not in the edge's replica.
const UNWATCHED: ChunkPos = ChunkPos::new(20, 0);

const KEY: u64 = 1 << 32;

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

/// The item `KEY` at `position`, with `hops` changes of holder behind it.
fn item(position: [f64; 3], hops: u32) -> EntityState {
    EntityState {
        entity: EntityId(KEY),
        kind: EntityKind::Other {
            kind: entity_types::ITEM as u16,
            uuid: Uuid::from_u128(u128::from(KEY) + 900),
            motion: [0.2, 0.0, 0.0],
            hops,
            data: EntityData::Item {
                stack: STACK,
                pick_up_delay: 0,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw: 30.0,
            pitch: 0.0,
            on_ground: true,
        },
    }
}

fn tracked(position: [f64; 3], hops: u32) -> Tracked {
    Tracked {
        entity: EntityId(KEY),
        chunk: ChunkPos::containing(position[0], position[2]),
        hops,
        position,
        motion: [0.2, 0.0, 0.0],
        yaw: 30.0,
        pitch: 0.0,
        on_ground: true,
        flags: Tracked::SYNC_POSITION,
        data: None,
    }
}

/// Where the item lies before it crosses: west of the line, in `HOME`.
const BEFORE: [f64; 3] = [15.7, -60.0, 0.5];
/// Its pose at the line, which the sender reports with "handed on": east of it.
const AT_LINE: [f64; 3] = [16.1, -60.0, 0.5];
/// Where the receiver's taking reports it, having ticked once in the tick it took it.
const TAKEN: [f64; 3] = [16.3, -60.0, 0.5];

fn handed_on(hops: u32) -> RegionEvent {
    RegionEvent::HandedOn {
        entity: EntityId(KEY),
        chunk: HOME,
        hops,
        position: AT_LINE,
        motion: [0.2, 0.0, 0.0],
    }
}

fn delta(tick: u64, events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick, events }
}

/// An edge with two players at the spawn, shown `HOME` from the west and `EASTERN`
/// from the east, which the west named as its holder; and the item lying in `HOME`,
/// spawned by the west.
async fn across_the_line() -> (Harness, Client, Client) {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    let ask = edge.ask(WEST, EASTERN);
    edge.tell(WEST, elsewhere(EASTERN, ask, EAST));
    edge.settle(WEST).await;
    edge.settle(EAST).await;
    let ask = edge.ask(EAST, EASTERN);
    edge.tell(EAST, snapshot_of(EASTERN, ask, empty_chunk(), Vec::new()));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(edge.shows(&mut first, EAST, EASTERN).await);
    assert!(edge.shows(&mut second, EAST, EASTERN).await);
    edge.tell(
        WEST,
        delta(2, vec![RegionEvent::EntitySpawned(item(BEFORE, 0))]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert!(first.entities.contains(&KEY));
    assert!(second.entities.contains(&KEY));
    (edge, first, second)
}

/// How many times the client was told to spawn the item, and to remove it.
fn spawns_and_removals(client: &Client) -> (usize, usize) {
    let named = client.naming(KEY);
    let count = |what: Naming| named.iter().filter(|(naming, _)| *naming == what).count();
    (count(Naming::Spawn), count(Naming::Removal))
}

/// Every wire id the client was told the item by.
fn ids(client: &Client) -> BTreeSet<i32> {
    client.naming(KEY).iter().map(|(_, id)| *id).collect()
}

/// Where the client was last told the item is, by a whole position.
fn last_whole(client: &Client, id: i32) -> Option<[f64; 3]> {
    client
        .tracking
        .iter()
        .rev()
        .find_map(|packet| match packet {
            ClientboundPlay::SyncEntityPosition(SyncEntityPosition {
                entity_id,
                path: PositionPath::Linear { x, y, z },
                ..
            }) if *entity_id == id => Some([*x, *y, *z]),
            _ => None,
        })
}

/// The whole crossing, seen by two clients: shown by one wire id throughout, never
/// removed, spawned once; the sender's "handed on" pose is shown; the receiver's taking
/// and its later moves go on under the same id, by the tracker that was there.
#[tokio::test]
async fn an_item_crossing_a_line_is_shown_once_by_one_wire_id_with_no_removal_and_no_new_spawn() {
    let (mut edge, mut first, mut second) = across_the_line().await;
    let id = *ids(&first).iter().next().expect("named");

    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    edge.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]),
    );
    edge.settle(EAST).await;
    // The receiver's ticks after the taking: it rolls on east, reported each tick.
    for tick in 4..=30 {
        let x = TAKEN[0] + 0.1 * (tick - 3) as f64;
        edge.tell(
            EAST,
            delta(
                tick,
                vec![
                    RegionEvent::BlockEventsBegin,
                    RegionEvent::EntityTracked(tracked([x, -60.0, 0.5], 1)),
                ],
            ),
        );
    }
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    for client in [&first, &second] {
        assert!(client.entities.contains(&KEY));
        assert_eq!(
            spawns_and_removals(client),
            (1, 0),
            "{:?}",
            client.naming(KEY)
        );
        assert_eq!(
            ids(client),
            BTreeSet::from([id]),
            "{:?}",
            client.naming(KEY)
        );
        let told: Vec<&Told> = client
            .told
            .iter()
            .filter(|told| matches!(told, Told::Shown(KEY) | Told::Hidden(KEY)))
            .collect();
        assert_eq!(told, [&Told::Shown(KEY)]);
        // The tracker went on: the receiver's moves reached the client.
        let moves = client
            .naming(KEY)
            .iter()
            .filter(|(naming, _)| *naming == Naming::Position)
            .count();
        assert!(moves >= 2, "{:?}", client.naming(KEY));
    }
    edge.end().await;
}

/// "Handed on" carries the entity's pose at the line, and the edge takes it (G9): the
/// clients see it there, and one who comes later is shown it there.
#[tokio::test]
async fn the_pose_at_the_line_that_a_handed_on_carries_is_shown() {
    let (mut edge, mut first, _second) = across_the_line().await;
    let id = *ids(&first).iter().next().expect("named");
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert_eq!(last_whole(&first, id), Some(AT_LINE));

    let mut third = edge.joined(player(3), EntityId(7)).await;
    edge.quiet().await;
    edge.sync(&mut third).await;
    let spawns: Vec<&SpawnEntity> = third
        .spawns
        .iter()
        .filter(|spawn| spawn.uuid == Uuid::from_u128(u128::from(KEY) + 900))
        .collect();
    let [spawn] = spawns[..] else {
        panic!("one spawn: {spawns:?}");
    };
    assert_eq!([spawn.x, spawn.y, spawn.z], AT_LINE);
    edge.end().await;
}

/// The taking's spawn goes before that tick's moves (G9): a taking and a move of the
/// receiver's in one tick continue the item under its one spawn, and the receiver's
/// next update moves it on from where the taking put it to where the move did.
#[tokio::test]
async fn a_taking_and_a_move_in_one_tick_continue_the_item_where_the_move_put_it() {
    let (mut edge, mut first, _second) = across_the_line().await;
    let id = *ids(&first).iter().next().expect("named");
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    let moved = [16.6, -60.0, 0.5];
    edge.tell(
        EAST,
        delta(
            3,
            vec![
                RegionEvent::EntitySpawned(item(TAKEN, 1)),
                RegionEvent::BlockEventsBegin,
                RegionEvent::EntityTracked(tracked(moved, 1)),
            ],
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    let before = first.tracking.len();
    edge.tell(EAST, delta(4, vec![RegionEvent::BlockEventsBegin]));
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert_eq!(
        spawns_and_removals(&first),
        (1, 0),
        "{:?}",
        first.naming(KEY)
    );
    // `Math.round(v * 4096)`, half up.
    let steps = |value: f64| (value * 4096.0 + 0.5).floor() as i64;
    let sent: Vec<&ClientboundPlay> = first.tracking[before..].iter().collect();
    let moved_on = sent.iter().any(|packet| match packet {
        ClientboundPlay::MoveEntityPos(MoveEntityPos {
            entity_id,
            delta: VecDelta::Linear([dx, _, _]),
            ..
        }) => *entity_id == id && i64::from(*dx) == steps(moved[0]) - steps(TAKEN[0]),
        ClientboundPlay::SyncEntityPosition(SyncEntityPosition {
            entity_id,
            path: PositionPath::Linear { x, .. },
            ..
        }) => *entity_id == id && *x == moved[0],
        _ => false,
    });
    assert!(moved_on, "{sent:?}");
    edge.end().await;
}

/// A "handed on" never introduces an entity the edge does not show, wherever it says it
/// went.
#[tokio::test]
async fn a_handed_on_of_an_entity_not_shown_introduces_nothing() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, empty_chunk(), vec![own(1, EntityId(5))]),
    );
    assert!(edge.shows(&mut first, WEST, HOME).await);
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.tell(WEST, delta(4, vec![handed_on(5)]));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&KEY));
    assert!(first.naming(KEY).is_empty());
    edge.end().await;
}

/// Reports at fewer `hops` than shown are passed over; at as many, only the region the
/// edge shows the entity from counts: another region's report of the item at the
/// handed-on `hops` does not move it, a move by the sender's old word does not take it
/// back once the receiver has it at one more.
#[tokio::test]
async fn reports_with_fewer_hops_or_as_many_from_another_region_are_passed_over() {
    let (mut edge, mut first, _second) = across_the_line().await;
    let id = *ids(&first).iter().next().expect("named");
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    // The north knows nothing of it but says it at the same hops elsewhere: passed over.
    let stray = [20.5, -60.0, 0.5];
    edge.tell(
        NORTH,
        delta(
            3,
            vec![
                RegionEvent::BlockEventsBegin,
                RegionEvent::EntityTracked(tracked(stray, 0)),
            ],
        ),
    );
    edge.settle(NORTH).await;
    edge.sync(&mut first).await;
    assert_eq!(last_whole(&first, id), Some(AT_LINE));

    edge.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]),
    );
    edge.settle(EAST).await;
    // The west's late word at the old hops: a move back, a spawn, its removal. All
    // passed over.
    edge.tell(
        WEST,
        delta(
            4,
            vec![
                RegionEvent::BlockEventsBegin,
                RegionEvent::EntityTracked(tracked(BEFORE, 0)),
                RegionEvent::EntitySpawned(item(BEFORE, 0)),
                RegionEvent::EntityGone {
                    entity: EntityId(KEY),
                    chunk: HOME,
                    hops: 0,
                },
                RegionEvent::EntityRemoved {
                    entity: EntityId(KEY),
                    chunk: HOME,
                },
            ],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&KEY));
    assert_eq!(
        spawns_and_removals(&first),
        (1, 0),
        "{:?}",
        first.naming(KEY)
    );
    let mut third = edge.joined(player(3), EntityId(7)).await;
    edge.quiet().await;
    edge.sync(&mut third).await;
    let spawn = third
        .spawns
        .iter()
        .find(|spawn| spawn.uuid == Uuid::from_u128(u128::from(KEY) + 900))
        .expect("shown to who comes later");
    assert_eq!([spawn.x, spawn.y, spawn.z], TAKEN);
    edge.end().await;
}

/// A snapshot of the receiving chunk made before the taking (it does not have the
/// item) removes the item handed on into it, and keeps its key at the handed-on hops:
/// the sender's late word at those hops does not bring it back, and the taking at one
/// more introduces it again.
#[tokio::test]
async fn a_snapshot_of_the_receiving_chunk_taken_mid_crossing_removes_it_until_the_taking() {
    let (mut edge, mut first, _second) = across_the_line().await;
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    let ask = edge.ask(EAST, EASTERN);
    edge.tell(EAST, snapshot_of(EASTERN, ask, empty_chunk(), Vec::new()));
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&KEY), "{:?}", first.naming(KEY));

    // The sender's late word, at the hops it was handed on with: passed over.
    edge.tell(
        WEST,
        delta(
            4,
            vec![
                handed_on(0),
                RegionEvent::EntitySpawned(item(AT_LINE, 0)),
                RegionEvent::BlockEventsBegin,
                RegionEvent::EntityTracked(tracked(AT_LINE, 0)),
            ],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&KEY), "{:?}", first.naming(KEY));

    edge.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&KEY), "{:?}", first.naming(KEY));
    let (spawns, removals) = spawns_and_removals(&first);
    assert_eq!((spawns, removals), (2, 1), "{:?}", first.naming(KEY));
    edge.end().await;
}

/// A snapshot of the receiving chunk made after the taking has the item with one more
/// change of holder: it continues the item, with no removal and no new spawn.
#[tokio::test]
async fn a_snapshot_made_after_the_taking_continues_the_item() {
    let (mut edge, mut first, _second) = across_the_line().await;
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    let ask = edge.ask(EAST, EASTERN);
    edge.tell(
        EAST,
        snapshot_of(EASTERN, ask, empty_chunk(), vec![item(TAKEN, 1)]),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&KEY));
    assert_eq!(
        spawns_and_removals(&first),
        (1, 0),
        "{:?}",
        first.naming(KEY)
    );
    edge.end().await;
}

/// The sender's own snapshots after the crossing, of the chunk it sent the item from
/// and from a link it resumes, do not take off what it handed on (G9).
#[tokio::test]
async fn the_sender_s_own_snapshots_do_not_take_off_what_it_handed_on() {
    let (mut edge, mut first, _second) = across_the_line().await;
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&KEY));
    assert_eq!(
        spawns_and_removals(&first),
        (1, 0),
        "{:?}",
        first.naming(KEY)
    );
    edge.end().await;
}

/// Two edges hear the sender and the receiver in either order (ADR-0028, section 6:
/// `hops` orders them): each shows the item once, by one wire id, where the receiver
/// says it is.
#[tokio::test]
async fn two_edges_hearing_the_two_regions_in_either_order_each_show_the_item_once() {
    for receiver_first in [false, true] {
        let (mut edge, mut first, _second) = across_the_line().await;
        let id = *ids(&first).iter().next().expect("named");
        let taking = delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]);
        let sending = delta(3, vec![handed_on(0)]);
        if receiver_first {
            edge.tell(EAST, taking);
            edge.settle(EAST).await;
            edge.tell(WEST, sending);
            edge.settle(WEST).await;
        } else {
            edge.tell(WEST, sending);
            edge.settle(WEST).await;
            edge.tell(EAST, taking);
            edge.settle(EAST).await;
        }
        edge.sync(&mut first).await;
        assert!(
            first.entities.contains(&KEY),
            "receiver first: {receiver_first}"
        );
        assert_eq!(
            spawns_and_removals(&first),
            (1, 0),
            "receiver first: {receiver_first}: {:?}",
            first.naming(KEY)
        );
        assert_eq!(ids(&first), BTreeSet::from([id]));
        assert_eq!(
            last_whole(&first, id),
            if receiver_first { None } else { Some(AT_LINE) },
            "receiver first: {receiver_first}"
        );
        let mut third = edge.joined(player(3), EntityId(7)).await;
        edge.quiet().await;
        edge.sync(&mut third).await;
        let spawn = third
            .spawns
            .iter()
            .find(|spawn| spawn.uuid == Uuid::from_u128(u128::from(KEY) + 900))
            .expect("shown to who comes later");
        assert_eq!(
            [spawn.x, spawn.y, spawn.z],
            TAKEN,
            "receiver first: {receiver_first}"
        );
        edge.end().await;
    }
}

/// Two edges run at once, one hearing the receiver's taking before the sender's
/// "handed on" and the other after: both show it once, by their own wire ids.
#[tokio::test]
async fn two_edges_at_once_show_a_crossing_item_once_each() {
    let (mut one, mut first, _) = across_the_line().await;
    let (mut other, mut seeing, _) = across_the_line().await;
    one.tell(WEST, delta(3, vec![handed_on(0)]));
    one.settle(WEST).await;
    other.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]),
    );
    other.settle(EAST).await;
    one.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]),
    );
    one.settle(EAST).await;
    other.tell(WEST, delta(3, vec![handed_on(0)]));
    other.settle(WEST).await;
    one.sync(&mut first).await;
    other.sync(&mut seeing).await;
    for client in [&first, &seeing] {
        assert!(client.entities.contains(&KEY));
        assert_eq!(
            spawns_and_removals(client),
            (1, 0),
            "{:?}",
            client.naming(KEY)
        );
        assert_eq!(ids(client).len(), 1);
    }
    one.end().await;
    other.end().await;
}

/// An entity reported in a chunk the edge's replica does not have, taken in land no
/// viewer of this edge watches, is removed from the viewers that were shown it: the
/// early return of a report outside the replica must not skip it (N3e).
#[tokio::test]
async fn an_item_taken_in_a_chunk_outside_the_replica_is_removed_from_its_viewers() {
    let (mut edge, mut first, _second) = across_the_line().await;
    let far = [
        f64::from(UNWATCHED.x) * 16.0 + 0.5,
        -60.0,
        f64::from(UNWATCHED.z) * 16.0 + 0.5,
    ];
    edge.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(far, 1))]),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&KEY), "{:?}", first.naming(KEY));
    edge.end().await;
}

/// The same for a move: reported at one more change of holder in a chunk outside the
/// replica, the item goes from its viewers.
#[tokio::test]
async fn an_item_moved_by_its_new_holder_out_of_the_replica_is_removed_from_its_viewers() {
    let (mut edge, mut first, _second) = across_the_line().await;
    edge.tell(WEST, delta(3, vec![handed_on(0)]));
    edge.settle(WEST).await;
    edge.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]),
    );
    let far = [
        f64::from(UNWATCHED.x) * 16.0 + 0.5,
        -60.0,
        f64::from(UNWATCHED.z) * 16.0 + 0.5,
    ];
    edge.tell(
        EAST,
        delta(
            4,
            vec![
                RegionEvent::BlockEventsBegin,
                RegionEvent::EntityTracked(tracked(far, 1)),
            ],
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&KEY), "{:?}", first.naming(KEY));
    edge.end().await;
}

/// A removal at fewer hops than shown is passed over; the receiver's removal takes it
/// away, and the key is kept so that the sender's late "handed on", its late snapshot
/// with the item and its late spawn do not bring back what was removed.
#[tokio::test]
async fn a_removed_item_is_not_brought_back_by_the_old_holder_s_late_word() {
    let (mut edge, mut first, _second) = across_the_line().await;
    edge.tell(
        EAST,
        delta(3, vec![RegionEvent::EntitySpawned(item(TAKEN, 1))]),
    );
    edge.settle(EAST).await;
    edge.tell(
        WEST,
        delta(
            3,
            vec![RegionEvent::EntityGone {
                entity: EntityId(KEY),
                chunk: HOME,
                hops: 0,
            }],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert!(first.entities.contains(&KEY));

    edge.tell(
        EAST,
        delta(
            4,
            vec![RegionEvent::EntityGone {
                entity: EntityId(KEY),
                chunk: EASTERN,
                hops: 1,
            }],
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&KEY));

    edge.tell(WEST, delta(4, vec![handed_on(0)]));
    edge.tell(
        WEST,
        delta(5, vec![RegionEvent::EntitySpawned(item(BEFORE, 0))]),
    );
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6)), item(BEFORE, 0)];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    // And the receiver's own late snapshot from before the removal, at its hops.
    let ask = edge.ask(EAST, EASTERN);
    edge.tell(
        EAST,
        snapshot_of(EASTERN, ask, empty_chunk(), vec![item(TAKEN, 1)]),
    );
    edge.settle(WEST).await;
    edge.settle(EAST).await;
    edge.sync(&mut first).await;
    assert!(!first.entities.contains(&KEY), "{:?}", first.naming(KEY));
    assert_eq!(
        spawns_and_removals(&first),
        (1, 1),
        "{:?}",
        first.naming(KEY)
    );
    edge.end().await;
}
