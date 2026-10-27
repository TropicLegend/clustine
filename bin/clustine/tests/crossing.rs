//! End-to-end tests of step N3, entities crossing a line, written from the specification
//! (`docs/adr/0028-entities.md`, sections 5 to 7; `docs/groundwork/n3-plan.md`,
//! sections 1 and 3) by someone who did not write the code: an item thrown, washed by
//! flowing water or pushed by a piston across a pinned line goes on across it and is
//! shown once, by one wire id and one spawn, to players on both sides, and is there once
//! after the server is stopped and started; a Q pressed right after the player crossed a
//! pinned line throws; the sending worker, the holding worker or the world store killed
//! around a crossing leaves the item there exactly once; and in a world whose regions
//! follow their players, an item thrown across the line between two regions makes them
//! merge at once, as a circuit at the line does.
//!
//! The tests at a pinned line divide their worlds themselves at chunk x = 0 and 4 (the
//! world of `CLUSTINE_TEST_PINS=0,4`), so that the line at block x = 0 is beside the
//! spawn whatever the environment says. Those that start clusters of processes run only
//! in the run without `CLUSTINE_TEST_PINS`, as the tests of chaos do.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine::Config;
use clustine_botswarm::{Bot, Connection, SeenEntity};
use clustine_data::{BlockState, entity_types, items};
use clustine_protocol::codec::{Position, Reader};
use clustine_protocol::delta::VecDeltaCodec;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientTickEnd, ClientboundKeepAlive, ClientboundPlay, EntityDataValue, PlayerAction,
    ServerboundKeepAlive, SetCreativeModeSlot, SetHeldItem, SetPlayerPosition, SetPlayerRotation,
    SpawnEntity, VecDelta, face, inventory, movement_flags, player_action,
};
use tokio::time::{Instant, timeout};
use uuid::Uuid;

use common::processes::{Cluster, Turn, turn};
use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// How long a cluster may take to be whole again after a kill: a takeover takes the
/// lease, three seconds in these tests, and a moment.
const RECOVERY: Duration = Duration::from_secs(90);

/// The top of the flat world's floor, where an item comes to lie.
const FLOOR: f64 = -60.0;

/// The pinned line beside the spawn: block x = 0, between chunk x = -1 and chunk x = 0.
const LINE: f64 = 0.0;

/// How far apart two clients' views of an item at rest may be (as in `items.rs`).
const AT_REST: f64 = 1.0 / 256.0;

/// How many of the region's ticks an item has to stay where it is shown, on the
/// ground, to be taken as at rest: more than two of the edge's updates of an item,
/// which come every 20 ticks. Counted by round trips, each answered in a tick.
const TICKS_AT_REST: usize = 45;

/// Yaws in degrees: 0 looks south (+z), 90 west, 180 north, -90 east (+x).
const EAST: f32 = -90.0;
const WEST: f32 = 90.0;
const NORTH: f32 = 180.0;

/// The block under the feet that a round trip breaks: bedrock, which nothing breaks,
/// so that the world is not changed by it.
const BEDROCK: i32 = -64;

/// Items that do not merge with each other when they lie side by side, so that each
/// throw stays an item of its own.
const KINDS: [i32; 8] = [
    items::STONE,
    items::DIRT,
    items::COBBLESTONE,
    items::OAK_PLANKS,
    items::SAND,
    items::GRAVEL,
    items::GLASS,
    items::BRICKS,
];

/// A world in `directory` divided at chunk x = 0 and 4, whose regions stay as they are.
fn pinned(directory: &std::path::Path) -> Config {
    Config {
        world: Some(directory.to_owned()),
        pins: vec![0, 4],
        follow: None,
        ..config()
    }
}

/// Whether this run of the tests is the one with `CLUSTINE_TEST_PINS`, in which the tests
/// that divide their worlds themselves and start clusters of processes do not run.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_PINS").is_some()
}

/// Joins and waits for the chunks around the spawn point, with the view distance `view`.
async fn join_with_view(address: &str, name: &str, view: i32) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), view).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

async fn join(address: &str, name: &str) -> Bot {
    join_with_view(address, name, VIEW_DISTANCE).await
}

/// The bedrock under `feet`.
fn under(feet: (f64, f64, f64)) -> (i32, i32) {
    (feet.0.floor() as i32, feet.2.floor() as i32)
}

/// A round trip: the server has handled everything the bot sent before, and the bot has
/// read what came before the answer.
async fn round_trip(bot: &mut Bot) {
    let (x, z) = under(bot.location);
    let last = bot.dig(x, BEDROCK, z).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap_or_else(|error| panic!("{error:#}: {}", bot.report()));
}

/// Walks the bot to `x`, `z` and waits until the server has its last position.
async fn stand(bot: &mut Bot, x: f64, z: f64) {
    bot.walk_to(x, z, 1.0).await.unwrap();
    round_trip(bot).await;
}

/// The items `bot` shows with the UUID `uuid`: one at most where the bot is content, as it
/// fails on a second entity of one UUID.
fn shown(bot: &Bot, uuid: Uuid) -> Vec<&SeenEntity> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == entity_types::ITEM && entity.uuid == uuid)
        .collect()
}

fn items_of(bot: &Bot) -> Vec<&SeenEntity> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == entity_types::ITEM)
        .collect()
}

/// Where each item lies as shown, with the wire id it is shown by.
type Lying = BTreeMap<Uuid, (i32, (f64, f64, f64))>;

/// Waits until `bot` shows every item of `uuids` on the ground and where it was through
/// [`TICKS_AT_REST`] ticks, and returns them as shown.
async fn at_rest(bot: &mut Bot, uuids: &[Uuid]) -> BTreeMap<Uuid, SeenEntity> {
    let lying = |bot: &Bot| -> Option<Lying> {
        uuids
            .iter()
            .map(|uuid| {
                let shown = shown(bot, *uuid);
                (shown.len() == 1 && shown[0].on_ground)
                    .then(|| (*uuid, (shown[0].entity_id, shown[0].position)))
            })
            .collect()
    };
    loop {
        bot.wait_until(PATIENCE, |bot| lying(bot).is_some())
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{error:#}: {} never showed {uuids:?} on the ground: {:?}",
                    bot.info.profile.name,
                    items_of(bot)
                )
            });
        let before = lying(bot);
        for _ in 0..TICKS_AT_REST {
            round_trip(bot).await;
        }
        if lying(bot) == before {
            return uuids
                .iter()
                .map(|uuid| (*uuid, shown(bot, *uuid)[0].clone()))
                .collect();
        }
    }
}

/// An item as the thrower's own connection was shown it.
#[derive(Debug, Clone)]
struct ShownItem {
    uuid: Uuid,
    position: (f64, f64, f64),
    on_ground: bool,
}

/// A player's raw connection after it joined, from which it turns, throws with Q and
/// walks, reading what it is shown meanwhile and following the items in it.
struct Thrower {
    connection: Connection,
    feet: (f64, f64, f64),
    /// The items it is shown, by wire id.
    items: BTreeMap<i32, ShownItem>,
    /// Every wire id each item was ever shown under.
    ever: BTreeMap<Uuid, BTreeSet<i32>>,
    /// The items it was told were removed, in order.
    gone: Vec<Uuid>,
    sequence: i32,
}

impl Thrower {
    /// Reads from `bot`'s connection from now on, going on following the items it showed.
    fn of(bot: Bot) -> Self {
        let sequence = bot.acknowledged_sequence;
        let feet = bot.location;
        let mut items = BTreeMap::new();
        let mut ever: BTreeMap<Uuid, BTreeSet<i32>> = BTreeMap::new();
        for entity in bot.entities.values() {
            if entity.kind == entity_types::ITEM {
                items.insert(
                    entity.entity_id,
                    ShownItem {
                        uuid: entity.uuid,
                        position: entity.position,
                        on_ground: entity.on_ground,
                    },
                );
                ever.entry(entity.uuid)
                    .or_default()
                    .insert(entity.entity_id);
            }
        }
        Self {
            connection: bot.into_connection(),
            feet,
            items,
            ever,
            gone: Vec::new(),
            sequence,
        }
    }

    /// Joins as `name`, stands at `x`, `z` with the chunks about it loaded, and reads
    /// from the raw connection from then on.
    async fn join(address: &str, name: &str, x: f64, z: f64) -> Self {
        let mut bot = join(address, name).await;
        stand(&mut bot, x, z).await;
        // A throw from a chunk that does not tick throws nothing; the chunks about the
        // player are ticked once they are loaded.
        bot.wait_until(PATIENCE, |bot| {
            let (x, z) = under(bot.location);
            let (x, z) = (x.div_euclid(16), z.div_euclid(16));
            (-1..=1).all(|dx| (-1..=1).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
        })
        .await
        .unwrap();
        Self::of(bot)
    }

    /// Turns to `yaw`, level.
    async fn look(&mut self, yaw: f32) {
        self.connection
            .write(&SetPlayerRotation {
                yaw,
                pitch: 0.0,
                flags: movement_flags::ON_GROUND,
            })
            .await
            .unwrap();
    }

    /// Puts one of `item` into hotbar slot 0 from the creative inventory and holds it.
    async fn hold(&mut self, item: i32) {
        self.connection
            .write(&SetCreativeModeSlot {
                slot: inventory::HOTBAR_START as i16,
                stack: Some(ItemStack::new(item, 1)),
            })
            .await
            .unwrap();
        self.connection
            .write(&SetHeldItem { slot: 0 })
            .await
            .unwrap();
    }

    /// Q.
    async fn press_q(&mut self) {
        self.connection
            .write(&PlayerAction {
                status: player_action::DROP_ITEM,
                position: Position { x: 0, y: 0, z: 0 },
                face: 0,
                sequence: 0,
            })
            .await
            .unwrap();
    }

    /// Turns to `yaw` and throws one `item` with Q, without waiting for anything.
    async fn throw(&mut self, item: i32, yaw: f32) {
        self.look(yaw).await;
        self.hold(item).await;
        self.press_q().await;
    }

    /// Moves to `x`, `z` in one step and ends its tick, as a client does.
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

    /// Walks to `x`, `z` at `speed` blocks a tick, a step every twentieth of a second,
    /// reading what comes meanwhile.
    async fn walk_to(&mut self, x: f64, z: f64, speed: f64) {
        loop {
            let (dx, dz) = (x - self.feet.0, z - self.feet.2);
            let distance = dx.hypot(dz);
            if distance < 1e-9 {
                return;
            }
            if distance <= speed + 1e-9 {
                self.step_to(x, z).await;
            } else {
                let fraction = speed / distance;
                self.step_to(self.feet.0 + dx * fraction, self.feet.2 + dz * fraction)
                    .await;
            }
            self.pump(Duration::from_millis(50)).await;
        }
    }

    /// The next frame that is not a keep-alive, which is answered, after following what
    /// it says of items; `None` once `deadline` has passed.
    async fn frame_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(frame) = timeout(left, self.connection.read_frame()).await else {
                return Ok(None);
            };
            let frame = frame?;
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
            return Ok(Some(frame.to_vec()));
        }
    }

    async fn next_frame(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        self.frame_until(deadline)
            .await?
            .context("nothing came in time")
    }

    /// Reads what comes for `duration`.
    async fn pump(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while self.frame_until(deadline).await.unwrap().is_some() {}
    }

    /// Breaks the bedrock under it, which nothing breaks, and returns once the server has
    /// acknowledged it.
    async fn round_trip(&mut self) {
        self.sequence += 1;
        let sequence = self.sequence;
        let (x, z) = under(self.feet);
        self.connection
            .write(&PlayerAction {
                status: player_action::START_DESTROY_BLOCK,
                position: Position { x, y: BEDROCK, z },
                face: face::TOP,
                sequence,
            })
            .await
            .unwrap();
        let deadline = Instant::now() + PATIENCE;
        loop {
            let frame = self.next_frame(deadline).await.unwrap();
            if matches!(
                ClientboundPlay::decode(&frame),
                Ok(ClientboundPlay::AcknowledgeBlockChange(answer)) if answer.sequence >= sequence
            ) {
                return;
            }
        }
    }

    /// Takes in what `frame` says of items, as a client does; fails where it shows an
    /// item twice.
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
                self.ever
                    .entry(spawn.uuid)
                    .or_default()
                    .insert(spawn.entity_id);
                self.items.insert(
                    spawn.entity_id,
                    ShownItem {
                        uuid: spawn.uuid,
                        position: (spawn.x, spawn.y, spawn.z),
                        on_ground: false,
                    },
                );
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
                        self.gone.push(item.uuid);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

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

    /// The item `uuid` as shown, with its wire id.
    fn item(&self, uuid: Uuid) -> Option<(i32, &ShownItem)> {
        (self.items.iter())
            .find(|(_, item)| item.uuid == uuid)
            .map(|(id, item)| (*id, item))
    }

    /// Reads until an item is spawned and its data follows, and returns its UUID; or
    /// `None` where the server puts hotbar slot 0 back as it was, which it does where it
    /// refuses a throw.
    async fn thrown_or_put_back(&mut self) -> Result<Option<Uuid>> {
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
                    let has_stack = data.values.iter().any(|(index, value)| {
                        *index == 8 && matches!(value, EntityDataValue::ItemStack(_))
                    });
                    ensure!(has_stack, "the item's data has no stack at index 8");
                    return Ok(Some(spawn.uuid));
                }
                _ => {}
            }
        }
    }

    /// Reads until `done` holds.
    async fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done(self) {
            if let Err(error) = self.next_frame(deadline).await {
                panic!("never {what}: {error:#}; shown {:?}", self.items);
            }
        }
    }

    /// Reads until every item of `uuids` is shown on the ground and stays as shown
    /// through [`TICKS_AT_REST`] ticks; returns them.
    async fn at_rest(&mut self, uuids: &[Uuid]) -> BTreeMap<Uuid, ShownItem> {
        let lying = |thrower: &Self| -> Option<Lying> {
            uuids
                .iter()
                .map(|uuid| {
                    let (id, item) = thrower.item(*uuid)?;
                    item.on_ground.then_some((*uuid, (id, item.position)))
                })
                .collect()
        };
        loop {
            self.until("shown the items at rest", |thrower| {
                lying(thrower).is_some()
            })
            .await;
            let before = lying(self);
            for _ in 0..TICKS_AT_REST {
                self.round_trip().await;
            }
            if lying(self) == before {
                return uuids
                    .iter()
                    .map(|uuid| (*uuid, self.item(*uuid).expect("shown").1.clone()))
                    .collect();
            }
        }
    }
}

/// `seen` lies at `lies`, give or take what two clients' views of an item at rest differ.
fn assert_lies_at(seen: (f64, f64, f64), lies: (f64, f64, f64)) {
    let off = [seen.0 - lies.0, seen.1 - lies.1, seen.2 - lies.2];
    assert!(
        off.iter().all(|off| off.abs() <= AT_REST),
        "the item is at {seen:?}, not where it lies, {lies:?}"
    );
}

/// What a watcher must have been shown of an item that crossed: it once, by the one wire
/// id it was first shown under, never taken away, lying east of the line where the
/// thrower is shown it lie.
fn assert_shown_once_across(bot: &Bot, uuid: Uuid, first_id: i32, lies: (f64, f64, f64)) {
    let name = &bot.info.profile.name;
    let shown = shown(bot, uuid);
    assert_eq!(shown.len(), 1, "{name} shows the item once: {shown:?}");
    assert_eq!(
        shown[0].entity_id, first_id,
        "{name} shows the item by the wire id it first showed it by"
    );
    assert!(
        !bot.vanished.iter().any(|gone| gone.uuid == uuid),
        "{name} was told the item went: {:?}",
        bot.vanished
    );
    assert!(
        shown[0].position.0 > LINE,
        "{name} shows it east of the line: {:?}",
        shown[0]
    );
    assert_lies_at(shown[0].position, lies);
}

/// Two watchers, west and east of the line and well away from where items go: a third
/// player's throws are for both of them, and neither picks anything up.
async fn watchers(address: &str) -> (Bot, Bot) {
    let mut west = join(address, "West").await;
    stand(&mut west, -6.5, 7.5).await;
    let mut east = join(address, "East").await;
    stand(&mut east, 6.5, 7.5).await;
    (west, east)
}

/// Waits until both watchers show the thrower, so that what they are spawned from then on
/// is the thrower's items alone; returns how many spawns each was shown so far.
async fn spawns_once_the_thrower_is_shown(west: &mut Bot, east: &mut Bot) -> [u32; 2] {
    for bot in [&mut *west, &mut *east] {
        bot.wait_until(PATIENCE, |bot| {
            bot.seen_player("Thrower").is_some()
                && (bot.seen_player("West").is_some() || bot.seen_player("East").is_some())
        })
        .await
        .unwrap();
        round_trip(bot).await;
    }
    [west.stats.entities_spawned, east.stats.entities_spawned]
}

/// Waits until `bot` shows the item `uuid`, and returns the wire id it shows it by.
async fn first_shown(bot: &mut Bot, uuid: Uuid) -> i32 {
    bot.wait_until(PATIENCE, |bot| shown(bot, uuid).len() == 1)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: {} never showed the item {uuid}",
                bot.info.profile.name
            )
        });
    shown(bot, uuid)[0].entity_id
}

/// The item went across and lies east of the line, the same for the thrower and both
/// watchers, each of whom was shown one item spawn, the item's, by one wire id, and
/// never told it went.
async fn seen_once_across(
    thrower: &mut Thrower,
    watchers: [&mut Bot; 2],
    uuid: Uuid,
    first_ids: [i32; 2],
    spawns_before: [u32; 2],
) -> (f64, f64, f64) {
    let lies = thrower.at_rest(&[uuid]).await[&uuid].position;
    assert!(lies.0 > LINE, "it lies east of the line: {lies:?}");
    assert!((lies.1 - FLOOR).abs() <= AT_REST, "on the floor: {lies:?}");
    assert_eq!(
        thrower.ever[&uuid].len(),
        1,
        "the thrower was shown it by one wire id: {:?}",
        thrower.ever[&uuid]
    );
    assert!(
        !thrower.gone.contains(&uuid),
        "the thrower was told it went"
    );
    for ((bot, first_id), before) in watchers.into_iter().zip(first_ids).zip(spawns_before) {
        at_rest(bot, &[uuid]).await;
        assert_shown_once_across(bot, uuid, first_id, lies);
        assert_eq!(
            bot.stats.entities_spawned,
            before + 1,
            "{} was shown one spawn, the item's: {}",
            bot.info.profile.name,
            bot.report()
        );
    }
    lies
}

/// The server is started again on the world in `directory`: the watchers, joining again,
/// are shown the item once, where it lay, east of the line.
async fn there_once_after_a_restart(
    directory: &std::path::Path,
    uuid: Uuid,
    lies: (f64, f64, f64),
) {
    let (server, address) = start_with(pinned(directory)).await;
    let mut west = join(&address, "West").await;
    let mut east = join(&address, "East").await;
    for bot in [&mut west, &mut east] {
        let first = first_shown(bot, uuid).await;
        round_trip(bot).await;
        assert_shown_once_across(bot, uuid, first, lies);
        assert_eq!(items_of(bot).len(), 1, "{:?}", items_of(bot));
    }
    server.stop().await;
}

/// The plan, section 1, and the record, sections 5 and 6: an item thrown by a player west
/// of a pinned line, east across it, goes on across it in flight, lands east of the
/// line, and is shown to the thrower and to players on both sides once, by one wire id,
/// with one spawn and never a removal. The server is stopped and started, and it is
/// there once where it lay.
#[tokio::test]
async fn an_item_thrown_across_a_pinned_line_is_seen_once_on_the_far_side_from_both_sides_and_outlasts_a_restart()
 {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let (mut west, mut east) = watchers(&address).await;
    let mut thrower = Thrower::join(&address, "Thrower", -1.5, 0.5).await;
    let spawns_before = spawns_once_the_thrower_is_shown(&mut west, &mut east).await;

    thrower.throw(items::STONE, EAST).await;
    let uuid = thrower
        .thrown_or_put_back()
        .await
        .unwrap()
        .expect("the throw is not refused");
    let (_, thrown) = thrower.item(uuid).expect("shown");
    assert!(
        thrown.position.0 < LINE,
        "thrown west of the line: {thrown:?}"
    );
    let first_ids = [
        first_shown(&mut west, uuid).await,
        first_shown(&mut east, uuid).await,
    ];
    let lies = seen_once_across(
        &mut thrower,
        [&mut west, &mut east],
        uuid,
        first_ids,
        spawns_before,
    )
    .await;
    drop(thrower);
    drop(west);
    drop(east);
    server.stop().await;
    there_once_after_a_restart(directory.path(), uuid, lies).await;
}

/// The plan, section 1: "A Q pressed right after crossing a pinned line throws (today it
/// is refused)". The thrower walks across the line and back, and presses Q in the same
/// breath as its last step each time: every throw throws, none is put back, and the
/// watchers are shown each item once, lying where the thrower is shown it.
#[tokio::test]
async fn a_q_pressed_right_after_crossing_a_pinned_line_throws_every_time() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let (mut west, mut east) = watchers(&address).await;
    let mut thrower = Thrower::join(&address, "Thrower", -2.5, 4.5).await;
    let mut thrown = Vec::new();
    for (crossing, kind) in KINDS.iter().take(4).enumerate() {
        let x = if crossing % 2 == 0 { 1.5 } else { -2.5 };
        thrower.walk_to(x, 4.5, 0.5).await;
        // Throws north, away from the path it walks, so that it never walks over what
        // it threw.
        thrower.throw(*kind, NORTH).await;
        match thrower.thrown_or_put_back().await.unwrap() {
            Some(uuid) => thrown.push(uuid),
            None => panic!("the Q pressed right after crossing {crossing} was refused"),
        }
    }
    let lying = thrower.at_rest(&thrown).await;
    for bot in [&mut west, &mut east] {
        let seen = at_rest(bot, &thrown).await;
        assert_eq!(items_of(bot).len(), thrown.len(), "{:?}", items_of(bot));
        for uuid in &thrown {
            assert_lies_at(seen[uuid].position, lying[uuid].position);
            assert!(!bot.vanished.iter().any(|gone| gone.uuid == *uuid));
        }
    }
    drop(thrower);
    server.stop().await;
}

/// The plan, section 1: an item washed by flowing water across a pinned line goes on
/// across it. An item lies west of the line; a player pours water three blocks west of
/// it, whose flow carries it east over the line. The thrower and both watchers are shown
/// it once by one wire id, east of the line, through what follows; the server is
/// stopped and started, and it is there once, east of the line.
#[tokio::test]
async fn an_item_washed_across_a_pinned_line_by_flowing_water_is_seen_once_on_the_far_side_and_outlasts_a_restart()
 {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let (mut west, mut east) = watchers(&address).await;
    let mut thrower = Thrower::join(&address, "Thrower", -1.5, 4.5).await;
    thrower.throw(items::STONE, NORTH).await;
    let uuid = thrower
        .thrown_or_put_back()
        .await
        .unwrap()
        .expect("the throw is not refused");
    let lies = thrower.at_rest(&[uuid]).await[&uuid].position;
    assert!(lies.0 < LINE, "it lies west of the line first: {lies:?}");
    let first_ids = [
        first_shown(&mut west, uuid).await,
        first_shown(&mut east, uuid).await,
    ];

    // The west watcher pours a spring in the item's row, three blocks and more west of
    // it, standing in it: the flow runs east along the row for seven blocks, over the
    // line, and carries the item with it.
    let row = lies.2.floor();
    let spring = (lies.0.floor() as i32 - 3, row as i32);
    stand(&mut west, f64::from(spring.0) + 0.5, row + 0.5).await;
    west.take_from_creative_inventory(0, items::WATER_BUCKET)
        .await
        .unwrap();
    west.select_slot(0).await.unwrap();
    let poured = west.use_item(0.0, 90.0).await.unwrap();
    west.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= poured)
        .await
        .unwrap();
    west.wait_until(PATIENCE, |bot| {
        bot.block_at(spring.0, -60, spring.1)
            .unwrap()
            .is_some_and(|state| BlockState(state as u16).props().fluid().is_some())
    })
    .await
    .unwrap_or_else(|error| panic!("{error:#}: the water was never poured"));

    // Shown across, by everyone; an item in water is not at rest, so this waits for it
    // to be east of the line, and then for the edge's updates of a few seconds.
    thrower
        .until("shown the item east of the line", |thrower| {
            thrower
                .item(uuid)
                .is_some_and(|(_, item)| item.position.0 > LINE + 0.25)
        })
        .await;
    for bot in [&mut west, &mut east] {
        bot.wait_until(PATIENCE, |bot| {
            shown(bot, uuid)
                .first()
                .is_some_and(|item| item.position.0 > LINE + 0.25)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: {} never showed the item across: {:?}",
                bot.info.profile.name,
                items_of(bot)
            )
        });
    }
    for _ in 0..TICKS_AT_REST {
        thrower.round_trip().await;
    }
    assert_eq!(thrower.ever[&uuid].len(), 1, "{:?}", thrower.ever);
    assert!(!thrower.gone.contains(&uuid));
    let (_, now) = thrower.item(uuid).expect("still shown");
    assert!(now.position.0 > LINE, "{now:?}");
    for (bot, first_id) in [&mut west, &mut east].into_iter().zip(first_ids) {
        round_trip(bot).await;
        let seen = shown(bot, uuid);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].entity_id, first_id, "one wire id throughout");
        assert!(seen[0].position.0 > LINE, "{seen:?}");
        assert!(!bot.vanished.iter().any(|gone| gone.uuid == uuid));
        assert_eq!(items_of(bot).len(), 1, "{:?}", items_of(bot));
    }
    drop(thrower);
    drop(west);
    drop(east);
    server.stop().await;

    let (server, address) = start_with(pinned(directory.path())).await;
    let mut west = join(&address, "West").await;
    let mut east = join(&address, "East").await;
    for bot in [&mut west, &mut east] {
        first_shown(bot, uuid).await;
        round_trip(bot).await;
        let seen = shown(bot, uuid);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].position.0 > LINE, "{seen:?}");
        assert_eq!(items_of(bot).len(), 1, "{:?}", items_of(bot));
    }
    server.stop().await;
}

/// The plan, sections 1 and 5 (G1, "Pistons send from the block entities' phase"): an
/// item lying in the block west of a pinned line is pushed across it by a piston west of
/// it, facing east. The thrower and both watchers are shown it once, by one wire id,
/// with one spawn, east of the line; the server is stopped and started, and it is there
/// once where it lay.
#[tokio::test]
async fn an_item_pushed_across_a_pinned_line_by_a_piston_is_seen_once_on_the_far_side_and_outlasts_a_restart()
 {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let (mut west, mut east) = watchers(&address).await;
    let mut thrower = Thrower::join(&address, "Thrower", -0.5, 4.5).await;
    let spawns_before = spawns_once_the_thrower_is_shown(&mut west, &mut east).await;
    thrower.throw(items::STONE, NORTH).await;
    let uuid = thrower
        .thrown_or_put_back()
        .await
        .unwrap()
        .expect("the throw is not refused");
    let lies = thrower.at_rest(&[uuid]).await[&uuid].position;
    // Thrown straight north from x = -0.5, it strays a little sideways at most.
    assert!(
        (-1.0 + 0.125..LINE - 0.125).contains(&lies.0),
        "it lies in the block west of the line: {lies:?}"
    );
    let row = lies.2.floor() as i32;
    let first_ids = [
        first_shown(&mut west, uuid).await,
        first_shown(&mut east, uuid).await,
    ];

    // The west watcher flies up over the place, out of the item's reach, looking west,
    // so that the piston it places faces east, towards the line: a stone in the block
    // west of the item's, the piston west of that, and a block of redstone behind it,
    // which powers it; the stone pushed into the item's block pushes the item over the
    // line.
    west.set_flying(true).await.unwrap();
    west.fly_to((-4.5, -58.5, f64::from(row) + 0.5), (WEST, 0.0), 1.0)
        .await
        .unwrap();
    for (slot, (item, x)) in [
        (items::STONE, -2),
        (items::PISTON, -3),
        (items::REDSTONE_BLOCK, -4),
    ]
    .into_iter()
    .enumerate()
    {
        west.take_from_creative_inventory(slot as i16, item)
            .await
            .unwrap();
        west.select_slot(slot as i16).await.unwrap();
        west.use_item_on(x, -61, row, face::TOP).await.unwrap();
        west.wait_until(PATIENCE, |bot| {
            bot.block_at(x, -60, row)
                .unwrap()
                .is_some_and(|state| !BlockState(state as u16).props().is_air())
        })
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the block {item} was never placed"));
    }
    // The stone is pushed into the block where the item lay.
    west.wait_until(PATIENCE, |bot| {
        bot.block_at(-1, -60, row)
            .unwrap()
            .is_some_and(|state| !BlockState(state as u16).props().is_air())
    })
    .await
    .unwrap_or_else(|error| {
        let states: Vec<Option<i32>> = (-5..=0)
            .map(|x| west.block_at(x, -60, row).unwrap())
            .collect();
        panic!(
            "{error:#}: the piston never pushed; the row from x = -5 to 0 has {states:?} \
             (a piston's states begin at {:?})",
            clustine_data::blocks::PISTON
        )
    });

    let lies = seen_once_across(
        &mut thrower,
        [&mut west, &mut east],
        uuid,
        first_ids,
        spawns_before,
    )
    .await;
    drop(thrower);
    drop(west);
    drop(east);
    server.stop().await;
    there_once_after_a_restart(directory.path(), uuid, lies).await;
}

/// A cluster's directory, with its world and the processes' logs, kept where the test
/// fails so that what the processes said can be read.
struct KeptOnFailure(Option<tempfile::TempDir>);

impl KeptOnFailure {
    fn new(prefix: &str) -> Self {
        Self(Some(
            tempfile::Builder::new().prefix(prefix).tempdir().unwrap(),
        ))
    }

    fn path(&self) -> &std::path::Path {
        self.0.as_ref().expect("until dropped").path()
    }
}

impl Drop for KeptOnFailure {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.0.take()
        {
            eprintln!("kept: {}", directory.keep().display());
        }
    }
}

/// What is killed around a crossing.
#[derive(Debug, Clone, Copy)]
enum Victim {
    /// The worker that runs the region west of the line, which sends.
    Sender,
    /// The worker that runs the region east of the line, which takes the item in.
    Holder,
    /// The world store.
    Store,
    /// Not killed: the region east of the line, which takes the item in, moved to another
    /// worker, as the coordinator moves one to even regions out (`clustine move`).
    MovedHolder,
}

/// When around a crossing it is killed.
#[derive(Debug, Clone, Copy)]
enum Moment {
    /// Right after the Q is sent: before the throwing tick's commit, or about it.
    AtOnce,
    /// Once the thrower is shown the item: the throwing tick is durable, the item
    /// in flight towards the line, and its sending one or a few ticks off.
    Shown,
    /// Once a watcher east of the line shows it across: sent, and taken or about to be.
    Across,
    /// Once it lies at rest on the far side: taken, and the taking durable.
    AtRest,
}

const MOMENTS: [Moment; 4] = [
    Moment::AtOnce,
    Moment::Shown,
    Moment::Across,
    Moment::AtRest,
];

/// A cluster pinned at chunk x = 0 and 4, a thrower west of the line at x = 0 and two
/// watchers, one on either side of it.
struct Crossings {
    _turn: Turn,
    cluster: Cluster,
    _directory: KeptOnFailure,
    west: Bot,
    east: Bot,
    thrower: Thrower,
}

impl Crossings {
    async fn start() -> Self {
        let turn = turn().await;
        let directory = KeptOnFailure::new("clustine-crossing-");
        // One worker for each region and one to spare.
        let mut cluster = Cluster::new(directory.path(), 4, "0,4").await;
        cluster.start().await;
        let deadline = Instant::now() + RECOVERY;
        while !(0..3).all(|region| cluster.runs(region)) {
            assert!(
                Instant::now() < deadline,
                "the cluster never ran its regions:\n{}",
                cluster.all_logs()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let address = cluster.edge.0.clone();
        let (west, east) = watchers(&address).await;
        let thrower = Thrower::join(&address, "Thrower", -1.5, 0.5).await;
        Self {
            _turn: turn,
            cluster,
            _directory: directory,
            west,
            east,
            thrower,
        }
    }

    /// Reads what every player is sent for a moment.
    async fn tend(&mut self) {
        self.west.idle(Duration::from_millis(30)).await.unwrap();
        self.east.idle(Duration::from_millis(30)).await.unwrap();
        self.thrower.pump(Duration::from_millis(30)).await;
    }

    /// Waits until `done` holds of the cluster, the players tended meanwhile.
    async fn until(&mut self, what: &str, done: impl Fn(&Cluster) -> bool) {
        let deadline = Instant::now() + RECOVERY;
        while !done(&self.cluster) {
            assert!(
                Instant::now() < deadline,
                "never {what}:\n{}",
                self.cluster.all_logs()
            );
            self.tend().await;
        }
    }

    /// Waits until every region runs on a worker the edge is linked to, and every player
    /// has an action acknowledged after that.
    async fn whole(&mut self) {
        self.until("every region runs", |cluster| {
            (0..3).all(|region| cluster.runs(region))
        })
        .await;
        round_trip(&mut self.west).await;
        round_trip(&mut self.east).await;
        self.thrower.round_trip().await;
    }

    /// Kills `victim` without warning and waits until the cluster is whole again, with
    /// a worker to spare.
    async fn kill(&mut self, victim: Victim) {
        let region = match victim {
            Victim::Sender => 0,
            Victim::Holder => 1,
            Victim::Store => {
                self.cluster.kill_and_start_the_store().await;
                self.whole().await;
                return;
            }
            Victim::MovedHolder => {
                let before = self.cluster.routes()[&1].clone();
                let asked = common::processes::ask(
                    "a move of region 1".to_owned(),
                    self.cluster.move_command(1, None),
                );
                while !asked.is_finished() {
                    self.tend().await;
                }
                let asked = asked.await.unwrap();
                println!("moved: {}", asked.outcome());
                assert_eq!(asked.code, Some(0), "{}", asked.outcome());
                self.until("the moved region runs on its new worker", |cluster| {
                    cluster
                        .routes()
                        .get(&1)
                        .is_some_and(|route| *route != before)
                        && cluster.runs(1)
                })
                .await;
                self.whole().await;
                return;
            }
        };
        let worker = self
            .cluster
            .owner(region)
            .unwrap_or_else(|| panic!("region {region} has no owner"));
        println!("killing worker {worker}, which runs region {region}");
        self.cluster.kill_worker(worker).await;
        let gone = self.cluster.routes()[&region].clone();
        self.until(
            "another worker runs the killed worker's region",
            |cluster| cluster.routes().get(&region) != Some(&gone) && cluster.runs(region),
        )
        .await;
        self.cluster.start_worker_again(worker);
        self.until("the killed worker is back", |cluster| {
            let log = cluster.log_since(worker);
            log.contains("waiting to be given a region") || log.contains("given a region")
        })
        .await;
        self.whole().await;
    }

    /// Every item any of the players was ever shown.
    fn ever_shown(&self) -> BTreeSet<Uuid> {
        let mut uuids: BTreeSet<Uuid> = self.thrower.ever.keys().copied().collect();
        for bot in [&self.west, &self.east] {
            uuids.extend(
                bot.entities
                    .values()
                    .chain(&bot.vanished)
                    .filter(|entity| entity.kind == entity_types::ITEM)
                    .map(|entity| entity.uuid),
            );
        }
        uuids
    }

    /// Throws one `item` east across the line and kills `victim` at `moment` of its
    /// crossing; returns once the cluster is whole again.
    async fn round(&mut self, item: i32, victim: Victim, moment: Moment) {
        println!(
            "{item}: killing the {victim:?} {moment:?}; the sender runs on worker {:?}, the holder on {:?}",
            self.cluster.owner(0),
            self.cluster.owner(1)
        );
        self.thrower.throw(item, EAST).await;
        if !matches!(moment, Moment::AtOnce) {
            let uuid = self
                .thrower
                .thrown_or_put_back()
                .await
                .unwrap()
                .expect("the throw is not refused");
            match moment {
                Moment::AtOnce | Moment::Shown => {}
                Moment::Across => {
                    self.east
                        .wait_until(PATIENCE, |bot| {
                            shown(bot, uuid)
                                .first()
                                .is_some_and(|item| item.position.0 > LINE + 0.25)
                        })
                        .await
                        .unwrap_or_else(|error| {
                            panic!("{error:#}: East never showed the item across")
                        });
                }
                Moment::AtRest => {
                    self.thrower.at_rest(&[uuid]).await;
                }
            }
        }
        self.kill(victim).await;
    }

    /// Every item any player was shown is there once, at rest east of the line, the same
    /// for the thrower and both watchers; no more items than were thrown.
    async fn there_once(&mut self, throws: usize) -> BTreeMap<Uuid, (f64, f64, f64)> {
        let uuids: Vec<Uuid> = self.ever_shown().into_iter().collect();
        let lying = self.thrower.at_rest(&uuids).await;
        assert_eq!(
            self.thrower.items.len(),
            uuids.len(),
            "the thrower shows no other item: {:?}",
            self.thrower.items
        );
        assert!(
            uuids.len() <= throws,
            "{} items of {throws} throws",
            uuids.len()
        );
        for bot in [&mut self.west, &mut self.east] {
            let seen = at_rest(bot, &uuids).await;
            assert_eq!(items_of(bot).len(), uuids.len(), "{:?}", items_of(bot));
            for uuid in &uuids {
                assert!(
                    seen[uuid].position.0 > LINE,
                    "{} shows {uuid} west of the line: {:?}",
                    bot.info.profile.name,
                    seen[uuid]
                );
                assert_lies_at(seen[uuid].position, lying[uuid].position);
            }
        }
        lying
            .into_iter()
            .map(|(uuid, item)| (uuid, item.position))
            .collect()
    }

    /// Kills every process at once and starts them again on the same disk: an auditor
    /// who joins is shown every item of `lying` once, where it lay, and no other.
    async fn after_the_power_goes(mut self, lying: BTreeMap<Uuid, (f64, f64, f64)>) {
        drop(self.thrower);
        drop(self.west);
        drop(self.east);
        self.cluster.kill().await;
        self.cluster.start().await;
        let address = self.cluster.edge.0.clone();
        // The thrower comes back to look, where it stood throughout and is put again: a
        // newcomer at the spawn point would be within reach of items that lie less than
        // two blocks east of the line, and pick them up, as the game's players do.
        let mut auditor = join(&address, "Thrower").await;
        let uuids: Vec<Uuid> = lying.keys().copied().collect();
        auditor
            .wait_until(RECOVERY, |bot| {
                uuids.iter().all(|uuid| shown(bot, *uuid).len() == 1)
            })
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{error:#}: after a restart the auditor shows {:?}, not {lying:?}",
                    items_of(&auditor)
                )
            });
        round_trip(&mut auditor).await;
        assert_eq!(
            items_of(&auditor).len(),
            uuids.len(),
            "{:?}",
            items_of(&auditor)
        );
        for (uuid, lies) in &lying {
            assert_lies_at(shown(&auditor, *uuid)[0].position, *lies);
        }
        drop(auditor);
        self.cluster.terminate().await;
    }
}

/// Throws an item across the line for each [`Moment`], killing `victim` at it, and checks
/// that every item any player was shown is there exactly once, before and after every
/// process is killed and started again.
async fn killed_around_crossings(victim: Victim) {
    let mut crossings = Crossings::start().await;
    for (round, (moment, item)) in MOMENTS.into_iter().zip(KINDS).enumerate() {
        crossings.round(item, victim, moment).await;
        // Each round's item is there once before the next, so that a failure says which
        // moment it is of.
        let lying = crossings.there_once(round + 1).await;
        println!("{victim:?} after {moment:?}: {lying:?}");
    }
    let lying = crossings.there_once(MOMENTS.len()).await;
    println!(
        "{victim:?}: {} of {} items are there",
        lying.len(),
        MOMENTS.len()
    );
    // Killed once it was shown, across or at rest, an item is durable and must be there.
    assert!(
        lying.len() >= MOMENTS.len() - 1,
        "the items thrown before the kills that came once they were shown are there: {lying:?}"
    );
    crossings.after_the_power_goes(lying).await;
}

/// The record, section 5 ("Commit and arrival are one group, so the entity is in exactly
/// one place in every durable state"): the worker of the sending region is killed at
/// each moment of a crossing, and every item is there once.
#[tokio::test(flavor = "multi_thread")]
async fn an_item_crossing_a_line_is_there_once_however_the_sending_worker_is_killed() {
    if a_repetition() {
        return;
    }
    killed_around_crossings(Victim::Sender).await;
}

/// As above, the worker of the region that takes the item in killed at each moment.
#[tokio::test(flavor = "multi_thread")]
async fn an_item_crossing_a_line_is_there_once_however_the_holding_worker_is_killed() {
    if a_repetition() {
        return;
    }
    killed_around_crossings(Victim::Holder).await;
}

/// As above, the world store killed at each moment.
#[tokio::test(flavor = "multi_thread")]
async fn an_item_crossing_a_line_is_there_once_however_the_world_store_is_killed() {
    if a_repetition() {
        return;
    }
    killed_around_crossings(Victim::Store).await;
}

/// Has `walker` walk to `to` at `speed` blocks a tick while `tended` reads what it is
/// sent.
async fn walk_tending(walker: &mut Bot, to: (f64, f64), speed: f64, tended: &mut Bot) {
    let walk = walker.walk_to(to.0, to.1, speed);
    tokio::pin!(walk);
    loop {
        tokio::select! {
            walked = &mut walk => return walked.unwrap(),
            idled = tended.idle(Duration::from_millis(50)) => idled.unwrap(),
        }
    }
}

/// The record, section 5 ("A holder that cannot take it yet: in a handshake, in
/// `Preparing`, being moved, or restarting: the arrival stays filed and is sent again at
/// the holder's hello"): the region that takes the item in is moved to another worker at
/// each moment of a crossing, and every item is there once.
#[tokio::test(flavor = "multi_thread")]
async fn an_item_crossing_a_line_is_there_once_however_the_holding_region_is_moved() {
    if a_repetition() {
        return;
    }
    killed_around_crossings(Victim::MovedHolder).await;
}

/// The record, section 7, and the plan, N3f: where regions follow their players, an
/// entity within four blocks of the line between two regions whose holder sent an
/// arrival across it makes them merge at once, as a circuit at the line does. A wanderer
/// walks so far east that it is split off; the thrower walks to the line the split left
/// and throws an item across it, while the two players are farther apart than the merge
/// distance, so that nothing but the item makes them merge. The coordinator begins the
/// merge for work at the line, and the item is shown to the thrower once throughout.
#[tokio::test(flavor = "multi_thread")]
async fn an_item_thrown_across_the_line_between_regions_that_follow_players_merges_them_at_once() {
    use common::following;
    use common::wandering::{absorbed_by, bounds};

    if a_repetition() {
        return;
    }
    let _turn = turn().await;
    let directory = KeptOnFailure::new("clustine-crossing-merge-");
    let mut cluster = Cluster::new(directory.path(), 3, "").await;
    cluster.coordinator_arguments = following::coordinator_arguments();
    cluster.view_distance = following::VIEW;
    cluster.start().await;
    let address = cluster.edge.0.clone();
    let mut thrower_bot = join_with_view(&address, "Thrower", following::VIEW).await;
    let mut wanderer = join_with_view(&address, "Wanderer", following::VIEW).await;

    // Far enough east to be split off: 40 chunks, twice the split distance of 20.
    let far = 40 * 16 + 8;
    walk_tending(
        &mut wanderer,
        (f64::from(far) + 0.5, 0.5),
        following::TO_THE_LANE,
        &mut thrower_bot,
    )
    .await;
    let deadline = Instant::now() + following::SETTLES_WITHIN;
    let (list, home, away) = loop {
        let list = tokio::task::spawn_blocking({
            let store = cluster.store.0.clone();
            move || clustine_worldstore::regions(&store)
        })
        .await
        .unwrap();
        if let Ok(list) = list {
            let home = following::region_of(&list, (0, 0));
            let away = following::region_of(&list, (40, 0));
            if let (Some(home), Some(away)) = (home, away)
                && home != away
            {
                break (list, home, away);
            }
        }
        assert!(
            Instant::now() < deadline,
            "the wanderer was never split off:\n{}",
            cluster.all_logs()
        );
        thrower_bot.idle(Duration::from_millis(50)).await.unwrap();
        wanderer.idle(Duration::from_millis(50)).await.unwrap();
    };
    println!("split: {list:?}");
    let (Some(near), Some(off)) = (bounds(&list, home), bounds(&list, away)) else {
        panic!("both regions hold land: {list:?}");
    };
    assert!(
        near.max.x < off.min.x,
        "the home region's land is west of the wanderer's: {list:?}"
    );
    // The line the split left, between the two regions' land, in the chunk row of the
    // spawn, where the wanderer walked.
    assert_eq!(
        near.max.x + 1,
        off.min.x,
        "the two regions' land meets at a line: {list:?}"
    );
    let line = off.min.x * 16;
    let merge_distance = 2 * following::VIEW + 8;
    assert!(
        40 - (off.min.x - 1) > merge_distance,
        "the thrower at the line is farther than the merge distance from the wanderer: {list:?}"
    );

    // The thrower walks to two and a half blocks west of the line and throws east.
    let mut thrower = {
        walk_tending(
            &mut thrower_bot,
            (f64::from(line) - 2.5, 0.5),
            following::TO_THE_LANE,
            &mut wanderer,
        )
        .await;
        round_trip(&mut thrower_bot).await;
        thrower_bot
            .wait_until(PATIENCE, |bot| {
                let (x, z) = under(bot.location);
                let (x, z) = (x.div_euclid(16), z.div_euclid(16));
                (-1..=1).all(|dx| (-1..=1).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
            })
            .await
            .unwrap();
        Thrower::of(thrower_bot)
    };
    let coordinator_before = cluster.log("coordinator").len();
    thrower.throw(items::STONE, EAST).await;
    let uuid = thrower
        .thrown_or_put_back()
        .await
        .unwrap()
        .expect("the throw is not refused");

    let begun = "a merge is begun for a circuit at the line";
    let deadline = Instant::now() + PATIENCE;
    loop {
        let log = cluster.log("coordinator");
        let since = log.get(coordinator_before..).unwrap_or_default();
        if since.contains(begun) {
            assert!(
                !since.contains("a merge is begun by the distances"),
                "the merge is the item's, not the distances': {since}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the coordinator never began a merge for the item at the line:\n{}",
            cluster.all_logs()
        );
        thrower.pump(Duration::from_millis(50)).await;
        wanderer.idle(Duration::from_millis(50)).await.unwrap();
    }
    let deadline = Instant::now() + PATIENCE;
    loop {
        let store = cluster.store.0.clone();
        let list = tokio::task::spawn_blocking(move || clustine_worldstore::regions(&store))
            .await
            .unwrap();
        if let Ok(list) = list
            && (absorbed_by(&list, away) == Some(home) || absorbed_by(&list, home) == Some(away))
        {
            println!("merged: {list:?}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the two regions never merged:\n{}",
            cluster.all_logs()
        );
        thrower.pump(Duration::from_millis(50)).await;
        wanderer.idle(Duration::from_millis(50)).await.unwrap();
    }
    let lying = thrower.at_rest(&[uuid]).await;
    assert!(lying[&uuid].position.0 > f64::from(line), "{lying:?}");
    assert_eq!(
        thrower.ever[&uuid].len(),
        1,
        "one wire id: {:?}",
        thrower.ever
    );
    assert!(!thrower.gone.contains(&uuid), "never taken away");
    drop(thrower);
    drop(wanderer);
    cluster.terminate().await;
}

/// The record, section 5 ("Held and not loaded"): an item thrown across a pinned line
/// into the holder's land that none of its players uses is taken all the same, the chunk
/// loaded for it with an arrival ticket that lasts until the arrival is taken and 100
/// ticks more, "the chunk goes with its next save when the ticket ends". Nobody but the
/// thrower, west of the line, is near: once the ticket has ended, the thrower comes back
/// and is shown the item where it lay; and so after the server is stopped and started.
/// It is the thrower who comes back, put where it stood: a newcomer at the spawn point
/// would be within reach of an item that lies less than two blocks east of the line,
/// and pick it up, as the game's players do.
#[tokio::test]
async fn an_item_thrown_into_land_its_holder_does_not_use_is_kept_when_the_chunk_is_let_go() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut thrower = Thrower::join(&address, "Thrower", -1.5, 0.5).await;
    thrower.throw(items::STONE, EAST).await;
    let uuid = thrower
        .thrown_or_put_back()
        .await
        .unwrap()
        .expect("the throw is not refused");
    let lies = thrower.at_rest(&[uuid]).await[&uuid].position;
    assert!(lies.0 > LINE, "it lies east of the line: {lies:?}");
    // Past the ticket's 100 ticks after the taking, each round trip a tick at least.
    for _ in 0..200 {
        thrower.round_trip().await;
    }
    drop(thrower);
    let mut back = join(&address, "Thrower").await;
    back.wait_until(PATIENCE, |bot| shown(bot, uuid).len() == 1)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: the thrower is never shown again the item that lies at {lies:?}; it shows {:?}",
                items_of(&back)
            )
        });
    assert_lies_at(shown(&back, uuid)[0].position, lies);
    drop(back);
    server.stop().await;
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut auditor = join(&address, "Thrower").await;
    auditor
        .wait_until(PATIENCE, |bot| shown(bot, uuid).len() == 1)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: after a restart the item that lay at {lies:?} is not there: {:?}",
                items_of(&auditor)
            )
        });
    assert_lies_at(shown(&auditor, uuid)[0].position, lies);
    server.stop().await;
}

/// The record, section 5: an arrival goes on its holder's stream whoever runs the holder,
/// and a moved region is opened by its new worker with a hello like any other. The region
/// east of the line is moved to another worker; well after, an item is thrown across the
/// line into it, and lies at rest east of the line, shown once, by one wire id; so it does
/// after the region is moved once more.
#[tokio::test(flavor = "multi_thread")]
async fn an_item_thrown_into_a_region_moved_a_while_before_comes_to_rest_on_the_far_side() {
    if a_repetition() {
        return;
    }
    let mut crossings = Crossings::start().await;
    crossings.kill(Victim::MovedHolder).await;
    // A while: the moved region has run for some hundred ticks.
    for _ in 0..100 {
        crossings.thrower.round_trip().await;
    }
    crossings.thrower.throw(items::STONE, EAST).await;
    let uuid = crossings
        .thrower
        .thrown_or_put_back()
        .await
        .unwrap()
        .expect("the throw is not refused");
    let lies = crossings.thrower.at_rest(&[uuid]).await[&uuid].position;
    assert!(lies.0 > LINE, "{lies:?}");
    crossings.kill(Victim::MovedHolder).await;
    let lying = crossings.there_once(1).await;
    assert_lies_at(lying[&uuid], lies);
    assert_eq!(crossings.thrower.ever[&uuid].len(), 1);
    crossings.after_the_power_goes(lying).await;
}
