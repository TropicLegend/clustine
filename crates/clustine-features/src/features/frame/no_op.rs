//! `no_op`: places nothing and says it was placed (`NoOpFeature`).

use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::Feature;

#[derive(Debug)]
pub struct NoOp;

impl Feature for NoOp {
    fn place(&self, _: &mut Context<'_, '_>, _: BlockPos) -> bool {
        true
    }
}
