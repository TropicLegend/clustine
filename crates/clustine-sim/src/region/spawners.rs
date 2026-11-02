//! Monster spawners (`SpawnerBlockEntity` and its `BaseSpawner`; n7 BaseSpawner): the
//! spawner's block entity ticks, and while a player is near it counts its delay down
//! and then spawns its next mob about it, up to its count of tries, as many as its
//! box of nearby ones lets; then it draws its delay again and tells its clients, who
//! spin the mob in its cage, to start theirs over.
//!
//! **What it keeps.** The spawner's state is its block entity's data, as the game saves
//! it (`BaseSpawner.save`, n7 BaseSpawner 689–745): its delay, its settings, the next
//! spawn's data and the weighted potentials. Each tick that changes it writes it back
//! ([`Spawner::write`]), so that it goes with the chunk through commits, merges, splits
//! and take-overs, and the data is all there is of it: nothing is kept in memory but
//! the ticker ([`super::block_entities::Typed::Spawner`]). A change of the delay alone
//! is committed and not sent (the client counts its own delay down and is told to start
//! it over by the block event); a new next spawn is sent, as the game's
//! `setNextSpawnData` sends the block's update (`SpawnerBlockEntity`'s spawner).
//!
//! A spawner whose block entity has no data yet (one just placed) is the game's new
//! `BaseSpawner` (n7 BaseSpawner 51–79): its defaults and no potentials. Data read from
//! a save is `load`'s (618–687): what is missing is its default, and missing
//! potentials are the next spawn alone, of weight 1.
//!
//! **Its draws** are the level's random in the game (`serverTick` and `delay` draw from
//! it); here a stream of the spawner's block and the region's tick, deterministic.
//! Within a tick they come in the game's order: the position's four doubles and its
//! height, the type's rule, the yaw, the mob's `finalizeSpawn`, and the delay's.
//!
//! **Players.** A spawner works while a living player who is no spectator stands within
//! its `RequiredPlayerRange` of its middle (`Level.hasNearbyAlivePlayer`, n5
//! EntityGetter 229–269): the players natural spawning reads, the region's own, the
//! faces it keeps of players across its lines and the world's players the store last
//! published in its dimension (`spawn_caps.rs`). A player in another region within range
//! so starts a spawner here as it would in one world: by their face, as it is a tick
//! late, or by the world's table, as it was last published, where the region keeps no
//! face of them. Only the region's own players are known to be alive; another's dead
//! player counts.
//!
//! Named differences:
//! - The game rule `spawner_blocks_work` is on (Clustine has none of it); `spawn_mobs`
//!   does not stop a spawner, as it does not in the game.
//! - A spawn's entity data beyond its `id` (and `Pos`) is not read: the mob is made as
//!   a fresh one of its type and, as the game skips it for such data, without its
//!   `finalizeSpawn`. A spawn data's `equipment` is not given. A type Clustine does not
//!   build, or that is no mob (an item, a minecart), is the game's entity that could not
//!   be loaded: the spawner draws its delay again.
//! - Mobs nearby are counted by their type, where the game counts those of the same
//!   class (`EntityTypeTest.forExactClass`): the same count wherever a type has a class
//!   of its own, which was not checked for every type.
//! - A spawn whose place is in a chunk the region does not tick (across a line) is a
//!   try that failed, as a box that collides is.
//! - `custom_spawn_rules` is `SpawnData.CustomSpawnRules.isValidPosition` as the game's
//!   source has it (the block light and the sky light at the spawn within their
//!   ranges), which the class dumps do not hold.
//! - Trial spawners are separate and stay inert (`TrialSpawnerBlockEntity`).

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, BlockState, DEFAULT_ATTRIBUTES, EntityTypeInfo, MobCategory};
use clustine_world::{BlockPos, ChunkPos, Difficulty};

use super::block_entities::{
    TAG_COMPOUND, TAG_DOUBLE, TAG_END, TAG_INT, TAG_LIST, TAG_STRING, each_entry, hash_map_order,
    nbt_put_string, nbt_string, skip_nbt,
};
use super::natural_spawning::spawn_box;
use super::vibrations::{Context, Source, Vibration};
use super::{Random, Region};
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::types::{self, SpawnReason};
use crate::physics::aabb::Aabb;
use crate::spawning::rules::{self, Reason};

const TAG_SHORT: u8 = 2;

/// `BaseSpawner`'s defaults (n7 BaseSpawner 1–79).
const DEFAULT_DELAY: i32 = 20;
const DEFAULT_MIN_SPAWN_DELAY: i32 = 200;
const DEFAULT_MAX_SPAWN_DELAY: i32 = 800;
const DEFAULT_SPAWN_COUNT: i32 = 4;
const DEFAULT_MAX_NEARBY_ENTITIES: i32 = 6;
const DEFAULT_REQUIRED_PLAYER_RANGE: i32 = 16;
const DEFAULT_SPAWN_RANGE: i32 = 4;

/// `BaseSpawner.EVENT_SPAWN`: the block event that starts the clients' delay over.
pub(crate) const EVENT_SPAWN: i32 = 1;

/// `LevelEvent.PARTICLES_MOBBLOCK_SPAWN`: the smoke and flames of a spawn at the
/// spawner (`serverTick` 658–664).
pub(crate) const LEVEL_EVENT_SPAWN: i32 = 2004;

/// `Mob.spawnAnim`'s entity event on the server (n7 Mob 552–566): the poof about the
/// mob.
const SPAWN_ANIMATION: u8 = 20;

/// The mark of a spawner's stream among a block's.
const DRAWS: u64 = 0x0073_7061_776e_6572;

/// The names `BaseSpawner.save` writes, in the order it puts them.
const DELAY: &str = "Delay";
const MIN_SPAWN_DELAY: &str = "MinSpawnDelay";
const MAX_SPAWN_DELAY: &str = "MaxSpawnDelay";
const SPAWN_COUNT: &str = "SpawnCount";
const MAX_NEARBY_ENTITIES: &str = "MaxNearbyEntities";
const REQUIRED_PLAYER_RANGE: &str = "RequiredPlayerRange";
const SPAWN_RANGE: &str = "SpawnRange";
const SPAWN_DATA: &str = "SpawnData";
const SPAWN_POTENTIALS: &str = "SpawnPotentials";
const KNOWN: [&str; 9] = [
    DELAY,
    MIN_SPAWN_DELAY,
    MAX_SPAWN_DELAY,
    SPAWN_COUNT,
    MAX_NEARBY_ENTITIES,
    REQUIRED_PLAYER_RANGE,
    SPAWN_RANGE,
    SPAWN_DATA,
    SPAWN_POTENTIALS,
];

/// One entry of a compound as it was read: its tag, its name and its payload.
#[derive(Debug, Clone, PartialEq)]
struct Raw {
    tag: u8,
    name: String,
    payload: Vec<u8>,
}

/// The entries of the compound whose entries begin `body`, as they were read; `None`
/// for data cut short.
fn entries(body: &[u8]) -> Option<Vec<Raw>> {
    let mut found = Vec::new();
    let mut rest = body;
    loop {
        let (&tag, after) = rest.split_first()?;
        if tag == TAG_END {
            return Some(found);
        }
        let (name, payload) = nbt_string(after)?;
        let next = skip_nbt(tag, payload, 1)?;
        found.push(Raw {
            tag,
            name: String::from_utf8_lossy(name).into_owned(),
            payload: payload[..payload.len() - next.len()].to_vec(),
        });
        rest = next;
    }
}

/// The payload of a compound of `entries`, put in that order, as the game's `HashMap`
/// writes it (`hash_map_order`).
fn compound_payload(entries: &[Raw]) -> Vec<u8> {
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    let mut out = Vec::new();
    for index in hash_map_order(&names) {
        let entry = &entries[index];
        out.push(entry.tag);
        nbt_put_string(&mut out, &entry.name);
        out.extend_from_slice(&entry.payload);
    }
    out.push(TAG_END);
    out
}

/// A numeric tag's value as `NumericTag.longValue()` gives it; `None` for any other.
fn numeric(tag: u8, payload: &[u8]) -> Option<i64> {
    Some(match tag {
        1 => i64::from(*payload.first()? as i8),
        2 => i64::from(i16::from_be_bytes(payload.get(..2)?.try_into().ok()?)),
        3 => i64::from(i32::from_be_bytes(payload.get(..4)?.try_into().ok()?)),
        4 => i64::from_be_bytes(payload.get(..8)?.try_into().ok()?),
        5 => f32::from_be_bytes(payload.get(..4)?.try_into().ok()?) as i64,
        6 => f64::from_be_bytes(payload.get(..8)?.try_into().ok()?) as i64,
        _ => return None,
    })
}

/// One spawn's data (`SpawnData`): the entity to spawn and its custom rules, kept as
/// the payload of its compound as it was read, so that it is written back unchanged.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpawnData {
    payload: Vec<u8>,
}

/// `SpawnData.CustomSpawnRules`: the ranges of block light and sky light a spawn may
/// have, both in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CustomRules {
    block: (i32, i32),
    sky: (i32, i32),
}

impl CustomRules {
    /// `isValidPosition`: the block light and the sky light at the spawn in range.
    fn allows(self, block: i32, sky: i32) -> bool {
        (self.block.0..=self.block.1).contains(&block) && (self.sky.0..=self.sky.1).contains(&sky)
    }
}

impl SpawnData {
    /// `new SpawnData()`: an empty entity, no rules, no equipment.
    fn empty() -> Self {
        let mut payload = vec![TAG_COMPOUND];
        nbt_put_string(&mut payload, "entity");
        payload.push(TAG_END);
        payload.push(TAG_END);
        Self { payload }
    }

    /// A spawn of the entity whose type is named `id` and nothing else.
    pub(crate) fn of_type(id: &str) -> Self {
        let mut entity = vec![TAG_STRING];
        nbt_put_string(&mut entity, "id");
        nbt_put_string(&mut entity, id);
        entity.push(TAG_END);
        Self::of_entries(&[Raw {
            tag: TAG_COMPOUND,
            name: "entity".to_owned(),
            payload: entity,
        }])
    }

    fn of_entries(entries: &[Raw]) -> Self {
        Self {
            payload: compound_payload(entries),
        }
    }

    /// The data of a `SpawnData` compound's payload, if `SpawnData.CODEC` reads it: it
    /// has its `entity` compound.
    fn read(payload: &[u8]) -> Option<Self> {
        let read = entries(payload)?;
        read.iter()
            .any(|entry| entry.tag == TAG_COMPOUND && entry.name == "entity")
            .then(|| Self {
                payload: payload.to_vec(),
            })
    }

    /// The entries of the entity to spawn (`getEntityToSpawn`).
    fn entity(&self) -> Vec<Raw> {
        entries(&self.payload)
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry.tag == TAG_COMPOUND && entry.name == "entity")
            .and_then(|entity| entries(&entity.payload))
            .unwrap_or_default()
    }

    /// The type the entity's `id` names (`EntityType.by`): a bare name is the game's
    /// own, as an `Identifier` reads it.
    pub(crate) fn kind(&self) -> Option<u16> {
        let entity = self.entity();
        let id = entity
            .iter()
            .find(|entry| entry.tag == TAG_STRING && entry.name == "id")?;
        let (name, _) = nbt_string(&id.payload)?;
        let name = String::from_utf8_lossy(name);
        let name = if name.contains(':') {
            name.into_owned()
        } else {
            format!("minecraft:{name}")
        };
        let index = clustine_data::ENTITY_TYPES
            .iter()
            .position(|kind| *kind == name)?;
        u16::try_from(index).ok()
    }

    /// Whether the entity is its `id` alone (`getEntityToSpawn().size() == 1` with an
    /// `id`): a mob of such data is finalised (`serverTick` 554–616).
    fn id_alone(&self) -> bool {
        let entity = self.entity();
        entity.len() == 1 && entity[0].tag == TAG_STRING && entity[0].name == "id"
    }

    /// The entity's `Pos`, where it has three doubles of it (`Vec3.CODEC`).
    fn position(&self) -> Option<[f64; 3]> {
        let entity = self.entity();
        let pos = entity
            .iter()
            .find(|entry| entry.tag == TAG_LIST && entry.name == "Pos")?;
        let (&element, rest) = pos.payload.split_first()?;
        let count = i32::from_be_bytes(rest.get(..4)?.try_into().ok()?);
        if element != TAG_DOUBLE || count != 3 {
            return None;
        }
        let value = |index: usize| -> Option<f64> {
            let at = 4 + index * 8;
            Some(f64::from_be_bytes(rest.get(at..at + 8)?.try_into().ok()?))
        };
        Some([value(0)?, value(1)?, value(2)?])
    }

    /// The spawn's custom rules, where it has them (`custom_spawn_rules`): each range a
    /// number or a compound of `min_inclusive` and `max_inclusive` (`InclusiveRange`'s
    /// codec), 0 to 15 where it is missing.
    fn custom_rules(&self) -> Option<CustomRules> {
        let read = entries(&self.payload)?;
        let rules = read
            .iter()
            .find(|entry| entry.tag == TAG_COMPOUND && entry.name == "custom_spawn_rules")?;
        let mut found = CustomRules {
            block: (0, 15),
            sky: (0, 15),
        };
        for entry in entries(&rules.payload)? {
            let range = if let Some(value) = numeric(entry.tag, &entry.payload) {
                let value = value as i32;
                Some((value, value))
            } else if entry.tag == TAG_COMPOUND {
                let bounds = entries(&entry.payload).unwrap_or_default();
                let bound = |name: &str| {
                    bounds
                        .iter()
                        .find(|bound| bound.name == name)
                        .and_then(|bound| numeric(bound.tag, &bound.payload))
                        .map(|value| value as i32)
                };
                bound("min_inclusive").zip(bound("max_inclusive"))
            } else {
                None
            };
            match (entry.name.as_str(), range) {
                ("block_light_limit", Some(range)) => found.block = range,
                ("sky_light_limit", Some(range)) => found.sky = range,
                _ => {}
            }
        }
        Some(found)
    }
}

/// One of the spawner's weighted potentials, its compound kept as it was read.
#[derive(Debug, Clone, PartialEq)]
struct Potential {
    data: SpawnData,
    weight: i32,
    payload: Vec<u8>,
}

impl Potential {
    /// A potential of `data` and `weight`, as `Weighted.codec` writes it.
    fn new(data: SpawnData, weight: i32) -> Self {
        let payload = compound_payload(&[
            Raw {
                tag: TAG_COMPOUND,
                name: "data".to_owned(),
                payload: data.payload.clone(),
            },
            Raw {
                tag: TAG_INT,
                name: "weight".to_owned(),
                payload: weight.to_be_bytes().to_vec(),
            },
        ]);
        Self {
            data,
            weight,
            payload,
        }
    }
}

/// A spawner's state (`BaseSpawner`), as its data keeps it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spawner {
    pub(crate) delay: i32,
    pub(crate) min_spawn_delay: i32,
    pub(crate) max_spawn_delay: i32,
    pub(crate) spawn_count: i32,
    pub(crate) max_nearby_entities: i32,
    pub(crate) required_player_range: i32,
    pub(crate) spawn_range: i32,
    pub(crate) next: Option<SpawnData>,
    potentials: Vec<Potential>,
    /// What else the data has (the block entity's `components`), written back as it
    /// was after the spawner's own.
    others: Vec<Raw>,
}

impl Spawner {
    /// A new spawner (`new BaseSpawner()` and the block entity's empty components).
    fn fresh() -> Self {
        Self {
            delay: DEFAULT_DELAY,
            min_spawn_delay: DEFAULT_MIN_SPAWN_DELAY,
            max_spawn_delay: DEFAULT_MAX_SPAWN_DELAY,
            spawn_count: DEFAULT_SPAWN_COUNT,
            max_nearby_entities: DEFAULT_MAX_NEARBY_ENTITIES,
            required_player_range: DEFAULT_REQUIRED_PLAYER_RANGE,
            spawn_range: DEFAULT_SPAWN_RANGE,
            next: None,
            potentials: Vec::new(),
            others: vec![Raw {
                tag: TAG_COMPOUND,
                name: "components".to_owned(),
                payload: vec![TAG_END],
            }],
        }
    }

    /// The spawner the block entity's data `data` keeps: a new one where it has none
    /// (or none the game could read), else `BaseSpawner.load`'s (n7 BaseSpawner
    /// 621–687): each number read as any numeric tag, or its default; the next spawn
    /// where `SpawnData` reads; the potentials, or the next spawn alone of weight 1.
    pub(crate) fn read(data: &[u8]) -> Self {
        let Some((&TAG_COMPOUND, body)) = data.split_first() else {
            return Self::fresh();
        };
        let Some(read) = entries(body) else {
            return Self::fresh();
        };
        let number = |name: &str| {
            read.iter()
                .find(|entry| entry.name == name)
                .and_then(|entry| numeric(entry.tag, &entry.payload))
        };
        // `getShortOr` and `getIntOr`: a short's value, an int's.
        let short =
            |name: &str, default: i32| number(name).map_or(default, |v| i32::from(v as i16));
        let int = |name: &str, default: i32| number(name).map_or(default, |v| v as i32);
        let next = read
            .iter()
            .find(|entry| entry.tag == TAG_COMPOUND && entry.name == SPAWN_DATA)
            .and_then(|entry| SpawnData::read(&entry.payload));
        let potentials = read
            .iter()
            .find(|entry| entry.tag == TAG_LIST && entry.name == SPAWN_POTENTIALS)
            .and_then(|entry| read_potentials(&entry.payload))
            .unwrap_or_else(|| {
                vec![Potential::new(
                    next.clone().unwrap_or_else(SpawnData::empty),
                    1,
                )]
            });
        Self {
            delay: short(DELAY, DEFAULT_DELAY),
            min_spawn_delay: int(MIN_SPAWN_DELAY, DEFAULT_MIN_SPAWN_DELAY),
            max_spawn_delay: int(MAX_SPAWN_DELAY, DEFAULT_MAX_SPAWN_DELAY),
            spawn_count: int(SPAWN_COUNT, DEFAULT_SPAWN_COUNT),
            max_nearby_entities: int(MAX_NEARBY_ENTITIES, DEFAULT_MAX_NEARBY_ENTITIES),
            required_player_range: int(REQUIRED_PLAYER_RANGE, DEFAULT_REQUIRED_PLAYER_RANGE),
            spawn_range: int(SPAWN_RANGE, DEFAULT_SPAWN_RANGE),
            next,
            potentials,
            others: read
                .into_iter()
                .filter(|entry| !KNOWN.contains(&entry.name.as_str()))
                .collect(),
        }
    }

    /// The data the game saves of the spawner (`BaseSpawner.save` then the block
    /// entity's components, n7 BaseSpawner 689–745): its numbers as shorts, the next
    /// spawn where there is one, the potentials, and what else the data had.
    pub(crate) fn write(&self) -> Vec<u8> {
        let short = |name: &str, value: i32| Raw {
            tag: TAG_SHORT,
            name: name.to_owned(),
            payload: (value as i16).to_be_bytes().to_vec(),
        };
        let mut put = vec![
            short(DELAY, self.delay),
            short(MIN_SPAWN_DELAY, self.min_spawn_delay),
            short(MAX_SPAWN_DELAY, self.max_spawn_delay),
            short(SPAWN_COUNT, self.spawn_count),
            short(MAX_NEARBY_ENTITIES, self.max_nearby_entities),
            short(REQUIRED_PLAYER_RANGE, self.required_player_range),
            short(SPAWN_RANGE, self.spawn_range),
        ];
        if let Some(next) = &self.next {
            put.push(Raw {
                tag: TAG_COMPOUND,
                name: SPAWN_DATA.to_owned(),
                payload: next.payload.clone(),
            });
        }
        // An empty list's element type is `TAG_END`, as the game's `ListTag` writes it.
        let mut list = vec![if self.potentials.is_empty() {
            TAG_END
        } else {
            TAG_COMPOUND
        }];
        let count = i32::try_from(self.potentials.len()).unwrap_or(i32::MAX);
        list.extend_from_slice(&count.to_be_bytes());
        for potential in &self.potentials {
            list.extend_from_slice(&potential.payload);
        }
        put.push(Raw {
            tag: TAG_LIST,
            name: SPAWN_POTENTIALS.to_owned(),
            payload: list,
        });
        put.extend(self.others.iter().cloned());
        let mut data = vec![TAG_COMPOUND];
        data.extend(compound_payload(&put));
        data
    }

    /// The spawner with its next spawn of the type named `id` and nothing else
    /// (`setEntityId` on a new spawner, whose `getOrCreateNextSpawnData` finds no
    /// potentials and so draws nothing).
    pub(crate) fn with_type(mut self, id: &str) -> Self {
        self.next = Some(SpawnData::of_type(id));
        self
    }

    /// `WeightedList.getRandom`: one `nextInt(total)` and the potential it falls in;
    /// nothing, and no draw, where the weights come to nought.
    fn pick(&self, draws: &mut Random) -> Option<SpawnData> {
        let total: i64 = self
            .potentials
            .iter()
            .map(|potential| i64::from(potential.weight))
            .sum();
        let total = u32::try_from(total).ok().filter(|total| *total > 0)?;
        let mut left = i64::from(draws.next_int(total));
        self.potentials.iter().find_map(|potential| {
            left -= i64::from(potential.weight);
            (left < 0).then(|| potential.data.clone())
        })
    }
}

/// `SpawnData.LIST_CODEC` of a list's payload: compounds each of a `data` that
/// `SpawnData.CODEC` reads and a `weight` of no less than 0; `None` where any is not.
fn read_potentials(payload: &[u8]) -> Option<Vec<Potential>> {
    let (&element, rest) = payload.split_first()?;
    let count = usize::try_from(i32::from_be_bytes(rest.get(..4)?.try_into().ok()?)).ok()?;
    if count == 0 {
        return Some(Vec::new());
    }
    if element != TAG_COMPOUND {
        return None;
    }
    let mut rest = rest.get(4..)?;
    let mut found = Vec::with_capacity(count);
    for _ in 0..count {
        let next = skip_nbt(TAG_COMPOUND, rest, 1)?;
        let whole = &rest[..rest.len() - next.len()];
        let mut data = None;
        let mut weight = None;
        each_entry(whole, 2, &mut |tag, name, value| match (tag, name) {
            (TAG_COMPOUND, b"data") => {
                let end = skip_nbt(TAG_COMPOUND, value, 3).map_or(0, <[u8]>::len);
                data = SpawnData::read(&value[..value.len() - end]);
            }
            (_, b"weight") => weight = numeric(tag, value).map(|weight| weight as i32),
            _ => {}
        })?;
        let weight = weight.filter(|weight| *weight >= 0)?;
        found.push(Potential {
            data: data?,
            weight,
            payload: whole.to_vec(),
        });
        rest = next;
    }
    Some(found)
}

/// What a spawner's tick did with its state.
struct Ticked {
    spawner: Spawner,
    /// Whether its next spawn was set, which its clients are sent.
    shown: bool,
}

impl Region {
    /// `SpawnerBlockEntity.serverTick` of the spawner `state` at `pos`: `BaseSpawner`'s
    /// (n7 BaseSpawner 223–562), its data written back where it changed.
    pub(crate) fn spawner_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let Some(data) = self.own_block_entity_data(pos) else {
            return;
        };
        let before = Spawner::read(data);
        // `isNearPlayer` (112–134), and `isSpawnerBlockEnabled`, which is always so.
        if !self.near_alive_player(pos, before.required_player_range) {
            return;
        }
        let mut ticked = Ticked {
            spawner: before.clone(),
            shown: false,
        };
        let mut level = Random::of_block(self.seed(), pos, self.tick, DRAWS);
        if ticked.spawner.delay == -1 {
            self.spawner_delay(pos, state, &mut ticked, &mut level);
        }
        if ticked.spawner.delay > 0 {
            ticked.spawner.delay -= 1;
        } else {
            self.spawner_spawn(pos, state, &mut ticked, &mut level, output);
        }
        if ticked.spawner != before || ticked.shown {
            let data = ticked.spawner.write();
            self.set_block_entity_data_as(pos, data, ticked.shown, output);
        }
    }

    /// `hasNearbyAlivePlayer` at the spawner's middle: a player who is no spectator and
    /// alive nearer than `range` (any distance for a range below 0).
    fn near_alive_player(&self, pos: BlockPos, range: i32) -> bool {
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        let range = f64::from(range);
        self.spawning_players().iter().any(|near| {
            let dead = self
                .players
                .get(&near.player)
                .is_some_and(|own| own.health <= 0.0);
            let d: [f64; 3] = std::array::from_fn(|axis| near.position[axis] - centre[axis]);
            let distance = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            !near.spectator && !dead && (range < 0.0 || distance < range * range)
        })
    }

    /// `delay` (578–619): the delay drawn between the least and the most, the next
    /// spawn drawn from the potentials where they have any weight, and the block event
    /// that starts the clients' delay over (`broadcastEvent`, the block entity's
    /// `level.blockEvent(pos, SPAWNER, 1, 0)`).
    fn spawner_delay(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        ticked: &mut Ticked,
        level: &mut Random,
    ) {
        let spawner = &mut ticked.spawner;
        spawner.delay = if spawner.max_spawn_delay <= spawner.min_spawn_delay {
            spawner.min_spawn_delay
        } else {
            let span = u32::try_from(spawner.max_spawn_delay - spawner.min_spawn_delay)
                .expect("the most is above the least");
            spawner.min_spawn_delay + level.next_int(span) as i32
        };
        if let Some(next) = spawner.pick(level) {
            spawner.next = Some(next);
            ticked.shown = true;
        }
        self.block_event(pos, state, EVENT_SPAWN, 0);
    }

    /// The spawning of `serverTick` once the delay is out (249–562): the next spawn's
    /// data (drawn from the potentials where there is none yet), then `SpawnCount`
    /// tries, each a place about the spawner that is free, which the rules allow, and
    /// where the nearby mobs of the type are fewer than `MaxNearbyEntities`; a mob made
    /// there, shown with the spawner's smoke and its own poof; and the delay drawn again
    /// where any came.
    fn spawner_spawn(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        ticked: &mut Ticked,
        level: &mut Random,
        output: &mut TickOutput,
    ) {
        // `getOrCreateNextSpawnData` (814–835).
        let next = match &ticked.spawner.next {
            Some(next) => next.clone(),
            None => {
                let next = ticked.spawner.pick(level).unwrap_or_else(SpawnData::empty);
                ticked.spawner.next = Some(next.clone());
                ticked.shown = true;
                next
            }
        };
        let custom = next.custom_rules();
        let range = f64::from(ticked.spawner.spawn_range);
        let mut spawned = false;
        for _ in 0..ticked.spawner.spawn_count {
            // `EntityType.by`: no type, and the delay is drawn again (120–139).
            let Some(kind) = next.kind() else {
                self.spawner_delay(pos, state, ticked, level);
                return;
            };
            let position = next.position().unwrap_or_else(|| {
                let x =
                    f64::from(pos.x) + (level.next_double() - level.next_double()) * range + 0.5;
                let y = f64::from(pos.y + level.next_int(3) as i32 - 1);
                let z =
                    f64::from(pos.z) + (level.next_double() - level.next_double()) * range + 0.5;
                [x, y, z]
            });
            let chunk = ChunkPos::containing(position[0], position[2]);
            if !self.ticks_chunk(chunk) || !self.no_collision(&spawn_box(kind, position)) {
                continue;
            }
            let block = crate::physics::containing(position);
            let world = self.spawn_world();
            if let Some(custom) = custom {
                let friendly = EntityTypeInfo::of(i32::from(kind)).category != MobCategory::Monster;
                if !friendly && Difficulty::WORLD == Difficulty::Peaceful {
                    continue;
                }
                let Some(light) = self.light_at(block) else {
                    continue;
                };
                if !custom.allows(i32::from(light.block), i32::from(light.sky)) {
                    continue;
                }
            } else if !rules::check_spawn_rules_as(kind, &world, Reason::Spawner, block, level) {
                continue;
            }
            // `loadEntityRecursive`: what Clustine cannot make is the game's entity that
            // could not be loaded (named).
            if !makes(kind) {
                self.spawner_delay(pos, state, ticked, level);
                return;
            }
            let around = Aabb::of_block(pos).inflate([range; 3]);
            if self.nearby_of_type(kind, &around) >= ticked.spawner.max_nearby_entities {
                self.spawner_delay(pos, state, ticked, level);
                return;
            }
            let yaw = level.next_float() * 360.0;
            // `Mob.checkSpawnRules` where the spawn has no rules of its own, then
            // `checkSpawnObstruction`.
            if custom.is_none() && !rules::mob_check_spawn_rules(kind, &world, block) {
                continue;
            }
            if !self.spawn_unobstructed(kind, position) {
                continue;
            }
            let made = self.make_spawned_mob(
                kind,
                (position, yaw),
                (SpawnReason::Spawner, next.id_alone()),
                &mut None,
                level,
                output,
            );
            // `tryAddFreshEntityWithPassengers` failing: no key left.
            let Some(key) = made else {
                self.spawner_delay(pos, state, ticked, level);
                return;
            };
            output.events.push(RegionEvent::LevelEvent {
                kind: LEVEL_EVENT_SPAWN,
                position: pos,
                data: 0,
            });
            let context = Context::of(Some(Source::of(key)), None);
            self.vibrate_at(Vibration::ENTITY_PLACE, block, context);
            if let Some((chunk, _)) = self.find_entity(key) {
                output.events.push(RegionEvent::EntityEvent {
                    entity: key,
                    chunk,
                    event: SPAWN_ANIMATION,
                });
            }
            spawned = true;
        }
        if spawned {
            self.spawner_delay(pos, state, ticked, level);
        }
    }

    /// How many entities of the type `kind` that are no spectators the box `around`
    /// holds, of the region's chunks and its copies (`getEntities` with
    /// `EntityTypeTest.forExactClass`, by the type, named).
    fn nearby_of_type(&self, kind: u16, around: &Aabb) -> i32 {
        let low = ChunkPos::containing(around.min[0] - 2.0, around.min[2] - 2.0);
        let high = ChunkPos::containing(around.max[0] + 2.0, around.max[2] + 2.0);
        let mut count = 0;
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(ChunkPos::new(x, z)) else {
                    continue;
                };
                count += chunk
                    .entities()
                    .iter()
                    .filter(|entity| {
                        entity.kind == kind && self.entity_box(entity).intersects(*around)
                    })
                    .count();
            }
        }
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// `Mob.checkSpawnObstruction` of a fresh mob of the type `kind` at `position`: the
    /// liquid its type minds in its box, and no entity that blocks building nor player
    /// in it (`natural_spawning.rs`).
    fn spawn_unobstructed(&self, kind: u16, position: [f64; 3]) -> bool {
        let info = EntityTypeInfo::of(i32::from(kind));
        let bounds = Aabb::of_entity(position, info.width, info.height);
        let blocks = (
            crate::physics::containing(bounds.min),
            crate::physics::containing(bounds.max),
        );
        let at = crate::physics::containing(position);
        rules::obstruction_by_blocks(kind, &self.spawn_world(), at, blocks)
            && self.unobstructed(&bounds)
    }

    /// `SpawnerBlockEntity.triggerEvent` of the spawner `state` at `at`: the event that
    /// starts the delay over is one, which the clients are sent (`onEventTriggered`,
    /// 790–805); any other is not. `None` for a block that is no spawner.
    pub(super) fn spawner_event(&self, at: BlockPos, state: BlockState, kind: i32) -> Option<bool> {
        if !is_class(state, BlockClass::SpawnerBlock) {
            return None;
        }
        Some(kind == EVENT_SPAWN && self.own_block_entity_data(at).is_some())
    }
}

/// Whether Clustine makes an entity of the type `kind` as a spawner's: a living mob of
/// a type it builds.
fn makes(kind: u16) -> bool {
    let living = DEFAULT_ATTRIBUTES
        .get(usize::from(kind))
        .is_some_and(Option::is_some);
    let id = i32::from(kind);
    living
        && id != clustine_data::entity_types::PLAYER
        && id != clustine_data::entity_types::ARMOR_STAND
        && types::is_built(kind)
}

#[cfg(test)]
#[path = "spawners_tests.rs"]
mod tests;
