//! A player's game mode (`docs/groundwork/v-plan.md`, section 4.1, step V1): a newcomer
//! is given the world's (`--gamemode`), keeps it across leaving whatever the world's
//! becomes, and is given the world's again where it is forced (`--force-gamemode`). The
//! client is told it in the login, with the abilities it gives, and a survival newcomer
//! starts with nothing, as the game's does. What a survival client asks of creative, a
//! stack out of the creative inventory and flying, is refused.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::items;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{abilities, game_mode};
use clustine_world::GameMode;

use common::{config, start_with};

const PATIENCE: Duration = Duration::from_secs(30);

/// The ability flags the login's first abilities packet told `bot`.
fn first_abilities(bot: &Bot) -> u8 {
    bot.abilities
        .first()
        .map(|packet| packet.flags)
        .expect("a bot that joined was told its abilities")
}

fn in_mode(mode: GameMode) -> Config {
    Config {
        game_mode: mode,
        ..config()
    }
}

#[tokio::test]
async fn a_newcomer_to_a_survival_world_is_told_survival_may_not_fly_and_starts_with_nothing() {
    let (server, address) = start_with(in_mode(GameMode::Survival)).await;
    let mut bot = Bot::join(&address, "Newcomer").await.unwrap();
    assert_eq!(bot.info.login.spawn.game_mode, game_mode::SURVIVAL);
    assert_eq!(bot.info.login.spawn.previous_game_mode, None);
    assert_eq!(first_abilities(&bot), 0, "{:?}", bot.abilities);
    bot.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    assert!(
        bot.hotbar.iter().all(Option::is_none),
        "a survival newcomer starts empty: {:?}",
        bot.hotbar
    );
    server.stop().await;
}

#[tokio::test]
async fn a_newcomer_to_a_creative_world_may_fly_and_build_at_once_and_is_given_the_hotbar() {
    let (server, address) = start_with(in_mode(GameMode::Creative)).await;
    let mut bot = Bot::join(&address, "Builder").await.unwrap();
    assert_eq!(bot.info.login.spawn.game_mode, game_mode::CREATIVE);
    assert_eq!(
        first_abilities(&bot),
        abilities::INVULNERABLE | abilities::MAY_FLY | abilities::INSTANT_BREAK,
    );
    bot.wait_until(PATIENCE, |bot| bot.hotbar.iter().any(Option::is_some))
        .await
        .unwrap();
    server.stop().await;
}

/// A player's mode is theirs: kept across leaving, also when the world's default has
/// changed since, and replaced by the default only where it is forced, which keeps the
/// one before it for the client's switcher.
#[tokio::test]
async fn a_player_keeps_their_mode_across_leaving_unless_the_world_s_is_forced() {
    let directory = tempfile::tempdir().unwrap();
    let on_disk = |mode: GameMode, forced: bool| Config {
        world: Some(directory.path().to_owned()),
        game_mode: mode,
        force_game_mode: forced,
        ..config()
    };

    let (server, address) = start_with(on_disk(GameMode::Survival, false)).await;
    let mut walker = Bot::join(&address, "Walker").await.unwrap();
    walker.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    assert_eq!(walker.info.login.spawn.game_mode, game_mode::SURVIVAL);
    server.stop().await;
    drop(walker);

    // The world's default is creative now: the walker is still in survival, and a
    // newcomer is in creative.
    let (server, address) = start_with(on_disk(GameMode::Creative, false)).await;
    let mut walker = Bot::join(&address, "Walker").await.unwrap();
    walker.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    assert_eq!(walker.info.login.spawn.game_mode, game_mode::SURVIVAL);
    assert_eq!(first_abilities(&walker), 0);
    let newcomer = Bot::join(&address, "Newcomer").await.unwrap();
    assert_eq!(newcomer.info.login.spawn.game_mode, game_mode::CREATIVE);
    server.stop().await;
    drop((walker, newcomer));

    // Forced: the walker is given creative, with survival before it.
    let (server, address) = start_with(on_disk(GameMode::Creative, true)).await;
    let walker = Bot::join(&address, "Walker").await.unwrap();
    assert_eq!(walker.info.login.spawn.game_mode, game_mode::CREATIVE);
    assert_eq!(
        walker.info.login.spawn.previous_game_mode,
        Some(game_mode::SURVIVAL)
    );
    assert_eq!(
        first_abilities(&walker),
        abilities::INVULNERABLE | abilities::MAY_FLY | abilities::INSTANT_BREAK,
    );
    server.stop().await;
}

/// What a survival player's client asks of creative is refused, by the edge and by the
/// region (`handleSetCreativeModeSlot @16`, `handlePlayerAbilities @13-44`): a stack
/// taken from the creative inventory is not theirs and their flying is not kept, which
/// the place they leave with shows when they come back. A creative player's are kept.
#[tokio::test]
async fn a_survival_player_s_creative_slot_and_flying_are_refused_and_a_creative_player_s_kept() {
    let directory = tempfile::tempdir().unwrap();
    let on_disk = |mode: GameMode, forced: bool| Config {
        world: Some(directory.path().to_owned()),
        game_mode: mode,
        force_game_mode: forced,
        ..config()
    };
    let stone = || ItemStack::new(items::STONE, 1);

    // In survival: a stone into the hotbar, and flying, both kept as far as the region
    // goes before the server stops.
    let (server, address) = start_with(on_disk(GameMode::Survival, false)).await;
    let mut walker = Bot::join(&address, "Walker").await.unwrap();
    walker.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    walker.put_in_hotbar(0, stone()).await.unwrap();
    walker.set_flying(true).await.unwrap();
    walker.stand_until_kept(PATIENCE).await.unwrap();
    server.stop().await;
    drop(walker);

    // Made creative by force, which keeps a creative flyer flying: they come back with
    // neither, and now take the stone and fly.
    let (server, address) = start_with(on_disk(GameMode::Creative, true)).await;
    let mut walker = Bot::join(&address, "Walker").await.unwrap();
    assert_eq!(walker.info.login.spawn.game_mode, game_mode::CREATIVE);
    let kept = walker.entered(PATIENCE).await.unwrap();
    assert!(!kept.flying, "the survival player's flying was kept");
    assert_eq!(
        kept.hotbar[0], None,
        "the survival player's creative slot was kept"
    );
    walker.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    walker.put_in_hotbar(0, stone()).await.unwrap();
    walker.set_flying(true).await.unwrap();
    walker.stand_until_kept(PATIENCE).await.unwrap();
    server.stop().await;
    drop(walker);

    let (server, address) = start_with(on_disk(GameMode::Creative, false)).await;
    let mut walker = Bot::join(&address, "Walker").await.unwrap();
    let kept = walker.entered(PATIENCE).await.unwrap();
    assert!(kept.flying);
    assert_eq!(kept.hotbar[0], Some(stone()));
    server.stop().await;
}
