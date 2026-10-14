// Adapted from SteelMC (steel-worldgen/src/structure/single_piece.rs at commit
// 885c4b3e60ed79862c37311780774f76806cb714), which ports the game's own classes; the
// data it reads and the context it asks are Clustine's. See NOTICE in this crate.

//! Single-piece buried treasure structure start generation.

use crate::compat::StructureData;
use crate::geometry::IVec3;
use crate::geometry::{BoundingBox, Identifier};
use clustine_noise::random::LegacyRandom;

use crate::{
    GenerationStub, ProceduralPieceData, Structure, StructureGenerationContext, StructurePiece,
    StructurePiecePayload,
};

/// Single 1×1×1 piece at `(chunkMinX+9, 90, chunkMinZ+9)`. Biome check at ocean-floor Y.
pub struct BuriedTreasureStructure;

const fn buried_treasure_piece(x: i32, z: i32) -> StructurePiece {
    StructurePiece {
        piece_type: Identifier::new_static("minecraft", "btp"),
        bounding_box: BoundingBox::new(IVec3::new(x, 90, z), IVec3::new(x, 90, z)),
        gen_depth: 0,
        orientation: None,
        payload: StructurePiecePayload::Procedural(ProceduralPieceData::BuriedTreasure),
        ground_level_delta: 0,
        junctions: Vec::new(),
        projection: None,
    }
}

impl Structure for BuriedTreasureStructure {
    fn find_generation_point(
        &self,
        ctx: &mut dyn StructureGenerationContext,
        structure: &StructureData,
        _rng: &mut LegacyRandom,
    ) -> Option<GenerationStub> {
        let ocean_floor_y = ctx.base_height(ctx.center_block_x(), ctx.center_block_z(), true) - 1;
        let biome = ctx.biome_at(ctx.center_block_x(), ocean_floor_y, ctx.center_block_z());
        if !structure.allowed_biomes.contains(&biome.key) {
            return None;
        }

        let (x, z) = (ctx.chunk_min_x() + 9, ctx.chunk_min_z() + 9);
        Some(GenerationStub {
            position: (x, 90, z),
            pieces: vec![buried_treasure_piece(x, z)],
        })
    }
}
