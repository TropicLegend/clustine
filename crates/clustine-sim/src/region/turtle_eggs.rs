//! Turtle eggs (`TurtleEggBlock`'s random tick; `docs/groundwork/n7-brief.md`, section
//! 6.10.7): on sand, an egg cracks twice and then hatches by a chance the time of day
//! sets (`gameplay/turtle_egg_hatch_chance` of the `day` timeline: certain in the last
//! hours before dawn, one in five hundred otherwise), drawn from the random tick's
//! random; a hatching leaves a baby turtle for each egg of the clutch, whose home is its
//! nest.
//!
//! Named difference: eggs are not trampled (the living bodies' and the players' steps
//! on a block do not reach it, `crate::living`).

use clustine_blocks::data::{number, set};
use clustine_data::{BlockState, entity_types, sounds};
use clustine_world::BlockPos;

use super::Region;
use super::updates::{UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Vibration};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::mobs::types::turtle;
use crate::region::Random;

/// The ticks of the day from which, and up to which, a turtle egg is sure to crack:
/// `day.json`'s keyframes of `turtle_egg_hatch_chance`, eased as `constant` (each value
/// holds until the next keyframe, round the day's end).
const SURE_FROM: u64 = 21_062;
const SURE_UNTIL: u64 = 21_905;

/// The chance otherwise.
const CHANCE: f32 = 0.002;

/// `environmentAttributes().getValue(TURTLE_EGG_HATCH_CHANCE, pos)` at the overworld
/// clock's `day` ticks: the timeline's value, its modifier `maximum` over the
/// attribute's default of nothing.
pub(crate) fn hatch_chance(day: u64) -> f32 {
    let t = day % crate::time::DAY;
    let value = if (SURE_FROM..SURE_UNTIL).contains(&t) {
        1.0
    } else {
        CHANCE
    };
    value.max(0.0)
}

impl Region {
    /// `TurtleEggBlock.randomTick`: where the time of day's chance comes up
    /// (`shouldUpdateHatchLevel`, one draw) and the egg is on sand, it cracks a stage,
    /// with its sound and `BLOCK_CHANGE`; past its second crack it hatches, the block
    /// gone with its sound and `BLOCK_DESTROY`, and for each egg the egg's breaking shown
    /// (level event 2001) and a baby turtle made in a row across the block.
    pub(crate) fn turtle_egg_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let chance = hatch_chance(self.clock.day);
        if !(chance > 0.0 && random.next_float() < chance) {
            return;
        }
        let below = self.block(pos.offset(0, -1, 0));
        if !below.is_some_and(turtle::is_sand) {
            return;
        }
        let hatch = number(state, "hatch").unwrap_or(0);
        let centre = clustine_world::Vec3::new(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        );
        let pitch = 0.9 + random.next_float() * 0.2;
        if hatch < 2 {
            output.events.push(RegionEvent::Sound {
                sound: sounds::ENTITY_TURTLE_EGG_CRACK,
                source: SOUND_SOURCE_BLOCKS,
                position: centre,
                volume: 0.7,
                pitch,
                except: None,
            });
            let cracked = set(state, "hatch", &(hatch + 1).to_string());
            self.set_block_updated(pos, cracked, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            self.vibrate_at(Vibration::BLOCK_CHANGE, pos, Context::of(None, Some(state)));
            return;
        }
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_TURTLE_EGG_HATCH,
            source: SOUND_SOURCE_BLOCKS,
            position: centre,
            volume: 0.7,
            pitch,
            except: None,
        });
        self.set_block(pos, clustine_data::blocks::AIR, output);
        self.vibrate_at(
            Vibration::BLOCK_DESTROY,
            pos,
            Context::of(None, Some(state)),
        );
        let eggs = number(state, "eggs").unwrap_or(1);
        let kind = entity_types::TURTLE as u16;
        for i in 0..eggs {
            output.events.push(RegionEvent::LevelEvent {
                kind: 2001,
                position: pos,
                data: i32::from(state.0),
            });
            let at = [
                f64::from(pos.x) + 0.3 + f64::from(i) * 0.2,
                f64::from(pos.y),
                f64::from(pos.z) + 0.3,
            ];
            if let Some(baby) = self.new_mob(kind, turtle::hatchling(pos), at, 0.0, 0.0) {
                self.add_made_mob(baby, output);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_egg_is_sure_to_crack_in_the_hours_before_dawn_and_rarely_else() {
        assert_eq!(hatch_chance(21_061), 0.002);
        assert_eq!(hatch_chance(21_062), 1.0);
        assert_eq!(hatch_chance(21_904), 1.0);
        assert_eq!(hatch_chance(21_905), 0.002);
        assert_eq!(hatch_chance(24_000 + 21_500), 1.0);
        assert_eq!(hatch_chance(6_000), 0.002);
    }
}
