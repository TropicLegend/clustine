//! A survival player digs a block by hand (`docs/groundwork/v-plan.md`, section 4.5,
//! step V2; the region's `digging.rs`): their client says START, counts on its own, and
//! says STOP when it has broken the block; the server breaks it then, tells the client
//! the block is gone, and drops the item. Another player near sees the cracks grow,
//! stage by stage, and go (`ClientboundBlockDestructionPacket`), as the game's
//! `ServerLevel.destroyBlockProgress` sends them to everyone but the breaker.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, Heard};
use clustine_data::entity_types;
use clustine_protocol::packets::play::{face, player_action};
use clustine_world::GameMode;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// Dirt under the grass of the flat world, two blocks east of where players stand: a
/// chunk of the spawn's region in both layouts.
const DIRT: (i32, i32, i32) = (2, -62, 1);

/// Joins and waits for the chunks around the spawn point.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// The stages of cracks at `at` that `bot` has heard, in order, as the bytes they came
/// as.
fn stages(bot: &Bot, at: (i32, i32, i32)) -> Vec<u8> {
    bot.heard
        .iter()
        .flatten()
        .filter_map(|(_, heard)| match heard {
            Heard::BlockDestruction {
                at: there, stage, ..
            } if *there == at => Some(*stage),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_survival_player_digs_dirt_by_hand_and_another_sees_it_crack_stage_by_stage() {
    let (server, address) = start_with(Config {
        game_mode: GameMode::Survival,
        ..config()
    })
    .await;
    let mut watcher = join(&address, "Watcher").await;
    watcher.record_heard();
    let mut digger = join(&address, "Digger").await;
    digger.record_heard();
    let breaker = watcher
        .wait_until(PATIENCE, |bot| bot.seen_player("Digger").is_some())
        .await
        .map(|()| watcher.seen_player("Digger").map(|seen| seen.entity_id))
        .unwrap()
        .expect("the watcher sees the digger");
    let dirt = digger.block_at(DIRT.0, DIRT.1, DIRT.2).unwrap();
    assert!(dirt.is_some_and(|state| state != 0), "{dirt:?}");

    let started = digger
        .destroy_step(player_action::START_DESTROY_BLOCK, DIRT, face::TOP)
        .await
        .unwrap();
    // The client counts the dig on its own and says STOP once it is through. The bot's
    // count is the watcher's: the server's stage 9, nine tenths of the block, which a
    // STOP takes as broken (it asks for seven).
    watcher
        .wait_until(PATIENCE, |bot| stages(bot, DIRT).contains(&9))
        .await
        .unwrap_or_else(|error| panic!("{error}: heard {:?}", stages(&watcher, DIRT)));
    let stopped = digger
        .destroy_step(player_action::STOP_DESTROY_BLOCK, DIRT, face::TOP)
        .await
        .unwrap();
    digger
        .wait_until(PATIENCE, |bot| {
            bot.acknowledged_sequence >= stopped
                && bot.block_at(DIRT.0, DIRT.1, DIRT.2).unwrap() == Some(0)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: acknowledged {}, the block {:?}",
                digger.acknowledged_sequence,
                digger.block_at(DIRT.0, DIRT.1, DIRT.2)
            )
        });
    assert!(started < stopped);
    // The dirt it dropped.
    digger
        .wait_until(PATIENCE, |bot| {
            bot.entities
                .values()
                .any(|entity| entity.kind == entity_types::ITEM)
        })
        .await
        .unwrap();

    // The watcher saw every stage from 0 to 9 in order, each once, and then the cracks
    // taken away (the byte of -1), all named as the digger's.
    watcher
        .wait_until(PATIENCE, |bot| stages(bot, DIRT).last() == Some(&255))
        .await
        .unwrap_or_else(|error| panic!("{error}: heard {:?}", stages(&watcher, DIRT)));
    let heard = stages(&watcher, DIRT);
    let (last, growing) = heard.split_last().expect("stages were heard");
    assert_eq!(*last, 255);
    assert_eq!(growing.first(), Some(&0), "{heard:?}");
    assert!(
        growing.windows(2).all(|pair| pair[0] < pair[1]),
        "{heard:?}"
    );
    assert!(growing.contains(&9), "{heard:?}");
    assert!(
        watcher
            .heard
            .iter()
            .flatten()
            .all(|(_, heard)| match heard {
                Heard::BlockDestruction { breaker: by, .. } => *by == breaker,
                _ => true,
            }),
        "every crack is the digger's"
    );
    // The breaker is sent none of their own cracks.
    assert_eq!(stages(&digger, DIRT), Vec::<u8>::new());
    server.stop().await;
}
