//! A chunk column: a stack of sections and what else a chunk carries
//! (`docs/adr/0022-what-a-chunk-carries.md`).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use clustine_data::{BLOCK_INFO, BLOCK_STATE_COUNT, BlockState, DimensionType, blocks};
use serde::{Deserialize, Serialize};

use crate::generation::{Done, Failed, GeneratorSettings, Wanted};
use crate::light::{Light, SectionLight};
use crate::poi::{Claim, PoiType, poi_type_of};
use crate::raid::Raid;
use crate::{
    Biome, BlockPos, COLUMNS_PER_CHUNK, ChunkPos, Entity, EntityChange, EntityId, SECTION_SIZE,
    Section,
};

/// The most data one block entity may have, in bytes, and the most all of a chunk's
/// may have together. Data is refused where it comes in, so that no chunk can be made
/// that a link or the store's reader would refuse for its size.
pub const MAX_BLOCK_ENTITY_DATA: usize = 16 * 1024;
pub const MAX_CHUNK_ENTITY_DATA: usize = 1024 * 1024;

/// A position in a chunk: `x` and `z` in `0..16`, and the world height `y`. Ordered by
/// y, then z, then x.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InChunk {
    pub y: i16,
    pub z: u8,
    pub x: u8,
}

impl InChunk {
    /// The position at `x` and `z` (each in `0..16`) and world height `y`.
    pub fn new(x: usize, y: i32, z: usize) -> Self {
        assert!(
            x < SECTION_SIZE && z < SECTION_SIZE,
            "({x}, {z}) is outside a chunk"
        );
        Self {
            y: i16::try_from(y).expect("no dimension is that high or deep"),
            z: z as u8,
            x: x as u8,
        }
    }
}

/// What a block keeps beside its state: its type, an index into
/// `clustine_data::BLOCK_ENTITY_TYPES`, and its data, which is the bytes of one NBT
/// compound as the game writes it on the network, without the keys `x`, `y`, `z` and
/// `id`. Nothing here looks into the data. No data stands for an empty compound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEntity {
    pub kind: u8,
    pub data: Vec<u8>,
}

/// Why a block entity's data was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRefused {
    /// No block entity is at the position.
    NoBlockEntity,
    /// More than [`MAX_BLOCK_ENTITY_DATA`] for one block entity.
    TooMuchForOne,
    /// More than [`MAX_CHUNK_ENTITY_DATA`] for the chunk's block entities together.
    TooMuchForTheChunk,
}

/// What a scheduled tick is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Ticked {
    /// A block, by its id.
    Block(u16),
    /// A fluid, by its id.
    Fluid(u8),
}

/// A tick that generation scheduled for a block or a fluid, as the official server
/// keeps them with a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledTick {
    pub at: InChunk,
    pub of: Ticked,
    pub delay: i32,
    pub priority: i8,
}

/// What kind of update a [`Seam`] keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SeamKind {
    /// `updateShape`: the block asks again what it is, beside the block that changed.
    Shape,
    /// `neighborChanged`: the block is told that its neighbour changed.
    Neighbour,
}

/// A block event that outlived the tick it was queued in (`docs/adr/0026-redstone.md`,
/// section 4): its block's chunk did not tick, or it was queued after the phase of
/// block events. Kept in the chunk of its block, in the order of the region's queue,
/// as the game's `BlockEventData`: the block it is of, and its two numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptEvent {
    pub at: InChunk,
    /// The block's id, which the block there has to be for the event to run.
    pub block: u16,
    /// The game's `b0`, which says what the event is (a piston's 0 to extend, say).
    pub kind: i32,
    /// The game's `b1`, the event's number (a piston's direction, say).
    pub data: i32,
}

/// An update a region could not make: one that would reach a block whose chunk, or a
/// neighbour's, it had not loaded, as at the border with another region. It is kept in
/// the chunk of the block that changed, and made when one region has the block it
/// reaches and that block's neighbours loaded
/// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seam {
    /// The block the update reaches.
    pub at: BlockPos,
    /// The direction from that block to the one that changed, by `Face`'s numbers.
    pub direction: u8,
    pub kind: SeamKind,
    /// The block that changed, as it was then.
    pub source: BlockState,
    /// The update's flags (`Block.UPDATE_*`).
    pub flags: u32,
    /// How much deeper a chain of shape updates may go from it.
    pub depth: i32,
}

impl Seam {
    /// What tells two entries apart: a chunk keeps one per block reached, direction and
    /// kind, the latest.
    pub fn key(&self) -> (BlockPos, u8, SeamKind) {
        (self.at, self.direction, self.kind)
    }
}

/// The structures whose starts a chunk keeps (`docs/groundwork/n9-plan.md`, section
/// 9.6): those with spawn overrides, which natural spawning asks for its lists
/// (`ChunkGenerator.getMobsAt`), and the villages, which the patrol spawner asks in
/// place of the points of interest Clustine does not have yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum KeptKind {
    SwampHut,
    Monument,
    PillagerOutpost,
    AncientCity,
    TrialChambers,
    /// The Nether's, which no overworld chunk has.
    Fortress,
    VillagePlains,
    VillageDesert,
    VillageSavanna,
    VillageSnowy,
    VillageTaiga,
}

/// Which box of a start a structure's spawn overrides ask whether a position is in
/// (`StructureSpawnOverride$BoundingBoxType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideBox {
    /// The start's box.
    Full,
    /// The box of any one of its pieces.
    Piece,
}

impl KeptKind {
    /// Every kind, in the order of the enum.
    pub const ALL: [Self; 11] = [
        Self::SwampHut,
        Self::Monument,
        Self::PillagerOutpost,
        Self::AncientCity,
        Self::TrialChambers,
        Self::Fortress,
        Self::VillagePlains,
        Self::VillageDesert,
        Self::VillageSavanna,
        Self::VillageSnowy,
        Self::VillageTaiga,
    ];

    /// The structure's name in the game's registry.
    pub fn name(self) -> &'static str {
        match self {
            Self::SwampHut => "minecraft:swamp_hut",
            Self::Monument => "minecraft:monument",
            Self::PillagerOutpost => "minecraft:pillager_outpost",
            Self::AncientCity => "minecraft:ancient_city",
            Self::TrialChambers => "minecraft:trial_chambers",
            Self::Fortress => "minecraft:fortress",
            Self::VillagePlains => "minecraft:village_plains",
            Self::VillageDesert => "minecraft:village_desert",
            Self::VillageSavanna => "minecraft:village_savanna",
            Self::VillageSnowy => "minecraft:village_snowy",
            Self::VillageTaiga => "minecraft:village_taiga",
        }
    }

    /// The kind of the structure named `name`, if a chunk keeps its starts.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Which box the structure's spawn overrides ask, as its data says (every override
    /// of one structure asks the same); `None` for a village, which has none.
    pub fn overrides(self) -> Option<OverrideBox> {
        match self {
            Self::Monument | Self::PillagerOutpost | Self::AncientCity => Some(OverrideBox::Full),
            Self::SwampHut | Self::TrialChambers | Self::Fortress => Some(OverrideBox::Piece),
            Self::VillagePlains
            | Self::VillageDesert
            | Self::VillageSavanna
            | Self::VillageSnowy
            | Self::VillageTaiga => None,
        }
    }

    pub fn is_village(self) -> bool {
        self.overrides().is_none()
    }
}

/// A box of blocks, both corners included, as the game's `BoundingBox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct KeptBox {
    pub min: BlockPos,
    pub max: BlockPos,
}

impl KeptBox {
    /// `BoundingBox.isInside`.
    pub fn holds(&self, at: BlockPos) -> bool {
        (self.min.x..=self.max.x).contains(&at.x)
            && (self.min.y..=self.max.y).contains(&at.y)
            && (self.min.z..=self.max.z).contains(&at.z)
    }

    /// The box grown by `by` on every side (`BoundingBox.inflatedBy`).
    pub fn widened(&self, by: i32) -> Self {
        Self {
            min: BlockPos::new(self.min.x - by, self.min.y - by, self.min.z - by),
            max: BlockPos::new(self.max.x + by, self.max.y + by, self.max.z + by),
        }
    }

    /// Whether the box has a block in the chunk column `chunk`.
    pub fn meets(&self, chunk: ChunkPos) -> bool {
        let (min, max) = (self.min.chunk(), self.max.chunk());
        (min.x..=max.x).contains(&chunk.x) && (min.z..=max.z).contains(&chunk.z)
    }
}

/// A structure start a chunk keeps: one whose box reaches the chunk, of a structure
/// [`KeptKind`] names (`docs/groundwork/n9-plan.md`, section 9.6). Generation writes
/// it and nothing changes it after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptStructure {
    pub kind: KeptKind,
    /// The chunk the start is in, which tells two starts of one structure apart.
    pub source: ChunkPos,
    /// The start's box, as the game reads it for a `full` override
    /// (`StructureStart.getBoundingBox`: around its pieces as made, grown by twelve for a
    /// structure that adapts the terrain).
    pub bounds: KeptBox,
    /// The boxes of its pieces that reach the chunk, as they stand once placed, in the
    /// start's order: for a `piece` override and for a village; none for a structure
    /// whose overrides ask the start's box, which nothing reads them for.
    pub pieces: Vec<KeptBox>,
}

/// The four heightmaps a client is sent and generation asks: for each column, indexed
/// by `z << 4 | x`, the height above the highest block that counts for the map,
/// counted from the bottom of the dimension; 0 for a column without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heights {
    /// What is not air.
    pub world_surface: [u16; COLUMNS_PER_CHUNK],
    /// What blocks motion.
    pub ocean_floor: [u16; COLUMNS_PER_CHUNK],
    /// What blocks motion or has a fluid.
    pub motion_blocking: [u16; COLUMNS_PER_CHUNK],
    /// The same without leaves.
    pub motion_blocking_no_leaves: [u16; COLUMNS_PER_CHUNK],
}

const MAPS: usize = 4;

impl Heights {
    fn none() -> Box<Self> {
        Box::new(Self {
            world_surface: [0; COLUMNS_PER_CHUNK],
            ocean_floor: [0; COLUMNS_PER_CHUNK],
            motion_blocking: [0; COLUMNS_PER_CHUNK],
            motion_blocking_no_leaves: [0; COLUMNS_PER_CHUNK],
        })
    }

    fn maps(&mut self) -> [&mut [u16; COLUMNS_PER_CHUNK]; MAPS] {
        [
            &mut self.world_surface,
            &mut self.ocean_floor,
            &mut self.motion_blocking,
            &mut self.motion_blocking_no_leaves,
        ]
    }
}

/// Whether `state` is one of the game's, so that the table has a row for it. A chunk
/// can be handed any number, and one that is no state's counts for nothing.
fn known(state: BlockState) -> bool {
    u32::from(state.0) < BLOCK_STATE_COUNT
}

/// For each heightmap, in the order of [`Heights::maps`], whether `state` counts.
fn counts_for(state: BlockState) -> [bool; MAPS] {
    if !known(state) {
        return [false; MAPS];
    }
    let props = state.props();
    let fluid = props.fluid().is_some();
    [
        !props.is_air(),
        props.in_motion_heightmap(),
        props.in_motion_heightmap() || fluid,
        props.in_motion_heightmap_no_leaves() || fluid,
    ]
}

/// The type of the block entity a block of `state` keeps, if it keeps one.
fn block_entity_type(state: BlockState) -> Option<u8> {
    if !known(state) || !state.props().has_block_entity() {
        return None;
    }
    BLOCK_INFO[state.block_id()?].block_entity_type
}

/// Whether light could tell the two states apart: by the light they give or take, by
/// whether and with which faces they stop it, by whether the sky passes down through
/// them, or by whether they are air. A state that is not the game's is taken to
/// differ from every other.
///
/// Whether a state is air is among it because which light sections a chunk and its
/// neighbours list turns on which sections hold a block that is not air. A chunk
/// drops its light by this at a change, and an edge asks it too.
pub fn light_tells_apart(old: BlockState, new: BlockState) -> bool {
    if !known(old) || !known(new) {
        return true;
    }
    let (old, new) = (old.props(), new.props());
    old.is_air() != new.is_air()
        || old.light_emission() != new.light_emission()
        || old.light_dampening() != new.light_dampening()
        || old.occludes() != new.occludes()
        || old.occludes_by_shape() != new.occludes_by_shape()
        || old.occlusion_face_set() != new.occlusion_face_set()
        || old.propagates_skylight_down() != new.propagates_skylight_down()
}

/// A 16×16 column of the world from the bottom of its dimension to the top.
///
/// It keeps three things true of itself: light is there only if no change that light
/// can see was made since it was made, and the failure bit is set only where the
/// chunk is as generated; a position has a block entity exactly if its block keeps
/// one, of that type; and the counts and heightmaps are those of the blocks.
///
/// Two chunks are equal if everything they carry is: the blocks and biomes by content,
/// the block entities, the generator's version and the failure bit, the light, the
/// ticks, the marks, the seams, the block events, the structures and the entities. [`Chunk::shows_the_same`] asks only what a client is shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "ChunkRepr", into = "ChunkRepr")]
pub struct Chunk {
    /// The y coordinate of the lowest block.
    min_y: i32,
    /// From bottom to top.
    sections: Vec<Section>,
    block_entities: BTreeMap<InChunk, BlockEntity>,
    heights: Box<Heights>,
    /// The version of the generator this chunk is exactly as made by; 0 otherwise.
    generated: u16,
    /// Something failed while it was made. Only with `generated != 0`.
    with_failure: bool,
    light: Option<Light>,
    /// In the order generation scheduled them.
    ticks: Vec<ScheduledTick>,
    /// Positions whose block is to take its shape, or whose fluid is to flow a step,
    /// when the chunk is first ticked. Ascending, each once.
    marks: Vec<InChunk>,
    /// Updates from this chunk's blocks that its region could not make, by
    /// [`Seam::key`], each once.
    seams: Vec<Seam>,
    /// The block events of this chunk's blocks that outlived their tick, in the order
    /// of the queue they wait in.
    events: Vec<KeptEvent>,
    /// The structure starts that reach the chunk which natural spawning and the patrol
    /// spawner ask, in generation's order (`docs/groundwork/n9-plan.md`, section 9.6).
    structures: Vec<KeptStructure>,
    /// The points of interest of the chunk's blocks whose tickets are not all free,
    /// ascending by position, each of its block's type (`docs/groundwork/n10-plan.md`,
    /// section 3.2).
    claims: Vec<Claim>,
    /// The raids kept in this chunk, ascending by id (`docs/groundwork/n10-plan.md`,
    /// section 9.1). A copy has them as its base had them and is never told of a change:
    /// a region reads other regions' raids from the published table, not from copies.
    raids: Vec<Raid>,
    /// The entities other than players in the chunk, ascending by key
    /// (`docs/adr/0028-entities.md`, section 2).
    entities: Vec<Entity>,
    /// Where light is kept up to date beside the chunk (a region's, ADR-0029): the
    /// positions whose change light could see since they were last taken; `None` where
    /// nobody asked. Not serialised, and no part of the
    /// chunk's equality: every write to the blocks passes [`Chunk::set`], so none can go
    /// round it (`docs/groundwork/n8-plan.md`, section 4.2).
    light_noted: Option<Vec<InChunk>>,
    /// The points of interest of the blocks, ascending by position, worked out when first
    /// asked after a change (`docs/groundwork/n10-plan.md`, section 3.3). Not serialised,
    /// and no part of the chunk's equality: it is what the blocks say, and every write to
    /// them passes [`Chunk::set`], which forgets it.
    pois: OnceLock<Box<[(InChunk, PoiType)]>>,
}

impl PartialEq for Chunk {
    fn eq(&self, other: &Self) -> bool {
        self.shows_the_same(other)
            && self.generated == other.generated
            && self.with_failure == other.with_failure
            && self.light == other.light
            && self.ticks == other.ticks
            && self.marks == other.marks
            && self.seams == other.seams
            && self.events == other.events
            && self.structures == other.structures
            && self.claims == other.claims
            && self.raids == other.raids
            && self.entities == other.entities
    }
}

impl Eq for Chunk {}

impl Chunk {
    /// An empty chunk column for a dimension of the given type.
    pub fn empty(dimension: &DimensionType, biome: Biome) -> Self {
        let section_count = dimension.height as usize / SECTION_SIZE;
        let sections = vec![Section::filled(blocks::AIR, biome); section_count];
        Self::from_sections(dimension.min_y, sections)
    }

    /// A chunk column of the given sections, from bottom to top, whose lowest block is at
    /// `min_y`. It has a block entity without data wherever a block keeps one, is not
    /// as generated, and has no light, ticks or marks.
    pub fn from_sections(min_y: i32, sections: Vec<Section>) -> Self {
        let mut chunk = Self {
            min_y,
            sections,
            block_entities: BTreeMap::new(),
            heights: Heights::none(),
            generated: 0,
            with_failure: false,
            light: None,
            ticks: Vec::new(),
            marks: Vec::new(),
            seams: Vec::new(),
            events: Vec::new(),
            structures: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
            entities: Vec::new(),
            light_noted: None,
            pois: OnceLock::new(),
        };
        chunk.heights = chunk.scanned_heights();
        chunk.block_entities = chunk.owed_block_entities(BTreeMap::new());
        chunk
    }

    /// The y coordinate of the lowest block.
    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// The number of blocks from the bottom of the column to its top.
    pub fn height(&self) -> usize {
        self.sections.len() * SECTION_SIZE
    }

    /// The sections from bottom to top.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// The block at `x` and `z` (each in `0..16`) and world height `y`, or `None` if `y`
    /// is below or above the dimension.
    pub fn get(&self, x: usize, y: i32, z: usize) -> Option<BlockState> {
        let (section, y) = self.locate(y)?;
        Some(self.sections[section].get(x, y, z))
    }

    /// Replaces the block at `x` and `z` (each in `0..16`) and world height `y`. Returns
    /// the old block, or `None` without changing anything if `y` is outside the dimension.
    ///
    /// Where the state changes, everything that follows the blocks follows: the
    /// heights of the column; the block entity of the position, which goes if the new
    /// block keeps none or one of another type, and is there without data if it keeps
    /// one; the chunk is no longer as generated; and its light is dropped if light
    /// could tell the two states apart. Ticks and marks are not touched.
    pub fn set(&mut self, x: usize, y: i32, z: usize, state: BlockState) -> Option<BlockState> {
        let (section, in_section) = self.locate(y)?;
        let old = self.sections[section].set(x, in_section, z, state);
        if old == state {
            return Some(old);
        }
        self.follow_heights(x, y, z, old, state);
        let at = InChunk::new(x, y, z);
        if poi_type_of(old) != poi_type_of(state) {
            self.pois = OnceLock::new();
        }
        // A record whose block changes type is gone, and its claim with it; nobody is
        // told (`PoiSection.remove`), its claimants find out by looking.
        if poi_type_of(old) != poi_type_of(state)
            && let Ok(index) = self.claims.binary_search_by(|claim| claim.at.cmp(&at))
        {
            self.claims.remove(index);
        }
        match block_entity_type(state) {
            None => {
                self.block_entities.remove(&at);
            }
            Some(kind) => {
                if self.block_entities.get(&at).map(|entity| entity.kind) != Some(kind) {
                    let data = Vec::new();
                    self.block_entities.insert(at, BlockEntity { kind, data });
                }
            }
        }
        self.generated = 0;
        self.with_failure = false;
        if light_tells_apart(old, state) {
            self.light = None;
            if let Some(noted) = &mut self.light_noted {
                noted.push(at);
            }
        }
        Some(old)
    }

    /// Whether the positions whose change light could see are being noted.
    pub fn notes_light_changes(&self) -> bool {
        self.light_noted.is_some()
    }

    /// Starts or stops noting the positions whose change light could see, for whoever
    /// keeps light up to date beside the chunk; stopping forgets what was noted.
    pub fn note_light_changes(&mut self, on: bool) {
        self.light_noted = on.then(|| self.light_noted.take().unwrap_or_default());
    }

    /// The positions noted since they were last taken, ascending, each once: light is a
    /// function of the blocks, so the order they changed in tells it nothing.
    pub fn take_light_changes(&mut self) -> Vec<InChunk> {
        let mut noted = self
            .light_noted
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        noted.sort_unstable();
        noted.dedup();
        noted
    }

    /// Brings the four heights of a column up to date with a change at `y`.
    fn follow_heights(&mut self, x: usize, y: i32, z: usize, old: BlockState, new: BlockState) {
        let column = z << 4 | x;
        let above = (y - self.min_y + 1) as u16;
        let (counted, counts) = (counts_for(old), counts_for(new));
        for map in 0..MAPS {
            let height = self.heights.maps()[map][column];
            let now = if counts[map] {
                height.max(above)
            } else if counted[map] && height == above {
                // The height rested on this block: the next one down that counts.
                let below = (0..above - 1).rev().find(|level| {
                    let y = self.min_y + i32::from(*level);
                    let state = self.get(x, y, z).expect("below a block of the chunk");
                    counts_for(state)[map]
                });
                below.map_or(0, |level| level + 1)
            } else {
                height
            };
            self.heights.maps()[map][column] = now;
        }
    }

    /// The heights of the blocks, found by going down from the top.
    fn scanned_heights(&self) -> Box<Heights> {
        let mut heights = Heights::none();
        let mut unresolved = MAPS * COLUMNS_PER_CHUNK;
        for (index, section) in self.sections.iter().enumerate().rev() {
            // Air counts for no map, and a fluid is not air.
            if section.non_air_count() == 0 {
                continue;
            }
            let counts = section.by_state(counts_for);
            for y in (0..SECTION_SIZE).rev() {
                let above = (index * SECTION_SIZE + y + 1) as u16;
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let counts = counts.at(x, y, z);
                        for (map, heights) in heights.maps().into_iter().enumerate() {
                            let height = &mut heights[z << 4 | x];
                            if *height == 0 && counts[map] {
                                *height = above;
                                unresolved -= 1;
                            }
                        }
                    }
                }
                if unresolved == 0 {
                    return heights;
                }
            }
        }
        heights
    }

    /// A block entity for every block that keeps one: the one `given` has for the
    /// position if it is of the block's type, and one without data otherwise.
    fn owed_block_entities(
        &self,
        mut given: BTreeMap<InChunk, BlockEntity>,
    ) -> BTreeMap<InChunk, BlockEntity> {
        let mut owed = BTreeMap::new();
        for (index, section) in self.sections.iter().enumerate() {
            if !section.may_hold(|state| block_entity_type(state).is_some()) {
                continue;
            }
            for (at, state) in section.states().enumerate() {
                let Some(kind) = block_entity_type(state) else {
                    continue;
                };
                let y = self.min_y + (index * SECTION_SIZE + (at >> 8)) as i32;
                let at = InChunk::new(at & 15, y, at >> 4 & 15);
                let entity = given.remove(&at).filter(|entity| entity.kind == kind);
                let data = Vec::new();
                owed.insert(at, entity.unwrap_or(BlockEntity { kind, data }));
            }
        }
        owed
    }

    /// The block entities, by position.
    pub fn block_entities(&self) -> &BTreeMap<InChunk, BlockEntity> {
        &self.block_entities
    }

    /// Gives the block entity at `at` the data `data`: the one way data comes into a
    /// chunk, which refuses more than a chunk may carry. Whoever brings data in handles
    /// the refusal, by keeping the entity without data.
    pub fn set_block_entity_data(&mut self, at: InChunk, data: Vec<u8>) -> Result<(), DataRefused> {
        let Some(entity) = self.block_entities.get(&at) else {
            return Err(DataRefused::NoBlockEntity);
        };
        if data.len() > MAX_BLOCK_ENTITY_DATA {
            return Err(DataRefused::TooMuchForOne);
        }
        let others = self.entity_data() - entity.data.len();
        if others + data.len() > MAX_CHUNK_ENTITY_DATA {
            return Err(DataRefused::TooMuchForTheChunk);
        }
        let entity = self.block_entities.get_mut(&at).expect("found above");
        // "As generated" is about the blocks, and the data is not one: it stays.
        entity.data = data;
        Ok(())
    }

    /// The bytes of data of all block entities together.
    fn entity_data(&self) -> usize {
        self.block_entities
            .values()
            .map(|entity| entity.data.len())
            .sum()
    }

    /// The four heightmaps.
    pub fn heights(&self) -> &Heights {
        &self.heights
    }

    /// For each column, indexed by `z << 4 | x`, the height above the highest block that
    /// is not air, counted from the bottom of the dimension; 0 for an empty column.
    pub fn surface_heights(&self) -> [u16; COLUMNS_PER_CHUNK] {
        self.heights.world_surface
    }

    /// The version of the generator this chunk is exactly as made by, or 0 if it was
    /// changed since or was made by none.
    pub fn generated(&self) -> u16 {
        self.generated
    }

    /// Whether something failed while the chunk was generated. Never of a chunk that
    /// is not as generated.
    pub fn with_failure(&self) -> bool {
        self.with_failure
    }

    /// Says the chunk is no longer exactly as generated, as a change of a block does:
    /// a change of its scheduled ticks makes it so too.
    pub fn clear_generated(&mut self) {
        self.generated = 0;
        self.with_failure = false;
    }

    /// Says that the chunk is exactly as the generator of `version` made it, and
    /// whether something failed on the way. A generator says so of what it has made;
    /// the first change takes it back.
    pub fn mark_generated(&mut self, version: u16, with_failure: bool) {
        self.generated = version;
        self.with_failure = with_failure && version != 0;
    }

    /// The light that was made for the chunk, if no change that light can see was made
    /// since.
    pub fn light(&self) -> Option<&Light> {
        self.light.as_ref()
    }

    /// Gives the chunk the light that was made for its blocks as they are now. It has
    /// an entry for the section below the chunk, one for each section and one for the
    /// section above, for the sky and for blocks.
    pub fn set_light(&mut self, light: Light) {
        let entries = self.sections.len() + 2;
        assert!(
            light.sky.len() == entries && light.block.len() == entries,
            "light for {} sections has {entries} entries",
            self.sections.len()
        );
        // Equal light is equal: an array that is 0 or 15 throughout is dark or full,
        // whoever made it.
        let one_form = |light: Vec<SectionLight>| -> Vec<SectionLight> {
            let one_form = |section| match section {
                SectionLight::Levels(levels) => SectionLight::of(levels),
                other => other,
            };
            light.into_iter().map(one_form).collect()
        };
        self.light = Some(Light {
            made_by: light.made_by,
            beside: light.beside,
            sky: one_form(light.sky),
            block: one_form(light.block),
        });
    }

    /// Takes the light out of the chunk and leaves the chunk without. An edge keeps
    /// the light of a chunk beside it (`docs/adr/0023-light-at-the-edge.md`,
    /// section 1); nothing else has a use for this.
    pub fn take_light(&mut self) -> Option<Light> {
        self.light.take()
    }

    /// The ticks generation scheduled, in its order.
    pub fn ticks(&self) -> &[ScheduledTick] {
        &self.ticks
    }

    pub fn set_ticks(&mut self, ticks: Vec<ScheduledTick>) {
        self.ticks = ticks;
    }

    /// The positions marked by generation, ascending, each once.
    pub fn marks(&self) -> &[InChunk] {
        &self.marks
    }

    pub fn set_marks(&mut self, mut marks: Vec<InChunk>) {
        marks.sort_unstable();
        marks.dedup();
        self.marks = marks;
    }

    /// The updates from this chunk that wait for their blocks to be loaded, by
    /// [`Seam::key`].
    pub fn seams(&self) -> &[Seam] {
        &self.seams
    }

    /// Takes `seams` as the chunk's, by [`Seam::key`]; of two with one key, the later.
    pub fn set_seams(&mut self, seams: Vec<Seam>) {
        let mut kept: BTreeMap<(BlockPos, u8, SeamKind), Seam> = BTreeMap::new();
        for seam in seams {
            kept.insert(seam.key(), seam);
        }
        self.seams = kept.into_values().collect();
    }

    /// The block events that wait in this chunk, in their order.
    pub fn events(&self) -> &[KeptEvent] {
        &self.events
    }

    /// Takes `events` as the chunk's, in their order.
    pub fn set_events(&mut self, events: Vec<KeptEvent>) {
        self.events = events;
    }

    /// The structure starts the chunk keeps, in generation's order.
    pub fn structures(&self) -> &[KeptStructure] {
        &self.structures
    }

    /// Takes `structures` as the chunk's, in their order.
    pub fn set_structures(&mut self, structures: Vec<KeptStructure>) {
        self.structures = structures;
    }

    /// The raids kept in the chunk, ascending by id.
    pub fn raids(&self) -> &[Raid] {
        &self.raids
    }

    /// Takes `raids` as the chunk's, ascending by id, of two with one id the later. A
    /// chunk whose raids changed is no longer as generated, as one whose claims changed.
    pub fn set_raids(&mut self, raids: Vec<Raid>) {
        let mut kept: BTreeMap<EntityId, Raid> = BTreeMap::new();
        for raid in raids {
            kept.insert(raid.id, raid);
        }
        let raids: Vec<Raid> = kept.into_values().collect();
        if raids != self.raids {
            self.raids = raids;
            self.generated = 0;
            self.with_failure = false;
        }
    }

    /// The claims on the chunk's points of interest, ascending by position.
    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }

    /// Takes `claims` as the chunk's, and says how many it dropped: a claim whose block
    /// is not of its type, or with as many free tickets as its type has, says nothing
    /// the blocks do not, and of two at one position the later is kept. A mismatch can
    /// come of a change made without corruption (a chunk made again, a commit's order),
    /// and the game itself drops such a record quietly, so it is no reason to refuse a
    /// chunk (`docs/groundwork/n10-plan.md`, section 3.2).
    pub fn set_claims(&mut self, claims: Vec<Claim>) -> usize {
        let given = claims.len();
        let mut kept: BTreeMap<InChunk, Claim> = BTreeMap::new();
        for claim in claims {
            if claim.free < claim.kind.max_tickets()
                && self.poi_type_at(claim.at) == Some(claim.kind)
            {
                kept.insert(claim.at, claim);
            }
        }
        self.claims = kept.into_values().collect();
        given - self.claims.len()
    }

    /// The chunk's points of interest, ascending by position: every block whose state is
    /// of a type (`PoiSection`'s records, as the blocks say them).
    pub fn pois(&self) -> &[(InChunk, PoiType)] {
        self.pois.get_or_init(|| {
            let mut found = Vec::new();
            for (index, section) in self.sections.iter().enumerate() {
                if !section.may_hold(|state| poi_type_of(state).is_some()) {
                    continue;
                }
                let bottom = self.min_y + (index * SECTION_SIZE) as i32;
                for y in 0..SECTION_SIZE {
                    for z in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if let Some(kind) = poi_type_of(section.get(x, y, z)) {
                                found.push((InChunk::new(x, bottom + y as i32, z), kind));
                            }
                        }
                    }
                }
            }
            found.into_boxed_slice()
        })
    }

    /// The type of point of interest of the block at `at`, if any.
    pub fn poi_type_at(&self, at: InChunk) -> Option<PoiType> {
        poi_type_of(self.get(usize::from(at.x), i32::from(at.y), usize::from(at.z))?)
    }

    /// How many of the tickets of the point of interest at `at` are free, if there is
    /// one (`PoiRecord.freeTickets`).
    pub fn free_tickets(&self, at: InChunk) -> Option<u8> {
        let kind = self.poi_type_at(at)?;
        Some(
            match self.claims.binary_search_by(|claim| claim.at.cmp(&at)) {
                Ok(index) => self.claims[index].free,
                Err(_) => kind.max_tickets(),
            },
        )
    }

    /// Takes a ticket of the point of interest at `at` (`PoiRecord.acquireTicket`):
    /// false where there is none or none is free.
    pub fn acquire_ticket(&mut self, at: InChunk) -> bool {
        let Some(kind) = self.poi_type_at(at) else {
            return false;
        };
        match self.claims.binary_search_by(|claim| claim.at.cmp(&at)) {
            Ok(index) => {
                let claim = &mut self.claims[index];
                if claim.free == 0 {
                    return false;
                }
                claim.free -= 1;
            }
            Err(index) => {
                let Some(free) = kind.max_tickets().checked_sub(1) else {
                    return false;
                };
                self.claims.insert(index, Claim { at, kind, free });
            }
        }
        self.claims_changed();
        true
    }

    /// A chunk whose claims changed is no longer as generated, as one whose blocks
    /// changed: a chunk whose generation failed is remade only where nothing of it is
    /// the world's own (`docs/groundwork/n10-plan.md`, section 3.2).
    fn claims_changed(&mut self) {
        self.generated = 0;
        self.with_failure = false;
    }

    /// Gives a ticket of the point of interest at `at` back (`PoiRecord.releaseTicket`):
    /// false where there is none or all are free.
    pub fn release_ticket(&mut self, at: InChunk) -> bool {
        let Ok(index) = self.claims.binary_search_by(|claim| claim.at.cmp(&at)) else {
            return false;
        };
        let claim = &mut self.claims[index];
        claim.free += 1;
        if claim.free >= claim.kind.max_tickets() {
            self.claims.remove(index);
        }
        self.claims_changed();
        true
    }

    /// Keeps `seam`, in place of an entry with its key.
    pub fn add_seam(&mut self, seam: Seam) {
        match self.seams.binary_search_by_key(&seam.key(), Seam::key) {
            Ok(index) => self.seams[index] = seam,
            Err(index) => self.seams.insert(index, seam),
        }
    }

    /// Whether a client is shown the same of both: the lowest block, the blocks and
    /// biomes by content, and the block entities. Whether the two are as generated,
    /// their light, their ticks and their marks are not asked. An edge decides by
    /// this whether those who view a chunk are sent it again.
    pub fn shows_the_same(&self, other: &Self) -> bool {
        self.min_y == other.min_y
            && self.sections == other.sections
            && self.block_entities == other.block_entities
    }

    /// The chunk as an edge is handed it: without the ticks, the marks, the seams, the
    /// block events, the structures and the claims, which are the simulation's and the
    /// store's, and
    /// without the entities, which an edge is told of as entities.
    pub fn for_viewers(&self) -> Self {
        Self {
            ticks: Vec::new(),
            marks: Vec::new(),
            seams: Vec::new(),
            events: Vec::new(),
            structures: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
            entities: Vec::new(),
            ..self.clone()
        }
    }

    /// The entities other than players in the chunk, ascending by key.
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// Takes `entities` as the chunk's; of two with one key, the later.
    pub fn set_entities(&mut self, entities: Vec<Entity>) {
        let mut kept: BTreeMap<EntityId, Entity> = BTreeMap::new();
        for entity in entities {
            kept.insert(entity.key, entity);
        }
        self.entities = kept.into_values().collect();
    }

    /// Does what a tick did to an entity of the chunk: a whole entity takes the place of
    /// the one with its key or is added, one gone is removed, and fields set are set on
    /// the entity with the key. A change of a key the chunk does not have is passed
    /// over, as replay from any state at or after the last checkpoint needs. Nothing is
    /// refused here: what a chunk may hold is the simulation's to decide before it makes
    /// a change.
    pub fn apply_entity(&mut self, change: &EntityChange) {
        let at = self
            .entities
            .binary_search_by_key(&change.key(), |entity| entity.key);
        match (change, at) {
            (EntityChange::Whole(entity), Ok(at)) => self.entities[at] = entity.clone(),
            (EntityChange::Whole(entity), Err(at)) => self.entities.insert(at, entity.clone()),
            (EntityChange::Gone(_), Ok(at)) => {
                self.entities.remove(at);
            }
            (EntityChange::Set(_, fields), Ok(at)) => {
                for field in fields {
                    field.set(&mut self.entities[at]);
                }
            }
            (EntityChange::Gone(_) | EntityChange::Set(..), Err(_)) => {}
        }
    }

    /// The section index and the y coordinate within that section for world height `y`.
    fn locate(&self, y: i32) -> Option<(usize, usize)> {
        let above_bottom = usize::try_from(y.checked_sub(self.min_y)?).ok()?;
        (above_bottom < self.height())
            .then_some((above_bottom / SECTION_SIZE, above_bottom % SECTION_SIZE))
    }
}

/// How a chunk travels: as it is, without the heights and the counts, which the
/// reader makes.
#[derive(Serialize, Deserialize)]
struct ChunkRepr {
    min_y: i32,
    sections: Vec<Section>,
    block_entities: Vec<(InChunk, BlockEntity)>,
    generated: u16,
    with_failure: bool,
    light: Option<Light>,
    ticks: Vec<ScheduledTick>,
    marks: Vec<InChunk>,
    seams: Vec<Seam>,
    events: Vec<KeptEvent>,
    structures: Vec<KeptStructure>,
    claims: Vec<Claim>,
    raids: Vec<Raid>,
    entities: Vec<Entity>,
}

impl From<Chunk> for ChunkRepr {
    fn from(chunk: Chunk) -> Self {
        Self {
            min_y: chunk.min_y,
            sections: chunk.sections,
            block_entities: chunk.block_entities.into_iter().collect(),
            generated: chunk.generated,
            with_failure: chunk.with_failure,
            light: chunk.light,
            ticks: chunk.ticks,
            marks: chunk.marks,
            seams: chunk.seams,
            events: chunk.events,
            structures: chunk.structures,
            claims: chunk.claims,
            raids: chunk.raids,
            entities: chunk.entities,
        }
    }
}

impl TryFrom<ChunkRepr> for Chunk {
    type Error = String;

    /// Checks what it is given, and makes what a chunk keeps true of itself hold: a
    /// block entity where the blocks ask for one and nowhere else, and no failure
    /// said of a chunk that is not as generated.
    fn try_from(repr: ChunkRepr) -> Result<Self, Self::Error> {
        let entries = repr.sections.len() + 2;
        if let Some(light) = &repr.light
            && (light.sky.len() != entries || light.block.len() != entries)
        {
            return Err(format!(
                "light of {} and {} entries for a chunk of {} sections",
                light.sky.len(),
                light.block.len(),
                repr.sections.len()
            ));
        }
        if !repr.marks.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("a chunk's marks are not ascending, each once".to_owned());
        }
        let mut data = 0;
        for (_, entity) in &repr.block_entities {
            if entity.data.len() > MAX_BLOCK_ENTITY_DATA {
                return Err(format!(
                    "a block entity has {} bytes of data",
                    entity.data.len()
                ));
            }
            data += entity.data.len();
        }
        if data > MAX_CHUNK_ENTITY_DATA {
            return Err(format!(
                "a chunk's block entities have {data} bytes of data"
            ));
        }
        let mut chunk = Self::from_sections(repr.min_y, repr.sections);
        chunk.block_entities = chunk.owed_block_entities(repr.block_entities.into_iter().collect());
        chunk.generated = repr.generated;
        chunk.with_failure = repr.with_failure && repr.generated != 0;
        chunk.light = repr.light;
        chunk.ticks = repr.ticks;
        chunk.marks = repr.marks;
        chunk.set_seams(repr.seams);
        chunk.events = repr.events;
        chunk.structures = repr.structures;
        chunk.set_claims(repr.claims);
        let mut raids = repr.raids;
        raids.sort_by_key(|raid| raid.id);
        raids.dedup_by_key(|raid| raid.id);
        chunk.raids = raids;
        if !repr
            .entities
            .windows(2)
            .all(|pair| pair[0].key < pair[1].key)
        {
            return Err("a chunk's entities are not ascending by key, each once".to_owned());
        }
        chunk.entities = repr.entities;
        Ok(chunk)
    }
}

/// Produces the initial contents of chunks that have never been stored.
///
/// A generator must be deterministic: the same position always yields the same chunk,
/// because unmodified chunks are regenerated instead of being persisted.
pub trait ChunkGenerator: Send + Sync {
    fn generate(&self, position: ChunkPos) -> Chunk;

    /// Names the generator and everything that decides what it produces, and its
    /// version. A stored world records both: it refuses to continue with a generator
    /// whose identity is another, or whose version is below the highest that made a
    /// chunk it stores (`docs/adr/0021-generation-is-a-function.md`, section 8).
    fn settings(&self) -> GeneratorSettings;

    /// The chunk the search for a new world's spawn begins in, as the game's
    /// `ChunkGenerator.getOrigin` says: the origin's, unless the generator looks for a
    /// place whose climate suits a start.
    fn origin(&self) -> ChunkPos {
        ChunkPos::new(0, 0)
    }

    /// Where the game puts a new world's spawn: the block a player's feet are in, on
    /// the first column to stand on in a spiral of chunks from [`origin`](Self::origin)
    /// ([`crate::spawn`]). It is the same for the same generator every time, and
    /// generates the chunks it reads, mostly one; a world keeps what it gave when the
    /// world was made, so that a later version of the generator does not move it.
    fn spawn(&self) -> BlockPos {
        crate::spawn::place_to_stand(self.origin(), |position| self.generate(position))
    }

    /// Makes these chunks ahead of any request, in this order, where the generator
    /// keeps what it makes for a while: what a later request costs changes, never what
    /// it gets. A world store asks it for the chunks around the spawn when it starts,
    /// so that the first player to enter does not wait for them. By default nothing.
    fn prepare(&self, _positions: &[ChunkPos]) {}

    /// Has `done` called once with the chunk, on any thread, perhaps before this
    /// returns. After the [`Wanted`] is dropped, `done` may never be called.
    ///
    /// Unless a generator says otherwise, the chunk is generated here and now, and a
    /// panic in [`generate`](Self::generate) is answered as [`Failed`].
    fn request(&self, position: ChunkPos, done: Done) -> Wanted {
        let made =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.generate(position)));
        done(made.map_err(|_| Failed));
        Wanted::nothing()
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::DIMENSION_TYPES;

    use super::*;

    fn overworld() -> &'static DimensionType {
        DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap()
    }

    fn empty() -> Chunk {
        Chunk::empty(overworld(), Biome(0))
    }

    fn item(key: u64, x: f64) -> Entity {
        Entity {
            portal: Default::default(),
            key: EntityId(key),
            kind: 72,
            uuid: uuid::Uuid::from_u128(u128::from(key)),
            position: [x, -59.0, 0.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            data: crate::EntityData::Item {
                stack: crate::ItemStack::new(1, 1),
                pick_up_delay: 0,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
            fall_distance: 0.0,
            fire_ticks: -1,
            riding: Default::default(),
        }
    }

    #[test]
    fn entity_changes_applied_in_order_leave_the_chunk_as_the_tick_left_it_whatever_was_applied_before()
     {
        use crate::{EntityChange, EntityField};
        let changes = [
            EntityChange::Whole(item(9, 1.5)),
            EntityChange::Whole(item(4, 2.5)),
            EntityChange::Set(EntityId(9), vec![EntityField::Position([3.5, -59.0, 0.5])]),
            EntityChange::Gone(EntityId(4)),
            EntityChange::Set(EntityId(4), vec![EntityField::Lived(3)]),
        ];
        let mut chunk = empty();
        for change in &changes {
            chunk.apply_entity(change);
        }
        let mut expected = item(9, 1.5);
        expected.position = [3.5, -59.0, 0.5];
        assert_eq!(chunk.entities(), [expected.clone()]);
        // Replayed over a chunk that has some of them already, as a restore does from a
        // stored chunk saved in the middle: the same.
        let mut again = chunk.clone();
        for change in &changes {
            again.apply_entity(change);
        }
        assert_eq!(again.entities(), [expected]);
        // Kept ascending by key, and part of what a chunk is equal by.
        let mut two = empty();
        two.apply_entity(&EntityChange::Whole(item(9, 1.5)));
        two.apply_entity(&EntityChange::Whole(item(4, 2.5)));
        assert_eq!(two.entities()[0].key, EntityId(4));
        assert_ne!(two, empty());
        assert!(two.shows_the_same(&empty()));
        assert!(two.for_viewers().entities().is_empty());
    }

    /// An edge keeps a chunk's light beside the chunk: it can take it out, with what
    /// the light was made beside, and the chunk is then without.
    #[test]
    fn light_is_taken_out_of_a_chunk_with_what_it_was_made_beside() {
        let mut chunk = empty();
        chunk.set(1, 0, 1, blocks::STONE);
        let entries = chunk.sections().len() + 2;
        let light = Light {
            made_by: 2,
            beside: 7,
            sky: vec![SectionLight::Full; entries],
            block: vec![SectionLight::Dark; entries],
        };
        chunk.set_light(light.clone());
        // A change light cannot see keeps the light, and what it was made beside.
        chunk.set(1, 0, 1, blocks::DEEPSLATE);
        assert_eq!(chunk.light().map(|light| light.beside), Some(7));
        assert_eq!(chunk.take_light(), Some(light));
        assert_eq!(chunk.light(), None);
        assert_eq!(chunk.take_light(), None);
    }

    /// Whether a state is air is among what light sees: which light sections are
    /// listed turns on which sections hold a block that is not air.
    #[test]
    fn light_tells_air_from_what_differs_from_it_in_nothing_else() {
        assert!(!light_tells_apart(blocks::AIR, blocks::CAVE_AIR));
        assert!(!light_tells_apart(blocks::STONE, blocks::DEEPSLATE));
        assert!(light_tells_apart(blocks::AIR, blocks::STONE));
        assert!(light_tells_apart(blocks::AIR, blocks::TORCH));
        let unlike_air_only_in_being_a_block = (0..BLOCK_STATE_COUNT as u16)
            .map(BlockState)
            .find(|state| {
                let (state, air) = (state.props(), blocks::AIR.props());
                !state.is_air()
                    && state.light_emission() == air.light_emission()
                    && state.light_dampening() == air.light_dampening()
                    && state.occludes() == air.occludes()
                    && state.occludes_by_shape() == air.occludes_by_shape()
                    && state.occlusion_face_set() == air.occlusion_face_set()
                    && state.propagates_skylight_down() == air.propagates_skylight_down()
            })
            .expect("a block that light passes as it passes air");
        assert!(light_tells_apart(
            blocks::AIR,
            unlike_air_only_in_being_a_block
        ));
    }

    #[test]
    fn overworld_chunk_has_24_sections_from_minus_64() {
        let chunk = empty();
        assert_eq!(chunk.sections().len(), 24);
        assert_eq!(chunk.min_y(), -64);
        assert_eq!(chunk.height(), 384);
    }

    #[test]
    fn blocks_are_addressed_by_world_height() {
        let mut chunk = empty();
        assert_eq!(chunk.set(3, -64, 4, blocks::BEDROCK), Some(blocks::AIR));
        assert_eq!(chunk.set(3, 319, 4, blocks::STONE), Some(blocks::AIR));
        assert_eq!(chunk.get(3, -64, 4), Some(blocks::BEDROCK));
        assert_eq!(chunk.get(3, 319, 4), Some(blocks::STONE));
        assert_eq!(chunk.get(3, 0, 4), Some(blocks::AIR));
        assert_eq!(chunk.sections()[0].non_air_count(), 1);
        assert_eq!(chunk.sections()[23].non_air_count(), 1);
    }

    #[test]
    fn heights_outside_the_dimension_are_rejected() {
        let mut chunk = empty();
        assert_eq!(chunk.get(0, -65, 0), None);
        assert_eq!(chunk.get(0, 320, 0), None);
        assert_eq!(chunk.get(0, i32::MIN, 0), None);
        assert_eq!(chunk.set(0, 320, 0, blocks::STONE), None);
        assert_eq!(chunk, empty());
    }

    /// The surface is the highest block that is not air to the game, and cave air is.
    #[test]
    fn surface_heights_pass_over_cave_air() {
        let mut chunk = empty();
        chunk.set(3, 10, 5, blocks::STONE);
        chunk.set(3, 40, 5, blocks::CAVE_AIR);
        chunk.set(7, 40, 7, blocks::VOID_AIR);
        let heights = chunk.surface_heights();
        assert_eq!(heights[5 << 4 | 3], 75);
        assert_eq!(heights[7 << 4 | 7], 0);
    }

    #[test]
    fn surface_heights_point_above_the_highest_block() {
        let mut chunk = empty();
        assert_eq!(chunk.surface_heights(), [0; 256]);

        chunk.set(0, -64, 0, blocks::BEDROCK);
        chunk.set(5, 100, 2, blocks::STONE);
        chunk.set(5, 20, 2, blocks::STONE);
        chunk.set(15, 319, 15, blocks::STONE);
        let heights = chunk.surface_heights();
        assert_eq!(heights[0], 1);
        assert_eq!(heights[2 << 4 | 5], 165);
        assert_eq!(heights[255], 384);
        assert_eq!(heights.iter().filter(|height| **height == 0).count(), 253);
    }

    fn hut() -> KeptStructure {
        let piece = KeptBox {
            min: BlockPos::new(3, 61, 5),
            max: BlockPos::new(9, 67, 13),
        };
        KeptStructure {
            kind: KeptKind::SwampHut,
            source: ChunkPos::new(0, 0),
            bounds: KeptBox {
                min: BlockPos::new(3, 64, 5),
                max: BlockPos::new(9, 70, 13),
            },
            pieces: vec![piece],
        }
    }

    #[test]
    fn a_chunk_s_structures_are_part_of_its_equality_and_its_copies_but_not_of_what_viewers_get() {
        let mut chunk = empty();
        chunk.set_structures(vec![hut()]);
        assert_ne!(chunk, empty());
        assert_eq!(chunk.clone(), chunk);
        assert_eq!(chunk.structures(), [hut()]);
        assert!(chunk.shows_the_same(&empty()));
        assert!(chunk.for_viewers().structures().is_empty());

        let travelled: Chunk = postcard::from_bytes(&postcard::to_stdvec(&chunk).unwrap()).unwrap();
        assert_eq!(travelled, chunk);
        assert_eq!(travelled.structures(), [hut()]);
    }

    fn lectern_at(chunk: &mut Chunk, x: usize, y: i32, z: usize) -> InChunk {
        chunk.set(x, y, z, blocks::LECTERN);
        InChunk::new(x, y, z)
    }

    #[test]
    fn a_chunk_s_points_of_interest_follow_its_blocks() {
        let mut chunk = empty();
        assert!(chunk.pois().is_empty());
        let lectern = lectern_at(&mut chunk, 3, 64, 5);
        chunk.set(1, 70, 1, blocks::BELL);
        assert_eq!(
            chunk.pois(),
            [
                (lectern, PoiType::Librarian),
                (InChunk::new(1, 70, 1), PoiType::Meeting)
            ]
        );
        chunk.set(3, 64, 5, blocks::AIR);
        assert_eq!(chunk.pois(), [(InChunk::new(1, 70, 1), PoiType::Meeting)]);
    }

    #[test]
    fn tickets_are_taken_and_given_back_and_only_a_claim_is_kept() {
        let mut chunk = empty();
        let lectern = lectern_at(&mut chunk, 3, 64, 5);
        assert_eq!(chunk.free_tickets(lectern), Some(1));
        assert!(chunk.claims().is_empty());
        assert!(chunk.acquire_ticket(lectern));
        assert_eq!(chunk.free_tickets(lectern), Some(0));
        assert!(!chunk.acquire_ticket(lectern), "a lectern has one ticket");
        assert_eq!(
            chunk.claims(),
            [Claim {
                at: lectern,
                kind: PoiType::Librarian,
                free: 0
            }]
        );
        assert!(chunk.release_ticket(lectern));
        assert!(!chunk.release_ticket(lectern), "every ticket is free");
        assert!(chunk.claims().is_empty(), "a full record is no claim");
        assert!(
            !chunk.acquire_ticket(InChunk::new(0, 64, 0)),
            "no record there"
        );

        // The bell's 32 tickets: a claim keeps how many are free.
        chunk.set(1, 70, 1, blocks::BELL);
        let bell = InChunk::new(1, 70, 1);
        assert!(chunk.acquire_ticket(bell) && chunk.acquire_ticket(bell));
        assert_eq!(chunk.free_tickets(bell), Some(30));
    }

    #[test]
    fn a_block_of_another_type_takes_its_claim_with_it_and_one_of_the_same_does_not() {
        let mut chunk = empty();
        chunk.set(2, 64, 2, blocks::COMPOSTER);
        let composter = InChunk::new(2, 64, 2);
        assert!(chunk.acquire_ticket(composter));
        let fuller = blocks::COMPOSTER
            .with_property("level", "3")
            .expect("a composter has levels");
        chunk.set(2, 64, 2, fuller);
        assert_eq!(
            chunk.free_tickets(composter),
            Some(0),
            "still a farmer's site"
        );
        chunk.set(2, 64, 2, blocks::LECTERN);
        assert!(chunk.claims().is_empty());
        assert_eq!(chunk.free_tickets(composter), Some(1));
    }

    #[test]
    fn claims_that_say_nothing_or_do_not_fit_the_blocks_are_dropped_not_refused() {
        let mut chunk = empty();
        let lectern = lectern_at(&mut chunk, 3, 64, 5);
        let claim = |at, kind, free| Claim { at, kind, free };
        let dropped = chunk.set_claims(vec![
            claim(lectern, PoiType::Librarian, 0),
            claim(InChunk::new(0, 64, 0), PoiType::Librarian, 0),
            claim(lectern, PoiType::Mason, 0),
        ]);
        assert_eq!(dropped, 2);
        assert_eq!(chunk.claims(), [claim(lectern, PoiType::Librarian, 0)]);
        assert_eq!(
            chunk.set_claims(vec![claim(lectern, PoiType::Librarian, 1)]),
            1
        );
        assert!(chunk.claims().is_empty());
    }

    #[test]
    fn a_chunk_s_claims_are_part_of_its_equality_and_its_copies_but_not_of_what_viewers_get() {
        let mut chunk = empty();
        let lectern = lectern_at(&mut chunk, 3, 64, 5);
        let unclaimed = chunk.clone();
        assert!(chunk.acquire_ticket(lectern));
        assert_ne!(chunk, unclaimed);
        assert!(chunk.shows_the_same(&unclaimed));
        assert!(chunk.for_viewers().claims().is_empty());

        let travelled: Chunk = postcard::from_bytes(&postcard::to_stdvec(&chunk).unwrap()).unwrap();
        assert_eq!(travelled, chunk);
        assert_eq!(travelled.free_tickets(lectern), Some(0));
    }

    #[test]
    fn a_box_holds_its_corners_and_widens_on_every_side() {
        let piece = hut().pieces[0];
        assert!(piece.holds(BlockPos::new(3, 61, 5)));
        assert!(piece.holds(BlockPos::new(9, 67, 13)));
        assert!(!piece.holds(BlockPos::new(9, 68, 13)));
        assert!(!piece.holds(BlockPos::new(2, 61, 5)));
        let wide = piece.widened(32);
        assert!(wide.holds(BlockPos::new(-29, 29, 45)));
        assert!(!wide.holds(BlockPos::new(-30, 29, 45)));
        assert!(wide.meets(ChunkPos::new(-2, 2)));
        assert!(!wide.meets(ChunkPos::new(-3, 0)));
        assert!(piece.meets(ChunkPos::new(0, 0)) && !piece.meets(ChunkPos::new(1, 0)));
    }

    #[test]
    fn every_kind_is_found_by_its_name_and_only_villages_have_no_overrides() {
        for kind in KeptKind::ALL {
            assert_eq!(KeptKind::from_name(kind.name()), Some(kind));
            assert_eq!(
                kind.is_village(),
                kind.name().starts_with("minecraft:village_")
            );
        }
        assert_eq!(KeptKind::from_name("minecraft:igloo"), None);
        assert_eq!(KeptKind::SwampHut.overrides(), Some(OverrideBox::Piece));
        assert_eq!(KeptKind::Monument.overrides(), Some(OverrideBox::Full));
    }
}
