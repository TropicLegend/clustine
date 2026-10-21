//! The body's rules on a mob alone, from the brief's sections 1.4 to 1.7: its hurts and
//! their cooldown, its push, its death and what it drops.

use clustine_data::{entity_types, items, sounds};
use clustine_world::EntityId;

use super::hurt::{
    self, Causer, DamageSource, DamageType, Struck, damage_after_absorb, experience_value,
};
use super::{Event, loot};
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::tick::MobOutput;
use crate::mobs::types::{self, MobType};
use crate::region::Random;

const COW: u16 = entity_types::COW as u16;

/// A cow of full health and air, as a body alone: the tests here are of the body's rules,
/// which a type that adds nothing ([`types::Plain`]) keeps.
fn cow() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = MAX_AIR;
    mob
}

/// The player entity 9, standing at `at`, as what caused and dealt a hit.
fn player_at(at: [f64; 3]) -> Causer {
    Causer {
        key: EntityId(9),
        kind: entity_types::PLAYER as u16,
        position: at,
        player: Some(9),
        living: true,
    }
}

/// The resting motion of a mob on the ground.
const RESTING: [f64; 3] = [0.0, -0.078_400_001_525_878_9, 0.0];

/// `hurtServer` of `mob` (a cow at the origin, on the ground, moving as `motion`) at the
/// moment `moment`, with the stream `draws`.
fn hurt_at(
    mob: &mut Mob,
    mob_type: &'static dyn MobType,
    motion: &mut [f64; 3],
    moment: u32,
    source: &DamageSource,
    amount: f32,
    draws: &mut Random,
) -> (bool, MobOutput) {
    let mut out = MobOutput::default();
    let took = {
        let mut struck = Struck {
            kind: COW,
            mob,
            mob_type,
            position: [0.0, 0.0, 0.0],
            motion,
            on_ground: true,
            on_fire: false,
            vehicle: None,
            carries: false,
            leashed: false,
            in_liquid: false,
            moment,
            game_time: 0,
            draws,
            out: &mut out,
        };
        hurt::hurt_server(&mut struck, source, amount)
    };
    (took, out)
}

#[test]
fn a_hit_within_the_cooldown_does_only_what_it_adds_and_pushes_and_shows_nothing_more() {
    let mut mob = cow();
    let mut motion = RESTING;
    let mut draws = Random::of_entity(1, EntityId(5), 0);
    let source = DamageSource::by(DamageType::PLAYER_ATTACK, player_at([1.0, 0.0, 0.0]));
    // The first hit: all of it, its event and sound, pushed away from the player and up.
    let (took, out) = hurt_at(
        &mut mob,
        &types::Plain,
        &mut motion,
        0,
        &source,
        4.0,
        &mut draws,
    );
    assert!(took);
    assert_eq!(mob.body.vitals.health, 6.0);
    assert_eq!(mob.body.vitals.last_hurt, 4.0);
    assert_eq!(
        motion,
        [
            -0.400_000_005_960_464_5,
            (RESTING[1] / 2.0 + 0.400_000_005_960_464_5).min(0.4),
            0.0
        ]
    );
    assert!(out.needs_sync && out.sync_velocity);
    assert!(matches!(
        out.events[..],
        [
            Event::GameEvent(hurt::GameEvent::EntityDamage),
            Event::Damaged {
                kind: DamageType::PLAYER_ATTACK,
                cause: Some(EntityId(9)),
                ..
            },
            Event::Sound {
                sound: sounds::ENTITY_GENERIC_HURT,
                ..
            }
        ]
    ));
    assert_eq!(mob.body.combat.player_memory(0), Some(9));
    // Five ticks on, its cooldown is 15: a hit no harder does nothing.
    let pushed = motion;
    let before = mob.clone();
    let (took, out) = hurt_at(
        &mut mob,
        &types::Plain,
        &mut motion,
        5,
        &source,
        3.0,
        &mut draws,
    );
    assert!(!took);
    assert_eq!(mob, before);
    assert_eq!(out, MobOutput::default());
    // A harder one does what it adds, with no event, no sound and no push (the bytecode:
    // `hurtServer`'s damage event and push are under its `playSounds`).
    let (took, out) = hurt_at(
        &mut mob,
        &types::Plain,
        &mut motion,
        5,
        &source,
        6.0,
        &mut draws,
    );
    assert!(took);
    assert_eq!(mob.body.vitals.health, 4.0);
    assert_eq!(mob.body.vitals.last_hurt, 6.0);
    assert_eq!(motion, pushed);
    assert_eq!(
        out.events,
        [Event::GameEvent(hurt::GameEvent::EntityDamage)]
    );
    assert!(!out.needs_sync && !out.sync_velocity);
    // Ten ticks after the first, the cooldown is 10, no longer above it: a hit counts whole.
    let (took, out) = hurt_at(
        &mut mob,
        &types::Plain,
        &mut motion,
        10,
        &source,
        1.0,
        &mut draws,
    );
    assert!(took);
    assert_eq!(mob.body.vitals.health, 3.0);
    assert!(
        out.events
            .iter()
            .any(|event| matches!(event, Event::Damaged { .. }))
    );
}

#[test]
fn the_counters_of_a_hurt_count_down_from_its_moment_and_a_player_is_remembered_for_100() {
    let mut mob = cow();
    let mut motion = RESTING;
    let mut draws = Random::of_entity(1, EntityId(5), 0);
    let source = DamageSource::by(DamageType::PLAYER_ATTACK, player_at([1.0, 0.0, 0.0]));
    hurt_at(
        &mut mob,
        &types::Plain,
        &mut motion,
        40,
        &source,
        1.0,
        &mut draws,
    );
    let vitals = &mob.body.vitals;
    assert_eq!(
        (
            vitals.cooldown(40),
            vitals.cooldown(50),
            vitals.cooldown(70)
        ),
        (20, 10, 0)
    );
    assert_eq!(mob.body.combat.player_memory(139), Some(9));
    assert_eq!(mob.body.combat.player_memory(140), None);
    // A hit resets the idle time where it counts, and puts the ambient sound off.
    assert_eq!(mob.body.idle.ambient_since, 40 + 80);
}

#[test]
fn what_hurts_a_mob_by_itself_pushes_nothing_and_drowning_does_not_mark_it_hurt() {
    for (kind, impact) in [
        (DamageType::FALL, true),
        (DamageType::LAVA, true),
        (DamageType::IN_WALL, true),
        (DamageType::DROWN, false),
        (DamageType::OUT_OF_WORLD, true),
    ] {
        let mut mob = cow();
        let mut motion = RESTING;
        let mut draws = Random::of_entity(1, EntityId(5), 0);
        let (took, out) = hurt_at(
            &mut mob,
            &types::Plain,
            &mut motion,
            0,
            &DamageSource::of(kind),
            2.0,
            &mut draws,
        );
        assert!(took, "{kind:?}");
        assert_eq!(motion, RESTING, "{kind:?}");
        assert_eq!(out.sync_velocity, impact, "{kind:?}");
        assert_eq!(mob.body.vitals.health, 8.0, "{kind:?}");
    }
}

#[test]
fn armour_takes_its_share_by_the_game_s_combat_rules() {
    // The brief's section 1.4: armour 2 against 4 takes 0.4 of 25 off.
    assert_eq!(damage_after_absorb(4.0, 2.0, 0.0), 4.0 * (1.0 - 0.4 / 25.0));
    assert_eq!(damage_after_absorb(4.0, 0.0, 0.0), 4.0);
    // `Mth.clamp(armour - damage / t, armour * 0.2, 20)`.
    assert_eq!(
        damage_after_absorb(1.0, 20.0, 8.0),
        1.0 * (1.0 - 19.75 / 25.0)
    );
}

#[test]
fn experience_is_cut_into_the_largest_orbs_first() {
    let cut = |mut amount: i32| {
        let mut orbs = Vec::new();
        while amount > 0 {
            let value = experience_value(amount);
            amount -= value;
            orbs.push(value);
        }
        orbs
    };
    assert_eq!(cut(3), [3]);
    assert_eq!(cut(2), [1, 1]);
    assert_eq!(cut(10), [7, 3]);
    assert_eq!(cut(2500), [2477, 17, 3, 3]);
}

/// A cow that is a baby, as an ageable type answers.
struct Calf;

impl MobType for Calf {
    fn is_baby(&self, mob: &Mob) -> bool {
        let _ = mob;
        true
    }
}

#[test]
fn a_cow_killed_by_a_player_drops_its_table_s_loot_by_the_game_s_draws_and_a_baby_none() {
    let source = DamageSource::by(DamageType::PLAYER_ATTACK, player_at([1.0, 0.0, 0.0]));
    let mut mob = cow();
    mob.body.vitals.health = 1.0;
    let mut motion = RESTING;
    let mut draws = Random::of_entity(3, EntityId(5), 0);
    // What the stream gives, walked by hand: the push draws nothing, the death sound's
    // pitch two floats; then the leather's `uniform(0, 2)`, the beef's `uniform(1, 3)`
    // (the smelt's conditions draw nothing: not burning, no enchanted weapon), and the
    // experience of an animal, 1 to 3, then each orb's merge draw of 40.
    let mut walk = draws.clone();
    walk.next_float();
    walk.next_float();
    let leather = walk.next_int(3) as i32;
    let beef = walk.next_int(3) as i32 + 1;
    let experience = 1 + walk.next_int(3) as i32;
    let (took, out) = hurt_at(
        &mut mob,
        &types::Plain,
        &mut motion,
        0,
        &source,
        2.0,
        &mut draws,
    );
    assert!(took);
    assert!(mob.body.vitals.dying());
    let death = out.died.expect("it died");
    assert_eq!(
        death.drops,
        [
            clustine_world::ItemStack {
                item: items::LEATHER,
                count: leather
            },
            clustine_world::ItemStack {
                item: items::BEEF,
                count: beef
            }
        ]
    );
    let given: i32 = death.orbs.iter().map(|(value, _)| value).sum();
    assert_eq!(given, experience);
    assert!(out.events.contains(&Event::Entity(3)));
    assert!(out.events.iter().any(|event| matches!(
        event,
        Event::Sound {
            sound: sounds::ENTITY_GENERIC_DEATH,
            ..
        }
    )));

    // A calf drops nothing and gives no experience.
    let mut calf = cow();
    calf.body.vitals.health = 1.0;
    let mut motion = RESTING;
    let (_, out) = hurt_at(&mut calf, &Calf, &mut motion, 0, &source, 2.0, &mut draws);
    let death = out.died.expect("it died");
    assert!(death.drops.is_empty() && death.orbs.is_empty());

    // Killed by nothing a player did, a cow drops its loot but no experience.
    let mut alone = cow();
    alone.body.vitals.health = 1.0;
    let mut motion = RESTING;
    let (_, out) = hurt_at(
        &mut alone,
        &types::Plain,
        &mut motion,
        0,
        &DamageSource::of(DamageType::FALL),
        2.0,
        &mut draws,
    );
    let death = out.died.expect("it died");
    assert!(!death.drops.is_empty() && death.orbs.is_empty());
}

#[test]
fn a_cow_that_dies_burning_drops_cooked_beef() {
    let subject = loot::Subject {
        kind: COW,
        on_fire: true,
        baby: false,
        vehicle: None,
        facts: loot::Facts::default(),
    };
    let mut draws = Random::of_entity(4, EntityId(5), 0);
    let drops = loot::entity_drops(
        &subject,
        &DamageSource::of(DamageType::LAVA),
        false,
        &mut draws,
    );
    assert!(drops.iter().any(|stack| stack.item == items::COOKED_BEEF));
    assert!(!drops.iter().any(|stack| stack.item == items::BEEF));
}

#[test]
fn a_sheep_s_wool_follows_its_colour_and_none_drops_where_it_is_sheared() {
    let sheep = entity_types::SHEEP as u16;
    let mut draws = Random::of_entity(4, EntityId(5), 0);
    let mut subject = loot::Subject {
        kind: sheep,
        on_fire: false,
        baby: false,
        vehicle: None,
        facts: loot::Facts {
            sheep_colour: Some("lime"),
            sheared: Some(false),
            ..loot::Facts::default()
        },
    };
    let source = DamageSource::of(DamageType::FALL);
    let drops = loot::entity_drops(&subject, &source, false, &mut draws);
    assert!(drops.iter().any(|stack| stack.item == items::LIME_WOOL));
    subject.facts.sheared = Some(true);
    let drops = loot::entity_drops(&subject, &source, false, &mut draws);
    assert!(!drops.iter().any(|stack| stack.item == items::LIME_WOOL));
}

/// A body written by a build before N7f's measurement: version 1 of the head, the
/// controls and the progress. The head's last stable yaw the controls held goes to the
/// head, and the timeout's timer and check the progress held to the timeout; the rest is
/// as it was.
#[test]
fn a_body_of_the_first_versions_is_read_with_its_moved_values_where_they_are_now() {
    use super::Body;
    use crate::mobs::ai::{LookState, MoveState};
    use crate::mobs::part::body_ids;
    use clustine_world::BlockPos;
    let entry = |id: u16, bytes: Vec<u8>| clustine_world::Entry {
        id,
        version: 1,
        bytes,
    };
    let head = postcard::to_allocvec(&(90.0_f32, 80.0_f32, 7_u32)).unwrap();
    let look = LookState {
        cooldown: 2,
        ..LookState::default()
    };
    let controls = postcard::to_allocvec(&(
        MoveState::default(),
        look.clone(),
        true,
        33.5_f32,
        vec![(3_u8, -1.0_f32)],
    ))
    .unwrap();
    let progress = postcard::to_allocvec(&(
        4_u32,
        1.25_f64,
        100_u32,
        [1.0_f64, 2.0, 3.0],
        false,
        [5_i32, 6, 7],
        41_i64,
        9_000_i64,
        12.5_f64,
        true,
        Some(80_u32),
        Some(BlockPos::new(8, -60, 9)),
        1_i32,
    ))
    .unwrap();
    let entries = vec![
        entry(body_ids::HEAD, head),
        entry(body_ids::CONTROLS, controls),
        entry(body_ids::PROGRESS, progress),
    ];
    let body = Body::decode(&entries).unwrap();
    assert_eq!(
        (
            body.head.head_yaw,
            body.head.body_yaw,
            body.head.still_since,
            body.head.last_stable_yaw
        ),
        (90.0, 80.0, 7, 33.5)
    );
    assert_eq!(body.controls.look, look);
    assert!(body.controls.jump);
    assert_eq!(body.controls.malus, [(3, -1.0)]);
    assert_eq!((body.timeout.timer, body.timeout.last_check), (41, 9_000));
    assert_eq!(body.progress.next, 4);
    assert_eq!(body.progress.timeout_node, [5, 6, 7]);
    assert_eq!(body.progress.timeout_limit, 12.5);
    assert_eq!(body.progress.target, Some(BlockPos::new(8, -60, 9)));
    // Written again, each in the version this build writes.
    let written = body.encode();
    assert!(
        written
            .iter()
            .all(|entry| entry.version == 2 || entry.id == body_ids::TIMEOUT),
        "{written:?}"
    );
    assert_eq!(Body::decode(&written).unwrap(), body);
}
