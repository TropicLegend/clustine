// Adapted from SteelMC's `place_simple_random_selector` in
// `steel-core/src/worldgen/feature/configured.rs`.

//! `simple_random_selector`: one placed feature of a list, drawn evenly and placed
//! (`SimpleRandomSelectorFeature`).

use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct SimpleRandomSelector(Vec<PlacedRef>);

impl SimpleRandomSelector {
    pub fn read(reader: &Reader) -> Read<Self> {
        let features = reader
            .field("features")?
            .one_or_list()
            .iter()
            .map(PlacedRef::read)
            .collect::<Read<Vec<_>>>()?;
        if features.is_empty() {
            return Err(reader.error("a selection from no features"));
        }
        Ok(Self(features))
    }
}

impl Feature for SimpleRandomSelector {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let index = context.random.next_i32_bounded(self.0.len() as i32) as usize;
        self.0[index].place(context, origin)
    }
}
