//! What item frames, paintings, cushions and leash knots share (`BlockAttachedEntity`,
//! `docs/groundwork/n5-brief.md`, sections 5.8 and 6.1, findings 19 and 33): a tick that
//! is no `Entity.tick` (no gravity, no fluids) and checks every 101 ticks that the entity
//! still holds where it is, a hit of anything breaking it, and any move or push of it
//! breaking it too (a piston pops a painting).
//!
//! The counter of the check is the game's `ticksSinceLastCheck`, which it does not save:
//! the region keeps it in its memory of the entity, which starts again at a take-in as
//! the game's starts again at a load (`docs/groundwork/n5-plan.md`, section 7 item 7).
//! What each kind holds to and drops are its own (`cushions.rs`, `hanging.rs`,
//! `knots.rs`), which the steps that bring them fill.

use clustine_world::{ChunkPos, Entity, EntityData, EntityId};

use super::Region;
use crate::api::TickOutput;
use crate::physics::aabb::Aabb;

/// The ticks between two checks less one (`CHECK_INTERVAL`): the check runs when the
/// count before the tick has reached it.
const CHECK_INTERVAL: u32 = 100;

/// Who broke a block-attached entity, as its drop asks (`dropItem(level, breaker)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Breaker {
    /// Nothing: its support went, it moved, a push.
    Nothing,
    /// A player, creative or not (a creative player's break drops nothing).
    Player { creative: bool },
}

/// Whether the record is of a block-attached entity.
pub(crate) fn is_block_attached(data: &EntityData) -> bool {
    matches!(
        data,
        EntityData::ItemFrame { .. }
            | EntityData::Painting { .. }
            | EntityData::Cushion { .. }
            | EntityData::LeashKnot { .. }
    )
}

impl Region {
    /// `BlockAttachedEntity.tick()` of `entity`, in the loaded chunk `position`: below
    /// the world it is discarded; otherwise its count goes on, and on the 101st tick
    /// since the last check (or its take-in) the kind's own check runs, and one that no
    /// longer holds is discarded and drops its item.
    pub(crate) fn tick_block_attached(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let min_y = self
            .chunks
            .get(&position)
            .map_or(-64, |chunk| chunk.min_y());
        if entity.position[1] < f64::from(min_y - 64) {
            // `onBelowWorld`: discarded, dropping nothing.
            self.remove_attached(entity.key, output);
            return;
        }
        let memory = self.entity_memory.entry(entity.key).or_default();
        let before = memory.ticks_since_check;
        memory.ticks_since_check = before.saturating_add(1);
        if before < CHECK_INTERVAL {
            return;
        }
        memory.ticks_since_check = 0;
        self.attached_check_interval(&entity, position, output);
        let still = self
            .find_entity(entity.key)
            .map(|(_, record)| record.clone());
        if let Some(record) = still
            && !self.attached_survives(&record, position)
        {
            self.remove_attached(record.key, output);
            self.attached_drop(&record, Breaker::Nothing, output);
        }
    }

    /// `BlockAttachedEntity.hurtServer` of the entity `key`, hit by `breaker`: the
    /// amount does not matter, one hit of anything kills it, and it drops its item as
    /// its kind does. False where there is no such entity.
    pub(crate) fn hurt_attached(
        &mut self,
        key: EntityId,
        breaker: Breaker,
        output: &mut TickOutput,
    ) -> bool {
        let Some((_, record)) = self.find_entity(key) else {
            return false;
        };
        let record = record.clone();
        // `kill`, then `markHurt` (nothing to send of a removed entity), then the drop.
        self.remove_attached(key, output);
        self.attached_drop(&record, breaker, output);
        true
    }

    /// `BlockAttachedEntity.move(type, vector)` and `push(dx, dy, dz)`: any move or push
    /// of length above 0 kills the entity `key` and drops its item, as if broken by
    /// nothing. True where it broke.
    pub(crate) fn moved_attached(
        &mut self,
        key: EntityId,
        by: [f64; 3],
        output: &mut TickOutput,
    ) -> bool {
        let [x, y, z] = by;
        if x * x + y * y + z * z <= 0.0 {
            return false;
        }
        self.hurt_attached(key, Breaker::Nothing, output)
    }

    /// `Entity.push(dx, dy, dz)` of the entity `key`, as carts and boats push what they
    /// meet: a block-attached entity breaks; anything else has the push added to its
    /// motion where all of it is finite (`needsSync`, which the next report of the
    /// entity carries), committed.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "for the pushes of carts and boats, N5e and N5f")
    )]
    pub(crate) fn push_entity(&mut self, key: EntityId, by: [f64; 3], output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        if is_block_attached(&record.data) {
            self.moved_attached(key, by, output);
            return;
        }
        if !by.iter().all(|component| component.is_finite()) {
            return;
        }
        let motion = crate::physics::add(record.motion, by);
        self.commit_entity(
            chunk,
            key,
            vec![clustine_world::EntityField::Motion(motion)],
            output,
        );
    }

    /// The record of the block-attached entity `key` goes, its passengers getting off,
    /// and the edges are told.
    fn remove_attached(&mut self, key: EntityId, output: &mut TickOutput) {
        self.unride_removed(key, true, output);
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let hops = record.hops;
        self.remove_entity(key, hops, chunk, output);
    }

    /// `tickAtCheckInterval()` of the kind (a cushion's fire and fluids; nothing for the
    /// others).
    fn attached_check_interval(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        if let EntityData::Cushion { .. } = entity.data {
            self.cushion_check_interval(entity, position, output);
        }
    }

    /// `survives()` of the kind.
    fn attached_survives(&self, entity: &Entity, position: ChunkPos) -> bool {
        match entity.data {
            EntityData::Cushion { .. } => self.cushion_survives(entity, position),
            EntityData::ItemFrame { .. } | EntityData::Painting { .. } => {
                self.hanging_survives(entity, position)
            }
            EntityData::LeashKnot { .. } => self.knot_survives(entity, position),
            _ => true,
        }
    }

    /// `dropItem(level, breaker)` of the kind.
    pub(crate) fn attached_drop(
        &mut self,
        entity: &Entity,
        breaker: Breaker,
        output: &mut TickOutput,
    ) {
        match entity.data {
            EntityData::Cushion { .. } => self.cushion_drop(entity, breaker, output),
            EntityData::ItemFrame { .. } | EntityData::Painting { .. } => {
                self.hanging_drop(entity, breaker, output);
            }
            EntityData::LeashKnot { .. } => self.knot_drop(entity, breaker, output),
            _ => {}
        }
    }

    /// The box of a kind whose box is its own rather than its type's size at its
    /// position: a frame's or a painting's, by its direction and variant.
    pub(crate) fn attached_box(&self, entity: &Entity) -> Option<Aabb> {
        match entity.data {
            EntityData::ItemFrame { .. } | EntityData::Painting { .. } => self.hanging_box(entity),
            _ => None,
        }
    }
}
