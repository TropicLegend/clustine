//! What the store publishes of the End and of the world that the End's steps read
//! (`docs/groundwork/p-plan.md`, sections 19.5 and 19.6): the fight's bar, whether the
//! arena is wanted, and the global level events. The End's entities that are not mobs
//! have files of their own (`eye_of_ender.rs`, `end_crystal.rs`, `shulker_bullet.rs`).

use super::Region;
use crate::api::{FightSeen, GlobalLevelEvent, PublishedVillage};

/// What the store published of the End and of the world that the End's steps read
/// (`docs/groundwork/p-plan.md`, sections 19.5 and 19.6): the fight's bar as the End's
/// home last said it, whether another region of the End has a player in the fight's
/// reach, and the global level events not yet told to the region's players. In memory
/// only: the store publishes them whole again after a restore.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PublishedEnd {
    pub(crate) fight: Option<FightSeen>,
    pub(crate) arena_wanted: bool,
    pub(crate) global_events: Vec<(u64, GlobalLevelEvent)>,
}

impl Region {
    /// Takes what a publication says of the End and of the world's global events; the
    /// events wait for the step that tells them (P10b, the fight's).
    pub(super) fn take_published_end(&mut self, published: &PublishedVillage) {
        let end = &mut self.published_end;
        end.fight = published.fight;
        end.arena_wanted = published.arena_wanted;
        end.global_events
            .extend(published.global_events.iter().copied());
    }
}
