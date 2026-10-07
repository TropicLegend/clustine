//! Helpers shared by the end-to-end tests.

use std::time::Duration;

use clustine::{Config, EdgeConfig, Server};

pub fn config() -> Config {
    Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        description: "integration test".to_owned(),
        max_players: 7,
        keep_alive_interval: EdgeConfig::DEFAULT_KEEP_ALIVE_INTERVAL,
    }
}

/// Starts a server on a free port and returns it with its address as `host:port`.
pub async fn start() -> (Server, String) {
    start_with(config()).await
}

pub async fn start_with(config: Config) -> (Server, String) {
    let server = Server::start(config).await.unwrap();
    let address = server.address().to_string();
    (server, address)
}

/// A keep-alive interval short enough to observe several rounds in a test.
#[allow(dead_code)] // Not every test binary uses it.
pub const SHORT_KEEP_ALIVE: Duration = Duration::from_millis(100);
