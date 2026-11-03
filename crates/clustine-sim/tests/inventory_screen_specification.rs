//! Tests of window 0, the player's own inventory screen with its 2×2 crafting grid (V3a),
//! written from the game's classes by someone who did not write the code, through the
//! region's public interface alone: its layout (`InventoryMenu.<init>`), its clicks
//! (`ServerGamePacketListenerImpl.handleContainerClick`), its armour slots (`ArmorSlot`)
//! and its shift-clicks (`InventoryMenu.quickMoveStack`), the equip sound
//! (`LivingEntity.onEquipItem`), its close (`handleContainerClose`, `InventoryMenu.removed`),
//! a chest open over it, a hand-over across a line (the game never closes it), a change
//! of level and a death, and a join and a leave with a grid kept.
//!
//! Window 0 is named by the serial 0 in the region's inputs and reports. The classes
//! cited are those of 26.3 (`/home/claude/reference/v3-javap` and `n5-javap`), with the
//! offsets of their bytecode. Nothing here makes a crafting result: the grids hold items
//! no recipe of the 2×2 grid takes together.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60), as in the scenes of
//! `menus_specification.rs`.

mod n6_scene;

use clustine_data::{blocks, items, sounds};
use clustine_sim::api::{
    ClickKind, Inventory, ItemStack, MenuKind, PlayerEvent, PlayerInput, RegionEvent,
    SOUND_SOURCE_PLAYERS, StayNote,
};
use clustine_sim::{Crafting, PlayerChange, TickInputs, TickOutput};
use clustine_world::{BlockPos, Dimension, GameMode, PlayerMode, RegionId, Vec3};
use n6_scene::*;

// ------------------------------------------------------------------------------------
// Window 0's slots (`InventoryMenu.<init>`): 0 the result, 1-4 the grid row by row, 5-8
// the armour from the head to the feet, 9-35 the main inventory, 36-44 the hotbar, 45 the
// off hand.

const RESULT: i16 = 0;
const GRID: [i16; 4] = [1, 2, 3, 4];
const HEAD: i16 = 5;
const CHEST: i16 = 6;
const LEGS: i16 = 7;
const FEET: i16 = 8;
const MAIN: i16 = 9;
const HOTBAR: i16 = 36;
const OFF_HAND: i16 = 45;

/// The inventory's own numbers of the armour and the off hand.
const INVENTORY_HEAD: usize = 39;
const INVENTORY_CHEST: usize = 38;
const INVENTORY_LEGS: usize = 37;
const INVENTORY_FEET: usize = 36;
const INVENTORY_OFF_HAND: usize = 40;

/// The equipment slots' ids as an equipment update names them (`EquipmentSlot` ordinals).
const EQUIPMENT_HEAD: u8 = 5;

/// Where the chest of these tests stands, beside player 1.
const BOX: BlockPos = BlockPos::new(2, -60, 2);
const BESIDE: Vec3 = Vec3::new(0.5, -60.0, 0.5);

fn survival() -> PlayerMode {
    PlayerMode::new(GameMode::Survival)
}

fn creative() -> PlayerMode {
    PlayerMode::new(GameMode::Creative)
}

/// A scene of `layout` over `world` with player 1 at `feet` in `mode`, holding
/// `inventory`, their screen's grid and cursor `crafting`.
fn scene_of(
    layout: Layout,
    world: &World,
    inventory: Inventory,
    feet: Vec3,
    mode: PlayerMode,
    crafting: Crafting,
) -> Scene {
    let region = region_of(&layout, world, inventory.clone());
    let mut scene = Scene {
        region,
        layout,
        inventory,
        actors: Vec::new(),
    };
    scene.join_with(1, feet, |place| {
        place.mode = mode;
        place.crafting = crafting;
    });
    // A tick for whatever the join reports to pass.
    scene.idle();
    scene
}

/// Player 1 alone, in `mode`, with `inventory` and nothing on their screen of its own.
fn alone(inventory: Inventory, mode: PlayerMode) -> Scene {
    scene_of(
        Layout::alone(),
        &World::default(),
        inventory,
        BESIDE,
        mode,
        Crafting::default(),
    )
}

/// A single chest at [`BOX`], facing north.
fn chest() -> World {
    World::of(&[(BOX, state("minecraft:chest[facing=north,type=single]"))])
}

/// A stack of `item` with Curse of Binding.
fn cursed(item: i32) -> ItemStack {
    let mut stack = stack(item, 1);
    stack
        .components
        .set(clustine_world::Component::Enchantments(vec![(
            "minecraft:binding_curse".to_owned(),
            1,
        )]));
    stack
}

/// Player 1 clicks `slot` of window 0.
fn click(scene: &mut Scene, slot: i16, button: i8, kind: ClickKind) -> TickOutput {
    scene.menu_click(0, slot, button, kind)
}

fn pick(scene: &mut Scene, slot: i16) -> TickOutput {
    click(scene, slot, 0, ClickKind::Pickup)
}

fn shift(scene: &mut Scene, slot: i16) -> TickOutput {
    click(scene, slot, 0, ClickKind::QuickMove)
}

/// Window 0's reports of the tick to `who`: its changes and its wholes.
fn window_0(output: &TickOutput, who: clustine_world::PlayerId) -> Vec<PlayerEvent> {
    events_of(output, who)
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                PlayerEvent::MenuChanged { serial: 0, .. }
                    | PlayerEvent::MenuOpened { serial: 0, .. }
            )
        })
        .collect()
}

/// A change of window 0 as reported: the input it says was applied last, the slots and
/// the cursor.
type Change = (
    u64,
    Vec<(u16, Option<ItemStack>)>,
    Option<Option<ItemStack>>,
);

/// The one change of window 0 reported to player 1 in the tick.
fn the_change(output: &TickOutput) -> Change {
    let reports = window_0(output, player(1));
    match &reports[..] {
        [
            PlayerEvent::MenuChanged {
                applied,
                slots,
                carried,
                ..
            },
        ] => (*applied, slots.clone(), carried.clone()),
        _ => panic!("one change of window 0: {:?}", output.player_events),
    }
}

/// Window 0 whole as reported to player 1 in the tick: its slots and cursor.
fn the_whole(output: &TickOutput) -> (u64, Vec<Option<ItemStack>>, Option<ItemStack>) {
    let reports = window_0(output, player(1));
    let wholes: Vec<_> = reports
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::MenuOpened {
                applied,
                kind,
                slots,
                carried,
                ..
            } => {
                assert_eq!(kind, MenuKind::Inventory);
                Some((applied, slots, carried))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        wholes.len(),
        1,
        "window 0 whole once: {:?}",
        output.player_events
    );
    wholes.into_iter().next().expect("one")
}

fn crafting(scene: &Scene) -> Crafting {
    scene
        .region
        .player_state(player(1))
        .expect("player 1 is there")
        .crafting
}

fn equip_sounds(output: &TickOutput) -> Vec<(i32, i32, Vec3, Option<clustine_world::PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                source,
                position,
                except,
                ..
            } if [
                sounds::ITEM_ARMOR_EQUIP_GENERIC,
                sounds::ITEM_ARMOR_EQUIP_IRON,
                sounds::ITEM_ARMOR_EQUIP_DIAMOND,
            ]
            .contains(sound) =>
            {
                Some((*sound, *source, *position, *except))
            }
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------
// Item 1: the layout.

/// `InventoryMenu.<init>`: the result (`ResultSlot`, 0), the grid row by row (1-4), the
/// armour from the head down (5-8, `ArmorSlot` over the inventory's 39 to 36), the main
/// inventory (9-35, the inventory's 9-35), the hotbar (36-44, the inventory's 0-8) and
/// the off hand (45, the inventory's 40). A resync with no other menu open has window 0
/// reported whole.
#[test]
fn window_0_whole_lays_out_the_result_the_grid_the_armour_head_down_the_main_inventory_the_hotbar_and_the_off_hand()
 {
    let held = inventory(&[
        (0, stack(items::STONE, 1)),
        (8, stack(items::STONE, 9)),
        (9, stack(items::STONE, 10)),
        (35, stack(items::STONE, 36)),
        (INVENTORY_HEAD, stack(items::IRON_HELMET, 1)),
        (INVENTORY_CHEST, stack(items::DIAMOND_CHESTPLATE, 1)),
        (INVENTORY_LEGS, stack(items::IRON_LEGGINGS, 1)),
        (INVENTORY_FEET, stack(items::IRON_BOOTS, 1)),
        (INVENTORY_OFF_HAND, stack(items::SHIELD, 1)),
    ]);
    let grid = Crafting {
        grid: [
            Some(stack(items::DIRT, 1)),
            Some(stack(items::COBBLESTONE, 2)),
            Some(stack(items::GRAVEL, 3)),
            Some(stack(items::SAND, 4)),
        ],
        ..Crafting::default()
    };
    let mut scene = scene_of(
        Layout::alone(),
        &World::default(),
        held,
        BESIDE,
        survival(),
        grid,
    );
    let number = scene.next_input(0);
    let output = scene.input(PlayerInput::MenuResync);
    let (applied, slots, carried) = the_whole(&output);
    assert_eq!(applied, number);
    assert_eq!(slots.len(), 46);
    assert_eq!(carried, None);
    let at = |slot: i16| slots[usize::try_from(slot).expect("a slot")].clone();
    assert_eq!(at(RESULT), None, "no recipe takes these four");
    assert_eq!(at(GRID[0]), Some(stack(items::DIRT, 1)));
    assert_eq!(at(GRID[1]), Some(stack(items::COBBLESTONE, 2)));
    assert_eq!(at(GRID[2]), Some(stack(items::GRAVEL, 3)));
    assert_eq!(at(GRID[3]), Some(stack(items::SAND, 4)));
    assert_eq!(at(HEAD), Some(stack(items::IRON_HELMET, 1)));
    assert_eq!(at(CHEST), Some(stack(items::DIAMOND_CHESTPLATE, 1)));
    assert_eq!(at(LEGS), Some(stack(items::IRON_LEGGINGS, 1)));
    assert_eq!(at(FEET), Some(stack(items::IRON_BOOTS, 1)));
    assert_eq!(at(MAIN), Some(stack(items::STONE, 10)), "the inventory's 9");
    assert_eq!(at(35), Some(stack(items::STONE, 36)), "the inventory's 35");
    assert_eq!(
        at(HOTBAR),
        Some(stack(items::STONE, 1)),
        "the inventory's 0"
    );
    assert_eq!(at(44), Some(stack(items::STONE, 9)), "the inventory's 8");
    assert_eq!(
        at(OFF_HAND),
        Some(stack(items::SHIELD, 1)),
        "the inventory's 40"
    );
    let filled = slots.iter().filter(|slot| slot.is_some()).count();
    assert_eq!(filled, 13, "nothing else: {slots:?}");
}

// ------------------------------------------------------------------------------------
// Item 2: clicks and their answers.

/// `handleContainerClick` @0-36 (the container id is the open menu's, 0 with none open)
/// and `AbstractContainerMenu.doClick`'s pick-up: a survival player's left click on a
/// hotbar stack carries it, a second on an empty main slot puts it down; each answer
/// names its input and says what changed (`broadcastChanges`).
#[test]
fn a_survival_players_clicks_on_window_0_pick_a_stack_up_and_put_it_down() {
    let mut scene = alone(inventory(&[(0, stack(items::DIRT, 64))]), survival());
    let number = scene.next_input(0);
    let output = pick(&mut scene, HOTBAR);
    assert_eq!(
        the_change(&output),
        (number, vec![(36, None)], Some(Some(stack(items::DIRT, 64))))
    );
    assert_eq!(crafting(&scene).carried, Some(stack(items::DIRT, 64)));
    let number = scene.next_input(0);
    let output = pick(&mut scene, MAIN + 3);
    assert_eq!(
        the_change(&output),
        (number, vec![(12, Some(stack(items::DIRT, 64)))], Some(None))
    );
    assert_eq!(scene.holds(0), None);
    assert_eq!(scene.holds(12), Some(stack(items::DIRT, 64)));
    assert_eq!(crafting(&scene).carried, None);
}

/// `handleContainerClick` asks nothing of the game mode: a creative player's
/// shift-click on window 0 moves a hotbar stack into the main inventory
/// (`quickMoveStack` @277-300: 36-44 into 9-35).
#[test]
fn a_creative_players_shift_click_on_window_0_is_taken_as_a_survival_players() {
    let mut scene = alone(inventory(&[(0, stack(items::DIRT, 64))]), creative());
    let number = scene.next_input(0);
    let output = shift(&mut scene, HOTBAR);
    let (applied, mut slots, carried) = the_change(&output);
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(applied, number);
    assert_eq!(slots, vec![(9, Some(stack(items::DIRT, 64))), (36, None)]);
    assert_eq!(carried, None);
    assert_eq!(scene.holds(9), Some(stack(items::DIRT, 64)));
}

/// `broadcastChanges` sends nothing where nothing changed: a click on an empty slot with
/// nothing carried is answered, naming its input, with no slot and no cursor.
#[test]
fn a_click_on_window_0_that_changes_nothing_is_answered_with_nothing_changed() {
    let mut scene = alone(inventory(&[(0, stack(items::DIRT, 64))]), survival());
    let number = scene.next_input(0);
    let output = pick(&mut scene, MAIN + 5);
    assert_eq!(the_change(&output), (number, Vec::new(), None));
}

/// `handleContainerClick` @19-36: with a chest open the player's menu is the chest's, and
/// a click naming window 0 is passed over and answered with nothing, the chest's
/// window included.
#[test]
fn a_click_on_window_0_while_a_chest_is_open_changes_nothing_and_is_not_answered() {
    let mut scene = scene_of(
        Layout::alone(),
        &chest(),
        inventory(&[(0, stack(items::DIRT, 64))]),
        BESIDE,
        survival(),
        Crafting::default(),
    );
    let (serial, _) = scene.open(BOX);
    let number = scene.next_input(0);
    let output = pick(&mut scene, HOTBAR);
    assert_eq!(window_0(&output, player(1)), []);
    let of_the_click: Vec<PlayerEvent> = events_of(&output, player(1))
        .into_iter()
        .filter(|event| match event {
            PlayerEvent::MenuOpened { applied, .. }
            | PlayerEvent::MenuChanged { applied, .. }
            | PlayerEvent::MenuClosed { applied, .. }
            | PlayerEvent::NoMenu { applied, .. } => *applied == number,
            _ => false,
        })
        .collect();
    assert_eq!(of_the_click, []);
    assert_eq!(scene.holds(0), Some(stack(items::DIRT, 64)));
    assert_eq!(crafting(&scene).carried, None);
    let menu = scene
        .region
        .player_state(player(1))
        .expect("there")
        .menu
        .expect("the chest is still open");
    assert_eq!(menu.serial, serial);
    assert_eq!(menu.carried, None);
}

/// `handleContainerClick` @37-67: a dead player's click is answered with the window
/// whole (`sendAllDataToRemote`) and runs nothing; so is a spectator's.
#[test]
fn a_spectators_click_on_window_0_is_answered_with_window_0_whole_and_runs_nothing() {
    let mut scene = alone(
        inventory(&[(0, stack(items::DIRT, 64))]),
        PlayerMode::new(GameMode::Spectator),
    );
    let number = scene.next_input(0);
    let output = pick(&mut scene, HOTBAR);
    let (applied, slots, carried) = the_whole(&output);
    assert_eq!(applied, number);
    assert_eq!(slots[36], Some(stack(items::DIRT, 64)));
    assert_eq!(carried, None);
    assert_eq!(scene.holds(0), Some(stack(items::DIRT, 64)));
}

// ------------------------------------------------------------------------------------
// Item 3: the armour slots and the shift-click.

/// `ArmorSlot.getMaxStackSize` is 1: a left click with 64 carved pumpkins (worn on the
/// head, `Equippable` of slot HEAD) puts one on and keeps 63 on the cursor.
#[test]
fn an_armour_slot_takes_one_of_a_carried_stack() {
    let mut scene = alone(
        inventory(&[(0, stack(items::CARVED_PUMPKIN, 64))]),
        survival(),
    );
    pick(&mut scene, HOTBAR);
    let output = pick(&mut scene, HEAD);
    let (_, slots, carried) = the_change(&output);
    assert_eq!(slots, vec![(5, Some(stack(items::CARVED_PUMPKIN, 1)))]);
    assert_eq!(carried, Some(Some(stack(items::CARVED_PUMPKIN, 63))));
    assert_eq!(
        scene.holds(INVENTORY_HEAD),
        Some(stack(items::CARVED_PUMPKIN, 1))
    );
}

/// `ArmorSlot.mayPlace` → `LivingEntity.isEquippableInSlot`: only an item whose
/// `equippable` slot is the slot's. Dirt (no `equippable`) is refused by the head, a
/// chestplate by the head, a helmet by the chest and the feet; the cursor keeps each.
#[test]
fn an_armour_slot_refuses_what_is_not_worn_there() {
    for (item, slot) in [
        (items::DIRT, HEAD),
        (items::DIAMOND_CHESTPLATE, HEAD),
        (items::IRON_HELMET, CHEST),
        (items::IRON_HELMET, FEET),
        (items::IRON_BOOTS, LEGS),
    ] {
        let mut scene = alone(inventory(&[(0, stack(item, 1))]), survival());
        pick(&mut scene, HOTBAR);
        let output = pick(&mut scene, slot);
        let (_, slots, carried) = the_change(&output);
        assert_eq!(slots, Vec::new(), "{item} on {slot}");
        assert_eq!(carried, None, "{item} on {slot}: the cursor is as it was");
        assert_eq!(
            crafting(&scene).carried,
            Some(stack(item, 1)),
            "{item} on {slot}"
        );
        for at in [
            INVENTORY_HEAD,
            INVENTORY_CHEST,
            INVENTORY_LEGS,
            INVENTORY_FEET,
        ] {
            assert_eq!(scene.holds(at), None, "{item} on {slot}");
        }
    }
}

/// `ArmorSlot.mayPickup` @0-36: a stack with `PREVENT_ARMOR_CHANGE` (Curse of Binding)
/// is not given up to a player who is not creative; a creative player takes it.
#[test]
fn a_helmet_with_curse_of_binding_stays_on_a_survival_player_and_comes_off_a_creative_one() {
    let worn = || inventory(&[(INVENTORY_HEAD, cursed(items::IRON_HELMET))]);
    let mut scene = alone(worn(), survival());
    let output = pick(&mut scene, HEAD);
    let (_, slots, carried) = the_change(&output);
    assert_eq!((slots, carried), (Vec::new(), None));
    assert_eq!(
        scene.holds(INVENTORY_HEAD),
        Some(cursed(items::IRON_HELMET))
    );
    // Nor does a shift-click move it.
    shift(&mut scene, HEAD);
    assert_eq!(
        scene.holds(INVENTORY_HEAD),
        Some(cursed(items::IRON_HELMET))
    );
    assert_eq!(scene.holds(9), None);

    let mut scene = alone(worn(), creative());
    let output = pick(&mut scene, HEAD);
    let (_, slots, carried) = the_change(&output);
    assert_eq!(slots, vec![(5, None)]);
    assert_eq!(carried, Some(Some(cursed(items::IRON_HELMET))));
    assert_eq!(scene.holds(INVENTORY_HEAD), None);
}

/// `quickMoveStack` @135-200: a piece of armour shift-clicked from the hotbar or the main
/// inventory goes to its own slot (8 less the equipment slot's index) if that is empty.
#[test]
fn a_shift_clicked_helmet_goes_to_the_head_when_it_is_empty() {
    for (from, from_slot) in [(0, HOTBAR), (9, MAIN), (20, 20)] {
        let mut scene = alone(
            inventory(&[(from, stack(items::IRON_HELMET, 1))]),
            survival(),
        );
        let output = shift(&mut scene, from_slot);
        let (_, mut slots, _) = the_change(&output);
        slots.sort_by_key(|(slot, _)| *slot);
        assert_eq!(
            slots,
            vec![
                (5, Some(stack(items::IRON_HELMET, 1))),
                (from_slot as u16, None)
            ],
            "from {from_slot}"
        );
        assert_eq!(
            scene.holds(INVENTORY_HEAD),
            Some(stack(items::IRON_HELMET, 1))
        );
    }
}

/// `quickMoveStack` @135-167 then @247-306: with the head taken, a helmet goes as any
/// stack does: from the hotbar into the main inventory's first empty slot (9-35), from
/// the main inventory into the hotbar's (36-44).
#[test]
fn a_shift_clicked_helmet_with_the_head_taken_goes_between_the_hotbar_and_the_main_inventory() {
    let mut scene = alone(
        inventory(&[
            (INVENTORY_HEAD, stack(items::DIAMOND_HELMET, 1)),
            (0, stack(items::IRON_HELMET, 1)),
            (9, stack(items::DIRT, 1)),
        ]),
        survival(),
    );
    shift(&mut scene, HOTBAR);
    assert_eq!(scene.holds(0), None);
    assert_eq!(
        scene.holds(10),
        Some(stack(items::IRON_HELMET, 1)),
        "9 is taken"
    );
    assert_eq!(
        scene.holds(INVENTORY_HEAD),
        Some(stack(items::DIAMOND_HELMET, 1))
    );
    shift(&mut scene, MAIN + 1);
    assert_eq!(scene.holds(10), None);
    assert_eq!(
        scene.holds(0),
        Some(stack(items::IRON_HELMET, 1)),
        "the hotbar's first"
    );
}

/// `quickMoveStack` @203-246: a shield (worn in the off hand) shift-clicked goes to an
/// empty off hand; with the off hand taken it goes into the hotbar.
#[test]
fn a_shift_clicked_shield_goes_to_an_empty_off_hand_else_to_the_hotbar() {
    let mut scene = alone(inventory(&[(9, stack(items::SHIELD, 1))]), survival());
    shift(&mut scene, MAIN);
    assert_eq!(
        scene.holds(INVENTORY_OFF_HAND),
        Some(stack(items::SHIELD, 1))
    );
    assert_eq!(scene.holds(9), None);

    let mut scene = alone(
        inventory(&[
            (9, stack(items::SHIELD, 1)),
            (INVENTORY_OFF_HAND, stack(items::TORCH, 5)),
        ]),
        survival(),
    );
    shift(&mut scene, MAIN);
    assert_eq!(
        scene.holds(INVENTORY_OFF_HAND),
        Some(stack(items::TORCH, 5))
    );
    assert_eq!(scene.holds(0), Some(stack(items::SHIELD, 1)));
}

/// `quickMoveStack` @106-131 and @307-321: what is shift-clicked out of an armour slot or
/// the off hand goes into the first empty slot of 9-44, the main inventory first.
#[test]
fn a_shift_click_out_of_the_armour_or_the_off_hand_goes_to_the_main_inventory_first() {
    let mut scene = alone(
        inventory(&[
            (INVENTORY_CHEST, stack(items::DIAMOND_CHESTPLATE, 1)),
            (INVENTORY_OFF_HAND, stack(items::TORCH, 5)),
            (9, stack(items::DIRT, 1)),
        ]),
        survival(),
    );
    shift(&mut scene, CHEST);
    assert_eq!(scene.holds(INVENTORY_CHEST), None);
    assert_eq!(scene.holds(10), Some(stack(items::DIAMOND_CHESTPLATE, 1)));
    shift(&mut scene, OFF_HAND);
    assert_eq!(scene.holds(INVENTORY_OFF_HAND), None);
    assert_eq!(scene.holds(11), Some(stack(items::TORCH, 5)));
}

/// `quickMoveStack` @78-105 with `moveItemStackTo(9, 45, false)`: a grid slot's stack
/// tops up a stack of its kind in 9-44 first, in order, and only then goes to an empty
/// slot.
#[test]
fn a_shift_click_out_of_the_grid_tops_up_a_stack_of_its_kind_first() {
    let grid = Crafting {
        grid: [Some(stack(items::DIRT, 10)), None, None, None],
        ..Crafting::default()
    };
    let mut scene = scene_of(
        Layout::alone(),
        &World::default(),
        inventory(&[(3, stack(items::DIRT, 60))]),
        BESIDE,
        survival(),
        grid,
    );
    let output = shift(&mut scene, GRID[0]);
    let (_, mut slots, _) = the_change(&output);
    slots.sort_by_key(|(slot, _)| *slot);
    assert_eq!(
        slots,
        vec![
            (1, None),
            (9, Some(stack(items::DIRT, 6))),
            (39, Some(stack(items::DIRT, 64))),
        ]
    );
    assert_eq!(crafting(&scene).grid, [None, None, None, None]);
}

/// `quickMoveStack` @247-276: a main stack goes into the hotbar, topping up a stack of
/// its kind there first.
#[test]
fn a_shift_clicked_main_stack_tops_up_the_hotbar_then_fills_its_first_empty_slot() {
    let mut scene = alone(
        inventory(&[(9, stack(items::DIRT, 10)), (3, stack(items::DIRT, 60))]),
        survival(),
    );
    shift(&mut scene, MAIN);
    assert_eq!(scene.holds(3), Some(stack(items::DIRT, 64)));
    assert_eq!(scene.holds(0), Some(stack(items::DIRT, 6)));
    assert_eq!(scene.holds(9), None);
}

/// The off hand's slot (`InventoryMenu$1`) is a plain `Slot`: it takes any item, a whole
/// stack.
#[test]
fn the_off_hand_takes_any_stack() {
    let mut scene = alone(inventory(&[(0, stack(items::DIRT, 64))]), survival());
    pick(&mut scene, HOTBAR);
    let output = pick(&mut scene, OFF_HAND);
    let (_, slots, carried) = the_change(&output);
    assert_eq!(slots, vec![(45, Some(stack(items::DIRT, 64)))]);
    assert_eq!(carried, Some(None));
    assert_eq!(
        scene.holds(INVENTORY_OFF_HAND),
        Some(stack(items::DIRT, 64))
    );
}

// ------------------------------------------------------------------------------------
// Item 4: putting armour on by the screen.

/// `ArmorSlot.setByPlayer` → `LivingEntity.onEquipItem` @34-107: the new item's equip
/// sound at the player's position, played with no player to leave out
/// (`playSeededSound(null, …)`), so the player hears it too, from the players' source
/// (`Player.getSoundSource`); and the others are shown the helmet on the head.
#[test]
fn a_helmet_put_on_by_the_screen_plays_its_equip_sound_to_everyone_and_shows_on_the_head() {
    let mut scene = alone(inventory(&[(0, stack(items::IRON_HELMET, 1))]), survival());
    let output = shift(&mut scene, HOTBAR);
    assert_eq!(
        equip_sounds(&output),
        vec![(
            sounds::ITEM_ARMOR_EQUIP_IRON,
            SOUND_SOURCE_PLAYERS,
            BESIDE,
            None
        )]
    );
    let me = scene.me();
    let shown: Vec<Vec<(u8, Option<ItemStack>)>> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EquipmentChanged { entity, slots, .. } if *entity == me => {
                Some(slots.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        shown
            .iter()
            .any(|slots| slots.contains(&(EQUIPMENT_HEAD, Some(stack(items::IRON_HELMET, 1))))),
        "{shown:?}"
    );
}

/// The same by a left click, and for the off hand (`InventoryMenu$1.setByPlayer` →
/// `onEquipItem(OFFHAND, …)`): a shield, `equippableUnswappable(OFFHAND)` with the
/// default sound, plays the generic equip sound.
#[test]
fn a_shield_put_in_the_off_hand_by_a_click_plays_the_generic_equip_sound() {
    let mut scene = alone(inventory(&[(0, stack(items::SHIELD, 1))]), survival());
    pick(&mut scene, HOTBAR);
    let output = pick(&mut scene, OFF_HAND);
    assert_eq!(
        equip_sounds(&output),
        vec![(
            sounds::ITEM_ARMOR_EQUIP_GENERIC,
            SOUND_SOURCE_PLAYERS,
            BESIDE,
            None
        )]
    );
}

/// `onEquipItem` @34-64: the sound is the new item's, and an empty slot has none, so
/// taking a helmet off plays nothing; nor does dirt put in the off hand (no
/// `equippable`).
#[test]
fn taking_a_helmet_off_or_putting_dirt_in_the_off_hand_plays_no_sound() {
    let mut scene = alone(
        inventory(&[
            (INVENTORY_HEAD, stack(items::IRON_HELMET, 1)),
            (0, stack(items::DIRT, 1)),
        ]),
        survival(),
    );
    let output = pick(&mut scene, HEAD);
    assert_eq!(equip_sounds(&output), []);
    assert_eq!(crafting(&scene).carried, Some(stack(items::IRON_HELMET, 1)));
    let output = shift(&mut scene, MAIN);
    assert_eq!(equip_sounds(&output), []);
    pick(&mut scene, MAIN + 1);
    pick(&mut scene, HOTBAR);
    let output = pick(&mut scene, OFF_HAND);
    assert_eq!(scene.holds(INVENTORY_OFF_HAND), Some(stack(items::DIRT, 1)));
    assert_eq!(equip_sounds(&output), []);
}

/// `onEquipItem` @18-33: a helmet swapped for one just like it
/// (`isSameItemSameComponents`) plays nothing.
#[test]
fn a_helmet_swapped_for_one_just_like_it_plays_no_sound() {
    let mut scene = alone(
        inventory(&[
            (INVENTORY_HEAD, stack(items::IRON_HELMET, 1)),
            (0, stack(items::IRON_HELMET, 1)),
        ]),
        survival(),
    );
    pick(&mut scene, HOTBAR);
    let output = pick(&mut scene, HEAD);
    assert_eq!(
        scene.holds(INVENTORY_HEAD),
        Some(stack(items::IRON_HELMET, 1))
    );
    assert_eq!(crafting(&scene).carried, Some(stack(items::IRON_HELMET, 1)));
    assert_eq!(equip_sounds(&output), []);
}

/// The same swap with a different helmet plays the new one's sound.
#[test]
fn a_helmet_swapped_for_another_kind_plays_the_new_ones_sound() {
    let mut scene = alone(
        inventory(&[
            (INVENTORY_HEAD, stack(items::IRON_HELMET, 1)),
            (0, stack(items::DIAMOND_HELMET, 1)),
        ]),
        survival(),
    );
    pick(&mut scene, HOTBAR);
    let output = pick(&mut scene, HEAD);
    assert_eq!(
        scene.holds(INVENTORY_HEAD),
        Some(stack(items::DIAMOND_HELMET, 1))
    );
    assert_eq!(crafting(&scene).carried, Some(stack(items::IRON_HELMET, 1)));
    assert_eq!(
        equip_sounds(&output)
            .into_iter()
            .map(|(sound, ..)| sound)
            .collect::<Vec<_>>(),
        vec![sounds::ITEM_ARMOR_EQUIP_DIAMOND]
    );
}

// ------------------------------------------------------------------------------------
// Item 5: the close of window 0.

/// The grid and cursor of the tests of the close: dirt carried, cobblestone in the
/// grid's first slot and gravel in its last.
fn grid_and_cursor() -> Crafting {
    Crafting {
        grid: [
            Some(stack(items::COBBLESTONE, 3)),
            None,
            None,
            Some(stack(items::GRAVEL, 7)),
        ],
        carried: Some(stack(items::DIRT, 5)),
        ..Crafting::default()
    }
}

/// `handleContainerClose` → `doCloseContainer` → `InventoryMenu.removed`: first
/// `AbstractContainerMenu.removed` puts the cursor back, then the result is cleared and
/// `clearContainer` puts the grid's slots back in order, each by
/// `placeItemBackInInventory` (the first empty slot of 0-35 for a stack that tops up
/// nothing). The client is told the grid and the cursor emptied, and the slots filled.
#[test]
fn a_close_with_no_menu_open_puts_the_cursor_then_the_grid_in_order_back_into_the_inventory() {
    let mut scene = scene_of(
        Layout::alone(),
        &World::default(),
        inventory(&[(0, stack(items::STONE, 1))]),
        BESIDE,
        survival(),
        grid_and_cursor(),
    );
    let output = scene.close(0);
    assert_eq!(crafting(&scene), Crafting::default());
    assert_eq!(
        scene.holds(1),
        Some(stack(items::DIRT, 5)),
        "the cursor first"
    );
    assert_eq!(scene.holds(2), Some(stack(items::COBBLESTONE, 3)));
    assert_eq!(scene.holds(3), Some(stack(items::GRAVEL, 7)));
    let (_, slots, carried) = the_change(&output);
    assert_eq!(carried, Some(None), "the cursor emptied");
    for (slot, stack) in [
        (1, None),
        (4, None),
        (37, Some(stack(items::DIRT, 5))),
        (38, Some(stack(items::COBBLESTONE, 3))),
        (39, Some(stack(items::GRAVEL, 7))),
    ] {
        assert!(slots.contains(&(slot, stack.clone())), "{slot}: {slots:?}");
    }
    assert_eq!(lying_count(&scene.all(), items::DIRT), 0);
}

/// `placeItemBackInInventory` tops up the selected slot first
/// (`getSlotWithRemainingSpace`): the cursor's dirt goes onto the selected stack of dirt
/// before the grid's dirt, which then has to go to an empty slot.
#[test]
fn a_close_tops_up_the_selected_stack_with_the_cursor_before_the_grid() {
    let grid = Crafting {
        grid: [Some(stack(items::DIRT, 3)), None, None, None],
        carried: Some(stack(items::DIRT, 2)),
        ..Crafting::default()
    };
    let mut scene = scene_of(
        Layout::alone(),
        &World::default(),
        inventory(&[(0, stack(items::DIRT, 62))]),
        BESIDE,
        survival(),
        grid,
    );
    scene.close(0);
    assert_eq!(scene.holds(0), Some(stack(items::DIRT, 64)));
    assert_eq!(scene.holds(1), Some(stack(items::DIRT, 3)));
}

/// `placeItemBackInInventory` drops what has no room: with every slot of 0-35 full of
/// stone, the cursor and the grid lie where the player stands.
#[test]
fn a_close_with_a_full_inventory_drops_the_cursor_and_the_grid() {
    let full: Vec<(usize, ItemStack)> = (0..36)
        .map(|slot| (slot, stack(items::STONE, 64)))
        .collect();
    let mut scene = scene_of(
        Layout::alone(),
        &World::default(),
        inventory(&full),
        BESIDE,
        survival(),
        grid_and_cursor(),
    );
    scene.close(0);
    let mut seen = Vec::new();
    scene.idle_until(5, &mut seen, |scene, _| {
        lying_count(&scene.all(), items::GRAVEL) == 7
    });
    let all = scene.all();
    assert_eq!(lying_count(&all, items::DIRT), 5);
    assert_eq!(lying_count(&all, items::COBBLESTONE), 3);
    assert_eq!(lying_count(&all, items::GRAVEL), 7);
    assert_eq!(crafting(&scene), Crafting::default());
}

// ------------------------------------------------------------------------------------
// Item 6: a chest over window 0.

/// While a chest is open the player's screen of their own keeps its grid and cursor; a
/// close names the chest's menu and closes it, and leaves the grid as it was.
#[test]
fn a_chest_opened_and_closed_over_window_0_leaves_its_grid_and_cursor() {
    let mut scene = scene_of(
        Layout::alone(),
        &chest(),
        inventory(&[(0, stack(items::STONE, 64))]),
        BESIDE,
        survival(),
        grid_and_cursor(),
    );
    let (serial, _) = scene.open(BOX);
    scene.menu_click(
        serial,
        i16::try_from(MenuKind::Rows3.container_slots() + 27).expect("fits"),
        0,
        ClickKind::QuickMove,
    );
    scene.close(serial);
    assert!(
        scene
            .region
            .player_state(player(1))
            .expect("there")
            .menu
            .is_none()
    );
    assert_eq!(crafting(&scene), grid_and_cursor());
}

/// The region reports window 0 whatever is open: a stack moved out of the hotbar into
/// an open chest is reported as window 0's slot 36 emptied, in the tick of the click.
#[test]
fn window_0_is_reported_while_a_chest_is_open() {
    let mut scene = scene_of(
        Layout::alone(),
        &chest(),
        inventory(&[(0, stack(items::STONE, 64))]),
        BESIDE,
        survival(),
        Crafting::default(),
    );
    let (serial, _) = scene.open(BOX);
    let output = scene.menu_click(
        serial,
        i16::try_from(MenuKind::Rows3.container_slots() + 27).expect("fits"),
        0,
        ClickKind::QuickMove,
    );
    assert_eq!(scene.holds(0), None);
    let reported: Vec<(u16, Option<ItemStack>)> = window_0(&output, player(1))
        .into_iter()
        .flat_map(|event| match event {
            PlayerEvent::MenuChanged { slots, .. } => slots,
            PlayerEvent::MenuOpened { slots, .. } => slots
                .into_iter()
                .enumerate()
                .map(|(at, stack)| (u16::try_from(at).expect("few"), stack))
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    assert!(reported.contains(&(36, None)), "{:?}", output.player_events);
}

// ------------------------------------------------------------------------------------
// Item 7: a join and a leave.

/// A grid and cursor kept in the player's place (Clustine's named difference: the game
/// drops them at the feet when a player leaves) come back with them and are reported
/// after the join.
#[test]
fn a_grid_and_cursor_kept_in_the_place_are_reported_after_the_join() {
    let region = region_of(&Layout::alone(), &World::default(), Inventory::default());
    let mut scene = Scene {
        region,
        layout: Layout::alone(),
        inventory: Inventory::default(),
        actors: Vec::new(),
    };
    let (_, entered) = scene.join_seen(1, BESIDE, |place| {
        place.mode = survival();
        place.crafting = grid_and_cursor();
    });
    let mut seen = vec![entered];
    seen.push(scene.idle());
    seen.push(scene.idle());
    assert_eq!(crafting(&scene), grid_and_cursor());
    // What was reported of window 0 since the join, the later over the earlier.
    let mut slots: std::collections::BTreeMap<u16, Option<ItemStack>> = Default::default();
    let mut carried = None;
    for output in &seen {
        for event in window_0(output, player(1)) {
            match event {
                PlayerEvent::MenuChanged {
                    slots: changed,
                    carried: cursor,
                    ..
                } => {
                    slots.extend(changed);
                    if let Some(cursor) = cursor {
                        carried = Some(cursor);
                    }
                }
                PlayerEvent::MenuOpened {
                    slots: whole,
                    carried: cursor,
                    ..
                } => {
                    slots.extend(
                        whole
                            .into_iter()
                            .enumerate()
                            .map(|(at, stack)| (u16::try_from(at).expect("few"), stack)),
                    );
                    carried = Some(cursor);
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        slots.get(&1),
        Some(&Some(stack(items::COBBLESTONE, 3))),
        "{slots:?}"
    );
    assert_eq!(
        slots.get(&4),
        Some(&Some(stack(items::GRAVEL, 7))),
        "{slots:?}"
    );
    assert_eq!(carried, Some(Some(stack(items::DIRT, 5))));
}

/// The same difference at a leave: the grid and the cursor are in the place the store
/// keeps, as the last note of the stay before it is gone says, and nothing is dropped.
/// A click that swaps the cursor's dirt with the grid's gravel makes a note of the
/// place.
#[test]
fn a_player_who_leaves_keeps_the_grid_and_cursor_in_their_place() {
    let mut scene = scene_of(
        Layout::alone(),
        &World::default(),
        Inventory::default(),
        BESIDE,
        survival(),
        grid_and_cursor(),
    );
    let swapped = Crafting {
        grid: [
            Some(stack(items::COBBLESTONE, 3)),
            None,
            None,
            Some(stack(items::DIRT, 5)),
        ],
        carried: Some(stack(items::GRAVEL, 7)),
        ..Crafting::default()
    };
    let mut outputs = vec![pick(&mut scene, GRID[3])];
    assert_eq!(crafting(&scene), swapped);
    let me = scene.me();
    outputs.push(scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(EDGE, player(1), Some(me), None)],
        ..TickInputs::default()
    }));
    assert!(scene.region.player(player(1)).is_none(), "the player left");
    let notes: Vec<StayNote> = outputs
        .iter()
        .flat_map(|output| output.stays.iter().cloned())
        .collect();
    let places: Vec<Crafting> = notes
        .iter()
        .filter_map(|note| match note {
            StayNote::Has {
                player: who, place, ..
            } if *who == player(1) => Some(place.crafting.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(places.last(), Some(&swapped), "{notes:?}");
    assert!(
        matches!(notes.last(), Some(StayNote::Gone { player: who, .. }) if *who == player(1)),
        "{notes:?}"
    );
    let mut seen = Vec::new();
    scene.idle_until(3, &mut seen, |_, _| false);
    let all = scene.all();
    for item in [items::DIRT, items::COBBLESTONE, items::GRAVEL] {
        assert_eq!(lying_count(&all, item), 0, "{item}");
    }
}

// ------------------------------------------------------------------------------------
// Item 8: a hand-over.

const BY_THE_LINE: Vec3 = Vec3::new(14.5, -60.0, 2.5);
const OVER_THE_LINE: Vec3 = Vec3::new(16.5, -60.0, 2.5);

/// The game never closes the inventory menu: a stack carried on window 0's cursor and a
/// grid go with the player across a line, still carried and still in the grid, and
/// nothing is put back into the inventory or dropped; in the next region a click puts
/// the carried stack down.
#[test]
fn a_stack_carried_across_a_line_is_still_carried_and_is_put_down_in_the_next_region() {
    let grid = Crafting {
        grid: [None, Some(stack(items::COBBLESTONE, 2)), None, None],
        ..Crafting::default()
    };
    let mut west = scene_of(
        Layout::side(true),
        &World::default(),
        inventory(&[(0, stack(items::DIRT, 64))]),
        BY_THE_LINE,
        survival(),
        grid,
    );
    pick(&mut west, HOTBAR);
    assert_eq!(crafting(&west).carried, Some(stack(items::DIRT, 64)));
    let output = west.walk_to(OVER_THE_LINE);
    let gone = departures(&output);
    assert_eq!(gone.len(), 1, "player 1 is let go: {:?}", output.durable);
    let (_, transfer, _) = &gone[0];
    assert_eq!(transfer.crafting.carried, Some(stack(items::DIRT, 64)));
    assert_eq!(
        transfer.crafting.grid[1],
        Some(stack(items::COBBLESTONE, 2))
    );
    assert!(
        transfer.inventory.0.iter().all(Option::is_none),
        "nothing put back: {:?}",
        transfer.inventory
    );
    assert_eq!(lying_count(&west.all(), items::DIRT), 0);

    let mut east = region_of(
        &Layout::side(false),
        &World::default(),
        Inventory::default(),
    );
    east.tick(&TickInputs {
        player_changes: vec![PlayerChange::Arrive(EDGE, player(1), transfer.clone())],
        ..TickInputs::default()
    });
    let state = east.player_state(player(1)).expect("arrived");
    assert_eq!(state.crafting.carried, Some(stack(items::DIRT, 64)));
    assert_eq!(state.crafting.grid[1], Some(stack(items::COBBLESTONE, 2)));
    let entity = east.player(player(1)).expect("there").0;
    let number = transfer.last_input + 1;
    let output = east.tick(&TickInputs {
        inputs: vec![(
            EDGE,
            player(1),
            entity,
            number,
            PlayerInput::ContainerClick {
                serial: 0,
                slot: MAIN,
                button: 0,
                kind: ClickKind::Pickup,
            },
        )],
        ..TickInputs::default()
    });
    let state = east.player_state(player(1)).expect("there");
    assert_eq!(state.inventory.0[9], Some(stack(items::DIRT, 64)));
    assert_eq!(state.crafting.carried, None);
    let reports = window_0(&output, player(1));
    assert!(
        reports.iter().any(|event| matches!(
            event,
            PlayerEvent::MenuChanged { applied, slots, carried, .. }
                if *applied == number
                    && slots.contains(&(9, Some(stack(items::DIRT, 64))))
                    && *carried == Some(None)
        ) || matches!(
            event,
            PlayerEvent::MenuOpened { applied, slots, carried, .. }
                if *applied == number
                    && slots[9] == Some(stack(items::DIRT, 64))
                    && carried.is_none()
        )),
        "{reports:?}"
    );
}

// ------------------------------------------------------------------------------------
// Item 9: a change of level and a death.

/// The homes of the dimensions, which a portal sends a player to.
const END_HOME: RegionId = RegionId(5);

/// `Player.remove` with `CHANGED_DIMENSION` → `InventoryMenu.removed`: before a change
/// of level by an End portal the cursor and then the grid go back into the inventory, in
/// that order, and the transfer has neither.
#[test]
fn before_a_change_of_level_the_cursor_and_grid_go_back_into_the_inventory() {
    let portal = BlockPos::new(3, -60, 0);
    let world = World::of(&[(portal, blocks::END_PORTAL)]);
    let layout = Layout::alone();
    let mut config = config(Inventory::default());
    config.homes = vec![
        (Dimension::Overworld, RegionId(0)),
        (Dimension::Nether, RegionId(1)),
        (Dimension::End, END_HOME),
    ];
    let region = region_configured(&layout, &world, config, None);
    let mut scene = Scene {
        region,
        layout,
        inventory: Inventory::default(),
        actors: Vec::new(),
    };
    scene.join_with(1, BESIDE, |place| {
        place.mode = survival();
        place.crafting = grid_and_cursor();
    });
    scene.input(PlayerInput::Loaded);
    let mut outputs = vec![scene.walk_to(Vec3::new(3.5, -60.0, 0.5))];
    let mut seen = Vec::new();
    scene.idle_until(5, &mut seen, |_, seen| {
        seen.iter().any(|output| !departures(output).is_empty())
    });
    outputs.extend(seen);
    let gone: Vec<_> = outputs.iter().flat_map(departures).collect();
    assert_eq!(gone.len(), 1, "the portal let player 1 go");
    let (_, transfer, to) = &gone[0];
    assert_eq!(*to, END_HOME);
    assert_eq!(transfer.dimension, Dimension::End);
    assert_eq!(transfer.crafting, Crafting::default());
    assert_eq!(
        transfer.inventory.0[0],
        Some(stack(items::DIRT, 5)),
        "the cursor first"
    );
    assert_eq!(transfer.inventory.0[1], Some(stack(items::COBBLESTONE, 3)));
    assert_eq!(transfer.inventory.0[2], Some(stack(items::GRAVEL, 7)));
}

/// `ServerPlayer.die` → `dropAllDeathLoot`: at a death the grid and the cursor go with
/// the rest of the inventory (in the void, where the only death of a player is yet,
/// nowhere).
#[test]
fn at_a_death_the_grid_and_cursor_go_with_the_inventory() {
    let mut scene = scene_of(
        Layout::alone(),
        &World::default(),
        inventory(&[(0, stack(items::STONE, 64))]),
        BESIDE,
        survival(),
        grid_and_cursor(),
    );
    scene.input(PlayerInput::Loaded);
    scene.walk_to(Vec3::new(0.5, -140.0, 0.5));
    let mut seen = Vec::new();
    let died = scene.idle_until(200, &mut seen, |scene, _| {
        scene
            .region
            .player_state(player(1))
            .is_none_or(|state| state.dead)
    });
    assert!(died, "the void kills player 1");
    let state = scene
        .region
        .player_state(player(1))
        .expect("their stay is kept");
    assert_eq!(state.crafting, Crafting::default());
    assert!(state.inventory.0.iter().all(Option::is_none));
    // A click of window 0 while dead is answered with it whole, empty.
    let output = pick(&mut scene, HOTBAR);
    let (_, slots, carried) = the_whole(&output);
    assert!(slots.iter().all(Option::is_none), "{slots:?}");
    assert_eq!(carried, None);
}
