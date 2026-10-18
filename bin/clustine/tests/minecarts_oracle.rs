//! Comparisons with the official server of what a client is sent of a minecart as it
//! moves (step N5e, `docs/groundwork/n5-plan.md`): a cart let go on a slope of rails,
//! which rolls down it, round a curve and along a straight run until it stops; a cart
//! at rest on a powered rail beside a block of stone, which the rail kicks away from the
//! stone and speeds up, coasting on along plain rails; and a cart a player gets into
//! and drives by holding the forward key on a straight run of rails.
//!
//! A builder lays each case from plain clicks at the same place in both worlds, which
//! are flat alike (grass on top at y -61), so that the cart's coordinates are the same
//! numbers on both servers and not only the same distances. A watcher hovering above it
//! records, as they arrive, the packets about the cart, each at its tick counted from
//! the cart's spawn, or for the cart that is driven from the moment its rider is shown
//! seated (when the player gets in depends on how quickly the bots act). The cart's
//! old behaviour (`OldMinecartBehavior`) draws no random numbers, so the two servers'
//! timelines are to hold the same packets in the same order, each within a tick of the
//! other's, as the sand of `entities_oracle.rs` is compared.
//!
//! Each packet is read whole by the codec. Compared exactly: the spawn's type, data,
//! position, velocity and angles; every value of the cart's entity data; a motion's
//! vector as the wire carries it; a relative move's delta in 4096ths of a block, its
//! angles and its on-ground flag; a whole sync's position, angles and flag; the number
//! of riders. The doubles of a cart's position are not on the wire but at a sync and the
//! spawn: what is compared is what a client is sent. Within a tick: when each arrives,
//! counted by its arrival at the watcher.
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine, with
//! what each server sent printed, with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test minecarts_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_botswarm::{Bot, Connection, Oracle};
use clustine_data::{BLOCKS, BlockState, entity_types, items};
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientboundKeepAlive, ClientboundPlay, EntityDataValue, PositionPath, ServerboundKeepAlive,
    VecDelta, face, player_input,
};
use tokio::time::{Instant, timeout_at};

use common::start;

/// How long a bot waits for what it should be sent soon.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long the watcher records after the cart's spawn, or after its rider is seated:
/// a cart let go comes to rest well within it.
const RECORDED: Duration = Duration::from_secs(10);

/// How long it records a driven cart after its rider is seated: the rider's push
/// brings it up to a tenth of a block a tick in about a hundred and twenty ticks, after
/// some six blocks, and it goes on at that speed short of the end of its rails.
const DRIVEN: Duration = Duration::from_secs(8);

/// The grass on top of both servers' flat worlds, and the height a player stands at
/// on it, where rails are laid.
const GROUND: i32 = -61;
const FEET: i32 = GROUND + 1;

/// How high above the rails the builder hovers while it builds and places the cart:
/// above everything a case builds, which is a block high at most, and above the cart,
/// which so never meets the builder.
const HOVER: i32 = 3;

/// The case's origin: clear of the column at the origin, where a joining player stands
/// on Clustine, and of the ten blocks around the spawn the official server spreads its
/// players over; every case lies within the chunks at x 1 and 2, a single region where
/// `CLUSTINE_TEST_PINS=0,4` divides the world.
const X: i32 = 20;
const Z: i32 = 4;

/// Where the watcher hovers: above the middle of the cases, eight blocks up.
const WATCHING: (f64, f64, f64) = (X as f64 + 4.5, (FEET + 8) as f64, Z as f64 + 4.5);

/// The hotbar slot the builder holds what it uses in.
const HELD: i16 = 0;

/// The yaw of a player looking east, and of one looking north, in degrees.
const EAST: f32 = 270.0;
const NORTH: f32 = 180.0;

/// A block's position.
type Block = (i32, i32, i32);

/// What the watcher was sent about the cart, as far as it is compared.
#[derive(Debug, Clone, PartialEq)]
enum Sent {
    /// `add_entity`.
    Spawn {
        kind: i32,
        data: i32,
        at: [f64; 3],
        velocity: [f64; 3],
        pitch: u8,
        yaw: u8,
        head_yaw: u8,
    },
    /// `set_entity_data`, every value with its index.
    Data(Vec<(u8, EntityDataValue)>),
    /// `set_entity_motion`.
    Motion([f64; 3]),
    /// `move_entity_pos`.
    Move {
        delta: VecDelta,
        on_ground: bool,
    },
    /// `move_entity_pos_rot`.
    MoveTurn {
        delta: VecDelta,
        yaw: i8,
        pitch: i8,
        on_ground: bool,
    },
    /// `move_entity_rot`.
    Turn {
        yaw: i8,
        pitch: i8,
        on_ground: bool,
    },
    /// `entity_position_sync`.
    Whole {
        path: PositionPath,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    /// `rotate_head`.
    Head(u8),
    /// `set_passengers`, with the number of riders (whose ids differ between servers).
    Riders(usize),
    Removed,
}

/// Each packet with the tick it came in, counted from the timeline's start.
type Timeline = Vec<(i64, Sent)>;

/// What a timeline's ticks are counted from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Start {
    /// The cart's spawn.
    Spawn,
    /// The first rider shown seated in it; what came before is left out.
    Seated,
}

/// Reads what `watching` is sent, answering keep-alives, and makes the timeline of the
/// first minecart to appear, from `start` for `window`. It gives up if `start` has not
/// come within [`PATIENCE`].
async fn record(watching: &mut Connection, start: Start, window: Duration) -> Result<Timeline> {
    let mut deadline = Instant::now() + PATIENCE;
    let mut cart = None;
    let mut started_at = None;
    let mut seen: Vec<(Instant, Sent)> = Vec::new();
    loop {
        let Ok(frame) = timeout_at(deadline, watching.read_frame()).await else {
            break;
        };
        let frame = frame?;
        let now = Instant::now();
        let sent = match ClientboundPlay::decode(&frame) {
            Ok(ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id })) => {
                watching.write(&ServerboundKeepAlive { id }).await?;
                continue;
            }
            Ok(ClientboundPlay::SpawnEntity(spawn))
                if spawn.kind == entity_types::MINECART && cart.is_none() =>
            {
                cart = Some(spawn.entity_id);
                Sent::Spawn {
                    kind: spawn.kind,
                    data: spawn.data,
                    at: [spawn.x, spawn.y, spawn.z],
                    velocity: spawn.velocity,
                    pitch: spawn.pitch,
                    yaw: spawn.yaw,
                    head_yaw: spawn.head_yaw,
                }
            }
            Ok(ClientboundPlay::SetEntityData(data)) if Some(data.entity_id) == cart => {
                Sent::Data(data.values)
            }
            Ok(ClientboundPlay::SetEntityMotion(motion)) if Some(motion.entity_id) == cart => {
                Sent::Motion(motion.motion)
            }
            Ok(ClientboundPlay::MoveEntityPos(moved)) if Some(moved.entity_id) == cart => {
                Sent::Move {
                    delta: moved.delta,
                    on_ground: moved.on_ground,
                }
            }
            Ok(ClientboundPlay::MoveEntityPosRot(moved)) if Some(moved.entity_id) == cart => {
                Sent::MoveTurn {
                    delta: moved.delta,
                    yaw: moved.yaw,
                    pitch: moved.pitch,
                    on_ground: moved.on_ground,
                }
            }
            Ok(ClientboundPlay::MoveEntityRot(turned)) if Some(turned.entity_id) == cart => {
                Sent::Turn {
                    yaw: turned.yaw,
                    pitch: turned.pitch,
                    on_ground: turned.on_ground,
                }
            }
            Ok(ClientboundPlay::SyncEntityPosition(moved)) if Some(moved.entity_id) == cart => {
                Sent::Whole {
                    path: moved.path,
                    yaw: moved.yaw,
                    pitch: moved.pitch,
                    on_ground: moved.on_ground,
                }
            }
            Ok(ClientboundPlay::SetHeadRotation(head)) if Some(head.entity_id) == cart => {
                Sent::Head(head.head_yaw)
            }
            Ok(ClientboundPlay::SetPassengers(riders)) if Some(riders.vehicle_id) == cart => {
                Sent::Riders(riders.passengers.len())
            }
            Ok(ClientboundPlay::RemoveEntities(removed))
                if cart.is_some_and(|cart| removed.entity_ids.contains(&cart)) =>
            {
                Sent::Removed
            }
            Ok(_) => continue,
            Err(error) => {
                // A packet of an entity's tracking that the codec cannot read whole is a
                // layout the two do not agree on.
                let id = frame.first().copied().map(i32::from);
                let tracked = [
                    clientbound::ADD_ENTITY,
                    clientbound::MOVE_ENTITY_POS,
                    clientbound::MOVE_ENTITY_POS_ROT,
                    clientbound::MOVE_ENTITY_ROT,
                    clientbound::SET_ENTITY_MOTION,
                    clientbound::ENTITY_POSITION_SYNC,
                    clientbound::SET_PASSENGERS,
                ];
                ensure!(
                    !id.is_some_and(|id| tracked.contains(&id)),
                    "a packet {id:?} the codec cannot read: {error}"
                );
                continue;
            }
        };
        let starts = match start {
            Start::Spawn => matches!(sent, Sent::Spawn { .. }),
            Start::Seated => matches!(sent, Sent::Riders(riders) if riders > 0),
        };
        if starts && started_at.is_none() {
            started_at = Some(now);
            deadline = now + window;
        }
        if started_at.is_some() {
            seen.push((now, sent));
        }
    }
    let started_at =
        started_at.with_context(|| format!("the cart's timeline never began ({start:?})"))?;
    let tick = |when: Instant| {
        let since = when.saturating_duration_since(started_at).as_secs_f64();
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

/// The id of the block state `text` names.
fn state(text: &str) -> i32 {
    let state = BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a state"));
    i32::from(state.0)
}

/// The block state `id` as the game prints one, as far as it is a state.
fn described(id: i32) -> String {
    let Ok(state) = u16::try_from(id).map(BlockState) else {
        return format!("state {id}");
    };
    let Some(block) = state.block_id() else {
        return format!("state {id}");
    };
    let properties: Vec<String> = state
        .properties()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    format!("{}[{}]", BLOCKS[block].name, properties.join(","))
}

/// A builder and a watcher in `address`'s world: the watcher hovering at [`WATCHING`]
/// with the chunks of the cases, the builder above the case's origin.
async fn two_bots(address: &str) -> Result<(Bot, Bot)> {
    let mut builder = Bot::join(address, "Builder").await?;
    builder.wait_until(PATIENCE, Bot::is_loaded).await?;
    // The builder flies, as a client hovering in creative mode does. The official
    // server moves a player who does not in the player's own tick, gravity and all,
    // before the player pushes what its box meets (`LivingEntity.aiStep`), and puts it
    // back after: so a builder hovering above a cart pushed it every tick there (its
    // run of 2026-10-18), where Clustine, which moves no player itself, did not.
    builder.set_flying(true).await?;
    let mut watcher = Bot::join(address, "Watcher").await?;
    watcher.wait_until(PATIENCE, Bot::is_loaded).await?;
    // Up from where each joined, then across above everything: the official server does
    // not let a player move into a block.
    let (x, _, z) = watcher.location;
    watcher.fly_to((x, WATCHING.1, z), (0.0, 90.0), 1.0).await?;
    watcher.fly_to(WATCHING, (0.0, 90.0), 1.0).await?;
    let case_chunks: Vec<(i32, i32)> = (X - 1..=X + 26)
        .step_by(4)
        .flat_map(|x| {
            [
                (x.div_euclid(16), Z.div_euclid(16)),
                (x.div_euclid(16), (Z + 9).div_euclid(16)),
            ]
        })
        .collect();
    watcher
        .wait_until(PATIENCE, |bot| {
            case_chunks
                .iter()
                .all(|chunk| bot.chunks.contains_key(chunk))
        })
        .await
        .context("the watcher was not sent the chunks of the case")?;
    let (x, _, z) = builder.location;
    builder
        .fly_to((x, f64::from(FEET + HOVER), z), (0.0, 90.0), 1.0)
        .await?;
    hover_over(&mut builder, (X, FEET, Z), 0.0).await?;
    Ok((builder, watcher))
}

/// Flies the builder to hover [`HOVER`] blocks above the block `at`, looking down with
/// the yaw `yaw`, which decides the way a rail it lays runs.
async fn hover_over(builder: &mut Bot, at: Block, yaw: f32) -> Result<()> {
    let over = (
        f64::from(at.0) + 0.5,
        f64::from(FEET + HOVER),
        f64::from(at.2) + 0.5,
    );
    builder.fly_to(over, (yaw, 90.0), 1.0).await
}

/// Puts `item` into `at`, clicking the top of the block below it from above it while
/// looking along `yaw`, and waits until the builder is shown something there.
async fn put(builder: &mut Bot, item: i32, at: Block, yaw: f32) -> Result<()> {
    hover_over(builder, at, yaw).await?;
    builder.take_from_creative_inventory(HELD, item).await?;
    builder.select_slot(HELD).await?;
    let sequence = builder.use_item_on(at.0, at.1 - 1, at.2, face::TOP).await?;
    builder
        .wait_until(PATIENCE, |bot| {
            bot.acknowledged_sequence >= sequence
                && bot
                    .block_at(at.0, at.1, at.2)
                    .is_ok_and(|state| state.is_some_and(|state| state != 0))
        })
        .await
        .with_context(|| format!("item {item} was not placed at {at:?}"))
}

/// Breaks the block `at` from above it and waits until the builder is shown air there.
async fn dig(builder: &mut Bot, at: Block) -> Result<()> {
    hover_over(builder, at, 0.0).await?;
    builder.dig(at.0, at.1, at.2).await?;
    builder
        .wait_until(PATIENCE, |bot| {
            bot.block_at(at.0, at.1, at.2)
                .is_ok_and(|state| state == Some(0))
        })
        .await
        .with_context(|| format!("the block at {at:?} was not broken"))
}

/// Waits until the builder is shown each block of `wanted` in its state, so that a case
/// whose rails did not join as meant says so rather than differing later.
async fn laid_as_meant(builder: &mut Bot, wanted: &[(Block, &str)]) -> Result<()> {
    let ids: Vec<(Block, i32)> = wanted.iter().map(|&(at, text)| (at, state(text))).collect();
    let shown = |bot: &Bot| {
        ids.iter()
            .all(|&((x, y, z), id)| bot.block_at(x, y, z).ok().flatten() == Some(id))
    };
    if builder.wait_until(PATIENCE, shown).await.is_err() {
        let seen: Vec<_> = wanted
            .iter()
            .map(|&((x, y, z), text)| {
                let was = builder.block_at(x, y, z).ok().flatten();
                let was = was.map(described);
                format!("{text} at {:?}: {was:?}", (x, y, z))
            })
            .collect();
        bail!("the case was not laid as meant: {seen:?}");
    }
    Ok(())
}

/// Places a minecart on the rail at `rail` from above it, as the builder holds one.
async fn place_cart(builder: &mut Bot, rail: Block) -> Result<()> {
    hover_over(builder, rail, 0.0).await?;
    builder
        .take_from_creative_inventory(HELD, items::MINECART)
        .await?;
    builder.select_slot(HELD).await?;
    builder
        .use_item_on(rail.0, rail.1, rail.2, face::TOP)
        .await?;
    Ok(())
}

/// Records the watcher's timeline from `start` for `window`, while `act` has the
/// builder do what starts it, and keeps the builder connected until the record ends.
async fn watched<F>(
    mut builder: Bot,
    watcher: Bot,
    start: Start,
    window: Duration,
    act: F,
) -> Result<Timeline>
where
    F: AsyncFnOnce(&mut Bot) -> Result<()>,
{
    let mut watching = watcher.into_connection();
    let recording = tokio::spawn(async move { record(&mut watching, start, window).await });
    act(&mut builder).await?;
    // The builder answers what it is sent until the record is done, so that neither
    // server takes it for gone.
    while !recording.is_finished() {
        builder.idle(Duration::from_millis(100)).await?;
    }
    recording.await?
}

/// A slope of rails up to a stone east of the origin, a curve at its foot west of the
/// origin and a straight run south from the curve; a cart placed on the slope.
async fn down_a_slope(address: &str) -> Result<Timeline> {
    let (mut builder, watcher) = two_bots(address).await?;
    let slope = (X, FEET, Z);
    let top = (X + 1, FEET + 1, Z);
    let curve = (X - 1, FEET, Z);
    put(&mut builder, items::STONE, (X + 1, FEET, Z), EAST).await?;
    // The rail on the stone is what the slope rises to join: a rail laid beside a rail a
    // step up slopes up to it.
    put(&mut builder, items::RAIL, top, EAST).await?;
    put(&mut builder, items::RAIL, slope, EAST).await?;
    let run: Vec<Block> = (1..=8).map(|dz| (X - 1, FEET, Z + dz)).collect();
    for &rail in &run {
        put(&mut builder, items::RAIL, rail, NORTH).await?;
    }
    put(&mut builder, items::RAIL, curve, NORTH).await?;
    let mut wanted = vec![
        (slope, "minecraft:rail[shape=ascending_east]"),
        (curve, "minecraft:rail[shape=south_east]"),
    ];
    wanted.extend(
        run.iter()
            .map(|&rail| (rail, "minecraft:rail[shape=north_south]")),
    );
    laid_as_meant(&mut builder, &wanted).await?;
    watched(builder, watcher, Start::Spawn, RECORDED, async |builder| {
        place_cart(builder, slope).await
    })
    .await
}

/// A line of powered rails running east from a stone, the first on a block of redstone
/// that powers the line, then plain rails; a cart placed at rest on the first. It
/// comes to the top speed on the powered rails and coasts some eighteen blocks; the
/// plain rails run on for twenty-three, so that it never leaves them.
async fn kicked_by_a_powered_rail(address: &str) -> Result<Timeline> {
    let (mut builder, watcher) = two_bots(address).await?;
    let first = (X, FEET, Z);
    dig(&mut builder, (X, GROUND, Z)).await?;
    put(&mut builder, items::REDSTONE_BLOCK, (X, GROUND, Z), EAST).await?;
    put(&mut builder, items::STONE, (X - 1, FEET, Z), EAST).await?;
    let powered: Vec<Block> = (0..4).map(|dx| (X + dx, FEET, Z)).collect();
    for &rail in &powered {
        put(&mut builder, items::POWERED_RAIL, rail, EAST).await?;
    }
    let plain: Vec<Block> = (4..27).map(|dx| (X + dx, FEET, Z)).collect();
    for &rail in &plain {
        put(&mut builder, items::RAIL, rail, EAST).await?;
    }
    let mut wanted: Vec<(Block, &str)> = powered
        .iter()
        .map(|&rail| (rail, "minecraft:powered_rail[powered=true,shape=east_west]"))
        .collect();
    wanted.extend(
        plain
            .iter()
            .map(|&rail| (rail, "minecraft:rail[shape=east_west]")),
    );
    laid_as_meant(&mut builder, &wanted).await?;
    watched(builder, watcher, Start::Spawn, RECORDED, async |builder| {
        place_cart(builder, first).await
    })
    .await
}

/// A straight run of rails east; a cart placed on the second, which the builder gets
/// into facing east and drives by holding the forward key.
async fn driven_forward(address: &str) -> Result<Timeline> {
    let (mut builder, watcher) = two_bots(address).await?;
    let run: Vec<Block> = (0..16).map(|dx| (X + dx, FEET, Z)).collect();
    for &rail in &run {
        put(&mut builder, items::RAIL, rail, EAST).await?;
    }
    let wanted: Vec<(Block, &str)> = run
        .iter()
        .map(|&rail| (rail, "minecraft:rail[shape=east_west]"))
        .collect();
    laid_as_meant(&mut builder, &wanted).await?;
    let start = run[1];
    watched(builder, watcher, Start::Seated, DRIVEN, async |builder| {
        place_cart(builder, start).await?;
        builder
            .wait_until(PATIENCE, |bot| {
                bot.entities
                    .values()
                    .any(|seen| seen.kind == entity_types::MINECART)
            })
            .await
            .context("the builder was not shown its cart")?;
        let cart = builder
            .entities
            .values()
            .find(|seen| seen.kind == entity_types::MINECART)
            .map(|seen| seen.entity_id)
            .context("the cart is gone")?;
        // Facing east from where it is, so that the forward key pushes the cart east.
        hover_over(builder, start, EAST).await?;
        let location = builder.location;
        builder.fly_to(location, (EAST, 0.0), 1.0).await?;
        let riders = |bot: &Bot| {
            bot.stats
                .received
                .get("minecraft:set_passengers")
                .copied()
                .unwrap_or(0)
        };
        let before = riders(builder);
        builder.interact(cart, [0.0, 0.35, 0.0]).await?;
        builder
            .wait_until(PATIENCE, |bot| riders(bot) > before)
            .await
            .context("the builder was not seated in the cart")?;
        builder.hold_keys(player_input::FORWARD).await
    })
    .await
}

/// The first packet of a timeline that moves the cart, at its tick.
fn first_move(timeline: &Timeline) -> Option<i64> {
    timeline
        .iter()
        .find(|(_, sent)| {
            matches!(
                sent,
                Sent::Move { .. } | Sent::MoveTurn { .. } | Sent::Whole { .. }
            )
        })
        .map(|(tick, _)| *tick)
}

/// Checks on each server's own timeline that the case did what it is for: the cart
/// appeared where `MinecartItem` puts it on the rail at `rail`, raised by `lift` on a
/// slope, and moved.
fn check_spawned_and_moved(timeline: &Timeline, rail: Block, lift: f64, server: &str) {
    let at = [
        f64::from(rail.0) + 0.5,
        f64::from(rail.1) + 0.0625 + lift,
        f64::from(rail.2) + 0.5,
    ];
    assert!(
        matches!(timeline.first(), Some((0, Sent::Spawn { at: spawned, .. })) if *spawned == at),
        "{server}: {timeline:?}"
    );
    assert!(first_move(timeline).is_some(), "{server}: {timeline:?}");
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_cart_let_go_on_a_slope_rolls_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = down_a_slope(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = down_a_slope(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    for (name, timeline) in [("the official server", &theirs), ("Clustine", &ours)] {
        check_spawned_and_moved(timeline, (X, FEET, Z), 0.5, name);
    }
    compare(&ours, &theirs).unwrap();
    server.stop().await;
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_cart_kicked_by_a_powered_rail_runs_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = kicked_by_a_powered_rail(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = kicked_by_a_powered_rail(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    for (name, timeline) in [("the official server", &theirs), ("Clustine", &ours)] {
        check_spawned_and_moved(timeline, (X, FEET, Z), 0.0, name);
    }
    compare(&ours, &theirs).unwrap();
    server.stop().await;
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_cart_driven_forward_by_its_rider_runs_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = driven_forward(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = driven_forward(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    // Seated first, then on its way.
    for (name, timeline) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(
            matches!(timeline.first(), Some((0, Sent::Riders(1)))),
            "{name}: {timeline:?}"
        );
        assert!(first_move(timeline).is_some(), "{name}: {timeline:?}");
    }
    compare(&ours, &theirs).unwrap();
    server.stop().await;
}
