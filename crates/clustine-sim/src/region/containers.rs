//! Container blocks (`docs/groundwork/n6-plan.md`; the brief's sections 3.1 to 3.9):
//! what a chest, a barrel, a shulker box, a dispenser, a dropper or a hopper keeps, read
//! from and written into its block entity's data, and what the game's blocks do about
//! it: the openers' count with its sounds, block events and rechecks, the barrel's
//! `open`, the trapped chest's signal, the double chest's halves, what a comparator
//! reads, and the spilling of what a container held when it goes. The shulker box's
//! lid is `shulker_boxes.rs`'s.
//!
//! The data is the one durable form, as the comparator's output is: its slots as the
//! game saves them (`Items`, a list of `{Slot, id, count, components}` with the empty
//! slots left out, each stack's components in the game's NBT form, `stack_nbt.rs`), whether a loot table is still to be unpacked, and the count of its openers,
//! which the game does not save and Clustine does, so that a region restored, split or
//! lent to knows what it last reported (section 7 item 5). The region reads a container
//! from it when it needs one and writes it back whole on every change. Every entry this
//! module does not own (a custom name, a lock, the components, a hopper's cooldown, a
//! loot table's name and seed) is kept as it came. A change is committed as any data
//! is, and not sent to the edges: the game never sends a container's data to a client
//! (section 6 item 12, finding 14).
//!
//! The menus open and close a container through [`Region::start_open`] and
//! [`Region::stop_open`], as the game's menu constructors and `removed` do, and change
//! its slots through [`Region::container_at`] and [`Region::set_container`].
//!
//! At a line (section 10): a chest does not pair with a chest in a chunk the region does
//! not hold, at its placement or at the shape update that crosses as an effect, so every
//! container is whole in one region; the recheck counts the region's own players, who
//! are the only ones whose menus show its containers; a comparator reads a container in
//! a copy from the copy's data. Named differences: the opener's game events
//! (`container_open`, `container_close`) are of no entity, where the game's are of the
//! player who opened or closed it, which a sensor reads only to pass on; the cat
//! sitting on a chest waits for N7; a loot table is not rolled (decision 4), so a structure's chest
//! spills nothing; a player's interaction range is a creative player's, which every
//! player of Clustine has; an opener's count does not go below 0 where the game's would.

use clustine_blocks::data::{facing, is_class, relative, same_block, set_flag, tick_id};
use clustine_blocks::growth::chest_connected_direction;
use clustine_data::{BlockClass, BlockState, Face, sounds};
use clustine_world::components::Value;
use clustine_world::{BlockPos, InChunk, Ticked, Vec3};

use super::Region;
use super::block_entities::{TAG_COMPOUND, TAG_END, TAG_INT, hash_map_order};
use super::block_entities::{nbt_string, skip_nbt};
use super::stack_nbt::{put_value, read_value, slot_of, slot_stack_value, stack_from_fields};
use crate::api::{ItemStack, MenuKind, RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::physics::aabb::Aabb;
use crate::state::{Menu, MenuOf};

const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;

/// The key of the openers' count, Clustine's own; written only while it is not 0.
const OPENERS: &str = "clustine:openers";

/// A container block's contents and openers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Container {
    /// Its slots, as many as its kind has ([`container_size`]).
    pub(crate) slots: Vec<Option<ItemStack>>,
    /// The players who have it open, as its `ContainerOpenersCounter` last counted.
    pub(crate) openers: u32,
    /// Whether a loot table is still set (`LootTable`): its slots were then not read,
    /// and are written only once [`Container::unpack`] has cleared it.
    pub(crate) loot: bool,
}

impl Container {
    /// What the game's `unpackLootTable` does to the data, without the loot, which N6
    /// does not have (decision 4): the table is gone, and the slots are saved from now
    /// on.
    pub(crate) fn unpack(&mut self) {
        self.loot = false;
    }
}

/// The slots of the container that the block `state` is, if it is one whose block entity
/// holds items: 27 for a chest of any kind, a barrel or a shulker box, 9 for a dispenser
/// or a dropper, 5 for a hopper. The ender chest's slots are the player's.
pub(crate) fn container_size(state: BlockState) -> Option<usize> {
    if is_class(state, BlockClass::ChestBlock)
        || is_class(state, BlockClass::BarrelBlock)
        || is_class(state, BlockClass::ShulkerBoxBlock)
    {
        Some(27)
    } else if is_class(state, BlockClass::DispenserBlock) {
        Some(9)
    } else if is_class(state, BlockClass::HopperBlock) {
        Some(5)
    } else {
        None
    }
}

/// Whether the game saves the slots of the block `state` even when they are all empty
/// (`ContainerHelper.saveAllItems`'s `alwaysPutTag`): every container but a shulker box.
fn always_saves_items(state: BlockState) -> bool {
    !is_class(state, BlockClass::ShulkerBoxBlock)
}

/// The container that `data`, the block entity's data of a block of `state`, describes
/// (`loadAdditional`): `size` slots, those within it of the `Items` entries set, unless
/// a loot table is set; an entry whose item the game does not have, or whose count is
/// not positive, is an empty slot.
pub(crate) fn read_container(size: usize, data: &[u8]) -> Container {
    let mut container = Container {
        slots: vec![None; size],
        openers: 0,
        loot: false,
    };
    let Some((&TAG_COMPOUND, rest)) = data.split_first() else {
        return container;
    };
    let mut items = None;
    let _ = each_raw(rest, |tag, name, payload| match (tag, name) {
        (TAG_STRING, b"LootTable") => container.loot = true,
        (TAG_LIST, b"Items") => items = Some(payload),
        (TAG_INT, name) if name == OPENERS.as_bytes() => {
            container.openers = int(payload).map_or(0, |count| count.max(0).unsigned_abs());
        }
        _ => {}
    });
    if container.loot {
        return container;
    }
    if let Some(list) = items {
        read_items(list, &mut container.slots);
    }
    container
}

/// Sets the slots `slots` from the payload of an `Items` list, each entry as
/// `ItemStackWithSlot.CODEC` reads it (`stack_nbt.rs`): an entry that is not a
/// compound, whose slot is beyond `slots`, or whose stack does not read, is passed over,
/// as the game passes over an entry its codec refuses. What of a stack's components
/// Clustine does not keep is dropped, the stack kept ([`stack_from_fields`]).
fn read_items(list: &[u8], slots: &mut [Option<ItemStack>]) {
    let Some((Value::List(entries), _)) = read_value(TAG_LIST, list) else {
        return;
    };
    for entry in &entries {
        let Value::Map(fields) = entry else {
            continue;
        };
        if let (Some(slot), Some(stack)) = (slot_of(fields), stack_from_fields(fields))
            && let Some(place) = slots.get_mut(slot)
        {
            *place = Some(stack);
        }
    }
}

/// The data of the block entity of a block of `state` holding `container`, from its data
/// before, `data`: every entry kept as it was but the slots, the loot table's (gone once
/// unpacked) and the openers', which are written anew. As the game writes a compound,
/// in the order of the names' buckets, those before in the order they came.
pub(crate) fn container_data(state: BlockState, container: &Container, data: &[u8]) -> Vec<u8> {
    let mut entries: Entries = Vec::new();
    if let Some((&TAG_COMPOUND, rest)) = data.split_first() {
        let _ = each_raw(rest, |tag, name, payload| {
            let ours = name == b"Items"
                || name == OPENERS.as_bytes()
                || (!container.loot && (name == b"LootTable" || name == b"LootTableSeed"));
            if !ours {
                entries.push((tag, name.to_vec(), payload.to_vec()));
            }
        });
    }
    if !container.loot {
        let filled: Vec<(usize, ItemStack)> = container
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, stack)| Some((slot, stack.clone()?)))
            .filter(|(_, stack)| stack.count > 0)
            .collect();
        if !filled.is_empty() || always_saves_items(state) {
            entries.push((TAG_LIST, b"Items".to_vec(), items_payload(&filled)));
        }
    }
    write_compound(entries, container.openers)
}

/// The entries of a compound as they are written: tag, name and payload.
type Entries = Vec<(u8, Vec<u8>, Vec<u8>)>;

/// The compound of `entries`, with the openers' count `openers` where it is not 0 and
/// the empty compound of components where they have none, as the game writes a
/// compound: in the order of its `HashMap` ([`hash_map_order`]), those before put in
/// the order they came.
fn write_compound(mut entries: Entries, openers: u32) -> Vec<u8> {
    if openers > 0 {
        let count = i32::try_from(openers).unwrap_or(i32::MAX);
        entries.push((
            TAG_INT,
            OPENERS.as_bytes().to_vec(),
            count.to_be_bytes().to_vec(),
        ));
    }
    if !entries.iter().any(|(_, name, _)| name == b"components") {
        entries.push((TAG_COMPOUND, b"components".to_vec(), vec![TAG_END]));
    }
    let names: Vec<String> = entries
        .iter()
        .map(|(_, name, _)| String::from_utf8_lossy(name).into_owned())
        .collect();
    let mut entries: Vec<Option<_>> = entries.into_iter().map(Some).collect();
    let mut written = vec![TAG_COMPOUND];
    for (tag, name, payload) in hash_map_order(&names)
        .into_iter()
        .filter_map(|index| entries[index].take())
    {
        written.push(tag);
        let length = u16::try_from(name.len()).expect("a name read from a short string");
        written.extend_from_slice(&length.to_be_bytes());
        written.extend_from_slice(&name);
        written.extend_from_slice(&payload);
    }
    written.push(TAG_END);
    written
}

/// The openers' count that `data`, a block entity's data, keeps: 0 where it keeps none.
pub(crate) fn read_openers(data: &[u8]) -> u32 {
    let Some((&TAG_COMPOUND, rest)) = data.split_first() else {
        return 0;
    };
    let mut openers = 0;
    let _ = each_raw(rest, |tag, name, payload| {
        if tag == TAG_INT && name == OPENERS.as_bytes() {
            openers = int(payload).map_or(0, |count| count.max(0).unsigned_abs());
        }
    });
    openers
}

/// `data`, a block entity's data, with the openers' count `openers` and every other
/// entry as it was: what an ender chest keeps, which has no slots, and what any
/// container's opening changes.
pub(crate) fn with_openers(data: &[u8], openers: u32) -> Vec<u8> {
    let mut entries: Entries = Vec::new();
    if let Some((&TAG_COMPOUND, rest)) = data.split_first() {
        let _ = each_raw(rest, |tag, name, payload| {
            if name != OPENERS.as_bytes() {
                entries.push((tag, name.to_vec(), payload.to_vec()));
            }
        });
    }
    write_compound(entries, openers)
}

/// The whole number `name` of `data`, a block entity's data, if it keeps one: a
/// hopper's `TransferCooldown` (`hoppers.rs`).
pub(crate) fn read_int(data: &[u8], name: &str) -> Option<i32> {
    let (&TAG_COMPOUND, rest) = data.split_first()? else {
        return None;
    };
    let mut found = None;
    let _ = each_raw(rest, |tag, entry, payload| {
        if tag == TAG_INT && entry == name.as_bytes() {
            found = int(payload);
        }
    });
    found
}

/// `data`, a block entity's data, with the whole number `name` set to `value` and every
/// other entry as it was: what a hopper's cooldown changes.
pub(crate) fn with_int(data: &[u8], name: &str, value: i32) -> Vec<u8> {
    let mut entries: Entries = Vec::new();
    if let Some((&TAG_COMPOUND, rest)) = data.split_first() {
        let _ = each_raw(rest, |tag, entry, payload| {
            if entry != name.as_bytes() {
                entries.push((tag, entry.to_vec(), payload.to_vec()));
            }
        });
    }
    entries.push((
        TAG_INT,
        name.as_bytes().to_vec(),
        value.to_be_bytes().to_vec(),
    ));
    write_compound(entries, 0)
}

/// The payload of an `Items` list of `filled`: compounds of `Slot`, `id`, `count` and,
/// where a stack has any, `components`, as `ItemStackWithSlot.CODEC` writes them with
/// `NbtOps` (`stack_nbt.rs`), so that a stack keeps its components at a save
/// (`docs/groundwork/n9-plan.md`, section 3.2).
///
/// The list's element type is a compound even where it is empty, as Clustine has
/// always written it; the game's `ListTag` writes an empty list's as `TAG_END`. Either
/// reads as an empty list.
fn items_payload(filled: &[(usize, ItemStack)]) -> Vec<u8> {
    let mut payload = vec![TAG_COMPOUND];
    let count = i32::try_from(filled.len()).expect("fewer slots than an int counts");
    payload.extend_from_slice(&count.to_be_bytes());
    for (slot, stack) in filled {
        let slot = u8::try_from(*slot).expect("a slot below 256");
        put_value(&mut payload, &slot_stack_value(slot, stack));
    }
    payload
}

/// Calls `each` with the tag, the name and the payload of every entry of the compound
/// whose entries begin `data`, and returns what follows its end; `None` for data cut
/// short.
fn each_raw<'a>(data: &'a [u8], mut each: impl FnMut(u8, &[u8], &'a [u8])) -> Option<&'a [u8]> {
    let mut rest = data;
    loop {
        let (&tag, after) = rest.split_first()?;
        if tag == TAG_END {
            return Some(after);
        }
        let (name, payload) = nbt_string(after)?;
        let after = skip_nbt(tag, payload, 1)?;
        each(tag, name, &payload[..payload.len() - after.len()]);
        rest = after;
    }
}

fn int(payload: &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(payload.get(..4)?.try_into().ok()?))
}

/// `ChestBlock.EVENT_SET_OPEN_COUNT`: the block event of a chest's, an ender chest's or
/// a shulker box's openers, whose number is their count.
pub(crate) const SET_OPEN_COUNT: i32 = 1;

/// `ContainerOpenersCounter.CHECK_TICK_DELAY`: how often a chest, a barrel or an ender
/// chest with openers counts them again.
const CHECK_TICK_DELAY: i32 = 5;

/// A creative player's `block_interaction_range` (4.5, and the creative 0.5), which
/// every player of Clustine has: the recheck finds openers within it and four more of
/// the block (`ContainerOpenersCounter.getEntitiesWithContainerOpen`).
const INTERACTION_RANGE: f64 = 5.0;

/// The volume of every container's opening and closing sound.
const OPENING_VOLUME: f32 = 0.5;

/// The two halves of a chest, as `ChestBlock.combine` finds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChestHalves {
    /// The single chest, or the right half of a double chest (`FIRST`), whose slots are
    /// the double chest's 0 to 26.
    pub(crate) first: BlockPos,
    /// The left half of a double chest (`SECOND`).
    pub(crate) second: Option<BlockPos>,
    /// Whether a redstone conductor above either half keeps it shut
    /// (`isChestBlockedAt`): it opens nothing and a comparator reads 0 from it. A cat
    /// sitting on it waits for N7.
    pub(crate) blocked: bool,
}

/// The containers that count their openers with a `ContainerOpenersCounter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Counted {
    /// A chest of any kind: a trapped chest, a copper chest.
    Chest,
    Barrel,
    EnderChest,
}

impl Counted {
    fn of(state: BlockState) -> Option<Counted> {
        if is_class(state, BlockClass::ChestBlock) {
            Some(Counted::Chest)
        } else if is_class(state, BlockClass::BarrelBlock) {
            Some(Counted::Barrel)
        } else if is_class(state, BlockClass::EnderChestBlock) {
            Some(Counted::EnderChest)
        } else {
            None
        }
    }
}

/// The type of the block entity a block of `state` keeps, if it keeps one; a block
/// changed to one of the same type keeps its block entity
/// (`shouldChangedStateKeepBlockEntity`), as a copper chest that weathers does.
fn block_entity_kind(state: BlockState) -> Option<u8> {
    state.block_info().and_then(|info| info.block_entity_type)
}

/// `AbstractContainerMenu.getRedstoneSignalFromContainer` of a container with `slots`:
/// how full it is, each slot by its stack's most (at most 99), scaled to 15 and 1 for
/// anything at all, in `float` as the game works it.
pub(crate) fn redstone_signal_of(slots: &[Option<ItemStack>]) -> u8 {
    if slots.is_empty() {
        return 0;
    }
    let mut full = 0.0_f32;
    for stack in slots.iter().flatten() {
        if stack.count > 0 && stack.item != clustine_data::items::AIR {
            let most = crate::physics::item::max_stack_size(stack).min(99);
            full += stack.count as f32 / most as f32;
        }
    }
    full /= slots.len() as f32;
    // `Mth.lerpDiscrete(full, 0, 15)`.
    let steps = (full * 14.0).floor() as u8;
    steps + u8::from(full > 0.0)
}

/// Whether `menu` shows the container block at `pos` that counts its openers as `kind`
/// (the counters' `isOwnContainer`): a chest's menu over it or over the double chest it
/// is half of, a barrel's over it, or the player's ender chest opened at it.
fn shows(menu: &Menu, pos: BlockPos, kind: Counted) -> bool {
    match (kind, menu.of) {
        (Counted::Chest, MenuOf::Block { at, other }) => {
            matches!(menu.kind, MenuKind::Rows3 | MenuKind::Rows6)
                && (at == pos || other == Some(pos))
        }
        (Counted::Barrel, MenuOf::Block { at, .. }) => menu.kind == MenuKind::Rows3 && at == pos,
        (Counted::EnderChest, MenuOf::Ender { at }) => at == pos,
        _ => false,
    }
}

impl Region {
    /// The data of the block entity at `pos`, in a chunk loaded or copied.
    pub(crate) fn block_entity_data(&self, pos: BlockPos) -> Option<&[u8]> {
        let chunk = self.readable_chunk(pos.chunk())?;
        let (x, z) = pos.in_chunk();
        chunk
            .block_entities()
            .get(&InChunk::new(x, pos.y, z))
            .map(|entity| entity.data.as_slice())
    }

    /// The data of the block entity at `pos`, in a chunk loaded: what the region may
    /// change.
    pub(super) fn own_block_entity_data(&self, pos: BlockPos) -> Option<&[u8]> {
        let chunk = self.chunks.get(&pos.chunk())?;
        let (x, z) = pos.in_chunk();
        chunk
            .block_entities()
            .get(&InChunk::new(x, pos.y, z))
            .map(|entity| entity.data.as_slice())
    }

    /// The container block at `pos`, read from its block entity's data, if it is one: of
    /// a chunk loaded, or of a copy, which a comparator across a line reads.
    pub(crate) fn container_at(&self, pos: BlockPos) -> Option<Container> {
        let state = self.block(pos)?;
        let size = container_size(state)?;
        Some(read_container(size, self.block_entity_data(pos)?))
    }

    /// Writes `container` back into the block entity at `pos`, of a chunk loaded, as
    /// `setChanged` does: the data, committed and not sent to the edges, and the
    /// comparators reading it told (`updateNeighbourForOutputSignal`). A container of
    /// another size than the block's is not written.
    pub(crate) fn set_container(
        &mut self,
        pos: BlockPos,
        container: Container,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if container_size(state) != Some(container.slots.len()) {
            return;
        }
        let Some(before) = self.own_block_entity_data(pos) else {
            return;
        };
        let data = container_data(state, &container, before);
        if data.as_slice() != before {
            self.set_block_entity_data_as(pos, data, false, output);
        }
        self.update_neighbour_for_output_signal(pos, state, output);
    }

    /// The openers' count the block entity at `pos` keeps, of a chunk loaded or copied.
    pub(super) fn openers(&self, pos: BlockPos) -> u32 {
        self.block_entity_data(pos).map_or(0, read_openers)
    }

    /// Keeps `openers` as the openers' count of the block entity at `pos`, of a chunk
    /// loaded: committed and not sent to the edges.
    pub(super) fn set_openers(&mut self, pos: BlockPos, openers: u32, output: &mut TickOutput) {
        let Some(before) = self.own_block_entity_data(pos) else {
            return;
        };
        if read_openers(before) == openers {
            return;
        }
        let data = with_openers(before, openers);
        self.set_block_entity_data_as(pos, data, false, output);
    }

    /// `ChestBlock.combine(state, level, pos, false)`: the halves of the chest at `pos`,
    /// the right (first) half first, and whether it is blocked from opening. Its other
    /// half is the chest of the same block beside it towards its connected direction,
    /// of the other type and facing the same way, with its block entity; otherwise it
    /// is single, whatever its type says. Of a chunk loaded or copied; `None` where
    /// there is no chest with its block entity.
    pub(crate) fn chest_halves(&self, pos: BlockPos) -> Option<ChestHalves> {
        let state = self.block(pos)?;
        if !is_class(state, BlockClass::ChestBlock) {
            return None;
        }
        self.block_entity_data(pos)?;
        let blocked = self.chest_blocked_at(pos);
        let single = ChestHalves {
            first: pos,
            second: None,
            blocked,
        };
        let kind = state.property("type");
        if !matches!(kind, Some("left" | "right")) {
            return Some(single);
        }
        let Some(towards) = chest_connected_direction(state) else {
            return Some(single);
        };
        let partner = relative(pos, towards);
        let joins = self.block(partner).is_some_and(|other| {
            same_block(other, state)
                && matches!(other.property("type"), Some("left" | "right"))
                && other.property("type") != kind
                && facing(other) == facing(state)
        }) && self.block_entity_data(partner).is_some();
        if !joins {
            return Some(single);
        }
        let blocked = blocked || self.chest_blocked_at(partner);
        let (first, second) = if kind == Some("right") {
            (pos, partner)
        } else {
            (partner, pos)
        };
        Some(ChestHalves {
            first,
            second: Some(second),
            blocked,
        })
    }

    /// `ChestBlock.isBlockedChestByBlock`: a redstone conductor stands above `pos`.
    fn chest_blocked_at(&self, pos: BlockPos) -> bool {
        self.block(relative(pos, Face::Up))
            .is_some_and(clustine_blocks::connect::is_conductor)
    }

    /// `startOpen` of the container block at `pos` by a player, as a menu's constructor
    /// calls it: a chest of any kind, a barrel or an ender chest counts the opener
    /// (`ContainerOpenersCounter.incrementOpeners`), and a shulker box its own way
    /// (`shulker_boxes.rs`); a dispenser, a dropper or a hopper counts nobody. A double
    /// chest's menu opens both halves, the right one first (`CompoundContainer`).
    pub(crate) fn start_open(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if is_class(state, BlockClass::ShulkerBoxBlock) {
            self.shulker_box_start_open(pos, state, output);
            return;
        }
        let Some(kind) = Counted::of(state) else {
            return;
        };
        let before = self.openers(pos);
        let now = before.saturating_add(1);
        self.set_openers(pos, now, output);
        if before == 0 {
            self.on_open_or_close(pos, kind, true, output);
            self.schedule_tick(pos, Ticked::Block(tick_id(state)), CHECK_TICK_DELAY, 0);
        }
        self.opener_count_changed(pos, kind, before, now, output);
    }

    /// `stopOpen` of the container block at `pos` by a player, as a menu's `removed`
    /// calls it (`ContainerOpenersCounter.decrementOpeners`, and the shulker box's). A
    /// count already at 0 stays there, where the game's would go below.
    pub(crate) fn stop_open(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if is_class(state, BlockClass::ShulkerBoxBlock) {
            self.shulker_box_stop_open(pos, state, output);
            return;
        }
        let Some(kind) = Counted::of(state) else {
            return;
        };
        let before = self.openers(pos);
        if before == 0 {
            return;
        }
        let now = before - 1;
        self.set_openers(pos, now, output);
        if now == 0 {
            self.on_open_or_close(pos, kind, false, output);
        }
        self.opener_count_changed(pos, kind, before, now, output);
    }

    /// The block tick of a chest, a barrel or an ender chest (`ChestBlock.tick`,
    /// `BarrelBlock.tick`, `EnderChestBlock.tick`): its openers counted again. Whether
    /// `state` is one of them.
    pub(super) fn container_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        let Some(kind) = Counted::of(state) else {
            return false;
        };
        if self.own_block_entity_data(pos).is_some() {
            self.recheck_openers(pos, state, kind, output);
        }
        true
    }

    /// `ContainerOpenersCounter.recheckOpeners`: the region's players whose menus show
    /// the container and who are within reach of it are its openers. A count that
    /// changes opens or closes it, as a player would; the count is said again either
    /// way (a chest's block event every five ticks while it is open), and the next
    /// recheck is scheduled while anyone is left. As the count is saved, a region
    /// restored, split or merged heals through this.
    fn recheck_openers(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        kind: Counted,
        output: &mut TickOutput,
    ) {
        let found = self.openers_found(pos, kind);
        let before = self.openers(pos);
        if before != found {
            if found > 0 && before == 0 {
                self.on_open_or_close(pos, kind, true, output);
            } else if found == 0 {
                self.on_open_or_close(pos, kind, false, output);
            }
            self.set_openers(pos, found, output);
        }
        self.opener_count_changed(pos, kind, before, found, output);
        if found > 0 {
            self.schedule_tick(pos, Ticked::Block(tick_id(state)), CHECK_TICK_DELAY, 0);
        }
    }

    /// The region's players whose menus show the container at `pos` and whose boxes
    /// meet the block's, grown by their reach and four more.
    fn openers_found(&self, pos: BlockPos, kind: Counted) -> u32 {
        let reach = INTERACTION_RANGE + 4.0;
        let area = Aabb::of_block(pos).inflate([reach; 3]);
        let found = self
            .players
            .values()
            .filter(|player| {
                player
                    .menu
                    .as_ref()
                    .is_some_and(|menu| shows(menu, pos, kind))
            })
            .filter(|player| {
                let [x, y, z] = [
                    player.pose.position.x,
                    player.pose.position.y,
                    player.pose.position.z,
                ];
                let half = super::PLAYER_WIDTH / 2.0;
                Aabb::new(
                    [x - half, y, z - half],
                    [x + half, y + super::PLAYER_HEIGHT, z + half],
                )
                .intersects(area)
            })
            .count();
        // A copper golem at work at the chest counts too (`copper_golems.rs`).
        let golems = match kind {
            Counted::Chest => self.golems_with_chest_open(pos),
            Counted::Barrel | Counted::EnderChest => 0,
        };
        u32::try_from(found)
            .unwrap_or(u32::MAX)
            .saturating_add(golems)
    }

    /// The counters' `onOpen` and `onClose`: a chest's and an ender chest's sound, and
    /// a barrel's, which then sets its `open` with every update (`setBlockAndUpdate`).
    fn on_open_or_close(
        &mut self,
        pos: BlockPos,
        kind: Counted,
        open: bool,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        match kind {
            Counted::Chest => self.chest_sound(pos, state, open, output),
            Counted::EnderChest => {
                let sound = if open {
                    sounds::BLOCK_ENDER_CHEST_OPEN
                } else {
                    sounds::BLOCK_ENDER_CHEST_CLOSE
                };
                self.container_sound(centre, sound, output);
            }
            Counted::Barrel => {
                // `BarrelBlockEntity.playSound`: at the middle of the face it faces.
                let [dx, dy, dz] =
                    crate::physics::piston::step(facing(state).unwrap_or(Face::North));
                let at = [
                    centre[0] + f64::from(dx) / 2.0,
                    centre[1] + f64::from(dy) / 2.0,
                    centre[2] + f64::from(dz) / 2.0,
                ];
                let sound = if open {
                    sounds::BLOCK_BARREL_OPEN
                } else {
                    sounds::BLOCK_BARREL_CLOSE
                };
                self.container_sound(at, sound, output);
                self.set_block(pos, set_flag(state, "open", open), output);
            }
        }
        // `incrementOpeners`, `decrementOpeners` and `recheckOpeners` post their event
        // after `onOpen` and `onClose` (`vibrations.rs`).
        self.container_vibrates(pos, open);
    }

    /// `container_open` or `container_close` at the container at `pos`.
    pub(super) fn container_vibrates(&mut self, pos: BlockPos, open: bool) {
        let event = if open {
            super::vibrations::Vibration::CONTAINER_OPEN
        } else {
            super::vibrations::Vibration::CONTAINER_CLOSE
        };
        self.vibrate_at(event, pos, super::vibrations::Context::default());
    }

    /// The counters' `openerCountChanged`: a chest's and an ender chest's block event
    /// of the count, which clients open and shut the lid by; a trapped chest whose count
    /// changed tells its neighbours and those of the block below, which it powers.
    fn opener_count_changed(
        &mut self,
        pos: BlockPos,
        kind: Counted,
        before: u32,
        now: u32,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        let count = i32::try_from(now).unwrap_or(i32::MAX);
        match kind {
            Counted::Chest => {
                self.block_event(pos, state, SET_OPEN_COUNT, count);
                if is_class(state, BlockClass::TrappedChestBlock) && before != now {
                    self.update_neighbours_at(pos, state, output);
                    self.update_neighbours_at(relative(pos, Face::Down), state, output);
                }
            }
            Counted::EnderChest => self.block_event(pos, state, SET_OPEN_COUNT, count),
            Counted::Barrel => {}
        }
    }

    /// `ChestBlockEntity.playSound`: a single chest's at its centre, a double chest's at
    /// the centre of the pair from its right half, the left half silent.
    fn chest_sound(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        open: bool,
        output: &mut TickOutput,
    ) {
        let kind = state.property("type");
        if kind == Some("left") {
            return;
        }
        let mut at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        if kind == Some("right")
            && let Some(towards) = chest_connected_direction(state)
        {
            let [dx, _, dz] = crate::physics::piston::step(towards);
            at[0] += f64::from(dx) * 0.5;
            at[2] += f64::from(dz) * 0.5;
        }
        let (opening, closing) = chest_sounds(state);
        self.container_sound(at, if open { opening } else { closing }, output);
    }

    /// `Level.playSound` of a container's opening or closing: from the blocks, at half
    /// the volume, a little lower at random (`nextFloat() * 0.1f + 0.9f`).
    pub(super) fn container_sound(&mut self, at: [f64; 3], sound: i32, output: &mut TickOutput) {
        let pitch = self.next_draw().next_float() * 0.1 + 0.9;
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_BLOCKS,
            position: Vec3::new(at[0], at[1], at[2]),
            volume: OPENING_VOLUME,
            pitch,
            except: None,
        });
    }

    /// `ChestBlock.triggerEvent` and the ender chest's, and the shulker box's
    /// (`shulker_boxes.rs`): the count's block event runs, and is sent, where the block
    /// entity is; the lid of a chest is the client's alone. `None` for a block that is
    /// none of them.
    pub(super) fn container_event(
        &mut self,
        at: BlockPos,
        state: BlockState,
        kind: i32,
        data: i32,
    ) -> Option<bool> {
        if is_class(state, BlockClass::ShulkerBoxBlock) {
            return Some(self.shulker_box_event(at, kind, data));
        }
        if is_class(state, BlockClass::ChestBlock) || is_class(state, BlockClass::EnderChestBlock) {
            return Some(kind == SET_OPEN_COUNT && self.own_block_entity_data(at).is_some());
        }
        None
    }

    /// How many players have the trapped chest at `at` open
    /// (`ChestBlockEntity.getOpenCount`), at most 15: its signal. 0 for any other block.
    pub(crate) fn trapped_chest_openers(&self, at: BlockPos) -> u8 {
        if !self
            .block(at)
            .is_some_and(|state| is_class(state, BlockClass::TrappedChestBlock))
        {
            return 0;
        }
        u8::try_from(self.openers(at).min(15)).expect("held to a signal")
    }

    /// `getAnalogOutputSignal` of a container block, as a comparator reads it: a chest
    /// of the slots of both its halves (`getContainer(…, false)`), 0 where it is
    /// blocked; a barrel, a shulker box, a dispenser, a dropper or a hopper of its own
    /// (`getRedstoneSignalFromBlockEntity`). `None` for a block that is none of them.
    pub(crate) fn container_output(&self, pos: BlockPos, state: BlockState) -> Option<u8> {
        if is_class(state, BlockClass::ChestBlock) {
            let Some(halves) = self.chest_halves(pos) else {
                return Some(0);
            };
            if halves.blocked {
                return Some(0);
            }
            let mut slots = Vec::new();
            for half in std::iter::once(halves.first).chain(halves.second) {
                slots.extend(self.container_at(half).map(|c| c.slots).unwrap_or_default());
            }
            return Some(redstone_signal_of(&slots));
        }
        container_size(state)?;
        Some(
            self.container_at(pos)
                .map_or(0, |container| redstone_signal_of(&container.slots)),
        )
    }

    /// What a container block that goes from `pos` holds, to be spilled
    /// (`BlockEntity.preRemoveSideEffects`): where `old` is a container and `new` does
    /// not keep its block entity, unless `flags` say not to. Read before the chunk lets
    /// the data go.
    pub(super) fn container_to_spill(
        &self,
        pos: BlockPos,
        old: BlockState,
        new: BlockState,
        flags: u32,
    ) -> Option<Container> {
        if flags & super::updates::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS != 0 {
            return None;
        }
        let size = container_size(old)?;
        if block_entity_kind(old) == block_entity_kind(new) {
            return None;
        }
        Some(read_container(size, self.own_block_entity_data(pos)?))
    }

    /// `Containers.dropContents` of a container that went from `pos`: each slot in
    /// order, from the corner of the block, with the draws of the block's stream. A
    /// shulker box spills as a chest does until stacks have components (section 6 item
    /// 5); a creative player's break spills too.
    pub(super) fn spill_container(
        &mut self,
        pos: BlockPos,
        container: Container,
        output: &mut TickOutput,
    ) {
        let contents: clustine_world::Contents = container
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, stack)| Some((u8::try_from(slot).ok()?, stack.clone()?)))
            .collect();
        let corner = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        self.drop_contents(corner, &contents, container.slots.len(), output);
    }

    /// `Containers.updateNeighboursAfterDestroy` of a container block that went from
    /// `pos` (its `affectNeighborsAfterRemoval`): the comparators reading it are told.
    pub(super) fn container_after_removal(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if container_size(old).is_some() || is_class(old, BlockClass::CrafterBlock) {
            self.update_neighbour_for_output_signal(pos, old, output);
        }
    }

    /// Whether the shape update of the chest at `pos`, from its neighbour towards
    /// `direction`, that would turn it from `old` into `new` is to leave it as it is: a
    /// single chest does not join a chest in a chunk the region does not hold, so that
    /// two chests across a line stay single (`docs/groundwork/n6-plan.md`, section 10
    /// item 2).
    pub(super) fn keeps_chest_single(
        &self,
        pos: BlockPos,
        direction: Face,
        old: BlockState,
        new: BlockState,
    ) -> bool {
        is_class(old, BlockClass::ChestBlock)
            && old.property("type") == Some("single")
            && new.property("type") != Some("single")
            && !self.chunks.contains_key(&relative(pos, direction).chunk())
    }
}

/// The opening and closing sounds of a chest of `state`'s block: a copper chest's by how
/// weathered it is, the weathered and the oxidised ones of their own and the rest the
/// plain copper ones (`CopperChestBlock.getHingeSound`); any other chest's the chest's.
fn chest_sounds(state: BlockState) -> (i32, i32) {
    if !is_class(state, BlockClass::CopperChestBlock) {
        return (sounds::BLOCK_CHEST_OPEN, sounds::BLOCK_CHEST_CLOSE);
    }
    let name = state.block().map_or("", |block| block.name);
    if name.contains("weathered") {
        (
            sounds::BLOCK_COPPER_CHEST_WEATHERED_OPEN,
            sounds::BLOCK_COPPER_CHEST_WEATHERED_CLOSE,
        )
    } else if name.contains("oxidized") {
        (
            sounds::BLOCK_COPPER_CHEST_OXIDIZED_OPEN,
            sounds::BLOCK_COPPER_CHEST_OXIDIZED_CLOSE,
        )
    } else {
        (
            sounds::BLOCK_COPPER_CHEST_OPEN,
            sounds::BLOCK_COPPER_CHEST_CLOSE,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::block_entities::{TAG_BYTE, nbt_put_string};
    use clustine_data::{blocks, items};

    fn stack(item: i32, count: i32) -> Option<ItemStack> {
        Some(ItemStack::new(item, count))
    }

    #[test]
    fn a_containers_slots_and_openers_read_back_as_they_were_written() {
        let mut container = read_container(27, &[]);
        container.slots[0] = stack(items::STONE, 64);
        container.slots[13] = stack(items::DIAMOND, 1);
        container.slots[26] = stack(items::OAK_LOG, 7);
        container.openers = 2;
        let data = container_data(blocks::CHEST, &container, &[]);
        assert_eq!(read_container(27, &data), container);

        // The last opener gone, the count is written no more.
        container.openers = 0;
        let data = container_data(blocks::CHEST, &container, &data);
        assert_eq!(read_container(27, &data), container);
        assert!(
            !data
                .windows(OPENERS.len())
                .any(|window| window == OPENERS.as_bytes())
        );
    }

    #[test]
    fn an_empty_chest_saves_an_empty_list_and_an_empty_shulker_box_none() {
        let empty = read_container(27, &[]);
        let chest = container_data(blocks::CHEST, &empty, &[]);
        let shulker = container_data(blocks::SHULKER_BOX, &empty, &[]);
        assert!(chest.windows(5).any(|window| window == b"Items"));
        assert!(!shulker.windows(5).any(|window| window == b"Items"));
        assert_eq!(read_container(27, &chest), empty);
        assert_eq!(read_container(27, &shulker), empty);
    }

    #[test]
    fn entries_the_container_does_not_own_are_kept_as_they_came() {
        // A hopper of a structure: its cooldown and its components, then its slots.
        let mut before = vec![TAG_COMPOUND];
        before.push(TAG_INT);
        nbt_put_string(&mut before, "TransferCooldown");
        before.extend_from_slice(&(-1i32).to_be_bytes());
        before.push(TAG_COMPOUND);
        nbt_put_string(&mut before, "components");
        before.push(TAG_END);
        before.push(TAG_END);
        let mut hopper = read_container(5, &before);
        hopper.slots[4] = stack(items::REDSTONE, 3);
        let after = container_data(blocks::HOPPER, &hopper, &before);
        assert_eq!(read_container(5, &after), hopper);
        let mut cooldown = None;
        let _ = each_raw(&after[1..], |tag, name, payload| {
            if tag == TAG_INT && name == b"TransferCooldown" {
                cooldown = int(payload);
            }
        });
        assert_eq!(cooldown, Some(-1));
    }

    #[test]
    fn a_loot_table_keeps_the_slots_unread_until_it_is_unpacked() {
        let mut before = vec![TAG_COMPOUND, TAG_STRING];
        nbt_put_string(&mut before, "LootTable");
        nbt_put_string(&mut before, "minecraft:chests/simple_dungeon");
        before.push(TAG_END);
        let mut chest = read_container(27, &before);
        assert!(chest.loot);
        assert_eq!(chest.slots, vec![None; 27]);
        // Written back untouched, the table stays.
        let kept = container_data(blocks::CHEST, &chest, &before);
        assert!(read_container(27, &kept).loot);
        // Unpacked, the table goes and the slots are saved.
        chest.unpack();
        chest.slots[1] = stack(items::BREAD, 2);
        let unpacked = container_data(blocks::CHEST, &chest, &kept);
        assert_eq!(read_container(27, &unpacked), chest);
    }

    #[test]
    fn a_count_of_one_is_written_and_one_left_out_reads_as_one() {
        // `ItemStack.CODEC`'s count is `optionalAlwaysPresentFieldOf`: written always,
        // read as one where it is missing.
        let mut container = read_container(9, &[]);
        container.slots[8] = stack(items::BONE_MEAL, 1);
        let data = container_data(blocks::DISPENSER, &container, &[]);
        assert!(data.windows(5).any(|window| window == b"count"));
        assert_eq!(read_container(9, &data), container);

        let mut entry = vec![TAG_BYTE];
        nbt_put_string(&mut entry, "Slot");
        entry.push(8);
        entry.push(TAG_STRING);
        nbt_put_string(&mut entry, "id");
        nbt_put_string(&mut entry, "minecraft:bone_meal");
        entry.push(TAG_END);
        let mut without = vec![TAG_COMPOUND, TAG_LIST];
        nbt_put_string(&mut without, "Items");
        without.push(TAG_COMPOUND);
        without.extend_from_slice(&1i32.to_be_bytes());
        without.extend_from_slice(&entry);
        without.push(TAG_END);
        assert_eq!(read_container(9, &without), container);
    }

    #[test]
    fn slots_beyond_the_kind_and_items_the_game_lacks_are_read_as_empty() {
        let mut container = read_container(27, &[]);
        container.slots[26] = stack(items::STONE, 2);
        let data = container_data(blocks::CHEST, &container, &[]);
        // Read as a hopper's five, slot 26 is beyond them.
        assert_eq!(read_container(5, &data).slots, vec![None; 5]);
    }

    #[test]
    fn every_kind_of_container_has_the_games_number_of_slots() {
        assert_eq!(container_size(blocks::CHEST), Some(27));
        assert_eq!(container_size(blocks::TRAPPED_CHEST), Some(27));
        assert_eq!(container_size(blocks::COPPER_CHEST), Some(27));
        assert_eq!(container_size(blocks::BARREL), Some(27));
        assert_eq!(container_size(blocks::RED_SHULKER_BOX), Some(27));
        assert_eq!(container_size(blocks::DISPENSER), Some(9));
        assert_eq!(container_size(blocks::DROPPER), Some(9));
        assert_eq!(container_size(blocks::HOPPER), Some(5));
        assert_eq!(container_size(blocks::ENDER_CHEST), None);
        assert_eq!(container_size(blocks::STONE), None);
    }
}
