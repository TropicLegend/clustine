// Adapted from SteelMC's `place_sequence` in
// `steel-core/src/worldgen/feature/configured.rs`.

//! `sequence`: placed features one after another, stopping at the first that is not
//! placed; placed only if all were.

use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Sequence(Vec<PlacedRef>);

impl Sequence {
    pub fn read(reader: &Reader) -> Read<Self> {
        let features = reader
            .field("features")?
            .one_or_list()
            .iter()
            .map(PlacedRef::read)
            .collect::<Read<Vec<_>>>()?;
        Ok(Self(features))
    }
}

impl Feature for Sequence {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        for feature in &self.0 {
            if !feature.place(context, origin) {
                return false;
            }
        }
        true
    }
}
