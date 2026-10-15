//! The kinds of configured features that step F1b of `docs/groundwork/f-plan.md` builds
//! (`crate::dispatch::KINDS`).

use crate::feature::Feature;
use crate::features::Unbuilt;
use crate::reader::{Read, Reader};

/// Reads a configured feature of one of this step's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    Ok(Unbuilt::read(kind, reader))
}
