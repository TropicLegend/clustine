//! How a section is encoded and stored.
//!
//! The canonical encoding of a section is:
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Biome | u16 |
//! | Palette length `n` | u16, 1 to 4096 |
//! | Palette | `n` × u16 block state ids, strictly ascending, each used at least once |
//! | Indices, only if `n` > 1 | u64 words holding one `bits_for(n)`-bit palette index per block, lowest bits first, none spanning two words |
//!
//! All integers are big-endian and blocks are in the order `y << 8 | z << 4 | x`. Equal
//! sections have equal encodings, whatever their history, which is what makes the hash
//! of the encoding usable as an address.
//!
//! What is written to storage is the canonical encoding behind a codec byte: 0 for
//! as is, 1 for compressed with zstd. The address is always that of the uncompressed
//! encoding, so the compression can change without changing any address.

use clustine_data::BlockState;
use clustine_world::{BLOCKS_PER_SECTION, Biome, Section};

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

const CODEC_RAW: u8 = 0;
const CODEC_ZSTD: u8 = 1;
const ZSTD_LEVEL: i32 = 3;

/// The number of bits needed for an index into a palette of `length` entries.
fn bits_for(length: usize) -> u32 {
    usize::BITS - (length - 1).leading_zeros()
}

/// The canonical encoding of `section`.
pub fn encode_section(section: &Section) -> Vec<u8> {
    let states: Vec<BlockState> = section.states().collect();
    let mut palette = states.clone();
    palette.sort_unstable();
    palette.dedup();

    let mut out = vec![FORMAT_VERSION];
    out.extend_from_slice(&section.biome().0.to_be_bytes());
    out.extend_from_slice(&(palette.len() as u16).to_be_bytes());
    for state in &palette {
        out.extend_from_slice(&state.0.to_be_bytes());
    }
    if palette.len() > 1 {
        let bits = bits_for(palette.len());
        let per_word = (u64::BITS / bits) as usize;
        for blocks in states.chunks(per_word) {
            let mut word = 0u64;
            for (slot, state) in blocks.iter().enumerate() {
                let index = palette
                    .binary_search(state)
                    .expect("the palette holds every state");
                word |= (index as u64) << (slot as u32 * bits);
            }
            out.extend_from_slice(&word.to_be_bytes());
        }
    }
    out
}

/// Decodes a canonical encoding. Anything that [`encode_section`] would not have
/// produced is rejected, so that one section never has two addresses.
pub fn decode_section(canonical: &[u8]) -> Result<Section, FormatError> {
    let mut input = Input(canonical);
    let version = input.u8()?;
    if version != FORMAT_VERSION {
        return Err(FormatError::UnsupportedVersion(version));
    }
    let biome = Biome(input.u16()?);
    let length = usize::from(input.u16()?);
    if length == 0 || length > BLOCKS_PER_SECTION {
        return Err(FormatError::Corrupt("palette length"));
    }
    let palette = (0..length)
        .map(|_| input.u16().map(BlockState))
        .collect::<Result<Vec<_>, _>>()?;
    if !palette.is_sorted_by(|a, b| a < b) {
        return Err(FormatError::Corrupt("palette is not strictly ascending"));
    }
    if length == 1 {
        input.finish()?;
        return Ok(Section::filled(palette[0], biome));
    }

    let bits = bits_for(length);
    let per_word = (u64::BITS / bits) as usize;
    let mask = (1u64 << bits) - 1;
    let mut states = Vec::with_capacity(BLOCKS_PER_SECTION);
    let mut used = vec![false; length];
    while states.len() < BLOCKS_PER_SECTION {
        let word = input.u64()?;
        for slot in 0..per_word.min(BLOCKS_PER_SECTION - states.len()) {
            let index = (word >> (slot as u32 * bits) & mask) as usize;
            let state = palette
                .get(index)
                .ok_or(FormatError::Corrupt("palette index"))?;
            used[index] = true;
            states.push(*state);
        }
    }
    input.finish()?;
    if used.contains(&false) {
        return Err(FormatError::Corrupt("unused palette entry"));
    }
    let states: Box<[BlockState; BLOCKS_PER_SECTION]> = states
        .into_boxed_slice()
        .try_into()
        .expect("exactly one section of blocks");
    Ok(Section::from_states(states, biome))
}

/// Wraps a canonical encoding for storage, compressing it if that makes it smaller.
pub fn pack(canonical: &[u8]) -> Vec<u8> {
    let compressed = zstd::bulk::compress(canonical, ZSTD_LEVEL).ok();
    let (codec, payload) = match &compressed {
        Some(compressed) if compressed.len() < canonical.len() => {
            (CODEC_ZSTD, compressed.as_slice())
        }
        _ => (CODEC_RAW, canonical),
    };
    let mut stored = Vec::with_capacity(1 + payload.len());
    stored.push(codec);
    stored.extend_from_slice(payload);
    stored
}

/// Recovers the canonical encoding from what [`pack`] produced.
pub fn unpack(stored: &[u8]) -> Result<Vec<u8>, FormatError> {
    // The largest canonical encoding: a palette of 4096 entries and 16-bit indices.
    const MAX_CANONICAL_LENGTH: usize = 5 + 2 * BLOCKS_PER_SECTION + 8 * 1024 + 64;
    let (codec, payload) = stored.split_first().ok_or(FormatError::Truncated)?;
    match *codec {
        CODEC_RAW => Ok(payload.to_vec()),
        CODEC_ZSTD => zstd::bulk::decompress(payload, MAX_CANONICAL_LENGTH)
            .map_err(|_| FormatError::Corrupt("compressed data")),
        other => Err(FormatError::UnknownCodec(other)),
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use proptest::prelude::*;

    use super::*;
    use crate::Hash;

    const PLAINS: Biome = Biome(41);

    fn flat_ground() -> Section {
        let mut section = Section::filled(blocks::AIR, PLAINS);
        for z in 0..16 {
            for x in 0..16 {
                section.set(x, 0, z, blocks::BEDROCK);
                section.set(x, 1, z, blocks::DIRT);
                section.set(x, 2, z, blocks::DIRT);
                section.set(x, 3, z, blocks::GRASS_BLOCK);
            }
        }
        section
    }

    #[test]
    fn uniform_section_known_answer() {
        let section = Section::filled(blocks::STONE, Biome(0x0102));
        // Version, biome, palette of one, stone.
        assert_eq!(encode_section(&section), [1, 1, 2, 0, 1, 0, 1]);
        assert_eq!(decode_section(&[1, 1, 2, 0, 1, 0, 1]), Ok(section));
    }

    #[test]
    fn two_states_use_one_bit_per_block() {
        let mut section = Section::filled(blocks::AIR, Biome(0));
        section.set(1, 0, 0, blocks::STONE);
        let bytes = encode_section(&section);
        assert_eq!(bytes[..9], [1, 0, 0, 0, 2, 0, 0, 0, 1]);
        // 4096 one-bit indices fill 64 words; the second block is the stone.
        assert_eq!(bytes.len(), 9 + 64 * 8);
        assert_eq!(bytes[9..17], 2u64.to_be_bytes());
        assert!(bytes[17..].iter().all(|byte| *byte == 0));
    }

    /// The encoding must never change for existing worlds: this pins it.
    #[test]
    fn the_encoding_of_flat_ground_is_pinned() {
        let bytes = encode_section(&flat_ground());
        // Four states take two bits per block: 32 blocks per word, 128 words.
        assert_eq!(bytes.len(), 5 + 4 * 2 + 128 * 8);
        assert_eq!(
            Hash::of(&bytes).to_string(),
            "45579a39ba2bb3ca03e8cc19234429560a3a43abae7cb9bfc75d7fd433e3c52c"
        );
    }

    #[test]
    fn history_does_not_change_the_encoding() {
        let mut edited = flat_ground();
        edited.set(5, 9, 5, blocks::STONE);
        edited.set(5, 9, 5, blocks::AIR);
        assert_eq!(encode_section(&edited), encode_section(&flat_ground()));

        // A section that became uniform encodes like one that always was.
        let mut emptied = Section::filled(blocks::AIR, PLAINS);
        emptied.set(0, 0, 0, blocks::STONE);
        emptied.set(0, 0, 0, blocks::AIR);
        assert_eq!(
            encode_section(&emptied),
            encode_section(&Section::filled(blocks::AIR, PLAINS))
        );
    }

    #[test]
    fn encodings_that_are_not_canonical_are_rejected() {
        let good = encode_section(&flat_ground());
        assert!(decode_section(&good).is_ok());

        let mut wrong_version = good.clone();
        wrong_version[0] = 2;
        assert_eq!(
            decode_section(&wrong_version),
            Err(FormatError::UnsupportedVersion(2))
        );

        // Palette entries swapped: not ascending.
        let mut unsorted = good.clone();
        unsorted.swap(6, 8);
        assert!(matches!(
            decode_section(&unsorted),
            Err(FormatError::Corrupt(_))
        ));

        assert_eq!(
            decode_section(&good[..good.len() - 1]),
            Err(FormatError::Truncated)
        );
        let mut longer = good.clone();
        longer.push(0);
        assert!(matches!(
            decode_section(&longer),
            Err(FormatError::Corrupt(_))
        ));

        // A palette entry no block uses: stone and air listed, all blocks air.
        let mut unused = vec![1, 0, 0, 0, 2, 0, 0, 0, 1];
        unused.extend_from_slice(&[0; 64 * 8]);
        assert_eq!(
            decode_section(&unused),
            Err(FormatError::Corrupt("unused palette entry"))
        );
    }

    #[test]
    fn packing_compresses_what_compresses() {
        let canonical = encode_section(&flat_ground());
        let stored = pack(&canonical);
        assert_eq!(stored[0], CODEC_ZSTD);
        assert!(
            stored.len() < canonical.len() / 10,
            "{} bytes",
            stored.len()
        );
        assert_eq!(unpack(&stored), Ok(canonical));

        // Seven bytes cannot be made smaller.
        let tiny = encode_section(&Section::filled(blocks::AIR, PLAINS));
        let stored = pack(&tiny);
        assert_eq!(stored[0], CODEC_RAW);
        assert_eq!(unpack(&stored), Ok(tiny));

        assert_eq!(unpack(&[]), Err(FormatError::Truncated));
        assert_eq!(unpack(&[9, 1, 2]), Err(FormatError::UnknownCodec(9)));
        assert!(unpack(&[CODEC_ZSTD, 1, 2, 3]).is_err());
    }

    fn section_strategy() -> impl Strategy<Value = Section> {
        // A few distinct states, or many, scattered over a section.
        (
            1..300u16,
            prop::collection::vec((0usize..4096, any::<u16>()), 0..600),
            any::<u16>(),
        )
            .prop_map(|(distinct, changes, biome)| {
                let mut section = Section::filled(BlockState(0), Biome(biome));
                for (index, state) in changes {
                    let (x, y, z) = (index & 15, index >> 8, index >> 4 & 15);
                    section.set(x, y, z, BlockState(state % distinct));
                }
                section
            })
    }

    proptest! {
        #[test]
        fn sections_round_trip(section in section_strategy()) {
            let canonical = encode_section(&section);
            let decoded = decode_section(&canonical);
            prop_assert_eq!(decoded.as_ref(), Ok(&section));
            prop_assert_eq!(unpack(&pack(&canonical)), Ok(canonical.clone()));
            // Decoding and encoding again gives the very same bytes.
            prop_assert_eq!(encode_section(&decode_section(&canonical).unwrap()), canonical);
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = decode_section(&bytes);
            let _ = unpack(&bytes);
        }
    }
}
