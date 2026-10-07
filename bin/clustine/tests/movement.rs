//! Walking: the server follows the player and keeps the chunks around them loaded.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Behaviour, Bot};
use clustine_protocol::packets::play::SetPlayerPosition;

use common::{VIEW_DISTANCE, config, start, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// Waits until `bot` holds exactly the chunks around `center`.
async fn settle(bot: &mut Bot, center: (i32, i32)) {
    let expected = view_area(center, VIEW_DISTANCE);
    bot.wait_until(PATIENCE, |bot| {
        bot.center == Some(center) && bot.chunks.keys().copied().eq(expected.iter().copied())
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "{error}: centre {:?}, {} chunks held",
            bot.center,
            bot.chunks.len()
        )
    });
}

/// Walking 200 blocks takes the view along: new chunks arrive and the ones left behind
/// are unloaded. Checked over a direct and over a serialising edge/worker link.
#[tokio::test]
async fn the_view_follows_a_walking_player() {
    for serialise_link in [false, true] {
        let (server, address) = start_with(Config {
            serialise_link,
            ..config()
        })
        .await;

        let mut bot = Bot::join(&address, "Walker").await.unwrap();
        settle(&mut bot, (0, 0)).await;

        // Two blocks per tick: faster than any player on foot, to keep the test short.
        bot.walk_to(200.5, 0.5, 2.0).await.unwrap();
        settle(&mut bot, (12, 0)).await;
        // Nothing else arrives once the view has caught up.
        bot.idle(Duration::from_millis(300)).await.unwrap();
        assert_eq!(bot.chunks.len(), view_area((12, 0), VIEW_DISTANCE).len());

        // And back, diagonally, into negative coordinates.
        bot.walk_to(-40.5, -40.5, 2.0).await.unwrap();
        settle(&mut bot, (-3, -3)).await;

        // The server never had to correct the bot's position.
        assert_eq!(bot.stats.teleports_confirmed, 1);
        server.stop().await;
    }
}

#[tokio::test]
async fn a_walking_player_does_not_disturb_one_that_stays() {
    let (server, address) = start().await;

    let mut resident = Bot::join(&address, "Resident").await.unwrap();
    settle(&mut resident, (0, 0)).await;

    let mut walker = Bot::join(&address, "Walker").await.unwrap();
    settle(&mut walker, (0, 0)).await;
    walker.walk_to(200.5, 0.5, 2.0).await.unwrap();
    settle(&mut walker, (12, 0)).await;
    drop(walker);

    // The chunks both had in view are still the resident's.
    resident.idle(Duration::from_millis(300)).await.unwrap();
    settle(&mut resident, (0, 0)).await;

    // Someone joining now gets them again, although the walker let go of them.
    let mut newcomer = Bot::join(&address, "Newcomer").await.unwrap();
    settle(&mut newcomer, (0, 0)).await;

    server.stop().await;
}

/// Until a client has confirmed being placed, the positions it sends are from before
/// that and must not move it.
#[tokio::test]
async fn movement_is_ignored_until_the_teleport_is_confirmed() {
    let (server, address) = start().await;

    let behaviour = Behaviour {
        confirm_teleports: false,
    };
    let mut bot = Bot::join_with(&address, "Dreamer", behaviour)
        .await
        .unwrap();
    settle(&mut bot, (0, 0)).await;

    bot.walk_to(200.5, 0.5, 2.0).await.unwrap();
    bot.idle(Duration::from_millis(300)).await.unwrap();
    assert_eq!(bot.center, Some((0, 0)), "the server followed the bot");

    // Once confirmed, movement counts. The bot is still where it walked to.
    let placed = bot.position.clone().unwrap();
    bot.confirm_teleport(&placed).await.unwrap();
    bot.walk_to(201.5, 0.5, 1.0).await.unwrap();
    settle(&mut bot, (12, 0)).await;

    server.stop().await;
}

#[tokio::test]
async fn movement_that_is_not_a_number_ends_the_connection() {
    let (server, address) = start().await;

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut connection = Bot::join(&address, "Glitch")
            .await
            .unwrap()
            .into_connection();
        let packet = SetPlayerPosition {
            x: bad,
            y: -60.0,
            z: 0.5,
            flags: 0,
        };
        connection.write(&packet).await.unwrap();
        // The server sends whatever was already on its way, then hangs up.
        let closed = tokio::time::timeout(PATIENCE, async {
            while connection.read_frame().await.is_ok() {}
        })
        .await;
        assert!(closed.is_ok(), "the connection stayed open after {bad}");

        // Wait for the server to notice, so the name is free for the next round.
        let mut rejoined = None;
        for _ in 0..100 {
            match Bot::join(&address, "Glitch").await {
                Ok(bot) => {
                    rejoined = Some(bot);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        drop(rejoined.expect("the player could not rejoin"));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    server.stop().await;
}
