// Adapted from SteelMC's `place_overlay` in
// `steel-core/src/worldgen/feature/configured.rs`.

//! `overlay`: every placed feature of a list at the same position, each whether the
//! one before was placed or not; placed if any was.

use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Overlay(Vec<PlacedRef>);

impl Overlay {
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

impl Feature for Overlay {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut placed = false;
        for feature in &self.0 {
            placed |= feature.place(context, origin);
        }
        placed
    }
}
