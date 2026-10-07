//! Edge service: terminates client connections, routes inbound packets, fans region deltas out to players.
//!
//! The edge owns everything that is specific to the Minecraft protocol; see
//! `docs/adr/0005-edge-worker-interface.md`.

mod connection;
mod login;
mod session;
mod status;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
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
}

/// State shared by all connections of one edge.
struct Shared {
    config: EdgeConfig,
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
            shared: Arc::new(Shared { config }),
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
