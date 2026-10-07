//! Players seeing each other: appearing, moving, leaving, and going out of view.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::entity_types;

use common::{config, start, start_with};

const PATIENCE: Duration = Duration::from_secs(30);

/// Where every player enters the world.
const SPAWN: (f64, f64, f64) = (0.5, -60.0, 0.5);

/// Waits until `bot` sees the player called `name` at `position`.
async fn sees_at(bot: &mut Bot, name: &str, position: (f64, f64, f64)) {
    bot.wait_until(PATIENCE, |bot| {
        bot.seen_player(name)
            .is_some_and(|entity| entity.position == position)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {name} is seen as {:?}", bot.seen_player(name)));
}

/// Waits until `bot` no longer sees the player called `name`.
async fn sees_no_more(bot: &mut Bot, name: &str) {
    bot.wait_until(PATIENCE, |bot| bot.seen_player(name).is_none())
        .await
        .unwrap_or_else(|error| panic!("{error}: {name} is still seen"));
}

/// Two players each see the other appear, over a direct and over a serialising
/// edge/worker link.
#[tokio::test]
async fn players_see_each_other() {
    for serialise_link in [false, true] {
        let (server, address) = start_with(Config {
            serialise_link,
            ..config()
        })
        .await;

        let mut alice = Bot::join(&address, "Alice").await.unwrap();
        let mut bob = Bot::join(&address, "Bob").await.unwrap();

        // The one who was there sees the newcomer, and the newcomer sees who was there.
        sees_at(&mut alice, "Bob", SPAWN).await;
        sees_at(&mut bob, "Alice", SPAWN).await;

        for (bot, own, other) in [(&alice, "Alice", "Bob"), (&bob, "Bob", "Alice")] {
            // Both are in the player list, including oneself.
            let listed: Vec<_> = bot.player_list.values().map(String::as_str).collect();
            assert_eq!(listed.len(), 2, "{own}'s list: {listed:?}");
            assert!(listed.contains(&own) && listed.contains(&other));

            // The other is a player entity with their own profile and entity id; one's
            // own entity is not sent, the client creates it itself.
            let seen = bot.seen_player(other).unwrap();
            assert_eq!(seen.kind, entity_types::PLAYER);
            assert_eq!(bot.entities.len(), 1);
            assert!(!bot.entities.contains_key(&bot.info.login.entity_id));
            assert!(bot.seen_player(own).is_none());
        }
        assert!(alice.entities.contains_key(&bob.info.login.entity_id));
        assert!(bob.entities.contains_key(&alice.info.login.entity_id));

        server.stop().await;
    }
}

#[tokio::test]
async fn a_walking_player_is_seen_moving() {
    let (server, address) = start().await;
    let mut watcher = Bot::join(&address, "Watcher").await.unwrap();
    let mut walker = Bot::join(&address, "Walker").await.unwrap();
    sees_at(&mut watcher, "Walker", SPAWN).await;

    walker.walk_to(10.5, 4.5, 0.5).await.unwrap();
    sees_at(&mut watcher, "Walker", (10.5, -60.0, 4.5)).await;
    assert!(watcher.seen_player("Walker").unwrap().position_syncs > 1);

    // The walker's own view of the watcher did not change.
    walker.idle(Duration::from_millis(200)).await.unwrap();
    let seen = walker.seen_player("Watcher").unwrap();
    assert_eq!((seen.position, seen.position_syncs), (SPAWN, 0));

    server.stop().await;
}

#[tokio::test]
async fn a_player_who_leaves_disappears() {
    let (server, address) = start().await;
    let mut stays = Bot::join(&address, "Stays").await.unwrap();
    let leaves = Bot::join(&address, "Leaves").await.unwrap();
    sees_at(&mut stays, "Leaves", SPAWN).await;

    drop(leaves);
    stays
        .wait_until(PATIENCE, |bot| {
            bot.entities.is_empty() && bot.player_list.len() == 1
        })
        .await
        .unwrap();
    assert_eq!(stays.player_list.values().collect::<Vec<_>>(), ["Stays"]);

    // Coming back gives the player a new entity.
    let mut returns = Bot::join(&address, "Leaves").await.unwrap();
    sees_at(&mut stays, "Leaves", SPAWN).await;
    sees_at(&mut returns, "Stays", SPAWN).await;

    server.stop().await;
}

/// A player is only shown while in view. The player list does not depend on distance.
#[tokio::test]
async fn players_out_of_view_are_hidden() {
    let (server, address) = start().await;
    let mut resident = Bot::join(&address, "Resident").await.unwrap();
    let mut traveller = Bot::join(&address, "Traveller").await.unwrap();
    sees_at(&mut resident, "Traveller", SPAWN).await;
    sees_at(&mut traveller, "Resident", SPAWN).await;

    // 200 blocks are more than twelve chunks, far beyond the test view distance.
    traveller.walk_to(200.5, 0.5, 2.0).await.unwrap();
    sees_no_more(&mut resident, "Traveller").await;
    sees_no_more(&mut traveller, "Resident").await;
    assert_eq!(resident.player_list.len(), 2);
    assert_eq!(traveller.player_list.len(), 2);

    // On the way back both come into each other's view again, where they are now.
    traveller.walk_to(20.5, 0.5, 2.0).await.unwrap();
    sees_at(&mut resident, "Traveller", (20.5, -60.0, 0.5)).await;
    sees_at(&mut traveller, "Resident", SPAWN).await;

    server.stop().await;
}

#[tokio::test]
async fn a_crowd_sees_each_other() {
    let (server, address) = start().await;

    let mut bots = Vec::new();
    for index in 0..12 {
        bots.push(Bot::join(&address, &format!("Crowd{index}")).await.unwrap());
    }
    for bot in &mut bots {
        bot.wait_until(PATIENCE, |bot| {
            bot.entities.len() == 11 && bot.player_list.len() == 12
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: {} entities, {} listed",
                bot.entities.len(),
                bot.player_list.len()
            )
        });
    }

    server.stop().await;
}
