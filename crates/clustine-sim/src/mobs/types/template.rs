//! A structure template's entity tag as a mob's load reads it
//! (`docs/groundwork/n7-brief.md`, sections 8.10 to 8.12): what every mob takes of it
//! and the few lookups the types' own readings need ([`super::MobType::from_template`]).
//!
//! The tag is NBT's binary form without a name: a compound's type byte, then its
//! entries, then the end. The region's readers of block entities are its own, so this
//! keeps the little it needs beside the mobs: a lookup by name and the payloads of the
//! few tag types a template's mob carries.

use clustine_data::ATTRIBUTES;

use super::super::Mob;
use super::super::body::attributes::{ADD_MULTIPLIED_BASE, ADD_MULTIPLIED_TOTAL, ADD_VALUE};
use super::super::body::{Modifier, attribute};

const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;

/// The entries of `tag` (a compound, its type byte first), or nothing for one that is
/// not a compound.
fn root(tag: &[u8]) -> &[u8] {
    match tag.split_first() {
        Some((&TAG_COMPOUND, entries)) => entries,
        _ => &[],
    }
}

/// The type and payload of the entry `name` of the compound whose entries begin
/// `entries`; `None` where it has none, or the data is cut short before it.
fn entry<'a>(entries: &'a [u8], name: &str) -> Option<(u8, &'a [u8])> {
    let mut rest = entries;
    loop {
        let (&tag, after) = rest.split_first()?;
        if tag == TAG_END {
            return None;
        }
        let (key, payload) = string(after)?;
        if key == name.as_bytes() {
            return Some((tag, payload));
        }
        rest = skip(tag, payload, 0)?;
    }
}

/// A string as NBT writes one, its length first, and what follows it.
fn string(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = usize::from(u16::from_be_bytes(data.get(..2)?.try_into().ok()?));
    let text = data.get(2..2 + length)?;
    Some((text, &data[2 + length..]))
}

/// What follows the payload of a tag of type `tag` at the start of `data`; `None` for
/// data cut short, a type NBT does not have, or nesting deeper than the game allows.
fn skip(tag: u8, data: &[u8], depth: u32) -> Option<&[u8]> {
    if depth > 512 {
        return None;
    }
    let length = |data: &[u8]| -> Option<usize> {
        usize::try_from(i32::from_be_bytes(data.get(..4)?.try_into().ok()?)).ok()
    };
    match tag {
        1 => data.get(1..),
        2 => data.get(2..),
        3 | 5 => data.get(4..),
        4 | 6 => data.get(8..),
        7 => data.get(4 + length(data)?..),
        8 => string(data).map(|(_, rest)| rest),
        9 => {
            let (&element, after) = data.split_first()?;
            let count = length(after)?;
            let mut rest = after.get(4..)?;
            for _ in 0..count {
                rest = skip(element, rest, depth + 1)?;
            }
            Some(rest)
        }
        10 => {
            let mut rest = data;
            loop {
                let (&inner, after) = rest.split_first()?;
                if inner == TAG_END {
                    return Some(after);
                }
                let (_, after) = string(after)?;
                rest = skip(inner, after, depth + 1)?;
            }
        }
        11 => data.get(4 + length(data)?.checked_mul(4)?..),
        12 => data.get(4 + length(data)?.checked_mul(8)?..),
        _ => None,
    }
}

/// A number of the tag's entry `name` as `ValueInput.getIntOr` reads one: any numeric
/// tag, cast; `None` where it has none.
fn number(entries: &[u8], name: &str) -> Option<f64> {
    let (tag, payload) = entry(entries, name)?;
    let value = match tag {
        TAG_BYTE => f64::from(*payload.first()? as i8),
        TAG_SHORT => f64::from(i16::from_be_bytes(payload.get(..2)?.try_into().ok()?)),
        TAG_INT => f64::from(i32::from_be_bytes(payload.get(..4)?.try_into().ok()?)),
        4 => i64::from_be_bytes(payload.get(..8)?.try_into().ok()?) as f64,
        TAG_FLOAT => f64::from(f32::from_be_bytes(payload.get(..4)?.try_into().ok()?)),
        TAG_DOUBLE => f64::from_be_bytes(payload.get(..8)?.try_into().ok()?),
        _ => return None,
    };
    Some(value)
}

/// The integer entry `name` of `tag` (a compound, its type byte first), of any numeric
/// tag cast as the game's `getIntOr` casts it.
pub(crate) fn int(tag: &[u8], name: &str) -> Option<i32> {
    number(root(tag), name).map(|value| value as i32)
}

/// The boolean entry `name` of `tag`: a numeric tag, true where it is not 0.
pub(crate) fn boolean(tag: &[u8], name: &str) -> Option<bool> {
    number(root(tag), name).map(|value| value != 0.0)
}

/// The string entry `name` of `tag`.
pub(crate) fn text<'a>(tag: &'a [u8], name: &str) -> Option<&'a str> {
    let (kind, payload) = entry(root(tag), name)?;
    if kind != TAG_STRING {
        return None;
    }
    std::str::from_utf8(string(payload)?.0).ok()
}

/// The compound entry `name` of `tag`, its type byte first as `tag`'s is, for the
/// readers above (a zombie villager's `VillagerData`).
pub(crate) fn compound(tag: &[u8], name: &str) -> Option<Vec<u8>> {
    let (TAG_COMPOUND, payload) = entry(root(tag), name)? else {
        return None;
    };
    let after = skip(TAG_COMPOUND, payload, 1)?;
    let mut whole = vec![TAG_COMPOUND];
    whole.extend_from_slice(&payload[..payload.len() - after.len()]);
    Some(whole)
}

/// The compounds of the list entry `name` of the compound whose entries begin
/// `entries`, each as its entries; none where it is no list of compounds.
fn compounds<'a>(entries: &'a [u8], name: &str) -> Vec<&'a [u8]> {
    let Some((TAG_LIST, payload)) = entry(entries, name) else {
        return Vec::new();
    };
    let Some((&element, after)) = payload.split_first() else {
        return Vec::new();
    };
    let Some(count) = after
        .get(..4)
        .and_then(|bytes| usize::try_from(i32::from_be_bytes(bytes.try_into().ok()?)).ok())
    else {
        return Vec::new();
    };
    if element != TAG_COMPOUND {
        return Vec::new();
    }
    let mut rest = &after[4..];
    let mut found = Vec::new();
    for _ in 0..count {
        let Some(next) = skip(TAG_COMPOUND, rest, 1) else {
            break;
        };
        found.push(rest);
        rest = next;
    }
    found
}

/// The string entry `name` of the compound whose entries begin `entries`.
fn entry_text<'a>(entries: &'a [u8], name: &str) -> Option<&'a str> {
    let (TAG_STRING, payload) = entry(entries, name)? else {
        return None;
    };
    std::str::from_utf8(string(payload)?.0).ok()
}

/// The attribute of the id `name` (`minecraft:max_health`), by its place in
/// `clustine_data::ATTRIBUTES`.
fn attribute_named(name: &str) -> Option<u16> {
    let at = ATTRIBUTES.iter().position(|known| known.name == name)?;
    Some(attribute(i32::try_from(at).ok()?))
}

/// `AttributeModifier.Operation`'s ordinal of its serialised name.
fn operation(name: &str) -> Option<u8> {
    match name {
        "add_value" => Some(ADD_VALUE),
        "add_multiplied_base" => Some(ADD_MULTIPLIED_BASE),
        "add_multiplied_total" => Some(ADD_MULTIPLIED_TOTAL),
        _ => None,
    }
}

/// What `Mob.load` reads of a template's tag for every mob of the type `kind`, in its
/// order: `PersistenceRequired`, then `LivingEntity`'s attributes (each entry's base and
/// modifiers, on the instance its type has; one its type lacks is skipped, as
/// `AttributeMap.load` skips it) and its `Health`, held to the maximum the attributes
/// now give, as `setHealth` holds it. The rest of a template's mob is redrawn by its
/// `finalizeSpawn` or is the fresh mob's already (no fire, no motion of its own).
pub(crate) fn read_mob(kind: u16, mob: &mut Mob, tag: &[u8]) {
    let entries = root(tag);
    if let Some(persistent) = boolean(tag, "PersistenceRequired") {
        mob.body.flags.persistent = persistent;
    }
    for each in compounds(entries, "attributes") {
        let Some(id) = entry_text(each, "id").and_then(attribute_named) else {
            continue;
        };
        let Some(instance) = mob.body.attributes.instance(kind, id) else {
            continue;
        };
        if let Some(base) = number(each, "base") {
            instance.base = base;
        }
        for modifier in compounds(each, "modifiers") {
            let (Some(id), Some(amount), Some(operation)) = (
                entry_text(modifier, "id"),
                number(modifier, "amount"),
                entry_text(modifier, "operation").and_then(operation),
            ) else {
                continue;
            };
            instance.add_modifier(Modifier {
                id: id.to_owned(),
                amount,
                operation,
            });
        }
    }
    if let Some(health) = number(entries, "Health") {
        let max = mob
            .body
            .attributes
            .value(kind, attribute(clustine_data::attributes::MAX_HEALTH)) as f32;
        mob.body.vitals.health = (health as f32).clamp(0.0, max);
    }
}

#[cfg(test)]
pub(crate) mod build {
    //! Tags as a template keeps them, for tests: a compound, its type byte first.

    /// An entry of a compound: its type, its name and its payload.
    pub(crate) fn entry(tag: u8, name: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend_from_slice(&(name.len() as u16).to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// A compound's payload of `entries`, its end after them.
    pub(crate) fn payload(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out: Vec<u8> = entries.concat();
        out.push(0);
        out
    }

    /// A compound tag of `entries`, its type byte first.
    pub(crate) fn compound(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![10];
        out.extend(payload(entries));
        out
    }

    pub(crate) fn byte(name: &str, value: i8) -> Vec<u8> {
        entry(1, name, &[value as u8])
    }

    pub(crate) fn int(name: &str, value: i32) -> Vec<u8> {
        entry(3, name, &value.to_be_bytes())
    }

    pub(crate) fn float(name: &str, value: f32) -> Vec<u8> {
        entry(5, name, &value.to_be_bytes())
    }

    pub(crate) fn double(name: &str, value: f64) -> Vec<u8> {
        entry(6, name, &value.to_be_bytes())
    }

    pub(crate) fn string(name: &str, value: &str) -> Vec<u8> {
        let mut payload = (value.len() as u16).to_be_bytes().to_vec();
        payload.extend_from_slice(value.as_bytes());
        entry(8, name, &payload)
    }

    /// A list of compounds, each given by its entries.
    pub(crate) fn compounds(name: &str, each: &[Vec<Vec<u8>>]) -> Vec<u8> {
        let mut out = vec![10];
        out.extend_from_slice(&(each.len() as i32).to_be_bytes());
        for entries in each {
            out.extend(payload(entries));
        }
        entry(9, name, &out)
    }

    /// A compound entry of `entries`.
    pub(crate) fn nested(name: &str, entries: &[Vec<u8>]) -> Vec<u8> {
        entry(10, name, &payload(entries))
    }

    /// An attribute's entry of `attributes`: its id, its base, and its modifiers, each
    /// an id, an amount and an operation.
    pub(crate) fn attribute(id: &str, base: f64, modifiers: &[(&str, f64, &str)]) -> Vec<Vec<u8>> {
        let mut entries = vec![string("id", id), double("base", base)];
        if !modifiers.is_empty() {
            let each: Vec<Vec<Vec<u8>>> = modifiers
                .iter()
                .map(|&(id, amount, operation)| {
                    vec![
                        double("amount", amount),
                        string("id", id),
                        string("operation", operation),
                    ]
                })
                .collect();
            entries.push(compounds("modifiers", &each));
        }
        entries
    }
}
