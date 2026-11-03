//! The click rules of N6's menus: the game's `AbstractContainerMenu.clicked` and
//! `doClick`, the `quickMoveStack` of each menu, and the rules of their slots, run on
//! what the region holds (`docs/groundwork/n6-plan.md`, decision 2; the brief's sections
//! 3.11 to 3.18, read again against the class dumps).
//!
//! The region keeps the menu (what it shows, the carried stack, a drag under way) and
//! runs every click here; the edge keeps the client's hashes and state ids and compares
//! (ADR-0005). Nothing here draws a random number or reads anything beyond what it is
//! handed: the stacks a click throws out of the window are returned for the region to
//! drop, in the order the game throws them.
//!
//! The two crafting grids (the inventory screen's 2×2 and the crafting table's 3×3) craft
//! here too (V3c): their result slot is worked out from the grid by `crate::crafting`, and
//! a take of it consumes the grid and leaves the recipe's remainders (`ResultSlot.onTake`).
//!
//! Every N6 container keeps the default 99 a slot as its most
//! (`Container.getMaxStackSize`), so a slot holds a full stack of its item at most.

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, ITEMS, ItemComponents, items};
use clustine_world::{ARMOUR_START, HOTBAR_SLOTS, MAIN_START, OFF_HAND};

use crate::api::{ClickKind, Inventory, ItemStack, MenuKind};
use crate::crafting;
use crate::living::slots;
use crate::physics::item::max_stack_size;
use crate::state::{Drag, DragKind, Menu};

/// The slot index of a click outside the window (`SLOT_CLICKED_OUTSIDE`).
const OUTSIDE: i32 = -999;
/// The most of anything a container's slot holds (`Container.getMaxStackSize`), the
/// player's inventory's too; none of N6's containers changes it.
const CONTAINER_MAX: i32 = 99;
/// The player's main inventory and hotbar, which `Inventory.getFreeSlot` and the
/// inventory's own search go through (`Inventory.items`, 36 slots).
const ITEM_SLOTS: usize = 36;

/// The slots a menu shows: the container's (for a double chest the right half's 27
/// then the left half's, for an ender chest the player's own 27), then the player's
/// main inventory (inventory slots 9 to 35) and hotbar (0 to 8), as every menu of N6
/// lays them out.
pub(crate) struct MenuSlots<'a> {
    pub(crate) kind: MenuKind,
    pub(crate) container: &'a mut [Option<ItemStack>],
    pub(crate) inventory: &'a mut Inventory,
    /// The hotbar slot the player holds (`Inventory.selected`, 0 to 8). `Inventory.add`
    /// fills it first, which a swap whose stack does not fit back reaches.
    pub(crate) selected: usize,
    /// What a horse's menu's saddle and body slots ask of the horse, for
    /// [`MenuKind::Mount`]: none for any other menu.
    pub(crate) mount: Option<MountFacts>,
    /// What a merchant's window asks of its merchant, for [`MenuKind::Merchant`]: none
    /// for any other menu (`region/trading.rs`).
    pub(crate) merchant: Option<&'a mut dyn MerchantSide>,
}

/// What a merchant's window asks of the merchant behind it (`MerchantResultSlot`,
/// `docs/groundwork/n10-dumps.md`, part B, section 6): whether its result may be taken,
/// and the trade that a take makes, which changes the payments and the result. The
/// container handed is the window's three slots, the payments then the result.
pub(crate) trait MerchantSide {
    /// `MerchantResultSlot.mayPickup`: an active offer that the payments satisfy, either
    /// way round.
    fn may_pickup(&self, container: &[Option<ItemStack>]) -> bool;
    /// `MerchantResultSlot.onTake`: the active offer takes its price from the payments,
    /// the merchant is told of the trade, and the result is worked out again.
    fn on_take(&mut self, container: &mut [Option<ItemStack>]);
}

/// The slot of a merchant's window that shows what the trade gives.
const MERCHANT_RESULT: usize = 2;

/// The player's inventory screen (`InventoryMenu.<init>` @18-137, v3): the result (0),
/// the 2×2 grid (1-4, `addCraftingGridSlots`), the armour from the head down (5-8, an
/// `ArmorSlot` each over inventory slot `39 - i`, @37-105), the main inventory (9-35)
/// and the hotbar (36-44, `addStandardInventorySlots`), then the off hand (45,
/// `InventoryMenu$1` over inventory slot 40).
const INVENTORY_RESULT: usize = 0;
const INVENTORY_ARMOUR: usize = 5;
const INVENTORY_MAIN: usize = 9;
const INVENTORY_HOTBAR: usize = 36;
const INVENTORY_OFF_HAND: usize = 45;
/// One past the inventory screen's last slot.
const INVENTORY_END: usize = 46;

/// The crafting table's menu (`CraftingMenu`, v3, whose bytecode inlines its constants:
/// `RESULT_SLOT` 0, the grid from `CRAFT_SLOT_START` 1 to `CRAFT_SLOT_END` 10,
/// `INV_SLOT_START` 10, `USE_ROW_SLOT_START` 37, `USE_ROW_SLOT_END` 46, quickMoveStack
/// @58-162): the result, the 3×3 grid row by row, then the main inventory and the hotbar
/// as every menu lays them out.
const CRAFTING_RESULT: usize = 0;
const CRAFTING_MAIN: usize = 10;
const CRAFTING_HOTBAR: usize = 37;
const CRAFTING_END: usize = 46;

/// What the two equipment slots of a horse's menu (`HorseInventoryMenu`'s `ArmorSlot`s)
/// ask of the horse: its type, which the item's allowed entities name, and whether it
/// may use each slot now (`canUseSlot`: a saddle only grown and tamed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MountFacts {
    pub(crate) kind: u16,
    pub(crate) saddle: bool,
    pub(crate) body: bool,
}

/// The menu slot of a horse's saddle and of its body.
const MOUNT_SADDLE: usize = 0;
const MOUNT_BODY: usize = 1;

/// What a click leaves beyond the slots, the carried stack and the drag.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Clicked {
    /// Stacks thrown out of the window, in the order the game throws them (a click
    /// outside, a `THROW`), for the region to drop as `Player.drop(stack, true)`.
    pub(crate) thrown: Vec<ItemStack>,
    /// Stacks a craft drops at the player's feet, in the order the game drops them, for
    /// the region to drop as `Player.drop(stack, false, PREDICTED)` (no throw upward): a
    /// remainder that neither goes back into its grid slot nor into the inventory
    /// (`ResultSlot.onTake` @245-258, v3), and what a shift-click on the result could not
    /// move (`InventoryMenu.quickMoveStack` and `CraftingMenu.quickMoveStack` @220-234).
    ///
    /// Named difference: the game interleaves the two kinds as it goes (a `THROW` with
    /// button 1 drops a craft's remainders, then throws its result, then crafts again);
    /// here each list keeps its own order and the region spawns the two lists apart, so
    /// only the order of the entities' ids can differ.
    pub(crate) dropped: Vec<ItemStack>,
    /// Whether the click was refused as the game refuses it, changing nothing.
    pub(crate) refused: bool,
    /// The inventory screen's equipment slots (5-8 the armour, 45 the off hand) that the
    /// click set by the player to another item or other components than they held, in
    /// the order it set them, once for each time: each is the game's
    /// `setByPlayer(new, old)` → `LivingEntity.onEquipItem(slot, old, new)` (`ArmorSlot`
    /// @0-19 and `InventoryMenu$1` @0-18, v3), which does nothing where
    /// `isSameItemSameComponents(old, new)` (n5 LivingEntity @18-23). A slot emptied by a
    /// take is one (the `UNEQUIP` game event, no sound). The region plays the sound (the
    /// new stack's `equippable` sound where its slot is this one, @34-107) and emits the
    /// event (@110-133); whether the player is a spectator, silent or in its first tick
    /// (@0-33) is the region's to ask. The off hand set by a `SWAP` with button 40 is
    /// written by `Inventory.setItem` (AbstractContainerMenu @1167, @1258, @1361), not by
    /// its slot, and is not reported, as the game calls no `onEquipItem` for it (n6
    /// Inventory.setItem @44: `EntityEquipment.set`). Empty for any other menu.
    pub(crate) equipped: Vec<usize>,
}

/// `AbstractContainerMenu.clicked` of the player's click, for a player who is creative
/// if `creative`.
///
/// Refused, changing nothing: a slot past the menu's last, as `handleContainerClick`'s
/// `isValidSlotIndex` refuses it; and the clicks on which the game's `doClick` throws
/// (finding 25): a `SWAP` of a valid button, or a drag's slot, at a negative slot, with
/// nothing changed before the throw. Any other negative slot goes through as the game
/// takes it, which does nothing but end a drag under way or run a drag's start or end
/// (which read no slot). A click the game takes and that changes nothing (a clone by a
/// player who is not creative, a swap of a button that names no slot) is not refused.
pub(crate) fn click(
    slots: &mut MenuSlots<'_>,
    menu: &mut Menu,
    slot: i16,
    button: i8,
    kind: ClickKind,
    creative: bool,
) -> Clicked {
    let refused = Clicked {
        thrown: Vec::new(),
        dropped: Vec::new(),
        refused: true,
        equipped: Vec::new(),
    };
    // The region hands the menu's container with the menu; one of another size is not
    // this menu's, and no rule below may index past it.
    if slots.kind != menu.kind || slots.container.len() != slots.kind.container_slots() {
        return refused;
    }
    let slot = i32::from(slot);
    if usize::try_from(slot).is_ok_and(|slot| slot >= slots.kind.slots()) {
        return refused;
    }
    let mut clicker = Clicker {
        slots,
        menu,
        creative,
        thrown: Vec::new(),
        dropped: Vec::new(),
        equipped: Vec::new(),
        grid_seen: None,
    };
    clicker.grid_seen = clicker.grid().map(<[Option<ItemStack>]>::to_vec);
    match clicker.do_click(slot, i32::from(button), kind) {
        Ok(()) => {
            // `slotChangedCraftingGrid` once at the end of a click that changed the grid
            // by any way in (a pickup, a swap, a drag, a quick move, a throw, a gathering,
            // a clone). The game works the result out again at every write to a grid
            // slot (`TransientCraftingContainer.setItem` @10-15 and `removeItem` @17-22,
            // v3); the result is a function of the grid alone, and nothing inside the
            // click reads it but a take, which works it out itself, so once at the end
            // leaves the same result. A stack grown in place calls no `slotsChanged` in
            // the game (finding 2 of the V3c review); the counts never change a match.
            if clicker
                .grid()
                .is_some_and(|grid| clicker.grid_seen.as_deref() != Some(grid))
            {
                clicker.recompute_result();
            }
            Clicked {
                thrown: clicker.thrown,
                dropped: clicker.dropped,
                refused: false,
                equipped: clicker.equipped,
            }
        }
        Err(Malformed) => refused,
    }
}

/// The stack in the menu's slot `index`, if the menu has such a slot.
pub(crate) fn menu_slot(slots: &MenuSlots<'_>, index: usize) -> Option<Option<ItemStack>> {
    let containers = slots.kind.container_slots();
    if index < containers {
        return slots
            .container
            .get(index)
            .map(|stack| present(stack.clone()));
    }
    let slot = inventory_slot_of(slots.kind, index)?;
    slots.inventory.get(slot).map(present)
}

/// The inventory slot shown in the menu's slot `index`, if it shows one (the edge maps
/// a change of the inventory onto the menu with it).
pub(crate) fn inventory_slot_of(kind: MenuKind, index: usize) -> Option<usize> {
    if kind == MenuKind::Inventory {
        return match index {
            INVENTORY_ARMOUR..INVENTORY_MAIN => Some(ARMOUR_START + INVENTORY_MAIN - 1 - index),
            INVENTORY_MAIN..INVENTORY_HOTBAR => Some(index),
            INVENTORY_HOTBAR..INVENTORY_OFF_HAND => Some(index - INVENTORY_HOTBAR),
            INVENTORY_OFF_HAND => Some(OFF_HAND),
            _ => None,
        };
    }
    let from = index.checked_sub(kind.container_slots())?;
    // `addStandardInventorySlots`: the main inventory's 27 first, then the hotbar.
    let main = ITEM_SLOTS - HOTBAR_SLOTS;
    if from < main {
        Some(MAIN_START + from)
    } else if from < ITEM_SLOTS {
        Some(from - main)
    } else {
        None
    }
}

/// The menu slot that shows the inventory slot `slot`, if one does: 0 to 35 in every
/// menu, and in the inventory screen the armour (36 to 39) and the off hand (40) too.
pub(crate) fn menu_slot_of(kind: MenuKind, slot: usize) -> Option<usize> {
    if kind == MenuKind::Inventory {
        return match slot {
            0..HOTBAR_SLOTS => Some(INVENTORY_HOTBAR + slot),
            MAIN_START..ITEM_SLOTS => Some(slot),
            ARMOUR_START..OFF_HAND => Some(INVENTORY_MAIN - 1 - (slot - ARMOUR_START)),
            OFF_HAND => Some(INVENTORY_OFF_HAND),
            _ => None,
        };
    }
    let main = ITEM_SLOTS - HOTBAR_SLOTS;
    let from = if slot < HOTBAR_SLOTS {
        main + slot
    } else if slot < ITEM_SLOTS {
        slot - MAIN_START
    } else {
        return None;
    };
    Some(kind.container_slots() + from)
}

/// `closeContainer`'s putting back of the carried stack for a player who stays
/// (`placeItemBackInInventory`): into the inventory as the game puts it, what does not
/// fit returned for the region to drop. The carried stack is then empty.
///
/// `selected` is the hotbar slot the player holds, which the game fills first
/// (`getSlotWithRemainingSpace`, Inventory.txt 494–538), then the off hand if it holds
/// the same item and has room, then slots 0 to 35 that have room, and only then the
/// first empty one of 0 to 35 (`getFreeSlot`, 124–146): an empty off hand is never
/// chosen. What does not fit is dropped whole, as one stack (724–784).
pub(crate) fn put_back(
    inventory: &mut Inventory,
    selected: usize,
    menu: &mut Menu,
) -> Option<ItemStack> {
    let mut stack = present(menu.carried.take())?;
    while stack.count > 0 {
        let Some(slot) =
            slot_with_room(inventory, selected, &stack).or_else(|| free_slot(inventory))
        else {
            return Some(stack);
        };
        let here = present(inventory.get(slot).flatten()).map_or(0, |here| here.count);
        let room = max_stack_size(&stack) - here;
        let mut part = split(&mut stack, room);
        // The part is what the slot has room for, so `add` takes it whole; whether the
        // player is creative matters only to what does not fit.
        add(inventory, selected, Some(slot), &mut part, false);
    }
    None
}

/// A click the game's `doClick` throws on (finding 25), refused here instead.
struct Malformed;

/// One click's run over the menu, with what it throws out so far.
struct Clicker<'s, 'a> {
    slots: &'s mut MenuSlots<'a>,
    menu: &'s mut Menu,
    creative: bool,
    thrown: Vec<ItemStack>,
    /// What the click's crafts drop so far ([`Clicked::dropped`]).
    dropped: Vec<ItemStack>,
    /// The equipment slots set by the player so far ([`Clicked::equipped`]).
    equipped: Vec<usize>,
    /// For a menu with a crafting grid, the grid as it was when its result was last
    /// worked out (or when the click began), so that the end of the click works it out
    /// again only where the grid changed since.
    grid_seen: Option<Vec<Option<ItemStack>>>,
}

impl Clicker<'_, '_> {
    /// `doClick` (AbstractContainerMenu.txt 912–1877 of the method's code), in its
    /// order: a drag's step, else the end of a drag under way, else the click's kind.
    fn do_click(&mut self, slot: i32, button: i32, kind: ClickKind) -> Result<(), Malformed> {
        if kind == ClickKind::QuickCraft {
            return self.quick_craft(slot, button);
        }
        // Any other click while a drag is under way only ends the drag (547–558).
        if self.menu.drag.is_some() {
            self.menu.drag = None;
            return Ok(());
        }
        match kind {
            ClickKind::Pickup | ClickKind::QuickMove if button == 0 || button == 1 => {
                let primary = button == 0;
                if slot == OUTSIDE {
                    self.click_outside(primary);
                } else if let Ok(slot) = usize::try_from(slot) {
                    if kind == ClickKind::QuickMove {
                        self.quick_move(slot);
                    } else {
                        self.pickup(slot, primary);
                    }
                }
            }
            ClickKind::Swap if (0..9).contains(&button) || button == 40 => {
                // The game reads `slots.get(slot)` with no test of the index (1105–1113).
                let slot = usize::try_from(slot).map_err(|_| Malformed)?;
                let held = usize::try_from(button).map_err(|_| Malformed)?;
                self.swap(slot, held);
            }
            ClickKind::Clone if self.creative && self.carried().is_none() => {
                if let Ok(slot) = usize::try_from(slot)
                    && let Some(here) = self.get(slot)
                {
                    // `safeClone`: a full stack of the item.
                    self.menu.carried = Some(here.with_count(max_stack_size(&here)));
                }
            }
            ClickKind::Throw if self.carried().is_none() => {
                if let Ok(slot) = usize::try_from(slot) {
                    self.throw(slot, button);
                }
            }
            ClickKind::PickupAll => {
                if let Ok(slot) = usize::try_from(slot) {
                    self.pickup_all(slot, button);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A drag's start, slot or end (`QUICK_CRAFT`, 14–544). The button carries the
    /// step in its low two bits (`getQuickcraftHeader`) and the drag's kind in the next
    /// two (`getQuickcraftType`). `Menu::drag` is the game's `quickcraftStatus`: none is
    /// 0, some is 1; the end's 2 lasts only through the click.
    fn quick_craft(&mut self, slot: i32, button: i32) -> Result<(), Malformed> {
        let before = i32::from(self.menu.drag.is_some());
        let header = button & 3;
        // Only a start with none under way, a slot of one under way and its end pass.
        if !(before == 1 && header == 2) && before != header {
            self.menu.drag = None;
            return Ok(());
        }
        let Some(carried) = self.carried() else {
            self.menu.drag = None;
            return Ok(());
        };
        match header {
            0 => {
                let kind = match (button >> 2) & 3 {
                    0 => Some(DragKind::Split),
                    1 => Some(DragKind::One),
                    // `isValidQuickcraftType`: the clone drag is a creative player's.
                    2 if self.creative => Some(DragKind::Clone),
                    _ => None,
                };
                self.menu.drag = kind.map(|kind| Drag {
                    kind,
                    slots: Vec::new(),
                });
            }
            1 => {
                // The game reads `slots.get(slot)` with no test of the index (134–145).
                let index = usize::try_from(slot).map_err(|_| Malformed)?;
                let taken = self.menu.drag.as_ref().map_or(0, |drag| drag.slots.len());
                let clone = self
                    .menu
                    .drag
                    .as_ref()
                    .is_some_and(|drag| drag.kind == DragKind::Clone);
                // 153–219: a slot that can take the carried stack, while there are more
                // items than slots so far (any number for a clone).
                if self.quick_replace(index, &carried)
                    && self.may_place(index, &carried)
                    && (clone || usize::try_from(carried.count).is_ok_and(|count| count > taken))
                    && let Ok(index) = u16::try_from(index)
                    && let Some(drag) = self.menu.drag.as_mut()
                    && !drag.slots.contains(&index)
                {
                    drag.slots.push(index);
                }
            }
            2 => {
                let drag = self.menu.drag.take();
                if let Some(drag) = drag {
                    return self.end_drag(&drag, carried);
                }
            }
            _ => self.menu.drag = None,
        }
        Ok(())
    }

    /// The end of a drag (231–537): a drag over one slot is a click on it with the
    /// drag's kind as the button; over several, each slot that can still take the
    /// carried stack gets its share and the carried stack keeps what is left.
    fn end_drag(&mut self, drag: &Drag, carried: ItemStack) -> Result<(), Malformed> {
        let kind = match drag.kind {
            DragKind::Split => 0,
            DragKind::One => 1,
            DragKind::Clone => 2,
        };
        match drag.slots.as_slice() {
            [] => Ok(()),
            // A middle drag over one slot comes back as a pickup of button 2, which is
            // no pickup's button and does nothing (3.14.1).
            [one] => self.do_click(i32::from(*one), kind, ClickKind::Pickup),
            several => {
                let copy = carried.clone();
                let mut left = carried.count;
                let count = several.len();
                let share = place_count(count, drag.kind, &copy);
                for &index in several {
                    let index = usize::from(index);
                    // The carried stack does not change in the loop: each slot's share
                    // depends only on the slot, the carried stack and how many slots
                    // there are, so the game's `HashSet` order does not matter. It
                    // matters only to the order of the equipment slots reported
                    // (`Clicked::equipped`), which here is the order dragged over (named:
                    // the game's is its `HashSet`'s, of no order a client can know).
                    if self.quick_replace(index, &carried)
                        && self.may_place(index, &carried)
                        && (drag.kind == DragKind::Clone
                            || usize::try_from(carried.count).is_ok_and(|n| n >= count))
                    {
                        let here = self.get(index).map_or(0, |here| here.count);
                        let most = max_stack_size(&copy).min(self.slot_max(index, &copy));
                        let now = share.saturating_add(here).min(most);
                        left -= now - here;
                        self.set(index, Some(copy.with_count(now)));
                    }
                }
                // A clone drag can leave `left` at or below nothing: an empty cursor.
                self.menu.carried = present(Some(copy.with_count(left)));
                Ok(())
            }
        }
    }

    /// `PICKUP` or `QUICK_MOVE` outside the window (588–655): the left button throws
    /// the carried stack, the right one item of it.
    fn click_outside(&mut self, primary: bool) {
        let Some(mut carried) = self.carried() else {
            return;
        };
        if primary {
            self.menu.carried = None;
            self.thrown.push(carried);
        } else {
            let one = split(&mut carried, 1);
            self.menu.carried = present(Some(carried));
            self.thrown.push(one);
        }
    }

    /// `PICKUP` on a slot (739–1068). No item of N6 overrides a click on it
    /// (`tryItemClickBehaviourOverride` is the bundle's).
    fn pickup(&mut self, slot: usize, primary: bool) {
        let carried = self.carried();
        // A slot that holds something is touched only where it may be taken from
        // (`mayPickup`, false only for a merchant's result without its price paid).
        if self.get(slot).is_some() && !self.may_pickup(slot) {
            return;
        }
        match (self.get(slot), carried) {
            (None, None) => {}
            (None, Some(carried)) => {
                let n = if primary { carried.count } else { 1 };
                self.menu.carried = self.safe_insert(slot, carried, n);
            }
            (Some(here), None) => {
                let n = if primary {
                    here.count
                } else {
                    (here.count + 1) / 2
                };
                if let Some(taken) = self.try_remove(slot, n, i32::MAX) {
                    self.menu.carried = Some(taken);
                }
            }
            (Some(here), Some(mut carried)) => {
                if self.may_place(slot, &carried) {
                    if here.same_kind(&carried) {
                        let n = if primary { carried.count } else { 1 };
                        self.menu.carried = self.safe_insert(slot, carried, n);
                    } else if carried.count <= self.slot_max(slot, &carried) {
                        self.menu.carried = Some(here);
                        self.set(slot, Some(carried));
                    }
                } else if here.same_kind(&carried) {
                    // A slot that takes nothing of the kind gives what fits the cursor.
                    let room = max_stack_size(&carried) - carried.count;
                    if let Some(taken) = self.try_remove(slot, here.count, room) {
                        carried.count += taken.count;
                        self.menu.carried = Some(carried);
                    }
                }
            }
        }
    }

    /// `QUICK_MOVE` on a slot (665–736): the menu's `quickMoveStack` again while the slot
    /// still holds the item it moved, until nothing more moves. Each round that goes
    /// on moved some of the slot's stack, so the loop ends; on a crafting result each
    /// round crafts and so consumes at least one item of the grid, which ends it too
    /// (and a full inventory moves nothing, which ends it sooner).
    fn quick_move(&mut self, slot: usize) {
        if !self.may_pickup(slot) {
            return;
        }
        let mut moved = self.quick_move_stack(slot);
        while let Some(stack) = moved
            && self.get(slot).is_some_and(|here| here.item == stack.item)
        {
            moved = self.quick_move_stack(slot);
        }
    }

    /// The menu's `quickMoveStack`: from the container to the player's slots, last
    /// first; from the player's slots to the container, first first. `ChestMenu` (for
    /// three and six rows), `ShulkerBoxMenu` and `HopperMenu` return the stack as it
    /// was whenever something moved (ChestMenu.txt 206–269, ShulkerBoxMenu.txt 90–150,
    /// HopperMenu.txt 71–131); `DispenserMenu` returns nothing when the count did not
    /// change (DispenserMenu.txt 105–168), which a move that moved never leaves.
    fn quick_move_stack(&mut self, slot: usize) -> Option<ItemStack> {
        if let MenuKind::Mount { .. } = self.slots.kind {
            return self.quick_move_mount(slot);
        }
        if let MenuKind::Merchant { .. } = self.slots.kind {
            return self.quick_move_merchant(slot);
        }
        if self.slots.kind == MenuKind::Inventory {
            return self.quick_move_inventory(slot);
        }
        if self.slots.kind == MenuKind::Crafting {
            return self.quick_move_crafting(slot);
        }
        let mut stack = self.get(slot)?;
        let copy = stack.clone();
        let containers = self.slots.kind.container_slots();
        let moved = if slot < containers {
            self.move_to(&mut stack, containers, self.slots.kind.slots(), true)
        } else {
            self.move_to(&mut stack, 0, containers, false)
        };
        if !moved {
            return None;
        }
        // The range moved into never holds the slot moved from, so the stack is written
        // back once, as the game's `setByPlayer(EMPTY)` or `setChanged` leaves it.
        self.set(slot, present(Some(stack.clone())));
        if self.slots.kind == MenuKind::Grid3x3 && stack.count == copy.count {
            return None;
        }
        Some(copy)
    }

    /// `AbstractMountInventoryMenu.quickMoveStack` (82–235): from the saddle, the body
    /// or the chest to the player's slots, last first; from the player's slots into the
    /// empty body slot that takes it, else the empty saddle slot that does, else the
    /// chest; failing those, between the main inventory and the hotbar, which answers
    /// nothing whatever moved, so that the click's loop stops after it.
    fn quick_move_mount(&mut self, slot: usize) -> Option<ItemStack> {
        let mut stack = self.get(slot)?;
        let copy = stack.clone();
        let inv = self.slots.kind.container_slots();
        let all = self.slots.kind.slots();
        let moved = if slot < inv {
            self.move_to(&mut stack, inv, all, true)
        } else if self.may_place(MOUNT_BODY, &stack) && self.get(MOUNT_BODY).is_none() {
            self.move_to(&mut stack, MOUNT_BODY, MOUNT_BODY + 1, false)
        } else if self.may_place(MOUNT_SADDLE, &stack) && self.get(MOUNT_SADDLE).is_none() {
            self.move_to(&mut stack, MOUNT_SADDLE, MOUNT_SADDLE + 1, false)
        } else if inv > 2 && self.move_to(&mut stack, 2, inv, false) {
            true
        } else {
            let main_end = inv + 27;
            let hotbar_end = main_end + 9;
            if (main_end..hotbar_end).contains(&slot) {
                self.move_to(&mut stack, inv, main_end, false);
            } else if (inv..main_end).contains(&slot) {
                self.move_to(&mut stack, main_end, hotbar_end, false);
            }
            // The stack moved in place, written back, and nothing answered.
            self.set(slot, present(Some(stack)));
            return None;
        };
        if !moved {
            return None;
        }
        self.set(slot, present(Some(stack)));
        Some(copy)
    }

    /// `MerchantMenu.quickMoveStack` (n10 MerchantMenu 199–304): the result to the
    /// player's slots, last first, and the trade made for what moved (`onTake`, which
    /// fills the result again where the payments pay once more, so that the click's loop
    /// trades on); a payment to the player's slots, first first; the main inventory to
    /// the hotbar and the hotbar to the main inventory. Nothing from the player's slots
    /// goes into a payment.
    fn quick_move_merchant(&mut self, slot: usize) -> Option<ItemStack> {
        let mut stack = self.get(slot)?;
        let copy = stack.clone();
        let all = self.slots.kind.slots();
        let inventory = self.slots.kind.container_slots();
        let main_end = inventory + 27;
        let moved = if slot == MERCHANT_RESULT {
            self.move_to(&mut stack, inventory, all, true)
        } else if slot < inventory {
            self.move_to(&mut stack, inventory, all, false)
        } else if slot < main_end {
            self.move_to(&mut stack, main_end, all, false)
        } else {
            self.move_to(&mut stack, inventory, main_end, false)
        };
        if !moved {
            return None;
        }
        self.set(slot, present(Some(stack.clone())));
        if stack.count == copy.count {
            return None;
        }
        if slot == MERCHANT_RESULT {
            self.result_taken();
        }
        Some(copy)
    }

    /// `InventoryMenu.quickMoveStack` (v3 InventoryMenu @0-390): the result to the main
    /// inventory and the hotbar, last first (@45-75: `moveItemStackTo(9, 45, true)`, the
    /// end not taken, so never the off hand); the grid and the armour to them, first
    /// first (@78-134). From any other slot (9-45), by the slot the player would wear the
    /// stack in (`getEquipmentSlotForItem`, n5 LivingEntity @10735-10753: the
    /// `equippable`'s slot, else the main hand): armour into its own armour slot if that
    /// is empty (@135-200: menu slot `8 - index`, and nothing else is tried if it will
    /// not go there), else an off-hand item into an empty off hand (@203-246), else the
    /// main inventory to the hotbar (@247-276), the hotbar to the main inventory
    /// (@277-306), and the off hand to both (@307-324). Then (@325-389) the slot is
    /// written back (`setByPlayer(EMPTY)` if emptied, which is an equipment slot's
    /// `onEquipItem`); an unchanged count answers nothing, which ends the click's loop;
    /// the slot's `onTake` runs; and what the result could not move is dropped
    /// (`player.drop(stack, false)`, [`Clicked::dropped`]).
    fn quick_move_inventory(&mut self, slot: usize) -> Option<ItemStack> {
        let mut stack = self.get(slot)?;
        let copy = stack.clone();
        let worn = equipment_slot_for(&copy);
        let moved = match slot {
            INVENTORY_RESULT => self.move_to(&mut stack, INVENTORY_MAIN, INVENTORY_OFF_HAND, true),
            _ if slot < INVENTORY_MAIN => {
                self.move_to(&mut stack, INVENTORY_MAIN, INVENTORY_OFF_HAND, false)
            }
            _ => {
                if let Some(armour) = armour_menu_slot(worn)
                    && self.get(armour).is_none()
                {
                    self.move_to(&mut stack, armour, armour + 1, false)
                } else if worn == slots::OFF_HAND && self.get(INVENTORY_OFF_HAND).is_none() {
                    self.move_to(&mut stack, INVENTORY_OFF_HAND, INVENTORY_END, false)
                } else if slot < INVENTORY_HOTBAR {
                    self.move_to(&mut stack, INVENTORY_HOTBAR, INVENTORY_OFF_HAND, false)
                } else if slot < INVENTORY_OFF_HAND {
                    self.move_to(&mut stack, INVENTORY_MAIN, INVENTORY_HOTBAR, false)
                } else {
                    self.move_to(&mut stack, INVENTORY_MAIN, INVENTORY_OFF_HAND, false)
                }
            }
        };
        if !moved {
            return None;
        }
        let rest = present(Some(stack.clone()));
        self.set(slot, rest.clone());
        if stack.count == copy.count {
            return None;
        }
        if slot == INVENTORY_RESULT {
            // The game throws the very stack the result slot still holds, and its
            // `onTake` has by then put the grid's new result into the slot
            // (`slotChangedCraftingGrid`), so the rest leaves the slot. The slot is
            // emptied first so that the result side sees the take and refills it.
            if rest.is_some() {
                self.set(slot, None);
            }
            self.result_taken();
            self.dropped.extend(rest);
        }
        Some(copy)
    }

    /// `CraftingMenu.quickMoveStack` (v3 CraftingMenu @0-236). The result (0): first
    /// `Item.onCraftedBy` on its stack (@47-55: statistics and a map's or a crafter's
    /// own ways, none of which Clustine keeps; named), then to the main inventory and the
    /// hotbar, last first (@58-75: `moveItemStackTo(10, 46, true)`), nothing answered if
    /// nothing moved. The grid (1-9) to them, first first (@154-171). The main inventory
    /// and the hotbar (10-45) into the grid first (@99-109: `moveItemStackTo(1, 10,
    /// false)`, where a merge of part of the stack counts as a move and the rest stays),
    /// and only if nothing went there the main inventory to the hotbar (@112-129) and the
    /// hotbar to the main inventory (@136-147). Then (@172-234) as the inventory screen:
    /// the slot written back, nothing answered for an unchanged count, `onTake`, and what
    /// the result could not move dropped (`player.drop(stack, false, PREDICTED)`).
    fn quick_move_crafting(&mut self, slot: usize) -> Option<ItemStack> {
        let mut stack = self.get(slot)?;
        let copy = stack.clone();
        let moved = if slot == CRAFTING_RESULT {
            self.move_to(&mut stack, CRAFTING_MAIN, CRAFTING_END, true)
        } else if slot < CRAFTING_MAIN {
            self.move_to(&mut stack, CRAFTING_MAIN, CRAFTING_END, false)
        } else if self.move_to(&mut stack, CRAFTING_RESULT + 1, CRAFTING_MAIN, false) {
            true
        } else if slot < CRAFTING_HOTBAR {
            self.move_to(&mut stack, CRAFTING_HOTBAR, CRAFTING_END, false)
        } else {
            self.move_to(&mut stack, CRAFTING_MAIN, CRAFTING_HOTBAR, false)
        };
        if !moved {
            return None;
        }
        let rest = present(Some(stack.clone()));
        self.set(slot, rest.clone());
        if stack.count == copy.count {
            return None;
        }
        if slot == CRAFTING_RESULT {
            // As the inventory screen's: the rest leaves the slot, which the take refills.
            if rest.is_some() {
                self.set(slot, None);
            }
            self.result_taken();
            self.dropped.extend(rest);
        }
        Some(copy)
    }

    /// Whether the menu is a merchant's.
    fn is_merchant(&self) -> bool {
        matches!(self.slots.kind, MenuKind::Merchant { .. })
    }

    /// The menu's result slot, whose stack the menu makes and which takes nothing
    /// placed: a merchant's trade (`MerchantResultSlot`) and the craft of the inventory
    /// screen and of the crafting table (`ResultSlot`, slot 0 of each). Every rule that
    /// asks for "the result slot" asks this.
    fn result_slot(&self) -> Option<usize> {
        match self.slots.kind {
            MenuKind::Merchant { .. } => Some(MERCHANT_RESULT),
            MenuKind::Inventory => Some(INVENTORY_RESULT),
            MenuKind::Crafting => Some(CRAFTING_RESULT),
            _ => None,
        }
    }

    /// The side of the menu's square crafting grid, if it has one: 2 for the inventory
    /// screen (`InventoryMenu`'s `TransientCraftingContainer(this, 2, 2)`), 3 for the
    /// crafting table (`CraftingMenu`'s, 3 by 3). The grid follows the result in the
    /// container, row by row.
    fn grid_side(&self) -> Option<usize> {
        match self.slots.kind {
            MenuKind::Inventory => Some(2),
            MenuKind::Crafting => Some(3),
            _ => None,
        }
    }

    /// The menu's crafting grid, if it has one.
    fn grid(&self) -> Option<&[Option<ItemStack>]> {
        let side = self.grid_side()?;
        self.slots.container.get(1..1 + side * side)
    }

    /// `CraftingMenu.slotChangedCraftingGrid` (v3 @0-147): the first recipe that matches
    /// the grid, assembled, into the result (`ResultContainer.setItem(0, …)`), or nothing.
    ///
    /// Named: `ResultContainer.setRecipeUsed` (@67-76) refuses a recipe the player has
    /// not unlocked only under the `doLimitedCrafting` rule, which Clustine does not have
    /// (its default is off), and records the recipe for the recipe book (V9);
    /// `isItemEnabled` (@90-99) holds for every item (no experimental features). The
    /// packet the game sends straight away is the edge's (the region's diff, V3c).
    fn recompute_result(&mut self) {
        let Some(side) = self.grid_side() else {
            return;
        };
        let Some(grid) = self.grid().map(<[Option<ItemStack>]>::to_vec) else {
            return;
        };
        let result = crafting::craft(side, side, &grid).map(|(_, stack)| stack);
        if let Some(place) = self.slots.container.get_mut(0) {
            *place = result;
        }
        self.grid_seen = Some(grid);
    }

    /// `ResultSlot.onTake` (v3 @0-271) after the result has left its slot: the grid's
    /// remainders as the grid stands (`getRemainingItems`, @5-43), then for each slot of
    /// the grid in order one item taken (@128-160), and its remainder put into the slot
    /// if that is now empty (@170-191), grown by the slot's stack and put back if it is
    /// the same item with the same components (@194-227; the game grows it past a full
    /// stack if need be, and so does this), else added to the inventory (`Inventory.add`,
    /// @230-242), else dropped (`player.drop(rem, false, PREDICTED)`, @245-258). Then the
    /// result worked out again from the grid (the game's `slotsChanged` at each write).
    ///
    /// The game's loop runs over the box of the grid's non-empty slots only; every slot
    /// outside it is empty and has no remainder, so running over the whole grid is the
    /// same.
    ///
    /// A take happens only on a result a recipe made: the result is worked out after
    /// every change of the grid, inside a click at every take. Where no recipe matches,
    /// `getRemainingItems` would copy the grid (`copyAllInputItems`) and the loop would
    /// double it; that is never applied here, and the take only works the result out.
    ///
    /// Named: `checkTakeAchievements` (@0-2: `onCraftedBy` with the count taken, the
    /// crafting statistics and the recipes it unlocks for the recipe book) is not run;
    /// Clustine keeps no statistics, and the recipe book is V9. `removeCount` feeds only
    /// that. `placingRecipe` is set only by the recipe book's placing, which is passed
    /// over.
    fn craft_taken(&mut self, side: usize) {
        let Some(grid) = self.grid().map(<[Option<ItemStack>]>::to_vec) else {
            return;
        };
        if crafting::craft(side, side, &grid).is_none() {
            self.recompute_result();
            return;
        }
        let remainders = crafting::remainders(side, side, &grid);
        for (at, remainder) in remainders.into_iter().enumerate() {
            let index = 1 + at;
            let mut here = self.get(index);
            if let Some(mut stack) = here.take() {
                stack.count -= 1;
                here = present(Some(stack));
                self.set(index, here.clone());
            }
            let Some(mut remainder) = present(remainder) else {
                continue;
            };
            match here {
                None => self.set(index, Some(remainder)),
                Some(here) if here.same_kind(&remainder) => {
                    remainder.count += here.count;
                    self.set(index, Some(remainder));
                }
                Some(_) => {
                    let creative = self.creative;
                    let selected = self.slots.selected;
                    if !add(
                        self.slots.inventory,
                        selected,
                        None,
                        &mut remainder,
                        creative,
                    ) && let Some(rest) = present(Some(remainder))
                    {
                        self.dropped.push(rest);
                    }
                }
            }
        }
        self.recompute_result();
    }

    /// Whether `index` is the menu's result slot.
    fn is_result(&self, index: usize) -> bool {
        self.result_slot() == Some(index)
    }

    /// `Slot.mayPickup`: true but for a merchant's result, which asks the merchant, and
    /// an armour slot of the inventory screen that holds a stack with Curse of Binding
    /// (`ArmorSlot.mayPickup` @0-36: `EnchantmentHelper.has(stack,
    /// PREVENT_ARMOR_CHANGE)`), unless the player is creative. The game asks
    /// `Player.isCreative()`, the game mode; the click's `creative` is the player's
    /// `instabuild`, which only the creative mode gives, so the two agree.
    fn may_pickup(&self, index: usize) -> bool {
        if self.slots.kind == MenuKind::Inventory
            && (INVENTORY_ARMOUR..INVENTORY_MAIN).contains(&index)
        {
            return self.creative || !self.get(index).is_some_and(|here| binds(&here));
        }
        if !(self.is_merchant() && index == MERCHANT_RESULT) {
            return true;
        }
        self.slots
            .merchant
            .as_deref()
            .is_some_and(|merchant| merchant.may_pickup(self.slots.container))
    }

    /// The result slot's `onTake`: for a merchant's window, the merchant's trade for the
    /// result taken (`MerchantResultSlot.onTake`); for a crafting grid, the craft
    /// (`ResultSlot.onTake`, [`Self::craft_taken`]).
    fn result_taken(&mut self) {
        if let Some(side) = self.grid_side() {
            self.craft_taken(side);
            return;
        }
        let MenuSlots {
            container,
            merchant,
            ..
        } = &mut *self.slots;
        if let Some(merchant) = merchant.as_deref_mut() {
            merchant.on_take(container);
        }
    }

    /// `moveItemStackTo(stack, start, end, reverse)` (AbstractContainerMenu.txt
    /// 1940–2091): first it merges into the slots of the same item in order (last first
    /// when `reverse`), asking no slot's `mayPlace`; then, if anything is left, it puts
    /// it all into the first empty slot in the same order that may take it, and stops
    /// there. Whether anything moved.
    fn move_to(&mut self, stack: &mut ItemStack, start: usize, end: usize, reverse: bool) -> bool {
        let order: Vec<usize> = if reverse {
            (start..end).rev().collect()
        } else {
            (start..end).collect()
        };
        let mut moved = false;
        // `isStackable` is asked once, before the merge (16–20).
        if max_stack_size(stack) > 1 {
            for &index in &order {
                if stack.count <= 0 {
                    break;
                }
                let Some(mut here) = self.get(index) else {
                    continue;
                };
                if !here.same_kind(stack) {
                    continue;
                }
                let total = here.count + stack.count;
                let most = self.slot_max(index, &here);
                if total <= most {
                    stack.count = 0;
                    here.count = total;
                    self.set(index, Some(here));
                    moved = true;
                } else if here.count < most {
                    stack.count -= most - here.count;
                    here.count = most;
                    self.set(index, Some(here));
                    moved = true;
                }
            }
        }
        if stack.count > 0 {
            for &index in &order {
                if self.get(index).is_none() && self.may_place(index, stack) {
                    let most = self.slot_max(index, stack);
                    let part = split(stack, stack.count.min(most));
                    self.set(index, Some(part));
                    moved = true;
                    break;
                }
            }
        }
        moved
    }

    /// `SWAP` of the menu's slot `slot` with the inventory's slot `held` (a hotbar slot,
    /// or 40 the off hand), 1097–1380. The inventory is written directly, so where the
    /// slot clicked shows that same inventory slot the two are one.
    fn swap(&mut self, slot: usize, held: usize) {
        let same = self.inventory_slot(slot) == Some(held);
        let held_stack = present(self.slots.inventory.get(held).flatten());
        match (held_stack, self.get(slot)) {
            (None, None) => {}
            (None, Some(here)) => {
                if !self.may_pickup(slot) {
                    return;
                }
                self.slots.inventory.set(held, Some(here));
                self.set(slot, None);
                if self.is_result(slot) {
                    self.result_taken();
                }
            }
            (Some(mut stack), None) => {
                if self.may_place(slot, &stack) {
                    let most = self.slot_max(slot, &stack);
                    if stack.count > most {
                        let part = split(&mut stack, most);
                        self.slots.inventory.set(held, present(Some(stack)));
                        self.set(slot, Some(part));
                    } else {
                        self.slots.inventory.set(held, None);
                        self.set(slot, Some(stack));
                    }
                }
            }
            (Some(mut stack), Some(here)) => {
                // 1271-1288: the slot must give up what it holds and take the held stack
                // (a cursed helmet stays on).
                if self.may_pickup(slot) && self.may_place(slot, &stack) {
                    let most = self.slot_max(slot, &stack);
                    if stack.count > most {
                        // The held stack is split where it lies, and the slot's stack
                        // goes back into the inventory, or out of the window if it does
                        // not fit. Where the two slots are one, what goes back is what
                        // the split left (no N6 slot holds less than a full stack, so
                        // this is the game's rule more than a case that arises).
                        let part = split(&mut stack, most);
                        self.slots.inventory.set(held, present(Some(stack.clone())));
                        let mut back = if same { stack } else { here };
                        self.set(slot, Some(part));
                        let creative = self.creative;
                        let selected = self.slots.selected;
                        if !add(self.slots.inventory, selected, None, &mut back, creative)
                            && let Some(back) = present(Some(back))
                        {
                            self.thrown.push(back);
                        }
                    } else {
                        self.slots.inventory.set(held, Some(here));
                        self.set(slot, Some(stack));
                    }
                }
            }
        }
    }

    /// `THROW` of the slot's stack (1454–1612): one item with button 0, the whole
    /// stack with any other; with button 1 again while the slot gives more of the same,
    /// which only a result slot does. Nothing in the inventory bounds that loop on a
    /// crafting result: each round crafts, consuming at least one item of the grid, and
    /// the loop ends when the grid no longer makes the item (64 logs throw 64 crafts).
    fn throw(&mut self, slot: usize, button: i32) {
        let n = if button == 0 {
            1
        } else {
            self.get(slot).map_or(0, |here| here.count)
        };
        let mut taken = self.try_remove(slot, n, i32::MAX);
        if let Some(ref stack) = taken {
            self.thrown.push(stack.clone());
        }
        if button == 1 {
            while let Some(stack) = taken
                && self.get(slot).is_some_and(|here| here.item == stack.item)
            {
                taken = self.try_remove(slot, n, i32::MAX);
                if let Some(ref stack) = taken {
                    self.thrown.push(stack.clone());
                }
            }
        }
    }

    /// `PICKUP_ALL`, a double click on an empty slot with a stack carried (1622–1871):
    /// it gathers the carried item from the menu's slots, from the first with button 0
    /// and from the last with any other, container and inventory alike, first from the
    /// stacks that are not full and then from the full ones, until the cursor is full.
    fn pickup_all(&mut self, slot: usize, button: i32) {
        // A merchant's window gathers nothing (`MerchantMenu.canTakeItemForPickAll`).
        if self.is_merchant() {
            return;
        }
        let Some(mut carried) = self.carried() else {
            return;
        };
        if self.get(slot).is_some() {
            return;
        }
        let size = self.slots.kind.slots();
        let order: Vec<usize> = if button == 0 {
            (0..size).collect()
        } else {
            (0..size).rev().collect()
        };
        let most = max_stack_size(&carried);
        for pass in 0..2 {
            for &index in &order {
                if carried.count >= most {
                    break;
                }
                let Some(here) = self.get(index) else {
                    continue;
                };
                // A crafting grid's result gives nothing to a gathering
                // (`InventoryMenu.canTakeItemForPickAll` @0-25, `CraftingMenu.
                // canTakeItemForPickAll` @0-25: the slot's container is not the result's).
                if !self.quick_replace(index, &carried) || self.is_result(index) {
                    continue;
                }
                if pass == 0 && here.count == max_stack_size(&here) {
                    continue;
                }
                if let Some(taken) = self.try_remove(index, here.count, most - carried.count) {
                    carried.count += taken.count;
                }
            }
        }
        self.menu.carried = Some(carried);
    }

    /// The carried stack, if there is one.
    fn carried(&self) -> Option<ItemStack> {
        present(self.menu.carried.clone())
    }

    /// The inventory slot the menu's slot `index` shows, if it shows one.
    fn inventory_slot(&self, index: usize) -> Option<usize> {
        inventory_slot_of(self.slots.kind, index)
    }

    /// The stack in the menu's slot `index` (`Slot.getItem`), none if it is empty.
    /// `click` has checked that the index is the menu's.
    fn get(&self, index: usize) -> Option<ItemStack> {
        menu_slot(self.slots, index).flatten()
    }

    /// Puts `stack` into the menu's slot `index` (`Slot.set`, or `setByPlayer`).
    ///
    /// Every write of a click to a slot is one the game makes by `setByPlayer` (a
    /// `safeInsert`, a `tryRemove` that empties the slot, a swap, a drag's share, a
    /// placing by `moveItemStackTo`, a quick move's emptied slot) or one that leaves the
    /// same item with the same components (a partial take, a merge, `setChanged`), so an
    /// equipment slot of the inventory screen whose item or components change here is
    /// one the game's `onEquipItem` hears of ([`Clicked::equipped`]).
    fn set(&mut self, index: usize, stack: Option<ItemStack>) {
        let stack = present(stack);
        if self.slots.kind == MenuKind::Inventory && is_equipment_slot(index) {
            let old = self.get(index);
            let same = match (&old, &stack) {
                (None, None) => true,
                (Some(old), Some(new)) => old.same_kind(new),
                _ => false,
            };
            if !same {
                self.equipped.push(index);
            }
        }
        if index < self.slots.container.len() {
            if let Some(place) = self.slots.container.get_mut(index) {
                *place = stack;
            }
        } else if let Some(slot) = self.inventory_slot(index) {
            self.slots.inventory.set(slot, stack);
        }
    }

    /// `Slot.mayPlace`: true but for a shulker box's slot, which refuses what may not go
    /// inside a container (`ShulkerBoxSlot`), and a horse's saddle and body, which take
    /// only what the horse may wear there (`ArmorSlot`: `isEquippableInSlot`); a result,
    /// which takes nothing (`MerchantResultSlot`, `ResultSlot.mayPlace` @0-1, v3); and the
    /// inventory screen's armour, which takes only what the player wears there
    /// (`ArmorSlot.mayPlace` @0-12).
    fn may_place(&self, index: usize, stack: &ItemStack) -> bool {
        if self.is_result(index) {
            return false;
        }
        if self.slots.kind == MenuKind::Inventory
            && let Some(slot) = armour_equipment_slot(index)
        {
            return equippable_in_slot(stack, slot);
        }
        if let MenuKind::Mount { .. } = self.slots.kind
            && index <= MOUNT_BODY
        {
            let Some(facts) = self.slots.mount else {
                return false;
            };
            let (slot, usable) = if index == MOUNT_SADDLE {
                (crate::living::slots::SADDLE, facts.saddle)
            } else {
                (crate::living::slots::BODY, facts.body)
            };
            return usable
                && crate::mobs::types::equine::equippable_by(stack.item, slot, facts.kind);
        }
        !(self.slots.kind == MenuKind::ShulkerBox
            && index < self.slots.kind.container_slots()
            && !fits_inside_containers(stack.item))
    }

    /// `Slot.getMaxStackSize(stack)`: the container's most, or the item's if less; one
    /// in a horse's saddle and body and in the inventory screen's armour (`ArmorSlot.
    /// getMaxStackSize` @0-1).
    fn slot_max(&self, index: usize, stack: &ItemStack) -> i32 {
        if let MenuKind::Mount { .. } = self.slots.kind
            && index <= MOUNT_BODY
        {
            return 1;
        }
        if self.slots.kind == MenuKind::Inventory && armour_equipment_slot(index).is_some() {
            return 1;
        }
        CONTAINER_MAX.min(max_stack_size(stack))
    }

    /// `canItemQuickReplace(slot, stack, true)` (2154–2190): the slot is empty, or holds
    /// the same item with the same components and no more than a full stack of it.
    fn quick_replace(&self, index: usize, stack: &ItemStack) -> bool {
        self.get(index)
            .is_none_or(|here| here.same_kind(stack) && here.count <= max_stack_size(stack))
    }

    /// `Slot.tryRemove(count, limit, player)` (Slot.txt 166–208) as `safeTake` runs it:
    /// up to `count`, and nothing if the slot would refuse what it holds and `limit` is
    /// less than all of it.
    fn try_remove(&mut self, index: usize, count: i32, limit: i32) -> Option<ItemStack> {
        let mut here = self.get(index)?;
        if !self.may_pickup(index) {
            return None;
        }
        // A result is taken whole (`MerchantContainer.removeItem`; `ResultContainer.
        // removeItem`, `ContainerHelper.takeItem`, v3 ResultContainer.txt 53-59), and the
        // take is the trade or the craft (`safeTake`'s and the pickup's `onTake`).
        let result = self.is_result(index);
        let count = if result { here.count } else { count };
        // `allowModification`: `mayPickup` (always) and `mayPlace` of what it holds.
        if !self.may_place(index, &here) && limit < here.count {
            return None;
        }
        let count = count.min(limit);
        // `Container.removeItem` (`ContainerHelper.removeItem`) takes nothing of none.
        if count <= 0 {
            return None;
        }
        let removed = split(&mut here, count);
        self.set(index, present(Some(here)));
        if result {
            self.result_taken();
        }
        present(Some(removed))
    }

    /// `Slot.safeInsert(stack, n)` (Slot.txt 249–302): up to `n` of `stack` into the
    /// slot, as far as it has room and takes the item; what is left of the stack.
    fn safe_insert(&mut self, index: usize, mut stack: ItemStack, n: i32) -> Option<ItemStack> {
        if !self.may_place(index, &stack) {
            return Some(stack);
        }
        let here = self.get(index);
        let room = self.slot_max(index, &stack) - here.as_ref().map_or(0, |here| here.count);
        let m = n.min(stack.count).min(room);
        if m <= 0 {
            return Some(stack);
        }
        match here {
            None => {
                let part = split(&mut stack, m);
                self.set(index, Some(part));
            }
            Some(mut here) if here.same_kind(&stack) => {
                stack.count -= m;
                here.count += m;
                self.set(index, Some(here));
            }
            Some(_) => {}
        }
        present(Some(stack))
    }
}

/// `getQuickCraftPlaceCount(slots, type, stack)` (2192–2216): what a drag puts into
/// each slot before the slot's own most. The game divides as floats and floors; for
/// counts of a stack that is the integer division.
fn place_count(slots: usize, kind: DragKind, stack: &ItemStack) -> i32 {
    match kind {
        DragKind::Split => stack.count / i32::try_from(slots).unwrap_or(i32::MAX).max(1),
        DragKind::One => 1,
        DragKind::Clone => max_stack_size(stack),
    }
}

/// The stack if it is one: the game's `isEmpty` is air or a count of none or less.
fn present(stack: Option<ItemStack>) -> Option<ItemStack> {
    stack.filter(|stack| stack.count > 0 && stack.item != items::AIR)
}

/// `ItemStack.split(n)`: takes up to `n` from `stack` and returns them.
fn split(stack: &mut ItemStack, n: i32) -> ItemStack {
    let taken = n.min(stack.count);
    stack.count -= taken;
    stack.with_count(taken)
}

/// `Item.canFitInsideContainerItems`: false only for a block item whose block is a
/// shulker box (BlockItem.txt 393–402); the bundle's rule is not read and no bundle
/// holds anything without components.
fn fits_inside_containers(item: i32) -> bool {
    !usize::try_from(item)
        .ok()
        .and_then(|item| ITEMS.get(item))
        .and_then(|item| item.block)
        .is_some_and(|block| is_class(block, BlockClass::ShulkerBoxBlock))
}

/// Whether the inventory screen's slot `index` is one of its equipment slots, whose
/// `setByPlayer` tells the player of what it wears (the armour and the off hand).
fn is_equipment_slot(index: usize) -> bool {
    (INVENTORY_ARMOUR..INVENTORY_MAIN).contains(&index) || index == INVENTORY_OFF_HAND
}

/// The `EquipmentSlot` (by ordinal, as [`slots`] numbers them) of the inventory screen's
/// armour slot `index`, if it is one: 5 the head down to 8 the feet (`SLOT_IDS`).
fn armour_equipment_slot(index: usize) -> Option<u8> {
    match index {
        5 => Some(slots::HEAD),
        6 => Some(slots::CHEST),
        7 => Some(slots::LEGS),
        8 => Some(slots::FEET),
        _ => None,
    }
}

/// The inventory screen's armour slot for the `EquipmentSlot` `slot`, if it is a
/// `HUMANOID_ARMOR` one: `8 - slot.getIndex()` (InventoryMenu @146-178), the index of
/// the feet being 0 and of the head 3.
fn armour_menu_slot(slot: u8) -> Option<usize> {
    (INVENTORY_ARMOUR..INVENTORY_MAIN).find(|&index| armour_equipment_slot(index) == Some(slot))
}

/// `LivingEntity.getEquipmentSlotForItem(stack)` for a player (n5 @10735-10753): the
/// slot of the item's `equippable` where the player can use it, else the main hand. A
/// player can use every slot that matters here; an animal's body or a saddle is neither
/// armour nor the off hand, so the quick move treats it as the main hand either way.
///
/// Named difference: the `equippable` read is the item's own; a stack whose patch
/// changes or removes it (by a command, say) is read as its item. The stacks keep no
/// such component in Clustine.
fn equipment_slot_for(stack: &ItemStack) -> u8 {
    equippable(stack).map_or(slots::MAIN_HAND, |equippable| equippable.slot)
}

/// `LivingEntity.isEquippableInSlot(stack, slot)` for a player (n5 @10755-10792): with
/// no `equippable`, the main hand only; with one, its own slot (`canUseSlot`, true for a
/// player) where `canBeEquippedBy` the player.
///
/// Named difference: `canBeEquippedBy` reads the component's `allowed_entities`, which
/// the generated data does not keep; no item of 26.3 whose slot is a player's armour
/// names any, so every one is allowed. The item's own component is read, as in
/// [`equipment_slot_for`].
fn equippable_in_slot(stack: &ItemStack, slot: u8) -> bool {
    match equippable(stack) {
        None => slot == slots::MAIN_HAND,
        Some(equippable) => equippable.slot == slot,
    }
}

/// The `minecraft:equippable` of the stack's item, where it has one.
fn equippable(stack: &ItemStack) -> Option<clustine_data::Equippable> {
    usize::try_from(stack.item)
        .ok()
        .filter(|item| *item < ITEMS.len())
        .and_then(|_| ItemComponents::of(stack.item).equippable)
}

/// `EnchantmentHelper.has(stack, PREVENT_ARMOR_CHANGE)`: of 26.3's enchantments only
/// Curse of Binding has that effect, read from the stack's `minecraft:enchantments`.
fn binds(stack: &ItemStack) -> bool {
    stack
        .components
        .enchantment_level("minecraft:binding_curse")
        > 0
}

/// `Inventory.getMaxStackSize(stack)`: the inventory's most, or the item's if less.
fn inventory_max(stack: &ItemStack) -> i32 {
    CONTAINER_MAX.min(max_stack_size(stack))
}

/// `Inventory.getSlotWithRemainingSpace(stack)` (Inventory.txt 494–538): the selected
/// slot, then the off hand, then 0 to 35, the first that holds the same item with the
/// same components, stacks and is not full (`hasRemainingSpaceForItem`, 101–123).
fn slot_with_room(inventory: &Inventory, selected: usize, stack: &ItemStack) -> Option<usize> {
    [selected, OFF_HAND]
        .into_iter()
        .chain(0..ITEM_SLOTS)
        .find(|&slot| {
            present(inventory.get(slot).flatten()).is_some_and(|here| {
                here.same_kind(stack)
                    && max_stack_size(&here) > 1
                    && here.count < inventory_max(&here)
            })
        })
}

/// `Inventory.getFreeSlot()` (124–146): the first empty slot of 0 to 35.
fn free_slot(inventory: &Inventory) -> Option<usize> {
    (0..ITEM_SLOTS).find(|&slot| present(inventory.get(slot).flatten()).is_none())
}

/// `Inventory.addResource(slot, stack)` (446–493): as much of `stack` as `slot` has
/// room for, returning what is left.
fn add_resource_at(inventory: &mut Inventory, slot: usize, stack: &ItemStack) -> i32 {
    let here = present(inventory.get(slot).flatten()).unwrap_or_else(|| stack.with_count(0));
    let room = inventory_max(&here) - here.count;
    let m = stack.count.min(room);
    if m <= 0 {
        return stack.count;
    }
    inventory.set(slot, Some(here.with_count(here.count + m)));
    stack.count - m
}

/// `Inventory.add(slot, stack)` (581–714) for a stack that wears no damage, which is
/// every stack without components: into `slot`, or with none where
/// `getSlotWithRemainingSpace` and then `getFreeSlot` say, again while each round adds
/// something. `stack` keeps what was not added. Whether it was all added; for a
/// creative player (`hasInfiniteMaterials`) a round that adds nothing deletes the rest
/// and counts as all added.
fn add(
    inventory: &mut Inventory,
    selected: usize,
    slot: Option<usize>,
    stack: &mut ItemStack,
    creative: bool,
) -> bool {
    if stack.count <= 0 || stack.item == items::AIR {
        return false;
    }
    let mut before;
    loop {
        before = stack.count;
        stack.count = match slot {
            Some(slot) => add_resource_at(inventory, slot, stack),
            None => {
                match slot_with_room(inventory, selected, stack).or_else(|| free_slot(inventory)) {
                    Some(slot) => add_resource_at(inventory, slot, stack),
                    None => stack.count,
                }
            }
        };
        if stack.count <= 0 || stack.count >= before {
            break;
        }
    }
    if stack.count == before && creative {
        stack.count = 0;
        return true;
    }
    stack.count < before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MenuOf;
    use clustine_world::{BlockPos, INVENTORY_SLOTS};

    const STONE: i32 = items::STONE;
    const DIRT: i32 = items::DIRT;
    const PEARL: i32 = items::ENDER_PEARL;
    const BOX: i32 = items::SHULKER_BOX;

    fn stack(item: i32, count: i32) -> Option<ItemStack> {
        Some(ItemStack::new(item, count))
    }

    /// A menu of `kind` over an empty container and an empty inventory.
    struct Fixture {
        kind: MenuKind,
        container: Vec<Option<ItemStack>>,
        inventory: Inventory,
        selected: usize,
        menu: Menu,
    }

    impl Fixture {
        fn new(kind: MenuKind) -> Self {
            Self {
                kind,
                container: vec![None; kind.container_slots()],
                inventory: Inventory::default(),
                selected: 0,
                menu: Menu {
                    serial: 1,
                    of: MenuOf::Block {
                        at: BlockPos { x: 0, y: 64, z: 0 },
                        other: None,
                    },
                    kind,
                    carried: None,
                    drag: None,
                    trade: None,
                    grid: None,
                },
            }
        }

        fn click(&mut self, slot: i16, button: i8, kind: ClickKind, creative: bool) -> Clicked {
            let mut slots = MenuSlots {
                kind: self.kind,
                container: &mut self.container,
                inventory: &mut self.inventory,
                selected: self.selected,
                mount: None,
                merchant: None,
            };
            click(&mut slots, &mut self.menu, slot, button, kind, creative)
        }

        /// A click by a player who is not creative.
        fn survival(&mut self, slot: i16, button: i8, kind: ClickKind) -> Clicked {
            self.click(slot, button, kind, false)
        }

        /// The stack the menu shows in `index`.
        fn at(&mut self, index: usize) -> Option<ItemStack> {
            let slots = MenuSlots {
                kind: self.kind,
                container: &mut self.container,
                inventory: &mut self.inventory,
                selected: self.selected,
                mount: None,
                merchant: None,
            };
            menu_slot(&slots, index).expect("the menu has the slot")
        }

        /// Puts `stack` into the menu's slot `index`.
        fn put(&mut self, index: usize, stack: Option<ItemStack>) {
            if index < self.container.len() {
                self.container[index] = stack;
            } else {
                let slot = inventory_slot_of(self.kind, index).expect("an inventory slot");
                self.inventory.set(slot, stack);
            }
        }

        /// Works the result of a crafting grid out from the grid, as the region keeps it
        /// after every change.
        fn settle(&mut self) {
            let side = if self.kind == MenuKind::Crafting {
                3
            } else {
                2
            };
            self.container[0] =
                crafting::craft(side, side, &self.container[1..]).map(|(_, stack)| stack);
        }

        /// A snapshot of everything a click may change.
        fn snapshot(&self) -> (Vec<Option<ItemStack>>, Inventory, Menu) {
            (
                self.container.clone(),
                self.inventory.clone(),
                self.menu.clone(),
            )
        }

        /// The drag's buttons: `header` the step, `kind` 0 split, 1 one, 2 clone.
        fn drag(&mut self, kind: i8, over: &[i16], creative: bool) -> Clicked {
            self.click(-999, kind << 2, ClickKind::QuickCraft, creative);
            for &slot in over {
                self.click(slot, (kind << 2) | 1, ClickKind::QuickCraft, creative);
            }
            self.click(-999, (kind << 2) | 2, ClickKind::QuickCraft, creative)
        }
    }

    // The chest's three rows are 0 to 26, the main inventory 27 to 53 (inventory 9 to
    // 35), the hotbar 54 to 62 (inventory 0 to 8).
    const CHEST_MAIN: usize = 27;
    const CHEST_HOTBAR: usize = 54;

    #[test]
    fn the_menus_lay_out_the_container_then_the_main_inventory_then_the_hotbar() {
        assert_eq!(inventory_slot_of(MenuKind::Rows3, 26), None);
        assert_eq!(inventory_slot_of(MenuKind::Rows3, 27), Some(9));
        assert_eq!(inventory_slot_of(MenuKind::Rows3, 53), Some(35));
        assert_eq!(inventory_slot_of(MenuKind::Rows3, 54), Some(0));
        assert_eq!(inventory_slot_of(MenuKind::Rows3, 62), Some(8));
        assert_eq!(inventory_slot_of(MenuKind::Rows3, 63), None);
        assert_eq!(inventory_slot_of(MenuKind::Hopper, 5), Some(9));
        assert_eq!(inventory_slot_of(MenuKind::Hopper, 32), Some(0));
        assert_eq!(inventory_slot_of(MenuKind::Grid3x3, 44), Some(8));
        assert_eq!(inventory_slot_of(MenuKind::Rows6, 54), Some(9));
        assert_eq!(inventory_slot_of(MenuKind::ShulkerBox, 62), Some(8));
        for kind in [
            MenuKind::Rows3,
            MenuKind::Rows6,
            MenuKind::ShulkerBox,
            MenuKind::Grid3x3,
            MenuKind::Hopper,
        ] {
            for slot in 0..36 {
                let index = menu_slot_of(kind, slot).expect("every item slot is shown");
                assert_eq!(inventory_slot_of(kind, index), Some(slot));
            }
            for slot in 36..INVENTORY_SLOTS {
                assert_eq!(menu_slot_of(kind, slot), None);
            }
        }
    }

    #[test]
    fn a_menu_slot_reads_the_container_and_the_inventory() {
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.container[4] = stack(STONE, 3);
        fixture.inventory.set(9, stack(DIRT, 2));
        fixture.inventory.set(0, stack(PEARL, 1));
        let slots = MenuSlots {
            kind: fixture.kind,
            container: &mut fixture.container,
            inventory: &mut fixture.inventory,
            selected: 0,
            mount: None,
            merchant: None,
        };
        assert_eq!(menu_slot(&slots, 4), Some(stack(STONE, 3)));
        assert_eq!(menu_slot(&slots, 5), Some(stack(DIRT, 2)));
        assert_eq!(menu_slot(&slots, 32), Some(stack(PEARL, 1)));
        assert_eq!(menu_slot(&slots, 0), Some(None));
        assert_eq!(menu_slot(&slots, 41), None);
    }

    #[test]
    fn a_left_click_picks_up_the_whole_stack_and_a_right_click_half_rounded_up() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(0, stack(STONE, 7));
        assert_eq!(
            fixture.survival(0, 1, ClickKind::Pickup),
            Clicked::default()
        );
        assert_eq!(fixture.menu.carried, stack(STONE, 4));
        assert_eq!(fixture.at(0), stack(STONE, 3));
        fixture.menu.carried = None;
        fixture.survival(0, 0, ClickKind::Pickup);
        assert_eq!(fixture.menu.carried, stack(STONE, 3));
        assert_eq!(fixture.at(0), None);
    }

    #[test]
    fn a_left_click_puts_the_carried_stack_down_and_a_right_click_one_of_it() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.menu.carried = stack(STONE, 10);
        fixture.survival(5, 1, ClickKind::Pickup);
        assert_eq!(fixture.at(5), stack(STONE, 1));
        assert_eq!(fixture.menu.carried, stack(STONE, 9));
        fixture.survival(6, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(6), stack(STONE, 9));
        assert_eq!(fixture.menu.carried, None);
    }

    #[test]
    fn a_click_on_the_same_item_fills_the_slot_and_keeps_the_rest_carried() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(0, stack(STONE, 60));
        fixture.menu.carried = stack(STONE, 10);
        fixture.survival(0, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(0), stack(STONE, 64));
        assert_eq!(fixture.menu.carried, stack(STONE, 6));
        // Ender pearls stack to 16.
        fixture.put(1, stack(PEARL, 15));
        fixture.menu.carried = stack(PEARL, 5);
        fixture.survival(1, 1, ClickKind::Pickup);
        assert_eq!(fixture.at(1), stack(PEARL, 16));
        assert_eq!(fixture.menu.carried, stack(PEARL, 4));
    }

    /// A stack of `item` named `name`, which no plain stack of it is the same as.
    fn named(item: i32, count: i32, name: &str) -> Option<ItemStack> {
        let mut stack = ItemStack::new(item, count);
        stack
            .components
            .set(clustine_world::Component::ItemName(format!("{name:?}")));
        Some(stack)
    }

    /// The menus merge by item and components (`isSameItemSameComponents`): a carried
    /// stack placed onto one that differs in them swaps with it, onto one alike fills it.
    #[test]
    fn a_click_merges_the_carried_stack_only_into_one_with_the_same_components() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(0, named(STONE, 60, "a"));
        fixture.menu.carried = stack(STONE, 10);
        fixture.survival(0, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(0), stack(STONE, 10));
        assert_eq!(fixture.menu.carried, named(STONE, 60, "a"));
        fixture.put(1, named(STONE, 1, "a"));
        fixture.survival(1, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(1), named(STONE, 61, "a"));
        assert_eq!(fixture.menu.carried, None);
    }

    /// A quick move merges into stacks alike only, and a double click gathers only those.
    #[test]
    fn a_quick_move_and_a_pickup_all_pass_over_stacks_whose_components_differ() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(0, stack(STONE, 10));
        fixture.put(CHEST_HOTBAR + 8, named(STONE, 5, "a"));
        fixture.survival(0, 0, ClickKind::QuickMove);
        // From the hotbar's right end: the named stack is passed over, and the stone
        // goes into the first empty slot in that order.
        assert_eq!(fixture.at(CHEST_HOTBAR + 8), named(STONE, 5, "a"));
        assert_eq!(fixture.at(CHEST_HOTBAR + 7), stack(STONE, 10));
        fixture.put(3, named(STONE, 7, "a"));
        fixture.menu.carried = stack(STONE, 1);
        fixture.survival(5, 0, ClickKind::PickupAll);
        assert_eq!(fixture.menu.carried, stack(STONE, 11));
        assert_eq!(fixture.at(3), named(STONE, 7, "a"));
        assert_eq!(fixture.at(CHEST_HOTBAR + 8), named(STONE, 5, "a"));
    }

    /// `Inventory.add` fills only slots of the same item with the same components.
    #[test]
    fn putting_back_fills_only_stacks_with_the_same_components() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.inventory.set(0, named(STONE, 60, "a"));
        fixture.inventory.set(1, stack(STONE, 60));
        fixture.menu.carried = named(STONE, 10, "a");
        assert_eq!(put_back(&mut fixture.inventory, 1, &mut fixture.menu), None);
        assert_eq!(fixture.inventory.get(1), Some(stack(STONE, 60)));
        assert_eq!(fixture.inventory.get(0), Some(named(STONE, 64, "a")));
        assert_eq!(fixture.inventory.get(2), Some(named(STONE, 6, "a")));
    }

    #[test]
    fn a_click_on_another_item_swaps_it_with_the_carried_stack() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(CHEST_HOTBAR, stack(DIRT, 3));
        fixture.menu.carried = stack(STONE, 10);
        fixture.survival(CHEST_HOTBAR as i16, 0, ClickKind::Pickup);
        assert_eq!(fixture.inventory.get(0), Some(stack(STONE, 10)));
        assert_eq!(fixture.menu.carried, stack(DIRT, 3));
    }

    #[test]
    fn a_click_outside_throws_the_carried_stack_with_the_left_button_and_one_with_the_right() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.menu.carried = stack(STONE, 5);
        let clicked = fixture.survival(-999, 1, ClickKind::Pickup);
        assert_eq!(clicked.thrown, vec![ItemStack::new(STONE, 1)]);
        assert_eq!(fixture.menu.carried, stack(STONE, 4));
        // A shift-click outside throws too.
        let clicked = fixture.survival(-999, 0, ClickKind::QuickMove);
        assert_eq!(clicked.thrown, vec![ItemStack::new(STONE, 4)]);
        assert_eq!(fixture.menu.carried, None);
        let clicked = fixture.survival(-999, 0, ClickKind::Pickup);
        assert_eq!(clicked, Clicked::default());
    }

    #[test]
    fn a_click_in_the_margin_does_nothing_and_is_not_refused() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.menu.carried = stack(STONE, 5);
        let before = fixture.snapshot();
        assert_eq!(
            fixture.survival(-1, 0, ClickKind::Pickup),
            Clicked::default()
        );
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_slot_past_the_menu_is_refused() {
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.menu.carried = stack(STONE, 5);
        let before = fixture.snapshot();
        assert!(fixture.survival(41, 0, ClickKind::Pickup).refused);
        assert!(fixture.survival(i16::MAX, 0, ClickKind::Throw).refused);
        assert_eq!(fixture.snapshot(), before);
        assert!(!fixture.survival(40, 0, ClickKind::Pickup).refused);
    }

    #[test]
    fn a_swap_or_a_drag_slot_at_a_negative_slot_is_refused_as_the_game_would_crash_on_it() {
        // Finding 25: `SWAP` and the drag's continue read `slots.get(slot)` unchecked.
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.inventory.set(0, stack(STONE, 5));
        let before = fixture.snapshot();
        assert!(fixture.survival(-1, 0, ClickKind::Swap).refused);
        assert!(fixture.survival(-999, 40, ClickKind::Swap).refused);
        assert!(fixture.survival(-5, 3, ClickKind::Swap).refused);
        assert_eq!(fixture.snapshot(), before);
        // A swap's button that names no slot is tested first: nothing, not a crash.
        assert!(!fixture.survival(-1, 9, ClickKind::Swap).refused);

        fixture.menu.carried = stack(STONE, 6);
        fixture.survival(-999, 0, ClickKind::QuickCraft);
        fixture.survival(0, 1, ClickKind::QuickCraft);
        let before = fixture.snapshot();
        assert!(fixture.survival(-999, 1, ClickKind::QuickCraft).refused);
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_menu_handed_with_a_container_of_another_size_is_refused() {
        let mut container = vec![None; 9];
        let mut inventory = Inventory::default();
        let mut menu = Fixture::new(MenuKind::Rows3).menu;
        let mut slots = MenuSlots {
            kind: MenuKind::Rows3,
            container: &mut container,
            inventory: &mut inventory,
            selected: 0,
            mount: None,
            merchant: None,
        };
        assert!(click(&mut slots, &mut menu, 0, 0, ClickKind::Pickup, false).refused);
    }

    #[test]
    fn a_quick_move_from_a_chest_goes_to_the_hotbar_from_its_right_end() {
        // ChestMenu.quickMoveStack moves a container's stack with
        // `moveItemStackTo(stack, 27, 63, true)`: the last slot (hotbar 8) first.
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(3, stack(STONE, 20));
        fixture.survival(3, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(3), None);
        assert_eq!(fixture.inventory.get(8), Some(stack(STONE, 20)));
        // The next goes to hotbar 7, then on to the main inventory's last row.
        fixture.put(4, stack(DIRT, 1));
        fixture.survival(4, 0, ClickKind::QuickMove);
        assert_eq!(fixture.inventory.get(7), Some(stack(DIRT, 1)));
        for slot in 0..7 {
            fixture.inventory.set(slot, stack(PEARL, 16));
        }
        fixture.put(5, stack(BOX, 1));
        fixture.survival(5, 0, ClickKind::QuickMove);
        assert_eq!(fixture.inventory.get(35), Some(stack(BOX, 1)));
    }

    #[test]
    fn a_quick_move_merges_into_stacks_of_the_item_before_it_takes_an_empty_slot() {
        // moveItemStackTo's first pass merges in its order (here last first) into every
        // stack of the item that has room (AbstractContainerMenu.txt 1940–2091, code
        // 16–190); only what is left goes into the first empty slot (193–328).
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.inventory.set(20, stack(STONE, 60));
        fixture.inventory.set(10, stack(STONE, 50));
        fixture.put(0, stack(STONE, 30));
        fixture.survival(0, 0, ClickKind::QuickMove);
        assert_eq!(fixture.inventory.get(20), Some(stack(STONE, 64)));
        assert_eq!(fixture.inventory.get(10), Some(stack(STONE, 64)));
        assert_eq!(fixture.inventory.get(8), Some(stack(STONE, 12)));
        assert_eq!(fixture.at(0), None);
    }

    #[test]
    fn a_quick_move_from_the_inventory_fills_the_chest_from_its_first_slot() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(CHEST_HOTBAR, stack(STONE, 10));
        fixture.survival(CHEST_HOTBAR as i16, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(0), stack(STONE, 10));
        // Into a chest partly filled: the stack of the item first, wherever it is.
        fixture.put(0, stack(DIRT, 1));
        fixture.put(12, stack(STONE, 60));
        fixture.put(CHEST_MAIN, stack(STONE, 10));
        fixture.survival(CHEST_MAIN as i16, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(12), stack(STONE, 64));
        assert_eq!(fixture.at(1), stack(STONE, 6));
        assert_eq!(fixture.at(CHEST_MAIN), None);
    }

    #[test]
    fn a_quick_move_into_a_full_container_moves_what_fits_and_leaves_the_rest() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        for index in 0..27 {
            fixture.put(index, stack(DIRT, 64));
        }
        fixture.put(CHEST_MAIN, stack(STONE, 10));
        let before = fixture.snapshot();
        assert_eq!(
            fixture.survival(CHEST_MAIN as i16, 0, ClickKind::QuickMove),
            Clicked::default()
        );
        assert_eq!(fixture.snapshot(), before);
        // One stack of stone with room for four: those four move, six stay.
        fixture.put(26, stack(STONE, 60));
        fixture.survival(CHEST_MAIN as i16, 1, ClickKind::QuickMove);
        assert_eq!(fixture.at(26), stack(STONE, 64));
        assert_eq!(fixture.at(CHEST_MAIN), stack(STONE, 6));
    }

    #[test]
    fn a_quick_move_works_both_ways_in_every_menu() {
        for kind in [
            MenuKind::Rows3,
            MenuKind::Rows6,
            MenuKind::ShulkerBox,
            MenuKind::Grid3x3,
            MenuKind::Hopper,
        ] {
            let mut fixture = Fixture::new(kind);
            let n = kind.container_slots();
            let last = kind.slots() - 1;
            fixture.put(n - 1, stack(STONE, 5));
            fixture.survival((n - 1) as i16, 0, ClickKind::QuickMove);
            assert_eq!(fixture.at(last), stack(STONE, 5), "{kind:?}");
            assert_eq!(fixture.inventory.get(8), Some(stack(STONE, 5)), "{kind:?}");
            fixture.survival(last as i16, 0, ClickKind::QuickMove);
            assert_eq!(fixture.at(0), stack(STONE, 5), "{kind:?}");
            assert_eq!(fixture.at(last), None, "{kind:?}");
        }
    }

    #[test]
    fn a_quick_move_into_a_shulker_box_skips_a_shulker_box() {
        let mut fixture = Fixture::new(MenuKind::ShulkerBox);
        fixture.put(27, stack(BOX, 1));
        let before = fixture.snapshot();
        fixture.survival(27, 0, ClickKind::QuickMove);
        assert_eq!(fixture.snapshot(), before);
        // A dispenser takes one.
        let mut fixture = Fixture::new(MenuKind::Grid3x3);
        fixture.put(9, stack(BOX, 1));
        fixture.survival(9, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(0), stack(BOX, 1));
    }

    #[test]
    fn a_shulker_box_slot_refuses_a_shulker_box_put_dragged_or_swapped_into_it() {
        let mut fixture = Fixture::new(MenuKind::ShulkerBox);
        fixture.menu.carried = stack(BOX, 1);
        let before = fixture.snapshot();
        fixture.survival(0, 0, ClickKind::Pickup);
        fixture.survival(0, 1, ClickKind::Pickup);
        fixture.inventory.set(0, stack(items::RED_SHULKER_BOX, 1));
        let before_swap = fixture.snapshot();
        fixture.survival(0, 0, ClickKind::Swap);
        assert_eq!(fixture.snapshot(), before_swap);
        fixture.inventory.set(0, None);
        assert_eq!(fixture.snapshot(), before);
        // The player's own slots in the menu take it.
        fixture.survival(27, 0, ClickKind::Pickup);
        assert_eq!(fixture.inventory.get(9), Some(stack(BOX, 1)));
        // A drag adds no shulker box slot: over 0 and 28, only 28 is added, and a drag
        // over one slot is a click on it.
        fixture.menu.carried = stack(BOX, 1);
        fixture.drag(0, &[0, 28], false);
        assert_eq!(fixture.at(0), None);
        assert_eq!(fixture.inventory.get(10), Some(stack(BOX, 1)));
        assert_eq!(fixture.menu.carried, None);
    }

    #[test]
    fn a_swap_moves_a_stack_between_a_slot_and_a_hotbar_slot_both_ways() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(4, stack(STONE, 9));
        fixture.survival(4, 2, ClickKind::Swap);
        assert_eq!(fixture.inventory.get(2), Some(stack(STONE, 9)));
        assert_eq!(fixture.at(4), None);
        fixture.survival(5, 2, ClickKind::Swap);
        assert_eq!(fixture.at(5), stack(STONE, 9));
        assert_eq!(fixture.inventory.get(2), Some(None));
        // Two stacks change places.
        fixture.inventory.set(2, stack(DIRT, 3));
        fixture.survival(5, 2, ClickKind::Swap);
        assert_eq!(fixture.at(5), stack(DIRT, 3));
        assert_eq!(fixture.inventory.get(2), Some(stack(STONE, 9)));
        // A hotbar slot swapped with itself stays as it is.
        let before = fixture.snapshot();
        fixture.survival((CHEST_HOTBAR + 2) as i16, 2, ClickKind::Swap);
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_swap_with_button_40_swaps_with_the_off_hand() {
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.inventory.set(OFF_HAND, stack(PEARL, 4));
        fixture.put(2, stack(STONE, 1));
        fixture.survival(2, 40, ClickKind::Swap);
        assert_eq!(fixture.inventory.get(OFF_HAND), Some(stack(STONE, 1)));
        assert_eq!(fixture.at(2), stack(PEARL, 4));
    }

    #[test]
    fn a_swap_with_a_button_that_names_no_slot_does_nothing() {
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.put(2, stack(STONE, 1));
        let before = fixture.snapshot();
        for button in [9, 39, 41, -1] {
            assert_eq!(
                fixture.survival(2, button, ClickKind::Swap),
                Clicked::default()
            );
        }
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_clone_gives_a_creative_player_a_full_stack_and_nobody_else_anything() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(0, stack(PEARL, 1));
        let before = fixture.snapshot();
        assert_eq!(fixture.survival(0, 2, ClickKind::Clone), Clicked::default());
        assert_eq!(fixture.snapshot(), before);
        fixture.click(0, 2, ClickKind::Clone, true);
        assert_eq!(fixture.menu.carried, stack(PEARL, 16));
        assert_eq!(fixture.at(0), stack(PEARL, 1));
        // Not with something carried, nor of an empty slot.
        fixture.menu.carried = stack(DIRT, 1);
        fixture.click(0, 2, ClickKind::Clone, true);
        assert_eq!(fixture.menu.carried, stack(DIRT, 1));
        fixture.menu.carried = None;
        fixture.click(1, 2, ClickKind::Clone, true);
        assert_eq!(fixture.menu.carried, None);
    }

    #[test]
    fn a_throw_throws_one_with_button_0_and_the_stack_with_button_1() {
        let mut fixture = Fixture::new(MenuKind::Grid3x3);
        fixture.put(0, stack(STONE, 5));
        let clicked = fixture.survival(0, 0, ClickKind::Throw);
        assert_eq!(clicked.thrown, vec![ItemStack::new(STONE, 1)]);
        assert_eq!(fixture.at(0), stack(STONE, 4));
        let clicked = fixture.survival(0, 1, ClickKind::Throw);
        assert_eq!(clicked.thrown, vec![ItemStack::new(STONE, 4)]);
        assert_eq!(fixture.at(0), None);
        // Nothing of an empty slot, and nothing while a stack is carried.
        assert_eq!(fixture.survival(0, 1, ClickKind::Throw), Clicked::default());
        fixture.put(1, stack(STONE, 5));
        fixture.menu.carried = stack(DIRT, 1);
        assert_eq!(fixture.survival(1, 1, ClickKind::Throw), Clicked::default());
        assert_eq!(fixture.at(1), stack(STONE, 5));
    }

    #[test]
    fn a_pickup_all_gathers_partial_stacks_first_then_full_ones_in_slot_order() {
        // AbstractContainerMenu.txt code 1671–1871: pass 0 skips full stacks, pass 1
        // takes them; from the first slot with button 0, the container before the
        // inventory, until the cursor is full.
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.put(0, stack(STONE, 64));
        fixture.put(1, stack(STONE, 10));
        fixture.put(3, stack(DIRT, 10));
        fixture.put(5, stack(STONE, 20)); // inventory 9
        fixture.put(32, stack(STONE, 64)); // hotbar 0
        fixture.menu.carried = stack(STONE, 30);
        fixture.survival(2, 0, ClickKind::PickupAll);
        assert_eq!(fixture.menu.carried, stack(STONE, 64));
        assert_eq!(fixture.at(1), None);
        assert_eq!(fixture.at(5), None);
        // 30 + 10 + 20 = 60; the last 4 from the first full stack, slot 0.
        assert_eq!(fixture.at(0), stack(STONE, 60));
        assert_eq!(fixture.at(32), stack(STONE, 64));
        assert_eq!(fixture.at(3), stack(DIRT, 10));
    }

    #[test]
    fn a_pickup_all_with_button_1_gathers_from_the_last_slot() {
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.put(0, stack(STONE, 10));
        fixture.put(40, stack(STONE, 10)); // hotbar 8
        fixture.menu.carried = stack(STONE, 59);
        fixture.survival(2, 1, ClickKind::PickupAll);
        assert_eq!(fixture.menu.carried, stack(STONE, 64));
        assert_eq!(fixture.at(40), stack(STONE, 5));
        assert_eq!(fixture.at(0), stack(STONE, 10));
    }

    #[test]
    fn a_pickup_all_on_a_slot_that_holds_something_does_nothing() {
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.put(0, stack(STONE, 10));
        fixture.put(1, stack(DIRT, 1));
        fixture.menu.carried = stack(STONE, 1);
        let before = fixture.snapshot();
        fixture.survival(1, 0, ClickKind::PickupAll);
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_split_drag_shares_evenly_over_the_slots_that_can_take_it_and_keeps_the_rest() {
        // The drag's end (AbstractContainerMenu.txt code 298–530): each slot added gets
        // floor(count / slots) on top of what it holds, up to the item's most, and the
        // carried stack keeps what is left. A slot of another item is never added
        // (canItemQuickReplace, code 147–219).
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(1, stack(STONE, 60));
        fixture.put(2, stack(DIRT, 1));
        fixture.menu.carried = stack(STONE, 64);
        fixture.drag(0, &[0, 1, 2, 3], false);
        // Three slots added: 64 / 3 = 21 each, slot 1 only up to 64.
        assert_eq!(fixture.at(0), stack(STONE, 21));
        assert_eq!(fixture.at(1), stack(STONE, 64));
        assert_eq!(fixture.at(2), stack(DIRT, 1));
        assert_eq!(fixture.at(3), stack(STONE, 21));
        assert_eq!(fixture.menu.carried, stack(STONE, 64 - 21 - 4 - 21));
        assert_eq!(fixture.menu.drag, None);
    }

    #[test]
    fn a_split_drag_adds_no_more_slots_than_items_carried() {
        // A slot is added only while the count is above the slots so far (code 174–196).
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.menu.carried = stack(STONE, 2);
        fixture.drag(0, &[0, 1, 2], false);
        assert_eq!(fixture.at(0), stack(STONE, 1));
        assert_eq!(fixture.at(1), stack(STONE, 1));
        assert_eq!(fixture.at(2), None);
        assert_eq!(fixture.menu.carried, None);
    }

    #[test]
    fn a_right_drag_puts_one_into_each_slot() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(CHEST_HOTBAR, stack(STONE, 63));
        fixture.put(CHEST_MAIN, stack(STONE, 64));
        fixture.menu.carried = stack(STONE, 10);
        // The full stack in the main inventory takes nothing more but is added; the
        // dirt slot is not added.
        fixture.put(1, stack(DIRT, 4));
        fixture.drag(1, &[0, 1, CHEST_MAIN as i16, CHEST_HOTBAR as i16, 5], false);
        assert_eq!(fixture.at(0), stack(STONE, 1));
        assert_eq!(fixture.at(1), stack(DIRT, 4));
        assert_eq!(fixture.at(CHEST_MAIN), stack(STONE, 64));
        assert_eq!(fixture.at(CHEST_HOTBAR), stack(STONE, 64));
        assert_eq!(fixture.at(5), stack(STONE, 1));
        assert_eq!(fixture.menu.carried, stack(STONE, 7));
    }

    #[test]
    fn a_clone_drag_fills_every_slot_for_a_creative_player_and_empties_the_cursor() {
        // Type 2 puts a full stack into each slot; `left` falls below nothing and the
        // carried stack is set to that count, which is empty (code 467–530).
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(1, stack(PEARL, 3));
        fixture.put(3, stack(DIRT, 3));
        fixture.menu.carried = stack(PEARL, 1);
        fixture.drag(2, &[0, 1, 2, 3], true);
        assert_eq!(fixture.at(0), stack(PEARL, 16));
        assert_eq!(fixture.at(1), stack(PEARL, 16));
        assert_eq!(fixture.at(2), stack(PEARL, 16));
        assert_eq!(fixture.at(3), stack(DIRT, 3));
        assert_eq!(fixture.menu.carried, None);
    }

    #[test]
    fn a_throw_with_any_button_but_0_throws_the_stack() {
        // The count is 1 for button 0 and the slot's count for any other (code
        // 1481–1497); only button 1 throws again while the slot gives more.
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.put(0, stack(STONE, 5));
        let clicked = fixture.survival(0, 7, ClickKind::Throw);
        assert_eq!(clicked.thrown, vec![ItemStack::new(STONE, 5)]);
        assert_eq!(fixture.at(0), None);
    }

    #[test]
    fn a_pickup_all_with_any_button_but_0_gathers_from_the_last_slot() {
        // The start is the first slot for button 0 and the last for any other (code
        // 1671–1699).
        let mut fixture = Fixture::new(MenuKind::Hopper);
        fixture.put(0, stack(STONE, 10));
        fixture.put(40, stack(STONE, 10));
        fixture.menu.carried = stack(STONE, 59);
        fixture.survival(2, 3, ClickKind::PickupAll);
        assert_eq!(fixture.at(40), stack(STONE, 5));
        assert_eq!(fixture.at(0), stack(STONE, 10));
    }

    #[test]
    fn a_swap_of_a_stack_larger_than_the_slot_takes_puts_the_slots_stack_back_or_throws_it() {
        // SWAP's branch for a held stack above the slot's most (code 1291–1353): what
        // the slot held goes into the inventory (`Inventory.add`) or out of the window.
        // No stack of N6 is above its item's most, so the test makes one.
        let mut fixture = Fixture::new(MenuKind::Hopper);
        for slot in 0..36 {
            fixture.inventory.set(slot, stack(DIRT, 64));
        }
        fixture.inventory.set(0, stack(PEARL, 20));
        fixture.put(1, stack(STONE, 3));
        let clicked = fixture.survival(1, 0, ClickKind::Swap);
        assert_eq!(fixture.at(1), stack(PEARL, 16));
        assert_eq!(fixture.inventory.get(0), Some(stack(PEARL, 4)));
        assert_eq!(clicked.thrown, vec![ItemStack::new(STONE, 3)]);
        // A creative player's stack that does not fit back is gone.
        fixture.inventory.set(0, stack(PEARL, 20));
        fixture.put(1, stack(STONE, 3));
        let clicked = fixture.click(1, 0, ClickKind::Swap, true);
        assert_eq!(clicked, Clicked::default());
        assert_eq!(fixture.at(1), stack(PEARL, 16));
    }

    #[test]
    fn a_clone_drag_is_not_started_for_a_player_who_is_not_creative() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.menu.carried = stack(PEARL, 4);
        fixture.survival(-999, 8, ClickKind::QuickCraft);
        assert_eq!(fixture.menu.drag, None);
        let before = fixture.snapshot();
        fixture.drag(2, &[0, 1], false);
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_drag_over_one_slot_is_a_click_on_it() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.menu.carried = stack(STONE, 5);
        fixture.drag(0, &[3], false);
        assert_eq!(fixture.at(3), stack(STONE, 5));
        assert_eq!(fixture.menu.carried, None);
        fixture.menu.carried = stack(STONE, 5);
        fixture.drag(1, &[4], false);
        assert_eq!(fixture.at(4), stack(STONE, 1));
        // A middle drag over one slot is a pickup of button 2, which does nothing.
        fixture.drag(2, &[5], true);
        assert_eq!(fixture.at(5), None);
        assert_eq!(fixture.menu.carried, stack(STONE, 4));
    }

    #[test]
    fn a_drag_is_ended_by_any_other_click_and_a_second_start_and_does_nothing_more() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.put(9, stack(DIRT, 1));
        fixture.menu.carried = stack(STONE, 6);
        fixture.survival(-999, 0, ClickKind::QuickCraft);
        fixture.survival(0, 1, ClickKind::QuickCraft);
        assert_eq!(
            fixture.menu.drag,
            Some(Drag {
                kind: DragKind::Split,
                slots: vec![0]
            })
        );
        // The pickup only ends the drag: slot 9 is not picked up.
        fixture.survival(9, 0, ClickKind::Pickup);
        assert_eq!(fixture.menu.drag, None);
        assert_eq!(fixture.at(9), stack(DIRT, 1));
        // An end with no drag under way does nothing.
        let before = fixture.snapshot();
        fixture.survival(-999, 2, ClickKind::QuickCraft);
        assert_eq!(fixture.snapshot(), before);
        // A start in the middle of a drag ends it.
        fixture.survival(-999, 0, ClickKind::QuickCraft);
        fixture.survival(1, 1, ClickKind::QuickCraft);
        fixture.survival(-999, 4, ClickKind::QuickCraft);
        assert_eq!(fixture.menu.drag, None);
        fixture.survival(-999, 2, ClickKind::QuickCraft);
        assert_eq!(fixture.at(1), None);
        assert_eq!(fixture.menu.carried, stack(STONE, 6));
    }

    #[test]
    fn a_drag_with_nothing_carried_does_not_start() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.survival(-999, 0, ClickKind::QuickCraft);
        assert_eq!(fixture.menu.drag, None);
    }

    #[test]
    fn a_drag_adds_a_slot_once() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.menu.carried = stack(STONE, 6);
        fixture.drag(0, &[0, 1, 0, 1], false);
        assert_eq!(fixture.at(0), stack(STONE, 3));
        assert_eq!(fixture.at(1), stack(STONE, 3));
    }

    #[test]
    fn putting_back_fills_the_selected_slot_then_the_off_hand_then_the_inventory_in_order() {
        // getSlotWithRemainingSpace (Inventory.txt 494–538): the selected slot, then
        // 40, then 0 to 35; then the first empty slot (getFreeSlot, 124–146).
        let mut fixture = Fixture::new(MenuKind::Rows3);
        fixture.inventory.set(3, stack(STONE, 60));
        fixture.inventory.set(OFF_HAND, stack(STONE, 62));
        fixture.inventory.set(20, stack(STONE, 63));
        fixture.inventory.set(0, stack(DIRT, 1));
        fixture.menu.carried = stack(STONE, 10);
        let left = put_back(&mut fixture.inventory, 3, &mut fixture.menu);
        assert_eq!(left, None);
        assert_eq!(fixture.menu.carried, None);
        assert_eq!(fixture.inventory.get(3), Some(stack(STONE, 64)));
        assert_eq!(fixture.inventory.get(OFF_HAND), Some(stack(STONE, 64)));
        assert_eq!(fixture.inventory.get(20), Some(stack(STONE, 64)));
        assert_eq!(fixture.inventory.get(1), Some(stack(STONE, 3)));
    }

    #[test]
    fn putting_back_into_a_full_inventory_returns_the_stack_to_drop() {
        let mut fixture = Fixture::new(MenuKind::Rows3);
        for slot in 0..36 {
            fixture.inventory.set(slot, stack(DIRT, 64));
        }
        fixture.inventory.set(7, stack(STONE, 60));
        fixture.menu.carried = stack(STONE, 10);
        let left = put_back(&mut fixture.inventory, 0, &mut fixture.menu);
        assert_eq!(left, stack(STONE, 6));
        assert_eq!(fixture.inventory.get(7), Some(stack(STONE, 64)));
        assert_eq!(fixture.menu.carried, None);
        // An empty off hand is not chosen.
        assert_eq!(fixture.inventory.get(OFF_HAND), Some(None));
        assert_eq!(put_back(&mut fixture.inventory, 0, &mut fixture.menu), None);
    }

    #[test]
    fn an_inventory_add_that_does_not_fit_is_deleted_for_a_creative_player_only() {
        // Inventory.add(-1, stack), Inventory.txt 581–714: a round that adds nothing
        // ends the loop; a creative player's rest is set to none and counts as added.
        let mut inventory = Inventory::default();
        for slot in 0..36 {
            inventory.set(slot, stack(DIRT, 64));
        }
        inventory.set(5, stack(STONE, 62));
        let mut rest = ItemStack::new(STONE, 5);
        assert!(!add(&mut inventory, 0, None, &mut rest, false));
        assert_eq!(rest.count, 3);
        assert_eq!(inventory.get(5), Some(stack(STONE, 64)));
        assert!(add(&mut inventory, 0, None, &mut rest, true));
        assert_eq!(rest.count, 0);
    }

    // The inventory screen (window 0): 0 the result, 1-4 the grid, 5-8 the armour from
    // the head down, 9-35 the main inventory, 36-44 the hotbar, 45 the off hand.
    const HELMET: i32 = items::IRON_HELMET;
    const CHESTPLATE: i32 = items::DIAMOND_CHESTPLATE;
    const BOOTS: i32 = items::IRON_BOOTS;
    const SHIELD: i32 = items::SHIELD;
    const TORCH: i32 = items::TORCH;

    /// A stack of `item` with Curse of Binding.
    fn cursed(item: i32) -> Option<ItemStack> {
        let mut stack = ItemStack::new(item, 1);
        stack
            .components
            .set(clustine_world::Component::Enchantments(vec![(
                "minecraft:binding_curse".to_owned(),
                1,
            )]));
        Some(stack)
    }

    #[test]
    fn the_inventory_screen_lays_out_its_own_slots_the_armour_the_inventory_and_the_off_hand() {
        let kind = MenuKind::Inventory;
        for index in 0..5 {
            assert_eq!(inventory_slot_of(kind, index), None);
        }
        assert_eq!(inventory_slot_of(kind, 5), Some(39));
        assert_eq!(inventory_slot_of(kind, 6), Some(38));
        assert_eq!(inventory_slot_of(kind, 7), Some(37));
        assert_eq!(inventory_slot_of(kind, 8), Some(36));
        assert_eq!(inventory_slot_of(kind, 9), Some(9));
        assert_eq!(inventory_slot_of(kind, 35), Some(35));
        assert_eq!(inventory_slot_of(kind, 36), Some(0));
        assert_eq!(inventory_slot_of(kind, 44), Some(8));
        assert_eq!(inventory_slot_of(kind, 45), Some(OFF_HAND));
        assert_eq!(inventory_slot_of(kind, 46), None);
        assert_eq!(menu_slot_of(kind, 0), Some(36));
        assert_eq!(menu_slot_of(kind, 8), Some(44));
        assert_eq!(menu_slot_of(kind, 9), Some(9));
        assert_eq!(menu_slot_of(kind, 35), Some(35));
        assert_eq!(menu_slot_of(kind, 36), Some(8));
        assert_eq!(menu_slot_of(kind, 39), Some(5));
        assert_eq!(menu_slot_of(kind, 40), Some(45));
        assert_eq!(menu_slot_of(kind, 41), None);
        // Every inventory slot is shown once, and the two maps are each other's inverse.
        for slot in 0..INVENTORY_SLOTS {
            let index = menu_slot_of(kind, slot).expect("the screen shows every slot");
            assert_eq!(inventory_slot_of(kind, index), Some(slot));
        }
        // The other menus show no armour and no off hand.
        assert_eq!(menu_slot_of(MenuKind::Rows3, 36), None);
        assert_eq!(menu_slot_of(MenuKind::Rows3, 40), None);
        assert_eq!(menu_slot_of(MenuKind::Rows3, 0), Some(54));
    }

    #[test]
    fn a_menu_slot_of_the_inventory_screen_reads_the_grid_and_the_inventory() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.container[2] = stack(STONE, 3);
        fixture.inventory.set(39, stack(HELMET, 1));
        fixture.inventory.set(OFF_HAND, stack(SHIELD, 1));
        assert_eq!(fixture.at(2), stack(STONE, 3));
        assert_eq!(fixture.at(5), stack(HELMET, 1));
        assert_eq!(fixture.at(45), stack(SHIELD, 1));
        assert_eq!(fixture.at(0), None);
    }

    #[test]
    fn a_helmet_is_put_on_by_two_clicks_and_its_slot_reported() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(36, stack(HELMET, 1));
        let taken = fixture.survival(36, 0, ClickKind::Pickup);
        assert_eq!(taken.equipped, Vec::<usize>::new());
        assert_eq!(fixture.menu.carried, stack(HELMET, 1));
        let put = fixture.survival(5, 0, ClickKind::Pickup);
        assert!(!put.refused);
        assert_eq!(put.equipped, vec![5]);
        assert_eq!(fixture.inventory.get(39), Some(stack(HELMET, 1)));
        assert_eq!(fixture.menu.carried, None);
    }

    #[test]
    fn a_quick_move_from_the_hotbar_puts_a_helmet_on() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(36, stack(HELMET, 1));
        let clicked = fixture.survival(36, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(5), stack(HELMET, 1));
        assert_eq!(fixture.at(36), None);
        assert_eq!(clicked.equipped, vec![5]);
    }

    #[test]
    fn a_quick_move_of_boots_puts_them_on_the_feet() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(20, stack(BOOTS, 1));
        fixture.survival(20, 0, ClickKind::QuickMove);
        assert_eq!(fixture.inventory.get(36), Some(stack(BOOTS, 1)));
    }

    #[test]
    fn a_quick_move_of_a_chestplate_when_the_chest_is_full_goes_to_the_main_inventory() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(6, named(CHESTPLATE, 1, "worn"));
        fixture.put(36, stack(CHESTPLATE, 1));
        let clicked = fixture.survival(36, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(6), named(CHESTPLATE, 1, "worn"));
        assert_eq!(fixture.at(9), stack(CHESTPLATE, 1));
        assert_eq!(fixture.at(36), None);
        assert_eq!(clicked.equipped, Vec::<usize>::new());
    }

    #[test]
    fn a_quick_move_takes_armour_off_into_the_main_inventory_then_the_hotbar() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(5, stack(HELMET, 1));
        for index in 9..36 {
            fixture.put(index, stack(DIRT, 64));
        }
        let clicked = fixture.survival(5, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(5), None);
        assert_eq!(fixture.at(36), stack(HELMET, 1));
        assert_eq!(clicked.equipped, vec![5]);
    }

    #[test]
    fn a_shield_quick_moves_into_an_empty_off_hand_and_else_between_the_inventories() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(9, stack(SHIELD, 1));
        let clicked = fixture.survival(9, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(45), stack(SHIELD, 1));
        assert_eq!(fixture.at(9), None);
        assert_eq!(clicked.equipped, vec![45]);
        // With the off hand full, a second shield goes from the main inventory to the
        // hotbar.
        fixture.put(10, stack(SHIELD, 1));
        let clicked = fixture.survival(10, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(36), stack(SHIELD, 1));
        assert_eq!(clicked.equipped, Vec::<usize>::new());
        // And the off hand's own goes to the main inventory first.
        let clicked = fixture.survival(45, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(9), stack(SHIELD, 1));
        assert_eq!(fixture.at(45), None);
        assert_eq!(clicked.equipped, vec![45]);
    }

    #[test]
    fn an_armour_slot_refuses_what_is_not_worn_there() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.menu.carried = stack(HELMET, 1);
        let clicked = fixture.survival(6, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(6), None);
        assert_eq!(fixture.menu.carried, stack(HELMET, 1));
        assert_eq!(clicked.equipped, Vec::<usize>::new());
        fixture.menu.carried = stack(STONE, 5);
        fixture.survival(5, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(5), None);
        // Nor by a swap from the hotbar, nor by a drag.
        fixture.put(36, stack(STONE, 1));
        fixture.survival(5, 0, ClickKind::Swap);
        assert_eq!(fixture.at(5), None);
        assert_eq!(fixture.at(36), stack(STONE, 1));
        fixture.drag(0, &[5, 6, 10, 11], false);
        assert_eq!(fixture.at(5), None);
        assert_eq!(fixture.at(6), None);
        assert_eq!(fixture.at(10), stack(STONE, 2));
        assert_eq!(fixture.at(11), stack(STONE, 2));
        assert_eq!(fixture.menu.carried, stack(STONE, 1));
    }

    #[test]
    fn an_armour_slot_holds_one_however_many_are_carried() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        // A carved pumpkin is worn on the head and stacks to 64.
        fixture.menu.carried = stack(items::CARVED_PUMPKIN, 10);
        let clicked = fixture.survival(5, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(5), stack(items::CARVED_PUMPKIN, 1));
        assert_eq!(fixture.menu.carried, stack(items::CARVED_PUMPKIN, 9));
        assert_eq!(clicked.equipped, vec![5]);
        // Another of the same changes nothing.
        let clicked = fixture.survival(5, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(5), stack(items::CARVED_PUMPKIN, 1));
        assert_eq!(fixture.menu.carried, stack(items::CARVED_PUMPKIN, 9));
        assert_eq!(clicked.equipped, Vec::<usize>::new());
    }

    #[test]
    fn a_helmet_with_curse_of_binding_stays_on_for_a_survival_player() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(5, cursed(HELMET));
        fixture.put(36, stack(HELMET, 1));
        for kind in [ClickKind::Pickup, ClickKind::QuickMove, ClickKind::Throw] {
            let clicked = fixture.survival(5, 0, kind);
            assert!(!clicked.refused);
            assert_eq!(clicked.equipped, Vec::<usize>::new());
            assert_eq!(fixture.at(5), cursed(HELMET));
        }
        fixture.survival(5, 1, ClickKind::Throw);
        // A swap with a hotbar slot that holds another helmet, or that is empty.
        fixture.survival(5, 0, ClickKind::Swap);
        fixture.survival(5, 1, ClickKind::Swap);
        assert_eq!(fixture.at(5), cursed(HELMET));
        assert_eq!(fixture.at(36), stack(HELMET, 1));
        assert_eq!(fixture.at(37), None);
        assert_eq!(fixture.menu.carried, None);
        // A helmet carried onto it does not swap.
        fixture.menu.carried = stack(HELMET, 1);
        fixture.survival(5, 0, ClickKind::Pickup);
        assert_eq!(fixture.at(5), cursed(HELMET));
        assert_eq!(fixture.menu.carried, stack(HELMET, 1));
    }

    #[test]
    fn a_helmet_with_curse_of_binding_is_taken_off_by_a_creative_player() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(5, cursed(HELMET));
        let clicked = fixture.click(5, 0, ClickKind::Pickup, true);
        assert_eq!(fixture.at(5), None);
        assert_eq!(fixture.menu.carried, cursed(HELMET));
        assert_eq!(clicked.equipped, vec![5]);
    }

    #[test]
    fn a_quick_move_from_the_grid_goes_to_the_main_inventory_first_and_never_the_off_hand() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.container[1] = stack(STONE, 10);
        fixture.survival(1, 0, ClickKind::QuickMove);
        assert_eq!(fixture.container[1], None);
        assert_eq!(fixture.at(9), stack(STONE, 10));
        // With the inventory full, nothing moves, not even into an off hand of the item
        // with room (`moveItemStackTo(9, 45)`: the end is not taken).
        for index in 9..45 {
            fixture.put(index, stack(DIRT, 64));
        }
        fixture.put(45, stack(STONE, 1));
        fixture.container[4] = stack(STONE, 10);
        fixture.survival(4, 0, ClickKind::QuickMove);
        assert_eq!(fixture.container[4], stack(STONE, 10));
        assert_eq!(fixture.at(45), stack(STONE, 1));
    }

    #[test]
    fn a_quick_move_goes_from_the_main_inventory_to_the_hotbar_and_back() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(12, stack(STONE, 20));
        fixture.survival(12, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(12), None);
        assert_eq!(fixture.at(36), stack(STONE, 20));
        fixture.survival(36, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(36), None);
        assert_eq!(fixture.at(9), stack(STONE, 20));
        // Stone in the off hand goes to the main inventory, merging first.
        fixture.put(45, stack(STONE, 5));
        let clicked = fixture.survival(45, 0, ClickKind::QuickMove);
        assert_eq!(fixture.at(45), None);
        assert_eq!(fixture.at(9), stack(STONE, 25));
        assert_eq!(clicked.equipped, vec![45]);
        // A full hotbar keeps the main inventory's stack where it is.
        for index in 36..45 {
            fixture.put(index, stack(DIRT, 64));
        }
        let before = fixture.snapshot();
        fixture.survival(9, 0, ClickKind::QuickMove);
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn nothing_is_placed_in_the_result_and_an_empty_result_gives_nothing() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.menu.carried = stack(DIRT, 10);
        for button in [0, 1] {
            let clicked = fixture.survival(0, button, ClickKind::Pickup);
            assert!(!clicked.refused);
        }
        assert_eq!(fixture.container[0], None);
        assert_eq!(fixture.menu.carried, stack(DIRT, 10));
        // A drag passes over it.
        fixture.drag(0, &[0, 1, 2], false);
        assert_eq!(fixture.container[0], None);
        assert_eq!(fixture.container[1], stack(DIRT, 5));
        assert_eq!(fixture.container[2], stack(DIRT, 5));
        assert_eq!(fixture.menu.carried, None);
        // A swap from the hotbar puts nothing into it.
        fixture.put(36, stack(DIRT, 1));
        fixture.survival(0, 0, ClickKind::Swap);
        assert_eq!(fixture.container[0], None);
        assert_eq!(fixture.at(36), stack(DIRT, 1));
        // Every way of taking from it while it is empty does nothing.
        let before = fixture.snapshot();
        for (button, kind) in [
            (0, ClickKind::Pickup),
            (1, ClickKind::Pickup),
            (0, ClickKind::QuickMove),
            (1, ClickKind::Throw),
            (2, ClickKind::Clone),
        ] {
            let clicked = fixture.click(0, button, kind, true);
            assert!(clicked.thrown.is_empty());
        }
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_pickup_all_gathers_from_the_inventory_screen_but_never_its_result() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.container[3] = stack(STONE, 2);
        fixture.settle();
        fixture.put(45, stack(STONE, 3));
        fixture.put(20, stack(STONE, 7));
        fixture.menu.carried = stack(STONE, 1);
        fixture.survival(10, 0, ClickKind::PickupAll);
        assert_eq!(fixture.menu.carried, stack(STONE, 13));
        assert_eq!(fixture.container[3], None);
        assert_eq!(fixture.at(45), None);
        assert_eq!(fixture.at(20), None);
        // The grid emptied by the gathering makes nothing.
        assert_eq!(fixture.container[0], None);
    }

    #[test]
    fn a_result_is_taken_whole_and_not_merged_into_a_cursor_it_overfills() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.container[0] = stack(STONE, 4);
        // A right click takes it all (`ResultContainer.removeItem`).
        fixture.survival(0, 1, ClickKind::Pickup);
        assert_eq!(fixture.menu.carried, stack(STONE, 4));
        assert_eq!(fixture.container[0], None);
        // Onto a cursor of the same item it goes only if all of it fits.
        fixture.container[0] = stack(STONE, 4);
        fixture.menu.carried = stack(STONE, 62);
        fixture.survival(0, 0, ClickKind::Pickup);
        assert_eq!(fixture.container[0], stack(STONE, 4));
        assert_eq!(fixture.menu.carried, stack(STONE, 62));
        fixture.menu.carried = stack(STONE, 60);
        fixture.survival(0, 0, ClickKind::Pickup);
        assert_eq!(fixture.container[0], None);
        assert_eq!(fixture.menu.carried, stack(STONE, 64));
    }

    #[test]
    fn a_swap_puts_a_helmet_on_from_the_hotbar_and_from_the_off_hand() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.inventory.set(2, stack(HELMET, 1));
        let clicked = fixture.survival(5, 2, ClickKind::Swap);
        assert_eq!(fixture.at(5), stack(HELMET, 1));
        assert_eq!(fixture.inventory.get(2), Some(None));
        assert_eq!(clicked.equipped, vec![5]);
        // Swapped back to the hotbar: the helmet comes off.
        let clicked = fixture.survival(5, 2, ClickKind::Swap);
        assert_eq!(fixture.at(5), None);
        assert_eq!(fixture.inventory.get(2), Some(stack(HELMET, 1)));
        assert_eq!(clicked.equipped, vec![5]);
        // Button 40 swaps with the off hand, which the game writes through the
        // inventory, not its slot: only the armour slot is reported.
        fixture.inventory.set(2, None);
        fixture.inventory.set(OFF_HAND, stack(HELMET, 1));
        let clicked = fixture.survival(5, 40, ClickKind::Swap);
        assert_eq!(fixture.at(5), stack(HELMET, 1));
        assert_eq!(fixture.at(45), None);
        assert_eq!(clicked.equipped, vec![5]);
        // A helmet held swaps with the one worn, each into the other's place.
        fixture.inventory.set(OFF_HAND, named(HELMET, 1, "spare"));
        let clicked = fixture.survival(5, 40, ClickKind::Swap);
        assert_eq!(fixture.at(5), named(HELMET, 1, "spare"));
        assert_eq!(fixture.at(45), stack(HELMET, 1));
        assert_eq!(clicked.equipped, vec![5]);
        // And a chestplate held does not go onto the head.
        fixture.inventory.set(OFF_HAND, stack(CHESTPLATE, 1));
        let before = fixture.snapshot();
        let clicked = fixture.survival(5, 40, ClickKind::Swap);
        assert_eq!(fixture.snapshot(), before);
        assert_eq!(clicked.equipped, Vec::<usize>::new());
    }

    #[test]
    fn a_swap_into_the_off_hand_slot_from_the_hotbar_is_reported() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.inventory.set(0, stack(SHIELD, 1));
        let clicked = fixture.survival(45, 0, ClickKind::Swap);
        assert_eq!(fixture.at(45), stack(SHIELD, 1));
        assert_eq!(clicked.equipped, vec![45]);
    }

    #[test]
    fn equipped_reports_a_helmet_put_on_and_taken_off_and_not_a_click_that_changes_nothing() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.menu.carried = stack(HELMET, 1);
        assert_eq!(fixture.survival(5, 0, ClickKind::Pickup).equipped, vec![5]);
        assert_eq!(fixture.survival(5, 0, ClickKind::Pickup).equipped, vec![5]);
        assert_eq!(fixture.menu.carried, stack(HELMET, 1));
        assert_eq!(fixture.at(5), None);
        // An empty slot clicked with nothing carried, and an armour slot clicked with
        // what it refuses.
        fixture.menu.carried = None;
        assert!(
            fixture
                .survival(5, 0, ClickKind::Pickup)
                .equipped
                .is_empty()
        );
        fixture.menu.carried = stack(STONE, 1);
        assert!(
            fixture
                .survival(5, 0, ClickKind::Pickup)
                .equipped
                .is_empty()
        );
        // Half of the off hand's torches taken, and some added: the same item stays.
        fixture.menu.carried = None;
        fixture.put(45, stack(TORCH, 10));
        assert!(
            fixture
                .survival(45, 1, ClickKind::Pickup)
                .equipped
                .is_empty()
        );
        assert_eq!(fixture.menu.carried, stack(TORCH, 5));
        assert!(
            fixture
                .survival(45, 0, ClickKind::Pickup)
                .equipped
                .is_empty()
        );
        assert_eq!(fixture.at(45), stack(TORCH, 10));
        // All of them taken: the off hand is emptied.
        assert_eq!(
            fixture.survival(45, 0, ClickKind::Pickup).equipped,
            vec![45]
        );
        // A throw of the last item empties it too.
        fixture.menu.carried = None;
        fixture.put(45, stack(TORCH, 1));
        let clicked = fixture.survival(45, 0, ClickKind::Throw);
        assert_eq!(clicked.thrown, vec![ItemStack::new(TORCH, 1)]);
        assert_eq!(clicked.equipped, vec![45]);
        // No other menu reports anything.
        let mut chest = Fixture::new(MenuKind::Rows3);
        chest.put(0, stack(HELMET, 1));
        assert!(chest.survival(0, 0, ClickKind::Pickup).equipped.is_empty());
    }

    #[test]
    fn a_drag_over_the_armour_and_the_off_hand_reports_each_slot_it_fills() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.menu.carried = stack(items::CARVED_PUMPKIN, 4);
        let clicked = fixture.drag(1, &[5, 45, 6], false);
        assert_eq!(fixture.at(5), stack(items::CARVED_PUMPKIN, 1));
        assert_eq!(fixture.at(45), stack(items::CARVED_PUMPKIN, 1));
        assert_eq!(fixture.at(6), None);
        assert_eq!(fixture.menu.carried, stack(items::CARVED_PUMPKIN, 2));
        let mut equipped = clicked.equipped;
        // The game's drag goes over a `HashSet`, whose order is not the client's.
        equipped.sort_unstable();
        assert_eq!(equipped, vec![5, 45]);
    }

    // The crafting grids (V3c): the inventory screen's 2×2 (result 0, grid 1-4) and the
    // crafting table's 3×3 (result 0, grid 1-9, then main 10-36 and the hotbar 37-45).

    const LOG: i32 = items::OAK_LOG;
    const PLANKS: i32 = items::OAK_PLANKS;
    const GRIDS: [MenuKind; 2] = [MenuKind::Inventory, MenuKind::Crafting];

    /// The menu slot of the grid's cell in `row` and `column`.
    fn cell(kind: MenuKind, row: usize, column: usize) -> usize {
        let side = if kind == MenuKind::Crafting { 3 } else { 2 };
        1 + row * side + column
    }

    /// The menu slot that shows the inventory slot `slot`.
    fn shown(kind: MenuKind, slot: usize) -> usize {
        menu_slot_of(kind, slot).expect("the menu shows the slot")
    }

    /// A grid menu of `kind` with `count` logs in its first cell, put there by a click.
    fn logs(kind: MenuKind, count: i32) -> Fixture {
        let mut fixture = Fixture::new(kind);
        fixture.menu.carried = stack(LOG, count);
        fixture.survival(1, 0, ClickKind::Pickup);
        fixture
    }

    #[test]
    fn the_crafting_table_lays_out_its_result_its_grid_then_the_main_inventory_and_the_hotbar() {
        let kind = MenuKind::Crafting;
        assert_eq!(kind.container_slots(), 10);
        assert_eq!(kind.slots(), 46);
        assert_eq!(inventory_slot_of(kind, 9), None);
        assert_eq!(inventory_slot_of(kind, 10), Some(9));
        assert_eq!(inventory_slot_of(kind, 36), Some(35));
        assert_eq!(inventory_slot_of(kind, 37), Some(0));
        assert_eq!(inventory_slot_of(kind, 45), Some(8));
        for slot in 0..36 {
            assert_eq!(inventory_slot_of(kind, shown(kind, slot)), Some(slot));
        }
        assert_eq!(menu_slot_of(kind, OFF_HAND), None);
    }

    #[test]
    fn a_log_in_the_grid_makes_planks_and_a_take_consumes_one_log_and_makes_them_again() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 3);
            assert_eq!(fixture.container[0], stack(PLANKS, 4), "{kind:?}");
            fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 4));
            assert_eq!(fixture.container[1], stack(LOG, 2));
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
            // Onto a cursor of planks with room for all of them.
            fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 8));
            assert_eq!(fixture.container[1], stack(LOG, 1));
            let clicked = fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 12));
            assert_eq!(fixture.container[1], None);
            assert_eq!(fixture.container[0], None);
            assert!(clicked.thrown.is_empty() && clicked.dropped.is_empty());
            // An empty result gives nothing more.
            fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 12));
        }
    }

    #[test]
    fn a_log_taken_out_of_the_grid_takes_the_result_away() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 3);
            fixture.survival(1, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(LOG, 3));
            assert_eq!(fixture.container[0], None, "{kind:?}");
        }
    }

    #[test]
    fn a_shift_click_on_the_result_crafts_until_the_logs_run_out() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 5);
            let clicked = fixture.survival(0, 0, ClickKind::QuickMove);
            // To the main inventory and the hotbar, last first: the hotbar's last slot.
            assert_eq!(
                fixture.inventory.get(8),
                Some(stack(PLANKS, 20)),
                "{kind:?}"
            );
            assert_eq!(fixture.container[1], None);
            assert_eq!(fixture.container[0], None);
            assert!(clicked.thrown.is_empty() && clicked.dropped.is_empty());
        }
    }

    #[test]
    fn a_shift_click_with_room_for_part_of_a_batch_drops_the_rest_and_consumes_one_log() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 3);
            for slot in 0..36 {
                fixture.inventory.set(slot, stack(DIRT, 64));
            }
            fixture.inventory.set(20, stack(PLANKS, 62));
            let clicked = fixture.survival(0, 0, ClickKind::QuickMove);
            assert_eq!(
                fixture.inventory.get(20),
                Some(stack(PLANKS, 64)),
                "{kind:?}"
            );
            assert_eq!(clicked.dropped, vec![ItemStack::new(PLANKS, 2)]);
            assert!(clicked.thrown.is_empty());
            // One batch made, and the full inventory stops the click's repeat.
            assert_eq!(fixture.container[1], stack(LOG, 2));
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
            // With no room at all nothing moves and nothing is crafted.
            let before = fixture.snapshot();
            let clicked = fixture.survival(0, 0, ClickKind::QuickMove);
            assert!(clicked.dropped.is_empty());
            assert_eq!(fixture.snapshot(), before);
        }
    }

    #[test]
    fn a_right_click_takes_the_result_whole_and_q_throws_it_whole() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 3);
            fixture.survival(0, 1, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 4), "{kind:?}");
            assert_eq!(fixture.container[1], stack(LOG, 2));
            fixture.menu.carried = None;
            let clicked = fixture.survival(0, 0, ClickKind::Throw);
            assert_eq!(clicked.thrown, vec![ItemStack::new(PLANKS, 4)]);
            assert!(clicked.dropped.is_empty());
            assert_eq!(fixture.container[1], stack(LOG, 1));
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
        }
    }

    #[test]
    fn q_with_ctrl_on_the_result_throws_a_craft_for_every_log() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 64);
            let clicked = fixture.survival(0, 1, ClickKind::Throw);
            assert_eq!(
                clicked.thrown,
                vec![ItemStack::new(PLANKS, 4); 64],
                "{kind:?}"
            );
            assert_eq!(fixture.container[1], None);
            assert_eq!(fixture.container[0], None);
        }
    }

    #[test]
    fn a_number_key_onto_an_empty_hotbar_slot_crafts_and_onto_a_full_one_does_nothing() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 2);
            fixture.survival(0, 3, ClickKind::Swap);
            assert_eq!(fixture.inventory.get(3), Some(stack(PLANKS, 4)), "{kind:?}");
            assert_eq!(fixture.container[1], stack(LOG, 1));
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
            let before = fixture.snapshot();
            fixture.survival(0, 3, ClickKind::Swap);
            assert_eq!(fixture.snapshot(), before);
            // The off hand, empty, takes it too.
            fixture.survival(0, 40, ClickKind::Swap);
            assert_eq!(fixture.inventory.get(OFF_HAND), Some(stack(PLANKS, 4)));
            assert_eq!(fixture.container[1], None);
            assert_eq!(fixture.container[0], None);
        }
    }

    #[test]
    fn a_creative_middle_click_copies_the_result_and_crafts_nothing() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 2);
            let before = fixture.snapshot();
            fixture.survival(0, 2, ClickKind::Clone);
            assert_eq!(fixture.snapshot(), before, "{kind:?}");
            fixture.click(0, 2, ClickKind::Clone, true);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 64));
            assert_eq!(fixture.container[1], stack(LOG, 2));
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
        }
    }

    #[test]
    fn a_double_click_passes_over_the_result() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 2);
            fixture.inventory.set(20, stack(PLANKS, 3));
            fixture.menu.carried = stack(PLANKS, 1);
            let empty = shown(kind, 30) as i16;
            fixture.survival(empty, 0, ClickKind::PickupAll);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 4), "{kind:?}");
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
            assert_eq!(fixture.container[1], stack(LOG, 2));
            fixture.survival(empty, 1, ClickKind::PickupAll);
            assert_eq!(fixture.menu.carried, stack(PLANKS, 4));
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
        }
    }

    #[test]
    fn nothing_can_be_placed_in_the_result_of_either_grid() {
        for kind in GRIDS {
            let mut fixture = logs(kind, 1);
            fixture.menu.carried = stack(DIRT, 10);
            let before = fixture.snapshot();
            for button in [0, 1] {
                fixture.survival(0, button, ClickKind::Pickup);
            }
            assert_eq!(fixture.snapshot(), before, "{kind:?}");
            fixture.inventory.set(2, stack(DIRT, 1));
            let before = fixture.snapshot();
            fixture.survival(0, 2, ClickKind::Swap);
            assert_eq!(fixture.snapshot(), before);
            // A drag passes over it and shares among the rest.
            let other = shown(kind, 30) as i16;
            fixture.drag(0, &[0, other], false);
            assert_eq!(fixture.container[0], stack(PLANKS, 4));
            assert_eq!(fixture.inventory.get(30), Some(stack(DIRT, 10)));
        }
    }

    #[test]
    fn a_result_with_no_recipe_behind_it_is_taken_without_touching_the_grid() {
        // The region keeps the result worked out, so this never arises; were it to, the
        // game's no-recipe remainders (copies of the inputs) would double the grid.
        for kind in GRIDS {
            let mut fixture = Fixture::new(kind);
            fixture.container[1] = stack(DIRT, 3);
            fixture.container[0] = stack(STONE, 4);
            let clicked = fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(STONE, 4), "{kind:?}");
            assert_eq!(fixture.container[1], stack(DIRT, 3));
            assert_eq!(fixture.container[0], None);
            assert!(clicked.dropped.is_empty());
            assert!(fixture.inventory.0.iter().all(Option::is_none));
        }
    }

    #[test]
    fn two_honey_bottles_in_a_slot_leave_a_glass_bottle_in_the_inventory_and_the_rest_in_place() {
        for kind in GRIDS {
            let mut fixture = Fixture::new(kind);
            fixture.put(cell(kind, 0, 0), stack(items::HONEY_BOTTLE, 2));
            fixture.put(cell(kind, 0, 1), stack(items::HONEY_BOTTLE, 1));
            fixture.put(cell(kind, 1, 0), stack(items::HONEY_BOTTLE, 1));
            fixture.put(cell(kind, 1, 1), stack(items::HONEY_BOTTLE, 1));
            fixture.settle();
            assert_eq!(
                fixture.container[0],
                stack(items::HONEY_BLOCK, 1),
                "{kind:?}"
            );
            let clicked = fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(items::HONEY_BLOCK, 1));
            // The first cell still holds a honey bottle, so its glass bottle goes into
            // the inventory (the selected slot, 0, is empty: `getFreeSlot`'s first).
            assert_eq!(fixture.at(cell(kind, 0, 0)), stack(items::HONEY_BOTTLE, 1));
            assert_eq!(
                fixture.inventory.get(0),
                Some(stack(items::GLASS_BOTTLE, 1))
            );
            for (row, column) in [(0, 1), (1, 0), (1, 1)] {
                assert_eq!(
                    fixture.at(cell(kind, row, column)),
                    stack(items::GLASS_BOTTLE, 1)
                );
            }
            assert!(clicked.dropped.is_empty());
            // Bottles and honey make nothing.
            assert_eq!(fixture.container[0], None);
        }
    }

    #[test]
    fn a_remainder_that_fits_nowhere_is_dropped_and_deleted_for_a_creative_player() {
        for kind in GRIDS {
            let mut fixture = Fixture::new(kind);
            fixture.put(cell(kind, 0, 0), stack(items::HONEY_BOTTLE, 3));
            fixture.settle();
            assert_eq!(fixture.container[0], stack(items::SUGAR, 3), "{kind:?}");
            for slot in 0..36 {
                fixture.inventory.set(slot, stack(DIRT, 64));
            }
            fixture.inventory.set(OFF_HAND, stack(DIRT, 64));
            let clicked = fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(items::SUGAR, 3));
            assert_eq!(
                clicked.dropped,
                vec![ItemStack::new(items::GLASS_BOTTLE, 1)]
            );
            assert!(clicked.thrown.is_empty());
            assert_eq!(fixture.container[1], stack(items::HONEY_BOTTLE, 2));
            // For a creative player `Inventory.add` deletes what does not fit.
            fixture.menu.carried = None;
            let clicked = fixture.click(0, 0, ClickKind::Pickup, true);
            assert_eq!(fixture.menu.carried, stack(items::SUGAR, 3));
            assert!(clicked.dropped.is_empty());
            assert_eq!(fixture.container[1], stack(items::HONEY_BOTTLE, 1));
            // The last bottle taken, its glass bottle goes into the emptied slot.
            fixture.menu.carried = None;
            fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.container[1], stack(items::GLASS_BOTTLE, 1));
        }
    }

    #[test]
    fn a_cake_leaves_its_three_milk_buckets_as_buckets_in_their_slots() {
        let kind = MenuKind::Crafting;
        let mut fixture = Fixture::new(kind);
        for column in 0..3 {
            fixture.put(cell(kind, 0, column), stack(items::MILK_BUCKET, 1));
            fixture.put(cell(kind, 2, column), stack(items::WHEAT, 1));
        }
        fixture.put(cell(kind, 1, 0), stack(items::SUGAR, 1));
        fixture.put(cell(kind, 1, 1), stack(items::EGG, 1));
        fixture.put(cell(kind, 1, 2), stack(items::SUGAR, 1));
        fixture.settle();
        assert_eq!(fixture.container[0], stack(items::CAKE, 1));
        let clicked = fixture.survival(0, 0, ClickKind::QuickMove);
        assert_eq!(fixture.inventory.get(8), Some(stack(items::CAKE, 1)));
        for column in 0..3 {
            assert_eq!(fixture.at(cell(kind, 0, column)), stack(items::BUCKET, 1));
            assert_eq!(fixture.at(cell(kind, 1, column)), None);
            assert_eq!(fixture.at(cell(kind, 2, column)), None);
        }
        assert!(clicked.dropped.is_empty());
        assert_eq!(fixture.container[0], None);
    }

    #[test]
    fn a_drag_over_the_grid_works_the_result_out_at_its_end() {
        for kind in GRIDS {
            let mut fixture = Fixture::new(kind);
            fixture.menu.carried = stack(PLANKS, 6);
            let (top, below) = (cell(kind, 0, 0) as i16, cell(kind, 1, 0) as i16);
            fixture.click(-999, 1 << 2, ClickKind::QuickCraft, false);
            fixture.click(top, (1 << 2) | 1, ClickKind::QuickCraft, false);
            fixture.click(below, (1 << 2) | 1, ClickKind::QuickCraft, false);
            // The drag's steps change no slot.
            assert_eq!(fixture.container[0], None, "{kind:?}");
            fixture.click(-999, (1 << 2) | 2, ClickKind::QuickCraft, false);
            assert_eq!(fixture.container[0], stack(items::STICK, 4));
            assert_eq!(fixture.menu.carried, stack(PLANKS, 4));
        }
    }

    #[test]
    fn a_shift_click_from_the_inventory_fills_the_table_grid_first() {
        let kind = MenuKind::Crafting;
        let mut fixture = Fixture::new(kind);
        fixture.inventory.set(9, stack(LOG, 10));
        fixture.survival(10, 0, ClickKind::QuickMove);
        assert_eq!(fixture.container[1], stack(LOG, 10));
        assert_eq!(fixture.inventory.get(9), Some(None));
        assert_eq!(fixture.container[0], stack(PLANKS, 4));
        // From the hotbar too, merging into the grid's logs.
        fixture.inventory.set(0, stack(LOG, 5));
        fixture.survival(37, 0, ClickKind::QuickMove);
        assert_eq!(fixture.container[1], stack(LOG, 15));
        assert_eq!(fixture.inventory.get(0), Some(None));
    }

    #[test]
    fn a_shift_click_that_merges_part_into_the_table_grid_stops_there() {
        let kind = MenuKind::Crafting;
        let mut fixture = Fixture::new(kind);
        fixture.container[1] = stack(STONE, 60);
        for index in 2..10 {
            fixture.container[index] = stack(DIRT, 64);
        }
        fixture.inventory.set(9, stack(STONE, 10));
        fixture.survival(10, 0, ClickKind::QuickMove);
        // The first round merges four into the grid and keeps the rest where it was; the
        // click's repeat finds the grid full and moves the rest to the hotbar.
        assert_eq!(fixture.container[1], stack(STONE, 64));
        assert_eq!(fixture.inventory.get(9), Some(None));
        assert_eq!(fixture.inventory.get(0), Some(stack(STONE, 6)));
        // A hotbar stack with the grid full goes to the main inventory.
        fixture.survival(37, 0, ClickKind::QuickMove);
        assert_eq!(fixture.inventory.get(9), Some(stack(STONE, 6)));
        assert_eq!(fixture.inventory.get(0), Some(None));
    }

    #[test]
    fn a_shift_click_from_the_table_grid_goes_to_the_main_inventory_first() {
        let kind = MenuKind::Crafting;
        let mut fixture = Fixture::new(kind);
        fixture.container[5] = stack(DIRT, 7);
        fixture.survival(5, 0, ClickKind::QuickMove);
        assert_eq!(fixture.inventory.get(9), Some(stack(DIRT, 7)));
        assert_eq!(fixture.container[5], None);
    }

    #[test]
    fn an_iron_pickaxe_crafts_from_its_three_by_three_pattern() {
        let kind = MenuKind::Crafting;
        let mut fixture = Fixture::new(kind);
        for column in 0..3 {
            fixture.put(cell(kind, 0, column), stack(items::IRON_INGOT, 2));
        }
        fixture.put(cell(kind, 1, 1), stack(items::STICK, 2));
        fixture.put(cell(kind, 2, 1), stack(items::STICK, 2));
        fixture.settle();
        assert_eq!(fixture.container[0], stack(items::IRON_PICKAXE, 1));
        fixture.survival(0, 0, ClickKind::Pickup);
        assert_eq!(fixture.menu.carried, stack(items::IRON_PICKAXE, 1));
        // A second pickaxe's worth is left, so the result is made again.
        assert_eq!(fixture.container[0], stack(items::IRON_PICKAXE, 1));
        assert_eq!(fixture.container[1], stack(items::IRON_INGOT, 1));
    }

    #[test]
    fn an_iron_axe_crafts_either_way_round() {
        let kind = MenuKind::Crafting;
        for mirrored in [false, true] {
            let mut fixture = Fixture::new(kind);
            let (near, far) = if mirrored { (1, 0) } else { (0, 1) };
            fixture.put(cell(kind, 0, 0), stack(items::IRON_INGOT, 1));
            fixture.put(cell(kind, 0, 1), stack(items::IRON_INGOT, 1));
            fixture.put(cell(kind, 1, near), stack(items::IRON_INGOT, 1));
            fixture.put(cell(kind, 1, far), stack(items::STICK, 1));
            fixture.put(cell(kind, 2, far), stack(items::STICK, 1));
            fixture.settle();
            assert_eq!(
                fixture.container[0],
                stack(items::IRON_AXE, 1),
                "{mirrored}"
            );
            fixture.survival(0, 0, ClickKind::Pickup);
            assert_eq!(fixture.menu.carried, stack(items::IRON_AXE, 1));
            assert!(fixture.container.iter().all(Option::is_none));
        }
    }

    #[test]
    fn an_iron_pickaxe_does_not_fit_the_inventory_screens_grid() {
        let mut fixture = Fixture::new(MenuKind::Inventory);
        fixture.put(1, stack(items::IRON_INGOT, 1));
        fixture.put(2, stack(items::IRON_INGOT, 1));
        fixture.put(4, stack(items::STICK, 1));
        fixture.settle();
        assert_eq!(fixture.container[0], None);
    }
}
