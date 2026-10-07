//! Login, configuration and entering the play state, end to end over TCP.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, Connection, intention};
use clustine_data::{GAME_VERSION, SYNCED_REGISTRIES, TAGS, blocks};
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::handshake::Intent;
use clustine_protocol::packets::login::{ClientboundLogin, LoginStart};
use clustine_protocol::packets::play::{ClientboundPlay, game_mode};
use uuid::Uuid;

use common::{SHORT_KEEP_ALIVE, VIEW_DISTANCE, config, start, start_with};

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
        let side = (2 * VIEW_DISTANCE + 1) as usize;
        bot.wait_for_chunks(side * side, Duration::from_secs(30))
            .await
            .unwrap();
        // Nothing beyond the view distance follows.
        bot.idle(Duration::from_millis(300)).await.unwrap();

        let expected: Vec<_> = (-VIEW_DISTANCE..=VIEW_DISTANCE)
            .flat_map(|x| (-VIEW_DISTANCE..=VIEW_DISTANCE).map(move |z| (x, z)))
            .collect();
        assert_eq!(bot.chunks.keys().copied().collect::<Vec<_>>(), expected);

        // The classic flat world: bedrock, two layers of dirt, grass blocks, then air.
        for position in [(0, 0), (-VIEW_DISTANCE, VIEW_DISTANCE)] {
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

#[tokio::test]
async fn second_connection_of_a_player_is_refused() {
    let (server, address) = start().await;

    let mut first = Bot::join(&address, "Twin").await.unwrap();
    let Err(error) = Bot::join(&address, "Twin").await else {
        panic!("the second connection was accepted");
    };
    assert!(error.to_string().contains("already connected"), "{error}");
    // The first connection is not affected.
    first.idle(Duration::from_millis(300)).await.unwrap();

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
    let (server, address) = start_with(Config {
        keep_alive_interval: SHORT_KEEP_ALIVE,
        ..config()
    })
    .await;

    let mut bot = Bot::join(&address, "Patient").await.unwrap();
    bot.idle(SHORT_KEEP_ALIVE * 10).await.unwrap();
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
            let side = (2 * VIEW_DISTANCE + 1) as usize;
            bot.wait_for_chunks(side * side, Duration::from_secs(30))
                .await?;
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
