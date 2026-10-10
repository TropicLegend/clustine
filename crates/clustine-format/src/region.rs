//! What the world store keeps besides the log: per region what it knows of the region
//! itself and the region's state as of its last checkpoint, and for the world the table
//! of its regions.
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
//! The table file is to the list of regions and the chunks each holds what a state file
//! is to a region: all of it as of a place in the log, so that the log before that place
//! can go. See `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Kind | u8, 3 for the table file |
//! | `from`: the first log segment whose records change this table | u64 |
//! | The next region id | u32 |
//! | The home chunk | i32 x, i32 z |
//! | The home region | u32 |
//! | The division the store was started with: count, then areas | u32, areas |
//! | Regions, in ascending order of their ids: count, then per region | u32 |
//! | … id | u32 |
//! | … the areas it is pinned to: count, areas | u32, areas |
//! | … its grants, ascending by x then z: count, then per grant x, z, tick | u32, (i32, i32, u64)… |
//! | Absorbed regions, oldest first: count, then per pair absorbed, into | u32, (u32, u32)… |
//! | CRC-32 of everything before | u32 |
//!
//! An area is a u8 of flags (bit 0: it has a western end, bit 1: an eastern end) followed
//! by i32 `min_x` and i32 `max_x`, each 0 if absent.
//!
//! The players' file is to the records of the players what the table file is to the list
//! of regions: all of them as of a place in the log. See
//! `docs/adr/0020-one-stay-per-player.md`, sections 2 and 8.
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Kind | u8, 4 for the players' file |
//! | `from`: the first log segment whose stay notes are not in this file | u64 |
//! | `issued`: the highest entity id a stay was ever given | i32 |
//! | Records, ascending by player: count, then per record | u32 |
//! | … player, stay, hops | u128, i32, u32 |
//! | … has a place | u8 0 or 1 |
//! | … the place, if it has one, as in a stay note of the log | |
//! | CRC-32 of everything before | u32 |
//!
//! All integers are big-endian. All four are written whole under a temporary name and
//! renamed, so a reader never sees half of one; the checksum is for damage.

use clustine_world::{ChunkArea, ChunkPos, EntityId, EntityIds};

use crate::bytes::Input;
use crate::log::{LoggedPlace, put_place, take_place};
use crate::{FORMAT_VERSION, FormatError};

const KIND_REGION: u8 = 1;
const KIND_STATE: u8 = 2;
const KIND_TABLE: u8 = 3;
const KIND_PLAYERS: u8 = 4;

/// Bytes a player's record takes in a players' file at the least.
const RECORD_LENGTH: usize = 25;

/// Bytes an area takes in a table file.
const AREA_LENGTH: usize = 9;

/// Bytes a grant takes in a table file.
const GRANT_LENGTH: usize = 16;

/// Bytes a region takes in a table file at the least, and a pair of absorbed regions.
const REGION_LENGTH: usize = 12;
const PAIR_LENGTH: usize = 8;

/// The flags of an area: which of its ends it has.
const HAS_WEST: u8 = 1;
const HAS_EAST: u8 = 2;

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

/// The regions there are and the chunks each holds, as of a place in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableFile {
    /// The first segment of the log whose records change this table. What a record in
    /// an earlier segment says of regions and chunks is in the table already.
    pub from: u64,
    /// The id the next region gets. Every id below it has been a region's, and is never
    /// another's.
    pub next_region: u32,
    /// The chunk players enter the world in.
    pub home_chunk: ChunkPos,
    /// The region that holds the home chunk.
    pub home_region: u32,
    /// The areas of the pinned regions the store was started with when the table was
    /// made, which need not be what the regions are pinned to now.
    pub division: Vec<ChunkArea>,
    /// The regions there are, in ascending order of their ids.
    pub regions: Vec<TableRegion>,
    /// The regions that were absorbed, each with the region it went into, oldest first.
    pub absorbed: Vec<(u32, u32)>,
}

/// A region as the table file has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRegion {
    pub id: u32,
    /// The areas the region is pinned to: it holds every chunk of them that is not
    /// granted to a region.
    pub pinned: Vec<ChunkArea>,
    /// The chunks the region was granted, each with the tick of the region from which
    /// it holds the chunk, in ascending order of the chunks: by x, then by z.
    pub grants: Vec<(ChunkPos, u64)>,
}

impl TableFile {
    /// The file. The regions and each region's grants are written in the order they are
    /// given in, which has to be ascending for the file to be read again.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT_VERSION, KIND_TABLE];
        out.extend_from_slice(&self.from.to_be_bytes());
        out.extend_from_slice(&self.next_region.to_be_bytes());
        out.extend_from_slice(&self.home_chunk.x.to_be_bytes());
        out.extend_from_slice(&self.home_chunk.z.to_be_bytes());
        out.extend_from_slice(&self.home_region.to_be_bytes());
        put_areas(&mut out, &self.division);
        out.extend_from_slice(&(self.regions.len() as u32).to_be_bytes());
        for region in &self.regions {
            out.extend_from_slice(&region.id.to_be_bytes());
            put_areas(&mut out, &region.pinned);
            out.extend_from_slice(&(region.grants.len() as u32).to_be_bytes());
            for (chunk, tick) in &region.grants {
                out.extend_from_slice(&chunk.x.to_be_bytes());
                out.extend_from_slice(&chunk.z.to_be_bytes());
                out.extend_from_slice(&tick.to_be_bytes());
            }
        }
        out.extend_from_slice(&(self.absorbed.len() as u32).to_be_bytes());
        for (absorbed, into) in &self.absorbed {
            out.extend_from_slice(&absorbed.to_be_bytes());
            out.extend_from_slice(&into.to_be_bytes());
        }
        seal(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut input = opened(bytes, KIND_TABLE)?;
        let from = input.u64()?;
        let next_region = input.u32()?;
        let home_chunk = ChunkPos::new(input.i32()?, input.i32()?);
        let home_region = input.u32()?;
        let division = take_areas(&mut input)?;

        let count = input.u32()? as usize;
        // A forged count must not reserve more than the file can hold.
        let mut regions: Vec<TableRegion> =
            Vec::with_capacity(count.min(input.0.len() / REGION_LENGTH));
        for _ in 0..count {
            let id = input.u32()?;
            if regions.last().is_some_and(|before| before.id >= id) {
                return Err(FormatError::Corrupt("regions out of order"));
            }
            let pinned = take_areas(&mut input)?;
            let count = input.u32()? as usize;
            let mut grants: Vec<(ChunkPos, u64)> =
                Vec::with_capacity(count.min(input.0.len() / GRANT_LENGTH));
            for _ in 0..count {
                let chunk = ChunkPos::new(input.i32()?, input.i32()?);
                if grants.last().is_some_and(|(before, _)| *before >= chunk) {
                    return Err(FormatError::Corrupt("grants out of order"));
                }
                grants.push((chunk, input.u64()?));
            }
            regions.push(TableRegion { id, pinned, grants });
        }

        let count = input.u32()? as usize;
        let mut absorbed = Vec::with_capacity(count.min(input.0.len() / PAIR_LENGTH));
        for _ in 0..count {
            absorbed.push((input.u32()?, input.u32()?));
        }
        input.finish()?;
        Ok(Self {
            from,
            next_region,
            home_chunk,
            home_region,
            division,
            regions,
            absorbed,
        })
    }
}

/// What the world store keeps of every player who ever joined, as of a place in the
/// log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayersFile {
    /// The first segment of the log whose stay notes are not in this file.
    pub from: u64,
    /// The highest entity id a stay was ever given; 0 if none was.
    pub issued: i32,
    /// The records, in ascending order of their players.
    pub records: Vec<PlayerRecord>,
}

/// What the world store keeps of one player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerRecord {
    pub player: u128,
    /// The entity of the latest stay the store was told the player was given.
    pub stay: i32,
    /// The highest number of hand-overs the store was told of that stay.
    pub hops: u32,
    /// Where the player was last, if a region has said.
    pub place: Option<LoggedPlace>,
}

impl PlayersFile {
    /// The file. The records are written in the order they are given in, which has to
    /// be ascending for the file to be read again.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT_VERSION, KIND_PLAYERS];
        out.extend_from_slice(&self.from.to_be_bytes());
        out.extend_from_slice(&self.issued.to_be_bytes());
        out.extend_from_slice(&(self.records.len() as u32).to_be_bytes());
        for record in &self.records {
            out.extend_from_slice(&record.player.to_be_bytes());
            out.extend_from_slice(&record.stay.to_be_bytes());
            out.extend_from_slice(&record.hops.to_be_bytes());
            match &record.place {
                None => out.push(0),
                Some(place) => {
                    out.push(1);
                    put_place(&mut out, place);
                }
            }
        }
        seal(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut input = opened(bytes, KIND_PLAYERS)?;
        let from = input.u64()?;
        let issued = input.i32()?;
        let count = input.u32()? as usize;
        // A forged count must not reserve more than the file can hold.
        let mut records: Vec<PlayerRecord> =
            Vec::with_capacity(count.min(input.0.len() / RECORD_LENGTH));
        for _ in 0..count {
            let player = input.u128()?;
            if records.last().is_some_and(|before| before.player >= player) {
                return Err(FormatError::Corrupt("players out of order"));
            }
            let (stay, hops) = (input.i32()?, input.u32()?);
            let place = match input.u8()? {
                0 => None,
                1 => Some(take_place(&mut input)?),
                _ => return Err(FormatError::Corrupt("whether a player has a place")),
            };
            records.push(PlayerRecord {
                player,
                stay,
                hops,
                place,
            });
        }
        input.finish()?;
        Ok(Self {
            from,
            issued,
            records,
        })
    }
}

fn put_areas(out: &mut Vec<u8>, areas: &[ChunkArea]) {
    out.extend_from_slice(&(areas.len() as u32).to_be_bytes());
    for area in areas {
        let west = if area.min_x.is_some() { HAS_WEST } else { 0 };
        let east = if area.max_x.is_some() { HAS_EAST } else { 0 };
        out.push(west | east);
        out.extend_from_slice(&area.min_x.unwrap_or(0).to_be_bytes());
        out.extend_from_slice(&area.max_x.unwrap_or(0).to_be_bytes());
    }
}

fn take_areas(input: &mut Input<'_>) -> Result<Vec<ChunkArea>, FormatError> {
    let count = input.u32()? as usize;
    let mut areas = Vec::with_capacity(count.min(input.0.len() / AREA_LENGTH));
    for _ in 0..count {
        let flags = input.u8()?;
        if flags & !(HAS_WEST | HAS_EAST) != 0 {
            return Err(FormatError::Corrupt("area flags"));
        }
        // An end that is absent is written as 0, so that equal areas have equal bytes.
        let mut end = |flag| {
            let x = input.i32()?;
            match (flags & flag != 0, x) {
                (true, x) => Ok(Some(x)),
                (false, 0) => Ok(None),
                (false, _) => Err(FormatError::Corrupt("an end of an area that has none")),
            }
        };
        areas.push(ChunkArea {
            min_x: end(HAS_WEST)?,
            max_x: end(HAS_EAST)?,
        });
    }
    Ok(areas)
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

    #[test]
    fn the_table_is_not_taken_for_another_kind_of_file_nor_another_for_it() {
        let state = StateFile {
            tick: 1,
            state: Vec::new(),
        };
        let region = RegionFile {
            epoch: 1,
            entity_ids: EntityIds::block(0).unwrap(),
        };
        for other in [state.encode(), region.encode()] {
            assert_eq!(
                TableFile::decode(&other),
                Err(FormatError::Corrupt("file kind"))
            );
        }
        assert!(StateFile::decode(&table().encode()).is_err());
        assert!(RegionFile::decode(&table().encode()).is_err());
    }

    fn table() -> TableFile {
        TableFile {
            from: 7,
            next_region: 4,
            home_chunk: ChunkPos::new(0, -1),
            home_region: 1,
            division: vec![
                ChunkArea {
                    min_x: None,
                    max_x: Some(0),
                },
                ChunkArea {
                    min_x: Some(0),
                    max_x: None,
                },
            ],
            regions: vec![
                TableRegion {
                    id: 1,
                    pinned: vec![
                        ChunkArea {
                            min_x: Some(-2),
                            max_x: Some(3),
                        },
                        ChunkArea::EVERYWHERE,
                    ],
                    grants: Vec::new(),
                },
                TableRegion {
                    id: 3,
                    pinned: Vec::new(),
                    grants: vec![(ChunkPos::new(-1, 5), 9), (ChunkPos::new(2, -6), 258)],
                },
            ],
            absorbed: vec![(0, 1), (2, 3)],
        }
    }

    #[test]
    fn known_answer_of_the_table() {
        let bytes = table().encode();
        let body = [
            1, 3, // version, kind
            0, 0, 0, 0, 0, 0, 0, 7, // from
            0, 0, 0, 4, // the next region id
            0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, // the home chunk
            0, 0, 0, 1, // the home region
            0, 0, 0, 2, // the division: two areas
            2, 0, 0, 0, 0, 0, 0, 0, 0, // west of 0
            1, 0, 0, 0, 0, 0, 0, 0, 0, // from 0 on
            0, 0, 0, 2, // two regions
            0, 0, 0, 1, // region 1
            0, 0, 0, 2, // pinned to two areas
            3, 0xFF, 0xFF, 0xFF, 0xFE, 0, 0, 0, 3, // from -2 up to 3
            0, 0, 0, 0, 0, 0, 0, 0, 0, // everywhere
            0, 0, 0, 0, // no grants
            0, 0, 0, 3, // region 3
            0, 0, 0, 0, // not pinned
            0, 0, 0, 2, // two grants
            0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 9, // the first
            0, 0, 0, 2, 0xFF, 0xFF, 0xFF, 0xFA, 0, 0, 0, 0, 0, 0, 1, 2, // the second
            0, 0, 0, 2, // two absorbed regions
            0, 0, 0, 0, 0, 0, 0, 1, // 0 into 1
            0, 0, 0, 2, 0, 0, 0, 3, // 2 into 3
        ];
        assert_eq!(bytes[..body.len()], body);
        assert_eq!(bytes[body.len()..], crc32fast::hash(&body).to_be_bytes());
        assert_eq!(TableFile::decode(&bytes), Ok(table()));

        // A world with nothing in it.
        let empty = TableFile {
            from: 0,
            next_region: 0,
            home_chunk: ChunkPos::new(0, 0),
            home_region: 0,
            division: Vec::new(),
            regions: Vec::new(),
            absorbed: Vec::new(),
        };
        assert_eq!(empty.encode().len(), 2 + 8 + 4 + 8 + 4 + 4 + 4 + 4 + 4);
        assert_eq!(TableFile::decode(&empty.encode()), Ok(empty));
    }

    fn players() -> PlayersFile {
        let mut hotbar = [None; crate::HOTBAR_SLOTS];
        hotbar[1] = Some((7, 3));
        PlayersFile {
            from: 7,
            issued: 258,
            records: vec![
                PlayerRecord {
                    player: 2,
                    stay: 257,
                    hops: 0,
                    place: None,
                },
                PlayerRecord {
                    player: 0x0102_0304_0506_0708_090A_0B0C_0D0E_0F10,
                    stay: 258,
                    hops: 4,
                    place: Some(LoggedPlace {
                        position: [1.5, -2.0, 0.0],
                        yaw: 90.0,
                        pitch: -45.0,
                        on_ground: true,
                        flying: true,
                        selected_slot: 1,
                        hotbar,
                    }),
                },
            ],
        }
    }

    #[test]
    fn known_answer_of_the_players() {
        let bytes = players().encode();
        let body = [
            1, 4, // version, kind
            0, 0, 0, 0, 0, 0, 0, 7, // from
            0, 0, 1, 2, // issued
            0, 0, 0, 2, // two records
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, // the first player
            0, 0, 1, 1, // their stay
            0, 0, 0, 0, // its hops
            0, // no place
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, // the second player
            0, 0, 1, 2, // their stay
            0, 0, 0, 4, // its hops
            1, // a place
            0x3F, 0xF8, 0, 0, 0, 0, 0, 0, // x: 1.5
            0xC0, 0, 0, 0, 0, 0, 0, 0, // y: -2
            0, 0, 0, 0, 0, 0, 0, 0, // z: 0
            0x42, 0xB4, 0, 0, // yaw: 90
            0xC2, 0x34, 0, 0, // pitch: -45
            3, // on the ground and flying
            1, // the selected slot
            0, // an empty slot
            1, 0, 0, 0, 7, 0, 0, 0, 3, // the second slot
            0, 0, 0, 0, 0, 0, 0, // seven empty slots
        ];
        assert_eq!(bytes[..body.len()], body);
        assert_eq!(bytes[body.len()..], crc32fast::hash(&body).to_be_bytes());
        assert_eq!(PlayersFile::decode(&bytes), Ok(players()));

        // A world nobody has joined.
        let empty = PlayersFile {
            from: 0,
            issued: 0,
            records: Vec::new(),
        };
        assert_eq!(empty.encode().len(), 2 + 8 + 4 + 4 + 4);
        assert_eq!(PlayersFile::decode(&empty.encode()), Ok(empty));
    }

    #[test]
    fn damage_anywhere_in_the_players_is_noticed() {
        let bytes = players().encode();
        for index in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[index] ^= 0x01;
            assert!(PlayersFile::decode(&damaged).is_err(), "byte {index}");
        }
        for length in 0..bytes.len() {
            assert!(
                PlayersFile::decode(&bytes[..length]).is_err(),
                "cut at {length}"
            );
        }
        let mut longer = bytes;
        longer.push(0);
        assert!(PlayersFile::decode(&longer).is_err());
    }

    #[test]
    fn players_that_check_out_but_make_no_sense_are_an_error() {
        // Out of order, and one player twice.
        let mut swapped = players();
        swapped.records.swap(0, 1);
        assert_eq!(
            PlayersFile::decode(&swapped.encode()),
            Err(FormatError::Corrupt("players out of order"))
        );
        let mut twice = players();
        twice.records[1].player = twice.records[0].player;
        assert_eq!(
            PlayersFile::decode(&twice.encode()),
            Err(FormatError::Corrupt("players out of order"))
        );
        // Neither with a place nor without. The byte is behind the head of 18 bytes
        // and the first record's player, stay and hops.
        let mut body = players().encode();
        body.truncate(body.len() - 4);
        assert_eq!(body[18 + 24], 0);
        body[18 + 24] = 2;
        assert_eq!(
            PlayersFile::decode(&seal(body)),
            Err(FormatError::Corrupt("whether a player has a place"))
        );
        // More records than the file holds, which must not be reserved either.
        let mut body = players().encode();
        body.truncate(body.len() - 4);
        body[14..18].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            PlayersFile::decode(&seal(body)),
            Err(FormatError::Truncated)
        );
    }

    #[test]
    fn the_players_are_not_taken_for_another_kind_of_file_nor_another_for_them() {
        for other in [
            table().encode(),
            StateFile {
                tick: 1,
                state: Vec::new(),
            }
            .encode(),
        ] {
            assert_eq!(
                PlayersFile::decode(&other),
                Err(FormatError::Corrupt("file kind"))
            );
        }
        assert!(TableFile::decode(&players().encode()).is_err());
        assert!(StateFile::decode(&players().encode()).is_err());
        assert!(RegionFile::decode(&players().encode()).is_err());
    }

    #[test]
    fn damage_anywhere_in_the_table_is_noticed() {
        let bytes = table().encode();
        for index in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[index] ^= 0x01;
            assert!(TableFile::decode(&damaged).is_err(), "byte {index}");
        }
        for length in 0..bytes.len() {
            assert!(
                TableFile::decode(&bytes[..length]).is_err(),
                "cut at {length}"
            );
        }
        let mut longer = bytes;
        longer.push(0);
        assert!(TableFile::decode(&longer).is_err());
    }

    /// The body of a table file with its checksum put right, so that what is wrong with
    /// it is found by reading it.
    fn resealed(mut bytes: Vec<u8>, change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        bytes.truncate(bytes.len() - 4);
        change(&mut bytes);
        seal(bytes)
    }

    #[test]
    fn a_table_that_checks_out_but_makes_no_sense_is_an_error() {
        let bytes = table().encode();
        // The flags of the first area of the division are at byte 30, its western end,
        // which it has not, from byte 31 on.
        assert_eq!(bytes[30], 2);
        let flags = resealed(bytes.clone(), |body| body[30] = 4);
        assert_eq!(
            TableFile::decode(&flags),
            Err(FormatError::Corrupt("area flags"))
        );
        let end = resealed(bytes.clone(), |body| body[34] = 1);
        assert_eq!(
            TableFile::decode(&end),
            Err(FormatError::Corrupt("an end of an area that has none"))
        );
        // Something behind the last pair, and a pair too few.
        let trailing = resealed(bytes.clone(), |body| body.push(0));
        assert_eq!(
            TableFile::decode(&trailing),
            Err(FormatError::Corrupt("trailing bytes"))
        );
        let short = resealed(bytes.clone(), |body| body.truncate(body.len() - 8));
        assert_eq!(TableFile::decode(&short), Err(FormatError::Truncated));
        // More of anything than the file holds, which must not be reserved either: of
        // absorbed regions, whose count is before the two pairs, and of areas.
        let forged = resealed(bytes.clone(), |body| {
            let count = body.len() - 20;
            body[count..count + 4].fill(0xFF);
        });
        assert_eq!(TableFile::decode(&forged), Err(FormatError::Truncated));
        let forged = resealed(bytes, |body| body[26..30].fill(0xFF));
        assert!(TableFile::decode(&forged).is_err());

        // Regions and grants that are not in ascending order, or are there twice.
        let mut unordered = table();
        unordered.regions.swap(0, 1);
        assert_eq!(
            TableFile::decode(&unordered.encode()),
            Err(FormatError::Corrupt("regions out of order"))
        );
        let mut twice = table();
        twice.regions[1].id = 1;
        assert_eq!(
            TableFile::decode(&twice.encode()),
            Err(FormatError::Corrupt("regions out of order"))
        );
        let mut unordered = table();
        unordered.regions[1].grants.swap(0, 1);
        assert_eq!(
            TableFile::decode(&unordered.encode()),
            Err(FormatError::Corrupt("grants out of order"))
        );
        let mut twice = table();
        twice.regions[1].grants[1].0 = ChunkPos::new(-1, 5);
        assert_eq!(
            TableFile::decode(&twice.encode()),
            Err(FormatError::Corrupt("grants out of order"))
        );
        // By x first, then by z.
        let mut by_x = table();
        by_x.regions[1].grants = vec![(ChunkPos::new(1, 9), 0), (ChunkPos::new(2, -9), 0)];
        assert_eq!(TableFile::decode(&by_x.encode()), Ok(by_x));
    }

    fn area() -> impl Strategy<Value = ChunkArea> {
        (any::<Option<i32>>(), any::<Option<i32>>())
            .prop_map(|(min_x, max_x)| ChunkArea { min_x, max_x })
    }

    fn areas() -> impl Strategy<Value = Vec<ChunkArea>> {
        prop::collection::vec(area(), 0..4)
    }

    proptest! {
        #[test]
        fn the_table_round_trips(
            from: u64,
            next_region: u32,
            home in (any::<i32>(), any::<i32>()),
            home_region: u32,
            division in areas(),
            regions in prop::collection::btree_map(
                any::<u32>(),
                (
                    areas(),
                    prop::collection::btree_map((-40..40, -40..40), any::<u64>(), 0..30),
                ),
                0..6,
            ),
            absorbed: Vec<(u32, u32)>,
        ) {
            let table = TableFile {
                from,
                next_region,
                home_chunk: ChunkPos::new(home.0, home.1),
                home_region,
                division,
                regions: regions
                    .into_iter()
                    .map(|(id, (pinned, grants))| TableRegion {
                        id,
                        pinned,
                        grants: grants
                            .into_iter()
                            .map(|((x, z), tick)| (ChunkPos::new(x, z), tick))
                            .collect(),
                    })
                    .collect(),
                absorbed,
            };
            prop_assert_eq!(TableFile::decode(&table.encode()), Ok(table));
        }

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
            let _ = TableFile::decode(&bytes);
            let _ = PlayersFile::decode(&bytes);
        }

        /// Bytes that pass for a players' file as far as its checksum, version and kind
        /// go.
        #[test]
        fn arbitrary_players_never_panic(mut body: Vec<u8>) {
            body.splice(0..0, [FORMAT_VERSION, KIND_PLAYERS]);
            let _ = PlayersFile::decode(&seal(body));
        }

        #[test]
        fn players_round_trip(
            from: u64,
            issued: i32,
            records in prop::collection::btree_map(
                any::<u128>(),
                (
                    any::<(i32, u32)>(),
                    prop::option::of((
                        any::<[f64; 3]>(),
                        any::<(f32, f32, bool, bool, u8)>(),
                        any::<[Option<(i32, i32)>; crate::HOTBAR_SLOTS]>(),
                    )),
                ),
                0..12,
            ),
        ) {
            let players = PlayersFile {
                from,
                issued,
                records: records
                    .into_iter()
                    .map(|(player, ((stay, hops), place))| PlayerRecord {
                        player,
                        stay,
                        hops,
                        place: place.map(
                            |(position, (yaw, pitch, on_ground, flying, selected_slot), hotbar)| {
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
                        ),
                    })
                    .collect(),
            };
            prop_assert_eq!(PlayersFile::decode(&players.encode()), Ok(players));
        }

        /// Bytes that pass for a table file as far as its checksum, version and kind go.
        #[test]
        fn arbitrary_tables_never_panic(mut body: Vec<u8>) {
            body.splice(0..0, [FORMAT_VERSION, KIND_TABLE]);
            let _ = TableFile::decode(&seal(body));
        }
    }
}
