//! One stay per player, and a player's place kept by the world store: step R1.7 of
//! `docs/adr/0020-one-stay-per-player.md`, end to end, **written from the record by
//! someone who has not read the code that implements it**. What each test expects is
//! what the record says (its sections 4, 5, 6, 9, 10 and 13, "What a player notices"
//! and "What is measured (R1.7)"), never what the server happens to do.
//!
//! Three kinds of server are played on: one process (`start`), one process whose world
//! is several regions (pins, or regions that follow their players), and a cluster of
//! processes with **two edges**, which the helpers start as a second `clustine edge`
//! with a name of its own.
//!
//! The tests of the record's section 10, "Every order of events", are **named by
//! their row**. The rows a test cannot reach from outside, and why, are at the end of
//! this file.
//!
//! How a test knows that the store has a place: a bot sends a *pulse* after its last
//! step, an action on a block far out of reach that a region acknowledges and does
//! nothing else about. What a tick produced is published only when the store has
//! confirmed the tick's commit, and a region applies what one connection sent in its
//! order, so the acknowledgement says that the commit with the step's note is on disk.
//!
//! That nobody ever sees two entities of one player is the bots' own rule: a bot that
//! is shown a second entity with the UUID of one it has fails, and so does whatever
//! waits on it here.
//!
//! The measurements print what they measured; run them with `--nocapture` to read it.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clustine::{Config, Server};
use clustine_botswarm::{Behaviour, Bot, Connection, Ending, Entry, Reason, Wire, intention};
use clustine_coordinator::Policy;
use clustine_data::{GAME_VERSION, entity_types, items};
use clustine_protocol::codec::Writer;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientInformation, ClientboundConfiguration,
    ServerboundKeepAlive as ConfigurationKeepAlive, ServerboundKnownPacks,
    ServerboundPluginMessage,
};
use clustine_protocol::packets::handshake::Intent;
use clustine_protocol::packets::login::{ClientboundLogin, LoginAcknowledged, LoginStart};
use clustine_protocol::packets::play::{
    ClientboundPlay, ServerboundKeepAlive, SynchronizePlayerPosition,
};
use clustine_protocol::text::Text;
use clustine_region::RegionId;
use clustine_rpc::RegionList;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use uuid::Uuid;

use common::homes::{other_homes_are_untouched, overworld, overworld_list};
use common::processes::{Cluster, Turn, ask, turn, turn_alone};
use common::{VIEW_DISTANCE, config, free_address, spawn_server, start, start_with};

/// How long anything may take that is merely waited for. It only ever runs out when
/// something hangs.
const PATIENCE: Duration = Duration::from_secs(30);

/// For what takes rests of the coordinator or leases to come about.
const LONG_PATIENCE: Duration = Duration::from_secs(240);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

/// The height players stand at in a flat world, and where a new player enters it.
const GROUND: f64 = -60.0;
const SPAWN_POINT: (f64, f64, f64) = (0.5, GROUND, 0.5);

/// The world's lowest block is at this height: feet below it are below the world
/// (the record's section 4, "A place below the world counts as no place").
const LOWEST: f64 = -64.0;

/// A look that no server gives a player by itself, as yaw and pitch in degrees.
const A_LOOK: (f32, f32) = (135.0, 30.0);
const ANOTHER_LOOK: (f32, f32) = (40.0, -25.0);

/// Blocks a tick on the way to a place: fast, and far below what the tests of
/// regions that follow their players run at.
const SPEED: f64 = 2.0;

/// A place far from the spawn point that someone at the spawn point still sees with
/// the tests' view distance. With `CLUSTINE_TEST_PINS=0,4` it is in another region
/// than the spawn point.
const FAR: (f64, f64, f64) = (72.5, GROUND, -40.5);

/// The key of the sentence a connection is ended with when its player logs in again.
const SENTENCE: &str = "multiplayer.disconnect.duplicate_login";

/// Whether `reason` is the sentence as the record has it: the compound with the one
/// key, which each client shows in its own language.
fn is_the_sentence(reason: &Reason) -> bool {
    let sentence = Text::translatable(SENTENCE).to_nbt();
    let mut bytes = Writer::new();
    bytes.put_nbt(&sentence);
    let expected = Wire::Nbt {
        bytes: bytes.into_bytes(),
        value: sentence,
    };
    reason.wire == expected
}

/// Whether a connection ended with a disconnect packet that has the sentence.
fn ended_with_the_sentence(ending: &Option<Ending>) -> bool {
    matches!(ending, Some(Ending::Disconnected(reason)) if is_the_sentence(reason))
}

fn near(a: (f64, f64, f64), b: (f64, f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01 && (a.2 - b.2).abs() < 0.01
}

/// Whether two angles in degrees are the same direction.
fn same_angle(a: f32, b: f32) -> bool {
    let apart = (a - b).rem_euclid(360.0);
    !(0.01..=359.99).contains(&apart)
}

/// Joins and waits until the bot has the chunk it stands in and has said so, by when
/// it has been sent its inventory too.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name)
        .await
        .unwrap_or_else(|error| panic!("{name} was not let in: {error:#}"));
    loaded(&mut bot).await;
    bot
}

async fn loaded(bot: &mut Bot) {
    if let Err(error) = bot.wait_until(PATIENCE, Bot::is_loaded).await {
        panic!(
            "{error:#}: {} was not given the chunk they stand in\n{}",
            bot.info.profile.name,
            bot.report()
        );
    }
}

/// Sends a pulse and waits for its acknowledgement: everything the bot sent before
/// it has been applied and is in a commit the store has confirmed.
async fn heard(bot: &mut Bot) {
    let (x, _, z) = bot.location;
    // Far above the bot's head, out of reach: acknowledged and nothing else.
    let block = (x.floor() as i32, GROUND as i32 + 64, z.floor() as i32);
    let sequence = bot.dig(block.0, block.1, block.2).await.unwrap();
    let acknowledged = bot
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await;
    if let Err(error) = acknowledged {
        panic!(
            "{error:#}: {} at {:?} was not answered\n{}",
            bot.info.profile.name,
            bot.location,
            bot.report()
        );
    }
}

/// Moves `bot` to `place` while it looks along `look`, and waits until the store has
/// that.
async fn goes_to(bot: &mut Bot, place: (f64, f64, f64), look: (f32, f32)) {
    bot.fly_to(place, look, SPEED).await.unwrap();
    heard(bot).await;
}

/// What a player holds: the hotbar and the selected slot.
type Kit = ([Option<ItemStack>; 9], i32);

/// Has `bot` put glass and bricks into two slots that a new player has nothing in or
/// something else in, and select the second: a hotbar and a slot that no join gives.
/// Returns what the bot then holds, from the hotbar it was sent on entering.
async fn furnish(bot: &mut Bot) -> Kit {
    let mut hotbar = bot.hotbar.clone();
    for (slot, item) in [(2, items::GLASS), (6, items::BRICKS)] {
        bot.take_from_creative_inventory(slot, item).await.unwrap();
        hotbar[slot as usize] = Some(ItemStack::new(item, 1));
    }
    bot.select_slot(6).await.unwrap();
    (hotbar, 6)
}

/// What the kit is for someone who has only just entered and done nothing to it.
fn kit_of(bot: &Bot) -> Kit {
    (bot.hotbar.clone(), bot.selected_slot)
}

/// A player as they were when they left, or as their latest connection last was.
#[derive(Debug, Clone)]
struct Left {
    name: String,
    uuid: Uuid,
    /// The wire id its edge gave that stay, which says nothing of the order of stays:
    /// that is in the keys, which no client sees (`docs/adr/0028-entities.md`, section 1).
    entity: i32,
    place: (f64, f64, f64),
    look: (f32, f32),
    flying: bool,
    hotbar: [Option<ItemStack>; 9],
    slot: i32,
}

impl Left {
    fn of(bot: &Bot, kit: &Kit) -> Self {
        Self {
            name: bot.info.profile.name.clone(),
            uuid: bot.info.profile.uuid,
            entity: bot.info.login.entity_id,
            place: bot.location,
            look: bot.look,
            flying: bot.flying,
            hotbar: kit.0.clone(),
            slot: kit.1,
        }
    }

    /// The same player after `bot`, their next connection, which did nothing.
    fn after(&self, bot: &Bot) -> Self {
        Self {
            entity: bot.info.login.entity_id,
            ..self.clone()
        }
    }
}

/// Fails unless `bot`, which has just joined and done nothing, was put into the world
/// as the record says a player comes back: at the place, looking the same way, flying
/// if they flew, with the hotbar and the slot, as a later entity, and was not moved
/// again.
async fn is_back(bot: &mut Bot, left: &Left) {
    loaded(bot).await;
    let name = &left.name;
    let position = bot
        .position
        .clone()
        .expect("a bot that joined was sent one");
    assert!(
        near((position.x, position.y, position.z), left.place)
            && same_angle(position.yaw, left.look.0)
            && same_angle(position.pitch, left.look.1)
            && position.relative_flags == 0,
        "{name} was to enter at {:?} looking along {:?} and was put at {position:?}",
        left.place,
        left.look
    );
    assert_eq!(
        bot.stats.teleports_confirmed,
        1,
        "{name} was moved again after entering: {}",
        bot.report()
    );
    let told = bot.abilities.first().map(|packet| packet.is_flying());
    assert_eq!(
        (told, bot.flying),
        (Some(left.flying), left.flying),
        "{name} flew: {}; {}",
        left.flying,
        bot.report()
    );
    assert_eq!(bot.hotbar, left.hotbar, "the hotbar of {name}");
    assert_eq!(bot.selected_slot, left.slot, "the selected slot of {name}");
    // Which stay is the later is told by the stays' keys, which no client sees: the wire
    // ids an edge gives are its own (`docs/adr/0028-entities.md`, section 1).
}

/// Waits until `watcher` sees the player called `name` at `place`, as one entity, and
/// has them in its list, which is how a bot finds a player by name. Which wire id it
/// is shown them by is its edge's own, and says nothing of the stay
/// (`docs/adr/0028-entities.md`, section 1).
async fn sees(watcher: &mut Bot, name: &str, place: (f64, f64, f64)) {
    let seen = watcher
        .wait_until(PATIENCE, |bot| {
            let Some(listed) = bot.seen_player(name) else {
                return false;
            };
            let mut shown = bot
                .entities
                .values()
                .filter(|seen| seen.uuid == listed.uuid);
            shown.next().is_some_and(|seen| near(seen.position, place)) && shown.next().is_none()
        })
        .await;
    if let Err(error) = seen {
        panic!(
            "{error:#}: {} was to see {name} at {place:?}, once; it sees {:?} and lists {:?}",
            watcher.info.profile.name, watcher.entities, watcher.player_list
        );
    }
}

/// Fails unless `watcher` is shown one entity of the player with `uuid`: the entity of
/// a stay before is gone from its screen, as the later is there.
fn shown_once(watcher: &Bot, uuid: Uuid) {
    let shown = watcher
        .entities
        .values()
        .filter(|seen| seen.uuid == uuid)
        .count();
    assert_eq!(
        shown, 1,
        "{} is shown {shown} entities of one player: {:?}",
        watcher.info.profile.name, watcher.entities
    );
}

/// Waits until `watcher` sees no entity of the player with `uuid`.
async fn sees_no_more(watcher: &mut Bot, uuid: Uuid) {
    let gone = watcher
        .wait_until(PATIENCE, |bot| {
            bot.entities.values().all(|seen| seen.uuid != uuid)
        })
        .await;
    if let Err(error) = gone {
        panic!(
            "{error:#}: {} still sees an entity of {uuid}: {:?}",
            watcher.info.profile.name, watcher.entities
        );
    }
}

/// Fails unless someone who joins now and looks around sees no entity of the player
/// with `uuid`: a stay that arrived after its own leave would stand there with nobody
/// behind it (the record's section 4.3, after path 3).
async fn nobody_stands_there(address: &str, uuid: Uuid, place: (f64, f64, f64)) {
    let mut latecomer = join(address, "Latecomer").await;
    // Up to the place, so that it is in view wherever it is.
    latecomer
        .fly_to((place.0, GROUND, place.2 + 3.0), (0.0, 0.0), SPEED * 2.0)
        .await
        .unwrap();
    heard(&mut latecomer).await;
    let ghosts: Vec<_> = latecomer
        .entities
        .iter()
        .filter(|(_, seen)| seen.kind == entity_types::PLAYER && seen.uuid == uuid)
        .collect();
    assert!(
        ghosts.is_empty(),
        "a player who left still stands in the world: {ghosts:?}"
    );
}

/// The bot that entered, or a failure that says what the server did instead.
fn entered(entry: anyhow::Result<Entry>) -> Bot {
    match entry {
        Ok(Entry::Entered(bot)) => *bot,
        Ok(Entry::Refused { state, reason }) => {
            panic!("the login was to enter the world and was disconnected {state}: {reason}")
        }
        Err(error) => panic!("the login was to enter the world: {error:#}"),
    }
}

/// Logs in through `address` as the player `first` is, while `first` is connected and
/// its edge is there to end it. Fails unless `first` is ended with the sentence, and
/// returns the second connection.
async fn logs_in_over(first: &mut Bot, address: &str) -> Bot {
    let name = first.info.profile.name.clone();
    let (end, second) = tokio::join!(
        first.wait_for_end(PATIENCE),
        Bot::attempt(address, &name, Behaviour::default())
    );
    assert!(
        ended_with_the_sentence(&end),
        "the first connection of {name} was to be ended with the sentence and was ended \
         with {end:?}\n{}",
        first.report()
    );
    entered(second)
}

// ---------------------------------------------------------------------------------
// A login whose steps a test takes one at a time.
// ---------------------------------------------------------------------------------

/// A login that has gone through configuration up to its last word: the server has
/// said that it has finished, and the client has not yet acknowledged it. The player
/// "has finished configuration" (the record's section 4, step 1) with that
/// acknowledgement, which [`Prepared::enters`] sends.
struct Prepared {
    connection: Connection,
}

impl Prepared {
    async fn new(address: &str, name: &str) -> Self {
        let mut connection = Connection::connect(address).await.unwrap();
        let handshake = intention(address, Intent::Login).unwrap();
        connection.write(&handshake).await.unwrap();
        let start = LoginStart {
            name: name.to_owned(),
            uuid: Uuid::nil(),
        };
        connection.write(&start).await.unwrap();
        loop {
            let frame = connection.read_frame().await.unwrap();
            match ClientboundLogin::decode(&frame).unwrap() {
                ClientboundLogin::SetCompression(packet) => {
                    connection.enable_compression(packet.threshold).unwrap();
                }
                ClientboundLogin::LoginSuccess(_) => break,
                _ => panic!("the login of {name} was not let through"),
            }
        }
        connection.write(&LoginAcknowledged).await.unwrap();
        let brand = ServerboundPluginMessage {
            channel: "minecraft:brand".to_owned(),
            data: b"\x08botswarm".to_vec(),
        };
        connection.write(&brand).await.unwrap();
        let information = ClientInformation {
            locale: "en_us".to_owned(),
            view_distance: 8,
            chat_mode: 0,
            chat_colors: true,
            displayed_skin_parts: 0x7F,
            main_hand: 1,
            text_filtering: false,
            allow_server_listing: true,
            particle_status: 0,
        };
        connection.write(&information).await.unwrap();
        loop {
            let frame = connection.read_frame().await.unwrap();
            match ClientboundConfiguration::decode(&frame).unwrap() {
                ClientboundConfiguration::ClientboundKnownPacks(packet) => {
                    let core = |pack: &&clustine_protocol::packets::configuration::KnownPack| {
                        pack.namespace == "minecraft"
                            && pack.id == "core"
                            && pack.version == GAME_VERSION
                    };
                    let packs = packet.packs.iter().filter(core).cloned().collect();
                    let known = ServerboundKnownPacks { packs };
                    connection.write(&known).await.unwrap();
                }
                ClientboundConfiguration::ClientboundKeepAlive(packet) => {
                    let answer = ConfigurationKeepAlive { id: packet.id };
                    connection.write(&answer).await.unwrap();
                }
                ClientboundConfiguration::FinishConfiguration(_) => break,
                ClientboundConfiguration::Disconnect(_) => {
                    panic!("{name} was disconnected during configuration")
                }
                _ => {}
            }
        }
        Self { connection }
    }

    /// Says the last word of configuration: from here on the edge has a view of the
    /// player and a join on its way.
    async fn enters(mut self) -> Entering {
        // A connection that the server has closed meanwhile shows in what is read.
        let _ = self.connection.write(&AcknowledgeFinishConfiguration).await;
        Entering {
            connection: self.connection,
        }
    }
}

/// A connection whose join is on its way.
struct Entering {
    connection: Connection,
}

/// What became of a join.
enum Outcome {
    /// The server put the player into the world.
    Entered(Raw),
    /// The server sent a disconnect packet instead.
    PutOut(Reason),
    /// The connection ended without a word.
    Closed(String),
}

/// A connection in the play state that was sent where it is and does nothing.
struct Raw {
    connection: Connection,
    entity: i32,
    position: SynchronizePlayerPosition,
}

impl Raw {
    fn place(&self) -> (f64, f64, f64) {
        (self.position.x, self.position.y, self.position.z)
    }

    /// Stays connected until the connection ends, and says how it did.
    async fn end(&mut self) -> Ending {
        loop {
            let frame = match self.connection.read_frame().await {
                Ok(frame) => frame,
                Err(error) => return Ending::Closed(format!("{error:#}")),
            };
            match ClientboundPlay::decode(&frame).unwrap() {
                ClientboundPlay::Disconnect(_) => {
                    return Ending::Disconnected(Reason::from_frame(&frame).unwrap());
                }
                ClientboundPlay::ClientboundKeepAlive(packet) => {
                    let answer = ServerboundKeepAlive { id: packet.id };
                    let _ = self.connection.write(&answer).await;
                }
                _ => {}
            }
        }
    }
}

impl Entering {
    /// Reads until the server has placed the player or ended the connection.
    async fn outcome(mut self) -> Outcome {
        let mut entity = None;
        loop {
            let frame = match self.connection.read_frame().await {
                Ok(frame) => frame,
                Err(error) => return Outcome::Closed(format!("{error:#}")),
            };
            match ClientboundPlay::decode(&frame).unwrap() {
                ClientboundPlay::Login(login) => entity = Some(login.entity_id),
                ClientboundPlay::Disconnect(_) => {
                    return Outcome::PutOut(Reason::from_frame(&frame).unwrap());
                }
                ClientboundPlay::SynchronizePlayerPosition(position) => {
                    return Outcome::Entered(Raw {
                        connection: self.connection,
                        entity: entity.expect("the login packet comes before the position"),
                        position,
                    });
                }
                _ => {}
            }
        }
    }
}

/// Of two logins as one player that ran into each other, the one that stays. Fails
/// unless exactly one was put into the world and stays and the other was ended with
/// the sentence, before it entered or after.
async fn the_one_that_stays(outcomes: [Outcome; 2]) -> Raw {
    let mut entered = Vec::new();
    let mut put_out = 0;
    for outcome in outcomes {
        match outcome {
            Outcome::Entered(raw) => entered.push(raw),
            Outcome::PutOut(reason) => {
                assert!(
                    is_the_sentence(&reason),
                    "a login that lost was to be told the sentence and was told {reason}"
                );
                put_out += 1;
            }
            Outcome::Closed(how) => {
                panic!("a login that lost was to be told the sentence and was closed: {how}")
            }
        }
    }
    if entered.len() == 2 {
        // Both entered: the earlier one is ended now.
        let (one, other) = entered.split_at_mut(1);
        let (loser, ending) = tokio::select! {
            ending = one[0].end() => (0, ending),
            ending = other[0].end() => (1, ending),
        };
        assert!(
            ended_with_the_sentence(&Some(ending.clone())),
            "of two connections that entered, the one that ended was to be told the \
             sentence and ended with {ending}"
        );
        entered.remove(loser);
        put_out += 1;
    }
    assert_eq!(
        (entered.len(), put_out),
        (1, 1),
        "of two logins exactly one was to stay and one to be ended with the sentence"
    );
    entered.pop().expect("there is one")
}

// ---------------------------------------------------------------------------------
// 1. One process.
// ---------------------------------------------------------------------------------

/// "Leave and join: back in the same place, looking the same way, flying if they
/// flew, with the same hotbar and held slot." Far from the spawn point, on the
/// ground and in the air, twice in a row, the second time without anybody waiting
/// until the leave was seen.
#[tokio::test]
async fn a_player_who_leaves_and_joins_again_is_where_they_left_with_their_look_hotbar_and_slot() {
    let (server, address) = start().await;
    let mut watcher = join(&address, "Watcher").await;

    let mut roamer = join(&address, "Roamer").await;
    let kit = furnish(&mut roamer).await;
    goes_to(&mut roamer, FAR, A_LOOK).await;
    let left = Left::of(&roamer, &kit);
    sees(&mut watcher, "Roamer", FAR).await;
    drop(roamer);
    sees_no_more(&mut watcher, left.uuid).await;

    let mut again = join(&address, "Roamer").await;
    is_back(&mut again, &left).await;
    sees(&mut watcher, "Roamer", FAR).await;

    // On into the air, with another look, and out and in again at once.
    let aloft = (FAR.0 - 6.0, GROUND + 9.0, FAR.2 + 5.0);
    again.set_flying(true).await.unwrap();
    goes_to(&mut again, aloft, ANOTHER_LOOK).await;
    let left = Left::of(&again, &kit);
    assert!(left.flying);
    drop(again);
    let mut third = join(&address, "Roamer").await;
    is_back(&mut third, &left).await;
    sees(&mut watcher, "Roamer", aloft).await;

    // Whoever comes back can play: they are heard, and walk on from where they are.
    goes_to(&mut third, FAR, A_LOOK).await;
    sees(&mut watcher, "Roamer", FAR).await;
    server.stop().await;
}

/// "The spawn point is where a player is once in the life of a world": someone new
/// enters there, looking ahead, not flying, and so does the next new player, with
/// the same things in their hands.
#[tokio::test]
async fn a_new_player_enters_at_the_spawn_point_looking_ahead_and_not_flying() {
    let (server, address) = start().await;
    let mut kits = Vec::new();
    for name in ["Newcomer", "Another"] {
        let mut bot = join(&address, name).await;
        let position = bot.position.clone().unwrap();
        assert!(
            near((position.x, position.y, position.z), SPAWN_POINT)
                && same_angle(position.yaw, 0.0)
                && same_angle(position.pitch, 0.0)
                && position.relative_flags == 0,
            "{name} is new and was put at {position:?}"
        );
        assert_eq!(bot.stats.teleports_confirmed, 1);
        assert_eq!(
            (
                bot.abilities.first().map(|packet| packet.is_flying()),
                bot.flying
            ),
            (Some(false), false),
            "{}",
            bot.report()
        );
        heard(&mut bot).await;
        kits.push(kit_of(&bot));
    }
    assert_eq!(kits[0], kits[1], "two new players hold the same");
    server.stop().await;
}

/// "Whoever leaves below the world's lowest block (below y = -64) comes back at the
/// spawn point, with what they held": looking ahead and not flying, though they
/// looked elsewhere and flew. From there on the spawn point is their place like any
/// other.
#[tokio::test]
async fn a_player_who_leaves_below_the_world_comes_back_at_the_spawn_point_with_what_they_held() {
    let (server, address) = start().await;
    let mut faller = join(&address, "Faller").await;
    let kit = furnish(&mut faller).await;
    faller.set_flying(true).await.unwrap();
    goes_to(&mut faller, (20.5, GROUND, 9.5), A_LOOK).await;
    // Half a block below the lowest block.
    goes_to(&mut faller, (20.5, LOWEST - 0.5, 9.5), A_LOOK).await;
    let below = Left::of(&faller, &kit);
    assert!(below.flying);
    drop(faller);

    let at_the_spawn_point = Left {
        place: SPAWN_POINT,
        look: (0.0, 0.0),
        flying: false,
        ..below
    };
    let mut back = join(&address, "Faller").await;
    is_back(&mut back, &at_the_spawn_point).await;
    heard(&mut back).await;
    // They did nothing, and are there again the next time.
    let left = at_the_spawn_point.after(&back);
    drop(back);
    let mut again = join(&address, "Faller").await;
    is_back(&mut again, &left).await;
    server.stop().await;
}

/// "One who stands in a hole anywhere above that comes back where they were": the
/// rule is feet below the lowest block, `y < -64`, so feet at the height of the
/// lowest block itself are a place like any other, and so is a place high above the
/// highest block, of which the record makes no exception.
#[tokio::test]
async fn a_player_who_leaves_at_the_height_of_the_lowest_block_or_high_above_the_world_comes_back_there()
 {
    let (server, address) = start().await;
    let mut digger = join(&address, "Digger").await;
    let kit = furnish(&mut digger).await;
    digger.set_flying(true).await.unwrap();
    goes_to(&mut digger, (20.5, GROUND, 9.5), A_LOOK).await;
    goes_to(&mut digger, (20.5, LOWEST, 9.5), A_LOOK).await;
    let left = Left::of(&digger, &kit);
    drop(digger);
    let mut back = join(&address, "Digger").await;
    is_back(&mut back, &left).await;

    // And up, above the highest block of the world, which is at 319.
    back.fly_to((20.5, 400.0, 9.5), ANOTHER_LOOK, 8.0)
        .await
        .unwrap();
    heard(&mut back).await;
    let left = Left::of(&back, &kit);
    drop(back);
    let mut again = join(&address, "Digger").await;
    is_back(&mut again, &left).await;
    server.stop().await;
}

/// "Also after the server was stopped and started": for someone who had left before
/// the stop and for someone who was connected when it came, the one on the ground
/// and the other in the air. And after a second session and a second stop they are
/// where the second session left them, not the first (the record's rule P: with the
/// segments of a session numbered below the players' file's `from`, everybody would
/// be where they were two sessions ago).
#[tokio::test]
async fn players_are_where_they_left_after_the_server_was_stopped_and_started_twice() {
    let directory = tempfile::tempdir().unwrap();
    let on_disk = || Config {
        world: Some(directory.path().to_owned()),
        ..config()
    };
    let hover = (-30.5, GROUND + 7.0, 20.5);

    let (server, address) = start_with(on_disk()).await;
    let mut leaver = join(&address, "Leaver").await;
    let kit = furnish(&mut leaver).await;
    goes_to(&mut leaver, FAR, A_LOOK).await;
    let mut leaver_left = Left::of(&leaver, &kit);
    drop(leaver);
    let mut stayer = join(&address, "Stayer").await;
    let stayer_kit = kit_of(&stayer);
    stayer.set_flying(true).await.unwrap();
    goes_to(&mut stayer, hover, ANOTHER_LOOK).await;
    let mut stayer_left = Left::of(&stayer, &stayer_kit);
    server.stop().await;
    drop(stayer);

    // The second session: both are back, and both move on.
    let second_far = (FAR.0 - 20.0, GROUND, FAR.2 + 12.0);
    let second_hover = (hover.0 + 9.0, GROUND + 3.0, hover.2 - 4.0);
    let (server, address) = start_with(on_disk()).await;
    let mut leaver = join(&address, "Leaver").await;
    is_back(&mut leaver, &leaver_left).await;
    let mut stayer = join(&address, "Stayer").await;
    is_back(&mut stayer, &stayer_left).await;
    goes_to(&mut leaver, second_far, ANOTHER_LOOK).await;
    goes_to(&mut stayer, second_hover, A_LOOK).await;
    leaver_left = Left::of(&leaver, &kit);
    stayer_left = Left::of(&stayer, &stayer_kit);
    drop(leaver);
    server.stop().await;
    drop(stayer);

    // The third: where the second left them.
    let (server, address) = start_with(on_disk()).await;
    let mut leaver = join(&address, "Leaver").await;
    is_back(&mut leaver, &leaver_left).await;
    let mut stayer = join(&address, "Stayer").await;
    is_back(&mut stayer, &stayer_left).await;
    server.stop().await;
}

/// The place is in the commit of the tick that changed it, and what a tick produced
/// is published only when the store has confirmed its commit: so a server that is
/// killed outright, with a player in it, has the place the player was last answered
/// at.
#[tokio::test]
async fn a_player_is_where_they_were_last_answered_after_the_server_was_killed() {
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("world");
    let address = free_address().await;

    let mut server = spawn_server(&address, &world, &[]).await;
    let mut player = join(&address, "Survivor").await;
    let kit = furnish(&mut player).await;
    player.set_flying(true).await.unwrap();
    goes_to(&mut player, (FAR.0, GROUND + 5.0, FAR.2), A_LOOK).await;
    let left = Left::of(&player, &kit);
    server.kill().await.unwrap();
    drop(player);

    let mut server = spawn_server(&address, &world, &[]).await;
    let mut back = join(&address, "Survivor").await;
    is_back(&mut back, &left).await;
    server.kill().await.unwrap();
}

/// Rows 2 and 3 of the record's section 10, on one edge: "Join as the same name from
/// a second client: the first is put out with 'You logged in from another location',
/// the second stands where the first stood." A bystander never has two entities of
/// the player and still has the player in its list afterwards. (With
/// `CLUSTINE_TEST_PINS=0,4` the first stands in another region than the home region,
/// which is row 2; without, in the home region, which is row 3.)
#[tokio::test]
async fn rows_2_and_3_a_second_login_puts_the_first_out_with_the_sentence_and_stands_where_the_first_stood()
 {
    let (server, address) = start().await;
    let mut bystander = join(&address, "Bystander").await;
    let mut first = join(&address, "Twin").await;
    let kit = furnish(&mut first).await;
    first.set_flying(true).await.unwrap();
    let place = (FAR.0, GROUND + 4.0, FAR.2);
    goes_to(&mut first, place, A_LOOK).await;
    let left = Left::of(&first, &kit);
    sees(&mut bystander, "Twin", place).await;

    let mut second = logs_in_over(&mut first, &address).await;
    is_back(&mut second, &left).await;
    // The bystander is answered by the edge after everything the edge sent when it
    // ended the first connection: what it has then is what it is left with.
    heard(&mut bystander).await;
    sees(&mut bystander, "Twin", place).await;
    heard(&mut bystander).await;
    assert!(bystander.seen_player("Twin").is_some());
    shown_once(&bystander, left.uuid);

    // And a third time, over the second.
    let left = left.after(&second);
    let mut third = logs_in_over(&mut second, &address).await;
    is_back(&mut third, &left).await;
    sees(&mut bystander, "Twin", place).await;
    heard(&mut third).await;
    server.stop().await;
}

/// Rows 13 and 14 of the record's section 10, on one edge: a second login while the
/// first is still entering.
///
/// **How near a test can make it.** Both connections go through configuration up to
/// its last word, and then both say it, one right after the other: the two
/// acknowledgements are written within microseconds, on two sockets. A join takes
/// two commits to enter (the record's section 4), a tick or two, so the second
/// acknowledgement reaches the edge while the first join is on its way to the home
/// region or held there as entering. The test cannot see which, nor which of the two
/// the edge read first. Whichever: exactly one stays, where the player was, and the
/// other is told the sentence.
#[tokio::test]
async fn rows_13_and_14_a_second_login_while_the_first_is_still_entering_leaves_one_player_where_they_were()
 {
    let (server, address) = start().await;
    let mut watcher = join(&address, "Watcher").await;
    let mut seed = join(&address, "Hasty").await;
    let kit = furnish(&mut seed).await;
    goes_to(&mut seed, FAR, A_LOOK).await;
    let mut left = Left::of(&seed, &kit);
    drop(seed);
    sees_no_more(&mut watcher, left.uuid).await;

    for round in 0..6 {
        let one = Prepared::new(&address, "Hasty").await;
        let other = Prepared::new(&address, "Hasty").await;
        let (one, other) = (one.enters().await, other.enters().await);
        let outcomes = tokio::join!(one.outcome(), other.outcome());
        let stays = the_one_that_stays([outcomes.0, outcomes.1]).await;
        assert!(
            near(stays.place(), FAR),
            "round {round}: the one that stays is entity {} at {:?}",
            stays.entity,
            stays.position
        );
        sees(&mut watcher, "Hasty", FAR).await;
        left.entity = stays.entity;
        drop(stays);
        sees_no_more(&mut watcher, left.uuid).await;
    }

    let mut back = join(&address, "Hasty").await;
    is_back(&mut back, &left).await;
    sees(&mut watcher, "Hasty", FAR).await;
    drop(back);
    sees_no_more(&mut watcher, left.uuid).await;
    nobody_stands_there(&address, left.uuid, FAR).await;
    server.stop().await;
}

/// Row 16 of the record's section 10 and what is around it: leaving and joining
/// again many times quickly. A connection that is closed before it has said the last
/// word of configuration, one that is closed right after it (the player leaves while
/// entering), one that is closed when it has been placed, and one that plays a
/// moment: after all of them one player is where they were, as one entity, and when
/// they leave nobody is left standing.
#[tokio::test]
async fn row_16_leaving_and_joining_again_many_times_quickly_leaves_one_player_where_they_were() {
    let (server, address) = start().await;
    let mut watcher = join(&address, "Watcher").await;
    let mut seed = join(&address, "Restless").await;
    let kit = furnish(&mut seed).await;
    goes_to(&mut seed, FAR, A_LOOK).await;
    let mut left = Left::of(&seed, &kit);
    drop(seed);

    for round in 0..24 {
        match round % 4 {
            0 => drop(Prepared::new(&address, "Restless").await),
            1 => drop(Prepared::new(&address, "Restless").await.enters().await),
            2 => {
                let entering = Prepared::new(&address, "Restless").await.enters().await;
                let Outcome::Entered(raw) = entering.outcome().await else {
                    panic!("round {round}: a login after a leave was not put into the world");
                };
                assert!(
                    near(raw.place(), FAR),
                    "round {round}: entity {} at {:?}",
                    raw.entity,
                    raw.position
                );
                left.entity = raw.entity;
            }
            _ => {
                // The server may still have the connection before: a join over it is
                // let in all the same (the record's section 4, step 1).
                let mut bot = join(&address, "Restless").await;
                is_back(&mut bot, &left).await;
                left.entity = bot.info.login.entity_id;
            }
        }
    }

    let mut back = join(&address, "Restless").await;
    is_back(&mut back, &left).await;
    heard(&mut back).await;
    sees(&mut watcher, "Restless", FAR).await;
    drop(back);
    sees_no_more(&mut watcher, left.uuid).await;
    nobody_stands_there(&address, left.uuid, FAR).await;
    // The watcher saw every one of them come and go and never two at once, or it
    // would have failed; and it is still served.
    heard(&mut watcher).await;
    server.stop().await;
}

/// Rows 13 to 15 of the record's section 10, on one edge: two whole logins as one
/// name are started at the same moment, round after round. Exactly one stays, where
/// the player was, and can play; the other is told the sentence, before it entered
/// or after; and the world has one entity of the player.
#[tokio::test]
async fn rows_13_to_15_of_two_logins_racing_exactly_one_stays_and_the_world_has_one_entity_of_the_player()
 {
    let (server, address) = start().await;
    let mut watcher = join(&address, "Watcher").await;
    let mut seed = join(&address, "Racer").await;
    let kit = furnish(&mut seed).await;
    goes_to(&mut seed, FAR, A_LOOK).await;
    let mut left = Left::of(&seed, &kit);
    drop(seed);
    sees_no_more(&mut watcher, left.uuid).await;

    for round in 0..5 {
        let (one, other) = tokio::join!(
            Bot::attempt(&address, "Racer", Behaviour::default()),
            Bot::attempt(&address, "Racer", Behaviour::default())
        );
        let mut entered = Vec::new();
        let mut put_out = 0;
        for entry in [one, other] {
            match entry {
                Ok(Entry::Entered(bot)) => entered.push(*bot),
                Ok(Entry::Refused { reason, .. }) if is_the_sentence(&reason) => put_out += 1,
                Ok(Entry::Refused { state, reason }) => {
                    panic!("round {round}: a login was disconnected {state}: {reason}")
                }
                Err(error) => panic!("round {round}: a login failed without a word: {error:#}"),
            }
        }
        if entered.len() == 2 {
            let (a, b) = entered.split_at_mut(1);
            let (loser, ending) = tokio::select! {
                ending = a[0].wait_for_end(PATIENCE) => (0, ending),
                ending = b[0].wait_for_end(PATIENCE) => (1, ending),
            };
            assert!(
                ended_with_the_sentence(&ending),
                "round {round}: of two that entered, one was to be ended with the sentence: \
                 {ending:?}"
            );
            entered.remove(loser);
            put_out += 1;
        }
        assert_eq!((entered.len(), put_out), (1, 1), "round {round}");
        let mut stays = entered.pop().unwrap();
        is_back(&mut stays, &left).await;
        heard(&mut stays).await;
        sees(&mut watcher, "Racer", FAR).await;
        left = left.after(&stays);
        drop(stays);
        sees_no_more(&mut watcher, left.uuid).await;
    }
    nobody_stands_there(&address, left.uuid, FAR).await;
    server.stop().await;
}

/// Row 20 of the record's section 10: "A world is started with another division."
/// The records are kept, so everybody is where they were, and "the new home region
/// gets a block above `issued` if its own has no id above it": every stay of the
/// world's life has a higher entity than every stay before it, whichever region is
/// the home region.
#[tokio::test]
async fn row_20_players_are_where_they_left_when_the_world_is_started_with_other_pins() {
    let directory = tempfile::tempdir().unwrap();
    let divided_at = |pins: &[i32]| Config {
        world: Some(directory.path().to_owned()),
        pins: pins.to_vec(),
        follow: None,
        ..config()
    };
    // One region; then three, with the home region between two others, numbered
    // from the west so that the home region is another than before; then two; then
    // one again.
    let divisions: [&[i32]; 4] = [&[], &[-2, 2], &[4], &[]];
    let places = [
        FAR,
        (-50.5, GROUND + 3.0, 20.5),
        (90.5, GROUND, 6.5),
        (8.5, GROUND + 1.0, -3.5),
    ];
    let mut left: Option<Left> = None;
    for (session, (pins, place)) in divisions.into_iter().zip(places).enumerate() {
        let (server, address) = start_with(divided_at(pins)).await;
        let mut traveller = join(&address, "Traveller").await;
        let kit = match &left {
            Some(left) => {
                is_back(&mut traveller, left).await;
                (left.hotbar.clone(), left.slot)
            }
            None => furnish(&mut traveller).await,
        };
        // Someone new in every session beside the traveller. That each is a later stay
        // than any the world has had is in the keys, which no client sees
        // (`docs/adr/0028-entities.md`, section 1); the store's tests hold it.
        let fresh = join(&address, &format!("Fresh{session}")).await;
        traveller.set_flying(place.1 > GROUND).await.unwrap();
        goes_to(&mut traveller, place, A_LOOK).await;
        left = Some(Left::of(&traveller, &kit));
        drop((traveller, fresh));
        server.stop().await;
    }
}

// ---------------------------------------------------------------------------------
// 2. One process, a world of several regions.
// ---------------------------------------------------------------------------------

/// The regions of a world pinned at chunk x = 1: the home region in the west, which
/// has the spawn point, and the east, which begins at block x = 16.
const WEST: RegionId = RegionId(0);
const EAST: RegionId = RegionId(1);

/// A place in the east that someone at the spawn point sees.
const IN_THE_EAST: (f64, f64, f64) = (40.5, GROUND, 4.5);

/// A server whose world is two regions pinned side by side, which stay as they are.
fn divided() -> Config {
    Config {
        pins: vec![1],
        follow: None,
        ..config()
    }
}

/// The record's section 4, step 7, third case: the store names the region that holds
/// the place, and the home region "lets the stay go to X in this tick without
/// placing it". A player who left in the east enters there, plays there, and walks
/// back into the home region like anyone; someone in the east and someone in the
/// west see one entity throughout.
#[tokio::test]
async fn a_player_who_left_in_another_region_than_the_home_region_enters_there_again() {
    for serialise_link in [false, true] {
        let (server, address) = start_with(Config {
            serialise_link,
            ..divided()
        })
        .await;
        let mut westerner = join(&address, "Westerner").await;
        let mut easterner = join(&address, "Easterner").await;
        goes_to(&mut easterner, (44.5, GROUND, -6.5), (0.0, 0.0)).await;

        let mut settler = join(&address, "Settler").await;
        let kit = furnish(&mut settler).await;
        settler.set_flying(true).await.unwrap();
        let place = (IN_THE_EAST.0, GROUND + 2.0, IN_THE_EAST.2);
        goes_to(&mut settler, place, A_LOOK).await;
        let left = Left::of(&settler, &kit);
        for watcher in [&mut westerner, &mut easterner] {
            sees(watcher, "Settler", place).await;
        }
        drop(settler);
        for watcher in [&mut westerner, &mut easterner] {
            sees_no_more(watcher, left.uuid).await;
        }

        let mut back = join(&address, "Settler").await;
        is_back(&mut back, &left).await;
        for watcher in [&mut westerner, &mut easterner] {
            sees(watcher, "Settler", place).await;
        }
        // The east answers them, and the west does when they have walked there.
        heard(&mut back).await;
        let home = (6.5, GROUND + 2.0, 4.5);
        goes_to(&mut back, home, ANOTHER_LOOK).await;
        for watcher in [&mut westerner, &mut easterner] {
            sees(watcher, "Settler", home).await;
        }
        assert_eq!(back.stats.teleports_confirmed, 1);
        // And once more from the east, at once.
        goes_to(&mut back, place, A_LOOK).await;
        let left = Left::of(&back, &kit);
        drop(back);
        let mut third = join(&address, "Settler").await;
        is_back(&mut third, &left).await;
        server.stop().await;
    }
}

/// Row 2 of the record's section 10: the old stay is in another region than the home
/// region, which runs, and the second login comes through the same edge. The first
/// is told the sentence, the second stands where the first stood, in the east, and
/// plays there; nobody in either region sees two.
#[tokio::test]
async fn row_2_a_second_login_while_the_first_stands_in_another_region_stands_there_too() {
    let (server, address) = start_with(divided()).await;
    let mut westerner = join(&address, "Westerner").await;
    let mut easterner = join(&address, "Easterner").await;
    goes_to(&mut easterner, (44.5, GROUND, -6.5), (0.0, 0.0)).await;

    let mut first = join(&address, "Twin").await;
    let kit = furnish(&mut first).await;
    goes_to(&mut first, IN_THE_EAST, A_LOOK).await;
    let mut left = Left::of(&first, &kit);
    let mut current = first;
    for _ in 0..3 {
        let mut next = logs_in_over(&mut current, &address).await;
        is_back(&mut next, &left).await;
        heard(&mut next).await;
        for watcher in [&mut westerner, &mut easterner] {
            heard(watcher).await;
            sees(watcher, "Twin", IN_THE_EAST).await;
            shown_once(watcher, left.uuid);
        }
        left = left.after(&next);
        current = next;
    }
    drop(current);
    for watcher in [&mut westerner, &mut easterner] {
        sees_no_more(watcher, left.uuid).await;
    }
    nobody_stands_there(&address, left.uuid, IN_THE_EAST).await;
    server.stop().await;
}

/// Row 16 of the record's section 10 where the place is another region's: the player
/// leaves while entering, again and again, so that the home region's letting go
/// ("`Departed` ... the edge sends `Discard`") finds no view. Afterwards they enter
/// in the east as one entity, and when they have left nobody stands there.
#[tokio::test]
async fn row_16_a_player_who_leaves_while_entering_another_region_is_not_left_standing_there() {
    let (server, address) = start_with(divided()).await;
    let mut easterner = join(&address, "Easterner").await;
    goes_to(&mut easterner, (44.5, GROUND, -6.5), (0.0, 0.0)).await;
    let mut seed = join(&address, "Fickle").await;
    let kit = furnish(&mut seed).await;
    goes_to(&mut seed, IN_THE_EAST, A_LOOK).await;
    let mut left = Left::of(&seed, &kit);
    drop(seed);

    for _ in 0..12 {
        drop(Prepared::new(&address, "Fickle").await.enters().await);
    }
    let mut back = join(&address, "Fickle").await;
    is_back(&mut back, &left).await;
    heard(&mut back).await;
    sees(&mut easterner, "Fickle", IN_THE_EAST).await;
    left = left.after(&back);
    drop(back);
    sees_no_more(&mut easterner, left.uuid).await;
    nobody_stands_there(&address, left.uuid, IN_THE_EAST).await;
    heard(&mut easterner).await;
    server.stop().await;
}

/// Rows 4 and 18 of the record's section 10, in one process: the region of the
/// player's place, and then the home region, is taken over while the player logs
/// in. "A login never waits for ... a region that is ... being taken over" but for
/// the home region and the store, and "the home region is restored or taken over
/// while N is entering: ... answers the naming of N with `Enter` again". Each time
/// the player is where they were, once.
#[tokio::test]
async fn rows_4_and_18_a_player_enters_where_they_were_while_a_region_is_taken_over() {
    let (mut server, address) = start_with(divided()).await;
    let mut westerner = join(&address, "Westerner").await;
    let mut seed = join(&address, "Punctual").await;
    let kit = furnish(&mut seed).await;
    goes_to(&mut seed, IN_THE_EAST, A_LOOK).await;
    let mut left = Left::of(&seed, &kit);
    drop(seed);
    sees_no_more(&mut westerner, left.uuid).await;

    for round in 0..6 {
        let region = if round % 2 == 0 { EAST } else { WEST };
        let entering = Prepared::new(&address, "Punctual").await;
        // The last word of configuration and the takeover at the same moment.
        let (taken, outcome) = tokio::join!(server.take_over(region), async {
            entering.enters().await.outcome().await
        });
        taken.unwrap();
        let Outcome::Entered(raw) = outcome else {
            panic!("round {round}: the login during a takeover of {region:?} did not enter");
        };
        assert!(
            near(raw.place(), IN_THE_EAST),
            "round {round}: entity {} at {:?}",
            raw.entity,
            raw.position
        );
        sees(&mut westerner, "Punctual", IN_THE_EAST).await;
        left.entity = raw.entity;
        drop(raw);
        sees_no_more(&mut westerner, left.uuid).await;
    }
    let mut back = join(&address, "Punctual").await;
    is_back(&mut back, &left).await;
    heard(&mut back).await;
    server.stop().await;
}

// ---------------------------------------------------------------------------------
// One process whose regions follow their players: merges and splits by themselves,
// and what the record has measured.
// ---------------------------------------------------------------------------------

/// A place so far out that someone alone there is split off into a region of their
/// own: 30 chunks from the spawn point, with a split distance of 20.
const FAR_OUT: (f64, f64, f64) = (480.5, GROUND, 0.5);
const FAR_OUT_CHUNK: (i32, i32) = (30, 0);

/// Blocks a tick on the way out there: what the tests of such worlds run at.
const OUT: f64 = 3.0;

/// A server whose regions follow their players by the distances of the tests' view
/// distance, as a server that is told nothing does: merged at 12 chunks, split at
/// 20, with the rest it is told.
fn following(rest: Duration, world: &TempDir) -> Config {
    let policy = Policy::for_view_distance(VIEW_DISTANCE as u32);
    Config {
        pins: Vec::new(),
        follow: Some(Policy { rest, ..policy }),
        world: Some(world.path().join("world")),
        ..config()
    }
}

/// A directory for the world of a measurement, and where it is, for what is printed:
/// in the directory `CLUSTINE_STAYS_WORLDS` names, for a measurement on a real disk,
/// or where the tests' temporary files are, which `TMPDIR` puts into memory.
fn a_world() -> (TempDir, String) {
    let directory = match std::env::var_os("CLUSTINE_STAYS_WORLDS") {
        Some(disk) => tempfile::tempdir_in(disk).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    let place = directory.path().display().to_string();
    (directory, place)
}

/// The rest of a server that is told nothing.
const REST: Duration = Duration::from_secs(10);

fn has_chunk(list: &RegionList, region: RegionId, chunk: (i32, i32)) -> bool {
    let info = list.regions.iter().find(|info| info.region == region);
    info.and_then(|info| info.bounds).is_some_and(|bounds| {
        bounds.min.x <= chunk.0
            && chunk.0 <= bounds.max.x
            && bounds.min.z <= chunk.1
            && chunk.1 <= bounds.max.z
    })
}

/// The overworld's region other than the home region whose land has `chunk`, if one
/// has.
fn holder_away_from_home(list: &RegionList, chunk: (i32, i32)) -> Option<RegionId> {
    let mut others = overworld(list).into_iter().map(|info| info.region);
    others.find(|region| *region != list.home && has_chunk(list, *region, chunk))
}

/// The list in a line.
fn in_short(list: &RegionList) -> String {
    let regions: Vec<String> = list
        .regions
        .iter()
        .map(|info| match info.bounds {
            Some(bounds) => format!(
                "{} ({}..={}, {}..={})",
                info.region.0, bounds.min.x, bounds.max.x, bounds.min.z, bounds.max.z
            ),
            None => format!("{} (no land)", info.region.0),
        })
        .collect();
    format!(
        "regions {regions:?}, home {}, next {}, absorbed {:?}",
        list.home.0,
        list.next.0,
        list.absorbed
            .iter()
            .map(|(gone, into)| (gone.0, into.0))
            .collect::<Vec<_>>()
    )
}

/// Waits until the store's list of regions is as `state` wants it, while the bots
/// stay connected, and returns it. `state` is given the overworld's regions alone:
/// the homes of the Nether and of the End, which nothing here goes to, are to stay
/// as the world made them.
async fn until_the_regions(
    server: &Server,
    bots: &mut [&mut Bot],
    what: &str,
    state: impl Fn(&RegionList) -> bool,
) -> RegionList {
    let waiting = Instant::now();
    loop {
        let list = server.regions().unwrap();
        if let Err(error) = other_homes_are_untouched(&list) {
            panic!("{error}");
        }
        if state(&overworld_list(&list)) {
            return list;
        }
        assert!(
            waiting.elapsed() < LONG_PATIENCE,
            "the regions did not come to be so: {what}; they are {}",
            in_short(&list)
        );
        for bot in bots.iter_mut() {
            bot.idle(LOOK).await.unwrap();
        }
        if bots.is_empty() {
            tokio::time::sleep(LOOK).await;
        }
    }
}

/// Has `bot` send a pulse every other client tick until `stop` is set and one more
/// after that, and returns the bot with how long each pulse waited for its
/// acknowledgement: for as long as a pulse waits, its player stands still.
fn pulsing(mut bot: Bot, stop: Arc<AtomicBool>) -> JoinHandle<(Bot, Vec<Duration>)> {
    tokio::spawn(async move {
        let mut waits = Vec::new();
        loop {
            let last = stop.load(Ordering::Relaxed);
            let (x, _, z) = bot.location;
            let sent = Instant::now();
            let sequence = bot
                .dig(x.floor() as i32, GROUND as i32 + 64, z.floor() as i32)
                .await
                .unwrap();
            let name = bot.info.profile.name.clone();
            bot.wait_until(LONG_PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
                .await
                .unwrap_or_else(|error| panic!("{name} was not answered: {error:#}"));
            waits.push(sent.elapsed());
            if last {
                return (bot, waits);
            }
            bot.idle(Duration::from_millis(100)).await.unwrap();
        }
    })
}

/// The pulses in a line: how many, the one in the middle, the worst, and how many
/// waited for more than a quarter of a second, which a player notices.
fn in_numbers(waits: &[Duration]) -> String {
    let mut sorted = waits.to_vec();
    sorted.sort_unstable();
    let middle = sorted.get(sorted.len() / 2).copied().unwrap_or_default();
    let worst = sorted.last().copied().unwrap_or_default();
    let noticed = sorted
        .iter()
        .filter(|wait| **wait > Duration::from_millis(250))
        .count();
    format!(
        "{} pulses, {:.3} s in the middle, {:.3} s at worst, {noticed} above a quarter of a \
         second",
        sorted.len(),
        middle.as_secs_f64(),
        worst.as_secs_f64()
    )
}

fn worst(waits: &[Duration]) -> Duration {
    waits.iter().copied().max().unwrap_or_default()
}

/// Has a player join, walk out to `place` alone and stand there until they have a
/// region of their own, and returns them.
async fn walks_out_alone(server: &Server, address: &str, name: &str) -> (Bot, Kit) {
    let mut bot = join(address, name).await;
    let kit = furnish(&mut bot).await;
    bot.fly_to(FAR_OUT, A_LOOK, OUT).await.unwrap();
    until_the_regions(
        server,
        &mut [&mut bot],
        "a region for whoever walked out",
        |list| list.regions.len() == 2 && holder_away_from_home(list, FAR_OUT_CHUNK).is_some(),
    )
    .await;
    heard(&mut bot).await;
    (bot, kit)
}

/// What is measured (R1.7), second row, and "a player who leaves, the regions are
/// merged, and joins again": **a player alone leaves far from home and joins again
/// after more than half a minute**. Their region has been absorbed by then, so
/// nobody holds the place; the home region places them (the record's section 4,
/// step 7, second case), claims what they see and is split at once.
///
/// Asserted: the player is in place; and "nobody at the spawn point stands still
/// longer than ADR-0017 allows for a split (half a second in the middle, a second at
/// worst)", of which one split gives the worst. Measured and printed: how long the
/// joining player stands still for the split that follows, and how long somebody at
/// the spawn point does.
#[tokio::test(flavor = "multi_thread")]
async fn measured_a_player_alone_far_out_comes_back_after_their_region_was_absorbed_and_is_split_off_again()
 {
    let _alone = turn_alone().await;
    let (world, worlds) = a_world();
    let (server, address) = start_with(following(REST, &world)).await;
    let (bot, kit) = walks_out_alone(&server, &address, "Hermit").await;
    let left = Left::of(&bot, &kit);
    drop(bot);
    let leaving = Instant::now();
    let absorbed = until_the_regions(&server, &mut [], "the hermit's region absorbed", |list| {
        list.regions.len() == 1
    })
    .await;
    println!(
        "the region of a player who left alone far out was absorbed {:.1} s after they left: {}",
        leaving.elapsed().as_secs_f64(),
        in_short(&absorbed)
    );

    let stop = Arc::new(AtomicBool::new(false));
    let resident = join(&address, "Resident").await;
    let resident = pulsing(resident, stop.clone());
    let joining = Instant::now();
    let mut back = Bot::join(&address, "Hermit").await.unwrap();
    let placed = joining.elapsed();
    is_back(&mut back, &left).await;
    let in_view = joining.elapsed();
    let back = pulsing(back, stop.clone());
    let split = until_the_regions(&server, &mut [], "the hermit split off again", |list| {
        list.regions.len() == 2 && holder_away_from_home(list, FAR_OUT_CHUNK).is_some()
    })
    .await;
    let split_after = joining.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (mut back, hermit_waits) = back.await.unwrap();
    let (mut resident, resident_waits) = resident.await.unwrap();
    println!(
        "MEASURED (a player alone, back after their region was absorbed; worlds in {}):\n  \
         placed {:.3} s after connecting, the chunk they stand in {:.3} s after connecting, \
         split off {:.3} s after connecting: {}\n  the joining player: {}\n  somebody at the \
         spawn point: {}",
        worlds,
        placed.as_secs_f64(),
        in_view.as_secs_f64(),
        split_after.as_secs_f64(),
        in_short(&split),
        in_numbers(&hermit_waits),
        in_numbers(&resident_waits)
    );
    // Both are served by the regions they are in now.
    heard(&mut back).await;
    heard(&mut resident).await;
    assert_eq!(back.stats.teleports_confirmed, 1);
    assert!(
        worst(&resident_waits) <= Duration::from_secs(1),
        "somebody at the spawn point stood still for {:?} when a player who came back far \
         out was split off: ADR-0017 allows a second at worst",
        worst(&resident_waits)
    );
    server.stop().await;
}

/// What is measured (R1.7), third row: **the same within half a minute**. "The
/// region is still there or has just given its land back; whichever of cases 2 and 3
/// it is, the player is in place." Printed: which it was, and how long entering took.
#[tokio::test(flavor = "multi_thread")]
async fn measured_a_player_alone_far_out_who_comes_back_within_half_a_minute_is_in_place() {
    let _alone = turn_alone().await;
    let (world, worlds) = a_world();
    let (server, address) = start_with(following(REST, &world)).await;
    let (bot, kit) = walks_out_alone(&server, &address, "Hermit").await;
    let mut left = Left::of(&bot, &kit);
    drop(bot);
    // At once, and again a few seconds later, each well within half a minute.
    for round in 0..3 {
        let before = server.regions().unwrap();
        let joining = Instant::now();
        let mut back = Bot::join(&address, "Hermit").await.unwrap();
        let placed = joining.elapsed();
        is_back(&mut back, &left).await;
        let in_view = joining.elapsed();
        heard(&mut back).await;
        let answered = joining.elapsed();
        println!(
            "MEASURED (a player alone, back within half a minute, round {round}; worlds in \
             {}): placed {:.3} s after connecting, the chunk they stand in after {:.3} s, \
             answered after {:.3} s; the place was {} when they connected: {}",
            worlds,
            placed.as_secs_f64(),
            in_view.as_secs_f64(),
            answered.as_secs_f64(),
            match holder_away_from_home(&before, FAR_OUT_CHUNK) {
                Some(region) => format!("held by region {} (case 3)", region.0),
                None => "nobody's land (case 2)".to_owned(),
            },
            in_short(&before)
        );
        left = left.after(&back);
        // A region for them again, so that the next round begins as the first did.
        until_the_regions(
            &server,
            &mut [&mut back],
            "a region for the hermit",
            |list| list.regions.len() == 2 && holder_away_from_home(list, FAR_OUT_CHUNK).is_some(),
        )
        .await;
        heard(&mut back).await;
        drop(back);
        if round == 1 {
            // Long enough for the region to notice that nobody is in it, and far too
            // short for it to be absorbed: a rest, of the three.
            tokio::time::sleep(REST / 2).await;
        }
    }
    server.stop().await;
}

/// What is measured (R1.7), first row: **join inside a friend's land. Asserted: no
/// merge, no split, no chunk granted to the home region.** A friend lives far out in
/// a region of their own; the player left beside them and joins again. They are in
/// place, and the list of regions is what it was: the same regions, no absorbed pair
/// more, the same next id, and the home region's land no larger. The friend, who
/// sends a pulse all the while, is printed.
#[tokio::test(flavor = "multi_thread")]
async fn measured_a_player_who_joins_inside_a_friends_land_causes_no_merge_no_split_and_no_grant_to_the_home_region()
 {
    let _alone = turn_alone().await;
    let (world, worlds) = a_world();
    let (server, address) = start_with(following(REST, &world)).await;
    let (mut friend, _) = walks_out_alone(&server, &address, "Friend").await;
    let beside = (FAR_OUT.0 - 5.0, GROUND, FAR_OUT.2 + 6.0);
    let mut guest = join(&address, "Guest").await;
    let kit = furnish(&mut guest).await;
    guest.fly_to(beside, A_LOOK, OUT).await.unwrap();
    // Whatever the guest's walk made of the regions is over when the guest is
    // answered beside the friend, has left, and the world is the home region and
    // the friend's again.
    heard(&mut guest).await;
    sees(&mut friend, "Guest", beside).await;
    let left = Left::of(&guest, &kit);
    drop(guest);
    sees_no_more(&mut friend, left.uuid).await;
    let settled = |list: &RegionList| {
        list.regions.len() == 2
            && holder_away_from_home(list, FAR_OUT_CHUNK).is_some()
            && !has_chunk(list, list.home, FAR_OUT_CHUNK)
    };
    until_the_regions(
        &server,
        &mut [&mut friend],
        "home and the friend's",
        settled,
    )
    .await;
    heard(&mut friend).await;
    // The home region has nobody: it gives back what the guest's walk left it with.
    // Until it has, a grant to it could hide in the land it still has.
    let before = until_the_regions(
        &server,
        &mut [&mut friend],
        "the home region with the home chunk and no more",
        |list| {
            let home = list.regions.iter().find(|info| info.region == list.home);
            let small = home.and_then(|info| info.bounds).is_none_or(|bounds| {
                bounds.max.x - bounds.min.x <= 1 && bounds.max.z - bounds.min.z <= 1
            });
            settled(list) && small
        },
    )
    .await;

    let stop = Arc::new(AtomicBool::new(false));
    let friend = pulsing(friend, stop.clone());
    let mut back = join(&address, "Guest").await;
    is_back(&mut back, &left).await;
    for _ in 0..20 {
        heard(&mut back).await;
        back.idle(Duration::from_millis(50)).await.unwrap();
    }
    let after = server.regions().unwrap();
    stop.store(true, Ordering::Relaxed);
    let (mut friend, friend_waits) = friend.await.unwrap();
    println!(
        "MEASURED (join inside a friend's land; worlds in {}):\n  before: {}\n  after: {}\n  \
         the friend: {}",
        worlds,
        in_short(&before),
        in_short(&after),
        in_numbers(&friend_waits)
    );
    sees(&mut friend, "Guest", beside).await;
    let ids = |list: &RegionList| {
        list.regions
            .iter()
            .map(|info| info.region)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&after), ids(&before), "a region more or fewer");
    assert_eq!(after.absorbed, before.absorbed, "a merge");
    assert_eq!(after.next, before.next, "a split");
    let home = |list: &RegionList| {
        let home = list.regions.iter().find(|info| info.region == list.home);
        home.and_then(|info| info.bounds)
    };
    assert_eq!(
        home(&after),
        home(&before),
        "the home region's land changed: a chunk was granted to it"
    );
    server.stop().await;
}

/// Notes every change of the store's list of regions until it has not changed for
/// `quiet`, or until `at_most` has passed: what a measurement watches, and asserts
/// nothing by.
async fn watched(server: &Server, quiet: Duration, at_most: Duration) -> Vec<(Duration, String)> {
    let watching = Instant::now();
    let mut changes = Vec::new();
    let mut last = (Instant::now(), String::new());
    loop {
        let list = in_short(&server.regions().unwrap());
        if list != last.1 {
            changes.push((watching.elapsed(), list.clone()));
            last = (Instant::now(), list);
        }
        if last.0.elapsed() > quiet || watching.elapsed() > at_most {
            return changes;
        }
        tokio::time::sleep(LOOK).await;
    }
}

fn in_lines(changes: &[(Duration, String)]) -> String {
    let lines = changes
        .iter()
        .map(|(when, list)| format!("    {:7.3} s: {list}", when.as_secs_f64()));
    lines.collect::<Vec<_>>().join("\n")
}

/// What is measured (R1.7), fourth row: **join 10 to 22 chunks from a friend**,
/// outside their land and inside the merge distance, which with the tests' view
/// distance of 3 is 5 to 14 chunks: here 8. The plan has it that "the friend's
/// region is merged into the home region and both are split off again"; what the
/// regions do and how often and how long the friend stands still is printed. Only
/// that the player is in place is asserted.
#[tokio::test(flavor = "multi_thread")]
async fn measured_a_player_who_joins_near_a_friends_land_and_how_long_the_friend_stands_still() {
    let _alone = turn_alone().await;
    let (world, worlds) = a_world();
    let (server, address) = start_with(following(REST, &world)).await;
    let (mut friend, _) = walks_out_alone(&server, &address, "Friend").await;
    // Eight chunks east of the friend, whose land reaches four.
    let near_by = (FAR_OUT.0 + 8.0 * 16.0, GROUND, FAR_OUT.2);
    let near_chunk = (FAR_OUT_CHUNK.0 + 8, FAR_OUT_CHUNK.1);
    let mut guest = join(&address, "Guest").await;
    let kit = furnish(&mut guest).await;
    // Around the friend at a distance, so that the walk is not through their feet.
    guest
        .fly_to((FAR_OUT.0, GROUND, 40.5), A_LOOK, OUT)
        .await
        .unwrap();
    guest.fly_to(near_by, A_LOOK, OUT).await.unwrap();
    heard(&mut guest).await;
    let left = Left::of(&guest, &kit);
    drop(guest);
    let before = until_the_regions(
        &server,
        &mut [&mut friend],
        "the home region and the friend's, with the guest's place nobody's",
        |list| {
            list.regions.len() == 2
                && holder_away_from_home(list, FAR_OUT_CHUNK).is_some()
                && holder_away_from_home(list, near_chunk).is_none()
                && !has_chunk(list, list.home, near_chunk)
                && !has_chunk(list, list.home, FAR_OUT_CHUNK)
        },
    )
    .await;
    heard(&mut friend).await;

    let stop = Arc::new(AtomicBool::new(false));
    let friend = pulsing(friend, stop.clone());
    let joining = Instant::now();
    let mut back = Bot::join(&address, "Guest").await.unwrap();
    let placed = joining.elapsed();
    is_back(&mut back, &left).await;
    let in_view = joining.elapsed();
    let back = pulsing(back, stop.clone());
    let changes = watched(&server, REST + REST / 2, Duration::from_secs(120)).await;
    stop.store(true, Ordering::Relaxed);
    let (mut back, guest_waits) = back.await.unwrap();
    let (mut friend, friend_waits) = friend.await.unwrap();
    println!(
        "MEASURED (join 8 chunks from a friend, whose land reaches 4 and who is merged with \
         at 12; worlds in {}):\n  before: {}\n  placed {:.3} s after connecting, the chunk \
         they stand in after {:.3} s\n  the regions from then on:\n{}\n  the friend: {}\n  \
         the joining player: {}",
        worlds,
        in_short(&before),
        placed.as_secs_f64(),
        in_view.as_secs_f64(),
        in_lines(&changes),
        in_numbers(&friend_waits),
        in_numbers(&guest_waits)
    );
    heard(&mut back).await;
    heard(&mut friend).await;
    assert_eq!(back.stats.teleports_confirmed, 1);
    server.stop().await;
}

/// What is measured (R1.7), fifth row: **twenty join far apart at once** (after an
/// edge died, say). Twenty players who each left alone, 21 chunks or more from every
/// other and from the spawn point, join in the same moment. Asserted: each is in
/// place. Printed: how often and how long somebody at the spawn point stands still,
/// "against ADR-0017's half a second in the middle and a second at worst", and what
/// the regions do.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement that takes minutes; run it by name with --ignored --nocapture"]
async fn measured_twenty_players_join_far_apart_at_once_and_how_long_the_spawn_point_stands_still()
{
    const PLAYERS: usize = 20;
    let _alone = turn_alone().await;
    let (world, worlds) = a_world();
    let (server, address) = start_with(following(REST, &world)).await;
    // On a ring of 70 chunks around the spawn point, 22 chunks from one to the next.
    let place = |number: usize| {
        let angle = number as f64 / PLAYERS as f64 * std::f64::consts::TAU;
        let (x, z) = (1120.0 * angle.cos(), 1120.0 * angle.sin());
        (x.floor() + 0.5, GROUND, z.floor() + 0.5)
    };
    let walking = (0..PLAYERS).map(|number| {
        let address = address.clone();
        tokio::spawn(async move {
            let mut bot = join(&address, &format!("Far{number}")).await;
            let kit = furnish(&mut bot).await;
            bot.fly_to(place(number), A_LOOK, 8.0).await.unwrap();
            heard(&mut bot).await;
            Left::of(&bot, &kit)
        })
    });
    let mut left = Vec::new();
    for walk in walking.collect::<Vec<_>>() {
        left.push(walk.await.unwrap());
    }
    let leaving = Instant::now();
    let alone = until_the_regions(&server, &mut [], "every region absorbed", |list| {
        list.regions.len() == 1
    })
    .await;
    println!(
        "twenty regions without players were absorbed {:.1} s after the players left: {}",
        leaving.elapsed().as_secs_f64(),
        in_short(&alone)
    );

    let stop = Arc::new(AtomicBool::new(false));
    let resident = join(&address, "Resident").await;
    let resident = pulsing(resident, stop.clone());
    let joining = Instant::now();
    let joins = left.into_iter().map(|left| {
        let (address, stop) = (address.clone(), stop.clone());
        tokio::spawn(async move {
            let mut back = Bot::join(&address, &left.name).await.unwrap();
            is_back(&mut back, &left).await;
            let in_view = joining.elapsed();
            let (_, waits) = pulsing(back, stop).await.unwrap();
            (in_view, waits)
        })
    });
    let joins: Vec<_> = joins.collect();
    let changes = watched(&server, REST * 3, Duration::from_secs(600)).await;
    stop.store(true, Ordering::Relaxed);
    let mut in_view = Vec::new();
    let mut waits = Vec::new();
    for join in joins {
        let (seen, waited) = join.await.unwrap();
        in_view.push(seen);
        waits.extend(waited);
    }
    let (mut resident, resident_waits) = resident.await.unwrap();
    println!(
        "MEASURED (twenty join far apart at once; worlds in {}):\n  the chunk they stand in: \
         {:.3} s after connecting for the first, {:.3} s for the last\n  the regions from \
         then on ({} changes):\n{}\n  somebody at the spawn point: {}\n  the twenty: {}",
        worlds,
        in_view.iter().min().unwrap().as_secs_f64(),
        in_view.iter().max().unwrap().as_secs_f64(),
        changes.len(),
        in_lines(&changes),
        in_numbers(&resident_waits),
        in_numbers(&waits)
    );
    heard(&mut resident).await;
    server.stop().await;
}

// ---------------------------------------------------------------------------------
// 3. A cluster of processes, with two edges.
// ---------------------------------------------------------------------------------

/// The chunk x coordinate at which the cluster's world is divided: block x = 48.
/// Region 0 in the west is the home region, region 1 the east.
const LINE: &str = "3";
const HOME: u32 = 0;
const OTHER: u32 = 1;

/// Places a friend at the spawn point sees: one in the east and one at home.
const EAST_OF_THE_LINE: (f64, f64, f64) = (56.5, GROUND, 4.5);
const AT_HOME: (f64, f64, f64) = (20.5, GROUND, -9.5);

/// The last block column of the home region on the way east, and the first of the
/// east: a step from the one to the other is a hand-over.
const BEFORE_THE_LINE: (f64, f64, f64) = (47.5, GROUND, 4.5);
const BEYOND_THE_LINE: (f64, f64, f64) = (48.5, GROUND, 4.5);

/// A cluster of a coordinator, a world store, two workers and **two edges**, with a
/// friend at the spawn point on the first edge and an onlooker there on the second.
/// The second edge is a `clustine edge` with a name of its own, started by the
/// test: the test cluster's helper starts one, and its `spawn` starts any process.
struct Stage {
    cluster: Cluster,
    /// Where each edge listens.
    first: String,
    second: String,
    second_edge: Option<Child>,
    friend: Bot,
    onlooker: Bot,
    directory: Option<TempDir>,
    _turn: Turn,
}

impl Drop for Stage {
    /// The logs of a test that fails are kept, and the failure says where.
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.directory.take()
        {
            eprintln!("the logs are kept in {}", directory.keep().display());
        }
    }
}

async fn signal(process: Option<u32>, signal: &str) {
    let pid = process.expect("the process is running");
    let sent = Command::new("kill")
        .args([format!("-{signal}"), pid.to_string()])
        .status()
        .await;
    assert!(sent.unwrap().success());
}

impl Stage {
    async fn new() -> Self {
        let turn = turn().await;
        let directory = tempfile::tempdir().unwrap();
        let mut cluster = Cluster::new(directory.path(), 2, LINE).await;
        cluster.start().await;
        let first = cluster.edge.0.clone();
        let second = free_address().await;
        let view = cluster.view_distance.to_string();
        let second_edge = cluster.spawn(
            "edge-2",
            &[
                "edge",
                "--name",
                "edge-2",
                "--coordinator",
                &cluster.coordinator.0,
                "--bind",
                &second,
                "--view-distance",
                &view,
            ],
        );
        let waiting = Instant::now();
        while clustine_botswarm::ping(&second).await.is_err() {
            assert!(
                waiting.elapsed() < PATIENCE,
                "the second edge did not come up:\n{}",
                cluster.log("edge-2")
            );
            tokio::time::sleep(LOOK).await;
        }
        let mut stage = Self {
            cluster,
            friend: join(&first, "Friend").await,
            onlooker: join(&second, "Onlooker").await,
            first,
            second,
            second_edge: Some(second_edge),
            directory: Some(directory),
            _turn: turn,
        };
        // A worker for each region, so that one can be stopped without the other.
        stage
            .until("each region runs on a worker of its own", |cluster| {
                cluster.runs(HOME)
                    && cluster.runs(OTHER)
                    && cluster.owner(HOME) != cluster.owner(OTHER)
            })
            .await;
        stage
    }

    /// Waits until the cluster is as `state` wants it, while the friend and the
    /// onlooker stay connected.
    async fn until(&mut self, what: &str, state: impl Fn(&Cluster) -> bool) {
        let waiting = Instant::now();
        while !state(&self.cluster) {
            assert!(
                waiting.elapsed() < LONG_PATIENCE,
                "the cluster did not come to be so: {what}\n{}",
                self.cluster.all_logs()
            );
            self.tend().await;
        }
    }

    /// Lets the friend and the onlooker hear what they are sent, if their edges are
    /// there.
    async fn tend(&mut self) {
        let (friend, onlooker) = tokio::join!(self.friend.idle(LOOK), self.onlooker.idle(LOOK));
        if self.cluster.edge.1.is_some() {
            friend.unwrap();
        }
        if self.second_edge.is_some() {
            onlooker.unwrap();
        }
    }

    /// Has the twin join through the first edge, take things into their hands and
    /// go to `place`, where the friend and the onlooker see them.
    async fn twin_at(&mut self, place: (f64, f64, f64)) -> (Bot, Left) {
        let mut twin = join(&self.first, "Twin").await;
        let kit = furnish(&mut twin).await;
        goes_to(&mut twin, place, A_LOOK).await;
        let left = Left::of(&twin, &kit);
        sees(&mut self.friend, "Twin", place).await;
        sees(&mut self.onlooker, "Twin", place).await;
        (twin, left)
    }

    /// The worker that runs `region`.
    fn worker_of(&self, region: u32) -> usize {
        self.cluster
            .owner(region)
            .unwrap_or_else(|| panic!("region {region} has no owner"))
    }

    fn pid_of(&self, worker: usize) -> Option<u32> {
        self.cluster.workers[worker].1.as_ref().and_then(Child::id)
    }

    /// How often the worker that runs the home region has said that it let a player
    /// go to another region.
    fn let_go(&self) -> usize {
        let home = self.worker_of(HOME);
        self.cluster.said(home, "player departed to another region")
    }

    /// Fails unless everybody who watches, on whichever edge is there, comes to see
    /// the twin, whose UUID is `twin`, once at `place`, with the twin in their list,
    /// and no longer as the entity of the stay before. The first connection has been
    /// ended by now, so what the friend is answered next comes after everything its
    /// edge sent when it removed the first connection.
    async fn one_twin(&mut self, place: (f64, f64, f64), twin: Uuid) {
        let mut watchers = Vec::new();
        if self.cluster.edge.1.is_some() {
            watchers.push(&mut self.friend);
        }
        if self.second_edge.is_some() {
            watchers.push(&mut self.onlooker);
        }
        for watcher in watchers {
            heard(watcher).await;
            sees(watcher, "Twin", place).await;
            heard(watcher).await;
            assert!(
                watcher.seen_player("Twin").is_some(),
                "{} no longer has the twin in the list, or no longer sees them: lists {:?}, \
                 sees {:?}",
                watcher.info.profile.name,
                watcher.player_list,
                watcher.entities
            );
            shown_once(watcher, twin);
        }
    }

    /// Ends the test: every process is killed.
    async fn end(mut self) {
        if let Some(mut edge) = self.second_edge.take() {
            edge.kill().await.unwrap();
        }
        self.cluster.kill().await;
    }
}

/// Row 1 of the record's section 10, and the case its step R1.7 names: *a friend on
/// the first edge has the player in the list after a second login through another
/// edge*. The first stay is in another region than the home region, which runs; the
/// second login comes through the other edge. The first connection reads the
/// sentence, the second enters at the place with what the first held, and the friend
/// on the first connection's edge sees one twin, the later one, and "keeps the
/// player in the player list through a second login from elsewhere". Then the same
/// back through the first edge, and once more.
#[tokio::test(flavor = "multi_thread")]
async fn row_1_a_second_login_through_another_edge_puts_the_first_out_and_a_friend_on_the_first_edge_has_the_player_in_the_list()
 {
    let mut stage = Stage::new().await;
    let (mut current, mut left) = stage.twin_at(EAST_OF_THE_LINE).await;
    for round in 0..3 {
        let through = if round % 2 == 0 {
            stage.second.clone()
        } else {
            stage.first.clone()
        };
        let mut next = logs_in_over(&mut current, &through).await;
        is_back(&mut next, &left).await;
        heard(&mut next).await;
        stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
        left = left.after(&next);
        current = next;
    }
    // The one that stays walks home and is seen there by both.
    goes_to(&mut current, AT_HOME, ANOTHER_LOOK).await;
    sees(&mut stage.friend, "Twin", AT_HOME).await;
    sees(&mut stage.onlooker, "Twin", AT_HOME).await;
    stage.end().await;
}

/// Row 3 of the record's section 10 with two edges: the old stay is in the home
/// region, where "the join itself removes D and tells A". As row 1 for everybody.
#[tokio::test(flavor = "multi_thread")]
async fn row_3_a_second_login_through_another_edge_while_the_first_stands_in_the_home_region() {
    let mut stage = Stage::new().await;
    let (mut first, left) = stage.twin_at(AT_HOME).await;
    let through = stage.second.clone();
    let mut second = logs_in_over(&mut first, &through).await;
    is_back(&mut second, &left).await;
    heard(&mut second).await;
    stage.one_twin(AT_HOME, left.uuid).await;
    stage.end().await;
}

/// Row 4 of the record's section 10: the old stay is in a region that has no
/// worker. "Nobody waits": the second login, through the other edge, is put into
/// the world at the place while the region still has nobody to run it ("P is in the
/// world without chunks until X runs"). When the region runs, on the worker that is
/// left, the second connection has its chunks, the first is "told when X runs", and
/// everybody sees one twin.
#[tokio::test(flavor = "multi_thread")]
async fn row_4_a_second_login_while_the_region_of_the_first_has_no_worker_enters_at_once_and_is_served_when_it_runs()
 {
    let mut stage = Stage::new().await;
    let (mut first, left) = stage.twin_at(EAST_OF_THE_LINE).await;
    let worker = stage.worker_of(OTHER);
    stage.cluster.kill_worker(worker).await;

    let placing = Instant::now();
    let entry = Bot::attempt(&stage.second, "Twin", Behaviour::default()).await;
    let mut second = entered(entry);
    let placed = placing.elapsed();
    let position = second.position.clone().unwrap();
    assert!(
        near((position.x, position.y, position.z), left.place),
        "{position:?}"
    );
    // Nothing runs the east yet: the home region and the store answered alone.
    assert!(
        !stage.cluster.runs(OTHER),
        "the east ran again before the second login was placed, {placed:?} after it \
         connected: the test cannot tell whether the login waited for it"
    );
    println!("row 4: placed {placed:?} after connecting, with no worker for the region");

    let end = first.wait_for_end(LONG_PATIENCE).await;
    assert!(
        ended_with_the_sentence(&end),
        "the first connection was to be told the sentence when its region ran again: \
         {end:?}\n{}",
        stage.cluster.all_logs()
    );
    is_back(&mut second, &left).await;
    heard(&mut second).await;
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    stage.end().await;
}

/// Row 5 of the record's section 10: the old stay is in a region that hangs (its
/// worker is stopped where it is, and carries on a moment later). The second login,
/// through the other edge, is put into the world at the place while the region
/// hangs. When it wakes, or has been taken over, things are as in row 4.
#[tokio::test(flavor = "multi_thread")]
async fn row_5_a_second_login_while_the_region_of_the_first_hangs_enters_at_once_and_is_served_when_it_wakes()
 {
    let mut stage = Stage::new().await;
    let (mut first, left) = stage.twin_at(EAST_OF_THE_LINE).await;
    let worker = stage.worker_of(OTHER);
    signal(stage.pid_of(worker), "STOP").await;

    let entry = Bot::attempt(&stage.second, "Twin", Behaviour::default()).await;
    let mut second = entered(entry);
    let position = second.position.clone().unwrap();
    assert!(
        near((position.x, position.y, position.z), left.place),
        "{position:?}"
    );
    signal(stage.pid_of(worker), "CONT").await;

    let end = first.wait_for_end(LONG_PATIENCE).await;
    assert!(
        ended_with_the_sentence(&end),
        "the first connection was to be told the sentence when its region woke: {end:?}\n{}",
        stage.cluster.all_logs()
    );
    is_back(&mut second, &left).await;
    heard(&mut second).await;
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    stage.end().await;
}

/// Rows 8 and 9 of the record's section 10: the old stay is in the middle of a
/// hand-over. The east hangs; the first connection steps across the line, the home
/// region lets it go (its worker says so) and the edge keeps the arrival for the
/// east. The second login, through the other edge: "the place is the one X's
/// letting go wrote", the first block column beyond the line. When the east wakes,
/// the old stay's arrival lands before or after the new one's; either way the first
/// connection is told, and everybody sees one twin, there.
#[tokio::test(flavor = "multi_thread")]
async fn rows_8_and_9_a_second_login_while_the_first_is_in_the_middle_of_a_hand_over_stands_where_it_was_let_go()
 {
    let mut stage = Stage::new().await;
    let (mut first, before_the_line) = stage.twin_at(BEFORE_THE_LINE).await;
    let worker = stage.worker_of(OTHER);
    let let_go = stage.let_go();
    signal(stage.pid_of(worker), "STOP").await;
    first
        .fly_to(BEYOND_THE_LINE, ANOTHER_LOOK, SPEED)
        .await
        .unwrap();
    stage
        .until("the home region let the twin go", |cluster| {
            let home = cluster.owner(HOME).expect("the home region has an owner");
            cluster.said(home, "player departed to another region") > let_go
        })
        .await;
    let left = Left {
        place: first.location,
        look: first.look,
        ..before_the_line
    };

    let entry = Bot::attempt(&stage.second, "Twin", Behaviour::default()).await;
    let mut second = entered(entry);
    let position = second.position.clone().unwrap();
    assert!(
        near((position.x, position.y, position.z), BEYOND_THE_LINE),
        "the place is the one the letting go wrote, {BEYOND_THE_LINE:?}: {position:?}"
    );
    signal(stage.pid_of(worker), "CONT").await;

    let end = first.wait_for_end(LONG_PATIENCE).await;
    assert!(
        ended_with_the_sentence(&end),
        "the first connection was to be told the sentence when its arrival landed: \
         {end:?}\n{}",
        stage.cluster.all_logs()
    );
    is_back(&mut second, &left).await;
    heard(&mut second).await;
    stage.one_twin(BEYOND_THE_LINE, left.uuid).await;
    stage.end().await;
}

/// Row 10 of the record's section 10: the first connection is alive and its client
/// walks on while the second login, through the other edge, is made. "What D did is
/// applied, as before any kick, and published", and the second stands "at the place
/// of the first one's last commit that the store had taken when the login arrived":
/// somewhere on the first one's way, behind where its client got to, looking as it
/// looked, and not where it started unless the login was that quick.
#[tokio::test(flavor = "multi_thread")]
async fn row_10_a_second_login_while_the_first_walks_on_stands_on_the_way_the_first_walked() {
    let mut stage = Stage::new().await;
    let (mut first, left) = stage.twin_at(EAST_OF_THE_LINE).await;
    let goal = (EAST_OF_THE_LINE.0 + 14.0, GROUND, EAST_OF_THE_LINE.2);
    let through = stage.second.clone();
    let (walked, entry) = tokio::join!(
        // A fifth of a block a tick: three and a half seconds of walking.
        first.fly_to(goal, A_LOOK, 0.2),
        Bot::attempt(&through, "Twin", Behaviour::default())
    );
    let mut second = entered(entry);
    // The walk ended when the connection did, or the connection ends now.
    let end = match walked {
        Ok(()) => first.wait_for_end(PATIENCE).await,
        Err(_) => first.ended.clone(),
    };
    assert!(ended_with_the_sentence(&end), "{end:?}\n{}", first.report());
    let position = second.position.clone().unwrap();
    let on_the_way = EAST_OF_THE_LINE.0 - 0.01 <= position.x
        && position.x <= first.location.0 + 0.01
        && near(
            (EAST_OF_THE_LINE.0, position.y, position.z),
            EAST_OF_THE_LINE,
        );
    assert!(
        on_the_way,
        "the second stands at {position:?}; the first walked from {EAST_OF_THE_LINE:?} and \
         got to {:?}",
        first.location
    );
    println!(
        "row 10: the first walked to x = {:.2} before it was ended; the second stands at x \
         = {:.2}",
        first.location.0, position.x
    );
    let place = (position.x, position.y, position.z);
    is_back(
        &mut second,
        &Left {
            place,
            ..left.clone()
        },
    )
    .await;
    heard(&mut second).await;
    stage.one_twin(place, left.uuid).await;
    stage.end().await;
}

/// Row 11 of the record's section 10: the first connection's edge is frozen and
/// wakes. While it is frozen the second login, through the other edge, enters at
/// the place and the onlooker on that edge sees one twin, the later one. When the
/// first edge wakes it "reads `Ended` first": its client is told the sentence, and
/// not that the server lost track of it; and the friend on that edge sees the later
/// twin and has them in the list.
#[tokio::test(flavor = "multi_thread")]
async fn row_11_a_second_login_while_the_first_edge_is_frozen_and_the_first_is_told_the_sentence_when_it_wakes()
 {
    let mut stage = Stage::new().await;
    let (mut first, left) = stage.twin_at(EAST_OF_THE_LINE).await;
    let edge = stage.cluster.edge.1.as_ref().and_then(Child::id);
    signal(edge, "STOP").await;

    let entry = Bot::attempt(&stage.second, "Twin", Behaviour::default()).await;
    let mut second = entered(entry);
    is_back(&mut second, &left).await;
    heard(&mut second).await;
    heard(&mut stage.onlooker).await;
    sees(&mut stage.onlooker, "Twin", EAST_OF_THE_LINE).await;
    shown_once(&stage.onlooker, left.uuid);

    signal(edge, "CONT").await;
    let end = first.wait_for_end(LONG_PATIENCE).await;
    assert!(
        ended_with_the_sentence(&end),
        "the first connection was to be told the sentence when its edge woke: {end:?}\n{}",
        stage.cluster.all_logs()
    );
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    heard(&mut second).await;
    stage.end().await;
}

/// Row 12 of the record's section 10: the first connection's edge is dead for good.
/// "D is removed by `Dead` all the same": the second login, through the other edge,
/// enters at the place, and the onlooker on that edge is left with one twin, the
/// later one. When the twin has left, nobody stands there.
#[tokio::test(flavor = "multi_thread")]
async fn row_12_a_second_login_after_the_first_edge_died_enters_at_the_place_and_the_old_entity_goes()
 {
    let mut stage = Stage::new().await;
    let (first, left) = stage.twin_at(EAST_OF_THE_LINE).await;
    let mut edge = stage.cluster.edge.1.take().expect("the first edge runs");
    edge.kill().await.unwrap();
    drop(first);

    let entry = Bot::attempt(&stage.second, "Twin", Behaviour::default()).await;
    let mut second = entered(entry);
    is_back(&mut second, &left).await;
    heard(&mut second).await;
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    drop(second);
    sees_no_more(&mut stage.onlooker, left.uuid).await;
    let through = stage.second.clone();
    nobody_stands_there(&through, left.uuid, EAST_OF_THE_LINE).await;
    stage.end().await;
}

/// Rows 13 and 14 of the record's section 10 with two edges: "two logins within a
/// tick, through B and then C" or a few ticks apart. Both connections, one on each
/// edge, are through configuration but for its last word, and say it one right
/// after the other (how near that is, is said at the test of the same rows on one
/// edge). The home region takes the joins in the order they reach it: exactly one
/// stays, at the place, and the other's edge is told and tells its client the
/// sentence. Everybody sees one twin.
#[tokio::test(flavor = "multi_thread")]
async fn rows_13_and_14_of_two_logins_through_two_edges_at_once_exactly_one_stays() {
    let mut stage = Stage::new().await;
    let (seed, mut left) = stage.twin_at(EAST_OF_THE_LINE).await;
    drop(seed);
    sees_no_more(&mut stage.friend, left.uuid).await;
    sees_no_more(&mut stage.onlooker, left.uuid).await;

    for round in 0..4 {
        let on_the_first = Prepared::new(&stage.first, "Twin").await;
        let on_the_second = Prepared::new(&stage.second, "Twin").await;
        let (one, other) = if round % 2 == 0 {
            let one = on_the_first.enters().await;
            (one, on_the_second.enters().await)
        } else {
            let one = on_the_second.enters().await;
            (one, on_the_first.enters().await)
        };
        let outcomes = tokio::join!(one.outcome(), other.outcome());
        let stays = the_one_that_stays([outcomes.0, outcomes.1]).await;
        assert!(
            near(stays.place(), EAST_OF_THE_LINE),
            "round {round}: the one that stays is entity {} at {:?}",
            stays.entity,
            stays.position
        );
        stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
        left.entity = stays.entity;
        drop(stays);
        sees_no_more(&mut stage.friend, left.uuid).await;
        sees_no_more(&mut stage.onlooker, left.uuid).await;
    }
    let mut back = join(&stage.first, "Twin").await;
    is_back(&mut back, &left).await;
    stage.end().await;
}

/// Row 17 of the record's section 10: the player leaves in the middle of a
/// hand-over. The east hangs, the twin steps across the line, the home region lets
/// them go, and the twin's connection is closed. "The record has the place of the
/// letting go": when the east has woken they join again on the first block column
/// beyond the line, and after they have left nobody stands there.
#[tokio::test(flavor = "multi_thread")]
async fn row_17_a_player_who_leaves_in_the_middle_of_a_hand_over_comes_back_where_they_were_let_go()
{
    let mut stage = Stage::new().await;
    let (mut twin, before_the_line) = stage.twin_at(BEFORE_THE_LINE).await;
    let worker = stage.worker_of(OTHER);
    let let_go = stage.let_go();
    signal(stage.pid_of(worker), "STOP").await;
    twin.fly_to(BEYOND_THE_LINE, ANOTHER_LOOK, SPEED)
        .await
        .unwrap();
    stage
        .until("the home region let the twin go", |cluster| {
            let home = cluster.owner(HOME).expect("the home region has an owner");
            cluster.said(home, "player departed to another region") > let_go
        })
        .await;
    let left = Left {
        place: twin.location,
        look: twin.look,
        ..before_the_line
    };
    drop(twin);
    signal(stage.pid_of(worker), "CONT").await;

    let through = stage.first.clone();
    let mut back = join(&through, "Twin").await;
    is_back(&mut back, &left).await;
    heard(&mut back).await;
    stage.one_twin(BEYOND_THE_LINE, left.uuid).await;
    drop(back);
    sees_no_more(&mut stage.friend, left.uuid).await;
    sees_no_more(&mut stage.onlooker, left.uuid).await;
    nobody_stands_there(&through, left.uuid, BEYOND_THE_LINE).await;
    stage.end().await;
}

/// Row 18 of the record's section 10, and "a worker killed while a player is
/// entering": the home region's worker is killed in the moment the player says the
/// last word of configuration. The home region is taken over by the worker that is
/// left; "its first tick names N", or the edge's kept join is sent again; either
/// way the player enters where they were, once, and the friend, who stood in the
/// home region through it, sees them.
#[tokio::test(flavor = "multi_thread")]
async fn row_18_a_player_enters_where_they_were_when_the_home_regions_worker_is_killed_while_they_enter()
 {
    let mut stage = Stage::new().await;
    let (seed, mut left) = stage.twin_at(EAST_OF_THE_LINE).await;
    drop(seed);
    sees_no_more(&mut stage.friend, left.uuid).await;

    for round in 0..2 {
        let worker = stage.worker_of(HOME);
        let entering = Prepared::new(&stage.first, "Twin").await;
        let entering = entering.enters().await;
        stage.cluster.kill_worker(worker).await;
        let Outcome::Entered(raw) = entering.outcome().await else {
            panic!(
                "round {round}: the login did not enter when the home region was taken \
                 over\n{}",
                stage.cluster.all_logs()
            );
        };
        assert!(
            near(raw.place(), EAST_OF_THE_LINE),
            "round {round}: entity {} at {:?}",
            raw.entity,
            raw.position
        );
        stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
        left.entity = raw.entity;
        drop(raw);
        sees_no_more(&mut stage.friend, left.uuid).await;
        // The worker comes back for the next round, and for the last join.
        stage.cluster.start_worker_again(worker);
        stage
            .until("both regions run", |cluster| {
                cluster.runs(HOME) && cluster.runs(OTHER)
            })
            .await;
    }
    let mut back = join(&stage.first, "Twin").await;
    is_back(&mut back, &left).await;
    heard(&mut back).await;
    stage.end().await;
}

/// Row 19 of the record's section 10, and "the store restarted between a leave and
/// a join": "records and floors are on disk with the commits that made them". The
/// twin leaves in the east, the store is killed and started, and the twin joins
/// through the other edge: at the place, with what they held, as a later entity
/// than before. Then the store is killed and started under them, and a second login
/// stands where they stand.
#[tokio::test(flavor = "multi_thread")]
async fn row_19_a_player_is_where_they_left_when_the_store_was_killed_and_started_between_the_leave_and_the_join()
 {
    let mut stage = Stage::new().await;
    let (twin, mut left) = stage.twin_at(EAST_OF_THE_LINE).await;
    drop(twin);
    sees_no_more(&mut stage.friend, left.uuid).await;

    for round in 0..2 {
        let (home, other) = (stage.worker_of(HOME), stage.worker_of(OTHER));
        let ran = |cluster: &Cluster, worker: usize| cluster.said(worker, "running a region");
        let before = (ran(&stage.cluster, home), ran(&stage.cluster, other));
        stage.cluster.kill_and_start_the_store().await;
        // Until the regions have been opened again nobody is let in; whoever tries
        // tries again, as the test of the store's restart does.
        let waiting = Instant::now();
        let mut back = loop {
            let restored =
                ran(&stage.cluster, home) > before.0 && ran(&stage.cluster, other) > before.1;
            if restored && let Ok(bot) = Bot::join(&stage.second, "Twin").await {
                break bot;
            }
            assert!(
                waiting.elapsed() < LONG_PATIENCE,
                "round {round}: nobody could join after the store was started again\n{}",
                stage.cluster.all_logs()
            );
            tokio::time::sleep(LOOK * 5).await;
        };
        is_back(&mut back, &left).await;
        heard(&mut back).await;
        left = left.after(&back);
        if round == 0 {
            drop(back);
        } else {
            // A second login over the one that came after the restart.
            let through = stage.first.clone();
            let mut second = logs_in_over(&mut back, &through).await;
            is_back(&mut second, &left).await;
        }
    }
    stage.end().await;
}

/// Asks the coordinator for something until it is not turned away for a move of its
/// own, which is over in a moment, and returns what it printed.
async fn asked(stage: &mut Stage, what: &str, command: impl Fn(&Cluster) -> Command) -> String {
    let waiting = Instant::now();
    loop {
        let asking = ask(what.to_owned(), command(&stage.cluster));
        while !asking.is_finished() {
            stage.tend().await;
        }
        let asked = asking.await.unwrap();
        if asked.code == Some(0) {
            return asked.said;
        }
        assert!(
            asked.was_told_no_because("is being released") && waiting.elapsed() < LONG_PATIENCE,
            "{what}: {}\n{}",
            asked.outcome(),
            stage.cluster.all_logs()
        );
        tokio::time::sleep(LOOK * 5).await;
    }
}

/// "A player who leaves, the regions are merged or split, and joins again", on a
/// cluster whose regions are merged and split by hand (the record's section 9). The
/// twin leaves in the east; the home region absorbs the east, so that the place is
/// the home region's own (step 7, second case): the twin joins in place. They leave
/// again beside a companion; the companion's chunk is split off into a new region,
/// so that the place is that region's (third case): the twin joins in place. Then
/// the new region is absorbed again under the twin, who stays, and comes back there
/// once more.
#[tokio::test(flavor = "multi_thread")]
async fn a_player_who_leaves_is_where_they_left_after_their_region_was_absorbed_and_after_it_was_split_off()
 {
    let mut stage = Stage::new().await;
    let (twin, mut left) = stage.twin_at(EAST_OF_THE_LINE).await;
    drop(twin);
    sees_no_more(&mut stage.friend, left.uuid).await;

    let said = asked(
        &mut stage,
        "the home region to absorb the east",
        |cluster| cluster.merge_command(HOME, OTHER),
    )
    .await;
    assert!(said.contains("region 0 has absorbed region 1"), "{said}");
    let through = stage.second.clone();
    let mut back = join(&through, "Twin").await;
    is_back(&mut back, &left).await;
    heard(&mut back).await;
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    left = left.after(&back);

    // A companion in the twin's chunk, who is whom the split takes.
    let mut companion = join(&stage.first, "Companion").await;
    goes_to(&mut companion, (58.5, GROUND, 9.5), (0.0, 0.0)).await;
    drop(back);
    sees_no_more(&mut companion, left.uuid).await;
    let said = asked(
        &mut stage,
        "the companion's chunk to be split off",
        |cluster| cluster.split_command(HOME, &[(3, 0)]),
    )
    .await;
    assert!(said.contains("has been split off region 0"), "{said}");
    heard(&mut companion).await;
    let mut back = join(&through, "Twin").await;
    is_back(&mut back, &left).await;
    heard(&mut back).await;
    sees(&mut companion, "Twin", EAST_OF_THE_LINE).await;
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    left = left.after(&back);

    // The part goes back into the home region under both, and the twin once more.
    let part = stage.cluster.regions().await.unwrap();
    let part = part.regions.iter().map(|info| info.region.0).max().unwrap();
    let merging = ask(
        "the home region to absorb the part".to_owned(),
        stage.cluster.merge_command(HOME, part),
    );
    while !merging.is_finished() {
        stage.tend().await;
        back.idle(LOOK).await.unwrap();
        companion.idle(LOOK).await.unwrap();
    }
    println!("the part absorbed: {}", merging.await.unwrap().outcome());
    heard(&mut back).await;
    drop(back);
    sees_no_more(&mut companion, left.uuid).await;
    let mut again = join(&stage.first, "Twin").await;
    is_back(&mut again, &left).await;
    heard(&mut again).await;
    sees(&mut companion, "Twin", EAST_OF_THE_LINE).await;
    stage.end().await;
}

/// Rows 6 and 7 of the record's section 10, as near as a test can come: the region
/// of the old stay is absorbed, and then a part with the old stay is split off, in
/// the very moment of a second login through the other edge. The command and the
/// login are started together; whether the floor was raised before the region's
/// tick read its `Dead`, or the new edge had a link to the part, the test cannot
/// see or arrange. Whichever order it was: the first connection is told the
/// sentence, the second stands where the first stood, and everybody sees one twin.
#[tokio::test(flavor = "multi_thread")]
async fn rows_6_and_7_a_second_login_in_the_moment_the_region_of_the_first_is_absorbed_or_split_off()
 {
    let mut stage = Stage::new().await;
    let (mut first, mut left) = stage.twin_at(EAST_OF_THE_LINE).await;

    // Row 7: absorbed.
    let merging = ask(
        "the home region to absorb the east".to_owned(),
        stage.cluster.merge_command(HOME, OTHER),
    );
    let through = stage.second.clone();
    let mut second = logs_in_over(&mut first, &through).await;
    is_back(&mut second, &left).await;
    while !merging.is_finished() {
        stage.tend().await;
        second.idle(LOOK).await.unwrap();
    }
    let merged = merging.await.unwrap();
    println!("row 7: {}", merged.outcome());
    heard(&mut second).await;
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    left = left.after(&second);

    // Row 6: split off a moment ago. Whatever the merge came to, the twin's chunk is
    // split off the region that has it now.
    let list = stage.cluster.regions().await.unwrap();
    let region = if overworld(&list).len() == 1 {
        HOME
    } else {
        OTHER
    };
    // Somebody who stays, so that the region is not all in the part: the friend at
    // the spawn point for the home region, and for the east a settler further out.
    let mut settler = join(&stage.first, "Settler").await;
    goes_to(&mut settler, (88.5, GROUND, 4.5), (0.0, 0.0)).await;
    let splitting = ask(
        "the twin's chunk to be split off".to_owned(),
        stage.cluster.split_command(region, &[(3, 0)]),
    );
    let through = stage.first.clone();
    let mut third = logs_in_over(&mut second, &through).await;
    is_back(&mut third, &left).await;
    while !splitting.is_finished() {
        stage.tend().await;
        third.idle(LOOK).await.unwrap();
        settler.idle(LOOK).await.unwrap();
    }
    println!("row 6: {}", splitting.await.unwrap().outcome());
    heard(&mut third).await;
    stage.one_twin(EAST_OF_THE_LINE, left.uuid).await;
    stage.end().await;
}

// ---------------------------------------------------------------------------------
// What of the record's section 10 no test here reaches, and why.
//
// - Row 7's own order (the region absorbed or split between the floor being raised
//   and its tick reading the `Dead`) and row 6's (the new edge without a link to the
//   part) are a tick or two wide and inside the processes; the test above starts
//   the command and the login together and cannot say which order it met.
// - Row 8a (the old stay's arrival lands in the home region while the new one is
//   entering there) needs an arrival and a join to meet within the two commits of
//   one login, from two edges. The tests of rows 8 and 9 keep the arrival for a
//   region that hangs, which is not the home region: a home region that hangs takes
//   no join either.
// - Row 15 is row 1 by the record's own word; the tests of two logins racing meet
//   it whenever the first has entered before the second's join is taken.
// - Rows 21 and 22 need the link between an edge and the home region to be lost
//   between the placing of a player and the edge's reading of `Spawned`, with a
//   split or a claim answered "another's" in that same moment. A test has no hand
//   on a link but to stop or kill a process, which loses more than the link; the
//   edge's own tests against scripted regions hold these paths (the record's "Found
//   by the tests written from this record (step R1.4)").
// ---------------------------------------------------------------------------------
