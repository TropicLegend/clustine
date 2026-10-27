//! The edge's side of a player's menus (`docs/groundwork/n6-plan.md`, decision 2 and
//! sections 6 to 9): what the game's `ContainerSynchronizer` keeps on the server for the
//! client, and the packets it sends. The region keeps the menu and runs its clicks; the
//! edge keeps the window's id, its state id, a copy of the menu's real slots and carried
//! stack as the region last reported them, and the client's remote slots: what it last
//! sent the client of each slot, or what the client said it shows after a click, by
//! hashes. After each answer of the region it compares the two and sends what differs,
//! as `broadcastChanges` does after every click.
//!
//! A click is held here, by the number of its input, until the region's answer covers
//! that number: only then are its hashes the client's view of the real slots the answer
//! describes. A menu is named by its serial, the number of the input that opened it, so
//! that an answer about one menu is never applied to another; and an event is taken only
//! if what it reports is at least as late as the last opening and the last close the
//! edge knows of, so that a region restored to before either cannot open an old window
//! again.

use std::collections::BTreeMap;

use bytes::Bytes;
use clustine_protocol::components::data_component_type;
use clustine_protocol::item::ItemStack as WireStack;
use clustine_protocol::packets;
use clustine_protocol::packets::play::{
    ClientboundContainerClose, ContainerClick, ContainerSetSlot, HashedStack, MountScreenOpen,
    OpenScreen, SetContainerContent, SetCursorItem, click, menu,
};
use clustine_protocol::text::Text;
use clustine_sim::api::{ClickKind, ItemStack, MenuKind, PlayerInput};
use clustine_world::components::Kind;

use crate::component_hash;

/// What the client shows of a slot, as far as the edge knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Remote {
    /// What the edge last sent of it.
    Sent(Option<ItemStack>),
    /// What the client said it shows after its click, by hashes.
    Hashed(Option<HashedStack>),
}

impl Remote {
    /// Whether the client shows `real` (`RemoteSlot.matches`).
    fn shows(&self, real: Option<&ItemStack>) -> bool {
        match self {
            Self::Sent(sent) => sent.as_ref() == real,
            Self::Hashed(None) => real.is_none(),
            Self::Hashed(Some(hashed)) => real.is_some_and(|real| hashed_matches(hashed, real)),
        }
    }
}

/// Whether a client that says `hashed` of a slot shows `real` (`HashedStack$ActualItem.
/// matches`): the same item and count, the same kinds added with the same hashes of
/// their values (`HashedPatchMap.matches`, the game's `HashOps` over each value), and
/// the same kinds removed, in whatever order the client names them. A client that
/// names a kind twice is not matched, and is sent the slot again, which it does not
/// show.
fn hashed_matches(hashed: &HashedStack, real: &ItemStack) -> bool {
    if hashed.item != real.item || hashed.count != real.count {
        return false;
    }
    let (added, removed) = real
        .components
        .0
        .as_deref()
        .map_or((&[][..], &[][..]), |patch| {
            (&patch.added[..], &patch.removed[..])
        });
    let mut ours: Vec<(i32, i32)> = added
        .iter()
        .map(|component| (kind_id(component.kind()), component_hash::hash(component)))
        .collect();
    let mut theirs = hashed.added.clone();
    ours.sort_unstable();
    theirs.sort_unstable();
    let mut ours_removed: Vec<i32> = removed.iter().map(|kind| kind_id(*kind)).collect();
    let mut theirs_removed = hashed.removed.clone();
    ours_removed.sort_unstable();
    theirs_removed.sort_unstable();
    theirs_removed.dedup();
    ours == theirs && ours_removed == theirs_removed
}

/// The id of `kind` in the `data_component_type` registry.
fn kind_id(kind: Kind) -> i32 {
    let at = data_component_type::NAMES
        .iter()
        .position(|name| *name == kind.name())
        .expect("every kind Clustine knows is in the registry");
    i32::try_from(at).expect("the registry is small")
}

/// A click forwarded to the region, held until its answer.
#[derive(Debug, Clone)]
struct HeldClick {
    serial: u64,
    changed: Vec<(i16, Option<HashedStack>)>,
    carried: Option<HashedStack>,
    /// Whether the client clicked with a stale state id, which has the game send the
    /// window whole after the click.
    stale: bool,
}

/// The window the client shows.
#[derive(Debug, Clone)]
struct Open {
    window: i32,
    serial: u64,
    kind: MenuKind,
    real: Vec<Option<ItemStack>>,
    real_carried: Option<ItemStack>,
    remote: Vec<Remote>,
    remote_carried: Remote,
    state_id: i32,
}

impl Open {
    /// `incrementStateId`.
    fn next_state(&mut self) -> i32 {
        self.state_id = (self.state_id + 1) & 0x7FFF;
        self.state_id
    }

    /// The window whole (`sendAllDataToRemote`), after which the client shows the real
    /// slots.
    fn whole(&mut self) -> Bytes {
        let state_id = self.next_state();
        self.remote = self
            .real
            .iter()
            .map(|stack| Remote::Sent(stack.clone()))
            .collect();
        self.remote_carried = Remote::Sent(self.real_carried.clone());
        encoded(&SetContainerContent {
            window_id: self.window,
            state_id,
            slots: self
                .real
                .iter()
                .map(|stack| stack.as_ref().map(wire))
                .collect(),
            carried: self.real_carried.as_ref().map(wire),
        })
    }

    /// `broadcastChanges` over the slots `slots`: each the client does not show as it
    /// is, then the carried stack.
    fn changes(&mut self, slots: impl IntoIterator<Item = usize>) -> Vec<Bytes> {
        let mut packets = Vec::new();
        let mut slots: Vec<usize> = slots.into_iter().collect();
        slots.sort_unstable();
        slots.dedup();
        for slot in slots {
            let (Some(real), Some(remote)) = (self.real.get(slot).cloned(), self.remote.get(slot))
            else {
                continue;
            };
            if remote.shows(real.as_ref()) {
                continue;
            }
            let state_id = self.next_state();
            self.remote[slot] = Remote::Sent(real.clone());
            packets.push(encoded(&ContainerSetSlot {
                window_id: self.window,
                state_id,
                slot: i16::try_from(slot).expect("a menu has fewer than 32768 slots"),
                stack: real.as_ref().map(wire),
            }));
        }
        if !self.remote_carried.shows(self.real_carried.as_ref()) {
            self.remote_carried = Remote::Sent(self.real_carried.clone());
            packets.push(encoded(&SetCursorItem {
                stack: self.real_carried.as_ref().map(wire),
            }));
        }
        packets
    }
}

/// A player's menus as the edge keeps them.
#[derive(Debug, Default)]
pub(crate) struct Menus {
    /// The last window id given out (`containerCounter`), 1 to 100.
    counter: i32,
    /// The higher of the serial of the last menu opened and the number of the input of
    /// the last close of the client's: a menu event that reports less is older than
    /// what the client shows.
    threshold: u64,
    open: Option<Open>,
    held: BTreeMap<u64, HeldClick>,
}

impl Menus {
    /// The serial of the menu the client shows, if any.
    pub(crate) fn serial(&self) -> Option<u64> {
        self.open.as_ref().map(|open| open.serial)
    }

    /// The client's click, which will go to the region as the input numbered `number`:
    /// the input, or `None` for a click the game refuses here, of a window that is not
    /// the one shown (`handleContainerClick`'s `containerId` test).
    pub(crate) fn click(&mut self, packet: ContainerClick, number: u64) -> Option<PlayerInput> {
        let open = self
            .open
            .as_ref()
            .filter(|open| open.window == packet.window_id)?;
        let serial = open.serial;
        let stale = packet.state_id != open.state_id;
        self.held.insert(
            number,
            HeldClick {
                serial,
                changed: packet.changed,
                carried: packet.carried,
                stale,
            },
        );
        Some(PlayerInput::ContainerClick {
            serial,
            slot: packet.slot,
            button: packet.button,
            kind: click_kind(packet.input),
        })
    }

    /// The client closed the window `window`, which goes to the region as the input
    /// numbered `number`: the input, or `None` for a window that is not the one shown.
    /// The window is closed here at once; the game sends nothing back.
    pub(crate) fn close(&mut self, window: i32, number: u64) -> Option<PlayerInput> {
        let open = self.open.as_ref().filter(|open| open.window == window)?;
        let serial = open.serial;
        self.open = None;
        self.held.clear();
        self.threshold = self.threshold.max(number);
        Some(PlayerInput::CloseContainer { serial })
    }

    /// The client picked a trade in the window it shows (`select_trade`, which names no
    /// window): the input, which the region refuses for a menu that is not a merchant's,
    /// or `None` with no window shown.
    pub(crate) fn select_trade(&mut self, item: i32) -> Option<PlayerInput> {
        let open = self.open.as_ref()?;
        Some(PlayerInput::SelectTrade {
            serial: open.serial,
            index: item,
        })
    }

    /// The region reported the menu `serial` whole: opened, which `openMenu` shows by
    /// closing any other window, a new id, `open_screen` and the window whole; or of the
    /// menu shown, which is sent whole again.
    #[cfg(test)]
    pub(crate) fn opened(
        &mut self,
        serial: u64,
        applied: u64,
        kind: MenuKind,
        title: &str,
        slots: Vec<Option<ItemStack>>,
        carried: Option<ItemStack>,
    ) -> Vec<Bytes> {
        self.opened_with(serial, applied, kind, title, slots, carried, None)
    }

    /// [`Menus::opened`] where the menu may be a horse's: `mount` is the id the client
    /// knows the horse by, and its window opens by `mount_screen_open`
    /// (`ServerPlayer.openHorseInventory`) instead of `open_screen`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the opening's parts, as the region reports them, and the horse's id"
    )]
    pub(crate) fn opened_with(
        &mut self,
        serial: u64,
        applied: u64,
        kind: MenuKind,
        title: &str,
        slots: Vec<Option<ItemStack>>,
        carried: Option<ItemStack>,
        mount: Option<i32>,
    ) -> Vec<Bytes> {
        if applied < self.threshold {
            return Vec::new();
        }
        if let Some(open) = self.open.as_mut().filter(|open| open.serial == serial) {
            open.real = slots;
            open.real_carried = carried;
            self.held.retain(|number, _| *number > applied);
            return vec![open.whole()];
        }
        let mut packets = self.close_shown();
        self.threshold = self.threshold.max(serial);
        self.counter = self.counter % 100 + 1;
        let remote = vec![Remote::Sent(None); slots.len()];
        let mut open = Open {
            window: self.counter,
            serial,
            kind,
            real: slots,
            real_carried: carried,
            remote,
            remote_carried: Remote::Sent(None),
            state_id: 0,
        };
        match (kind, mount) {
            (MenuKind::Mount { columns, .. }, Some(entity_id)) => {
                packets.push(encoded(&MountScreenOpen {
                    window_id: open.window,
                    columns: i32::from(columns),
                    entity_id,
                }));
            }
            _ => packets.push(encoded(&OpenScreen {
                window_id: open.window,
                menu: menu_id(kind),
                title: Text::translatable(title).to_nbt(),
            })),
        }
        packets.push(open.whole());
        self.open = Some(open);
        packets
    }

    /// The region reported changes of the menu `serial`, answering every click it took
    /// up to the input `applied`: the clicks' hashes are taken as what the client shows,
    /// and what it does not show as the region has it is sent.
    pub(crate) fn changed(
        &mut self,
        serial: u64,
        applied: u64,
        slots: &[(u16, Option<ItemStack>)],
        carried: Option<Option<ItemStack>>,
    ) -> Vec<Bytes> {
        if applied < self.threshold {
            return Vec::new();
        }
        let Some(open) = self.open.as_mut().filter(|open| open.serial == serial) else {
            return Vec::new();
        };
        let mut touched: Vec<usize> = Vec::new();
        for (slot, stack) in slots {
            let slot = usize::from(*slot);
            if let Some(real) = open.real.get_mut(slot) {
                *real = stack.clone();
                touched.push(slot);
            }
        }
        if let Some(carried) = carried {
            open.real_carried = carried;
        }
        let answered: Vec<u64> = self
            .held
            .range(..=applied)
            .map(|(number, _)| *number)
            .collect();
        let mut stale = false;
        for number in answered {
            let held = self.held.remove(&number).expect("listed above");
            if held.serial != serial {
                continue;
            }
            stale |= held.stale;
            for (slot, hashed) in held.changed {
                if let Some(remote) = usize::try_from(slot)
                    .ok()
                    .and_then(|slot| open.remote.get_mut(slot))
                {
                    *remote = Remote::Hashed(hashed);
                    touched.push(usize::try_from(slot).expect("found above"));
                }
            }
            open.remote_carried = Remote::Hashed(held.carried);
        }
        if stale {
            // `broadcastFullState`: everything again, after which nothing differs.
            return vec![open.whole()];
        }
        open.changes(touched)
    }

    /// The region closed the menu `serial`: the window shown is closed
    /// (`container_close`) if it is that menu's.
    pub(crate) fn closed(&mut self, serial: u64, applied: u64) -> Vec<Bytes> {
        if applied < self.threshold || self.serial() != Some(serial) {
            return Vec::new();
        }
        self.threshold = self.threshold.max(applied);
        self.close_shown()
    }

    /// The player's region let them go: a hand-over always closes the menu, and the
    /// close it reported may have been lost on a link that resumed (section 7 item 7).
    pub(crate) fn departed(&mut self) -> Vec<Bytes> {
        self.close_shown()
    }

    /// The region answered a resync of a player with no menu: one still shown is
    /// closed. `None` for an answer older than what the client shows, which is passed
    /// over.
    pub(crate) fn none(&mut self, applied: u64) -> Option<Vec<Bytes>> {
        if applied < self.threshold {
            return None;
        }
        Some(self.close_shown())
    }

    /// The region changed the inventory slot `slot` of its own accord, which the edge
    /// sends with `set_player_inventory`: the menu that shows the slot has it too, in
    /// its copy and as the client shows it, so that it is not sent again.
    pub(crate) fn inventory_slot(&mut self, slot: usize, stack: Option<ItemStack>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(index) = menu_slot_of(open.kind, slot) else {
            return;
        };
        if let (Some(real), Some(remote)) = (open.real.get_mut(index), open.remote.get_mut(index)) {
            *real = stack.clone();
            *remote = Remote::Sent(stack);
        }
    }

    /// Closes the window shown, if any, as the server closes one.
    fn close_shown(&mut self) -> Vec<Bytes> {
        self.held.clear();
        match self.open.take() {
            Some(open) => vec![encoded(&ClientboundContainerClose {
                window_id: open.window,
            })],
            None => Vec::new(),
        }
    }
}

/// The menu slot that shows the inventory slot `slot` (0 to 35): the main inventory after
/// the container's slots, then the hotbar.
pub(crate) fn menu_slot_of(kind: MenuKind, slot: usize) -> Option<usize> {
    let first = kind.container_slots();
    match slot {
        0..=8 => Some(first + 27 + slot),
        9..=35 => Some(first + slot - 9),
        _ => None,
    }
}

/// The kind of click a client sent; the game reads any id it does not have as a pickup
/// (`ByIdMap`'s `ZERO`).
fn click_kind(input: i32) -> ClickKind {
    match input {
        click::QUICK_MOVE => ClickKind::QuickMove,
        click::SWAP => ClickKind::Swap,
        click::CLONE => ClickKind::Clone,
        click::THROW => ClickKind::Throw,
        click::QUICK_CRAFT => ClickKind::QuickCraft,
        click::PICKUP_ALL => ClickKind::PickupAll,
        _ => ClickKind::Pickup,
    }
}

fn menu_id(kind: MenuKind) -> i32 {
    match kind {
        MenuKind::Rows3 => menu::GENERIC_9X3,
        MenuKind::Rows6 => menu::GENERIC_9X6,
        MenuKind::ShulkerBox => menu::SHULKER_BOX,
        MenuKind::Grid3x3 => menu::GENERIC_3X3,
        MenuKind::Hopper => menu::HOPPER,
        // A horse's window is no menu type: it opens by `mount_screen_open`, and one whose
        // horse has no id is shown as three rows, which its slots fit no worse.
        MenuKind::Mount { .. } => menu::GENERIC_9X3,
        MenuKind::Merchant { .. } => menu::MERCHANT,
    }
}

fn wire(stack: &ItemStack) -> WireStack {
    crate::encode::wire_stack(stack)
}

fn encoded<P: packets::Packet>(packet: &P) -> Bytes {
    packets::encode(packet).into()
}

#[cfg(test)]
mod tests;
