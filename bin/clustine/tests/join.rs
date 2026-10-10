//! Login, configuration and entering the play state, end to end over TCP.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{
    Bot, Connection, Ending, Entry, SameName, Wire, intention, same_name_twice,
};
use clustine_data::{GAME_VERSION, SYNCED_REGISTRIES, TAGS, blocks};
use clustine_protocol::codec::Writer;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::handshake::Intent;
use clustine_protocol::packets::login::{ClientboundLogin, LoginStart};
use clustine_protocol::packets::play::{ClientboundPlay, game_mode};
use clustine_protocol::text::Text;
use uuid::Uuid;

use common::{SHORT_KEEP_ALIVE, VIEW_DISTANCE, config, start, start_with, view_area};

#[tokio::test]
async fn bot_joins_and_is_placed_in_the_world() {
    let (server, address) = start().await;

    let bot = Bot::join(&address, "Notch").await.unwrap();
    let info = &bot.info;
    // The UUID the official server derives for this name in offline mode.
    assert_eq!(
        info.profile.uuid.to_string(),
        "b50ad385-829d-3141-a216-7e7d7539ba7f"
    );
    // Compression is on by default, with the threshold of the vanilla server.
    assert_eq!(info.compression, Some(256));
    assert_eq!(info.feature_flags, ["minecraft:vanilla"]);
    assert_eq!(info.offered_packs.len(), 1);
    assert_eq!(info.offered_packs[0].version, GAME_VERSION);

    assert_eq!(info.entries_with_data, 0);
    let registries: Vec<_> = info.registries.iter().map(|(name, _)| name).collect();
    let expected: Vec<_> = SYNCED_REGISTRIES
        .iter()
        .map(|registry| registry.name)
        .collect();
    assert_eq!(registries, expected);
    assert_eq!(info.tags.len(), TAGS.len());

    assert_ne!(info.login.entity_id, 0);
    assert_eq!(info.login.game_mode, game_mode::CREATIVE);
    assert_eq!(info.login.dimension_name, "minecraft:overworld");
    assert!(!info.login.online_mode);

    let position = bot.position.as_ref().unwrap();
    assert_eq!((position.x, position.y, position.z), (0.5, -60.0, 0.5));
    assert_eq!(bot.stats.teleports_confirmed, 1);

    server.stop().await;
}

/// The chunks within the view distance arrive, over a direct link between edge and
/// worker and over one that serialises every message.
#[tokio::test]
async fn bot_receives_the_chunks_around_it() {
    for serialise_link in [false, true] {
        let (server, address) = start_with(Config {
            serialise_link,
            ..config()
        })
        .await;

        let mut bot = Bot::join(&address, "Surveyor").await.unwrap();
        let expected = view_area((0, 0), VIEW_DISTANCE);
        bot.wait_for_chunks(expected.len(), Duration::from_secs(30))
            .await
            .unwrap();
        // Nothing beyond the view follows.
        bot.idle(Duration::from_millis(300)).await.unwrap();
        assert_eq!(bot.chunks.keys().copied().collect::<Vec<_>>(), expected);
        assert_eq!(bot.center, Some((0, 0)));

        // The classic flat world: bedrock, two layers of dirt, grass blocks, then air.
        for position in [(0, 0), (-VIEW_DISTANCE - 1, VIEW_DISTANCE + 1)] {
            let sections = bot.sections(position).unwrap().unwrap();
            assert_eq!(sections.len(), 24);
            assert_eq!(sections[0].block_count, 4 * 256);
            let layer = |y: usize| sections[0].blocks.get(y << 8) as u16;
            assert_eq!(layer(0), blocks::BEDROCK.0);
            assert_eq!(layer(1), blocks::DIRT.0);
            assert_eq!(layer(2), blocks::DIRT.0);
            assert_eq!(layer(3), blocks::GRASS_BLOCK.0);
            assert_eq!(layer(4), blocks::AIR.0);
            assert!(sections[1..].iter().all(|section| section.block_count == 0));
        }

        server.stop().await;
    }
}

/// A player who logs in again while connected takes the place of their first
/// connection, as on the official server (`a_second_login_as_one_name_on_the_official_server`
/// in `oracle.rs`): the first is ended with the game's sentence for it, the second
/// enters, and someone watching is left with one entity of the player, the later one.
#[tokio::test]
async fn a_second_login_of_a_player_puts_the_first_connection_out() {
    let (server, address) = start().await;
    let mut bystander = Bot::join(&address, "Bystander").await.unwrap();

    let patience = Duration::from_secs(10);
    let SameName {
        first,
        first_end,
        second,
    } = same_name_twice(&address, "Twin", patience).await.unwrap();
    let Some(Ending::Disconnected(reason)) = first_end else {
        panic!("the first connection was not ended with a disconnect packet: {first_end:?}");
    };
    // The reason as the official server sends it: a compound with the one key.
    let sentence = Text::translatable("multiplayer.disconnect.duplicate_login").to_nbt();
    let mut bytes = Writer::new();
    bytes.put_nbt(&sentence);
    let expected = Wire::Nbt {
        bytes: bytes.into_bytes(),
        value: sentence,
    };
    assert_eq!(reason.wire, expected, "{reason}");
    let Ok(Entry::Entered(mut second)) = second else {
        panic!("the second connection did not enter the world");
    };
    let entity = second.info.login.entity_id;
    assert!(entity > first.info.login.entity_id);
    second.wait_until(patience, Bot::is_loaded).await.unwrap();
    bystander
        .wait_until(patience, |bot| {
            bot.entities.len() == 1 && bot.entities.contains_key(&entity)
        })
        .await
        .unwrap();
    assert!(bystander.seen_player("Twin").is_some());

    server.stop().await;
}

#[tokio::test]
async fn player_can_rejoin_after_leaving() {
    let (server, address) = start().await;

    let first = Bot::join(&address, "Returner").await.unwrap();
    let first_entity_id = first.info.login.entity_id;
    drop(first);

    // The server notices the closed connection a moment later.
    let mut second = None;
    for _ in 0..100 {
        match Bot::join(&address, "Returner").await {
            Ok(bot) => {
                second = Some(bot);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let mut second = second.expect("rejoining never succeeded");
    assert_ne!(second.info.login.entity_id, first_entity_id);
    second
        .wait_for_chunks(1, Duration::from_secs(30))
        .await
        .unwrap();

    server.stop().await;
}

#[tokio::test]
async fn players_get_distinct_entity_ids() {
    let (server, address) = start().await;

    let first = Bot::join(&address, "First").await.unwrap();
    let second = Bot::join(&address, "Second").await.unwrap();
    assert_ne!(first.info.login.entity_id, second.info.login.entity_id);
    assert_ne!(first.info.profile.uuid, second.info.profile.uuid);

    server.stop().await;
}

#[tokio::test]
async fn bot_that_answers_keep_alives_stays_connected() {
    // A client has until the next keep-alive to answer, so the interval is the most the
    // test may stall at any moment. The short one of the other tests was once exceeded
    // on a busy runner, which disconnected a bot that did nothing wrong.
    let interval = Duration::from_millis(500);
    let (server, address) = start_with(Config {
        keep_alive_interval: interval,
        ..config()
    })
    .await;

    let mut bot = Bot::join(&address, "Patient").await.unwrap();
    bot.idle(interval * 10).await.unwrap();
    assert!(
        bot.stats.keep_alives_answered >= 5,
        "{}",
        bot.stats.keep_alives_answered
    );

    server.stop().await;
}

#[tokio::test]
async fn silent_client_is_timed_out() {
    let (server, address) = start_with(Config {
        keep_alive_interval: SHORT_KEEP_ALIVE,
        ..config()
    })
    .await;

    // Join properly, then stop answering.
    let mut connection = Bot::join(&address, "Silent")
        .await
        .unwrap()
        .into_connection();
    let reason = loop {
        let frame = connection.read_frame().await.unwrap();
        if let ClientboundPlay::Disconnect(disconnect) = ClientboundPlay::decode(&frame).unwrap() {
            break disconnect.reason;
        }
    };
    assert_eq!(reason, Nbt::String("Timed out".to_owned()));
    assert!(
        connection.read_frame().await.is_err(),
        "connection is closed"
    );

    server.stop().await;
}

#[tokio::test]
async fn invalid_name_is_refused_with_a_message() {
    let (server, address) = start().await;

    let mut connection = Connection::connect(&address).await.unwrap();
    let handshake = intention(&address, Intent::Login).unwrap();
    connection.write(&handshake).await.unwrap();
    let login = LoginStart {
        name: "two words".to_owned(),
        uuid: Uuid::nil(),
    };
    connection.write(&login).await.unwrap();
    let frame = connection.read_frame().await.unwrap();
    let ClientboundLogin::LoginDisconnect(disconnect) = ClientboundLogin::decode(&frame).unwrap()
    else {
        panic!("expected a login disconnect");
    };
    assert!(disconnect.reason_json.contains("Invalid player name"));

    server.stop().await;
}

#[tokio::test]
async fn many_bots_join_at_once() {
    let (server, address) = start().await;

    let joins = (0..50).map(|index| {
        let address = address.clone();
        tokio::spawn(async move {
            let mut bot = Bot::join(&address, &format!("Bot{index}")).await?;
            let count = view_area((0, 0), VIEW_DISTANCE).len();
            bot.wait_for_chunks(count, Duration::from_secs(30)).await?;
            anyhow::Ok(bot.info.login.entity_id)
        })
    });
    let mut entity_ids = Vec::new();
    for join in joins.collect::<Vec<_>>() {
        entity_ids.push(join.await.unwrap().unwrap());
    }
    entity_ids.sort_unstable();
    entity_ids.dedup();
    assert_eq!(entity_ids.len(), 50);

    server.stop().await;
}

/// With compression turned off the client is not told to compress, and everything
/// still arrives, including chunks, which are the packets compression matters for.
#[tokio::test]
async fn joining_works_without_compression() {
    let (server, address) = start_with(Config {
        compression_threshold: None,
        ..config()
    })
    .await;

    let mut bot = Bot::join(&address, "Plain").await.unwrap();
    assert_eq!(bot.info.compression, None);
    let expected = view_area((0, 0), VIEW_DISTANCE);
    bot.wait_for_chunks(expected.len(), Duration::from_secs(30))
        .await
        .unwrap();
    let sections = bot.sections((0, 0)).unwrap().unwrap();
    assert_eq!(sections[0].block_count, 4 * 256);

    server.stop().await;
}

/// A threshold of zero compresses every packet, however small.
#[tokio::test]
async fn joining_works_with_everything_compressed() {
    let (server, address) = start_with(Config {
        compression_threshold: Some(0),
        ..config()
    })
    .await;

    let mut bot = Bot::join(&address, "Squeezed").await.unwrap();
    assert_eq!(bot.info.compression, Some(0));
    bot.wait_for_chunks(1, Duration::from_secs(30))
        .await
        .unwrap();
    bot.walk_to(5.5, 0.5, 0.5).await.unwrap();

    server.stop().await;
}
