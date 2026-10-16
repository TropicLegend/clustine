//! The shapes of what grows, as a neighbour changes, which the simulation's growth
//! needs (`docs/adr/0024-block-behaviour-in-a-region.md`, step B7): the head of kelp or
//! of a vine that has grown becomes body, and the body at the end a head again
//! (`GrowingPlantHeadBlock` and `GrowingPlantBodyBlock.updateShape`); a pointed
//! dripstone or a sulfur spike takes its thickness from those beside it
//! (`SpeleothemBlock.updateShape`); a chorus plant joins what is beside it
//! (`ChorusPlantBlock.updateShape`). Read from the 26.3 jar's classes.
//!
//! Only the simulation asks these: generation places these blocks with their shapes,
//! and its judge compares it with the official server as it is.

use clustine_data::{BlockClass, BlockState, Face, SupportType};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::data::{in_tag, is_class, relative, same_block, tick_id};

/// What `state` becomes, by these rules, when its neighbour towards `direction` is
/// `neighbour`; `None` where they say nothing of it.
pub fn update_shape<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> Option<BlockState> {
    if let Some(plant) = GrowingPlant::of(state) {
        return Some(plant.update_shape(view, state, pos, direction, neighbour));
    }
    if is_class(state, BlockClass::SpeleothemBlock) {
        return Some(speleothem_shape(view, state, pos, direction));
    }
    if is_class(state, BlockClass::ChorusPlantBlock) {
        // Its `canSurvive` is not ported, so the tick it schedules where it cannot
        // stay is not either; the connection on the side that changed is.
        let joins = same_block(neighbour, state)
            || neighbour.block().map(|block| block.name) == Some("minecraft:chorus_flower")
            || (direction == Face::Down && in_tag(neighbour, "minecraft:supports_chorus_plant"));
        let name = face_name(direction);
        let value = if joins { "true" } else { "false" };
        return Some(state.with_property(name, value).unwrap_or(state));
    }
    None
}

/// A head or a body of kelp, cave vines, weeping vines or twisting vines.
#[derive(Debug, Clone, Copy)]
pub struct GrowingPlant {
    /// The head's block, by name without "_plant".
    head: &'static str,
    /// The way it grows.
    pub growth: Face,
    /// Whether this is the head.
    is_head: bool,
}

impl GrowingPlant {
    /// The growing plant `state` is part of, if it is one.
    pub fn of(state: BlockState) -> Option<Self> {
        let name = state.block()?.name;
        let (head, is_head) = match name.strip_suffix("_plant") {
            Some(head) => (head, false),
            None => (name, true),
        };
        let growth = match head {
            "minecraft:kelp" | "minecraft:twisting_vines" => Face::Up,
            "minecraft:cave_vines" | "minecraft:weeping_vines" => Face::Down,
            _ => return None,
        };
        // `head` borrows from the static name, so it is static too.
        Some(Self {
            head,
            growth,
            is_head,
        })
    }

    fn head_block(self) -> BlockState {
        default_of(self.head)
    }

    fn body_block(self) -> BlockState {
        default_of(&format!("{}_plant", self.head))
    }

    /// Whether `state` is this plant's head or body.
    fn is_part(self, state: BlockState) -> bool {
        same_block(state, self.head_block()) || same_block(state, self.body_block())
    }

    /// `GrowingPlantBlock.canSurvive`: on what it can hang or stand by, its own kind or
    /// a sturdy face. Kelp stands on nothing that cannot hold it.
    pub fn can_survive<V: BlockView + ?Sized>(self, view: &mut V, pos: BlockPos) -> bool {
        let attached = view.block(relative(pos, opposite(self.growth)));
        if self.head == "minecraft:kelp" && in_tag(attached, "minecraft:cannot_support_kelp") {
            return false;
        }
        self.is_part(attached)
            || attached
                .props()
                .is_face_sturdy(self.growth, SupportType::Full)
    }

    fn update_shape<V: BlockView + ?Sized>(
        self,
        view: &mut V,
        state: BlockState,
        pos: BlockPos,
        direction: Face,
        neighbour: BlockState,
    ) -> BlockState {
        let kelp = self.head == "minecraft:kelp";
        if self.is_head {
            if direction == opposite(self.growth) {
                if !self.can_survive(view, pos) {
                    view.schedule_block_tick(pos, tick_id(state), 1);
                } else if self.is_part(view.block(relative(pos, self.growth))) {
                    return self.body_from_head(state);
                }
            }
            if direction == self.growth && self.is_part(neighbour) {
                return self.body_from_head(state);
            }
        } else {
            if direction == opposite(self.growth) && !self.can_survive(view, pos) {
                view.schedule_block_tick(pos, tick_id(state), 1);
            }
            if direction == self.growth && !self.is_part(neighbour) {
                // `GrowingPlantHeadBlock.getStateForPlacement`: a head of any age.
                let age = view.next_int(25).to_string();
                let head = self.head_block();
                let head = head.with_property("age", &age).unwrap_or(head);
                return keep_berries(state, head);
            }
        }
        if kelp {
            view.schedule_fluid_tick(pos, crate::fluid::WATER, 5);
        }
        state
    }

    fn body_from_head(self, head: BlockState) -> BlockState {
        keep_berries(head, self.body_block())
    }
}

/// Cave vines keep their berries as a head becomes body and back
/// (`updateBodyAfterConvertedFromHead`, `updateHeadAfterConvertedFromBody`).
fn keep_berries(from: BlockState, to: BlockState) -> BlockState {
    match from.property("berries") {
        Some(berries) => to.with_property("berries", berries).unwrap_or(to),
        None => to,
    }
}

/// `SpeleothemBlock.updateShape`, after its water: up and down it takes its thickness
/// from those beside it, or looks two ticks on (one for a stalagmite) whether it is
/// held. The game leaves a falling stalactite's tick alone (`hasScheduledTick`), which
/// a view cannot ask; Clustine's dripstone does not fall.
fn speleothem_shape<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> BlockState {
    if direction != Face::Up && direction != Face::Down {
        return state;
    }
    let Some(tip) = tip_direction(state) else {
        return state;
    };
    if direction == opposite(tip) && !speleothem_can_survive(view, state, pos) {
        let delay = if tip == Face::Down { 2 } else { 1 };
        view.schedule_block_tick(pos, tick_id(state), delay);
        return state;
    }
    let mergeable = state.property("thickness") == Some("tip_merge");
    let thickness = speleothem_thickness(view, state, pos, tip, mergeable);
    state.with_property("thickness", thickness).unwrap_or(state)
}

/// `SpeleothemBlock.canSurvive`: a sturdy face behind its base, or one of its own
/// pointing the same way.
pub fn speleothem_can_survive<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> bool {
    let Some(tip) = tip_direction(state) else {
        return true;
    };
    let behind = view.block(relative(pos, opposite(tip)));
    behind.props().is_face_sturdy(tip, SupportType::Full)
        || (pointing(behind, tip) && same_block(behind, state))
}

/// `calculateSpeleothemThickness`.
fn speleothem_thickness<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    tip: Face,
    mergeable: bool,
) -> &'static str {
    let ahead = view.block(relative(pos, tip));
    if pointing(ahead, opposite(tip)) && same_block(ahead, state) {
        return if mergeable || ahead.property("thickness") == Some("tip_merge") {
            "tip_merge"
        } else {
            "tip"
        };
    }
    if !pointing(ahead, tip) {
        return "tip";
    }
    if matches!(ahead.property("thickness"), Some("tip" | "tip_merge")) {
        return "frustum";
    }
    if pointing(view.block(relative(pos, opposite(tip))), tip) {
        "middle"
    } else {
        "base"
    }
}

/// `isSpeleothemWithDirection`.
fn pointing(state: BlockState, tip: Face) -> bool {
    in_tag(state, "minecraft:speleothems") && tip_direction(state) == Some(tip)
}

fn tip_direction(state: BlockState) -> Option<Face> {
    match state.property("vertical_direction")? {
        "up" => Some(Face::Up),
        "down" => Some(Face::Down),
        _ => None,
    }
}

/// The default state of the block called `name`.
fn default_of(name: &str) -> BlockState {
    let id =
        clustine_data::block_id_by_name(name).expect("a growing plant's blocks are the game's");
    clustine_data::BLOCKS[id].default_state
}

fn face_name(face: Face) -> &'static str {
    match face {
        Face::Down => "down",
        Face::Up => "up",
        Face::North => "north",
        Face::South => "south",
        Face::West => "west",
        Face::East => "east",
    }
}

fn opposite(face: Face) -> Face {
    match face {
        Face::Down => Face::Up,
        Face::Up => Face::Down,
        Face::North => Face::South,
        Face::South => Face::North,
        Face::West => Face::East,
        Face::East => Face::West,
    }
}

/// Whether a growing plant or a speleothem at `pos` can stay: what the simulation's
/// tick of one asks before it destroys it.
pub fn grows_and_can_stay<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> Option<bool> {
    if let Some(plant) = GrowingPlant::of(state) {
        return Some(plant.can_survive(view, pos));
    }
    if is_class(state, BlockClass::SpeleothemBlock) {
        return Some(speleothem_can_survive(view, state, pos));
    }
    None
}
