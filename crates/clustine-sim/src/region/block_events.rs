//! Block events (`docs/adr/0026-redstone.md`, section 4): what the game's
//! `Level.blockEvent` queues and `ServerLevel.runBlockEvents` runs after the chunks'
//! ticks, a piston's move or a note block's note.
//!
//! The queue is in the order events were queued, without two the same. Its phase runs
//! it to empty, events queued while it runs included; an event whose chunk does not
//! tick is put back, in order, for the next tick, and one queued after the phase waits
//! for the next. A running region keeps the queue in memory, as the game does; at the
//! end of each tick every loaded chunk keeps the events of its blocks still queued, in
//! their order, so that they go with the chunk through saves, merges, splits and
//! loads by another region, and outlive a crash. A chunk unloaded takes its events out
//! of the queue; a chunk loaded puts its kept events at the queue's end.

use clustine_blocks::data::tick_id;
use clustine_data::BlockState;
use clustine_world::{BlockPos, ChunkPos, InChunk, KeptEvent};
use std::collections::BTreeSet;

use super::Region;
use crate::api::{RegionEvent, TickOutput};

/// One event of the queue: the game's `BlockEventData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueuedEvent {
    pub(crate) at: BlockPos,
    /// The block's id, which the block there has to be for the event to run.
    pub(crate) block: u16,
    pub(crate) kind: i32,
    pub(crate) data: i32,
}

impl QueuedEvent {
    fn kept(self) -> KeptEvent {
        let (x, z) = self.at.in_chunk();
        KeptEvent {
            at: InChunk::new(x, self.at.y, z),
            block: self.block,
            kind: self.kind,
            data: self.data,
        }
    }

    fn of_kept(chunk: ChunkPos, kept: KeptEvent) -> Self {
        Self {
            at: BlockPos::new(
                chunk.x * 16 + i32::from(kept.at.x),
                i32::from(kept.at.y),
                chunk.z * 16 + i32::from(kept.at.z),
            ),
            block: kept.block,
            kind: kept.kind,
            data: kept.data,
        }
    }
}

impl Region {
    /// `Level.blockEvent(pos, block, b0, b1)`: queues an event of the block `state` is
    /// of at `at`, unless the same one is queued already.
    pub(crate) fn block_event(&mut self, at: BlockPos, state: BlockState, kind: i32, data: i32) {
        let event = QueuedEvent {
            at,
            block: tick_id(state),
            kind,
            data,
        };
        if !self.block_events.contains(&event) {
            self.block_events.push(event);
        }
    }

    /// Puts the events kept in the chunk at `position`, just loaded, at the queue's end.
    pub(crate) fn take_up_events(&mut self, position: ChunkPos) {
        let Some(chunk) = self.chunks.get(&position) else {
            return;
        };
        let kept: Vec<QueuedEvent> = chunk
            .events()
            .iter()
            .map(|kept| QueuedEvent::of_kept(position, *kept))
            .collect();
        for event in kept {
            if !self.block_events.contains(&event) {
                self.block_events.push(event);
            }
        }
    }

    /// Takes the events of the chunk at `position`, unloaded, out of the queue: the
    /// chunk keeps them.
    pub(crate) fn let_go_events(&mut self, position: ChunkPos) {
        self.block_events
            .retain(|event| event.at.chunk() != position);
    }

    /// `ServerLevel.runBlockEvents`: runs the queue to empty, putting back those whose
    /// chunk does not tick.
    pub(crate) fn run_block_events(&mut self, output: &mut TickOutput) {
        // The edge sends the block changes before this with the tick's, and those
        // after it with the next tick's, as the game broadcasts them
        // (`docs/adr/0026-redstone.md`, section 8). A tick without events changes
        // nothing from here on before the block entities, which tick from R5 on.
        if !self.block_events.is_empty() {
            output.events.push(RegionEvent::BlockEventsBegin);
        }
        let mut deferred = Vec::new();
        while !self.block_events.is_empty() {
            let event = self.block_events.remove(0);
            if !self.ticks_chunk(event.at.chunk()) {
                deferred.push(event);
                continue;
            }
            if self.do_block_event(event, output) {
                output.events.push(RegionEvent::BlockEvent {
                    position: event.at,
                    block: event.block,
                    kind: event.kind,
                    data: event.data,
                });
            }
        }
        // The game adds them back to a set that may have taken some again meanwhile.
        for event in deferred {
            if !self.block_events.contains(&event) {
                self.block_events.push(event);
            }
        }
    }

    /// `ServerLevel.doBlockEvent`: the event runs where its block is still there, by
    /// that block's `triggerEvent`; whether it did is whether players are sent it.
    fn do_block_event(&mut self, event: QueuedEvent, output: &mut TickOutput) -> bool {
        let Some(state) = self.own_block(event.at) else {
            return false;
        };
        if state.block_id() != Some(usize::from(event.block)) {
            return false;
        }
        #[cfg(test)]
        self.events_run.push(event);
        self.trigger_event(event.at, state, event.kind, event.data, output)
    }

    /// `BlockBehaviour.triggerEvent` of the block `state` at `at`: a note block's note
    /// and a bell's ring (`instruments.rs`); R5's pistons come here too.
    fn trigger_event(
        &mut self,
        at: BlockPos,
        state: BlockState,
        kind: i32,
        data: i32,
        output: &mut TickOutput,
    ) -> bool {
        self.instrument_event(at, state, kind, data, output)
            .unwrap_or(false)
    }

    /// At the end of a tick: every loaded chunk keeps the events of its blocks still
    /// queued, in their order. A chunk whose kept events changed is named in the
    /// tick's output, to be committed.
    pub(crate) fn keep_events(&mut self, output: &mut TickOutput) {
        let mut touched: BTreeSet<ChunkPos> = self
            .block_events
            .iter()
            .map(|event| event.at.chunk())
            .collect();
        touched.extend(
            self.chunks
                .iter()
                .filter(|(_, chunk)| !chunk.events().is_empty())
                .map(|(position, _)| *position),
        );
        for position in touched {
            let events: Vec<KeptEvent> = self
                .block_events
                .iter()
                .filter(|event| event.at.chunk() == position)
                .map(|event| event.kept())
                .collect();
            let Some(chunk) = self.chunks.get_mut(&position) else {
                continue;
            };
            if chunk.events() != events {
                chunk.set_events(events);
                output.events_changed.push(position);
            }
        }
    }

    /// The events kept in the chunk at `position`, if it is loaded.
    pub fn kept_events(&self, position: ChunkPos) -> Vec<KeptEvent> {
        self.chunks
            .get(&position)
            .map(|chunk| chunk.events().to_vec())
            .unwrap_or_default()
    }
}
