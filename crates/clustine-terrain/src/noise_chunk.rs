// Written for Clustine from the pinned jar's own classes
// (`InterpolatedFunction$Sampler.sampleWithBlockStep` and `fillCell`,
// `NoiseBasedChunkGenerator.doFill` and `chunkVolume`, `DensityVolume`). SteelMC's
// steel-worldgen/src/noise/noise_chunk.rs at 885c4b3 (AGPL-3.0-or-later, Copyright (C)
// 2026 Alve Jeansson and contributors; see NOTICE) was read beside them for the shape
// of the work: corners first, then the blocks of each cell, the functions above the
// interpolation applied block by block.

//! The final density of a box of blocks, as the game fills it.
//!
//! The game's "fill from noise" step asks the router's final density for every block
//! of a chunk at once, as a volume. Two kinds of function do not give the same bits
//! that way as when they are asked at one block:
//!
//! - **The functions the data marks `interpolated`.** For a volume the game computes
//!   the input of such a function at the corners of the cells the volume touches, and
//!   then walks each cell: across z, then across x, and up y **by adding the same
//!   step again and again**, where a single position is interpolated in one
//!   expression. The last bit differs at about a third of the blocks.
//! - **A noise that is not shifted.** In a volume each of its layers multiplies the
//!   function's scale by its own frequency and the block coordinate by the product;
//!   at one position the coordinate is multiplied by the scale first. Where the scale
//!   is no power of two the two products can differ in the last bit of a double, and
//!   now and again a whole plane of corners then differs in the last bits of the
//!   noise: in 4 of 3,000 chunks of the overworld that were compared.
//!
//! A block whose density is that close to zero would be stone here and air there. So
//! this module does what the game does (`InterpolatedFunction$Sampler` and
//! `NoiseStack.addToVolume` of the pinned jar): the corners of each interpolated
//! function the final density reaches, asked as part of a volume
//! (`clustine_worldgen_data::Give::in_volume`); the walk through the cells in single
//! precision in the game's order of operations; and then, block by block, the
//! functions above the interpolation, which the emitted router computes with the
//! interpolated values given to it (`clustine_worldgen_data::Give::give`). In all
//! three dimensions every noise that is not shifted lies below an interpolation, so
//! nothing above it has two ways.
//!
//! The order in which the corners are asked is not the game's: a value does not depend
//! on it. They are asked column by column, every interpolated function at one corner
//! before the next corner, so that what the router remembers for a position and for a
//! column serves as often as it can.
//!
//! `tests/game_fill.rs` holds this to the game's own volume, bit for bit.

use clustine_worldgen_data::density::MAX_INTERPOLATED;
use clustine_worldgen_data::{Give, NoiseRouter, RouterEntry};

/// A box of blocks: `size_x` by `size_y` by `size_z` of them from the lowest corner.
/// The game's `DensityVolume` with a step of one block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Volume {
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub size_x: usize,
    pub size_y: usize,
    pub size_z: usize,
}

impl Volume {
    /// The blocks of a chunk between the heights of a dimension's noise settings:
    /// what the game fills at once.
    #[must_use]
    pub fn of_chunk(chunk_x: i32, chunk_z: i32, min_y: i32, height: i32) -> Self {
        Self {
            min_x: chunk_x << 4,
            min_y,
            min_z: chunk_z << 4,
            size_x: 16,
            size_y: height as usize,
            size_z: 16,
        }
    }

    /// One column of blocks, as the game makes it for a base height.
    #[must_use]
    pub fn of_column(x: i32, z: i32, min_y: i32, height: i32) -> Self {
        Self {
            min_x: x,
            min_y,
            min_z: z,
            size_x: 1,
            size_y: height as usize,
            size_z: 1,
        }
    }

    /// How many blocks there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.size_x * self.size_y * self.size_z
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Where the block at `x`, `y`, `z`, counted from the lowest corner, is in the
    /// values of the volume: the game's order, y fastest, then x, then z.
    #[inline]
    #[must_use]
    pub fn index(&self, x: usize, y: usize, z: usize) -> usize {
        y + (x + z * self.size_x) * self.size_y
    }

    /// Where the block at the block position `x`, `y`, `z` is in the values of the
    /// volume, or `None` for a block outside it (the game's `indexOfBlock`).
    #[inline]
    #[must_use]
    pub fn index_of_block(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let (x, y, z) = (
            usize::try_from(x.wrapping_sub(self.min_x)).ok()?,
            usize::try_from(y.wrapping_sub(self.min_y)).ok()?,
            usize::try_from(z.wrapping_sub(self.min_z)).ok()?,
        );
        (x < self.size_x && y < self.size_y && z < self.size_z).then(|| self.index(x, y, z))
    }

    /// The highest block of each axis: x, y, z.
    #[must_use]
    pub fn max(&self) -> [i32; 3] {
        [
            self.min_x + self.size_x as i32 - 1,
            self.min_y + self.size_y as i32 - 1,
            self.min_z + self.size_z as i32 - 1,
        ]
    }
}

/// The cells of one size that a volume touches along one axis, and the corners the
/// game computes for them.
#[derive(Clone, Copy, Debug)]
struct Axis {
    /// The size of a cell.
    cell: i32,
    /// The lowest block of the volume, and how many blocks it has.
    min: i32,
    size: i32,
    /// The first cell, counted in cells, and how many cells are touched.
    first: i32,
    cells: usize,
    /// How many corners there are: one more than cells, unless the volume ends
    /// exactly on a corner, where the game takes the last cell's far side to be its
    /// near side.
    corners: usize,
}

impl Axis {
    fn new(min: i32, size: usize, cell: i32) -> Self {
        let max = min + size as i32 - 1;
        let first = min.div_euclid(cell);
        let cells = (max.div_euclid(cell) - first + 1) as usize;
        let corners = cells + usize::from(max.rem_euclid(cell) != 0);
        Self {
            cell,
            min,
            size: size as i32,
            first,
            cells,
            corners,
        }
    }

    /// The block of a corner.
    fn block(&self, corner: usize) -> i32 {
        (self.first + corner as i32) * self.cell
    }

    /// The corner on the far side of a cell.
    fn next(&self, cell: usize) -> usize {
        (cell + 1).min(self.corners - 1)
    }

    /// For a cell: the block of its near corner counted from the volume's lowest, and
    /// the first and the last block of the cell that lie in the volume, counted from
    /// that corner.
    fn span(&self, cell: usize) -> (i32, i32, i32) {
        let from_min = self.block(cell) - self.min;
        let first = 0.max(-from_min);
        let last = self.cell.min(self.size - from_min) - 1;
        (from_min, first, last)
    }
}

/// `Mth.lerp` in single precision.
#[inline]
fn lerp(alpha: f32, from: f32, to: f32) -> f32 {
    from + alpha * (to - from)
}

/// What a fill needs room for, kept from one chunk to the next so that nothing is
/// allocated anew. It holds nothing that changes a value.
#[derive(Default, Debug)]
pub struct Scratch {
    /// The corners of each interpolated function, in the order of x, then z, then y.
    corners: Vec<Vec<f32>>,
    /// Each interpolated function at every block of the volume.
    blocks: Vec<Vec<f32>>,
}

impl Scratch {
    /// The interpolated function with this number at every block of the volume that
    /// was interpolated last ([`interpolated`]), in the volume's order; empty for a
    /// function that was not asked for.
    #[must_use]
    pub fn interpolated(&self, number: usize) -> &[f32] {
        self.blocks.get(number).map_or(&[], Vec::as_slice)
    }

    /// The corners that the last fill computed for the interpolated function with
    /// this number, in the order of the game's volume of corners: y fastest, then x,
    /// then z. For a test to compare with the game's.
    #[must_use]
    pub fn corners_in_the_games_order(
        &self,
        number: usize,
        corners: [usize; 3],
    ) -> Option<Vec<f32>> {
        let ours = self.corners.get(number).filter(|ours| !ours.is_empty())?;
        let [size_x, size_y, size_z] = corners;
        let mut theirs = Vec::with_capacity(ours.len());
        for z in 0..size_z {
            for x in 0..size_x {
                for y in 0..size_y {
                    theirs.push(ours[(x * size_z + z) * size_y + y]);
                }
            }
        }
        Some(theirs)
    }
}

/// The router's final density at every block of `volume`, in the volume's order
/// ([`Volume::index`]), as the game's `sampleVolume` gives it: bit for bit.
///
/// `memory` is the caller's for this router; nothing is left given in it.
pub fn final_density<R: NoiseRouter>(
    router: &R,
    memory: &mut R::Memory,
    volume: &Volume,
    scratch: &mut Scratch,
    out: &mut Vec<f32>,
) {
    let interpolated = router.interpolated();
    let wanted: Vec<usize> = (0..interpolated.len())
        .filter(|number| interpolated[*number].in_final_density)
        .collect();
    self::interpolated(router, memory, &wanted, volume, scratch);

    // What stands above the interpolation, block by block, a column at a time.
    out.clear();
    out.reserve(volume.len());
    for z in 0..volume.size_z {
        let block_z = volume.min_z + z as i32;
        for x in 0..volume.size_x {
            let block_x = volume.min_x + x as i32;
            let column = volume.index(x, 0, z);
            for y in 0..volume.size_y {
                for number in &wanted {
                    memory.give(*number, scratch.blocks[*number][column + y]);
                }
                out.push(router.sample(
                    memory,
                    RouterEntry::FinalDensity,
                    block_x,
                    volume.min_y + y as i32,
                    block_z,
                ));
            }
        }
    }
    memory.give_nothing();
}

/// The interpolated functions with the numbers `wanted` at every block of `volume`,
/// as the game interpolates them when it fills a volume: afterwards
/// [`Scratch::interpolated`] has each, in the volume's order. What the scratch held
/// of other functions is gone.
///
/// The final density is made of these ([`final_density`]); so are the router's
/// `chunk_surface_level`, which the surface rules read as a volume one block high,
/// and the density functions of the ore veins.
///
/// `memory` is the caller's for this router; nothing is left given in it.
pub fn interpolated<R: NoiseRouter>(
    router: &R,
    memory: &mut R::Memory,
    wanted: &[usize],
    volume: &Volume,
    scratch: &mut Scratch,
) {
    let interpolated = router.interpolated();
    assert!(
        interpolated.len() <= MAX_INTERPOLATED,
        "a router has at most {MAX_INTERPOLATED} interpolated functions"
    );
    scratch.corners.resize_with(interpolated.len(), Vec::new);
    scratch.blocks.resize_with(interpolated.len(), Vec::new);
    for (corners, blocks) in scratch.corners.iter_mut().zip(&mut scratch.blocks) {
        corners.clear();
        blocks.clear();
    }

    // The functions grouped by their cell sizes: those of one size are asked together
    // at each corner. (In the game's data all that the final density reaches have one
    // size.)
    let mut sizes: Vec<(i32, i32)> = Vec::new();
    for number in wanted {
        let size = (
            interpolated[*number].cell_size_xz,
            interpolated[*number].cell_size_y,
        );
        if !sizes.contains(&size) {
            sizes.push(size);
        }
    }

    memory.give_nothing();
    for (cell_xz, cell_y) in sizes {
        let group: Vec<usize> = wanted
            .iter()
            .copied()
            .filter(|number| {
                interpolated[*number].cell_size_xz == cell_xz
                    && interpolated[*number].cell_size_y == cell_y
            })
            .collect();
        let x_axis = Axis::new(volume.min_x, volume.size_x, cell_xz);
        let y_axis = Axis::new(volume.min_y, volume.size_y, cell_y);
        let z_axis = Axis::new(volume.min_z, volume.size_z, cell_xz);
        let count = x_axis.corners * y_axis.corners * z_axis.corners;
        for number in &group {
            scratch.corners[*number].resize(count, 0.0);
            scratch.blocks[*number].resize(volume.len(), 0.0);
        }

        // The corners, column by column, each as part of a volume: the game fills a
        // volume of corners, and a noise is not always the same there as when it is
        // asked alone.
        let before = memory.in_volume(true);
        for corner_x in 0..x_axis.corners {
            let x = x_axis.block(corner_x);
            for corner_z in 0..z_axis.corners {
                let z = z_axis.block(corner_z);
                let column = (corner_x * z_axis.corners + corner_z) * y_axis.corners;
                for corner_y in 0..y_axis.corners {
                    let y = y_axis.block(corner_y);
                    for number in &group {
                        scratch.corners[*number][column + corner_y] =
                            router.interpolated_input(memory, *number, x, y, z);
                    }
                }
            }
        }
        memory.in_volume(before);

        // The blocks of each cell, in the game's arithmetic.
        let inverse_xz = 1.0 / cell_xz as f32;
        let inverse_y = 1.0 / cell_y as f32;
        for number in &group {
            let corners = &scratch.corners[*number];
            let blocks = &mut scratch.blocks[*number];
            let corner = |x: usize, y: usize, z: usize| {
                corners[(x * z_axis.corners + z) * y_axis.corners + y]
            };
            for cell_z in 0..z_axis.cells {
                let far_z = z_axis.next(cell_z);
                let (from_z, first_z, last_z) = z_axis.span(cell_z);
                for cell_x in 0..x_axis.cells {
                    let far_x = x_axis.next(cell_x);
                    let (from_x, first_x, last_x) = x_axis.span(cell_x);
                    for cell_y in 0..y_axis.cells {
                        let far_y = y_axis.next(cell_y);
                        let (from_y, first_y, last_y) = y_axis.span(cell_y);
                        // Named by which side of the cell in x, y and z.
                        let c000 = corner(cell_x, cell_y, cell_z);
                        let c100 = corner(far_x, cell_y, cell_z);
                        let c010 = corner(cell_x, far_y, cell_z);
                        let c110 = corner(far_x, far_y, cell_z);
                        let c001 = corner(cell_x, cell_y, far_z);
                        let c101 = corner(far_x, cell_y, far_z);
                        let c011 = corner(cell_x, far_y, far_z);
                        let c111 = corner(far_x, far_y, far_z);

                        for in_z in first_z..=last_z {
                            let along_z = in_z as f32 * inverse_xz;
                            let low_near = lerp(along_z, c000, c001);
                            let high_near = lerp(along_z, c010, c011);
                            let low_far = lerp(along_z, c100, c101);
                            let high_far = lerp(along_z, c110, c111);
                            for in_x in first_x..=last_x {
                                let along_x = in_x as f32 * inverse_xz;
                                let low = lerp(along_x, low_near, low_far);
                                let high = lerp(along_x, high_near, high_far);
                                let step = (high - low) * inverse_y;
                                let mut value = low + step * first_y as f32;
                                let first = volume.index(
                                    (from_x + in_x) as usize,
                                    (from_y + first_y) as usize,
                                    (from_z + in_z) as usize,
                                );
                                let count = (last_y - first_y + 1) as usize;
                                for block in &mut blocks[first..first + count] {
                                    *block = value;
                                    value += step;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    memory.give_nothing();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_of_the_overworld_touches_four_cells_across_and_has_five_corners() {
        let volume = Volume::of_chunk(-3, 7, -64, 384);
        assert_eq!(volume.max(), [-33, 319, 127]);
        let x = Axis::new(volume.min_x, volume.size_x, 4);
        assert_eq!((x.first, x.cells, x.corners), (-12, 4, 5));
        assert_eq!(x.block(0), -48);
        assert_eq!(x.next(3), 4);
        assert_eq!(x.span(0), (0, 0, 3));
        assert_eq!(x.span(3), (12, 0, 3));
        let y = Axis::new(volume.min_y, volume.size_y, 8);
        assert_eq!((y.first, y.cells, y.corners), (-8, 48, 49));
        assert_eq!(y.span(47), (376, 0, 7));
    }

    #[test]
    fn a_column_that_is_not_on_a_corner_is_one_cell_and_one_block_of_it() {
        // The block 6 is the third of the cell from 4 to 8: the cell has two corners
        // and only that block of it is in the volume.
        let x = Axis::new(6, 1, 4);
        assert_eq!((x.first, x.cells, x.corners), (1, 1, 2));
        assert_eq!(x.span(0), (-2, 2, 2));
        // Below zero cells are counted downwards.
        let x = Axis::new(-1, 1, 4);
        assert_eq!((x.first, x.cells, x.corners), (-1, 1, 2));
        assert_eq!(x.block(0), -4);
        assert_eq!(x.span(0), (-3, 3, 3));
    }

    #[test]
    fn a_column_on_a_corner_has_that_corner_alone() {
        // The volume ends on a corner, so the game computes no far side and takes
        // the near side for it.
        let x = Axis::new(8, 1, 4);
        assert_eq!((x.first, x.cells, x.corners), (2, 1, 1));
        assert_eq!(x.next(0), 0);
        assert_eq!(x.span(0), (0, 0, 0));
    }

    #[test]
    fn the_values_of_a_volume_are_in_the_games_order() {
        let volume = Volume::of_chunk(0, 0, 0, 128);
        assert_eq!(volume.len(), 16 * 16 * 128);
        assert_eq!(volume.index(0, 5, 0), 5);
        assert_eq!(volume.index(1, 0, 0), 128);
        assert_eq!(volume.index(0, 0, 1), 16 * 128);
        assert!(!volume.is_empty());
    }

    #[test]
    fn the_steps_up_a_cell_are_added_one_by_one() {
        // What makes a volume differ from a single position: a step added three
        // times is not always three steps at once. Of a few pairs of corners some
        // differ in the last bit, and none by more than that matters to the eye.
        let mut differing = 0;
        for tenth in 1..10 {
            let low = tenth as f32 * 0.1;
            let high = 0.7_f32 - low;
            let step = (high - low) * (1.0 / 8.0);
            let mut value = low + step * 0.0;
            for _ in 0..3 {
                value += step;
            }
            let at_once = lerp(3.0 / 8.0, low, high);
            differing += usize::from(value.to_bits() != at_once.to_bits());
            assert!((value - at_once).abs() < 1e-6);
        }
        assert!(differing > 0);
    }
}
