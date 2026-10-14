// Written from the bytecode of the pinned jar's `Beardifier` and of `Mth.fastInvSqrt`,
// with SteelMC's steel-worldgen/src/noise/beardifier.rs (commit
// 885c4b3e60ed79862c37311780774f76806cb714) read beside them. See NOTICE in this crate.

//! What structures do to the ground: the density the game adds to the fill's final
//! density near the pieces of structures that adapt the terrain (its `Beardifier`).
//!
//! A village's houses stand on ground that was raised under them and cleared above
//! them; an ancient city has a hollow around it; a stronghold and a trail ruin are
//! buried; a trial chamber is walled in. All of it is one number added to the
//! density of every block within 24 blocks of such a piece, before the aquifer
//! decides what the block is: positive makes ground, negative makes room.
//!
//! - A **piece** adds by how its structure adapts the terrain: a *beard* below its
//!   floor and room above it (`beard_thin`, and `beard_box` which counts the
//!   distance to the whole box), ground all around it (`bury`), or a shell around
//!   its box (`encapsulate`).
//! - A **junction** of two pieces of a jigsaw structure adds a small beard at the
//!   point where they meet, so that a path between houses lies on ground.
//!
//! The beard's strength falls off by a kernel of 24 by 24 by 24 numbers, which the
//! game makes once with `Math.pow`; `crates/clustine-structures/tests` holds the
//! kernel made here to the game's, bit for bit.
//!
//! Which pieces and junctions are near a chunk is `clustine-structures`' to say
//! (the game's `Beardifier.forStructuresInChunk`); this module is what the fill adds
//! once it is told ([`crate::fill::Terrain::fill_bent`]). The sums are in single
//! precision and in the order given, as the game's are: the order is part of the
//! result.

use std::sync::LazyLock;

/// How a structure adapts the terrain around a piece (`TerrainAdjustment`, without
/// "none").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Adjustment {
    Bury,
    BeardThin,
    BeardBox,
    Encapsulate,
}

/// A piece that bends the ground: its box, both ends included, and how far above
/// the box's lowest block its floor is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rigid {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub adjustment: Adjustment,
    pub ground_level_delta: i32,
}

/// Where two pieces of a jigsaw structure meet: the block, at the ground's height.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Junction {
    pub x: i32,
    pub ground_y: i32,
    pub z: i32,
}

/// How far the kernel reaches from its middle.
const KERNEL_RADIUS: i32 = 12;
/// The kernel's side.
const KERNEL_SIZE: usize = 24;

/// The game's `BEARD_KERNEL`: for every offset of up to twelve blocks, e to the
/// power of minus a sixteenth of the squared distance, the height counted from the
/// middle of the block. Index: `z * 24 * 24 + x * 24 + y`.
static KERNEL: LazyLock<Vec<f32>> = LazyLock::new(|| {
    let mut kernel = vec![0.0; KERNEL_SIZE * KERNEL_SIZE * KERNEL_SIZE];
    for z in 0..KERNEL_SIZE {
        for x in 0..KERNEL_SIZE {
            for y in 0..KERNEL_SIZE {
                let dx = f64::from(x as i32 - KERNEL_RADIUS);
                let dy = f64::from(y as i32 - KERNEL_RADIUS) + 0.5;
                let dz = f64::from(z as i32 - KERNEL_RADIUS);
                let squared = dx * dx + dy * dy + dz * dz;
                // The game raises its constant e to the power; it does not call exp.
                let value = std::f64::consts::E.powf(-squared / 16.0);
                kernel[z * KERNEL_SIZE * KERNEL_SIZE + x * KERNEL_SIZE + y] = value as f32;
            }
        }
    }
    kernel
});

/// The kernel, for a test to hold to the game's.
#[must_use]
pub fn kernel() -> &'static [f32] {
    &KERNEL
}

/// The game's `Mth.fastInvSqrt` of a double: one step of Newton's method from a
/// guess made of the number's bits.
fn fast_inv_sqrt(x: f64) -> f64 {
    let half = 0.5 * x;
    let bits = 6_910_469_410_427_058_090_i64 - (x.to_bits() as i64 >> 1);
    let guess = f64::from_bits(bits as u64);
    guess * (1.5 - half * guess * guess)
}

/// The game's `getBeardContribution`: negative above the ground and positive below
/// it, weaker with the distance by the kernel.
fn beard(dx: i32, dy: i32, dz: i32, y_to_ground: i32) -> f32 {
    let (x, y, z) = (dx + KERNEL_RADIUS, dy + KERNEL_RADIUS, dz + KERNEL_RADIUS);
    let in_range = |index: i32| (0..KERNEL_SIZE as i32).contains(&index);
    if !in_range(x) || !in_range(y) || !in_range(z) {
        return 0.0;
    }
    let to_ground = y_to_ground as f32 + 0.5;
    let squared = dx as f32 * dx as f32 + to_ground * to_ground + dz as f32 * dz as f32;
    let value = -to_ground * fast_inv_sqrt(f64::from(squared / 2.0)) as f32 / 2.0;
    value * KERNEL[z as usize * KERNEL_SIZE * KERNEL_SIZE + x as usize * KERNEL_SIZE + y as usize]
}

/// The game's `getBuryContribution`: one at the piece, falling to nothing six
/// blocks away.
fn bury(dx: f32, dy: f32, dz: f32) -> f32 {
    let squared = dx * dx + dy * dy + dz * dz;
    // Written as the game writes it, so that a number that is no number gives zero.
    #[expect(clippy::neg_cmp_op_on_partial_ord, reason = "the game's comparison")]
    if !(squared < 36.0) {
        return 0.0;
    }
    1.0 - (f64::from(squared).sqrt() as f32) / 6.0
}

/// What the fill of one chunk adds for the structures near it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Beardifier {
    rigids: Vec<Rigid>,
    junctions: Vec<Junction>,
    /// The box around every piece and junction, grown by 24 blocks; nothing is added
    /// outside it. None where there is no piece and no junction.
    affected: Option<([i32; 3], [i32; 3])>,
}

impl Beardifier {
    /// The beardifier of pieces and junctions, in the order their sums are taken.
    #[must_use]
    pub fn new(rigids: Vec<Rigid>, junctions: Vec<Junction>) -> Self {
        let mut bounds: Option<([i32; 3], [i32; 3])> = None;
        let mut include = |min: [i32; 3], max: [i32; 3]| {
            bounds = Some(match bounds {
                None => (min, max),
                Some((low, high)) => (
                    [low[0].min(min[0]), low[1].min(min[1]), low[2].min(min[2])],
                    [
                        high[0].max(max[0]),
                        high[1].max(max[1]),
                        high[2].max(max[2]),
                    ],
                ),
            });
        };
        for rigid in &rigids {
            include(rigid.min, rigid.max);
        }
        for junction in &junctions {
            let at = [junction.x, junction.ground_y, junction.z];
            include(at, at);
        }
        let grown = KERNEL_SIZE as i32;
        let affected = bounds.map(|(min, max)| {
            (
                [min[0] - grown, min[1] - grown, min[2] - grown],
                [max[0] + grown, max[1] + grown, max[2] + grown],
            )
        });
        Self {
            rigids,
            junctions,
            affected,
        }
    }

    /// Whether nothing is added anywhere.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.affected.is_none()
    }

    #[must_use]
    pub fn rigids(&self) -> &[Rigid] {
        &self.rigids
    }

    #[must_use]
    pub fn junctions(&self) -> &[Junction] {
        &self.junctions
    }

    /// The box nothing is added outside of, both ends included.
    #[must_use]
    pub fn affected(&self) -> Option<([i32; 3], [i32; 3])> {
        self.affected
    }

    /// What is added to the final density of a block (`Beardifier.sampleValue`).
    #[must_use]
    pub fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        let Some((min, max)) = self.affected else {
            return 0.0;
        };
        if x < min[0] || x > max[0] || y < min[1] || y > max[1] || z < min[2] || z > max[2] {
            return 0.0;
        }
        let mut value = 0.0_f32;
        for rigid in &self.rigids {
            let dx = 0.max((rigid.min[0] - x).max(x - rigid.max[0]));
            let dz = 0.max((rigid.min[2] - z).max(z - rigid.max[2]));
            let ground_y = rigid.min[1] + rigid.ground_level_delta;
            let to_ground = y - ground_y;
            value += match rigid.adjustment {
                Adjustment::Bury => bury(dx as f32, to_ground as f32 / 2.0, dz as f32),
                Adjustment::BeardThin => beard(dx, to_ground, dz, to_ground) * 0.8,
                Adjustment::BeardBox => {
                    let dy = 0.max((ground_y - y).max(y - rigid.max[1]));
                    beard(dx, dy, dz, to_ground) * 0.8
                }
                Adjustment::Encapsulate => {
                    let dy = 0.max((rigid.min[1] - y).max(y - rigid.max[1]));
                    bury(dx as f32 / 2.0, dy as f32 / 2.0, dz as f32 / 2.0) * 0.8
                }
            };
        }
        for junction in &self.junctions {
            let (dx, dy, dz) = (x - junction.x, y - junction.ground_y, z - junction.z);
            value += beard(dx, dy, dz, dy) * 0.4;
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(adjustment: Adjustment) -> Rigid {
        Rigid {
            min: [0, 64, 0],
            max: [8, 70, 8],
            adjustment,
            ground_level_delta: 1,
        }
    }

    #[test]
    fn nothing_is_added_without_pieces_or_far_from_them() {
        assert!(Beardifier::default().is_empty());
        assert_eq!(Beardifier::default().sample(0, 64, 0).to_bits(), 0);
        let beard = Beardifier::new(vec![piece(Adjustment::BeardThin)], Vec::new());
        assert!(!beard.is_empty());
        assert_eq!(beard.affected(), Some(([-24, 40, -24], [32, 94, 32])));
        assert_eq!(beard.sample(-25, 64, 0), 0.0);
        assert_eq!(beard.sample(4, 95, 4), 0.0);
    }

    #[test]
    fn a_beard_makes_ground_below_the_floor_and_room_above_it() {
        let beard = Beardifier::new(vec![piece(Adjustment::BeardThin)], Vec::new());
        // The floor is one above the box's lowest block.
        assert!(beard.sample(4, 60, 4) > 0.0);
        assert!(beard.sample(4, 64, 4) > 0.0);
        assert!(beard.sample(4, 66, 4) < 0.0);
        assert!(beard.sample(4, 70, 4) < 0.0);
        // Weaker beside the piece than under it.
        assert!(beard.sample(14, 60, 4) < beard.sample(4, 60, 4));
        // Beyond the kernel's twelve blocks nothing.
        assert_eq!(beard.sample(21, 60, 4), 0.0);
    }

    #[test]
    fn a_buried_piece_has_ground_all_around_it() {
        let beard = Beardifier::new(vec![piece(Adjustment::Bury)], Vec::new());
        assert_eq!(beard.sample(4, 65, 4), 1.0);
        assert!(beard.sample(4, 70, 4) > 0.0);
        assert_eq!(beard.sample(15, 65, 4), 0.0);
        let shell = Beardifier::new(vec![piece(Adjustment::Encapsulate)], Vec::new());
        assert_eq!(shell.sample(4, 66, 4), 0.8);
        assert!(shell.sample(12, 66, 4) > 0.0 && shell.sample(12, 66, 4) < 0.8);
    }

    #[test]
    fn a_junction_adds_half_a_beard_at_its_point() {
        let junction = Junction {
            x: 100,
            ground_y: 70,
            z: -3,
        };
        let beard = Beardifier::new(Vec::new(), vec![junction]);
        assert!(beard.sample(100, 69, -3) > 0.0);
        assert!(beard.sample(100, 71, -3) < 0.0);
        assert_eq!(beard.sample(113, 69, -3), 0.0);
    }

    #[test]
    fn the_kernel_is_one_number_for_each_offset_and_strongest_in_the_middle() {
        let kernel = kernel();
        assert_eq!(kernel.len(), 13_824);
        let middle = 12 * 24 * 24 + 12 * 24 + 12;
        assert_eq!(
            kernel[middle],
            std::f64::consts::E.powf(-0.25 / 16.0) as f32
        );
        assert!(
            kernel
                .iter()
                .all(|value| *value > 0.0 && *value <= kernel[middle])
        );
        // One step of Newton's method is near and not exact.
        assert!((fast_inv_sqrt(4.0) - 0.5).abs() < 0.002);
        assert_ne!(fast_inv_sqrt(4.0), 0.5);
    }
}
