//! What generation is written against (`docs/adr/0021-generation-is-a-function.md`,
//! sections 3 and 4): the view of one feature run, the pieces of structures as a run
//! sees them, and how the store asks for a chunk without waiting for it.
//!
//! It also says how generation makes entities without knowing the simulation that
//! keeps them (`docs/groundwork/n7l-plan.md`, sections 2.1 to 2.3 and 9): a run or a
//! chunk's spawn makes each without its key, through a [`Maker`]'s methods and with the
//! level's [`Draws`]; the scheduler keys it by the chunk it lands in, and the maker then
//! makes its record with that key.
//!
//! The types of structures here are the shape the scheduler needs. G5 reads the
//! port's pieces and may add to them; nothing here decides what a piece is beyond a
//! box and the state it keeps between runs.

use std::fmt;

use clustine_data::BlockState;

use crate::{Biome, BlockEntity, BlockPos, Chunk, ChunkPos, Entity, EntityId, ScheduledTick};

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
    /// What a start does once its pieces in the centre are placed (`afterPlace`),
    /// with the structure's name.
    AfterPlace(StartRef, &'static str),
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

    /// Keeps an entity the run made (`WorldGenRegion.addFreshEntity`), without its key:
    /// it goes into the run's changes of the chunk of its position, and is keyed there
    /// when that chunk is finished (`docs/groundwork/n7l-plan.md`, section 9 item 1).
    /// False if it was refused: outside the three by three, or at a position that is
    /// not a number. Its `key` is not read. A view that keeps no entities, such as a
    /// test's that has no use for them, refuses every one.
    fn add_entity(&mut self, entity: Entity) -> bool {
        let _ = entity;
        false
    }

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

/// What generation draws from the level's random sources for an entity it makes: a
/// mob's `finalizeSpawn`, the spawning of a chunk. The names and meanings are those of
/// the simulation's own stream, so that one hook draws from a player's stream for an
/// egg and from the game's random sources for generation
/// (`docs/groundwork/n7l-plan.md`, section 2.1).
pub trait Draws {
    /// `nextInt(bound)`: from 0 up to `bound`, which is at least 1.
    fn next_int(&mut self, bound: u32) -> u32;
    /// `nextFloat`: from 0 up to 1.
    fn next_float(&mut self) -> f32;
    /// `nextDouble`: from 0 up to 1.
    fn next_double(&mut self) -> f64;
    /// `nextBoolean`.
    fn next_bool(&mut self) -> bool;
}

/// Any of the game's random sources as [`Draws`], each draw the source's own: the
/// spawn's `WorldgenRandom` and the region random of a feature run.
pub struct FromNoise<'a, R: ?Sized>(pub &'a mut R);

impl<R: clustine_noise::Random + ?Sized> Draws for FromNoise<'_, R> {
    /// A bound of 0 is taken as 1, as the simulation's stream takes it; the game's
    /// would throw, and none of its callers passes one. One above `i32::MAX` is taken
    /// as `i32::MAX`, which no caller comes near either.
    fn next_int(&mut self, bound: u32) -> u32 {
        let bound = i32::try_from(bound.max(1)).unwrap_or(i32::MAX);
        // At least 0 below a positive bound.
        self.0.next_i32_bounded(bound) as u32
    }

    fn next_float(&mut self) -> f32 {
        self.0.next_f32()
    }

    fn next_double(&mut self) -> f64 {
        self.0.next_f64()
    }

    fn next_bool(&mut self) -> bool {
        self.0.next_bool()
    }
}

/// What makes the entities of generation, so that the features and the scheduler need
/// not know the simulation that keeps them (`docs/groundwork/n7l-plan.md`, section
/// 2.1): the simulation's maker makes them by its types' own code, and a toy maker
/// serves tests.
///
/// An entity is made in two parts, because its key is known only once the chunk it
/// lands in is finished (section 9 item 1). In the run or the spawn, the maker makes it
/// without its key, drawing what the game draws from the level's random sources
/// ([`mob`](Self::mob), [`template_entity`](Self::template_entity)), and the run hands it
/// to its view ([`Neighbourhood::add_entity`]). When its chunk is finished, the
/// scheduler keys it and the maker makes its record with the key
/// ([`record`](Self::record)), drawing what it draws from its own stream, which is keyed
/// by the key and so independent of the level's sources.
pub trait Maker: Send + Sync {
    /// The record of `entity`, made by a run or a spawn, now that its key is `key`: the
    /// key set, and what the entity draws from its own stream at its making (its UUID,
    /// its type's `construct`) drawn from the stream of `key`. A function of the key
    /// and the entity.
    fn record(&self, key: EntityId, entity: Entity) -> Entity;

    /// A mob of the type `kind` (an index into `clustine_data::ENTITY_TYPES`) that a
    /// structure's code makes at `position` facing `yaw` (the swamp hut's witch, the
    /// monument's elders), finalised for the reason `STRUCTURE` with the level's
    /// `draws`, without its key; `None` for a type this maker does not make, having
    /// drawn nothing.
    fn mob(
        &self,
        kind: u16,
        position: [f64; 3],
        yaw: f32,
        draws: &mut dyn Draws,
    ) -> Option<Entity> {
        let _ = (kind, position, yaw, draws);
        None
    }

    /// An entity of a template, from its tag as the template file has it (a compound
    /// with its `id`, in the binary form of NBT without a name), at `position` facing
    /// `yaw` as the template's transform left it; finalised for the reason `STRUCTURE`
    /// with the level's `draws` where `finalise` and it is a mob. Without its key;
    /// `None` for what this maker does not make, having drawn nothing.
    fn template_entity(
        &self,
        tag: &[u8],
        position: [f64; 3],
        yaw: f32,
        finalise: bool,
        draws: &mut dyn Draws,
    ) -> Option<Entity> {
        let _ = (tag, position, yaw, finalise, draws);
        None
    }
}

/// A maker that only keys what it is handed: for generators that make no entities.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyOnly;

impl Maker for KeyOnly {
    fn record(&self, key: EntityId, mut entity: Entity) -> Entity {
        entity.key = key;
        entity
    }
}

/// The number of a dimension in a generated entity's key ([`crate::Generated`]): the
/// overworld 0, the Nether 1, the End 2, and any other 3.
pub fn dimension_number(name: &str) -> u8 {
    match name {
        "minecraft:overworld" => 0,
        "minecraft:the_nether" => 1,
        "minecraft:the_end" => 2,
        _ => 3,
    }
}

/// The most entities of one class, a structure's or the spawn's, that one chunk gets
/// from generation: the indices of a generated key (`docs/groundwork/n7l-plan.md`,
/// section 2.3). A chunk that would get more gets no more, and is marked "with a
/// failure".
pub const MAX_GENERATED_OF_A_CLASS: usize = 1 << 16;

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

        fn settings(&self) -> GeneratorSettings {
            GeneratorSettings {
                identity: "panics".into(),
                version: 1,
            }
        }
    }

    #[test]
    fn a_noise_source_draws_as_itself_through_the_adapter() {
        use clustine_noise::{LegacyRandom, Random};
        let mut through = LegacyRandom::from_seed(42);
        let mut direct = LegacyRandom::from_seed(42);
        let mut draws = FromNoise(&mut through);
        assert_eq!(draws.next_int(17) as i32, direct.next_i32_bounded(17));
        assert_eq!(draws.next_float(), direct.next_f32());
        assert_eq!(draws.next_double(), direct.next_f64());
        assert_eq!(draws.next_bool(), direct.next_bool());
        // A bound of 0 is taken as 1: a draw, and 0.
        assert_eq!(draws.next_int(0), 0);
        assert_eq!(direct.next_i32_bounded(1), 0);
        assert_eq!(draws.next_int(1000) as i32, direct.next_i32_bounded(1000));
    }

    #[test]
    fn dimensions_have_their_numbers_in_a_key() {
        assert_eq!(dimension_number("minecraft:overworld"), 0);
        assert_eq!(dimension_number("minecraft:the_nether"), 1);
        assert_eq!(dimension_number("minecraft:the_end"), 2);
        assert_eq!(dimension_number("toy:elsewhere"), 3);
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
