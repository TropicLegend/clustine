//! A player's menus in the region (`docs/groundwork/n6-plan.md`, decision 2 and sections
//! 6 to 10): opening one on a container block, an ender chest or a container vehicle,
//! its clicks run by the game's rules (`crate::menus`) on what the region holds, its
//! changes reported to the edge, and its closing, by the client, by `stillValid`, at a
//! hand-over and when the player leaves.
//!
//! The menu itself is in the player's state ([`Menu`]); what the region last reported
//! of it is not, so a region restored, merged or split reports the menu whole, once the
//! chunks of its container are loaded, and the edge's comparison heals the client
//! (section 7 item 3). Until they are, the menu stays open, is not checked, and its
//! clicks are refused with an empty answer.

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, BlockState, ENTITY_TYPES, entity_types};
use clustine_world::{BlockPos, EntityData, EntityField, EntityId, PlayerId};

use super::uses::distance_to_sqr;
use super::{Knowledge, Region};
use crate::api::{ClickKind, ItemStack, MenuKind, PlayerEvent, TickOutput};
use crate::menus::{self, MenuSlots};
use crate::physics::aabb::Aabb;
use crate::state::{Menu, MenuOf};

/// What the region last reported of a player's menu: every slot and the carried stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reported {
    serial: u64,
    slots: Vec<Option<ItemStack>>,
    carried: Option<ItemStack>,
}

/// The squared distance within which a player keeps a block's menu open
/// (`Container.stillValidBlockEntity`: the block interaction range, 4.5 and a creative
/// player's 0.5, and 4 more, from the eyes to the block's box), and an entity's
/// (`isWithinEntityInteractionRange(box, 4)`: 3, a creative player's 2, and 4).
const STILL_VALID: f64 = 9.0 * 9.0;

/// A player's eyes above their feet.
const EYES: f64 = 1.62;

/// Whether a menu may stay open.
enum Validity {
    Valid,
    /// Its container's chunks are held and not all loaded yet: it waits.
    Waits,
    Invalid,
}

impl Region {
    /// The chunks of the container the menu of `player` shows: a block's, a double
    /// chest's two, the ender chest's, a vehicle's where it is loaded. The runner holds a
    /// click of the menu while one of them is on its way (section 8 item 6).
    pub fn menu_chunks(&self, player: PlayerId) -> Vec<clustine_world::ChunkPos> {
        let Some(menu) = self.players.get(&player).and_then(|own| own.menu.as_ref()) else {
            return Vec::new();
        };
        match menu.of {
            MenuOf::Entity(key) => self.entity_chunks.get(&key).copied().into_iter().collect(),
            of => block_positions(of).iter().map(|pos| pos.chunk()).collect(),
        }
    }

    /// The menu the container block `state` at `pos` opens, if it is one that opens a
    /// menu and is not kept shut: its kind, the translation key of its title and what
    /// it shows (`docs/groundwork/n6-brief.md`, section 3.0 items 3 to 5 and 9).
    fn block_menu(&self, pos: BlockPos, state: BlockState) -> Option<(MenuKind, MenuOf)> {
        if is_class(state, BlockClass::ChestBlock) {
            let halves = self.chest_halves(pos)?;
            if halves.blocked {
                return None;
            }
            return Some(match halves.second {
                Some(second) => (
                    MenuKind::Rows6,
                    MenuOf::Block {
                        at: halves.first,
                        other: Some(second),
                    },
                ),
                None => (
                    MenuKind::Rows3,
                    MenuOf::Block {
                        at: pos,
                        other: None,
                    },
                ),
            });
        }
        if is_class(state, BlockClass::EnderChestBlock) {
            let above = self.block(BlockPos::new(pos.x, pos.y + 1, pos.z));
            if above.is_some_and(clustine_blocks::connect::is_conductor) {
                return None;
            }
            return Some((MenuKind::Rows3, MenuOf::Ender { at: pos }));
        }
        let single = MenuOf::Block {
            at: pos,
            other: None,
        };
        if is_class(state, BlockClass::BarrelBlock) {
            return Some((MenuKind::Rows3, single));
        }
        if is_class(state, BlockClass::ShulkerBoxBlock) {
            if !self.shulker_box_may_open(pos) {
                return None;
            }
            return Some((MenuKind::ShulkerBox, single));
        }
        if is_class(state, BlockClass::DispenserBlock) {
            return Some((MenuKind::Grid3x3, single));
        }
        if is_class(state, BlockClass::HopperBlock) {
            return Some((MenuKind::Hopper, single));
        }
        None
    }

    /// Whether `state` is a block whose use opens a menu (or would, were it not kept
    /// shut): the click is the block's either way (`useWithoutItem` answers `SUCCESS`).
    pub(super) fn opens_menu(state: BlockState) -> bool {
        is_class(state, BlockClass::ChestBlock)
            || is_class(state, BlockClass::EnderChestBlock)
            || is_class(state, BlockClass::BarrelBlock)
            || is_class(state, BlockClass::ShulkerBoxBlock)
            || is_class(state, BlockClass::DispenserBlock)
            || is_class(state, BlockClass::HopperBlock)
    }

    /// The use of the container block at `pos` by `player`: their menu opens if the
    /// player is this region's and is applying the input that clicked; a click that
    /// came from another region's player is consumed and opens nothing (section 6 item
    /// 2), as is one on a container kept shut.
    pub(super) fn use_container(
        &mut self,
        pos: BlockPos,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        if self.applying.map(|(id, _)| id) != Some(player) {
            return;
        }
        let Some(state) = self.own_block(pos) else {
            return;
        };
        let Some((kind, of)) = self.block_menu(pos, state) else {
            return;
        };
        self.open_menu(player, of, kind, output);
    }

    /// A player's use of the container vehicle `vehicle` (a chest cart, a hopper cart,
    /// a chest boat): their menu of it opens, if they are this region's and applying
    /// the input that used it.
    pub(super) fn use_container_vehicle(
        &mut self,
        vehicle: EntityId,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        if self.applying.map(|(id, _)| id) != Some(player) {
            return;
        }
        let Some((_, entity)) = self.find_entity(vehicle) else {
            return;
        };
        let Some(kind) = vehicle_menu(entity.kind) else {
            return;
        };
        self.open_menu(player, MenuOf::Entity(vehicle), kind, output);
    }

    /// `PlayerInput::OpenVehicleInventory`: the menu of the container vehicle the player
    /// rides, if it is one.
    pub(super) fn open_vehicle_inventory(&mut self, player: PlayerId, output: &mut TickOutput) {
        let Some(vehicle) = self
            .players
            .get(&player)
            .and_then(|own| own.riding.map(|riding| riding.vehicle))
        else {
            return;
        };
        self.use_container_vehicle(vehicle, player, output);
    }

    /// `ServerPlayer.openMenu`: the menu open is closed, the container counts the
    /// player as an opener (`startOpen`, both halves of a double chest, the right one
    /// first), and the menu is reported whole. Its serial is the number of the input
    /// that opened it.
    fn open_menu(&mut self, id: PlayerId, of: MenuOf, kind: MenuKind, output: &mut TickOutput) {
        let Some((_, serial)) = self.applying else {
            return;
        };
        self.close_menu(id, output);
        // A loot table still set is unpacked at the first touch, into nothing: N6 has no
        // loot (decision 4).
        for pos in block_positions(of) {
            if let Some(mut container) = self.container_at(pos)
                && container.loot
            {
                container.unpack();
                self.set_container(pos, container, output);
            }
            self.start_open(pos, output);
        }
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.menu = Some(Menu {
            serial,
            of,
            kind,
            carried: None,
            drag: None,
        });
        self.journal.players.insert(id);
        self.report_whole(id, output);
    }

    /// The slots of the container the menu `of` shows, as the region has them now:
    /// `None` where any of it is not here to read.
    fn container_slots(
        &self,
        id: PlayerId,
        of: MenuOf,
        kind: MenuKind,
    ) -> Option<Vec<Option<ItemStack>>> {
        match of {
            MenuOf::Block { at, other } => {
                let mut slots = self.container_at(at)?.slots;
                if let Some(other) = other {
                    slots.extend(self.container_at(other)?.slots);
                }
                (slots.len() == kind.container_slots()).then_some(slots)
            }
            MenuOf::Ender { .. } => Some(self.players.get(&id)?.ender.0.to_vec()),
            MenuOf::Entity(key) => {
                let (_, entity) = self.find_entity(key)?;
                let contents = match &entity.data {
                    EntityData::Minecart { contents, .. } | EntityData::Boat { contents, .. } => {
                        contents
                    }
                    _ => return None,
                };
                let mut slots = vec![None; kind.container_slots()];
                for (slot, stack) in contents {
                    if let Some(place) = slots.get_mut(usize::from(*slot)) {
                        *place = Some(*stack);
                    }
                }
                Some(slots)
            }
        }
    }

    /// Writes back the container's slots `slots` of the menu `of`, where they changed.
    fn write_container_slots(
        &mut self,
        id: PlayerId,
        of: MenuOf,
        slots: &[Option<ItemStack>],
        output: &mut TickOutput,
    ) {
        match of {
            MenuOf::Block { at, other } => {
                let halves = [Some(at), other];
                for (index, pos) in halves.into_iter().enumerate() {
                    let Some(pos) = pos else {
                        continue;
                    };
                    let Some(mut container) = self.container_at(pos) else {
                        continue;
                    };
                    let part =
                        &slots[index * 27..(index * 27 + container.slots.len()).min(slots.len())];
                    if container.slots != part {
                        container.slots = part.to_vec();
                        self.set_container(pos, container, output);
                    }
                }
            }
            MenuOf::Ender { .. } => {
                if let Some(player) = self.players.get_mut(&id)
                    && player.ender.0.as_slice() != slots
                {
                    for (place, stack) in player.ender.0.iter_mut().zip(slots) {
                        *place = *stack;
                    }
                    self.journal.players.insert(id);
                }
            }
            MenuOf::Entity(key) => {
                let Some((chunk, entity)) = self.find_entity(key) else {
                    return;
                };
                let contents: clustine_world::Contents = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, stack)| Some((u8::try_from(slot).ok()?, (*stack)?)))
                    .collect();
                let mut data = entity.data.clone();
                match &mut data {
                    EntityData::Minecart { contents: was, .. }
                    | EntityData::Boat { contents: was, .. } => {
                        if *was == contents {
                            return;
                        }
                        *was = contents;
                    }
                    _ => return,
                }
                self.commit_entity(chunk, key, vec![EntityField::Data(data)], output);
            }
        }
    }

    /// Every slot of the player's menu, as it is now: the container's, then the main
    /// inventory's and the hotbar's.
    fn menu_view(&self, id: PlayerId) -> Option<(Vec<Option<ItemStack>>, Option<ItemStack>)> {
        let player = self.players.get(&id)?;
        let menu = player.menu.as_ref()?;
        let mut container = self.container_slots(id, menu.of, menu.kind)?;
        let mut inventory = player.inventory.clone();
        let slots = MenuSlots {
            kind: menu.kind,
            container: &mut container,
            inventory: &mut inventory,
            selected: usize::from(player.selected_slot),
        };
        let all = (0..menu.kind.slots())
            .map(|index| menus::menu_slot(&slots, index).flatten())
            .collect();
        Some((all, menu.carried))
    }

    /// Reports the player's menu whole ([`PlayerEvent::MenuOpened`]), if its container
    /// is here to read.
    fn report_whole(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some((slots, carried)) = self.menu_view(id) else {
            return;
        };
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(menu) = player.menu.as_ref() else {
            return;
        };
        let (serial, kind, applied) = (menu.serial, menu.kind, player.last_input);
        let title = title_of(menu.of, menu.kind, self);
        self.reported.insert(
            id,
            Reported {
                serial,
                slots: slots.clone(),
                carried,
            },
        );
        output.player_events.push((
            id,
            PlayerEvent::MenuOpened {
                serial,
                applied,
                kind,
                title,
                slots,
                carried,
            },
        ));
    }

    /// `broadcastChanges`: reports what changed of the player's menu against what was
    /// last reported, and, if `answering`, an answer with nothing where nothing did. A
    /// menu never reported, as after a restore, is reported whole.
    fn report_changes(&mut self, id: PlayerId, answering: bool, output: &mut TickOutput) {
        let Some((slots, carried)) = self.menu_view(id) else {
            if answering {
                self.answer_empty(id, output);
            }
            return;
        };
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(menu) = player.menu.as_ref() else {
            return;
        };
        let (serial, applied) = (menu.serial, player.last_input);
        let Some(reported) = self
            .reported
            .get_mut(&id)
            .filter(|was| was.serial == serial)
        else {
            self.report_whole(id, output);
            return;
        };
        let changed: Vec<(u16, Option<ItemStack>)> = slots
            .iter()
            .enumerate()
            .filter(|(index, stack)| reported.slots.get(*index) != Some(*stack))
            .filter_map(|(index, stack)| Some((u16::try_from(index).ok()?, *stack)))
            .collect();
        let carried_changed = (reported.carried != carried).then_some(carried);
        reported.slots = slots;
        reported.carried = carried;
        if changed.is_empty() && carried_changed.is_none() && !answering {
            return;
        }
        output.player_events.push((
            id,
            PlayerEvent::MenuChanged {
                serial,
                applied,
                slots: changed,
                carried: carried_changed,
            },
        ));
    }

    /// An answer that changes nothing, to a click of the player's menu that was refused
    /// or passed over: the edge then compares what the client guessed.
    fn answer_empty(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(menu) = player.menu.as_ref() else {
            return;
        };
        output.player_events.push((
            id,
            PlayerEvent::MenuChanged {
                serial: menu.serial,
                applied: player.last_input,
                slots: Vec::new(),
                carried: None,
            },
        ));
    }

    /// A click of the menu of the serial `serial` (`handleContainerClick`): run by the
    /// game's rules on what the region holds and answered, whatever came of it. A
    /// click of another menu than the open one is answered with the menu whole, or
    /// with the other closed where none is open (section 7 item 2).
    pub(super) fn menu_click(
        &mut self,
        id: PlayerId,
        serial: u64,
        slot: i16,
        button: i8,
        kind: ClickKind,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let applied = player.last_input;
        let Some(menu) = player.menu.clone() else {
            output
                .player_events
                .push((id, PlayerEvent::MenuClosed { serial, applied }));
            return;
        };
        if menu.serial != serial {
            self.report_whole(id, output);
            return;
        }
        if !matches!(self.menu_validity(id), Validity::Valid) {
            self.answer_empty(id, output);
            return;
        }
        let Some(mut container) = self.container_slots(id, menu.of, menu.kind) else {
            self.answer_empty(id, output);
            return;
        };
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let mut inventory = player.inventory.clone();
        let mut menu = menu;
        let clicked = {
            let mut slots = MenuSlots {
                kind: menu.kind,
                container: &mut container,
                inventory: &mut inventory,
                selected: usize::from(player.selected_slot),
            };
            // Every player of Clustine is creative until survival comes.
            menus::click(&mut slots, &mut menu, slot, button, kind, true)
        };
        if !clicked.refused {
            if player.inventory != inventory {
                player.inventory = inventory;
            }
            player.menu = Some(menu.clone());
            self.journal.players.insert(id);
            self.write_container_slots(id, menu.of, &container, output);
            for stack in clicked.thrown {
                let thrown = super::entities::Thrown {
                    player: id,
                    stack,
                    crouching: false,
                };
                self.throw(thrown, output);
            }
        }
        self.report_changes(id, true, output);
    }

    /// The client closed the menu of the serial `serial` (`handleContainerClose`).
    pub(super) fn menu_closed_by_client(
        &mut self,
        id: PlayerId,
        serial: u64,
        output: &mut TickOutput,
    ) {
        let open = self
            .players
            .get(&id)
            .and_then(|player| player.menu.as_ref())
            .is_some_and(|menu| menu.serial == serial);
        if open {
            self.close_menu(id, output);
        }
    }

    /// `PlayerInput::MenuResync`: the menu whole, or the inventory whole where none is
    /// open.
    pub(super) fn menu_resync(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.menu.is_some() {
            self.reported.remove(&id);
            self.report_whole(id, output);
        } else {
            output.player_events.push((
                id,
                PlayerEvent::NoMenu {
                    applied: player.last_input,
                    inventory: player.inventory.clone(),
                },
            ));
        }
    }

    /// `doCloseContainer` of the player's menu, for a player who stays: the carried
    /// stack goes back into the inventory, what does not fit is thrown, the container
    /// stops counting them, and the edge is told, the inventory's slots that changed
    /// after the close (`closeContainer` sends `container_close` first).
    pub(super) fn close_menu(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let Some(mut menu) = player.menu.take() else {
            return;
        };
        let before = player.inventory.clone();
        let left = menus::put_back(
            &mut player.inventory,
            usize::from(player.selected_slot),
            &mut menu,
        );
        let applied = player.last_input;
        let changed: Vec<(usize, Option<ItemStack>)> = player
            .inventory
            .0
            .iter()
            .enumerate()
            .filter(|(slot, stack)| before.0[*slot] != **stack)
            .map(|(slot, stack)| (slot, *stack))
            .collect();
        self.journal.players.insert(id);
        self.reported.remove(&id);
        output.player_events.push((
            id,
            PlayerEvent::MenuClosed {
                serial: menu.serial,
                applied,
            },
        ));
        for (slot, stack) in changed {
            let slot = u8::try_from(slot).expect("an inventory has fewer than 256 slots");
            output
                .player_events
                .push((id, PlayerEvent::InventorySlot { slot, stack }));
        }
        if let Some(stack) = left {
            let thrown = super::entities::Thrown {
                player: id,
                stack,
                crouching: false,
            };
            self.throw(thrown, output);
        }
        for pos in block_positions(menu.of) {
            self.stop_open(pos, output);
        }
    }

    /// The player leaves the world with a menu open: the carried stack is thrown, if
    /// `throw`, as the game drops it for a player removed (`dropOrPlaceInInventory`,
    /// section 7 item 10), and the container stops counting them. Called before they
    /// are taken out.
    pub(super) fn menu_leaving(&mut self, id: PlayerId, throw: bool, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let Some(menu) = player.menu.take() else {
            return;
        };
        self.reported.remove(&id);
        if let Some(stack) = menu.carried.filter(|_| throw) {
            let thrown = super::entities::Thrown {
                player: id,
                stack,
                crouching: false,
            };
            self.throw(thrown, output);
        }
        for pos in block_positions(menu.of) {
            self.stop_open(pos, output);
        }
    }

    /// Whether the player's menu may stay open (`stillValid`): its container still
    /// there and within reach, its chunks held. One whose chunks are held and not all
    /// loaded waits; only one not held closes (section 7 item 3).
    fn menu_validity(&self, id: PlayerId) -> Validity {
        let Some(player) = self.players.get(&id) else {
            return Validity::Invalid;
        };
        let Some(menu) = player.menu.as_ref() else {
            return Validity::Invalid;
        };
        let feet = player.pose.position;
        let eyes = [feet.x, feet.y + EYES, feet.z];
        let positions = block_positions(menu.of);
        for pos in &positions {
            let chunk = pos.chunk();
            if !self.chunks.contains_key(&chunk) {
                return if self.land.knowledge(chunk) == Knowledge::Held {
                    Validity::Waits
                } else {
                    Validity::Invalid
                };
            }
        }
        match menu.of {
            MenuOf::Block { at, other } => {
                let Some(state) = self.own_block(at) else {
                    return Validity::Invalid;
                };
                // The block entity is still the one it was: the same kind of container,
                // and a double chest still both halves.
                let same = self
                    .block_menu_ignoring_shut(at, state)
                    .is_some_and(|(kind, of)| {
                        kind == menu.kind && of == MenuOf::Block { at, other }
                    });
                if !same || box_distance_sqr(eyes, at) >= STILL_VALID {
                    return Validity::Invalid;
                }
                Validity::Valid
            }
            MenuOf::Ender { at } => {
                let ender = self
                    .own_block(at)
                    .is_some_and(|state| is_class(state, BlockClass::EnderChestBlock));
                if !ender || box_distance_sqr(eyes, at) >= STILL_VALID {
                    return Validity::Invalid;
                }
                Validity::Valid
            }
            MenuOf::Entity(key) => {
                let Some((_, entity)) = self.find_entity(key) else {
                    // Gone, or, after a restore, in a chunk near the player that is held
                    // and not loaded yet: a vehicle within reach is in one of those.
                    let centre = player.chunk();
                    let waiting = (-1..=1).any(|dx| {
                        (-1..=1).any(|dz| {
                            let chunk = clustine_world::ChunkPos::new(centre.x + dx, centre.z + dz);
                            !self.chunks.contains_key(&chunk)
                                && self.land.knowledge(chunk) == Knowledge::Held
                        })
                    });
                    return if waiting {
                        Validity::Waits
                    } else {
                        Validity::Invalid
                    };
                };
                if distance_to_sqr(self.entity_box(entity), eyes) >= STILL_VALID {
                    return Validity::Invalid;
                }
                Validity::Valid
            }
        }
    }

    /// What the container block at `pos` would open, whether or not it is kept shut:
    /// `stillValid` does not ask whether a chest is blocked.
    fn block_menu_ignoring_shut(
        &self,
        pos: BlockPos,
        state: BlockState,
    ) -> Option<(MenuKind, MenuOf)> {
        if is_class(state, BlockClass::ChestBlock) {
            let halves = self.chest_halves(pos)?;
            let kind = if halves.second.is_some() {
                MenuKind::Rows6
            } else {
                MenuKind::Rows3
            };
            return Some((
                kind,
                MenuOf::Block {
                    at: halves.first,
                    other: halves.second,
                },
            ));
        }
        let single = MenuOf::Block {
            at: pos,
            other: None,
        };
        let kind = if is_class(state, BlockClass::BarrelBlock) {
            MenuKind::Rows3
        } else if is_class(state, BlockClass::ShulkerBoxBlock) {
            MenuKind::ShulkerBox
        } else if is_class(state, BlockClass::DispenserBlock) {
            MenuKind::Grid3x3
        } else if is_class(state, BlockClass::HopperBlock) {
            MenuKind::Hopper
        } else {
            return None;
        };
        Some((kind, single))
    }

    /// The menus' part of the end of the tick, after every move and before the block
    /// events are kept (section 8 item 7): a player about to be let go to another
    /// region has their menu closed, the carried stack put back into the inventory
    /// that goes with them; a menu no longer valid is closed; and every other is
    /// reported where it changed (`ServerPlayer.tick`'s `broadcastChanges`), or whole
    /// where it was never reported.
    pub(super) fn tick_menus(&mut self, output: &mut TickOutput) {
        let open: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| player.menu.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in open {
            let departing = self
                .players
                .get(&id)
                .is_some_and(|player| self.land.knowledge(player.chunk()).holder().is_some());
            if departing {
                self.close_menu(id, output);
                continue;
            }
            match self.menu_validity(id) {
                Validity::Invalid => self.close_menu(id, output),
                Validity::Waits => {}
                Validity::Valid => self.report_changes(id, false, output),
            }
        }
    }
}

/// The blocks a menu counts the player as an opener of: a container block's, both halves
/// of a double chest, the ender chest's; none for a vehicle.
fn block_positions(of: MenuOf) -> Vec<BlockPos> {
    match of {
        MenuOf::Block { at, other } => std::iter::once(at).chain(other).collect(),
        MenuOf::Ender { at } => vec![at],
        MenuOf::Entity(_) => Vec::new(),
    }
}

/// Whether an entity of the type `kind` is a container vehicle, whose use opens its menu.
pub(super) fn is_container_vehicle(kind: u16) -> bool {
    vehicle_menu(kind).is_some()
}

/// The menu a container vehicle of the entity type `kind` opens: a chest boat's or a chest
/// cart's three rows, a hopper cart's five.
fn vehicle_menu(kind: u16) -> Option<MenuKind> {
    let path = ENTITY_TYPES
        .get(usize::from(kind))?
        .strip_prefix("minecraft:")?;
    if i32::from(kind) == entity_types::HOPPER_MINECART {
        Some(MenuKind::Hopper)
    } else if i32::from(kind) == entity_types::CHEST_MINECART
        || path.ends_with("_chest_boat")
        || path.ends_with("_chest_raft")
    {
        Some(MenuKind::Rows3)
    } else {
        None
    }
}

/// The translation key of a menu's title (`getDisplayName`): a container block's own, a
/// double chest's, the ender chest's, or a container vehicle's entity name.
fn title_of(of: MenuOf, kind: MenuKind, region: &Region) -> String {
    let key = match of {
        MenuOf::Ender { .. } => "container.enderchest",
        MenuOf::Entity(key) => {
            let name = region
                .find_entity(key)
                .and_then(|(_, entity)| ENTITY_TYPES.get(usize::from(entity.kind)));
            return name.map_or_else(
                || "container.chest".to_owned(),
                |name| format!("entity.{}", name.replace(':', ".")),
            );
        }
        MenuOf::Block { at, .. } => {
            let state = region.block(at);
            let is = |class| state.is_some_and(|state| is_class(state, class));
            match kind {
                MenuKind::Rows6 => "container.chestDouble",
                MenuKind::ShulkerBox => "container.shulkerBox",
                MenuKind::Hopper => "container.hopper",
                MenuKind::Grid3x3 if is(BlockClass::DropperBlock) => "container.dropper",
                MenuKind::Grid3x3 => "container.dispenser",
                MenuKind::Rows3 if is(BlockClass::BarrelBlock) => "container.barrel",
                MenuKind::Rows3 => "container.chest",
            }
        }
    };
    key.to_owned()
}

/// The squared distance from `point` to the unit box of the block `pos`.
fn box_distance_sqr(point: [f64; 3], pos: BlockPos) -> f64 {
    let low = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
    let high = low.map(|value| value + 1.0);
    distance_to_sqr(Aabb::new(low, high), point)
}
