//! What the world store keeps per region besides the log: what it knows of the region
//! itself, and the region's state as of its last checkpoint.
//!
//! A region file is:
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Kind | u8, 1 for a region file |
//! | Highest epoch the region was opened with | u64 |
//! | First entity id of the region's block, and the one beyond it | i32, i32 |
//! | CRC-32 of everything before | u32 |
//!
//! and a state file is:
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Kind | u8, 2 for a state file |
//! | Tick the state is as of | u64 |
//! | Length of the state | u32 |
//! | The state, which the store does not look into | bytes |
//! | CRC-32 of everything before | u32 |
//!
//! All integers are big-endian. Both are written whole under a temporary name and
//! renamed, so a reader never sees half of one; the checksum is for damage.

use clustine_world::{EntityId, EntityIds};

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

const KIND_REGION: u8 = 1;
const KIND_STATE: u8 = 2;

/// What the world store keeps on disk about a region, so that it survives the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionFile {
    /// The highest epoch the region has been opened with. An owner with a lower one has
    /// been replaced.
    pub epoch: u64,
    /// The entity ids the store issued to the region when it was first opened.
    pub entity_ids: EntityIds,
}

impl RegionFile {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT_VERSION, KIND_REGION];
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.entity_ids.first.0.to_be_bytes());
        out.extend_from_slice(&self.entity_ids.end.0.to_be_bytes());
        seal(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut input = opened(bytes, KIND_REGION)?;
        let epoch = input.u64()?;
        let entity_ids = EntityIds {
            first: EntityId(input.i32()?),
            end: EntityId(input.i32()?),
        };
        input.finish()?;
        Ok(Self { epoch, entity_ids })
    }
}

/// The whole state of a region as of a tick, as its owner handed it to the store at a
/// checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFile {
    pub tick: u64,
    pub state: Vec<u8>,
}

impl StateFile {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.state.len() + 18);
        out.extend_from_slice(&[FORMAT_VERSION, KIND_STATE]);
        out.extend_from_slice(&self.tick.to_be_bytes());
        out.extend_from_slice(&(self.state.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.state);
        seal(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut input = opened(bytes, KIND_STATE)?;
        let tick = input.u64()?;
        let length = input.u32()? as usize;
        let state = input.take(length)?.to_vec();
        input.finish()?;
        Ok(Self { tick, state })
    }
}

/// Appends the checksum of `out` to it.
fn seal(mut out: Vec<u8>) -> Vec<u8> {
    let checksum = crc32fast::hash(&out);
    out.extend_from_slice(&checksum.to_be_bytes());
    out
}

/// Checks the checksum, the version and the kind of `bytes`, and returns what follows
/// them.
fn opened(bytes: &[u8], kind: u8) -> Result<Input<'_>, FormatError> {
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
    if input.u8()? != kind {
        return Err(FormatError::Corrupt("file kind"));
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn known_answer() {
        let region = RegionFile {
            epoch: 5,
            entity_ids: EntityIds::block(1).unwrap(),
        };
        let bytes = region.encode();
        let body = [
            1, 1, // version, kind
            0, 0, 0, 0, 0, 0, 0, 5, // epoch
            0, 0x10, 0, 0, 0, 0x20, 0, 0, // the block
        ];
        assert_eq!(bytes[..body.len()], body);
        assert_eq!(bytes[body.len()..], crc32fast::hash(&body).to_be_bytes());
        assert_eq!(RegionFile::decode(&bytes), Ok(region));

        let state = StateFile {
            tick: 258,
            state: vec![4, 2],
        };
        let bytes = state.encode();
        let body = [
            1, 2, // version, kind
            0, 0, 0, 0, 0, 0, 1, 2, // tick
            0, 0, 0, 2, 4, 2, // the state
        ];
        assert_eq!(bytes[..body.len()], body);
        assert_eq!(StateFile::decode(&bytes), Ok(state));
    }

    #[test]
    fn damage_anywhere_is_noticed() {
        let state = StateFile {
            tick: 9,
            state: b"the whole state".to_vec(),
        }
        .encode();
        for index in 0..state.len() {
            let mut damaged = state.clone();
            damaged[index] ^= 0x01;
            assert!(StateFile::decode(&damaged).is_err(), "byte {index}");
        }
        for length in 0..state.len() {
            assert!(
                StateFile::decode(&state[..length]).is_err(),
                "cut at {length}"
            );
        }
    }

    #[test]
    fn one_kind_of_file_is_not_taken_for_the_other() {
        let region = RegionFile {
            epoch: 1,
            entity_ids: EntityIds::block(0).unwrap(),
        };
        assert!(StateFile::decode(&region.encode()).is_err());
        let state = StateFile {
            tick: 1,
            state: Vec::new(),
        };
        assert!(RegionFile::decode(&state.encode()).is_err());
    }

    proptest! {
        #[test]
        fn files_round_trip(epoch: u64, first: i32, end: i32, tick: u64, state: Vec<u8>) {
            let region = RegionFile {
                epoch,
                entity_ids: EntityIds { first: EntityId(first), end: EntityId(end) },
            };
            prop_assert_eq!(RegionFile::decode(&region.encode()), Ok(region));
            let state = StateFile { tick, state };
            prop_assert_eq!(StateFile::decode(&state.encode()), Ok(state.clone()));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = RegionFile::decode(&bytes);
            let _ = StateFile::decode(&bytes);
        }
    }
}
