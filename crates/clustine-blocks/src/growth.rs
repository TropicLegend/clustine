//! The shapes of what grows, as a neighbour changes, which the simulation's growth
//! needs (`docs/adr/0024-block-behaviour-in-a-region.md`, step B7): the head of kelp or
//! of a vine that has grown becomes body, and the body at the end a head again
//! (`GrowingPlantHeadBlock` and `GrowingPlantBodyBlock.updateShape`); a pointed
//! dripstone or a sulfur spike takes its thickness from those beside it
//! (`SpeleothemBlock.updateShape`); a chorus plant joins what is beside it
//! (`ChorusPlantBlock.updateShape`); a bed's half goes with the other
//! (`AbstractBedBlock.updateShape`); and a chest joins the chest placed beside it as
//! the other half of a double chest, and is single again when that half goes
//! (`ChestBlock.updateShape`, `CopperChestBlock.updateShape`). Read from the 26.3 jar's
//! classes.
//!
//! Generation asks these as well, where a template's edge meets what is beside it
//! (`updateShapeAtEdge`): kelp there schedules its water, as the game's does. Its
//! `next_int` answers nought, where the game draws the age of a body that becomes a
//! head; the judge would show that as a difference in blocks.

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
    if is_class(state, BlockClass::AbstractBedBlock) {
        return Some(bed_shape(state, direction, neighbour));
    }
    if is_class(state, BlockClass::ChestBlock) {
        return Some(chest_shape(state, direction, neighbour));
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

/// `AbstractBedBlock.updateShape`: a half whose other half is gone goes too, and one
/// whose other half is there shares whether the bed is taken.
fn bed_shape(state: BlockState, direction: Face, neighbour: BlockState) -> BlockState {
    if Some(direction) != bed_other_half(state) {
        return state;
    }
    if same_block(neighbour, state) && neighbour.property("part") != state.property("part") {
        let occupied = neighbour.property("occupied").unwrap_or("false");
        return state.with_property("occupied", occupied).unwrap_or(state);
    }
    clustine_data::blocks::AIR
}

/// `AbstractBedBlock.getNeighbourDirection`: the way to a bed's other half, the way it
/// faces from its foot and back from its head.
pub fn bed_other_half(state: BlockState) -> Option<Face> {
    let facing = crate::data::facing(state)?;
    match state.property("part")? {
        "foot" => Some(facing),
        _ => Some(opposite(facing)),
    }
}

/// `ChestBlock.updateShape`, after its water, and `CopperChestBlock.updateShape`.
///
/// A single chest beside a chest it can pair with that is already half of a double
/// chest, faces the same way and has its other half here, becomes that other half.
/// A double chest whose other half is no longer a chest it pairs with is single again.
/// A copper chest that pairs takes its partner's block, so that both halves weather
/// alike: the chest placed second chose the least weathered of the two as it was
/// placed ([`least_weathered_chest`]).
fn chest_shape(state: BlockState, direction: Face, neighbour: BlockState) -> BlockState {
    let connects = chest_can_connect_to(state, neighbour);
    let mut shape = state;
    if connects && crate::data::is_horizontal(direction) {
        let theirs = neighbour.property("type");
        if state.property("type") == Some("single")
            && theirs.is_some_and(|kind| kind != "single")
            && crate::data::facing(state) == crate::data::facing(neighbour)
            && chest_connected_direction(neighbour) == Some(opposite(direction))
        {
            let ours = if theirs == Some("left") {
                "right"
            } else {
                "left"
            };
            shape = crate::data::set(state, "type", ours);
        }
    } else if chest_connected_direction(state) == Some(direction) {
        shape = crate::data::set(state, "type", "single");
    }
    if is_class(state, BlockClass::CopperChestBlock)
        && connects
        && shape.property("type") != Some("single")
        && chest_connected_direction(shape) == Some(direction)
    {
        return with_properties_of(neighbour, shape);
    }
    shape
}

/// `ChestBlock.chestCanConnectTo`: whether a chest of `chest`'s block pairs with
/// `other`. A chest or a trapped chest pairs only with its own block; a copper chest
/// with any copper chest, whatever its weathering or wax
/// (`CopperChestBlock.chestCanConnectTo`).
pub fn chest_can_connect_to(chest: BlockState, other: BlockState) -> bool {
    if is_class(chest, BlockClass::CopperChestBlock) {
        return in_tag(other, "minecraft:copper_chests") && other.property("type").is_some();
    }
    crate::data::is_block(other, chest)
}

/// `ChestBlock.getConnectedDirection`: the way to a chest's other half, clockwise from
/// its facing for the left half and counter-clockwise otherwise. A single chest
/// answers as a right half does, as in the game.
pub fn chest_connected_direction(chest: BlockState) -> Option<Face> {
    let facing = crate::data::facing(chest)?;
    Some(if chest.property("type") == Some("left") {
        crate::data::clockwise(facing)
    } else {
        crate::data::counter_clockwise(facing)
    })
}

/// `CopperChestBlock.getLeastOxidizedChestOfConnectedBlocks`: a copper chest placed as
/// half of a double chest becomes the least weathered of itself and its partner, as
/// waxed only where both are; the other half takes its block as it pairs
/// ([`update_shape`]). `partner` is the block towards its other half.
pub fn least_weathered_chest(placed: BlockState, partner: BlockState) -> BlockState {
    if placed.property("type") == Some("single")
        || !is_class(placed, BlockClass::CopperChestBlock)
        || !is_class(partner, BlockClass::CopperChestBlock)
    {
        return placed;
    }
    let (Some(ours), Some(theirs)) = (copper_chest_age(placed), copper_chest_age(partner)) else {
        return placed;
    };
    let (mut ours_state, mut theirs_state) = (placed, partner);
    if ours.waxed != theirs.waxed {
        ours_state = unwaxed(ours_state);
        theirs_state = unwaxed(theirs_state);
    }
    let block = if ours.weathering <= theirs.weathering {
        ours_state
    } else {
        theirs_state
    };
    with_properties_of(block, placed)
}

/// How weathered a copper chest is, and whether it is waxed.
#[derive(Debug, Clone, Copy)]
struct CopperAge {
    /// `WeatheringCopper.WeatherState`'s order: unaffected, exposed, weathered,
    /// oxidized.
    weathering: u8,
    waxed: bool,
}

fn copper_chest_age(state: BlockState) -> Option<CopperAge> {
    let name = state.block()?.name.strip_prefix("minecraft:")?;
    let (waxed, name) = match name.strip_prefix("waxed_") {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    let weathering = match name {
        "copper_chest" => 0,
        "exposed_copper_chest" => 1,
        "weathered_copper_chest" => 2,
        "oxidized_copper_chest" => 3,
        _ => return None,
    };
    Some(CopperAge { weathering, waxed })
}

/// `CopperChestBlock.unwaxBlock`: a waxed copper chest as the same chest unwaxed
/// (`HoneycombItem.WAX_OFF_BY_BLOCK`), anything else as it is.
fn unwaxed(state: BlockState) -> BlockState {
    let Some(name) = state.block().map(|block| block.name) else {
        return state;
    };
    let Some(rest) = name.strip_prefix("minecraft:waxed_") else {
        return state;
    };
    match clustine_data::block_id_by_name(&format!("minecraft:{rest}")) {
        Some(id) => with_properties_of(clustine_data::BLOCKS[id].default_state, state),
        None => state,
    }
}

/// `Block.withPropertiesOf`: the block of `block` with each property of `from` that it
/// has too.
fn with_properties_of(block: BlockState, from: BlockState) -> BlockState {
    let default = block
        .block_id()
        .map_or(block, |id| clustine_data::BLOCKS[id].default_state);
    from.properties().fold(default, |state, (name, value)| {
        crate::data::set(state, name, value)
    })
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
/// held. A stalactite whose tick is due keeps its shape until it falls
/// (`hasScheduledTick`).
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
    if tip == Face::Down && view.has_scheduled_block_tick(pos, tick_id(state)) {
        return state;
    }
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
