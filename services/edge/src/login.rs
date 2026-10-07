//! The login state.

use clustine_data::{GAME_VERSION, PROTOCOL_VERSION};
use clustine_protocol::packets::handshake::Intention;
use clustine_protocol::packets::login::LoginDisconnect;
use serde_json::json;

use crate::connection::{Connection, ConnectionError};

pub(crate) async fn serve(
    connection: &mut Connection,
    intention: &Intention,
) -> Result<(), ConnectionError> {
    let reason = if intention.protocol_version == PROTOCOL_VERSION {
        "Clustine does not accept players yet.".to_owned()
    } else {
        format!("This server runs Minecraft {GAME_VERSION}.")
    };
    let reason_json = json!({ "text": reason }).to_string();
    connection.write(&LoginDisconnect { reason_json }).await?;
    connection.close_gracefully().await;
    Ok(())
}
