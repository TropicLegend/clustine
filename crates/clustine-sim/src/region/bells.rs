//! What a bell's ring does to the living near it (`BellBlockEntity.updateEntities`,
//! `makeRaidersGlow`, the resonance; `docs/groundwork/n10-dumps.md`, part B, correction 1;
//! `docs/groundwork/n10-plan.md`, decision 17): the ring's block event lists the living
//! in the bell's box grown by 48 (again only once 60 game ticks have passed since the
//! last listing) and gives those strictly within 32 of its centre `heard_bell_time`;
//! from the bell's fifth tick of shaking, a raider of the list within 32 starts its
//! resonance (its sound, once a ring), and forty ticks of resonance on the raiders of the
//! list within 48 glow for 60 ticks.
//!
//! The list is of the region's land and copies; a villager or a raider another region
//! holds is reached by an entity effect (`VillagerUse::Heard`, `RaiderUse::Glow`), one
//! beyond the copies not at all (named, decision 17). Only a villager's brain keeps
//! `heard_bell_time`, so only villagers are told it.

use std::collections::BTreeMap;

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, EntityId, EntityUse, RaiderUse, Vec3, VillagerUse};

use super::Region;
use super::items::View;
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::ai::sensing::Looked;
use crate::mobs::view::MobWorld;
use crate::physics::aabb::Aabb;

/// `BellBlockEntity.MIN_TICKS_BETWEEN_SEARCHES`.
const BETWEEN_SEARCHES: i64 = 60;
/// `SEARCH_RADIUS`, `HEAR_BELL_RADIUS` and `HIGHLIGHT_RAIDERS_RADIUS`.
const SEARCH_RADIUS: f64 = 48.0;
const HEAR_RADIUS: f64 = 32.0;
const HIGHLIGHT_RADIUS: f64 = 48.0;
/// `TICKS_BEFORE_RESONATION` and `MAX_RESONATION_TICKS`.
const BEFORE_RESONATION: i32 = 5;
const RESONATION_TICKS: i32 = 40;

/// What the region remembers of a bell that rang: the living it listed and when
/// (`nearbyEntities`, `lastRingTimestamp`). In memory only, as the game's is unsaved.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Ring {
    pub(crate) entities: Vec<EntityId>,
    pub(crate) listed_at: i64,
}

/// Every bell's [`Ring`], by its place.
pub(crate) type Rings = BTreeMap<BlockPos, Ring>;

/// The listed thing `key` as the region reads it now: alive, and not gone
/// (`isAlive() && !isRemoved()`).
fn alive_now(view: &View<'_>, key: EntityId) -> Option<Looked> {
    let seen = view.entity(key)?;
    let looked = Looked::of_seen(&seen);
    looked.alive.then_some(looked)
}

/// `BlockPos.closerToCenterThan(position, distance)`: strictly within, of the block's
/// centre.
fn within(pos: BlockPos, point: [f64; 3], distance: f64) -> bool {
    let centre = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ];
    crate::physics::distance_sqr(centre, point) < distance * distance
}

/// Whether a thing of the type `kind` is of `#raiders`.
fn is_raider(kind: Option<u16>) -> bool {
    kind.is_some_and(|kind| {
        clustine_blocks::data::entity_in_tag(i32::from(kind), "minecraft:raiders")
    })
}

impl Region {
    /// The ring's block event ran at the bell `pos` (`triggerEvent`'s `updateEntities`):
    /// the living in its box grown by 48 listed (again only after 60 ticks), and the
    /// villagers within 32 of its centre given `heard_bell_time`.
    pub(super) fn bell_rang(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let listed = self
            .bell_rings
            .get(&pos)
            .is_some_and(|ring| time <= ring.listed_at + BETWEEN_SEARCHES);
        let (entities, heard): (Vec<EntityId>, Vec<EntityId>) = {
            let view = View {
                region: self,
                home: pos.chunk(),
            };
            let entities: Vec<EntityId> = if listed {
                self.bell_rings[&pos].entities.clone()
            } else {
                let low = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
                let bounds = Aabb::new(low, [low[0] + 1.0, low[1] + 1.0, low[2] + 1.0])
                    .inflate([SEARCH_RADIUS; 3]);
                view.entities_in(&bounds)
                    .iter()
                    .map(Looked::of_seen)
                    .filter(crate::mobs::brain::sensors::is_living)
                    .map(|looked| looked.key)
                    .collect()
            };
            let heard = entities
                .iter()
                .filter_map(|key| alive_now(&view, *key))
                .filter(|looked| {
                    looked.kind == Some(entity_types::VILLAGER as u16)
                        && within(pos, looked.position, HEAR_RADIUS)
                })
                .map(|looked| looked.key)
                .collect();
            (entities, heard)
        };
        if !listed {
            self.bell_rings.insert(
                pos,
                Ring {
                    entities,
                    listed_at: time,
                },
            );
        }
        for key in heard {
            self.villager_write(
                key,
                VillagerUse::Heard {
                    at: u64::try_from(time).unwrap_or(0),
                },
                output,
            );
        }
    }

    /// The bell's block entity at `pos` ticked (its shaking counted): from its fifth
    /// tick of shaking, with no resonance this ring, a raider of its list within 32
    /// starts one, heard; a resonance counts to forty, and on the tick after makes the
    /// raiders of the list within 48 glow (`makeRaidersGlow`). The list is let go once
    /// nothing can read it before it would be taken again.
    pub(super) fn bell_ticked(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(ring) = self.bell_rings.get(&pos).cloned() else {
            return;
        };
        let Some(bell) = self.bell_mut(pos).copied() else {
            return;
        };
        let raiders = |region: &Region, radius: f64| -> Vec<EntityId> {
            let view = View {
                region,
                home: pos.chunk(),
            };
            ring.entities
                .iter()
                .filter_map(|key| alive_now(&view, *key))
                .filter(|looked| is_raider(looked.kind) && within(pos, looked.position, radius))
                .map(|looked| looked.key)
                .collect()
        };
        let mut bell = bell;
        if bell.ticks >= BEFORE_RESONATION
            && bell.resonation_ticks == 0
            && !raiders(self, HEAR_RADIUS).is_empty()
        {
            bell.resonating = true;
            output.events.push(RegionEvent::Sound {
                sound: sounds::BLOCK_BELL_RESONATE,
                source: crate::api::SOUND_SOURCE_BLOCKS,
                position: Vec3::new(
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
        let mut glowing = Vec::new();
        if bell.resonating {
            if bell.resonation_ticks < RESONATION_TICKS {
                bell.resonation_ticks += 1;
            } else {
                glowing = raiders(self, HIGHLIGHT_RADIUS);
                bell.resonating = false;
            }
        }
        if let Some(kept) = self.bell_mut(pos) {
            *kept = bell;
        }
        for key in glowing {
            if self.find_entity(key).is_some() {
                self.raider_used(key, RaiderUse::Glow, output);
            } else {
                self.send_on_entity(key, EntityUse::Raider(RaiderUse::Glow));
            }
        }
        let time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        if !bell.shaking && !bell.resonating && time > ring.listed_at + BETWEEN_SEARCHES {
            self.bell_rings.remove(&pos);
        }
    }
}
