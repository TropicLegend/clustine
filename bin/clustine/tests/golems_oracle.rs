//! Comparisons with the official server of golems built from blocks
//! (`docs/groundwork/n7k-plan.md`, sections 2.6 and 6 item 2): an iron golem of four
//! iron blocks and a carved pumpkin, and a snow golem of two snow blocks and one, each
//! built upright and lying down; and an iron golem's blocks with a torch at a shoulder,
//! which build nothing, since the pattern's corners beside the head are air
//! (`BlockStateBase::isAir`, section 10).
//!
//! A builder places the blocks beside the spawn of a flat world, the pumpkin last, and
//! records what it is sent from the pumpkin on: every level event (the game clears each
//! cell of the pattern with event 2001 and the cleared block's state, the air of its
//! corners too, row by row), every block of the case's own cells changed, by its last
//! state (a snow golem leaves snow where it walks, which is drawn), and the golem's
//! spawn, the data and attributes with it. The cases are built eight blocks apart, so
//! that a golem of one case has not walked into the next one's cells in the seconds
//! between. Each is compared exactly between the servers,
//! relative to the case's corner: the events in order, the blocks as a set, the golem's
//! type, place (the bottom of the middle column, half a block in and 0.05 up, to the
//! bit), angles (both 0), data (the iron golem's player-made flag) and attributes. The
//! golem's movements after are not compared.
//!
//! What a mismatch prints: each case's record on each server, then the cases that
//! differ.
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test golems_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result};
use clustine_botswarm::{Bot, Heard, Oracle};
use clustine_data::{entity_types, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{AttributeSnapshot, EntityDataValue, face};

use common::shown::first_shown;
use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// The height a player stands at on both servers' flat worlds: on the grass at -61.
const GROUND: i32 = -61;

/// One block to place: the item, the block clicked relative to the case's corner (on
/// the grass, `GROUND` high, at 0), and its face.
type Placing = (i32, (i32, i32, i32), u8);

/// A case: its name and its blocks, the pumpkin last. The corner is the grass beneath
/// the builder's column, the golem built three blocks east.
struct Case {
    name: &'static str,
    blocks: &'static [Placing],
    /// The golem it builds, if any.
    golem: Option<i32>,
}

const IRON: i32 = items::IRON_BLOCK;
const SNOW: i32 = items::SNOW_BLOCK;
const PUMPKIN: i32 = items::CARVED_PUMPKIN;

const CASES: [Case; 5] = [
    Case {
        name: "an iron golem upright",
        blocks: &[
            (IRON, (3, 0, 0), face::TOP),
            (IRON, (3, 1, 0), face::TOP),
            (IRON, (3, 2, 0), face::NORTH),
            (IRON, (3, 2, 0), face::SOUTH),
            (PUMPKIN, (3, 2, 0), face::TOP),
        ],
        golem: Some(entity_types::IRON_GOLEM),
    },
    Case {
        // Its feet to the west, its head to the east, on the grass.
        name: "an iron golem lying",
        blocks: &[
            (IRON, (2, 0, 0), face::TOP),
            (IRON, (3, 0, 0), face::TOP),
            (IRON, (3, 0, -1), face::TOP),
            (IRON, (3, 0, 1), face::TOP),
            (PUMPKIN, (4, 0, 0), face::TOP),
        ],
        golem: Some(entity_types::IRON_GOLEM),
    },
    Case {
        name: "a snow golem upright",
        blocks: &[
            (SNOW, (3, 0, 0), face::TOP),
            (SNOW, (3, 1, 0), face::TOP),
            (PUMPKIN, (3, 2, 0), face::TOP),
        ],
        golem: Some(entity_types::SNOW_GOLEM),
    },
    Case {
        name: "a snow golem lying",
        blocks: &[
            (SNOW, (2, 0, 0), face::TOP),
            (SNOW, (3, 0, 0), face::TOP),
            (PUMPKIN, (4, 0, 0), face::TOP),
        ],
        golem: Some(entity_types::SNOW_GOLEM),
    },
    Case {
        // A torch on the north arm, in the corner beside the head.
        name: "an iron golem's blocks with a torch at a shoulder",
        blocks: &[
            (IRON, (3, 0, 0), face::TOP),
            (IRON, (3, 1, 0), face::TOP),
            (IRON, (3, 2, 0), face::NORTH),
            (IRON, (3, 2, 0), face::SOUTH),
            (items::TORCH, (3, 2, -1), face::TOP),
            (PUMPKIN, (3, 2, 0), face::TOP),
        ],
        golem: None,
    },
];

/// How far apart along z the cases are built, so that none touches another.
const APART: i32 = 8;

/// What a case showed from its pumpkin on, relative to its corner.
#[derive(Debug, Clone, PartialEq)]
struct Built {
    /// Every level event: the event, where, and its data.
    events: Vec<(i32, (i32, i32, i32), i32)>,
    /// Every block of the case's own cells changed, by its last state.
    blocks: BTreeMap<(i32, i32, i32), i32>,
    /// The golem's spawn: its type, place, yaw and pitch, data and attributes.
    golem: Option<Golem>,
}

#[derive(Debug, Clone, PartialEq)]
struct Golem {
    kind: i32,
    offset: (f64, f64, f64),
    angles: (u8, u8),
    values: Vec<(u8, EntityDataValue)>,
    attributes: Vec<AttributeSnapshot>,
}

/// Where a block placed by clicking `clicked` on its face `side` goes.
fn placed_at((x, y, z): (i32, i32, i32), side: u8) -> (i32, i32, i32) {
    match side {
        face::BOTTOM => (x, y - 1, z),
        face::TOP => (x, y + 1, z),
        face::NORTH => (x, y, z - 1),
        face::SOUTH => (x, y, z + 1),
        face::WEST => (x - 1, y, z),
        _ => (x + 1, y, z),
    }
}

/// Builds `case` with its corner at `corner` and records what it showed.
async fn build(bot: &mut Bot, case: &Case, corner: (i32, i32, i32)) -> Result<Built> {
    let relative = |(x, y, z): (i32, i32, i32)| (x - corner.0, y - corner.1, z - corner.2);
    let (last, rest) = case.blocks.split_last().context("a case has blocks")?;
    for (item, (dx, dy, dz), side) in rest {
        bot.put_in_hotbar(
            0,
            ItemStack {
                item: *item,
                count: 1,
            },
        )
        .await?;
        bot.select_slot(0).await?;
        let sequence = bot
            .use_item_on(corner.0 + dx, corner.1 + dy, corner.2 + dz, *side)
            .await?;
        bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
            .await?;
    }
    bot.put_in_hotbar(
        0,
        ItemStack {
            item: last.0,
            count: 1,
        },
    )
    .await?;
    bot.select_slot(0).await?;
    // From here on.
    bot.heard = Some(Vec::new());
    let from = bot.packets.as_ref().map_or(0, Vec::len);
    let (dx, dy, dz) = last.1;
    let sequence = bot
        .use_item_on(corner.0 + dx, corner.1 + dy, corner.2 + dz, last.2)
        .await?;
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await?;
    // The golem is made in the placing's own tick; a second is room for what follows.
    bot.idle(Duration::from_secs(1)).await?;
    let heard = bot.heard.take().unwrap_or_default();
    // Only the case's own cells: a snow golem leaves snow where it walks, which is
    // drawn.
    let cells: Vec<(i32, i32, i32)> = case
        .blocks
        .iter()
        .map(|(_, clicked, side)| placed_at(*clicked, *side))
        .collect();
    let mut built = Built {
        events: Vec::new(),
        blocks: BTreeMap::new(),
        golem: None,
    };
    for (_, heard) in heard {
        match heard {
            Heard::LevelEvent { event, at, data } => {
                built.events.push((event, relative(at), data));
            }
            Heard::Blocks { blocks, .. } => {
                for (at, state) in blocks {
                    let at = relative(at);
                    if cells.contains(&at) {
                        built.blocks.insert(at, state);
                    }
                }
            }
            Heard::BlockEvent { .. } | Heard::Sound { .. } => {}
        }
    }
    let packets = bot.packets.as_deref().unwrap_or_default();
    built.golem = first_shown(&packets[from.min(packets.len())..])
        .into_iter()
        .find(|entity| {
            entity.kind == entity_types::IRON_GOLEM || entity.kind == entity_types::SNOW_GOLEM
        })
        .map(|golem| {
            let mut values = golem.values.clone();
            values.sort_by_key(|(index, _)| *index);
            Golem {
                kind: golem.kind,
                offset: (
                    golem.position.0 - f64::from(corner.0),
                    golem.position.1 - f64::from(corner.1),
                    golem.position.2 - f64::from(corner.2),
                ),
                angles: (golem.yaw, golem.pitch),
                values,
                attributes: golem.attributes,
            }
        });
    Ok(built)
}

/// Every case on the server at `address`.
async fn build_all(address: &str) -> Result<Vec<Built>> {
    let mut bot = Bot::join(address, "Builder").await?;
    bot.wait_until(PATIENCE, |bot| {
        let x = (bot.location.0.floor() as i32).div_euclid(16);
        let z = (bot.location.2.floor() as i32).div_euclid(16);
        (-1..=2).all(|dx| (-1..=4).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
    })
    .await?;
    bot.record_packets();
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let mut built = Vec::new();
    for (number, case) in CASES.iter().enumerate() {
        let corner = (x, GROUND, z + APART * i32::try_from(number)?);
        // Over to the case's column, a step of no more than the game allows at once.
        while (bot.location.2 - (f64::from(corner.2) + 0.5)).abs() > 1e-9 {
            let to = (bot.location.2 + 4.0).min(f64::from(corner.2) + 0.5);
            bot.stand_at(f64::from(x) + 0.5, f64::from(GROUND + 1), to)
                .await?;
            bot.idle(Duration::from_millis(100)).await?;
        }
        built.push(build(&mut bot, case, corner).await?);
    }
    Ok(built)
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn golems_built_from_blocks_are_shown_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = build_all(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = build_all(&address).await.unwrap();
    server.stop().await;
    let mut differ = Vec::new();
    for ((case, theirs), ours) in CASES.iter().zip(&theirs).zip(&ours) {
        println!("{}:\n  official {theirs:?}\n  Clustine {ours:?}", case.name);
        assert_eq!(
            theirs.golem.as_ref().map(|golem| golem.kind),
            case.golem,
            "{}: what the official server built",
            case.name
        );
        if theirs != ours {
            differ.push(case.name);
        }
    }
    assert!(differ.is_empty(), "these differ, printed above: {differ:?}");
}
