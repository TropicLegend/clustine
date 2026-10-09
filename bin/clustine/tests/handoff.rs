//! A world divided into regions: players walk from one region into the next without
//! noticing, and without anyone who watches noticing.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, Crossing, cross};
use clustine_data::{blocks, items};
use clustine_protocol::packets::play::face;

use common::{VIEW_DISTANCE, config, free_address, spawn_server, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const GRASS: Option<i32> = Some(blocks::GRASS_BLOCK.0 as i32);
const GLASS: Option<i32> = Some(blocks::GLASS.0 as i32);
const STONE: Option<i32> = Some(blocks::STONE.0 as i32);

/// The chunk x coordinate at which the world of these tests is divided: the region in
/// the west has the spawn point, the one in the east begins at block x = 16.
const BOUNDARY: i32 = 1;

/// The height players stand at.
const GROUND: f64 = -60.0;

/// A server whose world is two regions.
fn divided() -> Config {
    Config {
        pins: vec![BOUNDARY],
        ..config()
    }
}

/// Joins and waits for the chunks around the spawn point, which come from both regions.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// Waits until `bot` holds exactly the chunks around `center`.
async fn settle(bot: &mut Bot, center: (i32, i32)) {
    let mut expected = view_area(center, VIEW_DISTANCE);
    expected.sort_unstable();
    bot.wait_until(PATIENCE, |bot| {
        bot.center == Some(center) && bot.chunks.keys().copied().eq(expected.iter().copied())
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: around {:?}", bot.center));
}

/// Waits until `bot` knows the block at `position` to be `state`.
async fn sees_block(bot: &mut Bot, position: (i32, i32, i32), state: Option<i32>) {
    let (x, y, z) = position;
    bot.wait_until(PATIENCE, |bot| bot.block_at(x, y, z).unwrap() == state)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the block at {position:?} is {:?}, not {state:?}",
                bot.block_at(x, y, z)
            )
        });
}

/// Waits until the server has handled everything `bot` did up to `sequence`.
async fn acknowledged(bot: &mut Bot, sequence: i32) {
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .unwrap_or_else(|error| panic!("{error}: sequence {sequence} was not acknowledged"));
}

/// Waits until `bot` sees the player called `name` at the given x and z.
async fn sees_at(bot: &mut Bot, name: &str, x: f64, z: f64) {
    bot.wait_until(PATIENCE, |bot| {
        bot.seen_player(name)
            .is_some_and(|entity| entity.position == (x, GROUND, z))
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {name} is seen as {:?}", bot.seen_player(name)));
}

/// A player walks east into the other region and back. They stay connected, keep
/// getting the chunks around them, and what they hold comes along. Checked over direct
/// and over serialising links to the regions.
#[tokio::test]
async fn a_player_walks_into_the_next_region_and_back() {
    for serialise_link in [false, true] {
        let (server, address) = start_with(Config {
            serialise_link,
            ..divided()
        })
        .await;
        let mut walker = join(&address, "Walker").await;
        let entity_id = walker.info.login.entity_id;

        // Glass into the fourth slot, selected, while still in the western region.
        walker
            .take_from_creative_inventory(3, items::GLASS)
            .await
            .unwrap();
        walker.select_slot(3).await.unwrap();

        // Well into the eastern region.
        walker.walk_to(40.5, 0.5, 0.5).await.unwrap();
        settle(&mut walker, (2, 0)).await;

        // The eastern region knows what the player holds and lets them build there.
        let sequence = walker.use_item_on(42, -61, 0, face::TOP).await.unwrap();
        sees_block(&mut walker, (42, -60, 0), GLASS).await;
        acknowledged(&mut walker, sequence).await;
        let sequence = walker.dig(41, -61, 1).await.unwrap();
        sees_block(&mut walker, (41, -61, 1), AIR).await;
        acknowledged(&mut walker, sequence).await;

        // And back, further west than where they started.
        walker.walk_to(-20.5, 0.5, 0.5).await.unwrap();
        settle(&mut walker, (-2, 0)).await;
        let sequence = walker.use_item_on(-22, -61, 0, face::TOP).await.unwrap();
        sees_block(&mut walker, (-22, -60, 0), GLASS).await;
        acknowledged(&mut walker, sequence).await;

        // The client was never told anything that a single region would not have said.
        assert_eq!(walker.info.login.entity_id, entity_id);
        assert_eq!(walker.stats.teleports_confirmed, 1);
        assert_eq!(walker.entities.len(), 0);

        // Someone who joins now is given the world as both regions have it.
        let mut newcomer = join(&address, "Newcomer").await;
        assert_eq!(newcomer.block_at(42, -60, 0).unwrap(), GLASS);
        assert_eq!(newcomer.block_at(41, -61, 1).unwrap(), AIR);
        sees_at(&mut newcomer, "Walker", -20.5, 0.5).await;

        server.stop().await;
    }
}

/// Someone who watches a player cross the boundary sees one entity move: it is not
/// removed and shown again, and it is not there twice.
#[tokio::test]
async fn a_watcher_sees_one_entity_cross_the_boundary() {
    let (server, address) = start_with(divided()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut walker = join(&address, "Walker").await;
    sees_at(&mut watcher, "Walker", 0.5, 0.5).await;
    let entity_id = walker.info.login.entity_id;

    for (x, z) in [(40.5, 0.5), (8.5, 6.5), (24.5, -6.5), (0.5, 0.5)] {
        walker.walk_to(x, z, 0.5).await.unwrap();
        sees_at(&mut watcher, "Walker", x, z).await;
        // The bot fails if an entity it already shows is spawned again, so having got
        // here also means that never happened.
        assert_eq!(watcher.stats.entities_spawned, 1);
        assert_eq!(watcher.stats.entities_removed, 0);
        assert_eq!(
            watcher.entities.keys().copied().collect::<Vec<_>>(),
            [entity_id]
        );
    }
    // Every step was passed on, on whichever side it was taken.
    let steps = watcher.seen_player("Walker").unwrap().position_syncs;
    assert!(steps >= 150, "only {steps} moves were seen");

    // The walker saw the watcher all along as well.
    assert_eq!(walker.stats.entities_spawned, 1);
    assert_eq!(walker.stats.entities_removed, 0);

    // When the walker leaves from the other region, they disappear for the watcher.
    walker.walk_to(30.5, 0.5, 0.5).await.unwrap();
    sees_at(&mut watcher, "Walker", 30.5, 0.5).await;
    drop(walker);
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.entities.is_empty() && bot.player_list.len() == 1
        })
        .await
        .unwrap();
    assert_eq!(watcher.stats.entities_removed, 1);

    server.stop().await;
}

/// Waits until the server has handled what `bot` did with `sequence`, and checks that it
/// had said before what became of the block at `position`: a client shows its own guess
/// until then and what the server said from then on, so the block would flicker if the
/// two came the other way round.
async fn settled(bot: &mut Bot, sequence: i32, position: (i32, i32, i32), state: Option<i32>) {
    acknowledged(bot, sequence).await;
    let (x, y, z) = position;
    assert_eq!(
        bot.block_at(x, y, z).unwrap(),
        state,
        "the block at {position:?} when sequence {sequence} was acknowledged"
    );
}

/// Blocks within reach are broken and placed whichever region has them. The boundary
/// runs between x = 15 and x = 16; the builder stands two blocks west of it, then two
/// blocks east of it, and works across it in every way there is.
#[tokio::test]
async fn blocks_across_the_boundary_are_changed_like_any_other() {
    for serialise_link in [false, true] {
        let (server, address) = start_with(Config {
            serialise_link,
            ..divided()
        })
        .await;
        let mut builder = join(&address, "Builder").await;
        let mut bystander = join(&address, "Bystander").await;
        builder.walk_to(14.5, 0.5, 0.5).await.unwrap();
        // Glass.
        builder.select_slot(7).await.unwrap();

        // Breaking a block of the other region.
        let sequence = builder.dig(16, -61, 0).await.unwrap();
        settled(&mut builder, sequence, (16, -61, 0), AIR).await;
        sees_block(&mut bystander, (16, -61, 0), AIR).await;

        // Placing on the other side, against a block of the other side.
        let sequence = builder.use_item_on(16, -61, 1, face::TOP).await.unwrap();
        settled(&mut builder, sequence, (16, -60, 1), GLASS).await;
        sees_block(&mut bystander, (16, -60, 1), GLASS).await;

        // Placing on this side against that block: the other region has to say that the
        // block is there, and this one whether the spot is free.
        let sequence = builder.use_item_on(16, -60, 1, face::WEST).await.unwrap();
        settled(&mut builder, sequence, (15, -60, 1), GLASS).await;
        sees_block(&mut bystander, (15, -60, 1), GLASS).await;

        // Placing on the other side against a block of this side.
        let sequence = builder.use_item_on(15, -60, 1, face::TOP).await.unwrap();
        settled(&mut builder, sequence, (15, -59, 1), GLASS).await;
        let sequence = builder.use_item_on(15, -59, 1, face::EAST).await.unwrap();
        settled(&mut builder, sequence, (16, -59, 1), GLASS).await;
        sees_block(&mut bystander, (16, -59, 1), GLASS).await;

        // What does not work on one's own side does not work across either, and is
        // acknowledged all the same: breaking air, placing against air, placing where
        // there is a block already, and placing where someone stands.
        bystander.walk_to(17.5, 4.5, 0.5).await.unwrap();
        sees_at(&mut builder, "Bystander", 17.5, 4.5).await;
        for sequence in [
            builder.dig(16, -60, 3).await.unwrap(),
            builder.use_item_on(16, -60, 3, face::TOP).await.unwrap(),
            builder.use_item_on(16, -62, 3, face::TOP).await.unwrap(),
            builder.use_item_on(17, -61, 4, face::TOP).await.unwrap(),
        ] {
            acknowledged(&mut builder, sequence).await;
        }
        // Nor can the builder be built into from where they stand, astride the line.
        builder.walk_to(15.8, 6.5, 0.5).await.unwrap();
        let sequence = builder.use_item_on(16, -61, 6, face::TOP).await.unwrap();
        acknowledged(&mut builder, sequence).await;
        for bot in [&builder, &bystander] {
            assert_eq!(bot.block_at(16, -60, 3).unwrap(), AIR);
            assert_eq!(bot.block_at(16, -59, 3).unwrap(), AIR);
            assert_eq!(bot.block_at(16, -61, 3).unwrap(), GRASS);
            assert_eq!(bot.block_at(17, -60, 4).unwrap(), AIR);
            assert_eq!(bot.block_at(16, -60, 6).unwrap(), AIR);
        }

        // The same from the other side: the builder crosses over and works westwards.
        builder.walk_to(17.5, 9.5, 0.5).await.unwrap();
        let sequence = builder.dig(15, -61, 9).await.unwrap();
        settled(&mut builder, sequence, (15, -61, 9), AIR).await;
        let sequence = builder.use_item_on(15, -61, 10, face::TOP).await.unwrap();
        settled(&mut builder, sequence, (15, -60, 10), GLASS).await;
        let sequence = builder.use_item_on(15, -60, 10, face::EAST).await.unwrap();
        settled(&mut builder, sequence, (16, -60, 10), GLASS).await;
        sees_block(&mut bystander, (15, -61, 9), AIR).await;
        sees_block(&mut bystander, (15, -60, 10), GLASS).await;
        sees_block(&mut bystander, (16, -60, 10), GLASS).await;

        // Actions on both sides in quick succession are each acknowledged in turn, and
        // none before what it did has been said.
        let mut sequences = Vec::new();
        for z in 12..16 {
            sequences.push((builder.dig(15, -61, z).await.unwrap(), (15, -61, z)));
            sequences.push((builder.dig(16, -61, z).await.unwrap(), (16, -61, z)));
        }
        for (sequence, position) in sequences {
            settled(&mut builder, sequence, position, AIR).await;
        }

        // Someone who joins now is given all of it.
        let newcomer = join(&address, "Newcomer").await;
        for (position, state) in [
            ((16, -61, 0), AIR),
            ((16, -60, 1), GLASS),
            ((15, -60, 1), GLASS),
            ((16, -59, 1), GLASS),
            ((15, -61, 9), AIR),
            ((16, -60, 10), GLASS),
            ((15, -61, 15), AIR),
            ((16, -61, 15), AIR),
        ] {
            let (x, y, z) = position;
            assert_eq!(newcomer.block_at(x, y, z).unwrap(), state, "{position:?}");
        }

        server.stop().await;
    }
}

/// A player's connection can end at any moment of being handed over, and the player
/// can be back before the regions have sorted it out. Whenever it happens, their entity
/// disappears for those watching, and they can join again as one entity.
#[tokio::test]
async fn leaving_while_being_handed_over_leaves_nothing_behind() {
    let (server, address) = start_with(divided()).await;
    let mut watcher = join(&address, "Watcher").await;

    for round in 0..40u64 {
        let mut walker = join(&address, "Walker").await;
        // Up to the boundary, then across it and gone, a little later each round so
        // that over the rounds the connection ends before, while and after the region
        // in the west lets go.
        walker.walk_to(15.5, 0.5, 1.0).await.unwrap();
        walker.step_to(17.5, 0.5).await.unwrap();
        tokio::time::sleep(Duration::from_millis(round * 3)).await;
        drop(walker);

        // Back at once in every other round, while the old entity may still be on its
        // way between the regions.
        if round % 2 == 0 {
            let mut again = join(&address, "Walker").await;
            again.walk_to(3.5, 0.5, 1.0).await.unwrap();
            sees_at(&mut watcher, "Walker", 3.5, 0.5).await;
            assert_eq!(watcher.entities.len(), 1, "round {round}");
            drop(again);
        }
        watcher
            .wait_until(PATIENCE, |bot| bot.entities.is_empty())
            .await
            .unwrap_or_else(|error| {
                panic!("{error}: round {round} left {:?} behind", watcher.entities)
            });
        watcher
            .wait_until(PATIENCE, |bot| bot.player_list.len() == 1)
            .await
            .unwrap();
    }

    server.stop().await;
}

/// Several players walk back and forth over the boundary for a while, each at their own
/// pace, with one watching. Nobody is disconnected, the watcher sees each of them as one
/// entity throughout, and when they have left nothing remains.
#[tokio::test(flavor = "multi_thread")]
async fn a_crowd_crossing_back_and_forth_stays_whole() {
    const WALKERS: usize = 8;
    const CROSSINGS: usize = 6;
    /// Where the walkers turn around: either side of the boundary at x = 16.
    const WEST: f64 = 2.5;
    const EAST: f64 = 30.5;

    let (server, address) = start_with(Config {
        max_players: 20,
        ..divided()
    })
    .await;
    let mut watcher = join(&address, "Watcher").await;
    let lane = |number: usize| number as f64 * 2.0 - 7.5;

    // Told to leave once the watcher has looked at where everyone ended up.
    let (leave, _) = tokio::sync::watch::channel(false);
    let mut walkers = Vec::new();
    for number in 0..WALKERS {
        let address = address.clone();
        let mut leave = leave.subscribe();
        walkers.push(tokio::spawn(async move {
            let mut bot = join(&address, &format!("Walker{number}")).await;
            let z = lane(number);
            // Different speeds, so that the crossings spread over the regions' ticks.
            let speed = 0.4 + number as f64 * 0.13;
            for crossing in 0..CROSSINGS {
                let x = if crossing % 2 == 0 { EAST } else { WEST };
                bot.walk_to(x, z, speed).await.unwrap();
                // Something only the region the bot is in can do.
                let block = (x.floor() as i32, -61, z.floor() as i32);
                let sequence = bot.dig(block.0, block.1, block.2).await.unwrap();
                acknowledged(&mut bot, sequence).await;
                sees_block(&mut bot, block, AIR).await;
            }
            // A client keeps reading for as long as it is connected.
            while !*leave.borrow_and_update() {
                tokio::select! {
                    _ = leave.changed() => {}
                    idled = bot.idle(Duration::from_millis(50)) => idled.unwrap(),
                }
            }
            bot
        }));
    }

    // Everyone ends in the west, having been seen to appear once and never to vanish.
    watcher
        .wait_until(Duration::from_secs(120), |bot| {
            (0..WALKERS).all(|number| {
                bot.seen_player(&format!("Walker{number}"))
                    .is_some_and(|entity| entity.position == (WEST, GROUND, lane(number)))
            })
        })
        .await
        .unwrap_or_else(|error| {
            let seen: Vec<_> = (0..WALKERS)
                .map(|number| {
                    watcher
                        .seen_player(&format!("Walker{number}"))
                        .map(|entity| entity.position)
                })
                .collect();
            panic!("{error}: the walkers are seen at {seen:?}")
        });
    assert_eq!(watcher.stats.entities_spawned, WALKERS as u32);
    assert_eq!(watcher.stats.entities_removed, 0);
    assert_eq!(watcher.entities.len(), WALKERS);
    // Each of them broke a block per crossing, in whichever region they were.
    for number in 0..WALKERS {
        let z = lane(number).floor() as i32;
        sees_block(&mut watcher, (EAST.floor() as i32, -61, z), AIR).await;
        sees_block(&mut watcher, (WEST.floor() as i32, -61, z), AIR).await;
    }

    leave.send(true).unwrap();
    for walker in walkers {
        drop(walker.await.unwrap());
    }
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.entities.is_empty() && bot.player_list.len() == 1
        })
        .await
        .unwrap();
    assert_eq!(watcher.stats.entities_removed, WALKERS as u32);

    server.stop().await;
}

/// Breaks a block west of the boundary and places one east of it, each from its own
/// side, and waits until the server has confirmed both.
async fn build_on_both_sides(bot: &mut Bot) {
    bot.walk_to(14.5, 0.5, 0.5).await.unwrap();
    bot.dig(15, -61, 1).await.unwrap();
    bot.walk_to(17.5, 0.5, 0.5).await.unwrap();
    let last = bot.use_item_on(18, -61, 1, face::TOP).await.unwrap();
    acknowledged(bot, last).await;
    assert_built_on_both_sides(bot);
}

fn assert_built_on_both_sides(bot: &Bot) {
    assert_eq!(bot.block_at(15, -61, 1).unwrap(), AIR);
    assert_eq!(bot.block_at(18, -60, 1).unwrap(), STONE);
    // The surroundings are as generated.
    assert_eq!(bot.block_at(14, -61, 1).unwrap(), GRASS);
    assert_eq!(bot.block_at(18, -59, 1).unwrap(), AIR);
}

/// What was built in either region is there again after a restart, also when the world
/// is then divided differently or not at all.
#[tokio::test]
async fn a_divided_world_survives_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let on_disk = |boundaries: Vec<i32>| Config {
        world: Some(directory.path().to_owned()),
        pins: boundaries,
        ..config()
    };

    let (server, address) = start_with(on_disk(vec![BOUNDARY])).await;
    let mut builder = join(&address, "Builder").await;
    build_on_both_sides(&mut builder).await;
    server.stop().await;
    drop(builder);

    for boundaries in [vec![BOUNDARY], vec![], vec![-2, 0, 5]] {
        let (server, address) = start_with(on_disk(boundaries)).await;
        let visitor = join(&address, "Visitor").await;
        assert_built_on_both_sides(&visitor);
        server.stop().await;
    }
}

/// Killing the server process outright loses nothing of what either region had told
/// players: what each region committed is applied when it is opened again.
#[tokio::test]
async fn a_divided_world_survives_the_server_being_killed() {
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("world");
    let address = free_address().await;
    let divided = ["--boundaries", "1"];
    // The regions share a log, kept in segments.
    let logged = || -> u64 {
        std::fs::read_dir(world.join("log"))
            .unwrap()
            .map(|segment| segment.unwrap().metadata().unwrap().len())
            .sum()
    };

    let mut server = spawn_server(&address, &world, &divided).await;
    let mut builder = join(&address, "Builder").await;
    build_on_both_sides(&mut builder).await;
    // The changes are handed to the logs before the player is told; give the store's
    // thread a moment to write them.
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.kill().await.unwrap();
    drop(builder);

    // The chunks were still loaded and no checkpoint was due, so nothing but the log
    // holds the changes.
    assert!(!world.join("manifests/overworld").exists());
    assert!(logged() > 0);

    let mut server = spawn_server(&address, &world, &divided).await;
    let visitor = join(&address, "Visitor").await;
    assert_built_on_both_sides(&visitor);

    server.kill().await.unwrap();
}

/// The scenario that the bot tool runs against a cluster, here against one process. It
/// places and breaks blocks on both sides of a boundary and watches the walkers.
#[tokio::test(flavor = "multi_thread")]
async fn the_crossing_scenario_passes_on_a_divided_world() {
    let (server, address) = start_with(Config {
        max_players: 20,
        // Between the two places the walkers walk between.
        pins: vec![3],
        ..config()
    })
    .await;
    let crossing = Crossing {
        walkers: 5,
        rounds: 2,
        west: 20.5,
        // As far east as the watcher at the spawn point sees with the tests' view distance.
        east: 70.5,
        // Chunk 3 begins here.
        line: Some(48),
        ..Crossing::default()
    };
    let report = cross(&address, &crossing).await.unwrap();
    assert_eq!(report.crossings, 20);
    // At either end and, from either side, across the boundary.
    assert_eq!(report.blocks_built, 40);
    // Fifty blocks at up to a block per tick, twenty times.
    assert!(report.moves_seen > 1000, "{report:?}");

    // Run again on the same world, the blocks having been cleared away each time.
    let again = Crossing {
        name_prefix: "Second".to_owned(),
        ..crossing
    };
    assert_eq!(cross(&address, &again).await.unwrap().blocks_built, 40);

    server.stop().await;
}
