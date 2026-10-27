//! The holder's side of N9's entity effects (`docs/groundwork/n9-plan.md`, sections 8.2
//! and 8.3; `region/entity_effects.rs`): a hurt and what follows it, effects given by a
//! splash or a cloud, and what is addressed to a player.

use clustine_data::entity_types;
use clustine_world::components::EffectInstance;
use clustine_world::{
    Causer, Double, Due, Effect, EffectId, Effected, EntityId, EntityUse, FiledEffect, Float,
    GivenEffects, HitExtra, HurtBy, PlayerId,
};

use super::thrown::{cow, health, lay, mob};
use super::*;
use crate::mobs::body::effects::Effect as MobEffect;
use crate::region::random_ticks::Random;
use crate::region::tnt::tests::{add_player, nothing, world};

const SEED: u64 = 0x0EFF_EC75;

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

/// A hurt by `amount` of a skeleton's arrow, nothing following it.
fn arrow_hurt(amount: f32) -> HurtBy {
    let skeleton = Causer {
        key: EntityId(900),
        kind: u16::try_from(entity_types::SKELETON).unwrap(),
        position: [Double(9.5), Double(-60.0), Double(0.5)],
        player: None,
        living: true,
    };
    HurtBy {
        damage_type: "minecraft:arrow".to_owned(),
        cause: Some(skeleton.clone()),
        direct: Some(Causer {
            key: EntityId(901),
            kind: u16::try_from(entity_types::ARROW).unwrap(),
            living: false,
            ..skeleton
        }),
        position: None,
        projectile_motion: Some([Double(-1.0), Double(0.0), Double(0.0)]),
        amount: Float(amount),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras: Vec::new(),
        sound: None,
        hit: None,
    }
}

fn a_cow_in(region: &mut Region) -> EntityId {
    let key = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
    lay(region, &cow(key, [3.5, -60.0, 0.5]));
    key
}

fn fire_of(region: &Region, key: EntityId) -> i32 {
    region.find_entity(key).expect("here").1.fire_ticks
}

#[test]
fn a_hurt_that_takes_is_followed_by_its_knockback_effects_and_fire() {
    let mut region = world(SEED, nothing);
    let key = a_cow_in(&mut region);
    let before = health(&region, key);
    let hurt = HurtBy {
        knockback: Some([Double(0.4), Double(1.0), Double(0.0)]),
        fire_after: 100,
        effects: vec![effect("minecraft:slowness", 0, 600)],
        ..arrow_hurt(3.0)
    };
    let mut output = TickOutput::default();
    region.entity_used(key, EntityUse::Hurt(Box::new(hurt)), &mut output);
    assert_eq!(health(&region, key), before - 3.0);
    assert!(mob(&region, key).body.effects.has(MobEffect(1)), "slowness");
    assert_eq!(fire_of(&region, key), 100);
    let motion = region.find_entity(key).expect("here").1.motion;
    assert!(motion[0] < 0.0, "pushed away from x + 1: {motion:?}");
}

#[test]
fn a_refused_hurt_takes_back_the_fire_a_burning_arrow_set_before_it() {
    let mut region = world(SEED, nothing);
    let key = a_cow_in(&mut region);
    let mut output = TickOutput::default();
    region.entity_used(key, EntityUse::Hurt(Box::new(arrow_hurt(4.0))), &mut output);
    let after_first = health(&region, key);
    // Within its invulnerable time a weaker hurt is refused: the fire it set goes.
    let burning = HurtBy {
        fire_before: 100,
        effects: vec![effect("minecraft:poison", 0, 100)],
        ..arrow_hurt(1.0)
    };
    region.entity_used(key, EntityUse::Hurt(Box::new(burning)), &mut output);
    assert_eq!(health(&region, key), after_first);
    assert_eq!(fire_of(&region, key), 0);
    assert!(
        !mob(&region, key).body.effects.has(MobEffect(18)),
        "no poison"
    );
}

#[test]
fn a_zombie_s_fire_is_drawn_from_the_hit_s_stream_and_needs_one() {
    // At easy on the world's first day the effective difficulty is 0.75: the draw is
    // made, and fire for `2 · (int) 0.75 = 0` seconds is none.
    let mut region = world(SEED, nothing);
    let key = a_cow_in(&mut region);
    let zombie = HurtBy {
        extras: vec![HitExtra::ZombieFire],
        hit: Some((EntityId(900), 12)),
        ..arrow_hurt(2.0)
    };
    let mut output = TickOutput::default();
    region.entity_used(key, EntityUse::Hurt(Box::new(zombie)), &mut output);
    assert_eq!(fire_of(&region, key), 0);
    assert!((region.difficulty_at(BlockPos::new(0, 0, 0)) - 0.75).abs() < 1e-6);
}

#[test]
fn the_hit_s_stream_is_its_attacker_s_lived_and_target_s_and_nothing_else() {
    let draw = |attacker: u64, lived: u32, target: u64| {
        Random::of_hit(SEED, EntityId(attacker), lived, EntityId(target)).next_float()
    };
    assert_eq!(draw(1, 2, 3), draw(1, 2, 3));
    assert_ne!(
        draw(1, 2, 3),
        draw(1, 2, 4),
        "two targets of one arrow draw apart"
    );
    assert_ne!(draw(1, 2, 3), draw(1, 3, 3));
}

#[test]
fn a_splash_s_effects_are_given_and_its_instant_health_heals_by_its_factor() {
    let mut region = world(SEED, nothing);
    let key = a_cow_in(&mut region);
    let mut output = TickOutput::default();
    region.entity_used(key, EntityUse::Hurt(Box::new(arrow_hurt(5.0))), &mut output);
    let hurt = health(&region, key);
    let given = GivenEffects {
        instances: vec![
            effect("minecraft:instant_health", 0, 1),
            effect("minecraft:speed", 1, 1800),
        ],
        instant_factor: Float(0.5),
        source: None,
        owner: None,
        water: false,
    };
    region.entity_used(key, EntityUse::Effects(Box::new(given)), &mut output);
    // `(int) (0.5 · (4 << 0) + 0.5)` is 2.
    assert_eq!(health(&region, key), hurt + 2.0);
    assert!(mob(&region, key).body.effects.has(MobEffect::SPEED));
}

#[test]
fn water_from_a_splash_hurts_nothing_that_is_not_sensitive_to_it() {
    let mut region = world(SEED, nothing);
    let key = a_cow_in(&mut region);
    let before = health(&region, key);
    let water = GivenEffects {
        instances: Vec::new(),
        instant_factor: Float(1.0),
        source: None,
        owner: None,
        water: true,
    };
    let mut output = TickOutput::default();
    region.entity_used(key, EntityUse::Effects(Box::new(water)), &mut output);
    assert_eq!(health(&region, key), before);
}

/// The effect `by` addressed to the player of `entity` at `at`, as the store files it.
fn on_player(player: PlayerId, entity: EntityId, at: BlockPos, by: EntityUse) -> FiledEffect {
    FiledEffect {
        id: EffectId {
            sequence: 3,
            index: 0,
        },
        effect: Effect {
            at,
            what: Effected::OnPlayer { player, entity, by },
        },
        due: Due::FromTaking,
    }
}

#[test]
fn effects_addressed_to_a_player_reach_them_where_their_region_holds_them() {
    let mut region = world(SEED, nothing);
    let (id, entity) = add_player(&mut region, 1, [2.5, -60.0, 2.5], false);
    let given = GivenEffects {
        instances: vec![effect("minecraft:speed", 0, 200)],
        instant_factor: Float(1.0),
        source: None,
        owner: None,
        water: false,
    };
    let by = EntityUse::Effects(Box::new(given));
    let output = region.tick(&TickInputs {
        effects: vec![on_player(id, entity, BlockPos::new(2, -60, 2), by)],
        ..TickInputs::default()
    });
    assert!(output.player_events.iter().any(|(to, event)| *to == id
        && matches!(event, PlayerEvent::EffectAdded { instance, .. } if instance.effect == "minecraft:speed")));
}

#[test]
fn an_effect_addressed_to_a_player_the_region_does_not_have_is_dropped() {
    let mut region = world(SEED, nothing);
    let given = GivenEffects {
        instances: vec![effect("minecraft:speed", 0, 200)],
        instant_factor: Float(1.0),
        source: None,
        owner: None,
        water: false,
    };
    let stranger = PlayerId(uuid::Uuid::from_u128(77));
    let by = EntityUse::Effects(Box::new(given));
    let output = region.tick(&TickInputs {
        effects: vec![on_player(
            stranger,
            EntityId(77),
            BlockPos::new(2, -60, 2),
            by,
        )],
        ..TickInputs::default()
    });
    assert!(
        output.player_events.is_empty(),
        "{:?}",
        output.player_events
    );
}

#[test]
fn a_pearl_that_landed_moves_its_player_there() {
    let mut region = world(SEED, nothing);
    let (id, entity) = add_player(&mut region, 1, [2.5, -60.0, 2.5], false);
    let by = EntityUse::PearlLanded {
        at: [Double(7.5), Double(-60.0), Double(4.5)],
        endermite: false,
    };
    let output = region.tick(&TickInputs {
        effects: vec![on_player(id, entity, BlockPos::new(2, -60, 2), by)],
        ..TickInputs::default()
    });
    let (_, pose) = region.player(id).expect("the player is here");
    assert_eq!((pose.position.x, pose.position.z), (7.5, 4.5));
    assert!(
        output
            .player_events
            .iter()
            .any(|(to, event)| *to == id && matches!(event, PlayerEvent::Teleported { .. }))
    );
}

#[test]
fn what_a_region_sends_to_a_player_goes_out_addressed_to_them_and_counts_at_the_line() {
    let mut region = world(SEED, nothing);
    let player = PlayerId(uuid::Uuid::from_u128(5));
    let by = EntityUse::PearlLanded {
        at: [Double(1.5), Double(-60.0), Double(1.5)],
        endermite: false,
    };
    region.send_on_player(player, EntityId(42), BlockPos::new(20, -60, 1), by);
    let output = region.tick(&TickInputs::default());
    assert!(output.effects.iter().any(|effect| matches!(
        effect.what,
        Effected::OnPlayer { player: to, entity: EntityId(42), .. } if to == player
    )));
}
