// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/dripstone/large.rs`.

//! `large_dripstone`: a pair of great cones of dripstone blocks, one hanging from a
//! cave's ceiling and one standing on its floor, their bases set into the stone, bent
//! by a wind where both are wide and blunt enough (`LargeDripstoneFeature`).

use std::f32::consts::{PI, TAU};

use clustine_data::blocks;
use clustine_noise::{Random, math, trig};
use clustine_world::BlockPos;

use super::shared::{
    Column, at_y, clamp, int_max, int_min, is_base_stone_overworld, is_empty_or_water,
    is_empty_or_water_or_lava, is_lava, random_between,
};
use crate::blocks::{BlockSet, same_block};
use crate::context::{Context, Heightmap, Level};
use crate::feature::Feature;
use crate::reader::{Read, Reader};
use crate::values::{FloatProvider, IntProvider};

#[derive(Debug)]
pub struct LargeDripstone {
    replaceable: BlockSet,
    floor_to_ceiling_search_range: i32,
    column_radius: IntProvider,
    height_scale: FloatProvider,
    max_column_radius_to_cave_height_ratio: f32,
    stalactite_bluntness: FloatProvider,
    stalagmite_bluntness: FloatProvider,
    wind_speed: FloatProvider,
    min_radius_for_wind: i32,
    min_bluntness_for_wind: f32,
}

impl LargeDripstone {
    pub fn read(reader: &Reader) -> Read<Self> {
        let float = |key: &str| FloatProvider::read(&reader.field(key)?);
        let feature = Self {
            replaceable: BlockSet::read(&reader.field("replaceable_blocks")?)?,
            floor_to_ceiling_search_range: match reader.optional("floor_to_ceiling_search_range") {
                Some(range) => range.int()?,
                None => 30,
            },
            column_radius: IntProvider::read(&reader.field("column_radius")?)?,
            height_scale: float("height_scale")?,
            max_column_radius_to_cave_height_ratio: reader
                .field("max_column_radius_to_cave_height_ratio")?
                .float()?,
            stalactite_bluntness: float("stalactite_bluntness")?,
            stalagmite_bluntness: float("stalagmite_bluntness")?,
            wind_speed: float("wind_speed")?,
            min_radius_for_wind: reader.field("min_radius_for_wind")?.int()?,
            min_bluntness_for_wind: reader.field("min_bluntness_for_wind")?.float()?,
        };
        reader.finish()?;
        Ok(feature)
    }

    /// `isDripstoneBaseOrLava` as 26.3 has it: dripstone, what may be replaced, lava.
    fn is_base_or_lava(&self, state: clustine_data::BlockState) -> bool {
        same_block(state, blocks::DRIPSTONE_BLOCK)
            || self.replaceable.contains(state)
            || same_block(state, blocks::LAVA)
    }
}

impl Feature for LargeDripstone {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !is_empty_or_water(context.level.block(origin)) {
            return false;
        }
        let Some(column) = Column::scan(
            context.level,
            origin,
            self.floor_to_ceiling_search_range,
            is_empty_or_water,
            |state| self.is_base_or_lava(state),
        ) else {
            return false;
        };
        let (Some(floor), Some(ceiling)) = (column.floor, column.ceiling) else {
            return false;
        };
        let height = ceiling - floor - 1;
        if height < 4 {
            return false;
        }

        let max_from_height = (height as f32 * self.max_column_radius_to_cave_height_ratio) as i32;
        let min_radius = int_min(&self.column_radius);
        let max_radius = clamp(max_from_height, min_radius, int_max(&self.column_radius));
        let radius = context.random.next_i32_between(min_radius, max_radius);

        let random = &mut *context.random;
        let mut stalactite = Cone::new(
            at_y(origin, ceiling - 1),
            false,
            random,
            radius,
            self.stalactite_bluntness,
            self.height_scale,
        );
        let mut stalagmite = Cone::new(
            at_y(origin, floor + 1),
            true,
            random,
            radius,
            self.stalagmite_bluntness,
            self.height_scale,
        );
        let wind = if stalactite.suits_wind(self) && stalagmite.suits_wind(self) {
            Wind::new(origin.y, random, self.wind_speed, 16 - radius)
        } else {
            Wind::none()
        };

        let stalactite_embedded =
            stalactite.move_back_until_base_is_inside_stone(context.level, &wind);
        let stalagmite_embedded =
            stalagmite.move_back_until_base_is_inside_stone(context.level, &wind);
        if stalactite_embedded {
            stalactite.place_blocks(context, &wind);
        }
        if stalagmite_embedded {
            stalagmite.place_blocks(context, &wind);
        }
        true
    }
}

/// `DripstoneUtils.getDripstoneHeight`: how high a cone of `radius` stands at a
/// distance from its axis.
pub fn dripstone_height(distance: f64, radius: f64, scale: f64, bluntness: f64) -> f64 {
    let distance = if distance < bluntness {
        bluntness
    } else {
        distance
    };
    let cutoff = 0.384;
    let r = distance / radius * cutoff;
    let part1 = 0.75 * libm::pow(r, 4.0 / 3.0);
    let part2 = libm::pow(r, 2.0 / 3.0);
    let part3 = (1.0 / 3.0) * libm::log(r);
    let height = (scale * (part1 - part2 - part3)).max(0.0);
    height / cutoff * radius
}

/// `isCircleMostlyEmbeddedInStone`: the centre and points round the circle, none of
/// them air or fluid.
fn is_circle_embedded_in_stone(level: &mut Level<'_>, centre: BlockPos, radius: i32) -> bool {
    if is_empty_or_water_or_lava(level.block(centre)) {
        return false;
    }
    let increment = 6.0 / radius as f32;
    let mut angle = 0.0_f32;
    while angle < TAU {
        let dx = (trig::cos(f64::from(angle)) * radius as f32) as i32;
        let dz = (trig::sin(f64::from(angle)) * radius as f32) as i32;
        if is_empty_or_water_or_lava(level.block(centre.offset(dx, 0, dz))) {
            return false;
        }
        angle += increment;
    }
    true
}

/// One of the two cones (`LargeDripstone.LargeDripstone`).
struct Cone {
    root: BlockPos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl Cone {
    fn new<R: Random + ?Sized>(
        root: BlockPos,
        pointing_up: bool,
        random: &mut R,
        radius: i32,
        bluntness: FloatProvider,
        scale: FloatProvider,
    ) -> Self {
        let bluntness = f64::from(bluntness.sample(random));
        let scale = f64::from(scale.sample(random));
        Self {
            root,
            pointing_up,
            radius,
            bluntness,
            scale,
        }
    }

    fn height_at(&self, distance: f32) -> i32 {
        dripstone_height(
            f64::from(distance),
            f64::from(self.radius),
            self.scale,
            self.bluntness,
        ) as i32
    }

    fn suits_wind(&self, feature: &LargeDripstone) -> bool {
        self.radius >= feature.min_radius_for_wind
            && self.bluntness >= f64::from(feature.min_bluntness_for_wind)
    }

    /// `moveBackUntilBaseIsInsideStoneAndShrinkRadiusIfNecessary`: up to ten steps back
    /// into the ground for a base set in stone, halving the radius each time none is
    /// found, until the radius is one.
    fn move_back_until_base_is_inside_stone(&mut self, level: &mut Level<'_>, wind: &Wind) -> bool {
        let step = if self.pointing_up { -1 } else { 1 };
        while self.radius > 1 {
            let mut root = self.root;
            let tries = self.height_at(0.0).min(10);
            for _ in 0..tries {
                if is_lava(level, root) {
                    return false;
                }
                if is_circle_embedded_in_stone(level, wind.offset(root), self.radius) {
                    self.root = root;
                    return true;
                }
                root = root.offset(0, step, 0);
            }
            self.radius /= 2;
        }
        false
    }

    /// `placeBlocks`: each column of the cone, from its base towards its point, through
    /// air and fluid, until it is back in stone.
    fn place_blocks(&self, context: &mut Context<'_, '_>, wind: &Wind) {
        let step = if self.pointing_up { 1 } else { -1 };
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let distance = ((dx * dx + dz * dz) as f32).sqrt();
                if distance > self.radius as f32 {
                    continue;
                }
                let mut height = self.height_at(distance);
                if height <= 0 {
                    continue;
                }
                if context.random.next_f32() < 0.2 {
                    height =
                        (height as f32 * random_between(&mut *context.random, 0.8, 1.0)) as i32;
                }
                let mut pos = self.root.offset(dx, 0, dz);
                let mut out_of_stone = false;
                let max_y = if self.pointing_up {
                    context
                        .level
                        .height_at(Heightmap::WorldSurfaceWg, pos.x, pos.z)
                } else {
                    i32::MAX
                };
                let mut i = 0;
                while i < height && pos.y < max_y {
                    let at = wind.offset(pos);
                    let state = context.level.block(at);
                    if is_empty_or_water_or_lava(state) {
                        out_of_stone = true;
                        context.level.set_block(at, blocks::DRIPSTONE_BLOCK);
                    } else if out_of_stone && is_base_stone_overworld(state) {
                        break;
                    }
                    pos = pos.offset(0, step, 0);
                    i += 1;
                }
            }
        }
    }
}

/// `WindOffsetter`: how far a block of the cones is pushed sideways at a height, in
/// proportion to its distance from the origin's height and at most `max_offset`.
struct Wind {
    origin_y: i32,
    speed: Option<(f64, f64)>,
    max_offset: i32,
}

impl Wind {
    fn new<R: Random + ?Sized>(
        origin_y: i32,
        random: &mut R,
        speed: FloatProvider,
        max_offset: i32,
    ) -> Self {
        let speed = speed.sample(random);
        let direction = random_between(random, 0.0, PI);
        Self {
            origin_y,
            speed: Some((
                f64::from(trig::cos(f64::from(direction)) * speed),
                f64::from(trig::sin(f64::from(direction)) * speed),
            )),
            max_offset,
        }
    }

    fn none() -> Self {
        Self {
            origin_y: 0,
            speed: None,
            max_offset: 0,
        }
    }

    fn offset(&self, pos: BlockPos) -> BlockPos {
        let Some((x, z)) = self.speed else {
            return pos;
        };
        let dy = f64::from(self.origin_y - pos.y);
        let clamp = |value: f64| {
            math::floor(value)
                .max(-self.max_offset)
                .min(self.max_offset)
        };
        pos.offset(clamp(x * dy), 0, clamp(z * dy))
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn large_dripstone() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:large_dripstone").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn the_height_of_a_cone_falls_from_its_axis_to_its_edge() {
        let axis = dripstone_height(0.0, 6.0, 1.0, 0.5);
        let middle = dripstone_height(3.0, 6.0, 1.0, 0.5);
        let edge = dripstone_height(6.0, 6.0, 1.0, 0.5);
        assert!(axis > middle && middle > edge, "{axis} {middle} {edge}");
        // Blunter cones are as high at the axis as at their bluntness.
        assert_eq!(
            dripstone_height(0.0, 6.0, 1.0, 2.0),
            dripstone_height(2.0, 6.0, 1.0, 2.0)
        );
        // The formula of `DripstoneUtils`, worked by hand at r = 0.384 / 2.
        let r: f64 = 0.192;
        let expected = (0.75 * r.powf(4.0 / 3.0) - r.powf(2.0 / 3.0) - r.ln() / 3.0) / 0.384 * 6.0;
        assert!((dripstone_height(3.0, 6.0, 1.0, 0.5) - expected).abs() < 1e-12);
    }

    #[test]
    fn a_large_dripstone_fills_part_of_a_tall_cave_with_dripstone() {
        let feature = large_dripstone();
        let mut placed_any = false;
        for seed in 0..10 {
            let mut world = World::flat(blocks::STONE, 100);
            for x in -14..=14 {
                for z in -14..=14 {
                    for y in 40..70 {
                        world.set(BlockPos::new(x, y, z), blocks::AIR);
                    }
                }
            }
            assert!(world.with_context(seed, |context| {
                feature.place(context, BlockPos::new(0, 55, 0))
            }));
            assert!(
                world
                    .writes
                    .iter()
                    .all(|(_, state)| *state == blocks::DRIPSTONE_BLOCK)
            );
            placed_any |= !world.writes.is_empty();
        }
        assert!(placed_any);
    }

    #[test]
    fn a_cave_lower_than_four_blocks_has_no_large_dripstone() {
        let feature = large_dripstone();
        let mut world = World::flat(blocks::STONE, 100);
        for y in 50..53 {
            world.set(BlockPos::new(0, y, 0), blocks::AIR);
        }
        assert!(!world.with_context(1, |context| {
            feature.place(context, BlockPos::new(0, 51, 0))
        }));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn the_wind_pushes_by_the_distance_from_the_origin_within_its_limit() {
        let wind = Wind {
            origin_y: 50,
            speed: Some((0.25, -0.1)),
            max_offset: 3,
        };
        assert_eq!(
            wind.offset(BlockPos::new(0, 46, 0)),
            BlockPos::new(1, 46, -1)
        );
        assert_eq!(
            wind.offset(BlockPos::new(0, 20, 0)),
            BlockPos::new(3, 20, -3)
        );
        assert_eq!(
            Wind::none().offset(BlockPos::new(1, 2, 3)),
            BlockPos::new(1, 2, 3)
        );
    }
}
