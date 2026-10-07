//! Single-binary mode: runs every Clustine service in one process.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clustine_edge::{Edge, EdgeConfig};
use tokio::task::JoinHandle;

/// Settings of a single-process server.
#[derive(Debug, Clone)]
pub struct Config {
    /// The address players connect to. Port 0 picks a free port.
    pub bind: SocketAddr,
    /// The text shown below the server's name in the client's server list.
    pub description: String,
    /// The player limit shown in the server list.
    pub max_players: u32,
}

/// A running server. Dropping it without calling [`Server::stop`] leaves it running
/// until the runtime shuts down.
pub struct Server {
    address: SocketAddr,
    edge: JoinHandle<()>,
}

impl Server {
    /// Starts all services and returns once the server accepts connections.
    pub async fn start(config: Config) -> Result<Self> {
        let edge_config = EdgeConfig {
            description: config.description,
            max_players: config.max_players,
        };
        let edge = Edge::bind(config.bind, edge_config)
            .await
            .with_context(|| format!("listening on {}", config.bind))?;
        let address = edge.local_addr()?;
        Ok(Self {
            address,
            edge: tokio::spawn(edge.run()),
        })
    }

    /// The address the server listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Stops accepting connections and closes the existing ones.
    pub async fn stop(self) {
        self.edge.abort();
        // The task was cancelled on purpose, so its result carries no information.
        let _ = self.edge.await;
    }
}
