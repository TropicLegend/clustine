// Written for Clustine from the pinned jar's classes
// (`RandomSpreadStructurePlacement`, `AbstractSpreadingStructurePlacement` and its
// `FrequencyReductionMethod` and `ExclusionZone`, `ConcentricRingsStructurePlacement`,
// `ChunkGeneratorStructureState.generateRingPositions`, `BiomeSource.findBiomeHorizontal`),
// with SteelMC's steel-worldgen/src/structure/placement.rs (commit
// 885c4b3e60ed79862c37311780774f76806cb714) read beside them. See NOTICE in this crate.

//! Which chunks a structure set starts its structures in.
//!
//! A set that is spread at random has one candidate chunk in every square of
//! `spacing` chunks, drawn from the world's seed, the square and the set's salt. A
//! set in concentric rings (the strongholds) has a fixed number of chunks around the
//! origin, each moved to a biome the set prefers if one is within 112 blocks. Either
//! kind may then thin its candidates out by a frequency and keep away from the start
//! chunks of another set.

use clustine_noise::random::{LegacyRandom, Random};
use clustine_terrain::biome::BiomeSource;
use clustine_worldgen_data::structure::{
    FrequencyReduction, Placement, Spread, SpreadType, StructureSet,
};

/// A number below `limit` for a spread: evenly, or the mean of two draws.
fn spread(kind: SpreadType, random: &mut LegacyRandom, limit: i32) -> i32 {
    match kind {
        SpreadType::Linear => random.next_i32_bounded(limit),
        // The sum is halved as the game halves it, towards zero.
        SpreadType::Triangular => {
            (random.next_i32_bounded(limit) + random.next_i32_bounded(limit)) / 2
        }
    }
}

/// The candidate chunk of the square of a random spread that holds a chunk
/// (`RandomSpreadStructurePlacement.getPotentialStructureChunk`).
#[must_use]
pub fn potential_chunk(
    seed: i64,
    salt: i32,
    spacing: i32,
    separation: i32,
    kind: SpreadType,
    chunk_x: i32,
    chunk_z: i32,
) -> (i32, i32) {
    let square_x = chunk_x.div_euclid(spacing);
    let square_z = chunk_z.div_euclid(spacing);
    let mut random = LegacyRandom::from_seed(0);
    random.set_large_feature_with_salt(seed, square_x, square_z, salt);
    let limit = spacing - separation;
    let x = spread(kind, &mut random, limit);
    let z = spread(kind, &mut random, limit);
    (square_x * spacing + x, square_z * spacing + z)
}

/// Whether a candidate chunk survives the set's thinning
/// (`FrequencyReductionMethod.shouldGenerate`). The default method gives the salt
/// where the seeding expects the chunk's x, as the game does.
#[must_use]
pub fn frequency_allows(
    method: FrequencyReduction,
    seed: i64,
    salt: i32,
    chunk_x: i32,
    chunk_z: i32,
    probability: f32,
) -> bool {
    let mut random = LegacyRandom::from_seed(0);
    match method {
        FrequencyReduction::Default => {
            random.set_large_feature_with_salt(seed, salt, chunk_x, chunk_z);
            random.next_f32() < probability
        }
        FrequencyReduction::LegacyType1 => {
            let x = chunk_x >> 4;
            let z = chunk_z >> 4;
            random.set_seed(i64::from(x ^ (z << 4)) ^ seed);
            random.next_i32();
            random.next_i32_bounded((1.0_f32 / probability) as i32) == 0
        }
        FrequencyReduction::LegacyType2 => {
            random.set_large_feature_with_salt(seed, chunk_x, chunk_z, 10_387_320);
            random.next_f32() < probability
        }
        FrequencyReduction::LegacyType3 => {
            random.set_large_feature_seed(seed, chunk_x, chunk_z);
            random.next_f64() < f64::from(probability)
        }
    }
}

/// Java's `Math.round` of a double.
fn round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

/// The game's `BiomeSource.findBiomeHorizontal` as the rings ask it: every cell of
/// the square of `radius` blocks around a block, row by row, and of those whose
/// biome is wanted one drawn so that each is as likely. Gives the block of the cell.
fn find_biome(
    biomes: &BiomeSource,
    x: i32,
    y: i32,
    z: i32,
    radius: i32,
    wanted: &[u8],
    random: &mut LegacyRandom,
) -> Option<(i32, i32)> {
    let (cell_x, cell_y, cell_z, reach) = (x >> 2, y >> 2, z >> 2, radius >> 2);
    let mut found = None;
    let mut count = 0;
    // Where two biomes are exactly tied the game answers by what its thread looked
    // up last, which within this walk is the cell before.
    let mut before = None;
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let biome = biomes.biome_after(cell_x + dx, cell_y, cell_z + dz, &mut before);
            let is_wanted =
                u8::try_from(biome.0).is_ok_and(|number| wanted.binary_search(&number).is_ok());
            if is_wanted {
                if found.is_none() || random.next_i32_bounded(count + 1) == 0 {
                    found = Some(((cell_x + dx) << 2, (cell_z + dz) << 2));
                }
                count += 1;
            }
        }
    }
    found
}

/// The chunks of a set placed in concentric rings
/// (`ChunkGeneratorStructureState.generateRingPositions`), in the game's order.
#[must_use]
pub fn ring_positions(
    seed: i64,
    distance: i32,
    spread: i32,
    count: i32,
    preferred_biomes: &[u8],
    biomes: &BiomeSource,
) -> Vec<(i32, i32)> {
    use std::f64::consts::PI;
    let mut positions = Vec::new();
    if count == 0 {
        return positions;
    }
    let mut random = LegacyRandom::from_seed(0);
    random.set_seed(seed);
    let mut angle = random.next_f64() * PI * 2.0;
    let mut spread = spread;
    let mut in_circle = 0;
    let mut circle = 0;
    for index in 0..count {
        let far = f64::from(4 * distance + distance * circle * 6)
            + (random.next_f64() - 0.5) * (f64::from(distance) * 2.5);
        let chunk_x = round(angle.cos() * far);
        let chunk_z = round(angle.sin() * far);
        let mut forked = random.fork();
        let found = find_biome(
            biomes,
            chunk_x * 16 + 8,
            0,
            chunk_z * 16 + 8,
            112,
            preferred_biomes,
            &mut forked,
        );
        positions.push(match found {
            Some((x, z)) => (x >> 4, z >> 4),
            None => (chunk_x, chunk_z),
        });
        angle += PI * 2.0 / f64::from(spread);
        in_circle += 1;
        if in_circle == spread {
            circle += 1;
            in_circle = 0;
            spread += 2 * spread / (circle + 1);
            spread = spread.min(count - index);
            angle += random.next_f64() * PI * 2.0;
        }
    }
    positions
}

/// The placements of the sets of one dimension of a world, with the rings of those
/// that have rings.
pub struct Placements<'a> {
    pub seed: i64,
    /// Every set of the game, by its place; the rings of a set that has none are
    /// empty.
    pub sets: &'static [StructureSet],
    pub rings: &'a [Vec<(i32, i32)>],
}

impl Placements<'_> {
    /// Whether a chunk is a candidate of a placement, before thinning and exclusion
    /// (`isPlacementChunk`).
    fn is_placement_chunk(&self, set: usize, placement: &Placement, x: i32, z: i32) -> bool {
        match placement.spread {
            Spread::Random {
                spacing,
                separation,
                spread_type,
            } => {
                potential_chunk(
                    self.seed,
                    placement.salt,
                    spacing,
                    separation,
                    spread_type,
                    x,
                    z,
                ) == (x, z)
            }
            Spread::ConcentricRings { .. } => self.rings[set].contains(&(x, z)),
        }
    }

    /// Whether a set starts a structure in a chunk (`StructurePlacement.isStructureChunk`).
    #[must_use]
    pub fn is_structure_chunk(&self, set: usize, x: i32, z: i32) -> bool {
        let placement = &self.sets[set].placement;
        if !self.is_placement_chunk(set, placement, x, z) {
            return false;
        }
        if placement.frequency < 1.0
            && !frequency_allows(
                placement.frequency_reduction,
                self.seed,
                placement.salt,
                x,
                z,
                placement.frequency,
            )
        {
            return false;
        }
        match placement.exclusion_zone {
            Some(zone) => !self.has_structure_chunk_in_range(
                usize::from(zone.other_set),
                x,
                z,
                zone.chunk_count,
            ),
            None => true,
        }
    }

    /// Whether a set starts a structure within so many chunks of a chunk
    /// (`ChunkGeneratorStructureState.hasStructureChunkInRange`).
    #[must_use]
    pub fn has_structure_chunk_in_range(&self, set: usize, x: i32, z: i32, range: i32) -> bool {
        for near_x in x - range..=x + range {
            for near_z in z - range..=z + range {
                if self.is_structure_chunk(set, near_x, near_z) {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_of_a_spread_has_one_candidate_and_it_lies_in_the_square() {
        for (x, z) in [(0, 0), (-1, -1), (100, -257), (-3400, 77)] {
            let (found_x, found_z) =
                potential_chunk(13579, 10_387_312, 34, 8, SpreadType::Linear, x, z);
            assert_eq!(found_x.div_euclid(34), x.div_euclid(34));
            assert_eq!(found_z.div_euclid(34), z.div_euclid(34));
            assert!(found_x.rem_euclid(34) < 26 && found_z.rem_euclid(34) < 26);
            assert_eq!(
                potential_chunk(
                    13579,
                    10_387_312,
                    34,
                    8,
                    SpreadType::Linear,
                    found_x,
                    found_z
                ),
                (found_x, found_z)
            );
        }
    }

    #[test]
    fn rounding_is_javas() {
        assert_eq!(round(0.5), 1);
        assert_eq!(round(-0.5), 0);
        assert_eq!(round(-1.5), -1);
        assert_eq!(round(2.4), 2);
    }
}
