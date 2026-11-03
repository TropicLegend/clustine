//! Tests of how an operator's order is read from its words (`clustine gamemode`,
//! `clustine give`, the single binary's standard input), written from the specification
//! by someone who did not write the code: the design `v1-orders-design.md`, its section 4
//! and the review's item 1, and the game's commands. `/give` takes an item named with or
//! without `minecraft:`, a count of at least 1 (`IntegerArgumentType.integer(1)`) and at
//! most 100 of the item's largest stacks (`commands.give.failed.toomanyitems`); the tool
//! also refuses air and an unknown name. `/gamemode` takes a mode's name; only survival
//! and creative are built.

use clustine_data::items;
use clustine_rpc::Order;
use clustine_world::GameMode;

use super::order_from_words;

fn read(line: &str) -> anyhow::Result<(String, Order)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    order_from_words(&words)
}

fn order(line: &str) -> (String, Order) {
    read(line).unwrap_or_else(|error| panic!("`{line}` is an order: {error:#}"))
}

fn refused(line: &str) {
    let read = read(line);
    assert!(read.is_err(), "`{line}` is refused: {read:?}");
}

fn given(player: &str, item: i32, count: i32) -> (String, Order) {
    (player.to_owned(), Order::Give { item, count })
}

#[test]
fn gamemode_survival_and_creative_are_orders_for_the_player_named() {
    assert_eq!(
        order("gamemode Alex survival"),
        ("Alex".to_owned(), Order::GameMode(GameMode::Survival))
    );
    assert_eq!(
        order("gamemode Steve creative"),
        ("Steve".to_owned(), Order::GameMode(GameMode::Creative))
    );
}

#[test]
fn an_unknown_mode_is_refused() {
    refused("gamemode Alex flying");
    refused("gamemode Alex 0");
    refused("gamemode Alex");
}

/// Adventure and spectator are the game's modes, but Clustine does not play them yet.
#[test]
fn adventure_and_spectator_are_refused() {
    refused("gamemode Alex adventure");
    refused("gamemode Alex spectator");
}

#[test]
fn give_names_the_item_with_or_without_its_namespace() {
    assert_eq!(order("give Alex stone 5"), given("Alex", items::STONE, 5));
    assert_eq!(
        order("give Alex minecraft:stone 5"),
        given("Alex", items::STONE, 5)
    );
    assert_eq!(
        order("give Alex minecraft:diamond_sword 1"),
        given("Alex", items::DIAMOND_SWORD, 1)
    );
}

/// `/give <targets> <item>` gives one.
#[test]
fn give_without_a_count_gives_one() {
    assert_eq!(order("give Alex stone"), given("Alex", items::STONE, 1));
}

#[test]
fn air_and_an_unknown_item_are_refused() {
    refused("give Alex air");
    refused("give Alex minecraft:air 3");
    refused("give Alex stoone 3");
    refused("give Alex other:stone 3");
}

#[test]
fn a_count_below_one_or_not_a_number_is_refused() {
    refused("give Alex stone 0");
    refused("give Alex stone -4");
    refused("give Alex stone five");
}

/// `GiveCommand`: at most 100 × the item's largest stack: 6400 stone, 1600 ender pearls,
/// 100 swords; one more is `commands.give.failed.toomanyitems`.
#[test]
fn exactly_a_hundred_stacks_are_given_and_one_more_is_refused() {
    for (name, item, most) in [
        ("stone", items::STONE, 6400),
        ("ender_pearl", items::ENDER_PEARL, 1600),
        ("diamond_sword", items::DIAMOND_SWORD, 100),
    ] {
        assert_eq!(
            order(&format!("give Alex {name} {most}")),
            given("Alex", item, most)
        );
        refused(&format!("give Alex {name} {}", most + 1));
    }
}

#[test]
fn a_line_of_neither_form_is_refused() {
    refused("");
    refused("give Alex");
    refused("give Alex stone 5 6");
    refused("tp Alex 0 0 0");
}
