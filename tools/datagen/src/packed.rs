//! Writes and reads the head of a packed table (ADR-0019, section 1).
//!
//! A packed file starts with the four bytes `CLT1`, the kind of table (`u16`), the
//! version of its layout (`u16`), the SHA-1 of the jar (20 bytes), the number of
//! sections (`u16`), and for each section its number of rows and the bytes of a row
//! (two `u32`). Everything is little-endian. The sections follow in order with nothing
//! between them, so the head gives the file's length.

use anyhow::{Context, Result, ensure};

pub const MAGIC: [u8; 4] = *b"CLT1";

pub const KIND_BLOCK_STATES: u16 = 1;
pub const KIND_BIOME_PARAMETERS: u16 = 2;

/// The bytes of the head before the list of sections.
const FIXED_HEAD: usize = 30;

/// A section being put together: rows of one length.
pub struct Section {
    row_bytes: usize,
    data: Vec<u8>,
}

impl Section {
    pub fn new(row_bytes: usize) -> Self {
        Self {
            row_bytes,
            data: Vec::new(),
        }
    }

    /// Adds a row, which has to have the section's length.
    pub fn push(&mut self, row: &[u8]) {
        assert_eq!(row.len(), self.row_bytes, "a row of another length");
        self.data.extend_from_slice(row);
    }

    pub fn rows(&self) -> usize {
        self.data.len() / self.row_bytes
    }
}

/// The file of a table of `kind` and `layout` made from the jar with `sha1`.
pub fn file(kind: u16, layout: u16, sha1: &str, sections: &[Section]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&layout.to_le_bytes());
    out.extend_from_slice(&sha1_bytes(sha1)?);
    out.extend_from_slice(&u16::try_from(sections.len())?.to_le_bytes());
    for section in sections {
        out.extend_from_slice(&u32::try_from(section.rows())?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(section.row_bytes)?.to_le_bytes());
    }
    for section in sections {
        out.extend_from_slice(&section.data);
    }
    Ok(out)
}

/// A SHA-1 written as 40 hexadecimal digits, as its 20 bytes.
pub fn sha1_bytes(sha1: &str) -> Result<[u8; 20]> {
    ensure!(
        sha1.len() == 40 && sha1.is_ascii(),
        "{sha1:?} is not a SHA-1 in hexadecimal"
    );
    let mut bytes = [0; 20];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&sha1[2 * index..2 * index + 2], 16)
            .with_context(|| format!("{sha1:?} is not a SHA-1 in hexadecimal"))?;
    }
    Ok(bytes)
}

/// A packed file taken apart again, for `--dump`.
pub struct Parsed<'a> {
    pub kind: u16,
    pub layout: u16,
    pub sha1: [u8; 20],
    pub sections: Vec<ParsedSection<'a>>,
}

pub struct ParsedSection<'a> {
    pub row_bytes: usize,
    pub data: &'a [u8],
}

impl<'a> ParsedSection<'a> {
    pub fn rows(&self) -> impl ExactSizeIterator<Item = &'a [u8]> + use<'a> {
        // A section of rows without bytes has no rows to give.
        self.data.chunks_exact(self.row_bytes.max(1))
    }

    pub fn row(&self, index: usize) -> Result<&'a [u8]> {
        self.rows()
            .nth(index)
            .with_context(|| format!("a row points at row {index} of a shorter section"))
    }
}

pub fn parse(bytes: &[u8]) -> Result<Parsed<'_>> {
    ensure!(
        bytes.len() >= FIXED_HEAD && bytes[..4] == MAGIC,
        "not a packed table: it does not start with CLT1"
    );
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let count = usize::from(u16_at(28));
    let head = FIXED_HEAD + 8 * count;
    ensure!(bytes.len() >= head, "the head of the table is cut short");

    let mut sha1 = [0; 20];
    sha1.copy_from_slice(&bytes[8..28]);
    let mut sections = Vec::new();
    let mut next = head;
    for section in 0..count {
        let entry = FIXED_HEAD + 8 * section;
        let u32_at = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
        };
        let (rows, row_bytes) = (u32_at(entry), u32_at(entry + 4));
        let end = rows
            .checked_mul(row_bytes)
            .and_then(|length| next.checked_add(length))
            .filter(|&end| end <= bytes.len())
            .context("a section reaches past the end of the table")?;
        sections.push(ParsedSection {
            row_bytes,
            data: &bytes[next..end],
        });
        next = end;
    }
    ensure!(
        next == bytes.len(),
        "the sections do not add up to the table's length"
    );
    Ok(Parsed {
        kind: u16_at(4),
        layout: u16_at(6),
        sha1,
        sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1: &str = "000102030405060708090a0b0c0d0e0f10111213";

    #[test]
    fn a_table_that_is_written_reads_back_section_by_section() {
        let mut first = Section::new(3);
        first.push(&[1, 2, 3]);
        first.push(&[4, 5, 6]);
        let mut second = Section::new(1);
        second.push(&[9]);
        let empty = Section::new(5);
        let bytes = file(7, 2, SHA1, &[first, second, empty]).unwrap();

        assert_eq!(&bytes[..4], b"CLT1");
        assert_eq!(bytes.len(), 30 + 3 * 8 + 6 + 1);
        let parsed = parse(&bytes).unwrap();
        assert_eq!((parsed.kind, parsed.layout), (7, 2));
        assert_eq!(parsed.sha1[19], 0x13);
        assert_eq!(parsed.sections.len(), 3);
        let rows: Vec<&[u8]> = parsed.sections[0].rows().collect();
        assert_eq!(rows, [&[1, 2, 3][..], &[4, 5, 6][..]]);
        assert_eq!(parsed.sections[1].row(0).unwrap(), [9]);
        assert_eq!(parsed.sections[2].rows().len(), 0);
        assert!(parsed.sections[1].row(1).is_err());
    }

    #[test]
    fn a_table_that_is_cut_short_or_too_long_is_refused() {
        let mut section = Section::new(2);
        section.push(&[1, 2]);
        let bytes = file(1, 1, SHA1, &[section]).unwrap();
        assert!(parse(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(parse(&longer).is_err());
        let mut other = bytes;
        other[0] = b'X';
        assert!(parse(&other).is_err());
    }

    #[test]
    fn a_checksum_that_is_not_one_is_refused() {
        assert!(sha1_bytes("abc").is_err());
        assert!(sha1_bytes(&"zz".repeat(20)).is_err());
    }
}
