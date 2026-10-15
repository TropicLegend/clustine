// Adapted from SteelMC's `place_weighted_random_selector` in
// `steel-core/src/worldgen/feature/configured.rs`.

//! `weighted_random_selector`: one placed feature of a weighted list, drawn and placed
//! (`WeightedRandomSelectorFeature`). A list of no weight places nothing.

use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::reader::{Read, Reader};
use crate::values::{read_weighted, weighted_index};

#[derive(Debug)]
pub struct WeightedRandomSelector {
    features: Vec<PlacedRef>,
    weights: Vec<i32>,
}

impl WeightedRandomSelector {
    pub fn read(reader: &Reader) -> Read<Self> {
        let (features, weights) = read_weighted(&reader.field("features")?, PlacedRef::read)?;
        Ok(Self { features, weights })
    }
}

impl Feature for WeightedRandomSelector {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        match weighted_index(&self.weights, &mut *context.random) {
            Some(index) => self.features[index].place(context, origin),
            None => false,
        }
    }
}
