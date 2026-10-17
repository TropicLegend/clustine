// The rules follow the 26.3 jar's `RedstoneTorchBlock`, `RedstoneWallTorchBlock`,
// `DiodeBlock`, `RepeaterBlock`, `ComparatorBlock` and `ObserverBlock`, read in its
// classes; SteelMC-26.3's `behavior/blocks/redstone/` was read beside them.

//! What torches, diodes and observers read of the blocks around them, as pure
//! functions of a [`BlockView`] (`docs/adr/0026-redstone.md`, step R4): whether a
//! torch's block is powered, what a diode reads behind it and at its sides, whether a
//! repeater is locked, what a comparator puts out, and with which priority a diode's
//! tick is scheduled. The region runs the rules that change blocks
//! (`clustine-sim`'s `region/diodes.rs`).

use clustine_data::{BlockClass, BlockState, Face};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::connect::is_conductor;
use crate::data::{clockwise, counter_clockwise, facing, flag, is_class, number, relative};
use crate::signal::{self, MAX_SIGNAL, Query};

/// `TickPriority.EXTREMELY_HIGH`, `VERY_HIGH`, `HIGH` and `NORMAL`, as a scheduled
/// tick keeps them.
pub const EXTREMELY_HIGH: i8 = -3;
pub const VERY_HIGH: i8 = -2;
pub const HIGH: i8 = -1;
pub const NORMAL: i8 = 0;

/// Whether `state` is a redstone torch, standing or on a wall.
pub fn is_torch(state: BlockState) -> bool {
    is_class(state, BlockClass::RedstoneTorchBlock)
}

/// `RedstoneTorchBlock.hasNeighborSignal` and the wall torch's: whether the block a
/// torch stands on, or hangs from, gives it a signal.
pub fn torch_powered<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> bool {
    let towards = if is_class(state, BlockClass::RedstoneWallTorchBlock) {
        facing(state).map_or(Face::Down, Face::opposite)
    } else {
        Face::Down
    };
    signal::has_signal(view, relative(pos, towards), towards)
}

/// Whether `state` is a repeater.
pub fn is_repeater(state: BlockState) -> bool {
    is_class(state, BlockClass::RepeaterBlock)
}

/// Whether `state` is a comparator.
pub fn is_comparator(state: BlockState) -> bool {
    is_class(state, BlockClass::ComparatorBlock)
}

/// `DiodeBlock.getDelay`: a repeater's two ticks for each step of its delay, a
/// comparator's two.
pub fn delay(state: BlockState) -> i32 {
    if is_repeater(state) {
        i32::from(number(state, "delay").unwrap_or(1)) * 2
    } else {
        2
    }
}

/// The direction from a diode to the block behind it, its input; its facing.
fn behind(state: BlockState) -> Face {
    facing(state).unwrap_or(Face::North)
}

/// `DiodeBlock.getInputSignal`: the signal of the block behind, and wire's power there
/// where that is more.
fn diode_input<V: BlockView + ?Sized>(view: &mut V, state: BlockState, pos: BlockPos) -> u8 {
    let direction = behind(state);
    let at = relative(pos, direction);
    let signal = signal::signal(view, at, direction, Query::ALL);
    if signal >= MAX_SIGNAL {
        return signal;
    }
    let block = view.block(at);
    signal.max(signal::wire_power(block))
}

/// `getInputSignal` of a diode: a repeater's as [`diode_input`]; a comparator's the
/// analog output of the block behind it in place of that, or else, behind a
/// conductor that is not powered whole, the analog output of the block beyond it
/// (`ComparatorBlock.getInputSignal`). An item frame beyond the conductor waits for
/// entities.
pub fn input_signal<V: BlockView + ?Sized>(view: &mut V, state: BlockState, pos: BlockPos) -> u8 {
    let input = diode_input(view, state, pos);
    if !is_comparator(state) {
        return input;
    }
    let direction = behind(state);
    let at = relative(pos, direction);
    let block = view.block(at);
    if block.props().has_analog_output_signal() {
        return signal::analog_output(block).unwrap_or(0);
    }
    if input < MAX_SIGNAL && is_conductor(block) {
        let beyond = view.block(relative(at, direction));
        if beyond.props().has_analog_output_signal() {
            return signal::analog_output(beyond).unwrap_or(0);
        }
    }
    input
}

/// `DiodeBlock.getAlternateSignal`: the stronger of what the two sides give, clockwise
/// first; only another diode's for a repeater (`sideInputDiodesOnly`).
pub fn alternate_signal<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> u8 {
    let direction = behind(state);
    let only_diodes = is_repeater(state);
    let mut best = 0;
    for side in [clockwise(direction), counter_clockwise(direction)] {
        let signal = signal::control_input_signal(view, relative(pos, side), side, only_diodes);
        best = best.max(signal);
    }
    best
}

/// `RepeaterBlock.isLocked`: a powered diode at either side; never for a comparator
/// (`DiodeBlock.isLocked`).
pub fn locked<V: BlockView + ?Sized>(view: &mut V, state: BlockState, pos: BlockPos) -> bool {
    is_repeater(state) && alternate_signal(view, state, pos) > 0
}

/// `shouldTurnOn`: a repeater whose input has a signal; a comparator whose input is
/// stronger than its sides, or as strong in compare mode
/// (`ComparatorBlock.shouldTurnOn`).
pub fn should_turn_on<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> bool {
    let input = input_signal(view, state, pos);
    if !is_comparator(state) {
        return input > 0;
    }
    if input == 0 {
        return false;
    }
    let side = alternate_signal(view, state, pos);
    input > side || (input == side && !subtracts(state))
}

/// Whether a comparator is in subtract mode.
pub fn subtracts(state: BlockState) -> bool {
    state.property("mode") == Some("subtract")
}

/// `ComparatorBlock.calculateOutputSignal`: nothing without an input or with a
/// stronger side; the input less the side in subtract mode, the input in compare mode.
pub fn comparator_output<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> u8 {
    let input = input_signal(view, state, pos);
    if input == 0 {
        return 0;
    }
    let side = alternate_signal(view, state, pos);
    if side > input {
        return 0;
    }
    if subtracts(state) {
        input - side
    } else {
        input
    }
}

/// `DiodeBlock.shouldPrioritize`: the block in front is a diode that does not face
/// back into this one.
pub fn should_prioritize<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> bool {
    let front = behind(state).opposite();
    let ahead = view.block(relative(pos, front));
    is_class(ahead, BlockClass::DiodeBlock) && facing(ahead) != Some(front)
}

/// The priority of the tick a diode told of a neighbour schedules
/// (`DiodeBlock.checkTickOnNeighbor`, `ComparatorBlock.checkTickOnNeighbor`): a
/// repeater's −3 when it should be prioritised, −2 while powered and −1 otherwise; a
/// comparator's −1 when it should be prioritised and 0 otherwise.
pub fn neighbour_tick_priority<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> i8 {
    let prioritised = should_prioritize(view, state, pos);
    if is_comparator(state) {
        return if prioritised { HIGH } else { NORMAL };
    }
    if prioritised {
        EXTREMELY_HIGH
    } else if flag(state, "powered") {
        VERY_HIGH
    } else {
        HIGH
    }
}

/// The direction from a diode or an observer towards the block it powers, in front of
/// a diode and behind an observer: the opposite of its facing.
pub fn front(state: BlockState) -> Face {
    behind(state).opposite()
}

/// `ObserverBlock.updateShape`: an observer whose watched side, the way it faces,
/// changed starts its signal, unless it is powered or a tick of it is pending
/// (`startSignal`). Returns whether `state` is an observer.
pub fn observer_shape<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> bool {
    if !is_class(state, BlockClass::ObserverBlock) {
        return false;
    }
    if facing(state) == Some(direction) && !flag(state, "powered") {
        let block = crate::data::tick_id(state);
        if !view.has_scheduled_block_tick(pos, block) {
            view.schedule_block_tick(pos, block, 2);
        }
    }
    true
}

/// `RepeaterBlock.updateShape` and `ComparatorBlock.updateShape`: a diode whose block
/// below can no longer hold it goes; a repeater asked from a side, or from above or
/// below, works out whether it is locked. `None` where `state` is not a diode.
pub fn diode_shape<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
) -> Option<BlockState> {
    if !is_class(state, BlockClass::DiodeBlock) {
        return None;
    }
    if direction == Face::Down && !crate::survive::can_survive(state, pos, view) {
        return Some(clustine_data::blocks::AIR);
    }
    if is_repeater(state) && crate::data::axis(direction) != crate::data::axis(behind(state)) {
        let locked = locked(view, state, pos);
        return Some(crate::data::set_flag(state, "locked", locked));
    }
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::blocks;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Blocks(
        BTreeMap<(i32, i32, i32), BlockState>,
        BTreeMap<(i32, i32, i32), u8>,
    );

    impl Blocks {
        fn with(mut self, pos: BlockPos, state: BlockState) -> Self {
            self.0.insert((pos.x, pos.y, pos.z), state);
            self
        }
    }

    impl BlockView for Blocks {
        fn block(&mut self, at: BlockPos) -> BlockState {
            self.0
                .get(&(at.x, at.y, at.z))
                .copied()
                .unwrap_or(blocks::AIR)
        }
        fn raw_brightness(&mut self, _at: BlockPos) -> Option<u8> {
            None
        }
        fn schedule_block_tick(&mut self, _at: BlockPos, _block: u16, _delay: i32) {}
        fn schedule_fluid_tick(&mut self, _at: BlockPos, _fluid: u8, _delay: i32) {}
        fn rules(&self) -> crate::Rules {
            crate::Rules::Placing
        }
        fn next_int(&mut self, _bound: u32) -> u32 {
            0
        }
        fn comparator_output(&mut self, at: BlockPos) -> u8 {
            self.1.get(&(at.x, at.y, at.z)).copied().unwrap_or(0)
        }
    }

    fn state(text: &str) -> BlockState {
        BlockState::parse(text).expect("a state")
    }

    const AT: BlockPos = BlockPos::new(0, 64, 0);

    #[test]
    fn a_lit_torch_signals_every_side_but_its_support_and_powers_only_the_block_above() {
        let torch = state("minecraft:redstone_torch[lit=true]");
        let mut view = Blocks::default().with(AT, torch);
        for direction in Face::ALL {
            let expected = if direction == Face::Up { 0 } else { 15 };
            assert_eq!(
                signal::block_signal(&mut view, torch, AT, direction, Query::ALL),
                expected,
                "{direction:?}"
            );
            let direct = if direction == Face::Down { 15 } else { 0 };
            assert_eq!(
                signal::block_direct_signal(&mut view, torch, AT, direction, Query::ALL),
                direct
            );
        }
        // Hung on the block to its west, a wall torch facing east signals all but it.
        let wall = state("minecraft:redstone_wall_torch[facing=east,lit=true]");
        assert_eq!(
            signal::block_signal(&mut view, wall, AT, Face::East, Query::ALL),
            0
        );
        assert_eq!(
            signal::block_signal(&mut view, wall, AT, Face::West, Query::ALL),
            15
        );
        assert_eq!(
            signal::block_signal(&mut view, wall, AT, Face::Up, Query::ALL),
            15
        );
    }

    #[test]
    fn a_wall_torch_is_put_out_by_its_wall_and_a_torch_by_the_block_below() {
        let wall = state("minecraft:redstone_wall_torch[facing=east,lit=true]");
        let west = relative(AT, Face::West);
        let lever = state("minecraft:lever[face=wall,facing=west,powered=true]");
        let mut view = Blocks::default()
            .with(AT, wall)
            .with(west, blocks::STONE)
            .with(relative(west, Face::West), lever);
        assert!(torch_powered(&mut view, wall, AT));
        let torch = state("minecraft:redstone_torch[lit=true]");
        assert!(!torch_powered(&mut view, torch, AT));
    }

    #[test]
    fn a_comparator_compares_and_subtracts_its_side_from_a_cake_behind_it() {
        // Facing north: the input is north, the sides east and west.
        let compare = state("minecraft:comparator[facing=north,mode=compare]");
        let subtract = state("minecraft:comparator[facing=north,mode=subtract]");
        let east = relative(AT, Face::East);
        let mut view = Blocks::default()
            .with(relative(AT, Face::North), state("minecraft:cake[bites=1]"))
            .with(
                east,
                state("minecraft:redstone_wire[power=5,east=side,west=side]"),
            );
        assert_eq!(input_signal(&mut view, compare, AT), 12);
        assert_eq!(alternate_signal(&mut view, compare, AT), 5);
        assert_eq!(comparator_output(&mut view, compare, AT), 12);
        assert_eq!(comparator_output(&mut view, subtract, AT), 7);
        assert!(should_turn_on(&mut view, subtract, AT));
        // A cake behind a block is read through it.
        let mut through = Blocks::default()
            .with(relative(AT, Face::North), blocks::STONE)
            .with(BlockPos::new(0, 64, -2), state("minecraft:cake[bites=6]"));
        assert_eq!(input_signal(&mut through, compare, AT), 2);
    }

    #[test]
    fn a_repeater_is_locked_only_by_a_powered_diode_at_its_side() {
        let repeater = state("minecraft:repeater[facing=north]");
        let east = relative(AT, Face::East);
        // A repeater east of it facing east has its output west, into this one.
        let side = state("minecraft:repeater[facing=east,powered=true]");
        let mut view = Blocks::default().with(east, side);
        assert!(locked(&mut view, repeater, AT));
        let mut redstone = Blocks::default().with(east, blocks::REDSTONE_BLOCK);
        assert!(!locked(&mut redstone, repeater, AT));
        let comparator = state("minecraft:comparator[facing=north]");
        assert!(!locked(&mut view, comparator, AT));
    }

    #[test]
    fn a_diode_facing_a_diode_that_does_not_face_it_back_is_prioritised() {
        // Facing west: the input is west, the front east.
        let repeater = state("minecraft:repeater[facing=west]");
        let east = relative(AT, Face::East);
        // Across it, and in line, taking its input from this one: prioritised.
        for front in ["north", "west"] {
            let ahead = state(&format!("minecraft:repeater[facing={front}]"));
            let mut view = Blocks::default().with(east, ahead);
            assert_eq!(
                neighbour_tick_priority(&mut view, repeater, AT),
                EXTREMELY_HIGH,
                "{front}"
            );
            let comparator = state("minecraft:comparator[facing=west]");
            assert_eq!(neighbour_tick_priority(&mut view, comparator, AT), HIGH);
        }
        // One facing east puts its output into this one: they face each other.
        let mut back = Blocks::default().with(east, state("minecraft:repeater[facing=east]"));
        assert_eq!(neighbour_tick_priority(&mut back, repeater, AT), HIGH);
        let powered = state("minecraft:repeater[facing=west,powered=true]");
        assert_eq!(neighbour_tick_priority(&mut back, powered, AT), VERY_HIGH);
        let comparator = state("minecraft:comparator[facing=west]");
        assert_eq!(neighbour_tick_priority(&mut back, comparator, AT), NORMAL);
        let mut nothing = Blocks::default();
        assert_eq!(neighbour_tick_priority(&mut nothing, repeater, AT), HIGH);
    }

    #[test]
    fn a_powered_comparator_gives_the_output_its_block_entity_keeps() {
        let comparator = state("minecraft:comparator[facing=north,powered=true]");
        let mut view = Blocks::default().with(AT, comparator);
        view.1.insert((AT.x, AT.y, AT.z), 9);
        // Asked from the front, towards the north it faces.
        assert_eq!(signal::signal(&mut view, AT, Face::North, Query::ALL), 9);
        assert_eq!(
            signal::direct_signal(&mut view, AT, Face::North, Query::ALL),
            9
        );
        assert_eq!(signal::signal(&mut view, AT, Face::South, Query::ALL), 0);
    }

    #[test]
    fn the_analog_outputs_of_blocks_without_an_inventory_are_the_games() {
        for (text, output) in [
            ("minecraft:cake[bites=0]", 14),
            ("minecraft:cake[bites=6]", 2),
            ("minecraft:candle_cake[lit=false]", 14),
            ("minecraft:composter[level=8]", 8),
            ("minecraft:water_cauldron[level=2]", 2),
            ("minecraft:powder_snow_cauldron[level=3]", 3),
            ("minecraft:lava_cauldron", 3),
            ("minecraft:end_portal_frame[eye=true]", 15),
            ("minecraft:end_portal_frame[eye=false]", 0),
            ("minecraft:respawn_anchor[charges=1]", 3),
            ("minecraft:respawn_anchor[charges=2]", 7),
            ("minecraft:respawn_anchor[charges=4]", 15),
            ("minecraft:beehive[honey_level=5]", 5),
            ("minecraft:bee_nest[honey_level=3]", 3),
            (
                "minecraft:copper_golem_statue[copper_golem_pose=standing]",
                1,
            ),
            (
                "minecraft:weathered_copper_golem_statue[copper_golem_pose=star]",
                4,
            ),
        ] {
            let block = state(text);
            assert!(block.props().has_analog_output_signal(), "{text}");
            assert_eq!(signal::analog_output(block), Some(output), "{text}");
        }
        assert_eq!(signal::analog_output(blocks::STONE), None);
    }
}
