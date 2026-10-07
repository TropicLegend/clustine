//! The write-ahead log: block changes that have not made it into stored chunks yet.
//!
//! The log is a sequence of records, each framed as:
//!
//! | Field | Type |
//! |---|---|
//! | Length of the payload | u32 |
//! | CRC-32 of the payload | u32 |
//! | Payload | bytes |
//!
//! and the payload of a record of block changes is:
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Record kind | u8, 1 for block changes |
//! | Tick the changes happened in | u64 |
//! | Epoch of the region | u64 |
//! | Number of changes | u32 |
//! | Per change | i32 x, i32 y, i32 z, u16 block state |
//!
//! All integers are big-endian. A process can die while appending, which leaves a
//! partial record at the end. Reading therefore stops at the first record that is
//! incomplete or fails its checksum, and reports how many bytes were valid.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

const KIND_BLOCK_CHANGES: u8 = 1;

/// Bytes of the length and the checksum in front of every payload.
const FRAME_HEADER_LENGTH: usize = 8;

/// A payload longer than this is taken for damage, not for a record.
const MAX_PAYLOAD_LENGTH: usize = 64 * 1024 * 1024;

/// The blocks that changed during one tick, in the order they changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockChanges {
    pub tick: u64,
    /// The epoch of the owner of the region that made the changes: the one it opened the
    /// region at the world store with.
    pub epoch: u64,
    pub changes: Vec<(BlockPos, BlockState)>,
}

impl BlockChanges {
    /// The record as it is appended to the log, frame included.
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = vec![FORMAT_VERSION, KIND_BLOCK_CHANGES];
        payload.extend_from_slice(&self.tick.to_be_bytes());
        payload.extend_from_slice(&self.epoch.to_be_bytes());
        payload.extend_from_slice(&(self.changes.len() as u32).to_be_bytes());
        for (position, state) in &self.changes {
            payload.extend_from_slice(&position.x.to_be_bytes());
            payload.extend_from_slice(&position.y.to_be_bytes());
            payload.extend_from_slice(&position.z.to_be_bytes());
            payload.extend_from_slice(&state.0.to_be_bytes());
        }

        let mut record = Vec::with_capacity(FRAME_HEADER_LENGTH + payload.len());
        record.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        record.extend_from_slice(&crc32fast::hash(&payload).to_be_bytes());
        record.extend_from_slice(&payload);
        record
    }

    fn decode(payload: &[u8]) -> Result<Self, FormatError> {
        let mut input = Input(payload);
        let version = input.u8()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        if input.u8()? != KIND_BLOCK_CHANGES {
            return Err(FormatError::Corrupt("record kind"));
        }
        let tick = input.u64()?;
        let epoch = input.u64()?;
        let count = input.u32()? as usize;
        // Each change takes 14 bytes; a forged count must not reserve more than that.
        let mut changes = Vec::with_capacity(count.min(input.0.len() / 14));
        for _ in 0..count {
            let position = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
            changes.push((position, BlockState(input.u16()?)));
        }
        input.finish()?;
        Ok(Self {
            tick,
            epoch,
            changes,
        })
    }
}

/// Reads the records at the start of `log`. Returns them with the number of bytes they
/// occupy; anything after that is a record that was not written completely.
///
/// A record that is complete and passes its checksum but cannot be understood is an
/// error: that is not what dying in the middle of a write looks like.
pub fn read_log(log: &[u8]) -> Result<(Vec<BlockChanges>, usize), FormatError> {
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
        records.push(BlockChanges::decode(payload)?);
        valid += FRAME_HEADER_LENGTH + length;
    }
    Ok((records, valid))
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use proptest::prelude::*;

    use super::*;

    fn record(tick: u64, count: i32) -> BlockChanges {
        BlockChanges {
            tick,
            epoch: 1,
            changes: (0..count)
                .map(|index| (BlockPos::new(index, -61, -index), blocks::STONE))
                .collect(),
        }
    }

    #[test]
    fn known_answer() {
        let record = BlockChanges {
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9))],
        };
        let bytes = record.encode();
        let payload = [
            1, 1, // version, kind
            0, 0, 0, 0, 0, 0, 0, 2, // tick
            0, 0, 0, 0, 0, 0, 0, 1, // epoch
            0, 0, 0, 1, // one change
            0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 3, 0, 9,
        ];
        assert_eq!(bytes[..4], (payload.len() as u32).to_be_bytes());
        assert_eq!(bytes[4..8], crc32fast::hash(&payload).to_be_bytes());
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    #[test]
    fn an_empty_log_has_no_records() {
        assert_eq!(read_log(&[]), Ok((Vec::new(), 0)));
    }

    /// Whatever byte the process died at while appending, exactly the records written
    /// in full before are read back.
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
        let framed = |payload: &[u8]| {
            let mut record = (payload.len() as u32).to_be_bytes().to_vec();
            record.extend_from_slice(&crc32fast::hash(payload).to_be_bytes());
            record.extend_from_slice(payload);
            record
        };
        assert_eq!(
            read_log(&framed(&[9, 1])),
            Err(FormatError::UnsupportedVersion(9))
        );
        assert!(read_log(&framed(&[1, 7])).is_err());
        assert!(read_log(&framed(&[])).is_err());
    }

    proptest! {
        #[test]
        fn records_round_trip(
            tick: u64,
            epoch: u64,
            changes in prop::collection::vec((any::<i32>(), any::<i32>(), any::<i32>(), any::<u16>()), 0..50),
        ) {
            let record = BlockChanges {
                tick,
                epoch,
                changes: changes
                    .into_iter()
                    .map(|(x, y, z, state)| (BlockPos::new(x, y, z), BlockState(state)))
                    .collect(),
            };
            let bytes = record.encode();
            prop_assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = read_log(&bytes);
        }
    }
}
