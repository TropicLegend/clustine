// Written for Clustine from the pinned jar's own classes (`BiomeManager`,
// `LinearCongruentialGenerator`).

//! The biome at a block: which of the biome cells around it the game takes.
//!
//! A biome is made for cells of four blocks ([`super`]). At a block the game does not
//! take the cell the block lies in. It looks at the eight cells whose middles are
//! around the block, moves each middle by an offset hashed from the world's seed and
//! the cell, and takes the cell that is then nearest (`BiomeManager.getBiome`). That
//! frays the borders between biomes, and it is what the surface rules, the features
//! and a client ask for. Nothing of it is stored.
//!
//! The seed that is hashed is not the world's but its "obfuscated" seed: the first
//! eight bytes of the SHA-256 of the world's seed, which is also what a client is
//! sent.
//!
//! `tests/game_surface.rs` holds the cells to the game's own, with the other gates of
//! the surface.

/// The round constants of SHA-256 (FIPS 180-4).
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The first two words of the SHA-256 of eight bytes: one block of the algorithm.
fn sha256_of_eight(bytes: [u8; 8]) -> [u32; 2] {
    // The message, a one bit, zeros, and the length in bits: 64.
    let mut block = [0_u8; 64];
    block[..8].copy_from_slice(&bytes);
    block[8] = 0x80;
    block[63] = 64;

    let mut w = [0_u32; 64];
    for (word, four) in w.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes([four[0], four[1], four[2], four[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choice = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(choice)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    [state[0].wrapping_add(a), state[1].wrapping_add(b)]
}

/// The game's `BiomeManager.obfuscateSeed`: the first eight bytes of the SHA-256 of
/// the seed's eight bytes, both from the lowest byte. A client is sent this in place
/// of the world's seed.
#[must_use]
pub fn obfuscate_seed(seed: i64) -> i64 {
    let [first, second] = sha256_of_eight(seed.to_le_bytes());
    let mut bytes = [0_u8; 8];
    bytes[..4].copy_from_slice(&first.to_be_bytes());
    bytes[4..].copy_from_slice(&second.to_be_bytes());
    i64::from_le_bytes(bytes)
}

/// The game's `LinearCongruentialGenerator.next`.
#[inline]
fn next(seed: i64, salt: i64) -> i64 {
    seed.wrapping_mul(
        seed.wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407),
    )
    .wrapping_add(salt)
}

/// An offset of a cell's middle along one axis, within nine twentieths of a cell.
#[inline]
fn fiddle(seed: i64) -> f64 {
    let part = ((seed >> 24).rem_euclid(1024)) as f64 / 1024.0;
    (part - 0.5) * 0.9
}

/// The cells' offsets of one world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Zoom {
    seed: i64,
}

impl Zoom {
    /// The zoom of the world with this seed.
    #[must_use]
    pub fn new(world_seed: i64) -> Self {
        Self {
            seed: obfuscate_seed(world_seed),
        }
    }

    /// The zoom that hashes with this seed, which is the one a client is sent.
    #[must_use]
    pub fn of_obfuscated(seed: i64) -> Self {
        Self { seed }
    }

    /// The square of the distance from a point to the moved middle of the cell `x`,
    /// `y`, `z`; the point is given from the cell's corner, in cells.
    #[inline]
    fn distance(self, x: i32, y: i32, z: i32, from_x: f64, from_y: f64, from_z: f64) -> f64 {
        let mut seed = next(self.seed, i64::from(x));
        seed = next(seed, i64::from(y));
        seed = next(seed, i64::from(z));
        seed = next(seed, i64::from(x));
        seed = next(seed, i64::from(y));
        seed = next(seed, i64::from(z));
        let fiddle_x = fiddle(seed);
        seed = next(seed, self.seed);
        let fiddle_y = fiddle(seed);
        seed = next(seed, self.seed);
        let fiddle_z = fiddle(seed);
        let (dx, dy, dz) = (from_x + fiddle_x, from_y + fiddle_y, from_z + fiddle_z);
        dz * dz + dy * dy + dx * dx
    }

    /// The biome cell whose biome the block `x`, `y`, `z` has. The cell's height may
    /// lie above or below a chunk's cells: whoever reads the biome holds it within
    /// them, as a chunk of the game does.
    #[must_use]
    pub fn cell(self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        let (moved_x, moved_y, moved_z) = (x - 2, y - 2, z - 2);
        let (cell_x, cell_y, cell_z) = (moved_x >> 2, moved_y >> 2, moved_z >> 2);
        let within_x = f64::from(moved_x & 3) / 4.0;
        let within_y = f64::from(moved_y & 3) / 4.0;
        let within_z = f64::from(moved_z & 3) / 4.0;
        let mut nearest = 0;
        let mut least = f64::INFINITY;
        for corner in 0..8 {
            // A clear bit is the near side of its axis.
            let (near_x, near_y, near_z) = (corner & 4 == 0, corner & 2 == 0, corner & 1 == 0);
            let distance = self.distance(
                if near_x { cell_x } else { cell_x + 1 },
                if near_y { cell_y } else { cell_y + 1 },
                if near_z { cell_z } else { cell_z + 1 },
                if near_x { within_x } else { within_x - 1.0 },
                if near_y { within_y } else { within_y - 1.0 },
                if near_z { within_z } else { within_z - 1.0 },
            );
            if least > distance {
                nearest = corner;
                least = distance;
            }
        }
        (
            cell_x + i32::from(nearest & 4 != 0),
            cell_y + i32::from(nearest & 2 != 0),
            cell_z + i32::from(nearest & 1 != 0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_of_eight_bytes_is_the_standards() {
        // The SHA-256 of eight zero bytes begins af5570f5 a1810b7a.
        assert_eq!(sha256_of_eight([0; 8]), [0xaf55_70f5, 0xa181_0b7a]);
    }

    #[test]
    fn the_obfuscated_seed_is_the_hashs_first_bytes_from_the_lowest() {
        assert_eq!(
            obfuscate_seed(0),
            i64::from_le_bytes([0xaf, 0x55, 0x70, 0xf5, 0xa1, 0x81, 0x0b, 0x7a])
        );
        assert_ne!(obfuscate_seed(13579), obfuscate_seed(13580));
    }

    #[test]
    fn a_block_has_the_biome_of_one_of_the_eight_cells_around_it() {
        let zoom = Zoom::new(13579);
        for (x, y, z) in [(0, 0, 0), (-1, -64, 17), (1000, 319, -1000), (7, 70, 9)] {
            let (cell_x, cell_y, cell_z) = zoom.cell(x, y, z);
            for (cell, block) in [(cell_x, x), (cell_y, y), (cell_z, z)] {
                let low = (block - 2) >> 2;
                assert!(cell == low || cell == low + 1, "{cell} of {block}");
            }
            assert_eq!(zoom.cell(x, y, z), (cell_x, cell_y, cell_z));
        }
    }

    #[test]
    fn an_offset_stays_within_nine_twentieths_of_a_cell() {
        let mut seed = 1_i64;
        for salt in 0..1000 {
            seed = next(seed, salt);
            assert!(fiddle(seed).abs() <= 0.45);
        }
    }
}
