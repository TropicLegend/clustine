//! A player's respawn point goes with them (`docs/groundwork/p-plan.md`, section 19.7,
//! the review's first point): the bed or anchor a player set is theirs after they walk
//! into another region, in the place the store keeps when they are let go, and after a
//! leave and a join. It was forgotten at the first hand-over, so every respawn after
//! one would have been at the world spawn.

use super::n9_world::*;
use super::*;
use crate::api::Respawn;

/// A bed in the overworld, set with a look.
fn bed() -> Respawn {
    Respawn {
        pos: BlockPos::new(3, -60, 4),
        yaw: 37.5,
        pitch: -12.0,
        dimension: Dimension::Overworld,
    }
}

/// Player `number` standing at (`x`, -60, 0.5) in `region`, their respawn at [`bed`].
fn standing_with_a_bed(region: &mut Region, number: u128, x: f64) -> Vec<TickOutput> {
    let outputs = enter_with(
        region,
        number,
        entering_place(Vec3::new(x, -60.0, 0.5), Vec::new(), 0),
    );
    region
        .players
        .get_mut(&player(number))
        .expect("entered")
        .respawn = Some(bed());
    outputs
}

#[test]
fn a_bed_set_in_one_region_is_the_player_s_in_the_region_they_walk_into() {
    let mut west = west(10, clock_at(5_000));
    let mut east = east(7_777, clock_at(5_000));
    standing_with_a_bed(&mut west, 1, 14.5);
    let output = acting(
        &mut west,
        1,
        vec![PlayerInput::Move {
            position: Some(Vec3::new(17.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        }],
    );
    let [(_, transfer)] = &departures(&output)[..] else {
        panic!("one departure: {:?}", output.durable);
    };
    assert_eq!(transfer.respawn, Some(bed()), "the transfer carries it");
    let kept = last_place(std::slice::from_ref(&output), 1).expect("a place is noted");
    assert_eq!(
        kept.respawn,
        Some(bed()),
        "the store keeps it with the place"
    );
    east.tick(&changes(vec![PlayerChange::Arrive(
        EDGE,
        player(1),
        transfer.clone(),
    )]));
    let state = east.player_state(player(1)).expect("player 1 arrived");
    assert_eq!(state.respawn, Some(bed()));
}

#[test]
fn a_bed_kept_with_a_place_is_the_player_s_again_at_a_join() {
    let mut region = alone(9_000, clock_at(2_000));
    let mut place = entering_place(Vec3::new(0.5, -60.0, 0.5), Vec::new(), 0);
    place.respawn = Some(bed());
    enter_with(&mut region, 1, place);
    let state = region.player_state(player(1)).expect("player 1 joined");
    assert_eq!(state.respawn, Some(bed()));
}
