//! Tests of the telling of global level events from the game's rule
//! (`ServerLevel.globalLevelEvent`; `docs/groundwork/p-plan.md`, sections 19.5 item 8
//! and 19.6): where a player hears one, and that every player a region holds is told
//! each published event once, in the order of its number. The worlds are those of the
//! tests of TNT (`tnt/tests.rs`), in the overworld.

use clustine_world::{BlockPos, Dimension, PlayerId};

use super::super::tnt::tests::{add_player, idle, world};
use super::heard_at;
use crate::api::{GlobalLevelEvent, PlayerEvent, PublishedVillage, TickInputs, TickOutput};

/// An end portal's opening (level event 1038) at `position` in `dimension`.
fn opened(dimension: Dimension, position: BlockPos) -> GlobalLevelEvent {
    GlobalLevelEvent {
        event: 1038,
        dimension,
        position,
        data: 0,
    }
}

/// The global events the tick told, by player, in order.
fn told(output: &TickOutput) -> Vec<(PlayerId, i32, BlockPos)> {
    output
        .player_events
        .iter()
        .filter_map(|(id, event)| match event {
            PlayerEvent::GlobalLevelEvent {
                event, position, ..
            } => Some((*id, *event, *position)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_player_within_thirty_two_blocks_hears_a_global_event_at_its_block() {
    let at = BlockPos::new(10, 64, 10);
    let event = opened(Dimension::Overworld, at);
    assert_eq!(heard_at(event, true, [0.5, 64.0, 0.5]), at);
}

#[test]
fn a_player_further_away_hears_it_thirty_two_blocks_towards_it() {
    // The centre is (100.5, 64.5, 0.5); from (0.5, 64.5, 0.5) that is straight along x.
    let event = opened(Dimension::Overworld, BlockPos::new(100, 64, 0));
    assert_eq!(
        heard_at(event, true, [0.5, 64.5, 0.5]),
        BlockPos::new(32, 64, 0)
    );
}

#[test]
fn a_player_in_another_level_hears_it_where_they_stand() {
    let event = opened(Dimension::End, BlockPos::new(0, 64, 0));
    assert_eq!(
        heard_at(event, false, [-3.2, 70.9, 5.5]),
        BlockPos::new(-4, 70, 5)
    );
}

#[test]
fn every_player_is_told_each_published_event_once_in_the_order_of_its_number() {
    let mut region = world(5, |_| None);
    let (first, _) = add_player(&mut region, 1, [0.5, -60.0, 0.5], false);
    let (second, _) = add_player(&mut region, 2, [4.5, -60.0, 4.5], false);
    idle(&mut region);
    let near = BlockPos::new(2, -60, 2);
    let far = BlockPos::new(2, -60, 1000);
    let elsewhere = BlockPos::new(0, 64, 0);
    let village = PublishedVillage {
        // Out of order, and with one number twice as two publications taken as one
        // could carry it.
        global_events: vec![
            (7, opened(Dimension::Overworld, far)),
            (6, opened(Dimension::Overworld, near)),
            (8, opened(Dimension::End, elsewhere)),
            (7, opened(Dimension::Overworld, far)),
        ],
        ..PublishedVillage::default()
    };
    let output = region.tick(&TickInputs {
        village: Some(village),
        ..TickInputs::default()
    });
    assert_eq!(
        told(&output),
        [
            (first, 1038, near),
            (second, 1038, near),
            (first, 1038, BlockPos::new(0, -60, 32)),
            (second, 1038, BlockPos::new(4, -60, 36)),
            (first, 1038, BlockPos::new(0, -60, 0)),
            (second, 1038, BlockPos::new(4, -60, 4)),
        ]
    );
    assert!(told(&idle(&mut region)).is_empty(), "told once");
}
