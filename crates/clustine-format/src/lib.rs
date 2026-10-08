//! The Clustine world format: content-addressed sections, chunk manifests, write-ahead log,
//! and what is kept per region.
//!
//! A chunk is stored as a [`ChunkManifest`] that lists, for each of its sections, the
//! [`Hash`] of that section's canonical encoding. The sections themselves are stored
//! once per distinct content, whichever chunks they appear in.
//!
//! Block state and biome ids are those of one Minecraft data version, which a world
//! records in its metadata. Moving a world to another version means rewriting them.
//!
//! This crate only turns values into bytes and back. Where the bytes are kept is the
//! world store's business.

mod bytes;
mod log;
mod manifest;
mod region;
mod section;

use std::fmt;

pub use log::{LogRecord, Logged, read_log, read_log_with_offsets};
pub use manifest::ChunkManifest;
pub use region::{RegionFile, StateFile};
pub use section::{decode_section, encode_section, pack, unpack};

/// The version of the encodings in this crate. It is written into everything stored.
pub const FORMAT_VERSION: u8 = 1;

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

/// The address of a stored section: the BLAKE3 hash of its canonical encoding.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    /// The address of `canonical`, the output of [`encode_section`].
    pub fn of(canonical: &[u8]) -> Self {
        Self(*blake3::hash(canonical).as_bytes())
    }
}

/// Lower-case hexadecimal, as used for file names.
impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({self})")
    }
}
