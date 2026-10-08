//! The coordinator as a service that workers and edges reach over TCP.
//!
//! Every client has one connection, and the first thing it says decides what the
//! connection is: a worker registers, an edge asks for the routing table, and whoever
//! operates the cluster asks for a region to be moved. A single task owns the
//! [`Coordinator`] and all connections. It turns what clients say into calls
//! and what the calls changed into messages, and it never waits for a client: what it
//! has to say is queued, and a client that does not take it is dropped.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine_region::{RegionId, RoutingTable};
use clustine_rpc::link::{End, Receiver, Sender};
use clustine_rpc::{Assignment, FromCoordinator, ToCoordinator, tcp};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

use crate::QUEUE;
use crate::state::{Changes, Coordinator, CoordinatorConfig, MoveBegun, MoveOutcome};

/// What clients have said and the service has not got to yet, for all of them together.
/// While it is full the tasks that read from clients wait, which holds back nobody but
/// the clients that talk.
const SAID_CAPACITY: usize = 1024;

/// How long the service waits before it accepts connections again after that failed.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// The service does not look at the leases more often than this, however short they are.
const SHORTEST_TICK: Duration = Duration::from_millis(50);

impl CoordinatorConfig {
    /// The lease of a coordinator that is not told another: how long the players of a
    /// worker that died stand still before another takes over (ADR-0008). Workers send a
    /// heartbeat every [`crate::HEARTBEAT_INTERVAL`], so several of them have to get lost
    /// in a row before a worker that is there loses its regions.
    pub const DEFAULT_LEASE: Duration = Duration::from_secs(5);
}

/// The service's end of the connection to a client.
type ClientEnd = End<FromCoordinator, ToCoordinator>;

/// What a client said on the connection with this number, or `None` once the connection
/// has ended.
type Said = (u64, Option<ToCoordinator>);

/// Runs a coordinator on `listener` until the future is dropped, which closes every
/// connection. It only returns if the listener is of no use from the start.
///
/// Workers reach the coordinator with [`crate::WorkerClient`], edges with
/// [`crate::RoutingWatch`], and whoever wants a region moved with [`crate::Mover`]. A
/// worker whose connection ends keeps its regions until its lease runs out, and for good
/// if it registers again before that, unless it had said that it is leaving: then it is
/// taken to be gone.
pub async fn serve(listener: TcpListener, config: CoordinatorConfig) -> io::Result<()> {
    // The wall clock is all a coordinator has of those before it: it went on while they
    // ran, so this one starts above whatever they issued.
    serve_from(listener, config, unix_milliseconds()).await
}

/// [`serve`] for a coordinator that issues no epoch at or below `first_epoch`.
async fn serve_from(
    listener: TcpListener,
    config: CoordinatorConfig,
    first_epoch: u64,
) -> io::Result<()> {
    let address = listener.local_addr()?;
    let lease = config.lease;
    info!(%address, ?lease, first_epoch, "the coordinator is listening");
    let mut service = Service::new(config, now(), first_epoch);

    let mut ticks = tokio::time::interval(tick_interval(lease));
    // A tick that comes late does the work of those it would have to catch up with.
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // After a failure only accepting pauses; the clients that are there are served on.
    let mut accepting = true;
    let pause = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(pause);
    loop {
        tokio::select! {
            accepted = listener.accept(), if accepting => match accepted {
                Ok((stream, peer)) => {
                    let connection = service.attach(tcp::link(stream, QUEUE), now());
                    debug!(connection, %peer, "a client connected");
                }
                Err(error) => {
                    // Typically the process is out of file descriptors, and trying
                    // again at once would do nothing but fill the log.
                    warn!(%error, "accepting a connection failed");
                    accepting = false;
                    pause.as_mut().reset(tokio::time::Instant::now() + ACCEPT_RETRY);
                }
            },
            () = &mut pause, if !accepting => accepting = true,
            Some((connection, message)) = service.said.recv() => {
                service.hear(now(), connection, message);
            }
            _ = ticks.tick() => service.tick(now()),
        }
    }
}

/// The time for the coordinator's leases, by the clock that the ticks follow as well.
fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}

/// The time on the wall clock, in milliseconds since the Unix epoch.
fn unix_milliseconds() -> u64 {
    // A clock that is set to a time before 1970 is of no help. The coordinator then has
    // to go by what the workers report alone.
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
}

/// How often the service lets the coordinator look at its leases. A lease is overrun by
/// at most this much before the worker is forgotten.
fn tick_interval(lease: Duration) -> Duration {
    (lease / 4).max(SHORTEST_TICK)
}

/// What a connection is, which the first thing its client says decides.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    /// The client has not said anything yet.
    Undecided,
    /// The worker of this name registered over it.
    Worker(String),
    /// An edge that is sent every routing table.
    Watcher,
    /// Somebody asked over it for a region to be moved, and waits to hear how that
    /// ended. The connection is closed once they have been told.
    Mover,
}

impl Role {
    /// Whether a worker called `name` may register over the connection. A connection
    /// is one worker's, which may register again and again.
    fn admits(&self, name: &str) -> bool {
        match self {
            Self::Undecided => true,
            Self::Worker(registered) => registered == name,
            Self::Watcher | Self::Mover => false,
        }
    }
}

/// The connection to a client.
#[derive(Debug)]
struct Connection {
    role: Role,
    /// Queues what the client is told.
    sender: Sender<FromCoordinator>,
    /// When the client last said something, or connected.
    heard: Instant,
    /// Stops the task that reads what the client says.
    reader: AbortHandle,
}

/// A coordinator and the connections to its clients.
///
/// Nothing here waits, and every call is given the time, as with the [`Coordinator`]
/// itself. [`serve`] makes the calls as things happen; a test can make them by hand.
#[derive(Debug)]
struct Service {
    coordinator: Coordinator,
    /// The open connections by their numbers.
    connections: BTreeMap<u64, Connection>,
    /// The number of the connection each connected worker last registered over.
    workers: BTreeMap<String, u64>,
    /// The workers whose connection has been closed and of which the coordinator has
    /// not been told yet. It is told before [`Service::hear`] or [`Service::tick`]
    /// return: where connections are closed, the time is not at hand, and what the
    /// coordinator makes of it may close more of them.
    lost: Vec<String>,
    /// The number the next connection gets.
    next_connection: u64,
    /// The tasks that read what the clients say. They stop when the service is dropped.
    readers: JoinSet<()>,
    /// Where those tasks pass on what they read, and where it comes out.
    said_sender: mpsc::Sender<Said>,
    said: mpsc::Receiver<Said>,
}

impl Service {
    fn new(config: CoordinatorConfig, now: Instant, first_epoch: u64) -> Self {
        let (said_sender, said) = mpsc::channel(SAID_CAPACITY);
        Self {
            coordinator: Coordinator::new(config, now, first_epoch),
            connections: BTreeMap::new(),
            workers: BTreeMap::new(),
            lost: Vec::new(),
            next_connection: 0,
            readers: JoinSet::new(),
            said_sender,
            said,
        }
    }

    /// Takes on a client that has connected and returns the number of its connection.
    /// Must be called within a tokio runtime.
    fn attach(&mut self, link: ClientEnd, now: Instant) -> u64 {
        // Reap the readers that are done so the set does not grow without bound.
        while self.readers.try_join_next().is_some() {}

        let id = self.next_connection;
        self.next_connection += 1;
        let (sender, receiver) = link.split();
        let reader = self
            .readers
            .spawn(pass_on(id, receiver, self.said_sender.clone()));
        let connection = Connection {
            role: Role::Undecided,
            sender,
            heard: now,
            reader,
        };
        self.connections.insert(id, connection);
        id
    }

    /// Deals with what the client of the connection `id` said, or with the end of the
    /// connection if `message` is `None`.
    fn hear(&mut self, now: Instant, id: u64, message: Option<ToCoordinator>) {
        self.heard(now, id, message);
        self.report_lost(now);
    }

    /// Tells the coordinator of every worker whose connection has been closed since it
    /// was last told, and acts on what that changes.
    fn report_lost(&mut self, now: Instant) {
        while let Some(name) = self.lost.pop() {
            // The worker may have registered over another connection in the meantime.
            if self.workers.contains_key(&name) {
                continue;
            }
            let changes = self.coordinator.disconnected(now, &name);
            self.announce(&changes, None);
        }
    }

    /// What [`Service::hear`] does before the coordinator is told of lost connections.
    fn heard(&mut self, now: Instant, id: u64, message: Option<ToCoordinator>) {
        // The service may have closed the connection after the client said this.
        let Some(connection) = self.connections.get_mut(&id) else {
            return;
        };
        connection.heard = now;
        let Some(message) = message else {
            // Nothing is taken from a worker for this, unless it is leaving: it may be
            // back before its lease is out, and what it runs goes on running in the
            // meantime.
            debug!(connection = id, "a client closed its connection");
            self.close(id);
            return;
        };
        match (&connection.role, message) {
            (Role::Undecided, ToCoordinator::WatchRouting) => {
                connection.role = Role::Watcher;
                debug!(connection = id, "an edge asked for the routing table");
                let table = self.coordinator.routing_table();
                self.tell(id, FromCoordinator::Routing(table));
            }
            (
                role,
                ToCoordinator::RegisterWorker {
                    name,
                    address,
                    holding,
                    layout,
                },
            ) if role.admits(&name) => self.register(now, id, name, &address, &holding, layout),
            (Role::Worker(name), ToCoordinator::Heartbeat { regions }) => {
                if !self.coordinator.heartbeat(now, name, &regions) {
                    // The worker has to register again, and nothing but the end of its
                    // connection can tell it so. A lease and the connection of a silent
                    // worker end at the same tick, so it should not come to this; if it
                    // did, the worker would go on believing that it is registered.
                    info!(
                        worker = %name,
                        connection = id,
                        "heard from a worker that is not registered"
                    );
                    self.close(id);
                }
            }
            (Role::Worker(name), ToCoordinator::EpochRefused { region, seen }) => {
                info!(worker = %name, %region, seen, "the world store refused an epoch");
                let name = name.clone();
                let changes = self.coordinator.epoch_refused(now, &name, region, seen);
                // The region the worker dropped has gone to a waiting worker, unless
                // there is none or the coordinator is new; then it is without an owner
                // until a tick gives it away.
                self.announce(&changes, None);
            }
            (Role::Worker(name), ToCoordinator::Released { region, epoch }) => {
                let name = name.clone();
                let changes = self.coordinator.released(now, &name, region, epoch);
                self.announce(&changes, None);
            }
            (Role::Worker(name), ToCoordinator::Leaving) => {
                let name = name.clone();
                let changes = self.coordinator.leaving(now, &name);
                self.announce(&changes, None);
            }
            (Role::Undecided, ToCoordinator::Move { region, to }) => {
                connection.role = Role::Mover;
                self.move_region(now, id, region, to.as_deref());
            }
            (role, message) => {
                let said = match message {
                    ToCoordinator::RegisterWorker { .. } => "a registration",
                    ToCoordinator::Heartbeat { .. } => "a heartbeat",
                    ToCoordinator::EpochRefused { .. } => "a refused epoch",
                    ToCoordinator::WatchRouting => "a request for the routing table",
                    ToCoordinator::Released { .. } => "that it released a region",
                    ToCoordinator::Leaving => "that it is leaving",
                    ToCoordinator::Move { .. } => "a request to move a region",
                };
                warn!(
                    connection = id,
                    ?role,
                    said,
                    "a client said what it may not say"
                );
                self.close(id);
            }
        }
    }

    /// A worker registers over the connection `id`, for the first time or again.
    fn register(
        &mut self,
        now: Instant,
        id: u64,
        name: String,
        address: &str,
        holding: &[Assignment],
        layout: Option<u64>,
    ) {
        info!(
            worker = %name,
            address,
            holding = %Holdings(holding),
            layout = %Fingerprint(layout),
            connection = id,
            "a worker registers"
        );
        let changes = match self
            .coordinator
            .register(now, &name, address, holding, layout)
        {
            Ok(changes) => changes,
            Err(refusal) => {
                info!(
                    worker = %name,
                    connection = id,
                    reason = %refusal,
                    "refused a worker"
                );
                let reason = refusal.to_string();
                self.tell(id, FromCoordinator::Refused { reason });
                self.close(id);
                return;
            }
        };

        // From now on this is the connection the worker is told things over. An
        // earlier one is of no use to it any more, or belongs to another process that
        // goes by its name; either way two would keep one lease alive.
        if let Some(earlier) = self.workers.insert(name.clone(), id)
            && earlier != id
        {
            info!(
                worker = %name,
                connection = earlier,
                "closing the earlier connection of a worker"
            );
            self.close(earlier);
        }
        if let Some(connection) = self.connections.get_mut(&id) {
            connection.role = Role::Worker(name.clone());
        }
        // The answer, whether or not anything is different for the worker.
        self.assign(&name);
        self.announce(&changes, Some(&name));
    }

    /// Somebody asks over the connection `id` for `region` to be moved, to the worker
    /// `to` or to any. They are told at once whether it has begun, and if it has, they
    /// are told how it ended by the call in which it ends.
    fn move_region(&mut self, now: Instant, id: u64, region: RegionId, to: Option<&str>) {
        match self.coordinator.move_region(now, region, to, id) {
            Ok((MoveBegun { from, to }, changes)) => {
                info!(connection = id, %region, %from, %to, "a move has begun");
                self.tell(id, FromCoordinator::MoveBegun { from, to });
                self.announce(&changes, None);
            }
            Err(refusal) => {
                info!(connection = id, %region, reason = %refusal, "refused a move");
                let reason = refusal.to_string();
                self.tell(id, FromCoordinator::MoveRefused { reason });
                self.close(id);
            }
        }
    }

    /// Lets leases run out and regions be handed out, and closes the connections whose
    /// clients have fallen silent.
    fn tick(&mut self, now: Instant) {
        let changes = self.coordinator.tick(now);
        // First, so that a worker which lost its regions is told before it is cut off.
        self.announce(&changes, None);

        // Whoever has not even said what it is, or is a worker and has let its lease
        // run out, is taken to be gone without having closed its connection. Edges are
        // left alone: they have nothing to say once they have asked for the table. So
        // is whoever waits to hear of a move, which ends within a lease by itself.
        let lease = self.coordinator.config().lease;
        let silent: Vec<u64> = self
            .connections
            .iter()
            .filter(|(_, connection)| !matches!(connection.role, Role::Watcher | Role::Mover))
            .filter(|(_, connection)| now.saturating_duration_since(connection.heard) > lease)
            .map(|(id, _)| *id)
            .collect();
        for id in silent {
            info!(
                connection = id,
                "closing a connection whose client is silent"
            );
            self.close(id);
        }
        self.report_lost(now);
    }

    /// Tells workers, edges and movers what a call into the coordinator changed, and
    /// closes the connections of the workers that have left. `told` is a worker that
    /// has been told its assignments already.
    fn announce(&mut self, changes: &Changes, told: Option<&str>) {
        for name in &changes.workers {
            if told != Some(name.as_str()) {
                self.assign(name);
            }
        }
        // After the assignments, so that a worker hears of a release in order with
        // them. A worker without a connection is asked again when it registers.
        for order in &changes.releases {
            if let Some(&id) = self.workers.get(&order.worker) {
                let (region, epoch) = (order.region, order.epoch);
                self.tell(id, FromCoordinator::Release { region, epoch });
            }
        }
        if changes.routing {
            self.announce_routing();
        }
        for outcome in &changes.moves {
            let id = outcome.mover;
            info!(
                connection = id,
                region = %outcome.region,
                owner = ?outcome.owner,
                released = outcome.released,
                "a move has ended"
            );
            self.tell(id, outcome_message(outcome));
            self.close(id);
        }
        // Last, so that such a worker has been told that it runs nothing.
        for name in &changes.gone {
            if let Some(&id) = self.workers.get(name) {
                info!(
                    worker = %name,
                    connection = id,
                    "closing the connection of a worker that has left"
                );
                self.close(id);
            }
        }
    }

    /// Sends every edge the routing table.
    fn announce_routing(&mut self) {
        let table = self.coordinator.routing_table();
        info!(
            version = table.version,
            regions = %Routes(&table),
            "the routing table changed"
        );
        let watchers: Vec<u64> = self
            .connections
            .iter()
            .filter(|(_, connection)| connection.role == Role::Watcher)
            .map(|(id, _)| *id)
            .collect();
        for id in watchers {
            self.tell(id, FromCoordinator::Routing(table.clone()));
        }
    }

    /// Tells the worker `name` what it is to run, if it is connected.
    fn assign(&mut self, name: &str) {
        let Some(&id) = self.workers.get(name) else {
            return;
        };
        let config = self.coordinator.config();
        let orders = FromCoordinator::Assigned {
            layout: config.layout.clone(),
            spawn: config.spawn,
            assignments: self.coordinator.assignments(name),
        };
        self.tell(id, orders);
    }

    /// Queues `message` for the client of the connection `id`. A client that is so far
    /// behind that its queue is full is dropped: the service waits for nobody.
    fn tell(&mut self, id: u64, message: FromCoordinator) {
        let Some(connection) = self.connections.get(&id) else {
            return;
        };
        if let Err(error) = connection.sender.try_send(message) {
            warn!(connection = id, %error, "giving up on a client");
            self.close(id);
        }
    }

    /// Closes the connection `id`. What is queued for its client still goes out. If it
    /// is the connection a worker last registered over, the coordinator is told that
    /// the worker has none before [`Service::hear`] or [`Service::tick`] return.
    fn close(&mut self, id: u64) {
        let Some(connection) = self.connections.remove(&id) else {
            return;
        };
        connection.reader.abort();
        if let Role::Worker(name) = connection.role
            // The worker may have registered over another connection since.
            && self.workers.get(&name) == Some(&id)
        {
            self.workers.remove(&name);
            self.lost.push(name);
        }
    }
}

/// What whoever asked for a move is told about how it ended.
///
/// A region that nobody could be given is not anybody's "now", so there is no
/// [`FromCoordinator::MoveDone`] to say: the mover is told that the move is not done and
/// why, although the region has left its old owner, and hears no more. The region is
/// assigned like any region without an owner as soon as a worker waits.
fn outcome_message(outcome: &MoveOutcome) -> FromCoordinator {
    match &outcome.owner {
        Some((to, epoch)) => FromCoordinator::MoveDone {
            to: to.clone(),
            epoch: *epoch,
            released: outcome.released,
        },
        None => {
            let region = outcome.region;
            let how = if outcome.released {
                "was released by its owner"
            } else {
                "was taken from its owner, which did not release it"
            };
            let reason = format!(
                "region {region} {how}, but no worker waits to be given it; \
                 it is without an owner until one does"
            );
            FromCoordinator::MoveRefused { reason }
        }
    }
}

/// Passes on what the client of the connection `id` says and, last of all, that the
/// connection has ended.
async fn pass_on(id: u64, mut client: Receiver<ToCoordinator>, said: mpsc::Sender<Said>) {
    loop {
        let message = client.recv().await;
        let ended = message.is_none();
        // Nobody takes it if the service is gone.
        if said.send((id, message)).await.is_err() || ended {
            return;
        }
    }
}

/// What a worker reports to be running, for the log.
struct Holdings<'a>(&'a [Assignment]);

impl fmt::Display for Holdings<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str("nothing");
        }
        for (index, held) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            let Assignment {
                region,
                epoch,
                entity_ids,
            } = held;
            let (first, end) = (entity_ids.first.0, entity_ids.end.0);
            write!(
                formatter,
                "region {region} at epoch {epoch} with entity ids {first}..{end}"
            )?;
        }
        Ok(())
    }
}

/// The fingerprint of the layout a worker reports, for the log; written the way a
/// refusal writes it.
struct Fingerprint(Option<u64>);

impl fmt::Display for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(fingerprint) => write!(formatter, "{fingerprint:#018x}"),
            None => formatter.write_str("none"),
        }
    }
}

/// Every region of a routing table with where its worker is reached and the epoch, for
/// the log.
struct Routes<'a>(&'a RoutingTable);

impl fmt::Display for Routes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (region, _)) in self.0.layout.regions().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            match self.0.route(region) {
                Some(route) => {
                    let (address, epoch) = (&route.address, route.epoch);
                    write!(formatter, "region {region} at {address} with epoch {epoch}")?;
                }
                None => write!(formatter, "region {region} without an owner")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use clustine_region::{Layout, RegionId, RegionRoute};
    use clustine_rpc::{Vouch, link};
    use clustine_world::{EntityIds, Vec3};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    use super::*;
    use crate::client::{
        ClientError, MoveAnswer, Mover, Orders, RoutingWatch, WorkerClient, WorkerEvent,
    };
    use crate::state::Refusal;

    /// The lease of the coordinators in these tests: long enough for a worker to keep it
    /// on a busy machine, short enough to wait until one has run out.
    const LEASE: Duration = Duration::from_millis(600);

    /// How often the workers of these tests say that they are there.
    const HEARTBEAT: Duration = Duration::from_millis(50);

    /// How long a test waits for something that should happen within a few leases.
    const PATIENCE: Duration = Duration::from_secs(20);

    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

    /// A client's end of a connection that a test drives by hand.
    type RawEnd = End<ToCoordinator, FromCoordinator>;

    /// A coordinator that is being served.
    struct Served {
        /// Where clients reach it.
        address: String,
        layout: Layout,
        task: JoinHandle<io::Result<()>>,
    }

    impl Served {
        /// Starts a coordinator for a world with region boundaries at these chunk x
        /// coordinates.
        async fn start(boundaries: &[i32]) -> Self {
            let (listener, address) = listen().await;
            let config = config(boundaries);
            Self {
                address,
                layout: config.layout.clone(),
                task: tokio::spawn(serve(listener, config)),
            }
        }

        /// The same, for a coordinator whose clock says `first_epoch`.
        async fn start_at(boundaries: &[i32], first_epoch: u64) -> Self {
            let (listener, address) = listen().await;
            let config = config(boundaries);
            Self {
                address,
                layout: config.layout.clone(),
                task: tokio::spawn(serve_from(listener, config, first_epoch)),
            }
        }

        /// The coordinator goes away: the future of [`serve`] is dropped.
        async fn stop(self) {
            self.task.abort();
            assert!(self.task.await.unwrap_err().is_cancelled());
        }

        /// Registers a worker that edges reach at an address made from its name.
        async fn worker(&self, name: &str, holding: &[Assignment]) -> (WorkerClient, Orders) {
            self.worker_at(name, &address_of(name), holding).await
        }

        async fn worker_at(
            &self,
            name: &str,
            address: &str,
            holding: &[Assignment],
        ) -> (WorkerClient, Orders) {
            let layout = Some(self.layout.fingerprint());
            let registering = WorkerClient::register_with_heartbeat(
                &self.address,
                name,
                address,
                holding,
                layout,
                HEARTBEAT,
            );
            within(registering).await.unwrap()
        }

        async fn watch(&self) -> RoutingWatch {
            within(RoutingWatch::connect(&self.address)).await.unwrap()
        }

        /// A connection on which nothing has been said yet.
        async fn connect(&self) -> RawEnd {
            let stream = within(TcpStream::connect(&self.address)).await.unwrap();
            tcp::link(stream, 8)
        }

        /// What a worker is told that is to run `assignments`.
        fn orders(&self, assignments: &[Assignment]) -> Orders {
            Orders {
                layout: self.layout.clone(),
                spawn: SPAWN,
                assignments: assignments.to_vec(),
            }
        }
    }

    /// Something for a coordinator to be served on, and its address.
    async fn listen() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        (listener, address)
    }

    fn config(boundaries: &[i32]) -> CoordinatorConfig {
        CoordinatorConfig {
            layout: Layout::new(boundaries.to_vec()).unwrap(),
            spawn: SPAWN,
            lease: LEASE,
        }
    }

    /// A coordinator, the workers `a` and `b` with the two westernmost regions, which
    /// they were given in the order they registered, and an edge that has seen as much.
    struct Cluster {
        served: Served,
        a: WorkerClient,
        b: WorkerClient,
        held_a: Assignment,
        held_b: Assignment,
        watch: RoutingWatch,
        /// The last table the edge was sent.
        table: RoutingTable,
    }

    impl Cluster {
        async fn start(boundaries: &[i32]) -> Self {
            Self::of(Served::start(boundaries).await).await
        }

        async fn of(served: Served) -> Self {
            let mut watch = served.watch().await;
            let (mut a, orders) = served.worker("a", &[]).await;
            assert_eq!(orders, served.orders(&[]));
            let (mut b, orders) = served.worker("b", &[]).await;
            assert_eq!(orders, served.orders(&[]));

            let held_a = next_region(&mut a).await;
            let held_b = next_region(&mut b).await;
            assert_eq!(held_a.region, RegionId(0));
            assert_eq!(held_b.region, RegionId(1));
            assert!(held_a.epoch < held_b.epoch);
            assert_ne!(held_a.entity_ids, held_b.entity_ids);

            let table = table_where(&mut watch, |table| table.routes.len() == 2).await;
            assert_eq!(table.routes, [route(held_a, "a"), route(held_b, "b")]);
            assert_eq!(table.layout, served.layout);
            assert_eq!(table.spawn, SPAWN);
            Self {
                served,
                a,
                b,
                held_a,
                held_b,
                watch,
                table,
            }
        }
    }

    /// Waits for `future`, but not for ever.
    async fn within<T>(future: impl Future<Output = T>) -> T {
        timeout(PATIENCE, future)
            .await
            .expect("this should not take so long")
    }

    /// Where edges reach the worker `name` unless a test says otherwise.
    fn address_of(name: &str) -> String {
        format!("{name}:25601")
    }

    /// What edges are told about a region that the worker `name` runs.
    fn route(assignment: Assignment, name: &str) -> RegionRoute {
        RegionRoute {
            region: assignment.region,
            epoch: assignment.epoch,
            address: address_of(name),
        }
    }

    fn assignment(region: u32, epoch: u64, block: u32) -> Assignment {
        Assignment {
            region: RegionId(region),
            epoch,
            entity_ids: EntityIds::block(block).unwrap(),
        }
    }

    /// What a worker of a world with this layout is told that is to run `assignments`,
    /// as the message that it is on a connection.
    fn assigned(layout: &Layout, assignments: &[Assignment]) -> FromCoordinator {
        FromCoordinator::Assigned {
            layout: layout.clone(),
            spawn: SPAWN,
            assignments: assignments.to_vec(),
        }
    }

    fn registration(name: &str, holding: &[Assignment]) -> ToCoordinator {
        ToCoordinator::RegisterWorker {
            name: name.to_owned(),
            address: address_of(name),
            holding: holding.to_vec(),
            layout: None,
        }
    }

    /// The next orders of a worker, which are to run one region: that region.
    async fn next_region(worker: &mut WorkerClient) -> Assignment {
        let orders = within(worker.next()).await.unwrap();
        assert_eq!(orders.assignments.len(), 1, "{orders:?}");
        orders.assignments[0]
    }

    /// The next table an edge is sent of which `wanted` holds.
    async fn table_where(
        watch: &mut RoutingWatch,
        wanted: impl Fn(&RoutingTable) -> bool,
    ) -> RoutingTable {
        within(async {
            loop {
                let table = watch.next().await.unwrap();
                if wanted(&table) {
                    return table;
                }
            }
        })
        .await
    }

    /// The next thing a worker is told, which is to release a region: the region and
    /// the epoch.
    async fn next_release(worker: &mut WorkerClient) -> (RegionId, u64) {
        match within(worker.event()).await.unwrap() {
            WorkerEvent::Release { region, epoch } => (region, epoch),
            other => panic!("{other:?} is no request to release a region"),
        }
    }

    /// Asks the coordinator to move `region`, to the worker `to` or to any.
    async fn ask_to_move(served: &Served, region: u32, to: Option<&str>) -> Mover {
        within(Mover::ask(&served.address, RegionId(region), to))
            .await
            .unwrap()
    }

    /// Whether the next thing a worker hears is that its connection is lost.
    async fn is_lost_next(worker: &mut WorkerClient) -> bool {
        matches!(within(worker.next()).await, Err(ClientError::Lost))
    }

    #[tokio::test]
    async fn two_workers_are_each_given_a_region_and_an_edge_sees_the_table_become_complete() {
        let before = unix_milliseconds();
        let started = Instant::now();
        let served = Served::start(&[0]).await;
        let mut watch = served.watch().await;
        let empty = within(watch.next()).await.unwrap();
        // Versions and epochs start at the time on the wall clock.
        assert!((before..=unix_milliseconds()).contains(&empty.version));
        assert!(empty.routes.is_empty());
        assert!(!empty.is_complete());
        drop(watch);

        let Cluster {
            held_a,
            held_b,
            table,
            ..
        } = Cluster::of(served).await;
        // Nothing was given away before the coordinator had been there for a lease.
        assert!(started.elapsed() >= LEASE);
        assert!(held_a.epoch > empty.version);
        assert!(table.is_complete());
        assert!(table.version > empty.version);
        assert_eq!(table.route(RegionId(0)), Some(&route(held_a, "a")));
        assert_eq!(table.route(RegionId(1)), Some(&route(held_b, "b")));
    }

    #[tokio::test]
    async fn an_edge_that_connects_later_is_given_the_complete_table_at_once() {
        let cluster = Cluster::start(&[0]).await;
        assert!(cluster.table.is_complete());
        let mut late = cluster.served.watch().await;
        assert_eq!(within(late.next()).await.unwrap(), cluster.table);
    }

    #[tokio::test]
    async fn a_waiting_worker_is_given_the_region_of_one_that_vanished_once_its_lease_is_out() {
        let served = Served::start(&[0]).await;
        let mut watch = served.watch().await;
        let (mut stays, _) = served.worker("stays", &[]).await;
        let west = next_region(&mut stays).await;
        assert_eq!(west.region, RegionId(0));

        // The coordinator has been there for a lease by now, so this worker is given
        // the other region at the next tick. It is heard from no earlier than now.
        let heard = Instant::now();
        let (mut vanishes, _) = served.worker("vanishes", &[]).await;
        let east = next_region(&mut vanishes).await;
        assert_eq!(east.region, RegionId(1));
        let table = table_where(&mut watch, RoutingTable::is_complete).await;
        assert_eq!(
            table.routes,
            [route(west, "stays"), route(east, "vanishes")]
        );

        let (mut waits, orders) = served.worker("waits", &[]).await;
        assert_eq!(orders, served.orders(&[]));
        drop(vanishes);

        let taken = next_region(&mut waits).await;
        assert!(heard.elapsed() > LEASE);
        assert_eq!(taken.region, RegionId(1));
        // A higher epoch than any before, and entity ids nobody has had.
        assert!(taken.epoch > east.epoch && east.epoch > west.epoch);
        assert_ne!(taken.entity_ids, east.entity_ids);
        assert_ne!(taken.entity_ids, west.entity_ids);

        // The edge sees the region change hands, and nothing else.
        let after = within(watch.next()).await.unwrap();
        assert_eq!(after.version, table.version + 1);
        assert_eq!(after.routes, [route(west, "stays"), route(taken, "waits")]);
        // The worker that stayed was told nothing: it runs what it ran.
        served.stop().await;
        assert!(is_lost_next(&mut stays).await);
    }

    #[tokio::test]
    async fn a_worker_back_within_its_lease_keeps_its_region_and_only_its_address_is_news() {
        let Cluster {
            served,
            a,
            mut b,
            held_a,
            held_b,
            mut watch,
            table,
        } = Cluster::start(&[-8, 8]).await;

        // The connection of `a` ends, and a tick goes by without it: the one at which a
        // worker that has come since is given a region. It is the one nobody had and
        // not the one `a` runs, because a lost connection is not the end of a worker.
        drop(a);
        let (mut c, _) = served.worker("c", &[]).await;
        let held_c = next_region(&mut c).await;
        assert_eq!(held_c.region, RegionId(2));
        let with_c = within(watch.next()).await.unwrap();
        assert_eq!(with_c.version, table.version + 1);
        let routes = [route(held_a, "a"), route(held_b, "b"), route(held_c, "c")];
        assert_eq!(with_c.routes, routes);

        // Back at the same address, the worker carries on.
        let (mut back, orders) = served.worker("a", &[held_a]).await;
        assert_eq!(orders, served.orders(&[held_a]));

        // At another address too, and that is news to the edge. This time the worker
        // has not closed the connection it had; the coordinator does that.
        let (mut moved, orders) = served.worker_at("a", "elsewhere:25601", &[held_a]).await;
        assert_eq!(orders, served.orders(&[held_a]));
        assert!(is_lost_next(&mut back).await);
        let elsewhere = within(watch.next()).await.unwrap();
        // One version on: the edge heard nothing of the worker coming back before.
        assert_eq!(elsewhere.version, with_c.version + 1);
        let mut routes = routes;
        routes[0].address = "elsewhere:25601".to_owned();
        assert_eq!(elsewhere.routes, routes);

        // Nobody was told anything else. When the coordinator goes away, that is the
        // next thing each of them hears.
        served.stop().await;
        for worker in [&mut b, &mut c, &mut moved] {
            assert!(is_lost_next(worker).await);
        }
        let lost = within(watch.next()).await;
        assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
    }

    #[tokio::test]
    async fn a_worker_with_another_layout_is_refused_and_told_why() {
        let served = Served::start(&[0]).await;
        let expected = served.layout.fingerprint();
        let reported = Layout::single().fingerprint();
        let registering =
            WorkerClient::register(&served.address, "a", "a:25601", &[], Some(reported));
        let refused = within(registering).await;
        let reason = Refusal::Layout { reported, expected }.to_string();
        assert!(
            matches!(&refused, Err(ClientError::Refused(told)) if *told == reason),
            "{refused:?}"
        );

        // With the coordinator's layout the same worker is welcome, and so is one that
        // names none.
        let (_a, orders) = served.worker("a", &[]).await;
        assert_eq!(orders, served.orders(&[]));
        let registering = WorkerClient::register(&served.address, "b", "b:25601", &[], None);
        let (_b, orders) = within(registering).await.unwrap();
        assert_eq!(orders, served.orders(&[]));
    }

    #[tokio::test]
    async fn workers_keep_their_regions_under_a_new_coordinator_and_it_issues_higher_epochs() {
        // The clock of the first coordinator is a day ahead, so the one after it
        // cannot rely on its own to stay clear of what was issued.
        let ahead = unix_milliseconds() + 24 * 60 * 60 * 1000;
        let Cluster {
            served,
            mut a,
            mut b,
            held_a,
            held_b,
            table,
            ..
        } = Cluster::of(Served::start_at(&[0], ahead).await).await;
        assert!(held_a.epoch > ahead);
        served.stop().await;
        assert!(is_lost_next(&mut a).await);
        assert!(is_lost_next(&mut b).await);

        // Another coordinator at another address. The workers tell it what they run,
        // and go on running it.
        let served = Served::start(&[0]).await;
        let mut watch = served.watch().await;
        let (mut a, orders) = served.worker("a", &[held_a]).await;
        assert_eq!(orders, served.orders(&[held_a]));
        let (b, orders) = served.worker("b", &[held_b]).await;
        assert_eq!(orders, served.orders(&[held_b]));
        let again = table_where(&mut watch, RoutingTable::is_complete).await;
        assert_eq!(again.routes, table.routes);
        // Its versions are its own.
        assert!(again.version < ahead);

        // What it issues itself is above everything they reported.
        let (mut c, orders) = served.worker("c", &[]).await;
        assert_eq!(orders, served.orders(&[]));
        drop(b);
        let taken = next_region(&mut c).await;
        assert_eq!(taken.region, held_b.region);
        assert!(taken.epoch > held_b.epoch && held_b.epoch > held_a.epoch);
        assert_ne!(taken.entity_ids, held_a.entity_ids);
        assert_ne!(taken.entity_ids, held_b.entity_ids);
        let after = within(watch.next()).await.unwrap();
        assert_eq!(after.version, again.version + 1);
        assert_eq!(after.routes, [route(held_a, "a"), route(taken, "c")]);

        // Each of them was told what it runs once, and nothing after that.
        served.stop().await;
        assert!(is_lost_next(&mut a).await);
        assert!(is_lost_next(&mut c).await);
    }

    #[tokio::test]
    async fn clients_that_say_nothing_or_the_wrong_thing_first_do_not_disturb_the_others() {
        let served = Served::start(&[0]).await;
        let mut silent = within(TcpStream::connect(&served.address)).await.unwrap();

        // Only a worker that has registered sends heartbeats.
        let mut hasty = served.connect().await;
        hasty
            .send(ToCoordinator::Heartbeat {
                regions: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(within(hasty.recv()).await, None);

        // A length, and then something that is no message.
        let mut garbled = within(TcpStream::connect(&served.address)).await.unwrap();
        garbled.write_all(&[0, 0, 0, 1, 9]).await.unwrap();
        let mut byte = [0];
        assert_eq!(within(garbled.read(&mut byte)).await.unwrap(), 0);

        // Workers and edges are served as ever.
        let cluster = Cluster::of(served).await;
        assert!(cluster.table.is_complete());

        // The client that says nothing was given a lease to say what it is.
        assert_eq!(within(silent.read(&mut byte)).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_connection_is_one_workers_and_an_edge_has_nothing_more_to_say() {
        let served = Served::start(&[0]).await;
        let mut watch = served.watch().await;
        let empty = within(watch.next()).await.unwrap();

        // The same worker may register as often as it likes, and is answered each time.
        let mut link = served.connect().await;
        let kept = assignment(1, 5, 1);
        for _ in 0..2 {
            link.send(registration("first", &[kept])).await.unwrap();
            let answer = assigned(&served.layout, &[kept]);
            assert_eq!(within(link.recv()).await, Some(answer));
        }
        // Another may not, and is not registered: the region it says it runs stays free.
        // Nor was the first worker told anything but the two answers.
        let claim = assignment(0, 7, 3);
        link.send(registration("second", &[claim])).await.unwrap();
        assert_eq!(within(link.recv()).await, None);
        let with_first = within(watch.next()).await.unwrap();
        assert_eq!(with_first.version, empty.version + 1);
        assert_eq!(with_first.routes, [route(kept, "first")]);

        // So the next worker to say that it runs the region goes on running it.
        let held = assignment(0, 9, 4);
        let (_third, orders) = served.worker("third", &[held]).await;
        assert_eq!(orders, served.orders(&[held]));
        let table = within(watch.next()).await.unwrap();
        assert_eq!(table.version, empty.version + 2);
        assert_eq!(table.routes, [route(held, "third"), route(kept, "first")]);

        // An edge that says anything after asking for the table is cut off as well.
        let mut edge = served.connect().await;
        edge.send(ToCoordinator::WatchRouting).await.unwrap();
        assert_eq!(
            within(edge.recv()).await,
            Some(FromCoordinator::Routing(table))
        );
        edge.send(ToCoordinator::Heartbeat {
            regions: Vec::new(),
        })
        .await
        .unwrap();
        assert_eq!(within(edge.recv()).await, None);
    }

    #[tokio::test]
    async fn workers_and_edges_are_told_when_the_coordinator_goes_away() {
        let Cluster {
            served,
            mut a,
            mut watch,
            ..
        } = Cluster::start(&[0]).await;
        let address = served.address.clone();
        served.stop().await;

        for _ in 0..2 {
            assert!(is_lost_next(&mut a).await);
            let lost = within(watch.next()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        }
        // And nobody listens at its address any more.
        let registering = WorkerClient::register(&address, "a", "a:25601", &[], None);
        let unreachable = within(registering).await;
        assert!(
            matches!(unreachable, Err(ClientError::Io(_))),
            "{unreachable:?}"
        );
    }

    /// Lets a service that is not being served deal with the next thing a client says,
    /// as if it were said at `now`.
    async fn hear_next(service: &mut Service, now: Instant) {
        let (connection, message) = within(service.said.recv()).await.unwrap();
        service.hear(now, connection, message);
    }

    /// The table in the next message to a client, if there is another message.
    async fn next_table(client: &mut RawEnd) -> Option<RoutingTable> {
        match within(client.recv()).await? {
            FromCoordinator::Routing(table) => Some(table),
            other => panic!("{other:?} is no routing table"),
        }
    }

    /// Between a queue and a client that does not read from a socket there are the
    /// buffers of the socket, of a size nobody knows. So here, for once, the links are
    /// within the process.
    #[tokio::test]
    async fn a_client_whose_queue_is_full_is_dropped_and_the_others_are_not_held_up() {
        const FIRST: u64 = 1000;
        let now = Instant::now();
        let mut service = Service::new(config(&[]), now, FIRST);

        // Two edges with room for two tables each. Only one of them reads.
        let (mut reads, end) = link::in_process(2);
        service.attach(end, now);
        let (mut stuck, end) = link::in_process(2);
        service.attach(end, now);
        for edge in [&reads, &stuck] {
            edge.send(ToCoordinator::WatchRouting).await.unwrap();
            hear_next(&mut service, now).await;
        }
        assert_eq!(next_table(&mut reads).await.unwrap().version, FIRST);

        // Whenever the worker registers from another address, there is a new table.
        let (mut worker, end) = link::in_process(8);
        service.attach(end, now);
        let held = assignment(0, 5, 0);
        for round in 1..=3 {
            let address = format!("a:{round}");
            let registration = ToCoordinator::RegisterWorker {
                name: "a".to_owned(),
                address: address.clone(),
                holding: vec![held],
                layout: None,
            };
            worker.send(registration).await.unwrap();
            hear_next(&mut service, now).await;
            let answer = within(worker.recv()).await;
            assert!(
                matches!(answer, Some(FromCoordinator::Assigned { .. })),
                "{answer:?}"
            );
            let table = next_table(&mut reads).await.unwrap();
            assert_eq!(table.version, FIRST + round);
            assert_eq!(table.routes[0].address, address);
        }

        // The table it was sent first and the next one filled the queue of the edge
        // that does not read. The one after that did not fit, and was its last.
        for version in [FIRST, FIRST + 1] {
            assert_eq!(next_table(&mut stuck).await.unwrap().version, version);
        }
        assert_eq!(next_table(&mut stuck).await, None);
    }

    /// A worker that is silent without having closed its connection, which the workers
    /// of the other tests never are. The times are made up, as in the coordinator's own
    /// tests.
    #[tokio::test]
    async fn a_worker_that_falls_silent_is_told_that_it_runs_nothing_and_is_cut_off() {
        const FIRST: u64 = 1000;
        let start = Instant::now();
        let mut service = Service::new(config(&[]), start, FIRST);
        let (mut edge, end) = link::in_process(8);
        service.attach(end, start);
        edge.send(ToCoordinator::WatchRouting).await.unwrap();
        hear_next(&mut service, start).await;

        let (mut worker, end) = link::in_process(8);
        service.attach(end, start);
        let held = assignment(0, 5, 0);
        worker.send(registration("a", &[held])).await.unwrap();
        hear_next(&mut service, start).await;
        let heard = start + LEASE / 2;
        worker
            .send(ToCoordinator::Heartbeat {
                regions: vec![(held.region, Vouch::Committed)],
            })
            .await
            .unwrap();
        hear_next(&mut service, heard).await;

        // Silence for exactly a lease is not too long. A moment more is.
        service.tick(heard + LEASE);
        service.tick(heard + LEASE + Duration::from_millis(1));
        let layout = Layout::single();
        for assignments in [&[held][..], &[]] {
            let told = within(worker.recv()).await;
            assert_eq!(told, Some(assigned(&layout, assignments)));
        }
        assert_eq!(within(worker.recv()).await, None);

        // The edge has been silent for longer, as edges are, and is still served: it
        // hears of the worker that takes the region over, as of everything before.
        let (other, end) = link::in_process(8);
        service.attach(end, heard + 2 * LEASE);
        other.send(registration("b", &[held])).await.unwrap();
        hear_next(&mut service, heard + 2 * LEASE).await;
        for (version, owner) in [(0, None), (1, Some("a")), (2, None), (3, Some("b"))] {
            let table = next_table(&mut edge).await.unwrap();
            assert_eq!(table.version, FIRST + version);
            let routes = Vec::from_iter(owner.map(|name| route(held, name)));
            assert_eq!(table.routes, routes);
        }
    }

    /// A service whose coordinator started at `start`, an edge that has asked it for the
    /// table and been sent the first, and the workers `a`, which reports that it runs
    /// `held`, and `b`, which runs nothing; both have been told so. All of it at `start`.
    async fn two_workers(
        start: Instant,
        first: u64,
        held: Assignment,
    ) -> (Service, RawEnd, RawEnd, RawEnd) {
        let mut service = Service::new(config(&[]), start, first);
        let (mut edge, end) = link::in_process(8);
        service.attach(end, start);
        edge.send(ToCoordinator::WatchRouting).await.unwrap();
        hear_next(&mut service, start).await;
        assert_eq!(next_table(&mut edge).await.unwrap().version, first);

        let layout = Layout::single();
        let mut workers = Vec::new();
        for (name, holding) in [("a", vec![held]), ("b", Vec::new())] {
            let (mut worker, end) = link::in_process(8);
            service.attach(end, start);
            worker.send(registration(name, &holding)).await.unwrap();
            hear_next(&mut service, start).await;
            let told = within(worker.recv()).await;
            assert_eq!(told, Some(assigned(&layout, &holding)));
            workers.push(worker);
        }
        let table = next_table(&mut edge).await.unwrap();
        assert_eq!(table.routes, [route(held, "a")]);
        let b = workers.pop().unwrap();
        let a = workers.pop().unwrap();
        (service, edge, a, b)
    }

    fn heartbeat(regions: &[(RegionId, Vouch)]) -> ToCoordinator {
        ToCoordinator::Heartbeat {
            regions: regions.to_vec(),
        }
    }

    /// The times are made up, as in the coordinator's own tests.
    #[tokio::test]
    async fn a_region_its_worker_no_longer_vouches_for_goes_to_another_while_both_are_heard_from() {
        const FIRST: u64 = 1000;
        let start = Instant::now();
        let held = assignment(0, 5, 0);
        let (mut service, mut edge, mut a, mut b) = two_workers(start, FIRST, held).await;

        // Both say that they are there, every quarter of a lease; `a` names its region
        // once, at the first of them, and then no more.
        let mut now = start;
        for step in 0..=4 {
            now = start + LEASE / 4 * step;
            let vouches: &[_] = if step == 0 {
                &[(held.region, Vouch::Committed)]
            } else {
                &[]
            };
            a.send(heartbeat(vouches)).await.unwrap();
            hear_next(&mut service, now).await;
            b.send(heartbeat(&[])).await.unwrap();
            hear_next(&mut service, now).await;
            service.tick(now);
        }
        // A lease after the vouch the region is still `a`'s; a moment later it is not.
        let later = now + Duration::from_millis(1);
        for worker in [&a, &b] {
            worker.send(heartbeat(&[])).await.unwrap();
            hear_next(&mut service, later).await;
        }
        service.tick(later);

        let layout = Layout::single();
        assert_eq!(within(a.recv()).await, Some(assigned(&layout, &[])));
        let taken = assignment(0, FIRST + 1, 1);
        assert_eq!(within(b.recv()).await, Some(assigned(&layout, &[taken])));
        let table = next_table(&mut edge).await.unwrap();
        assert_eq!(table.version, FIRST + 2);
        assert_eq!(table.routes, [route(taken, "b")]);
        // Nobody was cut off: `a` is still there, and waits.
        assert_eq!(service.connections.len(), 3);
        assert_eq!(service.workers.len(), 2);
    }

    #[tokio::test]
    async fn a_refused_epoch_takes_the_region_from_its_worker_and_it_comes_back_above_it() {
        const FIRST: u64 = 1000;
        let start = Instant::now();
        let held = assignment(0, 5, 0);
        let (mut service, mut edge, mut a, mut b) = two_workers(start, FIRST, held).await;

        // The store has seen an owner with an epoch far above anything the coordinator
        // knows of. The coordinator is new, so the region is without an owner for now.
        let seen = 5000;
        let refused = ToCoordinator::EpochRefused {
            region: held.region,
            seen,
        };
        a.send(refused).await.unwrap();
        hear_next(&mut service, start).await;
        let layout = Layout::single();
        assert_eq!(within(a.recv()).await, Some(assigned(&layout, &[])));
        let table = next_table(&mut edge).await.unwrap();
        assert_eq!(table.version, FIRST + 2);
        assert!(table.routes.is_empty());

        // Once the coordinator has been there for a lease, the region goes to the
        // worker that registered first, which is the same one, above the epoch seen.
        service.tick(start + LEASE);
        let again = assignment(0, seen + 1, 1);
        assert_eq!(within(a.recv()).await, Some(assigned(&layout, &[again])));
        let table = next_table(&mut edge).await.unwrap();
        assert_eq!(table.routes, [route(again, "a")]);
        // `b` was told nothing beyond its first answer.
        drop(service);
        assert_eq!(within(b.recv()).await, None);
    }

    #[tokio::test]
    async fn a_worker_whose_client_vouches_for_nothing_loses_its_region_to_one_that_waits() {
        let served = Served::start(&[]).await;
        let mut watch = served.watch().await;
        let (mut a, _) = served.worker("a", &[]).await;
        let held = next_region(&mut a).await;
        let (mut b, orders) = served.worker("b", &[]).await;
        assert_eq!(orders, served.orders(&[]));
        let table = table_where(&mut watch, |table| !table.routes.is_empty()).await;
        assert_eq!(table.routes, [route(held, "a")]);

        // The worker is there, but cannot vouch for its region.
        a.vouch(Vec::new());
        let taken = next_region(&mut b).await;
        assert!(taken.epoch > held.epoch);
        assert_eq!(within(a.next()).await.unwrap(), served.orders(&[]));
        let expected = [route(taken, "b")];
        table_where(&mut watch, |table| table.routes == expected).await;
    }

    #[tokio::test]
    async fn a_region_is_moved_to_a_spare_worker_when_its_owner_releases_it() {
        let Cluster {
            served,
            mut a,
            b: _b,
            held_a,
            held_b,
            mut watch,
            table,
        } = Cluster::start(&[0]).await;
        let (mut c, orders) = served.worker("c", &[]).await;
        assert_eq!(orders, served.orders(&[]));

        let mut mover = ask_to_move(&served, 0, None).await;
        let begun = MoveAnswer::Begun {
            from: "a".to_owned(),
            to: "c".to_owned(),
        };
        assert_eq!(within(mover.next()).await.unwrap(), begun);
        // The owner is asked, and nothing changes until it answers.
        assert_eq!(next_release(&mut a).await, (held_a.region, held_a.epoch));
        a.released(held_a.region, held_a.epoch);

        // The spare is given the region with a higher epoch than any before, the old
        // owner hears that it runs nothing, and the edge sees one new table.
        let taken = next_region(&mut c).await;
        assert_eq!(taken.region, held_a.region);
        assert!(taken.epoch > held_b.epoch);
        assert_eq!(
            within(a.event()).await.unwrap(),
            WorkerEvent::Orders(served.orders(&[]))
        );
        let after = within(watch.next()).await.unwrap();
        assert_eq!(after.version, table.version + 1);
        assert_eq!(after.routes, [route(taken, "c"), route(held_b, "b")]);

        // Whoever asked hears whom the region went to, and is then hung up on.
        let done = MoveAnswer::Done {
            to: "c".to_owned(),
            epoch: taken.epoch,
            released: true,
        };
        assert_eq!(within(mover.next()).await.unwrap(), done);
        let lost = within(mover.next()).await;
        assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");

        // The worker that released is a spare now: the region can go back to it.
        let mut mover = ask_to_move(&served, 0, Some("a")).await;
        let begun = MoveAnswer::Begun {
            from: "c".to_owned(),
            to: "a".to_owned(),
        };
        assert_eq!(within(mover.next()).await.unwrap(), begun);
        assert_eq!(next_release(&mut c).await, (taken.region, taken.epoch));
        c.released(taken.region, taken.epoch);
        let back = next_region(&mut a).await;
        assert!(back.epoch > taken.epoch);
        let done = MoveAnswer::Done {
            to: "a".to_owned(),
            epoch: back.epoch,
            released: true,
        };
        assert_eq!(within(mover.next()).await.unwrap(), done);
    }

    #[tokio::test]
    async fn a_move_that_cannot_be_made_is_refused_with_the_reason_and_nobody_is_disturbed() {
        let Cluster {
            served,
            mut a,
            mut b,
            mut watch,
            ..
        } = Cluster::start(&[0]).await;
        let asked = [
            (0, None, "no worker waits that region 0 could be moved to"),
            (
                0,
                Some("b"),
                "region 0 cannot be moved to b, which runs a region already",
            ),
            (
                1,
                Some("nobody"),
                "region 1 cannot be moved to nobody, which is not registered",
            ),
            (7, None, "the world has no region 7"),
        ];
        for (region, to, reason) in asked {
            let mut mover = ask_to_move(&served, region, to).await;
            let refused = MoveAnswer::Refused {
                reason: reason.to_owned(),
            };
            assert_eq!(within(mover.next()).await.unwrap(), refused);
            let lost = within(mover.next()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        }

        // Nobody was told anything.
        served.stop().await;
        assert!(is_lost_next(&mut a).await);
        assert!(is_lost_next(&mut b).await);
        let lost = within(watch.next()).await;
        assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
    }

    #[tokio::test]
    async fn a_region_its_owner_does_not_release_is_taken_once_a_lease_has_passed() {
        let Cluster {
            served,
            mut a,
            b: _b,
            held_a,
            ..
        } = Cluster::start(&[0]).await;
        let (mut c, _) = served.worker("c", &[]).await;

        let asked = Instant::now();
        let mut mover = ask_to_move(&served, 0, Some("c")).await;
        let begun = MoveAnswer::Begun {
            from: "a".to_owned(),
            to: "c".to_owned(),
        };
        assert_eq!(within(mover.next()).await.unwrap(), begun);

        // The owner is there and vouches for its region, but it is one that passes
        // over what it is asked to release. The next it hears is that it runs nothing.
        assert_eq!(within(a.next()).await.unwrap(), served.orders(&[]));
        assert!(asked.elapsed() > LEASE);
        let taken = next_region(&mut c).await;
        assert_eq!(taken.region, held_a.region);
        assert!(taken.epoch > held_a.epoch);
        // Whoever asked was not hung up on for being silent that long, and hears
        // that the owner was taken for dead.
        let done = MoveAnswer::Done {
            to: "c".to_owned(),
            epoch: taken.epoch,
            released: false,
        };
        assert_eq!(within(mover.next()).await.unwrap(), done);
    }

    #[tokio::test]
    async fn a_worker_that_leaves_hands_its_region_to_a_spare_and_is_then_hung_up_on() {
        let Cluster {
            served,
            mut a,
            mut b,
            held_a,
            held_b,
            mut watch,
            table,
        } = Cluster::start(&[0]).await;
        let (mut c, _) = served.worker("c", &[]).await;

        a.leaving();
        assert_eq!(next_release(&mut a).await, (held_a.region, held_a.epoch));
        a.released(held_a.region, held_a.epoch);
        let taken = next_region(&mut c).await;
        assert_eq!(taken.region, held_a.region);
        // It is told that it runs nothing, and then its connection is closed, which
        // is how it knows that it may exit.
        assert_eq!(
            within(a.event()).await.unwrap(),
            WorkerEvent::Orders(served.orders(&[]))
        );
        assert!(is_lost_next(&mut a).await);
        let after = within(watch.next()).await.unwrap();
        assert_eq!(after.version, table.version + 1);
        assert_eq!(after.routes, [route(taken, "c"), route(held_b, "b")]);

        // The process that replaces it registers under its name, and is the spare
        // that the next worker to leave hands over to.
        let (mut again, orders) = served.worker("a", &[]).await;
        assert_eq!(orders, served.orders(&[]));
        b.leaving();
        assert_eq!(next_release(&mut b).await, (held_b.region, held_b.epoch));
        b.released(held_b.region, held_b.epoch);
        let second = next_region(&mut again).await;
        assert_eq!(second.region, held_b.region);
        assert_eq!(
            within(b.event()).await.unwrap(),
            WorkerEvent::Orders(served.orders(&[]))
        );
        assert!(is_lost_next(&mut b).await);
        let after = within(watch.next()).await.unwrap();
        assert_eq!(after.routes, [route(taken, "c"), route(second, "a")]);
    }

    #[tokio::test]
    async fn the_region_of_a_leaving_worker_whose_connection_drops_goes_to_the_next_worker() {
        let served = Served::start(&[]).await;
        let mut watch = served.watch().await;
        assert!(within(watch.next()).await.unwrap().routes.is_empty());

        // A worker that kept running while there was no coordinator, and is told to
        // stop with nobody there to hand over to.
        let held = assignment(0, 5, 0);
        let mut leaves = served.connect().await;
        leaves.send(registration("a", &[held])).await.unwrap();
        let answer = assigned(&served.layout, &[held]);
        assert_eq!(within(leaves.recv()).await, Some(answer));
        leaves.send(ToCoordinator::Leaving).await.unwrap();
        drop(leaves);

        // Its region is without an owner, and goes to the worker that comes.
        let with_a = within(watch.next()).await.unwrap();
        assert_eq!(with_a.routes, [route(held, "a")]);
        let without = within(watch.next()).await.unwrap();
        assert_eq!(without.version, with_a.version + 1);
        assert!(without.routes.is_empty());
        let (mut b, _) = served.worker("b", &[]).await;
        let taken = next_region(&mut b).await;
        assert_eq!(taken.region, held.region);
        assert!(taken.epoch > held.epoch);
        let with_b = within(watch.next()).await.unwrap();
        assert_eq!(with_b.routes, [route(taken, "b")]);
    }

    /// With made-up times, to show that no lease is waited for.
    #[tokio::test]
    async fn a_leaving_worker_whose_connection_ends_loses_its_region_at_that_moment() {
        const FIRST: u64 = 1000;
        let start = Instant::now();
        let held = assignment(0, 5, 0);
        let (mut service, mut edge, mut a, mut b) = two_workers(start, FIRST, held).await;
        // The coordinator is no longer new, and both workers are heard from.
        let now = start + LEASE;
        for worker in [&a, &b] {
            worker.send(heartbeat(&[])).await.unwrap();
            hear_next(&mut service, now).await;
        }

        // There is a worker to hand over to, so the one that leaves is asked to.
        a.send(ToCoordinator::Leaving).await.unwrap();
        hear_next(&mut service, now).await;
        let release = FromCoordinator::Release {
            region: held.region,
            epoch: held.epoch,
        };
        assert_eq!(within(a.recv()).await, Some(release));

        // It dies instead. The other worker has the region before any time passes.
        drop(a);
        hear_next(&mut service, now).await;
        let layout = Layout::single();
        let taken = assignment(0, FIRST + 1, 1);
        assert_eq!(within(b.recv()).await, Some(assigned(&layout, &[taken])));
        let table = next_table(&mut edge).await.unwrap();
        assert_eq!(table.version, FIRST + 2);
        assert_eq!(table.routes, [route(taken, "b")]);
        assert_eq!(service.workers.len(), 1);

        // A worker that is not leaving keeps its region when its connection ends.
        drop(b);
        hear_next(&mut service, now).await;
        service.tick(now);
        assert!(service.workers.is_empty());
        assert_eq!(service.coordinator.assignments("b"), [taken]);
    }

    #[tokio::test]
    async fn a_worker_that_leaves_and_owns_nothing_is_hung_up_on_at_once() {
        const FIRST: u64 = 1000;
        let start = Instant::now();
        let held = assignment(0, 5, 0);
        let (mut service, _edge, _a, mut b) = two_workers(start, FIRST, held).await;
        b.send(ToCoordinator::Leaving).await.unwrap();
        hear_next(&mut service, start).await;
        assert_eq!(within(b.recv()).await, None);
        assert_eq!(service.workers.len(), 1);
        // It is forgotten, not just cut off: it has no lease left to run out.
        assert!(!service.coordinator.heartbeat(start, "b", &[]));
    }

    /// With made-up times, to show when exactly a release runs out.
    #[tokio::test]
    async fn whoever_asked_for_a_move_is_answered_at_the_tick_a_silent_client_is_cut_off_at() {
        const FIRST: u64 = 1000;
        let start = Instant::now();
        let held = assignment(0, 5, 0);
        let (mut service, mut edge, mut a, mut b) = two_workers(start, FIRST, held).await;

        let (mut mover, end) = link::in_process(8);
        service.attach(end, start);
        let asked = start + LEASE / 2;
        let ask = ToCoordinator::Move {
            region: held.region,
            to: None,
        };
        mover.send(ask).await.unwrap();
        hear_next(&mut service, asked).await;
        let begun = FromCoordinator::MoveBegun {
            from: "a".to_owned(),
            to: "b".to_owned(),
        };
        assert_eq!(within(mover.recv()).await, Some(begun));
        let release = FromCoordinator::Release {
            region: held.region,
            epoch: held.epoch,
        };
        assert_eq!(within(a.recv()).await, Some(release));

        // Both workers are heard from, and the owner does not answer. A lease after
        // it was asked nothing has happened; a moment later the region is taken.
        for worker in [&a, &b] {
            worker.send(heartbeat(&[])).await.unwrap();
            hear_next(&mut service, asked + LEASE).await;
        }
        service.tick(asked + LEASE);
        assert_eq!(service.connections.len(), 4);
        service.tick(asked + LEASE + Duration::from_millis(1));

        let layout = Layout::single();
        let taken = assignment(0, FIRST + 1, 1);
        assert_eq!(within(a.recv()).await, Some(assigned(&layout, &[])));
        assert_eq!(within(b.recv()).await, Some(assigned(&layout, &[taken])));
        let done = FromCoordinator::MoveDone {
            to: "b".to_owned(),
            epoch: taken.epoch,
            released: false,
        };
        assert_eq!(within(mover.recv()).await, Some(done));
        assert_eq!(within(mover.recv()).await, None);
        let table = next_table(&mut edge).await.unwrap();
        assert_eq!(table.routes, [route(taken, "b")]);
        // The workers are still there.
        assert_eq!(service.workers.len(), 2);
    }

    #[tokio::test]
    async fn whoever_asked_is_told_that_the_move_is_not_done_when_nobody_can_take_the_region() {
        const FIRST: u64 = 1000;
        let start = Instant::now();
        let held = assignment(0, 5, 0);
        let (mut service, mut edge, a, b) = two_workers(start, FIRST, held).await;
        let (mut mover, end) = link::in_process(8);
        service.attach(end, start);
        let ask = ToCoordinator::Move {
            region: held.region,
            to: Some("b".to_owned()),
        };
        mover.send(ask).await.unwrap();
        hear_next(&mut service, start).await;
        assert!(matches!(
            within(mover.recv()).await,
            Some(FromCoordinator::MoveBegun { .. })
        ));

        // The worker the region was meant for goes away, and then the owner lets go.
        drop(b);
        hear_next(&mut service, start).await;
        let released = ToCoordinator::Released {
            region: held.region,
            epoch: held.epoch,
        };
        a.send(released).await.unwrap();
        hear_next(&mut service, start).await;

        let reason = "region 0 was released by its owner, but no worker waits to be given \
                      it; it is without an owner until one does";
        let refused = FromCoordinator::MoveRefused {
            reason: reason.to_owned(),
        };
        assert_eq!(within(mover.recv()).await, Some(refused));
        assert_eq!(within(mover.recv()).await, None);
        let table = next_table(&mut edge).await.unwrap();
        assert!(table.routes.is_empty());

        // The other way a region can end up with nobody.
        let outcome = MoveOutcome {
            mover: 3,
            region: RegionId(2),
            owner: None,
            released: false,
        };
        let reason = "region 2 was taken from its owner, which did not release it, but no \
                      worker waits to be given it; it is without an owner until one does";
        let refused = FromCoordinator::MoveRefused {
            reason: reason.to_owned(),
        };
        assert_eq!(outcome_message(&outcome), refused);
    }

    #[tokio::test]
    async fn a_client_that_asked_for_a_move_has_nothing_more_to_say_and_only_workers_release() {
        // The two workers stay for the whole test.
        let Cluster {
            served,
            a: _a,
            b: _b,
            held_a,
            ..
        } = Cluster::start(&[0]).await;
        let released = ToCoordinator::Released {
            region: held_a.region,
            epoch: held_a.epoch,
        };
        // Neither somebody who has not said what it is nor an edge may say that a
        // region is released, or that it leaves.
        for first in [None, Some(ToCoordinator::WatchRouting)] {
            for said in [released.clone(), ToCoordinator::Leaving] {
                let mut client = served.connect().await;
                if let Some(first) = first.clone() {
                    client.send(first).await.unwrap();
                    assert!(within(client.recv()).await.is_some());
                }
                client.send(said).await.unwrap();
                assert_eq!(within(client.recv()).await, None);
            }
        }
        // A worker cannot ask for a move over its connection.
        let mut worker = served.connect().await;
        worker.send(registration("c", &[])).await.unwrap();
        assert!(within(worker.recv()).await.is_some());
        let ask = ToCoordinator::Move {
            region: RegionId(0),
            to: None,
        };
        worker.send(ask.clone()).await.unwrap();
        assert_eq!(within(worker.recv()).await, None);

        // Whoever asks twice is cut off; the move goes on, and ends when the owner,
        // which does not answer, has had a lease to do so.
        let (mut c, _) = served.worker("c", &[]).await;
        let mut mover = served.connect().await;
        mover.send(ask.clone()).await.unwrap();
        assert!(matches!(
            within(mover.recv()).await,
            Some(FromCoordinator::MoveBegun { .. })
        ));
        mover.send(ask).await.unwrap();
        assert_eq!(within(mover.recv()).await, None);
        let taken = next_region(&mut c).await;
        assert_eq!(taken.region, held_a.region);
    }

    #[test]
    fn the_log_names_what_a_worker_reports_and_whom_each_region_is_with() {
        assert_eq!(Holdings(&[]).to_string(), "nothing");
        let holding = [assignment(1, 30, 2), assignment(0, 12, 0)];
        assert_eq!(
            Holdings(&holding).to_string(),
            "region 1 at epoch 30 with entity ids 2097152..3145728, \
             region 0 at epoch 12 with entity ids 1..1048576"
        );
        assert_eq!(Fingerprint(None).to_string(), "none");
        assert_eq!(
            Fingerprint(Some(Layout::single().fingerprint())).to_string(),
            "0x4d25767f9dce13f5"
        );

        let table = RoutingTable {
            version: 7,
            layout: Layout::new(vec![-8, 8]).unwrap(),
            spawn: SPAWN,
            routes: vec![route(holding[1], "a"), route(assignment(2, 31, 1), "c")],
        };
        assert_eq!(
            Routes(&table).to_string(),
            "region 0 at a:25601 with epoch 12, region 1 without an owner, \
             region 2 at c:25601 with epoch 31"
        );
    }

    #[test]
    fn the_coordinator_looks_at_its_leases_four_times_per_lease_but_not_all_the_time() {
        assert_eq!(CoordinatorConfig::DEFAULT_LEASE, Duration::from_secs(5));
        assert_eq!(
            tick_interval(CoordinatorConfig::DEFAULT_LEASE),
            Duration::from_millis(1250)
        );
        assert_eq!(tick_interval(LEASE), Duration::from_millis(150));
        assert_eq!(
            tick_interval(Duration::from_millis(100)),
            Duration::from_millis(50)
        );
        assert_eq!(tick_interval(Duration::ZERO), Duration::from_millis(50));
    }
}
