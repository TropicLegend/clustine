//! What workers, edges and operators reach the coordinator with.
//!
//! Each of them has one connection to it. A worker registers over its connection and is
//! told what to run, an edge asks for the routing table and is sent every new one, and
//! whoever wants a region moved, two merged or one split asks for that and is told how
//! it went. The service at the other end is [`crate::serve`].

use std::io;
use std::time::Duration;

use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_rpc::link::{self, End};
use clustine_rpc::{Assignment, FromCoordinator, Off, PlayersOf, ToCoordinator, Vouch, tcp};
use clustine_world::{ChunkPos, Vec3};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};

use crate::QUEUE;
use crate::service::ClientEnd;

/// How often a worker tells the coordinator that it is still there.
///
/// A third of the lease would do, but nothing tells a worker the lease of its
/// coordinator. So this is fixed, and well below
/// [`crate::CoordinatorConfig::DEFAULT_LEASE`]: a worker only loses its regions once
/// several heartbeats in a row have not arrived.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// A client's end of its connection to the coordinator.
type CoordinatorEnd = End<ToCoordinator, FromCoordinator>;

/// Where a coordinator is: at an address (host:port), or in this process.
///
/// Every client takes anything that says so. An address as a `&str` or a `String` is
/// a coordinator to connect to over TCP.
#[derive(Debug, Clone)]
pub enum Reach {
    Tcp(String),
    Local(LocalCoordinator),
}

impl From<&str> for Reach {
    fn from(address: &str) -> Self {
        Self::Tcp(address.to_owned())
    }
}

impl From<String> for Reach {
    fn from(address: String) -> Self {
        Self::Tcp(address)
    }
}

impl From<&String> for Reach {
    fn from(address: &String) -> Self {
        Self::Tcp(address.clone())
    }
}

impl From<LocalCoordinator> for Reach {
    fn from(local: LocalCoordinator) -> Self {
        Self::Local(local)
    }
}

impl From<&Reach> for Reach {
    fn from(reach: &Reach) -> Self {
        reach.clone()
    }
}

/// The way to a coordinator in this process, which [`crate::serve_local`] makes with
/// it. Clones lead to the same coordinator.
#[derive(Debug, Clone)]
pub struct LocalCoordinator {
    /// The service's ends of new connections, to the service.
    connections: mpsc::UnboundedSender<ClientEnd>,
}

impl LocalCoordinator {
    /// A way to a coordinator, and where the service takes the connections made
    /// through it.
    pub(crate) fn new() -> (Self, mpsc::UnboundedReceiver<ClientEnd>) {
        let (connections, taken) = mpsc::unbounded_channel();
        (Self { connections }, taken)
    }

    /// A connection to the coordinator. It is lost from the start if nobody serves
    /// the coordinator any more, as a connection to an address is that nobody
    /// listens on.
    fn connect(&self) -> Result<CoordinatorEnd, ClientError> {
        let (client, service) = link::in_process(QUEUE);
        self.connections
            .send(service)
            .map_err(|_| ClientError::Lost)?;
        Ok(client)
    }
}

/// Why a client has nothing more to tell.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The coordinator could not be reached, or what it sent makes no sense.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The coordinator does not let the worker take part, for the reason given.
    #[error("the coordinator refused: {0}")]
    Refused(String),
    /// The connection has ended. The coordinator may well be there still, or again.
    #[error("the connection to the coordinator is lost")]
    Lost,
}

/// What a worker is told: how the world is divided and what it is to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Orders {
    pub layout: Layout,
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The regions the worker is to run, in ascending order. It has to stop running
    /// whatever is not among them.
    pub assignments: Vec<Assignment>,
}

/// What the coordinator tells a worker after its first orders.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerEvent {
    /// What the worker is to run from now on.
    Orders(Orders),
    /// The worker is to let go of `region`, which it holds with `epoch`, so that
    /// another worker can carry on with it, and to say so with
    /// [`WorkerClient::released`]. If it does not hold the region with that epoch, it
    /// says so at once all the same. See `docs/adr/0009-moving-a-region.md`.
    Release { region: RegionId, epoch: u64 },
    /// A merge or a split of `region`, which the worker holds with `epoch`, is coming:
    /// it is about to absorb a region that is being released for it, or to be split.
    /// The worker is to checkpoint it now, so that the merge or the split finds less
    /// to wait for. Not answered. This and the two below are of
    /// `docs/adr/0014-merging-and-splitting.md`, section 4; when it comes before a
    /// split is in `docs/adr/0016-when-to-merge-and-split.md`, section 5.6.
    Prepare { region: RegionId, epoch: u64 },
    /// The worker is to have `region`, which it holds with `epoch`, absorb the region
    /// `absorbed`, which nobody runs and which it opens with `as_epoch` for that, and
    /// to say what came of it with [`WorkerClient::absorb_ended`]. The order may come
    /// twice.
    Absorb {
        region: RegionId,
        epoch: u64,
        absorbed: RegionId,
        as_epoch: u64,
    },
    /// The worker is to split the players standing in `chunks` off `region`, which it
    /// holds with `epoch`, as the region `part`, to run that with `as_epoch`, and to
    /// say what came of it with [`WorkerClient::split_ended`]. The order comes once.
    SplitOff {
        region: RegionId,
        epoch: u64,
        chunks: Vec<ChunkPos>,
        as_epoch: u64,
        part: RegionId,
    },
}

/// A worker's connection to the coordinator.
///
/// For as long as it exists, the coordinator is told every [`HEARTBEAT_INTERVAL`] that
/// the worker is there, whether or not anybody is waiting in [`WorkerClient::event`],
/// and what it vouches for; see [`WorkerClient::vouch`]. Dropping it ends the
/// connection. The worker then keeps its regions until its lease runs out, and for good
/// if it registers again before that, unless it had said that it is leaving: then the
/// coordinator takes it to be gone at once.
#[derive(Debug)]
pub struct WorkerClient {
    /// What the coordinator said after the first orders and, last of all, why no more
    /// will come. From the task that holds the connection.
    events: mpsc::UnboundedReceiver<Result<WorkerEvent, ClientError>>,
    /// What the worker last said it vouches for, which every heartbeat says; `None`
    /// until it says anything.
    vouches: watch::Sender<Option<Vec<(RegionId, Vouch)>>>,
    /// What the worker has to tell the coordinator besides heartbeats, for the task to
    /// send.
    reports: mpsc::UnboundedSender<ToCoordinator>,
}

impl WorkerClient {
    /// Connects to the coordinator at `coordinator` (an address as host:port, or a [`Reach`]) and registers the worker
    /// `name`, which edges reach at `address`, reporting what it runs already: `holding`,
    /// and the fingerprint of the layout those regions belong to.
    ///
    /// Returns with the coordinator's first answer. What is not among those orders, the
    /// worker has to stop running.
    pub async fn register(
        coordinator: impl Into<Reach>,
        name: &str,
        address: &str,
        holding: &[Assignment],
        layout: Option<u64>,
    ) -> Result<(Self, Orders), ClientError> {
        Self::register_with_heartbeat(
            coordinator,
            name,
            address,
            holding,
            layout,
            HEARTBEAT_INTERVAL,
        )
        .await
    }

    /// [`WorkerClient::register`] for a worker that says every `heartbeat` that it is
    /// there. For tests, whose coordinators have leases too short for
    /// [`HEARTBEAT_INTERVAL`].
    #[doc(hidden)]
    pub async fn register_with_heartbeat(
        coordinator: impl Into<Reach>,
        name: &str,
        address: &str,
        holding: &[Assignment],
        layout: Option<u64>,
        heartbeat: Duration,
    ) -> Result<(Self, Orders), ClientError> {
        let mut link = connect(&coordinator.into()).await?;
        let registration = ToCoordinator::RegisterWorker {
            name: name.to_owned(),
            address: address.to_owned(),
            holding: holding.to_vec(),
            layout,
        };
        link.send(registration)
            .await
            .map_err(|_| ClientError::Lost)?;
        let first = orders_from(link.recv().await)?;
        let regions = first.assignments.iter().map(|held| held.region).collect();

        // The task must never wait for the worker, or the heartbeats would stop while
        // the worker is busy; hence a queue without a limit. It stays short all the
        // same: the coordinator only speaks when the worker's orders change, or it is
        // to release, merge or split a region.
        let (sender, events) = mpsc::unbounded_channel();
        let (vouches, vouched) = watch::channel(None);
        let (reports, reported) = mpsc::unbounded_channel();
        let task = Task {
            link,
            heartbeat,
            regions,
            vouched,
            reported,
            events: sender,
        };
        tokio::spawn(task.keep_registered());
        let client = Self {
            events,
            vouches,
            reports,
        };
        Ok((client, first))
    }

    /// Says what the worker vouches for in each heartbeat from now on: the regions it
    /// names, each with why. A region the worker was told to run and does not name is
    /// not vouched for, and loses its owner once it has gone a lease without being
    /// vouched for (see [`crate::Coordinator`]). What is said last counts, until it is
    /// said again; it is not sent before the next heartbeat.
    ///
    /// Until this is first called, the worker vouches [`Vouch::Committed`] for every
    /// region of its latest orders. A client made by registering again starts that way
    /// too, so a worker that registers again says once more what it vouches for.
    pub fn vouch(&self, regions: Vec<(RegionId, Vouch)>) {
        // The task holds the other end for as long as the connection lasts, and after
        // that there is nobody to tell.
        self.vouches.send_replace(Some(regions));
    }

    /// Tells the coordinator that the world store refused to let the worker open
    /// `region`, because it has seen an owner with the epoch `seen`. The worker has
    /// dropped the region; the coordinator issues epochs above `seen` from now on.
    /// Sent at once, ahead of the next heartbeat. If the connection is lost, nothing is
    /// sent, and [`WorkerClient::event`] says that the connection is lost.
    pub fn epoch_refused(&self, region: RegionId, seen: u64) {
        let _ = self
            .reports
            .send(ToCoordinator::EpochRefused { region, seen });
    }

    /// Tells the coordinator that the worker has let go of `region`, which it held with
    /// `epoch`: in answer to [`WorkerEvent::Release`], or by itself. The region is
    /// closed at the world store, and the worker never takes up that assignment again.
    /// The coordinator gives the region to another worker at once, and the worker's
    /// next orders are without it.
    ///
    /// Sent at once, ahead of the next heartbeat. If the connection is lost, nothing is
    /// sent, or what was sent may not have arrived, and [`WorkerClient::event`] says
    /// that the connection is lost. Nothing has to be remembered for that case. The
    /// worker registers again and reports what it holds, which is without the region:
    /// if the coordinator had asked for the release, it takes that as the answer. If it
    /// had not, the orders that answer the registration still contain the assignment,
    /// and the worker, which has released it, says so again with this.
    pub fn released(&self, region: RegionId, epoch: u64) {
        let _ = self.reports.send(ToCoordinator::Released { region, epoch });
    }

    /// Tells the coordinator that the worker has been told to stop. From now on the
    /// coordinator gives it nothing new and asks it to release what it runs, each
    /// region as soon as another worker is there to take it. Once the worker owns
    /// nothing, the coordinator closes the connection, and [`WorkerClient::event`] says
    /// that it is lost: that is how the worker knows that it may exit.
    ///
    /// Sent at once, ahead of the next heartbeat. If the connection is lost, nothing is
    /// sent, and [`WorkerClient::event`] says that the connection is lost. Leaving
    /// belongs to one registration: a worker that registers again and is still to stop
    /// says so again on the new client. If the connection is lost after this was said,
    /// the coordinator takes the worker to be gone and gives its regions away, so a
    /// worker that goes on running one by registering again reports it as held.
    pub fn leaving(&self) {
        let _ = self.reports.send(ToCoordinator::Leaving);
    }

    /// Tells the coordinator what came of [`WorkerEvent::Absorb`]: that `region` has
    /// absorbed `absorbed`, or why not. A worker also says this, with `Ok`, of a region
    /// that the world store refuses to let it open because `region` has absorbed it.
    /// Either only makes the coordinator read the world store's list of regions, which
    /// decides what happened.
    ///
    /// Sent at once, ahead of the next heartbeat. If the connection is lost, nothing is
    /// sent, or what was sent may not have arrived, and [`WorkerClient::event`] says
    /// that the connection is lost. Nothing has to be remembered for that case: the
    /// coordinator reads the list when the merge has had a lease, and if the worker
    /// registers again before that, it is given the order again.
    pub fn absorb_ended(&self, region: RegionId, absorbed: RegionId, outcome: Result<(), Off>) {
        let _ = self.reports.send(ToCoordinator::AbsorbEnded {
            region,
            absorbed,
            outcome,
        });
    }

    /// Tells the coordinator what came of the [`WorkerEvent::SplitOff`] of `region`
    /// that named `as_epoch`: the new region, which the worker runs with that epoch,
    /// or why there is none.
    ///
    /// Sent at once, ahead of the next heartbeat. If the connection is lost, nothing is
    /// sent, or what was sent may not have arrived, and [`WorkerClient::event`] says
    /// that the connection is lost. A worker that has split a region reports the new
    /// one as held when it registers again, and says this again on the new client
    /// until its orders have named the new region once: the order to split is not
    /// given twice, so nothing else tells the coordinator whose the region is.
    pub fn split_ended(&self, region: RegionId, as_epoch: u64, outcome: Result<RegionId, Off>) {
        let _ = self.reports.send(ToCoordinator::SplitEnded {
            region,
            as_epoch,
            outcome,
        });
    }

    /// Tells the coordinator where the players of the worker's regions are: of every
    /// region it runs, also of those without players, each time the whole of what it
    /// knows. See `docs/adr/0016-when-to-merge-and-split.md`, section 2.
    ///
    /// Sent at once, ahead of the next heartbeat, and behind whatever was said before
    /// it with [`WorkerClient::absorb_ended`], [`WorkerClient::split_ended`] or any
    /// other of these calls. The coordinator goes by that order: what it is told here
    /// behind the word of a merge or a split, it takes to be of the regions as they
    /// are after it. If the connection is lost, nothing is sent, and
    /// [`WorkerClient::event`] says that the connection is lost. Nothing has to be
    /// remembered for that case: the next call says it all again.
    pub fn players(&self, regions: Vec<PlayersOf>) {
        let _ = self.reports.send(ToCoordinator::Players { regions });
    }

    /// Waits for the next thing the coordinator says: new orders, that a region is to
    /// be released, or what is to be done for a merge or a split. What came while
    /// nobody waited is returned first, in the order it came. An error means the
    /// connection is lost.
    ///
    /// Nothing is lost if the returned future is dropped before it is done.
    pub async fn event(&mut self) -> Result<WorkerEvent, ClientError> {
        // Once the task has said why it ended, all there is left to say is that the
        // connection is gone.
        self.events.recv().await.unwrap_or(Err(ClientError::Lost))
    }

    /// Waits until the coordinator changes what the worker is to run, and returns the new
    /// orders. Orders that came while nobody waited are returned first, in the order
    /// they came. An error means the connection is lost.
    ///
    /// This is [`WorkerClient::event`] for a worker that releases nothing and neither
    /// merges nor splits: what the coordinator asks of it is passed over. A region it
    /// was to release is taken from it when the lease is out, and a merge or a split
    /// is given up then.
    ///
    /// Nothing is lost if the returned future is dropped before it is done, but for
    /// what was passed over.
    pub async fn next(&mut self) -> Result<Orders, ClientError> {
        loop {
            if let WorkerEvent::Orders(orders) = self.event().await? {
                return Ok(orders);
            }
        }
    }
}

/// What holds a worker's connection, in a task of its own.
struct Task {
    link: CoordinatorEnd,
    /// How often the worker says that it is there.
    heartbeat: Duration,
    /// The regions of the latest orders.
    regions: Vec<RegionId>,
    /// What the worker vouches for, if it has said.
    vouched: watch::Receiver<Option<Vec<(RegionId, Vouch)>>>,
    /// What the worker has to tell besides heartbeats.
    reported: mpsc::UnboundedReceiver<ToCoordinator>,
    /// Where what the coordinator says goes.
    events: mpsc::UnboundedSender<Result<WorkerEvent, ClientError>>,
}

impl Task {
    /// Tells the coordinator every `heartbeat` that the worker is there, passes on what
    /// else the worker reports and what the coordinator says, until the connection is
    /// lost or the worker's client is dropped.
    ///
    /// Until the worker says what it vouches for, it vouches for every region it has
    /// been told to run, which is what a worker that is heard from did before regions
    /// were vouched for one by one.
    async fn keep_registered(mut self) {
        // An interval cannot be zero.
        let heartbeat = self.heartbeat.max(Duration::from_millis(1));
        // To register was to be heard from, so the first heartbeat is due an interval
        // later.
        let mut beats = tokio::time::interval_at(Instant::now() + heartbeat, heartbeat);
        // After a stall one heartbeat says all that the missed ones would have said.
        beats.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let ended = loop {
            tokio::select! {
                message = self.link.recv() => match event_from(message) {
                    Ok(event) => {
                        if let WorkerEvent::Orders(new) = &event {
                            self.regions = new.assignments.iter().map(|held| held.region).collect();
                        }
                        if self.events.send(Ok(event)).is_err() {
                            return;
                        }
                    }
                    Err(error) => break error,
                },
                Some(report) = self.reported.recv() => {
                    if self.link.try_send(report).is_err() {
                        break ClientError::Lost;
                    }
                }
                _ = beats.tick() => {
                    // A queue full of heartbeats means that none has been written for
                    // hundreds of intervals. The lease is long over then.
                    let beat = ToCoordinator::Heartbeat {
                        regions: self.vouches(),
                    };
                    if self.link.try_send(beat).is_err() {
                        break ClientError::Lost;
                    }
                }
                // The worker's client was dropped.
                () = self.events.closed() => return,
            }
        };
        let _ = self.events.send(Err(ended));
    }

    /// What the next heartbeat vouches for.
    fn vouches(&self) -> Vec<(RegionId, Vouch)> {
        match &*self.vouched.borrow() {
            Some(said) => said.clone(),
            None => {
                let regions = self.regions.iter();
                regions.map(|region| (*region, Vouch::Committed)).collect()
            }
        }
    }
}

/// An edge's connection to the coordinator.
#[derive(Debug)]
pub struct RoutingWatch {
    link: CoordinatorEnd,
}

impl RoutingWatch {
    /// Connects to the coordinator at `coordinator` (an address as host:port, or a [`Reach`]) and asks for the routing
    /// table.
    pub async fn connect(coordinator: impl Into<Reach>) -> Result<Self, ClientError> {
        let link = connect(&coordinator.into()).await?;
        link.send(ToCoordinator::WatchRouting)
            .await
            .map_err(|_| ClientError::Lost)?;
        Ok(Self { link })
    }

    /// The current routing table on the first call, afterwards each new one as it comes.
    /// An error means the connection is lost.
    ///
    /// Nothing is lost if the returned future is dropped before it is done.
    pub async fn next(&mut self) -> Result<RoutingTable, ClientError> {
        match self.link.recv().await {
            Some(FromCoordinator::Routing(table)) => Ok(table),
            Some(FromCoordinator::Refused { reason }) => Err(ClientError::Refused(reason)),
            Some(FromCoordinator::Assigned { .. } | FromCoordinator::Release { .. }) => {
                Err(unexpected("orders to an edge"))
            }
            Some(
                FromCoordinator::MoveRefused { .. }
                | FromCoordinator::MoveBegun { .. }
                | FromCoordinator::MoveDone { .. },
            ) => Err(unexpected("word of a move nobody asked for")),
            Some(
                FromCoordinator::Absorb { .. }
                | FromCoordinator::SplitOff { .. }
                | FromCoordinator::Prepare { .. }
                | FromCoordinator::Asked(_),
            ) => Err(unexpected("word of a merge or a split")),
            None => Err(ClientError::Lost),
        }
    }
}

/// What the coordinator answers whoever asked for a region to be moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveAnswer {
    /// The move is not done, for the reason given. Nothing follows. This is the answer
    /// to a move that never began; it can also follow [`MoveAnswer::Begun`], if the
    /// region left its owner and no worker was there to be given it.
    Refused { reason: String },
    /// The worker `from` has been asked to release the region for the worker `to`.
    /// [`MoveAnswer::Done`] or [`MoveAnswer::Refused`] follows, within the
    /// coordinator's lease or little more.
    Begun { from: String, to: String },
    /// The region is the worker `to`'s now, with `epoch`; that need not be the worker
    /// the move began for. `released` says whether the old owner let go of it, or did
    /// not in time and was taken for dead. Nothing follows.
    Done {
        to: String,
        epoch: u64,
        released: bool,
    },
}

/// The connection of whoever asked the coordinator to merge two regions or to split
/// one. See `docs/adr/0014-merging-and-splitting.md`, section 5.1.
#[derive(Debug)]
pub struct Asker {
    link: CoordinatorEnd,
}

impl Asker {
    /// Connects to the coordinator at `coordinator` (an address as host:port, or a [`Reach`]) and asks it to have
    /// the region `survivor` absorb the region `absorbed`.
    pub async fn merge(
        coordinator: impl Into<Reach>,
        survivor: RegionId,
        absorbed: RegionId,
    ) -> Result<Self, ClientError> {
        Self::ask(coordinator, ToCoordinator::Merge { survivor, absorbed }).await
    }

    /// Connects to the coordinator at `coordinator` (an address as host:port, or a [`Reach`]) and asks it to have
    /// the players standing in `chunks` split off `region` as a region of its own.
    pub async fn split(
        coordinator: impl Into<Reach>,
        region: RegionId,
        chunks: &[ChunkPos],
    ) -> Result<Self, ClientError> {
        let chunks = chunks.to_vec();
        Self::ask(coordinator, ToCoordinator::Split { region, chunks }).await
    }

    async fn ask(
        coordinator: impl Into<Reach>,
        request: ToCoordinator,
    ) -> Result<Self, ClientError> {
        let link = connect(&coordinator.into()).await?;
        link.send(request).await.map_err(|_| ClientError::Lost)?;
        Ok(Self { link })
    }

    /// The coordinator's one answer: the region that absorbed the other, or the one
    /// that was split off; or, in words, why there is none. It comes at once if the
    /// coordinator refuses, and otherwise when the coordinator knows what came of it,
    /// which is within its lease or little more; the coordinator does not cut the
    /// connection off for being silent until then, and closes it after the answer.
    ///
    /// [`ClientError::Lost`] means that the connection ended without an answer. What
    /// was asked for may go on all the same, and the world store's list of regions
    /// shows what became of it.
    pub async fn answer(mut self) -> Result<Result<RegionId, String>, ClientError> {
        match self.link.recv().await {
            Some(FromCoordinator::Asked(answer)) => Ok(answer),
            Some(FromCoordinator::Refused { reason }) => Err(ClientError::Refused(reason)),
            Some(
                FromCoordinator::Assigned { .. }
                | FromCoordinator::Release { .. }
                | FromCoordinator::Absorb { .. }
                | FromCoordinator::SplitOff { .. }
                | FromCoordinator::Prepare { .. },
            ) => Err(unexpected(
                "orders to somebody who asked for a merge or a split",
            )),
            Some(FromCoordinator::Routing(_)) => Err(unexpected(
                "a routing table to somebody who asked for a merge or a split",
            )),
            Some(
                FromCoordinator::MoveRefused { .. }
                | FromCoordinator::MoveBegun { .. }
                | FromCoordinator::MoveDone { .. },
            ) => Err(unexpected("word of a move nobody asked for")),
            None => Err(ClientError::Lost),
        }
    }
}

/// The connection of whoever asked the coordinator to move a region.
#[derive(Debug)]
pub struct Mover {
    link: CoordinatorEnd,
}

impl Mover {
    /// Connects to the coordinator at `coordinator` (an address as host:port, or a [`Reach`]) and asks it to move
    /// `region` to the worker named `to`, or to the worker that runs the fewest.
    pub async fn ask(
        coordinator: impl Into<Reach>,
        region: RegionId,
        to: Option<&str>,
    ) -> Result<Self, ClientError> {
        let link = connect(&coordinator.into()).await?;
        let to = to.map(str::to_owned);
        link.send(ToCoordinator::Move { region, to })
            .await
            .map_err(|_| ClientError::Lost)?;
        Ok(Self { link })
    }

    /// The coordinator's next answer: first [`MoveAnswer::Refused`] or
    /// [`MoveAnswer::Begun`], and after the latter how the move ended. The coordinator
    /// closes the connection after its last answer, so asking for another gives
    /// [`ClientError::Lost`], as does a connection that is lost before: the move may
    /// then go on all the same, and the routing table shows what became of it.
    ///
    /// Nothing is lost if the returned future is dropped before it is done.
    pub async fn next(&mut self) -> Result<MoveAnswer, ClientError> {
        match self.link.recv().await {
            Some(FromCoordinator::MoveRefused { reason }) => Ok(MoveAnswer::Refused { reason }),
            Some(FromCoordinator::MoveBegun { from, to }) => Ok(MoveAnswer::Begun { from, to }),
            Some(FromCoordinator::MoveDone {
                to,
                epoch,
                released,
            }) => Ok(MoveAnswer::Done {
                to,
                epoch,
                released,
            }),
            Some(FromCoordinator::Refused { reason }) => Err(ClientError::Refused(reason)),
            Some(FromCoordinator::Assigned { .. } | FromCoordinator::Release { .. }) => {
                Err(unexpected("orders to somebody who asked for a move"))
            }
            Some(FromCoordinator::Routing(_)) => Err(unexpected(
                "a routing table to somebody who asked for a move",
            )),
            Some(
                FromCoordinator::Absorb { .. }
                | FromCoordinator::SplitOff { .. }
                | FromCoordinator::Prepare { .. }
                | FromCoordinator::Asked(_),
            ) => Err(unexpected("word of a merge or a split")),
            None => Err(ClientError::Lost),
        }
    }
}

/// A link to the coordinator at `coordinator`.
async fn connect(coordinator: &Reach) -> Result<CoordinatorEnd, ClientError> {
    match coordinator {
        Reach::Tcp(address) => {
            let stream = TcpStream::connect(address).await?;
            Ok(tcp::link(stream, QUEUE))
        }
        Reach::Local(local) => local.connect(),
    }
}

/// What the coordinator answered a registration, as the orders it is, or why there are
/// none.
fn orders_from(message: Option<FromCoordinator>) -> Result<Orders, ClientError> {
    match event_from(message)? {
        WorkerEvent::Orders(orders) => Ok(orders),
        // A worker is asked to release what it was told to run, so orders come first.
        WorkerEvent::Release { .. } => Err(unexpected("a release before any orders")),
        // The same holds of a region it is to merge or to split.
        WorkerEvent::Prepare { .. } | WorkerEvent::Absorb { .. } | WorkerEvent::SplitOff { .. } => {
            Err(unexpected("word of a merge or a split before any orders"))
        }
    }
}

/// What the coordinator said to a worker, or why it says no more.
fn event_from(message: Option<FromCoordinator>) -> Result<WorkerEvent, ClientError> {
    match message {
        Some(FromCoordinator::Assigned {
            layout,
            spawn,
            assignments,
        }) => Ok(WorkerEvent::Orders(Orders {
            layout,
            spawn,
            assignments,
        })),
        Some(FromCoordinator::Release { region, epoch }) => {
            Ok(WorkerEvent::Release { region, epoch })
        }
        Some(FromCoordinator::Refused { reason }) => Err(ClientError::Refused(reason)),
        Some(FromCoordinator::Routing(_)) => Err(unexpected("a routing table to a worker")),
        Some(
            FromCoordinator::MoveRefused { .. }
            | FromCoordinator::MoveBegun { .. }
            | FromCoordinator::MoveDone { .. },
        ) => Err(unexpected("word of a move nobody asked for")),
        Some(FromCoordinator::Prepare { region, epoch }) => {
            Ok(WorkerEvent::Prepare { region, epoch })
        }
        Some(FromCoordinator::Absorb {
            region,
            epoch,
            absorbed,
            as_epoch,
        }) => Ok(WorkerEvent::Absorb {
            region,
            epoch,
            absorbed,
            as_epoch,
        }),
        Some(FromCoordinator::SplitOff {
            region,
            epoch,
            chunks,
            as_epoch,
            part,
        }) => Ok(WorkerEvent::SplitOff {
            region,
            epoch,
            chunks,
            as_epoch,
            part,
        }),
        Some(FromCoordinator::Asked(_)) => Err(unexpected(
            "the answer to a merge or a split nobody asked for",
        )),
        None => Err(ClientError::Lost),
    }
}

/// The coordinator sent `what`, which is not for this kind of client.
fn unexpected(what: &str) -> ClientError {
    let error = format!("the coordinator sent {what}");
    ClientError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use clustine_region::{RegionId, RegionRoute};
    use clustine_world::EntityIds;
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::*;

    /// How often the workers of these tests say that they are there.
    const HEARTBEAT: Duration = Duration::from_millis(20);

    /// How long a test waits for something that should happen at once.
    const PATIENCE: Duration = Duration::from_secs(20);

    /// The coordinator's end of the connection to a client.
    type ClientEnd = End<FromCoordinator, ToCoordinator>;

    /// Something for clients to connect to, and its address.
    async fn listen() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        (listener, address)
    }

    /// The link to the next client that connects.
    async fn accept(listener: &TcpListener) -> ClientEnd {
        let (stream, _) = within(listener.accept()).await.unwrap();
        tcp::link(stream, 8)
    }

    /// Waits for `future`, but not for ever.
    async fn within<T>(future: impl Future<Output = T>) -> T {
        timeout(PATIENCE, future)
            .await
            .expect("this should not take so long")
    }

    fn assignment(region: u32, epoch: u64) -> Assignment {
        Assignment {
            region: RegionId(region),
            epoch,
            entity_ids: EntityIds::block(region).unwrap(),
        }
    }

    fn orders(assignments: &[Assignment]) -> Orders {
        Orders {
            layout: Layout::new(vec![0]).unwrap(),
            spawn: Vec3::new(0.5, -60.0, 0.5),
            assignments: assignments.to_vec(),
        }
    }

    fn assigned(assignments: &[Assignment]) -> FromCoordinator {
        let Orders {
            layout,
            spawn,
            assignments,
        } = orders(assignments);
        FromCoordinator::Assigned {
            layout,
            spawn,
            assignments,
        }
    }

    fn table(version: u64) -> RoutingTable {
        RoutingTable {
            home: None,
            absorbed: Vec::new(),
            waiting: 1,
            version,
            layout: Layout::new(vec![0]).unwrap(),
            spawn: Vec3::new(0.5, -60.0, 0.5),
            routes: vec![RegionRoute {
                region: RegionId(1),
                epoch: version,
                address: "a:25601".to_owned(),
            }],
        }
    }

    /// Registers the worker `a`, which runs nothing, with the coordinator at `address`.
    async fn register(address: &str) -> Result<(WorkerClient, Orders), ClientError> {
        let registering =
            WorkerClient::register_with_heartbeat(address, "a", "a:25601", &[], None, HEARTBEAT);
        within(registering).await
    }

    fn is_invalid_data<T>(result: &Result<T, ClientError>) -> bool {
        matches!(result, Err(ClientError::Io(error)) if error.kind() == io::ErrorKind::InvalidData)
    }

    #[tokio::test]
    async fn a_worker_registers_with_what_it_holds_and_is_given_the_first_answer() {
        let (listener, address) = listen().await;
        let held = [assignment(1, 7)];
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            let registration = within(link.recv()).await;
            link.send(assigned(&held)).await.unwrap();
            (link, registration)
        });
        let registering = WorkerClient::register_with_heartbeat(
            &address,
            "worker-1",
            "10.0.0.1:25601",
            &held,
            Some(42),
            HEARTBEAT,
        );
        let (_client, first) = within(registering).await.unwrap();
        assert_eq!(first, orders(&held));
        let (_link, registration) = coordinator.await.unwrap();
        assert_eq!(
            registration,
            Some(ToCoordinator::RegisterWorker {
                name: "worker-1".to_owned(),
                address: "10.0.0.1:25601".to_owned(),
                holding: held.to_vec(),
                layout: Some(42),
            })
        );
    }

    #[tokio::test]
    async fn a_worker_is_heard_from_for_as_long_as_its_client_exists() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            link.send(assigned(&[assignment(2, 7)])).await.unwrap();
            link
        });
        let started = Instant::now();
        let (client, _) = register(&address).await.unwrap();
        let mut link = coordinator.await.unwrap();
        // The worker vouches for what it has been told to run.
        let beat = ToCoordinator::Heartbeat {
            regions: vec![(RegionId(2), Vouch::Committed)],
        };

        // Nobody waits for orders, and the heartbeats come all the same.
        for _ in 0..5 {
            assert_eq!(within(link.recv()).await, Some(beat.clone()));
        }
        // The first of them an interval after registering, then one per interval.
        assert!(started.elapsed() >= 5 * HEARTBEAT);

        // New orders, and the heartbeats follow them.
        link.send(assigned(&[])).await.unwrap();
        let quiet = ToCoordinator::Heartbeat {
            regions: Vec::new(),
        };
        while within(link.recv()).await != Some(quiet.clone()) {}

        drop(client);
        // One or two may have been on their way.
        while let Some(message) = within(link.recv()).await {
            assert_eq!(message, quiet);
        }
    }

    #[tokio::test]
    async fn heartbeats_vouch_for_what_the_worker_last_said_once_it_has_said_anything() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            link.send(assigned(&[assignment(0, 7), assignment(1, 8)]))
                .await
                .unwrap();
            link
        });
        let (client, _) = register(&address).await.unwrap();
        let mut link = coordinator.await.unwrap();
        let everything = ToCoordinator::Heartbeat {
            regions: vec![
                (RegionId(0), Vouch::Committed),
                (RegionId(1), Vouch::Committed),
            ],
        };
        assert_eq!(within(link.recv()).await, Some(everything.clone()));

        // What the worker says replaces that, and each heartbeat says the latest.
        let said = [
            vec![(RegionId(1), Vouch::WaitingForStore)],
            vec![
                (RegionId(0), Vouch::Committed),
                (RegionId(1), Vouch::WaitingForStore),
            ],
            Vec::new(),
        ];
        for regions in said {
            client.vouch(regions.clone());
            let beat = ToCoordinator::Heartbeat { regions };
            // One that was on its way may still say what was said before.
            while within(link.recv()).await != Some(beat.clone()) {}
            assert_eq!(within(link.recv()).await, Some(beat));
        }

        // Even when the orders change: the worker says what it vouches for.
        link.send(assigned(&[assignment(2, 9)])).await.unwrap();
        let quiet = ToCoordinator::Heartbeat {
            regions: Vec::new(),
        };
        let mut client = client;
        assert_eq!(
            within(client.next()).await.unwrap(),
            orders(&[assignment(2, 9)])
        );
        for _ in 0..3 {
            assert_eq!(within(link.recv()).await, Some(quiet.clone()));
        }
    }

    #[tokio::test]
    async fn a_refused_epoch_is_reported_between_the_heartbeats() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            link.send(assigned(&[assignment(3, 7)])).await.unwrap();
            link
        });
        let (client, _) = register(&address).await.unwrap();
        let mut link = coordinator.await.unwrap();
        client.epoch_refused(RegionId(3), 99);
        let refused = ToCoordinator::EpochRefused {
            region: RegionId(3),
            seen: 99,
        };
        loop {
            match within(link.recv()).await {
                Some(ToCoordinator::Heartbeat { .. }) => {}
                other => {
                    assert_eq!(other, Some(refused));
                    break;
                }
            }
        }
    }

    #[tokio::test]
    async fn a_release_comes_in_order_with_the_orders_and_next_passes_it_over() {
        let first = [assignment(0, 5)];
        let release = WorkerEvent::Release {
            region: RegionId(0),
            epoch: 5,
        };
        // Two workers are told the same; one of them does not release anything.
        for releases in [true, false] {
            let (listener, address) = listen().await;
            let coordinator = tokio::spawn(async move {
                let mut link = accept(&listener).await;
                assert!(within(link.recv()).await.is_some());
                link.send(assigned(&[])).await.unwrap();
                link.send(assigned(&first)).await.unwrap();
                let (region, epoch) = (RegionId(0), 5);
                link.send(FromCoordinator::Release { region, epoch })
                    .await
                    .unwrap();
                link.send(assigned(&[])).await.unwrap();
                link
            });
            let (mut client, _) = register(&address).await.unwrap();
            let _link = coordinator.await.unwrap();
            if releases {
                let expected = [
                    WorkerEvent::Orders(orders(&first)),
                    release.clone(),
                    WorkerEvent::Orders(orders(&[])),
                ];
                for event in expected {
                    assert_eq!(within(client.event()).await.unwrap(), event);
                }
            } else {
                assert_eq!(within(client.next()).await.unwrap(), orders(&first));
                assert_eq!(within(client.next()).await.unwrap(), orders(&[]));
            }
        }
    }

    #[tokio::test]
    async fn a_released_region_and_leaving_are_reported_at_once_and_in_order() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            link.send(assigned(&[assignment(3, 7)])).await.unwrap();
            link
        });
        let (mut client, _) = register(&address).await.unwrap();
        let mut link = coordinator.await.unwrap();
        client.released(RegionId(3), 7);
        client.leaving();
        client.released(RegionId(4), 9);
        let said = [
            ToCoordinator::Released {
                region: RegionId(3),
                epoch: 7,
            },
            ToCoordinator::Leaving,
            ToCoordinator::Released {
                region: RegionId(4),
                epoch: 9,
            },
        ];
        for expected in said {
            loop {
                match within(link.recv()).await {
                    Some(ToCoordinator::Heartbeat { .. }) => {}
                    other => {
                        assert_eq!(other, Some(expected));
                        break;
                    }
                }
            }
        }

        // The coordinator closes the connection of a worker that has left. That is
        // all the worker hears, and what it says after that goes nowhere.
        drop(link);
        for _ in 0..2 {
            let lost = within(client.event()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
            client.released(RegionId(3), 7);
            client.leaving();
        }
    }

    #[tokio::test]
    async fn a_release_in_answer_to_a_registration_is_an_error() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let link = accept(&listener).await;
            let (region, epoch) = (RegionId(0), 5);
            link.send(FromCoordinator::Release { region, epoch })
                .await
                .unwrap();
            link
        });
        let answer = register(&address).await;
        assert!(is_invalid_data(&answer), "{answer:?}");
        drop(coordinator.await.unwrap());
    }

    #[tokio::test]
    async fn what_a_worker_is_to_do_for_a_merge_or_a_split_comes_in_order_with_its_orders() {
        let first = [assignment(0, 5)];
        let chunks = vec![ChunkPos::new(3, -2), ChunkPos::new(4, -2)];
        let (listener, address) = listen().await;
        let split = chunks.clone();
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            let (region, epoch) = (RegionId(0), 5);
            let told = [
                assigned(&first),
                FromCoordinator::Prepare { region, epoch },
                FromCoordinator::Absorb {
                    region,
                    epoch,
                    absorbed: RegionId(1),
                    as_epoch: 9,
                },
                FromCoordinator::SplitOff {
                    region,
                    epoch,
                    chunks: split,
                    as_epoch: 10,
                    part: RegionId(2),
                },
                assigned(&[assignment(0, 5), assignment(2, 10)]),
            ];
            for message in told {
                link.send(message).await.unwrap();
            }
            link
        });
        let (mut client, _) = register(&address).await.unwrap();
        let mut link = coordinator.await.unwrap();
        let (region, epoch) = (RegionId(0), 5);
        let expected = [
            WorkerEvent::Prepare { region, epoch },
            WorkerEvent::Absorb {
                region,
                epoch,
                absorbed: RegionId(1),
                as_epoch: 9,
            },
            WorkerEvent::SplitOff {
                region,
                epoch,
                chunks,
                as_epoch: 10,
                part: RegionId(2),
            },
            WorkerEvent::Orders(orders(&[assignment(0, 5), assignment(2, 10)])),
        ];
        for event in expected {
            assert_eq!(within(client.event()).await.unwrap(), event);
        }

        // What came of them is reported at once and in order.
        client.absorb_ended(region, RegionId(1), Ok(()));
        client.split_ended(region, 10, Ok(RegionId(2)));
        client.absorb_ended(region, RegionId(1), Err(Off::StoreLost));
        client.split_ended(region, 11, Err(Off::Nobody));
        let said = [
            ToCoordinator::AbsorbEnded {
                region,
                absorbed: RegionId(1),
                outcome: Ok(()),
            },
            ToCoordinator::SplitEnded {
                region,
                as_epoch: 10,
                outcome: Ok(RegionId(2)),
            },
            ToCoordinator::AbsorbEnded {
                region,
                absorbed: RegionId(1),
                outcome: Err(Off::StoreLost),
            },
            ToCoordinator::SplitEnded {
                region,
                as_epoch: 11,
                outcome: Err(Off::Nobody),
            },
        ];
        for expected in said {
            loop {
                match within(link.recv()).await {
                    Some(ToCoordinator::Heartbeat { regions }) => {
                        // The new region is vouched for like any the orders name.
                        let vouched: Vec<u32> = regions.iter().map(|(id, _)| id.0).collect();
                        assert_eq!(vouched, [0, 2]);
                    }
                    other => {
                        assert_eq!(other, Some(expected));
                        break;
                    }
                }
            }
        }
    }

    /// A report that is said behind the word of a split was made after the split:
    /// the coordinator goes by that order, so the client must keep it.
    #[tokio::test]
    async fn where_the_players_are_is_said_in_order_with_what_came_of_merges_and_splits() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            link.send(assigned(&[assignment(0, 5)])).await.unwrap();
            link
        });
        let (mut client, _) = register(&address).await.unwrap();
        let mut link = coordinator.await.unwrap();

        let region = RegionId(0);
        let players_of = |region: u32, epoch: u64, tick: u64, crowds: &[(ChunkPos, u32)]| {
            let crowds = crowds.to_vec();
            PlayersOf {
                region: RegionId(region),
                epoch,
                tick,
                crowds,
            }
        };
        let (near, far) = (ChunkPos::new(3, -2), ChunkPos::new(40, -2));
        let before = vec![players_of(0, 5, 40, &[(near, 2), (far, 1)])];
        let after = vec![
            players_of(0, 5, 40, &[(near, 2)]),
            players_of(2, 10, 0, &[(far, 1)]),
        ];
        client.players(before.clone());
        client.split_ended(region, 10, Ok(RegionId(2)));
        client.players(after.clone());
        client.absorb_ended(region, RegionId(1), Err(Off::Busy));
        // A worker that runs nothing says so as well.
        client.players(Vec::new());
        let said = [
            ToCoordinator::Players { regions: before },
            ToCoordinator::SplitEnded {
                region,
                as_epoch: 10,
                outcome: Ok(RegionId(2)),
            },
            ToCoordinator::Players { regions: after },
            ToCoordinator::AbsorbEnded {
                region,
                absorbed: RegionId(1),
                outcome: Err(Off::Busy),
            },
            ToCoordinator::Players {
                regions: Vec::new(),
            },
        ];
        for expected in said {
            loop {
                match within(link.recv()).await {
                    Some(ToCoordinator::Heartbeat { .. }) => {}
                    other => {
                        assert_eq!(other, Some(expected));
                        break;
                    }
                }
            }
        }

        // Once the connection is lost, what the worker says of its players goes
        // nowhere, like everything else it says.
        drop(link);
        for _ in 0..2 {
            let lost = within(client.event()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
            client.players(Vec::new());
        }
    }

    #[tokio::test]
    async fn word_of_a_merge_in_answer_to_a_registration_and_an_answer_nobody_asked_for_are_errors()
    {
        let (region, epoch) = (RegionId(0), 5);
        // A worker is told to merge what it was told to run, so orders come first.
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let link = accept(&listener).await;
            link.send(FromCoordinator::Prepare { region, epoch })
                .await
                .unwrap();
            link
        });
        let answer = register(&address).await;
        assert!(is_invalid_data(&answer), "{answer:?}");
        drop(coordinator.await.unwrap());

        // The answer to somebody who asked for a merge is not for a worker.
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let link = accept(&listener).await;
            link.send(assigned(&[])).await.unwrap();
            link.send(FromCoordinator::Asked(Ok(region))).await.unwrap();
            link
        });
        let (mut client, _) = register(&address).await.unwrap();
        let next = within(client.event()).await;
        assert!(is_invalid_data(&next), "{next:?}");
        drop(coordinator.await.unwrap());
    }

    #[tokio::test]
    async fn whoever_asks_for_a_merge_or_a_split_is_given_the_one_answer() {
        let answers = [Ok(RegionId(4)), Err("region 1 has no owner".to_owned())];
        for answer in answers {
            // A merge.
            let (listener, address) = listen().await;
            let answered = vec![FromCoordinator::Asked(answer.clone())];
            let coordinator = tokio::spawn(answer_a_request(listener, answered));
            let asker = within(Asker::merge(&address, RegionId(0), RegionId(1)))
                .await
                .unwrap();
            let asked = ToCoordinator::Merge {
                survivor: RegionId(0),
                absorbed: RegionId(1),
            };
            assert_eq!(coordinator.await.unwrap(), Some(asked));
            assert_eq!(within(asker.answer()).await.unwrap(), answer);

            // A split.
            let (listener, address) = listen().await;
            let answered = vec![FromCoordinator::Asked(answer.clone())];
            let coordinator = tokio::spawn(answer_a_request(listener, answered));
            let chunks = [ChunkPos::new(-1, 7)];
            let asker = within(Asker::split(&address, RegionId(2), &chunks))
                .await
                .unwrap();
            let asked = ToCoordinator::Split {
                region: RegionId(2),
                chunks: chunks.to_vec(),
            };
            assert_eq!(coordinator.await.unwrap(), Some(asked));
            assert_eq!(within(asker.answer()).await.unwrap(), answer);
        }
    }

    #[tokio::test]
    async fn a_merge_that_is_answered_wrongly_or_not_at_all_is_an_error() {
        // What is meant for a worker, an edge or somebody who asked for a move.
        let (region, epoch) = (RegionId(0), 5);
        let wrong = [
            assigned(&[]),
            FromCoordinator::Prepare { region, epoch },
            FromCoordinator::Routing(table(1)),
            FromCoordinator::MoveRefused {
                reason: "not today".to_owned(),
            },
        ];
        for wrong in wrong {
            let (listener, address) = listen().await;
            let coordinator = tokio::spawn(answer_a_request(listener, vec![wrong]));
            let asker = within(Asker::merge(&address, RegionId(0), RegionId(1)))
                .await
                .unwrap();
            coordinator.await.unwrap();
            let answer = within(asker.answer()).await;
            assert!(is_invalid_data(&answer), "{answer:?}");
        }

        // A coordinator that goes away without a word, and one that is not there.
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let _ = accept(&listener).await;
            listener
        });
        let asker = within(Asker::split(&address, RegionId(0), &[]))
            .await
            .unwrap();
        let lost = within(asker.answer()).await;
        assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        drop(coordinator.await.unwrap());
        let unreachable = within(Asker::merge(&address, RegionId(0), RegionId(1))).await;
        assert!(
            matches!(unreachable, Err(ClientError::Io(_))),
            "{unreachable:?}"
        );
    }

    /// A coordinator that answers one request, for a move, a merge or a split, with
    /// `answers` and hangs up. Returns what it was asked.
    async fn answer_a_request(
        listener: TcpListener,
        answers: Vec<FromCoordinator>,
    ) -> Option<ToCoordinator> {
        let mut link = accept(&listener).await;
        let asked = within(link.recv()).await;
        for answer in answers {
            link.send(answer).await.unwrap();
        }
        asked
    }

    #[tokio::test]
    async fn whoever_asks_for_a_move_is_given_the_answers_as_they_come_and_then_the_loss() {
        let (listener, address) = listen().await;
        let answers = vec![
            FromCoordinator::MoveBegun {
                from: "a".to_owned(),
                to: "b".to_owned(),
            },
            FromCoordinator::MoveDone {
                to: "c".to_owned(),
                epoch: 9,
                released: false,
            },
        ];
        let coordinator = tokio::spawn(answer_a_request(listener, answers));
        let mut mover = within(Mover::ask(&address, RegionId(2), Some("b")))
            .await
            .unwrap();
        let asked = ToCoordinator::Move {
            region: RegionId(2),
            to: Some("b".to_owned()),
        };
        assert_eq!(coordinator.await.unwrap(), Some(asked));
        let begun = MoveAnswer::Begun {
            from: "a".to_owned(),
            to: "b".to_owned(),
        };
        assert_eq!(within(mover.next()).await.unwrap(), begun);
        let done = MoveAnswer::Done {
            to: "c".to_owned(),
            epoch: 9,
            released: false,
        };
        assert_eq!(within(mover.next()).await.unwrap(), done);
        for _ in 0..2 {
            let lost = within(mover.next()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        }
    }

    #[tokio::test]
    async fn a_refused_move_carries_its_reason_before_or_after_it_began() {
        let refused = || FromCoordinator::MoveRefused {
            reason: "not today".to_owned(),
        };
        let begun = FromCoordinator::MoveBegun {
            from: "a".to_owned(),
            to: "b".to_owned(),
        };
        for answers in [vec![refused()], vec![begun, refused()]] {
            let (listener, address) = listen().await;
            let count = answers.len();
            let coordinator = tokio::spawn(answer_a_request(listener, answers));
            let mut mover = within(Mover::ask(&address, RegionId(0), None))
                .await
                .unwrap();
            let asked = ToCoordinator::Move {
                region: RegionId(0),
                to: None,
            };
            assert_eq!(coordinator.await.unwrap(), Some(asked));
            for _ in 1..count {
                let answer = within(mover.next()).await.unwrap();
                assert!(matches!(answer, MoveAnswer::Begun { .. }), "{answer:?}");
            }
            let answer = MoveAnswer::Refused {
                reason: "not today".to_owned(),
            };
            assert_eq!(within(mover.next()).await.unwrap(), answer);
            let lost = within(mover.next()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        }
    }

    #[tokio::test]
    async fn a_move_that_is_answered_wrongly_or_not_at_all_is_an_error() {
        // What is meant for a worker or an edge.
        for wrong in [assigned(&[]), FromCoordinator::Routing(table(1))] {
            let (listener, address) = listen().await;
            let coordinator = tokio::spawn(answer_a_request(listener, vec![wrong]));
            let mut mover = within(Mover::ask(&address, RegionId(0), None))
                .await
                .unwrap();
            coordinator.await.unwrap();
            let answer = within(mover.next()).await;
            assert!(is_invalid_data(&answer), "{answer:?}");
        }

        // A coordinator that goes away without a word, and one that is not there.
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let _ = accept(&listener).await;
            listener
        });
        let mut mover = within(Mover::ask(&address, RegionId(0), None))
            .await
            .unwrap();
        let lost = within(mover.next()).await;
        assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        drop(coordinator.await.unwrap());
        let unreachable = within(Mover::ask(&address, RegionId(0), None)).await;
        assert!(
            matches!(unreachable, Err(ClientError::Io(_))),
            "{unreachable:?}"
        );
    }

    #[tokio::test]
    async fn orders_come_in_the_order_they_were_sent_and_then_the_loss_of_the_connection() {
        let (listener, address) = listen().await;
        let all = [
            orders(&[]),
            orders(&[assignment(0, 5)]),
            orders(&[assignment(0, 5), assignment(1, 6)]),
            orders(&[]),
        ];
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            link
        });
        let registering = tokio::spawn(async move { register(&address).await });
        let link = coordinator.await.unwrap();
        for orders in &all {
            link.send(assigned(&orders.assignments)).await.unwrap();
        }
        let (mut client, first) = registering.await.unwrap().unwrap();
        assert_eq!(first, all[0]);

        // The coordinator goes away with three orders on their way. They still arrive.
        drop(link);
        for orders in &all[1..] {
            assert_eq!(within(client.next()).await.unwrap(), *orders);
        }
        for _ in 0..2 {
            let lost = within(client.next()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        }
    }

    #[tokio::test]
    async fn a_refusal_carries_its_reason() {
        let (listener, address) = listen().await;
        tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert!(within(link.recv()).await.is_some());
            let reason = "not today".to_owned();
            link.send(FromCoordinator::Refused { reason })
                .await
                .unwrap();
        });
        let refused = register(&address).await;
        assert!(
            matches!(&refused, Err(ClientError::Refused(reason)) if reason == "not today"),
            "{refused:?}"
        );
        let error = refused.unwrap_err().to_string();
        assert_eq!(error, "the coordinator refused: not today");
    }

    #[tokio::test]
    async fn an_edge_asks_for_the_table_and_is_passed_every_one_and_then_the_loss() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let mut link = accept(&listener).await;
            assert_eq!(within(link.recv()).await, Some(ToCoordinator::WatchRouting));
            for version in [7, 8, 9] {
                link.send(FromCoordinator::Routing(table(version)))
                    .await
                    .unwrap();
            }
        });
        let mut watch = within(RoutingWatch::connect(&address)).await.unwrap();
        coordinator.await.unwrap();
        for version in [7, 8, 9] {
            assert_eq!(within(watch.next()).await.unwrap(), table(version));
        }
        for _ in 0..2 {
            let lost = within(watch.next()).await;
            assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        }
    }

    #[tokio::test]
    async fn what_is_meant_for_the_other_kind_of_client_is_an_error() {
        // A routing table in answer to a registration.
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let link = accept(&listener).await;
            link.send(FromCoordinator::Routing(table(1))).await.unwrap();
            link
        });
        let answer = register(&address).await;
        assert!(is_invalid_data(&answer), "{answer:?}");
        drop(coordinator.await.unwrap());

        // A routing table to a worker that has registered. Its client gives up.
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let link = accept(&listener).await;
            link.send(assigned(&[])).await.unwrap();
            link.send(FromCoordinator::Routing(table(1))).await.unwrap();
            link
        });
        let (mut client, _) = register(&address).await.unwrap();
        let mut link = coordinator.await.unwrap();
        let next = within(client.next()).await;
        assert!(is_invalid_data(&next), "{next:?}");
        let lost = within(client.next()).await;
        assert!(matches!(lost, Err(ClientError::Lost)), "{lost:?}");
        // And closes the connection, once the coordinator has read what it was sent.
        while within(link.recv()).await.is_some() {}

        // Orders to an edge.
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            let link = accept(&listener).await;
            link.send(assigned(&[])).await.unwrap();
            link
        });
        let mut watch = within(RoutingWatch::connect(&address)).await.unwrap();
        let next = within(watch.next()).await;
        assert!(is_invalid_data(&next), "{next:?}");
        drop(coordinator.await.unwrap());
    }

    #[tokio::test]
    async fn a_coordinator_that_is_not_there_or_goes_away_without_an_answer_is_reported() {
        let (listener, address) = listen().await;
        let coordinator = tokio::spawn(async move {
            // Accepted and dropped without a word.
            let _ = accept(&listener).await;
            listener
        });
        let unanswered = register(&address).await;
        assert!(
            matches!(unanswered, Err(ClientError::Lost)),
            "{unanswered:?}"
        );

        // Now nobody listens at the address any more.
        drop(coordinator.await.unwrap());
        let unreachable = register(&address).await;
        assert!(
            matches!(unreachable, Err(ClientError::Io(_))),
            "{unreachable:?}"
        );
        let unreachable = within(RoutingWatch::connect(&address)).await;
        assert!(
            matches!(unreachable, Err(ClientError::Io(_))),
            "{unreachable:?}"
        );
    }
}
