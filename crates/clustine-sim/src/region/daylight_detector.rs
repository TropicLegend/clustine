//! The daylight detector (`DaylightDetectorBlock`, its block entity's ticker;
//! `docs/groundwork/n8-plan.md`, section 6.4): every 20 ticks its power from the sky's
//! light and the sun's angle, inverted or not, and a click turning it.
//!
//! The game makes the ticker only where the dimension has sky light; Clustine's one
//! dimension, the overworld, has it, so every detector ticks.

use clustine_blocks::data::{flag, is_class, number, set, set_flag};
use clustine_data::{BlockClass, BlockState};
use clustine_noise::trig;
use clustine_world::{BlockPos, PlayerId};

use super::Region;
use super::updates::{UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Vibration};
use crate::api::TickOutput;

/// `Mth.DEG_TO_RAD` as the detector writes it, `f32`. Its π and 2π, 3.1415927 and
/// 6.2831855, are `f32`'s own.
const DEGREES_TO_RADIANS: f32 = 0.017_453_292;
const PI: f32 = std::f32::consts::PI;
const TWO_PI: f32 = std::f32::consts::TAU;

/// The power the detector `state` gives with `sky`, the effective sky brightness at it,
/// and the sun at `sun_angle` degrees (`updateSignalStrength`): inverted, 15 less the
/// light; else the light lowered by the sun's angle, drawn a fifth of the way towards
/// noon so that the power holds longer through the morning and evening.
pub(crate) fn detector_power(state: BlockState, sky: i32, sun_angle: f32) -> u8 {
    let mut angle = sun_angle * DEGREES_TO_RADIANS;
    let mut power = sky;
    if flag(state, "inverted") {
        power = 15 - power;
    } else if power > 0 {
        let towards = if angle < PI { 0.0 } else { TWO_PI };
        angle += (towards - angle) * 0.2;
        power = java_round(power as f32 * trig::cos(f64::from(angle)));
    }
    power.clamp(0, 15) as u8
}

/// Java's `Math.round(float)`: the nearest integer, a half rounded up (towards positive
/// infinity, where Rust's `round` takes it away from zero).
fn java_round(value: f32) -> i32 {
    // Exact: an `f32` and a half meet in an `f64` without rounding.
    (f64::from(value) + 0.5).floor() as i32
}

impl Region {
    /// The ticker of the detector at `pos`, whose block is `state` (`tickEntity`): once
    /// every 20 ticks of game time.
    pub(crate) fn daylight_detector_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        if !self.clock.game_time.is_multiple_of(20) {
            return;
        }
        self.update_signal_strength(pos, state, self.clock.day, output);
    }

    /// `updateSignalStrength` at the overworld clock's `day`: the power worked out and,
    /// where it differs, set with every update (`setBlockAndUpdate`), so that the
    /// neighbours hear of it as of any power source's change. Where the chunk is not lit
    /// the light is not judged and nothing changes.
    fn update_signal_strength(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        day: u64,
        output: &mut TickOutput,
    ) {
        let darken = crate::time::level_sky_darken(self.config.dimension, day);
        let Some(sky) = self.effective_sky_brightness(pos, darken) else {
            return;
        };
        let power = detector_power(state, sky, crate::time::sun_angle(day));
        if number(state, "power") != Some(power) {
            self.set_block(pos, set(state, "power", &power.to_string()), output);
        }
    }

    /// A click on the detector `state` at `pos` by `player` (`useWithoutItem`): inverted
    /// with `UPDATE_CLIENTS` alone, the game event `block_change` of the player, and its
    /// power worked out again at once. Returns whether `state` is a detector.
    ///
    /// The game refuses it to a player who may not build (`Player.mayBuild`); every
    /// Clustine player is in creative, who may. The power read here is a packet's,
    /// handled between two ticks, so it is of the day as the tick before left the clock
    /// (as a bed's darkness is, `sleep.rs`), and the darkening of that tick. Its
    /// neighbours are told of a new power, so it changes nothing where its chunk does not
    /// tick (`docs/adr/0026-redstone.md`, section 1).
    pub(super) fn daylight_detector_used(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if !is_class(state, BlockClass::DaylightDetectorBlock) {
            return false;
        }
        if !self.ticks_chunk(pos.chunk()) {
            return true;
        }
        let turned = set_flag(state, "inverted", !flag(state, "inverted"));
        self.set_block_updated(pos, turned, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        let context = Context::of(self.player_source(player), Some(turned));
        self.vibrate_at(Vibration::BLOCK_CHANGE, pos, context);
        let day = self.sleep.last_day;
        self.update_signal_strength(pos, turned, day, output);
        true
    }
}
