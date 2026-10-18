//! Entities as obstacles to a move and to placing (`EntityGetter.getEntityCollisions`,
//! `Entity.canCollideWith`, `AbstractBoat.canVehicleCollide`, `EntityGetter.isUnobstructed`;
//! `docs/groundwork/n5-brief.md`, finding 18 and section 4.9).
//!
//! A boat is solid to every entity (`canBeCollidedWith` is true of boats alone), and
//! boats and minecarts meet every entity that is solid or pushable, so that a cart stops
//! against a cart or a boat and an item lands on a boat. A unit never meets itself
//! (`isPassengerOfSameVehicle`). What the region knows of players is given apart
//! ([`super::World::player_colliders`]): players are not entities of the region's
//! chunks.

use clustine_data::entity_types;
use clustine_world::{EntityData, EntityId};

use super::aabb::Aabb;

/// A moving entity as the colliders judge it (`canCollideWith`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mover {
    /// Its key: `getEntities` leaves the entity itself out.
    pub(crate) key: EntityId,
    /// Its unit's root, itself where it rides nothing.
    pub(crate) root: EntityId,
    /// Whether it meets what is pushable as well as what is solid (boats and minecarts,
    /// `canVehicleCollide`).
    pub(crate) like_a_vehicle: bool,
}

/// An entity another may meet, as its box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Collider {
    pub(crate) key: EntityId,
    pub(crate) root: EntityId,
    pub(crate) bounds: Aabb,
    /// `canBeCollidedWith`.
    pub(crate) solid: bool,
    /// `isPushable`.
    pub(crate) pushable: bool,
    /// `blocksBuilding`.
    pub(crate) blocks_building: bool,
}

/// Whether the entity type `kind` is a boat or a raft, with a chest or not.
pub(crate) fn is_boat(kind: u16) -> bool {
    let kind = i32::from(kind);
    use entity_types::*;
    matches!(
        kind,
        ACACIA_BOAT
            | ACACIA_CHEST_BOAT
            | BAMBOO_CHEST_RAFT
            | BAMBOO_RAFT
            | BIRCH_BOAT
            | BIRCH_CHEST_BOAT
            | CHERRY_BOAT
            | CHERRY_CHEST_BOAT
            | DARK_OAK_BOAT
            | DARK_OAK_CHEST_BOAT
            | JUNGLE_BOAT
            | JUNGLE_CHEST_BOAT
            | MANGROVE_BOAT
            | MANGROVE_CHEST_BOAT
            | OAK_BOAT
            | OAK_CHEST_BOAT
            | PALE_OAK_BOAT
            | PALE_OAK_CHEST_BOAT
            | POPLAR_BOAT
            | POPLAR_CHEST_BOAT
            | SPRUCE_BOAT
            | SPRUCE_CHEST_BOAT
    )
}

/// Whether the boat type `kind` carries a chest (`AbstractChestBoat`).
pub(crate) fn is_chest_boat(kind: u16) -> bool {
    use entity_types::*;
    matches!(
        i32::from(kind),
        ACACIA_CHEST_BOAT
            | BAMBOO_CHEST_RAFT
            | BIRCH_CHEST_BOAT
            | CHERRY_CHEST_BOAT
            | DARK_OAK_CHEST_BOAT
            | JUNGLE_CHEST_BOAT
            | MANGROVE_CHEST_BOAT
            | OAK_CHEST_BOAT
            | PALE_OAK_CHEST_BOAT
            | POPLAR_CHEST_BOAT
            | SPRUCE_CHEST_BOAT
    )
}

/// Whether the boat type `kind` is a raft (`Raft`, `ChestRaft`).
pub(crate) fn is_raft(kind: u16) -> bool {
    use entity_types::*;
    matches!(i32::from(kind), BAMBOO_RAFT | BAMBOO_CHEST_RAFT)
}

/// Whether the entity type `kind` is a minecart of any kind.
pub(crate) fn is_minecart(kind: u16) -> bool {
    use entity_types::*;
    matches!(
        i32::from(kind),
        MINECART
            | CHEST_MINECART
            | FURNACE_MINECART
            | TNT_MINECART
            | HOPPER_MINECART
            | SPAWNER_MINECART
            | COMMAND_BLOCK_MINECART
    )
}

/// `canBeCollidedWith`: boats alone, of the kinds the region has.
pub(crate) fn is_solid(kind: u16) -> bool {
    is_boat(kind)
}

/// `isPushable`: boats and minecarts (an armour stand never is; living entities that
/// are come with N7).
pub(crate) fn is_pushable(kind: u16) -> bool {
    is_boat(kind) || is_minecart(kind)
}

/// Whether a moving entity of `kind` meets pushable entities too (`canVehicleCollide`).
pub(crate) fn collides_like_a_vehicle(kind: u16) -> bool {
    is_boat(kind) || is_minecart(kind)
}

/// `blocksBuilding`: boats and minecarts set it, and so does every `LivingEntity`, an
/// armour stand among them unless it is a marker (`ArmorStand.onSyncedDataUpdated`).
pub(crate) fn blocks_building(kind: u16, data: &EntityData) -> bool {
    match data {
        EntityData::ArmorStand { flags, .. } => flags & ARMOUR_STAND_MARKER == 0,
        _ => is_boat(kind) || is_minecart(kind),
    }
}

/// An armour stand's flag of being a marker (`DATA_CLIENT_FLAGS`' 16).
const ARMOUR_STAND_MARKER: u8 = 16;

impl Mover {
    /// `canCollideWith(other)`, with `getEntities`' leaving the mover itself out.
    pub(crate) fn can_collide_with(&self, other: &Collider) -> bool {
        if other.key == self.key || other.root == self.root {
            return false;
        }
        if self.like_a_vehicle {
            other.solid || other.pushable
        } else {
            other.solid
        }
    }
}

/// `EntityGetter.getEntityCollisions(mover, bounds)`: the boxes of the entities in
/// `bounds`, grown by 1e-7, that the mover meets, the region's in the order its lookup
/// gives them and then `extra` (the players the region knows); none for a box smaller
/// than 1e-7 across on average.
pub(crate) fn entity_collisions(
    mover: &Mover,
    bounds: &Aabb,
    region: &[Collider],
    extra: &[Collider],
) -> Vec<Aabb> {
    let [x, y, z] = bounds.size();
    if (x + y + z) / 3.0 < 1.0e-7 {
        return Vec::new();
    }
    let reach = bounds.inflate([1.0e-7; 3]);
    region
        .iter()
        .chain(extra)
        .filter(|other| other.bounds.intersects(reach) && mover.can_collide_with(other))
        .map(|other| other.bounds)
        .collect()
}

/// `EntityGetter.isUnobstructed(except, shape)` for a shape of `boxes`: false where an
/// entity that blocks building, of another unit than `except`'s root, overlaps one of
/// them.
pub(crate) fn unobstructed(boxes: &[Aabb], except: Option<EntityId>, near: &[Collider]) -> bool {
    !near.iter().any(|other| {
        other.blocks_building
            && except.is_none_or(|root| root != other.root)
            && boxes.iter().any(|shape| shape.intersects(other.bounds))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collider(key: u64, root: u64, bounds: Aabb, solid: bool, pushable: bool) -> Collider {
        Collider {
            key: EntityId(key),
            root: EntityId(root),
            bounds,
            solid,
            pushable,
            blocks_building: true,
        }
    }

    #[test]
    fn an_item_meets_a_boat_and_not_a_minecart_and_a_cart_meets_both() {
        let boat = collider(2, 2, Aabb::new([0.0; 3], [1.0; 3]), true, true);
        let cart = collider(3, 3, Aabb::new([0.0; 3], [1.0; 3]), false, true);
        let item = Mover {
            key: EntityId(1),
            root: EntityId(1),
            like_a_vehicle: false,
        };
        let moving_cart = Mover {
            like_a_vehicle: true,
            ..item
        };
        assert!(item.can_collide_with(&boat));
        assert!(!item.can_collide_with(&cart));
        assert!(moving_cart.can_collide_with(&boat));
        assert!(moving_cart.can_collide_with(&cart));
    }

    #[test]
    fn a_unit_never_meets_itself() {
        let boat_on_the_cart = collider(2, 3, Aabb::new([0.0; 3], [1.0; 3]), true, true);
        let cart = Mover {
            key: EntityId(3),
            root: EntityId(3),
            like_a_vehicle: true,
        };
        assert!(!cart.can_collide_with(&boat_on_the_cart));
        let bounds = Aabb::new([0.2; 3], [0.8; 3]);
        assert!(entity_collisions(&cart, &bounds, &[boat_on_the_cart], &[]).is_empty());
    }

    #[test]
    fn a_box_that_only_touches_a_boat_still_finds_it_and_a_tiny_box_finds_nothing() {
        let boat = collider(2, 2, Aabb::new([0.0; 3], [1.0; 3]), true, true);
        let item = Mover {
            key: EntityId(1),
            root: EntityId(1),
            like_a_vehicle: false,
        };
        // Grown by 1e-7, a box resting on the boat's top meets it.
        let above = Aabb::new([0.2, 1.0, 0.2], [0.4, 1.2, 0.4]);
        assert_eq!(
            entity_collisions(&item, &above, &[boat], &[]),
            [boat.bounds]
        );
        let tiny = Aabb::new([0.5, 0.5, 0.5], [0.5, 0.5, 0.5]);
        assert!(entity_collisions(&item, &tiny, &[boat], &[]).is_empty());
    }
}
