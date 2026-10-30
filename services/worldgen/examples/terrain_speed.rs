//! Measures what the preview of the terrain costs: `cargo run --release -p
//! clustine-worldgen --example terrain_speed -- [seed]`. One thread, as the world
//! store's one chunk thread asks. The roadmap has the numbers this gave.

use std::time::{Duration, Instant};

use clustine_world::{ChunkGenerator, ChunkPos};
use clustine_worldgen::TerrainGenerator;

/// The chunks a client is sent around `centre` at the view distance `view`: what
/// the edge asks for at a join.
fn view_area(centre: (i32, i32), view: i32) -> Vec<ChunkPos> {
    let reach = view + 2;
    let mut area = Vec::new();
    for x in -reach..=reach {
        for z in -reach..=reach {
            let (dx, dz) = ((x.abs() - 2).max(0), (z.abs() - 2).max(0));
            if dx * dx + dz * dz < view * view {
                area.push(ChunkPos::new(centre.0 + x, centre.1 + z));
            }
        }
    }
    area
}

fn a_second(chunks: usize, took: Duration) -> f64 {
    chunks as f64 / took.as_secs_f64()
}

fn main() {
    let seed = std::env::args()
        .nth(1)
        .map_or(13579, |seed| seed.parse().expect("the seed is a number"));
    let started = Instant::now();
    let generator = TerrainGenerator::new(seed, clustine_world::Dimension::Overworld);
    println!(
        "seed {seed}: seeding the noises took {:?}",
        started.elapsed()
    );

    // The blocks of a chunk alone, which is what the cache spares.
    let started = Instant::now();
    let unlit: Vec<_> = (0..32)
        .map(|index| generator.unlit(ChunkPos::new(100 + index, -40)))
        .collect();
    let blocks = started.elapsed() / 32;
    println!("an unlit chunk (blocks and biomes): {blocks:?}");
    drop(unlit);

    // Cold: chunks far from each other, each of which needs nine chunks' blocks.
    let apart: Vec<_> = (0..16)
        .map(|index| ChunkPos::new(index * 40, 300))
        .collect();
    generator.forget();
    let started = Instant::now();
    for position in &apart {
        generator.generate(*position);
    }
    let cold = started.elapsed();
    println!(
        "cold, 16 chunks far apart: {:?} a chunk, {:.1} a second",
        cold / 16,
        a_second(16, cold)
    );
    // The same again with their blocks kept: what the light alone costs.
    let started = Instant::now();
    for position in &apart {
        generator.generate(*position);
    }
    println!(
        "the light of a chunk beside eight: {:?}",
        started.elapsed() / 16
    );

    // A join: the chunks of a view, nothing kept.
    for view in [8, 3] {
        generator.forget();
        let area = view_area((0, 0), view);
        let started = Instant::now();
        for position in &area {
            generator.generate(*position);
        }
        let took = started.elapsed();
        println!(
            "a join at view distance {view}: {} chunks in {took:?}, {:.1} a second ({} unlit chunks kept)",
            area.len(),
            a_second(area.len(), took),
            generator.kept()
        );
    }

    // Walking east from there at the view distance 8, a chunk at a time: only the
    // chunks that come into view are made.
    generator.forget();
    let mut seen = view_area((0, 0), 8);
    for position in &seen {
        generator.generate(*position);
    }
    let (mut made, mut took) = (0, Duration::ZERO);
    let steps = 24;
    for step in 1..=steps {
        let area = view_area((step, 0), 8);
        let started = Instant::now();
        for position in area.iter().filter(|position| !seen.contains(position)) {
            generator.generate(*position);
            made += 1;
        }
        took += started.elapsed();
        seen = area;
    }
    println!(
        "walking {steps} chunks east at view distance 8: {made} chunks in {took:?}, {:.1} a second, {:?} for each chunk walked",
        a_second(made, took),
        took / steps as u32
    );
}
