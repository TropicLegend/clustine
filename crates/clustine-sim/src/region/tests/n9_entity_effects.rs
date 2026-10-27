//! N9's entity effects and a hit's extras, tested from the plan (`docs/groundwork/
//! n9-plan.md`, sections 8.2, 8.3 and 14, and decisions 1 and 2 of 18.1 and 18.5) by
//! someone who did not write them: each use (`Hurt`, `Effects`, `Alert`, `PearlLanded`,
//! `PatrolTarget`) filed with the region that holds its mob or its player and taken
//! there, nothing done where the target has gone; an effect addressed to a player found
//! by their stay wherever the block it names; and a hit's extras run where the hit
//! lands, drawn from the hit's stream (`Random::of_hit`), alike in one region and across
//! a pinned line, and not where the hurt is refused.
//!
//! The effects are filed as the store files them (`TickInputs::effects`) wherever a test
//! can; the region's own landing of a hit (`Local::Strike`) is applied as its attacker's
//! tick would.

use clustine_data::entity_types;
use clustine_world::components::EffectInstance;
use clustine_world::{
    Causer, Double, Effect, Effected, EntityChange, EntityUse, Float, GivenEffects, HitExtra,
    HurtBy,
};

use super::n9_world::*;
use super::thrown::{cow, health, lay, mob};
use super::*;
use crate::api::WorldClock;
use crate::mobs::body::effects::Effect as MobEffect;
use crate::mobs::tick::Local;
use crate::region::random_ticks::Random;

/// The key of the zombie that hits, which no region of these tests has.
const ZOMBIE: EntityId = EntityId(0x5_0000_0001);

/// The key of the cow each test hits, the same in every region.
const TARGET: EntityId = EntityId(0x5_0000_0002);

/// A clock 48 days in, on a full moon: the effective difficulty at easy is
/// `0.75 + 0.1875 + 0.09375`, past 1, so a burning zombie's fire lasts `2 · 1` seconds
/// and its chance is `1.03125 · 0.3` (section 7).
const OLD_WORLD: u64 = 48 * 24_000;

fn effective(day: u64) -> f32 {
    let t = ((day as f32 - 72_000.0) / 1_440_000.0).clamp(0.0, 1.0) * 0.25;
    let moon = crate::mobs::making::moon_brightness(day);
    let c = (moon * 0.25).clamp(0.0, t) * 0.5;
    0.75 + t + c
}

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("a type's id")
}

/// A zombie's melee hit of `amount` (`Mob.doHurtTarget`) as its region makes it, with
/// `extras` and the hit's stream named by the zombie's `lived`.
fn zombie_hit(amount: f32, extras: Vec<HitExtra>, lived: u32) -> HurtBy {
    let zombie = Causer {
        key: ZOMBIE,
        kind: kind(entity_types::ZOMBIE),
        position: [Double(15.5), Double(-60.0), Double(0.5)],
        player: None,
        living: true,
    };
    HurtBy {
        damage_type: "minecraft:mob_attack".to_owned(),
        cause: Some(zombie.clone()),
        direct: Some(zombie),
        position: None,
        projectile_motion: None,
        amount: Float(amount),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras,
        sound: None,
        hit: Some((ZOMBIE, lived)),
    }
}

/// Whether the hit's stream of the zombie at `lived` on the target sets it alight at
/// the effective difficulty of `day` (`Zombie.doHurtTarget`: `nextFloat() < eff ·
/// 0.3f`).
fn fire_drawn(lived: u32, day: u64) -> bool {
    Random::of_hit(SEED, ZOMBIE, lived, TARGET).next_float() < effective(day) * 0.3
}

/// The cow at (17.5, -60, 0.5), fresh, laid in `region`'s loaded chunk.
fn fresh_target(region: &mut Region) {
    lay(region, &cow(TARGET, [17.5, -60.0, 0.5]));
}

fn fire_of(region: &Region) -> i32 {
    region.find_entity(TARGET).expect("the cow").1.fire_ticks
}

/// One region over the land about the origin, the world `day` days in by its clock.
fn alone_at(day: u64) -> Region {
    alone(
        0,
        WorldClock {
            game_time: day,
            day,
        },
    )
}

/// Section 8.2 and decision 1 of 18.1: a burning, empty-handed zombie's fire is drawn,
/// where the hit lands and only if it took, from the hit's stream keyed by the zombie,
/// its `lived` at the hit and the target; it sets the target alight for
/// `2 · (int) eff` seconds. Landed in the zombie's own region and filed across a line
/// with the target's holder, the same hits burn alike.
#[test]
fn a_zombie_s_fire_is_drawn_from_the_hit_s_stream_alike_in_one_region_and_across_a_line() {
    let mut here = alone_at(OLD_WORLD);
    let mut there = east(
        500,
        WorldClock {
            game_time: OLD_WORLD,
            day: OLD_WORLD,
        },
    );
    let mut burnt = 0;
    for lived in 0..80 {
        let expected = fire_drawn(lived, OLD_WORLD);
        burnt += u32::from(expected);
        let hit = zombie_hit(3.0, vec![HitExtra::ZombieFire], lived);

        fresh_target(&mut here);
        let mut output = TickOutput::default();
        here.apply_locals(
            ZOMBIE,
            vec![Local::Strike {
                key: TARGET,
                hurt: Box::new(hit.clone()),
            }],
            &mut output,
        );
        assert_eq!(
            fire_of(&here),
            if expected { 40 } else { 0 },
            "lived {lived}"
        );

        fresh_target(&mut there);
        let mut output = TickOutput::default();
        there.entity_used(TARGET, EntityUse::Hurt(Box::new(hit)), &mut output);
        assert_eq!(
            fire_of(&there),
            if expected { 40 } else { 0 },
            "lived {lived}"
        );
    }
    // About three in ten.
    assert!((10..=40).contains(&burnt), "{burnt} of 80 burnt");
}

/// The same across a pinned line from end to end: the zombie's region hits a cow it
/// sees in its copy, which sends the hurt; the store files it with the holder, which
/// lands it and burns the cow as the stream says.
#[test]
fn a_zombie_s_hit_across_a_pinned_line_is_sent_and_lands_with_its_fire() {
    let clock = WorldClock {
        game_time: OLD_WORLD,
        day: OLD_WORLD,
    };
    let lived = (0..200)
        .find(|lived| fire_drawn(*lived, OLD_WORLD))
        .expect("a hit that burns");
    let mut west = west(500, clock);
    let mut east = east(500, clock);
    fresh_target(&mut east);
    west.copies
        .get_mut(&ChunkPos::new(1, 0))
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(cow(TARGET, [17.5, -60.0, 0.5])));
    let before = health(&east, TARGET);
    let mut output = TickOutput::default();
    west.apply_locals(
        ZOMBIE,
        vec![Local::Strike {
            key: TARGET,
            hurt: Box::new(zombie_hit(3.0, vec![HitExtra::ZombieFire], lived)),
        }],
        &mut output,
    );
    let output = west.tick(&TickInputs::default());
    let sent: Vec<Effect> = output
        .effects
        .iter()
        .filter(|effect| matches!(&effect.what, Effected::OnEntity { key, .. } if *key == TARGET))
        .cloned()
        .collect();
    assert_eq!(sent.len(), 1, "{:?}", output.effects);
    east.tick(&TickInputs {
        effects: filed(sent, 1),
        ..TickInputs::default()
    });
    assert_eq!(health(&east, TARGET), before - 3.0);
    // Alight for 40 ticks, one of them burnt in the tick it landed in, or about to be.
    assert!((39..=40).contains(&fire_of(&east)), "{}", fire_of(&east));
}

/// Section 8.2: the extras follow only a hurt that took. A hurt within the cow's
/// invulnerable time and no stronger than the one before is refused, and the fire whose
/// draw would have passed is not set.
#[test]
fn a_refused_hit_sets_no_fire_whatever_its_stream_would_draw() {
    let lived = (0..200)
        .find(|lived| fire_drawn(*lived, OLD_WORLD))
        .expect("a hit that would burn");
    let unburnt = (0..200)
        .find(|lived| !fire_drawn(*lived, OLD_WORLD))
        .expect("a hit that would not");
    let mut region = alone_at(OLD_WORLD);
    fresh_target(&mut region);
    let mut output = TickOutput::default();
    region.entity_used(
        TARGET,
        EntityUse::Hurt(Box::new(zombie_hit(
            5.0,
            vec![HitExtra::ZombieFire],
            unburnt,
        ))),
        &mut output,
    );
    let after_first = health(&region, TARGET);
    assert_eq!(fire_of(&region), 0);
    region.entity_used(
        TARGET,
        EntityUse::Hurt(Box::new(zombie_hit(3.0, vec![HitExtra::ZombieFire], lived))),
        &mut output,
    );
    assert_eq!(health(&region, TARGET), after_first, "refused");
    assert_eq!(fire_of(&region), 0, "no fire after a refused hit");
}

/// Section 7: in a young world the effective difficulty at easy is 0.75, so a burning
/// zombie's fire is `2 · 0` seconds: drawn, and nothing burns.
#[test]
fn a_zombie_s_fire_in_a_young_world_is_of_no_seconds() {
    let mut region = alone_at(0);
    for lived in 0..30 {
        fresh_target(&mut region);
        let mut output = TickOutput::default();
        region.entity_used(
            TARGET,
            EntityUse::Hurt(Box::new(zombie_hit(3.0, vec![HitExtra::ZombieFire], lived))),
            &mut output,
        );
        assert_eq!(fire_of(&region), 0, "lived {lived}");
    }
}

fn has_hunger(region: &Region) -> bool {
    mob(region, TARGET).body.effects.has(MobEffect::HUNGER)
}

/// Section 5.3 (finding 47): the husk's hunger is `140 · (int) eff` ticks; at easy in a
/// young world that is none, but it is still an `addEffect`, there for one tick. In a
/// world old enough for `(int) eff` to be 1, it lasts 140 ticks.
#[test]
fn a_husk_s_hunger_at_easy_is_added_for_one_tick_in_a_young_world() {
    let husk_hit = |lived| {
        let mut hit = zombie_hit(3.0, vec![HitExtra::HuskHunger], lived);
        if let Some(cause) = hit.cause.as_mut() {
            cause.kind = kind(entity_types::HUSK);
        }
        hit.direct = hit.cause.clone();
        hit
    };
    let mut region = alone_at(0);
    fresh_target(&mut region);
    let mut output = TickOutput::default();
    region.entity_used(TARGET, EntityUse::Hurt(Box::new(husk_hit(3))), &mut output);
    assert!(has_hunger(&region), "added");
    region.tick(&TickInputs::default());
    assert!(!has_hunger(&region), "gone after one tick");

    let mut region = alone_at(OLD_WORLD);
    fresh_target(&mut region);
    let mut output = TickOutput::default();
    region.entity_used(TARGET, EntityUse::Hurt(Box::new(husk_hit(3))), &mut output);
    for _ in 0..139 {
        region.tick(&TickInputs::default());
    }
    assert!(has_hunger(&region), "still there after 139 ticks");
    region.tick(&TickInputs::default());
    assert!(!has_hunger(&region), "gone after 140");
}

/// `effect` addressed to the cow, filed at its block.
fn on_the_cow(by: EntityUse) -> Effect {
    Effect {
        at: BlockPos::new(17, -60, 0),
        what: Effected::OnEntity { key: TARGET, by },
    }
}

fn speed_given() -> GivenEffects {
    GivenEffects {
        instances: vec![EffectInstance {
            effect: "minecraft:speed".to_owned(),
            amplifier: 0,
            duration: 300,
            ambient: false,
            visible: true,
            show_icon: true,
        }],
        instant_factor: Float(1.0),
        source: None,
        owner: None,
        water: false,
    }
}

/// Section 8.3: each use filed with the holder of its mob is taken there: a hurt by its
/// amount, effects added, an alert taken by a mob with no target; and nothing happens
/// where the mob has gone, the effect being taken all the same.
#[test]
fn each_use_filed_with_the_holder_of_its_mob_is_taken_and_none_where_it_has_gone() {
    let mut east = east(10, clock_at(0));
    fresh_target(&mut east);
    let before = health(&east, TARGET);
    east.tick(&TickInputs {
        effects: filed(
            vec![
                on_the_cow(EntityUse::Hurt(Box::new(zombie_hit(2.0, Vec::new(), 1)))),
                on_the_cow(EntityUse::Effects(Box::new(speed_given()))),
            ],
            1,
        ),
        ..TickInputs::default()
    });
    assert_eq!(health(&east, TARGET), before - 2.0);
    assert!(mob(&east, TARGET).body.effects.has(MobEffect::SPEED));

    // An alert: a zombie with no target takes the attacker.
    let mut zombie = cow(TARGET, [17.5, -60.0, 0.5]);
    zombie.kind = kind(entity_types::ZOMBIE);
    lay(&mut east, &zombie);
    let attacker = EntityId(0x5_0000_0099);
    let alert = EntityUse::Alert {
        attacker,
        kind: kind(entity_types::PLAYER),
        class: kind(entity_types::ZOMBIE),
    };
    east.tick(&TickInputs {
        effects: filed(vec![on_the_cow(alert.clone())], 2),
        ..TickInputs::default()
    });
    assert_eq!(mob(&east, TARGET).body.combat.target, Some(attacker));

    // Gone: nothing is hurt, nothing new appears, and the effects are taken.
    let mut empty = super::n9_world::east(10, clock_at(0));
    let output = empty.tick(&TickInputs {
        effects: filed(
            vec![
                on_the_cow(EntityUse::Hurt(Box::new(zombie_hit(2.0, Vec::new(), 1)))),
                on_the_cow(EntityUse::Effects(Box::new(speed_given()))),
                on_the_cow(alert),
            ],
            3,
        ),
        ..TickInputs::default()
    });
    assert_eq!(output.taken.len(), 3);
    assert!(empty.find_entity(TARGET).is_none());
    assert!(
        output.entity_changes.is_empty(),
        "{:?}",
        output.entity_changes
    );
}

/// Section 8.6 (finding 26): a patrol's leader names its target to its companions within
/// 16 blocks, which may be in another region's copy; a mob that can join the patrol
/// takes it as its patrol target and patrols (`setPatrolTarget`).
#[test]
#[ignore = "waits for S11's patrols: the holder's patrol_target_given does nothing yet"]
fn a_patrol_target_filed_with_the_holder_of_a_patrolling_mob_is_taken() {
    let mut east = east(10, clock_at(0));
    let mut pillager = cow(TARGET, [17.5, -60.0, 0.5]);
    pillager.kind = kind(entity_types::PILLAGER);
    lay(&mut east, &pillager);
    let target = BlockPos::new(300, -60, 40);
    east.tick(&TickInputs {
        effects: filed(vec![on_the_cow(EntityUse::PatrolTarget { pos: target })], 1),
        ..TickInputs::default()
    });
    let state = crate::mobs::types::illagers::state_of(&mob(&east, TARGET));
    assert_eq!(state.patrol_target, Some(target));
    assert!(state.patrolling);
}

/// `by` addressed to player `number`, whose stay is `entity`, at the block `at`.
fn to_player(number: u128, entity: EntityId, at: BlockPos, by: EntityUse) -> Effect {
    Effect {
        at,
        what: Effected::OnPlayer {
            player: player(number),
            entity,
            by,
        },
    }
}

/// Section 8.3: what is addressed to a player is taken by the region that has their
/// stay: effects added and told to them (a creative player takes no instant ones), a
/// hurt refused (creative), a pearl's landing moving them there; and nothing where the
/// player has gone.
#[test]
fn each_use_addressed_to_a_player_is_taken_where_their_stay_is_and_none_where_they_have_gone() {
    let mut east = east(10, clock_at(0));
    enter_with(
        &mut east,
        2,
        entering_place(Vec3::new(20.5, -60.0, 0.5), Vec::new(), 0),
    );
    let (entity, _) = east.player(player(2)).expect("player 2 is in the east");
    let at = BlockPos::new(20, -60, 0);
    let mut given = speed_given();
    given.instances.push(EffectInstance {
        effect: "minecraft:instant_damage".to_owned(),
        amplifier: 1,
        duration: 1,
        ambient: false,
        visible: true,
        show_icon: true,
    });
    let output = east.tick(&TickInputs {
        effects: filed(
            vec![
                to_player(2, entity, at, EntityUse::Effects(Box::new(given))),
                to_player(
                    2,
                    entity,
                    at,
                    EntityUse::Hurt(Box::new(zombie_hit(4.0, Vec::new(), 1))),
                ),
            ],
            1,
        ),
        ..TickInputs::default()
    });
    let added: Vec<String> = effects_added(&output, 2)
        .into_iter()
        .map(|instance| instance.effect)
        .collect();
    assert_eq!(added, ["minecraft:speed"]);
    assert!(east.player(player(2)).is_some(), "the hurt did nothing");

    let landed = EntityUse::PearlLanded {
        at: [Double(24.5), Double(-60.0), Double(3.5)],
        endermite: false,
    };
    let output = east.tick(&TickInputs {
        effects: filed(vec![to_player(2, entity, at, landed.clone())], 2),
        ..TickInputs::default()
    });
    let (_, pose) = east.player(player(2)).expect("there");
    assert_eq!((pose.position.x, pose.position.z), (24.5, 3.5));
    assert!(
        told_to(&output, 2)
            .iter()
            .any(|event| matches!(event, PlayerEvent::Teleported { .. }))
    );

    // Gone: nobody is moved or told anything.
    let entity_gone = EntityId(entity.0 + 1);
    let output = east.tick(&TickInputs {
        effects: filed(
            vec![
                to_player(
                    2,
                    entity_gone,
                    at,
                    EntityUse::Effects(Box::new(speed_given())),
                ),
                to_player(2, entity_gone, at, landed),
            ],
            3,
        ),
        ..TickInputs::default()
    });
    assert!(told_to(&output, 2).is_empty(), "{:?}", output.player_events);
    assert_eq!(output.taken.len(), 2);
}

/// Section 8.3 (finding 27; ruling 2 of 18.5): a player's region and the land they stand
/// in can differ, and what is addressed to them goes to the region of their stay. That
/// region takes it, though the block it names is not of its land: player 2's stay is in
/// the east while they stand a step into the west's land, and effects addressed to them
/// there reach them.
///
/// As built (the plan's 8.3, "As built (N9e)"), the effect goes to the holder of the
/// block it names, and a region runs no filed effect of a chunk it has not loaded
/// (`run_effect`), so the region of the stay drops it: the two differ while a player
/// stands in land their region has asked about and not yet let them go into.
#[test]
#[ignore = "finding: an effect addressed to a player goes by its block, and the region of their stay drops it where the block is another region's land"]
fn an_effect_addressed_to_a_player_reaches_them_where_their_stay_is_whatever_land_it_names() {
    let mut east = east(10, clock_at(0));
    enter_with(
        &mut east,
        2,
        entering_place(Vec3::new(16.5, -60.0, 0.5), Vec::new(), 0),
    );
    let (entity, _) = east.player(player(2)).expect("player 2 is in the east");
    // The block of the west's land a maker saw them in, a step across the line.
    let at = BlockPos::new(15, -60, 0);
    let output = east.tick(&TickInputs {
        effects: filed(
            vec![to_player(
                2,
                entity,
                at,
                EntityUse::Effects(Box::new(speed_given())),
            )],
            1,
        ),
        ..TickInputs::default()
    });
    let added: Vec<String> = effects_added(&output, 2)
        .into_iter()
        .map(|instance| instance.effect)
        .collect();
    assert_eq!(added, ["minecraft:speed"]);
}
