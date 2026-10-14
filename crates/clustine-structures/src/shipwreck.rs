// Adapted from SteelMC (steel-worldgen/src/structure/shipwreck.rs at commit
// 885c4b3e60ed79862c37311780774f76806cb714), which ports the game's own classes; the
// data it reads and the context it asks are Clustine's. See NOTICE in this crate.

//! Shipwreck: picks a random template from the beached (11) or underwater (20) pool,
//! places at `(chunkMinX, 90, chunkMinZ)` with random rotation and pivot `(4, 15)`.

use crate::compat::{LiquidSettingsData, StructureConfigData, StructureData};
use crate::geometry::IVec3;
use crate::geometry::{Direction, Identifier, Rotation};
use clustine_noise::random::LegacyRandom;
use clustine_noise::random::Random;

use crate::{
    GenerationStub, Structure, StructureBlockIgnore, StructureGenerationContext, StructureMirror,
    StructurePiece, StructurePiecePayload, TemplateMarkerHandling, TemplatePieceData,
    TemplatePlacementAdjustment, TemplatePlacementClip, TemplatePostProcess, TemplateProcessorList,
};

static BEACHED: &[&str] = &[
    "shipwreck/with_mast",
    "shipwreck/sideways_full",
    "shipwreck/sideways_fronthalf",
    "shipwreck/sideways_backhalf",
    "shipwreck/rightsideup_full",
    "shipwreck/rightsideup_fronthalf",
    "shipwreck/rightsideup_backhalf",
    "shipwreck/with_mast_degraded",
    "shipwreck/rightsideup_full_degraded",
    "shipwreck/rightsideup_fronthalf_degraded",
    "shipwreck/rightsideup_backhalf_degraded",
];

static OCEAN: &[&str] = &[
    "shipwreck/with_mast",
    "shipwreck/upsidedown_full",
    "shipwreck/upsidedown_fronthalf",
    "shipwreck/upsidedown_backhalf",
    "shipwreck/sideways_full",
    "shipwreck/sideways_fronthalf",
    "shipwreck/sideways_backhalf",
    "shipwreck/rightsideup_full",
    "shipwreck/rightsideup_fronthalf",
    "shipwreck/rightsideup_backhalf",
    "shipwreck/with_mast_degraded",
    "shipwreck/upsidedown_full_degraded",
    "shipwreck/upsidedown_fronthalf_degraded",
    "shipwreck/upsidedown_backhalf_degraded",
    "shipwreck/sideways_full_degraded",
    "shipwreck/sideways_fronthalf_degraded",
    "shipwreck/sideways_backhalf_degraded",
    "shipwreck/rightsideup_full_degraded",
    "shipwreck/rightsideup_fronthalf_degraded",
    "shipwreck/rightsideup_backhalf_degraded",
];

fn make_shipwreck_piece(
    template_id: Identifier,
    position: IVec3,
    rotation: Rotation,
    size: IVec3,
    is_beached: bool,
) -> StructurePiece {
    StructurePiece {
        piece_type: Identifier::new_static("minecraft", "shipwreck"),
        bounding_box: rotation.get_bounding_box_with_pivot(position, size, IVec3::new(4, 0, 15)),
        gen_depth: 0,
        orientation: Some(Direction::North),
        payload: StructurePiecePayload::Template(TemplatePieceData {
            template_id,
            template_position: position,
            rotation,
            mirror: StructureMirror::None,
            rotation_pivot: IVec3::new(4, 0, 15),
            block_ignore: StructureBlockIgnore::StructureAndAir,
            late_block_ignore: StructureBlockIgnore::None,
            processors: TemplateProcessorList::Empty,
            liquid_settings: LiquidSettingsData::ApplyWaterlogging,
            marker_handling: TemplateMarkerHandling::Shipwreck,
            placement_adjustment: TemplatePlacementAdjustment::Shipwreck {
                is_beached,
                height_adjusted: false,
            },
            placement_clip: TemplatePlacementClip::CenterChunk,
            post_process: TemplatePostProcess::None,
        }),
        ground_level_delta: 0,
        junctions: Vec::new(),
        projection: None,
    }
}

/// Registered under `"minecraft:shipwreck"`. Beached vs underwater is distinguished by
/// `entry.structure.path`.
pub struct ShipwreckStructure;

impl Structure for ShipwreckStructure {
    fn find_generation_point(
        &self,
        ctx: &mut dyn StructureGenerationContext,
        structure: &StructureData,
        rng: &mut LegacyRandom,
    ) -> Option<GenerationStub> {
        let surface_y = ctx.surface_y();
        let biome = ctx.biome_at(ctx.center_block_x(), surface_y, ctx.center_block_z());
        if !structure.allowed_biomes.contains(&biome.key) {
            return None;
        }

        let StructureConfigData::Shipwreck { is_beached } = &structure.config else {
            return None;
        };
        let templates_arr = if *is_beached { BEACHED } else { OCEAN };

        let rotation = Rotation::get_random(rng);
        let idx = rng.next_i32_bounded(templates_arr.len() as i32) as usize;
        let template_id = Identifier::vanilla_static(templates_arr[idx]);
        let tmpl = ctx.templates().get(&template_id)?;
        let position = IVec3::new(ctx.chunk_min_x(), 90, ctx.chunk_min_z());

        Some(GenerationStub {
            position: (ctx.center_block_x(), surface_y, ctx.center_block_z()),
            pieces: vec![make_shipwreck_piece(
                template_id,
                position,
                rotation,
                IVec3::from(tmpl.size),
                *is_beached,
            )],
        })
    }
}
