//! Light in the region and the rules that read it, tested from the specification by
//! someone who wrote none of it (part B of section 10 of `docs/groundwork/n8-plan.md`,
//! as section 14 amends it; ADR-0029), with the class dumps in
//! `/home/claude/reference/n8-javap/` and `n7-javap/` as the oracle where they and the
//! plan differ.
//!
//! What a rule draws is checked against a replay of the draws the game's rule makes, in
//! its order, from the same stream: the two streams must end equal, which a draw too
//! many, too few or of another kind breaks. Where the game decides by a draw, the
//! replay predicts the outcome as well, and the region must agree with it seed for seed.
//!
//! The scenes are the 3×3 of stone floors about the origin (the floor at y = −61, the
//! ground at y = −60), lit by the region's own light step. The chunks beyond are absent,
//! which the light takes as walls, so a roof over the 3×3 makes a room no sky reaches.

mod budget;
mod detector;
mod draws;
mod hand_over;
mod lines;
mod mobs;
mod plants;
mod thresholds;
mod trees;

use super::*;
use crate::region::Random;

pub(super) const NOON: u64 = 6_000;
pub(super) const MIDNIGHT: u64 = 18_000;
/// The ground: the first air above the floor.
pub(super) const GROUND: i32 = -60;
/// The roof of a dark room, over the whole 3×3 (or 5×5) of chunks.
pub(super) const ROOF: i32 = -57;

pub(super) fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

pub(super) fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// A region with the 3×3 of floors about the origin and nobody in it, `placed` put
/// there without any update, lit, at `day`.
pub(super) fn scene_at(day: u64, placed: &[(BlockPos, BlockState)]) -> Region {
    let mut region = region();
    for x in -1..=1 {
        for z in -1..=1 {
            region.chunks.insert(ChunkPos::new(x, z), floor());
        }
    }
    for (pos, placed) in placed {
        put(&mut region, *pos, *placed);
    }
    region.clock.day = day;
    region.light_step();
    region
}

/// [`scene_at`] at noon.
pub(super) fn open(placed: &[(BlockPos, BlockState)]) -> Region {
    scene_at(NOON, placed)
}

/// The stone roof at [`ROOF`] over the chunks from `-reach` to `reach` about the origin.
pub(super) fn roof(reach: i32) -> Vec<(BlockPos, BlockState)> {
    let span = -16 * reach..16 * (reach + 1);
    span.clone()
        .flat_map(|x| span.clone().map(move |z| (at(x, ROOF, z), blocks::STONE)))
        .collect()
}

/// [`open`] under a roof over the whole 3×3: no sky anywhere below it.
pub(super) fn roofed(placed: &[(BlockPos, BlockState)]) -> Region {
    roofed_at(NOON, placed)
}

pub(super) fn roofed_at(day: u64, placed: &[(BlockPos, BlockState)]) -> Region {
    let mut all = roof(1);
    all.extend_from_slice(placed);
    scene_at(day, &all)
}

/// Puts `state` at `pos` without any update, as a test sets its scene. The chunk notes
/// the change for the next light step.
pub(super) fn put(region: &mut Region, pos: BlockPos, state: BlockState) {
    let (x, z) = pos.in_chunk();
    region
        .chunks
        .get_mut(&pos.chunk())
        .expect("a loaded chunk")
        .set(x, pos.y, z, state);
}

pub(super) fn read(region: &Region, pos: BlockPos) -> BlockState {
    region.block(pos).expect("a loaded block")
}

/// The stream of `seed`, of these tests alone.
pub(super) fn stream(seed: u64) -> Random {
    Random::of_section(0x4e38_6762, ChunkPos::new(7, -3), 5, seed)
}

/// A replay of what a rule of the game draws, in its order, from the stream of a seed.
pub(super) struct Replay {
    seed: u64,
    random: Random,
}

impl Replay {
    pub(super) fn of(seed: u64) -> Self {
        Self {
            seed,
            random: stream(seed),
        }
    }

    /// `nextInt(bound)`.
    pub(super) fn int(&mut self, bound: u32) -> u32 {
        self.random.next_int(bound)
    }

    /// `nextFloat()`.
    pub(super) fn float(&mut self) -> f32 {
        self.random.next_float()
    }

    /// `nextLong()`, as the seed of a feature's own random.
    pub(super) fn long(&mut self) {
        self.random.next_u64();
    }

    /// Asserts that `drawn` is the stream after exactly the replayed draws.
    pub(super) fn matches(&self, drawn: &Random, what: &str) {
        let start = stream(self.seed);
        assert!(
            *drawn == self.random,
            "{what}, seed {}: the rule drew {} numbers where the game's draws {}",
            self.seed,
            drawn.draws_since(&start),
            self.random.draws_since(&start),
        );
    }
}

/// The random tick of what stands at `pos`, by the rules of light (growth or melting),
/// drawn from the stream of `seed`; the stream as the rule left it, and what it said.
pub(super) fn random_tick(region: &mut Region, pos: BlockPos, seed: u64) -> (Random, TickOutput) {
    let state = read(region, pos);
    let mut random = stream(seed);
    let mut output = TickOutput::default();
    let taken = region.growth_random_tick(pos, state, &mut random, &mut output)
        || region.melting_random_tick(pos, state, &mut random, &mut output);
    assert!(taken, "{state:?} at {pos:?} is a rule of light");
    (random, output)
}

/// [`random_tick`], the stream alone.
pub(super) fn tick_at(region: &mut Region, pos: BlockPos, seed: u64) -> Random {
    random_tick(region, pos, seed).0
}

/// Sets `state` at `pos` as a rule does, with the updates of its neighbours and shapes.
pub(super) fn set_updated(region: &mut Region, pos: BlockPos, state: BlockState) {
    let mut output = TickOutput::default();
    region.set_block(pos, state, &mut output);
}

/// The light of a block in a region as `(sky, block)`, `None` where it is not lit.
pub(super) fn levels(region: &Region, pos: BlockPos) -> Option<(u8, u8)> {
    region.light_at(pos).map(|light| (light.sky, light.block))
}
