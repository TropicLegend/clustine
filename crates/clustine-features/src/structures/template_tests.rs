//! Tests of the template families' placers (shipwrecks, igloos, ocean ruins, ruined
//! portals) and of buried treasure, in the toy world with block entities, with
//! templates made by hand: what the game's classes do with them, read from the pinned
//! jar.

use std::sync::{Arc, OnceLock};

use clustine_data::{BlockState, blocks};
use clustine_noise::{Random, WorldgenRandom};
use clustine_protocol::codec::Reader as NbtReader;
use clustine_protocol::nbt::Nbt;
use clustine_structures::compat::{LiquidSettingsData, OceanRuinBiomeTempData};
use clustine_structures::geometry::{BoundingBox, Direction, IVec3, Identifier, Rotation};
use clustine_structures::piece::{
    ProceduralPieceData, RuinedPortalProperties, StructureBlockIgnore, StructureMirror,
    StructurePiece, StructurePiecePayload, TemplateMarkerHandling, TemplatePieceData,
    TemplatePlacementAdjustment, TemplatePlacementClip, TemplatePostProcess, TemplateProcessorList,
};
use clustine_structures::template_blocks::{TemplateBlock, TemplateBlocks, TemplateLibrary};
use clustine_terrain::biome::Dimension;
use clustine_world::generation::PieceState;
use clustine_world::{BlockPos, ChunkPos};
use clustine_worldgen_data::structure::PortalPlacement;

use super::Placing;
use crate::context::Level;
use crate::features::ground::shared::testing::WithEntities;
use crate::settings::FeatureSettings;
use crate::testing::World;

fn compound(members: Vec<(&str, Nbt)>) -> Nbt {
    Nbt::Compound(
        members
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

/// A data marker's structure block, as a template keeps it.
fn marker(metadata: &str) -> Option<Nbt> {
    Some(compound(vec![
        ("mode", Nbt::String("DATA".into())),
        ("metadata", Nbt::String(metadata.into())),
        ("id", Nbt::String("minecraft:structure_block".into())),
    ]))
}

fn block(x: i32, y: i32, z: i32, state: BlockState, nbt: Option<Nbt>) -> TemplateBlock {
    TemplateBlock {
        pos: BlockPos::new(x, y, z),
        state,
        nbt,
    }
}

/// A floor of planks `size` wide and deep, a chest on it at (1, 1, 1) with a marker
/// above it naming `metadata`.
fn hull(size: [i32; 3], metadata: &str) -> TemplateBlocks {
    let mut full = Vec::new();
    for x in 0..size[0] {
        for z in 0..size[2] {
            full.push(block(x, 0, z, blocks::OAK_PLANKS, None));
        }
    }
    // Sorted as the game sorts a palette: full blocks, the others, those with data.
    full.push(block(
        1,
        1,
        1,
        blocks::CHEST,
        Some(compound(vec![(
            "id",
            Nbt::String("minecraft:chest".into()),
        )])),
    ));
    full.push(block(1, 2, 1, blocks::STRUCTURE_BLOCK, marker(metadata)));
    TemplateBlocks {
        size,
        data_version: clustine_data::DATA_VERSION,
        palettes: vec![full],
        entities: Vec::new(),
    }
}

/// The overworld's features with the templates the tests place.
fn settings() -> &'static FeatureSettings {
    static SETTINGS: OnceLock<FeatureSettings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let templates = vec![
            (
                "shipwreck/test".to_string(),
                hull([4, 3, 5], "supply_chest"),
            ),
            ("igloo/top".to_string(), hull([7, 3, 8], "nothing")),
            ("igloo/bottom".to_string(), hull([7, 3, 9], "chest")),
            ("underwater_ruin/test".to_string(), hull([6, 3, 7], "chest")),
            (
                "underwater_ruin/drowned".to_string(),
                hull([6, 3, 7], "drowned"),
            ),
            ("ruined_portal/test".to_string(), hull([6, 3, 6], "chest")),
        ];
        FeatureSettings::new(13579, Dimension::Overworld)
            .expect("the data is read whole")
            .with_templates(Arc::new(TemplateLibrary::from_templates(templates)))
    })
}

/// The writable box of chunk (0, 0).
fn chunk_box() -> BoundingBox {
    BoundingBox::new(IVec3::new(0, -63, 0), IVec3::new(15, 319, 15))
}

/// Runs `f` with what a piece is placed with, in chunk (0, 0), with the random source
/// seeded with `seed`.
fn with_placing<R>(
    world: &mut WithEntities,
    random: &mut WorldgenRandom,
    f: impl FnOnce(&mut Placing<'_, '_, '_>) -> R,
) -> R {
    let settings = settings();
    let mut level = Level::new(world, settings.zoom(), -64, 384);
    let mut placing = Placing {
        level: &mut level,
        random,
        settings,
        chunk_box: chunk_box(),
        centre: ChunkPos { x: 0, z: 0 },
        pivot: IVec3::new(0, 0, 0),
    };
    f(&mut placing)
}

fn template_piece(data: TemplatePieceData, size: [i32; 3]) -> StructurePiece {
    let bounding_box = clustine_structures::place::template_bounding_box(
        size,
        BlockPos::new(
            data.template_position.x,
            data.template_position.y,
            data.template_position.z,
        ),
        data.mirror,
        data.rotation,
        BlockPos::new(
            data.rotation_pivot.x,
            data.rotation_pivot.y,
            data.rotation_pivot.z,
        ),
    );
    StructurePiece {
        piece_type: Identifier::new_static("minecraft", "test"),
        bounding_box,
        gen_depth: 0,
        orientation: Some(Direction::North),
        payload: StructurePiecePayload::Template(data),
        ground_level_delta: 0,
        junctions: Vec::new(),
        projection: None,
    }
}

fn data(path: &'static str, at: IVec3) -> TemplatePieceData {
    TemplatePieceData {
        template_id: Identifier::vanilla_static(path),
        template_position: at,
        rotation: Rotation::None,
        mirror: StructureMirror::None,
        rotation_pivot: IVec3::new(0, 0, 0),
        block_ignore: StructureBlockIgnore::None,
        late_block_ignore: StructureBlockIgnore::None,
        processors: TemplateProcessorList::Empty,
        liquid_settings: LiquidSettingsData::ApplyWaterlogging,
        marker_handling: TemplateMarkerHandling::Ignore,
        placement_adjustment: TemplatePlacementAdjustment::None,
        placement_clip: TemplatePlacementClip::CenterChunk,
        post_process: TemplatePostProcess::None,
    }
}

fn shipwreck(at: IVec3, is_beached: bool) -> StructurePiece {
    let mut data = data("shipwreck/test", at);
    data.rotation_pivot = IVec3::new(4, 0, 15);
    data.block_ignore = StructureBlockIgnore::StructureAndAir;
    data.marker_handling = TemplateMarkerHandling::Shipwreck;
    data.placement_adjustment = TemplatePlacementAdjustment::Shipwreck {
        is_beached,
        height_adjusted: false,
    };
    template_piece(data, [4, 3, 5])
}

fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    state: PieceState,
) -> PieceState {
    match &piece.payload {
        StructurePiecePayload::Template(data) => match data.marker_handling {
            TemplateMarkerHandling::Shipwreck => {
                super::shipwreck::place(placing, piece, data, state)
            }
            TemplateMarkerHandling::Igloo => super::igloo::place(placing, piece, data, state),
            TemplateMarkerHandling::OceanRuin { is_large } => {
                super::ocean_ruin::place(placing, piece, data, is_large, state)
            }
            _ => super::ruined_portal::place(placing, data, state),
        },
        StructurePiecePayload::Procedural(ProceduralPieceData::BuriedTreasure) => {
            super::buried_treasure::place(placing, piece, state)
        }
        _ => unreachable!("the tests place their own families"),
    }
}

fn read(data: &[u8]) -> Nbt {
    let mut reader = NbtReader::new(data);
    let nbt = reader.nbt().unwrap().unwrap();
    reader.finish().unwrap();
    nbt
}

/// The loot table of the block entity at `at`, if it has one.
fn loot_table(world: &mut WithEntities, at: BlockPos) -> Option<String> {
    let entity = world.entities.get(&at)?;
    match read(&entity.data).get("LootTable") {
        Some(Nbt::String(table)) => Some(table.clone()),
        _ => None,
    }
}

fn written_ys(world: &WithEntities, state: BlockState) -> Vec<i32> {
    world
        .world
        .writes
        .iter()
        .filter(|(_, written)| *written == state)
        .map(|(at, _)| at.y)
        .collect()
}

#[test]
fn a_shipwreck_settles_on_the_mean_of_the_ocean_floor_and_moves_its_box_with_it() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 40));
    let piece = shipwreck(IVec3::new(2, 90, 2), false);
    let mut random = WorldgenRandom::from_seed(1);
    let state = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    // The ocean floor's height over the base is 41 everywhere.
    assert_eq!(
        state,
        PieceState {
            shift_y: 41 - 90,
            settled: true,
            flags: 0
        }
    );
    assert!(
        written_ys(&world, blocks::OAK_PLANKS)
            .iter()
            .all(|y| *y == 41)
    );
    // The marker is ignored, and the chest under it takes the supply's loot.
    assert!(written_ys(&world, blocks::STRUCTURE_BLOCK).is_empty());
    assert_eq!(
        loot_table(&mut world, BlockPos::new(3, 42, 3)).as_deref(),
        Some("minecraft:chests/shipwreck_supply")
    );
}

#[test]
fn a_settled_shipwreck_keeps_its_height_when_another_chunk_places_it() {
    // The ground differs under the second run; the wreck stays where it settled, as
    // the game's `heightAdjusted` keeps it.
    let piece = shipwreck(IVec3::new(2, 90, 2), false);
    let kept = PieceState {
        shift_y: 41 - 90,
        settled: true,
        flags: 0,
    };
    let mut world = WithEntities::new(World::flat(blocks::STONE, 70));
    let mut random = WorldgenRandom::from_seed(1);
    let state = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, kept)
    });
    assert_eq!(state, kept);
    assert!(
        written_ys(&world, blocks::OAK_PLANKS)
            .iter()
            .all(|y| *y == 41)
    );
}

#[test]
fn a_beached_shipwreck_sinks_by_half_its_height_and_a_draw() {
    let piece = shipwreck(IVec3::new(2, 90, 2), true);
    for seed in 0..8 {
        let mut world = WithEntities::new(World::flat(blocks::STONE, 40));
        let mut random = WorldgenRandom::from_seed(seed);
        let state = with_placing(&mut world, &mut random, |placing| {
            place(placing, &piece, PieceState::default())
        });
        let mut expected = WorldgenRandom::from_seed(seed);
        let y = 41 - 3 / 2 - expected.next_i32_bounded(3);
        assert_eq!(state.shift_y, y - 90, "seed {seed}");
    }
}

#[test]
fn a_shipwreck_across_a_chunk_s_edge_places_only_what_lies_in_the_chunk() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 40));
    let piece = shipwreck(IVec3::new(14, 90, 2), false);
    let mut random = WorldgenRandom::from_seed(1);
    with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    let xs: Vec<i32> = world
        .world
        .writes
        .iter()
        .filter(|(_, state)| *state == blocks::OAK_PLANKS)
        .map(|(at, _)| at.x)
        .collect();
    assert!(!xs.is_empty());
    assert!(xs.iter().all(|x| (14..=15).contains(x)));
}

#[test]
fn an_igloo_s_top_sits_one_into_the_surface_and_snow_closes_its_trapdoor() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 64));
    let mut data = data("igloo/top", IVec3::new(0, 90, 0));
    data.rotation_pivot = IVec3::new(3, 5, 5);
    data.liquid_settings = LiquidSettingsData::IgnoreWaterlogging;
    data.marker_handling = TemplateMarkerHandling::Igloo;
    data.placement_adjustment = TemplatePlacementAdjustment::Igloo {
        template_offset: (0, 0, 0),
    };
    data.post_process = TemplatePostProcess::IglooTop;
    let piece = template_piece(data, [7, 3, 8]);
    let mut random = WorldgenRandom::from_seed(3);
    let state = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    // The surface's height is 65, so the template stands at 64; its box moves there.
    assert_eq!(state.shift_y, 64 - 90);
    assert!(!state.settled);
    assert!(
        written_ys(&world, blocks::OAK_PLANKS)
            .iter()
            .all(|y| *y == 64)
    );
    // The trapdoor's place (3, 0, 5) has planks under it: snow.
    assert_eq!(world.world.get(BlockPos::new(3, 64, 5)), blocks::SNOW_BLOCK);
}

#[test]
fn an_igloo_s_laboratory_marker_becomes_air_over_a_chest_with_the_igloo_s_loot() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 64));
    // Twelve blocks down, as a laboratory four segments deep is.
    let mut data = data("igloo/bottom", IVec3::new(0, 90 - 3 - 12, 0));
    data.rotation_pivot = IVec3::new(3, 6, 7);
    data.liquid_settings = LiquidSettingsData::IgnoreWaterlogging;
    data.marker_handling = TemplateMarkerHandling::Igloo;
    data.placement_adjustment = TemplatePlacementAdjustment::Igloo {
        template_offset: (0, -3, -2),
    };
    let piece = template_piece(data, [7, 3, 9]);
    let mut random = WorldgenRandom::from_seed(3);
    let state = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    // Moved by the surface at the top's entrance, 65, less 91.
    let y = 90 - 3 - 12 + 65 - 90 - 1;
    assert_eq!(state.shift_y, 65 - 91);
    // The marker stood in the stone, which the structure block left; it becomes air.
    assert_eq!(world.world.get(BlockPos::new(1, y + 2, 1)), blocks::AIR);
    assert_eq!(
        loot_table(&mut world, BlockPos::new(1, y + 1, 1)).as_deref(),
        Some("minecraft:chests/igloo_chest")
    );
}

#[test]
fn a_marker_outside_the_centre_chunk_is_left_to_that_chunk() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 64));
    let mut data = data("igloo/bottom", IVec3::new(0, 75, -2));
    data.rotation_pivot = IVec3::new(3, 6, 7);
    data.marker_handling = TemplateMarkerHandling::Igloo;
    data.placement_adjustment = TemplatePlacementAdjustment::Igloo {
        template_offset: (0, -3, -2),
    };
    let piece = template_piece(data, [7, 3, 9]);
    let mut random = WorldgenRandom::from_seed(3);
    with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    // The marker at z -1 lies in chunk (0, -1): the stone there stays.
    assert_eq!(world.world.get(BlockPos::new(1, 51, -1)), blocks::STONE);
}

fn ruin(at: IVec3) -> StructurePiece {
    let mut data = data("underwater_ruin/test", at);
    data.late_block_ignore = StructureBlockIgnore::StructureAndAir;
    data.processors = TemplateProcessorList::OceanRuin {
        biome_temp: OceanRuinBiomeTempData::Warm,
        integrity: 1.0,
    };
    data.marker_handling = TemplateMarkerHandling::OceanRuin { is_large: false };
    data.placement_adjustment = TemplatePlacementAdjustment::OceanRuin;
    template_piece(data, [6, 3, 7])
}

#[test]
fn an_ocean_ruin_s_chest_marker_becomes_a_waterlogged_chest_with_the_small_ruin_s_loot() {
    // The sea up to 62 over stone to 40.
    let mut ground = World::flat(blocks::STONE, 40);
    for x in 0..16 {
        for z in 0..16 {
            for y in 41..=62 {
                ground.set(BlockPos::new(x, y, z), blocks::WATER);
            }
        }
    }
    let mut world = WithEntities::new(ground);
    let piece = ruin(IVec3::new(2, 90, 2));
    let mut random = WorldgenRandom::from_seed(9);
    let state = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    assert_eq!(state.shift_y, 41 - 90);
    let at = BlockPos::new(3, 43, 3);
    let chest = world.world.get(at);
    assert!(crate::blocks::same_block(chest, blocks::CHEST));
    assert_eq!(chest.property("waterlogged"), Some("true"));
    assert_eq!(
        loot_table(&mut world, at).as_deref(),
        Some("minecraft:chests/underwater_ruin_small")
    );
}

#[test]
fn a_drowned_marker_under_the_sea_becomes_water_and_draws_from_the_region_s_source() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 40));
    let mut data = data("underwater_ruin/drowned", IVec3::new(2, 90, 2));
    data.processors = TemplateProcessorList::OceanRuin {
        biome_temp: OceanRuinBiomeTempData::Cold,
        integrity: 1.0,
    };
    data.marker_handling = TemplateMarkerHandling::OceanRuin { is_large: true };
    data.placement_adjustment = TemplatePlacementAdjustment::OceanRuin;
    let piece = template_piece(data, [6, 3, 7]);
    let mut random = WorldgenRandom::from_seed(9);
    let settings = settings();
    let (seed, legacy) = (settings.seed(), settings.legacy_random_source());
    let next = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default());
        placing.level.region_random(seed, legacy).next_i64()
    });
    // The marker at (3, 43, 3) lies under the sea, at 63.
    assert_eq!(world.world.get(BlockPos::new(3, 43, 3)), blocks::WATER);
    // The region's source moved on by what the drowned drew, and by nothing else.
    let mut fresh = WithEntities::new(World::flat(blocks::STONE, 40));
    let mut unused = WorldgenRandom::from_seed(9);
    let expected = with_placing(&mut fresh, &mut unused, |placing| {
        let region = placing.level.region_random(seed, legacy);
        super::ocean_ruin::drowned_draws(region, false);
        region.next_i64()
    });
    assert_eq!(next, expected);
}

fn portal(at: IVec3, cold: bool) -> StructurePiece {
    let mut data = data("ruined_portal/test", at);
    data.block_ignore = StructureBlockIgnore::StructureAndAir;
    data.processors = TemplateProcessorList::RuinedPortal {
        vertical_placement: PortalPlacement::OnLandSurface,
        properties: RuinedPortalProperties {
            cold,
            mossiness: 0.0,
            air_pocket: false,
            overgrown: false,
            vines: false,
            replace_with_blackstone: false,
        },
    };
    data.placement_clip =
        TemplatePlacementClip::CenterChunkContainsTemplateCenterExpandedToTemplate;
    data.post_process = TemplatePostProcess::RuinedPortal;
    template_piece(data, [6, 3, 6])
}

#[test]
fn a_ruined_portal_is_placed_whole_by_the_chunk_that_holds_its_middle() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 64));
    // From x 12 to 17: the middle, x 15, is in chunk (0, 0); the portal reaches into
    // chunk (1, 0), and is placed there too.
    let piece = portal(IVec3::new(12, 65, 4), true);
    let mut random = WorldgenRandom::from_seed(4);
    with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    let xs: Vec<i32> = world
        .world
        .writes
        .iter()
        .filter(|(_, state)| *state == blocks::OAK_PLANKS)
        .map(|(at, _)| at.x)
        .collect();
    assert!(xs.contains(&16) && xs.contains(&17));
    // Netherrack spreads around it over the ground, and only cold: no magma.
    assert!(!written_ys(&world, blocks::NETHERRACK).is_empty());
    assert!(written_ys(&world, blocks::MAGMA_BLOCK).is_empty());
}

#[test]
fn a_ruined_portal_whose_middle_is_in_another_chunk_places_and_draws_nothing() {
    let mut world = WithEntities::new(World::flat(blocks::STONE, 64));
    let piece = portal(IVec3::new(13, 65, 4), false);
    let mut random = WorldgenRandom::from_seed(4);
    let state = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    assert_eq!(state, PieceState::default());
    assert!(world.world.writes.is_empty());
    assert_eq!(random.next_i64(), WorldgenRandom::from_seed(4).next_i64());
}

fn treasure(x: i32, z: i32) -> StructurePiece {
    let mut piece = StructurePiece::non_jigsaw(
        Identifier::new_static("minecraft", "btp"),
        BoundingBox::new(IVec3::new(x, 90, z), IVec3::new(x, 90, z)),
        0,
        None,
    );
    piece.payload = StructurePiecePayload::Procedural(ProceduralPieceData::BuriedTreasure);
    piece
}

#[test]
fn buried_treasure_lies_where_the_sand_meets_the_stone_and_the_box_shrinks_to_it() {
    // Sand from 60 to 62 over stone, a hole beside the treasure's block at 60.
    let mut ground = World::flat(blocks::STONE, 59);
    for x in 0..16 {
        for z in 0..16 {
            for y in 60..=62 {
                ground.set(BlockPos::new(x, y, z), blocks::SAND);
            }
        }
    }
    ground.set(BlockPos::new(10, 60, 9), blocks::WATER);
    ground.set(BlockPos::new(10, 59, 9), blocks::WATER);
    let mut world = WithEntities::new(ground);
    let piece = treasure(9, 9);
    let mut random = WorldgenRandom::from_seed(2);
    let state = with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    assert_eq!(state.shift_y, 60 - 90);
    let at = BlockPos::new(9, 60, 9);
    assert!(crate::blocks::same_block(
        world.world.get(at),
        blocks::CHEST
    ));
    // The open side over water takes the stone below the treasure.
    assert_eq!(world.world.get(BlockPos::new(10, 60, 9)), blocks::STONE);
    assert_eq!(
        loot_table(&mut world, at).as_deref(),
        Some("minecraft:chests/buried_treasure")
    );
    // The chest's seed is the one draw.
    let mut expected = WorldgenRandom::from_seed(2);
    let seed = expected.next_i64();
    let data = read(&world.entities[&at].data);
    assert_eq!(data.get("LootTableSeed"), Some(&Nbt::Long(seed)));
}

#[test]
fn buried_treasure_outside_the_centre_chunk_fills_but_makes_no_chest() {
    let mut ground = World::flat(blocks::STONE, 59);
    ground.set(BlockPos::new(20, 60, 9), blocks::SAND);
    let mut world = WithEntities::new(ground);
    let piece = treasure(20, 9);
    let mut random = WorldgenRandom::from_seed(2);
    with_placing(&mut world, &mut random, |placing| {
        place(placing, &piece, PieceState::default())
    });
    assert!(!crate::blocks::same_block(
        world.world.get(BlockPos::new(20, 60, 9)),
        blocks::CHEST
    ));
    assert_eq!(random.next_i64(), WorldgenRandom::from_seed(2).next_i64());
}
