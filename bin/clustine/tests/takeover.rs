//! A region changes hands while people play in it: a new runner carries on from what the
//! world store has, the edge links to it and resumes, and nobody is disconnected or
//! loses anything they were shown.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::{blocks, items};
use clustine_protocol::packets::play::face;
use clustine_region::RegionId;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const GLASS: Option<i32> = Some(blocks::GLASS.0 as i32);

/// The chunk x coordinate at which the world of these tests is divided: the region in
/// the west has the spawn point, the one in the east begins at block x = 16.
const BOUNDARY: i32 = 1;
const WEST: RegionId = RegionId(0);
const EAST: RegionId = RegionId(1);

/// The height players stand at.
const GROUND: f64 = -60.0;

/// A server whose world is two regions, pinned side by side, which stay as they are.
fn divided(serialise_link: bool) -> Config {
    Config {
        pins: vec![BOUNDARY],
        follow: None,
        serialise_link,
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

/// Places glass on top of the ground at `x`, `z`, and waits until the server has handled
/// it and said that the glass is there.
async fn build(bot: &mut Bot, x: i32, z: i32) {
    let sequence = bot.use_item_on(x, -61, z, face::TOP).await.unwrap();
    sees_block(bot, (x, -60, z), GLASS).await;
    acknowledged(bot, sequence).await;
}

/// Both regions are taken over, one after the other, under a player in each and a
/// player who then walks from one into the other. Everyone stays connected as the
/// entity they were, goes on building, and sees the others where they are.
#[tokio::test]
async fn players_stay_and_go_on_playing_when_their_regions_are_taken_over() {
    for serialise_link in [false, true] {
        let (mut server, address) = start_with(divided(serialise_link)).await;
        let mut settler = join(&address, "Settler").await;
        let mut walker = join(&address, "Walker").await;
        let entities = (settler.info.login.entity_id, walker.info.login.entity_id);
        for bot in [&mut settler, &mut walker] {
            bot.take_from_creative_inventory(3, items::GLASS)
                .await
                .unwrap();
            bot.select_slot(3).await.unwrap();
        }
        build(&mut settler, 2, 2).await;
        walker.walk_to(40.5, 0.5, 0.5).await.unwrap();
        settle(&mut walker, (2, 0)).await;
        build(&mut walker, 42, 2).await;

        // The west changes hands under the settler, who notices nothing: what they
        // hold is still in their hand, and what they build is built.
        server.take_over(WEST).await.unwrap();
        build(&mut settler, 3, 2).await;
        // And the east under the walker.
        server.take_over(EAST).await.unwrap();
        build(&mut walker, 43, 2).await;
        let sequence = walker.dig(42, -60, 2).await.unwrap();
        sees_block(&mut walker, (42, -60, 2), AIR).await;
        acknowledged(&mut walker, sequence).await;

        // From one new runner into the other, and what the walker holds comes along.
        walker.walk_to(8.5, 0.5, 0.5).await.unwrap();
        settle(&mut walker, (0, 0)).await;
        build(&mut walker, 9, 2).await;
        sees_at(&mut settler, "Walker", 8.5, 0.5).await;
        sees_block(&mut settler, (9, -60, 2), GLASS).await;

        // Nobody was told anything that a world that never changed hands would not
        // have said: they are who they were, were never moved, and see each other once.
        for (bot, entity) in [(&settler, entities.0), (&walker, entities.1)] {
            assert_eq!(bot.info.login.entity_id, entity);
            assert_eq!(bot.stats.teleports_confirmed, 1);
            assert_eq!(bot.entities.len(), 1, "{:?}", bot.entities);
        }

        // Someone who joins now is given the world as it was built.
        let mut newcomer = join(&address, "Newcomer").await;
        for (position, state) in [
            ((2, -60, 2), GLASS),
            ((3, -60, 2), GLASS),
            ((9, -60, 2), GLASS),
            ((43, -60, 2), GLASS),
        ] {
            assert_eq!(
                newcomer
                    .block_at(position.0, position.1, position.2)
                    .unwrap(),
                state,
                "{position:?}"
            );
        }
        sees_at(&mut newcomer, "Walker", 8.5, 0.5).await;
        sees_at(&mut newcomer, "Settler", 0.5, 0.5).await;

        server.stop().await;
    }
}

/// A region is taken over while what a player did is still on its way: some of it the
/// old runner has made durable, some it has only applied, some it never got. The edge
/// sends again what was not reported as applied, and every action takes effect once.
#[tokio::test]
async fn nothing_a_player_did_is_lost_or_done_twice_when_the_region_is_taken_over() {
    for serialise_link in [false, true] {
        let (mut server, address) = start_with(divided(serialise_link)).await;
        let mut digger = join(&address, "Digger").await;

        // Three rounds, each with a takeover in the middle of a burst of digging.
        let mut dug = Vec::new();
        let mut last = 0;
        for round in 0..3 {
            // All within reach of where the digger stands.
            for step in 0..8 {
                let position = (step - 4, -61, round + 1);
                last = digger
                    .dig(position.0, position.1, position.2)
                    .await
                    .unwrap();
                dug.push(position);
                if step == 3 {
                    server.take_over(WEST).await.unwrap();
                }
            }
        }
        acknowledged(&mut digger, last).await;
        for position in &dug {
            sees_block(&mut digger, *position, AIR).await;
        }
        assert_eq!(digger.stats.teleports_confirmed, 1);

        // What the world store has is what the player was told.
        let newcomer = join(&address, "Newcomer").await;
        for (x, y, z) in &dug {
            assert_eq!(
                newcomer.block_at(*x, *y, *z).unwrap(),
                AIR,
                "{:?}",
                (x, y, z)
            );
        }
        server.stop().await;
    }
}

/// Regions change hands again and again while a player walks back and forth across the
/// boundary between them and another watches. The walker arrives, and the watcher
/// sees one entity where the walker is.
#[tokio::test]
async fn a_player_crossing_between_regions_that_change_hands_arrives() {
    let (mut server, address) = start_with(divided(false)).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut walker = join(&address, "Walker").await;
    let entity = walker.info.login.entity_id;

    for round in 0..3 {
        let (x, center, region) = match round % 2 {
            0 => (28.5, (1, 0), EAST),
            _ => (4.5, (0, 0), WEST),
        };
        // Half the way, a takeover of the region the walker is heading for, the rest.
        walker.walk_to(16.5, 0.5, 0.5).await.unwrap();
        server.take_over(region).await.unwrap();
        walker.walk_to(x, 0.5, 0.5).await.unwrap();
        settle(&mut walker, center).await;
        sees_at(&mut watcher, "Walker", x, 0.5).await;
        // And one of the region they have just left.
        let behind = if region == EAST { WEST } else { EAST };
        server.take_over(behind).await.unwrap();
    }

    assert_eq!(walker.info.login.entity_id, entity);
    assert_eq!(walker.stats.teleports_confirmed, 1);
    assert_eq!(watcher.entities.len(), 1, "{:?}", watcher.entities);
    // Both are still served.
    walker.idle(Duration::from_millis(200)).await.unwrap();
    watcher.idle(Duration::from_millis(200)).await.unwrap();
    server.stop().await;
}
