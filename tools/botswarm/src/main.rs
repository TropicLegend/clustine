//! Load-test harness: drives many scripted bot clients against a server.

use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use clustine_botswarm::{Bot, Oracle};

/// Scripted Minecraft clients for testing a server.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Run the scenario against the official Minecraft server instead of the given
    /// address. The server is started for the run; it needs `cargo datagen` to have
    /// downloaded the server jar.
    #[arg(long, global = true)]
    vanilla: bool,

    /// State that you agree to the Minecraft EULA (https://aka.ms/MinecraftEULA), which
    /// the official server requires. Only used with --vanilla.
    #[arg(long, global = true)]
    accept_eula: bool,

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
    /// Join the game and stay connected without doing anything.
    Idle {
        /// Server address as host:port.
        #[arg(default_value = "127.0.0.1:25565")]
        address: String,
        /// Player name of the bot.
        #[arg(long, default_value = "Bot")]
        name: String,
        /// How long to stay, in seconds.
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let oracle = if args.vanilla {
        Some(Oracle::start(args.accept_eula).await?)
    } else {
        None
    };
    // The official server's address replaces the one from the command line.
    let target = |address: String| match &oracle {
        Some(oracle) => oracle.address().to_owned(),
        None => address,
    };

    match args.scenario {
        Scenario::Ping { address } => {
            let status = clustine_botswarm::ping(&target(address)).await?;
            println!("{:#}", status.json);
            println!("latency: {:?}", status.latency);
        }
        Scenario::Idle {
            address,
            name,
            seconds,
        } => {
            let mut bot = Bot::join(&target(address), &name).await?;
            println!("{}", bot.info.summary());
            bot.idle(Duration::from_secs(seconds)).await?;
            println!(
                "stayed {seconds} s: {} keep-alives answered, {} teleports confirmed",
                bot.stats.keep_alives_answered, bot.stats.teleports_confirmed
            );
            for (name, count) in &bot.stats.received {
                println!("{count:>6} {name}");
            }
        }
    }
    Ok(())
}
