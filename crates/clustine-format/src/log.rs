//! The write-ahead log: what regions committed and the world store has not yet folded
//! into a region's state file and the stored chunks.
//!
//! The log is a sequence of records, each framed as:
//!
//! | Field | Type |
//! |---|---|
//! | Length of the payload | u32 |
//! | CRC-32 of the payload | u32 |
//! | Payload | bytes |
//!
//! Every payload starts with the format version (u8) and the kind of record (u8). What
//! follows depends on the kind:
//!
//! | Kind | Record | Fields |
//! |---|---|---|
//! | 1 | [`LogRecord::Changes`], written before regions had a state | tick u64, epoch u64, changes |
//! | 2 | [`LogRecord::Commit`] | region u32, tick u64, epoch u64, changes, state length u32, state bytes |
//! | 3 | [`LogRecord::Opened`] | region u32, epoch u64, restored u64 |
//! | 4 | [`LogRecord::Granted`] | region u32, tick u64, chunks |
//! | 5 | [`LogRecord::Returned`] | region u32, chunks |
//! | 6 | [`LogRecord::Absorbed`] | region u32, epoch u64, absorbed u32, tick u64, state bytes |
//! | 7 | [`LogRecord::Split`] | region u32, epoch u64, tick u64, state bytes, part u32, part epoch u64, chunks, part state bytes |
//! | 8 | [`LogRecord::Commit`] with stay notes | the fields of kind 2, stays |
//! | 9 | [`LogRecord::Commit`] with records of chunks | the fields of kind 2, stays (whose count may be 0), ticks, marks taken (chunks), seams |
//! | 10 | [`LogRecord::Commit`] with effects | the fields of kind 9, whose counts may all be 0, effects, taken |
//! | 11 | [`LogRecord::EffectsDropped`] | numbers |
//! | 12 | [`LogRecord::EffectsForward`] | filed effects |
//! | 13 | [`LogRecord::Commit`] with block entities | the fields of kind 10, whose counts may all be 0, block entities |
//!
//! where `changes` is a count (u32) followed, per change, by i32 x, i32 y, i32 z and a
//! u16 block state; `chunks` is a count (u32) followed, per chunk, by i32 x and i32 z;
//! and `bytes` is a length (u32) followed by that many bytes. All integers are
//! big-endian, and so are the floating-point numbers.
//!
//! A commit is written as kind 2 if it has no stay notes and as kind 8 if it has any, so
//! that a commit and its notes are on disk together or not at all; as kind 9 if it
//! carries records of chunks: the scheduled ticks of chunks whose ticks changed in its
//! tick, the chunks whose marks for post-processing were taken in it, or the seam
//! entries of chunks whose entries changed in it
//! (`docs/adr/0024-block-behaviour-in-a-region.md`, sections 4 to 6), whatever its
//! notes. `seams` is a count of chunks (u32), followed per chunk by i32 x, i32 z and the
//! chunk's entries whole, as `seams.rs` writes them.
//! A commit is written as kind 10 if it carries effects or takes any
//! (`docs/adr/0025-effects-between-regions.md`, sections 4 and 5), whatever else it
//! carries: `effects` are those the store keeps of what the tick made, each with when it
//! is due, numbered by the record's sequence and their place in it, and `taken` the
//! numbers of the effects the tick took. A commit is written as kind 13 if any of its
//! changes is to a block entity's data (`docs/adr/0026-redstone.md`, section 6): its
//! `changes` then hold the blocks alone, and `block entities` is a count (u32)
//! followed, per change, by the number of block changes before it (u32), i32 x, i32 y,
//! i32 z and its data as `bytes`, ascending by that number, so that the two together
//! are the tick's changes in the order it made them. Kind 11 says that effects untaken were dropped,
//! and kind 12 carries untaken effects forward out of a segment that is let go, with
//! their numbers. `effects`, `numbers` and `filed effects` are as `effects.rs` writes
//! them.
//! `ticks` is a count of chunks (u32), followed per chunk by i32 x, i32 z and a count of
//! ticks (u32), and per tick by u8 x, i16 y and u8 z within the chunk, u8 1 for a block
//! and 2 for a fluid, u16 its id, i32 the delay left and i8 the priority: the chunk's
//! ticks whole, as the delays left at the end of the tick. `stays` is a count
//! (u32), which is not 0, followed, per note ([`LoggedStay`]), by:
//!
//! | Field | Type |
//! |---|---|
//! | Kind | u8: 1 `Entering`, 2 `Has` |
//! | Player | u128 |
//! | Entity | i32 |
//! | *`Has` only:* hops | u32 |
//! | … the place | see below |
//!
//! and a place ([`LoggedPlace`]) is:
//!
//! | Field | Type |
//! |---|---|
//! | Position | f64 x, f64 y, f64 z |
//! | Yaw, pitch | f32, f32 |
//! | Flags | u8: bit 0 on-ground, bit 1 flying |
//! | Selected slot | u8 |
//! | Hotbar | nine times: u8 0 for an empty slot, or u8 1, i32 item, i32 count |
//!
//! See `docs/adr/0020-one-stay-per-player.md`, section 8.
//!
//! Kinds 4 to 7 are what decides which region holds which chunk and which regions there
//! are; see `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! A process can die while appending, which leaves a partial record at the end. Reading
//! therefore stops at the first record that is incomplete or fails its checksum, and
//! reports how many bytes were valid.

use clustine_data::BlockState;
use clustine_world::{
    BlockPos, Change, ChunkPos, Due, Effect, EffectId, FiledEffect, InChunk, ScheduledTick, Seam,
    Ticked,
};

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

const KIND_CHANGES: u8 = 1;
const KIND_COMMIT: u8 = 2;
const KIND_OPENED: u8 = 3;
const KIND_GRANTED: u8 = 4;
const KIND_RETURNED: u8 = 5;
const KIND_ABSORBED: u8 = 6;
const KIND_SPLIT: u8 = 7;
const KIND_COMMIT_WITH_STAYS: u8 = 8;
const KIND_COMMIT_WITH_TICKS: u8 = 9;
const KIND_COMMIT_WITH_EFFECTS: u8 = 10;
const KIND_EFFECTS_DROPPED: u8 = 11;
const KIND_EFFECTS_FORWARD: u8 = 12;
const KIND_COMMIT_WITH_ENTITIES: u8 = 13;

/// The kinds of a stay note.
const STAY_ENTERING: u8 = 1;
const STAY_HAS: u8 = 2;

/// The flags of a place.
const ON_GROUND: u8 = 1;
const FLYING: u8 = 2;

/// Bytes a stay note takes in a payload at the least.
const STAY_LENGTH: usize = 21;

/// The number of slots in a player's hotbar.
pub const HOTBAR_SLOTS: usize = 9;

/// Bytes of the length and the checksum in front of every payload.
const FRAME_HEADER_LENGTH: usize = 8;

/// A payload longer than this is taken for damage, not for a record.
const MAX_PAYLOAD_LENGTH: usize = 64 * 1024 * 1024;

/// The longest a record may be as [`LogRecord::encode`] returns it, frame included. A
/// longer one is written like any other and then read as a record that was cut off,
/// which hides it and whatever follows it in its segment.
pub const MAX_RECORD_LENGTH: usize = FRAME_HEADER_LENGTH + MAX_PAYLOAD_LENGTH;

/// Bytes a block change takes in a payload.
const CHANGE_LENGTH: usize = 14;

/// Bytes a chunk takes in a payload.
const CHUNK_LENGTH: usize = 8;

/// What a region said of a stay in a commit, as the log keeps it: in plain numbers, so
/// that the layout is the store's own and does not change with what a region's state
/// looks like. See `docs/adr/0020-one-stay-per-player.md`, sections 3 and 8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoggedStay {
    /// The home region has given `player` the stay `entity` and holds it as entering.
    Entering { player: u128, entity: i32 },
    /// The stay is in the region after the tick, or was let go in it, with `hops`
    /// hand-overs behind it and this place.
    Has {
        player: u128,
        entity: i32,
        hops: u32,
        place: LoggedPlace,
    },
}

/// Where a player is and what they hold, in plain numbers.
///
/// Two places are equal if their bytes on disk are: the numbers with a fraction are
/// compared by their bits.
#[derive(Debug, Clone, Copy)]
pub struct LoggedPlace {
    /// The position of the feet: x, y, z.
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub flying: bool,
    pub selected_slot: u8,
    /// Each slot of the hotbar: the item and how many, if there is anything in it.
    pub hotbar: [Option<(i32, i32)>; HOTBAR_SLOTS],
}

impl PartialEq for LoggedPlace {
    fn eq(&self, other: &Self) -> bool {
        self.position.map(f64::to_bits) == other.position.map(f64::to_bits)
            && self.yaw.to_bits() == other.yaw.to_bits()
            && self.pitch.to_bits() == other.pitch.to_bits()
            && self.on_ground == other.on_ground
            && self.flying == other.flying
            && self.selected_slot == other.selected_slot
            && self.hotbar == other.hotbar
    }
}

impl Eq for LoggedPlace {}

/// A record of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogRecord {
    /// The blocks that changed during one tick of a region, as logged before regions had
    /// a state of their own: such a log was one per region, so the record does not say
    /// which region it is of. The world store only reads these.
    Changes {
        tick: u64,
        /// The epoch of the owner of the region that made the changes.
        epoch: u64,
        changes: Vec<(BlockPos, BlockState)>,
    },
    /// What one tick of a region committed: the blocks that changed, in the order they
    /// changed, and the region's own record of what else changed, which the store keeps
    /// as it is; and what the region said of stays in that tick, which the store reads.
    Commit {
        region: u32,
        tick: u64,
        /// The epoch of the owner that committed it.
        epoch: u64,
        changes: Vec<Change>,
        state: Vec<u8>,
        /// The stay notes of the tick. A commit without any is written as it was
        /// before there were notes.
        stays: Vec<LoggedStay>,
        /// The scheduled ticks of each chunk whose ticks changed in the tick, whole, as
        /// the delays left at its end, in ascending order of the chunks. A commit
        /// without any is written as it was before there were ticks.
        ticks: Vec<(ChunkPos, Vec<ScheduledTick>)>,
        /// The chunks whose marks for post-processing were taken in the tick, in
        /// ascending order.
        marks_taken: Vec<ChunkPos>,
        /// The seam entries of each chunk whose entries changed in the tick, whole, in
        /// ascending order of the chunks.
        seams: Vec<(ChunkPos, Vec<Seam>)>,
        /// The effects of the tick that the store keeps, each with when it is due, in
        /// the order the tick made them. Their numbers are the record's sequence and
        /// their index here.
        effects: Vec<(Effect, Due)>,
        /// The numbers of the effects the tick took, ascending.
        taken: Vec<EffectId>,
    },
    /// The effects of these numbers were dropped untaken: the chunk they act on stopped
    /// being used (`docs/adr/0025-effects-between-regions.md`, section 5).
    EffectsDropped { ids: Vec<EffectId> },
    /// Untaken effects written again, with their numbers, so that the segment they were
    /// first written in can be let go (the same record, section 5).
    EffectsForward { effects: Vec<FiledEffect> },
    /// The region was opened by an owner with `epoch` and restored up to tick
    /// `restored`. A record of the region that comes before this one with a later tick
    /// is not part of the region's history: the owner that opened it here did not read
    /// it, and goes on from `restored` without it.
    Opened {
        region: u32,
        epoch: u64,
        restored: u64,
    },
    /// `region` holds each of `chunks` from its tick `tick` on.
    Granted {
        region: u32,
        tick: u64,
        chunks: Vec<ChunkPos>,
    },
    /// `region` no longer holds `chunks`.
    Returned { region: u32, chunks: Vec<ChunkPos> },
    /// The merge of two regions. `state` is the whole state of `region` as of `tick`;
    /// every chunk `absorbed` was granted is granted to `region` with that tick, every
    /// area `absorbed` was pinned to is one `region` is pinned to, and `absorbed` is no
    /// region any more.
    Absorbed {
        region: u32,
        /// The epoch of the owner of `region` that asked for it.
        epoch: u64,
        absorbed: u32,
        tick: u64,
        state: Vec<u8>,
    },
    /// The split of a region. `part` is a new region with the whole state `part_state`
    /// as of `tick`; it holds `chunks` from that tick on, which `region` no longer
    /// holds. `state` is the whole state of `region` as of `tick`.
    Split {
        region: u32,
        /// The epoch of the owner of `region` that asked for it.
        epoch: u64,
        tick: u64,
        state: Vec<u8>,
        part: u32,
        /// The highest epoch `part` has been opened with, which is the one its first
        /// owner is to say hello with.
        part_epoch: u64,
        chunks: Vec<ChunkPos>,
        part_state: Vec<u8>,
    },
}

impl LogRecord {
    /// The record as it is appended to the log, frame included.
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = vec![FORMAT_VERSION];
        match self {
            Self::Changes {
                tick,
                epoch,
                changes,
            } => {
                payload.push(KIND_CHANGES);
                payload.extend_from_slice(&tick.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                put_changes(&mut payload, changes);
            }
            Self::Commit {
                region,
                tick,
                epoch,
                changes,
                state,
                stays,
                ticks,
                marks_taken,
                seams,
                effects,
                taken,
            } => {
                let (blocks, entities) = split_changes(changes);
                let with_entities = !entities.is_empty();
                let effected = with_entities || !effects.is_empty() || !taken.is_empty();
                let records =
                    effected || !ticks.is_empty() || !marks_taken.is_empty() || !seams.is_empty();
                payload.push(if with_entities {
                    KIND_COMMIT_WITH_ENTITIES
                } else if effected {
                    KIND_COMMIT_WITH_EFFECTS
                } else if records {
                    KIND_COMMIT_WITH_TICKS
                } else if stays.is_empty() {
                    KIND_COMMIT
                } else {
                    KIND_COMMIT_WITH_STAYS
                });
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                put_changes(&mut payload, &blocks);
                put_bytes(&mut payload, state);
                if records {
                    put_stays(&mut payload, stays);
                    put_ticks(&mut payload, ticks);
                    put_chunks(&mut payload, marks_taken);
                    payload.extend_from_slice(&(seams.len() as u32).to_be_bytes());
                    for (chunk, of_chunk) in seams {
                        payload.extend_from_slice(&chunk.x.to_be_bytes());
                        payload.extend_from_slice(&chunk.z.to_be_bytes());
                        crate::seams::put_seams(&mut payload, of_chunk);
                    }
                    if effected {
                        crate::effects::put_effects(&mut payload, effects);
                        crate::effects::put_ids(&mut payload, taken);
                    }
                    if with_entities {
                        put_entities(&mut payload, &entities);
                    }
                } else if !stays.is_empty() {
                    put_stays(&mut payload, stays);
                }
            }
            Self::Opened {
                region,
                epoch,
                restored,
            } => {
                payload.push(KIND_OPENED);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                payload.extend_from_slice(&restored.to_be_bytes());
            }
            Self::Granted {
                region,
                tick,
                chunks,
            } => {
                payload.push(KIND_GRANTED);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                put_chunks(&mut payload, chunks);
            }
            Self::Returned { region, chunks } => {
                payload.push(KIND_RETURNED);
                payload.extend_from_slice(&region.to_be_bytes());
                put_chunks(&mut payload, chunks);
            }
            Self::Absorbed {
                region,
                epoch,
                absorbed,
                tick,
                state,
            } => {
                payload.push(KIND_ABSORBED);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                payload.extend_from_slice(&absorbed.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                put_bytes(&mut payload, state);
            }
            Self::Split {
                region,
                epoch,
                tick,
                state,
                part,
                part_epoch,
                chunks,
                part_state,
            } => {
                payload.push(KIND_SPLIT);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                put_bytes(&mut payload, state);
                payload.extend_from_slice(&part.to_be_bytes());
                payload.extend_from_slice(&part_epoch.to_be_bytes());
                put_chunks(&mut payload, chunks);
                put_bytes(&mut payload, part_state);
            }
            Self::EffectsDropped { ids } => {
                payload.push(KIND_EFFECTS_DROPPED);
                crate::effects::put_ids(&mut payload, ids);
            }
            Self::EffectsForward { effects } => {
                payload.push(KIND_EFFECTS_FORWARD);
                crate::effects::put_filed(&mut payload, effects);
            }
        }
        frame(&payload)
    }

    fn decode(payload: &[u8]) -> Result<Self, FormatError> {
        let mut input = Input(payload);
        let version = input.u8()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let record = match input.u8()? {
            KIND_CHANGES => Self::Changes {
                tick: input.u64()?,
                epoch: input.u64()?,
                changes: take_changes(&mut input)?,
            },
            KIND_COMMIT => Self::Commit {
                region: input.u32()?,
                tick: input.u64()?,
                epoch: input.u64()?,
                changes: blocks_only(take_changes(&mut input)?),
                state: take_bytes(&mut input)?,
                stays: Vec::new(),
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
            },
            KIND_COMMIT_WITH_STAYS => Self::Commit {
                region: input.u32()?,
                tick: input.u64()?,
                epoch: input.u64()?,
                changes: blocks_only(take_changes(&mut input)?),
                state: take_bytes(&mut input)?,
                stays: take_stays(&mut input)?,
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
            },
            kind @ (KIND_COMMIT_WITH_TICKS
            | KIND_COMMIT_WITH_EFFECTS
            | KIND_COMMIT_WITH_ENTITIES) => {
                let (region, tick, epoch) = (input.u32()?, input.u64()?, input.u64()?);
                let blocks = take_changes(&mut input)?;
                let state = take_bytes(&mut input)?;
                let stays = if input.peek_u32()? == 0 {
                    input.u32()?;
                    Vec::new()
                } else {
                    take_stays(&mut input)?
                };
                let ticks = take_ticks(&mut input)?;
                let marks_taken = take_chunks(&mut input)?;
                let chunks = input.u32()? as usize;
                // Each is eight bytes and a count at least.
                let mut seams = Vec::with_capacity(chunks.min(input.0.len() / 12));
                for _ in 0..chunks {
                    let chunk = ChunkPos::new(input.i32()?, input.i32()?);
                    seams.push((chunk, crate::seams::take_seams(&mut input)?));
                }
                let (effects, taken) = if kind == KIND_COMMIT_WITH_TICKS {
                    (Vec::new(), Vec::new())
                } else {
                    let effects = crate::effects::take_effects(&mut input)?;
                    (effects, crate::effects::take_ids(&mut input)?)
                };
                let changes = if kind == KIND_COMMIT_WITH_ENTITIES {
                    let entities = take_entities(&mut input)?;
                    if entities.is_empty() {
                        return Err(FormatError::Corrupt(
                            "a commit with block entities that has none",
                        ));
                    }
                    join_changes(blocks, entities)?
                } else {
                    blocks_only(blocks)
                };
                // A commit is written in one form only: without effects as kind 9, and
                // without records of chunks as kind 2 or 8.
                if kind == KIND_COMMIT_WITH_EFFECTS && effects.is_empty() && taken.is_empty() {
                    return Err(FormatError::Corrupt("a commit with effects that has none"));
                }
                if kind == KIND_COMMIT_WITH_TICKS
                    && ticks.is_empty()
                    && marks_taken.is_empty()
                    && seams.is_empty()
                {
                    return Err(FormatError::Corrupt(
                        "a commit with records of chunks that has none",
                    ));
                }
                Self::Commit {
                    region,
                    tick,
                    epoch,
                    changes,
                    state,
                    stays,
                    ticks,
                    marks_taken,
                    seams,
                    effects,
                    taken,
                }
            }
            KIND_OPENED => Self::Opened {
                region: input.u32()?,
                epoch: input.u64()?,
                restored: input.u64()?,
            },
            KIND_GRANTED => Self::Granted {
                region: input.u32()?,
                tick: input.u64()?,
                chunks: take_chunks(&mut input)?,
            },
            KIND_RETURNED => Self::Returned {
                region: input.u32()?,
                chunks: take_chunks(&mut input)?,
            },
            KIND_ABSORBED => Self::Absorbed {
                region: input.u32()?,
                epoch: input.u64()?,
                absorbed: input.u32()?,
                tick: input.u64()?,
                state: take_bytes(&mut input)?,
            },
            KIND_SPLIT => Self::Split {
                region: input.u32()?,
                epoch: input.u64()?,
                tick: input.u64()?,
                state: take_bytes(&mut input)?,
                part: input.u32()?,
                part_epoch: input.u64()?,
                chunks: take_chunks(&mut input)?,
                part_state: take_bytes(&mut input)?,
            },
            KIND_EFFECTS_DROPPED => Self::EffectsDropped {
                ids: crate::effects::take_ids(&mut input)?,
            },
            KIND_EFFECTS_FORWARD => Self::EffectsForward {
                effects: crate::effects::take_filed(&mut input)?,
            },
            _ => return Err(FormatError::Corrupt("record kind")),
        };
        input.finish()?;
        Ok(record)
    }
}

/// Frames `payload` as a record of the log.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(FRAME_HEADER_LENGTH + payload.len());
    record.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    record.extend_from_slice(&crc32fast::hash(payload).to_be_bytes());
    record.extend_from_slice(payload);
    record
}

fn put_changes(payload: &mut Vec<u8>, changes: &[(BlockPos, BlockState)]) {
    payload.extend_from_slice(&(changes.len() as u32).to_be_bytes());
    for (position, state) in changes {
        payload.extend_from_slice(&position.x.to_be_bytes());
        payload.extend_from_slice(&position.y.to_be_bytes());
        payload.extend_from_slice(&position.z.to_be_bytes());
        payload.extend_from_slice(&state.0.to_be_bytes());
    }
}

fn take_changes(input: &mut Input<'_>) -> Result<Vec<(BlockPos, BlockState)>, FormatError> {
    let count = input.u32()? as usize;
    // A forged count must not reserve more than the payload can hold.
    let mut changes = Vec::with_capacity(count.min(input.0.len() / CHANGE_LENGTH));
    for _ in 0..count {
        let position = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
        changes.push((position, BlockState(input.u16()?)));
    }
    Ok(changes)
}

/// A commit's changes as kind 13 writes them: the blocks alone, and each change of a
/// block entity with the number of block changes before it.
#[allow(clippy::type_complexity)]
fn split_changes(changes: &[Change]) -> (Vec<(BlockPos, BlockState)>, Vec<(u32, BlockPos, &[u8])>) {
    let mut blocks = Vec::with_capacity(changes.len());
    let mut entities = Vec::new();
    for change in changes {
        match change {
            Change::Block(position, state) => blocks.push((*position, *state)),
            Change::BlockEntity(position, data) => {
                entities.push((blocks.len() as u32, *position, data.as_slice()));
            }
        }
    }
    (blocks, entities)
}

/// The changes of a commit written before block entities were.
fn blocks_only(blocks: Vec<(BlockPos, BlockState)>) -> Vec<Change> {
    blocks.into_iter().map(Change::from).collect()
}

/// The changes of a commit of kind 13, in the order its tick made them.
fn join_changes(
    blocks: Vec<(BlockPos, BlockState)>,
    entities: Vec<(u32, BlockPos, Vec<u8>)>,
) -> Result<Vec<Change>, FormatError> {
    let mut changes = Vec::with_capacity(blocks.len() + entities.len());
    let mut blocks = blocks.into_iter();
    let mut before = 0;
    for (place, position, data) in entities {
        if place < before || place as usize > before as usize + blocks.len() {
            return Err(FormatError::Corrupt(
                "a block entity's place among a commit's blocks is out of order",
            ));
        }
        changes.extend(
            blocks
                .by_ref()
                .take((place - before) as usize)
                .map(Change::from),
        );
        before = place;
        changes.push(Change::BlockEntity(position, data));
    }
    changes.extend(blocks.map(Change::from));
    Ok(changes)
}

fn put_entities(payload: &mut Vec<u8>, entities: &[(u32, BlockPos, &[u8])]) {
    payload.extend_from_slice(&(entities.len() as u32).to_be_bytes());
    for (place, position, data) in entities {
        payload.extend_from_slice(&place.to_be_bytes());
        payload.extend_from_slice(&position.x.to_be_bytes());
        payload.extend_from_slice(&position.y.to_be_bytes());
        payload.extend_from_slice(&position.z.to_be_bytes());
        put_bytes(payload, data);
    }
}

#[allow(clippy::type_complexity)]
fn take_entities(input: &mut Input<'_>) -> Result<Vec<(u32, BlockPos, Vec<u8>)>, FormatError> {
    let count = input.u32()? as usize;
    // Each is twenty bytes at least: a forged count must not reserve more.
    let mut entities = Vec::with_capacity(count.min(input.0.len() / 20));
    for _ in 0..count {
        let place = input.u32()?;
        let position = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
        entities.push((place, position, take_bytes(input)?));
    }
    Ok(entities)
}

fn put_chunks(payload: &mut Vec<u8>, chunks: &[ChunkPos]) {
    payload.extend_from_slice(&(chunks.len() as u32).to_be_bytes());
    for chunk in chunks {
        payload.extend_from_slice(&chunk.x.to_be_bytes());
        payload.extend_from_slice(&chunk.z.to_be_bytes());
    }
}

fn take_chunks(input: &mut Input<'_>) -> Result<Vec<ChunkPos>, FormatError> {
    let count = input.u32()? as usize;
    // A forged count must not reserve more than the payload can hold.
    let mut chunks = Vec::with_capacity(count.min(input.0.len() / CHUNK_LENGTH));
    for _ in 0..count {
        chunks.push(ChunkPos::new(input.i32()?, input.i32()?));
    }
    Ok(chunks)
}

fn put_bytes(payload: &mut Vec<u8>, bytes: &[u8]) {
    payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(bytes);
}

fn take_bytes(input: &mut Input<'_>) -> Result<Vec<u8>, FormatError> {
    let length = input.u32()? as usize;
    Ok(input.take(length)?.to_vec())
}

fn put_stays(payload: &mut Vec<u8>, stays: &[LoggedStay]) {
    payload.extend_from_slice(&(stays.len() as u32).to_be_bytes());
    for stay in stays {
        match stay {
            LoggedStay::Entering { player, entity } => {
                payload.push(STAY_ENTERING);
                payload.extend_from_slice(&player.to_be_bytes());
                payload.extend_from_slice(&entity.to_be_bytes());
            }
            LoggedStay::Has {
                player,
                entity,
                hops,
                place,
            } => {
                payload.push(STAY_HAS);
                payload.extend_from_slice(&player.to_be_bytes());
                payload.extend_from_slice(&entity.to_be_bytes());
                payload.extend_from_slice(&hops.to_be_bytes());
                put_place(payload, place);
            }
        }
    }
}

/// The length of one tick in a commit's ticks.
const TICK_LENGTH: usize = 1 + 2 + 1 + 1 + 2 + 4 + 1;

fn put_ticks(payload: &mut Vec<u8>, ticks: &[(ChunkPos, Vec<ScheduledTick>)]) {
    payload.extend_from_slice(&(ticks.len() as u32).to_be_bytes());
    for (chunk, of_chunk) in ticks {
        payload.extend_from_slice(&chunk.x.to_be_bytes());
        payload.extend_from_slice(&chunk.z.to_be_bytes());
        payload.extend_from_slice(&(of_chunk.len() as u32).to_be_bytes());
        for tick in of_chunk {
            payload.push(tick.at.x);
            payload.extend_from_slice(&tick.at.y.to_be_bytes());
            payload.push(tick.at.z);
            let (kind, id) = match tick.of {
                Ticked::Block(block) => (1_u8, block),
                Ticked::Fluid(fluid) => (2, u16::from(fluid)),
            };
            payload.push(kind);
            payload.extend_from_slice(&id.to_be_bytes());
            payload.extend_from_slice(&tick.delay.to_be_bytes());
            payload.extend_from_slice(&tick.priority.to_be_bytes());
        }
    }
}

fn take_ticks(input: &mut Input<'_>) -> Result<Vec<(ChunkPos, Vec<ScheduledTick>)>, FormatError> {
    let chunks = input.u32()? as usize;
    let mut ticks = Vec::with_capacity(chunks.min(input.0.len() / 12));
    for _ in 0..chunks {
        let chunk = ChunkPos::new(input.i32()?, input.i32()?);
        let count = input.u32()? as usize;
        // A forged count must not reserve more than the payload can hold.
        let mut of_chunk = Vec::with_capacity(count.min(input.0.len() / TICK_LENGTH));
        for _ in 0..count {
            let (x, y, z) = (input.u8()?, input.i16()?, input.u8()?);
            if x > 15 || z > 15 {
                return Err(FormatError::Corrupt("a tick outside its chunk"));
            }
            let kind = input.u8()?;
            let id = input.u16()?;
            let of = match kind {
                1 => Ticked::Block(id),
                2 => Ticked::Fluid(u8::try_from(id).map_err(|_| FormatError::Corrupt("fluid"))?),
                _ => return Err(FormatError::Corrupt("tick kind")),
            };
            of_chunk.push(ScheduledTick {
                at: InChunk { y, z, x },
                of,
                delay: input.i32()?,
                priority: input.i8()?,
            });
        }
        ticks.push((chunk, of_chunk));
    }
    Ok(ticks)
}

fn take_stays(input: &mut Input<'_>) -> Result<Vec<LoggedStay>, FormatError> {
    let count = input.u32()? as usize;
    // A commit without notes is of the other kind, so that one commit has one form.
    if count == 0 {
        return Err(FormatError::Corrupt("a commit with stays that has none"));
    }
    // A forged count must not reserve more than the payload can hold.
    let mut stays = Vec::with_capacity(count.min(input.0.len() / STAY_LENGTH));
    for _ in 0..count {
        let kind = input.u8()?;
        let (player, entity) = (input.u128()?, input.i32()?);
        stays.push(match kind {
            STAY_ENTERING => LoggedStay::Entering { player, entity },
            STAY_HAS => LoggedStay::Has {
                player,
                entity,
                hops: input.u32()?,
                place: take_place(input)?,
            },
            _ => return Err(FormatError::Corrupt("stay kind")),
        });
    }
    Ok(stays)
}

/// Appends a place as the log and the players' file have it.
pub(crate) fn put_place(out: &mut Vec<u8>, place: &LoggedPlace) {
    for coordinate in place.position {
        out.extend_from_slice(&coordinate.to_be_bytes());
    }
    out.extend_from_slice(&place.yaw.to_be_bytes());
    out.extend_from_slice(&place.pitch.to_be_bytes());
    let on_ground = if place.on_ground { ON_GROUND } else { 0 };
    let flying = if place.flying { FLYING } else { 0 };
    out.push(on_ground | flying);
    out.push(place.selected_slot);
    for slot in place.hotbar {
        match slot {
            None => out.push(0),
            Some((item, count)) => {
                out.push(1);
                out.extend_from_slice(&item.to_be_bytes());
                out.extend_from_slice(&count.to_be_bytes());
            }
        }
    }
}

/// Reads what [`put_place`] wrote.
pub(crate) fn take_place(input: &mut Input<'_>) -> Result<LoggedPlace, FormatError> {
    let position = [input.f64()?, input.f64()?, input.f64()?];
    let (yaw, pitch) = (input.f32()?, input.f32()?);
    let flags = input.u8()?;
    if flags & !(ON_GROUND | FLYING) != 0 {
        return Err(FormatError::Corrupt("place flags"));
    }
    let selected_slot = input.u8()?;
    let mut hotbar = [None; HOTBAR_SLOTS];
    for slot in &mut hotbar {
        *slot = match input.u8()? {
            0 => None,
            1 => Some((input.i32()?, input.i32()?)),
            _ => return Err(FormatError::Corrupt("hotbar slot")),
        };
    }
    Ok(LoggedPlace {
        position,
        yaw,
        pitch,
        on_ground: flags & ON_GROUND != 0,
        flying: flags & FLYING != 0,
        selected_slot,
        hotbar,
    })
}

/// A record read from a log, with where it is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    /// The offset of the record's frame in the log.
    pub offset: usize,
    /// The length of the frame, header included.
    pub length: usize,
    pub record: LogRecord,
}

/// Reads the records at the start of `log`. Returns them with the number of bytes they
/// occupy; anything after that is a record that was not written completely.
///
/// A record that is complete and passes its checksum but cannot be understood is an
/// error: that is not what dying in the middle of a write looks like.
pub fn read_log(log: &[u8]) -> Result<(Vec<LogRecord>, usize), FormatError> {
    let (logged, valid) = read_log_with_offsets(log)?;
    Ok((
        logged.into_iter().map(|logged| logged.record).collect(),
        valid,
    ))
}

/// Does what [`read_log`] does, and says for each record where it is.
pub fn read_log_with_offsets(log: &[u8]) -> Result<(Vec<Logged>, usize), FormatError> {
    let mut records = Vec::new();
    let mut valid = 0;
    loop {
        let rest = &log[valid..];
        let Some((header, body)) = rest.split_at_checked(FRAME_HEADER_LENGTH) else {
            break;
        };
        let length = u32::from_be_bytes(header[..4].try_into().expect("four bytes")) as usize;
        let checksum = u32::from_be_bytes(header[4..].try_into().expect("four bytes"));
        if length > MAX_PAYLOAD_LENGTH {
            break;
        }
        let Some(payload) = body.get(..length) else {
            break;
        };
        if crc32fast::hash(payload) != checksum {
            break;
        }
        records.push(Logged {
            offset: valid,
            length: FRAME_HEADER_LENGTH + length,
            record: LogRecord::decode(payload)?,
        });
        valid += FRAME_HEADER_LENGTH + length;
    }
    Ok((records, valid))
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use proptest::prelude::*;

    use super::*;

    fn record(tick: u64, count: i32) -> LogRecord {
        LogRecord::Commit {
            region: 3,
            tick,
            epoch: 1,
            changes: (0..count)
                .map(|index| (BlockPos::new(index, -61, -index), blocks::STONE).into())
                .collect(),
            state: vec![tick as u8; count as usize],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        }
    }

    #[test]
    fn known_answer() {
        let record = LogRecord::Commit {
            region: 4,
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9)).into()],
            state: vec![7, 8],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        };
        let bytes = record.encode();
        let payload = [
            2, 2, // version, kind
            0, 0, 0, 4, // region
            0, 0, 0, 0, 0, 0, 0, 2, // tick
            0, 0, 0, 0, 0, 0, 0, 1, // epoch
            0, 0, 0, 1, // one change
            0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 3, 0, 9, // the change
            0, 0, 0, 2, 7, 8, // the state
        ];
        assert_eq!(bytes[..4], (payload.len() as u32).to_be_bytes());
        assert_eq!(bytes[4..8], crc32fast::hash(&payload).to_be_bytes());
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));

        let opened = LogRecord::Opened {
            region: 4,
            epoch: 9,
            restored: 300,
        };
        let bytes = opened.encode();
        let payload = [
            2, 3, // version, kind
            0, 0, 0, 4, // region
            0, 0, 0, 0, 0, 0, 0, 9, // epoch
            0, 0, 0, 0, 0, 0, 1, 44, // restored
        ];
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![opened], bytes.len())));
    }

    /// A record of block changes alone, as the world store wrote them before regions
    /// had a state, is still a record of the log. No world this server opens has one:
    /// those worlds are of format 1.
    #[test]
    fn records_of_block_changes_alone_are_read() {
        let payload = [
            2, 1, // version, kind
            0, 0, 0, 0, 0, 0, 0, 2, // tick
            0, 0, 0, 0, 0, 0, 0, 1, // epoch
            0, 0, 0, 1, // one change
            0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 3, 0, 9,
        ];
        let bytes = frame(&payload);
        let expected = LogRecord::Changes {
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9))],
        };
        assert_eq!(expected.encode(), bytes);
        assert_eq!(read_log(&bytes), Ok((vec![expected], bytes.len())));
    }

    /// A record of format 1 is not one this build reads, whatever its kind: the log of
    /// a world from before format 2 is not taken for a log with nothing in it.
    #[test]
    fn a_record_of_format_1_is_refused() {
        for kind in 1..=8 {
            assert_eq!(
                read_log(&frame(&[1, kind, 0, 0, 0, 4])),
                Err(FormatError::UnsupportedVersion(1)),
                "kind {kind}"
            );
        }
    }

    #[test]
    fn an_empty_log_has_no_records() {
        assert_eq!(read_log(&[]), Ok((Vec::new(), 0)));
    }

    /// Whatever byte the process died at while appending, exactly the records written
    /// in full before are read back, each where it was written.
    #[test]
    fn a_log_cut_off_anywhere_yields_the_complete_records() {
        let records = [record(1, 3), record(2, 0), record(5, 40)];
        let mut log = Vec::new();
        let mut ends = Vec::new();
        for record in &records {
            log.extend_from_slice(&record.encode());
            ends.push(log.len());
        }

        for length in 0..=log.len() {
            let complete = ends.iter().filter(|end| **end <= length).count();
            let valid = ends[..complete].last().copied().unwrap_or(0);
            assert_eq!(
                read_log(&log[..length]),
                Ok((records[..complete].to_vec(), valid)),
                "cut at {length}"
            );
        }
        let (logged, _) = read_log_with_offsets(&log).unwrap();
        for (index, logged) in logged.iter().enumerate() {
            let start = if index == 0 { 0 } else { ends[index - 1] };
            assert_eq!((logged.offset, logged.length), (start, ends[index] - start));
            assert_eq!(logged.record, records[index]);
        }
    }

    #[test]
    fn a_damaged_record_ends_the_log() {
        let mut log = record(1, 2).encode();
        let first = log.len();
        log.extend_from_slice(&record(2, 2).encode());
        log.extend_from_slice(&record(3, 2).encode());

        // Damage anywhere in the second record: only the first is read, and the third
        // is not trusted either.
        for index in first..first + record(2, 2).encode().len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            let (records, valid) = read_log(&damaged).unwrap();
            assert_eq!(records, [record(1, 2)], "byte {index}");
            assert_eq!(valid, first);
        }
    }

    #[test]
    fn a_record_that_checks_out_but_makes_no_sense_is_an_error() {
        assert_eq!(
            read_log(&frame(&[9, 1])),
            Err(FormatError::UnsupportedVersion(9))
        );
        assert!(read_log(&frame(&[1, 7])).is_err());
        assert!(read_log(&frame(&[])).is_err());
        // A state longer than what is left.
        let mut payload = record(1, 1).encode()[FRAME_HEADER_LENGTH..].to_vec();
        payload.pop();
        assert!(read_log(&frame(&payload)).is_err());
    }

    /// One of each record that says which region holds what.
    fn records_of_regions() -> [LogRecord; 4] {
        [
            LogRecord::Granted {
                region: 4,
                tick: 2,
                chunks: vec![ChunkPos::new(1, -1), ChunkPos::new(-2, 3)],
            },
            LogRecord::Returned {
                region: 4,
                chunks: vec![ChunkPos::new(-2, 3)],
            },
            LogRecord::Absorbed {
                region: 4,
                epoch: 9,
                absorbed: 5,
                tick: 300,
                state: vec![7, 8],
            },
            LogRecord::Split {
                region: 4,
                epoch: 9,
                tick: 301,
                state: vec![7],
                part: 6,
                part_epoch: 2,
                chunks: vec![ChunkPos::new(1, -1)],
                part_state: vec![8, 9, 10],
            },
        ]
    }

    #[test]
    fn known_answers_of_the_records_of_regions() {
        let payloads: [&[u8]; 4] = [
            &[
                2, 4, // version, kind
                0, 0, 0, 4, // region
                0, 0, 0, 0, 0, 0, 0, 2, // tick
                0, 0, 0, 2, // two chunks
                0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, // the first
                0xFF, 0xFF, 0xFF, 0xFE, 0, 0, 0, 3, // the second
            ],
            &[
                2, 5, // version, kind
                0, 0, 0, 4, // region
                0, 0, 0, 1, // one chunk
                0xFF, 0xFF, 0xFF, 0xFE, 0, 0, 0, 3, // the chunk
            ],
            &[
                2, 6, // version, kind
                0, 0, 0, 4, // region
                0, 0, 0, 0, 0, 0, 0, 9, // epoch
                0, 0, 0, 5, // absorbed
                0, 0, 0, 0, 0, 0, 1, 44, // tick
                0, 0, 0, 2, 7, 8, // the state
            ],
            &[
                2, 7, // version, kind
                0, 0, 0, 4, // region
                0, 0, 0, 0, 0, 0, 0, 9, // epoch
                0, 0, 0, 0, 0, 0, 1, 45, // tick
                0, 0, 0, 1, 7, // the state
                0, 0, 0, 6, // part
                0, 0, 0, 0, 0, 0, 0, 2, // part epoch
                0, 0, 0, 1, // one chunk
                0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, // the chunk
                0, 0, 0, 3, 8, 9, 10, // the part's state
            ],
        ];
        for (record, payload) in records_of_regions().into_iter().zip(payloads) {
            let bytes = record.encode();
            assert_eq!(bytes[..4], (payload.len() as u32).to_be_bytes());
            assert_eq!(bytes[4..8], crc32fast::hash(payload).to_be_bytes());
            assert_eq!(bytes[8..], *payload, "{record:?}");
            assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
        }
    }

    /// The records of regions are cut off and damaged like any other: whatever byte the
    /// process died at, the records written in full before are read back, and a record
    /// with a wrong bit ends the log.
    #[test]
    fn records_of_regions_cut_off_or_damaged_anywhere_end_the_log() {
        let records = records_of_regions();
        let mut log = Vec::new();
        let mut ends = Vec::new();
        for record in &records {
            log.extend_from_slice(&record.encode());
            ends.push(log.len());
        }
        for length in 0..=log.len() {
            let complete = ends.iter().filter(|end| **end <= length).count();
            let valid = ends[..complete].last().copied().unwrap_or(0);
            assert_eq!(
                read_log(&log[..length]),
                Ok((records[..complete].to_vec(), valid)),
                "cut at {length}"
            );
        }
        for index in 0..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            let complete = ends.iter().filter(|end| **end <= index).count();
            let (read, valid) = read_log(&damaged).unwrap();
            assert_eq!(read, records[..complete], "byte {index}");
            assert_eq!(valid, ends[..complete].last().copied().unwrap_or(0));
        }
    }

    #[test]
    fn a_record_of_regions_that_checks_out_but_makes_no_sense_is_an_error() {
        // Kinds there are none of. 8 to 13 were among them until a commit with stay
        // notes, one with the ticks of chunks, one with effects, the store's records
        // of effects and a commit with block entities became kinds of their own; 14
        // stands in for them.
        for kind in [0, 14, 255] {
            assert_eq!(
                read_log(&frame(&[2, kind])),
                Err(FormatError::Corrupt("record kind"))
            );
        }
        for record in records_of_regions() {
            let payload = record.encode()[FRAME_HEADER_LENGTH..].to_vec();
            // A byte too few, a byte too many, and nothing but the version and the kind.
            assert_eq!(
                read_log(&frame(&payload[..payload.len() - 1])),
                Err(FormatError::Truncated),
                "{record:?}"
            );
            let mut longer = payload.clone();
            longer.push(0);
            assert_eq!(
                read_log(&frame(&longer)),
                Err(FormatError::Corrupt("trailing bytes")),
                "{record:?}"
            );
            assert_eq!(read_log(&frame(&payload[..2])), Err(FormatError::Truncated));
        }
        // More chunks than the payload holds, which must not be reserved either.
        let mut payload = vec![2, KIND_RETURNED, 0, 0, 0, 4];
        payload.extend_from_slice(&u32::MAX.to_be_bytes());
        payload.extend_from_slice(&[0; 8]);
        assert_eq!(read_log(&frame(&payload)), Err(FormatError::Truncated));
    }

    /// A commit with the ticks of its chunks is written as kind 9, with its notes or
    /// none, and read back as it was; one without ticks keeps the kind it had.
    #[test]
    fn a_commit_with_the_ticks_of_chunks_is_read_back_as_it_was() {
        let ticks = vec![(
            ChunkPos::new(-3, 7),
            vec![
                ScheduledTick {
                    at: InChunk {
                        y: -61,
                        z: 15,
                        x: 0,
                    },
                    of: Ticked::Block(12),
                    delay: -1,
                    priority: 0,
                },
                ScheduledTick {
                    at: InChunk {
                        y: 300,
                        z: 0,
                        x: 15,
                    },
                    of: Ticked::Fluid(2),
                    delay: 4,
                    priority: -3,
                },
            ],
        )];
        for stays in [
            Vec::new(),
            vec![LoggedStay::Entering {
                player: 9,
                entity: 4,
            }],
        ] {
            let record = LogRecord::Commit {
                region: 2,
                tick: 40,
                epoch: 3,
                changes: vec![(BlockPos::new(1, 2, 3), BlockState(5)).into()],
                state: b"delta".to_vec(),
                stays,
                ticks: ticks.clone(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
            };
            let bytes = record.encode();
            assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_TICKS);
            assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
        }
    }

    // Tests of the commit with the ticks of chunks (ADR-0024, section 4, "The format"),
    // written from the record by someone who did not write the code.

    /// A tick as a chunk keeps it, of a block or of a fluid.
    fn kept_tick(x: u8, y: i16, z: u8, of: Ticked, delay: i32, priority: i8) -> ScheduledTick {
        ScheduledTick {
            at: InChunk { y, z, x },
            of,
            delay,
            priority,
        }
    }

    /// The ticks of three chunks: one with ticks of both queues, one whose list became
    /// empty in the tick, and one with a tick of delay 0 scheduled in the tick, which is
    /// kept as −1.
    fn ticks_of_three_chunks() -> Vec<(ChunkPos, Vec<ScheduledTick>)> {
        vec![
            (
                ChunkPos::new(-4, 2),
                vec![
                    kept_tick(0, -64, 15, Ticked::Block(200), -1, 0),
                    kept_tick(7, 0, 7, Ticked::Fluid(2), 5, -3),
                    kept_tick(15, 319, 0, Ticked::Block(u16::MAX), i32::MAX, i8::MIN),
                ],
            ),
            (ChunkPos::new(-1, 0), Vec::new()),
            (
                ChunkPos::new(30_000, -30_000),
                vec![kept_tick(3, -61, 4, Ticked::Block(1), -1, i8::MAX)],
            ),
        ]
    }

    /// A commit of `tick` with these ticks, without notes of stays.
    fn commit_with_ticks(tick: u64, ticks: Vec<(ChunkPos, Vec<ScheduledTick>)>) -> LogRecord {
        LogRecord::Commit {
            region: 6,
            tick,
            epoch: 2,
            changes: vec![
                (BlockPos::new(-60, -61, 40), blocks::AIR).into(),
                (BlockPos::new(-60, -60, 40), blocks::SAND).into(),
            ],
            state: b"a delta".to_vec(),
            stays: Vec::new(),
            ticks,
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        }
    }

    /// The ticks of several chunks are read back as they were written: every field of
    /// every tick, the order of the chunks and of each chunk's list, a list that is
    /// empty, and delays of −1.
    #[test]
    fn a_commit_with_the_ticks_of_several_chunks_with_an_empty_list_and_negative_delays_round_trips()
     {
        let record = commit_with_ticks(12, ticks_of_three_chunks());
        let bytes = record.encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH..FRAME_HEADER_LENGTH + 2], [2, 9]);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// A list that became empty is committed as empty (section 4): a commit whose only
    /// ticks are one empty list is still a commit with ticks, and that list is read
    /// back. Written as a commit without ticks, the chunk's ticks would not be cleared
    /// when the commit is replayed.
    #[test]
    fn a_commit_whose_only_ticks_are_an_empty_list_keeps_that_list() {
        let record = commit_with_ticks(3, vec![(ChunkPos::new(0, 0), Vec::new())]);
        let bytes = record.encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], 9);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// The commit with ticks is the commit of kind 2 with the notes of stays and the
    /// ticks after it, as the table at the top of this file says: what comes before
    /// the notes is written as in a commit without ticks, and a commit without notes
    /// says that it has none.
    #[test]
    fn a_commit_with_ticks_has_the_fields_of_a_commit_without_them_first() {
        let with = commit_with_ticks(12, ticks_of_three_chunks());
        let LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            ..
        } = with.clone()
        else {
            unreachable!()
        };
        let without = LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        };
        let with = with.encode()[FRAME_HEADER_LENGTH..].to_vec();
        let without = without.encode()[FRAME_HEADER_LENGTH..].to_vec();
        assert_eq!(without[..2], [2, 2]);
        assert_eq!(with[..2], [2, 9]);
        assert_eq!(with[2..without.len()], without[2..]);
        // No notes.
        assert_eq!(with[without.len()..without.len() + 4], [0, 0, 0, 0]);
        // Three chunks.
        assert_eq!(with[without.len() + 4..without.len() + 8], [0, 0, 0, 3]);
    }

    /// A commit with ticks and notes of stays keeps both.
    #[test]
    fn a_commit_with_ticks_and_notes_of_stays_keeps_both() {
        let LogRecord::Commit { stays, .. } = commit_with_stays() else {
            unreachable!()
        };
        let LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            ticks,
            ..
        } = commit_with_ticks(12, ticks_of_three_chunks())
        else {
            unreachable!()
        };
        let record = LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            stays,
            ticks,
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        };
        let bytes = record.encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], 9);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// A commit without ticks is written byte for byte as it was before there were
    /// any, with notes of stays or without: the known answers of kinds 2 and 8.
    #[test]
    fn a_commit_without_ticks_is_written_byte_for_byte_as_before() {
        let plain = LogRecord::Commit {
            region: 4,
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9)).into()],
            state: vec![7, 8],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        };
        let payload = [
            2, 2, // version, kind
            0, 0, 0, 4, // region
            0, 0, 0, 0, 0, 0, 0, 2, // tick
            0, 0, 0, 0, 0, 0, 0, 1, // epoch
            0, 0, 0, 1, // one change
            0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 3, 0, 9, // the change
            0, 0, 0, 2, 7, 8, // the state
        ];
        assert_eq!(plain.encode(), frame(&payload));
        // The notes of the commit with stays follow its state directly, as in kind 8.
        let bytes = commit_with_stays().encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH..FRAME_HEADER_LENGTH + 2], [2, 8]);
        assert_eq!(
            bytes[FRAME_HEADER_LENGTH + 26..FRAME_HEADER_LENGTH + 36],
            [0, 0, 0, 2, 7, 8, 0, 0, 0, 2]
        );
    }

    /// A log written before there were commits with ticks, of records of kinds 1, 2,
    /// 3 and 8 in bytes as they were written then, is read as it was, its commits
    /// without ticks; and a commit with ticks appended to it is read after them.
    #[test]
    fn an_older_log_without_commits_with_ticks_reads_unchanged() {
        let changes = [
            2, 1, // version, kind
            0, 0, 0, 0, 0, 0, 0, 2, // tick
            0, 0, 0, 0, 0, 0, 0, 1, // epoch
            0, 0, 0, 1, // one change
            0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 3, 0, 9,
        ];
        let commit = [
            2, 2, // version, kind
            0, 0, 0, 4, // region
            0, 0, 0, 0, 0, 0, 0, 2, // tick
            0, 0, 0, 0, 0, 0, 0, 1, // epoch
            0, 0, 0, 1, // one change
            0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 3, 0, 9, // the change
            0, 0, 0, 2, 7, 8, // the state
        ];
        let opened = [
            2, 3, // version, kind
            0, 0, 0, 4, // region
            0, 0, 0, 0, 0, 0, 0, 9, // epoch
            0, 0, 0, 0, 0, 0, 1, 44, // restored
        ];
        let mut log = frame(&changes);
        log.extend(frame(&commit));
        log.extend(commit_with_stays().encode());
        log.extend(frame(&opened));
        let older = vec![
            LogRecord::Changes {
                tick: 2,
                epoch: 1,
                changes: vec![(BlockPos::new(1, -1, 3), BlockState(9))],
            },
            LogRecord::Commit {
                region: 4,
                tick: 2,
                epoch: 1,
                changes: vec![(BlockPos::new(1, -1, 3), BlockState(9)).into()],
                state: vec![7, 8],
                stays: Vec::new(),
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
            },
            commit_with_stays(),
            LogRecord::Opened {
                region: 4,
                epoch: 9,
                restored: 300,
            },
        ];
        assert_eq!(read_log(&log), Ok((older.clone(), log.len())));

        let newer = commit_with_ticks(3, ticks_of_three_chunks());
        log.extend(newer.encode());
        let mut all = older;
        all.push(newer);
        assert_eq!(read_log(&log), Ok((all, log.len())));
    }

    /// A commit with ticks is cut off and damaged like any other record: what is left
    /// of it is not read, and what came before it is.
    #[test]
    fn a_commit_with_ticks_cut_off_or_damaged_anywhere_ends_the_log() {
        let first = record(1, 2);
        let mut log = first.encode();
        let start = log.len();
        let with_ticks = commit_with_ticks(2, ticks_of_three_chunks());
        log.extend_from_slice(&with_ticks.encode());
        for length in start..log.len() {
            assert_eq!(
                read_log(&log[..length]),
                Ok((vec![first.clone()], start)),
                "cut at {length}"
            );
        }
        for index in start..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            assert_eq!(
                read_log(&damaged),
                Ok((vec![first.clone()], start)),
                "byte {index}"
            );
        }
        assert_eq!(read_log(&log), Ok((vec![first, with_ticks], log.len())));
    }

    /// A commit whose changes are blocks and block entities' data, interleaved.
    fn commit_with_entities(changes: Vec<Change>) -> LogRecord {
        LogRecord::Commit {
            region: 2,
            tick: 7,
            epoch: 3,
            changes,
            state: vec![1, 2, 3],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        }
    }

    /// `docs/adr/0026-redstone.md`, section 6: the tick's changes are read back in the
    /// order it made them, with a block entity's data before every block, between
    /// them, after them all, twice at one place, or with no blocks at all.
    #[test]
    fn a_commit_with_block_entities_is_read_back_in_the_order_of_its_changes() {
        let at = |x| BlockPos::new(x, -60, 4);
        let entity = |x, data: &[u8]| Change::BlockEntity(at(x), data.to_vec());
        let block = |x| Change::Block(at(x), BlockState(5544));
        for changes in [
            vec![entity(0, b"first"), block(1), block(2)],
            vec![block(1), entity(1, b"between"), block(2)],
            vec![block(1), block(2), entity(2, b"last")],
            vec![block(1), entity(1, b""), entity(1, b"again"), block(2)],
            vec![entity(3, b"alone")],
        ] {
            let record = commit_with_entities(changes.clone());
            let bytes = record.encode();
            assert_eq!(
                bytes[FRAME_HEADER_LENGTH + 1],
                KIND_COMMIT_WITH_ENTITIES,
                "{changes:?}"
            );
            assert_eq!(
                read_log(&bytes),
                Ok((vec![record], bytes.len())),
                "{changes:?}"
            );
        }
    }

    /// A commit of blocks alone is written as it was before block entities were.
    #[test]
    fn a_commit_of_blocks_alone_is_not_written_with_block_entities() {
        let record =
            commit_with_entities(vec![Change::Block(BlockPos::new(1, 2, 3), BlockState(9))]);
        assert_eq!(record.encode()[FRAME_HEADER_LENGTH + 1], KIND_COMMIT);
    }

    /// A block entity's place that runs backwards or past the blocks is corrupt, and
    /// so is a commit of kind 13 with none: neither is ever written.
    #[test]
    fn a_commit_with_block_entities_out_of_place_is_an_error() {
        let blocks = vec![
            (BlockPos::new(1, 2, 3), BlockState(9)),
            (BlockPos::new(1, 2, 4), BlockState(9)),
        ];
        let entity = |place: u32| (place, BlockPos::new(1, 2, 3), b"data".to_vec());
        assert!(join_changes(blocks.clone(), vec![entity(1), entity(0)]).is_err());
        assert!(join_changes(blocks.clone(), vec![entity(3)]).is_err());
        assert_eq!(
            join_changes(blocks.clone(), vec![entity(2), entity(2)]).map(|changes| changes.len()),
            Ok(4)
        );
        // Kind 13 with no block entities: the fields of kind 10 with every count 0,
        // and a count of 0 for the block entities.
        let mut payload = vec![FORMAT_VERSION, KIND_COMMIT_WITH_ENTITIES];
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&7u64.to_be_bytes());
        payload.extend_from_slice(&3u64.to_be_bytes());
        put_changes(&mut payload, &blocks);
        put_bytes(&mut payload, &[1]);
        for _ in 0..4 {
            payload.extend_from_slice(&0u32.to_be_bytes());
        }
        crate::effects::put_effects(&mut payload, &[]);
        crate::effects::put_ids(&mut payload, &[]);
        payload.extend_from_slice(&0u32.to_be_bytes());
        assert!(LogRecord::decode(&payload).is_err());
    }

    /// Cut off or damaged anywhere, a commit with block entities ends the log there.
    #[test]
    fn a_commit_with_block_entities_cut_off_or_damaged_anywhere_ends_the_log() {
        let first = record(1, 2);
        let mut log = first.encode();
        let start = log.len();
        let with_entities = commit_with_entities(vec![
            Change::Block(BlockPos::new(1, 2, 3), BlockState(5544)),
            Change::BlockEntity(BlockPos::new(1, 2, 3), b"chest".to_vec()),
        ]);
        log.extend_from_slice(&with_entities.encode());
        for length in start..log.len() {
            assert_eq!(
                read_log(&log[..length]),
                Ok((vec![first.clone()], start)),
                "cut at {length}"
            );
        }
        for index in start..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            assert_eq!(
                read_log(&damaged),
                Ok((vec![first.clone()], start)),
                "byte {index}"
            );
        }
        assert_eq!(read_log(&log), Ok((vec![first, with_entities], log.len())));
    }

    /// What the store compares a record with before it writes it.
    #[test]
    fn a_record_longer_than_the_limit_is_read_as_cut_off() {
        let absorbed = |state: usize| {
            LogRecord::Absorbed {
                region: 1,
                epoch: 1,
                absorbed: 2,
                tick: 1,
                state: vec![0; state],
            }
            .encode()
        };
        // The payload has 30 bytes besides the state.
        let fits = absorbed(MAX_PAYLOAD_LENGTH - 30);
        assert_eq!(fits.len(), MAX_RECORD_LENGTH);
        let (records, valid) = read_log(&fits).unwrap();
        assert_eq!((records.len(), valid), (1, fits.len()));

        let mut too_long = absorbed(MAX_PAYLOAD_LENGTH - 29);
        assert_eq!(too_long.len(), MAX_RECORD_LENGTH + 1);
        // And whatever follows it is hidden with it.
        too_long.extend(records_of_regions()[0].encode());
        assert_eq!(read_log(&too_long), Ok((Vec::new(), 0)));
    }

    /// A place in which nothing is as it is by default.
    fn place() -> LoggedPlace {
        let mut hotbar = [None; HOTBAR_SLOTS];
        hotbar[0] = Some((1, 64));
        hotbar[8] = Some((-2, 258));
        LoggedPlace {
            position: [1.5, -2.0, 0.0],
            yaw: 90.0,
            pitch: -45.0,
            on_ground: false,
            flying: true,
            selected_slot: 8,
            hotbar,
        }
    }

    /// A commit with one note of each kind.
    fn commit_with_stays() -> LogRecord {
        LogRecord::Commit {
            region: 4,
            tick: 2,
            epoch: 1,
            changes: Vec::new(),
            state: vec![7, 8],
            stays: vec![
                LoggedStay::Entering {
                    player: 0x0102_0304_0506_0708_090A_0B0C_0D0E_0F10,
                    entity: 258,
                },
                LoggedStay::Has {
                    player: 3,
                    entity: -1,
                    hops: 5,
                    place: place(),
                },
            ],
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        }
    }

    /// A commit with stay notes is a record of a kind of its own: the fields of a
    /// commit, and then the notes.
    #[test]
    fn known_answer_of_a_commit_with_stays() {
        let record = commit_with_stays();
        let bytes = record.encode();
        let payload = [
            2, 8, // version, kind
            0, 0, 0, 4, // region
            0, 0, 0, 0, 0, 0, 0, 2, // tick
            0, 0, 0, 0, 0, 0, 0, 1, // epoch
            0, 0, 0, 0, // no changes
            0, 0, 0, 2, 7, 8, // the state
            0, 0, 0, 2, // two notes
            1, // entering
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, // the player
            0, 0, 1, 2, // the entity
            2, // has
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, // the player
            0xFF, 0xFF, 0xFF, 0xFF, // the entity
            0, 0, 0, 5, // hops
            0x3F, 0xF8, 0, 0, 0, 0, 0, 0, // x: 1.5
            0xC0, 0, 0, 0, 0, 0, 0, 0, // y: -2
            0, 0, 0, 0, 0, 0, 0, 0, // z: 0
            0x42, 0xB4, 0, 0, // yaw: 90
            0xC2, 0x34, 0, 0, // pitch: -45
            2, // flying, not on the ground
            8, // the selected slot
            1, 0, 0, 0, 1, 0, 0, 0, 64, // the first slot
            0, 0, 0, 0, 0, 0, 0, // seven empty slots
            1, 0xFF, 0xFF, 0xFF, 0xFE, 0, 0, 1, 2, // the last slot
        ];
        assert_eq!(bytes[..4], (payload.len() as u32).to_be_bytes());
        assert_eq!(bytes[4..8], crc32fast::hash(&payload).to_be_bytes());
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// A commit without notes is written as it was before there were any, and such a
    /// record is read as a commit without notes.
    #[test]
    fn a_commit_without_stays_is_of_the_kind_it_always_was() {
        let LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            ..
        } = commit_with_stays()
        else {
            unreachable!()
        };
        let without = LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        };
        let bytes = without.encode();
        assert_eq!(bytes[8..10], [2, KIND_COMMIT]);
        // Nothing follows the state.
        assert_eq!(bytes[bytes.len() - 6..], [0, 0, 0, 2, 7, 8]);
        assert_eq!(read_log(&bytes), Ok((vec![without], bytes.len())));
        assert_eq!(commit_with_stays().encode()[8..10], [2, 8]);
    }

    /// A commit with stays is cut off and damaged like any other record.
    #[test]
    fn a_commit_with_stays_cut_off_or_damaged_anywhere_ends_the_log() {
        let first = record(1, 2);
        let mut log = first.encode();
        let start = log.len();
        log.extend_from_slice(&commit_with_stays().encode());
        for length in start..log.len() {
            assert_eq!(
                read_log(&log[..length]),
                Ok((vec![first.clone()], start)),
                "cut at {length}"
            );
        }
        for index in start..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            assert_eq!(
                read_log(&damaged),
                Ok((vec![first.clone()], start)),
                "byte {index}"
            );
        }
        assert_eq!(
            read_log(&log),
            Ok((vec![first, commit_with_stays()], log.len()))
        );
    }

    #[test]
    fn a_commit_with_stays_that_checks_out_but_makes_no_sense_is_an_error() {
        let payload = commit_with_stays().encode()[FRAME_HEADER_LENGTH..].to_vec();
        assert_eq!(
            read_log(&frame(&payload[..payload.len() - 1])),
            Err(FormatError::Truncated)
        );
        let mut longer = payload.clone();
        longer.push(0);
        assert_eq!(
            read_log(&frame(&longer)),
            Err(FormatError::Corrupt("trailing bytes"))
        );
        // The payload up to the count of the notes, which begin 32 bytes in.
        let notes = 32;
        assert_eq!(payload[notes..notes + 4], [0, 0, 0, 2]);
        // No notes at all: such a commit is of the other kind.
        let mut none = payload[..notes].to_vec();
        none.extend_from_slice(&[0; 4]);
        assert_eq!(
            read_log(&frame(&none)),
            Err(FormatError::Corrupt("a commit with stays that has none"))
        );
        // More notes than the payload holds, which must not be reserved either.
        let mut many = payload.clone();
        many[notes..notes + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read_log(&frame(&many)), Err(FormatError::Truncated));
        // A note of no kind.
        let mut unknown = payload.clone();
        unknown[notes + 4] = 3;
        assert_eq!(
            read_log(&frame(&unknown)),
            Err(FormatError::Corrupt("stay kind"))
        );
        // The second note begins behind the first, which is 21 bytes; its place begins
        // behind its kind, player, entity and hops, with 32 bytes of position and look.
        let flags = notes + 4 + 21 + 25 + 32;
        assert_eq!(payload[flags], 2);
        let mut flagged = payload.clone();
        flagged[flags] = 4;
        assert_eq!(
            read_log(&frame(&flagged)),
            Err(FormatError::Corrupt("place flags"))
        );
        let mut slot = payload.clone();
        assert_eq!(slot[flags + 2], 1);
        slot[flags + 2] = 2;
        assert_eq!(
            read_log(&frame(&slot)),
            Err(FormatError::Corrupt("hotbar slot"))
        );
    }

    /// Places are the same if their bytes are, so that a record that was read is the
    /// record that was written whatever numbers a region put into it.
    #[test]
    fn places_are_compared_by_their_bytes() {
        let nan = LoggedPlace {
            position: [f64::NAN, 0.0, -0.0],
            yaw: f32::NAN,
            ..place()
        };
        assert_eq!(nan, nan);
        let zero = LoggedPlace {
            position: [f64::NAN, 0.0, 0.0],
            ..nan
        };
        assert_ne!(nan, zero);
        let record = LogRecord::Commit {
            region: 1,
            tick: 1,
            epoch: 1,
            changes: Vec::new(),
            state: Vec::new(),
            stays: vec![LoggedStay::Has {
                player: 1,
                entity: 1,
                hops: 0,
                place: nan,
            }],
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
        };
        let bytes = record.encode();
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    fn places() -> impl Strategy<Value = LoggedPlace> {
        (
            any::<[f64; 3]>(),
            any::<(f32, f32)>(),
            any::<(bool, bool, u8)>(),
            any::<[Option<(i32, i32)>; HOTBAR_SLOTS]>(),
        )
            .prop_map(
                |(position, (yaw, pitch), (on_ground, flying, selected_slot), hotbar)| {
                    LoggedPlace {
                        position,
                        yaw,
                        pitch,
                        on_ground,
                        flying,
                        selected_slot,
                        hotbar,
                    }
                },
            )
    }

    fn stays() -> impl Strategy<Value = Vec<LoggedStay>> {
        let stay = prop_oneof![
            (any::<u128>(), any::<i32>())
                .prop_map(|(player, entity)| LoggedStay::Entering { player, entity }),
            (any::<u128>(), any::<i32>(), any::<u32>(), places()).prop_map(
                |(player, entity, hops, place)| LoggedStay::Has {
                    player,
                    entity,
                    hops,
                    place,
                }
            ),
        ];
        prop::collection::vec(stay, 0..20)
    }

    fn scheduled_ticks() -> impl Strategy<Value = ScheduledTick> {
        let of = prop_oneof![
            any::<u16>().prop_map(Ticked::Block),
            any::<u8>().prop_map(Ticked::Fluid),
        ];
        (
            0..16_u8,
            any::<i16>(),
            0..16_u8,
            of,
            any::<i32>(),
            any::<i8>(),
        )
            .prop_map(|(x, y, z, of, delay, priority)| ScheduledTick {
                at: InChunk { y, z, x },
                of,
                delay,
                priority,
            })
    }

    fn chunks() -> impl Strategy<Value = Vec<ChunkPos>> {
        prop::collection::vec(
            (any::<i32>(), any::<i32>()).prop_map(|(x, z)| ChunkPos::new(x, z)),
            0..50,
        )
    }

    proptest! {
        #[test]
        fn records_of_regions_round_trip(
            region: u32,
            other: u32,
            tick: u64,
            epoch: u64,
            other_epoch: u64,
            granted in chunks(),
            returned in chunks(),
            state in prop::collection::vec(any::<u8>(), 0..200),
            other_state in prop::collection::vec(any::<u8>(), 0..200),
        ) {
            let records = vec![
                LogRecord::Granted { region, tick, chunks: granted },
                LogRecord::Returned { region, chunks: returned.clone() },
                LogRecord::Absorbed {
                    region,
                    epoch,
                    absorbed: other,
                    tick,
                    state: state.clone(),
                },
                LogRecord::Split {
                    region,
                    epoch,
                    tick,
                    state,
                    part: other,
                    part_epoch: other_epoch,
                    chunks: returned,
                    part_state: other_state,
                },
            ];
            let bytes: Vec<u8> = records.iter().flat_map(LogRecord::encode).collect();
            prop_assert_eq!(read_log(&bytes), Ok((records, bytes.len())));
        }

        #[test]
        fn records_round_trip(
            region: u32,
            tick: u64,
            epoch: u64,
            changes in prop::collection::vec((any::<i32>(), any::<i32>(), any::<i32>(), any::<u16>()), 0..50),
            state in prop::collection::vec(any::<u8>(), 0..200),
            restored: u64,
            stays in stays(),
        ) {
            let commit = LogRecord::Commit {
                region,
                tick,
                epoch,
                changes: changes
                    .into_iter()
                    .map(|(x, y, z, state)| (BlockPos::new(x, y, z), BlockState(state)).into())
                    .collect(),
                state,
                stays,
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
            };
            let opened = LogRecord::Opened { region, epoch, restored };
            let mut bytes = commit.encode();
            bytes.extend(opened.encode());
            prop_assert_eq!(read_log(&bytes), Ok((vec![commit, opened], bytes.len())));
        }

        /// Commits with the ticks of any chunks, of any number, are read back as they
        /// were written.
        #[test]
        fn commits_with_ticks_round_trip(
            tick: u64,
            ticks in prop::collection::vec(
                (
                    (any::<i32>(), any::<i32>()).prop_map(|(x, z)| ChunkPos::new(x, z)),
                    prop::collection::vec(scheduled_ticks(), 0..20),
                ),
                1..10,
            ),
            stays in stays(),
        ) {
            let commit = LogRecord::Commit {
                region: 1,
                tick,
                epoch: 3,
                changes: Vec::new(),
                state: Vec::new(),
                stays,
                ticks,
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
            };
            let bytes = commit.encode();
            prop_assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], 9);
            prop_assert_eq!(read_log(&bytes), Ok((vec![commit], bytes.len())));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = read_log(&bytes);
        }
    }
}
