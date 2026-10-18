//! Item frames, glow item frames and paintings (`HangingEntity`, `ItemFrame`,
//! `Painting`; `docs/groundwork/n5-brief.md`, section 6): the hooks the shared parts call
//! for them, which step N5c fills (`docs/groundwork/n5-plan.md`). Until then a frame or a
//! painting holds wherever it is, drops nothing, has its type's box, ticks only its
//! check's count and is moved by nothing; a painting breaks at a hit as any
//! block-attached entity does.

use clustine_world::{ChunkPos, Entity, EntityData, EntityUse};

use super::Region;
use super::attached::Breaker;
use crate::api::TickOutput;
use crate::physics::aabb::Aabb;

impl Region {
    /// `ItemFrameItem` and `HangingEntityItem.useOn`: a frame or a painting hung on the clicked face (N5c). Filled by its step; until then it does nothing.
    pub(crate) fn place_hanging(
        &mut self,
        click: &crate::placement::Click,
        player: clustine_world::PlayerId,
        output: &mut crate::api::TickOutput,
    ) -> bool {
        let _ = (click, player, output);
        false
    }

    /// `HangingEntity.survives()` (a painting's, and `ItemFrame`'s own): N5c's.
    pub(crate) fn hanging_survives(&self, entity: &Entity, position: ChunkPos) -> bool {
        let _ = (entity, position);
        true
    }

    /// `dropItem(level, breaker)` of a frame or a painting: N5c's.
    pub(crate) fn hanging_drop(
        &mut self,
        entity: &Entity,
        breaker: Breaker,
        output: &mut TickOutput,
    ) {
        let _ = (entity, breaker, output);
    }

    /// The box of a frame or a painting (`calculateBoundingBox`), by its direction and
    /// its variant: N5c's. `None` gives its type's size at its position.
    pub(crate) fn hanging_box(&self, entity: &Entity) -> Option<Aabb> {
        let _ = entity;
        None
    }

    /// A use of a frame or a painting by another region's player: a hit breaks a
    /// painting; a frame's two hits and its item are N5c's.
    pub(crate) fn hanging_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        if let (EntityData::Painting { .. }, EntityUse::Attack { creative, .. }) =
            (&entity.data, by)
        {
            self.hurt_attached(entity.key, Breaker::Player { creative }, output);
        }
    }
}
