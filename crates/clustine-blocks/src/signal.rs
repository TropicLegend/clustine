//! Redstone's signals (`docs/adr/0026-redstone.md`, section 1). The game's queries come
//! here with redstone's rules (step R3); what is here first is which blocks have rules
//! that read a signal, which decides where those rules may run.

use clustine_data::{BlockClass, BlockState};

use crate::data::is_class;

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
/// comparator's use works out its output again (`ComparatorBlock.useWithoutItem`).
/// Every other use reads the block itself, and what it sets in turn is updated as
/// any change, under the updater's own test.
pub fn use_reads_signal(state: BlockState) -> bool {
    is_class(state, BlockClass::ComparatorBlock)
}
