//! How the block events a chunk keeps are written, in a chunk's file and in a commit
//! (`docs/adr/0026-redstone.md`, section 4).
//!
//! | Field | Type |
//! |---|---|
//! | Count | u32 |
//! | Per event, in the order of the queue: where | `x << 4 \| z` u8, y i16 |
//! | … the block's id | u16 |
//! | … the kind and the data | i32, i32 |

use clustine_world::{InChunk, KeptEvent};

use crate::FormatError;
use crate::bytes::Input;

/// What one event takes.
const EVENT_LENGTH: usize = 1 + 2 + 2 + 4 + 4;

pub(crate) fn put_events(out: &mut Vec<u8>, events: &[KeptEvent]) {
    out.extend_from_slice(&(events.len() as u32).to_be_bytes());
    for event in events {
        out.push(event.at.x << 4 | event.at.z);
        out.extend_from_slice(&event.at.y.to_be_bytes());
        out.extend_from_slice(&event.block.to_be_bytes());
        out.extend_from_slice(&event.kind.to_be_bytes());
        out.extend_from_slice(&event.data.to_be_bytes());
    }
}

pub(crate) fn take_events(input: &mut Input<'_>) -> Result<Vec<KeptEvent>, FormatError> {
    let count = input.u32()? as usize;
    // A forged count must not reserve more than the input can hold.
    let mut events = Vec::with_capacity(count.min(input.0.len() / EVENT_LENGTH));
    for _ in 0..count {
        let column = input.u8()?;
        let y = input.u16()? as i16;
        let block = input.u16()?;
        let kind = input.i32()?;
        let data = input.i32()?;
        events.push(KeptEvent {
            at: InChunk {
                y,
                z: column & 15,
                x: column >> 4,
            },
            block,
            kind,
            data,
        });
    }
    Ok(events)
}
