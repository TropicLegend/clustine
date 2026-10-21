//! Tests of riding a pig (`docs/groundwork/n7h-riding-plan.md`, with its research note's
//! rules from the game's bytecode, `n7h-riding-research.md`): a saddled pig takes a
//! player on, who steers it with a carrot on a stick and boosts it; it takes its
//! rider's rotation and stands still for their client's moves; it lets them off beside
//! it; a fall its driver's moves end hurts it. The numbers each test expects are worked
//! out here from the game's formulas, not taken from what the region computes.
//!
//! The regions are those of the tests of redstone (`redstone.rs`): player 1 at the
//! spawn, (0.5, -60, 0.5), on a stone floor whose top is y = -60, with the nine chunks
//! within one of chunk (0, 0) ticking.

use clustine_data::{EntityTypeInfo, entity_types, items};
use clustine_world::{Double, Entity};

use super::redstone::{holding, ticking_with};
use super::*;
use crate::api::VehicleMove;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::types::pig;

const PIG_KEY: EntityId = EntityId(40);

/// A grown pig at `position`, saddled where `saddled`, of full health.
fn pig_at(position: [f64; 3], saddled: bool) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = crate::living::MAX_AIR;
    if saddled {
        mob.body.equipment.slots[slots::SADDLE as usize] = Some(ItemStack {
            item: items::SADDLE,
            count: 1,
        });
    }
    Entity {
        key: PIG_KEY,
        kind: entity_types::PIG as u16,
        uuid: uuid::Uuid::from_u128(40),
        position,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("a loaded chunk")
        .apply_entity(&clustine_world::EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .expect("the entity is here")
        .1
        .clone()
}

fn mob(region: &Region) -> Mob {
    Mob::decode(&record(region, PIG_KEY).data).expect("a mob")
}

fn interact(sneaking: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: PIG_KEY,
            location: [0.0, 0.5, 0.0],
            sneaking,
        },
    )])
}

fn use_stick() -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::UseItem {
            rotation: (0.0, 0.0),
            sneaking: false,
            sequence: 1,
        },
    )])
}

fn move_vehicle(position: [f64; 3], yaw: f32, pitch: f32, on_ground: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::MoveVehicle(VehicleMove {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw,
            pitch,
            on_ground,
        }),
    )])
}

fn feet(region: &Region) -> [f64; 3] {
    let at = region.players[&player(1)].pose.position;
    [at.x, at.y, at.z]
}

fn riding(region: &Region) -> Option<EntityId> {
    region.players[&player(1)]
        .riding
        .map(|riding| riding.vehicle)
}

/// A pig beside the spawn, saddled, with player 1 on it holding a carrot on a stick.
fn ridden() -> Region {
    let mut region = ticking_with(&[]);
    lay(&mut region, &pig_at([2.5, -60.0, 0.5], true));
    checked(&mut region, &holding(items::CARROT_ON_A_STICK));
    checked(&mut region, &interact(false));
    assert_eq!(riding(&region), Some(PIG_KEY));
    region
}

/// `Pig.mobInteract`: a saddled pig that carries nobody takes on a player who does not
/// sneak and holds none of its food, at its passenger point (0.86875 up) less the
/// player's own (0.6); one who sneaks, or holds a carrot, or a pig with no saddle, does
/// not.
#[test]
fn a_saddled_pig_takes_on_a_player_who_clicks_it_and_seats_them_at_its_back() {
    let mut region = ticking_with(&[]);
    lay(&mut region, &pig_at([2.5, -60.0, 0.5], true));
    checked(&mut region, &interact(true));
    assert_eq!(riding(&region), None, "sneaking");
    checked(&mut region, &holding(items::CARROT));
    checked(&mut region, &interact(false));
    assert_eq!(riding(&region), None, "its food in hand");
    checked(&mut region, &holding(items::STICK));
    checked(&mut region, &interact(false));
    assert_eq!(riding(&region), Some(PIG_KEY));
    let point = EntityTypeInfo::of(entity_types::PIG).passenger_points[0];
    let attach = EntityTypeInfo::of(entity_types::PLAYER).vehicle_points[0];
    assert_eq!(point[1].to_bits(), 0.868_749_976_158_142_1_f64.to_bits());
    let pig = record(&region, PIG_KEY);
    assert_eq!(
        feet(&region)[1].to_bits(),
        ((pig.position[1] + point[1]) - attach[1]).to_bits()
    );

    let mut bare = ticking_with(&[]);
    lay(&mut bare, &pig_at([2.5, -60.0, 0.5], false));
    checked(&mut bare, &interact(false));
    assert_eq!(riding(&bare), None, "no saddle");
}

/// `getControllingPassenger`: with a carrot on a stick, the rider's client moves the
/// pig and the move is taken where it says; with the stick put away it is the pig's,
/// and the rider's move does not move it.
#[test]
fn a_rider_with_a_carrot_on_a_stick_drives_the_pig_and_without_it_does_not() {
    let mut region = ridden();
    let to = [3.5, -60.0, 1.25];
    let output = checked(&mut region, &move_vehicle(to, 30.0, 5.0, true));
    assert!(
        !output
            .player_events
            .iter()
            .any(|(_, event)| matches!(event, PlayerEvent::VehicleCorrected { .. })),
        "{:?}",
        output.player_events
    );
    assert_eq!(
        record(&region, PIG_KEY).position.map(f64::to_bits),
        to.map(f64::to_bits)
    );
    // The stick away: the next move is a passenger's, and the pig stays its own.
    checked(&mut region, &holding(items::STICK));
    let before = record(&region, PIG_KEY).position;
    checked(
        &mut region,
        &move_vehicle([4.5, -60.0, 1.25], 30.0, 5.0, true),
    );
    let after = record(&region, PIG_KEY).position;
    assert_ne!(
        after.map(f64::to_bits),
        [4.5, -60.0, 1.25].map(f64::to_bits)
    );
    let _ = before;
}

/// `travelRidden` and `Pig.tickRidden`: a driven pig takes its rider's yaw and half
/// their pitch, its body and head turned to the yaw, and has no motion of its own.
#[test]
fn a_driven_pig_takes_its_riders_yaw_and_half_their_pitch_and_no_motion() {
    let mut region = ridden();
    {
        let rider = region.players.get_mut(&player(1)).expect("player 1");
        rider.pose.yaw = 90.0;
        rider.pose.pitch = 40.0;
    }
    idle(&mut region);
    let pig = record(&region, PIG_KEY);
    assert_eq!((pig.yaw, pig.pitch), (90.0, 20.0));
    assert_eq!(pig.motion, [0.0; 3]);
    let head = mob(&region).body.head;
    assert_eq!((head.head_yaw, head.body_yaw), (90.0, 90.0));
}

/// `FoodOnAStickItem.use` and `ItemBasedSteering`: a use of the stick boosts the driven
/// pig for `nextInt(841) + 140` ticks, shown at index 18; a second use while it boosts
/// does nothing; the boost counts only while it is driven, and is over once its count
/// passes the total.
#[test]
fn a_use_of_the_stick_boosts_the_driven_pig_once_until_its_time_is_out() {
    let mut region = ridden();
    checked(&mut region, &use_stick());
    let boost = pig::boost_of(&mob(&region));
    assert!(boost.boosting);
    assert!((140..=980).contains(&boost.total), "{boost:?}");
    let shown = crate::shown::mob_values(entity_types::PIG as u16, &record(&region, PIG_KEY).data);
    assert!(
        shown.iter().any(|(index, value, default)| *index == 18
            && *value == crate::shown::DataShown::Int(boost.total)
            && !default),
        "{shown:?}"
    );
    checked(&mut region, &use_stick());
    assert_eq!(
        pig::boost_of(&mob(&region)).total,
        boost.total,
        "refused while it boosts"
    );
    // `boostTime++ > total` ends it: at the tick whose count before is total + 1.
    let ticked = pig::boost_of(&mob(&region)).time;
    for _ in ticked..=(boost.total as u32 + 1) {
        idle(&mut region);
    }
    let after = pig::boost_of(&mob(&region));
    assert!(!after.boosting, "{after:?}");
    assert_eq!(after.total, boost.total, "the last boost's length is kept");
}

/// A pig kept before there was a boost reads as without one, its variant as it was;
/// a boost kept after keeps it too.
#[test]
fn a_pig_kept_before_the_boost_keeps_its_variant_and_reads_as_unboosted() {
    let mut mob = Mob::fresh();
    let warm = crate::mobs::types::animal::variants::id("minecraft:pig_variant", "minecraft:warm");
    mob.state = crate::mobs::types::animal::write(
        &crate::mobs::types::animal::Animal::default(),
        &pig::PigState {
            variant: warm,
            ..pig::PigState::default()
        },
    );
    assert_eq!(pig::boost_of(&mob), pig::Boost::default());
    let mut draws = crate::region::Random::of_section(7, ChunkPos::new(0, 0), 0, 0);
    assert!(pig::boost(&mut mob, &mut draws));
    let (_, own) = crate::mobs::types::animal::read::<pig::PigState>(&mob.state);
    assert_eq!(own.variant, warm);
    assert!(pig::boost_of(&mob).boosting);
}

/// Sneaking puts the rider off: `Animal.getDismountLocationForPassenger`, the first of
/// the places about the pig's block by its way that has a floor and room, the one to
/// the right of a pig facing south (yaw 0): its west.
#[test]
fn sneaking_puts_the_rider_off_beside_the_pig() {
    let mut region = ridden();
    let sneak = moves(vec![numbered(
        1,
        PlayerInput::Keys(crate::api::Keys {
            sneak: true,
            ..Default::default()
        }),
    )]);
    checked(&mut region, &sneak);
    idle(&mut region);
    assert_eq!(riding(&region), None);
    let pig = record(&region, PIG_KEY);
    assert!(pig.riding.passengers.is_empty());
    let block = crate::physics::containing(pig.position);
    let at = feet(&region);
    // Facing south, its right is west: one block to the west of its block, on the floor.
    assert_eq!(
        at,
        [
            f64::from(block.x - 1) + 0.5,
            -60.0,
            f64::from(block.z) + 0.5
        ],
        "{:?}",
        pig.yaw
    );
}

/// `LivingEntity.causeFallDamage` of a driven pig: its driver's moves up five blocks,
/// then down a block at a time, the last onto the ground, end a fall of five (the
/// landing move's own drop counts: `Entity.checkFallDamage` adds it before it ends the
/// fall on the ground), which hurts it by `floor(5 - 3) = 2`; a move up counts nothing.
#[test]
fn a_driven_pig_whose_drivers_moves_end_a_fall_of_five_is_hurt_by_two() {
    let mut region = ridden();
    let [x, _, z] = record(&region, PIG_KEY).position;
    for y in [-59.0, -57.0, -55.0, -56.0, -57.0, -58.0, -59.0] {
        checked(&mut region, &move_vehicle([x, y, z], 0.0, 0.0, false));
    }
    assert_eq!(
        mob(&region).body.vitals.health,
        10.0,
        "nothing before it lands"
    );
    checked(&mut region, &move_vehicle([x, -60.0, z], 0.0, 0.0, true));
    assert_eq!(mob(&region).body.vitals.health, 8.0);
    let _ = Double(0.0);
}

/// A pig whose first seat is kept for a player on their way to it across a line stands
/// still, thinking nothing, so that the moves their client sent meanwhile are judged
/// from where it stood (`docs/groundwork/n7h-riding-plan.md`, section 10 item 2).
#[test]
fn a_pig_whose_seat_is_kept_for_a_player_on_their_way_stands_still() {
    let mut region = ticking_with(&[]);
    let mut kept = pig_at([2.5, -60.0, 0.5], true);
    kept.motion = [0.2, 0.0, 0.0];
    kept.riding.passengers = vec![clustine_world::Seat {
        key: EntityId(900),
        player: true,
        pending: Some(clustine_world::Pending {
            left: 100,
            edge: clustine_world::EdgeId(1),
        }),
    }];
    lay(&mut region, &kept);
    let before = record(&region, PIG_KEY).position;
    assert_eq!(
        region.ridden(&kept, &Mob::decode(&kept.data).unwrap()),
        crate::mobs::tick::Ridden::Pending
    );
    for _ in 0..5 {
        idle(&mut region);
    }
    assert_eq!(
        record(&region, PIG_KEY).position.map(f64::to_bits),
        before.map(f64::to_bits)
    );
}

/// A later stay of a rider that arrives while the earlier one rides ends the earlier one,
/// which is noted where the pig lets it off, and the later one is noted as it is: two
/// notes, each by its own stay's entity, neither in place of the other
/// (`docs/groundwork/n7h-logout-on-a-mount.md`, section 13 item 2).
#[test]
fn a_later_stay_that_arrives_over_a_rider_keeps_both_stays_notes() {
    let mut region = ridden();
    let later = super::transfer(1, 77, 2.5, 2);
    let output = checked(&mut region, &changes(vec![super::arrive(1, &later)]));
    let has: Vec<(EntityId, [f64; 3])> = output
        .stays
        .iter()
        .filter_map(|note| match note {
            StayNote::Has { entity, place, .. } => Some((
                *entity,
                [
                    place.pose.position.x,
                    place.pose.position.y,
                    place.pose.position.z,
                ],
            )),
            _ => None,
        })
        .collect();
    assert_eq!(has.len(), 2, "{:?}", output.stays);
    let (first, off) = has[0];
    assert_eq!(first, EntityId(1), "the ended rider's note first");
    // Beside the pig, on the floor, not in its seat.
    assert_eq!(off[1], -60.0, "{off:?}");
    assert_eq!(has[1].0, EntityId(77));
}

/// `getKnownMovement()` of a ridden pig, which a lead it holds pulls by: its driver's
/// while they steer it with the stick, its own motion once they do not.
#[test]
fn a_ridden_pigs_known_movement_is_its_drivers_only_while_they_steer_it() {
    let mut region = ridden();
    region.known_movements.insert(player(1), [0.25, 0.0, -0.5]);
    assert_eq!(
        region.record_movement(&record(&region, PIG_KEY)),
        [0.25, 0.0, -0.5]
    );
    checked(&mut region, &holding(items::STICK));
    let pig = record(&region, PIG_KEY);
    assert_eq!(region.record_movement(&pig), pig.motion);
}
