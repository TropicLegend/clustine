// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/mod.rs` and
// the helpers of `steel-core/src/worldgen/feature/runner.rs`.

//! What the trees' placers and decorators share: the sets a tree collects its blocks
//! in, the tests of the game's `TreeFeature` (`validTreePos`, `isAirOrLeaves`), and
//! the game's ways of drawing a direction and shuffling a list.

use std::sync::LazyLock;

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{block_id, block_tag};
use crate::context::Level;
use crate::java_set::JavaSet;

/// A block tag, looked up once: its blocks' ids.
pub struct Tag {
    name: &'static str,
    entries: LazyLock<&'static [i32]>,
}

impl Tag {
    pub const fn new(name: &'static str, entries: fn() -> &'static [i32]) -> Self {
        Self {
            name,
            entries: LazyLock::new(entries),
        }
    }

    pub fn contains(&self, state: BlockState) -> bool {
        let Some(id) = state.block_id() else {
            return false;
        };
        self.entries.contains(&(id as i32))
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

macro_rules! tag {
    ($name:ident, $tag:literal) => {
        pub static $name: Tag = Tag::new($tag, || {
            block_tag($tag).expect(concat!("the game has the tag ", $tag))
        });
    };
}

tag!(LOGS, "minecraft:logs");
tag!(LEAVES, "minecraft:leaves");
tag!(REPLACEABLE_BY_TREES, "minecraft:replaceable_by_trees");
tag!(
    REPLACEABLE_BY_MUSHROOMS,
    "minecraft:replaceable_by_mushrooms"
);
tag!(
    PREVENTS_NEARBY_LEAF_DECAY,
    "minecraft:prevents_nearby_leaf_decay"
);
tag!(JUNGLE_LOGS, "minecraft:jungle_logs");
tag!(SNOW, "minecraft:snow");

/// The horizontal directions in the game's order (`Direction.Plane.HORIZONTAL`).
pub const HORIZONTAL: [Face; 4] = [Face::North, Face::East, Face::South, Face::West];

/// `TreeFeature.validTreePos`: air, or a block a tree may replace.
pub fn valid_tree_pos(level: &mut Level<'_>, pos: BlockPos) -> bool {
    let state = level.block(pos);
    state.props().is_air() || REPLACEABLE_BY_TREES.contains(state)
}

/// `TreeFeature.isAirOrLeaves`.
pub fn is_air_or_leaves(level: &mut Level<'_>, pos: BlockPos) -> bool {
    let state = level.block(pos);
    state.props().is_air() || LEAVES.contains(state)
}

pub fn is_air(level: &mut Level<'_>, pos: BlockPos) -> bool {
    level.block(pos).props().is_air()
}

/// `Direction.Plane.HORIZONTAL.getRandomDirection`.
pub fn random_horizontal<R: Random + ?Sized>(random: &mut R) -> Face {
    HORIZONTAL[random.next_i32_bounded(4) as usize]
}

/// `Util.shuffle`: from the end, each item swapped with one at or before it.
pub fn shuffle<T, R: Random + ?Sized>(items: &mut [T], random: &mut R) {
    for i in (1..items.len()).rev() {
        let j = random.next_i32_bounded(i as i32 + 1) as usize;
        items.swap(i, j);
    }
}

/// `Direction.getClockWise` of a horizontal direction.
pub fn clockwise(face: Face) -> Face {
    match face {
        Face::North => Face::East,
        Face::East => Face::South,
        Face::South => Face::West,
        Face::West => Face::North,
        other => other,
    }
}

/// Whether a direction points along its axis (`AxisDirection.POSITIVE`).
pub fn is_positive(face: Face) -> bool {
    matches!(face, Face::Up | Face::South | Face::East)
}

pub fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.z - b.z).abs()
}

/// The axis a log lies along, as the property `axis` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn of(face: Face) -> Self {
        match face {
            Face::Down | Face::Up => Axis::Y,
            Face::North | Face::South => Axis::Z,
            Face::West | Face::East => Axis::X,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }
}

/// `trySetValue(AXIS, axis)`: the state turned to `axis` where it has an axis.
pub fn with_axis(state: BlockState, axis: Axis) -> BlockState {
    state.with_property("axis", axis.name()).unwrap_or(state)
}

/// A vine on the side `face` of its block (`VineBlock`'s property of that face).
pub fn vine(face: Face) -> BlockState {
    blocks::VINE
        .with_property(crate::blocks::face_name(face), "true")
        .expect("a vine has a property for each side but the bottom")
}

pub fn is_block(state: BlockState, block: BlockState) -> bool {
    state.block_id().is_some() && state.block_id() == block.block_id()
}

/// The id of a block for a tick of it.
pub fn tick_id(state: BlockState) -> u16 {
    block_id(state) as u16
}

/// The blocks a tree has placed, by the setter that placed them: the game's four
/// `HashSet`s of `TreeFeature.place`, whose order the decorators and the walk of the
/// leaves follow. A position is added before the block is set, whether or not the
/// world takes it, as the game's setters do.
#[derive(Debug, Default)]
pub struct Placement {
    pub roots: JavaSet,
    pub trunks: JavaSet,
    pub foliage: JavaSet,
    pub decorations: JavaSet,
}

impl Placement {
    pub fn set_root(&mut self, level: &mut Level<'_>, pos: BlockPos, state: BlockState) {
        self.roots.insert(pos);
        level.set_block_known_shape(pos, state);
    }

    pub fn set_trunk(&mut self, level: &mut Level<'_>, pos: BlockPos, state: BlockState) {
        self.trunks.insert(pos);
        level.set_block_known_shape(pos, state);
    }

    pub fn set_foliage(&mut self, level: &mut Level<'_>, pos: BlockPos, state: BlockState) {
        self.foliage.insert(pos);
        level.set_block_known_shape(pos, state);
    }
}

/// The positions of a set sorted by y, the others kept in the set's order: the lists
/// of `TreeDecorator.Context` (a stable sort of a copy).
pub fn sorted_by_y(set: &JavaSet) -> Vec<BlockPos> {
    let mut positions: Vec<BlockPos> = set.iter().collect();
    positions.sort_by_key(|pos| pos.y);
    positions
}
