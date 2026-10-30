//! A player's hit on, or use of, an entity (`ServerGamePacketListenerImpl.handleAttack`
//! and `handleInteract`, `docs/groundwork/n5-brief.md`, sections 2.7 and 2.8): judged
//! within reach where the region has the entity, done there by its kind's hook, or sent
//! as an effect on the entity to the region that uses its chunk where the region has it
//! only in a copy (`docs/groundwork/n5-plan.md`, decision 10 and section 7, item 9).

use clustine_world::{BlockPos, ChunkPos, Double, Effect, Effected, Entity, EntityId, EntityUse};

use super::Region;
use crate::api::TickOutput;
use crate::physics::aabb::Aabb;

/// A creative player's reach to an entity: `entity_interaction_range` (3), the creative
/// modifier's 2, and the 3 the handlers add, from the eyes to the entity's box.
pub(super) const CREATIVE_ENTITY_REACH: f64 = 3.0 + 2.0 + 3.0;

/// The eyes' height of a player standing, and sneaking (`Pose.CROUCHING`).
const STANDING_EYES: f64 = 1.62;
const SNEAKING_EYES: f64 = 1.27;

/// `AABB.distanceToSqr(point)`: the square of the distance from `point` to the nearest
/// point of `bounds`.
pub(super) fn distance_to_sqr(bounds: Aabb, point: [f64; 3]) -> f64 {
    let mut squared = 0.0;
    for (axis, at) in point.into_iter().enumerate() {
        let off = (bounds.min[axis] - at).max(at - bounds.max[axis]).max(0.0);
        squared += off * off;
    }
    squared
}

/// Where a player stands and looks from, for a use.
#[derive(Debug, Clone, Copy)]
pub(crate) struct User {
    pub(crate) feet: [f64; 3],
    pub(crate) sneaking: bool,
}

impl User {
    pub(super) fn eyes(&self) -> [f64; 3] {
        let height = if self.sneaking {
            SNEAKING_EYES
        } else {
            STANDING_EYES
        };
        [self.feet[0], self.feet[1] + height, self.feet[2]]
    }

    pub(crate) fn from(&self) -> [Double; 3] {
        self.feet.map(Double)
    }
}

impl Region {
    /// Does what the player `user` did to the entity `key`: where the region has the
    /// entity, by its kind's hook; where it has it only in the copy of a chunk another
    /// region uses, by an effect on the entity there. One out of reach, or that the
    /// region neither has nor sees, is passed over, as the game passes over an entity it
    /// does not find; nothing is sent back either way.
    pub(crate) fn player_used_entity(
        &mut self,
        user: User,
        key: EntityId,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let reach = CREATIVE_ENTITY_REACH * CREATIVE_ENTITY_REACH;
        if let Some((_, entity)) = self.find_entity(key) {
            if distance_to_sqr(self.entity_box(entity), user.eyes()) < reach {
                self.entity_used(key, by, output);
            }
            return;
        }
        let Some((chunk, entity)) = self.copied_entity(key) else {
            return;
        };
        if distance_to_sqr(self.entity_box(&entity), user.eyes()) >= reach || !self.sends_to(chunk)
        {
            return;
        }
        self.made.push(Effect {
            at: block_in(chunk, &entity),
            what: Effected::OnEntity { key, by },
        });
    }

    /// The entity `key` as the copy of its chunk has it, with that chunk.
    pub(super) fn copied_entity(&self, key: EntityId) -> Option<(ChunkPos, Entity)> {
        self.copies.iter().find_map(|(position, copy)| {
            let entities = copy.entities();
            let at = entities
                .binary_search_by_key(&key, |entity| entity.key)
                .ok()?;
            Some((*position, entities[at].clone()))
        })
    }
}

/// The block an effect on `entity` of the chunk `chunk` is addressed to: its own, or for
/// a passenger, whose position may be in another chunk, the block of its unit's chunk
/// below it, as the store files an effect by its block's chunk.
pub(super) fn block_in(chunk: ChunkPos, entity: &Entity) -> BlockPos {
    let at = BlockPos::new(
        entity.position[0].floor() as i32,
        entity.position[1].floor() as i32,
        entity.position[2].floor() as i32,
    );
    if at.chunk() == chunk {
        return at;
    }
    BlockPos::new(chunk.x * 16, at.y, chunk.z * 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_distance_to_a_box_is_to_its_nearest_point_and_none_inside_it() {
        let bounds = Aabb::new([0.0, 0.0, 0.0], [1.0, 2.0, 1.0]);
        assert_eq!(distance_to_sqr(bounds, [0.5, 1.0, 0.5]), 0.0);
        assert_eq!(distance_to_sqr(bounds, [3.0, 1.0, 0.5]), 4.0);
        assert_eq!(distance_to_sqr(bounds, [-1.0, 3.0, 0.5]), 2.0);
    }

    #[test]
    fn a_passengers_effect_goes_to_a_block_of_its_units_chunk() {
        let mut entity = Entity {
            portal: Default::default(),
            key: EntityId(1),
            kind: 71,
            uuid: uuid::Uuid::from_u128(1),
            position: [33.5, 64.0, 2.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: clustine_world::EntityData::Painting {
                attached: clustine_world::Attached {
                    block: BlockPos::new(33, 64, 2),
                    direction: 2,
                },
                variant: 0,
            },
        };
        assert_eq!(
            block_in(ChunkPos::new(2, 0), &entity),
            BlockPos::new(33, 64, 2)
        );
        entity.position = [15.9, 64.5, 2.5];
        assert_eq!(
            block_in(ChunkPos::new(1, 0), &entity),
            BlockPos::new(16, 64, 0)
        );
    }
}
