// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/template_piece.rs`
// (see NOTICE in this crate), the parts for shipwrecks, and checked against the pinned
// jar's `ShipwreckPieces$ShipwreckPiece`.

//! A shipwreck, beached or under water (`ShipwreckPieces.ShipwreckPiece.postProcess`):
//! the first time a chunk places it, the wreck settles on the ground below its
//! template's base, then it is placed as any template piece, with its three kinds of
//! chests given their loot.
//!
//! - It settles once (`heightAdjusted`): under water at the mean of the ocean floor's
//!   heights over its base, beached at the lowest of the surface's heights there, sunk
//!   by half its height and a draw of up to two more. The game moves the template and,
//!   as every template piece does when it is placed, the piece's box with it; both are
//!   kept for the runs of the other chunks the wreck reaches.
//! - Ignored are structure blocks and air; fluids are taken in (waterlogging).
//! - A marker `map_chest`, `supply_chest` or `treasure_chest` gives the container below
//!   it its loot table.

use clustine_noise::Random;
use clustine_structures::geometry::{BoundingBox, IVec3};
use clustine_structures::piece::{StructurePiece, TemplatePieceData, TemplatePlacementAdjustment};
use clustine_structures::processors::Processor;
use clustine_structures::template_blocks::TemplateBlocks;
use clustine_world::BlockPos;
use clustine_world::generation::PieceState;

use super::Placing;
use super::template_piece::{post_process, set_block_entity_loot_table, settings, template};
use crate::context::Heightmap;
use crate::features::caves::templates::FeatureShapes;

/// `isTooBigToFitInWorldGenRegion`: a template wider or taller than 32 blocks settles
/// when its start is made instead. None of the pinned jar's wrecks is.
fn too_big(template: &TemplateBlocks) -> bool {
    template.size[0] > 32 || template.size[1] > 32
}

/// The loot table a data marker names (`MARKERS_TO_LOOT`).
fn loot_of(marker: &str) -> Option<&'static str> {
    match marker {
        "map_chest" => Some("minecraft:chests/shipwreck_map"),
        "supply_chest" => Some("minecraft:chests/shipwreck_supply"),
        "treasure_chest" => Some("minecraft:chests/shipwreck_treasure"),
        _ => None,
    }
}

/// Places a shipwreck's piece. `state` says whether it has settled and how far its box
/// has moved; the state returned is to be kept.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    data: &TemplatePieceData,
    mut state: PieceState,
) -> PieceState {
    let TemplatePlacementAdjustment::Shipwreck { is_beached, .. } = data.placement_adjustment
    else {
        unreachable!("a shipwreck's piece settles as a shipwreck")
    };
    let Some(template) = template(placing, data.template_id.path) else {
        return state;
    };
    let start = data.template_position;
    let mut position = BlockPos::new(start.x, start.y + state.shift_y, start.z);
    if !state.settled && !too_big(template) {
        position.y = settled_y(placing, template, position, is_beached);
        state.settled = true;
    }

    let shapes = FeatureShapes {
        settings: placing.settings,
    };
    let ignore = Processor::structure_and_air_ignore();
    let mut settings = settings(placing, placing.chunk_box, &shapes);
    settings.rotation = data.rotation;
    settings.mirror = data.mirror;
    settings.rotation_pivot = to_block(data.rotation_pivot);
    settings.liquid_settings = data.liquid_settings;
    settings.processors.push(&ignore);
    let bounds: BoundingBox = post_process(
        placing,
        template,
        position,
        &settings,
        &mut |placing, marker, at| {
            if let Some(table) = loot_of(marker) {
                let below = BlockPos::new(at.x, at.y - 1, at.z);
                set_block_entity_loot_table(placing.level, &mut *placing.random, below, table);
            }
        },
    );
    state.shift_y = bounds.min_y() - piece.bounding_box.min_y();
    state
}

/// The height a wreck settles at: the mean of the heights over its base under water,
/// the lowest beached, sunk by half its height and a draw.
fn settled_y(
    placing: &mut Placing<'_, '_, '_>,
    template: &TemplateBlocks,
    position: BlockPos,
    beached: bool,
) -> i32 {
    let [size_x, size_y, size_z] = template.size;
    let map = if beached {
        Heightmap::WorldSurfaceWg
    } else {
        Heightmap::OceanFloorWg
    };
    let base = size_x * size_z;
    let mut lowest = placing.level.top_y();
    let mut mean = 0;
    if base == 0 {
        mean = placing.level.height_at(map, position.x, position.z);
    } else {
        for z in position.z..position.z + size_z {
            for x in position.x..position.x + size_x {
                let height = placing.level.height_at(map, x, z);
                mean += height;
                lowest = lowest.min(height);
            }
        }
        mean /= base;
    }
    if beached {
        // `calculateBeachedPosition`.
        lowest - size_y / 2 - placing.random.next_i32_bounded(3)
    } else {
        mean
    }
}

pub(super) fn to_block(at: IVec3) -> BlockPos {
    BlockPos::new(at.x, at.y, at.z)
}
