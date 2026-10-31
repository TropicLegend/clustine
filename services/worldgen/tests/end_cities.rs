//! The End's cities placed with the End's land (step P10a of `docs/groundwork/p-plan.md`,
//! section 19.2), from the game's own templates: every block of every piece of a city
//! of seed 13579 where its pieces put it, the later pieces over the earlier, and the
//! chests under its `Chest` markers given the city's treasure. Whether the chunks are
//! the game's block for block, the seeds of the chests among them, is the judge's
//! question (`tests/game_features.rs`, `the_end_s_cities_...`).
//!
//! The templates' blocks are not in the repository: the test reads them from the jar
//! `cargo datagen` downloads, as a server reads its operator's.
//!
//! ```text
//! CLUSTINE_SERVER_JAR=target/datagen/26.3/server.jar \
//!     cargo test -p clustine-worldgen --test end_cities -- --ignored
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use clustine_data::{BlockState, blocks};
use clustine_protocol::codec::Reader as NbtReader;
use clustine_protocol::nbt::Nbt;
use clustine_structures::piece::{StructureBlockIgnore, StructurePiecePayload};
use clustine_structures::place::transform_pos;
use clustine_terrain::biome::Dimension;
use clustine_world::{BlockPos, ChunkPos, InChunk};
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::{FeatureStages, TerrainGenerator};

const SEED: i64 = 13579;

/// The chunk a city of the seed starts in (`clustine-structures`' `tests/starts.rs`,
/// which holds its pieces to the game's).
const CITY: ChunkPos = ChunkPos { x: 144, z: 146 };

/// What a column of pieces is expected to leave: each block, and the `Chest` markers'
/// chests below them.
struct Expected {
    blocks: BTreeMap<BlockPos, BlockState>,
    chests: Vec<BlockPos>,
}

/// The blocks the city's pieces put where, worked out from the templates alone: each
/// piece's palette (each of the city's templates has one), turned about its corner,
/// structure blocks left out, air left out where the piece does not overwrite, a later
/// piece over an earlier.
fn expected(
    start: &clustine_structures::Start,
    templates: &clustine_structures::template_blocks::TemplateLibrary,
) -> Expected {
    let mut found = Expected {
        blocks: BTreeMap::new(),
        chests: Vec::new(),
    };
    for piece in &start.pieces {
        let StructurePiecePayload::Template(data) = &piece.payload else {
            panic!("an End city is made of templates");
        };
        let template = templates
            .get(data.template_id.path)
            .unwrap_or_else(|| panic!("the jar has {}", data.template_id.path));
        assert_eq!(template.palettes.len(), 1, "{}", data.template_id.path);
        let overwrite = data.block_ignore == StructureBlockIgnore::StructureBlock;
        let corner = data.template_position;
        for block in &template.palettes[0] {
            let moved = transform_pos(
                block.pos,
                data.mirror,
                data.rotation,
                BlockPos::new(0, 0, 0),
            );
            let at = BlockPos::new(corner.x + moved.x, corner.y + moved.y, corner.z + moved.z);
            if block.state.block_id() == blocks::STRUCTURE_BLOCK.block_id() {
                let chest = block.nbt.as_ref().is_some_and(|nbt| {
                    matches!(nbt.get("metadata"), Some(Nbt::String(text)) if text.starts_with("Chest"))
                });
                if chest {
                    found.chests.push(BlockPos::new(at.x, at.y - 1, at.z));
                }
                continue;
            }
            if !overwrite && block.state.props().is_air() {
                continue;
            }
            found.blocks.insert(at, block.state);
        }
    }
    found
}

#[test]
#[ignore = "needs the server jar that `cargo datagen` downloads, named by CLUSTINE_SERVER_JAR"]
fn an_end_city_s_blocks_and_chests_stand_where_its_pieces_put_them() {
    let jar = std::env::var("CLUSTINE_SERVER_JAR").expect("CLUSTINE_SERVER_JAR names the jar");
    let templates = Arc::new(
        clustine_worldgen::templates::read_jar(std::path::Path::new(&jar))
            .expect("the jar's templates are read"),
    );
    let terrain = Arc::new(TerrainGenerator::new(SEED, Dimension::End));
    let starts = terrain.starts(CITY);
    let city = starts
        .iter()
        .find(|start| start.data().name == "minecraft:end_city")
        .expect("a city starts in the chunk");
    let expected = expected(city, &templates);
    let stages = FeatureStages::with_templates(Arc::clone(&terrain), templates);
    let scheduler = Scheduler::new(stages, Options::for_processors(4));

    let bounds = city.bounding_box;
    let mut chunks = BTreeMap::new();
    for x in bounds.min_x().div_euclid(16)..=bounds.max_x().div_euclid(16) {
        for z in bounds.min_z().div_euclid(16)..=bounds.max_z().div_euclid(16) {
            let at = ChunkPos::new(x, z);
            let chunk = scheduler.finished(at).expect("the land is made");
            assert!(!chunk.with_failure(), "{at:?}");
            chunks.insert(at, chunk);
        }
    }
    let mut differ = Vec::new();
    for (at, state) in &expected.blocks {
        let chunk = &chunks[&at.chunk()];
        let found = chunk
            .get(
                at.x.rem_euclid(16) as usize,
                at.y,
                at.z.rem_euclid(16) as usize,
            )
            .expect("within the End's height");
        // The shapes are settled later (the pieces place without knowing them), and
        // a state's turn is the placing's own: the block is what is judged here. On a
        // chunk's edge, what a run places has its shape brought up to date with what
        // stands beyond it (`StructureTemplate.updateShapeAtEdge`), which the chunk
        // beside may not have placed yet: a block that is no full cube may have fallen
        // away there, as a banner does whose wall is in the next chunk.
        let on_edge = [at.x, at.z]
            .iter()
            .any(|value| matches!(value.rem_euclid(16), 0 | 15));
        let fallen = on_edge && found.props().is_air() && !state.props().is_solid_render();
        if found.block_id() != state.block_id() && !fallen {
            differ.push(format!(
                "{at:?}: {:?} expected, {:?} found",
                state.block().map(|block| block.name),
                found.block().map(|block| block.name)
            ));
        }
    }
    assert!(
        differ.is_empty(),
        "{} of {} blocks differ, the first: {:?}",
        differ.len(),
        expected.blocks.len(),
        &differ[..differ.len().min(20)]
    );
    // Every city has chests, and each has the city's treasure.
    assert!(!expected.chests.is_empty());
    for at in &expected.chests {
        let chunk = &chunks[&at.chunk()];
        let inside = InChunk::new(
            at.x.rem_euclid(16) as usize,
            at.y,
            at.z.rem_euclid(16) as usize,
        );
        let entity = chunk
            .block_entities()
            .get(&inside)
            .unwrap_or_else(|| panic!("a chest at {at:?}"));
        let mut reader = NbtReader::new(&entity.data);
        let nbt = reader.nbt().unwrap().unwrap();
        assert_eq!(
            nbt.get("LootTable"),
            Some(&Nbt::String("minecraft:chests/end_city_treasure".into())),
            "{at:?}"
        );
        assert!(
            matches!(nbt.get("LootTableSeed"), Some(Nbt::Long(_))),
            "{at:?}"
        );
    }
    assert_eq!(scheduler.figures().failed_placements, 0);
}

#[test]
#[ignore = "needs the server jar that `cargo datagen` downloads, named by CLUSTINE_SERVER_JAR"]
fn the_city_s_templates_hold_only_the_markers_its_pieces_handle() {
    let jar = std::env::var("CLUSTINE_SERVER_JAR").expect("CLUSTINE_SERVER_JAR names the jar");
    let templates = clustine_worldgen::templates::read_jar(std::path::Path::new(&jar))
        .expect("the jar's templates are read");
    let table = clustine_worldgen_data::templates();
    let mut markers: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for index in 0..table.len() {
        let name = table.get(index).name;
        if !name.starts_with("end_city/") {
            continue;
        }
        let template = templates
            .get(name)
            .unwrap_or_else(|| panic!("the jar has {name}"));
        for block in template.palettes.iter().flatten() {
            if block.state.block_id() != blocks::STRUCTURE_BLOCK.block_id() {
                continue;
            }
            let Some(Nbt::String(text)) = block.nbt.as_ref().and_then(|nbt| nbt.get("metadata"))
            else {
                panic!("a marker of {name} names nothing");
            };
            let kind = ["Chest", "Sentry", "Elytra"]
                .into_iter()
                .find(|kind| text.starts_with(kind))
                .unwrap_or_else(|| panic!("{name} has the marker {text}"));
            *markers.entry((name, kind)).or_default() += 1;
        }
    }
    // The ship holds the one elytra; sentries and chests stand in the towers and houses.
    let elytras: Vec<_> = markers
        .iter()
        .filter(|((_, kind), _)| *kind == "Elytra")
        .collect();
    assert_eq!(elytras, [(&("end_city/ship", "Elytra"), &1)]);
    assert!(
        markers.keys().any(|(_, kind)| *kind == "Sentry"),
        "{markers:?}"
    );
    assert!(
        markers.keys().any(|(_, kind)| *kind == "Chest"),
        "{markers:?}"
    );
}
