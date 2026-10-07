//! Reading and writing the protocol's primitive data types.
//!
//! [`Writer`] appends values to a buffer and cannot fail. [`Reader`] consumes values from
//! a byte slice and returns a [`DecodeError`] for anything malformed; it never panics and
//! never allocates more than the input justifies, so it is safe on untrusted bytes.

use uuid::Uuid;

/// The longest string the protocol allows, in UTF-16 code units.
pub const MAX_STRING_LENGTH: usize = 32767;

/// Why a value could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of data")]
    UnexpectedEnd,
    #[error("VarInt is longer than 5 bytes")]
    VarIntTooLong,
    #[error("VarLong is longer than 10 bytes")]
    VarLongTooLong,
    #[error("negative length {0}")]
    NegativeLength(i32),
    #[error("length {length} exceeds the {remaining} bytes that are left")]
    LengthExceedsData { length: usize, remaining: usize },
    #[error("string is longer than {max} characters")]
    StringTooLong { max: usize },
    #[error("string is not valid UTF-8")]
    InvalidUtf8,
    #[error("{0} bytes left over after the packet")]
    TrailingBytes(usize),
    #[error("{value} is not a valid {what}")]
    InvalidValue { what: &'static str, value: i64 },
    #[error("packet id {0} does not exist in this protocol state")]
    UnknownPacket(i32),
}

/// A value that can be written to the wire.
pub trait Encode {
    fn encode(&self, w: &mut Writer);
}

/// A value that can be read from the wire.
pub trait Decode: Sized {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError>;
}

/// A block position, packed into 64 bits on the wire: 26 bits each for x and z, 12 for y.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// The number of bytes `value` occupies as a VarInt.
pub fn var_int_len(value: i32) -> usize {
    match value as u32 {
        0..=0x7F => 1,
        0x80..=0x3FFF => 2,
        0x4000..=0x1F_FFFF => 3,
        0x20_0000..=0xFFF_FFFF => 4,
        _ => 5,
    }
}

/// Appends protocol values to a growing buffer.
#[derive(Debug, Default)]
pub struct Writer {
    buffer: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }

    pub fn put_bool(&mut self, value: bool) {
        self.buffer.push(u8::from(value));
    }

    pub fn put_u8(&mut self, value: u8) {
        self.buffer.push(value);
    }

    pub fn put_i8(&mut self, value: i8) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_u16(&mut self, value: u16) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_i16(&mut self, value: i16) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_i32(&mut self, value: i32) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_i64(&mut self, value: i64) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_u64(&mut self, value: u64) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_f32(&mut self, value: f32) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_f64(&mut self, value: f64) {
        self.put_bytes(&value.to_be_bytes());
    }

    pub fn put_var_int(&mut self, value: i32) {
        let mut value = value as u32;
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                self.buffer.push(byte);
                return;
            }
            self.buffer.push(byte | 0x80);
        }
    }

    pub fn put_var_long(&mut self, value: i64) {
        let mut value = value as u64;
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                self.buffer.push(byte);
                return;
            }
            self.buffer.push(byte | 0x80);
        }
    }

    /// Writes a length or element count as a VarInt.
    ///
    /// # Panics
    ///
    /// If `length` does not fit a VarInt, which no well-formed packet comes close to.
    pub fn put_length(&mut self, length: usize) {
        let length = i32::try_from(length).expect("length fits a VarInt");
        self.put_var_int(length);
    }

    /// Writes raw bytes without a length prefix.
    pub fn put_bytes(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Writes a string as its UTF-8 byte length followed by the bytes.
    pub fn put_string(&mut self, value: &str) {
        self.put_length(value.len());
        self.put_bytes(value.as_bytes());
    }

    pub fn put_uuid(&mut self, value: Uuid) {
        self.put_bytes(value.as_bytes());
    }

    pub fn put_position(&mut self, value: Position) {
        let x = i64::from(value.x) & 0x3FF_FFFF;
        let y = i64::from(value.y) & 0xFFF;
        let z = i64::from(value.z) & 0x3FF_FFFF;
        self.put_i64(x << 38 | z << 12 | y);
    }

    /// Writes `items` as an element count followed by each element.
    pub fn put_array<T>(&mut self, items: &[T], mut put: impl FnMut(&mut Self, &T)) {
        self.put_length(items.len());
        for item in items {
            put(self, item);
        }
    }

    /// Writes a presence flag, followed by the value if there is one.
    pub fn put_option<T>(&mut self, value: Option<&T>, put: impl FnOnce(&mut Self, &T)) {
        self.put_bool(value.is_some());
        if let Some(value) = value {
            put(self, value);
        }
    }

    /// Writes a bit set, given as 64-bit words with bit 0 of the first word first.
    ///
    /// On the wire a bit set is a byte count followed by its bytes, lowest bits first,
    /// without trailing zero bytes.
    pub fn put_bit_set(&mut self, words: &[u64]) {
        let mut bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        self.put_length(bytes.len());
        self.put_bytes(&bytes);
    }
}

/// Consumes protocol values from a byte slice.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    input: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self { input }
    }

    /// The number of bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.input.len()
    }

    /// Succeeds only if every byte has been consumed.
    pub fn finish(self) -> Result<(), DecodeError> {
        match self.input.len() {
            0 => Ok(()),
            left => Err(DecodeError::TrailingBytes(left)),
        }
    }

    /// Consumes the next `count` bytes.
    pub fn bytes(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        let (taken, rest) = self
            .input
            .split_at_checked(count)
            .ok_or(DecodeError::UnexpectedEnd)?;
        self.input = rest;
        Ok(taken)
    }

    /// Consumes everything that is left.
    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.input)
    }

    fn array_of<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let bytes = self.bytes(N)?;
        Ok(bytes.try_into().expect("bytes() returned N bytes"))
    }

    /// Reads a boolean. Like the vanilla client and server, any non-zero byte is true.
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        Ok(self.u8()? != 0)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array_of::<1>()?[0])
    }

    pub fn i8(&mut self) -> Result<i8, DecodeError> {
        Ok(i8::from_be_bytes(self.array_of()?))
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array_of()?))
    }

    pub fn i16(&mut self) -> Result<i16, DecodeError> {
        Ok(i16::from_be_bytes(self.array_of()?))
    }

    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.array_of()?))
    }

    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_be_bytes(self.array_of()?))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.array_of()?))
    }

    pub fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_be_bytes(self.array_of()?))
    }

    pub fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_be_bytes(self.array_of()?))
    }

    /// Reads a VarInt. As in vanilla, bits beyond the 32nd in the fifth byte are ignored.
    pub fn var_int(&mut self) -> Result<i32, DecodeError> {
        let mut value = 0u32;
        for index in 0..5 {
            let byte = self.u8()?;
            value |= u32::from(byte & 0x7F) << (7 * index);
            if byte & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(DecodeError::VarIntTooLong)
    }

    /// Reads a VarLong. As in vanilla, bits beyond the 64th in the tenth byte are ignored.
    pub fn var_long(&mut self) -> Result<i64, DecodeError> {
        let mut value = 0u64;
        for index in 0..10 {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7F) << (7 * index);
            if byte & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(DecodeError::VarLongTooLong)
    }

    /// Reads a length or element count.
    ///
    /// Every element occupies at least one byte, so a count larger than the remaining
    /// input is rejected here, before anything is allocated for it.
    pub fn length(&mut self) -> Result<usize, DecodeError> {
        let raw = self.var_int()?;
        let length = usize::try_from(raw).map_err(|_| DecodeError::NegativeLength(raw))?;
        if length > self.remaining() {
            return Err(DecodeError::LengthExceedsData {
                length,
                remaining: self.remaining(),
            });
        }
        Ok(length)
    }

    /// Reads a string of at most `max_length` UTF-16 code units, the unit vanilla counts in.
    pub fn string(&mut self, max_length: usize) -> Result<String, DecodeError> {
        let too_long = DecodeError::StringTooLong { max: max_length };
        let byte_length = self.length()?;
        // A UTF-16 code unit takes at most three bytes in UTF-8.
        if byte_length > max_length.saturating_mul(3) {
            return Err(too_long);
        }
        let text =
            std::str::from_utf8(self.bytes(byte_length)?).map_err(|_| DecodeError::InvalidUtf8)?;
        if text.encode_utf16().count() > max_length {
            return Err(too_long);
        }
        Ok(text.to_owned())
    }

    /// Reads a namespaced identifier such as `minecraft:overworld`.
    pub fn identifier(&mut self) -> Result<String, DecodeError> {
        self.string(MAX_STRING_LENGTH)
    }

    pub fn uuid(&mut self) -> Result<Uuid, DecodeError> {
        Ok(Uuid::from_bytes(self.array_of()?))
    }

    pub fn position(&mut self) -> Result<Position, DecodeError> {
        let packed = self.i64()?;
        // Arithmetic shifts sign-extend each field.
        Ok(Position {
            x: (packed >> 38) as i32,
            y: (packed << 52 >> 52) as i32,
            z: (packed << 26 >> 38) as i32,
        })
    }

    /// Reads an element count followed by that many elements.
    pub fn array<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let count = self.length()?;
        // The count is bounded by the input length, but an element can be much larger in
        // memory than on the wire, so the vector grows as elements actually arrive.
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            items.push(item(self)?);
        }
        Ok(items)
    }

    /// Reads a presence flag, followed by the value if the flag is set.
    pub fn option<T>(
        &mut self,
        value: impl FnOnce(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Option<T>, DecodeError> {
        if self.bool()? {
            value(self).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Reads a bit set into 64-bit words, bit 0 of the first word first; see
    /// [`Writer::put_bit_set`].
    pub fn bit_set(&mut self) -> Result<Vec<u64>, DecodeError> {
        let length = self.length()?;
        let words = self.bytes(length)?.chunks(8).map(|chunk| {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            u64::from_le_bytes(word)
        });
        Ok(words.collect())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn written(put: impl FnOnce(&mut Writer)) -> Vec<u8> {
        let mut writer = Writer::new();
        put(&mut writer);
        writer.into_bytes()
    }

    /// Sample values from the protocol documentation.
    #[test]
    fn var_int_known_answers() {
        let cases: [(i32, &[u8]); 10] = [
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (255, &[0xFF, 0x01]),
            (25565, &[0xDD, 0xC7, 0x01]),
            (2097151, &[0xFF, 0xFF, 0x7F]),
            (i32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
            (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
            (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];
        for (value, bytes) in cases {
            assert_eq!(written(|w| w.put_var_int(value)), bytes, "{value}");
            assert_eq!(Reader::new(bytes).var_int(), Ok(value));
            assert_eq!(var_int_len(value), bytes.len(), "{value}");
        }
    }

    #[test]
    fn var_long_known_answers() {
        let cases: [(i64, &[u8]); 6] = [
            (0, &[0x00]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (
                i64::MAX,
                &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F],
            ),
            (
                -1,
                &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
            ),
            (
                i64::MIN,
                &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
            ),
        ];
        for (value, bytes) in cases {
            assert_eq!(written(|w| w.put_var_long(value)), bytes, "{value}");
            assert_eq!(Reader::new(bytes).var_long(), Ok(value));
        }
    }

    #[test]
    fn overlong_var_ints_are_rejected() {
        let mut reader = Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert_eq!(reader.var_int(), Err(DecodeError::VarIntTooLong));
        let mut reader = Reader::new(&[0x80; 11]);
        assert_eq!(reader.var_long(), Err(DecodeError::VarLongTooLong));
        assert_eq!(
            Reader::new(&[0x80]).var_int(),
            Err(DecodeError::UnexpectedEnd)
        );
    }

    /// The example from the protocol documentation.
    #[test]
    fn position_known_answer() {
        let position = Position {
            x: 18357644,
            y: 831,
            z: -20882616,
        };
        let bytes = [0x46, 0x07, 0x63, 0x2C, 0x15, 0xB4, 0x83, 0x3F];
        assert_eq!(written(|w| w.put_position(position)), bytes);
        assert_eq!(Reader::new(&bytes).position(), Ok(position));
    }

    #[test]
    fn string_known_answer() {
        let bytes = written(|w| w.put_string("hello"));
        assert_eq!(bytes, b"\x05hello");
        assert_eq!(Reader::new(&bytes).string(16).as_deref(), Ok("hello"));
    }

    #[test]
    fn string_length_is_counted_in_utf16_units() {
        // Three bytes in UTF-8 but one UTF-16 code unit.
        let euro = written(|w| w.put_string("€€"));
        assert_eq!(Reader::new(&euro).string(2).as_deref(), Ok("€€"));
        assert_eq!(
            Reader::new(&euro).string(1),
            Err(DecodeError::StringTooLong { max: 1 })
        );
        // Four bytes in UTF-8 and two UTF-16 code units.
        let emoji = written(|w| w.put_string("😀"));
        assert_eq!(
            Reader::new(&emoji).string(1),
            Err(DecodeError::StringTooLong { max: 1 })
        );
        assert!(Reader::new(&emoji).string(2).is_ok());
    }

    #[test]
    fn malformed_strings_are_rejected() {
        assert_eq!(
            Reader::new(&[0x02, 0xC3, 0x28]).string(16),
            Err(DecodeError::InvalidUtf8)
        );
        assert_eq!(
            Reader::new(&[0x05, b'h', b'i']).string(16),
            Err(DecodeError::LengthExceedsData {
                length: 5,
                remaining: 2
            })
        );
        let negative = written(|w| w.put_var_int(-1));
        assert_eq!(
            Reader::new(&negative).string(16),
            Err(DecodeError::NegativeLength(-1))
        );
    }

    #[test]
    fn huge_array_count_is_rejected_before_allocating() {
        let bytes = written(|w| w.put_var_int(i32::MAX));
        assert!(matches!(
            Reader::new(&bytes).array(Reader::u64),
            Err(DecodeError::LengthExceedsData { .. })
        ));
    }

    /// Light masks as sent by the official 26.3 server for a chunk of a normal world.
    #[test]
    fn bit_set_known_answers() {
        // Bits 9 and 10.
        assert_eq!(written(|w| w.put_bit_set(&[0x600])), [0x02, 0x00, 0x06]);
        assert_eq!(Reader::new(&[0x02, 0x00, 0x06]).bit_set(), Ok(vec![0x600]));
        // Bits 1, 2 and 5 to 8.
        assert_eq!(Reader::new(&[0x02, 0xE6, 0x01]).bit_set(), Ok(vec![0x1E6]));
        assert_eq!(written(|w| w.put_bit_set(&[])), [0x00]);
        assert_eq!(
            written(|w| w.put_bit_set(&[0, 1])),
            [9, 0, 0, 0, 0, 0, 0, 0, 0, 1]
        );
    }

    #[test]
    fn finish_reports_trailing_bytes() {
        let mut reader = Reader::new(&[1, 2, 3]);
        reader.u8().unwrap();
        assert_eq!(reader.finish(), Err(DecodeError::TrailingBytes(2)));
    }

    #[test]
    fn any_non_zero_byte_is_true() {
        assert_eq!(Reader::new(&[0]).bool(), Ok(false));
        assert_eq!(Reader::new(&[2]).bool(), Ok(true));
    }

    fn position_strategy() -> impl Strategy<Value = Position> {
        let horizontal = -(1 << 25)..(1 << 25);
        (horizontal.clone(), -2048..2048, horizontal).prop_map(|(x, y, z)| Position { x, y, z })
    }

    proptest! {
        #[test]
        fn var_int_round_trips(value: i32) {
            let bytes = written(|w| w.put_var_int(value));
            prop_assert_eq!(bytes.len(), var_int_len(value));
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(reader.var_int(), Ok(value));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn var_long_round_trips(value: i64) {
            let bytes = written(|w| w.put_var_long(value));
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(reader.var_long(), Ok(value));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn fixed_width_values_round_trip(
            a: bool, b: u8, c: i8, d: u16, e: i16, f: i32, g: i64, h: u64, i: f32, j: f64, k: u128,
        ) {
            let bytes = written(|w| {
                w.put_bool(a);
                w.put_u8(b);
                w.put_i8(c);
                w.put_u16(d);
                w.put_i16(e);
                w.put_i32(f);
                w.put_i64(g);
                w.put_u64(h);
                w.put_f32(i);
                w.put_f64(j);
                w.put_uuid(Uuid::from_u128(k));
            });
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(reader.bool(), Ok(a));
            prop_assert_eq!(reader.u8(), Ok(b));
            prop_assert_eq!(reader.i8(), Ok(c));
            prop_assert_eq!(reader.u16(), Ok(d));
            prop_assert_eq!(reader.i16(), Ok(e));
            prop_assert_eq!(reader.i32(), Ok(f));
            prop_assert_eq!(reader.i64(), Ok(g));
            prop_assert_eq!(reader.u64(), Ok(h));
            // Compared as bits so that NaN payloads count.
            prop_assert_eq!(reader.f32().map(f32::to_bits), Ok(i.to_bits()));
            prop_assert_eq!(reader.f64().map(f64::to_bits), Ok(j.to_bits()));
            prop_assert_eq!(reader.uuid(), Ok(Uuid::from_u128(k)));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn strings_round_trip(value: String) {
            let bytes = written(|w| w.put_string(&value));
            let mut reader = Reader::new(&bytes);
            let limit = value.encode_utf16().count();
            prop_assert_eq!(reader.string(limit), Ok(value));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn positions_round_trip(position in position_strategy()) {
            let bytes = written(|w| w.put_position(position));
            prop_assert_eq!(Reader::new(&bytes).position(), Ok(position));
        }

        #[test]
        fn collections_round_trip(
            numbers: Vec<i32>, words: Vec<u64>, maybe: Option<i64>,
        ) {
            let bytes = written(|w| {
                w.put_array(&numbers, |w, number| w.put_var_int(*number));
                w.put_bit_set(&words);
                w.put_option(maybe.as_ref(), |w, value| w.put_i64(*value));
            });
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(reader.array(Reader::var_int), Ok(numbers));
            // Trailing zero words carry no bits and are not transmitted.
            let mut words = words;
            while words.last() == Some(&0) {
                words.pop();
            }
            prop_assert_eq!(reader.bit_set(), Ok(words));
            prop_assert_eq!(reader.option(Reader::i64), Ok(maybe));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        /// Every reader method returns instead of panicking, whatever the input.
        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>, max_length in 0usize..40000) {
            let _ = Reader::new(&bytes).bool();
            let _ = Reader::new(&bytes).i16();
            let _ = Reader::new(&bytes).i64();
            let _ = Reader::new(&bytes).f64();
            let _ = Reader::new(&bytes).var_int();
            let _ = Reader::new(&bytes).var_long();
            let _ = Reader::new(&bytes).length();
            let _ = Reader::new(&bytes).string(max_length);
            let _ = Reader::new(&bytes).identifier();
            let _ = Reader::new(&bytes).uuid();
            let _ = Reader::new(&bytes).position();
            let _ = Reader::new(&bytes).bit_set();
            let _ = Reader::new(&bytes).array(|r| r.string(max_length));
            let _ = Reader::new(&bytes).option(|r| r.array(Reader::var_long));
        }
    }
}
