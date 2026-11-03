//! Hunger as the game keeps it (V4, `docs/groundwork/v-plan.md`, section 2.5): a
//! player's food, saturation and exhaustion, the one timer that heals or starves them,
//! eating, the peaceful difficulty's regeneration, when the client is told, and what
//! moving and jumping cost. Pure: every function takes the food data and the facts it
//! reads and returns what follows, and the region applies a heal or a starve to the
//! player's body (a starve is a hurt of the type `starve`, through the player's hurt).
//!
//! Read from the 26.3 jar (`javap -c -p`): `FoodData` and `FoodConstants`
//! (`/home/claude/reference/v4-javap`), `ServerPlayer.tickRegeneration`, `doTick`,
//! `checkMovementStatistics` and `jumpFromGround`, and `Player.causeFoodExhaustion`,
//! `canEat` and `isHurt` (`/home/claude/reference/n5-javap`). Offsets are bytecode
//! offsets in those methods.
//!
//! The timer is kept as the tick it began (`FoodData::timer_from`) so that a region does
//! not write the player each tick: after the tick `now` in which a branch counted, the
//! game's `tickTimer` is `now - timer_from`, and `None` is its 0. Named difference: the
//! game counts only the ticks the player is ticked, and a tick the region does not run
//! the player's body in (if there is one) is counted here all the same; the region runs
//! it every tick, and `reshape.rs` shifts the tick it began with the others.

use clustine_world::Difficulty;

use crate::state::FoodData;

/// `FoodConstants.MAX_FOOD`: the most food (`FoodData.add @8`, `needsFood @4`).
pub const MAX_FOOD: i32 = 20;
/// The most exhaustion (`FoodData.addExhaustion @7`).
pub const MAX_EXHAUSTION: f32 = 40.0;
/// The exhaustion one point of saturation or food pays for (`FoodData.tick @14, @25`).
const EXHAUSTION_DROP: f32 = 4.0;
/// The fast branch's period and the most saturation it spends (`tick @140, @149`).
const FAST_TICKS: i64 = 10;
const FAST_SATURATION: f32 = 6.0;
/// The slow and starving branches' period (`tick @214, @259`).
const SLOW_TICKS: i64 = 80;
/// The food the slow branch needs (`tick @188`, `FoodConstants.HEAL_LEVEL`).
const HEAL_LEVEL: i32 = 18;
/// What a slow heal costs (`tick @225`, `FoodConstants.EXHAUSTION_HEAL`).
const EXHAUSTION_HEAL: f32 = 6.0;

/// A jump's exhaustion, sprinting or not (`ServerPlayer.jumpFromGround @19, @29`).
pub const EXHAUSTION_SPRINT_JUMP: f32 = 0.2;
pub const EXHAUSTION_JUMP: f32 = 0.05;
/// Exhaustion a metre: swimming, under or in water (`checkMovementStatistics @66, @134,
/// @195`) and sprinting on the ground (`@295`); walking, crouching, climbing, gliding
/// and flying cost none (`@329`, `@354`, and no call in the other branches).
const EXHAUSTION_SWIM: f32 = 0.01;
const EXHAUSTION_SPRINT: f32 = 0.1;

/// What `FoodData.tick` reads of the player and the world besides the food.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoodFacts {
    pub health: f32,
    pub max_health: f32,
    pub difficulty: Difficulty,
    /// The rule `natural_health_regeneration`.
    pub natural_regeneration: bool,
}

impl FoodFacts {
    /// `Player.isHurt`: alive and below the most health.
    pub fn hurt(&self) -> bool {
        self.health > 0.0 && self.health < self.max_health
    }
}

/// What one `FoodData.tick` left: the food data after, and the heal and the starve for
/// the region to apply to the player, in the game's order (the heal or the starve is the
/// last thing the tick does to the player, so applying it after loses nothing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoodTick {
    pub food: FoodData,
    /// `ServerPlayer.heal(amount)` (`tick @162`, `@221`).
    pub heal: Option<f32>,
    /// `hurtServer(starve, amount)` (`tick @307`).
    pub starve: Option<f32>,
}

/// The game's `tickTimer` after the last tick counted, before the tick `now` counts.
fn timer_before(food: &FoodData, now: i64) -> i64 {
    food.timer_from.map_or(0, |from| (now - 1) - from)
}

/// `FoodData.tick(ServerPlayer)` in the tick `now`.
pub fn tick(food: FoodData, now: i64, facts: &FoodFacts) -> FoodTick {
    let mut food = food;
    // @10-75: the exhaustion pays for a point of saturation, or of food but not in
    // peaceful; it is spent either way.
    if food.exhaustion > EXHAUSTION_DROP {
        food.exhaustion -= EXHAUSTION_DROP;
        if food.saturation > 0.0 {
            food.saturation = (food.saturation - 1.0).max(0.0);
        } else if facts.difficulty != Difficulty::Peaceful {
            food.food = (food.food - 1).max(0);
        }
    }

    let rule = facts.natural_regeneration;
    let hurt = facts.hurt();
    // One timer for the three branches: counted up in whichever applies (and carried
    // from one to another), reset by a heal or a starve, and at rest when none applies.
    let counted = timer_before(&food, now) + 1;
    let mut heal = None;
    let mut starve = None;
    // Whether the timer rests after this tick: a heal or a starve, or no branch.
    let rests = if rule && food.saturation > 0.0 && hurt && food.food >= MAX_FOOD {
        // @96-176, fast: the saturation heals what it pays for.
        let fired = counted >= FAST_TICKS;
        if fired {
            let spent = food.saturation.min(FAST_SATURATION);
            heal = Some(spent / FAST_SATURATION);
            add_exhaustion(&mut food, spent);
        }
        fired
    } else if rule && food.food >= HEAL_LEVEL && hurt {
        // @179-235, slow.
        let fired = counted >= SLOW_TICKS;
        if fired {
            heal = Some(1.0);
            add_exhaustion(&mut food, EXHAUSTION_HEAL);
        }
        fired
    } else if food.food <= 0 {
        // @238-316, starving: always above 10 health (peaceful and easy too, @264-271,
        // which the plan leaves out for peaceful), at any health on hard, above 1 on
        // normal; the timer is reset whether it hurt or not.
        let fired = counted >= SLOW_TICKS;
        if fired {
            let health = facts.health;
            let difficulty = facts.difficulty;
            if health > 10.0
                || difficulty == Difficulty::Hard
                || (health > 1.0 && difficulty == Difficulty::Normal)
            {
                starve = Some(1.0);
            }
        }
        fired
    } else {
        // @319: none applies.
        true
    };
    food.timer_from = if rests { None } else { Some(now - counted) };
    FoodTick { food, heal, starve }
}

/// `FoodData.addExhaustion`: capped at 40.
pub fn add_exhaustion(food: &mut FoodData, amount: f32) {
    food.exhaustion = (food.exhaustion + amount).min(MAX_EXHAUSTION);
}

/// `Player.causeFoodExhaustion`: none while the abilities say invulnerable (creative,
/// spectator), else `addExhaustion`.
pub fn cause_exhaustion(food: &mut FoodData, amount: f32, invulnerable: bool) {
    if !invulnerable {
        add_exhaustion(food, amount);
    }
}

/// `FoodData.add(int, float) @0-35`: the food to 20, the saturation to at most the food
/// as it now is (`Mth.clamp` from 0).
fn add(food: &mut FoodData, nutrition: i32, saturation: f32) {
    food.food = (nutrition + food.food).clamp(0, MAX_FOOD);
    food.saturation = clamp(saturation + food.saturation, 0.0, food.food as f32);
}

/// `Mth.clamp(float, float, float)`: below the least is the least, else at most the most.
fn clamp(value: f32, least: f32, most: f32) -> f32 {
    if value < least {
        least
    } else {
        value.min(most)
    }
}

/// `FoodData.eat(FoodProperties)`: the component's nutrition and its saturation, which
/// is already final (`clustine-data`'s `Food.saturation`).
pub fn eat(food: &mut FoodData, nutrition: i32, saturation: f32) {
    add(food, nutrition, saturation);
}

/// `FoodData.eat(int, float)`, the Saturation effect's: the saturation is
/// `FoodConstants.saturationByModifier`, `nutrition · modifier · 2` in floats.
pub fn eat_scaled(food: &mut FoodData, nutrition: i32, modifier: f32) {
    add(food, nutrition, nutrition as f32 * modifier * 2.0);
}

/// `Player.canEat(boolean)`: invulnerable, food that can always be eaten, or hungry
/// (`FoodData.needsFood`).
pub fn can_eat(food: &FoodData, invulnerable: bool, always: bool) -> bool {
    invulnerable || always || food.food < MAX_FOOD
}

/// What peaceful's regeneration did: the food data after and a heal to apply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Regeneration {
    pub food: FoodData,
    pub heal: Option<f32>,
}

/// `ServerPlayer.tickRegeneration`, which does anything only in peaceful with the rule
/// `natural_health_regeneration` (@0-32), keyed to the player's age (`tickCount`).
/// Named difference: the age is the region's count for the player, so a hand-over may
/// shift the phase of these 20 and 10 ticks.
pub fn peaceful_regeneration(
    food: FoodData,
    player_age: i64,
    health: f32,
    max_health: f32,
    difficulty: Difficulty,
    natural_regeneration: bool,
) -> Regeneration {
    let mut food = food;
    let mut heal = None;
    if difficulty != Difficulty::Peaceful || !natural_regeneration {
        return Regeneration { food, heal };
    }
    // @35-85: every 20 ticks, heal 1 if hurt, and a point of saturation while below
    // 20, set directly and so not held to the food as eating holds it.
    if player_age % 20 == 0 {
        if health < max_health {
            heal = Some(1.0);
        }
        if food.saturation < 20.0 {
            food.saturation += 1.0;
        }
    }
    // @88-121: every 10 ticks, a point of food while it is needed.
    if player_age % 10 == 0 && food.food < MAX_FOOD {
        food.food += 1;
    }
    Regeneration { food, heal }
}

/// What `ServerPlayer.doTick` last sent in `SetHealth` (`lastSentHealth`,
/// `lastSentFood`, `lastFoodSaturationZero`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthSent {
    pub health: f32,
    pub food: i32,
    pub saturation_zero: bool,
}

impl HealthSent {
    /// What `SetHealth` says of a player with this health and food.
    pub fn of(health: f32, food: &FoodData) -> HealthSent {
        HealthSent {
            health,
            food: food.food,
            saturation_zero: food.saturation == 0.0,
        }
    }
}

/// `ServerPlayer.doTick @145-192`: `SetHealth` is sent when the health or the food
/// changed, or the saturation became or stopped being zero (and not for a change of
/// saturation alone).
pub fn needs_set_health(last_sent: &HealthSent, now: &HealthSent) -> bool {
    now.health != last_sent.health
        || now.food != last_sent.food
        || now.saturation_zero != last_sent.saturation_zero
}

/// What `ServerPlayer.checkMovementStatistics` reads of the player, in the order it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveFacts {
    pub passenger: bool,
    pub swimming: bool,
    /// `isEyeInFluid(WATER)`.
    pub eyes_in_water: bool,
    pub in_water: bool,
    pub on_climbable: bool,
    pub on_ground: bool,
    pub sprinting: bool,
}

/// A move's length in whole centimetres as the statistics count it:
/// `Math.round((float) Math.sqrt(…) * 100.0F)`, over all three axes or the horizontal
/// two. `f32::round` rounds halves away from zero and Java's halves up; the length is
/// never negative, where the two agree, and both make 0 of NaN and saturate.
pub fn centimetres(dx: f64, dy: f64, dz: f64, vertical: bool) -> i32 {
    let squared = if vertical {
        dx * dx + dy * dy + dz * dz
    } else {
        dx * dx + dz * dz
    };
    (squared.sqrt() as f32 * 100.0).round() as i32
}

/// The exhaustion of one move (`ServerPlayer.checkMovementStatistics`), before
/// `causeFoodExhaustion`'s invulnerability check: swimming counts all three axes (@25),
/// eyes under water all three (@93), in water the horizontal two (@158), sprinting on the
/// ground the horizontal two (@251); a passenger or a move of nothing costs none (@0-17).
pub fn movement_exhaustion(dx: f64, dy: f64, dz: f64, facts: &MoveFacts) -> f32 {
    if facts.passenger || (dx == 0.0 && dy == 0.0 && dz == 0.0) {
        return 0.0;
    }
    let (rate, cm) = if facts.swimming || facts.eyes_in_water {
        (EXHAUSTION_SWIM, centimetres(dx, dy, dz, true))
    } else if facts.in_water {
        (EXHAUSTION_SWIM, centimetres(dx, dy, dz, false))
    } else if facts.on_climbable || !facts.on_ground || !facts.sprinting {
        // Climbing (@212-241), gliding and flying (@369-449) award only statistics;
        // walking and crouching on the ground cost 0 (@312-363).
        return 0.0;
    } else {
        (EXHAUSTION_SPRINT, centimetres(dx, dy, dz, false))
    };
    if cm > 0 {
        // `rate * cm * 0.01F`, in that order, in floats.
        rate * cm as f32 * 0.01
    } else {
        0.0
    }
}

/// A jump's exhaustion (`ServerPlayer.jumpFromGround @11-32`).
pub fn jump_exhaustion(sprinting: bool) -> f32 {
    if sprinting {
        EXHAUSTION_SPRINT_JUMP
    } else {
        EXHAUSTION_JUMP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(health: f32, difficulty: Difficulty) -> FoodFacts {
        FoodFacts {
            health,
            max_health: 20.0,
            difficulty,
            natural_regeneration: true,
        }
    }

    fn food(level: i32, saturation: f32, exhaustion: f32) -> FoodData {
        FoodData {
            food: level,
            saturation,
            exhaustion,
            timer_from: None,
        }
    }

    /// Runs `tick` from `start` for `ticks` ticks at a fixed health (the region would
    /// apply the heal; these tests count when it comes), and returns each tick's result.
    fn run(start: FoodData, facts: &FoodFacts, ticks: i64) -> Vec<FoodTick> {
        let mut now = start;
        (1..=ticks)
            .map(|t| {
                let out = tick(now, 1000 + t, facts);
                now = out.food;
                out
            })
            .collect()
    }

    #[test]
    fn exhaustion_above_four_takes_one_saturation() {
        let out = tick(food(20, 3.0, 4.2), 1, &facts(20.0, Difficulty::Easy));
        assert_eq!(out.food.saturation, 2.0);
        assert_eq!(out.food.food, 20);
        assert!((out.food.exhaustion - 0.2).abs() < 1e-6);
    }

    #[test]
    fn exhaustion_with_no_saturation_takes_one_food() {
        let out = tick(food(15, 0.0, 4.2), 1, &facts(20.0, Difficulty::Easy));
        assert_eq!(out.food.food, 14);
        assert_eq!(out.food.saturation, 0.0);
        assert!((out.food.exhaustion - 0.2).abs() < 1e-6);
    }

    #[test]
    fn exhaustion_of_exactly_four_takes_nothing() {
        let out = tick(food(15, 0.0, 4.0), 1, &facts(20.0, Difficulty::Easy));
        assert_eq!(out.food.food, 15);
        assert_eq!(out.food.exhaustion, 4.0);
    }

    #[test]
    fn peaceful_spends_the_exhaustion_but_takes_no_food() {
        let out = tick(food(15, 0.0, 4.2), 1, &facts(20.0, Difficulty::Peaceful));
        assert_eq!(out.food.food, 15);
        assert!((out.food.exhaustion - 0.2).abs() < 1e-6);
    }

    #[test]
    fn a_hurt_player_at_full_food_heals_from_saturation_at_the_tenth_tick() {
        let out = run(food(20, 5.0, 0.0), &facts(15.0, Difficulty::Easy), 10);
        for (i, step) in out.iter().enumerate().take(9) {
            assert_eq!(step.heal, None, "tick {}", i + 1);
            assert_eq!(step.food.timer_from, Some(1000));
        }
        let last = out[9];
        assert_eq!(last.heal, Some(5.0 / 6.0));
        assert_eq!(last.food.exhaustion, 5.0);
        assert_eq!(last.food.timer_from, None);
        assert_eq!(last.starve, None);
    }

    #[test]
    fn the_fast_heal_spends_at_most_six_saturation() {
        let out = run(food(20, 9.0, 0.0), &facts(15.0, Difficulty::Easy), 10);
        assert_eq!(out[9].heal, Some(1.0));
        assert_eq!(out[9].food.exhaustion, 6.0);
    }

    #[test]
    fn a_hurt_player_at_eighteen_food_heals_one_at_the_eightieth_tick() {
        let out = run(food(18, 5.0, 0.0), &facts(15.0, Difficulty::Easy), 80);
        assert!(out[..79].iter().all(|step| step.heal.is_none()));
        assert_eq!(out[78].food.timer_from, Some(1000));
        assert_eq!(out[79].heal, Some(1.0));
        assert_eq!(out[79].food.exhaustion, 6.0);
        assert_eq!(out[79].food.timer_from, None);
    }

    #[test]
    fn no_heal_without_the_rule() {
        let mut no_rule = facts(15.0, Difficulty::Easy);
        no_rule.natural_regeneration = false;
        let out = run(food(20, 5.0, 0.0), &no_rule, 100);
        assert!(out.iter().all(|step| step.heal.is_none()));
        assert!(out.iter().all(|step| step.food.timer_from.is_none()));
    }

    #[test]
    fn starving_on_easy_stops_at_ten_health() {
        let out = run(food(0, 0.0, 0.0), &facts(11.0, Difficulty::Easy), 80);
        assert!(out[..79].iter().all(|step| step.starve.is_none()));
        assert_eq!(out[79].starve, Some(1.0));
        assert_eq!(out[79].food.timer_from, None);
        let out = run(food(0, 0.0, 0.0), &facts(10.0, Difficulty::Easy), 80);
        assert_eq!(out[79].starve, None);
        assert_eq!(out[79].food.timer_from, None, "reset even without a hurt");
    }

    #[test]
    fn starving_on_normal_stops_at_one_health() {
        let out = run(food(0, 0.0, 0.0), &facts(2.0, Difficulty::Normal), 80);
        assert_eq!(out[79].starve, Some(1.0));
        let out = run(food(0, 0.0, 0.0), &facts(1.0, Difficulty::Normal), 80);
        assert_eq!(out[79].starve, None);
    }

    #[test]
    fn starving_on_hard_hurts_at_any_health() {
        let out = run(food(0, 0.0, 0.0), &facts(1.0, Difficulty::Hard), 80);
        assert_eq!(out[79].starve, Some(1.0));
        let out = run(food(0, 0.0, 0.0), &facts(0.5, Difficulty::Hard), 80);
        assert_eq!(out[79].starve, Some(1.0));
    }

    #[test]
    fn starving_in_peaceful_hurts_above_ten_health_as_the_bytecode_has_it() {
        let out = run(food(0, 0.0, 0.0), &facts(12.0, Difficulty::Peaceful), 80);
        assert_eq!(out[79].starve, Some(1.0));
        let out = run(food(0, 0.0, 0.0), &facts(10.0, Difficulty::Peaceful), 80);
        assert_eq!(out[79].starve, None);
    }

    #[test]
    fn the_timer_resets_when_no_branch_applies() {
        let hurt = facts(15.0, Difficulty::Easy);
        let first = run(food(18, 5.0, 0.0), &hurt, 40);
        assert_eq!(first[39].food.timer_from, Some(1000));
        // Healed to full in between: no branch applies and the timer rests.
        let rested = tick(first[39].food, 1041, &facts(20.0, Difficulty::Easy));
        assert_eq!(rested.food.timer_from, None);
        // Hurt again, it counts from the start: 80 more ticks to the heal.
        let mut now = rested.food;
        for t in 1042..1042 + 79 {
            let out = tick(now, t, &hurt);
            assert_eq!(out.heal, None, "tick {t}");
            now = out.food;
        }
        assert_eq!(tick(now, 1042 + 79, &hurt).heal, Some(1.0));
    }

    #[test]
    fn one_timer_carries_from_the_slow_branch_to_the_fast() {
        let hurt = facts(15.0, Difficulty::Easy);
        let slow = run(food(18, 5.0, 0.0), &hurt, 30);
        let mut at_full = slow[29].food;
        at_full.food = 20;
        // The timer stands at 30, above the fast branch's 10: it heals at once.
        let out = tick(at_full, 1031, &hurt);
        assert_eq!(out.heal, Some(5.0 / 6.0));
    }

    #[test]
    fn eating_bread_from_ten_food_and_no_saturation() {
        let mut data = food(10, 0.0, 0.0);
        eat(&mut data, 5, 6.0);
        assert_eq!(data.food, 15);
        assert_eq!(data.saturation, 6.0);
    }

    #[test]
    fn eating_holds_food_to_twenty_and_saturation_to_the_food() {
        let mut data = food(18, 17.0, 0.0);
        eat(&mut data, 5, 6.0);
        assert_eq!(data.food, 20);
        assert_eq!(data.saturation, 20.0);
        let mut data = food(2, 0.0, 0.0);
        eat(&mut data, 1, 9.0);
        assert_eq!(data.saturation, 3.0);
    }

    #[test]
    fn eating_scaled_multiplies_nutrition_by_twice_the_modifier() {
        let mut data = food(10, 0.0, 0.0);
        eat_scaled(&mut data, 2, 1.0);
        assert_eq!(data.food, 12);
        assert_eq!(data.saturation, 4.0);
    }

    #[test]
    fn exhaustion_is_capped_at_forty() {
        let mut data = food(20, 5.0, 39.0);
        add_exhaustion(&mut data, 6.0);
        assert_eq!(data.exhaustion, 40.0);
    }

    #[test]
    fn an_invulnerable_player_is_not_exhausted() {
        let mut data = food(20, 5.0, 1.0);
        cause_exhaustion(&mut data, 2.0, true);
        assert_eq!(data.exhaustion, 1.0);
        cause_exhaustion(&mut data, 2.0, false);
        assert_eq!(data.exhaustion, 3.0);
    }

    #[test]
    fn a_player_can_eat_when_hungry_invulnerable_or_eating_what_is_always_eaten() {
        let full = food(20, 5.0, 0.0);
        assert!(!can_eat(&full, false, false));
        assert!(can_eat(&full, true, false));
        assert!(can_eat(&full, false, true));
        assert!(can_eat(&food(19, 5.0, 0.0), false, false));
    }

    #[test]
    fn peaceful_regeneration_heals_and_feeds_on_the_age() {
        let start = food(15, 3.0, 0.0);
        let at_20 = peaceful_regeneration(start, 40, 10.0, 20.0, Difficulty::Peaceful, true);
        assert_eq!(at_20.heal, Some(1.0));
        assert_eq!(at_20.food.saturation, 4.0);
        assert_eq!(at_20.food.food, 16);
        let at_10 = peaceful_regeneration(start, 30, 10.0, 20.0, Difficulty::Peaceful, true);
        assert_eq!(at_10.heal, None);
        assert_eq!(at_10.food.saturation, 3.0);
        assert_eq!(at_10.food.food, 16);
        let between = peaceful_regeneration(start, 31, 10.0, 20.0, Difficulty::Peaceful, true);
        assert_eq!(
            between,
            Regeneration {
                food: start,
                heal: None
            }
        );
    }

    #[test]
    fn peaceful_saturation_is_not_held_to_the_food() {
        let start = food(5, 5.0, 0.0);
        let out = peaceful_regeneration(start, 20, 20.0, 20.0, Difficulty::Peaceful, true);
        assert_eq!(out.heal, None, "not hurt");
        assert_eq!(out.food.saturation, 6.0);
        let full = food(20, 20.0, 0.0);
        let out = peaceful_regeneration(full, 20, 20.0, 20.0, Difficulty::Peaceful, true);
        assert_eq!(out.food, full);
    }

    #[test]
    fn regeneration_does_nothing_outside_peaceful_or_without_the_rule() {
        let start = food(15, 3.0, 0.0);
        let easy = peaceful_regeneration(start, 20, 10.0, 20.0, Difficulty::Easy, true);
        assert_eq!(
            easy,
            Regeneration {
                food: start,
                heal: None
            }
        );
        let off = peaceful_regeneration(start, 20, 10.0, 20.0, Difficulty::Peaceful, false);
        assert_eq!(
            off,
            Regeneration {
                food: start,
                heal: None
            }
        );
    }

    #[test]
    fn set_health_is_sent_on_health_food_or_saturation_reaching_zero() {
        let sent = HealthSent::of(20.0, &food(20, 5.0, 0.0));
        assert!(!needs_set_health(
            &sent,
            &HealthSent::of(20.0, &food(20, 5.0, 0.0))
        ));
        assert!(
            !needs_set_health(&sent, &HealthSent::of(20.0, &food(20, 4.0, 0.0))),
            "a change of saturation alone is not sent"
        );
        assert!(needs_set_health(
            &sent,
            &HealthSent::of(19.0, &food(20, 5.0, 0.0))
        ));
        assert!(needs_set_health(
            &sent,
            &HealthSent::of(20.0, &food(19, 5.0, 0.0))
        ));
        assert!(needs_set_health(
            &sent,
            &HealthSent::of(20.0, &food(20, 0.0, 0.0))
        ));
        let empty = HealthSent::of(20.0, &food(20, 0.0, 0.0));
        assert!(needs_set_health(
            &empty,
            &HealthSent::of(20.0, &food(20, 1.0, 0.0))
        ));
    }

    #[test]
    fn a_sprinted_metre_on_the_ground_costs_a_tenth() {
        let sprinting = MoveFacts {
            on_ground: true,
            sprinting: true,
            ..MoveFacts::default()
        };
        assert_eq!(centimetres(0.6, 0.0, 0.8, false), 100);
        let exhaustion = movement_exhaustion(0.6, 0.5, 0.8, &sprinting);
        assert_eq!(exhaustion, 0.1f32 * 100.0 * 0.01);
        assert!((exhaustion - 0.1).abs() < 1e-6);
    }

    #[test]
    fn walking_climbing_and_the_air_cost_nothing() {
        let walking = MoveFacts {
            on_ground: true,
            ..MoveFacts::default()
        };
        assert_eq!(movement_exhaustion(1.0, 0.0, 0.0, &walking), 0.0);
        let in_air = MoveFacts {
            sprinting: true,
            ..MoveFacts::default()
        };
        assert_eq!(movement_exhaustion(1.0, 0.0, 0.0, &in_air), 0.0);
        let climbing = MoveFacts {
            on_climbable: true,
            on_ground: true,
            sprinting: true,
            ..MoveFacts::default()
        };
        assert_eq!(movement_exhaustion(1.0, 0.0, 0.0, &climbing), 0.0);
        let riding = MoveFacts {
            passenger: true,
            swimming: true,
            ..MoveFacts::default()
        };
        assert_eq!(movement_exhaustion(1.0, 0.0, 0.0, &riding), 0.0);
    }

    #[test]
    fn water_costs_a_hundredth_a_metre_in_three_or_two_axes() {
        let swimming = MoveFacts {
            swimming: true,
            ..MoveFacts::default()
        };
        // A vertical metre counts only where all three axes are counted.
        assert_eq!(
            movement_exhaustion(0.0, 1.0, 0.0, &swimming),
            0.01 * 100.0 * 0.01
        );
        let under = MoveFacts {
            eyes_in_water: true,
            ..MoveFacts::default()
        };
        assert_eq!(
            movement_exhaustion(0.0, 1.0, 0.0, &under),
            0.01 * 100.0 * 0.01
        );
        let wading = MoveFacts {
            in_water: true,
            ..MoveFacts::default()
        };
        assert_eq!(movement_exhaustion(0.0, 1.0, 0.0, &wading), 0.0);
        assert_eq!(
            movement_exhaustion(1.0, 1.0, 0.0, &wading),
            0.01 * 100.0 * 0.01
        );
    }

    #[test]
    fn a_move_is_rounded_to_whole_centimetres() {
        assert_eq!(centimetres(0.004, 0.0, 0.0, false), 0);
        assert_eq!(centimetres(0.006, 0.0, 0.0, false), 1);
        let sprinting = MoveFacts {
            on_ground: true,
            sprinting: true,
            ..MoveFacts::default()
        };
        assert_eq!(movement_exhaustion(0.004, 0.0, 0.0, &sprinting), 0.0);
    }

    #[test]
    fn a_jump_costs_five_hundredths_and_a_sprinting_jump_a_fifth() {
        assert_eq!(jump_exhaustion(false), 0.05);
        assert_eq!(jump_exhaustion(true), 0.2);
    }
}
