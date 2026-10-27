//! Tests of a player who leaves the world on a mount (step N7h), written from
//! `docs/groundwork/n7h-logout-on-a-mount.md` (sections 3, 5 and 8 to 10, as sections 12
//! to 14 amend them), ADR-0020's stays and ADR-0028's arrivals, and the game's
//! `PlayerList.remove` (`hasExactlyOnePlayerPassenger`, `saveParentVehicle`,
//! `Entity.RemovalReason.UNLOADED_WITH_PLAYER`), by someone who did not write the code.
//! Regions are driven by their public interface alone, and given what the world store
//! would say.
//!
//! What the design asks of a region:
//!
//! - a leave of a player seated in a unit in which they are the only player, seated or
//!   pending, takes every member out of the world, gone from the root's chunk at its
//!   hops, and the note of the leave carries the unit and the place the vehicle would let
//!   them off at; otherwise (two players, a pending rider, a pending leaver, a stay that
//!   ends any other way) they are put off and the unit stays;
//! - notes of two stays of one player in one tick are each kept, a removal before an
//!   `Entering`;
//! - a take-in drops a player's seat whose player is not there riding it;
//! - the home region told of a unit with a stay that enters takes it (a `Took`), sends it
//!   back to its root's chunk as arrivals at the hops it left with, its seat naming the
//!   new stay and pending, and the stay waits for it, riding; taken, the unit is one hop
//!   higher and the player seated.
//!
//! The scene of `n6_scene`: a stone floor at y = -61, one region over the land near the
//! origin, or the west of a pinned line at block x = 16 with another region beyond.

mod n6_scene;

use clustine_data::{entity_types, items};
use clustine_sim::api::{EnderChest, Face, Inventory, PlayerEvent, PlayerInput, Pose, RegionEvent};
use clustine_sim::{
    Durable, Entered, Holdings, Place, PlayerChange, PlayerJoin, PlayerTransfer, Region, StayNote,
    TickInputs, TickOutput, Ticket, Unit,
};
use clustine_world::{
    Arrival, ChunkPos, Due, EffectId, Entity, EntityData, EntityId, Float, Hurt, Pending, PlayerId,
    RegionId, Seat, Vec3,
};
use n6_scene::*;
use uuid::Uuid;

/// The ticks a seat is kept for a player who is to come, and that a player waits for
/// their unit (`PENDING_FOR`, section 5).
const PENDING_FOR: u16 = 100;

/// Where a vehicle stands: within reach of the spawn, in the home chunk.
const VEHICLE_AT: [f64; 3] = [2.5, -60.0, 1.5];

/// The vehicles' numbers.
const VEHICLE: u32 = 1;
const PIG: u32 = 2;

// ------------------------------------------------------------------------------------
// The vehicles.

/// An oak boat standing on the floor at `position`, carrying nobody.
fn boat(n: u32, position: [f64; 3]) -> Entity {
    Entity {
        key: key(n),
        kind: u16::try_from(entity_types::OAK_BOAT).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 7_000),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            // `ON_LAND` both, by `AbstractBoat.Status`'s ordinals.
            status: [3, 3],
            contents: Vec::new(),
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// A minecart standing on the floor at `position`.
fn cart(n: u32, position: [f64; 3]) -> Entity {
    let mut cart = boat(n, position);
    cart.kind = u16::try_from(entity_types::MINECART).expect("fits");
    cart.data = EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents: Vec::new(),
        fuel: 0,
        push: [clustine_world::Double(0.0); 2],
        fuse: -1,
        enabled: true,
    };
    cart
}

/// A mob of the game's own, hatched by `egg` on the floor of a scratch region, and
/// saddled there by a saddle's use where `saddled`; set as entity `n` at `at`.
fn hatched(egg: i32, saddled: bool, n: u32, at: [f64; 3]) -> Entity {
    let mut scene = Scene::alone(
        &World::default(),
        inventory(&[(0, stack(egg, 1)), (1, stack(items::SADDLE, 1))]),
        SPAWN,
    );
    scene.select(0);
    scene.click(bp(2, -61, 2), Face::Top);
    let made = scene
        .all()
        .into_iter()
        .find(|entity| entity.kind != u16::try_from(entity_types::PLAYER).expect("fits"))
        .expect("the egg makes a mob");
    if saddled {
        scene.select(1);
        scene.input(used(made.key));
    }
    let mut mob = scene
        .all()
        .into_iter()
        .find(|entity| entity.key == made.key)
        .expect("there");
    if saddled {
        assert_ne!(mob.data, made.data, "the saddle went on");
    }
    mob.key = key(n);
    mob.uuid = Uuid::from_u128(u128::from(n) + 7_000);
    mob.position = at;
    mob.motion = [0.0; 3];
    mob.on_ground = true;
    mob.fall_distance = 0.0;
    mob.riding = Default::default();
    mob
}

fn saddled_pig(n: u32, at: [f64; 3]) -> Entity {
    hatched(items::PIG_SPAWN_EGG, true, n, at)
}

/// A boat with a pig sitting in it, as the game's boat takes a mob it bumps into.
fn boat_with_a_pig() -> Vec<Entity> {
    let mut boat = boat(VEHICLE, VEHICLE_AT);
    let mut pig = hatched(items::PIG_SPAWN_EGG, false, PIG, VEHICLE_AT);
    boat.riding.passengers = vec![Seat {
        key: pig.key,
        player: false,
        pending: None,
    }];
    pig.riding.vehicle = Some(boat.key);
    vec![boat, pig]
}

// ------------------------------------------------------------------------------------
// Inputs and the store's words.

/// A use of `key` with what the main hand holds, not sneaking.
fn used(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    }
}

/// A scene with `entities` in the world and player 1 at the spawn, holding nothing,
/// seated in `vehicle` by a click on it.
fn seated_in(entities: &[Entity], vehicle: EntityId) -> Scene {
    let world = World::default().with_entities(entities);
    let mut scene = Scene::alone(&world, inventory(&[]), SPAWN);
    scene.input(used(vehicle));
    assert_eq!(
        rides(&scene, 0),
        Some(vehicle),
        "player 1 rides {vehicle:?}"
    );
    scene.idle();
    scene
}

/// The leave of the actor `who`'s stay, naming its entity, as an edge sends it.
fn leave_of(scene: &Scene, who: usize) -> PlayerChange {
    let actor = &scene.actors[who];
    PlayerChange::Leave(EDGE, actor.id, Some(actor.entity), None)
}

fn leaves(scene: &mut Scene, who: usize) -> TickOutput {
    let change = leave_of(scene, who);
    scene.region.tick(&TickInputs {
        player_changes: vec![change],
        ..TickInputs::default()
    })
}

fn join_of(n: u128, attempt: u64) -> PlayerChange {
    PlayerChange::Join(
        EDGE,
        PlayerJoin {
            player: player(n),
            name: format!("Player{n}"),
            attempt,
        },
    )
}

/// The place a player stands at `at`, holding nothing.
fn place_at(at: [f64; 3]) -> Place {
    Place {
        respawn: None,
        experience: Default::default(),
        pose: Pose {
            position: Vec3::new(at[0], at[1], at[2]),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
        },
        flying: false,
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: EnderChest::default(),
        effects: Vec::new(),
        time_since_rest: 0,
    }
}

/// Player `n` joins the region, which holds the new stay as entering, and returns its
/// entity.
fn joins(region: &mut Region, n: u128, attempt: u64) -> EntityId {
    region.tick(&TickInputs {
        player_changes: vec![join_of(n, attempt)],
        ..TickInputs::default()
    });
    region
        .entering_state(player(n))
        .expect("the join is held as entering")
        .entity_id
}

/// The store's answer to the stay `entity` of player `n`.
fn entered(
    n: u128,
    entity: EntityId,
    place: Place,
    unit: Unit,
    holder: Option<RegionId>,
) -> Entered {
    Entered {
        player: player(n),
        entity,
        place: Some(place),
        holder,
        unit: Some(unit),
    }
}

/// The arrivals `output` sent, as the store files them under `sequence`, numbered in
/// their order.
fn filed(output: &TickOutput, sequence: u64) -> Vec<Arrival> {
    output
        .arrivals
        .iter()
        .enumerate()
        .map(|(index, (chunk, entity))| Arrival {
            id: EffectId {
                sequence,
                index: u32::try_from(index).expect("fits"),
            },
            chunk: *chunk,
            entity: entity.clone(),
            due: Due::FromTaking,
        })
        .collect()
}

// ------------------------------------------------------------------------------------
// What the region says.

fn rides(scene: &Scene, who: usize) -> Option<EntityId> {
    scene
        .region
        .player_state(scene.actors[who].id)
        .expect("there")
        .riding
        .map(|riding| riding.vehicle)
}

/// The one record of `key` among the region's loaded chunks.
fn record(region: &Region, key: EntityId) -> Option<Entity> {
    let mut found = Vec::new();
    for chunk in region.loaded_chunks() {
        for entity in region.chunk(chunk).expect("loaded").entities() {
            if entity.key == key {
                found.push(entity.clone());
            }
        }
    }
    assert!(found.len() <= 1, "{key:?} is there twice: {found:?}");
    found.into_iter().next()
}

fn player_seats(entity: &Entity) -> Vec<Seat> {
    entity
        .riding
        .passengers
        .iter()
        .filter(|seat| seat.player)
        .cloned()
        .collect()
}

/// The `Has` notes of `p` in `output`: the stay, its hops, the place and the unit.
fn has_notes(output: &TickOutput, p: PlayerId) -> Vec<(EntityId, u32, Place, Option<Unit>)> {
    output
        .stays
        .iter()
        .filter_map(|note| match note {
            StayNote::Has {
                player,
                entity,
                hops,
                place,
                unit,
                ..
            } if *player == p => Some((*entity, *hops, place.clone(), unit.clone())),
            _ => None,
        })
        .collect()
}

/// The units of `p`'s notes in `output`.
fn units_noted(output: &TickOutput, p: PlayerId) -> Vec<Unit> {
    has_notes(output, p)
        .into_iter()
        .filter_map(|(_, _, _, unit)| unit)
        .collect()
}

fn takings(output: &TickOutput) -> Vec<(PlayerId, EntityId)> {
    output
        .stays
        .iter()
        .filter_map(|note| match note {
            StayNote::Took { player, root } => Some((*player, *root)),
            _ => None,
        })
        .collect()
}

fn gone_reported(output: &TickOutput, key: EntityId) -> Vec<u32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityGone { entity, hops, .. } if *entity == key => Some(*hops),
            _ => None,
        })
        .collect()
}

fn handed_on(output: &TickOutput, key: EntityId) -> bool {
    output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::HandedOn { entity, .. } if *entity == key))
}

fn departures(output: &TickOutput, p: PlayerId) -> Vec<(PlayerTransfer, RegionId)> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, durable)| match durable {
            Durable::Departed {
                player,
                transfer,
                to,
            } if *player == p => Some((transfer.clone(), *to)),
            _ => None,
        })
        .collect()
}

fn across(a: Vec3, b: [f64; 3]) -> f64 {
    (a.x - b[0]).hypot(a.z - b[2])
}

/// What a leave alone on `vehicles` (root first) asks of the region: every member gone
/// from the root's chunk at its hops, reported as a removal and not a hand-over; the
/// leave's note carrying the unit, root first, with the player's seat as it was, seated
/// and naming the leaving stay; its place beside the vehicle and not the seat; then the
/// stay's `Gone`. Returns the note's place and unit.
fn assert_taken_with_them(scene: &mut Scene, vehicles: &[EntityId]) -> (Place, Unit) {
    let p = scene.actors[0].id;
    let rider = scene.actors[0].entity;
    let root_before = record(&scene.region, vehicles[0]).expect("the vehicle");
    let seat_pose = scene.region.player(p).expect("there").1.position;
    let hops: Vec<u32> = vehicles
        .iter()
        .map(|key| record(&scene.region, *key).expect("a member").hops)
        .collect();
    let output = leaves(scene, 0);
    assert!(scene.region.player(p).is_none(), "the player left");
    for (member, h) in vehicles.iter().zip(&hops) {
        assert!(
            record(&scene.region, *member).is_none(),
            "{member:?} left the world with its rider"
        );
        assert!(
            output.entity_changes.contains(&(
                root_before.chunk(),
                clustine_world::EntityChange::Gone(*member)
            )),
            "{member:?} gone from the root's chunk in the leave's commit: {:?}",
            output.entity_changes
        );
        assert_eq!(gone_reported(&output, *member), [*h], "{member:?}");
        assert!(!handed_on(&output, *member), "a removal, not handed on");
    }
    let notes = has_notes(&output, p);
    let [(stay, _, place, Some(unit))] = &notes[..] else {
        panic!("one note, with the unit: {notes:?}");
    };
    assert_eq!(*stay, rider);
    let keys: Vec<EntityId> = unit.0.iter().map(|member| member.key).collect();
    assert_eq!(keys, vehicles, "the members, root first");
    for (member, h) in unit.0.iter().zip(&hops) {
        assert_eq!(member.hops, *h, "the unit keeps its hops");
    }
    let seat = unit.0.iter().flat_map(player_seats).collect::<Vec<_>>();
    assert_eq!(
        seat,
        [Seat {
            key: rider,
            player: true,
            pending: None
        }],
        "the seat stays as it is: seated, naming the leaving stay"
    );
    assert!(
        (place.pose.position.x - seat_pose.x).abs() > 1e-3
            || (place.pose.position.y - seat_pose.y).abs() > 1e-3
            || (place.pose.position.z - seat_pose.z).abs() > 1e-3,
        "the place is where they would get off, not the seat: {:?}",
        place.pose
    );
    assert!(
        across(place.pose.position, root_before.position) < 3.0,
        "beside the vehicle: {:?} by {:?}",
        place.pose.position,
        root_before.position
    );
    let has_at = output
        .stays
        .iter()
        .position(|note| matches!(note, StayNote::Has { player, .. } if *player == p))
        .expect("the note");
    let gone_at = output
        .stays
        .iter()
        .position(|note| matches!(note, StayNote::Gone { player, .. } if *player == p))
        .expect("then `Gone`, as now");
    assert!(has_at < gone_at, "{:?}", output.stays);
    (place.clone(), unit.clone())
}

// ------------------------------------------------------------------------------------
// Section 3: who takes the unit.

/// `hasExactlyOnePlayerPassenger`: a player alone in a boat takes it out of the world.
#[test]
fn a_player_alone_in_a_boat_takes_it_out_of_the_world_when_they_leave() {
    let mut scene = seated_in(&[boat(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    assert_taken_with_them(&mut scene, &[key(VEHICLE)]);
}

/// The same of a saddled pig ridden alone (the probe of section 1).
#[test]
fn a_player_alone_on_a_saddled_pig_takes_it_out_of_the_world_when_they_leave() {
    let mut scene = seated_in(&[saddled_pig(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    assert_taken_with_them(&mut scene, &[key(VEHICLE)]);
}

/// "Leave on a pig, a cart, or a boat with a pig behind you, and it goes with you": a
/// minecart.
#[test]
fn a_player_alone_in_a_minecart_takes_it_out_of_the_world_when_they_leave() {
    let mut scene = seated_in(&[cart(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    assert_taken_with_them(&mut scene, &[key(VEHICLE)]);
}

/// "For every root vehicle": a horse, mounted by an empty hand, ridden alone.
#[test]
fn a_player_alone_on_a_horse_takes_it_out_of_the_world_when_they_leave() {
    let horse = hatched(items::HORSE_SPAWN_EGG, false, VEHICLE, VEHICLE_AT);
    let world = World::default().with_entities(&[horse]);
    let mut scene = Scene::alone(&world, inventory(&[]), SPAWN);
    scene.input(used(key(VEHICLE)));
    assert_eq!(rides(&scene, 0), Some(key(VEHICLE)), "on the horse");
    assert_taken_with_them(&mut scene, &[key(VEHICLE)]);
}

/// "Seats, over every member, hold exactly one player": a boat with a pig behind the
/// player goes whole, the pig with it, root first.
#[test]
fn a_player_in_a_boat_with_a_pig_behind_takes_both_out_of_the_world() {
    let mut scene = seated_in(&boat_with_a_pig(), key(VEHICLE));
    assert_eq!(
        record(&scene.region, key(VEHICLE))
            .expect("the boat")
            .riding
            .passengers
            .len(),
        2,
        "the pig and the player"
    );
    let (_, unit) = assert_taken_with_them(&mut scene, &[key(VEHICLE), key(PIG)]);
    assert_eq!(
        unit.0[1].riding.vehicle,
        Some(key(VEHICLE)),
        "the pig rides on"
    );
}

/// "Otherwise (two players ...) the N5 amendment stands: noted at the dismount pose, the
/// unit left." Of two players in a boat the one who leaves is put off, and the boat
/// stays with the other in it; then the other, alone now, takes it.
#[test]
fn of_two_players_in_a_boat_the_one_who_leaves_is_put_off_and_the_boat_stays() {
    let mut scene = seated_in(&[boat(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    let second = scene.join(2, Vec3::new(1.5, -60.0, 2.5));
    scene.input_of(second, used(key(VEHICLE)));
    assert_eq!(rides(&scene, second), Some(key(VEHICLE)));
    let output = leaves(&mut scene, 0);
    assert!(units_noted(&output, player(1)).is_empty(), "no unit taken");
    let notes = has_notes(&output, player(1));
    assert_eq!(notes.len(), 1, "the leave's note: {notes:?}");
    assert!(
        across(notes[0].2.pose.position, VEHICLE_AT) < 3.0,
        "put off beside the boat: {:?}",
        notes[0].2.pose
    );
    let boat = record(&scene.region, key(VEHICLE)).expect("the boat stays");
    assert_eq!(
        player_seats(&boat)
            .iter()
            .map(|seat| seat.key)
            .collect::<Vec<_>>(),
        [scene.actors[second].entity]
    );
    assert!(gone_reported(&output, key(VEHICLE)).is_empty());
    assert_eq!(rides(&scene, second), Some(key(VEHICLE)));

    // Player 2 is alone in it now, and takes it.
    let output = leaves(&mut scene, second);
    assert_eq!(units_noted(&output, player(2)).len(), 1);
    assert!(record(&scene.region, key(VEHICLE)).is_none());
}

/// "A pending second rider": a seat kept for a player on their way counts as a player
/// aboard, so the one seated is put off and the boat stays with the seat kept.
#[test]
fn a_player_in_a_boat_with_a_seat_kept_for_another_is_put_off_and_the_boat_stays() {
    let mut kept = boat(VEHICLE, VEHICLE_AT);
    kept.riding.passengers = vec![Seat {
        key: EntityId(5 << 32 | 77),
        player: true,
        pending: Some(Pending {
            granted: false,
            left: 80,
            edge: EDGE,
        }),
    }];
    let mut scene = seated_in(&[kept], key(VEHICLE));
    let output = leaves(&mut scene, 0);
    assert!(units_noted(&output, player(1)).is_empty(), "no unit taken");
    let boat = record(&scene.region, key(VEHICLE)).expect("the boat stays");
    let seats = player_seats(&boat);
    assert_eq!(seats.len(), 1, "the kept seat alone: {seats:?}");
    assert_eq!(seats[0].key, EntityId(5 << 32 | 77));
    assert!(seats[0].pending.is_some());
}

/// "A pending leaver": a player waiting for their unit, not seated in it, takes nothing
/// when they leave.
#[test]
fn a_player_who_leaves_while_waiting_for_their_unit_takes_nothing() {
    let (mut scene, back) = put_back_in_the_home_region();
    let state = scene.region.player_state(player(2)).expect("entered");
    assert!(state.riding.is_some_and(|riding| riding.pending.is_some()));
    let output = scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(EDGE, player(2), Some(back.stay), None)],
        ..TickInputs::default()
    });
    assert!(scene.region.player(player(2)).is_none());
    assert!(
        units_noted(&output, player(2)).is_empty(),
        "{:?}",
        output.stays
    );
    assert!(takings(&output).is_empty());
}

// ------------------------------------------------------------------------------------
// `end_player`'s paths take no unit.

/// `apply_dead`: a seated stay the store says is dead is put off, and the boat stays,
/// empty.
#[test]
fn a_seated_stay_the_store_says_is_dead_takes_no_unit() {
    let mut scene = seated_in(&[boat(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    let rider = scene.actors[0].entity;
    let output = scene.region.tick(&TickInputs {
        dead: vec![(player(1), EntityId(rider.0 + 1), 0)],
        ..TickInputs::default()
    });
    assert!(scene.region.player(player(1)).is_none(), "the stay ended");
    assert!(units_noted(&output, player(1)).is_empty());
    let boat = record(&scene.region, key(VEHICLE)).expect("the boat stays");
    assert!(player_seats(&boat).is_empty(), "empty: {:?}", boat.riding);
    assert!(gone_reported(&output, key(VEHICLE)).is_empty());
}

/// A join over a seated stay ends it and takes no unit: the boat stays, empty, and the
/// new stay, told of no unit, does not ride ("a second login while the first rides does
/// not ride").
#[test]
fn a_join_over_a_seated_stay_takes_no_unit_and_the_new_stay_does_not_ride() {
    let mut scene = seated_in(&[boat(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    let output = scene.region.tick(&TickInputs {
        player_changes: vec![join_of(1, 8_001)],
        ..TickInputs::default()
    });
    assert!(units_noted(&output, player(1)).is_empty());
    let boat = record(&scene.region, key(VEHICLE)).expect("the boat stays");
    assert!(player_seats(&boat).is_empty(), "empty: {:?}", boat.riding);
    let entering = scene
        .region
        .entering_state(player(1))
        .expect("the new stay enters")
        .entity_id;
    scene.region.tick(&TickInputs {
        entered: vec![Entered {
            player: player(1),
            entity: entering,
            place: Some(place_at([0.5, -60.0, 0.5])),
            holder: None,
            unit: None,
        }],
        ..TickInputs::default()
    });
    let state = scene.region.player_state(player(1)).expect("entered");
    assert_eq!(state.entity_id, entering);
    assert_eq!(state.riding, None);
}

/// A player handed over in their next stay, as an arrival from another region, takes
/// the place of a stay of theirs that sits in a boat. That ends the seated stay, which
/// takes no unit, and the two notes are kept apart (section 13 item 2): the ended stay's,
/// put off, and the arrival's, with its own hops, which it used to replace.
#[test]
fn an_arrival_over_a_seated_stay_takes_no_unit_and_keeps_both_notes() {
    let mut scene = seated_in(&[boat(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    let rider = scene.actors[0].entity;
    let later = EntityId(rider.0 + 1_000);
    let mut transfer = walker(later, Vec3::new(-1.5, -60.0, -1.5));
    transfer.hops = 4;
    let output = scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Arrive(EDGE, player(1), transfer)],
        ..TickInputs::default()
    });
    assert_eq!(
        scene.region.player(player(1)).map(|(entity, _)| entity),
        Some(later),
        "the later stay is the region's"
    );
    let notes = has_notes(&output, player(1));
    let of = |stay: EntityId| notes.iter().filter(|note| note.0 == stay).count();
    assert_eq!(of(later), 1, "the arrival's note: {notes:?}");
    assert_eq!(of(rider), 1, "the ended stay's note: {notes:?}");
    let arrival = notes.iter().find(|note| note.0 == later).expect("noted");
    assert_eq!(arrival.1, 4, "with its own hops");
    assert!(
        across(arrival.2.pose.position, [-1.5, -60.0, -1.5]) < 1e-6,
        "and its own place: {:?}",
        arrival.2.pose
    );
    assert!(units_noted(&output, player(1)).is_empty());
    let boat = record(&scene.region, key(VEHICLE)).expect("the boat stays");
    assert!(player_seats(&boat).is_empty());
}

/// Player 1, walking, handed over in the stay `entity` at `at`.
fn walker(entity: EntityId, at: Vec3) -> PlayerTransfer {
    PlayerTransfer {
        sleeping: None,
        respawn: None,
        experience: Default::default(),
        attack_ticks: None,
        using: None,
        cooldowns: Vec::new(),
        effects: Vec::new(),
        time_since_rest: 0,
        entity_id: entity,
        name: "Player1".to_owned(),
        pose: Pose::at(at),
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: Default::default(),
        last_input: 0,
        hops: 1,
        flying: false,
        attempt: None,
        riding: None,
        keys: Default::default(),
        kept: Vec::new(),
    }
}

// ------------------------------------------------------------------------------------
// Two stays of one player in one tick.

/// Section 8 rows 1a and 7: a leave on a boat and a join of the same player applied in
/// one tick keep both notes, the removal (with the unit) before the `Entering`, so that
/// the store puts the unit in the record and then tells the new stay of it.
#[test]
fn a_leave_on_a_boat_and_a_join_in_one_tick_note_the_removal_before_the_entering() {
    let mut scene = seated_in(&[boat(VEHICLE, VEHICLE_AT)], key(VEHICLE));
    let leave = leave_of(&scene, 0);
    let rider = scene.actors[0].entity;
    let output = scene.region.tick(&TickInputs {
        player_changes: vec![leave, join_of(1, 8_002)],
        ..TickInputs::default()
    });
    let removal = output
        .stays
        .iter()
        .position(
            |note| matches!(note, StayNote::Has { entity, unit: Some(_), .. } if *entity == rider),
        )
        .unwrap_or_else(|| panic!("the removal with the boat: {:?}", output.stays));
    let entering = scene
        .region
        .entering_state(player(1))
        .expect("the join is entering")
        .entity_id;
    let entering_at = output
        .stays
        .iter()
        .position(|note| {
            matches!(note, StayNote::Entering { player: who, entity } if *who == player(1) && *entity == entering)
        })
        .unwrap_or_else(|| panic!("the `Entering`: {:?}", output.stays));
    assert!(removal < entering_at, "{:?}", output.stays);
    assert!(record(&scene.region, key(VEHICLE)).is_none(), "taken");
}

// ------------------------------------------------------------------------------------
// The stale seat at a take-in.

/// "Take-ins drop a stale seat": an arrival of a boat whose record names a seated player
/// the region does not have arrives without that seat; a pig's seat stays.
#[test]
fn an_arrival_naming_a_seated_player_who_is_not_there_arrives_without_them() {
    let mut entities = boat_with_a_pig();
    entities[0].riding.passengers.insert(
        0,
        Seat {
            key: EntityId(1 << 32 | 77),
            player: true,
            pending: None,
        },
    );
    let world = World::default();
    let mut scene = Scene::alone(&world, inventory(&[]), SPAWN);
    let arrivals = entities
        .iter()
        .enumerate()
        .map(|(index, entity)| Arrival {
            id: EffectId {
                sequence: 40,
                index: u32::try_from(index).expect("fits"),
            },
            chunk: entities[0].chunk(),
            entity: entity.clone(),
            due: Due::FromTaking,
        })
        .collect();
    let output = scene.region.tick(&TickInputs {
        arrivals,
        ..TickInputs::default()
    });
    assert_eq!(output.arrivals_taken.len(), 2, "both taken");
    let boat = record(&scene.region, key(VEHICLE)).expect("the boat came");
    assert!(player_seats(&boat).is_empty(), "{:?}", boat.riding);
    assert_eq!(
        boat.riding
            .passengers
            .iter()
            .map(|seat| seat.key)
            .collect::<Vec<_>>(),
        [key(PIG)],
        "the pig sits on"
    );
}

// ------------------------------------------------------------------------------------
// Section 5: re-entry.

/// What a test of a re-entry has: the unit as the leave noted it, the place, and the new
/// stay and the tick that took the unit back.
struct Back {
    left: Unit,
    place: Place,
    stay: EntityId,
    output: TickOutput,
}

/// Player 2 rides the boat alone in the home region and leaves on it; joins again and
/// is told of the unit with no holder (the root's chunk is the home region's own).
fn put_back_in_the_home_region() -> (Scene, Back) {
    let world = World::default().with_entities(&[boat(VEHICLE, VEHICLE_AT)]);
    let mut scene = Scene::alone(&world, inventory(&[]), SPAWN);
    let second = scene.join(2, Vec3::new(1.5, -60.0, 0.5));
    scene.input_of(second, used(key(VEHICLE)));
    assert_eq!(rides(&scene, second), Some(key(VEHICLE)));
    let output = leaves(&mut scene, second);
    let (_, _, place, unit) = has_notes(&output, player(2))
        .into_iter()
        .find(|note| note.3.is_some())
        .expect("taken");
    let left = unit.expect("the unit");
    let stay = joins(&mut scene.region, 2, 9_002);
    let output = scene.region.tick(&TickInputs {
        entered: vec![entered(2, stay, place.clone(), left.clone(), None)],
        ..TickInputs::default()
    });
    (
        scene,
        Back {
            left,
            place,
            stay,
            output,
        },
    )
}

/// Steps 1 to 4 where the root's chunk is the home region's own and loaded: the taking
/// in the tick's commit; the unit sent back as an arrival to its root's chunk at the hops
/// it left with (section 14 item 3: always an arrival), its seat naming the new stay and
/// pending for `PENDING_FOR` ticks with the stay's edge; the stay placed in the home
/// region at the seat, riding the root, waiting, and its client told what it rides.
/// Taken, the unit is one hop higher and the player seated.
#[test]
fn a_unit_told_with_the_next_stay_is_taken_and_comes_back_under_them() {
    let (mut scene, back) = put_back_in_the_home_region();
    let root = back.left.root().clone();
    let output = &back.output;
    assert_eq!(
        takings(output),
        [(player(2), root.key)],
        "{:?}",
        output.stays
    );
    let sent: Vec<(ChunkPos, EntityId, u32)> = output
        .arrivals
        .iter()
        .map(|(chunk, entity)| (*chunk, entity.key, entity.hops))
        .collect();
    assert_eq!(
        sent,
        [(root.chunk(), root.key, root.hops)],
        "an arrival at h"
    );
    let (_, boat_back) = &output.arrivals[0];
    assert_eq!(
        player_seats(boat_back),
        [Seat {
            key: back.stay,
            player: true,
            pending: Some(Pending {
                granted: false,
                left: PENDING_FOR,
                edge: EDGE,
            }),
        }],
        "the seat rewritten to the new stay, pending"
    );
    assert!(departures(output, player(2)).is_empty(), "placed here");
    let state = scene.region.player_state(player(2)).expect("placed");
    assert_eq!(state.entity_id, back.stay);
    let riding = state.riding.expect("riding the root");
    assert_eq!(riding.vehicle, root.key);
    assert_eq!(riding.chunk, root.chunk());
    // Counted as an age from the tick that sent it: the tick itself may have counted one.
    assert!(
        riding
            .pending
            .is_some_and(|left| (PENDING_FOR - 1..=PENDING_FOR).contains(&left)),
        "waiting for `PENDING_FOR` ticks: {:?}",
        riding.pending
    );
    // Waiting where they got off, beside the unit, not in its seat in the air: the
    // lead's amendment of section 5 (section 14 item 5), which section 13 item 4's
    // lapse asks for, the place a lapse leaves them at being the one they wait at.
    assert!(
        across(state.pose.position, root.position) < 2.0
            && state.pose.position.y >= root.position[1],
        "beside the unit: {:?} by {:?}",
        state.pose.position,
        root.position
    );
    let spawned = output
        .player_events
        .iter()
        .find_map(|(who, event)| match event {
            PlayerEvent::Spawned { riding, .. } if *who == player(2) => Some(*riding),
            _ => None,
        });
    assert_eq!(spawned, Some(Some(root.key)), "told what it rides");
    assert!(
        record(&scene.region, root.key).is_none(),
        "not made whole at once"
    );

    let arrivals = filed(output, 900);
    let taking = scene.region.tick(&TickInputs {
        arrivals,
        ..TickInputs::default()
    });
    assert_eq!(taking.arrivals_taken.len(), 1);
    scene.idle();
    let boat = record(&scene.region, root.key).expect("the boat is back");
    assert_eq!(
        boat.hops,
        root.hops + 1,
        "one higher than its leave's removal"
    );
    assert_eq!(
        player_seats(&boat),
        [Seat {
            key: back.stay,
            player: true,
            pending: None,
        }]
    );
    let riding = scene
        .region
        .player_state(player(2))
        .and_then(|state| state.riding)
        .expect("riding");
    assert_eq!((riding.vehicle, riding.pending), (root.key, None), "seated");
    assert!(takings(&taking).is_empty(), "taken once");
}

/// Step 4, `holder` X: the root's chunk is another region's, so the stay is let go to it
/// unplaced, with one hand-over and the join's attempt, riding the root pending, in the
/// same commit as the unit's arrival there and the taking.
#[test]
fn a_unit_in_another_regions_chunk_goes_there_with_the_player_in_one_commit() {
    let mut region = region_of(&Layout::side(true), &World::default(), inventory(&[]));
    // Player 2 left a boat east of the line, in the other region's land, seated in it.
    let mut left = boat(VEHICLE, [20.5, -60.0, 8.5]);
    left.hops = 3;
    left.riding.passengers = vec![Seat {
        key: EntityId(1 << 32 | 900),
        player: true,
        pending: None,
    }];
    let unit = Unit(vec![left.clone()]);
    let stay = joins(&mut region, 2, 9_003);
    let output = region.tick(&TickInputs {
        entered: vec![entered(
            2,
            stay,
            place_at([19.5, -60.0, 8.5]),
            unit,
            Some(OTHER),
        )],
        ..TickInputs::default()
    });
    assert_eq!(takings(&output), [(player(2), left.key)]);
    let sent: Vec<(ChunkPos, EntityId, u32)> = output
        .arrivals
        .iter()
        .map(|(chunk, entity)| (*chunk, entity.key, entity.hops))
        .collect();
    assert_eq!(sent, [(ChunkPos::new(1, 0), left.key, 3)]);
    let seats = player_seats(&output.arrivals[0].1);
    assert_eq!(seats.len(), 1);
    assert_eq!(seats[0].key, stay, "the seat names the new stay");
    assert!(seats[0].pending.is_some(), "kept for them");
    let gone = departures(&output, player(2));
    let [(transfer, to)] = &gone[..] else {
        panic!("let go once: {gone:?}");
    };
    assert_eq!(*to, OTHER);
    assert_eq!(transfer.entity_id, stay);
    assert_eq!(transfer.hops, 1);
    assert_eq!(transfer.attempt, Some(9_003));
    let riding = transfer.riding.expect("riding the root");
    assert_eq!(riding.vehicle, left.key);
    assert_eq!(riding.chunk, ChunkPos::new(1, 0));
    assert!(riding.pending.is_some(), "waiting for it");
    assert!(region.player(player(2)).is_none(), "not placed here");
}

/// Step 4, the place below the world: the player at the spawn point, riding nothing;
/// the unit back to its chunk as an arrival with its seat dropped (row 15).
#[test]
fn a_unit_told_with_a_place_below_the_world_goes_back_empty_and_the_player_to_the_spawn() {
    let world = World::default();
    let mut scene = Scene::alone(&world, inventory(&[]), SPAWN);
    let mut left = boat(VEHICLE, [6.5, -60.0, 6.5]);
    left.riding.passengers = vec![Seat {
        key: EntityId(1 << 32 | 900),
        player: true,
        pending: None,
    }];
    let stay = joins(&mut scene.region, 2, 9_004);
    let output = scene.region.tick(&TickInputs {
        entered: vec![entered(
            2,
            stay,
            place_at([6.5, -100.0, 6.5]),
            Unit(vec![left.clone()]),
            None,
        )],
        ..TickInputs::default()
    });
    let state = scene.region.player_state(player(2)).expect("placed");
    assert_eq!(state.pose.position, SPAWN, "at the spawn point");
    assert_eq!(state.riding, None);
    assert_eq!(takings(&output), [(player(2), left.key)]);
    let sent: Vec<&Entity> = output
        .arrivals
        .iter()
        .filter(|(chunk, entity)| *chunk == left.chunk() && entity.key == left.key)
        .map(|(_, entity)| entity)
        .collect();
    let [boat_back] = sent[..] else {
        panic!("the boat back to its chunk: {:?}", output.arrivals);
    };
    assert!(player_seats(boat_back).is_empty(), "{:?}", boat_back.riding);
}

/// "An answer is taken only for the very stay the region still holds as entering":
/// passed over, it takes no unit, which stays in the record for the next login.
#[test]
fn an_answer_with_a_unit_for_a_stay_no_longer_entering_takes_nothing() {
    let world = World::default();
    let mut scene = Scene::alone(&world, inventory(&[]), SPAWN);
    let stay = joins(&mut scene.region, 2, 9_005);
    scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(EDGE, player(2), None, Some(9_005))],
        ..TickInputs::default()
    });
    assert!(scene.region.entering_state(player(2)).is_none());
    let output = scene.region.tick(&TickInputs {
        entered: vec![entered(
            2,
            stay,
            place_at([3.5, -60.0, 3.5]),
            Unit(vec![boat(VEHICLE, [3.5, -60.0, 3.5])]),
            None,
        )],
        ..TickInputs::default()
    });
    assert!(takings(&output).is_empty());
    assert!(output.arrivals.is_empty());
    assert!(scene.region.player(player(2)).is_none());
}

/// Section 13 item 4: where the wait lapses, the player is put at the record's place,
/// the dismount pose, and not left at the seat in the air.
#[test]
fn a_player_whose_unit_never_comes_is_put_at_the_place_they_got_off_at() {
    let (mut scene, back) = put_back_in_the_home_region();
    for _ in 0..=PENDING_FOR {
        scene.idle();
    }
    let state = scene.region.player_state(player(2)).expect("there");
    assert_eq!(state.riding, None, "the wait lapsed");
    let at = back.place.pose.position;
    assert!(
        across(state.pose.position, [at.x, at.y, at.z]) < 1e-6
            && (state.pose.position.y - at.y).abs() < 1e-6,
        "at the record's place: {:?}, not {:?}",
        state.pose.position,
        at
    );
}

/// Section 7: a home region restored from its state after the taking keeps the stay
/// waiting, and seats it when the unit comes; it does not take the unit again.
#[test]
fn a_home_region_restored_after_the_taking_seats_the_player_when_the_unit_comes() {
    let (scene, back) = put_back_in_the_home_region();
    let root = back.left.root().clone();
    let state = scene.region.state();
    let saved: Vec<(ChunkPos, clustine_world::Chunk)> = scene
        .region
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| scene.region.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    let layout = Layout::alone();
    let holdings: Holdings = layout.holdings();
    let tickets: Vec<(ChunkPos, Ticket)> = layout
        .loaded
        .iter()
        .map(|chunk| (*chunk, Ticket::Viewer))
        .collect();
    let mut restored = Region::restore_loaded(
        config(inventory(&[])),
        state,
        holdings,
        &tickets,
        saved,
        Vec::new(),
    );
    let first = restored.tick(&TickInputs::default());
    assert!(takings(&first).is_empty(), "taken once: {:?}", first.stays);
    assert!(first.arrivals.is_empty(), "sent once");
    let riding = restored
        .player_state(player(2))
        .and_then(|state| state.riding)
        .expect("still waiting");
    assert!(riding.pending.is_some());
    restored.tick(&TickInputs {
        arrivals: filed(&back.output, 900),
        ..TickInputs::default()
    });
    restored.tick(&TickInputs::default());
    let boat = record(&restored, root.key).expect("the boat is back");
    assert_eq!(boat.hops, root.hops + 1);
    let riding = restored
        .player_state(player(2))
        .and_then(|state| state.riding)
        .expect("riding");
    assert_eq!((riding.vehicle, riding.pending), (root.key, None), "seated");
}
