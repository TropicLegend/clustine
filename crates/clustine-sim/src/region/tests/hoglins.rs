//! The hoglin and the zoglin in a region (family E6 of P12, `docs/groundwork/p-plan.md`,
//! section 18): a hoglin hurt fights back and throws what it hits, the hoglins about it
//! take its target too, a creative player is never its target nor thrown, and it becomes
//! a zoglin where piglins zombify and nowhere else.

use clustine_data::{entity_types, sounds};
use clustine_world::{Causer, Double, Entity, EntityChange, EntityId, Float, HurtBy};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::brain::{self, MemoryKind, Value};
use crate::mobs::types::{hoglin, hoglin_ai, zoglin};

/// Keyed below the hoglins, so that it ticks before them and its record holds what their
/// hits leave it with.
const PIG: EntityId = EntityId(30);
const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(41);

/// A mob of the type `kind` at `position`, of its type's full health and air.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    Entity {
        portal: Default::default(),
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

/// An adult hoglin at `position`, immune to zombification where `immune`.
fn hoglin_at(key: EntityId, position: [f64; 3], immune: bool) -> Entity {
    let mut mob = Mob::fresh();
    hoglin::update(&mut mob, |own| own.immune_to_zombification = immune);
    mob_at(key, entity_types::HOGLIN, position, mob)
}

/// A pig with no mind of its own, which neither wanders nor flees.
fn still_pig(position: [f64; 3]) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.flags.no_ai = true;
    mob_at(PIG, entity_types::PIG, position, mob)
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

/// The attack target the hoglin `key`'s brain remembers.
fn attack_target(region: &Region, key: EntityId) -> Option<EntityId> {
    let entity = record(region, key)?;
    let mob = Mob::decode(&entity.data).ok()?;
    let memories = brain::memories_of(&hoglin_ai::PROVIDER, &mob, entity.lived);
    match memories.get(MemoryKind::AttackTarget) {
        Some(Value::Entity(target)) => Some(target),
        _ => None,
    }
}

/// `mobAttack` of the pig `PIG`, of `amount`, as the hoglin `key` takes it.
fn hurt_by_the_pig(region: &mut Region, key: EntityId, amount: f32) {
    let at = record(region, PIG).unwrap().position;
    let pig = Causer {
        key: PIG,
        kind: entity_types::PIG as u16,
        position: at.map(Double),
        player: None,
        living: true,
    };
    let hurt = HurtBy {
        damage_type: "minecraft:mob_attack".to_owned(),
        cause: Some(pig.clone()),
        direct: Some(pig),
        position: None,
        projectile_motion: None,
        amount: Float(amount),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras: Vec::new(),
        sound: None,
        hit: None,
    };
    region.strike(key, Box::new(hurt), &mut TickOutput::default());
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

#[test]
fn a_hoglin_hurt_by_a_mob_fights_it_and_throws_it_and_the_hoglin_beside_it_takes_it_too() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &still_pig([8.5, -60.0, 4.5]));
    lay(&mut region, &hoglin_at(ONE, [5.5, -60.0, 4.5], true));
    lay(&mut region, &hoglin_at(TWO, [5.5, -60.0, 7.5], true));
    // Long enough for each hoglin's sensors to have seen the other.
    for _ in 0..30 {
        checked(&mut region, &TickInputs::default());
    }
    assert_eq!(attack_target(&region, ONE), None);
    hurt_by_the_pig(&mut region, ONE, 1.0);
    // The hurt is answered at the head of its next tick, and the word reaches the other
    // after it.
    checked(&mut region, &TickInputs::default());
    assert_eq!(attack_target(&region, ONE), Some(PIG), "it retaliates");
    checked(&mut region, &TickInputs::default());
    assert_eq!(
        attack_target(&region, TWO),
        Some(PIG),
        "the hoglin beside it is told of its target"
    );
    let full = mob(&region, PIG).body.vitals.health;
    let mut hit = None;
    for _ in 0..200 {
        let output = checked(&mut region, &TickInputs::default());
        let pig = mob(&region, PIG);
        if pig.body.vitals.health < full {
            hit = Some((output, record(&region, PIG).unwrap()));
            break;
        }
    }
    let (output, pig) = hit.expect("a hoglin hits the pig");
    let hitter = [ONE, TWO]
        .into_iter()
        .find(|key| entity_events(&output, *key).contains(&4))
        .expect("the hitter's arms are shown");
    assert!(sounds_of(&output).contains(&sounds::ENTITY_HOGLIN_ATTACK));
    // The hurt's own knockback lifts a mob on the ground to at most 0.4; the throw adds
    // up to half its strength of 1 more (and turns across by a draw of up to ten
    // radians, `Vec3.yRot`'s, so that its way across is anybody's).
    assert!(pig.motion[1] > 0.4, "{:?} by {hitter:?}", pig.motion);
    let across = pig.motion[0].hypot(pig.motion[2]);
    assert!(across > 0.0, "{:?}", pig.motion);
}

#[test]
fn a_creative_player_s_hit_makes_no_target_and_a_player_as_its_target_is_never_struck() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &hoglin_at(ONE, [2.5, -60.0, 0.5], true));
    for _ in 0..30 {
        checked(&mut region, &TickInputs::default());
    }
    let before = mob(&region, ONE).body.vitals.health;
    checked(
        &mut region,
        &moves(vec![numbered(1, PlayerInput::Attack { entity: ONE })]),
    );
    assert!(mob(&region, ONE).body.vitals.health < before, "it is hurt");
    for _ in 0..60 {
        checked(&mut region, &TickInputs::default());
        assert_eq!(
            attack_target(&region, ONE),
            None,
            "no creative player is its target"
        );
    }
    // Made its target by force, the player is let go and never hit.
    let player_key = region.players[&player(1)].entity_id;
    let pose = region.players[&player(1)].pose;
    let entity = record(&region, ONE).unwrap();
    let mut hoglin = Mob::decode(&entity.data).unwrap();
    let mut memories = brain::memories_of(&hoglin_ai::PROVIDER, &hoglin, entity.lived);
    memories.set_with_expiry(
        MemoryKind::AttackTarget,
        Value::Entity(player_key),
        hoglin_ai::ATTACK_DURATION,
    );
    brain::write_memories(&hoglin_ai::PROVIDER, &mut hoglin, memories);
    let mut changed = entity.clone();
    changed.data = hoglin.encode();
    lay(&mut region, &changed);
    assert_eq!(attack_target(&region, ONE), Some(player_key));
    for _ in 0..40 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(
            !sounds_of(&output).contains(&sounds::ENTITY_HOGLIN_ATTACK),
            "it never hits them"
        );
    }
    assert_eq!(attack_target(&region, ONE), None, "the player is let go");
    assert_eq!(region.players[&player(1)].pose.position, pose.position);
}

#[test]
fn a_hoglin_becomes_a_zoglin_with_nausea_after_300_ticks_in_the_overworld() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &hoglin_at(ONE, [5.5, -60.0, 4.5], false));
    let mut converted = None;
    for tick in 1..=301 {
        let output = checked(&mut region, &TickInputs::default());
        if record(&region, ONE).is_none() {
            converted = Some((tick, output));
            break;
        }
        assert_eq!(
            hoglin::state_of(&mob(&region, ONE)).time_in_overworld,
            tick,
            "counted each tick"
        );
    }
    let (tick, output) = converted.expect("it converts");
    assert_eq!(tick, 301, "past 300 ticks");
    assert!(sounds_of(&output).contains(&sounds::ENTITY_HOGLIN_CONVERTED_TO_ZOMBIFIED));
    let zoglin = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .find(|entity| i32::from(entity.kind) == entity_types::ZOGLIN)
        .cloned()
        .expect("a zoglin is made");
    let made = Mob::decode(&zoglin.data).unwrap();
    let nausea = crate::mobs::body::effects::Effect::named("minecraft:nausea").unwrap();
    assert!(made.body.effects.has(nausea), "with nausea");
    assert!(!zoglin::state_of(&made).baby);
}

#[test]
fn a_hoglin_immune_to_zombification_never_converts_in_the_overworld() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &hoglin_at(ONE, [5.5, -60.0, 4.5], true));
    for _ in 0..320 {
        checked(&mut region, &TickInputs::default());
    }
    assert_eq!(
        record(&region, ONE).unwrap().kind,
        entity_types::HOGLIN as u16
    );
    assert_eq!(hoglin::state_of(&mob(&region, ONE)).time_in_overworld, 0);
}

#[test]
fn a_hoglin_in_the_nether_never_converts() {
    let floor = 64;
    let mut region =
        crate::region::portals::tests::world_in(clustine_world::Dimension::Nether, floor, |_| None);
    lay(
        &mut region,
        &hoglin_at(ONE, [5.5, f64::from(floor + 1), 4.5], false),
    );
    for _ in 0..320 {
        checked(&mut region, &TickInputs::default());
    }
    let entity = record(&region, ONE).expect("still here");
    assert_eq!(entity.kind, entity_types::HOGLIN as u16);
    assert!(entity.lived >= 320, "it ticked: {}", entity.lived);
    assert_eq!(hoglin::state_of(&mob(&region, ONE)).time_in_overworld, 0);
}

/// The attack target the zoglin `key`'s brain remembers.
fn zoglin_target(region: &Region, key: EntityId) -> Option<EntityId> {
    let entity = record(region, key)?;
    let mob = Mob::decode(&entity.data).ok()?;
    let memories = brain::memories_of(&zoglin::PROVIDER, &mob, entity.lived);
    match memories.get(MemoryKind::AttackTarget) {
        Some(Value::Entity(target)) => Some(target),
        _ => None,
    }
}

#[test]
fn a_zoglin_fights_the_nearest_mob_it_sees_but_a_creeper_and_throws_it() {
    let mut region = on_ticking_floor_with(&[]);
    let creeper = EntityId(31);
    let mut quiet = Mob::fresh();
    quiet.body.flags.no_ai = true;
    lay(
        &mut region,
        &mob_at(creeper, entity_types::CREEPER, [6.5, -60.0, 4.5], quiet),
    );
    lay(&mut region, &still_pig([9.5, -60.0, 4.5]));
    lay(
        &mut region,
        &mob_at(ONE, entity_types::ZOGLIN, [4.5, -60.0, 4.5], Mob::fresh()),
    );
    let full = mob(&region, PIG).body.vitals.health;
    let mut hit = None;
    for _ in 0..300 {
        let output = checked(&mut region, &TickInputs::default());
        assert_ne!(
            zoglin_target(&region, ONE),
            Some(creeper),
            "never a creeper"
        );
        if mob(&region, PIG).body.vitals.health < full {
            hit = Some((output, record(&region, PIG).unwrap()));
            break;
        }
    }
    let (output, pig) = hit.expect("the zoglin hits the pig");
    assert_eq!(zoglin_target(&region, ONE), Some(PIG));
    assert!(entity_events(&output, ONE).contains(&4));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_ZOGLIN_ATTACK));
    // As a hoglin's: above what the hurt's knockback alone lifts it to.
    assert!(pig.motion[1] > 0.4, "{:?}", pig.motion);
    let creeper_full = supplied(
        entity_types::CREEPER as u16,
        attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .unwrap() as f32;
    assert_eq!(mob(&region, creeper).body.vitals.health, creeper_full);
}

#[test]
fn a_hoglin_beside_a_respawn_anchor_is_pacified_and_walks_away_from_it() {
    let anchor = BlockPos::new(8, -60, 8);
    let mut region = on_ticking_floor_with(&[(anchor, clustine_data::blocks::RESPAWN_ANCHOR)]);
    lay(&mut region, &hoglin_at(ONE, [9.5, -60.0, 8.5], true));
    let pacified = |region: &Region| {
        let entity = record(region, ONE).unwrap();
        let mob = Mob::decode(&entity.data).unwrap();
        brain::memories_of(&hoglin_ai::PROVIDER, &mob, entity.lived)
            .check(MemoryKind::Pacified, brain::Status::Present)
    };
    let mut calmed = false;
    let mut away = false;
    for _ in 0..300 {
        checked(&mut region, &TickInputs::default());
        calmed |= pacified(&region);
        let at = record(&region, ONE).unwrap().position;
        away |= (at[0] - 8.5).hypot(at[2] - 8.5) > 5.0;
        if calmed && away {
            break;
        }
    }
    assert!(calmed, "the repellent pacifies it");
    assert!(away, "it walks away from the repellent");
}

#[test]
fn two_hoglins_in_love_make_a_baby_hoglin_kept_from_despawning() {
    let mut region = on_ticking_floor_with(&[]);
    for (key, z) in [(ONE, 4.5), (TWO, 6.5)] {
        let mut entity = hoglin_at(key, [5.5, -60.0, z], true);
        let mut mob = Mob::decode(&entity.data).unwrap();
        crate::mobs::types::animal::update(&mut mob, |own| own.set_in_love(0));
        entity.data = mob.encode();
        lay(&mut region, &entity);
    }
    let mut baby = None;
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        baby = region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities().iter())
            .find(|entity| {
                i32::from(entity.kind) == entity_types::HOGLIN && ![ONE, TWO].contains(&entity.key)
            })
            .cloned();
        if baby.is_some() {
            break;
        }
    }
    let baby = baby.expect("they breed");
    let made = Mob::decode(&baby.data).unwrap();
    assert!(crate::mobs::types::animal::is_baby(&made));
    assert!(made.body.flags.persistent);
}
