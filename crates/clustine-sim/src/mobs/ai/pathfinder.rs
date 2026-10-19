//! The game's path finder (`PathFinder`, `BinaryHeap`, `Node`, `Target`) and its node
//! evaluators (`WalkNodeEvaluator` with the amphibious and flying ones that extend it,
//! and `SwimNodeEvaluator`), as the brief's sections 3.10 to 3.17 and the bytecode have
//! them.
//!
//! A search is a function of the mob as it reads it ([`Searcher`]), its target and the
//! blocks a [`World`] gives, so that it can be judged node for node against the jar's
//! own: no clock, and no draw but the flying evaluator's start (`FlyNodeEvaluator.
//! getStart`, from the mob's own stream). What the world does not have reads as air, as
//! the game's unloaded chunks and the outside of its search box read
//! (`PathNavigationRegion.getChunk`): neither has a block, a fluid or a collision.
//!
//! The level's cache of path types (`PathTypeCache`) is not kept: every type is
//! computed afresh, which is the game's answer wherever its cache is not stale (the
//! brief's 3.0.5, a named difference). The search's own memos (`pathTypesByPosCacheByMob`,
//! the collision cache) are pure and kept for the one search, as the game keeps them.

use std::collections::BTreeMap;

use clustine_blocks::data::{fluid_in_tag, in_tag};
use clustine_data::{BlockState, PathComputation, PathType, blocks};
use clustine_world::BlockPos;

use crate::physics::aabb::Aabb;
use crate::physics::collide::{self, Context};
use crate::physics::{self, Vec3, World};
use crate::region::Random;

/// Which evaluator a mob's navigation makes (`createPathFinder` of each navigation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Evaluator {
    /// `WalkNodeEvaluator`: every land animal and golem.
    Walk,
    /// `AmphibiousNodeEvaluator(prefersShallowSwimming)`: axolotls and turtles.
    Amphibious { shallow: bool },
    /// `SwimNodeEvaluator(allowBreaching)`: fish, squid, dolphins (who breach).
    Swim { breach: bool },
    /// `FlyNodeEvaluator`: parrots, bees, allays.
    Fly,
}

/// The evaluator's flags (`NodeEvaluator`'s), which navigation and goals set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvaluatorFlags {
    pub(crate) can_pass_doors: bool,
    pub(crate) can_open_doors: bool,
    pub(crate) can_float: bool,
    pub(crate) can_walk_over_fences: bool,
}

impl Default for EvaluatorFlags {
    /// `NodeEvaluator`'s own: doors passed, not opened, no floating, no fences.
    fn default() -> Self {
        Self {
            can_pass_doors: true,
            can_open_doors: false,
            can_float: false,
            can_walk_over_fences: false,
        }
    }
}

/// What a mob pays for each path type (`Mob.getPathfindingMalus`), by ordinal.
pub(crate) type Malus = [f32; 27];

/// The game's maluses of every type, which a mob's map overrides.
pub(crate) fn default_malus() -> Malus {
    std::array::from_fn(|ordinal| PathType::ALL[ordinal].malus())
}

/// A mob as a search reads it: where it is and its box, how it moves, what it pays.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Searcher {
    /// Its feet.
    pub(crate) position: Vec3,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) on_ground: bool,
    /// `isInFloatableFluid`: in water deep enough to count.
    pub(crate) in_floatable: bool,
    /// `isInWater`.
    pub(crate) in_water: bool,
    /// Its fall, which its collision context reads.
    pub(crate) fall_distance: f64,
    /// `maxUpStep`: the step height attribute.
    pub(crate) step_height: f32,
    /// `getMaxFallDistance`.
    pub(crate) max_fall: i32,
    pub(crate) malus: Malus,
    pub(crate) flags: EvaluatorFlags,
    pub(crate) evaluator: Evaluator,
    /// The level's bottom and its sea level (`getMinY`, `getSeaLevel`).
    pub(crate) min_y: i32,
    pub(crate) sea_level: i32,
}

impl Searcher {
    /// `blockPosition()`.
    pub(crate) fn block_position(&self) -> BlockPos {
        physics::containing(self.position)
    }

    /// `getBoundingBox()`.
    pub(crate) fn bounds(&self) -> Aabb {
        Aabb::of_entity(self.position, self.width, self.height)
    }
}

/// A node of a path as its user reads it: where, and its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PathNode {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) z: i32,
    /// The `PathType`'s ordinal.
    pub(crate) kind: u8,
}

impl PathNode {
    pub(crate) fn pos(self) -> BlockPos {
        BlockPos::new(self.x, self.y, self.z)
    }

    pub(crate) fn kind(self) -> PathType {
        PathType::ALL
            .get(usize::from(self.kind))
            .copied()
            .unwrap_or(PathType::Blocked)
    }
}

/// What a search found (`Path` as made): the nodes from the start, the target asked
/// for, how far the last node is from it, and whether it was reached.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FoundPath {
    pub(crate) nodes: Vec<PathNode>,
    pub(crate) target: BlockPos,
    /// `distToTarget`: the last node's Manhattan distance to the target, in floats.
    pub(crate) dist_to_target: f32,
    pub(crate) reached: bool,
}

/// What a search counted, to measure its cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) pops: u32,
    pub(crate) evaluations: u32,
}

/// A node of the search (`Node`): its cell, its place in the heap, its costs.
#[derive(Debug, Clone)]
struct Node {
    x: i32,
    y: i32,
    z: i32,
    heap_index: i32,
    g: f32,
    h: f32,
    f: f32,
    came_from: Option<usize>,
    closed: bool,
    walked: f32,
    cost_malus: f32,
    kind: PathType,
}

impl Node {
    fn new(x: i32, y: i32, z: i32) -> Self {
        Self {
            x,
            y,
            z,
            heap_index: -1,
            g: 0.0,
            h: 0.0,
            f: 0.0,
            came_from: None,
            closed: false,
            walked: 0.0,
            cost_malus: 0.0,
            kind: PathType::Blocked,
        }
    }
}

/// `Node.createHash`: the table's key. It keeps eight bits of y and folds the signs of x
/// and z into other bits, so that two cells of one search never share it unless they
/// are 128 blocks apart in x or z or 256 in y (the brief's 3.12.2), where they share a
/// node as the game's do.
fn node_hash(x: i32, y: i32, z: i32) -> i32 {
    (y & 255)
        | ((x & 32767) << 8)
        | ((z & 32767).wrapping_shl(24))
        | if x < 0 { i32::MIN } else { 0 }
        | if z < 0 { 32768 } else { 0 }
}

/// `Node.distanceTo(Node)` of two cells: a float square root of float squares.
fn distance(a: (i32, i32, i32), b: (i32, i32, i32)) -> f32 {
    let dx = b.0.wrapping_sub(a.0) as f32;
    let dy = b.1.wrapping_sub(a.1) as f32;
    let dz = b.2.wrapping_sub(a.2) as f32;
    super::maths::sqrt_f32(dx * dx + dy * dy + dz * dz)
}

/// `Node.distanceManhattan`: the int differences' sizes, each a float, summed in order.
fn manhattan(a: (i32, i32, i32), b: (i32, i32, i32)) -> f32 {
    let dx = b.0.wrapping_sub(a.0).wrapping_abs() as f32;
    let dy = b.1.wrapping_sub(a.1).wrapping_abs() as f32;
    let dz = b.2.wrapping_sub(a.2).wrapping_abs() as f32;
    dx + dy + dz
}

/// The search's target (`Target`): its cell, the node seen nearest it, and whether it
/// was reached.
#[derive(Debug, Clone)]
struct Target {
    cell: (i32, i32, i32),
    best_heuristic: f32,
    best_node: Option<usize>,
}

/// `a < b`, false where either is not a number, as the heap's `fcmpg` tests are.
fn less(a: f32, b: f32) -> bool {
    a < b
}

/// The game's `BinaryHeap`: an array of nodes ordered by `f`, which never moves a node
/// past an equal one and breaks a tie between two children towards the right.
#[derive(Debug, Default)]
struct Heap {
    items: Vec<usize>,
}

impl Heap {
    fn insert(&mut self, nodes: &mut [Node], node: usize) {
        let at = self.items.len();
        self.items.push(node);
        nodes[node].heap_index = at as i32;
        self.up(nodes, at);
    }

    fn pop(&mut self, nodes: &mut [Node]) -> usize {
        let top = self.items[0];
        let last = self.items.pop().expect("a heap popped is not empty");
        if !self.items.is_empty() {
            self.items[0] = last;
            nodes[last].heap_index = 0;
            self.down(nodes, 0);
        }
        nodes[top].heap_index = -1;
        top
    }

    fn change_cost(&mut self, nodes: &mut [Node], node: usize, f: f32) {
        let old = nodes[node].f;
        nodes[node].f = f;
        let at = nodes[node].heap_index as usize;
        if f < old {
            self.up(nodes, at);
        } else {
            self.down(nodes, at);
        }
    }

    fn up(&mut self, nodes: &mut [Node], mut at: usize) {
        let node = self.items[at];
        let f = nodes[node].f;
        while at > 0 {
            let parent = (at - 1) >> 1;
            let above = self.items[parent];
            if !less(f, nodes[above].f) {
                break;
            }
            self.items[at] = above;
            nodes[above].heap_index = at as i32;
            at = parent;
        }
        self.items[at] = node;
        nodes[node].heap_index = at as i32;
    }

    fn down(&mut self, nodes: &mut [Node], mut at: usize) {
        let node = self.items[at];
        let f = nodes[node].f;
        let size = self.items.len();
        loop {
            let left = 1 + (at << 1);
            let right = left + 1;
            if left >= size {
                break;
            }
            let left_node = self.items[left];
            let left_f = nodes[left_node].f;
            let (right_node, right_f) = if right >= size {
                (None, f32::INFINITY)
            } else {
                let right_node = self.items[right];
                (Some(right_node), nodes[right_node].f)
            };
            if left_f < right_f {
                if !less(left_f, f) {
                    break;
                }
                self.items[at] = left_node;
                nodes[left_node].heap_index = at as i32;
                at = left;
            } else {
                let Some(right_node) = right_node else {
                    break;
                };
                if !less(right_f, f) {
                    break;
                }
                self.items[at] = right_node;
                nodes[right_node].heap_index = at as i32;
                at = right;
            }
        }
        self.items[at] = node;
        nodes[node].heap_index = at as i32;
    }
}

/// The horizontal directions in the game's order (`Direction.Plane.HORIZONTAL`: north,
/// east, south, west), as steps in x and z, with their `get2DDataValue` (south 0, west 1,
/// north 2, east 3) and their clockwise neighbour's index in this list.
const HORIZONTAL: [((i32, i32), usize); 4] = [((0, -1), 2), ((1, 0), 3), ((0, 1), 0), ((-1, 0), 1)];

/// `Direction.values()`: down, up, north, south, west, east.
const ALL_DIRECTIONS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

/// A cell around a flyer's node, with the cells before it that it cuts past.
type FlyCell = ((i32, i32, i32), &'static [usize]);

/// One search: the evaluator's state (`NodeEvaluator`, `PathfindingContext`) for a mob
/// and a world.
struct Search<'a, W: World + ?Sized> {
    world: &'a W,
    mob: &'a Searcher,
    malus: Malus,
    nodes: Vec<Node>,
    table: BTreeMap<i32, usize>,
    /// `pathTypesByPosCacheByMob` (and the flying and swimming evaluators' own).
    mob_types: BTreeMap<(i32, i32, i32), PathType>,
    /// The types from the states, which the level's cache would hold.
    state_types: BTreeMap<(i32, i32, i32), PathType>,
    /// `collisionCache`.
    collisions: Vec<(Aabb, bool)>,
    entity_width: i32,
    entity_height: i32,
    entity_depth: i32,
    /// `mobPosition`: the mob's block when the search began.
    mob_block: BlockPos,
    counts: Counts,
}

impl<'a, W: World + ?Sized> Search<'a, W> {
    fn new(world: &'a W, mob: &'a Searcher) -> Self {
        let mut malus = mob.malus;
        if let Evaluator::Amphibious { .. } = mob.evaluator {
            // `AmphibiousNodeEvaluator.prepare`: water costs nothing (and the mob keeps
            // that: the navigation writes it back), and walking and the water's edge
            // cost more for the one search.
            malus[PathType::Water as usize] = 0.0;
            malus[PathType::Walkable as usize] = 6.0;
            malus[PathType::WaterBorder as usize] = 4.0;
        }
        Self {
            world,
            mob,
            malus,
            nodes: Vec::new(),
            table: BTreeMap::new(),
            mob_types: BTreeMap::new(),
            state_types: BTreeMap::new(),
            collisions: Vec::new(),
            // `prepare`: the footprint in cells.
            entity_width: super::maths::floor_f32(mob.width + 1.0),
            entity_height: super::maths::floor_f32(mob.height + 1.0),
            entity_depth: super::maths::floor_f32(mob.width + 1.0),
            mob_block: mob.block_position(),
            counts: Counts::default(),
        }
    }

    fn malus(&self, kind: PathType) -> f32 {
        self.malus[kind as usize]
    }

    fn can_float(&self) -> bool {
        self.mob.flags.can_float
    }

    fn is_amphibious(&self) -> bool {
        matches!(self.mob.evaluator, Evaluator::Amphibious { .. })
    }

    /// `getNode(x, y, z)`: the node of the cell, made fresh the first time.
    fn node(&mut self, x: i32, y: i32, z: i32) -> usize {
        let hash = node_hash(x, y, z);
        if let Some(&at) = self.table.get(&hash) {
            return at;
        }
        let at = self.nodes.len();
        self.nodes.push(Node::new(x, y, z));
        self.table.insert(hash, at);
        at
    }

    fn cell(&self, node: usize) -> (i32, i32, i32) {
        let node = &self.nodes[node];
        (node.x, node.y, node.z)
    }

    fn state(&self, x: i32, y: i32, z: i32) -> BlockState {
        self.world.state(BlockPos::new(x, y, z))
    }

    /// `PathfindingContext.getPathTypeFromState`: the state's own type.
    fn state_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        if let Some(&kind) = self.state_types.get(&(x, y, z)) {
            return kind;
        }
        let kind = state_path_type(self.state(x, y, z));
        self.state_types.insert((x, y, z), kind);
        kind
    }

    /// `WalkNodeEvaluator.getPathTypeStatic(context, pos)`: what the cell is to stand in.
    fn static_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        let kind = self.state_type(x, y, z);
        if kind != PathType::Open || y < self.mob.min_y + 1 {
            return kind;
        }
        match self.state_type(x, y - 1, z) {
            PathType::Open | PathType::Water | PathType::Lava | PathType::Walkable => {
                PathType::Open
            }
            PathType::Fire => PathType::Fire,
            PathType::Damaging => PathType::Damaging,
            PathType::StickyHoney => PathType::StickyHoney,
            PathType::PowderSnow => PathType::OnTopOfPowderSnow,
            PathType::DamageCautious => PathType::DamageCautious,
            PathType::Trapdoor => PathType::OnTopOfTrapdoor,
            _ => self.neighbour_blocks(x, y, z, PathType::Walkable),
        }
    }

    /// `checkNeighbourBlocks`: the first danger of the 24 cells around, in the game's
    /// order, or `fallback`.
    fn neighbour_blocks(&mut self, x: i32, y: i32, z: i32, fallback: PathType) -> PathType {
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    match self.state_type(x + dx, y + dy, z + dz) {
                        PathType::Damaging => return PathType::DamagingInNeighbor,
                        PathType::Fire | PathType::Lava => return PathType::FireInNeighbor,
                        PathType::Water => return PathType::WaterBorder,
                        PathType::DamageCautious => return PathType::DamageCautious,
                        _ => {}
                    }
                }
            }
        }
        fallback
    }

    /// The evaluator's `getPathType(context, x, y, z)`.
    fn path_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        match self.mob.evaluator {
            Evaluator::Walk => self.static_type(x, y, z),
            Evaluator::Amphibious { .. } => {
                if self.state_type(x, y, z) == PathType::Water {
                    for (dx, dy, dz) in ALL_DIRECTIONS {
                        if self.state_type(x + dx, y + dy, z + dz) == PathType::Blocked {
                            return PathType::WaterBorder;
                        }
                    }
                    return PathType::Water;
                }
                self.static_type(x, y, z)
            }
            Evaluator::Fly => self.fly_path_type(x, y, z),
            Evaluator::Swim { .. } => self.swim_type_of_mob(x, y, z),
        }
    }

    /// `FlyNodeEvaluator.getPathType`.
    fn fly_path_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        let mut kind = self.state_type(x, y, z);
        if kind == PathType::Open && y > self.mob.min_y {
            kind = match self.state_type(x, y - 1, z) {
                PathType::Fire | PathType::Lava => PathType::Fire,
                PathType::Damaging => PathType::Damaging,
                PathType::Cocoa => PathType::Cocoa,
                PathType::Fence => {
                    if BlockPos::new(x, y - 1, z) == self.mob_block {
                        PathType::Open
                    } else {
                        PathType::Fence
                    }
                }
                PathType::Walkable | PathType::Open | PathType::Water => PathType::Open,
                _ => PathType::Walkable,
            };
        }
        if kind == PathType::Walkable || kind == PathType::Open {
            kind = self.neighbour_blocks(x, y, z, kind);
        }
        kind
    }

    /// `getCachedPathType`: the mob's type of the cell, kept for the search.
    fn cached_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        if let Some(&kind) = self.mob_types.get(&(x, y, z)) {
            return kind;
        }
        let kind = match self.mob.evaluator {
            Evaluator::Swim { .. } => self.swim_type_of_mob(x, y, z),
            _ => self.type_of_mob(x, y, z),
        };
        self.mob_types.insert((x, y, z), kind);
        kind
    }

    /// `WalkNodeEvaluator.getPathTypeOfMob` over `getPathTypeWithinMobBB`.
    fn type_of_mob(&mut self, x: i32, y: i32, z: i32) -> PathType {
        // An `EnumSet`, iterated by ordinal: a bit an ordinal.
        let mut set: u32 = 0;
        let pass = self.mob.flags.can_pass_doors;
        let mob_block = self.mob_block;
        for i in 0..self.entity_width {
            for j in 0..self.entity_height {
                for k in 0..self.entity_depth {
                    let mut kind = self.path_type(x + i, y + j, z + k);
                    if kind == PathType::DoorWoodClosed && self.mob.flags.can_open_doors && pass {
                        kind = PathType::WalkableDoor;
                    }
                    if kind == PathType::DoorOpen && !pass {
                        kind = PathType::Blocked;
                    }
                    if kind == PathType::Rail
                        && self.path_type(mob_block.x, mob_block.y, mob_block.z) != PathType::Rail
                        && self.path_type(mob_block.x, mob_block.y - 1, mob_block.z)
                            != PathType::Rail
                    {
                        kind = PathType::UnpassableRail;
                    }
                    set |= 1 << kind as u32;
                }
            }
        }
        if set.count_ones() == 1 {
            return PathType::ALL[set.trailing_zeros() as usize];
        }
        if set & 1 << PathType::Fence as u32 != 0 {
            return PathType::Fence;
        }
        if set & 1 << PathType::UnpassableRail as u32 != 0 {
            return PathType::UnpassableRail;
        }
        let mut best = PathType::Blocked;
        let mut best_malus = self.malus(PathType::Blocked);
        for kind in PathType::ALL {
            if set & 1 << kind as u32 == 0 {
                continue;
            }
            let malus = self.malus(kind);
            if malus < 0.0 {
                return kind;
            }
            if malus >= best_malus {
                best = kind;
                best_malus = malus;
            }
        }
        // The origin again (the brief's 3.0.7, the game's cost).
        let here = self.path_type(x, y, z);
        if self.entity_width > 1 {
            return if self.malus(here) < best_malus
                && self.malus(PathType::BigMobsCloseToDanger) < best_malus
            {
                PathType::BigMobsCloseToDanger
            } else {
                best
            };
        }
        if here == PathType::Open && best != PathType::Open && best_malus == 0.0 {
            PathType::Open
        } else {
            best
        }
    }

    /// `SwimNodeEvaluator.getPathTypeOfMob`.
    fn swim_type_of_mob(&mut self, x: i32, y: i32, z: i32) -> PathType {
        let mut last = (x, y, z);
        for i in x..x + self.entity_width {
            for j in y..y + self.entity_height {
                for k in z..z + self.entity_depth {
                    last = (i, j, k);
                    let state = self.state(i, j, k);
                    let below = self.state(i, j - 1, k);
                    if state.props().fluid().is_none()
                        && below.is_pathfindable(PathComputation::Water)
                        && state.props().is_air()
                    {
                        return PathType::Breach;
                    }
                    if !clustine_blocks::data::has_water(state) {
                        return PathType::Blocked;
                    }
                }
            }
        }
        // The far corner the loops left the mutable position at.
        if self
            .state(last.0, last.1, last.2)
            .is_pathfindable(PathComputation::Water)
        {
            PathType::Water
        } else {
            PathType::Blocked
        }
    }

    /// `getFloorLevel(pos)` of the evaluator: a floatable fluid's middle for a mob that
    /// floats or swims, else the top of what is below.
    fn floor_level(&self, x: i32, y: i32, z: i32) -> f64 {
        if (self.can_float() || self.is_amphibious())
            && fluid_in_tag(self.state(x, y, z), "minecraft:entity_floatable")
        {
            return f64::from(y) + 0.5;
        }
        floor_level(self.world, BlockPos::new(x, y, z))
    }

    /// `hasCollisions(box)`: whether the blocks meet the box, as the mob's own context
    /// has their shapes; no entity counts.
    fn has_collisions(&mut self, bounds: Aabb) -> bool {
        if let Some((_, hit)) = self.collisions.iter().find(|(held, _)| *held == bounds) {
            return *hit;
        }
        let context = Context {
            bottom: self.mob.position[1],
            fall_distance: self.mob.fall_distance,
            falling_block: false,
            noclip: None,
        };
        let hit = !collide::block_collisions(self.world, context, &bounds).is_empty();
        self.collisions.push((bounds, hit));
        hit
    }

    /// `getStartNode(pos)`.
    fn start_node(&mut self, pos: BlockPos) -> usize {
        let node = self.node(pos.x, pos.y, pos.z);
        let kind = self.cached_type(pos.x, pos.y, pos.z);
        self.nodes[node].kind = kind;
        self.nodes[node].cost_malus = self.malus(kind);
        node
    }

    /// `canStartAt(pos)`: the walker's refuses open air; the flyer's does not.
    fn can_start_at(&mut self, pos: BlockPos) -> bool {
        let kind = self.cached_type(pos.x, pos.y, pos.z);
        match self.mob.evaluator {
            Evaluator::Fly => self.malus(kind) >= 0.0,
            _ => kind != PathType::Open && self.malus(kind) >= 0.0,
        }
    }

    /// The evaluator's `getStart()`.
    fn start(&mut self, draws: &mut Random) -> Option<usize> {
        match self.mob.evaluator {
            Evaluator::Walk => Some(self.walk_start()),
            Evaluator::Amphibious { .. } => {
                if !self.mob.in_water {
                    return Some(self.walk_start());
                }
                let bounds = self.mob.bounds();
                Some(self.start_node(BlockPos::new(
                    physics::floor(bounds.min[0]),
                    physics::floor(bounds.min[1] + 0.5),
                    physics::floor(bounds.min[2]),
                )))
            }
            Evaluator::Swim { .. } => {
                let bounds = self.mob.bounds();
                Some(self.node(
                    physics::floor(bounds.min[0]),
                    physics::floor(bounds.min[1] + 0.5),
                    physics::floor(bounds.min[2]),
                ))
            }
            Evaluator::Fly => Some(self.fly_start(draws)),
        }
    }

    /// `WalkNodeEvaluator.getStart()`.
    fn walk_start(&mut self) -> usize {
        let [x, _, z] = self.mob.position;
        let block = self.mob.block_position();
        let mut y = block.y;
        let at = |y: i32| BlockPos::new(physics::floor(x), y, physics::floor(z));
        // No mob of these can stand on a fluid (`canStandOnFluid`, the strider's).
        if self.can_float() && self.mob.in_floatable {
            let mut state = self.world.state(at(y));
            while fluid_in_tag(state, "minecraft:entity_floatable") {
                y += 1;
                state = self.world.state(at(y));
            }
            y -= 1;
        } else if self.mob.on_ground {
            y = physics::floor(self.mob.position[1] + 0.5);
        } else {
            let mut probe = physics::floor(self.mob.position[1] + 1.0);
            while probe > self.mob.min_y {
                y = probe;
                probe -= 1;
                let below = self.world.state(at(probe));
                if !below.props().is_air() && !below.is_pathfindable(PathComputation::Land) {
                    break;
                }
            }
        }
        let first = BlockPos::new(block.x, y, block.z);
        if !self.can_start_at(first) {
            let bounds = self.mob.bounds();
            let fy = f64::from(y);
            for (cx, cz) in [
                (bounds.min[0], bounds.min[2]),
                (bounds.min[0], bounds.max[2]),
                (bounds.max[0], bounds.min[2]),
                (bounds.max[0], bounds.max[2]),
            ] {
                let corner = physics::containing([cx, fy, cz]);
                if self.can_start_at(corner) {
                    return self.start_node(corner);
                }
            }
        }
        self.start_node(first)
    }

    /// `FlyNodeEvaluator.getStart()`: the one place a search draws.
    fn fly_start(&mut self, draws: &mut Random) -> usize {
        let [x, _, z] = self.mob.position;
        let mut y;
        if self.can_float() && self.mob.in_water {
            y = self.mob.block_position().y;
            while clustine_blocks::data::same_block(
                self.world.state(physics::containing([x, f64::from(y), z])),
                blocks::WATER,
            ) {
                y += 1;
            }
        } else {
            y = physics::floor(self.mob.position[1] + 0.5);
        }
        let first = physics::containing([x, f64::from(y), z]);
        if self.can_start_at(first) {
            return self.start_node(first);
        }
        let bounds = self.mob.bounds();
        let size = bounds.size();
        if (size[0] + size[1] + size[2]) / 3.0 >= 1.0 {
            let block_y = f64::from(self.mob.block_position().y);
            for (cx, cz) in [
                (bounds.min[0], bounds.min[2]),
                (bounds.min[0], bounds.max[2]),
                (bounds.max[0], bounds.min[2]),
                (bounds.max[0], bounds.max[2]),
            ] {
                let corner = physics::containing([cx, block_y, cz]);
                if self.can_start_at(corner) {
                    return self.start_node(corner);
                }
            }
        } else {
            // `1.1f` widened, less each side's size, at least nothing.
            let grow = |extent: f64| (f64::from(1.1_f32) - extent).max(0.0);
            let wide = bounds.inflate([grow(size[0]), grow(size[1]), grow(size[2])]);
            let low = wide.min.map(physics::floor);
            let high = wide.max.map(physics::floor);
            let span = |axis: usize| (high[axis] - low[axis] + 1).max(1) as u32;
            // `BlockPos.randomBetweenClosed`: up to ten cells, each drawn as it is asked
            // for, x then y then z (**Inferred**: its iterator is not in the dump).
            for _ in 0..10 {
                let cx = low[0] + draws.next_int(span(0)) as i32;
                let cy = low[1] + draws.next_int(span(1)) as i32;
                let cz = low[2] + draws.next_int(span(2)) as i32;
                let candidate = BlockPos::new(cx, cy, cz);
                if self.can_start_at(candidate) {
                    return self.start_node(candidate);
                }
            }
        }
        self.start_node(first)
    }

    /// `getTarget(x, y, z)`: a target at the cell, whose node is made in the table too.
    fn target(&mut self, pos: BlockPos) -> Target {
        // The amphibious evaluator asks half a block up, which floors to the same cell.
        self.node(pos.x, pos.y, pos.z);
        Target {
            cell: (pos.x, pos.y, pos.z),
            best_heuristic: f32::MAX,
            best_node: None,
        }
    }

    /// `getNodeAndUpdateCostToMax`.
    fn node_with_cost(&mut self, x: i32, y: i32, z: i32, kind: PathType, malus: f32) -> usize {
        let node = self.node(x, y, z);
        self.nodes[node].kind = kind;
        self.nodes[node].cost_malus = self.nodes[node].cost_malus.max(malus);
        node
    }

    /// `getBlockedNode`.
    fn blocked_node(&mut self, x: i32, y: i32, z: i32) -> usize {
        let node = self.node(x, y, z);
        self.nodes[node].kind = PathType::Blocked;
        self.nodes[node].cost_malus = -1.0;
        node
    }

    /// `getClosedNode`: closed, of the type's own malus (not the mob's).
    fn closed_node(&mut self, x: i32, y: i32, z: i32, kind: PathType) -> usize {
        let node = self.node(x, y, z);
        self.nodes[node].closed = true;
        self.nodes[node].kind = kind;
        self.nodes[node].cost_malus = kind.malus();
        node
    }

    /// The evaluator's `getNeighbors(out, current)`.
    fn neighbours(&mut self, current: usize) -> Vec<usize> {
        match self.mob.evaluator {
            Evaluator::Walk => self.walk_neighbours(current),
            Evaluator::Amphibious { shallow } => self.amphibious_neighbours(current, shallow),
            Evaluator::Swim { .. } => self.swim_neighbours(current),
            Evaluator::Fly => self.fly_neighbours(current),
        }
    }

    /// The walker's jump: one step up where the cell above is allowed and this is no
    /// honey (`floor(max(1, maxUpStep))`).
    fn jump_and_floor(&mut self, (x, y, z): (i32, i32, i32)) -> (i32, f64, PathType) {
        let above = self.cached_type(x, y + 1, z);
        let here = self.cached_type(x, y, z);
        let mut jump = 0;
        if self.malus(above) >= 0.0 && here != PathType::StickyHoney {
            jump = super::maths::floor_f32(1.0_f32.max(self.mob.step_height));
        }
        let floor = self.floor_level(x, y, z);
        (jump, floor, here)
    }

    /// `WalkNodeEvaluator.getNeighbors`: north, east, south and west, then the diagonals
    /// clockwise of each.
    fn walk_neighbours(&mut self, current: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let cell = self.cell(current);
        let (jump, floor, here) = self.jump_and_floor(cell);
        let mut reusable: [Option<usize>; 4] = [None; 4];
        for ((dx, dz), data) in HORIZONTAL {
            let found = self.accepted_node(
                cell.0 + dx,
                cell.1,
                cell.2 + dz,
                jump,
                floor,
                (dx, dz),
                here,
            );
            reusable[data] = found;
            if self.neighbour_valid(found, current) {
                out.extend(found);
            }
        }
        for (index, ((dx, dz), data)) in HORIZONTAL.into_iter().enumerate() {
            let ((cx, cz), clockwise) = HORIZONTAL[(index + 1) % 4];
            if self.diagonal_valid(current, reusable[data], reusable[clockwise]) {
                let found = self.accepted_node(
                    cell.0 + dx + cx,
                    cell.1,
                    cell.2 + dz + cz,
                    jump,
                    floor,
                    (dx, dz),
                    here,
                );
                if self.diagonal_open(found) {
                    out.extend(found);
                }
            }
        }
        out
    }

    /// `AmphibiousNodeEvaluator.getNeighbors`: the walker's, then up and down through
    /// water, then the deep water's extra cost for a mob that prefers the shallows.
    fn amphibious_neighbours(&mut self, current: usize, shallow: bool) -> Vec<usize> {
        let mut out = self.walk_neighbours(current);
        let cell = self.cell(current);
        let (jump, floor, here) = self.jump_and_floor(cell);
        let up = self.accepted_node(
            cell.0,
            cell.1 + 1,
            cell.2,
            (jump - 1).max(0),
            floor,
            (0, 0),
            here,
        );
        let down = self.accepted_node(cell.0, cell.1 - 1, cell.2, jump, floor, (0, 0), here);
        if self.vertical_valid(up, current) {
            out.extend(up);
        }
        if self.vertical_valid(down, current) && here != PathType::Trapdoor {
            out.extend(down);
        }
        if shallow {
            for &node in &out {
                if self.nodes[node].kind == PathType::Water
                    && self.nodes[node].y < self.mob.sea_level - 10
                {
                    self.nodes[node].cost_malus += 1.0;
                }
            }
        }
        out
    }

    fn vertical_valid(&self, node: Option<usize>, current: usize) -> bool {
        self.neighbour_valid(node, current)
            && node.is_some_and(|node| self.nodes[node].kind == PathType::Water)
    }

    /// `isNeighborValid`: open, and allowed, or the mob is already somewhere it is not
    /// allowed.
    fn neighbour_valid(&self, node: Option<usize>, current: usize) -> bool {
        let Some(node) = node else {
            return false;
        };
        let node = &self.nodes[node];
        !node.closed && (node.cost_malus >= 0.0 || self.nodes[current].cost_malus < 0.0)
    }

    /// `isDiagonalValid(current, a, b)`, `a` the neighbour in the direction iterated and
    /// `b` in its clockwise one.
    fn diagonal_valid(&self, current: usize, a: Option<usize>, b: Option<usize>) -> bool {
        let (Some(a), Some(b)) = (a, b) else {
            return false;
        };
        let (current, a, b) = (&self.nodes[current], &self.nodes[a], &self.nodes[b]);
        if b.y > current.y || a.y > current.y {
            return false;
        }
        if a.kind == PathType::WalkableDoor || b.kind == PathType::WalkableDoor {
            return false;
        }
        if self.mob.width > 1.0 && (a.cost_malus > 0.0 || b.cost_malus > 0.0) {
            return false;
        }
        let fences = b.kind == PathType::Fence
            && a.kind == PathType::Fence
            && f64::from(self.mob.width) < 0.5;
        (b.y < current.y || b.cost_malus >= 0.0 || fences)
            && (a.y < current.y || a.cost_malus >= 0.0 || fences)
    }

    /// `isDiagonalValid(node)`.
    fn diagonal_open(&self, node: Option<usize>) -> bool {
        let Some(node) = node else {
            return false;
        };
        let node = &self.nodes[node];
        !node.closed && node.kind != PathType::WalkableDoor && node.cost_malus >= 0.0
    }

    /// `doesBlockHavePartialCollision`.
    fn partial_collision(kind: PathType) -> bool {
        matches!(
            kind,
            PathType::Fence | PathType::DoorWoodClosed | PathType::DoorIronClosed
        )
    }

    /// `WalkNodeEvaluator.findAcceptedNode(x, y, z, jump, floor, direction, here)`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the game's own arguments, each read as it reads them"
    )]
    fn accepted_node(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        jump: i32,
        floor: f64,
        direction: (i32, i32),
        here: PathType,
    ) -> Option<usize> {
        self.counts.evaluations += 1;
        let mut node = None;
        let level = self.floor_level(x, y, z);
        // `getMobJumpHeight`.
        let jump_height = 1.125_f64.max(f64::from(self.mob.step_height));
        if level - floor > jump_height {
            return None;
        }
        let kind = self.cached_type(x, y, z);
        let malus = self.malus(kind);
        if malus >= 0.0 {
            node = Some(self.node_with_cost(x, y, z, kind, malus));
        }
        if Self::partial_collision(here)
            && let Some(found) = node
            && self.nodes[found].cost_malus >= 0.0
            && !self.reaches_without_collision(found)
        {
            node = None;
        }
        if kind == PathType::Walkable || (self.is_amphibious() && kind == PathType::Water) {
            return node;
        }
        if node.is_none_or(|found| self.nodes[found].cost_malus < 0.0)
            && jump > 0
            && (kind != PathType::Fence || self.mob.flags.can_walk_over_fences)
            && kind != PathType::UnpassableRail
            && kind != PathType::Trapdoor
            && kind != PathType::PowderSnow
        {
            node = self.jump_on(x, y, z, jump, floor, direction, here);
        } else if !self.is_amphibious() && kind == PathType::Water && !self.can_float() {
            node = self.first_non_water_below(x, y, z, node);
        } else if kind == PathType::Open {
            node = Some(self.first_ground_below(x, y, z));
        } else if Self::partial_collision(kind) && node.is_none() {
            node = Some(self.closed_node(x, y, z, kind));
        }
        node
    }

    /// `tryJumpOn`: the cell a step up, if the mob has room to rise into it from the
    /// cell one step back along the direction iterated.
    #[allow(
        clippy::too_many_arguments,
        reason = "the game's own arguments, each read as it reads them"
    )]
    fn jump_on(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        jump: i32,
        floor: f64,
        direction: (i32, i32),
        here: PathType,
    ) -> Option<usize> {
        let up = self.accepted_node(x, y + 1, z, jump - 1, floor, direction, here)?;
        if self.mob.width >= 1.0 {
            return Some(up);
        }
        let kind = self.nodes[up].kind;
        if kind != PathType::Open && kind != PathType::Walkable {
            return Some(up);
        }
        let cx = f64::from(x - direction.0) + 0.5;
        let cz = f64::from(z - direction.1) + 0.5;
        let half = f64::from(self.mob.width) / 2.0;
        let lower = self.floor_level(
            physics::floor(cx),
            physics::floor(f64::from(y + 1)),
            physics::floor(cz),
        ) + 0.001;
        let (ux, uy, uz) = self.cell(up);
        let upper = f64::from(self.mob.height) + self.floor_level(ux, uy, uz) - 0.002;
        let room = Aabb::new([cx - half, lower, cz - half], [cx + half, upper, cz + half]);
        if self.has_collisions(room) {
            None
        } else {
            Some(up)
        }
    }

    /// `tryFindFirstNonWaterBelow`: a mob that cannot float plans through water as if
    /// sinking.
    fn first_non_water_below(
        &mut self,
        x: i32,
        mut y: i32,
        z: i32,
        mut node: Option<usize>,
    ) -> Option<usize> {
        y -= 1;
        while y > self.mob.min_y {
            let kind = self.cached_type(x, y, z);
            if kind != PathType::Water {
                return node;
            }
            let malus = self.malus(kind);
            node = Some(self.node_with_cost(x, y, z, kind, malus));
            y -= 1;
        }
        node
    }

    /// `tryFindFirstGroundNodeBelow`: the first cell that is not open air below, within
    /// the mob's fall, or a blocked one.
    fn first_ground_below(&mut self, x: i32, y: i32, z: i32) -> usize {
        let mut below = y - 1;
        while below >= self.mob.min_y {
            if y - below > self.mob.max_fall {
                return self.blocked_node(x, below, z);
            }
            let kind = self.cached_type(x, below, z);
            let malus = self.malus(kind);
            if kind != PathType::Open {
                return if malus >= 0.0 {
                    self.node_with_cost(x, below, z, kind, malus)
                } else {
                    self.blocked_node(x, below, z)
                };
            }
            below -= 1;
        }
        self.blocked_node(x, y, z)
    }

    /// `canReachWithoutCollision`: the mob's box swept to the node in steps of its size
    /// meets nothing.
    fn reaches_without_collision(&mut self, node: usize) -> bool {
        let mut bounds = self.mob.bounds();
        let size = bounds.size();
        let (nx, ny, nz) = self.cell(node);
        let mut step = [
            f64::from(nx) - self.mob.position[0] + size[0] / 2.0,
            f64::from(ny) - self.mob.position[1] + size[1] / 2.0,
            f64::from(nz) - self.mob.position[2] + size[2] / 2.0,
        ];
        let length = physics::length_sqr(step).sqrt();
        let parts = super::maths::ceil(length / ((size[0] + size[1] + size[2]) / 3.0));
        let fraction = f64::from(1.0_f32 / parts as f32);
        step = physics::scale(step, fraction);
        for _ in 1..=parts {
            bounds = bounds.moved(step);
            if self.has_collisions(bounds) {
                return false;
            }
        }
        true
    }

    /// `SwimNodeEvaluator.getNeighbors`: the six faces, then the horizontal diagonals
    /// between two allowed faces.
    fn swim_neighbours(&mut self, current: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let (x, y, z) = self.cell(current);
        let mut faces: [Option<usize>; 6] = [None; 6];
        for (index, (dx, dy, dz)) in ALL_DIRECTIONS.into_iter().enumerate() {
            let found = self.swim_accepted(x + dx, y + dy, z + dz);
            faces[index] = found;
            if found.is_some_and(|node| !self.nodes[node].closed) {
                out.extend(found);
            }
        }
        // North, east, south, west, by their places in `ALL_DIRECTIONS`, each with its
        // clockwise neighbour.
        const FACES: [(usize, (i32, i32)); 4] =
            [(2, (0, -1)), (5, (1, 0)), (3, (0, 1)), (4, (-1, 0))];
        for (index, (face, (dx, dz))) in FACES.into_iter().enumerate() {
            let (clockwise, (cx, cz)) = FACES[(index + 1) % 4];
            let allowed =
                |node: Option<usize>| node.is_some_and(|node| self.nodes[node].cost_malus >= 0.0);
            if allowed(faces[face]) && allowed(faces[clockwise]) {
                let found = self.swim_accepted(x + dx + cx, y, z + dz + cz);
                if found.is_some_and(|node| !self.nodes[node].closed) {
                    out.extend(found);
                }
            }
        }
        out
    }

    /// `SwimNodeEvaluator.findAcceptedNode`: water, or air to breach into, each time
    /// eight dearer where there is no fluid (the brief's 3.0.7).
    fn swim_accepted(&mut self, x: i32, y: i32, z: i32) -> Option<usize> {
        self.counts.evaluations += 1;
        let breach = matches!(self.mob.evaluator, Evaluator::Swim { breach: true });
        let kind = self.cached_type(x, y, z);
        if !((breach && kind == PathType::Breach) || kind == PathType::Water) {
            return None;
        }
        let malus = self.malus(kind);
        if malus < 0.0 {
            return None;
        }
        let node = self.node_with_cost(x, y, z, kind, malus);
        if self.state(x, y, z).props().fluid().is_none() {
            self.nodes[node].cost_malus += 8.0;
        }
        Some(node)
    }

    /// `FlyNodeEvaluator.findAcceptedNode(x, y, z)`: allowed cells, a walkable one each
    /// time one dearer.
    fn fly_accepted(&mut self, x: i32, y: i32, z: i32) -> Option<usize> {
        self.counts.evaluations += 1;
        let kind = self.cached_type(x, y, z);
        let malus = self.malus(kind);
        if malus < 0.0 {
            return None;
        }
        let node = self.node_with_cost(x, y, z, kind, malus);
        if kind == PathType::Walkable {
            self.nodes[node].cost_malus += 1.0;
        }
        Some(node)
    }

    /// `FlyNodeEvaluator.getNeighbors`: all 26 cells around, every one evaluated, each
    /// added if open and, off the faces, if the cells it cuts past are allowed.
    fn fly_neighbours(&mut self, current: usize) -> Vec<usize> {
        let (x, y, z) = self.cell(current);
        let mut out = Vec::new();
        let mut cells: Vec<Option<usize>> = Vec::with_capacity(26);
        // The cells in the game's order, each with the cells (by their place in this
        // list) that must be allowed for it to be added.
        const ORDER: [FlyCell; 26] = [
            ((0, 0, 1), &[]),                       // 0 south
            ((-1, 0, 0), &[]),                      // 1 west
            ((1, 0, 0), &[]),                       // 2 east
            ((0, 0, -1), &[]),                      // 3 north
            ((0, 1, 0), &[]),                       // 4 up
            ((0, -1, 0), &[]),                      // 5 down
            ((0, 1, 1), &[0, 4]),                   // 6 up south
            ((-1, 1, 0), &[1, 4]),                  // 7 up west
            ((1, 1, 0), &[2, 4]),                   // 8 up east
            ((0, 1, -1), &[3, 4]),                  // 9 up north
            ((0, -1, 1), &[0, 5]),                  // 10 down south
            ((-1, -1, 0), &[1, 5]),                 // 11 down west
            ((1, -1, 0), &[2, 5]),                  // 12 down east
            ((0, -1, -1), &[3, 5]),                 // 13 down north
            ((1, 0, -1), &[3, 2]),                  // 14 north east
            ((1, 0, 1), &[0, 2]),                   // 15 south east
            ((-1, 0, -1), &[3, 1]),                 // 16 north west
            ((-1, 0, 1), &[0, 1]),                  // 17 south west
            ((1, 1, -1), &[14, 3, 2, 4, 9, 8]),     // 18 up north east
            ((1, 1, 1), &[15, 0, 2, 4, 6, 8]),      // 19 up south east
            ((-1, 1, -1), &[16, 3, 1, 4, 9, 7]),    // 20 up north west
            ((-1, 1, 1), &[17, 0, 1, 4, 6, 7]),     // 21 up south west
            ((1, -1, -1), &[14, 3, 2, 5, 13, 12]),  // 22 down north east
            ((1, -1, 1), &[15, 0, 2, 5, 10, 12]),   // 23 down south east
            ((-1, -1, -1), &[16, 3, 1, 5, 13, 11]), // 24 down north west
            ((-1, -1, 1), &[17, 0, 1, 5, 10, 11]),  // 25 down south west
        ];
        for ((dx, dy, dz), needs) in ORDER {
            let found = self.fly_accepted(x + dx, y + dy, z + dz);
            cells.push(found);
            let open = found.is_some_and(|node| !self.nodes[node].closed);
            let allowed = needs
                .iter()
                .all(|&need| cells[need].is_some_and(|node| self.nodes[node].cost_malus >= 0.0));
            if open && allowed {
                out.extend(found);
            }
        }
        out
    }

    /// `getBestH`: the least distance to the targets, noting each target's best node.
    fn best_h(&mut self, node: usize, targets: &mut [Target]) -> f32 {
        let cell = self.cell(node);
        let mut best = f32::MAX;
        for target in targets {
            let d = distance(cell, target.cell);
            if d < target.best_heuristic {
                target.best_heuristic = d;
                target.best_node = Some(node);
            }
            best = d.min(best);
        }
        best
    }

    /// `reconstructPath`: the nodes from the start to `node`.
    fn reconstruct(&self, node: usize, target: BlockPos, reached: bool) -> FoundPath {
        let mut chain = vec![node];
        let mut at = node;
        while let Some(previous) = self.nodes[at].came_from {
            chain.push(previous);
            at = previous;
        }
        chain.reverse();
        let nodes: Vec<PathNode> = chain
            .into_iter()
            .map(|index| {
                let node = &self.nodes[index];
                PathNode {
                    x: node.x,
                    y: node.y,
                    z: node.z,
                    kind: node.kind as u8,
                }
            })
            .collect();
        let dist_to_target = nodes.last().map_or(f32::MAX, |last| {
            manhattan((last.x, last.y, last.z), (target.x, target.y, target.z))
        });
        FoundPath {
            nodes,
            target,
            dist_to_target,
            reached,
        }
    }
}

/// The state's own path type (`getPathTypeFromState`), blocked for an id the data does
/// not know.
pub(crate) fn state_path_type(state: BlockState) -> PathType {
    state.path_type().unwrap_or(PathType::Blocked)
}

/// `WalkNodeEvaluator.getFloorLevel(level, pos)`: the top of the collision shape of the
/// block below, as the empty context has it, or that block's bottom.
pub(crate) fn floor_level<W: World + ?Sized>(world: &W, pos: BlockPos) -> f64 {
    let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
    f64::from(below.y) + shape_top(world.state(below), below).unwrap_or(0.0)
}

/// `VoxelShape.max(Y)` of the state's collision shape at `pos` in its block, moved as a
/// block shifted by its position is; `None` where the shape is empty.
pub(crate) fn shape_top(state: BlockState, pos: BlockPos) -> Option<f64> {
    let shape = state.voxel_collision_shape();
    if shape.is_empty() {
        return None;
    }
    use clustine_data::Axis;
    let (sx, sy, sz) = (
        shape.size(Axis::X),
        shape.size(Axis::Y),
        shape.size(Axis::Z),
    );
    let top = (0..sy)
        .rev()
        .find(|&y| (0..sx).any(|x| (0..sz).any(|z| shape.is_full(x, y, z))))?;
    let shift = collide::offset_of(state, pos).map_or(0.0, |offset| offset[1]);
    Some(shape.coord(Axis::Y, top + 1) + shift)
}

/// `WalkNodeEvaluator.getPathTypeStatic(mob, pos)`: what a cell is to stand in, as goals
/// ask it of the whole level.
pub(crate) fn static_path_type<W: World + ?Sized>(
    world: &W,
    min_y: i32,
    pos: BlockPos,
) -> PathType {
    let mob = Searcher {
        position: [0.0; 3],
        width: 0.0,
        height: 0.0,
        on_ground: false,
        in_floatable: false,
        in_water: false,
        fall_distance: 0.0,
        step_height: 0.0,
        max_fall: 0,
        malus: default_malus(),
        flags: EvaluatorFlags::default(),
        evaluator: Evaluator::Walk,
        min_y,
        sea_level: 63,
    };
    Search::new(world, &mob).static_type(pos.x, pos.y, pos.z)
}

/// `NodeEvaluator.getPathType(mob, pos)`, the evaluator's type of a cell asked once (a
/// strafe's test of where it would step).
pub(crate) fn evaluator_path_type<W: World + ?Sized>(
    world: &W,
    mob: &Searcher,
    pos: BlockPos,
) -> PathType {
    Search::new(world, mob).path_type(pos.x, pos.y, pos.z)
}

/// `PathFinder.findPath` for one target: the path to it from where the mob stands, or
/// towards the node seen nearest it, within `range` walked and `max_visited` nodes (less
/// one: the counter is raised before it is compared, the brief's 3.0.6).
pub(crate) fn find_path<W: World + ?Sized>(
    world: &W,
    mob: &Searcher,
    target: BlockPos,
    range: f32,
    reach: i32,
    max_visited: i32,
    draws: &mut Random,
) -> Option<FoundPath> {
    find_path_counted(world, mob, target, range, reach, max_visited, draws).0
}

/// [`find_path`], with what it counted.
pub(crate) fn find_path_counted<W: World + ?Sized>(
    world: &W,
    mob: &Searcher,
    target: BlockPos,
    range: f32,
    reach: i32,
    max_visited: i32,
    draws: &mut Random,
) -> (Option<FoundPath>, Counts) {
    let mut search = Search::new(world, mob);
    let Some(start) = search.start(draws) else {
        return (None, search.counts);
    };
    let mut targets = [search.target(target)];
    let start_cell = search.cell(start);
    search.nodes[start].g = 0.0;
    let h = search.best_h(start, &mut targets);
    search.nodes[start].h = h;
    search.nodes[start].f = h;
    let mut heap = Heap::default();
    heap.insert(&mut search.nodes, start);
    let mut reached = false;
    let mut count = 0;
    while !heap.items.is_empty() {
        count += 1;
        if count >= max_visited {
            break;
        }
        let current = heap.pop(&mut search.nodes);
        search.counts.pops += 1;
        search.nodes[current].closed = true;
        let cell = search.cell(current);
        if manhattan(cell, targets[0].cell) <= reach as f32 {
            reached = true;
        }
        if reached {
            break;
        }
        if distance(cell, start_cell) >= range {
            continue;
        }
        for neighbour in search.neighbours(current) {
            let d = distance(cell, search.cell(neighbour));
            let walked = search.nodes[current].walked + d;
            search.nodes[neighbour].walked = walked;
            let g = search.nodes[current].g + d + search.nodes[neighbour].cost_malus;
            let in_open = search.nodes[neighbour].heap_index >= 0;
            if walked < range && (!in_open || g < search.nodes[neighbour].g) {
                search.nodes[neighbour].came_from = Some(current);
                search.nodes[neighbour].g = g;
                let h = search.best_h(neighbour, &mut targets) * 1.5;
                search.nodes[neighbour].h = h;
                if in_open {
                    let f = search.nodes[neighbour].g + search.nodes[neighbour].h;
                    heap.change_cost(&mut search.nodes, neighbour, f);
                } else {
                    search.nodes[neighbour].f =
                        search.nodes[neighbour].g + search.nodes[neighbour].h;
                    heap.insert(&mut search.nodes, neighbour);
                }
            }
        }
    }
    let best = targets[0]
        .best_node
        .expect("the start's distance is noted for the target");
    let path = search.reconstruct(best, target, reached);
    (Some(path), search.counts)
}

/// Whether the block at `pos` is a cauldron of any kind (`trimPath`).
pub(crate) fn is_cauldron(state: BlockState) -> bool {
    in_tag(state, "minecraft:cauldrons")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heap_of(fs: &[f32]) -> (Heap, Vec<Node>) {
        let mut nodes: Vec<Node> = fs
            .iter()
            .enumerate()
            .map(|(index, f)| Node {
                f: *f,
                ..Node::new(index as i32, 0, 0)
            })
            .collect();
        let mut heap = Heap::default();
        for index in 0..nodes.len() {
            heap.insert(&mut nodes, index);
        }
        (heap, nodes)
    }

    fn drain(heap: &mut Heap, nodes: &mut [Node]) -> Vec<usize> {
        let mut order = Vec::new();
        while !heap.items.is_empty() {
            order.push(heap.pop(nodes));
        }
        order
    }

    #[test]
    fn equal_costs_pop_in_the_order_the_heap_s_array_leaves_them() {
        // The last is moved to the top at a pop and moves past no equal: of three equal
        // nodes the first, then the last, then the second.
        let (mut heap, mut nodes) = heap_of(&[1.0, 1.0, 1.0]);
        assert_eq!(drain(&mut heap, &mut nodes), [0, 2, 1]);
        // Two equal children: the right one is taken.
        let (mut heap, mut nodes) = heap_of(&[0.0, 2.0, 2.0, 3.0]);
        assert_eq!(heap.pop(&mut nodes), 0);
        assert_eq!(heap.items, [2, 1, 3]);
    }

    #[test]
    fn a_cheaper_cost_moves_a_node_up_and_a_dearer_one_down() {
        let (mut heap, mut nodes) = heap_of(&[1.0, 2.0, 3.0]);
        heap.change_cost(&mut nodes, 2, 0.5);
        assert_eq!(heap.items[0], 2);
        heap.change_cost(&mut nodes, 2, 9.0);
        assert_eq!(drain(&mut heap, &mut nodes), [0, 1, 2]);
    }

    #[test]
    fn two_cells_of_one_search_never_share_a_hash() {
        let mut seen = std::collections::BTreeSet::new();
        for x in -40..40 {
            for z in -40..40 {
                for y in [-64, -1, 0, 1, 100, 191] {
                    assert!(seen.insert(node_hash(x, y, z)), "{x} {y} {z}");
                }
            }
        }
    }
}
