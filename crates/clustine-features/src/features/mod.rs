//! The kinds of configured features, a module for each step that builds them
//! (`crate::dispatch`). A kind a step has not built yet is [`Unbuilt`].

use std::fmt;

use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::Feature;
use crate::reader::Reader;

pub mod caves;
pub mod end;
pub mod frame;
pub mod ground;
pub mod nether_and_end;
pub mod plants;
pub mod trees;

/// A kind whose code is not built yet. It reads nothing of its settings, places
/// nothing and says it was placed, as the game's `no_op` does: the judge turns every
/// feature that is not built into `no_op` (the plan, section 3), so the two agree.
pub struct Unbuilt(pub &'static str);

impl Unbuilt {
    pub fn read(kind: &'static str, reader: &Reader) -> Box<dyn Feature> {
        reader.skip_rest();
        Box::new(Unbuilt(kind))
    }
}

impl fmt::Debug for Unbuilt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unbuilt({})", self.0)
    }
}

impl Feature for Unbuilt {
    fn place(&self, _: &mut Context<'_, '_>, _: BlockPos) -> bool {
        true
    }

    fn built(&self) -> bool {
        false
    }
}
