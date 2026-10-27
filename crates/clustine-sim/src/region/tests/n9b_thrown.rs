//! Thrown items, the cloud, the fangs and a dispensed rocket from the specification, by
//! someone who wrote none of their code (`docs/groundwork/n9-plan.md`, sections 4.4,
//! 4.5, 4.7, 4.8 and 4.9, with D9 1.1, 2.4 and 2.5): a splash's effect of exactly 20
//! ticks dropped and one of 21 kept; a lingering water bottle making no cloud; a cloud
//! whose radius shrinks within one pass of its targets; harming splashed on an undead
//! and on a creative player; the fangs' box; a dispensed rocket's straight flight and
//! its lifetime's draws.

use clustine_data::{entity_types, items};
use clustine_world::components::{Component, EffectInstance, PotionContents};
use clustine_world::projectiles::Cloud;
use clustine_world::{ChunkPos, Entity, EntityData, EntityId, Float, ItemStack};

use super::thrown::{cow, health, lay, mob};
use super::*;
use crate::mobs::body::effects::Effect;
use crate::region::projectiles::{Aim, Making};
use crate::region::random_ticks::Random;
use crate::region::tnt::tests::{add_player, idle, nothing, records, state, world};

const SEED: u64 = 0x7480_3B0B;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type's id")
}

fn a_cow_at(region: &mut Region, feet: [f64; 3]) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    lay(region, &cow(key, feet));
    key
}

/// A zombie standing at `feet` with `health`, with no AI.
fn a_zombie_at(region: &mut Region, feet: [f64; 3], health: f32) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    let mut zombie = cow(key, feet);
    zombie.kind = kind(entity_types::ZOMBIE);
    let mut body = crate::mobs::Mob::decode(&zombie.data).expect("a mob");
    body.body.vitals.health = health;
    zombie.data = body.encode();
    lay(region, &zombie);
    key
}

fn of_kind(region: &Region, of: i32) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == of)
        .collect()
}

/// A thrown item of the type `of` carrying `stack`, of nobody, shot from `at` along
/// `direction` at `power` with no uncertainty.
fn throw(
    region: &mut Region,
    of: i32,
    stack: ItemStack,
    at: [f64; 3],
    direction: [f64; 3],
    power: f32,
) -> EntityId {
    let making = Making {
        kind: kind(of),
        at,
        data: EntityData::Thrown {
            stack,
            owner: None,
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

fn effect(name: &str, amplifier: i32, duration: i32) -> EffectInstance {
    EffectInstance {
        effect: name.to_owned(),
        amplifier,
        duration,
        ambient: false,
        visible: true,
        show_icon: true,
    }
}

/// A stack of `item` whose potion contents are `potion` and `custom`.
fn potion_stack(item: i32, potion: Option<&str>, custom: Vec<EffectInstance>) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack
        .components
        .set(Component::PotionContents(PotionContents {
            potion: potion.map(str::to_owned),
            custom_effects: custom,
            ..PotionContents::default()
        }));
    stack
}

/// Ticks until the projectile `key` is gone, at most `ticks`.
fn until_gone(region: &mut Region, key: EntityId, ticks: usize) {
    for _ in 0..ticks {
        idle(region);
        if region.find_entity(key).is_none() {
            return;
        }
    }
    panic!("the projectile was still there after {ticks} ticks");
}

/// D9 2.4 and finding 10: a splash's effect whose duration, scaled by its proximity
/// (`(int)(prox · duration · scale + 0.5)`), `endsWithin(20)` (`duration <= 20`) is not
/// given. A potion breaking on the cow's own top is at proximity 1: 20 ticks of slowness
/// are dropped, 21 of swiftness kept.
#[test]
fn a_splash_drops_an_effect_of_exactly_twenty_ticks_and_gives_one_of_twenty_one() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [2.5, -60.0, 2.5]);
    let stack = potion_stack(
        items::SPLASH_POTION,
        None,
        vec![
            effect("minecraft:slowness", 0, 20),
            effect("minecraft:speed", 0, 21),
        ],
    );
    let key = throw(
        &mut region,
        entity_types::SPLASH_POTION,
        stack,
        [2.5, -57.5, 2.5],
        [0.0, -1.0, 0.0],
        0.5,
    );
    until_gone(&mut region, key, 10);
    let hit = mob(&region, target);
    assert!(
        !hit.body.effects.has(Effect::SLOWNESS),
        "20 ticks are dropped"
    );
    assert!(hit.body.effects.has(Effect::SPEED), "21 ticks are given");
}

/// D9 2.4: "a water bottle has no effects, so a lingering water bottle makes no cloud",
/// and a lingering potion with effects makes one.
#[test]
fn a_lingering_water_bottle_makes_no_cloud_and_a_lingering_potion_does() {
    for (potion, clouds) in [("minecraft:water", 0), ("minecraft:swiftness", 1)] {
        let mut region = world(SEED, nothing);
        let key = throw(
            &mut region,
            entity_types::LINGERING_POTION,
            potion_stack(items::LINGERING_POTION, Some(potion), Vec::new()),
            [0.5, -59.0, 0.5],
            [0.0, -1.0, 0.0],
            0.5,
        );
        until_gone(&mut region, key, 10);
        assert_eq!(
            of_kind(&region, entity_types::AREA_EFFECT_CLOUD).len(),
            clouds,
            "{potion}"
        );
    }
}

/// A lingering potion's cloud (D9 2.4) of swiftness made at `at`, as the potion makes it:
/// radius 3, less half a block a use and 3/600 a tick, waiting 10 ticks, for 600.
fn lingering_cloud(region: &mut Region, at: [f64; 3]) -> EntityId {
    let cloud = Cloud {
        duration: 600,
        wait_time: 10,
        reapplication_delay: 20,
        radius: Float(3.0),
        radius_at: 0,
        radius_on_use: Float(-0.5),
        radius_per_tick: Float(-3.0 / 600.0),
        duration_on_use: 0,
        potion: PotionContents {
            potion: Some("minecraft:swiftness".to_owned()),
            ..PotionContents::default()
        },
        duration_scale: Float(0.25),
        owner: None,
        victims: Vec::new(),
    };
    region
        .make_cloud(at, cloud, &mut TickOutput::default())
        .expect("a cloud")
}

/// D9 2.5: "`r` is a local carried through the loop, so a second target in the same pass
/// is tested against the already shrunk radius". Two cows each 2.6 from the middle: at
/// the first use the radius is about 2.95, so the first of them in the region's order is
/// given swiftness and the radius falls by half a block, below 2.6, and the second is
/// not; nor is it later, the radius only shrinking.
#[test]
fn a_cloud_s_use_shrinks_its_radius_before_the_next_target_of_the_same_pass() {
    let mut region = world(SEED, nothing);
    let middle = [8.5, -60.0, 8.5];
    let east = a_cow_at(&mut region, [middle[0] + 2.6, -60.0, middle[2]]);
    let west = a_cow_at(&mut region, [middle[0] - 2.6, -60.0, middle[2]]);
    lingering_cloud(&mut region, middle);
    let mut first_given = None;
    for tick in 1..=60 {
        idle(&mut region);
        let given = [east, west].map(|key| mob(&region, key).body.effects.has(Effect::SPEED));
        let count = given.iter().filter(|given| **given).count();
        assert!(count <= 1, "tick {tick}: both were given swiftness");
        if count == 1 && first_given.is_none() {
            first_given = Some(tick);
        }
    }
    assert!(first_given.is_some(), "neither cow was given swiftness");
}

/// D9 4 and 2.4: a thrown potion of harming heals an undead (`#inverted_healing_and_harm`)
/// by `(int)(prox · (4 << amp) + 0.5)`, and a creative player, invulnerable, is hurt by
/// none of it.
#[test]
fn harming_splashed_on_a_zombie_heals_it_by_four() {
    let mut region = world(SEED, nothing);
    let zombie = a_zombie_at(&mut region, [2.5, -60.0, 2.5], 10.0);
    let key = throw(
        &mut region,
        entity_types::SPLASH_POTION,
        potion_stack(items::SPLASH_POTION, Some("minecraft:harming"), Vec::new()),
        [2.5, -57.5, 2.5],
        [0.0, -1.0, 0.0],
        0.5,
    );
    until_gone(&mut region, key, 10);
    assert_eq!(health(&region, zombie), 14.0);
}

/// D9 1.2 row 6: a fang bites every living entity in its box inflated by (0.2, 0, 0.2),
/// and an ownerless fang bites by `magic` 6. A fang is 0.5 wide, so with the inflation
/// it reaches 0.45 from its middle; a cow is 0.9 wide: one 0.85 off is bitten, one
/// 0.95 off is not.
#[test]
fn a_fang_bites_a_cow_whose_box_meets_its_own_widened_by_a_fifth_and_no_further() {
    let mut region = world(SEED, nothing);
    let at = [8.5, -60.0, 8.5];
    let near = a_cow_at(&mut region, [at[0] + 0.85, -60.0, at[2]]);
    let far = a_cow_at(&mut region, [at[0] - 0.95, -60.0, at[2]]);
    let full = health(&region, near);
    region
        .make_fangs(at, 0.0, 0, None, &mut TickOutput::default())
        .expect("fangs");
    for _ in 0..30 {
        idle(&mut region);
    }
    assert_eq!(health(&region, near), full - 6.0);
    assert_eq!(health(&region, far), full);
    assert!(of_kind(&region, entity_types::EVOKER_FANGS).is_empty());
}

/// The plan's 4.7 and 4.9 (finding 7; D9 1.1 "FireworkRocketEntity", 2.3): a dispenser
/// makes a rocket just outside its face (the middle + 0.50001 · facing, no lift), shot
/// along the facing at 0.5 with uncertainty 1.0 and `shotAtAngle` true, so it flies
/// straight, neither speeding up nor rising. Its lifetime, `10 · flight + nextInt(6) +
/// nextInt(7)` with `flight = 1 + flight_duration`, is drawn from its own stream after
/// its UUID's two longs and its two triangles, and it pops (entity event 17) at the tick
/// its life passes it. The item's own `fireworks` component (the data's default) has a
/// `flight_duration` of 1, so a plain rocket's flight is 2: D9 1.2 row 17's "flight 1,
/// 10 to 21 ticks" is of a rocket with no component at all, which no stack is.
#[test]
fn a_dispensed_rocket_flies_straight_and_pops_at_its_drawn_lifetime() {
    let at = BlockPos::new(2, -60, 2);
    let block = state("minecraft:dispenser[facing=east,triggered=false]");
    let mut region = world(SEED, move |pos| (pos == at).then_some(block));
    let mut container = region.container_at(at).expect("a dispenser");
    container.slots[0] = Some(ItemStack::new(items::FIREWORK_ROCKET, 2));
    region.set_container(at, container, &mut TickOutput::default());
    let mut output = TickOutput::default();
    let block = region.block(at).expect("loaded");
    assert!(region.dispenser_tick(at, block, &mut output));
    let rockets = of_kind(&region, entity_types::FIREWORK_ROCKET);
    let [rocket] = &rockets[..] else {
        panic!("one rocket: {rockets:?}");
    };
    assert_eq!(
        rocket.position,
        [2.5 + 0.500_009_999_999_747_4, -59.5, 2.5],
        "just outside the east face, not lifted"
    );
    let EntityData::Firework(firework) = &rocket.data else {
        panic!("a rocket's record: {:?}", rocket.data);
    };
    assert!(firework.shot_at_angle);
    // Its own stream, drawn aside in no entity's tick.
    let mut own = Random::of_entity_aside(SEED, rocket.key, 0, region.tick);
    own.next_u64();
    own.next_u64();
    for _ in 0..4 {
        own.next_double();
    }
    let lifetime = 20 + own.next_int(6) as i32 + own.next_int(7) as i32;
    assert_eq!(firework.lifetime, lifetime);
    // Shot along x at 0.5 with an uncertainty of 1: about 0.5 along x.
    let speed = rocket.motion.iter().map(|m| m * m).sum::<f64>().sqrt();
    assert!((speed - 0.5).abs() < 0.05, "{:?}", rocket.motion);
    assert!(rocket.motion[0] > 0.45, "{:?}", rocket.motion);
    let made = rocket.motion;
    let key = rocket.key;
    let mut popped = None;
    for tick in 1..=30 {
        let output = idle(&mut region);
        let pops = output.events.iter().any(|event| {
            matches!(event, RegionEvent::EntityEvent { entity, event: 17, .. } if *entity == key)
        });
        if pops {
            popped = Some(tick);
            break;
        }
        let now = region.find_entity(key).expect("still flying").1.clone();
        assert_eq!(now.motion, made, "tick {tick}: flies straight");
    }
    assert_eq!(popped, Some(lifetime + 1), "life passes its lifetime");
    assert!(region.find_entity(key).is_none());
    assert_eq!(
        region.container_at(at).expect("a dispenser").slots[0],
        Some(ItemStack::new(items::FIREWORK_ROCKET, 1))
    );
}

/// The plan's 5.4 and D9 2.4: a creative player is affected by a splash (`Player` does not
/// override `isAffectedByPotions`) but refuses every hurt, so a harming potion broken on
/// them does nothing to their health, while swiftness splashed with it is theirs.
#[test]
fn a_creative_player_splashed_with_harming_and_swiftness_takes_only_the_swiftness() {
    let mut region = world(SEED, nothing);
    let (id, _) = add_player(&mut region, 1, [2.5, -60.0, 2.5], false);
    let key = throw(
        &mut region,
        entity_types::SPLASH_POTION,
        potion_stack(
            items::SPLASH_POTION,
            Some("minecraft:harming"),
            vec![effect("minecraft:speed", 0, 400)],
        ),
        [2.5, -57.0, 2.5],
        [0.0, -1.0, 0.0],
        0.5,
    );
    until_gone(&mut region, key, 10);
    let player = region.players.get(&id).expect("the player");
    assert!(player.effects.has(Effect::SPEED), "{:?}", player.effects);
    assert!(!player.effects.has(Effect::INSTANT_DAMAGE));
}

/// An enderman at `feet`, of full health, with no AI.
fn an_enderman_at(region: &mut Region, feet: [f64; 3]) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    let mut enderman = cow(key, feet);
    enderman.kind = kind(entity_types::ENDERMAN);
    let mut body = crate::mobs::Mob::decode(&enderman.data).expect("a mob");
    body.body.vitals.health = 40.0;
    enderman.data = body.encode();
    lay(region, &enderman);
    key
}

/// An enderman at (8.5, -60, 8.5) splashed by a potion of `potion` broken a block east of
/// it: its health after, and whether it has moved more than a block (a teleport) by two
/// ticks after (Clustine runs a teleport a hurt asked for in the enderman's own tick, a
/// named difference of `mobs/types/enderman.rs`).
fn splashed_enderman(potion: &str) -> (f32, bool) {
    let mut region = world(SEED, nothing);
    let feet = [8.5, -60.0, 8.5];
    let enderman = an_enderman_at(&mut region, feet);
    let key = throw(
        &mut region,
        entity_types::SPLASH_POTION,
        potion_stack(items::SPLASH_POTION, Some(potion), Vec::new()),
        [9.5, -59.0, 8.5],
        [0.0, -1.0, 0.0],
        0.5,
    );
    until_gone(&mut region, key, 10);
    idle(&mut region);
    idle(&mut region);
    let now = region
        .find_entity(enderman)
        .expect("the enderman")
        .1
        .position;
    let moved = (now[0] - feet[0]).abs() > 1.0 || (now[2] - feet[2]).abs() > 1.0;
    (health(&region, enderman), moved)
}

/// The plan's 8.4 (finding 25; n9 Enderman 882–945): a hurt whose direct cause is a
/// thrown potion goes to `hurtWithCleanWater`, which hurts only for a potion of
/// `#hurts_water_sensitive_entities` (water), and then, whatever came of it,
/// `repeatedlyTryToTeleport`. A water splash's water behaviour (D9 2.4: `indirectMagic`
/// of 1 within 4 blocks of the potion's middle) hurts it by 1, and it teleports.
#[test]
fn a_splash_of_water_hurts_an_enderman_by_one_and_it_teleports() {
    let (health, moved) = splashed_enderman("minecraft:water");
    assert_eq!(health, 39.0);
    assert!(moved, "it teleported away");
}

/// The same rule for harming: `HealOrHarmMobEffect.applyInstantaneousEffect` hurts by
/// `indirectMagic(potion, owner)` (n9 HealOrHarmMobEffect 46–101, the potion passed as
/// the source by ThrownSplashPotion 258–273), whose direct entity is the potion, so the
/// enderman's `hurtWithCleanWater` refuses it: the plan's 8.4 says "a harming splash
/// does nothing to it". It teleports all the same.
#[test]
fn a_splash_of_harming_does_not_hurt_an_enderman_but_it_teleports() {
    let (health, moved) = splashed_enderman("minecraft:harming");
    assert_eq!(health, 40.0, "harming does nothing to an enderman");
    assert!(moved, "it teleported away");
}

/// D9 2.5: a cloud skips a target for which none of its effects passes
/// (`canBeAffected`): a zombie refuses poison, so it is no victim and its radius is not
/// spent on it. Two zombies near the middle, laid first, and a cow 2.6 out: the cow is
/// poisoned at the first use, as it would be with no zombie there.
#[test]
fn a_cloud_of_poison_spends_nothing_on_zombies_and_reaches_the_cow_beyond_them() {
    let mut region = world(SEED, nothing);
    let middle = [8.5, -60.0, 8.5];
    let zombies = [
        a_zombie_at(&mut region, [middle[0] + 0.5, -60.0, middle[2]], 20.0),
        a_zombie_at(&mut region, [middle[0] - 0.5, -60.0, middle[2] + 0.5], 20.0),
    ];
    let target = a_cow_at(&mut region, [middle[0], -60.0, middle[2] + 2.6]);
    let cloud = Cloud {
        duration: 600,
        wait_time: 10,
        reapplication_delay: 20,
        radius: Float(3.0),
        radius_at: 0,
        radius_on_use: Float(-0.5),
        radius_per_tick: Float(-3.0 / 600.0),
        duration_on_use: 0,
        potion: PotionContents {
            potion: Some("minecraft:poison".to_owned()),
            ..PotionContents::default()
        },
        duration_scale: Float(0.25),
        owner: None,
        victims: Vec::new(),
    };
    let key = region
        .make_cloud(middle, cloud, &mut TickOutput::default())
        .expect("a cloud");
    let mut poisoned = None;
    for tick in 1..=15 {
        idle(&mut region);
        if poisoned.is_none() && mob(&region, target).body.effects.has(Effect::POISON) {
            poisoned = Some(tick);
        }
    }
    assert!(poisoned.is_some(), "the cow was never poisoned");
    for zombie in zombies {
        assert!(!mob(&region, zombie).body.effects.has(Effect::POISON));
    }
    let EntityData::Cloud(cloud) = &region.find_entity(key).expect("the cloud").1.data else {
        panic!("a cloud");
    };
    assert_eq!(
        cloud
            .victims
            .iter()
            .map(|(victim, _)| *victim)
            .collect::<Vec<_>>(),
        [target],
        "only the cow is a victim"
    );
}
