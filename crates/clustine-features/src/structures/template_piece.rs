// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/template_piece.rs`
// (see NOTICE in this crate), and checked against the pinned jar's
// `TemplateStructurePiece`, `StructureTemplate.filterBlocks` and
// `RandomizableContainer`, which it follows where they differ.

//! What every piece made of one template does when it is placed
//! (`TemplateStructurePiece.postProcess`): its box set again from where its template now
//! stands, the template placed clipped to the box it is given, then each of its data
//! markers handed to the piece, and each of its jigsaws replaced by the state it says it
//! becomes.
//!
//! The pieces of this kind place with the flags 2: the shape is not known, and what
//! asks for it is marked for post-processing. None of them gives its settings a random
//! source, so the palette, the processors and the markers' palette all draw from the
//! template's position (`StructurePlaceSettings.getRandom`).

use clustine_noise::Random;
use clustine_protocol::codec::Reader as NbtReader;
use clustine_protocol::nbt::Nbt;
use clustine_structures::geometry::BoundingBox;
use clustine_structures::place::{
    StructurePlaceSettings, place_in_world, template_bounding_box, transform_pos,
};
use clustine_structures::processors::{PlaceRandom, parse_block_state, position_random};
use clustine_structures::template_blocks::TemplateBlocks;
use clustine_world::BlockPos;

use super::Placing;
use crate::context::Level;
use crate::features::caves::templates::FeatureShapes;
use crate::features::ground::shared::{block_entity_type, compound};

/// The template called `name` (with or without `minecraft:`), from the operator's jar.
/// None where the jar is not there: then nothing of a template is placed, as for the
/// features made of templates.
pub(super) fn template<'l>(
    placing: &Placing<'_, 'l, '_>,
    name: &str,
) -> Option<&'l TemplateBlocks> {
    placing.settings.templates()?.get(name)
}

/// Settings as a template piece starts from: the game's defaults with the box to clip
/// to, the shape not known and brought up to date by `shapes`, places marked, each
/// position its own random source.
pub(super) fn settings<'a>(
    placing: &Placing<'_, '_, '_>,
    clip: BoundingBox,
    shapes: &'a FeatureShapes<'a>,
) -> StructurePlaceSettings<'a> {
    let mut settings = StructurePlaceSettings::new(placing.settings.seed());
    settings.shapes = shapes;
    settings.bounding_box = Some(clip);
    settings.known_shape = false;
    settings.marks = true;
    settings.random = PlaceRandom::Positional;
    settings
}

/// `TemplateStructurePiece.postProcess`: places `template` with its corner at
/// `position`, then hands each data marker inside the box (its metadata and where it
/// stands) to `marker`, then replaces each jigsaw inside the box. Returns the piece's
/// box as the game sets it from the position, which the caller keeps.
pub(super) fn post_process(
    placing: &mut Placing<'_, '_, '_>,
    template: &TemplateBlocks,
    position: BlockPos,
    settings: &StructurePlaceSettings<'_>,
    marker: &mut dyn FnMut(&mut Placing<'_, '_, '_>, &str, BlockPos),
) -> BoundingBox {
    let bounds = template_bounding_box(
        template.size,
        position,
        settings.mirror,
        settings.rotation,
        settings.rotation_pivot,
    );
    let pivot = BlockPos::new(placing.pivot.x, placing.pivot.y, placing.pivot.z);
    let Some(entities) = place_in_world(
        template,
        placing.level.view(),
        position,
        pivot,
        settings,
        &mut *placing.random,
    ) else {
        return bounds;
    };
    // The igloo's cleric and its zombie, which its settings do not finalise: made as
    // their template saved them, drawing nothing.
    placing
        .level
        .make_template_entities(placing.settings, entities);
    for (at, nbt) in filter_blocks(template, position, settings, "minecraft:structure_block") {
        let Some(nbt) = nbt else { continue };
        // `StructureMode.LEGACY_CODEC` reads the mode by its name.
        if !matches!(nbt.get("mode"), Some(Nbt::String(mode)) if mode == "DATA") {
            continue;
        }
        let metadata = match nbt.get("metadata") {
            Some(Nbt::String(text)) => text.as_str(),
            _ => "",
        };
        marker(placing, metadata, at);
    }
    for (at, nbt) in filter_blocks(template, position, settings, "minecraft:jigsaw") {
        let Some(nbt) = nbt else { continue };
        let text = match nbt.get("final_state") {
            Some(Nbt::String(text)) => text.as_str(),
            _ => "minecraft:air",
        };
        // A state the game cannot read leaves air, with an error in its log.
        let state = parse_block_state(text).unwrap_or(clustine_data::blocks::AIR);
        placing.level.set_block(at, state);
    }
    bounds
}

/// `StructureTemplate.filterBlocks`: the blocks of `block` in the palette the template's
/// position chooses, moved to where they stand in the world, those outside the box left
/// out, in the palette's order; with the data each has.
fn filter_blocks<'t>(
    template: &'t TemplateBlocks,
    position: BlockPos,
    settings: &StructurePlaceSettings<'_>,
    block: &str,
) -> Vec<(BlockPos, Option<&'t Nbt>)> {
    if template.palettes.is_empty() {
        return Vec::new();
    }
    // `getRandomPalette` with the template's position, as `placeInWorld` chose it.
    let count = i32::try_from(template.palettes.len()).expect("fewer than 2^31 palettes");
    let index = match settings.random {
        PlaceRandom::Positional => position_random(position).next_i32_bounded(count),
        PlaceRandom::Given => {
            unreachable!("no template piece of these structures gives its settings a random source")
        }
    };
    let wanted = clustine_data::block_id_by_name(block).expect("the block is the pinned jar's");
    template.palettes[index as usize]
        .iter()
        .filter(|info| info.state.block_id() == Some(wanted))
        .filter_map(|info| {
            let moved = transform_pos(
                info.pos,
                settings.mirror,
                settings.rotation,
                settings.rotation_pivot,
            );
            let at = BlockPos::new(
                moved.x + position.x,
                moved.y + position.y,
                moved.z + position.z,
            );
            let inside = settings
                .bounding_box
                .is_none_or(|bounds| bounds.contains_xyz(at.x, at.y, at.z));
            inside.then_some((at, info.nbt.as_ref()))
        })
        .collect()
}

/// The containers whose block entity keeps loot to be rolled
/// (`RandomizableContainer`).
const RANDOMIZABLE: [&str; 9] = [
    "minecraft:chest",
    "minecraft:trapped_chest",
    "minecraft:barrel",
    "minecraft:dispenser",
    "minecraft:dropper",
    "minecraft:hopper",
    "minecraft:shulker_box",
    "minecraft:crafter",
    "minecraft:decorated_pot",
];

/// The block entities of the class `ChestBlockEntity`: the chest's and the trapped
/// chest's, which is made from it.
pub(super) const CHESTS: [&str; 2] = ["minecraft:chest", "minecraft:trapped_chest"];

/// `RandomizableContainer.setBlockEntityLootTable`: where the block entity at `at` keeps
/// loot to be rolled, it is given `table` and a seed drawn from `random`; elsewhere
/// nothing is drawn.
pub(super) fn set_block_entity_loot_table(
    level: &mut Level<'_>,
    random: &mut dyn Random,
    at: BlockPos,
    table: &str,
) {
    set_loot_table_if(level, random, at, table, &RANDOMIZABLE);
}

/// `setLootTable(table, random.nextLong())` on the block entity at `at`, where it is of
/// one of the `kinds` (a test of its class in the game). Returns whether it was.
pub(super) fn set_loot_table_if(
    level: &mut Level<'_>,
    random: &mut dyn Random,
    at: BlockPos,
    table: &str,
    kinds: &[&str],
) -> bool {
    let Some(entity) = level.block_entity(at) else {
        return false;
    };
    let kind = entity.kind;
    if !kinds.iter().any(|name| block_entity_type(name) == kind) {
        return false;
    }
    let data = entity.data.clone();
    let seed = random.next_i64();
    let saved = with_loot_table(&data, table, seed);
    let _ = level.set_block_entity(at, Some(clustine_world::BlockEntity { kind, data: saved }));
    true
}

/// A container's data as the game saves it once it is given a loot table and a seed
/// (`trySaveLootTable`): what its own class saves (its lock and name) kept, its items
/// no longer written, the table, the seed unless it is nought, and its components.
fn with_loot_table(data: &[u8], table: &str, seed: i64) -> Vec<u8> {
    let members = if data.is_empty() {
        Vec::new()
    } else {
        let mut reader = NbtReader::new(data);
        match reader.nbt() {
            Ok(Some(Nbt::Compound(members))) => members,
            _ => Vec::new(),
        }
    };
    let mut components = Nbt::Compound(Vec::new());
    let mut kept: Vec<(String, Nbt)> = Vec::new();
    for (key, value) in members {
        match key.as_str() {
            "Items" | "LootTable" | "LootTableSeed" => {}
            "components" => components = value,
            _ => kept.push((key, value)),
        }
    }
    let mut ordered: Vec<(&str, Nbt)> = kept
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect();
    ordered.push(("LootTable", Nbt::String(table.to_string())));
    if seed != 0 {
        ordered.push(("LootTableSeed", Nbt::Long(seed)));
    }
    ordered.push(("components", components));
    let mut writer = clustine_protocol::codec::Writer::new();
    writer.put_nbt(&compound(ordered));
    writer.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(data: &[u8]) -> Nbt {
        let mut reader = NbtReader::new(data);
        let nbt = reader.nbt().unwrap().unwrap();
        reader.finish().unwrap();
        nbt
    }

    #[test]
    fn a_loot_table_takes_the_place_of_a_chest_s_items_and_keeps_its_name() {
        let mut writer = clustine_protocol::codec::Writer::new();
        writer.put_nbt(&compound(vec![
            ("Items", Nbt::List(Vec::new())),
            ("CustomName", Nbt::String("x".into())),
            ("components", Nbt::Compound(Vec::new())),
        ]));
        let saved = read(&with_loot_table(
            &writer.into_bytes(),
            "minecraft:chests/igloo_chest",
            7,
        ));
        assert_eq!(saved.get("Items"), None);
        assert_eq!(saved.get("CustomName"), Some(&Nbt::String("x".into())));
        assert_eq!(
            saved.get("LootTable"),
            Some(&Nbt::String("minecraft:chests/igloo_chest".into()))
        );
        assert_eq!(saved.get("LootTableSeed"), Some(&Nbt::Long(7)));
        assert_eq!(saved.get("components"), Some(&Nbt::Compound(Vec::new())));
    }

    #[test]
    fn a_fresh_chest_given_a_seed_of_nought_saves_no_seed() {
        let saved = read(&with_loot_table(&[], "minecraft:chests/shipwreck_map", 0));
        assert_eq!(saved.get("LootTableSeed"), None);
        assert!(saved.get("LootTable").is_some());
    }
}
