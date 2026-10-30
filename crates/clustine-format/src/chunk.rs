//! How a chunk is stored: one file, checksummed, written whole or not at all
//! (`docs/adr/0022-what-a-chunk-carries.md`, section 3).
//!
//! All integers are big-endian.
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8, 18 |
//! | Kind | u8, 5 |
//! | Chunk x, z | i32, i32 |
//! | Y of the lowest block | i32 |
//! | Tick at which the chunk was saved | u64 |
//! | Epoch of the region that saved it | u64 |
//! | Generated: the version of the generator the chunk is exactly as made by, or 0 | u16 |
//! | Flags | u8: bit 0 "with a failure", bit 1 "with seams", bit 2 "with block events", bit 3 "with structures", bit 4 "with claims", bit 5 "with raids"; the others 0 |
//! | Section count | u16 |
//! | Codec of the body | u8: 0 as it is, 1 zstd |
//! | Length of the body, as it is | u32, at most 8 MiB |
//! | Length of the body, as stored | u32 |
//! | The body, as stored | bytes |
//! | CRC-32 of everything before | u32 |
//!
//! The body, before compression:
//!
//! | Part | Encoding |
//! |---|---|
//! | Per section, bottom to top: blocks | palette length `n`, u16, 1 to 4,096; `n` × u16 state ids; if `n` > 1, 4,096 indices of `ceil(log2(n))` bits in u64 words, lowest bits first, none across two words, in the order `y << 8 \| z << 4 \| x` |
//! | … biomes | palette length `m`, u8, 1 to 64; `m` × u16 biome ids; if `m` > 1, 64 indices of `ceil(log2(m))` bits, packed the same way, in the order `y << 4 \| z << 2 \| x` |
//! | Block entities | count u32; then, by y, z, x: `x << 4 \| z` u8, y i16, type u16, length of the data u32 (at most 16 KiB), the data |
//! | Light | u8: 0 for none, 1 for present without `beside`, 2 for present with it. If present: `made_by` u16; for 2, `beside` u16; then for sky and then for block, per light section bottom to top: u8 0 absent, 1 dark, 2 full, or 3 followed by 2,048 bytes |
//! | Scheduled ticks | count u32; then, in order: kind u8 (0 block, 1 fluid), `x << 4 \| z` u8, y i16, id u16, delay i32, priority i8 |
//! | Marks | count u32; then, ascending: `x << 4 \| z` u8, y i16 |
//! | Seams, with bit 1 of the flags only | as `seams.rs` says, which is not empty |
//! | Block events, with bit 2 of the flags only | as `kept_events.rs` says, which is not empty |
//! | Structures, with bit 3 of the flags only | count u32, not 0; then, in generation's order: the structure's name, its length u8 and its bytes; the chunk it starts in, x i32 and z i32; its box; the count of its pieces' boxes u32 and the boxes |
//! | Claims, with bit 4 of the flags only | count u16, not 0; then, ascending by position: `x << 4 \| z` u8, y i16, the point of interest's type u8 (its registry id), its free tickets u8 |
//! | Raids, with bit 5 of the flags only | as `raids.rs` says: a count u32, not 0, and the raids ascending by id |
//!
//! A box is its lowest corner and its highest, each x, y, z as i32: six i32s, the lowest
//! no higher than the highest on any axis. A structure's name is one of those a chunk
//! keeps ([`clustine_world::KeptKind`]); a file with another is refused.
//!
//! A chunk's entities other than players are not in this file but in one of their own
//! beside it (`entities.rs`), so that an entity's step does not rewrite the blocks.
//!
//! The writer writes one form: palettes ascending with nothing unused, lists in
//! order, light that is all 0 or all 15 as dark and full. The reader refuses what it
//! cannot make sense of and nothing else: a palette that is not sorted or has an
//! entry no position uses is read. A file has a tick and an epoch, so no two files
//! are equal anyway, and a strict reader would call a chunk unreadable that has all
//! its blocks.

use clustine_data::{BLOCK_ENTITY_TYPES, BLOCK_STATE_COUNT, BlockState, synced_registry};
use clustine_world::poi::{Claim, PoiType};
use clustine_world::{
    BIOME_CELLS, BLOCKS_PER_SECTION, Biome, BlockPos, Chunk, ChunkPos, InChunk, KeptBox, KeptKind,
    KeptStructure, LIGHT_ARRAY_LENGTH, Light, MAX_BLOCK_ENTITY_DATA, ScheduledTick, Section,
    SectionLight, Ticked,
};

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

/// The kind of a chunk file, beside those of `region.rs`.
pub const KIND_CHUNK: u8 = 5;

const CODEC_RAW: u8 = 0;
const CODEC_ZSTD: u8 = 1;
const ZSTD_LEVEL: i32 = 3;
/// The most a body may be before compression. The largest chunk a server makes is far
/// below it; a file that claims more is not read into memory.
const MAX_BODY: usize = 8 * 1024 * 1024;
const WITH_FAILURE: u8 = 1;
/// A chunk with seam entries says so, so that one without, and every chunk from before
/// there were any, is written as it was (`docs/adr/0024-block-behaviour-in-a-region.md`,
/// section 4, "The format").
const WITH_SEAMS: u8 = 2;
/// A chunk with block events kept says so, so that one without, and every chunk from
/// before there were any, is written as it was (`docs/adr/0026-redstone.md`, section 4).
const WITH_EVENTS: u8 = 4;
/// A chunk with structure starts kept says so, so that one without, as every chunk of a
/// flat world, is written as before them but for the version
/// (`docs/groundwork/n9-plan.md`, section 9.6).
const WITH_STRUCTURES: u8 = 8;
/// A chunk with claims on its points of interest says so, so that one without, as
/// nearly every chunk, is written as before them but for the version
/// (`docs/groundwork/n10-plan.md`, section 3.2).
const WITH_CLAIMS: u8 = 16;
/// A chunk that keeps raids says so (section 9.1 of the same plan).
const WITH_RAIDS: u8 = 32;
/// What one box takes: two corners of three i32s.
const BOX_LENGTH: usize = 6 * 4;
/// The byte that says how a chunk's light is there: not at all, or with the version
/// of the generator whose chunks it was made beside. The value between them, 1, is
/// light without that version, which only a file from before it was kept has.
const LIGHT_NONE: u8 = 0;
const LIGHT_WITH_BESIDE: u8 = 2;

/// A chunk as it is stored, with what the file says beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkFile {
    pub position: ChunkPos,
    /// The tick at which the chunk was saved.
    pub tick: u64,
    /// The epoch of the region that saved it.
    pub epoch: u64,
    pub chunk: Chunk,
}

/// The number of bits of an index into a palette of `length` entries, which is more
/// than one.
fn bits_for(length: usize) -> u32 {
    usize::BITS - (length - 1).leading_zeros()
}

/// Appends `indices`, each of `bits` bits, in u64 words, lowest bits first, none
/// across two words.
fn put_packed(out: &mut Vec<u8>, bits: u32, indices: &[u16]) {
    let per_word = (u64::BITS / bits) as usize;
    for group in indices.chunks(per_word) {
        let mut word = 0u64;
        for (slot, index) in group.iter().enumerate() {
            word |= u64::from(*index) << (slot as u32 * bits);
        }
        out.extend_from_slice(&word.to_be_bytes());
    }
}

/// Reads `count` indices of `bits` bits, each below `length`.
fn packed(
    input: &mut Input,
    bits: u32,
    count: usize,
    length: usize,
) -> Result<Vec<u16>, FormatError> {
    let per_word = (u64::BITS / bits) as usize;
    let mask = (1u64 << bits) - 1;
    let mut indices = Vec::with_capacity(count);
    while indices.len() < count {
        let word = input.u64()?;
        for slot in 0..per_word.min(count - indices.len()) {
            let index = (word >> (slot as u32 * bits) & mask) as usize;
            if index >= length {
                return Err(FormatError::Corrupt("an index beyond its palette"));
            }
            indices.push(index as u16);
        }
    }
    Ok(indices)
}

/// Appends a palette of `values`, ascending with nothing unused, and where there is
/// more than one the index of each value.
fn put_paletted(out: &mut Vec<u8>, values: &[u16], wide_length: bool) {
    let mut palette = values.to_vec();
    palette.sort_unstable();
    palette.dedup();
    if wide_length {
        out.extend_from_slice(&(palette.len() as u16).to_be_bytes());
    } else {
        out.push(palette.len() as u8);
    }
    for value in &palette {
        out.extend_from_slice(&value.to_be_bytes());
    }
    if palette.len() > 1 {
        let index = |value: &u16| palette.binary_search(value).expect("the palette has it") as u16;
        let indices: Vec<u16> = values.iter().map(index).collect();
        put_packed(out, bits_for(palette.len()), &indices);
    }
}

/// Reads what [`put_paletted`] wrote for `count` positions: the value of each. `known`
/// says whether a value is one that exists.
fn paletted(
    input: &mut Input,
    count: usize,
    wide_length: bool,
    known: impl Fn(u16) -> bool,
) -> Result<Vec<u16>, FormatError> {
    let length = if wide_length {
        usize::from(input.u16()?)
    } else {
        usize::from(input.u8()?)
    };
    if length == 0 || length > count {
        return Err(FormatError::Corrupt("palette length"));
    }
    let palette = (0..length)
        .map(|_| input.u16())
        .collect::<Result<Vec<_>, _>>()?;
    if !palette.iter().all(|value| known(*value)) {
        return Err(FormatError::Corrupt("a palette names what does not exist"));
    }
    if length == 1 {
        return Ok(vec![palette[0]; count]);
    }
    let indices = packed(input, bits_for(length), count, length)?;
    Ok(indices
        .into_iter()
        .map(|index| palette[usize::from(index)])
        .collect())
}

fn put_in_chunk(out: &mut Vec<u8>, at: InChunk) {
    out.push(at.x << 4 | at.z);
    out.extend_from_slice(&at.y.to_be_bytes());
}

fn in_chunk(input: &mut Input) -> Result<InChunk, FormatError> {
    let column = input.u8()?;
    let y = input.u16()? as i16;
    Ok(InChunk {
        y,
        z: column & 15,
        x: column >> 4,
    })
}

fn put_box(out: &mut Vec<u8>, kept: &KeptBox) {
    for corner in [kept.min, kept.max] {
        out.extend_from_slice(&corner.x.to_be_bytes());
        out.extend_from_slice(&corner.y.to_be_bytes());
        out.extend_from_slice(&corner.z.to_be_bytes());
    }
}

fn kept_box(input: &mut Input) -> Result<KeptBox, FormatError> {
    let min = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
    let max = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
    if min.x > max.x || min.y > max.y || min.z > max.z {
        return Err(FormatError::Corrupt("a box whose corners are crossed"));
    }
    Ok(KeptBox { min, max })
}

fn put_structures(out: &mut Vec<u8>, structures: &[KeptStructure]) {
    out.extend_from_slice(&(structures.len() as u32).to_be_bytes());
    for structure in structures {
        let name = structure.kind.name().as_bytes();
        out.push(u8::try_from(name.len()).expect("a structure's name is short"));
        out.extend_from_slice(name);
        out.extend_from_slice(&structure.source.x.to_be_bytes());
        out.extend_from_slice(&structure.source.z.to_be_bytes());
        put_box(out, &structure.bounds);
        out.extend_from_slice(&(structure.pieces.len() as u32).to_be_bytes());
        for piece in &structure.pieces {
            put_box(out, piece);
        }
    }
}

/// Writes `claims`, which are not empty, as the table says.
pub(crate) fn put_claims(out: &mut Vec<u8>, claims: &[Claim]) {
    out.extend_from_slice(
        &u16::try_from(claims.len())
            .expect("a chunk has fewer points of interest than 65,536")
            .to_be_bytes(),
    );
    for claim in claims {
        put_in_chunk(out, claim.at);
        out.push(claim.kind.id());
        out.push(claim.free);
    }
}

/// Reads what [`put_claims`] wrote. Whether each claim fits the chunk's blocks is the
/// chunk's to judge ([`Chunk::set_claims`]), not the file's.
pub(crate) fn take_claims(input: &mut Input) -> Result<Vec<Claim>, FormatError> {
    let count = usize::from(input.u16()?);
    let mut claims = Vec::with_capacity(count.min(input.0.len() / 5));
    for _ in 0..count {
        let at = in_chunk(input)?;
        if at.x > 15 || at.z > 15 {
            return Err(FormatError::Corrupt("a claim outside its chunk"));
        }
        let kind = PoiType::from_id(input.u8()?)
            .ok_or(FormatError::Corrupt("a point of interest of no type"))?;
        let free = input.u8()?;
        claims.push(Claim { at, kind, free });
    }
    if !claims.windows(2).all(|pair| pair[0].at < pair[1].at) {
        return Err(FormatError::Corrupt(
            "a chunk's claims are not ascending, each once",
        ));
    }
    Ok(claims)
}

fn structures(input: &mut Input) -> Result<Vec<KeptStructure>, FormatError> {
    let count = input.u32()? as usize;
    if count == 0 {
        return Err(FormatError::Corrupt(
            "a chunk with structures that has none",
        ));
    }
    // A forged count must not reserve more than the input can hold.
    let mut structures = Vec::with_capacity(count.min(input.0.len() / BOX_LENGTH));
    for _ in 0..count {
        let length = usize::from(input.u8()?);
        let name = std::str::from_utf8(input.take(length)?)
            .map_err(|_| FormatError::Corrupt("a structure's name"))?;
        let kind = KeptKind::from_name(name)
            .ok_or(FormatError::Corrupt("a structure a chunk does not keep"))?;
        let source = ChunkPos::new(input.i32()?, input.i32()?);
        let bounds = kept_box(input)?;
        let pieces = input.u32()? as usize;
        let mut boxes = Vec::with_capacity(pieces.min(input.0.len() / BOX_LENGTH));
        for _ in 0..pieces {
            boxes.push(kept_box(input)?);
        }
        structures.push(KeptStructure {
            kind,
            source,
            bounds,
            pieces: boxes,
        });
    }
    Ok(structures)
}

fn put_light(out: &mut Vec<u8>, light: &[SectionLight]) {
    for section in light {
        match section {
            SectionLight::Absent => out.push(0),
            SectionLight::Dark => out.push(1),
            SectionLight::Full => out.push(2),
            SectionLight::Levels(levels) => {
                // One form for equal light, whoever made the value.
                match SectionLight::of(levels.clone()) {
                    SectionLight::Dark => out.push(1),
                    SectionLight::Full => out.push(2),
                    _ => {
                        out.push(3);
                        out.extend_from_slice(&levels[..]);
                    }
                }
            }
        }
    }
}

fn light(input: &mut Input, entries: usize) -> Result<Vec<SectionLight>, FormatError> {
    let mut light = Vec::with_capacity(entries);
    for _ in 0..entries {
        light.push(match input.u8()? {
            0 => SectionLight::Absent,
            1 => SectionLight::Dark,
            2 => SectionLight::Full,
            3 => {
                let levels: [u8; LIGHT_ARRAY_LENGTH] = input
                    .take(LIGHT_ARRAY_LENGTH)?
                    .try_into()
                    .expect("as many bytes as were taken");
                SectionLight::of(Box::new(levels))
            }
            _ => return Err(FormatError::Corrupt("a light section's form")),
        });
    }
    Ok(light)
}

impl ChunkFile {
    /// The bytes of the file.
    pub fn encode(&self) -> Vec<u8> {
        let chunk = &self.chunk;
        let mut body = Vec::new();
        for section in chunk.sections() {
            let states: Vec<u16> = section.states().map(|state| state.0).collect();
            put_paletted(&mut body, &states, true);
            let biomes: Vec<u16> = section.biomes().map(|biome| biome.0).collect();
            put_paletted(&mut body, &biomes, false);
        }
        body.extend_from_slice(&(chunk.block_entities().len() as u32).to_be_bytes());
        for (at, entity) in chunk.block_entities() {
            put_in_chunk(&mut body, *at);
            body.extend_from_slice(&u16::from(entity.kind).to_be_bytes());
            body.extend_from_slice(&(entity.data.len() as u32).to_be_bytes());
            body.extend_from_slice(&entity.data);
        }
        match chunk.light() {
            None => body.push(LIGHT_NONE),
            // With what the light was made beside. A file from before that was kept
            // says 1 here and has no such number; none is written so any more.
            Some(light) => {
                body.push(LIGHT_WITH_BESIDE);
                body.extend_from_slice(&light.made_by.to_be_bytes());
                body.extend_from_slice(&light.beside.to_be_bytes());
                put_light(&mut body, &light.sky);
                put_light(&mut body, &light.block);
            }
        }
        body.extend_from_slice(&(chunk.ticks().len() as u32).to_be_bytes());
        for tick in chunk.ticks() {
            let (kind, id) = match tick.of {
                Ticked::Block(id) => (0, id),
                Ticked::Fluid(id) => (1, u16::from(id)),
            };
            body.push(kind);
            put_in_chunk(&mut body, tick.at);
            body.extend_from_slice(&id.to_be_bytes());
            body.extend_from_slice(&tick.delay.to_be_bytes());
            body.extend_from_slice(&tick.priority.to_be_bytes());
        }
        body.extend_from_slice(&(chunk.marks().len() as u32).to_be_bytes());
        for mark in chunk.marks() {
            put_in_chunk(&mut body, *mark);
        }
        if !chunk.seams().is_empty() {
            crate::seams::put_seams(&mut body, chunk.seams());
        }
        if !chunk.events().is_empty() {
            crate::kept_events::put_events(&mut body, chunk.events());
        }
        if !chunk.structures().is_empty() {
            put_structures(&mut body, chunk.structures());
        }
        if !chunk.claims().is_empty() {
            put_claims(&mut body, chunk.claims());
        }
        if !chunk.raids().is_empty() {
            crate::raids::put_raids(&mut body, chunk.raids());
        }
        assert!(body.len() <= MAX_BODY, "a chunk of {} bytes", body.len());

        let compressed = zstd::bulk::compress(&body, ZSTD_LEVEL).ok();
        let (codec, stored) = match &compressed {
            Some(compressed) if compressed.len() < body.len() => (CODEC_ZSTD, &compressed[..]),
            _ => (CODEC_RAW, &body[..]),
        };
        let mut out = vec![FORMAT_VERSION, KIND_CHUNK];
        out.extend_from_slice(&self.position.x.to_be_bytes());
        out.extend_from_slice(&self.position.z.to_be_bytes());
        out.extend_from_slice(&chunk.min_y().to_be_bytes());
        out.extend_from_slice(&self.tick.to_be_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&chunk.generated().to_be_bytes());
        let failure = if chunk.with_failure() {
            WITH_FAILURE
        } else {
            0
        };
        let seams = if chunk.seams().is_empty() {
            0
        } else {
            WITH_SEAMS
        };
        let events = if chunk.events().is_empty() {
            0
        } else {
            WITH_EVENTS
        };
        let structures = if chunk.structures().is_empty() {
            0
        } else {
            WITH_STRUCTURES
        };
        let claims = if chunk.claims().is_empty() {
            0
        } else {
            WITH_CLAIMS
        };
        let raids = if chunk.raids().is_empty() {
            0
        } else {
            WITH_RAIDS
        };
        out.push(failure | seams | events | structures | claims | raids);
        out.extend_from_slice(&(chunk.sections().len() as u16).to_be_bytes());
        out.push(codec);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&(stored.len() as u32).to_be_bytes());
        out.extend_from_slice(stored);
        let checksum = crc32fast::hash(&out);
        out.extend_from_slice(&checksum.to_be_bytes());
        out
    }

    /// Reads a file. Returns the chunk and how many block entities and claims were not
    /// as the blocks ask: a block entity that was missing is there without data, and one
    /// where the blocks ask for none, or for another type, is dropped, as is a claim on
    /// a block of another type. Whoever reads says so of the chunk.
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), FormatError> {
        let (front, checksum) = bytes
            .split_last_chunk::<4>()
            .ok_or(FormatError::Truncated)?;
        if crc32fast::hash(front) != u32::from_be_bytes(*checksum) {
            return Err(FormatError::ChecksumMismatch);
        }
        let mut input = Input(front);
        let version = input.u8()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        if input.u8()? != KIND_CHUNK {
            return Err(FormatError::Corrupt("not a chunk file"));
        }
        let position = ChunkPos::new(input.i32()?, input.i32()?);
        let min_y = input.i32()?;
        let tick = input.u64()?;
        let epoch = input.u64()?;
        let generated = input.u16()?;
        let flags = input.u8()?;
        if flags
            & !(WITH_FAILURE
                | WITH_SEAMS
                | WITH_EVENTS
                | WITH_STRUCTURES
                | WITH_CLAIMS
                | WITH_RAIDS)
            != 0
        {
            return Err(FormatError::Corrupt("flags that mean nothing"));
        }
        let section_count = usize::from(input.u16()?);
        let codec = input.u8()?;
        let length = input.u32()? as usize;
        let stored = input.u32()? as usize;
        if length > MAX_BODY {
            return Err(FormatError::Corrupt("a body beyond its bound"));
        }
        let stored = input.take(stored)?;
        input.finish()?;
        let body = match codec {
            CODEC_RAW => stored.to_vec(),
            CODEC_ZSTD => zstd::bulk::decompress(stored, length)
                .map_err(|_| FormatError::Corrupt("compressed data"))?,
            other => return Err(FormatError::UnknownCodec(other)),
        };
        if body.len() != length {
            return Err(FormatError::Corrupt("the body's length"));
        }

        let biome_count =
            synced_registry("minecraft:worldgen/biome").map_or(0, |biomes| biomes.entries.len());
        let mut input = Input(&body);
        let mut sections = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            let known = |state: u16| u32::from(state) < BLOCK_STATE_COUNT;
            let states = paletted(&mut input, BLOCKS_PER_SECTION, true, known)?;
            let states: Box<[BlockState; BLOCKS_PER_SECTION]> = states
                .into_iter()
                .map(BlockState)
                .collect::<Box<[_]>>()
                .try_into()
                .expect("one section of blocks");
            let known = |biome: u16| usize::from(biome) < biome_count;
            let biomes = paletted(&mut input, BIOME_CELLS, false, known)?;
            let biomes: Box<[Biome; BIOME_CELLS]> = biomes
                .into_iter()
                .map(Biome)
                .collect::<Box<[_]>>()
                .try_into()
                .expect("one section of biomes");
            let mut section = Section::from_states(states, biomes[0]);
            section.set_biomes(biomes);
            sections.push(section);
        }
        let mut chunk = Chunk::from_sections(min_y, sections);
        // The chunk has a block entity without data wherever its blocks keep one. The
        // file's are laid over those: where one fits, its data is taken.
        let mut unfit = 0;
        let mut fitted = 0;
        for _ in 0..input.u32()? {
            let at = in_chunk(&mut input)?;
            let kind = input.u16()?;
            if usize::from(kind) >= BLOCK_ENTITY_TYPES.len() {
                return Err(FormatError::Corrupt(
                    "a block entity type that does not exist",
                ));
            }
            let length = input.u32()? as usize;
            if length > MAX_BLOCK_ENTITY_DATA {
                return Err(FormatError::Corrupt(
                    "a block entity's data beyond its bound",
                ));
            }
            let data = input.take(length)?.to_vec();
            let fits = chunk
                .block_entities()
                .get(&at)
                .is_some_and(|entity| u16::from(entity.kind) == kind);
            if !fits {
                unfit += 1;
                continue;
            }
            fitted += 1;
            if chunk.set_block_entity_data(at, data).is_err() {
                return Err(FormatError::Corrupt(
                    "block entity data beyond the chunk's bound",
                ));
            }
        }
        let missing = chunk.block_entities().len() - fitted;
        let lit = input.u8()?;
        if lit > LIGHT_WITH_BESIDE {
            return Err(FormatError::Corrupt("how a chunk's light is there"));
        }
        if lit != LIGHT_NONE {
            let made_by = input.u16()?;
            // Light written before it said what it was made beside did not say.
            let beside = match lit {
                LIGHT_WITH_BESIDE => input.u16()?,
                _ => 0,
            };
            let sky = light(&mut input, section_count + 2)?;
            let block = light(&mut input, section_count + 2)?;
            chunk.set_light(Light {
                made_by,
                beside,
                sky,
                block,
            });
        }
        let mut ticks = Vec::new();
        for _ in 0..input.u32()? {
            let kind = input.u8()?;
            let at = in_chunk(&mut input)?;
            let id = input.u16()?;
            let of = match kind {
                0 => Ticked::Block(id),
                1 => Ticked::Fluid(
                    u8::try_from(id).map_err(|_| FormatError::Corrupt("a fluid's id"))?,
                ),
                _ => return Err(FormatError::Corrupt("what a tick is of")),
            };
            let delay = input.i32()?;
            let priority = input.u8()? as i8;
            ticks.push(ScheduledTick {
                at,
                of,
                delay,
                priority,
            });
        }
        chunk.set_ticks(ticks);
        let mut marks = Vec::new();
        for _ in 0..input.u32()? {
            marks.push(in_chunk(&mut input)?);
        }
        chunk.set_marks(marks);
        if flags & WITH_SEAMS != 0 {
            let seams = crate::seams::take_seams(&mut input)?;
            if seams.is_empty() {
                return Err(FormatError::Corrupt("a chunk with seams that has none"));
            }
            chunk.set_seams(seams);
        }
        if flags & WITH_EVENTS != 0 {
            let events = crate::kept_events::take_events(&mut input)?;
            if events.is_empty() {
                return Err(FormatError::Corrupt(
                    "a chunk with block events that has none",
                ));
            }
            chunk.set_events(events);
        }
        if flags & WITH_STRUCTURES != 0 {
            chunk.set_structures(structures(&mut input)?);
        }
        let mut claims_dropped = 0;
        if flags & WITH_CLAIMS != 0 {
            let claims = take_claims(&mut input)?;
            if claims.is_empty() {
                return Err(FormatError::Corrupt("a chunk with claims that has none"));
            }
            claims_dropped = chunk.set_claims(claims);
        }
        if flags & WITH_RAIDS != 0 {
            let raids = crate::raids::take_raids(&mut input)?;
            if raids.is_empty() {
                return Err(FormatError::Corrupt("a chunk with raids that has none"));
            }
            chunk.set_raids(raids);
        }
        input.finish()?;
        chunk.mark_generated(generated, flags & WITH_FAILURE != 0);
        let file = Self {
            position,
            tick,
            epoch,
            chunk,
        };
        Ok((file, unfit + missing + claims_dropped))
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::{DIMENSION_TYPES, blocks};
    use clustine_world::{BlockPos, Seam, SeamKind};

    use super::*;

    const PLAINS: Biome = Biome(41);

    fn empty() -> Chunk {
        let overworld = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        Chunk::empty(overworld, PLAINS)
    }

    fn file(chunk: Chunk) -> ChunkFile {
        ChunkFile {
            position: ChunkPos::new(3, -4),
            tick: 77,
            epoch: 9,
            chunk,
        }
    }

    /// A chunk with something of everything a chunk carries.
    fn full() -> Chunk {
        let mut chunk = empty();
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, -64, z, blocks::BEDROCK);
                chunk.set(x, -63, z, blocks::DIRT);
            }
        }
        chunk.set(3, -62, 5, blocks::CHEST);
        chunk.set(4, -62, 5, blocks::WATER);
        chunk.set(9, 100, 9, blocks::CAVE_AIR);
        let at = InChunk::new(3, -62, 5);
        chunk.set_block_entity_data(at, vec![1, 2, 3]).unwrap();
        let entries = chunk.sections().len() + 2;
        let mut sky = vec![SectionLight::Full; entries];
        sky[0] = SectionLight::Absent;
        let mut levels = Box::new([0; LIGHT_ARRAY_LENGTH]);
        levels[7] = 0x5a;
        sky[1] = SectionLight::Levels(levels);
        chunk.set_light(Light {
            made_by: 1,
            beside: 4,
            sky,
            block: vec![SectionLight::Dark; entries],
        });
        chunk.set_ticks(vec![
            ScheduledTick {
                at: InChunk::new(4, -62, 5),
                of: Ticked::Fluid(2),
                delay: 5,
                priority: -1,
            },
            ScheduledTick {
                at: InChunk::new(1, -63, 1),
                of: Ticked::Block(600),
                delay: 0,
                priority: 3,
            },
        ]);
        chunk.set_marks(vec![InChunk::new(4, -62, 5), InChunk::new(0, -64, 0)]);
        chunk.mark_generated(7, true);
        chunk
    }

    #[test]
    fn a_chunk_comes_back_from_its_file_with_everything_it_carries() {
        for chunk in [empty(), full()] {
            let file = file(chunk);
            let bytes = file.encode();
            assert_eq!(bytes[..2], [FORMAT_VERSION, 5]);
            assert_eq!(ChunkFile::decode(&bytes), Ok((file.clone(), 0)));
            // One form: what was read is written as it was.
            assert_eq!(ChunkFile::decode(&bytes).unwrap().0.encode(), bytes);
        }
    }

    /// A chunk with block events kept is read back with them, in their order, and says
    /// so in its flags; one without has the flag clear, as every chunk from before
    /// there were any (`docs/adr/0026-redstone.md`, section 4).
    #[test]
    fn a_chunk_with_block_events_comes_back_with_them_in_order_and_one_without_is_as_before() {
        let mut chunk = full();
        let events = vec![
            clustine_world::KeptEvent {
                at: InChunk::new(15, -64, 0),
                block: 1200,
                kind: 1,
                data: -3,
            },
            clustine_world::KeptEvent {
                at: InChunk::new(0, 300, 15),
                block: 7,
                kind: 0,
                data: i32::MAX,
            },
        ];
        chunk.set_events(events.clone());
        let bytes = file(chunk).encode();
        let (read, _) = ChunkFile::decode(&bytes).unwrap();
        assert_eq!(read.chunk.events(), events);
        assert_eq!(bytes[32] & WITH_EVENTS, WITH_EVENTS);
        assert_eq!(read.encode(), bytes);
        assert_eq!(file(full()).encode()[32] & WITH_EVENTS, 0);
    }

    /// The flag of block events with none after it is refused, as seams' is.
    #[test]
    fn a_chunk_that_says_it_has_block_events_and_has_none_is_refused() {
        let mut bytes = file(empty()).encode();
        bytes[32] |= WITH_EVENTS;
        assert!(ChunkFile::decode(&bytes).is_err());
    }

    /// A chunk with seam entries is read back with them, an entry reaching beyond the
    /// chunk and below zero among them, and says so in its flags; one without has the
    /// flag clear, as every chunk from before there were seams.
    #[test]
    fn a_chunk_with_seams_comes_back_with_them_and_one_without_is_as_before() {
        let mut chunk = full();
        chunk.set_seams(vec![
            Seam {
                at: BlockPos::new(-1, -64, 5),
                direction: 5,
                kind: SeamKind::Shape,
                source: blocks::STONE,
                flags: 2,
                depth: 511,
            },
            Seam {
                at: BlockPos::new(16, 70, 0),
                direction: 4,
                kind: SeamKind::Neighbour,
                source: blocks::WATER,
                flags: 3,
                depth: -1,
            },
        ]);
        let file = file(chunk);
        let bytes = file.encode();
        assert_eq!(bytes[32] & WITH_SEAMS, WITH_SEAMS);
        assert_eq!(ChunkFile::decode(&bytes), Ok((file.clone(), 0)));
        assert_eq!(ChunkFile::decode(&bytes).unwrap().0.encode(), bytes);
        assert_eq!(file_of_full_without_seams()[32] & WITH_SEAMS, 0);
    }

    fn file_of_full_without_seams() -> Vec<u8> {
        file(full()).encode()
    }

    fn kept(min: (i32, i32, i32), max: (i32, i32, i32)) -> KeptBox {
        KeptBox {
            min: BlockPos::new(min.0, min.1, min.2),
            max: BlockPos::new(max.0, max.1, max.2),
        }
    }

    /// A swamp hut with its piece, a monument without pieces from a start far below and
    /// to the west, and a village with two, in that order.
    fn structures() -> Vec<KeptStructure> {
        vec![
            KeptStructure {
                kind: KeptKind::SwampHut,
                source: ChunkPos::new(3, -4),
                bounds: kept((50, 64, -60), (56, 70, -52)),
                pieces: vec![kept((50, 61, -60), (56, 67, -52))],
            },
            KeptStructure {
                kind: KeptKind::Monument,
                source: ChunkPos::new(-1, -9),
                bounds: kept((-29, 39, -144), (28, 61, -87)),
                pieces: Vec::new(),
            },
            KeptStructure {
                kind: KeptKind::VillageSnowy,
                source: ChunkPos::new(i32::MIN / 16, 2),
                bounds: kept((i32::MIN, -64, 0), (i32::MAX, 319, 1)),
                pieces: vec![
                    kept((48, 60, -64), (63, 75, -49)),
                    kept((0, 0, 0), (0, 0, 0)),
                ],
            },
        ]
    }

    /// A chunk with structures kept is read back with them, in their order, and says
    /// so in its flags; one without has the flag clear, and its file is as before them
    /// but for the version (`docs/groundwork/n9-plan.md`, section 9.6).
    #[test]
    fn a_chunk_with_structures_comes_back_with_them_in_order_and_one_without_is_as_before() {
        for base in [empty(), full()] {
            let mut chunk = base.clone();
            chunk.set_structures(structures());
            let with = file(chunk);
            let bytes = with.encode();
            assert_eq!(bytes[32] & WITH_STRUCTURES, WITH_STRUCTURES);
            let (read, unfit) = ChunkFile::decode(&bytes).unwrap();
            assert_eq!(unfit, 0);
            assert_eq!(read, with);
            assert_eq!(read.chunk.structures(), structures());
            assert_eq!(read.encode(), bytes);
            assert_eq!(file(base).encode()[32] & WITH_STRUCTURES, 0);
        }
    }

    /// A chunk with a lectern and a bell, a ticket taken of each.
    fn claimed(base: Chunk) -> Chunk {
        let mut chunk = base;
        chunk.set(2, 0, 2, blocks::LECTERN);
        chunk.set(9, 0, 9, blocks::BELL);
        assert!(chunk.acquire_ticket(InChunk::new(2, 0, 2)));
        assert!(chunk.acquire_ticket(InChunk::new(9, 0, 9)));
        chunk
    }

    #[test]
    fn a_chunk_with_claims_comes_back_with_them_and_one_without_is_as_before() {
        for base in [empty(), full()] {
            let chunk = claimed(base.clone());
            let with = file(chunk);
            let bytes = with.encode();
            assert_eq!(bytes[32] & WITH_CLAIMS, WITH_CLAIMS);
            let (read, unfit) = ChunkFile::decode(&bytes).unwrap();
            assert_eq!(unfit, 0);
            assert_eq!(read, with);
            assert_eq!(read.chunk.free_tickets(InChunk::new(9, 0, 9)), Some(31));
            assert_eq!(read.encode(), bytes);
            assert_eq!(file(base).encode()[32] & WITH_CLAIMS, 0);
        }
    }

    /// The claims' part is laid out as the table says: a count, then per claim the
    /// column, the height, the type's id and the free tickets.
    #[test]
    fn the_claims_part_of_the_body_is_laid_out_as_the_table_says() {
        let chunk = claimed(empty());
        let mut out = Vec::new();
        put_claims(&mut out, chunk.claims());
        assert_eq!(
            out,
            [
                0, 2, // two claims
                0x22, 0, 0, 8, 0, // the lectern: a librarian's, none free
                0x99, 0, 0, 14, 31, // the bell: a meeting point, 31 free
            ]
        );
    }

    /// A claim whose block is not of its type is dropped and counted, and the chunk is
    /// read: a mismatch can come of a change made without corruption.
    #[test]
    fn a_claim_on_a_block_of_another_type_is_dropped_and_the_chunk_kept() {
        let chunk = claimed(empty());
        let stored = file(chunk).encode();
        // The body as it is: the header is 44 bytes, its codec at 35.
        let mut body = match stored[35] {
            CODEC_ZSTD => zstd::bulk::decompress(&stored[44..stored.len() - 4], MAX_BODY).unwrap(),
            _ => stored[44..stored.len() - 4].to_vec(),
        };
        // The lectern's claim names a mason's stonecutter instead.
        let at = body
            .windows(5)
            .position(|window| window == [0x22, 0, 0, 8, 0])
            .expect("the claims are at the body's end");
        body[at + 3] = 9;
        let mut bytes = stored[..35].to_vec();
        bytes.push(CODEC_RAW);
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&body);
        let checksum = crc32fast::hash(&bytes);
        bytes.extend_from_slice(&checksum.to_be_bytes());
        let (read, unfit) = ChunkFile::decode(&bytes).unwrap();
        assert_eq!(unfit, 1);
        assert_eq!(read.chunk.free_tickets(InChunk::new(2, 0, 2)), Some(1));
        assert_eq!(read.chunk.free_tickets(InChunk::new(9, 0, 9)), Some(31));
    }

    #[test]
    fn a_chunk_with_raids_comes_back_with_them_and_one_without_is_as_before() {
        let mut chunk = full();
        chunk.set_raids(vec![crate::raids::tests::raid(4)]);
        let with = file(chunk);
        let bytes = with.encode();
        assert_eq!(bytes[32] & WITH_RAIDS, WITH_RAIDS);
        let (read, unfit) = ChunkFile::decode(&bytes).unwrap();
        assert_eq!(unfit, 0);
        assert_eq!(read, with);
        assert_eq!(file(full()).encode()[32] & WITH_RAIDS, 0);
    }

    /// The structures' part of the body is laid out as the table says.
    #[test]
    fn a_kept_structure_is_written_by_its_name_its_source_its_box_and_its_pieces() {
        let mut out = Vec::new();
        put_structures(&mut out, &structures()[..1]);
        let mut expected = vec![0, 0, 0, 1, 19];
        expected.extend_from_slice(b"minecraft:swamp_hut");
        for number in [3, -4, 50, 64, -60, 56, 70, -52] {
            expected.extend_from_slice(&i32::to_be_bytes(number));
        }
        expected.extend_from_slice(&[0, 0, 0, 1]);
        for number in [50, 61, -60, 56, 67, -52] {
            expected.extend_from_slice(&i32::to_be_bytes(number));
        }
        assert_eq!(out, expected);
    }

    /// The bytes of a file whose structures' part is `part` in place of the right one.
    fn with_structures_part(part: &[u8]) -> Vec<u8> {
        let mut chunk = empty();
        chunk.set_structures(structures()[..1].to_vec());
        let bytes = file(chunk).encode();
        let mut good = Vec::new();
        put_structures(&mut good, &structures()[..1]);
        // Taken apart as `assembled` puts a file together: the body raw, its
        // structures last.
        let (codec, length) = (
            bytes[35],
            u32::from_be_bytes(bytes[36..40].try_into().unwrap()),
        );
        let body = match codec {
            CODEC_RAW => bytes[44..bytes.len() - 4].to_vec(),
            _ => zstd::bulk::decompress(&bytes[44..bytes.len() - 4], length as usize).unwrap(),
        };
        assert!(body.ends_with(&good));
        let mut body = body[..body.len() - good.len()].to_vec();
        body.extend_from_slice(part);
        let mut out = bytes[..35].to_vec();
        out.push(CODEC_RAW);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        let checksum = crc32fast::hash(&out);
        out.extend_from_slice(&checksum.to_be_bytes());
        out
    }

    #[test]
    fn structures_that_cannot_be_made_sense_of_are_refused() {
        let mut good = Vec::new();
        put_structures(&mut good, &structures()[..1]);
        assert!(ChunkFile::decode(&with_structures_part(&good)).is_ok());
        // None after the flag.
        assert!(ChunkFile::decode(&with_structures_part(&[0, 0, 0, 0])).is_err());
        // A structure no chunk keeps, its name as long as the hut's.
        let mut igloo = good.clone();
        igloo[5..24].copy_from_slice(b"minecraft:igloo_hut");
        assert_eq!(
            ChunkFile::decode(&with_structures_part(&igloo)),
            Err(FormatError::Corrupt("a structure a chunk does not keep"))
        );
        // The start's box with its lowest y above its highest.
        let mut crossed = good.clone();
        crossed[36..40].copy_from_slice(&i32::to_be_bytes(71));
        assert_eq!(
            ChunkFile::decode(&with_structures_part(&crossed)),
            Err(FormatError::Corrupt("a box whose corners are crossed"))
        );
        // Cut short, and with something after.
        assert!(ChunkFile::decode(&with_structures_part(&good[..good.len() - 1])).is_err());
        let mut longer = good.clone();
        longer.push(0);
        assert!(ChunkFile::decode(&with_structures_part(&longer)).is_err());
    }

    /// A chunk of format 19, from before the table named each region's dimension, is
    /// refused as every earlier format is: worlds made before are not carried over.
    #[test]
    fn a_chunk_file_of_format_19_is_refused() {
        assert_eq!(FORMAT_VERSION, 20);
        let mut bytes = file(full()).encode();
        bytes[0] = 19;
        let (front, _) = bytes.split_at(bytes.len() - 4);
        let checksum = crc32fast::hash(front);
        let at = bytes.len() - 4;
        bytes[at..].copy_from_slice(&checksum.to_be_bytes());
        assert_eq!(
            ChunkFile::decode(&bytes),
            Err(FormatError::UnsupportedVersion(19))
        );
    }

    /// The record's two sizes: a section of one state and one biome is seven bytes of
    /// the body, and an empty chunk is a file of under 200 bytes.
    #[test]
    fn an_empty_chunk_is_a_small_file() {
        let bytes = file(empty()).encode();
        assert!(bytes.len() < 200, "{} bytes", bytes.len());
        let mut body = Vec::new();
        put_paletted(&mut body, &[0; BLOCKS_PER_SECTION], true);
        put_paletted(&mut body, &[41; BIOME_CELLS], false);
        assert_eq!(body.len(), 7);
    }

    #[test]
    fn a_file_that_was_damaged_or_cut_short_is_refused() {
        let bytes = file(full()).encode();
        for cut in 0..bytes.len() {
            assert!(ChunkFile::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        for at in (0..bytes.len()).step_by(7) {
            let mut damaged = bytes.clone();
            damaged[at] ^= 0x40;
            assert_eq!(
                ChunkFile::decode(&damaged),
                Err(FormatError::ChecksumMismatch),
                "byte {at}"
            );
        }
    }

    /// Bytes with a right checksum and `change` made to the front of a file of an
    /// empty chunk, whose body is stored as it is.
    fn changed(change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut bytes = file(empty()).encode();
        bytes.truncate(bytes.len() - 4);
        change(&mut bytes);
        let checksum = crc32fast::hash(&bytes);
        bytes.extend_from_slice(&checksum.to_be_bytes());
        bytes
    }

    #[test]
    fn what_cannot_be_made_sense_of_is_refused_though_its_checksum_is_right() {
        assert!(ChunkFile::decode(&changed(|_| {})).is_ok());
        let refused = |change: &dyn Fn(&mut Vec<u8>)| ChunkFile::decode(&changed(change)).is_err();
        // Another version, another kind, a flag that means nothing, a codec that is none.
        assert!(refused(&|bytes| bytes[0] = 1));
        assert!(refused(&|bytes| bytes[1] = 3));
        assert!(refused(&|bytes| bytes[32] = 2));
        assert!(refused(&|bytes| bytes[35] = 9));
        // More sections than the body has, and bytes behind the body.
        assert!(refused(&|bytes| bytes[34] += 1));
        assert!(refused(&|bytes| bytes.push(0)));
    }

    /// A palette that is not sorted, or has an entry no position uses, is read: the
    /// chunk has all its blocks.
    #[test]
    fn a_palette_out_of_order_or_with_an_unused_entry_is_read() {
        let mut chunk = empty();
        chunk.set(0, -64, 0, blocks::STONE);
        let mut body = Vec::new();
        // Stone, air and dirt, of which dirt is used nowhere: two bits an index.
        body.extend_from_slice(&3u16.to_be_bytes());
        for state in [blocks::STONE, blocks::AIR, blocks::DIRT] {
            body.extend_from_slice(&state.0.to_be_bytes());
        }
        let mut indices = [1u16; BLOCKS_PER_SECTION];
        indices[0] = 0;
        put_packed(&mut body, 2, &indices);
        body.extend_from_slice(&[1, 0, 41]);
        for _ in 1..24 {
            body.extend_from_slice(&[0, 1, 0, 0, 1, 0, 41]);
        }
        // No block entities, no light, no ticks, no marks.
        body.extend_from_slice(&[0; 4]);
        body.push(0);
        body.extend_from_slice(&[0; 8]);
        let bytes = changed(|bytes| {
            bytes.truncate(35);
            bytes.push(CODEC_RAW);
            bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
            bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
            bytes.extend_from_slice(&body);
        });
        let (read, unfit) = ChunkFile::decode(&bytes).unwrap();
        assert_eq!((read.chunk, unfit), (chunk, 0));
    }

    /// Light written before it said what it was made beside is read as light that
    /// does not say; a value that is no case is refused.
    #[test]
    fn light_without_what_it_was_made_beside_is_read_and_says_nothing_of_it() {
        let entries = 26;
        let mut rest = vec![0, 0, 0, 0];
        // Light of the rules' version 3: every sky section full, every block
        // section dark.
        rest.extend_from_slice(&[1, 0, 3]);
        rest.extend_from_slice(&[2; 26]);
        rest.extend_from_slice(&[1; 26]);
        rest.extend_from_slice(&[0; 8]);
        let air = [0, 1, 0, 0, 1, 0, 41];
        let (read, _) = ChunkFile::decode(&assembled(&air, &rest)).unwrap();
        let expected = Light {
            made_by: 3,
            beside: 0,
            sky: vec![SectionLight::Full; entries],
            block: vec![SectionLight::Dark; entries],
        };
        assert_eq!(read.chunk.light(), Some(&expected));
        // Written again it says what it has, which is nothing: the case with it.
        let written = read.encode();
        let (again, _) = ChunkFile::decode(&written).unwrap();
        assert_eq!(again.chunk.light(), Some(&expected));

        rest[4] = 3;
        assert!(ChunkFile::decode(&assembled(&air, &rest)).is_err());
    }

    /// A file of an overworld chunk whose lowest section is `lowest` and whose other
    /// sections are air, with `rest` behind the sections.
    fn assembled(lowest: &[u8], rest: &[u8]) -> Vec<u8> {
        let mut body = lowest.to_vec();
        for _ in 1..24 {
            body.extend_from_slice(&[0, 1, 0, 0, 1, 0, 41]);
        }
        body.extend_from_slice(rest);
        changed(|bytes| {
            bytes.truncate(35);
            bytes.push(CODEC_RAW);
            bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
            bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
            bytes.extend_from_slice(&body);
        })
    }

    /// A block entity where the blocks ask for none is dropped, and one that is
    /// missing is there without data; the reader says how many.
    #[test]
    fn block_entities_are_made_to_fit_the_blocks_on_reading() {
        // An entity of two bytes at (3, -62, 5) in a chunk of air; no light, ticks
        // or marks.
        let mut rest = vec![0, 0, 0, 1, 3 << 4 | 5];
        rest.extend_from_slice(&(-62i16).to_be_bytes());
        rest.extend_from_slice(&[0, 0, 0, 0, 0, 2, 7, 7]);
        rest.extend_from_slice(&[0; 9]);
        let air = [0, 1, 0, 0, 1, 0, 41];
        let (read, unfit) = ChunkFile::decode(&assembled(&air, &rest)).unwrap();
        assert_eq!((read.chunk, unfit), (empty(), 1));

        // A chest at (3, -62, 5) and no entity listed.
        let mut lowest = vec![0, 2];
        for state in [blocks::AIR, blocks::CHEST] {
            lowest.extend_from_slice(&state.0.to_be_bytes());
        }
        let mut indices = [0u16; BLOCKS_PER_SECTION];
        indices[2 << 8 | 5 << 4 | 3] = 1;
        put_packed(&mut lowest, 1, &indices);
        lowest.extend_from_slice(&[1, 0, 41]);
        let (read, unfit) = ChunkFile::decode(&assembled(&lowest, &[0; 13])).unwrap();
        let mut with_chest = empty();
        with_chest.set(3, -62, 5, blocks::CHEST);
        assert_eq!((read.chunk, unfit), (with_chest, 1));
    }
}
