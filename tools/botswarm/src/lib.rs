//! Load-test harness: drives many scripted bot clients against a server.
//!
//! The bots speak the protocol through `clustine-protocol`, the same codec the server
//! uses. To catch mistakes that both sides would share, the scenarios are also run
//! against the official server.

pub mod bot;
pub mod connection;
pub mod oracle;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clustine_data::PROTOCOL_VERSION;
use clustine_protocol::packets::handshake::{Intent, Intention};
use clustine_protocol::packets::status::{ClientboundStatus, PingRequest, StatusRequest};

pub use bot::{Bot, JoinInfo, PlayStats};
pub use connection::Connection;
pub use oracle::Oracle;

/// A server's answer to the server list ping.
#[derive(Debug)]
pub struct Status {
    /// The server list entry; see `docs/protocol-26.3.md` for its shape.
    pub json: serde_json::Value,
    /// Time between sending the ping and receiving the pong.
    pub latency: Duration,
}

/// Performs the server list ping against `address` (`host:port`).
pub async fn ping(address: &str) -> Result<Status> {
    let mut connection = Connection::connect(address).await?;
    connection
        .write(&intention(address, Intent::Status)?)
        .await?;

    connection.write(&StatusRequest).await?;
    let ClientboundStatus::StatusResponse(response) =
        ClientboundStatus::decode(&connection.read_frame().await?)?
    else {
        bail!("expected a status response");
    };
    let json = serde_json::from_str(&response.json).context("status response is not JSON")?;

    let payload = 0x436C_7573_7469_6E65;
    let sent = Instant::now();
    connection.write(&PingRequest { payload }).await?;
    let ClientboundStatus::PongResponse(pong) =
        ClientboundStatus::decode(&connection.read_frame().await?)?
    else {
        bail!("expected a pong");
    };
    let latency = sent.elapsed();
    if pong.payload != payload {
        bail!(
            "pong carries {:#x} instead of the ping's payload",
            pong.payload
        );
    }
    Ok(Status { json, latency })
}

/// The handshake a vanilla client sends when connecting to `address` (`host:port`).
pub fn intention(address: &str, intent: Intent) -> Result<Intention> {
    let (host, port) = address
        .rsplit_once(':')
        .with_context(|| format!("{address} is not of the form host:port"))?;
    Ok(Intention {
        protocol_version: PROTOCOL_VERSION,
        server_address: host.trim_matches(['[', ']']).to_owned(),
        server_port: port
            .parse()
            .with_context(|| format!("{port} is not a port number"))?,
        intent,
    })
}
