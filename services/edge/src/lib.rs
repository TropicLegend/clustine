//! Edge service: terminates client connections, routes inbound packets, fans region deltas out to players.
//!
//! The edge owns everything that is specific to the Minecraft protocol; see
//! `docs/adr/0005-edge-worker-interface.md`. It also is what makes a world of several
//! regions look like one to a client: it gathers what a player sees from the regions the
//! chunks in view belong to, and moves a player from region to region as they walk.

mod configuration;
mod connection;
mod encode;
mod fanout;
mod login;
mod play;
mod session;
mod status;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clustine_region::RegionId;
use clustine_rpc::link::EdgeEnd;
use clustine_world::{EdgeId, Vec3};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::fanout::{Command, Fanout, FanoutConfig};

/// Commands from connections that may wait for the fan-out task.
const COMMAND_CAPACITY: usize = 1024;

/// New links that may wait for the fan-out task. There is at most one per region under
/// way at a time.
const RELINK_CAPACITY: usize = 16;

/// Settings of an edge.
#[derive(Debug, Clone)]
pub struct EdgeConfig {
    /// The text shown below the server's name in the client's server list.
    pub description: String,
    /// The player limit shown in the server list.
    pub max_players: u32,
    /// How often a client has to prove it is still there. A client that has not answered
    /// one keep-alive by the time the next is due is disconnected.
    pub keep_alive_interval: Duration,
    /// The largest view distance granted to a client, in chunks. A client is sent the
    /// chunks within that distance of the one it is in.
    pub view_distance: i32,
    /// How long a client may take: to get from connecting to the play state, to send
    /// what is waited for before that, and to take what is sent to it.
    pub client_timeout: Duration,
    /// Packets of at least this many bytes are compressed. `None` turns compression off.
    pub compression_threshold: Option<usize>,
    /// How long a player is kept while the region they are in does not confirm what
    /// they do: because nobody runs it, it cannot be reached, or it does not get
    /// anything made durable. A player kept waiting longer is disconnected. It has to
    /// stay below the time after which a region forgets an edge it has not heard from.
    pub region_patience: Duration,
}

impl EdgeConfig {
    /// The keep-alive interval of the vanilla server. Clients give up on a server they
    /// have not heard from for 30 seconds, so the interval must stay well below that.
    pub const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

    pub const DEFAULT_VIEW_DISTANCE: i32 = 8;

    /// The time the vanilla server allows a client that is logging in or silent.
    pub const DEFAULT_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

    /// The compression threshold of the vanilla server.
    pub const DEFAULT_COMPRESSION_THRESHOLD: usize = 256;

    /// Long enough for another worker to take a region over, which takes the
    /// coordinator's lease and a moment, and well below the 30 seconds after which a
    /// region forgets an edge.
    pub const DEFAULT_REGION_PATIENCE: Duration = Duration::from_secs(20);
}

/// Who an edge is to the regions it talks to; see
/// `docs/adr/0008-durable-regions-and-resuming.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeIdentity {
    /// Follows from the edge's name, which stays the same across its restarts.
    pub edge: EdgeId,
    /// Which start of the edge this is: the milliseconds since the Unix epoch when it
    /// started, so that each start has a higher number than the one before.
    pub start: u64,
}

impl EdgeIdentity {
    /// The edge called `name`, starting now.
    pub fn starting_now(name: &str) -> Self {
        // A clock before 1970 is as good as one at it: a later start still has a higher
        // number once the clock has been put right.
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            edge: EdgeId::from_name(name),
            start: u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// What an edge starts with: where players enter the world and how the edge reaches the
/// regions. It is not told how the world is divided: what a region says tells it who
/// holds what (`docs/adr/0013-the-edge-without-a-layout.md`).
#[derive(Debug)]
pub struct Routing {
    /// The region players enter the world in.
    pub home: RegionId,
    /// Where players enter the world.
    pub spawn: Vec3,
    /// Who this edge is to the regions.
    pub identity: EdgeIdentity,
    /// The links the edge starts with. A region without one is waited for: what its
    /// players do is kept until a link to it comes through `relinks`.
    pub links: Vec<RegionLink>,
    /// New links to regions, which replace the ones the edge has: to a worker that has
    /// taken a region over, or to the same worker after a connection was lost; and
    /// what the routing table says of regions that were absorbed. One queue, so that
    /// the edge takes them in the order they were handed over.
    pub relinks: mpsc::Receiver<Relink>,
    /// Where the edge says that its link to a region has ended, with the epoch of the
    /// owner the link went to.
    pub lost: mpsc::UnboundedSender<(RegionId, u64)>,
}

impl Routing {
    /// The routing of an edge that starts with `links`, and the handle through which it
    /// is given new ones while it runs.
    pub fn new(
        home: RegionId,
        spawn: Vec3,
        identity: EdgeIdentity,
        links: Vec<RegionLink>,
    ) -> (Self, Relinks) {
        let (sender, relinks) = mpsc::channel(RELINK_CAPACITY);
        // Without a limit, as nobody has to listen. It stays short: a link ends once.
        let (lost, ended) = mpsc::unbounded_channel();
        let routing = Self {
            home,
            spawn,
            identity,
            links,
            relinks,
            lost,
        };
        (routing, Relinks { sender, ended })
    }
}

/// What a running edge is handed about the regions while it runs.
#[derive(Debug)]
pub enum Relink {
    /// A link that takes the place of the one the edge has to that region, if it has
    /// one.
    Link(RegionLink),
    /// The regions the routing table says were absorbed, each with the region it went
    /// into.
    Absorbed(Vec<(RegionId, RegionId)>),
}

/// Gives a running edge new links to regions, and hears from it which links have ended.
#[derive(Debug)]
pub struct Relinks {
    sender: mpsc::Sender<Relink>,
    ended: mpsc::UnboundedReceiver<(RegionId, u64)>,
}

impl Relinks {
    /// Hands the edge a link that takes the place of the one it has to that region, if
    /// it has one. Returns false if the edge is gone.
    pub async fn replace(&self, link: RegionLink) -> bool {
        self.sender.send(Relink::Link(link)).await.is_ok()
    }

    /// Tells the edge which regions the routing table says were absorbed, each with
    /// the region it went into: all of them, every time. The edge acts on a merge when
    /// the region that survived tells it; from this it knows that such a word is owed
    /// (`docs/adr/0015-the-edge-through-merges-and-splits.md`, section 5). Returns
    /// false if the edge is gone.
    pub async fn absorbed(&self, pairs: Vec<(RegionId, RegionId)>) -> bool {
        self.sender.send(Relink::Absorbed(pairs)).await.is_ok()
    }

    /// Waits until a link of the edge has ended, and returns the region it went to and
    /// the epoch of the owner at its other end. `None` once the edge is gone.
    ///
    /// Nothing is lost if the returned future is dropped before it is done.
    pub async fn ended(&mut self) -> Option<(RegionId, u64)> {
        self.ended.recv().await
    }
}

/// The edge's end of a link to a region, and the epoch of the region's owner at the
/// other end. A link to an owner with a lower epoch than the one the edge is linked to
/// is not taken: that owner has been replaced.
#[derive(Debug)]
pub struct RegionLink {
    pub region: RegionId,
    pub epoch: u64,
    pub end: EdgeEnd,
}

/// Why an edge stopped serving players by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// A region knows a later start of an edge with this name: another process has
    /// taken this one's place, and this one must not come back under the same name.
    Superseded,
    /// Whoever ran the edge dropped what it needs to go on.
    Abandoned,
}

/// State shared by all connections of one edge.
struct Shared {
    config: EdgeConfig,
    /// See [`configuration::opening_packets`].
    opening_packets: Vec<Vec<u8>>,
    /// See [`configuration::registry_packets`].
    registry_packets: Vec<Vec<u8>>,
    /// Where connections in the play state register, report, and pass on what their
    /// players do.
    fanout: mpsc::Sender<Command>,
    next_session: AtomicU64,
    /// The number of players in the world, kept by the fan-out task.
    online: Arc<AtomicU32>,
}

/// A listening edge that has not started accepting connections yet.
pub struct Edge {
    listener: TcpListener,
    shared: Arc<Shared>,
    fanout: Fanout,
}

impl Edge {
    /// Starts listening on `address`. Port 0 picks a free port; see [`Edge::local_addr`].
    pub async fn bind(
        address: SocketAddr,
        config: EdgeConfig,
        routing: Routing,
    ) -> io::Result<Self> {
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let online = Arc::new(AtomicU32::new(0));
        let fanout_config = FanoutConfig {
            max_players: config.max_players,
            view_distance: config.view_distance,
            online: Arc::clone(&online),
            region_patience: config.region_patience,
        };
        Ok(Self {
            listener: TcpListener::bind(address).await?,
            shared: Arc::new(Shared {
                config,
                opening_packets: configuration::opening_packets(),
                registry_packets: configuration::registry_packets(),
                fanout: commands,
                // From 1: a session is the attempt its join names to the region
                // (`docs/adr/0020-one-stay-per-player.md`, section 4, step 2).
                next_session: AtomicU64::new(1),
                online,
            }),
            fanout: Fanout::new(fanout_config, routing, command_receiver),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts and serves connections until the edge has to stop, which it says why, or
    /// the returned future is dropped. Either way every open connection is closed. A
    /// region that goes away does not stop the edge: its players wait for it.
    pub async fn run(self) -> Stopped {
        // Everything runs in this set, so dropping the future stops all of it.
        let mut connections = JoinSet::new();
        let (fanout_running, mut fanout_stopped) = oneshot::channel();
        let fanout = self.fanout;
        connections.spawn(async move {
            let _ = fanout_running.send(fanout.run().await);
        });
        loop {
            // Reap finished connections so the set does not grow without bound.
            while connections.try_join_next().is_some() {}

            let accepted = tokio::select! {
                accepted = self.listener.accept() => accepted,
                // Without the fan-out task nobody can play.
                stopped = &mut fanout_stopped => return stopped.unwrap_or(Stopped::Abandoned),
            };
            let (stream, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Typically the process is out of file descriptors; keep serving the
                    // connections that exist and retry.
                    warn!(%error, "accepting a connection failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let shared = Arc::clone(&self.shared);
            connections.spawn(async move {
                // Shown by default: a client that cannot get in is otherwise invisible
                // from the server's side.
                if let Err(error) = session::serve(stream, &shared).await {
                    info!(%peer, %error, "connection ended with an error");
                }
            });
        }
    }
}
