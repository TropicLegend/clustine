//! What a container block keeps, read from and written into its block entity's data
//! (`docs/groundwork/n6-plan.md`, section 6 item 7 and section 7 item 5): its slots as
//! the game saves them (`Items`, a list of `{Slot, id, count}` with the empty slots left
//! out), whether a loot table is still to be unpacked, and the count of its openers,
//! which the game does not save and Clustine does, so that a region restored, split or
//! lent to knows what it last reported.
//!
//! The data stays the one durable form, as the comparator's output does: the region
//! reads a container from it when it needs one and writes it back whole on every change.
//! Every entry this module does not own (a custom name, a lock, the components, a
//! hopper's cooldown, a loot table's name and seed) is kept as it came.
//!
//! Laid down by step N6a for the containers, dispensers, droppers and hoppers that
//! steps N6b, N6d and N6e build on it; until then only its tests use it.
#![cfg_attr(not(test), expect(dead_code, reason = "used from step N6b on"))]

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, BlockState, ITEMS, item_id_by_name};

use super::block_entities::{TAG_BYTE, TAG_COMPOUND, TAG_END, TAG_INT, string_bucket};
use super::block_entities::{nbt_put_string, nbt_string, skip_nbt};
use crate::api::ItemStack;

const TAG_SHORT: u8 = 2;
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

/// Sets the slots `slots` from the payload of an `Items` list.
fn read_items(list: &[u8], slots: &mut [Option<ItemStack>]) {
    let Some((&element, after)) = list.split_first() else {
        return;
    };
    let Some(count) = after
        .get(..4)
        .map(|bytes| i32::from_be_bytes(bytes.try_into().expect("four bytes")))
    else {
        return;
    };
    if element != TAG_COMPOUND {
        return;
    }
    let mut rest = &after[4..];
    for _ in 0..count.max(0) {
        let mut slot = None;
        let mut id = None;
        // `ItemStack.CODEC` leaves a count of one out.
        let mut items = 1;
        let Some(next) = each_raw(rest, |tag, name, payload| match (tag, name) {
            (TAG_BYTE, b"Slot") => slot = payload.first().map(|byte| usize::from(*byte)),
            (TAG_STRING, b"id") => {
                id = nbt_string(payload)
                    .and_then(|(text, _)| std::str::from_utf8(text).ok())
                    .and_then(item_id_by_name);
            }
            (TAG_BYTE | TAG_SHORT | TAG_INT, b"count") => {
                items = number(tag, payload).unwrap_or(0);
            }
            _ => {}
        }) else {
            return;
        };
        rest = next;
        if let (Some(slot), Some(item)) = (slot, id)
            && item != clustine_data::items::AIR
            && items > 0
            && let Some(place) = slots.get_mut(slot)
        {
            *place = Some(ItemStack { item, count: items });
        }
    }
}

/// The data of the block entity of a block of `state` holding `container`, from its data
/// before, `data`: every entry kept as it was but the slots, the loot table's (gone once
/// unpacked) and the openers', which are written anew. As the game writes a compound,
/// in the order of the names' buckets, those before in the order they came.
pub(crate) fn container_data(state: BlockState, container: &Container, data: &[u8]) -> Vec<u8> {
    let mut entries: Vec<(u8, Vec<u8>, Vec<u8>)> = Vec::new();
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
            .filter_map(|(slot, stack)| Some((slot, (*stack)?)))
            .filter(|(_, stack)| stack.count > 0)
            .collect();
        if !filled.is_empty() || always_saves_items(state) {
            entries.push((TAG_LIST, b"Items".to_vec(), items_payload(&filled)));
        }
    }
    if container.openers > 0 {
        let count = i32::try_from(container.openers).unwrap_or(i32::MAX);
        entries.push((
            TAG_INT,
            OPENERS.as_bytes().to_vec(),
            count.to_be_bytes().to_vec(),
        ));
    }
    if !entries.iter().any(|(_, name, _)| name == b"components") {
        entries.push((TAG_COMPOUND, b"components".to_vec(), vec![TAG_END]));
    }
    entries.sort_by_key(|(_, name, _)| string_bucket(&String::from_utf8_lossy(name)));
    let mut written = vec![TAG_COMPOUND];
    for (tag, name, payload) in entries {
        written.push(tag);
        let length = u16::try_from(name.len()).expect("a name read from a short string");
        written.extend_from_slice(&length.to_be_bytes());
        written.extend_from_slice(&name);
        written.extend_from_slice(&payload);
    }
    written.push(TAG_END);
    written
}

/// The payload of an `Items` list of `filled`: compounds of `Slot`, `id` and `count`, the
/// count left out where it is one, as `ItemStackWithSlot.CODEC` writes them.
fn items_payload(filled: &[(usize, ItemStack)]) -> Vec<u8> {
    let mut payload = vec![TAG_COMPOUND];
    let count = i32::try_from(filled.len()).expect("fewer slots than an int counts");
    payload.extend_from_slice(&count.to_be_bytes());
    for (slot, stack) in filled {
        let name = usize::try_from(stack.item)
            .ok()
            .and_then(|item| ITEMS.get(item))
            .map_or("minecraft:air", |item| item.name);
        let mut entries: Vec<(&str, u8, Vec<u8>)> = vec![
            (
                "Slot",
                TAG_BYTE,
                vec![u8::try_from(*slot).expect("a slot below 256")],
            ),
            ("id", TAG_STRING, {
                let mut text = Vec::new();
                nbt_put_string(&mut text, name);
                text
            }),
        ];
        if stack.count != 1 {
            entries.push(("count", TAG_INT, stack.count.to_be_bytes().to_vec()));
        }
        entries.sort_by_key(|(name, _, _)| string_bucket(name));
        for (name, tag, value) in entries {
            payload.push(tag);
            nbt_put_string(&mut payload, name);
            payload.extend_from_slice(&value);
        }
        payload.push(TAG_END);
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

/// A whole number of the tag `tag`, as the game reads a count of any width.
fn number(tag: u8, payload: &[u8]) -> Option<i32> {
    match tag {
        TAG_BYTE => payload.first().map(|byte| i32::from(*byte as i8)),
        TAG_SHORT => Some(i32::from(i16::from_be_bytes(
            payload.get(..2)?.try_into().ok()?,
        ))),
        TAG_INT => int(payload),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::{blocks, items};

    fn stack(item: i32, count: i32) -> Option<ItemStack> {
        Some(ItemStack { item, count })
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
    fn a_count_of_one_is_left_out_and_read_as_one() {
        let mut container = read_container(9, &[]);
        container.slots[8] = stack(items::BONE_MEAL, 1);
        let data = container_data(blocks::DISPENSER, &container, &[]);
        assert!(!data.windows(5).any(|window| window == b"count"));
        assert_eq!(read_container(9, &data), container);
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
