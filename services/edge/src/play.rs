//! The play state.
//!
//! There is no world yet: the player is placed at a fixed position and the connection is
//! kept alive. This becomes the bridge to the worker once regions exist.

use std::sync::atomic::Ordering;
use std::time::Duration;

use clustine_data::synced_registry;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::configuration::ClientInformation;
use clustine_protocol::packets::play::{
    ClientboundKeepAlive, Disconnect, GameEvent, Login, PlayerAbilities, ServerboundPlay,
    SetCenterChunk, SynchronizePlayerPosition, game_event, game_mode,
};
use tokio::time::{Instant, MissedTickBehavior, interval_at};
use tracing::info;

use crate::Shared;
use crate::connection::{Connection, ConnectionError};
use crate::login::Profile;

const OVERWORLD: &str = "minecraft:overworld";

/// Chunks the client is told to expect around the player in each direction.
const VIEW_DISTANCE: i32 = 8;

/// Player ability flags of creative mode: invulnerable, may fly, breaks blocks instantly.
const CREATIVE_ABILITIES: u8 = 0x01 | 0x04 | 0x08;

pub(crate) async fn serve(
    connection: &mut Connection,
    shared: &Shared,
    profile: Profile,
    _client_information: Option<ClientInformation>,
) -> Result<(), ConnectionError> {
    let entity_id = shared.next_entity_id.fetch_add(1, Ordering::Relaxed);
    let dimension_type = synced_registry("minecraft:dimension_type")
        .and_then(|registry| registry.id_of(OVERWORLD))
        .expect("the overworld is a vanilla dimension type");

    connection.queue(&Login {
        entity_id,
        hardcore: false,
        dimension_names: vec![OVERWORLD.to_owned()],
        max_players: shared.config.max_players as i32,
        view_distance: VIEW_DISTANCE,
        simulation_distance: VIEW_DISTANCE,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        limited_crafting: false,
        dimension_type,
        dimension_name: OVERWORLD.to_owned(),
        hashed_seed: 0,
        game_mode: game_mode::CREATIVE,
        previous_game_mode: None,
        is_debug: false,
        is_flat: true,
        death_location: None,
        portal_cooldown: 0,
        sea_level: -63,
        online_mode: false,
        enforces_secure_chat: false,
    })?;
    connection.queue(&PlayerAbilities {
        flags: CREATIVE_ABILITIES,
        flying_speed: 0.05,
        field_of_view_modifier: 0.1,
    })?;
    connection.queue(&SynchronizePlayerPosition {
        teleport_id: 1,
        x: 0.5,
        y: -60.0,
        z: 0.5,
        velocity_x: 0.0,
        velocity_y: 0.0,
        velocity_z: 0.0,
        yaw: 0.0,
        pitch: 0.0,
        relative_flags: 0,
    })?;
    connection.queue(&GameEvent {
        event: game_event::START_WAITING_FOR_LEVEL_CHUNKS,
        value: 0.0,
    })?;
    connection.queue(&SetCenterChunk {
        chunk_x: 0,
        chunk_z: 0,
    })?;
    connection.flush().await?;
    info!(name = %profile.name, uuid = %profile.uuid, entity_id, "player joined");

    let result = keep_alive(connection, shared.config.keep_alive_interval).await;
    info!(name = %profile.name, "player left");
    result
}

/// Serves the connection until the client leaves or stops answering keep-alives.
async fn keep_alive(
    connection: &mut Connection,
    interval: Duration,
) -> Result<(), ConnectionError> {
    let mut ticks = interval_at(Instant::now() + interval, interval);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut next_id = 0;
    let mut awaiting = None;
    loop {
        tokio::select! {
            frame = connection.read_frame() => {
                let Some(frame) = frame? else {
                    return Ok(());
                };
                match ServerboundPlay::decode(&frame)? {
                    ServerboundPlay::ServerboundKeepAlive(answer) => {
                        if awaiting != Some(answer.id) {
                            return Err(ConnectionError::Protocol("unexpected keep-alive id"));
                        }
                        awaiting = None;
                    }
                    ServerboundPlay::ConfirmTeleportation(_)
                    | ServerboundPlay::ClientTickEnd(_)
                    | ServerboundPlay::ChunkBatchReceived(_)
                    | ServerboundPlay::Unhandled { .. } => {}
                }
            }
            _ = ticks.tick() => {
                if awaiting.is_some() {
                    let reason = Nbt::String("Timed out".to_owned());
                    connection.write(&Disconnect { reason }).await?;
                    connection.close_gracefully().await;
                    return Ok(());
                }
                next_id += 1;
                awaiting = Some(next_id);
                connection.write(&ClientboundKeepAlive { id: next_id }).await?;
            }
        }
    }
}
