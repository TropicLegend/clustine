//! The water creatures in a region, from the brief's sections 6.5 to 6.10: eggs make
//! them with their defaults and their variants' draws, fish swim and keep to water and
//! out of it flop and drown, buckets take and pour fish, schools gather, a pufferfish
//! puffs and stings, squids stroke, fall and squirt, a glow squid goes dark, a dolphin
//! breathes, dries, plays and gives up its fish, turtles breed, lay and hatch, and that a
//! tick of them is a function of its inputs.

use clustine_data::{BlockState, entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityData, EntityId};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::fish::{self, Species};
use crate::mobs::types::{dolphin, squid, tropical_fish, turtle};
use crate::region::Random;

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);
const THREE: EntityId = EntityId(44);

/// A mob of the type `kind` at `position` as `mob` has it, of its type's full health and
/// air.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    // What its constructor sets and draws (a squid's strokes, a fish's flock, a dolphin's
    // pick-up), where the mob given has none of it.
    if mob.state == clustine_world::TypeState::default() && mob.mind.slots.is_empty() {
        crate::mobs::types::of(kind).construct(&mut mob, &mut Random::of_entity(0, key, 0));
    }
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    if mob.body.vitals.air == 0 {
        mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    }
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
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

fn take_away(region: &mut Region, key: EntityId) {
    let chunk = region.entity_chunks.remove(&key).unwrap();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Gone(key));
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the mob is here").data).unwrap()
}

fn fish_of(region: &Region, key: EntityId) -> fish::FishState {
    let entity = record(region, key).unwrap();
    fish::read(entity.kind, &mob(region, key).state)
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

fn holding(region: &mut Region, stack: ItemStack) {
    checked(
        region,
        &moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)]),
    );
}

fn one(item: i32) -> ItemStack {
    ItemStack::new(item, 1)
}

fn use_on(region: &mut Region, key: EntityId) -> TickOutput {
    checked(
        region,
        &moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: key,
                location: [0.0, 0.2, 0.0],
                sneaking: false,
            },
        )]),
    )
}

fn sounds_of(output: &TickOutput) -> Vec<(i32, Option<PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, except, .. } => Some((*sound, *except)),
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

fn held_count(region: &Region, item: i32) -> usize {
    region.players[&player(1)]
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .count()
}

/// A pool of water from x, z = 4 to 11 and y = -61 to -57 in the home chunk, stone
/// under it and round it.
fn pool() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in 3..=12 {
        for z in 3..=12 {
            blocks.push((BlockPos::new(x, -62, z), clustine_data::blocks::STONE));
            for y in -61..=-57 {
                let wall = x == 3 || x == 12 || z == 3 || z == 12;
                let state = if wall {
                    clustine_data::blocks::STONE
                } else {
                    clustine_data::blocks::WATER
                };
                blocks.push((BlockPos::new(x, y, z), state));
            }
        }
    }
    blocks
}

fn in_water(region: &Region, entity: &Entity) -> bool {
    let at = crate::physics::containing(entity.position);
    region
        .block(at)
        .is_some_and(clustine_blocks::data::has_water)
}

fn idle(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked(region, &TickInputs::default());
    }
}

#[test]
fn an_egg_makes_each_water_creature_with_its_defaults_and_its_variant_s_draws() {
    for kind in [
        entity_types::COD,
        entity_types::SALMON,
        entity_types::PUFFERFISH,
        entity_types::TROPICAL_FISH,
        entity_types::SQUID,
        entity_types::GLOW_SQUID,
        entity_types::DOLPHIN,
        entity_types::TURTLE,
    ] {
        let mut region = on_ticking_floor_with(&pool());
        let player_key = region.players[&player(1)].entity_id;
        let mut level = region.entity_stream(player_key).random;
        let mut output = TickOutput::default();
        let pos = BlockPos::new(6, -60, 6);
        let key = region
            .spawn_mob(kind as u16, player(1), pos, true, true, &mut output)
            .expect("made");
        let made = mob(&region, key);
        let entity = record(&region, key).unwrap();
        // Its own stream: its UUID and its constructor's yaw, then its type's.
        let mut own = Random::of_entity_aside(region.config.seed, key, 0, region.tick);
        own.next_u64();
        own.next_u64();
        own.next_float();
        // The level's on its behalf: its yaw, then `Mob`'s follow range and hand.
        let _yaw = level.next_float();
        let health = supplied(
            kind as u16,
            attribute(clustine_data::attributes::MAX_HEALTH),
        )
        .unwrap() as f32;
        assert_eq!(made.body.vitals.health, health, "{kind}");
        match kind {
            entity_types::COD | entity_types::SALMON | entity_types::TROPICAL_FISH => {
                // `FollowFlockLeaderGoal`'s first wait.
                own.next_int(200);
                let state = fish_of(&region, key);
                assert!(!state.from_bucket && state.school_size == 1 && state.leader.is_none());
                if kind == entity_types::SALMON {
                    assert_eq!(
                        state.variant,
                        crate::mobs::types::salmon::random_size(&mut own)
                    );
                }
                if kind == entity_types::TROPICAL_FISH {
                    level.next_double();
                    level.next_double();
                    level.next_float();
                    assert_eq!(state.variant, tropical_fish::random_variant(&mut level));
                }
                assert_eq!(made.body.vitals.air, 300);
            }
            entity_types::PUFFERFISH => {
                let state = fish_of(&region, key);
                assert_eq!(state.puff, 0);
                assert_eq!(state, fish::FishState::fresh(Species::Pufferfish));
            }
            entity_types::SQUID | entity_types::GLOW_SQUID => {
                let (shared, state) = squid::read(&made);
                assert!(!shared.is_baby());
                let speed = 1.0 / (own.next_float() + 1.0) * 0.2;
                assert_eq!(state.tentacle_speed, speed);
                assert_eq!(state.dark_ticks, 0);
            }
            entity_types::DOLPHIN => {
                assert_eq!(made.body.vitals.air, 4800);
                assert!(made.body.flags.picks_up_loot);
                let state = dolphin::read(&made);
                assert!(!state.got_fish && state.moistness == 2400);
            }
            _ => {
                // A turtle's home is where it is made.
                let block = crate::physics::containing(entity.position);
                assert_eq!(turtle::read(&made).home, block);
                assert!(!turtle::read(&made).has_egg);
            }
        }
    }
}

#[test]
fn a_fish_swims_about_in_water_and_keeps_to_it() {
    let mut region = on_ticking_floor_with(&pool());
    let start = [7.5, -59.0, 7.5];
    lay(
        &mut region,
        &mob_at(ONE, entity_types::COD, start, Mob::fresh()),
    );
    let mut moved = false;
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        let cod = record(&region, ONE).unwrap();
        assert!(in_water(&region, &cod), "{:?}", cod.position);
        moved |= crate::physics::distance_sqr(cod.position, start) > 4.0;
    }
    assert!(moved, "it swam about");
}

#[test]
fn a_fish_out_of_water_flops_and_runs_out_of_air_and_drowns_in_two_hits() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::COD, [8.5, -60.0, 8.5], Mob::fresh()),
    );
    let mut flopped = false;
    let mut first_hit = None;
    let mut died = None;
    for tick in 1..=360 {
        let output = checked(&mut region, &TickInputs::default());
        let Some(cod) = record(&region, ONE) else {
            break;
        };
        let body = mob(&region, ONE);
        if tick < 320 {
            assert_eq!(body.body.vitals.air, 300 - tick, "{tick}");
        }
        // A hop of 0.4 off the ground.
        flopped |= cod.position[1] > -59.8;
        // No bubbles: the water animal's own air rule sends no event 67.
        assert!(!entity_events(&output, ONE).contains(&67));
        if body.body.vitals.health < 3.0 && first_hit.is_none() {
            first_hit = Some(tick);
        }
        if body.body.vitals.dying() && died.is_none() {
            died = Some(tick);
        }
    }
    assert!(flopped, "it flopped about");
    assert_eq!(first_hit, Some(320));
    assert_eq!(died, Some(340));
}

/// A strip of water one block deep from x = 1 to 4 by the player, on stone.
fn strip() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in 1..=4 {
        for z in 0..=1 {
            blocks.push((BlockPos::new(x, -62, z), clustine_data::blocks::STONE));
            blocks.push((BlockPos::new(x, -61, z), clustine_data::blocks::WATER));
        }
    }
    blocks
}

/// The yaw and pitch from the player's eyes at the spawn to `at`.
fn aim(at: [f64; 3]) -> (f32, f32) {
    let eye = [0.5, -60.0 + 1.62, 0.5];
    let (dx, dy, dz) = (at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]);
    let yaw = (-dx).atan2(dz).to_degrees() as f32;
    let pitch = (-dy).atan2((dx * dx + dz * dz).sqrt()).to_degrees() as f32;
    (yaw, pitch)
}

#[test]
fn a_water_bucket_takes_a_fish_and_its_bucket_pours_it_back_as_from_a_bucket() {
    let mut region = on_ticking_floor_with(&strip());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::COD, [2.5, -61.0, 0.5], Mob::fresh()),
    );
    holding(&mut region, one(items::WATER_BUCKET));
    let output = use_on(&mut region, ONE);
    assert!(record(&region, ONE).is_none(), "the fish is taken");
    assert!(sounds_of(&output).contains(&(sounds::ITEM_BUCKET_FILL_FISH, None)));
    // The creative player keeps the water bucket and is given the fish's.
    assert_eq!(held_count(&region, items::WATER_BUCKET), 1);
    assert_eq!(held_count(&region, items::COD_BUCKET), 1);
    // A second fish taken gives a second bucket.
    lay(
        &mut region,
        &mob_at(TWO, entity_types::COD, [2.5, -61.0, 0.5], Mob::fresh()),
    );
    use_on(&mut region, TWO);
    assert_eq!(held_count(&region, items::COD_BUCKET), 2);

    // Poured where the player aims, into the water already there.
    holding(&mut region, one(items::COD_BUCKET));
    let (yaw, pitch) = aim([2.7, -61.95, 0.5]);
    let output = checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::UseItem {
                rotation: (yaw, pitch),
                sneaking: false,
                sequence: 1,
            },
        )]),
    );
    let cods = all_of(&region, entity_types::COD);
    let [poured] = &cods[..] else {
        panic!("one cod poured: {cods:?}");
    };
    assert_eq!(
        crate::physics::containing(poured.position),
        BlockPos::new(2, -61, 0)
    );
    assert!(fish::read(poured.kind, &Mob::decode(&poured.data).unwrap().state).from_bucket);
    assert!(sounds_of(&output).contains(&(sounds::ITEM_BUCKET_EMPTY_FISH, Some(player(1)))));
    assert_eq!(
        region.block(BlockPos::new(2, -61, 0)),
        Some(clustine_data::blocks::WATER)
    );
    // The creative player keeps the bucket: the one in hand and the two given.
    assert_eq!(held_count(&region, items::COD_BUCKET), 3);
}

#[test]
fn fish_alone_gather_into_a_school_that_follows_its_leader() {
    let mut region = on_ticking_floor_with(&pool());
    for (key, x) in [(ONE, 6.5), (TWO, 7.5), (THREE, 8.5)] {
        lay(
            &mut region,
            &mob_at(key, entity_types::COD, [x, -59.0, 7.5], Mob::fresh()),
        );
    }
    // Each looks for a school some 100 to 110 of its goal ticks after it is made.
    idle(&mut region, 240);
    let states: Vec<_> = [ONE, TWO, THREE]
        .into_iter()
        .map(|key| (key, fish_of(&region, key)))
        .collect();
    let leaders: Vec<_> = states
        .iter()
        .filter(|(_, state)| state.school_size > 1)
        .collect();
    let [(leader, state)] = &leaders[..] else {
        panic!("one leader: {states:?}");
    };
    assert_eq!(state.school_size, 3);
    for (key, state) in &states {
        if key != leader {
            assert_eq!(state.leader, Some(*leader));
        }
    }
}

/// A one-block hole of water at (5, -61, 5) and (6, -61, 5), with stone under it.
fn hole() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in [5, 6] {
        blocks.push((BlockPos::new(x, -62, 5), clustine_data::blocks::STONE));
        blocks.push((BlockPos::new(x, -61, 5), clustine_data::blocks::WATER));
    }
    blocks
}

fn still_cow(key: EntityId, at: [f64; 3]) -> Entity {
    let mut cow = Mob::fresh();
    cow.body.flags.no_ai = true;
    mob_at(key, entity_types::COW, at, cow)
}

#[test]
fn a_pufferfish_puffs_up_by_a_mob_near_and_down_again_once_it_is_gone() {
    let mut region = on_ticking_floor_with(&hole());
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::PUFFERFISH,
            [5.5, -61.0, 5.5],
            Mob::fresh(),
        ),
    );
    lay(&mut region, &still_cow(TWO, [6.7, -61.0, 5.5]));
    let puff = |region: &Region| fish_of(region, ONE).puff;
    let mut half = None;
    let mut whole = None;
    for tick in 1..=80 {
        let output = checked(&mut region, &TickInputs::default());
        if puff(&region) == 1 && half.is_none() {
            half = Some(tick);
            assert!(sounds_of(&output).contains(&(sounds::ENTITY_PUFFER_FISH_BLOW_UP, None)));
        }
        if puff(&region) == 2 && whole.is_none() {
            whole = Some(tick);
        }
    }
    let (half, whole) = (half.expect("puffed"), whole.expect("puffed whole"));
    assert!(half <= 3, "{half}");
    assert_eq!(whole - half, 40);
    // Its box grew with it.
    let fish = record(&region, ONE).unwrap();
    let bounds = region.entity_box(&fish);
    assert!((bounds.max[0] - bounds.min[0] - 0.7).abs() < 1e-6);

    take_away(&mut region, TWO);
    let mut flat = None;
    let mut deflated = None;
    for tick in 1..=140 {
        let output = checked(&mut region, &TickInputs::default());
        if puff(&region) == 1 && deflated.is_none() {
            deflated = Some(tick);
            assert!(sounds_of(&output).contains(&(sounds::ENTITY_PUFFER_FISH_BLOW_OUT, None)));
        }
        if puff(&region) == 0 && flat.is_none() {
            flat = Some(tick);
        }
    }
    let (deflated, flat) = (deflated.expect("deflated"), flat.expect("flat"));
    assert!((61..=64).contains(&deflated), "{deflated}");
    assert_eq!(flat - deflated, 40);
}

#[test]
fn a_puffed_pufferfish_stings_a_mob_it_touches() {
    let mut region = on_ticking_floor_with(&hole());
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::PUFFERFISH,
            [5.5, -61.0, 5.5],
            Mob::fresh(),
        ),
    );
    lay(&mut region, &still_cow(TWO, [6.4, -61.0, 5.5]));
    let mut stung = false;
    for _ in 0..20 {
        let output = checked(&mut region, &TickInputs::default());
        if sounds_of(&output).contains(&(sounds::ENTITY_PUFFER_FISH_STING, None)) {
            stung = true;
            break;
        }
    }
    assert!(stung);
    // One more than its half puff.
    assert_eq!(mob(&region, TWO).body.vitals.health, 10.0 - 2.0);
}

#[test]
fn a_squid_strokes_in_water_showing_each_stroke_and_falls_straight_down_on_land() {
    let mut region = on_ticking_floor_with(&pool());
    let start = [7.5, -59.5, 7.5];
    lay(
        &mut region,
        &mob_at(ONE, entity_types::SQUID, start, Mob::fresh()),
    );
    let mut strokes = 0;
    for _ in 0..140 {
        let output = checked(&mut region, &TickInputs::default());
        strokes += entity_events(&output, ONE)
            .iter()
            .filter(|event| **event == 19)
            .count();
    }
    // A stroke lasts 32 to 63 ticks.
    assert!((2..=5).contains(&strokes), "{strokes}");
    let squid = record(&region, ONE).unwrap();
    assert!(crate::physics::distance_sqr(squid.position, start) > 0.01);

    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::SQUID, [8.5, -57.0, 8.5], Mob::fresh()),
    );
    for _ in 0..30 {
        checked(&mut region, &TickInputs::default());
        let squid = record(&region, ONE).unwrap();
        assert_eq!((squid.position[0], squid.position[2]), (8.5, 8.5));
        assert_eq!((squid.motion[0], squid.motion[2]), (0.0, 0.0));
    }
    assert_eq!(record(&region, ONE).unwrap().position[1], -60.0);
}

#[test]
fn a_hurt_squid_squirts_its_ink_and_a_glow_squid_goes_dark_for_100_ticks() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::GLOW_SQUID,
            [2.5, -60.0, 0.5],
            Mob::fresh(),
        ),
    );
    let output = checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Attack {
                part: None,
                entity: ONE,
            },
        )]),
    );
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_GLOW_SQUID_SQUIRT, None)));
    // Dark for 100 from the hit, one less already at the end of the tick it came in.
    let dark = |region: &Region| squid::read(&mob(region, ONE)).1.dark_ticks;
    assert_eq!(dark(&region), 99);
    checked(&mut region, &TickInputs::default());
    assert_eq!(dark(&region), 98);
    let shown = crate::shown::mob_values(
        entity_types::GLOW_SQUID as u16,
        &record(&region, ONE).unwrap().data,
    );
    assert!(shown.contains(&(18, crate::shown::DataShown::Int(98), false)));
    idle(&mut region, 120);
    assert_eq!(dark(&region), 0);

    // A squid hurt by nothing living squirts nothing.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::SQUID, [8.5, -55.0, 8.5], Mob::fresh()),
    );
    for _ in 0..40 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(!sounds_of(&output).contains(&(sounds::ENTITY_SQUID_SQUIRT, None)));
    }
}

#[test]
fn a_dolphin_uses_its_4800_ticks_of_air_under_water_and_refills_them_at_once_out_of_it() {
    let mut region = on_ticking_floor_with(&pool());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DOLPHIN, [7.5, -60.0, 7.5], Mob::fresh()),
    );
    idle(&mut region, 5);
    assert_eq!(mob(&region, ONE).body.vitals.air, 4795);

    let mut region = on_ticking_floor_with(&[]);
    let mut low = Mob::fresh();
    low.body.vitals.air = 100;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DOLPHIN, [8.5, -60.0, 8.5], low),
    );
    checked(&mut region, &TickInputs::default());
    assert_eq!(mob(&region, ONE).body.vitals.air, 4800);
}

#[test]
fn a_dolphin_on_land_dries_a_tick_at_a_time_flops_and_is_hurt_once_dry() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DOLPHIN, [8.5, -60.0, 8.5], Mob::fresh()),
    );
    let mut flopped = false;
    for _ in 0..10 {
        checked(&mut region, &TickInputs::default());
        flopped |= record(&region, ONE).unwrap().motion[1] > 0.3;
    }
    assert_eq!(dolphin::read(&mob(&region, ONE)).moistness, 2390);
    assert!(flopped);

    let mut region = on_ticking_floor_with(&[]);
    let mut dry = Mob::fresh();
    dolphin::update(&mut dry, |own| own.moistness = 1);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DOLPHIN, [8.5, -60.0, 8.5], dry),
    );
    let output = checked(&mut region, &TickInputs::default());
    let dry_out = crate::mobs::body::hurt::DamageType::DRY_OUT.id().unwrap();
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::DamageEvent { entity, damage_type, .. } if *entity == ONE && *damage_type == dry_out
    )));
    assert!(mob(&region, ONE).body.vitals.health < 10.0);
}

#[test]
fn a_dolphin_fed_a_fish_finds_no_treasure_and_gives_it_up_at_once() {
    let mut region = on_ticking_floor_with(&strip());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DOLPHIN, [2.5, -61.0, 0.5], Mob::fresh()),
    );
    holding(&mut region, one(items::COD));
    let output = use_on(&mut region, ONE);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_DOLPHIN_EAT, None)));
    assert!(dolphin::read(&mob(&region, ONE)).got_fish);
    idle(&mut region, 4);
    assert!(!dolphin::read(&mob(&region, ONE)).got_fish);
}

fn item_at(key: EntityId, stack: ItemStack, position: [f64; 3]) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Item {
            stack,
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

#[test]
fn a_dolphin_takes_an_item_floating_by_and_throws_it_again() {
    let mut region = on_ticking_floor_with(&pool());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DOLPHIN, [7.5, -60.0, 7.5], Mob::fresh()),
    );
    lay(
        &mut region,
        &item_at(TWO, one(items::STICK), [7.5, -60.0, 7.5]),
    );
    let output = checked(&mut region, &TickInputs::default());
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::ItemTaken { item, collector, .. } if *item == TWO && *collector == ONE
    )));
    assert!(record(&region, TWO).is_none());
    let hand = usize::from(crate::living::slots::MAIN_HAND);
    assert_eq!(
        mob(&region, ONE).body.equipment.slots[hand],
        Some(one(items::STICK))
    );
    idle(&mut region, 3);
    assert_eq!(mob(&region, ONE).body.equipment.slots[hand], None);
    let thrown = all_of(&region, entity_types::ITEM);
    let [thrown] = &thrown[..] else {
        panic!("one thrown: {thrown:?}");
    };
    let EntityData::Item {
        ref stack,
        pick_up_delay,
        ..
    } = thrown.data
    else {
        unreachable!();
    };
    assert_eq!(*stack, one(items::STICK));
    assert!(pick_up_delay > 30, "{pick_up_delay}");
}

/// A beach of sand from x, z = 4 to 8 in the floor.
fn beach() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in 4..=8 {
        for z in 4..=8 {
            blocks.push((BlockPos::new(x, -61, z), clustine_data::blocks::SAND));
        }
    }
    blocks
}

fn turtle_with(change: impl FnOnce(&mut turtle::TurtleState)) -> Mob {
    let mut mob = Mob::fresh();
    turtle::update(&mut mob, change);
    mob
}

#[test]
fn a_turtle_with_an_egg_digs_into_the_sand_of_its_home_and_lays_a_clutch() {
    let mut region = on_ticking_floor_with(&beach());
    let home = BlockPos::new(6, -60, 6);
    let carrying = turtle_with(|own| {
        own.home = home;
        own.has_egg = true;
    });
    lay(
        &mut region,
        &mob_at(ONE, entity_types::TURTLE, [6.5, -60.0, 6.5], carrying),
    );
    let mut dug = 0;
    let mut laying_shown = false;
    let mut laid = None;
    // Its count goes on only while it stands at its sand, from which its going home,
    // restarted every other tick while it has an egg, leads it about.
    for tick in 1..=1500 {
        let output = checked(&mut region, &TickInputs::default());
        dug += output
            .events
            .iter()
            .filter(|event| matches!(event, RegionEvent::LevelEvent { kind: 2001, .. }))
            .count();
        let own = turtle::read(&mob(&region, ONE));
        laying_shown |= own.laying;
        if !own.has_egg {
            laid = Some((tick, output));
            break;
        }
    }
    let (tick, output) = laid.expect("laid");
    assert!(laying_shown);
    assert!(dug >= 30, "{dug}");
    assert!(tick > 200, "{tick}");
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_TURTLE_LAY_EGG, None)));
    let egg = region.block(home).unwrap();
    assert!(clustine_blocks::data::is_block(
        egg,
        clustine_data::blocks::TURTLE_EGG
    ));
    let eggs = clustine_blocks::data::number(egg, "eggs").unwrap();
    assert!((1..=4).contains(&eggs));
    // In love again.
    let lived = record(&region, ONE).unwrap().lived;
    assert!(animal::animal_of(&mob(&region, ONE).state).in_love(lived));
}

#[test]
fn a_turtle_egg_cracks_twice_and_hatches_a_baby_an_egg_at_its_nest_before_dawn() {
    let nest = BlockPos::new(5, -60, 5);
    let clutch = clustine_blocks::data::set(clustine_data::blocks::TURTLE_EGG, "eggs", "2");
    let mut blocks = beach();
    blocks.push((nest, clutch));
    let mut region = on_ticking_floor_with(&blocks);
    region.clock.day = 21_500;
    for stage in 1..=2 {
        let mut random = Random::of_entity(1, EntityId(stage), 0);
        let mut output = TickOutput::default();
        let state = region.block(nest).unwrap();
        region.turtle_egg_tick(nest, state, &mut random, &mut output);
        let now = region.block(nest).unwrap();
        assert_eq!(
            clustine_blocks::data::number(now, "hatch"),
            Some(stage as u8)
        );
        assert!(sounds_of(&output).contains(&(sounds::ENTITY_TURTLE_EGG_CRACK, None)));
    }
    let mut random = Random::of_entity(1, EntityId(3), 0);
    let mut output = TickOutput::default();
    let state = region.block(nest).unwrap();
    region.turtle_egg_tick(nest, state, &mut random, &mut output);
    assert_eq!(region.block(nest), Some(clustine_data::blocks::AIR));
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_TURTLE_EGG_HATCH, None)));
    let mut babies = all_of(&region, entity_types::TURTLE);
    babies.sort_by(|a, b| a.position[0].total_cmp(&b.position[0]));
    assert_eq!(babies.len(), 2);
    for (i, baby) in babies.iter().enumerate() {
        let made = Mob::decode(&baby.data).unwrap();
        assert!(animal::animal_of(&made.state).is_baby());
        assert_eq!(turtle::read(&made).home, nest);
        let x = 5.0 + 0.3 + i as f64 * 0.2;
        assert!((baby.position[0] - x).abs() < 1e-9);
        assert!((baby.position[2] - 5.3).abs() < 1e-9);
    }

    // At noon, an egg cracks one time in five hundred.
    let mut region = on_ticking_floor_with(&blocks);
    region.clock.day = 6_000;
    let mut seed = 0;
    let mut random = loop {
        let random = Random::of_entity(seed, EntityId(1), 0);
        if random.clone().next_float() >= 0.002 {
            break random;
        }
        seed += 1;
    };
    let mut output = TickOutput::default();
    region.turtle_egg_tick(nest, clutch, &mut random, &mut output);
    assert_eq!(region.block(nest), Some(clutch));
}

#[test]
fn two_turtles_in_love_breed_an_egg_for_one_of_them_and_no_child() {
    let mut region = on_ticking_floor_with(&beach());
    for (key, x) in [(ONE, 5.5), (TWO, 7.5)] {
        let mut mob = turtle_with(|own| own.home = BlockPos::new(6, -60, 6));
        let mut shared = Animal::default();
        shared.set_in_love(0);
        mob.state = animal::with_animal(&mob.state, &shared);
        lay(
            &mut region,
            &mob_at(key, entity_types::TURTLE, [x, -60.0, 6.5], mob),
        );
    }
    let mut bred = false;
    for _ in 0..200 {
        checked(&mut region, &TickInputs::default());
        let eggs = [ONE, TWO]
            .into_iter()
            .filter(|key| turtle::read(&mob(&region, *key)).has_egg)
            .count();
        if eggs > 0 {
            assert_eq!(eggs, 1);
            bred = true;
            break;
        }
    }
    assert!(bred);
    assert_eq!(all_of(&region, entity_types::TURTLE).len(), 2, "no child");
    for key in [ONE, TWO] {
        assert_eq!(
            animal::animal_of(&mob(&region, key).state).age,
            animal::BRED_AGE
        );
    }
    assert_eq!(all_of(&region, entity_types::EXPERIENCE_ORB).len(), 1);
}

#[test]
fn a_baby_turtle_that_grows_up_drops_a_scute() {
    let mut region = on_ticking_floor_with(&beach());
    let mut baby = Mob::fresh();
    let mut shared = Animal::default();
    shared.set_age(-20, 0);
    baby.state = animal::write(&shared, &turtle::TurtleState::default());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::TURTLE, [6.5, -60.0, 6.5], baby),
    );
    idle(&mut region, 25);
    assert!(!animal::animal_of(&mob(&region, ONE).state).is_baby());
    let items: Vec<_> = all_of(&region, entity_types::ITEM)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some(stack),
            _ => None,
        })
        .collect();
    assert_eq!(items, [one(items::TURTLE_SCUTE)]);
}

#[test]
fn a_tick_of_water_creatures_is_a_function_of_its_inputs() {
    let run = || {
        let mut region = on_ticking_floor_with(&pool());
        let kinds = [
            entity_types::COD,
            entity_types::SALMON,
            entity_types::PUFFERFISH,
            entity_types::TROPICAL_FISH,
            entity_types::SQUID,
            entity_types::GLOW_SQUID,
            entity_types::DOLPHIN,
            entity_types::TURTLE,
        ];
        for (i, kind) in kinds.into_iter().enumerate() {
            let x = 5.5 + (i % 4) as f64 * 1.5;
            let z = 5.5 + (i / 4) as f64 * 3.0;
            let mut made = Mob::fresh();
            if kind == entity_types::SQUID || kind == entity_types::GLOW_SQUID {
                squid::construct(&mut made, &mut Random::of_entity(3, EntityId(i as u64), 0));
            }
            lay(
                &mut region,
                &mob_at(EntityId(60 + 2 * i as u64), kind, [x, -59.5, z], made),
            );
        }
        // A fish out of water and a dolphin on land beside the pool.
        lay(
            &mut region,
            &mob_at(
                EntityId(90),
                entity_types::COD,
                [14.5, -60.0, 14.5],
                Mob::fresh(),
            ),
        );
        lay(
            &mut region,
            &mob_at(
                EntityId(92),
                entity_types::DOLPHIN,
                [1.5, -60.0, 14.5],
                Mob::fresh(),
            ),
        );
        let mut changes = Vec::new();
        for _ in 0..200 {
            let output = checked(&mut region, &TickInputs::default());
            changes.push((output.entity_changes.clone(), output.effects.clone()));
        }
        changes
    };
    assert_eq!(run(), run());
}
