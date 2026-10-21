//! End-to-end tests of step N5's riders crossing a line with their vehicles, written from
//! the specification (`docs/groundwork/n5-plan.md`, sections 1, 4, 7 and 8;
//! `docs/adr/0028-entities.md`, section 5 and its amendment of step N5;
//! `docs/adr/0020-one-stay-per-player.md`) by someone who did not write the code.
//!
//! A bot places a boat west of the pinned line at block x = 0, gets in and drives it
//! east across the line by `move_vehicle`, as a client does. What the specification asks:
//! the rider "goes on riding, with their vehicle and anyone else in it, without a jump
//! back, a flicker or a dismount" (plan, section 1): a watcher is shown the boat once, by
//! one wire id, and never shown it without its rider (section 7, item 2: "a rider was
//! shown dismounted at every crossing" was the defect mended); the rider's own client is
//! sent no dismount, no teleport and no correction of its moves (section 4, item 3: the
//! new region "applies the kept `MoveVehicle` as the old one would have, with no
//! correction"). A rider who leaves alone takes the boat out of the world with them and
//! comes back on it, as the game does (`docs/groundwork/n7h-logout-on-a-mount.md`, which
//! narrows N5's section 7, item 1 and section 8, item 6). With each process killed at
//! each moment of a crossing, the player and the boat are each in exactly one place
//! afterwards, the player riding it or standing, and no phantom seat is left (section 7,
//! items 5 and 6; section 8, items 1, 8 and 9).
//!
//! The tests divide their worlds themselves at chunk x = 0 and 4 (the world of
//! `CLUSTINE_TEST_PINS=0,4`), so that the line at block x = 0 is beside the spawn
//! whatever the environment says. Those that start clusters of processes run only in the
//! run without `CLUSTINE_TEST_PINS`, as the tests of chaos do; the others run in both.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, SeenEntity};
use clustine_data::{entity_types, items};
use clustine_protocol::packets::play::{face, player_input};
use tokio::time::Instant;
use uuid::Uuid;

use common::processes::{Cluster, Turn, turn};
use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// How long a cluster may take to be whole again after a kill: a takeover takes the
/// lease, three seconds in these tests, and a moment.
const RECOVERY: Duration = Duration::from_secs(90);

/// One tick of a client.
const TICK: Duration = Duration::from_millis(50);

/// The pinned line beside the spawn: block x = 0, between chunk x = -1 and chunk x = 0.
const LINE: f64 = 0.0;

/// Yaw in degrees looking east (+x).
const EAST: f32 = -90.0;

/// How far the bot drives its boat each tick: well within what the move check of the
/// game takes from a client, on flat ground.
const STEP: f64 = 0.1;

/// The block under the feet that a round trip breaks: bedrock, which nothing breaks.
const BEDROCK: i32 = -64;

/// The game's rule (`Entity.boardingCooldown` aside) and the plan's: a seat kept for a
/// player on their way, and a player waiting for their unit, are let go after 100 ticks
/// (section 4, item 4). Checks of a phantom seat wait longer than this, counted in round
/// trips, each of which takes at least a tick.
const SEAT_KEPT: usize = 100;

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
/// that start clusters of processes do not run.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_PINS").is_some()
}

async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name)
        .await
        .unwrap_or_else(|error| panic!("{name} could not join: {error:#}"));
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE)
        .await
        .unwrap_or_else(|error| panic!("{error:#}: {name} never had its chunks"));
    bot
}

/// A round trip: the server has handled everything the bot sent before, and the bot has
/// read what came before the answer. The dig of bedrock is acknowledged whether or not
/// it is within reach, and changes nothing.
async fn round_trip(bot: &mut Bot) {
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let last = bot.dig(x, BEDROCK, z).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap_or_else(|error| panic!("{error:#}: {}", bot.report()));
}

async fn stand(bot: &mut Bot, x: f64, z: f64) {
    bot.walk_to(x, z, 1.0).await.unwrap();
    round_trip(bot).await;
}

/// The entities `bot` shows with the UUID `uuid`.
fn shown(bot: &Bot, uuid: Uuid) -> Vec<&SeenEntity> {
    bot.entities
        .values()
        .filter(|entity| entity.uuid == uuid)
        .collect()
}

fn boats_of(bot: &Bot) -> Vec<&SeenEntity> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == entity_types::OAK_BOAT)
        .collect()
}

fn teleports(bot: &Bot) -> u32 {
    bot.stats
        .received
        .get("minecraft:player_position")
        .copied()
        .unwrap_or(0)
}

/// A player in a boat they placed, and what the test knows of the two.
struct Rider {
    bot: Bot,
    /// The boat's wire id and UUID as the rider is shown it.
    boat: i32,
    boat_uuid: Uuid,
    /// Where the rider's client has the boat.
    at: (f64, f64, f64),
    /// How many teleports, `set_passengers` and corrections the rider had been sent
    /// when it was seated.
    teleports_when_seated: u32,
    passengers_when_seated: usize,
    /// How many of the corrections the rider was sent its driving has followed.
    corrections_followed: usize,
}

impl Rider {
    /// Joins as `name`, stands at `x`, `z`, places an oak boat on the floor just east of
    /// itself and gets in.
    async fn join(address: &str, name: &str, x: f64, z: f64) -> Self {
        let mut bot = join(address, name).await;
        stand(&mut bot, x, z).await;
        bot.take_from_creative_inventory(0, items::OAK_BOAT)
            .await
            .unwrap();
        bot.select_slot(0).await.unwrap();
        round_trip(&mut bot).await;
        let before: Vec<Uuid> = boats_of(&bot).iter().map(|boat| boat.uuid).collect();
        let new = |bot: &Bot| -> Option<SeenEntity> {
            boats_of(bot)
                .into_iter()
                .find(|boat| !before.contains(&boat.uuid))
                .cloned()
        };
        // Looking down at 45 degrees the ray meets the floor some 1.6 blocks ahead, far
        // enough that the boat's box does not meet the player's, which refuses it.
        let placed = bot.use_item(EAST, 45.0).await.unwrap();
        bot.wait_until(PATIENCE, |bot| {
            bot.acknowledged_sequence >= placed && new(bot).is_some()
        })
        .await
        .unwrap_or_else(|error| panic!("{error:#}: {name} never placed a boat: {}", bot.report()));
        let boat = new(&bot).expect("placed");
        assert!(boat.position.0 > x, "placed ahead of the player: {boat:?}");
        let mut rider = Self {
            bot,
            boat: boat.entity_id,
            boat_uuid: boat.uuid,
            at: boat.position,
            teleports_when_seated: 0,
            passengers_when_seated: 0,
            corrections_followed: 0,
        };
        rider.get_in().await;
        rider
    }

    /// The rider's own entity id.
    fn own(&self) -> i32 {
        self.bot.info.login.entity_id
    }

    /// Gets into the boat it is shown and waits until it is told it rides it.
    async fn get_in(&mut self) {
        let boat = self.boat;
        self.bot.interact(boat, [0.0, 0.3, 0.0]).await.unwrap();
        self.bot
            .wait_until(PATIENCE, |bot| bot.riding() == Some(boat))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{error:#}: {} never rode the boat {boat}: {:?}",
                    self.bot.info.profile.name, self.bot.passenger_log
                )
            });
        round_trip(&mut self.bot).await;
        self.seated();
    }

    /// Notes what the rider has been sent so far, so that what comes later is judged on
    /// its own.
    fn seated(&mut self) {
        self.teleports_when_seated = teleports(&self.bot);
        self.passengers_when_seated = self.bot.passenger_log.len();
        if let Some(boat) = self.bot.entities.get(&self.boat) {
            self.at = boat.position;
        }
        self.bot.vehicle_corrections.clear();
        self.corrections_followed = 0;
    }

    /// Drives the boat east to block x `to` at [`STEP`] a tick, as a client does: one
    /// `move_vehicle` a tick, and where the server puts it back it goes on from there.
    /// `watchers` read what they are sent meanwhile.
    async fn drive(&mut self, to: f64, watchers: &mut [&mut Bot]) {
        while self.at.0 < to {
            self.follow_corrections();
            self.at.0 = (self.at.0 + STEP).min(to);
            self.tick(watchers).await;
        }
    }

    /// As [`Rider::drive`], west to block x `to`.
    async fn drive_west(&mut self, to: f64, watchers: &mut [&mut Bot]) {
        while self.at.0 > to {
            self.follow_corrections();
            self.at.0 = (self.at.0 - STEP).max(to);
            self.tick(watchers).await;
        }
    }

    /// Where the server put the boat back, the rider goes on from there, as a client
    /// does.
    fn follow_corrections(&mut self) {
        if self.bot.vehicle_corrections.len() > self.corrections_followed {
            let back = self.bot.vehicle_corrections.last().expect("one came");
            self.at = (back.x, back.y, back.z);
            self.corrections_followed = self.bot.vehicle_corrections.len();
        }
    }

    /// Sends the boat's place for one tick and reads what everyone is sent in it.
    async fn tick(&mut self, watchers: &mut [&mut Bot]) {
        self.bot
            .move_vehicle(self.at, EAST, 0.0, true)
            .await
            .unwrap();
        self.bot.location = self.at;
        self.bot.idle(TICK).await.unwrap();
        for watcher in watchers.iter_mut() {
            watcher.idle(Duration::from_millis(1)).await.unwrap();
        }
    }

    /// What the specification asks of the rider's own client since it was seated: it
    /// rides the boat still, and was sent no teleport (a mount or a dismount is one), no
    /// `set_passengers` of the boat without it, and no correction of its moves.
    fn assert_rode_throughout(&self) {
        let name = &self.bot.info.profile.name;
        let own = self.own();
        assert_eq!(
            self.bot.riding(),
            Some(self.boat),
            "{name} rides the boat: {:?}",
            self.bot.passengers
        );
        let since: Vec<_> = self.bot.passenger_log[self.passengers_when_seated..]
            .iter()
            .filter(|(vehicle, _)| *vehicle == self.boat)
            .collect();
        assert!(
            since.iter().all(|(_, riders)| riders.contains(&own)),
            "{name} was told the boat went on without it: {since:?}"
        );
        assert_eq!(
            teleports(&self.bot),
            self.teleports_when_seated,
            "{name} was teleported while it rode, as by a dismount or a mount"
        );
        assert!(
            self.bot.vehicle_corrections.is_empty(),
            "{name}'s moves of the boat were put back: {:?}",
            self.bot.vehicle_corrections
        );
        assert!(
            !self
                .bot
                .vanished
                .iter()
                .any(|gone| gone.uuid == self.boat_uuid),
            "{name} was told the boat went"
        );
        assert_eq!(shown(&self.bot, self.boat_uuid).len(), 1);
    }
}

/// What a watcher was shown of the boat since `from` in its log of `set_passengers`:
/// the boat once, by the wire id `boat`, never removed, and never without the rider
/// `rider` once it was shown with them.
fn assert_watched_riding(watcher: &Bot, boat: i32, boat_uuid: Uuid, rider: i32, from: usize) {
    let name = &watcher.info.profile.name;
    let shown = shown(watcher, boat_uuid);
    assert_eq!(shown.len(), 1, "{name} shows the boat once: {shown:?}");
    assert_eq!(shown[0].entity_id, boat, "{name} shows it by one wire id");
    assert!(
        !watcher.vanished.iter().any(|gone| gone.uuid == boat_uuid),
        "{name} was told the boat went"
    );
    let of_boat: Vec<&Vec<i32>> = watcher.passenger_log[from..]
        .iter()
        .filter(|(vehicle, _)| *vehicle == boat)
        .map(|(_, riders)| riders)
        .collect();
    let first = of_boat
        .iter()
        .position(|riders| riders.contains(&rider))
        .unwrap_or_else(|| panic!("{name} was never shown the rider in the boat"));
    assert!(
        of_boat[first..]
            .iter()
            .all(|riders| riders.contains(&rider)),
        "{name} was shown the boat without its rider, a dismount and a remount: {of_boat:?}"
    );
    assert_eq!(
        watcher.passengers.get(&boat).map(Vec::as_slice),
        Some(&[rider][..]),
        "{name} shows the rider in the boat at the end"
    );
}

/// A watcher north of where the boats go, at the line, who sees both sides of it.
async fn watcher(address: &str, name: &str) -> Bot {
    watcher_at(address, name, LINE).await
}

/// The same at the line at block x `line`.
async fn watcher_at(address: &str, name: &str, line: f64) -> Bot {
    let mut bot = join(address, name).await;
    stand(&mut bot, line - 0.5, -6.5).await;
    bot
}

/// Waits until `watcher` shows the boat of `rider` with the rider in it, and returns the
/// rider's wire id as the watcher shows it and how long the watcher's log was then.
async fn watching(watcher: &mut Bot, rider: &Rider) -> (i32, usize) {
    let name = rider.bot.info.profile.name.clone();
    let boat = rider.boat;
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.seen_player(&name).is_some_and(|seen| {
                bot.passengers
                    .get(&boat)
                    .is_some_and(|riders| riders.contains(&seen.entity_id))
            })
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: {} never showed {name} in the boat: {:?}",
                watcher.info.profile.name, watcher.passenger_log
            )
        });
    let id = watcher.seen_player(&name).expect("shown").entity_id;
    assert_eq!(id, rider.own(), "the rider is shown by its own id");
    (id, 0)
}

/// Reads what `bots` are sent until each has had a round trip.
async fn settle(bots: &mut [&mut Bot]) {
    for bot in bots.iter_mut() {
        round_trip(bot).await;
    }
}

/// Section 1 of the plan: "a player riding a boat or a cart across a line goes on
/// riding, with their vehicle and anyone else in it, without a jump back, a flicker or a
/// dismount"; section 7, item 2 and section 8, item 4: the rider is shown riding
/// throughout, whichever region speaks first; section 4, item 3 and section 7, item 4:
/// the kept moves are applied with no correction. A rider drives a boat east across the
/// pinned line, watched from beside the line.
#[tokio::test]
async fn a_player_driving_a_boat_across_a_pinned_line_goes_on_riding_shown_once_and_never_put_back()
{
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, "Watcher").await;
    let mut rider = Rider::join(&address, "Rider", -4.5, 0.5).await;
    let (shown_as, from) = watching(&mut watcher, &rider).await;

    rider.drive(4.0, &mut [&mut watcher]).await;
    settle(&mut [&mut rider.bot, &mut watcher]).await;
    // The boat is shown where the rider left it, east of the line.
    watcher
        .wait_until(PATIENCE, |bot| {
            shown(bot, rider.boat_uuid)
                .first()
                .is_some_and(|boat| boat.position.0 > LINE + 3.0)
        })
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the watcher never showed the boat across"));
    rider.assert_rode_throughout();
    assert_watched_riding(&watcher, rider.boat, rider.boat_uuid, shown_as, from);

    // The far side takes the boat's moves on: a little further on, still riding.
    rider.drive(6.0, &mut [&mut watcher]).await;
    settle(&mut [&mut rider.bot, &mut watcher]).await;
    rider.assert_rode_throughout();
    drop(rider);
    drop(watcher);
    server.stop().await;
}

/// Waits until `bot` shows the boat `uuid` with nobody in it, and returns it as shown.
async fn shown_empty(bot: &mut Bot, uuid: Uuid) -> SeenEntity {
    let empty = |bot: &Bot| {
        let shown = shown(bot, uuid);
        shown.len() == 1
            && bot
                .passengers
                .get(&shown[0].entity_id)
                .is_none_or(|riders| riders.is_empty())
    };
    bot.wait_until(PATIENCE, empty)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: {} never showed the boat {uuid} empty: {:?} {:?}",
                bot.info.profile.name,
                shown(bot, uuid),
                bot.passengers
            )
        });
    shown(bot, uuid)[0].clone()
}

/// How far apart two places are across the ground.
fn across(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.2 - b.2)
}

/// `docs/groundwork/n7h-logout-on-a-mount.md`, sections 1, 3 and 5, which narrow N5's
/// "the vehicle stays in its chunk" to a vehicle with another player aboard: a rider who
/// leaves alone takes their boat out of the world with them, as the game's
/// `PlayerList.remove` does, and comes back on it. A rider drives across the pinned line
/// and leaves; a watcher sees the boat go with them; after the server is stopped and
/// started, the rider joins again riding it, where it was, and it is there once.
#[tokio::test]
async fn a_rider_who_leaves_after_crossing_takes_the_boat_and_comes_back_on_it_after_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, "Watcher").await;
    let mut rider = Rider::join(&address, "Rider", -4.5, 0.5).await;
    watching(&mut watcher, &rider).await;
    rider.drive(3.0, &mut [&mut watcher]).await;
    settle(&mut [&mut rider.bot, &mut watcher]).await;
    rider.assert_rode_throughout();
    let uuid = rider.boat_uuid;
    let left_at = rider.at;
    rider.bot.close().await.unwrap();
    drop(rider);

    watcher
        .wait_until(PATIENCE, |bot| {
            bot.seen_player("Rider").is_none() && shown(bot, uuid).is_empty()
        })
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the boat goes with its rider"));
    drop(watcher);
    server.stop().await;

    let (server, address) = start_with(pinned(directory.path())).await;
    let mut back = join(&address, "Rider").await;
    back.wait_until(PATIENCE, |bot| {
        bot.riding().is_some_and(|vehicle| {
            shown(bot, uuid)
                .iter()
                .any(|boat| boat.entity_id == vehicle)
        })
    })
    .await
    .unwrap_or_else(|error| panic!("{error:#}: the rider comes back on the boat"));
    round_trip(&mut back).await;
    assert_eq!(boats_of(&back).len(), 1, "one boat: {:?}", boats_of(&back));
    let boat = shown(&back, uuid)[0].clone();
    assert!(
        across(boat.position, left_at) < 0.5,
        "the boat is where it was left: {boat:?}, left at {left_at:?}"
    );
    drop(back);
    server.stop().await;
}

/// `docs/groundwork/n7h-logout-on-a-mount.md`, section 13 item 1: a server stopped while
/// a rider is in their boat ends their stay as a leave first, as the game's
/// `PlayerList.removeAll` does at a stop, so that the rider, joining again as soon as the
/// server is up, is on the boat at once; it is there once.
#[tokio::test]
async fn a_rider_in_a_boat_when_the_server_stops_is_on_it_again_when_they_join_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut rider = Rider::join(&address, "Rider", -4.5, 0.5).await;
    settle(&mut [&mut rider.bot]).await;
    let uuid = rider.boat_uuid;
    let left_at = rider.at;
    // Stopped with the rider still connected.
    server.stop().await;
    drop(rider);

    let (server, address) = start_with(pinned(directory.path())).await;
    let mut back = join(&address, "Rider").await;
    back.wait_until(PATIENCE, |bot| {
        bot.riding().is_some_and(|vehicle| {
            shown(bot, uuid)
                .iter()
                .any(|boat| boat.entity_id == vehicle)
        })
    })
    .await
    .unwrap_or_else(|error| panic!("{error:#}: the rider is on the boat again: riding {:?}, boats {:?}, at {:?}, passengers {:?}", back.riding(), boats_of(&back), back.location, back.passengers));
    round_trip(&mut back).await;
    assert_eq!(boats_of(&back).len(), 1, "one boat: {:?}", boats_of(&back));
    let boat = shown(&back, uuid)[0].clone();
    assert!(
        across(boat.position, left_at) < 0.5,
        "where it was: {boat:?}, left at {left_at:?}"
    );
    drop(back);
    server.stop().await;
}

/// ADR-0028, section 5: "two players in one boat cross together"; the plan, section 4
/// item 2: "seat order is the order of `passengers`", and section 7 item 2: neither is
/// shown dismounted. A second player sits behind the driver; the boat crosses the line
/// with both, in the same order, and neither client nor a watcher is shown either get
/// off.
#[tokio::test]
async fn two_players_in_one_boat_cross_a_pinned_line_together_in_their_seats() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, "Watcher").await;
    let mut rider = Rider::join(&address, "Driver", -4.5, 0.5).await;
    let mut passenger = join(&address, "Passenger").await;
    stand(&mut passenger, -3.0, 2.5).await;
    passenger
        .wait_until(PATIENCE, |bot| shown(bot, rider.boat_uuid).len() == 1)
        .await
        .unwrap();
    let boat_as_shown = shown(&passenger, rider.boat_uuid)[0].entity_id;
    assert_eq!(boat_as_shown, rider.boat, "one wire id for everyone");
    passenger
        .interact(boat_as_shown, [0.0, 0.3, 0.0])
        .await
        .unwrap();
    let driver = rider.own();
    let behind = passenger.info.login.entity_id;
    for bot in [&mut passenger, &mut watcher, &mut rider.bot] {
        bot.wait_until(PATIENCE, |bot| {
            bot.passengers.get(&boat_as_shown).map(Vec::as_slice) == Some(&[driver, behind][..])
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: {} never showed the driver in front and the passenger behind: {:?}",
                bot.info.profile.name, bot.passenger_log
            )
        });
    }
    round_trip(&mut rider.bot).await;
    round_trip(&mut passenger).await;
    assert_eq!(
        teleports(&passenger),
        2,
        "the passenger was put in their seat by a teleport, after the one of joining"
    );
    rider.seated();
    let (logs, teleported) = (
        [passenger.passenger_log.len(), watcher.passenger_log.len()],
        teleports(&passenger),
    );

    rider.drive(4.0, &mut [&mut watcher, &mut passenger]).await;
    settle(&mut [&mut rider.bot, &mut watcher, &mut passenger]).await;
    watcher
        .wait_until(PATIENCE, |bot| {
            shown(bot, rider.boat_uuid)
                .first()
                .is_some_and(|boat| boat.position.0 > LINE + 3.0)
        })
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the watcher never showed the boat across"));
    rider.assert_rode_throughout();
    for (bot, from) in [&passenger, &watcher].into_iter().zip(logs) {
        let of_boat: Vec<&Vec<i32>> = bot.passenger_log[from..]
            .iter()
            .filter(|(vehicle, _)| *vehicle == boat_as_shown)
            .map(|(_, riders)| riders)
            .collect();
        assert!(
            of_boat.iter().all(|riders| **riders == [driver, behind]),
            "{} was shown the boat's seats change while it crossed: {of_boat:?}",
            bot.info.profile.name
        );
        assert_eq!(
            bot.passengers.get(&boat_as_shown).map(Vec::as_slice),
            Some(&[driver, behind][..])
        );
    }
    assert_eq!(
        teleports(&passenger),
        teleported,
        "the passenger was teleported while it rode"
    );
    drop(passenger);
    drop(rider);
    drop(watcher);
    server.stop().await;
}

/// The plan, section 4 item 4 and section 7 item 5: a rider who came with their unit is
/// seated there and stays so, not pending; brief 0.15 and plan N5b: "the shift dismount
/// from the latest `player_input`". Across the line the rider sneaks and gets off beside
/// the boat, which their client and a watcher are shown; the seat is free, and the rider
/// gets in again, in front.
#[tokio::test]
async fn a_rider_who_crossed_gets_off_by_sneaking_beside_the_boat_and_can_get_in_again() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, "Watcher").await;
    let mut rider = Rider::join(&address, "Rider", -4.5, 0.5).await;
    watching(&mut watcher, &rider).await;
    rider.drive(3.0, &mut [&mut watcher]).await;
    settle(&mut [&mut rider.bot, &mut watcher]).await;
    rider.assert_rode_throughout();

    let teleported = teleports(&rider.bot);
    rider.bot.sneak(true).await.unwrap();
    rider
        .bot
        .wait_until(PATIENCE, |bot| bot.riding().is_none())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: the rider never got off: {:?}",
                rider.bot.passenger_log
            )
        });
    rider.bot.sneak(false).await.unwrap();
    round_trip(&mut rider.bot).await;
    assert!(
        teleports(&rider.bot) > teleported,
        "getting off is a teleport, which the rider's client is sent"
    );
    let boat = shown_empty(&mut watcher, rider.boat_uuid).await;
    assert!(
        across(rider.bot.location, boat.position) < 3.0 && rider.bot.location.0 > LINE,
        "the rider stands beside the boat, east of the line: {:?}, the boat {boat:?}",
        rider.bot.location
    );
    rider.get_in().await;
    let (own, boat) = (rider.own(), rider.boat);
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.passengers.get(&boat).map(Vec::as_slice) == Some(&[own][..])
        })
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the watcher never showed the rider back in"));
    drop(rider);
    drop(watcher);
    server.stop().await;
}

/// The plan, section 4 items 3 to 5 and section 7 item 4: every crossing hands the rider
/// on with their unit, and a unit with a seat kept that is sent on is followed (section
/// 7 item 5, section 8 item 5). The rider drives east across the line, back west across
/// it and east again without stopping, and rides throughout, never put back.
#[tokio::test]
async fn a_rider_driving_back_and_forth_across_a_pinned_line_rides_throughout() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, "Watcher").await;
    let mut rider = Rider::join(&address, "Rider", -3.5, 0.5).await;
    let (shown_as, from) = watching(&mut watcher, &rider).await;
    rider.drive(1.0, &mut [&mut watcher]).await;
    rider.drive_west(-1.0, &mut [&mut watcher]).await;
    rider.drive(1.0, &mut [&mut watcher]).await;
    rider.drive_west(-1.0, &mut [&mut watcher]).await;
    rider.drive(2.0, &mut [&mut watcher]).await;
    settle(&mut [&mut rider.bot, &mut watcher]).await;
    watcher
        .wait_until(PATIENCE, |bot| {
            shown(bot, rider.boat_uuid)
                .first()
                .is_some_and(|boat| boat.position.0 > LINE + 1.0)
        })
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the watcher never showed the boat across"));
    rider.assert_rode_throughout();
    assert_watched_riding(&watcher, rider.boat, rider.boat_uuid, shown_as, from);
    drop(rider);
    drop(watcher);
    server.stop().await;
}

/// The plan, section 1: "a player riding a boat or a cart across a line goes on riding";
/// a cart is moved by the server, its rider's keys pushing it (brief 0.3), so its unit
/// reaches the far side before its rider's hand-over is seen there (section 7, item 2).
/// A rider holds the forward key in a cart on a straight rail across the pinned line;
/// the cart rolls across with them in it, and neither the rider nor a watcher is shown
/// them get off.
#[tokio::test]
async fn a_player_pushing_a_cart_across_a_pinned_line_by_their_keys_goes_on_riding() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, "Watcher").await;
    let mut bot = join(&address, "Rider").await;
    bot.take_from_creative_inventory(0, items::RAIL)
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
    // A straight rail along z = 0 from x = -8 to 12, laid from beside it, a few blocks
    // at a time, so that every block is within reach.
    for (from, to) in [(-8, -4), (-3, 1), (2, 6), (7, 12)] {
        stand(&mut bot, f64::from(from + to) / 2.0 + 0.5, 2.5).await;
        for x in from..=to {
            bot.use_item_on(x, -61, 0, face::TOP).await.unwrap();
        }
        round_trip(&mut bot).await;
    }
    stand(&mut bot, -2.5, 2.5).await;
    bot.take_from_creative_inventory(1, items::MINECART)
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();
    let placed = bot.use_item_on(-3, -60, 0, face::TOP).await.unwrap();
    let carts = |bot: &Bot| -> Vec<SeenEntity> {
        bot.entities
            .values()
            .filter(|entity| entity.kind == entity_types::MINECART)
            .cloned()
            .collect()
    };
    bot.wait_until(PATIENCE, |bot| {
        bot.acknowledged_sequence >= placed && carts(bot).len() == 1
    })
    .await
    .unwrap_or_else(|error| panic!("{error:#}: no cart was placed: {}", bot.report()));
    let cart = carts(&bot)[0].clone();
    let mut rider = Rider {
        bot,
        boat: cart.entity_id,
        boat_uuid: cart.uuid,
        at: cart.position,
        teleports_when_seated: 0,
        passengers_when_seated: 0,
        corrections_followed: 0,
    };
    rider.get_in().await;
    let (shown_as, from) = watching(&mut watcher, &rider).await;
    // The rider looks east, along the rail, and walks forwards.
    rider.bot.look = (EAST, 0.0);
    rider.bot.hold_keys(player_input::FORWARD).await.unwrap();
    let uuid = rider.boat_uuid;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !shown(&watcher, uuid)
        .first()
        .is_some_and(|cart| cart.position.0 > LINE + 2.0)
    {
        assert!(
            Instant::now() < deadline,
            "the cart never rolled across the line: {:?}",
            shown(&watcher, uuid)
        );
        rider.bot.idle(TICK).await.unwrap();
        watcher.idle(Duration::from_millis(1)).await.unwrap();
    }
    rider.bot.hold_keys(0).await.unwrap();
    settle(&mut [&mut rider.bot, &mut watcher]).await;
    rider.assert_rode_throughout();
    assert_watched_riding(&watcher, rider.boat, uuid, shown_as, from);
    drop(rider);
    drop(watcher);
    server.stop().await;
}

// ------------------------------------------------------------------------------------
// Crashes around a rider's crossing.

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
#[derive(Debug, Clone, Copy, PartialEq)]
enum Victim {
    /// The worker that runs the region west of the line, which sends the boat and hands
    /// the rider over.
    Sender,
    /// The worker that runs the region east of the line, which takes the boat in and
    /// seats the rider.
    Holder,
    /// The world store.
    Store,
    /// The edge, which every player is connected through: they connect again.
    Edge,
    /// Not killed: the first region absorbs the second (`clustine merge`).
    Merge(u32, u32),
    /// Not killed: the players standing in the chunk are split off the region
    /// (`clustine split`).
    Split(u32, (i32, i32)),
}

/// When around a crossing it is killed.
#[derive(Debug, Clone, Copy)]
enum Moment {
    /// The moment the rider's client has driven the boat over the line: the move that
    /// sends the unit and hands the rider over is with the edge or the region, or just
    /// committed.
    AtOnce,
    /// Once a watcher shows the boat across: sent, and its rider seated or pending.
    Across,
    /// Once the rider has driven on a few blocks and been seen to ride throughout: the
    /// crossing done and durable.
    Settled,
}

const MOMENTS: [Moment; 3] = [Moment::AtOnce, Moment::Across, Moment::Settled];

/// A cluster pinned at chunk x = 0 and 4, a watcher at the line, and a rider in a boat of
/// their own for each round, each in a row of their own west of the line.
struct Crossings {
    _turn: Turn,
    cluster: Cluster,
    _directory: KeptOnFailure,
    watcher: Bot,
    riders: Vec<Rider>,
    /// How many players have joined to look, each under a name of their own.
    auditors: usize,
    /// The block x of the line the riders cross.
    line: f64,
}

impl Crossings {
    /// A cluster pinned at chunk x = 0 and 4, whose riders cross the line at block x = 0
    /// out of land the home region does not hold into the home region's.
    async fn start() -> Self {
        Self::start_pinned("0,4", LINE).await
    }

    /// A cluster pinned at `pins`, whose riders cross the line at block x `line`.
    async fn start_pinned(pins: &str, line: f64) -> Self {
        let turn = turn().await;
        let directory = KeptOnFailure::new("clustine-riders-");
        // One worker for each region and one to spare.
        let mut cluster = Cluster::new(directory.path(), 4, pins).await;
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
        let watcher = watcher_at(&address, "Watcher", line).await;
        Self {
            _turn: turn,
            cluster,
            _directory: directory,
            watcher,
            riders: Vec::new(),
            auditors: 0,
            line,
        }
    }

    fn address(&self) -> String {
        self.cluster.edge.0.clone()
    }

    /// Reads what every player is sent for a moment.
    async fn tend(&mut self) {
        let _ = self.watcher.idle(Duration::from_millis(30)).await;
        for rider in &mut self.riders {
            let _ = rider.bot.idle(Duration::from_millis(10)).await;
        }
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
        let deadline = Instant::now() + RECOVERY;
        loop {
            let living = match self.cluster.regions().await {
                Ok(list) => list.regions.iter().map(|info| info.region.0).collect(),
                Err(_) => Vec::new(),
            };
            if !living.is_empty() && living.iter().all(|region| self.cluster.runs(*region)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "never did every region run:\n{}",
                self.cluster.all_logs()
            );
            self.tend().await;
        }
        round_trip(&mut self.watcher).await;
        for rider in &mut self.riders {
            round_trip(&mut rider.bot).await;
        }
    }

    /// Kills `victim` without warning and waits until the cluster is whole again.
    async fn kill(&mut self, victim: Victim) {
        let region = match victim {
            Victim::Sender => 0,
            Victim::Holder => 1,
            Victim::Store => {
                self.cluster.kill_and_start_the_store().await;
                self.whole().await;
                return;
            }
            Victim::Edge => {
                self.kill_the_edge().await;
                return;
            }
            Victim::Merge(survivor, absorbed) => {
                let what = format!("region {survivor} to absorb region {absorbed}");
                let asked = self
                    .ask_until_heard(&what, |cluster| cluster.merge_command(survivor, absorbed))
                    .await;
                assert!(
                    asked.says(&format!("has absorbed region {absorbed}")) || asked.code == Some(0),
                    "{what}: {}",
                    asked.outcome()
                );
                self.whole().await;
                return;
            }
            Victim::Split(region, chunk) => {
                let what = format!("region {region} to be split at {chunk:?}");
                // The region may not count the rider among its players yet when it is
                // first asked, their hand-over on its way: it is asked again until it
                // splits them off.
                let deadline = Instant::now() + PATIENCE;
                loop {
                    let asked = self
                        .ask_until_heard(&what, |cluster| cluster.split_command(region, &[chunk]))
                        .await;
                    if asked.says("has been split off") {
                        break;
                    }
                    assert!(
                        asked.was_told_no_because("no player stands in a chunk named")
                            && Instant::now() < deadline,
                        "{what}: {}",
                        asked.outcome()
                    );
                    self.tend().await;
                }
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

    /// Runs the command that `command` makes, anew for as long as it is turned away for
    /// a move of the coordinator's own, the players tended meanwhile.
    async fn ask_until_heard(
        &mut self,
        what: &str,
        command: impl Fn(&Cluster) -> tokio::process::Command,
    ) -> common::processes::Asked {
        let deadline = Instant::now() + RECOVERY;
        loop {
            let asking = common::processes::ask(what.to_owned(), command(&self.cluster));
            while !asking.is_finished() {
                self.tend().await;
            }
            let asked = asking.await.expect("asking does not panic");
            println!("{what}: {}", asked.outcome());
            if !asked.was_told_no_because("is being released") {
                return asked;
            }
            assert!(Instant::now() < deadline, "{what} was never heard");
            self.tend().await;
        }
    }

    /// Kills the edge and starts another where it listened, as the cluster starts it;
    /// every player connects again, and each rider finds their boat by its UUID.
    async fn kill_the_edge(&mut self) {
        let mut edge = self.cluster.edge.1.take().expect("the edge runs");
        edge.kill().await.unwrap();
        let view_distance = self.cluster.view_distance.to_string();
        let mut arguments = vec![
            "edge".to_owned(),
            "--coordinator".to_owned(),
            self.cluster.coordinator.0.clone(),
            "--bind".to_owned(),
            self.cluster.edge.0.clone(),
            "--view-distance".to_owned(),
            view_distance,
        ];
        arguments.extend(self.cluster.edge_arguments.iter().cloned());
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let edge = self.cluster.spawn("edge", &arguments);
        self.cluster.edge.1 = Some(edge);
        let address = self.address();
        self.watcher = rejoin(&address, "Watcher").await;
        stand(&mut self.watcher, self.line - 0.5, -6.5).await;
        for rider in &mut self.riders {
            let name = rider.bot.info.profile.name.clone();
            let mut bot = rejoin(&address, &name).await;
            let uuid = rider.boat_uuid;
            bot.wait_until(RECOVERY, |bot| shown(bot, uuid).len() == 1)
                .await
                .unwrap_or_else(|error| {
                    panic!("{error:#}: {name}, back, is never shown their boat")
                });
            round_trip(&mut bot).await;
            let boat = shown(&bot, uuid)[0].clone();
            // An edge's going is a leave (`docs/groundwork/n7h-logout-on-a-mount.md`,
            // section 8 row 6): a lone rider takes the boat and comes back on it, or,
            // where their new stay's entry raced the leave (row 2), beside it.
            assert!(
                bot.riding().is_none_or(|vehicle| vehicle == boat.entity_id),
                "{name} connected again riding something else"
            );
            // The plan, section 8 item 6: the removal writes the place they are
            // dismounted to.
            assert!(
                across(bot.location, boat.position) < 3.0,
                "{name} comes back on or beside their boat: at {:?}, the boat at {boat:?}",
                bot.location
            );
            rider.bot = bot;
            rider.boat = boat.entity_id;
            rider.at = boat.position;
        }
        self.whole().await;
    }

    /// Places a boat for a new rider, in a row of their own, gets them in and drives it
    /// across the line, killing `victim` at `moment`; returns once the cluster is whole
    /// again.
    async fn round(&mut self, victim: Victim, moment: Moment) {
        let row = self.riders.len();
        let name = format!("Rider{row}");
        let address = self.address();
        let mut rider = Rider::join(&address, &name, self.line - 4.5, 0.5 + 3.0 * row as f64).await;
        watching(&mut self.watcher, &rider).await;
        println!(
            "{name}: killing the {victim:?} {moment:?}; the sender runs on worker {:?}, the holder on {:?}",
            self.cluster.owner(0),
            self.cluster.owner(1)
        );
        let line = self.line;
        match moment {
            Moment::AtOnce => {
                rider.drive(line + 0.15, &mut [&mut self.watcher]).await;
            }
            Moment::Across => {
                rider.drive(line + 1.5, &mut [&mut self.watcher]).await;
                let uuid = rider.boat_uuid;
                self.watcher
                    .wait_until(PATIENCE, |bot| {
                        shown(bot, uuid)
                            .first()
                            .is_some_and(|boat| boat.position.0 > line + 0.5)
                    })
                    .await
                    .unwrap_or_else(|error| {
                        panic!("{error:#}: the watcher never showed the boat across")
                    });
            }
            Moment::Settled => {
                rider.drive(line + 3.0, &mut [&mut self.watcher]).await;
                settle(&mut [&mut rider.bot, &mut self.watcher]).await;
                rider.assert_rode_throughout();
            }
        }
        self.riders.push(rider);
        self.kill(victim).await;
    }

    /// What every durable state must have: the rider of `row` and their boat each in one
    /// place, the rider riding it or standing as their own client, a watcher and a
    /// newcomer all agree, and no seat held for anyone who is not in it: the rider, if
    /// standing, gets in again, and a newcomer gets in behind them. After any kill but
    /// the edge's, a standing rider first waits out the 100 ticks a seat may be kept for
    /// them; the seats of an edge that went are let go at once (section 8, item 9).
    async fn one_place(&mut self, row: usize, victim: Victim) {
        let uuid = self.riders[row].boat_uuid;
        let name = self.riders[row].bot.info.profile.name.clone();
        for bot in [&mut self.watcher, &mut self.riders[row].bot] {
            bot.wait_until(RECOVERY, |bot| shown(bot, uuid).len() == 1)
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "{error:#}: {} does not show {name}'s boat once: {:?}",
                        bot.info.profile.name,
                        shown(bot, uuid)
                    )
                });
            round_trip(bot).await;
        }
        let rider = &mut self.riders[row];
        let boat = shown(&rider.bot, uuid)[0].entity_id;
        rider.boat = boat;
        let own = rider.own();
        // The watcher and the rider agree whether the rider rides, once what each was
        // sent has come.
        let agree = |watcher: &Bot, rider: &Bot| {
            let watched = watcher
                .passengers
                .get(&boat)
                .is_some_and(|riders| riders.contains(&own));
            watched == (rider.riding() == Some(boat))
        };
        let deadline = Instant::now() + PATIENCE;
        while !agree(&self.watcher, &rider.bot) {
            assert!(
                Instant::now() < deadline,
                "{name} and the watcher disagree whether {name} rides: {:?} and {:?}",
                rider.bot.passengers,
                self.watcher.passengers.get(&boat)
            );
            let _ = rider.bot.idle(Duration::from_millis(20)).await;
            let _ = self.watcher.idle(Duration::from_millis(20)).await;
        }
        let riding = rider.bot.riding() == Some(boat);
        assert!(
            self.watcher
                .passengers
                .get(&boat)
                .is_none_or(|riders| riders.iter().all(|key| *key == own)),
            "{name}'s boat is shown with someone who is not in it: {:?}",
            self.watcher.passengers.get(&boat)
        );
        println!("{name} rides after recovery: {riding}");

        // A newcomer is shown the same.
        self.auditors += 1;
        let address = self.address();
        let auditor_name = format!("Auditor{}", self.auditors);
        let mut auditor = join(&address, &auditor_name).await;
        // Out of the way of the boats, which pass the spawn.
        stand(&mut auditor, self.line + 1.5, -6.5).await;
        auditor
            .wait_until(PATIENCE, |bot| shown(bot, uuid).len() == 1)
            .await
            .unwrap_or_else(|error| panic!("{error:#}: {auditor_name} is never shown the boat"));
        round_trip(&mut auditor).await;
        let told = auditor.passengers.get(&boat).cloned().unwrap_or_default();
        let expected = if riding { vec![own] } else { Vec::new() };
        assert_eq!(
            told, expected,
            "{auditor_name} is shown {name}'s boat with {told:?}, {name} riding: {riding}"
        );

        let rider = &mut self.riders[row];
        if riding {
            // Riding still, they drive on, and the far side takes their moves.
            let to = (rider.at.0 + 1.5).max(self.line + 3.0);
            rider.seated();
            rider
                .drive(to, &mut [&mut self.watcher, &mut auditor])
                .await;
            round_trip(&mut rider.bot).await;
            assert_eq!(rider.bot.riding(), Some(boat), "{name} rides on");
        } else {
            // Standing, they wait out every seat kept and get in again.
            let wait = if victim == Victim::Edge {
                0
            } else {
                SEAT_KEPT + 20
            };
            for _ in 0..wait {
                round_trip(&mut rider.bot).await;
                let _ = self.watcher.idle(Duration::from_millis(1)).await;
                let _ = auditor.idle(Duration::from_millis(1)).await;
            }
            let at = shown(&rider.bot, uuid)[0].position;
            stand(&mut rider.bot, at.0 - 1.5, at.2).await;
            rider.get_in().await;
        }
        // Behind them there is a seat, and nobody else holds it.
        let at = shown(&auditor, uuid)[0].position;
        stand(&mut auditor, at.0, at.2 + 2.0).await;
        auditor.interact(boat, [0.0, 0.3, 0.0]).await.unwrap();
        let both = [own, auditor.info.login.entity_id];
        for bot in [&mut auditor, &mut self.watcher] {
            bot.wait_until(PATIENCE, |bot| {
                bot.passengers.get(&boat).map(Vec::as_slice) == Some(&both[..])
            })
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{error:#}: {} never showed {name} in front and {auditor_name} behind: {:?}",
                    bot.info.profile.name,
                    bot.passengers.get(&boat)
                )
            });
        }
        auditor.close().await.unwrap();
        drop(auditor);
        shown_riding(&mut self.watcher, boat, own).await;
    }

    /// Kills every process at once and starts them again on the same disk: a newcomer is
    /// shown every rider's boat once, empty, as nobody is connected to ride it.
    async fn after_the_power_goes(mut self) {
        let uuids: Vec<Uuid> = self.riders.iter().map(|rider| rider.boat_uuid).collect();
        drop(self.riders);
        drop(self.watcher);
        self.cluster.kill().await;
        self.cluster.start().await;
        let address = self.cluster.edge.0.clone();
        let mut auditor = join(&address, "Watcher").await;
        stand(&mut auditor, self.line - 0.5, -6.5).await;
        for uuid in &uuids {
            shown_empty(&mut auditor, *uuid).await;
        }
        round_trip(&mut auditor).await;
        assert_eq!(
            boats_of(&auditor).len(),
            uuids.len(),
            "one boat for each rider: {:?}",
            boats_of(&auditor)
        );
        drop(auditor);
        self.cluster.terminate().await;
    }
}

/// Waits until `watcher` shows `rider` alone in `boat`.
async fn shown_riding(watcher: &mut Bot, boat: i32, rider: i32) {
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.passengers.get(&boat).map(Vec::as_slice) == Some(&[rider][..])
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: the watcher never showed {rider} alone in {boat} again: {:?}",
                watcher.passengers.get(&boat)
            )
        });
}

/// Connects again as `name` once the server lets them, which it may not at once while
/// the stay before is being ended.
async fn rejoin(address: &str, name: &str) -> Bot {
    let deadline = Instant::now() + RECOVERY;
    loop {
        match Bot::join(address, name).await {
            Ok(mut bot) => {
                let count = view_area((0, 0), VIEW_DISTANCE).len();
                bot.wait_for_chunks(count, PATIENCE)
                    .await
                    .unwrap_or_else(|error| panic!("{error:#}: {name} never had its chunks"));
                return bot;
            }
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "{name} could not connect again: {error:#}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

/// Drives a rider across the line for each [`Moment`], killing `victim` at it, and checks
/// after each that the rider and their boat are each in one place, riding or standing,
/// with no phantom seat; and that every boat is there once, empty, after every process is
/// killed and started again.
async fn killed_around_riders_crossings(victim: Victim) {
    let mut crossings = Crossings::start().await;
    for (row, moment) in MOMENTS.into_iter().enumerate() {
        crossings.round(victim, moment).await;
        crossings.one_place(row, victim).await;
        println!("{victim:?} after {moment:?}: in one place");
    }
    crossings.after_the_power_goes().await;
}

/// The plan, section 4 item 3 ("the unit's arrival and the players' hand-overs go in the
/// same commit") and section 7 items 5 and 6; ADR-0028, section 5 ("the entity is in
/// exactly one place in every durable state"): the worker of the sending region is
/// killed at each moment of a rider's crossing.
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_and_their_boat_are_each_in_one_place_however_the_sending_worker_is_killed()
 {
    if a_repetition() {
        return;
    }
    killed_around_riders_crossings(Victim::Sender).await;
}

/// As above, the worker of the region that takes the boat in and seats the rider killed
/// at each moment (section 8, item 1: a take-in keeps a seat with time left).
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_and_their_boat_are_each_in_one_place_however_the_holding_worker_is_killed()
 {
    if a_repetition() {
        return;
    }
    killed_around_riders_crossings(Victim::Holder).await;
}

/// As above, the world store killed at each moment.
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_and_their_boat_are_each_in_one_place_however_the_world_store_is_killed()
 {
    if a_repetition() {
        return;
    }
    killed_around_riders_crossings(Victim::Store).await;
}

/// As above, the edge killed at each moment: every player connects again, and the plan's
/// section 7 item 6 ("the edge's reset ... take the player out of their vehicle's
/// `passengers` in the same commit") and section 8 item 9 (the seats kept for an edge
/// that goes are dropped) leave the riders standing beside their boats, which nobody
/// holds a seat in.
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_and_their_boat_are_each_in_one_place_however_the_edge_is_killed() {
    if a_repetition() {
        return;
    }
    killed_around_riders_crossings(Victim::Edge).await;
}

/// Drives a rider across the line at block x `line` of a cluster pinned at `pins` and
/// reshapes the world at `moment` with `victim`, a merge or a split, and checks as after
/// a kill.
async fn reshaped_around_a_riders_crossing(pins: &str, line: f64, victim: Victim, moment: Moment) {
    let mut crossings = Crossings::start_pinned(pins, line).await;
    crossings.round(victim, moment).await;
    crossings.one_place(0, victim).await;
    crossings.after_the_power_goes().await;
}

/// The plan, section 8 item 1: "a restore, a merge, a split and a lend's load all take
/// chunks in. So each pending entry of a unit carries, saved with it, a countdown ... At
/// any take-in an entry with time left is kept"; and ADR-0028's amendment: "a region
/// restored, merged or split within those ticks has none [forwarding entry] (named)".
/// The holding region, home, absorbs the sending one the moment the rider crosses.
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_is_in_one_place_with_their_boat_when_the_holder_absorbs_the_sender_at_once()
 {
    if a_repetition() {
        return;
    }
    reshaped_around_a_riders_crossing("0,4", LINE, Victim::Merge(1, 0), Moment::AtOnce).await;
}

/// As above, in a world pinned at chunk x = 1 and 4, where the line at block x = 16 runs
/// between the home region and region 1: the sending region, home, absorbs the holding
/// one the moment the rider crosses, while their seat may be kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_is_in_one_place_with_their_boat_when_the_sender_absorbs_the_holder_at_once()
 {
    if a_repetition() {
        return;
    }
    reshaped_around_a_riders_crossing("1,4", 16.0, Victim::Merge(0, 1), Moment::AtOnce).await;
}

/// As above, the sending region absorbing the holding one once the boat is shown across.
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_is_in_one_place_with_their_boat_when_the_sender_absorbs_the_holder_once_across()
 {
    if a_repetition() {
        return;
    }
    reshaped_around_a_riders_crossing("1,4", 16.0, Victim::Merge(0, 1), Moment::Across).await;
}

/// As above, the players of the chunk the boat crosses into split off the holding
/// region as soon after the rider crosses as it counts them there (a split never takes
/// the home chunk, hence the line at block x = 16).
#[tokio::test(flavor = "multi_thread")]
async fn a_rider_crossing_a_line_is_in_one_place_with_their_boat_when_the_holder_is_split_at_once()
{
    if a_repetition() {
        return;
    }
    reshaped_around_a_riders_crossing("1,4", 16.0, Victim::Split(1, (1, 0)), Moment::AtOnce).await;
}
