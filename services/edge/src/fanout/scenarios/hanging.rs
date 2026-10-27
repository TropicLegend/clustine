//! Tests of what the edge sends of item frames and paintings (step N5c,
//! `docs/groundwork/n5-plan.md`; `docs/groundwork/n5-brief.md`, sections 6.8, 8.2 and
//! 8.3): a spawn at the block's corner with the direction's 3D value as its data and the
//! values that are not the defaults; nothing at intervals, as `UpdateInterval.NEVER`
//! has it, and only the changed values when a frame's item turns; a painting's moves
//! measured from its block's corner, so that it is never moved; a pick of an entity's
//! item passed to the player's region, and the slot it selects shown.

use clustine_protocol::packet_ids::play::clientbound;
use clustine_sim::api::Tracked;
use clustine_world::Attached;

use super::*;

const STACK: ItemStack = ItemStack::new(1, 1);

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

/// A player at the spawn, shown `HOME` from the west.
async fn at_home() -> (Harness, Client) {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, empty_chunk(), vec![own(1, EntityId(5))]),
    );
    assert!(edge.shows(&mut first, WEST, HOME).await);
    (edge, first)
}

fn frame_data(direction: u8, item: Option<ItemStack>, rotation: u8) -> EntityData {
    EntityData::ItemFrame {
        attached: Attached {
            block: BlockPos::new(2, -59, 3),
            direction,
        },
        item,
        rotation,
        drop_chance: clustine_world::Float(1.0),
        fixed: false,
        invisible: false,
    }
}

/// The hanging entity `key` of the type `kind` with `data`, at `position` (its box's
/// centre, as the region keeps it) and turned `yaw`.
fn hanging(key: u64, kind: i32, position: [f64; 3], yaw: f32, data: EntityData) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: kind as u16,
            uuid: Uuid::from_u128(u128::from(key) + 9_000),
            motion: [0.0; 3],
            hops: 0,
            data,
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw,
            pitch: 0.0,
            on_ground: false,
        },
    }
}

fn report(key: u64, position: [f64; 3], yaw: f32, data: Option<EntityData>) -> Tracked {
    Tracked {
        entity: EntityId(key),
        chunk: HOME,
        hops: 0,
        position,
        motion: [0.0; 3],
        yaw,
        pitch: 0.0,
        on_ground: false,
        flags: 0,
        data,
    }
}

/// The ids of what `client` was sent from its `from`th packet on, of those an entity's
/// showing, data and moves are made of.
fn kinds_from(client: &Client, from: usize) -> Vec<i32> {
    let of_interest = [
        clientbound::ADD_ENTITY,
        clientbound::SET_ENTITY_DATA,
        clientbound::MOVE_ENTITY_POS,
        clientbound::MOVE_ENTITY_POS_ROT,
        clientbound::MOVE_ENTITY_ROT,
        clientbound::SET_ENTITY_MOTION,
        clientbound::ENTITY_POSITION_SYNC,
    ];
    let ids = client.ids[from..].iter().copied();
    ids.filter(|id| of_interest.contains(id)).collect()
}

async fn tick(edge: &mut Harness, client: &mut Client, tick: u64, events: Vec<RegionEvent>) {
    edge.tell(WEST, WorkerToEdge::TickDelta { tick, events });
    edge.settle(WEST).await;
    edge.sync(client).await;
}

/// A frame on the north face of a block, holding a stack turned three eighths: its
/// spawn is at its block's corner with north's 3D value, 2, and its values are the
/// direction, the item and the rotation; a hundred quiet ticks send nothing, and a turn
/// of its item sends the rotation alone.
#[tokio::test]
async fn a_frame_is_spawned_at_its_blocks_corner_and_sent_only_what_changes() {
    let (mut edge, mut first) = at_home().await;
    let key = 1 << 32;
    let centre = [2.5, -58.5, 3.968_75];
    let data = frame_data(2, Some(STACK), 3);
    let spawned = vec![RegionEvent::EntitySpawned(hanging(
        key,
        entity_types::ITEM_FRAME,
        centre,
        180.0,
        data.clone(),
    ))];
    let before = first.ids.len();
    tick(&mut edge, &mut first, 10, spawned).await;
    // Made just now: the update of its first tick, which its data made dirty brings
    // about, sends the move every sixtieth count forces and its values again, as the
    // official server sends a new entity's (`entities_oracle.rs`).
    assert_eq!(
        kinds_from(&first, before),
        [
            clientbound::ADD_ENTITY,
            clientbound::SET_ENTITY_DATA,
            clientbound::MOVE_ENTITY_POS,
            clientbound::SET_ENTITY_DATA
        ]
    );
    let id = first.naming(key)[0].1;
    let spawn = first
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .unwrap();
    assert_eq!(spawn.kind, entity_types::ITEM_FRAME);
    assert_eq!((spawn.x, spawn.y, spawn.z), (2.0, -59.0, 3.0));
    assert_eq!(spawn.data, 2);
    assert_eq!((spawn.yaw, spawn.pitch, spawn.velocity), (128, 0, [0.0; 3]));
    let values = &first.entity_data.last().unwrap().values;
    assert_eq!(
        *values,
        [
            (8, EntityDataValue::Direction(2)),
            (
                9,
                EntityDataValue::ItemStack(Some(clustine_protocol::item::ItemStack {
                    item: 1,
                    count: 1
                }))
            ),
            (10, EntityDataValue::Int(3)),
        ]
    );
    // `UpdateInterval.NEVER`: no tick is an update tick, the sixtieth neither.
    let before = first.ids.len();
    for at in 11..=110 {
        tick(&mut edge, &mut first, at, Vec::new()).await;
    }
    assert!(kinds_from(&first, before).is_empty());
    // Turned once more: the rotation, and nothing else.
    let turned = frame_data(2, Some(STACK), 4);
    let events = vec![RegionEvent::EntityTracked(report(
        key,
        centre,
        180.0,
        Some(turned),
    ))];
    let before = first.ids.len();
    tick(&mut edge, &mut first, 111, events).await;
    assert_eq!(kinds_from(&first, before), [clientbound::SET_ENTITY_DATA]);
    assert_eq!(
        first.entity_data.last().unwrap().values,
        [(10, EntityDataValue::Int(4))]
    );
    // Its item taken out: the empty slot, and nothing else.
    let emptied = frame_data(2, None, 4);
    let events = vec![RegionEvent::EntityTracked(report(
        key,
        centre,
        180.0,
        Some(emptied),
    ))];
    tick(&mut edge, &mut first, 112, events).await;
    assert_eq!(
        first.entity_data.last().unwrap().values,
        [(9, EntityDataValue::ItemStack(None))]
    );
    edge.end().await;
}

/// An empty frame facing south has every value at its default: it is spawned with no
/// data packet.
#[tokio::test]
async fn an_empty_frame_facing_south_is_spawned_without_data() {
    let (mut edge, mut first) = at_home().await;
    let key = (1 << 32) + 1;
    let spawned = vec![RegionEvent::EntitySpawned(hanging(
        key,
        entity_types::GLOW_ITEM_FRAME,
        [2.5, -58.5, 3.031_25],
        0.0,
        frame_data(3, None, 0),
    ))];
    let before = first.ids.len();
    tick(&mut edge, &mut first, 10, spawned).await;
    assert_eq!(kinds_from(&first, before), [clientbound::ADD_ENTITY]);
    let id = first.naming(key)[0].1;
    let spawn = first
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .unwrap();
    assert_eq!((spawn.kind, spawn.data), (entity_types::GLOW_ITEM_FRAME, 3));
    edge.end().await;
}

/// A 2 by 1 painting facing west: its centre is half a block from its block, but its
/// spawn is at its block's corner, its moves are measured from there, and so a report
/// of where it is moves nothing; its values are its direction and its variant.
#[tokio::test]
async fn a_painting_is_spawned_at_its_blocks_corner_and_never_moved() {
    let (mut edge, mut first) = at_home().await;
    let key = (1 << 32) + 2;
    let data = EntityData::Painting {
        attached: Attached {
            block: BlockPos::new(4, -59, 4),
            direction: 4,
        },
        // courbet, 2 by 1.
        variant: 12,
    };
    let centre = [4.968_75, -58.5, 5.0];
    let spawned = vec![RegionEvent::EntitySpawned(hanging(
        key,
        entity_types::PAINTING,
        centre,
        90.0,
        data.clone(),
    ))];
    tick(&mut edge, &mut first, 10, spawned).await;
    let id = first.naming(key)[0].1;
    let spawn = first
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .unwrap();
    assert_eq!((spawn.x, spawn.y, spawn.z), (4.0, -59.0, 4.0));
    assert_eq!(
        (spawn.kind, spawn.data, spawn.yaw),
        (entity_types::PAINTING, 4, 64)
    );
    assert_eq!(
        first.entity_data.last().unwrap().values,
        [
            (8, EntityDataValue::Direction(4)),
            (9, EntityDataValue::PaintingVariant(12)),
        ]
    );
    let before = first.ids.len();
    let events = vec![RegionEvent::EntityTracked(report(key, centre, 90.0, None))];
    tick(&mut edge, &mut first, 11, events).await;
    for at in 12..=80 {
        tick(&mut edge, &mut first, at, Vec::new()).await;
    }
    assert!(kinds_from(&first, before).is_empty());
    edge.end().await;
}

/// A middle click on an entity the client was shown goes to the player's region as a
/// pick of that entity by its key, and the slot the region selects is shown.
#[tokio::test]
async fn a_pick_of_an_entity_goes_to_the_players_region_and_the_slot_it_selects_is_shown() {
    let (mut edge, mut first) = at_home().await;
    let key = (1 << 32) + 3;
    let spawned = vec![RegionEvent::EntitySpawned(hanging(
        key,
        entity_types::ITEM_FRAME,
        [2.5, -58.5, 3.968_75],
        180.0,
        frame_data(2, None, 0),
    ))];
    tick(&mut edge, &mut first, 10, spawned).await;
    let id = first.naming(key)[0].1;
    edge.said(WEST).await;
    let command = Command::OnEntity {
        session: first.session,
        player: first.player,
        id,
        what: OnEntity::Pick,
    };
    edge.commands.send(command).await.unwrap();
    edge.drained().await;
    let said = edge.said(WEST).await;
    let picked = said.iter().any(|message| {
        matches!(
            &message.body,
            EdgeToWorker::Input {
                input: PlayerInput::PickItemFromEntity { entity },
                ..
            } if *entity == EntityId(key)
        )
    });
    assert!(picked, "{said:?}");
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::HeldSlot { slot: 6 },
        },
    );
    edge.settle(WEST).await;
    let mut held = None;
    for _ in 0..16 {
        let Some(packet) = edge.packet(&mut first).await else {
            break;
        };
        if let Ok(ClientboundPlay::SetHeldSlot(slot)) = ClientboundPlay::decode(&packet) {
            held = Some(slot.slot);
            break;
        }
        first.take(packet);
    }
    assert_eq!(held, Some(6));
    edge.end().await;
}
