//! `chorus_plant`: a chorus plant grown whole on end stone, branching up to four
//! times and ending in dead flowers (`ChorusPlantFeature.place` with
//! `ChorusFlowerBlock.generatePlant` and `growTreeRecursive`, and the plant's joins
//! from `ChorusPlantBlock.getStateWithConnections`, read from `d-javap`; the offsets
//! below are their bytecode's).

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{face_name, in_block_tag, relative, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::reader::{Read, Reader};

/// How far from its root a plant of the feature spreads (`place`'s 8).
const MAX_HORIZONTAL_SPREAD: i32 = 8;

/// `Direction.Plane.HORIZONTAL`'s faces, in their order, which a branch's direction is
/// drawn from (`getRandomDirection`: `Util.getRandom` of them).
const HORIZONTAL: [Face; 4] = [Face::North, Face::East, Face::South, Face::West];

/// The age of a flower that grows no more (`ChorusFlowerBlock.DEAD_AGE`).
const DEAD_AGE: &str = "5";

/// `isEmptyBlock`: air of any kind.
fn empty(level: &mut Level<'_>, at: BlockPos) -> bool {
    level.block(at).props().is_air()
}

/// `allNeighborsEmpty`: air on every horizontal side of `at` but `except`.
fn neighbours_empty(level: &mut Level<'_>, at: BlockPos, except: Option<Face>) -> bool {
    HORIZONTAL
        .into_iter()
        .filter(|face| Some(*face) != except)
        .all(|face| empty(level, relative(at, face)))
}

/// `ChorusPlantBlock.getStateWithConnections` of a plant at `at` (offsets 0-314):
/// joined to each plant or flower beside it, and below also to what holds a plant.
pub fn plant_with_connections(level: &mut Level<'_>, at: BlockPos) -> BlockState {
    let joins = |state: BlockState| {
        same_block(state, blocks::CHORUS_PLANT) || same_block(state, blocks::CHORUS_FLOWER)
    };
    let mut plant = blocks::CHORUS_PLANT;
    for face in [
        Face::Down,
        Face::Up,
        Face::North,
        Face::East,
        Face::South,
        Face::West,
    ] {
        let neighbour = level.block(relative(at, face));
        let joined = joins(neighbour)
            || (face == Face::Down && in_block_tag(neighbour, "minecraft:supports_chorus_plant"));
        plant = plant
            .with_property(face_name(face), if joined { "true" } else { "false" })
            .expect("a chorus plant joins on six sides");
    }
    plant
}

/// `ChorusFlowerBlock.generatePlant` (offsets 0-29): a plant at `origin`, then grown.
pub fn generate_plant(
    level: &mut Level<'_>,
    origin: BlockPos,
    random: &mut impl Random,
    max_spread: i32,
) {
    let plant = plant_with_connections(level, origin);
    level.set_block(origin, plant);
    grow(level, origin, random, origin, max_spread, 0);
}

/// `growTreeRecursive` (offsets 0-364): a stem of one to four blocks (one more at the
/// root) while its sides are clear, then, below the fifth level of branching, up to
/// three branches (one more at the root) sideways from its top, each grown the same;
/// a top that grew no branch ends in a dead flower.
fn grow(
    level: &mut Level<'_>,
    branch: BlockPos,
    random: &mut impl Random,
    root: BlockPos,
    max_spread: i32,
    depth: i32,
) {
    let mut height = random.next_i32_bounded(4) + 1;
    if depth == 0 {
        height += 1;
    }
    for step in 0..height {
        let at = branch.offset(0, step + 1, 0);
        if !neighbours_empty(level, at, None) {
            // The stem stops here, with no flower on it.
            return;
        }
        let plant = plant_with_connections(level, at);
        level.set_block(at, plant);
        let below = at.offset(0, -1, 0);
        let plant = plant_with_connections(level, below);
        level.set_block(below, plant);
    }
    let mut branched = false;
    if depth < 4 {
        let mut branches = random.next_i32_bounded(4);
        if depth == 0 {
            branches += 1;
        }
        for _ in 0..branches {
            let face = HORIZONTAL[random.next_i32_bounded(4) as usize];
            let target = relative(branch.offset(0, height, 0), face);
            if (target.x - root.x).abs() >= max_spread || (target.z - root.z).abs() >= max_spread {
                continue;
            }
            if empty(level, target)
                && empty(level, target.offset(0, -1, 0))
                && neighbours_empty(level, target, Some(face.opposite()))
            {
                branched = true;
                let plant = plant_with_connections(level, target);
                level.set_block(target, plant);
                let back = relative(target, face.opposite());
                let plant = plant_with_connections(level, back);
                level.set_block(back, plant);
                grow(level, target, random, root, max_spread, depth + 1);
            }
        }
    }
    if !branched {
        let flower = blocks::CHORUS_FLOWER
            .with_property("age", DEAD_AGE)
            .expect("a chorus flower has an age of 0 to 5");
        level.set_block(branch.offset(0, height, 0), flower);
    }
}

#[derive(Debug)]
pub struct ChorusPlant;

impl ChorusPlant {
    /// The kind has no settings.
    pub fn read(reader: &Reader) -> Read<Self> {
        reader.finish()?;
        Ok(Self)
    }
}

impl Feature for ChorusPlant {
    /// `ChorusPlantFeature.place` (offsets 0-43): a plant in air over what holds one.
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !empty(context.level, origin) {
            return false;
        }
        let below = context.level.block(origin.offset(0, -1, 0));
        if !in_block_tag(below, "minecraft:supports_chorus_plant") {
            return false;
        }
        generate_plant(context.level, origin, context.random, MAX_HORIZONTAL_SPREAD);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::World;

    fn is_plant(state: BlockState) -> bool {
        same_block(state, blocks::CHORUS_PLANT)
    }

    fn is_flower(state: BlockState) -> bool {
        same_block(state, blocks::CHORUS_FLOWER)
    }

    #[test]
    fn a_plant_grows_only_in_air_on_end_stone() {
        let mut world = World::flat(blocks::STONE, 60);
        let placed = world.with_context(1, |context| {
            ChorusPlant.place(context, BlockPos::new(8, 61, 8))
        });
        assert!(!placed);
        assert!(world.writes.is_empty());
        let mut world = World::flat(blocks::END_STONE, 60);
        let placed = world.with_context(1, |context| {
            ChorusPlant.place(context, BlockPos::new(8, 60, 8))
        });
        assert!(!placed, "not in a block of end stone");
        let placed = world.with_context(1, |context| {
            ChorusPlant.place(context, BlockPos::new(8, 61, 8))
        });
        assert!(placed);
    }

    /// Every plant joins exactly what is beside it, every flower is dead, and the
    /// whole stands within the spread and on the root.
    #[test]
    fn a_grown_plant_is_joined_throughout_and_ends_in_dead_flowers() {
        for seed in 0..40 {
            let mut world = World::flat(blocks::END_STONE, 60);
            let root = BlockPos::new(8, 61, 8);
            world.with_context(seed, |context| {
                assert!(ChorusPlant.place(context, root));
            });
            assert!(is_plant(world.get(root)), "seed {seed}");
            let mut flowers = 0;
            for (at, _) in world.writes.clone() {
                let state = world.get(at);
                assert!((at.x - root.x).abs() < 8 && (at.z - root.z).abs() < 8);
                if is_flower(state) {
                    flowers += 1;
                    assert_eq!(state.property("age"), Some("5"), "seed {seed}");
                    continue;
                }
                assert!(is_plant(state), "seed {seed}: {at:?}");
                for face in Face::ALL {
                    let neighbour = world.get(relative(at, face));
                    let joined = is_plant(neighbour)
                        || is_flower(neighbour)
                        || (face == Face::Down && neighbour == blocks::END_STONE);
                    let expected = if joined { "true" } else { "false" };
                    assert_eq!(
                        state.property(face_name(face)),
                        Some(expected),
                        "seed {seed}: {at:?} {face:?}"
                    );
                }
            }
            // A plant grown in the open always ends somewhere in a flower.
            assert!(flowers > 0, "seed {seed}");
        }
    }

    #[test]
    fn a_stem_stops_without_a_flower_where_its_side_is_not_clear() {
        let mut world = World::flat(blocks::END_STONE, 60);
        let root = BlockPos::new(8, 61, 8);
        // A block beside the second block of the stem.
        world.set(BlockPos::new(9, 63, 8), blocks::END_STONE);
        world.with_context(5, |context| {
            assert!(ChorusPlant.place(context, root));
        });
        // The root and the block above it, joined, and nothing higher.
        assert!(is_plant(world.get(root)));
        assert!(is_plant(world.get(root.offset(0, 1, 0))));
        assert_eq!(world.get(root.offset(0, 2, 0)), blocks::AIR);
        assert_eq!(world.writes.len(), 3, "{:?}", world.writes);
        let stem = world.get(root.offset(0, 1, 0));
        assert_eq!(stem.property("down"), Some("true"));
        assert_eq!(stem.property("up"), Some("false"));
        assert_eq!(world.get(root).property("up"), Some("true"));
        assert_eq!(world.get(root).property("down"), Some("true"));
    }
}
