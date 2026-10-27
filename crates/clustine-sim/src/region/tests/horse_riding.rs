//! Tests of riding a horse (`docs/groundwork/n7h-riding-plan.md` section 7, with the
//! research note's R9 from the game's bytecode): a tamed horse takes on a player who
//! clicks it, turned to its way; a saddled one is steered by its rider whatever they
//! hold, and takes their rotation; the jump bar's start rears it and sounds its kind's
//! jump; sneaking puts the rider off at the side of their main arm; a fall ends with its
//! landing sound and half the damage past six blocks. The numbers each test expects are
//! worked out here from the game's formulas, not taken from what the region computes.
//!
//! The regions are those of the tests of redstone (`redstone.rs`): player 1 at the
//! spawn, (0.5, -60, 0.5), on a stone floor whose top is y = -60, with the chunks within
//! two of chunk (0, 0) ticking.

use clustine_data::{EntityTypeInfo, entity_types, items, sounds};
use clustine_world::Entity;

use super::redstone::ticking_with;
use super::*;
use crate::api::VehicleMove;
use crate::living::{MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::equine::{self, HorseState};

const HORSE_KEY: EntityId = EntityId(40);
const RIDER: EntityId = EntityId(1);

/// A grown, tamed horse of the type `kind` at `position`, facing `yaw`, saddled where
/// `saddled`, at its type's full health.
fn horse_at(kind: i32, position: [f64; 3], yaw: f32, saddled: bool) -> Entity {
    let kind = kind as u16;
    let mut mob = Mob::fresh();
    mob.body.vitals.health = health_of(kind);
    mob.body.vitals.air = MAX_AIR;
    mob.body.head.head_yaw = yaw;
    mob.body.head.body_yaw = yaw;
    mob.state = animal::write(
        &Animal::default(),
        &HorseState {
            tame: true,
            ..HorseState::default()
        },
    );
    if saddled {
        mob.body.equipment.slots[slots::SADDLE as usize] = Some(ItemStack::new(items::SADDLE, 1));
    }
    Entity {
        key: HORSE_KEY,
        kind,
        uuid: uuid::Uuid::from_u128(40),
        position,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
        yaw,
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

fn health_of(kind: u16) -> f32 {
    crate::mobs::body::attributes::supplied(kind, 23).expect("a max health") as f32
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

fn record(region: &Region) -> Entity {
    region
        .find_entity(HORSE_KEY)
        .expect("the horse is here")
        .1
        .clone()
}

fn mob(region: &Region) -> Mob {
    Mob::decode(&record(region).data).expect("a mob")
}

fn interact(sneaking: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: HORSE_KEY,
            location: [0.0, 0.5, 0.0],
            sneaking,
        },
    )])
}

fn jump(power: i32) -> TickInputs {
    moves(vec![numbered(1, PlayerInput::RidingJump { power })])
}

fn move_vehicle(position: [f64; 3], on_ground: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::MoveVehicle(VehicleMove {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw: 0.0,
            pitch: 0.0,
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

/// The sounds of `output`, each with its volume.
fn sounds_of(output: &TickOutput) -> Vec<(i32, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, volume, .. } => Some((*sound, *volume)),
            _ => None,
        })
        .collect()
}

/// A tamed horse of the type `kind` at (3.5, -60, 3.5) facing `yaw`, with player 1 on it,
/// their hand empty.
fn ridden(kind: i32, yaw: f32, saddled: bool) -> Region {
    let mut region = ticking_with(&[]);
    lay(
        &mut region,
        &horse_at(kind, [3.5, -60.0, 3.5], yaw, saddled),
    );
    checked(&mut region, &interact(false));
    assert_eq!(riding(&region), Some(HORSE_KEY));
    region
}

/// `AbstractHorse.mobInteract`, `doPlayerRide` and `addPassenger`: a tamed grown horse
/// clicked by a player who does not sneak takes them on, saddled or not, and turns them
/// to where it looks (its head, here on its yaw) and its pitch; they sit at its passenger point (1.44375 up) less their own (0.6).
#[test]
fn a_tamed_horse_takes_on_a_player_who_clicks_it_turned_to_its_way() {
    for saddled in [false, true] {
        let mut region = ticking_with(&[]);
        lay(
            &mut region,
            &horse_at(entity_types::HORSE, [3.5, -60.0, 3.5], 30.0, saddled),
        );
        checked(&mut region, &interact(false));
        assert_eq!(riding(&region), Some(HORSE_KEY), "saddled {saddled}");
        let pose = region.players[&player(1)].pose;
        assert_eq!((pose.yaw, pose.pitch), (30.0, 0.0));
        let seat = feet(&region)[1];
        assert!(
            (seat - (-60.0 + 1.443_75 - 0.6)).abs() < 1e-6,
            "seated at {seat}"
        );
        let horse = record(&region);
        assert_eq!(horse.riding.passengers.len(), 1);
        assert_eq!(horse.riding.passengers[0].key, RIDER);
    }
}

/// `AbstractHorse.getControllingPassenger`: the player on a saddled horse steers it with
/// nothing in their hand, and `tickRidden` turns it to their yaw and half their pitch,
/// its body and head with it, with no motion of its own; on an unsaddled one, nobody
/// steers.
#[test]
fn a_saddled_horse_is_steered_by_its_rider_and_takes_their_rotation() {
    let mut region = ridden(entity_types::HORSE, 0.0, true);
    assert!(region.drives_root(&record(&region), RIDER));
    {
        let rider = region.players.get_mut(&player(1)).expect("player 1");
        rider.pose.yaw = 90.0;
        rider.pose.pitch = 40.0;
    }
    idle(&mut region);
    let horse = record(&region);
    assert_eq!((horse.yaw, horse.pitch), (90.0, 20.0));
    assert_eq!(horse.motion, [0.0; 3]);
    let head = mob(&region).body.head;
    assert_eq!((head.head_yaw, head.body_yaw), (90.0, 90.0));

    let region = ridden(entity_types::HORSE, 0.0, false);
    assert!(!region.drives_root(&record(&region), RIDER), "unsaddled");
}

/// `handlePlayerCommand(START_RIDING_JUMP)`: with a charge above nothing, a saddled
/// horse that its rider drives rears (`standIfPossible`, on the server whatever its AI)
/// and plays its jump sound at 0.4; a charge of nothing, or a horse with no saddle, does
/// nothing. A donkey rears too (only a llama's `canPerformRearing` is false), and its
/// jump is its own sound.
#[test]
fn the_jump_bar_rears_a_saddled_horse_and_sounds_its_kinds_jump() {
    let mut region = ridden(entity_types::HORSE, 0.0, true);
    let output = checked(&mut region, &jump(0));
    assert!(!equine::horse_of(&mob(&region)).standing);
    assert!(!sounds_of(&output).contains(&(sounds::ENTITY_HORSE_JUMP, 0.4)));
    let output = checked(&mut region, &jump(50));
    assert!(equine::horse_of(&mob(&region)).standing, "rears");
    assert!(
        sounds_of(&output).contains(&(sounds::ENTITY_HORSE_JUMP, 0.4)),
        "{:?}",
        sounds_of(&output)
    );

    let mut region = ridden(entity_types::HORSE, 0.0, false);
    let output = checked(&mut region, &jump(50));
    assert!(!equine::horse_of(&mob(&region)).standing, "no saddle");
    assert!(!sounds_of(&output).contains(&(sounds::ENTITY_HORSE_JUMP, 0.4)));

    let mut region = ridden(entity_types::DONKEY, 0.0, true);
    let output = checked(&mut region, &jump(50));
    assert!(equine::horse_of(&mob(&region)).standing, "a donkey");
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_DONKEY_JUMP, 0.4)));
}

/// `AbstractHorse.getDismountLocationForPassenger`: sneaking puts the rider off first at
/// the side of their main arm, a right one for every player: `yRot + 90`, by
/// `getCollisionHorizontalEscapeVector(horse width, rider width, yaw)`, whose length
/// along its larger axis is half the two widths and 1.0E-5. A horse facing south (yaw 0)
/// has its right to the west.
#[test]
fn sneaking_puts_the_rider_off_at_the_horses_right() {
    let mut region = ridden(entity_types::HORSE, 0.0, true);
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
    assert!(record(&region).riding.passengers.is_empty());
    let horse = EntityTypeInfo::of(entity_types::HORSE).width;
    let rider = EntityTypeInfo::of(entity_types::PLAYER).width;
    let reach = (f64::from(horse) + f64::from(rider) + 9.999_999_747_378_752e-6) / 2.0;
    let at = feet(&region);
    assert!((at[0] - (3.5 - reach)).abs() < 1e-6, "{at:?}");
    assert!((at[2] - 3.5).abs() < 1e-6, "{at:?}");
    assert_eq!(at[1], -60.0);
}

/// `AbstractHorse.causeFallDamage`: a driven horse whose driver's moves end a fall of
/// eight sounds its landing at 0.4 and is hurt by `floor((8 - 6) · 0.5) = 1` (its safe
/// fall of six and multiplier of a half), with no fall sound of a living body's; a fall
/// of three sounds its landing and costs it nothing.
#[test]
fn a_driven_horses_fall_sounds_its_landing_and_hurts_it_by_half_past_six() {
    let fall = |height: i32| {
        let mut region = ridden(entity_types::HORSE, 0.0, true);
        let [x, _, z] = record(&region).position;
        for up in 1..=height {
            checked(
                &mut region,
                &move_vehicle([x, f64::from(-60 + up), z], false),
            );
        }
        for down in (1..height).rev() {
            checked(
                &mut region,
                &move_vehicle([x, f64::from(-60 + down), z], false),
            );
        }
        let output = checked(&mut region, &move_vehicle([x, -60.0, z], true));
        (region, sounds_of(&output))
    };
    let full = health_of(entity_types::HORSE as u16);

    let (region, heard) = fall(8);
    assert!(
        heard.contains(&(sounds::ENTITY_HORSE_LAND, 0.4)),
        "{heard:?}"
    );
    assert!(
        !heard
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_GENERIC_SMALL_FALL
                || *sound == sounds::ENTITY_GENERIC_BIG_FALL)
    );
    assert_eq!(mob(&region).body.vitals.health, full - 1.0);

    let (region, heard) = fall(3);
    assert!(
        heard.contains(&(sounds::ENTITY_HORSE_LAND, 0.4)),
        "{heard:?}"
    );
    assert_eq!(mob(&region).body.vitals.health, full);
}

/// The same landing of a horse that falls by itself: dropped from eight blocks up, it
/// lands with its landing sound and is hurt by one.
#[test]
fn a_horse_that_falls_by_itself_sounds_its_landing() {
    let mut region = ticking_with(&[]);
    let mut horse = horse_at(entity_types::HORSE, [3.5, -52.0, 3.5], 0.0, false);
    horse.on_ground = false;
    horse.motion = [0.0; 3];
    lay(&mut region, &horse);
    let mut heard = Vec::new();
    for _ in 0..40 {
        let output = checked(&mut region, &TickInputs::default());
        heard.extend(sounds_of(&output));
        if record(&region).on_ground {
            break;
        }
    }
    assert!(record(&region).on_ground);
    assert!(
        heard.contains(&(sounds::ENTITY_HORSE_LAND, 0.4)),
        "{heard:?}"
    );
    assert_eq!(
        mob(&region).body.vitals.health,
        health_of(entity_types::HORSE as u16) - 1.0
    );
}
