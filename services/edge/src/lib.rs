//! Edge service: terminates client connections, routes inbound packets, fans region deltas out to players.
//!
//! The edge owns everything that is specific to the Minecraft protocol; see
//! `docs/adr/0005-edge-worker-interface.md`.

mod configuration;
mod connection;
#[allow(dead_code)] // Used once chunks come from the worker.
mod encode;
mod login;
mod play;
mod session;
mod status;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tracing::{debug, warn};

/// Settings of an edge that players can see.
#[derive(Debug, Clone)]
pub struct EdgeConfig {
    /// The text shown below the server's name in the client's server list.
    pub description: String,
    /// The player limit shown in the server list.
    pub max_players: u32,
    /// How often a client has to prove it is still there. A client that has not answered
    /// one keep-alive by the time the next is due is disconnected.
    pub keep_alive_interval: Duration,
}

impl EdgeConfig {
    /// The keep-alive interval of the vanilla server. Clients give up on a server they
    /// have not heard from for 30 seconds, so the interval must stay well below that.
    pub const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);
}

/// State shared by all connections of one edge.
struct Shared {
    config: EdgeConfig,
    /// See [`configuration::opening_packets`].
    opening_packets: Vec<Vec<u8>>,
    /// See [`configuration::registry_packets`].
    registry_packets: Vec<Vec<u8>>,
    /// Entity ids start at 1 because clients reject 0.
    next_entity_id: AtomicI32,
}

/// A listening edge that has not started accepting connections yet.
pub struct Edge {
    listener: TcpListener,
    shared: Arc<Shared>,
}

impl Edge {
    /// Starts listening on `address`. Port 0 picks a free port; see [`Edge::local_addr`].
    pub async fn bind(address: SocketAddr, config: EdgeConfig) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(address).await?,
            shared: Arc::new(Shared {
                config,
                opening_packets: configuration::opening_packets(),
                registry_packets: configuration::registry_packets(),
                next_entity_id: AtomicI32::new(1),
            }),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts and serves connections until the returned future is dropped, which also
    /// closes every open connection.
    pub async fn run(self) {
        let mut connections = JoinSet::new();
        loop {
            // Reap finished connections so the set does not grow without bound.
            while connections.try_join_next().is_some() {}

            let (stream, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Typically the process is out of file descriptors; keep serving the
                    // connections that exist and retry.
                    warn!(%error, "accepting a connection failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let shared = Arc::clone(&self.shared);
            connections.spawn(async move {
                if let Err(error) = session::serve(stream, &shared).await {
                    debug!(%peer, %error, "connection ended with an error");
                }
            });
        }
    }
}
