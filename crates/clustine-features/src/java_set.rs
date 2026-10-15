// Adapted from SteelMC's `steel-core/src/worldgen/feature/vanilla_collections.rs`,
// restructured as a port of OpenJDK 25's `java.util.HashMap` (its table, bins, tree
// bins and red-black balancing), which this follows line for line.

//! `java.util.HashSet<BlockPos>` as JDK 25 iterates it. Trees, fallen trees and
//! vegetation patches collect positions in such a set and place in its order, so the
//! order is part of what they make.
//!
//! The set is a `HashMap`: a table of bins, a power of two long from 16, doubled when
//! the set holds more than three quarters of it. A bin is a list of entries in the
//! order they came, or, once it holds more than eight and the table is at least 64
//! long, a red-black tree whose entries also keep a list; its root is moved to the
//! front of that list, and a new entry goes in the list after its parent in the tree.
//! Iteration walks the table and each bin's list. A doubling splits each bin in two,
//! keeping the order, and a tree that ends with six or fewer becomes a list again; a
//! removal turns a tree that has become too small into a list.
//!
//! Positions whose whole hashes are equal and that share a tree are ordered in Java by
//! `System.identityHashCode`, which no port can know (`BlockPos` is not `Comparable`
//! to itself, so the map cannot compare them). Here they are ordered by y, z and x,
//! as `Vec3i.compareTo` would; such ties need two positions 31 apart in x one apart in
//! y, or the like, in one tree bin, and the game's own order is not repeatable there.

use std::cmp::Ordering;

use clustine_world::BlockPos;

const INITIAL_CAPACITY: usize = 16;
const TREEIFY_THRESHOLD: usize = 8;
const UNTREEIFY_THRESHOLD: usize = 6;
const MIN_TREEIFY_CAPACITY: usize = 64;

type Link = Option<usize>;

/// An entry: `HashMap.Node`, or `TreeNode` where `tree` is set.
#[derive(Debug, Clone, Copy)]
struct Node {
    pos: BlockPos,
    hash: i32,
    next: Link,
    tree: bool,
    prev: Link,
    parent: Link,
    left: Link,
    right: Link,
    red: bool,
}

/// A `HashSet<BlockPos>` with Java's iteration order.
#[derive(Debug, Clone, Default)]
pub struct JavaSet {
    /// The heads of the bins; empty until the first insertion.
    table: Vec<Link>,
    nodes: Vec<Node>,
    /// Entries removed, whose slots are taken again.
    free: Vec<usize>,
    size: usize,
    threshold: usize,
}

/// `BlockPos.hashCode` spread as `HashMap.hash` spreads it.
fn hash(pos: BlockPos) -> i32 {
    let h = pos
        .y
        .wrapping_add(pos.z.wrapping_mul(31))
        .wrapping_mul(31)
        .wrapping_add(pos.x);
    h ^ ((h as u32) >> 16) as i32
}

/// The order of two positions of equal hash in a tree (see the module's notes).
fn tie_break(a: BlockPos, b: BlockPos) -> Ordering {
    a.y.cmp(&b.y)
        .then_with(|| a.z.cmp(&b.z))
        .then_with(|| a.x.cmp(&b.x))
}

impl JavaSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    fn index(&self, hash: i32) -> usize {
        (hash as u32 as usize) & (self.table.len() - 1)
    }

    fn new_node(&mut self, pos: BlockPos, hash: i32, next: Link) -> usize {
        let node = Node {
            pos,
            hash,
            next,
            tree: false,
            prev: None,
            parent: None,
            left: None,
            right: None,
            red: false,
        };
        if let Some(slot) = self.free.pop() {
            self.nodes[slot] = node;
            slot
        } else {
            self.nodes.push(node);
            self.nodes.len() - 1
        }
    }

    /// The entry of `pos`, if the set has it.
    fn find(&self, pos: BlockPos) -> Link {
        if self.table.is_empty() {
            return None;
        }
        let hash = hash(pos);
        let mut link = self.table[self.index(hash)];
        while let Some(node) = link {
            if self.nodes[node].hash == hash && self.nodes[node].pos == pos {
                return Some(node);
            }
            link = self.nodes[node].next;
        }
        None
    }

    pub fn contains(&self, pos: BlockPos) -> bool {
        self.find(pos).is_some()
    }

    /// The positions in Java's order of iteration.
    pub fn iter(&self) -> impl Iterator<Item = BlockPos> + '_ {
        self.table.iter().flat_map(move |head| Chain {
            set: self,
            link: *head,
        })
    }

    /// Adds `pos`; false if the set had it (`HashMap.putVal`).
    pub fn insert(&mut self, pos: BlockPos) -> bool {
        if self.table.is_empty() {
            self.resize();
        }
        let hash = hash(pos);
        let index = self.index(hash);
        match self.table[index] {
            None => {
                let node = self.new_node(pos, hash, None);
                self.table[index] = Some(node);
            }
            Some(first) => {
                if self.nodes[first].hash == hash && self.nodes[first].pos == pos {
                    return false;
                }
                if self.nodes[first].tree {
                    if !self.put_tree_val(index, hash, pos) {
                        return false;
                    }
                } else {
                    let mut p = first;
                    let mut bin_count = 0;
                    loop {
                        match self.nodes[p].next {
                            None => {
                                let node = self.new_node(pos, hash, None);
                                self.nodes[p].next = Some(node);
                                if bin_count >= TREEIFY_THRESHOLD - 1 {
                                    self.treeify_bin(hash);
                                }
                                break;
                            }
                            Some(e) => {
                                if self.nodes[e].hash == hash && self.nodes[e].pos == pos {
                                    return false;
                                }
                                p = e;
                            }
                        }
                        bin_count += 1;
                    }
                }
            }
        }
        self.size += 1;
        if self.size > self.threshold {
            self.resize();
        }
        true
    }

    /// Removes `pos`; false if the set did not have it (`HashSet.remove`, which is
    /// `HashMap.removeNode` with `movable` true).
    pub fn remove(&mut self, pos: BlockPos) -> bool {
        let Some(node) = self.find(pos) else {
            return false;
        };
        self.remove_node(node, true);
        true
    }

    /// Takes the first position of the set's order away and returns it, as an iterator's
    /// `remove` does after its first `next` (`HashIterator.remove`). That passes
    /// `movable` false, so a tree bin keeps its form and its order: neither turned back
    /// into a list where it has grown small nor its root moved to the front, which a
    /// `remove` of the same position would do.
    pub fn remove_first(&mut self) -> Option<BlockPos> {
        let first = self.table.iter().find_map(|bin| *bin)?;
        let pos = self.nodes[first].pos;
        self.remove_node(first, false);
        Some(pos)
    }

    fn remove_node(&mut self, node: usize, movable: bool) {
        let index = self.index(self.nodes[node].hash);
        if self.nodes[node].tree {
            self.remove_tree_node(node, index, movable);
        } else {
            let first = self.table[index].expect("the bin of a node found in it");
            if first == node {
                self.table[index] = self.nodes[node].next;
            } else {
                let mut p = first;
                while self.nodes[p].next != Some(node) {
                    p = self.nodes[p].next.expect("the node is in its bin's list");
                }
                self.nodes[p].next = self.nodes[node].next;
            }
        }
        self.free.push(node);
        self.size -= 1;
    }

    /// `HashMap.resize`: a table of 16 at first, then twice as long, each bin split
    /// into the one at its index and the one an old length further on.
    fn resize(&mut self) {
        let old_capacity = self.table.len();
        let new_capacity = if old_capacity == 0 {
            INITIAL_CAPACITY
        } else {
            old_capacity * 2
        };
        self.threshold = new_capacity / 4 * 3;
        let old_table = std::mem::replace(&mut self.table, vec![None; new_capacity]);
        for (j, head) in old_table.into_iter().enumerate() {
            let Some(e) = head else {
                continue;
            };
            if self.nodes[e].next.is_none() {
                let index = self.index(self.nodes[e].hash);
                self.table[index] = Some(e);
            } else if self.nodes[e].tree {
                self.split(e, j, old_capacity);
            } else {
                let (mut lo_head, mut lo_tail, mut hi_head, mut hi_tail): (Link, Link, Link, Link) =
                    (None, None, None, None);
                let mut link = Some(e);
                while let Some(node) = link {
                    link = self.nodes[node].next;
                    let (head, tail) = if self.nodes[node].hash as u32 as usize & old_capacity == 0
                    {
                        (&mut lo_head, &mut lo_tail)
                    } else {
                        (&mut hi_head, &mut hi_tail)
                    };
                    match *tail {
                        None => *head = Some(node),
                        Some(t) => self.nodes[t].next = Some(node),
                    }
                    *tail = Some(node);
                }
                if let Some(t) = lo_tail {
                    self.nodes[t].next = None;
                    self.table[j] = lo_head;
                }
                if let Some(t) = hi_tail {
                    self.nodes[t].next = None;
                    self.table[j + old_capacity] = hi_head;
                }
            }
        }
    }

    /// `HashMap.treeifyBin`: a table too short is doubled instead; else the bin's
    /// list becomes tree nodes in its order, and a tree.
    fn treeify_bin(&mut self, hash: i32) {
        if self.table.len() < MIN_TREEIFY_CAPACITY {
            self.resize();
            return;
        }
        let index = self.index(hash);
        let Some(head) = self.table[index] else {
            return;
        };
        let mut previous: Link = None;
        let mut link = Some(head);
        while let Some(node) = link {
            let n = &mut self.nodes[node];
            n.tree = true;
            n.prev = previous;
            n.parent = None;
            n.left = None;
            n.right = None;
            n.red = false;
            previous = Some(node);
            link = n.next;
        }
        self.treeify(head);
    }

    /// `TreeNode.treeify`: builds the tree of the list from `head` in its order, then
    /// moves the root to the front of the bin.
    fn treeify(&mut self, head: usize) {
        let mut root: Link = None;
        let mut link = Some(head);
        while let Some(x) = link {
            link = self.nodes[x].next;
            self.nodes[x].left = None;
            self.nodes[x].right = None;
            match root {
                None => {
                    self.nodes[x].parent = None;
                    self.nodes[x].red = false;
                    root = Some(x);
                }
                Some(r) => {
                    let (hash, pos) = (self.nodes[x].hash, self.nodes[x].pos);
                    let mut p = r;
                    loop {
                        let dir = self.direction(hash, pos, p);
                        let next = if dir.is_le() {
                            self.nodes[p].left
                        } else {
                            self.nodes[p].right
                        };
                        match next {
                            Some(next) => p = next,
                            None => {
                                self.nodes[x].parent = Some(p);
                                if dir.is_le() {
                                    self.nodes[p].left = Some(x);
                                } else {
                                    self.nodes[p].right = Some(x);
                                }
                                root = Some(self.balance_insertion(r, x));
                                break;
                            }
                        }
                    }
                }
            }
        }
        if let Some(root) = root {
            self.move_root_to_front(root);
        }
    }

    /// Where a key goes below `p` in a tree: by hash, then by the tie-break.
    fn direction(&self, hash: i32, pos: BlockPos, p: usize) -> Ordering {
        let ph = self.nodes[p].hash;
        if ph > hash {
            Ordering::Less
        } else if ph < hash {
            Ordering::Greater
        } else {
            match tie_break(pos, self.nodes[p].pos) {
                Ordering::Equal => Ordering::Less,
                other => other,
            }
        }
    }

    /// `TreeNode.putTreeVal`, the set having been searched already: the new entry
    /// goes below its place in the tree and after its parent in the list.
    fn put_tree_val(&mut self, index: usize, hash: i32, pos: BlockPos) -> bool {
        let first = self.table[index].expect("a tree bin has a head");
        let root = self.root_of(first);
        // The set is searched by its lists; an entry already there is not added.
        let mut link = Some(first);
        while let Some(node) = link {
            if self.nodes[node].hash == hash && self.nodes[node].pos == pos {
                return false;
            }
            link = self.nodes[node].next;
        }
        let mut p = root;
        loop {
            let dir = self.direction(hash, pos, p);
            let next = if dir.is_le() {
                self.nodes[p].left
            } else {
                self.nodes[p].right
            };
            if let Some(next) = next {
                p = next;
                continue;
            }
            let xp = p;
            let xpn = self.nodes[xp].next;
            let x = self.new_node(pos, hash, xpn);
            self.nodes[x].tree = true;
            if dir.is_le() {
                self.nodes[xp].left = Some(x);
            } else {
                self.nodes[xp].right = Some(x);
            }
            self.nodes[xp].next = Some(x);
            self.nodes[x].parent = Some(xp);
            self.nodes[x].prev = Some(xp);
            if let Some(xpn) = xpn {
                self.nodes[xpn].prev = Some(x);
            }
            let root = self.balance_insertion(root, x);
            self.move_root_to_front(root);
            return true;
        }
    }

    /// The root of the tree `node` is in.
    fn root_of(&self, mut node: usize) -> usize {
        while let Some(parent) = self.nodes[node].parent {
            node = parent;
        }
        node
    }

    /// `HashMap.moveRootToFront`.
    fn move_root_to_front(&mut self, root: usize) {
        let index = self.index(self.nodes[root].hash);
        let first = self.table[index];
        if first == Some(root) {
            return;
        }
        self.table[index] = Some(root);
        let rp = self.nodes[root].prev;
        let rn = self.nodes[root].next;
        if let Some(rn) = rn {
            self.nodes[rn].prev = rp;
        }
        if let Some(rp) = rp {
            self.nodes[rp].next = rn;
        }
        if let Some(first) = first {
            self.nodes[first].prev = Some(root);
        }
        self.nodes[root].next = first;
        self.nodes[root].prev = None;
    }

    /// `TreeNode.untreeify`: the list from `head` as plain entries, in its order.
    fn untreeify(&mut self, head: usize) -> usize {
        let mut link = Some(head);
        while let Some(node) = link {
            let n = &mut self.nodes[node];
            n.tree = false;
            n.prev = None;
            n.parent = None;
            n.left = None;
            n.right = None;
            n.red = false;
            link = n.next;
        }
        head
    }

    /// `TreeNode.split`: a tree bin divided by the bit a doubling adds, each half a
    /// list again if it has six entries or fewer, made a tree anew if both halves
    /// have entries, and left as the same tree if all went one way.
    fn split(&mut self, head: usize, index: usize, bit: usize) {
        let (mut lo_head, mut lo_tail, mut hi_head, mut hi_tail): (Link, Link, Link, Link) =
            (None, None, None, None);
        let (mut lc, mut hc) = (0, 0);
        let mut link = Some(head);
        while let Some(e) = link {
            link = self.nodes[e].next;
            self.nodes[e].next = None;
            let lo = self.nodes[e].hash as u32 as usize & bit == 0;
            let (head, tail, count) = if lo {
                (&mut lo_head, &mut lo_tail, &mut lc)
            } else {
                (&mut hi_head, &mut hi_tail, &mut hc)
            };
            self.nodes[e].prev = *tail;
            match *tail {
                None => *head = Some(e),
                Some(t) => self.nodes[t].next = Some(e),
            }
            *tail = Some(e);
            *count += 1;
        }
        if let Some(lo) = lo_head {
            if lc <= UNTREEIFY_THRESHOLD {
                self.table[index] = Some(self.untreeify(lo));
            } else {
                self.table[index] = Some(lo);
                if hi_head.is_some() {
                    self.treeify(lo);
                }
            }
        }
        if let Some(hi) = hi_head {
            if hc <= UNTREEIFY_THRESHOLD {
                self.table[index + bit] = Some(self.untreeify(hi));
            } else {
                self.table[index + bit] = Some(hi);
                if lo_head.is_some() {
                    self.treeify(hi);
                }
            }
        }
    }

    /// `TreeNode.removeTreeNode`: with `movable`, as `HashSet.remove` has it, a bin
    /// grown small becomes a list again and the root goes to the front of the bin.
    fn remove_tree_node(&mut self, p: usize, index: usize, movable: bool) {
        let mut first = self.table[index];
        let succ = self.nodes[p].next;
        let pred = self.nodes[p].prev;
        match pred {
            None => {
                self.table[index] = succ;
                first = succ;
            }
            Some(pred) => self.nodes[pred].next = succ,
        }
        if let Some(succ) = succ {
            self.nodes[succ].prev = pred;
        }
        let Some(first) = first else {
            return;
        };
        let mut root = first;
        if self.nodes[root].parent.is_some() {
            root = self.root_of(root);
        }
        let too_small = self.nodes[root].right.is_none()
            || match self.nodes[root].left {
                None => true,
                Some(rl) => self.nodes[rl].left.is_none(),
            };
        if movable && too_small {
            self.table[index] = Some(self.untreeify(first));
            return;
        }
        let pl = self.nodes[p].left;
        let pr = self.nodes[p].right;
        let replacement;
        if let (Some(pl), Some(pr)) = (pl, pr) {
            let mut s = pr;
            while let Some(sl) = self.nodes[s].left {
                s = sl;
            }
            let c = self.nodes[s].red;
            self.nodes[s].red = self.nodes[p].red;
            self.nodes[p].red = c;
            let sr = self.nodes[s].right;
            let pp = self.nodes[p].parent;
            if s == pr {
                self.nodes[p].parent = Some(s);
                self.nodes[s].right = Some(p);
            } else {
                let sp = self.nodes[s].parent;
                self.nodes[p].parent = sp;
                if let Some(sp) = sp {
                    if self.nodes[sp].left == Some(s) {
                        self.nodes[sp].left = Some(p);
                    } else {
                        self.nodes[sp].right = Some(p);
                    }
                }
                self.nodes[s].right = Some(pr);
                self.nodes[pr].parent = Some(s);
            }
            self.nodes[p].left = None;
            self.nodes[p].right = sr;
            if let Some(sr) = sr {
                self.nodes[sr].parent = Some(p);
            }
            self.nodes[s].left = Some(pl);
            self.nodes[pl].parent = Some(s);
            self.nodes[s].parent = pp;
            match pp {
                None => root = s,
                Some(pp) => {
                    if self.nodes[pp].left == Some(p) {
                        self.nodes[pp].left = Some(s);
                    } else {
                        self.nodes[pp].right = Some(s);
                    }
                }
            }
            replacement = sr.unwrap_or(p);
        } else if let Some(pl) = pl {
            replacement = pl;
        } else if let Some(pr) = pr {
            replacement = pr;
        } else {
            replacement = p;
        }
        if replacement != p {
            let pp = self.nodes[p].parent;
            self.nodes[replacement].parent = pp;
            match pp {
                None => {
                    root = replacement;
                    self.nodes[replacement].red = false;
                }
                Some(pp) => {
                    if self.nodes[pp].left == Some(p) {
                        self.nodes[pp].left = Some(replacement);
                    } else {
                        self.nodes[pp].right = Some(replacement);
                    }
                }
            }
            self.nodes[p].left = None;
            self.nodes[p].right = None;
            self.nodes[p].parent = None;
        }
        let r = if self.nodes[p].red {
            root
        } else {
            self.balance_deletion(root, Some(replacement))
        };
        if replacement == p {
            let pp = self.nodes[p].parent;
            self.nodes[p].parent = None;
            if let Some(pp) = pp {
                if self.nodes[pp].left == Some(p) {
                    self.nodes[pp].left = None;
                } else if self.nodes[pp].right == Some(p) {
                    self.nodes[pp].right = None;
                }
            }
        }
        if movable {
            self.move_root_to_front(r);
        }
    }

    fn is_red(&self, link: Link) -> bool {
        link.is_some_and(|node| self.nodes[node].red)
    }

    fn rotate_left(&mut self, mut root: usize, p: usize) -> usize {
        let Some(r) = self.nodes[p].right else {
            return root;
        };
        let rl = self.nodes[r].left;
        self.nodes[p].right = rl;
        if let Some(rl) = rl {
            self.nodes[rl].parent = Some(p);
        }
        let pp = self.nodes[p].parent;
        self.nodes[r].parent = pp;
        match pp {
            None => {
                root = r;
                self.nodes[r].red = false;
            }
            Some(pp) => {
                if self.nodes[pp].left == Some(p) {
                    self.nodes[pp].left = Some(r);
                } else {
                    self.nodes[pp].right = Some(r);
                }
            }
        }
        self.nodes[r].left = Some(p);
        self.nodes[p].parent = Some(r);
        root
    }

    fn rotate_right(&mut self, mut root: usize, p: usize) -> usize {
        let Some(l) = self.nodes[p].left else {
            return root;
        };
        let lr = self.nodes[l].right;
        self.nodes[p].left = lr;
        if let Some(lr) = lr {
            self.nodes[lr].parent = Some(p);
        }
        let pp = self.nodes[p].parent;
        self.nodes[l].parent = pp;
        match pp {
            None => {
                root = l;
                self.nodes[l].red = false;
            }
            Some(pp) => {
                if self.nodes[pp].right == Some(p) {
                    self.nodes[pp].right = Some(l);
                } else {
                    self.nodes[pp].left = Some(l);
                }
            }
        }
        self.nodes[l].right = Some(p);
        self.nodes[p].parent = Some(l);
        root
    }

    /// `TreeNode.balanceInsertion`.
    fn balance_insertion(&mut self, mut root: usize, mut x: usize) -> usize {
        self.nodes[x].red = true;
        loop {
            let Some(mut xp) = self.nodes[x].parent else {
                self.nodes[x].red = false;
                return x;
            };
            if !self.nodes[xp].red {
                return root;
            }
            let Some(mut xpp) = self.nodes[xp].parent else {
                return root;
            };
            let xppl = self.nodes[xpp].left;
            if Some(xp) == xppl {
                let xppr = self.nodes[xpp].right;
                if let Some(xppr) = xppr.filter(|node| self.nodes[*node].red) {
                    self.nodes[xppr].red = false;
                    self.nodes[xp].red = false;
                    self.nodes[xpp].red = true;
                    x = xpp;
                } else {
                    let mut xpp_link = Some(xpp);
                    let mut xp_link = Some(xp);
                    if self.nodes[xp].right == Some(x) {
                        x = xp;
                        root = self.rotate_left(root, x);
                        xp_link = self.nodes[x].parent;
                        xpp_link = xp_link.and_then(|xp| self.nodes[xp].parent);
                    }
                    if let Some(found) = xp_link {
                        xp = found;
                        self.nodes[xp].red = false;
                        if let Some(found) = xpp_link {
                            xpp = found;
                            self.nodes[xpp].red = true;
                            root = self.rotate_right(root, xpp);
                        }
                    }
                }
            } else if let Some(xppl) = xppl.filter(|node| self.nodes[*node].red) {
                self.nodes[xppl].red = false;
                self.nodes[xp].red = false;
                self.nodes[xpp].red = true;
                x = xpp;
            } else {
                let mut xpp_link = Some(xpp);
                let mut xp_link = Some(xp);
                if self.nodes[xp].left == Some(x) {
                    x = xp;
                    root = self.rotate_right(root, x);
                    xp_link = self.nodes[x].parent;
                    xpp_link = xp_link.and_then(|xp| self.nodes[xp].parent);
                }
                if let Some(found) = xp_link {
                    xp = found;
                    self.nodes[xp].red = false;
                    if let Some(found) = xpp_link {
                        xpp = found;
                        self.nodes[xpp].red = true;
                        root = self.rotate_left(root, xpp);
                    }
                }
            }
        }
    }

    /// `TreeNode.balanceDeletion`.
    fn balance_deletion(&mut self, mut root: usize, mut x: Link) -> usize {
        loop {
            let Some(node) = x else {
                return root;
            };
            if node == root {
                return root;
            }
            let Some(mut xp) = self.nodes[node].parent else {
                self.nodes[node].red = false;
                return node;
            };
            if self.nodes[node].red {
                self.nodes[node].red = false;
                return root;
            }
            let xpl = self.nodes[xp].left;
            if xpl == Some(node) {
                let mut xpr = self.nodes[xp].right;
                if self.is_red(xpr) {
                    let sibling = xpr.expect("a red sibling exists");
                    self.nodes[sibling].red = false;
                    self.nodes[xp].red = true;
                    root = self.rotate_left(root, xp);
                    let parent = self.nodes[node].parent;
                    xpr = parent.and_then(|parent| self.nodes[parent].right);
                    if let Some(parent) = parent {
                        xp = parent;
                    }
                }
                match xpr {
                    None => x = Some(xp),
                    Some(sibling) => {
                        let sl = self.nodes[sibling].left;
                        let sr = self.nodes[sibling].right;
                        if !self.is_red(sr) && !self.is_red(sl) {
                            self.nodes[sibling].red = true;
                            x = Some(xp);
                        } else {
                            let mut xpr = Some(sibling);
                            let mut xp_link = Some(xp);
                            if !self.is_red(sr) {
                                if let Some(sl) = sl {
                                    self.nodes[sl].red = false;
                                }
                                self.nodes[sibling].red = true;
                                root = self.rotate_right(root, sibling);
                                xp_link = self.nodes[node].parent;
                                xpr = xp_link.and_then(|xp| self.nodes[xp].right);
                            }
                            if let Some(xpr) = xpr {
                                self.nodes[xpr].red = xp_link.is_some_and(|xp| self.nodes[xp].red);
                                if let Some(sr) = self.nodes[xpr].right {
                                    self.nodes[sr].red = false;
                                }
                            }
                            if let Some(xp) = xp_link {
                                self.nodes[xp].red = false;
                                root = self.rotate_left(root, xp);
                            }
                            x = Some(root);
                        }
                    }
                }
            } else {
                let mut xpl = xpl;
                if self.is_red(xpl) {
                    let sibling = xpl.expect("a red sibling exists");
                    self.nodes[sibling].red = false;
                    self.nodes[xp].red = true;
                    root = self.rotate_right(root, xp);
                    let parent = self.nodes[node].parent;
                    xpl = parent.and_then(|parent| self.nodes[parent].left);
                    if let Some(parent) = parent {
                        xp = parent;
                    }
                }
                match xpl {
                    None => x = Some(xp),
                    Some(sibling) => {
                        let sl = self.nodes[sibling].left;
                        let sr = self.nodes[sibling].right;
                        if !self.is_red(sl) && !self.is_red(sr) {
                            self.nodes[sibling].red = true;
                            x = Some(xp);
                        } else {
                            let mut xpl = Some(sibling);
                            let mut xp_link = Some(xp);
                            if !self.is_red(sl) {
                                if let Some(sr) = sr {
                                    self.nodes[sr].red = false;
                                }
                                self.nodes[sibling].red = true;
                                root = self.rotate_left(root, sibling);
                                xp_link = self.nodes[node].parent;
                                xpl = xp_link.and_then(|xp| self.nodes[xp].left);
                            }
                            if let Some(xpl) = xpl {
                                self.nodes[xpl].red = xp_link.is_some_and(|xp| self.nodes[xp].red);
                                if let Some(sl) = self.nodes[xpl].left {
                                    self.nodes[sl].red = false;
                                }
                            }
                            if let Some(xp) = xp_link {
                                self.nodes[xp].red = false;
                                root = self.rotate_right(root, xp);
                            }
                            x = Some(root);
                        }
                    }
                }
            }
        }
    }
}

/// The entries of one bin, along its list.
struct Chain<'a> {
    set: &'a JavaSet,
    link: Link,
}

impl Iterator for Chain<'_> {
    type Item = BlockPos;

    fn next(&mut self) -> Option<BlockPos> {
        let node = self.link?;
        self.link = self.set.nodes[node].next;
        Some(self.set.nodes[node].pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> BlockPos {
        let mut parts = text.split(',').map(|part| part.parse::<i32>().unwrap());
        let pos = BlockPos::new(
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        assert!(parts.next().is_none());
        pos
    }

    #[test]
    fn the_order_is_java_s_in_every_case_the_jdk_made() {
        let text = include_str!("../tests/data/java_sets.txt");
        let mut lines = text.lines().filter(|line| !line.starts_with('#'));
        let mut cases = 0;
        while let Some(header) = lines.next() {
            assert!(header.starts_with("case "), "{header}");
            let ops = lines.next().unwrap();
            let order = lines.next().unwrap();
            let mut set = JavaSet::new();
            let mut model = std::collections::BTreeSet::new();
            let mut words = ops.split(' ').filter(|word| !word.is_empty());
            while let Some(op) = words.next() {
                let pos = parse(words.next().unwrap());
                match op {
                    "a" => assert_eq!(set.insert(pos), model.insert(pos), "{header}"),
                    "r" => assert_eq!(set.remove(pos), model.remove(&pos), "{header}"),
                    other => panic!("{other}"),
                }
                assert_eq!(set.len(), model.len());
            }
            let expected: Vec<BlockPos> = order
                .split(' ')
                .filter(|word| !word.is_empty())
                .map(parse)
                .collect();
            let got: Vec<BlockPos> = set.iter().collect();
            assert_eq!(got, expected, "{header}");
            for pos in &expected {
                assert!(set.contains(*pos));
            }
            cases += 1;
        }
        assert!(cases >= 40, "{cases} cases");
    }

    #[test]
    fn a_position_is_added_once() {
        let mut set = JavaSet::new();
        assert!(set.is_empty());
        assert!(set.insert(BlockPos::new(1, 2, 3)));
        assert!(!set.insert(BlockPos::new(1, 2, 3)));
        assert_eq!(set.len(), 1);
        assert!(set.remove(BlockPos::new(1, 2, 3)));
        assert!(!set.remove(BlockPos::new(1, 2, 3)));
        assert!(set.is_empty());
        assert_eq!(set.iter().count(), 0);
    }

    #[test]
    fn the_order_after_a_doubling_is_steelmc_s_traced_one() {
        // SteelMC's test of the same order, from the JDK.
        let mut set = JavaSet::new();
        for x in 0..20 {
            assert!(set.insert(BlockPos::new(x, 64, x % 5)));
        }
        let expected = [
            (0, 0),
            (1, 1),
            (2, 2),
            (5, 0),
            (3, 3),
            (6, 1),
            (4, 4),
            (7, 2),
            (10, 0),
            (8, 3),
            (11, 1),
            (9, 4),
            (12, 2),
            (15, 0),
            (13, 3),
            (16, 1),
            (14, 4),
            (17, 2),
            (18, 3),
            (19, 4),
        ]
        .map(|(x, z)| BlockPos::new(x, 64, z));
        assert_eq!(set.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn a_tree_bin_keeps_steelmc_s_traced_order() {
        let mut set = JavaSet::new();
        for i in [
            34, 23, 3, 8, 44, 25, 11, 52, 36, 10, 21, 18, 20, 9, 54, 41, 40, 42, 12, 16, 14, 47,
            35, 6, 46, 31, 53, 5, 13, 7, 51, 29, 33, 0, 24, 32, 22, 1, 27, 37, 30, 50, 48, 43, 4,
            15, 26, 2, 45, 28, 39, 38, 17, 49, 19, 55,
        ] {
            assert!(set.insert(BlockPos::new(i * 64, 0, 0)));
        }
        assert_eq!(
            set.iter().map(|pos| pos.x).collect::<Vec<_>>(),
            [
                1408, 2176, 128, 256, 0, 384, 512, 2816, 1664, 1536, 1920, 1792, 2048, 3328, 3200,
                3072, 2944, 3456, 2304, 2688, 2560, 2432, 640, 1152, 768, 896, 1024, 1280, 1344,
                448, 1472, 192, 64, 320, 1600, 1728, 1984, 2112, 1856, 704, 3008, 3264, 3136, 2880,
                3392, 3520, 2368, 2240, 2624, 2496, 2752, 576, 960, 1088, 1216, 832,
            ]
        );
    }
}
