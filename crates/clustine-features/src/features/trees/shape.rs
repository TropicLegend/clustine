// Adapted from the `update_shape` of SteelMC's block behaviours under
// `steel-core/src/behavior/blocks/` (leaves, liquids, vegetation, double plants, vines,
// cocoa, hanging moss and roots, snowy dirt, mushroom blocks).

//! `BlockState.updateShape` for the blocks found at the edge of a tree: what a block
//! becomes, and the ticks it asks for, when its neighbour on one side is another.
//!
//! A tree ends by updating the shapes across the faces of all it placed
//! (`StructureTemplate.updateShapeAtEdge`), so what grows or flows beside a tree
//! answers to it: leaves and water ask for ticks, a plant that lost its ground or its
//! other half goes. The blocks ported are those that stand beside trees in the
//! overworld; any other keeps its state and asks for nothing, which the judge would
//! show as a missing tick or a block that should have gone.

use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::BlockPos;

use super::common::{
    JUNGLE_LOGS, MAINTAINS_FARMLAND, PREVENTS_NEARBY_LEAF_DECAY, SNOW, is_block, tick_id,
};
use crate::blocks::{face_name, face_named, fluid_id, is_class, relative};
use crate::context::Level;
use crate::survive::{can_survive, multiface_can_attach, sturdy};

/// Still and flowing water, still and flowing lava, by the game's fluid ids.
const FLOWING_WATER: u8 = 1;
const WATER: u8 = 2;
const FLOWING_LAVA: u8 = 3;
const LAVA: u8 = 4;

/// `Fluid.getTickDelay` in the overworld.
fn fluid_delay(fluid: u8) -> i32 {
    match fluid {
        FLOWING_WATER | WATER => 5,
        FLOWING_LAVA | LAVA => 30,
        _ => 0,
    }
}

/// `LeavesBlock.getOptionalDistanceAt`: nought for a block that keeps leaves alive,
/// the distance of leaves, none for anything else.
pub fn leaf_distance(state: BlockState) -> Option<i32> {
    if PREVENTS_NEARBY_LEAF_DECAY.contains(state) {
        return Some(0);
    }
    state.property("distance").and_then(|d| d.parse().ok())
}

/// What `state` at `pos` becomes when its neighbour towards `direction` is
/// `neighbour`, with the ticks it schedules.
pub fn update_shape(
    level: &mut Level<'_>,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> BlockState {
    if state.block_info().is_none() {
        return state;
    }
    // A waterlogged block lets its water flow on (`SimpleWaterloggedBlock`'s blocks
    // all do this first).
    if state.property("waterlogged") == Some("true") {
        level.schedule_fluid_tick(pos, WATER, fluid_delay(WATER));
    }
    // A block that falls looks again whether it is held up once something beside it
    // changes (`FallingBlock.updateShape`, and suspicious sand and gravel's
    // `BrushableBlock.updateShape`), however it is held up now.
    if is_class(state, BlockClass::FallingBlock) || is_class(state, BlockClass::BrushableBlock) {
        level.schedule_block_tick(pos, tick_id(state), 2);
        return state;
    }
    if is_class(state, BlockClass::LeavesBlock) {
        let distance = leaf_distance(neighbour).unwrap_or(7) + 1;
        let own = state
            .property("distance")
            .and_then(|d| d.parse::<i32>().ok());
        if distance != 1 || own != Some(distance) {
            level.schedule_block_tick(pos, tick_id(state), 1);
        }
        return state;
    }
    if is_class(state, BlockClass::LiquidBlock) {
        let source = |state: BlockState| state.props().fluid().is_some_and(|fluid| fluid.source);
        if source(state) || source(neighbour) {
            let fluid = fluid_id(state);
            level.schedule_fluid_tick(pos, fluid, fluid_delay(fluid));
        }
        return state;
    }
    // A path or farmland under something solid turns to dirt at its tick
    // (`PathBlock.updateShape`, `FarmlandBlock.updateShape`): a fence gate may stand on a
    // path, and what keeps farmland (crops, stems) on farmland.
    if is_class(state, BlockClass::PathBlock) || is_class(state, BlockClass::FarmlandBlock) {
        if direction == Face::Up {
            let above = level.block(relative(pos, Face::Up));
            let kept = if is_class(state, BlockClass::PathBlock) {
                is_class(above, BlockClass::FenceGateBlock)
            } else {
                MAINTAINS_FARMLAND.contains(above)
            };
            if above.props().is_solid() && !kept {
                level.schedule_block_tick(pos, tick_id(state), 1);
            }
        }
        return state;
    }
    if is_class(state, BlockClass::SnowyBlock) {
        if direction == Face::Up {
            let snowy = if SNOW.contains(neighbour) {
                "true"
            } else {
                "false"
            };
            return state.with_property("snowy", snowy).unwrap_or(state);
        }
        return state;
    }
    if is_class(state, BlockClass::VineBlock) {
        return vine(level, state, pos, direction);
    }
    if is_class(state, BlockClass::CocoaBlock) {
        let facing = state.property("facing").and_then(face_named);
        if facing == Some(direction) && !JUNGLE_LOGS.contains(level.block(relative(pos, direction)))
        {
            return blocks::AIR;
        }
        return state;
    }
    if is_class(state, BlockClass::HangingMossBlock) {
        let above = level.block(relative(pos, Face::Up));
        if !(multiface_can_attach(above, Face::Up) || is_block(above, state)) {
            level.schedule_block_tick(pos, tick_id(state), 1);
        }
        let below = level.block(relative(pos, Face::Down));
        let tip = if is_block(below, state) {
            "false"
        } else {
            "true"
        };
        return state.with_property("tip", tip).unwrap_or(state);
    }
    if is_class(state, BlockClass::HangingRootsBlock) {
        let above = level.block(relative(pos, Face::Up));
        if direction == Face::Up && !sturdy(above, Face::Down, clustine_data::SupportType::Full) {
            return blocks::AIR;
        }
        return state;
    }
    if is_class(state, BlockClass::HugeMushroomBlock) {
        if is_block(neighbour, state) {
            return state
                .with_property(face_name(direction), "false")
                .unwrap_or(state);
        }
        return state;
    }
    if is_class(state, BlockClass::SugarCaneBlock) || is_class(state, BlockClass::CactusBlock) {
        if !can_survive(state, pos, level) {
            level.schedule_block_tick(pos, tick_id(state), 1);
        }
        return state;
    }
    if is_class(state, BlockClass::MangrovePropaguleBlock) {
        if direction == Face::Up && !can_survive(state, pos, level) {
            return blocks::AIR;
        }
        return vegetation(level, state, pos);
    }
    if is_class(state, BlockClass::DoublePlantBlock) {
        let lower = state.property("half") == Some("lower");
        let other_half =
            is_block(neighbour, state) && neighbour.property("half") != state.property("half");
        let vertical = matches!(direction, Face::Up | Face::Down);
        if vertical && lower == (direction == Face::Up) && !other_half {
            return blocks::AIR;
        }
        if lower && direction == Face::Down && !can_survive(state, pos, level) {
            return blocks::AIR;
        }
        return vegetation(level, state, pos);
    }
    if is_class(state, BlockClass::SeagrassBlock) {
        let updated = vegetation(level, state, pos);
        if !updated.props().is_air() {
            level.schedule_fluid_tick(pos, WATER, fluid_delay(WATER));
        }
        return updated;
    }
    // A creaking heart looks again at its logs whenever something beside it changes
    // (`CreakingHeartBlock.updateShape`).
    if is_class(state, BlockClass::CreakingHeartBlock) {
        level.schedule_block_tick(pos, tick_id(state), 1);
        return state;
    }
    // A bamboo stalk that cannot stay falls by a tick, and one under an older stalk
    // takes the next age (`BambooStalkBlock.updateShape`).
    if is_class(state, BlockClass::BambooStalkBlock) {
        if !can_survive(state, pos, level) {
            level.schedule_block_tick(pos, tick_id(state), 1);
        }
        let age = |state: BlockState| state.property("age").and_then(|age| age.parse::<u8>().ok());
        if direction == Face::Up
            && is_block(neighbour, blocks::BAMBOO)
            && age(neighbour) > age(state)
        {
            let next = if age(state) == Some(0) { "1" } else { "0" };
            return state.with_property("age", next).unwrap_or(state);
        }
        return state;
    }
    if is_class(state, BlockClass::MossyCarpetBlock) {
        return mossy_carpet(level, state, pos);
    }
    if is_class(state, BlockClass::VegetationBlock)
        || is_class(state, BlockClass::CarpetBlock)
        || is_class(state, BlockClass::SnowLayerBlock)
    {
        return vegetation(level, state, pos);
    }
    state
}

/// `VegetationBlock.updateShape`, and the carpet's and the snow's: gone where it can
/// no longer stay.
/// `MossyCarpetBlock.updateShape`: gone where it cannot stay (a carpet with a base on
/// nothing; one without on anything but a carpet with a base), else its sides as
/// `getUpdatedState` makes them without creating new ones but where it has a base,
/// and gone where it is left without a base and without sides.
fn mossy_carpet(level: &mut Level<'_>, state: BlockState, pos: BlockPos) -> BlockState {
    let base = state.property("bottom") == Some("true");
    let below = level.block(relative(pos, Face::Down));
    let stays = if base {
        !below.props().is_air()
    } else {
        is_class(below, BlockClass::MossyCarpetBlock) && below.property("bottom") == Some("true")
    };
    if !stays {
        return blocks::AIR;
    }
    let updated = crate::features::frame::simple_block::mossy_updated(state, level, pos, false);
    let has_faces = base
        || ["north", "east", "south", "west"]
            .iter()
            .any(|side| updated.property(side) != Some("none"));
    if has_faces { updated } else { blocks::AIR }
}

fn vegetation(level: &mut Level<'_>, state: BlockState, pos: BlockPos) -> BlockState {
    if can_survive(state, pos, level) {
        state
    } else {
        blocks::AIR
    }
}

/// `VineBlock.updateShape`: each side kept where it still holds on, or where the
/// vine above holds on that side; gone with no side left.
fn vine(
    level: &mut Level<'_>,
    mut state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> BlockState {
    if direction == Face::Down {
        return state;
    }
    let above_pos = relative(pos, Face::Up);
    if state.property("up") == Some("true") {
        // The game asks this of the block above as a neighbour below it.
        let up = multiface_can_attach(level.block(above_pos), Face::Down);
        state = state
            .with_property("up", if up { "true" } else { "false" })
            .unwrap_or(state);
    }
    let mut above: Option<BlockState> = None;
    for face in super::common::HORIZONTAL {
        let name = face_name(face);
        if state.property(name) != Some("true") {
            continue;
        }
        let mut holds = multiface_can_attach(level.block(relative(pos, face)), face);
        if !holds {
            let above = *above.get_or_insert_with(|| level.block(above_pos));
            holds = is_block(above, blocks::VINE) && above.property(name) == Some("true");
        }
        state = state
            .with_property(name, if holds { "true" } else { "false" })
            .unwrap_or(state);
    }
    let any = ["up", "north", "east", "south", "west"]
        .iter()
        .any(|name| state.property(name) == Some("true"));
    if any { state } else { blocks::AIR }
}
