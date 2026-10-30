// Written from the pinned jar's `NetherFossilPieces$NetherFossilPiece`.

//! A Nether fossil's one piece (`NetherFossilPieces.NetherFossilPiece.postProcess`): a
//! template of bone blocks, turned as its start drew, placed whole by every chunk it
//! reaches, then perhaps a dried ghast beside it.
//!
//! - The game widens the box it was given (the start's own) to take in the template's,
//!   so each chunk the fossil reaches places all of it, into the chunks around.
//! - Ignored are structure blocks and air; the shape is not known, as for every
//!   template piece; the template has no data markers to hand on.
//! - The dried ghast (`placeDriedGhast`) draws from the world's seed's own random
//!   source at the middle of the template's box, never from the run's: half the time,
//!   at a column of the box drawn by x and then z, on the box's floor, where that is
//!   air and inside the widened box, it stands turned a random way. Each chunk draws
//!   the same, and the first to place it leaves no air for the others.

use clustine_data::blocks;
use clustine_noise::Random;
use clustine_structures::geometry::{BoundingBox, Rotation};
use clustine_structures::piece::TemplatePieceData;
use clustine_structures::place::template_bounding_box;
use clustine_structures::processors::Processor;
use clustine_structures::state_transform::rotate_state;
use clustine_world::BlockPos;
use clustine_world::generation::PieceState;

use super::Placing;
use super::scattered::seed_random_at;
use super::shipwreck::to_block;
use super::template_piece::{post_process, settings, template};
use crate::features::caves::templates::FeatureShapes;

/// Places a Nether fossil's piece. Its box is its template's from the start and does
/// not move; `state` is returned as it is.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    data: &TemplatePieceData,
    state: PieceState,
) -> PieceState {
    let Some(template) = template(placing, data.template_id.path) else {
        return state;
    };
    let position = to_block(data.template_position);
    let pivot = to_block(data.rotation_pivot);
    let bounds = template_bounding_box(template.size, position, data.mirror, data.rotation, pivot);
    let clip = BoundingBox::encapsulating(&placing.chunk_box, &bounds);

    let shapes = FeatureShapes {
        settings: placing.settings,
    };
    // `NetherFossilPiece.makeSettings`: the rotation drawn, no mirror, structure blocks
    // and air ignored.
    let ignore = Processor::structure_and_air_ignore();
    let mut settings = settings(placing, clip, &shapes);
    settings.rotation = data.rotation;
    settings.mirror = data.mirror;
    settings.rotation_pivot = pivot;
    settings.liquid_settings = data.liquid_settings;
    settings.processors.push(&ignore);
    // The fossil's `handleDataMarker` does nothing.
    post_process(placing, template, position, &settings, &mut |_, _, _| {});

    place_dried_ghast(placing, bounds, clip);
    state
}

/// `NetherFossilPiece.placeDriedGhast`: `fossil` is the template's box, `clip` the
/// widened box the template was placed into.
fn place_dried_ghast(placing: &mut Placing<'_, '_, '_>, fossil: BoundingBox, clip: BoundingBox) {
    let centre = fossil.center();
    let mut random = seed_random_at(placing, BlockPos::new(centre.x, centre.y, centre.z));
    if random.next_f32() >= 0.5 {
        return;
    }
    let x = fossil.min_x() + random.next_i32_bounded(fossil.max_x() - fossil.min_x() + 1);
    let y = fossil.min_y();
    let z = fossil.min_z() + random.next_i32_bounded(fossil.max_z() - fossil.min_z() + 1);
    let at = BlockPos::new(x, y, z);
    if !placing.level.block(at).props().is_air() || !clip.contains_xyz(x, y, z) {
        return;
    }
    let ghast = rotate_state(blocks::DRIED_GHAST, Rotation::get_random(&mut random));
    placing.level.set_block(at, ghast);
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, OnceLock};

    use clustine_data::BlockState;
    use clustine_noise::{LegacyRandom, PositionalRandom, WorldgenRandom};
    use clustine_structures::compat::LiquidSettingsData;
    use clustine_structures::geometry::{IVec3, Identifier};
    use clustine_structures::piece::{
        StructureBlockIgnore, StructureMirror, TemplateMarkerHandling, TemplatePlacementAdjustment,
        TemplatePlacementClip, TemplatePostProcess, TemplateProcessorList,
    };
    use clustine_structures::template_blocks::{TemplateBlock, TemplateBlocks, TemplateLibrary};
    use clustine_terrain::biome::Dimension;
    use clustine_world::ChunkPos;

    use super::*;
    use crate::context::Level;
    use crate::settings::FeatureSettings;
    use crate::testing::World;

    const WORLD_SEED: i64 = 13579;
    const RUN_SEED: u64 = 3;

    fn block(x: i32, y: i32, z: i32, state: BlockState) -> TemplateBlock {
        TemplateBlock {
            pos: BlockPos::new(x, y, z),
            state,
            nbt: None,
        }
    }

    /// Three by two by three: bone on the upper layer at three places; on the floor
    /// only air and a structure block, which the fossil's settings ignore.
    fn bones() -> TemplateBlocks {
        TemplateBlocks {
            size: [3, 2, 3],
            data_version: clustine_data::DATA_VERSION,
            palettes: vec![vec![
                block(0, 1, 0, blocks::BONE_BLOCK),
                block(2, 1, 0, blocks::BONE_BLOCK),
                block(2, 1, 2, blocks::BONE_BLOCK),
                block(0, 0, 0, blocks::AIR),
                block(2, 0, 0, blocks::STRUCTURE_BLOCK),
            ]],
            entities: Vec::new(),
        }
    }

    fn settings() -> &'static FeatureSettings {
        static SETTINGS: OnceLock<FeatureSettings> = OnceLock::new();
        SETTINGS.get_or_init(|| {
            let templates = vec![("nether_fossils/test".to_string(), bones())];
            FeatureSettings::new(WORLD_SEED, Dimension::Overworld)
                .expect("the data is read whole")
                .with_templates(Arc::new(TemplateLibrary::from_templates(templates)))
        })
    }

    /// The fossil's piece data as its start makes it (`NetherFossilPiece`).
    fn fossil(at: IVec3, rotation: Rotation) -> TemplatePieceData {
        TemplatePieceData {
            template_id: Identifier::vanilla_static("nether_fossils/test"),
            template_position: at,
            rotation,
            mirror: StructureMirror::None,
            rotation_pivot: IVec3::new(0, 0, 0),
            block_ignore: StructureBlockIgnore::StructureAndAir,
            late_block_ignore: StructureBlockIgnore::None,
            processors: TemplateProcessorList::Empty,
            liquid_settings: LiquidSettingsData::ApplyWaterlogging,
            marker_handling: TemplateMarkerHandling::Ignore,
            placement_adjustment: TemplatePlacementAdjustment::None,
            placement_clip: TemplatePlacementClip::CenterChunkExpandedToTemplate,
            post_process: TemplatePostProcess::NetherFossil,
        }
    }

    /// Places `data` as chunk (0, 0) does; returns the run's random source after.
    fn run(world: &mut World, data: &TemplatePieceData) -> WorldgenRandom {
        let settings = settings();
        let mut random = WorldgenRandom::from_seed(RUN_SEED);
        let mut level = Level::new(world, settings.zoom(), -64, 384);
        let mut placing = Placing {
            level: &mut level,
            random: &mut random,
            settings,
            chunk_box: BoundingBox::new(IVec3::new(0, -63, 0), IVec3::new(15, 319, 15)),
            centre: ChunkPos { x: 0, z: 0 },
            pivot: IVec3::new(0, 0, 0),
        };
        assert_eq!(
            place(&mut placing, data, PieceState::default()),
            PieceState::default()
        );
        random
    }

    /// Where the game's `placeDriedGhast` puts the ghast for the template's box
    /// `fossil`, and turned how, if it draws to place one: drawn from the world's seed
    /// at the box's middle, a chance of a half, then x, then z, then the turn.
    fn ghast_for(fossil: BoundingBox) -> Option<(BlockPos, BlockState)> {
        let centre = fossil.center();
        let mut random = LegacyRandom::from_seed(WORLD_SEED as u64)
            .next_positional()
            .at(centre.x, centre.y, centre.z);
        if random.next_f32() >= 0.5 {
            return None;
        }
        let x = fossil.min_x() + random.next_i32_bounded(fossil.max_x() - fossil.min_x() + 1);
        let z = fossil.min_z() + random.next_i32_bounded(fossil.max_z() - fossil.min_z() + 1);
        let facing = match Rotation::get_random(&mut random) {
            Rotation::None => "north",
            Rotation::Clockwise90 => "east",
            Rotation::Clockwise180 => "south",
            Rotation::CounterClockwise90 => "west",
        };
        let ghast = blocks::DRIED_GHAST
            .with_property("facing", facing)
            .expect("a dried ghast faces the four ways");
        Some((BlockPos::new(x, fossil.min_y(), z), ghast))
    }

    fn box_at(at: IVec3, size: [i32; 3]) -> BoundingBox {
        BoundingBox::new(
            at,
            IVec3::new(at.x + size[0] - 1, at.y + size[1] - 1, at.z + size[2] - 1),
        )
    }

    #[test]
    fn a_fossil_is_placed_whole_by_a_chunk_it_reaches_and_ignores_air_and_structure_blocks() {
        let mut world = World::flat(blocks::NETHERRACK, 10);
        // Something standing where the template has air: the air leaves it.
        world.set(BlockPos::new(14, 30, 4), blocks::GLOWSTONE);
        let mut random = run(&mut world, &fossil(IVec3::new(14, 30, 4), Rotation::None));
        assert_eq!(world.get(BlockPos::new(14, 31, 4)), blocks::BONE_BLOCK);
        // Into the chunk to the east.
        assert_eq!(world.get(BlockPos::new(16, 31, 4)), blocks::BONE_BLOCK);
        assert_eq!(world.get(BlockPos::new(16, 31, 6)), blocks::BONE_BLOCK);
        assert_eq!(world.get(BlockPos::new(14, 30, 4)), blocks::GLOWSTONE);
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state != blocks::STRUCTURE_BLOCK && *state != blocks::AIR)
        );
        // The run's own random source is left alone.
        let mut fresh = WorldgenRandom::from_seed(RUN_SEED);
        assert_eq!(random.next_i64(), fresh.next_i64());
    }

    #[test]
    fn a_fossil_turned_a_quarter_clockwise_lies_west_of_its_corner() {
        let mut world = World::flat(blocks::NETHERRACK, 10);
        run(
            &mut world,
            &fossil(IVec3::new(8, 30, 8), Rotation::Clockwise90),
        );
        // `StructureTemplate.transform` turns (x, z) to (-z, x).
        assert_eq!(world.get(BlockPos::new(8, 31, 8)), blocks::BONE_BLOCK);
        assert_eq!(world.get(BlockPos::new(8, 31, 10)), blocks::BONE_BLOCK);
        assert_eq!(world.get(BlockPos::new(6, 31, 10)), blocks::BONE_BLOCK);
        assert_eq!(world.get(BlockPos::new(10, 31, 8)), blocks::AIR);
    }

    #[test]
    fn a_dried_ghast_lies_on_the_fossil_s_floor_where_the_world_s_seed_draws_it() {
        // Some fossils get a ghast and some do not; try corners until both are seen.
        let (mut with, mut without) = (0, 0);
        for x in 0..12 {
            let at = IVec3::new(x, 30, 5);
            let mut world = World::flat(blocks::NETHERRACK, 10);
            run(&mut world, &fossil(at, Rotation::None));
            let ghasts: Vec<_> = world
                .writes
                .iter()
                .filter(|(_, state)| crate::blocks::same_block(*state, blocks::DRIED_GHAST))
                .copied()
                .collect();
            match ghast_for(box_at(at, [3, 2, 3])) {
                Some(expected) => {
                    with += 1;
                    assert_eq!(ghasts, [expected]);
                }
                None => {
                    without += 1;
                    assert!(ghasts.is_empty());
                }
            }
        }
        assert!(with > 0 && without > 0);
    }

    #[test]
    fn no_ghast_where_the_fossil_s_floor_is_not_air() {
        // Ground up to the fossil's floor: wherever the ghast is drawn to go, there is
        // netherrack.
        for x in 0..12 {
            let at = IVec3::new(x, 30, 5);
            let mut world = World::flat(blocks::NETHERRACK, 30);
            run(&mut world, &fossil(at, Rotation::None));
            assert!(
                world
                    .writes
                    .iter()
                    .all(|(_, state)| !crate::blocks::same_block(*state, blocks::DRIED_GHAST))
            );
        }
    }
}
