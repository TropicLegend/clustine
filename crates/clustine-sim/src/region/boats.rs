//! Boats and rafts (`AbstractBoat`; `docs/groundwork/n5-brief.md`, section 4): the hooks
//! the entities' tick and the uses of entities call for them, which step N5f fills
//! (`docs/groundwork/n5-plan.md`). Until then a boat lets its wobble and damage fall and
//! does `baseTick`'s riding (it gets off a vehicle that is gone, its boarding cooldown
//! counts down), and a hit hurts it as `VehicleEntity` is hurt.

use clustine_world::{ChunkPos, Entity, EntityUse};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `BoatItem.use`: a boat placed where the player's ray meets water or a block (N5f). Filled by its step; until then it does nothing.
    pub(crate) fn place_boat(
        &mut self,
        used: &crate::placement::ItemUse,
        player: clustine_world::PlayerId,
        output: &mut crate::api::TickOutput,
    ) -> bool {
        let _ = (used, player, output);
        false
    }

    /// `AbstractBoat.tick()` of `entity`, in the loaded chunk `position`: N5f's beyond
    /// its fourth step, the wobble's fall, and its `baseTick`'s riding.
    pub(crate) fn tick_boat(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let _ = position;
        self.decay_hurt(entity.key, output);
        self.base_tick_riding(entity.key, output);
    }

    /// A use of a boat by another region's player: a hit (`VehicleEntity.hurtServer`);
    /// a lead is N5g's, and riding is a mount, the riders' step's.
    pub(crate) fn boat_used(
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
            self.hurt_vehicle(entity.key, damage.0, creative, output);
        }
    }
}
