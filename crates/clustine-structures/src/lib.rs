//! Where Minecraft's structures start and with which pieces, as a function of a
//! world's seed, a dimension and a chunk: the game's "structure starts" and
//! "structure references" steps, and what they give the terrain to bend the ground
//! by; and the blocks of templates, placed with their processors.
//!
//! The starts place no block: that is the features' stage, which places a piece's
//! template through [`place::place_in_world`], the game's
//! `StructureTemplate.placeInWorld`, with the processors of [`processors`]. The blocks
//! of the templates come from the operator's own jar, which the binary reads and hands
//! to [`template_blocks::TemplateLibrary::from_files`]; nothing here reads a file.
//!
//! - [`Structures::starts`]: the starts of a chunk as the game's
//!   `ChunkGenerator.createStructures` makes them. For every structure set of the
//!   dimension: whether the chunk is one of the set's start chunks ([`placement`]),
//!   which structure of the set is tried (by weight, drawn from the seed and the
//!   chunk), and the start itself with its pieces in the game's order, each with its
//!   type, its box, its orientation and what its kind has beyond that ([`piece`]).
//! - [`Structures::references`]: the starts within eight chunks whose box reaches
//!   into a chunk (`createReferences`).
//! - [`Structures::beardifier`]: what the fill of a chunk adds to the density for
//!   the structures near it, for [`clustine_terrain::fill::Terrain::fill_bent`]: the
//!   ground raised under a village's houses and cleared above them, the hollow of an
//!   ancient city, the burial of a stronghold.
//!
//! All 52 structures of the game start here as in the game: 4,006 starts in 58,864
//! chunks on three seeds, every piece of every one, and every block of 9,875 chunks
//! whose ground they bend (`tests/game_structures.rs`, `tests/game_bent.rs`).
//!
//! The data is the game's, emitted by `cargo datagen` into `clustine-worldgen-data`:
//! the structures, the sets, the template pools, and of every structure template its
//! size and its jigsaw blocks. A template's blocks are not there (ADR-0019): they are
//! read from the operator's jar when the server starts (the terrain plan's question 3).
//!
//! Like the terrain this is a function of its inputs (ADR-0021): no clock, no thread,
//! no hash map, no I/O. The rings of the strongholds are made once for a world, when
//! first asked for, and kept.
//!
//! Much of the code is adapted from SteelMC; see `NOTICE` in this crate, and the head
//! of each file. The judge is the game itself:
//! `crates/clustine-terrain/tests/java/GameStructures.java` has the pinned jar's own
//! generator make the starts without a server, and `tests/game_structures.rs` holds
//! this crate to every one.

mod blocks;
pub mod compat;
pub mod context;
pub mod geometry;
pub mod piece;
pub mod place;
pub mod placement;
pub mod processors;
pub mod reader;
pub mod state_transform;
pub mod template_blocks;
#[cfg(test)]
mod testing;
mod utils;

pub mod desert_pyramid;
pub mod end_city;
pub mod fortress;
pub mod igloo;
pub mod jigsaw;
pub mod jungle_temple;
pub mod mansion;
pub mod mineshaft;
pub mod nether_fossil;
pub mod ocean_monument;
pub mod ocean_ruin;
pub mod ruined_portal;
pub mod shipwreck;
pub mod single_piece;
pub mod stronghold;
pub mod swamp_hut;

use std::sync::OnceLock;

use clustine_noise::random::{LegacyRandom, Random};
use clustine_terrain::beard::{Adjustment, Beardifier, Junction, Rigid};
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::Terrain;
use clustine_worldgen_data::structure::{Kind, Projection, Spread, TerrainAdaptation};
use clustine_worldgen_data::structures::{STRUCTURE_SETS, STRUCTURES};

pub use compat::StructureData;
pub use context::{ColumnBlock, StructureGenerationContext, TerrainContext};
pub use geometry::{BoundingBox, Direction, IVec3, Identifier, Rotation};
pub use piece::{
    ProceduralPieceData, RuinedPortalProperties, StructureBlockIgnore, StructureMirror,
    StructurePiece, StructurePiecePayload, TemplateMarkerHandling, TemplatePieceData,
    TemplatePlacementAdjustment, TemplatePlacementClip, TemplatePostProcess, TemplateProcessorList,
};
pub(crate) use utils::{make_oriented_piece_bounding_box, random_horizontal_direction};

/// What a structure's kind gives for a chunk it may start in: where it stands, and
/// its pieces in the game's order.
pub struct GenerationStub {
    /// The block the start is at, whose biome decided whether it may be.
    pub position: (i32, i32, i32),
    pub pieces: Vec<StructurePiece>,
}

/// The game's `Structure.findValidGenerationPoint` and the making of the pieces
/// after it, for one kind of structure. An implementation draws from `rng` in the
/// game's order and checks the biome itself, where the game checks it.
pub trait Structure: Send + Sync {
    /// `rng` is seeded for the chunk as the game seeds a start's
    /// (`setLargeFeatureSeed`). `None` where the kind does not start here.
    fn find_generation_point(
        &self,
        ctx: &mut dyn StructureGenerationContext,
        structure: &StructureData,
        rng: &mut LegacyRandom,
    ) -> Option<GenerationStub>;
}

/// The code of a structure's kind.
fn implementation(kind: &Kind) -> &'static dyn Structure {
    match kind {
        Kind::Jigsaw(_) => &jigsaw::JigsawStructure,
        Kind::BuriedTreasure => &single_piece::BuriedTreasureStructure,
        Kind::DesertPyramid => &desert_pyramid::DesertPyramidStructure,
        Kind::EndCity => &end_city::EndCityStructure,
        Kind::Fortress => &fortress::NetherFortressStructure,
        Kind::Igloo => &igloo::IglooStructure,
        Kind::JungleTemple => &jungle_temple::JungleTempleStructure,
        Kind::Mineshaft { .. } => &mineshaft::MineshaftStructure,
        Kind::NetherFossil { .. } => &nether_fossil::NetherFossilStructure,
        Kind::OceanMonument => &ocean_monument::OceanMonumentStructure,
        Kind::OceanRuin { .. } => &ocean_ruin::OceanRuinStructure,
        Kind::RuinedPortal { .. } => &ruined_portal::RuinedPortalStructure,
        Kind::Shipwreck { .. } => &shipwreck::ShipwreckStructure,
        Kind::Stronghold => &stronghold::StrongholdStructure,
        Kind::SwampHut => &swamp_hut::SwampHutStructure,
        Kind::WoodlandMansion => &mansion::WoodlandMansionStructure,
    }
}

/// A structure that starts in a chunk (`StructureStart`).
#[derive(Clone, Debug)]
pub struct Start {
    /// The structure, by its place in `clustine_worldgen_data::structures::STRUCTURES`.
    pub structure: usize,
    /// The chunk it starts in.
    pub chunk: (i32, i32),
    /// The pieces in the game's order.
    pub pieces: Vec<StructurePiece>,
    /// The box around all pieces, grown by twelve blocks where the structure bends
    /// the ground (`StructureStart.getBoundingBox`).
    pub bounding_box: BoundingBox,
}

impl Start {
    /// The emitted record of the structure.
    #[must_use]
    pub fn data(&self) -> &'static clustine_worldgen_data::structure::Structure {
        &STRUCTURES[self.structure]
    }
}

/// The box around pieces, grown by `inflate` on every side.
#[must_use]
pub fn bounding_box_of(pieces: &[StructurePiece], inflate: i32) -> Option<BoundingBox> {
    let mut boxes = pieces.iter().map(|piece| piece.bounding_box);
    let first = boxes.next()?;
    Some(
        boxes
            .fold(first, |all, one| BoundingBox::encapsulating(&all, &one))
            .inflate(inflate),
    )
}

/// How far a start can be from a chunk it reaches into, in chunks: the game looks
/// eight away.
pub const REACH: i32 = 8;

/// The order in which fastutil's `LongOpenHashSet` gives the numbers put into it in
/// this order: the game keeps a chunk's references to the starts of one structure in
/// such a set, and the beardifier adds the starts up in the set's order.
#[must_use]
pub fn long_set_order(inserted: &[i64]) -> Vec<i64> {
    fn mix(value: i64) -> u64 {
        let mixed = (value as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mixed = mixed ^ (mixed >> 32);
        mixed ^ (mixed >> 16)
    }
    fn put(table: &mut [i64], value: i64) {
        let mask = table.len() - 1;
        let mut slot = mix(value) as usize & mask;
        while table[slot] != 0 {
            slot = (slot + 1) & mask;
        }
        table[slot] = value;
    }
    // The set starts with room for 16 at three quarters full, and holds zero apart.
    let size_for = |expected: usize| {
        ((expected as f64 / 0.75).ceil() as usize)
            .max(2)
            .next_power_of_two()
    };
    let mut table = vec![0_i64; size_for(16)];
    let mut held = 0;
    let mut has_zero = false;
    for value in inserted {
        if *value == 0 {
            if !has_zero {
                has_zero = true;
                held += 1;
            }
            continue;
        }
        let mask = table.len() - 1;
        let mut slot = mix(*value) as usize & mask;
        let mut present = false;
        while table[slot] != 0 {
            if table[slot] == *value {
                present = true;
                break;
            }
            slot = (slot + 1) & mask;
        }
        if present {
            continue;
        }
        table[slot] = *value;
        let before = held;
        held += 1;
        let max_fill = (table.len() - table.len() / 4).min(table.len() - 1);
        if before >= max_fill {
            // The set grows by putting its numbers into a new table from the last
            // slot down.
            let mut grown = vec![0_i64; size_for(held + 1)];
            for old in table.iter().rev().filter(|old| **old != 0) {
                put(&mut grown, *old);
            }
            table = grown;
        }
    }
    let mut order = Vec::with_capacity(held);
    if has_zero {
        order.push(0);
    }
    order.extend(table.iter().rev().filter(|value| **value != 0));
    order
}

/// The structures of one dimension of a world.
pub struct Structures {
    seed: i64,
    terrain: Terrain,
    biomes: BiomeSource,
    /// The sets some structure of which can stand in a biome of the dimension, by
    /// their place, ascending: the game's `possibleStructureSets`.
    possible: Vec<usize>,
    /// For every set of the game its ring positions; empty for a set without rings.
    rings: OnceLock<Vec<Vec<(i32, i32)>>>,
}

impl Structures {
    /// The structures of a dimension of the world with this seed. It seeds the
    /// dimension's noises twice over, for the ground and for the biomes.
    #[must_use]
    pub fn new(seed: i64, dimension: Dimension) -> Self {
        let biomes = BiomeSource::new(seed, dimension);
        let possible_biomes = biomes.possible_biomes();
        let possible = (0..STRUCTURE_SETS.len())
            .filter(|set| {
                STRUCTURE_SETS[*set]
                    .structures
                    .iter()
                    .any(|(structure, _)| {
                        let allowed = compat::BiomeSet(STRUCTURES[usize::from(*structure)].biomes);
                        possible_biomes.iter().any(|biome| allowed.contains(biome))
                    })
            })
            .collect();
        Self {
            seed,
            terrain: Terrain::new(seed, dimension),
            biomes,
            possible,
            rings: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// The ground the structures ask.
    #[must_use]
    pub fn terrain(&self) -> &Terrain {
        &self.terrain
    }

    /// The biomes the structures ask.
    #[must_use]
    pub fn biomes(&self) -> &BiomeSource {
        &self.biomes
    }

    /// The sets the dimension can have, by their place among the game's, ascending.
    #[must_use]
    pub fn possible_sets(&self) -> &[usize] {
        &self.possible
    }

    /// The chunks of every set that is placed in rings, by the set's place; empty for
    /// the others. Made when first asked for: each position looks for a preferred
    /// biome in 3,249 cells.
    #[must_use]
    pub fn rings(&self) -> &[Vec<(i32, i32)>] {
        self.rings.get_or_init(|| {
            (0..STRUCTURE_SETS.len())
                .map(|set| match STRUCTURE_SETS[set].placement.spread {
                    Spread::ConcentricRings {
                        distance,
                        spread,
                        count,
                        preferred_biomes,
                    } if self.possible.contains(&set) => placement::ring_positions(
                        self.seed,
                        distance,
                        spread,
                        count,
                        preferred_biomes,
                        &self.biomes,
                    ),
                    _ => Vec::new(),
                })
                .collect()
        })
    }

    /// Whether any set of the dimension is placed in rings, which
    /// [`Self::is_structure_chunk`] then needs.
    fn has_rings(&self) -> bool {
        self.possible.iter().any(|set| {
            matches!(
                STRUCTURE_SETS[*set].placement.spread,
                Spread::ConcentricRings { .. }
            )
        })
    }

    fn placements(&self) -> placement::Placements<'_> {
        static NO_RINGS: [Vec<(i32, i32)>; 0] = [];
        placement::Placements {
            seed: self.seed,
            sets: &STRUCTURE_SETS,
            rings: if self.has_rings() {
                self.rings()
            } else {
                &NO_RINGS
            },
        }
    }

    /// Whether a set starts a structure in a chunk, if one of its structures finds a
    /// place there.
    #[must_use]
    pub fn is_structure_chunk(&self, set: usize, chunk_x: i32, chunk_z: i32) -> bool {
        self.placements().is_structure_chunk(set, chunk_x, chunk_z)
    }

    /// The starts of a chunk, in the order of the sets (`createStructures`).
    #[must_use]
    pub fn starts(&self, chunk_x: i32, chunk_z: i32) -> Vec<Start> {
        let placements = self.placements();
        let mut starts = Vec::new();
        for set in &self.possible {
            if !placements.is_structure_chunk(*set, chunk_x, chunk_z) {
                continue;
            }
            let entries = STRUCTURE_SETS[*set].structures;
            if let [(structure, _)] = entries {
                starts.extend(self.start(usize::from(*structure), chunk_x, chunk_z));
                continue;
            }
            // One of the set's structures by weight; where it finds no place, one of
            // the others, until none is left.
            let mut left: Vec<(u16, i32)> = entries.to_vec();
            let mut random = LegacyRandom::from_seed(0);
            random.set_large_feature_seed(self.seed, chunk_x, chunk_z);
            let mut total: i32 = left.iter().map(|(_, weight)| weight).sum();
            while !left.is_empty() {
                let mut drawn = random.next_i32_bounded(total);
                let mut chosen = 0;
                for (_, weight) in &left {
                    drawn -= weight;
                    if drawn < 0 {
                        break;
                    }
                    chosen += 1;
                }
                let (structure, weight) = left[chosen];
                if let Some(start) = self.start(usize::from(structure), chunk_x, chunk_z) {
                    starts.push(start);
                    break;
                }
                left.remove(chosen);
                total -= weight;
            }
        }
        starts
    }

    /// The start of one structure in a chunk, if it finds a place there
    /// (`Structure.generate`), whether or not the chunk is a start chunk of its set.
    #[must_use]
    pub fn start(&self, structure: usize, chunk_x: i32, chunk_z: i32) -> Option<Start> {
        let data = StructureData::of(structure);
        let mut context =
            TerrainContext::new(self.seed, chunk_x, chunk_z, &self.terrain, &self.biomes);
        let mut random = LegacyRandom::from_seed(0);
        random.set_large_feature_seed(self.seed, chunk_x, chunk_z);
        let stub = implementation(&data.data.kind).find_generation_point(
            &mut context,
            &data,
            &mut random,
        )?;
        let bounding_box = bounding_box_of(&stub.pieces, data.bb_inflate())?;
        Some(Start {
            structure,
            chunk: (chunk_x, chunk_z),
            pieces: stub.pieces,
            bounding_box,
        })
    }

    /// What the fill of a chunk adds to the density for the structures near it
    /// (`Beardifier.forStructuresInChunk`): the pieces within twelve blocks of the
    /// chunk of every start the chunk refers to whose structure adapts the terrain,
    /// and the junctions of jigsaw pieces there. `starts_of` gives the starts of a
    /// chunk.
    ///
    /// The game adds the starts up structure by structure in the order of a hash map
    /// of objects, which is not the same from run to run; here the structures come
    /// in the registry's order. Within a structure the starts come in the game's
    /// order ([`long_set_order`]). Where pieces of two structures bend the same
    /// block, the last bit of the sum can therefore differ from one run of the game
    /// to the next, and from here.
    pub fn beardifier<S: std::borrow::Borrow<[Start]>>(
        chunk_x: i32,
        chunk_z: i32,
        mut starts_of: impl FnMut(i32, i32) -> S,
    ) -> Beardifier {
        let references = Self::references(chunk_x, chunk_z, &mut starts_of);
        let mut structures: Vec<usize> =
            references.iter().map(|(structure, _)| *structure).collect();
        structures.sort_unstable();
        structures.dedup();
        let (min_x, min_z) = (chunk_x * 16, chunk_z * 16);
        let mut rigids = Vec::new();
        let mut junctions = Vec::new();
        for structure in structures {
            let adjustment = match STRUCTURES[structure].terrain_adaptation {
                TerrainAdaptation::None => continue,
                TerrainAdaptation::Bury => Adjustment::Bury,
                TerrainAdaptation::BeardThin => Adjustment::BeardThin,
                TerrainAdaptation::BeardBox => Adjustment::BeardBox,
                TerrainAdaptation::Encapsulate => Adjustment::Encapsulate,
            };
            let inserted: Vec<i64> = references
                .iter()
                .filter(|(of, _)| *of == structure)
                .map(|(_, (x, z))| geometry::ChunkPos::new(*x, *z).pack())
                .collect();
            for packed in long_set_order(&inserted) {
                let (x, z) = (packed as i32, (packed >> 32) as i32);
                let starts = starts_of(x, z);
                let Some(start) = starts
                    .borrow()
                    .iter()
                    .find(|start| start.structure == structure)
                else {
                    continue;
                };
                for piece in &start.pieces {
                    let bounds = piece.bounding_box;
                    if !bounds.intersects_xz(
                        min_x - 12,
                        min_z - 12,
                        min_x + 15 + 12,
                        min_z + 15 + 12,
                    ) {
                        continue;
                    }
                    let rigid = |ground_level_delta: i32| Rigid {
                        min: [bounds.min_x(), bounds.min_y(), bounds.min_z()],
                        max: [bounds.max_x(), bounds.max_y(), bounds.max_z()],
                        adjustment,
                        ground_level_delta,
                    };
                    if !matches!(piece.payload, StructurePiecePayload::Jigsaw(_)) {
                        rigids.push(rigid(0));
                        continue;
                    }
                    if piece.projection == Some(Projection::Rigid) {
                        rigids.push(rigid(piece.ground_level_delta));
                    }
                    for junction in &piece.junctions {
                        let (x, z) = (junction.source_pos.x, junction.source_pos.z);
                        if x > min_x - 12
                            && z > min_z - 12
                            && x < min_x + 15 + 12
                            && z < min_z + 15 + 12
                        {
                            junctions.push(Junction {
                                x,
                                ground_y: junction.source_pos.y,
                                z,
                            });
                        }
                    }
                }
            }
        }
        Beardifier::new(rigids, junctions)
    }

    /// The starts a chunk refers to (`createReferences`): for every start within
    /// eight chunks whose box reaches into the chunk's columns, its structure and the
    /// chunk it starts in. `starts_of` gives the starts of a chunk; the pairs come
    /// in the order the game looks: x counting most, then z, then the chunk's starts.
    pub fn references<S: std::borrow::Borrow<[Start]>>(
        chunk_x: i32,
        chunk_z: i32,
        mut starts_of: impl FnMut(i32, i32) -> S,
    ) -> Vec<(usize, (i32, i32))> {
        let (min_x, min_z) = (chunk_x * 16, chunk_z * 16);
        let mut references = Vec::new();
        for x in chunk_x - REACH..=chunk_x + REACH {
            for z in chunk_z - REACH..=chunk_z + REACH {
                for start in starts_of(x, z).borrow() {
                    if start
                        .bounding_box
                        .intersects_xz(min_x, min_z, min_x + 15, min_z + 15)
                    {
                        references.push((start.structure, (x, z)));
                    }
                }
            }
        }
        references
    }
}
