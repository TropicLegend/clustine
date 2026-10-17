// Adapted from SteelMC's `steel-core/src/behavior/blocks/building/fence_block.rs`,
// `bar_block.rs`, `wall_block.rs`, `stair_block.rs`, `blocks/utils.rs` and
// `redstone/wire/block.rs`, by way of clustine-sim's `src/placement/connect.rs`.

//! The shapes that fences, panes, walls, fence gates, stairs and redstone wire take
//! from their neighbours: as they are placed (each block's `getStateForPlacement`),
//! and on one side at a time as a neighbour changes, which [`crate::update_shape`]
//! asks (each block's `updateShape`). Both are made of the same tests, as the game's
//! are.

use clustine_data::{BlockClass, BlockState, Face, SupportType, blocks, faces};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::data::{
    Axis, HORIZONTAL, axis, clockwise, counter_clockwise, face_name, facing, flag, in_tag,
    is_class, relative, same_block, set, set_flag,
};
use crate::survive::{sturdy, wire_can_stand_on};

/// The game's `Block.isExceptionForConnection`: blocks whose sturdy sides nothing
/// joins.
fn is_exception(state: BlockState) -> bool {
    is_class(state, BlockClass::LeavesBlock)
        || in_tag(state, "minecraft:shulker_boxes")
        || [
            blocks::BARRIER,
            blocks::CARVED_PUMPKIN,
            blocks::JACK_O_LANTERN,
            blocks::MELON,
            blocks::PUMPKIN,
        ]
        .into_iter()
        .any(|other| same_block(state, other))
}

/// Whether the side of `neighbour` that faces back across `towards` (the side from the
/// block that joins to it) is sturdy and not an exception.
fn sturdy_towards(neighbour: BlockState, towards: Face) -> bool {
    !is_exception(neighbour) && sturdy(neighbour, towards.opposite(), SupportType::Full)
}

/// Whether a fence gate lies across `towards`, so that what is on that side of it
/// joins it: the game's `FenceGateBlock.connectsToDirection`.
fn gate_across(neighbour: BlockState, towards: Face) -> bool {
    is_class(neighbour, BlockClass::FenceGateBlock)
        && facing(neighbour).is_some_and(|gate| axis(gate) == axis(clockwise(towards)))
}

/// The game's `FenceBlock.connectsTo`: whether the fence `state` joins `neighbour`,
/// which lies on its side `towards`.
pub fn fence_joins(state: BlockState, towards: Face, neighbour: BlockState) -> bool {
    let wooden = in_tag(state, "minecraft:wooden_fences");
    let same_fence = in_tag(neighbour, "minecraft:fences")
        && in_tag(neighbour, "minecraft:wooden_fences") == wooden;
    sturdy_towards(neighbour, towards) || same_fence || gate_across(neighbour, towards)
}

/// A fence with the connections its neighbours give it.
pub fn fence<V: BlockView + ?Sized>(state: BlockState, pos: BlockPos, view: &mut V) -> BlockState {
    let mut joined = state;
    for towards in HORIZONTAL {
        let neighbour = view.block(relative(pos, towards));
        joined = set_flag(
            joined,
            face_name(towards),
            fence_joins(state, towards, neighbour),
        );
    }
    joined
}

/// The game's `IronBarsBlock.attachsTo`: whether a pane or bars join `neighbour`,
/// which lies on their side `towards`.
pub fn pane_joins(towards: Face, neighbour: BlockState) -> bool {
    sturdy_towards(neighbour, towards)
        || is_class(neighbour, BlockClass::IronBarsBlock)
        || in_tag(neighbour, "minecraft:walls")
}

/// A pane or bars with the connections its neighbours give it.
pub fn pane<V: BlockView + ?Sized>(state: BlockState, pos: BlockPos, view: &mut V) -> BlockState {
    let mut state = state;
    for towards in HORIZONTAL {
        let neighbour = view.block(relative(pos, towards));
        state = set_flag(state, face_name(towards), pane_joins(towards, neighbour));
    }
    state
}

/// The game's `WallBlock.connectsTo`: whether a wall joins `neighbour`, which lies on
/// its side `towards`.
pub fn wall_joins(towards: Face, neighbour: BlockState) -> bool {
    in_tag(neighbour, "minecraft:walls")
        || sturdy_towards(neighbour, towards)
        || is_class(neighbour, BlockClass::IronBarsBlock)
        || gate_across(neighbour, towards)
}

/// A wall with the sides and the post its neighbours and the block above give it.
pub fn wall<V: BlockView + ?Sized>(state: BlockState, pos: BlockPos, view: &mut V) -> BlockState {
    let above = view.block(relative(pos, Face::Up));
    let mut joins = [false; 4];
    for (joined, towards) in joins.iter_mut().zip(HORIZONTAL) {
        *joined = wall_joins(towards, view.block(relative(pos, towards)));
    }
    wall_with(state, joins, above)
}

/// The game's `WallBlock.updateShape` of its sides and post: the wall `state` joined on
/// the sides `joins` (in the order of [`HORIZONTAL`]), each side tall where the block
/// `above` reaches down over it and low elsewhere, and its post.
pub fn wall_with(state: BlockState, joins: [bool; 4], above: BlockState) -> BlockState {
    let above_boxes: Vec<[f64; 6]> = above.props().collision_shape().boxes().collect();
    // The part of the bottom of the block above that a side of the wall reaches up to,
    // in x and z: the game's `TEST_SHAPES_WALL`.
    let arm = |towards: Face| -> [f64; 4] {
        let (low, high) = (7.0 / 16.0, 9.0 / 16.0);
        match towards {
            Face::North => [low, 0.0, high, high],
            Face::South => [low, low, high, 1.0],
            Face::West => [0.0, low, high, high],
            _ => [low, low, 1.0, high],
        }
    };
    let mut state = state;
    let mut sides = ["none"; 4];
    for ((slot, towards), joined) in sides.iter_mut().zip(HORIZONTAL).zip(joins) {
        let height = if !joined {
            "none"
        } else if faces::face_covers(&above_boxes, Face::Down, arm(towards)) {
            "tall"
        } else {
            "low"
        };
        *slot = height;
        state = set(state, face_name(towards), height);
    }
    let [north, east, south, west] = sides;
    let post = if in_tag(above, "minecraft:walls") && flag(above, "up") {
        true
    } else {
        let all_none = sides.iter().all(|side| *side == "none");
        let corner = all_none
            || (north == "none") != (south == "none")
            || (east == "none") != (west == "none");
        if corner {
            true
        } else if (north == "tall" && south == "tall") || (east == "tall" && west == "tall") {
            false
        } else {
            let post_area = [7.0 / 16.0, 7.0 / 16.0, 9.0 / 16.0, 9.0 / 16.0];
            in_tag(above, "minecraft:wall_post_override")
                || faces::face_covers(&above_boxes, Face::Down, post_area)
        }
    };
    set_flag(state, "up", post)
}

/// Which sides of the wall `state` are joined, in the order of [`HORIZONTAL`].
pub fn wall_joined(state: BlockState) -> [bool; 4] {
    HORIZONTAL.map(|towards| {
        state
            .property(face_name(towards))
            .is_some_and(|s| s != "none")
    })
}

/// Whether a fence gate facing `facing` at `pos` is in a wall: a wall is beside it
/// across its axis.
pub fn gate_in_wall<V: BlockView + ?Sized>(pos: BlockPos, facing: Face, view: &mut V) -> bool {
    let sides = match axis(facing) {
        Axis::Z => [Face::West, Face::East],
        Axis::X => [Face::North, Face::South],
        Axis::Y => return false,
    };
    sides
        .into_iter()
        .any(|side| in_tag(view.block(relative(pos, side)), "minecraft:walls"))
}

/// Whether `state` is stairs.
fn is_stairs(state: BlockState) -> bool {
    is_class(state, BlockClass::StairBlock)
}

/// Stairs with the shape the stairs beside them give: the game's `getStairsShape`.
pub fn stairs<V: BlockView + ?Sized>(state: BlockState, pos: BlockPos, view: &mut V) -> BlockState {
    let Some(own_facing) = facing(state) else {
        return state;
    };
    let half = state.property("half");
    // Whether the stairs on the side `side` would not take the same shape, so that the
    // corner may form.
    let can_take_shape = |side: Face, view: &mut V| {
        let neighbour = view.block(relative(pos, side));
        !is_stairs(neighbour)
            || facing(neighbour) != Some(own_facing)
            || neighbour.property("half") != half
    };
    let shape = 'shape: {
        let behind = view.block(relative(pos, own_facing));
        if is_stairs(behind)
            && behind.property("half") == half
            && let Some(theirs) = facing(behind)
            && axis(theirs) != axis(own_facing)
            && can_take_shape(theirs.opposite(), view)
        {
            break 'shape if theirs == counter_clockwise(own_facing) {
                "outer_left"
            } else {
                "outer_right"
            };
        }
        let front = view.block(relative(pos, own_facing.opposite()));
        if is_stairs(front)
            && front.property("half") == half
            && let Some(theirs) = facing(front)
            && axis(theirs) != axis(own_facing)
            && can_take_shape(theirs, view)
        {
            break 'shape if theirs == counter_clockwise(own_facing) {
                "inner_left"
            } else {
                "inner_right"
            };
        }
        "straight"
    };
    set(state, "shape", shape)
}

/// Redstone wire as placed: the game's `getConnectionState` of a cross, which keeps the
/// sides its neighbours connect and is a cross where none does.
pub fn redstone_wire<V: BlockView + ?Sized>(
    state: BlockState,
    pos: BlockPos,
    view: &mut V,
) -> BlockState {
    let mut state = state;
    let above = view.block(relative(pos, Face::Up));
    let can_connect_up = !is_conductor(above);
    for towards in HORIZONTAL {
        let side = wire_side(pos, towards, can_connect_up, view);
        state = set(state, face_name(towards), side);
    }
    wire_cross_where_alone(state)
}

/// The end of the game's `getConnectionState`: a wire that runs one way only, or none,
/// reaches out on the sides across that way too.
fn wire_cross_where_alone(mut state: BlockState) -> BlockState {
    let connected = |state: BlockState, towards: Face| {
        state
            .property(face_name(towards))
            .is_some_and(|s| s != "none")
    };
    let (north, south) = (connected(state, Face::North), connected(state, Face::South));
    let (east, west) = (connected(state, Face::East), connected(state, Face::West));
    let north_south_empty = !north && !south;
    let east_west_empty = !east && !west;
    if !west && north_south_empty {
        state = set(state, "west", "side");
    }
    if !east && north_south_empty {
        state = set(state, "east", "side");
    }
    if !north && east_west_empty {
        state = set(state, "north", "side");
    }
    if !south && east_west_empty {
        state = set(state, "south", "side");
    }
    state
}

/// The game's `RedStoneWireBlock.updateShape` for a neighbour on the side `towards`:
/// gone where the block below no longer holds it; all sides again where the block
/// above changed; otherwise that side alone, unless that changes whether it connects
/// or the wire is a cross, where all sides are found again.
pub fn redstone_wire_shape<V: BlockView + ?Sized>(
    state: BlockState,
    pos: BlockPos,
    towards: Face,
    neighbour: BlockState,
    view: &mut V,
) -> BlockState {
    match towards {
        Face::Down => {
            if wire_can_stand_on(neighbour) {
                state
            } else {
                blocks::AIR
            }
        }
        Face::Up => wire_connection_state(state, pos, view),
        side => {
            let above = view.block(relative(pos, Face::Up));
            let connection = wire_side(pos, side, !is_conductor(above), view);
            let name = face_name(side);
            let was = state.property(name).is_some_and(|s| s != "none");
            if (connection != "none") == was && !wire_is_cross(state) {
                set(state, name, connection)
            } else {
                let cross = HORIZONTAL.into_iter().fold(state, |cross, towards| {
                    set(cross, face_name(towards), "side")
                });
                wire_connection_state(set(cross, name, connection), pos, view)
            }
        }
    }
}

/// The game's `getConnectionState`: the sides a wire's neighbours connect, kept a dot
/// where it was one and still is.
fn wire_connection_state<V: BlockView + ?Sized>(
    state: BlockState,
    pos: BlockPos,
    view: &mut V,
) -> BlockState {
    let was_dot = wire_is_dot(state);
    // `getMissingConnections` of the default state with this one's power: every side
    // is found again.
    let power = state.property("power").unwrap_or("0");
    let mut found = set(blocks::REDSTONE_WIRE, "power", power);
    let above = view.block(relative(pos, Face::Up));
    let can_connect_up = !is_conductor(above);
    for towards in HORIZONTAL {
        let side = wire_side(pos, towards, can_connect_up, view);
        found = set(found, face_name(towards), side);
    }
    if was_dot && wire_is_dot(found) {
        return found;
    }
    wire_cross_where_alone(found)
}

fn wire_is_cross(state: BlockState) -> bool {
    HORIZONTAL.into_iter().all(|towards| {
        state
            .property(face_name(towards))
            .is_some_and(|s| s != "none")
    })
}

fn wire_is_dot(state: BlockState) -> bool {
    HORIZONTAL
        .into_iter()
        .all(|towards| state.property(face_name(towards)) == Some("none"))
}

/// The game's `getConnectingSide`: how wire at `pos` meets what is on its side
/// `towards`.
fn wire_side<V: BlockView + ?Sized>(
    pos: BlockPos,
    towards: Face,
    can_connect_up: bool,
    view: &mut V,
) -> &'static str {
    let beside_pos = relative(pos, towards);
    let beside = view.block(beside_pos);
    if can_connect_up {
        let holds_wire = is_class(beside, BlockClass::TrapDoorBlock) || wire_can_stand_on(beside);
        if holds_wire && wire_connects_to(view.block(relative(beside_pos, Face::Up)), None) {
            return if sturdy(beside, towards.opposite(), SupportType::Full) {
                "up"
            } else {
                "side"
            };
        }
    }
    let below_beside = view.block(relative(beside_pos, Face::Down));
    if !wire_connects_to(beside, Some(towards))
        && (is_conductor(beside) || !wire_connects_to(below_beside, None))
    {
        "none"
    } else {
        "side"
    }
}

/// The game's `RedStoneWireBlock.shouldConnectTo`.
fn wire_connects_to(state: BlockState, towards: Option<Face>) -> bool {
    if same_block(state, blocks::REDSTONE_WIRE) {
        return true;
    }
    if same_block(state, blocks::REPEATER) {
        let facing = facing(state);
        return towards.is_some() && (towards == facing || towards == facing.map(Face::opposite));
    }
    if same_block(state, blocks::OBSERVER) {
        return towards.is_some() && towards == facing(state);
    }
    towards.is_some() && is_signal_source(state)
}

/// Whether a block gives a redstone signal of its own: the game's `isSignalSource`, by
/// the classes that answer yes.
fn is_signal_source(state: BlockState) -> bool {
    use BlockClass as C;
    [
        C::RedstoneTorchBlock,
        C::LeverBlock,
        C::ButtonBlock,
        C::BasePressurePlateBlock,
        C::PoweredBlock,
        C::DaylightDetectorBlock,
        C::ObserverBlock,
        C::DiodeBlock,
        C::TargetBlock,
        C::TrappedChestBlock,
        C::TripWireHookBlock,
        C::DetectorRailBlock,
        C::LecternBlock,
        C::SculkSensorBlock,
        C::LightningRodBlock,
        C::JukeboxBlock,
    ]
    .into_iter()
    .any(|class| is_class(state, class))
}

/// Whether a block passes redstone power on, the game's `isRedstoneConductor`, as the
/// game answers it of the state (`docs/adr/0026-redstone.md`, section 1). It was a
/// guess from the shapes until the table had the answer, and differed for 32 blocks:
/// TNT, ice, glowstone, sea lanterns, copper bulbs and the chorus flower do not
/// conduct, and spawners and closed shulker boxes do.
pub fn is_conductor(state: BlockState) -> bool {
    state.props().is_redstone_conductor()
}
