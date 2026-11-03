//! Tests of a player's falls, hunger and death in the region (V4a to V4c: `vibrations.rs`'s
//! moves, `player_damage.rs`'s landings, `hunger.rs`, `death.rs`).
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, made a survival player whose client has
//! loaded.

use clustine_data::items;
use clustine_world::{EntityData, Vec3};

use super::redstone::{state, ticking_with};
use super::*;
use crate::state::{CombatEntry, FoodData};

const FLOOR: BlockPos = BlockPos::new(0, -61, 0);

/// A region whose floor under player 1 is `floor`, the player in `mode`, loaded.
fn standing_on(mode: clustine_world::GameMode, floor: &str) -> Region {
    let mut region = ticking_with(&[(FLOOR, state(floor))]);
    let own = region.players.get_mut(&player(1)).expect("joined");
    own.mode = clustine_world::PlayerMode::new(mode);
    own.inventory = Inventory::default();
    own.hurting.loading.mark_loaded();
    region
}

fn survival_on(floor: &str) -> Region {
    standing_on(clustine_world::GameMode::Survival, floor)
}

fn health(region: &Region) -> f32 {
    region.players.get(&player(1)).expect("joined").health
}

fn move_to(number: u64, y: f64, on_ground: bool) -> TickInputs {
    moves(vec![with_number(
        number,
        numbered(
            1,
            PlayerInput::Move {
                position: Some(Vec3::new(0.5, y, 0.5)),
                rotation: None,
                on_ground,
            },
        ),
    )])
}

/// Player 1 lifted to `height` above the floor and let fall onto it in two moves, the
/// three numbered on from the player's last input.
fn fall(region: &mut Region, height: f64) -> TickOutput {
    let last = region.players[&player(1)].last_input;
    region.tick(&move_to(last + 1, -60.0 + height, false));
    region.tick(&move_to(last + 2, -60.0 + height / 2.0, false));
    region.tick(&move_to(last + 3, -60.0, true))
}

/// `LivingEntity.calculateFallDamage`: `floor(5 + 1e-6 − 3)` is 2 from five blocks.
#[test]
fn a_fall_of_five_blocks_onto_stone_hurts_two() {
    let mut region = survival_on("minecraft:stone");
    // The rise to the top resets the fall (`Player.aiStep`), and the fall counts from there.
    fall(&mut region, 5.0);
    assert_eq!(health(&region), 18.0);
    let fall_distance = region.players[&player(1)].vitals.fall_distance;
    assert_eq!(fall_distance, 0.0, "a landing ends the fall");
}

/// `HayBlock.fallOn`: a fifth of the hurt, so five blocks hurt `floor(2 · 0.2)`, nothing.
#[test]
fn a_fall_onto_hay_hurts_a_fifth() {
    let mut region = survival_on("minecraft:hay_block[axis=y]");
    fall(&mut region, 5.0);
    assert_eq!(health(&region), 20.0);
    fall(&mut region, 13.0);
    // floor((13 − 3) · 0.2) = 2.
    assert_eq!(health(&region), 18.0);
}

/// A bed halves the fall (`fallDistanceReduction` 0.5): eleven blocks hurt as 5.5 do.
#[test]
fn a_fall_onto_a_bed_hurts_as_half_the_height() {
    let mut region = survival_on("minecraft:red_bed[facing=north,occupied=false,part=foot]");
    fall(&mut region, 11.0);
    // floor(5.5 + 1e-6 − 3) = 2.
    assert_eq!(health(&region), 18.0);
}

/// `Player.causeFallDamage`: nothing for one who may fly.
#[test]
fn a_creative_players_fall_hurts_nothing() {
    let mut region = standing_on(clustine_world::GameMode::Creative, "minecraft:stone");
    fall(&mut region, 20.0);
    assert_eq!(health(&region), 20.0);
}

/// `FoodData.tick`'s fast branch: a hurt player at food 20 with saturation heals
/// `min(saturation, 6) / 6` ten ticks on, and is told so.
#[test]
fn a_hurt_player_with_full_food_heals_and_is_told() {
    let mut region = survival_on("minecraft:stone");
    region.players.get_mut(&player(1)).expect("in").health = 10.0;
    let mut told = Vec::new();
    for _ in 0..12 {
        let output = region.tick(&TickInputs::default());
        told.extend(
            output
                .player_events
                .iter()
                .filter_map(|(id, event)| match event {
                    PlayerEvent::Health { health, .. } if *id == player(1) => Some(*health),
                    _ => None,
                }),
        );
    }
    assert!(health(&region) > 10.0, "{}", health(&region));
    assert_eq!(told.last().copied(), Some(health(&region)));
}

/// `FoodData.tick`'s starving branch: at food 0 on easy, `starve` 1 every 80 ticks down
/// to 10.
#[test]
fn a_starving_player_on_easy_loses_health_down_to_ten() {
    let mut region = survival_on("minecraft:stone");
    region.players.get_mut(&player(1)).expect("in").vitals.food = FoodData {
        food: 0,
        saturation: 0.0,
        exhaustion: 0.0,
        timer_from: None,
    };
    for _ in 0..81 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(health(&region), 19.0);
    region.players.get_mut(&player(1)).expect("in").health = 10.0;
    for _ in 0..200 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(health(&region), 10.0);
}

/// Eating feeds (`FoodData.eat`): bread from 10 food gives 15 and its saturation.
#[test]
fn a_player_who_eats_bread_is_fed() {
    let mut region = survival_on("minecraft:stone");
    region.players.get_mut(&player(1)).expect("in").vitals.food = FoodData {
        food: 10,
        saturation: 0.0,
        exhaustion: 0.0,
        timer_from: None,
    };
    region.feed_player(player(1), 5, 6.0);
    let food = region.players[&player(1)].vitals.food;
    assert_eq!((food.food, food.saturation), (15, 6.0));
    assert!(region.needs_food(player(1)));
}

/// `dropAllDeathLoot`: a death drops what the player carried around them, with no
/// thrower, and their experience as orbs; the message is the void's.
#[test]
fn a_death_drops_what_the_player_carried_and_their_experience() {
    let mut region = survival_on("minecraft:stone");
    let own = region.players.get_mut(&player(1)).expect("in");
    own.inventory.set(0, Some(ItemStack::new(items::STONE, 5)));
    own.inventory.set(9, Some(ItemStack::new(items::DIRT, 3)));
    own.experience.level = 3;
    own.health = 1.0;
    own.pose.position = Vec3::new(0.5, -60.0, 0.5);
    let mut output = TickOutput::default();
    let source =
        crate::mobs::body::hurt::DamageSource::of(crate::mobs::body::hurt::DamageType::IN_WALL);
    region.hurt_player(player(1), &source, 5.0, Default::default(), &mut output);
    let state = region.player_state(player(1)).expect("their body stays");
    assert!(state.dead);
    assert!(state.inventory.0.iter().all(Option::is_none));
    let records = super::super::tnt::tests::records(&region);
    let dropped: Vec<(i32, Option<uuid::Uuid>)> = records
        .iter()
        .filter_map(|entity| match &entity.data {
            EntityData::Item { stack, thrower, .. } => Some((stack.item, *thrower)),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, [(items::STONE, None), (items::DIRT, None)]);
    let orbs = records
        .iter()
        .filter(|entity| matches!(entity.data, EntityData::ExperienceOrb { .. }))
        .count();
    assert!(orbs > 0, "the experience lies as orbs");
    let died = output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Died { message, .. } => Some(message.key.clone()),
        _ => None,
    });
    assert_eq!(died.as_deref(), Some("death.attack.inWall"));
}

/// `CombatTracker.getDeathMessage`: the type's message naming its causer, and
/// `death.fell.accident.generic` for a fall.
#[test]
fn a_death_message_names_its_killer_or_says_the_fall() {
    let mut region = survival_on("minecraft:stone");
    let zombie = crate::api::Killer {
        entity: EntityId(9),
        kind: u16::try_from(clustine_data::entity_types::ZOMBIE).expect("fits"),
        name: None,
        player: false,
        weapon: None,
    };
    let entry = |damage_type: &str, killer: Option<crate::api::Killer>| CombatEntry {
        damage_type: damage_type.to_owned(),
        amount: 4.0,
        killer,
        fall_location: None,
        fall_distance: 0.0,
        at: 0,
    };
    let own = region.players.get_mut(&player(1)).expect("in");
    own.vitals.combat = vec![entry("minecraft:mob_attack", Some(zombie.clone()))];
    let message = region.death_message(player(1));
    assert_eq!(message.key, "death.attack.mob");
    assert_eq!(message.killer, Some(zombie));
    let own = region.players.get_mut(&player(1)).expect("in");
    own.vitals.combat = vec![entry("minecraft:fall", None)];
    assert_eq!(
        region.death_message(player(1)).key,
        "death.fell.accident.generic"
    );
}
