//! The game's hash of an item component, as a client sends it in a click
//! (`HashedPatchMap`, `HashOps.CRC32C_INSTANCE`), written for N9's comparisons
//! (`docs/groundwork/n9-plan.md`, section 11, comparison 1) from the game's rules and not
//! from the edge's code, so that a bot's click is judged by the official server against
//! a hash that was not taken from what is being tested.
//!
//! `HashOps` hashes each value on its own with CRC-32C over: a tag byte, then the value's
//! bytes little-endian (a string as its length in UTF-16 units and those units, a boolean
//! as one byte), a list as the hashes of its items between a start and an end tag, a map
//! as the hashes of its keys and values, entry by entry, sorted by the key's hash and then
//! the value's as unsigned numbers, between a start and an end tag. What is hashed is the
//! component's codec encoding, which `clustine_world::components::structure` gives.

use clustine_world::Component;
use clustine_world::components::{Value, structure};

const EMPTY: u8 = 1;
const MAP_START: u8 = 2;
const MAP_END: u8 = 3;
const LIST_START: u8 = 4;
const LIST_END: u8 = 5;
const BYTE: u8 = 6;
const SHORT: u8 = 7;
const INT: u8 = 8;
const LONG: u8 = 9;
const FLOAT: u8 = 10;
const DOUBLE: u8 = 11;
const STRING: u8 = 12;
const BOOLEAN: u8 = 13;

/// The hash a client sends for `component`, read as `HashCode.asInt()` reads it.
pub fn component_hash(component: &Component) -> i32 {
    i32::from_ne_bytes(value_hash(&structure(component)).to_ne_bytes())
}

/// `HashOps`'s hash of `value`.
pub fn value_hash(value: &Value) -> u32 {
    let mut bytes = Vec::new();
    match value {
        Value::Empty => bytes.push(EMPTY),
        Value::Byte(n) => {
            bytes.push(BYTE);
            bytes.extend(n.to_le_bytes());
        }
        Value::Short(n) => {
            bytes.push(SHORT);
            bytes.extend(n.to_le_bytes());
        }
        Value::Int(n) => {
            bytes.push(INT);
            bytes.extend(n.to_le_bytes());
        }
        Value::Long(n) => {
            bytes.push(LONG);
            bytes.extend(n.to_le_bytes());
        }
        Value::Float(n) => {
            bytes.push(FLOAT);
            bytes.extend(n.to_bits().to_le_bytes());
        }
        Value::Double(n) => {
            bytes.push(DOUBLE);
            bytes.extend(n.to_bits().to_le_bytes());
        }
        Value::Bool(b) => {
            bytes.push(BOOLEAN);
            bytes.push(u8::from(*b));
        }
        Value::String(text) => {
            let units: Vec<u16> = text.encode_utf16().collect();
            bytes.push(STRING);
            bytes.extend((units.len() as u32).to_le_bytes());
            for unit in units {
                bytes.extend(unit.to_le_bytes());
            }
        }
        Value::List(items) => {
            bytes.push(LIST_START);
            for item in items {
                bytes.extend(value_hash(item).to_le_bytes());
            }
            bytes.push(LIST_END);
        }
        Value::Map(entries) => {
            let mut pairs: Vec<(u32, u32)> = entries
                .iter()
                .map(|(key, value)| (value_hash(&Value::String(key.clone())), value_hash(value)))
                .collect();
            pairs.sort_unstable();
            bytes.push(MAP_START);
            for (key, value) in pairs {
                bytes.extend(key.to_le_bytes());
                bytes.extend(value.to_le_bytes());
            }
            bytes.push(MAP_END);
        }
    }
    crc32c(&bytes)
}

/// CRC-32C, as Guava's `Hashing.crc32c()`: reflected, the polynomial 0x1EDC6F41
/// (0x82F63B78 reflected), all ones in and out. Bit by bit: the tests' clicks are few.
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_gives_the_check_value_of_the_standard() {
        // The check value of CRC-32C (iSCSI) over the ASCII digits one to nine.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn a_map_is_hashed_whatever_the_order_of_its_entries() {
        let one = Value::Map(vec![
            ("a".to_owned(), Value::Int(1)),
            ("b".to_owned(), Value::Bool(true)),
        ]);
        let other = Value::Map(vec![
            ("b".to_owned(), Value::Bool(true)),
            ("a".to_owned(), Value::Int(1)),
        ]);
        assert_eq!(value_hash(&one), value_hash(&other));
    }

    #[test]
    fn a_string_is_hashed_by_its_utf16_units() {
        // "é" is one unit, two bytes; its UTF-8 form would be two units' worth.
        let mut bytes = vec![STRING];
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(0x00E9u16.to_le_bytes());
        assert_eq!(value_hash(&Value::String("é".to_owned())), crc32c(&bytes));
    }
}
