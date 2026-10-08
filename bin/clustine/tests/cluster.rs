//! The services as processes of their own on this machine: a coordinator, a world
//! store, two workers and an edge, which together are one server.

mod common;

use std::time::Duration;

use clustine_botswarm::{Bot, Crossing, cross};
use clustine_data::blocks;
use clustine_protocol::packets::play::face;

use common::processes::Cluster;
use common::{VIEW_DISTANCE, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const STONE: Option<i32> = Some(blocks::STONE.0 as i32);

/// The chunk x coordinate at which the world is divided: block x = 48.
const BOUNDARY: &str = "3";

/// A cluster of two workers, one for each side of the boundary.
async fn cluster(directory: &std::path::Path) -> Cluster {
    Cluster::new(directory, 2, BOUNDARY).await
}

/// Joins and waits for the chunks around the spawn point, which reach across the
/// boundary.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// Breaks a block west of the boundary and places one east of it, each from its own
/// side, and breaks one east of it from the west, which the two workers have to do
/// together. Waits until the server has confirmed all of it.
async fn build_on_both_sides(bot: &mut Bot) {
    bot.walk_to(45.5, 0.5, 0.5).await.unwrap();
    bot.dig(46, -61, 1).await.unwrap();
    bot.dig(48, -61, 3).await.unwrap();
    bot.walk_to(49.5, 0.5, 0.5).await.unwrap();
    let last = bot.use_item_on(50, -61, 1, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    assert_built_on_both_sides(bot);
}

fn assert_built_on_both_sides(bot: &Bot) {
    assert_eq!(bot.block_at(46, -61, 1).unwrap(), AIR);
    assert_eq!(bot.block_at(48, -61, 3).unwrap(), AIR);
    assert_eq!(bot.block_at(50, -60, 1).unwrap(), STONE);
}

/// The world store dies and is started again. The workers keep their regions: each
/// opens its region again, restores it from what the store has and lets edges in once
/// more. What players were shown before is there, because nothing is shown that the
/// store does not have.
#[tokio::test(flavor = "multi_thread")]
async fn workers_restore_their_regions_when_the_world_store_is_back() {
    let directory = tempfile::tempdir().unwrap();
    let mut cluster = cluster(directory.path()).await;
    cluster.start().await;
    let address = cluster.edge.0.clone();
    let workers = ["worker-0", "worker-1"];

    let mut builder = join(&address, "Builder").await;
    build_on_both_sides(&mut builder).await;
    let mut store = cluster.store.1.take().unwrap();
    store.kill().await.unwrap();
    for worker in workers {
        cluster
            .wait_for_log(worker, "lost the world store; opening the region again", 1)
            .await;
    }
    drop(builder);

    cluster.start_store();
    for worker in workers {
        cluster.wait_for_log(worker, "running a region", 2).await;
    }
    // The edge starts over once it reaches every region again.
    let mut visitor = None;
    for _ in 0..600 {
        if let Ok(bot) = Bot::join(&address, "Visitor").await {
            visitor = Some(bot);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut visitor =
        visitor.unwrap_or_else(|| panic!("nobody could join again:\n{}", cluster.all_logs()));
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    visitor.wait_for_chunks(count, PATIENCE).await.unwrap();
    assert_built_on_both_sides(&visitor);
    drop(visitor);

    // The workers are the processes they were, and stop cleanly with their regions.
    for (name, process) in cluster.processes() {
        let ended = process.as_mut().unwrap().try_wait().unwrap();
        assert_eq!(ended, None, "{name} ended");
    }
    cluster.terminate().await;
}

/// Bots walk back and forth between two worker processes and build on both sides. What
/// they built outlasts every process being killed, and each process stops cleanly when
/// asked to.
#[tokio::test(flavor = "multi_thread")]
async fn a_cluster_of_processes_is_one_server() {
    let directory = tempfile::tempdir().unwrap();
    let mut cluster = cluster(directory.path()).await;
    cluster.start().await;
    let address = cluster.edge.0.clone();

    let crossing = Crossing {
        walkers: 3,
        rounds: 2,
        west: 20.5,
        // As far east as the watcher at the spawn point sees with the tests' view distance.
        east: 70.5,
        // Where the eastern worker's region begins. The bots build across it too, which
        // takes both workers.
        line: Some(48),
        ..Crossing::default()
    };
    let report = cross(&address, &crossing)
        .await
        .unwrap_or_else(|error| panic!("{error:#}\n{}", cluster.all_logs()));
    assert_eq!(report.crossings, 12);
    assert_eq!(report.blocks_built, 24);
    // Both workers took part: each let every walker go twice and took them in twice.
    for worker in ["worker-0", "worker-1"] {
        let log = cluster.log(worker);
        for message in [
            "player arrived from another region",
            "player departed to another region",
        ] {
            let count = log.matches(message).count();
            assert!(
                count >= 6,
                "{worker} logged `{message}` {count} times:\n{log}"
            );
        }
    }

    // What both regions were told is on disk before players are, whatever becomes of
    // the processes.
    let mut builder = join(&address, "Builder").await;
    build_on_both_sides(&mut builder).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    cluster.kill().await;
    drop(builder);

    cluster.start().await;
    let mut visitor = join(&address, "Visitor").await;
    assert_built_on_both_sides(&visitor);

    // Asked to stop, the workers hand what has changed to the world store, which then
    // has nothing left in its logs.
    visitor.dig(2, -61, 1).await.unwrap();
    visitor.walk_to(49.5, 0.5, 0.5).await.unwrap();
    let last = visitor.dig(50, -60, 1).await.unwrap();
    visitor
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    drop(visitor);
    cluster.terminate().await;
    // The log is in segments, which are removed once checkpoints cover them.
    let segments: Vec<_> = std::fs::read_dir(cluster.world.join("log"))
        .unwrap()
        .map(|segment| segment.unwrap().path())
        .collect();
    assert_eq!(segments, Vec::<std::path::PathBuf>::new());

    // And the next cluster on that world finds it as it was left.
    cluster.start().await;
    let visitor = join(&address, "Visitor").await;
    assert_eq!(visitor.block_at(2, -61, 1).unwrap(), AIR);
    assert_eq!(visitor.block_at(46, -61, 1).unwrap(), AIR);
    assert_eq!(visitor.block_at(50, -60, 1).unwrap(), AIR);
    drop(visitor);
    cluster.terminate().await;
}
