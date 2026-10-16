//! How a chunk's seam entries are written, in a chunk file and in a commit with records
//! of chunks (`docs/adr/0024-block-behaviour-in-a-region.md`, section 5): a count (u32),
//! then per entry the block it reaches as i32 x, i32 y and i32 z, the direction u8 (0 to
//! 5, down, up, north, south, west, east), the kind u8 (0 `updateShape`, 1
//! `neighborChanged`), the block that changed u16, the flags u32 and the depth i32, in
//! the order of their keys, each key once.

use clustine_data::{BLOCK_STATE_COUNT, BlockState};
use clustine_world::{BlockPos, Seam, SeamKind};

use crate::FormatError;
use crate::bytes::Input;

pub(crate) fn put_seams(out: &mut Vec<u8>, seams: &[Seam]) {
    out.extend_from_slice(&(seams.len() as u32).to_be_bytes());
    for seam in seams {
        out.extend_from_slice(&seam.at.x.to_be_bytes());
        out.extend_from_slice(&seam.at.y.to_be_bytes());
        out.extend_from_slice(&seam.at.z.to_be_bytes());
        out.push(seam.direction);
        out.push(match seam.kind {
            SeamKind::Shape => 0,
            SeamKind::Neighbour => 1,
        });
        out.extend_from_slice(&seam.source.0.to_be_bytes());
        out.extend_from_slice(&seam.flags.to_be_bytes());
        out.extend_from_slice(&seam.depth.to_be_bytes());
    }
}

pub(crate) fn take_seams(input: &mut Input<'_>) -> Result<Vec<Seam>, FormatError> {
    let count = input.u32()? as usize;
    // Each entry is 23 bytes: no count can promise more than the input holds.
    let mut seams = Vec::with_capacity(count.min(input.0.len() / 23));
    for _ in 0..count {
        let at = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
        let direction = input.u8()?;
        if direction > 5 {
            return Err(FormatError::Corrupt("a seam's direction"));
        }
        let kind = match input.u8()? {
            0 => SeamKind::Shape,
            1 => SeamKind::Neighbour,
            _ => return Err(FormatError::Corrupt("a seam's kind")),
        };
        let source = input.u16()?;
        if u32::from(source) >= BLOCK_STATE_COUNT {
            return Err(FormatError::Corrupt("a seam's block"));
        }
        let seam = Seam {
            at,
            direction,
            kind,
            source: BlockState(source),
            flags: input.u32()?,
            depth: input.i32()?,
        };
        if seams
            .last()
            .is_some_and(|last: &Seam| last.key() >= seam.key())
        {
            return Err(FormatError::Corrupt("seams out of order"));
        }
        seams.push(seam);
    }
    Ok(seams)
}
