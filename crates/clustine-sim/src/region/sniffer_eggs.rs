//! Sniffer eggs (`SnifferEggBlock`; `docs/groundwork/n7-brief.md`, section 7.9.6): an egg
//! placed waits a third of 24000 ticks (of 12000 on a moss block, which shows its boost)
//! and up to 299 more, then cracks, and again, and at its third wait hatches a snifflet
//! where it was, facing a way of the level's random. Every placement and every crack
//! (each a new state of the block, whose `onPlace` runs again) schedules the next wait.

use clustine_blocks::data::{is_block, is_class, number, set, tick_id};
use clustine_data::{BlockClass, BlockState, blocks, entity_types, sounds};
use clustine_world::{BlockPos, Ticked};

use super::Region;
use super::updates::{UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Vibration};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::mobs::ai::maths::wrap_degrees;
use crate::mobs::types::sniffer;

/// `HATCH_TIME_TICKS` and `BOOSTED_HATCH_TIME_TICKS`, of which each wait is a third.
pub(crate) const HATCH_TIME: i32 = 24_000;
pub(crate) const BOOSTED_HATCH_TIME: i32 = 12_000;

/// `RANDOM_HATCH_OFFSET_TICKS`: what a wait may add.
pub(crate) const HATCH_OFFSET: u32 = 300;

/// The level event of a boosted egg's green sparks (`onPlace`'s 3009).
const BOOST_EVENT: i32 = 3009;

/// `hatchBoost(level, pos)`: the block below is of `#sniffer_egg_hatch_boost` (moss).
pub(crate) fn hatch_boost(below: BlockState) -> bool {
    clustine_blocks::data::in_tag(below, "minecraft:sniffer_egg_hatch_boost")
        || is_block(below, blocks::MOSS_BLOCK)
}

/// `getHatchDelay`'s base: a third of the hatching's time, boosted or not.
pub(crate) fn wait(boosted: bool) -> i32 {
    (if boosted {
        BOOSTED_HATCH_TIME
    } else {
        HATCH_TIME
    }) / 3
}

impl Region {
    /// `SnifferEggBlock.onPlace`: the boost shown on moss, `BLOCK_PLACE`, and its next
    /// tick scheduled a wait and up to 299 ticks on (a draw of the level's random).
    pub(crate) fn sniffer_egg_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_class(state, BlockClass::SnifferEggBlock) {
            return;
        }
        let below = self.block(pos.offset(0, -1, 0)).unwrap_or(blocks::AIR);
        let boosted = hatch_boost(below);
        if boosted {
            output.events.push(RegionEvent::LevelEvent {
                kind: BOOST_EVENT,
                position: pos,
                data: 0,
            });
        }
        self.vibrate_at(Vibration::BLOCK_PLACE, pos, Context::of(None, Some(state)));
        let delay = wait(boosted) + self.next_draw().next_int(HATCH_OFFSET) as i32;
        self.schedule_tick(pos, Ticked::Block(tick_id(state)), delay, 0);
    }

    /// `SnifferEggBlock.tick`: before its last stage it cracks, with its sound (its pitch
    /// a draw of the tick's random), into the next stage (which `onPlace` schedules on
    /// from); at its last it hatches, with its sound: the block destroyed without drops,
    /// and a snifflet at its middle facing a way drawn of the level's random. Whether the
    /// block was an egg.
    pub(crate) fn sniffer_egg_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if !is_class(state, BlockClass::SnifferEggBlock) {
            return false;
        }
        let mut random = self.next_draw();
        let centre = clustine_world::Vec3::new(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        );
        let hatch = number(state, "hatch").unwrap_or(0);
        let pitch = 0.9 + random.next_float() * 0.2;
        if hatch < 2 {
            output.events.push(RegionEvent::Sound {
                sound: sounds::BLOCK_SNIFFER_EGG_CRACK,
                source: SOUND_SOURCE_BLOCKS,
                position: centre,
                volume: 0.7,
                pitch,
                except: None,
            });
            let cracked = set(state, "hatch", &(hatch + 1).to_string());
            self.set_block_updated(pos, cracked, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            return true;
        }
        output.events.push(RegionEvent::Sound {
            sound: sounds::BLOCK_SNIFFER_EGG_HATCH,
            source: SOUND_SOURCE_BLOCKS,
            position: centre,
            volume: 0.7,
            pitch,
            except: None,
        });
        self.destroy_block(pos, false, UPDATE_LIMIT, output);
        let yaw = wrap_degrees(random.next_float() * 360.0);
        let kind = entity_types::SNIFFER as u16;
        let at = [centre.x, centre.y, centre.z];
        if let Some(baby) = self.new_mob(kind, sniffer::hatchling(), at, yaw, 0.0) {
            self.add_made_mob(baby, output);
        }
        true
    }
}
