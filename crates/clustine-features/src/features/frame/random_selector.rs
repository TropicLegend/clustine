// Adapted from SteelMC's `place_random_selector` in
// `steel-core/src/worldgen/feature/configured.rs`.

//! `random_selector`: each of a list of placed features in turn with its chance, the
//! first that comes up placed; the default where none does (`RandomSelectorFeature`).

use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct RandomSelector {
    features: Vec<(f32, PlacedRef)>,
    default: PlacedRef,
}

impl RandomSelector {
    pub fn read(reader: &Reader) -> Read<Self> {
        let mut features = Vec::new();
        for entry in reader.field("features")?.list()? {
            features.push((
                entry.field("chance")?.float()?,
                PlacedRef::read(&entry.field("feature")?)?,
            ));
            entry.finish()?;
        }
        Ok(Self {
            features,
            default: PlacedRef::read(&reader.field("default")?)?,
        })
    }
}

impl Feature for RandomSelector {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        for (chance, feature) in &self.features {
            if context.random.next_f32() < *chance {
                return feature.place(context, origin);
            }
        }
        self.default.place(context, origin)
    }
}
