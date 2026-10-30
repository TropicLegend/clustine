//! The travel through a Nether portal end to end (step P9, `docs/groundwork/p-plan.md`,
//! section 16.6): a bot builds a frame of obsidian, lights it with flint and steel and
//! walks in; it is sent `Respawn` into the Nether and placed standing in a portal there;
//! it steps out and back in, and is sent back to the overworld, placed in the portal it
//! came through.

mod common;

use std::time::Duration;

use clustine_botswarm::Bot;
use clustine_data::{BlockState, blocks, items};
use clustine_protocol::packets::play::face;

use common::{VIEW_DISTANCE, start, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// The flat world's floor: the grass, whose top the bot stands on.
const GRASS: i32 = -61;

/// The frame's plane, and the x of its inside's two columns.
const Z: i32 = 2;
const INSIDE: [i32; 2] = [1, 2];

fn is(state: Option<i32>, block: BlockState) -> bool {
    state.is_some_and(|id| {
        u16::try_from(id).is_ok_and(|id| {
            let state = BlockState(id);
            state.block_id().is_some() && state.block_id() == block.block_id()
        })
    })
}

async fn place(bot: &mut Bot, against: (i32, i32, i32), side: u8, at: (i32, i32, i32)) {
    let (x, y, z) = against;
    bot.use_item_on(x, y, z, side).await.unwrap();
    let (x, y, z) = at;
    bot.wait_until(PATIENCE, |bot| {
        is(bot.block_at(x, y, z).unwrap(), blocks::OBSIDIAN)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: no obsidian came at {at:?}"));
}

/// A frame of obsidian across x, its inside at x 1 and 2 from one above the grass's
/// top up three, with its corners, built from the ground up.
async fn build_frame(bot: &mut Bot) {
    bot.take_from_creative_inventory(0, items::OBSIDIAN)
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
    let bottom = GRASS + 1;
    for x in 0..=3 {
        place(bot, (x, GRASS, Z), face::TOP, (x, bottom, Z)).await;
    }
    for x in [0, 3] {
        for y in bottom + 1..=bottom + 4 {
            place(bot, (x, y - 1, Z), face::TOP, (x, y, Z)).await;
        }
    }
    let top = bottom + 4;
    place(bot, (0, top, Z), face::EAST, (1, top, Z)).await;
    place(bot, (3, top, Z), face::WEST, (2, top, Z)).await;
}

fn in_portal(bot: &Bot) -> bool {
    let (x, y, z) = bot.location;
    let feet = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    bot.block_at(feet.0, feet.1, feet.2)
        .is_ok_and(|state| is(state, blocks::NETHER_PORTAL))
}

#[tokio::test]
async fn a_bot_walks_through_a_portal_into_the_nether_and_back_beside_where_it_left() {
    let (_server, address) = start().await;
    let mut bot = Bot::join(&address, "Traveller").await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    assert_eq!(bot.info.login.spawn.dimension_name, "minecraft:overworld");

    build_frame(&mut bot).await;
    bot.take_from_creative_inventory(1, items::FLINT_AND_STEEL)
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();
    bot.use_item_on(INSIDE[0], GRASS + 1, Z, face::TOP)
        .await
        .unwrap();
    bot.wait_until(PATIENCE, |bot| {
        INSIDE.iter().all(|x| {
            (GRASS + 2..=GRASS + 4)
                .all(|y| is(bot.block_at(*x, y, Z).unwrap(), blocks::NETHER_PORTAL))
        })
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the frame was never lit"));

    // Into the portal: a creative player goes on their first tick inside.
    let entry = (2.0, f64::from(GRASS + 2), f64::from(Z) + 0.5);
    bot.stand_at(entry.0, entry.1, entry.2).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.info.login.spawn.dimension_name == "minecraft:the_nether"
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the bot was never sent into the Nether"));
    // Placed, in the Nether's chunks, standing in a portal there.
    bot.wait_until(PATIENCE, |bot| bot.position.is_some() && in_portal(bot))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the bot was never placed in a portal of the Nether: at {:?}",
                bot.location
            )
        });
    let (nx, ny, nz) = bot.location;

    // Out of the portal, so that its cooldown runs down, and back in. The cooldown is a
    // player's 10 ticks, which nothing shows a client: the bot stays out for 40, and
    // tries again, out and in, should its step out have been missed.
    let mut back = false;
    for _ in 0..10 {
        bot.stand_at(nx, ny, nz + 2.0).await.unwrap();
        bot.idle(Duration::from_secs(2)).await.unwrap();
        bot.stand_at(nx, ny, nz).await.unwrap();
        let sent = bot
            .wait_until(Duration::from_secs(5), |bot| {
                bot.info.login.spawn.dimension_name == "minecraft:overworld"
            })
            .await;
        if sent.is_ok() {
            back = true;
            break;
        }
    }
    assert!(back, "the bot was never sent back to the overworld");
    // Beside where it left: in the portal it came through.
    bot.wait_until(PATIENCE, |bot| bot.position.is_some() && in_portal(bot))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the bot was never placed back in a portal: at {:?}",
                bot.location
            )
        });
    let (x, _, z) = bot.location;
    assert!(
        (1.0..3.0).contains(&x) && (f64::from(Z)..f64::from(Z) + 1.0).contains(&z),
        "back at {:?}",
        bot.location
    );
}

/// A second player through the same portal while the first is in the Nether already:
/// both are placed there, and the edge counts each one's view of the Nether's chunks
/// once (its invariants hold in a build with its checks).
#[tokio::test]
async fn two_bots_one_after_the_other_both_come_into_the_nether() {
    let (_server, address) = start().await;
    let mut first = Bot::join(&address, "First").await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    first.wait_for_chunks(count, PATIENCE).await.unwrap();
    build_frame(&mut first).await;
    first
        .take_from_creative_inventory(1, items::FLINT_AND_STEEL)
        .await
        .unwrap();
    first.select_slot(1).await.unwrap();
    first
        .use_item_on(INSIDE[0], GRASS + 1, Z, face::TOP)
        .await
        .unwrap();
    first
        .wait_until(PATIENCE, |bot| {
            is(
                bot.block_at(INSIDE[0], GRASS + 2, Z).unwrap(),
                blocks::NETHER_PORTAL,
            )
        })
        .await
        .unwrap();
    let entry = (2.0, f64::from(GRASS + 2), f64::from(Z) + 0.5);
    for (index, bot) in [&mut first].into_iter().enumerate() {
        bot.stand_at(entry.0, entry.1, entry.2).await.unwrap();
        bot.wait_until(PATIENCE, |bot| bot.position.is_some() && in_portal(bot))
            .await
            .unwrap_or_else(|error| panic!("{error}: bot {index} was never placed"));
    }
    let mut second = Bot::join(&address, "Second").await.unwrap();
    second.wait_for_chunks(count, PATIENCE).await.unwrap();
    second.stand_at(entry.0, entry.1, entry.2).await.unwrap();
    second
        .wait_until(PATIENCE, |bot| {
            bot.info.login.spawn.dimension_name == "minecraft:the_nether"
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the second never went"));
    second
        .wait_until(PATIENCE, |bot| bot.position.is_some() && in_portal(bot))
        .await
        .unwrap_or_else(|error| panic!("{error}: the second was never placed"));
    // The first stays in the Nether, connected, seeing it.
    first.idle(Duration::from_millis(500)).await.unwrap();
    assert_eq!(
        first.info.login.spawn.dimension_name,
        "minecraft:the_nether"
    );
}
