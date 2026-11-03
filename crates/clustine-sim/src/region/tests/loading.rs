//! What a player's mode refuses of their client (`handleSetCreativeModeSlot @16`,
//! `handlePlayerAbilities @13-44`), asked again by the region whatever the edge passed
//! on, and the client's loading, before which nothing hurts the player
//! (`ServerPlayer.isInvulnerableTo @26-33`, `hasClientLoaded`;
//! `docs/groundwork/v-plan.md`, sections 2.1 and 4.1).

use clustine_world::{EntityChange, GameMode, PlayerMode};

use super::*;

/// Puts player `number` of `region` in `mode`.
fn in_mode(region: &mut Region, number: u128, mode: GameMode) {
    region
        .players
        .get_mut(&player(number))
        .expect("the player is here")
        .mode = PlayerMode::new(mode);
}

fn inventory_slot(region: &Region, slot: usize) -> Option<ItemStack> {
    region.state().players[&player(1)].inventory.0[slot].clone()
}

fn flying(region: &Region) -> bool {
    region.state().players[&player(1)].flying
}

/// Whether a tick made a whole entity: a thrown item.
fn made_an_entity(output: &TickOutput) -> bool {
    output
        .entity_changes
        .iter()
        .any(|(_, change)| matches!(change, EntityChange::Whole(_)))
}

/// The health a tick told player 1, if it told one: what a hurt tells.
fn hurt(output: &TickOutput) -> Option<f32> {
    output
        .player_events
        .iter()
        .find_map(|(id, event)| match event {
            PlayerEvent::Health { health } if *id == player(1) => Some(*health),
            _ => None,
        })
}

/// Player 1 moved far below the overworld's floor, into the void.
fn into_the_void() -> TickInputs {
    let input = PlayerInput::Move {
        position: Some(Vec3::new(0.5, -200.0, 0.5)),
        rotation: None,
        on_ground: false,
    };
    moves(vec![numbered(1, input)])
}

/// The ticks from `region`'s next until the void first hurts player 1, at most `limit`.
fn ticks_until_hurt(region: &mut Region, first: &TickInputs, limit: u64) -> Option<u64> {
    if hurt(&region.tick(first)).is_some() {
        return Some(0);
    }
    (1..limit).find(|_| hurt(&idle(region)).is_some())
}

#[test]
fn a_survival_player_s_creative_slot_sets_nothing_and_a_creative_player_s_sets_it() {
    let mut region = joined(&[1]);
    in_mode(&mut region, 1, GameMode::Survival);
    let set = || {
        numbered(
            1,
            PlayerInput::SetInventorySlot {
                slot: 4,
                stack: Some(STONE),
            },
        )
    };
    region.tick(&moves(vec![set()]));
    assert_eq!(inventory_slot(&region, 4), None);

    in_mode(&mut region, 1, GameMode::Creative);
    region.tick(&moves(vec![set()]));
    assert_eq!(inventory_slot(&region, 4), Some(STONE));

    // Nor does a survival player's empty a slot.
    in_mode(&mut region, 1, GameMode::Survival);
    let empty = PlayerInput::SetInventorySlot {
        slot: 4,
        stack: None,
    };
    region.tick(&moves(vec![numbered(1, empty)]));
    assert_eq!(inventory_slot(&region, 4), Some(STONE));
}

#[test]
fn a_survival_player_throws_nothing_out_of_the_creative_inventory() {
    let mut region = on_ticking_floor_with(&[]);
    in_mode(&mut region, 1, GameMode::Survival);
    let throw = || numbered(1, PlayerInput::Throw { stack: DIRT });
    assert!(!made_an_entity(&region.tick(&moves(vec![throw()]))));

    in_mode(&mut region, 1, GameMode::Creative);
    assert!(made_an_entity(&region.tick(&moves(vec![throw()]))));
}

#[test]
fn a_player_whose_mode_may_not_fly_does_not_fly_whatever_their_client_says() {
    let mut region = joined(&[1]);
    in_mode(&mut region, 1, GameMode::Survival);
    let fly = |flying| numbered(1, PlayerInput::SetFlying { flying });
    let output = region.tick(&moves(vec![fly(true)]));
    assert!(!flying(&region));
    // Nobody is cut off or told anything for it.
    assert!(
        output.player_events.is_empty(),
        "{:?}",
        output.player_events
    );

    in_mode(&mut region, 1, GameMode::Creative);
    region.tick(&moves(vec![fly(true)]));
    assert!(flying(&region));
    region.tick(&moves(vec![fly(false)]));
    assert!(!flying(&region));
}

#[test]
fn a_joined_player_is_not_hurt_by_the_void_before_their_client_has_loaded() {
    let mut region = joined(&[1]);
    // The join's tick and the entering's have passed; the timer runs 60 ticks from
    // the entering.
    let hurt_after = ticks_until_hurt(&mut region, &into_the_void(), 70);
    assert_eq!(hurt_after, Some(59));
}

#[test]
fn a_client_that_says_it_has_loaded_lets_the_void_hurt_its_player_at_once() {
    let mut region = joined(&[1]);
    let mut inputs = into_the_void();
    inputs.inputs.push(numbered(1, PlayerInput::Loaded));
    assert_eq!(ticks_until_hurt(&mut region, &inputs, 5), Some(0));
}

#[test]
fn a_client_that_has_not_loaded_counts_as_loaded_after_the_timeout_whatever_it_moves() {
    let mut region = joined(&[1]);
    // 30 ticks spent standing, then into the void: hurt 30 ticks later, when the
    // 60 have run out since the entering.
    for _ in 0..29 {
        idle(&mut region);
    }
    assert_eq!(
        ticks_until_hurt(&mut region, &into_the_void(), 70),
        Some(30)
    );
}

#[test]
fn a_player_handed_over_from_another_region_has_loaded() {
    // Named in `loading.rs`: what the region keeps of the loading is in memory only.
    let joined = joined(&[1]);
    assert!(!joined.client_loaded(player(1)));
    let mut other = region_in(ChunkArea::EVERYWHERE);
    let arrival = TickInputs {
        player_changes: vec![PlayerChange::Arrive(
            EDGE,
            player(1),
            transfer(1, 1, 0.5, 0),
        )],
        ..TickInputs::default()
    };
    other.tick(&arrival);
    assert!(other.client_loaded(player(1)));
}
