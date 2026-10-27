//! The keepers in a region, from the brief's sections 7.12 and 7.13 as a player meets
//! them before their brains come: eggs make each with its defaults and its draws in the
//! game's order, a nautilus is tamed, kept near where it was left and dries out on land,
//! a copper golem is built from a pumpkin on copper, weathers, is waxed and scraped,
//! turns into a statue whose pose a click turns, and counts as an opener of the chest it
//! has open; and their brains: a golem sorting a copper chest's items into a chest and
//! waiting by one a player has open, a nautilus charging what hurt it and following a
//! player's fish, and at a pinned line, a golem passing over a chest across it, a
//! nautilus tempted, bred and carried over by its charge across it.

use clustine_data::{BlockState, entity_types, items, sounds};
use clustine_world::{Effected, Entity, EntityChange, EntityId, EntityUse};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::copper_golem::{self, CopperGolemState};
use crate::mobs::types::nautilus::{self, NautilusState};
use crate::mobs::types::tamable::Tame;
use crate::region::Random;
use crate::region::copper_golems::{chest_of, copper_age};
use crate::region::golem_chests;

const ONE: EntityId = EntityId(40);

/// The UUID of player 1, who tames what these tests tame.
const OWNER: u128 = 1;

/// A mob of the type `kind` at `position` with the type state `state`, of its type's
/// full health and air.
fn mob_at(
    key: EntityId,
    kind: i32,
    position: [f64; 3],
    state: clustine_world::TypeState,
) -> Entity {
    let kind = kind as u16;
    let mut mob = Mob::fresh();
    mob.state = state;
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
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
        data: mob.encode(),
    }
}

fn nautilus_state(own: &NautilusState) -> clustine_world::TypeState {
    animal::write(&Animal::default(), own)
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

fn all_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

fn sounds_of(output: &TickOutput) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => Some((*kind, *position, *data)),
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

fn idle(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked(region, &TickInputs::default());
    }
}

fn holding(region: &mut Region, item: i32) {
    checked(
        region,
        &moves(vec![
            set_slot(1, 0, Some(ItemStack::new(item, 1))),
            select(1, 0),
        ]),
    );
}

/// Player 1's use of the mob `key` holding `held`, between ticks: the mob's stream aside
/// as it was before, and what the use showed.
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

/// `Mob.finalizeSpawn`'s own draws of the level's: the follow range's bonus and the
/// hand.
fn mob_draws(level: &mut Random, made: &Mob) {
    let bonus = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
    let left = level.next_float() < 0.05;
    let follow = made
        .body
        .attributes
        .made(clustine_data::attributes::FOLLOW_RANGE as u16)
        .unwrap();
    assert_eq!(follow.modifiers[0].amount.to_bits(), bonus.to_bits());
    assert_eq!(made.body.flags.left_handed, left);
}

#[test]
fn an_egg_makes_each_keeper_with_its_defaults_and_its_draws_in_the_game_s_order() {
    for kind in [entity_types::NAUTILUS, entity_types::ZOMBIE_NAUTILUS] {
        let mut region = on_ticking_floor_with(&pool());
        let player_key = region.players[&player(1)].entity_id;
        let mut level = region.entity_stream(player_key).random;
        let mut output = TickOutput::default();
        let key = region
            .spawn_mob(
                kind as u16,
                player(1),
                BlockPos::new(6, -60, 6),
                true,
                true,
                &mut output,
            )
            .expect("made");
        let made = mob(&region, key);
        let _yaw = level.next_float();
        if kind == entity_types::ZOMBIE_NAUTILUS {
            // The variant's `nextInt(1)`: the floor's biome is no warm ocean.
            assert_eq!(level.next_int(1), 0);
        }
        let cooldown = level.next_int_between(2400, 3600);
        mob_draws(&mut level, &made);
        let (shared, own) = nautilus::state_of(&made);
        assert!(!shared.is_baby());
        let provider = crate::mobs::types::of(kind as u16).brain().unwrap();
        let memories = crate::mobs::brain::memories_of(provider, &made, 0);
        assert_eq!(
            memories.int(crate::mobs::brain::MemoryKind::AttackTargetCooldown),
            Some(cooldown),
            "{kind}"
        );
        assert_eq!(own.variant, 0);
        assert!(!own.tame.is_tame() && !own.dashing);
        assert_eq!(made.body.vitals.health, 15.0);
        assert_eq!(made.body.vitals.air, 300);
        // Its ambient sound, of the medium a fresh one's eyes were in: on land.
        let on_land = if kind == entity_types::NAUTILUS {
            sounds::ENTITY_NAUTILUS_AMBIENT_LAND
        } else {
            sounds::ENTITY_ZOMBIE_NAUTILUS_AMBIENT_LAND
        };
        assert_eq!(sounds_of(&output), [on_land]);
        // In the water its eyes are under it by its next tick.
        idle(&mut region, 2);
        assert!(nautilus::state_of(&mob(&region, key)).1.under_water);
    }

    let mut region = on_ticking_floor_with(&[]);
    let player_key = region.players[&player(1)].entity_id;
    let mut level = region.entity_stream(player_key).random;
    let mut output = TickOutput::default();
    let key = region
        .spawn_mob(
            entity_types::COPPER_GOLEM as u16,
            player(1),
            BlockPos::new(3, -60, 3),
            true,
            true,
            &mut output,
        )
        .expect("made");
    let made = mob(&region, key);
    // Its own stream: its UUID, its constructor's yaw, its brain's two sensors' delays,
    // then its first transport cooldown.
    let mut own = Random::of_entity_aside(region.config.seed, key, 0, region.tick);
    own.next_u64();
    own.next_u64();
    own.next_float();
    own.next_int(20);
    own.next_int(20);
    let cooldown = 60 + own.next_int(40) as i32;
    let golem = copper_golem::state_of(&made);
    let memories = crate::mobs::brain::memories_of(&copper_golem::PROVIDER, &made, 0);
    assert_eq!(
        memories.int(crate::mobs::brain::MemoryKind::TransportItemsCooldownTicks),
        Some(cooldown)
    );
    assert_eq!(
        (golem.weather, golem.next_weathering, golem.state),
        (
            copper_golem::UNAFFECTED,
            copper_golem::UNSET,
            copper_golem::IDLE
        )
    );
    assert!(made.body.flags.persistent);
    assert_eq!(made.body.vitals.health, 12.0);
    let _yaw = level.next_float();
    mob_draws(&mut level, &made);
    // Its spawn sound, and no ambient one: a golem has none.
    assert_eq!(sounds_of(&output), [sounds::ENTITY_COPPER_GOLEM_SPAWN]);
    // Its first tick draws its next weathering of the level's, from the game time.
    let now = region.clock.game_time as i64;
    idle(&mut region, 1);
    let next = copper_golem::state_of(&mob(&region, key)).next_weathering;
    assert!(
        (now + 504_000..=now + 1 + 552_000).contains(&next),
        "{next}"
    );
}

#[test]
fn a_pufferfish_tames_a_nautilus_which_then_keeps_a_home_where_it_is() {
    let mut region = on_ticking_floor_with(&pool());
    let at = [6.5, -60.0, 6.5];
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::NAUTILUS,
            at,
            nautilus_state(&NautilusState::default()),
        ),
    );
    let mut tamed = false;
    for _ in 0..60 {
        let fish = Some(ItemStack::new(items::PUFFERFISH, 1));
        let (mut replay, output) = used_now(&mut region, ONE, fish);
        let tames = replay.next_int(3) == 0;
        let own = nautilus::state_of(&mob(&region, ONE)).1;
        assert!(sounds_of(&output).contains(&sounds::ENTITY_NAUTILUS_EAT));
        if tames {
            assert_eq!(entity_events(&output, ONE), [7]);
            assert_eq!(own.tame.owner, Some(OWNER));
            tamed = true;
            break;
        }
        assert_eq!(entity_events(&output, ONE), [6]);
        assert!(!own.tame.is_tame());
        idle(&mut region, 1);
    }
    assert!(tamed, "tamed within sixty pufferfish");
    let made = mob(&region, ONE);
    assert!(made.body.flags.persistent);
    // Its next turn keeps it near where it is: a home at its block, of radius 32.
    idle(&mut region, 1);
    let home = mob(&region, ONE).body.home;
    let block = crate::physics::containing(record(&region, ONE).unwrap().position);
    assert_eq!((home.centre, home.radius), (Some(block), 32));
    // A tamed one shows it to clients.
    let values = crate::shown::mob_values(
        entity_types::NAUTILUS as u16,
        &record(&region, ONE).unwrap().data,
    );
    assert!(values.contains(&(18, crate::shown::DataShown::Byte(4), false)));
}

#[test]
fn a_nautilus_on_land_dries_out_and_a_zombie_nautilus_does_not() {
    let mut region = on_ticking_floor_with(&[]);
    // At night: by day the zombie nautilus would burn, as the game's.
    region.clock.day = 18_000;
    let mut tame = Tame::default();
    tame.set_tame_flag(true);
    let own = NautilusState {
        tame,
        ..NautilusState::default()
    };
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::NAUTILUS,
            [2.5, -60.0, 2.5],
            nautilus_state(&own),
        ),
    );
    let two = EntityId(42);
    lay(
        &mut region,
        &mob_at(
            two,
            entity_types::ZOMBIE_NAUTILUS,
            [5.5, -60.0, 5.5],
            nautilus_state(&own),
        ),
    );
    idle(&mut region, 3);
    assert_eq!(mob(&region, ONE).body.vitals.air, 297);
    assert_eq!(mob(&region, two).body.vitals.air, 300);
    // At -20 it takes 2 of drying out, its air back to nothing.
    idle(&mut region, 317);
    let dried = mob(&region, ONE);
    assert_eq!(dried.body.vitals.air, 0);
    assert_eq!(dried.body.vitals.health, 13.0);
    assert_eq!(mob(&region, two).body.vitals.health, 15.0);
}

/// The region with a block of `copper` at (2, -60, 0), and player 1 holding a carved
/// pumpkin.
fn with_copper(copper: BlockState) -> Region {
    let mut region = on_ticking_floor_with(&[(BlockPos::new(2, -60, 0), copper)]);
    holding(&mut region, items::CARVED_PUMPKIN);
    region
}

#[test]
fn a_carved_pumpkin_on_copper_builds_a_golem_of_its_age_over_a_copper_chest() {
    for (copper, weather, chest) in [
        (
            clustine_data::blocks::COPPER_BLOCK,
            copper_golem::UNAFFECTED,
            "minecraft:copper_chest",
        ),
        (
            clustine_data::blocks::WEATHERED_COPPER,
            copper_golem::WEATHERED,
            "minecraft:weathered_copper_chest",
        ),
        (
            clustine_data::blocks::WAXED_EXPOSED_COPPER,
            copper_golem::EXPOSED,
            "minecraft:waxed_exposed_copper_chest",
        ),
    ] {
        let mut region = with_copper(copper);
        let output = checked(&mut region, &moves(vec![place(1, 2, -60, 0, Face::Top)]));
        let pumpkin = BlockPos::new(2, -59, 0);
        let copper_at = BlockPos::new(2, -60, 0);
        // Both blocks broken as they were, the pumpkin first.
        let broken: Vec<_> = level_events(&output)
            .into_iter()
            .filter(|(kind, _, _)| *kind == 2001)
            .collect();
        assert_eq!(broken.len(), 2);
        assert_eq!(broken[0].1, pumpkin);
        assert_eq!(broken[1], (2001, copper_at, i32::from(copper.0)));
        assert_eq!(region.block(pumpkin), Some(clustine_data::blocks::AIR));
        let built = region.block(copper_at).unwrap();
        assert_eq!(built.block().unwrap().name, chest);
        assert_eq!(built.property("type"), Some("single"));
        let golems = all_of(&region, entity_types::COPPER_GOLEM);
        assert_eq!(golems.len(), 1);
        assert_eq!(golems[0].position, [2.5, -58.95, 0.5]);
        let golem = copper_golem::state_of(&Mob::decode(&golems[0].data).unwrap());
        // Unwaxed whatever the copper, of its age; ticked in the tick it was built in, its
        // next weathering drawn from then.
        assert_eq!(golem.weather, weather);
        let now = region.clock.game_time as i64;
        assert!(
            (now + 503_999..=now + 552_000).contains(&golem.next_weathering),
            "{}",
            golem.next_weathering
        );
        assert!(sounds_of(&output).contains(&sounds::ENTITY_COPPER_GOLEM_SPAWN));
        // It falls onto its chest.
        idle(&mut region, 20);
        let golem = record(&region, golems[0].key).unwrap();
        assert!(golem.on_ground && golem.position[1] < -59.0);
    }
    // On anything else, a pumpkin is a pumpkin.
    let mut region = with_copper(clustine_data::blocks::CUT_COPPER);
    checked(&mut region, &moves(vec![place(1, 2, -60, 0, Face::Top)]));
    assert!(all_of(&region, entity_types::COPPER_GOLEM).is_empty());
    let pumpkin = region.block(BlockPos::new(2, -59, 0)).unwrap();
    assert!(clustine_blocks::data::is_block(
        pumpkin,
        clustine_data::blocks::CARVED_PUMPKIN
    ));
}

#[test]
fn the_copper_s_age_names_its_chest_and_its_golem() {
    assert_eq!(copper_age(clustine_data::blocks::COPPER_BLOCK), (0, false));
    assert_eq!(
        copper_age(clustine_data::blocks::OXIDIZED_COPPER),
        (3, false)
    );
    assert_eq!(
        copper_age(clustine_data::blocks::WAXED_WEATHERED_COPPER),
        (2, true)
    );
    let chest = chest_of(clustine_data::blocks::WAXED_OXIDIZED_COPPER, "east");
    assert_eq!(
        chest.block().unwrap().name,
        "minecraft:waxed_oxidized_copper_chest"
    );
    assert_eq!(chest.property("facing"), Some("east"));
}

#[test]
fn a_copper_golem_weathers_at_its_time_and_honeycomb_and_an_axe_change_it() {
    let mut region = on_ticking_floor_with(&[]);
    let own = CopperGolemState {
        next_weathering: 1,
        ..CopperGolemState::default()
    };
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::COPPER_GOLEM,
            [2.5, -60.0, 2.5],
            copper_golem::write(&own),
        ),
    );
    // Past its time: a stage more, the next drawn from the last.
    idle(&mut region, 1);
    let own = copper_golem::state_of(&mob(&region, ONE));
    assert_eq!(own.weather, copper_golem::EXPOSED);
    assert!((504_001..=552_001).contains(&own.next_weathering));
    let values = crate::shown::mob_values(
        entity_types::COPPER_GOLEM as u16,
        &record(&region, ONE).unwrap().data,
    );
    assert!(values.contains(&(
        16,
        crate::shown::DataShown::Serialised(crate::shown::Serialiser::WeatheringCopperState, 1),
        false
    )));
    // Waxed: level event 3003 at its block, and it weathers no more.
    let honeycomb = Some(ItemStack::new(items::HONEYCOMB, 1));
    let (_, output) = used_now(&mut region, ONE, honeycomb);
    assert_eq!(level_events(&output), [(3003, BlockPos::new(2, -60, 2), 0)]);
    assert!(sounds_of(&output).contains(&sounds::ITEM_HONEYCOMB_WAX_ON));
    assert_eq!(
        copper_golem::state_of(&mob(&region, ONE)).next_weathering,
        copper_golem::WAXED
    );
    idle(&mut region, 2);
    assert_eq!(
        copper_golem::state_of(&mob(&region, ONE)).next_weathering,
        copper_golem::WAXED
    );
    // The axe takes the wax off, then scrapes it back to unaffected.
    let axe = Some(ItemStack::new(items::IRON_AXE, 1));
    let (_, output) = used_now(&mut region, ONE, axe.clone());
    assert_eq!(level_events(&output)[0].0, 3004);
    let (_, output) = used_now(&mut region, ONE, axe);
    assert_eq!(level_events(&output)[0].0, 3005);
    assert!(sounds_of(&output).contains(&sounds::ITEM_AXE_SCRAPE));
    assert_eq!(
        copper_golem::state_of(&mob(&region, ONE)).weather,
        copper_golem::UNAFFECTED
    );
}

#[test]
fn an_oxidised_copper_golem_in_the_air_soon_turns_into_a_statue_facing_its_way() {
    let mut region = on_ticking_floor_with(&[]);
    let own = CopperGolemState {
        weather: copper_golem::OXIDIZED,
        next_weathering: 0,
        ..CopperGolemState::default()
    };
    let mut golem = mob_at(
        ONE,
        entity_types::COPPER_GOLEM,
        [4.5, -60.0, 4.5],
        copper_golem::write(&own),
    );
    golem.yaw = 90.0;
    lay(&mut region, &golem);
    let at = BlockPos::new(4, -60, 4);
    let mut turned = None;
    for tick in 0..5000 {
        let output = checked(&mut region, &TickInputs::default());
        if record(&region, ONE).is_none() {
            turned = Some((tick, output));
            break;
        }
    }
    let (tick, output) = turned.expect("a statue within 5000 ticks");
    // One tick in about 172, of a draw a tick.
    assert!(tick < 2000, "{tick}");
    let statue = region.block(at).unwrap();
    assert!(clustine_blocks::data::is_block(
        statue,
        clustine_data::blocks::OXIDIZED_COPPER_GOLEM_STATUE
    ));
    assert_eq!(statue.property("facing"), Some("west"));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_COPPER_GOLEM_BECOME_STATUE));
    assert!(all_of(&region, entity_types::COPPER_GOLEM).is_empty());
    // A click with anything but an axe turns it to its next pose, with its sound.
    let pose = statue.property("copper_golem_pose").unwrap();
    let next =
        copper_golem::POSES[(copper_golem::POSES.iter().position(|p| *p == pose).unwrap() + 1) % 4];
    holding(&mut region, items::STONE);
    let output = checked(&mut region, &moves(vec![place(1, 4, -60, 4, Face::Top)]));
    let used = region.block(at).unwrap();
    assert_eq!(used.property("copper_golem_pose"), Some(next));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_COPPER_GOLEM_BECOME_STATUE));
    // An axe passes.
    holding(&mut region, items::IRON_AXE);
    checked(&mut region, &moves(vec![place(1, 4, -60, 4, Face::Top)]));
    assert_eq!(region.block(at), Some(used));
}

#[test]
fn a_golem_in_water_or_waxed_never_turns_into_a_statue() {
    let mut region = on_ticking_floor_with(&pool());
    let oxidised = CopperGolemState {
        weather: copper_golem::OXIDIZED,
        next_weathering: 0,
        ..CopperGolemState::default()
    };
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::COPPER_GOLEM,
            [6.5, -61.0, 6.5],
            copper_golem::write(&oxidised),
        ),
    );
    let two = EntityId(42);
    let waxed = CopperGolemState {
        next_weathering: copper_golem::WAXED,
        ..oxidised
    };
    lay(
        &mut region,
        &mob_at(
            two,
            entity_types::COPPER_GOLEM,
            [1.5, -60.0, 1.5],
            copper_golem::write(&waxed),
        ),
    );
    idle(&mut region, 1500);
    assert!(record(&region, ONE).is_some());
    assert!(record(&region, two).is_some());
}

#[test]
fn a_golem_with_a_chest_open_is_counted_at_its_rechecks_until_it_lets_go() {
    let chest = BlockPos::new(4, -60, 4);
    let mut region = on_ticking_floor_with(&[(chest, clustine_data::blocks::CHEST)]);
    let own = CopperGolemState {
        opened_chest: Some(chest),
        next_weathering: copper_golem::WAXED,
        ..CopperGolemState::default()
    };
    // Without its AI, so that its sorting does not let go of the chest.
    let mut golem = mob_at(
        ONE,
        entity_types::COPPER_GOLEM,
        [5.5, -60.0, 4.5],
        copper_golem::write(&own),
    );
    let mut still = Mob::decode(&golem.data).unwrap();
    still.body.flags.no_ai = true;
    golem.data = still.encode();
    lay(&mut region, &golem);
    // The golem's `startOpen`, as its brain does at its interaction's first tick.
    let mut output = TickOutput::default();
    region.start_open(chest, &mut output);
    assert_eq!(region.openers(chest), 1);
    assert!(sounds_of(&output).contains(&sounds::BLOCK_CHEST_OPEN));
    // Its rechecks every five ticks find it, with no player at the chest.
    idle(&mut region, 12);
    assert_eq!(region.openers(chest), 1);
    // Let go of without `stopOpen` (a golem hurt or pushed away): the next recheck
    // closes the lid.
    let mut golem = mob(&region, ONE);
    copper_golem::update(&mut golem, |own| own.opened_chest = None);
    let mut entity = record(&region, ONE).unwrap();
    entity.data = golem.encode();
    lay(&mut region, &entity);
    let mut closed = false;
    for _ in 0..6 {
        let output = checked(&mut region, &TickInputs::default());
        if sounds_of(&output).contains(&sounds::BLOCK_CHEST_CLOSE) {
            closed = true;
            break;
        }
    }
    assert!(closed);
    assert_eq!(region.openers(chest), 0);
}

// ------------------------------------------------------------------------------------
// Their brains, as a player meets them.

fn stack_of(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

/// What the container at `pos` holds, slot by slot where anything is.
fn held_in(region: &Region, pos: BlockPos) -> Vec<(usize, ItemStack)> {
    region
        .container_at(pos)
        .expect("a container")
        .slots
        .iter()
        .enumerate()
        .filter_map(|(slot, held)| {
            held.as_ref()
                .filter(|stack| stack.count > 0)
                .map(|stack| (slot, stack.clone()))
        })
        .collect()
}

/// `slots` put into the container at `pos`.
fn fill_with(region: &mut Region, pos: BlockPos, slots: &[(usize, ItemStack)]) {
    let mut container = region.container_at(pos).expect("a container");
    for (slot, stack) in slots {
        container.slots[*slot] = Some(stack.clone());
    }
    region.set_container(pos, container, &mut TickOutput::default());
}

/// The openers' counts the lid of the chest at `pos` was said to have in `output`.
fn lid_counts(output: &TickOutput, pos: BlockPos) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEvent {
                position,
                kind: 1,
                data,
                ..
            } if *position == pos => Some(*data),
            _ => None,
        })
        .collect()
}

/// A waxed copper golem (no weathering's draws) at `position`, holding `held`.
fn golem_holding(key: EntityId, position: [f64; 3], held: Option<ItemStack>) -> Entity {
    let own = CopperGolemState {
        next_weathering: copper_golem::WAXED,
        ..CopperGolemState::default()
    };
    let mut golem = mob_at(
        key,
        entity_types::COPPER_GOLEM,
        position,
        copper_golem::write(&own),
    );
    let mut body = Mob::decode(&golem.data).unwrap();
    body.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)] = held;
    golem.data = body.encode();
    golem
}

/// What the golem `key` holds.
fn golem_hand(region: &Region, key: EntityId) -> Option<ItemStack> {
    mob(region, key).body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)].clone()
}

#[test]
fn a_golem_carries_16_of_a_copper_chest_s_first_stack_to_a_chest_with_both_lids_shown() {
    let source = BlockPos::new(12, -60, 3);
    let goal = BlockPos::new(3, -60, 12);
    let mut region = on_ticking_floor_with(&[
        (source, clustine_data::blocks::COPPER_CHEST),
        (goal, clustine_data::blocks::CHEST),
    ]);
    fill_with(&mut region, source, &[(2, stack_of(items::DIRT, 20))]);
    lay(&mut region, &golem_holding(ONE, [7.5, -60.0, 7.5], None));
    let (mut source_lid, mut goal_lid) = (Vec::new(), Vec::new());
    let mut states = Vec::new();
    let mut carried = None;
    let mut done = false;
    for _ in 0..1500 {
        let output = checked(&mut region, &TickInputs::default());
        source_lid.extend(lid_counts(&output, source));
        goal_lid.extend(lid_counts(&output, goal));
        let state = copper_golem::state_of(&mob(&region, ONE)).state;
        if states.last() != Some(&state) {
            states.push(state);
        }
        carried = carried.or(golem_hand(&region, ONE));
        if !held_in(&region, goal).is_empty() {
            done = true;
            break;
        }
    }
    assert!(done, "carried within 1500 ticks; states {states:?}");
    assert_eq!(carried, Some(stack_of(items::DIRT, 16)));
    assert_eq!(held_in(&region, source), [(2, stack_of(items::DIRT, 4))]);
    assert_eq!(held_in(&region, goal), [(0, stack_of(items::DIRT, 16))]);
    assert_eq!(golem_hand(&region, ONE), None);
    // Its state as clients animate it: taking, idle again, putting.
    assert_eq!(
        states[..4],
        [
            copper_golem::IDLE,
            copper_golem::GETTING_ITEM,
            copper_golem::IDLE,
            copper_golem::DROPPING_ITEM
        ]
    );
    // Each lid opened by it (a count of 1) and shut again (0).
    assert_eq!(source_lid.first(), Some(&1));
    assert_eq!(source_lid.last(), Some(&0));
    assert_eq!(goal_lid.first(), Some(&1));
    assert_eq!(region.openers(source), 0);
}

#[test]
fn a_golem_waits_by_a_chest_a_player_has_open_and_puts_into_it_once_it_is_shut() {
    let goal = BlockPos::new(3, -60, 3);
    let mut region = on_ticking_floor_with(&[(goal, clustine_data::blocks::CHEST)]);
    lay(
        &mut region,
        &golem_holding(ONE, [9.5, -60.0, 9.5], Some(stack_of(items::DIRT, 16))),
    );
    // Player 1, at the spawn, has the chest open.
    region.players.get_mut(&player(1)).unwrap().menu = Some(crate::state::Menu {
        serial: 1,
        of: crate::state::MenuOf::Block {
            at: goal,
            other: None,
        },
        kind: crate::api::MenuKind::Rows3,
        carried: None,
        drag: None,
    });
    region.start_open(goal, &mut TickOutput::default());
    let near = |region: &Region| {
        let at = record(region, ONE).unwrap().position;
        let dx = at[0] - 3.5;
        let dz = at[2] - 3.5;
        (dx * dx + dz * dz).sqrt() < 6.0
    };
    let mut waited = 0;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        assert!(held_in(&region, goal).is_empty());
        if near(&region) {
            waited += 1;
            if waited == 100 {
                break;
            }
        }
    }
    assert_eq!(waited, 100, "it came to the chest and waited");
    assert_eq!(golem_hand(&region, ONE), Some(stack_of(items::DIRT, 16)));
    // The player shuts it: the golem puts its stack in.
    region.players.get_mut(&player(1)).unwrap().menu = None;
    region.stop_open(goal, &mut TickOutput::default());
    let mut put = false;
    for _ in 0..300 {
        checked(&mut region, &TickInputs::default());
        if !held_in(&region, goal).is_empty() {
            put = true;
            break;
        }
    }
    assert!(put);
    assert_eq!(held_in(&region, goal), [(0, stack_of(items::DIRT, 16))]);
}

/// The health of the region's mob `key`, or none where it is gone.
fn health(region: &Region, key: EntityId) -> Option<f32> {
    record(region, key).map(|entity| Mob::decode(&entity.data).unwrap().body.vitals.health)
}

#[test]
fn a_nautilus_hurt_by_a_mob_in_the_water_charges_it_and_hits_it() {
    for kind in [entity_types::NAUTILUS, entity_types::ZOMBIE_NAUTILUS] {
        let mut region = on_ticking_floor_with(&pool());
        let puffer = EntityId(44);
        lay(
            &mut region,
            &mob_at(
                ONE,
                kind,
                [5.5, -60.0, 7.5],
                nautilus_state(&NautilusState::default()),
            ),
        );
        lay(
            &mut region,
            &mob_at(
                puffer,
                entity_types::PUFFERFISH,
                [9.5, -60.0, 7.5],
                Default::default(),
            ),
        );
        idle(&mut region, 2);
        let full = health(&region, puffer).unwrap();
        // The pufferfish's hit, between ticks.
        let from = record(&region, puffer).unwrap().position;
        region.hit_by_mob(
            puffer,
            entity_types::PUFFERFISH as u16,
            from,
            ONE,
            1.0,
            (None, None),
            None,
            &mut TickOutput::default(),
        );
        let mut hit = false;
        for _ in 0..100 {
            checked(&mut region, &TickInputs::default());
            if health(&region, puffer).is_none_or(|now| now < full) {
                hit = true;
                break;
            }
        }
        assert!(hit, "{kind}");
        // Its charge's cooldown runs.
        let provider = crate::mobs::types::of(kind as u16).brain().unwrap();
        let memories = crate::mobs::brain::memories_of(provider, &mob(&region, ONE), 0);
        assert!(memories.check(
            crate::mobs::brain::MemoryKind::ChargeCooldownTicks,
            crate::mobs::brain::Status::Present
        ));
    }
}

#[test]
fn a_nautilus_looks_at_and_follows_a_player_holding_fish() {
    let mut region = on_ticking_floor_with(&pool());
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::NAUTILUS,
            [4.5, -60.0, 4.5],
            nautilus_state(&NautilusState::default()),
        ),
    );
    holding(&mut region, items::SALMON);
    let player_key = region.players[&player(1)].entity_id;
    let mut followed = false;
    for _ in 0..60 {
        checked(&mut region, &TickInputs::default());
        let provider = crate::mobs::types::of(entity_types::NAUTILUS as u16)
            .brain()
            .unwrap();
        let memories = crate::mobs::brain::memories_of(provider, &mob(&region, ONE), 0);
        if let Some(crate::mobs::brain::Value::Tracker(crate::mobs::brain::Tracker::Entity {
            key,
            ..
        })) = memories.get(crate::mobs::brain::MemoryKind::LookTarget)
            && key == player_key
        {
            followed = true;
            break;
        }
    }
    assert!(followed);
}

#[test]
fn a_tick_of_the_keepers_is_a_function_of_its_inputs() {
    let run = || {
        let mut blocks = pool();
        blocks.push((
            BlockPos::new(13, -60, 1),
            clustine_data::blocks::COPPER_CHEST,
        ));
        blocks.push((BlockPos::new(1, -60, 13), clustine_data::blocks::CHEST));
        let mut region = on_ticking_floor_with(&blocks);
        fill_with(
            &mut region,
            BlockPos::new(13, -60, 1),
            &[(0, stack_of(items::COBBLESTONE, 40))],
        );
        lay(
            &mut region,
            &mob_at(
                ONE,
                entity_types::NAUTILUS,
                [6.5, -60.0, 6.5],
                nautilus_state(&NautilusState::default()),
            ),
        );
        lay(
            &mut region,
            &mob_at(
                EntityId(42),
                entity_types::ZOMBIE_NAUTILUS,
                [9.5, -60.0, 9.5],
                nautilus_state(&NautilusState::default()),
            ),
        );
        lay(
            &mut region,
            &mob_at(
                EntityId(44),
                entity_types::PUFFERFISH,
                [8.5, -60.0, 5.5],
                Default::default(),
            ),
        );
        lay(
            &mut region,
            &golem_holding(EntityId(46), [13.5, -60.0, 8.5], None),
        );
        let mut changes = Vec::new();
        for _ in 0..300 {
            let output = checked(&mut region, &TickInputs::default());
            changes.push((output.entity_changes.clone(), output.effects.clone()));
        }
        changes
    };
    assert_eq!(run(), run());
}

// ------------------------------------------------------------------------------------
// At a pinned line.

/// A region of the land west of chunk x = 2 (x below 32), its chunks x of -1 to 1 by z
/// of -1 to 1 loaded on a stone floor with `here` set, and the chunks of x = 2
/// another's, copied, with `beyond` set, `chest` laid with its data and `entities` in the
/// one at z = 0.
fn at_a_line(
    here: &[(BlockPos, BlockState)],
    own: &[Entity],
    beyond: &[(BlockPos, BlockState)],
    chest: Option<(BlockPos, Vec<u8>)>,
    entities: &[Entity],
) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let set = |chunk: &mut Chunk, position: ChunkPos, blocks: &[(BlockPos, BlockState)]| {
        for (at, state) in blocks {
            if at.chunk() == position {
                chunk.set(
                    at.x.rem_euclid(16) as usize,
                    at.y,
                    at.z.rem_euclid(16) as usize,
                    *state,
                );
            }
        }
    };
    let chunks = around
        .iter()
        .map(|position| {
            let mut chunk = floor();
            set(&mut chunk, *position, here);
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    for entity in own {
        lay(&mut region, entity);
    }
    let copies: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    let bases = copies
        .iter()
        .map(|position| {
            let mut base = floor();
            set(&mut base, *position, beyond);
            if let Some((at, data)) = &chest
                && at.chunk() == *position
            {
                base.set_block_entity_data(
                    clustine_world::InChunk::new(
                        at.x.rem_euclid(16) as usize,
                        at.y,
                        at.z.rem_euclid(16) as usize,
                    ),
                    data.clone(),
                )
                .unwrap();
            }
            if position.z == 0 {
                base.set_entities(entities.to_vec());
            }
            (*position, base)
        })
        .collect();
    region.tick(&TickInputs {
        foreign: copies
            .iter()
            .map(|position| (*position, RegionId(1)))
            .collect(),
        copy_bases: bases,
        ..TickInputs::default()
    });
    region
}

/// Water from x of 16 to 47 and z of 0 to 15, y of -60 to -57.
fn water_across() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in 16..48 {
        for z in 0..16 {
            for y in -60..=-57 {
                blocks.push((BlockPos::new(x, y, z), clustine_data::blocks::WATER));
            }
        }
    }
    blocks
}

#[test]
fn across_a_pinned_line_a_golem_neither_opens_nor_writes_a_chest_it_comes_to_and_passes_it_over() {
    // The first block across the line: the golem stands at it on its own side.
    let chest = BlockPos::new(32, -60, 4);
    let mut slots = vec![None; 27];
    slots[0] = Some(stack_of(items::DIRT, 10));
    let data = golem_chests::chest_data(clustine_data::blocks::COPPER_CHEST, &slots, 0);
    let golem = golem_holding(ONE, [29.5, -60.0, 4.5], None);
    let mut region = at_a_line(
        &[],
        &[golem],
        &[(chest, clustine_data::blocks::COPPER_CHEST)],
        Some((chest, data.clone())),
        &[],
    );
    let provider = &copper_golem::PROVIDER;
    let mut passed_over = false;
    for _ in 0..600 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(lid_counts(&output, chest).is_empty());
        assert!(output.effects.is_empty(), "{:?}", output.effects);
        let memories = crate::mobs::brain::memories_of(provider, &mob(&region, ONE), 0);
        if memories.get(crate::mobs::brain::MemoryKind::UnreachableTransportBlockPositions)
            == Some(crate::mobs::brain::Value::Blocks(vec![chest]))
        {
            passed_over = true;
            break;
        }
    }
    assert!(passed_over);
    assert_eq!(golem_hand(&region, ONE), None);
    assert_eq!(region.block_entity_data(chest), Some(data.as_slice()));
}

#[test]
fn across_a_pinned_line_a_nautilus_is_tempted_by_a_player_there() {
    for kind in [entity_types::NAUTILUS, entity_types::ZOMBIE_NAUTILUS] {
        let mut region = at_a_line(
            &water_across(),
            &[mob_at(
                ONE,
                kind,
                [28.5, -60.0, 8.5],
                nautilus_state(&NautilusState::default()),
            )],
            &water_across(),
            None,
            &[],
        );
        let mut inventory = clustine_world::Inventory::default();
        inventory.0[0] = Some(stack_of(items::COD, 1));
        let face = crate::api::FaceChange::Face {
            player: player(9),
            entity: EntityId(900),
            hops: 0,
            seq: crate::api::FaceSeq(1, 0),
            face: crate::api::PlayerFace::of(
                &Pose::at(Vec3::new(34.5, -60.0, 8.5)),
                false,
                false,
                &inventory,
                0,
                None,
            ),
        };
        checked(
            &mut region,
            &TickInputs {
                faces: vec![face],
                ..TickInputs::default()
            },
        );
        let provider = crate::mobs::types::of(kind as u16).brain().unwrap();
        let mut tempted = false;
        for _ in 0..40 {
            checked(&mut region, &TickInputs::default());
            let memories = crate::mobs::brain::memories_of(provider, &mob(&region, ONE), 0);
            if let Some(crate::mobs::brain::Value::Tracker(crate::mobs::brain::Tracker::Entity {
                key,
                ..
            })) = memories.get(crate::mobs::brain::MemoryKind::LookTarget)
                && key == EntityId(900)
            {
                tempted = true;
                break;
            }
        }
        assert!(tempted, "{kind}");
    }
}

#[test]
fn across_a_pinned_line_tamed_nautiluses_in_love_breed_by_the_protocol() {
    let mut tame = Tame::default();
    tame.set_tame_flag(true);
    tame.owner = Some(OWNER);
    let own = NautilusState {
        tame,
        ..NautilusState::default()
    };
    let in_love = |key: EntityId, x: f64| {
        let mut entity = mob_at(
            key,
            entity_types::NAUTILUS,
            [x, -60.0, 8.5],
            nautilus_state(&own),
        );
        let mut body = Mob::decode(&entity.data).unwrap();
        let mut shared = Animal::default();
        shared.set_in_love(0);
        body.state = animal::write(&shared, &own);
        entity.data = body.encode();
        entity
    };
    // The region's has the higher key: it asks the one across the line. On the floor,
    // where neither swims off.
    let mut region = at_a_line(
        &[],
        &[in_love(ONE, 31.2)],
        &[],
        None,
        &[in_love(EntityId(5), 32.6)],
    );
    let mut asked = None;
    for _ in 0..200 {
        let output = checked(&mut region, &TickInputs::default());
        if let Some(found) = output.effects.iter().find_map(|effect| match effect.what {
            Effected::OnEntity { key, ref by } => Some((key, by.clone())),
            _ => None,
        }) {
            asked = Some(found);
            break;
        }
    }
    let (to, by) = asked.expect("asked");
    assert_eq!(to, EntityId(5));
    assert!(
        matches!(by, EntityUse::BreedAsk { asker: ONE, .. }),
        "{by:?}"
    );
}

#[test]
fn across_a_pinned_line_a_charge_carries_a_nautilus_over_with_its_brain_whole() {
    let puffer = EntityId(5);
    let fish = mob_at(
        puffer,
        entity_types::PUFFERFISH,
        [36.5, -60.0, 8.5],
        Default::default(),
    );
    let mut region = at_a_line(
        &water_across(),
        &[mob_at(
            ONE,
            entity_types::NAUTILUS,
            [28.5, -60.0, 8.5],
            nautilus_state(&NautilusState::default()),
        )],
        &water_across(),
        None,
        std::slice::from_ref(&fish),
    );
    idle(&mut region, 2);
    // The pufferfish across the line hurts it, as its holder's effect would.
    let provider = crate::mobs::types::of(entity_types::NAUTILUS as u16)
        .brain()
        .unwrap();
    let mut nautilus = record(&region, ONE).unwrap();
    let mut body = Mob::decode(&nautilus.data).unwrap();
    let mut memories = crate::mobs::brain::memories_of(provider, &body, nautilus.lived);
    memories.set(
        crate::mobs::brain::MemoryKind::AttackTargetCooldown,
        crate::mobs::brain::Value::Int(3000),
    );
    memories.set_with_expiry(
        crate::mobs::brain::MemoryKind::AngryAt,
        crate::mobs::brain::Value::Entity(puffer),
        400,
    );
    crate::mobs::brain::write_memories(provider, &mut body, memories);
    nautilus.data = body.encode();
    lay(&mut region, &nautilus);
    let mut arrived = None;
    for _ in 0..60 {
        let output = checked(&mut region, &TickInputs::default());
        if let Some((_, entity)) = output.arrivals.iter().find(|(_, entity)| entity.key == ONE) {
            arrived = Some(entity.clone());
            break;
        }
    }
    let arrived = arrived.expect("carried over the line");
    assert!(arrived.position[0] >= 32.0);
    // Its brain whole: its charge running, its target and its anger with it.
    let body = Mob::decode(&arrived.data).unwrap();
    let memories = crate::mobs::brain::memories_of(provider, &body, arrived.lived);
    assert_eq!(
        memories.get(crate::mobs::brain::MemoryKind::AttackTarget),
        Some(crate::mobs::brain::Value::Entity(puffer))
    );
    assert!(memories.check(
        crate::mobs::brain::MemoryKind::AngryAt,
        crate::mobs::brain::Status::Present
    ));
    let charge = crate::mobs::brain::ids::FIRST_BEHAVIOUR + 12;
    let running = body
        .mind
        .brain
        .iter()
        .find(|entry| entry.id == charge)
        .map(|entry| {
            postcard::from_bytes::<(bool, i64, crate::mobs::types::nautilus::Charging)>(
                &entry.bytes,
            )
            .unwrap()
            .0
        });
    assert_eq!(running, Some(true));
}

// ------------------------------------------------------------------------------------
// Taken whole into another region (a split, a merge, a restore), half-way.

/// A brain's part `n` (its place among its provider's behaviours) as `(running, end,
/// own)`.
fn behaviour_part<S: serde::de::DeserializeOwned>(mob: &Mob, n: u16) -> Option<(bool, i64, S)> {
    let id = crate::mobs::brain::ids::FIRST_BEHAVIOUR + n;
    mob.mind
        .brain
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| postcard::from_bytes(&entry.bytes).unwrap())
}

#[test]
fn a_golem_taken_into_another_region_half_way_through_its_sort_finishes_it_on_its_tick() {
    let source = BlockPos::new(12, -60, 3);
    let goal = BlockPos::new(3, -60, 12);
    let build = || {
        on_ticking_floor_with(&[
            (source, clustine_data::blocks::COPPER_CHEST),
            (goal, clustine_data::blocks::CHEST),
        ])
    };
    let mut region = build();
    fill_with(&mut region, source, &[(2, stack_of(items::DIRT, 20))]);
    lay(&mut region, &golem_holding(ONE, [7.5, -60.0, 7.5], None));
    let mut opened_at = None;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        if copper_golem::state_of(&mob(&region, ONE)).state == copper_golem::GETTING_ITEM {
            opened_at = Some(record(&region, ONE).unwrap().lived);
            break;
        }
    }
    let opened_at = opened_at.expect("at the chest");
    idle(&mut region, 20);
    // Into another region: the golem's record as it is, and the chest's data, its
    // slots and its count of one opener.
    let golem = record(&region, ONE).unwrap();
    let mut other = build();
    other.set_container(
        source,
        region.container_at(source).unwrap(),
        &mut TickOutput::default(),
    );
    assert_eq!(other.openers(source), 1);
    lay(&mut other, &golem);
    let mut taken_at = None;
    for _ in 0..100 {
        checked(&mut other, &TickInputs::default());
        if golem_hand(&other, ONE).is_some() {
            taken_at = Some(record(&other, ONE).unwrap().lived);
            break;
        }
    }
    // Its first tick at the chest was the one before `opened_at`; the 60th, 59 later.
    assert_eq!(taken_at, Some(opened_at + 59));
    assert_eq!(golem_hand(&other, ONE), Some(stack_of(items::DIRT, 16)));
    assert_eq!(held_in(&other, source), [(2, stack_of(items::DIRT, 4))]);
    // And let go of the lid, which shuts.
    idle(&mut other, 6);
    assert_eq!(other.openers(source), 0);
}

#[test]
fn a_panicking_nautilus_taken_into_another_region_goes_on_panicking() {
    let mut region = on_ticking_floor_with(&pool());
    let puffer = EntityId(44);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::NAUTILUS,
            [6.5, -60.0, 6.5],
            nautilus_state(&NautilusState::default()),
        ),
    );
    idle(&mut region, 2);
    region.hit_by_mob(
        puffer,
        entity_types::PUFFERFISH as u16,
        [9.5, -60.0, 6.5],
        ONE,
        1.0,
        (None, None),
        None,
        &mut TickOutput::default(),
    );
    let provider = crate::mobs::types::of(entity_types::NAUTILUS as u16)
        .brain()
        .unwrap();
    let panicking = |region: &Region| {
        crate::mobs::brain::memories_of(provider, &mob(region, ONE), 0).check(
            crate::mobs::brain::MemoryKind::IsPanicking,
            crate::mobs::brain::Status::Present,
        )
    };
    let mut started = false;
    for _ in 0..40 {
        checked(&mut region, &TickInputs::default());
        if panicking(&region) {
            started = true;
            break;
        }
    }
    assert!(started);
    idle(&mut region, 10);
    let (running, end, ()) = behaviour_part::<()>(&mob(&region, ONE), 0).expect("its panic");
    assert!(running);
    let nautilus = record(&region, ONE).unwrap();
    let mut other = on_ticking_floor_with(&pool());
    lay(&mut other, &nautilus);
    idle(&mut other, 5);
    assert!(panicking(&other));
    // The same run of its panic, to the same end.
    let (running, kept_end, ()) = behaviour_part::<()>(&mob(&other, ONE), 0).expect("its panic");
    assert!(running);
    assert_eq!(kept_end, end);
}
