//! What is stored for a chunk: where its sections are.
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Chunk x, z | i32, i32 |
//! | Y of the lowest block | i32 |
//! | Tick the chunk was saved at | u64 |
//! | Epoch of the region that saved it | u64 |
//! | Air biome | u16 |
//! | Section count | u16 |
//! | Per section, bottom to top | u8 flag: 0 for "all air in the air biome", 1 followed by a 32-byte hash |
//! | CRC-32 of everything before | u32 |
//!
//! All integers are big-endian.

use clustine_world::{Biome, Chunk, ChunkPos, Section};

use crate::bytes::Input;
use crate::section::{decode_section, encode_section};
use crate::{FORMAT_VERSION, FormatError, Hash};

/// A stored chunk: for each section the address of its content, or `None` for a section
/// of nothing but air in the air biome, which needs no storage at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkManifest {
    pub position: ChunkPos,
    /// The y coordinate of the chunk's lowest block.
    pub min_y: i32,
    /// The tick of the region at which the chunk was saved.
    pub tick: u64,
    /// The ownership epoch of the region that saved the chunk. Always 1 until regions
    /// change owners.
    pub epoch: u64,
    /// The biome of the sections that are `None`.
    pub air_biome: Biome,
    /// From bottom to top.
    pub sections: Vec<Option<Hash>>,
}

impl ChunkManifest {
    /// Describes `chunk`. Returns the manifest and, for every section that needs
    /// storing, its address and canonical encoding.
    pub fn describe(position: ChunkPos, chunk: &Chunk, tick: u64) -> (Self, Vec<(Hash, Vec<u8>)>) {
        // Most of a chunk is air, so the biome of its top section is the best choice.
        let air_biome = chunk.sections().last().map_or(Biome(0), Section::biome);
        let mut blobs = Vec::new();
        let sections = chunk
            .sections()
            .iter()
            .map(|section| {
                if section.non_air_count() == 0 && section.biome() == air_biome {
                    return None;
                }
                let canonical = encode_section(section);
                let hash = Hash::of(&canonical);
                blobs.push((hash, canonical));
                Some(hash)
            })
            .collect();
        let manifest = Self {
            position,
            min_y: chunk.min_y(),
            tick,
            epoch: 1,
            air_biome,
            sections,
        };
        (manifest, blobs)
    }

    /// Rebuilds the chunk. `canonical` returns the canonical encoding stored under an
    /// address.
    pub fn restore<E: From<FormatError>>(
        &self,
        mut canonical: impl FnMut(&Hash) -> Result<Vec<u8>, E>,
    ) -> Result<Chunk, E> {
        let air = Section::filled(clustine_data::blocks::AIR, self.air_biome);
        let mut sections = Vec::with_capacity(self.sections.len());
        for section in &self.sections {
            sections.push(match section {
                None => air.clone(),
                Some(hash) => {
                    let bytes = canonical(hash)?;
                    // What is stored under an address must be what the address says.
                    if Hash::of(&bytes) != *hash {
                        return Err(FormatError::ChecksumMismatch.into());
                    }
                    decode_section(&bytes)?
                }
            });
        }
        Ok(Chunk::from_sections(self.min_y, sections))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT_VERSION];
        out.extend_from_slice(&self.position.x.to_be_bytes());
        out.extend_from_slice(&self.position.z.to_be_bytes());
        out.extend_from_slice(&self.min_y.to_be_bytes());
        out.extend_from_slice(&self.tick.to_be_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.air_biome.0.to_be_bytes());
        out.extend_from_slice(&(self.sections.len() as u16).to_be_bytes());
        for section in &self.sections {
            match section {
                None => out.push(0),
                Some(hash) => {
                    out.push(1);
                    out.extend_from_slice(&hash.0);
                }
            }
        }
        out.extend_from_slice(&crc32fast::hash(&out).to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (body, checksum) = bytes
            .split_last_chunk::<4>()
            .ok_or(FormatError::Truncated)?;
        if crc32fast::hash(body) != u32::from_be_bytes(*checksum) {
            return Err(FormatError::ChecksumMismatch);
        }
        let mut input = Input(body);
        let version = input.u8()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let position = ChunkPos::new(input.i32()?, input.i32()?);
        let min_y = input.i32()?;
        let tick = input.u64()?;
        let epoch = input.u64()?;
        let air_biome = Biome(input.u16()?);
        let count = usize::from(input.u16()?);
        let mut sections = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            sections.push(match input.u8()? {
                0 => None,
                1 => Some(Hash(input.take(32)?.try_into().expect("32 bytes"))),
                _ => return Err(FormatError::Corrupt("section flag")),
            });
        }
        input.finish()?;
        Ok(Self {
            position,
            min_y,
            tick,
            epoch,
            air_biome,
            sections,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use clustine_data::{DIMENSION_TYPES, blocks};
    use proptest::prelude::*;

    use super::*;

    const PLAINS: Biome = Biome(41);

    fn empty() -> Chunk {
        let overworld = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        Chunk::empty(overworld, PLAINS)
    }

    fn flat() -> Chunk {
        let mut chunk = empty();
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, -64, z, blocks::BEDROCK);
                chunk.set(x, -63, z, blocks::DIRT);
            }
        }
        chunk
    }

    /// Stores the sections of `chunk` in a map and returns its manifest.
    fn store(chunk: &Chunk, blobs: &mut BTreeMap<Hash, Vec<u8>>) -> ChunkManifest {
        let (manifest, sections) = ChunkManifest::describe(ChunkPos::new(3, -4), chunk, 77);
        blobs.extend(sections);
        manifest
    }

    fn restore(
        manifest: &ChunkManifest,
        blobs: &BTreeMap<Hash, Vec<u8>>,
    ) -> Result<Chunk, FormatError> {
        manifest.restore(|hash| {
            blobs
                .get(hash)
                .cloned()
                .ok_or(FormatError::Corrupt("missing section"))
        })
    }

    #[test]
    fn air_needs_no_storage() {
        let mut blobs = BTreeMap::new();
        let manifest = store(&empty(), &mut blobs);
        assert!(blobs.is_empty());
        assert_eq!(manifest.sections, vec![None; 24]);
        assert_eq!(
            (manifest.min_y, manifest.tick, manifest.epoch),
            (-64, 77, 1)
        );
        assert_eq!(restore(&manifest, &blobs), Ok(empty()));
    }

    #[test]
    fn only_sections_with_content_are_stored() {
        let mut blobs = BTreeMap::new();
        let manifest = store(&flat(), &mut blobs);
        assert_eq!(blobs.len(), 1);
        assert!(manifest.sections[0].is_some());
        assert!(manifest.sections[1..].iter().all(Option::is_none));
        assert_eq!(restore(&manifest, &blobs), Ok(flat()));
    }

    #[test]
    fn equal_sections_share_one_address() {
        let mut blobs = BTreeMap::new();
        let mut chunk = empty();
        for y in [-64, -48, 0, 304] {
            chunk.set(1, y, 2, blocks::STONE);
        }
        let manifest = store(&chunk, &mut blobs);
        // Four sections with the same content, stored once; and the same again for
        // another chunk.
        assert_eq!(blobs.len(), 1);
        assert_eq!(manifest.sections.iter().flatten().count(), 4);
        store(&chunk, &mut blobs);
        assert_eq!(blobs.len(), 1);
        assert_eq!(restore(&manifest, &blobs), Ok(chunk));
    }

    #[test]
    fn manifests_round_trip_and_detect_damage() {
        let mut blobs = BTreeMap::new();
        let manifest = store(&flat(), &mut blobs);
        let bytes = manifest.encode();
        // Header, 24 flags, one hash, checksum.
        assert_eq!(bytes.len(), 33 + 24 + 32 + 4);
        assert_eq!(ChunkManifest::decode(&bytes), Ok(manifest));

        for index in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[index] ^= 0x40;
            assert_eq!(
                ChunkManifest::decode(&damaged),
                Err(FormatError::ChecksumMismatch),
                "byte {index}"
            );
        }
        for length in 0..bytes.len() {
            assert!(
                ChunkManifest::decode(&bytes[..length]).is_err(),
                "{length} bytes"
            );
        }
    }

    #[test]
    fn a_section_that_is_not_what_its_address_says_is_rejected() {
        let mut blobs = BTreeMap::new();
        let manifest = store(&flat(), &mut blobs);
        let hash = manifest.sections[0].unwrap();
        let other = encode_section(&Section::filled(blocks::STONE, PLAINS));
        blobs.insert(hash, other);
        assert_eq!(
            restore(&manifest, &blobs),
            Err(FormatError::ChecksumMismatch)
        );

        blobs.clear();
        assert_eq!(
            restore(&manifest, &blobs),
            Err(FormatError::Corrupt("missing section"))
        );
    }

    proptest! {
        #[test]
        fn chunks_round_trip(changes in prop::collection::vec((0usize..16, -64i32..320, 0usize..16, 0u16..50), 0..200)) {
            let mut chunk = empty();
            for (x, y, z, state) in changes {
                chunk.set(x, y, z, clustine_data::BlockState(state));
            }
            let mut blobs = BTreeMap::new();
            let manifest = store(&chunk, &mut blobs);
            let decoded = ChunkManifest::decode(&manifest.encode()).unwrap();
            prop_assert_eq!(restore(&decoded, &blobs), Ok(chunk));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ChunkManifest::decode(&bytes);
        }
    }
}
