//! Sleep in a region (`docs/groundwork/n7d-plan.md`; the game's rules in
//! `docs/groundwork/n7d-brief.md`): a bed used by day and by night, lying in it, getting
//! up, the straw bed, and the night skipped by one region for the world.

use clustine_data::BlockState;

use super::*;
use crate::api::{Respawn, Sleep, WorldPlayer};

/// A red bed whose foot is at (2, -60, 1) and head at (2, -60, 0), facing north.
const HEAD: BlockPos = BlockPos::new(2, -60, 0);
const FOOT: BlockPos = BlockPos::new(2, -60, 1);

fn bed(part: &str) -> BlockState {
    BlockState::parse(&format!("minecraft:red_bed[facing=north,part={part}]")).unwrap()
}

fn straw_bed(part: &str) -> BlockState {
    BlockState::parse(&format!("minecraft:straw_bed[facing=north,part={part}]")).unwrap()
}

/// The region with a bed beside player 1, at the day time `day`.
fn bedroom(head: BlockState, foot: BlockState, day: u64) -> Region {
    let mut region = on_ticking_floor_with(&[(HEAD, head), (FOOT, foot)]);
    region.clock.day = day;
    let player = region.players.get_mut(&player(1)).unwrap();
    player.pose.position = Vec3::new(4.5, -60.0, 0.5);
    player.pose.yaw = 90.0;
    region
}

fn use_bed(region: &mut Region, at: BlockPos) -> TickOutput {
    region.tick(&moves(vec![place(1, at.x, at.y, at.z, Face::Top)]))
}

fn messages(output: &TickOutput) -> Vec<(String, Vec<i32>, bool)> {
    output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::Message { key, args, overlay } => {
                Some((key.clone(), args.clone(), *overlay))
            }
            _ => None,
        })
        .collect()
}

fn said(key: &str, args: &[i32], overlay: bool) -> (String, Vec<i32>, bool) {
    (key.to_owned(), args.to_vec(), overlay)
}

fn sleeping(region: &Region) -> Option<Sleep> {
    region.players[&player(1)].sleeping
}

const NIGHT: u64 = 13_000;

#[test]
fn a_bed_used_by_day_sets_the_spawn_and_says_one_sleeps_only_at_night() {
    let mut region = bedroom(bed("head"), bed("foot"), 1_000);
    let output = use_bed(&mut region, FOOT);
    assert_eq!(
        messages(&output),
        [
            said("block.minecraft.set_spawn", &[], false),
            said("block.minecraft.bed.no_sleep", &[], true),
        ]
    );
    assert_eq!(
        region.players[&player(1)].respawn,
        Some(Respawn {
            pos: HEAD,
            yaw: 90.0,
            pitch: 0.0,
        })
    );
    assert_eq!(sleeping(&region), None);
    // The same bed again says nothing of the spawn.
    let output = use_bed(&mut region, HEAD);
    assert_eq!(
        messages(&output),
        [said("block.minecraft.bed.no_sleep", &[], true)]
    );
}

#[test]
fn at_night_the_player_lies_in_the_bed_which_is_taken_and_alone_they_skip_the_night() {
    let mut region = bedroom(bed("head"), bed("foot"), NIGHT);
    let output = use_bed(&mut region, FOOT);
    let lying = region.players[&player(1)].pose.position;
    assert_eq!(lying, Vec3::new(2.5, -60.0 + 0.6875, 0.5));
    assert!(sleeping(&region).is_some_and(|sleep| sleep.head == HEAD));
    assert!(clustine_blocks::data::flag(
        region.block(HEAD).unwrap(),
        "occupied"
    ));
    assert!(clustine_blocks::data::flag(
        region.block(FOOT).unwrap(),
        "occupied"
    ));
    assert!(messages(&output).contains(&said("sleep.skipping_night", &[], true)));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Sleeping { head: Some(head), .. } if *head == HEAD
    )));
    // Deep after 100 ticks: the night goes in the tick that makes it 100.
    for _ in 0..99 {
        let output = region.tick(&TickInputs::default());
        assert!(!output.clock_jumped);
        assert!(sleeping(&region).is_some());
    }
    let output = region.tick(&TickInputs::default());
    assert!(output.clock_jumped);
    assert_eq!(region.clock.day, 24_000);
    assert_eq!(output.delta.clock, Some(region.clock));
    assert_eq!(sleeping(&region), None);
    assert!(messages(&output).is_empty(), "{:?}", messages(&output));
    assert!(!clustine_blocks::data::flag(
        region.block(HEAD).unwrap(),
        "occupied"
    ));
    // Up beside the bed, facing it.
    let stood = region.players[&player(1)].pose;
    assert_ne!(stood.position, lying);
    assert_eq!(stood.pitch, 0.0);
}

#[test]
fn the_leave_bed_input_stands_the_player_up_and_frees_the_bed() {
    let mut region = bedroom(bed("head"), bed("foot"), NIGHT);
    use_bed(&mut region, HEAD);
    let output = region.tick(&moves(vec![numbered(1, PlayerInput::LeaveBed)]));
    assert_eq!(sleeping(&region), None);
    assert!(!clustine_blocks::data::flag(
        region.block(HEAD).unwrap(),
        "occupied"
    ));
    // Alone in bed and out of it: no sleeper is left to count.
    assert_eq!(
        messages(&output),
        [said("sleep.players_sleeping", &[0, 1], true)]
    );
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::Animated { action: 0, .. }))
    );
}

#[test]
fn a_bed_too_far_obstructed_or_taken_is_not_slept_in() {
    let mut region = bedroom(bed("head"), bed("foot"), NIGHT);
    region.players.get_mut(&player(1)).unwrap().pose.position = Vec3::new(6.5, -60.0, 0.5);
    let output = use_bed(&mut region, HEAD);
    assert_eq!(
        messages(&output),
        [said("block.minecraft.bed.too_far_away", &[], true)]
    );
    let mut region = on_ticking_floor_with(&[
        (HEAD, bed("head")),
        (FOOT, bed("foot")),
        (BlockPos::new(2, -59, 1), clustine_data::blocks::STONE),
    ]);
    region.clock.day = NIGHT;
    let output = use_bed(&mut region, HEAD);
    assert_eq!(
        messages(&output),
        [said("block.minecraft.bed.obstructed", &[], true)]
    );
    let taken = bed("head").with_property("occupied", "true").unwrap();
    let mut region = bedroom(taken, bed("foot"), NIGHT);
    let output = use_bed(&mut region, FOOT);
    assert_eq!(
        messages(&output),
        [said("block.minecraft.bed.occupied", &[], true)]
    );
}

#[test]
fn a_straw_bed_never_sets_the_spawn_and_breaks_when_its_sleeper_gets_up() {
    let mut region = bedroom(straw_bed("head"), straw_bed("foot"), NIGHT);
    use_bed(&mut region, HEAD);
    assert!(sleeping(&region).is_some());
    assert_eq!(region.players[&player(1)].respawn, None);
    assert_eq!(
        region.players[&player(1)].pose.position,
        Vec3::new(2.5, -60.0 + 0.375, 0.5)
    );
    region.tick(&moves(vec![numbered(1, PlayerInput::LeaveBed)]));
    assert_eq!(region.block(HEAD), Some(clustine_data::blocks::AIR));
    assert_eq!(region.block(FOOT), Some(clustine_data::blocks::AIR));
}

/// Another player of the world, `n`, as the store publishes them.
fn elsewhere(n: u128, sleeping: Option<u32>) -> WorldPlayer {
    WorldPlayer {
        player: player(n),
        entity: EntityId(1000 + n as u64),
        position: [500.5, -60.0, 0.5],
        game_mode: 1,
        sleeping,
    }
}

#[test]
fn the_world_s_players_count_and_both_must_sleep_deeply_at_one_hundred_percent() {
    let mut region = bedroom(bed("head"), bed("foot"), NIGHT);
    let output = region.tick(&TickInputs {
        world_players: Some(vec![elsewhere(2, None)]),
        ..moves(vec![place(1, HEAD.x, HEAD.y, HEAD.z, Face::Top)])
    });
    assert!(messages(&output).contains(&said("sleep.players_sleeping", &[1, 2], true)));
    for _ in 0..120 {
        assert!(!region.tick(&TickInputs::default()).clock_jumped);
    }
    // The other lies down, but is not deep yet; then is.
    let output = region.tick(&TickInputs {
        world_players: Some(vec![elsewhere(2, Some(40))]),
        ..TickInputs::default()
    });
    assert!(!output.clock_jumped);
    assert!(messages(&output).contains(&said("sleep.skipping_night", &[], true)));
    let output = region.tick(&TickInputs {
        world_players: Some(vec![elsewhere(2, Some(100))]),
        ..TickInputs::default()
    });
    assert!(output.clock_jumped);
}

#[test]
fn a_morning_taken_from_the_store_wakes_the_region_s_sleepers_without_a_word() {
    let mut region = bedroom(bed("head"), bed("foot"), NIGHT);
    use_bed(&mut region, HEAD);
    let mut morning = region.clock;
    morning.day = 24_000;
    let output = region.tick(&TickInputs {
        clock: Some(morning),
        ..TickInputs::default()
    });
    assert_eq!(sleeping(&region), None);
    assert!(messages(&output).is_empty(), "{:?}", messages(&output));
}

#[test]
fn a_region_without_a_sleeper_of_its_own_does_not_skip_the_night() {
    let mut region = bedroom(bed("head"), bed("foot"), NIGHT);
    let output = region.tick(&TickInputs {
        world_players: Some(vec![elsewhere(2, Some(100))]),
        ..TickInputs::default()
    });
    assert!(!output.clock_jumped);
}
