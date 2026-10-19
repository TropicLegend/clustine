//! Comparisons with the official server of TNT (step N6c, `docs/groundwork/n6-plan.md`,
//! section 6 item 11): the primed TNT a client is shown, with its data, and the
//! explosion packet. The bots share Clustine's codec, so these packets are only really
//! checked here.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test tnt_oracle -- --ignored --nocapture --test-threads 1
//! ```
//!
//! The rays draw at random, and so does the way primed TNT jumps as it is lit, so how
//! many positions the rays reached and where in its block the TNT went off differ from
//! run to run on either server and are not compared exactly; nor is the push, whose size
//! depends on where the player stands, which the two servers spawn them at differently.
//! Whether there is a push is compared, and everything else of the packet.

mod common;

use std::time::Duration;

use clustine_botswarm::{Bot, Oracle};
use clustine_data::{blocks, entity_types, items};
use clustine_protocol::packets::play::{Explode, face};

use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// What a run showed: the primed TNT's kind and its data's values as first shown, and
/// the explosion with its centre relative to the TNT's block.
#[derive(Debug)]
struct Ran {
    primed_kind: i32,
    explosion: Explode,
    centre_in_block: [f64; 3],
}

async fn run(address: &str) -> Ran {
    let mut bot = Bot::join(address, "Lighter").await.unwrap();
    bot.wait_for_chunks(9, PATIENCE).await.unwrap();
    // Three blocks east of the bot, on the ground.
    let (x, z) = (
        bot.location.0.floor() as i32 + 3,
        bot.location.2.floor() as i32,
    );
    bot.take_from_creative_inventory(0, items::TNT)
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
    bot.use_item_on(x, -61, z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(x, -60, z).unwrap() == Some(i32::from(blocks::TNT.0))
    })
    .await
    .unwrap();
    bot.take_from_creative_inventory(1, items::FLINT_AND_STEEL)
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();
    bot.use_item_on(x, -60, z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.entities
            .values()
            .any(|entity| entity.kind == entity_types::TNT)
    })
    .await
    .unwrap();
    let primed_kind = bot
        .entities
        .values()
        .find(|entity| entity.kind == entity_types::TNT)
        .map(|entity| entity.kind)
        .expect("shown");
    bot.wait_until(PATIENCE, |bot| !bot.explosions.is_empty())
        .await
        .unwrap();
    let explosion = bot.explosions[0].clone();
    let [cx, cy, cz] = explosion.centre;
    let centre_in_block = [cx - f64::from(x), cy - f64::from(-60), cz - f64::from(z)];
    Ran {
        primed_kind,
        explosion,
        centre_in_block,
    }
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn tnt_and_its_explosion_are_sent_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let theirs = run(oracle.address()).await;
    let ours = run(&address).await;
    server.stop().await;
    drop(oracle);
    println!("official: {theirs:?}\nClustine: {ours:?}");

    let (their, our) = (&theirs.explosion, &ours.explosion);
    let mut differences = Vec::new();
    let mut differ = |what: &str, same: bool| {
        if !same {
            differences.push(what.to_owned());
        }
    };
    differ(
        "the primed TNT's kind",
        theirs.primed_kind == ours.primed_kind,
    );
    // Primed TNT jumps off in a direction drawn at random as it is lit, so its x and z
    // are compared as within its block, and its height, which the game fixes, exactly.
    let [their_x, their_y, their_z] = theirs.centre_in_block;
    let [our_x, our_y, our_z] = ours.centre_in_block;
    differ("the centre's height", their_y == our_y);
    differ(
        "the centre within the block",
        [their_x, their_z, our_x, our_z]
            .iter()
            .all(|part| (0.0..1.0).contains(part)),
    );
    differ("the radius", their.radius == our.radius);
    differ(
        "whether there is a push",
        their.knockback.is_some() == our.knockback.is_some(),
    );
    differ("the particle", their.particle == our.particle);
    differ("the sound", their.sound == our.sound);
    differ(
        "the block particles",
        their.block_particles == our.block_particles,
    );
    differ(
        "whether the sound plays",
        their.play_sound == our.play_sound,
    );
    differ(
        "a count of positions reached",
        their.block_count > 0 && our.block_count > 0,
    );
    assert!(differences.is_empty(), "differ: {differences:?}");
}
