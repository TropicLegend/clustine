//! Helpers shared by the end-to-end tests.

// Not every test binary starts processes.
#[allow(dead_code)]
pub mod processes;

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
        client_timeout: EdgeConfig::DEFAULT_CLIENT_TIMEOUT,
        region_patience: EdgeConfig::DEFAULT_REGION_PATIENCE,
        compression_threshold: Some(EdgeConfig::DEFAULT_COMPRESSION_THRESHOLD),
        // Nothing is written to disk unless a test asks for it.
        world: None,
        checkpoint_interval: Duration::from_secs(300),
        serialise_link: false,
        pins: boundaries(),
        // Regions stay as they are unless a test says otherwise.
        follow: None,
    }
}

/// Where the tests' worlds are divided into regions: nowhere, unless the environment
/// variable `CLUSTINE_TEST_BOUNDARIES` lists chunk x coordinates, separated by commas.
/// Setting it runs every test against a world of several regions.
fn boundaries() -> Vec<i32> {
    std::env::var("CLUSTINE_TEST_BOUNDARIES")
        .map(|list| {
            list.split(',')
                .map(|x| x.trim().parse().expect("a chunk x coordinate"))
                .collect()
        })
        .unwrap_or_default()
}

/// Starts a server on a free port and returns it with its address as `host:port`.
#[allow(dead_code)] // Not every test binary uses it.
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

/// An address on this machine that nothing listens on right now, as `host:port`.
#[allow(dead_code)] // Not every test binary uses it.
///
/// The port is below those the system gives to whoever asks for any port or connects
/// out. A process of a test cluster that is killed and started again binds its address
/// a second time, and a port the system had picked was once given to someone else in
/// between: the worker could not come back and its players were disconnected.
pub async fn free_address() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    // Where the system's own range begins on Linux unless it was changed.
    const FIRST: u32 = 10_240;
    const COUNT: u32 = 32_768 - FIRST;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    // Test binaries that run at the same time begin in different places.
    let start = std::process::id().wrapping_mul(7_919);
    for _ in 0..COUNT {
        let offset = start.wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed)) % COUNT;
        let address = format!("127.0.0.1:{}", FIRST + offset);
        if tokio::net::TcpListener::bind(&address).await.is_ok() {
            return address;
        }
    }
    panic!("no port is free");
}

/// Starts the real server binary on `address` with its world in `world` and with the
/// further arguments `more`, and waits until it accepts connections. The process is
/// killed when the returned handle is dropped.
#[allow(dead_code)] // Not every test binary uses it.
pub async fn spawn_server(
    address: &str,
    world: &std::path::Path,
    more: &[&str],
) -> tokio::process::Child {
    let server = tokio::process::Command::new(env!("CARGO_BIN_EXE_clustine"))
        .args(["--bind", address, "--view-distance"])
        .arg(VIEW_DISTANCE.to_string())
        .arg("--world")
        .arg(world)
        .args(more)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if clustine_botswarm::ping(address).await.is_ok() {
            return server;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server did not start");
}
