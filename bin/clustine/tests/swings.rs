//! A player's arm swinging for another player: the swing of a block placed, and a punch
//! at air with an empty hand and with a spear; and the comparison with the official server
//! of what the other is shown (ignored, as every comparison: it needs Java, the jar and
//! agreement to the Minecraft EULA).
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test swings -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::Duration;

use clustine_botswarm::{Bot, Oracle};
use clustine_data::{entity_types, items};
use clustine_protocol::packets::play::face;

use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// How long a punch is waited for before it is sent again.
const RETRY: Duration = Duration::from_millis(300);

/// What the watcher was shown of the swinger's arm: the hand, the kind and the duration
/// of each swing, in order, and whether every one named the swinger.
#[derive(Debug, PartialEq)]
struct Seen {
    swings: Vec<(i32, i32, i32)>,
    all_the_swinger_s: bool,
}

async fn run(address: &str) -> Seen {
    let mut swinger = Bot::join(address, "Swinger").await.unwrap();
    swinger.wait_for_chunks(9, PATIENCE).await.unwrap();
    let mut watcher = Bot::join(address, "Watcher").await.unwrap();
    watcher.wait_for_chunks(9, PATIENCE).await.unwrap();
    let id = swinger.info.login.entity_id;
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.entities
                .values()
                .any(|entity| entity.kind == entity_types::PLAYER && entity.entity_id == id)
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher never saw the swinger"));
    // Stone on the ground two blocks east of the swinger, within reach: the first swing,
    // with none under way.
    let (x, z) = (
        swinger.location.0.floor() as i32 + 2,
        swinger.location.2.floor() as i32,
    );
    swinger
        .take_from_creative_inventory(6, items::STONE)
        .await
        .unwrap();
    swinger.select_slot(6).await.unwrap();
    swinger.use_item_on(x, -61, z, face::TOP).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| !bot.swings.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error}: no swing of a placing"));

    // An empty hand: Clustine gives a joining player a hotbar, the official server none.
    // Both refuse a swing while the last is not yet half done, and how many ticks pass
    // between two of a bot's packets is not fixed, so each punch is sent again until the
    // watcher is shown it.
    swinger.empty_hotbar_slot(8).await.unwrap();
    swinger.select_slot(8).await.unwrap();
    punch_until_shown(&mut swinger, &mut watcher, 2).await;
    swinger
        .take_from_creative_inventory(7, items::IRON_SPEAR)
        .await
        .unwrap();
    swinger.select_slot(7).await.unwrap();
    punch_until_shown(&mut swinger, &mut watcher, 3).await;
    Seen {
        swings: watcher
            .swings
            .iter()
            .map(|swing| (swing.hand, swing.kind, swing.duration))
            .collect(),
        all_the_swinger_s: watcher.swings.iter().all(|swing| swing.entity_id == id),
    }
}

/// Punches until the watcher has been shown `count` swings.
async fn punch_until_shown(swinger: &mut Bot, watcher: &mut Bot, count: usize) {
    for _ in 0..100 {
        swinger.punch().await.unwrap();
        if watcher
            .wait_until(RETRY, |bot| bot.swings.len() >= count)
            .await
            .is_ok()
        {
            return;
        }
    }
    panic!("no swing of a punch was shown");
}

#[tokio::test]
async fn another_player_sees_a_placing_a_punch_and_a_spear_s_stab_swing_the_arm() {
    let (_server, address) = start().await;
    let seen = run(&address).await;
    assert_eq!(
        seen,
        Seen {
            // The main hand: a whack of six ticks twice, then a stab of an iron
            // spear's nineteen.
            swings: vec![(0, 1, 6), (0, 1, 6), (0, 2, 19)],
            all_the_swinger_s: true,
        }
    );
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_swing_is_shown_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let theirs = run(oracle.address()).await;
    let ours = run(&address).await;
    server.stop().await;
    drop(oracle);
    println!("official: {theirs:?}\nClustine: {ours:?}");
    assert_eq!(theirs, ours);
}
