// Adapted from SteelMC, steel-utils/src/climate/parameter_list.rs and
// steel-utils/src/climate/types.rs at 885c4b3 (AGPL-3.0-or-later, Copyright (C) 2026
// Alve Jeansson and contributors; see NOTICE). Changed for Clustine in October 2026:
// the tree is built in one thread from Clustine's packed table, with the nineteen
// children a node that the 26.3 jar has where SteelMC has six; the bucket size is
// computed in whole numbers; what the game remembers from its last search is an
// argument, and the search without it is the one Clustine uses; a search that gives
// every entry at the smallest distance, and one that is bounded by a guess, were
// added.

//! The search of a parameter list: the entry whose climate is nearest to a climate.
//!
//! The game keeps the entries of a list in a tree of boxes (an R-tree with up to
//! nineteen children a node) and walks it for the entry at the smallest distance. Two things
//! of the game's decide between entries that are equally near, and both are here:
//!
//! - **The order of the tree.** A later entry replaces the one found so far only if
//!   it is strictly nearer, so of several at the smallest distance the first in the
//!   tree's order is found. The tree is therefore built exactly as the game builds
//!   it: the same sorts, the same buckets, the same order of children.
//! - **What the thread found last.** The game starts each search from the entry its
//!   thread found the time before. If that entry is at the smallest distance again,
//!   it stays, whatever the tree's order says. So in the game the biome at an exact
//!   tie depends on which position the same thread asked about before.
//!
//! [`ParameterTree::nearest`] is the search without a memory, a function of the
//! climate alone, and is what Clustine's biomes are made with
//! (`docs/groundwork/terrain-plan.md`, the rule of step T3). Where it and the game
//! differ, the two entries are exactly tied; [`ParameterTree::tied`] gives every entry
//! at the smallest distance so that a test can say so. [`ParameterTree::nearest_after`]
//! is the game's search with its memory handed in, for the tests that replay the
//! game's order.

use std::cmp::Ordering;
use std::sync::OnceLock;

use clustine_data::biome_parameters::{BiomeParameterList, BiomeParameters};
use clustine_world::Biome;

use super::climate::Climate;

/// The six parameters of a climate and the offset of an entry.
const DIMENSIONS: usize = 7;

/// The most children a node has. The game of 26.3 builds its lists with nineteen
/// (`Climate.ParameterList`'s constructor, read in the jar's bytecode); earlier
/// versions, and SteelMC, have six. The number decides the order of the tree and so
/// which of two tied entries is found: with six, the replay of the game's searches
/// (`tests/game.rs`) differed from the game at one column in 3,076 chunks.
const CHILDREN: usize = 19;

/// A box in the space of the seven dimensions: a lower and an upper bound for each.
type Space = [[i64; 2]; DIMENSIONS];

/// A climate as a point in the seven dimensions; a climate has no offset, so the last
/// is zero.
fn point(climate: &Climate) -> [i64; DIMENSIONS] {
    let [t, h, c, e, d, w] = climate.0;
    [t, h, c, e, d, w, 0]
}

fn space_of(entry: &BiomeParameters) -> Space {
    let mut space = [[0; 2]; DIMENSIONS];
    for (into, bounds) in space.iter_mut().zip(entry.bounds) {
        *into = [i64::from(bounds[0]), i64::from(bounds[1])];
    }
    let offset = i64::from(entry.offset);
    space[DIMENSIONS - 1] = [offset, offset];
    space
}

/// The squared distance between a point and a box: nothing for a dimension in which
/// the point lies within the bounds.
#[inline]
fn distance(space: &Space, target: &[i64; DIMENSIONS]) -> i64 {
    let mut sum = 0;
    for (bounds, at) in space.iter().zip(target) {
        let outside = (at - bounds[1]).max(bounds[0] - at).max(0);
        sum += outside * outside;
    }
    sum
}

/// The smallest box that holds all of `spaces`.
fn around<'a>(spaces: impl Iterator<Item = &'a Space>) -> Space {
    let mut all = [[i64::MAX, i64::MIN]; DIMENSIONS];
    for space in spaces {
        for (into, bounds) in all.iter_mut().zip(space) {
            *into = [into[0].min(bounds[0]), into[1].max(bounds[1])];
        }
    }
    all
}

/// The middle of a box in one dimension. The game divides by two as Java does,
/// towards zero, which a midpoint that rounds down would not be for a negative sum.
fn centre(space: &Space, dimension: usize) -> i64 {
    (space[dimension][0] + space[dimension][1]) / 2
}

/// The order the game sorts boxes into for one dimension: by the centre in that
/// dimension, then in the next, and so around all seven. `absolute` compares the
/// centres' magnitudes.
fn by_dimension(a: &Space, b: &Space, dimension: usize, absolute: bool) -> Ordering {
    for step in 0..DIMENSIONS {
        let at = (dimension + step) % DIMENSIONS;
        let (mut left, mut right) = (centre(a, at), centre(b, at));
        if absolute {
            (left, right) = (left.abs(), right.abs());
        }
        let order = left.cmp(&right);
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

/// How many entries go into one bucket when `count` are divided: the largest power
/// of nineteen below `count`. The game computes nineteen to the power of the floor of
/// the logarithm to base nineteen of `count - 0.01`, which is that number; whole
/// numbers give it without the platform's logarithm.
fn bucket_size(count: usize) -> usize {
    let mut size = 1;
    while size * CHILDREN < count {
        size *= CHILDREN;
    }
    size
}

/// A node while the tree is built.
enum Built {
    Leaf { space: Space, entry: u32 },
    Branch { space: Space, children: Vec<Built> },
}

impl Built {
    fn space(&self) -> &Space {
        match self {
            Built::Leaf { space, .. } | Built::Branch { space, .. } => space,
        }
    }

    fn branch(children: Vec<Built>) -> Built {
        Built::Branch {
            space: around(children.iter().map(Built::space)),
            children,
        }
    }
}

/// The game's `RTree.build` over leaves, each an entry's box and its number.
fn build(mut leaves: Vec<(Space, u32)>) -> Built {
    if let [(space, entry)] = leaves[..] {
        return Built::Leaf { space, entry };
    }
    if leaves.len() <= CHILDREN {
        // Few enough for one node: nearest to the origin first. The sorts are stable,
        // as the game's are, so equal keys keep the order they came in.
        leaves.sort_by_key(|(space, _)| {
            (0..DIMENSIONS)
                .map(|dimension| centre(space, dimension).abs())
                .sum::<i64>()
        });
        let children = leaves
            .into_iter()
            .map(|(space, entry)| Built::Leaf { space, entry })
            .collect();
        return Built::branch(children);
    }

    // Sort along each dimension in turn, cut into buckets, and keep the buckets of
    // the dimension whose boxes are smallest in sum. The buckets are kept as they
    // were cut: the order within them is that sort's, and a later sort of the same
    // leaves must not change it.
    let size = bucket_size(leaves.len());
    let mut least = i64::MAX;
    let mut chosen = 0;
    let mut kept: Vec<(Space, Vec<(Space, u32)>)> = Vec::new();
    for dimension in 0..DIMENSIONS {
        leaves.sort_by(|a, b| by_dimension(&a.0, &b.0, dimension, false));
        let buckets: Vec<(Space, Vec<(Space, u32)>)> = leaves
            .chunks(size)
            .map(|bucket| (around(bucket.iter().map(|leaf| &leaf.0)), bucket.to_vec()))
            .collect();
        let cost: i64 = buckets
            .iter()
            .map(|(space, _)| {
                space
                    .iter()
                    .map(|bounds| (bounds[1] - bounds[0]).abs())
                    .sum::<i64>()
            })
            .sum();
        if least > cost {
            least = cost;
            chosen = dimension;
            kept = buckets;
        }
    }
    kept.sort_by(|a, b| by_dimension(&a.0, &b.0, chosen, true));
    Built::branch(kept.into_iter().map(|(_, bucket)| build(bucket)).collect())
}

/// A node of the finished tree. The children of a node lie side by side in
/// [`ParameterTree::nodes`], in the order the game walks them.
struct Node {
    space: Space,
    /// For a leaf, the number of its entry in the list.
    entry: u32,
    first_child: u32,
    /// Nought for a leaf.
    children: u32,
}

/// One of the game's parameter lists, ready to be searched.
pub struct ParameterTree {
    list: BiomeParameterList,
    /// The root is the first.
    nodes: Vec<Node>,
    /// The box of each entry, by its number in the list.
    spaces: Vec<Space>,
    biomes: Vec<Biome>,
}

/// An entry of a list and how far from the climate it was found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Nearest {
    /// The entry's number in the list ([`BiomeParameterList::get`]).
    pub entry: usize,
    pub biome: Biome,
    /// The squared distance, in the game's whole numbers.
    pub distance: i64,
}

impl ParameterTree {
    /// The tree of one of the game's two lists. It is built once in a process and is
    /// the same every time, for it is a function of the list.
    pub fn of(list: BiomeParameterList) -> &'static ParameterTree {
        static TREES: [OnceLock<ParameterTree>; 2] = [OnceLock::new(), OnceLock::new()];
        TREES[list as usize].get_or_init(|| ParameterTree::build(list))
    }

    fn build(list: BiomeParameterList) -> ParameterTree {
        let entries: Vec<BiomeParameters> = list.iter().collect();
        let spaces: Vec<Space> = entries.iter().map(space_of).collect();
        let leaves = spaces
            .iter()
            .enumerate()
            .map(|(entry, space)| (*space, entry as u32))
            .collect();

        // Laid out breadth first, so that the children of a node are neighbours.
        let mut nodes: Vec<Node> = Vec::with_capacity(spaces.len() * 2);
        let mut waiting = std::collections::VecDeque::from([(vec![build(leaves)], None)]);
        while let Some((siblings, parent)) = waiting.pop_front() {
            if let Some(parent) = parent {
                let first = nodes.len() as u32;
                let parent: &mut Node = &mut nodes[parent];
                parent.first_child = first;
            }
            for node in siblings {
                match node {
                    Built::Leaf { space, entry } => nodes.push(Node {
                        space,
                        entry,
                        first_child: 0,
                        children: 0,
                    }),
                    Built::Branch { space, children } => {
                        nodes.push(Node {
                            space,
                            entry: u32::MAX,
                            first_child: 0,
                            children: children.len() as u32,
                        });
                        waiting.push_back((children, Some(nodes.len() - 1)));
                    }
                }
            }
        }

        ParameterTree {
            list,
            nodes,
            spaces,
            biomes: entries
                .iter()
                .map(|entry| Biome(u16::from(entry.biome)))
                .collect(),
        }
    }

    /// The list this is the tree of.
    pub fn list(&self) -> BiomeParameterList {
        self.list
    }

    /// The biome of the entry with this number.
    pub fn biome_of(&self, entry: usize) -> Biome {
        self.biomes[entry]
    }

    /// How far the entry with this number is from a climate.
    pub fn distance_of(&self, entry: usize, climate: &Climate) -> i64 {
        distance(&self.spaces[entry], &point(climate))
    }

    /// The smallest distance from a climate to an entry of `biome`, if the list has
    /// one. This looks at every entry and is for tests and for saying what was found.
    pub fn distance_to(&self, biome: Biome, climate: &Climate) -> Option<i64> {
        let target = point(climate);
        self.spaces
            .iter()
            .zip(&self.biomes)
            .filter(|(_, of)| **of == biome)
            .map(|(space, _)| distance(space, &target))
            .min()
    }

    fn found(&self, entry: u32, distance: i64) -> Nearest {
        Nearest {
            entry: entry as usize,
            biome: self.biomes[entry as usize],
            distance,
        }
    }

    /// The entry nearest to a climate; of several equally near, the first in the
    /// order of the game's tree. A function of the climate alone.
    pub fn nearest(&self, climate: &Climate) -> Nearest {
        self.nearest_after(climate, None)
    }

    /// The game's own search: as [`Self::nearest`], but an entry found `before`
    /// stays if nothing is strictly nearer. The game hands in what the same thread
    /// found last.
    pub fn nearest_after(&self, climate: &Climate, before: Option<usize>) -> Nearest {
        let target = point(climate);
        let root = &self.nodes[0];
        if root.children == 0 {
            // A list of one entry: the game answers with it and measures nothing.
            return self.found(root.entry, distance(&root.space, &target));
        }
        let mut best = match before {
            Some(entry) => (distance(&self.spaces[entry], &target), entry as u32),
            None => (i64::MAX, u32::MAX),
        };
        self.walk(root, &target, &mut best);
        self.found(best.1, best.0)
    }

    /// What [`Self::nearest`] finds, found sooner: `guess` is any entry of the list,
    /// and the nearer it is, the less of the tree is walked. The guess only bounds
    /// the walk and is never the answer for being the guess, so that the answer
    /// stays a function of the climate alone. Whoever asks for a climate close to
    /// the last one hands in what was found then.
    pub fn nearest_guessing(&self, climate: &Climate, guess: usize) -> Nearest {
        let target = point(climate);
        let root = &self.nodes[0];
        if root.children == 0 {
            return self.found(root.entry, distance(&root.space, &target));
        }
        // Nothing farther than the guess can be the nearest, so the walk may start
        // as if something one step farther than the guess had been found already:
        // every box that holds an entry at the smallest distance is then still
        // strictly nearer than what was found, until the first such entry is.
        let mut best = (distance(&self.spaces[guess], &target) + 1, u32::MAX);
        self.walk(root, &target, &mut best);
        self.found(best.1, best.0)
    }

    fn walk(&self, node: &Node, target: &[i64; DIMENSIONS], best: &mut (i64, u32)) {
        let first = node.first_child as usize;
        for child in &self.nodes[first..first + node.children as usize] {
            let near = distance(&child.space, target);
            // Strictly nearer, as in the game: a box at the same distance as what
            // was found is not looked into.
            if best.0 > near {
                if child.children == 0 {
                    *best = (near, child.entry);
                } else {
                    self.walk(child, target, best);
                }
            }
        }
    }

    /// Every entry at the smallest distance from a climate, in the order of the
    /// tree: the first is what [`Self::nearest`] finds, and any of them is what the
    /// game may find, depending on what its thread found before. Where they are not
    /// all of one biome, the game's biome at this climate is not a function of the
    /// position.
    pub fn tied(&self, climate: &Climate) -> Vec<Nearest> {
        let target = point(climate);
        let root = &self.nodes[0];
        if root.children == 0 {
            return vec![self.found(root.entry, distance(&root.space, &target))];
        }
        let mut least = i64::MAX;
        let mut entries = Vec::new();
        self.gather(root, &target, &mut least, &mut entries);
        entries
            .into_iter()
            .map(|entry| self.found(entry, least))
            .collect()
    }

    fn gather(
        &self,
        node: &Node,
        target: &[i64; DIMENSIONS],
        least: &mut i64,
        entries: &mut Vec<u32>,
    ) {
        let first = node.first_child as usize;
        for child in &self.nodes[first..first + node.children as usize] {
            let near = distance(&child.space, target);
            if near > *least {
                continue;
            }
            if child.children != 0 {
                self.gather(child, target, least, entries);
                continue;
            }
            if near < *least {
                *least = near;
                entries.clear();
            }
            entries.push(child.entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numbers that look random and are the same in every run.
    struct Numbers(u64);

    impl Numbers {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        /// A climate anywhere in the range noises reach, and a little beyond.
        fn climate(&mut self) -> Climate {
            Climate(std::array::from_fn(|_| {
                (self.next() % 30_001) as i64 - 15_000
            }))
        }

        /// A climate that sits on bounds of the entry with this number, where ties
        /// are.
        fn climate_on_bounds_of(&mut self, tree: &ParameterTree, entry: usize) -> Climate {
            let space = tree.spaces[entry];
            Climate(std::array::from_fn(|parameter| {
                space[parameter][(self.next() % 2) as usize]
            }))
        }
    }

    /// The smallest distance and every entry at it, by looking at all of them.
    fn by_looking_at_all(tree: &ParameterTree, climate: &Climate) -> (i64, Vec<usize>) {
        let distances: Vec<i64> = (0..tree.spaces.len())
            .map(|entry| tree.distance_of(entry, climate))
            .collect();
        let least = *distances.iter().min().unwrap();
        let entries = (0..distances.len())
            .filter(|entry| distances[*entry] == least)
            .collect();
        (least, entries)
    }

    #[test]
    fn the_lists_have_the_entries_the_game_has() {
        assert_eq!(
            ParameterTree::of(BiomeParameterList::Overworld)
                .spaces
                .len(),
            7594
        );
        assert_eq!(
            ParameterTree::of(BiomeParameterList::Nether).spaces.len(),
            5
        );
    }

    #[test]
    fn a_box_is_at_no_distance_from_a_point_inside_it_and_squared_outside() {
        // The values of SteelMC's `test_parameter_distance` and
        // `test_parameter_point_fitness`.
        let mut space: Space = [[0, 0]; DIMENSIONS];
        space[0] = [-5000, 5000];
        for (at, expected) in [(0, 0), (5000, 0), (-5000, 0), (6000, 1000), (-6000, 1000)] {
            assert_eq!(
                distance(&space, &[at, 0, 0, 0, 0, 0, 0]),
                expected * expected
            );
        }
        let origin: Space = [[0, 0]; DIMENSIONS];
        assert_eq!(distance(&origin, &[100, 100, 0, 0, 0, 0, 0]), 20_000);
        // The offset of an entry counts as a seventh distance.
        let mut offset = origin;
        offset[6] = [300, 300];
        assert_eq!(distance(&offset, &[0; 7]), 90_000);
    }

    #[test]
    fn a_bucket_is_the_largest_power_of_nineteen_below_the_count() {
        // The game's formula, for the counts it is asked about here, by the
        // platform's logarithm; the whole numbers must agree with it.
        for count in 20..=8000_usize {
            let logarithm = ((count as f64) - 0.01).ln() / 19.0_f64.ln();
            let expected = 19.0_f64.powf(logarithm.floor()) as usize;
            assert_eq!(bucket_size(count), expected, "for {count} entries");
        }
        assert_eq!(bucket_size(20), 19);
        assert_eq!(bucket_size(361), 19);
        assert_eq!(bucket_size(362), 361);
        assert_eq!(bucket_size(6859), 361);
        assert_eq!(bucket_size(7594), 6859);
    }

    #[test]
    fn every_entry_is_a_leaf_once_and_no_node_has_more_than_nineteen_children() {
        for list in [BiomeParameterList::Overworld, BiomeParameterList::Nether] {
            let tree = ParameterTree::of(list);
            let mut seen = vec![0; tree.spaces.len()];
            for node in &tree.nodes {
                assert!(node.children as usize <= CHILDREN);
                if node.children == 0 {
                    seen[node.entry as usize] += 1;
                    assert_eq!(node.space, tree.spaces[node.entry as usize]);
                } else {
                    let first = node.first_child as usize;
                    let children = &tree.nodes[first..first + node.children as usize];
                    assert_eq!(
                        node.space,
                        around(children.iter().map(|child| &child.space))
                    );
                }
            }
            assert!(seen.iter().all(|times| *times == 1));
        }
    }

    #[test]
    fn the_tree_finds_the_smallest_distance_that_looking_at_every_entry_finds() {
        let mut numbers = Numbers(13579);
        for list in [BiomeParameterList::Overworld, BiomeParameterList::Nether] {
            let tree = ParameterTree::of(list);
            for _ in 0..2000 {
                let climate = numbers.climate();
                let (least, entries) = by_looking_at_all(tree, &climate);
                let nearest = tree.nearest(&climate);
                assert_eq!(nearest.distance, least);
                assert!(entries.contains(&nearest.entry));
                assert_eq!(nearest.biome, tree.biome_of(nearest.entry));
            }
        }
    }

    #[test]
    fn the_tied_entries_are_all_those_at_the_smallest_distance_and_the_nearest_is_the_first() {
        let mut numbers = Numbers(97531);
        let mut with_several = 0;
        for list in [BiomeParameterList::Overworld, BiomeParameterList::Nether] {
            let tree = ParameterTree::of(list);
            for round in 0..2000 {
                let entry = round * 37 % tree.spaces.len();
                let climate = numbers.climate_on_bounds_of(tree, entry);
                let (least, mut expected) = by_looking_at_all(tree, &climate);
                let tied = tree.tied(&climate);
                assert!(tied.iter().all(|found| found.distance == least));
                assert_eq!(tied[0], tree.nearest(&climate));
                let mut entries: Vec<usize> = tied.iter().map(|found| found.entry).collect();
                entries.sort_unstable();
                expected.sort_unstable();
                assert_eq!(entries, expected);
                with_several += usize::from(entries.len() > 1);
            }
        }
        assert!(
            with_several > 1000,
            "the climates were meant to sit on ties"
        );
    }

    #[test]
    fn what_was_found_before_stays_exactly_when_it_is_tied() {
        let mut numbers = Numbers(24680);
        let tree = ParameterTree::of(BiomeParameterList::Overworld);
        for round in 0..2000 {
            let climate = numbers.climate_on_bounds_of(tree, round * 53 % tree.spaces.len());
            let tied = tree.tied(&climate);
            for found in &tied {
                assert_eq!(tree.nearest_after(&climate, Some(found.entry)), *found);
            }
            // An entry that is farther gives way to the first of the tree's order.
            let far = (0..tree.spaces.len())
                .find(|entry| tree.distance_of(*entry, &climate) > tied[0].distance)
                .unwrap();
            assert_eq!(tree.nearest_after(&climate, Some(far)), tied[0]);
        }
    }

    #[test]
    fn a_guess_never_changes_what_is_found() {
        let mut numbers = Numbers(11235);
        for list in [BiomeParameterList::Overworld, BiomeParameterList::Nether] {
            let tree = ParameterTree::of(list);
            let count = tree.spaces.len();
            for round in 0..3000 {
                // Half of the climates sit on ties, where a guess that was taken
                // for an answer would show.
                let climate = if round % 2 == 0 {
                    numbers.climate()
                } else {
                    numbers.climate_on_bounds_of(tree, round * 31 % count)
                };
                let nearest = tree.nearest(&climate);
                let anywhere = numbers.next() as usize % count;
                assert_eq!(tree.nearest_guessing(&climate, anywhere), nearest);
                for tied in tree.tied(&climate) {
                    assert_eq!(tree.nearest_guessing(&climate, tied.entry), nearest);
                }
            }
        }
    }

    #[test]
    fn the_smallest_distance_to_a_biome_is_over_all_its_entries() {
        let tree = ParameterTree::of(BiomeParameterList::Nether);
        let climate = Climate([0; 6]);
        let nearest = tree.nearest(&climate);
        assert_eq!(
            tree.distance_to(nearest.biome, &climate),
            Some(nearest.distance)
        );
        assert_eq!(tree.distance_to(Biome(u16::MAX), &climate), None);
    }
}
