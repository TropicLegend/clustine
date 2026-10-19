//! The world's clock in a region (`docs/groundwork/n7-plan.md`, sections 7.4 and 8.4).

use super::*;
use crate::api::WorldClock;

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

#[test]
fn a_region_s_clock_moves_on_by_one_a_tick_and_says_it_did_not_jump() {
    let mut region = on_ticking_floor_with(&[]);
    let start = region.state().clock;
    let output = idle(&mut region);
    assert_eq!(
        output.clock,
        WorldClock {
            game_time: start.game_time + 1,
            day: start.day + 1,
        }
    );
    assert!(!output.clock_jumped);
    assert_eq!(region.state().clock, output.clock);
}

#[test]
fn a_published_clock_ahead_is_taken_and_one_behind_is_not() {
    let mut region = on_ticking_floor_with(&[]);
    let now = region.state().clock;
    let ahead = WorldClock {
        game_time: now.game_time + 100,
        day: now.day + 7_000,
    };
    let output = region.tick(&TickInputs {
        clock: Some(ahead),
        ..TickInputs::default()
    });
    assert!(output.clock_jumped);
    assert_eq!(
        output.clock,
        WorldClock {
            game_time: ahead.game_time + 1,
            day: ahead.day + 1,
        }
    );
    // Behind in both, or ahead in one only: each is taken the later of, apart.
    let behind = WorldClock {
        game_time: 1,
        day: ahead.day + 50_000,
    };
    let output = region.tick(&TickInputs {
        clock: Some(behind),
        ..TickInputs::default()
    });
    assert!(output.clock_jumped);
    assert_eq!(output.clock.game_time, ahead.game_time + 2);
    assert_eq!(output.clock.day, behind.day + 1);
}

#[test]
fn a_merge_takes_the_later_clock_of_the_two() {
    let region = on_ticking_floor_with(&[]);
    let mut other = region.state();
    other.clock = WorldClock {
        game_time: 5_000,
        day: 0,
    };
    let merged = region.absorb(RegionId(9), &other);
    assert_eq!(merged.clock.game_time, 5_000);
    assert_eq!(merged.clock.day, region.state().clock.day);
}
