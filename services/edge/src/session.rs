//! The start of every connection: the handshake, then the state the client asked for.

use clustine_protocol::packets::configuration::ClientInformation;
use clustine_protocol::packets::handshake::{Intent, ServerboundHandshake};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::connection::{Connection, ConnectionError};
use crate::login::Profile;
use crate::{Shared, configuration, login, play, status};

/// First byte of the server list ping used before Minecraft 1.7.
const LEGACY_PING: u8 = 0xFE;

pub(crate) async fn serve(stream: TcpStream, shared: &Shared) -> Result<(), ConnectionError> {
    let limit = shared.config.client_timeout;
    let mut connection = Connection::new(stream, limit);
    // Everything before the play state takes a client a fraction of a second. The limit
    // covers all of it, so that sending a byte now and then does not keep a connection
    // open for ever.
    let entered = timeout(limit, before_play(&mut connection, shared))
        .await
        .map_err(|_| ConnectionError::TimedOut)??;
    match entered {
        Some((profile, client_information)) => {
            play::serve(&mut connection, shared, profile, client_information).await
        }
        None => Ok(()),
    }
}

/// Serves a connection up to the play state. Returns who the player is if the client
/// logged in and got that far, and `None` if the connection ended before, as it does
/// for the server list ping.
async fn before_play(
    connection: &mut Connection,
    shared: &Shared,
) -> Result<Option<(Profile, Option<ClientInformation>)>, ConnectionError> {
    // Current clients fall back to the legacy ping when the modern one fails. It is not a
    // framed packet, so it has to be recognised before framing starts. Closing at once
    // makes the client give up instead of waiting for a timeout.
    if matches!(connection.first_byte().await?, None | Some(LEGACY_PING)) {
        return Ok(None);
    }

    let Some(frame) = connection.read_frame().await? else {
        return Ok(None);
    };
    let ServerboundHandshake::Intention(intention) = ServerboundHandshake::decode(&frame)? else {
        return Err(ConnectionError::Protocol("expected a handshake"));
    };
    match intention.intent {
        Intent::Status => {
            status::serve(connection, shared).await?;
            Ok(None)
        }
        Intent::Login | Intent::Transfer => {
            let Some(profile) = login::serve(connection, shared, &intention).await? else {
                return Ok(None);
            };
            let Some(client_information) = configuration::serve(connection, shared).await? else {
                return Ok(None);
            };
            Ok(Some((profile, client_information)))
        }
    }
}
