//! The port of `java.util.HashSet<net.minecraft.core.BlockPos>`'s order of iteration
//! (`clustine_features::java_set::JavaSet`), which trees and patches place in.
//!
//! Written from JDK 25's `HashMap` and the game's `Vec3i.hashCode`, without the port's
//! code. The fixture `tests/data/java_sets.txt` was made by the JDK with the game's
//! own `BlockPos`; besides it, a plain model of `HashMap`'s chained table is worked
//! out here from the JDK's source, checked against the fixture, and then used to judge
//! the port on many more sequences than the fixture holds. The model does not follow
//! a bin once it becomes a tree, so it judges only sequences that never make one.

use std::borrow::Borrow;
use std::collections::BTreeSet;

use clustine_features::java_set::JavaSet;
use clustine_world::BlockPos;

const FIXTURE: &str = include_str!("data/java_sets.txt");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add(BlockPos),
    Remove(BlockPos),
}

struct Case {
    number: usize,
    ops: Vec<Op>,
    order: Vec<BlockPos>,
}

fn position(text: &str) -> BlockPos {
    let parts: Vec<i32> = text
        .split(',')
        .map(|part| {
            part.parse()
                .unwrap_or_else(|_| panic!("{text} is not a position"))
        })
        .collect();
    assert_eq!(parts.len(), 3, "{text} is not a position");
    BlockPos::new(parts[0], parts[1], parts[2])
}

fn cases() -> Vec<Case> {
    let lines: Vec<&str> = FIXTURE
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect();
    let mut cases = Vec::new();
    for chunk in lines.chunks(3) {
        let [head, ops, order] = chunk else {
            panic!("a case is three lines: {chunk:?}");
        };
        let number = head
            .strip_prefix("case ")
            .and_then(|number| number.parse().ok())
            .unwrap_or_else(|| panic!("{head} does not start a case"));
        let words: Vec<&str> = ops.split_whitespace().collect();
        let ops = words
            .chunks(2)
            .map(|op| match op {
                ["a", at] => Op::Add(position(at)),
                ["r", at] => Op::Remove(position(at)),
                _ => panic!("{op:?} is not an operation"),
            })
            .collect();
        let order = order.split_whitespace().map(position).collect();
        cases.push(Case { number, ops, order });
    }
    assert!(cases.len() >= 40, "the fixture holds {} cases", cases.len());
    cases
}

/// The positions a set gives, in its order, whether it gives them by value or by
/// reference.
fn order<I>(items: I) -> Vec<BlockPos>
where
    I: IntoIterator,
    I::Item: Borrow<BlockPos>,
{
    items.into_iter().map(|at| *at.borrow()).collect()
}

/// `Vec3i.hashCode`: `(y + z * 31) * 31 + x`, in Java's wrapping `int`.
fn hash(at: BlockPos) -> i32 {
    at.y.wrapping_add(at.z.wrapping_mul(31))
        .wrapping_mul(31)
        .wrapping_add(at.x)
}

/// `HashMap.hash`: `h ^ (h >>> 16)`.
fn spread(at: BlockPos) -> u32 {
    let h = hash(at) as u32;
    h ^ (h >> 16)
}

/// JDK 25's `HashMap` as long as no bin is a tree: a table of chained bins, made
/// with sixteen at the first insertion, doubled when the size passes three quarters
/// of it or when a bin of a table smaller than 64 would grow past eight. A doubling
/// splits each bin in two, keeping the order within each. A new key goes to the end
/// of its bin.
struct Model {
    table: Vec<Vec<BlockPos>>,
    size: usize,
    /// Whether a bin has become a tree, which this model does not follow.
    treed: bool,
}

impl Model {
    fn new() -> Self {
        Self {
            table: Vec::new(),
            size: 0,
            treed: false,
        }
    }

    fn resize(&mut self) {
        let old = self.table.len();
        if old == 0 {
            self.table = vec![Vec::new(); 16];
            return;
        }
        let mut table = vec![Vec::new(); old * 2];
        for (index, bin) in self.table.drain(..).enumerate() {
            for at in bin {
                let high = spread(at) as usize & old != 0;
                table[if high { index + old } else { index }].push(at);
            }
        }
        self.table = table;
    }

    fn bin(&self, at: BlockPos) -> usize {
        spread(at) as usize & (self.table.len() - 1)
    }

    fn insert(&mut self, at: BlockPos) -> bool {
        if self.table.is_empty() {
            self.resize();
        }
        let index = self.bin(at);
        if self.table[index].contains(&at) {
            return false;
        }
        let before = self.table[index].len();
        self.table[index].push(at);
        // `putVal` calls `treeifyBin` when the bin it walked held eight or more.
        if before >= 8 {
            if self.table.len() < 64 {
                self.resize();
            } else {
                self.treed = true;
            }
        }
        self.size += 1;
        if self.size > self.table.len() * 3 / 4 {
            self.resize();
        }
        true
    }

    fn remove(&mut self, at: BlockPos) -> bool {
        if self.table.is_empty() {
            return false;
        }
        let index = self.bin(at);
        let Some(place) = self.table[index].iter().position(|kept| *kept == at) else {
            return false;
        };
        self.table[index].remove(place);
        self.size -= 1;
        true
    }

    fn order(&self) -> Vec<BlockPos> {
        self.table.iter().flatten().copied().collect()
    }
}

/// Applies every operation to the port and to a sorted model, asking each answer.
fn apply_checked(ops: &[Op], label: &str) -> JavaSet {
    let mut set = JavaSet::new();
    let mut sorted = BTreeSet::new();
    for (step, op) in ops.iter().enumerate() {
        match *op {
            Op::Add(at) => {
                assert_eq!(
                    set.insert(at),
                    sorted.insert(at),
                    "{label}, operation {step}: inserting {at:?}"
                );
            }
            Op::Remove(at) => {
                assert_eq!(
                    set.remove(at),
                    sorted.remove(&at),
                    "{label}, operation {step}: removing {at:?}"
                );
            }
        }
        assert_eq!(set.len(), sorted.len(), "{label}, operation {step}: len");
        assert_eq!(set.is_empty(), sorted.is_empty(), "{label}, {step}");
    }
    for at in &sorted {
        assert!(set.contains(*at), "{label}: {at:?} is in the set");
    }
    for op in ops {
        let (Op::Add(at) | Op::Remove(at)) = *op;
        assert_eq!(set.contains(at), sorted.contains(&at), "{label}: {at:?}");
    }
    let mut given = order(set.iter());
    given.sort();
    assert_eq!(given, sorted.into_iter().collect::<Vec<_>>(), "{label}");
    set
}

/// A small generator of numbers, so that the sequences are the same on every run.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> i32 {
        i32::try_from(self.next() % bound).unwrap()
    }
}

/// Sequences shaped like a tree's or a patch's: positions in a box around a centre,
/// near the origin or far from it, some removed again.
fn sequences() -> Vec<Vec<Op>> {
    let mut numbers = Numbers(0x2026_1015_0026_0003);
    let mut sequences = Vec::new();
    for round in 0..300 {
        let size = 1 + numbers.below(if round % 3 == 0 { 400 } else { 60 });
        let (cx, cy, cz) = if round % 2 == 0 {
            (
                numbers.below(60_000_000) - 30_000_000,
                numbers.below(384) - 64,
                numbers.below(60_000_000) - 30_000_000,
            )
        } else {
            (
                numbers.below(64) - 32,
                numbers.below(384) - 64,
                numbers.below(64) - 32,
            )
        };
        let reach = 2 + numbers.below(12);
        let mut ops = Vec::new();
        let mut added = Vec::new();
        for _ in 0..size {
            let at = BlockPos::new(
                cx + numbers.below(2 * reach as u64 + 1) - reach,
                cy + numbers.below(2 * reach as u64 + 1) - reach,
                cz + numbers.below(2 * reach as u64 + 1) - reach,
            );
            ops.push(Op::Add(at));
            added.push(at);
            if round % 4 == 1 && numbers.below(3) == 0 {
                let gone = added[numbers.below(added.len() as u64) as usize];
                ops.push(Op::Remove(gone));
            }
        }
        sequences.push(ops);
    }
    sequences
}

fn model_of(ops: &[Op]) -> Model {
    let mut model = Model::new();
    for op in ops {
        match *op {
            Op::Add(at) => {
                model.insert(at);
            }
            Op::Remove(at) => {
                model.remove(at);
            }
        }
    }
    model
}

#[test]
fn every_case_of_the_fixture_iterates_in_the_jdks_order() {
    for case in cases() {
        let set = apply_checked(&case.ops, &format!("case {}", case.number));
        assert_eq!(
            order(set.iter()),
            case.order,
            "case {}: the order of iteration",
            case.number
        );
    }
}

#[test]
fn the_answers_agree_with_a_sorted_set_through_every_case_of_the_fixture() {
    for case in cases() {
        apply_checked(&case.ops, &format!("case {}", case.number));
    }
}

#[test]
fn inserting_a_position_already_there_returns_false_and_changes_nothing() {
    for case in cases() {
        let mut set = apply_checked(&case.ops, &format!("case {}", case.number));
        let before = order(set.iter());
        for at in before.clone() {
            assert!(!set.insert(at), "case {}: {at:?} again", case.number);
        }
        assert_eq!(order(set.iter()), before, "case {}", case.number);
        assert_eq!(set.len(), before.len(), "case {}", case.number);
    }
}

#[test]
fn removing_a_position_not_there_returns_false_and_changes_nothing() {
    let mut set = JavaSet::new();
    assert!(!set.remove(BlockPos::new(1, 2, 3)));
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    for x in 0..20 {
        assert!(set.insert(BlockPos::new(x, 64, -x)));
    }
    let before = order(set.iter());
    assert!(!set.remove(BlockPos::new(0, 64, 1)));
    assert!(!set.contains(BlockPos::new(0, 64, 1)));
    assert_eq!(order(set.iter()), before);
}

#[test]
fn an_empty_set_gives_nothing() {
    let set = JavaSet::new();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    assert!(order(set.iter()).is_empty());
    assert!(!set.contains(BlockPos::new(0, 0, 0)));
}

#[test]
fn the_model_of_the_jdks_table_agrees_with_the_fixture_where_no_bin_is_a_tree() {
    // Checks the model the next tests judge by, against the JDK itself.
    let mut judged = 0;
    for case in cases() {
        let model = model_of(&case.ops);
        if model.treed {
            continue;
        }
        judged += 1;
        assert_eq!(model.order(), case.order, "case {}", case.number);
    }
    assert!(judged >= 40, "the model judged only {judged} cases");
}

#[test]
fn many_sequences_iterate_as_the_jdks_table_does() {
    let mut judged = 0;
    for (number, ops) in sequences().iter().enumerate() {
        let label = format!("sequence {number}");
        let set = apply_checked(ops, &label);
        let model = model_of(ops);
        if model.treed {
            continue;
        }
        judged += 1;
        assert_eq!(order(set.iter()), model.order(), "{label}");
    }
    assert!(judged >= 250, "the model judged only {judged} sequences");
}

#[test]
fn removing_and_adding_again_moves_a_position_to_the_end_of_its_bin() {
    // Positions whose spread hashes share their low four bits share a bin of the
    // first table; re-adding the first puts it after the others.
    let base = BlockPos::new(3, 70, -2);
    let bin = spread(base) & 15;
    let mut same = vec![base];
    let mut x = 4;
    while same.len() < 4 {
        let at = BlockPos::new(x, 70, -2);
        if spread(at) & 15 == bin {
            same.push(at);
        }
        x += 1;
    }
    let mut set = JavaSet::new();
    for at in &same {
        assert!(set.insert(*at));
    }
    assert_eq!(order(set.iter()), same);
    assert!(set.remove(base));
    assert!(set.insert(base));
    let mut want = same[1..].to_vec();
    want.push(base);
    assert_eq!(order(set.iter()), want);
}

#[test]
fn fewer_than_thirteen_positions_in_distinct_bins_iterate_by_the_low_bits_of_the_spread_hash() {
    let mut numbers = Numbers(0x0000_5eed_0000_0013);
    for round in 0..200 {
        let wanted = 1 + numbers.below(12) as usize;
        let far = round % 2 == 0;
        let mut chosen: Vec<BlockPos> = Vec::new();
        while chosen.len() < wanted {
            let at = if far {
                BlockPos::new(
                    numbers.below(60_000_000) - 30_000_000,
                    numbers.below(384) - 64,
                    numbers.below(60_000_000) - 30_000_000,
                )
            } else {
                BlockPos::new(
                    numbers.below(32) - 16,
                    numbers.below(32) + 60,
                    numbers.below(32) - 16,
                )
            };
            if chosen
                .iter()
                .all(|kept| spread(*kept) & 15 != spread(at) & 15)
            {
                chosen.push(at);
            }
        }
        let mut set = JavaSet::new();
        for at in &chosen {
            assert!(set.insert(*at));
        }
        let mut want = chosen.clone();
        want.sort_by_key(|at| spread(*at) & 15);
        assert_eq!(order(set.iter()), want, "round {round}: {chosen:?}");
    }
}

#[test]
fn the_hash_of_a_position_is_the_games() {
    // `Vec3i.hashCode`, worked by hand, so that the model above rests on it.
    assert_eq!(hash(BlockPos::new(0, 0, 0)), 0);
    assert_eq!(hash(BlockPos::new(1, 2, 3)), (2 + 3 * 31) * 31 + 1);
    assert_eq!(hash(BlockPos::new(-5, 70, -9)), (70 - 9 * 31) * 31 - 5);
    // Java's int wraps: 30 000 000 * 31 * 31 is past `i32::MAX`.
    let far = BlockPos::new(30_000_000, 0, 30_000_000);
    let wide = (i64::from(far.z) * 31 * 31 + i64::from(far.x)) as i32;
    assert_eq!(hash(far), wide);
}
