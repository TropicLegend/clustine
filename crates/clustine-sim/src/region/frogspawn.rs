//! The region's side of the frog and the tadpole (`docs/groundwork/n7-brief.md`, section
//! 7.5.7 and 7.5.8): frogspawn (`FrogspawnBlock`), which schedules its hatching as it is
//! placed and, at that tick, hatches two to five tadpoles at its block, or goes where it
//! cannot stay; and a tadpole grown up (`Tadpole.ageUp()`), made a frog of the biome it
//! is in.
//!
//! A frog lays frogspawn by its brain's behaviour (`TryLaySpawnOnFluidNearLand`,
//! `mobs/types/frog_ai.rs`), a block set like any other, which schedules its hatching
//! here as it is placed; frogspawn a player places hatches the same.
//!
//! The draws of the level's random a block's tick makes come from the region's stream
//! for rules outside a random tick ([`Region::next_draw`]); a frog's made of a tadpole,
//! from the tadpole's own stream, as the level's on its behalf.
//!
//! Named difference: a frog grown from a tadpole is made where the tadpole was, its box
//! not moved out of the blocks it may then meet (`fudgePositionAfterSizeChange`).

use clustine_blocks::data::{is_block, tick_id};
use clustine_blocks::survive::can_survive;
use clustine_data::{BlockState, blocks, entity_types, sounds};
use clustine_world::{BlockPos, EntityId, Ticked};

use super::Region;
use super::updates::{RegionView, UPDATE_LIMIT};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::mobs::Mob;
use crate::mobs::types::{self, Spawning, frog, tadpole};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;

/// `minHatchTickDelay` and `maxHatchTickDelay`: from three minutes to just under ten.
pub(crate) const MIN_HATCH_DELAY: i32 = 3600;
pub(crate) const MAX_HATCH_DELAY: i32 = 12_000;

/// `getFrogspawnHatchDelay(random)`: `nextInt(3600, 12000)`, one draw of
/// `nextInt(8400)`.
pub(crate) fn hatch_delay(random: &mut Random) -> i32 {
    MIN_HATCH_DELAY + random.next_int((MAX_HATCH_DELAY - MIN_HATCH_DELAY) as u32) as i32
}

/// `getRandomTadpolePositionOffset(random)`: a `nextDouble` kept within
/// `0.20000000298023224` and `0.7999999970197678` (the floats 0.2 and 0.8, widened).
pub(crate) fn tadpole_offset(random: &mut Random) -> f64 {
    random
        .next_double()
        .clamp(0.200_000_002_980_232_24, 0.799_999_997_019_767_8)
}

/// Where each tadpole of a hatching at `pos` goes and how it faces, of the hatching's
/// draws in the game's order: their count (`nextInt(2, 6)`), then for each its x and z
/// within the block's middle and its yaw of 1 to 360, half a block below the
/// frogspawn.
pub(crate) fn hatched_at(pos: BlockPos, random: &mut Random) -> Vec<([f64; 3], f32)> {
    let count = 2 + random.next_int(4);
    (0..count)
        .map(|_| {
            let x = f64::from(pos.x) + tadpole_offset(random);
            let z = f64::from(pos.z) + tadpole_offset(random);
            let yaw = 1 + random.next_int(360);
            ([x, f64::from(pos.y) - 0.5, z], yaw as f32)
        })
        .collect()
}

impl Region {
    /// `FrogspawnBlock.onPlace`: frogspawn come where there was none schedules its
    /// hatching, 3600 to 11999 ticks on.
    pub(crate) fn frogspawn_on_place(&mut self, pos: BlockPos, state: BlockState, old: BlockState) {
        if !is_block(state, blocks::FROGSPAWN) || is_block(old, blocks::FROGSPAWN) {
            return;
        }
        let mut random = self.next_draw();
        let delay = hatch_delay(&mut random);
        self.schedule_tick(pos, Ticked::Block(tick_id(state)), delay, 0);
    }

    /// `FrogspawnBlock.tick`: frogspawn that cannot stay goes without drops; else it
    /// hatches (`hatchFrogspawn`): gone without drops, its sound at its block, and its
    /// tadpoles made. Whether the block was frogspawn.
    pub(crate) fn frogspawn_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if !is_block(state, blocks::FROGSPAWN) {
            return false;
        }
        if !can_survive(state, pos, &mut RegionView(self)) {
            self.destroy_block(pos, false, UPDATE_LIMIT, output);
            return true;
        }
        let mut random = self.next_draw();
        self.hatch_frogspawn(pos, &mut random, output);
        true
    }

    /// `hatchFrogspawn(level, pos, random)`, of the scheduled tick's random.
    pub(crate) fn hatch_frogspawn(
        &mut self,
        pos: BlockPos,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        self.destroy_block(pos, false, UPDATE_LIMIT, output);
        output.events.push(RegionEvent::Sound {
            sound: sounds::BLOCK_FROGSPAWN_HATCH,
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
        let kind = entity_types::TADPOLE as u16;
        for (at, yaw) in hatched_at(pos, random) {
            if let Some(made) = self.new_mob(kind, tadpole::hatchling(), at, yaw, 0.0) {
                self.add_made_mob(made, output);
            }
        }
    }

    /// `Tadpole.ageUp()` of the region's tadpole `key`: `convertTo(FROG, single)` where
    /// it is, as it faces and moves, keeping its flags; the frog's `finalizeSpawn` of
    /// `CONVERSION` (its biome's variant, its long jump's cooldown, `Mob`'s follow range
    /// and hand) of the level's draws, made from the tadpole's stream; the frog kept for
    /// good; the tadpole gone, with its sound of growing up.
    pub(crate) fn tadpole_grown_up(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            return;
        };
        if i32::from(record.kind) != entity_types::TADPOLE {
            return;
        }
        let Ok(old) = Mob::decode(&record.data) else {
            return;
        };
        let kind = entity_types::FROG as u16;
        let Some(mut made) = self.new_mob(
            kind,
            frog::grown_from(&old),
            record.position,
            record.yaw,
            record.pitch,
        ) else {
            return;
        };
        let Ok(mut grown) = Mob::decode(&made.data) else {
            return;
        };
        let mut level = self.entity_stream(key).random;
        let start = level.clone();
        let mut own = self.entity_stream(made.key).random;
        let block = crate::physics::containing(made.position);
        let biome = self.biome_at(block);
        let day = self.clock.day;
        let game_time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let mut group = None;
        crate::mobs::making::finalize(
            kind,
            &mut Spawning {
                mob: &mut grown,
                level: &mut level,
                own: &mut own,
                biome,
                at: block,
                day,
                game_time,
                reason: types::SpawnReason::Conversion,
                group: &mut group,
                structure: None,
                moon_brightness: crate::mobs::making::moon_brightness(day),
                difficulty: (0.0, 0.0),
                companions: None,
            },
        );
        grown.body.flags.persistent = true;
        self.entity_drew(key, level.draws_since(&start));
        made.data = grown.encode();
        made.motion = record.motion;
        self.add_made_mob(made, output);
        let hops = record.hops;
        self.unride_removed(key, true, output);
        self.remove_entity(key, hops, chunk, output);
        // `playSound(TADPOLE_GROW_UP, 0.15, 1)` of the tadpole, of its own source.
        let at = record.position;
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_TADPOLE_GROW_UP,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(at[0], at[1], at[2]),
            volume: 0.15,
            pitch: 1.0,
            except: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frogspawn_hatches_three_minutes_to_just_under_ten_after_it_is_placed() {
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..4000 {
            let mut random = Random::of_entity(seed, EntityId(3), 0);
            let mut replay = random.clone();
            let delay = hatch_delay(&mut random);
            assert_eq!(delay, 3600 + replay.next_int(8400) as i32);
            assert_eq!(random, replay, "one draw");
            assert!((3600..=11_999).contains(&delay), "{delay}");
            seen.insert(delay / 1000);
        }
        // From the third thousand to the eleventh, every thousand is reached.
        assert_eq!(seen.len(), 9, "{seen:?}");
    }

    #[test]
    fn a_hatching_makes_two_to_five_tadpoles_inside_the_block_s_middle_half_a_block_down() {
        let pos = BlockPos::new(5, -60, -3);
        let mut counts = [0; 6];
        for seed in 0..500 {
            let mut random = Random::of_entity(seed, EntityId(8), 0);
            let mut replay = random.clone();
            let hatched = hatched_at(pos, &mut random);
            let count = 2 + replay.next_int(4) as usize;
            assert_eq!(hatched.len(), count);
            counts[count] += 1;
            for (at, yaw) in hatched {
                // x, then z, then the yaw, each its own draw, each kept within the
                // dump's two doubles (FrogspawnBlock.txt 171-243).
                let (low, high) = (0.200_000_002_980_232_24, 0.799_999_997_019_767_8);
                let x = replay.next_double().clamp(low, high);
                let z = replay.next_double().clamp(low, high);
                let turn = 1 + replay.next_int(360);
                assert_eq!(at, [5.0 + x, -60.5, -3.0 + z]);
                assert_eq!(yaw, turn as f32);
                assert!((1.0..=360.0).contains(&yaw));
                assert!((5.2..=5.8 + 1e-6).contains(&at[0]), "{at:?}");
            }
            assert_eq!(random, replay);
        }
        assert!(counts[2..=5].iter().all(|&count| count > 80), "{counts:?}");
        assert_eq!(counts[0] + counts[1], 0);
    }
}
