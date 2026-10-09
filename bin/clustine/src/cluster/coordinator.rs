//! The coordinator as a process of its own.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use clustine_coordinator::{CoordinatorConfig, Policy};
use clustine_region::Layout;
use clustine_worldstore::StoreError;
use tokio::net::TcpListener;
use tracing::{info, warn};

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
    /// The largest view distance the edges grant, in chunks, as the coordinator was
    /// told it.
    pub view_distance: u32,
}

/// Says in the log how regions are reshaped and with which numbers, for whoever reads
/// it to know which of the two this server is
/// (`docs/adr/0016-when-to-merge-and-split.md`, section 8). The coordinator's process
/// says it, and the single process, which has its coordinator within.
///
/// And it says when the merge distance is too short for `view_distance`, the largest
/// the edges grant: a region's land reaches as far as its players see, one chunk
/// further than the view distance, so two regions whose players are nearer than
/// twice that and a chunk have lands that touch before they are merged. Players are
/// then handed over at the line where the lands met, and split off again if the
/// split distance is short as well: nothing breaks and play is worse
/// (`docs/adr/0017-the-end-of-the-stripes.md`, section 3.5). A warning and no
/// refusal: tests set such distances on purpose.
pub(crate) fn say_how_it_reshapes(follow: Option<&Policy>, view_distance: u32) {
    let Some(policy) = follow else {
        info!("reshaping by hand: regions merge and split when somebody asks");
        return;
    };
    info!(
        merge_distance = policy.merge_distance,
        split_distance = policy.split_distance,
        margin = policy.margin(),
        rest_seconds = policy.rest.as_secs(),
        "reshaping by itself: regions merge and split by where their players are"
    );
    let needs = 2 * view_distance + 3;
    if policy.merge_distance < needs {
        warn!(
            merge_distance = policy.merge_distance,
            view_distance,
            needs,
            "the merge distance is less than players see across: regions will hand players over where they would merge"
        );
    }
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
    say_how_it_reshapes(args.follow.as_ref(), args.view_distance);
    let config = CoordinatorConfig {
        layout,
        spawn: spawn_point(),
        lease: args.lease,
        follow: args.follow,
    };
    // Which regions there are, and which of them were absorbed, the world store says
    // (`docs/adr/0014-merging-and-splitting.md`, section 5.2). While it cannot be
    // reached the coordinator goes by what its workers report, and by its stripes if
    // it was told any, and refuses to merge and to split. One that was told no
    // boundary knows no region until the store has answered, and reads until it has
    // (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.3).
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
