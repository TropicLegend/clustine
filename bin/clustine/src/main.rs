//! Single-binary mode: runs every Clustine service in one process.

use std::net::SocketAddr;

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

    /// Largest view distance granted to a client, in chunks.
    #[arg(long, default_value_t = EdgeConfig::DEFAULT_VIEW_DISTANCE, value_parser = clap::value_parser!(i32).range(2..=32))]
    view_distance: i32,
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
        serialise_link: false,
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
