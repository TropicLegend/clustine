//! What a villager, a raid or a bell does to a villager or a raider that another region
//! holds, as its holder applies it (`EntityUse::Villager`, `EntityUse::Raider`;
//! `docs/groundwork/n10-plan.md`, decisions 2, 4 and 17, section 9.3). The writer acted
//! as if the write took; where a ticket of a point of interest rides on it, the holder
//! checks the game's conditions as it lands, and gives the ticket back where they fail,
//! so that no ticket is left with nobody remembering it.

use clustine_world::poi::PoiType;
use clustine_world::{BlockPos, EntityId, Float, RaiderUse, VillagerUse};

use super::Region;
use crate::api::TickOutput;
use crate::mobs::brain::{self, MemoryKind, MemoryWrite, Tracker, Value};
use crate::mobs::types::{self, gossip::GossipType, villager};
use crate::mobs::{Mob, brain::behaviours::social_villagers};

/// How long a summoned golem is remembered (`GolemSensor.golemDetected`: 599 ticks).
const GOLEM_DETECTED_FOR: i64 = 599;
/// How many points of `major_negative` a witness of a murder adds.
const MURDER_GOSSIP: i32 = 25;
/// How long a bell makes a raider glow (`BellBlockEntity.makeRaidersGlow`).
const GLOW_TICKS: i32 = 60;

impl Region {
    /// A villager's write `used` into the villager `key` (`Local::Villager`): the
    /// region's own at once, one only a copy shows through its holder.
    pub(crate) fn villager_write(
        &mut self,
        key: EntityId,
        used: VillagerUse,
        output: &mut TickOutput,
    ) {
        if self.find_entity(key).is_some() {
            self.villager_used(key, used, output);
        } else {
            self.send_on_entity(key, clustine_world::EntityUse::Villager(used));
        }
    }

    /// The holder's side of `used` on its villager `key`.
    pub(crate) fn villager_used(
        &mut self,
        key: EntityId,
        used: VillagerUse,
        output: &mut TickOutput,
    ) {
        let Some((_, record)) = self.find_entity(key) else {
            // Gone before it landed: a ticket that rode on it goes back.
            if let VillagerUse::Yield { site, kind, .. } = used {
                self.release_poi(site, kind, output);
            }
            return;
        };
        if i32::from(record.kind) != clustine_data::entity_types::VILLAGER {
            if let VillagerUse::Yield { site, kind, .. } = used {
                self.release_poi(site, kind, output);
            }
            return;
        }
        match used {
            VillagerUse::Gossiped { at } => self.update_villager(key, output, |_, own, _| {
                own.last_gossip_time = Some(at as i64);
            }),
            VillagerUse::Gaze { to, speed, close } => {
                self.write_mob_memories(key, gaze_at(to, speed, close), output);
            }
            VillagerUse::Yield { site, kind, speed } => {
                self.yielded(key, site, kind, speed, output)
            }
            VillagerUse::LoseJobSite { site } => {
                let Some(provider) = types::of(record.kind).brain() else {
                    return;
                };
                let Ok(mob) = Mob::decode(&record.data) else {
                    return;
                };
                let memories = brain::memories_of(provider, &mob, record.lived);
                if memories.get(MemoryKind::JobSite) == Some(Value::Block(site)) {
                    self.write_mob_memories(
                        key,
                        vec![MemoryWrite::Erase(MemoryKind::JobSite)],
                        output,
                    );
                }
            }
            VillagerUse::Born => self.update_villager(key, output, |mob, _, lived| {
                social_villagers::born_as_partner(mob, lived);
            }),
            VillagerUse::GolemDetected => self.write_mob_memories(
                key,
                vec![MemoryWrite::SetFor(
                    MemoryKind::GolemDetectedRecently,
                    Value::Bool(true),
                    GOLEM_DETECTED_FOR,
                )],
                output,
            ),
            VillagerUse::Witnessed { murderer } => {
                self.update_villager(key, output, |_, own, _| {
                    own.gossips
                        .add(murderer, GossipType::MajorNegative, MURDER_GOSSIP);
                })
            }
            VillagerUse::Heard { at } => self.write_mob_memories(
                key,
                vec![MemoryWrite::Set(
                    MemoryKind::HeardBellTime,
                    Value::Long(at as i64),
                )],
                output,
            ),
        }
    }

    /// `YieldJobSite`'s write as it lands: the partner takes the site only where the
    /// game's test of it still holds (an adult with no reservation, whose job site is
    /// none or this one, of a profession that holds the site's type); else the ticket
    /// goes back.
    fn yielded(
        &mut self,
        key: EntityId,
        site: BlockPos,
        kind: PoiType,
        speed: Float,
        output: &mut TickOutput,
    ) {
        let takes = self.find_entity(key).is_some_and(|(_, record)| {
            let Some(provider) = types::of(record.kind).brain() else {
                return false;
            };
            let Ok(mob) = Mob::decode(&record.data) else {
                return false;
            };
            let (shared, own) = villager::state_of(&mob);
            let memories = brain::memories_of(provider, &mob, record.lived);
            let job_site = memories.get(MemoryKind::JobSite);
            shared.age(record.lived) >= 0
                && memories.get(MemoryKind::PotentialJobSite).is_none()
                && (job_site.is_none() || job_site == Some(Value::Block(site)))
                && PoiType::held_by(i32::from(own.data.profession)) == Some(kind)
        });
        if !takes {
            self.release_poi(site, kind, output);
            return;
        }
        let writes = vec![
            MemoryWrite::Set(MemoryKind::PotentialJobSite, Value::Block(site)),
            MemoryWrite::Set(MemoryKind::LookTarget, Value::Tracker(Tracker::Block(site))),
            MemoryWrite::Set(
                MemoryKind::WalkTarget,
                Value::Walk {
                    to: Tracker::Block(site),
                    speed: speed.0,
                    close_enough: 1,
                },
            ),
        ];
        self.write_mob_memories(key, writes, output);
    }

    /// The holder's side of `used` on its raider `key`.
    pub(crate) fn raider_used(&mut self, key: EntityId, used: RaiderUse, output: &mut TickOutput) {
        match used {
            RaiderUse::Glow => {
                let glowing = clustine_world::components::EffectInstance {
                    effect: "minecraft:glowing".to_owned(),
                    amplifier: 0,
                    duration: GLOW_TICKS,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                };
                self.give_mob_effect(key, &glowing, output);
            }
            // The raider's raid is N10d's (section 9.3): until it keeps one, a refusal
            // finds nothing to clear.
            RaiderUse::Rejected { .. } => {}
        }
    }

    /// Changes the region's villager `key`: `change` is given its mob, its own state and
    /// its record's `lived`.
    fn update_villager(
        &mut self,
        key: EntityId,
        output: &mut TickOutput,
        change: impl FnOnce(&mut Mob, &mut villager::VillagerState, u32),
    ) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        let (_, mut own) = villager::state_of(&mob);
        change(&mut mob, &mut own, record.lived);
        villager::update(&mut mob, |kept| *kept = own);
        let fields = mob.changes_from(&record.data);
        if !fields.is_empty() {
            self.commit_entity(chunk, key, fields, output);
        }
    }
}

/// `BehaviorUtils.lockGazeAndWalkToEachOther`'s writes into one of the two: it looks at
/// the other's eyes and walks to it.
fn gaze_at(to: EntityId, speed: Float, close: i32) -> Vec<MemoryWrite> {
    vec![
        MemoryWrite::Set(
            MemoryKind::LookTarget,
            Value::Tracker(Tracker::Entity {
                key: to,
                eyes: true,
                eyes_block: true,
            }),
        ),
        MemoryWrite::Set(
            MemoryKind::WalkTarget,
            Value::Walk {
                to: Tracker::Entity {
                    key: to,
                    eyes: false,
                    eyes_block: false,
                },
                speed: speed.0,
                close_enough: close,
            },
        ),
    ]
}
