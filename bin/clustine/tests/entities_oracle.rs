//! Comparisons with the official server of what a client is sent of an entity other
//! than a player as it moves (step N4d, `docs/groundwork/n4-plan.md`, sections 4 and 7;
//! `docs/groundwork/n4-brief.md`, section 9): an item thrown onto a slab, where it lands
//! and is picked up by its thrower after its two seconds; an item thrown into water,
//! where it is corrected every tick; and a block of sand that falls two blocks. A
//! watcher records, as they arrive, the packets about the entity and the blocks of the
//! case, each at its tick counted from the entity's spawn; the two servers' timelines
//! are to hold the same packets, by layout (each is read whole by the codec and its
//! kind and flags kept), in the same order, each within a tick of the other's.
//!
//! A throw draws its motion at random on both servers, so where a thrown item lands,
//! and when, differs between runs by a tick or two: the item cases compare what the
//! rules fix whatever the draw (the updates at the counts of 20 and 60 from the spawn,
//! the landing sent whole at the update after it, the pick-up at 40 ticks, a move every
//! tick in water), not every tick. A resting item's `set_entity_motion` is left out:
//! Clustine reports a resting item's motion only with its committed changes (the
//! plan's section 7). The sand draws nothing and is compared tick by tick.
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine, with
//! what each server sent printed, with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test entities_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clustine_botswarm::{Bot, Connection, Oracle};
use clustine_data::{entity_types, items};
use clustine_protocol::codec::Position;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientboundKeepAlive, ClientboundPlay, PlayerAction, ServerboundKeepAlive, face, player_action,
};
use tokio::time::{Instant, timeout};

use common::start;

/// How long a bot waits for what it should be sent soon.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long the watcher records after a case has begun: the item's pick-up comes at two
/// seconds, the sand lands in under one.
const RECORDED: Duration = Duration::from_secs(5);

/// The height a player stands at on both servers' flat worlds: on the grass at -61.
const GROUND: i32 = -61;

/// What the watcher was sent, as far as it is compared.
#[derive(Debug, Clone, PartialEq)]
enum Sent {
    /// `add_entity`: the type, the spawn's data (a falling block's block).
    Spawn {
        kind: i32,
        data: i32,
    },
    /// `set_entity_data`: the indices and serializers of its values.
    Data(Vec<u8>),
    Motion,
    /// `move_entity_pos`, with the count of its steps and whether it is on the ground.
    Move {
        steps: usize,
        on_ground: bool,
    },
    /// `entity_position_sync`, and whether it is on the ground.
    Whole {
        on_ground: bool,
    },
    Taken {
        count: i32,
    },
    Removed,
    /// A block of the case changed, by `block_update` or in a `section_blocks_update`.
    Block((i32, i32, i32), i32),
    Transient((i32, i32, i32), i32),
}

/// Each packet with the tick it came in, counted from the entity's spawn.
type Timeline = Vec<(i64, Sent)>;

/// Reads what `watching` is sent for [`RECORDED`] from now, answering keep-alives, and
/// makes the timeline of the first entity of type `kind` to appear and of the blocks
/// within four of `around`.
async fn record(watching: &mut Connection, kind: i32, around: (i32, i32, i32)) -> Result<Timeline> {
    let deadline = Instant::now() + RECORDED;
    let near = |at: (i32, i32, i32)| {
        (at.0 - around.0).abs() <= 4 && (at.1 - around.1).abs() <= 4 && (at.2 - around.2).abs() <= 4
    };
    let mut entity = None;
    let mut spawned_at = None;
    let mut seen: Vec<(Instant, Sent)> = Vec::new();
    loop {
        let Ok(frame) = timeout(
            deadline.saturating_duration_since(Instant::now()),
            watching.read_frame(),
        )
        .await
        else {
            break;
        };
        let frame = frame?;
        let now = Instant::now();
        let sent = match ClientboundPlay::decode(&frame) {
            Ok(ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id })) => {
                watching.write(&ServerboundKeepAlive { id }).await?;
                continue;
            }
            Ok(ClientboundPlay::SpawnEntity(spawn)) if spawn.kind == kind && entity.is_none() => {
                entity = Some(spawn.entity_id);
                spawned_at = Some(now);
                Sent::Spawn {
                    kind: spawn.kind,
                    data: spawn.data,
                }
            }
            Ok(ClientboundPlay::SetEntityData(data)) if Some(data.entity_id) == entity => {
                Sent::Data(data.values.iter().map(|(index, _)| *index).collect())
            }
            Ok(ClientboundPlay::SetEntityMotion(motion)) if Some(motion.entity_id) == entity => {
                Sent::Motion
            }
            Ok(ClientboundPlay::MoveEntityPos(moved)) if Some(moved.entity_id) == entity => {
                Sent::Move {
                    steps: match moved.delta {
                        clustine_protocol::packets::play::VecDelta::Linear(_) => 0,
                        clustine_protocol::packets::play::VecDelta::Stepped(steps) => steps.len(),
                    },
                    on_ground: moved.on_ground,
                }
            }
            Ok(ClientboundPlay::SyncEntityPosition(moved)) if Some(moved.entity_id) == entity => {
                Sent::Whole {
                    on_ground: moved.on_ground,
                }
            }
            Ok(ClientboundPlay::TakeItemEntity(taken)) if Some(taken.item_id) == entity => {
                Sent::Taken { count: taken.count }
            }
            Ok(ClientboundPlay::RemoveEntities(removed))
                if entity.is_some_and(|entity| removed.entity_ids.contains(&entity)) =>
            {
                Sent::Removed
            }
            Ok(ClientboundPlay::BlockUpdate(update)) => {
                let Position { x, y, z } = update.position;
                if !near((x, y, z)) {
                    continue;
                }
                Sent::Block((x, y, z), update.state)
            }
            Ok(ClientboundPlay::SectionBlocksUpdate(update)) => {
                let (sx, sy, sz) = update.section;
                for ((x, y, z), state) in update.blocks {
                    let at = (
                        sx * 16 + i32::from(x),
                        sy * 16 + i32::from(y),
                        sz * 16 + i32::from(z),
                    );
                    if near(at) {
                        seen.push((now, Sent::Block(at, state)));
                    }
                }
                continue;
            }
            Ok(ClientboundPlay::AddTransientBlock(transient)) => {
                let Position { x, y, z } = transient.position;
                Sent::Transient((x, y, z), transient.state)
            }
            Ok(_) => continue,
            Err(error) => {
                // A packet of an entity's tracking that the codec cannot read whole is a
                // layout the two do not agree on.
                let id = frame.first().copied().map(i32::from);
                let tracked = [
                    clientbound::MOVE_ENTITY_POS,
                    clientbound::MOVE_ENTITY_POS_ROT,
                    clientbound::SET_ENTITY_MOTION,
                    clientbound::TAKE_ITEM_ENTITY,
                    clientbound::ADD_TRANSIENT_BLOCK,
                    clientbound::ENTITY_POSITION_SYNC,
                ];
                ensure!(
                    !id.is_some_and(|id| tracked.contains(&id)),
                    "a packet {id:?} the codec cannot read: {error}"
                );
                continue;
            }
        };
        seen.push((now, sent));
    }
    let spawned_at = spawned_at.context("no entity of the case appeared")?;
    let tick = |when: Instant| {
        let since = when.saturating_duration_since(spawned_at).as_secs_f64()
            - spawned_at.saturating_duration_since(when).as_secs_f64();
        (since * 20.0).round() as i64
    };
    Ok(seen
        .into_iter()
        .map(|(when, sent)| (tick(when), sent))
        .collect())
}

/// Whether two timelines hold the same packets in the same order, each within a tick of
/// the other's; the first difference otherwise.
fn compare(ours: &Timeline, theirs: &Timeline) -> Result<()> {
    for (index, (our, their)) in ours.iter().zip(theirs).enumerate() {
        ensure!(
            our.1 == their.1 && (our.0 - their.0).abs() <= 1,
            "packet {index}: Clustine sent {our:?}, the official server {their:?}"
        );
    }
    ensure!(
        ours.len() == theirs.len(),
        "Clustine sent {} packets, the official server {}",
        ours.len(),
        theirs.len()
    );
    Ok(())
}

/// A builder and a watcher in `address`'s world, with the chunks around the builder.
async fn two_bots(address: &str) -> Result<(Bot, Bot)> {
    let mut builder = Bot::join(address, "Builder").await?;
    let mut watcher = Bot::join(address, "Watcher").await?;
    watcher
        .wait_until(PATIENCE, |bot| bot.seen_player("Builder").is_some())
        .await?;
    builder
        .wait_until(PATIENCE, |bot| {
            let x = (bot.location.0.floor() as i32).div_euclid(16);
            let z = (bot.location.2.floor() as i32).div_euclid(16);
            let around = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (x + dx, z + dz)));
            bot.seen_player("Watcher").is_some()
                && around
                    .into_iter()
                    .all(|chunk| bot.chunks.contains_key(&chunk))
        })
        .await?;
    Ok((builder, watcher))
}

/// The block the builder stands in.
fn feet(bot: &Bot) -> (i32, i32, i32) {
    (
        bot.location.0.floor() as i32,
        GROUND + 1,
        bot.location.2.floor() as i32,
    )
}

/// Places `item` from the hotbar's slot 1 against `side` of the block at `at`, and
/// waits until the builder is shown `placed` there.
async fn place(
    bot: &mut Bot,
    item: i32,
    at: (i32, i32, i32),
    side: u8,
    placed: (i32, i32, i32),
) -> Result<()> {
    bot.take_from_creative_inventory(1, item).await?;
    bot.select_slot(1).await?;
    bot.use_item_on(at.0, at.1, at.2, side).await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(placed.0, placed.1, placed.2)
            .is_ok_and(|state| state.is_some_and(|state| state != 0))
    })
    .await
}

/// Throws a stone looking straight down from where the builder stands.
async fn throw_down(mut builder: Bot) -> Result<Connection> {
    builder
        .take_from_creative_inventory(0, items::STONE)
        .await?;
    builder.select_slot(0).await?;
    let at = builder.location;
    builder.fly_to(at, (0.0, 90.0), 1.0).await?;
    let mut throwing = builder.into_connection();
    throwing
        .write(&PlayerAction {
            status: player_action::DROP_ITEM,
            position: Position { x: 0, y: 0, z: 0 },
            face: 0,
            sequence: 0,
        })
        .await?;
    Ok(throwing)
}

/// The builder stands on a stone slab beside them and throws a stone straight down onto
/// it, and is still there when the item may be picked up.
async fn onto_a_slab(address: &str) -> Result<Timeline> {
    let (mut builder, watcher) = two_bots(address).await?;
    let (x, y, z) = feet(&builder);
    let slab = (x + 1, y, z);
    place(
        &mut builder,
        items::STONE_SLAB,
        (x + 1, GROUND, z),
        face::TOP,
        slab,
    )
    .await?;
    builder
        .stand_at(
            f64::from(x + 1) + 0.5,
            f64::from(y) + 0.5,
            f64::from(z) + 0.5,
        )
        .await?;
    builder.idle(Duration::from_millis(200)).await?;
    let mut watching = watcher.into_connection();
    let _throwing = throw_down(builder).await?;
    record(&mut watching, entity_types::ITEM, slab).await
}

/// The builder digs a hole of one block beside them, fills it with water and throws a
/// stone straight down into it from above it.
async fn into_water(address: &str) -> Result<Timeline> {
    let (mut builder, watcher) = two_bots(address).await?;
    let (x, y, z) = feet(&builder);
    let hole = (x + 1, GROUND, z);
    builder.dig(hole.0, hole.1, hole.2).await?;
    builder
        .wait_until(PATIENCE, |bot| {
            bot.block_at(hole.0, hole.1, hole.2)
                .is_ok_and(|state| state == Some(0))
        })
        .await?;
    place(
        &mut builder,
        items::WATER_BUCKET,
        (x + 1, GROUND - 1, z),
        face::TOP,
        hole,
    )
    .await?;
    builder
        .stand_at(f64::from(x + 1) + 0.5, f64::from(y), f64::from(z) + 0.5)
        .await?;
    builder.idle(Duration::from_millis(200)).await?;
    let mut watching = watcher.into_connection();
    let _throwing = throw_down(builder).await?;
    record(&mut watching, entity_types::ITEM, hole).await
}

/// The builder puts a stone two high beside them and a block of sand against its side
/// at the top, with air below, which falls two blocks.
async fn sand_falls(address: &str) -> Result<Timeline> {
    let (mut builder, watcher) = two_bots(address).await?;
    let (x, y, z) = feet(&builder);
    place(
        &mut builder,
        items::COBBLESTONE,
        (x + 2, GROUND, z),
        face::TOP,
        (x + 2, y, z),
    )
    .await?;
    place(
        &mut builder,
        items::COBBLESTONE,
        (x + 2, y, z),
        face::TOP,
        (x + 2, y + 1, z),
    )
    .await?;
    let mut watching = watcher.into_connection();
    builder.take_from_creative_inventory(1, items::SAND).await?;
    builder.select_slot(1).await?;
    builder.use_item_on(x + 2, y + 1, z, face::EAST).await?;
    let recorded = record(&mut watching, entity_types::FALLING_BLOCK, (x + 3, y, z)).await;
    drop(builder);
    recorded
}

/// The packets of a thrown item but a resting item's motion, which Clustine reports
/// only with the item's committed changes.
fn fixed_by_the_rules(timeline: &Timeline) -> Timeline {
    let landed = timeline
        .iter()
        .position(|(_, sent)| matches!(sent, Sent::Whole { on_ground: true }));
    let after_landing = |index: usize| landed.is_some_and(|landed| index > landed);
    let entries = timeline.iter().enumerate();
    entries
        .filter(|(index, (_, sent))| !(after_landing(*index) && *sent == Sent::Motion))
        .map(|(_, entry)| entry.clone())
        .collect()
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn an_item_thrown_onto_a_slab_is_sent_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = onto_a_slab(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = onto_a_slab(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    // On both: the spawn and its stack; the spawn's update a move of nothing; the
    // landing whole at the update after it, the 20th tick; the pick-up at 40, then the
    // removal. Every update at a count of 20 from the spawn, but a pick-up's.
    for (server, timeline) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(
            matches!(timeline[..2], [(_, Sent::Spawn { .. }), (_, Sent::Data(ref values))] if values == &[8]),
            "{server}: {timeline:?}"
        );
        let landed = timeline
            .iter()
            .find(|(_, sent)| matches!(sent, Sent::Whole { on_ground: true }));
        assert!(
            landed.is_some_and(|(tick, _)| (tick - 20).abs() <= 1),
            "{server}: {timeline:?}"
        );
        let taken = timeline
            .iter()
            .find(|(_, sent)| matches!(sent, Sent::Taken { .. }));
        assert!(
            taken.is_some_and(|(tick, _)| (tick - 40).abs() <= 2),
            "{server}: {timeline:?}"
        );
    }
    compare(&fixed_by_the_rules(&ours), &fixed_by_the_rules(&theirs)).unwrap();
    server.stop().await;
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn an_item_thrown_into_water_is_sent_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = into_water(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = into_water(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    // In water an item is updated every tick (`needsSync`): from its first move that
    // is no update of the 20, a move every tick for the second it is recorded after.
    for (server, timeline) in [("the official server", &theirs), ("Clustine", &ours)] {
        let moves: Vec<i64> = timeline
            .iter()
            .filter(|(_, sent)| matches!(sent, Sent::Move { .. } | Sent::Whole { .. }))
            .map(|(tick, _)| *tick)
            .collect();
        let entered = moves
            .iter()
            .find(|tick| tick.rem_euclid(20) > 1 && tick.rem_euclid(20) < 19)
            .copied()
            .unwrap_or_else(|| panic!("{server}: never in water: {timeline:?}"));
        let after: Vec<i64> = moves
            .iter()
            .copied()
            .filter(|tick| (entered..entered + 20).contains(tick))
            .collect();
        // Each tick within one of the next, as packets of a tick may arrive across the
        // tick's end.
        assert!(after.len() >= 18, "{server}: {after:?} of {timeline:?}");
    }
    // The layouts: the same kinds of packet, in water both send motion and moves.
    let kinds = |timeline: &Timeline| {
        let mut kinds: Vec<&'static str> = timeline
            .iter()
            .map(|(_, sent)| match sent {
                Sent::Spawn { .. } => "spawn",
                Sent::Data(_) => "data",
                Sent::Motion => "motion",
                Sent::Move { .. } => "move",
                Sent::Whole { .. } => "whole",
                Sent::Taken { .. } => "taken",
                Sent::Removed => "removed",
                Sent::Block(..) => "block",
                Sent::Transient(..) => "transient",
            })
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    };
    assert_eq!(kinds(&ours), kinds(&theirs));
    server.stop().await;
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_block_of_sand_falls_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = sand_falls(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = sand_falls(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    // The falling block appears before the air its block leaves, with its block as the
    // spawn's data and its start at index 8; it lands with its block at once, the
    // transient block, then its removal, and its block comes again with the next tick's
    // changes. It draws nothing: every packet within a tick of the other server's.
    for (server, timeline) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(
            matches!(timeline[..2], [(_, Sent::Spawn { .. }), (_, Sent::Data(ref values))] if values == &[8]),
            "{server}: {timeline:?}"
        );
        let transient = timeline
            .iter()
            .position(|(_, sent)| matches!(sent, Sent::Transient(..)));
        let removed = timeline.iter().position(|(_, sent)| *sent == Sent::Removed);
        assert!(
            matches!((transient, removed), (Some(transient), Some(removed)) if transient > 0 && removed == transient + 1),
            "{server}: {timeline:?}"
        );
    }
    compare(&ours, &theirs).unwrap();
    server.stop().await;
}
