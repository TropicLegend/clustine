//! The goals, the controls and sensing as a mob's AI steps run them.

use clustine_data::{attributes, items};
use clustine_world::{BlockPos, Entry};

use super::super::Setup;
use super::super::goals::{
    HurtByTargetGoal, Items, PanicGoal, RandomLookAroundGoal, RandomStrollGoal, Strolling,
    TemptGoal,
};
use super::super::selector::Goal;
use super::{Built, Plot, Ticked, player};
use crate::mobs::part::mind_ids;

static STROLLER: Built = Built(|setup: &mut Setup| {
    setup.add_goal(6, RandomStrollGoal::new(1.0));
});

/// The state of the slot's goal as its part.
fn slot<G: Goal>(index: u16, state: &G::State) -> Entry {
    Entry {
        id: mind_ids::FIRST_GOAL + index,
        version: G::VERSION,
        bytes: postcard::to_allocvec(state).unwrap(),
    }
}

#[test]
fn a_stroll_s_target_is_drawn_as_the_game_draws_it_and_walked_to() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &STROLLER, [0.5, 0.0, 0.5]);
    // Triggered, it draws no chance of starting.
    cow.mob.mind.slots = vec![slot::<RandomStrollGoal>(
        0,
        &Strolling {
            wanted: [0.0; 3],
            forced: true,
        },
    )];
    // `DefaultRandomPos.getPos(10, 7)` with no home: ten tries of x, y and z; on flat
    // stone only the ground's level is a stable place without a cost, and every place
    // is liked as well, so the first try at that level is the target. A world's seed
    // whose ten tries have one is taken.
    let expected = loop {
        let mut draws = cow.stream();
        let mut expected = None;
        for _ in 0..10 {
            let x = draws.next_int(21) as i32 - 10;
            let y = draws.next_int(15) as i32 - 7;
            let z = draws.next_int(21) as i32 - 10;
            if y == 0 && expected.is_none() {
                expected = Some(BlockPos::new(x, 0, z));
            }
        }
        match expected {
            Some(expected) => break expected,
            None => cow.seed += 1,
        }
    };
    let drawn = cow.step(&plot);
    assert_eq!(drawn, 30);
    assert_eq!(cow.mob.mind.goals.0.running, [0]);
    assert_eq!(navigation_target(&cow), Some(expected));
    // Walked to: a walker that goes where its move control wants ends at the target.
    for _ in 0..200 {
        cow.walk_to_wanted(&plot);
        cow.step(&plot);
        if cow.mob.body.path.followed.is_none() {
            break;
        }
    }
    let at = cow.block();
    assert!(
        (at.x - expected.x).abs() + (at.z - expected.z).abs() <= 1,
        "{at:?} for {expected:?}"
    );
}

fn navigation_target(cow: &Ticked) -> Option<BlockPos> {
    cow.mob.body.progress.target
}

static PANICKER: Built = Built(|setup: &mut Setup| {
    setup.add_goal(1, PanicGoal { speed: 2.0 });
    setup.add_target(1, HurtByTargetGoal { ignore: &[] });
});

#[test]
fn a_mob_hurt_by_a_creative_player_panics_and_takes_no_target() {
    let mut plot = Plot::flat();
    let hitter = player(9, [3.5, 0.0, 0.5], None);
    plot.players.push(hitter);
    let mut cow = Ticked::new(&plot, &PANICKER, [0.5, 0.0, 0.5]);
    cow.lived = 101;
    cow.mob.body.combat.last_hurt_by_player = Some((9, 100));
    cow.mob.body.combat.last_hurt_by = Some((clustine_world::EntityId(9), 100));
    let drawn = cow.step(&plot);
    // It runs to a place drawn by `DefaultRandomPos.getPos(5, 4)`: ten tries of three.
    assert_eq!(drawn, 30);
    assert_eq!(cow.mob.mind.goals.0.running, [0]);
    let target = navigation_target(&cow).expect("a place to run to");
    assert!((target.x).abs() <= 6 && (target.z).abs() <= 6, "{target:?}");
    assert!(cow.mob.body.path.followed.is_some());
    // A creative player is never taken as a target.
    assert_eq!(cow.mob.body.combat.target, None);
    assert!(cow.mob.mind.targets.0.running.is_empty());
}

#[test]
fn a_hurt_older_than_forty_ticks_makes_no_panic() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &PANICKER, [0.5, 0.0, 0.5]);
    cow.lived = 141;
    cow.mob.body.combat.last_hurt_by_player = Some((9, 100));
    cow.step(&plot);
    assert!(cow.mob.mind.goals.0.running.is_empty());
}

static TEMPTED: Built = Built(|setup: &mut Setup| {
    setup.add_goal(3, TemptGoal::new(1.25, Items::Of(&[items::WHEAT]), false));
});

#[test]
fn a_mob_follows_a_player_holding_its_food_within_the_tempt_range() {
    let mut plot = Plot::flat();
    plot.players
        .push(player(9, [6.5, 0.0, 0.5], Some(items::WHEAT)));
    let mut cow = Ticked::new(&plot, &TEMPTED, [0.5, 0.0, 0.5]);
    assert_eq!(cow.step(&plot), 0, "tempting draws nothing");
    assert_eq!(cow.mob.mind.goals.0.running, [0]);
    // It paths to the player's block and looks at their eyes.
    assert_eq!(navigation_target(&cow), Some(BlockPos::new(6, 0, 0)));
    let look = &cow.mob.body.controls.look;
    assert_eq!(look.wanted, [6.5, 1.62, 0.5]);
    assert_eq!(look.y_max_rot_speed, 95.0);
}

#[test]
fn a_mob_does_not_follow_food_beyond_its_tempt_range_or_other_items() {
    let mut plot = Plot::flat();
    plot.players
        .push(player(9, [11.5, 0.0, 0.5], Some(items::WHEAT)));
    plot.players
        .push(player(10, [3.5, 0.0, 0.5], Some(items::STONE)));
    let mut cow = Ticked::new(&plot, &TEMPTED, [0.5, 0.0, 0.5]);
    cow.step(&plot);
    assert!(cow.mob.mind.goals.0.running.is_empty());
    assert_eq!(cow.mob.body.path.followed, None);
}

#[test]
fn a_crouching_player_tempts_from_four_fifths_of_the_range() {
    let mut plot = Plot::flat();
    let mut crouching = player(9, [9.5, 0.0, 0.5], Some(items::WHEAT));
    crouching.sneaking = true;
    plot.players.push(crouching);
    let mut cow = Ticked::new(&plot, &TEMPTED, [0.5, 0.0, 0.5]);
    cow.step(&plot);
    assert!(cow.mob.mind.goals.0.running.is_empty());
    plot.players[0].sneaking = false;
    cow.step(&plot);
    cow.step(&plot);
    assert_eq!(cow.mob.mind.goals.0.running, [0]);
}

static LOOKER: Built = Built(|_: &mut Setup| {});

#[test]
fn the_look_control_turns_the_head_by_at_most_its_step_and_back_to_the_body() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &LOOKER, [0.5, 0.0, 0.5]);
    // Looking at a point due east (yaw -90) at ten degrees a tick.
    let look = &mut cow.mob.body.controls.look;
    look.wanted = [10.5, 1.3, 0.5];
    look.y_max_rot_speed = 10.0;
    look.x_max_rot_angle = 40.0;
    look.cooldown = 2;
    cow.step(&plot);
    assert_eq!(cow.mob.body.head.head_yaw, -10.0);
    cow.step(&plot);
    assert_eq!(cow.mob.body.head.head_yaw, -20.0);
    // The cooldown over, the head goes back towards the body at ten a tick.
    cow.step(&plot);
    assert_eq!(cow.mob.body.head.head_yaw, -10.0);
    // And the pitch is put back to nothing at every step it is not looking.
    assert_eq!(cow.pitch, 0.0);
}

#[test]
fn a_still_body_follows_its_head_after_ten_still_ticks() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &LOOKER, [0.5, 0.0, 0.5]);
    cow.mob.body.head.head_yaw = 40.0;
    // Nothing looks: the head would return to the body; keep it where it is by looking
    // at a point at its yaw each tick.
    let mut bodies = Vec::new();
    for _ in 0..21 {
        let look = &mut cow.mob.body.controls.look;
        let yaw = 40.0_f64.to_radians();
        look.wanted = [0.5 - 5.0 * yaw.sin(), 1.3, 0.5 + 5.0 * yaw.cos()];
        look.y_max_rot_speed = 0.0;
        look.x_max_rot_angle = 0.0;
        look.cooldown = 2;
        cow.step(&plot);
        bodies.push(cow.mob.body.head.body_yaw);
    }
    // The first still tick notes the head; the body waits ten ticks, then turns over the
    // next ten, the head's reach closing on it.
    assert!(bodies[..15].iter().all(|body| *body == 0.0), "{bodies:?}");
    assert!(bodies[15] > 0.0 && bodies[15] < 40.0, "{bodies:?}");
    assert_eq!(bodies[20], 40.0, "{bodies:?}");
}

static LOOKS_AROUND: Built = Built(|setup: &mut Setup| {
    setup.add_goal(7, RandomLookAroundGoal);
});

#[test]
fn a_tick_of_the_ai_is_a_function_of_its_inputs() {
    let mut plot = Plot::flat();
    plot.players
        .push(player(9, [4.5, 0.0, 2.5], Some(items::WHEAT)));
    let run = || {
        let mut cow = Ticked::new(&plot, &LOOKS_AROUND, [0.5, 0.0, 0.5]);
        let mut draws = Vec::new();
        for _ in 0..600 {
            draws.push(cow.step(&plot));
        }
        (cow.mob, cow.yaw, cow.pitch, draws)
    };
    let once = run();
    assert_eq!(once, run());
    // It looked around at least once in that time (a draw in a few hundred).
    assert!(once.3.iter().any(|drawn| *drawn > 1));
}

#[test]
fn the_tempt_range_is_the_attribute_s() {
    let plot = Plot::flat();
    let cow = Ticked::new(&plot, &TEMPTED, [0.5, 0.0, 0.5]);
    let mut draws = cow.stream();
    let tick = crate::mobs::tick::MobTick {
        key: clustine_world::EntityId(40),
        kind: clustine_data::entity_types::COW as u16,
        lived: 0,
        tick_count: 0,

        vehicle: None,

        carries: false,

        leashed: false,

        counted: false,
        mob: cow.mob.clone(),
        living: cow.living.clone(),
        yaw: 0.0,
        pitch: 0.0,
        mob_type: &TEMPTED,
        world: &plot,
        draws: &mut draws,
        out: Default::default(),
    };
    assert_eq!(
        super::super::attribute_value(&tick, attributes::TEMPT_RANGE),
        10.0
    );
    assert_eq!(
        super::super::attribute_value(&tick, attributes::MOVEMENT_SPEED),
        f64::from(0.2_f32)
    );
}
