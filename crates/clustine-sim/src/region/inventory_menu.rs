//! The player's own inventory screen, window 0 (`InventoryMenu`; V3,
//! `docs/groundwork/v-plan.md`, section 4.7): its clicks run by the game's rules over the
//! inventory and the player's [`Crafting`], its changes reported to the edge every tick
//! whatever else is open, and its `removed`.
//!
//! Window 0 has the serial 0, which no input has. The game never opens or closes it: it
//! is the menu a player has whenever no other is open, and it stays open across a
//! hand-over (`PlayerTransfer::crafting`). What the region last reported of it is not in
//! the state: a region that has not reported it (after a restore, a merge or a split, or
//! an arrival) takes it as it is, reporting nothing, and the edge, which asks for window 0
//! whole after each of those (`MenuResync`), keeps its copy whole (the V3 review's
//! findings 2 and 9). So a restored region reports what the region run on would.
//!
//! [`Crafting`]: crate::state::Crafting

use std::collections::BTreeMap;

use clustine_world::PlayerId;

use super::Region;
use crate::api::{ClickKind, ItemStack, MenuKind, PlayerEvent, TickOutput};
use crate::menus::{self, MenuSlots};
use crate::state::{Menu, MenuOf};

/// The serial of window 0.
pub(crate) const SERIAL: u64 = 0;

/// What the region last reported of a player's window 0: its 46 slots and the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reported {
    slots: Vec<Option<ItemStack>>,
    carried: Option<ItemStack>,
}

impl Region {
    /// Every slot of the player's window 0 as it is now, and the cursor.
    fn inventory_view(&self, id: PlayerId) -> Option<(Vec<Option<ItemStack>>, Option<ItemStack>)> {
        let player = self.players.get(&id)?;
        let mut container = self.container_slots(id, MenuOf::Inventory, MenuKind::Inventory)?;
        let mut inventory = player.inventory.clone();
        let slots = MenuSlots {
            kind: MenuKind::Inventory,
            container: &mut container,
            inventory: &mut inventory,
            selected: usize::from(player.selected_slot),
            mount: None,
            merchant: None,
        };
        let all = (0..MenuKind::Inventory.slots())
            .map(|index| menus::menu_slot(&slots, index).flatten())
            .collect();
        Some((all, player.crafting.carried.clone()))
    }

    /// `broadcastChanges` of window 0: what changed against what was last reported, and
    /// if `answering` an answer with nothing where nothing did. Never reported, every
    /// slot and the cursor are.
    pub(super) fn report_inventory_menu(
        &mut self,
        id: PlayerId,
        answering: bool,
        output: &mut TickOutput,
    ) {
        let Some((slots, carried)) = self.inventory_view(id) else {
            return;
        };
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let applied = player.last_input;
        let (changed, carried_changed) = match self.reported_inventory.get(&id) {
            Some(was) => (
                slots
                    .iter()
                    .enumerate()
                    .filter(|(index, stack)| was.slots.get(*index) != Some(*stack))
                    .map(|(index, stack)| (index, stack.clone()))
                    .collect(),
                (was.carried != carried).then(|| carried.clone()),
            ),
            None => (Vec::new(), None),
        };
        self.reported_inventory
            .insert(id, Reported { slots, carried });
        if changed.is_empty() && carried_changed.is_none() && !answering {
            return;
        }
        let slots = changed
            .into_iter()
            .map(|(index, stack)| {
                let index = u16::try_from(index).expect("window 0 has 46 slots");
                (index, stack)
            })
            .collect();
        output.player_events.push((
            id,
            PlayerEvent::MenuChanged {
                serial: SERIAL,
                applied,
                slots,
                carried: carried_changed,
            },
        ));
    }

    /// Takes what the edge has of a joining player's window 0 as reported: the
    /// inventory, with the grid, result and cursor empty, as the edge sends it at the
    /// join. A grid kept in their place is then reported.
    pub(super) fn seed_inventory_report(&mut self, id: PlayerId) {
        let Some((mut slots, _)) = self.inventory_view(id) else {
            return;
        };
        let own_slots = MenuKind::Inventory.container_slots();
        slots
            .iter_mut()
            .take(own_slots)
            .for_each(|slot| *slot = None);
        self.reported_inventory.insert(
            id,
            Reported {
                slots,
                carried: None,
            },
        );
    }

    /// Window 0 whole ([`PlayerEvent::MenuOpened`] of the serial 0): the answer to a
    /// resync, which the edge sends as `container_set_content` with the next state id.
    pub(super) fn report_inventory_whole(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some((slots, carried)) = self.inventory_view(id) else {
            return;
        };
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let applied = player.last_input;
        self.reported_inventory.insert(
            id,
            Reported {
                slots: slots.clone(),
                carried: carried.clone(),
            },
        );
        output.player_events.push((
            id,
            PlayerEvent::MenuOpened {
                serial: SERIAL,
                applied,
                kind: MenuKind::Inventory,
                title: String::new(),
                owner: None,
                slots,
                carried,
            },
        ));
    }

    /// A click of window 0 (`handleContainerClick` with `containerId` 0): passed over
    /// without a word while another menu is open, as the game's test of the window's id
    /// does; else run by the game's rules and answered.
    pub(super) fn inventory_click(
        &mut self,
        id: PlayerId,
        slot: i16,
        button: i8,
        kind: ClickKind,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.menu.is_some() {
            return;
        }
        // A spectator's is answered with the window whole and runs nothing
        // (`handleContainerClick` @38-64).
        if player.mode.current == clustine_world::GameMode::Spectator {
            self.report_inventory_whole(id, output);
            return;
        }
        let Some(mut container) = self.container_slots(id, MenuOf::Inventory, MenuKind::Inventory)
        else {
            return;
        };
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let mut menu = Menu {
            serial: SERIAL,
            of: MenuOf::Inventory,
            kind: MenuKind::Inventory,
            carried: player.crafting.carried.clone(),
            drag: player.crafting.drag.clone(),
            trade: None,
            grid: None,
        };
        let mut inventory = player.inventory.clone();
        let clicked = {
            let mut slots = MenuSlots {
                kind: MenuKind::Inventory,
                container: &mut container,
                inventory: &mut inventory,
                selected: usize::from(player.selected_slot),
                mount: None,
                merchant: None,
            };
            // As for any menu (`menu_play.rs`): what a player with infinite materials
            // may do.
            let creative = player.mode.current.abilities().instabuild;
            menus::click(&mut slots, &mut menu, slot, button, kind, creative)
        };
        if !clicked.refused {
            player.inventory = inventory;
            player.crafting.carried = menu.carried;
            player.crafting.drag = menu.drag;
            self.journal.players.insert(id);
            self.write_container_slots(id, MenuOf::Inventory, &container, output);
            self.equipped_by_click(id, &clicked.equipped, output);
            for stack in clicked.thrown {
                let thrown = super::entities::Thrown {
                    player: id,
                    stack,
                    crouching: false,
                };
                self.throw(thrown, output);
            }
        }
        self.report_inventory_menu(id, true, output);
    }

    /// `LivingEntity.onEquipItem` for each armour or off-hand slot a click put
    /// something new in (`ArmorSlot.setByPlayer`, `InventoryMenu$1.setByPlayer`): the new
    /// item's equip sound where it goes in its own slot, heard by everyone, the player too
    /// (`playSeededSound` with no player), as the dispenser's equipping plays it. Not for
    /// a spectator (@0-33). Named: the `EQUIP` and `UNEQUIP` game events, which only
    /// sculk hears, are not sent, as for the dispenser's.
    fn equipped_by_click(&mut self, id: PlayerId, equipped: &[usize], output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.mode.current == clustine_world::GameMode::Spectator {
            return;
        }
        let feet = player.pose.position;
        for &index in equipped {
            // The equipment slot the menu slot is: the head down to the feet, then the
            // off hand.
            let slot = match index {
                5 => crate::living::slots::HEAD,
                6 => crate::living::slots::CHEST,
                7 => crate::living::slots::LEGS,
                8 => crate::living::slots::FEET,
                45 => crate::living::slots::OFF_HAND,
                _ => continue,
            };
            let stack = menus::inventory_slot_of(MenuKind::Inventory, index)
                .and_then(|at| player.inventory.get(at).flatten());
            let Some(equippable) = stack
                .filter(|stack| {
                    usize::try_from(stack.item).is_ok_and(|item| item < clustine_data::ITEMS.len())
                })
                .and_then(|stack| clustine_data::ItemComponents::of(stack.item).equippable)
            else {
                continue;
            };
            if equippable.slot != slot {
                continue;
            }
            output.events.push(crate::api::RegionEvent::Sound {
                sound: equippable.sound,
                source: crate::api::SOUND_SOURCE_PLAYERS,
                position: feet,
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
    }

    /// `InventoryMenu.removed` for a player who stays (`ContainerClose` with no other
    /// menu open, and before a change of level): the cursor first
    /// (`AbstractContainerMenu.removed`), then the result cleared, then the grid's slots
    /// in order (`clearContainer`), each put back into the inventory as
    /// `placeItemBackInInventory` puts it, what does not fit thrown. The client is told
    /// by window 0's report, which empties its grid and cursor (the V3 review's finding
    /// 15).
    pub(super) fn inventory_menu_removed(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if player.crafting.is_empty() {
            return;
        }
        let crafting = std::mem::take(&mut player.crafting);
        self.journal.players.insert(id);
        let selected = usize::from(player.selected_slot);
        let mut thrown = Vec::new();
        for stack in std::iter::once(crafting.carried)
            .chain(crafting.grid)
            .flatten()
        {
            let mut menu = Menu {
                serial: SERIAL,
                of: MenuOf::Inventory,
                kind: MenuKind::Inventory,
                carried: Some(stack),
                drag: None,
                trade: None,
                grid: None,
            };
            thrown.extend(menus::put_back(&mut player.inventory, selected, &mut menu));
        }
        for stack in thrown {
            let thrown = super::entities::Thrown {
                player: id,
                stack,
                crouching: false,
            };
            self.throw(thrown, output);
        }
    }

    /// The reports of every player's window 0, at the end of the tick's menus: whatever
    /// else they have open, so that the edge's copy stays the region's (the V3 review's
    /// finding 2); the edge sends it only while window 0 is shown.
    pub(super) fn tick_inventory_menus(&mut self, output: &mut TickOutput) {
        let players = &self.players;
        self.reported_inventory
            .retain(|id, _| players.contains_key(id));
        // An inventory slot the region told the edge of since its last report of window 0
        // is window 0's too at the edge, in its copy and as its client shows it
        // (`Menus::inventory_slot`): it is not reported again.
        let mut told: BTreeMap<PlayerId, Vec<(u8, Option<ItemStack>)>> = BTreeMap::new();
        for (id, event) in &output.player_events {
            match event {
                PlayerEvent::InventorySlot { slot, stack } => {
                    told.entry(*id).or_default().push((*slot, stack.clone()));
                }
                PlayerEvent::MenuChanged { serial: SERIAL, .. }
                | PlayerEvent::MenuOpened { serial: SERIAL, .. } => {
                    told.remove(id);
                }
                _ => {}
            }
        }
        for (id, slots) in told {
            let Some(reported) = self.reported_inventory.get_mut(&id) else {
                continue;
            };
            for (slot, stack) in slots {
                let index = menus::menu_slot_of(MenuKind::Inventory, usize::from(slot));
                if let Some(at) = index.and_then(|index| reported.slots.get_mut(index)) {
                    *at = stack;
                }
            }
        }
        let ids: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| !player.dead)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.report_inventory_menu(id, false, output);
        }
    }
}
