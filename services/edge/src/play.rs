//! The play state of one connection.
//!
//! The connection registers with the fan-out task and from then on does three things:
//! it writes the packets the fan-out task queues for it, it passes on what the client
//! sends, and it checks with keep-alives that the client is still there.

use std::sync::atomic::Ordering;

use bytes::Bytes;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::configuration::ClientInformation;
use clustine_protocol::packets::play::{ClientboundKeepAlive, Disconnect, ServerboundPlay};
use clustine_world::PlayerId;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::Shared;
use crate::connection::{Connection, ConnectionError};
use crate::fanout::{Command, SessionId};
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
    let join = Command::Join {
        session,
        profile,
        requested_view_distance: client_information
            .map(|information| i32::from(information.view_distance)),
        outbound,
    };
    if shared.fanout.send(join).await.is_err() {
        return Err(ConnectionError::ShuttingDown);
    }

    let result = pump(connection, shared, session, player, &mut packets).await;
    // Whatever ended the connection, the fan-out task has to forget the player.
    let _ = shared.fanout.send(Command::Leave { session, player }).await;
    result
}

/// Moves packets in both directions until the connection ends.
async fn pump(
    connection: &mut Connection,
    shared: &Shared,
    session: SessionId,
    player: PlayerId,
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
                            session,
                            player,
                            chunks_per_tick: received.chunks_per_tick,
                        };
                        if shared.fanout.send(command).await.is_err() {
                            return Err(ConnectionError::ShuttingDown);
                        }
                    }
                    ServerboundPlay::ConfirmTeleportation(_)
                    | ServerboundPlay::ClientTickEnd(_)
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
