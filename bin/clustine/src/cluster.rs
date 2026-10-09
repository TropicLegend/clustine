//! The services as processes of their own, which together are a cluster.
//!
//! A coordinator divides the world into regions and decides which worker runs which. A
//! worker registers with it, is given a region, opens that region at the world store
//! and runs it. An edge learns from the coordinator where each region's worker is,
//! connects to all of them and then lets players in.
//!
//! A worker that loses the world store keeps its region and restores it from the store
//! once that is back. An edge whose link to a region ends keeps the region's players,
//! links to whoever runs the region then and resumes with it. Nothing players were
//! shown is lost on the way, because a region shows nothing that the world store does
//! not have. A coordinator that goes away is merely missed until it is back.
//!
//! Regions are moved from worker to worker, merged and split when somebody asks the
//! coordinator for that: `clustine move`, `clustine merge` and `clustine split`. The
//! coordinator learns which regions there are from the world store's list of them.
//! Every worker tells it where the players of its regions are, and a coordinator that
//! is started with `--reshape by-itself` merges and splits regions by that as well.

use std::time::{Duration, Instant};

mod commands;
pub(crate) mod coordinator;
pub(crate) mod edge;
pub(crate) mod worker;
mod worldstore;

pub use commands::{MergeArgs, MoveArgs, SplitArgs, merge_regions, move_region, split_region};
pub use coordinator::{CoordinatorArgs, coordinator};
pub use edge::{EdgeArgs, edge};
pub use worker::{WorkerArgs, worker};
pub use worldstore::worldstore;

/// The ports the services listen on unless told otherwise.
pub const COORDINATOR_PORT: u16 = 25600;
pub const WORKER_PORT: u16 = 25601;
pub const WORLDSTORE_PORT: u16 = 25602;

/// How long to wait before trying again to reach a service that is not there.
const RETRY: Duration = Duration::from_secs(1);

/// Sleeps until `when`, which the caller has made sure is there.
async fn sleep_until_some(when: Option<Instant>) {
    if let Some(when) = when {
        tokio::time::sleep_until(when.into()).await;
    }
}
