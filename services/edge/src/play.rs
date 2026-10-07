//! The play state of one connection.
//!
//! The connection registers with the fan-out task and from then on does three things:
//! it writes the packets the fan-out task queues for it, it passes on what the client
//! sends, and it checks with keep-alives that the client is still there.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use bytes::Bytes;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::configuration::ClientInformation;
use clustine_protocol::packets::play::{
    AcknowledgeBlockChange, ClientboundKeepAlive, Disconnect, PlayerAction, ServerboundPlay,
    movement_flags, player_action,
};
use clustine_rpc::EdgeToWorker;
use clustine_sim::api::PlayerInput;
use clustine_world::{BlockPos, PlayerId, Vec3};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::Shared;
use crate::connection::{Connection, ConnectionError};
use crate::fanout::{Command, NO_TELEPORT, SessionId};
use crate::login::Profile;

/// Packets that may wait for a client before it is considered too slow and dropped.
const OUTBOUND_CAPACITY: usize = 2048;

pub(crate) async fn serve(
    connection: &mut Connection,
    shared: &Shared,
    profile: Profile,
    client_information: Option<ClientInformation>,
) -> Result<(), ConnectionError> {
    let session = SessionId(shared.next_session.fetch_add(1, Ordering::Relaxed));
    let player = PlayerId(profile.uuid);
    let (outbound, mut packets) = mpsc::channel(OUTBOUND_CAPACITY);
    let awaiting_teleport = Arc::new(AtomicI32::new(NO_TELEPORT));
    let join = Command::Join {
        session,
        profile,
        requested_view_distance: client_information
            .map(|information| i32::from(information.view_distance)),
        outbound,
        awaiting_teleport: Arc::clone(&awaiting_teleport),
    };
    if shared.fanout.send(join).await.is_err() {
        return Err(ConnectionError::ShuttingDown);
    }

    let client = Client {
        session,
        player,
        awaiting_teleport,
    };
    let result = pump(connection, shared, &client, &mut packets).await;
    // Whatever ended the connection, the fan-out task has to forget the player.
    let leave = Command::Leave { session, player };
    let _ = shared.fanout.send(leave).await;
    result
}

/// What the connection knows about its player while in the play state.
struct Client {
    session: SessionId,
    player: PlayerId,
    /// Set by the fan-out task; see [`NO_TELEPORT`].
    awaiting_teleport: Arc<AtomicI32>,
}

/// Moves packets in both directions until the connection ends.
async fn pump(
    connection: &mut Connection,
    shared: &Shared,
    client: &Client,
    packets: &mut mpsc::Receiver<Bytes>,
) -> Result<(), ConnectionError> {
    let interval = shared.config.keep_alive_interval;
    let mut keep_alive = interval_at(Instant::now() + interval, interval);
    keep_alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut next_keep_alive_id = 0;
    let mut awaiting_keep_alive = None;
    // The client only understands keep-alives once it has been put into the world.
    let mut in_world = false;

    loop {
        tokio::select! {
            frame = connection.read_frame() => {
                let Some(frame) = frame? else {
                    return Ok(());
                };
                match ServerboundPlay::decode(&frame)? {
                    ServerboundPlay::ServerboundKeepAlive(answer) => {
                        if awaiting_keep_alive != Some(answer.id) {
                            return Err(ConnectionError::Protocol("unexpected keep-alive id"));
                        }
                        awaiting_keep_alive = None;
                    }
                    ServerboundPlay::ChunkBatchReceived(received) => {
                        let command = Command::ChunkBatchReceived {
                            session: client.session,
                            player: client.player,
                            chunks_per_tick: received.chunks_per_tick,
                        };
                        if shared.fanout.send(command).await.is_err() {
                            return Err(ConnectionError::ShuttingDown);
                        }
                    }
                    ServerboundPlay::ConfirmTeleportation(confirmation) => {
                        // Only the teleport sent last counts; an older id changes nothing.
                        let _ = client.awaiting_teleport.compare_exchange(
                            confirmation.teleport_id,
                            NO_TELEPORT,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        );
                    }
                    ServerboundPlay::SetPlayerPosition(packet) => {
                        let position = Vec3::new(packet.x, packet.y, packet.z);
                        moved(shared, client, Some(position), None, packet.flags).await?;
                    }
                    ServerboundPlay::SetPlayerPositionAndRotation(packet) => {
                        let position = Vec3::new(packet.x, packet.y, packet.z);
                        let rotation = (packet.yaw, packet.pitch);
                        moved(shared, client, Some(position), Some(rotation), packet.flags).await?;
                    }
                    ServerboundPlay::SetPlayerRotation(packet) => {
                        let rotation = (packet.yaw, packet.pitch);
                        moved(shared, client, None, Some(rotation), packet.flags).await?;
                    }
                    ServerboundPlay::SetPlayerMovementFlags(packet) => {
                        moved(shared, client, None, None, packet.flags).await?;
                    }
                    ServerboundPlay::PlayerAction(action) => {
                        acted(connection, shared, client, action).await?;
                    }
                    ServerboundPlay::ClientTickEnd(_)
                    | ServerboundPlay::PlayerLoaded(_)
                    | ServerboundPlay::Unhandled { .. } => {}
                }
            }
            packet = packets.recv() => {
                let Some(packet) = packet else {
                    // The fan-out task dropped the player: it was refused, too slow, or
                    // the server is stopping. What it queued before has been written.
                    connection.close_gracefully().await;
                    return Ok(());
                };
                connection.queue_encoded(&packet)?;
                // Write everything that is ready in one go.
                while let Ok(packet) = packets.try_recv() {
                    connection.queue_encoded(&packet)?;
                }
                connection.flush().await?;
                in_world = true;
            }
            _ = keep_alive.tick(), if in_world => {
                if awaiting_keep_alive.is_some() {
                    let reason = Nbt::String("Timed out".to_owned());
                    connection.write(&Disconnect { reason }).await?;
                    connection.close_gracefully().await;
                    return Ok(());
                }
                next_keep_alive_id += 1;
                awaiting_keep_alive = Some(next_keep_alive_id);
                connection.write(&ClientboundKeepAlive { id: next_keep_alive_id }).await?;
            }
        }
    }
}

/// Passes a movement of the player on to the worker.
async fn moved(
    shared: &Shared,
    client: &Client,
    position: Option<Vec3>,
    rotation: Option<(f32, f32)>,
    flags: u8,
) -> Result<(), ConnectionError> {
    // Like vanilla, a client that sends something that is not a number is cut off.
    let numbers = position
        .iter()
        .flat_map(|position| [position.x, position.y, position.z])
        .chain(
            rotation
                .iter()
                .flat_map(|(yaw, pitch)| [f64::from(*yaw), f64::from(*pitch)]),
        );
    if !numbers.into_iter().all(f64::is_finite) {
        return Err(ConnectionError::Protocol("movement that is not a number"));
    }
    // Positions from before a teleport the client has not confirmed are out of date.
    if client.awaiting_teleport.load(Ordering::Relaxed) != NO_TELEPORT {
        return Ok(());
    }
    let input = PlayerInput::Move {
        position,
        rotation,
        on_ground: flags & movement_flags::ON_GROUND != 0,
    };
    send_input(shared, client, input).await
}

/// Handles something the player did with their hands.
async fn acted(
    connection: &mut Connection,
    shared: &Shared,
    client: &Client,
    action: PlayerAction,
) -> Result<(), ConnectionError> {
    match action.status {
        // In creative mode, starting to break a block breaks it.
        player_action::START_DESTROY_BLOCK => {
            let input = PlayerInput::Dig {
                position: BlockPos::new(action.position.x, action.position.y, action.position.z),
                sequence: action.sequence,
            };
            send_input(shared, client, input).await
        }
        // The other ways of breaking a block only exist in survival mode. The client
        // still counts them among its guesses and waits to hear they were handled.
        player_action::ABORT_DESTROY_BLOCK | player_action::STOP_DESTROY_BLOCK => {
            let acknowledgement = AcknowledgeBlockChange {
                sequence: action.sequence,
            };
            connection.write(&acknowledgement).await
        }
        // Dropping and using items does not exist yet.
        _ => Ok(()),
    }
}

async fn send_input(
    shared: &Shared,
    client: &Client,
    input: PlayerInput,
) -> Result<(), ConnectionError> {
    let message = EdgeToWorker::Input {
        player: client.player,
        input,
    };
    // Waiting here when the worker is busy slows down reading from this client only.
    shared
        .worker
        .send(message)
        .await
        .map_err(|_| ConnectionError::ShuttingDown)
}
