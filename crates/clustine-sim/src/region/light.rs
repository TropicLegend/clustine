//! Light in the region (`docs/groundwork/n8-plan.md`, section 4; ADR-0029): kept up to
//! date by `clustine_light::engine` at a step of its own before `update_ticking`, so that
//! every rule of the tick reads light current with the blocks as the tick began.
//!
//! A chunk the region comes to have (loaded or copied) waits until the step takes it,
//! within a budget, nearest the region's players first; a chunk ticks only once it and its
//! eight neighbours are lit. What changes a chunk's blocks is noted by the chunk itself
//! (`Chunk::set`), so no write goes round the engine. The light is no part of the
//! region's state, its commit or its equality: a restored region lights its chunks anew.
//!
//! A level without a sky (`has_skylight` false, the Nether) carries no sky light at all:
//! the game makes its light engine with the dimension's `hasSkyLight` (n7 `ChunkMap`
//! 371–398, `ThreadedLevelLightEngine`'s constructor), which then has no sky layer and
//! reads 0 for it, and reads no sky light from a saved chunk there (n10
//! `SerializableChunkData` 71–78). So the engine runs on the blocks' light alone and every
//! sky level read is 0 (`docs/groundwork/p-plan.md`, sections 17.2 D1 and 17.5, their 8).

use std::collections::BTreeSet;

use clustine_light::Around;
use clustine_light::engine::{Chunks, Kept, LightAt, Lighting};
use clustine_world::{BlockPos, Chunk, ChunkPos, SectionLight};

use super::Region;

/// What one chunk lit whole costs of a step's budget, and one adopted with levels kept
/// from elsewhere (the plan's 4.5).
pub(crate) const WHOLE: u32 = 8;
pub(crate) const ADOPTED: u32 = 1;

/// How much light a step may take on: a budget of units, or none at all; and how much of
/// it the region checks. The region's configuration says which (a server's has a budget;
/// a test's has none, so that a chunk it loads ticks in the tick it comes, as before).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightSettings {
    /// The units of waiting chunks a step takes, `None` for all of them.
    pub budget: Option<u32>,
    /// How much of the light the region checks after each step.
    pub checks: LightChecks,
}

impl LightSettings {
    /// A server's: 256 units a step, 32 chunks lit whole (the plan's 4.5, the lead's
    /// figure until the measurements of N8e set it), and no checks.
    pub const SERVER: LightSettings = LightSettings {
        budget: Some(256),
        checks: LightChecks::Off,
    };
}

/// A test's: no budget, and in the simulation's own tests every chunk a step touched
/// checked (the plan's 4.7).
impl Default for LightSettings {
    fn default() -> Self {
        Self {
            budget: None,
            checks: if cfg!(test) {
                LightChecks::Touched
            } else {
                LightChecks::Off
            },
        }
    }
}

/// How much of its light a region compares, after each step, with the light
/// `clustine_light::light` gives a lit chunk from its lit 3×3, panicking at a difference
/// (the plan's 4.7). A setting of the configuration and not a cargo feature, which would
/// unify into the workspace's whole test build; and a difference left by a missed change
/// lasts until something lights that place again, so a check that comes round to it
/// later still finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightChecks {
    /// None, as on a server.
    Off,
    /// Every chunk the step lit whole, adopted, replaced or could have changed the light
    /// of: the simulation's own tests, whose worlds are small.
    Touched,
    /// This many lit chunks a step, taken in turn through the region's in ascending
    /// order from where the last step stopped: the end-to-end tests'
    /// (`CLUSTINE_LIGHT_ORACLE`), so that the whole check grows by little.
    InTurn(u32),
}

/// The region's light: the engine, and the chunks that wait for it. Equal always and
/// shown by counts, as it is no part of what the region is between ticks (the plan's
/// 4.6).
pub(crate) struct RegionLight {
    /// Whether the region's level has a sky (`DimensionType.hasSkyLight`), which the
    /// engine was made with.
    sky: bool,
    engine: Lighting,
    /// The chunks given to the engine.
    known: BTreeSet<ChunkPos>,
    /// The chunks present that wait for their turn.
    waiting: BTreeSet<ChunkPos>,
    /// The light some waiting chunks came with from a merge or a split, adopted when
    /// their turn comes.
    kept: std::collections::BTreeMap<ChunkPos, Kept>,
    /// The chunks whose light the last step could have changed, where every one is
    /// checked ([`LightChecks::Touched`]).
    touched: BTreeSet<ChunkPos>,
    /// The last chunk checked in turn ([`LightChecks::InTurn`]).
    checked: Option<ChunkPos>,
}

impl RegionLight {
    /// No chunk known yet, in a level with a sky or without one.
    pub(crate) fn new(sky: bool) -> Self {
        Self {
            sky,
            engine: Lighting::new(sky),
            known: BTreeSet::new(),
            waiting: BTreeSet::new(),
            kept: std::collections::BTreeMap::new(),
            touched: BTreeSet::new(),
            checked: None,
        }
    }
}

/// A level with a sky, as the overworld has; the region makes its light again for its
/// own level at its first step ([`Region::light_for_level`]).
impl Default for RegionLight {
    fn default() -> Self {
        Self::new(true)
    }
}

/// A region's clone has no light of its own yet: it lights what it has at its next step,
/// as a restored region does, which gives the same light (a function of the blocks). It
/// keeps whether its level has a sky, which is the level's and not the light's.
impl Clone for RegionLight {
    fn clone(&self) -> Self {
        Self::new(self.sky)
    }
}

/// Levels kept from elsewhere without their sky's: a level without a sky has none to
/// adopt, and the arrays would only be carried until the adoption ignored them.
fn without_sky(kept: &mut Kept) {
    let sections = kept.light.sky.len();
    if kept
        .light
        .sky
        .iter()
        .any(|section| *section != SectionLight::Absent)
    {
        kept.light.sky = vec![SectionLight::Absent; sections];
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
    /// The light some of the chunks loaded in this tick kept from a merge or a split.
    /// Where the region's level has no sky, their sky's levels are dropped as they come.
    pub(crate) fn take_kept_light(&mut self, kept: &[(ChunkPos, Kept)]) {
        self.light_for_level();
        let sky = self.light.sky;
        for (at, levels) in kept {
            let mut levels = levels.clone();
            if !sky {
                without_sky(&mut levels);
            }
            self.light.kept.insert(*at, levels);
        }
    }

    /// The light made again for the region's level where it was made for another: a
    /// region is made (`Region::restore`, and through it a split's part, a merge's
    /// survivor and a restore) with the light of a level with a sky, and a Nether
    /// region's has none. Whatever the engine knew is lit again, as after a restore; the
    /// levels kept for waiting chunks stay, without their sky's where there is none.
    fn light_for_level(&mut self) {
        let sky = self.config.dimension.rules().has_skylight;
        if self.light.sky == sky {
            return;
        }
        let mut kept = std::mem::take(&mut self.light.kept);
        if !sky {
            kept.values_mut().for_each(without_sky);
        }
        self.light = RegionLight::new(sky);
        self.light.kept = kept;
    }

    /// The light step (the plan's 4.1 and 4.5): the chunks that went are taken out, those
    /// replaced whole relit, the changes noted in the chunks drained into the engine, the
    /// waiting chunks taken within the budget nearest the players first, and a run.
    pub(crate) fn light_step(&mut self) {
        self.sync_light();
        self.take_waiting();
        self.run_light();
        self.check_light();
    }

    /// The light of the region's loaded chunks to hand on with them at a merge or a
    /// split, the last tick's changes lit first (ADR-0029, section 7): what the runner
    /// keeps beside each chunk it keeps warm, and gives back in
    /// [`crate::api::TickInputs::chunks_kept`] with it.
    pub fn hand_over_light(&mut self) -> Vec<(ChunkPos, Kept)> {
        self.sync_light();
        self.run_light();
        let taken = (self.seed(), self.tick);
        self.chunks
            .keys()
            .filter_map(|at| Some((*at, self.light.engine.kept(*at, taken)?)))
            .collect()
    }

    /// The engine told what went, what was replaced whole and what changed; what came
    /// waits.
    fn sync_light(&mut self) {
        self.light_for_level();
        let present: BTreeSet<ChunkPos> = self
            .chunks
            .keys()
            .chain(self.copies.keys())
            .copied()
            .collect();
        let touching = self.config.light.checks == LightChecks::Touched;
        let light = &mut self.light;
        let gone: Vec<ChunkPos> = light.known.difference(&present).copied().collect();
        for chunk in gone {
            light.known.remove(&chunk);
            light.engine.left(chunk);
            if touching {
                light.touched.extend(around(chunk));
            }
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
                if touching {
                    light.touched.extend(around(*at));
                    light.touched.insert(*at);
                }
            } else {
                if touching && !changes.is_empty() {
                    light.touched.extend(around(*at));
                    light.touched.insert(*at);
                }
                for changed in changes {
                    light.engine.changed(BlockPos::new(
                        at.x * 16 + i32::from(changed.x),
                        i32::from(changed.y),
                        at.z * 16 + i32::from(changed.z),
                    ));
                }
            }
        }
    }

    /// The waiting chunks the budget allows, nearest the region's players first, then by
    /// position: one with light kept from a merge or a split adopted, the rest lit whole.
    fn take_waiting(&mut self) {
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
        let light = &mut self.light;
        let mut order: Vec<ChunkPos> = light.waiting.iter().copied().collect();
        order.sort_by_key(|chunk| (distance(chunk), *chunk));
        let mut left = self.config.light.budget;
        for chunk in order {
            let kept = light.kept.remove(&chunk);
            let cost = if kept.is_some() { ADOPTED } else { WHOLE };
            if let Some(units) = &mut left {
                if *units < cost {
                    if let Some(kept) = kept {
                        light.kept.insert(chunk, kept);
                    }
                    break;
                }
                *units -= cost;
            }
            light.waiting.remove(&chunk);
            light.known.insert(chunk);
            if self.config.light.checks == LightChecks::Touched {
                light.touched.extend(around(chunk));
                light.touched.insert(chunk);
            }
            match kept {
                Some(kept) => light.engine.adopt(chunk, kept),
                None => light.engine.arrived(chunk),
            }
        }
        let waiting = &light.waiting;
        light.kept.retain(|chunk, _| waiting.contains(chunk));
    }

    /// A run of the engine over the region's chunks.
    fn run_light(&mut self) {
        self.light.engine.run(&Present {
            chunks: &self.chunks,
            copies: &self.copies,
        });
    }

    /// The check of the plan's 4.7: each chosen chunk's light, as the engine holds it,
    /// equal to `clustine_light::light` of the chunk with those of its neighbours the
    /// engine has (the neighbours its light can come from), or a panic that names it.
    fn check_light(&mut self) {
        let chosen: Vec<ChunkPos> = match self.config.light.checks {
            LightChecks::Off => return,
            LightChecks::Touched => std::mem::take(&mut self.light.touched)
                .into_iter()
                .collect(),
            LightChecks::InTurn(count) => {
                let lit: Vec<ChunkPos> = self
                    .light
                    .known
                    .iter()
                    .copied()
                    .filter(|at| self.light.engine.is_lit(*at))
                    .collect();
                let from = self.light.checked;
                let start = lit
                    .iter()
                    .position(|at| from.is_none_or(|from| *at > from))
                    .unwrap_or(0);
                let chosen: Vec<ChunkPos> = lit
                    .iter()
                    .cycle()
                    .skip(start)
                    .take((count as usize).min(lit.len()))
                    .copied()
                    .collect();
                self.light.checked = chosen.last().copied().or(from);
                chosen
            }
        };
        let present = Present {
            chunks: &self.chunks,
            copies: &self.copies,
        };
        let engine = &self.light.engine;
        for at in chosen {
            if !engine.is_lit(at) {
                continue;
            }
            let Some(centre) = present.chunk(at) else {
                continue;
            };
            let neighbours =
                around(at).map(|near| engine.has(near).then(|| present.chunk(near)).flatten());
            let expected = clustine_light::light(&Around {
                centre,
                neighbours,
                sky: self.light.sky,
            });
            let made = engine.light(at, &present);
            assert!(
                made.as_ref() == Some(&expected),
                "the light of {at:?} differs from the function's of its lit 3×3 after the \
                 light step of tick {} (the plan of N8, 4.7): the engine was not told of a \
                 change, or lit it wrongly",
                self.tick
            );
        }
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

    /// `getEffectiveSkyBrightness`: the sky's light less `darken`, at least 0, as the
    /// daylight detector reads it; `None` where the chunk is not lit.
    pub(crate) fn effective_sky_brightness(&self, pos: BlockPos, darken: i32) -> Option<i32> {
        let light = self.light_at(pos)?;
        Some((i32::from(light.sky) - darken).max(0))
    }

    /// `getMaxLocalRawBrightness(pos)`: [`Self::raw_brightness`] darkened by the night as
    /// the tick has it, and 15 beyond the world's border of ±30,000,000 (n7 LevelReader
    /// 297–330). Grass reads it, so that it spreads by day or by a torch.
    pub(crate) fn max_local_raw_brightness(&self, pos: BlockPos) -> Option<u8> {
        const BORDER: i32 = 30_000_000;
        if !(-BORDER..BORDER).contains(&pos.x) || !(-BORDER..BORDER).contains(&pos.z) {
            return Some(15);
        }
        self.raw_brightness(pos, self.sky_darken())
    }

    /// `getBrightness(LightLayer.BLOCK, pos)`: the blocks' light alone, which melts snow
    /// and ice; the sun melts nothing.
    pub(crate) fn block_brightness(&self, pos: BlockPos) -> Option<u8> {
        Some(self.light_at(pos)?.block)
    }
}

/// The eight chunks around `at`, in the order of `Around::neighbours` (north-west first,
/// row by row, without the centre).
fn around(at: ChunkPos) -> [ChunkPos; 8] {
    [
        ChunkPos::new(at.x - 1, at.z - 1),
        ChunkPos::new(at.x, at.z - 1),
        ChunkPos::new(at.x + 1, at.z - 1),
        ChunkPos::new(at.x - 1, at.z),
        ChunkPos::new(at.x + 1, at.z),
        ChunkPos::new(at.x - 1, at.z + 1),
        ChunkPos::new(at.x, at.z + 1),
        ChunkPos::new(at.x + 1, at.z + 1),
    ]
}

#[cfg(test)]
#[path = "light_dimension_tests.rs"]
pub(super) mod dimension_tests;
