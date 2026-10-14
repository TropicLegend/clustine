//! Bytes that serde is told are bytes. A `Vec<u8>` is otherwise a sequence of
//! numbers, which postcard writes and reads a number at a time: a region sends an edge
//! a thousand chunks again after a merge or a split, and on land writing and reading
//! their blocks and light took 55 ms more that way. Postcard writes both as a length
//! and then the bytes, so what was written before reads the same.

use serde::de::{Error, SeqAccess, Visitor};
use serde::{Deserializer, Serializer};

pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(bytes)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    deserializer.deserialize_byte_buf(Bytes)
}

struct Bytes;

impl<'de> Visitor<'de> for Bytes {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("bytes")
    }

    fn visit_bytes<E: Error>(self, bytes: &[u8]) -> Result<Vec<u8>, E> {
        Ok(bytes.to_vec())
    }

    fn visit_byte_buf<E: Error>(self, bytes: Vec<u8>) -> Result<Vec<u8>, E> {
        Ok(bytes)
    }

    /// For a format that writes bytes as a sequence of numbers.
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
        let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 16));
        while let Some(byte) = seq.next_element()? {
            bytes.push(byte);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct AsBytes(#[serde(with = "super")] Vec<u8>);

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct AsNumbers(Vec<u8>);

    #[test]
    fn postcard_writes_bytes_as_it_wrote_a_sequence_of_numbers_and_reads_either() {
        for length in [0, 1, 127, 128, 2048, 70_000] {
            let bytes: Vec<u8> = (0..length).map(|at| (at * 7 % 256) as u8).collect();
            let new = postcard::to_stdvec(&AsBytes(bytes.clone())).unwrap();
            let old = postcard::to_stdvec(&AsNumbers(bytes.clone())).unwrap();
            assert_eq!(new, old, "{length} bytes");
            assert_eq!(postcard::from_bytes::<AsBytes>(&old).unwrap().0, bytes);
            assert_eq!(postcard::from_bytes::<AsNumbers>(&new).unwrap().0, bytes);
        }
    }
}
