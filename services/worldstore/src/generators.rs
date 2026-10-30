//! What makes the chunks nobody has stored, in each of the world's three dimensions
//! (`docs/groundwork/p-plan.md`, section 3.3): one generator per dimension, built from
//! the world's seed, each of which a chunk of its dimension is asked of.
//!
//! The world's generator identity names all three, so that a world is continued only by
//! the generators it was made with in every dimension: its land would meet land of
//! another generator in a seam otherwise (`docs/adr/0021-generation-is-a-function.md`,
//! section 8). A world made before there were three is refused, as the owner agreed
//! (the plan, question 3).

use std::sync::Arc;

use clustine_data::DimensionType;
use clustine_world::generation::GeneratorSettings;
use clustine_world::{Biome, Chunk, ChunkGenerator, ChunkPos, Dimension};

/// What sets the identities of the three generators apart in the world's: none of them
/// has it in its own.
const SEPARATOR: &str = " | ";

/// The world's generators, one per dimension.
#[derive(Clone)]
pub struct Generators {
    pub overworld: Arc<dyn ChunkGenerator>,
    pub nether: Arc<dyn ChunkGenerator>,
    pub end: Arc<dyn ChunkGenerator>,
}

impl Generators {
    pub fn new(
        overworld: Arc<dyn ChunkGenerator>,
        nether: Arc<dyn ChunkGenerator>,
        end: Arc<dyn ChunkGenerator>,
    ) -> Self {
        Self {
            overworld,
            nether,
            end,
        }
    }

    /// A world whose overworld `overworld` makes and whose Nether and End are air
    /// ([`Void`]): for tests and tools that use the overworld alone, which need no
    /// generator of the others. A server's world has the game's
    /// ([`Generators::new`]), and is of another identity, so neither continues the
    /// other's world.
    pub fn overworld_alone(overworld: Arc<dyn ChunkGenerator>) -> Self {
        Self {
            overworld,
            nether: Arc::new(Void::of(Dimension::Nether)),
            end: Arc::new(Void::of(Dimension::End)),
        }
    }

    /// The generator of `dimension`.
    pub fn of(&self, dimension: Dimension) -> &Arc<dyn ChunkGenerator> {
        match dimension {
            Dimension::Overworld => &self.overworld,
            Dimension::Nether => &self.nether,
            Dimension::End => &self.end,
        }
    }

    /// The world's generator identity: the three generators' identities, in the order
    /// of the dimensions' numbers, the overworld's first.
    pub fn identity(&self) -> String {
        Dimension::ALL
            .map(|dimension| self.of(dimension).settings().identity)
            .join(SEPARATOR)
    }

    /// The version of each dimension's generator, in the order of the dimensions'
    /// numbers.
    pub fn versions(&self) -> [u16; 3] {
        Dimension::ALL.map(|dimension| self.of(dimension).settings().version)
    }
}

impl From<Arc<dyn ChunkGenerator>> for Generators {
    /// [`Generators::overworld_alone`].
    fn from(overworld: Arc<dyn ChunkGenerator>) -> Self {
        Self::overworld_alone(overworld)
    }
}

impl<G: ChunkGenerator + 'static> From<Arc<G>> for Generators {
    /// [`Generators::overworld_alone`].
    fn from(overworld: Arc<G>) -> Self {
        Self::overworld_alone(overworld)
    }
}

/// The overworld's part of a world's generator identity ([`Generators::identity`]).
pub(crate) fn overworld_identity(identity: &str) -> &str {
    identity
        .split(SEPARATOR)
        .next()
        .expect("a split yields at least one part")
}

/// The version of [`Void`]: what a chunk says it is exactly as made by.
const VOID_VERSION: u16 = 1;

/// A dimension of air: every chunk as empty as the dimension's type makes a chunk,
/// lit by nothing. For a world whose dimension nobody enters
/// ([`Generators::overworld_alone`]); its identity says so, so that no world of the
/// game's land is taken for one of these.
#[derive(Debug)]
pub struct Void {
    dimension: Dimension,
    made: Chunk,
}

impl Void {
    pub fn of(dimension: Dimension) -> Self {
        let kind: &DimensionType = dimension.rules();
        let mut made = Chunk::empty(kind, Biome(0));
        made.mark_generated(VOID_VERSION, false);
        Self { dimension, made }
    }
}

impl ChunkGenerator for Void {
    fn generate(&self, _position: ChunkPos) -> Chunk {
        self.made.clone()
    }

    fn settings(&self) -> GeneratorSettings {
        GeneratorSettings {
            identity: format!("void;{}", self.dimension.type_name()),
            version: VOID_VERSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen::FlatGenerator;

    use super::*;

    #[test]
    fn the_world_s_identity_names_the_three_generators_the_overworld_s_first() {
        let flat: Arc<dyn ChunkGenerator> = Arc::new(FlatGenerator::classic());
        let generators = Generators::overworld_alone(Arc::clone(&flat));
        let identity = generators.identity();
        let flat_identity = flat.settings().identity;
        assert_eq!(
            identity,
            format!("{flat_identity} | void;minecraft:the_nether | void;minecraft:the_end")
        );
        assert_eq!(overworld_identity(&identity), flat_identity);
        assert_eq!(overworld_identity(&flat_identity), flat_identity);
        assert_eq!(generators.versions(), [flat.settings().version, 1, 1]);
        // Another generator of one dimension is another world.
        let other = Generators::new(
            Arc::clone(&flat),
            Arc::new(Void::of(Dimension::End)),
            Arc::new(Void::of(Dimension::End)),
        );
        assert_ne!(other.identity(), identity);
    }

    #[test]
    fn a_chunk_is_asked_of_its_dimension_s_generator() {
        let flat: Arc<dyn ChunkGenerator> = Arc::new(FlatGenerator::classic());
        let generators = Generators::from(flat);
        let origin = ChunkPos::new(0, 0);
        let nether = generators.of(Dimension::Nether).generate(origin);
        let (lowest, sections) = Dimension::Nether.sections();
        assert_eq!(nether.sections().len(), sections);
        assert_eq!(nether.min_y(), lowest * 16);
        assert_eq!(
            nether.get(0, lowest * 16, 0),
            Some(clustine_data::blocks::AIR)
        );
        assert_eq!(nether.generated(), VOID_VERSION);
        let overworld = generators.of(Dimension::Overworld).generate(origin);
        assert_eq!(
            overworld.get(0, -64, 0),
            Some(clustine_data::blocks::BEDROCK)
        );
    }
}
