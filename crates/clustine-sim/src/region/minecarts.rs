//! Minecarts (`AbstractMinecart`, `OldMinecartBehavior`; `docs/groundwork/n5-brief.md`,
//! section 3): the hooks the entities' tick and the uses of entities call for them, which
//! step N5e fills (`docs/groundwork/n5-plan.md`). Until then a cart only lets its
//! wobble and damage fall, as `AbstractMinecart.tick` begins, and a hit hurts it as
//! `VehicleEntity` is hurt.

use clustine_world::{ChunkPos, Entity, EntityUse};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `MinecartItem.useOn`: a cart placed on the clicked rail (N5e). Filled by its step; until then it does nothing.
    pub(crate) fn place_minecart(
        &mut self,
        click: &crate::placement::Click,
        player: clustine_world::PlayerId,
        output: &mut crate::api::TickOutput,
    ) -> bool {
        let _ = (click, player, output);
        false
    }

    /// `AbstractMinecart.tick()` of `entity`, in the loaded chunk `position`: N5e's
    /// beyond its first lines. The cart never calls `baseTick`.
    pub(crate) fn tick_minecart(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let _ = position;
        self.decay_hurt(entity.key, output);
    }

    /// A use of a cart by another region's player: a hit (`VehicleEntity.hurtServer`);
    /// the furnace cart's fuel is N5e's, and riding is a mount, the riders' step's.
    pub(crate) fn minecart_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        if let EntityUse::Attack {
            creative, damage, ..
        } = by
        {
            // A sprinting hit's push of a cart that survives it is N5e's.
            self.hurt_vehicle(entity.key, damage.0, creative, output);
        }
    }
}
