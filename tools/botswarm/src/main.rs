//! Load-test harness: drives many scripted bot clients against a server.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use clustine_botswarm::{Bot, Crossing, Oracle, cross};
use clustine_protocol::chunk::unpack_heightmap;

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
    /// Join the game and walk east in a straight line.
    Walk {
        /// Server address as host:port.
        #[arg(default_value = "127.0.0.1:25565")]
        address: String,
        /// Player name of the bot.
        #[arg(long, default_value = "Bot")]
        name: String,
        /// How far to walk, in blocks.
        #[arg(long, default_value_t = 200.0)]
        distance: f64,
        /// Blocks per tick; a walking player covers about 0.22.
        #[arg(long, default_value_t = 0.22)]
        speed: f64,
    },
    /// Several bots walk back and forth between two places, building at each, while
    /// another watches from the spawn point. Fails unless they all stay connected and
    /// the watcher sees each of them as one entity throughout. With a region boundary
    /// between the two places this tests the handing over of players.
    Cross {
        /// Server address as host:port.
        #[arg(default_value = "127.0.0.1:25565")]
        address: String,
        /// How many bots walk.
        #[arg(long, default_value_t = Crossing::default().walkers)]
        walkers: usize,
        /// How many times each walks there and back.
        #[arg(long, default_value_t = Crossing::default().rounds)]
        rounds: u32,
        /// The x coordinates to walk between; both must be in view of the spawn point.
        #[arg(long, default_value_t = Crossing::default().west, allow_negative_numbers = true)]
        west: f64,
        #[arg(long, default_value_t = Crossing::default().east, allow_negative_numbers = true)]
        east: f64,
        /// The x coordinate of the first block east of a region boundary between the
        /// two places. With it the bots also build across that boundary.
        #[arg(long, allow_negative_numbers = true)]
        line: Option<i32>,
        /// Blocks per tick of the slowest bot.
        #[arg(long, default_value_t = Crossing::default().speed)]
        speed: f64,
        /// What the bots' names begin with, to tell runs apart.
        #[arg(long, default_value = "")]
        name_prefix: String,
    },
    /// Join the game and describe the chunk the bot is placed in.
    Chunks {
        /// Server address as host:port.
        #[arg(default_value = "127.0.0.1:25565")]
        address: String,
        /// Player name of the bot.
        #[arg(long, default_value = "Bot")]
        name: String,
        /// How many chunks to wait for.
        #[arg(long, default_value_t = 1)]
        count: usize,
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
        Scenario::Walk {
            address,
            name,
            distance,
            speed,
        } => {
            let mut bot = Bot::join(&target(address), &name).await?;
            let (x, _, z) = bot.location;
            println!(
                "starting at x = {x}, z = {z} with {} chunks",
                bot.chunks.len()
            );
            bot.walk_to(x + distance, z, speed).await?;
            // Let the last chunks around the destination arrive.
            bot.idle(Duration::from_secs(2)).await?;
            println!(
                "arrived at x = {:.1}, z = {:.1}; view centred on chunk {:?}, {} chunks held",
                bot.location.0,
                bot.location.2,
                bot.center,
                bot.chunks.len()
            );
            println!(
                "{} teleports confirmed, {} keep-alives answered",
                bot.stats.teleports_confirmed, bot.stats.keep_alives_answered
            );
        }
        Scenario::Cross {
            address,
            walkers,
            rounds,
            west,
            east,
            line,
            speed,
            name_prefix,
        } => {
            let crossing = Crossing {
                walkers,
                rounds,
                west,
                east,
                line,
                speed,
                name_prefix,
            };
            let report = cross(&target(address), &crossing).await?;
            println!(
                "{} crossings by {walkers} bots, {} blocks built, {} moves seen by the watcher",
                report.crossings, report.blocks_built, report.moves_seen
            );
        }
        Scenario::Chunks {
            address,
            name,
            count,
        } => {
            let mut bot = Bot::join(&target(address), &name).await?;
            bot.wait_for_chunks(count, Duration::from_secs(30)).await?;
            println!("{} chunks received", bot.chunks.len());
            let position = bot.position.as_ref().context("no position")?;
            let key = (
                (position.x.floor() as i32) >> 4,
                (position.z.floor() as i32) >> 4,
            );
            describe_chunk(&bot, key)?;
        }
    }
    Ok(())
}

/// Prints what the server sent for the chunk at `key`.
fn describe_chunk(bot: &Bot, key: (i32, i32)) -> Result<()> {
    let chunk = bot
        .chunks
        .get(&key)
        .context("the bot's own chunk is missing")?;
    let sections = bot
        .sections(key)?
        .context("the bot's own chunk is missing")?;
    println!("chunk {key:?}: {} bytes of sections", chunk.sections.len());
    for heightmap in &chunk.heightmaps {
        let heights = unpack_heightmap(&heightmap.data, 9).context("short heightmap")?;
        println!(
            "  heightmap {}: {} words, heights {}..={}",
            heightmap.kind,
            heightmap.data.len(),
            heights.iter().min().unwrap(),
            heights.iter().max().unwrap()
        );
    }
    for (index, section) in sections.iter().enumerate() {
        if section.block_count != 0 || index == 0 {
            let blocks: BTreeSet<i32> = (0..4096).map(|i| section.blocks.get(i)).collect();
            let biomes: BTreeSet<i32> = (0..64).map(|i| section.biomes.get(i)).collect();
            println!(
                "  section {index}: {} blocks, {} fluids, states {blocks:?}, biomes {biomes:?}",
                section.block_count, section.fluid_count
            );
        }
    }
    let light = &chunk.light;
    println!(
        "  sky light: mask {:#b}, empty mask {:#b}, {} arrays",
        light.sky_mask.first().copied().unwrap_or(0),
        light.empty_sky_mask.first().copied().unwrap_or(0),
        light.sky.len()
    );
    for array in &light.sky {
        let levels: BTreeSet<u8> = array
            .iter()
            .flat_map(|byte| [byte & 15, byte >> 4])
            .collect();
        println!("    {} bytes, levels {levels:?}", array.len());
    }
    println!(
        "  block light: mask {:#b}, empty mask {:#b}, {} arrays",
        light.block_mask.first().copied().unwrap_or(0),
        light.empty_block_mask.first().copied().unwrap_or(0),
        light.block.len()
    );
    println!("  {} block entities", chunk.block_entities.len());
    Ok(())
}
