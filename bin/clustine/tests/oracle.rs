//! Comparisons against the official Minecraft server.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test --workspace -- --ignored
//! ```

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use clustine_botswarm::{Bot, Oracle};

use clustine::Config;

use common::{config, start, start_with, view_area};

/// What a joining client is told must agree with the official server, apart from the
/// numeric ids, which each server assigns through the order of its registry entries.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn join_matches_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;

    let mut vanilla = Bot::join(oracle.address(), "Notch").await.unwrap();
    let mut clustine = Bot::join(&address, "Notch").await.unwrap();
    let (theirs, ours) = (&vanilla.info, &clustine.info);

    assert_eq!(ours.profile.uuid, theirs.profile.uuid);
    assert_eq!(ours.profile.name, theirs.profile.name);
    assert_eq!(ours.feature_flags, theirs.feature_flags);
    assert_eq!(ours.offered_packs, theirs.offered_packs);
    assert_eq!(
        theirs.entries_with_data, 0,
        "the oracle relied on the core pack"
    );
    assert_eq!(ours.entries_with_data, 0);

    // The same registries in the same order, each with the same set of entries.
    let names = |registries: &[(String, Vec<String>)]| -> Vec<String> {
        registries.iter().map(|(name, _)| name.clone()).collect()
    };
    assert_eq!(names(&ours.registries), names(&theirs.registries));
    for ((registry, our_entries), (_, their_entries)) in
        ours.registries.iter().zip(&theirs.registries)
    {
        let our_entries: BTreeSet<_> = our_entries.iter().collect();
        let their_entries: BTreeSet<_> = their_entries.iter().collect();
        assert_eq!(our_entries, their_entries, "entries of {registry}");
    }

    // The same tags with the same members.
    let our_tags = ours.tags_by_name();
    let their_tags = theirs.tags_by_name();
    assert_eq!(
        our_tags.keys().collect::<Vec<_>>(),
        their_tags.keys().collect::<Vec<_>>()
    );
    for (registry, tags) in &their_tags {
        assert_eq!(&our_tags[registry], tags, "tags of {registry}");
    }

    // The same world as far as the join packet describes it.
    let (our_login, their_login) = (&ours.login, &theirs.login);
    assert_eq!(our_login.dimension_names, their_login.dimension_names[..1]);
    assert_eq!(our_login.dimension_name, their_login.dimension_name);
    assert_eq!(our_login.game_mode, their_login.game_mode);
    assert_eq!(our_login.is_flat, their_login.is_flat);
    assert_eq!(our_login.sea_level, their_login.sea_level);
    assert_eq!(our_login.online_mode, their_login.online_mode);
    assert_eq!(our_login.hardcore, their_login.hardcore);
    let dimension_type = |info: &clustine_botswarm::JoinInfo| {
        let (_, types) = info
            .registries
            .iter()
            .find(|(name, _)| name == "minecraft:dimension_type")
            .unwrap();
        types[info.login.dimension_type as usize].clone()
    };
    assert_eq!(dimension_type(ours), dimension_type(theirs));

    // Both keep a well-behaved client connected.
    vanilla.idle(Duration::from_secs(2)).await.unwrap();
    clustine.idle(Duration::from_secs(2)).await.unwrap();

    server.stop().await;
}

/// Walking moves the view the same way on both servers: the same centre chunk and the
/// same set of chunks around it, with the ones left behind unloaded.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn walking_matches_the_official_server() {
    // The bots ask for a view distance of 8, which both servers have to grant.
    const VIEW_DISTANCE: i32 = 8;
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start_with(Config {
        view_distance: VIEW_DISTANCE,
        ..config()
    })
    .await;

    for address in [oracle.address(), address.as_str()] {
        let mut bot = Bot::join(address, "Walker").await.unwrap();
        let (x, _, z) = bot.location;
        let chunk_of = |x: f64, z: f64| ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4);

        let start = chunk_of(x, z);
        let expected = view_area(start, VIEW_DISTANCE);
        bot.wait_until(Duration::from_secs(60), |bot| {
            bot.chunks.keys().copied().eq(expected.iter().copied())
        })
        .await
        .unwrap_or_else(|error| panic!("{address} at the start: {error}"));
        assert_eq!(bot.center, Some(start), "{address}");

        // One block per tick: fast, but within what the official server accepts.
        bot.walk_to(x + 100.0, z - 60.0, 1.0).await.unwrap();
        let end = chunk_of(x + 100.0, z - 60.0);
        let expected = view_area(end, VIEW_DISTANCE);
        bot.wait_until(Duration::from_secs(60), |bot| {
            bot.center == Some(end) && bot.chunks.keys().copied().eq(expected.iter().copied())
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{address} after walking: {error}: centre {:?}, {} chunks",
                bot.center,
                bot.chunks.len()
            )
        });
        // Neither server had to put the bot back.
        assert_eq!(bot.stats.teleports_confirmed, 1, "{address}");
    }

    server.stop().await;
}

/// Two players see each other appear and disappear the same way on both servers.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn players_appear_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let patience = Duration::from_secs(30);

    for address in [oracle.address(), address.as_str()] {
        let mut alice = Bot::join(address, "Alice").await.unwrap();
        let mut bob = Bot::join(address, "Bob").await.unwrap();

        for (watcher, name) in [(&mut alice, "Bob"), (&mut bob, "Alice")] {
            watcher
                .wait_until(patience, |bot| bot.seen_player(name).is_some())
                .await
                .unwrap_or_else(|error| panic!("{address}: {name} never appeared: {error}"));
        }
        for (watcher, other) in [(&alice, &bob), (&bob, &alice)] {
            let name = &other.info.profile.name;
            let seen = watcher.seen_player(name).unwrap();
            // The entity is the other player: their profile, their entity id, a player.
            assert_eq!(seen.uuid, other.info.profile.uuid, "{address}");
            assert_eq!(seen.kind, clustine_data::entity_types::PLAYER, "{address}");
            assert!(
                watcher.entities.contains_key(&other.info.login.entity_id),
                "{address}"
            );
            // It appears where the other player was placed.
            let (x, y, z) = seen.position;
            let (ox, oy, oz) = other.location;
            assert!(
                (x - ox).abs() < 0.01 && (y - oy).abs() < 0.01 && (z - oz).abs() < 0.01,
                "{address}: {name} seen at {:?} but placed at {:?}",
                seen.position,
                other.location
            );
            // Both players are in the list, and one's own entity is never sent.
            assert_eq!(watcher.player_list.len(), 2, "{address}");
            assert!(
                !watcher.entities.contains_key(&watcher.info.login.entity_id),
                "{address}"
            );
        }

        drop(bob);
        alice
            .wait_until(patience, |bot| {
                bot.seen_player("Bob").is_none() && bot.player_list.len() == 1
            })
            .await
            .unwrap_or_else(|error| panic!("{address}: Bob never disappeared: {error}"));
    }

    server.stop().await;
}

/// Breaking a block in creative mode has the same visible effect on both servers: the
/// one who broke it gets an acknowledgement, and the block is air for them, for a
/// bystander and for someone who joins afterwards.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn breaking_a_block_matches_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let patience = Duration::from_secs(30);
    let air = Some(i32::from(clustine_data::blocks::AIR.0));
    let grass = Some(i32::from(clustine_data::blocks::GRASS_BLOCK.0));

    for address in [oracle.address(), address.as_str()] {
        let mut digger = Bot::join(address, "Digger").await.unwrap();
        let mut bystander = Bot::join(address, "Bystander").await.unwrap();
        // The grass block next to the one the digger stands on.
        let (x, y, z) = (
            digger.location.0.floor() as i32 + 1,
            digger.location.1.floor() as i32 - 1,
            digger.location.2.floor() as i32,
        );
        for bot in [&mut digger, &mut bystander] {
            bot.wait_until(patience, |bot| bot.block_at(x, y, z).unwrap() == grass)
                .await
                .unwrap_or_else(|error| panic!("{address}: no grass at the start: {error}"));
        }

        let sequence = digger.dig(x, y, z).await.unwrap();
        digger
            .wait_until(patience, |bot| bot.acknowledged_sequence == sequence)
            .await
            .unwrap_or_else(|error| panic!("{address}: not acknowledged: {error}"));
        for bot in [&mut digger, &mut bystander] {
            bot.wait_until(patience, |bot| bot.block_at(x, y, z).unwrap() == air)
                .await
                .unwrap_or_else(|error| panic!("{address}: the block is still there: {error}"));
        }
        assert_eq!(bystander.acknowledged_sequence, 0, "{address}");

        let mut latecomer = Bot::join(address, "Latecomer").await.unwrap();
        latecomer
            .wait_until(patience, |bot| bot.block_at(x, y, z).unwrap() == air)
            .await
            .unwrap_or_else(|error| panic!("{address}: the latecomer sees the block: {error}"));
    }

    server.stop().await;
}

/// Placing a block in creative mode has the same visible effect on both servers. The
/// bot takes the block from the creative inventory, since a player's starting items
/// are each server's own choice.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn placing_a_block_matches_the_official_server() {
    use clustine_data::{blocks, items};
    use clustine_protocol::packets::play::face;

    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let patience = Duration::from_secs(30);
    let air = Some(i32::from(blocks::AIR.0));
    let bricks = Some(i32::from(blocks::BRICKS.0));

    for address in [oracle.address(), address.as_str()] {
        let mut builder = Bot::join(address, "Builder").await.unwrap();
        let mut bystander = Bot::join(address, "Bystander").await.unwrap();
        // The ground block two steps east of the builder, and the space above it.
        let (x, ground, z) = (
            builder.location.0.floor() as i32 + 2,
            builder.location.1.floor() as i32 - 1,
            builder.location.2.floor() as i32,
        );
        for bot in [&mut builder, &mut bystander] {
            bot.wait_until(patience, |bot| {
                bot.block_at(x, ground + 1, z).unwrap() == air
            })
            .await
            .unwrap_or_else(|error| panic!("{address}: no chunk at the start: {error}"));
        }

        builder
            .take_from_creative_inventory(4, items::BRICKS)
            .await
            .unwrap();
        builder.select_slot(4).await.unwrap();
        let sequence = builder.use_item_on(x, ground, z, face::TOP).await.unwrap();
        builder
            .wait_until(patience, |bot| bot.acknowledged_sequence == sequence)
            .await
            .unwrap_or_else(|error| panic!("{address}: not acknowledged: {error}"));
        for bot in [&mut builder, &mut bystander] {
            bot.wait_until(patience, |bot| {
                bot.block_at(x, ground + 1, z).unwrap() == bricks
            })
            .await
            .unwrap_or_else(|error| panic!("{address}: no bricks appeared: {error}"));
        }

        let mut latecomer = Bot::join(address, "Latecomer").await.unwrap();
        latecomer
            .wait_until(patience, |bot| {
                bot.block_at(x, ground + 1, z).unwrap() == bricks
            })
            .await
            .unwrap_or_else(|error| panic!("{address}: the latecomer sees no bricks: {error}"));
    }

    server.stop().await;
}
