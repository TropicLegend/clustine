//! The llama's spit: its flight and its hit (`docs/groundwork/n9-plan.md`, section 4.4;
//! part S3's).
//!
//! Until S3 fills it, nothing makes one and the tick leaves one as it is.

use clustine_world::{ChunkPos, Entity};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// The tick of the spit `entity` of the loaded chunk `position` (`LlamaSpit.tick`); it
    /// commits through `write_projectile`.
    #[expect(
        clippy::unused_self,
        reason = "part S3 fills it (docs/groundwork/n9-plan.md, section 4.4)"
    )]
    pub(crate) fn tick_spit(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }
}
