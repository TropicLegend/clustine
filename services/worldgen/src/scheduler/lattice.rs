//! The order of runs (ADR-0021, section 2) and the tasks of section 5, as functions of
//! positions alone.

use clustine_world::ChunkPos;

use super::Order;

/// The rank of a chunk: of its class `(x mod 3, z mod 3)`, by the record's table.
pub(crate) fn rank(chunk: ChunkPos) -> u8 {
    const RANKS: [[u8; 3]; 3] = [[0, 1, 2], [7, 6, 3], [8, 5, 4]];
    RANKS[chunk.x.rem_euclid(3) as usize][chunk.z.rem_euclid(3) as usize]
}

/// The Chebyshev distance between two chunks, without overflow at the rim of `i32`.
pub(crate) fn distance(a: ChunkPos, b: ChunkPos) -> i64 {
    let dx = (i64::from(a.x) - i64::from(b.x)).abs();
    let dz = (i64::from(a.z) - i64::from(b.z)).abs();
    dx.max(dz)
}

/// The chunks within `radius` of `centre`, row by row from the north (least z), each
/// row from the west (least x): the order `clustine_light::Around` takes neighbours
/// in, so that the centre of a three by three is the fifth.
pub(crate) fn around(centre: ChunkPos, radius: i32) -> impl Iterator<Item = ChunkPos> {
    (-radius..=radius).flat_map(move |dz| {
        (-radius..=radius).map(move |dx| ChunkPos::new(centre.x + dx, centre.z + dz))
    })
}

/// Whether the run of `d` sees what the run of `seer` sees: the one function that
/// decides between the exact order and shape B.
pub(crate) fn sees(order: Order, d: ChunkPos, seer: ChunkPos) -> bool {
    match order {
        Order::Nine => rank(d) < rank(seer) && distance(d, seer) <= 2,
        Order::Alone | Order::Bare => false,
    }
}

/// The place of `chunk` among the three by three around `centre`, in the order of
/// [`around`], or `None` if it lies further off.
pub(crate) fn index_in_three(centre: ChunkPos, chunk: ChunkPos) -> Option<usize> {
    let dx = i64::from(chunk.x) - i64::from(centre.x);
    let dz = i64::from(chunk.z) - i64::from(centre.z);
    ((-1..=1).contains(&dx) && (-1..=1).contains(&dz)).then(|| ((dz + 1) * 3 + dx + 1) as usize)
}

/// A task: one stage of one chunk (ADR-0021, section 5). Ordered so that a set of them
/// iterates the same way whatever was put in first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    Starts(ChunkPos),
    Terrain(ChunkPos),
    Run(ChunkPos),
    Finished(ChunkPos),
    Lit(ChunkPos),
}

impl Key {
    /// What has to be done before this task can start, by section 5's table. A run's
    /// starts are not among them: the terrain of its centre keeps the starts it
    /// references.
    pub(crate) fn needs(self, order: Order) -> Vec<Key> {
        if order == Order::Bare {
            // Nothing starts and nothing is placed, so a chunk is finished as its
            // terrain is, and terrain needs no starts.
            return match self {
                Key::Starts(_) | Key::Terrain(_) | Key::Run(_) => Vec::new(),
                Key::Finished(e) => vec![Key::Terrain(e)],
                Key::Lit(p) => around(p, 1).map(Key::Finished).collect(),
            };
        }
        match self {
            Key::Starts(_) => Vec::new(),
            Key::Terrain(p) => around(p, 8).map(Key::Starts).collect(),
            Key::Run(c) => {
                let mut seen: Vec<ChunkPos> = around(c, 2).filter(|d| sees(order, *d, c)).collect();
                seen.sort_by_key(|d| (rank(*d), *d));
                around(c, 2)
                    .map(Key::Terrain)
                    .chain(seen.into_iter().map(Key::Run))
                    .collect()
            }
            Key::Finished(e) => std::iter::once(Key::Terrain(e))
                .chain(around(e, 1).map(Key::Run))
                .collect(),
            Key::Lit(p) => around(p, 1).map(Key::Finished).collect(),
        }
    }

    /// What kind of task it is, for a line in the log.
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Key::Starts(_) => "starts",
            Key::Terrain(_) => "terrain",
            Key::Run(_) => "run",
            Key::Finished(_) => "finished",
            Key::Lit(_) => "lit",
        }
    }

    pub(crate) fn position(self) -> ChunkPos {
        match self {
            Key::Starts(p) | Key::Terrain(p) | Key::Run(p) | Key::Finished(p) | Key::Lit(p) => p,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn the_nine_chunks_of_any_three_by_three_have_nine_ranks() {
        for x in -4..4 {
            for z in -4..4 {
                let ranks: BTreeSet<u8> = around(ChunkPos::new(x, z), 1).map(rank).collect();
                assert_eq!(ranks.len(), 9);
            }
        }
    }

    #[test]
    fn two_chunks_of_one_rank_are_at_least_three_apart() {
        let chunks: Vec<ChunkPos> = around(ChunkPos::new(0, 0), 6).collect();
        for a in &chunks {
            for b in &chunks {
                if a != b && rank(*a) == rank(*b) {
                    assert!(distance(*a, *b) >= 3, "{a:?} {b:?}");
                }
            }
        }
    }

    /// The runs a run depends on, by section 2's table: 0, 2, 5, 12, 20, 33, 48, 69, 93.
    #[test]
    fn the_runs_a_run_depends_on_are_those_of_the_table() {
        let mut by_rank = [0usize; 9];
        for x in 0..3 {
            for z in 0..3 {
                let c = ChunkPos::new(x, z);
                let mut found = BTreeSet::new();
                let mut open = vec![c];
                while let Some(run) = open.pop() {
                    for need in Key::Run(run).needs(Order::Nine) {
                        if let Key::Run(d) = need
                            && found.insert(d)
                        {
                            open.push(d);
                        }
                    }
                }
                by_rank[rank(c) as usize] = found.len();
            }
        }
        assert_eq!(by_rank, [0, 2, 5, 12, 20, 33, 48, 69, 93]);
    }

    #[test]
    fn alone_no_run_sees_another() {
        let needs = Key::Run(ChunkPos::new(2, 0)).needs(Order::Alone);
        assert_eq!(needs.len(), 25);
        assert!(needs.iter().all(|key| matches!(key, Key::Terrain(_))));
    }

    #[test]
    fn the_centre_of_a_three_by_three_is_its_fifth() {
        let centre = ChunkPos::new(-7, 12);
        assert_eq!(around(centre, 1).nth(4), Some(centre));
        assert_eq!(index_in_three(centre, centre), Some(4));
        assert_eq!(index_in_three(centre, ChunkPos::new(-8, 11)), Some(0));
        assert_eq!(index_in_three(centre, ChunkPos::new(-5, 12)), None);
    }
}
