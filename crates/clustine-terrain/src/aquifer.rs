// Written for Clustine from the pinned jar's own classes (`Aquifer`,
// `Aquifer$NoiseBasedAquifer`, `Aquifer$FluidStatus`, and
// `NoiseBasedChunkGenerator.createFluidPicker`), method by method. SteelMC's
// steel-worldgen/src/noise/aquifer.rs at 885c4b3 (AGPL-3.0-or-later, Copyright (C)
// 2026 Alve Jeansson and contributors; see NOTICE) was read beside them, and the
// names of the parts and the table of offsets follow it where the two agree.

//! Which fluid, if any, is where there is no ground: the game's aquifers.
//!
//! The fill asks for every block whose density is not above zero what it holds. The
//! Nether and the End answer by the height alone ([`FluidPicker`]): a sea of lava
//! below 32, and nothing. The overworld has aquifers: the world is divided into a
//! grid of 16 by 12 by 16 blocks, each cell of which has a centre at a place drawn
//! from the seed and its position, and each centre a fluid and a level, worked out
//! from how high the ground is around it and from three noises. A block takes the
//! fluid of the nearest centre; between two centres whose fluids differ the game adds
//! a "pressure" to the density, which leaves a wall of stone between them.
//!
//! An aquifer is made for a volume, as the game makes one for each chunk it fills (and
//! one for each column it asks a base height of). It remembers centres and their
//! fluids as it goes, which changes no answer; but **the volume changes answers**:
//! above a height that depends on the highest ground near the volume, the aquifer
//! answers by the sea level alone, so a column asked alone can differ from the same
//! column in its chunk. That is the game's, and `base_height` has it too.
//!
//! Beside the block the game says whether the block is to be looked at again when the
//! chunk is first ticked ("should schedule fluid update"): where two centres with
//! different fluids are near, the fluid placed here may have to flow.

use clustine_data::{BlockState, blocks};
use clustine_noise::{NameHash, PositionalRandom, Random, RandomSplitter};
use clustine_worldgen_data::{AquiferEntry, NoiseRouter};

use crate::noise_chunk::Volume;

/// Below this the fluid of the world at large is lava, whatever the sea is.
const LAVA_LEVEL: i32 = -54;
/// The game's `DimensionType.WAY_BELOW_MIN_Y`: the level of a centre that has no
/// fluid.
const WAY_BELOW_MIN_Y: i32 = -32_512;

/// The grid of centres: a cell is 16 by 12 by 16 blocks, and its centre lies within
/// the first 10 by 9 by 10 of them.
const Y_SPACING: i32 = 12;
const X_RANGE: i32 = 10;
const Y_RANGE: i32 = 9;
const Z_RANGE: i32 = 10;
/// A block looks for centres in the cells from the one this far from it.
const SAMPLE_OFFSET_X: i32 = -5;
const SAMPLE_OFFSET_Y: i32 = 1;
const SAMPLE_OFFSET_Z: i32 = -5;

/// Where around a centre the height of the ground is asked, in chunks.
const SURFACE_SAMPLING_OFFSETS_IN_CHUNKS: [[i32; 2]; 13] = [
    [0, 0],
    [-2, -1],
    [-1, -1],
    [0, -1],
    [1, -1],
    [-3, 0],
    [-2, 0],
    [-1, 0],
    [1, 0],
    [-2, 1],
    [-1, 1],
    [0, 1],
    [1, 1],
];

/// A fluid up to a level: what one centre, or the world at large, has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FluidStatus {
    /// The first height that is above the fluid.
    pub level: i32,
    pub fluid: BlockState,
}

impl FluidStatus {
    /// What is at the height `y`: the fluid below the level, air from it upwards.
    #[inline]
    #[must_use]
    pub fn at(self, y: i32) -> BlockState {
        if y < self.level {
            self.fluid
        } else {
            blocks::AIR
        }
    }
}

/// The fluid of the world at large (the game's `createFluidPicker`): lava up to -54
/// for what is below the lower of -54 and the sea level, and the dimension's fluid up
/// to the sea level above.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FluidPicker {
    lava: FluidStatus,
    sea: FluidStatus,
    /// The lower of -54 and the sea level.
    lava_below: i32,
}

impl FluidPicker {
    #[must_use]
    pub fn new(sea_level: i32, default_fluid: BlockState) -> Self {
        Self {
            lava: FluidStatus {
                level: LAVA_LEVEL,
                fluid: blocks::LAVA,
            },
            sea: FluidStatus {
                level: sea_level,
                fluid: default_fluid,
            },
            lava_below: LAVA_LEVEL.min(sea_level),
        }
    }

    /// The fluid at a height; the game's picker is asked with a position and reads
    /// the height alone.
    #[inline]
    #[must_use]
    pub fn at(&self, y: i32) -> FluidStatus {
        if y < self.lava_below {
            self.lava
        } else {
            self.sea
        }
    }
}

/// The game's `Mth.floorDiv` by sixteen and its inverse, for the grid's x and z.
#[inline]
fn grid_xz(block: i32) -> i32 {
    block >> 4
}

#[inline]
fn grid_y(block: i32) -> i32 {
    block.div_euclid(Y_SPACING)
}

#[inline]
fn from_grid_xz(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

#[inline]
fn from_grid_y(grid: i32, offset: i32) -> i32 {
    grid * Y_SPACING + offset
}

/// How alike the distances to two centres are: above zero where the farther is less
/// than 25 farther (in squares), one where they are equal.
#[inline]
fn similarity(nearer: i32, farther: i32) -> f64 {
    1.0 - f64::from(farther - nearer) / 25.0
}

/// At and above this likeness of the distances, a fluid may have to flow.
fn flowing_update_similarity() -> f64 {
    similarity(10 * 10, 12 * 12)
}

/// The game's `Mth.clampedMap` and `Mth.map` in double precision.
fn inverse_lerp(value: f64, from: f64, to: f64) -> f64 {
    (value - from) / (to - from)
}

fn lerp(alpha: f64, from: f64, to: f64) -> f64 {
    from + alpha * (to - from)
}

fn map(value: f64, from_low: f64, from_high: f64, to_low: f64, to_high: f64) -> f64 {
    lerp(inverse_lerp(value, from_low, from_high), to_low, to_high)
}

fn clamped_map(value: f64, from_low: f64, from_high: f64, to_low: f64, to_high: f64) -> f64 {
    let alpha = inverse_lerp(value, from_low, from_high);
    if alpha < 0.0 {
        to_low
    } else if alpha > 1.0 {
        to_high
    } else {
        lerp(alpha, to_low, to_high)
    }
}

/// The game's `Mth.clamp` of a double: the lower bound below it, else the smaller of
/// the value and the upper bound.
fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if value < low { low } else { value.min(high) }
}

/// The aquifer of a volume. What it remembers changes no answer.
pub struct Aquifer<'a, R: NoiseRouter> {
    picker: FluidPicker,
    /// `None` in a dimension without aquifers: the picker answers alone.
    noise: Option<NoiseBased<'a, R>>,
    should_schedule_fluid_update: bool,
}

/// The game's `NoiseBasedAquifer`.
struct NoiseBased<'a, R: NoiseRouter> {
    router: &'a R,
    memory: R::Memory,
    /// Draws the centre of a cell from the cell's position.
    positions: RandomSplitter,
    /// The cells the volume's blocks can ask, as the game sizes them.
    min_grid: [i32; 3],
    grid_size_x: i32,
    grid_size_z: i32,
    /// The centre of each cell once it was drawn.
    centres: Vec<Option<[i32; 3]>>,
    /// The fluid of each centre once it was worked out.
    statuses: Vec<Option<FluidStatus>>,
    /// Above this height the picker answers alone.
    skip_sampling_above_y: i32,
    /// The router's surface level by column, on a grid of four blocks, floored;
    /// `i32::MIN` where it was not asked yet. From `surface_min` in units of four
    /// blocks, `surface_size` each way.
    surface: Vec<i32>,
    surface_min: [i32; 2],
    surface_size: [i32; 2],
}

/// The seed's factory of the aquifers' positions (`RandomState.getOrCreateRandomFactory`
/// of `minecraft:aquifer`), from the dimension's factory.
#[must_use]
pub fn positions(dimension: &RandomSplitter) -> RandomSplitter {
    const AQUIFER: NameHash = NameHash::new("minecraft:aquifer");
    dimension.with_hash_of(&AQUIFER).next_positional()
}

impl<'a, R: NoiseRouter> Aquifer<'a, R> {
    /// The aquifer of `volume`. `positions` is [`positions`] of the dimension's
    /// factory; a dimension without aquifers does not read it.
    #[must_use]
    pub fn new(router: &'a R, positions: &RandomSplitter, volume: &Volume) -> Self {
        let settings = router.settings();
        let picker = FluidPicker::new(
            settings.sea_level,
            BlockState(settings.default_fluid as u16),
        );
        let noise = settings
            .has_aquifers
            .then(|| NoiseBased::new(router, positions.clone(), volume));
        Self {
            picker,
            noise,
            should_schedule_fluid_update: false,
        }
    }

    /// The fluid of the world at large.
    #[must_use]
    pub fn picker(&self) -> FluidPicker {
        self.picker
    }

    /// The height above which the fluid of the world at large answers alone, in a
    /// dimension with aquifers.
    #[must_use]
    pub fn skip_sampling_above_y(&self) -> Option<i32> {
        self.noise.as_ref().map(|noise| noise.skip_sampling_above_y)
    }

    /// What the block at `x`, `y`, `z` holds, its density being `density`: `None` for
    /// the ground (the dimension's default block), else air or a fluid.
    pub fn compute_substance(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> Option<BlockState> {
        if density > 0.0 {
            self.should_schedule_fluid_update = false;
            return None;
        }
        let Some(noise) = &mut self.noise else {
            self.should_schedule_fluid_update = false;
            return Some(self.picker.at(y).at(y));
        };
        let (state, schedule) = noise.compute_substance(&self.picker, x, y, z, density);
        self.should_schedule_fluid_update = schedule;
        state
    }

    /// Whether the block last asked for is to be looked at again when its chunk is
    /// first ticked, if it is a fluid.
    #[must_use]
    pub fn should_schedule_fluid_update(&self) -> bool {
        self.should_schedule_fluid_update
    }
}

impl<'a, R: NoiseRouter> NoiseBased<'a, R> {
    fn new(router: &'a R, positions: RandomSplitter, volume: &Volume) -> Self {
        let [max_x, max_y, max_z] = volume.max();

        let min_grid_x = grid_xz(volume.min_x + SAMPLE_OFFSET_X);
        let max_grid_x = grid_xz(max_x + SAMPLE_OFFSET_X) + 1;
        let grid_size_x = max_grid_x - min_grid_x + 1;
        let min_grid_y = grid_y(volume.min_y + SAMPLE_OFFSET_Y) - 1;
        let max_grid_y = grid_y(max_y + SAMPLE_OFFSET_Y) + 1;
        let grid_size_y = max_grid_y - min_grid_y + 1;
        let min_grid_z = grid_xz(volume.min_z + SAMPLE_OFFSET_Z);
        let max_grid_z = grid_xz(max_z + SAMPLE_OFFSET_Z) + 1;
        let grid_size_z = max_grid_z - min_grid_z + 1;
        let cells = (grid_size_x * grid_size_y * grid_size_z) as usize;

        // A centre lies within its cell's first ten blocks, and the ground is asked
        // up to three chunks before it and one after.
        let lowest = SURFACE_SAMPLING_OFFSETS_IN_CHUNKS
            .iter()
            .fold([0, 0], |low, offset| {
                [low[0].min(offset[0]), low[1].min(offset[1])]
            });
        let highest = SURFACE_SAMPLING_OFFSETS_IN_CHUNKS
            .iter()
            .fold([0, 0], |high, offset| {
                [high[0].max(offset[0]), high[1].max(offset[1])]
            });
        let surface_min = [
            (from_grid_xz(min_grid_x, 0) + lowest[0] * 16) >> 2,
            (from_grid_xz(min_grid_z, 0) + lowest[1] * 16) >> 2,
        ];
        let surface_max = [
            (from_grid_xz(max_grid_x, X_RANGE - 1) + highest[0] * 16) >> 2,
            (from_grid_xz(max_grid_z, Z_RANGE - 1) + highest[1] * 16) >> 2,
        ];
        let surface_size = [
            surface_max[0] - surface_min[0] + 1,
            surface_max[1] - surface_min[1] + 1,
        ];

        let mut aquifer = Self {
            router,
            memory: R::Memory::default(),
            positions,
            min_grid: [min_grid_x, min_grid_y, min_grid_z],
            grid_size_x,
            grid_size_z,
            centres: vec![None; cells],
            statuses: vec![None; cells],
            skip_sampling_above_y: 0,
            surface: vec![i32::MIN; (surface_size[0] * surface_size[1]) as usize],
            surface_min,
            surface_size,
        };

        // The highest ground under any centre the volume can ask.
        let mut highest_surface = i32::MIN;
        let from_z = from_grid_xz(min_grid_z, 0) >> 2;
        let to_z = from_grid_xz(max_grid_z, Z_RANGE - 1) >> 2;
        let from_x = from_grid_xz(min_grid_x, 0) >> 2;
        let to_x = from_grid_xz(max_grid_x, X_RANGE - 1) >> 2;
        for quart_z in from_z..=to_z {
            for quart_x in from_x..=to_x {
                highest_surface =
                    highest_surface.max(aquifer.surface_level(quart_x << 2, quart_z << 2));
            }
        }
        let adjusted = highest_surface + 8;
        let skip_cell = grid_y(adjusted + 12) + 1;
        aquifer.skip_sampling_above_y = from_grid_y(skip_cell, 11) - 1;
        aquifer
    }

    /// The router's surface level at the column, on the grid of four blocks, rounded
    /// down.
    fn surface_level(&mut self, x: i32, z: i32) -> i32 {
        let quart_x = x >> 2;
        let quart_z = z >> 2;
        let column_x = quart_x - self.surface_min[0];
        let column_z = quart_z - self.surface_min[1];
        debug_assert!(
            (0..self.surface_size[0]).contains(&column_x)
                && (0..self.surface_size[1]).contains(&column_z),
            "a surface level is asked outside what a centre of this volume can reach"
        );
        let index = (column_z * self.surface_size[0] + column_x) as usize;
        if self.surface[index] == i32::MIN {
            let value = self.router.aquifer(
                &mut self.memory,
                AquiferEntry::SurfaceLevel,
                quart_x << 2,
                0,
                quart_z << 2,
            );
            self.surface[index] = f64::from(value).floor() as i32;
        }
        self.surface[index]
    }

    fn index(&self, grid_x: i32, grid_y: i32, grid_z: i32) -> usize {
        let x = grid_x - self.min_grid[0];
        let y = grid_y - self.min_grid[1];
        let z = grid_z - self.min_grid[2];
        ((y * self.grid_size_z + z) * self.grid_size_x + x) as usize
    }

    /// The block and whether it is to be looked at again.
    fn compute_substance(
        &mut self,
        picker: &FluidPicker,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> (Option<BlockState>, bool) {
        let global = picker.at(y);
        if y > self.skip_sampling_above_y {
            return (Some(global.at(y)), false);
        }
        if global.at(y) == blocks::LAVA {
            return (Some(blocks::LAVA), false);
        }

        // The four nearest centres of the twelve cells around.
        let anchor_x = grid_xz(x + SAMPLE_OFFSET_X);
        let anchor_y = grid_y(y + SAMPLE_OFFSET_Y);
        let anchor_z = grid_xz(z + SAMPLE_OFFSET_Z);
        let mut distances = [i32::MAX; 4];
        let mut nearest = [0_usize; 4];
        for offset_x in 0..=1 {
            for offset_y in -1..=1 {
                for offset_z in 0..=1 {
                    let grid = [
                        anchor_x + offset_x,
                        anchor_y + offset_y,
                        anchor_z + offset_z,
                    ];
                    let index = self.index(grid[0], grid[1], grid[2]);
                    let centre = match self.centres[index] {
                        Some(centre) => centre,
                        None => {
                            let mut random = self.positions.at(grid[0], grid[1], grid[2]);
                            // In this order: x, then y, then z.
                            let centre_x = from_grid_xz(grid[0], random.next_i32_bounded(X_RANGE));
                            let centre_y = from_grid_y(grid[1], random.next_i32_bounded(Y_RANGE));
                            let centre_z = from_grid_xz(grid[2], random.next_i32_bounded(Z_RANGE));
                            let centre = [centre_x, centre_y, centre_z];
                            self.centres[index] = Some(centre);
                            centre
                        }
                    };
                    let dx = centre[0] - x;
                    let dy = centre[1] - y;
                    let dz = centre[2] - z;
                    let distance = dx * dx + dy * dy + dz * dz;
                    // A centre as near as one found before goes in front of it.
                    if distances[0] >= distance {
                        nearest = [index, nearest[0], nearest[1], nearest[2]];
                        distances = [distance, distances[0], distances[1], distances[2]];
                    } else if distances[1] >= distance {
                        nearest = [nearest[0], index, nearest[1], nearest[2]];
                        distances = [distances[0], distance, distances[1], distances[2]];
                    } else if distances[2] >= distance {
                        nearest = [nearest[0], nearest[1], index, nearest[2]];
                        distances = [distances[0], distances[1], distance, distances[2]];
                    } else if distances[3] >= distance {
                        nearest[3] = index;
                        distances[3] = distance;
                    }
                }
            }
        }

        let flowing = flowing_update_similarity();
        let first = self.status(picker, nearest[0]);
        let first_second = similarity(distances[0], distances[1]);
        let state = first.at(y);
        if first_second <= 0.0 {
            // Well inside the first centre's ground.
            let schedule = first_second >= flowing && first != self.status(picker, nearest[1]);
            return (Some(state), schedule);
        }
        if state == blocks::WATER && picker.at(y - 1).at(y - 1) == blocks::LAVA {
            return (Some(state), true);
        }

        // The barrier's noise is asked once for the block, and only if a pressure
        // needs it.
        let mut barrier = f64::NAN;
        let second = self.status(picker, nearest[1]);
        let pressure = first_second * self.pressure(x, y, z, &mut barrier, first, second);
        if density + pressure > 0.0 {
            return (None, false);
        }
        let third = self.status(picker, nearest[2]);
        let first_third = similarity(distances[0], distances[2]);
        if first_third > 0.0 {
            let pressure =
                first_second * first_third * self.pressure(x, y, z, &mut barrier, first, third);
            if density + pressure > 0.0 {
                return (None, false);
            }
        }
        let second_third = similarity(distances[1], distances[2]);
        if second_third > 0.0 {
            let pressure =
                first_second * second_third * self.pressure(x, y, z, &mut barrier, second, third);
            if density + pressure > 0.0 {
                return (None, false);
            }
        }

        let schedule = if first != second
            || (second_third >= flowing && second != third)
            || (first_third >= flowing && first != third)
        {
            true
        } else {
            first_third >= flowing
                && similarity(distances[0], distances[3]) >= flowing
                && first != self.status(picker, nearest[3])
        };
        (Some(state), schedule)
    }

    /// What is added to the density between two centres: nothing where their fluids
    /// stand equally high, more the farther the block is inside the difference, and
    /// most between water and lava.
    fn pressure(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        barrier: &mut f64,
        first: FluidStatus,
        second: FluidStatus,
    ) -> f64 {
        let first_at = first.at(y);
        let second_at = second.at(y);
        if (first_at == blocks::LAVA && second_at == blocks::WATER)
            || (first_at == blocks::WATER && second_at == blocks::LAVA)
        {
            return 2.0;
        }
        let difference = (first.level - second.level).abs();
        if difference == 0 {
            return 0.0;
        }
        let middle = 0.5 * f64::from(first.level + second.level);
        let above_middle = f64::from(y) + 0.5 - middle;
        let half = f64::from(difference) / 2.0;
        let inside = half - above_middle.abs();
        let gradient = if above_middle > 0.0 {
            let centred = 0.0 + inside;
            if centred > 0.0 {
                centred / 1.5
            } else {
                centred / 2.5
            }
        } else {
            let centred = 3.0 + inside;
            if centred > 0.0 {
                centred / 3.0
            } else {
                centred / 10.0
            }
        };
        let noise = if (-2.0..=2.0).contains(&gradient) {
            if barrier.is_nan() {
                *barrier = f64::from(self.router.aquifer(
                    &mut self.memory,
                    AquiferEntry::Barrier,
                    x,
                    y,
                    z,
                ));
            }
            *barrier
        } else {
            0.0
        };
        2.0 * (noise + gradient)
    }

    /// The fluid of the centre of a cell.
    fn status(&mut self, picker: &FluidPicker, index: usize) -> FluidStatus {
        if let Some(status) = self.statuses[index] {
            return status;
        }
        let centre = self.centres[index].expect("a centre is drawn before its fluid is asked");
        let status = self.compute_fluid(picker, centre[0], centre[1], centre[2]);
        self.statuses[index] = Some(status);
        status
    }

    fn compute_fluid(&mut self, picker: &FluidPicker, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = picker.at(y);
        let mut lowest_surface = i32::MAX;
        let above = y + 12;
        let below = y - 12;
        let mut surface_here_is_under_the_fluid = false;
        for offset in SURFACE_SAMPLING_OFFSETS_IN_CHUNKS {
            let sample_x = x + (offset[0] << 4);
            let sample_z = z + (offset[1] << 4);
            let surface = self.surface_level(sample_x, sample_z);
            let adjusted = surface + 8;
            let here = offset == [0, 0];
            if here && below > adjusted {
                // In the open, above the ground.
                return global;
            }
            let reaches_the_surface = above > adjusted;
            if reaches_the_surface || here {
                let at_surface = picker.at(adjusted);
                if !at_surface.at(adjusted).is_air() {
                    if here {
                        surface_here_is_under_the_fluid = true;
                    }
                    if reaches_the_surface {
                        return at_surface;
                    }
                }
            }
            lowest_surface = lowest_surface.min(surface);
        }
        let level = self.compute_surface_level(
            x,
            y,
            z,
            global,
            lowest_surface,
            surface_here_is_under_the_fluid,
        );
        FluidStatus {
            level,
            fluid: self.compute_fluid_type(x, y, z, global, level),
        }
    }

    fn compute_surface_level(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        lowest_surface: i32,
        surface_here_is_under_the_fluid: bool,
    ) -> i32 {
        let exclusion = self
            .router
            .aquifer(&mut self.memory, AquiferEntry::Exclusion, x, y, z);
        let (partly, fully) = if f64::from(exclusion) > 0.0 {
            (-1.0, -1.0)
        } else {
            let under_the_surface = lowest_surface + 8 - y;
            let near_the_surface = if surface_here_is_under_the_fluid {
                clamped_map(f64::from(under_the_surface), 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let floodedness = clamp(
                f64::from(self.router.aquifer(
                    &mut self.memory,
                    AquiferEntry::FluidLevelFloodedness,
                    x,
                    y,
                    z,
                )),
                -1.0,
                1.0,
            );
            let fully_threshold = map(near_the_surface, 1.0, 0.0, -0.3, 0.8);
            let partly_threshold = map(near_the_surface, 1.0, 0.0, -0.8, 0.4);
            (
                floodedness - partly_threshold,
                floodedness - fully_threshold,
            )
        };
        if fully > 0.0 {
            global.level
        } else if partly > 0.0 {
            self.randomised_level(x, y, z, lowest_surface)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    fn randomised_level(&mut self, x: i32, y: i32, z: i32, lowest_surface: i32) -> i32 {
        let cell_x = x.div_euclid(16);
        let cell_y = y.div_euclid(40);
        let cell_z = z.div_euclid(16);
        let middle = cell_y * 40 + 20;
        // The noise is multiplied in single precision.
        let spread = f64::from(
            self.router.aquifer(
                &mut self.memory,
                AquiferEntry::FluidLevelSpread,
                cell_x,
                cell_y,
                cell_z,
            ) * 10.0,
        );
        let quantised = (spread / 3.0).floor() as i32 * 3;
        lowest_surface.min(middle + quantised)
    }

    fn compute_fluid_type(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        level: i32,
    ) -> BlockState {
        if level <= -10 && level != WAY_BELOW_MIN_Y && global.fluid != blocks::LAVA {
            let lava = f64::from(self.router.aquifer(
                &mut self.memory,
                AquiferEntry::Lava,
                x.div_euclid(64),
                y.div_euclid(40),
                z.div_euclid(64),
            ));
            if lava.abs() > 0.3 {
                return blocks::LAVA;
            }
        }
        global.fluid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fluid_of_the_world_at_large_is_lava_far_down_and_the_sea_above() {
        let overworld = FluidPicker::new(63, blocks::WATER);
        assert_eq!(overworld.at(-55).at(-55), blocks::LAVA);
        assert_eq!(overworld.at(-55).level, -54);
        assert_eq!(overworld.at(-54).at(-54), blocks::WATER);
        assert_eq!(overworld.at(62).at(62), blocks::WATER);
        assert_eq!(overworld.at(63).at(63), blocks::AIR);
        // The Nether's sea is lava from the bottom, the End has none: its sea level
        // is 0 and nothing lies below it.
        let nether = FluidPicker::new(32, blocks::LAVA);
        assert_eq!(nether.at(0).at(0), blocks::LAVA);
        assert_eq!(nether.at(31).at(31), blocks::LAVA);
        assert_eq!(nether.at(32).at(32), blocks::AIR);
        let end = FluidPicker::new(0, blocks::AIR);
        assert_eq!(end.at(0).at(0), blocks::AIR);
        assert_eq!(end.at(-60).at(-60), blocks::LAVA);
    }

    #[test]
    fn the_grid_counts_downwards_below_zero() {
        assert_eq!(grid_xz(-1), -1);
        assert_eq!(grid_xz(15), 0);
        assert_eq!(grid_y(-1), -1);
        assert_eq!(grid_y(-12), -1);
        assert_eq!(grid_y(-13), -2);
        assert_eq!(from_grid_xz(-2, 9), -23);
        assert_eq!(from_grid_y(-2, 8), -16);
    }

    #[test]
    fn likeness_is_one_for_equal_distances_and_nothing_25_apart() {
        assert_eq!(similarity(100, 100), 1.0);
        assert_eq!(similarity(100, 125), 0.0);
        assert!(similarity(100, 126) < 0.0);
        assert_eq!(flowing_update_similarity(), 1.0 - 44.0 / 25.0);
    }

    #[test]
    fn the_games_maps_and_clamp() {
        assert_eq!(clamped_map(-5.0, 0.0, 64.0, 1.0, 0.0), 1.0);
        assert_eq!(clamped_map(32.0, 0.0, 64.0, 1.0, 0.0), 0.5);
        assert_eq!(clamped_map(100.0, 0.0, 64.0, 1.0, 0.0), 0.0);
        assert_eq!(map(1.0, 1.0, 0.0, -0.3, 0.8), -0.3);
        assert_eq!(map(0.0, 1.0, 0.0, -0.3, 0.8), -0.3 + (0.8 - -0.3));
        assert_eq!(clamp(-3.0, -1.0, 1.0), -1.0);
        assert_eq!(clamp(3.0, -1.0, 1.0), 1.0);
        assert_eq!(clamp(0.25, -1.0, 1.0), 0.25);
    }
}
