//! A raid's tick by its rules (`Raid.tick`; `docs/groundwork/n10-plan.md`, section 9.7):
//! the countdown, the waves and their spawn search, joining, the raiders' removal,
//! victory and loss, Hero of the Village and the celebrations. Part R1 fills it on the
//! frame of `raids.rs`, which calls it for each raid of the land in the tick's order.

use clustine_world::{ChunkPos, EntityId};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `Raid.tick` of the raid `id`, kept in the chunk `chunk`.
    pub(super) fn tick_raid(&mut self, chunk: ChunkPos, id: EntityId, output: &mut TickOutput) {
        let _ = (chunk, id, output);
    }
}
