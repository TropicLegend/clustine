//! The latest face of every player in the world, filed by the chunk they stand in
//! (`docs/groundwork/n7-faces.md`, step 2): the commit thread's, in memory only. A start
//! of the store, or a log that failed, begins with none, and every region's first tick
//! names every stay it has, which brings them back.

use std::collections::{BTreeMap, BTreeSet};

use clustine_region::RegionId;
use clustine_rpc::{FaceChange, FaceSeq, PlayerFace, WorldPlayer};
use clustine_world::{ChunkPos, EntityId, PlayerId};

/// A player's face as the store keeps it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Held {
    pub(crate) entity: EntityId,
    pub(crate) hops: u32,
    pub(crate) seq: FaceSeq,
    pub(crate) face: PlayerFace,
    /// The region whose note wrote it.
    pub(crate) writer: RegionId,
    /// Whether that note let the stay go: the player is between two regions.
    pub(crate) let_go: bool,
}

impl Held {
    pub(crate) fn chunk(&self) -> ChunkPos {
        ChunkPos::containing(self.face.position[0], self.face.position[2])
    }
}

/// The faces, by player and by chunk.
#[derive(Debug, Default)]
pub(crate) struct Faces {
    kept: BTreeMap<PlayerId, Held>,
    by_chunk: BTreeMap<ChunkPos, BTreeSet<PlayerId>>,
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
    pub(crate) fn chunks(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.by_chunk.keys().copied()
    }

    /// The faces in `chunks`, as a base: each as the face it is.
    pub(crate) fn base(&self, chunks: impl IntoIterator<Item = ChunkPos>) -> Vec<FaceChange> {
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
        moved: impl Fn(ChunkPos) -> bool,
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

    /// The world's players, in ascending order of their ids.
    pub(crate) fn world(&self) -> Vec<WorldPlayer> {
        let listed = self.kept.iter().map(|(player, held)| WorldPlayer {
            player: *player,
            entity: held.entity,
            position: held.face.position,
            game_mode: held.face.game_mode,
            sleeping: held.face.sleeping,
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
