// Adapted from SteelMC's `place_random_boolean_selector` in
// `steel-core/src/worldgen/feature/configured.rs`.

//! `random_boolean_selector`: one of two placed features, by a drawn truth value
//! (`RandomBooleanSelectorFeature`).

use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct RandomBooleanSelector {
    if_true: PlacedRef,
    if_false: PlacedRef,
}

impl RandomBooleanSelector {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            if_true: PlacedRef::read(&reader.field("feature_true")?)?,
            if_false: PlacedRef::read(&reader.field("feature_false")?)?,
        })
    }
}

impl Feature for RandomBooleanSelector {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if context.random.next_bool() {
            self.if_true.place(context, origin)
        } else {
            self.if_false.place(context, origin)
        }
    }
}
