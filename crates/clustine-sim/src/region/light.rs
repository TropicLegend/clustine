//! Light in the region (`docs/groundwork/n8-plan.md`, section 4; ADR-0029): kept up to
//! date by `clustine_light::engine` at a step of its own before `update_ticking`, so that
//! every rule of the tick reads light current with the blocks as the tick began.
//!
//! A chunk the region comes to have (loaded or copied) waits until the step takes it,
//! within a budget, nearest the region's players first; a chunk ticks only once it and its
//! eight neighbours are lit. What changes a chunk's blocks is noted by the chunk itself
//! (`Chunk::set`), so no write goes round the engine. The light is no part of the
//! region's state, its commit or its equality: a restored region lights its chunks anew.

use std::collections::BTreeSet;

use clustine_light::engine::{Chunks, LightAt, Lighting};
use clustine_world::{BlockPos, Chunk, ChunkPos};

use super::Region;

/// What one chunk lit whole costs of a step's budget, and one adopted with levels kept
/// from elsewhere (the plan's 4.5).
pub(crate) const WHOLE: u32 = 8;

/// How much light a step may take on: a budget of units, or none at all. The region's
/// configuration says which (a server's has a budget; a test's has none, so that a chunk
/// it loads ticks in the tick it comes, as before).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightSettings {
    /// The units of waiting chunks a step takes, `None` for all of them.
    pub budget: Option<u32>,
}

impl LightSettings {
    /// A server's: 256 units a step, 32 chunks lit whole (the plan's 4.5, the lead's
    /// figure until the measurements of N8e set it).
    pub const SERVER: LightSettings = LightSettings { budget: Some(256) };
}

/// The region's light: the engine, and the chunks that wait for it. Equal always and
/// shown by counts, as it is no part of what the region is between ticks (the plan's
/// 4.6).
pub(crate) struct RegionLight {
    engine: Lighting,
    /// The chunks given to the engine.
    known: BTreeSet<ChunkPos>,
    /// The chunks present that wait for their turn.
    waiting: BTreeSet<ChunkPos>,
}

impl Default for RegionLight {
    fn default() -> Self {
        Self {
            engine: Lighting::new(true),
            known: BTreeSet::new(),
            waiting: BTreeSet::new(),
        }
    }
}

/// A region's clone has no light of its own yet: it lights what it has at its next step,
/// as a restored region does, which gives the same light (a function of the blocks).
impl Clone for RegionLight {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for RegionLight {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for RegionLight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegionLight")
            .field("known", &self.known.len())
            .field("waiting", &self.waiting.len())
            .finish()
    }
}

/// The region's chunks as the engine reads them: a loaded one before its copy.
struct Present<'a> {
    chunks: &'a std::collections::BTreeMap<ChunkPos, Chunk>,
    copies: &'a std::collections::BTreeMap<ChunkPos, Chunk>,
}

impl Chunks for Present<'_> {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&at).or_else(|| self.copies.get(&at))
    }
}

impl Region {
    /// The light step (the plan's 4.1 and 4.5): the chunks that went are taken out, those
    /// replaced whole relit, the changes noted in the chunks drained into the engine, the
    /// waiting chunks taken within the budget nearest the players first, and a run.
    pub(crate) fn light_step(&mut self) {
        let present: BTreeSet<ChunkPos> = self
            .chunks
            .keys()
            .chain(self.copies.keys())
            .copied()
            .collect();
        let light = &mut self.light;
        let gone: Vec<ChunkPos> = light.known.difference(&present).copied().collect();
        for chunk in gone {
            light.known.remove(&chunk);
            light.engine.left(chunk);
        }
        light.waiting.retain(|chunk| present.contains(chunk));
        for at in &present {
            let chunk = match self.chunks.get_mut(at) {
                Some(chunk) => chunk,
                None => self.copies.get_mut(at).expect("present"),
            };
            let noting = chunk.notes_light_changes();
            chunk.note_light_changes(true);
            let changes = chunk.take_light_changes();
            if !light.known.contains(at) {
                // Come since the last step, or in a region made anew (a clone, a restore):
                // it waits for its turn, and its changes are in what it is lit with.
                light.waiting.insert(*at);
            } else if !noting {
                // A chunk object the region has not seen, where one was: replaced whole (a
                // copy's new base, a loaded chunk become a copy or the reverse), lit in
                // this run outside the budget so that no ticking chunk beside it stops.
                light.engine.arrived(*at);
            } else {
                for changed in changes {
                    light.engine.changed(BlockPos::new(
                        at.x * 16 + i32::from(changed.x),
                        i32::from(changed.y),
                        at.z * 16 + i32::from(changed.z),
                    ));
                }
            }
        }
        // The waiting chunks, nearest the region's players first, then by position.
        let players: Vec<ChunkPos> = self
            .players
            .values()
            .map(|player| ChunkPos::containing(player.pose.position.x, player.pose.position.z))
            .collect();
        let distance = |chunk: &ChunkPos| {
            players
                .iter()
                .map(|player| (chunk.x - player.x).abs().max((chunk.z - player.z).abs()))
                .min()
                .unwrap_or(0)
        };
        let mut order: Vec<ChunkPos> = light.waiting.iter().copied().collect();
        order.sort_by_key(|chunk| (distance(chunk), *chunk));
        let mut left = self.config.light.budget;
        for chunk in order {
            if let Some(units) = &mut left {
                if *units < WHOLE {
                    break;
                }
                *units -= WHOLE;
            }
            light.waiting.remove(&chunk);
            light.known.insert(chunk);
            light.engine.arrived(chunk);
        }
        light.engine.run(&Present {
            chunks: &self.chunks,
            copies: &self.copies,
        });
    }

    /// Whether the chunk at `position` and its eight neighbours are lit, as a chunk must
    /// be to tick.
    pub(crate) fn lit_around(&self, position: ChunkPos) -> bool {
        (-1..=1).all(|dx| {
            (-1..=1).all(|dz| {
                self.light
                    .engine
                    .is_lit(ChunkPos::new(position.x + dx, position.z + dz))
            })
        })
    }

    /// The light at `pos` as of the last step: the sky's undarkened and the blocks';
    /// `None` where its chunk is not lit, absent or still waiting (the plan's 4.3).
    pub(crate) fn light_at(&self, pos: BlockPos) -> Option<LightAt> {
        if !self.light.engine.is_lit(pos.chunk()) {
            return None;
        }
        self.light.engine.at(pos)
    }

    /// `getRawBrightness(pos, darken)`: the sky's light less `darken`, or the blocks',
    /// the larger.
    pub(crate) fn raw_brightness(&self, pos: BlockPos, darken: i32) -> Option<u8> {
        let light = self.light_at(pos)?;
        Some(
            (i32::from(light.sky) - darken)
                .max(i32::from(light.block))
                .max(0) as u8,
        )
    }
}
