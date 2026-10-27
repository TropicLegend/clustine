//! End-to-end tests of a player who leaves the world on a mount (step N7h), written from
//! `docs/groundwork/n7h-logout-on-a-mount.md` (sections 1, 8, 10 and 13) and the game's
//! `PlayerList.remove` and `removeAll` by someone who did not write the code.
//!
//! What a player sees, as section 1 has it: "leave on a pig, a cart, or a boat with a pig
//! behind you, and it goes with you; come back and you are on it, where it was. A boat
//! with two players stays." A watcher sees no vehicle while its rider is away (the probe
//! of the official server), and the vehicle comes back once, by its UUID. Section 13 item
//! 1: a server stopped with a rider aboard ends their stay as a leave first, so that the
//! rider joining as soon as it is up again is on the vehicle.
//!
//! The world is divided at chunk x = 0 and 4: the home region holds the land from block
//! x = 0 to 63, the spawn in it, and a pinned line runs at block x = 0. Each test divides
//! its world itself, so they run alike with or without `CLUSTINE_TEST_PINS`.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, SeenEntity};
use clustine_data::{entity_types, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::face;
use uuid::Uuid;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// Yaw in degrees looking east (+x).
const EAST: f32 = -90.0;

/// The block under the feet that a round trip breaks: bedrock, which nothing breaks.
const BEDROCK: i32 = -64;

/// The top block of the flat world's ground.
const GRASS: i32 = -61;

/// A world in `directory` divided at chunk x = 0 and 4, whose regions stay as they are.
fn pinned(directory: &std::path::Path) -> Config {
    Config {
        world: Some(directory.to_owned()),
        pins: vec![0, 4],
        follow: None,
        ..config()
    }
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

fn of_kind(bot: &Bot, kind: i32) -> Vec<&SeenEntity> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == kind)
        .collect()
}

fn across(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.2 - b.2)
}

/// A vehicle as its rider is shown it, by wire id and UUID, and where.
#[derive(Clone, Debug)]
struct Vehicle {
    id: i32,
    uuid: Uuid,
    at: (f64, f64, f64),
}

/// Waits until `bot` shows a new entity of `kind`, one that was not among `before`.
async fn new_of_kind(bot: &mut Bot, kind: i32, before: &[Uuid], what: &str) -> Vehicle {
    let new = |bot: &Bot| -> Option<SeenEntity> {
        of_kind(bot, kind)
            .into_iter()
            .find(|entity| !before.contains(&entity.uuid))
            .cloned()
    };
    bot.wait_until(PATIENCE, |bot| new(bot).is_some())
        .await
        .unwrap_or_else(|error| panic!("{error:#}: no {what} came: {}", bot.report()));
    let seen = new(bot).expect("came");
    Vehicle {
        id: seen.entity_id,
        uuid: seen.uuid,
        at: seen.position,
    }
}

/// Gets `bot` into `vehicle` with its main hand and waits until it is told it rides it.
async fn get_in(bot: &mut Bot, vehicle: i32) {
    bot.interact(vehicle, [0.0, 0.5, 0.0]).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.riding() == Some(vehicle))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: {} never rode {vehicle}: {:?}",
                bot.info.profile.name, bot.passenger_log
            )
        });
    round_trip(bot).await;
}

/// `name` joins, stands at `x`, `z`, places an oak boat on the ground just east of
/// itself and gets in.
async fn in_a_boat(address: &str, name: &str, x: f64, z: f64) -> (Bot, Vehicle) {
    let mut bot = join(address, name).await;
    stand(&mut bot, x, z).await;
    bot.take_from_creative_inventory(0, items::OAK_BOAT)
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
    round_trip(&mut bot).await;
    let before: Vec<Uuid> = of_kind(&bot, entity_types::OAK_BOAT)
        .iter()
        .map(|boat| boat.uuid)
        .collect();
    // Looking down at 45 degrees the ray meets the ground some 1.6 blocks ahead, far
    // enough that the boat's box does not meet the player's.
    bot.use_item(EAST, 45.0).await.unwrap();
    let boat = new_of_kind(&mut bot, entity_types::OAK_BOAT, &before, "boat").await;
    // The hand empty, so that the click on the boat is no use of a boat item.
    bot.select_slot(1).await.unwrap();
    get_in(&mut bot, boat.id).await;
    (bot, boat)
}

/// `name` joins, stands at `x`, `z`, hatches a pig two blocks east by a spawn egg, puts
/// a saddle on it and gets on with an empty hand.
async fn on_a_pig(address: &str, name: &str, x: f64, z: f64) -> (Bot, Vehicle) {
    let mut bot = join(address, name).await;
    stand(&mut bot, x, z).await;
    let before: Vec<Uuid> = of_kind(&bot, entity_types::PIG)
        .iter()
        .map(|pig| pig.uuid)
        .collect();
    let egg = ItemStack::new(items::PIG_SPAWN_EGG, 1);
    bot.put_in_hotbar(0, egg).await.unwrap();
    let saddle = ItemStack::new(items::SADDLE, 1);
    bot.put_in_hotbar(1, saddle).await.unwrap();
    bot.select_slot(0).await.unwrap();
    let (bx, bz) = (x.floor() as i32, z.floor() as i32);
    bot.use_item_on(bx + 2, GRASS, bz, face::TOP).await.unwrap();
    let pig = new_of_kind(&mut bot, entity_types::PIG, &before, "pig").await;
    bot.select_slot(1).await.unwrap();
    bot.interact(pig.id, [0.0, 0.5, 0.0]).await.unwrap();
    round_trip(&mut bot).await;
    bot.select_slot(2).await.unwrap();
    get_in(&mut bot, pig.id).await;
    (bot, pig)
}

/// Waits until `bot` rides the vehicle of `uuid`, shown once, and returns it as shown.
async fn riding_again(bot: &mut Bot, uuid: Uuid) -> SeenEntity {
    bot.wait_until(PATIENCE, |bot| {
        bot.riding().is_some_and(|vehicle| {
            shown(bot, uuid)
                .iter()
                .any(|seen| seen.entity_id == vehicle)
        })
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "{error:#}: {} is not on {uuid} again: riding {:?}, shown {:?}, at {:?}",
            bot.info.profile.name,
            bot.riding(),
            shown(bot, uuid),
            bot.location
        )
    });
    round_trip(bot).await;
    let seen = shown(bot, uuid);
    assert_eq!(seen.len(), 1, "the vehicle once: {seen:?}");
    seen[0].clone()
}

/// A watcher at `x`, `z`, who sees where the vehicles are.
async fn watcher(address: &str, x: f64, z: f64) -> Bot {
    let mut bot = join(address, "Watcher").await;
    stand(&mut bot, x, z).await;
    bot
}

/// Waits until `watcher` shows the rider `name` and the vehicle `uuid` gone.
async fn both_gone(watcher: &mut Bot, name: &str, uuid: Uuid) {
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.seen_player(name).is_none() && shown(bot, uuid).is_empty()
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: the vehicle goes with its rider: {:?}",
                shown(watcher, uuid)
            )
        });
}

/// Waits until `watcher` shows `name` riding the vehicle `uuid`, once.
async fn shown_riding(watcher: &mut Bot, name: &str, uuid: Uuid) {
    watcher
        .wait_until(PATIENCE, |bot| {
            let shown = shown(bot, uuid);
            bot.seen_player(name).is_some_and(|rider| {
                shown.len() == 1
                    && bot
                        .passengers
                        .get(&shown[0].entity_id)
                        .is_some_and(|riders| riders.contains(&rider.entity_id))
            })
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:#}: the watcher never showed {name} on {uuid}: {:?} {:?}",
                shown(watcher, uuid),
                watcher.passengers
            )
        });
}

/// Section 1 and row 1 of section 8: a rider who leaves a boat alone takes it out of the
/// world, which a watcher sees go; joining again they are on it, where it was, and the
/// watcher sees them on it.
#[tokio::test]
async fn a_rider_who_leaves_a_boat_and_joins_again_is_on_it_where_it_was() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, 6.5, -6.5).await;
    let (mut rider, boat) = in_a_boat(&address, "Rider", 3.5, 0.5).await;
    shown_riding(&mut watcher, "Rider", boat.uuid).await;
    rider.close().await.unwrap();
    drop(rider);
    both_gone(&mut watcher, "Rider", boat.uuid).await;

    let mut back = join(&address, "Rider").await;
    let seen = riding_again(&mut back, boat.uuid).await;
    assert!(
        across(seen.position, boat.at) < 0.5,
        "where it was: {seen:?}, left at {:?}",
        boat.at
    );
    assert_eq!(of_kind(&back, entity_types::OAK_BOAT).len(), 1);
    shown_riding(&mut watcher, "Rider", boat.uuid).await;
    round_trip(&mut watcher).await;
    assert_eq!(shown(&watcher, boat.uuid).len(), 1);
    drop((back, watcher));
    server.stop().await;
}

/// The probe of section 1, against Clustine: a saddled pig ridden alone goes with its
/// rider, a watcher sees no pig while the rider is away, and the rider comes back seated
/// on it.
#[tokio::test]
async fn a_rider_who_leaves_a_saddled_pig_and_joins_again_is_on_it() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, 6.5, -6.5).await;
    let (mut rider, pig) = on_a_pig(&address, "Rider", 3.5, 0.5).await;
    shown_riding(&mut watcher, "Rider", pig.uuid).await;
    rider.close().await.unwrap();
    drop(rider);
    both_gone(&mut watcher, "Rider", pig.uuid).await;
    round_trip(&mut watcher).await;
    assert!(
        of_kind(&watcher, entity_types::PIG).is_empty(),
        "no pig while its rider is away"
    );

    let mut back = join(&address, "Rider").await;
    riding_again(&mut back, pig.uuid).await;
    assert_eq!(of_kind(&back, entity_types::PIG).len(), 1, "one pig");
    shown_riding(&mut watcher, "Rider", pig.uuid).await;
    drop((back, watcher));
    server.stop().await;
}

/// Section 1, "a boat with two players stays", and row 18: of two players in a boat,
/// the one who leaves is put off and the boat stays with the other in it; joining
/// again, the first is not on it.
#[tokio::test]
async fn of_two_players_in_a_boat_the_one_who_leaves_leaves_it_to_the_other() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let (mut driver, boat) = in_a_boat(&address, "Driver", 3.5, 0.5).await;
    let mut passenger = join(&address, "Passenger").await;
    stand(&mut passenger, 4.5, 2.5).await;
    passenger
        .wait_until(PATIENCE, |bot| shown(bot, boat.uuid).len() == 1)
        .await
        .unwrap();
    let as_shown = shown(&passenger, boat.uuid)[0].entity_id;
    get_in(&mut passenger, as_shown).await;
    let behind = passenger.info.login.entity_id;
    driver
        .wait_until(PATIENCE, |bot| {
            bot.passengers
                .get(&boat.id)
                .is_some_and(|riders| riders.contains(&behind))
        })
        .await
        .unwrap();

    driver.close().await.unwrap();
    drop(driver);
    passenger
        .wait_until(PATIENCE, |bot| bot.seen_player("Driver").is_none())
        .await
        .unwrap();
    round_trip(&mut passenger).await;
    assert_eq!(passenger.riding(), Some(as_shown), "still in the boat");
    assert_eq!(shown(&passenger, boat.uuid).len(), 1, "the boat stays");
    assert_eq!(
        passenger.passengers.get(&as_shown).map(Vec::as_slice),
        Some(&[behind][..]),
        "with the passenger alone in it"
    );

    let mut back = join(&address, "Driver").await;
    back.wait_until(PATIENCE, |bot| shown(bot, boat.uuid).len() == 1)
        .await
        .unwrap();
    round_trip(&mut back).await;
    assert_eq!(back.riding(), None, "the one who left does not ride");
    let seen = shown(&back, boat.uuid)[0].clone();
    assert!(
        across(back.location, seen.position) < 3.0,
        "beside the boat: {:?} by {:?}",
        back.location,
        seen.position
    );
    drop((back, passenger));
    server.stop().await;
}

/// Section 13 item 1: a server stopped with a rider on a pig ends their stay as a leave
/// first (`PlayerList.removeAll`), so that the rider, joining as soon as it is up again,
/// is on the pig, and there is one pig.
#[tokio::test]
async fn a_rider_on_a_pig_when_the_server_stops_is_on_it_when_they_join_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let (mut rider, pig) = on_a_pig(&address, "Rider", 3.5, 0.5).await;
    round_trip(&mut rider).await;
    server.stop().await;
    drop(rider);

    let (server, address) = start_with(pinned(directory.path())).await;
    let mut back = join(&address, "Rider").await;
    riding_again(&mut back, pig.uuid).await;
    assert_eq!(of_kind(&back, entity_types::PIG).len(), 1, "one pig");
    drop(back);
    server.stop().await;
}

/// Section 10, "a pinned line between place and home", and rows 1 and 17: a rider who
/// leaves a boat west of the line, in another region than the home region, joins again
/// through the home region, is handed to the region of the boat and is on it there.
#[tokio::test]
async fn a_rider_who_leaves_a_boat_across_a_pinned_line_from_home_is_on_it_again() {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut watcher = watcher(&address, -0.5, -6.5).await;
    let (mut rider, boat) = in_a_boat(&address, "Rider", -9.5, 0.5).await;
    assert!(boat.at.0 < 0.0, "west of the line: {boat:?}");
    shown_riding(&mut watcher, "Rider", boat.uuid).await;
    rider.close().await.unwrap();
    drop(rider);
    both_gone(&mut watcher, "Rider", boat.uuid).await;

    let mut back = join(&address, "Rider").await;
    let seen = riding_again(&mut back, boat.uuid).await;
    assert!(
        across(seen.position, boat.at) < 0.5,
        "where it was: {seen:?}, left at {:?}",
        boat.at
    );
    shown_riding(&mut watcher, "Rider", boat.uuid).await;
    drop((back, watcher));
    server.stop().await;
}
