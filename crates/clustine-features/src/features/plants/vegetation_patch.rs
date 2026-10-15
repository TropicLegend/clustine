// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/vegetation_patch.rs`.

//! `vegetation_patch` and `waterlogged_vegetation_patch`: a patch of ground (moss,
//! pale moss, clay) laid over the floor or the ceiling around the origin, and a
//! placed feature (what grows on it) on each block of its surface by a chance
//! (`VegetationPatchFeature`, `WaterloggedVegetationPatchFeature`).
//!
//! The surface is collected in a Java `HashSet` and the vegetation placed in its
//! order, which [`JavaSet`] keeps. The waterlogged kind turns the blocks of the
//! surface that are closed in on every side but up into water, and grows only there.

use clustine_data::{Face, SupportType, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{BlockSet, HORIZONTAL, relative, same_block};
use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::java_set::JavaSet;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::survive::sturdy;
use crate::values::IntProvider;

#[derive(Debug)]
pub struct VegetationPatch {
    replaceable: BlockSet,
    ground_state: Provider,
    vegetation_feature: PlacedRef,
    /// The direction into the surface: down for a floor, up for a ceiling.
    inwards: Face,
    depth: IntProvider,
    extra_bottom_block_chance: f32,
    vertical_range: i32,
    vegetation_chance: f32,
    xz_radius: IntProvider,
    extra_edge_column_chance: f32,
    waterlogged: bool,
}

impl VegetationPatch {
    pub fn read(reader: &Reader, waterlogged: bool) -> Read<Self> {
        let surface = reader.field("surface")?;
        let inwards = match surface.text()? {
            "floor" => Face::Down,
            "ceiling" => Face::Up,
            other => return Err(surface.error(format!("{other} is not a surface"))),
        };
        Ok(Self {
            replaceable: BlockSet::read(&reader.field("replaceable")?)?,
            ground_state: Provider::read(&reader.field("ground_state")?)?,
            vegetation_feature: PlacedRef::read(&reader.field("vegetation_feature")?)?,
            inwards,
            depth: IntProvider::read(&reader.field("depth")?)?,
            extra_bottom_block_chance: reader.field("extra_bottom_block_chance")?.float()?,
            vertical_range: reader.field("vertical_range")?.int()?,
            vegetation_chance: reader.field("vegetation_chance")?.float()?,
            xz_radius: IntProvider::read(&reader.field("xz_radius")?)?,
            extra_edge_column_chance: reader.field("extra_edge_column_chance")?.float()?,
            waterlogged,
        })
    }

    /// `placeGroundPatch`: the patch's ground, and the set of the blocks of its
    /// surface where ground was placed.
    fn place_ground_patch(
        &self,
        context: &mut Context<'_, '_>,
        origin: BlockPos,
        x_radius: i32,
        z_radius: i32,
    ) -> JavaSet {
        let inwards = self.inwards;
        let outwards = inwards.opposite();
        let mut surface = JavaSet::new();
        for dx in -x_radius..=x_radius {
            let at_longitude_limit = dx == -x_radius || dx == x_radius;
            for dz in -z_radius..=z_radius {
                let at_latitude_limit = dz == -z_radius || dz == z_radius;
                let corner = at_longitude_limit && at_latitude_limit;
                let edge = (at_longitude_limit || at_latitude_limit) && !corner;
                if corner {
                    continue;
                }
                // An edge is kept by a chance; the game draws it only where the
                // chance is not nothing.
                if edge
                    && (self.extra_edge_column_chance == 0.0
                        || context.random.next_f32() > self.extra_edge_column_chance)
                {
                    continue;
                }
                let mut pos = origin.offset(dx, 0, dz);
                // In through air, then out through what is not air, as far as the
                // range goes. The game tests the block before it counts, so it reads
                // the block one past the range too.
                let mut i = 0;
                while context.level.block(pos).props().is_air() && i < self.vertical_range {
                    pos = relative(pos, inwards);
                    i += 1;
                }
                let mut i = 0;
                while !context.level.block(pos).props().is_air() && i < self.vertical_range {
                    pos = relative(pos, outwards);
                    i += 1;
                }
                let below = relative(pos, inwards);
                let below_state = context.level.block(below);
                if !context.level.block(pos).props().is_air()
                    || !sturdy(below_state, outwards, SupportType::Full)
                {
                    continue;
                }
                let depth = self.depth.sample(&mut *context.random)
                    + i32::from(
                        self.extra_bottom_block_chance > 0.0
                            && context.random.next_f32() < self.extra_bottom_block_chance,
                    );
                if self.place_ground(context, below, depth) {
                    surface.insert(below);
                }
            }
        }
        surface
    }

    /// `placeGround`: up to `depth` blocks of ground inwards from `pos`. A block that
    /// is already the ground's is passed without moving on; one the ground may not
    /// replace ends the column, which counts if any block was placed before it.
    fn place_ground(&self, context: &mut Context<'_, '_>, mut pos: BlockPos, depth: i32) -> bool {
        for i in 0..depth {
            let state = self.ground_state.state(context, pos);
            let there = context.level.block(pos);
            if !same_block(state, there) {
                if !self.replaceable.contains(there) {
                    return i != 0;
                }
                context.level.set_block(pos, state);
                pos = relative(pos, self.inwards);
            }
        }
        true
    }

    /// The waterlogged kind's `placeGroundPatch`: the blocks of the surface closed in
    /// on the four sides and below become water, and only they are kept.
    fn flood(&self, context: &mut Context<'_, '_>, surface: &JavaSet) -> JavaSet {
        let mut water = JavaSet::new();
        for pos in surface.iter() {
            if !exposed(context, pos) {
                water.insert(pos);
            }
        }
        for pos in water.iter() {
            context.level.set_block(pos, blocks::WATER);
        }
        water
    }

    /// `distributeVegetation`: the vegetation on each block of the surface, in the
    /// set's order, by a chance.
    fn distribute_vegetation(&self, context: &mut Context<'_, '_>, surface: &JavaSet) {
        let outwards = self.inwards.opposite();
        for pos in surface.iter() {
            if !(self.vegetation_chance > 0.0 && context.random.next_f32() < self.vegetation_chance)
            {
                continue;
            }
            if self.waterlogged {
                // The waterlogged kind grows from the block below the water, and fills
                // with water what it grew where it can hold some.
                let grown = self
                    .vegetation_feature
                    .place(context, relative(pos.offset(0, -1, 0), outwards));
                if grown {
                    let state = context.level.block(pos);
                    if state.property("waterlogged") == Some("false") {
                        let wet = state
                            .with_property("waterlogged", "true")
                            .expect("the state has the property");
                        context.level.set_block(pos, wet);
                    }
                }
            } else {
                self.vegetation_feature
                    .place(context, relative(pos, outwards));
            }
        }
    }
}

/// Whether a block of the surface lies open on one of the four sides or below: the
/// neighbour there does not offer a full side to it.
fn exposed(context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
    HORIZONTAL.into_iter().chain([Face::Down]).any(|face| {
        !sturdy(
            context.level.block(relative(pos, face)),
            face.opposite(),
            SupportType::Full,
        )
    })
}

impl Feature for VegetationPatch {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let x_radius = self.xz_radius.sample(&mut *context.random) + 1;
        let z_radius = self.xz_radius.sample(&mut *context.random) + 1;
        let mut surface = self.place_ground_patch(context, origin, x_radius, z_radius);
        if self.waterlogged {
            surface = self.flood(context, &surface);
        }
        self.distribute_vegetation(context, &surface);
        !surface.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::testing::World;

    static MOSS: Data = Data::Object(&[
        ("type", Data::Text("minecraft:vegetation_patch")),
        ("depth", Data::Number("1")),
        ("extra_bottom_block_chance", Data::Number("0.0")),
        ("extra_edge_column_chance", Data::Number("0.0")),
        (
            "ground_state",
            Data::Object(&[("id", Data::Text("minecraft:moss_block"))]),
        ),
        ("replaceable", Data::Text("#minecraft:moss_replaceable")),
        ("surface", Data::Text("floor")),
        ("vegetation_chance", Data::Number("1.0")),
        (
            "vegetation_feature",
            Data::Object(&[
                (
                    "feature",
                    Data::Object(&[
                        ("type", Data::Text("minecraft:simple_block")),
                        ("to_place", Data::Text("minecraft:short_grass")),
                    ]),
                ),
                ("placement", Data::List(&[])),
            ]),
        ),
        ("vertical_range", Data::Number("5")),
        ("xz_radius", Data::Number("2")),
    ]);

    #[test]
    fn a_patch_lays_its_ground_on_the_floor_and_grows_on_every_block_of_it() {
        let moss = crate::feature::read_configured(&Reader::new(&MOSS, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 60);
        // The origin hangs three above the floor; the patch finds the floor.
        assert!(world.with_context(1, |context| moss.place(context, BlockPos::new(8, 63, 8))));
        // Radius three, its edges left out: the five by five inside.
        for x in 6..=10 {
            for z in 6..=10 {
                assert_eq!(
                    world.get(BlockPos::new(x, 60, z)),
                    blocks::MOSS_BLOCK,
                    "{x} {z}"
                );
                assert_eq!(
                    world.get(BlockPos::new(x, 61, z)),
                    blocks::SHORT_GRASS,
                    "{x} {z}"
                );
            }
        }
        assert_eq!(world.get(BlockPos::new(5, 60, 8)), blocks::STONE);
        assert_eq!(world.get(BlockPos::new(8, 60, 11)), blocks::STONE);
    }

    #[test]
    fn a_patch_over_nothing_within_reach_is_not_placed() {
        let moss = crate::feature::read_configured(&Reader::new(&MOSS, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 40);
        assert!(!world.with_context(1, |context| moss.place(context, BlockPos::new(8, 63, 8))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn the_vegetation_follows_the_order_of_java_s_set() {
        // The vegetation draws its chance once per block of the surface, in the set's
        // order: with a chance of a half, which blocks grow follows the draws in that
        // order and not in the order the ground was laid.
        static HALF: Data = Data::Object(&[
            ("type", Data::Text("minecraft:vegetation_patch")),
            ("depth", Data::Number("1")),
            ("extra_bottom_block_chance", Data::Number("0.0")),
            ("extra_edge_column_chance", Data::Number("0.0")),
            (
                "ground_state",
                Data::Object(&[("id", Data::Text("minecraft:moss_block"))]),
            ),
            ("replaceable", Data::Text("#minecraft:moss_replaceable")),
            ("surface", Data::Text("floor")),
            ("vegetation_chance", Data::Number("0.5")),
            (
                "vegetation_feature",
                Data::Object(&[
                    (
                        "feature",
                        Data::Object(&[
                            ("type", Data::Text("minecraft:simple_block")),
                            ("to_place", Data::Text("minecraft:short_grass")),
                        ]),
                    ),
                    ("placement", Data::List(&[])),
                ]),
            ),
            ("vertical_range", Data::Number("5")),
            ("xz_radius", Data::Number("2")),
        ]);
        let patch = crate::feature::read_configured(&Reader::new(&HALF, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 60);
        let origin = BlockPos::new(8, 61, 8);
        assert!(world.with_context(9, |context| patch.place(context, origin)));
        // The same draws by hand: the radii, depth, ground and grass draw nothing, so
        // only one chance per block in the set's order.
        let mut set = JavaSet::new();
        for x in 6..=10 {
            for z in 6..=10 {
                set.insert(BlockPos::new(x, 60, z));
            }
        }
        let mut by_hand = clustine_noise::WorldgenRandom::from_seed(9);
        for pos in set.iter() {
            let grows = by_hand.next_f32() < 0.5;
            let grown = world.get(pos.offset(0, 1, 0)) == blocks::SHORT_GRASS;
            assert_eq!(grows, grown, "{pos:?}");
        }
    }

    #[test]
    fn a_waterlogged_patch_floods_only_what_is_closed_in() {
        static POOL: Data = Data::Object(&[
            ("type", Data::Text("minecraft:waterlogged_vegetation_patch")),
            ("depth", Data::Number("2")),
            ("extra_bottom_block_chance", Data::Number("0.0")),
            ("extra_edge_column_chance", Data::Number("0.0")),
            (
                "ground_state",
                Data::Object(&[("id", Data::Text("minecraft:clay"))]),
            ),
            (
                "replaceable",
                Data::Text("#minecraft:lush_ground_replaceable"),
            ),
            ("surface", Data::Text("floor")),
            ("vegetation_chance", Data::Number("0.0")),
            (
                "vegetation_feature",
                Data::Object(&[
                    ("feature", Data::Text("minecraft:dripleaf")),
                    ("placement", Data::List(&[])),
                ]),
            ),
            ("vertical_range", Data::Number("5")),
            ("xz_radius", Data::Number("2")),
        ]);
        let pool = crate::feature::read_configured(&Reader::new(&POOL, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 60);
        // A moat of air around the patch, so that its rim lies open.
        for i in 5..=11 {
            for (x, z) in [(i, 5), (i, 11), (5, i), (11, i)] {
                world.set(BlockPos::new(x, 60, z), blocks::AIR);
            }
        }
        assert!(world.with_context(1, |context| pool.place(context, BlockPos::new(8, 61, 8))));
        // The rim of the five by five stays clay; the three by three inside is water
        // over clay.
        for x in 6..=10 {
            for z in 6..=10 {
                let inside = (7..=9).contains(&x) && (7..=9).contains(&z);
                let top = world.get(BlockPos::new(x, 60, z));
                let expected = if inside { blocks::WATER } else { blocks::CLAY };
                assert_eq!(top, expected, "{x} {z}");
                assert_eq!(world.get(BlockPos::new(x, 59, z)), blocks::CLAY);
            }
        }
    }
}
