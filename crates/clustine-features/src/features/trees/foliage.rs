// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/foliage.rs`.

//! The foliage placers (`FoliagePlacer` and its kinds): how much foliage a tree has
//! and the leaves it puts around each attachment of its trunk.

use clustine_data::Face;
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::common::{Axis, with_axis};
use super::common::{
    HORIZONTAL, Placement, REPLACEABLE_BY_TREES, clockwise, is_positive, manhattan,
};
use super::tree::TreeBlocks;
use super::trunk::Attachment;
use crate::blocks::{relative, relative_by};
use crate::context::Context;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Blob {
        height: IntProvider,
    },
    Bush {
        height: IntProvider,
    },
    Fancy {
        height: IntProvider,
    },
    Jungle {
        height: IntProvider,
    },
    Spruce {
        trunk_height: IntProvider,
    },
    Pine {
        height: IntProvider,
    },
    MegaPine {
        crown_height: IntProvider,
    },
    Acacia,
    DarkOak,
    RandomSpread {
        foliage_height: IntProvider,
        leaf_placement_attempts: i32,
    },
    Cherry {
        height: IntProvider,
        wide_bottom_layer_hole_chance: f32,
        corner_hole_chance: f32,
        hanging_leaves_chance: f32,
        hanging_leaves_extension_chance: f32,
    },
    Poplar {
        height: IntProvider,
        side_hole_chance: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FoliagePlacer {
    radius: IntProvider,
    offset: IntProvider,
    kind: Kind,
}

impl FoliagePlacer {
    pub fn read(reader: &Reader) -> Read<Self> {
        let provider = |key: &str| IntProvider::read(&reader.field(key)?);
        let float = |key: &str| reader.field(key)?.float();
        let kind = match reader.kind()? {
            "minecraft:blob_foliage_placer" => Kind::Blob {
                height: provider("height")?,
            },
            "minecraft:bush_foliage_placer" => Kind::Bush {
                height: provider("height")?,
            },
            "minecraft:fancy_foliage_placer" => Kind::Fancy {
                height: provider("height")?,
            },
            "minecraft:jungle_foliage_placer" => Kind::Jungle {
                height: provider("height")?,
            },
            "minecraft:spruce_foliage_placer" => Kind::Spruce {
                trunk_height: provider("trunk_height")?,
            },
            "minecraft:pine_foliage_placer" => Kind::Pine {
                height: provider("height")?,
            },
            "minecraft:mega_pine_foliage_placer" => Kind::MegaPine {
                crown_height: provider("crown_height")?,
            },
            "minecraft:acacia_foliage_placer" => Kind::Acacia,
            "minecraft:dark_oak_foliage_placer" => Kind::DarkOak,
            "minecraft:random_spread_foliage_placer" => Kind::RandomSpread {
                foliage_height: provider("foliage_height")?,
                leaf_placement_attempts: reader.field("leaf_placement_attempts")?.int()?,
            },
            "minecraft:cherry_foliage_placer" => Kind::Cherry {
                height: provider("height")?,
                wide_bottom_layer_hole_chance: float("wide_bottom_layer_hole_chance")?,
                corner_hole_chance: float("corner_hole_chance")?,
                hanging_leaves_chance: float("hanging_leaves_chance")?,
                hanging_leaves_extension_chance: float("hanging_leaves_extension_chance")?,
            },
            "minecraft:poplar_foliage_placer" => Kind::Poplar {
                height: provider("height")?,
                side_hole_chance: float("side_hole_chance")?,
            },
            other => return Err(reader.error(format!("{other} is not a foliage placer"))),
        };
        let placer = Self {
            radius: provider("radius")?,
            offset: provider("offset")?,
            kind,
        };
        reader.finish()?;
        Ok(placer)
    }

    /// `foliageHeight`.
    pub fn foliage_height<R: Random + ?Sized>(&self, random: &mut R, tree_height: i32) -> i32 {
        match &self.kind {
            Kind::Blob { height }
            | Kind::Bush { height }
            | Kind::Fancy { height }
            | Kind::Jungle { height }
            | Kind::Pine { height }
            | Kind::Cherry { height, .. }
            | Kind::Poplar { height, .. } => height.sample(random),
            Kind::Spruce { trunk_height } => 4.max(tree_height - trunk_height.sample(random)),
            Kind::MegaPine { crown_height } => crown_height.sample(random),
            Kind::Acacia => 0,
            Kind::DarkOak => 4,
            Kind::RandomSpread { foliage_height, .. } => foliage_height.sample(random),
        }
    }

    /// `foliageRadius`: the pine's grows with its trunk.
    pub fn foliage_radius<R: Random + ?Sized>(&self, random: &mut R, trunk_height: i32) -> i32 {
        let radius = self.radius.sample(random);
        match self.kind {
            Kind::Pine { .. } => radius + random.next_i32_bounded((trunk_height + 1).max(1)),
            _ => radius,
        }
    }

    /// `createFoliage`: the leaves around one attachment.
    pub fn create(
        &self,
        context: &mut Context<'_, '_>,
        blocks: &TreeBlocks,
        placement: &mut Placement,
        attachment: Attachment,
        foliage_height: i32,
        leaf_radius: i32,
    ) {
        let offset = self.offset.sample(&mut *context.random);
        let mut foliage = Foliage {
            placer: self,
            context,
            blocks,
            placement,
            double_trunk: attachment.double_trunk,
        };
        let pos = attachment.pos;
        let radius_offset = attachment.radius_offset;
        match &self.kind {
            Kind::Blob { .. } => {
                for y in (offset - foliage_height..=offset).rev() {
                    let radius = (leaf_radius + radius_offset - 1 - y / 2).max(0);
                    foliage.row(pos, radius, y);
                }
            }
            Kind::Bush { .. } => {
                for y in (offset - foliage_height..=offset).rev() {
                    foliage.row(pos, leaf_radius + radius_offset - 1 - y, y);
                }
            }
            Kind::Fancy { .. } => {
                for y in (offset - foliage_height..=offset).rev() {
                    let wide = y != offset && y != offset - foliage_height;
                    foliage.row(pos, leaf_radius + i32::from(wide), y);
                }
            }
            Kind::Jungle { .. } => {
                let leaf_height = if attachment.double_trunk {
                    foliage_height
                } else {
                    1 + foliage.context.random.next_i32_bounded(2)
                };
                for y in (offset - leaf_height..=offset).rev() {
                    foliage.row(pos, leaf_radius + radius_offset + 1 - y, y);
                }
            }
            Kind::Spruce { .. } => {
                let mut radius = foliage.context.random.next_i32_bounded(2);
                let mut max_radius = 1;
                let mut min_radius = 0;
                for y in (-foliage_height..=offset).rev() {
                    foliage.row(pos, radius, y);
                    if radius >= max_radius {
                        radius = min_radius;
                        min_radius = 1;
                        max_radius = (max_radius + 1).min(leaf_radius + radius_offset);
                    } else {
                        radius += 1;
                    }
                }
            }
            Kind::Pine { .. } => {
                let mut radius = 0;
                for y in (offset - foliage_height..=offset).rev() {
                    foliage.row(pos, radius, y);
                    if radius >= 1 && y == offset - foliage_height + 1 {
                        radius -= 1;
                    } else if radius < leaf_radius + radius_offset {
                        radius += 1;
                    }
                }
            }
            Kind::MegaPine { .. } => {
                let mut previous = 0;
                for y in pos.y - foliage_height + offset..=pos.y + offset {
                    let below_attachment = pos.y - y;
                    // In single precision, as the game divides.
                    let smooth = leaf_radius
                        + radius_offset
                        + clustine_noise::math::floor(f64::from(
                            below_attachment as f32 / foliage_height as f32 * 3.5,
                        ));
                    let jagged = if below_attachment > 0 && smooth == previous && y & 1 == 0 {
                        smooth + 1
                    } else {
                        smooth
                    };
                    foliage.row(BlockPos::new(pos.x, y, pos.z), jagged, 0);
                    previous = smooth;
                }
            }
            Kind::Acacia => {
                let at = pos.offset(0, offset, 0);
                foliage.row(at, leaf_radius + radius_offset, -1 - foliage_height);
                foliage.row(at, leaf_radius - 1, -foliage_height);
                foliage.row(at, leaf_radius + radius_offset - 1, 0);
            }
            Kind::DarkOak => {
                let at = pos.offset(0, offset, 0);
                if attachment.double_trunk {
                    foliage.row(at, leaf_radius + 2, -1);
                    foliage.row(at, leaf_radius + 3, 0);
                    foliage.row(at, leaf_radius + 2, 1);
                    if foliage.context.random.next_bool() {
                        foliage.row(at, leaf_radius, 2);
                    }
                } else {
                    foliage.row(at, leaf_radius + 2, -1);
                    foliage.row(at, leaf_radius + 1, 0);
                }
            }
            Kind::RandomSpread {
                leaf_placement_attempts,
                ..
            } => {
                for _ in 0..*leaf_placement_attempts {
                    let random = &mut *foliage.context.random;
                    let x =
                        random.next_i32_bounded(leaf_radius) - random.next_i32_bounded(leaf_radius);
                    let y = random.next_i32_bounded(foliage_height)
                        - random.next_i32_bounded(foliage_height);
                    let z =
                        random.next_i32_bounded(leaf_radius) - random.next_i32_bounded(leaf_radius);
                    foliage.leaf(pos.offset(x, y, z));
                }
            }
            Kind::Cherry {
                hanging_leaves_chance,
                hanging_leaves_extension_chance,
                ..
            } => {
                let at = pos.offset(0, offset, 0);
                let radius = leaf_radius + radius_offset - 1;
                foliage.row(at, radius - 2, foliage_height - 3);
                foliage.row(at, radius - 1, foliage_height - 4);
                for y in (0..=foliage_height - 5).rev() {
                    foliage.row(at, radius, y);
                }
                foliage.row_with_hanging_leaves(
                    at,
                    radius,
                    -1,
                    *hanging_leaves_chance,
                    *hanging_leaves_extension_chance,
                );
                foliage.row_with_hanging_leaves(
                    at,
                    radius - 1,
                    -2,
                    *hanging_leaves_chance,
                    *hanging_leaves_extension_chance,
                );
            }
            Kind::Poplar {
                side_hole_chance, ..
            } => {
                let at = pos.offset(0, offset, 0);
                let radius = leaf_radius + radius_offset - 1;
                let flip = foliage.context.random.next_bool();
                let poplar = Poplar {
                    foliage_height,
                    flip,
                    side_hole_chance: *side_hole_chance,
                };
                foliage.poplar_row(&poplar, at, radius - 2, foliage_height - 1);
                foliage.poplar_row(&poplar, at, radius - 1, foliage_height - 2);
                foliage.poplar_row(&poplar, at, radius - 1, foliage_height - 3);
                for y in (1..foliage_height - 3).rev() {
                    foliage.poplar_row(&poplar, at, radius, y);
                }
                foliage.poplar_logs(&poplar, at, radius, foliage_height - 4);
                foliage.poplar_row(&poplar, at, radius - 1, 0);
                foliage.poplar_row(&poplar, at, (radius - 2).clamp(1, 2), -1);
            }
        }
    }
}

/// The shape of one poplar's crown.
struct Poplar {
    foliage_height: i32,
    flip: bool,
    side_hole_chance: f32,
}

/// The leaves around one attachment being placed.
struct Foliage<'p, 'c, 'a, 'v> {
    placer: &'p FoliagePlacer,
    context: &'c mut Context<'a, 'v>,
    blocks: &'p TreeBlocks,
    placement: &'p mut Placement,
    double_trunk: bool,
}

impl Foliage<'_, '_, '_, '_> {
    /// `placeLeavesRow`: a square of leaves `y` above `origin`, less what the kind
    /// leaves out.
    fn row(&mut self, origin: BlockPos, radius: i32, y: i32) {
        let extra = i32::from(self.double_trunk);
        for dx in -radius..=radius + extra {
            for dz in -radius..=radius + extra {
                if !self.skip_signed(dx, y, dz, radius) {
                    self.leaf(origin.offset(dx, y, dz));
                }
            }
        }
    }

    /// `shouldSkipLocationSigned`.
    fn skip_signed(&mut self, dx: i32, y: i32, dz: i32, radius: i32) -> bool {
        let double_trunk = self.double_trunk;
        if matches!(self.placer.kind, Kind::DarkOak)
            && y == 0
            && double_trunk
            && (dx == -radius || dx >= radius)
            && (dz == -radius || dz >= radius)
        {
            return true;
        }
        let (x, z) = if double_trunk {
            (dx.abs().min((dx - 1).abs()), dz.abs().min((dz - 1).abs()))
        } else {
            (dx.abs(), dz.abs())
        };
        self.skip(x, y, z, radius)
    }

    /// `shouldSkipLocation` of each kind, on distances from the trunk.
    fn skip(&mut self, dx: i32, y: i32, dz: i32, radius: i32) -> bool {
        let random = &mut *self.context.random;
        match &self.placer.kind {
            Kind::Blob { .. } => {
                dx == radius && dz == radius && (random.next_i32_bounded(2) == 0 || y == 0)
            }
            Kind::Bush { .. } => dx == radius && dz == radius && random.next_i32_bounded(2) == 0,
            Kind::Fancy { .. } => {
                let x = dx as f32 + 0.5;
                let z = dz as f32 + 0.5;
                x * x + z * z > (radius * radius) as f32
            }
            Kind::Pine { .. } | Kind::Spruce { .. } => dx == radius && dz == radius && radius > 0,
            Kind::MegaPine { .. } | Kind::Jungle { .. } => {
                dx + dz >= 7 || dx * dx + dz * dz > radius * radius
            }
            Kind::RandomSpread { .. } => false,
            Kind::Acacia => {
                if y == 0 {
                    (dx > 1 || dz > 1) && dx != 0 && dz != 0
                } else {
                    dx == radius && dz == radius && radius > 0
                }
            }
            Kind::DarkOak => {
                if y == -1 && !self.double_trunk {
                    dx == radius && dz == radius
                } else if y == 1 {
                    dx + dz > radius * 2 - 2
                } else {
                    false
                }
            }
            Kind::Cherry {
                wide_bottom_layer_hole_chance,
                corner_hole_chance,
                ..
            } => {
                if y == -1
                    && (dx == radius || dz == radius)
                    && random.next_f32() < *wide_bottom_layer_hole_chance
                {
                    return true;
                }
                let corner = dx == radius && dz == radius;
                if radius > 2 {
                    corner || dx + dz > radius * 2 - 2 && random.next_f32() < *corner_hole_chance
                } else {
                    corner && random.next_f32() < *corner_hole_chance
                }
            }
            // The poplar places its rows by its own rule (`poplar_row`), on signed
            // distances; no row of the common kind is asked of it.
            Kind::Poplar { .. } => false,
        }
    }

    /// `tryPlaceLeaf`: a leaf where a tree may replace the block and it is not a
    /// persistent leaf, waterlogged where a source of water is.
    fn leaf(&mut self, pos: BlockPos) -> bool {
        let here = self.context.level.block(pos);
        if here.property("persistent") == Some("true") {
            return false;
        }
        if !(here.props().is_air() || REPLACEABLE_BY_TREES.contains(here)) {
            return false;
        }
        let mut state = self.blocks.foliage.state(self.context, pos);
        if state.property("waterlogged").is_some() {
            let water = here
                .props()
                .fluid()
                .is_some_and(|fluid| fluid.source && fluid.fluid == WATER);
            state = state
                .with_property("waterlogged", if water { "true" } else { "false" })
                .unwrap_or(state);
        }
        self.placement.set_foliage(self.context.level, pos, state);
        true
    }

    /// `placeLeavesRowWithHangingLeavesBelow`.
    fn row_with_hanging_leaves(
        &mut self,
        origin: BlockPos,
        radius: i32,
        y: i32,
        chance: f32,
        extension_chance: f32,
    ) {
        self.row(origin, radius, y);
        let extra = i32::from(self.double_trunk);
        let log = origin.offset(0, -1, 0);
        for along in HORIZONTAL {
            let to_edge = clockwise(along);
            let to_edge_distance = if is_positive(to_edge) {
                radius + extra
            } else {
                radius
            };
            let mut pos = relative_by(
                relative_by(origin.offset(0, y - 1, 0), to_edge, to_edge_distance),
                along,
                -radius,
            );
            let mut offset_along = -radius;
            while offset_along < radius + extra {
                let leaves_above = self.placement.foliage.contains(relative(pos, Face::Up));
                if leaves_above && self.hanging_leaf(chance, log, pos) {
                    self.hanging_leaf(extension_chance, log, relative(pos, Face::Down));
                }
                offset_along += 1;
                pos = relative(pos, along);
            }
        }
    }

    /// `tryPlaceExtension`.
    fn hanging_leaf(&mut self, chance: f32, log: BlockPos, pos: BlockPos) -> bool {
        if manhattan(pos, log) >= 7 || self.context.random.next_f32() > chance {
            return false;
        }
        self.leaf(pos)
    }

    /// A poplar's row: a rhombus, with a side cut on one diagonal and holes drawn in
    /// its edge.
    fn poplar_row(&mut self, poplar: &Poplar, origin: BlockPos, radius: i32, y: i32) {
        let extra = i32::from(self.double_trunk);
        for dx in -radius..=radius + extra {
            for dz in -radius..=radius + extra {
                if !self.poplar_skip(poplar, dx, y, dz, radius) {
                    self.leaf(origin.offset(dx, y, dz));
                }
            }
        }
    }

    fn poplar_skip(&mut self, poplar: &Poplar, dx: i32, y: i32, dz: i32, radius: i32) -> bool {
        let partial = poplar_row_is_partial(poplar.foliage_height, y);
        let cut = poplar_corner_cut(dx, dz, radius, partial, poplar.flip);
        let (x, z) = (dx.abs(), dz.abs());
        if partial && (x == radius || z == radius) {
            return true;
        }
        let side_hole = i32::from(self.context.random.next_f32() <= poplar.side_hole_chance);
        !poplar_within_rhombus(radius, x, z, cut, side_hole)
    }

    /// The poplar's branches inside its crown: leaves on the two axes far enough from
    /// the edge become logs, where the leaf the provider gives is there.
    fn poplar_logs(&mut self, poplar: &Poplar, origin: BlockPos, radius: i32, y: i32) {
        let extra = i32::from(self.double_trunk);
        for dx in -radius..=radius + extra {
            for dz in -radius..=radius + extra {
                let (x, z) = (dx.abs(), dz.abs());
                let cut = poplar_corner_cut(
                    dx,
                    dz,
                    radius,
                    poplar_row_is_partial(poplar.foliage_height, y),
                    poplar.flip,
                );
                if !poplar_within_rhombus(radius, x, z, cut, 2)
                    || !(z == 0 && radius - x >= 4 || x == 0 && radius - z >= 4)
                {
                    continue;
                }
                let pos = origin.offset(dx, y, dz);
                let leaf = self.blocks.foliage.state(self.context, pos);
                if self.context.level.block(pos) != leaf {
                    continue;
                }
                let log = self.blocks.trunk.state(self.context, pos);
                let axis = if z == 0 { Axis::X } else { Axis::Z };
                self.placement
                    .set_foliage(self.context.level, pos, with_axis(log, axis));
            }
        }
    }
}

/// The id of still water among the fluids.
const WATER: u8 = 2;

fn poplar_row_is_partial(foliage_height: i32, y: i32) -> bool {
    foliage_height - 1 == y || foliage_height - 2 == y
}

fn poplar_corner_cut(dx: i32, dz: i32, radius: i32, partial: bool, flip: bool) -> i32 {
    let small_corner = if flip {
        dx > 0 && dz > 0 || dz < 0 && dx < 0
    } else {
        dx > 0 && dz < 0 || dz > 0 && dx < 0
    };
    if small_corner {
        radius - 1
    } else if partial {
        radius + 1
    } else {
        radius
    }
}

fn poplar_within_rhombus(radius: i32, x: i32, z: i32, cut: i32, side: i32) -> bool {
    x + z <= radius * 2 - (cut + side)
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;

    use super::*;
    use crate::providers::Provider;
    use crate::testing::World;

    /// Whether a placer of `kind` leaves out the place `dx`, `y`, `dz` of a row of
    /// `radius`, for kinds that draw nothing there.
    fn skips(kind: Kind, dx: i32, y: i32, dz: i32, radius: i32, double_trunk: bool) -> bool {
        let placer = FoliagePlacer {
            radius: IntProvider::Constant(0),
            offset: IntProvider::Constant(0),
            kind,
        };
        let blocks = TreeBlocks {
            trunk: Provider::Simple(blocks::OAK_LOG),
            below_trunk: Provider::Simple(blocks::DIRT),
            foliage: Provider::Simple(blocks::OAK_LEAVES),
        };
        let mut placement = Placement::default();
        let mut world = World::flat(blocks::STONE, 0);
        world.with_context(0, |context| {
            let mut foliage = Foliage {
                placer: &placer,
                context,
                blocks: &blocks,
                placement: &mut placement,
                double_trunk,
            };
            foliage.skip_signed(dx, y, dz, radius)
        })
    }

    fn pine() -> Kind {
        Kind::Pine {
            height: IntProvider::Constant(0),
        }
    }

    fn mega_pine() -> Kind {
        Kind::MegaPine {
            crown_height: IntProvider::Constant(0),
        }
    }

    fn fancy() -> Kind {
        Kind::Fancy {
            height: IntProvider::Constant(0),
        }
    }

    #[test]
    fn the_acacia_s_top_layer_keeps_a_cross_and_the_inner_corners() {
        assert!(skips(Kind::Acacia, 2, 0, 2, 2, false));
        assert!(skips(Kind::Acacia, 1, 0, 2, 2, false));
        assert!(!skips(Kind::Acacia, 0, 0, 2, 2, false));
        assert!(!skips(Kind::Acacia, 1, 0, 1, 2, false));
    }

    #[test]
    fn the_acacia_s_lower_layers_leave_out_only_the_outer_corners() {
        assert!(skips(Kind::Acacia, 2, -1, 2, 2, false));
        assert!(!skips(Kind::Acacia, 1, -1, 2, 2, false));
        assert!(!skips(Kind::Acacia, 0, -1, 0, 0, false));
    }

    #[test]
    fn conifers_leave_out_only_the_corners_of_rows_wider_than_one_block() {
        assert!(skips(pine(), 2, 0, 2, 2, false));
        assert!(!skips(pine(), 1, 0, 2, 2, false));
        assert!(!skips(pine(), 0, 0, 0, 0, false));
    }

    #[test]
    fn the_mega_pine_s_rows_are_round_and_no_wider_than_seven_across_the_diagonal() {
        assert!(skips(mega_pine(), 4, 0, 3, 5, false));
        assert!(skips(mega_pine(), 4, 0, 4, 5, false));
        assert!(!skips(mega_pine(), 3, 0, 3, 5, false));
    }

    #[test]
    fn the_fancy_rows_are_circles_shifted_by_half_a_block() {
        assert!(!skips(fancy(), 0, 0, 0, 1, false));
        assert!(skips(fancy(), 1, 0, 0, 1, false));
        assert!(skips(fancy(), 0, 0, 0, 0, false));
    }

    #[test]
    fn the_dark_oak_s_double_trunk_leaves_out_the_outer_corners_of_its_middle_layer() {
        assert!(skips(Kind::DarkOak, -3, 0, -3, 3, true));
        assert!(skips(Kind::DarkOak, 4, 0, 4, 3, true));
        assert!(!skips(Kind::DarkOak, -2, 0, -3, 3, true));
    }

    #[test]
    fn the_dark_oak_s_side_crowns_leave_out_their_lower_corners() {
        assert!(skips(Kind::DarkOak, 2, -1, 2, 2, false));
        assert!(!skips(Kind::DarkOak, 2, -1, 1, 2, false));
    }

    #[test]
    fn the_dark_oak_s_upper_layer_is_cut_on_the_diagonal() {
        assert!(skips(Kind::DarkOak, 3, 1, 2, 3, false));
        assert!(!skips(Kind::DarkOak, 2, 1, 2, 3, false));
    }

    #[test]
    fn a_poplar_s_full_row_is_a_rhombus_with_one_diagonal_cut_short() {
        // Radius 3, not partial, unflipped: the corners where the signs differ are cut
        // to radius - 1, the others to the radius.
        assert!(poplar_within_rhombus(
            3,
            3,
            0,
            poplar_corner_cut(3, 0, 3, false, false),
            0
        ));
        assert!(poplar_within_rhombus(
            3,
            1,
            2,
            poplar_corner_cut(1, 2, 3, false, false),
            0
        ));
        assert!(!poplar_within_rhombus(
            3,
            2,
            2,
            poplar_corner_cut(2, 2, 3, false, false),
            0
        ));
        assert!(poplar_within_rhombus(
            3,
            2,
            2,
            poplar_corner_cut(2, -2, 3, false, false),
            0
        ));
        assert!(!poplar_within_rhombus(
            3,
            2,
            3,
            poplar_corner_cut(2, -3, 3, false, false),
            0
        ));
        // Flipped, the other diagonal is the short one.
        assert!(poplar_within_rhombus(
            3,
            2,
            2,
            poplar_corner_cut(2, 2, 3, false, true),
            0
        ));
    }

    #[test]
    fn the_two_top_rows_of_a_poplar_are_partial() {
        assert!(poplar_row_is_partial(8, 7));
        assert!(poplar_row_is_partial(8, 6));
        assert!(!poplar_row_is_partial(8, 5));
    }
}
