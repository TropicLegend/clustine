//! Load-test harness: drives many scripted bot clients against a server.

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Scripted Minecraft clients for testing a server.
#[derive(Parser)]
#[command(version)]
struct Args {
    #[command(subcommand)]
    scenario: Scenario,
}

#[derive(Subcommand)]
enum Scenario {
    /// Query the server list entry and measure the latency.
    Ping {
        /// Server address as host:port.
        #[arg(default_value = "127.0.0.1:25565")]
        address: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Args::parse().scenario {
        Scenario::Ping { address } => {
            let status = clustine_botswarm::ping(&address).await?;
            println!("{:#}", status.json);
            println!("latency: {:?}", status.latency);
        }
    }
    Ok(())
}
