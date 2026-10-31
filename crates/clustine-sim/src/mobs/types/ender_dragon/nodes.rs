//! The dragon's graph of 24 nodes about the fight's origin and its search over them
//! (`EnderDragon.findClosestNode`, `findPath`, `reconstructPath`; d EnderDragon.txt
//! 2089–2683), with the game's `BinaryHeap` (`net.minecraft.world.level.pathfinder`).
//!
//! The nodes are three rings about (0, 0): twelve 60 blocks out, eight 40 out and four
//! 20 out, each at the column's `MOTION_BLOCKING_NO_LEAVES` height and 5 above it (15 on
//! the middle ring), never below 73. They are worked out once, at the first search, and
//! kept for the dragon's life; the edges between them are the game's bit sets.

use serde::{Deserialize, Serialize};

use crate::mobs::ai::maths::{floor_f32, sqrt_f32};

/// The number of nodes.
pub(crate) const NODE_COUNT: usize = 24;

/// The first node of the middle ring: a search without crystals keeps to the two inner
/// rings, from here on.
pub(crate) const INNER: usize = 12;

/// `nodeAdjacency` (2257–2490): node `i` reaches node `j` where bit `j` of entry `i` is
/// set.
pub(crate) const ADJACENCY: [i32; NODE_COUNT] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131392, 131712, 263424, 526848, 525313, 1581057,
    3166214, 2138120, 6373424, 4358208, 12910976, 9044480, 9706496, 15216640, 13688832, 11763712,
    8257536,
];

/// A node's cell.
pub(crate) type Node = [i32; 3];

/// `float` π as the bytecode has it (`-3.1415927f`).
const PI: f32 = std::f32::consts::PI;

/// The nodes as the first `findClosestNode()` works them out (2089–2256), the height of
/// each column read by `height` (`getHeightmapPos(MOTION_BLOCKING_NO_LEAVES, …).getY()`).
pub(crate) fn make_nodes(height: impl Fn(i32, i32) -> i32) -> Vec<Node> {
    (0..NODE_COUNT)
        .map(|i| {
            let mut above = 5;
            // `radius · Mth.cos(2 · (-π + step · k))`, floored as a float; the steps are
            // the bytecode's `0.2617994f`, `0.3926991f` and `0.7853982f`, the last two
            // the same floats as π / 8 and π / 4.
            let (radius, step, k) = if i < 12 {
                (60.0_f32, 0.261_799_4_f32, i)
            } else if i < 20 {
                above += 10;
                (40.0, std::f32::consts::FRAC_PI_8, i - 12)
            } else {
                (20.0, std::f32::consts::FRAC_PI_4, i - 20)
            };
            let angle = f64::from(2.0 * (-PI + step * k as f32));
            let x = floor_f32(radius * clustine_noise::trig::cos(angle));
            let z = floor_f32(radius * clustine_noise::trig::sin(angle));
            let y = 73.max(height(x, z) + above);
            [x, y, z]
        })
        .collect()
}

/// `Node.distanceToSqr(Node)`: the int differences as floats, squared and summed.
fn distance_sqr(a: Node, b: Node) -> f32 {
    let dx = (b[0] - a[0]) as f32;
    let dy = (b[1] - a[1]) as f32;
    let dz = (b[2] - a[2]) as f32;
    dx * dx + dy * dy + dz * dz
}

/// `Node.distanceTo(Node)`: the float square root of [`distance_sqr`].
fn distance(a: Node, b: Node) -> f32 {
    sqrt_f32(distance_sqr(a, b))
}

/// `findClosestNode(x, y, z)` (2358–2414): of the nodes from `first` on (12 without a
/// fight or without crystals, else 0), the one nearest the cell of `at`, the earlier of
/// two as near, and none nearer than 100 blocks taken as node 0.
pub(crate) fn closest(nodes: &[Node], first: usize, at: [f64; 3]) -> usize {
    let probe = [
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    ];
    let mut least = 10_000.0_f32;
    let mut found = 0;
    for (index, node) in nodes.iter().enumerate().skip(first) {
        let d = distance_sqr(*node, probe);
        if d < least {
            least = d;
            found = index;
        }
    }
    found
}

/// A path of nodes (`Path`), whole as it was made, and how far along it the dragon is
/// (`nextNodeIndex`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Path {
    pub(crate) nodes: Vec<Node>,
    pub(crate) next: u32,
}

impl Path {
    /// `isDone()`.
    pub(crate) fn is_done(&self) -> bool {
        self.next as usize >= self.nodes.len()
    }

    /// `advance()`.
    pub(crate) fn advance(&mut self) {
        self.next += 1;
    }

    /// `getNextNodePos()`.
    pub(crate) fn next_node(&self) -> Node {
        self.nodes[self.next as usize]
    }
}

/// A node as the search keeps it (`Node`'s fields `g`, `h`, `f`, `heapIdx`,
/// `cameFrom`, `closed`).
#[derive(Debug, Clone, Copy)]
struct Searched {
    g: f32,
    h: f32,
    f: f32,
    heap_index: i32,
    came_from: Option<usize>,
    closed: bool,
}

/// `a < b`, false where either is not a number, as the heap's `fcmpg` tests are.
fn less(a: f32, b: f32) -> bool {
    a < b
}

/// The game's `BinaryHeap` of the search's nodes by `f`: it never moves a node past an
/// equal one, and breaks a tie between two children towards the right.
#[derive(Debug, Default)]
struct Heap {
    items: Vec<usize>,
}

impl Heap {
    fn insert(&mut self, nodes: &mut [Searched], node: usize) {
        let at = self.items.len();
        self.items.push(node);
        nodes[node].heap_index = at as i32;
        self.up(nodes, at);
    }

    fn pop(&mut self, nodes: &mut [Searched]) -> usize {
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

    fn change_cost(&mut self, nodes: &mut [Searched], node: usize, f: f32) {
        let old = nodes[node].f;
        nodes[node].f = f;
        let at = nodes[node].heap_index as usize;
        if f < old {
            self.up(nodes, at);
        } else {
            self.down(nodes, at);
        }
    }

    fn up(&mut self, nodes: &mut [Searched], mut at: usize) {
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

    fn down(&mut self, nodes: &mut [Searched], mut at: usize) {
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

/// `findPath(from, to, finalNode)` (2415–2646): A* over the graph from node `from` to
/// node `to`, the neighbours of each node taken from `first` on (as for
/// [`closest`]), with `last` after `to` where it is given. Where `to` is not reached, the
/// path ends at the node found nearest it (then `last`), or there is none where that is
/// `from` itself.
pub(crate) fn find_path(
    nodes: &[Node],
    first: usize,
    from: usize,
    to: usize,
    last: Option<Node>,
) -> Option<Path> {
    let mut searched = vec![
        Searched {
            g: 0.0,
            h: 0.0,
            f: 0.0,
            heap_index: -1,
            came_from: None,
            closed: false,
        };
        NODE_COUNT
    ];
    let end = nodes[to];
    searched[from].g = 0.0;
    searched[from].h = distance(nodes[from], end);
    searched[from].f = searched[from].h;
    let mut heap = Heap::default();
    heap.insert(&mut searched, from);
    let mut closest = from;
    while !heap.items.is_empty() {
        let current = heap.pop(&mut searched);
        // `Node.equals`: the same cell.
        if nodes[current] == end {
            return Some(reconstruct(nodes, &searched, current, last));
        }
        if distance(nodes[current], end) < distance(nodes[closest], end) {
            closest = current;
        }
        searched[current].closed = true;
        for next in first..NODE_COUNT {
            if ADJACENCY[current] & (1 << next) <= 0 || searched[next].closed {
                continue;
            }
            let g = searched[current].g + distance(nodes[current], nodes[next]);
            let open = searched[next].heap_index >= 0;
            if open && g >= searched[next].g {
                continue;
            }
            searched[next].came_from = Some(current);
            searched[next].g = g;
            searched[next].h = distance(nodes[next], end);
            let f = g + searched[next].h;
            if open {
                heap.change_cost(&mut searched, next, f);
            } else {
                searched[next].f = f;
                heap.insert(&mut searched, next);
            }
        }
    }
    if closest == from {
        return None;
    }
    Some(reconstruct(nodes, &searched, closest, last))
}

/// `reconstructPath(start, end)`: the nodes from the start to `end` by the way each was
/// reached, then `last` after them where there is one.
fn reconstruct(nodes: &[Node], searched: &[Searched], end: usize, last: Option<Node>) -> Path {
    let mut path = vec![nodes[end]];
    let mut at = end;
    while let Some(previous) = searched[at].came_from {
        path.push(nodes[previous]);
        at = previous;
    }
    path.reverse();
    path.extend(last);
    Path {
        nodes: path,
        next: 0,
    }
}
