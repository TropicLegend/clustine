// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/template_piece.rs`
// (see NOTICE.md at the root), which follows the game's template pieces; the
// mansion's `afterPlace`, which SteelMC has not, follows the game's
// `WoodlandMansionStructure.afterPlace` as the pinned jar has it.

//! The woodland mansion's blocks: each piece one template of the mansion's rooms,
//! walls and roofs (`WoodlandMansionPieces.WoodlandMansionPiece`, a
//! `TemplateStructurePiece`), and, once a chunk's pieces stand, the cobblestone the
//! start puts under its floor (`WoodlandMansionStructure.afterPlace`).
//!
//! A piece is placed as `TemplateStructurePiece.postProcess` places it: clipped to the
//! centre chunk, with structure blocks ignored, its entities ignored, its palette and
//! processors drawn from each position's own source, and the flags 2, which lack
//! `UPDATE_KNOWN_SHAPE`: the shapes at the template's edges are brought up to date and
//! placed blocks mark themselves where their state asks. Then every data marker of the
//! template inside the centre chunk goes to `handleDataMarker`, and every jigsaw there
//! becomes its final state: `filterBlocks` leaves out what the settings' box, the
//! centre chunk's, does not hold, so that a marker is handled once, by the run of the
//! chunk it lies in.
//!
//! The markers: a chest of the mansion's loot faces as its name says, turned with the
//! piece, and draws its seed from the run's random source; an evoker (`Mage`), a
//! vindicator (`Warrior`) or one to three allays (`Group of Allays`) are made, kept from
//! despawning, and finalised from the region's random source
//! (`WorldGenRegion.getRandom`), each followed by the marker's block made air. A type
//! the maker does not make yet is drawn for as the game's would be, since the features
//! after it draw from the same source.

use clustine_data::{BlockState, blocks, entity_types};
use clustine_noise::Random;
use clustine_protocol::nbt::Nbt;
use clustine_structures::Start;
use clustine_structures::geometry::{BoundingBox, Direction, Rotation};
use clustine_structures::piece::{StructureBlockIgnore, TemplatePieceData};
use clustine_structures::place::{StructurePlaceSettings, place_in_world, transform_pos};
use clustine_structures::processors::{PlaceRandom, Processor, parse_block_state, position_random};
use clustine_structures::template_blocks::TemplateBlock;
use clustine_world::{BlockEntity, BlockPos};

use super::Placing;
use crate::blocks::same_block;
use crate::features::caves::templates::FeatureShapes;
use crate::features::ground::shared::{block_entity, compound};

/// The loot of the mansion's chests (`BuiltInLootTables.WOODLAND_MANSION`).
const LOOT: &str = "minecraft:chests/woodland_mansion";

/// Places a piece of the mansion. Its box never moves, so it keeps no state between
/// runs.
pub(crate) fn place(placing: &mut Placing<'_, '_, '_>, data: &TemplatePieceData) -> bool {
    // Without the operator's jar no template's block is placed, as for features.
    let Some(library) = placing.settings.templates() else {
        return false;
    };
    let Some(template) = library.get(data.template_id.path) else {
        return false;
    };

    let ignore_structure = Processor::structure_block_ignore();
    let shapes = FeatureShapes {
        settings: placing.settings,
    };
    let mut settings = StructurePlaceSettings::new(placing.settings.seed());
    settings.mirror = data.mirror;
    settings.rotation = data.rotation;
    settings.rotation_pivot = BlockPos::new(
        data.rotation_pivot.x,
        data.rotation_pivot.y,
        data.rotation_pivot.z,
    );
    settings.bounding_box = Some(placing.chunk_box);
    settings.liquid_settings = data.liquid_settings;
    // `makeSettings`: entities ignored, then the one processor.
    settings.ignore_entities = true;
    debug_assert_eq!(data.block_ignore, StructureBlockIgnore::StructureBlock);
    settings.processors.push(&ignore_structure);
    settings.known_shape = false;
    settings.marks = true;
    settings.random = PlaceRandom::Positional;
    settings.shapes = &shapes;

    let at = BlockPos::new(
        data.template_position.x,
        data.template_position.y,
        data.template_position.z,
    );
    let reference = BlockPos::new(placing.pivot.x, placing.pivot.y, placing.pivot.z);
    // Its settings ignore the template's entities: there are none to make.
    if place_in_world(
        template,
        placing.level.view(),
        at,
        reference,
        &settings,
        &mut *placing.random,
    )
    .is_none()
    {
        return false;
    }

    // `filterBlocks` takes the palette `getRandomPalette` gives for the template's
    // position, from that position's own source, as `placeInWorld` did.
    let count = i32::try_from(template.palettes.len()).expect("fewer than 2^31 palettes");
    let palette = &template.palettes[position_random(at).next_i32_bounded(count) as usize];
    let moved = |block: &TemplateBlock| {
        let pos = transform_pos(
            block.pos,
            settings.mirror,
            settings.rotation,
            settings.rotation_pivot,
        );
        BlockPos::new(pos.x + at.x, pos.y + at.y, pos.z + at.z)
    };
    let inside = |at: BlockPos| settings.contains(at);
    for block in palette {
        if !same_block(block.state, blocks::STRUCTURE_BLOCK) || !inside(moved(block)) {
            continue;
        }
        let Some(nbt) = &block.nbt else { continue };
        if string(nbt, "mode") != Some("DATA") {
            continue;
        }
        let marker = string(nbt, "metadata").unwrap_or("");
        data_marker(placing, data.rotation, marker, moved(block));
    }
    for block in palette {
        if !same_block(block.state, blocks::JIGSAW) || !inside(moved(block)) {
            continue;
        }
        let Some(nbt) = &block.nbt else { continue };
        // A state the game cannot read leaves air, with an error in its log.
        let state = parse_block_state(string(nbt, "final_state").unwrap_or("minecraft:air"))
            .unwrap_or(blocks::AIR);
        // The flags 3, which mark.
        placing.level.set_block(moved(block), state);
    }
    true
}

fn string<'n>(nbt: &'n Nbt, key: &str) -> Option<&'n str> {
    match nbt.get(key) {
        Some(Nbt::String(text)) => Some(text.as_str()),
        _ => None,
    }
}

/// `WoodlandMansionPiece.handleDataMarker`.
fn data_marker(placing: &mut Placing<'_, '_, '_>, rotation: Rotation, marker: &str, at: BlockPos) {
    if marker.starts_with("Chest") {
        let facing = match marker {
            "ChestWest" => Some(Direction::West),
            "ChestEast" => Some(Direction::East),
            "ChestSouth" => Some(Direction::South),
            "ChestNorth" => Some(Direction::North),
            _ => None,
        };
        let chest = facing.map_or(blocks::CHEST, |facing| {
            blocks::CHEST
                .with_property("facing", rotation.rotate(facing).as_str())
                .expect("a chest faces every horizontal direction")
        });
        create_chest(placing, at, chest);
        return;
    }
    let mobs = match marker {
        "Mage" => vec![Mob::Evoker],
        "Warrior" => vec![Mob::Vindicator],
        "Group of Allays" => {
            let random = region_random(placing);
            let count = random.next_i32_bounded(3) + 1;
            vec![Mob::Allay; count as usize]
        }
        _ => return,
    };
    // `snapTo(Vec3.atBottomCenterOf(pos), 0, 0)`.
    let position = [
        f64::from(at.x) + 0.5,
        f64::from(at.y),
        f64::from(at.z) + 0.5,
    ];
    for mob in mobs {
        let settings = placing.settings;
        if !placing
            .level
            .make_mob(settings, mob.kind(), true, position, Some(STRUCTURE))
        {
            mob.finalize_spawn(region_random(placing));
        }
        // The game sets the air once for every mob, with the flags 2.
        placing.level.set_block(at, blocks::AIR);
    }
}

/// The structure the mansion's mobs are made in.
const STRUCTURE: &str = "minecraft:woodland_mansion";

fn region_random<'r>(placing: &'r mut Placing<'_, '_, '_>) -> &'r mut clustine_noise::RandomSource {
    let seed = placing.settings.seed();
    let legacy = placing.settings.legacy_random_source();
    placing.level.region_random(seed, legacy)
}

/// `StructurePiece.createChest` with a state given: only inside the centre chunk, and
/// not over a chest. The seed is drawn from the piece's random source, as
/// `RandomizableContainer.setLootTable` is given it.
fn create_chest(placing: &mut Placing<'_, '_, '_>, at: BlockPos, chest: BlockState) {
    if !placing.chunk_box.contains_xyz(at.x, at.y, at.z)
        || same_block(placing.level.block(at), blocks::CHEST)
    {
        return;
    }
    // Inside the centre chunk the game's `setBlock` always takes, and leaves a chest's
    // block entity for `getBlockEntity` to find.
    if !placing.level.set_block(at, chest) {
        return;
    }
    let seed = placing.random.next_i64();
    placing.level.set_block_entity(at, Some(chest_data(seed)));
}

/// What the game saves for a chest whose loot is still to be rolled
/// (`RandomizableContainer.trySaveLootTable`), as the dungeon's chests: the table, the
/// seed unless it is nought, and no components.
fn chest_data(seed: i64) -> BlockEntity {
    let mut members = vec![("LootTable", Nbt::String(LOOT.to_string()))];
    if seed != 0 {
        members.push(("LootTableSeed", Nbt::Long(seed)));
    }
    members.push(("components", compound(Vec::new())));
    block_entity("minecraft:chest", members)
}

/// The mobs the markers name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mob {
    Evoker,
    Vindicator,
    Allay,
}

impl Mob {
    /// Its entity type.
    fn kind(self) -> i32 {
        match self {
            Self::Evoker => entity_types::EVOKER,
            Self::Vindicator => entity_types::VINDICATOR,
            Self::Allay => entity_types::ALLAY,
        }
    }

    /// The draws of the mob's `finalizeSpawn` with the reason `STRUCTURE`, in order,
    /// for a type the maker does not make yet, so that what is placed after it draws
    /// as the game's would.
    ///
    /// - `Mob.finalizeSpawn` (every one of them): the follow range's random bonus,
    ///   `triangle(0, 0.11485)` (two doubles), then whether it is left-handed (a
    ///   float). `PatrollingMonster` draws for a patrol's leader only for other
    ///   reasons, and `Raider` draws nothing.
    /// - `Vindicator.finalizeSpawn`, after that: its iron axe is given without a draw,
    ///   then `enchantSpawnedEquipment` draws one float for the axe, whether it is
    ///   enchanted (`0.25 × getSpecialMultiplier`); its empty armour slots draw
    ///   nothing. The special multiplier of a world being generated is nought on easy
    ///   and on a young world on normal, so the axe is never enchanted there; where it
    ///   is (hard, or normal after many days), the game draws for the enchantments
    ///   too, which is not followed.
    fn finalize_spawn(self, random: &mut impl Random) {
        let _follow_range_bonus = random.triangle(0.0, 0.114_850_000_000_000_01);
        let _left_handed = random.next_f32() < 0.05;
        if self == Self::Vindicator {
            let _enchanted = random.next_f32();
        }
    }
}

/// `WoodlandMansionStructure.afterPlace`: under every column of the centre chunk where
/// a piece stands on the start's lowest floor, cobblestone down through air and liquid
/// to the first other block, above the world's lowest block.
pub(crate) fn after_place(placing: &mut Placing<'_, '_, '_>, start: &Start) {
    let mut boxes = start.pieces.iter().map(|piece| piece.bounding_box);
    let Some(first) = boxes.next() else { return };
    // `PiecesContainer.calculateBoundingBox`: the pieces' own boxes, not the start's,
    // which may be grown.
    let all = boxes.fold(first, |all, piece| BoundingBox::encapsulating(&all, &piece));
    let floor = all.min_y();
    let world_min = placing.level.min_y();
    let chunk = placing.chunk_box;
    for x in chunk.min_x()..=chunk.max_x() {
        for z in chunk.min_z()..=chunk.max_z() {
            let at = BlockPos::new(x, floor, z);
            if placing.level.block(at).is_air()
                || !all.contains_xyz(x, floor, z)
                || !start
                    .pieces
                    .iter()
                    .any(|piece| piece.bounding_box.contains_xyz(x, floor, z))
            {
                continue;
            }
            for y in (world_min + 1..floor).rev() {
                let at = BlockPos::new(x, y, z);
                let state = placing.level.block(at);
                if !state.is_air() && !state.props().is_liquid() {
                    break;
                }
                placing.level.set_block(at, blocks::COBBLESTONE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use clustine_noise::{RandomSource, WorldgenRandom};
    use clustine_structures::geometry::{IVec3, Identifier};
    use clustine_structures::piece::{
        StructureMirror, StructurePiece, StructurePiecePayload, TemplateMarkerHandling,
        TemplatePlacementAdjustment, TemplatePlacementClip, TemplatePostProcess,
        TemplateProcessorList,
    };
    use clustine_structures::template_blocks::{TemplateBlocks, TemplateLibrary};
    use clustine_terrain::biome::Dimension;
    use clustine_world::generation::{
        FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing as Attempt, StartRef,
        WorldgenHeightmap,
    };
    use clustine_world::{Biome, ChunkPos, ScheduledTick};
    use clustine_worldgen_data::structure::LiquidSettings;
    use clustine_worldgen_data::structures::STRUCTURES;

    use super::*;
    use crate::context::Level;
    use crate::settings::FeatureSettings;
    use crate::structures::{after_place as start_after_place, place_piece};
    use crate::testing::World;

    const MIN_Y: i32 = -64;
    const HEIGHT: i32 = 384;
    const ROOM: &str = "test_mansion_room";

    /// The toy world with the block entities it drops kept.
    struct Kept {
        world: World,
        entities: BTreeMap<BlockPos, BlockEntity>,
    }

    impl Neighbourhood for Kept {
        fn centre(&self) -> ChunkPos {
            self.world.centre()
        }
        fn block(&mut self, at: BlockPos) -> BlockState {
            self.world.block(at)
        }
        fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
            self.world.set_block(at, state)
        }
        fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
            self.entities.get(&at)
        }
        fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
            match entity {
                Some(entity) => self.entities.insert(at, entity),
                None => self.entities.remove(&at),
            };
            true
        }
        fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
            self.world.height(map, x, z)
        }
        fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
            self.world.worldgen_height(map, x, z)
        }
        fn biome(&mut self, at: BlockPos) -> Biome {
            self.world.biome(at)
        }
        fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
            self.world.schedule(at, tick);
        }
        fn mark(&mut self, at: BlockPos) {
            self.world.mark(at);
        }
        fn add_entity(&mut self, entity: clustine_world::Entity) -> bool {
            self.world.add_entity(entity)
        }
        fn starts(&self) -> &[StartRef] {
            &[]
        }
        fn piece(&mut self, key: PieceKey) -> PieceState {
            self.world.piece(key)
        }
        fn set_piece(&mut self, key: PieceKey, state: PieceState) {
            self.world.set_piece(key, state);
        }
        fn attempt(&mut self, _: Attempt, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
            place(self);
            true
        }
    }

    fn marker(pos: [i32; 3], metadata: &str) -> TemplateBlock {
        TemplateBlock {
            pos: BlockPos::new(pos[0], pos[1], pos[2]),
            state: blocks::STRUCTURE_BLOCK,
            nbt: Some(Nbt::Compound(vec![
                ("metadata".into(), Nbt::String(metadata.into())),
                ("mode".into(), Nbt::String("DATA".into())),
            ])),
        }
    }

    fn block(pos: [i32; 3], state: BlockState) -> TemplateBlock {
        TemplateBlock {
            pos: BlockPos::new(pos[0], pos[1], pos[2]),
            state,
            nbt: None,
        }
    }

    fn settings(blocks: Vec<TemplateBlock>) -> FeatureSettings {
        let template = TemplateBlocks {
            size: [8, 4, 8],
            data_version: clustine_data::DATA_VERSION,
            palettes: vec![blocks],
            entities: Vec::new(),
        };
        FeatureSettings::new(13579, Dimension::Overworld)
            .unwrap()
            .with_templates(Arc::new(TemplateLibrary::from_templates([(
                ROOM.to_string(),
                template,
            )])))
    }

    fn piece(at: IVec3, rotation: Rotation, mirror: StructureMirror) -> StructurePiece {
        let bounding_box = clustine_structures::place::template_bounding_box(
            [8, 4, 8],
            BlockPos::new(at.x, at.y, at.z),
            mirror,
            rotation,
            BlockPos::new(0, 0, 0),
        );
        let mut piece = StructurePiece::non_jigsaw(
            Identifier::new_static("minecraft", "wmp"),
            bounding_box,
            0,
            Some(Direction::North),
        );
        piece.payload = StructurePiecePayload::Template(TemplatePieceData {
            template_id: Identifier::new_static("minecraft", ROOM),
            template_position: at,
            rotation,
            mirror,
            rotation_pivot: IVec3::ZERO,
            block_ignore: StructureBlockIgnore::StructureBlock,
            late_block_ignore: StructureBlockIgnore::None,
            processors: TemplateProcessorList::Empty,
            liquid_settings: LiquidSettings::ApplyWaterlogging,
            marker_handling: TemplateMarkerHandling::WoodlandMansion,
            placement_adjustment: TemplatePlacementAdjustment::None,
            placement_clip: TemplatePlacementClip::CenterChunk,
            post_process: TemplatePostProcess::None,
        });
        piece
    }

    fn mansion() -> usize {
        STRUCTURES
            .iter()
            .position(|structure| {
                matches!(
                    structure.kind,
                    clustine_worldgen_data::structure::Kind::WoodlandMansion
                )
            })
            .expect("the data has the mansion")
    }

    fn start(pieces: Vec<StructurePiece>) -> Start {
        let bounding_box = pieces
            .iter()
            .map(|piece| piece.bounding_box)
            .reduce(|a, b| BoundingBox::encapsulating(&a, &b))
            .unwrap();
        Start {
            structure: mansion(),
            chunk: (0, 0),
            pieces,
            bounding_box,
        }
    }

    fn chunk_box() -> BoundingBox {
        BoundingBox::new(
            clustine_structures::geometry::IVec3::new(0, MIN_Y + 1, 0),
            clustine_structures::geometry::IVec3::new(15, MIN_Y + HEIGHT - 1, 15),
        )
    }

    /// Places every piece of `start` that meets the centre chunk, then its
    /// `afterPlace`, as `run.rs` does; returns the run's random source and the
    /// region's as they are left.
    fn run(
        world: &mut Kept,
        settings: &FeatureSettings,
        start: &Start,
    ) -> (WorldgenRandom, RandomSource) {
        run_making(world, settings, start, &clustine_world::generation::KeyOnly)
    }

    /// [`run`], the mobs made by `maker`.
    fn run_making(
        world: &mut Kept,
        settings: &FeatureSettings,
        start: &Start,
        maker: &dyn clustine_world::generation::Maker,
    ) -> (WorldgenRandom, RandomSource) {
        let mut random = WorldgenRandom::from_seed(42);
        let makings = clustine_world::generation::Makings::new(0, ChunkPos { x: 0, z: 0 });
        let mut level =
            Level::new(world, settings.zoom(), MIN_Y, HEIGHT).with_maker(maker, makings);
        let mut placing = super::Placing {
            level: &mut level,
            random: &mut random,
            settings,
            chunk_box: chunk_box(),
            centre: ChunkPos { x: 0, z: 0 },
            pivot: IVec3::ZERO,
        };
        for (index, piece) in start.pieces.iter().enumerate() {
            if !piece.bounding_box.intersects(chunk_box()) {
                continue;
            }
            let key = PieceKey {
                start: StartRef {
                    source: ChunkPos { x: 0, z: 0 },
                    index: 0,
                },
                piece: u16::try_from(index).unwrap(),
            };
            let state = PieceState {
                shift_y: 0,
                settled: false,
                flags: 7,
            };
            // The mansion's pieces never move: what a run keeps of them is unchanged.
            assert_eq!(place_piece(&mut placing, start, key, state), state);
        }
        start_after_place(&mut placing, start);
        let region = placing
            .level
            .region_random(settings.seed(), settings.legacy_random_source())
            .clone();
        (random, region)
    }

    fn fresh_region(settings: &FeatureSettings) -> RandomSource {
        let mut world = kept(blocks::AIR, MIN_Y);
        let mut level = Level::new(&mut world, settings.zoom(), MIN_Y, HEIGHT);
        level
            .region_random(settings.seed(), settings.legacy_random_source())
            .clone()
    }

    fn kept(ground: BlockState, top: i32) -> Kept {
        Kept {
            world: World::flat(ground, top),
            entities: BTreeMap::new(),
        }
    }

    #[test]
    fn a_piece_places_its_blocks_clipped_to_the_centre_chunk_and_leaves_its_markers_out() {
        let settings = settings(vec![
            block([0, 0, 0], blocks::STONE_BRICKS),
            block([7, 0, 7], blocks::STONE_BRICKS),
            marker([1, 1, 1], "nothing the mansion knows"),
        ]);
        // The piece reaches from x 12 to 19: its far corner lies in the next chunk.
        let start = start(vec![piece(
            IVec3::new(12, 70, 12),
            Rotation::None,
            StructureMirror::None,
        )]);
        let mut world = kept(blocks::AIR, MIN_Y);
        run(&mut world, &settings, &start);
        assert_eq!(
            world.world.get(BlockPos::new(12, 70, 12)),
            blocks::STONE_BRICKS
        );
        assert_eq!(world.world.get(BlockPos::new(19, 70, 19)), blocks::AIR);
        assert_eq!(world.world.get(BlockPos::new(13, 71, 13)), blocks::AIR);
    }

    #[test]
    fn a_chest_marker_turns_with_the_piece_and_draws_its_seed_from_the_run() {
        let settings = settings(vec![marker([1, 1, 1], "ChestWest")]);
        // Turned a quarter clockwise, (1, 1, 1) of the template lies at (-1, 1, 1)
        // from its corner, and west becomes north.
        let start = start(vec![piece(
            IVec3::new(8, 70, 2),
            Rotation::Clockwise90,
            StructureMirror::None,
        )]);
        let mut world = kept(blocks::AIR, MIN_Y);
        let (mut random, mut region) = run(&mut world, &settings, &start);
        let at = BlockPos::new(7, 71, 3);
        let north = blocks::CHEST.with_property("facing", "north").unwrap();
        assert_eq!(world.world.get(at), north);

        let mut expected = WorldgenRandom::from_seed(42);
        let seed = expected.next_i64();
        assert_eq!(world.entities.get(&at), Some(&chest_data(seed)));
        assert_eq!(world.entities.len(), 1);
        // The run's source moved by the seed alone, the region's not at all.
        assert_eq!(random.next_i64(), expected.next_i64());
        assert_eq!(region.next_i64(), fresh_region(&settings).next_i64());
    }

    #[test]
    fn a_chest_marker_outside_the_centre_chunk_places_nothing_and_draws_nothing() {
        let settings = settings(vec![
            block([0, 0, 0], blocks::STONE_BRICKS),
            marker([6, 1, 6], "Chest"),
        ]);
        let start = start(vec![piece(
            IVec3::new(12, 70, 12),
            Rotation::None,
            StructureMirror::None,
        )]);
        let mut world = kept(blocks::AIR, MIN_Y);
        let (mut random, _) = run(&mut world, &settings, &start);
        assert_eq!(world.world.get(BlockPos::new(18, 71, 18)), blocks::AIR);
        assert!(world.entities.is_empty());
        assert_eq!(random.next_i64(), WorldgenRandom::from_seed(42).next_i64());
    }

    #[test]
    fn mob_markers_draw_from_the_region_as_the_mobs_finalize_spawn_and_leave_air() {
        // Markers of mobs are handled in the template's order (by y, x, z), only where
        // they lie in the centre chunk: the warrior's, in the next chunk, is left to
        // that chunk's run.
        let settings = settings(vec![
            marker([1, 1, 1], "Group of Allays"),
            marker([2, 1, 1], "Mage"),
            marker([7, 2, 7], "Warrior"),
        ]);
        let start = start(vec![piece(
            IVec3::new(12, 70, 12),
            Rotation::None,
            StructureMirror::None,
        )]);
        let mut world = kept(blocks::AIR, MIN_Y);
        let places = [
            BlockPos::new(13, 71, 13),
            BlockPos::new(14, 71, 13),
            BlockPos::new(19, 72, 19),
        ];
        for at in places {
            world.world.set(at, blocks::DIRT);
        }
        let (_, mut region) = run(&mut world, &settings, &start);
        for at in &places[..2] {
            assert_eq!(world.world.get(*at), blocks::AIR, "{at:?}");
        }
        assert_eq!(world.world.get(places[2]), blocks::DIRT);

        let mut expected = fresh_region(&settings);
        let allays = expected.next_i32_bounded(3) + 1;
        for _ in 0..allays {
            expected.next_f64();
            expected.next_f64();
            expected.next_f32();
        }
        // The evoker.
        expected.next_f64();
        expected.next_f64();
        expected.next_f32();
        assert_eq!(region.next_i64(), expected.next_i64());
    }

    #[test]
    fn after_place_fills_air_and_liquid_under_the_floor_of_the_pieces_down_to_the_first_other_block()
     {
        let settings = settings(Vec::new());
        // Two pieces on the same floor at y 70, with a gap of columns between them
        // (x 8 and 9) inside the box around both.
        let start = start(vec![
            piece(IVec3::new(0, 70, 0), Rotation::None, StructureMirror::None),
            piece(IVec3::new(10, 70, 0), Rotation::None, StructureMirror::None),
        ]);
        let mut world = kept(blocks::STONE, 60);
        for x in 0..18 {
            for z in 0..9 {
                world.world.set(BlockPos::new(x, 70, z), blocks::OAK_PLANKS);
            }
        }
        // Water is filled through; a waterlogged block is neither air nor liquid.
        world.world.set(BlockPos::new(1, 65, 0), blocks::WATER);
        let logged = BlockState::parse("minecraft:oak_slab[waterlogged=true]").unwrap();
        world.world.set(BlockPos::new(2, 65, 0), logged);
        // A floor of air is left alone.
        world.world.set(BlockPos::new(3, 70, 0), blocks::AIR);
        run(&mut world, &settings, &start);

        let column = |world: &Kept, x: i32, z: i32| -> Vec<BlockState> {
            (60..70)
                .map(|y| world.world.get(BlockPos::new(x, y, z)))
                .collect()
        };
        let mut filled = vec![blocks::STONE];
        filled.extend([blocks::COBBLESTONE; 9]);
        assert_eq!(column(&world, 0, 0), filled);
        assert_eq!(column(&world, 1, 0), filled);
        let mut stopped = vec![blocks::STONE, blocks::AIR, blocks::AIR, blocks::AIR];
        stopped.extend([blocks::AIR, logged]);
        stopped.extend([blocks::COBBLESTONE; 4]);
        assert_eq!(column(&world, 2, 0), stopped);
        let mut untouched = vec![blocks::STONE];
        untouched.extend([blocks::AIR; 9]);
        assert_eq!(column(&world, 3, 0), untouched);
        // In the box around both pieces but in neither.
        assert_eq!(column(&world, 8, 0), untouched);
        assert_eq!(column(&world, 9, 0), untouched);
        assert_eq!(column(&world, 10, 0), filled);
        // In the second piece but outside the centre chunk.
        assert_eq!(column(&world, 16, 0), untouched);
        // Outside every piece.
        assert_eq!(column(&world, 0, 8), untouched);
    }

    #[test]
    fn after_place_never_fills_the_lowest_block_of_the_world() {
        let settings = settings(Vec::new());
        let start = start(vec![piece(
            IVec3::new(0, 70, 0),
            Rotation::None,
            StructureMirror::None,
        )]);
        let mut world = kept(blocks::AIR, MIN_Y - 1);
        world.world.set(BlockPos::new(0, 70, 0), blocks::OAK_PLANKS);
        run(&mut world, &settings, &start);
        assert_eq!(world.world.get(BlockPos::new(0, MIN_Y, 0)), blocks::AIR);
        for y in MIN_Y + 1..70 {
            assert_eq!(
                world.world.get(BlockPos::new(0, y, 0)),
                blocks::COBBLESTONE,
                "{y}"
            );
        }
    }

    #[test]
    fn the_mansion_counts_as_built() {
        assert!(crate::structures::built(mansion()));
    }

    #[test]
    fn a_group_of_allays_draws_its_size_first_and_each_allay_is_made_kept_at_the_marker() {
        let settings = settings(vec![marker([1, 1, 1], "Group of Allays")]);
        let start = start(vec![piece(
            IVec3::new(12, 70, 12),
            Rotation::None,
            StructureMirror::None,
        )]);
        let maker = crate::testing::ToyMaker {
            mobs: &[entity_types::ALLAY],
            draws: false,
        };
        let mut world = kept(blocks::AIR, MIN_Y);
        let (_, mut region) = run_making(&mut world, &settings, &start, &maker);
        // `level.getRandom().nextInt(3) + 1` of them, drawn before any is made.
        let mut expected = fresh_region(&settings);
        let count = expected.next_i32_bounded(3) + 1;
        let made = &world.world.made;
        assert_eq!(made.len(), count as usize);
        for allay in made {
            assert_eq!(allay.kind, entity_types::ALLAY as u16);
            // `snapTo(Vec3.atBottomCenterOf(pos), 0, 0)` at the marker's block.
            assert_eq!(allay.position, [13.5, 71.0, 13.5]);
            let (structure, persistent) = crate::testing::ToyMaker::handed(allay);
            assert_eq!(structure, STRUCTURE.as_bytes());
            assert!(persistent);
        }
        // The toy allays draw nothing: the region's source moved by the count alone.
        assert_eq!(region.next_i64(), expected.next_i64());
    }
}
