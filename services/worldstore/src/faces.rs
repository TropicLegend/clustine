//! The latest face of every player in the world, filed by the chunk they stand in
//! (`docs/groundwork/n7-faces.md`, step 2): the commit thread's, in memory only. A start
//! of the store, or a log that failed, begins with none, and every region's first tick
//! names every stay it has, which brings them back.
//!
//! A face is filed in the dimension of the region whose note wrote it: a player let go
//! through a portal is of the source until the destination writes them
//! (`docs/groundwork/p-plan.md`, section 15.4, their D1).

use std::collections::{BTreeMap, BTreeSet};

use clustine_region::RegionId;
use clustine_rpc::{FaceChange, FaceSeq, PlayerFace, WorldPlayer};
use clustine_world::{ChunkPos, Dimension, EntityId, LevelChunk, PlayerId};

/// A player's face as the store keeps it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Held {
    pub(crate) entity: EntityId,
    pub(crate) hops: u32,
    pub(crate) seq: FaceSeq,
    pub(crate) face: PlayerFace,
    /// The region whose note wrote it.
    pub(crate) writer: RegionId,
    /// The dimension of that region, which the face is of.
    pub(crate) dimension: Dimension,
    /// Whether that note let the stay go: the player is between two regions.
    pub(crate) let_go: bool,
}

impl Held {
    pub(crate) fn chunk(&self) -> LevelChunk {
        let position = ChunkPos::containing(self.face.position[0], self.face.position[2]);
        LevelChunk::new(self.dimension, position)
    }
}

/// The faces, by player and by chunk.
#[derive(Debug, Default)]
pub(crate) struct Faces {
    kept: BTreeMap<PlayerId, Held>,
    by_chunk: BTreeMap<LevelChunk, BTreeSet<PlayerId>>,
}

impl Faces {
    /// Files `held` as the player's face, and gives the one it replaces.
    pub(crate) fn write(&mut self, player: PlayerId, held: Held) -> Option<Held> {
        let chunk = held.chunk();
        let before = self.remove(player);
        self.by_chunk.entry(chunk).or_default().insert(player);
        self.kept.insert(player, held);
        before
    }

    /// Takes the player's face out, and gives it.
    pub(crate) fn remove(&mut self, player: PlayerId) -> Option<Held> {
        let held = self.kept.remove(&player)?;
        let chunk = held.chunk();
        if let Some(players) = self.by_chunk.get_mut(&chunk) {
            players.remove(&player);
            if players.is_empty() {
                self.by_chunk.remove(&chunk);
            }
        }
        Some(held)
    }

    /// The chunks with faces in them, ascending.
    pub(crate) fn chunks(&self) -> impl Iterator<Item = LevelChunk> + '_ {
        self.by_chunk.keys().copied()
    }

    /// The faces in `chunks`, as a base: each as the face it is.
    pub(crate) fn base(&self, chunks: impl IntoIterator<Item = LevelChunk>) -> Vec<FaceChange> {
        let mut base = Vec::new();
        for chunk in chunks {
            for player in self.by_chunk.get(&chunk).into_iter().flatten() {
                let held = &self.kept[player];
                base.push(face_change(*player, held));
            }
        }
        base
    }

    /// A `Gone` of the stay `entity` with `hops` from `region`: whether it ends the
    /// player's face, which it then takes out and gives. It counts from the face's
    /// writer with as many hand-overs or more, from any region with more, and from any
    /// region with as many while the stay is between regions: the edge tells the region
    /// a player who left while being handed over was heading for, which never held them
    /// (the review's R1).
    pub(crate) fn gone(
        &mut self,
        player: PlayerId,
        entity: EntityId,
        hops: u32,
        region: RegionId,
    ) -> Option<Held> {
        let held = self.kept.get(&player)?;
        let counts = held.entity == entity
            && (hops > held.hops || (hops == held.hops && (held.writer == region || held.let_go)));
        if counts { self.remove(player) } else { None }
    }

    /// The faces `region` wrote of players its first tick does not name, but those it
    /// let go, who are between regions: they are offline. Takes them out and gives them.
    pub(crate) fn offline(
        &mut self,
        region: RegionId,
        named: &BTreeSet<PlayerId>,
    ) -> Vec<(PlayerId, Held)> {
        let gone: Vec<PlayerId> = self
            .kept
            .iter()
            .filter(|(player, held)| {
                held.writer == region && !held.let_go && !named.contains(player)
            })
            .map(|(player, _)| *player)
            .collect();
        gone.into_iter()
            .filter_map(|player| Some((player, self.remove(player)?)))
            .collect()
    }

    /// The faces `from` wrote in chunks `moved` says went to `to` are `to`'s from now
    /// on: a split gave them to it.
    pub(crate) fn rewrite_writer_where(
        &mut self,
        from: RegionId,
        to: RegionId,
        moved: impl Fn(LevelChunk) -> bool,
    ) {
        for held in self.kept.values_mut() {
            if held.writer == from && moved(held.chunk()) {
                held.writer = to;
            }
        }
    }

    /// The faces `from` wrote are `to`'s from now on: `to` absorbed it.
    pub(crate) fn rewrite_writer(&mut self, from: RegionId, to: RegionId) {
        for held in self.kept.values_mut() {
            if held.writer == from {
                held.writer = to;
            }
        }
    }

    /// The world's players, in ascending order of their ids: a sleeper's counter is
    /// reckoned from when they lay down to the game time of their region's latest
    /// commit (`committed`), low by up to a second, at most 100.
    pub(crate) fn world(&self, committed: impl Fn(RegionId) -> u64) -> Vec<WorldPlayer> {
        let listed = self.kept.iter().map(|(player, held)| WorldPlayer {
            player: *player,
            entity: held.entity,
            dimension: held.dimension,
            position: held.face.position,
            game_mode: held.face.game_mode,
            sleeping: held.face.sleeping.map(|since| {
                let counter = committed(held.writer).saturating_sub(since).min(100);
                u32::try_from(counter).expect("at most 100")
            }),
        });
        listed.collect()
    }

    /// Forgets every face, as a store that starts has none.
    pub(crate) fn clear(&mut self) {
        self.kept.clear();
        self.by_chunk.clear();
    }
}

/// The face as the stream says it.
pub(crate) fn face_change(player: PlayerId, held: &Held) -> FaceChange {
    FaceChange::Face {
        player,
        entity: held.entity,
        hops: held.hops,
        seq: held.seq,
        face: held.face.clone(),
    }
}

#[cfg(test)]
mod tests {
    use clustine_rpc::PlayerFace;

    use super::*;
    use crate::stays::{place, player};

    fn held(dimension: Dimension, writer: u32, x: f64) -> Held {
        Held {
            entity: EntityId(7),
            hops: 0,
            seq: FaceSeq(1, 0),
            face: PlayerFace::standing(&place(x, 3.0)),
            writer: RegionId(writer),
            dimension,
            let_go: false,
        }
    }

    #[test]
    fn a_face_is_filed_and_listed_in_the_dimension_of_the_region_that_wrote_it() {
        let mut faces = Faces::default();
        faces.write(player(1), held(Dimension::Overworld, 0, 5.0));
        faces.write(player(2), held(Dimension::Nether, 1, 5.0));
        let position = ChunkPos::containing(5.0, 3.0);
        let overworld = LevelChunk::new(Dimension::Overworld, position);
        let nether = LevelChunk::new(Dimension::Nether, position);
        assert_eq!(faces.chunks().collect::<Vec<_>>(), [overworld, nether]);
        let of = |changes: Vec<FaceChange>| -> Vec<PlayerId> {
            changes
                .iter()
                .filter_map(|change| match change {
                    FaceChange::Face { player, .. } => Some(*player),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(of(faces.base([overworld])), [player(1)]);
        assert_eq!(of(faces.base([nether])), [player(2)]);
        let world: Vec<Dimension> = faces.world(|_| 0).iter().map(|p| p.dimension).collect();
        assert_eq!(world, [Dimension::Overworld, Dimension::Nether]);
        // Written again by a region of another dimension, the face moves to its chunk.
        let before = faces.write(player(1), held(Dimension::Nether, 1, 5.0));
        assert_eq!(before.map(|held| held.chunk()), Some(overworld));
        assert_eq!(faces.chunks().collect::<Vec<_>>(), [nether]);
        assert_eq!(of(faces.base([nether])), [player(1), player(2)]);
    }
}
