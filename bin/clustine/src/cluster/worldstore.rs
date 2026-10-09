//! The world store as a process of its own.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clustine_region::Layout;
use clustine_world::ChunkPos;
use clustine_worldstore::{Division, Store};
use tracing::info;

use crate::{division, generator, spawn_point};

/// Runs a world store for the world in the directory `world` until the process is
/// asked to stop. The world is divided into the stripes of `boundaries`, as the
/// coordinator is told them, if there are any; else it has regions pinned side by side
/// at `pins`, if there are any; else it is one home region that is pinned to nothing
/// (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.1).
pub async fn worldstore(
    listen: SocketAddr,
    world: PathBuf,
    boundaries: Vec<i32>,
    pins: Vec<i32>,
) -> Result<()> {
    let spawn = spawn_point();
    let home = ChunkPos::containing(spawn.x, spawn.z);
    let division = if !boundaries.is_empty() {
        let layout = Layout::new(boundaries).context("dividing the world into regions")?;
        division(&layout)
    } else if !pins.is_empty() {
        Division::side_by_side(home, &pins).map_err(|error| anyhow!("--pin takes {error}"))?
    } else {
        Division::open(home)
    };
    let pinned = division.pinned.len();
    let store = Store::local_divided(&world, generator(), division)
        .with_context(|| format!("opening the world in {}", world.display()))?;
    let listener =
        std::net::TcpListener::bind(listen).with_context(|| format!("listening on {listen}"))?;
    let server = clustine_worldstore::serve(store, listener).context("serving the world")?;
    info!(
        address = %server.local_addr(),
        world = %world.display(),
        pinned,
        "serving the world"
    );
    crate::stop_signal().await;
    info!("shutting down");
    // Closes the connections; what their owners asked for before is done first.
    tokio::task::spawn_blocking(move || server.stop()).await?;
    Ok(())
}
