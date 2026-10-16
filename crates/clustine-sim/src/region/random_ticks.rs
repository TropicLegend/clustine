//! Random ticks (`docs/adr/0024-block-behaviour-in-a-region.md`, step B7): in each
//! chunk the region ticks, each section that holds a block or fluid that ticks randomly
//! has three of its positions drawn every tick (the game rule `randomTickSpeed`, 3 by
//! default), and what stands there takes its random tick, as the game's
//! `ServerLevel.tickChunk` has it.
//!
//! The game draws the positions, and what a random tick decides, from the level's one
//! random. Clustine draws them from a stream of its own for each section and tick,
//! seeded by the world, the chunk, the section and the region's tick, so that a tick
//! stays a function of the region and its inputs; the stream then goes on to the
//! rules of the blocks that tick, in the order they do.
//!
//! What needs light (crops, saplings, grass, mushrooms, bamboo, ice and snow melting)
//! does nothing yet: the simulation has no light (section 8 of the record).

mod fire;
mod minerals;
mod plants;

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, ChunkPos};

use super::Region;
use crate::api::TickOutput;

/// The game rule `randomTickSpeed`: how many positions of a section are drawn a tick.
const RANDOM_TICK_SPEED: usize = 3;

/// A stream of numbers for what the game draws from its random: SplitMix64, which is
/// small, fast and good enough for chances that no player can count.
#[derive(Debug, Clone)]
pub(crate) struct Random(u64);

impl Random {
    /// The stream of a section of a chunk in a tick of a world.
    pub(crate) fn of_section(world: u64, chunk: ChunkPos, section: i32, tick: u64) -> Self {
        let mut random = Self(world);
        random.mix(chunk.x as u32 as u64);
        random.mix(chunk.z as u32 as u64);
        random.mix(section as u32 as u64);
        random.mix(tick);
        random
    }

    fn mix(&mut self, value: u64) {
        self.0 = (self.0 ^ value).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.next_u64();
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `nextFloat`: from 0 up to 1.
    #[allow(dead_code, reason = "the rules that draw a float use it")]
    pub(crate) fn next_float(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// `nextDouble`: from 0 up to 1.
    #[allow(dead_code, reason = "the rules that draw a double use it")]
    pub(crate) fn next_double(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `nextInt(bound)`: from 0 up to `bound`, which is at least 1, each as likely.
    pub(crate) fn next_int(&mut self, bound: u32) -> u32 {
        // Lemire's multiply and shift, with the rejection that makes it exact.
        let bound = u64::from(bound.max(1));
        loop {
            let product = (self.next_u64() >> 32) * bound;
            let low = product & 0xffff_ffff;
            if low >= (1 << 32) % bound {
                return (product >> 32) as u32;
            }
        }
    }
}

impl Region {
    /// The random ticks of this tick, in the chunks the region ticks, in ascending
    /// position, each chunk's sections from the bottom.
    pub(crate) fn run_random_ticks(&mut self, output: &mut TickOutput) {
        let ticking: Vec<ChunkPos> = self.ticking_chunks();
        for chunk in ticking {
            let Some((min_y, sections)) = self.chunks.get(&chunk).map(|loaded| {
                let ticking: Vec<bool> = loaded
                    .sections()
                    .iter()
                    .map(|section| section.may_hold(ticks_randomly))
                    .collect();
                (loaded.min_y(), ticking)
            }) else {
                continue;
            };
            for (index, ticks) in sections.into_iter().enumerate() {
                if !ticks {
                    continue;
                }
                let bottom = min_y + 16 * index as i32;
                let section = bottom.div_euclid(16);
                let mut random = Random::of_section(self.config.seed, chunk, section, self.tick);
                for _ in 0..RANDOM_TICK_SPEED {
                    // `Level.getBlockRandomPos`: x, y and z within the section.
                    let drawn = random.next_int(1 << 12);
                    let pos = BlockPos::new(
                        chunk.x * 16 + (drawn & 15) as i32,
                        bottom + ((drawn >> 8) & 15) as i32,
                        chunk.z * 16 + ((drawn >> 4) & 15) as i32,
                    );
                    let Some(state) = self.block(pos) else {
                        continue;
                    };
                    if state.props().is_randomly_ticking() {
                        self.random_tick(pos, state, &mut random, output);
                    }
                }
            }
        }
    }

    /// `BlockState.randomTick` and `FluidState.randomTick` of what stands at `pos`.
    /// Each rule draws what it needs from `random`. A block whose rule needs light, or
    /// is not ported, does nothing.
    fn random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let _ = self.plant_random_tick(pos, state, random, output)
            || self.mineral_random_tick(pos, state, random, output)
            || self.fire_random_tick(pos, state, random, output);
    }

    /// A stream for a rule that draws outside a random tick (a coral's time to die),
    /// of the world, the tick and how many such streams the tick has had.
    pub(crate) fn next_draw(&mut self) -> Random {
        self.draws += 1;
        Random::of_section(
            self.config.seed,
            ChunkPos::new(0, 0),
            self.draws as i32,
            self.tick,
        )
    }
}

/// Whether `state` may take a random tick.
fn ticks_randomly(state: BlockState) -> bool {
    state != blocks::AIR && state.props().is_randomly_ticking()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_s_stream_is_the_same_for_the_same_world_chunk_section_and_tick() {
        let draw = |world, x, section, tick| {
            let mut random = Random::of_section(world, ChunkPos::new(x, -3), section, tick);
            (0..4).map(|_| random.next_u64()).collect::<Vec<_>>()
        };
        assert_eq!(draw(7, 1, 4, 100), draw(7, 1, 4, 100));
        assert_ne!(draw(7, 1, 4, 100), draw(8, 1, 4, 100));
        assert_ne!(draw(7, 1, 4, 100), draw(7, 2, 4, 100));
        assert_ne!(draw(7, 1, 4, 100), draw(7, 1, -4, 100));
        assert_ne!(draw(7, 1, 4, 100), draw(7, 1, 4, 101));
    }

    #[test]
    fn next_int_stays_below_its_bound_and_reaches_every_value_about_as_often() {
        let mut random = Random::of_section(1, ChunkPos::new(0, 0), 0, 0);
        let mut seen = [0_u32; 7];
        for _ in 0..7_000 {
            let value = random.next_int(7);
            seen[value as usize] += 1;
        }
        assert!(
            seen.iter().all(|count| (800..1_200).contains(count)),
            "{seen:?}"
        );
    }
}
