//! The head of a packed table, as `docs/adr/0019-data-made-from-mojangs-jar.md` lays it
//! out, and the checks a crate makes on a table it takes in with `include_bytes!`.
//!
//! A packed file starts with the four bytes `CLT1`, the kind of table (`u16`), the
//! version of its layout (`u16`), the SHA-1 of the server jar it was made from (20
//! bytes), the number of sections (`u16`), and for each section its number of rows and
//! the bytes of a row (two `u32`). Everything is little-endian. The sections follow in
//! order with nothing between them, so the head gives the file's length.
//!
//! [`Table::parse`] is a constant function and panics on a table that is not what the
//! code expects, so a table of another jar, another layout or another length does not
//! compile.

use crate::generated::version::SERVER_JAR_SHA1_BYTES;

/// The bytes every packed table starts with.
pub const MAGIC: [u8; 4] = *b"CLT1";

/// The kind of [`crate::block_states`]' table.
pub const KIND_BLOCK_STATES: u16 = 1;
/// The kind of [`crate::biome_parameters`]' table.
pub const KIND_BIOME_PARAMETERS: u16 = 2;

/// The bytes of the head before the list of sections.
const FIXED_HEAD: usize = 30;

/// A packed table with `N` sections, its head read and checked.
pub(crate) struct Table<const N: usize> {
    bytes: &'static [u8],
    /// Where each section begins in `bytes`.
    starts: [usize; N],
    rows: [usize; N],
    row_bytes: [usize; N],
}

impl<const N: usize> Table<N> {
    /// Reads the head of `bytes` and panics unless it is a table of this `kind` and
    /// `layout`, made from the jar `generated/version.rs` names, with `N` sections
    /// whose rows have `row_bytes` bytes and whose lengths add up to the file's.
    pub(crate) const fn parse(
        bytes: &'static [u8],
        kind: u16,
        layout: u16,
        row_bytes: [usize; N],
    ) -> Self {
        assert!(
            bytes.len() >= FIXED_HEAD + 8 * N,
            "a packed table is cut short"
        );
        assert!(
            bytes[0] == MAGIC[0]
                && bytes[1] == MAGIC[1]
                && bytes[2] == MAGIC[2]
                && bytes[3] == MAGIC[3],
            "a packed table does not start with CLT1"
        );
        assert!(
            u16_at(bytes, 4) == kind,
            "a packed table is of another kind"
        );
        assert!(
            u16_at(bytes, 6) == layout,
            "a packed table has another layout than this code reads"
        );
        let mut i = 0;
        while i < 20 {
            assert!(
                bytes[8 + i] == SERVER_JAR_SHA1_BYTES[i],
                "a packed table was made from another jar than version.rs names"
            );
            i += 1;
        }
        assert!(
            u16_at(bytes, 28) as usize == N,
            "a packed table has another number of sections"
        );

        let mut starts = [0; N];
        let mut rows = [0; N];
        let mut next = FIXED_HEAD + 8 * N;
        let mut section = 0;
        while section < N {
            let entry = FIXED_HEAD + 8 * section;
            rows[section] = u32_at(bytes, entry) as usize;
            assert!(
                u32_at(bytes, entry + 4) as usize == row_bytes[section],
                "a section of a packed table has rows of another length"
            );
            starts[section] = next;
            next += rows[section] * row_bytes[section];
            section += 1;
        }
        assert!(
            next == bytes.len(),
            "the sections of a packed table do not add up to its length"
        );
        Self {
            bytes,
            starts,
            rows,
            row_bytes,
        }
    }

    /// The number of rows of `section`.
    pub(crate) const fn rows(&self, section: usize) -> usize {
        self.rows[section]
    }

    /// Row `row` of `section`. Panics if the section has no such row.
    pub(crate) fn row(&self, section: usize, row: usize) -> &'static [u8] {
        assert!(row < self.rows[section], "no such row in a packed table");
        let length = self.row_bytes[section];
        let start = self.starts[section] + row * length;
        &self.bytes[start..start + length]
    }
}

const fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

const fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The `u16` at `at` of a row.
pub(crate) fn row_u16(row: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([row[at], row[at + 1]])
}

/// The `i16` at `at` of a row.
pub(crate) fn row_i16(row: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([row[at], row[at + 1]])
}

/// The `u32` at `at` of a row.
pub(crate) fn row_u32(row: &[u8], at: usize) -> u32 {
    u32_at(row, at)
}

/// The `f64` whose bits are at `at` of a row.
pub(crate) fn row_f64(row: &[u8], at: usize) -> f64 {
    let mut bits = [0; 8];
    bits.copy_from_slice(&row[at..at + 8]);
    f64::from_le_bytes(bits)
}
