//! A player's experience and attack strength (`docs/groundwork/n7-plan.md`, section
//! 7.7): `Player.giveExperiencePoints` with its levels and the level-up's sound, and
//! `getAttackStrengthScale` from the ticks since its ticker was last reset.
//!
//! The ticker is kept as the tick it was reset in, so that a player who waits commits
//! nothing for it: the game's `Player.tick` counts it up after the inputs of a tick, so
//! the inputs of the `k`th tick after a reset read `k`. An attack and a punch reset it
//! as they are handled; a change of the item in the main hand resets it in the tick, as
//! `Player.tick` sees it after counting, so that the next tick's inputs read 0. It is
//! told by the hand before the tick's inputs and after them, which the state has.
//!
//! Named difference: the score a death screen shows (`increaseScore`) is not kept.

use clustine_data::{attributes as ids, sounds};
use clustine_world::PlayerId;

use super::Region;
use super::mob_attacks::held_value;
use crate::api::{Experience, PlayerEvent, RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput};

/// `getXpNeededForNextLevel`.
fn needed(level: i32) -> i32 {
    if level >= 30 {
        112 + (level - 30) * 9
    } else if level >= 15 {
        37 + (level - 15) * 5
    } else {
        7 + level * 2
    }
}

/// What `giveExperienceLevels(levels)` does to the numbers, and whether it plays the
/// level-up's sound, which it does every fifth level up at most once in 100 ticks.
fn give_levels(experience: &mut Experience, levels: i32) -> bool {
    experience.level = experience.level.saturating_add(levels);
    if experience.level < 0 {
        experience.level = 0;
        experience.progress = 0.0;
        experience.total = 0;
    }
    levels > 0 && experience.level % 5 == 0
}

/// `giveExperiencePoints(points)` on the numbers: whether a level up is to be heard.
pub(crate) fn give_points(experience: &mut Experience, points: i32) -> bool {
    let mut heard = false;
    experience.progress += points as f32 / needed(experience.level) as f32;
    experience.total = experience.total.saturating_add(points).max(0);
    while experience.progress < 0.0 {
        let over = experience.progress * needed(experience.level) as f32;
        if experience.level > 0 {
            give_levels(experience, -1);
            experience.progress = 1.0 + over / needed(experience.level) as f32;
        } else {
            give_levels(experience, -1);
            experience.progress = 0.0;
        }
    }
    while experience.progress >= 1.0 {
        experience.progress = (experience.progress - 1.0) * needed(experience.level) as f32;
        heard |= give_levels(experience, 1);
        experience.progress /= needed(experience.level) as f32;
    }
    heard
}

impl Region {
    /// `giveExperiencePoints(points)` of the region's player `player`: their client is
    /// told, and every fifth level heard by all near.
    pub(crate) fn add_experience(
        &mut self,
        player: PlayerId,
        points: i32,
        output: &mut TickOutput,
    ) {
        let tick = self.tick;
        let Some(held) = self.players.get_mut(&player) else {
            return;
        };
        let before = held.experience;
        let level_up = give_points(&mut held.experience, points);
        // `lastLevelUpTime < tickCount - 100`, the game's counts both in its ticks.
        if level_up && held.last_level_up.is_none_or(|at| at + 100 < tick) {
            held.last_level_up = Some(tick);
            let volume = if held.experience.level > 30 {
                1.0
            } else {
                held.experience.level as f32 / 30.0
            };
            output.events.push(RegionEvent::Sound {
                sound: sounds::ENTITY_PLAYER_LEVELUP,
                source: SOUND_SOURCE_PLAYERS,
                position: held.pose.position,
                volume: volume * 0.75,
                pitch: 1.0,
                except: None,
            });
        }
        if held.experience != before {
            let now = held.experience;
            self.journal.players.insert(player);
            output
                .player_events
                .push((player, PlayerEvent::Experience(now)));
        }
    }

    /// `getAttackStrengthScale(0.5f)` of the region's player `player` now, by what
    /// their main hand holds (`getCurrentItemAttackStrengthDelay`: twenty over the attack
    /// speed); full for a player the region does not have.
    pub(crate) fn attack_strength(&self, player: PlayerId) -> f32 {
        let Some(held) = self.players.get(&player) else {
            return 1.0;
        };
        let Some(reset) = held.attack_reset else {
            return 1.0;
        };
        let speed = held_value(4.0, ids::ATTACK_SPEED, held.held().as_ref());
        let delay = (1.0 / speed * 20.0) as f32;
        let ticks = self.tick.saturating_sub(reset) as f32;
        ((ticks + 0.5) / delay).clamp(0.0, 1.0)
    }

    /// `resetAttackStrengthTicker`, as an input is handled.
    pub(crate) fn reset_attack_strength(&mut self, player: PlayerId) {
        let tick = self.tick;
        if let Some(held) = self.players.get_mut(&player)
            && held.attack_reset != Some(tick)
        {
            held.attack_reset = Some(tick);
            self.journal.players.insert(player);
        }
    }

    /// What each player's main hand holds before the tick's inputs, an empty hand as
    /// item -1, for [`Region::watch_main_hands`].
    pub(crate) fn main_hands(&self) -> Vec<(PlayerId, i32)> {
        let held = self.players.iter();
        held.map(|(id, player)| (*id, player.held().map_or(-1, |stack| stack.item)))
            .collect()
    }

    /// `Player.tick`'s look at the main hand, after the inputs: a different item than
    /// before them (`before`, [`Region::main_hands`]) resets the attack strength, so
    /// that the next tick's inputs read none. A player who came in the tick is not
    /// looked at: the game's player was there before.
    pub(crate) fn watch_main_hands(&mut self, before: &[(PlayerId, i32)]) {
        let tick = self.tick;
        for (id, was) in before {
            let Some(held) = self.players.get_mut(id) else {
                continue;
            };
            let now = held.held().map_or(-1, |stack| stack.item);
            if now != *was && held.attack_reset != Some(tick + 1) {
                held.attack_reset = Some(tick + 1);
                self.journal.players.insert(*id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn experience(level: i32, progress: f32, total: i32) -> Experience {
        Experience {
            level,
            progress,
            total,
        }
    }

    #[test]
    fn points_fill_the_bar_and_carry_over_into_the_next_level() {
        let mut xp = Experience::default();
        assert!(!give_points(&mut xp, 3));
        assert_eq!(xp, experience(0, 3.0 / 7.0, 3));
        // Seven fill level 0; the rest counts towards level 1's nine.
        give_points(&mut xp, 6);
        assert_eq!((xp.level, xp.total), (1, 9));
        assert!((xp.progress - 2.0 / 9.0).abs() < 1e-6, "{xp:?}");
    }

    #[test]
    fn the_fifth_level_is_heard_and_the_cost_of_a_level_rises_by_its_bands() {
        assert_eq!(
            (
                needed(0),
                needed(14),
                needed(15),
                needed(29),
                needed(30),
                needed(31)
            ),
            (7, 35, 37, 107, 112, 121)
        );
        let mut xp = experience(4, 0.0, 0);
        assert!(give_points(&mut xp, needed(4)));
        assert_eq!(xp.level, 5);
        assert!(!give_points(&mut xp, needed(5)));
    }

    #[test]
    fn taking_points_away_goes_down_levels_and_stops_at_nothing() {
        let mut xp = experience(1, 0.5, 11);
        give_points(&mut xp, -6);
        assert_eq!(xp.level, 0);
        assert!((xp.progress - (1.0 - 1.5 / 7.0)).abs() < 1e-6, "{xp:?}");
        give_points(&mut xp, -100);
        assert_eq!(xp, experience(0, 0.0, 0));
    }
}
