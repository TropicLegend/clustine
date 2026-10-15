// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/leaves.rs`.

//! The last step of a tree (`TreeFeature.updateLeaves` and
//! `StructureTemplate.updateShapeAtEdge`): the leaves are given their distance from
//! the logs by a walk out from them, all that is reached makes a shape over the
//! tree's box, and the blocks across that shape's faces are updated.

use clustine_data::Face;
use clustine_world::BlockPos;

use super::common::Placement;
use super::shape::{leaf_distance, update_shape};
use crate::blocks::relative;
use crate::context::Level;
use crate::java_set::JavaSet;

/// Leaves this far from a log or further are not reached.
const MAX_DISTANCE: usize = 7;

/// A box of blocks, both corners included (`BoundingBox`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub min: BlockPos,
    pub max: BlockPos,
}

impl Bounds {
    /// `BoundingBox.encapsulatingPositions`: the box around every position of a
    /// tree, if it has any.
    pub fn around(placement: &Placement) -> Option<Self> {
        let mut positions = placement
            .roots
            .iter()
            .chain(placement.trunks.iter())
            .chain(placement.foliage.iter())
            .chain(placement.decorations.iter());
        let first = positions.next()?;
        let mut bounds = Bounds {
            min: first,
            max: first,
        };
        for pos in positions {
            bounds.min = BlockPos::new(
                bounds.min.x.min(pos.x),
                bounds.min.y.min(pos.y),
                bounds.min.z.min(pos.z),
            );
            bounds.max = BlockPos::new(
                bounds.max.x.max(pos.x),
                bounds.max.y.max(pos.y),
                bounds.max.z.max(pos.z),
            );
        }
        Some(bounds)
    }

    pub fn contains(&self, pos: BlockPos) -> bool {
        (self.min.x..=self.max.x).contains(&pos.x)
            && (self.min.y..=self.max.y).contains(&pos.y)
            && (self.min.z..=self.max.z).contains(&pos.z)
    }

    fn span(&self) -> (usize, usize, usize) {
        (
            (self.max.x - self.min.x + 1) as usize,
            (self.max.y - self.min.y + 1) as usize,
            (self.max.z - self.min.z + 1) as usize,
        )
    }
}

/// The blocks of the box that are filled (`BitSetDiscreteVoxelShape`).
pub struct Shape {
    bounds: Bounds,
    full: Vec<bool>,
}

impl Shape {
    fn new(bounds: Bounds) -> Self {
        let (x, y, z) = bounds.span();
        Self {
            bounds,
            full: vec![false; x * y * z],
        }
    }

    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        let (_, sy, sz) = self.bounds.span();
        (x * sy + y) * sz + z
    }

    fn local(&self, pos: BlockPos) -> (usize, usize, usize) {
        (
            (pos.x - self.bounds.min.x) as usize,
            (pos.y - self.bounds.min.y) as usize,
            (pos.z - self.bounds.min.z) as usize,
        )
    }

    fn fill(&mut self, pos: BlockPos) {
        let (x, y, z) = self.local(pos);
        let index = self.index(x, y, z);
        self.full[index] = true;
    }

    pub fn is_full(&self, pos: BlockPos) -> bool {
        if !self.bounds.contains(pos) {
            return false;
        }
        let (x, y, z) = self.local(pos);
        self.full[self.index(x, y, z)]
    }
}

/// `TreeFeature.updateLeaves`: from the logs out, each leaf reached is given the
/// least distance it is found at, in the order of the game's sets; the shape holds
/// the logs, the leaves reached, the decorations and the roots.
pub fn update_leaves(level: &mut Level<'_>, bounds: Bounds, placement: &Placement) -> Shape {
    let mut shape = Shape::new(bounds);
    for pos in placement.decorations.iter().chain(placement.roots.iter()) {
        if bounds.contains(pos) {
            shape.fill(pos);
        }
    }
    let mut to_check: [JavaSet; MAX_DISTANCE] = Default::default();
    for pos in placement.trunks.iter() {
        to_check[0].insert(pos);
    }
    let mut smallest = 0;
    loop {
        while smallest >= MAX_DISTANCE || !to_check[smallest].is_empty() {
            if smallest >= MAX_DISTANCE {
                return shape;
            }
            let pos = to_check[smallest]
                .iter()
                .next()
                .expect("the set is not empty");
            to_check[smallest].remove(pos);
            if !bounds.contains(pos) {
                continue;
            }
            if smallest != 0 {
                let state = level.block(pos);
                if let Some(updated) = state.with_property("distance", &smallest.to_string()) {
                    level.set_block_known_shape(pos, updated);
                }
            }
            shape.fill(pos);
            for face in Face::ALL {
                let neighbour = relative(pos, face);
                if !bounds.contains(neighbour) || shape.is_full(neighbour) {
                    continue;
                }
                let Some(distance) = leaf_distance(level.block(neighbour)) else {
                    continue;
                };
                let distance = (distance as usize).min(smallest + 1);
                if distance < MAX_DISTANCE {
                    to_check[distance].insert(neighbour);
                    smallest = smallest.min(distance);
                }
            }
        }
        smallest += 1;
    }
}

/// `StructureTemplate.updateShapeAtEdge` with the shape over the tree's box: across
/// each face of the shape, the block inside and then the one outside are updated for
/// each other. The faces come along z, then along y, then along x, as
/// `DiscreteVoxelShape.forAllFaces` gives them.
pub fn update_edges(level: &mut Level<'_>, shape: &Shape) {
    let Bounds { min, max } = shape.bounds;
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            line(
                level,
                shape,
                min.z,
                max.z,
                |z| BlockPos::new(x, y, z),
                Face::North,
                Face::South,
            );
        }
    }
    for z in min.z..=max.z {
        for x in min.x..=max.x {
            line(
                level,
                shape,
                min.y,
                max.y,
                |y| BlockPos::new(x, y, z),
                Face::Down,
                Face::Up,
            );
        }
    }
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            line(
                level,
                shape,
                min.x,
                max.x,
                |x| BlockPos::new(x, y, z),
                Face::West,
                Face::East,
            );
        }
    }
}

/// The faces along one line of the box: where the shape starts and where it ends.
fn line(
    level: &mut Level<'_>,
    shape: &Shape,
    start: i32,
    end: i32,
    at: impl Fn(i32) -> BlockPos,
    negative: Face,
    positive: Face,
) {
    let mut last_full = false;
    for cursor in start..=end + 1 {
        let full = cursor != end + 1 && shape.is_full(at(cursor));
        if !last_full && full {
            face(level, at(cursor), negative);
        }
        if last_full && !full {
            face(level, at(cursor - 1), positive);
        }
        last_full = full;
    }
}

/// The two blocks across one face, each updated for the other.
fn face(level: &mut Level<'_>, pos: BlockPos, direction: Face) {
    let neighbour_pos = relative(pos, direction);
    let state = level.block(pos);
    let neighbour = level.block(neighbour_pos);
    let updated = update_shape(level, state, pos, direction, neighbour);
    if updated != state {
        level.set_block(pos, updated);
    }
    let neighbour_updated = update_shape(
        level,
        neighbour,
        neighbour_pos,
        direction.opposite(),
        updated,
    );
    if neighbour_updated != neighbour {
        level.set_block(neighbour_pos, neighbour_updated);
    }
}
