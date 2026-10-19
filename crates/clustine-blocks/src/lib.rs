//! How a block takes its shape from its neighbours (the game's `updateShape`) and
//! whether it can stay where it is (`canSurvive`): one set of rules, which the
//! simulation asks as a player places a block and world generation asks at the edges
//! of what a feature places (ADR-0024, section 7).
//!
//! The rules read blocks and schedule ticks through a [`BlockView`], and nothing else:
//! no I/O, no clock and no unordered collections, so that a region's tick and a feature
//! run stay functions of what they read. `clippy.toml` in this crate enforces part of
//! that.

pub mod connect;
pub mod data;
pub mod diode;
pub mod fluid;
pub mod growth;
pub mod loot;
pub mod note;
pub mod piston;
pub mod shape;
pub mod signal;
pub mod support;
pub mod survive;
#[cfg(test)]
mod tests;

use clustine_data::{BlockState, Face};
use clustine_world::BlockPos;

pub use shape::{leaf_distance, mossy_carpet_updated, update_shape};
pub use survive::{Survival, can_survive, multiface_can_attach, sturdy, survival_rule};

/// What the rules read of the world and ask of it.
pub trait BlockView {
    /// The block at `at`. A view answers for every position a rule asks of: the
    /// simulation answers air for a block in a chunk it cannot see.
    fn block(&mut self, at: BlockPos) -> BlockState;

    /// The light at `at` as the game's `getRawBrightness(at, 0)` gives it, or `None`
    /// where the caller has no light: a rule that needs light then does not judge it.
    fn raw_brightness(&mut self, at: BlockPos) -> Option<u8>;

    /// `scheduleTick` of the block `block` (its id) at `at`, `delay` ticks from now.
    fn schedule_block_tick(&mut self, at: BlockPos, block: u16, delay: i32);

    /// `scheduleTick` of the fluid `fluid` (its id in `clustine_data::FLUIDS`).
    fn schedule_fluid_tick(&mut self, at: BlockPos, fluid: u8, delay: i32);

    /// Which side of the rules the two former copies disagreed on this caller takes.
    fn rules(&self) -> Rules;

    /// `RandomSource.nextInt(bound)`, for a rule that draws (a coral's time to die out
    /// of water). Generation's views draw nothing and answer 0: the rules that draw are
    /// the simulation's.
    fn next_int(&mut self, bound: u32) -> u32;

    /// `LevelTickAccess.hasScheduledTick` of the block `block` (its id) at `at`: a tick
    /// of it pending there, not one already drawn to run in this tick. An observer
    /// asks it as its shape is asked again (`ObserverBlock.startSignal`). A view that
    /// keeps no ticks answers that none is pending.
    fn has_scheduled_block_tick(&mut self, _at: BlockPos, _block: u16) -> bool {
        false
    }

    /// The output a comparator's block entity at `at` keeps
    /// (`ComparatorBlockEntity.getOutputSignal`), which is the signal a powered
    /// comparator gives; 0 where there is none, or for a view that keeps no block
    /// entities.
    fn comparator_output(&mut self, _at: BlockPos) -> u8 {
        0
    }

    /// What a comparator facing `facing` reads of the item frames at `at`, the block
    /// beyond the conductor behind it (`ComparatorBlock.getItemFrame`): the analog
    /// output of the one frame facing the same way whose box meets the block's, or
    /// `None` where there is none, or more than one. A view without entities has none.
    fn item_frame_signal(&mut self, _at: BlockPos, _facing: Face) -> Option<u8> {
        None
    }

    /// `getAnalogOutputSignal` of the block `state` at `at`, which a comparator reads
    /// behind it: what the state alone gives ([`signal::analog_output`]), but where the
    /// caller counts what entities on the block hold (a detector rail's minecarts).
    fn analog_output(&mut self, _at: BlockPos, state: BlockState) -> u8 {
        signal::analog_output(state).unwrap_or(0)
    }

    /// How many players have the trapped chest at `at` open
    /// (`ChestBlockEntity.getOpenCount`), which is its signal; 0 for a view that keeps
    /// no block entities.
    fn trapped_chest_openers(&mut self, _at: BlockPos) -> u8 {
        0
    }
}

/// The callers whose copies of these rules disagreed before they were made one, each
/// of which keeps its own answer where they did until the game judges between them.
/// Where only one copy had a rule (the other said "not ported"), the rule is taken
/// for both unless taking it would change what a player can place; those cases are
/// named at [`survive::survival_rule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rules {
    /// World generation's features (`clustine-features`).
    Generation,
    /// The simulation, placing a block for a player (`clustine-sim`).
    Placing,
}
