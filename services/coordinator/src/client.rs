//! What workers and edges reach the coordinator with.
//!
//! Each of them has one connection to it. A worker registers over its connection and is
//! told what to run, an edge asks for the routing table and is sent every new one. The
//! service at the other end is [`crate::serve`].

use std::io;
use std::time::Duration;

use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_rpc::link::End;
use clustine_rpc::{Assignment, FromCoordinator, ToCoordinator, Vouch, tcp};
use clustine_world::Vec3;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};

use crate::QUEUE;

/// How often a worker tells the coordinator that it is still there.
///
/// A third of the lease would do, but nothing tells a worker the lease of its
/// coordinator. So this is fixed, and well below
/// [`crate::CoordinatorConfig::DEFAULT_LEASE`]: a worker only loses its regions once
/// several heartbeats in a row have not arrived.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// A client's end of its connection to the coordinator.
type CoordinatorEnd = End<ToCoordinator, FromCoordinator>;

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

/// A worker's connection to the coordinator.
///
/// For as long as it exists, the coordinator is told every [`HEARTBEAT_INTERVAL`] that
/// the worker is there, whether or not anybody is waiting in [`WorkerClient::next`],
/// and what it vouches for; see [`WorkerClient::vouch`]. Dropping it ends the
/// connection. The worker then keeps its regions until its lease runs out, and for good
/// if it registers again before that.
#[derive(Debug)]
pub struct WorkerClient {
    /// The orders that came after the first and, last of all, why no more will come.
    /// From the task that holds the connection.
    orders: mpsc::UnboundedReceiver<Result<Orders, ClientError>>,
    /// What the worker last said it vouches for, which every heartbeat says; `None`
    /// until it says anything.
    vouches: watch::Sender<Option<Vec<(RegionId, Vouch)>>>,
    /// What the worker has to tell the coordinator besides heartbeats, for the task to
    /// send.
    reports: mpsc::UnboundedSender<ToCoordinator>,
}

impl WorkerClient {
    /// Connects to the coordinator at `coordinator` (host:port) and registers the worker
    /// `name`, which edges reach at `address`, reporting what it runs already: `holding`,
    /// and the fingerprint of the layout those regions belong to.
    ///
    /// Returns with the coordinator's first answer. What is not among those orders, the
    /// worker has to stop running.
    pub async fn register(
        coordinator: &str,
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
        coordinator: &str,
        name: &str,
        address: &str,
        holding: &[Assignment],
        layout: Option<u64>,
        heartbeat: Duration,
    ) -> Result<(Self, Orders), ClientError> {
        let mut link = connect(coordinator).await?;
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
        // same: the coordinator only speaks when the worker's orders change.
        let (sender, orders) = mpsc::unbounded_channel();
        let (vouches, vouched) = watch::channel(None);
        let (reports, reported) = mpsc::unbounded_channel();
        let task = Task {
            link,
            heartbeat,
            regions,
            vouched,
            reported,
            orders: sender,
        };
        tokio::spawn(task.keep_registered());
        let client = Self {
            orders,
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
    /// sent, and [`WorkerClient::next`] says that the connection is lost.
    pub fn epoch_refused(&self, region: RegionId, seen: u64) {
        let _ = self
            .reports
            .send(ToCoordinator::EpochRefused { region, seen });
    }

    /// Waits until the coordinator changes what the worker is to run, and returns the new
    /// orders. Orders that came while nobody waited are returned first, in the order
    /// they came. An error means the connection is lost.
    ///
    /// Nothing is lost if the returned future is dropped before it is done.
    pub async fn next(&mut self) -> Result<Orders, ClientError> {
        // Once the task has said why it ended, all there is left to say is that the
        // connection is gone.
        self.orders.recv().await.unwrap_or(Err(ClientError::Lost))
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
    /// Where the orders go.
    orders: mpsc::UnboundedSender<Result<Orders, ClientError>>,
}

impl Task {
    /// Tells the coordinator every `heartbeat` that the worker is there, passes on what
    /// else the worker reports and the orders that come, until the connection is lost
    /// or the worker's client is dropped.
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
                message = self.link.recv() => match orders_from(message) {
                    Ok(new) => {
                        self.regions = new.assignments.iter().map(|held| held.region).collect();
                        if self.orders.send(Ok(new)).is_err() {
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
                () = self.orders.closed() => return,
            }
        };
        let _ = self.orders.send(Err(ended));
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
    /// Connects to the coordinator at `coordinator` (host:port) and asks for the routing
    /// table.
    pub async fn connect(coordinator: &str) -> Result<Self, ClientError> {
        let link = connect(coordinator).await?;
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
            None => Err(ClientError::Lost),
        }
    }
}

/// A link to the coordinator at `coordinator`.
async fn connect(coordinator: &str) -> Result<CoordinatorEnd, ClientError> {
    let stream = TcpStream::connect(coordinator).await?;
    Ok(tcp::link(stream, QUEUE))
}

/// What the coordinator said to a worker, as the orders it is, or why there are none.
fn orders_from(message: Option<FromCoordinator>) -> Result<Orders, ClientError> {
    match message {
        Some(FromCoordinator::Assigned {
            layout,
            spawn,
            assignments,
        }) => Ok(Orders {
            layout,
            spawn,
            assignments,
        }),
        Some(FromCoordinator::Refused { reason }) => Err(ClientError::Refused(reason)),
        Some(FromCoordinator::Routing(_)) => Err(unexpected("a routing table to a worker")),
        // No coordinator asks a worker to release anything yet; see
        // docs/adr/0009-moving-a-region.md.
        Some(FromCoordinator::Release { .. }) => Err(unexpected("a release")),
        Some(
            FromCoordinator::MoveRefused { .. }
            | FromCoordinator::MoveBegun { .. }
            | FromCoordinator::MoveDone { .. },
        ) => Err(unexpected("word of a move nobody asked for")),
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
