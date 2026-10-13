//! What the biomes of a chunk cost: a measurement that reports and asserts nothing
//! (`docs/groundwork/terrain-plan.md`, section 6).
//!
//! ```text
//! cargo test --release -p clustine-terrain --test cost -- --ignored --nocapture
//! ```

use clustine_terrain::biome::{BiomeSource, Dimension};

#[test]
#[ignore = "a measurement, for a release build"]
#[allow(
    clippy::disallowed_methods,
    reason = "the test, not the crate, takes the time"
)]
fn the_time_a_chunks_biomes_take() {
    for dimension in Dimension::ALL {
        let started = std::time::Instant::now();
        let source = BiomeSource::new(13579, dimension);
        let made = started.elapsed();

        // A square at the origin, as a player's view is, and chunks far apart, where
        // nothing found for one helps the next.
        let near: Vec<(i32, i32)> = (-16..16)
            .flat_map(|x| (-16..16).map(move |z| (x, z)))
            .collect();
        let far: Vec<(i32, i32)> = (0..1024)
            .map(|index| (index * 7919 - 4_000_000, 3_000_000 - index * 6007))
            .collect();
        for (name, chunks) in [("in a square of 32 by 32", near), ("far apart", far)] {
            let started = std::time::Instant::now();
            let mut cells = 0;
            for (x, z) in &chunks {
                cells += std::hint::black_box(source.chunk(*x, *z)).len() * 64;
            }
            let spent = started.elapsed();
            println!(
                "{dimension:?}, {} chunks {name}: {:.3} ms a chunk, {:.0} ns a cell",
                chunks.len(),
                spent.as_secs_f64() * 1e3 / chunks.len() as f64,
                spent.as_secs_f64() * 1e9 / cells as f64,
            );
        }
        println!(
            "{dimension:?}: {:.3} ms to make the source of a seed",
            made.as_secs_f64() * 1e3
        );
    }
}
