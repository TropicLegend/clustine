//! The keepers in a region, from the brief's sections 7.12 and 7.13 as a player meets
//! them before their brains come: eggs make each with its defaults and its draws in the
//! game's order, a nautilus is tamed, kept near where it was left and dries out on land,
//! a copper golem is built from a pumpkin on copper, weathers, is waxed and scraped,
//! turns into a statue whose pose a click turns, and counts as an opener of the chest it
//! has open.

use clustine_data::{BlockState, entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityId};

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
            set_slot(1, 0, Some(ItemStack { item, count: 1 })),
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
        assert_eq!(own.attack_target_cooldown, Some(cooldown), "{kind}");
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
    // Its own stream: its UUID, its constructor's yaw, then its first transport cooldown.
    let mut own = Random::of_entity_aside(region.config.seed, key, 0, region.tick);
    own.next_u64();
    own.next_u64();
    own.next_float();
    let cooldown = 60 + own.next_int(40) as i32;
    let golem = copper_golem::state_of(&made);
    assert_eq!(golem.transport_cooldown, Some(cooldown));
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
        let fish = Some(ItemStack {
            item: items::PUFFERFISH,
            count: 1,
        });
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
    let honeycomb = Some(ItemStack {
        item: items::HONEYCOMB,
        count: 1,
    });
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
    let axe = Some(ItemStack {
        item: items::IRON_AXE,
        count: 1,
    });
    let (_, output) = used_now(&mut region, ONE, axe);
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
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::COPPER_GOLEM,
            [5.5, -60.0, 4.5],
            copper_golem::write(&own),
        ),
    );
    // The golem's `startOpen`, as its brain will at its interaction's first tick.
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
