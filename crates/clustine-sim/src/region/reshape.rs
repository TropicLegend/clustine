//! Merging and splitting: one region takes another in, or a part of it becomes a region
//! of its own. See `docs/adr/0014-merging-and-splitting.md`, section 2.
//!
//! Each is one tick of the region in which nothing else happens, in two steps. The
//! first works out what would be and changes nothing ([`Region::absorb`],
//! [`Region::split`]), so that the world store can be asked to write it down; the
//! second, when the store has answered that it is on disk, makes the region that
//! ([`Region::take_absorbed`], [`Region::take_split`]). A region that plans and does
//! not take is the region it was.
//!
//! After the second step a region is what [`Region::restore`] makes of its new state
//! and of what it holds, and so is the region a split makes: nothing is loaded or
//! subscribed to, and whatever was asked or believed of other regions' chunks is asked
//! again when it is wanted. So a region that is run on from memory and one that another
//! worker restores from the store's record do the same.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use clustine_world::{Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId};

use super::{Holdings, Known, Region};
use crate::api::Durable;
use crate::state::{EdgeState, PlayerState, RegionState};

/// Why a region is not split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSplit {
    /// No player stands in a chunk named that the region holds and that is not the
    /// home chunk.
    Nobody,
    /// Nobody would stay, and the region would be left with nothing.
    NothingStays,
}

/// A split that has been worked out and not taken: what [`Region::split`] gives the
/// world store to write down, and [`Region::take_split`] makes of the region.
#[derive(Debug, Clone, PartialEq)]
pub struct Splitting {
    /// This region's whole state after the split, as of the tick of the split.
    pub state: RegionState,
    /// The new region's whole state, as of the same tick.
    pub part: RegionState,
    /// The chunks the new region holds, ascending.
    pub chunks: Vec<ChunkPos>,
    /// Where the line of this split is.
    pub sides: Sides,
}

/// Where a split put its line: the chunks in which those stood who went, and the
/// chunks in which those stood who stayed, with the home chunk if the region held
/// it. Both ascending.
///
/// It says more than the chunks that went do: of a chunk that neither region held at
/// the split, too, on whose side it lies. See
/// `docs/adr/0017-the-end-of-the-stripes.md`, section 3.6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sides {
    /// The chunks in which those stood who went.
    pub seeds: Vec<ChunkPos>,
    /// The chunks in which those stood who stayed, and the home chunk if the region
    /// held it, whether anyone stood there or not: only the home region is joined.
    pub staying: Vec<ChunkPos>,
}

impl Sides {
    /// Whether the chunk at `position` is on the part's side: nearer to a seed than
    /// to every chunk of `staying`, and any chunk if nothing stays. Distance is
    /// counted in chunks along the longer of the two axes, and a chunk that is as
    /// near to the one as to the other is not on the part's side.
    pub fn goes(&self, position: ChunkPos) -> bool {
        let nearest = |to: &[ChunkPos]| to.iter().map(|to| distance(position, *to)).min();
        match nearest(&self.staying) {
            Some(to_stay) => nearest(&self.seeds).is_some_and(|to_go| to_go < to_stay),
            None => true,
        }
    }
}

/// The region a split has made.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// The new region, as any worker would restore it from the store's record.
    pub region: Region,
    /// The chunks of the part that were loaded, as they were, in ascending order: what
    /// the store has of each, since nothing was unsaved.
    pub chunks: Vec<(ChunkPos, Chunk)>,
}

/// The block of entity ids of a region that a split made: none, which is what the
/// world store says of such a region. It refuses every join; only the home region is
/// joined, and that is never the part of a split.
const NO_ENTITY_IDS: EntityIds = EntityIds {
    first: EntityId(0),
    end: EntityId(0),
};

/// How far two chunks are apart: in chunks along the longer of the two axes. In 64
/// bits, as the difference of two coordinates does not fit into 32.
fn distance(from: ChunkPos, to: ChunkPos) -> i64 {
    let along = |from: i32, to: i32| (i64::from(from) - i64::from(to)).abs();
    along(from.x, to.x).max(along(from.z, to.z))
}

/// An edge as a region knows it from the tick `since` on, with nothing applied or sent.
fn noted(start: u64, since: u64) -> EdgeState {
    EdgeState {
        start,
        since,
        ..EdgeState::default()
    }
}

impl Region {
    /// The chunks the region holds, in ascending order.
    fn held(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        let held = |(position, known): (&ChunkPos, &Known)| {
            matches!(known, Known::Held { .. }).then_some(*position)
        };
        self.land.known.iter().filter_map(held)
    }

    /// The chunks the region holds for a split: by what its ticks were told, or by a
    /// grant of `granted`, which the world store made in answer to a claim that no
    /// tick has been told of. The store's table has the one as it has the other.
    fn held_with(&self, granted: &[ChunkPos]) -> BTreeSet<ChunkPos> {
        self.held().chain(granted.iter().copied()).collect()
    }

    /// The whole state this region would have after absorbing `absorbed`, whose state
    /// is `other`, as of the tick after this region's last. Changes nothing.
    ///
    /// The players of both are in it. Of a player both have, the later stay stays,
    /// which is the one with the higher entity id. The entity ids to give out are this
    /// region's; the absorbed region's block is never used again.
    ///
    /// Every edge either region knows is known with the higher of its starts, and the
    /// side that knew a lower one is reset as a higher start resets a region: its
    /// players of that edge are not in the state, and its outbox for the edge is
    /// dropped. An edge only the other region knew is known since this tick, with
    /// nothing applied or sent. Each edge's outbox then gains a [`Durable::Absorbed`]
    /// and, behind it, what the other region had in its outbox for the edge, in order
    /// and under the next numbers. Nothing of an entry is rewritten: one that names the
    /// absorbed region stays as it is.
    ///
    /// No event says what the merge did. Its tick closes every link, and what is not as
    /// it was is put right by what answers the next hello, as after a restore.
    pub fn absorb(&self, absorbed: RegionId, other: &RegionState) -> RegionState {
        let mut state = self.state();
        state.tick += 1;
        let tick = state.tick;

        // The other's side of every edge that it knows and that was not reset away.
        let mut theirs: BTreeMap<EdgeId, &EdgeState> = BTreeMap::new();
        for (id, b) in &other.edges {
            match state.edges.get(id).map(|a| a.start) {
                // It knew a lower start: whatever it kept for the edge, the edge has
                // forgotten. It counts as not knowing the edge from here on.
                Some(start) if b.start < start => continue,
                Some(start) if start == b.start => {}
                // This region knew a lower start or none. Its players of the edge, if
                // it had any, were of a start that is over.
                Some(_) | None => {
                    state.players.retain(|_, player| player.edge != *id);
                    state.edges.insert(*id, noted(b.start, tick));
                }
            }
            theirs.insert(*id, b);
        }

        for (id, player) in &other.players {
            if !theirs.contains_key(&player.edge) {
                continue;
            }
            // Of two stays of a player the one with the higher entity id is the later.
            // One id under two players cannot be, and is not looked for.
            let later = |present: &PlayerState| present.entity_id >= player.entity_id;
            if !state.players.get(id).is_some_and(later) {
                state.players.insert(*id, player.clone());
            }
        }

        // An entry for every edge, also for one that only one of the two knew: the edge
        // may keep things for the other region, and hears here that it is no more.
        for (id, a) in &mut state.edges {
            let b = theirs.get(id);
            a.sent += 1;
            let entry = Durable::Absorbed {
                region: absorbed,
                since: b.map_or(0, |b| b.since),
                applied: b.map_or(0, |b| b.applied),
                numbers: b.map_or_else(Vec::new, |b| b.outbox.keys().copied().collect()),
            };
            a.outbox.insert(a.sent, entry);
            for entry in b.into_iter().flat_map(|b| b.outbox.values()) {
                a.sent += 1;
                a.outbox.insert(a.sent, entry.clone());
            }
        }
        state
    }

    /// Makes the region what [`Region::restore`] makes of `state`, of the chunks it
    /// holds and `chunks`, and of its pinned areas and `pinned`, and returns the chunks
    /// that were loaded, in ascending order. `state` is what [`Region::absorb`] gave
    /// for this very region, with no tick in between; `chunks` and `pinned` are what
    /// came with the merge, as the world store says.
    ///
    /// So the region knows the areas that came with the merge, and forgets everything
    /// it believed of other regions and everything it had asked: not only of the region
    /// it absorbed, as a belief in a region that went into that one earlier would stand
    /// against the store's table with nothing to doubt it. It has no ticket and no
    /// loaded chunk, the links being closed with this tick, and the time before a
    /// chunk is given back starts anew.
    pub fn take_absorbed(
        &mut self,
        state: RegionState,
        chunks: &[ChunkPos],
        pinned: &[ChunkArea],
    ) -> Vec<(ChunkPos, Chunk)> {
        let held = self.held().chain(chunks.iter().copied()).collect();
        let mut areas = mem::take(&mut self.land.pinned);
        areas.extend_from_slice(pinned);
        let loaded = mem::take(&mut self.chunks).into_iter().collect();
        let holdings = Holdings {
            held,
            pinned: areas,
        };
        *self = Self::restore(self.config.clone(), state, holdings);
        loaded
    }

    /// What this region and a new region `part` would be if the players standing in
    /// `named` were split off, or why the split is off. Changes nothing. `granted`
    /// are the chunks the world store has granted the region in answer to claims
    /// that no tick has been told of; each counts as a chunk the region holds.
    ///
    /// Those go who stand in a chunk of `named` that the region holds and that is not
    /// the home chunk; everyone else stays. With them goes every chunk the region holds
    /// that is nearer to one of theirs than to any chunk a player who stays stands in,
    /// and than to the home chunk if the region holds it, counted in chunks along the
    /// longer of the two axes. A chunk that is as near to the one as to the other
    /// stays. Chunks of the region's own pinned areas go like any other; the region
    /// stays pinned to its areas.
    ///
    /// A grant that waits goes by the same rule, and is not left to the region for
    /// want of a tick that was told of it. A player who walks on has the row of chunks
    /// that just came into view granted so, and would find it ahead of them as land of
    /// the region they were split off; and one who stands in such a chunk would stay
    /// behind alone. See `docs/adr/0017-the-end-of-the-stripes.md`, section 3.6.1.
    ///
    /// The part's state has the players who go as they are, no entity ids to give out,
    /// and of the edges only those of its players, each known since this tick with the
    /// start this region knows and nothing applied or sent. This region's state tells
    /// each of those edges with a [`Durable::SplitOff`] which of its stays went.
    pub fn split(
        &self,
        named: &[ChunkPos],
        part: RegionId,
        granted: &[ChunkPos],
    ) -> Result<Splitting, NoSplit> {
        let home = self.land.home;
        let held = self.held_with(granted);
        let standing: BTreeSet<ChunkPos> =
            self.players.values().map(|player| player.chunk()).collect();
        let seeds: BTreeSet<ChunkPos> = named
            .iter()
            .filter(|position| {
                held.contains(*position) && **position != home && standing.contains(*position)
            })
            .copied()
            .collect();
        if seeds.is_empty() {
            return Err(NoSplit::Nobody);
        }

        // Where those are who stay; and the home chunk counts as a place where somebody
        // stays, whether anyone stands there or not: only the home region is joined.
        let mut staying: BTreeSet<ChunkPos> = standing.difference(&seeds).copied().collect();
        if staying.is_empty() && !held.contains(&home) && self.land.pinned.is_empty() {
            // The region would only get a new name.
            return Err(NoSplit::NothingStays);
        }
        if held.contains(&home) {
            staying.insert(home);
        }
        let sides = Sides {
            seeds: seeds.iter().copied().collect(),
            staying: staying.into_iter().collect(),
        };
        // With nobody and nothing to stay near, every chunk goes. A seed is among the
        // chunks in any case, and the home chunk never is.
        let chunks: Vec<ChunkPos> = held
            .into_iter()
            .filter(|position| sides.goes(*position))
            .collect();

        let mut state = self.state();
        state.tick += 1;
        let tick = state.tick;
        let mut new = RegionState::new(NO_ENTITY_IDS);
        new.tick = tick;
        // The stays that go, by the edge they are of: ascending, as the players are.
        let mut went: BTreeMap<EdgeId, Vec<(PlayerId, EntityId)>> = BTreeMap::new();
        for (id, player) in &self.players {
            if seeds.contains(&player.chunk())
                && let Some(player) = state.players.remove(id)
            {
                went.entry(player.edge)
                    .or_default()
                    .push((*id, player.entity_id));
                new.players.insert(*id, player);
            }
        }
        for (id, players) in went {
            let edge = state
                .edges
                .get_mut(&id)
                .expect("a player belongs to an edge the region knows");
            new.edges.insert(id, noted(edge.start, tick));
            edge.sent += 1;
            let entry = Durable::SplitOff {
                region: part,
                players,
            };
            edge.outbox.insert(edge.sent, entry);
        }
        Ok(Splitting {
            state,
            part: new,
            chunks,
            sides,
        })
    }

    /// Makes the region what [`Region::restore`] makes of `splitting.state`, of the
    /// chunks it holds and those of `granted`, without the part's, and of its pinned
    /// areas; returns the chunks that were loaded and stay, in ascending order, and the
    /// part. `splitting` is what [`Region::split`] gave for this very region, with no
    /// tick in between, and `granted` has to be what `split` was given: the chunks the
    /// world store has granted the region in answer to claims that no tick has been
    /// told of. Those of them that are on the part's side are among
    /// `splitting.chunks`; the others are the region's from here on.
    ///
    /// So the region knows nothing of the part's chunks, nor of any other region's, and
    /// asks again for what it wants. The part is restored with this region's
    /// configuration, holds its chunks and is pinned to nothing.
    pub fn take_split(
        &mut self,
        splitting: Splitting,
        granted: &[ChunkPos],
    ) -> (Vec<(ChunkPos, Chunk)>, Part) {
        let Splitting {
            state,
            part,
            chunks,
            sides: _,
        } = splitting;
        let gone: BTreeSet<ChunkPos> = chunks.iter().copied().collect();
        let held = self
            .held_with(granted)
            .into_iter()
            .filter(|position| !gone.contains(position))
            .collect();
        let pinned = mem::take(&mut self.land.pinned);
        let (theirs, kept): (Vec<_>, Vec<_>) = mem::take(&mut self.chunks)
            .into_iter()
            .partition(|(position, _)| gone.contains(position));
        let config = self.config.clone();
        let of_part = Holdings {
            held: chunks,
            pinned: Vec::new(),
        };
        let region = Self::restore(config.clone(), part, of_part);
        *self = Self::restore(config, state, Holdings { held, pinned });
        (
            kept,
            Part {
                region,
                chunks: theirs,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_world::{Biome, Vec3};

    use super::*;
    use crate::api::{
        HOTBAR_SLOTS, ItemStack, PlayerChange, PlayerEvent, PlayerInput, PlayerJoin,
        PlayerTransfer, Pose, RegionEvent, TickInputs, TickOutput, Ticket,
    };
    use crate::region::{Knowledge, RegionConfig};

    /// The spawn point is in the chunk at the origin, which is the home chunk.
    const SPAWN: Vec3 = Vec3::new(8.5, 64.0, 8.5);
    const HOME: ChunkPos = ChunkPos::new(0, 0);

    /// The region that is absorbed, and the one a split makes.
    const ABSORBED: RegionId = RegionId(1);
    const PART: RegionId = RegionId(2);

    const E: EdgeId = EdgeId(1);
    const F: EdgeId = EdgeId(2);

    fn config(return_after: u64) -> RegionConfig {
        RegionConfig {
            spawn: SPAWN,
            starting_hotbar: [None; HOTBAR_SLOTS],
            return_after,
        }
    }

    fn player(number: u128) -> PlayerId {
        PlayerId(uuid::Uuid::from_u128(number))
    }

    fn at(x: i32, z: i32) -> ChunkPos {
        ChunkPos::new(x, z)
    }

    /// The middle of the chunk at `position`, on the ground.
    fn middle(position: ChunkPos) -> Vec3 {
        Vec3::new(
            f64::from(position.x) * 16.0 + 8.5,
            64.0,
            f64::from(position.z) * 16.0 + 8.5,
        )
    }

    /// A stay with the entity `entity` under `edge`, standing in the middle of `chunk`.
    /// Everything else a region keeps of a player follows from the entity, so that no
    /// two stays are alike in anything.
    fn stay(entity: i32, edge: EdgeId, chunk: ChunkPos) -> PlayerState {
        let mut hotbar = [None; HOTBAR_SLOTS];
        hotbar[entity as usize % HOTBAR_SLOTS] = Some(ItemStack {
            item: 1,
            count: entity,
        });
        PlayerState {
            entity_id: EntityId(entity),
            name: format!("Stay{entity}"),
            pose: Pose {
                position: middle(chunk),
                yaw: entity as f32,
                pitch: -10.0,
                on_ground: true,
            },
            hotbar,
            selected_slot: (entity % 9) as u8,
            last_input: entity as u64 * 3,
            handled: Some(entity * 7),
            edge,
        }
    }

    /// The stay as a region hands it to another.
    fn transfer_of(stay: PlayerState) -> PlayerTransfer {
        PlayerTransfer {
            entity_id: stay.entity_id,
            name: stay.name,
            pose: stay.pose,
            hotbar: stay.hotbar,
            selected_slot: stay.selected_slot,
            last_input: stay.last_input,
        }
    }

    /// An outbox entry that can be told from any other by `number`.
    fn entry(number: i32) -> Durable {
        Durable::RemoteDone {
            player: player(900),
            sequence: number,
        }
    }

    fn edge(start: u64, since: u64, applied: u64, sent: u64, outbox: &[(u64, i32)]) -> EdgeState {
        EdgeState {
            start,
            since,
            applied,
            sent,
            outbox: outbox
                .iter()
                .map(|(number, of)| (*number, entry(*of)))
                .collect(),
        }
    }

    /// A state after `tick` ticks with the block of entity ids `block`, of which
    /// `given` are given out.
    fn state(
        tick: u64,
        block: u32,
        given: i32,
        players: Vec<(u128, PlayerState)>,
        edges: Vec<(EdgeId, EdgeState)>,
    ) -> RegionState {
        let entity_ids = EntityIds::block(block).unwrap();
        RegionState {
            tick,
            entity_ids,
            next_entity_id: EntityId(entity_ids.first.0 + given),
            players: players
                .into_iter()
                .map(|(number, state)| (player(number), state))
                .collect(),
            edges: edges.into_iter().collect(),
        }
    }

    /// A region with `state` that gives chunks back after three ticks without use.
    fn region(state: RegionState, held: &[ChunkPos], pinned: &[ChunkArea]) -> Region {
        Region::restore(config(3), state, holdings(held, pinned))
    }

    fn holdings(held: &[ChunkPos], pinned: &[ChunkArea]) -> Holdings {
        Holdings {
            held: held.to_vec(),
            pinned: pinned.to_vec(),
        }
    }

    fn area(min_x: Option<i32>, max_x: Option<i32>) -> ChunkArea {
        ChunkArea { min_x, max_x }
    }

    /// A chunk with one block of stone, at a place that `mark` says.
    fn chunk(mark: usize) -> Chunk {
        let overworld = clustine_data::DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        let mut chunk = Chunk::empty(overworld, Biome(0));
        chunk.set(mark, 63, 0, blocks::STONE).unwrap();
        chunk
    }

    fn viewers(chunks: &[ChunkPos]) -> Vec<(ChunkPos, Ticket)> {
        chunks
            .iter()
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect()
    }

    fn idle(region: &mut Region) -> TickOutput {
        region.tick(&TickInputs::default())
    }

    fn bytes(state: &RegionState) -> Vec<u8> {
        postcard::to_stdvec(state).unwrap()
    }

    /// A known edge with nothing in its outbox.
    fn quiet(start: u64) -> EdgeState {
        edge(start, 1, 0, 0, &[])
    }

    #[test]
    fn a_merged_state_has_the_players_of_both_and_the_later_of_two_stays() {
        let here = state(
            10,
            0,
            6,
            vec![
                (1, stay(1, E, at(0, 0))),
                (2, stay(5, E, at(0, 1))),
                (3, stay(3, F, at(1, 1))),
            ],
            vec![(E, quiet(4)), (F, quiet(4))],
        );
        let there = state(
            99,
            1,
            2,
            vec![
                (2, stay(4, F, at(3, 0))),
                (3, stay(6, E, at(3, 1))),
                (4, stay(2, F, at(4, 0))),
            ],
            vec![(E, quiet(4)), (F, quiet(4))],
        );
        let survivor = region(here.clone(), &[HOME], &[]);
        let merged = survivor.absorb(ABSORBED, &there);

        // The tick is the survivor's next, whatever the other's was, and the entity ids
        // to give out are the survivor's.
        assert_eq!(merged.tick, 11);
        assert_eq!(merged.entity_ids, here.entity_ids);
        assert_eq!(merged.next_entity_id, here.next_entity_id);
        // Everyone is there as they were. Of player 2 the survivor has the later stay,
        // of player 3 the other, under another edge.
        let expected: BTreeMap<_, _> = [
            (player(1), here.players[&player(1)].clone()),
            (player(2), here.players[&player(2)].clone()),
            (player(3), there.players[&player(3)].clone()),
            (player(4), there.players[&player(4)].clone()),
        ]
        .into();
        assert_eq!(merged.players, expected);

        // Working it out changed nothing, and gives the same every time, to the byte.
        assert_eq!(survivor, region(here, &[HOME], &[]));
        assert_eq!(bytes(&survivor.absorb(ABSORBED, &there)), bytes(&merged));

        // A join after the merge gets the id it would have got without it.
        let mut merged_region = survivor.clone();
        merged_region.take_absorbed(merged.clone(), &[], &[]);
        let join = PlayerChange::Join(
            E,
            PlayerJoin {
                player: player(8),
                name: "Late".to_owned(),
            },
        );
        let output = merged_region.tick(&TickInputs {
            player_changes: vec![join],
            ..TickInputs::default()
        });
        assert!(matches!(
            output.player_events.as_slice(),
            [(_, PlayerEvent::Spawned { entity_id, .. })] if *entity_id == merged.next_entity_id
        ));
    }

    #[test]
    fn a_merged_state_knows_every_edge_either_knew_with_the_higher_start() {
        // Five edges: one both know with one start, one only the survivor knows, one
        // only the other knows, one the other knows with a lower start, and one the
        // survivor knows with a lower start. Each has a player on either side that
        // knows it.
        let [same, ours, theirs, stale_there, stale_here] = [10, 20, 30, 40, 50].map(EdgeId);
        let here = state(
            7,
            0,
            9,
            vec![
                (1, stay(1, same, at(0, 0))),
                (2, stay(2, ours, at(0, 0))),
                (4, stay(4, stale_there, at(0, 0))),
                (5, stay(5, stale_here, at(0, 0))),
            ],
            vec![
                (same, edge(3, 4, 20, 7, &[(6, 106), (7, 107)])),
                (ours, edge(1, 1, 5, 2, &[])),
                (stale_there, edge(5, 2, 8, 4, &[(4, 104)])),
                (stale_here, edge(2, 2, 9, 6, &[(5, 105), (6, 106)])),
            ],
        );
        let there = state(
            3,
            1,
            9,
            vec![
                (11, stay(11, same, at(5, 0))),
                (13, stay(13, theirs, at(5, 0))),
                (14, stay(14, stale_there, at(5, 0))),
                (15, stay(15, stale_here, at(5, 0))),
            ],
            vec![
                (same, edge(3, 2, 11, 5, &[(2, 202), (5, 205)])),
                (theirs, edge(6, 3, 4, 3, &[(3, 203)])),
                (stale_there, edge(2, 1, 30, 9, &[(9, 209)])),
                (stale_here, edge(5, 1, 12, 8, &[(7, 207), (8, 208)])),
            ],
        );
        let merged = region(here.clone(), &[HOME], &[]).absorb(ABSORBED, &there);
        let absorbed = |since, applied, numbers: &[u64]| Durable::Absorbed {
            region: ABSORBED,
            since,
            applied,
            numbers: numbers.to_vec(),
        };
        let with = |mut edge: EdgeState, entries: Vec<(u64, Durable)>| {
            edge.sent = entries.last().unwrap().0;
            edge.outbox.extend(entries);
            edge
        };

        // One start in both: the survivor's side is as it was, the entry is next, and
        // the other's entries follow under the next numbers, with their old ones named.
        assert_eq!(
            merged.edges[&same],
            with(
                here.edges[&same].clone(),
                vec![
                    (8, absorbed(2, 11, &[2, 5])),
                    (9, entry(202)),
                    (10, entry(205)),
                ],
            )
        );
        // Known to the survivor only: an entry that says the other knew nothing.
        assert_eq!(
            merged.edges[&ours],
            with(here.edges[&ours].clone(), vec![(3, absorbed(0, 0, &[]))])
        );
        // Known to the other only: known since this tick, with nothing applied, and
        // the other's entries from number 2.
        assert_eq!(
            merged.edges[&theirs],
            with(
                edge(6, 8, 0, 0, &[]),
                vec![(1, absorbed(3, 4, &[3])), (2, entry(203))]
            )
        );
        // The other's start is lower: as for an edge it did not know.
        assert_eq!(
            merged.edges[&stale_there],
            with(
                here.edges[&stale_there].clone(),
                vec![(5, absorbed(0, 0, &[]))]
            )
        );
        // The survivor's start is lower: its side begins anew, and then as for an edge
        // only the other knows.
        assert_eq!(
            merged.edges[&stale_here],
            with(
                edge(5, 8, 0, 0, &[]),
                vec![
                    (1, absorbed(1, 12, &[7, 8])),
                    (2, entry(207)),
                    (3, entry(208)),
                ],
            )
        );
        assert_eq!(merged.edges.len(), 5);

        // The players of an edge are there unless their side of it was reset.
        let stays: Vec<i32> = merged
            .players
            .values()
            .map(|player| player.entity_id.0)
            .collect();
        assert_eq!(stays, [1, 2, 4, 11, 13, 15]);
        // A player of the survivor whose edge was reset makes way for the other's stay
        // of them, though its entity id is lower: theirs was of a start that is over.
        let mut here = here;
        here.players.insert(player(15), stay(99, stale_here, HOME));
        let merged = region(here, &[HOME], &[]).absorb(ABSORBED, &there);
        assert_eq!(merged.players[&player(15)], stay(15, stale_here, at(5, 0)));
    }

    /// A region pinned to the chunks with x below 2 that has come to know a chunk in
    /// every way: it holds the home chunk and a chunk of its area that it claimed, both
    /// loaded; it believes one chunk the absorbed region's and one a third region's;
    /// and it has asked for one without an answer.
    fn knowing() -> (Region, [ChunkPos; 5]) {
        let chunks = [HOME, at(1, 0), at(2, 0), at(3, 0), at(5, 5)];
        let [home, claimed, of_absorbed, of_third, asked] = chunks;
        let own = area(None, Some(2));
        let here = state(0, 0, 0, vec![], vec![(E, quiet(1))]);
        let mut region = region(here, &[home], &[own]);
        let output = region.tick(&TickInputs {
            tickets_added: viewers(&chunks),
            ..TickInputs::default()
        });
        assert_eq!(output.claims, [claimed, of_absorbed, of_third, asked]);
        region.tick(&TickInputs {
            granted: vec![claimed],
            foreign: vec![(of_absorbed, ABSORBED), (of_third, RegionId(7))],
            chunks_loaded: vec![(home, chunk(1))],
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: vec![(claimed, chunk(2))],
            ..TickInputs::default()
        });
        let known = chunks.map(|chunk| region.knowledge(chunk));
        let expected = [
            Knowledge::Held,
            Knowledge::Held,
            Knowledge::Foreign(ABSORBED),
            Knowledge::Foreign(RegionId(7)),
            Knowledge::Asked,
        ];
        assert_eq!(known, expected);
        assert_eq!(region.loaded_chunk_count(), 2);
        (region, chunks)
    }

    #[test]
    fn a_region_that_takes_a_merge_is_the_region_restored_from_it() {
        let (mut survivor, [home, claimed, of_absorbed, of_third, asked]) = knowing();
        let there = state(
            40,
            1,
            0,
            vec![(3, stay(3, E, of_absorbed))],
            vec![(E, quiet(1))],
        );
        let merged = survivor.absorb(ABSORBED, &there);
        // What came with the merge, as the store says: the chunk the other was
        // granted, one far off, and the area it was pinned to.
        let (far, came) = (at(9, 9), area(Some(2), Some(6)));
        let loaded = survivor.take_absorbed(merged.clone(), &[of_absorbed, far], &[came]);

        let own = area(None, Some(2));
        let restored = Region::restore(
            config(3),
            merged.clone(),
            holdings(&[home, claimed, of_absorbed, far], &[own, came]),
        );
        assert_eq!(survivor, restored);
        assert_eq!(survivor.state(), merged);
        // The chunks that were loaded come back block for block, and none is loaded.
        assert_eq!(loaded, [(home, chunk(1)), (claimed, chunk(2))]);
        assert_eq!(survivor.loaded_chunk_count(), 0);
        // It holds what it held and what came, and has forgotten what it believed of
        // any region and what it had asked.
        for held in [home, claimed, of_absorbed, far] {
            assert_eq!(survivor.knowledge(held), Knowledge::Held, "{held:?}");
        }
        assert_eq!(survivor.held_chunk_count(), 4);
        for forgotten in [of_third, asked] {
            assert_eq!(survivor.knowledge(forgotten), Knowledge::Unknown);
        }
        assert!(survivor.pins(at(4, -3)) && !survivor.pins(at(6, 0)));

        // No ticket is left: a chunk it holds is not asked of storage, and a guest's
        // ticket on a chunk of the area that came makes it ask the store.
        let output = survivor.tick(&TickInputs {
            tickets_added: vec![(at(4, 0), Ticket::Guest)],
            ..TickInputs::default()
        });
        assert!(output.chunk_requests.is_empty());
        assert_eq!(output.claims, [at(4, 0)]);
        // The time before a return starts with the merge: the one chunk that nothing
        // uses and no area keeps goes with the fourth tick, and not before.
        for _ in 0..2 {
            assert!(idle(&mut survivor).returns.is_empty());
        }
        assert_eq!(idle(&mut survivor).returns, [far]);
        assert_eq!(survivor.tick_number(), merged.tick + 4);
    }

    #[test]
    fn a_region_that_plans_a_merge_or_a_split_and_does_not_take_it_ticks_on_as_before() {
        let (mut region, [.., of_absorbed, _, _]) = knowing();
        region.tick(&TickInputs {
            player_changes: vec![PlayerChange::Arrive(
                E,
                player(1),
                transfer_of(stay(1, E, at(1, 0))),
            )],
            ..TickInputs::default()
        });
        let mut untouched = region.clone();
        let there = state(
            40,
            1,
            0,
            vec![(3, stay(3, E, of_absorbed))],
            vec![(E, quiet(1))],
        );
        region.absorb(ABSORBED, &there);
        region.split(&[at(1, 0)], PART, &[]).unwrap();
        assert_eq!(region, untouched);
        let inputs = TickInputs {
            tickets_removed: viewers(&[of_absorbed]),
            inputs: vec![(
                E,
                player(1),
                EntityId(1),
                4,
                PlayerInput::Move {
                    position: Some(middle(HOME)),
                    rotation: None,
                    on_ground: true,
                },
            )],
            ..TickInputs::default()
        };
        assert_eq!(region.tick(&inputs), untouched.tick(&inputs));
        assert_eq!(region, untouched);
    }

    /// The chunks of a row from `from` to `to`, at `z`.
    fn row(from: i32, to: i32, z: i32) -> Vec<ChunkPos> {
        (from..=to).map(|x| at(x, z)).collect()
    }

    /// A region on open land that holds `held`, with the players standing in `stands`,
    /// each with the entity of their number, under `E`.
    fn standing(stands: &[(u128, ChunkPos)], held: &[ChunkPos], pinned: &[ChunkArea]) -> Region {
        let players = stands
            .iter()
            .map(|(number, chunk)| (*number, stay(*number as i32, E, *chunk)))
            .collect();
        region(state(20, 0, 9, players, vec![(E, quiet(1))]), held, pinned)
    }

    #[test]
    fn a_region_is_not_split_where_nobody_would_go_or_nothing_would_stay() {
        let off = |region: &Region, named: &[ChunkPos]| region.split(named, PART, &[]).unwrap_err();
        let held = [row(0, 3, 0), vec![at(9, 9)]].concat();
        let region = standing(&[(1, at(0, 0)), (2, at(3, 0)), (3, at(7, 0))], &held, &[]);

        // No chunk named, a chunk without a player, one with a player that is not held,
        // and the home chunk, in which a player stands.
        for named in [vec![], vec![at(2, 0)], vec![at(7, 0)], vec![HOME]] {
            assert_eq!(off(&region, &named), NoSplit::Nobody, "{named:?}");
        }
        // One such chunk beside one that will do is passed over.
        let split = region
            .split(&[HOME, at(7, 0), at(3, 0)], PART, &[])
            .unwrap();
        assert_eq!(split.part.players.len(), 1);

        // Everyone would go from a region that neither holds the home chunk nor is
        // pinned to anything: it would only get a new name.
        let away = [at(3, 0), at(4, 0)];
        let region = standing(&[(1, at(3, 0)), (2, at(4, 0))], &away, &[]);
        assert_eq!(off(&region, &away), NoSplit::NothingStays);
        // Not so if one of them stays, if the region holds the home chunk, or if it is
        // pinned to an area, be it one it holds nothing of.
        assert_eq!(
            region.split(&away[..1], PART, &[]).unwrap().chunks,
            [at(3, 0)]
        );
        let with_home = [away.as_slice(), &[HOME]].concat();
        let region = standing(&[(1, at(3, 0)), (2, at(4, 0))], &with_home, &[]);
        assert_eq!(region.split(&away, PART, &[]).unwrap().chunks, away);
        let pinned = [area(Some(40), None)];
        let region = standing(&[(1, at(3, 0)), (2, at(4, 0))], &away, &pinned);
        let split = region.split(&away, PART, &[]).unwrap();
        assert_eq!(split.chunks, away);
        assert!(split.state.players.is_empty());
    }

    #[test]
    fn the_chunks_nearer_to_those_who_go_than_to_those_who_stay_go_with_them() {
        // One player goes at (10, 0), and the origin is where somebody stays. Of the
        // row between them the chunks beyond the middle go; the middle, which is as
        // near to the one as to the other, stays, and so do chunks off the row that
        // are. Distance is counted along the longer axis.
        let sorted = |mut chunks: Vec<ChunkPos>| {
            chunks.sort();
            chunks
        };
        let off_the_row = vec![at(6, 7), at(8, 7), at(6, -8), at(41, 40)];
        let held = sorted([row(-2, 12, 0), off_the_row].concat());
        let going = sorted([row(6, 12, 0), vec![at(8, 7), at(41, 40)]].concat());
        // A region whose spawn point is far off, in a chunk it does not hold: the
        // origin is no home to it.
        let elsewhere = RegionConfig {
            spawn: middle(at(0, 90)),
            ..config(3)
        };
        let cases = [
            // A player stays at the origin, which is not the home chunk.
            (vec![(1, at(10, 0)), (2, at(0, 0))], elsewhere.clone()),
            // Nobody stays, and the home chunk at the origin does in their place.
            (vec![(1, at(10, 0))], config(3)),
        ];
        for (stands, config) in cases {
            let state = standing(&stands, &[], &[]).state();
            let region = Region::restore(config, state, holdings(&held, &[]));
            let split = region.split(&[at(10, 0)], PART, &[]).unwrap();
            assert_eq!(split.chunks, going, "{stands:?}");
            // Naming the chunk twice, or more chunks in which nobody stands, changes
            // nothing.
            let named = [at(11, 0), at(10, 0), at(-2, 0), at(10, 0)];
            assert_eq!(region.split(&named, PART, &[]).unwrap(), split);
        }

        // Nobody stays in a pinned region without the home chunk: every chunk it holds
        // goes, chunks of its area among them, and it stays pinned to the area.
        let state = standing(&[(1, at(10, 0))], &[], &[]).state();
        let pinned = [area(Some(-2), Some(9))];
        let mut region = Region::restore(elsewhere, state, holdings(&held, &pinned));
        let split = region.split(&[at(10, 0)], PART, &[]).unwrap();
        assert_eq!(split.chunks, held);
        region.take_split(split, &[]);
        assert_eq!(region.held_chunk_count(), 0);
        assert!(region.pins(at(0, 0)) && !region.pins(at(10, 0)));
    }

    #[test]
    fn those_standing_in_the_chunks_named_go_and_their_edges_are_told() {
        // Players 1 and 4 are of `E` and 2, 3 and 5 of `F`. Players 2, 4 and 5 stand in
        // the chunks named; player 3 stands in a chunk named that the region does not
        // hold.
        let players = vec![
            (1, stay(1, E, at(0, 0))),
            (2, stay(2, F, at(8, 0))),
            (3, stay(3, F, at(8, 5))),
            (4, stay(4, E, at(9, 0))),
            (5, stay(5, F, at(9, 0))),
        ];
        let edges = vec![
            (E, edge(3, 4, 20, 7, &[(7, 107)])),
            (F, edge(5, 2, 8, 0, &[])),
            // An edge without a player.
            (EdgeId(3), edge(1, 1, 0, 2, &[])),
        ];
        let before = state(20, 0, 9, players.clone(), edges.clone());
        let held = row(0, 9, 0);
        let region = region(before.clone(), &held, &[]);
        let named = [at(8, 0), at(8, 5), at(9, 0), at(4, 0)];
        let split = region.split(&named, PART, &[]).unwrap();

        // The part has those who go as they were, no entity ids, and their edges as of
        // this tick with the start the region knows.
        let (gone, stayed): (Vec<_>, Vec<_>) = players
            .into_iter()
            .partition(|(number, _)| [2, 4, 5].contains(number));
        let expected = RegionState {
            tick: 21,
            entity_ids: NO_ENTITY_IDS,
            next_entity_id: EntityId(0),
            players: gone
                .into_iter()
                .map(|(number, stay)| (player(number), stay))
                .collect(),
            edges: [(E, edge(3, 21, 0, 0, &[])), (F, edge(5, 21, 0, 0, &[]))].into(),
        };
        assert_eq!(split.part, expected);
        // The region has lost them, and tells each of their edges which stays went
        // under the next number. Everything else is as it was.
        let split_off = |stays: &[(u128, i32)]| Durable::SplitOff {
            region: PART,
            players: stays
                .iter()
                .map(|(number, entity)| (player(*number), EntityId(*entity)))
                .collect(),
        };
        let mut expected = state(21, 0, 9, stayed, edges);
        let told = expected.edges.get_mut(&E).unwrap();
        told.sent = 8;
        told.outbox.insert(8, split_off(&[(4, 4)]));
        let told = expected.edges.get_mut(&F).unwrap();
        told.sent = 1;
        told.outbox.insert(1, split_off(&[(2, 2), (5, 5)]));
        assert_eq!(split.state, expected);
        assert_eq!(split.chunks, row(5, 9, 0));

        // Working it out changed nothing, and gives the same every time, to the byte.
        assert_eq!(region.state(), before);
        let again = region.split(&named, PART, &[]).unwrap();
        assert_eq!(bytes(&again.state), bytes(&split.state));
        assert_eq!(bytes(&again.part), bytes(&split.part));
        assert_eq!(again.chunks, split.chunks);
    }

    /// So many ticks without use that no chunk is given back in these tests.
    const LONG: u64 = 1000;

    /// A region on open land that holds the row from the origin to (9, 0), with the
    /// chunks at its ends and in its middle loaded for viewers, a belief about a chunk
    /// beyond it, and two players of `E`: player 1 at home and player 2 at the far end.
    fn stretched() -> Region {
        let players = vec![(1, stay(1, E, HOME)), (2, stay(2, E, at(9, 0)))];
        let here = state(20, 0, 2, players, vec![(E, quiet(1)), (F, quiet(1))]);
        let mut region = Region::restore(config(LONG), here, holdings(&row(0, 9, 0), &[]));
        let watched = [HOME, at(4, 0), at(8, 0), at(9, 0), at(10, 0)];
        let output = region.tick(&TickInputs {
            tickets_added: viewers(&watched),
            ..TickInputs::default()
        });
        assert_eq!(output.chunk_requests, watched[..4]);
        region.tick(&TickInputs {
            foreign: vec![(at(10, 0), RegionId(7))],
            chunks_loaded: watched[..4]
                .iter()
                .map(|position| (*position, chunk(position.x as usize)))
                .collect(),
            ..TickInputs::default()
        });
        assert_eq!(region.loaded_chunk_count(), 4);
        region
    }

    #[test]
    fn a_region_that_takes_a_split_and_its_part_are_the_regions_restored_from_it() {
        let mut region = stretched();
        // What the store granted the region meanwhile, which no tick has been told of:
        // a chunk beside those who stay.
        let granted = [at(0, 3)];
        let split = region.split(&[at(9, 0)], PART, &granted).unwrap();
        assert_eq!(split.chunks, row(5, 9, 0));
        let (kept, part) = region.take_split(split.clone(), &granted);

        let stays = [row(0, 4, 0).as_slice(), &granted].concat();
        let restored = Region::restore(config(LONG), split.state.clone(), holdings(&stays, &[]));
        assert_eq!(region, restored);
        let restored = Region::restore(
            config(LONG),
            split.part.clone(),
            holdings(&split.chunks, &[]),
        );
        assert_eq!(part.region, restored);
        assert_eq!(
            (region.state(), part.region.state()),
            (split.state, split.part)
        );
        // The chunks that were loaded are divided between the two, block for block.
        assert_eq!(kept, [(HOME, chunk(0)), (at(4, 0), chunk(4))]);
        assert_eq!(part.chunks, [(at(8, 0), chunk(8)), (at(9, 0), chunk(9))]);
        assert_eq!(region.loaded_chunk_count(), 0);
        assert_eq!(part.region.loaded_chunk_count(), 0);
        // Neither knows anything of the other's chunks, or of anyone else's.
        assert_eq!(region.held_chunk_count(), 6);
        assert_eq!(part.region.held_chunk_count(), 5);
        for position in [at(5, 0), at(9, 0), at(10, 0)] {
            assert_eq!(region.knowledge(position), Knowledge::Unknown);
        }
        for position in [at(4, 0), at(0, 3), at(10, 0)] {
            assert_eq!(part.region.knowledge(position), Knowledge::Unknown);
        }
        assert!(!part.region.pins(at(9, 0)));
    }

    /// The chunks of `chunks` in ascending order, each once.
    fn ascending(chunks: &[ChunkPos]) -> Vec<ChunkPos> {
        let chunks: BTreeSet<ChunkPos> = chunks.iter().copied().collect();
        chunks.into_iter().collect()
    }

    /// A region as [`standing`] makes it that has claimed `asked` for viewers and has
    /// had no answer. What the store grants of them waits for the next tick, which a
    /// split comes before.
    fn asking(
        stands: &[(u128, ChunkPos)],
        held: &[ChunkPos],
        pinned: &[ChunkArea],
        asked: &[ChunkPos],
    ) -> Region {
        let mut region = standing(stands, held, pinned);
        region.tick(&TickInputs {
            tickets_added: viewers(asked),
            ..TickInputs::default()
        });
        for position in asked {
            assert_eq!(
                region.knowledge(*position),
                Knowledge::Asked,
                "{position:?}"
            );
        }
        region
    }

    /// Holds the two regions of a split that was taken to its line: each of `chunks`
    /// is the part's and unknown to the region if it is on the part's side, and the
    /// region's and unknown to the part if it is not.
    fn divided_by_the_line(sides: &Sides, region: &Region, part: &Region, chunks: &[ChunkPos]) {
        for position in chunks {
            let (ours, theirs) = if sides.goes(*position) {
                (Knowledge::Unknown, Knowledge::Held)
            } else {
                (Knowledge::Held, Knowledge::Unknown)
            };
            assert_eq!(region.knowledge(*position), ours, "{position:?}");
            assert_eq!(part.knowledge(*position), theirs, "{position:?}");
        }
        let held = region.held_chunk_count() + part.held_chunk_count();
        assert_eq!(held, ascending(chunks).len());
    }

    /// Player 1 at home and player 2 at (12, 0), each on four chunks of the row that
    /// the region holds.
    const APART: [(u128, ChunkPos); 2] = [(1, HOME), (2, ChunkPos::new(12, 0))];

    fn held_apart() -> Vec<ChunkPos> {
        [row(0, 3, 0), row(9, 12, 0)].concat()
    }

    #[test]
    fn a_grant_that_waits_goes_with_the_part_if_it_is_nearer_to_who_goes_and_stays_if_not() {
        // The region has asked for a row ahead of player 2, as a player has who walks
        // on, for one behind player 1, and for chunks between the two: one nearer to
        // home, one nearer to player 2, and two that are six chunks from both. The
        // store has granted them all, and no tick has been told.
        let ahead = [at(14, -1), at(14, 0), at(14, 1)];
        let behind = [at(-2, -1), at(-2, 0), at(-2, 1)];
        let (nearer_home, nearer_two, ties) = (at(5, 0), at(7, 0), [at(6, 0), at(6, 6)]);
        let between = [nearer_home, nearer_two, ties[0], ties[1]];
        let granted = [ahead, behind].concat().into_iter().chain(between);
        let granted: Vec<ChunkPos> = granted.collect();
        let mut region = asking(&APART, &held_apart(), &[], &granted);
        let untouched = region.clone();

        let split = region.split(&[at(12, 0)], PART, &granted).unwrap();
        let going = ascending(&[row(9, 12, 0).as_slice(), &ahead, &[nearer_two]].concat());
        assert_eq!(split.chunks, going);
        let line = Sides {
            seeds: vec![at(12, 0)],
            staying: vec![HOME],
        };
        assert_eq!(split.sides, line);
        // The players and what their edge is told are those of a split that was handed
        // no grant, which takes only what the ticks were told of.
        let plain = region.split(&[at(12, 0)], PART, &[]).unwrap();
        assert_eq!(plain.chunks, row(9, 12, 0));
        assert_eq!((&plain.state, &plain.part), (&split.state, &split.part));
        assert_eq!(plain.sides, line);
        assert_eq!(region, untouched);

        // Each of the two is what a restore makes of its state and of what the store
        // has for it once the split is written down.
        let (kept, part) = region.take_split(split.clone(), &granted);
        let staying = [row(0, 3, 0).as_slice(), &behind, &[nearer_home], &ties].concat();
        let restored = Region::restore(config(3), split.state.clone(), holdings(&staying, &[]));
        assert_eq!(region, restored);
        let restored = Region::restore(config(3), split.part.clone(), holdings(&going, &[]));
        assert_eq!(part.region, restored);
        assert!(kept.is_empty() && part.chunks.is_empty());
        let all = [held_apart(), granted].concat();
        divided_by_the_line(&split.sides, &region, &part.region, &all);

        // Neither asks for anything it was granted so: the row ahead is the part's
        // when its player's view is asked of it, with no word to the store, and the
        // region is not told of it again.
        let mut part = part.region;
        let output = part.tick(&TickInputs {
            tickets_added: viewers(&ahead),
            ..TickInputs::default()
        });
        assert!(output.claims.is_empty());
        assert_eq!(output.chunk_requests, ahead);
        assert!(idle(&mut region).claims.is_empty());
    }

    #[test]
    fn a_player_standing_in_a_chunk_whose_grant_waits_goes_and_the_part_holds_that_chunk() {
        // Player 2 has walked off the row the region holds, and the region has claimed
        // the chunk they stand in.
        let mut region = standing(&[(1, HOME), (2, at(9, 0))], &row(0, 8, 0), &[]);
        assert_eq!(idle(&mut region).claims, [at(9, 0)]);
        assert_eq!(region.knowledge(at(9, 0)), Knowledge::Asked);
        // By what its ticks were told the region does not hold the chunk, and nobody
        // would go. By the grant that waits it does: the player goes, and the chunk
        // with them, and what is nearer to it than to home.
        let named = [at(9, 0)];
        assert_eq!(region.split(&named, PART, &[]), Err(NoSplit::Nobody));
        let split = region.split(&named, PART, &named).unwrap();
        let gone: Vec<PlayerId> = split.part.players.keys().copied().collect();
        assert_eq!(gone, [player(2)]);
        assert_eq!(split.sides.seeds, named);
        assert_eq!(split.chunks, row(5, 9, 0));

        // The part holds the chunk its player stands in, and claims nothing for them.
        let (_, part) = region.take_split(split, &named);
        let mut part = part.region;
        assert_eq!(part.knowledge(at(9, 0)), Knowledge::Held);
        assert_eq!(region.knowledge(at(9, 0)), Knowledge::Unknown);
        assert!(idle(&mut part).claims.is_empty());
        assert_eq!(part.player_count(), 1);
    }

    #[test]
    fn whether_anything_stays_is_judged_by_the_grants_that_wait_as_well() {
        let off = |region: &Region, named: &[ChunkPos], granted: &[ChunkPos]| {
            region.split(named, PART, granted).unwrap_err()
        };
        // A region that holds nothing by its ticks: with the grant of the chunk its
        // only player stands in, somebody would go, and nothing would stay.
        let away = [at(3, 0), at(4, 0)];
        let region = standing(&[(1, at(3, 0))], &[], &[]);
        assert_eq!(off(&region, &away, &[]), NoSplit::Nobody);
        assert_eq!(off(&region, &away, &away[..1]), NoSplit::NothingStays);

        // A home chunk that is held by a grant that waits is a place where somebody
        // stays, as one that a tick was told of is, and is no seed for the player who
        // stands in it.
        let stands = [(1, at(3, 0)), (2, at(4, 0)), (3, HOME)];
        let region = standing(&stands[..2], &away, &[]);
        assert_eq!(off(&region, &away, &[]), NoSplit::NothingStays);
        let split = region.split(&away, PART, &[HOME]).unwrap();
        assert_eq!(split.sides.staying, [HOME]);
        assert_eq!(split.chunks, away);
        let region = standing(&stands, &away, &[]);
        let named = [HOME, at(3, 0), at(4, 0)];
        let split = region.split(&named, PART, &[HOME]).unwrap();
        assert_eq!(split.sides.seeds, away);
        assert_eq!(split.state.players.len(), 1);
    }

    #[test]
    fn a_split_planned_with_grants_that_wait_and_not_taken_leaves_them_to_the_next_tick() {
        let granted = [at(14, 0), at(-2, 0)];
        let mut region = asking(&APART, &held_apart(), &[], &granted);
        let mut untouched = region.clone();
        let split = region.split(&[at(12, 0)], PART, &granted).unwrap();
        assert!(split.chunks.contains(&at(14, 0)) && !split.chunks.contains(&at(-2, 0)));
        assert_eq!(region, untouched);
        // It gives the same every time, to the byte.
        let again = region.split(&[at(12, 0)], PART, &granted).unwrap();
        assert_eq!(bytes(&again.state), bytes(&split.state));
        assert_eq!(bytes(&again.part), bytes(&split.part));
        assert_eq!(again, split);

        // The split is off or declined: the answers are where they were, the next tick
        // takes them, and it is the tick of a region that never planned. Both chunks
        // are the region's, the one ahead of player 2 as well.
        let inputs = TickInputs {
            granted: granted.to_vec(),
            ..TickInputs::default()
        };
        let output = region.tick(&inputs);
        assert_eq!(output, untouched.tick(&inputs));
        assert_eq!(output.chunk_requests, [at(-2, 0), at(14, 0)]);
        assert_eq!(region, untouched);
    }

    #[test]
    fn a_grant_that_waits_for_a_chunk_of_a_pinned_area_goes_and_the_region_stays_pinned() {
        // The region is pinned to the chunks with x below 20. It has asked for a chunk
        // of that area ahead of player 2, for one beyond the area, and for one behind
        // player 1.
        let own = area(None, Some(20));
        let (inside, beyond, behind) = (at(14, 0), at(25, 0), at(-2, 0));
        let granted = [inside, beyond, behind];
        let mut region = asking(&APART, &held_apart(), &[own], &granted);
        let split = region.split(&[at(12, 0)], PART, &granted).unwrap();
        let going = [row(9, 12, 0).as_slice(), &[inside, beyond]].concat();
        assert_eq!(split.chunks, going);

        let (_, part) = region.take_split(split.clone(), &granted);
        let staying = [row(0, 3, 0).as_slice(), &[behind]].concat();
        let restored = Region::restore(config(3), split.state.clone(), holdings(&staying, &[own]));
        assert_eq!(region, restored);
        let restored = Region::restore(config(3), split.part.clone(), holdings(&going, &[]));
        assert_eq!(part.region, restored);
        let all = [held_apart(), granted.to_vec()].concat();
        divided_by_the_line(&split.sides, &region, &part.region, &all);
        // The area is the region's still, the chunk that went among it; the part is
        // pinned to nothing.
        assert!(region.pins(inside) && !region.pins(beyond));
        assert!(!part.region.pins(inside));

        // With nobody and nothing to stay near, a grant that waits goes like every
        // other chunk, however far off it is.
        let far = at(-50, 7);
        let mut region = standing(&[(1, at(3, 0))], &[at(3, 0)], &[area(Some(40), None)]);
        let split = region.split(&[at(3, 0)], PART, &[far]).unwrap();
        assert_eq!(split.chunks, [far, at(3, 0)]);
        assert!(split.sides.staying.is_empty());
        region.take_split(split, &[far]);
        assert_eq!(region.held_chunk_count(), 0);
        assert!(region.pins(at(40, 0)));
    }

    #[test]
    fn a_grant_for_a_chunk_the_region_holds_and_one_that_is_named_twice_change_nothing() {
        let region = stretched();
        let named = [at(9, 0)];
        let plain = region.split(&named, PART, &[]).unwrap();
        // Chunks the ticks were told of already, on either side.
        let known = [at(8, 0), at(2, 0), HOME];
        assert_eq!(region.split(&named, PART, &known).unwrap(), plain);

        let once = [at(11, 0)];
        let waiting = region.split(&named, PART, &once).unwrap();
        assert_eq!(waiting.chunks, [row(5, 9, 0).as_slice(), &once].concat());
        let twice = [at(11, 0), at(9, 0), at(11, 0)];
        assert_eq!(region.split(&named, PART, &twice).unwrap(), waiting);
        let (mut one, mut other) = (region.clone(), region);
        let taken = one.take_split(waiting.clone(), &once);
        assert_eq!(other.take_split(waiting, &twice), taken);
        assert_eq!(one, other);
        assert_eq!(taken.1.region.held_chunk_count(), 6);
    }

    #[test]
    fn a_chunk_is_on_the_parts_side_if_it_is_nearer_to_a_seed_than_to_all_that_stays() {
        let sides = Sides {
            seeds: vec![at(10, 0), at(10, 4)],
            staying: vec![at(0, 0), at(20, 0)],
        };
        for position in &sides.seeds {
            assert!(sides.goes(*position), "{position:?}");
        }
        for position in &sides.staying {
            assert!(!sides.goes(*position), "{position:?}");
        }
        // Along the longer axis: (5, 9) is nine from the origin and five from the seed
        // at (10, 4). A chunk that is as near to something that stays as to the
        // nearest seed is not on the part's side.
        for position in [at(6, 0), at(14, -3), at(5, 9), at(10, 2000)] {
            assert!(sides.goes(position), "{position:?}");
        }
        for position in [at(5, 0), at(15, 0), at(5, -5), at(4, 2), at(-3, 0)] {
            assert!(!sides.goes(position), "{position:?}");
        }

        // Where the difference of two coordinates does not fit into 32 bits.
        let sides = Sides {
            seeds: vec![at(10, 0)],
            staying: vec![at(20, 0)],
        };
        assert!(sides.goes(at(i32::MIN, 0)) && sides.goes(at(i32::MIN, i32::MAX)));
        assert!(!sides.goes(at(i32::MAX, 0)));
        // As far along z from both as z reaches.
        assert!(!sides.goes(at(15, i32::MIN)) && !sides.goes(at(0, i32::MAX)));

        // Nothing stays: every chunk is on the part's side.
        let sides = Sides {
            seeds: vec![at(10, 0)],
            staying: Vec::new(),
        };
        let ends = [i32::MIN, 0, i32::MAX];
        for position in ends.iter().flat_map(|x| ends.map(|z| at(*x, z))) {
            assert!(sides.goes(position), "{position:?}");
        }
    }

    /// What a tick let go: each player with the region they go to.
    fn let_go(output: &TickOutput) -> Vec<(PlayerId, PlayerTransfer, RegionId)> {
        let departed = |(_, _, entry): &(EdgeId, u64, Durable)| match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } => Some((*player, transfer.clone(), *to)),
            _ => None,
        };
        output.durable.iter().filter_map(departed).collect()
    }

    /// The input numbered `count` of the player `number`, who has `entity` and is of
    /// `E`: a step to the middle of `to`.
    fn step(number: u128, entity: i32, count: u64, to: ChunkPos) -> TickInputs {
        let input = PlayerInput::Move {
            position: Some(middle(to)),
            rotation: None,
            on_ground: true,
        };
        TickInputs {
            inputs: vec![(E, player(number), EntityId(entity), count, input)],
            ..TickInputs::default()
        }
    }

    #[test]
    fn after_a_split_players_walk_between_the_two_and_the_part_can_be_absorbed_again() {
        let whole = stretched();
        let mut region = whole.clone();
        let split = region.split(&[at(9, 0)], PART, &[]).unwrap();
        let (_, part) = region.take_split(split, &[]);
        let mut part = part.region;
        assert_eq!(region.tick_number(), part.tick_number());

        // The part gives out no entity ids: a join there is refused.
        let join = PlayerChange::Join(
            E,
            PlayerJoin {
                player: player(8),
                name: "Late".to_owned(),
            },
        );
        let output = part.tick(&TickInputs {
            player_changes: vec![join],
            ..TickInputs::default()
        });
        assert_eq!(
            output.durable,
            [(E, 1, Durable::Refused { player: player(8) })]
        );
        // Its player is its own: what the stay does is applied there, with the numbers
        // going on from where they were, and no longer in the region it was split off.
        let stepping = step(2, 2, 7, at(8, 0));
        let output = part.tick(&stepping);
        assert!(matches!(
            output.events.as_slice(),
            [RegionEvent::EntityMoved { entity, .. }] if *entity == EntityId(2)
        ));
        assert!(region.tick(&stepping).events.is_empty());

        // The player who stayed steps into a chunk of the part. The region knows
        // nothing of it any more: they stay, and it asks. The store names the part,
        // and they are let go to it, where they are taken in.
        let output = region.tick(&step(1, 1, 4, at(5, 0)));
        assert_eq!(output.claims, [at(5, 0)]);
        assert!(let_go(&output).is_empty());
        let output = region.tick(&TickInputs {
            foreign: vec![(at(5, 0), PART)],
            ..TickInputs::default()
        });
        let [(id, transfer, to)] = let_go(&output).try_into().unwrap();
        assert_eq!((id, to), (player(1), PART));
        assert_eq!(region.player_count(), 0);
        let output = part.tick(&TickInputs {
            player_changes: vec![PlayerChange::Arrive(E, id, transfer)],
            ..TickInputs::default()
        });
        assert!(output.durable.is_empty() && output.claims.is_empty());
        assert_eq!(part.player_count(), 2);

        // Both walk back to where they stood, player 1 into a chunk the part has asked
        // for and not heard about, and the part is absorbed again by the region it was
        // split off, which the store hands its chunks. The region then has the players
        // and the chunks of one that never was split.
        part.tick(&step(1, 1, 5, HOME));
        part.tick(&step(2, 2, 8, at(9, 0)));
        let came: Vec<ChunkPos> = part.held().collect();
        assert_eq!(came, row(5, 9, 0));
        let merged = region.absorb(PART, &part.state());
        region.take_absorbed(merged, &came, &[]);
        assert_eq!(
            region.held().collect::<Vec<_>>(),
            whole.held().collect::<Vec<_>>()
        );
        let pose = |region: &Region, number| region.player(player(number)).unwrap();
        for number in [1, 2] {
            assert_eq!(pose(&region, number), pose(&whole, number));
        }
        assert_eq!(region.player_count(), 2);
        // Each stay goes on from where it was in the part: what it does next is
        // applied, and what the part applied already is not applied again.
        let mut inputs = step(1, 1, 6, at(1, 0));
        inputs.inputs.append(&mut step(2, 2, 8, at(8, 0)).inputs);
        region.tick(&inputs);
        let stands = |number| region.player(player(number)).unwrap().1.position;
        assert_eq!((stands(1), stands(2)), (middle(at(1, 0)), middle(at(9, 0))));
        // The edge is told of each in turn: the split, what the part left for it, and
        // the merge.
        let outbox: Vec<&Durable> = region.edge(E).unwrap().outbox.values().collect();
        assert!(matches!(
            outbox.as_slice(),
            [
                Durable::SplitOff { region: PART, .. },
                Durable::Departed { to: PART, .. },
                Durable::Absorbed { region: PART, numbers, .. },
                Durable::Refused { .. },
            ] if numbers == &[1]
        ));
    }
}
