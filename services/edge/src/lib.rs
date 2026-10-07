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
use std::time::Duration;

use clustine_region::Layout;
use clustine_rpc::link::EdgeEnd;
use clustine_world::Vec3;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::fanout::{Command, Fanout, FanoutConfig};

/// Commands from connections that may wait for the fan-out task.
const COMMAND_CAPACITY: usize = 1024;

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
}

/// How the world is divided into regions and how the edge reaches each of them.
#[derive(Debug)]
pub struct Routing {
    pub layout: Layout,
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The edge's ends of its links to the regions of the layout, from west to east.
    pub links: Vec<EdgeEnd>,
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
    ///
    /// # Panics
    ///
    /// If `routing` does not have exactly one link per region of its layout.
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
        };
        Ok(Self {
            listener: TcpListener::bind(address).await?,
            shared: Arc::new(Shared {
                config,
                opening_packets: configuration::opening_packets(),
                registry_packets: configuration::registry_packets(),
                fanout: commands,
                next_session: AtomicU64::new(0),
                online,
            }),
            fanout: Fanout::new(fanout_config, routing, command_receiver),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts and serves connections until a region is gone or the returned future is
    /// dropped. Either way every open connection is closed.
    pub async fn run(self) {
        // Everything runs in this set, so dropping the future stops all of it.
        let mut connections = JoinSet::new();
        let (fanout_running, mut fanout_stopped) = oneshot::channel::<()>();
        let fanout = self.fanout;
        connections.spawn(async move {
            fanout.run().await;
            drop(fanout_running);
        });
        loop {
            // Reap finished connections so the set does not grow without bound.
            while connections.try_join_next().is_some() {}

            let accepted = tokio::select! {
                accepted = self.listener.accept() => accepted,
                // Without the fan-out task nobody can play.
                _ = &mut fanout_stopped => return,
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
