//! A comparison with the official server of what a client is sent when another player
//! throws an item (step N2c, `docs/groundwork/n2-plan.md`, sections 8 and 11): the
//! layout of the `add_entity` and `set_entity_data` packets, the entity type, the index
//! and the serializer of the item's data, and where the item leaves the thrower from.
//! The motion is drawn at random on both servers, so only that there is one is
//! compared, not its value.
//!
//! It needs Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so it is ignored by default. Run it, with what each server sent
//! printed, with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test items_oracle -- --ignored --nocapture
//! ```

mod common;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clustine_botswarm::{Bot, Connection, Oracle};
use clustine_data::{entity_types, items};
use clustine_protocol::codec::{Position, Reader};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::{clientbound, serverbound};
use clustine_protocol::packets::play::{
    ClientboundKeepAlive, PlayerAction, ServerboundKeepAlive, SpawnEntity, player_action,
};
use clustine_protocol::packets::{Packet, play::ClientboundPlay};
use tokio::time::{Instant, timeout};

use common::start;

/// How long the watcher waits for the item to appear.
const PATIENCE: Duration = Duration::from_secs(30);

/// What a watcher is sent of an item another player threw, as far as it is compared.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    kind: i32,
    pitch: u8,
    head_yaw: u8,
    data: i32,
    /// Whether the motion was written as more than the one byte of none.
    moving: bool,
    /// How far below the thrower's feet plus their standing eyes' 1.62 (a float) the
    /// item leaves, which the game has as 0.3 (a float too), and how far off their x
    /// and z: none.
    below_the_eyes: f64,
    off_the_feet: (f64, f64),
    /// The index and the serializer of each value of the item's data, in order.
    values: Vec<(u8, i32)>,
    /// The stack of the value of the serializer of stacks.
    stack: Option<ItemStack>,
    /// Whether the data came as the next packet about an entity after the spawn.
    data_next: bool,
}

/// The index and the serializer of each value of an entity's data, and the stack of
/// the last value of the serializer of stacks.
type Values = (Vec<(u8, i32)>, Option<ItemStack>);

/// The values of a `set_entity_data` body after its id: index, serializer and, for the
/// serializer of stacks (7), the stack. A serializer that is not one of a stack ends the
/// reading, as what it writes cannot be skipped without knowing it.
fn values_of(reader: &mut Reader<'_>) -> Result<Values> {
    let mut values = Vec::new();
    let mut stack = None;
    loop {
        let index = reader.u8()?;
        if index == 0xFF {
            return Ok((values, stack));
        }
        let serializer = reader.var_int()?;
        values.push((index, serializer));
        if serializer != 7 {
            bail!("a value of serializer {serializer} at index {index}: {values:?}");
        }
        stack = reader.item_stack()?;
    }
}

/// Reads what `watching` is sent until an item appears and its data follows, answering
/// keep-alives meanwhile, and says what it was shown. `feet` is where the thrower
/// stands.
async fn watch(watching: &mut Connection, feet: (f64, f64, f64)) -> Result<Shown> {
    let deadline = Instant::now() + PATIENCE;
    let mut spawned: Option<SpawnEntity> = None;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = timeout(left, watching.read_frame())
            .await
            .context("no item appeared")??;
        let mut reader = Reader::new(&frame);
        let id = reader.var_int()?;
        match id {
            clientbound::KEEP_ALIVE => {
                if let ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id }) =
                    ClientboundPlay::decode(&frame)?
                {
                    watching.write(&ServerboundKeepAlive { id }).await?;
                }
            }
            clientbound::ADD_ENTITY => {
                let ClientboundPlay::SpawnEntity(spawn) = ClientboundPlay::decode(&frame)? else {
                    bail!("an add_entity that is none");
                };
                if spawn.kind == entity_types::ITEM && spawned.is_none() {
                    println!(
                        "spawned: {spawn:?}, its motion's first byte {}",
                        frame_velocity_byte(&frame)?
                    );
                    spawned = Some(spawn);
                }
            }
            clientbound::SET_ENTITY_DATA => {
                let entity = reader.var_int()?;
                let Some(spawn) = &spawned else {
                    continue;
                };
                if entity != spawn.entity_id {
                    seen.push(id);
                    continue;
                }
                let (values, stack) = values_of(&mut reader)?;
                let names: Vec<&str> = seen
                    .iter()
                    .filter_map(|id| usize::try_from(*id).ok())
                    .filter_map(|id| clientbound::NAMES.get(id).copied())
                    .collect();
                println!("its data: {values:?} with {stack:?}; sent between: {names:?}");
                let eyes = feet.1 + f64::from(1.62_f32);
                return Ok(Shown {
                    kind: spawn.kind,
                    pitch: spawn.pitch,
                    head_yaw: spawn.head_yaw,
                    data: spawn.data,
                    moving: spawn.velocity != [0.0; 3],
                    below_the_eyes: eyes - spawn.y,
                    off_the_feet: (spawn.x - feet.0, spawn.z - feet.2),
                    values,
                    stack,
                    // A bundle's delimiter around the pair is nothing between them.
                    data_next: seen.iter().all(|id| *id == clientbound::BUNDLE_DELIMITER),
                });
            }
            _ if spawned.is_some() => seen.push(id),
            _ => {}
        }
    }
}

/// The first byte of the motion of an `add_entity`, which is 0 for none.
fn frame_velocity_byte(frame: &[u8]) -> Result<u8> {
    let mut reader = Reader::new(frame);
    reader.var_int()?;
    reader.var_int()?;
    reader.uuid()?;
    reader.var_int()?;
    for _ in 0..3 {
        reader.f64()?;
    }
    Ok(reader.u8()?)
}

/// A player takes a stone from the creative inventory into their hand and throws it with
/// Q while another watches; what the watcher is sent.
async fn thrown_at(address: &str) -> Result<Shown> {
    let mut thrower = Bot::join(address, "Thrower").await?;
    let mut watcher = Bot::join(address, "Watcher").await?;
    watcher
        .wait_until(PATIENCE, |bot| bot.seen_player("Thrower").is_some())
        .await?;
    // With the chunks around the thrower's, which Clustine has loaded then and ticks:
    // it throws nothing where the chunk does not tick.
    thrower
        .wait_until(PATIENCE, |bot| {
            let (x, z) = (
                (bot.location.0.floor() as i32).div_euclid(16),
                (bot.location.2.floor() as i32).div_euclid(16),
            );
            let around = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (x + dx, z + dz)));
            bot.seen_player("Watcher").is_some()
                && around
                    .into_iter()
                    .all(|chunk| bot.chunks.contains_key(&chunk))
        })
        .await?;
    thrower
        .take_from_creative_inventory(0, items::STONE)
        .await?;
    thrower.select_slot(0).await?;
    let feet = thrower.location;
    let mut watching = watcher.into_connection();
    let mut throwing = thrower.into_connection();
    // A connection's packets are handled in order: the stone is in hand before the Q.
    throwing
        .write(&PlayerAction {
            status: player_action::DROP_ITEM,
            position: Position { x: 0, y: 0, z: 0 },
            face: 0,
            sequence: 0,
        })
        .await?;
    let shown = watch(&mut watching, feet).await;
    // The thrower's own keep-alives are not answered from here on; the test is over
    // well before the server would miss them.
    drop(throwing);
    shown
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_thrown_item_is_sent_like_on_the_official_server() {
    assert_eq!(PlayerAction::ID, serverbound::PLAYER_ACTION);
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;

    let theirs = thrown_at(oracle.address())
        .await
        .unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = thrown_at(&address)
        .await
        .unwrap_or_else(|error| panic!("Clustine: {error:#}"));
    println!("the official server showed {theirs:?}\nClustine showed {ours:?}");

    // What the game's rules give, on both: an item, at the thrower's x and z and 0.3
    // below their eyes, moving, with no pitch, head or data; then its stack at
    // `ItemEntity.DATA_ITEM`, index 8, by the serializer of stacks, 7.
    for (server, shown) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert_eq!(shown.kind, entity_types::ITEM, "{server}");
        assert!(shown.moving, "{server}");
        assert!(
            (shown.below_the_eyes - 0.300_000_011_920_928_96).abs() < 1e-9,
            "{server}: {shown:?}"
        );
        assert_eq!(shown.off_the_feet, (0.0, 0.0), "{server}");
        assert_eq!(shown.values, [(8, 7)], "{server}");
        assert_eq!(
            shown.stack.clone(),
            Some(ItemStack::new(items::STONE, 1)),
            "{server}"
        );
    }
    let layout = |shown: &Shown| {
        (
            shown.kind,
            shown.pitch,
            shown.head_yaw,
            shown.data,
            shown.values.clone(),
            shown.stack.clone(),
            shown.data_next,
        )
    };
    assert_eq!(layout(&ours), layout(&theirs));

    server.stop().await;
}
