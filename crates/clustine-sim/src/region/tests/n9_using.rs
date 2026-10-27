//! A player's use of an item over time, tested from the plan (`docs/groundwork/
//! n9-plan.md`, sections 4.6, 5.5 and 14) and the game's `BowItem` and `CrossbowItem`
//! (`n9-javap`) by someone who did not write it: begun by a use, ticked once in each of
//! the player's ticks, ended by `ReleaseUse`, going on across a hand-over at a pinned
//! line; stopped when the hand's item is no longer the same item (`isSameItem`, which
//! reads the item alone); and a held crossbow loaded on its 26th use tick into the
//! hand's own stack.

use clustine_data::items;
use clustine_world::components::{Component, Kind};
use clustine_world::{EntityData, ItemStack};

use super::n9_world::*;
use super::*;
use crate::api::{Hand, Using};

fn using(region: &Region, number: u128) -> Option<Using> {
    region
        .players
        .get(&player(number))
        .and_then(|player| player.using)
}

/// The arrows of the region's loaded chunks, each with whether it is critical.
fn arrows(region: &Region) -> Vec<bool> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match &entity.data {
            EntityData::Arrow(arrow) => Some(arrow.crit),
            _ => None,
        })
        .collect()
}

/// A region alone with player 1 at the origin holding `stack`.
fn holding(stack: ItemStack) -> Region {
    let mut region = alone(0, clock_at(0));
    enter_with(
        &mut region,
        1,
        entering_place(Vec3::new(0.5, -60.0, 0.5), Vec::new(), 0),
    );
    hold(&mut region, 1, stack);
    region
}

/// Draws the bow player 1 holds for `ticks` of their ticks, the use's own first tick
/// among them, and lets go in the tick after: whether the arrow is critical, which it
/// is at full power, twenty ticks of drawing (`BowItem.getPowerForTime`).
fn drawn_for(ticks: u32) -> bool {
    let mut region = holding(ItemStack::new(items::BOW, 1));
    acting(&mut region, 1, vec![use_item(0.0)]);
    for _ in 1..ticks {
        region.tick(&TickInputs::default());
    }
    assert_eq!(
        using(&region, 1).map(|using| using.remaining),
        Some(72_000 - i32::try_from(ticks).expect("few")),
        "the use has ticked {ticks} times"
    );
    acting(&mut region, 1, vec![PlayerInput::ReleaseUse]);
    assert_eq!(using(&region, 1), None, "the release ends the use");
    let shot = arrows(&region);
    let [crit] = shot[..] else {
        panic!("one arrow: {shot:?}");
    };
    crit
}

/// Section 5.5: a use begins with `UseItem` where the item's `use` starts one, ticks
/// once a tick from that tick on, and ends at `ReleaseUse`, where the bow's
/// `releaseUsing` shoots by the ticks it was drawn: at full power after twenty.
#[test]
fn a_bow_s_use_begins_ticks_and_is_released_into_a_shot_by_the_ticks_it_was_drawn() {
    assert!(drawn_for(20), "full power after twenty ticks");
    assert!(!drawn_for(19), "not quite after nineteen");
}

/// Section 5.5: a use goes on across a hand-over, which the client does not see: the
/// next region ticks it on from where the last left it, and its release there shoots by
/// all the ticks it was drawn in both.
#[test]
fn a_use_goes_on_across_a_hand_over_and_its_release_counts_every_tick_drawn() {
    for (in_west, crit) in [(5, true), (12, true), (4, false)] {
        let mut west = west(100, clock_at(0));
        let mut east = east(7_000, clock_at(20));
        enter_with(
            &mut west,
            1,
            entering_place(Vec3::new(14.5, -60.0, 0.5), Vec::new(), 0),
        );
        hold(&mut west, 1, ItemStack::new(items::BOW, 1));
        acting(&mut west, 1, vec![use_item(0.0)]);
        for _ in 1..in_west {
            west.tick(&TickInputs::default());
        }
        // The step's tick is a tick of the use too.
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
        assert!(transfer.using.is_some(), "the use goes with the player");
        east.tick(&changes(vec![arrive(1, transfer)]));
        // Nineteen or twenty ticks drawn in all: the west's, the step's, the arrival's
        // and those after it.
        let total = if crit { 20 } else { 19 };
        let in_east = total - in_west - 1;
        for _ in 1..in_east {
            east.tick(&TickInputs::default());
        }
        assert_eq!(
            using(&east, 1),
            Some(Using {
                hand: Hand::Main,
                item: items::BOW,
                remaining: 72_000 - total,
            }),
            "{in_west} ticks in the west"
        );
        acting(&mut east, 1, vec![PlayerInput::ReleaseUse]);
        assert_eq!(arrows(&east), [crit], "{in_west} ticks in the west");
    }
}

/// Section 5.5 (finding 19): the use reads the hand every tick and stops when the item
/// there is no longer the same item (`isSameItem`, the item alone, not its components):
/// another item set into the slot, or another slot selected, ends it; the same item with
/// other components does not.
#[test]
fn a_use_stops_when_the_hand_s_item_changes_and_not_when_only_its_components_do() {
    let mut region = holding(ItemStack::new(items::BOW, 1));
    acting(&mut region, 1, vec![use_item(0.0)]);
    acting(
        &mut region,
        1,
        vec![PlayerInput::SetInventorySlot {
            slot: 0,
            stack: Some(ItemStack::new(items::STONE, 1)),
        }],
    );
    assert_eq!(using(&region, 1), None, "another item");

    let mut region = holding(ItemStack::new(items::BOW, 1));
    acting(&mut region, 1, vec![use_item(0.0)]);
    acting(&mut region, 1, vec![PlayerInput::SelectSlot { slot: 3 }]);
    assert_eq!(using(&region, 1), None, "another slot");

    let mut region = holding(ItemStack::new(items::BOW, 1));
    acting(&mut region, 1, vec![use_item(0.0)]);
    let mut worn = ItemStack::new(items::BOW, 1);
    worn.components.set(Component::Damage(7));
    acting(
        &mut region,
        1,
        vec![PlayerInput::SetInventorySlot {
            slot: 0,
            stack: Some(worn),
        }],
    );
    assert_eq!(
        using(&region, 1).map(|using| using.remaining),
        Some(72_000 - 2),
        "the same item, worn: the use goes on"
    );

    let mut region = holding(ItemStack::new(items::BOW, 1));
    acting(&mut region, 1, vec![use_item(0.0)]);
    acting(
        &mut region,
        1,
        vec![PlayerInput::SetInventorySlot {
            slot: 0,
            stack: None,
        }],
    );
    assert_eq!(using(&region, 1), None, "the item gone from the hand");
}

/// What the crossbow in inventory slot `slot` of player 1 is loaded with.
fn charge(region: &Region, slot: usize) -> Option<Vec<ItemStack>> {
    let stack = region.players[&player(1)].inventory.0[slot]
        .clone()
        .expect("a crossbow there");
    assert_eq!(stack.item, items::CROSSBOW);
    match stack.components.get(Kind::ChargedProjectiles) {
        Some(Component::ChargedProjectiles(loaded)) => Some(loaded.clone()),
        _ => None,
    }
}

/// Section 4.6 and `CrossbowItem.onUseTick` (n9-javap, 489 on): the use lasts 72,000
/// ticks and the charge 25 (1.25 s), and `onUseTick` sees the ticks remaining before
/// the countdown, so the load, at a progress of 1, is in the use's 26th tick. It writes
/// `charged_projectiles` into the hand's own stack, not into another crossbow; a creative
/// player with no arrow loads an intangible one.
#[test]
fn a_held_crossbow_loads_on_its_26th_use_tick_into_the_hand_s_own_stack() {
    let mut region = holding(ItemStack::new(items::CROSSBOW, 1));
    // Another crossbow, in the third slot, which is not used.
    acting(
        &mut region,
        1,
        vec![PlayerInput::SetInventorySlot {
            slot: 2,
            stack: Some(ItemStack::new(items::CROSSBOW, 1)),
        }],
    );
    acting(&mut region, 1, vec![use_item(0.0)]);
    for tick in 2..=25 {
        region.tick(&TickInputs::default());
        assert_eq!(charge(&region, 0), None, "not loaded in use tick {tick}");
    }
    region.tick(&TickInputs::default());
    // A creative player's ammunition is intangible (`useAmmo`, section 4.6).
    let mut arrow = ItemStack::new(items::ARROW, 1);
    arrow.components.set(Component::IntangibleProjectile);
    assert_eq!(charge(&region, 0), Some(vec![arrow]), "loaded in the 26th");
    assert_eq!(charge(&region, 2), None, "the other crossbow is as it was");
    // A crossbow's use is ended by its release, not by its loading.
    assert!(using(&region, 1).is_some());
    acting(&mut region, 1, vec![PlayerInput::ReleaseUse]);
    assert_eq!(using(&region, 1), None);
    assert!(
        charge(&region, 0).is_some(),
        "a release loads nothing and empties nothing"
    );
}
