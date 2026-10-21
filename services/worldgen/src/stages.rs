//! The stages of generation as functions (`docs/adr/0021-generation-is-a-function.md`,
//! sections 1 and 4): what the scheduler is written against, and what the real
//! generator and the tests' toy generators implement.

use clustine_data::DimensionType;
use clustine_world::generation::{
    BlockBox, GeneratorSettings, KeyOnly, Maker, Neighbourhood, PieceState, StartRef,
};
use clustine_world::{COLUMNS_PER_CHUNK, Chunk, ChunkPos, Entity};

/// A generator's five stages. Every one is a function of the settings and of what it
/// is handed: no clock, no thread, no earlier call.
pub trait Stages: Send + Sync + 'static {
    fn settings(&self) -> GeneratorSettings;
    fn dimension(&self) -> &'static DimensionType;
    /// [`ChunkGenerator::origin`](clustine_world::ChunkGenerator::origin) of the chunks
    /// the stages make.
    fn origin(&self) -> ChunkPos {
        ChunkPos::new(0, 0)
    }
    /// The structure starts whose source chunk is `source`, as made.
    fn starts(&self, source: ChunkPos) -> Starts;
    /// The chunk after carving. `starts` answers for every source chunk within 8.
    fn terrain(&self, position: ChunkPos, starts: &dyn StartsNear) -> Terrain;
    /// One feature run: structures and features of `centre`, in the game's steps,
    /// each placement in an `attempt` of the view.
    fn decorate(&self, view: &mut dyn Neighbourhood, centre: ChunkPos);
    /// The animals of a chunk, as the game's `spawn` step makes them after its light
    /// (`docs/groundwork/n7l-plan.md`, section 2.1): from the lit chunk at `position`
    /// and the nine finished chunks around it, in the order of `lattice::around` (the
    /// centre fifth). Each without its key, which is the scheduler's to give; one that
    /// is not in the chunk at `position` is refused. None by default.
    fn spawn(&self, position: ChunkPos, lit: &Chunk, nine: [&Chunk; 9]) -> Vec<Entity> {
        let _ = (position, lit, nine);
        Vec::new()
    }
    /// What makes the records of the entities the runs and the spawn make, once they
    /// are keyed, and what the runs make them with: by default a maker that only keys
    /// them, for stages that make none.
    fn maker(&self) -> &dyn Maker {
        &KeyOnly
    }
}

/// The starts of the source chunks around a chunk whose terrain is made.
pub trait StartsNear {
    /// The starts of `source`, which is within 8 of the chunk; panics beyond.
    fn starts(&self, source: ChunkPos) -> &Starts;
}

/// The structure starts of one source chunk, in the order the game makes them; a
/// [`StartRef`]'s index is a place in `starts`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Starts {
    pub starts: Vec<Start>,
}

/// One structure start as made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The structure's name, such as `minecraft:desert_pyramid`.
    pub structure: &'static str,
    pub pieces: Vec<PieceMade>,
}

/// One piece of a start as made: its box and its state before any run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PieceMade {
    pub bounds: BlockBox,
    pub state: PieceState,
}

/// A chunk after carving: its blocks and biomes, the two heightmaps features read
/// frozen, and the starts within 8 whose box meets it.
#[derive(Debug, Clone)]
pub struct Terrain {
    /// The blocks and biomes. Nothing else of the chunk is read: no light, no
    /// block entities, no ticks or marks.
    pub chunk: Chunk,
    pub worldgen_heights: Box<WorldgenHeights>,
    pub references: Vec<StartRef>,
}

/// For each column, indexed by `z << 4 | x`, the height above the highest block that
/// counts for the map, counted from the bottom of the dimension, as
/// [`clustine_world::Heights`] counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldgenHeights {
    /// What is not air.
    pub world_surface: [u16; COLUMNS_PER_CHUNK],
    /// What blocks motion.
    pub ocean_floor: [u16; COLUMNS_PER_CHUNK],
}
