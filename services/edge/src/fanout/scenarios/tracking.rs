//! Tests of what the edge sends of an entity other than a player as it moves, falls,
//! lands, is picked up and goes (step N4d, `docs/groundwork/n4-plan.md`, section 7;
//! `docs/groundwork/n4-brief.md`, section 9; `docs/adr/0028-entities.md`, section 6),
//! from reports fed by hand as a region would send them: each tick's updates where the
//! game's `sendChanges` has them, after the tick's block changes and before its
//! entities' reports; the ticks a region said nothing in caught up; a falling block's
//! spawn, its block's change and its landing in the game's order; a pick-up and its
//! removal; a removal by `hops`, kept against late word.

use clustine_protocol::delta::encode as steps;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    MoveEntityPos, SetEntityMotion, TakeItemEntity as Taken, VecDelta,
};
use clustine_sim::api::Tracked;

use super::*;

const STACK: ItemStack = ItemStack { item: 1, count: 3 };
const SAND: BlockState = BlockState(118);
const AIR: BlockState = BlockState(0);

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

/// Two players at the spawn, both shown `HOME` from the west with each other in it.
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

fn other(key: u64, position: [f64; 3], hops: u32, data: EntityData) -> EntityState {
    let kind = match data {
        EntityData::Item { .. } => entity_types::ITEM,
        EntityData::FallingBlock { .. } => entity_types::FALLING_BLOCK,
        _ => unreachable!("the scenarios of tracking show items and falling blocks"),
    };
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: kind as u16,
            uuid: Uuid::from_u128(u128::from(key) + 9_000),
            motion: [0.0; 3],
            hops,
            data,
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        },
    }
}

fn item(key: u64, position: [f64; 3], hops: u32) -> EntityState {
    let data = EntityData::Item {
        stack: STACK,
        pick_up_delay: 40,
        thrower: None,
        health: 5,
        age_offset: 0,
    };
    other(key, position, hops, data)
}

fn tracked(key: u64, position: [f64; 3], motion: [f64; 3], on_ground: bool, flags: u8) -> Tracked {
    Tracked {
        entity: EntityId(key),
        chunk: HOME,
        hops: 0,
        position,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
        flags,
        data: None,
    }
}

/// What `client` was sent of entities' tracking because of `region`'s report of `tick`
/// with `events`.
async fn told(
    edge: &mut Harness,
    client: &mut Client,
    region: RegionId,
    tick: u64,
    events: Vec<RegionEvent>,
) -> Vec<ClientboundPlay> {
    let before = client.tracking.len();
    edge.tell(region, WorkerToEdge::TickDelta { tick, events });
    edge.settle(region).await;
    edge.sync(client).await;
    client.tracking[before..].to_vec()
}

/// `client`'s packets from the `from`th on, by id, of those an entity's showing and its
/// blocks are made of.
fn kinds_from(client: &Client, from: usize) -> Vec<i32> {
    let of_interest = [
        clientbound::ADD_ENTITY,
        clientbound::SET_ENTITY_DATA,
        clientbound::BLOCK_UPDATE,
        clientbound::SECTION_BLOCKS_UPDATE,
        clientbound::MOVE_ENTITY_POS,
        clientbound::SET_ENTITY_MOTION,
        clientbound::ENTITY_POSITION_SYNC,
        clientbound::ADD_TRANSIENT_BLOCK,
        clientbound::TAKE_ITEM_ENTITY,
        clientbound::REMOVE_ENTITIES,
    ];
    let ids = client.ids[from..].iter().copied();
    ids.filter(|id| of_interest.contains(id)).collect()
}

/// An item falls from y 70 at 0.1 a tick from its spawn in tick 10, and lands at 67 in
/// tick 40; then its region says nothing until tick 100, and then reports every tick.
/// Worked through `sendChanges` (the brief's section 9): the update of tick `t` has the
/// count of ticks `t - 10` and the item as reported in tick `t - 1`. Tick 10: the
/// spawn's update, a move of nothing (count 0, a multiple of 60). Tick 30 (count 20):
/// the motion, changed from none, then the move from 70 to tick 29's 68.1. Tick 50
/// (count 40), caught up at tick 100: the motion, now none, then the landing whole
/// (`wasOnGround` changed); tick 70's move of nothing (count 60) changes nothing and is
/// not sent late. Tick 130 (count 120), live: a move of nothing.
#[tokio::test]
async fn a_falling_item_is_sent_what_send_changes_sends_in_each_tick() {
    let (mut edge, mut first, _second) = two_at_home().await;
    let key = 1 << 32;
    let start = [0.5, 70.0, 0.5];
    let spawned = vec![RegionEvent::EntitySpawned(item(key, start, 0))];
    let sent = told(&mut edge, &mut first, WEST, 10, spawned).await;
    let id = first.naming(key)[0].1;
    let pos = |delta: [i16; 3], on_ground: bool| {
        ClientboundPlay::MoveEntityPos(MoveEntityPos {
            entity_id: id,
            delta: VecDelta::Linear(delta),
            on_ground,
        })
    };
    // As a client reads it: the protocol keeps 15 bits of each component.
    let motion = |motion: [f64; 3]| {
        let packet = SetEntityMotion {
            entity_id: id,
            motion,
        };
        ClientboundPlay::decode(&packets::encode(&packet)).unwrap()
    };
    assert_eq!(sent, [pos([0; 3], false)]);
    let falling = [0.0, -0.1, 0.0];
    let at = |tick: u64| [0.5, 70.0 - 0.1 * (tick - 10) as f64, 0.5];
    for tick in 11..=40 {
        let report = if tick == 40 {
            tracked(
                key,
                [0.5, 67.0, 0.5],
                [0.0; 3],
                true,
                Tracked::VERTICAL_COLLISION,
            )
        } else {
            tracked(key, at(tick), falling, false, 0)
        };
        let events = vec![
            RegionEvent::BlockEventsBegin,
            RegionEvent::EntityTracked(report),
        ];
        let sent = told(&mut edge, &mut first, WEST, tick, events).await;
        if tick == 30 {
            let down = steps(at(29)[1]) - steps(70.0);
            let delta = [0, i16::try_from(down).unwrap(), 0];
            assert_eq!(sent, [motion(falling), pos(delta, false)]);
        } else {
            assert_eq!(sent, [], "tick {tick}");
        }
    }
    let sent = told(&mut edge, &mut first, WEST, 100, Vec::new()).await;
    let landed = ClientboundPlay::SyncEntityPosition(SyncEntityPosition {
        entity_id: id,
        path: PositionPath::Linear {
            x: 0.5,
            y: 67.0,
            z: 0.5,
        },
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
    });
    assert_eq!(sent, [motion([0.0; 3]), landed]);
    for tick in 101..=130 {
        let sent = told(&mut edge, &mut first, WEST, tick, Vec::new()).await;
        if tick == 130 {
            assert_eq!(sent, [pos([0; 3], true)]);
        } else {
            assert_eq!(sent, [], "tick {tick}");
        }
    }
    // A client shown it now is shown it where it was sent last, as it was sent.
    let mut third = edge.joined(player(3), EntityId(7)).await;
    edge.quiet().await;
    edge.sync(&mut third).await;
    let id = third.naming(key)[0].1;
    let spawn = third
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .unwrap();
    assert_eq!(
        (spawn.x, spawn.y, spawn.z, spawn.velocity),
        (0.5, 67.0, 0.5, [0.0; 3])
    );
    edge.end().await;
}

/// A falling block appears at once where its block was, before the air its block
/// leaves, with its block as the spawn's data and its start at index 8, and is updated
/// after the tick's block changes; landing, its block is shown at once and as a
/// transient block, and then it goes; the landing's change comes again with the next
/// tick's changes, as the game broadcasts it.
#[tokio::test]
async fn a_falling_block_appears_before_its_block_leaves_and_lands_before_it_goes() {
    let (mut edge, mut first, _second) = two_at_home().await;
    let key = (1 << 32) + 1;
    let data = EntityData::FallingBlock {
        state: SAND.0,
        time: 0,
        drop_item: true,
        cancel_drop: false,
    };
    let from = BlockPos::new(3, -55, 3);
    let falling = other(key, [3.5, -55.0, 3.5], 0, data);
    let fell = tracked(key, [3.5, -55.04, 3.5], [0.0, -0.04, 0.0], false, 0);
    let before = first.ids.len();
    let events = vec![
        RegionEvent::EntitySpawned(falling),
        RegionEvent::BlockChanged {
            position: from,
            state: AIR,
            seen: true,
        },
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityTracked(fell),
    ];
    told(&mut edge, &mut first, WEST, 5, events).await;
    assert_eq!(
        kinds_from(&first, before),
        [
            clientbound::ADD_ENTITY,
            clientbound::SET_ENTITY_DATA,
            clientbound::BLOCK_UPDATE,
            clientbound::MOVE_ENTITY_POS,
            // Its start again, as the official server sends a new entity's data.
            clientbound::SET_ENTITY_DATA
        ]
    );
    let id = first.naming(key)[0].1;
    let spawn = first
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .unwrap();
    assert_eq!((spawn.kind, spawn.data), (entity_types::FALLING_BLOCK, 118));
    assert_eq!(spawn.velocity, [0.0; 3]);
    let start = first
        .entity_data
        .iter()
        .find(|data| data.entity_id == id)
        .unwrap();
    let at = Position { x: 3, y: -55, z: 3 };
    assert_eq!(
        start.values,
        [(entity_data::START_POSITION, EntityDataValue::BlockPos(at))]
    );

    let landing = BlockPos::new(3, -60, 3);
    let before = first.ids.len();
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::BlockChanged {
            position: landing,
            state: SAND,
            seen: true,
        },
        RegionEvent::TransientBlock {
            position: landing,
            state: SAND.0,
        },
        RegionEvent::EntityGone {
            entity: EntityId(key),
            chunk: HOME,
            hops: 0,
        },
    ];
    let sent = told(&mut edge, &mut first, WEST, 6, events).await;
    assert_eq!(
        kinds_from(&first, before),
        [
            clientbound::BLOCK_UPDATE,
            clientbound::ADD_TRANSIENT_BLOCK,
            clientbound::REMOVE_ENTITIES
        ]
    );
    let ClientboundPlay::AddTransientBlock(transient) = &sent[0] else {
        panic!("{sent:?}");
    };
    assert_eq!(
        (transient.position, transient.state),
        (Position { x: 3, y: -60, z: 3 }, 118)
    );
    assert!(!first.entities.contains(&key));
    let before = first.ids.len();
    told(&mut edge, &mut first, WEST, 7, Vec::new()).await;
    assert_eq!(kinds_from(&first, before), [clientbound::BLOCK_UPDATE]);
    edge.end().await;
}

/// A pick-up is shown to everyone shown the item, by the item's wire id and the
/// collector's, and then the item goes.
#[tokio::test]
async fn a_pick_up_is_shown_to_everyone_shown_the_item_and_then_it_goes() {
    let (mut edge, mut first, mut second) = two_at_home().await;
    let key = (1 << 32) + 2;
    let spawned = vec![RegionEvent::EntitySpawned(item(key, [1.5, -60.0, 1.5], 0))];
    told(&mut edge, &mut first, WEST, 3, spawned).await;
    edge.sync(&mut second).await;
    let (first_before, second_before) = (first.ids.len(), second.ids.len());
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::ItemTaken {
            item: EntityId(key),
            collector: EntityId(5),
            count: 3,
            chunk: HOME,
        },
        RegionEvent::EntityGone {
            entity: EntityId(key),
            chunk: HOME,
            hops: 0,
        },
    ];
    told(&mut edge, &mut first, WEST, 4, events).await;
    edge.sync(&mut second).await;
    for (client, before, collector) in [
        (&first, first_before, first.own_id()),
        (&second, second_before, second.naming(5)[0].1),
    ] {
        assert_eq!(
            kinds_from(client, before),
            [clientbound::TAKE_ITEM_ENTITY, clientbound::REMOVE_ENTITIES]
        );
        let item_id = client.naming(key)[0].1;
        let taken: Vec<&ClientboundPlay> = client
            .tracking
            .iter()
            .filter(|packet| matches!(packet, ClientboundPlay::TakeItemEntity(_)))
            .collect();
        assert_eq!(
            taken,
            [&ClientboundPlay::TakeItemEntity(Taken {
                item_id,
                collector_id: collector,
                count: 3,
            })]
        );
        assert!(!client.entities.contains(&key));
    }
    edge.end().await;
}

/// ADR-0028, section 6: a removal with fewer changes of holder than the edge shows is
/// passed over; one with as many removes it, and the key is kept with its `hops`, so
/// that a late report with as many does not bring it back, until every region the edge
/// hears from has reported 600 ticks after its first report following the removal.
#[tokio::test]
async fn a_removal_goes_by_hops_and_is_kept_against_late_word_for_600_ticks() {
    let (mut edge, mut first, _second) = two_at_home().await;
    let key = (1 << 32) + 3;
    let at = [2.5, -60.0, 2.5];
    told(
        &mut edge,
        &mut first,
        WEST,
        3,
        vec![RegionEvent::EntitySpawned(item(key, at, 0))],
    )
    .await;
    told(
        &mut edge,
        &mut first,
        EAST,
        3,
        vec![RegionEvent::EntitySpawned(item(key, at, 1))],
    )
    .await;
    let gone = |hops| {
        vec![RegionEvent::EntityGone {
            entity: EntityId(key),
            chunk: HOME,
            hops,
        }]
    };
    told(&mut edge, &mut first, WEST, 4, gone(0)).await;
    assert!(
        first.entities.contains(&key),
        "a removal with fewer is passed over"
    );
    told(&mut edge, &mut first, EAST, 4, gone(1)).await;
    assert!(!first.entities.contains(&key));

    // A late word with as many, of before the removal, does not bring it back.
    let late = vec![RegionEvent::EntitySpawned(item(key, at, 1))];
    for region in REGIONS {
        told(&mut edge, &mut first, region, 10, Vec::new()).await;
    }
    told(&mut edge, &mut first, EAST, 11, late.clone()).await;
    assert!(!first.entities.contains(&key));
    // Every region but the north has reported 600 ticks since: still kept.
    told(&mut edge, &mut first, WEST, 610, Vec::new()).await;
    told(&mut edge, &mut first, EAST, 610, late.clone()).await;
    assert!(!first.entities.contains(&key));
    // Once the north has too, it is forgotten (keys are never given again, so nothing
    // else could have it; the test shows that it is forgotten by a word that would
    // have been passed over before).
    told(&mut edge, &mut first, NORTH, 610, Vec::new()).await;
    told(&mut edge, &mut first, EAST, 611, late).await;
    assert!(first.entities.contains(&key));
    edge.end().await;
}

/// A change of an item's stack and its catching fire are sent in the update after,
/// after the move, the shared flags at index 0 before the stack at index 8.
#[tokio::test]
async fn a_change_of_an_item_s_data_is_sent_in_the_next_update() {
    let (mut edge, mut first, _second) = two_at_home().await;
    let key = (1 << 32) + 4;
    let at = [1.5, -60.0, 1.5];
    told(
        &mut edge,
        &mut first,
        WEST,
        3,
        vec![RegionEvent::EntitySpawned(item(key, at, 0))],
    )
    .await;
    let mut merged = tracked(key, at, [0.0; 3], false, Tracked::ON_FIRE);
    merged.data = Some(EntityData::Item {
        stack: ItemStack { item: 1, count: 9 },
        pick_up_delay: 40,
        thrower: None,
        health: 5,
        age_offset: 0,
    });
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityTracked(merged),
    ];
    told(&mut edge, &mut first, WEST, 4, events).await;
    let before = first.entity_data.len();
    told(&mut edge, &mut first, WEST, 5, Vec::new()).await;
    let id = first.naming(key)[0].1;
    assert_eq!(
        first.entity_data[before..],
        [SetEntityData {
            entity_id: id,
            values: vec![
                (
                    entity_data::SHARED_FLAGS,
                    EntityDataValue::Byte(entity_data::ON_FIRE)
                ),
                (
                    entity_data::ITEM_STACK,
                    EntityDataValue::ItemStack(Some(clustine_protocol::item::ItemStack {
                        item: 1,
                        count: 9
                    }))
                ),
            ],
        }]
    );
    edge.end().await;
}
