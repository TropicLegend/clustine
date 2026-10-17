//! Rails that join their neighbours as they are placed, curving and sloping
//! (`BaseRailBlock.onPlace`, `updateState` and `updateDir`, and `RailState`), the rule
//! under which a rail goes when what holds it goes (`BaseRailBlock.neighborChanged`
//! and `shouldBeRemoved`), and what power does to rails (step R3b of
//! `docs/adr/0026-redstone.md`). Read from the 26.3 jar's classes.
//!
//! A rail placed turns the rails beside it towards itself, each set with every update
//! (`setBlockAndUpdate`) as the game sets it. Powered, detector and activator rails
//! never curve (`isStraight`). Power:
//!
//! - a rail chooses between the curves of a junction by whether it is powered
//!   (`hasNeighborSignal`), as it is placed and when it turns again;
//! - a plain rail at a junction of three turns again when a block that gives power
//!   beside it changes (`RailBlock.updateState`);
//! - a powered or activator rail is powered by power beside it, or along its line by
//!   a powered rail of its kind up to eight rails away that has power beside it
//!   (`PoweredRailBlock.updateState`, `findPoweredRailSignal`), and tells the blocks
//!   below it, and above it on a slope, when that changes;
//! - a rail taken away tells the blocks around it and the one above a slope
//!   (`BaseRailBlock.affectNeighborsAfterRemoval`).
//!
//! A detector rail tells comparators of its state (`DetectorRailBlock.checkPressed`),
//! which without minecarts never changes.
//!
//! A rail's rules read signals, so they run only where its chunk ticks (section 1 of
//! the record), where every block they read is loaded or copied.

use clustine_blocks::data::{in_tag, is_class, relative};
use clustine_data::{BlockClass, BlockState, Face, SupportType};
use clustine_world::BlockPos;

use super::Region;
use super::updates::RegionView;
use crate::api::TickOutput;
use clustine_blocks::signal;

/// The four sides, in the game's `Direction.Plane.HORIZONTAL` order.
const HORIZONTAL_SIDES: [Face; 4] = [Face::North, Face::East, Face::South, Face::West];

/// The region's blocks read without changing them, as the signal queries read them:
/// a block of a chunk neither loaded nor copied reads as air, which the rules that
/// read signals never meet, as they run only where their chunk ticks.
struct ReadOnly<'r>(&'r Region);

impl clustine_blocks::BlockView for ReadOnly<'_> {
    fn block(&mut self, at: BlockPos) -> BlockState {
        self.0.read(at)
    }
    fn raw_brightness(&mut self, _at: BlockPos) -> Option<u8> {
        None
    }
    fn schedule_block_tick(&mut self, _at: BlockPos, _block: u16, _delay: i32) {}
    fn schedule_fluid_tick(&mut self, _at: BlockPos, _fluid: u8, _delay: i32) {}
    fn rules(&self) -> clustine_blocks::Rules {
        self.0.rules()
    }
    fn next_int(&mut self, _bound: u32) -> u32 {
        0
    }
    fn comparator_output(&mut self, at: BlockPos) -> u8 {
        self.0.comparator_output(at)
    }
}

/// `RailShape`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    NorthSouth,
    EastWest,
    AscendingEast,
    AscendingWest,
    AscendingNorth,
    AscendingSouth,
    SouthEast,
    SouthWest,
    NorthWest,
    NorthEast,
}

impl Shape {
    const ALL: [Shape; 10] = [
        Shape::NorthSouth,
        Shape::EastWest,
        Shape::AscendingEast,
        Shape::AscendingWest,
        Shape::AscendingNorth,
        Shape::AscendingSouth,
        Shape::SouthEast,
        Shape::SouthWest,
        Shape::NorthWest,
        Shape::NorthEast,
    ];

    fn name(self) -> &'static str {
        match self {
            Shape::NorthSouth => "north_south",
            Shape::EastWest => "east_west",
            Shape::AscendingEast => "ascending_east",
            Shape::AscendingWest => "ascending_west",
            Shape::AscendingNorth => "ascending_north",
            Shape::AscendingSouth => "ascending_south",
            Shape::SouthEast => "south_east",
            Shape::SouthWest => "south_west",
            Shape::NorthWest => "north_west",
            Shape::NorthEast => "north_east",
        }
    }

    /// The shape of a rail's state.
    fn of(state: BlockState) -> Option<Shape> {
        let name = state.property("shape")?;
        Shape::ALL.into_iter().find(|shape| shape.name() == name)
    }

    /// The side a sloped rail rises towards, where the block that holds its upper end
    /// is.
    fn rises_towards(self) -> Option<Face> {
        match self {
            Shape::AscendingEast => Some(Face::East),
            Shape::AscendingWest => Some(Face::West),
            Shape::AscendingNorth => Some(Face::North),
            Shape::AscendingSouth => Some(Face::South),
            _ => None,
        }
    }
}

/// `BaseRailBlock.isRail`: a block of the rails tag that is a rail.
fn is_rail(state: BlockState) -> bool {
    in_tag(state, "minecraft:rails") && is_class(state, BlockClass::BaseRailBlock)
}

/// `BaseRailBlock.isStraight`: powered, detector and activator rails, which never
/// curve.
fn is_straight(state: BlockState) -> bool {
    is_class(state, BlockClass::PoweredRailBlock) || is_class(state, BlockClass::DetectorRailBlock)
}

/// `RailState`: a rail and the positions it leads to, by its shape.
#[derive(Debug, Clone)]
struct Rail {
    pos: BlockPos,
    state: BlockState,
    straight: bool,
    connections: Vec<BlockPos>,
}

impl Rail {
    fn new(pos: BlockPos, state: BlockState) -> Self {
        let mut rail = Self {
            pos,
            state,
            straight: is_straight(state),
            connections: Vec::new(),
        };
        if let Some(shape) = Shape::of(state) {
            rail.update_connections(shape);
        }
        rail
    }

    /// `RailState.updateConnections`.
    fn update_connections(&mut self, shape: Shape) {
        let pos = self.pos;
        let side = |face| relative(pos, face);
        let above = |face| relative(relative(pos, face), Face::Up);
        self.connections = match shape {
            Shape::NorthSouth => vec![side(Face::North), side(Face::South)],
            Shape::EastWest => vec![side(Face::West), side(Face::East)],
            Shape::AscendingEast => vec![side(Face::West), above(Face::East)],
            Shape::AscendingWest => vec![above(Face::West), side(Face::East)],
            Shape::AscendingNorth => vec![above(Face::North), side(Face::South)],
            Shape::AscendingSouth => vec![side(Face::North), above(Face::South)],
            Shape::SouthEast => vec![side(Face::East), side(Face::South)],
            Shape::SouthWest => vec![side(Face::West), side(Face::South)],
            Shape::NorthWest => vec![side(Face::West), side(Face::North)],
            Shape::NorthEast => vec![side(Face::East), side(Face::North)],
        };
    }

    /// `RailState.hasConnection`: whether this rail leads to the column of `pos`, at
    /// any height.
    fn has_connection(&self, pos: BlockPos) -> bool {
        self.connections
            .iter()
            .any(|connection| connection.x == pos.x && connection.z == pos.z)
    }

    /// `RailState.connectsTo`.
    fn connects_to(&self, other: &Rail) -> bool {
        self.has_connection(other.pos)
    }

    /// `RailState.canConnectTo`: a rail leads to `other` already, or has a free end.
    fn can_connect_to(&self, other: &Rail) -> bool {
        self.connects_to(other) || self.connections.len() != 2
    }
}

impl Region {
    /// `BaseRailBlock.onPlace`: a rail placed where there was no rail of its block
    /// takes the shape that joins it to its neighbours and turns them towards it
    /// (`updateState`, `updateDir` with `alwaysPlace`). A powered, detector or
    /// activator rail then looks whether it can stay as it is now sloped
    /// (`Level.neighborChanged` of itself).
    pub(super) fn rail_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_class(state, BlockClass::BaseRailBlock)
            || clustine_blocks::data::is_block(old, state)
        {
            return;
        }
        let Some(shape) = Shape::of(state) else {
            return;
        };
        let rail = Rail::new(pos, state);
        // Power is read only where the chunk ticks, where all it reaches is there (the
        // record, section 1); a player places a rail nowhere else, and a rail set
        // there otherwise chooses its curve as if unpowered.
        let powered = self.ticks_chunk(pos.chunk())
            && signal::has_neighbour_signal(&mut RegionView(self), pos);
        let placed = self.rail_place(rail, powered, true, shape, output);
        // `Level.neighborChanged` of the rail placed, through the updater.
        if placed.straight {
            self.neighbour_changed_at(pos, Face::Down, placed.state, false, output);
        }
    }

    /// `BaseRailBlock.neighborChanged`: a rail goes where nothing rigid holds it, or
    /// where the block a slope rises onto went (`shouldBeRemoved`), as `removeBlock`
    /// takes it, and otherwise follows power (`updateState`), told that `block`
    /// changed beside it.
    pub(super) fn rail_neighbour_changed(
        &mut self,
        pos: BlockPos,
        block: BlockState,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.block(pos) else {
            return;
        };
        if !is_class(state, BlockClass::BaseRailBlock) {
            return;
        }
        if self.rail_should_be_removed(pos, state) {
            self.set_block(pos, super::updates::fluid_block(state), output);
            return;
        }
        if is_class(state, BlockClass::PoweredRailBlock) {
            self.powered_rail_update(pos, state, output);
        } else if is_class(state, BlockClass::RailBlock) {
            self.plain_rail_update(pos, state, block, output);
        }
    }

    /// `RailBlock.updateState`: a plain rail at a junction of three chooses its curve
    /// again when a block that gives power changed beside it.
    fn plain_rail_update(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        block: BlockState,
        output: &mut TickOutput,
    ) {
        let source = block
            .block()
            .is_some_and(|of| signal::is_signal_source(of.default_state, signal::Query::ALL));
        if !source {
            return;
        }
        let Some(shape) = Shape::of(state) else {
            return;
        };
        // `RailState.countPotentialConnections`: the sides with a rail at, above or
        // below them.
        let joins = HORIZONTAL_SIDES
            .into_iter()
            .filter(|side| self.rail_near(relative(pos, *side)).is_some())
            .count();
        if joins == 3 {
            let rail = Rail::new(pos, state);
            let powered = signal::has_neighbour_signal(&mut RegionView(self), pos);
            self.rail_place(rail, powered, false, shape, output);
        }
    }

    /// `PoweredRailBlock.updateState`: powered by power beside it or along its line,
    /// set with every update, and then the blocks below it told, and above it where it
    /// slopes.
    fn powered_rail_update(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let was = clustine_blocks::data::flag(state, "powered");
        let powered = signal::has_neighbour_signal(&mut RegionView(self), pos)
            || self.powered_rail_signal(pos, state, true, 0)
            || self.powered_rail_signal(pos, state, false, 0);
        if powered == was {
            return;
        }
        let changed = clustine_blocks::data::set_flag(state, "powered", powered);
        self.set_block(pos, changed, output);
        self.update_neighbours_at(relative(pos, Face::Down), changed, output);
        if Shape::of(state).and_then(Shape::rises_towards).is_some() {
            self.update_neighbours_at(relative(pos, Face::Up), changed, output);
        }
    }

    /// `PoweredRailBlock.findPoweredRailSignal`: whether the next rail along the line
    /// of `state` at `pos`, forwards or back, at its height or one below (or one above
    /// up a slope), is a powered rail of its kind that has power beside it or, within
    /// eight rails, from further along.
    fn powered_rail_signal(
        &self,
        pos: BlockPos,
        state: BlockState,
        forward: bool,
        depth: u32,
    ) -> bool {
        if depth >= 8 {
            return false;
        }
        let Some(shape) = Shape::of(state) else {
            return false;
        };
        let (mut x, mut y, mut z) = (pos.x, pos.y, pos.z);
        let mut below = true;
        let line = match shape {
            Shape::NorthSouth => {
                z += if forward { 1 } else { -1 };
                Shape::NorthSouth
            }
            Shape::EastWest => {
                x += if forward { -1 } else { 1 };
                Shape::EastWest
            }
            Shape::AscendingEast => {
                if forward {
                    x -= 1;
                } else {
                    x += 1;
                    y += 1;
                    below = false;
                }
                Shape::EastWest
            }
            Shape::AscendingWest => {
                if forward {
                    x -= 1;
                    y += 1;
                    below = false;
                } else {
                    x += 1;
                }
                Shape::EastWest
            }
            Shape::AscendingNorth => {
                if forward {
                    z += 1;
                } else {
                    z -= 1;
                    y += 1;
                    below = false;
                }
                Shape::NorthSouth
            }
            Shape::AscendingSouth => {
                if forward {
                    z += 1;
                    y += 1;
                    below = false;
                } else {
                    z -= 1;
                }
                Shape::NorthSouth
            }
            // A powered rail never curves.
            curve => curve,
        };
        let next = BlockPos::new(x, y, z);
        self.same_rail_with_power(next, state, forward, depth, line)
            || (below
                && self.same_rail_with_power(
                    BlockPos::new(x, y - 1, z),
                    state,
                    forward,
                    depth,
                    line,
                ))
    }

    /// `PoweredRailBlock.isSameRailWithPower`: a rail of the block of `rail` at `pos`,
    /// on the line `line`, powered, with power beside it or from further along.
    fn same_rail_with_power(
        &self,
        pos: BlockPos,
        rail: BlockState,
        forward: bool,
        depth: u32,
        line: Shape,
    ) -> bool {
        let state = self.read(pos);
        if !clustine_blocks::data::same_block(state, rail) {
            return false;
        }
        let Some(other) = Shape::of(state) else {
            return false;
        };
        let across = match line {
            Shape::EastWest => matches!(
                other,
                Shape::NorthSouth | Shape::AscendingNorth | Shape::AscendingSouth
            ),
            Shape::NorthSouth => matches!(
                other,
                Shape::EastWest | Shape::AscendingEast | Shape::AscendingWest
            ),
            _ => false,
        };
        if across || !clustine_blocks::data::flag(state, "powered") {
            return false;
        }
        signal::has_neighbour_signal(&mut ReadOnly(self), pos)
            || self.powered_rail_signal(pos, state, forward, depth + 1)
    }

    /// `BaseRailBlock.affectNeighborsAfterRemoval` of `old`, a rail that has just gone
    /// at `pos`: the blocks around the one above a slope are told, and for a straight
    /// rail those around it and around the block below it.
    pub(super) fn rail_after_removal(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_class(old, BlockClass::BaseRailBlock) {
            return;
        }
        if Shape::of(old).and_then(Shape::rises_towards).is_some() {
            self.update_neighbours_at(relative(pos, Face::Up), old, output);
        }
        if is_straight(old) {
            self.update_neighbours_at(pos, old, output);
            self.update_neighbours_at(relative(pos, Face::Down), old, output);
        }
    }

    /// `BaseRailBlock.shouldBeRemoved`.
    fn rail_should_be_removed(&self, pos: BlockPos, state: BlockState) -> bool {
        if !self.supports_rigid(relative(pos, Face::Down)) {
            return true;
        }
        match Shape::of(state).and_then(Shape::rises_towards) {
            Some(towards) => !self.supports_rigid(relative(pos, towards)),
            None => false,
        }
    }

    /// `Block.canSupportRigidBlock`: a top face sturdy enough for a rail. A block not
    /// loaded holds nothing, as air.
    fn supports_rigid(&self, pos: BlockPos) -> bool {
        self.read(pos)
            .props()
            .is_face_sturdy(Face::Up, SupportType::Rigid)
    }

    /// `BaseRailBlock.isRail(level, pos)`.
    fn rail_at(&self, pos: BlockPos) -> bool {
        is_rail(self.read(pos))
    }

    /// `RailState.getRail`: the rail at `pos`, or else just above it, or else just
    /// below it.
    fn rail_near(&self, pos: BlockPos) -> Option<Rail> {
        [pos, relative(pos, Face::Up), relative(pos, Face::Down)]
            .into_iter()
            .find_map(|at| {
                let state = self.read(at);
                is_rail(state).then(|| Rail::new(at, state))
            })
    }

    /// `RailState.removeSoftConnections`: a rail's ends that lead to no rail leading
    /// back are dropped, and those that do lead to where that rail is, at its height.
    fn remove_soft_connections(&self, rail: &mut Rail) {
        let mut index = 0;
        while index < rail.connections.len() {
            match self.rail_near(rail.connections[index]) {
                Some(other) if other.connects_to(rail) => {
                    rail.connections[index] = other.pos;
                    index += 1;
                }
                _ => {
                    rail.connections.remove(index);
                }
            }
        }
    }

    /// `RailState.hasNeighborRail`: whether there is a rail near `pos` that could be
    /// joined to `rail`.
    fn has_neighbour_rail(&self, rail: &Rail, pos: BlockPos) -> bool {
        let Some(mut other) = self.rail_near(pos) else {
            return false;
        };
        self.remove_soft_connections(&mut other);
        other.can_connect_to(rail)
    }

    /// `RailState.connectTo`: `rail` takes `other` as one of its ends, and the shape
    /// its ends now give it, sloping up to a rail on a block beside it.
    fn rail_connect_to(&mut self, mut rail: Rail, other: &Rail, output: &mut TickOutput) {
        rail.connections.push(other.pos);
        let pos = rail.pos;
        let north = rail.has_connection(relative(pos, Face::North));
        let south = rail.has_connection(relative(pos, Face::South));
        let west = rail.has_connection(relative(pos, Face::West));
        let east = rail.has_connection(relative(pos, Face::East));
        let mut shape = None;
        if north || south {
            shape = Some(Shape::NorthSouth);
        }
        if west || east {
            shape = Some(Shape::EastWest);
        }
        if !rail.straight {
            if south && east && !north && !west {
                shape = Some(Shape::SouthEast);
            }
            if south && west && !north && !east {
                shape = Some(Shape::SouthWest);
            }
            if north && west && !south && !east {
                shape = Some(Shape::NorthWest);
            }
            if north && east && !south && !west {
                shape = Some(Shape::NorthEast);
            }
        }
        let shape = self.sloped(pos, shape).unwrap_or(Shape::NorthSouth);
        let state = clustine_blocks::data::set(rail.state, "shape", shape.name());
        self.set_block(pos, state, output);
    }

    /// A straight shape turned up towards a rail one block higher on its line, the
    /// north before the south and the east before the west, the later winning.
    fn sloped(&self, pos: BlockPos, shape: Option<Shape>) -> Option<Shape> {
        let rail_above = |face| self.rail_at(relative(relative(pos, face), Face::Up));
        let mut shape = shape;
        if shape == Some(Shape::NorthSouth) {
            if rail_above(Face::North) {
                shape = Some(Shape::AscendingNorth);
            }
            if rail_above(Face::South) {
                shape = Some(Shape::AscendingSouth);
            }
        }
        if shape == Some(Shape::EastWest) {
            if rail_above(Face::East) {
                shape = Some(Shape::AscendingEast);
            }
            if rail_above(Face::West) {
                shape = Some(Shape::AscendingWest);
            }
        }
        shape
    }

    /// `RailState.place`: the shape that joins `rail` to the rails beside it that can
    /// take it, set where `always` or where it changes, and then each rail it leads to
    /// turned towards it where that rail has an end free. `powered` chooses between
    /// the curves of a junction.
    fn rail_place(
        &mut self,
        mut rail: Rail,
        powered: bool,
        always: bool,
        current: Shape,
        output: &mut TickOutput,
    ) -> Rail {
        let pos = rail.pos;
        let north = self.has_neighbour_rail(&rail, relative(pos, Face::North));
        let south = self.has_neighbour_rail(&rail, relative(pos, Face::South));
        let west = self.has_neighbour_rail(&rail, relative(pos, Face::West));
        let east = self.has_neighbour_rail(&rail, relative(pos, Face::East));
        let mut shape = None;
        let along_z = north || south;
        let along_x = west || east;
        if along_z && !along_x {
            shape = Some(Shape::NorthSouth);
        }
        if along_x && !along_z {
            shape = Some(Shape::EastWest);
        }
        let south_east = south && east;
        let south_west = south && west;
        let north_east = north && east;
        let north_west = north && west;
        if !rail.straight {
            if south_east && !north && !west {
                shape = Some(Shape::SouthEast);
            }
            if south_west && !north && !east {
                shape = Some(Shape::SouthWest);
            }
            if north_west && !south && !east {
                shape = Some(Shape::NorthWest);
            }
            if north_east && !south && !west {
                shape = Some(Shape::NorthEast);
            }
        }
        if shape.is_none() {
            if along_z && along_x {
                shape = Some(current);
            } else if along_z {
                shape = Some(Shape::NorthSouth);
            } else if along_x {
                shape = Some(Shape::EastWest);
            }
            if !rail.straight {
                // At a junction the curve is chosen in an order that power reverses.
                let curves = [
                    (south_east, Shape::SouthEast),
                    (south_west, Shape::SouthWest),
                    (north_east, Shape::NorthEast),
                    (north_west, Shape::NorthWest),
                ];
                let mut order = curves.to_vec();
                if !powered {
                    order = vec![curves[3], curves[2], curves[1], curves[0]];
                }
                for (joins, curve) in order {
                    if joins {
                        shape = Some(curve);
                    }
                }
            }
        }
        let shape = self.sloped(pos, shape).unwrap_or(current);
        rail.update_connections(shape);
        rail.state = clustine_blocks::data::set(rail.state, "shape", shape.name());
        if always || self.block(pos) != Some(rail.state) {
            self.set_block(pos, rail.state, output);
            for index in 0..rail.connections.len() {
                let Some(mut other) = self.rail_near(rail.connections[index]) else {
                    continue;
                };
                self.remove_soft_connections(&mut other);
                if other.can_connect_to(&rail) {
                    self.rail_connect_to(other, &rail, output);
                }
            }
        }
        rail
    }
}
