// Adapted from SteelMC's `steel-core/src/worldgen/feature/{placed,placement}.rs`.

//! The modifiers that move their position or give new ones: `in_square`, `offset`,
//! `height_range`, `heightmap`, `environment_scan`, `fixed_placement`, `cuboid` and
//! `randomly_selected`.

use clustine_data::Face;
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{read_face, read_vec, relative};
use crate::context::{Context, Heightmap};
use crate::placement::{Modifier, read_modifier};
use crate::predicates::BlockPredicate;
use crate::reader::{Read, Reader};
use crate::values::{HeightProvider, IntProvider};

/// `in_square`: a random column of the chunk, at the same height.
#[derive(Debug)]
pub struct InSquare;

impl Modifier for InSquare {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let x = context.random.next_i32_bounded(16) + origin.x;
        let z = context.random.next_i32_bounded(16) + origin.z;
        out.push(BlockPos::new(x, origin.y, z));
    }
}

/// `offset`: moved by three drawn amounts, x, y and z in that order.
#[derive(Debug)]
pub struct Offset {
    x: IntProvider,
    y: IntProvider,
    z: IntProvider,
}

impl Offset {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            x: IntProvider::read(&reader.field("x")?)?,
            y: IntProvider::read(&reader.field("y")?)?,
            z: IntProvider::read(&reader.field("z")?)?,
        })
    }
}

impl Modifier for Offset {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let x = origin.x + self.x.sample(&mut *context.random);
        let y = origin.y + self.y.sample(&mut *context.random);
        let z = origin.z + self.z.sample(&mut *context.random);
        out.push(BlockPos::new(x, y, z));
    }
}

/// `height_range`: the same column at a drawn height, against the generator's
/// heights.
#[derive(Debug)]
pub struct HeightRange(HeightProvider);

impl HeightRange {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(HeightProvider::read(&reader.field("height")?)?))
    }
}

impl Modifier for HeightRange {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let generation = context.settings.generation();
        let y = self.0.sample(&mut *context.random, generation);
        out.push(BlockPos::new(origin.x, y, origin.z));
    }
}

/// `heightmap`: the same column on a heightmap's surface, where that is above the
/// bottom of the world.
#[derive(Debug)]
pub struct HeightmapPlacement(Heightmap);

impl HeightmapPlacement {
    pub fn read(reader: &Reader) -> Read<Self> {
        let heightmap = reader.field("heightmap")?;
        let name = heightmap.text()?;
        Ok(Self(Heightmap::named(name).ok_or_else(|| {
            heightmap.error(format!("{name} is not a heightmap"))
        })?))
    }
}

impl Modifier for HeightmapPlacement {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let y = context.level.height_at(self.0, origin.x, origin.z);
        if y > context.level.min_y() {
            out.push(BlockPos::new(origin.x, y, origin.z));
        }
    }
}

/// `environment_scan`: the first block up or down the column, within a number of
/// steps, where a predicate holds, while another holds on the way.
#[derive(Debug)]
pub struct EnvironmentScan {
    direction: Face,
    target: BlockPredicate,
    allowed: BlockPredicate,
    max_steps: i32,
}

impl EnvironmentScan {
    pub fn read(reader: &Reader) -> Read<Self> {
        let direction = reader.field("direction_of_search")?;
        let face = read_face(&direction)?;
        if !matches!(face, Face::Up | Face::Down) {
            return Err(direction.error("a scan goes up or down"));
        }
        let steps = reader.field("max_steps")?;
        let max_steps = steps.int()?;
        if !(1..=32).contains(&max_steps) {
            return Err(steps.error("a scan takes from 1 to 32 steps"));
        }
        Ok(Self {
            direction: face,
            target: BlockPredicate::read(&reader.field("target_condition")?)?,
            allowed: match reader.optional("allowed_search_condition") {
                Some(allowed) => BlockPredicate::read(&allowed)?,
                None => BlockPredicate::True,
            },
            max_steps,
        })
    }
}

impl Modifier for EnvironmentScan {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let mut pos = origin;
        if !self.allowed.test(context, pos) {
            return;
        }
        for _ in 0..self.max_steps {
            if self.target.test(context, pos) {
                out.push(pos);
                return;
            }
            pos = relative(pos, self.direction);
            if context.level.is_outside_build_height(pos.y) {
                return;
            }
            if !self.allowed.test(context, pos) {
                break;
            }
        }
        if self.target.test(context, pos) {
            out.push(pos);
        }
    }
}

/// `fixed_placement`: the positions listed that lie in the chunk of the position.
#[derive(Debug)]
pub struct FixedPlacement(Vec<BlockPos>);

impl FixedPlacement {
    pub fn read(reader: &Reader) -> Read<Self> {
        let positions = reader
            .field("positions")?
            .list()?
            .iter()
            .map(|position| read_vec(position).map(|(x, y, z)| BlockPos::new(x, y, z)))
            .collect::<Read<_>>()?;
        Ok(Self(positions))
    }
}

impl Modifier for FixedPlacement {
    fn positions(&self, _: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let chunk = origin.chunk();
        out.extend(self.0.iter().filter(|pos| pos.chunk() == chunk));
    }
}

/// `cuboid`: the blocks of a box of drawn size from the position, its edges and its
/// inside each included or not.
#[derive(Debug)]
pub struct Cuboid {
    xz_size: IntProvider,
    y_size: IntProvider,
    include_edges: bool,
    include_interior: bool,
}

impl Cuboid {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            xz_size: IntProvider::read(&reader.field("xz_size")?)?,
            y_size: IntProvider::read(&reader.field("y_size")?)?,
            include_edges: reader.field("include_edges")?.boolean()?,
            include_interior: reader.field("include_interior")?.boolean()?,
        })
    }
}

impl Modifier for Cuboid {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let height = self.y_size.sample(&mut *context.random);
        let width = self.xz_size.sample(&mut *context.random);
        let length = self.xz_size.sample(&mut *context.random);
        for x in 0..=width {
            for y in 0..=height {
                for z in 0..=length {
                    let x_side = x == 0 || x == width;
                    let y_side = y == 0 || y == height;
                    let z_side = z == 0 || z == length;
                    // An edge is where two sides meet; the inside touches none.
                    let edge = x_side && y_side || z_side && y_side || x_side && z_side;
                    let inside = !x_side && !y_side && !z_side;
                    if (self.include_edges || !edge) && (self.include_interior || !inside) {
                        out.push(BlockPos::new(origin.x + x, origin.y + y, origin.z + z));
                    }
                }
            }
        }
    }
}

/// `randomly_selected`: one modifier of a list, drawn, gives the positions.
#[derive(Debug)]
pub struct RandomlySelected(Vec<Box<dyn Modifier>>);

impl RandomlySelected {
    pub fn read(reader: &Reader) -> Read<Self> {
        let placements = reader
            .field("placements")?
            .list()?
            .iter()
            .map(read_modifier)
            .collect::<Read<Vec<_>>>()?;
        if placements.is_empty() {
            return Err(reader.error("a selection from no modifiers"));
        }
        Ok(Self(placements))
    }
}

impl Modifier for RandomlySelected {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let index = context.random.next_i32_bounded(self.0.len() as i32) as usize;
        self.0[index].positions(context, origin, out);
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_noise::WorldgenRandom;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::testing::World;
    use crate::values::VerticalAnchor;

    fn run(world: &mut World, modifier: &dyn Modifier, seed: u64, at: BlockPos) -> Vec<BlockPos> {
        let mut out = Vec::new();
        world.with_context(seed, |context| modifier.positions(context, at, &mut out));
        out
    }

    #[test]
    fn in_square_draws_x_then_z_within_the_chunk() {
        let mut world = World::flat(blocks::STONE, 0);
        let mut expected = WorldgenRandom::from_seed(12);
        let x = expected.next_i32_bounded(16);
        let z = expected.next_i32_bounded(16);
        assert_eq!(
            run(&mut world, &InSquare, 12, BlockPos::new(16, -64, 32)),
            [BlockPos::new(16 + x, -64, 32 + z)]
        );
    }

    #[test]
    fn the_first_vein_of_dirt_of_a_chunk_starts_where_the_game_starts_it() {
        // The trace of `clustine-noise`'s decoration seed test, through the modifiers:
        // in_square, then height_range uniform from 0 to 160, as `ore_dirt` has it.
        let mut random = WorldgenRandom::from_seed(0);
        let decoration = random.set_decoration_seed(13_579, -6_695_392, 5_868_656);
        random.set_feature_seed(decoration, 0, 6);
        let height = HeightRange(HeightProvider::Uniform {
            min: VerticalAnchor::Absolute(0),
            max: VerticalAnchor::Absolute(160),
        });
        let mut world = World::flat(blocks::STONE, 0);
        let origin = BlockPos::new(-6_695_392, -64, 5_868_656);
        let mut squared = Vec::new();
        let mut placed = Vec::new();
        world.with_random(&mut random, None, |context| {
            InSquare.positions(context, origin, &mut squared);
            height.positions(context, squared[0], &mut placed);
        });
        // The game's own trace: x -6695386, y 149, z 5868662.
        assert_eq!(placed, [BlockPos::new(-6_695_386, 149, 5_868_662)]);
    }

    #[test]
    fn an_offset_draws_x_then_y_then_z() {
        static SPREAD: Data = Data::Object(&[
            ("type", Data::Text("minecraft:offset")),
            ("x", Data::Number("1")),
            (
                "y",
                Data::Object(&[
                    ("type", Data::Text("minecraft:uniform")),
                    ("max_inclusive", Data::Number("-1")),
                    ("min_inclusive", Data::Number("-3")),
                ]),
            ),
            ("z", Data::Number("-2")),
        ]);
        let offset = read_modifier(&Reader::new(&SPREAD, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 0);
        let mut expected = WorldgenRandom::from_seed(3);
        let dy = expected.next_i32_bounded(3) - 3;
        assert_eq!(
            run(&mut world, offset.as_ref(), 3, BlockPos::new(0, 10, 0)),
            [BlockPos::new(1, 10 + dy, -2)]
        );
    }

    #[test]
    fn a_heightmap_puts_the_position_on_the_surface_above_the_bottom() {
        let mut world = World::flat(blocks::GRASS_BLOCK, 70);
        let surface = HeightmapPlacement(Heightmap::WorldSurfaceWg);
        assert_eq!(
            run(&mut world, &surface, 0, BlockPos::new(5, -64, 5)),
            [BlockPos::new(5, 71, 5)]
        );
        let mut void = World::flat(blocks::AIR, -100);
        assert!(run(&mut void, &surface, 0, BlockPos::new(5, -64, 5)).is_empty());
    }

    #[test]
    fn a_scan_down_stops_at_the_first_solid_block_through_air() {
        let scan = EnvironmentScan {
            direction: Face::Down,
            target: BlockPredicate::Solid((0, 0, 0)),
            allowed: BlockPredicate::MatchingBlockTag("minecraft:air", (0, 0, 0)),
            max_steps: 12,
        };
        let mut world = World::flat(blocks::STONE, 50);
        assert_eq!(
            run(&mut world, &scan, 0, BlockPos::new(0, 60, 0)),
            [BlockPos::new(0, 50, 0)]
        );
        assert!(run(&mut world, &scan, 0, BlockPos::new(0, 63, 0)).is_empty());
        world.set(BlockPos::new(0, 55, 0), blocks::WATER);
        assert!(run(&mut world, &scan, 0, BlockPos::new(0, 60, 0)).is_empty());
    }

    #[test]
    fn a_fixed_placement_keeps_the_positions_of_the_chunk() {
        let fixed = FixedPlacement(vec![BlockPos::new(100, 49, 0), BlockPos::new(3, 4, 5)]);
        let mut world = World::flat(blocks::STONE, 0);
        assert_eq!(
            run(&mut world, &fixed, 0, BlockPos::new(96, -64, 0)),
            [BlockPos::new(100, 49, 0)]
        );
        assert!(run(&mut world, &fixed, 0, BlockPos::new(16, -64, 0)).is_empty());
    }

    #[test]
    fn a_hollow_cuboid_without_edges_gives_the_middles_of_its_faces() {
        let cuboid = Cuboid {
            xz_size: IntProvider::Constant(2),
            y_size: IntProvider::Constant(2),
            include_edges: false,
            include_interior: false,
        };
        let mut world = World::flat(blocks::STONE, 0);
        let out = run(&mut world, &cuboid, 0, BlockPos::new(0, 0, 0));
        assert_eq!(
            out,
            [
                BlockPos::new(0, 1, 1),
                BlockPos::new(1, 0, 1),
                BlockPos::new(1, 1, 0),
                BlockPos::new(1, 1, 2),
                BlockPos::new(1, 2, 1),
                BlockPos::new(2, 1, 1),
            ]
        );
        let full = Cuboid {
            include_edges: true,
            include_interior: true,
            ..cuboid
        };
        assert_eq!(run(&mut world, &full, 0, BlockPos::new(0, 0, 0)).len(), 27);
    }

    #[test]
    fn a_random_selection_draws_one_of_its_modifiers() {
        static WELL: Data = Data::Object(&[
            ("type", Data::Text("minecraft:randomly_selected")),
            (
                "placements",
                Data::List(&[
                    Data::Object(&[
                        ("type", Data::Text("minecraft:offset")),
                        ("x", Data::Number("0")),
                        ("y", Data::Number("-1")),
                        ("z", Data::Number("0")),
                    ]),
                    Data::Object(&[
                        ("type", Data::Text("minecraft:offset")),
                        ("x", Data::Number("-1")),
                        ("y", Data::Number("-1")),
                        ("z", Data::Number("0")),
                    ]),
                ]),
            ),
        ]);
        let selected = read_modifier(&Reader::new(&WELL, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 0);
        for seed in 0..10 {
            let mut expected = WorldgenRandom::from_seed(seed);
            let x = -expected.next_i32_bounded(2);
            assert_eq!(
                run(&mut world, selected.as_ref(), seed, BlockPos::new(0, 5, 0)),
                [BlockPos::new(x, 4, 0)]
            );
        }
    }
}
