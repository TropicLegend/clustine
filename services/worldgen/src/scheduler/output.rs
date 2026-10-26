//! What the tasks give, as the cache holds it, and how the changes of runs are put
//! into a chunk (definition D of ADR-0021, section 2).

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use clustine_data::BlockState;
use clustine_world::generation::{MAX_GENERATED_OF_A_CLASS, Maker, PieceKey, PieceState};
use clustine_world::{
    BlockEntity, Chunk, ChunkPos, Entity, EntityId, Generated, InChunk, ScheduledTick,
    SectionLight, Ticked,
};

use super::Recorded;
use super::view::{Context, message};
use crate::stages::{Starts, Terrain};

/// The result of a task.
#[derive(Debug)]
pub(crate) enum Output {
    Starts(Starts),
    Terrain(MadeTerrain),
    Run(RunResult),
    Finished(FinishedChunk),
    Lit(LitChunk),
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
    /// The entities it made whose position is in the chunk, in its order, without
    /// their keys.
    pub(crate) entities: Vec<Entity>,
}

/// A finished chunk: definition D, before light.
#[derive(Debug)]
pub(crate) struct FinishedChunk {
    pub(crate) chunk: Chunk,
    /// Whether any of the nine runs whose changes it has, or a run they saw, failed,
    /// or putting in its entities did.
    pub(crate) tainted: bool,
    /// What failed in putting in its entities: each entity whose record could not be
    /// made, and the class that had more than its keys.
    pub(crate) failures: usize,
}

/// A lit chunk, marked as generated, with the animals of its spawn.
#[derive(Debug)]
pub(crate) struct LitChunk {
    pub(crate) chunk: Chunk,
    /// What failed of its spawn, as [`FinishedChunk::failures`] counts.
    pub(crate) failures: usize,
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

    pub(crate) fn lit(&self) -> &LitChunk {
        match self {
            Output::Lit(lit) => lit,
            other => panic!("a lit chunk was expected, not {}", other.kind()),
        }
    }

    /// The chunk a request is answered with: a lit chunk, or a finished one.
    pub(crate) fn answer(&self) -> Chunk {
        match self {
            Output::Lit(lit) => lit.chunk.clone(),
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
                        let block_entities: usize = changes
                            .blocks
                            .iter()
                            .filter_map(|(_, _, entity)| entity.as_ref())
                            .map(|entity| entity.data.len() + 32)
                            .sum();
                        48 + changes.blocks.len() * 24
                            + block_entities
                            + changes.ticks.len() * std::mem::size_of::<ScheduledTick>()
                            + changes.marks.len() * std::mem::size_of::<InChunk>()
                            + changes.entities.len() * ENTITY_BYTES
                    })
                    .sum();
                changes + run.pieces.len() * 48
            }
            Output::Finished(finished) => chunk_bytes(&finished.chunk),
            Output::Lit(lit) => chunk_bytes(&lit.chunk),
        }
    }
}

/// What an entity is reckoned to take: its record, and what its data holds beside it
/// for a mob's mind and inventories, as a guess. Enough to bound the cache.
const ENTITY_BYTES: usize = std::mem::size_of::<Entity>() + 256;

fn starts_bytes(starts: &Starts) -> usize {
    starts
        .starts
        .iter()
        .map(|start| 48 + start.pieces.len() * 48)
        .sum()
}

/// A chunk's size, by the forms its sections take: one that holds one state takes
/// next to nothing, any other a palette and its indices, which for most is four bits a
/// block (2 KiB); the four heightmaps; block entities with their data; entities; light.
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
    let block_entities: usize = chunk
        .block_entities()
        .values()
        .map(|entity| 48 + entity.data.len())
        .sum();
    let entities = chunk.entities().len() * ENTITY_BYTES;
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
        + block_entities
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
///
/// The entities the runs made in `e` are put in in the same order, and keyed there
/// (`docs/groundwork/n7l-plan.md`, section 9 item 1): by `e`, as a structure's, with
/// their place in that order from 0. The maker makes each one's record with its key.
pub(crate) fn finish(
    e: ChunkPos,
    terrain: &Terrain,
    runs: &[(ChunkPos, Arc<Output>)],
    maker: &dyn Maker,
    context: &Context,
) -> FinishedChunk {
    let mut chunk = terrain.chunk.clone();
    let mut ticks = chunk.ticks().to_vec();
    // The game keeps one tick of a block or a fluid at a position while a chunk is
    // generated (`ProtoChunkTicks.schedule`): a second is not added, whichever run
    // asks for it.
    let mut scheduled: BTreeSet<_> = ticks.iter().map(tick_key).collect();
    let mut marks = chunk.marks().to_vec();
    let mut tainted = false;
    let mut made = Vec::new();
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
        made.extend(changes.entities.iter().cloned());
        tainted |= run.tainted;
    }
    chunk.set_ticks(ticks);
    chunk.set_marks(marks);
    let failures = add_keyed(&mut chunk, e, true, made, maker, context);
    FinishedChunk {
        chunk,
        tainted: tainted || failures > 0,
        failures,
    }
}

/// Keys `made`, the entities of one class in `chunk` at `position` in the order they
/// were made, by the chunk and their place in that order from 0, has the maker make
/// their records with their keys, and adds them to the chunk. Returns what failed: one
/// for each entity whose record could not be made, which is left out, and one if there
/// were more than a class has keys, the rest of which are left out.
///
/// The members of a unit (a baby zombie villager on its chicken) are made naming each
/// other by the keys of their makings, which each carries as its `key`; those names
/// become their keys here. A link to an entity not among `made` is dropped.
pub(crate) fn add_keyed(
    chunk: &mut Chunk,
    position: ChunkPos,
    structure: bool,
    made: Vec<Entity>,
    maker: &dyn Maker,
    context: &Context,
) -> usize {
    if made.is_empty() {
        return 0;
    }
    let class = if structure {
        "a structure's"
    } else {
        "the spawn's"
    };
    let key_of = |index: usize| {
        Generated {
            dimension: context.dimension_number,
            chunk: position,
            structure,
            index: u16::try_from(index).ok()?,
        }
        .key()
    };
    let renamed: Vec<(EntityId, EntityId)> = made
        .iter()
        .enumerate()
        .filter(|(_, entity)| entity.key != EntityId(0))
        .filter_map(|(index, entity)| Some((entity.key, key_of(index)?)))
        .collect();
    let rename = |made: EntityId| {
        renamed
            .iter()
            .find(|(named, _)| *named == made)
            .map(|(_, key)| *key)
    };
    let mut failures = 0;
    let mut entities = chunk.entities().to_vec();
    let count = made.len();
    for (index, mut entity) in made.into_iter().enumerate() {
        if !entity.riding.is_free() {
            entity.riding.vehicle = entity.riding.vehicle.and_then(rename);
            entity
                .riding
                .passengers
                .retain_mut(|seat| match rename(seat.key) {
                    Some(key) => {
                        seat.key = key;
                        true
                    }
                    None => false,
                });
        }
        let Ok(index) = u16::try_from(index) else {
            failures += 1;
            tracing::error!(
                identity = %context.identity,
                version = context.version,
                dimension = context.dimension,
                chunk = ?position,
                made = count,
                "a chunk would get more entities of {class} than it has keys for, and gets only the first {MAX_GENERATED_OF_A_CLASS}"
            );
            break;
        };
        let generated = Generated {
            dimension: context.dimension_number,
            chunk: position,
            structure,
            index,
        };
        // A chunk beyond the game's border is never generated by the game, and
        // nothing is made there (section 9 item 8).
        let Some(key) = generated.key() else {
            tracing::warn!(
                chunk = ?position,
                "an entity of a chunk beyond the world's border is not made"
            );
            continue;
        };
        match catch_unwind(AssertUnwindSafe(|| maker.record(key, entity))) {
            Ok(record) => entities.push(record),
            Err(payload) => {
                failures += 1;
                tracing::error!(
                    identity = %context.identity,
                    version = context.version,
                    dimension = context.dimension,
                    chunk = ?position,
                    index,
                    message = %message(&*payload),
                    "the record of an entity of {class} could not be made, and it is left out"
                );
            }
        }
    }
    chunk.set_entities(entities);
    failures
}
