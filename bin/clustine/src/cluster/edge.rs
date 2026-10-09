//! An edge as a process of its own: the routing table it is sent, and the links to
//! the regions that it keeps by it.

use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clustine_coordinator::{ClientError, Reach, RoutingWatch};
use clustine_edge::{Edge, EdgeConfig, EdgeIdentity, RegionLink, Relinks, Routing, Stopped};
use clustine_region::{RegionId, RoutingTable};
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{EdgeMessage, RegionHello, WorkerToEdge, tcp};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use super::worker::Serving;
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
    let coordinator = Reach::Tcp(args.coordinator);
    let (watch, table, home) = tokio::select! {
        _ = &mut stop => return Ok(()),
        table = whole_world(&coordinator) => table,
    };

    // One start for as long as the process lives: what the edge keeps for the regions
    // lives as long, and so do the numbers it gives its messages.
    let identity = EdgeIdentity::starting_now(&args.name);
    // Players enter the world in its home region, which the world store's list names
    // and the routing table passes on. The edge is told that region and nothing else of
    // how the world is divided.
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
        () = keep_linked(&coordinator, Linking::Tcp, home, watch, table, relinks) => {
            bail!("the edge is gone")
        }
    }
}

/// Waits until the coordinator names the world's home region, has a worker for it and
/// has no region waiting for one, trying until the coordinator can be reached. Returns
/// the connection, the table and the home region.
///
/// A table without a home region is that of a coordinator that has not read the world
/// store's list yet: it knows no region, or only those its workers report, and nothing
/// says where players enter.
pub(crate) async fn whole_world(coordinator: &Reach) -> (RoutingWatch, RoutingTable, RegionId) {
    loop {
        let watching = async {
            let mut watch = RoutingWatch::connect(coordinator).await?;
            loop {
                let table = watch.next().await?;
                if let Some(home) = whole(&table) {
                    return Ok::<_, ClientError>((watch, table, home));
                }
                info!(
                    home = ?table.home,
                    with_worker = table.routes.len(),
                    without = table.waiting,
                    "waiting for the home region to be known and every region to have a worker"
                );
            }
        };
        match watching.await {
            Ok(whole) => return whole,
            Err(error) => {
                let coordinator = place(coordinator);
                info!(%error, coordinator, "the coordinator cannot be reached yet");
                sleep(RETRY).await;
            }
        }
    }
}

/// Where the coordinator is, for the log.
fn place(coordinator: &Reach) -> &str {
    match coordinator {
        Reach::Tcp(address) => address,
        Reach::Local(_) => "this process",
    }
}

/// The home region of `table`, if the table is one an edge can let players in by: it
/// names the home region, has a route for it, and no region waits for a worker.
fn whole(table: &RoutingTable) -> Option<RegionId> {
    let home = table.home?;
    (table.route(home).is_some() && table.is_complete()).then_some(home)
}

/// How an edge links to the regions of its routing table.
#[derive(Clone)]
pub(crate) enum Linking {
    /// Over TCP, to the address the table has for each region.
    Tcp,
    /// In this process, to the regions a worker of this process shows that it serves:
    /// through a pair of queues, or through bytes in memory as a link between two
    /// processes works, if `serialise`.
    Local {
        serving: watch::Receiver<Serving>,
        serialise: bool,
    },
}

impl Linking {
    /// A link to the region of `hello` at `address`, or why there is none yet.
    async fn link(&self, address: &str, hello: RegionHello) -> Result<EdgeEnd, String> {
        match self {
            Self::Tcp => {
                let connecting =
                    tcp::connect::<EdgeMessage, WorkerToEdge>(address, hello, LINK_CAPACITY);
                match timeout(LINK_TIMEOUT, connecting).await {
                    Ok(Ok(end)) => Ok(end),
                    // A worker that restores its region takes no links until it is done.
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err("the worker did not answer in time".to_owned()),
                }
            }
            Self::Local { serving, serialise } => {
                // As a worker greets an edge that connects: by what it serves at this
                // moment, and only the region as it is asked for.
                let serving = serving.borrow().clone();
                let Some((served, links)) = serving.get(&hello.region) else {
                    return Err("the worker does not run the region at the moment".to_owned());
                };
                if *served != hello {
                    return Err(format!(
                        "the worker runs the region with epoch {}",
                        served.epoch
                    ));
                }
                let (end, worker_end): (EdgeEnd, _) = if *serialise {
                    link::framed(LINK_CAPACITY)
                } else {
                    link::in_process(LINK_CAPACITY)
                };
                links.attach(worker_end);
                Ok(end)
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

/// When the keeper has to look again without anything having happened: the earliest
/// time at which a link is to be tried again, among the regions `table` routes.
///
/// A region the table no longer routes is not tried, so the time noted for it is
/// nothing to wake up for. A region is released before it is absorbed, and the edge
/// tries its route again in between and is turned away: a keeper that waited for that
/// time as well would find it past at every pass from then on, and go round without
/// rest for as long as the edge lives.
fn next_retry(regions: &BTreeMap<RegionId, LinkState>, table: &RoutingTable) -> Option<Instant> {
    let routed = table.routes.iter().map(|route| route.region);
    routed
        .filter_map(|region| regions.get(&region)?.again)
        .min()
}

/// Keeps the edge linked to whoever runs each region, for as long as the edge is
/// there: links to every region of `table`, and links again when the coordinator names
/// another owner or the edge says that a link has ended. Each region is tried by itself
/// and at once, so that a worker that does not answer keeps nobody but its own region
/// waiting, and a new route is taken up also while the old one is still being tried. A
/// worker that cannot be reached is tried again, often at first; a coordinator that
/// goes away is waited for, with the regions where they were.
pub(crate) async fn keep_linked(
    coordinator: &Reach,
    linking: Linking,
    home: RegionId,
    watch: RoutingWatch,
    mut table: RoutingTable,
    mut relinks: Relinks,
) {
    // Whether the edge has said that the world has another home region than its own.
    let mut warned = false;
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
            let hello = RegionHello { region, epoch };
            let linking = linking.clone();
            attempts.spawn(async move {
                let link = match linking.link(&address, hello).await {
                    Ok(end) => Some(end),
                    Err(error) => {
                        debug!(%region, %address, %error, "a region cannot be linked to yet");
                        None
                    }
                };
                (region, epoch, address, link)
            });
        }
        let again = next_retry(&regions, &table);

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
                Some(next) => {
                    // The home region of a world never changes. A table without one is
                    // a coordinator that has started anew and not read the list yet,
                    // whose routes are as good as any; one with another is a world
                    // that was made over under this edge.
                    if next.home.is_some_and(|other| other != home) && !warned {
                        warn!(
                            %home,
                            now = ?next.home,
                            "the world has another home region now; this edge has to be started anew"
                        );
                        warned = true;
                    }
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
                    // A region that has no route any more is not tried: what was
                    // noted of the attempts at it is forgotten, so that it is tried
                    // at once, and often, should it have an owner again.
                    for (region, state) in &mut regions {
                        if next.route(*region).is_none() {
                            state.again = None;
                            state.since = None;
                        }
                    }
                    table = next;
                }
                None => {
                    warn!("lost the coordinator; carrying on with the regions as they are");
                    watch = None;
                    search = Some(Box::pin(find_coordinator(coordinator.clone())));
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
async fn find_coordinator(coordinator: Reach) -> RoutingWatch {
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

#[cfg(test)]
mod tests {
    use clustine_region::RegionRoute;
    use clustine_world::Vec3;

    use super::*;

    /// A table with routes for `routed` and with so many regions waiting for a worker.
    fn table(home: Option<u32>, routed: &[u32], waiting: u32) -> RoutingTable {
        let route = |region: &u32| RegionRoute {
            region: RegionId(*region),
            epoch: 7,
            address: "a:25601".to_owned(),
        };
        RoutingTable {
            version: 1,
            spawn: Vec3::new(0.5, 64.0, 0.5),
            routes: routed.iter().map(route).collect(),
            home: home.map(RegionId),
            absorbed: Vec::new(),
            waiting,
        }
    }

    /// A keeper's regions, each with when its link is to be tried again.
    fn noted(again: &[(u32, Option<Instant>)]) -> BTreeMap<RegionId, LinkState> {
        let state = |(region, again): &(u32, Option<Instant>)| {
            let state = LinkState {
                again: *again,
                ..LinkState::default()
            };
            (RegionId(*region), state)
        };
        again.iter().map(state).collect()
    }

    #[test]
    fn the_keeper_wakes_for_the_earliest_retry_of_a_region_that_has_a_route() {
        let now = Instant::now();
        let (soon, later) = (now + RETRY, now + 2 * RETRY);
        let regions = noted(&[(0, Some(later)), (1, Some(soon)), (2, None)]);
        assert_eq!(
            next_retry(&regions, &table(Some(0), &[0, 1, 2], 0)),
            Some(soon)
        );
        assert_eq!(
            next_retry(&regions, &table(Some(0), &[0, 2], 0)),
            Some(later)
        );
        assert_eq!(next_retry(&regions, &table(Some(0), &[2], 0)), None);
        // A region the keeper has not met yet is tried at once, not waited for.
        assert_eq!(next_retry(&regions, &table(Some(0), &[2, 3], 0)), None);
    }

    /// A region is released before it is absorbed, and the edge is turned away by its
    /// worker in between. The time it noted then is past for ever after: waiting for
    /// it would be no wait at all.
    #[test]
    fn the_keeper_does_not_wake_for_a_region_that_has_left_the_routing_table() {
        let now = Instant::now();
        let regions = noted(&[(0, None), (1, Some(now))]);
        assert_eq!(next_retry(&regions, &table(Some(0), &[0, 1], 0)), Some(now));
        assert_eq!(next_retry(&regions, &table(Some(0), &[0], 0)), None);
        assert_eq!(next_retry(&regions, &table(None, &[], 0)), None);
    }

    #[test]
    fn players_are_let_in_by_a_table_that_names_the_home_region_with_a_worker_and_nobody_waiting() {
        assert_eq!(whole(&table(Some(0), &[0], 0)), Some(RegionId(0)));
        assert_eq!(whole(&table(Some(2), &[0, 2, 5], 0)), Some(RegionId(2)));
    }

    #[test]
    fn a_table_of_a_coordinator_that_has_not_read_the_list_lets_nobody_in() {
        // It knows no region at all, which is no region waiting, or it knows those its
        // workers report: nothing says which of them players enter in.
        assert_eq!(whole(&table(None, &[], 0)), None);
        assert_eq!(whole(&table(None, &[0, 1], 0)), None);
    }

    #[test]
    fn a_table_whose_home_region_has_no_worker_or_with_a_region_waiting_lets_nobody_in() {
        assert_eq!(whole(&table(Some(0), &[], 1)), None);
        assert_eq!(whole(&table(Some(0), &[1], 0)), None);
        assert_eq!(whole(&table(Some(0), &[0], 1)), None);
    }
}
