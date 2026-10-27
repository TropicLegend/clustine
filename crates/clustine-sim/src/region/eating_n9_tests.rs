//! Eating and drinking, and what a mob's effects do at its death and its hurt, from
//! `docs/groundwork/n9-plan.md`, sections 5.2 and 5.5, and the game's dumps (n9
//! `Consumable`, `FoodProperties`, the `ConsumeEffect` kinds, `OozingMobEffect`,
//! `WeavingMobEffect`, `WindChargedMobEffect`, `InfestedMobEffect`, `ServerExplosion`
//! and the blocks' `onExplosionHit`). The worlds are the TNT tests' (`tnt/tests.rs`): a
//! stone floor at y = -61, its top at -60.
//!
//! Draws are worked out from the streams the game draws them from: a player's and a
//! mob's between its ticks are their streams aside (`Random::of_entity_aside` of their
//! key, `lived` and the region's tick), which a test replays.

use clustine_blocks::data::flag;
use clustine_data::{BlockState, ItemComponents, entity_types, items, sounds};
use clustine_world::components::{Component, PotionContents};
use clustine_world::{BlockPos, Entity, EntityId, ItemStack, PlayerId};

use super::super::Region;
use super::super::effect_deaths::hash_set_order;
use super::super::random_ticks::Random;
use super::super::tnt::tests::{add, add_player, at, idle, record, records, state, world};
use crate::api::{Hand, PlayerEvent, RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput, Using};
use crate::items::consumables;
use crate::mobs::Mob;
use crate::mobs::body::effects::{Effect, Layer};

const SEED: u64 = 0x5EED_0051;

/// Where player 1 stands.
const FEET: [f64; 3] = [0.5, -60.0, 0.5];

/// Only the floor.
fn nothing(_: BlockPos) -> Option<BlockState> {
    None
}

/// A world with player 1 on the floor holding `stack` in their main hand.
fn holding(stack: ItemStack) -> (Region, PlayerId, EntityId) {
    let mut region = world(SEED, nothing);
    let (id, entity) = add_player(&mut region, 1, FEET, false);
    let player = region.players.get_mut(&id).expect("player 1");
    player.inventory.0[0] = Some(stack);
    player.selected_slot = 0;
    (region, id, entity)
}

/// The use of the main hand's `item` with `remaining` ticks left.
fn using(item: i32, remaining: i32) -> Using {
    Using {
        hand: Hand::Main,
        item,
        remaining,
    }
}

/// The player's (or any entity's) stream aside in the region's tick, from the start.
fn aside(region: &Region, key: EntityId, lived: u32) -> Random {
    Random::of_entity_aside(SEED, key, lived, region.tick)
}

/// What the stream aside of `key` has drawn in this tick.
fn drawn(region: &Region, key: EntityId) -> u64 {
    region.aside_draws.get(&key).copied().unwrap_or(0)
}

/// The sounds of `output`: (sound, source, volume, pitch, except).
fn sounds_of(output: &TickOutput) -> Vec<(i32, i32, f32, f32, Option<PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                source,
                volume,
                pitch,
                except,
                ..
            } => Some((*sound, *source, *volume, *pitch, *except)),
            _ => None,
        })
        .collect()
}

fn has_effect(region: &Region, id: PlayerId, effect: Effect) -> bool {
    region
        .players
        .get(&id)
        .is_some_and(|player| player.effects.has(effect))
}

/// A mouthful's draws as the game makes them: volume, the triangle's two, the drink's
/// pitch, then four a crumb.
fn mouthful(random: &mut Random, drink: bool, crumbs: u32) -> (f32, f32) {
    let volume = if random.next_bool() { 0.5 } else { 1.0 };
    let triangle = 1.0 + 0.2 * (random.next_float() - random.next_float());
    let between = random.next_float() * (1.0 - 0.9) + 0.9;
    for _ in 0..crumbs * 4 {
        random.next_float();
    }
    if drink {
        (0.5, between)
    } else {
        (volume, triangle)
    }
}

#[test]
fn a_meal_s_mouthfuls_come_every_fourth_tick_after_the_first_fifth_of_its_use() {
    let bread = ItemComponents::of(items::BREAD)
        .consumable
        .expect("bread is eaten");
    assert_eq!(consumables::consume_ticks(&bread), 32);
    // `(int) (32 · 0.21875)` is 7: from the ninth tick of use on, every fourth.
    let emitting: Vec<i32> = (1..=32)
        .rev()
        .filter(|remaining| consumables::should_emit(&bread, *remaining))
        .collect();
    assert_eq!(emitting, [24, 20, 16, 12, 8, 4]);
}

#[test]
fn a_mouthful_of_bread_draws_four_and_four_a_crumb_and_is_heard_by_the_others() {
    let (mut region, id, entity) = holding(ItemStack::new(items::BREAD, 1));
    let mut output = TickOutput::default();
    region.consume_tick(id, using(items::BREAD, 24), &mut output);
    let mut replay = aside(&region, entity, 0);
    let (volume, pitch) = mouthful(&mut replay, false, 5);
    assert_eq!(drawn(&region, entity), 4 + 5 * 4);
    assert_eq!(
        sounds_of(&output),
        [(
            sounds::ENTITY_GENERIC_EAT,
            SOUND_SOURCE_PLAYERS,
            volume,
            pitch,
            Some(id)
        )]
    );
    // Not every tick.
    let mut output = TickOutput::default();
    region.consume_tick(id, using(items::BREAD, 23), &mut output);
    assert!(sounds_of(&output).is_empty());
    assert_eq!(drawn(&region, entity), 24, "nothing drawn");
}

#[test]
fn bread_eaten_is_heard_with_its_crumbs_its_meal_and_its_burp_and_is_not_used_up() {
    let (mut region, id, entity) = holding(ItemStack::new(items::BREAD, 3));
    let mut output = TickOutput::default();
    region.finish_consuming(id, using(items::BREAD, 0), &mut output);
    let mut replay = aside(&region, entity, 0);
    let (volume, pitch) = mouthful(&mut replay, false, 16);
    let eaten = 1.0 + 0.4 * (replay.next_float() - replay.next_float());
    let burp = replay.next_float() * (1.0 - 0.9) + 0.9;
    assert_eq!(drawn(&region, entity), 4 + 16 * 4 + 3);
    assert_eq!(
        sounds_of(&output),
        [
            (
                sounds::ENTITY_GENERIC_EAT,
                SOUND_SOURCE_PLAYERS,
                volume,
                pitch,
                Some(id)
            ),
            (sounds::ENTITY_GENERIC_EAT, 6, 1.0, eaten, None),
            (
                sounds::ENTITY_PLAYER_BURP,
                SOUND_SOURCE_PLAYERS,
                0.5,
                burp,
                None
            ),
        ]
    );
    assert_eq!(
        region.in_hand(id, Hand::Main),
        Some(ItemStack::new(items::BREAD, 3)),
        "a creative player uses nothing up"
    );
}

#[test]
fn rotten_flesh_gives_hunger_where_its_draw_falls_below_eight_tenths() {
    let mut given = 0;
    let mut spared = 0;
    for ticks in 0..24 {
        let (mut region, id, entity) = holding(ItemStack::new(items::ROTTEN_FLESH, 1));
        for _ in 0..ticks {
            idle(&mut region);
        }
        let mut output = TickOutput::default();
        region.finish_consuming(id, using(items::ROTTEN_FLESH, 0), &mut output);
        let mut replay = aside(&region, entity, 0);
        mouthful(&mut replay, false, 16);
        replay.next_float();
        replay.next_float();
        replay.next_float();
        let chance = replay.next_float();
        assert_eq!(
            drawn(&region, entity),
            4 + 64 + 3 + 1,
            "the draw is made always"
        );
        let hungry = has_effect(&region, id, Effect::HUNGER);
        assert_eq!(hungry, chance < 0.8, "draw {chance}");
        if hungry {
            given += 1;
            let player = region.players.get(&id).expect("player 1");
            let level = player.effects.active[0].top();
            assert_eq!(level.duration(region.tick), 600);
        } else {
            spared += 1;
        }
    }
    assert!(given > 0 && spared > 0, "both ways: {given} and {spared}");
}

#[test]
fn honey_takes_poison_away_and_is_heard_as_a_drink_without_crumbs() {
    let (mut region, id, entity) = holding(ItemStack::new(items::HONEY_BOTTLE, 1));
    let mut output = TickOutput::default();
    let layer = Layer::new(0, 200, region.tick);
    region.add_player_effect(id, Effect::POISON, layer, &mut output);
    region.add_player_effect(id, Effect::SPEED, layer, &mut output);
    let mut output = TickOutput::default();
    region.finish_consuming(id, using(items::HONEY_BOTTLE, 0), &mut output);
    assert!(!has_effect(&region, id, Effect::POISON));
    assert!(has_effect(&region, id, Effect::SPEED), "only poison");
    assert!(output.player_events.contains(&(
        id,
        PlayerEvent::EffectRemoved {
            effect: "minecraft:poison".to_owned()
        }
    )));
    let mut replay = aside(&region, entity, 0);
    let (volume, pitch) = mouthful(&mut replay, true, 0);
    assert_eq!(volume, 0.5);
    assert_eq!(drawn(&region, entity), 4 + 3, "a drink throws no crumbs");
    assert_eq!(
        sounds_of(&output)[0],
        (
            sounds::ITEM_HONEY_BOTTLE_DRINK,
            SOUND_SOURCE_PLAYERS,
            0.5,
            pitch,
            Some(id)
        )
    );
}

#[test]
fn milk_clears_every_effect_and_tells_the_client_of_each() {
    let (mut region, id, entity) = holding(ItemStack::new(items::MILK_BUCKET, 1));
    let mut output = TickOutput::default();
    let layer = Layer::new(1, 200, region.tick);
    region.add_player_effect(id, Effect::POISON, layer, &mut output);
    region.add_player_effect(id, Effect::GLOWING, layer, &mut output);
    let mut output = TickOutput::default();
    region.finish_consuming(id, using(items::MILK_BUCKET, 0), &mut output);
    let player = region.players.get(&id).expect("player 1");
    assert!(player.effects.active.is_empty());
    let removed: Vec<&str> = output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::EffectRemoved { effect } => Some(effect.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(removed, ["minecraft:poison", "minecraft:glowing"]);
    assert_eq!(drawn(&region, entity), 4, "milk has no food");
    assert_eq!(
        region.in_hand(id, Hand::Main),
        Some(ItemStack::new(items::MILK_BUCKET, 1)),
        "no bucket left in its place in creative"
    );
}

#[test]
fn a_potion_gives_its_effects_but_not_an_instant_one_to_a_creative_player() {
    let potion = |name: &str| {
        let mut stack = ItemStack::new(items::POTION, 1);
        stack
            .components
            .set(Component::PotionContents(PotionContents {
                potion: Some(name.to_owned()),
                ..PotionContents::default()
            }));
        stack
    };
    let (mut region, id, entity) = holding(potion("minecraft:swiftness"));
    let mut output = TickOutput::default();
    region.finish_consuming(id, using(items::POTION, 0), &mut output);
    let player = region.players.get(&id).expect("player 1");
    let speed = player
        .effects
        .active
        .iter()
        .find(|active| active.effect == Effect::SPEED)
        .expect("swiftness given");
    assert_eq!(speed.top().amplifier, 0);
    assert_eq!(speed.top().duration(region.tick), 3600);
    assert_eq!(drawn(&region, entity), 4);

    let (mut region, id, _) = holding(potion("minecraft:healing"));
    let mut output = TickOutput::default();
    region.finish_consuming(id, using(items::POTION, 0), &mut output);
    let player = region.players.get(&id).expect("player 1");
    assert!(player.effects.active.is_empty(), "nothing to heal");
}

#[test]
fn the_ominous_bottle_gives_bad_omen_with_its_sound_and_is_heard_disposed_of() {
    let mut bottle = ItemStack::new(items::OMINOUS_BOTTLE, 1);
    bottle.components.set(Component::OminousBottleAmplifier(2));
    let (mut region, id, _) = holding(bottle);
    let mut output = TickOutput::default();
    region.finish_consuming(id, using(items::OMINOUS_BOTTLE, 0), &mut output);
    let player = region.players.get(&id).expect("player 1");
    let omen = &player.effects.active[0];
    assert_eq!(omen.effect, Effect::BAD_OMEN);
    let level = omen.top();
    assert_eq!(level.amplifier, 2);
    assert_eq!(level.duration(region.tick), 120_000);
    assert!(!level.visible && !level.ambient && level.show_icon);
    let heard: Vec<(i32, Option<PlayerId>)> = sounds_of(&output)
        .into_iter()
        .map(|(sound, _, _, _, except)| (sound, except))
        .collect();
    assert!(heard.contains(&(sounds::EVENT_MOB_EFFECT_BAD_OMEN, None)));
    assert!(heard.contains(&(sounds::ITEM_OMINOUS_BOTTLE_DISPOSE, None)));
    // Disposed of at the middle of the block they stand in.
    let disposed = output.events.iter().find_map(|event| match event {
        RegionEvent::Sound {
            sound, position, ..
        } if *sound == sounds::ITEM_OMINOUS_BOTTLE_DISPOSE => Some(*position),
        _ => None,
    });
    assert_eq!(disposed, Some(clustine_world::Vec3::new(0.5, -59.5, 0.5)));
}

#[test]
fn the_chorus_fruit_teleports_after_its_meal_and_leaves_a_second_s_cooldown() {
    let fruit = ItemStack::new(items::CHORUS_FRUIT, 1);
    let (mut region, id, entity) = holding(fruit.clone());
    let mut output = TickOutput::default();
    region.finish_consuming(id, using(items::CHORUS_FRUIT, 0), &mut output);
    // The meal's draws, the food's, then the teleport's (`teleports.rs`, which draws
    // nothing until it is filled).
    assert!(drawn(&region, entity) >= 4 + 64 + 3);
    assert!(output.player_events.contains(&(
        id,
        PlayerEvent::Cooldown {
            group: "minecraft:chorus_fruit".to_owned(),
            ticks: 20
        }
    )));
    assert!(region.is_on_cooldown(id, &fruit));
}

#[test]
fn bread_eaten_through_the_use_is_heard_at_its_mouthfuls_and_at_its_end() {
    let (mut region, id, _) = holding(ItemStack::new(items::BREAD, 1));
    let mut output = TickOutput::default();
    assert!(region.start_item_use(id, Hand::Main, &mut output));
    let mut eaten = Vec::new();
    for _ in 0..32 {
        let output = idle(&mut region);
        let heard = sounds_of(&output)
            .iter()
            .filter(|(sound, ..)| *sound == sounds::ENTITY_GENERIC_EAT)
            .count();
        eaten.push(heard);
    }
    // Remaining 24, 20, 16, 12, 8 and 4 before the count down: the 9th to the 29th tick;
    // at the 32nd the last mouthful and the meal.
    let expected: Vec<usize> = (1..=32)
        .map(|tick| match tick {
            9 | 13 | 17 | 21 | 25 | 29 => 1,
            32 => 2,
            _ => 0,
        })
        .collect();
    assert_eq!(eaten, expected);
    assert!(
        region
            .players
            .get(&id)
            .is_some_and(|player| player.using.is_none())
    );
}

#[test]
fn a_survivor_s_meal_is_used_up_and_leaves_its_remainder() {
    let soup = ItemStack::new(items::MUSHROOM_STEW, 1);
    assert_eq!(
        consumables::eaten(&soup, false),
        (Some(ItemStack::new(items::BOWL, 1)), None)
    );
    let bottles = ItemStack::new(items::HONEY_BOTTLE, 3);
    assert_eq!(
        consumables::eaten(&bottles, false),
        (
            Some(ItemStack::new(items::HONEY_BOTTLE, 2)),
            Some(ItemStack::new(items::GLASS_BOTTLE, 1))
        )
    );
    let bread = ItemStack::new(items::BREAD, 1);
    assert_eq!(consumables::eaten(&bread, false), (None, None));
    assert_eq!(consumables::eaten(&bread, true), (Some(bread), None));
}

// ------------------------------------------------------------------------------------
// The effects at a death and a hurt.

/// A cow of the region at `feet`, of full health, without AI.
fn cow(region: &mut Region, feet: [f64; 3]) -> EntityId {
    mob(region, entity_types::COW, feet)
}

fn mob(region: &mut Region, kind: i32, feet: [f64; 3]) -> EntityId {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.no_ai = true;
    let mut entity = record(region, kind, feet, mob.encode());
    entity.on_ground = true;
    add(region, entity)
}

fn entity(region: &Region, key: EntityId) -> Entity {
    region.find_entity(key).expect("here").1.clone()
}

fn blocks_set(output: &TickOutput) -> Vec<(BlockPos, BlockState)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } => Some((*position, *state)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_java_hash_set_of_positions_gives_them_by_bucket_then_as_added() {
    let positions = [at(0, -60, 0), at(1, -60, 0), at(0, -60, 1), at(5, -60, -3)];
    // Buckets 3, 2, 2 and 1 (`(y + z · 31) · 31 + x`, spread).
    assert_eq!(
        hash_set_order(&positions),
        [at(5, -60, -3), at(1, -60, 0), at(0, -60, 1), at(0, -60, 0)]
    );
}

#[test]
fn weaving_weaves_two_or_three_cobwebs_on_sturdy_ground_about_its_dead_bearer() {
    let mut region = world(SEED, nothing);
    let key = cow(&mut region, [4.5, -60.0, 4.5]);
    let lived = entity(&region, key).lived;
    let mut output = TickOutput::default();
    region.effects_at_removal(key, [4.5, -60.0, 4.5], &[(Effect::WEAVING, 0)], &mut output);
    // Replayed: the count, then the cube's positions until enough stand on the floor.
    let mut replay = aside(&region, key, lived);
    let most = 2 + replay.next_int(2) as usize;
    let mut taken = Vec::new();
    let mut draws = 1;
    for _ in 0..15 {
        let pos = at(
            3 + replay.next_int(3) as i32,
            -61 + replay.next_int(3) as i32,
            3 + replay.next_int(3) as i32,
        );
        draws += 3;
        if taken.contains(&pos) {
            continue;
        }
        // Air above the stone floor's top alone.
        if pos.y == -60 {
            taken.push(pos);
            if taken.len() >= most {
                break;
            }
        }
    }
    assert_eq!(drawn(&region, key), draws);
    let woven: Vec<BlockPos> = blocks_set(&output)
        .into_iter()
        .filter(|(_, state)| *state == clustine_data::blocks::COBWEB)
        .map(|(pos, _)| pos)
        .collect();
    assert!(woven.len() >= 2, "{woven:?}");
    assert_eq!(woven, hash_set_order(&taken));
    let heard: Vec<BlockPos> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind: 3018,
                position,
                ..
            } => Some(*position),
            _ => None,
        })
        .collect();
    assert_eq!(heard, woven);
}

#[test]
fn oozing_counts_the_slimes_about_its_bearer_and_makes_none_while_the_slime_is_unbuilt() {
    let mut region = world(SEED, nothing);
    let key = cow(&mut region, [4.5, -60.0, 4.5]);
    for n in 0..3 {
        mob(
            &mut region,
            entity_types::SLIME,
            [5.0 + f64::from(n), -60.0, 4.5],
        );
    }
    // One beyond the box inflated by 2.
    mob(&mut region, entity_types::SLIME, [12.5, -60.0, 4.5]);
    let record = entity(&region, key);
    assert_eq!(region.nearby_slimes(&record, record.position, 24), 3);
    assert_eq!(
        region.nearby_slimes(&record, record.position, 2),
        2,
        "capped"
    );
    let before = records(&region).len();
    let mut output = TickOutput::default();
    region.effects_at_removal(key, record.position, &[(Effect::OOZING, 0)], &mut output);
    assert!(!crate::mobs::types::is_built(entity_types::SLIME as u16));
    assert_eq!(records(&region).len(), before, "the game's null: none made");
    assert_eq!(drawn(&region, key), 0, "nor drawn");
}

#[test]
fn infested_draws_its_chance_at_each_hurt_and_its_count_where_the_chance_holds() {
    let mut held = 0;
    for ticks in 0..30 {
        let mut region = world(SEED, nothing);
        for _ in 0..ticks {
            idle(&mut region);
        }
        let key = cow(&mut region, [4.5, -60.0, 4.5]);
        let lived = entity(&region, key).lived;
        let mut output = TickOutput::default();
        region.effects_at_hurt(
            key,
            [4.5, -60.0, 4.5],
            &[(Effect::INFESTED, 0)],
            &mut output,
        );
        let mut replay = aside(&region, key, lived);
        let chance = replay.next_float();
        let expected = if chance <= 0.1 { 2 } else { 1 };
        if chance <= 0.1 {
            held += 1;
        }
        assert_eq!(drawn(&region, key), expected, "chance {chance}");
        assert!(
            records(&region)
                .iter()
                .all(|entity| i32::from(entity.kind) != entity_types::SILVERFISH),
            "none made while the silverfish is unbuilt"
        );
    }
    assert!(held > 0, "the chance held at least once in thirty");
}

#[test]
fn a_wind_charged_death_bursts_pushing_and_triggering_but_hurting_and_breaking_nothing() {
    let button = at(6, -60, 4);
    let lever = at(4, -60, 6);
    let oak = at(2, -60, 4);
    let iron = at(4, -60, 2);
    let candle = at(6, -60, 6);
    let blocks = move |pos: BlockPos| {
        Some(match pos {
            _ if pos == button => state("minecraft:oak_button[face=floor,powered=false]"),
            _ if pos == lever => state("minecraft:lever[face=floor,powered=false]"),
            _ if pos == oak => state("minecraft:oak_door[half=lower,open=false]"),
            _ if pos == at(2, -59, 4) => state("minecraft:oak_door[half=upper,open=false]"),
            _ if pos == iron => state("minecraft:iron_door[half=lower,open=false]"),
            _ if pos == at(4, -59, 2) => state("minecraft:iron_door[half=upper,open=false]"),
            _ if pos == candle => state("minecraft:candle[candles=1,lit=true]"),
            _ => return None,
        })
    };
    let mut region = world(SEED, blocks);
    let feet = [4.5, -60.0, 4.5];
    let key = cow(&mut region, feet);
    let other = cow(&mut region, [5.5, -60.0, 4.5]);
    let lived = entity(&region, key).lived;
    let mut output = TickOutput::default();
    region.effects_at_removal(key, feet, &[(Effect::WIND_CHARGED, 0)], &mut output);
    let block = |pos: BlockPos| region.block(pos).expect("loaded");
    assert!(flag(block(button), "powered"), "the button pressed");
    assert!(flag(block(lever), "powered"), "the lever pulled");
    assert!(flag(block(oak), "open"), "the oak door opened");
    assert!(!flag(block(iron), "open"), "the wind opens no iron door");
    assert!(!flag(block(candle), "lit"), "the candle put out");
    // Pushed away from the burst, and not hurt.
    let pushed = entity(&region, other);
    assert!(pushed.motion[0] > 0.0, "{:?}", pushed.motion);
    let health = Mob::decode(&pushed.data).expect("a mob").body.vitals.health;
    assert_eq!(health, 10.0);
    // Nothing broken: every block still there but those it changed.
    assert!(
        blocks_set(&output)
            .iter()
            .all(|(_, state)| !state.props().is_air())
    );
    // Its radius from the bearer's own first draw, and its sound at its middle.
    let mut replay = aside(&region, key, lived);
    let radius = 3.0 + replay.next_float() * 2.0;
    assert!((3.0..5.0).contains(&radius));
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ENTITY_BREEZE_WIND_BURST)
    );
    // Its own, the rays' 1352 and the shuffle's draws, then the door's pitch.
    assert!(drawn(&region, key) > 1 + 1352);
}

/// The draws of a hurt of 1 of magic between the ticks of a cow with `effects`.
fn hurt_draws(effects: &[(Effect, u8)]) -> u64 {
    let mut region = world(SEED, nothing);
    let key = cow(&mut region, [4.5, -60.0, 4.5]);
    let mut record = entity(&region, key);
    let mut mob = Mob::decode(&record.data).expect("a mob");
    for (effect, amplifier) in effects {
        crate::mobs::body::effects::add(&mut mob, record.kind, *effect, *amplifier, 200, 0);
    }
    record.data = mob.encode();
    region
        .chunks
        .get_mut(&record.chunk())
        .expect("the chunk is loaded")
        .apply_entity(&clustine_world::EntityChange::Whole(record));
    let source =
        crate::mobs::body::hurt::DamageSource::of(crate::mobs::body::hurt::DamageType::MAGIC);
    assert!(region.hurt_mob(key, &source, 1.0, &mut TickOutput::default()));
    drawn(&region, key)
}

#[test]
fn a_mob_hurt_between_its_ticks_draws_its_infested_chance_after_the_hurt() {
    let plain = hurt_draws(&[]);
    let infested = hurt_draws(&[(Effect::INFESTED, 0)]);
    assert!(
        infested == plain + 1 || infested == plain + 2,
        "{plain} and {infested}"
    );
}
