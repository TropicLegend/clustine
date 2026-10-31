//! What the store publishes of the End and of the world that the End's steps read
//! (`docs/groundwork/p-plan.md`, sections 19.5 and 19.6): the fight's bar, whether the
//! arena is wanted, and the global level events. The End's entities that are not mobs
//! have files of their own (`eye_of_ender.rs`, `end_crystal.rs`, `shulker_bullet.rs`).

use clustine_world::BlockPos;

use super::Region;
use crate::api::{FightSeen, GlobalLevelEvent, PlayerEvent, PublishedVillage, TickOutput};
use crate::physics::containing;

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

    /// Tells every player the region holds each global level event published since its
    /// last tick (`ServerLevel.globalLevelEvent`; section 19.6), in the order of their
    /// numbers. The store publishes to a session only the events made since its first
    /// publication, so a restored region tells none twice. A player in a hand-over is
    /// told by the region that receives them, if that region's record holds the event
    /// still: one who crosses in the moment between two regions' publications can hear
    /// it twice or not at all, which is left so.
    pub(super) fn tell_global_events(&mut self, output: &mut TickOutput) {
        let mut events = std::mem::take(&mut self.published_end.global_events);
        events.sort_by_key(|(number, _)| *number);
        events.dedup_by_key(|(number, _)| *number);
        for (_, event) in events {
            for (id, player) in &self.players {
                let position = heard_at(event, self.config.dimension == event.dimension, {
                    let at = player.pose.position;
                    [at.x, at.y, at.z]
                });
                output.player_events.push((
                    *id,
                    PlayerEvent::GlobalLevelEvent {
                        event: event.event,
                        position,
                        data: event.data,
                    },
                ));
            }
        }
    }
}

/// Where a player hears a global event (`ServerLevel.globalLevelEvent`'s lambda): at the
/// event's block within 32 blocks of its centre in the same level, 32 blocks from the
/// player towards it beyond that, and at the player's own position in another level.
fn heard_at(event: GlobalLevelEvent, same_level: bool, player: [f64; 3]) -> BlockPos {
    if !same_level {
        return containing(player);
    }
    let centre = [
        f64::from(event.position.x) + 0.5,
        f64::from(event.position.y) + 0.5,
        f64::from(event.position.z) + 0.5,
    ];
    let towards = [
        centre[0] - player[0],
        centre[1] - player[1],
        centre[2] - player[2],
    ];
    let distance_sq = towards.iter().map(|d| d * d).sum::<f64>();
    if distance_sq < 32.0 * 32.0 {
        return event.position;
    }
    // `Vec3.normalize` then `scale(32)`.
    let length = distance_sq.sqrt();
    containing([
        player[0] + towards[0] / length * 32.0,
        player[1] + towards[1] / length * 32.0,
        player[2] + towards[2] / length * 32.0,
    ])
}

#[cfg(test)]
#[path = "end_world_tests.rs"]
mod tests;
