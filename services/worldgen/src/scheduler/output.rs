//! What the tasks give, as the cache holds it, and how the changes of runs are put
//! into a chunk (definition D of ADR-0021, section 2).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use clustine_data::BlockState;
use clustine_world::generation::{PieceKey, PieceState};
use clustine_world::{BlockEntity, Chunk, ChunkPos, InChunk, ScheduledTick, SectionLight, Ticked};

use super::Recorded;
use crate::stages::{Starts, Terrain};

/// The result of a task.
#[derive(Debug)]
pub(crate) enum Output {
    Starts(Starts),
    Terrain(MadeTerrain),
    Run(RunResult),
    Finished(FinishedChunk),
    /// A lit chunk, marked as generated.
    Lit(Chunk),
}

/// A chunk's terrain, with the starts its references name: a run of the chunk needs
/// those, and keeping them here spares the run a dependency it could only find out
/// once the terrain is made.
#[derive(Debug)]
pub(crate) struct MadeTerrain {
    pub(crate) terrain: Terrain,
    /// The output of `Starts(s)` for each source `s` the references name.
    pub(crate) starts: BTreeMap<ChunkPos, Arc<Output>>,
}

/// What one run changed (ADR-0021, section 5, "run results").
#[derive(Debug)]
pub(crate) struct RunResult {
    /// By chunk of the run's three by three, in the order of `lattice::around`.
    pub(crate) changes: Vec<Changes>,
    /// The pieces whose state the run set, with the state it left them in.
    pub(crate) pieces: BTreeMap<PieceKey, PieceState>,
    pub(crate) recorded: Recorded,
    /// Placements that failed, and the run's being cut short.
    pub(crate) failures: usize,
    /// Whether this run or any run it saw has a failure: whether a chunk that depends
    /// on it is marked "with a failure".
    pub(crate) tainted: bool,
}

/// What a run changed in one chunk.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    /// Each position the run wrote, with the block and the block entity it left there,
    /// ascending.
    pub(crate) blocks: Vec<(InChunk, BlockState, Option<BlockEntity>)>,
    /// The ticks it scheduled, in its order.
    pub(crate) ticks: Vec<ScheduledTick>,
    /// The positions it marked, ascending.
    pub(crate) marks: Vec<InChunk>,
}

/// A finished chunk: definition D, before light.
#[derive(Debug)]
pub(crate) struct FinishedChunk {
    pub(crate) chunk: Chunk,
    /// Whether any of the nine runs whose changes it has, or a run they saw, failed.
    pub(crate) tainted: bool,
}

impl Output {
    pub(crate) fn starts(&self) -> &Starts {
        match self {
            Output::Starts(starts) => starts,
            other => panic!("starts were expected, not {}", other.kind()),
        }
    }

    pub(crate) fn terrain(&self) -> &MadeTerrain {
        match self {
            Output::Terrain(terrain) => terrain,
            other => panic!("terrain was expected, not {}", other.kind()),
        }
    }

    pub(crate) fn run(&self) -> &RunResult {
        match self {
            Output::Run(run) => run,
            other => panic!("a run was expected, not {}", other.kind()),
        }
    }

    pub(crate) fn finished(&self) -> &FinishedChunk {
        match self {
            Output::Finished(finished) => finished,
            other => panic!("a finished chunk was expected, not {}", other.kind()),
        }
    }

    /// The chunk a request is answered with: a lit chunk, or a finished one.
    pub(crate) fn answer(&self) -> Chunk {
        match self {
            Output::Lit(chunk) => chunk.clone(),
            Output::Finished(finished) => finished.chunk.clone(),
            other => panic!("only chunks answer a request, not {}", other.kind()),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Output::Starts(_) => "starts",
            Output::Terrain(_) => "terrain",
            Output::Run(_) => "a run",
            Output::Finished(_) => "a finished chunk",
            Output::Lit(_) => "a lit chunk",
        }
    }

    /// What the entry reckons it takes in memory. It is an estimate from the shape of
    /// what is held, not a count of the allocator's: enough to bound the cache, and
    /// cheap enough to take for every entry.
    pub(crate) fn bytes(&self) -> usize {
        const BASE: usize = 64;
        BASE + match self {
            Output::Starts(starts) => starts_bytes(starts),
            Output::Terrain(made) => {
                // The starts it keeps are counted where the cache holds them.
                chunk_bytes(&made.terrain.chunk)
                    + std::mem::size_of_val(&*made.terrain.worldgen_heights)
                    + made.terrain.references.len() * 8
                    + made.starts.len() * 48
            }
            Output::Run(run) => {
                let changes: usize = run
                    .changes
                    .iter()
                    .map(|changes| {
                        let entities: usize = changes
                            .blocks
                            .iter()
                            .filter_map(|(_, _, entity)| entity.as_ref())
                            .map(|entity| entity.data.len() + 32)
                            .sum();
                        48 + changes.blocks.len() * 24
                            + entities
                            + changes.ticks.len() * std::mem::size_of::<ScheduledTick>()
                            + changes.marks.len() * std::mem::size_of::<InChunk>()
                    })
                    .sum();
                changes + run.pieces.len() * 48
            }
            Output::Finished(finished) => chunk_bytes(&finished.chunk),
            Output::Lit(chunk) => chunk_bytes(chunk),
        }
    }
}

fn starts_bytes(starts: &Starts) -> usize {
    starts
        .starts
        .iter()
        .map(|start| 48 + start.pieces.len() * 48)
        .sum()
}

/// A chunk's size, by the forms its sections take: one that holds one state takes
/// next to nothing, any other a palette and its indices, which for most is four bits a
/// block (2 KiB); the four heightmaps; block entities with their data; light.
pub(crate) fn chunk_bytes(chunk: &Chunk) -> usize {
    const SECTION: usize = 96;
    const INDICES: usize = 2048;
    const BIOME_CELLS: usize = 64 * 2;
    let sections: usize = chunk
        .sections()
        .iter()
        .map(|section| {
            let blocks = if section.uniform_state().is_some() {
                0
            } else {
                INDICES
            };
            let biomes = if section.uniform_biome().is_some() {
                0
            } else {
                BIOME_CELLS
            };
            SECTION + blocks + biomes
        })
        .sum();
    let entities: usize = chunk
        .block_entities()
        .values()
        .map(|entity| 48 + entity.data.len())
        .sum();
    let light = chunk.light().map_or(0, |light| {
        light
            .sky
            .iter()
            .chain(&light.block)
            .map(|section| match section {
                SectionLight::Levels(levels) => levels.len() + 16,
                _ => 16,
            })
            .sum()
    });
    sections
        + entities
        + light
        + std::mem::size_of_val(chunk.heights())
        + std::mem::size_of_val(chunk.ticks())
        + std::mem::size_of_val(chunk.marks())
}

/// Puts the blocks and block entities a run left into `chunk`. The ticks and marks
/// are not: a run does not read them, and [`finish`] puts them in.
pub(crate) fn apply_blocks(chunk: &mut Chunk, changes: &Changes) {
    for (at, state, entity) in &changes.blocks {
        chunk.set(
            usize::from(at.x),
            i32::from(at.y),
            usize::from(at.z),
            *state,
        );
        if let Some(entity) = entity {
            // The run left this data with this block in its own view of the chunk,
            // where it fitted. Under the order `alone` the chunk can hold other data
            // from a lower run than the view did, and data that does not fit beside
            // it is left out: that shape is not claimed to be exact.
            let _ = chunk.set_block_entity_data(*at, entity.data.clone());
        }
    }
}

/// What makes two ticks one for the game: where, and of which block or fluid.
fn tick_key(tick: &ScheduledTick) -> (i16, u8, u8, bool, u16) {
    let (fluid, id) = match tick.of {
        Ticked::Block(block) => (false, block),
        Ticked::Fluid(fluid) => (true, u16::from(fluid)),
    };
    (tick.at.y, tick.at.z, tick.at.x, fluid, id)
}

/// Definition D: the terrain of `e` with what the runs within 1 of it changed in it,
/// in ascending rank. `runs` are those runs with their positions, in ascending rank.
pub(crate) fn finish(
    e: ChunkPos,
    terrain: &Terrain,
    runs: &[(ChunkPos, Arc<Output>)],
) -> FinishedChunk {
    let mut chunk = terrain.chunk.clone();
    let mut ticks = chunk.ticks().to_vec();
    // The game keeps one tick of a block or a fluid at a position while a chunk is
    // generated (`ProtoChunkTicks.schedule`): a second is not added, whichever run
    // asks for it.
    let mut scheduled: BTreeSet<_> = ticks.iter().map(tick_key).collect();
    let mut marks = chunk.marks().to_vec();
    let mut tainted = false;
    for (d, run) in runs {
        let run = run.run();
        let index =
            super::lattice::index_in_three(*d, e).expect("a finished chunk has runs within 1");
        let changes = &run.changes[index];
        apply_blocks(&mut chunk, changes);
        for tick in &changes.ticks {
            if scheduled.insert(tick_key(tick)) {
                ticks.push(*tick);
            }
        }
        marks.extend_from_slice(&changes.marks);
        tainted |= run.tainted;
    }
    chunk.set_ticks(ticks);
    chunk.set_marks(marks);
    FinishedChunk { chunk, tainted }
}
