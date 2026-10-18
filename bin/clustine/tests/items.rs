//! End-to-end tests of the item a player throws, written from the specification
//! (`docs/adr/0028-entities.md`, sections 1 to 3 and 6; `docs/groundwork/n2-plan.md`,
//! sections 1, 7, 8 and 11; the roadmap's "What a player meets until N4") by someone
//! who did not write the code: Q, ctrl+Q and the creative screen throw an item that
//! every client in view is shown once, that lies where it was thrown, and that is there
//! again, exactly once, after the server is stopped and started, after the process is
//! killed, after its region is taken over and after its chunk was left alone.
//!
//! With `CLUSTINE_TEST_PINS` set, the world is divided as the other end-to-end tests
//! divide it, and the spawn, where the item is thrown, is at a pinned line.

mod common;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clustine::Config;
use clustine_botswarm::{Bot, Connection, SeenEntity};
use clustine_data::{entity_types, items};
use clustine_protocol::codec::{Position, Reader};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientboundKeepAlive, ClientboundPlay, EntityDataValue, PlayerAction, ServerboundKeepAlive,
    SetCreativeModeSlot, SetHeldItem, SpawnEntity, face, inventory, player_action,
};
use tokio::time::{Instant, timeout};
use uuid::Uuid;

use common::{VIEW_DISTANCE, config, free_address, spawn_server, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// The game's eye height of a standing player, a float, and how far below the eyes a
/// thrown item leaves (`ServerPlayer.drop`): the plan, section 11.
const EYES: f32 = 1.62;
const BELOW_THE_EYES: f64 = 0.300_000_011_920_928_96;

/// Joins and waits for the chunks around the spawn point.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// The items `bot` shows.
fn items_of(bot: &Bot) -> Vec<&SeenEntity> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == entity_types::ITEM)
        .collect()
}

/// Waits until `bot` shows the item `uuid`, and returns it as shown, checking that it
/// shows no other item and not this one twice.
async fn sees(bot: &mut Bot, uuid: Uuid) -> SeenEntity {
    bot.wait_until(PATIENCE, |bot| {
        items_of(bot).iter().any(|item| item.uuid == uuid)
    })
    .await
    .unwrap_or_else(|error| panic!("{} never showed the item {uuid}: {error:#}", bot.report()));
    let shown = items_of(bot);
    assert_eq!(shown.len(), 1, "one item, once: {shown:?}");
    shown[0].clone()
}

/// The chunks around the bot's, which the server ticks once they are loaded: a throw
/// from a chunk that does not tick throws nothing (the plan, section 11), so the
/// tests throw once the bot has them all.
async fn around_loaded(bot: &mut Bot) {
    bot.wait_until(PATIENCE, |bot| {
        let x = (bot.location.0.floor() as i32).div_euclid(16);
        let z = (bot.location.2.floor() as i32).div_euclid(16);
        (-1..=1).all(|dx| (-1..=1).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
    })
    .await
    .unwrap();
}

/// What a thrower's own connection was shown of the item it threw.
#[derive(Debug, Clone)]
struct ShownToTheThrower {
    spawn: SpawnEntity,
    stack: Option<ItemStack>,
}

/// A player's connection after it has joined, from which it throws and reads what it
/// is shown, answering keep-alives meanwhile.
struct Thrower {
    connection: Connection,
    /// Where its feet are.
    feet: (f64, f64, f64),
    /// The wire id the login gave the player's own entity.
    own_id: i32,
}

impl Thrower {
    async fn join(address: &str, name: &str, watcher: &mut Bot) -> Self {
        let mut bot = join(address, name).await;
        around_loaded(&mut bot).await;
        // The other one is in the world and in view, so that the throw is for both.
        watcher
            .wait_until(PATIENCE, |bot| bot.seen_player(name).is_some())
            .await
            .unwrap();
        bot.wait_until(PATIENCE, |bot| {
            bot.seen_player(&watcher.info.profile.name).is_some()
        })
        .await
        .unwrap();
        Self {
            feet: bot.location,
            own_id: bot.info.login.entity_id,
            connection: bot.into_connection(),
        }
    }

    /// Puts `stack` into hotbar slot 0 from the creative inventory and holds it.
    async fn hold(&mut self, stack: Option<ItemStack>) {
        self.connection
            .write(&SetCreativeModeSlot {
                slot: inventory::HOTBAR_START as i16,
                stack,
            })
            .await
            .unwrap();
        self.connection
            .write(&SetHeldItem { slot: 0 })
            .await
            .unwrap();
    }

    /// Q, or ctrl+Q if `all`. A connection's packets are handled in order.
    async fn drop_held(&mut self, all: bool) {
        let status = if all {
            player_action::DROP_ALL_ITEMS
        } else {
            player_action::DROP_ITEM
        };
        self.connection
            .write(&PlayerAction {
                status,
                position: Position { x: 0, y: 0, z: 0 },
                face: 0,
                sequence: 0,
            })
            .await
            .unwrap();
    }

    /// Throws `stack` out of the creative inventory screen: a creative slot of -1.
    async fn throw_from_the_screen(&mut self, stack: ItemStack) {
        self.connection
            .write(&SetCreativeModeSlot {
                slot: -1,
                stack: Some(stack),
            })
            .await
            .unwrap();
    }

    /// Breaks a block of the floor where nothing was thrown, and returns once the
    /// server has acknowledged it: everything the server did with what was sent
    /// before has been done, and what it sent of it has come.
    async fn round_trip(&mut self, sequence: i32) -> Vec<Vec<u8>> {
        self.connection
            .write(&PlayerAction {
                status: player_action::START_DESTROY_BLOCK,
                position: Position { x: 5, y: -61, z: 5 },
                face: face::TOP,
                sequence,
            })
            .await
            .unwrap();
        let mut frames = Vec::new();
        let deadline = Instant::now() + PATIENCE;
        loop {
            let frame = self.next_frame(deadline).await.unwrap();
            let acknowledged = matches!(
                ClientboundPlay::decode(&frame),
                Ok(ClientboundPlay::AcknowledgeBlockChange(answer)) if answer.sequence >= sequence
            );
            frames.push(frame);
            if acknowledged {
                return frames;
            }
        }
    }

    /// The next frame that is not a keep-alive, which is answered.
    async fn next_frame(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = timeout(left, self.connection.read_frame())
                .await
                .context("nothing came in time")??;
            let id = Reader::new(&frame).var_int()?;
            if id == clientbound::KEEP_ALIVE {
                if let ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id }) =
                    ClientboundPlay::decode(&frame)?
                {
                    self.connection.write(&ServerboundKeepAlive { id }).await?;
                }
                continue;
            }
            return Ok(frame.to_vec());
        }
    }

    /// Reads until an item is spawned and its data follows.
    async fn shown_an_item(&mut self) -> Result<ShownToTheThrower> {
        match self.thrown_or_put_back().await? {
            Some(shown) => Ok(shown),
            None => bail!("the throw was refused and the slot put back"),
        }
    }

    /// Reads until an item is spawned and its data follows, or until the server puts
    /// hotbar slot 0 back as it was, which it does where it refuses a throw (the plan,
    /// section 11): none then.
    async fn thrown_or_put_back(&mut self) -> Result<Option<ShownToTheThrower>> {
        let deadline = Instant::now() + PATIENCE;
        let mut spawned: Option<SpawnEntity> = None;
        loop {
            let frame = self.next_frame(deadline).await?;
            match ClientboundPlay::decode(&frame) {
                Ok(ClientboundPlay::SetPlayerInventory(put))
                    if spawned.is_none() && put.slot == 0 =>
                {
                    return Ok(None);
                }
                Ok(ClientboundPlay::SpawnEntity(spawn)) if spawn.kind == entity_types::ITEM => {
                    if spawned.is_some() {
                        bail!("a second item before the first one's data: {spawn:?}");
                    }
                    spawned = Some(spawn);
                }
                Ok(ClientboundPlay::SetEntityData(data)) => {
                    let Some(spawn) = &spawned else { continue };
                    if data.entity_id != spawn.entity_id {
                        continue;
                    }
                    let stack = data.values.iter().find_map(|(index, value)| {
                        let EntityDataValue::ItemStack(stack) = value;
                        (*index == 8).then_some(*stack)
                    });
                    return Ok(Some(ShownToTheThrower {
                        spawn: spawn.clone(),
                        stack: stack.context("the item's data has no stack at index 8")?,
                    }));
                }
                _ => {}
            }
        }
    }
}

/// Where an item thrown by a player standing at `feet` is, by the game's rule: at
/// their x and z, 0.3 below their eyes.
fn thrown_from(feet: (f64, f64, f64)) -> (f64, f64, f64) {
    (feet.0, feet.1 + f64::from(EYES) - BELOW_THE_EYES, feet.2)
}

fn assert_near(seen: (f64, f64, f64), expected: (f64, f64, f64)) {
    let off = [
        seen.0 - expected.0,
        seen.1 - expected.1,
        seen.2 - expected.2,
    ];
    assert!(
        off.iter().all(|off| off.abs() < 1e-6),
        "the item is at {seen:?}, not at {expected:?}"
    );
}

const STONE: ItemStack = ItemStack {
    item: items::STONE,
    count: 1,
};

/// A world in `directory`, divided as `CLUSTINE_TEST_PINS` says.
fn on_disk(directory: &std::path::Path) -> Config {
    Config {
        world: Some(directory.to_owned()),
        ..config()
    }
}

/// A player throws a stone with Q while another watches: both are shown it once,
/// where the game puts it, and the watcher sees the thrower's arm swing. The server is
/// stopped and started on the same world, twice, and both players, joining again,
/// are shown the same item, once, where it was.
#[tokio::test]
async fn a_thrown_item_is_seen_by_both_players_and_is_there_once_after_the_server_is_started_again()
{
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(on_disk(directory.path())).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;
    let swings_before = watcher
        .stats
        .received
        .get(clientbound::NAMES[clientbound::ANIMATE as usize])
        .copied()
        .unwrap_or(0);

    thrower.hold(Some(STONE)).await;
    thrower.drop_held(false).await;
    let own = thrower.shown_an_item().await.unwrap();
    assert_ne!(own.spawn.entity_id, thrower.own_id);
    assert_eq!(own.stack, Some(STONE));
    assert!(
        own.spawn.velocity != [0.0; 3],
        "it moves off: {:?}",
        own.spawn
    );
    let thrown = thrown_from(thrower.feet);
    assert_near((own.spawn.x, own.spawn.y, own.spawn.z), thrown);

    let seen = sees(&mut watcher, own.spawn.uuid).await;
    assert_near(seen.position, thrown);
    // The thrower's arm swings for the watcher (`Animate`).
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.stats
                .received
                .get(clientbound::NAMES[clientbound::ANIMATE as usize])
                .copied()
                .unwrap_or(0)
                > swings_before
        })
        .await
        .unwrap();
    // Someone who comes later is shown it where it was thrown, as it was, not where it
    // would have fallen: until N4 the server keeps it where it was thrown.
    let mut latecomer = join(&address, "Latecomer").await;
    let late = sees(&mut latecomer, own.spawn.uuid).await;
    assert_near(late.position, thrown);
    drop(latecomer);
    drop(thrower);
    server.stop().await;
    drop(watcher);

    for round in 0..2 {
        let (server, address) = start_with(on_disk(directory.path())).await;
        let mut watcher = join(&address, "Watcher").await;
        let mut thrower = join(&address, "Thrower").await;
        for bot in [&mut watcher, &mut thrower] {
            let again = sees(bot, own.spawn.uuid).await;
            assert_near(again.position, thrown);
        }
        // A round trip later, it is still shown once and nothing else was thrown.
        let last = thrower.dig(6, -61, 6).await.unwrap();
        thrower
            .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
            .await
            .unwrap();
        assert_eq!(items_of(&thrower).len(), 1, "round {round}");
        assert_eq!(items_of(&watcher).len(), 1, "round {round}");
        server.stop().await;
    }
}

/// Q throws one of the held stack, ctrl+Q the whole stack, and the creative screen's
/// slot -1 the stack named; an empty hand throws nothing. Each is shown to the
/// thrower with its stack (the item's data, index 8).
#[tokio::test]
async fn q_ctrl_q_and_the_creative_screen_each_throw_what_the_game_throws_and_an_empty_hand_nothing()
 {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;

    // An empty hand: nothing. The round trip shows that the server has handled it.
    thrower.hold(None).await;
    thrower.drop_held(false).await;
    thrower.drop_held(true).await;
    let frames = thrower.round_trip(1).await;
    assert!(
        !frames.iter().any(|frame| matches!(
            ClientboundPlay::decode(frame),
            Ok(ClientboundPlay::SpawnEntity(spawn)) if spawn.kind == entity_types::ITEM
        )),
        "an empty hand threw something"
    );

    let five = ItemStack {
        item: items::STONE,
        count: 5,
    };
    thrower.hold(Some(five)).await;
    thrower.drop_held(false).await;
    let one = thrower.shown_an_item().await.unwrap();
    assert_eq!(one.stack, Some(STONE), "Q throws one");
    // Four are left in the hand, and ctrl+Q throws them all.
    thrower.drop_held(true).await;
    let rest = thrower.shown_an_item().await.unwrap();
    assert_eq!(
        rest.stack,
        Some(ItemStack {
            item: items::STONE,
            count: 4
        }),
        "ctrl+Q throws the rest of the stack"
    );
    // Now the hand is empty: Q throws nothing more.
    thrower.drop_held(false).await;
    let frames = thrower.round_trip(2).await;
    assert!(
        !frames.iter().any(|frame| matches!(
            ClientboundPlay::decode(frame),
            Ok(ClientboundPlay::SpawnEntity(spawn)) if spawn.kind == entity_types::ITEM
        )),
        "the emptied hand threw something"
    );

    let dirt = ItemStack {
        item: items::DIRT,
        count: 3,
    };
    thrower.throw_from_the_screen(dirt).await;
    let screen = thrower.shown_an_item().await.unwrap();
    assert_eq!(screen.stack, Some(dirt));

    // The watcher sees the three, each once, with three wire ids and three UUIDs.
    let uuids = [one.spawn.uuid, rest.spawn.uuid, screen.spawn.uuid];
    watcher
        .wait_until(PATIENCE, |bot| {
            uuids
                .iter()
                .all(|uuid| items_of(bot).iter().any(|item| item.uuid == *uuid))
        })
        .await
        .unwrap();
    assert_eq!(items_of(&watcher).len(), 3);
    let ids = [
        one.spawn.entity_id,
        rest.spawn.entity_id,
        screen.spawn.entity_id,
    ];
    assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
    server.stop().await;
}

/// The item's region is taken over by a new runner, as when its worker is believed
/// dead: the watcher, who never left, keeps being shown the item, without its being
/// taken away, and someone who joins after is shown it once, where it was thrown.
#[tokio::test]
async fn a_thrown_item_stays_shown_once_through_its_region_being_taken_over() {
    let (mut server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;
    thrower.hold(Some(STONE)).await;
    thrower.drop_held(false).await;
    let own = thrower.shown_an_item().await.unwrap();
    let thrown = thrown_from(thrower.feet);
    sees(&mut watcher, own.spawn.uuid).await;

    let home = server.regions().unwrap().home;
    server.take_over(home).await.unwrap();
    // The new runner serves the watcher: a block broken is acknowledged.
    let last = watcher.dig(6, -61, 6).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    let mut newcomer = join(&address, "Newcomer").await;
    let shown = sees(&mut newcomer, own.spawn.uuid).await;
    assert_near(shown.position, thrown);
    // The watcher still shows it, once, and was never told it went: the restored
    // region's load raises its `hops`, and the edge continues it on the wire
    // (ADR-0028, section 6).
    let still = sees(&mut watcher, own.spawn.uuid).await;
    assert_near(still.position, thrown);
    assert!(
        !watcher
            .vanished
            .iter()
            .any(|gone| gone.uuid == own.spawn.uuid),
        "the watcher was told the item went: {:?}",
        watcher.vanished
    );
    drop(thrower);
    server.stop().await;
}

/// The server's process is killed once the watcher is shown the item, which a worker
/// publishes only once its tick is durable: the item is there again, once, when the
/// server is started on the same world.
#[tokio::test]
async fn a_thrown_item_outlasts_the_server_process_being_killed() {
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("world");
    let address = free_address().await;
    let pins = std::env::var("CLUSTINE_TEST_PINS").unwrap_or_default();
    let more: Vec<&str> = if pins.is_empty() {
        Vec::new()
    } else {
        vec!["--pin", &pins, "--reshape", "by-hand"]
    };

    let mut server = spawn_server(&address, &world, &more).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;
    thrower.hold(Some(STONE)).await;
    thrower.drop_held(false).await;
    let own = thrower.shown_an_item().await.unwrap();
    let thrown = thrown_from(thrower.feet);
    sees(&mut watcher, own.spawn.uuid).await;
    server.kill().await.unwrap();
    drop(thrower);
    drop(watcher);

    let mut server = spawn_server(&address, &world, &more).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = join(&address, "Thrower").await;
    for bot in [&mut watcher, &mut thrower] {
        let again = sees(bot, own.spawn.uuid).await;
        assert_near(again.position, thrown);
    }
    server.kill().await.unwrap();
}

/// A player throws an item and walks so far away that its chunk is let go; when they
/// come back it is loaded again from the store and shown to them once, where it was.
#[tokio::test]
async fn an_item_left_alone_is_there_again_when_its_thrower_comes_back() {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;
    thrower.hold(Some(STONE)).await;
    thrower.drop_held(false).await;
    let own = thrower.shown_an_item().await.unwrap();
    let thrown = thrown_from(thrower.feet);
    sees(&mut watcher, own.spawn.uuid).await;
    drop(thrower);

    // The watcher walks away until the spawn's chunk is out of its view, and back.
    watcher.walk_to(300.5, 0.5, 2.0).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.center == Some((18, 0)) && !bot.chunks.contains_key(&(0, 0))
        })
        .await
        .unwrap();
    assert!(items_of(&watcher).is_empty(), "{:?}", items_of(&watcher));
    watcher.walk_to(0.5, 0.5, 2.0).await.unwrap();
    let back = sees(&mut watcher, own.spawn.uuid).await;
    assert_near(back.position, thrown);
    server.stop().await;
}

/// How long a split or a merge of a world whose regions follow their players may
/// take: the coordinator rests ten seconds after each thing it does.
const RESHAPE_PATIENCE: Duration = Duration::from_secs(120);

/// Waits until `done` holds of the server's list of regions, while the bots read
/// what they are sent and answer the server; returns the list.
async fn until_listed(
    server: &clustine::Server,
    what: &str,
    done: impl Fn(&clustine_rpc::RegionList) -> bool,
    bots: &mut [&mut Bot],
) -> clustine_rpc::RegionList {
    let deadline = Instant::now() + RESHAPE_PATIENCE;
    loop {
        let list = server.regions().unwrap();
        if done(&list) {
            return list;
        }
        assert!(Instant::now() < deadline, "never {what}: {list:?}");
        // The coordinator does what it does by itself; the list is asked again.
        for bot in bots.iter_mut() {
            bot.idle(Duration::from_millis(50)).await.unwrap();
        }
    }
}

/// Has `walker` walk to `x`, `z` while `tended` reads what it is sent.
async fn walk_tending(walker: &mut Bot, x: f64, z: f64, tended: &mut Bot) {
    let walk = walker.walk_to(x, z, 2.0);
    tokio::pin!(walk);
    loop {
        tokio::select! {
            walked = &mut walk => return walked.unwrap(),
            idled = tended.idle(Duration::from_millis(50)) => idled.unwrap(),
        }
    }
}

/// In a world whose regions follow their players: an item lies at the spawn beside a
/// watcher, and a wanderer walks so far that it is split off into a region of its own,
/// and comes back. The watcher shows the item once all the while and is never told it
/// went; the wanderer is shown it once when it is back.
#[tokio::test]
async fn a_thrown_item_lies_once_through_a_split_of_a_world_that_follows_its_players() {
    if !std::env::var("CLUSTINE_TEST_PINS")
        .unwrap_or_default()
        .is_empty()
    {
        // A pinned world is not split or merged by its players' walking.
        return;
    }
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;
    thrower.hold(Some(STONE)).await;
    thrower.drop_held(false).await;
    let own = thrower.shown_an_item().await.unwrap();
    let thrown = thrown_from(thrower.feet);
    sees(&mut watcher, own.spawn.uuid).await;
    drop(thrower);

    let mut wanderer = join(&address, "Wanderer").await;
    sees(&mut wanderer, own.spawn.uuid).await;
    walk_tending(&mut wanderer, 400.5, 0.5, &mut watcher).await;
    use common::following::region_of;
    let apart = |list: &clustine_rpc::RegionList| {
        let (home, away) = (region_of(list, (0, 0)), region_of(list, (25, 0)));
        home.is_some() && away.is_some() && home != away
    };
    let list = until_listed(&server, "split", apart, &mut [&mut watcher, &mut wanderer]).await;
    println!("split: {list:?}");
    walk_tending(&mut wanderer, 0.5, 3.5, &mut watcher).await;
    let back = sees(&mut wanderer, own.spawn.uuid).await;
    assert_near(back.position, thrown);
    // The watcher answered the server all the while (its keep-alives are answered as
    // it waits), and shows the item as it did.
    let last = watcher.dig(6, -61, 6).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    let still = sees(&mut watcher, own.spawn.uuid).await;
    assert_near(still.position, thrown);
    assert!(
        !watcher
            .vanished
            .iter()
            .any(|gone| gone.uuid == own.spawn.uuid),
        "the watcher was told the item went: {:?}",
        watcher.vanished
    );
    server.stop().await;
}

/// Beside a pinned line, where the world is divided: a player west of it throws an
/// item into the western region's chunk, which a player east of it is shown too. The
/// server is stopped and started, and both are shown it once where it was.
#[tokio::test]
async fn an_item_thrown_beside_a_pinned_line_is_seen_from_both_sides_and_outlasts_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(on_disk(directory.path())).await;
    let mut watcher = join(&address, "Watcher").await;
    watcher.walk_to(3.5, 0.5, 1.0).await.unwrap();
    let mut thrower_bot = join(&address, "Thrower").await;
    thrower_bot.walk_to(-2.5, 0.5, 1.0).await.unwrap();
    thrower_bot
        .wait_until(PATIENCE, |bot| (bot.location.0 - -2.5).abs() < 1e-9)
        .await
        .unwrap();
    // The walk is the bot's own; the server has its last position once a round trip
    // after it is answered.
    let last = thrower_bot.dig(-6, -61, 6).await.unwrap();
    thrower_bot
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    around_loaded(&mut thrower_bot).await;
    watcher
        .wait_until(PATIENCE, |bot| bot.seen_player("Thrower").is_some())
        .await
        .unwrap();
    let mut thrower = Thrower {
        feet: thrower_bot.location,
        own_id: thrower_bot.info.login.entity_id,
        connection: thrower_bot.into_connection(),
    };
    // Right after a player has crossed a line, their region may not yet tick the
    // chunk they stand in (it is being handed over): a throw then is refused and the
    // slot put back (the plan, section 11), and the player throws again.
    let mut refused = 0;
    let own = loop {
        thrower.hold(Some(STONE)).await;
        thrower.drop_held(false).await;
        match thrower.thrown_or_put_back().await.unwrap() {
            Some(own) => break own,
            None => refused += 1,
        }
        assert!(refused < 100, "every throw was refused");
    };
    println!("refused {refused} times before the item was thrown");
    let thrown = thrown_from(thrower.feet);
    assert!(thrown.0 < 0.0, "west of chunk x = 0: {thrown:?}");
    let seen = sees(&mut watcher, own.spawn.uuid).await;
    assert_near(seen.position, thrown);
    drop(thrower);
    server.stop().await;
    drop(watcher);

    let (server, address) = start_with(on_disk(directory.path())).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = join(&address, "Thrower").await;
    for bot in [&mut watcher, &mut thrower] {
        let again = sees(bot, own.spawn.uuid).await;
        assert_near(again.position, thrown);
    }
    server.stop().await;
}

/// Whether any of `frames` spawns an item.
fn spawns_an_item(frames: &[Vec<u8>]) -> usize {
    frames
        .iter()
        .filter(|frame| {
            matches!(
                ClientboundPlay::decode(frame),
                Ok(ClientboundPlay::SpawnEntity(spawn)) if spawn.kind == entity_types::ITEM
            )
        })
        .count()
}

/// The plan, section 11, as the game's `handleSetCreativeModeSlot` has it: a throw out
/// of the creative screen is taken if the item exists and the count is within its
/// maximum stack, from the data; stone stacks to 64, so 65 of it are passed over and
/// 64 thrown.
///
/// A finding: the edge holds a thrown stack to 99 of any item
/// (`ABSOLUTE_MAX_STACK_SIZE`, `services/edge/src/play.rs`), as the data has no
/// maximum per item yet, so 65 stone are thrown.
#[tokio::test]
#[ignore = "finding: a creative throw is held to 99 of any item, not to the item's own maximum"]
async fn a_creative_throw_of_more_than_the_item_s_maximum_stack_is_passed_over() {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;
    thrower
        .throw_from_the_screen(ItemStack {
            item: items::STONE,
            count: 65,
        })
        .await;
    let frames = thrower.round_trip(1).await;
    assert_eq!(spawns_an_item(&frames), 0, "65 stone were thrown");
    let full = ItemStack {
        item: items::STONE,
        count: 64,
    };
    thrower.throw_from_the_screen(full).await;
    let shown = thrower.shown_an_item().await.unwrap();
    assert_eq!(shown.stack, Some(full));
    server.stop().await;
}

/// The plan, section 11: throws out of the creative screen go through the game's
/// `dropSpamThrottler`, which lets 74 through at once (20 a throw, under 1480) and
/// then about one a second; a hundred at once throw no more than that.
#[tokio::test]
async fn a_burst_of_creative_throws_is_held_to_the_game_s_throttle() {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Thrower::join(&address, "Thrower", &mut watcher).await;
    let started = Instant::now();
    for _ in 0..100 {
        thrower.throw_from_the_screen(STONE).await;
    }
    let frames = thrower.round_trip(1).await;
    let thrown = spawns_an_item(&frames);
    // One more for every tick of the game the burst took.
    let ticks = started.elapsed().as_millis() / 50;
    assert!(
        (74..=74 + ticks as usize + 1).contains(&thrown),
        "{thrown} of a hundred thrown in {ticks} ticks"
    );
    server.stop().await;
}
