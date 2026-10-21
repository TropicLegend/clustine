//! The axolotl, the frog and the tadpole in a region as a player meets them, from the
//! brief's sections 7.4, 7.5 and 10.7.2: an egg makes each with its draws, a water bucket
//! takes an axolotl and a tadpole and their buckets pour them, an axolotl breathes water
//! and dries out on land, a tadpole grows into a frog of the biome it is in, frogspawn
//! schedules its hatching as it is placed and hatches tadpoles, and a slime ball feeds a
//! frog and a tadpole; and with their brains, an axolotl plays dead, hunts and is
//! tempted, a frog long-jumps, croaks, breeds and lays frogspawn, a tadpole panics, each
//! is tempted across a line, breeds across it by the region's protocol and jumps over it
//! with its brain whole, and a restore goes on mid-jump, mid-panic and playing dead.

use clustine_blocks::data::{is_block, tick_id};
use clustine_data::{BlockState, attributes as ids, blocks, entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityId, Ticked};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::{RANDOM_SPAWN_BONUS, supplied};
use crate::mobs::brain::{self, Activity, BrainState, Memories, MemoryKind, Status, Value};
use crate::mobs::types::animal;
use crate::mobs::types::axolotl;
use crate::mobs::types::frog::{self, Pose, Variant};
use crate::mobs::types::tadpole;
use crate::mobs::types::{axolotl_ai, frog_ai, tadpole_ai};
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

// ------------------------------------------------------------------------------------
// Their brains, as a player meets them (`docs/groundwork/n7j-plan.md`, sections 1 and
// 4.3): each test waits for the state it is about, within a bound far above what the
// game's rules take.

/// The kept memories of the region's mob `key` of a type with a brain, as code outside
/// its tick reads them.
fn memories_of(region: &Region, key: EntityId) -> Memories {
    let found = record(region, key).expect("the mob is here");
    let provider = crate::mobs::types::of(found.kind).brain().expect("a brain");
    let made = Mob::decode(&found.data).unwrap();
    brain::memories_of(provider, &made, found.lived)
}

/// The brain's own state of the region's mob `key`.
fn brain_state(region: &Region, key: EntityId) -> BrainState {
    crate::mobs::part::decode(&mob(region, key).mind.brain).unwrap_or_default()
}

/// A sensed memory of the region's mob `key`, as the region keeps it.
fn sensed(region: &Region, key: EntityId, kind: MemoryKind) -> Option<Value> {
    match region
        .entity_memory
        .get(&key)?
        .sensed
        .as_ref()?
        .get(&kind)?
    {
        brain::Slot::Kept { value, .. } => Some(value.clone()),
        brain::Slot::Count(_) => None,
    }
}

fn attack(region: &mut Region, key: EntityId) -> TickOutput {
    checked(
        region,
        &moves(vec![numbered(1, PlayerInput::Attack { entity: key })]),
    )
}

/// A ring of stone two blocks high about the block at `x`, `z` on the floor: a frog in it
/// cannot walk out, and so long-jumps (`LongJumpToRandomPos` jumps only where no path of
/// 8 reaches).
fn ring(x: i32, z: i32) -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            for y in [-60, -59] {
                placed.push((BlockPos::new(x + dx, y, z + dz), blocks::STONE));
            }
        }
    }
    placed
}

/// A frog whose long jump cools for `ticks`, so that it idles.
fn cooled_frog(ticks: i32) -> Mob {
    let mut made = Mob::fresh();
    frog_ai::set_long_jump_cooldown(&mut made, ticks);
    made
}

/// The brief's 7.4.7: hit by a player in water, one time in nine it plays dead (its
/// first draw a nought, its second below the hurt of 1): its flag shows at once, for
/// 199 ticks, it plays dead and comes out of it. Each key's stream decides; the first key
/// whose hit plays dead is taken.
#[test]
fn an_axolotl_hit_in_water_may_play_dead_and_shows_it_for_199_ticks() {
    let mut played = None;
    for n in 0..60 {
        let key = EntityId(40 + 2 * n);
        let mut region = on_ticking_floor_with(&strip());
        lay(
            &mut region,
            &mob_at(key, entity_types::AXOLOTL, [2.5, -61.0, 0.5], Mob::fresh()),
        );
        idle(&mut region, 2);
        attack(&mut region, key);
        let showing = |region: &Region| {
            record(region, key).is_some() && axolotl::read(&mob(region, key)).playing_dead
        };
        // Shown at the axolotl's first tick after the hit, in its tick or the next.
        if showing(&region) {
            played = Some((region, key));
            break;
        }
        checked(&mut region, &TickInputs::default());
        if showing(&region) {
            played = Some((region, key));
            break;
        }
    }
    let (mut region, key) = played.expect("some key's hit plays dead");
    assert_eq!(brain_state(&region, key).active, Some(Activity::PlayDead));
    let mut shown = 1;
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        if !axolotl::read(&mob(&region, key)).playing_dead {
            break;
        }
        shown += 1;
    }
    assert_eq!(shown, 199);
    // Its count is gone in the tick after, and so is its playing dead.
    checked(&mut region, &TickInputs::default());
    assert!(memories_of(&region, key).check(MemoryKind::PlayDeadTicks, Status::Absent));
    assert_ne!(brain_state(&region, key).active, Some(Activity::PlayDead));
}

/// The brief's 7.4.8: in water it hunts a tropical fish within 8 blocks, hits it until it
/// dies (2 a hit, 20 ticks apart), and then rests from hunting for 2400 ticks.
#[test]
fn an_axolotl_hunts_a_tropical_fish_in_water_and_then_rests_from_hunting() {
    let mut region = on_ticking_floor_with(&pool());
    let fish = EntityId(42);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::AXOLOTL, [6.5, -59.0, 6.5], Mob::fresh()),
    );
    lay(
        &mut region,
        &mob_at(
            fish,
            entity_types::TROPICAL_FISH,
            [9.5, -59.0, 9.5],
            Mob::fresh(),
        ),
    );
    let mut fought = false;
    let mut rested = None;
    for _ in 0..2000 {
        checked(&mut region, &TickInputs::default());
        fought |= brain_state(&region, ONE).active == Some(Activity::Fight);
        let memories = memories_of(&region, ONE);
        if memories.check(MemoryKind::HasHuntingCooldown, Status::Present) {
            rested = Some(memories.expires_in(MemoryKind::HasHuntingCooldown));
            break;
        }
    }
    assert!(fought, "it hunted");
    // The fish is dead, or dying, of its hits.
    assert!(
        record(&region, fish).is_none_or(|_| mob(&region, fish).body.vitals.health <= 0.0),
        "the fish is dead"
    );
    assert_eq!(rested, Some(2400));
}

/// A player holding a bucket of tropical fish within 10 blocks tempts an axolotl.
#[test]
fn an_axolotl_is_tempted_by_a_bucket_of_tropical_fish() {
    let mut region = on_ticking_floor_with(&strip());
    holding(&mut region, one(items::TROPICAL_FISH_BUCKET));
    lay(
        &mut region,
        &mob_at(ONE, entity_types::AXOLOTL, [4.5, -61.0, 1.5], Mob::fresh()),
    );
    let player_key = region.players[&player(1)].entity_id;
    let tempted = (0..40).any(|_| {
        checked(&mut region, &TickInputs::default());
        sensed(&region, ONE, MemoryKind::TemptingPlayer) == Some(Value::Entity(player_key))
    });
    assert!(tempted);
}

/// The brief's 7.3.16 with the frog's arguments: walled in, with its jump cooled, it
/// long-jumps (`LONG_JUMPING`, its jump's sound), lands within four blocks with its
/// landing's sound at twice the volume, and cools for 100 to 140 ticks.
#[test]
fn a_frog_long_jumps_with_its_pose_and_sounds_and_cools_down_after() {
    let mut region = on_ticking_floor_with(&ring(8, 8));
    lay(
        &mut region,
        &mob_at(ONE, entity_types::FROG, [8.5, -60.0, 8.5], Mob::fresh()),
    );
    let mut took_off = None;
    let mut heard_jump = false;
    for _ in 0..1000 {
        let output = checked(&mut region, &TickInputs::default());
        heard_jump |= sounds_of(&output).contains(&(sounds::ENTITY_FROG_LONG_JUMP, None));
        if frog::read(&mob(&region, ONE)).pose == Pose::LongJumping {
            took_off = Some(record(&region, ONE).unwrap().position);
            break;
        }
    }
    let from = took_off.expect("it jumped");
    assert!(heard_jump, "its jump is heard");
    // Landed: its jump's memory gone (a croak that outlived its activity may stand it up
    // before, as the game's `Croak.stop` does).
    let mut landed = None;
    for _ in 0..100 {
        let output = checked(&mut region, &TickInputs::default());
        if memories_of(&region, ONE).check(MemoryKind::LongJumpMidJump, Status::Absent) {
            landed = Some((record(&region, ONE).unwrap().position, output));
            break;
        }
    }
    let (to, output) = landed.expect("it landed");
    assert_eq!(frog::read(&mob(&region, ONE)).pose, Pose::Standing);
    let across = ((to[0] - from[0]).powi(2) + (to[2] - from[2]).powi(2)).sqrt();
    assert!(across > 0.5 && across < 6.0, "{across}");
    let landing: Vec<f32> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, volume, .. } if *sound == sounds::ENTITY_FROG_STEP => {
                Some(*volume)
            }
            _ => None,
        })
        .collect();
    assert!(landing.contains(&2.0), "{landing:?}");
    let cooldown = memories_of(&region, ONE).int(MemoryKind::LongJumpCoolingDown);
    assert!(
        cooldown.is_some_and(|ticks| (99..=140).contains(&ticks)),
        "{cooldown:?}"
    );
}

/// The brief's 7.5.6: an idle frog croaks, its pose `CROAKING` for 60 ticks.
#[test]
fn an_idle_frog_croaks_for_three_seconds() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::FROG,
            [8.5, -60.0, 8.5],
            cooled_frog(1 << 20),
        ),
    );
    let croaking = |region: &Region| frog::read(&mob(region, ONE)).pose == Pose::Croaking;
    let began = (0..3000).any(|_| {
        checked(&mut region, &TickInputs::default());
        croaking(&region)
    });
    assert!(began, "it croaked");
    let mut held = 1;
    while held < 200 {
        checked(&mut region, &TickInputs::default());
        if !croaking(&region) {
            break;
        }
        held += 1;
    }
    assert_eq!(held, 60);
}

/// The brief's 7.5.7: two frogs in love breed; one is pregnant, goes to land beside
/// water and lays frogspawn on it, which schedules its hatching.
#[test]
fn two_frogs_in_love_breed_and_the_pregnant_one_lays_frogspawn_on_water() {
    let mut region = on_ticking_floor_with(&pool());
    let two = EntityId(42);
    for (key, z) in [(ONE, 6.5), (two, 8.5)] {
        let mut made = cooled_frog(1 << 20);
        animal::update(&mut made, |shared| shared.set_in_love(0));
        lay(
            &mut region,
            &mob_at(key, entity_types::FROG, [3.5, -56.0, z], made),
        );
    }
    let mut pregnant = None;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        pregnant = [ONE, two]
            .into_iter()
            .find(|key| memories_of(&region, *key).check(MemoryKind::IsPregnant, Status::Present));
        if pregnant.is_some() {
            break;
        }
    }
    let mother = pregnant.expect("one of them is pregnant");
    assert_eq!(
        brain_state(&region, mother).active,
        Some(Activity::LaySpawn)
    );
    let spawn_at = |region: &Region| {
        (3..=12).find_map(|x| {
            (3..=12).find_map(|z| {
                let at = BlockPos::new(x, -56, z);
                is_block(region.block(at)?, blocks::FROGSPAWN).then_some(at)
            })
        })
    };
    let mut laid = None;
    for _ in 0..6000 {
        checked(&mut region, &TickInputs::default());
        if let Some(at) = spawn_at(&region) {
            laid = Some(at);
            break;
        }
    }
    let at = laid.expect("frogspawn was laid");
    // On water, its hatching scheduled, and the mother no longer pregnant.
    assert!(is_block(
        region.block(BlockPos::new(at.x, at.y - 1, at.z)).unwrap(),
        blocks::WATER
    ));
    assert!(region.has_scheduled_tick(at, Ticked::Block(tick_id(blocks::FROGSPAWN))));
    assert!(memories_of(&region, mother).check(MemoryKind::IsPregnant, Status::Absent));
}

/// A tadpole hit by a player panics in its brain (`AnimalPanic`, `is_panicking`).
#[test]
fn a_tadpole_hit_by_a_player_panics() {
    let mut region = on_ticking_floor_with(&strip());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::TADPOLE, [2.5, -61.0, 0.5], Mob::fresh()),
    );
    idle(&mut region, 2);
    attack(&mut region, ONE);
    let panicked = (0..40).any(|_| {
        checked(&mut region, &TickInputs::default());
        record(&region, ONE).is_some()
            && memories_of(&region, ONE).check(MemoryKind::IsPanicking, Status::Present)
    });
    assert!(panicked);
}

#[test]
fn a_tick_of_amphibians_with_their_brains_is_a_function_of_its_inputs() {
    let run = || {
        let mut region = on_ticking_floor_with(&pool());
        lay(
            &mut region,
            &mob_at(ONE, entity_types::AXOLOTL, [5.5, -59.0, 5.5], Mob::fresh()),
        );
        lay(
            &mut region,
            &mob_at(
                EntityId(42),
                entity_types::FROG,
                [3.5, -56.0, 7.5],
                Mob::fresh(),
            ),
        );
        lay(
            &mut region,
            &mob_at(
                EntityId(44),
                entity_types::TADPOLE,
                [9.5, -59.0, 9.5],
                Mob::fresh(),
            ),
        );
        lay(
            &mut region,
            &mob_at(
                EntityId(46),
                entity_types::TROPICAL_FISH,
                [8.5, -58.0, 6.5],
                Mob::fresh(),
            ),
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
// Across a pinned line (`docs/groundwork/n7j-plan.md`, sections 3 and 4.3).

/// A region of the land west of chunk x = 2, its chunks x of -1 to 1 loaded with a
/// floor, and the chunks of x = 2 another's, copied, with `beyond` in the one at z = 0.
fn at_a_line(own: &[Entity], beyond: &[Entity]) -> Region {
    at_a_line_with(&[], own, beyond)
}

/// [`at_a_line`] with `blocks` set in the region's chunks and the copies.
fn at_a_line_with(blocks: &[(BlockPos, BlockState)], own: &[Entity], beyond: &[Entity]) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let chunks = around
        .iter()
        .map(|position| {
            let mut chunk = floor();
            for (at, state) in blocks {
                if at.chunk() == *position {
                    let (x, z) = (at.x.rem_euclid(16), at.z.rem_euclid(16));
                    chunk.set(x as usize, at.y, z as usize, *state);
                }
            }
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
            for (at, state) in blocks {
                if at.chunk() == *position {
                    let (x, z) = (at.x.rem_euclid(16), at.z.rem_euclid(16));
                    base.set(x as usize, at.y, z as usize, *state);
                }
            }
            if position.z == 0 {
                base.set_entities(beyond.to_vec());
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

/// A player of the other region at `x` in the copied chunks, holding `held`.
fn face_holding(x: f64, held: i32) -> crate::api::FaceChange {
    let mut face = crate::api::PlayerFace::of(
        &crate::api::Pose::at(Vec3::new(x, -60.0, 8.5)),
        false,
        false,
        &Default::default(),
        0,
        None,
    );
    face.main_hand = Some(one(held));
    crate::api::FaceChange::Face {
        player: player(9),
        entity: EntityId(900),
        hops: 0,
        seq: crate::api::FaceSeq(1, 0),
        face,
    }
}

/// Each is tempted by a player of the other region holding its food, whom its tempting
/// sensor sees in the faces of the copied chunks; the frog follows them to the line.
#[test]
fn each_is_tempted_by_a_player_across_the_line() {
    for (kind, food) in [
        (entity_types::FROG, items::SLIME_BALL),
        (entity_types::TADPOLE, items::SLIME_BALL),
        (entity_types::AXOLOTL, items::TROPICAL_FISH_BUCKET),
    ] {
        let mut region = at_a_line(&[mob_at(ONE, kind, [27.5, -60.0, 8.5], Mob::fresh())], &[]);
        checked(
            &mut region,
            &TickInputs {
                faces: vec![face_holding(34.5, food)],
                ..TickInputs::default()
            },
        );
        let tempted = (0..40).any(|_| {
            checked(&mut region, &TickInputs::default());
            sensed(&region, ONE, MemoryKind::TemptingPlayer) == Some(Value::Entity(EntityId(900)))
        });
        assert!(tempted, "{kind}");
        if kind == entity_types::FROG {
            let followed = (0..400).any(|_| {
                checked(&mut region, &TickInputs::default());
                record(&region, ONE).is_none_or(|frog| frog.position[0] > 30.0)
            });
            assert!(followed, "it goes to them");
        }
    }
}

/// A request to breed from a frog across the line, which the frog here answers: it is
/// the game's `this`, and so the one made pregnant, and it goes to lay.
#[test]
fn a_frog_bred_by_a_request_from_across_the_line_is_the_pregnant_one() {
    let asker = EntityId(1 << 34);
    let mut beyond = cooled_frog(1 << 20);
    animal::update(&mut beyond, |shared| shared.set_in_love(0));
    let mut here = cooled_frog(1 << 20);
    animal::update(&mut here, |shared| shared.set_in_love(0));
    let mut region = at_a_line(
        &[mob_at(ONE, entity_types::FROG, [30.5, -60.0, 8.5], here)],
        &[mob_at(
            asker,
            entity_types::FROG,
            [33.5, -60.0, 8.5],
            beyond,
        )],
    );
    let request = TickInputs {
        effects: vec![clustine_world::FiledEffect {
            id: clustine_world::EffectId {
                sequence: 9,
                index: 0,
            },
            effect: clustine_world::Effect {
                at: BlockPos::new(30, -60, 8),
                what: clustine_world::Effected::OnEntity {
                    key: ONE,
                    by: clustine_world::EntityUse::BreedAsk { asker, number: 1 },
                },
            },
            due: clustine_world::Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    region.tick(&request);
    // No child: the frog is pregnant, taken into its brain at its next tick.
    assert_eq!(all_of(&region, entity_types::FROG).len(), 1);
    checked(&mut region, &TickInputs::default());
    assert!(memories_of(&region, ONE).check(MemoryKind::IsPregnant, Status::Present));
    assert_eq!(brain_state(&region, ONE).active, Some(Activity::LaySpawn));
}

/// A frog that long-jumps over the line is sent mid-jump, its brain whole in its record:
/// its jump's memory, its pose and the motion of its flight; taken in, it lands as it
/// would have, with its landing's sound and its cooldown. Each key's stream decides where
/// it jumps; the first key whose frog jumps over is taken.
#[test]
fn a_frog_that_long_jumps_over_the_line_arrives_mid_jump_with_its_brain_whole() {
    let mut sent = None;
    'keys: for n in 0..40 {
        let key = EntityId(40 + 2 * n);
        // Walled in at the last block of the land, so that each jump eastwards, onto the
        // wall or beyond it, lands across the line.
        let mut region = at_a_line_with(
            &ring(31, 8),
            &[mob_at(
                key,
                entity_types::FROG,
                [31.5, -60.0, 8.5],
                Mob::fresh(),
            )],
            &[],
        );
        let mut jumped = false;
        for _ in 0..600 {
            let output = checked(&mut region, &TickInputs::default());
            if let Some((_, gone)) = output.arrivals.iter().find(|(_, gone)| gone.key == key) {
                sent = Some((gone.clone(), region.clock.game_time));
                break 'keys;
            }
            let Some(frog) = record(&region, key) else {
                continue 'keys;
            };
            let pose = frog::read(&Mob::decode(&frog.data).unwrap()).pose;
            if pose == Pose::LongJumping {
                jumped = true;
            } else if jumped {
                // It landed on this side: the next key.
                continue 'keys;
            }
        }
    }
    let (gone, sent_at) = sent.expect("some key's frog jumped over the line");
    let made = Mob::decode(&gone.data).unwrap();
    assert_eq!(frog::read(&made).pose, Pose::LongJumping);
    let memories = brain::memories_of(&frog_ai::PROVIDER, &made, gone.lived);
    assert!(memories.check(MemoryKind::LongJumpMidJump, Status::Present));
    assert!(gone.motion[0] > 0.0, "{:?}", gone.motion);
    // Taken in by a region that ticks it, as the same record two chunks to the west (the
    // tests' regions tick only their home chunk), at the world's next game time, it flies
    // on and lands.
    let mut region = on_ticking_floor_with(&[]);
    region.clock.game_time = sent_at;
    let mut arriving = gone.clone();
    arriving.position[0] -= 32.0;
    let taken = TickInputs {
        arrivals: vec![clustine_world::Arrival {
            id: clustine_world::EffectId {
                sequence: 7,
                index: 0,
            },
            chunk: arriving.chunk(),
            entity: arriving.clone(),
            due: clustine_world::Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    let mut output = checked(&mut region, &taken);
    let mut heard = false;
    for _ in 0..100 {
        heard |= output.events.iter().any(|event| {
            matches!(event, RegionEvent::Sound { sound, volume, .. }
                if *sound == sounds::ENTITY_FROG_STEP && *volume == 2.0)
        });
        if memories_of(&region, gone.key).check(MemoryKind::LongJumpMidJump, Status::Absent) {
            break;
        }
        output = checked(&mut region, &TickInputs::default());
    }
    let landed = record(&region, gone.key).unwrap();
    assert!(landed.on_ground);
    assert_eq!(frog::read(&mob(&region, gone.key)).pose, Pose::Standing);
    assert!(heard, "it lands with its sound");
    let after = memories_of(&region, gone.key);
    assert!(after.check(MemoryKind::LongJumpMidJump, Status::Absent));
    assert!(after.check(MemoryKind::LongJumpCoolingDown, Status::Present));
}

/// A restore of a region in the middle of a frog's long jump, a tadpole's panic and an
/// axolotl's playing dead goes on as the region does, tick for tick: their brains are in
/// their records.
#[test]
fn a_restore_mid_jump_mid_panic_and_playing_dead_goes_on_as_the_region_does() {
    let mut walled = pool();
    walled.extend(ring(14, 14));
    let mut original = on_ticking_floor_with(&walled);
    let (frog_key, tadpole_key, axolotl_key) = (ONE, EntityId(42), EntityId(44));
    lay(
        &mut original,
        &mob_at(
            frog_key,
            entity_types::FROG,
            [14.5, -60.0, 14.5],
            Mob::fresh(),
        ),
    );
    let mut panicking = Mob::fresh();
    {
        let provider = &tadpole_ai::PROVIDER;
        let mut memories = brain::memories_of(provider, &panicking, 0);
        memories.set(MemoryKind::IsPanicking, Value::Bool(true));
        brain::write_memories(provider, &mut panicking, memories);
    }
    let mut playing = Mob::fresh();
    axolotl_ai::set_play_dead_ticks(&mut playing, 0, 200);
    lay(
        &mut original,
        &mob_at(
            tadpole_key,
            entity_types::TADPOLE,
            [9.5, -59.0, 9.5],
            panicking,
        ),
    );
    lay(
        &mut original,
        &mob_at(
            axolotl_key,
            entity_types::AXOLOTL,
            [6.5, -59.0, 6.5],
            playing,
        ),
    );
    // Until the frog is in the air.
    let jumping = (0..600).any(|_| {
        checked(&mut original, &TickInputs::default());
        frog::read(&mob(&original, frog_key)).pose == Pose::LongJumping
    });
    assert!(jumping, "the frog jumped");
    let mut copy = super::scheduled_ticks::restored(&original);
    for tick in 0..60 {
        let (a, b) = (
            checked(&mut original, &TickInputs::default()),
            checked(&mut copy, &TickInputs::default()),
        );
        assert_eq!(a.entity_changes, b.entity_changes, "tick {tick}");
        for key in [frog_key, tadpole_key, axolotl_key] {
            assert_eq!(record(&original, key), record(&copy, key), "tick {tick}");
        }
    }
}

/// Two axolotls in love across the line: only the higher key asks, by the region's
/// protocol (`AnimalMakeLove`'s breeding with a partner a copy shows); a request from
/// across is answered with a child made here, at the partner the region holds.
#[test]
fn axolotls_in_love_across_the_line_breed_by_the_region_s_protocol() {
    let in_love = || {
        let mut made = Mob::fresh();
        animal::update(&mut made, |shared| shared.set_in_love(0));
        made
    };
    // The axolotl here has the higher key: it asks its partner's region.
    let partner = EntityId(5);
    let mut region = at_a_line(
        &[mob_at(
            ONE,
            entity_types::AXOLOTL,
            [30.5, -60.0, 8.5],
            in_love(),
        )],
        &[mob_at(
            partner,
            entity_types::AXOLOTL,
            [32.5, -60.0, 8.5],
            in_love(),
        )],
    );
    let asked = (0..400).find_map(|_| {
        let output = checked(&mut region, &TickInputs::default());
        output.effects.iter().find_map(|effect| match effect.what {
            clustine_world::Effected::OnEntity {
                key,
                by: clustine_world::EntityUse::BreedAsk { asker, .. },
            } if key == partner => Some(asker),
            _ => None,
        })
    });
    assert_eq!(asked, Some(ONE));

    // Asked from across the line by a higher key: the child is made here.
    let asker = EntityId(1 << 34);
    let mut region = at_a_line(
        &[mob_at(
            ONE,
            entity_types::AXOLOTL,
            [30.5, -60.0, 8.5],
            in_love(),
        )],
        &[mob_at(
            asker,
            entity_types::AXOLOTL,
            [33.5, -60.0, 8.5],
            in_love(),
        )],
    );
    let request = TickInputs {
        effects: vec![clustine_world::FiledEffect {
            id: clustine_world::EffectId {
                sequence: 9,
                index: 0,
            },
            effect: clustine_world::Effect {
                at: BlockPos::new(30, -60, 8),
                what: clustine_world::Effected::OnEntity {
                    key: ONE,
                    by: clustine_world::EntityUse::BreedAsk { asker, number: 1 },
                },
            },
            due: clustine_world::Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    region.tick(&request);
    let axolotls = all_of(&region, entity_types::AXOLOTL);
    let child = axolotls
        .iter()
        .find(|axolotl| axolotl.key != ONE)
        .expect("a child");
    assert!(animal::is_baby(&Mob::decode(&child.data).unwrap()));
}
