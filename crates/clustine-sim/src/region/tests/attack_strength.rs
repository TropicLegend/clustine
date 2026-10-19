//! A player's attack strength (`docs/groundwork/n7-plan.md`, section 7.7): the ticks
//! since its last reset over twenty by the attack speed of what the main hand holds, as
//! `getAttackStrengthScale(0.5f)` reads it in the inputs of a tick.

use super::*;

fn strength(region: &Region) -> f32 {
    region.attack_strength(player(1))
}

#[test]
fn a_punch_resets_the_strength_which_comes_back_a_fifth_a_tick_with_a_bare_hand() {
    let mut region = on_ticking_floor_with(&[]);
    for _ in 0..10 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(strength(&region), 1.0);
    region.tick(&moves(vec![numbered(1, PlayerInput::Punch)]));
    // Read in this tick's inputs: none since; then one tick more each.
    assert_eq!(strength(&region), 0.5 / 5.0);
    region.tick(&TickInputs::default());
    assert_eq!(strength(&region), 1.5 / 5.0);
    for _ in 0..3 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(strength(&region), 4.5 / 5.0);
    region.tick(&TickInputs::default());
    assert_eq!(strength(&region), 1.0);
}

#[test]
fn a_sword_takes_longer_and_a_change_of_item_resets_the_strength_for_the_next_tick() {
    let mut region = on_ticking_floor_with(&[]);
    let sword = ItemStack {
        item: items::IRON_SWORD,
        count: 1,
    };
    region.tick(&moves(vec![set_slot(1, 0, Some(sword)), select(1, 0)]));
    // Seen after the inputs: the next tick's read none.
    region.tick(&TickInputs::default());
    let first = strength(&region);
    // A sword's speed is 1.6: twenty over it is 12.5 ticks.
    assert!((first - 0.5 / 12.5).abs() < 1e-6, "{first}");
    region.tick(&TickInputs::default());
    assert!((strength(&region) - 1.5 / 12.5).abs() < 1e-6);
    // The same item again changes nothing.
    region.tick(&moves(vec![set_slot(2, 0, Some(sword))]));
    assert!((strength(&region) - 2.5 / 12.5).abs() < 1e-6);
}

#[test]
fn a_hand_over_carries_the_ticks_since_the_reset_to_the_next_region() {
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&moves(vec![numbered(1, PlayerInput::Punch)]));
    region.tick(&TickInputs::default());
    region.tick(&TickInputs::default());
    let tick = region.tick_number();
    let player = region.players.remove(&player(1)).expect("player 1 is here");
    // Let go at the end of this tick: the next region's first inputs read three.
    let transfer = player.into_transfer(tick);
    assert_eq!(transfer.attack_ticks, Some(3));
}

#[test]
fn a_waiting_player_commits_nothing_for_their_strength() {
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&moves(vec![numbered(1, PlayerInput::Punch)]));
    let output = region.tick(&TickInputs::default());
    assert!(output.delta.changes_only_the_tick(), "{:?}", output.delta);
}
