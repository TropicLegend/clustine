//! The same question gets the same bits: asked twice, asked of a second noise made
//! the same way, and asked through each of the paths that are meant to be one
//! computation (a plane through zero and the full noise there, a column and its
//! blocks, either family of generator inside a `RandomSource`).
//!
//! Every Clustine process that makes a chunk has to arrive at the same chunk, so
//! nothing here may depend on what was computed before or on the path taken.

use clustine_noise::{
    BlendedNoise, ImprovedNoise, LegacyRandom, NameHash, NormalNoise, PerlinNoise,
    PerlinSimplexNoise, PositionalRandom, Random, RandomSource, SimplexNoise, Xoroshiro, spline,
    trig,
};

/// Positions near the origin, on and between lattice points, on both sides of zero,
/// and beyond where the noises wrap their coordinates.
const POSITIONS: [(f64, f64, f64); 9] = [
    (0.0, 0.0, 0.0),
    (-0.0, 64.0, 0.5),
    (1.25, -30.75, 8.0),
    (-1000.0, 319.0, 4096.5),
    (255.999_999, -64.0, -256.000_001),
    (13_579.0, 72.0, -24_680.0),
    (20_000_068.0, 296.0, -19_999_796.0),
    (33_554_431.5, 0.25, -33_554_432.25),
    (-418_462.0, 110.0, 366_791.0),
];

fn overworld_blended(seed: u64) -> BlendedNoise {
    BlendedNoise::new(
        &mut LegacyRandom::from_seed(seed),
        0.25,
        0.125,
        80.0,
        160.0,
        8.0,
    )
}

#[test]
fn every_noise_gives_the_same_bits_when_asked_twice_and_when_made_again() {
    for seed in [0_u64, 13_579, u64::MAX] {
        let make_improved = || ImprovedNoise::new(&mut Xoroshiro::from_seed(seed));
        let make_normal = || {
            let splitter = Xoroshiro::from_seed(seed).next_positional();
            NormalNoise::create(&splitter, "minecraft:continentalness", -9, &[1.0, 1.0, 2.0])
        };
        let make_perlin = || {
            let splitter = Xoroshiro::from_seed(seed).next_positional();
            PerlinNoise::create(&splitter, -6, &[1.0, 0.0, 1.0, 0.5])
        };
        let make_simplex = || SimplexNoise::new(&mut LegacyRandom::from_seed(seed));
        let make_octaves =
            || PerlinSimplexNoise::new(&mut LegacyRandom::from_seed(seed), &[-2, -1, 0, 1]);

        let (improved, improved_again) = (make_improved(), make_improved());
        let (normal, normal_again) = (make_normal(), make_normal());
        let (perlin, perlin_again) = (make_perlin(), make_perlin());
        let (simplex, simplex_again) = (make_simplex(), make_simplex());
        let (octaves, octaves_again) = (make_octaves(), make_octaves());
        let (blended, blended_again) = (overworld_blended(seed), overworld_blended(seed));

        for (x, y, z) in POSITIONS {
            let at = format!("seed {seed}, ({x}, {y}, {z})");

            let value = improved.noise(x, y, z).to_bits();
            assert_eq!(value, improved.noise(x, y, z).to_bits(), "{at}");
            assert_eq!(value, improved_again.noise(x, y, z).to_bits(), "{at}");

            let value = improved.smeared_noise(x, y, z, 5475.296).to_bits();
            assert_eq!(
                value,
                improved.smeared_noise(x, y, z, 5475.296).to_bits(),
                "{at}"
            );
            assert_eq!(
                value,
                improved_again.smeared_noise(x, y, z, 5475.296).to_bits(),
                "{at}"
            );

            let value = normal.get(x, y, z).to_bits();
            assert_eq!(value, normal.get(x, y, z).to_bits(), "{at}");
            assert_eq!(value, normal_again.get(x, y, z).to_bits(), "{at}");

            let value = perlin.get(x, y, z).to_bits();
            assert_eq!(value, perlin.get(x, y, z).to_bits(), "{at}");
            assert_eq!(value, perlin_again.get(x, y, z).to_bits(), "{at}");

            let value = blended.get(x, y, z).to_bits();
            assert_eq!(value, blended.get(x, y, z).to_bits(), "{at}");
            assert_eq!(value, blended_again.get(x, y, z).to_bits(), "{at}");

            let value = simplex.get_2d(x, z).to_bits();
            assert_eq!(value, simplex.get_2d(x, z).to_bits(), "{at}");
            assert_eq!(value, simplex_again.get_2d(x, z).to_bits(), "{at}");

            let value = simplex.get_3d(x, y, z).to_bits();
            assert_eq!(value, simplex.get_3d(x, y, z).to_bits(), "{at}");
            assert_eq!(value, simplex_again.get_3d(x, y, z).to_bits(), "{at}");

            let value = octaves.get(x, z).to_bits();
            assert_eq!(value, octaves.get(x, z).to_bits(), "{at}");
            assert_eq!(value, octaves_again.get(x, z).to_bits(), "{at}");
        }
    }
}

#[test]
fn asking_in_another_order_changes_nothing() {
    let noise = overworld_blended(13_579);
    let forwards: Vec<u32> = POSITIONS
        .iter()
        .map(|&(x, y, z)| noise.get(x, y, z).to_bits())
        .collect();
    let mut backwards: Vec<u32> = POSITIONS
        .iter()
        .rev()
        .map(|&(x, y, z)| noise.get(x, y, z).to_bits())
        .collect();
    backwards.reverse();
    assert_eq!(forwards, backwards);
}

#[test]
fn the_planes_through_zero_give_the_bits_of_the_full_noise() {
    for seed in [0_u64, 13_579, u64::MAX] {
        let improved = ImprovedNoise::new(&mut Xoroshiro::from_seed(seed));
        let splitter = Xoroshiro::from_seed(seed).next_positional();
        let normal = NormalNoise::create(&splitter, "minecraft:erosion", -9, &[1.0, 1.0, 0.0, 1.0]);
        let perlin = PerlinNoise::create(&splitter, -6, &[1.0, 0.0, 1.0, 0.5]);

        for (x, y, z) in POSITIONS {
            let at = format!("seed {seed}, ({x}, {y}, {z})");
            assert_eq!(
                improved.noise_xz(x, z).to_bits(),
                improved.noise(x, 0.0, z).to_bits(),
                "{at}"
            );
            assert_eq!(
                improved.noise_xy(x, y).to_bits(),
                improved.noise(x, y, 0.0).to_bits(),
                "{at}"
            );
            assert_eq!(
                normal.get_xz(x, z).to_bits(),
                normal.get(x, 0.0, z).to_bits(),
                "{at}"
            );
            assert_eq!(
                normal.get_xy(x, y).to_bits(),
                normal.get(x, y, 0.0).to_bits(),
                "{at}"
            );
            assert_eq!(
                perlin.get_xz(x, z).to_bits(),
                perlin.get(x, 0.0, z).to_bits(),
                "{at}"
            );
            assert_eq!(
                perlin.get_xy(x, y).to_bits(),
                perlin.get(x, y, 0.0).to_bits(),
                "{at}"
            );
        }
    }
}

#[test]
fn the_value_with_slopes_gives_the_bits_of_the_noise() {
    let improved = ImprovedNoise::new(&mut Xoroshiro::from_seed(13_579));
    for (x, y, z) in POSITIONS {
        let mut slopes = [0.0_f32; 3];
        assert_eq!(
            improved
                .noise_with_derivative(x, y, z, &mut slopes)
                .to_bits(),
            improved.noise(x, y, z).to_bits(),
            "({x}, {y}, {z})"
        );
    }
}

#[test]
fn a_column_of_the_terrain_noise_gives_the_bits_of_its_blocks() {
    let heights: Vec<i32> = (-64..320).step_by(8).collect();
    for seed in [0_u64, 13_579, u64::MAX] {
        let noise = overworld_blended(seed);
        for (x, z) in [(0, 0), (-418_462, 366_791), (20_000_068, -19_999_796)] {
            let mut column = vec![f32::NAN; heights.len()];
            noise.get_column(x, &heights, z, &mut column);
            for (&y, value) in heights.iter().zip(&column) {
                let single = noise.get(f64::from(x), f64::from(y), f64::from(z));
                assert_eq!(
                    value.to_bits(),
                    single.to_bits(),
                    "seed {seed}, ({x}, {y}, {z})"
                );
            }
        }
    }
}

#[test]
fn a_noise_is_the_same_from_a_generator_and_from_a_source_that_holds_it() {
    for seed in [0_u64, 13_579, u64::MAX] {
        let from_legacy =
            NormalNoise::create_from_random(&mut LegacyRandom::from_seed(seed), -7, &[1.0, 1.0]);
        let from_source = NormalNoise::create_from_random(
            &mut RandomSource::Legacy(LegacyRandom::from_seed(seed)),
            -7,
            &[1.0, 1.0],
        );
        let from_xoroshiro =
            NormalNoise::create_from_random(&mut Xoroshiro::from_seed(seed), -7, &[1.0, 1.0]);
        let from_xoroshiro_source = NormalNoise::create_from_random(
            &mut RandomSource::Xoroshiro(Xoroshiro::from_seed(seed)),
            -7,
            &[1.0, 1.0],
        );
        for (x, y, z) in POSITIONS {
            assert_eq!(
                from_legacy.get(x, y, z).to_bits(),
                from_source.get(x, y, z).to_bits()
            );
            assert_eq!(
                from_xoroshiro.get(x, y, z).to_bits(),
                from_xoroshiro_source.get(x, y, z).to_bits()
            );
        }
    }
}

#[test]
fn generators_and_factories_give_the_same_numbers_when_made_again() {
    let name = NameHash::new("minecraft:aquifer");
    for seed in [0_u64, 13_579, u64::MAX] {
        let draw = |mut source: RandomSource| {
            let splitter = source.next_positional();
            let mut by_name = splitter.with_hash_of(&name);
            let mut by_position = splitter.at(-418_462, 110, 366_791);
            (
                source.next_i64(),
                source.next_f64().to_bits(),
                source.next_gaussian().to_bits(),
                by_name.next_i32_bounded(1000),
                by_name.next_f32().to_bits(),
                by_position.next_i64(),
            )
        };
        let xoroshiro = || RandomSource::Xoroshiro(Xoroshiro::from_seed(seed));
        let legacy = || RandomSource::Legacy(LegacyRandom::from_seed(seed));
        assert_eq!(draw(xoroshiro()), draw(xoroshiro()));
        assert_eq!(draw(legacy()), draw(legacy()));
        assert_ne!(draw(xoroshiro()), draw(legacy()));
    }
}

#[test]
fn the_table_of_sines_and_the_splines_give_the_same_bits_twice() {
    for step in -2000..2000 {
        let angle = f64::from(step) * 0.0123;
        assert_eq!(trig::sin(angle).to_bits(), trig::sin(angle).to_bits());
        assert_eq!(trig::cos(angle).to_bits(), trig::cos(angle).to_bits());
    }
    // The cosine is the sine a quarter of the table on, by the game's definition.
    assert_eq!(
        trig::cos(0.0).to_bits(),
        trig::sin_table_entry(16_384).to_bits()
    );

    let locations = [-1.0_f32, -0.4, 0.0, 0.3, 1.0];
    let derivatives = [0.0_f32, 0.5, -0.25, 1.5, 0.0];
    let values = [0.1_f32, -0.3, 0.7, 0.2, 0.9];
    for step in -150..150 {
        let input = step as f32 * 0.01;
        let first = spline::evaluate_spline(&locations, &derivatives, input, |i| values[i]);
        let second = spline::evaluate_spline(&locations, &derivatives, input, |i| values[i]);
        assert_eq!(first.to_bits(), second.to_bits());
    }
}
