//! Typed block entities and their tickers (`docs/adr/0026-redstone.md`, section 5):
//! the block entities whose state the simulation reads or ticks, beside the opaque
//! data every chunk keeps, and the list of tickers the game's
//! `Level.tickBlockEntities` runs after the block events and the entities.
//!
//! The first is the bell, whose ring and shaking live in memory only, as in the game
//! (its NBT keeps neither): a bell placed or loaded starts still. The second is the
//! comparator's (R4), which is durable: its output is written into the chunk's data as
//! the game saves it, `{components: {}, OutputSignal: n}`, whenever it changes, and
//! read back from that data when the chunk is loaded. R5's moving piston comes in as a
//! further kind of [`Typed`], with a ticker as the bell has, and the list stays as it
//! is:
//!
//! - tickers run in the order they were made; one made while the phase runs waits
//!   for the next tick (the game's `pendingBlockEntityTickers`);
//! - a chunk unloaded takes its tickers out of the list, and one loaded puts those of
//!   its block entities at the list's end in the order of their positions in the chunk
//!   (height, then z, then x); a restore, a merge or a split loads its chunks in
//!   ascending order, so builds the list by chunk and then position;
//! - a ticker whose chunk does not tick waits, and one whose block entity went is
//!   dropped as the game drops a removed one.

use clustine_blocks::data::{is_class, same_block};
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::{BlockPos, ChunkPos, InChunk};
use std::collections::BTreeMap;

use super::Region;
use super::containers::read_openers;
use super::hoppers::Timer;
use super::sculk::Sensor;
use super::shulker_boxes::Lid;
use crate::api::TickOutput;

/// How many ticks a bell shakes after a ring (`BellBlockEntity.DURATION`).
pub(crate) const BELL_SHAKING_TICKS: i32 = 50;

/// A block entity whose state the simulation reads or ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typed {
    Bell(Bell),
    /// `ComparatorBlockEntity`: the output it keeps.
    Comparator(u8),
    /// `PistonMovingBlockEntity` (R5): a block on its way, durable as the game saves
    /// it.
    MovingPiston(Moving),
    /// `ShulkerBoxBlockEntity`'s lid (N6), in memory as the game's is; its slots and
    /// openers are in the data (`containers.rs`, `shulker_boxes.rs`).
    ShulkerBox(Lid),
    /// `HopperBlockEntity`'s cooldown and the tick it last ticked in (N6e); its slots
    /// are in the data, and its cooldown too (`hoppers.rs`).
    Hopper(Timer),
    /// `SculkSensorBlockEntity` and the calibrated sensor's (N6f): the vibration it
    /// holds and its travel, its selector and the frequency it last took, durable as
    /// the game saves them (`sculk.rs`).
    Sensor(Sensor),
    /// `DaylightDetectorBlockEntity`, which keeps nothing but ticks
    /// (`daylight_detector.rs`, N8).
    Detector,
}

/// `PistonMovingBlockEntity`'s state: the block it moves, which way and whether out or
/// in, whether it is the piston's own head or base, and how far: its progress in
/// halves of a block, as the game's 0, 0.5 and 1. The game saves the progress before a
/// tick, and a load sets both from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Moving {
    pub(crate) moved: BlockState,
    pub(crate) direction: Face,
    pub(crate) extending: bool,
    pub(crate) source: bool,
    pub(crate) progress: u8,
    pub(crate) progress_before: u8,
    /// The region's tick it last ticked in (`lastTicked`); not saved, as the game does
    /// not save it.
    pub(crate) last_ticked: u64,
}

/// A move's progress when it is done: 1, in halves.
pub(crate) const DONE: u8 = 2;

impl Typed {
    /// The block entity of the kind `state` keeps, if it is a typed one, as `data`
    /// describes it: a new one's data is empty.
    fn of(state: BlockState, data: &[u8]) -> Option<Typed> {
        if is_class(state, BlockClass::BellBlock) {
            return Some(Typed::Bell(Bell::default()));
        }
        if is_class(state, BlockClass::ComparatorBlock) {
            return Some(Typed::Comparator(read_output_signal(data)));
        }
        if is_class(state, BlockClass::MovingPistonBlock) {
            return Some(Typed::MovingPiston(read_moving(data)));
        }
        if is_class(state, BlockClass::ShulkerBoxBlock) {
            return Some(Typed::ShulkerBox(Lid::loaded(read_openers(data) > 0)));
        }
        if is_class(state, BlockClass::HopperBlock) {
            return Some(Typed::Hopper(Timer::loaded(data)));
        }
        if is_class(state, BlockClass::SculkSensorBlock) {
            return Some(Typed::Sensor(Sensor::loaded(data)));
        }
        if is_class(state, BlockClass::DaylightDetectorBlock) {
            return Some(Typed::Detector);
        }
        None
    }

    /// Whether the kind has a ticker (`EntityBlock.getTicker` on the server).
    fn ticks(self) -> bool {
        match self {
            Typed::Bell(_)
            | Typed::MovingPiston(_)
            | Typed::ShulkerBox(_)
            | Typed::Hopper(_)
            | Typed::Sensor(_)
            | Typed::Detector => true,
            Typed::Comparator(_) => false,
        }
    }
}

pub(super) const TAG_END: u8 = 0;
pub(super) const TAG_BYTE: u8 = 1;
pub(super) const TAG_INT: u8 = 3;
pub(super) const TAG_LONG: u8 = 4;
pub(super) const TAG_FLOAT: u8 = 5;
pub(super) const TAG_DOUBLE: u8 = 6;
pub(super) const TAG_STRING: u8 = 8;
pub(super) const TAG_LIST: u8 = 9;
pub(super) const TAG_COMPOUND: u8 = 10;

/// The bucket of a `HashMap` of 16 that Java puts the string `name` in: its
/// `hashCode` over UTF-16, spread as `HashMap.hash` spreads it. A game's compound is
/// such a map, and its entries are written in the order of their buckets, then of
/// their putting in.
pub(super) fn string_bucket(name: &str) -> u32 {
    let hash = name.encode_utf16().fold(0i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    }) as u32;
    (hash ^ (hash >> 16)) & 15
}

/// One entry of a compound being written: its name, its tag and its payload.
pub(super) struct Entry {
    pub(super) name: &'static str,
    pub(super) tag: u8,
    pub(super) payload: Vec<u8>,
}

/// `entries`, put in that order, as the game writes the compound of them: in the order
/// of their buckets, which holds for fewer than thirteen.
pub(super) fn compound(mut entries: Vec<Entry>) -> Vec<u8> {
    entries.sort_by_key(|entry| string_bucket(entry.name));
    let mut data = Vec::new();
    for entry in entries {
        data.push(entry.tag);
        nbt_put_string(&mut data, entry.name);
        data.extend_from_slice(&entry.payload);
    }
    data.push(TAG_END);
    data
}

pub(super) fn nbt_put_string(data: &mut Vec<u8>, text: &str) {
    let length = u16::try_from(text.len()).expect("a short string");
    data.extend_from_slice(&length.to_be_bytes());
    data.extend_from_slice(text.as_bytes());
}

/// A moving piston's block entity as the game saves it (`saveWithoutMetadata`), in
/// NBT's network form: the moved state as `BlockState.CODEC` writes it (its `Name`,
/// and its `Properties` where it has any), its facing as a byte of the legacy id, the
/// progress before a tick as a float, whether it extends and whether it is the source
/// as bytes, and the empty compound of components.
pub(crate) fn moving_data(moving: Moving) -> Vec<u8> {
    let name = moving
        .moved
        .block()
        .map_or("minecraft:air", |block| block.name);
    let mut state = vec![Entry {
        name: "Name",
        tag: TAG_STRING,
        payload: {
            let mut payload = Vec::new();
            nbt_put_string(&mut payload, name);
            payload
        },
    }];
    let properties: Vec<Entry> = moving
        .moved
        .properties()
        .map(|(key, value)| Entry {
            name: key,
            tag: TAG_STRING,
            payload: {
                let mut payload = Vec::new();
                nbt_put_string(&mut payload, value);
                payload
            },
        })
        .collect();
    if !properties.is_empty() {
        state.push(Entry {
            name: "Properties",
            tag: TAG_COMPOUND,
            payload: compound(properties),
        });
    }
    let byte = |value: bool| vec![u8::from(value)];
    let progress = f32::from(moving.progress_before) / 2.0;
    let mut data = vec![TAG_COMPOUND];
    data.extend(compound(vec![
        Entry {
            name: "blockState",
            tag: TAG_COMPOUND,
            payload: compound(state),
        },
        Entry {
            name: "facing",
            tag: TAG_BYTE,
            payload: vec![moving.direction as u8],
        },
        Entry {
            name: "progress",
            tag: TAG_FLOAT,
            payload: progress.to_be_bytes().to_vec(),
        },
        Entry {
            name: "extending",
            tag: TAG_BYTE,
            payload: byte(moving.extending),
        },
        Entry {
            name: "source",
            tag: TAG_BYTE,
            payload: byte(moving.source),
        },
        Entry {
            name: "components",
            tag: TAG_COMPOUND,
            payload: vec![TAG_END],
        },
    ]));
    data
}

/// `PistonMovingBlockEntity.loadAdditional`: what `data` says, with the game's
/// defaults for what it lacks (air, down, 0, not extending, not the source); both
/// progresses from the one saved, rounded down to a half.
fn read_moving(data: &[u8]) -> Moving {
    let mut moving = Moving {
        moved: blocks::AIR,
        direction: Face::Down,
        extending: false,
        source: false,
        progress: 0,
        progress_before: 0,
        last_ticked: 0,
    };
    let Some((&TAG_COMPOUND, rest)) = data.split_first() else {
        return moving;
    };
    let _ = each_entry(rest, 0, &mut |tag, name, payload| match (tag, name) {
        (TAG_COMPOUND, b"blockState") => {
            if let Some(state) = read_block_state(payload) {
                moving.moved = state;
            }
        }
        (TAG_BYTE, b"facing") => {
            let id = payload.first().copied().map_or(0, |id| i32::from(id as i8));
            moving.direction = Face::ALL[(id % 6).unsigned_abs() as usize];
        }
        (TAG_FLOAT, b"progress") => {
            if let Some(bytes) = payload.get(..4) {
                let value = f32::from_be_bytes(bytes.try_into().expect("four bytes"));
                let halves = (value * 2.0).floor().clamp(0.0, 2.0) as u8;
                moving.progress = halves;
                moving.progress_before = halves;
            }
        }
        (TAG_BYTE, b"extending") => moving.extending = payload.first().is_some_and(|b| *b != 0),
        (TAG_BYTE, b"source") => moving.source = payload.first().is_some_and(|b| *b != 0),
        _ => {}
    });
    moving
}

/// `BlockState.CODEC` read from the payload of a compound: its `Name` and its
/// `Properties`; `None` for a block the game does not have.
fn read_block_state(compound: &[u8]) -> Option<BlockState> {
    let mut name = None;
    let mut properties: Vec<(String, String)> = Vec::new();
    each_entry(compound, 1, &mut |tag, key, payload| match (tag, key) {
        (TAG_STRING, b"Name") => {
            name = nbt_string(payload).map(|(text, _)| String::from_utf8_lossy(text).into_owned());
        }
        (TAG_COMPOUND, b"Properties") => {
            let _ = each_entry(payload, 2, &mut |tag, key, value| {
                if tag == TAG_STRING
                    && let Some((value, _)) = nbt_string(value)
                {
                    properties.push((
                        String::from_utf8_lossy(key).into_owned(),
                        String::from_utf8_lossy(value).into_owned(),
                    ));
                }
            });
        }
        _ => {}
    })?;
    let name = name?;
    let text = if properties.is_empty() {
        name
    } else {
        let listed: Vec<String> = properties
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        format!("{name}[{}]", listed.join(","))
    };
    BlockState::parse(&text)
}

/// What [`each_entry`] calls for an entry: with its tag, its name and its payload.
pub(super) type EachEntry<'a, 'f> = dyn FnMut(u8, &[u8], &'a [u8]) + 'f;

/// Calls `each` with the tag, the name and the payload of every entry of the compound
/// whose entries begin `data`, up to its end; returns what follows the end, or `None`
/// for data cut short.
pub(super) fn each_entry<'a>(
    data: &'a [u8],
    depth: u32,
    each: &mut EachEntry<'a, '_>,
) -> Option<&'a [u8]> {
    let mut rest = data;
    loop {
        let (&tag, after) = rest.split_first()?;
        if tag == TAG_END {
            return Some(after);
        }
        let (name, payload) = nbt_string(after)?;
        let after = skip_nbt(tag, payload, depth + 1)?;
        each(tag, name, payload);
        rest = after;
    }
}

/// A comparator's block entity as the game saves it (`saveWithoutMetadata`) and in
/// NBT's network form, one nameless compound with its type byte first: its
/// `OutputSignal` and the empty compound of components every block entity saves, in
/// the order of the game's `HashMap` of the two names (the bucket of `components` is
/// 2 and that of `OutputSignal` 15, in a table of 16).
pub(crate) fn comparator_data(output: u8) -> Vec<u8> {
    let name = |data: &mut Vec<u8>, tag: u8, name: &str| {
        data.push(tag);
        let length = u16::try_from(name.len()).expect("a short name");
        data.extend_from_slice(&length.to_be_bytes());
        data.extend_from_slice(name.as_bytes());
    };
    let mut data = vec![TAG_COMPOUND];
    name(&mut data, TAG_COMPOUND, "components");
    data.push(TAG_END);
    name(&mut data, TAG_INT, "OutputSignal");
    data.extend_from_slice(&i32::from(output).to_be_bytes());
    data.push(TAG_END);
    data
}

/// `ComparatorBlockEntity.loadAdditional`: the `OutputSignal` of the data, 0 where it
/// has none or is not a compound as the game writes one (`getIntOr`), held to what a
/// signal can be.
fn read_output_signal(data: &[u8]) -> u8 {
    let Some((&TAG_COMPOUND, mut rest)) = data.split_first() else {
        return 0;
    };
    while let Some((&tag, after)) = rest.split_first() {
        if tag == TAG_END {
            break;
        }
        let Some((name, after)) = nbt_string(after) else {
            return 0;
        };
        if tag == TAG_INT && name == b"OutputSignal" {
            let Some(value) = after.get(..4) else {
                return 0;
            };
            let value = i32::from_be_bytes(value.try_into().expect("four bytes"));
            return u8::try_from(value.clamp(0, 15)).expect("held to a signal");
        }
        let Some(after) = skip_nbt(tag, after, 0) else {
            return 0;
        };
        rest = after;
    }
    0
}

/// A string as NBT writes one, its length first, and what follows it.
pub(super) fn nbt_string(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = usize::from(u16::from_be_bytes(data.get(..2)?.try_into().ok()?));
    let string = data.get(2..2 + length)?;
    Some((string, &data[2 + length..]))
}

/// What follows the payload of a tag of type `tag` at the start of `data`; `None` for
/// data cut short, a type NBT does not have, or nesting deeper than the game allows.
pub(super) fn skip_nbt(tag: u8, data: &[u8], depth: u32) -> Option<&[u8]> {
    if depth > 512 {
        return None;
    }
    let length = |data: &[u8]| -> Option<usize> {
        usize::try_from(i32::from_be_bytes(data.get(..4)?.try_into().ok()?)).ok()
    };
    match tag {
        1 => data.get(1..),
        2 => data.get(2..),
        3 | 5 => data.get(4..),
        4 | 6 => data.get(8..),
        7 => data.get(4 + length(data)?..),
        8 => nbt_string(data).map(|(_, rest)| rest),
        9 => {
            let (&element, after) = data.split_first()?;
            let count = length(after)?;
            let mut rest = after.get(4..)?;
            for _ in 0..count {
                rest = skip_nbt(element, rest, depth + 1)?;
            }
            Some(rest)
        }
        10 => {
            let mut rest = data;
            loop {
                let (&inner, after) = rest.split_first()?;
                if inner == TAG_END {
                    return Some(after);
                }
                let (_, after) = nbt_string(after)?;
                rest = skip_nbt(inner, after, depth + 1)?;
            }
        }
        11 => data.get(4 + length(data)?.checked_mul(4)?..),
        12 => data.get(4 + length(data)?.checked_mul(8)?..),
        _ => None,
    }
}

/// `BellBlockEntity`'s state on the server: which way it was last struck, whether it
/// shakes and for how long it has, and its resonance with raiders, which waits for
/// entities and so never begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Bell {
    pub(crate) click_direction: Option<Face>,
    pub(crate) shaking: bool,
    pub(crate) ticks: i32,
    pub(crate) resonating: bool,
    pub(crate) resonation_ticks: i32,
}

impl Bell {
    /// `BellBlockEntity.tick` on the server: a shaking bell counts its ticks and stops
    /// at fifty. The search for raiders five ticks into a ring, and the resonance it
    /// starts, need entities other than players and do nothing yet.
    fn tick(&mut self) {
        if self.shaking {
            self.ticks += 1;
        }
        if self.ticks >= BELL_SHAKING_TICKS {
            self.shaking = false;
            self.ticks = 0;
        }
        if self.resonating {
            if self.resonation_ticks < 40 {
                self.resonation_ticks += 1;
            } else {
                self.resonating = false;
            }
        }
    }

    /// `BellBlockEntity.onHit`: struck from `direction`; a bell already shaking starts
    /// its shaking again.
    fn hit(&mut self, direction: Face) {
        self.click_direction = Some(direction);
        if self.shaking {
            self.ticks = 0;
        } else {
            self.shaking = true;
        }
    }

    /// `BellBlockEntity.triggerEvent` of the ring's block event: it shakes from the
    /// start, struck from `direction`.
    fn rung(&mut self, direction: Face) {
        self.resonation_ticks = 0;
        self.click_direction = Some(direction);
        self.ticks = 0;
        self.shaking = true;
    }
}

/// The typed block entities of the loaded chunks and the list of their tickers; no
/// part of the region's state, rebuilt from the chunks as they load.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct BlockEntities {
    typed: BTreeMap<BlockPos, Typed>,
    /// The tickers in the order they run.
    tickers: Vec<BlockPos>,
    /// Tickers made while the phase runs, which join the list at the next.
    pending: Vec<BlockPos>,
    /// Whether the phase of block entities runs (`Level.tickingBlockEntities`).
    ticking: bool,
    /// The sculk sensors whose saved part changed in this tick, written into their
    /// chunks' data once at its end (`sculk.rs`); empty between ticks.
    pub(super) sensors_changed: std::collections::BTreeSet<BlockPos>,
}

impl BlockEntities {
    fn add(&mut self, pos: BlockPos, typed: Typed) {
        let had_ticker = self.typed.insert(pos, typed).is_some_and(Typed::ticks);
        if !typed.ticks() || had_ticker {
            return;
        }
        if self.ticking {
            self.pending.push(pos);
        } else {
            self.tickers.push(pos);
        }
    }

    fn remove(&mut self, pos: BlockPos) {
        if self.typed.remove(&pos).is_some() {
            self.tickers.retain(|ticker| *ticker != pos);
            self.pending.retain(|ticker| *ticker != pos);
        }
    }
}

impl Region {
    /// What a change of `old` to `new` at `pos` makes of the block entity there, as the
    /// game's `LevelChunk.setBlockState` does: a block of another kind drops it, a new
    /// block that keeps a typed one makes it, a block of the same kind keeps it.
    pub(super) fn block_entity_changed(&mut self, pos: BlockPos, old: BlockState, new: BlockState) {
        if same_block(old, new) {
            return;
        }
        self.block_entities.remove(pos);
        if let Some(typed) = Typed::of(new, &[]) {
            self.block_entities.add(pos, typed);
        }
    }

    /// The typed block entities of the chunk at `position`, just loaded, in the order
    /// of their positions, with their tickers at the list's end.
    pub(crate) fn take_up_block_entities(&mut self, position: ChunkPos) {
        let Some(chunk) = self.chunks.get(&position) else {
            return;
        };
        let made: Vec<(BlockPos, Typed)> = chunk
            .block_entities()
            .iter()
            .filter_map(|(at, entity)| {
                let state = chunk.get(usize::from(at.x), i32::from(at.y), usize::from(at.z))?;
                let pos = BlockPos::new(
                    position.x * 16 + i32::from(at.x),
                    i32::from(at.y),
                    position.z * 16 + i32::from(at.z),
                );
                Some((pos, Typed::of(state, &entity.data)?))
            })
            .collect();
        for (pos, typed) in made {
            self.block_entities.add(pos, typed);
        }
    }

    /// Takes the typed block entities of the chunk at `position`, unloaded, and their
    /// tickers away: what they held in memory is lost, as the game loses a bell's ring.
    pub(crate) fn let_go_block_entities(&mut self, position: ChunkPos) {
        let gone: Vec<BlockPos> = self
            .block_entities
            .typed
            .keys()
            .copied()
            .filter(|pos| pos.chunk() == position)
            .collect();
        for pos in gone {
            self.block_entities.remove(pos);
        }
    }

    /// `Level.tickBlockEntities`: the tickers made since the last phase join the list,
    /// and each ticks where its chunk ticks and its block is still of its kind.
    pub(crate) fn run_block_entities(&mut self, output: &mut TickOutput) {
        self.block_entities.ticking = true;
        let pending = std::mem::take(&mut self.block_entities.pending);
        self.block_entities.tickers.extend(pending);
        let tickers = self.block_entities.tickers.clone();
        for pos in tickers {
            if !self.ticks_chunk(pos.chunk()) {
                continue;
            }
            let Some(state) = self.own_block(pos) else {
                continue;
            };
            match self.block_entities.typed.get_mut(&pos) {
                Some(Typed::Bell(bell)) if is_class(state, BlockClass::BellBlock) => bell.tick(),
                Some(Typed::MovingPiston(_)) if is_class(state, BlockClass::MovingPistonBlock) => {
                    self.moving_piston_tick(pos, output);
                }
                Some(Typed::ShulkerBox(_)) if is_class(state, BlockClass::ShulkerBoxBlock) => {
                    self.shulker_box_tick(pos, state, output);
                }
                Some(Typed::Hopper(_)) if is_class(state, BlockClass::HopperBlock) => {
                    self.hopper_tick(pos, state, output);
                }
                Some(Typed::Sensor(_)) if is_class(state, BlockClass::SculkSensorBlock) => {
                    self.sensor_entity_tick(pos, output);
                }
                Some(Typed::Detector) if is_class(state, BlockClass::DaylightDetectorBlock) => {
                    self.daylight_detector_tick(pos, state, output);
                }
                _ => {}
            }
        }
        self.block_entities.ticking = false;
    }

    /// The bell at `pos`, if its block entity is there, for the tests.
    #[cfg(test)]
    pub(crate) fn bell(&self, pos: BlockPos) -> Option<Bell> {
        match self.block_entities.typed.get(&pos)? {
            Typed::Bell(bell) => Some(*bell),
            Typed::Comparator(_)
            | Typed::MovingPiston(_)
            | Typed::ShulkerBox(_)
            | Typed::Hopper(_)
            | Typed::Sensor(_)
            | Typed::Detector => None,
        }
    }

    /// The output the comparator's block entity at `pos` keeps
    /// (`ComparatorBlockEntity.getOutputSignal`): of a loaded chunk, the typed block
    /// entity's; of a copy, read from the copy's data, which the region never writes.
    /// 0 where there is none.
    pub(crate) fn comparator_output(&self, pos: BlockPos) -> u8 {
        if let Some(typed) = self.block_entities.typed.get(&pos) {
            return match typed {
                Typed::Comparator(output) => *output,
                Typed::Bell(_)
                | Typed::MovingPiston(_)
                | Typed::ShulkerBox(_)
                | Typed::Hopper(_)
                | Typed::Sensor(_)
                | Typed::Detector => 0,
            };
        }
        if self.chunks.contains_key(&pos.chunk()) {
            return 0;
        }
        let Some(copy) = self.copies.get(&pos.chunk()) else {
            return 0;
        };
        let (x, z) = pos.in_chunk();
        let is_comparator = copy
            .get(x, pos.y, z)
            .is_some_and(|state| is_class(state, BlockClass::ComparatorBlock));
        match copy.block_entities().get(&InChunk::new(x, pos.y, z)) {
            Some(entity) if is_comparator => read_output_signal(&entity.data),
            _ => 0,
        }
    }

    /// `ComparatorBlockEntity.setOutputSignal` of the comparator at `pos`, written into
    /// its chunk's data and said among the tick's changes when it changes, so that it
    /// is committed with them (`docs/adr/0026-redstone.md`, sections 5 and 6). Returns
    /// the output it kept before, 0 where there is no comparator's block entity.
    pub(crate) fn set_comparator_output(
        &mut self,
        pos: BlockPos,
        output: u8,
        events: &mut TickOutput,
    ) -> u8 {
        let Some(Typed::Comparator(kept)) = self.block_entities.typed.get_mut(&pos) else {
            return 0;
        };
        let before = *kept;
        if before == output {
            return before;
        }
        *kept = output;
        self.set_block_entity_data(pos, comparator_data(output), events);
        before
    }

    /// `BellBlockEntity.onHit` of the bell at `pos`. Returns whether there was one.
    pub(super) fn hit_bell(&mut self, pos: BlockPos, direction: Face) -> bool {
        match self.block_entities.typed.get_mut(&pos) {
            Some(Typed::Bell(bell)) => {
                bell.hit(direction);
                true
            }
            _ => false,
        }
    }

    /// The ring's block event reaching the bell at `pos`: whether it had a bell to
    /// shake (`BaseEntityBlock.triggerEvent`).
    pub(super) fn ring_bell_event(&mut self, pos: BlockPos, direction: Face) -> bool {
        match self.block_entities.typed.get_mut(&pos) {
            Some(Typed::Bell(bell)) => {
                bell.rung(direction);
                true
            }
            _ => false,
        }
    }

    /// The moving piston's block entity at `pos`, if there is one.
    pub(crate) fn moving_piston(&self, pos: BlockPos) -> Option<Moving> {
        match self.block_entities.typed.get(&pos)? {
            Typed::MovingPiston(moving) => Some(*moving),
            _ => None,
        }
    }

    /// Gives the moving piston's block entity at `pos` the state `moving`, and writes
    /// it into the chunk's data, said among the tick's changes to be committed, where
    /// what the game saves of it changed.
    pub(crate) fn set_moving_piston(
        &mut self,
        pos: BlockPos,
        moving: Moving,
        output: &mut TickOutput,
    ) {
        let Some(Typed::MovingPiston(kept)) = self.block_entities.typed.get_mut(&pos) else {
            return;
        };
        let before = *kept;
        *kept = moving;
        let saved = |moving: Moving| {
            (
                moving.moved,
                moving.direction,
                moving.extending,
                moving.source,
                moving.progress_before,
            )
        };
        if saved(before) != saved(moving) {
            self.set_block_entity_data(pos, moving_data(moving), output);
        }
    }

    /// The lid of the shulker box at `pos`, if its block entity is there.
    pub(crate) fn shulker_lid(&self, pos: BlockPos) -> Option<Lid> {
        match self.block_entities.typed.get(&pos)? {
            Typed::ShulkerBox(lid) => Some(*lid),
            _ => None,
        }
    }

    pub(super) fn shulker_lid_mut(&mut self, pos: BlockPos) -> Option<&mut Lid> {
        match self.block_entities.typed.get_mut(&pos)? {
            Typed::ShulkerBox(lid) => Some(lid),
            _ => None,
        }
    }

    /// The cooldown of the hopper at `pos`, if its block entity is there.
    pub(crate) fn hopper_timer(&self, pos: BlockPos) -> Option<Timer> {
        match self.block_entities.typed.get(&pos)? {
            Typed::Hopper(timer) => Some(*timer),
            _ => None,
        }
    }

    pub(super) fn hopper_timer_mut(&mut self, pos: BlockPos) -> Option<&mut Timer> {
        match self.block_entities.typed.get_mut(&pos)? {
            Typed::Hopper(timer) => Some(timer),
            _ => None,
        }
    }

    /// The sculk sensor's block entity at `pos`, if it is there.
    pub(crate) fn sensor(&self, pos: BlockPos) -> Option<Sensor> {
        match self.block_entities.typed.get(&pos)? {
            Typed::Sensor(sensor) => Some(*sensor),
            _ => None,
        }
    }

    /// Gives the sculk sensor's block entity at `pos` the state `sensor`, its saved
    /// part to be written at the tick's end where it changed.
    pub(super) fn set_sensor(&mut self, pos: BlockPos, sensor: Sensor) {
        if let Some(Typed::Sensor(kept)) = self.block_entities.typed.get_mut(&pos) {
            if kept.saved() != sensor.saved() {
                self.block_entities.sensors_changed.insert(pos);
            }
            *kept = sensor;
        }
    }

    /// The positions of the sculk sensors' block entities whose blocks are within
    /// `radius` of `origin` along each axis, ascending: the listeners a game event
    /// there may reach (`GameEventDispatcher.post` visits the sections about it).
    pub(super) fn sensors_about(&self, origin: BlockPos, radius: i32) -> Vec<BlockPos> {
        let low = BlockPos::new(origin.x - radius, i32::MIN, i32::MIN);
        let high = BlockPos::new(origin.x + radius, i32::MAX, i32::MAX);
        self.block_entities
            .typed
            .range(low..=high)
            .filter(|(pos, typed)| {
                matches!(typed, Typed::Sensor(_))
                    && (pos.y - origin.y).abs() <= radius
                    && (pos.z - origin.z).abs() <= radius
            })
            .map(|(pos, _)| *pos)
            .collect()
    }

    /// The sculk sensors whose saved part changed in this tick, taken.
    pub(super) fn take_sensors_changed(&mut self) -> Vec<BlockPos> {
        std::mem::take(&mut self.block_entities.sensors_changed)
            .into_iter()
            .collect()
    }

    /// The positions of the shulker boxes' block entities of the chunk at `position`,
    /// in order.
    pub(super) fn shulker_boxes_in(&self, position: ChunkPos) -> Vec<BlockPos> {
        self.block_entities
            .typed
            .iter()
            .filter(|(pos, typed)| pos.chunk() == position && matches!(typed, Typed::ShulkerBox(_)))
            .map(|(pos, _)| *pos)
            .collect()
    }

    /// The game's `removeBlockEntityIfSame` and `setRemoved`: the typed block entity
    /// at `pos` goes from memory and its ticker with it; the chunk drops its data when
    /// its block changes.
    pub(crate) fn remove_typed(&mut self, pos: BlockPos) {
        self.block_entities.remove(pos);
    }

    /// The positions of the tickers in the order they run, for the tests.
    #[cfg(test)]
    pub(crate) fn tickers(&self) -> Vec<BlockPos> {
        self.block_entities.tickers.clone()
    }
}

#[cfg(test)]
mod moving_tests {
    use super::*;

    /// A moving piston's data reads back as it was written: the moved block with its
    /// properties, the facing, the progress before a tick (into both, as the game
    /// loads it) and the two flags; data that is no compound reads as the game's
    /// defaults.
    #[test]
    fn a_moving_pistons_data_reads_back_as_it_was_written() {
        let moved = BlockState::parse(
            "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
        )
        .expect("a state");
        for (progress, source, extending) in [(0, false, true), (1, true, false), (2, false, false)]
        {
            let moving = Moving {
                moved,
                direction: Face::South,
                extending,
                source,
                progress: 2,
                progress_before: progress,
                last_ticked: 77,
            };
            let read = read_moving(&moving_data(moving));
            assert_eq!(read.moved, moved);
            assert_eq!(read.direction, Face::South);
            assert_eq!((read.extending, read.source), (extending, source));
            assert_eq!((read.progress, read.progress_before), (progress, progress));
            assert_eq!(read.last_ticked, 0, "not saved");
        }
        let empty = read_moving(&[]);
        assert_eq!((empty.moved, empty.direction), (blocks::AIR, Face::Down));
    }

    /// The game's compound is a `HashMap`: its entries go by the buckets of their
    /// names' hashes, which for these names are the order the jar's saved data has.
    #[test]
    fn the_names_of_a_moving_pistons_data_go_in_the_order_of_their_buckets() {
        // What the comparator's data shows of the game's buckets (R4, read in the jar).
        assert_eq!(string_bucket("components"), 2);
        assert_eq!(string_bucket("OutputSignal"), 15);
        let names = [
            "blockState",
            "facing",
            "progress",
            "extending",
            "source",
            "components",
        ];
        let mut ordered = names.to_vec();
        ordered.sort_by_key(|name| string_bucket(name));
        let data = moving_data(Moving {
            moved: blocks::STONE,
            direction: Face::Up,
            extending: true,
            source: false,
            progress: 0,
            progress_before: 0,
            last_ticked: 0,
        });
        let mut found = Vec::new();
        let _ = each_entry(&data[1..], 0, &mut |_, name, _| {
            found.push(String::from_utf8_lossy(name).into_owned());
        });
        assert_eq!(found, ordered);
    }
}
