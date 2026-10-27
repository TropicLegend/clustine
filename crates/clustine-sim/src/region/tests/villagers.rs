//! The villager and the wandering trader in a region, as a player meets them
//! (`docs/groundwork/n7k-plan.md`, sections 1.2 and 10, part K2), each number from the
//! game's rules (`n7k-brief.md` section 2 and the dumps): an egg makes each with the
//! draws of its spawn, a villager of the type of the biome at its feet; a click makes a
//! villager shake its head with `villager_no` at its voice, and a trader do nothing that
//! opens; a villager runs from a zombie and calms down once it is gone, runs when a
//! player hits it, floats, walks to bread and takes it, and at night walks about under a
//! roof; an egg on a villager makes a baby of a type drawn as the game draws it; a
//! trader drinks invisibility at night and milk by day.

use clustine_data::{blocks, entity_types, items, sounds, villagers};
use clustine_world::{Entity, EntityChange, EntityData, EntityId};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect;
use crate::mobs::brain::Activity;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::villager::{self, Data, VillagerState};
use crate::mobs::types::wandering_trader::{self, TraderState};
use crate::shown::DataShown;

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);
const BREAD_KEY: EntityId = EntityId(44);

/// The floor's biome: the registry's first, the badlands, a desert villager's.
const FLOOR_TYPE: u8 = villagers::types::DESERT as u8;

/// A mob of the type `kind` at `position`, standing, at its type's full health, with
/// the type state `state`, picking up loot where `picks_up` (a villager's constructor
/// sets it).
fn mob_at(
    key: EntityId,
    kind: i32,
    position: [f64; 3],
    state: clustine_world::TypeState,
    picks_up: bool,
) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.body.flags.picks_up_loot = picks_up;
    mob.state = state;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

fn villager_at(key: EntityId, position: [f64; 3], shared: &Animal, own: &VillagerState) -> Entity {
    mob_at(
        key,
        entity_types::VILLAGER,
        position,
        animal::write(shared, own),
        true,
    )
}

fn trader_at(key: EntityId, position: [f64; 3]) -> Entity {
    mob_at(
        key,
        entity_types::WANDERING_TRADER,
        position,
        animal::write(&Animal::default(), &TraderState::default()),
        false,
    )
}

fn baby() -> Animal {
    let mut shared = Animal::default();
    shared.set_age(animal::BABY_START_AGE, 0);
    shared
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the mob is here").data).unwrap()
}

fn own(region: &Region, key: EntityId) -> VillagerState {
    villager::state_of(&mob(region, key)).1
}

fn all_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

fn idle(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    crate::physics::distance_sqr(a, b).sqrt()
}

/// The activity beside the core that the region's mob `key` has active.
fn active(region: &Region, key: EntityId) -> Option<Activity> {
    let entity = record(region, key).expect("the mob is here");
    let mob = Mob::decode(&entity.data).unwrap();
    crate::mobs::brain::Brain::read(
        &villager::PROVIDER,
        &mob.mind.brain,
        Some(&Default::default()),
        entity.lived,
    )
    .state
    .active
}

/// Ticks with no inputs until `done` holds, at most `bound`: the outputs on the way.
fn until(
    region: &mut Region,
    bound: usize,
    mut done: impl FnMut(&Region) -> bool,
) -> Option<Vec<TickOutput>> {
    let mut outputs = Vec::new();
    for _ in 0..bound {
        outputs.push(idle(region));
        if done(region) {
            return Some(outputs);
        }
    }
    None
}

/// Player 1's use of the mob `key` holding `held`, between ticks: the mob's stream
/// aside as it was before, and what the use showed.
fn used_now(region: &mut Region, key: EntityId, held: Option<ItemStack>) -> (Random, TickOutput) {
    let stream = region.entity_stream(key).random;
    let (chunk, entity) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .unwrap();
    let player_key = region.players[&player(1)].entity_id;
    let from = region.players[&player(1)].pose.position;
    let mut output = TickOutput::default();
    region.mob_used_by(
        &entity,
        chunk,
        player_key,
        held,
        true,
        false,
        [from.x, from.y, from.z],
        &mut output,
    );
    (stream, output)
}

fn sounds_heard(output: &TickOutput) -> Vec<(i32, f32, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                volume,
                pitch,
                ..
            } => Some((*sound, *volume, *pitch)),
            _ => None,
        })
        .collect()
}

fn entity_events(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
            _ => None,
        })
        .collect()
}

fn swings(output: &TickOutput) -> usize {
    output
        .events
        .iter()
        .filter(|event| matches!(event, RegionEvent::ArmSwung { .. }))
        .count()
}

#[test]
fn an_egg_makes_a_villager_of_the_biome_at_its_feet_and_a_trader_each_drawing_the_tail() {
    for kind in [entity_types::VILLAGER, entity_types::WANDERING_TRADER] {
        let mut region = on_ticking_floor_with(&[]);
        let player_key = region.players[&player(1)].entity_id;
        let mut level = region.entity_stream(player_key).random;
        let mut output = TickOutput::default();
        let key = region
            .spawn_mob(
                kind as u16,
                player(1),
                BlockPos::new(3, -60, 3),
                true,
                true,
                &mut output,
            )
            .expect("made");
        let _yaw = level.next_float();
        // `AbstractVillager.finalizeSpawn`'s group of no babies draws nothing; then
        // `Mob`'s: the follow range's bonus and the hand.
        let bonus = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
        let left = level.next_float() < 0.05;
        let made = mob(&region, key);
        let follow = made
            .body
            .attributes
            .made(clustine_data::attributes::FOLLOW_RANGE as u16)
            .unwrap();
        assert_eq!(follow.modifiers[0].amount.to_bits(), bonus.to_bits());
        assert_eq!(made.body.flags.left_handed, left);
        assert!(!animal::is_baby(&made));
        if kind == entity_types::VILLAGER {
            let (_, own) = villager::state_of(&made);
            assert_eq!(
                own.data,
                Data {
                    kind: FLOOR_TYPE,
                    ..Data::default()
                }
            );
            assert!(own.finalized);
            assert!(made.body.flags.picks_up_loot);
            // Clients see its data and that it is settled: a desert villager,
            // unemployed, of level 1.
            let values = crate::shown::mob_values(kind as u16, &made.encode());
            assert!(values.contains(&(
                villager::DATA_INDEX,
                DataShown::VillagerData {
                    kind: villagers::types::DESERT,
                    profession: villagers::professions::NONE,
                    level: 1,
                },
                false
            )));
            assert!(values.contains(&(villager::FINALIZED_INDEX, DataShown::Boolean(true), false)));
            assert!(values.contains(&(villager::UNHAPPY_INDEX, DataShown::Int(0), true)));
        } else {
            let (_, own) = wandering_trader::state_of(&made);
            assert_eq!(own.despawn_delay, 0, "an egg's trader never leaves");
        }
    }
    // And each from its egg used on the floor.
    for (egg, kind) in [
        (items::VILLAGER_SPAWN_EGG, entity_types::VILLAGER),
        (
            items::WANDERING_TRADER_SPAWN_EGG,
            entity_types::WANDERING_TRADER,
        ),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        checked(
            &mut region,
            &moves(vec![
                set_slot(1, 0, Some(ItemStack::new(egg, 1))),
                select(1, 0),
            ]),
        );
        checked(&mut region, &moves(vec![place(1, 3, -61, 3, Face::Top)]));
        assert_eq!(all_of(&region, kind).len(), 1, "{kind}");
    }
}

#[test]
fn a_click_on_a_villager_makes_it_shake_its_head_and_say_no_and_a_baby_too() {
    for (shared, baby_pitch, result_swings) in [(Animal::default(), 1.0, 0), (baby(), 1.5, 1)] {
        let mut region = on_ticking_floor_with(&[]);
        region.clock.day = 6000;
        lay(
            &mut region,
            &villager_at(ONE, [4.5, -60.0, 4.5], &shared, &VillagerState::default()),
        );
        let (mut stream, output) =
            used_now(&mut region, ONE, Some(ItemStack::new(items::BREAD, 1)));
        assert_eq!(own(&region, ONE).unhappy, villager::UNHAPPY_TICKS);
        // `makeSound(VILLAGER_NO)`: its volume, and its voice of two draws.
        let pitch = (stream.next_float() - stream.next_float()) * 0.2 + baby_pitch;
        assert_eq!(
            sounds_heard(&output),
            [(sounds::ENTITY_VILLAGER_NO, 1.0, pitch)]
        );
        // An adult's use is consumed (no arm swings); a baby's succeeds. Nothing opens.
        assert_eq!(swings(&output), result_swings);
        assert!(
            output.player_events.is_empty(),
            "{:?}",
            output.player_events
        );
        // Clients see it unhappy, and it counts down to nothing.
        let values = crate::shown::mob_values(
            entity_types::VILLAGER as u16,
            &record(&region, ONE).unwrap().data,
        );
        assert!(values.contains(&(villager::UNHAPPY_INDEX, DataShown::Int(40), false)));
        let mut seen = Vec::new();
        for _ in 0..41 {
            idle(&mut region);
            seen.push(own(&region, ONE).unhappy);
        }
        assert_eq!(seen[..3], [39, 38, 37]);
        assert_eq!(seen[39..], [0, 0]);
    }
}

#[test]
fn a_click_on_a_wandering_trader_opens_nothing_and_changes_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &trader_at(ONE, [4.5, -60.0, 4.5]));
    let before = record(&region, ONE).unwrap();
    let (_, output) = used_now(&mut region, ONE, None);
    assert!(
        output.player_events.is_empty(),
        "{:?}",
        output.player_events
    );
    assert!(sounds_heard(&output).is_empty());
    assert_eq!(swings(&output), 0, "consumed");
    assert_eq!(record(&region, ONE).unwrap().data, before.data);
}

/// A pen of stone three blocks high round x, z = 2 to 13 in the home chunk, the one
/// whose entities the region ticks, so that a mob that runs or wanders stays in it; with
/// a roof of stone over it three blocks up where `roofed`.
fn pen(roofed: bool) -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut placed = Vec::new();
    for x in 1..=14 {
        for z in 1..=14 {
            let wall = x == 1 || x == 14 || z == 1 || z == 14;
            if wall {
                for y in -60..=-58 {
                    placed.push((BlockPos::new(x, y, z), blocks::STONE));
                }
            }
            if roofed {
                placed.push((BlockPos::new(x, -57, z), blocks::STONE));
            }
        }
    }
    placed
}

/// A zombie standing still where it is laid (its type's goals, if any, notwithstanding:
/// it has no AI).
fn still_zombie(key: EntityId, position: [f64; 3]) -> Entity {
    let mut zombie = mob_at(
        key,
        entity_types::ZOMBIE,
        position,
        Default::default(),
        false,
    );
    let mut mob = Mob::decode(&zombie.data).unwrap();
    mob.body.flags.no_ai = true;
    zombie.data = mob.encode();
    zombie
}

#[test]
fn a_villager_by_a_zombie_panics_and_runs_and_calms_down_once_it_is_gone() {
    let mut region = on_ticking_floor_with(&pen(false));
    region.clock.day = 6000;
    lay(
        &mut region,
        &villager_at(
            ONE,
            [7.5, -60.0, 7.5],
            &Animal::default(),
            &VillagerState::default(),
        ),
    );
    lay(&mut region, &still_zombie(TWO, [10.5, -60.0, 7.5]));
    until(&mut region, 40, |region| {
        active(region, ONE) == Some(Activity::Panic)
    })
    .expect("it panics within a scan of its hostiles' sensor");
    let zombie = record(&region, TWO).unwrap().position;
    until(&mut region, 200, |region| {
        distance(record(region, ONE).unwrap().position, zombie) > 6.0
    })
    .expect("it runs from the zombie");
    // The zombie gone, it calms down at its next calm-down after a sensor's scan and a
    // schedule's update.
    let (chunk, hops) = region
        .find_entity(TWO)
        .map(|(at, entity)| (at, entity.hops))
        .unwrap();
    let mut output = TickOutput::default();
    region.remove_entity(TWO, hops, chunk, &mut output);
    until(&mut region, 80, |region| {
        active(region, ONE) == Some(Activity::Idle)
    })
    .expect("it calms down");
}

#[test]
fn a_villager_hit_by_a_player_is_angry_and_runs_from_them() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 6000;
    lay(
        &mut region,
        &villager_at(
            ONE,
            [2.5, -60.0, 1.5],
            &Animal::default(),
            &VillagerState::default(),
        ),
    );
    let output = checked(
        &mut region,
        &moves(vec![numbered(1, PlayerInput::Attack { entity: ONE })]),
    );
    assert!(entity_events(&output, ONE).contains(&13), "angry particles");
    let from = region.players[&player(1)].pose.position;
    let from = [from.x, from.y, from.z];
    until(&mut region, 40, |region| {
        active(region, ONE) == Some(Activity::Panic)
    })
    .expect("it panics");
    until(&mut region, 200, |region| {
        distance(record(region, ONE).unwrap().position, from) > 6.0
    })
    .expect("it runs from the player");
}

/// A pool of water from x, z = 4 to 11 and y = -61 to -57 in the home chunk, stone
/// under it and round it.
fn pool() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut placed = Vec::new();
    for x in 3..=12 {
        for z in 3..=12 {
            placed.push((BlockPos::new(x, -62, z), blocks::STONE));
            for y in -61..=-57 {
                let wall = x == 3 || x == 12 || z == 3 || z == 12;
                let state = if wall { blocks::STONE } else { blocks::WATER };
                placed.push((BlockPos::new(x, y, z), state));
            }
        }
    }
    placed
}

#[test]
fn a_villager_in_deep_water_swims_up_to_its_top() {
    let mut region = on_ticking_floor_with(&pool());
    region.clock.day = 6000;
    lay(
        &mut region,
        &villager_at(
            ONE,
            [7.5, -61.0, 7.5],
            &Animal::default(),
            &VillagerState::default(),
        ),
    );
    until(&mut region, 100, |region| {
        record(region, ONE).unwrap().position[1] > -58.5
    })
    .expect("it rises to the water's top, where its head is out");
}

fn bread_at(position: [f64; 3]) -> Entity {
    Entity {
        key: BREAD_KEY,
        kind: entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(BREAD_KEY.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Item {
            stack: ItemStack::new(items::BREAD, 3),
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

#[test]
fn a_villager_walks_to_bread_on_the_ground_and_takes_it_into_its_slots() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 6000;
    lay(
        &mut region,
        &villager_at(
            ONE,
            [4.5, -60.0, 4.5],
            &Animal::default(),
            &VillagerState::default(),
        ),
    );
    lay(&mut region, &bread_at([7.5, -60.0, 4.5]));
    until(&mut region, 300, |region| {
        record(region, BREAD_KEY).is_none()
    })
    .expect("the bread is taken");
    let inventory = own(&region, ONE).inventory;
    assert_eq!(inventory[0], Some(ItemStack::new(items::BREAD, 3)));
    assert!(inventory[1..].iter().all(Option::is_none));
}

/// The walk target the region's villager `key` has now: its block and how close is
/// close enough, where it is a block's.
fn walk_target(region: &Region, key: EntityId) -> Option<(BlockPos, i32)> {
    let entity = record(region, key).expect("the mob is here");
    let mob = Mob::decode(&entity.data).unwrap();
    let memories = crate::mobs::brain::memories_of(&villager::PROVIDER, &mob, entity.lived);
    match memories.get(crate::mobs::brain::MemoryKind::WalkTarget)? {
        crate::mobs::brain::Value::Walk {
            to: crate::mobs::brain::Tracker::Block(pos),
            close_enough,
            ..
        } => Some((pos, close_enough)),
        _ => None,
    }
}

/// The night's walks of a villager laid in the pen, roofed or not, over 600 ticks:
/// each new walk target, with the villager's block as it was set.
fn night_walks(roofed: bool) -> Vec<(BlockPos, i32, BlockPos)> {
    let mut region = on_ticking_floor_with(&pen(roofed));
    region.clock.day = 18_000;
    lay(
        &mut region,
        &villager_at(
            ONE,
            [7.5, -60.0, 7.5],
            &Animal::default(),
            &VillagerState::default(),
        ),
    );
    let mut walks = Vec::new();
    let mut last = None;
    for _ in 0..600 {
        idle(&mut region);
        assert_eq!(active(&region, ONE), Some(Activity::Rest));
        let now = walk_target(&region, ONE);
        if let Some((to, close_enough)) = now
            && now != last
        {
            let at = crate::physics::containing(record(&region, ONE).unwrap().position);
            walks.push((to, close_enough, at));
        }
        last = now;
    }
    walks
}

#[test]
fn a_villager_at_night_rests_walking_about_under_a_roof_and_wandering_off_outside() {
    // Under a roof, `InsideBrownianWalk`'s steps to a block beside its own (close
    // enough at 0) among `GoToClosestVillage`'s walks to land within 15 (at 4).
    let walks = night_walks(true);
    let steps = walks
        .iter()
        .filter(|(to, close_enough, at)| {
            *close_enough == 0
                && (to.x - at.x).abs() <= 1
                && (to.y - at.y).abs() <= 1
                && (to.z - at.z).abs() <= 1
        })
        .count();
    assert!(steps > 0, "{walks:?}");
    // In the open, no step: the sky is seen, and only the walks to the nearest village
    // (there is none, so to any land within 15) take it off.
    let walks = night_walks(false);
    assert!(!walks.is_empty());
    assert!(
        walks.iter().all(|(_, close_enough, _)| *close_enough == 4),
        "{walks:?}"
    );
}

#[test]
fn an_egg_on_a_villager_makes_a_baby_of_a_type_drawn_from_its_own_stream() {
    for draw_at in 0..6 {
        let mut region = on_ticking_floor_with(&[]);
        region.clock.day = 6000;
        let parent = VillagerState {
            data: Data {
                kind: villagers::types::SNOW as u8,
                profession: villagers::professions::FARMER as u8,
                level: 3,
            },
            finalized: true,
            ..VillagerState::default()
        };
        // Some ticks first, so that each round draws at another place of the stream.
        lay(
            &mut region,
            &villager_at(ONE, [4.5, -60.0, 4.5], &Animal::default(), &parent),
        );
        for _ in 0..draw_at {
            idle(&mut region);
        }
        let mut stream = region.entity_stream(ONE).random;
        let entity = record(&region, ONE).unwrap();
        let player_key = region.players[&player(1)].entity_id;
        let mut output = TickOutput::default();
        assert!(region.offspring_from_egg(
            &entity,
            Some(ItemStack::new(items::VILLAGER_SPAWN_EGG, 1)),
            player_key,
            &mut output,
        ));
        let r = stream.next_double();
        let expected = if r < 0.5 {
            FLOOR_TYPE
        } else {
            villagers::types::SNOW as u8
        };
        let children: Vec<Entity> = all_of(&region, entity_types::VILLAGER)
            .into_iter()
            .filter(|villager| villager.key != ONE)
            .collect();
        let [child] = &children[..] else {
            panic!("one child");
        };
        let child = Mob::decode(&child.data).unwrap();
        let (shared, own) = villager::state_of(&child);
        assert!(shared.is_baby());
        assert_eq!(
            own.data,
            Data {
                kind: expected,
                ..Data::default()
            },
            "unemployed, of level 1, of the drawn type ({r})"
        );
        assert!(own.finalized);
    }
}

/// The trader's main hand.
fn trader_hand(region: &Region) -> Option<i32> {
    mob(region, ONE).body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)]
        .clone()
        .map(|stack| stack.item)
}

#[test]
fn a_wandering_trader_drinks_invisibility_at_night_and_milk_by_day() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 18_000;
    lay(&mut region, &trader_at(ONE, [6.5, -60.0, 6.5]));
    until(&mut region, 10, |region| {
        trader_hand(region) == Some(items::POTION)
    })
    .expect("it takes out its potion");
    let living = |region: &Region| {
        crate::shown::mob_values(
            entity_types::WANDERING_TRADER as u16,
            &record(region, ONE).unwrap().data,
        )
    };
    assert!(
        living(&region).contains(&(8, DataShown::Byte(1), false)),
        "drinking"
    );
    let outputs = until(&mut region, 40, |region| {
        mob(region, ONE).body.effects.has(Effect::INVISIBILITY)
    })
    .expect("it drinks it");
    let heard: Vec<i32> = outputs
        .iter()
        .flat_map(sounds_heard)
        .map(|(sound, _, _)| sound)
        .collect();
    // From the eighth tick on, every fourth, and once more as it is drunk.
    let drinking = heard
        .iter()
        .filter(|sound| **sound == sounds::ENTITY_WANDERING_TRADER_DRINK_POTION)
        .count();
    assert!((6..=7).contains(&drinking), "{heard:?}");
    let outputs =
        until(&mut region, 5, |region| trader_hand(region).is_none()).expect("its hand emptied");
    assert!(
        outputs
            .iter()
            .flat_map(sounds_heard)
            .any(|(sound, _, _)| { sound == sounds::ENTITY_WANDERING_TRADER_DISAPPEARED })
    );
    assert!(
        record(&region, ONE)
            .is_some_and(|entity| crate::shown::mob_shared_flags(&entity.data) & 0x20 != 0)
    );

    // By day, milk.
    region.clock.day = 1000;
    until(&mut region, 10, |region| {
        trader_hand(region) == Some(items::MILK_BUCKET)
    })
    .expect("it takes out milk");
    let outputs =
        until(&mut region, 60, |region| trader_hand(region).is_none()).expect("it drinks it");
    assert!(!mob(&region, ONE).body.effects.has(Effect::INVISIBILITY));
    assert!(
        outputs
            .iter()
            .flat_map(sounds_heard)
            .any(|(sound, _, _)| { sound == sounds::ENTITY_WANDERING_TRADER_REAPPEARED })
    );
}
