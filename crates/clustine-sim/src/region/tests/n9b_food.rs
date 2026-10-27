//! Eating and drinking from the specification, by someone who wrote none of its code
//! (`docs/groundwork/n9-plan.md`, section 5.5 and decision 5 of section 18; n9
//! `Consumable`, `FoodProperties`, `ApplyStatusEffectsConsumeEffect`; the items' data):
//! every food whose data gives effects, eaten by a creative player, with its draws in
//! the game's order; and the uses' lengths by `consume_seconds`.
//!
//! The draws of a meal's end, all from the eater's own random (a player's stream aside):
//! `emitParticlesAndSounds(…, 16)` (a boolean, the triangle's two floats and the
//! drink's pitch float, then four floats a crumb for an item with crumbs); the stack's
//! listeners (a food's `FoodProperties.onConsume`: the sound's triangle, two floats, and
//! the burp's one); then each `on_consume_effects` entry in order, `apply_effects`
//! drawing one float against its probability whatever it is.

use clustine_data::items;
use clustine_world::{EntityId, ItemStack, PlayerId};

use super::*;
use crate::api::{Hand, Using};
use crate::mobs::body::effects::Effect;
use crate::region::random_ticks::Random;
use crate::region::tnt::tests::{add_player, idle, nothing, world};

const SEED: u64 = 0xF00D_0B19;

fn holding(stack: ItemStack) -> (Region, PlayerId, EntityId) {
    let mut region = world(SEED, nothing);
    let (id, entity) = add_player(&mut region, 1, [0.5, -60.0, 0.5], false);
    let player = region.players.get_mut(&id).expect("player 1");
    player.inventory.0[0] = Some(stack);
    player.selected_slot = 0;
    (region, id, entity)
}

/// An effect a food gives: its name, amplifier and duration.
type Given = (Effect, u8, i64);

/// An `apply_effects` entry: its probability and what it gives.
type Entry = (f32, Vec<Given>);

/// Every food of the data with `apply_effects`: its entries in order, each a probability
/// and what it gives (`components/item/*.json`, `on_consume_effects`).
fn foods() -> Vec<(i32, Vec<Entry>)> {
    let hunger = Effect::HUNGER;
    let poison = Effect::POISON;
    let nausea = Effect::named("minecraft:nausea").expect("nausea");
    vec![
        (items::CHICKEN, vec![(0.3, vec![(hunger, 0, 600)])]),
        (items::ROTTEN_FLESH, vec![(0.8, vec![(hunger, 0, 600)])]),
        (items::POISONOUS_POTATO, vec![(0.6, vec![(poison, 0, 100)])]),
        (items::SPIDER_EYE, vec![(1.0, vec![(poison, 0, 100)])]),
        (
            items::PUFFERFISH,
            vec![(
                1.0,
                vec![(poison, 1, 1200), (hunger, 2, 300), (nausea, 0, 300)],
            )],
        ),
        (
            items::GOLDEN_APPLE,
            vec![(
                1.0,
                vec![
                    (Effect::REGENERATION, 1, 100),
                    (Effect::ABSORPTION, 0, 2400),
                ],
            )],
        ),
        (
            items::ENCHANTED_GOLDEN_APPLE,
            vec![(
                1.0,
                vec![
                    (Effect::REGENERATION, 1, 400),
                    (Effect::RESISTANCE, 0, 6000),
                    (Effect::FIRE_RESISTANCE, 0, 6000),
                    (Effect::ABSORPTION, 3, 2400),
                ],
            )],
        ),
    ]
}

/// Each food eaten to its end (`finish_consuming`), in several regions so that its
/// draws fall both ways: the effects given are those of the entries whose draw fell below
/// their probability, at their amplifiers and for their durations, and the eater's
/// stream drew the mouthful's 4, sixteen crumbs' 64, the food's 3 and one an entry.
#[test]
fn every_food_s_effects_are_given_by_one_draw_an_entry_after_its_crumbs_and_sounds() {
    for (item, entries) in foods() {
        let mut seen = [false, false];
        for ticks in 0..24 {
            let (mut region, id, entity) = holding(ItemStack::new(item, 1));
            for _ in 0..ticks {
                idle(&mut region);
            }
            let mut output = TickOutput::default();
            region.finish_consuming(
                id,
                Using {
                    hand: Hand::Main,
                    item,
                    remaining: 0,
                },
                &mut output,
            );
            let mut replay = Random::of_entity_aside(SEED, entity, 0, region.tick);
            replay.next_bool();
            for _ in 0..3 + 16 * 4 + 3 {
                replay.next_float();
            }
            let mut expected: Vec<Given> = Vec::new();
            for (probability, given) in &entries {
                let draw = replay.next_float();
                if draw < *probability {
                    expected.extend(given.iter().copied());
                    seen[0] = true;
                } else {
                    seen[1] = true;
                }
            }
            let drawn = region.aside_draws.get(&entity).copied().unwrap_or(0);
            assert_eq!(
                drawn,
                4 + 64 + 3 + entries.len() as u64,
                "item {item}: the draws"
            );
            let player = region.players.get(&id).expect("player 1");
            let mut had: Vec<Given> = player
                .effects
                .active
                .iter()
                .map(|active| {
                    let top = active.top();
                    (active.effect, top.amplifier, top.duration(region.tick))
                })
                .collect();
            had.sort();
            expected.sort();
            assert_eq!(had, expected, "item {item} after {ticks} ticks");
        }
        let certain = entries.iter().all(|(probability, _)| *probability >= 1.0);
        assert!(seen[0], "item {item}: never given");
        assert!(certain || seen[1], "item {item}: never spared");
    }
}

/// Section 4.6 and 5.5: a use lasts `consume_seconds · 20` ticks: 32 for bread by
/// default, 16 for dried kelp (0.8 s), 40 for a honey bottle (2 s), 32 for milk; at its
/// end the item's sound is heard and the use is over.
#[test]
fn each_use_lasts_its_item_s_consume_seconds() {
    for (item, ticks) in [
        (items::BREAD, 32),
        (items::DRIED_KELP, 16),
        (items::HONEY_BOTTLE, 40),
        (items::MILK_BUCKET, 32),
    ] {
        let (mut region, id, _) = holding(ItemStack::new(item, 1));
        let mut output = TickOutput::default();
        assert!(
            region.start_item_use(id, Hand::Main, &mut output),
            "item {item}"
        );
        let mut ended = None;
        for tick in 1..=60 {
            idle(&mut region);
            if region.players.get(&id).expect("player 1").using.is_none() {
                ended = Some(tick);
                break;
            }
        }
        assert_eq!(ended, Some(ticks), "item {item}");
    }
}

/// The plan's 5.5 (decision 5): a suspicious stew's `suspicious_stew_effects` are one of
/// the stack's listeners, given with no draw of their own; the stew is a food, so its
/// meal's draws are those of any food (4, 64 for its crumbs, 3).
#[test]
fn a_suspicious_stew_gives_its_listed_effects_and_draws_only_a_food_s_draws() {
    let mut stew = ItemStack::new(items::SUSPICIOUS_STEW, 1);
    stew.components.set(
        clustine_world::components::Component::SuspiciousStewEffects(vec![
            ("minecraft:night_vision".to_owned(), 100),
            ("minecraft:weakness".to_owned(), 140),
        ]),
    );
    let (mut region, id, entity) = holding(stew);
    let mut output = TickOutput::default();
    region.finish_consuming(
        id,
        Using {
            hand: Hand::Main,
            item: items::SUSPICIOUS_STEW,
            remaining: 0,
        },
        &mut output,
    );
    assert_eq!(
        region.aside_draws.get(&entity).copied().unwrap_or(0),
        4 + 64 + 3
    );
    let player = region.players.get(&id).expect("player 1");
    let night_vision = Effect::named("minecraft:night_vision").expect("night vision");
    let mut had: Vec<(Effect, i64)> = player
        .effects
        .active
        .iter()
        .map(|active| (active.effect, active.top().duration(region.tick)))
        .collect();
    had.sort();
    let mut expected = vec![(night_vision, 100), (Effect::WEAKNESS, 140)];
    expected.sort();
    assert_eq!(had, expected);
}
