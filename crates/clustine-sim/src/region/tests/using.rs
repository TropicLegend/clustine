//! A player's use of an item over time and their cooldowns
//! (`docs/groundwork/n9-plan.md`, section 5.5; `region/using.rs`), from the game's
//! `LivingEntity` use methods and `ItemCooldowns`.

use super::*;
use crate::api::{Cooldown, Hand, Using};
use crate::shown::DataShown;

fn use_item(number: u128) -> Input {
    numbered(
        number,
        PlayerInput::UseItem {
            rotation: (0.0, 0.0),
            sneaking: false,
            sequence: 1,
        },
    )
}

fn using(region: &Region) -> Option<Using> {
    region
        .players
        .get(&player(1))
        .and_then(|player| player.using)
}

/// The living flags the tick showed the others of player 1's entity.
fn flags_shown(output: &TickOutput) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::PlayerData { values, .. } => {
                values
                    .iter()
                    .find_map(|(index, value)| match (index, value) {
                        (8, DataShown::Byte(flags)) => Some(*flags),
                        _ => None,
                    })
            }
            _ => None,
        })
        .collect()
}

/// A region with player 1 holding `item` in the first slot of the hotbar.
fn holding(item: i32) -> Region {
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&moves(vec![
        set_slot(1, 0, Some(ItemStack::new(item, 1))),
        select(1, 0),
    ]));
    region
}

#[test]
fn a_bow_drawn_is_in_use_until_it_is_let_go_and_the_others_are_shown_so() {
    let mut region = holding(items::BOW);
    let output = region.tick(&moves(vec![use_item(1)]));
    // Begun in the inputs and ticked once after them, as `Player.tick` ticks it.
    assert_eq!(
        using(&region),
        Some(Using {
            hand: Hand::Main,
            item: items::BOW,
            remaining: 72_000 - 1,
        })
    );
    assert_eq!(flags_shown(&output), [1]);
    region.tick(&TickInputs::default());
    assert_eq!(
        using(&region).map(|using| using.remaining),
        Some(72_000 - 2)
    );
    let output = region.tick(&moves(vec![numbered(1, PlayerInput::ReleaseUse)]));
    assert_eq!(using(&region), None);
    assert_eq!(flags_shown(&output), [0]);
}

#[test]
fn a_use_with_the_off_hand_shows_the_off_hand_flag() {
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&moves(vec![set_slot(
        1,
        u8::try_from(clustine_world::OFF_HAND).unwrap(),
        Some(ItemStack::new(items::SHIELD, 1)),
    )]));
    let off = PlayerInput::WithOffHand(Box::new(PlayerInput::UseItem {
        rotation: (0.0, 0.0),
        sneaking: false,
        sequence: 1,
    }));
    let output = region.tick(&moves(vec![numbered(1, off)]));
    assert_eq!(using(&region).map(|using| using.hand), Some(Hand::Off));
    assert_eq!(flags_shown(&output), [1 | 2]);
}

#[test]
fn a_change_of_slot_or_of_the_item_in_the_hand_ends_the_use() {
    let mut region = holding(items::BOW);
    region.tick(&moves(vec![use_item(1)]));
    region.tick(&moves(vec![select(1, 1)]));
    assert_eq!(using(&region), None);

    let mut region = holding(items::BOW);
    region.tick(&moves(vec![use_item(1)]));
    let output = region.tick(&moves(vec![set_slot(
        1,
        0,
        Some(ItemStack::new(items::STONE, 1)),
    )]));
    assert_eq!(using(&region), None);
    assert_eq!(flags_shown(&output), [0]);
}

#[test]
fn a_meal_lasts_its_consume_seconds_and_ends_by_itself() {
    // An apple's `consume_seconds` are 1.6: 32 ticks, the first of them in the tick it
    // is begun.
    let mut region = holding(items::APPLE);
    region.tick(&moves(vec![use_item(1)]));
    for _ in 0..30 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(using(&region).map(|using| using.remaining), Some(1));
    let output = region.tick(&TickInputs::default());
    assert_eq!(using(&region), None);
    assert_eq!(flags_shown(&output), [0]);
}

#[test]
fn an_item_whose_use_starts_none_leaves_the_player_without_a_use() {
    let mut region = holding(items::STONE);
    region.tick(&moves(vec![use_item(1)]));
    assert_eq!(using(&region), None);
}

#[test]
fn a_cooldown_holds_its_group_until_its_end_and_the_client_is_told_both() {
    let mut region = holding(items::ENDER_PEARL);
    let pearl = ItemStack::new(items::ENDER_PEARL, 1);
    let mut output = TickOutput::default();
    region.add_cooldown(player(1), &pearl, 20, &mut output);
    assert_eq!(
        output.player_events,
        [(
            player(1),
            PlayerEvent::Cooldown {
                group: "minecraft:ender_pearl".to_owned(),
                ticks: 20
            }
        )]
    );
    for _ in 0..19 {
        let output = region.tick(&TickInputs::default());
        assert!(region.is_on_cooldown(player(1), &pearl));
        assert!(
            output
                .player_events
                .iter()
                .all(|(_, event)| !matches!(event, PlayerEvent::Cooldown { .. }))
        );
    }
    let output = region.tick(&TickInputs::default());
    assert!(!region.is_on_cooldown(player(1), &pearl));
    assert!(output.player_events.contains(&(
        player(1),
        PlayerEvent::Cooldown {
            group: "minecraft:ender_pearl".to_owned(),
            ticks: 0
        }
    )));
}

#[test]
fn a_hand_over_carries_the_use_and_what_each_cooldown_has_run() {
    let mut region = holding(items::BOW);
    region.tick(&moves(vec![use_item(1)]));
    let pearl = ItemStack::new(items::ENDER_PEARL, 1);
    region.add_cooldown(player(1), &pearl, 20, &mut TickOutput::default());
    for _ in 0..4 {
        region.tick(&TickInputs::default());
    }
    let tick = region.tick_number();
    let player = region.players.remove(&player(1)).expect("player 1 is here");
    let using = player.using;
    let transfer = player.into_transfer(tick);
    assert_eq!(transfer.using, using);
    // Counted at the end of the tick, as the attack's reset is: the next region's
    // first tick is the cooldown's sixth.
    assert_eq!(
        transfer.cooldowns,
        [("minecraft:ender_pearl".to_owned(), 5, 20)]
    );

    let mut next = joined_in(ChunkArea::EVERYWHERE, &[]);
    next.tick(&changes(vec![arrive(1, &transfer)]));
    let arrived = next
        .players
        .get(&super::player(1))
        .expect("player 1 arrived");
    // Ticked on in the tick it arrives in, the one after its last in the other region.
    let ticked = using.map(|using| Using {
        remaining: using.remaining - 1,
        ..using
    });
    assert_eq!(arrived.using, ticked);
    let start = next.tick_number().saturating_sub(5);
    assert_eq!(
        arrived.cooldowns,
        [Cooldown {
            group: "minecraft:ender_pearl".to_owned(),
            start,
            end: start + 20,
        }]
    );
}
