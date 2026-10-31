//! Sculk sensors end to end (step N6f): a block placed near a sensor sets it off, and
//! everyone near is shown the vibration travelling to it; and the comparison with the
//! official server of what a client is shown of it (ignored, as every comparison: it needs
//! Java, the jar and agreement to the Minecraft EULA).
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test sculk -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::Duration;

use clustine_botswarm::{Bot, Oracle};
use clustine_data::{BlockState, items};
use clustine_protocol::packets::play::{LevelParticles, Particle, face};

use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// What a run showed: the particle, with its start relative to the placed block, and
/// whether the sensor was seen active.
#[derive(Debug)]
struct Ran {
    particle: LevelParticles,
    start_in_block: [f64; 3],
    to_in_offsets: (i32, i32, i32),
    active: bool,
}

fn phase(state: Option<i32>) -> Option<&'static str> {
    BlockState(u16::try_from(state?).ok()?).property("sculk_sensor_phase")
}

async fn run(address: &str) -> Ran {
    let mut bot = Bot::join(address, "Listener").await.unwrap();
    bot.wait_for_chunks(9, PATIENCE).await.unwrap();
    let (x, z) = (
        bot.location.0.floor() as i32 + 2,
        bot.location.2.floor() as i32,
    );
    // Ground the bot does not stand on: the sensor two blocks east of it.
    bot.take_from_creative_inventory(0, items::SCULK_SENSOR)
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
    bot.use_item_on(x, -61, z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        phase(bot.block_at(x, -60, z).unwrap()) == Some("inactive")
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: no sensor"));
    // Once the server has handled the placing: if the sensor heard itself placed, it
    // is let go through its cooldown first.
    let sequence = bot.use_item_on(x - 1, -61, z - 1, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .unwrap();
    if !bot.particles.is_empty() {
        bot.wait_until(PATIENCE, |bot| {
            phase(bot.block_at(x, -60, z).unwrap()) == Some("cooldown")
        })
        .await
        .unwrap();
        bot.wait_until(PATIENCE, |bot| {
            phase(bot.block_at(x, -60, z).unwrap()) == Some("inactive")
        })
        .await
        .unwrap();
    }
    bot.particles.clear();
    bot.take_from_creative_inventory(1, items::STONE)
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();
    // Five blocks west of the sensor, three from the bot, within its reach.
    let placed = (x - 5, -60, z);
    bot.use_item_on(placed.0, placed.1 - 1, placed.2, face::TOP)
        .await
        .unwrap();
    bot.wait_until(PATIENCE, |bot| !bot.particles.is_empty())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: no vibration was shown; the sensor is {:?}, the stone {:?}",
                phase(bot.block_at(x, -60, z).unwrap()),
                bot.block_at(placed.0, placed.1, placed.2)
            )
        });
    let particle = bot.particles[0];
    let Particle::Vibration { to, .. } = particle.particle else {
        panic!("the first particle shown is not a vibration: {particle:?}");
    };
    let start_in_block = [
        particle.position[0] - f64::from(placed.0),
        particle.position[1] - f64::from(placed.1),
        particle.position[2] - f64::from(placed.2),
    ];
    let active = bot
        .wait_until(PATIENCE, |bot| {
            phase(bot.block_at(x, -60, z).unwrap()) == Some("active")
        })
        .await
        .is_ok();
    Ran {
        particle,
        start_in_block,
        to_in_offsets: (to.x - x, to.y - -60, to.z - z),
        active,
    }
}

#[tokio::test]
async fn a_block_placed_near_a_sensor_sets_it_off_and_its_vibration_is_shown() {
    let (_server, address) = start().await;
    let ran = run(&address).await;
    assert!(ran.active, "{ran:?}");
    assert_eq!(ran.to_in_offsets, (0, 0, 0), "it travels to the sensor");
    let Particle::Vibration { arrival, .. } = ran.particle.particle else {
        panic!("a vibration was shown: {ran:?}");
    };
    assert_eq!(arrival, 5, "floor of the distance, five blocks");
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_vibration_is_sent_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let theirs = run(oracle.address()).await;
    let ours = run(&address).await;
    server.stop().await;
    drop(oracle);
    println!("official: {theirs:?}\nClustine: {ours:?}");
    assert_eq!(theirs.active, ours.active);
    assert_eq!(theirs.to_in_offsets, ours.to_in_offsets);
    assert_eq!(theirs.start_in_block, ours.start_in_block);
    let strip = |particle: LevelParticles| LevelParticles {
        position: [0.0; 3],
        particle: match particle.particle {
            Particle::Vibration { arrival, .. } => Particle::Vibration {
                to: clustine_protocol::codec::Position { x: 0, y: 0, z: 0 },
                arrival,
            },
            other => other,
        },
        ..particle
    };
    assert_eq!(strip(theirs.particle), strip(ours.particle));
}
