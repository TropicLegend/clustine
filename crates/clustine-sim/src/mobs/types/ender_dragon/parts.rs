//! The dragon's eight parts (`EnderDragonPart`; d EnderDragon.txt 65–263 and
//! EnderDragonPart.txt): head, neck, body, three of tail and two wings, in the order of
//! `subEntities`, which is the order of their ids on the wire (`dragonId + 1 … + 8`,
//! `recreateFromPacket`) and of the part an attack names. Each is a box of its own size
//! at a place `aiStep` sets each tick (`tickPart`), kept in the dragon's state.

use crate::physics::Vec3;
use crate::physics::aabb::Aabb;

/// The parts by their index.
pub(crate) const HEAD: u8 = 0;
pub(crate) const NECK: u8 = 1;
pub(crate) const BODY: u8 = 2;
pub(crate) const TAIL: [u8; 3] = [3, 4, 5];
pub(crate) const WING: [u8; 2] = [6, 7];

/// How many parts a dragon has: the ids after its own on the wire.
pub(crate) const COUNT: u8 = 8;

/// `EntityDimensions.scalable(width, height)` of each part, in the order of the parts.
pub(crate) const SIZES: [(f32, f32); COUNT as usize] = [
    (1.0, 1.0),
    (3.0, 3.0),
    (5.0, 3.0),
    (2.0, 2.0),
    (2.0, 2.0),
    (2.0, 2.0),
    (4.0, 2.0),
    (4.0, 2.0),
];

/// The box of the part `part` standing at `at` (`makeBoundingBox`).
pub(crate) fn box_of(part: u8, at: Vec3) -> Aabb {
    let (width, height) = SIZES[usize::from(part % COUNT)];
    Aabb::of_entity(at, width, height)
}
