//! A player's arm swinging for everyone else: a punch with what the main hand holds, and
//! the swing after a use or a placing that succeeded, as the game's `handlePunch` and
//! `handleUseItemOn` send `ClientboundSwingAnimationPacket`.

use clustine_data::Swing;

use super::redstone::{block_at, lever};
use super::*;

/// The swings a tick reported: the entity, the off hand, the swing and whether the
/// player's own client is shown it too.
fn swings(output: &TickOutput) -> Vec<(EntityId, bool, Swing, bool)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::ArmSwung {
                entity,
                off_hand,
                swing,
                own,
                ..
            } => Some((*entity, *off_hand, *swing, *own)),
            _ => None,
        })
        .collect()
}

fn entity_of(region: &Region) -> EntityId {
    region.player(player(1)).expect("player 1 is here").0
}

fn punch() -> TickInputs {
    moves(vec![numbered(1, PlayerInput::Punch)])
}

#[test]
fn a_punch_swings_the_main_hand_for_the_others_with_the_default_whack() {
    let mut region = on_ticking_floor_with(&[]);
    let entity = entity_of(&region);
    let output = region.tick(&punch());
    assert_eq!(swings(&output), [(entity, false, Swing::DEFAULT, false)]);
    assert_eq!(
        Swing::DEFAULT,
        Swing {
            kind: Swing::WHACK,
            duration: 6
        }
    );
}

#[test]
fn a_punch_with_a_spear_in_hand_stabs_for_as_long_as_the_spear_says() {
    let mut region = on_ticking_floor_with(&[]);
    let entity = entity_of(&region);
    let spear = ItemStack::new(items::IRON_SPEAR, 1);
    region.tick(&moves(vec![set_slot(1, 0, Some(spear)), select(1, 0)]));
    let output = region.tick(&punch());
    let stab = Swing {
        kind: Swing::STAB,
        duration: 19,
    };
    assert_eq!(swings(&output), [(entity, false, stab, false)]);
}

#[test]
fn placing_a_block_swings_the_arm_once_for_the_others() {
    let mut region = on_ticking_floor_with(&[]);
    let entity = entity_of(&region);
    region.tick(&moves(vec![set_slot(1, 0, Some(STONE)), select(1, 0)]));
    let output = region.tick(&moves(vec![place(1, 2, -61, 0, Face::Top)]));
    assert_eq!(
        block_at(&region, BlockPos::new(2, -60, 0)),
        Some(blocks::STONE)
    );
    assert_eq!(swings(&output), [(entity, false, Swing::DEFAULT, false)]);
}

#[test]
fn flipping_a_lever_swings_the_arm_and_a_click_that_does_nothing_does_not() {
    let at = BlockPos::new(2, -60, 0);
    let mut region = on_ticking_floor_with(&[(at, lever(false))]);
    let entity = entity_of(&region);
    region.tick(&moves(vec![set_slot(1, 0, None), select(1, 0)]));
    let output = region.tick(&moves(vec![place(1, at.x, at.y, at.z, Face::Top)]));
    assert_eq!(block_at(&region, at), Some(lever(true)));
    assert_eq!(swings(&output), [(entity, false, Swing::DEFAULT, false)]);
    // An empty hand on the floor uses nothing and places nothing.
    let output = region.tick(&moves(vec![place(1, 1, -61, 1, Face::Top)]));
    assert_eq!(swings(&output), []);
}

#[test]
fn a_swing_while_the_last_is_not_yet_half_done_shows_nothing_but_one_in_the_same_tick_does() {
    let mut region = on_ticking_floor_with(&[]);
    let entity = entity_of(&region);
    let whack = (entity, false, Swing::DEFAULT, false);
    // Two in one tick: the second starts the swing again (`startIfAble` at 0 ticks).
    let twice = moves(vec![
        numbered(1, PlayerInput::Punch),
        numbered(1, PlayerInput::Punch),
    ]);
    assert_eq!(swings(&region.tick(&twice)), [whack, whack]);
    // Six ticks long: at one, two and three ticks it is not more than half done.
    for _ in 1..=3 {
        assert_eq!(swings(&region.tick(&punch())), []);
    }
    // At four it is: a refused swing does not begin one.
    assert_eq!(swings(&region.tick(&punch())), [whack]);
    // And once it is over, after six, any swing begins.
    for _ in 1..=6 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(swings(&region.tick(&punch())), [whack]);
}
