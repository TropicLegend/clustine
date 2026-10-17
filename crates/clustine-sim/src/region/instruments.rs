//! Note blocks and bells (step R3b of `docs/adr/0026-redstone.md`), as the 26.3 jar's
//! `NoteBlock`, `BellBlock` and `BellBlockEntity` have them:
//!
//! - a note block plays when power rises beside it and when a player clicks it, which
//!   tunes it a semitone up first; it plays only where the block above is air, unless
//!   a head above gives its sound. Its play is a block event (kind 0), and the event's
//!   `triggerEvent` plays the instrument's sound at the note's pitch, heard by
//!   everyone near, which the client shows with its particle;
//! - a bell rings when power rises beside it, the way it faces, and when a player hits
//!   it on a side it swings from, away from that side. Its block entity shakes, a
//!   block event (kind 1) tells the clients which way, and it shakes for fifty ticks
//!   in the phase of block entities (`block_entities.rs`).
//!
//! A left click on a note block plays it only in survival: the game's
//! `ServerPlayerGameMode.handleBlockBreakAction` destroys the block at once for a
//! player who builds instantly, without its `attack`, and every Clustine player is in
//! creative mode.

use clustine_blocks::data::{flag, is_class, number, relative, set, set_flag};
use clustine_blocks::note::{self, Instrument};
use clustine_blocks::signal;
use clustine_data::{BlockClass, BlockState, Face, sounds};
use clustine_world::{BlockPos, Vec3};

use super::Region;
use super::updates::RegionView;
use crate::api::{RegionEvent, SOUND_SOURCE_RECORDS, TickOutput};

/// The highest note of a note block (`NoteBlock.NOTE`'s range is 0 to 24).
const HIGHEST_NOTE: u8 = 24;

/// The block event of a note block's play, and of a bell's ring.
const NOTE_EVENT: i32 = 0;
const RING_EVENT: i32 = 1;

impl Region {
    /// `NoteBlock.neighborChanged`: power that rises plays the note, and the block
    /// keeps whether it is powered.
    pub(super) fn note_block_powered(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let powered = signal::has_neighbour_signal(&mut RegionView(self), pos);
        if powered == flag(state, "powered") {
            return;
        }
        if powered {
            self.play_note(pos, state);
        }
        self.set_block(pos, set_flag(state, "powered", powered), output);
    }

    /// `NoteBlock.useWithoutItem`: a click tunes the block one note up, round to the
    /// lowest after the highest, and plays it.
    pub(super) fn note_block_used(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let note = number(state, "note").unwrap_or(0);
        let next = if note >= HIGHEST_NOTE { 0 } else { note + 1 };
        let tuned = set(state, "note", &next.to_string());
        self.set_block(pos, tuned, output);
        self.play_note(pos, tuned);
    }

    /// `NoteBlock.playNote`: the note's block event, where a head gives the sound or
    /// nothing above muffles it.
    fn play_note(&mut self, pos: BlockPos, state: BlockState) {
        let instrument = Instrument::of_note_block(state);
        let above = self
            .block(relative(pos, Face::Up))
            .unwrap_or(clustine_data::blocks::AIR);
        if instrument.works_above_note_block() || above.props().is_air() {
            self.block_event(pos, state, NOTE_EVENT, 0);
        }
    }

    /// `NoteBlock.triggerEvent`: the instrument's sound at the note's pitch, or at 1 for
    /// a head, heard by everyone near. A player's head plays the sound its own data
    /// names, which the simulation does not read: it plays nothing, and the event is
    /// not sent, as the game's is not for a head without one.
    pub(super) fn note_block_event(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        let instrument = Instrument::of_note_block(state);
        let pitch = if instrument.is_tunable() {
            note::pitch(number(state, "note").unwrap_or(0))
        } else {
            1.0
        };
        let Some(sound) = instrument.sound() else {
            return false;
        };
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_RECORDS,
            position: Vec3::new(
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ),
            volume: 3.0,
            pitch,
            except: None,
        });
        true
    }

    /// `BellBlock.neighborChanged`: power that rises rings the bell the way it faces,
    /// and the block keeps whether it is powered.
    pub(super) fn bell_powered(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let powered = signal::has_neighbour_signal(&mut RegionView(self), pos);
        if powered == flag(state, "powered") {
            return;
        }
        if powered {
            self.ring_bell(pos, None, output);
        }
        self.set_block(pos, set_flag(state, "powered", powered), output);
    }

    /// `BellBlock.attemptToRing`: the bell's block entity is struck from `direction`,
    /// or the way the bell faces, and the bell is heard by everyone near.
    pub(super) fn ring_bell(
        &mut self,
        pos: BlockPos,
        direction: Option<Face>,
        output: &mut TickOutput,
    ) -> bool {
        let Some(state) = self.own_block(pos) else {
            return false;
        };
        let Some(direction) = direction.or_else(|| clustine_blocks::data::facing(state)) else {
            return false;
        };
        if !self.hit_bell(pos, direction) {
            return false;
        }
        self.block_event(pos, state, RING_EVENT, direction as i32);
        output.events.push(RegionEvent::Sound {
            sound: sounds::BLOCK_BELL_USE,
            source: crate::api::SOUND_SOURCE_BLOCKS,
            position: Vec3::new(
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ),
            volume: 2.0,
            pitch: 1.0,
            except: None,
        });
        true
    }

    /// `BellBlockEntity.triggerEvent`: the ring's block event shakes the bell, struck
    /// from the side its data names.
    pub(super) fn bell_event(&mut self, pos: BlockPos, kind: i32, data: i32) -> bool {
        if kind != RING_EVENT {
            return false;
        }
        let Some(direction) = face_of_data(data) else {
            return false;
        };
        self.ring_bell_event(pos, direction)
    }

    /// `BlockBehaviour.triggerEvent` of the blocks of this step.
    pub(super) fn instrument_event(
        &mut self,
        at: BlockPos,
        state: BlockState,
        kind: i32,
        data: i32,
        output: &mut TickOutput,
    ) -> Option<bool> {
        if note::is_note_block(state) {
            return Some(self.note_block_event(at, state, output));
        }
        if is_class(state, BlockClass::BellBlock) {
            return Some(self.bell_event(at, kind, data));
        }
        None
    }
}

/// `Direction.from3DDataValue`, down, up, north, south, west and east.
fn face_of_data(data: i32) -> Option<Face> {
    Face::ALL.get(usize::try_from(data).ok()?).copied()
}
