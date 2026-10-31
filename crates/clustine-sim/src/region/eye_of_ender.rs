//! The thrown eye of ender (`EyeOfEnder`; `docs/groundwork/p-brief.md`, section 4.3):
//! P10b's (`docs/groundwork/p-plan.md`, sections 19.2, 19.5 and 19.6). Its hooks are
//! fixed by P10-0 so that every shared table calls them; until P10b fills them an eye
//! is left as it is.

use clustine_world::{ChunkPos, Entity};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// The tick of a thrown eye of ender (`EyeOfEnder.tick`): P10b's.
    pub(crate) fn tick_eye_of_ender(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }
}
