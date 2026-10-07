//! The crossing scenario: players walk back and forth between two places while another
//! player watches.
//!
//! It is meant for a world that is divided into regions with a boundary between the two
//! places, where it checks what a division must not change: nobody is disconnected or
//! moved by the server, everybody can build wherever they are, and the watcher sees each
//! walker as one entity that appears once and never vanishes.

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_data::blocks;
use clustine_protocol::packets::play::face;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use crate::Bot;

const PATIENCE: Duration = Duration::from_secs(30);

/// The height of the surface of a flat world, which is the height players stand at.
const GROUND: i32 = -60;

/// What the bots of the crossing scenario do.
#[derive(Debug, Clone)]
pub struct Crossing {
    /// How many players walk.
    pub walkers: usize,
    /// How many times each of them walks to `east` and back to `west`.
    pub rounds: u32,
    /// The x coordinates the walkers walk between. Both have to be in view of the spawn
    /// point, where the watcher stays.
    pub west: f64,
    pub east: f64,
    /// Blocks per tick of the slowest walker; each further one is a little faster.
    pub speed: f64,
    /// What the names of the bots begin with.
    pub name_prefix: String,
}

impl Default for Crossing {
    fn default() -> Self {
        Self {
            walkers: 4,
            rounds: 3,
            west: 40.5,
            east: 90.5,
            speed: 0.5,
            name_prefix: String::new(),
        }
    }
}

/// What a run of the crossing scenario observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossingReport {
    /// How many times a walker went from one place to the other.
    pub crossings: u32,
    /// How many blocks the walkers placed and broke again.
    pub blocks_built: u32,
    /// How many moves of the walkers the watcher was sent.
    pub moves_seen: u32,
}

/// What a walker reports when it is back where it started.
struct Arrival {
    entity_id: i32,
    /// The x and z coordinates the walker stands at.
    location: (f64, f64),
    /// How many blocks it placed and broke again.
    built: u32,
}

/// Runs the crossing scenario against the server at `address`. Fails if any of what the
/// module documentation lists does not hold.
pub async fn cross(address: &str, crossing: &Crossing) -> Result<CrossingReport> {
    let name = |role: &str| format!("{}{role}", crossing.name_prefix);
    let mut watcher = Bot::join(address, &name("Watcher"))
        .await
        .context("the watcher could not join")?;
    watcher.wait_for_chunks(1, PATIENCE).await?;
    // The walkers are compared with what the watcher saw before they came.
    let spawned_before = watcher.stats.entities_spawned;
    let removed_before = watcher.stats.entities_removed;
    let seen_before = watcher.entities.len();

    // The walkers say when they are back and then stay until told to leave, so that the
    // watcher can look at them.
    let (arrived, mut arrivals) = mpsc::channel(crossing.walkers.max(1));
    let (leave, _) = watch::channel(false);
    let mut walkers = JoinSet::new();
    for number in 0..crossing.walkers {
        let address = address.to_owned();
        let name = name(&format!("Walker{number}"));
        let crossing = crossing.clone();
        let arrived = arrived.clone();
        let leave = leave.subscribe();
        walkers.spawn(async move {
            walk(&address, &name, number, &crossing, arrived, leave)
                .await
                .with_context(|| format!("{name} failed"))
        });
    }
    drop(arrived);

    // The watcher has to keep reading while the others walk.
    let mut back: Vec<Arrival> = Vec::new();
    while back.len() < crossing.walkers {
        tokio::select! {
            arrival = arrivals.recv() => {
                back.push(arrival.context("the walkers are gone")?);
            }
            Some(ended) = walkers.join_next() => {
                // Nobody has been told to leave yet.
                ended??;
                bail!("a walker left before it was told to");
            }
            idled = watcher.idle(Duration::from_millis(50)) => {
                idled.context("the watcher was disconnected")?;
            }
        }
    }

    // Everyone was seen all the way to where they are now.
    let seen = |watcher: &Bot, walker: &Arrival| {
        let entity = watcher.entities.get(&walker.entity_id)?;
        ((entity.position.0, entity.position.2) == walker.location).then_some(entity.position_syncs)
    };
    watcher
        .wait_until(PATIENCE, |watcher| {
            back.iter().all(|walker| seen(watcher, walker).is_some())
        })
        .await
        .context("the watcher does not see every walker where it is")?;
    let moves_seen = back
        .iter()
        .filter_map(|walker| seen(&watcher, walker))
        .sum();
    let spawned = watcher.stats.entities_spawned - spawned_before;
    let removed = watcher.stats.entities_removed - removed_before;
    ensure!(
        spawned == crossing.walkers as u32 && removed == 0,
        "the watcher saw {spawned} entities appear and {removed} vanish for {} walkers",
        crossing.walkers
    );

    // And when they leave, they are gone.
    leave.send_replace(true);
    while let Some(ended) = walkers.join_next().await {
        ended??;
    }
    watcher
        .wait_until(PATIENCE, |watcher| watcher.entities.len() == seen_before)
        .await
        .context("walkers who left are still seen")?;

    Ok(CrossingReport {
        crossings: crossing.walkers as u32 * crossing.rounds * 2,
        blocks_built: back.iter().map(|walker| walker.built).sum(),
        moves_seen,
    })
}

/// One walker: joins, walks back and forth building at each end, says that it is back
/// and stays until told to leave.
async fn walk(
    address: &str,
    name: &str,
    number: usize,
    crossing: &Crossing,
    arrived: mpsc::Sender<Arrival>,
    mut leave: watch::Receiver<bool>,
) -> Result<()> {
    let mut bot = Bot::join(address, name).await?;
    bot.wait_for_chunks(1, PATIENCE).await?;
    let entity_id = bot.info.login.entity_id;
    // Lanes two blocks apart, so that nobody builds where someone else stands.
    let z = number as f64 * 2.0 + 0.5 - crossing.walkers as f64;
    let speed = crossing.speed * (1.0 + number as f64 * 0.17);

    let mut built = 0;
    for _ in 0..crossing.rounds {
        for x in [crossing.east, crossing.west] {
            bot.walk_to(x, z, speed).await?;
            build(&mut bot)
                .await
                .with_context(|| format!("building at x = {x}"))?;
            built += 1;
        }
    }

    // A server that is unsure where a player is puts them somewhere, which a client
    // sees as being moved. That happens once, on joining.
    ensure!(
        bot.stats.teleports_confirmed == 1,
        "the server moved the player {} times",
        bot.stats.teleports_confirmed
    );
    let own_chunk = (
        (bot.location.0.floor() as i32) >> 4,
        (bot.location.2.floor() as i32) >> 4,
    );
    bot.wait_until(PATIENCE, |bot| {
        bot.center == Some(own_chunk) && bot.chunks.contains_key(&own_chunk)
    })
    .await
    .context("the chunks around the player did not follow")?;

    let arrival = Arrival {
        entity_id,
        location: (bot.location.0, bot.location.2),
        built,
    };
    // Nobody listens any more if the scenario has failed elsewhere.
    let _ = arrived.send(arrival).await;
    while !*leave.borrow_and_update() {
        tokio::select! {
            _ = leave.changed() => {}
            idled = bot.idle(Duration::from_millis(50)) => idled?,
        }
    }
    Ok(())
}

/// Places a block next to where the bot stands and breaks it again, waiting each time
/// for the server to report the change and to acknowledge the action. Only the region
/// the bot is in can do that.
async fn build(bot: &mut Bot) -> Result<()> {
    let below = (
        bot.location.0.floor() as i32,
        GROUND - 1,
        bot.location.2.floor() as i32 + 1,
    );
    let (x, y, z) = (below.0, GROUND, below.2);
    let air = Some(i32::from(blocks::AIR.0));

    let sequence = bot
        .use_item_on(below.0, below.1, below.2, face::TOP)
        .await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.acknowledged_sequence >= sequence
            && bot
                .block_at(x, y, z)
                .is_ok_and(|block| block.is_some() && block != air)
    })
    .await
    .context("the placed block did not appear")?;

    let sequence = bot.dig(x, y, z).await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.acknowledged_sequence >= sequence
            && bot.block_at(x, y, z).is_ok_and(|block| block == air)
    })
    .await
    .context("the broken block did not disappear")
}
