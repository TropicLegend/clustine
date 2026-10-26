//! What a feature places with: the world as the game's `WorldGenRegion` offers it to a
//! feature, through the view of one feature run (ADR-0021, section 3), and the random
//! source and settings of the run.

use clustine_data::BlockState;
use clustine_noise::{NameHash, PositionalRandom, Random, RandomSource, WorldgenRandom};
use clustine_structures::place::PlacedEntity;
use clustine_terrain::biome::zoom::Zoom;
use clustine_world::generation::{
    FinalHeightmap, FromNoise, KeyOnly, Maker, Makings, Neighbourhood, Placed, WorldgenHeightmap,
};
use clustine_world::{
    Biome, BlockEntity, BlockPos, ChunkPos, Entity, InChunk, ScheduledTick, Ticked,
};

use crate::settings::FeatureSettings;

/// The block a position is in (`BlockPos.containing`).
fn containing(position: [f64; 3]) -> BlockPos {
    BlockPos::new(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )
}

/// The game's `Heightmap.Types`. The two of world generation are the terrain's,
/// which do not change once terrain is made; the four others follow the blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heightmap {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

impl Heightmap {
    /// The type a file names (`WORLD_SURFACE_WG` and the like).
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "WORLD_SURFACE_WG" => Self::WorldSurfaceWg,
            "WORLD_SURFACE" => Self::WorldSurface,
            "OCEAN_FLOOR_WG" => Self::OceanFloorWg,
            "OCEAN_FLOOR" => Self::OceanFloor,
            "MOTION_BLOCKING" => Self::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Self::MotionBlockingNoLeaves,
            _ => return None,
        })
    }
}

/// A fresh region random source of a region of world generation centred on `centre`
/// (`WorldGenRegion.getRandom`): the world's factory for
/// `minecraft:worldgen_region_random` at the chunk's lowest corner. A feature run
/// starts from it, and so does the spawn of the same chunk, with a new one equal to
/// the run's first (`docs/groundwork/n7l-plan.md`, section 3.1).
pub fn region_random(seed: i64, legacy_random_source: bool, centre: ChunkPos) -> RandomSource {
    let world = if legacy_random_source {
        clustine_noise::LegacyRandom::from_seed(seed as u64).next_positional()
    } else {
        clustine_noise::Xoroshiro::from_seed(seed as u64).next_positional()
    };
    world
        .with_hash_of(&NameHash::new("minecraft:worldgen_region_random"))
        .next_positional()
        .at(centre.x * 16, 0, centre.z * 16)
}

/// The world a feature reads and changes: the three by three chunks of its run.
pub struct Level<'a> {
    view: &'a mut dyn Neighbourhood,
    zoom: Zoom,
    min_y: i32,
    height: i32,
    /// The region's own random source (`WorldGenRegion.getRandom`), made when it is
    /// first asked for.
    region_random: Option<RandomSource>,
    /// What makes the entities the structures make (`docs/groundwork/n7l-plan.md`,
    /// section 2.1): one that makes none unless the run hands its own.
    maker: &'a dyn Maker,
    /// The run's makings, carried from placement to placement as the region's random
    /// source is.
    makings: Makings,
}

impl<'a> Level<'a> {
    /// A level of `view` that makes no entity: a structure's mob is then drawn for and
    /// not made, as before entities were made with chunks.
    pub fn new(view: &'a mut dyn Neighbourhood, zoom: Zoom, min_y: i32, height: i32) -> Self {
        let centre = view.centre();
        Self {
            view,
            zoom,
            min_y,
            height,
            region_random: None,
            maker: &KeyOnly,
            makings: Makings::new(0, centre),
        }
    }

    /// This level with the structures' entities made by `maker`, their makings
    /// `makings`.
    pub fn with_maker(mut self, maker: &'a dyn Maker, makings: Makings) -> Self {
        self.maker = maker;
        self.makings = makings;
        self
    }

    /// The makings as the placements so far left them, which the run hands to the next
    /// placement's level.
    pub fn makings(&self) -> Makings {
        self.makings
    }

    /// Makes the mob of the type `kind` that a structure's code makes (`create` for the
    /// reason `STRUCTURE`, `setPersistenceRequired` where `persistent`, `snapTo` at
    /// `position` facing nowhere, `finalizeSpawn` from the region's random source), in
    /// the structure `structure` where the type reads it (a swamp hut's cat), and keeps
    /// it with any made beside it. False where the
    /// maker makes none of the type, having drawn nothing: the caller then draws what
    /// the game's mob would, which later placements read.
    pub fn make_mob(
        &mut self,
        settings: &FeatureSettings,
        kind: i32,
        persistent: bool,
        position: [f64; 3],
        structure: Option<&'static str>,
    ) -> bool {
        let Ok(kind) = u16::try_from(kind) else {
            return false;
        };
        let biome = self.biome(containing(position));
        self.region_random(settings.seed(), settings.legacy_random_source());
        let Self {
            view,
            region_random,
            maker,
            makings,
            ..
        } = self;
        let region = region_random
            .as_mut()
            .expect("the region's random source was just made");
        let made = maker.mob(
            kind,
            persistent,
            Placed {
                position,
                yaw: 0.0,
                pitch: 0.0,
                structure,
                biome,
                makings,
                level: &mut FromNoise(region),
            },
        );
        let any = !made.is_empty();
        for entity in made {
            view.add_entity(entity);
        }
        any
    }

    /// Makes the entity that is no mob, of the type `kind`, that a structure's code
    /// makes at `position` (a mineshaft's chest minecart), and keeps it. It draws
    /// nothing of the region's.
    pub fn make_entity(&mut self, settings: &FeatureSettings, kind: i32, position: [f64; 3]) {
        let Ok(kind) = u16::try_from(kind) else {
            return;
        };
        let biome = self.biome(containing(position));
        self.region_random(settings.seed(), settings.legacy_random_source());
        let Self {
            view,
            region_random,
            maker,
            makings,
            ..
        } = self;
        let region = region_random
            .as_mut()
            .expect("the region's random source was just made");
        if let Some(entity) = maker.entity(
            kind,
            Placed {
                position,
                yaw: 0.0,
                pitch: 0.0,
                structure: None,
                biome,
                makings,
                level: &mut FromNoise(region),
            },
        ) {
            view.add_entity(entity);
        }
    }

    /// Makes the entities a template's placing handed back, in its order, at once
    /// (the end of `placeInWorld`, `addFreshEntityWithPassengers`): each loaded from its
    /// tag, and those to be finalised that are mobs finalised from the region's random
    /// source.
    pub fn make_template_entities(
        &mut self,
        settings: &FeatureSettings,
        placed: Vec<PlacedEntity>,
    ) {
        for entity in placed {
            let biome = self.biome(containing(entity.position));
            self.region_random(settings.seed(), settings.legacy_random_source());
            let Self {
                view,
                region_random,
                maker,
                makings,
                ..
            } = self;
            let region = region_random
                .as_mut()
                .expect("the region's random source was just made");
            let made = maker.template_entity(
                &entity.tag_bytes(),
                entity.finalise,
                Placed {
                    position: entity.position,
                    yaw: entity.yaw,
                    pitch: entity.pitch,
                    structure: None,
                    biome,
                    makings,
                    level: &mut FromNoise(region),
                },
            );
            for made in made {
                view.add_entity(made);
            }
        }
    }

    /// `WorldGenRegion.ensureCanWrite`: whether the chunk of `at` is one the run may
    /// write, within one chunk of the centre. The game checks this before it places a
    /// configured feature at all, and before each block an ore places.
    pub fn ensure_can_write(&self, at: BlockPos) -> bool {
        let centre = self.view.centre();
        let chunk = at.chunk();
        (chunk.x - centre.x).abs() <= 1 && (chunk.z - centre.z).abs() <= 1
    }

    /// The region's own random source, which a few blocks draw from as they are
    /// placed (the pale moss carpet's sides): the world's factory for
    /// `minecraft:worldgen_region_random` at the centre chunk's lowest corner.
    ///
    /// The game makes one for a whole feature run and every feature draws from it in
    /// turn; `run.rs` hands it from each feature's `Level` to the next
    /// ([`Self::carry_region_random`]).
    pub fn region_random(&mut self, seed: i64, legacy_random_source: bool) -> &mut RandomSource {
        let centre = self.view.centre();
        self.region_random
            .get_or_insert_with(|| region_random(seed, legacy_random_source, centre))
    }

    /// Keeps an entity the feature made, without its key, as the game's
    /// `addFreshEntity` in a region of world generation: false where the view refuses
    /// it, outside the three by three.
    pub fn add_entity(&mut self, entity: Entity) -> bool {
        self.view.add_entity(entity)
    }

    /// Gives this level the region's random source as the feature before left it, and
    /// returns what it holds instead: so one source lasts a whole run, as the game's
    /// does. A feature that panics loses its draws with it, which only a chunk already
    /// marked as made with a failure (ADR-0021) can show.
    pub(crate) fn carry_region_random(
        &mut self,
        random: Option<RandomSource>,
    ) -> Option<RandomSource> {
        std::mem::replace(&mut self.region_random, random)
    }

    /// The view itself, for what this does not offer.
    pub fn view(&mut self) -> &mut dyn Neighbourhood {
        &mut *self.view
    }

    /// The lowest block's y.
    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// The y above the highest block (`getMaxY() + 1` in the game).
    pub fn top_y(&self) -> i32 {
        self.min_y + self.height
    }

    /// The number of blocks from the bottom to the top.
    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn is_outside_build_height(&self, y: i32) -> bool {
        y < self.min_y || y >= self.top_y()
    }

    pub fn block(&mut self, at: BlockPos) -> BlockState {
        self.view.block(at)
    }

    /// Sets a block, as the game's `setBlock` does in a region of world generation:
    /// false where the view refuses it, outside the three by three or the height.
    /// The game's flags of updates have no effect there, as no neighbour is updated
    /// while features are placed. A block whose state asks for it is marked for
    /// post-processing (`getPostProcessPos`): of the pinned jar's blocks, the two small
    /// mushrooms mark themselves, and soul sand and magma the block above them, where a
    /// bubble column may stand.
    pub fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        if !self.view.set_block(at, state) {
            return false;
        }
        if let Some([dx, dy, dz]) = state.props().post_process() {
            self.view
                .mark(at.offset(i32::from(dx), i32::from(dy), i32::from(dz)));
        }
        true
    }

    /// Sets a block as the game writes straight into a chunk's section
    /// (`BulkSectionAccess`, which ores use): nothing is marked.
    pub fn set_block_in_section(&mut self, at: BlockPos, state: BlockState) -> bool {
        self.view.set_block(at, state)
    }

    pub fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
        self.view.block_entity(at)
    }

    pub fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
        self.view.set_block_entity(at, entity)
    }

    /// The y above the highest block that counts for `map` in the column of x, z
    /// (`getHeight` in the game).
    pub fn height_at(&mut self, map: Heightmap, x: i32, z: i32) -> i32 {
        match map {
            Heightmap::WorldSurfaceWg => {
                self.view
                    .worldgen_height(WorldgenHeightmap::WorldSurface, x, z)
            }
            Heightmap::OceanFloorWg => {
                self.view
                    .worldgen_height(WorldgenHeightmap::OceanFloor, x, z)
            }
            Heightmap::WorldSurface => self.view.height(FinalHeightmap::WorldSurface, x, z),
            Heightmap::OceanFloor => self.view.height(FinalHeightmap::OceanFloor, x, z),
            Heightmap::MotionBlocking => self.view.height(FinalHeightmap::MotionBlocking, x, z),
            Heightmap::MotionBlockingNoLeaves => {
                self.view
                    .height(FinalHeightmap::MotionBlockingNoLeaves, x, z)
            }
        }
    }

    /// The biome at a block as the game's `getBiome` gives it: the zoom of the biome
    /// cells around it.
    pub fn biome(&mut self, at: BlockPos) -> Biome {
        let (x, y, z) = self.zoom.cell(at.x, at.y, at.z);
        self.view.biome(BlockPos::new(x << 2, y << 2, z << 2))
    }

    /// The light of blocks at a block, which nothing has made while features are
    /// placed: the game answers 0 there as well.
    pub fn block_light(&mut self, _at: BlockPos) -> u8 {
        0
    }

    /// The light of the sky at a block while features are placed: 15, also under
    /// ground. No chunk within one of a run is lit before the run (it would need the
    /// run's own chunk decorated first), and where a column has no light at all the
    /// game's sky light answers 15 (`SkyLightSectionStorage.getLightValue`).
    pub fn sky_light(&mut self, _at: BlockPos) -> u8 {
        15
    }

    /// `getRawBrightness(at, 0)`: the greater of the sky's and the blocks' light.
    pub fn raw_brightness(&mut self, at: BlockPos) -> u8 {
        self.sky_light(at).max(self.block_light(at))
    }

    /// `scheduleTick` of a block, which waits for phase B in the chunk.
    pub fn schedule_block_tick(&mut self, at: BlockPos, block: u16, delay: i32) {
        self.view
            .schedule(at, tick(at, Ticked::Block(block), delay));
    }

    /// `scheduleTick` of a fluid.
    pub fn schedule_fluid_tick(&mut self, at: BlockPos, fluid: u8, delay: i32) {
        self.view
            .schedule(at, tick(at, Ticked::Fluid(fluid), delay));
    }

    /// `markPosForPostprocessing`: the block takes its shape when the chunk is
    /// finished (phase B).
    pub fn mark_for_post_processing(&mut self, at: BlockPos) {
        self.view.mark(at);
    }
}

/// The rules of blocks (`updateShape`, `canSurvive`) read a feature run's world as the
/// game's `WorldGenRegion` offers it, and take generation's side where the copies of
/// the rules disagreed.
impl clustine_blocks::BlockView for Level<'_> {
    fn next_int(&mut self, _bound: u32) -> u32 {
        0
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        Level::block(self, at)
    }

    fn raw_brightness(&mut self, at: BlockPos) -> Option<u8> {
        Some(Level::raw_brightness(self, at))
    }

    fn schedule_block_tick(&mut self, at: BlockPos, block: u16, delay: i32) {
        Level::schedule_block_tick(self, at, block, delay);
    }

    fn schedule_fluid_tick(&mut self, at: BlockPos, fluid: u8, delay: i32) {
        Level::schedule_fluid_tick(self, at, fluid, delay);
    }

    fn rules(&self) -> clustine_blocks::Rules {
        clustine_blocks::Rules::Generation
    }
}

/// A tick of the game's normal priority, the one `scheduleTick` without a priority
/// gives, as a chunk being generated keeps it: without the delay it was asked for
/// (`ProtoChunkTicks.schedule` saves every tick with a delay of nought), so that it
/// falls due as soon as the chunk is whole.
fn tick(at: BlockPos, of: Ticked, _delay: i32) -> ScheduledTick {
    ScheduledTick {
        at: InChunk::new(
            at.x.rem_euclid(16) as usize,
            at.y,
            at.z.rem_euclid(16) as usize,
        ),
        of,
        delay: 0,
        priority: 0,
    }
}

/// What a feature places with.
pub struct Context<'a, 'v> {
    pub level: &'a mut Level<'v>,
    /// The run's random source, seeded for the placed feature being placed.
    pub random: &'a mut WorldgenRandom,
    pub settings: &'a FeatureSettings,
    /// The placed feature the run is placing, which the modifier `biome` asks about
    /// (`PlacementContext.topFeature`): its index among the placed features. None
    /// where a feature is placed by another, outside a run's list.
    pub top: Option<usize>,
}

impl Level<'_> {
    /// Sets a block as the game's `setBlock` does in a region of world generation
    /// with the flag `UPDATE_KNOWN_SHAPE` (16) among its flags: false where the view
    /// refuses it, and nothing marked for post-processing. Trees set their logs,
    /// leaves, roots and decorations so (the flags 19).
    pub fn set_block_known_shape(&mut self, at: BlockPos, state: BlockState) -> bool {
        self.view.set_block(at, state)
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::{blocks, entity_types};
    use clustine_noise::WorldgenRandom;
    use clustine_protocol::nbt::Nbt;
    use clustine_structures::geometry::{BoundingBox, IVec3, Rotation};
    use clustine_structures::place::{StructurePlaceSettings, place_in_world};
    use clustine_structures::template_blocks::{TemplateBlocks, TemplateEntity};

    use super::*;
    use crate::testing::{ToyMaker, World, settings};

    /// The desert village's `camel_spawn`: its `pos` (0, 0, 0), its `blockPos` (0, 0, 1),
    /// its tag `{id}` alone.
    fn camel_spawn() -> TemplateBlocks {
        TemplateBlocks {
            size: [1, 1, 2],
            data_version: clustine_data::DATA_VERSION,
            palettes: vec![Vec::new()],
            entities: vec![TemplateEntity {
                pos: [0.0, 0.0, 0.0],
                block_pos: BlockPos::new(0, 0, 1),
                nbt: Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:camel".into()))]),
            }],
        }
    }

    /// Places `camel_spawn` turned clockwise about its corner at (16, 70, 0) in a run of
    /// the chunk `(x, 0)`, and makes its entities: those the toy world kept, and the
    /// makings the run took.
    fn run_of(x: i32) -> (Vec<Entity>, u32) {
        let mut world = World::flat(blocks::SAND, 69);
        let maker = ToyMaker {
            mobs: &[],
            draws: true,
        };
        let settings = settings();
        let taken = {
            let mut level = Level::new(&mut world, settings.zoom(), -64, 384)
                .with_maker(&maker, Makings::new(0, ChunkPos::new(x, 0)));
            let mut placing = StructurePlaceSettings::new(settings.seed());
            placing.rotation = Rotation::Clockwise90;
            placing.finalize_entities = true;
            placing.known_shape = true;
            placing.bounding_box = Some(BoundingBox::new(
                IVec3::new(x * 16, -63, 0),
                IVec3::new(x * 16 + 15, 319, 15),
            ));
            let at = BlockPos::new(16, 70, 0);
            let entities = place_in_world(
                &camel_spawn(),
                level.view(),
                at,
                at,
                &placing,
                &mut WorldgenRandom::from_seed(1),
            )
            .expect("it places");
            level.make_template_entities(settings, entities);
            level.makings().taken()
        };
        (world.made, taken)
    }

    #[test]
    fn a_turned_camel_is_made_by_the_run_whose_chunk_holds_its_block_and_kept_in_the_next() {
        // Its block, turned to x 15, is chunk 0's; its position, x 17, chunk 1's.
        let (made, taken) = run_of(0);
        assert_eq!(made.len(), 1);
        assert_eq!(taken, 1);
        assert_eq!(made[0].position, [17.0, 70.0, 0.0]);
        assert_eq!(made[0].chunk(), ChunkPos::new(1, 0));
        assert_eq!(made[0].yaw, 90.0);
        // Named by the first making of chunk 0's run.
        let making = clustine_world::Generated {
            dimension: 0,
            chunk: ChunkPos::new(0, 0),
            structure: true,
            index: 0,
        };
        assert_eq!(Some(made[0].key), making.key());
        // Chunk 1's run, which holds its position but not its block, makes nothing.
        assert_eq!(run_of(1), (Vec::new(), 0));
    }

    #[test]
    fn a_mob_a_maker_does_not_make_is_left_to_the_caller_and_takes_no_making() {
        let mut world = World::flat(blocks::GRASS_BLOCK, 63);
        let maker = ToyMaker {
            mobs: &[entity_types::CAT],
            draws: false,
        };
        let settings = settings();
        let mut level = Level::new(&mut world, settings.zoom(), -64, 384)
            .with_maker(&maker, Makings::new(0, ChunkPos::new(0, 0)));
        let at = [3.5, 64.0, 3.5];
        assert!(!level.make_mob(settings, entity_types::WITCH, true, at, None));
        assert_eq!(level.makings().taken(), 0);
        assert!(level.make_mob(settings, entity_types::CAT, true, at, None));
        assert_eq!(level.makings().taken(), 1);
        assert_eq!(world.made.len(), 1);
    }
}
