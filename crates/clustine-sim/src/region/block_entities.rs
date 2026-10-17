//! Typed block entities and their tickers (`docs/adr/0026-redstone.md`, section 5):
//! the block entities whose state the simulation reads or ticks, beside the opaque
//! data every chunk keeps, and the list of tickers the game's
//! `Level.tickBlockEntities` runs after the block events and the entities.
//!
//! The first is the bell, whose ring and shaking live in memory only, as in the game
//! (its NBT keeps neither): a bell placed or loaded starts still. The second is the
//! comparator's (R4), which is durable: its output is written into the chunk's data as
//! the game saves it, `{components: {}, OutputSignal: n}`, whenever it changes, and
//! read back from that data when the chunk is loaded. R5's moving piston comes in as a
//! further kind of [`Typed`], with a ticker as the bell has, and the list stays as it
//! is:
//!
//! - tickers run in the order they were made; one made while the phase runs waits
//!   for the next tick (the game's `pendingBlockEntityTickers`);
//! - a chunk unloaded takes its tickers out of the list, and one loaded puts those of
//!   its block entities at the list's end in the order of their positions in the chunk
//!   (height, then z, then x); a restore, a merge or a split loads its chunks in
//!   ascending order, so builds the list by chunk and then position;
//! - a ticker whose chunk does not tick waits, and one whose block entity went is
//!   dropped as the game drops a removed one.

use clustine_blocks::data::{is_class, same_block};
use clustine_data::{BlockClass, BlockState, Face};
use clustine_world::{BlockPos, ChunkPos, InChunk};
use std::collections::BTreeMap;

use super::Region;
use crate::api::TickOutput;

/// How many ticks a bell shakes after a ring (`BellBlockEntity.DURATION`).
pub(crate) const BELL_SHAKING_TICKS: i32 = 50;

/// A block entity whose state the simulation reads or ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typed {
    Bell(Bell),
    /// `ComparatorBlockEntity`: the output it keeps.
    Comparator(u8),
}

impl Typed {
    /// The block entity of the kind `state` keeps, if it is a typed one, as `data`
    /// describes it: a new one's data is empty.
    fn of(state: BlockState, data: &[u8]) -> Option<Typed> {
        if is_class(state, BlockClass::BellBlock) {
            return Some(Typed::Bell(Bell::default()));
        }
        if is_class(state, BlockClass::ComparatorBlock) {
            return Some(Typed::Comparator(read_output_signal(data)));
        }
        None
    }

    /// Whether the kind has a ticker (`EntityBlock.getTicker` on the server).
    fn ticks(self) -> bool {
        match self {
            Typed::Bell(_) => true,
            Typed::Comparator(_) => false,
        }
    }
}

const TAG_END: u8 = 0;
const TAG_INT: u8 = 3;
const TAG_COMPOUND: u8 = 10;

/// A comparator's block entity as the game saves it (`saveWithoutMetadata`) and in
/// NBT's network form, one nameless compound with its type byte first: its
/// `OutputSignal` and the empty compound of components every block entity saves, in
/// the order of the game's `HashMap` of the two names (the bucket of `components` is
/// 2 and that of `OutputSignal` 15, in a table of 16).
pub(crate) fn comparator_data(output: u8) -> Vec<u8> {
    let name = |data: &mut Vec<u8>, tag: u8, name: &str| {
        data.push(tag);
        let length = u16::try_from(name.len()).expect("a short name");
        data.extend_from_slice(&length.to_be_bytes());
        data.extend_from_slice(name.as_bytes());
    };
    let mut data = vec![TAG_COMPOUND];
    name(&mut data, TAG_COMPOUND, "components");
    data.push(TAG_END);
    name(&mut data, TAG_INT, "OutputSignal");
    data.extend_from_slice(&i32::from(output).to_be_bytes());
    data.push(TAG_END);
    data
}

/// `ComparatorBlockEntity.loadAdditional`: the `OutputSignal` of the data, 0 where it
/// has none or is not a compound as the game writes one (`getIntOr`), held to what a
/// signal can be.
fn read_output_signal(data: &[u8]) -> u8 {
    let Some((&TAG_COMPOUND, mut rest)) = data.split_first() else {
        return 0;
    };
    while let Some((&tag, after)) = rest.split_first() {
        if tag == TAG_END {
            break;
        }
        let Some((name, after)) = nbt_string(after) else {
            return 0;
        };
        if tag == TAG_INT && name == b"OutputSignal" {
            let Some(value) = after.get(..4) else {
                return 0;
            };
            let value = i32::from_be_bytes(value.try_into().expect("four bytes"));
            return u8::try_from(value.clamp(0, 15)).expect("held to a signal");
        }
        let Some(after) = skip_nbt(tag, after, 0) else {
            return 0;
        };
        rest = after;
    }
    0
}

/// A string as NBT writes one, its length first, and what follows it.
fn nbt_string(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = usize::from(u16::from_be_bytes(data.get(..2)?.try_into().ok()?));
    let string = data.get(2..2 + length)?;
    Some((string, &data[2 + length..]))
}

/// What follows the payload of a tag of type `tag` at the start of `data`; `None` for
/// data cut short, a type NBT does not have, or nesting deeper than the game allows.
fn skip_nbt(tag: u8, data: &[u8], depth: u32) -> Option<&[u8]> {
    if depth > 512 {
        return None;
    }
    let length = |data: &[u8]| -> Option<usize> {
        usize::try_from(i32::from_be_bytes(data.get(..4)?.try_into().ok()?)).ok()
    };
    match tag {
        1 => data.get(1..),
        2 => data.get(2..),
        3 | 5 => data.get(4..),
        4 | 6 => data.get(8..),
        7 => data.get(4 + length(data)?..),
        8 => nbt_string(data).map(|(_, rest)| rest),
        9 => {
            let (&element, after) = data.split_first()?;
            let count = length(after)?;
            let mut rest = after.get(4..)?;
            for _ in 0..count {
                rest = skip_nbt(element, rest, depth + 1)?;
            }
            Some(rest)
        }
        10 => {
            let mut rest = data;
            loop {
                let (&inner, after) = rest.split_first()?;
                if inner == TAG_END {
                    return Some(after);
                }
                let (_, after) = nbt_string(after)?;
                rest = skip_nbt(inner, after, depth + 1)?;
            }
        }
        11 => data.get(4 + length(data)?.checked_mul(4)?..),
        12 => data.get(4 + length(data)?.checked_mul(8)?..),
        _ => None,
    }
}

/// `BellBlockEntity`'s state on the server: which way it was last struck, whether it
/// shakes and for how long it has, and its resonance with raiders, which waits for
/// entities and so never begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Bell {
    pub(crate) click_direction: Option<Face>,
    pub(crate) shaking: bool,
    pub(crate) ticks: i32,
    pub(crate) resonating: bool,
    pub(crate) resonation_ticks: i32,
}

impl Bell {
    /// `BellBlockEntity.tick` on the server: a shaking bell counts its ticks and stops
    /// at fifty. The search for raiders five ticks into a ring, and the resonance it
    /// starts, need entities other than players and do nothing yet.
    fn tick(&mut self) {
        if self.shaking {
            self.ticks += 1;
        }
        if self.ticks >= BELL_SHAKING_TICKS {
            self.shaking = false;
            self.ticks = 0;
        }
        if self.resonating {
            if self.resonation_ticks < 40 {
                self.resonation_ticks += 1;
            } else {
                self.resonating = false;
            }
        }
    }

    /// `BellBlockEntity.onHit`: struck from `direction`; a bell already shaking starts
    /// its shaking again.
    fn hit(&mut self, direction: Face) {
        self.click_direction = Some(direction);
        if self.shaking {
            self.ticks = 0;
        } else {
            self.shaking = true;
        }
    }

    /// `BellBlockEntity.triggerEvent` of the ring's block event: it shakes from the
    /// start, struck from `direction`.
    fn rung(&mut self, direction: Face) {
        self.resonation_ticks = 0;
        self.click_direction = Some(direction);
        self.ticks = 0;
        self.shaking = true;
    }
}

/// The typed block entities of the loaded chunks and the list of their tickers; no
/// part of the region's state, rebuilt from the chunks as they load.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct BlockEntities {
    typed: BTreeMap<BlockPos, Typed>,
    /// The tickers in the order they run.
    tickers: Vec<BlockPos>,
    /// Tickers made while the phase runs, which join the list at the next.
    pending: Vec<BlockPos>,
    /// Whether the phase of block entities runs (`Level.tickingBlockEntities`).
    ticking: bool,
}

impl BlockEntities {
    fn add(&mut self, pos: BlockPos, typed: Typed) {
        let had_ticker = self.typed.insert(pos, typed).is_some_and(Typed::ticks);
        if !typed.ticks() || had_ticker {
            return;
        }
        if self.ticking {
            self.pending.push(pos);
        } else {
            self.tickers.push(pos);
        }
    }

    fn remove(&mut self, pos: BlockPos) {
        if self.typed.remove(&pos).is_some() {
            self.tickers.retain(|ticker| *ticker != pos);
            self.pending.retain(|ticker| *ticker != pos);
        }
    }
}

impl Region {
    /// What a change of `old` to `new` at `pos` makes of the block entity there, as the
    /// game's `LevelChunk.setBlockState` does: a block of another kind drops it, a new
    /// block that keeps a typed one makes it, a block of the same kind keeps it.
    pub(super) fn block_entity_changed(&mut self, pos: BlockPos, old: BlockState, new: BlockState) {
        if same_block(old, new) {
            return;
        }
        self.block_entities.remove(pos);
        if let Some(typed) = Typed::of(new, &[]) {
            self.block_entities.add(pos, typed);
        }
    }

    /// The typed block entities of the chunk at `position`, just loaded, in the order
    /// of their positions, with their tickers at the list's end.
    pub(crate) fn take_up_block_entities(&mut self, position: ChunkPos) {
        let Some(chunk) = self.chunks.get(&position) else {
            return;
        };
        let made: Vec<(BlockPos, Typed)> = chunk
            .block_entities()
            .iter()
            .filter_map(|(at, entity)| {
                let state = chunk.get(usize::from(at.x), i32::from(at.y), usize::from(at.z))?;
                let pos = BlockPos::new(
                    position.x * 16 + i32::from(at.x),
                    i32::from(at.y),
                    position.z * 16 + i32::from(at.z),
                );
                Some((pos, Typed::of(state, &entity.data)?))
            })
            .collect();
        for (pos, typed) in made {
            self.block_entities.add(pos, typed);
        }
    }

    /// Takes the typed block entities of the chunk at `position`, unloaded, and their
    /// tickers away: what they held in memory is lost, as the game loses a bell's ring.
    pub(crate) fn let_go_block_entities(&mut self, position: ChunkPos) {
        let gone: Vec<BlockPos> = self
            .block_entities
            .typed
            .keys()
            .copied()
            .filter(|pos| pos.chunk() == position)
            .collect();
        for pos in gone {
            self.block_entities.remove(pos);
        }
    }

    /// `Level.tickBlockEntities`: the tickers made since the last phase join the list,
    /// and each ticks where its chunk ticks and its block is still of its kind.
    pub(crate) fn run_block_entities(&mut self, output: &mut TickOutput) {
        // A bell changes no block as it ticks; R5's moving pistons will.
        let _ = output;
        self.block_entities.ticking = true;
        let pending = std::mem::take(&mut self.block_entities.pending);
        self.block_entities.tickers.extend(pending);
        let tickers = self.block_entities.tickers.clone();
        for pos in tickers {
            if !self.ticks_chunk(pos.chunk()) {
                continue;
            }
            let Some(state) = self.own_block(pos) else {
                continue;
            };
            if let Some(Typed::Bell(bell)) = self.block_entities.typed.get_mut(&pos)
                && is_class(state, BlockClass::BellBlock)
            {
                bell.tick();
            }
        }
        self.block_entities.ticking = false;
    }

    /// The bell at `pos`, if its block entity is there, for the tests.
    #[cfg(test)]
    pub(crate) fn bell(&self, pos: BlockPos) -> Option<Bell> {
        match self.block_entities.typed.get(&pos)? {
            Typed::Bell(bell) => Some(*bell),
            Typed::Comparator(_) => None,
        }
    }

    /// The output the comparator's block entity at `pos` keeps
    /// (`ComparatorBlockEntity.getOutputSignal`): of a loaded chunk, the typed block
    /// entity's; of a copy, read from the copy's data, which the region never writes.
    /// 0 where there is none.
    pub(crate) fn comparator_output(&self, pos: BlockPos) -> u8 {
        if let Some(typed) = self.block_entities.typed.get(&pos) {
            return match typed {
                Typed::Comparator(output) => *output,
                Typed::Bell(_) => 0,
            };
        }
        if self.chunks.contains_key(&pos.chunk()) {
            return 0;
        }
        let Some(copy) = self.copies.get(&pos.chunk()) else {
            return 0;
        };
        let (x, z) = pos.in_chunk();
        let is_comparator = copy
            .get(x, pos.y, z)
            .is_some_and(|state| is_class(state, BlockClass::ComparatorBlock));
        match copy.block_entities().get(&InChunk::new(x, pos.y, z)) {
            Some(entity) if is_comparator => read_output_signal(&entity.data),
            _ => 0,
        }
    }

    /// `ComparatorBlockEntity.setOutputSignal` of the comparator at `pos`, written into
    /// its chunk's data and said among the tick's changes when it changes, so that it
    /// is committed with them (`docs/adr/0026-redstone.md`, sections 5 and 6). Returns
    /// the output it kept before, 0 where there is no comparator's block entity.
    pub(crate) fn set_comparator_output(
        &mut self,
        pos: BlockPos,
        output: u8,
        events: &mut TickOutput,
    ) -> u8 {
        let Some(Typed::Comparator(kept)) = self.block_entities.typed.get_mut(&pos) else {
            return 0;
        };
        let before = *kept;
        if before == output {
            return before;
        }
        *kept = output;
        self.set_block_entity_data(pos, comparator_data(output), events);
        before
    }

    /// `BellBlockEntity.onHit` of the bell at `pos`. Returns whether there was one.
    pub(super) fn hit_bell(&mut self, pos: BlockPos, direction: Face) -> bool {
        match self.block_entities.typed.get_mut(&pos) {
            Some(Typed::Bell(bell)) => {
                bell.hit(direction);
                true
            }
            _ => false,
        }
    }

    /// The ring's block event reaching the bell at `pos`: whether it had a bell to
    /// shake (`BaseEntityBlock.triggerEvent`).
    pub(super) fn ring_bell_event(&mut self, pos: BlockPos, direction: Face) -> bool {
        match self.block_entities.typed.get_mut(&pos) {
            Some(Typed::Bell(bell)) => {
                bell.rung(direction);
                true
            }
            _ => false,
        }
    }

    /// The positions of the tickers in the order they run, for the tests.
    #[cfg(test)]
    pub(crate) fn tickers(&self) -> Vec<BlockPos> {
        self.block_entities.tickers.clone()
    }
}
