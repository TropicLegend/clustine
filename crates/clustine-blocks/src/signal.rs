// The queries follow the game's `SignalGetter` and each block's `getSignal`,
// `getDirectSignal`, `ownSignal` and `isSignalSource` as the 26.3 jar has them; the
// shape of the wire's flag follows SteelMC's `steel-core/src/world/signal_getter.rs`,
// and the order of the seven positions its `redstone/java_hash.rs`.

//! Redstone's signals (`docs/adr/0026-redstone.md`, section 1): which blocks have
//! rules that read a signal, which decides where those rules may run; the level's
//! queries of signals, as the game's `SignalGetter` asks them; each block's own
//! signals; and what wire works out of them (its default evaluator).
//!
//! The direction a query is given is the game's: from the block that asks towards
//! the block asked, so that a lever on the floor, whose support asks it upwards,
//! powers that support directly.

use clustine_data::{BlockClass, BlockState, Face};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::connect::{is_conductor, wire_connection_state, wire_is_cross, wire_is_dot};
use crate::data::{HORIZONTAL, face_name, facing, flag, is_class, relative, same_block, set};

/// The strongest signal there is.
pub const MAX_SIGNAL: u8 = 15;

/// Whether `state`'s rules read a signal, or what is up to two blocks from it, as
/// redstone does: such a block's updates, its clicks and its placing run only in a
/// chunk that ticks, where all it can read is there (the record, section 1). The rest
/// read at most their six neighbours, which an update's own test makes sure of.
pub fn reads_signal(state: BlockState) -> bool {
    use BlockClass as C;
    [
        C::RedstoneWireBlock,
        C::DiodeBlock,
        C::RedstoneTorchBlock,
        C::RedstoneWallTorchBlock,
        C::ObserverBlock,
        C::PistonBaseBlock,
        C::RedstoneLampBlock,
        C::DoorBlock,
        C::TrapDoorBlock,
        C::FenceGateBlock,
        C::BaseRailBlock,
        C::NoteBlock,
        C::CopperBulbBlock,
        C::BellBlock,
        C::TargetBlock,
        C::BigDripleafBlock,
        C::TntBlock,
        C::DispenserBlock,
        C::HopperBlock,
        C::CrafterBlock,
    ]
    .into_iter()
    .any(|class| is_class(state, class))
}

/// Whether using `state` by a click reads a signal or what is two blocks away: a
/// comparator's use works out its output again (`ComparatorBlock.useWithoutItem`),
/// and wire's finds its sides again, which reads the blocks above and below its
/// neighbours (`RedstoneWireBlock.useWithoutItem`). Every other use reads the block
/// itself, and what it sets in turn is updated as any change, under the updater's own
/// test.
pub fn use_reads_signal(state: BlockState) -> bool {
    is_class(state, BlockClass::ComparatorBlock) || is_wire(state)
}

/// Whether placing `state` reads a signal or what is two blocks away, so that it is
/// placed only where its chunk ticks (the record, section 1): a lamp, a door, a
/// trapdoor and a gate take their state from the power beside them
/// (`getStateForPlacement`), wire works out its power as it is placed
/// (`RedstoneWireBlock.onPlace`), a rail chooses its curve and a powered rail its
/// power by it (`BaseRailBlock.onPlace`), a copper bulb lights
/// (`CopperBulbBlock.onPlace`), and a repeater or a comparator beside power schedules
/// its tick (`DiodeBlock.setPlacedBy`).
pub fn placing_reads_signal(state: BlockState) -> bool {
    use BlockClass as C;
    [
        C::RedstoneLampBlock,
        C::DoorBlock,
        C::TrapDoorBlock,
        C::FenceGateBlock,
        C::RedstoneWireBlock,
        C::BaseRailBlock,
        C::CopperBulbBlock,
        C::DiodeBlock,
        C::PistonBaseBlock,
    ]
    .into_iter()
    .any(|class| is_class(state, class))
}

/// What a query of signals is asked with. The game's wire turns its own signals off
/// while it measures what else powers it (`RedstoneWireBlock.shouldSignal`, a field of
/// the one wire block); here that is a flag of the query, so that nothing outside the
/// query sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Query {
    /// Whether wire gives its signals in this query.
    pub wire_signals: bool,
}

impl Query {
    /// A query as every rule but wire's asks it.
    pub const ALL: Query = Query { wire_signals: true };
    /// A query as wire asks it while it measures what powers it.
    pub const WITHOUT_WIRE: Query = Query {
        wire_signals: false,
    };
}

/// Whether `state` is redstone wire.
pub fn is_wire(state: BlockState) -> bool {
    is_class(state, BlockClass::RedstoneWireBlock)
}

/// The game's `isSignalSource`, of the state as the table has it, but for wire, which
/// is one only while it gives its signals.
pub fn is_signal_source(state: BlockState, query: Query) -> bool {
    if is_wire(state) {
        return query.wire_signals;
    }
    state.props().is_signal_source()
}

/// The game's `ownSignal`: the signal a block gives of itself, whatever the side, as
/// far as its state says it. A comparator's is kept in its block entity, which
/// [`own_signal_at`] reads; here it is 0.
pub fn own_signal(state: BlockState) -> u8 {
    if is_class(state, BlockClass::PoweredBlock) {
        return MAX_SIGNAL;
    }
    if is_class(state, BlockClass::LeverBlock) || is_class(state, BlockClass::ButtonBlock) {
        return if flag(state, "powered") {
            MAX_SIGNAL
        } else {
            0
        };
    }
    if is_wire(state) {
        return wire_power(state);
    }
    if let Some(signal) = plate_signal(state) {
        return signal;
    }
    if is_class(state, BlockClass::TripWireHookBlock) && flag(state, "powered") {
        return MAX_SIGNAL;
    }
    // `RedstoneTorchBlock.ownSignal`, the wall torch's too.
    if crate::diode::is_torch(state) {
        return if flag(state, "lit") { MAX_SIGNAL } else { 0 };
    }
    // `DiodeBlock.ownSignal` with a repeater's `getOutputSignal`, and
    // `ObserverBlock.ownSignal`: all while powered.
    if is_class(state, BlockClass::RepeaterBlock) || is_class(state, BlockClass::ObserverBlock) {
        return if flag(state, "powered") {
            MAX_SIGNAL
        } else {
            0
        };
    }
    // `TargetBlock.ownSignal`: the power an arrow last gave it, which falls back to 0.
    if is_class(state, BlockClass::TargetBlock) {
        return crate::data::number(state, "power").unwrap_or(0);
    }
    0
}

/// `ownSignal` of the block `state` at `pos`: as [`own_signal`], and a powered
/// comparator's the output its block entity keeps (`ComparatorBlock.getOutputSignal`).
pub fn own_signal_at<V: BlockView + ?Sized>(view: &mut V, state: BlockState, pos: BlockPos) -> u8 {
    if is_class(state, BlockClass::ComparatorBlock) {
        return if flag(state, "powered") {
            view.comparator_output(pos).min(MAX_SIGNAL)
        } else {
            0
        };
    }
    own_signal(state)
}

/// `BasePressurePlateBlock.getSignalForState`: a plate's power, all or nothing for
/// one that is pressed or not and its count of what stands on it for a weighted one;
/// `None` for anything that is not a plate.
pub fn plate_signal(state: BlockState) -> Option<u8> {
    if is_class(state, BlockClass::WeightedPressurePlateBlock) {
        return Some(crate::data::number(state, "power").unwrap_or(0));
    }
    if is_class(state, BlockClass::BasePressurePlateBlock) {
        return Some(if flag(state, "powered") {
            MAX_SIGNAL
        } else {
            0
        });
    }
    None
}

/// `getAnalogOutputSignal` of a block whose output is a function of its state alone,
/// which a comparator reads behind it, as the jar's classes give it: a copper bulb's 15
/// while lit (`CopperBulbBlock`), a cake's two for each slice left
/// (`CakeBlock.getOutputSignal`) and a candle cake's 14, a composter's and a cauldron's
/// level (`ComposterBlock`, `LayeredCauldronBlock`), lava in a cauldron 3, a portal
/// frame's 15 with an eye in it, a respawn anchor's charges scaled to 15
/// (`RespawnAnchorBlock.getScaledChargeLevel`), a hive's honey, a copper golem
/// statue's pose (1 to 4). `None` for a block
/// without an output, and for those whose output is kept in an inventory or a block
/// entity (containers, a jukebox's disc, a lectern's book, a creaking heart), which
/// wait for their phase.
pub fn analog_output(state: BlockState) -> Option<u8> {
    use BlockClass as C;
    let level = |name| crate::data::number(state, name).unwrap_or(0);
    if is_class(state, C::CopperBulbBlock) {
        return Some(if flag(state, "lit") { MAX_SIGNAL } else { 0 });
    }
    if is_class(state, C::CandleCakeBlock) {
        return Some(14);
    }
    if is_class(state, C::CakeBlock) {
        return Some(7u8.saturating_sub(level("bites")) * 2);
    }
    if is_class(state, C::ComposterBlock) || is_class(state, C::LayeredCauldronBlock) {
        return Some(level("level"));
    }
    if is_class(state, C::LavaCauldronBlock) {
        return Some(3);
    }
    if is_class(state, C::EndPortalFrameBlock) {
        return Some(if flag(state, "eye") { MAX_SIGNAL } else { 0 });
    }
    if is_class(state, C::RespawnAnchorBlock) {
        // `Mth.floor(charge / 4.0f * 15)`.
        return Some((f32::from(level("charges")) / 4.0 * 15.0).floor() as u8);
    }
    if is_class(state, C::BeehiveBlock) {
        return Some(level("honey_level"));
    }
    // `CopperGolemStatueBlock`: one more than the ordinal of its pose.
    if is_class(state, C::CopperGolemStatueBlock) {
        let poses = ["standing", "sitting", "running", "star"];
        let pose = state.property("copper_golem_pose");
        let index = poses
            .iter()
            .position(|name| Some(*name) == pose)
            .unwrap_or(0);
        return Some(u8::try_from(index + 1).expect("one of four"));
    }
    None
}

/// A block's `getSignal`: the signal `state` at `pos` gives the block that asks it
/// towards `direction`. For all but wire it is the block's own signal, as the game's
/// `BlockBehaviour.getSignal` gives it.
pub fn block_signal<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    query: Query,
) -> u8 {
    if is_wire(state) {
        return wire_signal(view, state, pos, direction, query);
    }
    // `RedstoneWallTorchBlock.getSignal`: every side but the wall it hangs on, which
    // asks it the way it faces; `RedstoneTorchBlock.getSignal`: every side but the
    // block below, which asks it upwards.
    if is_class(state, BlockClass::RedstoneWallTorchBlock) {
        return if facing(state) == Some(direction) {
            0
        } else {
            own_signal(state)
        };
    }
    if crate::diode::is_torch(state) {
        return if direction == Face::Up {
            0
        } else {
            own_signal(state)
        };
    }
    // `DiodeBlock.getSignal` and `ObserverBlock.getSignal`: only the block in front,
    // behind for an observer, which asks it the way it faces.
    if is_class(state, BlockClass::DiodeBlock) || is_class(state, BlockClass::ObserverBlock) {
        return if facing(state) == Some(direction) {
            own_signal_at(view, state, pos)
        } else {
            0
        };
    }
    own_signal_at(view, state, pos)
}

/// A block's `getDirectSignal`: the signal `state` at `pos` gives directly, through a
/// conductor, to the block that asks it towards `direction`. A lever, a button or a
/// tripwire hook powers the block it hangs on, a pressure plate the block below it;
/// wire powers what it gives a signal to; the rest give none.
pub fn block_direct_signal<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    query: Query,
) -> u8 {
    if is_class(state, BlockClass::LeverBlock) || is_class(state, BlockClass::ButtonBlock) {
        let held = flag(state, "powered") && attached_towards(state) == Some(direction);
        return if held { MAX_SIGNAL } else { 0 };
    }
    // A plate powers the block it lies on, which asks it upwards.
    if let Some(signal) = plate_signal(state) {
        return if direction == Face::Up { signal } else { 0 };
    }
    // A hook powers the block it hangs on, which asks it the way the hook faces.
    if is_class(state, BlockClass::TripWireHookBlock) {
        let held = flag(state, "powered") && facing(state) == Some(direction);
        return if held { MAX_SIGNAL } else { 0 };
    }
    if is_wire(state) {
        return if query.wire_signals {
            wire_signal(view, state, pos, direction, query)
        } else {
            0
        };
    }
    // `RedstoneTorchBlock.getDirectSignal`: the block above, which asks it downwards.
    if crate::diode::is_torch(state) {
        return if direction == Face::Down {
            block_signal(view, state, pos, direction, query)
        } else {
            0
        };
    }
    // A diode and an observer power directly what they power (`getDirectSignal` is
    // their `getSignal`).
    if is_class(state, BlockClass::DiodeBlock) || is_class(state, BlockClass::ObserverBlock) {
        return block_signal(view, state, pos, direction, query);
    }
    0
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection`: from a lever or a
/// button towards the block it hangs on, as asked from that block; up for one on the
/// floor, down for one on the ceiling, and its facing on a wall.
pub fn attached_towards(state: BlockState) -> Option<Face> {
    match state.property("face")? {
        "ceiling" => Some(Face::Down),
        "floor" => Some(Face::Up),
        _ => facing(state),
    }
}

/// `SignalGetter.getDirectSignal(pos, direction)`.
pub fn direct_signal<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    direction: Face,
    query: Query,
) -> u8 {
    let state = view.block(pos);
    block_direct_signal(view, state, pos, direction, query)
}

/// `SignalGetter.getDirectSignalTo`: the strongest direct signal into `pos` from its
/// six neighbours, down, up, north, south, west and east.
pub fn direct_signal_to<V: BlockView + ?Sized>(view: &mut V, pos: BlockPos, query: Query) -> u8 {
    let mut best = 0;
    for direction in Face::ALL {
        best = best.max(direct_signal(
            view,
            relative(pos, direction),
            direction,
            query,
        ));
        if best >= MAX_SIGNAL {
            return best;
        }
    }
    best
}

/// `SignalGetter.getSignal(pos, direction)`: the signal of the block at `pos` towards
/// `direction`, and a conductor's strongest direct signal into it.
pub fn signal<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    direction: Face,
    query: Query,
) -> u8 {
    let state = view.block(pos);
    let own = block_signal(view, state, pos, direction, query);
    if is_conductor(state) {
        own.max(direct_signal_to(view, pos, query))
    } else {
        own
    }
}

/// `SignalGetter.hasSignal(pos, direction)`.
pub fn has_signal<V: BlockView + ?Sized>(view: &mut V, pos: BlockPos, direction: Face) -> bool {
    signal(view, pos, direction, Query::ALL) > 0
}

/// `SignalGetter.hasNeighborSignal`: whether a neighbour of `pos` gives it a signal,
/// asked down, up, north, south, west and east.
pub fn has_neighbour_signal<V: BlockView + ?Sized>(view: &mut V, pos: BlockPos) -> bool {
    Face::ALL
        .into_iter()
        .any(|direction| has_signal(view, relative(pos, direction), direction))
}

/// `SignalGetter.getBestNeighborSignal`: the strongest signal a neighbour of `pos`
/// gives it.
pub fn best_neighbour_signal<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    query: Query,
) -> u8 {
    let mut best = 0;
    for direction in Face::ALL {
        let signal = signal(view, relative(pos, direction), direction, query);
        if signal >= MAX_SIGNAL {
            return MAX_SIGNAL;
        }
        best = best.max(signal);
    }
    best
}

/// `SignalGetter.getBestOwnOrNeighbourSignal`: the strongest of a neighbour's signals
/// and the block's own, if it is a source.
pub fn best_own_or_neighbour_signal<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    query: Query,
) -> u8 {
    let state = view.block(pos);
    let own = if is_signal_source(state, query) {
        own_signal_at(view, state, pos)
    } else {
        0
    };
    best_neighbour_signal(view, pos, query).max(own)
}

/// `SignalGetter.getControlInputSignal`: what a diode reads from its side at `pos`,
/// towards `direction`. With `only_diodes`, only another diode's direct signal; else
/// a redstone block's full signal, wire's power, or a source's direct signal.
pub fn control_input_signal<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    direction: Face,
    only_diodes: bool,
) -> u8 {
    let state = view.block(pos);
    if only_diodes {
        return if is_class(state, BlockClass::DiodeBlock) {
            direct_signal(view, pos, direction, Query::ALL)
        } else {
            0
        };
    }
    if same_block(state, clustine_data::blocks::REDSTONE_BLOCK) {
        return MAX_SIGNAL;
    }
    if is_wire(state) {
        return wire_power(state);
    }
    if is_signal_source(state, Query::ALL) {
        direct_signal(view, pos, direction, Query::ALL)
    } else {
        0
    }
}

/// `RedstoneWireEvaluator.getWireSignal`: the power of wire, and 0 for anything else.
pub fn wire_power(state: BlockState) -> u8 {
    if !is_wire(state) {
        return 0;
    }
    state
        .property("power")
        .and_then(|power| power.parse().ok())
        .unwrap_or(0)
}

/// Wire of `state` with the power `power`.
pub fn with_power(state: BlockState, power: u8) -> BlockState {
    set(state, "power", &power.to_string())
}

/// `RedstoneWireBlock.getSignal`: nothing upwards or while wire gives no signals; its
/// power downwards, and to a side its sides, found again, run towards.
fn wire_signal<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    query: Query,
) -> u8 {
    if !query.wire_signals || direction == Face::Down {
        return 0;
    }
    let power = wire_power(state);
    if power == 0 {
        return 0;
    }
    if direction == Face::Up {
        return power;
    }
    let sides = wire_connection_state(state, pos, view);
    let towards = sides.property(face_name(direction.opposite()));
    if towards.is_some_and(|side| side != "none") {
        power
    } else {
        0
    }
}

/// `DefaultRedstoneWireEvaluator.calculateTargetStrength`: the power wire at `pos`
/// is to have. The strongest signal of a neighbour but wire's, measured while wire
/// gives none; else one less than the strongest wire beside it, above a conductor
/// beside it where nothing that conducts is above `pos`, or below a block beside it
/// that does not conduct.
pub fn wire_target_strength<V: BlockView + ?Sized>(view: &mut V, pos: BlockPos) -> u8 {
    let block_signal = best_neighbour_signal(view, pos, Query::WITHOUT_WIRE);
    if block_signal == MAX_SIGNAL {
        return MAX_SIGNAL;
    }
    // `RedstoneWireEvaluator.getIncomingWireSignal`.
    let mut wire_signal = 0;
    for direction in HORIZONTAL {
        let beside_pos = relative(pos, direction);
        let beside = view.block(beside_pos);
        wire_signal = wire_signal.max(wire_power(beside));
        let above = view.block(relative(pos, Face::Up));
        if is_conductor(beside) && !is_conductor(above) {
            let over = view.block(relative(beside_pos, Face::Up));
            wire_signal = wire_signal.max(wire_power(over));
        } else if !is_conductor(beside) {
            let under = view.block(relative(beside_pos, Face::Down));
            wire_signal = wire_signal.max(wire_power(under));
        }
    }
    block_signal.max(wire_signal.saturating_sub(1))
}

/// The seven positions whose neighbours wire at `pos` updates when its power changes,
/// in the order the game's `DefaultRedstoneWireEvaluator.updatePowerStrength` goes
/// through them: a `HashSet<BlockPos>` of 16 buckets into which the wire and then its
/// neighbours, down, up, north, south, west and east, are put
/// (`docs/adr/0026-redstone.md`, section 2). Java goes through the buckets from the
/// lowest, each in the order its positions were put in; seven never make the set
/// grow, so a stable sort by bucket is that order.
pub fn wire_update_order(pos: BlockPos) -> [BlockPos; 7] {
    let mut positions = [
        pos,
        relative(pos, Face::Down),
        relative(pos, Face::Up),
        relative(pos, Face::North),
        relative(pos, Face::South),
        relative(pos, Face::West),
        relative(pos, Face::East),
    ];
    positions.sort_by_key(|position| java_bucket(*position));
    positions
}

/// The bucket of a `HashMap` of 16 that Java puts `pos` in: `Vec3i.hashCode`,
/// `(z * 31 + y) * 31 + x`, spread as `HashMap.hash` spreads it.
pub fn java_bucket(pos: BlockPos) -> u32 {
    let hash = pos
        .z
        .wrapping_mul(31)
        .wrapping_add(pos.y)
        .wrapping_mul(31)
        .wrapping_add(pos.x) as u32;
    (hash ^ (hash >> 16)) & 15
}

/// `RedstoneWireBlock.useWithoutItem`: a cross clicked becomes a dot and a dot a
/// cross, each as its neighbours let it, keeping its power; `None` where the wire is
/// neither, or nothing would change, and the click passes on.
pub fn wire_used<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
) -> Option<BlockState> {
    let cross = wire_is_cross(state);
    if !cross && !wire_is_dot(state) {
        return None;
    }
    let base = if cross {
        clustine_data::blocks::REDSTONE_WIRE
    } else {
        HORIZONTAL
            .into_iter()
            .fold(clustine_data::blocks::REDSTONE_WIRE, |cross, towards| {
                set(cross, face_name(towards), "side")
            })
    };
    let changed = wire_connection_state(with_power(base, wire_power(state)), pos, view);
    (changed != state).then_some(changed)
}

/// Whether wire of `state` runs towards `towards` (`RedstoneSide.isConnected`).
pub fn wire_runs(state: BlockState, towards: Face) -> bool {
    state
        .property(face_name(towards))
        .is_some_and(|side| side != "none")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::blocks;
    use std::collections::BTreeMap;

    /// Blocks set by hand around an otherwise empty world.
    #[derive(Default)]
    struct Blocks(BTreeMap<(i32, i32, i32), BlockState>);

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
    }

    fn state(text: &str) -> BlockState {
        BlockState::parse(text).expect("a state")
    }

    fn wire(power: u8) -> BlockState {
        with_power(blocks::REDSTONE_WIRE, power)
    }

    const AT: BlockPos = BlockPos::new(4, 64, -3);

    fn up(pos: BlockPos) -> BlockPos {
        relative(pos, Face::Up)
    }

    fn down(pos: BlockPos) -> BlockPos {
        relative(pos, Face::Down)
    }

    #[test]
    fn a_button_on_a_ceiling_powers_the_block_it_hangs_on_directly() {
        let button = state("minecraft:stone_button[face=ceiling,powered=true]");
        let mut stone = Blocks::default()
            .with(AT, blocks::STONE)
            .with(down(AT), button);
        assert_eq!(direct_signal_to(&mut stone, AT, Query::ALL), 15);
        assert_eq!(signal(&mut stone, AT, Face::East, Query::ALL), 15);
        // Glass does not conduct, though the button powers it directly all the same.
        let mut glass = Blocks::default()
            .with(AT, blocks::GLASS)
            .with(down(AT), button);
        assert_eq!(direct_signal_to(&mut glass, AT, Query::ALL), 15);
        assert_eq!(signal(&mut glass, AT, Face::East, Query::ALL), 0);
    }

    #[test]
    fn a_lever_on_a_wall_powers_the_block_behind_it_and_signals_every_side() {
        // Facing east, it hangs on the block to its west.
        let lever = state("minecraft:lever[face=wall,facing=east,powered=true]");
        let behind = relative(AT, Face::West);
        let mut view = Blocks::default()
            .with(AT, lever)
            .with(behind, blocks::STONE);
        assert_eq!(direct_signal(&mut view, AT, Face::East, Query::ALL), 15);
        assert_eq!(direct_signal(&mut view, AT, Face::West, Query::ALL), 0);
        for direction in Face::ALL {
            assert_eq!(signal(&mut view, AT, direction, Query::ALL), 15);
        }
        assert!(has_neighbour_signal(&mut view, relative(AT, Face::Up)));
        // The stone behind passes it on.
        assert_eq!(signal(&mut view, behind, Face::West, Query::ALL), 15);
        let off = state("minecraft:lever[face=wall,facing=east,powered=false]");
        let mut view = Blocks::default().with(AT, off).with(behind, blocks::STONE);
        assert_eq!(signal(&mut view, behind, Face::West, Query::ALL), 0);
    }

    #[test]
    fn a_control_input_takes_a_redstone_block_and_wire_whole_but_only_diodes_when_asked() {
        let east = relative(AT, Face::East);
        let mut view = Blocks::default()
            .with(AT, blocks::REDSTONE_BLOCK)
            .with(east, wire(7));
        assert_eq!(control_input_signal(&mut view, AT, Face::North, false), 15);
        assert_eq!(signal(&mut view, AT, Face::Down, Query::ALL), 15);
        assert_eq!(best_own_or_neighbour_signal(&mut view, AT, Query::ALL), 15);
        assert_eq!(control_input_signal(&mut view, east, Face::North, false), 7);
        assert_eq!(control_input_signal(&mut view, AT, Face::North, true), 0);
    }

    #[test]
    fn powered_wire_signals_the_sides_it_runs_to_once_found_again_and_never_up() {
        let pos = BlockPos::new(0, 64, 0);
        let north_east = state("minecraft:redstone_wire[power=9,north=side,east=side]");
        let mut view = Blocks::default()
            .with(down(pos), blocks::STONE)
            .with(relative(pos, Face::North), blocks::REDSTONE_WIRE)
            .with(relative(pos, Face::East), blocks::REDSTONE_WIRE)
            .with(pos, north_east);
        // Asked from the east side, towards the west: the wire runs east.
        assert_eq!(
            block_signal(&mut view, north_east, pos, Face::West, Query::ALL),
            9
        );
        assert_eq!(
            block_signal(&mut view, north_east, pos, Face::East, Query::ALL),
            0
        );
        assert_eq!(
            block_signal(&mut view, north_east, pos, Face::Up, Query::ALL),
            9
        );
        assert_eq!(
            block_signal(&mut view, north_east, pos, Face::Down, Query::ALL),
            0
        );
        let quiet = Query::WITHOUT_WIRE;
        assert_eq!(block_signal(&mut view, north_east, pos, Face::Up, quiet), 0);
        assert_eq!(
            block_direct_signal(&mut view, north_east, pos, Face::Up, quiet),
            0
        );
    }

    /// `calculateTargetStrength` as the game writes it, for the cases SteelMC's tests
    /// of the evaluator take.
    #[test]
    fn wire_takes_the_power_of_what_is_beside_it_and_one_less_than_wire_beside_it() {
        let pos = BlockPos::new(0, 64, 0);
        let east = relative(pos, Face::East);
        let north = relative(pos, Face::North);
        let cases: [(Blocks, u8); 9] = [
            (Blocks::default(), 0),
            (Blocks::default().with(east, blocks::REDSTONE_BLOCK), 15),
            (Blocks::default().with(east, wire(15)), 14),
            // Up onto a stone beside it.
            (
                Blocks::default()
                    .with(east, blocks::STONE)
                    .with(up(east), wire(9)),
                8,
            ),
            // Down from beside glass, which does not conduct.
            (
                Blocks::default()
                    .with(east, blocks::GLASS)
                    .with(down(east), wire(9)),
                8,
            ),
            // Not up onto the stone where stone above cuts the wire off.
            (
                Blocks::default()
                    .with(up(pos), blocks::STONE)
                    .with(east, blocks::STONE)
                    .with(up(east), wire(15)),
                0,
            ),
            // A redstone block powers the stone beside it only weakly: nothing.
            (
                Blocks::default()
                    .with(east, blocks::STONE)
                    .with(relative(east, Face::East), blocks::REDSTONE_BLOCK),
                0,
            ),
            (
                Blocks::default()
                    .with(north, blocks::REDSTONE_BLOCK)
                    .with(east, wire(12)),
                15,
            ),
            // Wire's own signal does not feed back while it measures.
            (
                Blocks::default()
                    .with(east, blocks::STONE)
                    .with(relative(east, Face::East), wire(15)),
                0,
            ),
        ];
        for (index, (mut view, power)) in cases.into_iter().enumerate() {
            assert_eq!(wire_target_strength(&mut view, pos), power, "case {index}");
        }
    }

    /// The fixtures of SteelMC's test of the order, taken from the JDK the game runs
    /// on.
    #[test]
    fn the_seven_positions_go_in_the_order_of_javas_hash_set() {
        let fixtures = [
            (
                BlockPos::new(0, 64, 0),
                ["center", "down", "south", "east", "up", "north", "west"],
            ),
            (
                BlockPos::new(1, 64, 0),
                ["up", "north", "west", "center", "down", "south", "east"],
            ),
            (
                BlockPos::new(15, 64, 0),
                ["down", "south", "east", "up", "north", "west", "center"],
            ),
            (
                BlockPos::new(16, 64, 0),
                ["center", "down", "south", "east", "up", "north", "west"],
            ),
            (
                BlockPos::new(-16, -64, 31),
                ["down", "south", "east", "up", "north", "west", "center"],
            ),
            (
                BlockPos::new(30_000_000, 319, -30_000_000),
                ["down", "south", "east", "center", "up", "north", "west"],
            ),
        ];
        for (pos, labels) in fixtures {
            let expected = labels.map(|label| match label {
                "center" => pos,
                "down" => relative(pos, Face::Down),
                "up" => relative(pos, Face::Up),
                "north" => relative(pos, Face::North),
                "south" => relative(pos, Face::South),
                "west" => relative(pos, Face::West),
                "east" => relative(pos, Face::East),
                _ => unreachable!("a label of the fixtures"),
            });
            assert_eq!(wire_update_order(pos), expected, "at {pos:?}");
        }
    }

    #[test]
    fn a_lone_cross_clicked_becomes_a_dot_and_back_and_a_line_passes() {
        let pos = BlockPos::new(0, 64, 0);
        let cross =
            state("minecraft:redstone_wire[north=side,east=side,south=side,west=side,power=3]");
        let mut view = Blocks::default()
            .with(down(pos), blocks::STONE)
            .with(pos, cross);
        let dot = wire_used(&mut view, cross, pos).expect("a cross becomes a dot");
        assert!(wire_is_dot(dot));
        assert_eq!(wire_power(dot), 3);
        assert_eq!(wire_used(&mut view, dot, pos), Some(cross));
        let line = state("minecraft:redstone_wire[north=side,south=side]");
        assert_eq!(wire_used(&mut view, line, pos), None);
    }
}
