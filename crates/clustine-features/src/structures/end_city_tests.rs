//! Tests of the End city's placer (`EndCityPieces.EndCityPiece`), in the toy world with
//! block entities, with a template made by hand: its chests' loot and seed, its sentry
//! shulkers, the elytra's item frame on the ship, and the two kinds of ignoring.

use std::sync::{Arc, OnceLock};

use clustine_data::{BlockState, blocks};
use clustine_noise::{Random, WorldgenRandom};
use clustine_protocol::codec::Reader as NbtReader;
use clustine_protocol::nbt::Nbt;
use clustine_structures::compat::LiquidSettingsData;
use clustine_structures::geometry::{BoundingBox, IVec3, Identifier, Rotation};
use clustine_structures::piece::{
    StructureBlockIgnore, StructureMirror, TemplateMarkerHandling, TemplatePieceData,
    TemplatePlacementAdjustment, TemplatePlacementClip, TemplatePostProcess, TemplateProcessorList,
};
use clustine_structures::template_blocks::{TemplateBlock, TemplateBlocks, TemplateLibrary};
use clustine_terrain::biome::Dimension;
use clustine_world::{BlockPos, ChunkPos};
use clustine_worldgen_data::structures::STRUCTURES;

use super::Placing;
use super::end_city::{LOOT, place};
use crate::context::Level;
use crate::features::ground::shared::testing::WithEntities;
use crate::settings::FeatureSettings;
use crate::testing::{ToyMaker, World};

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

/// A floor of purpur six by six, air at (4, 1, 4), a chest at (1, 1, 1) with the marker
/// `Chest` above it, a sentry's marker at (3, 1, 3) and the elytra's at (2, 1, 4): what
/// the city's templates hold, the ship's frame and the towers' shulkers in one.
fn city() -> TemplateBlocks {
    let mut palette = Vec::new();
    for x in 0..6 {
        for z in 0..6 {
            palette.push(block(x, 0, z, blocks::PURPUR_BLOCK, None));
        }
    }
    // Sorted as the game sorts a palette: full blocks, the others, those with data.
    palette.push(block(4, 1, 4, blocks::AIR, None));
    palette.push(block(
        1,
        1,
        1,
        blocks::CHEST,
        Some(compound(vec![(
            "id",
            Nbt::String("minecraft:chest".into()),
        )])),
    ));
    palette.push(block(1, 2, 1, blocks::STRUCTURE_BLOCK, marker("Chest")));
    palette.push(block(3, 1, 3, blocks::STRUCTURE_BLOCK, marker("Sentry")));
    palette.push(block(2, 1, 4, blocks::STRUCTURE_BLOCK, marker("Elytra")));
    TemplateBlocks {
        size: [6, 3, 6],
        data_version: clustine_data::DATA_VERSION,
        palettes: vec![palette],
        entities: Vec::new(),
    }
}

fn settings() -> &'static FeatureSettings {
    static SETTINGS: OnceLock<FeatureSettings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        FeatureSettings::new(13579, Dimension::End)
            .expect("the data is read whole")
            .with_templates(Arc::new(TemplateLibrary::from_templates(vec![(
                "end_city/test".to_string(),
                city(),
            )])))
    })
}

/// The writable box of chunk (0, 0).
fn chunk_box() -> BoundingBox {
    BoundingBox::new(IVec3::new(0, -63, 0), IVec3::new(15, 319, 15))
}

/// A piece as `EndCityPieces` makes one: the template at `at`, turned by `rotation`,
/// overwriting or not.
fn piece(at: IVec3, rotation: Rotation, overwrite: bool) -> TemplatePieceData {
    TemplatePieceData {
        template_id: Identifier::vanilla_static("end_city/test"),
        template_position: at,
        rotation,
        mirror: StructureMirror::None,
        rotation_pivot: IVec3::new(0, 0, 0),
        block_ignore: if overwrite {
            StructureBlockIgnore::StructureBlock
        } else {
            StructureBlockIgnore::StructureAndAir
        },
        late_block_ignore: StructureBlockIgnore::None,
        processors: TemplateProcessorList::Empty,
        liquid_settings: LiquidSettingsData::ApplyWaterlogging,
        marker_handling: TemplateMarkerHandling::EndCity,
        placement_adjustment: TemplatePlacementAdjustment::None,
        placement_clip: TemplatePlacementClip::CenterChunk,
        post_process: TemplatePostProcess::None,
    }
}

/// What a placing of `data` in chunk (0, 0) left: the world, the next draw of the
/// piece's random source (seeded with 7), and the next draw of the region's.
fn placed(world: &mut WithEntities, data: &TemplatePieceData) -> (i64, i64) {
    let settings = settings();
    let maker = ToyMaker {
        mobs: &[],
        draws: true,
    };
    let makings = clustine_world::generation::Makings::new(0, ChunkPos { x: 0, z: 0 });
    let mut level = Level::new(world, settings.zoom(), -64, 384).with_maker(&maker, makings);
    let mut random = WorldgenRandom::from_seed(7);
    let mut placing = Placing {
        level: &mut level,
        random: &mut random,
        settings,
        chunk_box: chunk_box(),
        centre: ChunkPos { x: 0, z: 0 },
        pivot: IVec3::new(0, 0, 0),
    };
    place(&mut placing, data);
    let region = placing
        .level
        .region_random(settings.seed(), settings.legacy_random_source())
        .next_i64();
    (random.next_i64(), region)
}

/// The draws of fresh sources: the piece's seeded with 7, the region's of chunk (0, 0).
fn fresh_draws() -> [i64; 2] {
    let settings = settings();
    let mut piece = WorldgenRandom::from_seed(7);
    let mut fresh = WithEntities::new(World::flat(blocks::STONE, 0));
    let mut level = Level::new(&mut fresh, settings.zoom(), -64, 384);
    let region = level
        .region_random(settings.seed(), settings.legacy_random_source())
        .next_i64();
    [piece.next_i64(), region]
}

fn read(data: &[u8]) -> Nbt {
    let mut reader = NbtReader::new(data);
    let nbt = reader.nbt().unwrap().unwrap();
    reader.finish().unwrap();
    nbt
}

/// What a toy entity was handed: its tag, and whether it was to be finalised.
fn handed(entity: &clustine_world::Entity) -> (Nbt, bool) {
    let (tag, finalised) = ToyMaker::handed(entity);
    (read(tag), finalised)
}

#[test]
fn the_end_city_is_built() {
    let city = STRUCTURES
        .iter()
        .position(|structure| structure.name == "minecraft:end_city")
        .expect("the End city is a structure of the data");
    assert!(super::built(city));
}

#[test]
fn a_chest_under_a_chest_marker_takes_the_city_s_treasure_and_the_marker_s_draw_as_its_seed() {
    let mut world = WithEntities::new(World::flat(blocks::END_STONE, 63));
    let (next, _) = placed(
        &mut world,
        &piece(IVec3::new(2, 64, 2), Rotation::None, true),
    );
    let chest = BlockPos::new(3, 65, 3);
    assert_eq!(world.world.get(chest).block_id(), blocks::CHEST.block_id());
    let saved = read(&world.entities[&chest].data);
    assert_eq!(saved.get("LootTable"), Some(&Nbt::String(LOOT.into())));
    // The template's chest drew a seed of the piece's random source as it was placed
    // (`StructureTemplate.placeInWorld`), and the marker's
    // `RandomizableContainer.setBlockEntityLootTable` draws the next, which it keeps.
    let mut random = WorldgenRandom::from_seed(7);
    let _placed = random.next_i64();
    assert_eq!(
        saved.get("LootTableSeed"),
        Some(&Nbt::Long(random.next_i64()))
    );
    assert_eq!(next, random.next_i64());
    // The markers are structure blocks, which every piece's settings ignore.
    assert!(
        world
            .world
            .writes
            .iter()
            .all(|(_, state)| state.block_id() != blocks::STRUCTURE_BLOCK.block_id())
    );
}

#[test]
fn a_chest_below_the_box_is_left_without_loot_and_nothing_is_drawn() {
    // The marker stands on the box's lowest layer, the chest below it outside.
    let mut world = WithEntities::new(World::flat(blocks::END_STONE, -70));
    let (next, _) = placed(
        &mut world,
        &piece(IVec3::new(2, -65, 2), Rotation::None, true),
    );
    assert!(world.entities.is_empty());
    assert_eq!(next, fresh_draws()[0]);
}

#[test]
fn a_sentry_marker_makes_a_shulker_standing_at_its_middle_as_a_template_s_entity() {
    let mut world = WithEntities::new(World::flat(blocks::END_STONE, 63));
    let (_, region) = placed(
        &mut world,
        &piece(IVec3::new(2, 64, 2), Rotation::None, true),
    );
    let made = &world.world.made;
    let shulkers: Vec<_> = made
        .iter()
        .filter(|entity| {
            handed(entity).0.get("id") == Some(&Nbt::String("minecraft:shulker".into()))
        })
        .collect();
    assert_eq!(shulkers.len(), 1);
    let shulker = shulkers[0];
    // The marker at (3, 1, 3) of the template at (2, 64, 2).
    assert_eq!(shulker.position, [5.5, 65.0, 5.5]);
    assert_eq!((shulker.yaw, shulker.pitch), (0.0, 0.0));
    let (tag, finalised) = handed(shulker);
    // The game calls neither `finalizeSpawn` nor `setPersistenceRequired`, and its
    // face is the default, downwards: the tag names none.
    assert!(!finalised);
    assert_eq!(tag.get("PersistenceRequired"), None);
    assert_eq!(tag.get("AttachFace"), None);
    assert_eq!(
        tag.get("Pos"),
        Some(&Nbt::List(vec![
            Nbt::Double(5.5),
            Nbt::Double(65.0),
            Nbt::Double(5.5)
        ]))
    );
    // Neither the shulker nor the frame draws from the region's source.
    assert_eq!(region, fresh_draws()[1]);
}

#[test]
fn an_elytra_marker_makes_an_item_frame_facing_south_turned_with_the_piece_holding_an_elytra() {
    for (rotation, facing, facing_value, yaw) in [
        (Rotation::None, [0, 1], 3, 0.0),
        (Rotation::Clockwise90, [-1, 0], 4, 90.0),
        (Rotation::Clockwise180, [0, -1], 2, 180.0),
        (Rotation::CounterClockwise90, [1, 0], 5, 270.0),
    ] {
        let mut world = WithEntities::new(World::flat(blocks::END_STONE, 63));
        // The template's corner in the middle of the chunk, so that every turn keeps
        // the marker inside it.
        placed(&mut world, &piece(IVec3::new(8, 64, 8), rotation, true));
        let frames: Vec<_> = world
            .world
            .made
            .iter()
            .filter(|entity| {
                handed(entity).0.get("id") == Some(&Nbt::String("minecraft:item_frame".into()))
            })
            .collect();
        assert_eq!(frames.len(), 1, "{rotation:?}");
        let frame = frames[0];
        let (tag, finalised) = handed(frame);
        assert!(!finalised);
        // The marker at (2, 1, 4), turned about the template's corner.
        let at = clustine_structures::place::transform_pos(
            BlockPos::new(2, 1, 4),
            StructureMirror::None,
            rotation,
            BlockPos::new(0, 0, 0),
        );
        let at = BlockPos::new(at.x + 8, at.y + 64, at.z + 8);
        // At the middle of its block, moved 0.46875 back against its facing.
        let expected = [
            f64::from(at.x) + 0.5 - 0.46875 * f64::from(facing[0]),
            f64::from(at.y) + 0.5,
            f64::from(at.z) + 0.5 - 0.46875 * f64::from(facing[1]),
        ];
        assert_eq!(frame.position, expected, "{rotation:?}");
        assert_eq!((frame.yaw, frame.pitch), (yaw, 0.0), "{rotation:?}");
        assert_eq!(tag.get("Facing"), Some(&Nbt::Byte(facing_value)));
        assert_eq!(
            tag.get("block_pos"),
            Some(&Nbt::IntArray(vec![at.x, at.y, at.z]))
        );
        let Some(item) = tag.get("Item") else {
            panic!("the frame holds an item");
        };
        assert_eq!(
            item.get("id"),
            Some(&Nbt::String("minecraft:elytra".into()))
        );
        assert_eq!(item.get("count"), Some(&Nbt::Int(1)));
        assert_eq!(tag.get("ItemRotation"), Some(&Nbt::Byte(0)));
        assert_eq!(tag.get("ItemDropChance"), Some(&Nbt::Float(1.0)));
        assert_eq!(tag.get("Fixed"), Some(&Nbt::Byte(0)));
        assert_eq!(tag.get("Invisible"), Some(&Nbt::Byte(0)));
    }
}

#[test]
fn markers_outside_the_centre_chunk_make_nothing_there() {
    // The template's corner at x 14: the sentry's marker at x 17 and the elytra's at
    // x 16 lie beyond the chunk's edge, in the run of the chunk beside it.
    let mut world = WithEntities::new(World::flat(blocks::END_STONE, 63));
    placed(
        &mut world,
        &piece(IVec3::new(14, 64, 2), Rotation::None, true),
    );
    assert!(world.world.made.is_empty());
}

#[test]
fn a_piece_that_overwrites_places_its_air_and_one_that_does_not_keeps_what_stands() {
    for (overwrite, expected) in [(true, blocks::AIR), (false, blocks::END_STONE)] {
        let mut world = WithEntities::new(World::flat(blocks::STONE, 63));
        // The template's air at (4, 1, 4), over a block that stands there.
        let inside = BlockPos::new(6, 65, 6);
        world.world.set(inside, blocks::END_STONE);
        placed(
            &mut world,
            &piece(IVec3::new(2, 64, 2), Rotation::None, overwrite),
        );
        assert_eq!(world.world.get(inside), expected, "overwrite {overwrite}");
        assert_eq!(
            world.world.get(BlockPos::new(2, 64, 2)),
            blocks::PURPUR_BLOCK
        );
    }
}
