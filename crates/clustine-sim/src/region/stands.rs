//! Armour stands (`ArmorStand`, a reduced `LivingEntity`; `docs/groundwork/n5-brief.md`,
//! section 5): the hooks the entities' tick and the uses of entities call for them,
//! which step N5d fills (`docs/groundwork/n5-plan.md`). Until then a stand does only
//! `baseTick`'s riding.

use clustine_world::{ChunkPos, Entity, EntityUse};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `LivingEntity.tick()` of the stand `entity`: N5d's, but `baseTick`'s riding.
    pub(crate) fn tick_armor_stand(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let _ = position;
        self.base_tick_riding(entity.key, output);
    }

    /// A use of a stand by another region's player: N5d's.
    pub(crate) fn armor_stand_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = (entity, position, by, output);
    }
}
