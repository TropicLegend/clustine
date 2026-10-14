//! What generation is written against (`docs/adr/0021-generation-is-a-function.md`,
//! sections 3 and 4): the view of one feature run, the pieces of structures as a run
//! sees them, and how the store asks for a chunk without waiting for it.
//!
//! The types of structures here are the shape the scheduler needs. G5 reads the
//! port's pieces and may add to them; nothing here decides what a piece is beyond a
//! box and the state it keeps between runs.

use std::fmt;

use clustine_data::BlockState;

use crate::{Biome, BlockEntity, BlockPos, Chunk, ChunkPos, ScheduledTick};

/// One of the four heightmaps a finished chunk has; a run reads them as the blocks
/// are now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FinalHeightmap {
    WorldSurface,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

/// One of the two heightmaps terrain leaves, which features read frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorldgenHeightmap {
    WorldSurface,
    OceanFloor,
}

/// A box of blocks, both corners included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockBox {
    pub min: BlockPos,
    pub max: BlockPos,
}

impl BlockBox {
    /// Whether the box has a block in the chunk column `chunk`.
    pub fn meets(&self, chunk: ChunkPos) -> bool {
        let (min, max) = (self.min.chunk(), self.max.chunk());
        (min.x..=max.x).contains(&chunk.x) && (min.z..=max.z).contains(&chunk.z)
    }
}

/// A structure start, by the chunk it starts in and its place among that chunk's
/// starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StartRef {
    pub source: ChunkPos,
    pub index: u16,
}

/// A piece of a structure start: the start and the piece's place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PieceKey {
    pub start: StartRef,
    pub piece: u16,
}

/// What a piece keeps between runs (ADR-0021, section 3: a guess that G5 confirms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PieceState {
    /// How far the piece's box has been moved in y since it was made.
    pub shift_y: i32,
    /// Whether the piece's height has been settled.
    pub settled: bool,
    pub flags: u16,
}

/// What a run is placing, for the recorder and for a failure's record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placing {
    /// A placed feature, by its name.
    Feature(&'static str),
    /// A piece of a structure start, with the structure's name.
    Piece(PieceKey, &'static str),
}

/// What one feature run sees and changes: the three by three chunks around
/// `centre()` as the runs before it left them, and what was fixed before features
/// around that. Section 3 of ADR-0021 says what each read is answered with and what
/// is recorded. Reads take `&mut self` because the recorder counts them.
pub trait Neighbourhood {
    fn centre(&self) -> ChunkPos;

    // Live: the three by three.
    fn block(&mut self, at: BlockPos) -> BlockState;
    /// False if the write was refused: outside the three by three or the height.
    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool;
    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity>;
    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool;
    /// One of the four final heightmaps, as the blocks are now: the y above the
    /// highest block that counts.
    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32;

    // Frozen: terrain's, five by five.
    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32;
    fn biome(&mut self, at: BlockPos) -> Biome;

    // What generation leaves for later (ADR-0022, phase B). The tick's `at` is taken
    // in the chunk that `at` lies in.
    fn schedule(&mut self, at: BlockPos, tick: ScheduledTick);
    fn mark(&mut self, at: BlockPos);

    // Structures.
    /// The starts that the centre's references name, as made.
    fn starts(&self) -> &[StartRef];
    fn piece(&mut self, piece: PieceKey) -> PieceState;
    fn set_piece(&mut self, piece: PieceKey, state: PieceState);

    /// Places one thing: a placed feature, or one piece of a start. Everything
    /// `place` does to the view is kept if it returns, and undone if it panics.
    /// Returns whether it was kept. Attempts do not nest.
    fn attempt(&mut self, what: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool;
}

/// What decides what a generator produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorSettings {
    /// Everything that decides what is produced, but the version.
    pub identity: String,
    /// Raised whenever a change makes any chunk come out otherwise (ADR-0021,
    /// section 8).
    pub version: u16,
}

/// Generation failed for this chunk. The generator has logged why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed;

/// What is called with a requested chunk, once, on any thread.
pub type Done = Box<dyn FnOnce(Result<Chunk, Failed>) + Send>;

/// A request that is under way. Dropping it says that nobody waits any more.
pub struct Wanted(Option<Box<dyn FnOnce() + Send>>);

impl Wanted {
    /// A request that nothing has to be told about when it is dropped.
    pub fn nothing() -> Self {
        Self(None)
    }

    /// A request that calls `forget` when it is dropped.
    pub fn new(forget: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(forget)))
    }
}

impl Drop for Wanted {
    fn drop(&mut self) {
        if let Some(forget) = self.0.take() {
            forget();
        }
    }
}

impl fmt::Debug for Wanted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Wanted").field(&self.0.is_some()).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::ChunkGenerator;

    struct Panics;

    impl ChunkGenerator for Panics {
        fn generate(&self, _: ChunkPos) -> Chunk {
            panic!("this generator always fails");
        }

        fn settings(&self) -> String {
            "panics".into()
        }
    }

    #[test]
    fn a_dropped_request_says_so_once() {
        let told = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&told);
        drop(Wanted::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }));
        drop(Wanted::nothing());
        assert_eq!(told.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_generator_that_panics_is_answered_failed_by_the_provided_request() {
        let answer = Arc::new(std::sync::Mutex::new(None));
        let kept = Arc::clone(&answer);
        let _wanted = Panics.request(
            ChunkPos::new(3, -4),
            Box::new(move |made| *kept.lock().unwrap() = Some(made.err())),
        );
        assert_eq!(*answer.lock().unwrap(), Some(Some(Failed)));
    }
}
