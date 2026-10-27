//! Item stacks in a block entity's NBT, as the game writes and reads them
//! (`docs/groundwork/n9-plan.md`, section 3.2): the world's structure of a value
//! ([`Value`], `clustine_world::components`) as `NbtOps` turns it into tags and back,
//! and a stack's compound as `ItemStack.CODEC` writes it with those ops, so that a
//! potion put into a chest keeps its contents at a save and a chunk the game wrote
//! reads.
//!
//! The tags of a structure: a byte, a short, an int, a long, a float, a double and a
//! string as themselves; a boolean as a byte of 1 or 0 (`NbtOps.createBoolean`), which
//! the structure's reader takes back from any number; a list as a `ListTag`; a map as
//! a compound. A list goes through the codec's list builder, which collects into a
//! `ListTag` whatever its elements: the game's own form of a firework's colours, a list
//! of ints, is `[16711680]` and not the `[I;16711680]` of an `IntArrayTag`
//! (`n9-hashes/answers.tsv`). A `ListTag` whose elements are not all of one type is
//! written as the game's is since 1.21.5: as a list of compounds, an element that is not
//! a compound (or is a compound that would read as a wrapper) wrapped as the one entry of
//! a compound with the empty name. On reading, a byte, an int and a long array read as
//! lists of their numbers, as `NbtOps.getStream` gives them, and wrapped elements are
//! unwrapped. A compound's entries are written in the order its `HashMap` iterates over
//! them ([`hash_map_order`]), the structure's order of putting them in.
//!
//! [`Value::Empty`] (`NbtOps.empty()`, an `EndTag`) is never part of a component's
//! structure; one in a map is left out, and one in a list is written as an empty
//! compound, since an `EndTag` cannot be written there.

use clustine_data::item_id_by_name;
use clustine_world::components::{Kind, Value, component_from, stack_structure};

use super::block_entities::{
    TAG_BYTE, TAG_COMPOUND, TAG_DOUBLE, TAG_END, TAG_FLOAT, TAG_INT, TAG_LIST, TAG_LONG,
    TAG_STRING, hash_map_order, nbt_string,
};
use crate::api::ItemStack;

const TAG_SHORT: u8 = 2;
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// How deep the game reads nested tags (`NbtAccounter`'s limit).
const MAX_DEPTH: u32 = 512;

/// The tag type `NbtOps` gives `value`.
pub(super) fn tag_of(value: &Value) -> u8 {
    match value {
        Value::Byte(_) | Value::Bool(_) => TAG_BYTE,
        Value::Short(_) => TAG_SHORT,
        Value::Int(_) => TAG_INT,
        Value::Long(_) => TAG_LONG,
        Value::Float(_) => TAG_FLOAT,
        Value::Double(_) => TAG_DOUBLE,
        Value::String(_) => TAG_STRING,
        Value::List(_) => TAG_LIST,
        Value::Map(_) | Value::Empty => TAG_COMPOUND,
    }
}

/// Appends the payload of `value`'s tag ([`tag_of`]) to `out`.
pub(super) fn put_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Byte(n) => out.push(*n as u8),
        Value::Bool(b) => out.push(u8::from(*b)),
        Value::Short(n) => out.extend_from_slice(&n.to_be_bytes()),
        Value::Int(n) => out.extend_from_slice(&n.to_be_bytes()),
        Value::Long(n) => out.extend_from_slice(&n.to_be_bytes()),
        Value::Float(n) => out.extend_from_slice(&n.to_be_bytes()),
        Value::Double(n) => out.extend_from_slice(&n.to_be_bytes()),
        Value::String(text) => put_string(out, text),
        Value::List(items) => put_list(out, items),
        Value::Map(entries) => put_compound(out, entries),
        Value::Empty => out.push(TAG_END),
    }
}

/// A string as `DataOutput.writeUTF` writes one: its length in bytes, then the bytes.
/// The game cannot write a string of more than 65,535 bytes (`writeUTF` throws, and the
/// chunk is not saved); the world's strings are far shorter, and one that is not is cut
/// at a character's end there rather than lose the chunk. Characters outside the basic
/// plane, which Java writes as two surrogates of three bytes, are written as UTF-8's
/// four, as the rest of the region's NBT does.
fn put_string(out: &mut Vec<u8>, text: &str) {
    let mut end = text.len().min(usize::from(u16::MAX));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let length = u16::try_from(end).expect("cut to a short string's length");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&text.as_bytes()[..end]);
}

/// `ListTag.write`: the type of its elements (`TAG_END` for an empty list, a compound
/// for one of mixed types), its length, then their payloads.
fn put_list(out: &mut Vec<u8>, items: &[Value]) {
    let first = items.first().map(tag_of);
    let mixed = items.iter().any(|item| Some(tag_of(item)) != first);
    let element = if mixed {
        TAG_COMPOUND
    } else {
        first.unwrap_or(TAG_END)
    };
    out.push(element);
    let length = i32::try_from(items.len()).expect("fewer elements than an int counts");
    out.extend_from_slice(&length.to_be_bytes());
    for item in items {
        // `ListTag.wrapIfNeeded`: a compound stays as it is unless it would read as a
        // wrapper; anything else is wrapped.
        let wraps = mixed
            && match item {
                Value::Map(entries) => is_wrapper(entries),
                _ => true,
            };
        if wraps {
            put_compound(out, &[(String::new(), item.clone())]);
        } else {
            put_value(out, item);
        }
    }
}

/// Whether a compound of `entries` is what a `ListTag` of mixed types wraps an element
/// in: one entry, with the empty name.
fn is_wrapper(entries: &[(String, Value)]) -> bool {
    matches!(entries, [(name, _)] if name.is_empty())
}

/// A compound's entries in the order of the game's `HashMap`, each its tag, its name
/// and its payload, then the end.
fn put_compound(out: &mut Vec<u8>, entries: &[(String, Value)]) {
    let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    for index in hash_map_order(&names) {
        let (name, value) = &entries[index];
        if matches!(value, Value::Empty) {
            continue;
        }
        out.push(tag_of(value));
        put_string(out, name);
        put_value(out, value);
    }
    out.push(TAG_END);
}

/// The value of the tag `tag` whose payload begins `data`, and what follows it; `None`
/// for data cut short, a type NBT does not have, or nesting deeper than the game reads.
pub(super) fn read_value(tag: u8, data: &[u8]) -> Option<(Value, &[u8])> {
    read_at(tag, data, 0)
}

fn read_at(tag: u8, data: &[u8], depth: u32) -> Option<(Value, &[u8])> {
    if depth > MAX_DEPTH {
        return None;
    }
    fn bytes<const N: usize>(data: &[u8]) -> Option<([u8; N], &[u8])> {
        Some((data.get(..N)?.try_into().ok()?, &data[N..]))
    }
    fn length(data: &[u8]) -> Option<(usize, &[u8])> {
        let (raw, rest) = bytes::<4>(data)?;
        Some((usize::try_from(i32::from_be_bytes(raw)).ok()?, rest))
    }
    Some(match tag {
        TAG_BYTE => {
            let (raw, rest) = bytes::<1>(data)?;
            (Value::Byte(raw[0] as i8), rest)
        }
        TAG_SHORT => {
            let (raw, rest) = bytes::<2>(data)?;
            (Value::Short(i16::from_be_bytes(raw)), rest)
        }
        TAG_INT => {
            let (raw, rest) = bytes::<4>(data)?;
            (Value::Int(i32::from_be_bytes(raw)), rest)
        }
        TAG_LONG => {
            let (raw, rest) = bytes::<8>(data)?;
            (Value::Long(i64::from_be_bytes(raw)), rest)
        }
        TAG_FLOAT => {
            let (raw, rest) = bytes::<4>(data)?;
            (Value::Float(f32::from_be_bytes(raw)), rest)
        }
        TAG_DOUBLE => {
            let (raw, rest) = bytes::<8>(data)?;
            (Value::Double(f64::from_be_bytes(raw)), rest)
        }
        TAG_STRING => {
            let (text, rest) = nbt_string(data)?;
            (
                Value::String(String::from_utf8_lossy(text).into_owned()),
                rest,
            )
        }
        TAG_BYTE_ARRAY => {
            let (count, rest) = length(data)?;
            let raw = rest.get(..count)?;
            let items = raw.iter().map(|byte| Value::Byte(*byte as i8)).collect();
            (Value::List(items), &rest[count..])
        }
        TAG_INT_ARRAY | TAG_LONG_ARRAY => {
            let (count, mut rest) = length(data)?;
            let element = if tag == TAG_INT_ARRAY {
                TAG_INT
            } else {
                TAG_LONG
            };
            // Each element is at least four bytes: a count beyond the data is refused
            // before anything is kept for it.
            rest.get(..count.checked_mul(4)?)?;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let (item, after) = read_at(element, rest, depth + 1)?;
                items.push(item);
                rest = after;
            }
            (Value::List(items), rest)
        }
        TAG_LIST => {
            let (&element, after) = data.split_first()?;
            let (count, mut rest) = length(after)?;
            let mut items = Vec::new();
            for _ in 0..count {
                let (item, after) = read_at(element, rest, depth + 1)?;
                // `ListTag`'s readers unwrap an element of a list of mixed types.
                items.push(match item {
                    Value::Map(mut entries) if is_wrapper(&entries) => {
                        entries.pop().expect("a wrapper has its one entry").1
                    }
                    item => item,
                });
                rest = after;
            }
            (Value::List(items), rest)
        }
        TAG_COMPOUND => {
            let mut entries = Vec::new();
            let mut rest = data;
            loop {
                let (&inner, after) = rest.split_first()?;
                if inner == TAG_END {
                    break (Value::Map(entries), after);
                }
                let (name, after) = nbt_string(after)?;
                let (value, after) = read_at(inner, after, depth + 1)?;
                // A compound read twice under one name keeps the last, as the map does.
                let name = String::from_utf8_lossy(name).into_owned();
                entries.retain(|(known, _)| *known != name);
                entries.push((name, value));
                rest = after;
            }
        }
        _ => return None,
    })
}

/// The game's `ItemStack.CODEC` encoding of `stack` with `NbtOps`: its item by name
/// (`id`), its count, always (`optionalAlwaysPresentFieldOf`), and its components
/// (`DataComponentPatch.CODEC`, `optionalFieldOf` with the empty patch as its default,
/// so left out where it has none): an added one under its kind's name, a removed kind
/// under its name after `!`, with an empty compound.
pub(super) fn stack_value(stack: &ItemStack) -> Value {
    // The template's structure has the item's name and the components as the stack's
    // codec writes them; only its count differs, left out there at one.
    let mut entries = Vec::new();
    let mut components = None;
    if let Value::Map(template) = stack_structure(stack) {
        for (name, value) in template {
            match name.as_str() {
                "id" => entries.push((name, value)),
                "components" => components = Some(value),
                _ => {}
            }
        }
    }
    entries.push(("count".to_owned(), Value::Int(stack.count)));
    if let Some(components) = components {
        entries.push(("components".to_owned(), components));
    }
    Value::Map(entries)
}

/// The game's `ItemStackWithSlot.CODEC` encoding of `stack` in `slot`: its `Slot` as an
/// unsigned byte, then the stack's fields in the same compound.
pub(super) fn slot_stack_value(slot: u8, stack: &ItemStack) -> Value {
    let mut entries = vec![("Slot".to_owned(), Value::Byte(slot as i8))];
    if let Value::Map(fields) = stack_value(stack) {
        entries.extend(fields);
    }
    Value::Map(entries)
}

/// The stack that a compound of `fields` holds, as `ItemStack.CODEC` reads it: `None`
/// for one whose item the game does not have, air, or a count below one.
///
/// Its components are read into its patch where Clustine keeps them, and the rest is
/// dropped, the stack kept: a kind Clustine does not know (a chest the game wrote may
/// hold a `minecraft:custom_name`, `minecraft:lore`, `minecraft:container` and many
/// more), and a kind it knows whose value does not read (the game would refuse the
/// whole stack and empty its slot). Clustine keeps the stack in both cases because the
/// alternative is worse for the player: an item lost from a chest is noticed at once,
/// a name or a lore line gone from it far less, and the item and its count are what the
/// region's rules read. A removed kind Clustine does not know is dropped likewise, so
/// that the item's default holds for it.
pub(super) fn stack_from_fields(fields: &[(String, Value)]) -> Option<ItemStack> {
    let id = match field(fields, "id")? {
        Value::String(name) => item_id_by_name(name)?,
        _ => return None,
    };
    if id == clustine_data::items::AIR {
        return None;
    }
    // `optionalAlwaysPresentFieldOf`: a count left out is one; any number reads, as the
    // game's `getNumberValue` gives it.
    let count = match field(fields, "count") {
        None => 1,
        Some(value) => int_value(value)?,
    };
    if count < 1 {
        return None;
    }
    let mut stack = ItemStack::new(id, count);
    if let Some(Value::Map(components)) = field(fields, "components") {
        for (key, value) in components {
            let (removed, name) = match key.strip_prefix('!') {
                Some(name) => (true, name),
                None => (false, key.as_str()),
            };
            let name = if name.contains(':') {
                name.to_owned()
            } else {
                format!("minecraft:{name}")
            };
            let Some(kind) = Kind::named(&name) else {
                continue;
            };
            if removed {
                stack.components.remove(kind);
            } else if let Ok(component) = component_from(kind, value) {
                stack.components.set(component);
            }
        }
    }
    Some(stack)
}

/// The slot of a compound of `fields` as `ItemStackWithSlot.CODEC` reads it: an
/// unsigned byte, the low eight bits of any number.
pub(super) fn slot_of(fields: &[(String, Value)]) -> Option<usize> {
    field(fields, "Slot")
        .and_then(int_value)
        .map(|slot| usize::from(slot as u8))
}

fn field<'a>(fields: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// Java's `Number.intValue()` of a number tag.
fn int_value(value: &Value) -> Option<i32> {
    Some(match *value {
        Value::Byte(n) => i32::from(n),
        Value::Short(n) => i32::from(n),
        Value::Int(n) => n,
        Value::Long(n) => n as i32,
        Value::Float(n) => n as i32,
        Value::Double(n) => n as i32,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
