//! Beehives and bee nests (`BeehiveBlockEntity`; `docs/groundwork/n7-brief.md`, section
//! 6.4.13 and finding B3), as far as bees need them: a bee that enters is taken into the
//! hive's data and leaves the world, and the hive lets each of its bees out once it has
//! stayed long enough, by day, through its front where the front is clear, a bee that
//! brought nectar raising the honey.
//!
//! The data is the game's (`{bees: [{entity_data, ticks_in_hive, min_ticks_in_hive}],
//! flower_pos}`), which generation already writes into nests. A bee's ticks in its hive
//! are not written every tick: each occupant keeps the game time its ticks were last
//! written at (`clustine_since`, a key of Clustine's own), and its ticks now are counted
//! from there, so that a full hive is not committed every tick; an occupant without one
//! (a generated nest's) is given one the first tick its hive is seen. What Clustine's
//! own bees keep of themselves is a byte array of their own (`clustine_bee`) in the
//! occupant's entity data, beside the game's `id` and `HasNectar`.
//!
//! Named differences: hives tick after the other block entities, each from a stream of
//! its own where the game draws the level's; a bee enters only a hive in its own region
//! (across a line it waits outside); nothing empties a hive in an emergency (a player's
//! break, smoke-less harvesting, fire beside it, an explosion), and a creative player's
//! break drops nothing (the item would need its components); a released bee faces south
//! and has no hive until it finds one, and its age and love are counted on by its ticks
//! inside (`setBeeReleaseData`, **Inferred**: `Occupant` is not in the dump).

use clustine_blocks::data::{facing, number, relative, set};
use clustine_data::{BlockState, EntityTypeInfo, entity_types, sounds};
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::Region;
use super::block_entities::{
    TAG_BYTE, TAG_COMPOUND, TAG_END, TAG_INT, TAG_LIST, TAG_LONG, TAG_STRING, nbt_put_string,
    nbt_string, skip_nbt, string_bucket,
};
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::mobs::Mob;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::bee::{self, BeeState};
use crate::mobs::types::neutral::Anger;
use crate::region::Random;

/// `MAX_OCCUPANTS`.
pub(crate) const MAX_OCCUPANTS: usize = 3;

/// `MIN_OCCUPATION_TICKS_NECTAR` and `_NECTARLESS`.
const STAY_WITH_NECTAR: i32 = 2400;
const STAY_WITHOUT_NECTAR: i32 = 600;

/// The salt of a hive's own stream.
const HIVES: u64 = 0x0068_6976_6573;

const TAG_BYTE_ARRAY: u8 = 7;
const TAG_INT_ARRAY: u8 = 11;

/// An entry of a compound as read: its tag, name and payload.
type Raw = (u8, Vec<u8>, Vec<u8>);

/// The entries of the compound whose entries begin `data`, and what follows its end.
fn entries(data: &[u8]) -> Option<(Vec<Raw>, &[u8])> {
    let mut rest = data;
    let mut found = Vec::new();
    loop {
        let (&tag, after) = rest.split_first()?;
        if tag == TAG_END {
            return Some((found, after));
        }
        let (name, payload) = nbt_string(after)?;
        let next = skip_nbt(tag, payload, 1)?;
        found.push((
            tag,
            name.to_vec(),
            payload[..payload.len() - next.len()].to_vec(),
        ));
        rest = next;
    }
}

/// The entries of a whole block entity's data (one nameless compound).
fn top_entries(data: &[u8]) -> Option<Vec<Raw>> {
    let (&TAG_COMPOUND, rest) = data.split_first()? else {
        return None;
    };
    entries(rest).map(|(found, _)| found)
}

/// A compound's payload of `entries`, in the order of their names' buckets as the game
/// writes a compound (stable within one).
fn write_entries(mut found: Vec<Raw>) -> Vec<u8> {
    found.sort_by_key(|(_, name, _)| string_bucket(&String::from_utf8_lossy(name)));
    let mut data = Vec::new();
    for (tag, name, payload) in found {
        data.push(tag);
        let length = u16::try_from(name.len()).expect("a short name");
        data.extend_from_slice(&length.to_be_bytes());
        data.extend_from_slice(&name);
        data.extend_from_slice(&payload);
    }
    data.push(TAG_END);
    data
}

fn int_of(payload: &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(payload.get(..4)?.try_into().ok()?))
}

fn long_of(payload: &[u8]) -> Option<i64> {
    Some(i64::from_be_bytes(payload.get(..8)?.try_into().ok()?))
}

/// How many bees the hive's data `data` holds.
pub(crate) fn occupant_count(data: &[u8]) -> usize {
    top_entries(data)
        .and_then(|found| {
            found
                .into_iter()
                .find(|(tag, name, _)| *tag == TAG_LIST && name == b"bees")
        })
        .and_then(|(_, _, payload)| {
            let (&element, after) = payload.split_first()?;
            let count = usize::try_from(int_of(after)?).ok()?;
            (element == TAG_COMPOUND || count == 0).then_some(count)
        })
        .unwrap_or(0)
}

/// What a Clustine bee keeps of itself in its hive: what the game saves of it and keeps
/// there (its age, its love, its flags, its anger, its flower, its health).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredBee {
    pub(crate) age: i32,
    pub(crate) love: i32,
    pub(crate) age_locked: bool,
    pub(crate) flags: u8,
    pub(crate) anger: Anger,
    pub(crate) flower: Option<BlockPos>,
    pub(crate) health: f32,
}

/// One bee in a hive (`BeehiveBlockEntity$BeeData`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Occupant {
    pub(crate) ticks_in_hive: i32,
    pub(crate) min_ticks: i32,
    /// The game time `ticks_in_hive` was written at, where it was.
    pub(crate) since: Option<i64>,
    /// The entity data's entries.
    pub(crate) entity: Vec<Raw>,
    /// The occupant's other entries, as read.
    pub(crate) other: Vec<Raw>,
}

impl Occupant {
    /// Its ticks in the hive at the game time `now`, before this tick counts.
    fn ticks(&self, now: i64) -> i32 {
        let gone = self.since.map_or(0, |since| (now - since).max(0));
        i32::try_from(i64::from(self.ticks_in_hive) + gone).unwrap_or(i32::MAX)
    }

    /// `HasNectar` of its entity data.
    fn nectar(&self) -> bool {
        self.entity
            .iter()
            .find(|(tag, name, _)| *tag == TAG_BYTE && name == b"HasNectar")
            .is_some_and(|(_, _, payload)| payload.first().is_some_and(|byte| *byte != 0))
    }

    /// What a Clustine bee kept of itself.
    fn stored(&self) -> Option<StoredBee> {
        let (_, _, payload) = self
            .entity
            .iter()
            .find(|(tag, name, _)| *tag == TAG_BYTE_ARRAY && name == b"clustine_bee")?;
        let length = usize::try_from(int_of(payload)?).ok()?;
        postcard::from_bytes(payload.get(4..4 + length)?).ok()
    }

    fn read(payload: &[u8]) -> Option<Self> {
        let (found, _) = entries(payload)?;
        let mut occupant = Self {
            ticks_in_hive: 0,
            min_ticks: 0,
            since: None,
            entity: Vec::new(),
            other: Vec::new(),
        };
        for (tag, name, value) in found {
            match (tag, name.as_slice()) {
                (TAG_INT, b"ticks_in_hive") => occupant.ticks_in_hive = int_of(&value)?,
                (TAG_INT, b"min_ticks_in_hive") => occupant.min_ticks = int_of(&value)?,
                (TAG_LONG, b"clustine_since") => occupant.since = long_of(&value),
                (TAG_COMPOUND, b"entity_data") => occupant.entity = entries(&value)?.0,
                _ => occupant.other.push((tag, name, value)),
            }
        }
        Some(occupant)
    }

    fn write(&self) -> Vec<u8> {
        let mut found = self.other.clone();
        found.push((
            TAG_INT,
            b"ticks_in_hive".to_vec(),
            self.ticks_in_hive.to_be_bytes().to_vec(),
        ));
        found.push((
            TAG_COMPOUND,
            b"entity_data".to_vec(),
            write_entries(self.entity.clone()),
        ));
        found.push((
            TAG_INT,
            b"min_ticks_in_hive".to_vec(),
            self.min_ticks.to_be_bytes().to_vec(),
        ));
        if let Some(since) = self.since {
            found.push((
                TAG_LONG,
                b"clustine_since".to_vec(),
                since.to_be_bytes().to_vec(),
            ));
        }
        write_entries(found)
    }

    /// `Occupant.of(bee)`: a bee just come in, to stay two minutes with nectar and half a
    /// minute without.
    fn of(stored: &StoredBee, now: i64) -> Self {
        let nectar = stored.flags & bee::flag_bits::HAS_NECTAR != 0;
        let mut id = Vec::new();
        nbt_put_string(&mut id, "minecraft:bee");
        let bytes = postcard::to_allocvec(stored).expect("a bee is encoded into memory");
        let mut array = i32::try_from(bytes.len())
            .expect("a small bee")
            .to_be_bytes()
            .to_vec();
        array.extend_from_slice(&bytes);
        Self {
            ticks_in_hive: 0,
            min_ticks: if nectar {
                STAY_WITH_NECTAR
            } else {
                STAY_WITHOUT_NECTAR
            },
            since: Some(now),
            entity: vec![
                (TAG_STRING, b"id".to_vec(), id),
                (TAG_BYTE, b"HasNectar".to_vec(), vec![u8::from(nectar)]),
                (TAG_BYTE_ARRAY, b"clustine_bee".to_vec(), array),
            ],
            other: Vec::new(),
        }
    }
}

/// A hive's data, as read: its bees, its flower, and the rest as it was.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hive {
    pub(crate) bees: Vec<Occupant>,
    pub(crate) flower: Option<BlockPos>,
    pub(crate) other: Vec<Raw>,
}

impl Hive {
    /// The hive of `data`; a hive placed and never saved has none, and is empty.
    pub(crate) fn read(data: &[u8]) -> Option<Self> {
        let mut hive = Self {
            bees: Vec::new(),
            flower: None,
            other: Vec::new(),
        };
        if data.is_empty() {
            return Some(hive);
        }
        for (tag, name, payload) in top_entries(data)? {
            match (tag, name.as_slice()) {
                (TAG_LIST, b"bees") => {
                    let (&element, after) = payload.split_first()?;
                    let count = usize::try_from(int_of(after)?).ok()?;
                    let mut rest = after.get(4..)?;
                    for _ in 0..count {
                        if element != TAG_COMPOUND {
                            return None;
                        }
                        let next = skip_nbt(TAG_COMPOUND, rest, 1)?;
                        hive.bees
                            .push(Occupant::read(&rest[..rest.len() - next.len()])?);
                        rest = next;
                    }
                }
                (TAG_INT_ARRAY, b"flower_pos") => {
                    let count = int_of(&payload)?;
                    if count == 3 {
                        let at = |i: usize| int_of(payload.get(4 + 4 * i..)?);
                        hive.flower = Some(BlockPos::new(at(0)?, at(1)?, at(2)?));
                    }
                }
                _ => hive.other.push((tag, name, payload)),
            }
        }
        Some(hive)
    }

    pub(crate) fn write(&self) -> Vec<u8> {
        let mut found = self.other.clone();
        let mut list = vec![if self.bees.is_empty() {
            TAG_END
        } else {
            TAG_COMPOUND
        }];
        list.extend_from_slice(
            &i32::try_from(self.bees.len())
                .expect("a few bees")
                .to_be_bytes(),
        );
        for occupant in &self.bees {
            list.extend(occupant.write());
        }
        found.push((TAG_LIST, b"bees".to_vec(), list));
        if let Some(flower) = self.flower {
            let mut array = 3_i32.to_be_bytes().to_vec();
            for value in [flower.x, flower.y, flower.z] {
                array.extend_from_slice(&value.to_be_bytes());
            }
            found.push((TAG_INT_ARRAY, b"flower_pos".to_vec(), array));
        }
        let mut data = vec![TAG_COMPOUND];
        data.extend(write_entries(found));
        data
    }
}

/// `setBeeReleaseData(ticks, bee)` (**Inferred**): a cub grown and a grown bee's cooldown
/// run down by its ticks inside, and its love too.
fn aged(age: i32, ticks: i32) -> i32 {
    if age < 0 {
        (age.saturating_add(ticks)).min(0)
    } else if age > 0 {
        (age.saturating_sub(ticks)).max(0)
    } else {
        0
    }
}

impl Region {
    /// The game time as the hives count by.
    fn hive_time(&self) -> i64 {
        i64::try_from(self.clock.game_time).unwrap_or(i64::MAX)
    }

    /// `BeehiveBlockEntity.addOccupant(bee)` of the region's bee `key` at the hive at
    /// `hive`, which a bee asks for as it goes in: a hive with room keeps it (its flower
    /// the hive's, if the hive has none or by a coin's toss), with the hive's sound at
    /// its corner and `BLOCK_CHANGE`, and the bee leaves the world.
    pub(crate) fn enter_hive(&mut self, key: EntityId, hive: BlockPos, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            return;
        };
        let Some(state) = self.own_block(hive) else {
            return;
        };
        if !bee::is_hive(state) {
            return;
        }
        let Some(mut data) = self.own_block_entity_data(hive).and_then(Hive::read) else {
            return;
        };
        if data.bees.len() >= MAX_OCCUPANTS {
            return;
        }
        let Ok(mob) = Mob::decode(&record.data) else {
            return;
        };
        let now = record.lived;
        let shared = animal::animal_of(&mob.state);
        let own = bee::state_of(&mob);
        let stored = StoredBee {
            age: shared.age(now),
            love: shared.love(now),
            age_locked: shared.age_locked,
            flags: own.flags,
            anger: own.anger.clone(),
            flower: own.flower,
            health: mob.body.vitals.health,
        };
        data.bees.push(Occupant::of(&stored, self.hive_time()));
        if let Some(flower) = own.flower {
            let mut random = self.hive_stream(hive);
            if data.flower.is_none() || random.next_bool() {
                data.flower = Some(flower);
            }
            self.block_streams.insert(hive, random);
        }
        self.set_block_entity_data_as(hive, data.write(), false, output);
        output.events.push(RegionEvent::Sound {
            sound: sounds::BLOCK_BEEHIVE_ENTER,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(
                f64::from(hive.x),
                f64::from(hive.y),
                f64::from(hive.z),
            ),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
        if let Some(vibration) = Vibration::named("minecraft:block_change") {
            self.vibrate_at(
                vibration,
                hive,
                Context::of(Some(Source::of(key)), Some(state)),
            );
        }
        let hops = record.hops;
        self.unride_removed(key, true, output);
        self.remove_entity(key, hops, chunk, output);
    }

    /// A bee's sting of the region's mob `key` (`Bee.doHurtTarget`): of the damage type
    /// `sting`, the sting's sound where it took.
    pub(crate) fn stung_by_bee(
        &mut self,
        bee: EntityId,
        kind: u16,
        from: crate::physics::Vec3,
        key: EntityId,
        amount: f32,
        output: &mut TickOutput,
    ) {
        use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
        if self.find_entity(key).is_none() {
            return;
        }
        let source = DamageSource::by(
            DamageType("minecraft:sting"),
            Causer {
                key: bee,
                kind,
                position: from,
                player: None,
                living: true,
            },
        );
        if self.hurt_mob(key, &source, amount, output) {
            output.events.push(RegionEvent::Sound {
                sound: sounds::ENTITY_BEE_STING,
                source: crate::physics::item::SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(from[0], from[1], from[2]),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
    }

    /// The stream a hive draws from in this tick.
    fn hive_stream(&mut self, pos: BlockPos) -> Random {
        self.block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.config.seed, pos, self.tick, HIVES))
    }

    /// `BeehiveBlockEntity.serverTick` of every hive with bees in the chunks the region
    /// ticks, in the order of chunks and positions: each bee that has stayed long enough
    /// is let out, and an occupied hive hums one tick in 200.
    pub(crate) fn run_hives(&mut self, output: &mut TickOutput) {
        let mut hives = Vec::new();
        for (position, chunk) in &self.chunks {
            if !self.ticks_chunk(*position) {
                continue;
            }
            for (at, entity) in chunk.block_entities() {
                let pos = BlockPos::new(
                    position.x * 16 + i32::from(at.x),
                    i32::from(at.y),
                    position.z * 16 + i32::from(at.z),
                );
                let Some(state) = chunk.get(usize::from(at.x), i32::from(at.y), usize::from(at.z))
                else {
                    continue;
                };
                if bee::is_hive(state) && occupant_count(&entity.data) > 0 {
                    hives.push((pos, state));
                }
            }
        }
        for (pos, state) in hives {
            self.tick_hive(pos, state, output);
        }
    }

    /// One hive's tick.
    fn tick_hive(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let Some(mut hive) = self.own_block_entity_data(pos).and_then(Hive::read) else {
            return;
        };
        let now = self.hive_time();
        let before = hive.clone();
        let mut random = self.hive_stream(pos);
        let mut state = state;
        let mut index = 0;
        while index < hive.bees.len() {
            let occupant = &mut hive.bees[index];
            if occupant.since.is_none() {
                occupant.since = Some(now);
            }
            // `BeeData.tick()`: its ticks count on, and it is due once past its stay.
            if occupant.ticks(now) <= occupant.min_ticks {
                index += 1;
                continue;
            }
            let occupant = occupant.clone();
            let flower = hive.flower;
            match self.release_bee(pos, state, &occupant, flower, &mut random, output) {
                Some(after) => {
                    state = after;
                    hive.bees.remove(index);
                }
                None => index += 1,
            }
        }
        if !hive.bees.is_empty() && random.next_double() < 0.005 {
            output.events.push(RegionEvent::Sound {
                sound: sounds::BLOCK_BEEHIVE_WORK,
                source: SOUND_SOURCE_BLOCKS,
                position: clustine_world::Vec3::new(
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y),
                    f64::from(pos.z) + 0.5,
                ),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
        self.block_streams.insert(pos, random);
        if hive != before {
            self.set_block_entity_data_as(pos, hive.write(), false, output);
        }
    }

    /// `releaseOccupant(...)` of a bee that is due: not while bees stay in by the time
    /// of day, nor through a blocked front; the bee made again before the hive's front
    /// (its honey raised if it brought nectar), with the hive's sound and
    /// `BLOCK_CHANGE`. The hive's state after, where it let the bee out.
    fn release_bee(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        occupant: &Occupant,
        flower: Option<BlockPos>,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> Option<BlockState> {
        if bee::bees_stay_in_hive(self.clock.day) {
            return None;
        }
        let face = facing(state)?;
        let front = relative(pos, face);
        let blocked = self
            .block(front)
            .is_none_or(|ahead| !ahead.voxel_collision_shape().is_empty());
        if blocked {
            return None;
        }
        let honey = occupant.nectar();
        let stored = occupant.stored();
        let ticks = occupant.ticks(self.hive_time());
        let mut mob = Mob::fresh();
        let mut own = BeeState::default();
        let mut shared = Animal::default();
        if let Some(stored) = &stored {
            shared.set_age(aged(stored.age, ticks), 0);
            shared.age_locked = stored.age_locked;
            shared.love_until = u32::try_from(aged(stored.love, ticks).max(0)).unwrap_or(0);
            own.flags = stored.flags;
            own.anger = stored.anger.clone();
            own.flower = stored.flower;
        } else {
            own.set(bee::flag_bits::HAS_NECTAR, honey);
        }
        if let Some(flower) = flower
            && own.flower.is_none()
            && random.next_float() < 0.9
        {
            own.flower = Some(flower);
        }
        let mut after = state;
        if honey {
            own.drop_off_nectar();
            if let Some(level) = number(state, "honey_level").filter(|level| *level < 5) {
                let mut more = if random.next_int(100) == 0 { 2 } else { 1 };
                if level + more > 5 {
                    more -= 1;
                }
                after = set(state, "honey_level", &(level + more).to_string());
                self.set_block(pos, after, output);
            }
        }
        mob.state = animal::write(&shared, &own);
        let kind = entity_types::BEE as u16;
        let info = EntityTypeInfo::of(entity_types::BEE);
        let probe = clustine_world::Entity {
            key: EntityId(0),
            kind,
            uuid: Default::default(),
            position: [0.0; 3],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: mob.encode(),
        };
        let (width, height) = crate::mobs::types::own_size(&probe)
            .map_or((info.width, info.height), |size| (size.width, size.height));
        let step = relative(BlockPos::new(0, 0, 0), face);
        let d = 0.55 + f64::from(width / 2.0);
        let at = [
            f64::from(pos.x) + 0.5 + d * f64::from(step.x),
            f64::from(pos.y) + 0.5 - f64::from(height / 2.0),
            f64::from(pos.z) + 0.5 + d * f64::from(step.z),
        ];
        let mut made = self.new_mob(kind, mob, at, 0.0, 0.0)?;
        if let Some(health) = stored.as_ref().map(|stored| stored.health)
            && let Ok(mut released) = Mob::decode(&made.data)
        {
            released.body.vitals.health = health;
            made.data = released.encode();
        }
        output.events.push(RegionEvent::Sound {
            sound: sounds::BLOCK_BEEHIVE_EXIT,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
        if let Some(vibration) = Vibration::named("minecraft:block_change") {
            self.vibrate_at(
                vibration,
                pos,
                Context::of(Some(Source::of(made.key)), Some(after)),
            );
        }
        self.add_made_mob(made, output);
        Some(after)
    }

    /// The hive's data at `pos`, as read, for the tests.
    #[cfg(test)]
    pub(crate) fn hive(&self, pos: BlockPos) -> Option<Hive> {
        self.own_block_entity_data(pos).and_then(Hive::read)
    }
}

/// An occupant come in at the game time `since` with what `stored` keeps, for the tests.
#[cfg(test)]
pub(crate) fn occupant_for_tests(stored: &StoredBee, since: i64) -> Occupant {
    Occupant::of(stored, since)
}
