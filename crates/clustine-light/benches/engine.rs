//! What the engine costs (`docs/groundwork/n8-plan.md`, section 8), against the oracle
//! that lights whole: `cargo bench -p clustine-light --bench engine`.
//!
//! The world is made here, not generated: rolling ground of stone under dirt and grass
//! between 50 and 80, a sea at 62, caves, trees of logs and leaves, and torches in the
//! caves, 24 sections from -64 as the overworld has them. It has what costs light its
//! work (columns of sky ending at different heights, water and leaves that dampen by
//! one, sources under the ground) without a generator, which this crate does not have.
//!
//! Each case is timed whole, a run of the engine with what was noted before it, and
//! said in milliseconds with the engine's own count of its work.

#![allow(
    clippy::disallowed_methods,
    reason = "the bench, not the crate, takes the time"
)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use clustine_data::{BlockState, blocks};
use clustine_light::engine::{Chunks, Kept, Lighting, Oracle, Work};
use clustine_world::{Biome, BlockPos, Chunk, ChunkPos, Section};

const MIN_Y: i32 = -64;
const SECTIONS: usize = 24;
const SEA: i32 = 62;

struct World(BTreeMap<ChunkPos, Chunk>);

impl Chunks for World {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk> {
        self.0.get(&at)
    }
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// A number from a position, the same everywhere.
fn hash(x: i32, y: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b1)
        ^ (y as u32).wrapping_mul(0x85eb_ca77)
        ^ (z as u32).wrapping_mul(0xc2b2_ae3d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ h >> 12
}

fn ground(x: i32, z: i32) -> i32 {
    let (x, z) = (f64::from(x), f64::from(z));
    (65.0 + 9.0 * (x / 23.0).sin() + 7.0 * (z / 31.0).cos() + 3.0 * ((x + z) / 11.0).sin()) as i32
}

fn chunk(at: ChunkPos) -> Chunk {
    let sections = (0..SECTIONS)
        .map(|index| {
            // The deep sections are stone through, as most of a real world's are.
            let state = if index < 6 {
                blocks::STONE
            } else {
                blocks::AIR
            };
            Section::filled(state, Biome(0))
        })
        .collect();
    let mut chunk = Chunk::from_sections(MIN_Y, sections);
    let (dirt, grass, log, leaves) = (
        state("minecraft:dirt"),
        state("minecraft:grass_block[snowy=false]"),
        state("minecraft:oak_log[axis=y]"),
        state("minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]"),
    );
    for z in 0..16 {
        for x in 0..16 {
            let (wx, wz) = (at.x * 16 + x as i32, at.z * 16 + z as i32);
            let top = ground(wx, wz);
            for y in MIN_Y + 96..=top {
                let state = match top - y {
                    0 if top >= SEA => grass,
                    0..=3 => dirt,
                    _ => blocks::STONE,
                };
                chunk.set(x, y, z, state);
            }
            for y in top + 1..=SEA {
                chunk.set(x, y, z, blocks::WATER);
            }
            // Caves: a winding tube of air at two depths.
            for depth in [12, 30] {
                let centre = top - depth + (f64::from(wx) / 9.0).sin() as i32 * 3;
                if ((wz as f64 / 7.0).sin() * 5.0 + (wx as f64 / 13.0).cos() * 4.0).abs() < 2.5 {
                    for y in centre - 2..=centre + 2 {
                        chunk.set(x, y, z, blocks::AIR);
                    }
                    if hash(wx, depth, wz).is_multiple_of(97) {
                        chunk.set(x, centre - 2, z, blocks::TORCH);
                    }
                }
            }
        }
    }
    // A few trees on the land.
    for tree in 0..3 {
        let (x, z) = (
            (hash(at.x, tree, at.z) % 12 + 2) as usize,
            (hash(at.z, tree, at.x) % 12 + 2) as usize,
        );
        let top = ground(at.x * 16 + x as i32, at.z * 16 + z as i32);
        if top < SEA {
            continue;
        }
        for y in top + 1..top + 6 {
            chunk.set(x, y, z, log);
        }
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                for y in top + 4..top + 8 {
                    let (lx, lz) = (x as i32 + dx, z as i32 + dz);
                    if chunk.get(lx as usize, y, lz as usize) == Some(blocks::AIR) {
                        chunk.set(lx as usize, y, lz as usize, leaves);
                    }
                }
            }
        }
    }
    chunk
}

fn world_of(side: i32) -> World {
    let mut chunks = BTreeMap::new();
    for x in 0..side {
        for z in 0..side {
            chunks.insert(ChunkPos::new(x, z), chunk(ChunkPos::new(x, z)));
        }
    }
    World(chunks)
}

fn lit(world: &World) -> Lighting {
    let mut lighting = Lighting::new(true);
    for at in world.0.keys() {
        lighting.arrived(*at);
    }
    lighting.run(world);
    lighting
}

fn milliseconds(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

fn say(case: &str, time: Duration, work: Work) {
    println!(
        "{case:<52} {:>9.3} ms   {:>9} cells, peak {:>7}, {} lit whole, {} adopted",
        milliseconds(time),
        work.cells,
        work.peak,
        work.chunks_lit_whole,
        work.adopted
    );
}

/// Sets blocks, notes them and times the run that lights them.
fn changes(
    world: &mut World,
    lighting: &mut Lighting,
    blocks: &[(BlockPos, BlockState)],
) -> (Duration, Work) {
    for (at, state) in blocks {
        let chunk = world.0.get_mut(&at.chunk()).expect("the world has it");
        chunk.set((at.x & 15) as usize, at.y, (at.z & 15) as usize, *state);
        lighting.changed(*at);
    }
    let started = Instant::now();
    let work = lighting.run(world);
    (started.elapsed(), work)
}

fn main() {
    let side = 12;
    let mut world = world_of(side);
    let mut lighting = lit(&world);

    // A torch placed in the open and broken, many times over: the mean.
    let middle = BlockPos::new(
        side * 8 + 3,
        ground(side * 8 + 3, side * 8 + 5) + 1,
        side * 8 + 5,
    );
    let times = 200;
    let (mut placed, mut broken) = (Duration::ZERO, Duration::ZERO);
    let (mut placed_work, mut broken_work) = (Work::default(), Work::default());
    for _ in 0..times {
        let (time, work) = changes(&mut world, &mut lighting, &[(middle, blocks::TORCH)]);
        placed += time;
        placed_work = work;
        let (time, work) = changes(&mut world, &mut lighting, &[(middle, blocks::AIR)]);
        broken += time;
        broken_work = work;
    }
    say(
        "a torch placed on the ground (mean)",
        placed / times,
        placed_work,
    );
    say("the torch broken (mean)", broken / times, broken_work);

    // A block of stone put over the open ground and taken away: a column of sky.
    let above = BlockPos::new(middle.x + 1, middle.y + 6, middle.z);
    let (time, work) = changes(&mut world, &mut lighting, &[(above, blocks::STONE)]);
    say("a stone put in the open sky", time, work);
    let (time, work) = changes(&mut world, &mut lighting, &[(above, blocks::AIR)]);
    say("the stone taken away", time, work);

    // A glowstone ceiling of 64, eight blocks over the ground across a chunk's corner,
    // put and taken: it shades the ground from the sky and lights it.
    let height = (40..48).map(|x| ground(x, 40)).max().unwrap_or(0) + 8;
    let ceiling: Vec<BlockPos> = (0..8)
        .flat_map(|dx| (0..8).map(move |dz| BlockPos::new(44 + dx, height, 44 + dz)))
        .collect();
    let put: Vec<_> = ceiling.iter().map(|at| (*at, blocks::GLOWSTONE)).collect();
    let (time, work) = changes(&mut world, &mut lighting, &put);
    say("a glowstone ceiling of 64 put over the ground", time, work);
    let taken: Vec<_> = ceiling.iter().map(|at| (*at, blocks::AIR)).collect();
    let (time, work) = changes(&mut world, &mut lighting, &taken);
    say("the ceiling taken away", time, work);

    // A river: a trench two deep dug across the world and filled with water.
    let river: Vec<(BlockPos, BlockState)> = (0..side * 16)
        .flat_map(|x| {
            let z = side * 8;
            let top = ground(x, z);
            [
                (BlockPos::new(x, top, z), blocks::WATER),
                (BlockPos::new(x, top - 1, z), blocks::WATER),
            ]
        })
        .collect();
    let (time, work) = changes(&mut world, &mut lighting, &river);
    say(
        &format!("a river of {} blocks across the world", river.len()),
        time,
        work,
    );

    // A crater: a sphere of radius 8 emptied from the ground, as 64 TNT would.
    let centre = BlockPos::new(side * 4, ground(side * 4, side * 4) - 2, side * 4);
    let mut crater = Vec::new();
    for dx in -8i32..=8 {
        for dy in -8i32..=8 {
            for dz in -8i32..=8 {
                if dx * dx + dy * dy + dz * dz <= 64 {
                    let at = BlockPos::new(centre.x + dx, centre.y + dy, centre.z + dz);
                    crater.push((at, blocks::AIR));
                }
            }
        }
    }
    let (time, work) = changes(&mut world, &mut lighting, &crater);
    say(
        &format!("a crater of {} blocks emptied", crater.len()),
        time,
        work,
    );

    // A hundred chunks arriving at once, against the oracle lighting them whole.
    let arriving = world_of(10);
    let mut fresh = Lighting::new(true);
    for at in arriving.0.keys() {
        fresh.arrived(*at);
    }
    let started = Instant::now();
    let work = fresh.run(&arriving);
    say("100 chunks arriving", started.elapsed(), work);
    let mut oracle = Oracle::new(true);
    for at in arriving.0.keys() {
        oracle.arrived(*at);
    }
    let started = Instant::now();
    let work = oracle.run(&arriving);
    say(
        "100 chunks arriving, lit whole by the oracle",
        started.elapsed(),
        work,
    );

    // A hundred chunks beside a lit area, as a view grows: one ring at a time.
    let mut grown = Lighting::new(true);
    let large = world_of(20);
    for at in large.0.keys().filter(|at| at.x < 10) {
        grown.arrived(*at);
    }
    grown.run(&large);
    for at in large.0.keys().filter(|at| (10..15).contains(&at.x)) {
        grown.arrived(*at);
    }
    let started = Instant::now();
    let work = grown.run(&large);
    say(
        "100 chunks arriving beside 200 lit",
        started.elapsed(),
        work,
    );

    // A hundred chunks adopted with their levels: from one hand-over in one run, and
    // each beside chunks lit already.
    let kept: Vec<(ChunkPos, Kept)> = arriving
        .0
        .keys()
        .map(|at| (*at, fresh.kept(*at, (1, 1)).expect("lit")))
        .collect();
    let mut adopting = Lighting::new(true);
    for (at, kept) in kept.iter().cloned() {
        adopting.adopt(at, kept);
    }
    let started = Instant::now();
    let work = adopting.run(&arriving);
    say(
        "100 chunks adopted from one hand-over",
        started.elapsed(),
        work,
    );
    let mut adopting = Lighting::new(true);
    for (at, kept) in kept.iter().cloned() {
        // Each from a hand-over of its own: every face mended.
        let mut kept = kept;
        kept.taken = (2, u64::from(at.x as u32) << 32 | u64::from(at.z as u32));
        adopting.adopt(at, kept);
    }
    let started = Instant::now();
    let work = adopting.run(&arriving);
    say(
        "100 chunks adopted, each from its own hand-over",
        started.elapsed(),
        work,
    );

    // The same hundred lit whole by the oracle again, chunk by chunk, for the plan's
    // bound of 0.5 ms a chunk.
    println!(
        "{:<52} {:>9.3} ms",
        "a chunk lit whole by the crate's function (mean)",
        {
            let started = Instant::now();
            for at in arriving.0.keys() {
                let neighbours = [
                    (-1, -1),
                    (0, -1),
                    (1, -1),
                    (-1, 0),
                    (1, 0),
                    (-1, 1),
                    (0, 1),
                    (1, 1),
                ]
                .map(|(dx, dz)| arriving.0.get(&ChunkPos::new(at.x + dx, at.z + dz)));
                std::hint::black_box(clustine_light::light(&clustine_light::Around {
                    centre: &arriving.0[at],
                    neighbours,
                    sky: true,
                }));
            }
            milliseconds(started.elapsed()) / arriving.0.len() as f64
        }
    );
}
