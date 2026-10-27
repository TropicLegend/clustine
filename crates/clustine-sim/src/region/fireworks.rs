//! Firework rockets: their construction, flight and explosion
//! (`docs/groundwork/n9-plan.md`, section 4.7; part S3's).
//!
//! Until S3 fills it, nothing makes one and the tick leaves one as it is.

use clustine_world::{ChunkPos, Entity};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// The tick of the firework rocket `entity` of the loaded chunk `position`
    /// (`FireworkRocketEntity.tick`); it commits through `write_projectile`.
    #[expect(
        clippy::unused_self,
        reason = "part S3 fills it (docs/groundwork/n9-plan.md, section 4.7)"
    )]
    pub(crate) fn tick_firework(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }
}
