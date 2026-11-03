//! Tests of what the edge sends a client of the End's gateways, written from the
//! specification by someone who did not write the edge's handling of them:
//! `docs/groundwork/p-plan.md`, section 19.8 (where the traveller comes out) and its
//! review (the sixth point), the brief's section 2.6, and the game's
//! `ServerPlayer.teleport(TeleportTransition)` (`d-javap` ServerPlayer.txt 2462-2503:
//! within one level, the teleport alone) with `EndGatewayBlock.getPortalDestination`'s
//! transition (EndGatewayBlock.txt 72-109: `DELTA ∪ ROTATION`, yaw and pitch 0).
//!
//! A gateway changes no level: no `Respawn`, no level's information, no wait for chunks
//! and no portal sound; the client's moves are held while the traveller is between
//! regions, and the placing keeps its velocity and turns its look by the turn from the
//! look it left with. The beam is the block event (1, 0) of `triggerCooldown`.
//!
//! These tests' regions are of the overworld; the edge goes by the travel's way, not by
//! the level's name, so a gateway's travel between two of them stands for one in the
//! End.

use clustine_protocol::packet_ids::play::clientbound as ids;
use clustine_world::{Travel, Way};

use super::*;

/// A gateway in the chunk the player stands in.
const GATEWAY: BlockPos = BlockPos::new(3, -59, 5);

/// Every relative bit of `DELTA ∪ ROTATION`: the velocity kept (`Relative.DELTA`, with
/// `ROTATE_DELTA`) and the look kept (`Relative.ROTATION`).
const DELTA_AND_ROTATION: i32 = relative::DELTA_X
    | relative::DELTA_Y
    | relative::DELTA_Z
    | relative::ROTATE_DELTA
    | relative::Y_ROT
    | relative::X_ROT;

/// Player 1 as the west lets them go through a gateway to the east, looking `look`.
fn through_a_gateway(look: (f32, f32)) -> PlayerTransfer {
    let mut moved = transfer(EntityId(5), 0, EASTERN);
    moved.pose.yaw = look.0;
    moved.pose.pitch = look.1;
    moved.travel = Some(Travel {
        to: moved.dimension,
        target: block_of(EASTERN),
        way: Way::Gateway {
            from: GATEWAY,
            exact: false,
            island: None,
        },
    });
    moved
}

/// The placing of player 1 by `region` at `at`, looking `look`.
fn placed_at(at: Vec3, look: (f32, f32)) -> Durable {
    Durable::Placed {
        player: player(1),
        entity: EntityId(5),
        dimension: Dimension::Overworld,
        pose: Pose {
            position: at,
            yaw: look.0,
            pitch: look.1,
            on_ground: true,
        },
        flying: false,
        respawn: None,
        gateway: true,
    }
}

/// Serves `region`'s chunks of the view about `centre`, as far as the edge asks.
fn serve(edge: &mut Harness, region: RegionId, centre: ChunkPos) {
    for chunk in view(centre) {
        if edge.at(region).subscriptions.contains_key(&chunk) {
            let ask = edge.ask(region, chunk);
            edge.tell(region, snapshot(chunk, ask));
        }
    }
}

/// Player 1 standing in the west, let go through a gateway to the east, looking
/// (30, 10): the client's packets up to then counted, and whether the east was told of
/// the traveller.
async fn gone_through_to_the_east() -> (Harness, Client, usize, bool) {
    let (mut edge, mut client) = standing_in_the_west().await;
    let from = client.ids.len();
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: through_a_gateway((30.0, 10.0)),
            to: EAST,
        },
    );
    edge.settle(WEST).await;
    let arrived = edge
        .said(EAST)
        .await
        .into_iter()
        .any(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }));
    edge.quiet().await;
    client.drain();
    (edge, client, from, arrived)
}

/// The review's sixth point: a gateway's traveller handed to another region of the
/// level is sent no `Respawn` and nothing of a change of level, and its moves are held
/// while it is between regions; the region it goes to is told of it.
#[tokio::test]
async fn a_gateway_s_traveller_handed_over_is_sent_no_respawn_and_their_moves_are_held() {
    let (edge, client, from, arrived) = gone_through_to_the_east().await;
    assert_eq!(client.respawns, [], "a gateway changes no level");
    let after = &client.ids[from..];
    for id in [
        ids::RESPAWN,
        ids::CHANGE_DIFFICULTY,
        ids::SET_DEFAULT_SPAWN_POSITION,
    ] {
        assert!(!after.contains(&id), "{id} in {after:?}");
    }
    assert_eq!(
        client.awaiting.load(Ordering::Relaxed),
        TRAVELLING,
        "the moves are held"
    );
    assert!(arrived, "the east is told of the traveller");
    edge.end().await;
}

/// The review's sixth point and `ServerPlayer.teleport` within one level (offsets
/// 61-103): the placing is the teleport alone, relative by `DELTA ∪ ROTATION`, its look
/// the turn from the look the traveller left with; no level's information, no wait for
/// chunks, no portal sound. The moves wait for the client to accept it, and the view
/// about the exit follows.
#[tokio::test]
async fn a_gateway_s_traveller_is_placed_keeping_their_velocity_with_their_look_turned() {
    let (mut edge, mut client, _, _) = gone_through_to_the_east().await;
    serve(&mut edge, EAST, HOME);
    edge.quiet().await;
    client.drain();
    let (teleports, from) = (client.teleports.len(), client.ids.len());
    let at = within(EASTERN);
    edge.say(EAST, placed_at(at, (75.0, 10.0)));
    edge.settle(EAST).await;
    edge.quiet().await;
    serve(&mut edge, EAST, EASTERN);
    edge.sync(&mut client).await;
    let teleport = &client.teleports[teleports];
    assert_eq!((teleport.x, teleport.y, teleport.z), (at.x, at.y, at.z));
    assert_eq!(teleport.relative_flags, DELTA_AND_ROTATION);
    assert_eq!((teleport.yaw, teleport.pitch), (45.0, 0.0), "the turn");
    assert_eq!(
        (
            teleport.velocity_x,
            teleport.velocity_y,
            teleport.velocity_z
        ),
        (0.0, 0.0, 0.0),
        "nothing added to the velocity the client keeps"
    );
    assert_eq!(
        client.awaiting.load(Ordering::Relaxed),
        teleport.teleport_id,
        "the moves wait for the client to accept the placing"
    );
    let after = &client.ids[from..];
    assert_eq!(after.first(), Some(&ids::PLAYER_POSITION), "{after:?}");
    for id in [
        ids::RESPAWN,
        ids::SET_DEFAULT_SPAWN_POSITION,
        ids::GAME_EVENT,
        ids::CHANGE_DIFFICULTY,
    ] {
        assert!(!after.contains(&id), "{id} in {after:?}");
    }
    assert!(
        !client
            .level_events
            .iter()
            .any(|event| event.event == level_event::PORTAL_TRAVEL),
        "no portal sound"
    );
    assert_eq!(client.respawns, []);
    assert_eq!(
        client.chunks.keys().copied().collect::<BTreeSet<_>>(),
        view(EASTERN),
        "the view about the exit"
    );
    edge.end().await;
}

/// 19.8, "Where the traveller comes out": in an End of one region the traveller is
/// placed by the region it is in, with a `Placed` and no departure, and a player is
/// placed with `DELTA ∪ ROTATION` (the client keeps its velocity and look) as through
/// any gateway: the look the region kept is no turn at all.
#[tokio::test]
async fn a_gateway_s_traveller_placed_by_the_region_they_are_in_keeps_their_look() {
    let (mut edge, mut client) = standing_in_the_west().await;
    let teleports = client.teleports.len();
    let at = Vec3::new(9.5, -60.0, 10.5);
    edge.say(WEST, placed_at(at, (30.0, 10.0)));
    edge.settle(WEST).await;
    edge.quiet().await;
    serve(&mut edge, WEST, HOME);
    edge.sync(&mut client).await;
    let teleport = &client.teleports[teleports];
    assert_eq!((teleport.x, teleport.y, teleport.z), (at.x, at.y, at.z));
    assert_eq!(client.respawns, []);
    assert_eq!(teleport.relative_flags, DELTA_AND_ROTATION);
    assert_eq!((teleport.yaw, teleport.pitch), (0.0, 0.0));
    edge.end().await;
}

/// `triggerCooldown` (TheEndGatewayBlockEntity 230-249): `level.blockEvent(pos, block,
/// 1, 0)`, which a client near the gateway is sent as a `BlockEvent` with action 1 and
/// parameter 0 of the gateway's block, and draws the beam by.
#[tokio::test]
async fn the_beam_s_block_event_is_sent_to_a_player_near_the_gateway() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let own = EntityState {
        entity: EntityId(5),
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(1),
            name: "Player1".to_owned(),
        },
        pose: Pose::at(SPAWN),
    };
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), vec![own]));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    client.heard.clear();
    let block = clustine_data::blocks::END_GATEWAY
        .block_id()
        .and_then(|id| u16::try_from(id).ok())
        .expect("the gateway's block");
    edge.tell(
        WEST,
        WorkerToEdge::TickDelta {
            tick: 2,
            events: vec![
                RegionEvent::BlockEventsBegin,
                RegionEvent::BlockEvent {
                    position: GATEWAY,
                    block,
                    kind: 1,
                    data: 0,
                },
            ],
        },
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let beam = Received::Event(BlockEvent {
        position: Position {
            x: GATEWAY.x,
            y: GATEWAY.y,
            z: GATEWAY.z,
        },
        action: 1,
        param: 0,
        block: i32::from(block),
    });
    assert_eq!(client.heard, [beam]);
    edge.end().await;
}
