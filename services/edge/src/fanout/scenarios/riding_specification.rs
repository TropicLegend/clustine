//! Tests of what the edge shows of riding, written from the specification by someone
//! who did not write the edge's riding (step N5j): `docs/groundwork/n5-plan.md`, section
//! 7 item 2 and section 8 item 4; `docs/adr/0028-entities.md`, section 6 and its
//! amendment of step N5; `docs/groundwork/n5-brief.md`, findings 0.11 and 0.38 and
//! section 1.11.
//!
//! What they ask of the edge: every client that shows a vehicle is told whom it carries
//! (`set_passengers`) on every change of the list and when it is paired with the vehicle
//! or with a rider, a snapshot's whole entity included; a rider is sent to others by
//! rotation only, and the first move after they get off is a whole position; the rider's
//! own client is moved on and off as by a teleport and told whom the vehicle carries; a
//! vehicle's report counts by `hops`, as its moves do; and a player is shown riding by
//! their own region's word, so that a player whose unit crosses a line is never shown
//! dismounted or remounted, in either order of the unit's and the player's hand-overs,
//! and never in two vehicles, and one whose region says they got off is not shown in a
//! seat a vehicle's report still names.

use clustine_protocol::packets::play::ClientboundPlay;
use clustine_sim::api::Tracked;
use clustine_sim::{PlayerRiding, VehicleCheck};
use clustine_world::{Double, Hurt, Pending, Seat};

use super::*;

/// The chunk east of `HOME`, which the eastern region holds: the line is x = 16.
const EASTERN: ChunkPos = ChunkPos::new(1, 0);

const CART: u64 = 1 << 32;
/// Armour stands, which ride as entities of the vehicle's own region do.
const STAND: u64 = (1 << 32) + 1;
const OTHER_STAND: u64 = (1 << 32) + 2;

const RIDER: EntityId = EntityId(5);
const WATCHER: EntityId = EntityId(6);

/// Where the cart stands before it crosses, west of the line.
const BEFORE: [f64; 3] = [15.5, -60.0, 0.5];
/// Its pose at the line, which the west reports with "handed on".
const AT_LINE: [f64; 3] = [16.1, -60.0, 0.5];
/// Where the east's taking has it, having ticked once.
const TAKEN: [f64; 3] = [16.4, -60.0, 0.5];

/// Where the rider sits.
const SEATED: Vec3 = Vec3::new(15.5, -59.9375, 0.5);

fn own(number: u128, entity: EntityId) -> EntityState {
    EntityState {
        entity,
        kind: EntityKind::Player {
            player: player(number),
            name: format!("Player{number}"),
        },
        pose: Pose::at(SPAWN),
    }
}

fn seat(key: u64, player: bool, pending: bool) -> Seat {
    Seat {
        key: EntityId(key),
        player,
        pending: pending.then_some(Pending {
            left: 100,
            edge: IDENTITY.edge,
            granted: false,
        }),
    }
}

fn riding_of(vehicle: Option<u64>, passengers: Vec<Seat>) -> clustine_world::Riding {
    clustine_world::Riding {
        vehicle: vehicle.map(EntityId),
        passengers,
        cooldown: 0,
        leash: None,
    }
}

fn at(position: [f64; 3]) -> Pose {
    Pose::at(Vec3::new(position[0], position[1], position[2]))
}

/// The cart `CART` at `position`, with `hops` changes of holder behind it, carrying
/// `passengers`.
fn cart(position: [f64; 3], hops: u32, passengers: Vec<Seat>) -> EntityState {
    EntityState {
        entity: EntityId(CART),
        kind: EntityKind::Other {
            kind: entity_types::MINECART as u16,
            uuid: Uuid::from_u128(u128::from(CART) + 9_000),
            motion: [0.3, 0.0, 0.0],
            hops,
            data: EntityData::Minecart {
                hurt: Hurt::default(),
                display: None,
                display_offset: None,
                flipped: false,
                contents: Vec::new(),
                fuel: 0,
                push: [Double(0.0), Double(0.0)],
                fuse: -1,
                enabled: true,
            },
            riding: riding_of(None, passengers),
        },
        pose: at(position),
    }
}

/// The boat `CART` (a boat takes two), carrying `passengers`.
fn boat(position: [f64; 3], passengers: Vec<Seat>) -> EntityState {
    EntityState {
        entity: EntityId(CART),
        kind: EntityKind::Other {
            kind: entity_types::OAK_BOAT as u16,
            uuid: Uuid::from_u128(u128::from(CART) + 9_000),
            motion: [0.0; 3],
            hops: 0,
            data: EntityData::Boat {
                hurt: Hurt::default(),
                paddles: [false, false],
                bubble_time: 0,
                out_of_control: clustine_world::Float(0.0),
                turning: clustine_world::Float(0.0),
                status: [0, 0],
                contents: Vec::new(),
            },
            riding: riding_of(None, passengers),
        },
        pose: at(position),
    }
}

/// An armour stand of the key `key` at `position`, riding `vehicle` if any.
fn stand(key: u64, position: [f64; 3], vehicle: Option<u64>) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: entity_types::ARMOR_STAND as u16,
            uuid: Uuid::from_u128(u128::from(key) + 9_000),
            motion: [0.0; 3],
            hops: 0,
            data: EntityData::ArmorStand {
                flags: 0,
                disabled_slots: 0,
                pose: Box::new([[clustine_world::Float(0.0); 3]; 6]),
                equipment: Vec::new(),
                health: clustine_world::Float(20.0),
                last_hit: None,
                air: 300,
                frozen: 0,
            },
            riding: riding_of(vehicle, Vec::new()),
        },
        pose: at(position),
    }
}

fn delta(tick: u64, events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick, events }
}

fn passengers(chunk: ChunkPos, hops: u32, riders: &[u64]) -> RegionEvent {
    RegionEvent::Passengers {
        vehicle: EntityId(CART),
        chunk,
        hops,
        passengers: riders.iter().map(|key| EntityId(*key)).collect(),
    }
}

fn rides(number: u128, vehicle: Option<u64>, position: Vec3) -> WorkerToEdge {
    WorkerToEdge::ToPlayer {
        player: player(number),
        event: PlayerEvent::Rides {
            vehicle: vehicle.map(EntityId),
            pose: Pose::at(position),
        },
    }
}

fn cart_tracked(position: [f64; 3], hops: u32) -> Tracked {
    Tracked {
        entity: EntityId(CART),
        chunk: ChunkPos::containing(position[0], position[2]),
        hops,
        position,
        motion: [0.3, 0.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        flags: Tracked::SYNC_POSITION,
        data: None,
    }
}

/// The lists of passengers `client` was told for the vehicle `vehicle`, from the
/// `from`th `set_passengers` on, by the wire ids the client knows.
fn lists_for(client: &Client, from: usize, vehicle: u64) -> Vec<Vec<i32>> {
    let id = client.ledger_id(vehicle);
    client.passengers[from..]
        .iter()
        .filter(|told| told.vehicle_id == id)
        .map(|told| told.passengers.clone())
        .collect()
}

/// Every vehicle other than `vehicle` that `client` was told carries `rider`.
fn other_vehicles_of(client: &Client, vehicle: u64, rider: u64) -> Vec<i32> {
    let vehicle = client.ledger_id(vehicle);
    let rider = client.ledger_id(rider);
    client
        .passengers
        .iter()
        .filter(|told| told.vehicle_id != vehicle && told.passengers.contains(&rider))
        .map(|told| told.vehicle_id)
        .collect()
}

/// The packets carrying a position of the entity of the wire id `id` that `client` had
/// from the `from`th tracking packet on, as names.
fn positions_of(client: &Client, from: usize, id: i32) -> Vec<&'static str> {
    client.tracking[from..]
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::SyncEntityPosition(moved) if moved.entity_id == id => {
                Some("entity_position_sync")
            }
            ClientboundPlay::MoveEntityPos(moved) if moved.entity_id == id => {
                Some("move_entity_pos")
            }
            ClientboundPlay::MoveEntityPosRot(moved) if moved.entity_id == id => {
                Some("move_entity_pos_rot")
            }
            _ => None,
        })
        .collect()
}

/// An edge with two players at the spawn in `HOME`, the west's, shown `HOME` from the
/// west and `EASTERN` from the east, and the cart in `HOME` with player 1 in it, by the
/// west's word of the cart and of player 1. Both clients have been told the cart
/// carries player 1.
async fn riding_by_the_line() -> (Harness, Client, Client) {
    let mut edge = Harness::start().await;
    let mut rider = edge.joined(player(1), RIDER).await;
    let mut watcher = edge.joined(player(2), WATCHER).await;
    edge.quiet().await;
    let there = vec![own(1, RIDER), own(2, WATCHER), cart(BEFORE, 0, Vec::new())];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    let ask = edge.ask(WEST, EASTERN);
    edge.tell(WEST, elsewhere(EASTERN, ask, EAST));
    edge.settle(WEST).await;
    edge.settle(EAST).await;
    let ask = edge.ask(EAST, EASTERN);
    edge.tell(EAST, snapshot_of(EASTERN, ask, empty_chunk(), Vec::new()));
    assert!(edge.shows(&mut rider, WEST, HOME).await);
    assert!(edge.shows(&mut rider, EAST, EASTERN).await);
    assert!(edge.shows(&mut watcher, EAST, EASTERN).await);

    edge.tell(WEST, delta(2, vec![passengers(HOME, 0, &[RIDER.0])]));
    edge.tell(WEST, rides(1, Some(CART), SEATED));
    edge.quiet().await;
    rider.drain();
    watcher.drain();
    for client in [&rider, &watcher] {
        let rider_id = client.ledger_id(RIDER.0);
        assert_eq!(
            lists_for(client, 0, CART).last(),
            Some(&vec![rider_id]),
            "the cart was shown carrying player 1"
        );
    }
    (edge, rider, watcher)
}

/// The transfer with which the west lets player 1 go to the east with their unit, which
/// the east holds in `EASTERN` by then.
fn riding_transfer() -> PlayerTransfer {
    PlayerTransfer {
        sleeping: None,
        respawn: None,
        experience: Default::default(),
        attack_ticks: None,
        using: None,
        cooldowns: Vec::new(),
        effects: Vec::new(),
        time_since_rest: 0,
        entity_id: RIDER,
        name: "Player1".to_owned(),
        pose: Pose::at(Vec3::new(TAKEN[0], SEATED.y, TAKEN[2])),
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: Default::default(),
        last_input: 0,
        hops: 1,
        flying: false,
        attempt: None,
        riding: Some(PlayerRiding {
            vehicle: EntityId(CART),
            chunk: EASTERN,
            check: VehicleCheck::default(),
            pending: Some(100),
        }),
        keys: Default::default(),
        kept: Vec::new(),
    }
}

async fn west_lets_the_rider_go(edge: &mut Harness) {
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: riding_transfer(),
            to: EAST,
        },
    );
    edge.settle(WEST).await;
}

async fn west_hands_the_cart_on(edge: &mut Harness) {
    edge.tell(
        WEST,
        delta(
            3,
            vec![RegionEvent::HandedOn {
                entity: EntityId(CART),
                chunk: HOME,
                hops: 0,
                position: AT_LINE,
                motion: [0.3, 0.0, 0.0],
            }],
        ),
    );
    edge.settle(WEST).await;
}

/// The east takes the unit, reporting it whole with `riders` in its seats and its list
/// of passengers as `listed`.
async fn east_takes_the_cart(edge: &mut Harness, riders: Vec<Seat>, listed: &[u64]) {
    edge.tell(
        EAST,
        delta(
            3,
            vec![
                RegionEvent::EntitySpawned(cart(TAKEN, 1, riders)),
                passengers(EASTERN, 1, listed),
            ],
        ),
    );
    edge.settle(EAST).await;
}

/// The east's ticks after the crossing: the cart rolls on, the rider is seated and
/// looks about.
async fn east_rolls_on(edge: &mut Harness) {
    for tick in 4..=12 {
        let x = TAKEN[0] + 0.3 * (tick - 3) as f64;
        edge.tell(
            EAST,
            delta(
                tick,
                vec![
                    RegionEvent::EntityTracked(cart_tracked([x, -60.0, 0.5], 1)),
                    RegionEvent::EntityMoved {
                        entity: RIDER,
                        pose: Pose {
                            position: Vec3::new(x, SEATED.y, 0.5),
                            yaw: tick as f32 * 10.0,
                            pitch: 0.0,
                            on_ground: false,
                        },
                        previous_chunk: EASTERN,
                    },
                ],
            ),
        );
    }
    edge.tell(EAST, delta(13, vec![passengers(EASTERN, 1, &[RIDER.0])]));
    edge.quiet().await;
}

/// What a crossing must leave each client with: every `set_passengers` of the cart
/// since `from` names the rider, the cart was never removed, the rider was put in no
/// other vehicle and was sent no position while riding.
fn assert_never_dismounted(clients: [&Client; 2], from: [usize; 2], tracking_from: [usize; 2]) {
    for (index, client) in clients.into_iter().enumerate() {
        let rider_id = client.ledger_id(RIDER.0);
        let lists = lists_for(client, from[index], CART);
        assert!(
            lists.iter().all(|list| list.contains(&rider_id)),
            "client {index} was told the cart carries {lists:?}, without the rider {rider_id}"
        );
        let removals: Vec<_> = client
            .naming(CART)
            .into_iter()
            .filter(|(naming, _)| *naming == Naming::Removal)
            .collect();
        assert!(removals.is_empty(), "client {index}: {removals:?}");
        assert_eq!(other_vehicles_of(client, CART, RIDER.0), Vec::<i32>::new());
        if client.ledger_id(RIDER.0) != client.own_id() {
            assert_eq!(
                positions_of(client, tracking_from[index], rider_id),
                Vec::<&str>::new(),
                "a rider is sent no position"
            );
        }
    }
}

/// Plan, section 7 item 2; ADR-0028's amendment: the unit reaches the new region first,
/// which reports it with the pending player in their seat, and the player's hand-over
/// comes after. Neither client is shown the cart without its rider at any step.
#[tokio::test]
async fn a_rider_whose_cart_is_taken_before_they_come_is_never_shown_dismounted() {
    let (mut edge, mut rider, mut watcher) = riding_by_the_line().await;
    let from = [rider.passengers.len(), watcher.passengers.len()];
    let tracking = [rider.tracking.len(), watcher.tracking.len()];

    west_hands_the_cart_on(&mut edge).await;
    east_takes_the_cart(&mut edge, vec![seat(RIDER.0, true, true)], &[RIDER.0]).await;
    west_lets_the_rider_go(&mut edge).await;
    east_rolls_on(&mut edge).await;
    rider.drain();
    watcher.drain();
    assert_never_dismounted([&rider, &watcher], from, tracking);
}

/// The same with the player's hand-over read before the unit's taking.
#[tokio::test]
async fn a_rider_who_comes_before_their_cart_is_never_shown_dismounted() {
    let (mut edge, mut rider, mut watcher) = riding_by_the_line().await;
    let from = [rider.passengers.len(), watcher.passengers.len()];
    let tracking = [rider.tracking.len(), watcher.tracking.len()];

    west_hands_the_cart_on(&mut edge).await;
    west_lets_the_rider_go(&mut edge).await;
    east_takes_the_cart(&mut edge, vec![seat(RIDER.0, true, true)], &[RIDER.0]).await;
    east_rolls_on(&mut edge).await;
    rider.drain();
    watcher.drain();
    assert_never_dismounted([&rider, &watcher], from, tracking);
}

/// Plan, section 8 item 4: "the edge shows a player in that list only where the
/// player's own latest stay says they ride K, and adds one it has the player's word for
/// and the vehicle's report has not yet named". The player's transfer says they ride
/// the cart; the east's first report of the cart does not name them yet.
#[tokio::test]
async fn a_rider_whose_word_says_they_ride_is_shown_riding_before_the_vehicle_names_them() {
    let (mut edge, mut rider, mut watcher) = riding_by_the_line().await;
    let from = [rider.passengers.len(), watcher.passengers.len()];
    let tracking = [rider.tracking.len(), watcher.tracking.len()];

    west_hands_the_cart_on(&mut edge).await;
    west_lets_the_rider_go(&mut edge).await;
    east_takes_the_cart(&mut edge, Vec::new(), &[]).await;
    east_rolls_on(&mut edge).await;
    rider.drain();
    watcher.drain();
    assert_never_dismounted([&rider, &watcher], from, tracking);
}

/// Plan, section 8 item 4, the other half: a player whose own region says they got off
/// (a rider whose unit never came is dismounted where they are, plan section 4 item 4)
/// is not shown in the seat that the vehicle's region still names in a later report
/// (a restore of the sender, whose loads raise `hops`).
#[tokio::test]
async fn a_player_whose_region_says_they_got_off_is_not_shown_in_a_seat_a_vehicle_still_names() {
    let (mut edge, mut rider, mut watcher) = riding_by_the_line().await;
    west_lets_the_rider_go(&mut edge).await;
    // The unit never comes: the east lets the rider off where they are.
    edge.tell(EAST, rides(1, None, Vec3::new(16.4, -60.0, 0.5)));
    edge.settle(EAST).await;
    // The west, restored, reports the cart again with the seat it had.
    edge.tell(
        WEST,
        delta(
            4,
            vec![
                RegionEvent::EntitySpawned(cart(BEFORE, 1, vec![seat(RIDER.0, true, true)])),
                passengers(HOME, 1, &[RIDER.0]),
            ],
        ),
    );
    edge.quiet().await;
    rider.drain();
    watcher.drain();
    for client in [&rider, &watcher] {
        let rider_id = client.ledger_id(RIDER.0);
        let last = lists_for(client, 0, CART).last().cloned();
        assert!(
            last.as_ref().is_some_and(|list| !list.contains(&rider_id)),
            "the cart was last shown carrying {last:?}"
        );
    }
}

/// Brief, finding 0.11 and section 2.6: the riding player is sent their own
/// `player_position` and `set_passengers` when they get on, by their region's word of
/// it, whatever the vehicle's report says and before it comes.
#[tokio::test]
async fn a_player_told_they_ride_is_shown_riding_by_their_own_regions_word() {
    let mut edge = Harness::start().await;
    let mut rider = edge.joined(player(1), RIDER).await;
    let mut watcher = edge.joined(player(2), WATCHER).await;
    edge.quiet().await;
    let there = vec![own(1, RIDER), own(2, WATCHER), cart(BEFORE, 0, Vec::new())];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut rider, WEST, HOME).await);
    assert!(edge.shows(&mut watcher, WEST, HOME).await);
    let teleports = rider.teleports.len();

    edge.tell(WEST, rides(1, Some(CART), SEATED));
    edge.quiet().await;
    rider.drain();
    watcher.drain();
    assert_eq!(rider.teleports.len(), teleports + 1);
    for client in [&rider, &watcher] {
        let rider_id = client.ledger_id(RIDER.0);
        assert_eq!(lists_for(client, 0, CART).last(), Some(&vec![rider_id]));
    }
}

/// ADR-0028, section 6, and the plan, section 7 item 2: reports of passengers obey the
/// `hops` rule, as moves do. Once the east shows the cart with one more change of
/// holder, the west's word of its passengers, and a third region's at the east's
/// `hops`, are passed over.
#[tokio::test]
async fn a_report_of_passengers_with_fewer_hops_or_from_another_region_is_passed_over() {
    let (mut edge, mut rider, mut watcher) = riding_by_the_line().await;
    west_hands_the_cart_on(&mut edge).await;
    east_takes_the_cart(&mut edge, vec![seat(RIDER.0, true, true)], &[RIDER.0]).await;
    west_lets_the_rider_go(&mut edge).await;
    edge.quiet().await;
    rider.drain();
    watcher.drain();
    let from = [rider.passengers.len(), watcher.passengers.len()];

    edge.tell(WEST, delta(5, vec![passengers(HOME, 0, &[])]));
    edge.tell(NORTH, delta(5, vec![passengers(EASTERN, 1, &[])]));
    edge.quiet().await;
    rider.drain();
    watcher.drain();
    assert_eq!(lists_for(&rider, from[0], CART), Vec::<Vec<i32>>::new());
    assert_eq!(lists_for(&watcher, from[1], CART), Vec::<Vec<i32>>::new());

    // The east's own word counts.
    edge.tell(EAST, delta(5, vec![passengers(EASTERN, 1, &[STAND])]));
    edge.quiet().await;
    watcher.drain();
    assert_eq!(lists_for(&watcher, from[1], CART).len(), 1);
}

/// Brief, section 1.11.1: `set_passengers` goes to every tracker on any change of the
/// list, in order; the rider's own client among them.
#[tokio::test]
async fn every_change_of_a_boats_passengers_is_told_in_order_to_every_client_that_shows_it() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), RIDER).await;
    let mut second = edge.joined(player(2), WATCHER).await;
    edge.quiet().await;
    let there = vec![
        own(1, RIDER),
        own(2, WATCHER),
        boat(BEFORE, Vec::new()),
        stand(STAND, [14.5, -60.0, 0.5], None),
    ];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(edge.shows(&mut second, WEST, HOME).await);
    let from = [first.passengers.len(), second.passengers.len()];

    // Player 1 gets in, then the stand, then player 1 gets off.
    edge.tell(WEST, delta(2, vec![passengers(HOME, 0, &[RIDER.0])]));
    edge.tell(WEST, rides(1, Some(CART), SEATED));
    edge.tell(WEST, delta(3, vec![passengers(HOME, 0, &[RIDER.0, STAND])]));
    edge.tell(WEST, delta(4, vec![passengers(HOME, 0, &[STAND])]));
    edge.tell(WEST, rides(1, None, Vec3::new(15.5, -60.0, 1.5)));
    edge.quiet().await;
    first.drain();
    second.drain();
    for (index, client) in [&first, &second].into_iter().enumerate() {
        let rider = client.ledger_id(RIDER.0);
        let stand = client.ledger_id(STAND);
        let mut lists = lists_for(client, from[index], CART);
        lists.dedup();
        assert_eq!(
            lists,
            [vec![rider], vec![rider, stand], vec![stand]],
            "client {index}"
        );
    }
}

/// Brief, section 1.11.2, items 6 and 7: a client newly shown a vehicle is told whom it
/// carries, and one newly shown a rider is told what that rider rides, whichever it is
/// shown first: spawned in either order.
#[tokio::test]
async fn a_client_is_told_whom_a_vehicle_carries_whichever_of_the_two_it_is_shown_first() {
    for vehicle_first in [true, false] {
        let mut edge = Harness::start().await;
        let mut watcher = edge.joined(player(2), WATCHER).await;
        edge.quiet().await;
        let ask = edge.ask(WEST, HOME);
        edge.tell(
            WEST,
            snapshot_of(HOME, ask, empty_chunk(), vec![own(2, WATCHER)]),
        );
        assert!(edge.shows(&mut watcher, WEST, HOME).await);
        let vehicle = RegionEvent::EntitySpawned(cart(BEFORE, 0, vec![seat(STAND, false, false)]));
        let rider = RegionEvent::EntitySpawned(stand(STAND, BEFORE, Some(CART)));
        let events = if vehicle_first {
            vec![vehicle, rider]
        } else {
            vec![rider, vehicle]
        };
        edge.tell(WEST, delta(2, events));
        edge.quiet().await;
        watcher.drain();
        let stand_id = watcher.ledger_id(STAND);
        assert_eq!(
            lists_for(&watcher, 0, CART).last(),
            Some(&vec![stand_id]),
            "vehicle first: {vehicle_first}"
        );
    }
}

/// Brief, section 1.11.2: a client newly shown a vehicle from a snapshot is told its
/// passengers in their order, from the riding the snapshot's whole entity carries.
#[tokio::test]
async fn a_snapshot_tells_a_new_client_a_vehicles_passengers_in_their_order() {
    let mut edge = Harness::start().await;
    let mut watcher = edge.joined(player(2), WATCHER).await;
    edge.quiet().await;
    let there = vec![
        own(2, WATCHER),
        boat(
            BEFORE,
            vec![seat(OTHER_STAND, false, false), seat(STAND, false, false)],
        ),
        stand(OTHER_STAND, BEFORE, Some(CART)),
        stand(STAND, BEFORE, Some(CART)),
    ];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut watcher, WEST, HOME).await);
    watcher.drain();
    let order = vec![watcher.ledger_id(OTHER_STAND), watcher.ledger_id(STAND)];
    assert_eq!(lists_for(&watcher, 0, CART).last(), Some(&order));
}

/// Brief, findings 0.11 and section 1.11.1: a rider is sent no position, only which way
/// they look; the first move after getting off is a whole `entity_position_sync`
/// (`wasRiding`), whatever their old place, here a few hundredths from it, would allow
/// as a relative move; the rider's own client is moved on and off as by a teleport.
#[tokio::test]
async fn a_rider_is_sent_by_rotation_alone_and_their_first_move_off_is_a_whole_position() {
    let (mut edge, mut rider, mut watcher) = riding_by_the_line().await;
    let rider_id = watcher.ledger_id(RIDER.0);
    let tracking = watcher.tracking.len();
    let teleports = rider.teleports.len();
    for tick in 3..=8 {
        edge.tell(
            WEST,
            delta(
                tick,
                vec![RegionEvent::EntityMoved {
                    entity: RIDER,
                    pose: Pose {
                        position: SEATED,
                        yaw: tick as f32 * 15.0,
                        pitch: 5.0,
                        on_ground: false,
                    },
                    previous_chunk: HOME,
                }],
            ),
        );
    }
    edge.quiet().await;
    watcher.drain();
    assert_eq!(
        positions_of(&watcher, tracking, rider_id),
        Vec::<&str>::new()
    );
    assert!(
        watcher
            .naming(RIDER.0)
            .iter()
            .any(|(naming, _)| *naming == Naming::HeadRotation),
        "the rider's turns are shown"
    );

    // Off, beside where they last stood before they got on.
    let off = Vec3::new(SPAWN.x + 0.03, SPAWN.y, SPAWN.z);
    edge.tell(WEST, delta(9, vec![passengers(HOME, 0, &[])]));
    edge.tell(WEST, rides(1, None, off));
    edge.quiet().await;
    rider.drain();
    let teleport = rider.teleports.last().expect("a teleport off");
    assert_eq!(rider.teleports.len(), teleports + 1);
    assert_eq!((teleport.x, teleport.y, teleport.z), (off.x, off.y, off.z));
    let tracking = watcher.tracking.len();
    edge.tell(
        WEST,
        delta(
            10,
            vec![RegionEvent::EntityMoved {
                entity: RIDER,
                pose: Pose::at(Vec3::new(SPAWN.x + 0.06, SPAWN.y, SPAWN.z)),
                previous_chunk: HOME,
            }],
        ),
    );
    edge.quiet().await;
    watcher.drain();
    let first = positions_of(&watcher, tracking, rider_id).first().copied();
    assert_eq!(first, Some("entity_position_sync"));
}
