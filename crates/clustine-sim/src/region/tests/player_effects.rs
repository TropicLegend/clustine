//! A player's effects (`docs/groundwork/n9-plan.md`, section 5.4; `region/player_effects.rs`):
//! their ends on the region's ticks, what the player's client is told, and their
//! carrying across a hand-over and a join.

use super::*;
use crate::api::AttributeShown;
use crate::mobs::body::effects::{Effect, Layer};
use crate::shown::DataShown;

fn effect_events(output: &TickOutput) -> Vec<PlayerEvent> {
    output
        .player_events
        .iter()
        .filter(|(id, _)| *id == player(1))
        .map(|(_, event)| event.clone())
        .filter(|event| {
            matches!(
                event,
                PlayerEvent::EffectAdded { .. }
                    | PlayerEvent::EffectRemoved { .. }
                    | PlayerEvent::Attributes(_)
            )
        })
        .collect()
}

fn swiftness(amplifier: u8, duration: i32, region: &Region) -> Layer {
    Layer::new(amplifier, duration, region.tick_number())
}

#[test]
fn swiftness_is_told_with_its_speed_and_ends_after_its_duration() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let layer = swiftness(1, 5, &region);
    assert!(region.add_player_effect(player(1), Effect::SPEED, layer, &mut output));
    let speed = AttributeShown {
        attribute: u16::try_from(clustine_data::attributes::MOVEMENT_SPEED).unwrap(),
        base: 0.10000000149011612,
        modifiers: vec![(
            "minecraft:effect.speed".to_owned(),
            0.200_000_002_980_232_24 * 2.0,
            2,
        )],
    };
    assert_eq!(
        effect_events(&output),
        [
            PlayerEvent::EffectAdded {
                instance: clustine_world::components::EffectInstance {
                    effect: "minecraft:speed".to_owned(),
                    amplifier: 1,
                    duration: 5,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                },
                blend: true,
            },
            PlayerEvent::Attributes(vec![speed.clone()]),
        ]
    );
    // Its five ticks: told nothing until it ends, and then the speed goes back.
    for _ in 0..4 {
        let output = region.tick(&TickInputs::default());
        assert!(effect_events(&output).is_empty());
    }
    let output = region.tick(&TickInputs::default());
    assert_eq!(
        effect_events(&output),
        [
            PlayerEvent::EffectRemoved {
                effect: "minecraft:speed".to_owned()
            },
            PlayerEvent::Attributes(vec![AttributeShown {
                modifiers: Vec::new(),
                ..speed
            }]),
        ]
    );
}

#[test]
fn a_stronger_shorter_effect_hides_the_weaker_which_comes_back_after_it() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    region.add_player_effect(
        player(1),
        Effect::SPEED,
        swiftness(0, 20, &region),
        &mut output,
    );
    region.add_player_effect(
        player(1),
        Effect::SPEED,
        swiftness(2, 3, &region),
        &mut output,
    );
    for _ in 0..2 {
        region.tick(&TickInputs::default());
    }
    let output = region.tick(&TickInputs::default());
    let added: Vec<i32> = effect_events(&output)
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::EffectAdded { instance, .. } => Some(instance.amplifier),
            _ => None,
        })
        .collect();
    assert_eq!(added, [0], "the weaker level is in force again");
}

#[test]
fn invisibility_and_glowing_are_shown_to_the_others_in_the_shared_flags() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let layer = Layer::new(0, 100, region.tick_number());
    region.add_player_effect(player(1), Effect::INVISIBILITY, layer, &mut output);
    region.add_player_effect(player(1), Effect::GLOWING, layer, &mut output);
    let flags: Vec<u8> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::PlayerData { values, .. } => values.iter().find_map(|value| match value {
                (0, DataShown::Byte(flags)) => Some(*flags),
                _ => None,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(flags, [1 << 5, (1 << 5) | (1 << 6)]);
}

#[test]
fn a_hand_over_carries_each_level_s_ticks_left_and_the_next_region_counts_them_on() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    region.add_player_effect(
        player(1),
        Effect::SPEED,
        swiftness(0, 40, &region),
        &mut output,
    );
    for _ in 0..10 {
        region.tick(&TickInputs::default());
    }
    let tick = region.tick_number();
    let player = region.players.remove(&player(1)).expect("player 1 is here");
    let transfer = player.into_transfer(tick);
    // Counted from the next region's first tick: added for forty, ten ticked here and
    // the next one there.
    let [ref carried] = transfer.effects[..] else {
        panic!("one effect: {:?}", transfer.effects);
    };
    assert_eq!(
        (carried.effect.as_str(), carried.duration),
        ("minecraft:speed", 29)
    );

    let mut next = joined_in(ChunkArea::EVERYWHERE, &[]);
    for _ in 0..1000 {
        next.tick(&TickInputs::default());
    }
    next.tick(&changes(vec![arrive(1, &transfer)]));
    let arrived = next
        .players
        .get(&super::player(1))
        .expect("player 1 arrived");
    let end = arrived.effects.active[0].top().end.expect("an end");
    assert_eq!(end, next.tick_number() + 29);
}

#[test]
fn an_effect_does_not_run_down_while_its_player_is_away() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    region.add_player_effect(
        player(1),
        Effect::SPEED,
        swiftness(0, 40, &region),
        &mut output,
    );
    let tick = region.tick_number();
    let place = region.players[&player(1)].place(tick);
    let [ref kept] = place.effects[..] else {
        panic!("one effect: {:?}", place.effects);
    };
    // As of the tick after: thirty-nine left, however long until they come back.
    assert_eq!(kept.duration, 39);
}

#[test]
fn an_effect_taken_off_is_told_ended_and_one_not_had_tells_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let layer = Layer::new(0, 100, region.tick_number());
    region.add_player_effect(player(1), Effect::POISON, layer, &mut output);
    let mut output = TickOutput::default();
    assert!(region.remove_player_effect(player(1), Effect::POISON, &mut output));
    assert_eq!(
        effect_events(&output),
        [PlayerEvent::EffectRemoved {
            effect: "minecraft:poison".to_owned()
        }]
    );
    let mut output = TickOutput::default();
    assert!(!region.remove_player_effect(player(1), Effect::POISON, &mut output));
    assert!(effect_events(&output).is_empty());
}
