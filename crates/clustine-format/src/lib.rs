//! The Clustine world format: chunk files, the write-ahead log, what is kept per region,
//! the table of the regions and the chunks they hold, and what is kept of each player.
//!
//! A chunk is stored as one [`ChunkFile`]: its blocks and biomes, its block entities,
//! its light, the ticks and marks generation left, and which generator it is exactly
//! as made by, with a checksum over all of it. Nothing is shared between chunks, so a
//! file is whole by itself and a chunk that is saved again leaves nothing behind
//! (`docs/adr/0022-what-a-chunk-carries.md`, section 3).
//!
//! Block state and biome ids are those of one Minecraft data version, which a world
//! records in its metadata. Moving a world to another version means rewriting them.
//!
//! This crate only turns values into bytes and back. Where the bytes are kept is the
//! world store's business.

mod bytes;
mod chunk;
mod effects;
mod entities;
mod kept_events;
mod log;
mod region;
mod seams;

pub use chunk::{ChunkFile, KIND_CHUNK};
pub use entities::{EntitiesFile, KIND_ENTITIES};
pub use log::{
    ENDER_SLOTS, INVENTORY_SLOTS, LogRecord, Logged, LoggedPlace, LoggedStay, LoggedUnit,
    MAX_RECORD_LENGTH, read_log, read_log_with_offsets,
};
pub use region::{
    BlocksFile, CoupledRegion, CouplingsFile, Handshake, PlayerRecord, PlayersFile, RegionFile,
    StateFile, TableFile, TableRegion,
};

/// The version of the encodings in this crate. It is written into everything stored:
/// the first byte of every file and of every record of the log. A world says in its
/// `meta` which one it is of, and the store opens no world of another.
///
/// 4 since a player's place holds their whole inventory, 41 slots, where it held the
/// nine of the hotbar (`docs/groundwork/n2-plan.md`, section 2); 8 since it holds their
/// ender chest's 27 slots after it (`docs/groundwork/n6-plan.md`, section 7 item 9); 9
/// since a minecart holds a TNT cart's fuse and a hopper cart's switch; 10 since an
/// entity keeps `lived` and `fresh_at`, an item its age apart, a change may set an
/// entity's hops, and a mob is kept by the parts of its body and mind and its type's
/// state (`docs/groundwork/n7-plan.md`, section 11.3); 11 since a player's place holds
/// their experience after their ender chest (section 7.7 of the same plan); 12 since it
/// holds where a bed set their respawn point after it (`docs/groundwork/n7d-plan.md`); 13
/// since entities made with a chunk have keys of their own above every region's, a chunk's
/// first blocks file follows its entities, and a seat may be granted across a line
/// (`docs/groundwork/n7l-plan.md`, section 4; `n7j-mount-across-a-line.md`).
pub const FORMAT_VERSION: u8 = 13;

/// Why stored bytes could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("the data ends too early")]
    Truncated,
    #[error("format version {0} is not supported")]
    UnsupportedVersion(u8),
    #[error("unknown compression codec {0}")]
    UnknownCodec(u8),
    #[error("the data is corrupt: {0}")]
    Corrupt(&'static str),
    #[error("the checksum does not match")]
    ChecksumMismatch,
}
