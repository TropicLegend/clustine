//! An edge as a process of its own: the routing table it is sent, and the links to
//! the regions that it keeps by it.

use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clustine_coordinator::{ClientError, RoutingWatch};
use clustine_edge::{Edge, EdgeConfig, EdgeIdentity, RegionLink, Relinks, Routing, Stopped};
use clustine_region::{RegionId, RoutingTable};
use clustine_rpc::{EdgeMessage, RegionHello, WorkerToEdge, tcp};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use super::{RETRY, sleep_until_some};
use crate::LINK_CAPACITY;

/// Settings of an edge process.
#[derive(Debug, Clone)]
pub struct EdgeArgs {
    /// Name of this edge, by which regions know it again after a restart.
    pub name: String,
    /// Host and port of the coordinator.
    pub coordinator: String,
    /// The address players connect to.
    pub bind: SocketAddr,
    pub edge: EdgeConfig,
}

/// How long connecting to a worker may take before the edge tries again later. A
/// worker that is gone may not refuse the connection but swallow it.
const LINK_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs an edge until the process is asked to stop. Players are let in once every
/// region has had a worker, and stay when a region changes hands or cannot be reached
/// for a while: the edge links to whoever runs it then and resumes.
///
/// Fails if another process has taken this edge's name.
pub async fn edge(args: EdgeArgs) -> Result<()> {
    let stop = crate::stop_signal();
    tokio::pin!(stop);
    let (watch, table) = tokio::select! {
        _ = &mut stop => return Ok(()),
        table = whole_world(&args.coordinator) => table,
    };

    // One start for as long as the process lives: what the edge keeps for the regions
    // lives as long, and so do the numbers it gives its messages.
    let identity = EdgeIdentity::starting_now(&args.name);
    // Players enter the world in the region that has the spawn point, which the world
    // store pins as home. The edge is told that region and nothing else of how the world
    // is divided.
    let spawn_chunk = clustine_world::ChunkPos::containing(table.spawn.x, table.spawn.z);
    let home = table.layout.region_of(spawn_chunk);
    let (routing, relinks) = Routing::new(home, table.spawn, identity, Vec::new());
    let edge = Edge::bind(args.bind, args.edge.clone(), routing)
        .await
        .with_context(|| format!("listening on {}", args.bind))?;
    info!(address = %edge.local_addr()?, regions = table.routes.len(), "listening");

    // Dropping the edge's future, which happens when another branch wins, closes every
    // connection.
    tokio::select! {
        _ = &mut stop => Ok(()),
        stopped = edge.run() => match stopped {
            Stopped::Superseded => bail!(
                "another edge runs under the name {}; this one must not come back as it",
                args.name
            ),
            Stopped::Abandoned => bail!("the edge stopped unexpectedly"),
        },
        () = keep_linked(&args.coordinator, watch, table, relinks) => {
            bail!("the edge is gone")
        }
    }
}

/// Waits until the coordinator has a worker for every region, trying until it can be
/// reached. Returns the connection and the table.
async fn whole_world(coordinator: &str) -> (RoutingWatch, RoutingTable) {
    loop {
        let watching = async {
            let mut watch = RoutingWatch::connect(coordinator).await?;
            loop {
                let table = watch.next().await?;
                if table.is_complete() {
                    return Ok::<_, ClientError>((watch, table));
                }
                info!(
                    with_worker = table.routes.len(),
                    without = table.waiting,
                    "waiting for every region to have a worker"
                );
            }
        };
        match watching.await {
            Ok(whole) => return whole,
            Err(error) => {
                info!(%error, coordinator, "the coordinator cannot be reached yet");
                sleep(RETRY).await;
            }
        }
    }
}

/// How often the edge tries to link to a region's owner while that is new: the owner
/// of a region that was moved takes links a moment after the routing table names it, and
/// its players stand still until the edge is through.
const LINK_RETRY_AT_FIRST: Duration = Duration::from_millis(20);

/// For how long it tries that often, before it tries every [`RETRY`].
const LINK_RETRY_EAGERLY_FOR: Duration = Duration::from_secs(2);

/// What the link-keeper knows of one region.
#[derive(Default)]
struct LinkState {
    /// The epoch of the owner the edge has a link to.
    linked: Option<u64>,
    /// The epoch of the owner an attempt to link is under way to.
    trying: Option<u64>,
    /// Since when the edge has been without a link to the owner the table names.
    since: Option<Instant>,
    /// When to try again, if the last attempt failed.
    again: Option<Instant>,
}

/// Keeps the edge linked to whoever runs each region, for as long as the edge is
/// there: links to every region of `table`, and links again when the coordinator names
/// another owner or the edge says that a link has ended. Each region is tried by itself
/// and at once, so that a worker that does not answer keeps nobody but its own region
/// waiting, and a new route is taken up also while the old one is still being tried. A
/// worker that cannot be reached is tried again, often at first; a coordinator that
/// goes away is waited for, with the regions where they were.
async fn keep_linked(
    coordinator: &str,
    watch: RoutingWatch,
    mut table: RoutingTable,
    mut relinks: Relinks,
) {
    let layout = table.layout.fingerprint();
    // The regions that were absorbed before this edge started, before any link: what
    // a region says can name one of them from the first message on.
    if !relinks.absorbed(table.absorbed.clone()).await {
        return;
    }
    let mut watch = Some(watch);
    // The search for a coordinator, while there is none. It is one future that lives
    // across the passes of the loop below: a wait begun anew at every pass would never
    // end while link attempts make the loop pass more often than it lasts.
    let mut search: Option<Pin<Box<dyn Future<Output = RoutingWatch> + Send>>> = None;
    let mut regions: BTreeMap<RegionId, LinkState> = BTreeMap::new();
    // The attempts under way: each ends with the region, the epoch it was for, and the
    // link if there is one.
    let mut attempts = JoinSet::new();
    loop {
        // An attempt for every region that is not linked to the owner the table names,
        // has none under way to that owner, and is not waiting to try again.
        let now = Instant::now();
        for route in &table.routes {
            let state = regions.entry(route.region).or_default();
            if state.linked == Some(route.epoch) {
                state.since = None;
                continue;
            }
            state.since.get_or_insert(now);
            let due = state.again.is_none_or(|again| again <= now);
            if state.trying == Some(route.epoch) || !due {
                continue;
            }
            state.trying = Some(route.epoch);
            state.again = None;
            let (region, epoch, address) = (route.region, route.epoch, route.address.clone());
            let hello = RegionHello {
                region,
                epoch,
                layout,
            };
            attempts.spawn(async move {
                let connecting =
                    tcp::connect::<EdgeMessage, WorkerToEdge>(&address, hello, LINK_CAPACITY);
                let link = match timeout(LINK_TIMEOUT, connecting).await {
                    Ok(Ok(end)) => Some(end),
                    // A worker that restores its region takes no links until it is done.
                    Ok(Err(error)) => {
                        debug!(%region, %address, %error, "a region cannot be linked to yet");
                        None
                    }
                    Err(_) => {
                        debug!(%region, %address, "a worker did not answer in time");
                        None
                    }
                };
                (region, epoch, address, link)
            });
        }
        let again = regions.values().filter_map(|state| state.again).min();

        tokio::select! {
            Some(ended) = attempts.join_next() => {
                // An attempt neither panics nor is cancelled.
                let Ok((region, epoch, address, link)) = ended else {
                    continue;
                };
                let state = regions.entry(region).or_default();
                if state.trying == Some(epoch) {
                    state.trying = None;
                }
                let current = table.route(region).is_some_and(|route| route.epoch == epoch);
                match link {
                    // The table has moved on meanwhile; the link is closed by dropping it.
                    Some(_) if !current => {}
                    Some(end) => {
                        if !relinks.replace(RegionLink { region, epoch, end }).await {
                            return;
                        }
                        info!(%region, epoch, %address, "linked to a region");
                        state.linked = Some(epoch);
                        state.since = None;
                    }
                    None if current => {
                        let eager = state
                            .since
                            .is_some_and(|since| since.elapsed() < LINK_RETRY_EAGERLY_FOR);
                        let wait = if eager { LINK_RETRY_AT_FIRST } else { RETRY };
                        state.again = Some(Instant::now() + wait);
                    }
                    None => {}
                }
            }
            ended = relinks.ended() => match ended {
                // A link to an owner the edge has left behind already ended.
                Some((region, epoch)) => {
                    let state = regions.entry(region).or_default();
                    if state.linked == Some(epoch) {
                        state.linked = None;
                        state.again = None;
                    }
                }
                None => return,
            },
            next = next_table(&mut watch), if watch.is_some() => {
                // Which regions were absorbed is passed on from every table, also from
                // one this edge otherwise keeps away from: the edge has to know that a
                // region it still has something of is no more.
                if let Some(next) = &next
                    && !relinks.absorbed(next.absorbed.clone()).await
                {
                    return;
                }
                match next {
                Some(next) if next.layout == table.layout => {
                    // A region with a new owner is tried at once, whatever the last
                    // attempt at the old one came to.
                    for route in &next.routes {
                        let changed = table
                            .route(route.region)
                            .is_none_or(|old| old.epoch != route.epoch);
                        if changed && let Some(state) = regions.get_mut(&route.region) {
                            state.again = None;
                            state.since = None;
                        }
                    }
                    table = next;
                }
                Some(_) => warn!("the coordinator divides the world differently now; keeping to the layout this edge started with"),
                None => {
                    warn!("lost the coordinator; carrying on with the regions as they are");
                    watch = None;
                    search = Some(Box::pin(find_coordinator(coordinator.to_owned())));
                }
            }
            }
            () = sleep_until_some(again), if again.is_some() => {}
            found = found_coordinator(&mut search), if search.is_some() => {
                info!("found a coordinator again");
                watch = Some(found);
                search = None;
            }
        }
    }
}

/// Connects to the coordinator at `coordinator`, trying at once and then every
/// [`RETRY`] until it is there.
async fn find_coordinator(coordinator: String) -> RoutingWatch {
    loop {
        if let Ok(watch) = RoutingWatch::connect(&coordinator).await {
            return watch;
        }
        sleep(RETRY).await;
    }
}

/// What `search` comes to, which the caller has made sure is there.
async fn found_coordinator(
    search: &mut Option<Pin<Box<dyn Future<Output = RoutingWatch> + Send>>>,
) -> RoutingWatch {
    match search {
        Some(search) => search.await,
        None => std::future::pending().await,
    }
}

/// The next routing table, or `None` if the connection to the coordinator is lost.
async fn next_table(watch: &mut Option<RoutingWatch>) -> Option<RoutingTable> {
    watch.as_mut()?.next().await.ok()
}
