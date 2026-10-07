//! The start of every connection: the handshake, then the state the client asked for.

use clustine_protocol::packets::handshake::{Intent, ServerboundHandshake};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::connection::{Connection, ConnectionError, READ_TIMEOUT};
use crate::{Shared, login, status};

/// First byte of the server list ping used before Minecraft 1.7.
const LEGACY_PING: u8 = 0xFE;

pub(crate) async fn serve(stream: TcpStream, shared: &Shared) -> Result<(), ConnectionError> {
    // Current clients fall back to the legacy ping when the modern one fails. It is not a
    // framed packet, so it has to be recognised before framing starts. Closing at once
    // makes the client give up instead of waiting for a timeout.
    let mut first = [0];
    let peeked = timeout(READ_TIMEOUT, stream.peek(&mut first))
        .await
        .map_err(|_| ConnectionError::TimedOut)??;
    if peeked == 0 || first[0] == LEGACY_PING {
        return Ok(());
    }

    let mut connection = Connection::new(stream);
    let Some(frame) = connection.read_frame().await? else {
        return Ok(());
    };
    let ServerboundHandshake::Intention(intention) = ServerboundHandshake::decode(&frame)? else {
        return Err(ConnectionError::Protocol("expected a handshake"));
    };
    match intention.intent {
        Intent::Status => status::serve(&mut connection, shared).await,
        Intent::Login | Intent::Transfer => login::serve(&mut connection, shared, &intention).await,
    }
}
