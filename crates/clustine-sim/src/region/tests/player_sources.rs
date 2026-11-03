//! Tests of what hurts a player in their own tick (V4a, `player_sources.rs`): fire,
//! lava, drowning and Respiration, a wall, the blocks' hurts (cactus, a berry bush,
//! magma, powder snow), the effects' ticks, a creative player spared, and the cooldown
//! between two sources of one tick.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, made a survival player whose client has
//! loaded.

use clustine_data::items;
use clustine_world::ARMOUR_START;
use clustine_world::components::Component;

use super::redstone::{state, ticking_with};
use super::*;
use crate::mobs::body::effects::{Effect, Layer};
use crate::mobs::body::hurt::DamageType;

const FEET: BlockPos = BlockPos::new(0, -60, 0);
const EYES: BlockPos = BlockPos::new(0, -59, 0);
const FLOOR: BlockPos = BlockPos::new(0, -61, 0);

/// A region with `blocks` whose player 1, in `mode`, has loaded and holds nothing.
fn in_mode(mode: clustine_world::GameMode, blocks: &[(BlockPos, &str)]) -> Region {
    let blocks: Vec<(BlockPos, BlockState)> =
        blocks.iter().map(|(at, text)| (*at, state(text))).collect();
    let mut region = ticking_with(&blocks);
    let own = region.players.get_mut(&player(1)).expect("joined");
    own.mode = clustine_world::PlayerMode::new(mode);
    own.inventory = Inventory::default();
    own.hurting.loading.mark_loaded();
    region
}

fn survival(blocks: &[(BlockPos, &str)]) -> Region {
    in_mode(clustine_world::GameMode::Survival, blocks)
}

fn own(region: &mut Region) -> &mut Player {
    region.players.get_mut(&player(1)).expect("joined")
}

fn health(region: &Region) -> f32 {
    region.players.get(&player(1)).expect("joined").health
}

fn vitals(region: &Region) -> crate::state::Vitals {
    region
        .players
        .get(&player(1))
        .expect("joined")
        .vitals
        .clone()
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

/// The damage types of the damage events of `output`, in order.
fn damage_types(output: &TickOutput) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::DamageEvent { damage_type, .. } => Some(*damage_type),
            _ => None,
        })
        .collect()
}

fn id_of(kind: &'static str) -> i32 {
    DamageType(kind).id().expect("a damage type of the game")
}

fn give(region: &mut Region, effect: Effect, amplifier: u8, duration: i32) {
    let layer = Layer::new(amplifier, duration, region.tick_number());
    let mut output = TickOutput::default();
    region.add_player_effect(player(1), effect, layer, &mut output);
}

fn entity_events(output: &TickOutput, wanted: u8) -> usize {
    output
        .events
        .iter()
        .filter(|event| matches!(event, RegionEvent::EntityEvent { event, .. } if *event == wanted))
        .count()
}

#[test]
fn a_burning_player_takes_one_every_twenty_ticks_and_water_puts_the_fire_out() {
    let mut region = survival(&[]);
    own(&mut region).vitals.fire = 100;
    let output = idle(&mut region);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:on_fire")]);
    assert_eq!(health(&region), 19.0);
    assert_eq!(vitals(&region).fire, 99);
    for _ in 0..19 {
        let output = idle(&mut region);
        assert!(damage_types(&output).is_empty());
    }
    // The fire ticks at 80 at the start of the tick: the next hurt.
    let output = idle(&mut region);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:on_fire")]);
    assert_eq!(health(&region), 18.0);

    let mut region = survival(&[(FEET, "minecraft:water")]);
    own(&mut region).vitals.fire = 100;
    let output = idle(&mut region);
    // Put out in the water's step, the fire then rests at a player's −20 out of fire.
    assert!(vitals(&region).fire <= 0, "{:?}", vitals(&region).fire);
    let extinguished = output.events.iter().any(|event| {
        matches!(event, RegionEvent::Sound { sound, .. }
            if *sound == clustine_data::sounds::ENTITY_GENERIC_EXTINGUISH_FIRE)
    });
    assert!(extinguished);
    idle(&mut region);
    assert_eq!(vitals(&region).fire, -20);
}

#[test]
fn lava_hurts_four_and_sets_the_player_on_fire_for_fifteen_seconds() {
    let mut region = survival(&[(FEET, "minecraft:lava")]);
    let output = idle(&mut region);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:lava")]);
    assert_eq!(health(&region), 16.0);
    assert_eq!(vitals(&region).fire, 300);
    let burnt = output.events.iter().any(|event| {
        matches!(event, RegionEvent::Sound { sound, volume, .. }
            if *sound == clustine_data::sounds::ENTITY_GENERIC_BURN && *volume == 0.4)
    });
    assert!(burnt);
    let told = output.player_events.iter().any(|(id, event)| {
        *id == player(1) && matches!(event, PlayerEvent::Health { health, .. } if *health == 16.0)
    });
    assert!(told);
}

#[test]
fn under_water_the_air_runs_down_one_a_tick_and_at_minus_twenty_drowning_hurts_two() {
    let mut region = survival(&[(FEET, "minecraft:water"), (EYES, "minecraft:water")]);
    idle(&mut region);
    assert_eq!(vitals(&region).air, 299);
    idle(&mut region);
    assert_eq!(vitals(&region).air, 298);
    own(&mut region).vitals.air = -19;
    let output = idle(&mut region);
    assert_eq!(vitals(&region).air, 0);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:drown")]);
    assert_eq!(entity_events(&output, 67), 1);
    assert_eq!(health(&region), 18.0);
    let told = output.player_events.iter().any(|(id, event)| {
        *id == player(1) && matches!(event, PlayerEvent::OwnData(own) if own.air == 0)
    });
    assert!(told);
}

#[test]
fn respiration_keeps_the_air_on_some_ticks_by_the_players_draws() {
    let mut helmet = ItemStack::new(items::DIAMOND_HELMET, 1);
    helmet.components.set(Component::Enchantments(vec![(
        "minecraft:respiration".to_owned(),
        3,
    )]));
    let mut region = survival(&[(FEET, "minecraft:water"), (EYES, "minecraft:water")]);
    own(&mut region)
        .inventory
        .set(ARMOUR_START + 3, Some(helmet));
    // The equipment is looked at after the inputs of the next tick.
    idle(&mut region);
    own(&mut region).vitals.air = 300;
    for _ in 0..40 {
        idle(&mut region);
    }
    // Respiration III keeps the air on a draw of 1/4 or more: about three ticks in four.
    let lost = 300 - vitals(&region).air;
    assert!(lost > 0 && lost < 25, "lost {lost}");
}

#[test]
fn out_of_water_the_air_comes_back_four_a_tick() {
    let mut region = survival(&[]);
    own(&mut region).vitals.air = 100;
    idle(&mut region);
    assert_eq!(vitals(&region).air, 104);
    own(&mut region).vitals.air = 298;
    idle(&mut region);
    assert_eq!(vitals(&region).air, 300);
}

#[test]
fn a_player_in_a_wall_takes_one_a_tick() {
    // Stone at the feet and the eyes: no pose fits, and the eyes are in stone.
    let mut region = survival(&[(FEET, "minecraft:stone"), (EYES, "minecraft:stone")]);
    let output = idle(&mut region);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:in_wall")]);
    assert_eq!(health(&region), 19.0);
}

#[test]
fn a_cactus_hurts_one() {
    let mut region = survival(&[(FEET, "minecraft:cactus")]);
    let output = idle(&mut region);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:cactus")]);
    assert_eq!(health(&region), 19.0);
}

/// Player 1's move to `to`, numbered `number`.
fn move_to(number: u64, to: Vec3) -> TickInputs {
    moves(vec![with_number(
        number,
        numbered(
            1,
            PlayerInput::Move {
                position: Some(to),
                rotation: None,
                on_ground: true,
            },
        ),
    )])
}

#[test]
fn a_grown_berry_bush_hurts_a_moving_player_and_not_a_still_one() {
    let mut region = survival(&[(FEET, "minecraft:sweet_berry_bush[age=1]")]);
    let output = idle(&mut region);
    assert!(damage_types(&output).is_empty());
    assert_eq!(health(&region), 20.0);
    let output = region.tick(&move_to(41, Vec3::new(0.5, -60.0, 0.6)));
    assert_eq!(
        damage_types(&output),
        vec![id_of("minecraft:sweet_berry_bush")]
    );
    assert_eq!(health(&region), 19.0);

    // A bush of the first age hurts nobody.
    let mut region = survival(&[(FEET, "minecraft:sweet_berry_bush[age=0]")]);
    let output = region.tick(&move_to(41, Vec3::new(0.5, -60.0, 0.6)));
    assert!(damage_types(&output).is_empty());
}

#[test]
fn magma_hurts_a_player_standing_on_it_unless_they_sneak() {
    let mut region = survival(&[(FLOOR, "minecraft:magma_block")]);
    own(&mut region).pose.on_ground = true;
    let output = idle(&mut region);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:hot_floor")]);
    assert_eq!(health(&region), 19.0);

    let mut region = survival(&[(FLOOR, "minecraft:magma_block")]);
    own(&mut region).pose.on_ground = true;
    own(&mut region).keys.sneak = true;
    let output = idle(&mut region);
    assert!(damage_types(&output).is_empty());
    assert_eq!(health(&region), 20.0);
}

#[test]
fn powder_snow_freezes_to_one_hundred_and_forty_then_hurts_one_every_forty_ticks() {
    let mut region = survival(&[(FEET, "minecraft:powder_snow")]);
    own(&mut region).vitals.frozen = 0;
    idle(&mut region);
    assert_eq!(vitals(&region).frozen, 1);
    own(&mut region).vitals.frozen = 138;
    idle(&mut region);
    assert_eq!(vitals(&region).frozen, 139);
    idle(&mut region);
    assert_eq!(vitals(&region).frozen, 140);
    let mut hurts = Vec::new();
    for _ in 0..80 {
        let output = idle(&mut region);
        assert_eq!(vitals(&region).frozen, 140);
        hurts.extend(damage_types(&output));
    }
    assert_eq!(hurts, vec![id_of("minecraft:freeze"); 2]);
    assert_eq!(health(&region), 18.0);
}

#[test]
fn leather_boots_keep_powder_snow_from_freezing_a_player() {
    let mut region = survival(&[(FEET, "minecraft:powder_snow")]);
    own(&mut region)
        .inventory
        .set(ARMOUR_START, Some(ItemStack::new(items::LEATHER_BOOTS, 1)));
    own(&mut region).vitals.frozen = 140;
    let mut hurts = Vec::new();
    for _ in 0..80 {
        let output = idle(&mut region);
        hurts.extend(damage_types(&output));
    }
    assert_eq!(vitals(&region).frozen, 0);
    assert!(hurts.is_empty());
    assert_eq!(health(&region), 20.0);
}

#[test]
fn poison_hurts_every_twenty_five_ticks_and_stops_at_one_health() {
    let mut region = survival(&[]);
    give(&mut region, Effect::POISON, 0, 1000);
    own(&mut region).health = 3.0;
    let mut hurts = Vec::new();
    for _ in 0..100 {
        let output = idle(&mut region);
        hurts.extend(damage_types(&output));
    }
    assert_eq!(hurts, vec![id_of("minecraft:magic"); 2]);
    assert_eq!(health(&region), 1.0);
    assert!(!region.players.get(&player(1)).expect("joined").dead);
}

#[test]
fn wither_kills() {
    let mut region = survival(&[]);
    give(&mut region, Effect::WITHER, 0, 1000);
    own(&mut region).health = 1.0;
    let mut died = false;
    for _ in 0..41 {
        let output = idle(&mut region);
        died |= output
            .durable
            .iter()
            .any(|(_, _, durable)| matches!(durable, Durable::Died { .. }));
    }
    assert!(died);
}

#[test]
fn regeneration_heals_one_every_fifty_ticks() {
    let mut region = survival(&[]);
    give(&mut region, Effect::REGENERATION, 0, 1000);
    own(&mut region).health = 10.0;
    for _ in 0..50 {
        idle(&mut region);
    }
    assert_eq!(health(&region), 11.0);
    for _ in 0..50 {
        idle(&mut region);
    }
    assert_eq!(health(&region), 12.0);
}

#[test]
fn hunger_adds_its_exhaustion_every_tick() {
    let mut region = survival(&[]);
    give(&mut region, Effect::HUNGER, 1, 1000);
    for _ in 0..10 {
        idle(&mut region);
    }
    let exhaustion = vitals(&region).food.exhaustion;
    // 0.005 · 2 a tick.
    assert!((exhaustion - 0.1).abs() < 1.0e-4, "{exhaustion}");
}

#[test]
fn instant_damage_given_as_an_effect_hurts_six() {
    let mut region = survival(&[]);
    give(&mut region, Effect::INSTANT_DAMAGE, 0, 1);
    let output = idle(&mut region);
    assert_eq!(damage_types(&output), vec![id_of("minecraft:magic")]);
    assert_eq!(health(&region), 14.0);
}

#[test]
fn absorptions_start_gives_four_a_level_and_its_end_takes_it_away() {
    let mut region = survival(&[]);
    give(&mut region, Effect::ABSORPTION, 1, 5);
    assert_eq!(
        region
            .players
            .get(&player(1))
            .expect("joined")
            .effects
            .absorption,
        8.0
    );
    for _ in 0..6 {
        idle(&mut region);
    }
    let own = region.players.get(&player(1)).expect("joined");
    assert!(!own.effects.has(Effect::ABSORPTION));
    assert_eq!(own.effects.absorption, 0.0);
}

#[test]
fn a_creative_player_takes_none_of_it() {
    let mut region = in_mode(
        clustine_world::GameMode::Creative,
        &[(FEET, "minecraft:lava"), (EYES, "minecraft:stone")],
    );
    give(&mut region, Effect::POISON, 5, 1000);
    for _ in 0..40 {
        let output = idle(&mut region);
        assert!(damage_types(&output).is_empty());
    }
    assert_eq!(health(&region), 20.0);
    // Their fire lasts a tick at most.
    assert!(vitals(&region).fire <= 1);

    let mut region = in_mode(
        clustine_world::GameMode::Creative,
        &[(FEET, "minecraft:water"), (EYES, "minecraft:water")],
    );
    idle(&mut region);
    assert_eq!(vitals(&region).air, 300);
}

#[test]
fn the_cooldown_keeps_a_second_source_of_the_tick_to_its_excess() {
    // Magma's 1 as the player stands on it, then lava's 4 in the blocks they are in:
    // 3 more.
    let mut region = survival(&[(FLOOR, "minecraft:magma_block"), (FEET, "minecraft:lava")]);
    own(&mut region).pose.on_ground = true;
    let output = idle(&mut region);
    assert_eq!(health(&region), 16.0);
    // Only the full hit shows its damage event.
    assert_eq!(damage_types(&output), vec![id_of("minecraft:hot_floor")]);
}
