//! A player's effects, cooldowns and time since rest, tested from the plan
//! (`docs/groundwork/n9-plan.md`, sections 5.1, 5.4 and 14, and decision 8 of section
//! 18) by someone who did not write them: an effect added by a drink, told to its
//! player and ended after its duration; kept on the region's ticks, so that its end
//! does not move when the world clock jumps forward a second at a publication or when
//! the player arrives in a region whose clock is a second ahead; carried as the ticks
//! left across a hand-over, a leave and a join, and a merge; given across a pinned line
//! by a splash; and a cooldown and the time since rest likewise.
//!
//! The rule held to throughout is the plan's "an effect does not run down while its
//! player is away, as the game's", and its converse: it runs down once in each tick the
//! player is in a region, wherever they are and whatever the world clock says. So a
//! test counts the ticks from an effect's adding to its end, with whatever happens in
//! between, and compares them with the effect's duration.

use clustine_data::items;
use clustine_world::components::EffectInstance;
use clustine_world::{Effect, Effected};

use super::n9_world::*;
use super::*;
use crate::api::WorldClock;
use crate::mobs::body::effects::{Effect as MobEffect, Layer};

const SPEED: &str = "minecraft:speed";

/// Adds speed for `duration` ticks to player `number` between two ticks of `region`.
fn give_speed(region: &mut Region, number: u128, duration: i32) {
    let layer = Layer::new(0, duration, region.tick_number());
    let mut output = TickOutput::default();
    assert!(region.add_player_effect(player(number), MobEffect::SPEED, layer, &mut output));
}

/// The ticks `region` runs until player `number` is told their speed ended.
fn ticks_to_the_end(region: &mut Region, number: u128) -> u32 {
    ticks_until(region, 5_000, |output| {
        effects_removed(output, number).contains(&SPEED.to_owned())
    })
    .expect("the speed ends")
}

/// Player `number` standing at (`x`, -60, 0.5) in `region`, who entered it with nothing.
fn standing_in(region: &mut Region, number: u128, x: f64) {
    enter_with(
        region,
        number,
        entering_place(Vec3::new(x, -60.0, 0.5), Vec::new(), 0),
    );
}

/// Section 5.4 and 5.5: drinking a potion adds its effects at the end of the drink,
/// 32 ticks counting the one it began in; the player is told the effect with its level
/// and duration, and told it ended that many ticks later.
#[test]
fn a_potion_drunk_adds_its_effect_which_its_player_is_told_of_and_which_ends_its_duration_later() {
    let mut region = alone(0, clock_at(0));
    standing_in(&mut region, 1, 0.5);
    hold(
        &mut region,
        1,
        potion(items::POTION, vec![instance(SPEED, 1, 60)]),
    );
    let output = acting(&mut region, 1, vec![use_item(0.0)]);
    assert!(effects_added(&output, 1).is_empty());
    let drunk = ticks_until(&mut region, 100, |output| {
        !effects_added(output, 1).is_empty()
    })
    .expect("the drink ends");
    assert_eq!(
        drunk + 1,
        32,
        "the drink lasts 1.6 s, its first tick counted"
    );
    let ended = ticks_to_the_end(&mut region, 1);
    assert_eq!(ended, 60, "sixty ticks from the tick it was added in");
}

/// The effect a drink adds is told with its level and its whole duration.
#[test]
fn the_effect_a_drink_adds_is_told_with_its_level_and_whole_duration() {
    let mut region = alone(0, clock_at(0));
    standing_in(&mut region, 1, 0.5);
    hold(
        &mut region,
        1,
        potion(items::POTION, vec![instance(SPEED, 1, 60)]),
    );
    acting(&mut region, 1, vec![use_item(0.0)]);
    let mut added = Vec::new();
    ticks_until(&mut region, 100, |output| {
        added = effects_added(output, 1);
        !added.is_empty()
    })
    .expect("the drink ends");
    assert_eq!(added, [instance(SPEED, 1, 60)]);
}

/// Decision 8 of section 18 (finding 1): an effect's end is kept in the region's ticks,
/// not the world's game time, which jumps forward by up to a second when the store
/// publishes its clock. With the clock jumping a second ahead every so often, the
/// effect ends in as many ticks as without.
#[test]
fn an_effect_s_end_does_not_move_when_the_world_clock_jumps_a_second_at_a_publication() {
    let mut steady = alone(0, clock_at(1_000));
    standing_in(&mut steady, 1, 0.5);
    give_speed(&mut steady, 1, 100);
    let without = ticks_to_the_end(&mut steady, 1);
    assert_eq!(without, 100);

    let mut jumping = alone(0, clock_at(1_000));
    standing_in(&mut jumping, 1, 0.5);
    give_speed(&mut jumping, 1, 100);
    let mut ticked = 0;
    let mut ended = None;
    while ended.is_none() && ticked < 400 {
        ticked += 1;
        // Every thirtieth tick the store publishes a clock a second ahead.
        let clock = (ticked % 30 == 0).then(|| {
            let now = jumping.clock;
            WorldClock {
                game_time: now.game_time + 21,
                day: now.day + 21,
            }
        });
        let output = jumping.tick(&TickInputs {
            clock,
            ..TickInputs::default()
        });
        if clock.is_some() {
            assert!(output.clock_jumped, "the clock jumped in tick {ticked}");
        }
        if effects_removed(&output, 1).contains(&SPEED.to_owned()) {
            ended = Some(ticked);
        }
    }
    assert_eq!(ended, Some(without));
}

/// Section 5.1: a mob's effect ticks on its own `lived`, its intervals too. A cow
/// poisoned for 100 ticks is hurt on the same ticks whether the world clock jumps a
/// second ahead or not.
#[test]
fn a_mob_s_poison_hurts_on_the_same_ticks_whatever_the_world_clock_does() {
    let hurts = |jump: bool| {
        let mut region = alone(0, clock_at(1_000));
        let key = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
        let mut cow = super::thrown::cow(key, [3.5, -60.0, 0.5]);
        let mut mob = crate::mobs::Mob::decode(&cow.data).expect("a mob's record");
        mob.body
            .effects
            .add(MobEffect::POISON, Layer::new(0, 100, 0), 0);
        cow.data = mob.encode();
        super::thrown::lay(&mut region, &cow);
        let mut healths = Vec::new();
        for tick in 1..=120 {
            let clock = (jump && tick % 25 == 7).then(|| {
                let now = region.clock;
                WorldClock {
                    game_time: now.game_time + 20,
                    day: now.day + 20,
                }
            });
            region.tick(&TickInputs {
                clock,
                ..TickInputs::default()
            });
            healths.push(super::thrown::health(&region, key));
        }
        healths
    };
    let steady = hurts(false);
    assert!(
        steady.first() > steady.last(),
        "the poison hurts: {steady:?}"
    );
    assert_eq!(hurts(true), steady);
}

/// Section 5.4 and decision 8: a hand-over carries each effect's ticks left, and the next
/// region counts them on from its own tick, whatever its tick's number and its world
/// clock, here a second ahead. The effect ends in as many of the player's ticks as had
/// they stayed.
#[test]
fn a_hand_over_into_a_region_whose_clock_is_a_second_ahead_ends_the_effect_on_time() {
    let mut west = west(10, clock_at(5_000));
    // The eastern region numbers its ticks otherwise and is a second ahead.
    let mut east = east(7_777, clock_at(5_021));
    standing_in(&mut west, 1, 14.5);
    give_speed(&mut west, 1, 100);
    let mut in_west = 0;
    for _ in 0..30 {
        let output = west.tick(&TickInputs::default());
        assert!(effects_removed(&output, 1).is_empty());
        in_west += 1;
    }
    let output = acting(
        &mut west,
        1,
        vec![PlayerInput::Move {
            position: Some(Vec3::new(17.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        }],
    );
    in_west += 1;
    let [(_, transfer)] = &departures(&output)[..] else {
        panic!("one departure: {:?}", output.durable);
    };
    // The next tick of the world is the arrival's.
    let arrival = east.tick(&changes(vec![arrive(1, transfer)]));
    assert!(east.player(player(1)).is_some(), "player 1 arrived");
    let in_east = if effects_removed(&arrival, 1).contains(&SPEED.to_owned()) {
        1
    } else {
        1 + ticks_to_the_end(&mut east, 1)
    };
    assert_eq!(in_west + in_east, 100, "{in_west} in the west");
}

/// Section 5.4: at a join the place's effects are the player's again, each with the
/// ticks it has left: told to them at once, in a region of other tick numbers a second
/// ahead of the one they left, and ended that many ticks after.
#[test]
fn a_join_gives_back_the_place_s_effects_with_the_ticks_they_have_left() {
    let mut region = alone(9_000, clock_at(2_061));
    let place = entering_place(Vec3::new(0.5, -60.0, 0.5), vec![instance(SPEED, 0, 60)], 0);
    let entering = enter_with(&mut region, 1, place);
    let told: Vec<EffectInstance> = entering
        .iter()
        .flat_map(|output| effects_added(output, 1))
        .collect();
    assert_eq!(told, [instance(SPEED, 0, 60)], "told at the join");
    assert_eq!(ticks_to_the_end(&mut region, 1), 60);
}

/// Section 5.4: "an effect does not run down while its player is away", nor does it run
/// on. The place the store keeps of a player who leaves (the last `StayNote::Has` of
/// their stay, ADR-0020) has to have each effect with the ticks it had left at the
/// leave, and the time since rest as it was then; a player who stood still has their
/// effects back at a join with the ticks they had when they last changed otherwise. (A
/// game client says where it is at least once a second, which notes the stay, so with
/// one the place is at most twenty ticks old: `bin/clustine/tests/n9_lines.rs`.)
#[test]
#[ignore = "finding: the place kept at a leave has the effects' ticks and the time since rest of the stay's last noted change, not of the leave"]
fn the_place_kept_at_a_leave_has_each_effect_s_ticks_left_and_the_time_since_rest_of_the_leave() {
    let mut first = alone(5_000, clock_at(2_000));
    let mut outputs = enter_with(
        &mut first,
        1,
        entering_place(Vec3::new(0.5, -60.0, 0.5), Vec::new(), 0),
    );
    give_speed(&mut first, 1, 100);
    for _ in 0..40 {
        outputs.push(first.tick(&TickInputs::default()));
    }
    let entity = first.player(player(1)).expect("there").0;
    outputs.push(first.tick(&changes(vec![PlayerChange::Leave(
        EDGE,
        player(1),
        Some(entity),
        None,
    )])));
    assert!(first.player(player(1)).is_none(), "player 1 left");
    let place = last_place(&outputs, 1).expect("a place is kept");
    let [kept] = &place.effects[..] else {
        panic!("one effect kept: {:?}", place.effects);
    };
    assert_eq!(kept.effect, SPEED);
    // Forty of its hundred ticks ran, give or take the leave's own tick.
    assert!(
        (59..=61).contains(&kept.duration),
        "kept with {} ticks left",
        kept.duration
    );
    assert!(
        place.time_since_rest >= 40,
        "the time since rest counted the forty ticks: {}",
        place.time_since_rest
    );
}

/// The leave and the join together, as a player sees them.
#[test]
#[ignore = "finding: the place kept at a leave has the effects' ticks and the time since rest of the stay's last noted change, not of the leave"]
fn an_effect_saved_at_a_leave_comes_back_at_the_join_with_the_ticks_it_had_left() {
    let mut first = alone(0, clock_at(2_000));
    let mut outputs = enter_with(
        &mut first,
        1,
        entering_place(Vec3::new(0.5, -60.0, 0.5), Vec::new(), 0),
    );
    give_speed(&mut first, 1, 100);
    for _ in 0..40 {
        outputs.push(first.tick(&TickInputs::default()));
    }
    let entity = first.player(player(1)).expect("there").0;
    outputs.push(first.tick(&changes(vec![PlayerChange::Leave(
        EDGE,
        player(1),
        Some(entity),
        None,
    )])));
    assert!(first.player(player(1)).is_none(), "player 1 left");
    let place = last_place(&outputs, 1).expect("a place is kept");
    let [kept] = &place.effects[..] else {
        panic!("one effect kept: {:?}", place.effects);
    };
    assert_eq!(kept.effect, SPEED);
    assert_eq!(kept.duration, 60, "forty ticks of a hundred ran");

    let mut second = alone(9_000, clock_at(2_061));
    let entering = enter_with(&mut second, 1, place);
    let told: Vec<EffectInstance> = entering
        .iter()
        .flat_map(|output| effects_added(output, 1))
        .collect();
    assert_eq!(told, [instance(SPEED, 0, 60)], "told at the join");
    assert_eq!(ticks_to_the_end(&mut second, 1), 60);
}

/// Section 5.4 and decision 8: `PlayerState` at a merge carries the ticks left, and the
/// survivor counts the ends on from its own tick. Two regions' tick numbers differ by
/// the absorbed one's lag; the effect of a player of the absorbed region ends after the
/// merge in the ticks it had left.
#[test]
fn a_merge_ends_an_absorbed_player_s_effect_in_the_ticks_it_had_left() {
    // The absorbed region lags the survivor by forty ticks.
    let mut absorbed = west(1_000, clock_at(3_000));
    let survivor = east(1_040, clock_at(3_000));
    standing_in(&mut absorbed, 1, 14.5);
    give_speed(&mut absorbed, 1, 100);
    for _ in 0..30 {
        absorbed.tick(&TickInputs::default());
    }
    let state = survivor.absorb(WEST_ID, &absorbed.state());
    let holdings = Holdings {
        held: (-2..=3)
            .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
            .collect(),
        pinned: vec![ChunkArea::EVERYWHERE],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let tickets: Vec<(ChunkPos, Ticket)> = holdings
        .held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let chunks = holdings
        .held
        .iter()
        .map(|position| (*position, floor()))
        .collect();
    let mut merged = Region::restore_loaded(
        RegionConfig {
            return_after: u64::MAX,
            seed: SEED,
            ..config()
        },
        state,
        holdings,
        &tickets,
        chunks,
        Vec::new(),
    );
    assert!(merged.player(player(1)).is_some(), "player 1 came with it");
    let after = ticks_to_the_end(&mut merged, 1);
    // Seventy were left; the merge's own tick may count as one of the player's.
    assert!(
        (69..=71).contains(&after),
        "ended {after} ticks after the merge, of seventy left"
    );
}

/// The cooldown a pearl's throw begins, `use_cooldown`'s second.
fn cooldown_events(output: &TickOutput, number: u128) -> Vec<i32> {
    told_to(output, number)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::Cooldown { group, ticks } if group == "minecraft:ender_pearl" => {
                Some(ticks)
            }
            _ => None,
        })
        .collect()
}

/// Section 4.4 and decision 8: a pearl's cooldown of a second, told at its start and at
/// its end, ends twenty ticks after the throw however the world clock jumps, and in
/// twenty of the player's ticks across a hand-over into a region a second ahead.
#[test]
fn a_cooldown_ends_on_the_player_s_ticks_through_clock_jumps_and_a_hand_over() {
    let mut region = alone(0, clock_at(0));
    standing_in(&mut region, 1, 0.5);
    hold(&mut region, 1, ItemStack::new(items::ENDER_PEARL, 1));
    let output = acting(&mut region, 1, vec![use_item(90.0)]);
    assert_eq!(cooldown_events(&output, 1), [20]);
    let mut ticked = 0;
    let ended = loop {
        ticked += 1;
        assert!(ticked < 100, "the cooldown ends");
        let clock = (ticked % 7 == 0).then(|| WorldClock {
            game_time: region.clock.game_time + 20,
            day: region.clock.day + 20,
        });
        let output = region.tick(&TickInputs {
            clock,
            ..TickInputs::default()
        });
        if cooldown_events(&output, 1) == [0] {
            break ticked;
        }
    };
    assert_eq!(ended, 20, "through the clock's jumps");

    let mut west = west(10, clock_at(100));
    let mut east = east(4_321, clock_at(121));
    standing_in(&mut west, 1, 14.5);
    hold(&mut west, 1, ItemStack::new(items::ENDER_PEARL, 1));
    let output = acting(&mut west, 1, vec![use_item(90.0)]);
    assert_eq!(cooldown_events(&output, 1), [20]);
    let mut in_west = 0;
    for _ in 0..5 {
        west.tick(&TickInputs::default());
        in_west += 1;
    }
    let output = acting(
        &mut west,
        1,
        vec![PlayerInput::Move {
            position: Some(Vec3::new(17.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        }],
    );
    in_west += 1;
    let [(_, transfer)] = &departures(&output)[..] else {
        panic!("one departure: {:?}", output.durable);
    };
    let arrival = east.tick(&changes(vec![arrive(1, transfer)]));
    let in_east = if cooldown_events(&arrival, 1) == [0] {
        1
    } else {
        1 + ticks_until(&mut east, 100, |output| cooldown_events(output, 1) == [0])
            .expect("the cooldown ends in the east")
    };
    assert_eq!(in_west + in_east, 20, "{in_west} in the west");
}

/// Player `number` steps from where they stand to `x` in one tick, and is let go there:
/// the transfer their region hands on.
fn step_across(region: &mut Region, number: u128, x: f64) -> PlayerTransfer {
    let output = acting(
        region,
        number,
        vec![PlayerInput::Move {
            position: Some(Vec3::new(x, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        }],
    );
    let [(_, transfer)] = &departures(&output)[..] else {
        panic!("one departure: {:?}", output.durable);
    };
    transfer.clone()
}

/// Section 9.7 and decision 8: the time since rest counts the player's ticks, as the
/// game's statistic does, kept on the region's ticks: a clock that jumps a second ahead
/// adds nothing to it, and a hand-over into a region whose clock is a second ahead and
/// whose ticks are numbered otherwise counts on from what it was.
#[test]
fn the_time_since_rest_counts_the_player_s_ticks_and_not_the_clock_s_jumps() {
    let run = |jump: bool| {
        let mut region = west(5_000, clock_at(0));
        enter_with(
            &mut region,
            1,
            entering_place(Vec3::new(14.5, -60.0, 0.5), Vec::new(), 1_000),
        );
        for tick in 1..=60 {
            let clock = (jump && tick % 20 == 0).then(|| WorldClock {
                game_time: region.clock.game_time + 20,
                day: region.clock.day + 20,
            });
            region.tick(&TickInputs {
                clock,
                ..TickInputs::default()
            });
        }
        step_across(&mut region, 1, 17.5).time_since_rest
    };
    let steady = run(false);
    assert!(
        (1_060..=1_063).contains(&steady),
        "sixty ticks and those of the join and the step on 1,000: {steady}"
    );
    assert_eq!(run(true), steady);

    // Thirty ticks in the east, a second ahead, and back.
    let mut west = west(5_000, clock_at(0));
    let mut east = east(9_999, clock_at(21));
    enter_with(
        &mut west,
        1,
        entering_place(Vec3::new(14.5, -60.0, 0.5), Vec::new(), 1_000),
    );
    let there = step_across(&mut west, 1, 17.5);
    east.tick(&changes(vec![arrive(1, &there)]));
    for _ in 0..29 {
        east.tick(&TickInputs::default());
    }
    let back = step_across(&mut east, 1, 14.5);
    // The arrival's tick, twenty-nine more and the step's.
    assert_eq!(back.time_since_rest - there.time_since_rest, 31);
}

/// Sections 4.4 and 8.3: a splash potion that breaks by the line gives its effects to
/// a player of the region across it, whose face the thrower's region keeps, by an
/// effect addressed to that player; their region adds them and tells them.
#[test]
fn a_splash_by_a_pinned_line_gives_its_effects_to_the_player_of_the_region_across_it() {
    let mut west = west(10, clock_at(0));
    let mut east = east(10, clock_at(0));
    // Player 2 stands just east of the line, in the eastern region.
    standing_in(&mut east, 2, 16.8);
    let (entity, _) = east.player(player(2)).expect("player 2 is in the east");
    // The western region keeps their face, in its copy of the column east of the line.
    west.tick(&TickInputs {
        faces: vec![face_of(2, entity, [16.8, -60.0, 0.5], 1)],
        ..TickInputs::default()
    });
    // Player 1 throws a splash potion at their own feet, a block and a half west of
    // player 2.
    standing_in(&mut west, 1, 15.2);
    hold(
        &mut west,
        1,
        potion(items::SPLASH_POTION, vec![instance(SPEED, 0, 600)]),
    );
    let mut outputs = vec![acting(&mut west, 1, vec![use_item(90.0)])];
    for _ in 0..40 {
        outputs.push(west.tick(&TickInputs::default()));
    }
    let thrower_got = outputs
        .iter()
        .any(|output| !effects_added(output, 1).is_empty());
    assert!(thrower_got, "the thrower is splashed too");
    let to_the_east: Vec<Effect> = outputs
        .iter()
        .flat_map(|output| output.effects.iter().cloned())
        .filter(|effect| effect.at.x >= 16)
        .collect();
    assert!(
        to_the_east.iter().any(|effect| matches!(
            &effect.what,
            Effected::OnPlayer { player: to, entity: stay, .. } if *to == player(2) && *stay == entity
        )),
        "addressed to player 2: {to_the_east:?}"
    );
    let output = east.tick(&TickInputs {
        effects: filed(to_the_east, 1),
        ..TickInputs::default()
    });
    let told = effects_added(&output, 2);
    let [given] = &told[..] else {
        panic!("one effect given: {:?}", output.player_events);
    };
    assert_eq!(given.effect, SPEED);
    // Scaled by how near the splash was, and longer than the 20 ticks a splash drops.
    assert!(given.duration > 20 && given.duration < 600, "{given:?}");
}
