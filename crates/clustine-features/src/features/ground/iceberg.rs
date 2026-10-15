// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/iceberg.rs`.

//! `iceberg`: a berg of packed or blue ice standing in the sea at sea level, round or
//! an ellipse, smoothed, with a part under the water, snow on top now and then and a
//! cut through it (`IcebergFeature`).
//!
//! An iceberg is up to 22 blocks across and reaches past the chunks its run may write;
//! those blocks are refused by the view, as the game's `ensureCanWrite` refuses them.

use std::f64::consts::PI;

use clustine_data::{BlockState, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{read_state, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Iceberg {
    state: BlockState,
}

impl Iceberg {
    pub fn read(reader: &Reader) -> Read<Self> {
        let iceberg = Self {
            state: read_state(&reader.field("state")?)?,
        };
        reader.finish()?;
        Ok(iceberg)
    }
}

/// What one iceberg is drawn to be.
struct Shape {
    main: BlockState,
    snow_on_top: bool,
    angle: f64,
    ellipse_a: i32,
    ellipse_c: i32,
    is_ellipse: bool,
}

/// One block's place in a layer of the berg.
struct Layer {
    height: i32,
    x: i32,
    y: i32,
    z: i32,
    radius: i32,
    a: i32,
}

/// `Mth.ceil` of a float.
fn ceil(value: f32) -> i32 {
    value.ceil() as i32
}

/// `Math.pow(y, 2.0)` of an int, as a float.
fn squared(y: i32) -> f32 {
    (f64::from(y) * f64::from(y)) as f32
}

fn is_iceberg_state(state: BlockState) -> bool {
    same_block(state, blocks::PACKED_ICE)
        || same_block(state, blocks::SNOW_BLOCK)
        || same_block(state, blocks::BLUE_ICE)
}

fn height_dependent_radius_round<R: Random + ?Sized>(
    random: &mut R,
    y: i32,
    height: i32,
    width: i32,
) -> i32 {
    let k = 3.5 - random.next_f32();
    let mut scale = (1.0 - squared(y) / (height as f32 * k)) * width as f32;
    if height > 15 + random.next_i32_bounded(5) {
        let y = if y < 3 + random.next_i32_bounded(6) {
            y / 2
        } else {
            y
        };
        scale = (1.0 - y as f32 / (height as f32 * k * 0.4)) * width as f32;
    }
    ceil(scale / 2.0)
}

fn height_dependent_radius_ellipse(y: i32, height: i32, width: i32) -> i32 {
    let scale = (1.0 - squared(y) / (height as f32 * 1.0)) * width as f32;
    ceil(scale / 2.0)
}

fn height_dependent_radius_steep<R: Random + ?Sized>(
    random: &mut R,
    y: i32,
    height: i32,
    width: i32,
) -> i32 {
    let k = 1.0 + random.next_f32() / 2.0;
    let scale = (1.0 - y as f32 / (height as f32 * k)) * width as f32;
    ceil(scale / 2.0)
}

/// `signedDistanceCircle`: below nought inside the circle, with a drawn wobble.
fn signed_distance_circle<R: Random + ?Sized>(random: &mut R, x: i32, z: i32, radius: i32) -> f64 {
    let offset = 10.0 * random.next_f32().clamp(0.2, 0.8) / radius as f32;
    f64::from(offset) + f64::from(x) * f64::from(x) + f64::from(z) * f64::from(z)
        - f64::from(radius) * f64::from(radius)
}

/// `signedDistanceEllipse`: below nought inside the ellipse of half-axes `a` and `c`
/// turned by `angle` around `(origin_x, origin_z)`.
fn signed_distance_ellipse(
    x: i32,
    z: i32,
    (origin_x, origin_z): (i32, i32),
    a: i32,
    c: i32,
    angle: f64,
) -> f64 {
    // `Math.cos` and `Math.sin`, as the ores have them.
    let (cos, sin) = (libm::cos(angle), libm::sin(angle));
    let dx = f64::from(x - origin_x);
    let dz = f64::from(z - origin_z);
    let u = (dx * cos - dz * sin) / f64::from(a);
    let v = (dx * sin + dz * cos) / f64::from(c);
    u * u + v * v - 1.0
}

/// `getEllipseC`: the ellipse narrows over the top three layers.
fn ellipse_c(y: i32, height: i32, shape_c: i32) -> i32 {
    if y > 0 && height - y <= 3 {
        shape_c - (4 - (height - y))
    } else {
        shape_c
    }
}

fn set(level: &mut Level<'_>, pos: BlockPos, state: BlockState) {
    level.set_block(pos, state);
}

impl Shape {
    /// `generateIcebergBlock`.
    fn block(&self, context: &mut Context<'_, '_>, origin: BlockPos, layer: &Layer) {
        let distance = if self.is_ellipse {
            signed_distance_ellipse(
                layer.x,
                layer.z,
                (0, 0),
                layer.a,
                ellipse_c(layer.y, layer.height, self.ellipse_c),
                self.angle,
            )
        } else {
            signed_distance_circle(&mut *context.random, layer.x, layer.z, layer.radius)
        };
        if distance >= 0.0 || distance.is_nan() {
            return;
        }
        let compare = if self.is_ellipse {
            -0.5
        } else {
            -6.0 - f64::from(context.random.next_i32_bounded(3))
        };
        if distance > compare && context.random.next_f64() > 0.9 {
            return;
        }
        let pos = origin.offset(layer.x, layer.y, layer.z);
        self.shape_block(context, pos, layer.height - layer.y, layer.height);
    }

    /// `setIcebergBlock`: ice, or snow towards the top where the berg has snow.
    fn shape_block(
        &self,
        context: &mut Context<'_, '_>,
        pos: BlockPos,
        below_top: i32,
        height: i32,
    ) {
        let state = context.level.block(pos);
        let open = state.props().is_air()
            || same_block(state, blocks::SNOW_BLOCK)
            || same_block(state, blocks::ICE)
            || same_block(state, blocks::WATER);
        if !open {
            return;
        }
        let random = &mut *context.random;
        let randomness = !self.is_ellipse || random.next_f64() > 0.05;
        let divisor = if self.is_ellipse { 3 } else { 2 };
        if self.snow_on_top
            && !same_block(state, blocks::WATER)
            && f64::from(below_top)
                <= f64::from(random.next_i32_bounded((height / divisor).max(1)))
                    + f64::from(height) * 0.6
            && randomness
        {
            set(context.level, pos, blocks::SNOW_BLOCK);
        } else {
            set(context.level, pos, self.main);
        }
    }
}

/// `smooth`: what hangs over air falls away with the block above it, and a block of
/// ice with three open sides is taken off.
fn smooth(level: &mut Level<'_>, origin: BlockPos, height: i32, a: i32) {
    for x in -a..=a {
        for z in -a..=a {
            for y in 0..=height {
                let pos = origin.offset(x, y, z);
                let state = level.block(pos);
                if !is_iceberg_state(state) && !same_block(state, blocks::SNOW) {
                    continue;
                }
                if level.block(pos.offset(0, -1, 0)).props().is_air() {
                    set(level, pos, blocks::AIR);
                    set(level, pos.offset(0, 1, 0), blocks::AIR);
                } else if is_iceberg_state(state) {
                    let open = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                        .into_iter()
                        .filter(|(dx, dz)| !is_iceberg_state(level.block(pos.offset(*dx, 0, *dz))))
                        .count();
                    if open >= 3 {
                        set(level, pos, blocks::AIR);
                    }
                }
            }
        }
    }
}

/// The cut through a berg.
struct Cut {
    radius: i32,
    y: i32,
    under_water: bool,
    angle: f64,
    local: (i32, i32),
}

/// `carve`: one layer of the cut.
fn carve(level: &mut Level<'_>, origin: BlockPos, cut: &Cut, ellipse_a: i32, ellipse_c: i32) {
    let a = cut.radius + 1 + ellipse_a / 3;
    let c = (cut.radius - 3).min(3) + ellipse_c / 2 - 1;
    for x in -a..a {
        for z in -a..a {
            let distance = signed_distance_ellipse(x, z, cut.local, a, c, cut.angle);
            if distance >= 0.0 || distance.is_nan() {
                continue;
            }
            let pos = origin.offset(x, cut.y, z);
            let state = level.block(pos);
            if !is_iceberg_state(state) && !same_block(state, blocks::SNOW_BLOCK) {
                continue;
            }
            if cut.under_water {
                set(level, pos, blocks::WATER);
            } else {
                set(level, pos, blocks::AIR);
                // `removeFloatingSnowLayer`.
                let above = pos.offset(0, 1, 0);
                if same_block(level.block(above), blocks::SNOW) {
                    set(level, above, blocks::AIR);
                }
            }
        }
    }
}

impl Shape {
    /// `generateCutOut`.
    fn cut_out(&self, context: &mut Context<'_, '_>, origin: BlockPos, width: i32, height: i32) {
        let random = &mut *context.random;
        let sign_x = if random.next_bool() { -1 } else { 1 };
        let sign_z = if random.next_bool() { -1 } else { 1 };
        let mut x = random.next_i32_bounded((width / 2 - 2).max(1));
        if random.next_bool() {
            x = width / 2 + 1 - random.next_i32_bounded((width - width / 2 - 1).max(1));
        }
        let mut z = random.next_i32_bounded((width / 2 - 2).max(1));
        if random.next_bool() {
            z = width / 2 + 1 - random.next_i32_bounded((width - width / 2 - 1).max(1));
        }
        if self.is_ellipse {
            x = random.next_i32_bounded((self.ellipse_a - 5).max(1));
            z = x;
        }
        let local = (sign_x * x, sign_z * z);
        let angle = if self.is_ellipse {
            self.angle + PI / 2.0
        } else {
            random.next_f64() * 2.0 * PI
        };
        for y in 0..height - 3 {
            let radius = height_dependent_radius_round(&mut *context.random, y, height, width);
            let cut = Cut {
                radius,
                y,
                under_water: false,
                angle,
                local,
            };
            carve(context.level, origin, &cut, self.ellipse_a, self.ellipse_c);
        }
        // The bound is drawn anew for each layer, as the game's loop does.
        let mut y = -1;
        while y > -height + context.random.next_i32_bounded(5) {
            let radius = height_dependent_radius_steep(&mut *context.random, -y, height, width);
            let cut = Cut {
                radius,
                y,
                under_water: true,
                angle,
                local,
            };
            carve(context.level, origin, &cut, self.ellipse_a, self.ellipse_c);
            y -= 1;
        }
    }
}

impl Feature for Iceberg {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let sea_level = context.settings.generation().sea_level;
        let origin = BlockPos::new(origin.x, sea_level, origin.z);
        let random = &mut *context.random;
        let snow_on_top = random.next_f64() > 0.7;
        let angle = random.next_f64() * 2.0 * PI;
        let ellipse_a = 11 - random.next_i32_bounded(5);
        let ellipse_c = 3 + random.next_i32_bounded(3);
        let is_ellipse = random.next_f64() > 0.7;
        let mut over_water = if is_ellipse {
            random.next_i32_bounded(6) + 6
        } else {
            random.next_i32_bounded(15) + 3
        };
        if !is_ellipse && random.next_f64() > 0.9 {
            over_water += random.next_i32_bounded(19) + 7;
        }
        let under_water = (over_water + random.next_i32_bounded(11)).min(18);
        let width = (over_water + random.next_i32_bounded(7) - random.next_i32_bounded(5)).min(11);
        let a = if is_ellipse { ellipse_a } else { 11 };
        let shape = Shape {
            main: self.state,
            snow_on_top,
            angle,
            ellipse_a,
            ellipse_c,
            is_ellipse,
        };

        for x in -a..a {
            for z in -a..a {
                for y in 0..over_water {
                    let radius = if is_ellipse {
                        height_dependent_radius_ellipse(y, over_water, width)
                    } else {
                        height_dependent_radius_round(&mut *context.random, y, over_water, width)
                    };
                    if is_ellipse || x < radius {
                        let layer = Layer {
                            height: over_water,
                            x,
                            y,
                            z,
                            radius,
                            a,
                        };
                        shape.block(context, origin, &layer);
                    }
                }
            }
        }

        let smooth_a = if is_ellipse { ellipse_a } else { width / 2 };
        smooth(context.level, origin, over_water, smooth_a);

        for x in -a..a {
            for z in -a..a {
                for y in (-(under_water - 1)..=-1).rev() {
                    let layer_a = if is_ellipse {
                        ceil(a as f32 * (1.0 - squared(y) / (under_water as f32 * 8.0)))
                    } else {
                        a
                    };
                    let radius =
                        height_dependent_radius_steep(&mut *context.random, -y, under_water, width);
                    if x < radius {
                        let layer = Layer {
                            height: under_water,
                            x,
                            y,
                            z,
                            radius,
                            a: layer_a,
                        };
                        shape.block(context, origin, &layer);
                    }
                }
            }
        }

        let cut = if is_ellipse {
            context.random.next_f64() > 0.1
        } else {
            context.random.next_f64() > 0.7
        };
        if cut {
            shape.cut_out(context, origin, width, over_water);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn named(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    fn sea() -> World {
        let mut world = World::flat(blocks::STONE, 30);
        for x in -32..48 {
            for z in -32..48 {
                for y in 31..=62 {
                    world.set(BlockPos::new(x, y, z), blocks::WATER);
                }
            }
        }
        world.writes.clear();
        world
    }

    #[test]
    fn an_iceberg_stands_in_the_sea_above_and_below_its_level() {
        let iceberg = named("minecraft:iceberg_packed");
        for seed in 0..8 {
            let mut world = sea();
            assert!(world.with_context(seed, |context| {
                iceberg.place(context, BlockPos::new(8, 0, 8))
            }));
            let ice: Vec<_> = world
                .writes
                .iter()
                .filter(|(_, state)| *state == blocks::PACKED_ICE)
                .collect();
            assert!(!ice.is_empty(), "{seed}");
            assert!(ice.iter().any(|(pos, _)| pos.y >= 63), "{seed}");
            assert!(ice.iter().any(|(pos, _)| pos.y < 63), "{seed}");
            for (pos, state) in &world.writes {
                assert!(
                    [
                        blocks::PACKED_ICE,
                        blocks::SNOW_BLOCK,
                        blocks::AIR,
                        blocks::WATER
                    ]
                    .contains(state),
                    "{state:?}"
                );
                assert!(
                    (pos.x - 8).abs() <= 16 && (pos.z - 8).abs() <= 16,
                    "{pos:?}"
                );
                assert!((63 - 18..=63 + 44).contains(&pos.y), "{pos:?}");
            }
        }
    }

    #[test]
    fn a_blue_iceberg_is_made_of_blue_ice() {
        let iceberg = named("minecraft:iceberg_blue");
        let mut world = sea();
        world.with_context(3, |context| iceberg.place(context, BlockPos::new(8, 0, 8)));
        assert!(
            world
                .writes
                .iter()
                .any(|(_, state)| *state == blocks::BLUE_ICE)
        );
        assert!(
            !world
                .writes
                .iter()
                .any(|(_, state)| *state == blocks::PACKED_ICE)
        );
    }

    #[test]
    fn an_iceberg_at_the_edge_of_the_three_by_three_writes_nothing_beyond_it() {
        let iceberg = named("minecraft:iceberg_packed");
        let mut world = sea();
        world.with_context(5, |context| {
            iceberg.place(context, BlockPos::new(30, 0, 30))
        });
        assert!(!world.writes.is_empty());
        assert!(world.writes.iter().all(|(pos, _)| pos.x < 32 && pos.z < 32));
    }

    #[test]
    fn the_shapes_distances_are_the_game_s() {
        assert_eq!(ellipse_c(0, 10, 4), 4);
        assert_eq!(ellipse_c(8, 10, 4), 2);
        assert_eq!(height_dependent_radius_ellipse(0, 10, 8), 4);
        assert!(signed_distance_ellipse(0, 0, (0, 0), 5, 3, 0.3) < 0.0);
        assert!(signed_distance_ellipse(6, 0, (0, 0), 5, 3, 0.0) > 0.0);
    }
}
