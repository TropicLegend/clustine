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

use clustine_botswarm::{Bot, Ending, Entry, Oracle, SameName, Wire, same_name_twice};
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
async fn reached(bots: &mut [&mut Bot], what: &str, mut done: impl FnMut(&Bot) -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if done(&*bots[0]) {
            println!("  reached: {what}");
            return true;
        }
        if Instant::now() >= deadline {
            println!("  NOT REACHED within {PATIENCE:?}: {what}");
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
