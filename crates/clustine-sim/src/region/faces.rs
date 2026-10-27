//! The faces of players the region does not hold, which the world store streams to it,
//! and the world's players it publishes (`docs/groundwork/n7-plan.md`, sections 7.3, 8.3,
//! 9.2 and 10.6). Memory only, as copies are: a restored, merged or split region is
//! sent the faces of its chunks again.
//!
//! A face is taken only for a stay at least as late as the latest the region has held
//! or let go of that player, at a sequence at least that of the player's tombstone and
//! above the face kept, and of a player the region does not hold. A stream and an
//! edge's link are apart, so a face written before the region took a player in and let
//! them go again can come after; the stay it names is then below what the region knows,
//! and it is refused.
//!
//! Every face so taken is kept, but mobs see only those in chunks the region holds or
//! copies (and the faces of players it let go). The worker can hold a copy's base back
//! until its chunk may be read, and faces streamed meanwhile are not lost so. A face
//! kept where the region no longer holds or copies is told of no more, and may be out
//! of date; the base of a chunk the region comes to hold or copy begins by dropping
//! what it keeps there from before ([`FaceChange::Base`]).

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

    /// Takes a change the store streamed, by the rules above. `is_held` says whether the
    /// region holds the player.
    pub(super) fn take(&mut self, change: &FaceChange, is_held: impl Fn(PlayerId) -> bool) {
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
            FaceChange::Base { chunks, seq } => {
                let within =
                    |chunk: ChunkPos| chunks.as_ref().is_none_or(|chunks| chunks.contains(&chunk));
                self.kept.retain(|_, kept| {
                    kept.departed.is_some()
                        || !within(chunk_of(&kept.face))
                        || kept.seq.is_some_and(|kept| kept > *seq)
                });
            }
        }
    }

    /// Drops the departed faces that are old.
    pub(super) fn expire(&mut self, tick: u64) {
        self.kept
            .retain(|_, kept| kept.departed.is_none_or(|at| tick < at + DEPARTED_TICKS));
    }

    /// The faces mobs see, by the player: those in chunks `holds` says the region holds
    /// or copies, and those of players it let go.
    pub(super) fn seen(
        &self,
        holds: impl Fn(ChunkPos) -> bool,
    ) -> impl Iterator<Item = (&PlayerId, &Kept)> {
        self.kept
            .iter()
            .filter(move |(_, kept)| kept.departed.is_some() || holds(chunk_of(&kept.face)))
    }
}

pub(super) fn chunk_of(face: &PlayerFace) -> ChunkPos {
    ChunkPos::containing(face.position[0], face.position[2])
}

impl Region {
    /// The face of the player whose stay is `key`, as mobs see it: in land the region
    /// holds or copies, or of a player it let go.
    pub(crate) fn face_of_entity(&self, key: EntityId) -> Option<&PlayerFace> {
        let holds = |chunk: ChunkPos| self.land.holds(chunk) || self.copies.contains_key(&chunk);
        self.faces
            .seen(holds)
            .find(|(_, kept)| kept.entity == key)
            .map(|(_, kept)| &kept.face)
    }

    /// What the store streamed of faces and the world's players, after the players'
    /// changes of the tick; and the faces kept to what the region holds and copies.
    pub(super) fn take_faces(
        &mut self,
        granted: &[ChunkPos],
        changes: &[FaceChange],
        world: Option<&Vec<WorldPlayer>>,
    ) {
        // What it kept of chunks it begins to hold is from before, and their faces come
        // behind the grant.
        if !granted.is_empty() {
            self.faces.kept.retain(|_, kept| {
                kept.departed.is_some() || !granted.contains(&chunk_of(&kept.face))
            });
        }
        // Whoever the region holds it sees itself.
        for (id, player) in &self.players {
            self.faces.held(*id, player.entity_id, player.hops);
        }
        let players = &self.players;
        for change in changes {
            self.faces
                .take(change, |player| players.contains_key(&player));
        }
        self.faces.expire(self.tick);
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
            worn: [const { None }; 4],
            sneaking: false,
            game_mode: 1,
            sleeping: None,
            vehicle: None,
            alive: true,
            last_hurt_mob: None,
            last_hurt_by: None,
            moved_horizontally: false,
            sprinting: false,
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

    fn nobody(_: PlayerId) -> bool {
        false
    }

    fn kept_x(faces: &Faces) -> Option<f64> {
        faces.kept.get(&player(1)).map(|kept| kept.face.position[0])
    }

    #[test]
    fn a_face_is_taken_only_above_the_one_kept_and_at_least_at_the_tombstone() {
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 10, 1.0), nobody);
        faces.take(&face(5, 0, 9, 2.0), nobody);
        faces.take(&face(5, 0, 10, 3.0), nobody);
        assert_eq!(kept_x(&faces), Some(1.0));
        faces.take(&removed(5, 0, 20), nobody);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 0, 15, 4.0), nobody);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 0, 20, 5.0), nobody);
        assert_eq!(kept_x(&faces), Some(5.0));
    }

    #[test]
    fn a_tombstone_drops_only_a_face_below_it() {
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 30, 1.0), nobody);
        faces.take(&removed(5, 0, 20), nobody);
        assert_eq!(kept_x(&faces), Some(1.0));
    }

    #[test]
    fn a_face_or_a_removal_of_a_stay_before_one_the_region_held_is_refused() {
        // The player came, was taken in and let go again before a face written before
        // came on the stream (the review's R3b).
        let mut faces = Faces::default();
        faces.know(player(1), EntityId(5), 2);
        faces.take(&face(5, 1, 10, 1.0), nobody);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 2, 11, 2.0), nobody);
        assert_eq!(kept_x(&faces), Some(2.0));
        faces.take(&removed(5, 1, 12), nobody);
        assert_eq!(kept_x(&faces), Some(2.0));
        // A later stay is always above.
        faces.take(&face(6, 0, 13, 3.0), nobody);
        assert_eq!(kept_x(&faces), Some(3.0));
    }

    #[test]
    fn the_face_of_a_player_the_region_holds_is_not_kept_and_is_dropped_when_it_takes_them_in() {
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 10, 1.0), |_| true);
        assert_eq!(kept_x(&faces), None);
        faces.take(&face(5, 0, 10, 1.0), nobody);
        faces.held(player(1), EntityId(5), 1);
        assert_eq!(kept_x(&faces), None);
    }

    #[test]
    fn a_face_outside_what_the_region_holds_and_copies_is_kept_but_not_seen() {
        let near = |chunk: ChunkPos| chunk.x == 0;
        let seen = |faces: &Faces| faces.seen(near).count();
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 10, 40.0), nobody);
        assert_eq!((kept_x(&faces), seen(&faces)), (Some(40.0), 0));
        faces.take(&face(5, 0, 11, 1.0), nobody);
        assert_eq!(seen(&faces), 1);
    }

    #[test]
    fn a_base_drops_what_is_kept_in_its_chunks_from_before_it_and_nothing_after() {
        let base = |chunks: Option<Vec<ChunkPos>>, seq: u64| FaceChange::Base {
            chunks,
            seq: FaceSeq(seq, u32::MAX),
        };
        let mut faces = Faces::default();
        faces.take(&face(5, 0, 10, 1.0), nobody);
        faces.take(&base(Some(vec![ChunkPos::new(3, 0)]), 20), nobody);
        assert_eq!(kept_x(&faces), Some(1.0));
        faces.take(&base(Some(vec![ChunkPos::new(0, 0)]), 9), nobody);
        assert_eq!(kept_x(&faces), Some(1.0));
        faces.take(&base(None, 10), nobody);
        assert_eq!(kept_x(&faces), None);
        // A departed face is the region's own, and no base drops it.
        faces.departed(player(1), EntityId(5), 1, at(2.0), 30);
        faces.take(&base(None, 40), nobody);
        assert_eq!(kept_x(&faces), Some(2.0));
    }

    #[test]
    fn a_departed_face_lasts_until_the_next_regions_face_a_removal_at_its_stay_or_its_time() {
        let departed = |faces: &mut Faces| faces.departed(player(1), EntityId(5), 2, at(9.0), 100);
        // A late removal of the stay as this region held it does not end it (R3a).
        let mut faces = Faces::default();
        faces.know(player(1), EntityId(5), 1);
        departed(&mut faces);
        faces.take(&removed(5, 1, 50), nobody);
        faces.expire(150);
        assert_eq!(kept_x(&faces), Some(9.0));
        // The next region's face replaces it, wherever it is.
        faces.take(&face(5, 2, 51, 12.0), nobody);
        assert_eq!(kept_x(&faces), Some(12.0));

        let mut faces = Faces::default();
        departed(&mut faces);
        faces.take(&removed(5, 2, 50), nobody);
        assert_eq!(kept_x(&faces), None);

        let mut faces = Faces::default();
        departed(&mut faces);
        faces.expire(199);
        assert_eq!(kept_x(&faces), Some(9.0));
        faces.expire(200);
        assert_eq!(kept_x(&faces), None);
    }
}
