// Adapted from the `update_shape` of SteelMC's block behaviours under
// `steel-core/src/behavior/blocks/` (leaves, liquids, vegetation, double plants, vines,
// cocoa, hanging moss and roots, snowy dirt, mushroom blocks, fences, bars, walls,
// fence gates, stairs, redstone wire), by way of clustine-features'
// `src/features/trees/shape.rs`.

//! `BlockState.updateShape`: what a block becomes, and the ticks it asks for, when its
//! neighbour on one side is another.
//!
//! World generation asks it across the faces of all a tree or a template placed
//! (`StructureTemplate.updateShapeAtEdge`), so what grows, flows or stands beside them
//! answers to it: leaves and water ask for ticks, a plant that lost its ground or its
//! other half goes, a fence joins the log beside it. Any block whose rule is not ported
//! keeps its state and asks for nothing, which generation's judge would show as a
//! missing tick or a block that should have changed.

use clustine_data::{BlockClass, BlockState, Face, SupportType, blocks};
use clustine_world::BlockPos;

use crate::connect;
use crate::data::{
    HORIZONTAL, JUNGLE_LOGS, MAINTAINS_FARMLAND, PREVENTS_NEARBY_LEAF_DECAY, SNOW, axis, clockwise,
    face_name, facing, fluid_id, fluid_in_tag, has_water, in_tag, is_block, is_class,
    is_horizontal, number, relative, same_block, set, set_flag, tick_id,
};
use crate::survive::{can_survive, fire_can_burn, multiface_can_attach, sturdy};
use crate::{BlockView, Rules};

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
pub fn update_shape<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> BlockState {
    if state.block_info().is_none() {
        return state;
    }
    // A block whose support went goes, before its water is scheduled.
    if let Some(gone) = crate::support::without_support(view, state, pos, direction) {
        return gone;
    }
    // Fire and coral as the simulation has them. Generation keeps the answers its judge
    // compared with the official server's.
    if view.rules() == Rules::Placing
        && let Some(updated) = fire_or_coral(view, state, pos, direction)
    {
        return updated;
    }
    // A waterlogged block lets its water flow on (`SimpleWaterloggedBlock`'s blocks
    // all do this first).
    if state.property("waterlogged") == Some("true") {
        view.schedule_fluid_tick(pos, WATER, fluid_delay(WATER));
    }
    // What grows takes its shape as it grows, in the simulation (`growth.rs`).
    if view.rules() == crate::Rules::Placing
        && let Some(grown) = crate::growth::update_shape(view, state, pos, direction, neighbour)
    {
        return grown;
    }
    // Concrete powder that water touches hardens (`ConcretePowderBlock.updateShape`).
    if let Some(concrete) = crate::fluid::concrete_of(state)
        && crate::fluid::touches_water(view, pos)
    {
        return concrete;
    }
    // A block that falls looks again whether it is held up once something beside it
    // changes (`FallingBlock.updateShape`, and suspicious sand and gravel's
    // `BrushableBlock.updateShape`), however it is held up now.
    if is_class(state, BlockClass::FallingBlock) || is_class(state, BlockClass::BrushableBlock) {
        view.schedule_block_tick(pos, tick_id(state), 2);
        return state;
    }
    if is_class(state, BlockClass::LeavesBlock) {
        let distance = leaf_distance(neighbour).unwrap_or(7) + 1;
        let own = state
            .property("distance")
            .and_then(|d| d.parse::<i32>().ok());
        if distance != 1 || own != Some(distance) {
            view.schedule_block_tick(pos, tick_id(state), 1);
        }
        return state;
    }
    if is_class(state, BlockClass::LiquidBlock) {
        let source = |state: BlockState| state.props().fluid().is_some_and(|fluid| fluid.source);
        if source(state) || source(neighbour) {
            let fluid = fluid_id(state);
            view.schedule_fluid_tick(pos, fluid, fluid_delay(fluid));
        }
        // Since 26.x bubble columns are water's own business: a full source of a fluid
        // a column may stand in, told of a block below that makes one, ticks itself to
        // become the column (`LiquidBlock.tryScheduleBubbleBlockColumn`).
        let full_source = state
            .props()
            .fluid()
            .is_some_and(|fluid| fluid.source && fluid.amount == 8);
        if direction == Face::Down
            && full_source
            && fluid_in_tag(state, "minecraft:bubble_column_can_occupy")
            && (in_tag(neighbour, "minecraft:enables_bubble_column_drag_down")
                || in_tag(neighbour, "minecraft:enables_bubble_column_push_up"))
        {
            view.schedule_block_tick(pos, tick_id(state), 20);
        }
        return state;
    }
    // A path or farmland under something solid turns to dirt at its tick
    // (`PathBlock.updateShape`, `FarmlandBlock.updateShape`): a fence gate may stand on a
    // path, and what keeps farmland (crops, stems) on farmland.
    if is_class(state, BlockClass::PathBlock) || is_class(state, BlockClass::FarmlandBlock) {
        if direction == Face::Up {
            let above = view.block(relative(pos, Face::Up));
            let kept = if is_class(state, BlockClass::PathBlock) {
                is_class(above, BlockClass::FenceGateBlock)
            } else {
                MAINTAINS_FARMLAND.contains(above)
            };
            if above.props().is_solid() && !kept {
                view.schedule_block_tick(pos, tick_id(state), 1);
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
        return vine(view, state, pos, direction);
    }
    if is_class(state, BlockClass::CocoaBlock) {
        if facing(state) == Some(direction)
            && !JUNGLE_LOGS.contains(view.block(relative(pos, direction)))
        {
            return blocks::AIR;
        }
        return state;
    }
    if is_class(state, BlockClass::HangingMossBlock) {
        let above = view.block(relative(pos, Face::Up));
        if !(multiface_can_attach(above, Face::Up) || is_block(above, state)) {
            view.schedule_block_tick(pos, tick_id(state), 1);
        }
        let below = view.block(relative(pos, Face::Down));
        let tip = if is_block(below, state) {
            "false"
        } else {
            "true"
        };
        return state.with_property("tip", tip).unwrap_or(state);
    }
    if is_class(state, BlockClass::HangingRootsBlock) {
        let above = view.block(relative(pos, Face::Up));
        if direction == Face::Up && !sturdy(above, Face::Down, SupportType::Full) {
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
        if !can_survive(state, pos, view) {
            view.schedule_block_tick(pos, tick_id(state), 1);
        }
        return state;
    }
    if is_class(state, BlockClass::MangrovePropaguleBlock) {
        if direction == Face::Up && !can_survive(state, pos, view) {
            return blocks::AIR;
        }
        return vegetation(view, state, pos);
    }
    // A door's half told of its other half becomes that half with its own half, and air
    // where the other is no door's; a lower half told of the block below goes where it
    // can no longer stand (`DoorBlock.updateShape`).
    if is_class(state, BlockClass::DoorBlock) {
        let half = state.property("half");
        let lower = half == Some("lower");
        if matches!(direction, Face::Up | Face::Down) && lower == (direction == Face::Up) {
            if is_class(neighbour, BlockClass::DoorBlock) && neighbour.property("half") != half {
                return neighbour
                    .with_property("half", half.expect("a door has a half"))
                    .expect("a door has a half");
            }
            return blocks::AIR;
        }
        if lower && direction == Face::Down && !can_survive(state, pos, view) {
            return blocks::AIR;
        }
        return state;
    }
    if is_class(state, BlockClass::DoublePlantBlock) {
        let lower = state.property("half") == Some("lower");
        let other_half =
            is_block(neighbour, state) && neighbour.property("half") != state.property("half");
        let vertical = matches!(direction, Face::Up | Face::Down);
        if vertical && lower == (direction == Face::Up) && !other_half {
            return blocks::AIR;
        }
        if lower && direction == Face::Down && !can_survive(state, pos, view) {
            return blocks::AIR;
        }
        return vegetation(view, state, pos);
    }
    if is_class(state, BlockClass::SeagrassBlock) {
        let updated = vegetation(view, state, pos);
        if !updated.props().is_air() {
            view.schedule_fluid_tick(pos, WATER, fluid_delay(WATER));
        }
        return updated;
    }
    // A creaking heart looks again at its logs whenever something beside it changes
    // (`CreakingHeartBlock.updateShape`).
    if is_class(state, BlockClass::CreakingHeartBlock) {
        view.schedule_block_tick(pos, tick_id(state), 1);
        return state;
    }
    // A bamboo stalk that cannot stay falls by a tick, and one under an older stalk
    // takes the next age (`BambooStalkBlock.updateShape`).
    if is_class(state, BlockClass::BambooStalkBlock) {
        if !can_survive(state, pos, view) {
            view.schedule_block_tick(pos, tick_id(state), 1);
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
        return mossy_carpet(view, state, pos);
    }
    if is_class(state, BlockClass::VegetationBlock)
        || is_class(state, BlockClass::CarpetBlock)
        || is_class(state, BlockClass::SnowLayerBlock)
    {
        return vegetation(view, state, pos);
    }
    connections(view, state, pos, direction, neighbour)
}

/// The cases of blocks that join their neighbours: a side of a fence, a pane or bars
/// set by the neighbour on it (`FenceBlock.updateShape`, `IronBarsBlock.updateShape`),
/// a wall's side or its post (`WallBlock.updateShape`), a fence gate in a wall
/// (`FenceGateBlock.updateShape`), stairs' corners (`StairBlock.updateShape`) and
/// redstone wire (`RedStoneWireBlock.updateShape`). The tests are those the blocks are
/// placed by.
fn connections<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> BlockState {
    let horizontal = is_horizontal(direction);
    if is_class(state, BlockClass::FenceBlock) {
        if horizontal {
            let joins = connect::fence_joins(state, direction, neighbour);
            return set_flag(state, face_name(direction), joins);
        }
        return state;
    }
    if is_class(state, BlockClass::IronBarsBlock) {
        if horizontal {
            let joins = connect::pane_joins(direction, neighbour);
            return set_flag(state, face_name(direction), joins);
        }
        return state;
    }
    if is_class(state, BlockClass::WallBlock) {
        return match direction {
            Face::Down => state,
            // The block above decides which sides are tall and whether the post rises.
            Face::Up => connect::wall_with(state, connect::wall_joined(state), neighbour),
            side => {
                let mut joins = connect::wall_joined(state);
                for (joined, towards) in joins.iter_mut().zip(HORIZONTAL) {
                    if towards == side {
                        *joined = connect::wall_joins(side, neighbour);
                    }
                }
                let above = view.block(relative(pos, Face::Up));
                connect::wall_with(state, joins, above)
            }
        };
    }
    if is_class(state, BlockClass::FenceGateBlock) {
        // Only a neighbour on either end of the gate, across its facing, changes it.
        if facing(state).is_some_and(|gate| axis(clockwise(gate)) == axis(direction)) {
            let other_end = view.block(relative(pos, direction.opposite()));
            let in_wall =
                in_tag(neighbour, "minecraft:walls") || in_tag(other_end, "minecraft:walls");
            return set_flag(state, "in_wall", in_wall);
        }
        return state;
    }
    if is_class(state, BlockClass::StairBlock) {
        if horizontal {
            return connect::stairs(state, pos, view);
        }
        return state;
    }
    if same_block(state, blocks::REDSTONE_WIRE) {
        return connect::redstone_wire_shape(state, pos, direction, neighbour, view);
    }
    state
}

/// `MossyCarpetBlock.updateShape`: gone where it cannot stay (a carpet with a base on
/// nothing; one without on anything but a carpet with a base), else its sides as
/// `getUpdatedState` makes them without creating new ones but where it has a base,
/// and gone where it is left without a base and without sides.
fn mossy_carpet<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> BlockState {
    let base = state.property("bottom") == Some("true");
    let below = view.block(relative(pos, Face::Down));
    let stays = if base {
        !below.props().is_air()
    } else {
        is_class(below, BlockClass::MossyCarpetBlock) && below.property("bottom") == Some("true")
    };
    if !stays {
        return blocks::AIR;
    }
    let updated = mossy_carpet_updated(state, view, pos, false);
    let has_faces = base
        || HORIZONTAL
            .iter()
            .any(|side| updated.property(face_name(*side)) != Some("none"));
    if has_faces { updated } else { blocks::AIR }
}

/// `MossyCarpetBlock.getUpdatedState`: each side low where it can hold on to the block
/// beside it (new sides only where `create_sides` or the carpet has a base), tall
/// under a carpet without a base that has that side, and none where the carpet below
/// without a base has none.
pub fn mossy_carpet_updated<V: BlockView + ?Sized>(
    mut state: BlockState,
    view: &mut V,
    pos: BlockPos,
    create_sides: bool,
) -> BlockState {
    let base = state.property("bottom") == Some("true");
    let create_sides = create_sides || base;
    let mut above: Option<BlockState> = None;
    let mut below: Option<BlockState> = None;
    for face in HORIZONTAL {
        let name = face_name(face);
        let attaches = multiface_can_attach(view.block(relative(pos, face)), face);
        let mut side = if attaches {
            if create_sides {
                "low"
            } else {
                state.property(name).unwrap_or("none")
            }
        } else {
            "none"
        };
        if side == "low" {
            let above = *above.get_or_insert_with(|| view.block(relative(pos, Face::Up)));
            if same_block(above, blocks::PALE_MOSS_CARPET)
                && above.property(name) != Some("none")
                && above.property("bottom") == Some("false")
            {
                side = "tall";
            }
            if !base {
                let below = *below.get_or_insert_with(|| view.block(relative(pos, Face::Down)));
                if same_block(below, blocks::PALE_MOSS_CARPET)
                    && below.property(name) == Some("none")
                {
                    side = "none";
                }
            }
        }
        state = state.with_property(name, side).unwrap_or(state);
    }
    state
}

/// `VegetationBlock.updateShape`, and the carpet's and the snow's: gone where it can
/// no longer stay.
fn vegetation<V: BlockView + ?Sized>(view: &mut V, state: BlockState, pos: BlockPos) -> BlockState {
    if can_survive(state, pos, view) {
        state
    } else {
        blocks::AIR
    }
}

/// `VineBlock.updateShape`: each side kept where it still holds on, or where the
/// vine above holds on that side; gone with no side left.
fn vine<V: BlockView + ?Sized>(
    view: &mut V,
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
        let up = multiface_can_attach(view.block(above_pos), Face::Down);
        state = state
            .with_property("up", if up { "true" } else { "false" })
            .unwrap_or(state);
    }
    let mut above: Option<BlockState> = None;
    for face in HORIZONTAL {
        let name = face_name(face);
        if state.property(name) != Some("true") {
            continue;
        }
        let mut holds = multiface_can_attach(view.block(relative(pos, face)), face);
        if !holds {
            let above = *above.get_or_insert_with(|| view.block(above_pos));
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

/// The `updateShape` of fire and of coral: what `state` becomes, or `None` for a
/// block that is neither. Coral schedules its own water, in the order its classes do.
fn fire_or_coral<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> Option<BlockState> {
    use BlockClass as C;
    // `FireBlock.updateShape`: gone where it cannot stay, else the fire this place
    // makes (soul fire on its ground), at the same age.
    if is_class(state, C::FireBlock) {
        if !can_survive(state, pos, view) {
            return Some(blocks::AIR);
        }
        let age = number(state, "age").unwrap_or(0);
        return Some(fire_state_with_age(view, pos, age));
    }
    // `SoulFireBlock.updateShape`.
    if is_class(state, C::SoulFireBlock) {
        return Some(if can_survive(state, pos, view) {
            blocks::SOUL_FIRE
        } else {
            blocks::AIR
        });
    }
    // `CoralWallFanBlock.updateShape`, then its parent's.
    if is_class(state, C::CoralWallFanBlock) {
        if facing(state) == Some(direction.opposite()) && !coral_survives(view, state, pos) {
            return Some(blocks::AIR);
        }
        schedule_water_if_waterlogged(view, state, pos);
        coral_try_schedule_die_tick(view, state, pos);
        return Some(coral_wall_fan_base(view, state, pos, direction));
    }
    if is_class(state, C::BaseCoralWallFanBlock) {
        return Some(coral_wall_fan_base(view, state, pos, direction));
    }
    // `CoralPlantBlock.updateShape` and `CoralFanBlock.updateShape`, then their
    // parents'.
    if is_class(state, C::CoralPlantBlock) || is_class(state, C::CoralFanBlock) {
        if direction == Face::Down && !coral_survives(view, state, pos) {
            return Some(blocks::AIR);
        }
        coral_try_schedule_die_tick(view, state, pos);
        schedule_water_if_waterlogged(view, state, pos);
        return Some(coral_plant_base(view, state, pos, direction));
    }
    if is_class(state, C::BaseCoralPlantTypeBlock) {
        return Some(coral_plant_base(view, state, pos, direction));
    }
    // `CoralBlock.updateShape`: a coral block with no water beside it dies at a tick.
    if is_class(state, C::CoralBlock) {
        let wet = Face::ALL
            .into_iter()
            .any(|face| has_water(view.block(relative(pos, face))));
        if !wet {
            let delay = 60 + view.next_int(40) as i32;
            view.schedule_block_tick(pos, tick_id(state), delay);
        }
        return Some(state);
    }
    None
}

/// `BaseCoralWallFanBlock.updateShape`: its water flows on, and it goes when the
/// block it hangs on can no longer hold it.
fn coral_wall_fan_base<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> BlockState {
    schedule_water_if_waterlogged(view, state, pos);
    if facing(state) == Some(direction.opposite()) && !coral_survives(view, state, pos) {
        return blocks::AIR;
    }
    state
}

/// `BaseCoralPlantTypeBlock.updateShape`: its water flows on, and it goes when the
/// block below can no longer hold it.
fn coral_plant_base<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> BlockState {
    schedule_water_if_waterlogged(view, state, pos);
    if direction == Face::Down && !coral_survives(view, state, pos) {
        return blocks::AIR;
    }
    state
}

fn schedule_water_if_waterlogged<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) {
    if state.property("waterlogged") == Some("true") {
        view.schedule_fluid_tick(pos, WATER, fluid_delay(WATER));
    }
}

/// The `canSurvive` of coral: a sturdy top below a plant or a fan
/// (`BaseCoralPlantTypeBlock`), a sturdy face behind a wall fan
/// (`BaseCoralWallFanBlock`). The coral's own, since placing has no rule for coral
/// plants.
fn coral_survives<V: BlockView + ?Sized>(view: &mut V, state: BlockState, pos: BlockPos) -> bool {
    if is_class(state, BlockClass::BaseCoralWallFanBlock) {
        let Some(facing) = facing(state) else {
            return false;
        };
        let behind = view.block(relative(pos, facing.opposite()));
        return sturdy(behind, facing, SupportType::Full);
    }
    sturdy(
        view.block(relative(pos, Face::Down)),
        Face::Up,
        SupportType::Full,
    )
}

/// `BaseCoralPlantTypeBlock.scanForWater`: whether a coral plant or fan is in water or
/// has water beside it.
pub fn coral_scan_for_water<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> bool {
    state.property("waterlogged") == Some("true")
        || Face::ALL
            .into_iter()
            .any(|face| has_water(view.block(relative(pos, face))))
}

/// `BaseCoralPlantTypeBlock.tryScheduleDieTick`: coral out of water dies at a tick in
/// three to five seconds.
pub fn coral_try_schedule_die_tick<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) {
    if !coral_scan_for_water(view, state, pos) {
        let delay = 60 + view.next_int(40) as i32;
        view.schedule_block_tick(pos, tick_id(state), delay);
    }
}

/// `BaseFireBlock.getState`: soul fire on its ground, else fire as
/// `FireBlock.getStateForPlacement` makes it: plain on a block that burns or holds it
/// up, else joined to each side but the bottom where the block there burns.
pub fn fire_state<V: BlockView + ?Sized>(view: &mut V, pos: BlockPos) -> BlockState {
    let below = view.block(relative(pos, Face::Down));
    if in_tag(below, "minecraft:soul_fire_base_blocks") {
        return blocks::SOUL_FIRE;
    }
    if fire_can_burn(below) || sturdy(below, Face::Up, SupportType::Full) {
        return blocks::FIRE;
    }
    let mut fire = blocks::FIRE;
    for face in [Face::Up, Face::North, Face::South, Face::West, Face::East] {
        let burns = fire_can_burn(view.block(relative(pos, face)));
        fire = set_flag(fire, face_name(face), burns);
    }
    fire
}

/// `FireBlock.getStateWithAge`: the fire this place makes, at `age` where it is fire
/// and not soul fire, which has no age.
pub fn fire_state_with_age<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    age: u8,
) -> BlockState {
    let fire = fire_state(view, pos);
    if is_block(fire, blocks::FIRE) {
        set(fire, "age", &age.to_string())
    } else {
        fire
    }
}
