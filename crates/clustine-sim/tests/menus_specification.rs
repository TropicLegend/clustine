//! Tests of the menus of step N6, written from `docs/groundwork/n6-plan.md` (decisions 2
//! and 5; section 6 items 1 to 3; section 7 items 1 to 3, 6, 7, 8 and 10; section 8
//! items 6 to 9; section 9 item 3), the porting brief `docs/groundwork/n6-brief.md`
//! (section 3, findings 1 to 26, and 3.10 to 3.21) and the public API alone, by someone
//! who did not write the code: which menu each container opens and under what title,
//! the serial a menu is named by, every kind of click and what its answer says changed,
//! the answer to a click that changes nothing or is of another serial, closing, leaving,
//! a hand-over and a restore with a menu open, a resync, and the ender chest's slots
//! going with the player.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60) in chunk (0, 0), whose
//! 5x5 is loaded. Every player of Clustine is a creative one.

mod n6_scene;

use clustine_data::{blocks, items};
use clustine_sim::api::{
    ClickKind, Face, Inventory, ItemStack, MenuKind, PlayerEvent, PlayerInput, StayNote,
};
use clustine_sim::{MenuOf, PlayerChange, Region, TickInputs, Ticket};
use clustine_world::{BlockPos, ChunkPos, Vec3};
use n6_scene::*;

/// Where the containers of these tests stand, beside player 1.
const BOX: BlockPos = BlockPos::new(2, -60, 2);
const BESIDE: Vec3 = Vec3::new(0.5, -60.0, 0.5);

/// The menu slot of hotbar slot `n` in a menu of `kind`.
fn hotbar_slot(kind: MenuKind, n: usize) -> i16 {
    i16::try_from(kind.container_slots() + 27 + n).expect("fits")
}

/// Player 1 holds 64 dirt in hotbar slot 0, 64 stone in 1 and 64 cobblestone in 2.
fn holding() -> Inventory {
    inventory(&[
        (0, stack(items::DIRT, 64)),
        (1, stack(items::STONE, 64)),
        (2, stack(items::COBBLESTONE, 64)),
    ])
}

/// A single chest at [`BOX`], facing north.
fn chest() -> World {
    World::of(&[(BOX, state("minecraft:chest[facing=north,type=single]"))])
}

/// Player 1 beside a chest at [`BOX`] with [`holding`]; its menu open, of the serial
/// returned.
fn at_an_open_chest() -> (Scene, u64) {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let (serial, output) = scene.open(BOX);
    let opened = the_opened(&output, player(1));
    assert_eq!(opened.serial, serial);
    (scene, serial)
}

/// As [`at_an_open_chest`], with the dirt of hotbar slot 0 moved into chest slot 0.
fn at_a_chest_with_dirt() -> (Scene, u64) {
    let (mut scene, serial) = at_an_open_chest();
    let output = scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    let change = the_changed(&output, player(1));
    assert!(
        change.slots.contains(&(0, Some(stack(items::DIRT, 64)))),
        "the dirt went into the chest's first slot: {change:?}"
    );
    (scene, serial)
}

fn the_menu(scene: &Scene) -> clustine_sim::Menu {
    scene
        .region
        .player_state(player(1))
        .expect("player 1 is there")
        .menu
        .expect("a menu is open")
}

fn no_menu(scene: &Scene) -> bool {
    scene
        .region
        .player_state(player(1))
        .expect("player 1 is there")
        .menu
        .is_none()
}

// ------------------------------------------------------------------------------------
// Opening (decision 2; section 7 item 2; brief 3.0 items 1 to 5, 3.1, 3.10, 3.11).

/// Section 7 item 2: the serial is the number of the input that opened the menu; the
/// report says which input was the last applied, and has every slot of the menu: the
/// chest's 27 and then the player's main 27 and hotbar 9 (brief 3.11).
#[test]
fn a_chest_opened_by_a_use_is_a_menu_of_three_rows_named_by_the_number_of_its_input() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    scene.select(1);
    let (serial, output) = scene.open(BOX);
    assert_eq!(serial, 2, "the second input of the player");
    let opened = the_opened(&output, player(1));
    assert_eq!(opened.serial, serial);
    assert_eq!(opened.applied, serial, "the use was the last input applied");
    assert_eq!(opened.kind, MenuKind::Rows3);
    assert_eq!(opened.title, "container.chest");
    assert_eq!(opened.slots.len(), 63);
    assert!(
        opened.slots[..27].iter().all(Option::is_none),
        "an empty chest"
    );
    assert_eq!(
        opened.slots[54],
        Some(stack(items::DIRT, 64)),
        "hotbar slot 0"
    );
    assert_eq!(
        opened.slots[55],
        Some(stack(items::STONE, 64)),
        "hotbar slot 1"
    );
    assert_eq!(opened.carried, None);
    let menu = the_menu(&scene);
    assert_eq!(menu.serial, serial);
    assert_eq!(menu.kind, MenuKind::Rows3);
    assert_eq!(
        menu.of,
        MenuOf::Block {
            at: BOX,
            other: None
        }
    );
    assert_eq!(
        scene.block(BOX.offset(0, 1, 0)),
        blocks::AIR,
        "nothing was placed"
    );
}

/// Brief 3.0 item 3 and 3.1: the menu and the title of every container block of N6.
#[test]
fn each_container_block_opens_its_own_kind_of_menu_under_its_own_title() {
    let cases = [
        (
            "minecraft:chest[facing=north,type=single]",
            MenuKind::Rows3,
            "container.chest",
        ),
        (
            "minecraft:trapped_chest[facing=north,type=single]",
            MenuKind::Rows3,
            "container.chest",
        ),
        (
            "minecraft:barrel[facing=up,open=false]",
            MenuKind::Rows3,
            "container.barrel",
        ),
        (
            "minecraft:shulker_box[facing=up]",
            MenuKind::ShulkerBox,
            "container.shulkerBox",
        ),
        (
            "minecraft:red_shulker_box[facing=up]",
            MenuKind::ShulkerBox,
            "container.shulkerBox",
        ),
        (
            "minecraft:ender_chest[facing=north]",
            MenuKind::Rows3,
            "container.enderchest",
        ),
        (
            "minecraft:dispenser[facing=north,triggered=false]",
            MenuKind::Grid3x3,
            "container.dispenser",
        ),
        (
            "minecraft:dropper[facing=north,triggered=false]",
            MenuKind::Grid3x3,
            "container.dropper",
        ),
        (
            "minecraft:hopper[facing=down,enabled=true]",
            MenuKind::Hopper,
            "container.hopper",
        ),
    ];
    for (name, kind, title) in cases {
        let mut scene = Scene::alone(&World::of(&[(BOX, state(name))]), holding(), BESIDE);
        let (serial, output) = scene.open(BOX);
        let opened = the_opened(&output, player(1));
        assert_eq!(
            (opened.kind, opened.title.as_str()),
            (kind, title),
            "{name}"
        );
        assert_eq!(opened.serial, serial, "{name}");
        assert_eq!(opened.slots.len(), kind.slots(), "{name}");
        assert_eq!(
            opened.slots[usize::try_from(hotbar_slot(kind, 0)).expect("fits")],
            Some(stack(items::DIRT, 64)),
            "{name}: the hotbar comes last"
        );
        assert_eq!(the_menu(&scene).kind, kind, "{name}");
    }
}

/// Brief 3.0 item 4 and 3.3: a double chest is one menu of six rows, the right half's
/// slots first, whichever half is clicked; for halves facing north the right one is
/// the eastern (its partner lies counter-clockwise, to the west).
#[test]
fn a_double_chest_opens_six_rows_with_the_right_half_first_whichever_half_is_clicked() {
    let west = BOX;
    let east = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (west, state("minecraft:chest[facing=north,type=left]")),
        (east, state("minecraft:chest[facing=north,type=right]")),
    ]);
    for clicked in [west, east] {
        let mut scene = Scene::alone(&world, holding(), BESIDE);
        let (_, output) = scene.open(clicked);
        let opened = the_opened(&output, player(1));
        assert_eq!(opened.kind, MenuKind::Rows6);
        assert_eq!(opened.title, "container.chestDouble");
        assert_eq!(opened.slots.len(), 90);
        assert_eq!(
            opened.slots[81],
            Some(stack(items::DIRT, 64)),
            "hotbar slot 0"
        );
        assert_eq!(
            the_menu(&scene).of,
            MenuOf::Block {
                at: east,
                other: Some(west)
            },
            "clicked at {clicked:?}"
        );
    }
}

/// Brief 3.0 items 1 and 5: a chest under a redstone conductor opens nothing, and the
/// click is consumed all the same: the dirt in hand is not placed against it.
#[test]
fn a_chest_under_a_stone_block_opens_nothing_and_the_click_places_nothing() {
    let world = World::of(&[
        (BOX, state("minecraft:chest[facing=north,type=single]")),
        (BOX.offset(0, 1, 0), blocks::STONE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let output = scene.click(BOX, Face::North);
    assert!(
        opened(&output, player(1)).is_empty(),
        "{:?}",
        output.player_events
    );
    assert!(no_menu(&scene));
    assert_eq!(
        scene.block(BOX.offset(0, 0, -1)),
        blocks::AIR,
        "no dirt placed"
    );
    assert_eq!(scene.holds(0), Some(stack(items::DIRT, 64)));
}

/// Brief 3.0 item 9: a closed shulker box opens only with room for its lid; one facing
/// up under a stone block does not, and the click is consumed.
#[test]
fn a_shulker_box_with_a_block_in_front_of_its_lid_does_not_open() {
    let world = World::of(&[
        (BOX, state("minecraft:shulker_box[facing=up]")),
        (BOX.offset(0, 1, 0), blocks::STONE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let output = scene.click(BOX, Face::North);
    assert!(
        opened(&output, player(1)).is_empty(),
        "{:?}",
        output.player_events
    );
    assert!(no_menu(&scene));
    assert_eq!(
        scene.block(BOX.offset(0, 0, -1)),
        blocks::AIR,
        "no dirt placed"
    );
}

/// Brief 3.10 (`openMenu` closes the open menu first) and 3.2 (`removed` runs
/// `stopOpen`): a second container opened while a chest is open takes the menu's
/// place, and the chest's openers fall to none, which its block event says.
#[test]
fn opening_a_second_container_closes_the_first_and_its_lid() {
    let barrel = BOX.offset(-2, 0, 0);
    let world = World::of(&[
        (BOX, state("minecraft:chest[facing=north,type=single]")),
        (barrel, state("minecraft:barrel[facing=up,open=false]")),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (first, _) = scene.open(BOX);
    let (second, output) = scene.open(barrel);
    let opened = the_opened(&output, player(1));
    assert_eq!(opened.serial, second);
    assert_eq!(opened.title, "container.barrel");
    assert_ne!(first, second);
    assert_eq!(the_menu(&scene).serial, second);
    let chest = block_id(blocks::CHEST);
    let mut seen = vec![output];
    seen.extend(scene.idle_for(2));
    assert!(
        seen.iter()
            .any(|output| block_events_at(output, BOX).contains(&(chest, 1, 0))),
        "the chest's openers fell to none"
    );
}

// ------------------------------------------------------------------------------------
// Clicks and their answers (section 7 item 1; brief 3.14).

/// Section 7 item 1: every click is answered, with the serial and the last input
/// applied; a left click on a stack picks it all up.
#[test]
fn a_left_click_on_a_stack_picks_it_all_up_and_the_answer_says_so() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    let number = scene.next_input(0);
    let output = scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let change = the_changed(&output, player(1));
    assert_eq!(change.serial, serial);
    assert_eq!(change.applied, number);
    assert_eq!(change.slots, vec![(0, None)]);
    assert_eq!(change.carried, Some(Some(stack(items::DIRT, 64))));
    assert_eq!(the_menu(&scene).carried, Some(stack(items::DIRT, 64)));
}

/// Section 7 item 1: a click that changes nothing is answered too, with no slots and
/// no carried stack, so that the edge compares what the client guessed.
#[test]
fn a_click_that_changes_nothing_is_answered_with_no_changes() {
    let (mut scene, serial) = at_an_open_chest();
    let number = scene.next_input(0);
    let output = scene.menu_click(serial, 5, 0, ClickKind::Pickup);
    let change = the_changed(&output, player(1));
    assert_eq!(change.serial, serial);
    assert_eq!(change.applied, number);
    assert!(change.slots.is_empty(), "{change:?}");
    assert_eq!(change.carried, None);
}

/// Brief 3.14.3: a right click on a stack with nothing carried takes half, rounded up.
#[test]
fn a_right_click_on_a_stack_takes_half_of_it_rounded_up() {
    let (mut scene, serial) = at_an_open_chest();
    // Make the chest's stack odd: 64 dirt in, then one put back from a carried one.
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    scene.menu_click(serial, 1, 1, ClickKind::Pickup);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::Pickup,
    );
    // Slot 1 holds 1 dirt and hotbar slot 0 holds 63.
    let output = scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        1,
        ClickKind::Pickup,
    );
    let change = the_changed(&output, player(1));
    assert_eq!(
        change.slots,
        vec![(54, Some(stack(items::DIRT, 31)))],
        "{change:?}"
    );
    assert_eq!(change.carried, Some(Some(stack(items::DIRT, 32))));
}

/// Brief 3.14.3: with a stack carried, a left click on an empty slot puts it all down
/// and a right click puts down one.
#[test]
fn a_carried_stack_is_put_down_whole_by_a_left_click_and_one_by_a_right_click() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let output = scene.menu_click(serial, 4, 1, ClickKind::Pickup);
    let change = the_changed(&output, player(1));
    assert_eq!(change.slots, vec![(4, Some(stack(items::DIRT, 1)))]);
    assert_eq!(change.carried, Some(Some(stack(items::DIRT, 63))));
    let output = scene.menu_click(serial, 7, 0, ClickKind::Pickup);
    let change = the_changed(&output, player(1));
    assert_eq!(change.slots, vec![(7, Some(stack(items::DIRT, 63)))]);
    assert_eq!(change.carried, Some(None));
    assert_eq!(the_menu(&scene).carried, None);
}

/// Brief 3.14.3: a carried stack of another item swaps with the slot's.
#[test]
fn a_carried_stack_of_another_item_swaps_with_the_slots() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 1),
        0,
        ClickKind::Pickup,
    );
    let output = scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let change = the_changed(&output, player(1));
    assert_eq!(change.slots, vec![(0, Some(stack(items::STONE, 64)))]);
    assert_eq!(change.carried, Some(Some(stack(items::DIRT, 64))));
}

/// Brief 3.14.9: a shift-click from the player's part goes into the chest's first slot
/// that takes it, and one from the chest to the hotbar's right end first.
#[test]
fn a_shift_click_moves_into_the_chest_and_back_to_the_hotbars_right_end() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    let output = scene.menu_click(serial, 0, 0, ClickKind::QuickMove);
    let change = the_changed(&output, player(1));
    let mut slots = change.slots.clone();
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(
        slots,
        vec![(0, None), (62, Some(stack(items::DIRT, 64)))],
        "{change:?}"
    );
    assert_eq!(
        scene.holds(8),
        Some(stack(items::DIRT, 64)),
        "hotbar slot 8"
    );
}

/// Brief 3.14.5: a number key over a slot swaps it with that hotbar slot.
#[test]
fn a_number_key_over_a_chest_slot_swaps_it_with_the_hotbar_slot() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    // Hotbar slot 1 holds stone; the chest's slot 0 dirt.
    let output = scene.menu_click(serial, 0, 1, ClickKind::Swap);
    let change = the_changed(&output, player(1));
    let mut slots = change.slots.clone();
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(
        slots,
        vec![
            (0, Some(stack(items::STONE, 64))),
            (55, Some(stack(items::DIRT, 64)))
        ]
    );
    assert_eq!(change.carried, None);
    assert_eq!(scene.holds(1), Some(stack(items::DIRT, 64)));
}

/// Brief 3.14.6: a creative player's middle click on a slot carries a full stack of
/// its item, and the slot keeps what it has.
#[test]
fn a_creative_middle_click_carries_a_full_stack_of_the_slots_item() {
    let (mut scene, serial) = at_an_open_chest();
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.menu_click(serial, 0, 1, ClickKind::Pickup);
    // The chest's slot 0 holds 32 dirt; put the carried 32 back in slot 3.
    scene.menu_click(serial, 3, 0, ClickKind::Pickup);
    let output = scene.menu_click(serial, 0, 2, ClickKind::Clone);
    let change = the_changed(&output, player(1));
    assert!(change.slots.is_empty(), "{change:?}");
    assert_eq!(change.carried, Some(Some(stack(items::DIRT, 64))));
}

/// Brief 3.14.7: Q over a slot throws one of it, and with ctrl the whole stack, as the
/// player's throw.
#[test]
fn q_over_a_slot_throws_one_and_ctrl_q_the_whole_stack() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    let output = scene.menu_click(serial, 0, 0, ClickKind::Throw);
    let change = the_changed(&output, player(1));
    assert_eq!(change.slots, vec![(0, Some(stack(items::DIRT, 63)))]);
    assert_eq!(lying_count(&scene.all(), items::DIRT), 1);
    let output = scene.menu_click(serial, 0, 1, ClickKind::Throw);
    let change = the_changed(&output, player(1));
    assert_eq!(change.slots, vec![(0, None)]);
    assert_eq!(lying_count(&scene.all(), items::DIRT), 64);
}

/// Brief 3.14.2: a left click outside the window throws the carried stack, a right
/// click one of it.
#[test]
fn a_click_outside_the_window_throws_what_is_carried() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let output = scene.menu_click(serial, -999, 1, ClickKind::Pickup);
    assert_eq!(
        the_changed(&output, player(1)).carried,
        Some(Some(stack(items::DIRT, 63)))
    );
    assert_eq!(lying_count(&scene.all(), items::DIRT), 1);
    let output = scene.menu_click(serial, -999, 0, ClickKind::Pickup);
    assert_eq!(the_changed(&output, player(1)).carried, Some(None));
    assert_eq!(lying_count(&scene.all(), items::DIRT), 64);
}

/// The buttons of a drag: `(header & 3) | ((type & 3) << 2)` (brief 3.14.1).
fn drag(scene: &mut Scene, serial: u64, kind: i8, slots: &[i16]) -> Vec<clustine_sim::TickOutput> {
    let mut outputs = vec![scene.menu_click(serial, -999, kind << 2, ClickKind::QuickCraft)];
    for slot in slots {
        outputs.push(scene.menu_click(serial, *slot, (kind << 2) | 1, ClickKind::QuickCraft));
    }
    outputs.push(scene.menu_click(serial, -999, (kind << 2) | 2, ClickKind::QuickCraft));
    outputs
}

/// Brief 3.14.1: a left drag shares the carried stack evenly over the slots dragged
/// over, `floor(64 / 3)` each, and keeps the rest carried; the drag is in the menu's
/// state until its end.
#[test]
fn a_left_drag_over_three_slots_puts_twenty_one_in_each_and_keeps_one() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let start = scene.menu_click(serial, -999, 0, ClickKind::QuickCraft);
    assert!(the_menu(&scene).drag.is_some(), "a drag under way");
    let mut outputs = vec![start];
    for slot in [3, 4, 5] {
        outputs.push(scene.menu_click(serial, slot, 1, ClickKind::QuickCraft));
    }
    outputs.push(scene.menu_click(serial, -999, 2, ClickKind::QuickCraft));
    for output in &outputs {
        assert_eq!(changed(output, player(1)).len(), 1, "each step is answered");
    }
    let end = the_changed(outputs.last().expect("an end"), player(1));
    let mut slots = end.slots.clone();
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(
        slots,
        vec![
            (3, Some(stack(items::DIRT, 21))),
            (4, Some(stack(items::DIRT, 21))),
            (5, Some(stack(items::DIRT, 21)))
        ]
    );
    assert_eq!(end.carried, Some(Some(stack(items::DIRT, 1))));
    assert!(the_menu(&scene).drag.is_none(), "the drag ended");
}

/// Brief 3.14.1: a right drag puts one into each slot.
#[test]
fn a_right_drag_puts_one_into_each_slot() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let outputs = drag(&mut scene, serial, 1, &[3, 4, 5, 6]);
    let end = the_changed(outputs.last().expect("an end"), player(1));
    assert_eq!(end.slots.len(), 4, "{end:?}");
    assert!(
        end.slots
            .iter()
            .all(|(_, stack)| *stack == Some(ItemStack::new(items::DIRT, 1)))
    );
    assert_eq!(end.carried, Some(Some(stack(items::DIRT, 60))));
}

/// Brief 3.14.1: a drag over one slot is a click of that slot.
#[test]
fn a_drag_over_one_slot_is_a_click_of_it() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let outputs = drag(&mut scene, serial, 0, &[8]);
    let end = the_changed(outputs.last().expect("an end"), player(1));
    assert_eq!(end.slots, vec![(8, Some(stack(items::DIRT, 64)))]);
    assert_eq!(end.carried, Some(None));
}

/// Brief 3.14.1: a creative player's middle drag puts a full stack into each slot and
/// leaves the cursor as it was.
#[test]
fn a_creative_middle_drag_puts_a_full_stack_into_each_slot() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let outputs = drag(&mut scene, serial, 2, &[3, 4]);
    let end = the_changed(outputs.last().expect("an end"), player(1));
    let mut slots = end.slots.clone();
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(
        slots,
        vec![
            (3, Some(stack(items::DIRT, 64))),
            (4, Some(stack(items::DIRT, 64)))
        ]
    );
    // `left` is the carried count less what went in beyond what was there: 64 - 128,
    // which leaves the cursor empty (3.14.1's last note).
    assert_eq!(end.carried, Some(None));
}

/// Brief 3.14.8: a double click on an empty slot gathers the item onto the cursor,
/// from partial stacks first.
#[test]
fn a_double_click_gathers_the_item_onto_the_cursor() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    // 64 dirt in slot 0: carry it, put 10 in slot 1 by right clicks, keep 54.
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    for _ in 0..10 {
        scene.menu_click(serial, 1, 1, ClickKind::Pickup);
    }
    // Put the 54 into slot 2, take 2 of slot 1 back, and leave slot 0 empty.
    scene.menu_click(serial, 2, 0, ClickKind::Pickup);
    scene.menu_click(serial, 1, 1, ClickKind::Pickup);
    // Carried: 5 (half of 10); slot 1: 5; slot 2: 54.
    assert_eq!(the_menu(&scene).carried, Some(stack(items::DIRT, 5)));
    let output = scene.menu_click(serial, 0, 0, ClickKind::PickupAll);
    let change = the_changed(&output, player(1));
    // From slot 1's 5 first (a partial stack), then 54 of slot 2: up to 64.
    assert_eq!(change.carried, Some(Some(stack(items::DIRT, 64))));
    let mut slots = change.slots.clone();
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(slots, vec![(1, None), (2, None)]);
}

// ------------------------------------------------------------------------------------
// Serials, reports, closing and leaving (section 7 items 1, 2, 10; section 8 item 8;
// brief 3.17, 3.18).

/// Section 7 items 1 and 2: a use and a click of the menu it opens in one tick are
/// answered in order: the menu whole as of the use, then the click's answer.
#[test]
fn an_open_and_a_click_in_one_tick_are_answered_in_their_order() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let serial = scene.next_input(0);
    scene.actors[0].sequence += 1;
    let sequence = scene.actors[0].sequence;
    let output = scene.inputs_of(
        0,
        vec![
            PlayerInput::UseItemOn {
                position: BOX,
                face: Face::Top,
                cursor: [0.5, 1.0, 0.5],
                sneaking: false,
                sequence,
            },
            PlayerInput::ContainerClick {
                serial,
                slot: hotbar_slot(MenuKind::Rows3, 0),
                button: 0,
                kind: ClickKind::QuickMove,
            },
            PlayerInput::ContainerClick {
                serial,
                slot: 0,
                button: 1,
                kind: ClickKind::Pickup,
            },
        ],
    );
    let menu_events: Vec<PlayerEvent> = events_of(&output, player(1))
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                PlayerEvent::MenuOpened { .. } | PlayerEvent::MenuChanged { .. }
            )
        })
        .collect();
    assert_eq!(menu_events.len(), 3, "{menu_events:?}");
    match &menu_events[0] {
        PlayerEvent::MenuOpened {
            serial: opened,
            applied,
            slots,
            ..
        } => {
            assert_eq!((*opened, *applied), (serial, serial));
            assert!(slots[0].is_none(), "as of the use");
        }
        other => panic!("the opening first: {other:?}"),
    }
    match &menu_events[1] {
        PlayerEvent::MenuChanged { applied, slots, .. } => {
            assert_eq!(*applied, serial + 1);
            assert!(slots.contains(&(0, Some(stack(items::DIRT, 64)))));
        }
        other => panic!("the first click's answer: {other:?}"),
    }
    match &menu_events[2] {
        PlayerEvent::MenuChanged {
            applied, carried, ..
        } => {
            assert_eq!(*applied, serial + 2);
            assert_eq!(*carried, Some(Some(stack(items::DIRT, 32))));
        }
        other => panic!("the second click's answer: {other:?}"),
    }
}

/// Section 6 item 3 and section 7 items 1 and 2: a region with no menu open passes a
/// click over, and still answers it, with none. (The plan does not say by which event;
/// [`PlayerEvent::NoMenu`], [`PlayerEvent::MenuClosed`] and an empty
/// [`PlayerEvent::MenuChanged`] are each taken, so long as it names the click.)
#[test]
fn a_click_with_no_menu_open_is_answered() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let number = scene.next_input(0);
    let output = scene.menu_click(5, 0, 0, ClickKind::Pickup);
    let answers: Vec<PlayerEvent> = events_of(&output, player(1))
        .into_iter()
        .filter(|event| match event {
            PlayerEvent::NoMenu { applied, .. } | PlayerEvent::MenuClosed { applied, .. } => {
                *applied == number
            }
            PlayerEvent::MenuChanged { applied, slots, .. } => {
                *applied == number && slots.is_empty()
            }
            _ => false,
        })
        .collect();
    assert_eq!(answers.len(), 1, "{:?}", output.player_events);
    assert_eq!(scene.holds(0), Some(stack(items::DIRT, 64)));
}

/// Section 7 item 2: a click of another serial than the open menu's is refused, and
/// answered with the menu whole.
#[test]
fn a_click_of_another_serial_is_refused_and_answered_with_the_menu_whole() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    let number = scene.next_input(0);
    let output = scene.menu_click(serial + 100, 0, 0, ClickKind::Pickup);
    let opened = the_opened(&output, player(1));
    assert_eq!(opened.serial, serial, "the menu that is open");
    assert_eq!(opened.applied, number);
    assert_eq!(
        opened.slots[0],
        Some(stack(items::DIRT, 64)),
        "nothing was taken"
    );
    assert_eq!(opened.carried, None);
    assert!(changed(&output, player(1)).is_empty());
    assert_eq!(the_menu(&scene).carried, None);
}

/// Section 8 item 8: the region reports every tick the open menu's slots that changed,
/// not only after its own player's clicks: a second player taking a stack from a chest
/// both have open shows at once in the first one's menu.
#[test]
fn a_change_another_player_makes_to_a_chest_is_reported_to_everyone_who_has_it_open() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let second = scene.join(2, Vec3::new(3.5, -60.0, 0.5));
    let (mine, _) = scene.open(BOX);
    let (theirs, _) = scene.open_by(second, BOX);
    let output = scene.menu_click(
        mine,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    let reported: Vec<Changed> = changed(&output, player(2));
    assert_eq!(reported.len(), 1, "{:?}", output.player_events);
    assert_eq!(reported[0].serial, theirs);
    assert_eq!(reported[0].slots, vec![(0, Some(stack(items::DIRT, 64)))]);
    let output = scene.menu_click_by(second, theirs, 0, 0, ClickKind::Pickup);
    let reported = the_changed(&output, player(1));
    assert_eq!(reported.serial, mine);
    assert_eq!(reported.slots, vec![(0, None)]);
    assert_eq!(
        reported.carried, None,
        "the other player's cursor is not mine"
    );
}

/// Brief 3.18 and section 7 item 15: a close puts the carried stack back into the
/// inventory, the selected hotbar slot first, else the first empty slot, and reports the
/// close and then the slots it filled.
#[test]
fn a_close_puts_the_carried_stack_back_into_the_inventory_and_reports_its_slots() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let number = scene.next_input(0);
    let output = scene.close(serial);
    assert_eq!(closed(&output, player(1)), vec![(serial, number)]);
    let events = events_of(&output, player(1));
    let close_at = events
        .iter()
        .position(|event| matches!(event, PlayerEvent::MenuClosed { .. }))
        .expect("a close");
    let slot_at = events
        .iter()
        .position(|event| matches!(event, PlayerEvent::InventorySlot { .. }))
        .expect("the slot the stack went into");
    assert!(close_at < slot_at, "the close comes first: {events:?}");
    // Hotbar slot 0, selected, was emptied by the quick move; it is empty, so the stack
    // goes to the first empty slot of 0 to 35, which is slot 0 again.
    assert_eq!(
        inventory_slots(&output, player(1)),
        vec![(0, Some(stack(items::DIRT, 64)))]
    );
    assert_eq!(scene.holds(0), Some(stack(items::DIRT, 64)));
    assert!(no_menu(&scene));
}

/// Brief 3.18: a carried stack of an item the selected slot holds tops that slot up
/// first.
#[test]
fn a_carried_stack_put_back_tops_up_the_selected_slot_first() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    // Carry 32 dirt, put 1 into hotbar slot 0 (which the quick move emptied).
    scene.menu_click(serial, 0, 1, ClickKind::Pickup);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        1,
        ClickKind::Pickup,
    );
    // Carried 31; hotbar 0 holds 1 and is selected.
    let output = scene.close(serial);
    assert_eq!(
        inventory_slots(&output, player(1)),
        vec![(0, Some(stack(items::DIRT, 32)))]
    );
}

/// Section 7 item 10: a player who leaves with a stack carried drops it where they are,
/// as the game does for a removed player.
#[test]
fn leaving_with_a_stack_carried_drops_it_where_the_player_stands() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let me = scene.me();
    scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(EDGE, player(1), Some(me), None)],
        ..TickInputs::default()
    });
    assert!(scene.region.player(player(1)).is_none(), "the player left");
    let lying = items_lying(&scene.all());
    assert_eq!(lying.len(), 1, "{lying:?}");
    let (ref dropped, at) = lying[0];
    assert_eq!(*dropped, stack(items::DIRT, 64));
    assert!(
        (at[0] - BESIDE.x).abs() < 1.0 && (at[2] - BESIDE.z).abs() < 1.0,
        "dropped where the player stood: {at:?}"
    );
}

/// Brief 3.0 item 22: a menu closes when its block is broken; the region reports it.
#[test]
fn a_menu_closes_when_its_chest_is_broken() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let second = scene.join(2, Vec3::new(3.5, -60.0, 0.5));
    let (serial, _) = scene.open(BOX);
    scene.actors[second].sequence += 1;
    let sequence = scene.actors[second].sequence;
    let mut seen = vec![scene.input_of(
        second,
        PlayerInput::Dig {
            position: BOX,
            sequence,
        },
    )];
    seen.extend(scene.idle_for(2));
    assert_eq!(scene.block(BOX), blocks::AIR, "the chest is broken");
    let closes: Vec<(u64, u64)> = seen
        .iter()
        .flat_map(|output| closed(output, player(1)))
        .collect();
    assert_eq!(closes.len(), 1, "{closes:?}");
    assert_eq!(closes[0].0, serial);
    assert!(no_menu(&scene));
}

// ------------------------------------------------------------------------------------
// Resync (section 7 item 7; section 8 item 9).

/// Section 7 item 7: a resync is answered with the menu whole.
#[test]
fn a_resync_with_a_menu_open_is_answered_with_the_menu_whole() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 1, ClickKind::Pickup);
    let number = scene.next_input(0);
    let output = scene.input(PlayerInput::MenuResync);
    let opened = the_opened(&output, player(1));
    assert_eq!(opened.serial, serial);
    assert_eq!(opened.applied, number);
    assert_eq!(opened.slots[0], Some(stack(items::DIRT, 32)));
    assert_eq!(opened.carried, Some(stack(items::DIRT, 32)));
}

/// Section 8 item 9: a resync with no menu open is answered with the inventory whole.
#[test]
fn a_resync_with_no_menu_open_is_answered_with_the_inventory_whole() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let number = scene.next_input(0);
    let output = scene.input(PlayerInput::MenuResync);
    let answers: Vec<PlayerEvent> = events_of(&output, player(1))
        .into_iter()
        .filter(|event| matches!(event, PlayerEvent::NoMenu { .. }))
        .collect();
    assert_eq!(
        answers,
        vec![PlayerEvent::NoMenu {
            applied: number,
            inventory: holding()
        }]
    );
    assert!(opened(&output, player(1)).is_empty());
}

// ------------------------------------------------------------------------------------
// A hand-over and a restore with a menu open (section 6 items 1 and 2; section 7
// items 3 and 6).

const BY_THE_LINE: Vec3 = Vec3::new(14.5, -60.0, 2.5);
const OVER_THE_LINE: Vec3 = Vec3::new(16.5, -60.0, 2.5);
const NEAR_THE_LINE: BlockPos = BlockPos::new(13, -60, 4);

/// Section 6 item 2 and section 7 item 6: a player who crosses into another region's
/// land with a chest open has it closed in that tick: the carried stack goes into the
/// inventory, which travels in the transfer, the close is reported, and the chest's
/// openers fall to none.
#[test]
fn a_menu_open_at_a_hand_over_closes_in_that_tick_and_the_carried_stack_travels() {
    let world = World::of(&[(
        NEAR_THE_LINE,
        state("minecraft:chest[facing=north,type=single]"),
    )]);
    let mut scene = Scene::new(Layout::side(true), &world, holding(), BY_THE_LINE);
    let (serial, _) = scene.open(NEAR_THE_LINE);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.menu_click(serial, 0, 1, ClickKind::Pickup);
    assert_eq!(the_menu(&scene).carried, Some(stack(items::DIRT, 32)));
    let output = scene.walk_to(OVER_THE_LINE);
    let gone = departures(&output);
    assert_eq!(gone.len(), 1, "player 1 is let go: {:?}", output.durable);
    let (_, transfer, to) = &gone[0];
    assert_eq!(*to, OTHER);
    let closes = closed(&output, player(1));
    assert_eq!(closes.len(), 1, "{:?}", output.player_events);
    assert_eq!(closes[0].0, serial);
    let dirt: i32 = transfer
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == items::DIRT)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(dirt, 32, "the carried half went back into the inventory");
    // The close sound plays in the tick; the block event of `stopOpen` is kept with its
    // chunk (section 7 item 6), so it runs in the block events of the tick or the next.
    assert_eq!(
        sounds(&output, clustine_data::sounds::BLOCK_CHEST_CLOSE).len(),
        1
    );
    let next = scene.idle();
    assert!(
        [&output, &next]
            .iter()
            .any(|output| block_events_at(output, NEAR_THE_LINE).contains(&(
                block_id(blocks::CHEST),
                1,
                0
            ))),
        "the lid closes: {:?} {:?}",
        output.events,
        next.events
    );
    assert_eq!(
        lying_count(&scene.all(), items::DIRT),
        0,
        "nothing is dropped"
    );
}

/// Section 7 item 3: after a restore a menu whose chest's chunk is held and not loaded
/// stays open; its clicks are refused and answered with no slots; in the tick the chunk
/// loads it is reported whole, under its serial.
#[test]
fn a_menu_open_across_a_restore_stays_and_is_reported_whole_once_its_chunk_loads() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 1, ClickKind::Pickup);
    let state = scene.region.state();
    let saved: Vec<(ChunkPos, clustine_world::Chunk)> = scene
        .region
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| scene.region.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    let mut restored = Region::restore(config(holding()), state, scene.layout.holdings());
    let asking = restored.tick(&TickInputs {
        tickets_added: scene
            .layout
            .loaded
            .iter()
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    });
    assert!(
        opened(&asking, player(1)).is_empty() && closed(&asking, player(1)).is_empty(),
        "nothing is said of the menu before its chunk loads: {:?}",
        asking.player_events
    );
    scene.region = restored;
    assert_eq!(the_menu(&scene).serial, serial, "the menu stays");
    let number = scene.next_input(0);
    let refused = scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let answer = the_changed(&refused, player(1));
    assert_eq!(answer.serial, serial);
    assert_eq!(answer.applied, number);
    assert!(
        answer.slots.is_empty() && answer.carried.is_none(),
        "{answer:?}"
    );
    assert_eq!(the_menu(&scene).carried, Some(stack(items::DIRT, 32)));
    let loading = scene.region.tick(&TickInputs {
        chunks_loaded: saved,
        ..TickInputs::default()
    });
    let whole = the_opened(&loading, player(1));
    assert_eq!(whole.serial, serial);
    assert_eq!(whole.applied, number);
    assert_eq!(whole.kind, MenuKind::Rows3);
    assert_eq!(
        whole.slots[0],
        Some(stack(items::DIRT, 32)),
        "the chest's half"
    );
    assert_eq!(whole.carried, Some(stack(items::DIRT, 32)));
    assert!(closed(&loading, player(1)).is_empty());
}

// ------------------------------------------------------------------------------------
// The ender chest (decision 5; section 6 item 8; section 7 item 8).

/// Decision 5: an ender chest opens the player's own 27 slots, which are in their state
/// and their place in the store's note, and every ender chest shows them.
#[test]
fn an_ender_chest_holds_the_players_own_slots_which_every_ender_chest_shows() {
    let other = BOX.offset(-3, 0, 0);
    let world = World::of(&[
        (BOX, state("minecraft:ender_chest[facing=north]")),
        (other, state("minecraft:ender_chest[facing=north]")),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (serial, _) = scene.open(BOX);
    assert_eq!(the_menu(&scene).of, MenuOf::Ender { at: BOX });
    let output = scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    let ender = scene.region.player_state(player(1)).expect("there").ender;
    assert_eq!(ender.0[0], Some(stack(items::DIRT, 64)));
    let noted = output.stays.iter().find_map(|note| match note {
        StayNote::Has {
            player: who, place, ..
        } if *who == player(1) => Some(place.clone()),
        _ => None,
    });
    let place = noted.expect("the change of the ender chest is a change of the player");
    assert_eq!(place.ender.0[0], Some(stack(items::DIRT, 64)));
    scene.close(serial);
    let (_, output) = scene.open(other);
    let opened = the_opened(&output, player(1));
    assert_eq!(opened.title, "container.enderchest");
    assert_eq!(opened.slots[0], Some(stack(items::DIRT, 64)));
}

/// Section 6 item 8: the ender chest's slots are in the transfer.
#[test]
fn the_ender_chests_slots_go_with_the_player_to_another_region() {
    let world = World::of(&[(NEAR_THE_LINE, state("minecraft:ender_chest[facing=north]"))]);
    let mut scene = Scene::new(Layout::side(true), &world, holding(), BY_THE_LINE);
    let (serial, _) = scene.open(NEAR_THE_LINE);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 1),
        0,
        ClickKind::QuickMove,
    );
    let output = scene.walk_to(OVER_THE_LINE);
    let gone = departures(&output);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].1.ender.0[0], Some(stack(items::STONE, 64)));
    assert_eq!(gone[0].1.inventory.0[1], None);
}

/// A chest's slots are the chest's: what goes into a chest is not in the player's
/// ender chest.
#[test]
fn what_goes_into_a_chest_is_not_in_the_players_ender_chest() {
    let (scene, _) = at_a_chest_with_dirt();
    let ender = scene.region.player_state(player(1)).expect("there").ender;
    assert!(ender.0.iter().all(Option::is_none));
}

/// Brief 3.1: an ender chest under a conductor opens nothing.
#[test]
fn an_ender_chest_under_a_stone_block_opens_nothing() {
    let world = World::of(&[
        (BOX, state("minecraft:ender_chest[facing=north]")),
        (BOX.offset(0, 1, 0), blocks::STONE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let output = scene.click(BOX, Face::North);
    assert!(opened(&output, player(1)).is_empty());
    assert!(no_menu(&scene));
}

// ------------------------------------------------------------------------------------
// More of the rules of clicks (brief 3.0 item 25, 3.11, 3.14).

/// Brief 3.0 item 25 and 3.17: clicks that would throw in the game's handler (a swap
/// or a drag's step outside the window) or name no slot are refused; each is still
/// answered (section 7 item 1), and nothing changes.
#[test]
fn malformed_clicks_are_answered_and_change_nothing() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    let clicks = [
        (-999, 0, ClickKind::Swap),
        (-1, 0, ClickKind::Swap),
        (-5, 0, ClickKind::Pickup),
        (63, 0, ClickKind::Pickup),
        (500, 0, ClickKind::QuickMove),
        (-7, 0, ClickKind::Throw),
    ];
    for (slot, button, kind) in clicks {
        let output = scene.menu_click(serial, slot, button, kind);
        let answers = changed(&output, player(1));
        assert_eq!(
            answers.len(),
            1,
            "{slot} {kind:?}: {:?}",
            output.player_events
        );
        assert!(
            answers[0].slots.is_empty() && answers[0].carried.is_none(),
            "{slot} {kind:?}: {:?}",
            answers[0]
        );
    }
    // A drag whose step names the margin.
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    scene.menu_click(serial, -999, 0, ClickKind::QuickCraft);
    let output = scene.menu_click(serial, -1, 1, ClickKind::QuickCraft);
    assert_eq!(changed(&output, player(1)).len(), 1);
    assert_eq!(the_menu(&scene).carried, Some(stack(items::DIRT, 64)));
    assert_eq!(
        scene
            .region
            .player_state(player(1))
            .expect("there")
            .inventory,
        {
            let mut expected = holding();
            expected.set(0, None);
            expected
        }
    );
}

/// Brief 3.14.1: a click of another kind while a drag is under way only ends the drag.
#[test]
fn a_click_of_another_kind_during_a_drag_only_ends_the_drag() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    scene.menu_click(serial, -999, 0, ClickKind::QuickCraft);
    scene.menu_click(serial, 3, 1, ClickKind::QuickCraft);
    let output = scene.menu_click(serial, 4, 0, ClickKind::Pickup);
    let change = the_changed(&output, player(1));
    assert!(
        change.slots.is_empty() && change.carried.is_none(),
        "{change:?}"
    );
    assert!(the_menu(&scene).drag.is_none());
    assert_eq!(the_menu(&scene).carried, Some(stack(items::DIRT, 64)));
}

/// Brief 3.14.5: the swap's button 40 is the off hand, which is no slot of the menu.
#[test]
fn the_swap_with_the_off_hand_takes_the_slots_stack_into_it() {
    let (mut scene, serial) = at_a_chest_with_dirt();
    let output = scene.menu_click(serial, 0, 40, ClickKind::Swap);
    let change = the_changed(&output, player(1));
    assert_eq!(change.slots, vec![(0, None)]);
    assert_eq!(scene.holds(40), Some(stack(items::DIRT, 64)));
}

/// Brief 3.11: a shulker box's slots refuse shulker boxes; a shift-click of one from
/// the player's part leaves it where it is.
#[test]
fn a_shulker_box_takes_no_shulker_box() {
    let world = World::of(&[(BOX, state("minecraft:shulker_box[facing=up]"))]);
    let mut scene = Scene::alone(
        &world,
        inventory(&[(0, stack(items::SHULKER_BOX, 1))]),
        BESIDE,
    );
    let (serial, _) = scene.open(BOX);
    let from = hotbar_slot(MenuKind::ShulkerBox, 0);
    let output = scene.menu_click(serial, from, 0, ClickKind::QuickMove);
    let change = the_changed(&output, player(1));
    assert!(
        change
            .slots
            .iter()
            .all(|(slot, _)| usize::from(*slot) >= MenuKind::ShulkerBox.container_slots()),
        "nothing went into the box: {change:?}"
    );
    scene.menu_click(serial, from, 0, ClickKind::Pickup);
    let output = scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let change = the_changed(&output, player(1));
    assert!(
        change.slots.is_empty(),
        "not put down in the box: {change:?}"
    );
    assert_eq!(the_menu(&scene).carried, Some(stack(items::SHULKER_BOX, 1)));
}

/// Brief 3.14.9: a shift-click out of a dispenser goes to the hotbar's right end first.
#[test]
fn a_shift_click_out_of_a_dispenser_goes_to_the_hotbars_right_end() {
    let world = World::of(&[(
        BOX,
        state("minecraft:dispenser[facing=north,triggered=false]"),
    )]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (serial, _) = scene.open(BOX);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Grid3x3, 0),
        0,
        ClickKind::QuickMove,
    );
    let output = scene.menu_click(serial, 0, 0, ClickKind::QuickMove);
    let change = the_changed(&output, player(1));
    let mut slots = change.slots.clone();
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(slots, vec![(0, None), (44, Some(stack(items::DIRT, 64)))]);
}

// ------------------------------------------------------------------------------------
// More of serials and validity (section 7 item 2; brief 3.0 item 22, 3.3).

/// Section 7 item 2: a close carries the serial of the screen it closes, and the region
/// refuses one that does not match its menu: a late close of a screen the client has
/// already replaced does not close the menu that is open.
#[test]
fn a_close_of_an_older_serial_leaves_the_open_menu_open() {
    let barrel = BOX.offset(-2, 0, 0);
    let world = World::of(&[
        (BOX, state("minecraft:chest[facing=north,type=single]")),
        (barrel, state("minecraft:barrel[facing=up,open=false]")),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (first, _) = scene.open(BOX);
    let (second, _) = scene.open(barrel);
    let output = scene.close(first);
    assert!(
        closed(&output, player(1)).is_empty(),
        "{:?}",
        output.player_events
    );
    assert_eq!(the_menu(&scene).serial, second);
    assert_eq!(
        scene.block(barrel),
        state("minecraft:barrel[facing=up,open=true]")
    );
}

/// Brief 3.0 item 22: a menu stays valid while the player's eyes are within the reach
/// and 4 more of the block's box, 9 for a creative player, and closes beyond.
#[test]
fn a_menu_closes_when_its_player_walks_out_of_reach_and_four_blocks_more() {
    let (mut scene, serial) = at_an_open_chest();
    let mut seen = Vec::new();
    for x in [2.5, 4.5, 6.5, 8.5, 10.5] {
        seen.push(scene.walk_to(Vec3::new(x, -60.0, 2.5)));
    }
    seen.extend(scene.idle_for(2));
    assert!(
        seen.iter()
            .all(|output| closed(output, player(1)).is_empty()),
        "7.5 blocks from the chest it stays open"
    );
    assert_eq!(the_menu(&scene).serial, serial);
    let mut seen = vec![scene.walk_to(Vec3::new(12.5, -60.0, 2.5))];
    seen.extend(scene.idle_for(2));
    let closes: Vec<(u64, u64)> = seen
        .iter()
        .flat_map(|output| closed(output, player(1)))
        .collect();
    assert_eq!(closes.len(), 1, "9.5 blocks away it closes");
    assert!(no_menu(&scene));
}

/// Brief 3.3: a double chest is blocked when either half is: clicking the free half
/// opens nothing.
#[test]
fn a_double_chest_with_a_block_over_one_half_opens_nothing() {
    let west = BOX;
    let east = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (west, state("minecraft:chest[facing=north,type=left]")),
        (east, state("minecraft:chest[facing=north,type=right]")),
        (west.offset(0, 1, 0), blocks::STONE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let output = scene.click(east, Face::North);
    assert!(opened(&output, player(1)).is_empty());
    assert!(no_menu(&scene));
    assert_eq!(scene.block(east.offset(0, 0, -1)), blocks::AIR);
}

/// Section 7 item 1: every report carries the number of the last input of its own
/// player applied, also one that answers no click of theirs.
#[test]
fn a_report_to_a_second_viewer_carries_their_own_last_input() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let second = scene.join(2, Vec3::new(3.5, -60.0, 0.5));
    let (mine, _) = scene.open(BOX);
    let (_, _) = scene.open_by(second, BOX);
    let theirs = scene.actors[second].input;
    let output = scene.menu_click(
        mine,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    let reported = the_changed(&output, player(2));
    assert_eq!(reported.applied, theirs);
}

// ------------------------------------------------------------------------------------
// Containers of vehicles (brief 3.9; plan section 1).

fn cart(kind: i32, position: [f64; 3]) -> clustine_world::Entity {
    clustine_world::Entity {
        key: key(1),
        kind: u16::try_from(kind).expect("fits"),
        uuid: uuid::Uuid::from_u128(7_001),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: clustine_world::EntityData::Minecart {
            hurt: clustine_world::Hurt::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents: Vec::new(),
            fuel: 0,
            push: [clustine_world::Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
    }
}

/// Brief 3.9: a chest cart opens three rows titled with its type's name, and a hopper
/// cart a hopper's five; what goes in is the cart's contents.
#[test]
fn a_chest_cart_and_a_hopper_cart_open_their_menus_on_a_use() {
    for (kind, menu, title) in [
        (
            clustine_data::entity_types::CHEST_MINECART,
            MenuKind::Rows3,
            "entity.minecraft.chest_minecart",
        ),
        (
            clustine_data::entity_types::HOPPER_MINECART,
            MenuKind::Hopper,
            "entity.minecraft.hopper_minecart",
        ),
    ] {
        let world = World::default().with_entities(&[cart(kind, [4.5, -60.0, 2.5])]);
        let mut scene = Scene::alone(&world, holding(), BESIDE);
        let serial = scene.next_input(0);
        let output = scene.input(PlayerInput::Interact {
            entity: key(1),
            location: [0.0, 0.5, 0.0],
            sneaking: false,
        });
        let opened = the_opened(&output, player(1));
        assert_eq!((opened.kind, opened.title.as_str()), (menu, title));
        assert_eq!(opened.serial, serial);
        assert_eq!(the_menu(&scene).of, MenuOf::Entity(key(1)));
        scene.menu_click(serial, hotbar_slot(menu, 0), 0, ClickKind::QuickMove);
        let record = scene
            .all()
            .into_iter()
            .find(|entity| entity.key == key(1))
            .expect("the cart");
        match record.data {
            clustine_world::EntityData::Minecart { contents, .. } => {
                assert_eq!(contents, vec![(0, stack(items::DIRT, 64))], "{title}");
            }
            other => panic!("{other:?}"),
        }
    }
}

// ------------------------------------------------------------------------------------
// At a pinned line (section 6 item 2; section 7 item 6; section 10 item 3).

/// Section 7 item 6: what of the carried stack does not fit back into the inventory at
/// a hand-over is dropped as an arrival at the player's new region, not laid in this
/// region's land.
#[test]
fn a_carried_stack_that_does_not_fit_at_a_hand_over_goes_as_an_arrival() {
    let world = World::of(&[(
        NEAR_THE_LINE,
        state("minecraft:chest[facing=north,type=single]"),
    )]);
    let full: Vec<(usize, ItemStack)> = (0..36)
        .map(|slot| (slot, stack(items::STONE, 64)))
        .collect();
    let mut scene = Scene::new(Layout::side(true), &world, inventory(&full), BY_THE_LINE);
    let (serial, _) = scene.open(NEAR_THE_LINE);
    // A creative middle click carries a full stack of stone, which no slot has room for.
    scene.menu_click(serial, hotbar_slot(MenuKind::Rows3, 0), 2, ClickKind::Clone);
    assert_eq!(the_menu(&scene).carried, Some(stack(items::STONE, 64)));
    let output = scene.walk_to(OVER_THE_LINE);
    assert_eq!(departures(&output).len(), 1);
    assert_eq!(
        lying_count(&scene.all(), items::STONE),
        0,
        "nothing laid here"
    );
    let sent: Vec<(ChunkPos, ItemStack)> = output
        .arrivals
        .iter()
        .filter_map(|(chunk, entity)| match &entity.data {
            clustine_world::EntityData::Item { stack, .. } => Some((*chunk, stack.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "{:?}", output.arrivals);
    assert!(
        sent[0].0.x >= 1,
        "into the new region's land: {:?}",
        sent[0].0
    );
    assert_eq!(sent[0].1, stack(items::STONE, 64));
}

/// Section 10 item 3 and section 6 item 2: a chest a step across a pinned line is the
/// other region's; a use of it is a remote action there, which consumes the click and
/// opens nothing, and places nothing either.
#[test]
fn a_chest_across_a_pinned_line_opens_nothing_from_this_side() {
    let across = BlockPos::new(16, -60, 4);
    let world = World::of(&[(across, state("minecraft:chest[facing=north,type=single]"))]);
    let mut west = Scene::new(Layout::side(true), &world, holding(), BY_THE_LINE);
    let mut east = region_of(&Layout::side(false), &world, holding());
    let output = west.click(across, Face::Top);
    assert!(opened(&output, player(1)).is_empty());
    let actions: Vec<clustine_sim::RemoteAction> = output
        .durable
        .iter()
        .filter_map(|(_, _, durable)| match durable {
            clustine_sim::Durable::Remote { action, .. } => Some(action.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 1, "{:?}", output.durable);
    let answer = east.tick(&TickInputs {
        remote_actions: vec![(EDGE, actions[0].clone())],
        ..TickInputs::default()
    });
    assert!(
        answer.player_events.is_empty(),
        "{:?}",
        answer.player_events
    );
    assert_eq!(
        block_at(&east, across.offset(0, 1, 0)),
        blocks::AIR,
        "no dirt placed"
    );
    assert!(
        answer
            .durable
            .iter()
            .any(|(_, _, durable)| matches!(durable, clustine_sim::Durable::RemoteDone { .. })),
        "dealt with: {:?}",
        answer.durable
    );
    assert!(no_menu(&west));
}

/// Section 6 item 1 and section 7 item 5: the openers' count is saved with the chest;
/// a region restored with its chunks and a menu open on it counts it again and sends
/// its block event again, without closing it.
#[test]
fn a_restored_region_sends_an_open_chests_count_again_and_keeps_it_open() {
    let (scene, serial) = at_an_open_chest();
    let state = scene.region.state();
    let tickets: Vec<(ChunkPos, Ticket)> = scene
        .layout
        .loaded
        .iter()
        .map(|chunk| (*chunk, Ticket::Viewer))
        .collect();
    let saved: Vec<(ChunkPos, clustine_world::Chunk)> = scene
        .region
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| scene.region.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    let mut restored = Region::restore_loaded(
        config(holding()),
        state,
        scene.layout.holdings(),
        &tickets,
        saved,
        Vec::new(),
    );
    let outputs: Vec<clustine_sim::TickOutput> = (0..7)
        .map(|_| restored.tick(&TickInputs::default()))
        .collect();
    let chest = block_id(blocks::CHEST);
    assert!(
        outputs
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(chest, 1, 1))),
        "the count is sent again"
    );
    assert!(outputs.iter().all(|output| {
        sounds(output, clustine_data::sounds::BLOCK_CHEST_CLOSE).is_empty()
            && sounds(output, clustine_data::sounds::BLOCK_CHEST_OPEN).is_empty()
            && closed(output, player(1)).is_empty()
    }));
    assert_eq!(
        restored
            .player_state(player(1))
            .expect("there")
            .menu
            .expect("open")
            .serial,
        serial
    );
}

/// Section 7 item 5: a chest restored with a saved count of openers that no menu
/// accounts for heals at its recheck: the close sound plays and its count falls to
/// none.
#[test]
fn a_chest_saved_open_with_nobody_at_it_closes_at_its_recheck() {
    let mut scene = Scene::alone(&chest(), holding(), BESIDE);
    let before = scene.region.state();
    scene.open(BOX);
    scene.idle_for(2);
    let tickets: Vec<(ChunkPos, Ticket)> = scene
        .layout
        .loaded
        .iter()
        .map(|chunk| (*chunk, Ticket::Viewer))
        .collect();
    let saved: Vec<(ChunkPos, clustine_world::Chunk)> = scene
        .region
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| scene.region.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    let mut restored = Region::restore_loaded(
        config(holding()),
        before,
        scene.layout.holdings(),
        &tickets,
        saved,
        Vec::new(),
    );
    let outputs: Vec<clustine_sim::TickOutput> = (0..8)
        .map(|_| restored.tick(&TickInputs::default()))
        .collect();
    assert!(
        outputs
            .iter()
            .any(|output| !sounds(output, clustine_data::sounds::BLOCK_CHEST_CLOSE).is_empty()),
        "the lid closes with its sound"
    );
    assert!(
        outputs
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(block_id(blocks::CHEST), 1, 0))),
        "the count falls to none"
    );
}

/// The saved chunks and state of a scene's region, to restore it from.
fn saved_of(
    scene: &Scene,
) -> (
    clustine_sim::RegionState,
    Vec<(ChunkPos, clustine_world::Chunk)>,
) {
    let saved = scene
        .region
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| scene.region.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    (scene.region.state(), saved)
}

/// Section 6 item 1, as section 7 items 3 and 7 settle it: a restored region reports a
/// menu whole once its container is loaded, and says nothing of a player who has none at
/// its first tick; a client that still shows a window the restored region does not have
/// is set right by the resync its edge sends on the resumed link, which a region answers
/// for a player with no menu with [`PlayerEvent::NoMenu`]. (The plan's "and for one that
/// has none" meant a menu with no container to wait for; the test of this file's first
/// reading took it as a player with no menu, and is kept in this form.)
#[test]
fn after_a_restore_a_resync_says_of_a_player_with_no_menu_that_they_have_none() {
    let scene = Scene::alone(&chest(), holding(), BESIDE);
    let (state, _) = saved_of(&scene);
    let mut restored = Region::restore(config(holding()), state, scene.layout.holdings());
    let tickets = TickInputs {
        tickets_added: scene
            .layout
            .loaded
            .iter()
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    };
    let first = restored.tick(&tickets);
    assert!(
        events_of(&first, player(1))
            .iter()
            .all(|event| !matches!(event, PlayerEvent::NoMenu { .. })),
        "nothing is said unasked: {:?}",
        first.player_events
    );
    let resync = TickInputs {
        inputs: vec![(
            EDGE,
            player(1),
            scene.me(),
            scene.next_input(0),
            PlayerInput::MenuResync,
        )],
        ..TickInputs::default()
    };
    let answered = restored.tick(&resync);
    let said: Vec<PlayerEvent> = events_of(&answered, player(1))
        .into_iter()
        .filter(|event| matches!(event, PlayerEvent::NoMenu { .. }))
        .collect();
    assert_eq!(said.len(), 1, "{:?}", answered.player_events);
}

/// Section 7 item 3: a menu whose container's chunk the restored region does not hold
/// is closed, and its carried stack is put back.
#[test]
fn a_restored_menu_whose_chest_is_in_a_chunk_no_longer_held_closes() {
    let chest_at = BlockPos::new(2, -60, 17);
    let world = World::of(&[(chest_at, state("minecraft:chest[facing=north,type=single]"))]);
    let mut scene = Scene::alone(&world, holding(), Vec3::new(2.5, -60.0, 14.5));
    let (serial, _) = scene.open(chest_at);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.menu_click(serial, 0, 0, ClickKind::Pickup);
    let (state, _) = saved_of(&scene);
    let mut holdings = scene.layout.holdings();
    holdings.held.retain(|chunk| chunk.z <= 0);
    let mut restored = Region::restore(config(holding()), state, holdings);
    let mut outputs = vec![restored.tick(&TickInputs {
        tickets_added: vec![(ChunkPos::new(0, 0), Ticket::Viewer)],
        ..TickInputs::default()
    })];
    outputs.push(restored.tick(&TickInputs::default()));
    let closes: Vec<(u64, u64)> = outputs
        .iter()
        .flat_map(|output| closed(output, player(1)))
        .collect();
    assert_eq!(closes.len(), 1, "{:?}", outputs[0].player_events);
    assert_eq!(closes[0].0, serial);
    let player_state = restored.player_state(player(1)).expect("there");
    assert!(player_state.menu.is_none());
    assert!(
        player_state
            .inventory
            .0
            .iter()
            .any(|stack| *stack == Some(stack_of_dirt())),
        "the carried dirt went back"
    );
}

fn stack_of_dirt() -> ItemStack {
    stack(items::DIRT, 64)
}

/// Section 6 item 2: the close at a hand-over is of the move that crossed: the close and
/// the transfer name it as the last input applied.
#[test]
fn the_close_at_a_hand_over_names_the_move_that_crossed() {
    let world = World::of(&[(
        NEAR_THE_LINE,
        state("minecraft:chest[facing=north,type=single]"),
    )]);
    let mut scene = Scene::new(Layout::side(true), &world, holding(), BY_THE_LINE);
    let (serial, _) = scene.open(NEAR_THE_LINE);
    let number = scene.next_input(0);
    let output = scene.walk_to(OVER_THE_LINE);
    assert_eq!(closed(&output, player(1)), vec![(serial, number)]);
    assert_eq!(departures(&output)[0].1.last_input, number);
}

/// Section 7 item 3: a vehicle's container is loaded with the vehicle, so a menu on it
/// is reported whole at the first tick it is loaded after a restore.
#[test]
fn a_menu_on_a_chest_cart_is_reported_whole_once_its_cart_is_loaded_after_a_restore() {
    let world = World::default().with_entities(&[cart(
        clustine_data::entity_types::CHEST_MINECART,
        [4.5, -60.0, 2.5],
    )]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let serial = scene.next_input(0);
    scene.input(PlayerInput::Interact {
        entity: key(1),
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    });
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    let (state, saved) = saved_of(&scene);
    let tickets: Vec<(ChunkPos, Ticket)> = scene
        .layout
        .loaded
        .iter()
        .map(|chunk| (*chunk, Ticket::Viewer))
        .collect();
    let mut restored = Region::restore_loaded(
        config(holding()),
        state,
        scene.layout.holdings(),
        &tickets,
        saved,
        Vec::new(),
    );
    let first = restored.tick(&TickInputs::default());
    let whole = the_opened(&first, player(1));
    assert_eq!(whole.serial, serial);
    assert_eq!(whole.slots[0], Some(stack(items::DIRT, 64)));
}

// ------------------------------------------------------------------------------------
// A split with a menu open (section 6 items 1 and 2; section 7 item 3).

/// Player 1 at a chest in chunk (1, 0), standing in chunk (2, 0) with its menu open and
/// half its dirt carried, and player 2 at home; then the region split, the chunks of x
/// = 2 named for the part. Returns the region that stays, the part, the chunks each is
/// to load, and the menu's serial.
fn split_at_a_chest(chest_at: BlockPos) -> Split {
    let world = World::of(&[(chest_at, state("minecraft:chest[facing=north,type=single]"))]);
    let mut scene = Scene::alone(&world, holding(), Vec3::new(33.5, -60.0, 2.5));
    scene.join(2, Vec3::new(0.5, -60.0, 0.5));
    let (serial, opened) = scene.open(chest_at);
    the_opened(&opened, player(1));
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.menu_click(serial, 0, 1, ClickKind::Pickup);
    let named: Vec<ChunkPos> = scene
        .region
        .loaded_chunks()
        .into_iter()
        .filter(|chunk| chunk.x == 2)
        .collect();
    let splitting = match scene.region.split(&named, OTHER, &[]) {
        Ok(splitting) => splitting,
        Err(refused) => panic!("the region splits: {refused:?}"),
    };
    let (kept, part) = scene.region.take_split(splitting, &[]);
    Split {
        stays: scene.region,
        part: part.region,
        kept,
        theirs: part.chunks,
        serial,
    }
}

/// What [`split_at_a_chest`] leaves.
struct Split {
    stays: Region,
    part: Region,
    kept: Vec<(ChunkPos, clustine_world::Chunk)>,
    theirs: Vec<(ChunkPos, clustine_world::Chunk)>,
    serial: u64,
}

/// Loads `chunks` into `region` with viewers' tickets on them, as a worker would, and
/// returns the outputs of the ticks that did so and of `then` more.
fn load(
    region: &mut Region,
    chunks: Vec<(ChunkPos, clustine_world::Chunk)>,
    copies: Vec<(ChunkPos, clustine_world::Chunk)>,
    then: u32,
) -> Vec<clustine_sim::TickOutput> {
    let mut outputs = vec![
        region.tick(&TickInputs {
            tickets_added: chunks
                .iter()
                .map(|(chunk, _)| (*chunk, Ticket::Viewer))
                .collect(),
            ..TickInputs::default()
        }),
    ];
    outputs.push(region.tick(&TickInputs {
        chunks_loaded: chunks,
        copy_bases: copies,
        ..TickInputs::default()
    }));
    for _ in 0..then {
        outputs.push(region.tick(&TickInputs::default()));
    }
    outputs
}

/// Section 6 item 2: a menu closes when a chunk of its container is not held: the
/// player goes to the part and the chest stays, so the part closes the menu, puts the
/// carried stack back, and the chest's region, finding nobody at it at its recheck,
/// closes its lid.
#[test]
fn a_split_that_parts_a_player_from_their_open_chest_closes_the_menu() {
    let chest_at = BlockPos::new(31, -60, 2);
    let Split {
        mut stays,
        mut part,
        kept,
        theirs,
        serial,
    } = split_at_a_chest(chest_at);
    assert!(
        part.player(player(1)).is_some(),
        "player 1 went with the part"
    );
    assert!(
        theirs.iter().all(|(chunk, _)| *chunk != chest_at.chunk()),
        "the chest stayed"
    );
    let copies = theirs.clone();
    let outputs = load(&mut part, theirs, Vec::new(), 2);
    let closes: Vec<(u64, u64)> = outputs
        .iter()
        .flat_map(|output| closed(output, player(1)))
        .collect();
    assert_eq!(closes.len(), 1, "{:?}", outputs[0].player_events);
    assert_eq!(closes[0].0, serial);
    let state = part.player_state(player(1)).expect("there");
    assert!(state.menu.is_none());
    let dirt: i32 = state
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == items::DIRT)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(dirt, 32, "the carried half went back");
    // The part's chunks are copies to the region that stays, as the runner hands them
    // over, so that the chest's chunk beside them ticks.
    let outputs = load(&mut stays, kept, copies, 8);
    assert!(
        outputs
            .iter()
            .any(|output| !sounds(output, clustine_data::sounds::BLOCK_CHEST_CLOSE).is_empty()),
        "the lid closes"
    );
    assert!(
        outputs
            .iter()
            .any(|output| block_events_at(output, chest_at).contains(&(
                block_id(blocks::CHEST),
                1,
                0
            )))
    );
}

/// Section 7 item 3: a menu whose chest goes with the player to the part stays open,
/// and is reported whole when its chunk loads there.
#[test]
fn a_split_that_takes_a_player_and_their_chest_keeps_the_menu_and_reports_it_whole() {
    let chest_at = BlockPos::new(35, -60, 4);
    let Split {
        mut part,
        theirs,
        serial,
        ..
    } = split_at_a_chest(chest_at);
    assert!(theirs.iter().any(|(chunk, _)| *chunk == chest_at.chunk()));
    let outputs = load(&mut part, theirs, Vec::new(), 6);
    assert!(
        outputs
            .iter()
            .all(|output| closed(output, player(1)).is_empty())
    );
    let whole: Vec<Opened> = outputs
        .iter()
        .flat_map(|output| opened(output, player(1)))
        .collect();
    assert_eq!(whole.len(), 1, "{whole:?}");
    assert_eq!(whole[0].serial, serial);
    assert_eq!(whole[0].slots[0], Some(stack(items::DIRT, 32)));
    assert_eq!(whole[0].carried, Some(stack(items::DIRT, 32)));
    assert!(
        outputs
            .iter()
            .all(|output| sounds(output, clustine_data::sounds::BLOCK_CHEST_CLOSE).is_empty()),
        "the lid stays open"
    );
}
