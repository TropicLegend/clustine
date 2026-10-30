//! What a fluid reads of the world to know where it flows: the game's `FlowingFluid`,
//! `WaterFluid` and `LavaFluid` in the overworld, read from the 26.3 jar's classes. What
//! a fluid's tick then sets (`spread`, `spreadTo`, a block a fluid fills) is the
//! simulation's, which owns setting blocks; this is the arithmetic both sides share.
//!
//! A fluid state is what a block holds: the water or lava of a liquid block, the water
//! of a waterlogged block. Flowing and still water are two fluids, as in the game,
//! and a scheduled tick names which one it is for (`ServerLevel.tickFluid` runs it only
//! where the block still holds exactly that fluid).
//!
//! The overworld's rules are taken: water turns into a source between two sources
//! (the game rule `water_source_conversion`, on by default) and lava does not
//! (`lava_source_conversion`, off), and lava is slow (it is fast only where the
//! dimension says so, in the Nether, which Clustine does not have yet).

use clustine_data::{BlockClass, BlockState, Face, Shape, blocks};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::data::{HORIZONTAL, in_tag, is_block, is_class, relative};

/// The fluids by the game's ids (`clustine_data::FLUIDS`).
pub const EMPTY: u8 = 0;
pub const FLOWING_WATER: u8 = 1;
pub const WATER: u8 = 2;
pub const FLOWING_LAVA: u8 = 3;
pub const LAVA: u8 = 4;

/// Water or lava, flowing or still: what `Fluid.isSame` tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Water,
    Lava,
}

impl Kind {
    /// The kind of the fluid `fluid`, none for the empty fluid.
    pub fn of(fluid: u8) -> Option<Self> {
        match fluid {
            FLOWING_WATER | WATER => Some(Self::Water),
            FLOWING_LAVA | LAVA => Some(Self::Lava),
            _ => None,
        }
    }

    /// The still fluid, which a source holds (`getSource`).
    pub fn source(self) -> u8 {
        match self {
            Self::Water => WATER,
            Self::Lava => LAVA,
        }
    }

    /// The flowing fluid (`getFlowing`).
    pub fn flowing(self) -> u8 {
        match self {
            Self::Water => FLOWING_WATER,
            Self::Lava => FLOWING_LAVA,
        }
    }

    /// `getTickDelay`, in a level whose lava is fast (`fast_lava`, the Nether's) or not
    /// (`LavaFluid.txt` 317–388).
    pub fn tick_delay(self, fast_lava: bool) -> i32 {
        match (self, fast_lava) {
            (Self::Water, _) => 5,
            (Self::Lava, true) => 10,
            (Self::Lava, false) => 30,
        }
    }

    /// `getDropOff`: how much less a block beside holds.
    pub fn drop_off(self, fast_lava: bool) -> i32 {
        match (self, fast_lava) {
            (Self::Water, _) | (Self::Lava, true) => 1,
            (Self::Lava, false) => 2,
        }
    }

    /// `getSlopeFindDistance`: how far a fluid looks for a way down.
    pub fn slope_find_distance(self, fast_lava: bool) -> i32 {
        match (self, fast_lava) {
            (Self::Water, _) | (Self::Lava, true) => 4,
            (Self::Lava, false) => 2,
        }
    }

    /// `canConvertToSource`, by the game rules' defaults.
    pub fn converts_to_source(self) -> bool {
        self == Self::Water
    }
}

/// What a block holds of a fluid (`FluidState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidState {
    /// The fluid's id; [`EMPTY`] for none.
    pub fluid: u8,
    /// How much of the block it fills, from 1 to 8; a source has 8.
    pub amount: u8,
    /// Whether it falls from the block above, which fills the block whatever its
    /// amount.
    pub falling: bool,
}

impl FluidState {
    pub const EMPTY: Self = Self {
        fluid: EMPTY,
        amount: 0,
        falling: false,
    };

    /// The fluid `state` holds.
    pub fn of(state: BlockState) -> Self {
        state.props().fluid().map_or(Self::EMPTY, |fluid| Self {
            fluid: fluid.fluid,
            amount: fluid.amount,
            falling: fluid.falling,
        })
    }

    /// `getSource(falling)` of `kind`.
    pub fn source(kind: Kind, falling: bool) -> Self {
        Self {
            fluid: kind.source(),
            amount: 8,
            falling,
        }
    }

    /// `getFlowing(amount, falling)` of `kind`.
    pub fn flowing(kind: Kind, amount: u8, falling: bool) -> Self {
        Self {
            fluid: kind.flowing(),
            amount,
            falling,
        }
    }

    pub fn is_empty(self) -> bool {
        self.fluid == EMPTY
    }

    pub fn is_source(self) -> bool {
        matches!(self.fluid, WATER | LAVA)
    }

    pub fn kind(self) -> Option<Kind> {
        Kind::of(self.fluid)
    }

    /// `getOwnHeight`: the amount in ninths, in the game's single precision.
    pub fn own_height(self) -> f32 {
        f32::from(self.amount) / 9.0
    }

    /// `createLegacyBlock`: the liquid block that holds this fluid, air for none.
    pub fn legacy_block(self) -> BlockState {
        let block = match self.kind() {
            None => return blocks::AIR,
            Some(Kind::Water) => blocks::WATER,
            Some(Kind::Lava) => blocks::LAVA,
        };
        // `FlowingFluid.getLegacyLevel`.
        let level = if self.is_source() {
            0
        } else {
            8 - self.amount.min(8) + if self.falling { 8 } else { 0 }
        };
        block
            .with_property("level", &level.to_string())
            .expect("a liquid block has every level from 0 to 15")
    }
}

/// `FluidState.getHeight`: a whole block where the same fluid is above, its own height
/// otherwise.
pub fn height<V: BlockView + ?Sized>(view: &mut V, fluid: FluidState, pos: BlockPos) -> f32 {
    let above = FluidState::of(view.block(relative(pos, Face::Up)));
    if fluid.kind().is_some() && above.kind() == fluid.kind() {
        1.0
    } else {
        fluid.own_height()
    }
}

/// `FlowingFluid.getFlow`: the way `fluid`, held at `pos`, pushes what is in it, as a
/// vector of length 1 (or none): towards the neighbours that hold less of it or none,
/// each by how much less, and steeply down where it falls beside a wall. In the game's
/// widths: the differences are floats, summed as doubles.
pub fn flow<V: BlockView + ?Sized>(view: &mut V, fluid: FluidState, pos: BlockPos) -> [f64; 3] {
    let Some(kind) = fluid.kind() else {
        return [0.0; 3];
    };
    // `affectsFlow`: no fluid, or the same fluid.
    let affects = |other: FluidState| other.is_empty() || other.kind() == Some(kind);
    let (mut dx, mut dz) = (0.0_f64, 0.0_f64);
    for face in HORIZONTAL {
        let beside = relative(pos, face);
        let beside_state = view.block(beside);
        let beside_fluid = FluidState::of(beside_state);
        if !affects(beside_fluid) {
            continue;
        }
        let height = beside_fluid.own_height();
        let mut difference = 0.0_f32;
        if height == 0.0 {
            if !crate::data::BLOCKS_FLUID_FLOW.contains(beside_state) {
                let below = FluidState::of(view.block(relative(beside, Face::Down)));
                if affects(below) {
                    let below_height = below.own_height();
                    if below_height > 0.0 {
                        difference = fluid.own_height() - (below_height - 0.888_888_9);
                    }
                }
            }
        } else if height > 0.0 {
            difference = fluid.own_height() - height;
        }
        if difference != 0.0 {
            let (step_x, step_z) = horizontal_step(face);
            dx += f64::from(step_x as f32 * difference);
            dz += f64::from(step_z as f32 * difference);
        }
    }
    let mut flow = [dx, 0.0, dz];
    if fluid.falling {
        for face in HORIZONTAL {
            let beside = relative(pos, face);
            if is_solid_face(view, kind, beside, face)
                || is_solid_face(view, kind, relative(beside, Face::Up), face)
            {
                let [x, y, z] = normalize(flow);
                flow = [x, y + -6.0, z];
                break;
            }
        }
    }
    normalize(flow)
}

/// `Vec3.normalize`: the vector scaled to length 1, or none where it is shorter than
/// the game's 1e-5 (a float, widened).
pub fn normalize([x, y, z]: [f64; 3]) -> [f64; 3] {
    let length = (x * x + y * y + z * z).sqrt();
    if length < f64::from(1.0e-5_f32) {
        [0.0; 3]
    } else {
        [x / length, y / length, z / length]
    }
}

/// The x and z of a horizontal direction's step.
fn horizontal_step(face: Face) -> (i32, i32) {
    match face {
        Face::North => (0, -1),
        Face::South => (0, 1),
        Face::West => (-1, 0),
        Face::East => (1, 0),
        Face::Up | Face::Down => (0, 0),
    }
}

/// `FlowingFluid.isSolidFace`: whether the block at `pos` stops a fall of `kind` beside
/// it on its side `face`: not where it holds the same fluid, always upwards, never for
/// ice, and otherwise where that side is sturdy.
fn is_solid_face<V: BlockView + ?Sized>(
    view: &mut V,
    kind: Kind,
    pos: BlockPos,
    face: Face,
) -> bool {
    let state = view.block(pos);
    if FluidState::of(state).kind() == Some(kind) {
        return false;
    }
    if face == Face::Up {
        return true;
    }
    if is_class(state, BlockClass::IceBlock) {
        return false;
    }
    state
        .props()
        .is_face_sturdy(face, clustine_data::SupportType::Full)
}

/// `FluidState.canBeReplacedWith`: whether `fluid`, held at `pos`, gives way to
/// `other` coming from the direction `direction` (the way it flows). Nothing gives way
/// to anything; water to anything but water from above; lava to water where it is
/// high enough.
pub fn can_be_replaced_with<V: BlockView + ?Sized>(
    view: &mut V,
    fluid: FluidState,
    pos: BlockPos,
    other: u8,
    direction: Face,
) -> bool {
    match fluid.kind() {
        None => true,
        Some(Kind::Water) => direction == Face::Down && Kind::of(other) != Some(Kind::Water),
        Some(Kind::Lava) => {
            height(view, fluid, pos) >= 0.444_444_45 && Kind::of(other) == Some(Kind::Water)
        }
    }
}

/// Whether `state` takes a fluid in beside what it is (`LiquidBlockContainer`): every
/// block that can be waterlogged (`SimpleWaterloggedBlock`, which is exactly the blocks
/// with the property `waterlogged`), kelp and seagrass.
pub fn is_container(state: BlockState) -> bool {
    state.property("waterlogged").is_some()
        || is_block(state, blocks::KELP)
        || is_block(state, blocks::KELP_PLANT)
        || is_block(state, blocks::SEAGRASS)
        || is_block(state, blocks::TALL_SEAGRASS)
}

/// `LiquidBlockContainer.canPlaceLiquid` with no entity: still water into a block that
/// can be waterlogged, but for a double slab and a barrier (which takes water only from
/// a player in creative); nothing into kelp or seagrass.
pub fn can_place_liquid(state: BlockState, fluid: u8) -> bool {
    if state.property("waterlogged").is_none()
        || is_block(state, blocks::BARRIER)
        || (is_class(state, BlockClass::SlabBlock) && state.property("type") == Some("double"))
    {
        return false;
    }
    fluid == WATER
}

/// `FlowingFluid.canHoldAnyFluid`: a container, or what fluids wash away.
pub fn can_hold_any_fluid(state: BlockState) -> bool {
    is_container(state) || in_tag(state, "minecraft:washed_away_by_fluids")
}

/// `FlowingFluid.canHoldSpecificFluid`: a container that takes `fluid`, or anything
/// else.
pub fn can_hold_specific_fluid(state: BlockState, fluid: u8) -> bool {
    !is_container(state) || can_place_liquid(state, fluid)
}

/// `FlowingFluid.canHoldFluid`.
pub fn can_hold_fluid(state: BlockState, fluid: u8) -> bool {
    can_hold_any_fluid(state) && can_hold_specific_fluid(state, fluid)
}

/// `FlowingFluid.canPassThroughWall`: whether a fluid in `state` passes to its neighbour
/// `spread_state` the way `direction`, which it does unless their collision shapes
/// together close the face between them. The shapes do not depend on where the blocks
/// are, so neither does this.
pub fn can_pass_through_wall(direction: Face, state: BlockState, spread_state: BlockState) -> bool {
    let spread_shape = spread_state.props().collision_shape();
    let shape = state.props().collision_shape();
    if spread_shape == Shape::FULL_BLOCK || shape == Shape::FULL_BLOCK {
        return false;
    }
    if shape == Shape::EMPTY && spread_shape == Shape::EMPTY {
        return true;
    }
    !merged_face_occludes(shape, spread_shape, direction)
}

/// `Shapes.mergedFaceOccludes`: whether `first`'s side towards `direction` and
/// `second`'s side facing it close the face together. Each counts only where it
/// reaches the face: the part of `first` in its last cell along the axis, if that ends
/// at the face, and of `second` in its first, if that begins there.
fn merged_face_occludes(first: Shape, second: Shape, direction: Face) -> bool {
    const EPSILON: f64 = 1.0e-7;
    let (axis, positive) = match direction {
        Face::Down => (1, false),
        Face::Up => (1, true),
        Face::North => (2, false),
        Face::South => (2, true),
        Face::West => (0, false),
        Face::East => (0, true),
    };
    let (high, low) = if positive {
        (first, second)
    } else {
        (second, first)
    };
    let high: Vec<[f64; 6]> = high.boxes().collect();
    let low: Vec<[f64; 6]> = low.boxes().collect();
    let max = high
        .iter()
        .map(|b| b[axis + 3])
        .fold(f64::NEG_INFINITY, f64::max);
    let min = low.iter().map(|b| b[axis]).fold(f64::INFINITY, f64::min);
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let mut rectangles = Vec::new();
    if (max - 1.0).abs() <= EPSILON {
        rectangles.extend(
            high.iter()
                .filter(|b| (b[axis + 3] - max).abs() <= EPSILON)
                .map(|b| [b[u], b[v], b[u + 3], b[v + 3]]),
        );
    }
    if min.abs() <= EPSILON {
        rectangles.extend(
            low.iter()
                .filter(|b| (b[axis] - min).abs() <= EPSILON)
                .map(|b| [b[u], b[v], b[u + 3], b[v + 3]]),
        );
    }
    covers_the_face(&rectangles)
}

/// Whether `rectangles` together cover the unit square: the rectangles' edges divide it
/// into cells, each wholly in or out of every rectangle.
fn covers_the_face(rectangles: &[[f64; 4]]) -> bool {
    let mut us = vec![0.0, 1.0];
    let mut vs = vec![0.0, 1.0];
    for r in rectangles {
        us.extend([r[0], r[2]].into_iter().filter(|u| *u > 0.0 && *u < 1.0));
        vs.extend([r[1], r[3]].into_iter().filter(|v| *v > 0.0 && *v < 1.0));
    }
    us.sort_by(f64::total_cmp);
    vs.sort_by(f64::total_cmp);
    us.dedup();
    vs.dedup();
    us.windows(2).all(|u| {
        vs.windows(2).all(|v| {
            let (mu, mv) = ((u[0] + u[1]) / 2.0, (v[0] + v[1]) / 2.0);
            rectangles
                .iter()
                .any(|r| r[0] <= mu && mu <= r[2] && r[1] <= mv && mv <= r[3])
        })
    })
}

/// `isSourceBlockOfThisType`.
fn is_source_of(kind: Kind, fluid: FluidState) -> bool {
    fluid.kind() == Some(kind) && fluid.is_source()
}

/// `FlowingFluid.canMaybePassThrough`: whether a fluid of `kind` in `state` may flow
/// into `spread_state` the way `direction`, before what it would be there is known.
pub fn can_maybe_pass_through(
    kind: Kind,
    state: BlockState,
    direction: Face,
    spread_state: BlockState,
) -> bool {
    !is_source_of(kind, FluidState::of(spread_state))
        && can_hold_any_fluid(spread_state)
        && can_pass_through_wall(direction, state, spread_state)
}

/// `FlowingFluid.canPassThrough`, with the flowing fluid of `kind`.
fn can_pass_through(
    kind: Kind,
    state: BlockState,
    direction: Face,
    spread_state: BlockState,
) -> bool {
    can_maybe_pass_through(kind, state, direction, spread_state)
        && can_hold_specific_fluid(spread_state, kind.flowing())
}

/// `FlowingFluid.isWaterHole`: whether a fluid of `kind` in `state` at a position can
/// go down into `below`.
pub fn is_water_hole(kind: Kind, state: BlockState, below: BlockState) -> bool {
    if !can_pass_through_wall(Face::Down, state, below) {
        return false;
    }
    FluidState::of(below).kind() == Some(kind) || can_hold_fluid(below, kind.flowing())
}

/// Whether a fluid of `kind` at `pos` would go down from there (`SpreadContext.isHole`).
fn is_hole<V: BlockView + ?Sized>(view: &mut V, kind: Kind, pos: BlockPos) -> bool {
    let state = view.block(pos);
    let below = view.block(relative(pos, Face::Down));
    is_water_hole(kind, state, below)
}

/// `FlowingFluid.sourceNeighborCount`: the sources of `kind` beside `pos`.
pub fn source_neighbour_count<V: BlockView + ?Sized>(
    view: &mut V,
    kind: Kind,
    pos: BlockPos,
) -> usize {
    HORIZONTAL
        .iter()
        .filter(|face| is_source_of(kind, FluidState::of(view.block(relative(pos, **face)))))
        .count()
}

/// `FlowingFluid.getNewLiquid`: what a fluid of `kind` makes of the block `state` at
/// `pos` from its neighbours: a source between two sources (where it converts) on
/// something solid or a source; a falling fluid under the same fluid; otherwise the
/// most of its sides less the drop-off, or nothing.
pub fn new_liquid<V: BlockView + ?Sized>(
    view: &mut V,
    kind: Kind,
    pos: BlockPos,
    state: BlockState,
) -> FluidState {
    let mut highest = 0;
    let mut sources = 0;
    for face in HORIZONTAL {
        let beside = view.block(relative(pos, face));
        let fluid = FluidState::of(beside);
        if fluid.kind() == Some(kind) && can_pass_through_wall(face, state, beside) {
            if fluid.is_source() {
                sources += 1;
            }
            highest = highest.max(i32::from(fluid.amount));
        }
    }
    if sources >= 2 && kind.converts_to_source() {
        let below = view.block(relative(pos, Face::Down));
        if below.props().is_solid() || is_source_of(kind, FluidState::of(below)) {
            return FluidState::source(kind, false);
        }
    }
    let above = view.block(relative(pos, Face::Up));
    let fluid = FluidState::of(above);
    if fluid.kind() == Some(kind) && can_pass_through_wall(Face::Up, state, above) {
        return FluidState::flowing(kind, 8, true);
    }
    let amount = highest - kind.drop_off(view.fast_lava());
    if amount <= 0 {
        FluidState::EMPTY
    } else {
        FluidState::flowing(kind, amount as u8, false)
    }
}

/// `FlowingFluid.getSlopeDistance`: how many blocks from `pos` the nearest way down is
/// along what the fluid can flow through, not back the way `from` it came; 1000 for
/// none within the fluid's reach.
fn slope_distance<V: BlockView + ?Sized>(
    view: &mut V,
    kind: Kind,
    pos: BlockPos,
    depth: i32,
    from: Face,
    state: BlockState,
) -> i32 {
    let mut nearest = 1000;
    for face in HORIZONTAL {
        if face == from {
            continue;
        }
        let beside_pos = relative(pos, face);
        let beside = view.block(beside_pos);
        if !can_pass_through(kind, state, face, beside) {
            continue;
        }
        if is_hole(view, kind, beside_pos) {
            return depth;
        }
        if depth < kind.slope_find_distance(view.fast_lava()) {
            let distance =
                slope_distance(view, kind, beside_pos, depth + 1, opposite(face), beside);
            nearest = nearest.min(distance);
        }
    }
    nearest
}

/// `FlowingFluid.getSpread`: where a fluid of `kind` in `state` at `pos` flows sideways,
/// and what it makes there: towards the nearest way down, or everywhere it can where
/// there is none. In the order the game's map of directions gives them (north, south,
/// west, east), which is the order it spreads in.
pub fn spread<V: BlockView + ?Sized>(
    view: &mut V,
    kind: Kind,
    pos: BlockPos,
    state: BlockState,
) -> Vec<(Face, FluidState)> {
    let mut nearest = 1000;
    let mut found: Vec<(Face, FluidState)> = Vec::new();
    for face in HORIZONTAL {
        let beside_pos = relative(pos, face);
        let beside = view.block(beside_pos);
        if !can_maybe_pass_through(kind, state, face, beside) {
            continue;
        }
        let new = new_liquid(view, kind, beside_pos, beside);
        if !can_hold_specific_fluid(beside, new.fluid) {
            continue;
        }
        let distance = if is_hole(view, kind, beside_pos) {
            0
        } else {
            slope_distance(view, kind, beside_pos, 1, opposite(face), beside)
        };
        if distance < nearest {
            found.clear();
        }
        if distance <= nearest {
            let here = FluidState::of(beside);
            if can_be_replaced_with(view, here, beside_pos, new.fluid, face) {
                found.push((face, new));
            }
            nearest = distance;
        }
    }
    found.sort_by_key(|(face, _)| *face as u8);
    found
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

/// The concrete that the concrete powder `state` hardens into (`ConcretePowderBlock`'s
/// `concrete`), if `state` is concrete powder.
pub fn concrete_of(state: BlockState) -> Option<BlockState> {
    if !is_class(state, BlockClass::ConcretePowderBlock) {
        return None;
    }
    let name = state.block()?.name.strip_suffix("_powder")?;
    let id = clustine_data::block_id_by_name(name)?;
    Some(clustine_data::BLOCKS[id].default_state)
}

/// `ConcretePowderBlock.touchesLiquid`: whether water beside `pos` reaches it, through a
/// side of its block that is not sturdy.
pub fn touches_water<V: BlockView + ?Sized>(view: &mut V, pos: BlockPos) -> bool {
    Face::ALL.into_iter().any(|face| {
        let beside = view.block(relative(pos, face));
        crate::data::has_water(beside)
            && !beside
                .props()
                .is_face_sturdy(opposite(face), clustine_data::SupportType::Full)
    })
}

/// `ConcretePowderBlock.shouldSolidify`: concrete powder at `pos`, in place of
/// `replaced`, hardens where it replaces water or water touches it.
pub fn powder_hardens<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    replaced: BlockState,
) -> bool {
    crate::data::has_water(replaced) || touches_water(view, pos)
}
