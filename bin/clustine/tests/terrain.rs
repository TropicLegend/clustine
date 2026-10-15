//! A world with a seed: the single process serves the land the terrain generator
//! makes, puts players on its ground, and keeps what they do to it.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use clustine::{Config, Land, Server};
use clustine_botswarm::Bot;
use clustine_data::blocks;
use clustine_protocol::packets::play::face;
use clustine_world::{BlockPos, Chunk, ChunkGenerator, ChunkPos, Vec3};
use clustine_worldgen::TerrainGenerator;

use common::processes::Cluster;
use common::{config, free_address, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// A world whose origin is on land: grass at the height 70, on ground that is not
/// level. The game puts its spawn on the column at the origin, the first of its chunk.
const SEED: i64 = 3;

/// The smallest a server grants, so that the test makes few chunks.
const VIEW: i32 = 2;

const AIR: i32 = blocks::AIR.0 as i32;
const STONE: i32 = blocks::STONE.0 as i32;

fn on_disk(world: &std::path::Path) -> Config {
    Config {
        world: Some(world.to_owned()),
        view_distance: VIEW,
        ..config()
    }
}

async fn start(world: &std::path::Path) -> (Server, String) {
    let server = match Server::start_in(on_disk(world), Land::of_seed(SEED)).await {
        Ok(server) => server,
        Err(error) => panic!("the server did not start: {error:#}"),
    };
    let address = server.address().to_string();
    (server, address)
}

/// Joins, trying again while the server still has the player's last connection, and
/// waits for the chunks around `centre`.
async fn join(address: &str, name: &str, centre: (i32, i32)) -> Bot {
    let mut bot = loop {
        match Bot::join(address, name).await {
            Ok(bot) => break bot,
            // Until the server has noticed that the one before left.
            Err(_) => tokio::task::yield_now().await,
        }
    };
    let area = view_area(centre, VIEW);
    bot.wait_until(PATIENCE, |bot| {
        area.iter().all(|chunk| bot.chunks.contains_key(chunk))
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {name} has {} chunks", bot.chunks.len()));
    bot
}

/// The heights of a heightmap as a chunk's packet carries them: nine bits a column
/// for a world 384 blocks high, seven columns to a long.
fn heights(packed: &[u64]) -> Vec<u64> {
    (0..256)
        .map(|column| packed[column / 7] >> (column % 7 * 9) & 0x1ff)
        .collect()
}

/// The highest block of the column that is not air, in the chunk as generated.
fn top(chunk: &Chunk, x: usize, z: usize) -> i32 {
    chunk.min_y() + i32::from(chunk.surface_heights()[z << 4 | x]) - 1
}

#[tokio::test]
async fn a_world_with_a_seed_is_its_land_and_keeps_what_is_done_to_it() {
    let directory = tempfile::tempdir().unwrap();
    let generator = TerrainGenerator::new(SEED);
    assert_eq!(generator.spawn(), BlockPos::new(0, 71, 0));
    let spawn = Vec3::bottom_centre_of(generator.spawn());
    let origin = generator.generate(ChunkPos::new(0, 0));

    let (server, address) = start(directory.path()).await;
    let mut walker = join(&address, "Walker", (0, 0)).await;

    // Put on the ground of the spawn's column: something under the feet,
    // nothing where the feet and the head are.
    let entered = walker.entered(PATIENCE).await.unwrap();
    assert_eq!(entered.location, (spawn.x, spawn.y, spawn.z));
    assert_eq!(entered.location, (0.5, 71.0, 0.5));
    let feet = spawn.y as i32;
    let under = walker.block_at(0, feet - 1, 0).unwrap().unwrap();
    assert_eq!(under, blocks::GRASS_BLOCK.0 as i32);
    assert_eq!(walker.block_at(0, feet, 0).unwrap(), Some(AIR));
    assert_eq!(walker.block_at(0, feet + 1, 0).unwrap(), Some(AIR));
    // The client is told the world's spawn, which a compass points to.
    let told = walker
        .world_spawn
        .clone()
        .expect("the world's spawn was told");
    assert_eq!(told.dimension, "minecraft:overworld");
    assert_eq!(
        (told.position.x, told.position.y, told.position.z),
        (0, 71, 0)
    );
    // Under the open sky, and dark inside the ground.
    assert_eq!(walker.light_at(0, feet, 0).unwrap().unwrap().sky, Some(15));
    let buried = walker.light_at(0, feet - 3, 0).unwrap().unwrap();
    assert_eq!((buried.sky, buried.block), (Some(0), Some(0)));

    // A client is told the hash of the seed, that the world is not flat and where
    // its sea is. (Until the edge's login packet says them, it says what it said.)
    let login = Land::of_seed(SEED).login();
    assert_eq!(login.hashed_seed, TerrainGenerator::hashed_seed(SEED));
    assert!(!login.flat);
    assert_eq!(login.sea_level, 63);

    // Every chunk of the view is the generator's, block for block and cell for cell,
    // and none of them is one block throughout or level like the flat world.
    let area = view_area((0, 0), VIEW);
    assert_eq!(walker.chunks.len(), area.len());
    let mut tops = BTreeSet::new();
    for position in &area {
        let made = generator.generate(ChunkPos::new(position.0, position.1));
        let sections = walker.sections(*position).unwrap().unwrap();
        assert_eq!(sections.len(), made.sections().len());
        let mut states = BTreeSet::new();
        for (sent, made) in sections.iter().zip(made.sections()) {
            assert_eq!(
                sent.block_count as u16,
                made.non_air_count(),
                "{position:?}"
            );
            for (index, state) in made.states().enumerate() {
                assert_eq!(sent.blocks.get(index), i32::from(state.0), "{position:?}");
                states.insert(state);
            }
            for (cell, biome) in made.biomes().enumerate() {
                assert_eq!(sent.biomes.get(cell), i32::from(biome.0), "{position:?}");
            }
        }
        assert!(states.len() > 3, "{position:?} has only {states:?}");

        let packet = &walker.chunks[position];
        assert!(!packet.heightmaps.is_empty());
        for heightmap in &packet.heightmaps {
            tops.extend(heights(&heightmap.data));
        }
    }
    assert!(tops.len() > 5, "the land is level: {tops:?}");
    let surface = &walker.chunks[&(0, 0)].heightmaps[0];
    let surface: BTreeSet<u64> = heights(&surface.data).into_iter().collect();
    assert!(surface.len() > 1, "the chunk at the origin is level");

    // Breaks the highest block of one column and places a stone on another, in
    // another chunk; both within reach.
    let beyond = generator.generate(ChunkPos::new(-1, -1));
    let (dug, built) = (top(&origin, 2, 1), top(&beyond, 14, 15));
    assert!(!origin.get(2, dug, 1).unwrap().is_air());
    walker.dig(2, dug, 1).await.unwrap();
    let last = walker.use_item_on(-2, built, -1, face::TOP).await.unwrap();
    walker
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence == last)
        .await
        .unwrap();
    let assert_built = |bot: &Bot| {
        assert_eq!(bot.block_at(2, dug, 1).unwrap(), Some(AIR));
        assert_eq!(bot.block_at(-2, built + 1, -1).unwrap(), Some(STONE));
        // What is beside them is as generated.
        let beside = i32::from(origin.get(3, dug - 1, 1).unwrap().0);
        assert_eq!(bot.block_at(3, dug - 1, 1).unwrap(), Some(beside));
        let under = i32::from(beyond.get(14, built, 15).unwrap().0);
        assert_eq!(bot.block_at(-2, built, -1).unwrap(), Some(under));
    };
    assert_built(&walker);

    // Goes a few steps, leaves, and comes back to where they left.
    walker.walk_to(3.5, 6.5, 0.5).await.unwrap();
    walker.stand_until_kept(PATIENCE).await.unwrap();
    let left = walker.kept();
    assert_eq!(left.location, (3.5, 71.0, 6.5));
    drop(walker);
    let mut walker = join(&address, "Walker", (0, 0)).await;
    assert_eq!(
        walker.entered(PATIENCE).await.unwrap().location,
        left.location
    );
    assert_built(&walker);
    drop(walker);
    server.stop().await;

    // The same world again: a stranger sees what was built, in land that is the
    // seed's as before, and whoever left is where they left.
    let (server, address) = start(directory.path()).await;
    let mut visitor = join(&address, "Visitor", (0, 0)).await;
    let entered = visitor.entered(PATIENCE).await.unwrap();
    assert_eq!(entered.location, (spawn.x, spawn.y, spawn.z));
    assert_built(&visitor);
    let far = generator.generate(ChunkPos::new(-2, 3));
    let sections = visitor.sections((-2, 3)).unwrap().unwrap();
    for (sent, made) in sections.iter().zip(far.sections()) {
        for (index, state) in made.states().enumerate() {
            assert_eq!(sent.blocks.get(index), i32::from(state.0));
        }
    }
    let mut walker = join(&address, "Walker", (0, 0)).await;
    assert_eq!(
        walker.entered(PATIENCE).await.unwrap().location,
        left.location
    );
    server.stop().await;

    // The world is this seed's for good: neither a flat server nor one of another
    // seed opens it.
    for (other, name) in [(Land::flat(), "flat"), (Land::of_seed(SEED + 1), "seed 4")] {
        let Err(error) = Server::start_in(on_disk(directory.path()), other).await else {
            panic!("the world of seed {SEED} was opened as {name}");
        };
        let said = format!("{error:#}");
        assert!(said.contains("generator"), "{said}");
        assert!(said.contains("seed 3"), "{said}");
    }
    // And it still opens as what it is.
    let (server, _) = start(directory.path()).await;
    server.stop().await;
}

/// The height above the highest block of the column at `x`, `z` that stops movement or
/// holds a fluid: the chunk's `motion_blocking` heightmap, counted from its bottom.
fn column_top(chunk: &Chunk, x: i32, z: i32) -> i32 {
    let column = ((z & 15) << 4 | (x & 15)) as usize;
    chunk.min_y() + i32::from(chunk.heights().motion_blocking[column])
}

/// The services as processes of their own, of which the two that need it are told the
/// seed: the store, which makes the chunks and puts the spawn where the game would,
/// and the edge, which tells clients of the world. The coordinator learns where
/// players enter from the store.
#[tokio::test(flavor = "multi_thread")]
async fn a_cluster_whose_processes_are_told_the_seed_serves_its_land() {
    let directory = tempfile::tempdir().unwrap();
    let generator = TerrainGenerator::new(SEED);
    let spawn = Vec3::bottom_centre_of(generator.spawn());
    let seed = format!("--seed={SEED}");

    let mut cluster = Cluster::new(directory.path(), 1, "").await;
    cluster.view_distance = VIEW;
    cluster.store_arguments = vec![seed.clone()];
    cluster.edge_arguments = vec![seed];
    cluster.start().await;

    let mut walker = join(&cluster.edge.0, "Walker", (0, 0)).await;
    let entered = walker.entered(PATIENCE).await.unwrap();
    // On the spawn column as the home chunk has it, which nobody has changed: its
    // top as the generator made it, whether the region had loaded the chunk when the
    // player entered or not (docs/groundwork/w6-plan.md, section 2).
    let home = generator.generate(ChunkPos::containing(spawn.x, spawn.z));
    let top = column_top(&home, spawn.x.floor() as i32, spawn.z.floor() as i32);
    assert_eq!(f64::from(top), spawn.y);
    assert_eq!(entered.location, (spawn.x, spawn.y, spawn.z));
    // The edge, which makes no generator, tells the client the spawn the store keeps.
    let told = walker
        .world_spawn
        .clone()
        .expect("the world's spawn was told");
    let block = generator.spawn();
    assert_eq!(
        (told.position.x, told.position.y, told.position.z),
        (block.x, block.y, block.z)
    );
    for position in [(0, 0), (-3, 2)] {
        let made = generator.generate(ChunkPos::new(position.0, position.1));
        let sections = walker.sections(position).unwrap().unwrap();
        for (sent, made) in sections.iter().zip(made.sections()) {
            for (index, state) in made.states().enumerate() {
                assert_eq!(sent.blocks.get(index), i32::from(state.0), "{position:?}");
            }
        }
    }
    // The ground under the player's feet can be dug.
    let feet = spawn.y as i32;
    let dug = walker.dig(0, feet - 1, 0).await.unwrap();
    walker
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence == dug)
        .await
        .unwrap();
    assert_eq!(walker.block_at(0, feet - 1, 0).unwrap(), Some(AIR));
    drop(walker);
    cluster.terminate().await;
}

/// What the server's command says to a seed that is none, and to a world that was
/// made with another.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_refuses_a_seed_that_is_no_number_and_a_world_of_another_seed() {
    let run = |arguments: Vec<String>| async move {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_clustine"))
            .args(&arguments)
            .env("NO_COLOR", "1")
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(PATIENCE, output).await;
        let output = output
            .unwrap_or_else(|_| panic!("{arguments:?} did not end"))
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };

    // Not a number, for the single process and for each service that takes a seed.
    for service in [
        vec![],
        vec!["worldstore"],
        vec!["coordinator"],
        vec!["edge"],
    ] {
        for nonsense in [
            "--seed=lots",
            "--seed=1.5",
            "--seed=99999999999999999999",
            "--seed=",
        ] {
            let mut arguments = words(&service);
            arguments.push(nonsense.to_owned());
            let (code, said) = run(arguments).await;
            assert_eq!(code, Some(2), "{service:?} {nonsense}: {said}");
            assert!(said.contains("--seed"), "{service:?} {nonsense}: {said}");
        }
    }
    // A worker makes nothing and is told no seed.
    let (code, said) = run(words(&["worker", "--name", "w", "--seed=3"])).await;
    assert_eq!(code, Some(2), "{said}");
    // Nor is the coordinator any more, which learns where players enter from the
    // world store, and whoever still gives it one is told so.
    let (code, said) = run(words(&["coordinator", "--seed=3"])).await;
    assert_eq!(code, Some(2), "{said}");
    assert!(said.contains("world store"), "{said}");

    // A world of the seed 3, made by a server that has stopped.
    let directory = tempfile::tempdir().unwrap();
    let (server, _) = start(directory.path()).await;
    server.stop().await;
    let world = directory.path().to_str().unwrap();
    let listen = free_address().await;
    for other in [None, Some("--seed=4"), Some("--seed=-3")] {
        let mut single = words(&["--world", world, "--bind", &listen]);
        let mut store = words(&["worldstore", "--world", world, "--listen", &listen]);
        single.extend(other.map(str::to_owned));
        store.extend(other.map(str::to_owned));
        for arguments in [single, store] {
            let (code, said) = run(arguments.clone()).await;
            assert_eq!(code, Some(1), "{arguments:?}: {said}");
            assert!(said.contains("generator"), "{arguments:?}: {said}");
            assert!(said.contains("seed 3"), "{arguments:?}: {said}");
        }
    }
}

/// What the preview costs a player, made on the generation scheduler's pool:
///
/// ```text
/// cargo test --release -p clustine --test terrain -- --ignored --nocapture
/// ```
///
/// The roadmap has the numbers this gave.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, for a release build"]
async fn what_a_join_and_a_flight_cost_with_a_seed() {
    use std::time::Instant;

    const SEED: i64 = 13579;
    const VIEW: i32 = 8;
    /// Over the land of that seed, whatever it is.
    const HEIGHT: f64 = 250.0;
    let whole = |bot: &Bot, centre: (i32, i32)| {
        let area = view_area(centre, VIEW);
        area.iter().all(|chunk| bot.chunks.contains_key(chunk))
    };

    let directory = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let config = Config {
        world: Some(directory.path().to_owned()),
        view_distance: VIEW,
        ..config()
    };
    let server = Server::start_in(config, Land::of_seed(SEED)).await.unwrap();
    let address = server.address().to_string();
    println!("the server started in {:?}", started.elapsed());

    for name in ["First", "Second"] {
        let asked = Instant::now();
        let mut bot = Bot::join(&address, name).await.unwrap();
        let entered = asked.elapsed();
        // The chunk the player is in, which a client waits for before it shows the
        // world, and the nine around it.
        let own = |bot: &Bot| bot.chunks.contains_key(&(0, 0));
        bot.wait_until(PATIENCE, own).await.unwrap();
        let (stood, had) = (asked.elapsed(), bot.chunks.len());
        bot.wait_until(PATIENCE, |bot| whole(bot, (0, 0)))
            .await
            .unwrap();
        println!(
            "{name} joined: in the world after {entered:?}, its own chunk after {stood:?} (as one of the first {had}), all {} chunks after {:?}",
            bot.chunks.len(),
            asked.elapsed()
        );
        if name == "First" {
            // Stays, so that the second finds the chunks loaded.
            std::mem::forget(bot);
            continue;
        }

        // Straight east for 32 chunks at each speed, in blocks a tick: sprinting,
        // flying, flying at a sprint, and gliding with rockets.
        bot.set_flying(true).await.unwrap();
        bot.fly_to((0.5, HEIGHT, 0.5), (-90.0, 0.0), 4.0)
            .await
            .unwrap();
        let mut from = 0.5;
        for speed in [0.28, 0.54, 1.08, 1.7, 3.0] {
            let to = from + 32.0 * 16.0;
            let centre = ((to as i32) >> 4, 0);
            let left = Instant::now();
            bot.fly_to((to, HEIGHT, 0.5), (-90.0, 0.0), speed)
                .await
                .unwrap();
            let flew = left.elapsed();
            let missing = view_area(centre, VIEW)
                .iter()
                .filter(|chunk| !bot.chunks.contains_key(chunk))
                .count();
            let arrived = Instant::now();
            bot.wait_until(PATIENCE, |bot| whole(bot, centre))
                .await
                .unwrap();
            println!(
                "{speed} blocks a tick ({:.1} a second): 32 chunks in {flew:?}; on arriving {missing} chunks of the view were missing, and came within {:?}",
                speed * 20.0,
                arrived.elapsed()
            );
            from = to;
        }
    }
    server.stop().await;
}
