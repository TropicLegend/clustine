//! A death in the void and the respawn after it, end to end, written from
//! `docs/groundwork/p-plan.md`, sections 19.5 (items 1 and 2) and 19.7, and the game's
//! `PlayerList.respawn` (`docs/groundwork/p-respawn-facts.md`, section 5) by someone who
//! did not write the code: a player who goes below the world is hurt by the void until
//! they die, is shown their death, asks for their respawn and is placed at the world
//! spawn, with full health and a `Respawn` that keeps nothing.
//!
//! The bots have no way to ask for a respawn, so once the bot is dead the test takes its
//! raw connection, sends the client's command (`PERFORM_RESPAWN`) and reads what comes,
//! answering keep-alives as a client does.

mod common;

use std::time::Duration;

use anyhow::{Context, Result};
use clustine_botswarm::{Bot, Connection};
use clustine_protocol::packets::play::{
    ClientCommand, ClientboundKeepAlive, ClientboundPlay, Respawn, ServerboundKeepAlive, SetHealth,
    SynchronizePlayerPosition, client_command,
};
use tokio::time::{Instant, timeout};

const PATIENCE: Duration = Duration::from_secs(60);

/// Far below the flat world's lowest y, -64, less the void's 64.
const IN_THE_VOID: f64 = -200.0;

/// What the client was sent once it asked for its respawn: the `Respawn`, and after it
/// the place and the health.
#[derive(Default)]
struct AfterTheAsk {
    respawn: Option<Respawn>,
    place: Option<SynchronizePlayerPosition>,
    health: Option<SetHealth>,
}

/// Reads what the server sends `connection` until a `Respawn` has come and, after it, a
/// place and a health of more than nothing, answering keep-alives.
async fn read_the_respawn(connection: &mut Connection) -> Result<AfterTheAsk> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = AfterTheAsk::default();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = timeout(left, connection.read_frame())
            .await
            .context("the respawn did not come in time")??;
        match ClientboundPlay::decode(&frame)? {
            ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id }) => {
                connection.write(&ServerboundKeepAlive { id }).await?;
            }
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

/// 19.5 item 2 and 19.7: the void hurts until the player dies, shown with the void's
/// message; `PerformRespawn` places them at the world spawn, where a newcomer enters,
/// with full health and `Respawn` keeping nothing (`PlayerList.respawn`, keep 0 after a
/// death).
#[tokio::test]
async fn a_player_who_falls_into_the_void_dies_and_respawns_at_the_world_spawn() {
    let (server, address) = common::start().await;
    let mut bot = Bot::join(&address, "Faller").await.unwrap();
    let entered = bot.entered(PATIENCE).await.unwrap();
    let (x, _, z) = entered.location;
    bot.stand_at(x, IN_THE_VOID, z).await.unwrap();
    bot.wait_until(PATIENCE, |bot| !bot.deaths.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error}: the void did not kill: {}", bot.report()));
    let health = bot.health.as_ref().expect("the health was told").health;
    assert_eq!(health, 0.0);
    // Each hurt's `DamageEvent` is not shown to the player who takes it: a finding of
    // the edge's tests (`fanout/scenarios/respawn_specification.rs`).
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
    assert_eq!(respawn.spawn.dimension_name, "minecraft:overworld");
    let place = seen.place.expect("a place");
    assert_eq!((place.x, place.y, place.z), entered.location);
    assert_eq!(place.relative_flags, 0, "absolute");
    let health = seen.health.expect("a health");
    assert_eq!(health.health, 20.0);
    drop(connection);
    server.stop().await;
}
