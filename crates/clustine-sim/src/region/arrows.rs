//! Arrows, spectral arrows and tridents (`docs/groundwork/n9-plan.md`, sections 4.2 and
//! 4.3; part S2's): their tick in D9 1.1's order, `stepMoveAndHit` and its hits, the
//! ground, `applyEffectsFromBlocks`, pick-up, the trident's loyalty and return. The
//! arithmetic is `crate::physics::arrows`'.
//!
//! Until S2 fills it, nothing makes an arrow and the tick leaves one as it is.

use clustine_world::{ChunkPos, Entity};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// The tick of the arrow or trident `entity` of the loaded chunk `position`
    /// (`AbstractArrow.tick`, `ThrownTrident.tick`); it commits through
    /// `write_projectile`.
    #[expect(
        clippy::unused_self,
        reason = "part S2 fills it (docs/groundwork/n9-plan.md, section 4.2)"
    )]
    pub(crate) fn tick_arrow(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }

    /// An arrow counted in the body of the region's mob `key`, after a hurt that took
    /// (`setArrowCount`, the living value 12; section 4.2).
    #[expect(
        clippy::unused_self,
        reason = "part S2 fills it (docs/groundwork/n9-plan.md, section 4.2)"
    )]
    pub(crate) fn count_arrow(&mut self, _key: clustine_world::EntityId, _output: &mut TickOutput) {
    }
}
