//! The End, end to end, written from `docs/groundwork/p-plan.md`, sections 19.2 (P10b,
//! P10d), 19.5 (items 2, 7 and 15) and 19.7, `p-brief.md` 2.4 and 4.3, and
//! `p-respawn-facts.md`, by someone who did not write the code: a bot that places a
//! ring of end portal frames and fills them with eyes of ender opens the End's portal,
//! drops in and stands on the obsidian platform; there it is shown the dragon fight's
//! bar and, nearer the island's middle, the dragon; and walking off the platform into
//! the void it dies and respawns in the overworld.
//!
//! The bots have no way to ask for a respawn, so once the bot is dead the test takes its
//! raw connection, sends the client's command (`PERFORM_RESPAWN`) and reads what comes,
//! as `respawn.rs` does; the connection answers keep-alives itself.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use anyhow::{Context, Result};
use clustine_botswarm::{Bot, Connection};
use clustine_data::{blocks, entity_types};
use clustine_protocol::packets::play::{
    BossOperation, ClientCommand, ClientboundPlay, Respawn, SetHealth, SynchronizePlayerPosition,
    client_command,
};
use tokio::time::{Instant, timeout};

use dimensions::{
    END_ARRIVAL, END_ARRIVAL_YAW, END_FLOOR, END_PLATFORM, EndTravel, OVERWORLD, PATIENCE, THE_END,
    describe, fly_over_the_island, is, is_air, off_the_platform, open_an_end_portal, to_the_end,
    wait,
};

/// The level event of an End portal opened, heard by every player
/// (`EnderEyeItem.useOn`, `ServerLevel.globalLevelEvent`).
const END_PORTAL_OPENED: i32 = 1038;

/// The level event of a frame filled with an eye (`EnderEyeItem.useOn`).
const FRAME_FILLED: i32 = 1503;

/// Below the End's lowest y less the void's 64, where the void hurts.
const IN_THE_VOID: f64 = (END_FLOOR - 80) as f64;

/// Where the bot watches for the dragon: over the island, a little east of its middle,
/// within any view distance of (0, 128, 0).
const OVER_THE_ISLAND: (f64, f64) = (24.5, 0.5);

/// Fails with what the bot was sent.
fn fail(bot: &Bot, what: impl std::fmt::Display) -> ! {
    panic!("{what}\n{}", bot.report())
}

/// The bar the dragon's fight adds, if the bot has been sent it among `packets`: its
/// name as the codec shows it, its progress, colour, overlay and flags.
fn bar_added<'a>(
    packets: impl IntoIterator<Item = &'a (Instant, ClientboundPlay)>,
) -> Option<(String, f32, i32, i32, u8)> {
    packets.into_iter().find_map(|(_, packet)| match packet {
        ClientboundPlay::BossEvent(event) => match &event.operation {
            BossOperation::Add {
                name,
                progress,
                colour,
                overlay,
                properties,
            } => Some((
                format!("{name:?}"),
                *progress,
                *colour,
                *overlay,
                *properties,
            )),
            _ => None,
        },
        _ => None,
    })
}

async fn into_the_end(address: &str, name: &str) -> (Bot, EndTravel) {
    to_the_end(address, name, &mut [])
        .await
        .unwrap_or_else(|error| panic!("{name} did not come into the End: {error:#}"))
}

/// 19.2 P10b and `p-brief.md` 2.4 and 4.3: the twelfth eye opens the portal, heard as
/// level event 1038 at the ring's middle by every player; the portal takes the bot to the
/// End with a `Respawn` that keeps everything, and places it on the platform's middle,
/// (100.5, 49, 0.5) turned west, on the platform's floor of obsidian with air above.
#[tokio::test]
async fn a_bot_that_fills_an_end_portal_with_eyes_comes_onto_the_obsidian_platform() {
    let (server, address) = common::start().await;
    let (mut bot, travel) = into_the_end(&address, "EyeFiller").await;

    let changed = travel
        .packets
        .iter()
        .position(|(_, packet)| matches!(packet, ClientboundPlay::Respawn(_)))
        .unwrap_or_else(|| fail(&bot, "no Respawn"));
    let ClientboundPlay::Respawn(respawn) = &travel.packets[changed].1 else {
        unreachable!("found as a Respawn");
    };
    assert_eq!(respawn.spawn.dimension_name, THE_END);
    assert_eq!(
        respawn.data_to_keep,
        Respawn::KEEP_ALL_DATA,
        "a portal's change of level keeps everything"
    );

    assert_eq!(
        bot.location, END_ARRIVAL,
        "placed on the platform's middle: {:?}",
        bot.position
    );
    assert_eq!(
        bot.look.0, END_ARRIVAL_YAW,
        "turned west: {:?}",
        bot.position
    );
    let (x, y, z) = END_PLATFORM;
    let floor = bot.block_at(x, y - 1, z).unwrap();
    assert!(
        is(floor, blocks::OBSIDIAN),
        "the bot stands on {}",
        floor.map_or_else(|| "nothing held".to_owned(), describe)
    );
    for dx in -2..=2 {
        for dz in -2..=2 {
            let below = bot.block_at(x + dx, y - 1, z + dz).unwrap();
            assert!(
                is(below, blocks::OBSIDIAN),
                "the platform's floor at {:?} is {}",
                (x + dx, y - 1, z + dz),
                below.map_or_else(|| "not held".to_owned(), describe)
            );
            for up in 0..3 {
                let above = bot.block_at(x + dx, y + up, z + dz).unwrap();
                assert!(
                    is_air(above),
                    "above the platform at {:?} is {}",
                    (x + dx, y + up, z + dz),
                    above.map_or_else(|| "not held".to_owned(), describe)
                );
            }
        }
    }

    // The game tells the opening in the tick the last eye opens the portal, before the
    // portal's blocks are sent (`EnderEyeItem.useOn`; level events go out at once,
    // block changes at the tick's end), so a player who drops in as soon as they see
    // the portal has heard it before their change of level, at the ring's middle.
    let opening = |(when, packet): &(Instant, ClientboundPlay)| match packet {
        ClientboundPlay::LevelEvent(event) if event.event == END_PORTAL_OPENED => {
            Some((*when, event.clone()))
        }
        _ => None,
    };
    let Some((_, opened)) = travel.packets[..changed].iter().find_map(opening) else {
        // When it comes, if it does, says how late it is.
        let filled = travel
            .packets
            .iter()
            .find_map(|(when, packet)| match packet {
                ClientboundPlay::LevelEvent(event) if event.event == FRAME_FILLED => Some(*when),
                _ => None,
            })
            .unwrap_or_else(|| fail(&bot, "the last eye was not heard"));
        let heard = |bot: &Bot| {
            travel
                .packets
                .iter()
                .chain(bot.packets.iter().flatten())
                .find_map(opening)
        };
        let _ = wait(&mut bot, &mut [], PATIENCE, "the portal's opening", |bot| {
            heard(bot).is_some()
        })
        .await;
        let late = heard(&bot).map(|(when, event)| (when.duration_since(filled), event));
        fail(
            &bot,
            format!(
                "the portal's opening was not heard before the change of level; after the \
                 last eye it came {late:?}"
            ),
        );
    };
    let centre = travel.ring.centre();
    assert_eq!(
        (opened.position.x, opened.position.y, opened.position.z),
        centre,
        "the opening is heard at the ring's middle"
    );
    assert!(opened.global, "the opening is heard by every player");

    server.stop().await;
}

/// 19.5 item 8 and 19.6, `p-brief.md` 4.3: the opening of an End portal is a global
/// level event, which every player hears; the one who filled the last eye and stays
/// beside the portal hears it at the ring's middle.
#[tokio::test]
async fn a_bot_that_fills_the_last_eye_and_stays_hears_the_portal_open_at_its_middle() {
    let (server, address) = common::start().await;
    let (mut bot, _, ring) = open_an_end_portal(&address, "Listener", &mut [])
        .await
        .unwrap_or_else(|error| panic!("the portal did not open: {error:#}"));
    let opening = |bot: &Bot| {
        bot.packets
            .iter()
            .flatten()
            .find_map(|(_, packet)| match packet {
                ClientboundPlay::LevelEvent(event) if event.event == END_PORTAL_OPENED => {
                    Some(event.clone())
                }
                _ => None,
            })
    };
    let heard = |bot: &Bot| opening(bot).is_some();
    if let Err(error) = wait(&mut bot, &mut [], PATIENCE, "the portal's opening", heard).await {
        fail(&bot, format!("{error:#}"));
    }
    let opened = opening(&bot).expect("heard just now");
    // Shown, not judged here: how long after the last eye's own event the opening came,
    // which the game sends in the same tick.
    let when = |wanted: i32| {
        bot.packets
            .iter()
            .flatten()
            .find_map(|(when, packet)| match packet {
                ClientboundPlay::LevelEvent(event) if event.event == wanted => Some(*when),
                _ => None,
            })
    };
    if let (Some(filled), Some(heard)) = (when(FRAME_FILLED), when(END_PORTAL_OPENED)) {
        println!(
            "the opening came {:?} after the last eye",
            heard.duration_since(filled)
        );
    }
    let centre = ring.centre();
    assert_eq!(
        (opened.position.x, opened.position.y, opened.position.z),
        centre,
        "the opening is heard at the ring's middle"
    );
    assert!(opened.global, "the opening is heard by every player");
    server.stop().await;
}

/// 19.5 item 7 and the fight: a player within 192 blocks of (0, 128, 0) in the End is
/// shown the dragon's bar, named after the dragon (`EnderDragonFight.EVENT_DISPLAY_NAME`,
/// `entity.minecraft.ender_dragon`), and the dragon the fight makes at (0, 128, 0) once
/// it is within the bot's view.
#[tokio::test]
async fn a_bot_in_the_end_is_shown_the_dragon_fight_s_bar_and_the_dragon() {
    let (server, address) = common::start().await;
    let (mut bot, travel) = into_the_end(&address, "BarWatcher").await;

    // The platform is within the bar's reach: it comes there, before the bot moves.
    let seen =
        |bot: &Bot| bar_added(travel.packets.iter().chain(bot.packets.iter().flatten())).is_some();
    if let Err(error) = wait(&mut bot, &mut [], PATIENCE, "the dragon's bar", seen).await {
        fail(&bot, format!("{error:#}"));
    }
    let (name, ..) = bar_added(travel.packets.iter().chain(bot.packets.iter().flatten()))
        .expect("seen just now");
    assert!(
        name.contains("entity.minecraft.ender_dragon"),
        "the bar's name: {name}"
    );

    if let Err(error) = fly_over_the_island(&mut bot, &mut [], OVER_THE_ISLAND).await {
        fail(&bot, format!("{error:#}"));
    }
    let dragon = |bot: &Bot| {
        bot.entities
            .values()
            .any(|entity| entity.kind == entity_types::ENDER_DRAGON)
    };
    if let Err(error) = wait(&mut bot, &mut [], PATIENCE, "the dragon", dragon).await {
        fail(&bot, format!("{error:#}"));
    }
    server.stop().await;
}

/// What the client was sent once it asked for its respawn: the `Respawn`, and after it
/// the place and the health.
#[derive(Default)]
struct AfterTheAsk {
    respawn: Option<Respawn>,
    place: Option<SynchronizePlayerPosition>,
    health: Option<SetHealth>,
}

/// Reads what the server sends `connection` until a `Respawn` has come and, after it, a
/// place and a health of more than nothing.
async fn read_the_respawn(connection: &mut Connection) -> Result<AfterTheAsk> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = AfterTheAsk::default();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = timeout(left, connection.read_frame())
            .await
            .context("the respawn did not come in time")??;
        match ClientboundPlay::decode(&frame)? {
            ClientboundPlay::Respawn(respawn) => {
                assert!(seen.respawn.is_none(), "a second Respawn: {respawn:?}");
                seen.respawn = Some(respawn);
            }
            ClientboundPlay::SynchronizePlayerPosition(place) if seen.respawn.is_some() => {
                seen.place.get_or_insert(place);
            }
            ClientboundPlay::SetHealth(health) if seen.respawn.is_some() && health.health > 0.0 => {
                seen.health.get_or_insert(health);
            }
            _ => {}
        }
        if seen.place.is_some() && seen.health.is_some() {
            return Ok(seen);
        }
    }
}

/// 19.5 item 2 and 19.7, `p-respawn-facts.md` 3 and 5: off the platform and below the
/// End's floor less 64 the void hurts the bot, creative as it is, until it dies with the
/// void's message; `PerformRespawn` then sends it to the overworld with a `Respawn` that
/// keeps nothing, at the world spawn where it joined (it has no bed), with full health.
#[tokio::test]
async fn a_bot_that_walks_off_the_platform_dies_in_the_void_and_respawns_in_the_overworld() {
    let (server, address) = common::start().await;
    let (mut bot, travel) = into_the_end(&address, "VoidWalker").await;

    if let Err(error) = off_the_platform(&mut bot, &mut [], IN_THE_VOID).await {
        fail(&bot, format!("{error:#}"));
    }
    let died = |bot: &Bot| !bot.deaths.is_empty();
    if let Err(error) = wait(&mut bot, &mut [], PATIENCE, "the void's death", died).await {
        fail(&bot, format!("{error:#}"));
    }
    let health = bot.health.as_ref().expect("the health was told").health;
    assert_eq!(health, 0.0);
    let message = format!("{:?}", bot.deaths[0].message);
    assert!(message.contains("death.attack.outOfWorld"), "{message}");
    assert_eq!(bot.deaths.len(), 1);

    let mut connection = bot.into_connection();
    connection
        .write(&ClientCommand {
            action: client_command::PERFORM_RESPAWN,
        })
        .await
        .unwrap();
    let seen = read_the_respawn(&mut connection).await.unwrap();
    let respawn = seen.respawn.expect("a Respawn");
    assert_eq!(respawn.data_to_keep, 0, "a death's respawn keeps nothing");
    assert_eq!(respawn.spawn.dimension_name, OVERWORLD);
    let place = seen.place.expect("a place");
    assert_eq!(
        (place.x, place.y, place.z),
        travel.entered,
        "respawned at the world spawn, where the bot joined"
    );
    assert_eq!(place.relative_flags, 0, "absolute");
    let health = seen.health.expect("a health");
    assert_eq!(health.health, 20.0);
    drop(connection);
    server.stop().await;
}
