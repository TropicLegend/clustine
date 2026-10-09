//! `clustine move`, `clustine merge` and `clustine split`: what somebody asks the
//! coordinator for by hand.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use clustine_coordinator::{Asker, MoveAnswer, Mover};
use clustine_region::RegionId;
use clustine_world::ChunkPos;

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
