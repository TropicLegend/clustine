//! The axolotl, the frog and the tadpole in a region as a player meets them without
//! their brains, from the brief's sections 7.4, 7.5 and 10.7.2: an egg makes each with
//! its draws, a water bucket takes an axolotl and a tadpole and their buckets pour them,
//! an axolotl breathes water and dries out on land, a tadpole grows into a frog of the
//! biome it is in, frogspawn schedules its hatching as it is placed and hatches
//! tadpoles, and a slime ball feeds a frog and a tadpole.

use clustine_blocks::data::{is_block, tick_id};
use clustine_data::{BlockState, attributes as ids, blocks, entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityId, Ticked};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::{RANDOM_SPAWN_BONUS, supplied};
use crate::mobs::types::animal;
use crate::mobs::types::axolotl;
use crate::mobs::types::frog::{self, Variant};
use crate::mobs::types::tadpole;
use crate::region::Random;

const ONE: EntityId = EntityId(40);

/// A mob of the type `kind` at `position` as `mob` has it, of its type's full health and
/// air.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    mob.body.vitals.health = supplied(kind, attribute(ids::MAX_HEALTH)).unwrap() as f32;
    if mob.body.vitals.air == 0 {
        mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    }
    Entity {
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

fn holding(region: &mut Region, stack: ItemStack) {
    checked(
        region,
        &moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)]),
    );
}

fn one(item: i32) -> ItemStack {
    ItemStack { item, count: 1 }
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

/// A strip of water one block deep from x = 1 to 4 by the player, on stone.
fn strip() -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for x in 1..=4 {
        for z in 0..=1 {
            placed.push((BlockPos::new(x, -62, z), blocks::STONE));
            placed.push((BlockPos::new(x, -61, z), blocks::WATER));
        }
    }
    placed
}

/// The yaw and pitch from the player's eyes at the spawn to `at`.
fn aim(at: [f64; 3]) -> (f32, f32) {
    let eye = [0.5, -60.0 + 1.62, 0.5];
    let (dx, dy, dz) = (at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]);
    let yaw = (-dx).atan2(dz).to_degrees() as f32;
    let pitch = (-dy).atan2((dx * dx + dz * dz).sqrt()).to_degrees() as f32;
    (yaw, pitch)
}

fn pour(region: &mut Region, at: [f64; 3]) -> TickOutput {
    let (yaw, pitch) = aim(at);
    checked(
        region,
        &moves(vec![numbered(
            1,
            PlayerInput::UseItem {
                rotation: (yaw, pitch),
                sneaking: false,
                sequence: 1,
            },
        )]),
    )
}

fn idle(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked(region, &TickInputs::default());
    }
}

/// `Mob.finalizeSpawn`'s draws after the type's, and what they set: the follow range's
/// bonus and the hand.
fn assert_mob_s_draws(made: &Mob, level: &mut Random, kind: i32) {
    let bonus = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
    let left = level.next_float() < 0.05;
    let follow = made
        .body
        .attributes
        .made(ids::FOLLOW_RANGE as u16)
        .expect("a bonus to the follow range");
    assert_eq!(
        follow.modifiers[0].amount.to_bits(),
        bonus.to_bits(),
        "{kind}"
    );
    assert_eq!(made.body.flags.left_handed, left, "{kind}");
}

#[test]
fn an_egg_makes_each_amphibian_with_its_attributes_and_its_type_s_draws_in_order() {
    // The floor's biome is the registry's first, the badlands: a warm frog's.
    for kind in [
        entity_types::AXOLOTL,
        entity_types::FROG,
        entity_types::TADPOLE,
    ] {
        assert!(crate::mobs::types::is_built(kind as u16), "{kind}");
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
        let health = supplied(kind as u16, attribute(ids::MAX_HEALTH)).unwrap() as f32;
        assert_eq!(made.body.vitals.health, health, "{kind}");
        match kind {
            entity_types::AXOLOTL => {
                let first = level.next_int(4) as i32;
                let second = level.next_int(4) as i32;
                let variant = [first, second][level.next_int(2) as usize];
                let own = axolotl::read(&made);
                assert_eq!(own.variant, variant);
                assert!(own.variant != axolotl::BLUE);
                assert!(!own.from_bucket && !own.playing_dead);
                assert_eq!(made.body.vitals.air, 6000);
                assert!(!animal::is_baby(&made));
            }
            entity_types::FROG => {
                assert_eq!(level.next_int(1), 0);
                let _long_jump = level.next_int(41);
                assert_eq!(frog::read(&made).variant, Variant::Warm);
                assert_eq!(made.body.vitals.air, 300);
            }
            _ => {
                assert_eq!(tadpole::read(&made.state).age(0), 0);
                assert!(!made.body.flags.persistent);
            }
        }
        assert_mob_s_draws(&made, &mut level, kind);
        // It is shown as made, and its first ambient sound heard where it has one.
        let heard: Vec<i32> = sounds_of(&output).into_iter().map(|(s, _)| s).collect();
        match kind {
            entity_types::AXOLOTL => assert!(heard.contains(&sounds::ENTITY_AXOLOTL_IDLE_AIR)),
            entity_types::FROG => assert!(heard.contains(&sounds::ENTITY_FROG_AMBIENT)),
            _ => assert!(heard.is_empty(), "{heard:?}"),
        }
    }
}

#[test]
fn a_water_bucket_takes_an_axolotl_and_its_bucket_pours_a_fresh_lucy_from_a_bucket() {
    let mut region = on_ticking_floor_with(&strip());
    let mut gold = Mob::fresh();
    axolotl::update(&mut gold, |own| own.variant = axolotl::GOLD);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::AXOLOTL, [2.5, -61.0, 0.5], gold),
    );
    holding(&mut region, one(items::WATER_BUCKET));
    let output = use_on(&mut region, ONE);
    assert!(record(&region, ONE).is_none(), "the axolotl is taken");
    assert!(sounds_of(&output).contains(&(sounds::ITEM_BUCKET_FILL_AXOLOTL, None)));
    // The creative player keeps the water bucket and is given the axolotl's.
    assert_eq!(held_count(&region, items::WATER_BUCKET), 1);
    assert_eq!(held_count(&region, items::AXOLOTL_BUCKET), 1);

    holding(&mut region, one(items::AXOLOTL_BUCKET));
    let output = pour(&mut region, [2.7, -61.95, 0.5]);
    let poured = all_of(&region, entity_types::AXOLOTL);
    let [poured] = &poured[..] else {
        panic!("one axolotl poured: {poured:?}");
    };
    assert_eq!(
        crate::physics::containing(poured.position),
        BlockPos::new(2, -61, 0)
    );
    let made = Mob::decode(&poured.data).unwrap();
    let own = axolotl::read(&made);
    // The bucket kept nothing of it: lucy, from a bucket, and no `finalizeSpawn` at all
    // (no colours drawn, no follow range's bonus, the right hand).
    assert_eq!(own.variant, axolotl::LUCY);
    assert!(own.from_bucket);
    assert!(
        made.body
            .attributes
            .made(ids::FOLLOW_RANGE as u16)
            .is_none()
    );
    assert!(!made.body.flags.left_handed);
    assert!(sounds_of(&output).contains(&(sounds::ITEM_BUCKET_EMPTY_AXOLOTL, Some(player(1)))));
    // Kept: one from a bucket does not despawn.
    let module = crate::mobs::types::of(poured.kind);
    assert!(!module.remove_when_far_away(poured.kind, &made, 0, 1.0e6));
}

#[test]
fn a_water_bucket_takes_a_tadpole_and_its_bucket_pours_a_newborn_drawn_as_an_egg_s() {
    let mut region = on_ticking_floor_with(&strip());
    let mut old = Mob::fresh();
    tadpole::update(&mut old, |own| {
        own.set_age(9000, 0);
    });
    lay(
        &mut region,
        &mob_at(ONE, entity_types::TADPOLE, [2.5, -61.0, 0.5], old),
    );
    holding(&mut region, one(items::WATER_BUCKET));
    let output = use_on(&mut region, ONE);
    assert!(record(&region, ONE).is_none(), "the tadpole is taken");
    assert!(sounds_of(&output).contains(&(sounds::ITEM_BUCKET_FILL_TADPOLE, None)));
    assert_eq!(held_count(&region, items::TADPOLE_BUCKET), 1);

    holding(&mut region, one(items::TADPOLE_BUCKET));
    let output = pour(&mut region, [2.7, -61.95, 0.5]);
    let poured = all_of(&region, entity_types::TADPOLE);
    let [poured] = &poured[..] else {
        panic!("one tadpole poured: {poured:?}");
    };
    let made = Mob::decode(&poured.data).unwrap();
    // A newborn, `Mob.finalizeSpawn`'s bonus drawn (its own `finalizeSpawn` is
    // `Mob`'s), and nothing marked from a bucket: its `setFromBucket` does nothing.
    assert_eq!(made.state, clustine_world::TypeState::default());
    let follow = made.body.attributes.made(ids::FOLLOW_RANGE as u16).unwrap();
    assert!(follow.has_modifier(RANDOM_SPAWN_BONUS));
    assert!(sounds_of(&output).contains(&(sounds::ITEM_BUCKET_EMPTY_TADPOLE, Some(player(1)))));
}

#[test]
fn an_axolotl_keeps_its_air_full_in_water_and_dries_out_on_land_by_two_at_a_time() {
    let mut region = on_ticking_floor_with(&pool());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::AXOLOTL, [7.5, -59.0, 7.5], Mob::fresh()),
    );
    idle(&mut region, 5);
    assert_eq!(mob(&region, ONE).body.vitals.air, 6000);

    // On land with 5 ticks of air left: one less a tick, and at -20 a drying out of 2,
    // its air back to nothing.
    let mut region = on_ticking_floor_with(&[]);
    let mut dry = Mob::fresh();
    dry.body.vitals.air = 5;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::AXOLOTL, [8.5, -60.0, 8.5], dry),
    );
    let mut hurt_at = None;
    for tick in 1..=30 {
        checked(&mut region, &TickInputs::default());
        let body = mob(&region, ONE);
        if tick < 25 {
            assert_eq!(body.body.vitals.air, 5 - tick, "{tick}");
        }
        if body.body.vitals.health < 14.0 && hurt_at.is_none() {
            hurt_at = Some(tick);
            assert_eq!(body.body.vitals.health, 12.0);
            assert_eq!(body.body.vitals.air, 0);
        }
    }
    assert_eq!(hurt_at, Some(25));
}

#[test]
fn a_tadpole_of_age_grows_into_a_frog_of_its_biome_s_variant_kept_for_good() {
    let mut region = on_ticking_floor_with(&pool());
    let mut young = Mob::fresh();
    tadpole::update(&mut young, |own| {
        own.set_age(tadpole::TICKS_TO_BE_FROG - 10, 0);
    });
    lay(
        &mut region,
        &mob_at(ONE, entity_types::TADPOLE, [7.5, -59.0, 7.5], young),
    );
    let mut grew = None;
    for tick in 1..=12 {
        let output = checked(&mut region, &TickInputs::default());
        if record(&region, ONE).is_none() {
            grew = Some((tick, output));
            break;
        }
        assert!(all_of(&region, entity_types::FROG).is_empty(), "{tick}");
    }
    // Its tenth tick takes its age to 24000.
    let (tick, output) = grew.expect("it grew up");
    assert_eq!(tick, 10);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_TADPOLE_GROW_UP, None)));
    let frogs = all_of(&region, entity_types::FROG);
    let [frog] = &frogs[..] else {
        panic!("one frog: {frogs:?}");
    };
    let made = Mob::decode(&frog.data).unwrap();
    // The badlands: warm.
    assert_eq!(frog::read(&made).variant, Variant::Warm);
    assert!(made.body.flags.persistent);
    // `Mob.finalizeSpawn`'s bonus is drawn for it.
    assert!(
        made.body
            .attributes
            .made(ids::FOLLOW_RANGE as u16)
            .is_some_and(|follow| follow.has_modifier(RANDOM_SPAWN_BONUS))
    );
    assert_eq!(
        made.body.vitals.health,
        supplied(frog.kind, attribute(ids::MAX_HEALTH)).unwrap() as f32
    );
    assert!((frog.position[0] - 7.5).abs() < 2.0 && (frog.position[2] - 7.5).abs() < 2.0);
}

#[test]
fn a_tadpole_at_rest_commits_nothing_for_its_growing() {
    let mut region = on_ticking_floor_with(&pool());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::TADPOLE, [7.5, -59.0, 7.5], Mob::fresh()),
    );
    idle(&mut region, 50);
    let made = mob(&region, ONE);
    // Its record keeps nothing of its age, which is its `lived`.
    assert_eq!(made.state, clustine_world::TypeState::default());
    let lived = record(&region, ONE).unwrap().lived;
    assert_eq!(tadpole::read(&made.state).age(lived), lived as i32);
}

#[test]
fn a_slime_ball_feeds_a_frog_into_love_and_a_tadpole_a_tenth_of_its_time() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::FROG, [2.5, -60.0, 0.5], Mob::fresh()),
    );
    holding(&mut region, one(items::SLIME_BALL));
    let output = use_on(&mut region, ONE);
    assert_eq!(entity_events(&output, ONE), [18]);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_FROG_EAT, None)));
    let lived = record(&region, ONE).unwrap().lived;
    assert!(animal::animal_of(&mob(&region, ONE).state).in_love(lived));

    let tadpole_key = EntityId(42);
    lay(
        &mut region,
        &mob_at(
            tadpole_key,
            entity_types::TADPOLE,
            [2.5, -60.0, 1.5],
            Mob::fresh(),
        ),
    );
    use_on(&mut region, tadpole_key);
    let lived = record(&region, tadpole_key).unwrap().lived;
    let age = tadpole::read(&mob(&region, tadpole_key).state).age(lived);
    // At its first moment: 23 999 or 24 000 left, a tenth of 1199 or 1200 s, 119 or
    // 120 s: 2380 or 2400 ticks more than its own.
    assert!(
        age == lived as i32 + 2380 || age == lived as i32 + 2400,
        "{age} {lived}"
    );
}

#[test]
fn an_axolotl_bred_with_a_bucket_of_tropical_fish_gives_back_a_water_bucket() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::AXOLOTL, [2.5, -60.0, 0.5], Mob::fresh()),
    );
    holding(&mut region, one(items::TROPICAL_FISH_BUCKET));
    let output = use_on(&mut region, ONE);
    assert_eq!(entity_events(&output, ONE), [18]);
    // The creative player keeps the fish and is given a water bucket, once.
    assert_eq!(held_count(&region, items::TROPICAL_FISH_BUCKET), 1);
    assert_eq!(held_count(&region, items::WATER_BUCKET), 1);
}

#[test]
fn frogspawn_placed_on_water_schedules_its_hatching_and_hatches_two_to_five_tadpoles() {
    let at = BlockPos::new(6, -56, 6);
    let mut region = on_ticking_floor_with(&pool());
    let mut output = TickOutput::default();
    assert!(region.set_block_updated(
        at,
        blocks::FROGSPAWN,
        3,
        super::super::updates::UPDATE_LIMIT,
        &mut output
    ));
    let of = Ticked::Block(tick_id(blocks::FROGSPAWN));
    assert!(region.has_scheduled_tick(at, of));
    let kept: Vec<_> = region
        .ticks_to_save(at.chunk())
        .into_iter()
        .filter(|tick| tick.of == of)
        .collect();
    let [tick] = &kept[..] else {
        panic!("one tick: {kept:?}");
    };
    assert!((3600..=11_999).contains(&tick.delay), "{:?}", tick.delay);

    // At its tick, it is gone without drops, its sound at its block, and its tadpoles
    // half a block below it, kept for good.
    let mut output = TickOutput::default();
    assert!(region.frogspawn_tick(at, blocks::FROGSPAWN, &mut output));
    assert!(!is_block(region.block(at).unwrap(), blocks::FROGSPAWN));
    assert!(sounds_of(&output).contains(&(sounds::BLOCK_FROGSPAWN_HATCH, None)));
    let tadpoles = all_of(&region, entity_types::TADPOLE);
    assert!((2..=5).contains(&tadpoles.len()), "{}", tadpoles.len());
    for tadpole in &tadpoles {
        let [x, y, z] = tadpole.position;
        assert!(
            (6.2..=6.8).contains(&x) && (6.2..=6.8).contains(&z),
            "{x} {z}"
        );
        assert_eq!(y, -56.5);
        assert!((1.0..=360.0).contains(&tadpole.yaw));
        let made = Mob::decode(&tadpole.data).unwrap();
        assert!(made.body.flags.persistent);
        assert_eq!(tadpole::read(&made.state).age(0), 0);
    }
    // No item dropped.
    assert!(all_of(&region, entity_types::ITEM).is_empty());
}

#[test]
fn frogspawn_that_cannot_stay_goes_at_its_tick_without_hatching() {
    let at = BlockPos::new(6, -60, 6);
    let mut region = on_ticking_floor_with(&[(at, blocks::FROGSPAWN)]);
    let mut output = TickOutput::default();
    // On stone, with no water under it.
    assert!(region.frogspawn_tick(at, blocks::FROGSPAWN, &mut output));
    assert_eq!(region.block(at), Some(blocks::AIR));
    assert!(all_of(&region, entity_types::TADPOLE).is_empty());
    assert!(all_of(&region, entity_types::ITEM).is_empty());
    // Not frogspawn: nothing of it.
    assert!(!region.frogspawn_tick(at, blocks::STONE, &mut output));
}

#[test]
fn a_frog_takes_five_less_of_a_fall_s_hurt_than_another_mob() {
    // From seven and a half blocks: four of hurt for an axolotl, none for a frog.
    let mut region = on_ticking_floor_with(&[]);
    let frog_key = EntityId(42);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::AXOLOTL, [8.5, -52.5, 8.5], Mob::fresh()),
    );
    lay(
        &mut region,
        &mob_at(
            frog_key,
            entity_types::FROG,
            [5.5, -52.5, 5.5],
            Mob::fresh(),
        ),
    );
    idle(&mut region, 40);
    assert_eq!(record(&region, ONE).unwrap().position[1], -60.0);
    assert_eq!(record(&region, frog_key).unwrap().position[1], -60.0);
    assert_eq!(mob(&region, ONE).body.vitals.health, 10.0);
    assert_eq!(mob(&region, frog_key).body.vitals.health, 10.0);
}
