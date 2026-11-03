//! The dragon's rules that read nothing of the world, from the game's code (d javap
//! EnderDragon.txt, its phases and DragonFlightHistory.txt), not from Clustine's: the
//! hurt a part takes and what is refused, its flight history, its graph's nodes as the
//! bytecode's arithmetic puts them (worked out apart, with the game's table of sines),
//! the nearest node and a path along the ring, and what it shows.

use clustine_data::entity_types;
use clustine_world::EntityId;

use super::nodes::{self, Path};
use super::phases::{self, DYING, HOLDING_PATTERN, HOVERING, SITTING_SCANNING};
use super::{DRAGON, History, PHASE_INDEX, Verdict, change, state_of, verdict};
use crate::mobs::Mob;
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::mobs::types::of;
use crate::shown::DataShown;

/// A player's hit (`playerAttack`).
fn by_player() -> DamageSource {
    DamageSource::by(
        DamageType::PLAYER_ATTACK,
        Causer {
            name: None,
            key: EntityId(7),
            kind: entity_types::PLAYER as u16,
            position: [0.0; 3],
            player: Some(7),
            living: true,
        },
    )
}

fn taken(verdict: Verdict) -> Option<f32> {
    match verdict {
        Verdict::Taken { amount, .. } => Some(amount),
        Verdict::Refused => None,
    }
}

#[test]
fn the_head_and_neck_take_a_hit_whole_and_every_other_part_a_quarter_and_up_to_one_more() {
    let hit = |part, amount| {
        taken(verdict(
            HOLDING_PATTERN,
            part,
            &by_player(),
            amount,
            200.0,
            [0.0; 3],
        ))
    };
    assert_eq!(hit(super::parts::HEAD, 10.0), Some(10.0));
    assert_eq!(hit(super::parts::NECK, 10.0), Some(10.0));
    // `a / 4 + min(a, 1)`.
    assert_eq!(hit(super::parts::WING[0], 10.0), Some(3.5));
    assert_eq!(hit(super::parts::BODY, 0.5), Some(0.625));
    assert_eq!(hit(super::parts::TAIL[2], 1.0), Some(1.25));
    // Below 0.01 after that, nothing.
    assert_eq!(hit(super::parts::BODY, 0.005), None);
}

#[test]
fn only_a_players_hurt_or_an_explosions_hurts_it_and_nothing_does_while_it_dies() {
    let fall = DamageSource::of(DamageType::FALL);
    assert_eq!(
        verdict(
            HOLDING_PATTERN,
            super::parts::HEAD,
            &fall,
            5.0,
            200.0,
            [0.0; 3]
        ),
        Verdict::Refused
    );
    // `#always_hurts_ender_dragons` is `#is_explosion`.
    let blast = DamageSource::of(DamageType::named("minecraft:explosion").expect("a type"));
    assert_eq!(
        taken(verdict(
            HOLDING_PATTERN,
            super::parts::HEAD,
            &blast,
            5.0,
            200.0,
            [0.0; 3]
        )),
        Some(5.0)
    );
    assert_eq!(
        verdict(DYING, super::parts::HEAD, &by_player(), 5.0, 1.0, [0.0; 3]),
        Verdict::Refused
    );
}

#[test]
fn a_sitting_dragon_takes_nothing_of_an_arrow_and_the_hover_takes_it() {
    let arrow = Causer {
        name: None,
        key: EntityId(9),
        kind: entity_types::ARROW as u16,
        position: [0.0; 3],
        player: None,
        living: false,
    };
    let shooter = Causer {
        name: None,
        key: EntityId(7),
        kind: entity_types::PLAYER as u16,
        position: [0.0; 3],
        player: Some(7),
        living: true,
    };
    let shot = DamageSource::thrown(arrow, [0.0, 0.0, 1.0], Some(shooter));
    assert_eq!(
        verdict(
            SITTING_SCANNING,
            super::parts::HEAD,
            &shot,
            6.0,
            200.0,
            [0.0; 3]
        ),
        Verdict::Refused
    );
    // The hover is sitting but keeps `AbstractDragonPhaseInstance.onHurt`.
    assert_eq!(
        taken(verdict(
            HOVERING,
            super::parts::HEAD,
            &shot,
            6.0,
            200.0,
            [0.0; 3]
        )),
        Some(6.0)
    );
}

#[test]
fn the_flight_history_is_filled_by_its_first_sample_and_reads_back_from_the_newest() {
    let mut history = History::default();
    assert_eq!(history.get(5), (0.0, 0.0));
    history.record(70.0, 10.0);
    assert_eq!(history.get(0), (70.0, 10.0));
    assert_eq!(history.get(16), (70.0, 10.0));
    history.record(71.0, 12.0);
    assert_eq!(history.get(0), (71.0, 12.0));
    assert_eq!(history.get(1), (70.0, 10.0));
}

/// The nodes as `findClosestNode()` puts them on a flat world whose columns are all
/// below 68: each at 73, about the origin as the game's table of sines places them.
#[test]
fn the_graphs_nodes_stand_on_three_rings_at_the_games_places() {
    let made = nodes::make_nodes(|_, _| -60);
    let expected: [(i32, i32); 24] = [
        (60, 0),
        (51, 30),
        (29, 51),
        (0, 60),
        (-31, 51),
        (-52, 29),
        (-60, 0),
        (-52, -31),
        (-30, -52),
        (0, -60),
        (29, -52),
        (51, -30),
        (40, 0),
        (28, 28),
        (0, 40),
        (-29, 28),
        (-40, 0),
        (-29, -29),
        (0, -40),
        (28, -29),
        (20, 0),
        (0, 20),
        (-20, 0),
        (0, -20),
    ];
    for (index, (node, (x, z))) in made.iter().zip(expected).enumerate() {
        assert_eq!(*node, [x, 73, z], "node {index}");
    }
    // Over a high column a node stands 5 above it, 15 on the middle ring.
    let high = nodes::make_nodes(|_, _| 100);
    assert_eq!(high[0][1], 105);
    assert_eq!(high[12][1], 115);
    assert_eq!(high[20][1], 105);
}

#[test]
fn the_nearest_node_is_looked_for_from_the_inner_rings_without_crystals() {
    let made = nodes::make_nodes(|_, _| -60);
    // By (58, 73, 1): node 0 of the outer ring, or the middle ring's 12 when only the
    // inner rings count.
    assert_eq!(nodes::closest(&made, 0, [58.5, 73.0, 1.5]), 0);
    assert_eq!(nodes::closest(&made, nodes::INNER, [58.5, 73.0, 1.5]), 12);
    // Nothing within 100 blocks: node 0, even among the inner rings.
    assert_eq!(nodes::closest(&made, nodes::INNER, [500.0, 73.0, 0.0]), 0);
}

#[test]
fn a_path_to_a_neighbour_on_the_ring_goes_straight_to_it_and_one_far_round_the_ring() {
    let made = nodes::make_nodes(|_, _| -60);
    // 12 and 19 are neighbours on the middle ring.
    let path = nodes::find_path(&made, nodes::INNER, 12, 19, None).expect("a path");
    assert_eq!(path.nodes, vec![made[12], made[19]]);
    assert_eq!(path.next, 0);
    // From 0 to 6 across the outer ring: every step is an edge of the graph.
    let path = nodes::find_path(&made, 0, 0, 6, None).expect("a path");
    assert_eq!(path.nodes.first(), Some(&made[0]));
    assert_eq!(path.nodes.last(), Some(&made[6]));
    for step in path.nodes.windows(2) {
        let from = made
            .iter()
            .position(|node| *node == step[0])
            .expect("a node");
        let to = made
            .iter()
            .position(|node| *node == step[1])
            .expect("a node");
        assert!(nodes::ADJACENCY[from] & (1 << to) != 0, "{from} to {to}");
    }
    // A final node comes after the last.
    let path = nodes::find_path(&made, nodes::INNER, 12, 19, Some([0, -60, 0])).expect("a path");
    assert_eq!(path.nodes.last(), Some(&[0, -60, 0]));
    // From a node to itself: that node alone.
    let path = nodes::find_path(&made, nodes::INNER, 12, 12, None).expect("a path");
    assert_eq!(path.nodes, vec![made[12]]);
    let mut done = Path {
        nodes: vec![made[12]],
        next: 0,
    };
    done.advance();
    assert!(done.is_done());
}

#[test]
fn the_phases_by_id_and_what_sits() {
    assert_eq!(phases::by_id(4), phases::TAKEOFF);
    assert_eq!(phases::by_id(11), HOLDING_PATTERN);
    assert_eq!(phases::by_id(-1), HOLDING_PATTERN);
    for sitting in [5, 6, 7, 10] {
        assert!(phases::is_sitting(sitting));
    }
    for flying in [0, 1, 2, 3, 4, 8, 9] {
        assert!(!phases::is_sitting(flying));
    }
}

#[test]
fn it_shows_its_phase_as_an_int_hovering_by_default() {
    let dragon = of(DRAGON);
    let mut mob = Mob::fresh();
    assert_eq!(state_of(&mob).phase, HOVERING);
    assert_eq!(
        dragon.shown(&mob),
        vec![(PHASE_INDEX, DataShown::Int(10), true)]
    );
    change(&mut mob, |own| own.phase = SITTING_SCANNING);
    assert_eq!(
        dragon.shown(&mob),
        vec![(PHASE_INDEX, DataShown::Int(6), false)]
    );
}

#[test]
fn setting_a_phase_begins_it_and_leaving_the_flame_discards_its_cloud() {
    let mut mob = Mob::fresh();
    change(&mut mob, |own| {
        own.phase = phases::SITTING_FLAMING;
        own.phases.flaming.flame = true;
        own.phases.flaming.count = 2;
    });
    let locals = super::set_phase(&mut mob, phases::TAKEOFF);
    assert_eq!(
        locals,
        vec![crate::mobs::tick::Local::Dragon(Box::new(
            super::DragonLocal::DiscardFlame
        ))]
    );
    let own = state_of(&mob);
    assert_eq!(own.phase, phases::TAKEOFF);
    assert!(own.phases.takeoff.first_tick);
    // To the phase it is in: nothing.
    assert!(super::set_phase(&mut mob, phases::TAKEOFF).is_empty());
    // The flame's count goes up as it begins.
    super::set_phase(&mut mob, phases::SITTING_FLAMING);
    assert_eq!(state_of(&mob).phases.flaming.count, 3);
}
