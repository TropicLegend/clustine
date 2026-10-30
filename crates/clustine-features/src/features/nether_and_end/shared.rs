//! What several of the Nether's features share: the game's walk outwards from a block
//! by Manhattan distance.

use clustine_world::BlockPos;

/// The positions within `reach_x`, `reach_y` and `reach_z` of `origin` along each axis,
/// in the order of the game's `BlockPos.withinManhattan` (`BlockPos$3.computeNext`,
/// dumped in `n7-javap`): by Manhattan distance from the origin, nearest first; within
/// one distance x from low to high, then y from low to high, and z on the positive side
/// before its mirror on the negative. Features that stop at a distance, or whose
/// blocks read what the blocks before them placed, follow this order.
pub fn within_manhattan(
    origin: BlockPos,
    reach_x: i32,
    reach_y: i32,
    reach_z: i32,
) -> impl Iterator<Item = BlockPos> {
    WithinManhattan {
        origin,
        reach: [reach_x, reach_y, reach_z],
        max_depth: reach_x + reach_y + reach_z,
        depth: 0,
        max_x: 0,
        max_y: 0,
        x: 0,
        y: 0,
        mirror: None,
        done: false,
    }
}

/// The game's iterator, field for field: the fields start at nought, so the first
/// position is the origin itself.
struct WithinManhattan {
    origin: BlockPos,
    reach: [i32; 3],
    max_depth: i32,
    depth: i32,
    max_x: i32,
    max_y: i32,
    x: i32,
    y: i32,
    /// The position whose z the next call mirrors about the origin's.
    mirror: Option<BlockPos>,
    done: bool,
}

impl Iterator for WithinManhattan {
    type Item = BlockPos;

    fn next(&mut self) -> Option<BlockPos> {
        if let Some(last) = self.mirror.take() {
            return Some(BlockPos::new(
                last.x,
                last.y,
                self.origin.z - (last.z - self.origin.z),
            ));
        }
        if self.done {
            return None;
        }
        loop {
            if self.y > self.max_y {
                self.x += 1;
                if self.x > self.max_x {
                    self.depth += 1;
                    if self.depth > self.max_depth {
                        self.done = true;
                        return None;
                    }
                    self.max_x = self.reach[0].min(self.depth);
                    self.x = -self.max_x;
                }
                self.max_y = self.reach[1].min(self.depth - self.x.abs());
                self.y = -self.max_y;
            }
            let (x, y) = (self.x, self.y);
            let z = self.depth - x.abs() - y.abs();
            self.y += 1;
            if z <= self.reach[2] {
                let found = self.origin.offset(x, y, z);
                if z != 0 {
                    self.mirror = Some(found);
                }
                return Some(found);
            }
        }
    }
}

pub fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.z - b.z).abs()
}

/// The Nether's features, read once for the tests that need the Nether's heights.
#[cfg(test)]
pub fn nether_settings() -> &'static crate::settings::FeatureSettings {
    static SETTINGS: std::sync::OnceLock<crate::settings::FeatureSettings> =
        std::sync::OnceLock::new();
    SETTINGS.get_or_init(|| {
        crate::settings::FeatureSettings::new(13579, clustine_terrain::biome::Dimension::Nether)
            .expect("the data is read whole")
    })
}

/// Runs `f` on the toy world as a feature of the Nether sees it: the level 256 blocks
/// high from 0, the generator's 128 of them, with a random source seeded with `seed`.
#[cfg(test)]
pub fn with_nether_context<R>(
    world: &mut crate::testing::World,
    seed: u64,
    f: impl FnOnce(&mut crate::context::Context<'_, '_>) -> R,
) -> R {
    let settings = nether_settings();
    let mut random = clustine_noise::WorldgenRandom::from_seed(seed);
    let mut level =
        crate::context::Level::new(world, settings.zoom(), settings.min_y(), settings.height());
    let mut context = crate::context::Context {
        level: &mut level,
        random: &mut random,
        settings,
        top: None,
    };
    f(&mut context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_starts_at_the_origin_and_goes_out_by_distance_with_z_mirrored() {
        let origin = BlockPos::new(10, 20, 30);
        let walk: Vec<(i32, i32, i32)> = within_manhattan(origin, 1, 1, 1)
            .take(8)
            .map(|pos| (pos.x - 10, pos.y - 20, pos.z - 30))
            .collect();
        assert_eq!(
            walk,
            vec![
                (0, 0, 0),
                (-1, 0, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
                (0, 1, 0),
                (1, 0, 0),
                (-1, -1, 0),
            ]
        );
    }

    #[test]
    fn the_walk_holds_every_position_of_the_box_within_the_reaches_once() {
        let origin = BlockPos::new(0, 0, 0);
        let (rx, ry, rz) = (3, 0, 2);
        let walk: Vec<BlockPos> = within_manhattan(origin, rx, ry, rz).collect();
        let mut sorted = walk.clone();
        sorted.sort_by_key(|pos| (pos.x, pos.y, pos.z));
        sorted.dedup();
        assert_eq!(sorted.len(), walk.len());
        assert_eq!(walk.len(), 7 * 5);
        assert!(
            walk.windows(2)
                .all(|pair| manhattan(pair[0], origin) <= manhattan(pair[1], origin))
        );
    }
}
