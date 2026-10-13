//! What the biomes, the fill and the surface of a chunk cost: a measurement that
//! reports and asserts nothing (`docs/groundwork/terrain-plan.md`, section 6).
//!
//! ```text
//! cargo test --release -p clustine-terrain --test cost -- --ignored --nocapture
//! ```

use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::{Heightmap, Terrain};
use clustine_terrain::noise_chunk::{self, Scratch};
use clustine_terrain::surface::Biomes;
use clustine_worldgen_data::{EndRouter, Give, NetherRouter, NoiseRouter, OverworldRouter};

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

/// The inputs of the interpolated functions of the final density at the corners of a
/// chunk's cells, asked as the fill asks them; their sum, so that they are computed.
fn corners<R: NoiseRouter>(router: &R, chunk_x: i32, chunk_z: i32) -> f32 {
    let settings = router.settings();
    let mut memory = R::Memory::default();
    memory.in_volume(true);
    let mut sum = 0.0;
    for (number, function) in router.interpolated().iter().enumerate() {
        if !function.in_final_density {
            continue;
        }
        let (across, up) = (function.cell_size_xz, function.cell_size_y);
        for x in 0..=16 / across {
            for z in 0..=16 / across {
                for y in 0..=settings.height / up {
                    sum += router.interpolated_input(
                        &mut memory,
                        number,
                        chunk_x * 16 + x * across,
                        settings.min_y + y * up,
                        chunk_z * 16 + z * across,
                    );
                }
            }
        }
    }
    sum
}

#[test]
#[ignore = "a measurement, for a release build"]
#[allow(
    clippy::disallowed_methods,
    reason = "the test, not the crate, takes the time"
)]
fn the_time_a_chunks_fill_takes() {
    let overworld = OverworldRouter::new(13579);
    let nether = NetherRouter::new(13579);
    let end = EndRouter::new(13579);
    for dimension in Dimension::ALL {
        let started = std::time::Instant::now();
        let terrain = Terrain::new(13579, dimension);
        let made = started.elapsed();

        // A square at the origin, as a player's view is, and chunks far apart.
        let near: Vec<(i32, i32)> = (-8..8).flat_map(|x| (-8..8).map(move |z| (x, z))).collect();
        let far: Vec<(i32, i32)> = (0..256)
            .map(|index| (index * 7919 - 1_000_000, 750_000 - index * 6007))
            .collect();
        for (name, chunks) in [("in a square of 16 by 16", &near), ("far apart", &far)] {
            // The density alone, and the whole fill, which is the density and the
            // aquifer's answer for every block.
            let mut scratch = Scratch::default();
            let started = std::time::Instant::now();
            for (x, z) in chunks {
                let volume = terrain.volume(*x, *z);
                std::hint::black_box(terrain.final_density(&volume, &mut scratch));
            }
            let density = started.elapsed();
            let started = std::time::Instant::now();
            let mut marks = 0;
            for (x, z) in chunks {
                marks += std::hint::black_box(terrain.fill(*x, *z)).marks.len();
            }
            let fill = started.elapsed();
            // Of the density, the inputs of the interpolated functions at the
            // corners of the cells; the rest is the walk through the cells and what
            // stands above the interpolation, block by block.
            let started = std::time::Instant::now();
            for (x, z) in chunks {
                std::hint::black_box(match dimension {
                    Dimension::Overworld => corners(&overworld, *x, *z),
                    Dimension::Nether => corners(&nether, *x, *z),
                    Dimension::End => corners(&end, *x, *z),
                });
            }
            let corners = started.elapsed();
            // And of what is not the density, making the aquifer: the height of the
            // ground around the chunk. The rest is the aquifer's answer for each
            // block and the placing.
            let started = std::time::Instant::now();
            for (x, z) in chunks {
                std::hint::black_box(terrain.aquifer_ceiling(*x, *z));
            }
            let aquifer = started.elapsed();
            let each = |spent: std::time::Duration| spent.as_secs_f64() * 1e3 / chunks.len() as f64;
            println!(
                "{dimension:?}, {} chunks {name}: {:.3} ms a chunk to fill; of which {:.3} ms \
                 the density ({:.3} ms of it the corners) and {:.3} ms making the aquifer; \
                 {:.1} marks a chunk",
                chunks.len(),
                each(fill),
                each(density),
                each(corners),
                each(aquifer),
                marks as f64 / chunks.len() as f64,
            );
        }

        // A base height: one column made alone, as structures ask for.
        let started = std::time::Instant::now();
        let mut sum = 0_i64;
        let columns = 2000;
        for index in 0..columns {
            sum += i64::from(terrain.base_height(
                index * 37 - 30_000,
                20_000 - index * 53,
                Heightmap::WorldSurfaceWg,
            ));
        }
        std::hint::black_box(sum);
        println!(
            "{dimension:?}: {:.1} µs a base height; {:.3} ms to make the terrain of a seed",
            started.elapsed().as_secs_f64() * 1e6 / f64::from(columns),
            made.as_secs_f64() * 1e3
        );
    }
}

#[test]
#[ignore = "a measurement, for a release build"]
#[allow(
    clippy::disallowed_methods,
    reason = "the test, not the crate, takes the time"
)]
fn the_time_a_chunks_surface_takes() {
    let overworld = OverworldRouter::new(13579);
    let in_rule: Vec<usize> = (0..overworld.interpolated().len())
        .filter(|number| overworld.interpolated()[*number].in_material_rule)
        .collect();
    for dimension in Dimension::ALL {
        let terrain = Terrain::new(13579, dimension);
        let source = BiomeSource::new(13579, dimension);
        // A square at the origin, as a player's view is, and chunks far apart.
        let near: Vec<(i32, i32)> = (-8..8).flat_map(|x| (-8..8).map(move |z| (x, z))).collect();
        let far: Vec<(i32, i32)> = (0..256)
            .map(|index| (index * 7919 - 1_000_000, 750_000 - index * 6007))
            .collect();
        for (name, chunks) in [("in a square of 16 by 16", &near), ("far apart", &far)] {
            let mut scratch = Scratch::default();
            // The biomes the step reads, the fill it starts from, and the step.
            let started = std::time::Instant::now();
            let biomes: Vec<Biomes> = chunks
                .iter()
                .map(|(x, z)| Biomes::from_source(&source, *x, *z))
                .collect();
            let reading = started.elapsed();
            let mut filled: Vec<_> = chunks.iter().map(|(x, z)| terrain.fill(*x, *z)).collect();
            let started = std::time::Instant::now();
            let mut marks = 0;
            for (((x, z), filled), biomes) in chunks.iter().zip(&mut filled).zip(&biomes) {
                terrain.build_surface(filled, *x, *z, biomes, &mut scratch);
                marks += filled.marks.len();
            }
            let surface = started.elapsed();
            // Of the step: the level of the surface, and at most this for the ore
            // veins' interpolated functions, which is what they take over the whole
            // height (the step interpolates them up to the highest section that is
            // not air). The rest is the rule, asked for every block of ground.
            let started = std::time::Instant::now();
            for (x, z) in chunks {
                std::hint::black_box(terrain.surface_level(*x, *z));
            }
            let level = started.elapsed();
            let started = std::time::Instant::now();
            if dimension == Dimension::Overworld {
                for (x, z) in chunks {
                    noise_chunk::interpolated(
                        &overworld,
                        &mut Default::default(),
                        &in_rule,
                        &terrain.volume(*x, *z),
                        &mut scratch,
                    );
                }
            }
            let veins = started.elapsed();
            let each = |spent: std::time::Duration| spent.as_secs_f64() * 1e3 / chunks.len() as f64;
            println!(
                "{dimension:?}, {} chunks {name}: {:.3} ms a chunk for the surface, after \
                 {:.3} ms for the biomes it reads; of the surface {:.3} ms the level of the \
                 surface and at most {:.3} ms the ore veins' interpolated functions; {:.1} \
                 marks a chunk",
                chunks.len(),
                each(surface),
                each(reading),
                each(level),
                each(veins),
                marks as f64 / chunks.len() as f64,
            );
        }
    }
}
