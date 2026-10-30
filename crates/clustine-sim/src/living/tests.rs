//! The living body's own rules, apart from the region: its kind's damage from what it
//! meets, the arithmetic of its friction, and the game's numbering of slots. Its
//! travel against the game's numbers is tested in the region's tests of stands.

use clustine_world::EntityId;

use super::*;
use crate::physics::entities::Mover;

fn stand_body() -> Living {
    let body = Body {
        stands_on_lava: false,
        position: [0.5, 0.0, 0.5],
        motion: [0.0; 3],
        on_ground: true,
        fall_distance: 0.0,
        width: 0.5,
        height: 1.975,
        old_position: [0.5, 0.0, 0.5],
        horizontal_collision: false,
        vertical_collision: false,
        vertical_collision_below: false,
        supporting: None,
        on_ground_no_blocks: false,
        stuck_speed: [0.0; 3],
        movements: Vec::new(),
        in_water: false,
        sync_position: false,
        falling_block: false,
        item: false,
        fell_on: None,
        noclip: None,
        mover: Mover {
            key: EntityId(1),
            root: EntityId(1),
            like_a_vehicle: false,
            happy_ghast: false,
        },
        steps: None,
    };
    Living::new(
        body,
        Kind::ArmorStand {
            health: 20.0,
            invulnerable: false,
        },
        1.7775,
    )
}

fn health(living: &Living) -> f32 {
    let Kind::ArmorStand { health, .. } = living.kind else {
        panic!("a stand's body");
    };
    health
}

#[test]
fn fire_lights_a_stand_and_then_burns_it_a_little_at_a_time_until_half_a_heart() {
    let mut living = stand_body();
    // Not burning, fire lights it for five seconds and takes nothing.
    living.hurt(Damage::InFire, 1.0);
    assert_eq!((living.fire_ticks, health(&living)), (100, 20.0));
    // Burning, fire takes 0.15 and burning 4, as floats.
    living.hurt(Damage::InFire, 1.0);
    assert_eq!(health(&living), 20.0_f32 - 0.15);
    living.hurt(Damage::OnFire, 1.0);
    assert_eq!(health(&living), 20.0_f32 - 0.15 - 4.0);
    // Down to half a heart it breaks, as by anything.
    for _ in 0..4 {
        living.hurt(Damage::OnFire, 1.0);
    }
    assert_eq!(living.end, End::Broken);
}

#[test]
fn the_void_kills_a_stand_and_lava_or_an_invisible_stand_takes_nothing() {
    let mut living = stand_body();
    living.hurt(Damage::Lava, 4.0);
    assert_eq!((living.end, health(&living)), (End::Lives, 20.0));
    living.kind = Kind::ArmorStand {
        health: 20.0,
        invulnerable: true,
    };
    living.hurt(Damage::InFire, 1.0);
    assert_eq!((living.end, living.fire_ticks), (End::Lives, 0));
    living.hurt(Damage::OutOfWorld, 4.0);
    assert_eq!(living.end, End::Killed);
}

#[test]
fn modified_friction_is_the_friction_itself_at_a_modifier_of_one_and_clamped() {
    assert_eq!(modified_friction(0.6, 1.0), 0.6);
    assert_eq!(modified_friction(0.98, 1.0), 0.98);
    assert_eq!(modified_friction(0.6, 0.0), 1.0);
    assert_eq!(modified_friction(0.6, 3.0), 0.0);
}

#[test]
fn a_slots_filter_bit_is_its_id_which_puts_the_off_hand_after_the_armour() {
    let bits: Vec<u32> = (0..8).map(slots::filter_bit).collect();
    assert_eq!(bits, [0, 5, 1, 2, 3, 4, 6, 7]);
    assert!(slots::is_hand(slots::MAIN_HAND) && slots::is_hand(slots::OFF_HAND));
    assert!(!slots::is_hand(slots::HEAD));
}
