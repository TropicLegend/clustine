//! Spawning's inputs from the specification, by someone who wrote none of its code
//! (`docs/groundwork/n9-plan.md`, sections 9.4 and 9.7, with D9 5.5 and finding 42):
//! the slime chunks against a Java random written here from the formula, far out where
//! the `int` products overflow; and the time since rest not counted while asleep.

use clustine_data::BlockState;

use super::*;
use crate::spawning::rules;

/// `java.util.Random`, written from its specification: the seed scrambled by
/// `0x5DEECE66D`, 48 bits of state, `next(bits)` its top bits, and `nextInt(bound)`
/// rejecting the top of the range for a bound that is not a power of two.
struct JavaRandom(i64);

impl JavaRandom {
    const MULTIPLIER: i64 = 0x5_DEEC_E66D;
    const MASK: i64 = (1 << 48) - 1;

    fn new(seed: i64) -> Self {
        Self((seed ^ Self::MULTIPLIER) & Self::MASK)
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB)) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & -bound == bound {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        let mut bits = self.next(31);
        let mut value = bits % bound;
        while bits.wrapping_sub(value).wrapping_add(bound - 1) < 0 {
            bits = self.next(31);
            value = bits % bound;
        }
        value
    }
}

/// D9 5.5: `seedSlimeChunk(x, z, seed, 987234911L)`: `seed + (long)(x·x·4987142) +
/// (long)(x·5947611) + (long)(z·z)·4392871L + (long)(z·389711) ^ salt`, each `int`
/// product overflowing as an `int`, `z·z` widened before its `long` product, the `^`
/// after the sum; a slime chunk where its `nextInt(10)` is 0.
fn slime_chunk(x: i32, z: i32, seed: i64) -> bool {
    let sum = seed
        .wrapping_add(i64::from(x.wrapping_mul(x).wrapping_mul(4_987_142)))
        .wrapping_add(i64::from(x.wrapping_mul(5_947_611)))
        .wrapping_add(i64::from(z.wrapping_mul(z)).wrapping_mul(4_392_871))
        .wrapping_add(i64::from(z.wrapping_mul(389_711)));
    JavaRandom::new(sum ^ 987_234_911).next_int(10) == 0
}

#[test]
fn the_slime_chunks_far_out_are_those_of_the_formula_with_its_overflows() {
    let mut found = 0;
    let mut asked = 0;
    for seed in [0_i64, 1, -1, 42, 0x1234_5678_9ABC_DEF0, i64::MIN, i64::MAX] {
        // Near, and about the world's edge, where `x·x·4987142` overflows many times.
        for (cx, cz) in [
            (0, 0),
            (1_000, -1_000),
            (-1_874_999, 1_874_999),
            (65_536, 46_341),
        ] {
            for dx in -6..=6 {
                for dz in -6..=6 {
                    let (x, z) = (cx + dx, cz + dz);
                    let expected = slime_chunk(x, z, seed);
                    assert_eq!(
                        rules::is_slime_chunk(x, z, seed),
                        expected,
                        "chunk ({x}, {z}) of the seed {seed}"
                    );
                    asked += 1;
                    if expected {
                        found += 1;
                    }
                }
            }
        }
    }
    // About one chunk in ten.
    assert!(
        found * 20 > asked && found * 5 < asked,
        "{found} of {asked}"
    );
}

/// The plan's 9.7 (finding 42; n7d ServerPlayer 1913, 2879): the time since rest is reset
/// when sleep starts and does not count while the player sleeps; it counts on from the
/// waking.
#[test]
fn the_time_since_rest_stays_at_nothing_while_the_player_sleeps_and_counts_from_the_waking() {
    let head = BlockPos::new(2, -60, 0);
    let foot = BlockPos::new(2, -60, 1);
    let bed = |part: &str| {
        BlockState::parse(&format!("minecraft:red_bed[facing=north,part={part}]")).expect("a bed")
    };
    let mut region = on_ticking_floor_with(&[(head, bed("head")), (foot, bed("foot"))]);
    region.clock.day = 13_000;
    for _ in 0..50 {
        region.tick(&TickInputs::default());
    }
    {
        let player = region.players.get_mut(&player(1)).expect("player 1");
        player.pose.position = Vec3::new(4.5, -60.0, 0.5);
        player.pose.yaw = 90.0;
    }
    let awake = region.players[&player(1)].time_since_rest(region.tick);
    assert!(awake >= 50, "awake for the ticks so far: {awake}");
    region.tick(&moves(vec![place(1, foot.x, foot.y, foot.z, Face::Top)]));
    assert!(region.players[&player(1)].sleeping.is_some(), "asleep");
    let mut slept = 0;
    while region.players[&player(1)].sleeping.is_some() {
        assert_eq!(
            region.players[&player(1)].time_since_rest(region.tick),
            0,
            "after {slept} ticks asleep"
        );
        region.tick(&TickInputs::default());
        slept += 1;
        assert!(slept < 200, "never woke");
    }
    let woke = region.players[&player(1)].time_since_rest(region.tick);
    assert!(woke <= 1, "counted from the waking: {woke}");
    for more in 1..=20 {
        region.tick(&TickInputs::default());
        assert_eq!(
            region.players[&player(1)].time_since_rest(region.tick),
            woke + more
        );
    }
}
