//! The status state: answering the server list ping.

use std::sync::atomic::Ordering;

use clustine_data::{GAME_VERSION, PROTOCOL_VERSION};
use clustine_protocol::packets::status::{PongResponse, ServerboundStatus, StatusResponse};
use serde_json::json;

use crate::Shared;
use crate::connection::{Connection, ConnectionError};

pub(crate) async fn serve(
    connection: &mut Connection,
    shared: &Shared,
) -> Result<(), ConnectionError> {
    let mut answered = false;
    loop {
        let Some(frame) = connection.read_frame().await? else {
            return Ok(());
        };
        match ServerboundStatus::decode(&frame)? {
            ServerboundStatus::StatusRequest(_) if !answered => {
                answered = true;
                let json = status_json(shared);
                connection.write(&StatusResponse { json }).await?;
            }
            ServerboundStatus::StatusRequest(_) => {
                return Err(ConnectionError::Protocol("status requested twice"));
            }
            // The ping ends the exchange, whether or not the status was requested first.
            ServerboundStatus::PingRequest(ping) => {
                let pong = PongResponse {
                    payload: ping.payload,
                };
                return connection.write(&pong).await;
            }
            ServerboundStatus::Unhandled { .. } => {
                return Err(ConnectionError::Protocol("unexpected packet in status"));
            }
        }
    }
}

fn status_json(shared: &Shared) -> String {
    json!({
        "version": { "name": GAME_VERSION, "protocol": PROTOCOL_VERSION },
        "players": {
            "max": shared.config.max_players,
            "online": shared.online.load(Ordering::Relaxed),
        },
        "description": { "text": shared.config.description },
        "enforcesSecureChat": false,
    })
    .to_string()
}
