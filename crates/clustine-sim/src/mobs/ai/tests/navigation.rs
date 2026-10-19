//! The navigation (the brief's 3.3 to 3.6): making paths, keeping them, making them
//! again when a block near them changes, and giving up when stuck.

use clustine_data::blocks;
use clustine_world::BlockPos;

use super::super::Setup;
use super::super::navigation::{self, Made};
use super::{Built, Plot, Ticked};

static PLAIN: Built = Built(|_: &mut Setup| {});

#[test]
fn a_walker_makes_no_path_to_a_target_in_a_chunk_neither_held_nor_copied() {
    let mut plot = Plot::flat();
    plot.edge = 16;
    let mut cow = Ticked::new(&plot, &PLAIN, [10.5, 0.0, 0.5]);
    let made = cow.with_ai(&plot, |ai| {
        let made = navigation::create_path(ai, BlockPos::new(20, 0, 0), 1);
        (made, navigation::move_to(ai, [20.5, 0.0, 0.5], 1.0))
    });
    assert_eq!(made, (None, false));
    assert_eq!(cow.mob.body.path.followed, None);
}

#[test]
fn a_walker_s_target_in_the_air_is_taken_down_to_the_ground() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &PLAIN, [0.5, 0.0, 0.5]);
    cow.with_ai(&plot, |ai| navigation::move_to(ai, [4.5, 5.0, 0.5], 1.0));
    assert_eq!(cow.mob.body.progress.target, Some(BlockPos::new(4, 0, 0)));
}

#[test]
fn a_path_asked_for_again_to_the_same_target_is_kept_with_its_progress() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &PLAIN, [0.5, 0.0, 0.5]);
    cow.with_ai(&plot, |ai| navigation::move_to(ai, [8.5, 0.0, 0.5], 1.0));
    cow.mob.body.progress.next = 2;
    let made = cow.with_ai(&plot, |ai| {
        let made = navigation::create_path_at(ai, [8.5, 0.0, 0.5], 1);
        let kept = made == Some(Made::Current);
        (kept, navigation::move_to_path(ai, made, 1.5))
    });
    assert_eq!(made, (true, true));
    assert_eq!(cow.mob.body.progress.next, 2);
    assert_eq!(cow.mob.body.progress.speed, 1.5);
}

#[test]
fn a_block_changed_near_a_path_makes_it_again_and_one_far_away_does_not() {
    let mut plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &PLAIN, [0.5, 0.0, 0.5]);
    cow.with_ai(&plot, |ai| navigation::move_to(ai, [8.5, 0.0, 0.5], 1.0));
    let before = cow.mob.body.path.followed.clone().expect("a path");
    assert!(before.nodes.iter().any(|node| (node.x, node.z) == (4, 0)));
    // Far away: nothing.
    plot.set(40, 0, 40, blocks::STONE);
    cow.with_ai(&plot, |ai| {
        navigation::block_changed(ai, BlockPos::new(40, 0, 40))
    });
    assert_eq!(cow.mob.body.path.followed.as_ref(), Some(&before));
    // On the path: made again round it, at once (no recompute has been made yet).
    plot.set(4, 0, 0, blocks::STONE);
    plot.set(4, 1, 0, blocks::STONE);
    cow.with_ai(&plot, |ai| {
        navigation::block_changed(ai, BlockPos::new(4, 0, 0))
    });
    let after = cow.mob.body.path.followed.clone().expect("a path");
    assert!(
        !after
            .nodes
            .iter()
            .any(|node| (node.x, node.y, node.z) == (4, 0, 0))
    );
    assert_eq!(cow.mob.body.progress.last_recompute, Some(cow.lived));
    // Another change within twenty ticks waits for the navigation's next ticks.
    plot.set(5, 0, 1, blocks::STONE);
    cow.with_ai(&plot, |ai| {
        navigation::block_changed(ai, BlockPos::new(5, 0, 1))
    });
    assert!(cow.mob.body.progress.delayed_recompute);
}

#[test]
fn a_mob_that_has_not_moved_in_a_hundred_navigation_ticks_is_stuck_and_stops() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &PLAIN, [0.5, 0.0, 0.5]);
    cow.with_ai(&plot, |ai| navigation::move_to(ai, [8.5, 0.0, 0.5], 1.0));
    for _ in 0..100 {
        cow.step(&plot);
    }
    assert!(cow.mob.body.path.followed.is_some());
    assert!(!cow.mob.body.progress.stuck);
    cow.step(&plot);
    assert!(cow.mob.body.path.followed.is_none());
    assert!(cow.mob.body.progress.stuck);
}

#[test]
fn a_pathing_mob_s_move_control_is_aimed_at_its_next_node_and_its_head_kept_near_its_body() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &PLAIN, [0.5, 0.0, 0.5]);
    cow.mob.body.head.head_yaw = 120.0;
    cow.with_ai(&plot, |ai| navigation::move_to(ai, [8.5, 0.0, 0.5], 1.0));
    cow.step(&plot);
    // At its own node, it aims at the next one's middle, on the ground.
    assert_eq!(cow.mob.body.controls.movement.wanted, [1.5, 0.0, 0.5]);
    // Its yaw turns towards it, at most 90 a tick and within 0 to 360.
    assert_eq!(cow.yaw, 270.0);
    // And its speed is the speed asked times its movement speed.
    assert_eq!(cow.mob.body.inputs.speed, (1.0 * f64::from(0.2_f32)) as f32);
    let head = cow.mob.body.head.head_yaw;
    let body = cow.mob.body.head.body_yaw;
    let off = super::super::maths::wrap_degrees(head - body);
    assert!(off.abs() <= 75.0, "{head} {body}");
}
