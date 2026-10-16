//! The structures' blocks (`docs/groundwork/s-plan.md`): the pieces of the starts a
//! chunk refers to, placed in each step before the step's features, as the game's
//! `ChunkGenerator.applyBiomeDecoration` places them.
//!
//! The pieces themselves are made with the starts (`clustine-structures`, T7); what
//! is here places their blocks. A piece's placer is chosen by its payload. Until a
//! batch of the plan is built its placers place nothing and say so ([`built`]), and
//! the judge leaves out the chunks such a structure reaches.

mod buried_treasure;
mod igloo;
mod jigsaw;
mod ocean_ruin;
mod ruined_portal;
mod shipwreck;
mod template_piece;
#[cfg(test)]
mod template_tests;

use std::sync::Arc;

use clustine_noise::WorldgenRandom;
use clustine_structures::Start;
use clustine_structures::geometry::{BoundingBox, IVec3};
use clustine_structures::piece::{
    ProceduralPieceData, StructurePiecePayload, TemplateMarkerHandling, TemplatePostProcess,
};
use clustine_world::ChunkPos;
use clustine_world::generation::{PieceKey, PieceState};
use clustine_worldgen_data::structure::Kind;
use clustine_worldgen_data::structures::STRUCTURES;

use crate::context::Level;
use crate::settings::FeatureSettings;

/// Where a run finds the starts that its centre's references name: the starts of a
/// source chunk as T7 makes them, in the order of `Structures::starts`, so that a
/// reference's index is a place among them.
pub trait StartsOf: Send + Sync {
    fn starts(&self, source: ChunkPos) -> Arc<[Start]>;
}

/// What one piece is placed with: the game's arguments to `StructurePiece.postProcess`.
pub struct Placing<'p, 'l, 'v> {
    pub level: &'p mut Level<'v>,
    /// The run's random source, seeded for the structure as the game seeds it.
    pub random: &'p mut WorldgenRandom,
    pub settings: &'l FeatureSettings,
    /// The writable box of the centre chunk (`getWritableArea`): its columns, from one
    /// above the bottom of the world to its top.
    pub chunk_box: BoundingBox,
    pub centre: ChunkPos,
    /// The middle of the start's first piece at its lowest y, as the game passes it.
    pub pivot: IVec3,
}

/// Whether every piece a structure can have, by its place in the data's
/// `STRUCTURES`, has its placer built. Until it has, the judge counts the chunks
/// the structure reaches rather than comparing them.
#[must_use]
pub fn built(structure: usize) -> bool {
    // A jigsaw structure is pool elements only; its pools' features are the
    // features', all of the overworld's built.
    matches!(
        STRUCTURES[structure].kind,
        Kind::Jigsaw(_)
            | Kind::Shipwreck { .. }
            | Kind::Igloo
            | Kind::OceanRuin { .. }
            | Kind::RuinedPortal { .. }
            | Kind::BuriedTreasure
    )
}

/// Places the piece `key` of `start`, whose state between runs is `state`. Returns the
/// state as it is to be kept, which a piece that settles its height on first placing
/// changes.
pub(crate) fn place_piece(
    placing: &mut Placing<'_, '_, '_>,
    start: &Start,
    key: PieceKey,
    state: PieceState,
) -> PieceState {
    let piece = &start.pieces[usize::from(key.piece)];
    match &piece.payload {
        StructurePiecePayload::Jigsaw(data) => {
            jigsaw::place(placing, data);
        }
        StructurePiecePayload::Template(data) => match (data.marker_handling, data.post_process) {
            (TemplateMarkerHandling::Shipwreck, _) => {
                return shipwreck::place(placing, piece, data, state);
            }
            (TemplateMarkerHandling::Igloo, _) => return igloo::place(placing, piece, data, state),
            (TemplateMarkerHandling::OceanRuin { is_large }, _) => {
                return ocean_ruin::place(placing, piece, data, is_large, state);
            }
            (_, TemplatePostProcess::RuinedPortal) => {
                return ruined_portal::place(placing, data, state);
            }
            _ => {}
        },
        StructurePiecePayload::Procedural(ProceduralPieceData::BuriedTreasure) => {
            return buried_treasure::place(placing, piece, state);
        }
        StructurePiecePayload::Procedural(_) => {}
    }
    state
}

/// The start's `afterPlace`, once its pieces are placed: only the desert pyramid's and
/// the woodland mansion's do anything.
pub(crate) fn after_place(_placing: &mut Placing<'_, '_, '_>, _start: &Start) {}
