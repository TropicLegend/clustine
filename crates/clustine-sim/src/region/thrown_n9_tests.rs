//! Tests of N9's thrown kinds, the cloud, rockets, fangs and the spit from the game's
//! rules (`docs/groundwork/n9-plan.md`, sections 4.4, 4.5, 4.7 and 4.8, as D9 settles
//! them, read against `ThrownEgg`, `ThrownSplashPotion`, `ThrownLingeringPotion`,
//! `ThrownExperienceBottle`, `ThrownEnderpearl`, `ExperienceOrb`, `AreaEffectCloud`,
//! `FireworkRocketEntity`, `EvokerFangs`, `LlamaSpit` and `Llama`). The worlds are those
//! of the tests of TNT (`tnt/tests.rs`): a stone floor at y = -61, its top at -60.
//!
//! Draws are worked out from the streams the game draws them from: a projectile's own
//! stream in its tick (`Random::of_entity` of its key and `lived`), and a fresh entity's
//! aside (`Random::of_entity_aside`) where it is made outside any entity's tick.

use clustine_data::{BlockState, entity_types, items, sounds};
use clustine_world::components::{
    Component, EffectInstance, FireworkExplosion, FireworkShape, Fireworks, PotionContents,
};
use clustine_world::projectiles::{Cloud, Fangs, Firework, Spit};
use clustine_world::{
    BlockPos, ChunkPos, Effected, Entity, EntityChange, EntityData, EntityId, EntityUse, ItemStack,
    PlayerId,
};

use super::super::Region;
use super::super::projectiles::{Aim, Making};
use super::super::random_ticks::Random;
use super::super::tnt::tests::{add, add_player, idle, records, west_of_a_line, world};
use crate::api::{PlayerEvent, RegionEvent, TickInputs, TickOutput};
use crate::mobs::Mob;
use crate::physics::aabb::Aabb;
use crate::physics::thrown::{AIR_DRAG, margin};

const SEED: u64 = 0x5EED_0009;

/// Only the floor.
fn nothing(_: BlockPos) -> Option<BlockState> {
    None
}

fn kind(of: i32) -> u16 {
    u16::try_from(of).expect("entity type ids fit 16 bits")
}

/// A cow resting on the floor at `feet`, of full health, with no AI.
fn cow(key: EntityId, feet: [f64; 3]) -> Entity {
    let kind = kind(entity_types::COW);
    let mut mob = Mob::fresh();
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(
        kind,
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a cow has a health") as f32;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.no_ai = true;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 1),
        position: feet,
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

/// Lays `entity` in the region's loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// A cow of the region at `feet`, under its next key.
fn a_cow_at(region: &mut Region, feet: [f64; 3]) -> EntityId {
    let chunk = ChunkPos::containing(feet[0], feet[2]);
    let key = region.take_entity_key(chunk).expect("a key");
    lay(region, &cow(key, feet));
    key
}

fn mob_of(region: &Region, key: EntityId) -> Mob {
    let (_, record) = region.find_entity(key).expect("the mob is here");
    Mob::decode(&record.data).expect("a mob's record")
}

fn health(region: &Region, key: EntityId) -> f32 {
    mob_of(region, key).body.vitals.health
}

/// The effects the region's mob `key` has, with the ticks each has left.
fn effects_of(region: &Region, key: EntityId) -> Vec<EffectInstance> {
    let lived = region.find_entity(key).expect("here").1.lived;
    crate::mobs::body::effects::instances(&mob_of(region, key).body.effects, u64::from(lived))
}

fn of_kind(region: &Region, of: i32) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == of)
        .collect()
}

fn the_one(region: &Region, of: i32) -> Entity {
    let found = of_kind(region, of);
    let [one] = &found[..] else {
        panic!("one of {of}: {found:?}");
    };
    one.clone()
}

fn gone(output: &TickOutput, key: EntityId) -> bool {
    output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key))
}

fn events_of(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
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

/// A thrown item of the type `of` made of `stack`, owned by `owner`, at `at` and shot
/// along `direction` at `power` with no uncertainty. Its key.
fn throw(
    region: &mut Region,
    of: i32,
    stack: ItemStack,
    owner: Option<EntityId>,
    at: [f64; 3],
    direction: [f64; 3],
    power: f32,
) -> EntityId {
    let making = Making {
        kind: kind(of),
        at,
        data: EntityData::Thrown {
            stack,
            owner,
            left_owner: false,
            has_been_shot: false,
        },
        aim: Aim::Direction(direction),
        power,
        uncertainty: 0.0,
    };
    region
        .make_projectile(making, |_, _| {}, &mut TickOutput::default())
        .expect("made")
}

/// A stack of `item` of the potion `potion`.
fn potion(item: i32, potion: &str) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some(potion.to_owned()),
            ..PotionContents::default()
        }));
    stack
}

/// `1 - d / 4` of a potion that hit at `at` and a living entity of `width` and `height`
/// at `feet`: their boxes' gap (`AABB.distanceToSqr`), the potion's 0.25 square.
fn proximity(at: [f64; 3], feet: [f64; 3], width: f32, height: f32) -> f64 {
    let potion = Aabb::of_entity(at, 0.25, 0.25);
    let target = Aabb::of_entity(feet, width, height);
    1.0 - crate::physics::thrown::box_distance_sqr(&potion, &target).sqrt() / 4.0
}

/// A potion's level events, without where: the floor's top is met at -60 or a hair
/// below it.
fn level_events_here(output: &TickOutput) -> Vec<(i32, i32)> {
    level_events(output)
        .into_iter()
        .map(|(kind, at, data)| {
            assert_eq!((at.x, at.z), (0, 0));
            assert!(at.y == -60 || at.y == -61, "{at:?}");
            (kind, data)
        })
        .collect()
}

/// The ticks until the projectile `key` is gone, each tick's output kept; the last is
/// the tick it went in. Panics after `limit`.
fn until_gone(region: &mut Region, key: EntityId, limit: usize) -> Vec<TickOutput> {
    let mut outputs = Vec::new();
    for _ in 0..limit {
        let output = idle(region);
        let done = gone(&output, key);
        outputs.push(output);
        if done {
            return outputs;
        }
    }
    panic!("{key:?} still here after {limit} ticks");
}

/// The `lived` of the projectile `key` as the tick that it hits begins, found by
/// ticking until it is gone: each tick before, it lives one more.
fn lived_at_hit(outputs: &[TickOutput]) -> u32 {
    u32::try_from(outputs.len() - 1).expect("few ticks")
}

// The thrown kinds' flight.

/// D9 2.1: each kind's gravity, then the air's drag of 0.99.
#[test]
fn each_thrown_kind_falls_by_its_own_gravity() {
    for (of, item, gravity) in [
        (entity_types::SNOWBALL, items::SNOWBALL, 0.03),
        (entity_types::EGG, items::EGG, 0.03),
        (entity_types::ENDER_PEARL, items::ENDER_PEARL, 0.03),
        (entity_types::SPLASH_POTION, items::SPLASH_POTION, 0.05),
        (
            entity_types::LINGERING_POTION,
            items::LINGERING_POTION,
            0.05,
        ),
        (
            entity_types::EXPERIENCE_BOTTLE,
            items::EXPERIENCE_BOTTLE,
            0.07,
        ),
    ] {
        let mut region = world(SEED, nothing);
        let key = throw(
            &mut region,
            of,
            ItemStack::new(item, 1),
            None,
            [0.5, -50.0, 0.5],
            [0.0, 0.0, 1.0],
            1.0,
        );
        idle(&mut region);
        let (_, now) = region.find_entity(key).expect("still flying");
        let drag = f64::from(AIR_DRAG);
        let expected = [0.0, (0.0 - gravity) * drag, 1.0 * drag];
        assert_eq!(
            now.motion.map(f64::to_bits),
            expected.map(f64::to_bits),
            "{of}: {:?}",
            now.motion
        );
    }
}

// The egg.

/// The cow it hits is hurt by nothing; its particles; its chicks by its own draws.
#[test]
fn an_egg_hurts_by_nothing_shows_its_particles_and_hatches_by_its_own_draws() {
    let mut hatched_any = false;
    for seed in 0..64_u64 {
        let mut region = world(seed, nothing);
        let target = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
        let egg = throw(
            &mut region,
            entity_types::EGG,
            ItemStack::new(items::EGG, 1),
            None,
            [0.5, -59.0, 0.5],
            [1.0, 0.0, 0.0],
            1.5,
        );
        let outputs = until_gone(&mut region, egg, 5);
        let last = outputs.last().expect("a tick");
        assert_eq!(events_of(last, egg), [3]);
        assert!(mob_of(&region, target).body.vitals.hurt_from.is_some());
        let mut own = Random::of_entity(seed, egg, lived_at_hit(&outputs));
        let expected = if own.next_int(8) == 0 {
            if own.next_int(32) == 0 { 4 } else { 1 }
        } else {
            0
        };
        let chicks = of_kind(&region, entity_types::CHICKEN);
        assert_eq!(chicks.len(), expected, "seed {seed}");
        hatched_any |= expected > 0;
        if expected > 0 {
            break;
        }
    }
    assert!(hatched_any, "one of the seeds hatches a chick");
}

/// `ThrownEgg.onHit` as its draws fall out at their rarest: four chicks, each a baby of
/// the stack's variant, at the egg with its yaw, put on the floor.
#[test]
fn four_chicks_hatch_when_both_draws_are_nothing_each_a_baby_of_the_eggs_variant() {
    // A stream whose first `nextInt(8)` and `nextInt(32)` are both 0.
    let random = (0..100_000_u64)
        .map(|n| Random::of_entity(SEED, EntityId(n), 0))
        .find(|random| {
            let mut probe = random.clone();
            probe.next_int(8) == 0 && probe.next_int(32) == 0
        })
        .expect("one stream in 256 does");
    let mut region = world(SEED, nothing);
    let egg = Entity {
        yaw: 37.0,
        position: [4.5, -60.0, 4.5],
        ..cow(EntityId(1), [4.5, -60.0, 4.5])
    };
    let mut own = random.clone();
    let mut output = TickOutput::default();
    region.hatch(
        &egg,
        &ItemStack::new(items::BLUE_EGG, 1),
        &mut own,
        &mut output,
    );
    assert_eq!(own.draws_since(&random), 2);
    let chicks = of_kind(&region, entity_types::CHICKEN);
    assert_eq!(chicks.len(), 4);
    let cold = clustine_data::synced_registry("minecraft:chicken_variant")
        .and_then(|known| known.id_of("minecraft:cold"))
        .expect("the cold chicken");
    for chick in chicks {
        assert_eq!(chick.yaw, 37.0);
        assert_eq!(chick.position[0], 4.5);
        assert_eq!(chick.position[2], 4.5);
        // Out of the floor by the millionth `fudgePositionAfterSizeChange` keeps.
        assert!(
            (chick.position[1] + 60.0).abs() < 1.0e-5,
            "{:?}",
            chick.position
        );
        let mob = Mob::decode(&chick.data).expect("a mob");
        let (animal, own) = crate::mobs::types::animal::read::<
            crate::mobs::types::chicken::ChickenState,
        >(&mob.state);
        assert_eq!(animal.age(0), crate::mobs::types::animal::BABY_START_AGE);
        assert_eq!(Some(own.variant), u8::try_from(cold).ok());
    }
}

// The splash potion.

/// Three ticks for a potion shot down at 0.5 from a block above the floor.
fn splash_on_the_floor(
    region: &mut Region,
    stack: ItemStack,
    at: [f64; 3],
) -> (EntityId, TickOutput) {
    let key = throw(
        region,
        entity_types::SPLASH_POTION,
        stack,
        None,
        at,
        [0.0, -1.0, 0.0],
        0.5,
    );
    let outputs = until_gone(region, key, 5);
    (key, outputs.last().expect("a tick").clone())
}

/// `ThrownSplashPotion.onHitAsPotion`: box to box, `1 - d / 4`, the duration
/// `(int) (prox · 3600 + 0.5)`; its level events of swiftness's colour.
#[test]
fn a_splash_gives_a_cow_its_effect_by_the_distance_between_their_boxes() {
    let mut region = world(SEED, nothing);
    let near = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let far = a_cow_at(&mut region, [6.5, -60.0, 0.5]);
    let stack = potion(items::SPLASH_POTION, "minecraft:swiftness");
    let (_, output) = splash_on_the_floor(&mut region, stack, [0.5, -59.0, 0.5]);
    // The potion's box at the floor spans x 0.375 to 0.625; the cow's begins at 3.05.
    let proximity = proximity([0.5, -60.0, 0.5], [3.5, -60.0, 0.5], 0.9, 1.4);
    assert!((proximity - (1.0 - 2.425 / 4.0)).abs() < 1.0e-6);
    let duration = (proximity * 3600.0 + 0.5) as i32;
    let given = effects_of(&region, near);
    let [speed] = &given[..] else {
        panic!("swiftness alone: {given:?}");
    };
    assert_eq!(speed.effect, "minecraft:speed");
    assert_eq!(speed.duration, duration, "{proximity}");
    assert!(effects_of(&region, far).is_empty());
    let colour = (0xFF00_0000_u32 as i32) | 3_402_751;
    assert_eq!(level_events_here(&output), [(2002, colour), (1053, 0)]);
}

/// An instant potion's splash: instant health heals by its proximity, and the events
/// are the instant ones.
#[test]
fn an_instant_splash_heals_by_its_proximity_with_the_instant_events() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [0.5, -60.0, 2.5]);
    // Hurt it first, so that the heal shows.
    let (chunk, _) = region.find_entity(target).expect("here");
    let mut mob = mob_of(&region, target);
    mob.body.vitals.health = 2.0;
    region.commit_entity(
        chunk,
        target,
        vec![clustine_world::EntityField::Data(mob.encode())],
        &mut TickOutput::default(),
    );
    let stack = potion(items::SPLASH_POTION, "minecraft:healing");
    let (_, output) = splash_on_the_floor(&mut region, stack, [0.5, -59.0, 0.5]);
    // The cow's box begins at z 2.05, the potion's ends at 0.625.
    let proximity = proximity([0.5, -60.0, 0.5], [0.5, -60.0, 2.5], 0.9, 1.4);
    // `(int) (prox · (4 << 0) + 0.5)`, through the float the effect carries: 3.
    let heal = (f64::from(proximity as f32) * 4.0 + 0.5) as i32;
    assert_eq!(heal, 3);
    assert_eq!(health(&region, target), 2.0 + heal as f32);
    let events: Vec<i32> = level_events_here(&output)
        .iter()
        .map(|(kind, _)| *kind)
        .collect();
    assert_eq!(events, [2007, 1054]);
}

/// Section 4.4: a cow in a copy is given its effects by its holder, scaled already.
#[test]
fn a_splash_reaches_a_cow_across_a_line_as_effects_for_its_holder() {
    let mut region = west_of_a_line(SEED, nothing);
    let key = EntityId(77_777);
    let across = cow(key, [17.0, -60.0, 0.5]);
    region
        .copies
        .get_mut(&across.chunk())
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(across));
    let stack = potion(items::SPLASH_POTION, "minecraft:swiftness");
    let (_, output) = splash_on_the_floor(&mut region, stack, [15.5, -59.0, 0.5]);
    let sent: Vec<&EntityUse> = output
        .effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnEntity { key: to, by } if *to == key => Some(by),
            _ => None,
        })
        .collect();
    let [EntityUse::Effects(given)] = &sent[..] else {
        panic!("effects for the cow: {:?}", output.effects);
    };
    let proximity = proximity([15.5, -60.0, 0.5], [17.0, -60.0, 0.5], 0.9, 1.4);
    let [speed] = &given.instances[..] else {
        panic!("{given:?}");
    };
    assert_eq!(speed.effect, "minecraft:speed");
    assert_eq!(speed.duration, (proximity * 3600.0 + 0.5) as i32);
    assert!(!given.water);
}

/// Section 4.4: the region's own player takes the splash's effect into their store.
#[test]
fn a_splash_gives_a_player_of_the_region_the_effect() {
    let mut region = world(SEED, nothing);
    let (id, _) = add_player(&mut region, 1, [0.5, -60.0, 2.5], false);
    let stack = potion(items::SPLASH_POTION, "minecraft:swiftness");
    let (_, output) = splash_on_the_floor(&mut region, stack, [0.5, -59.0, 0.5]);
    let proximity = proximity([0.5, -60.0, 0.5], [0.5, -60.0, 2.5], 0.6, 1.8);
    let added: Vec<&EffectInstance> = output
        .player_events
        .iter()
        .filter_map(|(to, event)| match event {
            PlayerEvent::EffectAdded { instance, .. } if *to == id => Some(instance),
            _ => None,
        })
        .collect();
    let [speed] = &added[..] else {
        panic!("{:?}", output.player_events);
    };
    assert_eq!(speed.effect, "minecraft:speed");
    assert_eq!(speed.duration, (proximity * 3600.0 + 0.5) as i32);
}

/// A splash of water hurts what water hurts (the snow golem) and puts out a burning
/// cow; a potion of no effects gives nothing.
#[test]
fn a_splash_of_water_puts_out_a_burning_cow_and_gives_no_effects() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [1.5, -60.0, 0.5]);
    let (chunk, _) = region.find_entity(target).expect("here");
    region.commit_entity(
        chunk,
        target,
        vec![clustine_world::EntityField::FireTicks(100)],
        &mut TickOutput::default(),
    );
    let stack = potion(items::SPLASH_POTION, "minecraft:water");
    let (_, output) = splash_on_the_floor(&mut region, stack, [0.5, -59.0, 0.5]);
    assert_eq!(region.find_entity(target).expect("here").1.fire_ticks, 0);
    assert!(effects_of(&region, target).is_empty());
    assert_eq!(
        level_events_here(&output),
        [(2002, super::BASE_POTION_COLOUR), (1053, 0)]
    );
}

// The lingering potion and its cloud.

/// `ThrownLingeringPotion.onHitAsPotion` and `AreaEffectCloud.serverTick`: the cloud
/// made at the potion, its wait, its first use at the age of 10 (a quarter of the
/// duration), its victim kept for 20, its shrink, and its end below half a block.
#[test]
fn a_lingering_potion_s_cloud_waits_applies_keeps_its_victims_shrinks_and_ends() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [1.0, -60.0, 0.5]);
    let key = throw(
        &mut region,
        entity_types::LINGERING_POTION,
        potion(items::LINGERING_POTION, "minecraft:swiftness"),
        None,
        [0.5, -59.0, 0.5],
        [0.0, -1.0, 0.0],
        0.5,
    );
    until_gone(&mut region, key, 5);
    let made = the_one(&region, entity_types::AREA_EFFECT_CLOUD);
    let EntityData::Cloud(cloud) = &made.data else {
        panic!("{:?}", made.data);
    };
    assert_eq!((made.position[0], made.position[2]), (0.5, 0.5));
    assert!(
        (made.position[1] + 60.0).abs() < 1.0e-9,
        "{:?}",
        made.position
    );
    assert_eq!(cloud.radius.0, 3.0);
    assert_eq!(cloud.radius_per_tick.0, -3.0_f32 / 600.0);
    assert_eq!(cloud.duration_scale.0, 0.25);
    assert_eq!((cloud.duration, cloud.wait_time), (600, 10));
    let cloud_key = made.key;
    let mut applied_at = Vec::new();
    let mut ended_at = None;
    for _ in 0..200 {
        let lived = region.find_entity(cloud_key).expect("the cloud").1.lived;
        let age = lived + 1;
        let before = effects_of(&region, target);
        let output = idle(&mut region);
        let after = effects_of(&region, target);
        let fresh = after.first().is_some_and(|speed| speed.duration == 900)
            && before.first().is_none_or(|speed| speed.duration != 900);
        if gone(&output, cloud_key) {
            // The use that takes it below half a block still gives its effects.
            if fresh {
                applied_at.push(age);
            }
            ended_at = Some(age);
            break;
        }
        if fresh {
            applied_at.push(age);
            let EntityData::Cloud(cloud) = &region.find_entity(cloud_key).expect("here").1.data
            else {
                panic!("a cloud");
            };
            assert_eq!(cloud.victims, [(target, age + 20)]);
            assert_eq!(cloud.radius_at, age);
        } else if !age.is_multiple_of(5) {
            // Between uses the cloud commits nothing.
            assert!(
                !output.entity_changes.iter().any(
                    |(_, change)| matches!(change, EntityChange::Set(key, _) if *key == cloud_key)
                ),
                "age {age}"
            );
        }
    }
    // Used at 10 and every twenty after; its fifth use, at 90, gives its effects and
    // ends it.
    assert_eq!(applied_at, [10, 30, 50, 70, 90]);
    // Its radius by the game's floats: 3, less 0.005 a tick from the tenth, less 0.5 a
    // use; at 90 it is 0.595 before its fifth use, which takes it below half a block.
    let mut radius = 3.0_f32;
    let mut end = None;
    for age in 10..200_u32 {
        radius += -3.0_f32 / 600.0;
        if radius < 0.5 {
            end = Some(age);
            break;
        }
        if age.is_multiple_of(10) && !(age / 10).is_multiple_of(2) {
            radius += -0.5;
            if radius < 0.5 {
                end = Some(age);
                break;
            }
        }
    }
    assert_eq!(ended_at, end);
    assert_eq!(ended_at, Some(90));
}

/// The cloud's last use, the one that takes it below half a block, gives the player
/// standing in it its effect in the very tick it ends (`serverTick` gives the effects
/// before it takes the radius on use off): five uses at 10, 30, 50, 70 and 90.
#[test]
fn a_cloud_s_last_use_gives_a_player_its_effect_in_the_tick_it_ends() {
    let mut region = world(SEED, nothing);
    let (id, _) = add_player(&mut region, 1, [1.0, -60.0, 0.5], false);
    let key = throw(
        &mut region,
        entity_types::LINGERING_POTION,
        potion(items::LINGERING_POTION, "minecraft:swiftness"),
        None,
        [0.5, -59.0, 0.5],
        [0.0, -1.0, 0.0],
        0.5,
    );
    until_gone(&mut region, key, 5);
    let cloud_key = the_one(&region, entity_types::AREA_EFFECT_CLOUD).key;
    let mut applied_at = Vec::new();
    let mut ended_at = None;
    for _ in 0..200 {
        let age = region.find_entity(cloud_key).expect("the cloud").1.lived + 1;
        let output = idle(&mut region);
        if output.player_events.iter().any(|(to, event)| {
            *to == id
                && matches!(event, PlayerEvent::EffectAdded { instance, .. }
                    if instance.effect == "minecraft:speed" && instance.duration == 900)
        }) {
            applied_at.push(age);
        }
        if gone(&output, cloud_key) {
            ended_at = Some(age);
            break;
        }
    }
    assert_eq!(applied_at, [10, 30, 50, 70, 90]);
    assert_eq!(ended_at, Some(90));
}

/// Section 4.1's rule: a lingering potion that hits a cow in a copy makes its cloud at
/// the cow, in the copy, sent to its holder.
#[test]
fn a_lingering_potion_that_hits_a_cow_across_a_line_sends_its_cloud_there() {
    let mut region = west_of_a_line(SEED, nothing);
    let key = EntityId(77_777);
    let across = cow(key, [17.0, -60.0, 0.5]);
    region
        .copies
        .get_mut(&across.chunk())
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(across));
    let thrown = throw(
        &mut region,
        entity_types::LINGERING_POTION,
        potion(items::LINGERING_POTION, "minecraft:swiftness"),
        None,
        [14.0, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        1.5,
    );
    let outputs = until_gone(&mut region, thrown, 5);
    let output = outputs.last().expect("a tick");
    let sent: Vec<&Entity> = output
        .arrivals
        .iter()
        .map(|(_, entity)| entity)
        .filter(|entity| i32::from(entity.kind) == entity_types::AREA_EFFECT_CLOUD)
        .collect();
    let [cloud] = &sent[..] else {
        panic!("one cloud sent: {:?}", output.arrivals);
    };
    assert_eq!(cloud.position, [17.0, -60.0, 0.5]);
    assert!(of_kind(&region, entity_types::AREA_EFFECT_CLOUD).is_empty());
}

/// The cloud's radius as it is derived between uses: the game's float additions.
#[test]
fn a_cloud_s_radius_is_derived_by_the_game_s_float_additions() {
    let cloud = Cloud {
        duration: 600,
        wait_time: 10,
        reapplication_delay: 20,
        radius: clustine_world::Float(3.0),
        radius_at: 0,
        radius_on_use: clustine_world::Float(-0.5),
        radius_per_tick: clustine_world::Float(-0.005),
        duration_on_use: 0,
        potion: PotionContents::default(),
        duration_scale: clustine_world::Float(1.0),
        owner: None,
        victims: Vec::new(),
    };
    assert_eq!(super::super::clouds::radius_at(&cloud, 9), Some(3.0));
    let mut radius = 3.0_f32;
    for _ in 10..=40 {
        radius += -0.005;
    }
    assert_eq!(super::super::clouds::radius_at(&cloud, 40), Some(radius));
    // Without a use it ends after 500 shrinking ticks.
    assert_eq!(super::super::clouds::radius_at(&cloud, 600), None);
}

// The experience bottle.

/// `ThrownExperienceBottle.onHit`: its events, then `3 + nextInt(5) + nextInt(5)` in
/// orbs at the floor, along its up side: a quarter of a block above, thrown upwards.
#[test]
fn a_bottle_breaks_into_its_drawn_experience_in_orbs_along_the_side_hit() {
    for seed in [1_u64, 2, 3] {
        let mut region = world(seed, nothing);
        let key = throw(
            &mut region,
            entity_types::EXPERIENCE_BOTTLE,
            ItemStack::new(items::EXPERIENCE_BOTTLE, 1),
            None,
            [0.5, -59.0, 0.5],
            [0.0, -1.0, 0.0],
            0.5,
        );
        let outputs = until_gone(&mut region, key, 5);
        let output = outputs.last().expect("a tick");
        assert_eq!(
            level_events_here(output),
            [(2002, super::BASE_POTION_COLOUR), (1053, 0)]
        );
        let mut own = Random::of_entity(seed, key, lived_at_hit(&outputs));
        let first = own.next_int(5) as i32;
        let experience = 3 + first + own.next_int(5) as i32;
        let orbs = of_kind(&region, entity_types::EXPERIENCE_ORB);
        let mut total = 0;
        for orb in &orbs {
            let EntityData::ExperienceOrb { value, count, .. } = orb.data else {
                panic!("an orb");
            };
            total += value * count;
            assert_eq!((orb.position[0], orb.position[2]), (0.5, 0.5));
            assert!(
                (orb.position[1] + 59.75).abs() < 1.0e-9,
                "{:?}",
                orb.position
            );
            assert!(orb.motion[1] >= 0.0, "{:?}", orb.motion);
        }
        assert_eq!(total, experience, "seed {seed}");
        // Split largest first: 3s, 7s and 1s.
        let mut values: Vec<i32> = orbs
            .iter()
            .map(|orb| match orb.data {
                EntityData::ExperienceOrb { value, .. } => value,
                _ => 0,
            })
            .collect();
        values.sort_unstable();
        assert!(
            values.iter().all(|value| [1, 3, 7].contains(value)),
            "{values:?}"
        );
    }
}

/// `awardWithDirection` against a motion: the orbs' throws turned back where they go
/// against the direction.
#[test]
fn a_directed_orb_s_throw_is_turned_back_where_it_goes_against_the_direction() {
    let mut region = world(SEED, nothing);
    let mut output = TickOutput::default();
    region.add_directed_orb([0.5, -58.0, 0.5], [0.0, -1.0, 0.0], 3, &mut output);
    let orb = the_one(&region, entity_types::EXPERIENCE_ORB);
    // Moved a quarter down, and its upward throw turned down.
    assert_eq!(orb.position, [0.5, -58.25, 0.5]);
    assert!(orb.motion[1] <= 0.0, "{:?}", orb.motion);
    let mut own = Random::of_entity_aside(SEED, orb.key, 0, region.tick);
    own.next_u64();
    own.next_u64();
    let yaw = own.next_float() * 360.0;
    assert_eq!(orb.yaw, yaw);
}

// The ender pearl.

/// `ThrownEnderpearl.onHit`: its owner, the region's player, moved to where the pearl
/// was as the tick it hit began, and told so.
#[test]
fn a_pearl_moves_its_player_to_where_it_was_as_its_landing_tick_began() {
    let mut region = world(SEED, nothing);
    let (id, entity) = add_player(&mut region, 1, [8.5, -60.0, 8.5], false);
    let key = throw(
        &mut region,
        entity_types::ENDER_PEARL,
        ItemStack::new(items::ENDER_PEARL, 1),
        Some(entity),
        [3.5, -59.0, 0.5],
        [0.0, -1.0, 0.0],
        0.5,
    );
    let mut before = None;
    let mut last = None;
    for _ in 0..5 {
        before = region.find_entity(key).map(|(_, pearl)| pearl.position);
        let output = idle(&mut region);
        let done = gone(&output, key);
        last = Some(output);
        if done {
            break;
        }
    }
    let from = before.expect("it flew");
    let output = last.expect("it landed");
    let (_, pose) = region.player(id).expect("the player");
    assert_eq!(
        [pose.position.x, pose.position.y, pose.position.z],
        from,
        "the pearl's place before its last move"
    );
    assert!(
        output
            .player_events
            .iter()
            .any(|(to, event)| *to == id && matches!(event, PlayerEvent::Teleported { .. }))
    );
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Sound { sound, .. } if *sound == sounds::ENTITY_PLAYER_TELEPORT
    )));
}

/// The pearl's draws: 32 particles of a double and two gaussians, then the endermite's
/// `nextFloat`, for a player owner.
#[test]
fn a_pearl_s_landing_draws_its_particles_then_the_endermite_s_chance() {
    let mut region = world(SEED, nothing);
    let (_, entity) = add_player(&mut region, 1, [8.5, -60.0, 8.5], false);
    let start = Random::of_entity(SEED, EntityId(5), 3);
    let mut own = start.clone();
    region.pearl_lands(
        [1.5, -60.0, 1.5],
        Some(entity),
        &mut own,
        &mut TickOutput::default(),
    );
    let mut by_hand = start.clone();
    for _ in 0..32 {
        by_hand.next_double();
        by_hand.next_gaussian();
        by_hand.next_gaussian();
    }
    by_hand.next_float();
    assert_eq!(own.draws_since(&start), by_hand.draws_since(&start));
    // With no owner, the particles alone.
    let mut alone = start.clone();
    region.pearl_lands(
        [1.5, -60.0, 1.5],
        None,
        &mut alone,
        &mut TickOutput::default(),
    );
    assert_eq!(alone.draws_since(&start) + 1, by_hand.draws_since(&start));
}

/// Section 4.4: a pearl of another region's player lands as an effect addressed to
/// them, to the block their face was last seen at.
#[test]
fn a_pearl_of_another_region_s_player_lands_as_an_effect_addressed_to_them() {
    let mut region = west_of_a_line(SEED, nothing);
    let player = PlayerId(uuid::Uuid::from_u128(9));
    let face = crate::api::FaceChange::Face {
        player,
        entity: EntityId(500),
        hops: 0,
        seq: crate::api::FaceSeq(1, 0),
        face: crate::api::PlayerFace::of(
            &crate::api::Pose::at(clustine_world::Vec3::new(20.5, -60.0, 0.5)),
            false,
            false,
            &clustine_world::Inventory::default(),
            0,
            None,
        ),
    };
    region.tick(&TickInputs {
        faces: vec![face],
        ..TickInputs::default()
    });
    let mut output = TickOutput::default();
    let mut own = Random::of_entity(SEED, EntityId(5), 3);
    region.pearl_lands(
        [3.5, -60.0, 0.5],
        Some(EntityId(500)),
        &mut own,
        &mut output,
    );
    let output = idle(&mut region);
    let sent: Vec<_> = output
        .effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnPlayer {
                player: to,
                entity,
                by: EntityUse::PearlLanded { at, .. },
            } if *to == player && *entity == EntityId(500) => Some((effect.at, *at)),
            _ => None,
        })
        .collect();
    let [(block, at)] = &sent[..] else {
        panic!("{:?}", output.effects);
    };
    assert_eq!(*block, BlockPos::new(20, -60, 0));
    assert_eq!(at.map(|value| value.0), [3.5, -60.0, 0.5]);
}

// Firework rockets.

fn rocket_stack(explosions: usize, flight: i32) -> ItemStack {
    let mut stack = ItemStack::new(items::FIREWORK_ROCKET, 1);
    let explosion = FireworkExplosion {
        shape: FireworkShape::SmallBall,
        colours: vec![0xFF_0000],
        fade_colours: Vec::new(),
        trail: false,
        twinkle: false,
    };
    stack.components.set(Component::Fireworks(Fireworks {
        flight_duration: flight,
        explosions: vec![explosion; explosions],
    }));
    stack
}

/// `FireworkRocketEntity.<init>`: after its UUID, two triangles across and the
/// lifetime's two ints, `10 · (1 + flight) + nextInt(6) + nextInt(7)`; then its flight,
/// 1.15 across and 0.04 up a tick, and its launch sound at life 0.
#[test]
fn a_rocket_draws_its_motion_and_lifetime_and_speeds_up_as_it_rises() {
    let mut region = world(SEED, nothing);
    let mut output = TickOutput::default();
    let stack = ItemStack::new(items::FIREWORK_ROCKET, 1);
    let key = region
        .make_firework(&stack, [0.5, -59.0, 0.5], None, false, None, &mut output)
        .expect("made");
    let made = the_one(&region, entity_types::FIREWORK_ROCKET);
    let mut own = Random::of_entity_aside(SEED, key, 0, region.tick);
    let uuid = super::super::entities::insecure_uuid(own.next_u64(), own.next_u64());
    let x = crate::physics::thrown::triangle(&mut own, 0.0, 0.002_297);
    let z = crate::physics::thrown::triangle(&mut own, 0.0, 0.002_297);
    let first = own.next_int(6) as i32;
    // The item's default flight of 1.
    let lifetime = 10 * 2 + first + own.next_int(7) as i32;
    assert_eq!(made.uuid, uuid);
    assert_eq!(made.motion, [x, 0.05, z]);
    let EntityData::Firework(firework) = &made.data else {
        panic!("{:?}", made.data);
    };
    assert_eq!(firework.lifetime, lifetime);
    assert_eq!(firework.life, 0);
    let output = idle(&mut region);
    let now = the_one(&region, entity_types::FIREWORK_ROCKET);
    let motion = [x * 1.15 + 0.0, 0.05 + 0.04, z * 1.15 + 0.0];
    assert_eq!(now.motion.map(f64::to_bits), motion.map(f64::to_bits));
    assert_eq!(
        now.position,
        [0.5 + motion[0], -59.0 + motion[1], 0.5 + motion[2]]
    );
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Sound { sound, volume, .. }
            if *sound == sounds::ENTITY_FIREWORK_ROCKET_LAUNCH && *volume == 3.0
    )));
    // It explodes past its lifetime: event 17, gone, and with no explosions it hurts
    // nobody.
    let outputs = until_gone(&mut region, key, 40);
    let last = outputs.last().expect("a tick");
    assert_eq!(events_of(last, key), [17]);
    assert_eq!(
        u32::try_from(outputs.len()).expect("few") + 1,
        u32::try_from(lifetime).expect("positive") + 1
    );
}

/// `dealExplosionDamage`: `(5 + 2n) · sqrt((5 - d) / 5)` to each living entity within
/// five with a clear line to it, of `distanceTo` in floats; nothing without explosions.
#[test]
fn a_rocket_s_explosion_hurts_by_distance_and_not_at_all_without_explosions() {
    for explosions in [0, 1, 2] {
        let mut region = world(SEED, nothing);
        let near = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
        let far = a_cow_at(&mut region, [6.5, -60.0, 0.5]);
        let full = health(&region, near);
        let rocket = Entity {
            kind: kind(entity_types::FIREWORK_ROCKET),
            position: [0.5, -59.0, 0.5],
            data: EntityData::Firework(Box::new(Firework {
                stack: rocket_stack(explosions, 0),
                owner: None,
                left_owner: true,
                has_been_shot: true,
                life: 5,
                lifetime: 5,
                shot_at_angle: false,
            })),
            ..cow(EntityId(42), [0.5, -59.0, 0.5])
        };
        let mut output = TickOutput::default();
        region.rocket_explodes(&rocket, &mut output);
        assert_eq!(events_of(&output, EntityId(42)), [17]);
        let damage = 5.0_f32 + 2.0 * explosions as f32;
        let distance = (3.0_f32 * 3.0 + 1.0 * 1.0).sqrt();
        let expected = if explosions == 0 {
            0.0
        } else {
            damage * ((5.0 - f64::from(distance)) / 5.0).sqrt() as f32
        };
        assert_eq!(health(&region, near), full - expected, "{explosions}");
        assert_eq!(health(&region, far), full);
    }
}

/// A rocket that hits a cow explodes there; one shot at an angle does not speed up.
#[test]
fn a_rocket_shot_at_an_angle_flies_straight_and_explodes_at_the_cow_it_hits() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let full = health(&region, target);
    let shot = Some((Aim::Direction([1.0, 0.0, 0.0]), 1.0, 0.0));
    let key = region
        .make_firework(
            &rocket_stack(1, 3),
            [0.5, -59.5, 0.5],
            shot,
            true,
            None,
            &mut TickOutput::default(),
        )
        .expect("made");
    let made = the_one(&region, entity_types::FIREWORK_ROCKET);
    assert_eq!(made.motion, [1.0, 0.0, 0.0]);
    idle(&mut region);
    let now = the_one(&region, entity_types::FIREWORK_ROCKET);
    assert_eq!(now.motion, [1.0, 0.0, 0.0]);
    let outputs = until_gone(&mut region, key, 5);
    assert_eq!(events_of(outputs.last().expect("a tick"), key), [17]);
    assert!(health(&region, target) < full);
}

// The evoker's fangs.

/// `EvokerFangs.tick`: they rise (event 4) the first tick the warmup is below nothing,
/// bite by `magic` 6 at -8, and are gone 23 ticks after the warmup ran out.
#[test]
fn fangs_rise_bite_after_their_warmup_and_are_gone_23_ticks_after_it() {
    let warmup = 3;
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [4.5, -60.0, 4.5]);
    let full = health(&region, target);
    let chunk = ChunkPos::new(0, 0);
    let fangs_key = region.take_entity_key(chunk).expect("a key");
    let fangs = Entity {
        key: fangs_key,
        kind: kind(entity_types::EVOKER_FANGS),
        position: [4.5, -60.0, 4.5],
        motion: [0.0; 3],
        on_ground: false,
        data: EntityData::Fangs(Fangs {
            owner: None,
            warmup,
        }),
        ..cow(fangs_key, [4.5, -60.0, 4.5])
    };
    add(&mut region, fangs);
    let mut rose_at = None;
    let mut bit_at = None;
    let mut gone_at = None;
    for tick in 1..=40_i32 {
        let output = idle(&mut region);
        if events_of(&output, fangs_key).contains(&4) {
            assert!(rose_at.is_none(), "it rises once");
            rose_at = Some(tick);
        }
        if bit_at.is_none() && health(&region, target) < full {
            bit_at = Some(tick);
            assert_eq!(health(&region, target), full - 6.0);
        }
        if gone(&output, fangs_key) {
            gone_at = Some(tick);
            break;
        }
    }
    assert_eq!(rose_at, Some(warmup + 1));
    assert_eq!(bit_at, Some(warmup + 8));
    assert_eq!(gone_at, Some(warmup + 23));
}

/// `dealDamageTo`: the fangs' owner is spared.
#[test]
fn fangs_spare_their_owner() {
    let mut region = world(SEED, nothing);
    let owner = a_cow_at(&mut region, [4.5, -60.0, 4.5]);
    let full = health(&region, owner);
    let chunk = ChunkPos::new(0, 0);
    let fangs_key = region.take_entity_key(chunk).expect("a key");
    let fangs = Entity {
        key: fangs_key,
        kind: kind(entity_types::EVOKER_FANGS),
        position: [4.5, -60.0, 4.5],
        data: EntityData::Fangs(Fangs {
            owner: Some(owner),
            warmup: 0,
        }),
        ..cow(fangs_key, [4.5, -60.0, 4.5])
    };
    add(&mut region, fangs);
    for _ in 0..10 {
        idle(&mut region);
    }
    assert_eq!(health(&region, owner), full);
}

// The llama's spit.

/// `LlamaSpit.onHitEntity`: a hurt of 1 by `spit` where its owner is a living entity,
/// and it flies on; at a block it is gone.
#[test]
fn a_spit_hurts_a_cow_by_one_and_flies_on_and_is_gone_at_a_block() {
    let wall = |pos: BlockPos| (pos.x == 8 && pos.y > -61).then_some(clustine_data::blocks::STONE);
    let mut region = world(SEED, wall);
    let owner = a_cow_at(&mut region, [-6.5, -60.0, 0.5]);
    let target = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let full = health(&region, target);
    let making = Making {
        kind: kind(entity_types::LLAMA_SPIT),
        at: [0.5, -59.5, 0.5],
        data: EntityData::Spit(Spit {
            owner: Some(owner),
            left_owner: false,
            has_been_shot: false,
        }),
        aim: Aim::Direction([1.0, 0.0, 0.0]),
        power: 1.5,
        uncertainty: 0.0,
    };
    let key = region
        .make_projectile(making, |_, _| {}, &mut TickOutput::default())
        .expect("made");
    // At 0.5 it is shot; its hit test runs from where it is before it moves.
    let first = idle(&mut region);
    assert!(!gone(&first, key));
    let (_, spit) = region.find_entity(key).expect("flying");
    assert_eq!(spit.position, [2.0, -59.5, 0.5]);
    assert_eq!(
        spit.motion.map(f64::to_bits),
        [1.5 * f64::from(0.99_f32), -0.06, 0.0].map(f64::to_bits)
    );
    let second = idle(&mut region);
    assert!(!gone(&second, key), "it flies on after a hit");
    assert_eq!(health(&region, target), full - 1.0);
    let outputs = until_gone(&mut region, key, 10);
    assert!(!outputs.is_empty());
}

/// The llama's aim (`Llama.spit`): beside its mouth along its facing, at a third of
/// its target's height, lifted by a fifth of the way across.
#[test]
fn a_llama_spits_from_beside_its_mouth_at_a_third_of_its_target_s_height() {
    let target = crate::mobs::ai::sensing::Looked {
        key: EntityId(2),
        kind: Some(kind(entity_types::COW)),
        mob: true,
        position: [3.5, 64.0, 4.5],
        bounds: crate::physics::aabb::Aabb::of_entity([3.5, 64.0, 4.5], 0.9, 1.4),
        eye_y: 65.3,
        alive: true,
        sneaking: false,
        creative: false,
        spectator: false,
        vehicle: None,
        main_hand: None,
        off_hand: None,
        yaw: 0.0,
        pitch: 0.0,
        copied: false,
    };
    // Facing south (yaw 0): along +z.
    let [at, direction] =
        crate::mobs::types::llama::spit_aim([0.5, 64.0, 0.5], 0.9, 65.7, 0.0, &target);
    // `(width + 1f) · 0.5`, the sum a float.
    let reach = f64::from(0.9_f32 + 1.0) * 0.5;
    assert_eq!(at, [0.5, 65.7 - 0.100_000_001_490_116_12, 0.5 + reach]);
    let height = target.bounds.max[1] - target.bounds.min[1];
    let dy = 64.0 + height / 3.0 - at[1];
    let lift = (3.0_f64 * 3.0 + 4.0 * 4.0).sqrt() * 0.200_000_002_980_232_24;
    assert_eq!(direction, [3.0, dy + lift, 4.0]);
}

/// The margin the splash grows boxes by is the hit test's, of the potion's `tickCount`.
#[test]
fn the_splash_s_margin_is_the_hit_test_s() {
    assert_eq!(margin(2), 0.0);
    assert!((margin(5) - 0.15).abs() < 1.0e-6);
}
