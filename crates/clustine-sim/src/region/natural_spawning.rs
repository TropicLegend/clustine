//! The per-chunk natural spawner (`docs/groundwork/n9-plan.md`, section 9.3; part
//! S10's) on N9h's frame (`spawn_caps.rs`): `NaturalSpawner.spawnForChunk` of a spawning
//! chunk, each category the global cap left open and the local cap lets through, in the
//! categories' order.

use clustine_world::ChunkPos;

use super::Region;
use super::spawn_caps::SpawnState;
use crate::api::{SPAWN_CATEGORIES, TickOutput};

impl Region {
    /// `spawnForChunk` of `chunk`: for each category open at the start of the phase and
    /// let through by the local cap at this chunk, `spawnCategoryForChunk`.
    pub(super) fn spawn_for_chunk(
        &mut self,
        chunk: ChunkPos,
        state: &mut SpawnState,
        output: &mut TickOutput,
    ) {
        for category in 0..SPAWN_CATEGORIES {
            if state.open[category] && state.can_spawn_local(category, chunk) {
                self.spawn_category_for_chunk(category, chunk, state, output);
            }
        }
    }

    /// `spawnCategoryForChunk` of the counted category `category` (`spawn_caps.rs`'
    /// order) in `chunk`, its draws from the chunk's stream for spawning
    /// ([`super::random_ticks::Random::of_chunk`] with
    /// [`super::spawn_caps::SPAWNING`]): the position, the groups, the nearest player
    /// ([`SpawnState::nearest`]), the type's rules and the making, each mob made counted
    /// by [`SpawnState::after_spawn`]. Part S10's; nothing until then.
    #[expect(
        clippy::unused_self,
        reason = "part S10 fills it (docs/groundwork/n9-plan.md, section 9.3)"
    )]
    fn spawn_category_for_chunk(
        &mut self,
        _category: usize,
        _chunk: ChunkPos,
        _state: &mut SpawnState,
        _output: &mut TickOutput,
    ) {
    }
}
