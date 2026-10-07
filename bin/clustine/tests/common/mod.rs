//! Helpers shared by the end-to-end tests.

use std::time::Duration;

use clustine::{Config, EdgeConfig, Server};

pub fn config() -> Config {
    Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        description: "integration test".to_owned(),
        max_players: 7,
        keep_alive_interval: EdgeConfig::DEFAULT_KEEP_ALIVE_INTERVAL,
        // Small, so that tests do not wait for hundreds of chunks.
        view_distance: VIEW_DISTANCE,
        serialise_link: false,
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

/// The view distance of test servers, in chunks.
#[allow(dead_code)] // Not every test binary uses it.
pub const VIEW_DISTANCE: i32 = 3;

/// The chunks a server sends a client that is in the chunk `center` and has the view
/// distance `view_distance`, as the official server determines them.
#[allow(dead_code)] // Not every test binary uses it.
pub fn view_area(center: (i32, i32), view_distance: i32) -> Vec<(i32, i32)> {
    let reach = view_distance + 2;
    let mut area = Vec::new();
    for x in -reach..=reach {
        for z in -reach..=reach {
            let (dx, dz) = ((x.abs() - 2).max(0), (z.abs() - 2).max(0));
            if dx * dx + dz * dz < view_distance * view_distance {
                area.push((center.0 + x, center.1 + z));
            }
        }
    }
    area
}
