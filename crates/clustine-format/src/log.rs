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
//!
//! where `changes` is a count (u32) followed, per change, by i32 x, i32 y, i32 z and a
//! u16 block state; `chunks` is a count (u32) followed, per chunk, by i32 x and i32 z;
//! and `bytes` is a length (u32) followed by that many bytes. All integers are
//! big-endian.
//!
//! Kinds 4 to 7 are what decides which region holds which chunk and which regions there
//! are; see `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! A process can die while appending, which leaves a partial record at the end. Reading
//! therefore stops at the first record that is incomplete or fails its checksum, and
//! reports how many bytes were valid.

use clustine_data::BlockState;
use clustine_world::{BlockPos, ChunkPos};

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

const KIND_CHANGES: u8 = 1;
const KIND_COMMIT: u8 = 2;
const KIND_OPENED: u8 = 3;
const KIND_GRANTED: u8 = 4;
const KIND_RETURNED: u8 = 5;
const KIND_ABSORBED: u8 = 6;
const KIND_SPLIT: u8 = 7;

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
    /// as it is.
    Commit {
        region: u32,
        tick: u64,
        /// The epoch of the owner that committed it.
        epoch: u64,
        changes: Vec<(BlockPos, BlockState)>,
        state: Vec<u8>,
    },
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
            } => {
                payload.push(KIND_COMMIT);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                put_changes(&mut payload, changes);
                put_bytes(&mut payload, state);
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
                changes: take_changes(&mut input)?,
                state: take_bytes(&mut input)?,
            },
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
                .map(|index| (BlockPos::new(index, -61, -index), blocks::STONE))
                .collect(),
            state: vec![tick as u8; count as usize],
        }
    }

    #[test]
    fn known_answer() {
        let record = LogRecord::Commit {
            region: 4,
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9))],
            state: vec![7, 8],
        };
        let bytes = record.encode();
        let payload = [
            1, 2, // version, kind
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
            1, 3, // version, kind
            0, 0, 0, 4, // region
            0, 0, 0, 0, 0, 0, 0, 9, // epoch
            0, 0, 0, 0, 0, 0, 1, 44, // restored
        ];
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![opened], bytes.len())));
    }

    /// A log as the world store wrote it before regions had a state is still read.
    #[test]
    fn records_of_block_changes_alone_are_read() {
        let payload = [
            1, 1, // version, kind
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
                1, 4, // version, kind
                0, 0, 0, 4, // region
                0, 0, 0, 0, 0, 0, 0, 2, // tick
                0, 0, 0, 2, // two chunks
                0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, // the first
                0xFF, 0xFF, 0xFF, 0xFE, 0, 0, 0, 3, // the second
            ],
            &[
                1, 5, // version, kind
                0, 0, 0, 4, // region
                0, 0, 0, 1, // one chunk
                0xFF, 0xFF, 0xFF, 0xFE, 0, 0, 0, 3, // the chunk
            ],
            &[
                1, 6, // version, kind
                0, 0, 0, 4, // region
                0, 0, 0, 0, 0, 0, 0, 9, // epoch
                0, 0, 0, 5, // absorbed
                0, 0, 0, 0, 0, 0, 1, 44, // tick
                0, 0, 0, 2, 7, 8, // the state
            ],
            &[
                1, 7, // version, kind
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
        // Kinds there are none of.
        for kind in [0, 8, 255] {
            assert_eq!(
                read_log(&frame(&[1, kind])),
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
        let mut payload = vec![1, KIND_RETURNED, 0, 0, 0, 4];
        payload.extend_from_slice(&u32::MAX.to_be_bytes());
        payload.extend_from_slice(&[0; 8]);
        assert_eq!(read_log(&frame(&payload)), Err(FormatError::Truncated));
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
        ) {
            let commit = LogRecord::Commit {
                region,
                tick,
                epoch,
                changes: changes
                    .into_iter()
                    .map(|(x, y, z, state)| (BlockPos::new(x, y, z), BlockState(state)))
                    .collect(),
                state,
            };
            let opened = LogRecord::Opened { region, epoch, restored };
            let mut bytes = commit.encode();
            bytes.extend(opened.encode());
            prop_assert_eq!(read_log(&bytes), Ok((vec![commit, opened], bytes.len())));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = read_log(&bytes);
        }
    }
}
