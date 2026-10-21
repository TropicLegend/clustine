//! Entities as the physics meet them (`docs/groundwork/n5-brief.md`, finding 18): the
//! boxes of the entities of the region's loaded and copied chunks about a move, each
//! with its unit's root, and whether a box is free of entities that block building, for
//! the placing code (`EntityGetter.isUnobstructed`).
//!
//! An entity's box is its type's size at its position, but where a kind's box is its
//! own (a frame's or a painting's, by its direction and variant), which that kind's step
//! gives ([`Region::attached_box`]). A copy's entities are read as the region's are,
//! never written. Players are not entities of the chunks: the physics are given none
//! yet (a named difference, `docs/groundwork/n5-plan.md`, section 7 item 3).

use clustine_data::EntityTypeInfo;
use clustine_world::{ChunkPos, Entity, EntityId};

use super::Region;
use crate::physics;
use crate::physics::aabb::Aabb;
use crate::physics::entities::{self, Collider, Mover};

/// How many vehicles up a lookup of a root follows at most: a unit is never deeper in
/// play, and a malformed record must not loop.
const DEEPEST_UNIT: usize = 64;

/// The root of the entity `key` among `entities`, the records of the chunk its unit is
/// in: itself where it rides nothing or its vehicle is not there.
pub(crate) fn root_among(entities: &[Entity], key: EntityId) -> EntityId {
    let mut root = key;
    for _ in 0..DEEPEST_UNIT {
        let vehicle = entities
            .binary_search_by_key(&root, |entity| entity.key)
            .ok()
            .and_then(|at| entities[at].riding.vehicle);
        match vehicle {
            Some(vehicle)
                if entities
                    .binary_search_by_key(&vehicle, |entity| entity.key)
                    .is_ok() =>
            {
                root = vehicle;
            }
            _ => break,
        }
    }
    root
}

impl Region {
    /// The box of `entity` (`getBoundingBox`): its type's size at its position, or its
    /// kind's own.
    pub(crate) fn entity_box(&self, entity: &Entity) -> Aabb {
        if let Some(bounds) = self.attached_box(entity) {
            return bounds;
        }
        if let Some((width, height)) = super::stands::own_size(entity) {
            return Aabb::of_entity(entity.position, width, height);
        }
        if let Some(size) = crate::mobs::types::own_size(entity) {
            return Aabb::of_entity(entity.position, size.width, size.height);
        }
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        Aabb::of_entity(entity.position, info.width, info.height)
    }

    /// What `entity` of the chunk `chunk` is to the entities it may meet as it moves.
    pub(crate) fn mover_of(&self, entity: &Entity, chunk: ChunkPos) -> Mover {
        let root = self.readable_chunk(chunk).map_or(entity.key, |loaded| {
            root_among(loaded.entities(), entity.key)
        });
        Mover {
            key: entity.key,
            root,
            like_a_vehicle: entities::collides_like_a_vehicle(entity.kind),
            happy_ghast: i32::from(entity.kind) == clustine_data::entity_types::HAPPY_GHAST,
        }
    }

    /// The entities of the loaded and copied chunks whose boxes meet `bounds`, as
    /// obstacles, in the order of the game's lookups as [`Region::entities_in`] has
    /// them: by chunk, then by section, then by key.
    pub(crate) fn colliders_in(&self, bounds: &Aabb) -> Vec<Collider> {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut found = Vec::new();
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(ChunkPos::new(x, z)) else {
                    continue;
                };
                let records = chunk.entities();
                let mut here: Vec<(i32, Collider)> = records
                    .iter()
                    .filter_map(|entity| {
                        let own = self.entity_box(entity);
                        own.intersects(*bounds).then(|| {
                            let (solid, solid_to_ghasts) = solidity(entity);
                            let collider = Collider {
                                key: entity.key,
                                root: root_among(records, entity.key),
                                bounds: own,
                                solid,
                                solid_to_ghasts,
                                pushable: entities::is_pushable(entity.kind),
                                blocks_building: entities::blocks_building(
                                    entity.kind,
                                    &entity.data,
                                ),
                            };
                            (physics::floor(entity.position[1]) >> 4, collider)
                        })
                    })
                    .collect();
                here.sort_by_key(|(section, collider)| (*section, collider.key));
                found.extend(here.into_iter().map(|(_, collider)| collider));
            }
        }
        found
    }

    /// `EntityGetter.isUnobstructed(except, shape)` for the shape made of `boxes`, as
    /// the placing code asks it (`BlockItem.canPlace`, the brief's finding 18): false
    /// where a boat, a minecart or an armour stand that is no marker overlaps one, of
    /// another unit than `except`'s.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "for the placing code of the steps that place blocks among entities"
        )
    )]
    pub(crate) fn unobstructed_by_entities(
        &self,
        boxes: &[Aabb],
        except: Option<EntityId>,
    ) -> bool {
        let Some(first) = boxes.first() else {
            return true;
        };
        let bounds = boxes.iter().fold(*first, |all, one| all.minmax(*one));
        let except = except.map(|key| self.root_of(key));
        entities::unobstructed(boxes, except, &self.colliders_in(&bounds))
    }
}

/// `canBeCollidedWith(other)` of `entity`, to a mover that is no happy ghast and to one
/// that is: a boat always; a grown happy ghast while it stays still, and to another
/// ghast while it carries riders (`docs/groundwork/n7j-riding-plan.md`, section 9 item
/// 14); nothing else.
fn solidity(entity: &Entity) -> (bool, bool) {
    if i32::from(entity.kind) == clustine_data::entity_types::HAPPY_GHAST
        && let Ok(mob) = crate::mobs::Mob::decode(&entity.data)
    {
        let carries = !entity.riding.passengers.is_empty();
        let ghast = crate::mobs::types::happy_ghast::riding::can_be_collided_with;
        return (ghast(&mob, carries, false), ghast(&mob, carries, true));
    }
    let solid = entities::is_solid(entity.kind);
    (solid, solid)
}
