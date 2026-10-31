//! An End city's piece (`EndCityPieces.EndCityPiece`, checked against the pinned jar's
//! dump): a template placed as every template piece is, then its data markers.
//!
//! - Its settings (`makeSettings`): the template's entities ignored, one processor, which
//!   ignores structure blocks where the piece overwrites (`OW`) and structure blocks and
//!   air where it does not, and the piece's rotation. Mirror and pivot are the
//!   defaults; fluids are taken in, as the default settings do.
//! - Its markers (`handleDataMarker`, offsets 0 to 192): a marker whose name starts with
//!   `Chest` gives the chest below it the loot `chests/end_city_treasure`, where that
//!   chest lies in the box; one that starts with `Sentry` makes a shulker standing at the
//!   marker; one that starts with `Elytra` makes an item frame in the marker's block,
//!   facing south turned with the piece, holding an elytra.
//!
//! The shulker and the item frame are made as a template's entities are
//! ([`Level::make_template_entities`](crate::context::Level::make_template_entities)),
//! from the tag the game's would save, not finalised: the game calls neither
//! `finalizeSpawn` nor `setPersistenceRequired` on them. A type the maker does not make
//! yet makes nothing and draws nothing, as for every template; neither draws from the
//! region's random source in the game either, so nothing after them is moved.

use clustine_protocol::nbt::Nbt;
use clustine_structures::geometry::{Direction, Rotation};
use clustine_structures::piece::{StructureBlockIgnore, TemplatePieceData};
use clustine_structures::place::PlacedEntity;
use clustine_structures::processors::Processor;
use clustine_world::BlockPos;

use super::Placing;
use super::shipwreck::to_block;
use super::template_piece::{post_process, set_block_entity_loot_table, settings, template};
use crate::features::caves::templates::FeatureShapes;
use crate::features::ground::shared::compound;

/// `BuiltInLootTables.END_CITY_TREASURE`.
pub(super) const LOOT: &str = "minecraft:chests/end_city_treasure";

/// Places a piece of an End city. Its box never moves, so it keeps no state between
/// runs.
pub(super) fn place(placing: &mut Placing<'_, '_, '_>, data: &TemplatePieceData) {
    let Some(template) = template(placing, data.template_id.path) else {
        return;
    };
    let shapes = FeatureShapes {
        settings: placing.settings,
    };
    // `makeSettings(overwrite, rotation)`: `STRUCTURE_BLOCK` where the piece overwrites,
    // `STRUCTURE_AND_AIR` where it does not, as `EndCityPieces` chose for each piece.
    let ignore = match data.block_ignore {
        StructureBlockIgnore::StructureAndAir => Processor::structure_and_air_ignore(),
        _ => Processor::structure_block_ignore(),
    };
    let mut settings = settings(placing, placing.chunk_box, &shapes);
    settings.ignore_entities = true;
    settings.rotation = data.rotation;
    settings.mirror = data.mirror;
    settings.rotation_pivot = to_block(data.rotation_pivot);
    settings.liquid_settings = data.liquid_settings;
    settings.processors.push(&ignore);
    let rotation = data.rotation;
    let position = to_block(data.template_position);
    post_process(
        placing,
        template,
        position,
        &settings,
        &mut |placing, marker, at| data_marker(placing, rotation, marker, at),
    );
}

/// `EndCityPiece.handleDataMarker`. The markers `post_process` hands over all lie in the
/// box already (`filterBlocks` with the settings' box, which is the one handed here), so
/// the game's test of the marker's own place in the box, kept below, always holds.
fn data_marker(placing: &mut Placing<'_, '_, '_>, rotation: Rotation, marker: &str, at: BlockPos) {
    if marker.starts_with("Chest") {
        // Offsets 9 to 33: the chest is the block below, and only where the box holds it.
        let below = BlockPos::new(at.x, at.y - 1, at.z);
        if placing.chunk_box.contains_xyz(below.x, below.y, below.z) {
            set_block_entity_loot_table(placing.level, &mut *placing.random, below, LOOT);
        }
        return;
    }
    // Offsets 39 to 52.
    if !placing.chunk_box.contains_xyz(at.x, at.y, at.z) || !in_spawnable_bounds(at) {
        return;
    }
    let made = if marker.starts_with("Sentry") {
        sentry(at)
    } else if marker.starts_with("Elytra") {
        elytra(at, rotation.rotate(Direction::South))
    } else {
        return;
    };
    let settings = placing.settings;
    placing.level.make_template_entities(settings, vec![made]);
}

/// `Level.isInSpawnableBounds`: within 30 million blocks each way and 20 million up and
/// down.
fn in_spawnable_bounds(at: BlockPos) -> bool {
    const HORIZONTAL: i32 = 30_000_000;
    const VERTICAL: i32 = 20_000_000;
    (-VERTICAL..VERTICAL).contains(&at.y)
        && (-HORIZONTAL..HORIZONTAL).contains(&at.x)
        && (-HORIZONTAL..HORIZONTAL).contains(&at.z)
}

/// Offsets 64 to 120: `EntityType.SHULKER.create(level, STRUCTURE)`, then `setPos` at the
/// middle of the marker's block at its floor. The shulker keeps every default, its face
/// among them: attached downwards (`Shulker.DEFAULT_ATTACH_FACE`), to the block it
/// stands on, as the template's sentry spots all have one below; its yaw is nought.
fn sentry(at: BlockPos) -> PlacedEntity {
    let position = [
        f64::from(at.x) + 0.5,
        f64::from(at.y),
        f64::from(at.z) + 0.5,
    ];
    let tag = compound(vec![
        ("id", Nbt::String("minecraft:shulker".into())),
        ("Pos", doubles(position)),
        (
            "Rotation",
            Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(0.0)]),
        ),
    ]);
    PlacedEntity {
        tag,
        block: at,
        position,
        yaw: 0.0,
        pitch: 0.0,
        finalise: false,
    }
}

/// Offsets 138 to 186: `new ItemFrame(level, pos, facing)` with
/// `setItem(new ItemStack(ELYTRA), false)`. The constructor (`ItemFrame.setDirection`)
/// sets the yaw to the facing's two-dimensional value times 90 and the pitch to
/// nought, and puts the frame at the middle of its box
/// (`HangingEntity.recalculateBoundingBox`): the middle of the block moved 0.46875
/// against the facing (`ItemFrame.createBoundingBox`). The tag is what the frame saves
/// (`ItemFrame.addAdditionalSaveData`, `BlockAttachedEntity`'s `block_pos`): the facing
/// as its three-dimensional value, the item, no turn, a drop chance of one, visible
/// and not fixed.
fn elytra(at: BlockPos, facing: Direction) -> PlacedEntity {
    let (dx, dy, dz) = facing.offset();
    let position = [
        f64::from(at.x) + 0.5 - 0.46875 * f64::from(dx),
        f64::from(at.y) + 0.5 - 0.46875 * f64::from(dy),
        f64::from(at.z) + 0.5 - 0.46875 * f64::from(dz),
    ];
    let yaw = (facing.get_2d_data_value() * 90) as f32;
    let facing_byte =
        i8::try_from(facing.get_3d_data_value()).expect("a direction's value is below six");
    let item = compound(vec![
        ("id", Nbt::String("minecraft:elytra".into())),
        ("count", Nbt::Int(1)),
    ]);
    let tag = compound(vec![
        ("id", Nbt::String("minecraft:item_frame".into())),
        ("Pos", doubles(position)),
        (
            "Rotation",
            Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(0.0)]),
        ),
        ("block_pos", Nbt::IntArray(vec![at.x, at.y, at.z])),
        ("Item", item),
        ("ItemRotation", Nbt::Byte(0)),
        ("ItemDropChance", Nbt::Float(1.0)),
        ("Facing", Nbt::Byte(facing_byte)),
        ("Invisible", Nbt::Byte(0)),
        ("Fixed", Nbt::Byte(0)),
    ]);
    PlacedEntity {
        tag,
        block: at,
        position,
        yaw,
        pitch: 0.0,
        finalise: false,
    }
}

fn doubles(values: [f64; 3]) -> Nbt {
    Nbt::List(values.into_iter().map(Nbt::Double).collect())
}
