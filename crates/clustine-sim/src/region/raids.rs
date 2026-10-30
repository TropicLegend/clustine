//! Raids as world state in the region (`Raids`, `Raid`'s frame;
//! `docs/groundwork/n10-plan.md`, sections 9.1 to 9.5). The rules of a raid's tick are
//! part R1's (`raid_tick.rs`); this is what the lead keeps, since ordering mistakes hide
//! in it: where a raid's record is kept and how it moves, the world's table and
//! `getRaidAt`, the news other regions send a raid, a raid's start at an omen's end, the
//! raid's stream, and each player's bar.
//!
//! **The record** is kept in a chunk of the land (its centre's at its making) and moves
//! with that chunk; a region ticks the raids of its land. **The table** the store
//! publishes holds every other region's raids, with the chunk of each record, which is
//! where every effect to it goes. **Membership** is the raid's region's to judge: a member
//! it cannot see is alive until its holder says it died or left.

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::raid::{Raid, RaidBarName, RaidStatus};
use clustine_world::{
    BlockPos, ChunkPos, Effect, Effected, EntityId, EntityUse, Float, PlayerId, RaidNews, RaiderUse,
};
use uuid::Uuid;

use super::Random;
use super::Region;
use crate::api::{PlayerEvent, RaidBarChange, RaidSeen, TickOutput};

/// `Raids.getNearbyRaid`'s reach: a raid within 96 blocks of the place asked of
/// (`distSqr < 9216`).
const RAID_REACH_SQR: f64 = 9216.0;
/// `Raid.getMaxRaidOmenLevel()`.
const MOST_OMEN: i32 = 5;

/// A raid as `getRaidAt` finds it: one of the region's own, by its chunk and id, or one
/// of the world's table.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RaidAt {
    Own { chunk: ChunkPos, id: EntityId },
    Seen(RaidSeen),
}

/// What the region keeps of raids in memory: the world's table as last published, what
/// it last reported of its own, the draws of each raid's stream in this tick, the bar
/// each of its players is shown, and the bars of its own raids whose players their tick
/// gathered, each as it stood then.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RaidMemory {
    pub(crate) table: Vec<RaidSeen>,
    reported: Option<Vec<RaidSeen>>,
    draws: BTreeMap<EntityId, u64>,
    bars: BTreeMap<PlayerId, Shown>,
    pub(super) gathered: BTreeMap<Uuid, Shown>,
}

/// The bar a player is shown: its UUID, progress and name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Shown {
    pub(super) bar: Uuid,
    pub(super) progress: f32,
    pub(super) name: RaidBarName,
}

/// A raider of a raid in a chunk the region lets go of or takes up.
struct RaiderOf {
    key: EntityId,
    raid: EntityId,
    wave: i32,
    health: f32,
    leads: bool,
}

fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    super::pois::dist_sqr(a, b)
}

/// What other regions read of `raid`, kept in the chunk at `chunk`.
pub(crate) fn seen(chunk: ChunkPos, raid: &Raid) -> RaidSeen {
    RaidSeen {
        id: raid.id,
        chunk,
        centre: raid.centre,
        active: raid.active,
        status: raid.status,
        groups_spawned: raid.groups_spawned,
        between_waves: raid.is_between_waves(),
        omen: raid.omen_level,
        bar: raid.bar,
        shown: raid.bar_shown,
        progress: raid.bar_progress.0,
        name: raid.bar_name,
    }
}

impl Region {
    /// The raids of the region's land, each with the chunk that keeps it, ascending by
    /// chunk and then id.
    pub(crate) fn own_raids(&self) -> Vec<(ChunkPos, &Raid)> {
        self.chunks
            .iter()
            .flat_map(|(position, chunk)| chunk.raids().iter().map(move |raid| (*position, raid)))
            .collect()
    }

    /// The region's raid `id` and the chunk that keeps it.
    pub(crate) fn find_raid(&self, id: EntityId) -> Option<(ChunkPos, &Raid)> {
        self.own_raids().into_iter().find(|(_, raid)| raid.id == id)
    }

    /// Keeps `raid` in the chunk `chunk` of the land, in place of the raid of its id
    /// there, and says the chunk's raids changed.
    pub(crate) fn put_raid(&mut self, chunk: ChunkPos, raid: Raid, output: &mut TickOutput) {
        let Some(kept) = self.chunks.get_mut(&chunk) else {
            return;
        };
        let mut raids: Vec<Raid> = kept.raids().to_vec();
        raids.retain(|other| other.id != raid.id);
        raids.push(raid);
        kept.set_raids(raids);
        output.raids_changed.push(chunk);
    }

    /// Takes the raid `id` out of the chunk `chunk` (`Raids.tick` removes a stopped raid).
    pub(crate) fn remove_raid(&mut self, chunk: ChunkPos, id: EntityId, output: &mut TickOutput) {
        let Some(kept) = self.chunks.get_mut(&chunk) else {
            return;
        };
        let raids: Vec<Raid> = kept
            .raids()
            .iter()
            .filter(|raid| raid.id != id)
            .cloned()
            .collect();
        if raids.len() != kept.raids().len() {
            kept.set_raids(raids);
            output.raids_changed.push(chunk);
        }
    }

    /// `ServerLevel.getRaidAt(pos)` (`Raids.getNearbyRaid(pos, 9216)`): the nearest active
    /// raid within 96 blocks, of the region's own and the world's table together, the
    /// first of equals by id.
    pub(crate) fn get_raid_at(&self, pos: BlockPos) -> Option<RaidAt> {
        let own = self
            .own_raids()
            .into_iter()
            .filter(|(_, raid)| raid.active)
            .map(|(chunk, raid)| {
                (
                    dist_sqr(raid.centre, pos),
                    raid.id,
                    RaidAt::Own { chunk, id: raid.id },
                )
            });
        let seen = self
            .raid_memory
            .table
            .iter()
            .filter(|raid| raid.active)
            .map(|raid| {
                (
                    dist_sqr(raid.centre, pos),
                    raid.id,
                    RaidAt::Seen(raid.clone()),
                )
            });
        own.chain(seen)
            .filter(|(distance, _, _)| *distance < RAID_REACH_SQR)
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, _, at)| at)
    }

    /// Takes the world's table of raids the store published, of other regions only.
    pub(super) fn take_raid_table(&mut self, raids: &[RaidSeen]) {
        let own: BTreeSet<EntityId> = self.own_raids().iter().map(|(_, raid)| raid.id).collect();
        self.raid_memory.table = raids
            .iter()
            .filter(|raid| !own.contains(&raid.id))
            .cloned()
            .collect();
    }

    /// What the region's report says of its raids: every one, where any changed since
    /// the last report; none where nothing did; all after it is made or restored.
    pub(super) fn report_raids(&mut self, full: bool) -> Option<Vec<RaidSeen>> {
        let now: Vec<RaidSeen> = self
            .own_raids()
            .into_iter()
            .map(|(chunk, raid)| seen(chunk, raid))
            .collect();
        if !full && self.raid_memory.reported.as_ref() == Some(&now) {
            return None;
        }
        self.raid_memory.reported = Some(now.clone());
        Some(now)
    }

    /// The raid `id`'s stream as it stands in this tick, and what it drew before.
    pub(crate) fn raid_stream(&self, id: EntityId, ticks_active: u64) -> Random {
        let mut stream = Random::of_raid(self.seed(), id, ticks_active);
        for _ in 0..self.raid_memory.draws.get(&id).copied().unwrap_or(0) {
            stream.next_u64();
        }
        stream
    }

    /// Notes that `stream`, which began at `start`, drew for the raid `id`.
    pub(crate) fn raid_drew(&mut self, id: EntityId, start: &Random, stream: &Random) {
        let used = stream.draws_since(start);
        if used > 0 {
            *self.raid_memory.draws.entry(id).or_default() += used;
        }
    }

    /// Sends `news` to the raid it names, held at `chunk`: applied here where the region
    /// keeps it, else an effect to the chunk's holder.
    pub(crate) fn send_raid_news(
        &mut self,
        chunk: ChunkPos,
        news: RaidNews,
        output: &mut TickOutput,
    ) {
        if self.chunks.contains_key(&chunk) {
            self.raid_news(news, output);
        } else if self.sends_to(chunk) {
            let at = BlockPos::new(chunk.x * 16, 0, chunk.z * 16);
            self.made.push(Effect {
                at,
                what: Effected::Raid(news),
            });
        }
    }

    /// The holder's side of `news` (section 9.3): the raid it names among the region's
    /// own; one it does not keep ignores it, but a start, which makes one.
    pub(crate) fn raid_news(&mut self, news: RaidNews, output: &mut TickOutput) {
        match news {
            RaidNews::Start { centre, omen } => self.create_or_extend_raid(centre, omen, output),
            RaidNews::Absorb { raid, omen } => {
                self.change_raid(raid, output, |raid| absorb(raid, omen))
            }
            RaidNews::Joined {
                raid,
                member,
                wave,
                health,
                from,
            } => {
                let accepted = self.find_raid(raid).is_some_and(|(_, kept)| {
                    kept.active
                        && !kept.is_over()
                        && !kept.is_stopped()
                        && kept.groups_spawned == wave
                });
                if accepted {
                    // `addWaveMob(…, true)`: its health counted in the raid's total.
                    self.change_raid(raid, output, |kept| {
                        if !kept.members.iter().any(|m| m.key == member) {
                            kept.members.push(clustine_world::raid::RaidMember {
                                wave,
                                key: member,
                                health,
                            });
                            kept.members.sort_by_key(|m| (m.wave, m.key));
                            kept.total_health = Float(kept.total_health.0 + health.0);
                            super::raid_tick::update_bossbar(kept);
                        }
                    });
                } else if self.find_entity(member).is_some() {
                    self.raider_used(member, RaiderUse::Rejected { raid }, output);
                } else if self.sends_to(from) {
                    self.made.push(Effect {
                        at: BlockPos::new(from.x * 16, 0, from.z * 16),
                        what: Effected::OnEntity {
                            key: member,
                            by: EntityUse::Raider(RaiderUse::Rejected { raid }),
                        },
                    });
                }
            }
            RaidNews::Hurt {
                raid,
                member,
                health,
            } => self.change_raid(raid, output, |kept| {
                if let Some(known) = kept.members.iter_mut().find(|m| m.key == member) {
                    known.health = health;
                }
            }),
            RaidNews::Died {
                raid,
                member,
                killer,
            } => self.change_raid(raid, output, |kept| {
                kept.members.retain(|m| m.key != member);
                kept.leaders.retain(|(_, leader)| *leader != member);
                if let Some(hero) = killer
                    && let Err(at) = kept.heroes.binary_search(&hero)
                {
                    kept.heroes.insert(at, hero);
                }
            }),
            // `removeFromRaid(…, true)`: its health, as it last told it, taken from the
            // raid's total.
            RaidNews::Left { raid, member } => self.change_raid(raid, output, |kept| {
                if let Some(at) = kept.members.iter().position(|m| m.key == member) {
                    let left = kept.members.remove(at);
                    kept.total_health = Float(kept.total_health.0 - left.health.0);
                    super::raid_tick::update_bossbar(kept);
                }
                kept.leaders.retain(|(_, leader)| *leader != member);
            }),
        }
    }

    /// The living raiders of a raid in the chunk at `position`, in the chunk's order:
    /// each one's key, raid, wave and health, and whether it leads.
    fn raid_raiders_in(&self, position: ChunkPos) -> Vec<RaiderOf> {
        let Some(chunk) = self.chunks.get(&position) else {
            return Vec::new();
        };
        chunk
            .entities()
            .iter()
            .filter(|entity| crate::mobs::ai::patrol::PATROLLING.contains(&entity.kind))
            .filter_map(|entity| {
                let mob = crate::mobs::Mob::decode(&entity.data).ok()?;
                if mob.body.vitals.dying() {
                    return None;
                }
                let state = crate::mobs::types::illagers::state_of(&mob);
                Some(RaiderOf {
                    key: entity.key,
                    raid: state.raid?,
                    wave: state.wave,
                    health: mob.body.vitals.health,
                    leads: state.patrol_leader,
                })
            })
            .collect()
    }

    /// The raiders of a raid in the chunk at `position`, which the region lets go of,
    /// leave their raids: the game's `updateRaiders` removes a raider that `isRemoved()`,
    /// as one unloaded with its chunk is (`removeFromRaid(…, true)`, n7k Raid 991–1102),
    /// so that the next wave does not wait for it. The region's own raid hears it at
    /// once, another region's by its news (`docs/groundwork/n10-plan.md`, section 9.3).
    /// Each keeps the raid's id, as the game's saved raider keeps `RaidId`, and rejoins
    /// when its chunk is taken up again ([`Region::raiders_taken_up`]).
    pub(super) fn raiders_let_go(&mut self, position: ChunkPos, output: &mut TickOutput) {
        for raider in self.raid_raiders_in(position) {
            let news = RaidNews::Left {
                raid: raider.raid,
                member: raider.key,
            };
            self.tell_raid(raider.raid, news, output);
        }
    }

    /// The raiders of a raid in the chunk at `position`, which the region has just
    /// loaded, rejoin it as the game's `Raider.readAdditionalSaveData` does
    /// (`addWaveMob(level, wave, raider, false)` of the raid its `RaidId` names, and
    /// `setLeader` where it leads, n7 Raider): the region's own raid takes it on its wave
    /// with its health, its total unchanged; another region's by `Joined`, which judges
    /// it on its current wave and counts its health (named). A raid the region does not
    /// know hears nothing.
    pub(super) fn raiders_taken_up(&mut self, position: ChunkPos, output: &mut TickOutput) {
        for raider in self.raid_raiders_in(position) {
            if self.find_raid(raider.raid).is_some() {
                self.change_raid(raider.raid, output, |kept| {
                    kept.members.retain(|member| member.key != raider.key);
                    kept.members.push(clustine_world::raid::RaidMember {
                        wave: raider.wave,
                        key: raider.key,
                        health: Float(raider.health),
                    });
                    kept.members.sort_by_key(|member| (member.wave, member.key));
                    if raider.leads {
                        kept.leaders.retain(|(wave, _)| *wave != raider.wave);
                        kept.leaders.push((raider.wave, raider.key));
                        kept.leaders.sort_unstable();
                    }
                    super::raid_tick::update_bossbar(kept);
                });
            } else if self
                .raid_memory
                .table
                .iter()
                .any(|seen| seen.id == raider.raid)
            {
                let news = RaidNews::Joined {
                    raid: raider.raid,
                    member: raider.key,
                    wave: raider.wave,
                    health: Float(raider.health),
                    from: position,
                };
                self.tell_raid(raider.raid, news, output);
            }
        }
    }

    /// Changes the region's raid `id` where it keeps it.
    pub(crate) fn change_raid(
        &mut self,
        id: EntityId,
        output: &mut TickOutput,
        change: impl FnOnce(&mut Raid),
    ) {
        let Some((chunk, raid)) = self.find_raid(id) else {
            return;
        };
        let mut raid = raid.clone();
        let before = raid.clone();
        change(&mut raid);
        if raid != before {
            self.put_raid(chunk, raid, output);
        }
    }

    /// `Raids.createOrExtendRaid`'s end where the region holds the centre's chunk
    /// (section 9.5): the raid at `centre` absorbs the omen, or a new one is made there.
    /// A raid of another region's land near it gets the omen by its news.
    pub(crate) fn create_or_extend_raid(
        &mut self,
        centre: BlockPos,
        omen: i32,
        output: &mut TickOutput,
    ) {
        if !self.chunks.contains_key(&centre.chunk()) {
            self.send_raid_news(centre.chunk(), RaidNews::Start { centre, omen }, output);
            return;
        }
        match self.get_raid_at(centre) {
            Some(RaidAt::Own { id, .. }) => self.change_raid(id, output, |raid| absorb(raid, omen)),
            Some(RaidAt::Seen(raid)) => {
                self.send_raid_news(
                    raid.chunk,
                    RaidNews::Absorb {
                        raid: raid.id,
                        omen,
                    },
                    output,
                );
            }
            None => {
                let Some(id) = self.take_key() else {
                    return;
                };
                let start = self.raid_stream(id, 0);
                let mut stream = start.clone();
                // `Mth.createInsecureUUID(random)`: two longs.
                let high = stream.next_u64();
                let low = stream.next_u64();
                self.raid_drew(id, &start, &stream);
                let mut raid = Raid {
                    id,
                    centre,
                    started: false,
                    active: true,
                    ticks_active: 0,
                    omen_level: 0,
                    groups_spawned: 0,
                    group_count: groups_for(clustine_world::Difficulty::WORLD),
                    cooldown_ticks: 300,
                    post_raid_ticks: 0,
                    celebration_ticks: 0,
                    total_health: Float(0.0),
                    status: RaidStatus::Ongoing,
                    spawn_pos: None,
                    bar: Uuid::from_u64_pair(high, low),
                    bar_shown: true,
                    bar_progress: Float(0.0),
                    bar_name: RaidBarName::Raid,
                    heroes: Vec::new(),
                    members: Vec::new(),
                    leaders: Vec::new(),
                };
                absorb(&mut raid, omen);
                self.put_raid(centre.chunk(), raid, output);
            }
        }
    }

    /// The raids' part of the tick (`Raids.tick`, between the fluid ticks and the chunk
    /// source): each raid of the land ticked by its rules (`raid_tick.rs`), a stopped raid
    /// taken away; then each player's bar.
    pub(super) fn tick_raids(&mut self, output: &mut TickOutput) {
        self.raid_memory.draws.clear();
        let raids: Vec<(ChunkPos, EntityId, BlockPos)> = self
            .own_raids()
            .into_iter()
            .map(|(chunk, raid)| (chunk, raid.id, raid.centre))
            .collect();
        for (chunk, id, centre) in raids {
            // `active` is `hasChunkAt(centre)`: here the record's chunk ticks with the
            // land, and the centre's has to be the land's or a copy's.
            let active = self.chunks.contains_key(&centre.chunk())
                || self.copies.contains_key(&centre.chunk());
            self.change_raid(id, output, |raid| raid.active = active);
            self.tick_raid(chunk, id, output);
            if self
                .find_raid(id)
                .is_some_and(|(_, raid)| raid.is_stopped())
            {
                self.remove_raid(chunk, id, output);
            }
        }
        self.show_raid_bars(output);
    }

    /// Each of the region's players is shown the bar of the raid that `getRaidAt` finds
    /// where they stand, while it is shown (`Raid.updatePlayers`' set, the players whose
    /// raid it is): added where it begins, changed where its progress or name changed,
    /// taken away where it ends. The set of a bar is worked out where its raid's tick
    /// gathered it ([`RaidMemory::gathered`]), a bar added as it stood then and changed
    /// after as it stands now; that of another region's raid, every twentieth game tick
    /// (named). The bar's changes are sent every tick.
    fn show_raid_bars(&mut self, output: &mut TickOutput) {
        let gathered = std::mem::take(&mut self.raid_memory.gathered);
        let own: Vec<Uuid> = self
            .own_raids()
            .into_iter()
            .map(|(_, raid)| raid.bar)
            .collect();
        let every_twenty = self.clock.game_time.is_multiple_of(20);
        let regathers =
            |bar: Uuid| gathered.contains_key(&bar) || (every_twenty && !own.contains(&bar));
        // Every player is alive and none a spectator while everyone is in creative mode.
        let players: Vec<(PlayerId, BlockPos, bool)> = self
            .players
            .iter()
            .map(|(id, player)| {
                let at = player.pose.position;
                (*id, crate::physics::containing([at.x, at.y, at.z]), true)
            })
            .collect();
        for (id, at, alive) in players {
            let kept = self.raid_memory.bars.get(&id).copied();
            let now = if alive { self.raid_bar_at(at) } else { None };
            // A gathering of the bar it is shown takes it away where its raid is not
            // theirs any more; one of the bar of their raid adds it.
            let kept_now = kept.and_then(|kept| {
                if regathers(kept.bar) {
                    now.filter(|now| now.bar == kept.bar)
                } else {
                    self.raid_bar_of(kept.bar)
                }
            });
            let wanted = kept_now.or_else(|| now.filter(|now| regathers(now.bar)));
            // A bar is added as it stood when its raid gathered its players.
            let added = |now: Shown| {
                let then = gathered.get(&now.bar).copied().unwrap_or(now);
                let mut changes = vec![(
                    Some(now.bar),
                    RaidBarChange::Add {
                        name: then.name,
                        progress: then.progress,
                    },
                )];
                if then.progress != now.progress {
                    changes.push((Some(now.bar), RaidBarChange::Progress(now.progress)));
                }
                if then.name != now.name {
                    changes.push((Some(now.bar), RaidBarChange::Name(now.name)));
                }
                changes
            };
            let change = match (kept, wanted) {
                (None, None) => continue,
                (Some(_), None) => vec![(kept.map(|k| k.bar), RaidBarChange::Remove)],
                (None, Some(now)) => added(now),
                (Some(was), Some(now)) if was.bar != now.bar => {
                    let mut changes = vec![(Some(was.bar), RaidBarChange::Remove)];
                    changes.extend(added(now));
                    changes
                }
                (Some(was), Some(now)) => {
                    let mut changes = Vec::new();
                    if was.progress != now.progress {
                        changes.push((Some(now.bar), RaidBarChange::Progress(now.progress)));
                    }
                    if was.name != now.name {
                        changes.push((Some(now.bar), RaidBarChange::Name(now.name)));
                    }
                    changes
                }
            };
            for (bar, change) in change {
                if let Some(bar) = bar {
                    output
                        .player_events
                        .push((id, PlayerEvent::RaidBar { id: bar, change }));
                }
            }
            match wanted {
                Some(now) => {
                    self.raid_memory.bars.insert(id, now);
                }
                None => {
                    self.raid_memory.bars.remove(&id);
                }
            }
        }
        self.raid_memory
            .bars
            .retain(|id, _| self.players.contains_key(id));
    }

    /// The bar of the raid `getRaidAt` finds at `at`, while it is shown.
    fn raid_bar_at(&self, at: BlockPos) -> Option<Shown> {
        match self.get_raid_at(at)? {
            RaidAt::Own { id, .. } => {
                let (_, raid) = self.find_raid(id)?;
                raid.bar_shown.then_some(Shown {
                    bar: raid.bar,
                    progress: raid.bar_progress.0,
                    name: raid.bar_name,
                })
            }
            RaidAt::Seen(raid) => raid.shown.then_some(Shown {
                bar: raid.bar,
                progress: raid.progress,
                name: raid.name,
            }),
        }
    }

    /// The bar `bar` as it stands now, where its raid is known and shows it.
    fn raid_bar_of(&self, bar: Uuid) -> Option<Shown> {
        let own = self
            .own_raids()
            .into_iter()
            .find(|(_, raid)| raid.bar == bar)
            .and_then(|(_, raid)| {
                raid.bar_shown.then_some(Shown {
                    bar,
                    progress: raid.bar_progress.0,
                    name: raid.bar_name,
                })
            });
        own.or_else(|| {
            self.raid_memory
                .table
                .iter()
                .find(|raid| raid.bar == bar)
                .and_then(|raid| {
                    raid.shown.then_some(Shown {
                        bar,
                        progress: raid.progress,
                        name: raid.name,
                    })
                })
        })
    }
}

/// `Raid.absorbRaidOmen`: the omen's level added, at most [`MOST_OMEN`].
fn absorb(raid: &mut Raid, omen: i32) {
    raid.omen_level = (raid.omen_level + omen).clamp(0, MOST_OMEN);
}

/// `Raid.getNumGroups(difficulty)`: easy 3, normal 5, hard 7.
fn groups_for(difficulty: clustine_world::Difficulty) -> i32 {
    match difficulty {
        clustine_world::Difficulty::Peaceful | clustine_world::Difficulty::Easy => 3,
        clustine_world::Difficulty::Normal => 5,
        clustine_world::Difficulty::Hard => 7,
    }
}
