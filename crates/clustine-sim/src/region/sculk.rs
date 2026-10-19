//! Sculk sensors and calibrated sculk sensors (step N6f of `docs/groundwork/n6-plan.md`;
//! `docs/groundwork/n6-brief.md`, sections 8.8 to 8.12), as the 26.3 jar's
//! `SculkSensorBlock`, `CalibratedSculkSensorBlock` and their block entities have them:
//! the block's phases and power, its block entity, which holds the vibration it took up
//! and keeps it as the game saves it, and what the vibration does when it arrives.
//!
//! What hears a vibration, and how it travels, is `vibrations.rs`'s. Here:
//!
//! - a sensor that receives one becomes `active` with the power its distance gives
//!   (`max(1, 15 - floor(15 / radius * distance))`), schedules its tick, tells the blocks
//!   around it and around the block below it (which it powers strongly), sets off the
//!   amethyst blocks beside it, and clicks; 30 ticks on (10 for a calibrated one) it
//!   goes to `cooldown` with no power, and 10 more to `inactive`, which is the only
//!   phase that hears;
//! - a comparator reads the frequency it last took while it is `active`;
//! - an entity that stands on an inactive sensor sets it off, sneaking or not
//!   (`stepOn`), with what it would hear of a step there;
//! - a calibrated sensor hears only the frequency that comes into its back while a
//!   signal does, gives no signal out of the back, and hears twice as far.
//!
//! The block entity is durable: what the game saves of it (`last_vibration_frequency`,
//! and its `listener`: the vibration held, its selector and the travel left) is written
//! into the chunk's data once at the end of a tick that changed it, and read back when
//! the chunk loads, which sends the held vibration's particle again from where it had
//! got to. Named differences: the entity of a vibration is not saved (the game saves its
//! UUID), so one that a sensor holds over a load is received with none, which only the
//! clicking's own event carries on; the particle sent again after a load is sent once,
//! where the game sends it again until a player is near enough to see it.

use clustine_blocks::data::{facing, flag, in_tag, is_class, number, relative, set, tick_id};
use clustine_blocks::note;
use clustine_blocks::signal::{self, Query};
use clustine_data::{BlockClass, BlockState, Face, sounds};
use clustine_world::{BlockPos, Double, EntityId, Float, InChunk, Ticked};

use super::Region;
use super::block_entities::{
    Entry, TAG_COMPOUND, TAG_DOUBLE, TAG_END, TAG_FLOAT, TAG_INT, TAG_LIST, TAG_LONG, TAG_STRING,
    compound, each_entry, nbt_put_string, nbt_string,
};
use super::redstone::play;
use super::updates::RegionView;
use super::vibrations::{Context, Source, Vibration, block_centre};
use crate::api::TickOutput;

/// `SculkSensorBlock.ACTIVE_TICKS` and the calibrated sensor's `getActiveTicks`.
const ACTIVE_TICKS: i32 = 30;
const CALIBRATED_ACTIVE_TICKS: i32 = 10;
/// `SculkSensorBlock.COOLDOWN_TICKS`.
const COOLDOWN_TICKS: i32 = 10;
/// `SculkSensorBlockEntity.VibrationUser.LISTENER_RANGE`, and the calibrated user's.
const LISTENER_RANGE: i32 = 8;
const CALIBRATED_LISTENER_RANGE: i32 = 16;

/// `SculkSensorBlock.RESONANCE_PITCH_BEND`'s notes, by frequency.
const RESONANCE_NOTES: [u8; 16] = [0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24];

/// A vibration as a sensor holds it (`VibrationInfo`): the event, how far it was from
/// the sensor's centre, where it was, and the entity it came of, kept in memory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Heard {
    pub(crate) event: Vibration,
    pub(crate) distance: Float,
    pub(crate) pos: [Double; 3],
    pub(crate) source: Option<EntityId>,
}

impl Heard {
    pub(crate) fn position(&self) -> [f64; 3] {
        self.pos.map(|value| value.0)
    }
}

/// A sculk sensor's block entity (`SculkSensorBlockEntity` and its
/// `VibrationSystem.Data`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Sensor {
    /// `lastVibrationFrequency`.
    pub(crate) last_frequency: u8,
    /// `currentVibration`: the vibration on its way to the sensor.
    pub(crate) current: Option<Heard>,
    /// `travelTimeInTicks`.
    pub(crate) travel: i32,
    /// `VibrationSelector`'s candidate and the game time it came in.
    pub(crate) selector: Option<(Heard, i64)>,
    /// `reloadVibrationParticle`: set by a load, until the particle is sent again.
    pub(crate) reload_particle: bool,
}

/// What the game saves of a sensor, told apart for the writing of its data.
type Saved = (u8, Option<SavedHeard>, i32, Option<(SavedHeard, i64)>);
type SavedHeard = (Vibration, Float, [Double; 3]);

impl Sensor {
    /// The part of the sensor the game saves.
    pub(crate) fn saved(&self) -> Saved {
        let heard = |heard: &Heard| (heard.event, heard.distance, heard.pos);
        (
            self.last_frequency,
            self.current.as_ref().map(heard),
            self.travel,
            self.selector
                .as_ref()
                .map(|(info, tick)| (heard(info), *tick)),
        )
    }

    /// `SculkSensorBlockEntity.loadAdditional`: what `data` saved, a vibration held to be
    /// shown again; a fresh one where it saved no listener.
    pub(crate) fn loaded(data: &[u8]) -> Sensor {
        let mut sensor = Sensor::default();
        let Some((&TAG_COMPOUND, rest)) = data.split_first() else {
            return sensor;
        };
        let _ = each_entry(rest, 0, &mut |tag, name, payload| match (tag, name) {
            (TAG_INT, b"last_vibration_frequency") => {
                sensor.last_frequency = read_i32(payload).map_or(0, |n| n.clamp(0, 15) as u8);
            }
            (TAG_COMPOUND, b"listener") => {
                sensor.reload_particle = true;
                let _ = each_entry(payload, 1, &mut |tag, name, payload| match (tag, name) {
                    (TAG_COMPOUND, b"event") => sensor.current = read_heard(payload),
                    (TAG_INT, b"event_delay") => {
                        sensor.travel = read_i32(payload).map_or(0, |n| n.max(0));
                    }
                    (TAG_COMPOUND, b"selector") => {
                        let (mut event, mut tick) = (None, -1);
                        let _ =
                            each_entry(payload, 2, &mut |tag, name, payload| match (tag, name) {
                                (TAG_COMPOUND, b"event") => event = read_heard(payload),
                                (TAG_LONG, b"tick") => tick = read_i64(payload).unwrap_or(-1),
                                _ => {}
                            });
                        sensor.selector = event.map(|event| (event, tick));
                    }
                    _ => {}
                });
            }
            _ => {}
        });
        sensor
    }

    /// `SculkSensorBlockEntity.saveAdditional`, in NBT's network form: the frequency, the
    /// listener (`VibrationSystem.Data.CODEC`: the vibration held where there is one,
    /// the selector, whose `tick` is -1 while it holds nothing, and the travel left) and
    /// the empty compound of components.
    pub(crate) fn data(&self) -> Vec<u8> {
        let mut selector = Vec::new();
        if let Some((heard, _)) = &self.selector {
            selector.push(heard_entry(heard));
        }
        let tick = self.selector.map_or(-1, |(_, tick)| tick);
        selector.push(Entry {
            name: "tick",
            tag: TAG_LONG,
            payload: tick.to_be_bytes().to_vec(),
        });
        let mut listener = Vec::new();
        if let Some(heard) = &self.current {
            listener.push(heard_entry(heard));
        }
        listener.push(Entry {
            name: "selector",
            tag: TAG_COMPOUND,
            payload: compound(selector),
        });
        listener.push(Entry {
            name: "event_delay",
            tag: TAG_INT,
            payload: self.travel.max(0).to_be_bytes().to_vec(),
        });
        let mut data = vec![TAG_COMPOUND];
        data.extend(compound(vec![
            Entry {
                name: "last_vibration_frequency",
                tag: TAG_INT,
                payload: i32::from(self.last_frequency).to_be_bytes().to_vec(),
            },
            Entry {
                name: "listener",
                tag: TAG_COMPOUND,
                payload: compound(listener),
            },
            Entry {
                name: "components",
                tag: TAG_COMPOUND,
                payload: vec![TAG_END],
            },
        ]));
        data
    }
}

/// The entry `event` of a `VibrationInfo`: its event by name, its distance and its
/// position; the source's UUID is not kept.
fn heard_entry(heard: &Heard) -> Entry {
    let mut name = Vec::new();
    nbt_put_string(&mut name, heard.event.name());
    let mut pos = vec![TAG_DOUBLE];
    pos.extend_from_slice(&3i32.to_be_bytes());
    for value in heard.pos {
        pos.extend_from_slice(&value.0.to_be_bytes());
    }
    Entry {
        name: "event",
        tag: TAG_COMPOUND,
        payload: compound(vec![
            Entry {
                name: "game_event",
                tag: TAG_STRING,
                payload: name,
            },
            Entry {
                name: "distance",
                tag: TAG_FLOAT,
                payload: heard.distance.0.to_be_bytes().to_vec(),
            },
            Entry {
                name: "pos",
                tag: TAG_LIST,
                payload: pos,
            },
        ]),
    }
}

/// A `VibrationInfo` read from the payload of its compound: `None` where it names no
/// event the game has, or lacks its position (the codec would fail, and the lenient
/// field reads as absent).
fn read_heard(payload: &[u8]) -> Option<Heard> {
    let (mut event, mut distance, mut pos) = (None, Float(0.0), None);
    each_entry(payload, 3, &mut |tag, name, payload| match (tag, name) {
        (TAG_STRING, b"game_event") => {
            event = nbt_string(payload)
                .and_then(|(text, _)| std::str::from_utf8(text).ok())
                .and_then(Vibration::named);
        }
        (TAG_FLOAT, b"distance") => {
            if let Some(bytes) = payload.get(..4) {
                let value = f32::from_be_bytes(bytes.try_into().expect("four bytes"));
                distance = Float(value.max(0.0));
            }
        }
        // A list of three doubles.
        (TAG_LIST, b"pos")
            if payload.first() == Some(&TAG_DOUBLE) && read_i32(&payload[1..]) == Some(3) =>
        {
            let at = |index: usize| {
                let start = 5 + index * 8;
                payload
                    .get(start..start + 8)
                    .map(|bytes| Double(f64::from_be_bytes(bytes.try_into().expect("eight"))))
            };
            if let (Some(x), Some(y), Some(z)) = (at(0), at(1), at(2)) {
                pos = Some([x, y, z]);
            }
        }
        _ => {}
    })?;
    Some(Heard {
        event: event?,
        distance,
        pos: pos?,
        source: None,
    })
}

fn read_i32(payload: &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(payload.get(..4)?.try_into().ok()?))
}

fn read_i64(payload: &[u8]) -> Option<i64> {
    Some(i64::from_be_bytes(payload.get(..8)?.try_into().ok()?))
}

/// Whether `state` is a sculk sensor of either kind.
pub(crate) fn is_sensor(state: BlockState) -> bool {
    is_class(state, BlockClass::SculkSensorBlock)
}

fn is_calibrated(state: BlockState) -> bool {
    is_class(state, BlockClass::CalibratedSculkSensorBlock)
}

/// `SculkSensorBlock.canActivate`: whether it is `inactive`, the one phase that hears.
pub(crate) fn can_activate(state: BlockState) -> bool {
    is_sensor(state) && state.property("sculk_sensor_phase") == Some("inactive")
}

/// `getListenerRadius` of the sensor `state`'s user.
pub(crate) fn listener_radius(state: BlockState) -> i32 {
    if is_calibrated(state) {
        CALIBRATED_LISTENER_RANGE
    } else {
        LISTENER_RANGE
    }
}

/// `VibrationSystem.getRedstoneStrengthForDistance`.
pub(crate) fn strength_for_distance(distance: f32, radius: i32) -> u8 {
    let lost = (15.0 / f64::from(radius) * f64::from(distance)).floor();
    (15.0 - lost).max(1.0) as u8
}

impl Region {
    /// `VibrationUser.canReceiveVibration` of the sensor `state` at `pos`, for `event`
    /// whose block is `at`: a calibrated sensor that a signal comes into from behind
    /// hears only that frequency; no sensor hears its own placing or breaking, nor an
    /// event without a frequency; and only an inactive one hears.
    pub(super) fn sensor_can_receive(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        at: BlockPos,
        event: Vibration,
    ) -> bool {
        if is_calibrated(state)
            && let Some(facing) = facing(state)
        {
            let behind = facing.opposite();
            let back = signal::signal(
                &mut RegionView(self),
                relative(pos, behind),
                behind,
                Query::ALL,
            );
            if back != 0 && event.frequency() != back {
                return false;
            }
        }
        if at == pos && (event == Vibration::BLOCK_DESTROY || event == Vibration::BLOCK_PLACE) {
            return false;
        }
        event.frequency() != 0 && can_activate(state)
    }

    /// `VibrationUser.onReceiveVibration` of the sensor at `pos`: an inactive one keeps
    /// the frequency and activates with the power of `distance`, between the event's
    /// block and its own.
    pub(super) fn sensor_received(
        &mut self,
        pos: BlockPos,
        heard: &Heard,
        distance: f32,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if !can_activate(state) {
            return;
        }
        let frequency = heard.event.frequency();
        if let Some(mut sensor) = self.sensor(pos) {
            sensor.last_frequency = frequency;
            self.set_sensor(pos, sensor);
        }
        let power = strength_for_distance(distance, listener_radius(state));
        self.activate_sensor(heard.source, pos, state, power, frequency, output);
    }

    /// `SculkSensorBlock.activate`: active with `power`, its tick scheduled, its
    /// neighbours and those of the block below told, the amethyst beside it set off, and
    /// the click heard where it is not under water.
    fn activate_sensor(
        &mut self,
        source: Option<EntityId>,
        pos: BlockPos,
        state: BlockState,
        power: u8,
        frequency: u8,
        output: &mut TickOutput,
    ) {
        let active = set(
            set(state, "sculk_sensor_phase", "active"),
            "power",
            &power.to_string(),
        );
        self.set_block(pos, active, output);
        let ticks = if is_calibrated(state) {
            CALIBRATED_ACTIVE_TICKS
        } else {
            ACTIVE_TICKS
        };
        self.schedule_tick(pos, Ticked::Block(tick_id(state)), ticks, 0);
        self.sensor_updates_neighbours(pos, active, output);
        self.resonate(source, pos, frequency, output);
        let source = source.map(Source::of);
        self.vibrate_at(
            Vibration::SCULK_SENSOR_TENDRILS_CLICKING,
            pos,
            Context::of(source, None),
        );
        if !flag(state, "waterlogged") {
            let pitch = self.next_draw().next_float() * 0.2 + 0.8;
            play(
                pos,
                sounds::BLOCK_SCULK_SENSOR_CLICKING,
                1.0,
                pitch,
                None,
                output,
            );
        }
    }

    /// `SculkSensorBlock.tryResonateVibration`: each amethyst block beside the sensor,
    /// in the order of the directions, makes the resonance of `frequency` heard where it
    /// is, which sensors about it take up as that frequency.
    fn resonate(
        &mut self,
        source: Option<EntityId>,
        pos: BlockPos,
        frequency: u8,
        output: &mut TickOutput,
    ) {
        let Some(event) = Vibration::resonance(frequency) else {
            return;
        };
        for face in Face::ALL {
            let beside = relative(pos, face);
            let Some(state) = self.block(beside) else {
                continue;
            };
            if !in_tag(state, "minecraft:vibration_resonators") {
                continue;
            }
            let context = Context::of(source.map(Source::of), Some(state));
            self.vibrate_at(event, beside, context);
            let pitch = note::pitch(RESONANCE_NOTES[usize::from(frequency)]);
            play(
                beside,
                sounds::BLOCK_AMETHYST_BLOCK_RESONATE,
                1.0,
                pitch,
                None,
                output,
            );
        }
    }

    /// `SculkSensorBlock.updateNeighbours`: the blocks around the sensor, then those
    /// around the block below it, which it powers.
    fn sensor_updates_neighbours(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        self.update_neighbours_at(pos, state, output);
        self.update_neighbours_at(relative(pos, Face::Down), state, output);
    }

    /// `SculkSensorBlock.tick`: an active sensor goes to its cooldown without power and
    /// tells its neighbours; one cooling down becomes inactive again, with a click where
    /// it is not under water. Whether `state` is a sensor.
    pub(super) fn sensor_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if !is_sensor(state) {
            return false;
        }
        match state.property("sculk_sensor_phase") {
            Some("active") => {
                let cooling = set(set(state, "sculk_sensor_phase", "cooldown"), "power", "0");
                self.set_block(pos, cooling, output);
                let block = Ticked::Block(tick_id(state));
                self.schedule_tick(pos, block, COOLDOWN_TICKS, 0);
                self.sensor_updates_neighbours(pos, cooling, output);
            }
            Some("cooldown") => {
                self.set_block(pos, set(state, "sculk_sensor_phase", "inactive"), output);
                if !flag(state, "waterlogged") {
                    let pitch = self.next_draw().next_float() * 0.2 + 0.8;
                    let sound = sounds::BLOCK_SCULK_SENSOR_CLICKING_STOP;
                    play(pos, sound, 1.0, pitch, None, output);
                }
            }
            _ => {}
        }
        true
    }

    /// `SculkSensorBlock.onPlace`: a sensor that comes with power and no tick to take it
    /// away (pasted, or loaded so) loses its power, telling only the clients.
    pub(super) fn sensor_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_sensor(state) || clustine_blocks::data::same_block(state, old) {
            return;
        }
        let ticked = Ticked::Block(tick_id(state));
        if number(state, "power").unwrap_or(0) > 0 && !self.has_scheduled_tick(pos, ticked) {
            let flags = super::updates::UPDATE_CLIENTS | super::updates::UPDATE_KNOWN_SHAPE;
            let unpowered = set(state, "power", "0");
            self.set_block_updated(pos, unpowered, flags, super::updates::UPDATE_LIMIT, output);
        }
    }

    /// `SculkSensorBlock.affectNeighborsAfterRemoval`: an active sensor that goes tells
    /// the blocks it powered.
    pub(super) fn sensor_after_removal(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if is_sensor(old) && old.property("sculk_sensor_phase") == Some("active") {
            self.sensor_updates_neighbours(pos, old, output);
        }
    }

    /// `SculkSensorBlock.getAnalogOutputSignal`: the frequency the sensor last took
    /// while it is active, and 0 otherwise; of a copy, read from the copy's data.
    /// `None` for a block that is not a sensor.
    pub(crate) fn sensor_output(&self, pos: BlockPos, state: BlockState) -> Option<u8> {
        if !is_sensor(state) {
            return None;
        }
        if state.property("sculk_sensor_phase") != Some("active") {
            return Some(0);
        }
        if let Some(sensor) = self.sensor(pos) {
            return Some(sensor.last_frequency);
        }
        let (x, z) = pos.in_chunk();
        let data = self
            .copies
            .get(&pos.chunk())
            .and_then(|copy| copy.block_entities().get(&InChunk::new(x, pos.y, z)))
            .map(|entity| entity.data.as_slice())
            .unwrap_or_default();
        Some(Sensor::loaded(data).last_frequency)
    }

    /// `SculkSensorBlock.stepOn` of the block at `pos` by the entity `entity`, its feet
    /// at `feet`: an inactive sensor that would hear a step on itself takes it up at
    /// once, with neither the entity's sneaking nor wool nor anything between asked. An
    /// entity of another region's stepping on it is not heard (named, as other blocks'
    /// `stepOn` across a line is).
    pub(crate) fn sensor_stepped_on(
        &mut self,
        pos: BlockPos,
        entity: Option<EntityId>,
        feet: [f64; 3],
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if !can_activate(state) || self.sensor(pos).is_none() {
            return;
        }
        if !self.sensor_can_receive(pos, state, pos, Vibration::STEP) {
            return;
        }
        self.schedule_vibration(pos, Vibration::STEP, feet, entity);
    }

    /// Writes the saved part of each sensor that changed in this tick into its chunk's
    /// data, to be committed with the tick's changes and never sent: the game sends a
    /// sensor's block entity to no client.
    pub(crate) fn save_sensors(&mut self, output: &mut TickOutput) {
        for pos in self.take_sensors_changed() {
            if let Some(sensor) = self.sensor(pos) {
                self.set_block_entity_data_as(pos, sensor.data(), false, output);
            }
        }
    }
}

/// The centre of the sensor's block at `pos`, where its listener is
/// (`BlockPositionSource`).
pub(crate) fn listener_position(pos: BlockPos) -> [f64; 3] {
    block_centre(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sensor's data reads back as it was written, but for the vibration's entity,
    /// which is not saved, and with the particle to be sent again.
    #[test]
    fn a_sensors_data_reads_back_as_it_was_written() {
        let heard = Heard {
            event: Vibration::BLOCK_PLACE,
            distance: Float(4.5),
            pos: [Double(1.5), Double(-3.25), Double(7.0)],
            source: Some(EntityId(9)),
        };
        let sensor = Sensor {
            last_frequency: 13,
            current: Some(heard),
            travel: 3,
            selector: Some((
                Heard {
                    event: Vibration::STEP,
                    ..heard
                },
                41,
            )),
            reload_particle: false,
        };
        let read = Sensor::loaded(&sensor.data());
        assert_eq!(read.saved(), sensor.saved());
        assert_eq!(read.current.and_then(|heard| heard.source), None);
        assert!(read.reload_particle);
        let fresh = Sensor::loaded(&Sensor::default().data());
        assert_eq!(fresh.saved(), Sensor::default().saved());
        assert_eq!(
            Sensor::loaded(&[]),
            Sensor::default(),
            "nothing saved: fresh"
        );
    }

    /// `getRedstoneStrengthForDistance` at a sensor's radius of 8: 15 close by, one
    /// less for each 8/15 of a block, and never below 1.
    #[test]
    fn the_power_of_a_vibration_falls_with_its_distance_and_stays_at_least_one() {
        assert_eq!(strength_for_distance(0.0, 8), 15);
        assert_eq!(strength_for_distance(0.53, 8), 15);
        assert_eq!(strength_for_distance(0.54, 8), 14);
        assert_eq!(strength_for_distance(5.0, 8), 6);
        assert_eq!(strength_for_distance(7.5, 8), 1);
        assert_eq!(strength_for_distance(8.0, 8), 1);
        assert_eq!(strength_for_distance(8.0, 16), 8);
    }
}
