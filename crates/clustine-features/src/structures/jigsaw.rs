// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/pool_element.rs`
// (see NOTICE in this crate), which follows the game's pool elements.

//! The pieces of jigsaw structures (`PoolElementStructurePiece.postProcess`): each is
//! one pool element placed where the layout put it, as the game's elements place
//! themselves.
//!
//! - A single element is its template, placed clipped to the centre chunk with its
//!   shape known, through the processors the game adds in its order: the structure
//!   block ignored, jigsaws replaced by what they say they become, the element's own
//!   list, then what its projection adds (gravity onto the terrain for one that
//!   follows the ground). A legacy element ignores air and structure blocks last
//!   instead of structure blocks first (`LegacySinglePoolElement.getSettings`).
//! - A feature element places its placed feature at the piece's position, with the
//!   run's random source, and no top feature: the game's `PlacedFeature.place`.
//! - A list places its elements in turn and stops at the first that places nothing.
//!
//! After a single element is placed the game runs its data markers through the
//! processors and hands them to `handleDataMarker`, which does nothing for these
//! elements; the processors write nothing and draw only from their own positional
//! sources, so that is left out.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use clustine_structures::geometry::{IVec3, Rotation};
use clustine_structures::jigsaw::JigsawPieceData;
use clustine_structures::place::{StructurePlaceSettings, place_in_world};
use clustine_structures::processors::{PlaceRandom, Processor, ProcessorList};
use clustine_world::BlockPos;
use clustine_worldgen_data::feature as data;
use clustine_worldgen_data::structure::{LiquidSettings, PoolElement, Projection};

use super::Placing;
use crate::context::Context;
use crate::features::caves::templates::read_processors;
use crate::reader::Reader;

/// Places a jigsaw piece. Returns whether anything was placed, as the game's elements
/// do; the start goes on with its other pieces either way.
pub(crate) fn place(placing: &mut Placing<'_, '_, '_>, piece: &JigsawPieceData) -> bool {
    place_element(
        placing,
        piece.pool_element,
        piece.position,
        piece.rotation,
        piece.liquid_settings,
    )
}

fn place_element(
    placing: &mut Placing<'_, '_, '_>,
    element: &PoolElement,
    position: IVec3,
    rotation: Rotation,
    liquids: LiquidSettings,
) -> bool {
    match element {
        PoolElement::Empty => true,
        PoolElement::Single {
            template,
            processors,
            projection,
            legacy,
        } => single(
            placing,
            usize::from(*template),
            processors,
            *projection,
            *legacy,
            position,
            rotation,
            liquids,
        ),
        PoolElement::Feature { feature, .. } => {
            let (index, _) =
                data::placed(feature).expect("a pool names a placed feature of the data");
            let settings = placing.settings;
            let mut context = Context {
                level: &mut *placing.level,
                random: &mut *placing.random,
                settings,
                top: None,
            };
            settings.placed(index).place(
                &mut context,
                BlockPos::new(position.x, position.y, position.z),
            )
        }
        PoolElement::List { elements, .. } => elements
            .iter()
            .all(|member| place_element(placing, member, position, rotation, liquids)),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the game's SinglePoolElement.place takes as many"
)]
fn single(
    placing: &mut Placing<'_, '_, '_>,
    template: usize,
    processors: &str,
    projection: Projection,
    legacy: bool,
    position: IVec3,
    rotation: Rotation,
    liquids: LiquidSettings,
) -> bool {
    // Without the operator's jar no template's block is placed, as for features.
    let Some(library) = placing.settings.templates() else {
        return false;
    };
    let name = clustine_worldgen_data::templates().get(template).name;
    let Some(blocks) = library.get(name) else {
        return false;
    };

    let ignore_structure = Processor::structure_block_ignore();
    let ignore_structure_and_air = Processor::structure_and_air_ignore();
    let jigsaws = Processor::JigsawReplacement;
    let gravity = Processor::terrain_matching();
    let mut settings = StructurePlaceSettings::new(placing.settings.seed());
    settings.rotation = rotation;
    settings.bounding_box = Some(placing.chunk_box);
    settings.known_shape = true;
    settings.finalize_entities = true;
    settings.liquid_settings = liquids;
    // The game places pool elements with the flags 18, which hold `UPDATE_KNOWN_SHAPE`.
    settings.marks = false;
    settings.random = PlaceRandom::Positional;
    if !legacy {
        settings.processors.push(&ignore_structure);
    }
    settings.processors.push(&jigsaws);
    if !processors.is_empty() {
        settings
            .processors
            .extend(named(processors).processors.iter());
    }
    if projection == Projection::TerrainMatching {
        settings.processors.push(&gravity);
    }
    if legacy {
        settings.processors.push(&ignore_structure_and_air);
    }
    let at = BlockPos::new(position.x, position.y, position.z);
    let pivot = BlockPos::new(placing.pivot.x, placing.pivot.y, placing.pivot.z);
    place_in_world(
        blocks,
        placing.level.view(),
        at,
        pivot,
        &settings,
        &mut *placing.random,
    )
}

/// The processor list of the data called `name`, read once.
fn named(name: &str) -> &'static ProcessorList {
    static LISTS: OnceLock<BTreeMap<&'static str, ProcessorList>> = OnceLock::new();
    let lists = LISTS.get_or_init(|| {
        data::processor_lists()
            .iter()
            .map(|entry| {
                let list = read_processors(&Reader::new(&entry.data, entry.name))
                    .expect("every processor list of the data reads whole");
                (entry.name, list)
            })
            .collect()
    });
    lists
        .get(name)
        .unwrap_or_else(|| panic!("a pool names {name}, which is not a processor list"))
}
