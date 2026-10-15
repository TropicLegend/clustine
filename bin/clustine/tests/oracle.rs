//! Comparisons against the official Minecraft server.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test --workspace -- --ignored
//! ```
//!
//! The first tests compare Clustine with the official server. The last three ask the
//! official server alone what Clustine is going to do like it (ADR-0020), and print
//! their answers: see the note above them.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use clustine_botswarm::{
    Bot, ChunkLight, Ending, Entry, Oracle, SameName, SectionLight as SentLight, Wire,
    same_name_twice,
};
use clustine_protocol::packets::Packet;
use clustine_protocol::packets::play::{ServerboundPlayerAbilities, abilities};
use clustine_protocol::text::Text;
use tokio::time::Instant;

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

    // Both tell the client where the world's spawn is, which a compass points to: in
    // the flat world, the first column of the chunk at the origin, by the game's rule.
    let told = |bot: &Bot| bot.world_spawn.clone().expect("the world's spawn was told");
    assert_eq!(told(&clustine), told(&vanilla));

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

// What follows asks the official server alone, for ADR-0020 (step R1.0c). Clustine does
// none of it yet, and the record names, under "If a guess is wrong", what it has guessed
// about the official server. Each test prints all it saw and then asserts the guess and
// nothing else, so that its output answers the question also where the guess is wrong.
// Run one with its output shown:
//
//   CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked --test oracle \
//       -- --ignored --nocapture --test-threads 1 <name of the test>

/// How long a state on the official server is waited for before the test says that it
/// was not reached and goes on.
const PATIENCE: Duration = Duration::from_secs(10);

/// The key of the sentence the record guesses a first connection is given when its
/// player logs in again (ADR-0020, section 6).
const DUPLICATE_LOGIN: &str = "multiplayer.disconnect.duplicate_login";

/// Waits until `done` holds for the first of `bots`, attending to all of them
/// meanwhile, so that none is given up by the server for not answering. Says whether
/// the state was reached; not reaching it is an answer here, and never a panic.
async fn reached(bots: &mut [&mut Bot], what: &str, done: impl FnMut(&Bot) -> bool) -> bool {
    let reached = waited(bots, what, done).await;
    if reached {
        println!("  reached: {what}");
    } else {
        println!("  NOT REACHED within {PATIENCE:?}: {what}");
    }
    reached
}

/// As [`reached`], without a line for the state: for a test that waits some hundred
/// times and says itself what became of each wait.
async fn waited(bots: &mut [&mut Bot], what: &str, mut done: impl FnMut(&Bot) -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if done(&*bots[0]) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        for bot in bots.iter_mut() {
            if bot.ended.is_some() {
                continue;
            }
            if let Err(error) = bot.idle(Duration::from_millis(20)).await {
                println!(
                    "  {} stopped while waiting ({what}): {error:#}",
                    bot.info.profile.name
                );
            }
        }
    }
}

/// The packets that came between two readings of a bot's counts, by name.
fn arrived_since(before: &BTreeMap<&'static str, u32>, bot: &Bot) -> String {
    let news: Vec<String> = bot
        .stats
        .received
        .iter()
        .filter_map(|(name, count)| {
            let more = count - before.get(name).copied().unwrap_or(0);
            (more > 0).then(|| format!("{} x{more}", name.trim_start_matches("minecraft:")))
        })
        .collect();
    if news.is_empty() {
        "nothing".to_owned()
    } else {
        news.join(", ")
    }
}

fn near(a: (f64, f64, f64), b: (f64, f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01 && (a.2 - b.2).abs() < 0.01
}

/// A second login as a name that is connected: the record guesses that the official
/// server puts the first connection out with a disconnect packet whose reason is the
/// compound `{translate: "multiplayer.disconnect.duplicate_login"}`, and lets the
/// second in.
///
/// In the output: "FIRST CONNECTION" says whether it was ended and with which reason
/// (as read, as NBT and as bytes); "SECOND CONNECTION" whether it entered; "BYSTANDER"
/// what a third client was shown of the two.
///
/// Clustine's counterpart is `a_second_login_of_a_player_puts_the_first_connection_out`
/// in `join.rs`, which holds Clustine to the same reason, byte for byte.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_second_login_as_one_name_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let mut bystander = Bot::join(oracle.address(), "Bystander").await.unwrap();
    let SameName {
        first,
        first_end,
        second,
    } = same_name_twice(oracle.address(), "Twin", PATIENCE)
        .await
        .unwrap();

    println!("\n== A second login as one name, on the official server ==");
    match &first_end {
        Some(ending) => println!("FIRST CONNECTION: ended with {ending}"),
        None => println!("FIRST CONNECTION: NOT ENDED within {PATIENCE:?} of the second login"),
    }
    println!("{}", first.report());
    let mut second = match second {
        Ok(Entry::Entered(bot)) => {
            println!("SECOND CONNECTION: ENTERED the world");
            Some(*bot)
        }
        Ok(Entry::Refused { state, reason }) => {
            println!("SECOND CONNECTION: REFUSED {state} with {reason}");
            None
        }
        Err(error) => {
            println!("SECOND CONNECTION: FAILED without a disconnect packet: {error:#}");
            None
        }
    };
    if let Some(second) = &mut second {
        let entity = second.info.login.entity_id;
        reached(
            &mut [&mut *second, &mut bystander],
            "the second connection has its chunk and left the loading screen",
            Bot::is_loaded,
        )
        .await;
        reached(
            &mut [&mut bystander, &mut *second],
            "a bystander sees the player as the second connection's entity and no other",
            |bot| {
                bot.entities.len() == 1
                    && bot.entities.contains_key(&entity)
                    && bot.seen_player("Twin").is_some()
            },
        )
        .await;
        println!("{}", second.report());
        println!(
            "ENTITY IDS: the first connection's was {}, the second's is {entity}",
            first.info.login.entity_id
        );
    }
    println!(
        "BYSTANDER: entities appeared {}, vanished {}; now sees {:?}; saw vanish {:?}; list {:?}",
        bystander.stats.entities_spawned,
        bystander.stats.entities_removed,
        bystander.entities,
        bystander.vanished,
        bystander.player_list
    );
    println!("{}", bystander.report());

    // The record's three guesses, and nothing else.
    let Some(Ending::Disconnected(reason)) = first_end else {
        panic!("GUESS WRONG: the first connection was not given a disconnect packet");
    };
    assert_eq!(
        reason.wire,
        Wire::Nbt {
            bytes: {
                let mut writer = clustine_protocol::codec::Writer::new();
                writer.put_nbt(&Text::translatable(DUPLICATE_LOGIN).to_nbt());
                writer.into_bytes()
            },
            value: Text::translatable(DUPLICATE_LOGIN).to_nbt(),
        },
        "GUESS WRONG: the reason is not the compound with the one key"
    );
    assert!(
        second.is_some(),
        "GUESS WRONG: the second login did not enter"
    );
}

/// The serverbound abilities packet: the record guesses that it is id 40 with one byte
/// of flags of which `0x02` is flying, and that nobody else is shown that a player
/// flies.
///
/// A server that could not read the packet would end the connection, and one that
/// reads it answers nothing. So the bot breaks a block right after each packet: the
/// acknowledgement shows that the server read on past the abilities packet. Three
/// rounds: a block alone (the control), flying begun and a block, flying stopped and a
/// block. In the output, "the one who flies was sent" and "the onlooker was sent" list
/// what came in each round; anything in the second and third that the control lacks is
/// what flying shows. That the bit is `0x02` this test cannot tell (a server takes any
/// byte); `entering_again_after_leaving_in_flight_on_the_official_server` does.
///
/// Clustine's edge reads the packet and tells the player's region whether they fly
/// (`services/edge/src/play.rs`); that a player who left in flight enters flying is
/// held to the official server once a player's place is kept by the store (step R1.5
/// of `docs/adr/0020-one-stay-per-player.md`).
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_abilities_packet_is_taken_by_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let mut aviator = Bot::join(oracle.address(), "Aviator").await.unwrap();
    let mut onlooker = Bot::join(oracle.address(), "Onlooker").await.unwrap();
    let air = i32::from(clustine_data::blocks::AIR.0);
    let (x, ground, z) = (
        aviator.location.0.floor() as i32,
        aviator.location.1.floor() as i32 - 1,
        aviator.location.2.floor() as i32,
    );

    println!("\n== The serverbound abilities packet, on the official server ==");
    println!(
        "the packet is id {} with the byte {:#04x} to begin flying and {:#04x} to stop",
        ServerboundPlayerAbilities::ID,
        ServerboundPlayerAbilities::flying(true).flags,
        ServerboundPlayerAbilities::flying(false).flags
    );
    reached(
        &mut [&mut aviator, &mut onlooker],
        "the one who flies has left the loading screen",
        Bot::is_loaded,
    )
    .await;
    reached(
        &mut [&mut onlooker, &mut aviator],
        "the onlooker has left the loading screen, sees the other and has the ground",
        |bot| {
            bot.is_loaded()
                && bot.seen_player("Aviator").is_some()
                && matches!(bot.block_at(x + 3, ground, z), Ok(Some(state)) if state != air)
        },
    )
    .await;
    println!("BEFORE:\n{}", aviator.report());

    let rounds = [
        ("a block broken and no abilities packet (the control)", None),
        ("flying begun (0x02), then a block broken", Some(true)),
        ("flying stopped (0x00), then a block broken", Some(false)),
    ];
    let mut taken = Vec::new();
    for (round, (what, flying)) in rounds.into_iter().enumerate() {
        println!("ROUND {}: {what}", round + 1);
        let before = (
            aviator.stats.received.clone(),
            onlooker.stats.received.clone(),
        );
        if let Some(flying) = flying {
            aviator.set_flying(flying).await.unwrap();
        }
        let block = x + 1 + round as i32;
        let sequence = aviator.dig(block, ground, z).await.unwrap();
        let acknowledged = reached(
            &mut [&mut aviator, &mut onlooker],
            "the server acknowledged the block broken after the packet",
            |bot| bot.acknowledged_sequence == sequence,
        )
        .await;
        reached(
            &mut [&mut onlooker, &mut aviator],
            "the onlooker sees the block gone",
            |bot| matches!(bot.block_at(block, ground, z), Ok(Some(state)) if state == air),
        )
        .await;
        println!(
            "  the one who flies was sent: {}",
            arrived_since(&before.0, &aviator)
        );
        println!(
            "  the onlooker was sent: {}",
            arrived_since(&before.1, &onlooker)
        );
        println!(
            "  the one who flies: {} abilities packets so far, flies by its own account: {}, \
             connection ended: {:?}",
            aviator.abilities.len(),
            aviator.flying,
            aviator.ended
        );
        if flying.is_some() {
            taken.push((what, acknowledged && aviator.ended.is_none()));
        }
    }
    println!("AFTER:\n{}\n{}", aviator.report(), onlooker.report());

    // The record's guess: the official server takes the packet in this shape.
    for (what, taken) in taken {
        assert!(
            taken,
            "GUESS WRONG: the server did not read on after: {what}"
        );
    }
}

/// What a client is sent that logs in again after it left flying, high in the air and
/// looking somewhere: the record guesses that the official server sends an abilities
/// packet with `0x02` and then a position that is the place left, with its look, which
/// is what Clustine will send (ADR-0020, section 4.1 and the section on flying). It is
/// also what shows that `0x02` in the serverbound packet is flying: nothing else of
/// this bot's could have set the bit the server sends back.
///
/// In the output: "FIRST ENTRY" is the same player before it ever flew, for comparison
/// (its abilities packet and the order of its packets). "ON ENTERING AGAIN, when the
/// position had come" lists every packet up to the position in the order they came,
/// the abilities packets with their flags, and the position packet in full; "once the
/// chunks had come" the same a little later, which shows anything the server sent
/// after the position: a second abilities packet or a position that puts the client
/// elsewhere. Whether a real client then stays in the air only a real client shows.
///
/// Clustine's counterpart is step R1.5, the first in which a client is sent a look,
/// the flying bit and a place; this test then asks Clustine the same.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn entering_again_after_leaving_in_flight_on_the_official_server() {
    /// Yaw and pitch, in degrees: neither is what a server gives a player by itself.
    const LOOK: (f32, f32) = (135.0, 30.0);

    let oracle = Oracle::start(false).await.unwrap();
    let mut flyer = Bot::join(oracle.address(), "Flyer").await.unwrap();
    println!("\n== Entering again after leaving in flight, on the official server ==");
    reached(
        &mut [&mut flyer],
        "the flyer has left the loading screen",
        Bot::is_loaded,
    )
    .await;
    println!("FIRST ENTRY, before it ever flew:\n{}", flyer.report());

    let (x, y, z) = flyer.location;
    let place = (x + 3.0, y + 12.0, z - 2.0);
    flyer.set_flying(true).await.unwrap();
    match flyer.fly_to(place, LOOK, 1.0).await {
        Ok(()) => println!("the flyer flew to {place:?}, looking along {LOOK:?}"),
        Err(error) => println!("THE FLIGHT FAILED: {error:#}"),
    }

    // Someone who joins now is shown the flyer where the server has it.
    let mut watcher = Bot::join(oracle.address(), "Watcher").await.unwrap();
    reached(
        &mut [&mut watcher, &mut flyer],
        "a client that joins now sees the flyer at the place it flew to",
        |bot| {
            bot.seen_player("Flyer")
                .is_some_and(|seen| near(seen.position, place))
        },
    )
    .await;
    println!(
        "the watcher sees the flyer as {:?}",
        watcher.seen_player("Flyer")
    );
    println!("BEFORE LEAVING:\n{}", flyer.report());

    drop(flyer);
    reached(
        &mut [&mut watcher],
        "the server has taken the leave: the flyer is gone from the watcher's list",
        |bot| !bot.player_list.values().any(|name| name == "Flyer"),
    )
    .await;

    let mut again = Bot::join(oracle.address(), "Flyer").await.unwrap();
    // `join` returns with the first position, so this is what came up to it.
    let abilities_before_the_position = again.abilities.clone();
    let position = again.position.clone();
    println!(
        "ON ENTERING AGAIN, when the position had come:\n{}",
        again.report()
    );
    reached(
        &mut [&mut again, &mut watcher],
        "the flyer has its chunk again and left the loading screen",
        Bot::is_loaded,
    )
    .await;
    reached(
        &mut [&mut watcher, &mut again],
        "the watcher sees the flyer again, at the place it left",
        |bot| {
            bot.seen_player("Flyer")
                .is_some_and(|seen| near(seen.position, place))
        },
    )
    .await;
    println!(
        "ON ENTERING AGAIN, once the chunks had come:\n{}",
        again.report()
    );
    println!(
        "the watcher sees the flyer as {:?}",
        watcher.seen_player("Flyer")
    );

    // The record's guess: the abilities packet with the flying bit, then the place and
    // the look, and nothing after them that takes either back.
    assert!(
        abilities_before_the_position
            .first()
            .is_some_and(|packet| packet.flags & abilities::FLYING != 0),
        "GUESS WRONG: no abilities packet with 0x02 came before the position: \
         {abilities_before_the_position:?}"
    );
    let position = position.expect("a bot that joined was sent a position");
    assert!(
        near((position.x, position.y, position.z), place)
            && (position.yaw - LOOK.0).abs() < 0.01
            && (position.pitch - LOOK.1).abs() < 0.01
            && position.relative_flags == 0,
        "GUESS WRONG: the position is not the place and the look that were left \
         ({place:?}, {LOOK:?}): {position:?}"
    );
    assert!(
        again.flying && near(again.location, place),
        "GUESS WRONG: the server took the flying or the place back after the position"
    );
}

// What follows asks the official server alone what it sends of light, for ADR-0023
// (step L1; `docs/adr/0023-light-at-the-edge.md`, section 8, has the guesses G1 to G7
// and what each outcome means). Clustine's edge sends no light update yet.

/// How many turns of the server in a row have to bring no light update before a step
/// is taken as over; see [`settle`].
const QUIET_TURNS: u32 = 5;

/// The most turns a step is given to become quiet.
const MOST_TURNS: u32 = 60;

/// The hotbar slots the builder keeps its two items in.
const STONE_SLOT: i16 = 0;
const TORCH_SLOT: i16 = 1;

/// The bots of the light comparison: one that builds and two that watch.
struct Cast {
    builder: Bot,
    /// Stands across a chunk border from what is built, with the builder's chunk well
    /// inside its view.
    near: Bot,
    /// Stands so far off that the builder's chunk is the outermost of its view.
    rim: Bot,
}

impl Cast {
    /// Says what every bot was sent and gives up. For what has to hold before the
    /// question can be asked at all: a block that was not placed, a bot the server
    /// put back. Such a failure says nothing about light.
    fn give_up(&self, what: &str) -> ! {
        for bot in [&self.builder, &self.near, &self.rim] {
            println!("{}", bot.report());
            println!(
                "  all it was sent: {}",
                arrived_since(&BTreeMap::new(), bot)
            );
        }
        panic!("THE ARRANGEMENT FAILED, which says nothing about light: {what}");
    }

    fn all(&mut self) -> [&mut Bot; 3] {
        [&mut self.builder, &mut self.near, &mut self.rim]
    }
}

/// Where the comparison happens: everything is built in the builder's chunk, against
/// its eastern border, beyond which Near stands and, further off, Rim.
#[derive(Debug, Clone, Copy)]
struct Site {
    /// The x of the first block east of the builder's chunk.
    border: i32,
    /// The z of the chunk's northernmost row of blocks.
    north: i32,
    /// The height a bot stands at; the top block of the ground is the one below.
    feet: i32,
    /// The lowest height of the world.
    min_y: i32,
}

impl Site {
    fn ground(&self) -> i32 {
        self.feet - 1
    }

    /// The torch: on the ground, two blocks from the border.
    fn torch(&self) -> (i32, i32, i32) {
        (self.border - 2, self.feet, self.north + 3)
    }

    /// The column of the stone that is placed at head height, as x and z.
    fn stone(&self) -> (i32, i32) {
        (self.border - 2, self.north + 7)
    }

    /// The column of the pillar, as x and z. The platform is built around its top.
    fn pillar(&self) -> (i32, i32) {
        (self.border - 1, self.north + 11)
    }

    /// The pillar's first top: the lowest block of the third section of the world, so
    /// that it is the only block of its section.
    fn pillar_top(&self) -> i32 {
        self.min_y + 2 * 16
    }

    /// The height of the platform: in the fifth section, so that the third neither
    /// holds a block nor has a section beside it that does once the pillar is gone.
    fn platform(&self) -> i32 {
        self.min_y + 4 * 16 + 8
    }

    /// The x the builder keeps to on the ground, where nothing is ever built: it goes
    /// north and south along it, and leaves it only for the column it flies up in.
    fn lane(&self) -> f64 {
        f64::from(self.border) - 4.5
    }

    /// Where the builder flies up and down, as x and z: two blocks west of the
    /// pillar, in a column that stays empty up to the platform.
    fn column(&self) -> (f64, f64) {
        (f64::from(self.border) - 2.5, f64::from(self.north) + 11.5)
    }

    /// The light section the height `y` is in: 0 is the one below the world.
    fn light_section(&self, y: i32) -> usize {
        ((y - self.min_y) / 16 + 1) as usize
    }
}

/// Whether `bot` has `state` at `at`, as far as it holds the chunk: a bot that does
/// not hold it has nothing to be shown.
fn shows(bot: &Bot, at: (i32, i32, i32), state: i32) -> bool {
    match bot.block_at(at.0, at.1, at.2) {
        Ok(Some(there)) => there == state,
        Ok(None) => true,
        Err(_) => false,
    }
}

/// Waits until the server has acknowledged the builder's action `sequence` and every
/// bot that holds the chunk has `state` at `at`.
async fn change(cast: &mut Cast, what: &str, sequence: i32, at: (i32, i32, i32), state: i32) {
    let acknowledged = waited(&mut cast.all(), what, |bot| {
        bot.acknowledged_sequence >= sequence
    })
    .await;
    if !acknowledged {
        cast.give_up(&format!("{what}: the server did not acknowledge it"));
    }
    for first in 0..3 {
        let mut bots = cast.all();
        bots.rotate_left(first);
        if !waited(&mut bots, what, |bot| shows(bot, at, state)).await {
            let name = bots[0].info.profile.name.clone();
            let there = bots[0].block_at(at.0, at.1, at.2);
            // The bots read a block update and not the packet that carries several
            // blocks of one section, which the official server sends where two
            // change in one tick: grass under a stone turns to dirt at a moment of
            // its own, and that moment can be this one.
            let together = bots[0]
                .stats
                .received
                .get("minecraft:section_blocks_update")
                .map_or("", |_| {
                    "; it was sent a section_blocks_update, which the bots do not read, \
                     so the change may have come in one: run the test again"
                });
            cast.give_up(&format!(
                "{what}: {name} has {there:?} at {at:?} and not the state {state}{together}"
            ));
        }
    }
    // The official server puts back a player whose movement it does not take, and a
    // builder that is not where the test thinks cannot reach what it builds next.
    if cast.builder.stats.teleports_confirmed != 1 {
        cast.give_up(&format!(
            "{what}: the server put the builder back, to {:?}",
            cast.builder.location
        ));
    }
}

/// The block that a click on the `side` of `block` places a block into.
fn beside(block: (i32, i32, i32), side: u8) -> (i32, i32, i32) {
    use clustine_protocol::packets::play::face;

    let (x, y, z) = block;
    match side {
        face::BOTTOM => (x, y - 1, z),
        face::TOP => (x, y + 1, z),
        face::NORTH => (x, y, z - 1),
        face::SOUTH => (x, y, z + 1),
        face::WEST => (x - 1, y, z),
        _ => (x + 1, y, z),
    }
}

/// The builder uses the item it holds on the `side` of the block `against`, and
/// everyone is shown `state` in the block beside it.
async fn place(cast: &mut Cast, against: (i32, i32, i32), side: u8, state: i32) {
    let at = beside(against, side);
    let used = cast
        .builder
        .use_item_on(against.0, against.1, against.2, side)
        .await;
    match used {
        Ok(sequence) => change(cast, &format!("placing at {at:?}"), sequence, at, state).await,
        Err(error) => cast.give_up(&format!("placing at {at:?}: {error:#}")),
    }
}

/// The builder breaks the block `at`, and everyone is shown air there.
async fn take_away(cast: &mut Cast, at: (i32, i32, i32)) {
    let air = i32::from(clustine_data::blocks::AIR.0);
    match cast.builder.dig(at.0, at.1, at.2).await {
        Ok(sequence) => change(cast, &format!("breaking at {at:?}"), sequence, at, air).await,
        Err(error) => cast.give_up(&format!("breaking at {at:?}: {error:#}")),
    }
}

/// The builder takes the item of `slot` into its hand.
async fn hold(cast: &mut Cast, slot: i16) {
    if let Err(error) = cast.builder.select_slot(slot).await {
        cast.give_up(&format!("selecting the slot {slot}: {error:#}"));
    }
}

/// The builder flies in a straight line to `to`, a block a tick.
async fn fly(cast: &mut Cast, to: (f64, f64, f64)) {
    if let Err(error) = cast.builder.fly_to(to, (0.0, 0.0), 1.0).await {
        cast.give_up(&format!("the builder's flight to {to:?}: {error:#}"));
    }
}

/// The builder goes to where the lane meets the row `z`, on the ground: back to the
/// lane first if it is in its column, so that it never cuts a corner past something
/// built.
async fn to_the_lane(cast: &mut Cast, site: &Site, z: f64) {
    let feet = f64::from(site.feet);
    let (_, _, from) = cast.builder.location;
    fly(cast, (site.lane(), feet, from)).await;
    fly(cast, (site.lane(), feet, z)).await;
}

/// The builder goes to the foot of the column it flies up in.
async fn to_the_column(cast: &mut Cast, site: &Site) {
    let (x, z) = site.column();
    to_the_lane(cast, site, z).await;
    fly(cast, (x, f64::from(site.feet), z)).await;
}

/// The builder, in its column, comes within easy reach of the pillar's block at the
/// height `y`: its feet no more than two blocks from the block below it.
async fn reach(cast: &mut Cast, site: &Site, y: i32) {
    let want = f64::from((y - 1).max(site.feet));
    if (want - cast.builder.location.1).abs() >= 2.0 {
        let (x, z) = site.column();
        fly(cast, (x, want, z)).await;
    }
}

/// Lets a turn of the server pass for `bots[0]`: it breaks the air above its head and
/// waits for the acknowledgement, which the server sends at the end of the tick in
/// which it read the packet. Whatever the server had made for this client before that
/// tick ended has then come. A wait for a message, where a test of light would
/// otherwise have to wait for time: whether a light update follows a block update is
/// the question, so there is no message to wait for.
async fn a_turn_of_the_server(bots: &mut [&mut Bot]) -> Result<(), String> {
    let (x, y, z) = bots[0].location;
    let name = bots[0].info.profile.name.clone();
    let sequence = bots[0]
        .dig(x.floor() as i32, y.floor() as i32 + 3, z.floor() as i32)
        .await
        .map_err(|error| format!("{name} could not break the air above it: {error:#}"))?;
    if waited(bots, "a turn of the server", |bot| {
        bot.acknowledged_sequence >= sequence
    })
    .await
    {
        Ok(())
    } else {
        Err(format!(
            "the server did not acknowledge {name} breaking the air above it"
        ))
    }
}

/// Waits until the server has nothing more to say of light: until [`QUIET_TURNS`]
/// turns of the server in a row have brought neither watcher a light update.
async fn settle(cast: &mut Cast) {
    let came = |cast: &Cast| {
        [&cast.near, &cast.rim].map(|bot| bot.stats.light_updates + bot.stats.stray_light_updates)
    };
    let (mut quiet, mut turns) = (0, 0);
    while quiet < QUIET_TURNS && turns < MOST_TURNS {
        let before = came(cast);
        for first in [1, 2] {
            let mut bots = cast.all();
            bots.rotate_left(first);
            if let Err(error) = a_turn_of_the_server(&mut bots).await {
                cast.give_up(&error);
            }
        }
        quiet = if came(cast) == before { quiet + 1 } else { 0 };
        turns += 1;
    }
    if quiet < QUIET_TURNS {
        println!("  LIGHT UPDATES KEPT COMING for {MOST_TURNS} turns of the server");
    }
}

/// The blocks `bot` holds of the chunk at `position`, as a chunk of Clustine's.
fn blocks_of(bot: &Bot, position: (i32, i32)) -> Option<clustine_world::Chunk> {
    use clustine_data::BlockState;
    use clustine_world::{Biome, Chunk};

    let sections = bot.sections(position).ok()??;
    let dimension = bot.info.dimension_type().ok()?;
    // Light does not read biomes.
    let mut chunk = Chunk::empty(dimension, Biome(0));
    let air = i32::from(clustine_data::blocks::AIR.0);
    for (section, data) in sections.iter().enumerate() {
        let bottom = dimension.min_y + section as i32 * 16;
        for index in 0..4096 {
            let state = data.blocks.get(index);
            if state != air {
                let state = BlockState(u16::try_from(state).ok()?);
                chunk.set(
                    index & 15,
                    bottom + (index >> 8) as i32,
                    index >> 4 & 15,
                    state,
                );
            }
        }
    }
    for ((x, y, z), state) in bot.changed_blocks(position) {
        let state = BlockState(u16::try_from(state).ok()?);
        chunk.set(x as usize & 15, y, z as usize & 15, state);
    }
    Some(chunk)
}

/// The blocks of a chunk and of the eight around it, as a bot held them at one moment.
///
/// The light of them is made when the run is over ([`Nine::light`]): while it is made
/// nobody attends to the bots, and a bot that does not answer is put out.
#[derive(Clone)]
struct Nine {
    centre: clustine_world::Chunk,
    /// North-west first, row by row, as `clustine_light::Around` wants them.
    neighbours: Vec<clustine_world::Chunk>,
}

impl Nine {
    /// The blocks `bot` holds of the chunk at `centre` and around it. `None` if it
    /// lacks one of the nine: the server has it, so that taking it as solid would be
    /// another question.
    fn of(bot: &Bot, centre: (i32, i32)) -> Option<Self> {
        const BESIDE: [(i32, i32); 8] = [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ];
        let neighbours = BESIDE
            .into_iter()
            .map(|(dx, dz)| blocks_of(bot, (centre.0 + dx, centre.1 + dz)))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            centre: blocks_of(bot, centre)?,
            neighbours,
        })
    }

    /// What `clustine_light::light` gives for the centre.
    fn light(&self) -> clustine_world::Light {
        let mut neighbours = [None; 8];
        for (place, chunk) in neighbours.iter_mut().zip(&self.neighbours) {
            *place = Some(chunk);
        }
        clustine_light::light(&clustine_light::Around {
            centre: &self.centre,
            neighbours,
            // The official server's world here is the overworld.
            sky: true,
        })
    }
}

/// The level of the block with the index `index` in a section's array.
fn level(levels: &[u8], index: usize) -> u8 {
    levels[index / 2] >> (index % 2 * 4) & 15
}

/// The levels an array holds, for a person to read.
fn levels_in(levels: &[u8]) -> String {
    let distinct: BTreeSet<u8> = (0..levels.len() * 2).map(|i| level(levels, i)).collect();
    let distinct: Vec<u8> = distinct.into_iter().collect();
    match distinct.as_slice() {
        [0] => "an array of zeros".to_owned(),
        [15] => "an array of fifteens".to_owned(),
        _ => format!("levels {distinct:?}"),
    }
}

/// How a server said a section's light.
fn said_as(sent: &SentLight) -> String {
    match sent {
        SentLight::Unsaid => "no mask".to_owned(),
        SentLight::Dark => "the empty mask".to_owned(),
        SentLight::Levels(levels) => levels_in(levels),
    }
}

/// The case the crate has a section in, and its levels where it has any.
fn of_the_crate(light: &clustine_world::SectionLight) -> (&'static str, Option<Vec<u8>>) {
    use clustine_world::{LIGHT_ARRAY_LENGTH, SectionLight};

    match light {
        SectionLight::Absent => ("Absent", None),
        SectionLight::Dark => ("Dark", Some(vec![0; LIGHT_ARRAY_LENGTH])),
        SectionLight::Full => ("Full", Some(vec![0xFF; LIGHT_ARRAY_LENGTH])),
        SectionLight::Levels(levels) => ("Levels", Some(levels.to_vec())),
    }
}

/// The levels the crate's light reads as in a section it lists nothing for, by the
/// crate's own rule (the record's G3): sky light as the bottom layer of the nearest
/// listed section above, full where there is none; block light dark.
fn read_as(sky: bool, sections: &[clustine_world::SectionLight], section: usize) -> Vec<u8> {
    let length = clustine_world::LIGHT_ARRAY_LENGTH;
    if !sky {
        return vec![0; length];
    }
    let above = sections
        .iter()
        .skip(section + 1)
        .find_map(|light| of_the_crate(light).1);
    match above {
        // A layer is 256 blocks, which is 128 bytes.
        Some(levels) => levels[..128].repeat(16),
        None => vec![0xFF; length],
    }
}

/// How one section a server sent compares with the crate's.
enum Compared {
    /// The same levels, or nothing listed by either.
    Equal,
    /// One of the two lists the section and the other does not. With what was sent
    /// and whether it is what the other reads there.
    Listing(String),
    /// Both have levels and they differ.
    Differs(String),
}

/// Compares a section as a client has it with the crate's, level by level: an array
/// of zeros or the empty mask is the crate's `Dark`, an array of fifteens its `Full`.
fn compared(
    sky: bool,
    sent: &SentLight,
    sections: &[clustine_world::SectionLight],
    section: usize,
) -> Compared {
    let (case, ours) = of_the_crate(&sections[section]);
    let Some(theirs) = sent.levels() else {
        return match ours {
            None => Compared::Equal,
            Some(_) => Compared::Listing(format!(
                "the server names it in no mask; the crate has it as {case}"
            )),
        };
    };
    let said = said_as(sent);
    let Some(ours) = ours else {
        let reads = if read_as(sky, sections, section) == theirs {
            "which is what the crate's light reads as there"
        } else {
            "WHICH IS NOT what the crate's light reads as there"
        };
        return Compared::Listing(format!(
            "the server sent {said}; the crate lists nothing there (Absent), {reads}"
        ));
    };
    let differing: Vec<usize> = (0..4096)
        .filter(|index| level(&theirs, *index) != level(&ours, *index))
        .collect();
    match differing.first() {
        None => Compared::Equal,
        Some(first) => Compared::Differs(format!(
            "{} of 4096 levels differ; the first at x {}, y {}, z {} of the section, where \
             the server has {} and the crate {}; the server sent {said}, the crate has {case}",
            differing.len(),
            first & 15,
            first >> 8,
            first >> 4 & 15,
            level(&theirs, *first),
            level(&ours, *first)
        )),
    }
}

/// The light a watcher's packets added up to for one chunk at the end of a step, to be
/// compared with the crate's in the sections the step's updates named.
struct Owed {
    step: usize,
    name: String,
    chunk: (i32, i32),
    /// The sections named, for the sky and for blocks.
    sky: u64,
    block: u64,
    sent: Option<ChunkLight>,
}

/// What the comparison has seen so far, for its summary and for what it asserts.
#[derive(Default)]
struct Tally {
    /// Each step, with how many light updates Near and Rim were sent in it for
    /// chunks they hold.
    steps: Vec<(String, usize, usize)>,
    /// Light updates for chunks the bot they went to did not hold.
    strays: usize,
    /// Light updates for chunks held; how many of them were for a chunk within two
    /// of the builder's, which is as far as anything built can have reached; and how
    /// many named fewer sections than their chunk lists.
    updates: usize,
    around_home: usize,
    naming_fewer: usize,
    /// What is to be compared with the crate once the run is over, and the blocks to
    /// make the crate's light from: the builder's, by step and chunk.
    owed: Vec<Owed>,
    blocks: BTreeMap<(usize, (i32, i32)), Option<Nine>>,
}

/// Ends a step: waits until the server is quiet, says what each watcher was sent in
/// it, packet by packet, and keeps the light their packets now add up to, with the
/// blocks as the builder holds them, for the comparison at the end.
async fn observe(cast: &mut Cast, home: (i32, i32), step: &str, tally: &mut Tally) {
    use clustine_botswarm::light::sections_of;

    settle(cast).await;
    println!("STEP {}: {step}", tally.steps.len());
    let Cast { builder, near, rim } = cast;
    let number = tally.steps.len();
    let mut counts = [0; 2];
    for (which, watcher) in [near, rim].into_iter().enumerate() {
        let name = watcher.info.profile.name.clone();
        let updates = std::mem::take(&mut watcher.light_updates);
        let strays = std::mem::take(&mut watcher.stray_light_updates);
        println!(
            "  {name} was sent {} light updates for chunks it holds and {} for chunks it \
             does not hold",
            updates.len(),
            strays.len()
        );
        counts[which] = updates.len();
        tally.strays += strays.len();

        // The sections the step's updates named, by chunk: sky and block light.
        let mut named: BTreeMap<(i32, i32), (u64, u64)> = BTreeMap::new();
        for update in updates.iter().chain(&strays) {
            println!("    {update}");
            if let Some(light) = &update.light {
                for (kind, mask, arrays) in [
                    ("sky", update.named.sky, &light.sky),
                    ("block", update.named.block, &light.block),
                ] {
                    for (section, array) in sections_of(mask).into_iter().zip(arrays) {
                        println!("      {kind} light section {section}: {}", levels_in(array));
                    }
                }
            }
            if !update.held {
                continue;
            }
            let (sky, block) = (update.named.any_sky(), update.named.any_block());
            let entry = named.entry(update.chunk).or_default();
            entry.0 |= sky;
            entry.1 |= block;
            tally.updates += 1;
            let (dx, dz) = (update.chunk.0 - home.0, update.chunk.1 - home.1);
            if dx.abs() <= 2 && dz.abs() <= 2 {
                tally.around_home += 1;
            }
            match watcher.chunk_light(update.chunk) {
                Ok(Some(light)) => {
                    let listed = |sections: &[SentLight]| {
                        let said = |light: &&SentLight| **light != SentLight::Unsaid;
                        sections.iter().filter(said).count() as u32
                    };
                    let (sky_listed, block_listed) = (listed(&light.sky), listed(&light.block));
                    if sky.count_ones() < sky_listed || block.count_ones() < block_listed {
                        tally.naming_fewer += 1;
                    }
                    println!(
                        "      it names {} of the {sky_listed} sky light sections and {} of \
                         the {block_listed} block light sections that the chunk lists by now",
                        sky.count_ones(),
                        block.count_ones()
                    );
                }
                other => println!("      THE CHUNK'S LIGHT COULD NOT BE READ: {other:?}"),
            }
        }

        for (chunk, (sky, block)) in named {
            tally
                .blocks
                .entry((number, chunk))
                .or_insert_with(|| Nine::of(builder, chunk));
            tally.owed.push(Owed {
                step: number,
                name: name.clone(),
                chunk,
                sky,
                block,
                sent: watcher.chunk_light(chunk).ok().flatten(),
            });
        }
    }
    tally.steps.push((step.to_owned(), counts[0], counts[1]));
}

/// Compares what the steps left to be compared, now that nobody has to attend to the
/// bots, and says what it finds. Returns how many sections have the crate's levels,
/// how many one side lists and the other does not, how many chunks could not be
/// compared, and every section whose levels differ.
fn compare_what_is_owed(tally: &Tally) -> (usize, usize, usize, Vec<String>) {
    use clustine_botswarm::light::sections_of;

    println!("COMPARED WITH THE CRATE: what the watchers' packets added up to after each step");
    let (mut equal, mut listed_otherwise, mut not_compared) = (0, 0, 0);
    let mut differing = Vec::new();
    let mut lights: BTreeMap<(usize, (i32, i32)), Option<clustine_world::Light>> = BTreeMap::new();
    for owed in &tally.owed {
        let key = (owed.step, owed.chunk);
        let expected = lights
            .entry(key)
            .or_insert_with(|| tally.blocks.get(&key)?.as_ref().map(Nine::light));
        let step = format!("step {} ({})", owed.step, tally.steps[owed.step].0);
        let (name, chunk) = (&owed.name, owed.chunk);
        let (Some(expected), Some(sent)) = (expected, &owed.sent) else {
            println!(
                "  {step}: {name}, chunk {chunk:?}: NOT COMPARED: the builder lacked it or \
                 a chunk beside it, or {name} no longer held it"
            );
            not_compared += 1;
            continue;
        };
        for (kind, mask, sent, expected) in [
            ("sky", owed.sky, &sent.sky, &expected.sky),
            ("block", owed.block, &sent.block, &expected.block),
        ] {
            for section in sections_of(mask) {
                let place =
                    format!("{step}: {name}, chunk {chunk:?}, {kind} light section {section}");
                match compared(kind == "sky", &sent[section], expected, section) {
                    Compared::Equal => equal += 1,
                    Compared::Listing(note) => {
                        listed_otherwise += 1;
                        println!("  {place}: {note}");
                    }
                    Compared::Differs(note) => {
                        println!("  {place}: DIFFERS FROM THE CRATE: {note}");
                        differing.push(format!("{place}: {note}"));
                    }
                }
            }
        }
        println!(
            "  {step}: {name}, chunk {chunk:?}: compared in the sky light sections {:?} and \
             the block light sections {:?}",
            sections_of(owed.sky),
            sections_of(owed.block)
        );
    }
    (equal, listed_otherwise, not_compared, differing)
}

/// The cases of a chunk's light sections in a row, the section below the world first.
/// For a client: `.` in no mask, `D` in the empty mask, `0` an array of zeros, `F` an
/// array of fifteens, `L` other levels.
fn row_of_a_client(sections: &[SentLight]) -> String {
    let case = |light: &SentLight| match light {
        SentLight::Unsaid => '.',
        SentLight::Dark => 'D',
        SentLight::Levels(levels) if levels.iter().all(|byte| *byte == 0) => '0',
        SentLight::Levels(levels) if levels.iter().all(|byte| *byte == 0xFF) => 'F',
        SentLight::Levels(_) => 'L',
    };
    sections.iter().map(case).collect()
}

/// The same for the crate: `.` Absent, `D` Dark, `F` Full, `L` Levels.
fn row_of_the_crate(sections: &[clustine_world::SectionLight]) -> String {
    let case = |light: &clustine_world::SectionLight| match of_the_crate(light).0 {
        "Absent" => '.',
        "Dark" => 'D',
        "Full" => 'F',
        _ => 'L',
    };
    sections.iter().map(case).collect()
}

/// What light the official server sends when blocks change, and what light it sends
/// with a chunk afterwards: the comparison of `docs/adr/0023-light-at-the-edge.md`,
/// section 8, whose table says what each outcome means.
///
/// **Who stands where.** The builder builds in its own chunk, against that chunk's
/// eastern border. Near stands just across the border. Rim walks east a chunk at a
/// time until the server takes the builder's chunk from it, and then one chunk back:
/// the builder's chunk is then the outermost of its view.
///
/// **What the builder does**, each a step: a torch placed on the ground two blocks
/// from the border, and broken; a stone placed at head height under the sky (on
/// another stone, which is then broken), and broken; a pillar from the ground to the
/// lowest block of the world's third section, and its top block broken; the torch
/// again, and a roof of three by three stones over it (with the post of two stones it
/// hangs on); a platform of five by five stones in the world's fifth section, across
/// the border, built on the pillar, and the pillar taken down under it.
///
/// **In the output.** "WHO STANDS WHERE" has the arrangement. Each "STEP" lists what
/// Near and Rim were sent while it lasted: every light update as a line with its
/// chunk, its place in the stream ("packet n after k block updates"), the sections it
/// named and what its arrays hold, and how many of the sections the chunk lists it
/// named (fewer settles G2). Step 0 is what came before anything was built, and counts
/// for nothing. "COMPARED WITH THE CRATE" has, for each step and chunk, how the light
/// the watcher's packets added up to compares with `clustine_light::light` over the
/// blocks the builder held then, in the sections named (G6); it comes after the steps
/// because nobody attends to the bots while light is made. "(e)" says how the section
/// that lost its light with the pillar's top stands, in the crate and in what the
/// watchers were sent; whether an update named it is in that step's lines. "THE
/// LATECOMER" is part (d): the light of the chunk packets a client is sent that joins
/// afterwards, for the twelve chunks around the platform, against the crate (G3) and
/// against what Near's and Rim's packets add up to, which is where a client that was
/// sent no update is stale and has to have lit by itself (G1). "SUMMARY" has, step by
/// step, which watcher saw updates (G7), and the counts.
///
/// **It fails** if no watcher was sent any light update for a chunk within two of
/// the builder's after the building began, since a
/// comparison that looked at nothing settles nothing; if levels the server sent differ
/// from the crate's; and if the latecomer's light differs from the crate's in levels
/// or in which sections are listed. All of it is printed first. A panic that begins
/// "THE ARRANGEMENT FAILED" is none of these: something the test needs before it can
/// ask did not happen.
///
/// Clustine's counterpart comes with step L5, when the edge sends light updates.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn light_after_blocks_change_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    println!("\n== Light after blocks change, on the official server ==");
    light_after_blocks_change(oracle.address()).await;
}

/// The comparison of [`light_after_blocks_change_on_the_official_server`], against the
/// server at `address`.
async fn light_after_blocks_change(address: &str) {
    use clustine_botswarm::light::sections_of;
    use clustine_data::{blocks, items};
    use clustine_protocol::packets::play::face;

    let stone = i32::from(blocks::STONE.0);
    let torch = i32::from(blocks::TORCH.0);
    let air = i32::from(blocks::AIR.0);

    let mut cast = Cast {
        builder: Bot::join(address, "Builder").await.unwrap(),
        near: Bot::join(address, "Near").await.unwrap(),
        rim: Bot::join(address, "Rim").await.unwrap(),
    };
    for first in 0..3 {
        let mut bots = cast.all();
        bots.rotate_left(first);
        bots[0].keep_light_arrays();
        let what = format!("{} has left the loading screen", bots[0].info.profile.name);
        if !reached(&mut bots, &what, Bot::is_loaded).await {
            cast.give_up(&what);
        }
    }

    let dimension = cast.builder.info.dimension_type().unwrap();
    let home = (
        (cast.builder.location.0.floor() as i32) >> 4,
        (cast.builder.location.2.floor() as i32) >> 4,
    );
    let site = Site {
        border: (home.0 + 1) * 16,
        north: home.1 * 16,
        feet: cast.builder.location.1.floor() as i32,
        min_y: dimension.min_y,
    };
    if site.ground() < site.min_y || site.ground() >= site.min_y + 16 {
        cast.give_up(&format!(
            "the ground's top is at {}, not in the lowest section of a world that begins \
             at {}; the heights of the pillar and the platform are reckoned from that",
            site.ground(),
            site.min_y
        ));
    }
    // The chunks whose light is compared at the end, and every chunk beside one.
    let compared_chunks: Vec<(i32, i32)> = (home.0 - 1..=home.0 + 2)
        .flat_map(|x| (home.1 - 1..=home.1 + 1).map(move |z| (x, z)))
        .collect();
    let needed: Vec<(i32, i32)> = (home.0 - 2..=home.0 + 3)
        .flat_map(|x| (home.1 - 2..=home.1 + 2).map(move |z| (x, z)))
        .collect();
    let what = "the builder holds the chunks around its own and stands on the ground";
    let ground = (site.border - 2, site.ground(), site.north + 3);
    let holds = |bot: &Bot| {
        needed.iter().all(|chunk| bot.chunks.contains_key(chunk))
            && matches!(bot.block_at(ground.0, ground.1, ground.2), Ok(Some(state)) if state != air)
            && shows(bot, site.torch(), air)
    };
    if !reached(&mut cast.all(), what, holds).await {
        cast.give_up(what);
    }

    // Near goes to its place, two blocks beyond the border.
    let row = f64::from(site.north) + 7.5;
    if let Err(error) = cast
        .near
        .walk_to(f64::from(site.border) + 2.5, row, 1.0)
        .await
    {
        cast.give_up(&format!("Near's walk: {error:#}"));
    }
    // Rim goes east until the builder's chunk is taken from it, and one chunk back.
    let middle_of = |east: i32| f64::from((home.0 + east) * 16) + 8.5;
    let mut east = 1;
    loop {
        if let Err(error) = cast.rim.walk_to(middle_of(east), row, 1.0).await {
            cast.give_up(&format!("Rim's walk: {error:#}"));
        }
        // The server takes a chunk away when it reads the step that moves the view,
        // so after two turns of it the chunk is gone if it was to go.
        for _ in 0..2 {
            let mut bots = cast.all();
            bots.rotate_left(2);
            if let Err(error) = a_turn_of_the_server(&mut bots).await {
                cast.give_up(&error);
            }
        }
        if !cast.rim.chunks.contains_key(&home) {
            break;
        }
        if east == 40 {
            cast.give_up("Rim still holds the builder's chunk forty chunks to the east of it");
        }
        east += 1;
    }
    east -= 1;
    if let Err(error) = cast.rim.walk_to(middle_of(east), row, 1.0).await {
        cast.give_up(&format!("Rim's walk back: {error:#}"));
    }
    let what = "Rim holds the builder's chunk again, and the chunk east of it";
    let mut bots = cast.all();
    bots.rotate_left(2);
    let holds = |bot: &Bot| {
        bot.chunks.contains_key(&home)
            && bot.chunks.contains_key(&(home.0 + 1, home.1))
            && bot.center == Some((home.0 + east, home.1))
    };
    if !reached(&mut bots, what, holds).await {
        cast.give_up(what);
    }
    for (name, bot) in [("Near", &cast.near), ("Rim", &cast.rim)] {
        if bot.stats.teleports_confirmed != 1 {
            cast.give_up(&format!("the server put {name} back while it walked"));
        }
    }
    let westmost = |bot: &Bot| {
        let in_the_row = |chunk: &&(i32, i32)| chunk.1 == home.1;
        bot.chunks
            .keys()
            .filter(in_the_row)
            .map(|chunk| chunk.0)
            .min()
    };
    println!(
        "WHO STANDS WHERE: the builder's chunk is {home:?}; the ground's top is at y {}; \
         the border is at x {}",
        site.ground(),
        site.border
    );
    println!(
        "  Near stands at {:?}, in the chunk {:?}; the westmost chunk it holds in the \
         builder's row has x {:?}",
        cast.near.location,
        cast.near.center,
        westmost(&cast.near)
    );
    println!(
        "  Rim stands at {:?}, in the chunk {:?}, {east} chunks east of the builder's; the \
         westmost chunk it holds in the builder's row has x {:?}; it holds {} chunks, and \
         asked for a view distance of 8 where the server allows {}",
        cast.rim.location,
        cast.rim.center,
        westmost(&cast.rim),
        cast.rim.chunks.len(),
        cast.rim.info.login.view_distance
    );

    // The builder flies from here on, so that it can build upwards; and it takes its
    // two items.
    let taken = async {
        cast.builder.set_flying(true).await?;
        cast.builder
            .take_from_creative_inventory(STONE_SLOT, items::STONE)
            .await?;
        cast.builder
            .take_from_creative_inventory(TORCH_SLOT, items::TORCH)
            .await
    };
    if let Err(error) = taken.await {
        cast.give_up(&format!("the builder taking its items: {error:#}"));
    }
    // Whatever light was sent before anything was built is no answer: its counts go.
    let mut tally = Tally::default();
    let step = "nothing built yet: what came while the bots took their places";
    observe(&mut cast, home, step, &mut tally).await;
    (tally.updates, tally.around_home, tally.naming_fewer) = (0, 0, 0);

    // A torch, two blocks from the border.
    let (torch_at, feet) = (site.torch(), site.feet);
    let under_the_torch = (torch_at.0, site.ground(), torch_at.2);
    to_the_lane(&mut cast, &site, f64::from(torch_at.2) + 0.5).await;
    hold(&mut cast, TORCH_SLOT).await;
    place(&mut cast, under_the_torch, face::TOP, torch).await;
    observe(&mut cast, home, "a torch placed on the ground", &mut tally).await;
    take_away(&mut cast, torch_at).await;
    observe(&mut cast, home, "the torch broken", &mut tally).await;

    // A stone at head height under the open sky. It needs something to be placed
    // against, which goes again before the stone is looked at alone.
    let (x, z) = site.stone();
    to_the_lane(&mut cast, &site, f64::from(z) + 0.5).await;
    hold(&mut cast, STONE_SLOT).await;
    place(&mut cast, (x, site.ground(), z), face::TOP, stone).await;
    observe(&mut cast, home, "a stone placed on the ground", &mut tally).await;
    place(&mut cast, (x, feet, z), face::TOP, stone).await;
    observe(
        &mut cast,
        home,
        "a stone placed on it, at head height",
        &mut tally,
    )
    .await;
    take_away(&mut cast, (x, feet, z)).await;
    let step = "the stone under it broken: one stone at head height under the sky";
    observe(&mut cast, home, step, &mut tally).await;
    take_away(&mut cast, (x, feet + 1, z)).await;
    observe(
        &mut cast,
        home,
        "the stone at head height broken",
        &mut tally,
    )
    .await;

    // A pillar to the lowest block of the third section, with nothing else above the
    // ground in the chunks around.
    let (x, z) = site.pillar();
    to_the_column(&mut cast, &site).await;
    for y in feet..=site.pillar_top() {
        reach(&mut cast, &site, y).await;
        place(&mut cast, (x, y - 1, z), face::TOP, stone).await;
    }
    let step = format!(
        "a pillar built from the ground to y {}, the lowest block of its section",
        site.pillar_top()
    );
    observe(&mut cast, home, &step, &mut tally).await;

    // (e): its top block broken, which leaves the section above the top's without a
    // block in it or beside it.
    let bereft = site.light_section(site.pillar_top()) + 1;
    let with_the_top = Nine::of(&cast.builder, home);
    take_away(&mut cast, (x, site.pillar_top(), z)).await;
    observe(&mut cast, home, "the pillar's top block broken", &mut tally).await;
    let without_the_top = Nine::of(&cast.builder, home);
    let said_of_the_bereft: Vec<String> = [&cast.near, &cast.rim]
        .into_iter()
        .map(|bot| {
            let (sky, block) = match bot.chunk_light(home) {
                Ok(Some(light)) => (said_as(&light.sky[bereft]), said_as(&light.block[bereft])),
                other => (format!("{other:?}"), "?".to_owned()),
            };
            format!(
                "{} has, of all its packets up to the end of that step, the sky light of \
                 the section said as {sky} and its block light as {block}",
                bot.info.profile.name
            )
        })
        .collect();

    // The torch again, and a roof over it: a post of two stones west of the torch,
    // the roof's middle row eastwards from the post's top, and a row on either side.
    fly(
        &mut cast,
        (site.column().0, f64::from(feet), site.column().1),
    )
    .await;
    to_the_lane(&mut cast, &site, f64::from(torch_at.2) + 0.5).await;
    hold(&mut cast, TORCH_SLOT).await;
    place(&mut cast, under_the_torch, face::TOP, torch).await;
    observe(&mut cast, home, "the torch placed again", &mut tally).await;
    hold(&mut cast, STONE_SLOT).await;
    let post = (torch_at.0 - 2, torch_at.2);
    place(&mut cast, (post.0, site.ground(), post.1), face::TOP, stone).await;
    place(&mut cast, (post.0, feet, post.1), face::TOP, stone).await;
    let roof = feet + 1;
    for x in post.0..post.0 + 3 {
        place(&mut cast, (x, roof, post.1), face::EAST, stone).await;
    }
    for x in post.0 + 1..=post.0 + 3 {
        place(&mut cast, (x, roof, post.1), face::NORTH, stone).await;
        place(&mut cast, (x, roof, post.1), face::SOUTH, stone).await;
    }
    let step = "a roof of three by three stones over the torch, on a post of two";
    observe(&mut cast, home, step, &mut tally).await;

    // The platform: the pillar again, up to under it; the stone on the pillar's top;
    // from that a row of five from west to east across the border; from each of the
    // five, two to the north and two to the south.
    let (x, z) = site.pillar();
    let platform = site.platform();
    to_the_column(&mut cast, &site).await;
    for y in site.pillar_top()..platform {
        reach(&mut cast, &site, y).await;
        place(&mut cast, (x, y - 1, z), face::TOP, stone).await;
    }
    // The builder's column is under the platform, so it stays below it from here on.
    let (column_x, column_z) = site.column();
    fly(&mut cast, (column_x, f64::from(platform - 3), column_z)).await;
    place(&mut cast, (x, platform - 1, z), face::TOP, stone).await;
    for step in 0..2 {
        place(&mut cast, (x - step, platform, z), face::WEST, stone).await;
        place(&mut cast, (x + step, platform, z), face::EAST, stone).await;
    }
    for x in x - 2..=x + 2 {
        for step in 0..2 {
            place(&mut cast, (x, platform, z - step), face::NORTH, stone).await;
            place(&mut cast, (x, platform, z + step), face::SOUTH, stone).await;
        }
    }
    let step = format!(
        "a platform of five by five stones at y {platform}, across the border, on its pillar"
    );
    observe(&mut cast, home, &step, &mut tally).await;
    for y in (feet..platform).rev() {
        reach(&mut cast, &site, y).await;
        take_away(&mut cast, (x, y, z)).await;
    }
    let step = "the pillar taken down: the platform floats";
    observe(&mut cast, home, step, &mut tally).await;

    // (d): someone who joins now is sent the chunks with their light.
    println!("THE LATECOMER: the light of the chunk packets a client is sent that joins now");
    let mut latecomer = Bot::join(address, "Latecomer").await.unwrap();
    let what = "the latecomer holds the chunks around the platform, with the platform";
    let centre_of_the_platform = (x, platform, z);
    let holds = |bot: &Bot| {
        needed.iter().all(|chunk| bot.chunks.contains_key(chunk))
            && matches!(
                bot.block_at(centre_of_the_platform.0, platform, centre_of_the_platform.2),
                Ok(Some(state)) if state == stone
            )
    };
    let arrived = {
        let Cast { builder, near, rim } = &mut cast;
        reached(&mut [&mut latecomer, builder, near, rim], what, holds).await
    };
    if !arrived {
        println!("{}", latecomer.report());
        cast.give_up(what);
    }
    println!(
        "  the latecomer was sent {} light updates for chunks it holds and {} for others",
        latecomer.stats.light_updates, latecomer.stats.stray_light_updates
    );
    println!(
        "  each chunk's 26 light sections, the one below the world first. The server: \
         . in no mask, D in the empty mask, 0 an array of zeros, F an array of fifteens, \
         L other levels. The crate: . Absent, D Dark, F Full, L Levels"
    );
    // First everything the bots hold is taken; then light is made, with nobody
    // attending to them any more.
    let held: Vec<_> = compared_chunks
        .iter()
        .map(|chunk| {
            let watchers = [&cast.near, &cast.rim].map(|watcher| {
                (
                    watcher.info.profile.name.clone(),
                    watcher.chunk_light(*chunk).ok().flatten(),
                    watcher.light_update_count(*chunk),
                )
            });
            (
                *chunk,
                latecomer.chunk_light(*chunk).ok().flatten(),
                Nine::of(&latecomer, *chunk),
                watchers,
            )
        })
        .collect();
    let mut of_the_latecomer = Vec::new();
    let mut latecomers_not_compared = 0;
    for (chunk, sent, nine, watchers) in held {
        let (Some(sent), Some(nine)) = (sent, nine) else {
            println!("  chunk {chunk:?}: NOT COMPARED: its light could not be read or made");
            latecomers_not_compared += 1;
            continue;
        };
        let expected = nine.light();
        println!("  chunk {chunk:?}");
        println!("    sky, the server:   {}", row_of_a_client(&sent.sky));
        println!("    sky, the crate:    {}", row_of_the_crate(&expected.sky));
        println!("    block, the server: {}", row_of_a_client(&sent.block));
        println!(
            "    block, the crate:  {}",
            row_of_the_crate(&expected.block)
        );
        for (kind, sent, expected) in [
            ("sky", &sent.sky, &expected.sky),
            ("block", &sent.block, &expected.block),
        ] {
            for (section, sent) in sent.iter().enumerate() {
                let place = format!("chunk {chunk:?}, {kind} light section {section}");
                match compared(kind == "sky", sent, expected, section) {
                    Compared::Equal => {}
                    Compared::Listing(note) | Compared::Differs(note) => {
                        println!("    {place}: DIFFERS FROM THE CRATE: {note}");
                        of_the_latecomer.push(format!("{place}: {note}"));
                    }
                }
            }
        }
        // What the watchers' packets add up to for the same chunk. A section in which
        // a watcher has other light than the latecomer was sent is one the server left
        // to the client.
        for (name, theirs, updates) in watchers {
            let Some(theirs) = theirs else {
                println!("    {name} does not hold it");
                continue;
            };
            let stale = |mine: &[SentLight], theirs: &[SentLight]| -> Vec<usize> {
                let differs =
                    |section: &usize| mine[*section].levels() != theirs[*section].levels();
                (0..mine.len()).filter(differs).collect()
            };
            println!(
                "    what {name}'s packets add up to ({updates} light updates for this \
                 chunk) differs from it in the sky light sections {:?} and the block \
                 light sections {:?}",
                stale(&sent.sky, &theirs.sky),
                stale(&sent.block, &theirs.block)
            );
        }
    }

    let (equal, listed_otherwise, not_compared, differing) = compare_what_is_owed(&tally);
    let case_of = |nine: &Option<Nine>| {
        nine.as_ref().map(|nine| {
            let light = nine.light();
            (
                of_the_crate(&light.sky[bereft]).0,
                of_the_crate(&light.block[bereft]).0,
            )
        })
    };
    println!(
        "(e) light section {bereft} of the chunk {home:?}, the one above the section the \
         pillar's top was the only block of: the crate has its sky and block light as {:?} \
         with the top and as {:?} without",
        case_of(&with_the_top),
        case_of(&without_the_top)
    );
    for said in &said_of_the_bereft {
        println!("  (e) {said}");
    }

    println!("SUMMARY: light updates for chunks held, step by step");
    for (step, near, rim) in &tally.steps {
        let who = match (near, rim) {
            (0, 0) => "neither",
            (_, 0) => "Near alone",
            (0, _) => "Rim alone",
            _ => "both",
        };
        println!("  Near {near:>3}, Rim {rim:>3} ({who}): {step}");
    }
    println!(
        "  {} of the {} light updates named fewer sections than their chunk lists; {} \
         came for chunks not held",
        tally.naming_fewer, tally.updates, tally.strays
    );
    println!(
        "  {} of them were for a chunk within two of the builder's",
        tally.around_home
    );
    println!(
        "  the builder, in the middle of it all, was sent {} light updates for chunks it \
         holds in the whole run, and {} for others",
        cast.builder.stats.light_updates, cast.builder.stats.stray_light_updates
    );
    println!(
        "  of the sections the updates named, {equal} have the crate's levels, {} have \
         others and {listed_otherwise} are listed by one side alone; {not_compared} chunks \
         could not be compared",
        differing.len()
    );
    println!(
        "  of the latecomer's {} chunks, {latecomers_not_compared} could not be compared, \
         and {} sections differ from the crate's",
        compared_chunks.len(),
        of_the_latecomer.len()
    );
    println!(
        "  the platform's height and all below it are the light sections {:?}",
        sections_of((1 << (site.light_section(platform) + 1)) - 1)
    );

    // (a) of the record: a run in which nobody was sent a light update looked at
    // nothing. Then what the record holds the crate to.
    assert!(
        tally.around_home > 0,
        "NOTHING WAS LOOKED AT: neither watcher was sent a light update for a chunk near \
         the builder's after the building began; see WHO STANDS WHERE before reading \
         anything into it"
    );
    assert!(
        differing.is_empty(),
        "LEVELS THE SERVER SENT DIFFER FROM THE CRATE'S: {differing:#?}"
    );
    assert!(
        of_the_latecomer.is_empty(),
        "THE LIGHT OF A CHUNK SENT AFTERWARDS DIFFERS FROM THE CRATE'S: {of_the_latecomer:#?}"
    );
}
