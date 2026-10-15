// Adapted from SteelMC's `steel-core/src/worldgen/template/state_transforms.rs`.

//! How a block state turns and mirrors with a template: the game's `BlockState.rotate`
//! and `BlockState.mirror`, which each block's class answers for its own properties.
//!
//! SteelMC does this by the names of the properties alone. The game does it by class,
//! and the classes differ where a name is shared, so this keeps SteelMC's tables and
//! asks the block's class where that matters:
//!
//! - `facing`, any block that has it: turned, and mirrored as the game's
//!   `state.rotate(mirror.getRotation(facing))` does, which comes to mirroring the
//!   direction. A door also swaps its hinge, and a chest (of any kind) its `type`, under
//!   any mirror (`DoorBlock.mirror`, `ChestBlock.mirror`); SteelMC missed the chest.
//! - Stairs mirror their shape only where the facing lies along the mirror's axis, and
//!   are otherwise left as they are, as `StairBlock.mirror` does.
//! - `rotation` (signs, banners, skulls): sixteenths of a turn (`Rotation.rotate`,
//!   `Mirror.mirror` of 16).
//! - `axis`: x and z swap under a quarter turn; a mirror leaves it.
//! - `shape` of rails (`BaseRailBlock` and those made from it) only: the stairs' shape
//!   is theirs.
//! - `north`, `east`, `south`, `west`: the four sides move round (fences, panes, bars,
//!   walls, vines, lichen, mushroom blocks, redstone wire, tripwire, pale moss
//!   carpet). Fire and chorus plants are left: their classes do not turn their sides,
//!   as far as could be told without the game; they take their shape from their
//!   neighbours anyway.
//! - `orientation` (jigsaws and crafters, the game's `FrontAndTop`): both directions
//!   turn or mirror; SteelMC left it.

use clustine_data::{BlockClass, BlockState};

use crate::blocks::is_class;
use crate::geometry::{Axis, Direction, Rotation};
use crate::piece::StructureMirror;

/// `state.mirror(mirror).rotate(rotation)`, as `placeInWorld` turns every block.
pub fn transform_state(
    state: BlockState,
    mirror: StructureMirror,
    rotation: Rotation,
) -> BlockState {
    rotate_state(mirror_state(state, mirror), rotation)
}

/// The game's `BlockState.rotate`.
pub fn rotate_state(state: BlockState, rotation: Rotation) -> BlockState {
    if rotation == Rotation::None {
        return state;
    }
    let original: Vec<(&'static str, &'static str)> = state.properties().collect();
    let mut result = state;
    for &(name, value) in &original {
        let new = match name {
            "facing" => parse_direction(value).map(|d| rotation.rotate(d).as_str()),
            "axis"
                if matches!(
                    rotation,
                    Rotation::Clockwise90 | Rotation::CounterClockwise90
                ) =>
            {
                match value {
                    "x" => Some("z"),
                    "z" => Some("x"),
                    _ => None,
                }
            }
            "rotation" => value.parse::<i32>().ok().map(|segment| {
                let turned = match rotation {
                    Rotation::None => segment,
                    Rotation::Clockwise90 => segment + 4,
                    Rotation::Clockwise180 => segment + 8,
                    Rotation::CounterClockwise90 => segment + 12,
                };
                SIXTEENTHS[(turned % 16) as usize]
            }),
            "shape" if is_class(state, BlockClass::BaseRailBlock) => {
                rotate_rail_shape(value, rotation)
            }
            "north" | "east" | "south" | "west" if turns_its_sides(state) => {
                // The side now at `name` is the one that was at the side it came from.
                let to = side_direction(name);
                let from = inverse(rotation).rotate(to);
                side_value(&original, from)
            }
            "orientation" => map_orientation(value, |d| rotation.rotate(d)),
            _ => None,
        };
        if let Some(new) = new {
            result = with(result, name, new);
        }
    }
    result
}

/// The game's `BlockState.mirror`.
pub fn mirror_state(state: BlockState, mirror: StructureMirror) -> BlockState {
    if mirror == StructureMirror::None {
        return state;
    }
    let original: Vec<(&'static str, &'static str)> = state.properties().collect();
    let facing = state.property("facing").and_then(parse_direction);
    if is_class(state, BlockClass::StairBlock) {
        return mirror_stairs(state, facing, mirror);
    }
    let mut result = state;
    for &(name, value) in &original {
        let new = match name {
            "facing" => facing.map(|d| mirror_direction(d, mirror).as_str()),
            "rotation" => value
                .parse::<i32>()
                .ok()
                .map(|segment| SIXTEENTHS[mirror_segment(segment, mirror) as usize]),
            "hinge" if is_class(state, BlockClass::DoorBlock) => match value {
                "left" => Some("right"),
                "right" => Some("left"),
                _ => None,
            },
            "type" if is_class(state, BlockClass::ChestBlock) => match value {
                "left" => Some("right"),
                "right" => Some("left"),
                _ => None,
            },
            "shape" if is_class(state, BlockClass::BaseRailBlock) => {
                mirror_rail_shape(value, mirror)
            }
            "north" | "east" | "south" | "west" if turns_its_sides(state) => {
                let from = mirror_direction(side_direction(name), mirror);
                side_value(&original, from)
            }
            "orientation" => map_orientation(value, |d| mirror_direction(d, mirror)),
            _ => None,
        };
        if let Some(new) = new {
            result = with(result, name, new);
        }
    }
    result
}

const SIXTEENTHS: [&str; 16] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
];

fn with(state: BlockState, name: &str, value: &str) -> BlockState {
    state
        .with_property(name, value)
        .expect("a turned value is one the property has")
}

/// Whether the block's class moves its four sides round when it turns or mirrors.
fn turns_its_sides(state: BlockState) -> bool {
    !(is_class(state, BlockClass::BaseFireBlock) || is_class(state, BlockClass::PipeBlock))
}

fn side_direction(name: &str) -> Direction {
    match name {
        "north" => Direction::North,
        "east" => Direction::East,
        "south" => Direction::South,
        _ => Direction::West,
    }
}

fn side_value(
    properties: &[(&'static str, &'static str)],
    side: Direction,
) -> Option<&'static str> {
    properties
        .iter()
        .find(|(name, _)| *name == side.as_str())
        .map(|(_, value)| *value)
}

fn inverse(rotation: Rotation) -> Rotation {
    match rotation {
        Rotation::None => Rotation::None,
        Rotation::Clockwise90 => Rotation::CounterClockwise90,
        Rotation::Clockwise180 => Rotation::Clockwise180,
        Rotation::CounterClockwise90 => Rotation::Clockwise90,
    }
}

pub(crate) fn parse_direction(value: &str) -> Option<Direction> {
    Some(match value {
        "down" => Direction::Down,
        "up" => Direction::Up,
        "north" => Direction::North,
        "south" => Direction::South,
        "west" => Direction::West,
        "east" => Direction::East,
        _ => return None,
    })
}

/// The game's `Mirror.mirror(Direction)`.
pub fn mirror_direction(direction: Direction, mirror: StructureMirror) -> Direction {
    match (mirror, direction) {
        (StructureMirror::FrontBack, Direction::West | Direction::East)
        | (StructureMirror::LeftRight, Direction::North | Direction::South) => direction.opposite(),
        _ => direction,
    }
}

/// The game's `Mirror.mirror(rotation, 16)`.
fn mirror_segment(rotation: i32, mirror: StructureMirror) -> i32 {
    const STEPS: i32 = 16;
    let half = STEPS / 2;
    let corrected = if rotation > half {
        rotation - STEPS
    } else {
        rotation
    };
    match mirror {
        StructureMirror::LeftRight => (half - corrected + STEPS) % STEPS,
        StructureMirror::FrontBack => (STEPS - corrected) % STEPS,
        StructureMirror::None => rotation,
    }
}

/// `StairBlock.mirror`: turned half round with the shape's hand swapped where the
/// facing lies along the mirror's axis; otherwise as it was (the game falls back to
/// `Block.mirror`, which changes nothing).
fn mirror_stairs(
    state: BlockState,
    facing: Option<Direction>,
    mirror: StructureMirror,
) -> BlockState {
    let Some(facing) = facing else {
        return state;
    };
    let shape = state.property("shape").unwrap_or("straight");
    let new_shape = match (mirror, facing.axis()) {
        (StructureMirror::LeftRight, Axis::Z) => match shape {
            "inner_left" => "inner_right",
            "inner_right" => "inner_left",
            "outer_left" => "outer_right",
            "outer_right" => "outer_left",
            other => other,
        },
        (StructureMirror::FrontBack, Axis::X) => match shape {
            "outer_left" => "outer_right",
            "outer_right" => "outer_left",
            other => other,
        },
        _ => return state,
    };
    let turned = rotate_state(state, Rotation::Clockwise180);
    with(turned, "shape", new_shape)
}

/// A `FrontAndTop` value (`north_up`, `up_east`) with both its directions mapped.
fn map_orientation(value: &str, map: impl Fn(Direction) -> Direction) -> Option<&'static str> {
    let (front, top) = value.split_once('_')?;
    let (front, top) = (map(parse_direction(front)?), map(parse_direction(top)?));
    ORIENTATIONS
        .iter()
        .find(|name| **name == format!("{}_{}", front.as_str(), top.as_str()))
        .copied()
}

const ORIENTATIONS: [&str; 12] = [
    "down_east",
    "down_north",
    "down_south",
    "down_west",
    "up_east",
    "up_north",
    "up_south",
    "up_west",
    "west_up",
    "east_up",
    "north_up",
    "south_up",
];

fn rotate_rail_shape(shape: &str, rotation: Rotation) -> Option<&'static str> {
    Some(match rotation {
        Rotation::Clockwise180 => match shape {
            "ascending_east" => "ascending_west",
            "ascending_west" => "ascending_east",
            "ascending_north" => "ascending_south",
            "ascending_south" => "ascending_north",
            "south_east" => "north_west",
            "south_west" => "north_east",
            "north_west" => "south_east",
            "north_east" => "south_west",
            _ => return None,
        },
        Rotation::CounterClockwise90 => match shape {
            "ascending_east" => "ascending_north",
            "ascending_west" => "ascending_south",
            "ascending_north" => "ascending_west",
            "ascending_south" => "ascending_east",
            "north_south" => "east_west",
            "east_west" => "north_south",
            "south_east" => "north_east",
            "south_west" => "south_east",
            "north_west" => "south_west",
            "north_east" => "north_west",
            _ => return None,
        },
        Rotation::Clockwise90 => match shape {
            "ascending_east" => "ascending_south",
            "ascending_west" => "ascending_north",
            "ascending_north" => "ascending_east",
            "ascending_south" => "ascending_west",
            "north_south" => "east_west",
            "east_west" => "north_south",
            "south_east" => "south_west",
            "south_west" => "north_west",
            "north_west" => "north_east",
            "north_east" => "south_east",
            _ => return None,
        },
        Rotation::None => return None,
    })
}

fn mirror_rail_shape(shape: &str, mirror: StructureMirror) -> Option<&'static str> {
    Some(match mirror {
        StructureMirror::LeftRight => match shape {
            "ascending_north" => "ascending_south",
            "ascending_south" => "ascending_north",
            "south_east" => "north_east",
            "south_west" => "north_west",
            "north_west" => "south_west",
            "north_east" => "south_east",
            _ => return None,
        },
        StructureMirror::FrontBack => match shape {
            "ascending_east" => "ascending_west",
            "ascending_west" => "ascending_east",
            "south_east" => "south_west",
            "south_west" => "south_east",
            "north_west" => "north_east",
            "north_east" => "north_west",
            _ => return None,
        },
        StructureMirror::None => return None,
    })
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;

    use super::*;

    fn state(text: &str) -> BlockState {
        BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a state"))
    }

    #[test]
    fn a_facing_turns_with_the_template_and_mirrors_across_its_axis() {
        let stairs = state("minecraft:oak_stairs[facing=north]");
        assert_eq!(
            rotate_state(stairs, Rotation::Clockwise90),
            state("minecraft:oak_stairs[facing=east]")
        );
        assert_eq!(
            rotate_state(stairs, Rotation::CounterClockwise90),
            state("minecraft:oak_stairs[facing=west]")
        );
        let furnace = state("minecraft:furnace[facing=west]");
        assert_eq!(
            mirror_state(furnace, StructureMirror::FrontBack),
            state("minecraft:furnace[facing=east]")
        );
        assert_eq!(mirror_state(furnace, StructureMirror::LeftRight), furnace);
        let observer = state("minecraft:observer[facing=up]");
        assert_eq!(rotate_state(observer, Rotation::Clockwise90), observer);
    }

    #[test]
    fn stairs_mirror_their_shape_only_along_the_mirrors_axis() {
        let along = state("minecraft:oak_stairs[facing=north,shape=inner_left]");
        assert_eq!(
            mirror_state(along, StructureMirror::LeftRight),
            state("minecraft:oak_stairs[facing=south,shape=inner_right]")
        );
        let across = state("minecraft:oak_stairs[facing=east,shape=inner_left]");
        assert_eq!(mirror_state(across, StructureMirror::LeftRight), across);
        let front_back = state("minecraft:oak_stairs[facing=east,shape=inner_left]");
        assert_eq!(
            mirror_state(front_back, StructureMirror::FrontBack),
            state("minecraft:oak_stairs[facing=west,shape=inner_left]")
        );
        let outer = state("minecraft:oak_stairs[facing=west,shape=outer_left]");
        assert_eq!(
            mirror_state(outer, StructureMirror::FrontBack),
            state("minecraft:oak_stairs[facing=east,shape=outer_right]")
        );
    }

    #[test]
    fn a_mirror_swaps_a_doors_hinge_and_a_chests_half_whatever_their_facing() {
        let door = state("minecraft:oak_door[facing=north,hinge=left]");
        assert_eq!(
            mirror_state(door, StructureMirror::FrontBack),
            state("minecraft:oak_door[facing=north,hinge=right]")
        );
        let chest = state("minecraft:chest[facing=east,type=left]");
        assert_eq!(
            mirror_state(chest, StructureMirror::LeftRight),
            state("minecraft:chest[facing=east,type=right]")
        );
        let slab = state("minecraft:oak_slab[type=top]");
        assert_eq!(mirror_state(slab, StructureMirror::LeftRight), slab);
    }

    #[test]
    fn a_fence_moves_its_sides_round_and_fire_keeps_them() {
        let fence = state("minecraft:oak_fence[north=true]");
        assert_eq!(
            rotate_state(fence, Rotation::Clockwise90),
            state("minecraft:oak_fence[east=true]")
        );
        assert_eq!(
            mirror_state(fence, StructureMirror::LeftRight),
            state("minecraft:oak_fence[south=true]")
        );
        let wall = state("minecraft:cobblestone_wall[west=tall]");
        assert_eq!(
            rotate_state(wall, Rotation::Clockwise180),
            state("minecraft:cobblestone_wall[east=tall]")
        );
        let fire = state("minecraft:fire[north=true]");
        assert_eq!(rotate_state(fire, Rotation::Clockwise90), fire);
    }

    #[test]
    fn a_signs_sixteenths_turn_and_mirror() {
        let sign = state("minecraft:oak_sign[rotation=1]");
        assert_eq!(
            rotate_state(sign, Rotation::CounterClockwise90),
            state("minecraft:oak_sign[rotation=13]")
        );
        assert_eq!(
            mirror_state(sign, StructureMirror::FrontBack),
            state("minecraft:oak_sign[rotation=15]")
        );
        assert_eq!(
            mirror_state(sign, StructureMirror::LeftRight),
            state("minecraft:oak_sign[rotation=7]")
        );
    }

    #[test]
    fn an_axis_swaps_on_a_quarter_turn_and_a_rail_turns_its_shape() {
        let log = state("minecraft:oak_log[axis=x]");
        assert_eq!(
            rotate_state(log, Rotation::Clockwise90),
            state("minecraft:oak_log[axis=z]")
        );
        assert_eq!(rotate_state(log, Rotation::Clockwise180), log);
        let rail = state("minecraft:rail[shape=north_east]");
        assert_eq!(
            rotate_state(rail, Rotation::Clockwise90),
            state("minecraft:rail[shape=south_east]")
        );
        assert_eq!(
            mirror_state(rail, StructureMirror::FrontBack),
            state("minecraft:rail[shape=north_west]")
        );
    }

    #[test]
    fn a_crafters_orientation_turns_both_its_directions() {
        let crafter = state("minecraft:crafter[orientation=north_up]");
        assert_eq!(
            rotate_state(crafter, Rotation::Clockwise90),
            state("minecraft:crafter[orientation=east_up]")
        );
        let down = state("minecraft:crafter[orientation=down_west]");
        assert_eq!(
            mirror_state(down, StructureMirror::FrontBack),
            state("minecraft:crafter[orientation=down_east]")
        );
    }

    #[test]
    fn a_block_without_turning_properties_is_left_alone() {
        assert_eq!(
            transform_state(
                blocks::STONE,
                StructureMirror::LeftRight,
                Rotation::Clockwise90
            ),
            blocks::STONE
        );
    }
}
