//! The path finder on a fixed layout of blocks, for the comparison with the game's own
//! (`tests/pathharness/PathHarness.java`, which runs the jar's `PathFinder` on the same
//! layouts): the only way in from outside the crate, and not for anything else.

use std::collections::BTreeMap;

use clustine_data::{BlockState, PathType, blocks};
use clustine_world::{BlockPos, ChunkPos, EntityId};

use crate::mobs::ai::pathfinder::{self, Evaluator, EvaluatorFlags, Searcher};
use crate::physics::World;
use crate::region::Random;

/// Blocks by position, air elsewhere, between `min_y` and `min_y + height`.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub blocks: BTreeMap<BlockPos, BlockState>,
    pub min_y: i32,
    pub height: i32,
}

impl World for Layout {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if pos.y < self.min_y || pos.y >= self.min_y + self.height {
            return None;
        }
        Some(self.blocks.get(&pos).copied().unwrap_or(blocks::AIR))
    }

    fn has_chunk(&self, _chunk: ChunkPos) -> bool {
        true
    }
}

/// Which node evaluator a search uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Walk,
    Swim,
    Fly,
    Amphibious,
}

/// A mob and what it searches for.
#[derive(Debug, Clone)]
pub struct Case {
    pub kind: Kind,
    /// Its feet.
    pub position: [f64; 3],
    pub width: f32,
    pub height: f32,
    pub step_height: f32,
    pub max_fall: i32,
    pub on_ground: bool,
    pub in_floatable: bool,
    /// Pass doors, open doors, float, walk over fences.
    pub flags: [bool; 4],
    /// What it pays for a path type where not the game's, by the type.
    pub malus: Vec<(PathType, f32)>,
    pub target: BlockPos,
    pub range: f32,
    pub reach: i32,
    pub max_visited: i32,
}

/// A path found: whether it reaches the target, and its nodes with their types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub reached: bool,
    pub nodes: Vec<(BlockPos, PathType)>,
}

/// `PathFinder.findPath` for `case` on `layout`.
pub fn find(layout: &Layout, case: &Case) -> Option<Found> {
    let mut malus = pathfinder::default_malus();
    for (kind, value) in &case.malus {
        malus[*kind as usize] = *value;
    }
    let searcher = Searcher {
        stands_on_lava: false,
        position: case.position,
        width: case.width,
        height: case.height,
        on_ground: case.on_ground,
        in_floatable: case.in_floatable,
        in_water: false,
        fall_distance: 0.0,
        step_height: case.step_height,
        max_fall: case.max_fall,
        malus,
        flags: EvaluatorFlags {
            can_pass_doors: case.flags[0],
            can_open_doors: case.flags[1],
            can_float: case.flags[2],
            can_walk_over_fences: case.flags[3],
        },
        evaluator: match case.kind {
            Kind::Walk => Evaluator::Walk,
            Kind::Swim => Evaluator::Swim { breach: false },
            Kind::Fly => Evaluator::Fly,
            Kind::Amphibious => Evaluator::Amphibious { shallow: false },
        },
        min_y: layout.min_y,
        sea_level: 63,
    };
    // A walking search draws nothing; a flying one's start may.
    let mut draws = Random::of_entity(0, EntityId(1), 0);
    let found = pathfinder::find_path(
        layout,
        &searcher,
        case.target,
        case.range,
        case.reach,
        case.max_visited,
        &mut draws,
    )?;
    Some(Found {
        reached: found.reached,
        nodes: found
            .nodes
            .iter()
            .map(|node| (node.pos(), node.kind()))
            .collect(),
    })
}
