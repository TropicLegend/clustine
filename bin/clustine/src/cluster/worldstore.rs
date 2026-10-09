//! The world store as a process of its own.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clustine_region::Layout;
use clustine_worldstore::Store;
use tracing::info;

use crate::{division, generator};

/// Runs a world store for the world in the directory `world`, divided into regions at
/// `boundaries` as the coordinator divides it, until the process is asked to stop.
pub async fn worldstore(listen: SocketAddr, world: PathBuf, boundaries: Vec<i32>) -> Result<()> {
    let layout = Layout::new(boundaries).context("dividing the world into regions")?;
    let store = Store::local_divided(&world, generator(), division(&layout))
        .with_context(|| format!("opening the world in {}", world.display()))?;
    let listener =
        std::net::TcpListener::bind(listen).with_context(|| format!("listening on {listen}"))?;
    let server = clustine_worldstore::serve(store, listener).context("serving the world")?;
    info!(
        address = %server.local_addr(),
        world = %world.display(),
        regions = layout.region_count(),
        "serving the world"
    );
    crate::stop_signal().await;
    info!("shutting down");
    // Closes the connections; what their owners asked for before is done first.
    tokio::task::spawn_blocking(move || server.stop()).await?;
    Ok(())
}
