//! A chunk column: a stack of sections and what else a chunk carries
//! (`docs/adr/0022-what-a-chunk-carries.md`).

use std::collections::BTreeMap;

use clustine_data::{BLOCK_INFO, BLOCK_STATE_COUNT, BlockState, DimensionType, blocks};
use serde::{Deserialize, Serialize};

use crate::generation::{Done, Failed, GeneratorSettings, Wanted};
use crate::light::{Light, SectionLight};
use crate::{Biome, BlockPos, COLUMNS_PER_CHUNK, ChunkPos, SECTION_SIZE, Section};

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
/// ticks and the marks. [`Chunk::shows_the_same`] asks only what a client is shown.
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
}

impl PartialEq for Chunk {
    fn eq(&self, other: &Self) -> bool {
        self.shows_the_same(other)
            && self.generated == other.generated
            && self.with_failure == other.with_failure
            && self.light == other.light
            && self.ticks == other.ticks
            && self.marks == other.marks
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
        if self.light.is_some() && light_tells_apart(old, state) {
            self.light = None;
        }
        Some(old)
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

    /// Whether a client is shown the same of both: the lowest block, the blocks and
    /// biomes by content, and the block entities. Whether the two are as generated,
    /// their light, their ticks and their marks are not asked. An edge decides by
    /// this whether those who view a chunk are sent it again.
    pub fn shows_the_same(&self, other: &Self) -> bool {
        self.min_y == other.min_y
            && self.sections == other.sections
            && self.block_entities == other.block_entities
    }

    /// The chunk as an edge is handed it: without the ticks and the marks, which are
    /// the simulation's and the store's.
    pub fn for_viewers(&self) -> Self {
        Self {
            ticks: Vec::new(),
            marks: Vec::new(),
            ..self.clone()
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
}
