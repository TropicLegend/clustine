//! The gate of this crate: every sample of SteelMC's fixture is reproduced bit for bit.
//!
//! `noise_samples.json` is copied unchanged from SteelMC's
//! `steel-worldgen/test_assets/noise_samples.json` (branch 26.3, commit
//! 885c4b3e60ed79862c37311780774f76806cb714). SteelMC's extractor wrote it from the
//! game itself: each entry names a sampler, a seed, a block position and the 32 bits
//! of the float the game computed there. This test is written from that format and
//! from SteelMC's own test of it, `steel-worldgen/tests/noise_samples.rs`, which says
//! how each sampler is made.

use clustine_noise::{BlendedNoise, LegacyRandom, NormalNoise, PerlinSimplexNoise};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Sampler {
    Normal,
    LegacyNether,
    Blended,
    FrozenTemperature,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoiseSample {
    sampler: Sampler,
    seed: i64,
    x: i32,
    y: i32,
    z: i32,
    value_bits: u32,
}

fn samples() -> Vec<NoiseSample> {
    serde_json::from_str(include_str!("noise_samples.json")).expect("the fixture is JSON")
}

/// What this crate computes for a sample, each sampler made as SteelMC's test makes it.
fn computed(sample: &NoiseSample) -> f32 {
    let x = f64::from(sample.x);
    let y = f64::from(sample.y);
    let z = f64::from(sample.z);
    let mut random = LegacyRandom::from_seed(sample.seed as u64);
    match sample.sampler {
        Sampler::Normal => {
            NormalNoise::create_from_random(&mut random, -7, &[1.0, 1.0]).get(x, y, z)
        }
        Sampler::LegacyNether => {
            NormalNoise::create_legacy_nether_biome(&mut random, -7, &[1.0, 1.0]).get(x, y, z)
        }
        Sampler::Blended => {
            BlendedNoise::new(&mut random, 0.25, 0.125, 80.0, 160.0, 8.0).get(x, y, z)
        }
        // The game seeds this noise of biomes with 3456 whatever the world's seed is.
        Sampler::FrozenTemperature => {
            let mut random = LegacyRandom::from_seed(3456);
            PerlinSimplexNoise::new(&mut random, &[-2, -1, 0]).get(x * 0.05, z * 0.05)
        }
    }
}

#[test]
fn every_sample_of_the_fixture_is_reproduced_bit_for_bit() {
    let samples = samples();
    let mut differing = Vec::new();
    for sample in &samples {
        let actual = computed(sample);
        if actual.to_bits() != sample.value_bits {
            differing.push(format!(
                "{:?}, seed {}, ({}, {}, {}): {:#010x} ({actual:e}) where the game has {:#010x} ({:e})",
                sample.sampler,
                sample.seed,
                sample.x,
                sample.y,
                sample.z,
                actual.to_bits(),
                sample.value_bits,
                f32::from_bits(sample.value_bits),
            ));
        }
    }
    assert!(
        differing.is_empty(),
        "{} of {} samples differ:\n{}",
        differing.len(),
        samples.len(),
        differing.join("\n")
    );
}

#[test]
fn the_fixture_is_the_one_that_was_copied() {
    // A fixture that lost its samples would let the gate pass with nothing in it.
    let samples = samples();
    let count = |sampler| samples.iter().filter(|s| s.sampler == sampler).count();
    assert_eq!(samples.len(), 90);
    assert_eq!(count(Sampler::Normal), 27);
    assert_eq!(count(Sampler::LegacyNether), 27);
    assert_eq!(count(Sampler::Blended), 27);
    assert_eq!(count(Sampler::FrozenTemperature), 9);

    let mut seeds: Vec<i64> = samples.iter().map(|sample| sample.seed).collect();
    seeds.sort_unstable();
    seeds.dedup();
    assert_eq!(seeds, [-1, 0, 13_579]);
    assert!(
        samples.iter().any(|sample| sample.x.abs() > (1 << 24)),
        "some samples lie beyond where coordinates are wrapped"
    );
}
