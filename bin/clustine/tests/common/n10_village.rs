//! What N10's comparisons with the official server share (`docs/groundwork/n10-plan.md`,
//! section 14): both servers started at a day time, a pit beside the player with a
//! villager in it, and the steps a [`Scene`] has none of (an interaction with a mob, the
//! choice of a trade).
//!
//! **Neither server is driven by commands once a scene runs.** Clustine has none: no
//! `/summon` with an entity's tag, no `/time set`, no `/effect`. So every scene is made
//! by what a creative player can do on both (a mob by its egg, a job site placed beside
//! it, an ominous bottle drunk), and the one thing set beforehand is the clock: the
//! official server's by `time set` on its console, Clustine's by the world store's
//! table, written between a first start and the start the scene runs on (as
//! `monsters_oracle.rs` does), which the region takes as the later of it and its own.
//! Clustine's day is then the second one, the official server's the first: a villager's
//! schedule reads the time of day alone, and the day's number only where a restock
//! compares two of them, which no scene here reaches.
//!
//! **The game time is left as each server has it**, a few thousand ticks at most on
//! both, since `Villager.shouldRestock` compares it with the last restock plus 12000:
//! a world whose game time had been moved on would make up missed restocks the other
//! does not.
//!
//! **The pit.** A row of cells two east of the player, dug two deep, so that a mob in it
//! stands with its feet at `GROUND - 1` and cannot climb out. The player hovers above
//! it, within reach of its floor and of the mob in it.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clustine::Config;
use clustine_botswarm::Oracle;
use clustine_format::TableFile;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientboundPlay, EntityDataValue, Interact, SelectTrade, face,
};
use tokio::time::Instant;

use crate::common::{config, start_with};
use crate::n9_scene::{GROUND, PATIENCE, Scene, Seen, lenient_data, ticks};

/// How long a villager may take to claim a point of interest near it: `AcquirePoi`
/// scans every 20 to 40 ticks once its first wait is over, and a path's search may
/// fail and be put back by 40 to 79 ticks (`docs/groundwork/n10-brief.md`, finding 12).
/// The player's classic test sees a librarian within a minute.
pub const CLAIM_PATIENCE: Duration = Duration::from_secs(90);

/// How deep the pit is dug.
const DEPTH: i32 = 2;

/// Runs `run` against the official server and then against Clustine, each on a fresh
/// flat world whose overworld clock is at `day` ticks into a day, and gives what each
/// did, the official server's first.
pub async fn on_both_at<T>(
    day: u64,
    run: impl AsyncFn(&str) -> Result<T>,
) -> (Result<T>, Result<T>) {
    let oracle = Oracle::start(false)
        .await
        .expect("the official server starts");
    let theirs = match oracle
        .command(
            &format!("time set {day}"),
            &["tick(s)", "Set the time", "already set"],
        )
        .await
    {
        Ok(_) => run(oracle.address()).await,
        Err(error) => Err(error),
    };
    drop(oracle);
    let world = tempfile::tempdir().expect("a directory for Clustine's world");
    let ours = match start_clustine_at(world.path(), day).await {
        Ok(server) => {
            let address = server.address().to_string();
            let ours = run(&address).await;
            server.stop().await;
            ours
        }
        Err(error) => Err(error),
    };
    (theirs, ours)
}

/// Starts Clustine on a new world in `directory` whose overworld clock is at `day`
/// ticks into its second day, its game time as it was.
async fn start_clustine_at(directory: &Path, day: u64) -> Result<clustine::Server> {
    let on_disk = || Config {
        world: Some(directory.to_owned()),
        ..config()
    };
    let (server, _) = start_with(on_disk()).await;
    server.stop().await;
    let path = directory.join("regions").join("table");
    let mut table = TableFile::decode(&std::fs::read(&path)?).context("the table file")?;
    let wanted = 24_000 + day;
    ensure!(
        table.clock[1] < wanted,
        "the new world's clock {:?} is already past {day} of its second day",
        table.clock
    );
    table.clock[1] = wanted;
    std::fs::write(&path, table.encode())?;
    Ok(start_with(on_disk()).await.0)
}

/// The time of day, 0 to 23999, of the overworld's clock at `at`, as a client tells it:
/// the latest time packet before then and the ticks since it came.
pub fn day_time(scene: &Scene, at: Instant) -> Result<i64> {
    let clock = scene.registered("minecraft:world_clock", "minecraft:overworld")?;
    scene
        .log
        .iter()
        .take_while(|seen| seen.at <= at)
        .filter_map(|seen| match &seen.packet {
            Some(ClientboundPlay::SetTime(time)) => time
                .clocks
                .iter()
                .find(|state| state.clock == clock)
                .map(|state| (seen.at, state.total_ticks)),
            _ => None,
        })
        .last()
        .map(|(came, total)| (total + ticks(came, at)).rem_euclid(24_000))
        .context("no time packet before")
}

/// A row of `length` cells along z, two east of where the player stood, dug
/// [`DEPTH`] deep.
#[derive(Debug, Clone, Copy)]
pub struct Pit {
    pub x: i32,
    pub z: i32,
    pub length: i32,
}

impl Pit {
    /// The pit beside the player of `scene`, not dug yet.
    pub fn beside(scene: &Scene, length: i32) -> Pit {
        let (x, z) = scene.column();
        Pit {
            x: x + 2,
            z,
            length,
        }
    }

    pub fn cells(&self) -> Vec<(i32, i32)> {
        (0..self.length).map(|dz| (self.x, self.z + dz)).collect()
    }

    /// The block under the cell `dz`, which what stands in it stands on.
    pub fn floor(&self, dz: i32) -> (i32, i32, i32) {
        (self.x, GROUND - DEPTH, self.z + dz)
    }

    /// The block of the cell `dz` where a mob's feet are.
    pub fn feet(&self, dz: i32) -> (i32, i32, i32) {
        (self.x, GROUND - DEPTH + 1, self.z + dz)
    }

    /// Digs it, which leaves the player hovering above its middle.
    pub async fn dig(&self, scene: &mut Scene) -> Result<()> {
        scene.dig_pit(&self.cells(), DEPTH).await
    }
}

/// Uses the egg `egg` on the top of `floor` and returns the mob of `kind` it made.
pub async fn hatch(scene: &mut Scene, egg: i32, kind: i32, floor: (i32, i32, i32)) -> Result<i32> {
    let from = scene.mark();
    scene.take(ItemStack::new(egg, 1)).await?;
    scene.use_on(floor.0, floor.1, floor.2, face::TOP).await?;
    let made = scene.wait_added(from, kind, 1, PATIENCE).await?;
    Ok(made.first().context("the mob")?.1.entity_id)
}

/// Uses the hand on the entity `entity` (`interact`), aiming half a block above its feet,
/// within a baby's box too, not sneaking: what opens a villager's trades or feeds a
/// zombie villager its apple.
pub async fn interact(scene: &mut Scene, entity: i32) -> Result<()> {
    scene
        .send(&Interact {
            entity_id: entity,
            hand: 0,
            location: [0.0, 0.5, 0.0],
            using_secondary_action: false,
        })
        .await
}

/// Picks the trade `index` in the merchant's window (`select_trade`).
pub async fn select_trade(scene: &mut Scene, index: i32) -> Result<()> {
    scene.send(&SelectTrade { item: index }).await
}

/// The entity data `seen` gives `entity`, where it is such a packet.
pub fn data_of(seen: &Seen, entity: i32) -> Option<Vec<(u8, EntityDataValue)>> {
    if seen.id != clientbound::SET_ENTITY_DATA {
        return None;
    }
    let (of, values) = lenient_data(&seen.frame)?;
    (of == entity).then_some(values)
}

/// The villager data among `values`: its type, profession and level.
pub fn villager_data(values: &[(u8, EntityDataValue)]) -> Option<(i32, i32, i32)> {
    values.iter().find_map(|(_, value)| match value {
        EntityDataValue::VillagerData {
            kind,
            profession,
            level,
        } => Some((*kind, *profession, *level)),
        _ => None,
    })
}

/// Reads on until `test` holds for a packet read from `from` on, at most for
/// `patience`, and gives when that packet came. Only the newest packet is looked at
/// after each read, so a long wait does not go over the whole log again and again.
pub async fn until_seen(
    scene: &mut Scene,
    from: usize,
    patience: Duration,
    what: &str,
    test: impl Fn(&Seen) -> bool,
) -> Result<Instant> {
    if let Some(seen) = scene.since(from).iter().find(|seen| test(seen)) {
        return Ok(seen.at);
    }
    scene
        .until(patience, what, |scene| {
            scene.log.len() > from && scene.log.last().is_some_and(&test)
        })
        .await?;
    Ok(scene.log.last().context("the packet waited for")?.at)
}

/// Waits until the villager `villager` is shown with the profession `profession`, from
/// `from` on, and gives when.
pub async fn until_profession(
    scene: &mut Scene,
    from: usize,
    villager: i32,
    profession: i32,
) -> Result<Instant> {
    until_seen(
        scene,
        from,
        CLAIM_PATIENCE,
        "the villager's profession",
        |seen| {
            data_of(seen, villager)
                .and_then(|values| villager_data(&values))
                .is_some_and(|(_, shown, _)| shown == profession)
        },
    )
    .await
}

/// The entity events `entity` was sent from `from` on, in order.
pub fn events_of(scene: &Scene, from: usize, entity: i32) -> Vec<i8> {
    scene
        .of(from, |packet| match packet {
            ClientboundPlay::EntityEvent(event) if event.entity_id == entity => Some(event.event),
            _ => None,
        })
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}
