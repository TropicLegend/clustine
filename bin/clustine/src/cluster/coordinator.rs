//! The coordinator as a process of its own.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use clustine_coordinator::{CoordinatorConfig, Policy};
use clustine_region::Layout;
use clustine_worldstore::StoreError;
use tokio::net::TcpListener;
use tracing::info;

use crate::spawn_point;

/// Settings of a coordinator process.
#[derive(Debug, Clone)]
pub struct CoordinatorArgs {
    pub listen: SocketAddr,
    /// The chunk x coordinates at which the world is divided into regions, ascending.
    pub boundaries: Vec<i32>,
    /// How long a worker may be silent before its region is given to another.
    pub lease: Duration,
    /// Host and port of the world store, whose list says which regions there are.
    pub store: String,
    /// What the coordinator goes by to merge and split regions by itself, or `None`
    /// for one that leaves that to whoever asks.
    pub follow: Option<Policy>,
}

/// Runs a coordinator until the process is asked to stop.
pub async fn coordinator(args: CoordinatorArgs) -> Result<()> {
    let layout = Layout::new(args.boundaries).context("dividing the world into regions")?;
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("listening on {}", args.listen))?;
    info!(
        address = %listener.local_addr()?,
        regions = layout.region_count(),
        store = %args.store,
        "coordinating"
    );
    // How it reshapes and with which numbers, for whoever reads the log to know which
    // of the two this coordinator is (`docs/adr/0016-when-to-merge-and-split.md`,
    // section 8).
    match &args.follow {
        None => info!("reshaping by hand: regions merge and split when somebody asks"),
        Some(policy) => info!(
            merge_distance = policy.merge_distance,
            split_distance = policy.split_distance,
            margin = policy.margin(),
            rest_seconds = policy.rest.as_secs(),
            "reshaping by itself: regions merge and split by where their players are"
        ),
    }
    let config = CoordinatorConfig {
        layout,
        spawn: spawn_point(),
        lease: args.lease,
        follow: args.follow,
    };
    // Which regions there are, and which of them were absorbed, the world store says
    // (`docs/adr/0014-merging-and-splitting.md`, section 5.2). While it cannot be
    // reached the coordinator goes by the layout and by what its workers report, and
    // refuses to merge and to split.
    let store = args.store;
    let lists = move || {
        clustine_worldstore::regions(&store).map_err(|error| match error {
            StoreError::Io(error) => error,
            refusal => io::Error::other(refusal),
        })
    };
    tokio::select! {
        served = clustine_coordinator::serve(listener, config, lists) => served.context("coordinating"),
        _ = crate::stop_signal() => Ok(()),
    }
}
