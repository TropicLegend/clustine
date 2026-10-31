//! The shulker's rules that read nothing of the world, from the game's code (d javap
//! Shulker.txt and its inner classes), not from Clustine's: its box as its lid opens,
//! the lid's steps, its look about its face, its refusal of an arrow while closed, what
//! it shows, and its goals and limits.

use clustine_world::EntityId;

use super::{
    ATTACH_FACE_INDEX, COLOUR_INDEX, NO_COLOUR, PEEK_INDEX, SHULKER, ShulkerState, box_of, change,
    look_yaw, physical_peek, progress_box, snapped, state_of, update_peek_amount,
};
use crate::mobs::Mob;
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::mobs::types::of;
use crate::physics::aabb::Aabb;
use crate::shown::DataShown;

#[test]
fn a_closed_shulker_is_its_block_and_an_open_one_reaches_a_block_out_of_its_face() {
    let at = [0.5, 64.0, 0.5];
    let closed = ShulkerState::default();
    assert_eq!(
        box_of(&closed, at),
        Aabb {
            min: [0.0, 64.0, 0.0],
            max: [1.0, 65.0, 1.0]
        }
    );
    // Clinging to the ceiling (up), open: down into the block below.
    let hanging = ShulkerState {
        attach_face: 1,
        opening: 1.0,
        ..ShulkerState::default()
    };
    let open = box_of(&hanging, at);
    assert_eq!(open.min, [0.0, 63.0, 0.0]);
    assert_eq!(open.max, [1.0, 65.0, 1.0]);
    // Clinging to a wall to its west (4): out to the east.
    let on_a_wall = ShulkerState {
        attach_face: 4,
        opening: 1.0,
        ..ShulkerState::default()
    };
    let open = box_of(&on_a_wall, at);
    assert_eq!(open.min, [0.0, 64.0, 0.0]);
    assert_eq!(open.max, [2.0, 65.0, 1.0]);
}

#[test]
fn the_lid_carries_the_box_by_a_sine_of_its_opening() {
    // `0.5 - sin((0.5 + peek) π) / 2`: nothing shut, all of it open, half at a half.
    assert_eq!(physical_peek(0.0), 0.0);
    assert_eq!(physical_peek(1.0), 1.0);
    assert!((physical_peek(0.5) - 0.5).abs() < 1e-3);
    // The fully opened box of `canStayAt`, stretched by the peek itself: two blocks.
    let opened = progress_box(1, 1.0, [3.5, 10.0, 3.5]);
    assert_eq!(opened.min, [3.0, 10.0, 3.0]);
    assert_eq!(opened.max, [4.0, 12.0, 4.0]);
}

#[test]
fn the_lid_moves_a_twentieth_a_tick_towards_the_peek_asked_for_and_no_further() {
    let mut own = ShulkerState {
        peek: 30,
        ..ShulkerState::default()
    };
    let mut steps = 0;
    while update_peek_amount(&mut own) {
        steps += 1;
        assert!(own.opening <= 30.0_f32 * 0.01);
    }
    assert_eq!(own.opening, 30.0_f32 * 0.01);
    assert!((5..=7).contains(&steps), "{steps}");
    own.peek = 0;
    assert!(update_peek_amount(&mut own));
    assert!((own.opening - 0.25).abs() < 1e-6);
}

#[test]
fn a_shulker_is_snapped_to_its_block_s_middle_and_the_nearest_whole_height() {
    assert_eq!(snapped([3.2, 64.4, -0.7]), [3.5, 64.0, -0.5]);
    assert_eq!(snapped([3.9, 64.5, -1.0]), [3.5, 65.0, -0.5]);
}

#[test]
fn a_shulker_on_the_floor_looks_as_any_mob_and_one_on_a_wall_looks_in_the_wall_s_plane() {
    let eyes = [0.5, 64.5, 0.5];
    // On the floor: south is 0, east -90, as `yaw_towards` would have them.
    let near = |yaw: Option<f32>, wanted: f32| yaw.is_some_and(|yaw| (yaw - wanted).abs() < 1e-4);
    assert!(near(look_yaw(0, eyes, [0.5, 64.5, 5.5]), 0.0));
    assert!(near(look_yaw(0, eyes, [5.5, 64.5, 0.5]), -90.0));
    // Straight above it, nothing.
    assert_eq!(look_yaw(0, eyes, [0.5, 70.0, 0.5]), None);
    // On a wall to its west, looking out east: below it is its forward.
    assert!(near(look_yaw(4, eyes, [0.5, 60.0, 0.5]), 0.0));
    // On the ceiling, looking out down: south is still its forward's other way.
    assert!(
        near(look_yaw(1, eyes, [0.5, 64.5, 5.5]), 180.0)
            || near(look_yaw(1, eyes, [0.5, 64.5, 5.5]), -180.0)
    );
    // Straight out of the face, nothing.
    assert_eq!(look_yaw(4, eyes, [9.5, 64.5, 0.5]), None);
}

fn source_of_kind(kind: i32) -> DamageSource {
    let arrow = Causer {
        key: EntityId(9),
        kind: kind as u16,
        position: [0.0; 3],
        player: None,
        living: false,
    };
    DamageSource::thrown(arrow, [0.0, 0.0, 1.0], None)
}

#[test]
fn a_closed_shulker_refuses_an_arrow_and_an_open_one_takes_it() {
    let shulker = of(SHULKER);
    let mut closed = Mob::fresh();
    change(&mut closed, |own| own.colour = 3);
    let arrow = source_of_kind(clustine_data::entity_types::ARROW);
    assert!(shulker.refuses_hurt(&closed, &arrow));
    assert!(shulker.refuses_hurt(
        &closed,
        &source_of_kind(clustine_data::entity_types::TRIDENT)
    ));
    // A bullet or a snowball is no arrow.
    assert!(!shulker.refuses_hurt(
        &closed,
        &source_of_kind(clustine_data::entity_types::SHULKER_BULLET)
    ));
    assert!(!shulker.refuses_hurt(&closed, &DamageSource::of(DamageType::FALL)));
    let mut open = Mob::fresh();
    change(&mut open, |own| own.peek = 30);
    assert!(!shulker.refuses_hurt(&open, &arrow));
    // Its hurt sound is the closed one while closed.
    assert_eq!(
        shulker.hurt_sound(&closed, &arrow),
        Some(clustine_data::sounds::ENTITY_SHULKER_HURT_CLOSED)
    );
    assert_eq!(
        shulker.hurt_sound(&open, &arrow),
        Some(clustine_data::sounds::ENTITY_SHULKER_HURT)
    );
}

#[test]
fn a_shulker_shows_its_face_peek_and_colour_and_only_those_not_the_defaults_are_paired() {
    let mut mob = Mob::fresh();
    let shown = of(SHULKER).shown(&mob);
    assert_eq!(
        shown,
        [
            (ATTACH_FACE_INDEX, DataShown::Direction(0), true),
            (PEEK_INDEX, DataShown::Byte(0), true),
            (COLOUR_INDEX, DataShown::Byte(NO_COLOUR), true),
        ]
    );
    change(&mut mob, |own| {
        own.attach_face = 2;
        own.peek = 100;
        own.colour = 10;
    });
    assert_eq!(
        of(SHULKER).shown(&mob),
        [
            (ATTACH_FACE_INDEX, DataShown::Direction(2), false),
            (PEEK_INDEX, DataShown::Byte(100), false),
            (COLOUR_INDEX, DataShown::Byte(10), false),
        ]
    );
    // Among a mob's values, by index.
    let values = crate::shown::mob_values(SHULKER, &mob.encode());
    assert!(values.contains(&(ATTACH_FACE_INDEX, DataShown::Direction(2), false)));
}

#[test]
fn a_shulker_on_the_floor_grows_with_its_lid_and_one_elsewhere_keeps_its_size() {
    let mut mob = Mob::fresh();
    assert_eq!(of(SHULKER).dimensions(&mob), None);
    change(&mut mob, |own| own.opening = 0.5);
    let size = of(SHULKER).dimensions(&mob).expect("a size of its own");
    assert_eq!((size.width, size.height, size.eye_height), (1.0, 1.5, 0.75));
    change(&mut mob, |own| own.attach_face = 3);
    assert_eq!(of(SHULKER).dimensions(&mob), None);
}

#[test]
fn a_shulker_is_built_never_despawns_and_reads_its_face_peek_and_colour_from_a_tag() {
    assert!(crate::mobs::types::is_built(SHULKER));
    let shulker = of(SHULKER);
    assert!(!shulker.remove_when_far_away(SHULKER, &Mob::fresh(), 10_000, 1.0e6));
    assert_eq!(shulker.ambient_interval(), 120);
    use super::super::template::build;
    let tag = build::compound(&[
        build::byte("AttachFace", 5),
        build::byte("Peek", 30),
        build::byte("Color", 4),
    ]);
    let mut mob = Mob::fresh();
    shulker.from_template(&mut mob, &tag);
    let own = state_of(&mob);
    assert_eq!((own.attach_face, own.peek, own.colour), (5, 30, 4));
}
