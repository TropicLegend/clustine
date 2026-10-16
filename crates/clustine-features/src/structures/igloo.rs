// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/template_piece.rs`
// (see NOTICE in this crate), the parts for igloos, and checked against the pinned jar's
// `IglooPieces$IglooPiece`.

//! An igloo's pieces, the top and, below it, the ladder's shaft and the laboratory
//! (`IglooPieces.IglooPiece.postProcess`). Each is placed afresh at the height of the
//! surface at the top's entrance, one block down, every time: the game moves its
//! template there for the placing alone and back after, but its box, which every
//! template piece sets again from where its template stands, stays moved.
//!
//! - Ignored are structure blocks; fluids are not taken in.
//! - The marker `chest` in the laboratory becomes air, and the chest below it gets the
//!   igloo's loot.
//! - Under the top's trapdoor, where neither air nor a ladder is below it (no shaft),
//!   the game puts a snow block.

use clustine_data::blocks;
use clustine_structures::piece::{
    StructurePiece, TemplatePieceData, TemplatePlacementAdjustment, TemplatePostProcess,
};
use clustine_structures::place::transform_pos;
use clustine_structures::processors::Processor;
use clustine_world::BlockPos;
use clustine_world::generation::PieceState;

use super::Placing;
use super::shipwreck::to_block;
use super::template_piece::{CHESTS, post_process, set_loot_table_if, settings, template};
use crate::blocks::same_block;
use crate::context::Heightmap;
use crate::features::caves::templates::FeatureShapes;

/// The height the pieces' templates are made at.
const GENERATION_Y: i32 = 90;

/// Places a piece of an igloo. `state` says how far its box has moved; the state
/// returned is to be kept.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    data: &TemplatePieceData,
    mut state: PieceState,
) -> PieceState {
    let TemplatePlacementAdjustment::Igloo { template_offset } = data.placement_adjustment else {
        unreachable!("an igloo's piece is placed as an igloo's")
    };
    let Some(template) = template(placing, data.template_id.path) else {
        return state;
    };
    let pivot = to_block(data.rotation_pivot);
    let start = to_block(data.template_position);
    let (offset_x, _, offset_z) = template_offset;
    // The top's entrance, by where this piece lies under it.
    let entrance = transform_pos(
        BlockPos::new(3 - offset_x, 0, -offset_z),
        data.mirror,
        data.rotation,
        pivot,
    );
    let surface = placing.level.height_at(
        Heightmap::WorldSurfaceWg,
        start.x + entrance.x,
        start.z + entrance.z,
    );
    let position = BlockPos::new(start.x, start.y + surface - GENERATION_Y - 1, start.z);

    let shapes = FeatureShapes {
        settings: placing.settings,
    };
    let ignore = Processor::structure_block_ignore();
    let mut settings = settings(placing, placing.chunk_box, &shapes);
    settings.rotation = data.rotation;
    settings.mirror = data.mirror;
    settings.rotation_pivot = pivot;
    settings.liquid_settings = data.liquid_settings;
    settings.processors.push(&ignore);
    let bounds = post_process(
        placing,
        template,
        position,
        &settings,
        &mut |placing, marker, at| {
            if marker != "chest" {
                return;
            }
            placing.level.set_block(at, blocks::AIR);
            let below = BlockPos::new(at.x, at.y - 1, at.z);
            set_loot_table_if(
                placing.level,
                &mut *placing.random,
                below,
                "minecraft:chests/igloo_chest",
                &CHESTS,
            );
        },
    );

    if data.post_process == TemplatePostProcess::IglooTop {
        let door = transform_pos(BlockPos::new(3, 0, 5), data.mirror, data.rotation, pivot);
        let door = BlockPos::new(
            position.x + door.x,
            position.y + door.y,
            position.z + door.z,
        );
        let below = placing
            .level
            .block(BlockPos::new(door.x, door.y - 1, door.z));
        if !below.props().is_air() && !same_block(below, blocks::LADDER) {
            placing.level.set_block(door, blocks::SNOW_BLOCK);
        }
    }
    state.shift_y = bounds.min_y() - piece.bounding_box.min_y();
    state
}
