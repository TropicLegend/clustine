//! NBT as it appears inside packets.
//!
//! Packets carry NBT without the root name that files have: a type byte followed
//! directly by the payload. A lone end tag stands for "no value". The root is often not a
//! compound; a text component, for example, may be a single string tag.

use std::fmt;

use crate::codec::{DecodeError, Reader, Writer};

/// How deeply lists and compounds may nest, as in vanilla.
const MAX_DEPTH: usize = 512;

const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// An NBT value.
#[derive(Debug, Clone, PartialEq)]
pub enum Nbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    /// All elements must be of the same kind.
    List(Vec<Nbt>),
    /// Entries in the order they appear on the wire.
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    fn tag(&self) -> u8 {
        match self {
            Nbt::Byte(_) => TAG_BYTE,
            Nbt::Short(_) => TAG_SHORT,
            Nbt::Int(_) => TAG_INT,
            Nbt::Long(_) => TAG_LONG,
            Nbt::Float(_) => TAG_FLOAT,
            Nbt::Double(_) => TAG_DOUBLE,
            Nbt::ByteArray(_) => TAG_BYTE_ARRAY,
            Nbt::String(_) => TAG_STRING,
            Nbt::List(_) => TAG_LIST,
            Nbt::Compound(_) => TAG_COMPOUND,
            Nbt::IntArray(_) => TAG_INT_ARRAY,
            Nbt::LongArray(_) => TAG_LONG_ARRAY,
        }
    }

    /// The value of `key` if this is a compound that has it.
    pub fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn put_payload(&self, w: &mut Writer) {
        match self {
            Nbt::Byte(value) => w.put_i8(*value),
            Nbt::Short(value) => w.put_i16(*value),
            Nbt::Int(value) => w.put_i32(*value),
            Nbt::Long(value) => w.put_i64(*value),
            Nbt::Float(value) => w.put_f32(*value),
            Nbt::Double(value) => w.put_f64(*value),
            Nbt::ByteArray(bytes) => {
                put_count(w, bytes.len());
                w.put_bytes(bytes);
            }
            Nbt::String(text) => put_string(w, text),
            Nbt::List(items) => {
                w.put_u8(items.first().map_or(TAG_END, Nbt::tag));
                put_count(w, items.len());
                for item in items {
                    item.put_payload(w);
                }
            }
            Nbt::Compound(entries) => {
                for (name, value) in entries {
                    w.put_u8(value.tag());
                    put_string(w, name);
                    value.put_payload(w);
                }
                w.put_u8(TAG_END);
            }
            Nbt::IntArray(values) => {
                put_count(w, values.len());
                for value in values {
                    w.put_i32(*value);
                }
            }
            Nbt::LongArray(values) => {
                put_count(w, values.len());
                for value in values {
                    w.put_i64(*value);
                }
            }
        }
    }

    fn read_payload(r: &mut Reader<'_>, tag: u8, depth: usize) -> Result<Self, DecodeError> {
        Ok(match tag {
            TAG_BYTE => Nbt::Byte(r.i8()?),
            TAG_SHORT => Nbt::Short(r.i16()?),
            TAG_INT => Nbt::Int(r.i32()?),
            TAG_LONG => Nbt::Long(r.i64()?),
            TAG_FLOAT => Nbt::Float(r.f32()?),
            TAG_DOUBLE => Nbt::Double(r.f64()?),
            TAG_BYTE_ARRAY => {
                let count = read_count(r, 1)?;
                Nbt::ByteArray(r.bytes(count)?.to_vec())
            }
            TAG_STRING => Nbt::String(read_string(r)?),
            TAG_LIST => {
                let depth = nested(depth)?;
                let item_tag = r.u8()?;
                // Elements of an end-typed list have no payload, so the element size
                // cannot bound the count; such a list is only valid when empty.
                let count = read_count(r, if item_tag == TAG_END { 0 } else { 1 })?;
                if item_tag == TAG_END && count != 0 {
                    return Err(invalid("NBT list element type", item_tag));
                }
                let mut items = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    items.push(Self::read_payload(r, item_tag, depth)?);
                }
                Nbt::List(items)
            }
            TAG_COMPOUND => {
                let depth = nested(depth)?;
                let mut entries = Vec::new();
                loop {
                    let entry_tag = r.u8()?;
                    if entry_tag == TAG_END {
                        break;
                    }
                    let name = read_string(r)?;
                    entries.push((name, Self::read_payload(r, entry_tag, depth)?));
                }
                Nbt::Compound(entries)
            }
            TAG_INT_ARRAY => {
                let count = read_count(r, 4)?;
                Nbt::IntArray((0..count).map(|_| r.i32()).collect::<Result<_, _>>()?)
            }
            TAG_LONG_ARRAY => {
                let count = read_count(r, 8)?;
                Nbt::LongArray((0..count).map(|_| r.i64()).collect::<Result<_, _>>()?)
            }
            other => return Err(invalid("NBT tag", other)),
        })
    }
}

/// Renders the value in the notation the game uses for NBT in commands.
impl fmt::Display for Nbt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn list<T: fmt::Display>(
            f: &mut fmt::Formatter<'_>,
            open: &str,
            items: impl Iterator<Item = T>,
            suffix: &str,
        ) -> fmt::Result {
            f.write_str(open)?;
            for (index, item) in items.enumerate() {
                if index > 0 {
                    f.write_str(",")?;
                }
                write!(f, "{item}{suffix}")?;
            }
            f.write_str("]")
        }

        match self {
            Nbt::Byte(value) => write!(f, "{value}b"),
            Nbt::Short(value) => write!(f, "{value}s"),
            Nbt::Int(value) => write!(f, "{value}"),
            Nbt::Long(value) => write!(f, "{value}L"),
            Nbt::Float(value) => write!(f, "{value}f"),
            Nbt::Double(value) => write!(f, "{value}d"),
            Nbt::ByteArray(bytes) => list(f, "[B;", bytes.iter().map(|byte| *byte as i8), "b"),
            Nbt::String(text) => write!(f, "{text:?}"),
            Nbt::List(items) => list(f, "[", items.iter(), ""),
            Nbt::Compound(entries) => {
                f.write_str("{")?;
                for (index, (name, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{name:?}:{value}")?;
                }
                f.write_str("}")
            }
            Nbt::IntArray(values) => list(f, "[I;", values.iter(), ""),
            Nbt::LongArray(values) => list(f, "[L;", values.iter(), "L"),
        }
    }
}

impl Writer {
    /// Writes an NBT value in the nameless form packets use.
    pub fn put_nbt(&mut self, value: &Nbt) {
        self.put_u8(value.tag());
        value.put_payload(self);
    }
}

impl Reader<'_> {
    /// Reads an NBT value in the nameless form packets use. A lone end tag is `None`.
    pub fn nbt(&mut self) -> Result<Option<Nbt>, DecodeError> {
        match self.u8()? {
            TAG_END => Ok(None),
            tag => Nbt::read_payload(self, tag, 0).map(Some),
        }
    }
}

fn invalid(what: &'static str, value: u8) -> DecodeError {
    DecodeError::InvalidValue {
        what,
        value: value.into(),
    }
}

fn nested(depth: usize) -> Result<usize, DecodeError> {
    if depth == MAX_DEPTH {
        return Err(DecodeError::InvalidValue {
            what: "NBT nesting depth",
            value: MAX_DEPTH as i64 + 1,
        });
    }
    Ok(depth + 1)
}

fn put_count(w: &mut Writer, count: usize) {
    w.put_i32(i32::try_from(count).expect("NBT collection fits an i32 count"));
}

/// Reads an element count and checks that `count` elements of `item_size` bytes can
/// still follow, so that a forged count cannot trigger a large allocation.
fn read_count(r: &mut Reader<'_>, item_size: usize) -> Result<usize, DecodeError> {
    let raw = r.i32()?;
    let count = usize::try_from(raw).map_err(|_| DecodeError::NegativeLength(raw))?;
    if count.saturating_mul(item_size) > r.remaining() {
        return Err(DecodeError::LengthExceedsData {
            length: count,
            remaining: r.remaining(),
        });
    }
    Ok(count)
}

/// Writes a string in Java's "modified UTF-8": a 16-bit byte length, NUL as two bytes,
/// and characters outside the basic plane as two three-byte surrogates.
fn put_string(w: &mut Writer, text: &str) {
    let mut bytes = Vec::with_capacity(text.len());
    for unit in text.encode_utf16() {
        match unit {
            0x0001..=0x007F => bytes.push(unit as u8),
            0x0000 | 0x0080..=0x07FF => {
                bytes.push(0xC0 | (unit >> 6) as u8);
                bytes.push(0x80 | (unit & 0x3F) as u8);
            }
            _ => {
                bytes.push(0xE0 | (unit >> 12) as u8);
                bytes.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                bytes.push(0x80 | (unit & 0x3F) as u8);
            }
        }
    }
    w.put_u16(u16::try_from(bytes.len()).expect("NBT string fits a 16-bit length"));
    w.put_bytes(&bytes);
}

fn read_string(r: &mut Reader<'_>) -> Result<String, DecodeError> {
    let length = usize::from(r.u16()?);
    let bytes = r.bytes(length)?;
    let mut units = Vec::with_capacity(length);
    let mut rest = bytes.iter().copied();
    while let Some(first) = rest.next() {
        let mut continuation = || match rest.next() {
            Some(byte) if byte & 0xC0 == 0x80 => Ok(u16::from(byte & 0x3F)),
            _ => Err(DecodeError::InvalidUtf8),
        };
        units.push(match first {
            0x00..=0x7F => u16::from(first),
            0xC0..=0xDF => u16::from(first & 0x1F) << 6 | continuation()?,
            0xE0..=0xEF => u16::from(first & 0x0F) << 12 | continuation()? << 6 | continuation()?,
            _ => return Err(DecodeError::InvalidUtf8),
        });
    }
    String::from_utf16(&units).map_err(|_| DecodeError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn written(value: &Nbt) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.put_nbt(value);
        writer.into_bytes()
    }

    #[test]
    fn string_root_known_answer() {
        let bytes = written(&Nbt::String("hi".to_owned()));
        assert_eq!(bytes, [TAG_STRING, 0, 2, b'h', b'i']);
    }

    #[test]
    fn compound_known_answer() {
        let value = Nbt::Compound(vec![("a".to_owned(), Nbt::Byte(1))]);
        assert_eq!(
            written(&value),
            [TAG_COMPOUND, TAG_BYTE, 0, 1, b'a', 1, TAG_END]
        );
        assert_eq!(value.get("a"), Some(&Nbt::Byte(1)));
        assert_eq!(value.get("b"), None);
    }

    #[test]
    fn lone_end_tag_is_no_value() {
        assert_eq!(Reader::new(&[TAG_END]).nbt(), Ok(None));
    }

    #[test]
    fn strings_use_modified_utf8() {
        // NUL takes two bytes; an emoji takes two three-byte surrogates.
        let bytes = written(&Nbt::String("\0😀".to_owned()));
        assert_eq!(
            bytes,
            [
                TAG_STRING, 0, 8, 0xC0, 0x80, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80
            ]
        );
        assert_eq!(
            Reader::new(&bytes).nbt(),
            Ok(Some(Nbt::String("\0😀".to_owned())))
        );
    }

    #[test]
    fn display_uses_command_notation() {
        let value = Nbt::Compound(vec![
            ("text".to_owned(), Nbt::String("hi".to_owned())),
            ("n".to_owned(), Nbt::List(vec![Nbt::Long(1), Nbt::Long(2)])),
            ("b".to_owned(), Nbt::ByteArray(vec![255])),
        ]);
        assert_eq!(
            value.to_string(),
            r#"{"text":"hi","n":[1L,2L],"b":[B;-1b]}"#
        );
    }

    #[test]
    fn forged_counts_are_rejected_before_allocating() {
        let mut bytes = vec![TAG_LONG_ARRAY];
        bytes.extend_from_slice(&i32::MAX.to_be_bytes());
        assert!(matches!(
            Reader::new(&bytes).nbt(),
            Err(DecodeError::LengthExceedsData { .. })
        ));

        // A list of end tags would otherwise need no bytes per element.
        let mut bytes = vec![TAG_LIST, TAG_END];
        bytes.extend_from_slice(&i32::MAX.to_be_bytes());
        assert!(Reader::new(&bytes).nbt().is_err());
    }

    #[test]
    fn excessive_nesting_is_rejected() {
        let mut bytes = vec![TAG_LIST];
        for _ in 0..MAX_DEPTH + 1 {
            bytes.extend_from_slice(&[TAG_LIST, 0, 0, 0, 1]);
        }
        assert!(matches!(
            Reader::new(&bytes).nbt(),
            Err(DecodeError::InvalidValue {
                what: "NBT nesting depth",
                ..
            })
        ));
    }

    fn nbt_strategy() -> impl Strategy<Value = Nbt> {
        let leaf = prop_oneof![
            any::<i8>().prop_map(Nbt::Byte),
            any::<i16>().prop_map(Nbt::Short),
            any::<i32>().prop_map(Nbt::Int),
            any::<i64>().prop_map(Nbt::Long),
            // NaN is not equal to itself, which would fail the round-trip comparison.
            (-1e6f32..1e6).prop_map(Nbt::Float),
            (-1e6f64..1e6).prop_map(Nbt::Double),
            any::<Vec<u8>>().prop_map(Nbt::ByteArray),
            any::<String>().prop_map(Nbt::String),
            any::<Vec<i32>>().prop_map(Nbt::IntArray),
            any::<Vec<i64>>().prop_map(Nbt::LongArray),
        ];
        leaf.prop_recursive(4, 64, 8, |inner| {
            prop_oneof![
                // Lists are homogeneous, so repeat one generated element.
                (inner.clone(), 0usize..4).prop_map(|(item, count)| Nbt::List(vec![item; count])),
                prop::collection::vec((any::<String>(), inner), 0..4).prop_map(Nbt::Compound),
            ]
        })
    }

    proptest! {
        #[test]
        fn values_round_trip(value in nbt_strategy()) {
            let bytes = written(&value);
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(reader.nbt(), Ok(Some(value)));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = Reader::new(&bytes).nbt();
        }
    }
}
