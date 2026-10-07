//! The login state: establishing who the player is.
//!
//! Only offline mode exists so far: the name the client claims is accepted without
//! authentication, which is why the server listens on localhost by default.

use clustine_data::{GAME_VERSION, PROTOCOL_VERSION};
use clustine_protocol::packets::handshake::Intention;
use clustine_protocol::packets::login::{
    LoginDisconnect, LoginStart, LoginSuccess, MAX_NAME_LENGTH, ServerboundLogin,
};
use md5::{Digest, Md5};
use serde_json::json;
use uuid::Uuid;

use crate::connection::{Connection, ConnectionError};
use crate::{Shared, configuration};

/// Who a connection belongs to, once login has succeeded.
#[derive(Debug, Clone)]
pub(crate) struct Profile {
    pub(crate) uuid: Uuid,
    pub(crate) name: String,
}

pub(crate) async fn serve(
    connection: &mut Connection,
    shared: &Shared,
    intention: &Intention,
) -> Result<(), ConnectionError> {
    if intention.protocol_version != PROTOCOL_VERSION {
        let reason = format!("This server runs Minecraft {GAME_VERSION}.");
        return refuse(connection, &reason).await;
    }

    let Some(frame) = connection.read_frame().await? else {
        return Ok(());
    };
    let ServerboundLogin::LoginStart(LoginStart { name, .. }) = ServerboundLogin::decode(&frame)?
    else {
        return Err(ConnectionError::Protocol("expected a login start"));
    };
    if !is_valid_name(&name) {
        return refuse(connection, "Invalid player name.").await;
    }
    let profile = Profile {
        uuid: offline_uuid(&name),
        name,
    };

    connection
        .write(&LoginSuccess {
            uuid: profile.uuid,
            name: profile.name.clone(),
            properties: Vec::new(),
            session_id: Uuid::new_v4(),
        })
        .await?;
    let Some(frame) = connection.read_frame().await? else {
        return Ok(());
    };
    let ServerboundLogin::LoginAcknowledged(_) = ServerboundLogin::decode(&frame)? else {
        return Err(ConnectionError::Protocol(
            "expected a login acknowledgement",
        ));
    };

    configuration::serve(connection, shared, profile).await
}

async fn refuse(connection: &mut Connection, reason: &str) -> Result<(), ConnectionError> {
    let reason_json = json!({ "text": reason }).to_string();
    connection.write(&LoginDisconnect { reason_json }).await?;
    connection.close_gracefully().await;
    Ok(())
}

/// Whether `name` could be the name of a Minecraft account.
fn is_valid_name(name: &str) -> bool {
    (1..=MAX_NAME_LENGTH).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The UUID an offline-mode vanilla server gives the player called `name`: a
/// name-based (version 3) UUID of `OfflinePlayer:<name>` without a namespace.
fn offline_uuid(name: &str) -> Uuid {
    let digest = Md5::digest(format!("OfflinePlayer:{name}"));
    uuid::Builder::from_md5_bytes(digest.into()).into_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UUID the official 26.3 server assigned to a player named Notch in offline mode.
    #[test]
    fn offline_uuid_matches_vanilla() {
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn names_are_validated() {
        assert!(is_valid_name("Notch"));
        assert!(is_valid_name("a_1"));
        assert!(is_valid_name(&"a".repeat(16)));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name(&"a".repeat(17)));
        assert!(!is_valid_name("two words"));
        assert!(!is_valid_name("naïve"));
    }
}
