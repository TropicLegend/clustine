//! A mob's body in a region, from the brief's sections 1.3 to 1.13: what its tick draws
//! and commits at rest, its falls, lava, a player's hit, its death and loot, the orbs
//! of its experience, spawn eggs, despawning, a lead, and that a tick is a function of
//! its inputs.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{
    Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData, EntityField, EntityId,
    EntityUse, Entry, FiledEffect, LeashHolder,
};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::body::attributes::RANDOM_SPAWN_BONUS;
use crate::mobs::part::body_ids;

const COW_KEY: EntityId = EntityId(40);

/// A mob of the type `kind` at `position`, with its type's full health and air.
fn mob_of(key: EntityId, kind: i32, position: [f64; 3], on_ground: bool) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

fn cow(position: [f64; 3], on_ground: bool) -> Entity {
    mob_of(COW_KEY, entity_types::COW, position, on_ground)
}

/// A mob of a type no part has built yet (a goat, until its brain), which is a body of the
/// game's defaults: what the body's own rules are seen on, without a mind's moves.
fn plain(position: [f64; 3], on_ground: bool) -> Entity {
    mob_of(COW_KEY, entity_types::GOAT, position, on_ground)
}

/// Lays `entity` in its loaded chunk, as a load would have it.
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

/// The fields each commit of `key` in `output` set.
fn sets(output: &TickOutput, key: EntityId) -> Vec<Vec<EntityField>> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Set(changed, fields) if *changed == key => Some(fields.clone()),
            _ => None,
        })
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

fn items_lying(region: &Region) -> Vec<ItemStack> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some(stack),
            _ => None,
        })
        .collect()
}

fn orbs(region: &Region) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| matches!(entity.data, EntityData::ExperienceOrb { .. }))
        .cloned()
        .collect()
}

fn mobs_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

/// Player 1 holding `stack` in the hotbar's first slot.
fn holding(region: &mut Region, stack: ItemStack) {
    checked(
        region,
        &moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)]),
    );
}

fn attack(target: EntityId) -> TickInputs {
    moves(vec![numbered(1, PlayerInput::Attack { entity: target })])
}

/// The entity of player 1.
fn player_entity(region: &Region) -> EntityId {
    region.players.get(&player(1)).unwrap().entity_id
}

/// The resting motion of a mob on the ground.
const RESTING: f64 = -0.078_400_001_525_878_9;

#[test]
fn a_mob_at_rest_draws_its_ambient_chance_each_tick_and_commits_only_its_resets() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &plain([4.5, -60.0, 4.5], true));
    let mut resets = 0;
    for _ in 0..200 {
        let before = mob(&region, COW_KEY);
        let output = checked(&mut region, &TickInputs::default());
        let after = mob(&region, COW_KEY);
        let fields = sets(&output, COW_KEY);
        if after == before {
            assert!(fields.is_empty(), "{fields:?}");
            continue;
        }
        // The draw came under the counter (`nextInt(1000) < ambientSoundTime`): the
        // counter starts again 80 below nought, and nothing else changed.
        resets += 1;
        let lived = record(&region, COW_KEY).unwrap().lived;
        assert_eq!(after.body.idle.ambient_since, lived + 80, "{fields:?}");
        let mut idle_only = before.clone();
        idle_only.body.idle = after.body.idle.clone();
        assert_eq!(idle_only, after);
        let [one] = &fields[..] else {
            panic!("one commit: {fields:?}");
        };
        assert!(one.iter().all(|field| matches!(
            field,
            EntityField::BodyEntry(Entry {
                id: body_ids::IDLE,
                ..
            }) | EntityField::Lived(_)
        )));
    }
    // A chance of a thousandth more each tick of its count makes a reset within 200
    // ticks all but certain.
    assert!(resets >= 1);
}

#[test]
fn a_fall_of_five_blocks_hurts_by_two_with_the_small_sound_and_one_of_eight_by_five_with_the_big() {
    for (height, damage, sound) in [
        (5.0, 2.0, sounds::ENTITY_GENERIC_SMALL_FALL),
        (8.0, 5.0, sounds::ENTITY_GENERIC_BIG_FALL),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        let mut falling = plain([4.5, -60.0 + height, 4.5], false);
        falling.motion = [0.0; 3];
        lay(&mut region, &falling);
        let mut heard = Vec::new();
        let mut damaged = false;
        for _ in 0..60 {
            let output = checked(&mut region, &TickInputs::default());
            heard.extend(sounds_of(&output));
            damaged |= output.events.iter().any(|event| {
                matches!(event, RegionEvent::DamageEvent { entity, .. } if *entity == COW_KEY)
            });
        }
        assert_eq!(mob(&region, COW_KEY).body.vitals.health, 10.0 - damage);
        assert!(heard.contains(&sound), "{height}: {heard:?}");
        assert!(heard.contains(&sounds::ENTITY_GENERIC_HURT));
        assert!(damaged);
    }
}

#[test]
fn a_mob_in_lava_burns_and_is_hurt_by_four_every_ten_ticks() {
    let lava: Vec<(BlockPos, clustine_data::BlockState)> = (2..7)
        .flat_map(|x| (2..7).map(move |z| (BlockPos::new(x, -60, z), blocks::LAVA)))
        .collect();
    let mut region = on_ticking_floor_with(&lava);
    lay(&mut region, &cow([4.5, -60.0, 4.5], false));
    let mut health = Vec::new();
    let mut burns = Vec::new();
    for _ in 0..15 {
        let output = checked(&mut region, &TickInputs::default());
        burns.push(
            sounds_of(&output)
                .iter()
                .filter(|&&sound| sound == sounds::ENTITY_GENERIC_BURN)
                .count(),
        );
        health.push(mob(&region, COW_KEY).body.vitals.health);
    }
    // Lava's first tick does not count (`firstTick`); then 4 at once, and again only once
    // its cooldown is 10.
    assert!(record(&region, COW_KEY).unwrap().fire_ticks > 0);
    let first = health.iter().position(|&left| left < 10.0).unwrap();
    assert_eq!(health[first], 6.0);
    assert_eq!(health[first + 9], 6.0);
    assert_eq!(health[first + 10], 2.0);
    // The burn's sound only with a hurt that counted.
    assert_eq!(burns.iter().take(first + 11).sum::<usize>(), 2);
    assert_eq!((burns[first], burns[first + 10]), (1, 1));
}

#[test]
fn a_creative_player_s_hit_is_a_survival_hit_a_crit_falling_and_a_sweep_with_a_sword() {
    // A bare hand, standing: 1, and both pushes but the attack's own (no knockback).
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow([2.5, -60.0, 0.5], true));
    let output = checked(&mut region, &attack(COW_KEY));
    let hit = record(&region, COW_KEY).unwrap();
    assert_eq!(mob(&region, COW_KEY).body.vitals.health, 9.0);
    assert!(
        hit.motion[0] > 0.0,
        "pushed away from the player: {:?}",
        hit.motion
    );
    assert!(sounds_of(&output).contains(&sounds::ENTITY_PLAYER_ATTACK_STRONG));
    assert_eq!(
        mob(&region, COW_KEY).body.combat.player_memory(hit.lived),
        Some(1)
    );

    // Falling: half again, and the critical hit's sound and show.
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow([2.5, -60.0, 0.5], true));
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Move {
                position: Some(Vec3::new(0.5, -59.0, 0.5)),
                rotation: None,
                on_ground: false,
            },
        )]),
    );
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Move {
                position: Some(Vec3::new(0.5, -59.5, 0.5)),
                rotation: None,
                on_ground: false,
            },
        )]),
    );
    let output = checked(&mut region, &attack(COW_KEY));
    assert_eq!(mob(&region, COW_KEY).body.vitals.health, 8.5);
    assert!(sounds_of(&output).contains(&sounds::ENTITY_PLAYER_ATTACK_CRIT));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Animated { target, action: 1, .. } if *target == COW_KEY
    )));

    // A sword on the ground sweeps: the target its sword's 7, one beside it the sweep's 1.
    let mut region = on_ticking_floor_with(&[]);
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Move {
                position: Some(Vec3::new(0.5, -60.0, 0.5)),
                rotation: None,
                on_ground: true,
            },
        )]),
    );
    holding(
        &mut region,
        ItemStack {
            item: items::DIAMOND_SWORD,
            count: 1,
        },
    );
    // A sword is at full strength twenty over its speed of 1.6 ticks after it is taken
    // up, 12.5.
    for _ in 0..13 {
        checked(&mut region, &TickInputs::default());
    }
    lay(&mut region, &cow([2.5, -60.0, 0.5], true));
    let beside = mob_of(EntityId(41), entity_types::COW, [2.5, -60.0, 1.5], true);
    lay(&mut region, &beside);
    let output = checked(&mut region, &attack(COW_KEY));
    assert_eq!(mob(&region, COW_KEY).body.vitals.health, 3.0);
    assert_eq!(mob(&region, EntityId(41)).body.vitals.health, 9.0);
    assert!(sounds_of(&output).contains(&sounds::ENTITY_PLAYER_ATTACK_SWEEP));
    assert!(!sounds_of(&output).contains(&sounds::ENTITY_PLAYER_ATTACK_STRONG));
}

#[test]
fn death_lasts_twenty_ticks_and_drops_the_table_s_loot_and_experience() {
    let mut region = on_ticking_floor_with(&[]);
    let mut weak = cow([2.5, -60.0, 0.5], true);
    let EntityData::Mob { .. } = weak.data else {
        unreachable!();
    };
    let mut body = Mob::decode(&weak.data).unwrap();
    body.body.vitals.health = 1.0;
    weak.data = body.encode();
    lay(&mut region, &weak);
    let output = checked(&mut region, &attack(COW_KEY));
    assert_eq!(entity_events(&output, COW_KEY), [3]);
    let dropped = items_lying(&region);
    assert!(
        dropped
            .iter()
            .all(|stack| [items::LEATHER, items::BEEF].contains(&stack.item)),
        "{dropped:?}"
    );
    assert!(dropped.iter().any(|stack| stack.item == items::BEEF));
    let given: i32 = orbs(&region)
        .iter()
        .map(|orb| match orb.data {
            EntityData::ExperienceOrb { value, count, .. } => value * count,
            _ => 0,
        })
        .sum();
    assert!((1..=3).contains(&given), "{given}");
    // It lies dying for twenty ticks, then goes in smoke.
    let mut gone_at = None;
    for tick in 1..=25 {
        let output = checked(&mut region, &TickInputs::default());
        if entity_events(&output, COW_KEY).contains(&60) {
            gone_at = Some(tick);
            assert!(record(&region, COW_KEY).is_none());
            break;
        }
        assert!(record(&region, COW_KEY).is_some());
    }
    // Hit between ticks (the region's inputs come before its entities), its first
    // `tickDeath` is in the tick of the hit: twenty `tickDeath`s, the hit's tick and 19.
    assert_eq!(gone_at, Some(19));
}

#[test]
fn an_experience_orb_merges_with_one_of_its_worth_and_is_picked_up_one_at_a_time() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let at = [6.5, -60.0, 6.5];
    region.award_orb(at, 3, 0, &mut output);
    let [orb] = &orbs(&region)[..] else {
        panic!("one orb");
    };
    // The level's draw that names the first orb's id modulo 40 merges the next into it.
    let id = (orb.key.0 as u32 as i32).rem_euclid(40) as u32;
    region.award_orb(at, 3, id, &mut output);
    let [orb] = &orbs(&region)[..] else {
        panic!("still one orb");
    };
    assert!(matches!(
        orb.data,
        EntityData::ExperienceOrb {
            value: 3,
            count: 2,
            ..
        }
    ));
    // One of another worth is an orb of its own.
    region.award_orb(at, 1, id, &mut output);
    assert_eq!(orbs(&region).len(), 2);
    // Brought to the player, it is taken one at a time, every other tick.
    let orb_key = orb.key;
    let mut taken = 0;
    for _ in 0..80 {
        let output = checked(&mut region, &TickInputs::default());
        taken += output
            .events
            .iter()
            .filter(
                |event| matches!(event, RegionEvent::ItemTaken { item, .. } if *item == orb_key),
            )
            .count();
        if let Some(orb) = record(&region, orb_key) {
            let mut moved = orb.clone();
            moved.position = [0.5, -60.0, 0.5];
            lay(&mut region, &moved);
        }
    }
    assert_eq!(taken, 2);
    assert!(record(&region, orb_key).is_none());
    // The other, more than 8 blocks off, does not follow the player and lies there.
    assert_eq!(region.players[&player(1)].experience.total, 3 + 3);
    assert_eq!(orbs(&region).len(), 1);
}

#[test]
fn a_spawn_egg_of_a_type_not_built_yet_makes_nothing() {
    // The plan's named difference of the first half (`docs/groundwork/n7-plan.md`,
    // section 11.6): a mob of a type without its AI would stand like a statue.
    let mut region = on_ticking_floor_with(&[(BlockPos::new(3, -60, 0), blocks::WATER)]);
    holding(
        &mut region,
        ItemStack {
            item: items::GOAT_SPAWN_EGG,
            count: 1,
        },
    );
    checked(&mut region, &moves(vec![place(1, 1, -61, 1, Face::Top)]));
    assert!(mobs_of(&region, entity_types::GOAT).is_empty());
}

#[test]
fn a_spawned_mob_has_the_game_s_defaults_on_a_block_and_at_the_bottom_of_water() {
    let mut region = on_ticking_floor_with(&[(BlockPos::new(3, -60, 0), blocks::WATER)]);
    let cow = entity_types::COW as u16;
    let mut output = TickOutput::default();
    // As an egg used on the top of the floor makes it: beside, put on what is below.
    let key = region.spawn_mob(
        cow,
        player(1),
        BlockPos::new(1, -60, 1),
        true,
        true,
        &mut output,
    );
    let [made] = &mobs_of(&region, entity_types::COW)[..] else {
        panic!("one cow");
    };
    assert_eq!(Some(made.key), key);
    assert_eq!(made.position, [1.5, -60.0, 1.5]);
    let body = Mob::decode(&made.data).unwrap().body;
    assert_eq!(body.vitals.health, 10.0);
    assert_eq!(body.vitals.air, MAX_AIR);
    assert_eq!(body.head.head_yaw, made.yaw);
    let follow = body
        .attributes
        .made(clustine_data::attributes::FOLLOW_RANGE as u16)
        .expect("its follow range is made");
    let bonus = &follow.modifiers[0];
    assert_eq!(bonus.id, RANDOM_SPAWN_BONUS);
    assert!(bonus.amount.abs() <= 0.11485);
    assert!(output.events.iter().any(
        |event| matches!(event, RegionEvent::EntitySpawned(state) if state.entity == made.key)
    ));
    // As an egg used on water makes it: at the bottom of the source block.
    region.spawn_mob(
        cow,
        player(1),
        BlockPos::new(3, -60, 0),
        false,
        false,
        &mut output,
    );
    let cows = mobs_of(&region, entity_types::COW);
    assert_eq!(cows.len(), 2);
    assert!(cows.iter().any(|cow| cow.position == [3.5, -60.0, 0.5]));
}

#[test]
fn an_animal_never_despawns_and_a_fish_far_from_every_player_does() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow([4.5, -60.0, 4.5], true));
    let fish = mob_of(EntityId(42), entity_types::COD, [5.5, -60.0, 5.5], true);
    lay(&mut region, &fish);
    // The player goes far beyond both: 64 is a fish's despawn distance, 128 a cow's.
    region.players.get_mut(&player(1)).unwrap().pose.position = Vec3::new(300.0, -60.0, 0.5);
    for _ in 0..3 {
        checked(&mut region, &TickInputs::default());
    }
    assert!(record(&region, COW_KEY).is_some());
    assert!(record(&region, EntityId(42)).is_none());
}

#[test]
fn a_leashed_mob_is_kept_to_its_holder_and_pulled_after_them() {
    let mut region = on_ticking_floor_with(&[]);
    holding(
        &mut region,
        ItemStack {
            item: items::LEAD,
            count: 1,
        },
    );
    let tied = cow([3.5, -60.0, 0.5], true);
    lay(&mut region, &tied);
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: COW_KEY,
                location: [0.0, 0.5, 0.0],
                sneaking: false,
            },
        )]),
    );
    let holder = player_entity(&region);
    let leash = record(&region, COW_KEY)
        .unwrap()
        .riding
        .leash
        .expect("leashed");
    assert_eq!(leash.holder, LeashHolder::Entity(holder));
    checked(&mut region, &TickInputs::default());
    let home = mob(&region, COW_KEY).body.home;
    assert_eq!(home.centre, Some(BlockPos::new(0, -60, 0)));
    assert_eq!(home.radius, 5);
    // The holder walks off: past the elastic distance the leash pulls the cow after them.
    region.players.get_mut(&player(1)).unwrap().pose.position = Vec3::new(-6.0, -60.0, 0.5);
    checked(&mut region, &TickInputs::default());
    let pulled = record(&region, COW_KEY).unwrap();
    assert!(pulled.motion[0] < 0.0, "{:?}", pulled.motion);
    // Beyond the snap distance it breaks and the lead drops.
    region.players.get_mut(&player(1)).unwrap().pose.position = Vec3::new(-14.0, -60.0, 0.5);
    let output = checked(&mut region, &TickInputs::default());
    assert!(record(&region, COW_KEY).unwrap().riding.leash.is_none());
    assert!(sounds_of(&output).contains(&sounds::ITEM_LEAD_BREAK));
    assert!(
        items_lying(&region)
            .iter()
            .any(|stack| stack.item == items::LEAD)
    );
}

#[test]
fn a_tick_is_a_function_of_its_inputs() {
    let run = || {
        let mut region = on_ticking_floor_with(&[]);
        let mut falling = cow([4.5, -55.0, 4.5], false);
        falling.motion = [0.1, 0.0, -0.05];
        lay(&mut region, &falling);
        lay(
            &mut region,
            &mob_of(EntityId(41), entity_types::PIG, [4.8, -60.0, 4.5], true),
        );
        let mut changes = Vec::new();
        for tick in 0..40 {
            let inputs = if tick == 10 {
                attack(EntityId(41))
            } else {
                TickInputs::default()
            };
            let output = checked(&mut region, &inputs);
            changes.push(output.entity_changes.clone());
        }
        (
            changes,
            record(&region, COW_KEY),
            record(&region, EntityId(41)),
        )
    };
    assert_eq!(run(), run());
}

#[test]
fn a_mob_pushes_another_in_its_box_and_both_move_apart() {
    let mut region = on_ticking_floor_with(&[]);
    let mut one = cow([4.5, -60.0, 4.5], true);
    one.motion = [0.0, RESTING, 0.0];
    let mut two = mob_of(EntityId(41), entity_types::COW, [4.7, -60.0, 4.5], true);
    two.motion = [0.0, RESTING, 0.0];
    lay(&mut region, &one);
    lay(&mut region, &two);
    checked(&mut region, &TickInputs::default());
    checked(&mut region, &TickInputs::default());
    let one = record(&region, COW_KEY).unwrap();
    let two = record(&region, EntityId(41)).unwrap();
    assert!(one.position[0] < 4.5, "{:?}", one.position);
    assert!(two.position[0] > 4.7, "{:?}", two.position);
}

#[test]
fn a_leashed_mob_is_tied_to_a_fence_its_holder_uses_and_kept_home_there() {
    let fence = BlockPos::new(2, -60, 3);
    let mut region = on_ticking_floor_with(&[(fence, blocks::OAK_FENCE)]);
    holding(
        &mut region,
        ItemStack {
            item: items::LEAD,
            count: 1,
        },
    );
    lay(&mut region, &cow([3.5, -60.0, 0.5], true));
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: COW_KEY,
                location: [0.0, 0.5, 0.0],
                sneaking: false,
            },
        )]),
    );
    checked(
        &mut region,
        &moves(vec![place(1, fence.x, fence.y, fence.z, Face::Top)]),
    );
    let leash = record(&region, COW_KEY)
        .unwrap()
        .riding
        .leash
        .expect("leashed");
    assert_eq!(leash.holder, LeashHolder::Fence(fence));
    checked(&mut region, &TickInputs::default());
    let home = mob(&region, COW_KEY).body.home;
    assert_eq!((home.centre, home.radius), (Some(fence), 5));
}

/// `docs/groundwork/n7-plan.md`, sections 9.4 and 10.7: a request to breed goes to a
/// partner that only a copy shows as an effect on it, and a request taken from across
/// a line is answered back the same way (refused, until the animals of N7e decide).
#[test]
fn a_request_to_breed_crosses_a_line_as_an_effect_and_its_answer_comes_back() {
    // A region of the land west of chunk x = 2, with the floor about its spawn.
    let west = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let chunks = around.iter().map(|position| (*position, floor())).collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    lay(&mut region, &cow([3.5, -60.0, 0.5], true));
    let beyond = ChunkPos::new(40, 0);
    let partner = mob_of(
        EntityId(1 << 34),
        entity_types::COW,
        [645.5, -60.0, 2.5],
        true,
    );
    let mut base = crate::region::tests::floor();
    base.set_entities(vec![partner.clone()]);
    region.tick(&TickInputs {
        foreign: vec![(beyond, RegionId(1))],
        copy_bases: vec![(beyond, base)],
        ..TickInputs::default()
    });
    let mut output = TickOutput::default();
    region.ask_to_breed(COW_KEY, partner.key, 7, &mut output);
    assert_eq!(
        region.made,
        [Effect {
            at: BlockPos::new(645, -60, 2),
            what: Effected::OnEntity {
                key: partner.key,
                by: EntityUse::BreedAsk {
                    asker: COW_KEY,
                    number: 7,
                },
            },
        }]
    );
    region.made.clear();

    // The partner's side: the request comes as an effect on the region's own cow, from
    // the animal beyond, which a copy shows; the answer goes back on it.
    let asked = TickInputs {
        effects: vec![FiledEffect {
            id: EffectId {
                sequence: 9,
                index: 0,
            },
            effect: Effect {
                at: BlockPos::new(3, -60, 0),
                what: Effected::OnEntity {
                    key: COW_KEY,
                    by: EntityUse::BreedAsk {
                        asker: partner.key,
                        number: 3,
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    let output = region.tick(&asked);
    assert!(
        output.effects.contains(&Effect {
            at: BlockPos::new(645, -60, 2),
            what: Effected::OnEntity {
                key: partner.key,
                by: EntityUse::BreedAnswer {
                    partner: COW_KEY,
                    number: 3,
                    accepted: false,
                },
            },
        }),
        "{:?}",
        output.effects
    );
}

/// `docs/groundwork/n7-plan.md`, section 7.3.5: a leash held by a player of another
/// region, whom only their face shows, holds and pulls as one held by the region's own.
#[test]
fn a_leash_held_by_a_player_of_another_region_pulls_by_their_face() {
    let mut region = on_ticking_floor_with(&[]);
    let holder = EntityId(500);
    let mut tied = cow([3.5, -60.0, 0.5], true);
    tied.riding.leash = Some(Box::new(clustine_world::Leash {
        holder: LeashHolder::Entity(holder),
        angular_momentum: clustine_world::Double(0.0),
    }));
    lay(&mut region, &tied);
    let face = |x: f64, seq: u64| crate::api::FaceChange::Face {
        player: player(9),
        entity: holder,
        hops: 0,
        seq: crate::api::FaceSeq(seq, 0),
        face: crate::api::PlayerFace::of(
            &Pose::at(Vec3::new(x, -60.0, 0.5)),
            false,
            false,
            &Default::default(),
            0,
            None,
        ),
    };
    checked(
        &mut region,
        &TickInputs {
            faces: vec![face(1.5, 1)],
            ..TickInputs::default()
        },
    );
    assert!(record(&region, COW_KEY).unwrap().riding.leash.is_some());
    // They walk off past the elastic distance: the cow is pulled after them.
    checked(
        &mut region,
        &TickInputs {
            faces: vec![face(-6.0, 2)],
            ..TickInputs::default()
        },
    );
    let pulled = record(&region, COW_KEY).unwrap();
    assert!(pulled.riding.leash.is_some());
    assert!(pulled.motion[0] < 0.0, "{:?}", pulled.motion);
}

/// What a mob's use gives a player: into the first empty slot, once only where duplicates
/// are kept out; across a line, as an effect to the player's region.
#[test]
fn a_given_stack_goes_into_the_player_s_inventory_or_across_the_line() {
    let mut region = on_ticking_floor_with(&[]);
    let bucket = ItemStack {
        item: items::MILK_BUCKET,
        count: 1,
    };
    let mut output = TickOutput::default();
    let own = player_entity(&region);
    region.give_to_player(own, bucket, true, [0.5, -60.0, 0.5], &mut output);
    region.give_to_player(own, bucket, true, [0.5, -60.0, 0.5], &mut output);
    let held = region.players[&player(1)]
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == items::MILK_BUCKET)
        .count();
    assert_eq!(held, 1);
    region.give_to_player(own, bucket, false, [0.5, -60.0, 0.5], &mut output);
    let held = region.players[&player(1)]
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == items::MILK_BUCKET)
        .count();
    assert_eq!(held, 2);
}

#[test]
fn a_stack_given_to_a_player_of_another_region_goes_to_them_as_an_effect() {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };
    let mut region = joined_in(west, &[1]);
    let beyond = ChunkPos::new(40, 0);
    region.tick(&TickInputs {
        foreign: vec![(beyond, RegionId(1))],
        copy_bases: vec![(beyond, crate::region::tests::floor())],
        ..TickInputs::default()
    });
    let stew = ItemStack {
        item: items::MUSHROOM_STEW,
        count: 1,
    };
    let mut output = TickOutput::default();
    region.give_to_player(EntityId(900), stew, false, [645.5, -60.0, 2.5], &mut output);
    assert_eq!(
        region.made,
        [Effect {
            at: BlockPos::new(645, -60, 2),
            what: Effected::Given {
                player: EntityId(900),
                stack: stew,
                no_duplicates: false,
            },
        }]
    );
}

#[test]
fn a_stack_given_from_another_region_reaches_the_player_s_inventory() {
    let mut region = on_ticking_floor_with(&[]);
    let own = player_entity(&region);
    let stew = ItemStack {
        item: items::MUSHROOM_STEW,
        count: 1,
    };
    let output = region.tick(&TickInputs {
        effects: vec![FiledEffect {
            id: EffectId {
                sequence: 4,
                index: 0,
            },
            effect: Effect {
                at: BlockPos::new(0, -60, 0),
                what: Effected::Given {
                    player: own,
                    stack: stew,
                    no_duplicates: false,
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    });
    assert!(region.players[&player(1)].inventory.0.contains(&Some(stew)));
    assert!(output.player_events.iter().any(|(id, event)| *id == player(1)
        && matches!(event, PlayerEvent::InventorySlot { stack: Some(given), .. } if *given == stew)));
}

/// `ItemUtils.createFilledResult` for a creative player whose inventory is full: what
/// duplicates are kept out of is not given, and anything else is thrown.
#[test]
fn a_full_inventory_throws_what_is_given_unless_duplicates_are_kept_out() {
    let mut region = on_ticking_floor_with(&[]);
    let own = player_entity(&region);
    let stone = ItemStack {
        item: items::STONE,
        count: 1,
    };
    for slot in &mut region.players.get_mut(&player(1)).unwrap().inventory.0[..36] {
        *slot = Some(stone);
    }
    let bucket = ItemStack {
        item: items::MILK_BUCKET,
        count: 1,
    };
    let mut output = TickOutput::default();
    region.give_to_player(own, bucket, true, [0.5, -60.0, 0.5], &mut output);
    assert!(items_lying(&region).is_empty());
    region.give_to_player(own, bucket, false, [0.5, -60.0, 0.5], &mut output);
    assert_eq!(items_lying(&region), [bucket]);
}

/// A mob that goes from one of the region's chunks into another is reported as
/// entering it whole (`RegionEvent::EntityEntered`), for an edge that was shown the two
/// chunks at different ticks; one that stays in its chunk is not.
#[test]
fn a_mob_that_goes_into_another_chunk_is_reported_entering_it_whole() {
    let mut region = on_ticking_floor_with(&[]);
    let mut goat = mob_of(COW_KEY, entity_types::GOAT, [15.7, -60.0, 0.5], true);
    goat.motion = [0.8, 0.0, 0.0];
    lay(&mut region, &goat);
    let output = checked(&mut region, &TickInputs::default());
    let entered: Vec<&EntityState> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEntered(state) => Some(state),
            _ => None,
        })
        .collect();
    let [state] = entered[..] else {
        panic!("{:?}", output.events);
    };
    assert_eq!(state.entity, COW_KEY);
    assert_eq!(state.chunk(), ChunkPos::new(1, 0));
    assert_eq!(
        record(&region, COW_KEY).unwrap().chunk(),
        ChunkPos::new(1, 0)
    );
    let output = checked(&mut region, &TickInputs::default());
    assert!(
        !output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::EntityEntered(_)))
    );
}
