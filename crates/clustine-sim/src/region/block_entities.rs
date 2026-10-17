//! Typed block entities and their tickers (`docs/adr/0026-redstone.md`, section 5):
//! the block entities whose state the simulation reads or ticks, beside the opaque
//! data every chunk keeps, and the list of tickers the game's
//! `Level.tickBlockEntities` runs after the block events and the entities.
//!
//! The first is the bell, whose ring and shaking live in memory only, as in the game
//! (its NBT keeps neither): a bell placed or loaded starts still. R4's comparator and
//! R5's moving piston come in as further kinds of [`Typed`], the piston with a ticker
//! as the bell has, and the list stays as it is:
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
use clustine_world::{BlockPos, ChunkPos};
use std::collections::BTreeMap;

use super::Region;
use crate::api::TickOutput;

/// How many ticks a bell shakes after a ring (`BellBlockEntity.DURATION`).
pub(crate) const BELL_SHAKING_TICKS: i32 = 50;

/// A block entity whose state the simulation reads or ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typed {
    Bell(Bell),
}

impl Typed {
    /// A new block entity of the kind `state` keeps, if it is a typed one.
    fn of(state: BlockState) -> Option<Typed> {
        is_class(state, BlockClass::BellBlock).then(|| Typed::Bell(Bell::default()))
    }

    /// Whether the kind has a ticker (`EntityBlock.getTicker` on the server).
    fn ticks(self) -> bool {
        match self {
            Typed::Bell(_) => true,
        }
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
        if let Some(typed) = Typed::of(new) {
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
            .keys()
            .filter_map(|at| {
                let state = chunk.get(usize::from(at.x), i32::from(at.y), usize::from(at.z))?;
                let pos = BlockPos::new(
                    position.x * 16 + i32::from(at.x),
                    i32::from(at.y),
                    position.z * 16 + i32::from(at.z),
                );
                Some((pos, Typed::of(state)?))
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
            match self.block_entities.typed.get_mut(&pos) {
                Some(Typed::Bell(bell)) if is_class(state, BlockClass::BellBlock) => bell.tick(),
                _ => {}
            }
        }
        self.block_entities.ticking = false;
    }

    /// The bell at `pos`, if its block entity is there, for the tests.
    #[cfg(test)]
    pub(crate) fn bell(&self, pos: BlockPos) -> Option<Bell> {
        match self.block_entities.typed.get(&pos)? {
            Typed::Bell(bell) => Some(*bell),
        }
    }

    /// `BellBlockEntity.onHit` of the bell at `pos`. Returns whether there was one.
    pub(super) fn hit_bell(&mut self, pos: BlockPos, direction: Face) -> bool {
        match self.block_entities.typed.get_mut(&pos) {
            Some(Typed::Bell(bell)) => {
                bell.hit(direction);
                true
            }
            None => false,
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
            None => false,
        }
    }

    /// The positions of the tickers in the order they run, for the tests.
    #[cfg(test)]
    pub(crate) fn tickers(&self) -> Vec<BlockPos> {
        self.block_entities.tickers.clone()
    }
}
