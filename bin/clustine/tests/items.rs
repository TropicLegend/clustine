//! End-to-end tests of the item a player throws, written from the specification
//! (`docs/adr/0028-entities.md`, sections 1 to 3 and 6; `docs/groundwork/n2-plan.md`,
//! sections 1, 7, 8 and 11; `docs/groundwork/n4-plan.md`, section 1; the roadmap's
//! "What a player meets until N4") by someone who did not write the code: Q, ctrl+Q and
//! the creative screen throw an item that every client in view is shown once, that flies
//! off, lands and lies where it landed, and that is there again, exactly once and where
//! it lay, after the server is stopped and started, after the process is killed, after
//! its region is taken over and after its chunk was left alone; and a creative player
//! who walks onto it after its two seconds picks it up.
//!
//! A thrown item lands about three blocks in front of its thrower, so a thrower who
//! stands still is not over it (a player picks up what is within a block of their
//! box). The bots here never walk over an item but in the test that means them to.
//!
//! With `CLUSTINE_TEST_PINS` set, the world is divided as the other end-to-end tests
//! divide it, and the spawn, where the item is thrown, is at a pinned line.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine::Config;
use clustine_botswarm::{Bot, Connection, SeenEntity};
use clustine_data::{entity_types, items};
use clustine_protocol::codec::{Position, Reader};
use clustine_protocol::delta::VecDeltaCodec;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientTickEnd, ClientboundKeepAlive, ClientboundPlay, EntityDataValue, PlayerAction,
    ServerboundKeepAlive, SetCreativeModeSlot, SetHeldItem, SetPlayerPosition, SpawnEntity,
    TakeItemEntity, VecDelta, face, inventory, movement_flags, player_action,
};
use tokio::time::{Instant, timeout};
use uuid::Uuid;

use common::{VIEW_DISTANCE, config, free_address, spawn_server, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// The game's eye height of a standing player, a float, and how far below the eyes a
/// thrown item leaves (`ServerPlayer.drop`): the plan, section 11.
const EYES: f32 = 1.62;
const BELOW_THE_EYES: f64 = 0.300_000_011_920_928_96;

/// The top of the flat world's floor, where a thrown item comes to lie.
const FLOOR: f64 = -60.0;

/// How far apart two clients' views of an item at rest may be: one follows its moves,
/// and the edge sends a move only once the item is more than the game's tolerance
/// (the square root of 7.6e-6, under 0.003 of a block) from where it was last sent, and
/// the other was shown it whole.
const AT_REST: f64 = 1.0 / 256.0;

/// How many of the region's ticks an item's position has to stay as shown, on the
/// ground, for it to be taken as at rest: more than two of the edge's updates of an
/// item, which come every 20 ticks (the brief's section 9). An item that has stopped
/// is sent nothing more, so there is no message that says it has; the ticks are
/// counted by round trips, each of which the region answers in a tick of its own.
const TICKS_AT_REST: usize = 45;

/// How far from a player's feet, horizontally, an item is out of their reach: a player
/// picks up what touches their box (0.6 wide) grown by a block, and an item is 0.25
/// wide; half a block more than that.
const OUT_OF_REACH: f64 = 0.3 + 1.0 + 0.125 + 0.5;

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

/// Waits until `bot` shows the item `uuid` at rest, and returns it as shown: on the
/// ground, and where it was through [`TICKS_AT_REST`] ticks of the region.
async fn landed(bot: &mut Bot, uuid: Uuid) -> SeenEntity {
    let shown = |bot: &Bot| {
        items_of(bot)
            .into_iter()
            .find(|item| item.uuid == uuid && item.on_ground)
            .map(|item| item.position)
    };
    loop {
        bot.wait_until(PATIENCE, |bot| shown(bot).is_some())
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{} never showed the item {uuid} on the ground: {error:#}",
                    bot.report()
                )
            });
        let before = shown(bot);
        for _ in 0..TICKS_AT_REST {
            let last = bot.dig(6, -61, 6).await.unwrap();
            bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
                .await
                .unwrap();
        }
        if shown(bot) == before {
            return sees(bot, uuid).await;
        }
    }
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

/// An item as a [`Player`]'s connection was shown it, its moves followed as a client
/// follows them.
#[derive(Debug, Clone)]
struct ShownItem {
    uuid: Uuid,
    position: (f64, f64, f64),
    on_ground: bool,
    /// From its data, index 8, as it was last sent.
    stack: Option<ItemStack>,
}

/// A player's connection after it has joined, from which it throws and reads what it
/// is shown, answering keep-alives and following the items it is shown meanwhile.
struct Player {
    connection: Connection,
    /// Where its feet are.
    feet: (f64, f64, f64),
    /// The wire id the login gave the player's own entity.
    own_id: i32,
    /// The items it is shown, by wire id.
    items: BTreeMap<i32, ShownItem>,
    /// The items it was shown and then told were removed, in order.
    gone: Vec<ShownItem>,
    /// Every pick-up it was shown (`take_item_entity`), in order.
    taken: Vec<TakeItemEntity>,
    /// The slots of its inventory the server changed of its own accord, as the
    /// inventory numbers them (`set_player_inventory`).
    told_slots: BTreeMap<i32, Option<ItemStack>>,
    /// The number of its latest round trip.
    sequence: i32,
}

impl Player {
    /// Reads from `bot`'s connection from now on; it is shown no item yet.
    fn of(bot: Bot) -> Self {
        assert!(items_of(&bot).is_empty(), "{:?}", items_of(&bot));
        // Its round trips go on from the bot's: a sequence number acknowledged already
        // is not acknowledged again.
        let sequence = bot.acknowledged_sequence;
        Self {
            feet: bot.location,
            own_id: bot.info.login.entity_id,
            connection: bot.into_connection(),
            items: BTreeMap::new(),
            gone: Vec::new(),
            taken: Vec::new(),
            told_slots: BTreeMap::new(),
            sequence,
        }
    }

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
        Self::of(bot)
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

    /// Moves to `x`, `z` at the height of its feet in a single step, as a client says
    /// where it is, and ends its tick.
    async fn step_to(&mut self, x: f64, z: f64) {
        self.feet = (x, self.feet.1, z);
        self.connection
            .write(&SetPlayerPosition {
                x,
                y: self.feet.1,
                z,
                flags: movement_flags::ON_GROUND,
            })
            .await
            .unwrap();
        self.connection.write(&ClientTickEnd).await.unwrap();
    }

    /// Breaks a block of the floor where nothing was thrown, and returns once the
    /// server has acknowledged it: everything the server did with what was sent
    /// before has been done, and what it sent of it has come.
    async fn round_trip(&mut self) -> Vec<Vec<u8>> {
        self.sequence += 1;
        let sequence = self.sequence;
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

    /// The next frame that is not a keep-alive, which is answered, after following
    /// what it says of items.
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
            self.follow(&frame)?;
            return Ok(frame.to_vec());
        }
    }

    /// Takes in what `frame` says of items, of pick-ups and of the inventory, as a
    /// client does; fails where it shows an item twice.
    fn follow(&mut self, frame: &[u8]) -> Result<()> {
        let Ok(packet) = ClientboundPlay::decode(frame) else {
            return Ok(());
        };
        match packet {
            ClientboundPlay::SpawnEntity(spawn) if spawn.kind == entity_types::ITEM => {
                ensure!(
                    !self.items.contains_key(&spawn.entity_id)
                        && !self.items.values().any(|item| item.uuid == spawn.uuid),
                    "an item shown twice: {spawn:?}"
                );
                self.items.insert(
                    spawn.entity_id,
                    ShownItem {
                        uuid: spawn.uuid,
                        position: (spawn.x, spawn.y, spawn.z),
                        on_ground: false,
                        stack: None,
                    },
                );
            }
            ClientboundPlay::SetEntityData(data) => {
                if let Some(item) = self.items.get_mut(&data.entity_id) {
                    for (index, value) in &data.values {
                        if let (8, EntityDataValue::ItemStack(stack)) = (index, value) {
                            item.stack = stack.clone();
                        }
                    }
                }
            }
            ClientboundPlay::MoveEntityPos(moved) => {
                self.moved(moved.entity_id, &moved.delta, moved.on_ground)
            }
            ClientboundPlay::MoveEntityPosRot(moved) => {
                self.moved(moved.entity_id, &moved.delta, moved.on_ground)
            }
            ClientboundPlay::SyncEntityPosition(sync) => {
                if let Some(item) = self.items.get_mut(&sync.entity_id) {
                    if let Some(position) = sync.path.end() {
                        item.position = position;
                    }
                    item.on_ground = sync.on_ground;
                }
            }
            ClientboundPlay::RemoveEntities(removed) => {
                for id in removed.entity_ids {
                    if let Some(item) = self.items.remove(&id) {
                        self.gone.push(item);
                    }
                }
            }
            ClientboundPlay::TakeItemEntity(taken) => self.taken.push(taken),
            ClientboundPlay::SetPlayerInventory(put) => {
                self.told_slots.insert(put.slot, put.stack);
            }
            _ => {}
        }
        Ok(())
    }

    /// A relative move of the item `id`, followed as a client follows it.
    fn moved(&mut self, id: i32, delta: &VecDelta, on_ground: bool) {
        let Some(item) = self.items.get_mut(&id) else {
            return;
        };
        let delta = match delta {
            VecDelta::Linear(delta) => *delta,
            VecDelta::Stepped(steps) => steps.last().map_or([0; 3], |(_, delta)| *delta),
        };
        let (x, y, z) = item.position;
        let [x, y, z] = VecDeltaCodec::new([x, y, z]).decode(delta);
        item.position = (x, y, z);
        item.on_ground = on_ground;
    }

    /// Reads what comes until `done` holds.
    async fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) -> Result<()> {
        let deadline = Instant::now() + PATIENCE;
        while !done(self) {
            if let Err(error) = self.next_frame(deadline).await {
                bail!("never {what}: {error:#}; shown {:?}", self.items);
            }
        }
        Ok(())
    }

    /// The item `uuid` with its wire id, if it is shown.
    fn item(&self, uuid: Uuid) -> Option<(i32, &ShownItem)> {
        (self.items.iter())
            .find(|(_, item)| item.uuid == uuid)
            .map(|(id, item)| (*id, item))
    }

    /// Reads until `done` holds and every item shown is on the ground, and they all
    /// stay as shown through [`TICKS_AT_REST`] ticks of the region (see [`landed`]).
    async fn at_rest(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
        let shown = |player: &Self| {
            let all = (player.items.iter())
                .map(|(id, item)| (*id, item.position, item.on_ground, item.stack.clone()))
                .collect::<Vec<_>>();
            (done(player) && all.iter().all(|(_, _, on_ground, _)| *on_ground)).then_some(all)
        };
        loop {
            self.until(what, |player| shown(player).is_some())
                .await
                .unwrap();
            let before = shown(self);
            for _ in 0..TICKS_AT_REST {
                self.round_trip().await;
            }
            if shown(self) == before {
                return;
            }
        }
    }

    /// Reads until the item `uuid` is shown at rest, and returns it.
    async fn landed(&mut self, uuid: Uuid) -> ShownItem {
        self.at_rest("shown the item at rest", |player| {
            player.item(uuid).is_some()
        })
        .await;
        self.item(uuid).expect("it was just shown").1.clone()
    }

    /// How many of each item the items it is shown hold together, by item id.
    fn totals(&self) -> BTreeMap<i32, i32> {
        let mut totals = BTreeMap::new();
        for stack in self.items.values().filter_map(|item| item.stack.clone()) {
            *totals.entry(stack.item).or_default() += stack.count;
        }
        totals
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
                    let stack = data.values.iter().find_map(|(index, value)| match value {
                        EntityDataValue::ItemStack(stack) if *index == 8 => Some(stack.clone()),
                        _ => None,
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

/// Two views of an item at rest, within [`AT_REST`] of each other.
fn assert_lies_at(seen: (f64, f64, f64), landed: (f64, f64, f64)) {
    let off = [seen.0 - landed.0, seen.1 - landed.1, seen.2 - landed.2];
    assert!(
        off.iter().all(|off| off.abs() <= AT_REST),
        "the item is at {seen:?}, not where it landed, {landed:?}"
    );
}

fn horizontally(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.2 - b.2)
}

/// An item thrown from `thrown` landed at `landed`: on the floor, about three blocks
/// off, out of the reach of a thrower who stood still.
fn assert_flew(thrown: (f64, f64, f64), landed: (f64, f64, f64)) {
    assert!(
        (landed.1 - FLOOR).abs() <= AT_REST,
        "it lies on the floor: {landed:?}"
    );
    let far = horizontally(thrown, landed);
    assert!(
        (OUT_OF_REACH..5.0).contains(&far),
        "an item thrown from {thrown:?} landed {far} blocks off, at {landed:?}"
    );
}

/// Throws one stone with Q, and waits until both the thrower and the watcher show it at
/// rest, the same: returns what the thrower was shown of the throw, and where it lies.
async fn throw_a_stone_and_let_it_land(
    thrower: &mut Player,
    watcher: &mut Bot,
) -> (ShownToTheThrower, (f64, f64, f64)) {
    thrower.hold(Some(STONE)).await;
    thrower.drop_held(false).await;
    let own = thrower.shown_an_item().await.unwrap();
    sees(watcher, own.spawn.uuid).await;
    let lies = thrower.landed(own.spawn.uuid).await.position;
    assert_flew(thrown_from(thrower.feet), lies);
    let seen = landed(watcher, own.spawn.uuid).await;
    assert_lies_at(seen.position, lies);
    (own, lies)
}

const STONE: ItemStack = ItemStack::new(items::STONE, 1);

/// A world in `directory`, divided as `CLUSTINE_TEST_PINS` says.
fn on_disk(directory: &std::path::Path) -> Config {
    Config {
        world: Some(directory.to_owned()),
        ..config()
    }
}

/// A player throws a stone with Q while another watches: both are shown it once,
/// where the game puts it, and the watcher sees the thrower's arm swing; it flies off
/// and lands, the same for both. The server is stopped and started on the same world,
/// twice, and both players, joining again, are shown the same item, once, where it lay.
#[tokio::test]
async fn a_thrown_item_is_seen_by_both_players_and_is_there_once_after_the_server_is_started_again()
{
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(on_disk(directory.path())).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    let swings_before = watcher
        .stats
        .received
        .get(clientbound::NAMES[clientbound::SWING_ANIMATION as usize])
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
    // The thrower's arm swings for the watcher (`SwingAnimation`).
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.stats
                .received
                .get(clientbound::NAMES[clientbound::SWING_ANIMATION as usize])
                .copied()
                .unwrap_or(0)
                > swings_before
        })
        .await
        .unwrap();
    // It lands, the same for both.
    let lies = thrower.landed(own.spawn.uuid).await.position;
    assert_flew(thrown, lies);
    let seen = landed(&mut watcher, own.spawn.uuid).await;
    assert_lies_at(seen.position, lies);
    // Someone who comes later is shown it where it lies.
    let mut latecomer = join(&address, "Latecomer").await;
    let late = sees(&mut latecomer, own.spawn.uuid).await;
    assert_lies_at(late.position, lies);
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
            assert_lies_at(again.position, lies);
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
/// thrower with its stack (the item's data, index 8). The two throws of stone may
/// merge where they land (`ItemEntity.mergeWithNeighbours`), so what both players are
/// shown in the end is counted by what the items hold: five stone and three dirt.
#[tokio::test]
async fn q_ctrl_q_and_the_creative_screen_each_throw_what_the_game_throws_and_an_empty_hand_nothing()
 {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    // The watcher, too, is followed by its connection from here, for the items' stacks.
    let mut watcher = Player::of(watcher);

    // An empty hand: nothing. The round trip shows that the server has handled it.
    thrower.hold(None).await;
    thrower.drop_held(false).await;
    thrower.drop_held(true).await;
    let frames = thrower.round_trip().await;
    assert_eq!(spawns_an_item(&frames), 0, "an empty hand threw something");

    let five = ItemStack::new(items::STONE, 5);
    thrower.hold(Some(five)).await;
    thrower.drop_held(false).await;
    let one = thrower.shown_an_item().await.unwrap();
    assert_eq!(one.stack, Some(STONE), "Q throws one");
    // Four are left in the hand, and ctrl+Q throws them all.
    thrower.drop_held(true).await;
    let rest = thrower.shown_an_item().await.unwrap();
    assert_eq!(
        rest.stack,
        Some(ItemStack::new(items::STONE, 4)),
        "ctrl+Q throws the rest of the stack"
    );
    // Now the hand is empty: Q throws nothing more.
    thrower.drop_held(false).await;
    let frames = thrower.round_trip().await;
    assert_eq!(
        spawns_an_item(&frames),
        0,
        "the emptied hand threw something"
    );

    let dirt = ItemStack::new(items::DIRT, 3);
    thrower.throw_from_the_screen(dirt.clone()).await;
    let screen = thrower.shown_an_item().await.unwrap();
    assert_eq!(screen.stack, Some(dirt));

    // Three throws, with three wire ids and three UUIDs.
    let ids = [
        one.spawn.entity_id,
        rest.spawn.entity_id,
        screen.spawn.entity_id,
    ];
    assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
    let uuids = [one.spawn.uuid, rest.spawn.uuid, screen.spawn.uuid];
    assert!(uuids[0] != uuids[1] && uuids[1] != uuids[2] && uuids[0] != uuids[2]);

    // Both are shown five stone and three dirt, in items each shown once (a second
    // spawn of one fails the reading), all at rest; any that went went into another
    // (a merge), not away.
    let expected = BTreeMap::from([(items::STONE, 5), (items::DIRT, 3)]);
    for player in [&mut thrower, &mut watcher] {
        player
            .at_rest("shown five stone and three dirt, at rest", |player| {
                player.totals() == expected
            })
            .await;
        assert!(
            player.items.len() == 2 || player.items.len() == 3,
            "{:?}",
            player.items
        );
        assert_eq!(
            player.gone.len(),
            3 - player.items.len(),
            "{:?}",
            player.gone
        );
        assert!(
            player.items.values().all(|item| uuids.contains(&item.uuid)),
            "{:?}",
            player.items
        );
    }
    server.stop().await;
}

/// The item's region is taken over by a new runner, as when its worker is believed
/// dead: the watcher, who never left, keeps being shown the item, without its being
/// taken away, and someone who joins after is shown it once, where it lies.
#[tokio::test]
async fn a_thrown_item_stays_shown_once_through_its_region_being_taken_over() {
    let (mut server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    let (own, lies) = throw_a_stone_and_let_it_land(&mut thrower, &mut watcher).await;

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
    assert_lies_at(shown.position, lies);
    // The watcher still shows it, once, and was never told it went: the restored
    // region's load raises its `hops`, and the edge continues it on the wire
    // (ADR-0028, section 6).
    let still = sees(&mut watcher, own.spawn.uuid).await;
    assert_lies_at(still.position, lies);
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

/// The server's process is killed once the watcher is shown the item at rest, which a
/// worker publishes only once its tick is durable: the item is there again, once,
/// where it lay, when the server is started on the same world.
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
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    let (own, lies) = throw_a_stone_and_let_it_land(&mut thrower, &mut watcher).await;
    server.kill().await.unwrap();
    drop(thrower);
    drop(watcher);

    let mut server = spawn_server(&address, &world, &more).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = join(&address, "Thrower").await;
    for bot in [&mut watcher, &mut thrower] {
        let again = sees(bot, own.spawn.uuid).await;
        assert_lies_at(again.position, lies);
    }
    server.kill().await.unwrap();
}

/// A player throws an item and walks so far away that its chunk is let go; when they
/// come back it is loaded again from the store and shown to them once, where it lay.
#[tokio::test]
async fn an_item_left_alone_is_there_again_when_its_thrower_comes_back() {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    let (own, lies) = throw_a_stone_and_let_it_land(&mut thrower, &mut watcher).await;
    drop(thrower);

    // The watcher walks away until the spawn's chunk is out of its view, and back, along
    // a line that the item lies out of the reach of.
    let home = watcher.location;
    assert!(
        (lies.2 - home.2).abs() >= OUT_OF_REACH,
        "{lies:?}, {home:?}"
    );
    watcher.walk_to(300.5, home.2, 2.0).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.center == Some((18, 0)) && !bot.chunks.contains_key(&(0, 0))
        })
        .await
        .unwrap();
    assert!(items_of(&watcher).is_empty(), "{:?}", items_of(&watcher));
    watcher.walk_to(home.0, home.2, 2.0).await.unwrap();
    let back = sees(&mut watcher, own.spawn.uuid).await;
    assert_lies_at(back.position, lies);
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

/// In a world whose regions follow their players: an item lies near the spawn beside a
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
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    let (own, lies) = throw_a_stone_and_let_it_land(&mut thrower, &mut watcher).await;
    drop(thrower);

    let mut wanderer = join(&address, "Wanderer").await;
    let home = wanderer.location;
    // The item lies in front of the spawn; the wanderer walks off along the spawn's
    // line and comes back to behind it, out of the item's reach all the way.
    let back_at = (home.0, home.1, home.2 - 3.0);
    assert!(horizontally(back_at, lies) >= OUT_OF_REACH, "{lies:?}");
    assert!(
        (lies.2 - home.2).abs() >= OUT_OF_REACH,
        "{lies:?}, {home:?}"
    );
    sees(&mut wanderer, own.spawn.uuid).await;
    walk_tending(&mut wanderer, 400.5, home.2, &mut watcher).await;
    use common::following::region_of;
    let apart = |list: &clustine_rpc::RegionList| {
        let (home, away) = (region_of(list, (0, 0)), region_of(list, (25, 0)));
        home.is_some() && away.is_some() && home != away
    };
    let list = until_listed(&server, "split", apart, &mut [&mut watcher, &mut wanderer]).await;
    println!("split: {list:?}");
    walk_tending(&mut wanderer, back_at.0, home.2, &mut watcher).await;
    walk_tending(&mut wanderer, back_at.0, back_at.2, &mut watcher).await;
    let back = sees(&mut wanderer, own.spawn.uuid).await;
    assert_lies_at(back.position, lies);
    // The watcher answered the server all the while (its keep-alives are answered as
    // it waits), and shows the item as it did.
    let last = watcher.dig(6, -61, 6).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    let still = sees(&mut watcher, own.spawn.uuid).await;
    assert_lies_at(still.position, lies);
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
/// item into the western region's chunk, which a player east of it is shown too. It
/// lands west of the line, the same for both. The server is stopped and started, and
/// both are shown it once where it lay.
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
    let mut thrower = Player::of(thrower_bot);
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
    let lies = thrower.landed(own.spawn.uuid).await.position;
    assert_flew(thrown, lies);
    assert!(lies.0 < 0.0, "it lies west of chunk x = 0: {lies:?}");
    let seen = landed(&mut watcher, own.spawn.uuid).await;
    assert_lies_at(seen.position, lies);
    drop(thrower);
    server.stop().await;
    drop(watcher);

    let (server, address) = start_with(on_disk(directory.path())).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = join(&address, "Thrower").await;
    for bot in [&mut watcher, &mut thrower] {
        let again = sees(bot, own.spawn.uuid).await;
        assert_lies_at(again.position, lies);
    }
    server.stop().await;
}

/// The plan of N4, section 1, and the brief's section 5: a creative player who walks
/// onto an item after its pick-up delay of 40 ticks picks it up. The thrower throws its
/// one stone, which lands; once it lies (it has been still for [`TICKS_AT_REST`]
/// ticks, more than its delay) the thrower steps onto it. The watcher is shown the pick-up
/// (`TakeItemEntity`, the thrower collecting one) and then the item's removal; so is
/// the thrower, whose emptied hotbar slot 0 has the stone again.
#[tokio::test]
async fn a_creative_player_who_walks_onto_a_landed_item_picks_it_up_and_everyone_sees_it_go() {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    let (own, lies) = throw_a_stone_and_let_it_land(&mut thrower, &mut watcher).await;
    // Standing still, the thrower has not picked it up.
    assert_ne!(
        thrower.told_slots.get(&0),
        Some(&Some(STONE)),
        "{:?}",
        thrower.told_slots
    );
    assert!(thrower.taken.is_empty() && watcher.taken.is_empty());

    let shown_as = watcher
        .seen_player("Thrower")
        .expect("the thrower is in view")
        .entity_id;
    let item_as = sees(&mut watcher, own.spawn.uuid).await.entity_id;
    thrower.step_to(lies.0, lies.2).await;

    // The thrower is shown it taken by itself, its slot filled and the item gone.
    thrower
        .until("shown its item picked up", |player| {
            player.item(own.spawn.uuid).is_none()
                && player.told_slots.get(&0) == Some(&Some(STONE))
                && !player.taken.is_empty()
        })
        .await
        .unwrap();
    assert_eq!(
        thrower.taken,
        [TakeItemEntity {
            item_id: own.spawn.entity_id,
            collector_id: thrower.own_id,
            count: 1,
        }]
    );
    assert!(
        thrower.gone.iter().any(|gone| gone.uuid == own.spawn.uuid),
        "{:?}",
        thrower.gone
    );

    // The watcher is shown the thrower take it, and then that it went.
    watcher
        .wait_until(PATIENCE, |bot| {
            !bot.taken.is_empty() && bot.vanished.iter().any(|gone| gone.uuid == own.spawn.uuid)
        })
        .await
        .unwrap();
    assert_eq!(
        watcher.taken,
        [TakeItemEntity {
            item_id: item_as,
            collector_id: shown_as,
            count: 1,
        }]
    );
    assert!(items_of(&watcher).is_empty(), "{:?}", items_of(&watcher));
    // A round trip later, nothing has come back.
    let last = watcher.dig(6, -61, 6).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    assert!(items_of(&watcher).is_empty(), "{:?}", items_of(&watcher));
    assert!(thrower.items.is_empty(), "{:?}", thrower.items);
    server.stop().await;
}

/// How many of `frames` spawn an item.
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
/// maximum stack, from the data (`ItemComponents::of`); stone stacks to 64, so 65 of it
/// are passed over and 64 thrown.
#[tokio::test]
async fn a_creative_throw_of_more_than_the_item_s_maximum_stack_is_passed_over() {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    thrower
        .throw_from_the_screen(ItemStack::new(items::STONE, 65))
        .await;
    let frames = thrower.round_trip().await;
    assert_eq!(spawns_an_item(&frames), 0, "65 stone were thrown");
    let full = ItemStack::new(items::STONE, 64);
    thrower.throw_from_the_screen(full.clone()).await;
    let shown = thrower.shown_an_item().await.unwrap();
    assert_eq!(shown.stack, Some(full));
    server.stop().await;
}

/// The plan, section 11: throws out of the creative screen go through the game's
/// `dropSpamThrottler`, which lets 74 through at once (20 a throw, under 1480) and
/// then about one a second; a hundred at once throw no more than that. Each throw is
/// a spawn, whether or not its item merges into another once it has landed.
#[tokio::test]
async fn a_burst_of_creative_throws_is_held_to_the_game_s_throttle() {
    let (server, address) = start_with(config()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut thrower = Player::join(&address, "Thrower", &mut watcher).await;
    let started = Instant::now();
    for _ in 0..100 {
        thrower.throw_from_the_screen(STONE).await;
    }
    let frames = thrower.round_trip().await;
    let thrown = spawns_an_item(&frames);
    // One more for every tick of the game the burst took.
    let ticks = started.elapsed().as_millis() / 50;
    assert!(
        (74..=74 + ticks as usize + 1).contains(&thrown),
        "{thrown} of a hundred thrown in {ticks} ticks"
    );
    server.stop().await;
}
