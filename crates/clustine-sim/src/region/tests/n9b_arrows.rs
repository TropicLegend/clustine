//! Arrows and tridents from the specification, by someone who wrote none of their code
//! (`docs/groundwork/n9-plan.md`, sections 4.2 and 4.3, read with the dumps' findings,
//! D9 1.1 and 1.2): a refused arrow living on and falling, an enderman refusing an arrow
//! that flies on and keeps the fire it gave, a passenger enderman hurt, a piercing crit
//! arrow's draws in its targets' order, a wooden plate and a stone one, lava, a
//! trident's pick-up by its owner alone, its bounce off a creative player and its flight
//! on through an enderman.
//!
//! The worlds are those of the tests of TNT: a stone floor whose top is y = -60. An
//! arrow's own stream in its tick is `Random::of_entity(seed, key, lived)` for the
//! `lived` its tick began at.

use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::components::Component;
use clustine_world::{
    Arrow, ArrowKind, ChunkPos, Double, Entity, EntityData, EntityId, InGround, ItemStack, Pickup,
    Riding, Seat,
};

use super::thrown::{cow, health, lay, mob};
use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::region::projectiles::{Aim, Making};
use crate::region::random_ticks::Random;
use crate::region::tnt::tests::{add_player, idle, nothing, state, world};

const SEED: u64 = 0x0B19_A770;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type's id")
}

/// An arrow as the game's `(type, level)` constructor and a test's owner leave it:
/// base damage 2, the arrow's hit sound, nothing pierced, not in the ground.
fn plain(owner: Option<EntityId>, pickup: Pickup) -> Arrow {
    Arrow {
        pickup,
        pickup_stack: ItemStack::new(items::ARROW, 1),
        weapon: None,
        owner,
        left_owner: false,
        has_been_shot: false,
        base_damage: Double(2.0),
        crit: false,
        pierce_level: 0,
        pierced: Vec::new(),
        hit_sound: "minecraft:entity.arrow.hit".to_owned(),
        in_ground: None,
        shake_until: 0,
        of_kind: ArrowKind::Arrow,
    }
}

/// A trident as a player's throw makes it (no weapon, the trident's own ground sound).
fn trident(owner: Option<EntityId>) -> Arrow {
    Arrow {
        pickup_stack: ItemStack::new(items::TRIDENT, 1),
        hit_sound: "minecraft:item.trident.hit_ground".to_owned(),
        of_kind: ArrowKind::Trident {
            dealt_damage: false,
            returning: 0,
        },
        ..plain(owner, Pickup::Allowed)
    }
}

/// `arrow` of the type `of` at `at`, shot along `direction` at `power` with no
/// uncertainty, so that its motion is the direction made of length `power`; alight for
/// `fire` ticks.
fn shoot(
    region: &mut Region,
    of: i32,
    at: [f64; 3],
    direction: [f64; 3],
    power: f32,
    arrow: Arrow,
    fire: i32,
) -> EntityId {
    let making = Making {
        kind: kind(of),
        at,
        data: EntityData::Arrow(Box::new(arrow)),
        aim: Aim::Direction(direction),
        power,
        uncertainty: 0.0,
    };
    let key = region
        .make_projectile(making, |_, _| {}, &mut TickOutput::default())
        .expect("the arrow is made");
    if fire > 0 {
        let (_, made) = region.find_entity(key).expect("made here");
        let lit = Entity {
            fire_ticks: fire,
            ..made.clone()
        };
        lay(region, &lit);
    }
    key
}

/// A mob of the type `of` at `feet` at its type's full health, standing, with no AI.
fn mob_of(region: &mut Region, of: i32, feet: [f64; 3]) -> Entity {
    let kind = kind(of);
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    let mut mob = Mob::fresh();
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(
        kind,
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a health") as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.body.flags.no_ai = true;
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 3),
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
        riding: Riding::default(),
        data: mob.encode(),
    }
}

fn a_cow_at(region: &mut Region, feet: [f64; 3]) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    lay(region, &cow(key, feet));
    key
}

fn the(region: &Region, key: EntityId) -> Entity {
    region.find_entity(key).expect("it is here").1.clone()
}

fn arrow_of(entity: &Entity) -> Arrow {
    match &entity.data {
        EntityData::Arrow(arrow) => (**arrow).clone(),
        other => panic!("not an arrow: {other:?}"),
    }
}

/// The pitches the sound `sound` was heard at in `output`.
fn pitches(output: &TickOutput, sound: i32) -> Vec<f32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: heard,
                pitch,
                ..
            } if *heard == sound => Some(*pitch),
            _ => None,
        })
        .collect()
}

/// `1.2f / (f · 0.2f + 0.9f)`: the pitch of a hit's sound for the draw `f` (n9
/// AbstractArrow 1192–1199).
fn hit_pitch(f: f32) -> f32 {
    1.2 / (f * 0.2 + 0.9)
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1.0e-9)
}

/// D9 1.2 rows 1 and 4: a hurt refused turns the arrow back at a tenth of its speed
/// (`deflect(REVERSE, …, 0.2)`: × −0.5 then × 0.2), its yaw turned by `170 + 20 · f`
/// of one own draw, and it **lives on**: it is discarded only below a speed² of 1e-7.
/// It then falls back to the ground, where it stays. The brief for this test says
/// Clustine was seen removing it.
#[test]
fn an_arrow_a_creative_player_refuses_lives_on_falls_back_and_lies_in_the_ground() {
    let mut region = world(SEED, nothing);
    add_player(&mut region, 1, [3.5, -60.0, 0.5], false);
    // Disallowed, so that the player standing beside where it lands cannot take it.
    let key = shoot(
        &mut region,
        entity_types::ARROW,
        [1.5, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        plain(None, Pickup::Disallowed),
        0,
    );
    let shot = the(&region, key);
    assert_eq!(shot.motion, [2.0, 0.0, 0.0]);
    let output = idle(&mut region);
    let draw = Random::of_entity(SEED, key, shot.lived).next_float();
    let after = region
        .find_entity(key)
        .unwrap_or_else(|| panic!("a refused arrow lives on: {:?}", output.events))
        .1
        .clone();
    // `v · −0.5 · 0.2`, then the air's 0.99 and gravity's 0.05 of the same tick.
    let drag = f64::from(0.99_f32);
    let back = [2.0 * -0.5 * 0.2 * drag, -0.05, 0.0];
    assert!(close(after.motion, back), "{:?}", after.motion);
    // Shot along x its yaw is 90, and the turn adds `170 + 20 · f` to it.
    let yaw = 90.0_f32 + (170.0_f32 + draw * 20.0);
    assert!(
        (after.yaw - yaw).abs() < 1.0e-3,
        "{} against {yaw}",
        after.yaw
    );
    // Stopped at the player's box, short of it.
    assert!(after.position[0] < 3.2, "{:?}", after.position);
    assert!(pitches(&output, sounds::ENTITY_ARROW_HIT).is_empty());
    // It falls back the way it came and lies in the floor, tick after tick.
    let mut landed = None;
    for tick in 1..=60 {
        idle(&mut region);
        let now = region
            .find_entity(key)
            .unwrap_or_else(|| panic!("the arrow is gone at tick {tick} after its refusal"))
            .1
            .clone();
        assert!(now.position[0] <= after.position[0], "{:?}", now.position);
        if arrow_of(&now).in_ground.is_some() && landed.is_none() {
            landed = Some(tick);
        }
    }
    let landed = landed.expect("it reached the floor");
    let lying = the(&region, key);
    assert!(
        (lying.position[1] + 60.0).abs() < 0.1,
        "on the floor: {:?} (landed after {landed} ticks)",
        lying.position
    );
}

/// D9 1.2 row 11 and 5.6, the plan's 4.2 item 3: an enderman refuses every projectile
/// that is not its vehicle's passenger's, and receives no side effects: no bounce, no
/// sound, the arrow flies on; the fire the arrow set **before** the hurt is kept; and
/// the owner's `setLastHurtMob` was done before the hurt, whatever came of it.
#[test]
fn an_arrow_flies_on_through_an_enderman_that_keeps_its_fire_and_is_its_owner_s_last_hurt() {
    let mut region = world(SEED, nothing);
    let (id, shooter) = add_player(&mut region, 1, [-6.5, -60.0, 0.5], false);
    let enderman = mob_of(&mut region, entity_types::ENDERMAN, [3.5, -60.0, 0.5]);
    lay(&mut region, &enderman);
    let full = health(&region, enderman.key);
    let key = shoot(
        &mut region,
        entity_types::ARROW,
        [1.5, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        3.0,
        plain(Some(shooter), Pickup::Disallowed),
        200,
    );
    let output = idle(&mut region);
    assert_eq!(
        health(&region, enderman.key),
        full,
        "an arrow never hurts it"
    );
    let burnt = the(&region, enderman.key);
    assert!(
        burnt.fire_ticks >= 95,
        "the arrow's five seconds of fire are kept: {}",
        burnt.fire_ticks
    );
    let flying = region
        .find_entity(key)
        .unwrap_or_else(|| panic!("the arrow flies on: {:?}", output.events))
        .1
        .clone();
    assert!(
        flying.motion[0] > 2.0,
        "not turned back: {:?}",
        flying.motion
    );
    assert!(arrow_of(&flying).in_ground.is_none());
    assert!(pitches(&output, sounds::ENTITY_ARROW_HIT).is_empty());
    let player = region.players.get(&id).expect("the shooter");
    assert_eq!(
        player.last_hurt_mob.map(|(mob, _)| mob),
        Some(enderman.key),
        "the owner marks the enderman as the last it hurt"
    );
}

/// The plan's 4.2 item 3 and finding 25: only an enderman that rides nothing refuses a
/// projectile; one riding a minecart is hurt by the arrow as any mob, and the arrow,
/// whose hurt took, is gone with its sound.
#[test]
fn an_enderman_riding_a_minecart_is_hurt_by_an_arrow() {
    let mut region = world(SEED, nothing);
    let at = [3.5, -60.0, 0.5];
    let mut cart = crate::region::tnt::tests::record(
        &mut region,
        entity_types::MINECART,
        at,
        EntityData::Minecart {
            hurt: Default::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents: Vec::new(),
            fuel: 0,
            push: [Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
    );
    let mut enderman = mob_of(&mut region, entity_types::ENDERMAN, at);
    cart.riding.passengers = vec![Seat {
        key: enderman.key,
        player: false,
        pending: None,
    }];
    enderman.riding.vehicle = Some(cart.key);
    lay(&mut region, &cart);
    lay(&mut region, &enderman);
    idle(&mut region);
    let seated = the(&region, enderman.key);
    assert_eq!(seated.riding.vehicle, Some(cart.key), "it still rides");
    let full = health(&region, enderman.key);
    // Aimed at its middle, at 2 along x: `ceil(2 · 2) = 4`.
    let middle = seated.position[1] + 1.45;
    let key = shoot(
        &mut region,
        entity_types::ARROW,
        [seated.position[0] - 2.0, middle, seated.position[2]],
        [1.0, 0.0, 0.0],
        2.0,
        plain(None, Pickup::Disallowed),
        0,
    );
    let shot = the(&region, key);
    let output = idle(&mut region);
    assert_eq!(health(&region, enderman.key), full - 4.0);
    assert!(
        region.find_entity(key).is_none(),
        "its hurt took: it is gone"
    );
    let pitch = hit_pitch(Random::of_entity(SEED, key, shot.lived).next_float());
    assert_eq!(pitches(&output, sounds::ENTITY_ARROW_HIT), [pitch]);
}

/// D9 1.1, `stepMoveAndHit` and `onHitEntity`: a piercing arrow hits every entity along
/// its step, sorted by the distance to each entity's position, and for each in turn
/// draws its crit (`nextInt(j / 2 + 2)`) before the hurt and its sound's pitch after
/// it took. The farther cow has the lower key, so that an order by key would differ.
#[test]
fn a_piercing_crit_arrow_draws_each_cow_s_crit_then_its_sound_nearest_first() {
    let mut region = world(SEED, nothing);
    let far = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let near = a_cow_at(&mut region, [2.5, -60.0, 0.5]);
    assert!(far < near);
    let full = health(&region, near);
    let mut arrow = plain(None, Pickup::Disallowed);
    arrow.crit = true;
    arrow.pierce_level = 1;
    // At 2 along x from 1.5: into the near cow at 2.05 and the far one at 3.05.
    let key = shoot(
        &mut region,
        entity_types::ARROW,
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        arrow,
        0,
    );
    let shot = the(&region, key);
    let output = idle(&mut region);
    let mut own = Random::of_entity(SEED, key, shot.lived);
    // `j = ceil(2 · 2) = 4`; the crit's bound `4 / 2 + 2`.
    let first = own.next_int(4);
    let first_pitch = hit_pitch(own.next_float());
    let second = own.next_int(4);
    let second_pitch = hit_pitch(own.next_float());
    assert_eq!(health(&region, near), full - (4 + first) as f32);
    assert_eq!(health(&region, far), full - (4 + second) as f32);
    assert_eq!(
        pitches(&output, sounds::ENTITY_ARROW_HIT),
        [first_pitch, second_pitch]
    );
    // It pierced both and flies on, keeping both in its set.
    let flying = arrow_of(&the(&region, key));
    assert_eq!(flying.pierced, [near, far]);
}

/// The plan's 4.2, `applyEffectsFromBlocks` (finding 21): an arrow lying in a wooden
/// pressure plate's cell presses it, as every entity does; a stone plate answers living
/// entities only and stays up.
#[test]
fn an_arrow_lying_on_a_wooden_plate_presses_it_and_one_on_a_stone_plate_does_not() {
    let wooden = BlockPos::new(2, -60, 2);
    let stone = BlockPos::new(6, -60, 2);
    let mut region = world(SEED, move |pos| {
        if pos == wooden {
            Some(state("minecraft:oak_pressure_plate"))
        } else if pos == stone {
            Some(state("minecraft:stone_pressure_plate"))
        } else {
            None
        }
    });
    for x in [2.5, 6.5] {
        shoot(
            &mut region,
            entity_types::ARROW,
            [x, -58.0, 2.5],
            [0.0, -1.0, 0.0],
            0.5,
            plain(None, Pickup::Disallowed),
            0,
        );
    }
    let mut pressed_at = None;
    for tick in 1..=20 {
        idle(&mut region);
        let plate = region.block(wooden).expect("loaded");
        if pressed_at.is_none() && plate.property("powered") == Some("true") {
            pressed_at = Some(tick);
        }
        assert_eq!(
            region.block(stone).expect("loaded").property("powered"),
            Some("false"),
            "tick {tick}"
        );
    }
    assert!(pressed_at.is_some(), "the wooden plate was never pressed");
    assert_eq!(
        region.block(wooden).expect("loaded").property("powered"),
        Some("true"),
        "it stays pressed while the arrow lies there"
    );
}

/// The plan's 4.2 (finding 21): `applyEffectsFromBlocks` on every flying step is what
/// sets an arrow alight in lava (`LAVA_IGNITE`, fifteen seconds).
#[test]
fn an_arrow_flying_through_lava_is_set_alight() {
    let mut region = world(SEED, |pos| {
        (pos.y == -60 && (3..=5).contains(&pos.x) && pos.z == 0).then_some(blocks::LAVA)
    });
    let key = shoot(
        &mut region,
        entity_types::ARROW,
        [0.5, -59.6, 0.5],
        [1.0, 0.0, 0.0],
        1.0,
        plain(None, Pickup::Disallowed),
        0,
    );
    let mut alight = false;
    for _ in 0..8 {
        idle(&mut region);
        let Some((_, now)) = region.find_entity(key) else {
            break;
        };
        if now.fire_ticks > 200 {
            alight = true;
            break;
        }
    }
    assert!(alight, "the arrow never caught fire in the lava");
}

/// D9 1.1 "ThrownTrident" `playerTouch` (n9 TT 479–491): a trident lying in the ground is
/// picked up only by its owner (or by anyone if it has none); another player standing on
/// it takes nothing.
#[test]
fn a_trident_in_the_ground_is_picked_up_by_its_owner_and_by_no_one_else() {
    let mut region = world(SEED, nothing);
    let (other_id, _) = add_player(&mut region, 2, [2.5, -60.0, 2.5], false);
    let (owner_id, owner) = add_player(&mut region, 1, [40.5, -60.0, 40.5], false);
    let mut lying = trident(Some(owner));
    lying.in_ground = Some(InGround {
        state: blocks::STONE.0,
        since: 10,
    });
    let key = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
    let entity = Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::TRIDENT),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 5),
        position: [2.5, -60.05, 2.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 100,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Riding::default(),
        data: EntityData::Arrow(Box::new(lying)),
    };
    lay(&mut region, &entity);
    let mut output = TickOutput::default();
    region.players_touch_arrows(&mut output);
    assert!(region.find_entity(key).is_some(), "not taken by another");
    let held = |region: &Region, id| {
        region
            .players
            .get(&id)
            .expect("a player")
            .inventory
            .0
            .iter()
            .flatten()
            .any(|stack: &ItemStack| stack.item == items::TRIDENT)
    };
    assert!(!held(&region, other_id));
    // Its owner comes and stands on it.
    region
        .players
        .get_mut(&owner_id)
        .expect("the owner")
        .pose
        .position = clustine_world::Vec3::new(2.5, -60.0, 2.5);
    let mut output = TickOutput::default();
    region.players_touch_arrows(&mut output);
    assert!(region.find_entity(key).is_none(), "its owner took it");
    assert!(held(&region, owner_id));
}

/// D9 1.2 rows 2 and 14: a trident deals its damage (`dealtDamage` set before the hurt)
/// and bounces by REVERSE scaled (0.02, 0.2, 0.02) with one own draw **whether or not
/// the hurt took**: off a creative player, who refuses it, with `trident_hit`.
#[test]
fn a_trident_bounces_off_a_creative_player_who_refuses_it() {
    let mut region = world(SEED, nothing);
    add_player(&mut region, 1, [3.5, -60.0, 0.5], false);
    let key = shoot(
        &mut region,
        entity_types::TRIDENT,
        [1.5, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        trident(None),
        0,
    );
    let shot = the(&region, key);
    let output = idle(&mut region);
    let after = the(&region, key);
    let drag = f64::from(0.99_f32);
    // `v · −0.5 · (0.02, 0.2, 0.02)`, then the drag and gravity.
    let back = [2.0 * -0.5 * 0.02 * drag, -0.05, 0.0];
    assert!(close(after.motion, back), "{:?}", after.motion);
    let draw = Random::of_entity(SEED, key, shot.lived).next_float();
    let yaw = 90.0_f32 + (170.0_f32 + draw * 20.0);
    assert!(
        (after.yaw - yaw).abs() < 1.0e-3,
        "{} against {yaw}",
        after.yaw
    );
    assert_eq!(pitches(&output, sounds::ITEM_TRIDENT_HIT), [1.0]);
    assert!(matches!(
        arrow_of(&after).of_kind,
        ArrowKind::Trident {
            dealt_damage: true,
            ..
        }
    ));
}

/// The plan's 4.3 (finding 6; n9 ThrownTrident 376–399, Enderman 973–976): at an
/// enderman that refused it, a trident gets no side effects: no sound, no bounce; it
/// flies on with its damage dealt, and so hits nothing after (a cow behind is spared).
#[test]
fn a_trident_flies_on_through_an_enderman_silently_and_hits_nothing_after() {
    let mut region = world(SEED, nothing);
    let enderman = mob_of(&mut region, entity_types::ENDERMAN, [3.5, -60.0, 0.5]);
    lay(&mut region, &enderman);
    let behind = a_cow_at(&mut region, [6.5, -60.0, 0.5]);
    let full = health(&region, behind);
    let key = shoot(
        &mut region,
        entity_types::TRIDENT,
        [1.5, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        2.5,
        trident(None),
        0,
    );
    let output = idle(&mut region);
    let after = the(&region, key);
    assert!(after.motion[0] > 2.0, "no bounce: {:?}", after.motion);
    assert!(pitches(&output, sounds::ITEM_TRIDENT_HIT).is_empty());
    assert!(matches!(
        arrow_of(&after).of_kind,
        ArrowKind::Trident {
            dealt_damage: true,
            ..
        }
    ));
    for _ in 0..4 {
        idle(&mut region);
    }
    assert_eq!(
        health(&region, behind),
        full,
        "a trident that dealt its damage hits nothing"
    );
    assert_eq!(mob(&region, enderman.key).body.vitals.health, 40.0);
}

/// D9 1.1 "Arrow" and the plan's 4.2: a tipped arrow's potion is its pickup stack's
/// `potion_contents`, scaled by its `potion_duration_scale` (the tipped arrow's data:
/// 0.125), and `withScaledDuration` floors: 900 ticks of poison become 112.
#[test]
fn a_tipped_arrow_s_poison_lasts_an_eighth_floored() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let mut stack = ItemStack::new(items::TIPPED_ARROW, 1);
    stack.components.set(Component::PotionContents(
        clustine_world::components::PotionContents {
            potion: Some("minecraft:poison".to_owned()),
            ..Default::default()
        },
    ));
    let mut arrow = plain(None, Pickup::Disallowed);
    arrow.pickup_stack = stack;
    shoot(
        &mut region,
        entity_types::ARROW,
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        arrow,
        0,
    );
    idle(&mut region);
    let hit = mob(&region, target);
    let lived = the(&region, target).lived;
    let poison = crate::mobs::body::effects::Effect::POISON;
    let active = hit
        .body
        .effects
        .active
        .iter()
        .find(|active| active.effect == poison)
        .expect("poisoned");
    // Given between the cow's ticks or in its tick: 112 from the moment it was given,
    // so 111 or 112 left as the tick ends.
    let left = active.top().duration(u64::from(lived));
    assert!((111..=112).contains(&left), "{left}");
}

/// A whole arrow record of the type `of` lying stuck in the stone floor at `x`, `z`
/// since the `lived` `since`, having lived `lived`.
fn stuck(
    region: &mut Region,
    of: i32,
    x: f64,
    z: f64,
    lived: u32,
    since: u32,
    arrow: Arrow,
) -> Entity {
    let key = region
        .take_entity_key(ChunkPos::containing(x, z))
        .expect("a key");
    let mut arrow = arrow;
    arrow.in_ground = Some(InGround {
        state: blocks::STONE.0,
        since,
    });
    let entity = Entity {
        portal: Default::default(),
        key,
        kind: kind(of),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 9),
        position: [x, -60.05, z],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Riding::default(),
        data: EntityData::Arrow(Box::new(arrow)),
    };
    lay(region, &entity);
    entity
}

/// D9 1.1 step 5: an arrow in the ground returns before `Projectile.tick`, so its base
/// tick does not run and its fire does not count down: a burning arrow lying in the
/// ground burns on, and (its counters derived) commits nothing as it lies.
#[test]
fn a_burning_arrow_lying_in_the_ground_burns_on_and_commits_nothing() {
    let mut region = world(SEED, nothing);
    let mut lying = stuck(
        &mut region,
        entity_types::ARROW,
        2.5,
        2.5,
        40,
        30,
        plain(None, Pickup::Disallowed),
    );
    lying.fire_ticks = 100;
    lay(&mut region, &lying);
    for tick in 1..=30 {
        let output = idle(&mut region);
        let now = the(&region, lying.key);
        assert_eq!(now.fire_ticks, 100, "tick {tick}");
        assert!(
            !output
                .entity_changes
                .iter()
                .any(|(_, change)| change.key() == lying.key),
            "tick {tick}: a lying arrow commits nothing"
        );
    }
}

/// D9 1.1 `onHitBlock` (n9 AbstractArrow 1317–1408) and the plan's 4.2 item 4: at a block
/// an arrow's crit and pierce are taken off, its pierced set cleared, and its hit sound
/// set back to `arrow_hit` (a crossbow's arrow had `crossbow_hit`).
#[test]
fn a_crossbow_s_piercing_crit_arrow_stuck_in_a_wall_is_a_plain_arrow_again() {
    let wall =
        |pos: BlockPos| (pos.x == 4 && (-60..=-58).contains(&pos.y)).then_some(blocks::STONE);
    let mut region = world(SEED, wall);
    let mut arrow = plain(None, Pickup::Disallowed);
    arrow.crit = true;
    arrow.pierce_level = 2;
    arrow.pierced = vec![EntityId(424_242)];
    arrow.hit_sound = "minecraft:item.crossbow.hit".to_owned();
    arrow.weapon = Some(ItemStack::new(items::CROSSBOW, 1));
    let key = shoot(
        &mut region,
        entity_types::ARROW,
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        1.0,
        arrow,
        0,
    );
    for _ in 0..6 {
        idle(&mut region);
    }
    let stuck = arrow_of(&the(&region, key));
    assert!(stuck.in_ground.is_some(), "it stuck in the wall");
    assert!(!stuck.crit);
    assert_eq!(stuck.pierce_level, 0);
    assert!(stuck.pierced.is_empty());
    assert_eq!(stuck.hit_sound, "minecraft:entity.arrow.hit");
}

fn loyal_trident(owner: Option<EntityId>, pickup: Pickup) -> Arrow {
    let mut stack = ItemStack::new(items::TRIDENT, 1);
    stack.components.set(Component::Enchantments(vec![(
        "minecraft:loyalty".to_owned(),
        2,
    )]));
    Arrow {
        pickup,
        pickup_stack: stack,
        of_kind: ArrowKind::Trident {
            dealt_damage: true,
            returning: 0,
        },
        ..trident(owner)
    }
}

/// D9 1.1 `tickDespawn` (n9 TT 550–567): a trident with loyalty that may be picked up
/// (ALLOWED) never despawns; one that may not goes at 1200 ticks in the ground as any
/// arrow.
#[test]
fn a_loyal_trident_that_may_be_picked_up_never_despawns_and_one_that_may_not_does() {
    let mut region = world(SEED, nothing);
    let kept = stuck(
        &mut region,
        entity_types::TRIDENT,
        2.5,
        2.5,
        1190,
        0,
        loyal_trident(None, Pickup::Allowed),
    );
    let lost = stuck(
        &mut region,
        entity_types::TRIDENT,
        6.5,
        2.5,
        1190,
        0,
        loyal_trident(None, Pickup::Disallowed),
    );
    for _ in 0..40 {
        idle(&mut region);
    }
    assert!(
        region.find_entity(kept.key).is_some(),
        "an allowed loyal trident stays"
    );
    assert!(
        region.find_entity(lost.key).is_none(),
        "a disallowed one is gone at 1200"
    );
}

/// D9 1.2 row 10 (n9 ThrownTrident 7, 569–572): a trident's water inertia is 0.99, where
/// an arrow's is 0.6.
#[test]
fn a_trident_in_water_keeps_ninety_nine_hundredths_of_its_motion() {
    let water = |pos: BlockPos| {
        ((-60..=-58).contains(&pos.y) && (0..=12).contains(&pos.x) && (-1..=1).contains(&pos.z))
            .then_some(blocks::WATER)
    };
    let mut region = world(SEED, water);
    let key = shoot(
        &mut region,
        entity_types::TRIDENT,
        [0.5, -59.0, 0.5],
        [1.0, 0.0, 0.0],
        1.0,
        trident(None),
        0,
    );
    let mut motions = Vec::new();
    for _ in 0..4 {
        idle(&mut region);
        motions.push(the(&region, key).motion);
    }
    // From the second tick on it knows it is in water.
    for pair in motions[1..].windows(2) {
        let ratio = pair[1][0] / pair[0][0];
        assert!((ratio - f64::from(0.99_f32)).abs() < 1.0e-9, "{motions:?}");
    }
}

/// D9 1.1 "SpectralArrow" (n9 SpectralArrow 77–96): a spectral arrow's hurt that took
/// gives the target glowing for its duration (200), amplifier 0, and the arrow is gone.
#[test]
fn a_spectral_arrow_makes_the_cow_it_hurts_glow_for_ten_seconds() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let arrow = Arrow {
        pickup_stack: ItemStack::new(items::SPECTRAL_ARROW, 1),
        of_kind: ArrowKind::Spectral {
            glowing_duration: 200,
        },
        ..plain(None, Pickup::Disallowed)
    };
    let key = shoot(
        &mut region,
        entity_types::SPECTRAL_ARROW,
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        arrow,
        0,
    );
    idle(&mut region);
    assert!(region.find_entity(key).is_none());
    let hit = mob(&region, target);
    let glowing = crate::mobs::body::effects::Effect::GLOWING;
    let active = hit
        .body
        .effects
        .active
        .iter()
        .find(|active| active.effect == glowing)
        .expect("it glows");
    assert_eq!(active.top().amplifier, 0);
    let left = active.top().duration(u64::from(the(&region, target).lived));
    assert!((199..=200).contains(&left), "{left}");
}
