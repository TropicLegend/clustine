//! Step R7 of `docs/adr/0026-redstone.md`: tests written from the record and the game's
//! own classes by someone who did not write the code they test. Each says which rule it
//! checks; the numbers are the jar's (26.3), read from its bytecode where the record
//! does not give them.
//!
//! The regions are those of [`ticking_with`]: player 1 at the spawn, (0.5, -60, 0.5),
//! on a stone floor at y = -61, with the 25 chunks within two of chunk (0, 0) loaded, so
//! that the nine within one tick. "Tick T" in the tests is the tick in which the player's
//! input is applied: inputs run before the block ticks, so a tick scheduled with a delay
//! of 2 in tick T runs in tick T + 2.

use super::*;

pub(super) use super::redstone::{block_at, click, holding, lever, sounds, state, ticking_with};

mod diodes;
mod ground;
mod pistons;
mod signals;

pub(super) fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// The value of the property `name` of the block at `pos`.
pub(super) fn property(region: &Region, pos: BlockPos, name: &str) -> &'static str {
    let block = block_at(region, pos).expect("loaded");
    block
        .property(name)
        .unwrap_or_else(|| panic!("{block:?} at {pos:?} has no {name}"))
}

/// Whether the boolean property `name` of the block at `pos` is true.
pub(super) fn on(region: &Region, pos: BlockPos, name: &str) -> bool {
    property(region, pos, name) == "true"
}

/// The power of the wire at `pos`.
pub(super) fn power(region: &Region, pos: BlockPos) -> u8 {
    let block = block_at(region, pos).expect("loaded");
    assert_eq!(
        block.block_id(),
        blocks::REDSTONE_WIRE.block_id(),
        "{block:?} at {pos:?} is not wire"
    );
    property(region, pos, "power").parse().expect("a number")
}

/// Wire with the given sides, the rest left at their defaults (none) and no power.
pub(super) fn wire(sides: &str) -> BlockState {
    state(&format!("minecraft:redstone_wire[{sides},power=0]"))
}

/// The id of the block `state` is of, as a block event names it.
pub(super) fn block_id(state: BlockState) -> u16 {
    u16::try_from(state.block_id().expect("a block")).expect("a small id")
}

/// The block events a tick ran, in order: position, block, kind and data.
pub(super) fn block_events(output: &TickOutput) -> Vec<(BlockPos, u16, i32, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEvent {
                position,
                block,
                kind,
                data,
            } => Some((*position, *block, *kind, *data)),
            _ => None,
        })
        .collect()
}

/// The level events a tick showed: kind, position and data.
pub(super) fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => Some((*kind, *position, *data)),
            _ => None,
        })
        .collect()
}

/// The ids of the sounds a tick played, in order.
pub(super) fn sound_ids(output: &TickOutput) -> Vec<i32> {
    sounds(output).into_iter().map(|(id, ..)| id).collect()
}

/// Player 1 uses what is in hand on `face` of the block at `pos`: a block in hand is
/// placed against that face unless the block there has a use.
pub(super) fn use_on(pos: BlockPos, face: crate::api::Face) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::UseItemOn {
            position: pos,
            face,
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    )])
}

/// Player 1 breaks the block at `pos`.
pub(super) fn dig_at(pos: BlockPos) -> TickInputs {
    moves(vec![dig(1, pos.x, pos.y, pos.z, 1)])
}

/// Player 1 turns to look along `yaw`, level, where they stand.
pub(super) fn turn(yaw: f32) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: None,
            rotation: Some((yaw, 0.0)),
            on_ground: true,
        },
    )])
}

/// One tick with `inputs`.
pub(super) fn step(region: &mut Region, inputs: &TickInputs) -> TickOutput {
    checked(region, inputs)
}

/// `n` ticks in which nothing comes in, and what `look` sees after each.
pub(super) fn watch<T>(region: &mut Region, n: usize, look: impl Fn(&Region) -> T) -> Vec<T> {
    (0..n)
        .map(|_| {
            idle(region);
            look(region)
        })
        .collect()
}

/// The `OutputSignal` of the data of a comparator's block entity, as the game writes it
/// (`{components:{}, OutputSignal:n}`): the int tag of that name, wherever it is in the
/// compound. `None` where the data has none, which reads as 0.
pub(super) fn output_signal(data: &[u8]) -> Option<i32> {
    let name = b"OutputSignal";
    let mut tag = vec![3u8, 0, name.len() as u8];
    tag.extend_from_slice(name);
    let start = data.windows(tag.len()).position(|window| window == tag)?;
    let value = data.get(start + tag.len()..start + tag.len() + 4)?;
    Some(i32::from_be_bytes(value.try_into().expect("four bytes")))
}

/// The data of the block entity at `pos`, if the block there has one.
pub(super) fn block_entity_data(region: &Region, pos: BlockPos) -> Option<Vec<u8>> {
    let (x, z) = pos.in_chunk();
    let chunk = region.chunk(pos.chunk()).expect("loaded");
    chunk
        .block_entities()
        .get(&InChunk::new(x, pos.y, z))
        .map(|entity| entity.data.clone())
}
