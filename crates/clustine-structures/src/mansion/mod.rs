// Adapted from SteelMC (steel-worldgen/src/structure/mansion/mod.rs at commit
// 885c4b3e60ed79862c37311780774f76806cb714), which ports the game's own classes; the
// data it reads and the context it asks are Clustine's. See NOTICE in this crate.

//! Woodland mansion. Vanilla's `WoodlandMansionPieces`: grid-based layout with
//! template pieces for walls, corridors, rooms, and roofs.

use crate::compat::{LiquidSettingsData, StructureData};
use crate::geometry::IVec3;
use crate::geometry::{BoundingBox, Direction, Identifier, Rotation};
use clustine_noise::random::LegacyRandom;
use clustine_noise::random::Random;

use crate::{
    GenerationStub, Structure, StructureBlockIgnore, StructureGenerationContext, StructureMirror,
    StructurePiece, StructurePiecePayload, TemplateMarkerHandling, TemplatePieceData,
    TemplatePlacementAdjustment, TemplatePlacementClip, TemplatePostProcess, TemplateProcessorList,
};

mod grid;
mod placement;
mod roof;
mod rooms;
mod template;
mod walls;

use placement::generate_mansion_pieces;
use template::MansionTemplatePiece;

/// `Structure` impl — registered under `"minecraft:woodland_mansion"`.
///
/// Vanilla's `WoodlandMansionStructure.findGenerationPoint`: consumes a
/// rotation, probes a rotation-dependent 5×5 box for the lowest Y, rejects
/// if `< 60`, then runs `generate_mansion_pieces`.
pub struct WoodlandMansionStructure;

impl Structure for WoodlandMansionStructure {
    fn find_generation_point(
        &self,
        ctx: &mut dyn StructureGenerationContext,
        structure: &StructureData,
        rng: &mut LegacyRandom,
    ) -> Option<GenerationStub> {
        let rotation = Rotation::get_random(rng);

        let (off_x, off_z) = match rotation {
            Rotation::None => (5, 5),
            Rotation::Clockwise90 => (-5, 5),
            Rotation::Clockwise180 => (-5, -5),
            Rotation::CounterClockwise90 => (5, -5),
        };
        let bx = ctx.chunk_min_x() + 7;
        let bz = ctx.chunk_min_z() + 7;
        // The pinned jar takes the highest occupied block of each corner
        // (`Structure.getCornerHeights` asks `getFirstOccupiedHeight`), which is one
        // below what SteelMC takes: its mansions stand a block lower.
        let h0 = ctx.base_height(bx, bz, false) - 1;
        let h1 = ctx.base_height(bx, bz + off_z, false) - 1;
        let h2 = ctx.base_height(bx + off_x, bz, false) - 1;
        let h3 = ctx.base_height(bx + off_x, bz + off_z, false) - 1;
        let lowest = h0.min(h1).min(h2).min(h3);
        if lowest < 60 {
            return None;
        }

        let biome = ctx.biome_at(bx, lowest, bz);
        if !structure.allowed_biomes.contains(&biome.key) {
            return None;
        }

        let origin = IVec3::new(bx, lowest, bz);
        let pieces = generate_mansion_pieces(origin, rotation, rng)
            .into_iter()
            .map(MansionTemplatePiece::into_structure_piece)
            .collect();

        Some(GenerationStub {
            position: (origin.x, origin.y, origin.z),
            pieces,
        })
    }
}
