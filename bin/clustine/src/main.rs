//! Single-binary mode: runs every Clustine service in one process.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Parser;
use clustine::{Config, EdgeConfig, Server};
use tracing::info;

/// A Minecraft: Java Edition server, all services in one process.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Address to listen on. There is no authentication yet, so keep this on localhost.
    #[arg(long, default_value = "127.0.0.1:25565")]
    bind: SocketAddr,

    /// Text shown in the client's server list.
    #[arg(long, default_value = "A Clustine server")]
    description: String,

    /// Player limit shown in the server list.
    #[arg(long, default_value_t = 20)]
    max_players: u32,

    /// Directory the world is kept in; created if it does not exist.
    #[arg(long, default_value = "world")]
    world: PathBuf,

    /// Seconds between two saves of all changed chunks that are still loaded.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    checkpoint_interval: u64,

    /// Packets of at least this many bytes are compressed. A negative number turns
    /// compression off.
    #[arg(long, default_value_t = EdgeConfig::DEFAULT_COMPRESSION_THRESHOLD as i64, allow_negative_numbers = true)]
    compression_threshold: i64,

    /// Largest view distance granted to a client, in chunks.
    #[arg(long, default_value_t = EdgeConfig::DEFAULT_VIEW_DISTANCE, value_parser = clap::value_parser!(i32).range(2..=32))]
    view_distance: i32,

    /// Chunk x coordinates at which to divide the world into regions that are simulated
    /// separately, in ascending order and separated by commas. Blocks cannot be changed
    /// across such a boundary. Without this the world is one region.
    #[arg(long, value_delimiter = ',', allow_negative_numbers = true)]
    boundaries: Vec<i32>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt::init();

    let mut server = Server::start(Config {
        bind: args.bind,
        description: args.description,
        max_players: args.max_players,
        keep_alive_interval: EdgeConfig::DEFAULT_KEEP_ALIVE_INTERVAL,
        view_distance: args.view_distance,
        client_timeout: EdgeConfig::DEFAULT_CLIENT_TIMEOUT,
        compression_threshold: usize::try_from(args.compression_threshold).ok(),
        world: Some(args.world),
        checkpoint_interval: Duration::from_secs(args.checkpoint_interval),
        serialise_link: false,
        boundaries: args.boundaries,
    })
    .await?;
    info!(address = %server.address(), "listening");

    tokio::select! {
        interrupted = tokio::signal::ctrl_c() => interrupted?,
        _ = server.stopped() => {
            server.stop().await;
            bail!("the server stopped unexpectedly");
        }
    }
    info!("shutting down");
    server.stop().await;
    Ok(())
}
