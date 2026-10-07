//! The contents of a chunk as they travel inside the chunk packet.
//!
//! A chunk column is sent as a run of 16×16×16 sections from bottom to top. Each section
//! stores its block states and its biomes in a paletted container: a list of the
//! distinct values plus, per position, a small index into that list. Indices are packed
//! into 64-bit words without ever spanning two words.

use crate::codec::{DecodeError, Reader, Writer};

/// Block positions in a section.
pub const BLOCKS_PER_SECTION: usize = 4096;
/// Biome cells in a section: biomes have a resolution of 4×4×4 blocks.
pub const BIOMES_PER_SECTION: usize = 64;
/// Columns in a chunk, and therefore entries in a heightmap.
pub const COLUMNS_PER_CHUNK: usize = 256;

/// The number of bits needed to tell `count` values apart: `ceil(log2(count))`.
///
/// This is the width of a container that stores registry ids directly, where `count` is
/// the size of the registry.
pub const fn bits_for(count: usize) -> u8 {
    (usize::BITS - count.saturating_sub(1).leading_zeros()) as u8
}

/// What a paletted container holds, which decides how it is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteKind {
    /// Number of positions.
    entries: usize,
    /// Indices into a palette are at least this wide.
    min_indirect_bits: u8,
    /// With more distinct values than this many bits can index, ids are stored directly.
    max_indirect_bits: u8,
    /// Width of a directly stored id.
    direct_bits: u8,
}

impl PaletteKind {
    /// Block states, given the number of block states that exist.
    pub const fn blocks(state_count: usize) -> Self {
        Self {
            entries: BLOCKS_PER_SECTION,
            min_indirect_bits: 4,
            max_indirect_bits: 8,
            direct_bits: bits_for(state_count),
        }
    }

    /// Biomes, given the number of biomes in the registry sent to the client.
    pub const fn biomes(biome_count: usize) -> Self {
        Self {
            entries: BIOMES_PER_SECTION,
            min_indirect_bits: 1,
            max_indirect_bits: 3,
            direct_bits: bits_for(biome_count),
        }
    }
}

/// The values of a paletted container, by position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PalettedContainer {
    /// Every position holds this value.
    Single(i32),
    /// One value per position.
    Values(Vec<i32>),
}

impl PalettedContainer {
    /// Builds a container from one value per position, noticing when all are equal.
    pub fn from_values(values: Vec<i32>) -> Self {
        match values.split_first() {
            Some((first, rest)) if rest.iter().all(|value| value == first) => Self::Single(*first),
            _ => Self::Values(values),
        }
    }

    /// The value at `index`.
    pub fn get(&self, index: usize) -> i32 {
        match self {
            Self::Single(value) => *value,
            Self::Values(values) => values[index],
        }
    }

    pub fn encode(&self, kind: PaletteKind, w: &mut Writer) {
        let values = match self {
            Self::Single(value) => {
                w.put_u8(0);
                w.put_var_int(*value);
                return;
            }
            Self::Values(values) => values,
        };
        debug_assert_eq!(values.len(), kind.entries);

        // Distinct values in order of first appearance.
        let mut palette: Vec<i32> = Vec::new();
        let mut indices = Vec::with_capacity(values.len());
        for value in values {
            let index = match palette.iter().position(|entry| entry == value) {
                Some(index) => index,
                None => {
                    palette.push(*value);
                    palette.len() - 1
                }
            };
            indices.push(index as u64);
        }

        let bits = bits_for(palette.len()).max(kind.min_indirect_bits);
        if bits <= kind.max_indirect_bits {
            w.put_u8(bits);
            w.put_array(&palette, |w, entry| w.put_var_int(*entry));
            put_packed(w, indices.into_iter(), bits);
        } else {
            w.put_u8(kind.direct_bits);
            put_packed(
                w,
                values.iter().map(|value| *value as u64),
                kind.direct_bits,
            );
        }
    }

    pub fn decode(kind: PaletteKind, r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let bits = r.u8()?;
        if bits == 0 {
            return Ok(Self::Single(r.var_int()?));
        }
        if bits > 32 {
            return Err(DecodeError::InvalidValue {
                what: "palette entry width",
                value: bits.into(),
            });
        }
        if bits > kind.max_indirect_bits {
            let values = read_packed(r, kind.entries, bits)?;
            return Ok(Self::Values(
                values.into_iter().map(|value| value as i32).collect(),
            ));
        }
        let palette = r.array(Reader::var_int)?;
        let values = read_packed(r, kind.entries, bits)?
            .into_iter()
            .map(|index| {
                palette
                    .get(index as usize)
                    .copied()
                    .ok_or(DecodeError::InvalidValue {
                        what: "palette index",
                        value: index as i64,
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self::Values(values))
    }
}

/// One 16×16×16 section of a chunk column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionData {
    /// Number of blocks that are not air; lets the client skip empty sections.
    pub block_count: i16,
    /// Number of blocks that contain a fluid.
    pub fluid_count: i16,
    /// Block state ids, indexed by `y << 8 | z << 4 | x`.
    pub blocks: PalettedContainer,
    /// Biome ids, indexed by `y << 4 | z << 2 | x` in 4-block cells.
    pub biomes: PalettedContainer,
}

/// Encodes the sections of a chunk column, bottom to top, as the chunk packet's data.
pub fn encode_sections(
    sections: &[SectionData],
    blocks: PaletteKind,
    biomes: PaletteKind,
) -> Vec<u8> {
    let mut w = Writer::new();
    for section in sections {
        w.put_i16(section.block_count);
        w.put_i16(section.fluid_count);
        section.blocks.encode(blocks, &mut w);
        section.biomes.encode(biomes, &mut w);
    }
    w.into_bytes()
}

/// Decodes `count` sections from a chunk packet's data.
pub fn decode_sections(
    data: &[u8],
    count: usize,
    blocks: PaletteKind,
    biomes: PaletteKind,
) -> Result<Vec<SectionData>, DecodeError> {
    let mut r = Reader::new(data);
    let mut sections = Vec::new();
    for _ in 0..count {
        sections.push(SectionData {
            block_count: r.i16()?,
            fluid_count: r.i16()?,
            blocks: PalettedContainer::decode(blocks, &mut r)?,
            biomes: PalettedContainer::decode(biomes, &mut r)?,
        });
    }
    // Vanilla pads the buffer it reserves for sections, so trailing bytes are allowed.
    Ok(sections)
}

/// Packs the heights of a chunk's 256 columns for a heightmap, `bits` wide each.
///
/// A height is the y coordinate above the highest matching block, counted from the
/// bottom of the world, so `bits` is `bits_for(world height + 1)`.
pub fn pack_heightmap(heights: &[u16; COLUMNS_PER_CHUNK], bits: u8) -> Vec<u64> {
    let mut w = Writer::new();
    put_packed(
        &mut w,
        heights.iter().map(|height| u64::from(*height)),
        bits,
    );
    w.as_bytes()
        .chunks_exact(8)
        .map(|word| u64::from_be_bytes(word.try_into().expect("chunks of 8 bytes")))
        .collect()
}

/// The inverse of [`pack_heightmap`]. Returns `None` if `words` is too short.
pub fn unpack_heightmap(words: &[u64], bits: u8) -> Option<[u16; COLUMNS_PER_CHUNK]> {
    let per_word = entries_per_word(bits);
    let mask = (1u64 << bits) - 1;
    let mut heights = [0; COLUMNS_PER_CHUNK];
    for (index, height) in heights.iter_mut().enumerate() {
        let word = words.get(index / per_word)?;
        *height = (word >> (index % per_word * usize::from(bits)) & mask) as u16;
    }
    Some(heights)
}

fn entries_per_word(bits: u8) -> usize {
    64 / usize::from(bits)
}

/// Writes `values` packed into 64-bit words, lowest bits first, without a count.
fn put_packed(w: &mut Writer, values: impl Iterator<Item = u64>, bits: u8) {
    let per_word = entries_per_word(bits);
    let mut word = 0;
    let mut filled = 0;
    for value in values {
        word |= value << (filled * usize::from(bits));
        filled += 1;
        if filled == per_word {
            w.put_u64(word);
            word = 0;
            filled = 0;
        }
    }
    if filled > 0 {
        w.put_u64(word);
    }
}

/// Reads `count` values of `bits` bits each, as written by [`put_packed`].
fn read_packed(r: &mut Reader<'_>, count: usize, bits: u8) -> Result<Vec<u64>, DecodeError> {
    let per_word = entries_per_word(bits);
    let mask = (1u64 << bits) - 1;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count.div_ceil(per_word) {
        let word = r.u64()?;
        for slot in 0..per_word {
            if values.len() == count {
                break;
            }
            values.push(word >> (slot * usize::from(bits)) & mask);
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const BLOCKS: PaletteKind = PaletteKind::blocks(35_723);
    const BIOMES: PaletteKind = PaletteKind::biomes(67);

    fn encoded(container: &PalettedContainer, kind: PaletteKind) -> Vec<u8> {
        let mut w = Writer::new();
        container.encode(kind, &mut w);
        w.into_bytes()
    }

    #[test]
    fn direct_widths_follow_registry_sizes() {
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(16), 4);
        assert_eq!(bits_for(17), 5);
        // Minecraft 26.3 has 35,723 block states and 67 biomes.
        assert_eq!(BLOCKS.direct_bits, 16);
        assert_eq!(BIOMES.direct_bits, 7);
        // The overworld is 384 blocks high.
        assert_eq!(bits_for(384 + 1), 9);
    }

    #[test]
    fn single_value_known_answer() {
        assert_eq!(encoded(&PalettedContainer::Single(1), BLOCKS), [0, 1]);
        assert_eq!(
            PalettedContainer::from_values(vec![5; 4096]),
            PalettedContainer::Single(5)
        );
    }

    #[test]
    fn indirect_known_answer() {
        // Two values use the 4-bit minimum for blocks: 16 indices per word, 256 words.
        let mut values = vec![0; BLOCKS_PER_SECTION];
        values[1] = 9;
        let bytes = encoded(&PalettedContainer::Values(values), BLOCKS);
        assert_eq!(bytes[..4], [4, 2, 0, 9]);
        assert_eq!(bytes[4..12], 0x10u64.to_be_bytes());
        assert_eq!(bytes.len(), 4 + 256 * 8);
    }

    #[test]
    fn biomes_use_one_bit_for_two_values() {
        let mut values = vec![3; BIOMES_PER_SECTION];
        values[63] = 7;
        let bytes = encoded(&PalettedContainer::Values(values), BIOMES);
        assert_eq!(bytes[..4], [1, 2, 3, 7]);
        assert_eq!(bytes[4..], (1u64 << 63).to_be_bytes());
    }

    #[test]
    fn many_values_are_stored_directly() {
        // 257 distinct values need 9 bits, more than a block palette may index.
        let values: Vec<i32> = (0..BLOCKS_PER_SECTION as i32).map(|i| i % 257).collect();
        let bytes = encoded(&PalettedContainer::Values(values), BLOCKS);
        assert_eq!(bytes[0], 16);
        // Four 16-bit ids per word and no palette.
        assert_eq!(bytes.len(), 1 + 1024 * 8);
    }

    #[test]
    fn entries_do_not_span_words() {
        // 5-bit indices: 12 per word with 4 bits unused, so 4096 need 342 words.
        let values: Vec<i32> = (0..BLOCKS_PER_SECTION as i32).map(|i| i % 17).collect();
        let bytes = encoded(&PalettedContainer::Values(values), BLOCKS);
        assert_eq!(bytes[0], 5);
        assert_eq!(bytes.len(), 1 + 1 + 17 + 342 * 8);
    }

    #[test]
    fn out_of_range_palette_index_is_rejected() {
        let mut bytes = vec![1, 1, 3];
        bytes.extend_from_slice(&1u64.to_be_bytes());
        assert!(matches!(
            PalettedContainer::decode(BIOMES, &mut Reader::new(&bytes)),
            Err(DecodeError::InvalidValue {
                what: "palette index",
                ..
            })
        ));
    }

    #[test]
    fn heightmap_known_answer() {
        let mut heights = [0; COLUMNS_PER_CHUNK];
        heights[0] = 4;
        heights[6] = 384;
        let words = pack_heightmap(&heights, 9);
        // Seven 9-bit heights per word.
        assert_eq!(words.len(), 37);
        assert_eq!(words[0], 4 | 384 << 54);
        assert_eq!(unpack_heightmap(&words, 9), Some(heights));
        assert_eq!(unpack_heightmap(&words[..36], 9), None);
    }

    fn values_strategy(entries: usize, distinct: i32) -> impl Strategy<Value = Vec<i32>> {
        prop::collection::vec(0..distinct, entries)
    }

    proptest! {
        #[test]
        fn block_containers_round_trip(
            // Covers single-valued, every indirect width and direct storage.
            values in (1..600i32).prop_flat_map(|distinct| values_strategy(BLOCKS_PER_SECTION, distinct)),
        ) {
            let container = PalettedContainer::from_values(values);
            let bytes = encoded(&container, BLOCKS);
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(PalettedContainer::decode(BLOCKS, &mut reader), Ok(container));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn biome_containers_round_trip(
            values in (1..40i32).prop_flat_map(|distinct| values_strategy(BIOMES_PER_SECTION, distinct)),
        ) {
            let container = PalettedContainer::from_values(values);
            let bytes = encoded(&container, BIOMES);
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(PalettedContainer::decode(BIOMES, &mut reader), Ok(container));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn sections_round_trip(
            block_count: i16,
            fluid_count: i16,
            block in 0..35_723i32,
            biomes in values_strategy(BIOMES_PER_SECTION, 67),
        ) {
            let sections = vec![
                SectionData {
                    block_count,
                    fluid_count,
                    blocks: PalettedContainer::Single(block),
                    biomes: PalettedContainer::from_values(biomes),
                };
                3
            ];
            let bytes = encode_sections(&sections, BLOCKS, BIOMES);
            prop_assert_eq!(decode_sections(&bytes, 3, BLOCKS, BIOMES), Ok(sections));
        }

        #[test]
        fn heightmaps_round_trip(heights in prop::array::uniform32(0u16..512)) {
            let mut all = [0; COLUMNS_PER_CHUNK];
            for (index, height) in all.iter_mut().enumerate() {
                *height = heights[index % 32];
            }
            prop_assert_eq!(unpack_heightmap(&pack_heightmap(&all, 9), 9), Some(all));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>, count in 0usize..30) {
            let _ = decode_sections(&bytes, count, BLOCKS, BIOMES);
            let _ = PalettedContainer::decode(BLOCKS, &mut Reader::new(&bytes));
            let _ = PalettedContainer::decode(BIOMES, &mut Reader::new(&bytes));
        }
    }
}
