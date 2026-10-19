//! The faces of players the region does not hold, which the world store streams to it,
//! and the world's players it publishes (`docs/groundwork/n7-plan.md`, sections 7.3, 8.3,
//! 9.2 and 10.6). Memory only, as copies are: a restored, merged or split region is
//! sent the faces of its chunks again.
//!
//! A face is taken only for a stay at least as late as the latest the region has held
//! or let go of that player, at a sequence at least that of the player's tombstone and
//! above the face kept, of a player the region does not hold, and in a chunk the region
//! holds or copies. A stream and an edge's link are apart, so a face written before the
//! region took a player in and let them go again can come after; the stay it names is
//! then below what the region knows, and it is refused.

use std::collections::BTreeMap;

use clustine_world::{ChunkPos, EntityId, PlayerId};

use super::Region;
use crate::api::{FaceChange, FaceSeq, PlayerFace, WorldPlayer};

/// How long the region keeps the face of a player it let go, at most, if nothing ends
/// it before: about the time a hand-over takes (`docs/groundwork/n7-plan.md`, 9.2.1).
const DEPARTED_TICKS: u64 = 100;

/// A face the region keeps.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Kept {
    pub(super) entity: EntityId,
    pub(super) hops: u32,
    /// Where the store took it; `None` for the face of a player the region let go,
    /// which is below every face the store streams.
    pub(super) seq: Option<FaceSeq>,
    pub(super) face: PlayerFace,
    /// The tick the region let the player go in, for a departed face.
    pub(super) departed: Option<u64>,
}

/// The region's faces, tombstones and the world's players.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Faces {
    pub(super) kept: BTreeMap<PlayerId, Kept>,
    /// The highest removal of each player taken.
    tombstones: BTreeMap<PlayerId, FaceSeq>,
    /// The latest stay of each player the region has held or let go, by its entity and
    /// hand-overs.
    known: BTreeMap<PlayerId, (EntityId, u32)>,
    /// The world's players, as the store last published them.
    pub(super) world: Vec<WorldPlayer>,
}

impl Faces {
    fn known(&self, player: PlayerId) -> (EntityId, u32) {
        self.known.get(&player).copied().unwrap_or((EntityId(0), 0))
    }

    /// The region holds or let go of this stay of the player: a face below it is out of
    /// date from now on.
    pub(super) fn know(&mut self, player: PlayerId, entity: EntityId, hops: u32) {
        let known = self.known.entry(player).or_insert((entity, hops));
        *known = (*known).max((entity, hops));
    }

    /// The region took the player in: it sees them itself.
    pub(super) fn held(&mut self, player: PlayerId, entity: EntityId, hops: u32) {
        self.know(player, entity, hops);
        self.kept.remove(&player);
    }

    /// The region let the player go in `tick`, looking so: their face is kept until the
    /// next region's comes, or a removal, or the player is back, or it is old.
    pub(super) fn departed(
        &mut self,
        player: PlayerId,
        entity: EntityId,
        hops: u32,
        face: PlayerFace,
        tick: u64,
    ) {
        self.know(player, entity, hops);
        self.kept.insert(
            player,
            Kept {
                entity,
                hops,
                seq: None,
                face,
                departed: Some(tick),
            },
        );
    }

    /// Takes a change the store streamed, by the rules above. `holds` says whether the
    /// region holds or copies a chunk, `is_held` whether it holds the player.
    pub(super) fn take(
        &mut self,
        change: &FaceChange,
        holds: impl Fn(ChunkPos) -> bool,
        is_held: impl Fn(PlayerId) -> bool,
    ) {
        match change {
            FaceChange::Face {
                player,
                entity,
                hops,
                seq,
                face,
            } => {
                if (*entity, *hops) < self.known(*player)
                    || self.tombstones.get(player).is_some_and(|stone| seq < stone)
                    || is_held(*player)
                    || !holds(chunk_of(face))
                {
                    return;
                }
                if let Some(kept) = self.kept.get(player)
                    && kept.seq.is_some_and(|kept| kept >= *seq)
                {
                    return;
                }
                self.kept.insert(
                    *player,
                    Kept {
                        entity: *entity,
                        hops: *hops,
                        seq: Some(*seq),
                        face: face.clone(),
                        departed: None,
                    },
                );
            }
            FaceChange::Removed {
                player,
                entity,
                hops,
                seq,
            } => {
                if (*entity, *hops) < self.known(*player) {
                    return;
                }
                let stone = self.tombstones.entry(*player).or_insert(*seq);
                *stone = (*stone).max(*seq);
                if self
                    .kept
                    .get(player)
                    .is_some_and(|kept| kept.seq.is_none_or(|kept| kept < *seq))
                {
                    self.kept.remove(player);
                }
            }
        }
    }

    /// Drops the faces in chunks the region neither holds nor copies any more, but those
    /// of players it let go, and the departed faces that are old.
    pub(super) fn keep_to(&mut self, tick: u64, holds: impl Fn(ChunkPos) -> bool) {
        self.kept.retain(|_, kept| match kept.departed {
            Some(at) => tick < at + DEPARTED_TICKS,
            None => holds(chunk_of(&kept.face)),
        });
    }
}

fn chunk_of(face: &PlayerFace) -> ChunkPos {
    ChunkPos::containing(face.position[0], face.position[2])
}

impl Region {
    /// What the store streamed of faces and the world's players, after the players'
    /// changes of the tick; and the faces kept to what the region holds and copies.
    pub(super) fn take_faces(&mut self, changes: &[FaceChange], world: Option<&Vec<WorldPlayer>>) {
        // Whoever the region holds it sees itself.
        for (id, player) in &self.players {
            self.faces.held(*id, player.entity_id, player.hops);
        }
        let land = &self.land;
        let copies = &self.copies;
        let players = &self.players;
        let holds = |chunk: ChunkPos| land.holds(chunk) || copies.contains_key(&chunk);
        for change in changes {
            self.faces
                .take(change, holds, |player| players.contains_key(&player));
        }
        self.faces.keep_to(self.tick, holds);
        if let Some(world) = world {
            self.faces.world.clone_from(world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(n: u128) -> PlayerId {
        PlayerId(uuid::Uuid::from_u128(n))
    }

    fn at(x: f64) -> PlayerFace {
        PlayerFace {
            position: [x, 64.0, 0.5],
            yaw: 0.0,
            pitch: 0.0,
            eye_height: 1.62,
            main_hand: None,
            off_hand: None,
            worn: [None; 4],
            sneaking: false,
            game_mode: 1,
            sleeping: None,
            vehicle: None,
            alive: true,
            last_hurt_mob: None,
            last_hurt_by: None,
            moved_horizontally: false,
        }
    }

    fn face(entity: u64, hops: u32, seq: u64, x: f64) -> FaceChange {
        FaceChange::Face {
            player: player(1),
            entity: EntityId(entity),
            hops,
            seq: FaceSeq(seq, 0),
            face: at(x),
        }
    }

    fn removed(entity: u64, hops: u32, seq: u64) -> FaceChange {
        FaceChange::Removed {
            player: player(1),
            entity: EntityId(entity),
            hops,
            seq: FaceSeq(seq, 0),
        }
    }

    fn everywhere(_: ChunkPos) -> bool {
        true
    }

    fn nobody(_: PlayerId) -> bool {
        false
    }

    fn kept_x(faces: &Faces) -> Option<f64> {
        faces.kept.get(&player(1)).map(|kept| kept.face.position[0])
    }

    #[test]
    fn a_face_is_taken_only_above_the_one_kept_and_at_least_at_the_tombstone() {
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 10, 1.0), everywhere, nobody);
        faces.take(&face(5, 0, 9, 2.0), everywhere, nobody);
        faces.take(&face(5, 0, 10, 3.0), everywhere, nobody);
        assert_eq!(kept_x(&faces), Some(1.0));
        faces.take(&removed(5, 0, 20), everywhere, nobody);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 0, 15, 4.0), everywhere, nobody);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 0, 20, 5.0), everywhere, nobody);
        assert_eq!(kept_x(&faces), Some(5.0));
    }

    #[test]
    fn a_tombstone_drops_only_a_face_below_it() {
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 30, 1.0), everywhere, nobody);
        faces.take(&removed(5, 0, 20), everywhere, nobody);
        assert_eq!(kept_x(&faces), Some(1.0));
    }

    #[test]
    fn a_face_or_a_removal_of_a_stay_before_one_the_region_held_is_refused() {
        // The player came, was taken in and let go again before a face written before
        // came on the stream (the review's R3b).
        let mut faces = Faces::default();
        faces.know(player(1), EntityId(5), 2);
        faces.take(&face(5, 1, 10, 1.0), everywhere, nobody);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 2, 11, 2.0), everywhere, nobody);
        assert_eq!(kept_x(&faces), Some(2.0));
        faces.take(&removed(5, 1, 12), everywhere, nobody);
        assert_eq!(kept_x(&faces), Some(2.0));
        // A later stay is always above.
        faces.take(&face(6, 0, 13, 3.0), everywhere, nobody);
        assert_eq!(kept_x(&faces), Some(3.0));
    }

    #[test]
    fn the_face_of_a_player_the_region_holds_is_not_kept_and_is_dropped_when_it_takes_them_in() {
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 10, 1.0), everywhere, |_| true);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 0, 10, 1.0), everywhere, nobody);
        faces.held(player(1), EntityId(5), 1);
        assert_eq!(kept_x(&faces), None);
    }

    #[test]
    fn a_face_outside_what_the_region_holds_and_copies_is_refused_and_dropped_when_that_shrinks() {
        let near = |chunk: ChunkPos| chunk.x == 0;
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 10, 40.0), near, nobody);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 0, 11, 1.0), near, nobody);
        assert_eq!(kept_x(&faces), Some(1.0));
        faces.keep_to(1, |_| false);
        assert_eq!(kept_x(&faces), None);
    }

    #[test]
    fn a_departed_face_lasts_until_the_next_regions_face_a_removal_at_its_stay_or_its_time() {
        let departed = |faces: &mut Faces| faces.departed(player(1), EntityId(5), 2, at(9.0), 100);
        // A late removal of the stay as this region held it does not end it (R3a).
        let mut faces = Faces::default();
        faces.know(player(1), EntityId(5), 1);
        departed(&mut faces);
        faces.take(&removed(5, 1, 50), everywhere, nobody);
        faces.keep_to(150, |_| false);
        assert_eq!(kept_x(&faces), Some(9.0));
        // The next region's face replaces it, wherever it is.
        faces.take(&face(5, 2, 51, 12.0), everywhere, nobody);
        assert_eq!(kept_x(&faces), Some(12.0));

        let mut faces = Faces::default();
        departed(&mut faces);
        faces.take(&removed(5, 2, 50), everywhere, nobody);
        assert_eq!(kept_x(&faces), None);

        let mut faces = Faces::default();
        departed(&mut faces);
        faces.keep_to(199, |_| false);
        assert_eq!(kept_x(&faces), Some(9.0));
        faces.keep_to(200, |_| false);
        assert_eq!(kept_x(&faces), None);
    }
}
