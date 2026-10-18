//! Tests of riding, entity collision and what vehicles and block-attached entities share
//! (step N5b, `docs/groundwork/n5-plan.md`; `docs/groundwork/n5-brief.md`, sections 0.18,
//! 1, 3.14, 3.18, 4.6, 4.9, 5.8 and 6.1). The numbers each test expects are worked out
//! here from the game's formulas, as its bytecode has them.
//!
//! Player 1 stands at the spawn, (0.5, -60, 0.5), on the stone floor whose top is y =
//! -60; only chunk (0, 0) ticks, its neighbours are loaded. The entities stand well away
//! from the player, and the items are never picked up.

use clustine_data::entity_types;
use clustine_world::{
    Arrival, Attached, Double, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData,
    EntityField, EntityUse, FiledEffect, Float, Hurt, Seat,
};

use super::arrivals::at_a_line;
use super::redstone::block_at;
use super::*;
use crate::region::attached::Breaker;
use crate::region::random_ticks::Random;
use crate::region::riding::{BOARDING_COOLDOWN, add_passenger};

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

/// An entity of the type `kind` at `position`, at rest, with `data`.
fn entity(n: u64, kind: i32, position: [f64; 3], data: EntityData) -> Entity {
    Entity {
        key: key(n),
        kind: kind as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        age: 1,
        data,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

fn cart_data(contents: Vec<(u8, ItemStack)>) -> EntityData {
    EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents,
        fuel: 0,
        push: [Double(0.0); 2],
    }
}

fn cart(n: u64, position: [f64; 3]) -> Entity {
    entity(n, entity_types::MINECART, position, cart_data(Vec::new()))
}

fn boat_data() -> EntityData {
    EntityData::Boat {
        hurt: Hurt::default(),
        paddles: [false; 2],
        bubble_time: 0,
        out_of_control: Float(0.0),
        turning: Float(0.0),
        status: [0; 2],
        contents: Vec::new(),
    }
}

fn boat(n: u64, position: [f64; 3]) -> Entity {
    entity(n, entity_types::OAK_BOAT, position, boat_data())
}

fn stand(n: u64, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::ARMOR_STAND,
        position,
        EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new([[Float(0.0); 3]; 6]),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    )
}

fn cushion(n: u64, position: [f64; 3], colour: u8) -> Entity {
    entity(
        n,
        entity_types::CUSHION,
        position,
        EntityData::Cushion {
            attached: Attached {
                block: crate::physics::containing(position),
                direction: 1,
            },
            colour,
        },
    )
}

fn item(n: u64, position: [f64; 3], motion: [f64; 3], on_ground: bool) -> Entity {
    Entity {
        motion,
        on_ground,
        ..entity(
            n,
            entity_types::ITEM,
            position,
            EntityData::Item {
                stack: DIRT,
                pick_up_delay: 32_767,
                thrower: None,
                health: 5,
            },
        )
    }
}

/// Lays `entity` in the loaded chunk `chunk`, as a load would have it.
fn lay_in(region: &mut Region, chunk: ChunkPos, entity: &Entity) {
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn lay(region: &mut Region, entity: &Entity) {
    lay_in(region, entity.chunk(), entity);
}

/// Lays `vehicle` and its `passengers`, riding it in that order, in the vehicle's chunk.
fn lay_unit(
    region: &mut Region,
    vehicle: &Entity,
    passengers: &[&Entity],
) -> (Entity, Vec<Entity>) {
    let mut vehicle = vehicle.clone();
    let chunk = vehicle.chunk();
    let mut riders = Vec::new();
    for passenger in passengers {
        let mut passenger = (*passenger).clone();
        passenger.riding.vehicle = Some(vehicle.key);
        vehicle.riding.passengers.push(Seat {
            key: passenger.key,
            player: false,
            pending: None,
        });
        lay_in(region, chunk, &passenger);
        riders.push(passenger);
    }
    lay_in(region, chunk, &vehicle);
    (vehicle, riders)
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn bits(values: [f64; 3]) -> [u64; 3] {
    values.map(f64::to_bits)
}

/// An item lying in a loaded chunk: its key, place, stack, pick-up delay and motion.
type Lying = (EntityId, [f64; 3], ItemStack, u16, [f64; 3]);

/// The items of the loaded chunks.
fn items_lying(region: &Region) -> Vec<Lying> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item {
                stack,
                pick_up_delay,
                ..
            } => Some((
                entity.key,
                entity.position,
                stack,
                pick_up_delay,
                entity.motion,
            )),
            _ => None,
        })
        .collect()
}

/// A hit on `target` from another region's player, as an effect at its block.
fn hit(target: &Entity, sequence: u64, damage: f32, creative: bool) -> TickInputs {
    TickInputs {
        effects: vec![FiledEffect {
            id: EffectId { sequence, index: 0 },
            effect: Effect {
                at: crate::physics::containing(target.position),
                what: Effected::OnEntity {
                    key: target.key,
                    by: EntityUse::Attack {
                        attacker: EntityId(1),
                        creative,
                        sprinting: false,
                        damage: Float(damage),
                        from: [Double(0.5), Double(-60.0), Double(0.5)],
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    }
}

// Entity collision.

#[test]
fn an_item_falling_onto_a_boat_lands_on_its_top_and_stays_there() {
    let mut region = on_ticking_floor_with(&[]);
    let raft = boat(1, [8.5, -60.0, 8.5]);
    lay(&mut region, &raft);
    let falling = item(2, [8.5, -58.0, 8.5], [0.0; 3], false);
    lay(&mut region, &falling);
    // The boat's box is 0.5625 high, a float widened.
    let top = -60.0 + f64::from(0.5625_f32);
    for _ in 0..40 {
        idle(&mut region);
    }
    let landed = record(&region, falling.key).unwrap();
    assert!(landed.on_ground, "{landed:?}");
    assert!((landed.position[1] - top).abs() < 1.0e-7, "{landed:?}");
    for _ in 0..40 {
        idle(&mut region);
        let lying = record(&region, falling.key).unwrap();
        assert!((lying.position[1] - top).abs() < 1.0e-7, "{lying:?}");
        assert!(lying.on_ground);
    }
}

#[test]
fn an_item_falls_through_a_minecart_which_is_not_solid_to_it() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cart(1, [8.5, -60.0, 8.5]));
    let falling = item(2, [8.5, -58.0, 8.5], [0.0; 3], false);
    lay(&mut region, &falling);
    for _ in 0..40 {
        idle(&mut region);
    }
    let landed = record(&region, falling.key).unwrap();
    assert!(landed.on_ground);
    assert_eq!(landed.position[1], -60.0, "{landed:?}");
}

#[test]
fn a_boat_and_a_cart_block_building_and_an_item_does_not() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &boat(1, [8.5, -60.0, 8.5]));
    lay(&mut region, &cart(2, [12.5, -60.0, 8.5]));
    lay(&mut region, &item(3, [4.5, -60.0, 8.5], [0.0; 3], true));
    let block = |x: i32| crate::physics::aabb::Aabb::of_block(BlockPos::new(x, -60, 8));
    assert!(!region.unobstructed_by_entities(&[block(8)], None));
    assert!(!region.unobstructed_by_entities(&[block(12)], None));
    assert!(region.unobstructed_by_entities(&[block(4)], None));
    // The block above the boat's top is free, and the boat itself does not block what
    // its own unit places.
    let above = crate::physics::aabb::Aabb::of_block(BlockPos::new(8, -59, 8));
    assert!(region.unobstructed_by_entities(&[above], None));
    assert!(region.unobstructed_by_entities(&[block(8)], Some(key(1))));
}

// The order of seats and the room in a vehicle.

#[test]
fn a_player_takes_the_first_seat_where_the_first_is_not_a_player_and_anyone_else_the_last() {
    let seat = |n: u64, player: bool| Seat {
        key: key(n),
        player,
        pending: None,
    };
    let mut seats = Vec::new();
    add_passenger(&mut seats, seat(1, false));
    add_passenger(&mut seats, seat(2, true));
    assert_eq!(seats, [seat(2, true), seat(1, false)]);
    // With a player first already, a second player sits behind.
    add_passenger(&mut seats, seat(3, true));
    assert_eq!(seats, [seat(2, true), seat(1, false), seat(3, true)]);
    // A first player in an empty vehicle simply sits.
    let mut alone = Vec::new();
    add_passenger(&mut alone, seat(4, true));
    add_passenger(&mut alone, seat(5, false));
    assert_eq!(alone, [seat(4, true), seat(5, false)]);
}

#[test]
fn a_cart_and_a_cushion_take_one_rider_a_boat_two_and_a_chest_boat_one() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let vehicles = [
        (cart(1, [2.5, -60.0, 8.5]), 1),
        (cushion(2, [4.5, -60.0, 8.5], 0), 1),
        (boat(3, [6.5, -60.0, 8.5]), 2),
        (
            entity(
                4,
                entity_types::OAK_CHEST_BOAT,
                [10.5, -60.0, 8.5],
                boat_data(),
            ),
            1,
        ),
    ];
    for (n, (vehicle, room)) in vehicles.iter().enumerate() {
        lay(&mut region, vehicle);
        for seat in 0..3 {
            let rider = stand(
                100 + 10 * n as u64 + seat,
                [vehicle.position[0], -60.0, 12.5],
            );
            lay(&mut region, &rider);
            let took = region.start_riding(rider.key, vehicle.key, false, &mut output);
            assert_eq!(took, seat < *room, "{:?} seat {seat}", vehicle.kind);
        }
    }
}

#[test]
fn nobody_boards_a_boat_whose_eyes_are_under_water() {
    let water = |x: i32, y: i32| (BlockPos::new(x, y, 8), blocks::WATER);
    let mut region = on_ticking_floor_with(&[water(8, -60), water(8, -59)]);
    let mut output = TickOutput::default();
    let sunk = boat(1, [8.5, -60.0, 8.5]);
    lay(&mut region, &sunk);
    let rider = stand(2, [4.5, -60.0, 12.5]);
    lay(&mut region, &rider);
    assert!(!region.start_riding(rider.key, sunk.key, false, &mut output));
    // Forced, the rule is not asked.
    assert!(region.start_riding(rider.key, sunk.key, true, &mut output));
}

// Getting on and off.

#[test]
fn a_rider_joins_the_vehicles_chunk_and_is_reported_seated() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let vehicle = cart(1, [15.5, -60.0, 8.5]);
    lay(&mut region, &vehicle);
    // A stand just over the line in chunk (1, 0).
    let rider = stand(2, [16.5, -60.0, 8.5]);
    lay(&mut region, &rider);
    assert!(region.start_riding(rider.key, vehicle.key, false, &mut output));
    assert_eq!(region.entity_chunks[&rider.key], ORIGIN);
    assert_eq!(
        record(&region, rider.key).unwrap().riding.vehicle,
        Some(vehicle.key)
    );
    assert_eq!(
        record(&region, vehicle.key).unwrap().riding.passengers,
        [Seat {
            key: rider.key,
            player: false,
            pending: None
        }]
    );
    assert!(
        output
            .entity_changes
            .contains(&(EAST, EntityChange::Gone(rider.key)))
    );
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Passengers { vehicle: v, chunk: ORIGIN, passengers, .. }
            if *v == vehicle.key && passengers == &[rider.key]
    )));
    // Nothing rides what rides it.
    assert!(!region.start_riding(vehicle.key, rider.key, true, &mut output));
}

#[test]
fn a_passenger_that_got_off_waits_sixty_ticks_before_it_rides_again() {
    let mut region = on_ticking_floor_with(&[]);
    let (vehicle, riders) = lay_unit(
        &mut region,
        &cart(1, [8.5, -60.0, 8.5]),
        &[&stand(2, [8.5, -60.0, 8.5])],
    );
    let rider = riders[0].key;
    let mut output = TickOutput::default();
    region.stop_riding(rider, &mut output);
    assert_eq!(
        record(&region, rider).unwrap().riding.cooldown,
        BOARDING_COOLDOWN
    );
    assert!(
        record(&region, vehicle.key)
            .unwrap()
            .riding
            .passengers
            .is_empty()
    );
    for tick in 0..60 {
        let mut output = TickOutput::default();
        assert!(
            !region.start_riding(rider, vehicle.key, false, &mut output),
            "tick {tick}"
        );
        idle(&mut region);
    }
    let mut output = TickOutput::default();
    assert!(region.start_riding(rider, vehicle.key, false, &mut output));
}

#[test]
fn ejecting_lets_the_last_passenger_off_first() {
    let mut region = on_ticking_floor_with(&[]);
    let (vehicle, riders) = lay_unit(
        &mut region,
        &boat(1, [8.5, -60.0, 8.5]),
        &[&stand(2, [8.5, -60.0, 8.5]), &stand(3, [8.5, -60.0, 8.5])],
    );
    let mut output = TickOutput::default();
    region.eject_passengers(vehicle.key, &mut output);
    let reported: Vec<Vec<EntityId>> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Passengers { passengers, .. } => Some(passengers.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reported, [vec![riders[0].key], vec![]]);
    for rider in &riders {
        assert_eq!(record(&region, rider.key).unwrap().riding.vehicle, None);
    }
}

// The tick of passengers and their seats.

#[test]
fn a_passenger_sits_at_its_vehicles_passenger_point_less_its_own_vehicle_point() {
    let mut region = on_ticking_floor_with(&[]);
    // A boat on a cart, the cart's own point 0.1875 up, a boat's vehicle point nothing.
    let (vehicle, riders) = lay_unit(
        &mut region,
        &cart(1, [8.5, -60.0, 8.5]),
        &[&boat(2, [3.0, -60.0, 3.0])],
    );
    idle(&mut region);
    let seated = record(&region, riders[0].key).unwrap();
    assert_eq!(
        bits(seated.position),
        bits([8.5, (-60.0 + 0.1875) - 0.0, 8.5]),
        "{seated:?}"
    );
    assert_eq!(region.entity_chunks[&seated.key], ORIGIN);
    assert_eq!(
        record(&region, vehicle.key).unwrap().position,
        vehicle.position
    );
}

/// The game's table of sines at an angle in radians, worked out here as the game fills
/// the table and looks it up.
fn table_sin(radians: f32) -> f64 {
    let index = (f64::from(radians) * 10_430.378_350_470_453) as i64 & 0xFFFF;
    f64::from(libm::sin(index as f64 / 10_430.378_350_470_453) as f32)
}

fn table_cos(radians: f32) -> f64 {
    let index = (f64::from(radians) * 10_430.378_350_470_453 + 16_384.0) as i64 & 0xFFFF;
    f64::from(libm::sin(index as f64 / 10_430.378_350_470_453) as f32)
}

#[test]
fn two_passengers_of_a_turned_boat_sit_two_tenths_ahead_and_six_tenths_behind() {
    let mut region = on_ticking_floor_with(&[]);
    let turned = Entity {
        yaw: 90.0,
        ..boat(1, [8.5, -60.0, 8.5])
    };
    let (_, riders) = lay_unit(
        &mut region,
        &turned,
        &[&stand(2, [3.0, -60.0, 3.0]), &stand(3, [3.0, -60.0, 3.0])],
    );
    idle(&mut region);
    // `Vec3(0, height / 3, along).yRot(-90 * DEG_TO_RAD)`: x' = along * sin, z' = along *
    // cos, the height a float third of 0.5625.
    let angle = -90.0_f32 * 0.017_453_292;
    let height = f64::from(0.5625_f32 / 3.0);
    for (rider, along) in riders.iter().zip([0.2_f32, -0.6]) {
        let along = f64::from(along);
        let expected = [
            8.5 + (0.0 * table_cos(angle) + along * table_sin(angle)),
            -60.0 + height,
            8.5 + (along * table_cos(angle) - 0.0 * table_sin(angle)),
        ];
        let seated = record(&region, rider.key).unwrap();
        assert_eq!(bits(seated.position), bits(expected), "{seated:?}");
    }
    // Facing west, the front seat is to the west.
    let front = record(&region, riders[0].key).unwrap();
    assert!((front.position[0] - 8.3).abs() < 1.0e-6, "{front:?}");
    // A rider faces within 105 degrees of the boat's way: one facing behind it turns.
    let back = record(&region, riders[1].key).unwrap();
    assert_eq!(back.yaw, 0.0);
}

#[test]
fn a_unit_ticks_root_first_and_each_passenger_before_its_own_passengers() {
    let mut region = on_ticking_floor_with(&[]);
    // A cart carries a boat, which carries a stand; the boat starts off its seat.
    let carrier = cart(1, [8.5, -60.0, 8.5]);
    let (_, riders) = lay_unit(&mut region, &carrier, &[&boat(2, [2.0, -60.0, 2.0])]);
    let mut on_boat = stand(3, [4.0, -60.0, 4.0]);
    on_boat.riding.vehicle = Some(riders[0].key);
    let mut middle = riders[0].clone();
    middle.riding.passengers.push(Seat {
        key: on_boat.key,
        player: false,
        pending: None,
    });
    lay_in(&mut region, ORIGIN, &middle);
    lay_in(&mut region, ORIGIN, &on_boat);
    idle(&mut region);
    // The boat is put on the cart first, and the stand on the boat where it is now, in
    // the same tick.
    let boat_at = [8.5, -60.0 + 0.1875, 8.5];
    assert_eq!(
        bits(record(&region, middle.key).unwrap().position),
        bits(boat_at)
    );
    let height = f64::from(0.5625_f32 / 3.0);
    let seated = record(&region, on_boat.key).unwrap();
    assert_eq!(
        seated.position[1].to_bits(),
        (boat_at[1] + height).to_bits()
    );
    assert_eq!(region.entity_chunks[&on_boat.key], ORIGIN);
}

#[test]
fn a_passenger_whose_vehicle_does_not_hold_it_gets_off_and_ticks_alone() {
    let mut region = on_ticking_floor_with(&[]);
    let carrier = cart(1, [8.5, -60.0, 8.5]);
    lay(&mut region, &carrier);
    let mut stray = stand(2, [4.5, -60.0, 4.5]);
    stray.riding.vehicle = Some(carrier.key);
    lay(&mut region, &stray);
    idle(&mut region);
    let alone = record(&region, stray.key).unwrap();
    assert_eq!(alone.riding.vehicle, None);
    assert_eq!(
        alone.riding.cooldown,
        BOARDING_COOLDOWN - 1,
        "it ticked after"
    );
}

// Getting off a vehicle.

#[test]
fn a_stand_gets_off_a_cart_beside_it_where_the_first_side_is_walled() {
    // A cart facing south (yaw 0) moves along x, the sides it lets off at north first,
    // then south (`offsetsForDirection` of west). Stone fills the block north of it.
    let mut region = on_ticking_floor_with(&[(BlockPos::new(8, -60, 7), blocks::STONE)]);
    let (_, riders) = lay_unit(
        &mut region,
        &cart(1, [8.5, -60.0, 8.5]),
        &[&stand(2, [8.5, -60.0, 8.5])],
    );
    let mut output = TickOutput::default();
    region.stop_riding(riders[0].key, &mut output);
    let off = record(&region, riders[0].key).unwrap();
    assert_eq!(bits(off.position), bits([8.5, -60.0, 9.5]), "{off:?}");
    assert!(output.entity_changes.contains(&(
        ORIGIN,
        EntityChange::Set(
            riders[0].key,
            vec![
                EntityField::Riding(off.riding.clone()),
                EntityField::Position([8.5, -60.0, 9.5]),
            ]
        )
    )));
}

#[test]
fn a_stand_gets_off_a_boat_to_the_side_it_faces_by_the_escape_vector() {
    let mut region = on_ticking_floor_with(&[]);
    let (_, riders) = lay_unit(
        &mut region,
        &boat(1, [8.5, -60.0, 8.5]),
        &[&stand(2, [8.5, -60.0, 8.5])],
    );
    let mut output = TickOutput::default();
    region.stop_riding(riders[0].key, &mut output);
    // Facing south (yaw 0): `-sin 0` is -0 and `cos 0` 1, so the stand goes out along
    // +z by half the boat's diagonal and its own width, and lands on the floor.
    let d = (f64::from(1.375_f32 * std::f32::consts::SQRT_2)
        + f64::from(0.5_f32)
        + 9.999_999_747_378_752e-6)
        / 2.0;
    let off = record(&region, riders[0].key).unwrap();
    assert_eq!(
        bits(off.position),
        bits([8.5 + -0.0 * d / 1.0, -60.0, 8.5 + d / 1.0])
    );
}

#[test]
fn a_stand_on_a_broken_vehicle_stays_where_it_has_room() {
    let mut region = on_ticking_floor_with(&[]);
    let (vehicle, riders) = lay_unit(
        &mut region,
        &cart(1, [8.5, -60.0, 8.5]),
        &[&stand(2, [8.5, -60.0 + 0.1875, 8.5])],
    );
    checked(&mut region, &hit(&vehicle, 7, 1.0, true));
    assert!(record(&region, vehicle.key).is_none());
    // `findFreePosition` from the higher of the two: nothing is in the way of its box.
    let off = record(&region, riders[0].key).unwrap();
    assert_eq!(off.riding.vehicle, None);
    let free = record(&region, riders[0].key).unwrap();
    assert_eq!(free.position[0], 8.5);
    assert_eq!(free.position[2], 8.5);
    assert!(
        (free.position[1] - (-60.0 + 0.1875)).abs() < 1.0e-9,
        "{free:?}"
    );
}

// Units crossing lines and chunks.

#[test]
fn a_cart_and_its_rider_cross_a_line_as_one_group_of_arrivals_and_are_taken_whole() {
    let mut region = at_a_line(ChunkArea::EVERYWHERE, &[]);
    // The cart's tick has left it over the line at x = 16.2; it is still filed here.
    let mut carrier = cart(1, [16.2, -60.0, 8.5]);
    carrier.hops = 2;
    let mut rider = boat(2, [16.2, -60.0 + 0.1875, 8.5]);
    rider.riding.vehicle = Some(carrier.key);
    carrier.riding.passengers.push(Seat {
        key: rider.key,
        player: false,
        pending: None,
    });
    lay_in(&mut region, ORIGIN, &carrier);
    lay_in(&mut region, ORIGIN, &rider);
    let output = idle(&mut region);
    let [(first_to, first), (second_to, second)] = &output.arrivals[..] else {
        panic!("{:?}", output.arrivals);
    };
    assert_eq!(
        (*first_to, *second_to),
        (EAST, EAST),
        "both to the root's chunk"
    );
    assert_eq!(
        (first.key, second.key),
        (carrier.key, rider.key),
        "the root first"
    );
    assert_eq!(first.riding, carrier.riding);
    assert_eq!(second.riding.vehicle, Some(carrier.key));
    assert_eq!(first.hops, 2);
    assert!(
        output
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(carrier.key)))
    );
    assert!(
        output
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(rider.key)))
    );
    assert!(record(&region, carrier.key).is_none() && record(&region, rider.key).is_none());

    // The holder of the chunk takes both together, keys and links as they were.
    let mut holder = on_ticking_floor_with(&[]);
    let arrivals = output
        .arrivals
        .iter()
        .enumerate()
        .map(|(index, (chunk, entity))| Arrival {
            id: EffectId {
                sequence: 9,
                index: index as u32,
            },
            chunk: *chunk,
            entity: entity.clone(),
            due: Due::FromTaking,
        })
        .collect();
    let taken = checked(
        &mut holder,
        &TickInputs {
            arrivals,
            ..TickInputs::default()
        },
    );
    assert_eq!(taken.arrivals_taken.len(), 2);
    let root = record(&holder, carrier.key).unwrap();
    let passenger = record(&holder, rider.key).unwrap();
    assert_eq!((root.hops, passenger.hops), (3, 1));
    assert_eq!(root.riding, carrier.riding);
    assert_eq!(passenger.riding.vehicle, Some(carrier.key));
    assert_eq!(holder.entity_chunks[&rider.key], EAST);
    assert!(
        taken
            .entity_changes
            .iter()
            .any(|(chunk, change)| *chunk == EAST
                && matches!(change, EntityChange::Whole(whole) if whole.key == rider.key))
    );
}

#[test]
fn a_root_that_moves_into_another_loaded_chunk_takes_its_passengers_with_it() {
    let mut region = on_ticking_floor_with(&[]);
    // An item that slides over the line into chunk (1, 0), carrying a boat.
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.0], true);
    let (_, riders) = lay_unit(&mut region, &sliding, &[&boat(2, [15.95, -60.0, 8.5])]);
    let output = idle(&mut region);
    assert_eq!(region.entity_chunks[&sliding.key], EAST);
    assert_eq!(region.entity_chunks[&riders[0].key], EAST);
    let order: Vec<(ChunkPos, EntityId, bool)> = output
        .entity_changes
        .iter()
        .filter_map(|(chunk, change)| match change {
            EntityChange::Gone(key) => Some((*chunk, *key, false)),
            EntityChange::Whole(whole) => Some((*chunk, whole.key, true)),
            EntityChange::Set(..) => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            (ORIGIN, sliding.key, false),
            (EAST, sliding.key, true),
            (ORIGIN, riders[0].key, false),
            (EAST, riders[0].key, true),
        ]
    );
    assert!(output.arrivals.is_empty());
}

#[test]
fn a_unit_with_a_players_seat_is_held_back_at_the_line() {
    let mut region = at_a_line(ChunkArea::EVERYWHERE, &[]);
    let mut sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.1], true);
    sliding.riding.passengers.push(Seat {
        key: EntityId(77),
        player: true,
        pending: None,
    });
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    assert!(output.arrivals.is_empty(), "{:?}", output.arrivals);
    let held = record(&region, sliding.key).unwrap();
    assert_eq!(held.position[0], 15.95);
    assert_eq!(held.motion[0], 0.0);
    assert!(held.position[2] > 8.5, "it goes on along z: {held:?}");
    assert_eq!(region.entity_chunks[&sliding.key], ORIGIN);
}

// Vehicles hit and broken.

#[test]
fn five_full_hits_break_a_cart_which_drops_itself_and_the_fourth_does_not() {
    let mut region = on_ticking_floor_with(&[]);
    let target = cart(1, [8.5, -60.0, 8.5]);
    lay(&mut region, &target);
    let mut output = TickOutput::default();
    let attack = EntityUse::Attack {
        attacker: EntityId(1),
        creative: false,
        sprinting: false,
        damage: Float(1.0),
        from: [Double(0.5), Double(-60.0), Double(0.5)],
    };
    for (n, direction) in (1..=4).zip([-1, 1, -1, 1]) {
        region.entity_used(target.key, attack, &mut output);
        let EntityData::Minecart { hurt, .. } = record(&region, target.key).unwrap().data else {
            panic!();
        };
        // Exactly 40 does not break it.
        assert_eq!(
            hurt,
            Hurt {
                time: 10,
                direction,
                damage: Float(10.0 * n as f32)
            }
        );
    }
    assert!(items_lying(&region).is_empty());
    region.entity_used(target.key, attack, &mut output);
    assert!(record(&region, target.key).is_none());
    let dropped = items_lying(&region);
    let [(_, at, stack, delay, _)] = &dropped[..] else {
        panic!("{dropped:?}");
    };
    assert_eq!(bits(*at), bits(target.position));
    assert_eq!(
        *stack,
        ItemStack {
            item: clustine_data::items::MINECART,
            count: 1
        }
    );
    assert_eq!(*delay, 10);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityGone { entity, .. } if *entity == target.key
    )));
}

#[test]
fn a_hit_from_across_a_line_hurts_a_boat_and_its_damage_falls_by_one_a_tick() {
    let mut region = on_ticking_floor_with(&[]);
    let target = boat(1, [8.5, -60.0, 8.5]);
    lay(&mut region, &target);
    let output = checked(&mut region, &hit(&target, 7, 0.5, false));
    // The hit comes before the boat's tick, which lets the wobble and the damage fall.
    let hurt_now = |region: &Region| match record(region, target.key).unwrap().data {
        EntityData::Boat { hurt, .. } => hurt,
        _ => panic!(),
    };
    assert_eq!(
        hurt_now(&region),
        Hurt {
            time: 9,
            direction: -1,
            damage: Float(5.0 - 1.0)
        }
    );
    // The hit commits; the fall does not, the chunk ageing instead.
    assert!(output.entity_changes.iter().any(|(_, change)| matches!(
        change,
        EntityChange::Set(changed, _) if *changed == target.key
    )));
    for _ in 0..3 {
        let output = idle(&mut region);
        assert!(output.entity_changes.is_empty());
        assert!(output.entities_ageing.contains(&ORIGIN));
    }
    assert_eq!(
        hurt_now(&region),
        Hurt {
            time: 6,
            direction: -1,
            damage: Float(1.0)
        }
    );
    for _ in 0..10 {
        idle(&mut region);
    }
    assert_eq!(
        hurt_now(&region),
        Hurt {
            time: 0,
            direction: -1,
            damage: Float(0.0)
        }
    );
}

#[test]
fn a_creative_hit_discards_a_boat_and_it_drops_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    let target = boat(1, [8.5, -60.0, 8.5]);
    lay(&mut region, &target);
    checked(&mut region, &hit(&target, 7, 0.1, true));
    assert!(record(&region, target.key).is_none());
    assert!(items_lying(&region).is_empty());
}

/// What `Containers.dropContents` draws, worked out from the stream as the game's rule
/// draws: for every slot three doubles of its place, and for each piece of a stack its
/// size and then its motion.
fn expected_spill(
    stream: &mut Random,
    position: [f64; 3],
    contents: &[(u8, ItemStack)],
    size: usize,
) -> Vec<([f64; 3], ItemStack, [f64; 3])> {
    let mut items = Vec::new();
    for slot in 0..size {
        let x = position[0].floor() + stream.next_double() * 0.75 + 0.125;
        let y = position[1].floor() + stream.next_double() * 0.75;
        let z = position[2].floor() + stream.next_double() * 0.75 + 0.125;
        let Some((_, mut stack)) = contents
            .iter()
            .find(|(at, _)| usize::from(*at) == slot)
            .copied()
        else {
            continue;
        };
        while stack.count > 0 {
            let piece = (stream.next_int(21) as i32 + 10).min(stack.count);
            stack.count -= piece;
            let spread = 0.114_850_001_711_398_36;
            let mut triangle = |centre: f64| {
                let a = stream.next_double();
                let b = stream.next_double();
                centre + spread * (a - b)
            };
            let motion = [triangle(0.0), triangle(0.2), triangle(0.0)];
            items.push((
                [x, y, z],
                ItemStack {
                    item: stack.item,
                    count: piece,
                },
                motion,
            ));
        }
    }
    items
}

#[test]
fn a_chest_carts_contents_spill_by_the_games_draws_in_creative_too() {
    let mut region = on_ticking_floor_with(&[]);
    let stone = ItemStack {
        item: clustine_data::items::STONE,
        count: 3,
    };
    let dirt = ItemStack {
        item: DIRT.item,
        count: 64,
    };
    let contents = vec![(0, dirt), (5, stone)];
    let chest = entity(
        1,
        entity_types::CHEST_MINECART,
        [8.3, -60.0, 8.7],
        cart_data(contents.clone()),
    );
    lay(&mut region, &chest);
    let mut output = TickOutput::default();
    let attack = EntityUse::Attack {
        attacker: EntityId(1),
        creative: true,
        sprinting: false,
        damage: Float(0.1),
        from: [Double(0.5), Double(-60.0), Double(0.5)],
    };
    region.entity_used(chest.key, attack, &mut output);
    assert!(record(&region, chest.key).is_none());
    let mut stream = Random::of_block(
        region.config.seed,
        BlockPos::new(8, -60, 8),
        region.tick,
        crate::region::vehicles::SPILLS,
    );
    let expected = expected_spill(&mut stream, chest.position, &contents, 27);
    assert!(
        expected.len() >= 4,
        "64 dirt come out in pieces of 10 to 30"
    );
    // The items are made in the order spilled, so their keys ascend in it.
    let mut lying = items_lying(&region);
    lying.sort_by_key(|(key, ..)| *key);
    assert_eq!(lying.len(), expected.len());
    for ((_, at, stack, delay, motion), (place, piece, thrown)) in lying.iter().zip(&expected) {
        assert_eq!(stack, piece);
        assert_eq!(*delay, 0, "no pick-up delay of their own");
        assert_eq!(bits(*at), bits(*place));
        assert_eq!(bits(*motion), bits(*thrown));
        assert!(at[0] >= 8.125 && at[0] < 8.875 && at[2] >= 8.125 && at[2] < 8.875);
    }
    let total: i32 = lying.iter().map(|(_, _, stack, ..)| stack.count).sum();
    assert_eq!(total, 67);
}

#[test]
fn a_chest_cart_broken_in_survival_spills_then_drops_itself_then_draws_its_empty_slots_again() {
    let mut region = on_ticking_floor_with(&[]);
    let contents = vec![(
        3,
        ItemStack {
            item: DIRT.item,
            count: 5,
        },
    )];
    let chest = entity(
        1,
        entity_types::CHEST_MINECART,
        [8.5, -60.0, 8.5],
        cart_data(contents.clone()),
    );
    lay(&mut region, &chest);
    let mut output = TickOutput::default();
    let attack = EntityUse::Attack {
        attacker: EntityId(1),
        creative: false,
        sprinting: false,
        damage: Float(5.0),
        from: [Double(0.5), Double(-60.0), Double(0.5)],
    };
    region.entity_used(chest.key, attack, &mut output);
    assert!(record(&region, chest.key).is_none());
    let mut lying = items_lying(&region);
    lying.sort_by_key(|(key, ..)| *key);
    // The dirt first, then the cart's own item.
    let stacks: Vec<ItemStack> = lying.iter().map(|(_, _, stack, ..)| *stack).collect();
    assert_eq!(
        stacks,
        [
            ItemStack {
                item: DIRT.item,
                count: 5
            },
            ItemStack {
                item: clustine_data::items::CHEST_MINECART,
                count: 1
            }
        ]
    );
    // The stream has drawn both spills: 27 places, the piece and its motion, then 27
    // places more.
    let mut stream = Random::of_block(
        region.config.seed,
        BlockPos::new(8, -60, 8),
        region.tick,
        crate::region::vehicles::SPILLS,
    );
    expected_spill(&mut stream, chest.position, &contents, 27);
    expected_spill(&mut stream, chest.position, &[], 27);
    assert_eq!(region.block_streams[&BlockPos::new(8, -60, 8)], stream);
}

// Block-attached entities.

#[test]
fn a_cushion_in_a_pistons_way_is_popped_and_drops_its_item() {
    let piston = BlockPos::new(1, -60, 2);
    let mut region = on_ticking_floor_with(&[(
        piston,
        BlockState::parse("minecraft:piston[facing=east]").unwrap(),
    )]);
    let seat = cushion(1, [2.5, -60.0, 2.5], 14);
    lay(&mut region, &seat);
    idle(&mut region);
    let mut output = TickOutput::default();
    region.set_block(
        BlockPos::new(1, -60, 3),
        blocks::REDSTONE_BLOCK,
        &mut output,
    );
    let popped = idle(&mut region);
    assert!(record(&region, seat.key).is_none());
    assert!(popped.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityGone { entity, .. } if *entity == seat.key
    )));
    let dropped = items_lying(&region);
    let [(_, at, stack, delay, _)] = &dropped[..] else {
        panic!("{dropped:?}");
    };
    // A red cushion, at the cushion's place.
    assert_eq!(
        *stack,
        ItemStack {
            item: clustine_data::items::WHITE_CUSHION + 14,
            count: 1
        }
    );
    assert_eq!(bits(*at), bits(seat.position));
    assert_eq!(*delay, 10);
    let _ = block_at(&region, piston);
}

#[test]
fn a_painting_in_a_pistons_way_is_popped() {
    let piston = BlockPos::new(1, -60, 2);
    let mut region = on_ticking_floor_with(&[(
        piston,
        BlockState::parse("minecraft:piston[facing=east]").unwrap(),
    )]);
    let painting = entity(
        1,
        entity_types::PAINTING,
        [2.5, -59.75, 2.5],
        EntityData::Painting {
            attached: Attached {
                block: BlockPos::new(2, -60, 2),
                direction: 3,
            },
            variant: 0,
        },
    );
    lay(&mut region, &painting);
    idle(&mut region);
    let mut output = TickOutput::default();
    region.set_block(
        BlockPos::new(1, -60, 3),
        blocks::REDSTONE_BLOCK,
        &mut output,
    );
    idle(&mut region);
    assert!(record(&region, painting.key).is_none());
}

#[test]
fn a_cushion_whose_floor_went_is_dropped_at_its_hundred_and_first_tick_and_not_before() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [8.5, -60.0, 8.5], 0);
    lay(&mut region, &seat);
    // One tick with its floor, which counts: the checks count from its first tick.
    idle(&mut region);
    let mut output = TickOutput::default();
    region.set_block(BlockPos::new(8, -61, 8), blocks::AIR, &mut output);
    for tick in 2..=100 {
        idle(&mut region);
        assert!(record(&region, seat.key).is_some(), "tick {tick}");
    }
    let output = idle(&mut region);
    assert!(record(&region, seat.key).is_none());
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityGone { entity, .. } if *entity == seat.key
    )));
    let dropped = items_lying(&region);
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].2.item, clustine_data::items::WHITE_CUSHION);
}

#[test]
fn a_cushion_on_its_floor_holds_at_every_check() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [8.5, -60.0, 8.5], 3);
    lay(&mut region, &seat);
    for _ in 0..303 {
        idle(&mut region);
    }
    assert_eq!(record(&region, seat.key).unwrap(), seat);
}

#[test]
fn a_creative_players_hit_breaks_a_cushion_and_it_drops_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [8.5, -60.0, 8.5], 3);
    lay(&mut region, &seat);
    checked(&mut region, &hit(&seat, 7, 0.0, true));
    assert!(record(&region, seat.key).is_none());
    assert!(items_lying(&region).is_empty());
    // A survival player's gets the item.
    let other = cushion(2, [10.5, -60.0, 8.5], 3);
    lay(&mut region, &other);
    let mut output = TickOutput::default();
    assert!(region.hurt_attached(other.key, Breaker::Player { creative: false }, &mut output));
    assert_eq!(items_lying(&region).len(), 1);
}

#[test]
fn a_push_breaks_a_block_attached_entity_and_moves_anything_else() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [8.5, -60.0, 8.5], 0);
    lay(&mut region, &seat);
    let target = boat(2, [4.5, -60.0, 4.5]);
    lay(&mut region, &target);
    let mut output = TickOutput::default();
    region.push_entity(seat.key, [0.0; 3], &mut output);
    assert!(
        record(&region, seat.key).is_some(),
        "a push of nothing does nothing"
    );
    region.push_entity(seat.key, [0.05, 0.0, 0.0], &mut output);
    assert!(record(&region, seat.key).is_none());
    region.push_entity(target.key, [0.05, 0.0, -0.05], &mut output);
    assert_eq!(
        record(&region, target.key).unwrap().motion,
        [0.05, 0.0, -0.05]
    );
}

/// The player's hit on `target` as their client sends it.
fn attack(target: &Entity) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Attack { entity: target.key },
    )])
}

#[test]
fn a_players_hit_within_reach_breaks_a_cushion_and_one_out_of_reach_does_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    // The player's eyes are at (0.5, -58.38, 0.5); a creative player reaches 8 from them
    // to the nearest point of a box.
    let near = cushion(1, [7.5, -60.0, 0.5], 0);
    let far = cushion(2, [9.5, -60.0, 0.5], 0);
    lay(&mut region, &near);
    lay(&mut region, &far);
    checked(&mut region, &attack(&far));
    assert!(
        record(&region, far.key).is_some(),
        "8.6 from the eyes: out of reach"
    );
    checked(&mut region, &attack(&near));
    assert!(
        record(&region, near.key).is_none(),
        "6.6 from the eyes: in reach"
    );
    assert!(
        items_lying(&region).is_empty(),
        "a creative player's break drops nothing"
    );
}

#[test]
fn a_players_hit_on_an_entity_in_another_regions_copy_is_an_effect_on_it() {
    // The region holds the land west of x = 16; the chunks east of it are copies.
    let mut region = at_a_line(super::arrivals::WEST_OF_THE_LINE, &[]);
    let seat = cushion(9, [17.5, -60.0, 0.5], 0);
    region
        .copies
        .get_mut(&EAST)
        .unwrap()
        .apply_entity(&EntityChange::Whole(seat.clone()));
    // 16.4 from the player at the spawn: within reach once they stand at x = 12.
    let walked = moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(12.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        },
    )]);
    checked(&mut region, &walked);
    let output = region.tick(&attack(&seat));
    assert_eq!(output.effects.len(), 1, "{:?}", output.effects);
    let made = output.effects[0];
    assert_eq!(made.at, BlockPos::new(17, -60, 0));
    assert!(matches!(
        made.what,
        Effected::OnEntity {
            key,
            by: EntityUse::Attack { creative: true, .. },
        } if key == seat.key
    ));
}

/// The player's use of `target`, as their client sends it, not sneaking.
fn use_entity(target: &Entity) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: target.key,
            location: [0.0, 0.1, 0.0],
            sneaking: false,
        },
    )])
}

fn keys(sneak: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Keys(crate::api::Keys {
            sneak,
            ..Default::default()
        }),
    )])
}

/// Where the region has player 1.
fn feet(region: &Region) -> [f64; 3] {
    let position = region.players[&player(1)].pose.position;
    [position.x, position.y, position.z]
}

#[test]
fn a_player_who_uses_a_cushion_sits_on_it_at_its_seat_less_their_own_vehicle_point() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [2.5, -60.0, 0.5], 0);
    lay(&mut region, &seat);
    let output = checked(&mut region, &use_entity(&seat));
    // The cushion's passenger point is 0.25 up, the player's vehicle point 0.6.
    assert_eq!(bits(feet(&region)), bits([2.5, (-60.0 + 0.25) - 0.6, 0.5]));
    let riding = record(&region, seat.key).unwrap().riding.passengers;
    assert_eq!(riding.len(), 1);
    assert!(riding[0].player && riding[0].key == EntityId(1));
    assert!(output.player_events.iter().any(|(id, event)| {
        *id == player(1)
            && matches!(event, PlayerEvent::Rides { vehicle: Some(vehicle), .. } if *vehicle == seat.key)
    }));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Passengers { vehicle, passengers, .. }
            if *vehicle == seat.key && passengers == &[EntityId(1)]
    )));
}

#[test]
fn a_riding_player_s_moves_turn_their_head_and_move_them_nowhere() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [2.5, -60.0, 0.5], 0);
    lay(&mut region, &seat);
    checked(&mut region, &use_entity(&seat));
    let sat = feet(&region);
    let walked = moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(9.5, -60.0, 9.5)),
            rotation: Some((45.0, 10.0)),
            on_ground: true,
        },
    )]);
    checked(&mut region, &walked);
    assert_eq!(bits(feet(&region)), bits(sat));
    assert_eq!(region.players[&player(1)].pose.yaw, 45.0);
}

#[test]
fn a_player_who_sneaks_gets_off_beside_the_cushion_and_its_seat_is_free_again() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [2.5, -60.0, 0.5], 0);
    lay(&mut region, &seat);
    checked(&mut region, &use_entity(&seat));
    let output = checked(&mut region, &keys(true));
    assert!(region.players[&player(1)].riding.is_none());
    assert!(
        record(&region, seat.key)
            .unwrap()
            .riding
            .passengers
            .is_empty()
    );
    // On top of the cushion's box, as `Entity.getDismountLocationForPassenger` lets off.
    assert_eq!(bits(feet(&region)), bits([2.5, -60.0 + 0.25, 0.5]));
    assert!(output.player_events.iter().any(|(id, event)| {
        *id == player(1) && matches!(event, PlayerEvent::Rides { vehicle: None, .. })
    }));
    // A player who sneaks does not get on.
    checked(&mut region, &use_entity(&seat));
    assert!(region.players[&player(1)].riding.is_none());
}

#[test]
fn a_player_who_leaves_while_seated_leaves_the_seat_free() {
    let mut region = on_ticking_floor_with(&[]);
    let seat = cushion(1, [2.5, -60.0, 0.5], 0);
    lay(&mut region, &seat);
    checked(&mut region, &use_entity(&seat));
    checked(
        &mut region,
        &TickInputs {
            player_changes: vec![leave_as(1, 1)],
            ..TickInputs::default()
        },
    );
    assert!(
        record(&region, seat.key)
            .unwrap()
            .riding
            .passengers
            .is_empty()
    );
}

#[test]
fn a_cushion_used_on_the_floor_is_laid_on_it_facing_the_player_s_way_and_sat_on() {
    let mut region = on_ticking_floor_with(&[]);
    let held = ItemStack {
        item: clustine_data::items::RED_CUSHION,
        count: 1,
    };
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::SetInventorySlot {
                slot: 0,
                stack: Some(held),
            },
        )]),
    );
    // The top of the floor block at (2, -61, 0), clicked at its middle by a player who
    // faces west.
    let laid = checked(
        &mut region,
        &moves(vec![
            numbered(
                1,
                PlayerInput::Move {
                    position: None,
                    rotation: Some((90.0, 30.0)),
                    on_ground: true,
                },
            ),
            numbered(
                1,
                PlayerInput::UseItemOn {
                    position: BlockPos::new(2, -61, 0),
                    face: Face::Top,
                    cursor: [0.5, 1.0, 0.5],
                    sneaking: false,
                    sequence: 3,
                },
            ),
        ]),
    );
    let cushions: Vec<Entity> = region
        .chunk(ORIGIN)
        .unwrap()
        .entities()
        .iter()
        .filter(|entity| entity.kind == entity_types::CUSHION as u16)
        .cloned()
        .collect();
    let [seat] = &cushions[..] else {
        panic!("one cushion: {cushions:?} after {:?}", laid.events);
    };
    assert_eq!(bits(seat.position), bits([2.5, -60.0, 0.5]));
    assert_eq!(seat.yaw, 90.0);
    assert!(matches!(seat.data, EntityData::Cushion { colour: 14, .. }));
    // A creative player keeps the item.
    assert_eq!(region.players[&player(1)].inventory.0[0], Some(held));
    // A second one on the same place is refused.
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::UseItemOn {
                position: BlockPos::new(2, -61, 0),
                face: Face::Top,
                cursor: [0.5, 1.0, 0.5],
                sneaking: false,
                sequence: 4,
            },
        )]),
    );
    let count = region
        .chunk(ORIGIN)
        .unwrap()
        .entities()
        .iter()
        .filter(|entity| entity.kind == entity_types::CUSHION as u16)
        .count();
    assert_eq!(count, 1);
    checked(&mut region, &use_entity(seat));
    assert!(region.players[&player(1)].riding.is_some());
}
