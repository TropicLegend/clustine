//! `clustine move`, `clustine merge` and `clustine split`: what somebody asks the
//! coordinator for by hand; and `clustine gamemode` and `clustine give`, an operator's
//! orders for a player.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use clustine_coordinator::{Asker, MoveAnswer, Mover, give_order};
use clustine_region::RegionId;
use clustine_rpc::Order;
use clustine_world::{ChunkPos, GameMode};

/// Settings of `clustine move`.
#[derive(Debug, Clone)]
pub struct MoveArgs {
    /// Host and port of the coordinator.
    pub coordinator: String,
    pub region: RegionId,
    /// The worker to move the region to, or any worker that waits.
    pub to: Option<String>,
}

/// Asks the coordinator to move a region and says what came of it; see
/// `docs/adr/0009-moving-a-region.md`. Fails if the move is refused or the coordinator
/// goes away before it says how it ended.
///
/// The time it prints is from asking to the region being another worker's. How long
/// players stood still it cannot know: that also takes the new owner restoring the
/// region and the edges resuming with it.
pub async fn move_region(args: MoveArgs) -> Result<()> {
    let asked = Instant::now();
    let mut mover = Mover::ask(&args.coordinator, args.region, args.to.as_deref())
        .await
        .with_context(|| format!("reaching the coordinator at {}", args.coordinator))?;
    loop {
        let answer = mover
            .next()
            .await
            .context("the coordinator went away before it said what came of the move")?;
        match answer {
            MoveAnswer::Refused { reason } => bail!("the coordinator refused: {reason}"),
            MoveAnswer::Begun { from, to } => {
                println!("{from} is releasing region {} for {to}", args.region);
            }
            MoveAnswer::Done {
                to,
                epoch,
                released,
            } => {
                let how = if released {
                    "released by its owner"
                } else {
                    "taken from its owner, which did not release it in time"
                };
                println!(
                    "region {} is run by {to} with epoch {epoch} now, {how}, {} ms after asking",
                    args.region,
                    asked.elapsed().as_millis()
                );
                return Ok(());
            }
        }
    }
}

/// Settings of `clustine merge`.
#[derive(Debug, Clone)]
pub struct MergeArgs {
    /// Host and port of the coordinator.
    pub coordinator: String,
    /// The region that absorbs the other and goes on.
    pub survivor: RegionId,
    /// The region that is absorbed and is no more afterwards.
    pub absorbed: RegionId,
}

/// Asks the coordinator to have one region absorb another and says what came of it;
/// see `docs/adr/0014-merging-and-splitting.md`, section 5.1. Fails if the coordinator
/// refuses, if nothing came of the merge, or if the coordinator goes away before it
/// says.
///
/// The time it prints is from asking to the coordinator knowing that the world store
/// has the merge. Like `clustine move`, it cannot know how long players stood still.
pub async fn merge_regions(args: MergeArgs) -> Result<()> {
    let asked = Instant::now();
    let (survivor, absorbed) = (args.survivor, args.absorbed);
    let asker = Asker::merge(&args.coordinator, survivor, absorbed)
        .await
        .with_context(|| format!("reaching the coordinator at {}", args.coordinator))?;
    let answer = asker
        .answer()
        .await
        .context("the coordinator went away before it said what came of the merge")?;
    match answer {
        Ok(survivor) => {
            println!(
                "region {survivor} has absorbed region {absorbed}, {} ms after asking",
                asked.elapsed().as_millis()
            );
            Ok(())
        }
        Err(reason) => {
            bail!(
                "the coordinator reports no merge of region {absorbed} into region {survivor}: {reason}"
            )
        }
    }
}

/// Settings of `clustine split`.
#[derive(Debug, Clone)]
pub struct SplitArgs {
    /// Host and port of the coordinator.
    pub coordinator: String,
    /// The region to split.
    pub region: RegionId,
    /// The chunks whose players are to be split off, with what is nearer to them than
    /// to anyone who stays.
    pub chunks: Vec<ChunkPos>,
}

/// Asks the coordinator to split the players standing in certain chunks off a region,
/// as a region of their own, and says what came of it; see
/// `docs/adr/0014-merging-and-splitting.md`, section 5.1. Fails if the coordinator
/// refuses, if nothing came of the split, or if the coordinator goes away before it
/// says.
///
/// The time it prints is from asking to the coordinator hearing of the new region
/// from the worker that made it and runs it.
pub async fn split_region(args: SplitArgs) -> Result<()> {
    let asked = Instant::now();
    let region = args.region;
    let asker = Asker::split(&args.coordinator, region, &args.chunks)
        .await
        .with_context(|| format!("reaching the coordinator at {}", args.coordinator))?;
    let answer = asker
        .answer()
        .await
        .context("the coordinator went away before it said what came of the split")?;
    match answer {
        Ok(part) => {
            println!(
                "region {part} has been split off region {region}, {} ms after asking",
                asked.elapsed().as_millis()
            );
            Ok(())
        }
        Err(reason) => bail!("the coordinator reports no split of region {region}: {reason}"),
    }
}

/// An operator's order as it is written after `clustine` or on the single binary's
/// standard input: `gamemode <player> <survival|creative>` or `give <player> <item>
/// [count]`, with the item named with or without `minecraft:`. Refused as the game's
/// commands refuse them: an unknown mode or item, air, a count below 1, or more than a
/// hundred of the item's stacks (`commands.give.failed.toomanyitems`).
pub fn order_from_words(words: &[&str]) -> Result<(String, Order)> {
    match words {
        ["gamemode", player, mode] => {
            let Some(mode) = GameMode::named(mode) else {
                bail!("there is no game mode called {mode}");
            };
            if !matches!(mode, GameMode::Survival | GameMode::Creative) {
                bail!("only survival and creative are built");
            }
            Ok(((*player).to_owned(), Order::GameMode(mode)))
        }
        ["give", player, item] => give_words(player, item, "1"),
        ["give", player, item, count] => give_words(player, item, count),
        _ => bail!(
            "an order is `gamemode <player> <survival|creative>` or `give <player> <item> [count]`"
        ),
    }
}

fn give_words(player: &str, item: &str, count: &str) -> Result<(String, Order)> {
    let name = if item.contains(':') {
        item.to_owned()
    } else {
        format!("minecraft:{item}")
    };
    let Some(id) = clustine_data::item_id_by_name(&name).filter(|&id| id != 0) else {
        bail!("there is no item called {item}");
    };
    let count: i32 = count
        .parse()
        .with_context(|| format!("{count} is not a count"))?;
    let largest = i32::from(clustine_data::ItemComponents::of(id).max_stack_size).max(1);
    if count < 1 {
        bail!("a count has to be at least 1");
    }
    if count > largest * 100 {
        bail!("can't give more than {} of {name}", largest * 100);
    }
    Ok((player.to_owned(), Order::Give { item: id, count }))
}

/// Gives an operator's order through the coordinator at `coordinator`, which passes it
/// to every edge (`clustine gamemode`, `clustine give`), and says to how many. Whether
/// the player was there to take it is not known.
pub async fn give(coordinator: &str, words: &[&str]) -> Result<()> {
    let (player, order) = order_from_words(words)?;
    let edges = give_order(coordinator, &player, order)
        .await
        .with_context(|| format!("reaching the coordinator at {coordinator}"))?;
    println!("passed the order for {player} to {edges} edge(s)");
    Ok(())
}
