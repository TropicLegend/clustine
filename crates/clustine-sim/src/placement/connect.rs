// Adapted from SteelMC's `steel-core/src/behavior/blocks/building/fence_block.rs`,
// `bar_block.rs`, `wall_block.rs`, `stair_block.rs`, `blocks/utils.rs` and
// `redstone/wire/block.rs`.

//! The shapes that fences, panes, walls, stairs and redstone wire take from their
//! neighbours as they are placed (section 4.4, the owner's choice 2). The neighbours do
//! not change in turn.

use clustine_data::Face as Dir;
use clustine_data::{BlockClass, BlockState, SupportType, blocks};
use clustine_world::BlockPos;

use super::context::World;
use super::data;
use super::geometry::{self, Axis, DirExt, HORIZONTAL};
use super::survive::{self, facing, sturdy};

/// Vanilla's `Block.isExceptionForConnection`: blocks whose sturdy sides nothing joins.
fn is_exception(state: BlockState) -> bool {
    data::is(state, BlockClass::LeavesBlock)
        || data::in_tag(state, "minecraft:shulker_boxes")
        || [
            blocks::BARRIER,
            blocks::CARVED_PUMPKIN,
            blocks::JACK_O_LANTERN,
            blocks::MELON,
            blocks::PUMPKIN,
        ]
        .into_iter()
        .any(|other| data::same_block(state, other))
}

/// Whether the side of `neighbour` that faces back across `towards` (the side from the
/// placed block to it) is sturdy and not an exception.
fn sturdy_towards(neighbour: BlockState, towards: Dir) -> bool {
    !is_exception(neighbour) && sturdy(neighbour, towards.opposite(), SupportType::Full)
}

/// Whether a fence gate lies across `towards`, so that what is on that side of it
/// joins it: vanilla's `FenceGateBlock.connectsToDirection`.
fn gate_across(neighbour: BlockState, towards: Dir) -> bool {
    data::is(neighbour, BlockClass::FenceGateBlock)
        && facing(neighbour).is_some_and(|gate| gate.axis() == towards.clockwise().axis())
}

/// A fence with the connections its neighbours give it.
pub(crate) fn fence(state: BlockState, pos: BlockPos, world: &World) -> BlockState {
    let wooden = data::in_tag(state, "minecraft:wooden_fences");
    let mut state = state;
    for towards in HORIZONTAL {
        let neighbour = world.get(towards.relative(pos));
        let same_fence = data::in_tag(neighbour, "minecraft:fences")
            && data::in_tag(neighbour, "minecraft:wooden_fences") == wooden;
        let joins =
            sturdy_towards(neighbour, towards) || same_fence || gate_across(neighbour, towards);
        state = data::set_flag(state, towards.name(), joins);
    }
    state
}

/// A pane or bars with the connections its neighbours give it.
pub(crate) fn pane(state: BlockState, pos: BlockPos, world: &World) -> BlockState {
    let mut state = state;
    for towards in HORIZONTAL {
        let neighbour = world.get(towards.relative(pos));
        let joins = sturdy_towards(neighbour, towards)
            || data::is(neighbour, BlockClass::IronBarsBlock)
            || data::in_tag(neighbour, "minecraft:walls");
        state = data::set_flag(state, towards.name(), joins);
    }
    state
}

/// A wall with the sides and the post its neighbours and the block above give it.
pub(crate) fn wall(state: BlockState, pos: BlockPos, world: &World) -> BlockState {
    let above = world.get(Dir::Up.relative(pos));
    let above_boxes: Vec<[f64; 6]> = above.props().collision_shape().boxes().collect();
    // The part of the bottom of the block above that a side of the wall reaches up to,
    // in x and z: vanilla's `TEST_SHAPES_WALL`.
    let arm = |towards: Dir| -> [f64; 4] {
        let (low, high) = (7.0 / 16.0, 9.0 / 16.0);
        match towards {
            Dir::North => [low, 0.0, high, high],
            Dir::South => [low, low, high, 1.0],
            Dir::West => [0.0, low, high, high],
            _ => [low, low, 1.0, high],
        }
    };
    let mut state = state;
    let mut sides = [("none", Dir::North); 4];
    for (slot, towards) in sides.iter_mut().zip(HORIZONTAL) {
        let neighbour = world.get(towards.relative(pos));
        let joins = data::in_tag(neighbour, "minecraft:walls")
            || sturdy_towards(neighbour, towards)
            || data::is(neighbour, BlockClass::IronBarsBlock)
            || gate_across(neighbour, towards);
        let height = if !joins {
            "none"
        } else if geometry::face_covers(&above_boxes, Dir::Down, arm(towards)) {
            "tall"
        } else {
            "low"
        };
        *slot = (height, towards);
        state = data::set(state, towards.name(), height);
    }
    let side = |dir: Dir| {
        sides
            .iter()
            .find(|(_, d)| *d == dir)
            .map_or("none", |s| s.0)
    };
    let post = if data::in_tag(above, "minecraft:walls") && data::flag(above, "up") {
        true
    } else {
        let (north, south) = (side(Dir::North), side(Dir::South));
        let (east, west) = (side(Dir::East), side(Dir::West));
        let all_none = [north, south, east, west].iter().all(|s| *s == "none");
        let corner = all_none
            || (north == "none") != (south == "none")
            || (east == "none") != (west == "none");
        if corner {
            true
        } else if (north == "tall" && south == "tall") || (east == "tall" && west == "tall") {
            false
        } else {
            let post_area = [7.0 / 16.0, 7.0 / 16.0, 9.0 / 16.0, 9.0 / 16.0];
            data::in_tag(above, "minecraft:wall_post_override")
                || geometry::face_covers(&above_boxes, Dir::Down, post_area)
        }
    };
    data::set_flag(state, "up", post)
}

/// Whether `state` is stairs.
fn is_stairs(state: BlockState) -> bool {
    data::is(state, BlockClass::StairBlock)
}

/// Stairs with the shape the stairs beside them give: vanilla's `getStairsShape`.
pub(crate) fn stairs(state: BlockState, pos: BlockPos, world: &World) -> BlockState {
    let Some(own_facing) = facing(state) else {
        return state;
    };
    let half = state.property("half");
    // Whether the stairs on the side `side` would not take the same shape, so that the
    // corner may form.
    let can_take_shape = |side: Dir| {
        let neighbour = world.get(side.relative(pos));
        !is_stairs(neighbour)
            || facing(neighbour) != Some(own_facing)
            || neighbour.property("half") != half
    };
    let shape = 'shape: {
        let behind = world.get(own_facing.relative(pos));
        if is_stairs(behind)
            && behind.property("half") == half
            && let Some(theirs) = facing(behind)
            && theirs.axis() != own_facing.axis()
            && can_take_shape(theirs.opposite())
        {
            break 'shape if theirs == own_facing.counter_clockwise() {
                "outer_left"
            } else {
                "outer_right"
            };
        }
        let front = world.get(own_facing.opposite().relative(pos));
        if is_stairs(front)
            && front.property("half") == half
            && let Some(theirs) = facing(front)
            && theirs.axis() != own_facing.axis()
            && can_take_shape(theirs)
        {
            break 'shape if theirs == own_facing.counter_clockwise() {
                "inner_left"
            } else {
                "inner_right"
            };
        }
        "straight"
    };
    data::set(state, "shape", shape)
}

/// Redstone wire as placed: vanilla's `getConnectionState` of a cross, which keeps the
/// sides its neighbours connect and is a cross where none does.
pub(crate) fn redstone_wire(state: BlockState, pos: BlockPos, world: &World) -> BlockState {
    let mut state = state;
    let above = world.get(Dir::Up.relative(pos));
    let can_connect_up = !is_conductor(above);
    for towards in HORIZONTAL {
        state = data::set(
            state,
            towards.name(),
            wire_side(pos, towards, can_connect_up, world),
        );
    }
    let connected = |dir: Dir| state.property(dir.name()) != Some("none");
    let (north, south) = (connected(Dir::North), connected(Dir::South));
    let (east, west) = (connected(Dir::East), connected(Dir::West));
    let north_south_empty = !north && !south;
    let east_west_empty = !east && !west;
    if !west && north_south_empty {
        state = data::set(state, "west", "side");
    }
    if !east && north_south_empty {
        state = data::set(state, "east", "side");
    }
    if !north && east_west_empty {
        state = data::set(state, "north", "side");
    }
    if !south && east_west_empty {
        state = data::set(state, "south", "side");
    }
    state
}

/// Vanilla's `getConnectingSide`: how wire at `pos` meets what is on its side `towards`.
fn wire_side(pos: BlockPos, towards: Dir, can_connect_up: bool, world: &World) -> &'static str {
    let beside_pos = towards.relative(pos);
    let beside = world.get(beside_pos);
    if can_connect_up {
        let holds_wire =
            data::is(beside, BlockClass::TrapDoorBlock) || survive::wire_can_stand_on(beside);
        if holds_wire && connects_to(world.get(Dir::Up.relative(beside_pos)), None) {
            return if sturdy(beside, towards.opposite(), SupportType::Full) {
                "up"
            } else {
                "side"
            };
        }
    }
    let below_beside = world.get(Dir::Down.relative(beside_pos));
    if !connects_to(beside, Some(towards))
        && (is_conductor(beside) || !connects_to(below_beside, None))
    {
        "none"
    } else {
        "side"
    }
}

/// Vanilla's `RedStoneWireBlock.shouldConnectTo`.
fn connects_to(state: BlockState, towards: Option<Dir>) -> bool {
    if data::same_block(state, blocks::REDSTONE_WIRE) {
        return true;
    }
    if data::same_block(state, blocks::REPEATER) {
        let facing = facing(state);
        return towards.is_some() && (towards == facing || towards == facing.map(Dir::opposite));
    }
    if data::same_block(state, blocks::OBSERVER) {
        return towards.is_some() && towards == facing(state);
    }
    towards.is_some() && is_signal_source(state)
}

/// Whether a block gives a redstone signal of its own: vanilla's `isSignalSource`, by
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
    .any(|class| data::is(state, class))
}

/// Whether a block passes redstone power on, vanilla's `isRedstoneConductor`. By
/// default that is a block whose collision is a full cube; the classes here say "never"
/// in the game's code, and soul sand and mud "always". Not taken from the jar: an
/// approximation that W6f judges.
pub(crate) fn is_conductor(state: BlockState) -> bool {
    use BlockClass as C;
    if data::same_block(state, blocks::SOUL_SAND) || data::same_block(state, blocks::MUD) {
        return true;
    }
    let never = [
        C::TransparentBlock,
        C::LeavesBlock,
        C::PistonBaseBlock,
        C::ObserverBlock,
        C::PoweredBlock,
        C::BeaconBlock,
        C::SpawnerBlock,
        C::ShulkerBoxBlock,
    ]
    .into_iter()
    .any(|class| data::is(state, class));
    !never && state.props().collision_is_full_block()
}

/// Whether a fence gate placed at `pos` along `axis` is in a wall: a wall is beside it
/// across its axis.
pub(crate) fn gate_in_wall(pos: BlockPos, facing_axis: Axis, world: &World) -> bool {
    let sides = match facing_axis {
        Axis::Z => [Dir::West, Dir::East],
        Axis::X => [Dir::North, Dir::South],
        Axis::Y => return false,
    };
    sides
        .into_iter()
        .any(|side| data::in_tag(world.get(side.relative(pos)), "minecraft:walls"))
}
