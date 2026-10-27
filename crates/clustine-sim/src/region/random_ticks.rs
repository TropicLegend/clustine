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
//! What needs light (crops, grass, mushrooms, bamboo, ice and snow melting) reads the
//! region's (`growth.rs`, `melting.rs`; `docs/adr/0029-light-in-the-simulation.md`);
//! saplings grow with N8f.

mod fire;
pub(super) mod growth;
mod melting;
mod minerals;
mod plants;

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, ChunkPos, EntityId};

use super::Region;
use crate::api::TickOutput;

/// The game rule `randomTickSpeed`: how many positions of a section are drawn a tick.
const RANDOM_TICK_SPEED: usize = 3;

/// The step SplitMix64 takes at each draw, odd.
const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;

/// The inverse of [`GOLDEN`] modulo 2^64, by Newton's iteration: an odd number is its
/// own inverse modulo 8, and each step doubles the bits that are right.
const GOLDEN_INVERSE: u64 = {
    let mut inverse = GOLDEN;
    let mut step = 0;
    while step < 6 {
        inverse = inverse.wrapping_mul(2_u64.wrapping_sub(GOLDEN.wrapping_mul(inverse)));
        step += 1;
    }
    inverse
};

/// A stream of numbers for what the game draws from its random: SplitMix64, which is
/// small, fast and good enough for chances that no player can count.
#[derive(Debug, Clone, PartialEq)]
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

    /// The stream of the entity `key` in its own tick, which began at `lived`
    /// (`docs/adr/0028-entities.md`, section 4, as `docs/groundwork/n7-plan.md`, section
    /// 11.2, amends it): what it draws is a function of the world, the key, the ticks it
    /// has lived and the draw's place in its tick, with nothing to keep between ticks.
    /// `lived` rises at each of its ticks wherever it is held, where a region's tick
    /// does not (a merge continues the survivor's), so no entity draws the same numbers
    /// twice. Apart from the streams of sections by a mark of its own.
    pub(crate) fn of_entity(world: u64, key: EntityId, lived: u32) -> Self {
        /// Mixed in first, so that no entity's stream is a section's.
        const ENTITIES: u64 = 0x656e_7469_7469_6573;
        let mut random = Self(world);
        random.mix(ENTITIES);
        random.mix(key.0);
        random.mix(u64::from(lived));
        random
    }

    /// The stream of what a generated entity `key` draws at its making on the pool (its
    /// UUID, its constructor's yaw, `construct`'s timers), by a mark of its own, which no
    /// region counts: the chunk's generation is a function, and its first tick at `lived`
    /// 0 then draws its own stream afresh (`docs/groundwork/n7l-plan.md`, sections 2.2
    /// and 9 item 10).
    pub(crate) fn of_generated(world: u64, key: EntityId) -> Self {
        /// Mixed in first, so that this is no other stream.
        const GENERATED: u64 = 0x6765_6e65_7261_7465;
        let mut random = Self(world);
        random.mix(GENERATED);
        random.mix(key.0);
        random
    }

    /// The stream of what is drawn for the entity `key` outside its own tick: its
    /// birth, a hit, a throw by a player (who is no entity of a chunk). Keyed by its
    /// `lived` and the region's tick, by a mark of its own, so that it never meets the
    /// entity's own stream: an entity born in a tick draws its first tick's numbers
    /// afresh.
    pub(crate) fn of_entity_aside(world: u64, key: EntityId, lived: u32, tick: u64) -> Self {
        /// Mixed in first, so that this is neither an entity's own stream nor a
        /// section's.
        const ASIDE: u64 = 0x61_7369_6465;
        let mut random = Self(world);
        random.mix(ASIDE);
        random.mix(key.0);
        random.mix(u64::from(lived));
        random.mix(tick);
        random
    }

    /// The stream of a hit (`docs/groundwork/n9-plan.md`, section 8.2): what the extras
    /// of the hit of `attacker` at its `lived` on `target` draw where it lands, the same
    /// in the attacker's region and across a line, by a mark of its own; keyed by the
    /// target too, so that a piercing arrow's hits draw apart, and not by a region's tick,
    /// since the lander's region may not be the attacker's.
    pub(crate) fn of_hit(world: u64, attacker: EntityId, lived: u32, target: EntityId) -> Self {
        /// Mixed in first, so that this is no other stream.
        const HIT: u64 = 0x68_6974;
        let mut random = Self(world);
        random.mix(HIT);
        random.mix(attacker.0);
        random.mix(u64::from(lived));
        random.mix(target.0);
        random
    }

    /// The stream of the block at `pos` in a tick of a world for the rule `purpose`
    /// (`docs/groundwork/n4-plan.md`, section 7, "Drops"): what a block broken by a
    /// rule drops, and where. Apart from the streams of sections and entities by a
    /// mark of its own.
    pub(crate) fn of_block(world: u64, pos: BlockPos, tick: u64, purpose: u64) -> Self {
        /// Mixed in first, so that no block's stream is a section's or an entity's.
        const BLOCKS: u64 = 0x626c_6f63_6b73;
        let mut random = Self(world);
        random.mix(BLOCKS);
        random.mix(purpose);
        random.mix(pos.x as u32 as u64);
        random.mix(pos.y as u32 as u64);
        random.mix(pos.z as u32 as u64);
        random.mix(tick);
        random
    }

    /// How many numbers the stream has drawn since it was `start`: each draw steps
    /// SplitMix64's state on by the same odd number, which has an inverse modulo 2^64,
    /// so the count is the state's difference times that inverse.
    pub(crate) fn draws_since(&self, start: &Random) -> u64 {
        self.0.wrapping_sub(start.0).wrapping_mul(GOLDEN_INVERSE)
    }

    /// Takes `value` into the seed. The state is put through SplitMix64's output mix,
    /// which spreads every bit over all of them: without it, a seed that ends with a
    /// tick or a `lived` and the next one's lay exactly one draw apart on every other
    /// tick (`(S ^ t) * G` and `(S ^ (t + 1)) * G` differ by `G`, the step of a draw), so
    /// that a section's random ticks hit the same blocks tick after tick and an entity
    /// drew yesterday's numbers again (found by the water animals' tests from the
    /// specification: a turtle's eggs hatched in three ticks).
    fn mix(&mut self, value: u64) {
        let mut z = (self.0 ^ value).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        self.0 = z ^ (z >> 31);
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `nextFloat`: from 0 up to 1.
    pub(crate) fn next_float(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// `nextDouble`: from 0 up to 1.
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

    /// `nextBoolean`.
    #[allow(dead_code, reason = "for the goals and the body of mobs, N7b and N7c")]
    pub(crate) fn next_bool(&mut self) -> bool {
        self.next_u64() >> 63 != 0
    }

    /// `nextIntBetweenInclusive(low, high)`: `nextInt(high - low + 1) + low`.
    #[allow(dead_code, reason = "for the goals and the body of mobs, N7b and N7c")]
    pub(crate) fn next_int_between(&mut self, low: i32, high: i32) -> i32 {
        let span = u32::try_from(i64::from(high) - i64::from(low) + 1).unwrap_or(1);
        low.wrapping_add(self.next_int(span) as i32)
    }

    /// `nextGaussian`: normal, of mean 0 and deviation 1, by the polar method, which
    /// draws two doubles at a time until a pair falls in the circle and keeps neither
    /// for later, so that what a draw takes is the game's in count only in distribution.
    #[allow(dead_code, reason = "for the goals and the body of mobs, N7b and N7c")]
    pub(crate) fn next_gaussian(&mut self) -> f64 {
        loop {
            let x = 2.0 * self.next_double() - 1.0;
            let y = 2.0 * self.next_double() - 1.0;
            let s = x * x + y * y;
            if s < 1.0 && s != 0.0 {
                return x * (-2.0 * libm::log(s) / s).sqrt();
            }
        }
    }
}

impl crate::physics::item::Draws for Random {
    fn next_float(&mut self) -> f32 {
        Random::next_float(self)
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
    /// Each rule draws what it needs from `random`. A block whose rule is not ported
    /// does nothing.
    fn random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let _ = self.plant_random_tick(pos, state, random, output)
            || self.mineral_random_tick(pos, state, random, output)
            || self.fire_random_tick(pos, state, random, output)
            || self.growth_random_tick(pos, state, random, output)
            || self.melting_random_tick(pos, state, random, output);
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

    #[test]
    fn an_int_between_two_bounds_lies_within_them_both_and_reaches_each() {
        let mut random = Random::of_entity(1, EntityId(2), 3);
        let drawn: Vec<i32> = (0..200).map(|_| random.next_int_between(-2, 2)).collect();
        assert!(drawn.iter().all(|value| (-2..=2).contains(value)));
        assert!(drawn.contains(&-2) && drawn.contains(&2));
        assert_eq!(random.next_int_between(5, 5), 5);
        let heads = (0..1000).filter(|_| random.next_bool()).count();
        assert!((400..600).contains(&heads), "{heads}");
        let mean = (0..2000).map(|_| random.next_gaussian()).sum::<f64>() / 2000.0;
        assert!(mean.abs() < 0.1, "{mean}");
    }
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
    fn a_stream_counts_the_numbers_it_has_drawn_since_any_point() {
        assert_eq!(GOLDEN.wrapping_mul(GOLDEN_INVERSE), 1);
        let mut random = Random::of_entity(3, EntityId(9), 40);
        random.next_u64();
        let start = random.clone();
        for _ in 0..37 {
            random.next_int(7);
        }
        let used = random.draws_since(&start);
        assert!(used >= 37, "{used}");
        let mut again = start.clone();
        for _ in 0..used {
            again.next_u64();
        }
        assert_eq!(again, random);
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

#[cfg(test)]
mod streams {
    use super::*;

    /// The first draws of a stream.
    fn draws(mut random: Random) -> Vec<u64> {
        (0..16).map(|_| random.next_u64()).collect()
    }

    /// No stream of a tick, or of an entity's `lived`, is the one before shifted by any
    /// number of draws: the two share none of their first sixteen.
    #[test]
    fn the_streams_of_consecutive_ticks_and_lives_share_no_draws() {
        for at in 0..200_u64 {
            let section = |tick| Random::of_section(7, ChunkPos::new(3, -2), 4, tick);
            let this = draws(section(at));
            let next = draws(section(at + 1));
            assert!(this.iter().all(|draw| !next.contains(draw)), "tick {at}");
            let lived = u32::try_from(at).expect("small");
            let entity = |lived| Random::of_entity(7, EntityId(40), lived);
            let this = draws(entity(lived));
            let next = draws(entity(lived + 1));
            assert!(this.iter().all(|draw| !next.contains(draw)), "lived {at}");
        }
    }
}
