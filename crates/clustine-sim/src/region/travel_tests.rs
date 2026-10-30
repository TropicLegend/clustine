//! Tests of a Nether portal's travel, tick by tick (`docs/groundwork/p-plan.md`, sections
//! 16.2 to 16.4, 16.7 and 16.8): the source lets a player go to the destination's home
//! in the tick the clock fires, and files an entity for the target's chunk; the
//! destination holds the traveller waiting, asks the index, and places it beside the
//! portal the index names, or one it makes.

use clustine_data::{blocks, entity_types, items};
use clustine_world::{
    BlockPos, ChunkPos, Dimension, Double, EdgeId, EffectId, EntityId, LevelChunk, PlayerId,
    PortalAxis, RegionId, Travel, Traveller, Way,
};

use super::super::Region;
use super::super::portals::tests::{
    LEFT, RIGHT, Z, frame_x, inside, lit_portal, portal, use_on, world_in,
};
use super::super::tnt::tests::{FLOOR, add_player, at, cart, idle};
use crate::api::{
    Durable, EdgeEvent, Face as ClickFace, PlayerChange, PlayerTransfer, PortalAnswer, RegionEvent,
    StayNote, TickInputs, TickOutput,
};

const EDGE: EdgeId = EdgeId(1);
const NETHER_HOME: RegionId = RegionId(7);

/// The Nether's floor in the destination's world.
const NETHER_FLOOR: i32 = 40;

fn with_homes(region: &mut Region) {
    region.config.homes = vec![
        (Dimension::Overworld, RegionId(0)),
        (Dimension::Nether, NETHER_HOME),
    ];
}

fn tick_with(region: &mut Region, inputs: TickInputs) -> TickOutput {
    region.tick(&inputs)
}

fn start_edge(region: &mut Region) {
    tick_with(
        region,
        TickInputs {
            edges: vec![EdgeEvent::Started {
                edge: EDGE,
                start: 1,
            }],
            ..TickInputs::default()
        },
    );
}

/// The departure of `id` the tick sent their edge, if it sent one.
fn departed(output: &TickOutput, id: PlayerId) -> Option<(PlayerTransfer, RegionId)> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Departed {
            player,
            transfer,
            to,
        } if *player == id => Some((transfer.clone(), *to)),
        _ => None,
    })
}

/// The overworld with a lit portal, a known edge and the homes, and a creative player
/// standing in the portal, who fires on the second tick.
fn player_in_portal() -> (Region, PlayerId, EntityId) {
    let mut region = lit_portal();
    with_homes(&mut region);
    start_edge(&mut region);
    let feet = [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5];
    let (id, entity) = add_player(&mut region, 1, feet, false);
    (region, id, entity)
}

#[test]
fn a_player_whose_portal_fires_is_let_go_to_the_nethers_home_with_the_travel() {
    let (mut region, id, entity) = player_in_portal();
    let marked = idle(&mut region);
    assert!(departed(&marked, id).is_none());
    let fired = idle(&mut region);
    let (transfer, to) = departed(&fired, id).expect("let go in the tick it fired");
    assert_eq!(to, NETHER_HOME);
    assert!(!region.players.contains_key(&id));
    // The target is the scaled feet, (3 / 8, y, 5.5 / 8) as a block; the place in the
    // 2 × 3 portal from x 2 is the middle along, the bottom, in the plane.
    let travel = Travel {
        to: Dimension::Nether,
        target: at(0, FLOOR + 2, 0),
        way: Way::NetherPortal {
            axis: Some(PortalAxis::X),
            relative: [Double(0.5), Double(0.0), Double(0.0)],
        },
    };
    assert_eq!(transfer.travel, Some(travel));
    assert_eq!(transfer.dimension, Dimension::Nether);
    assert_eq!(transfer.entity_id, entity);
    assert_eq!(transfer.attempt, None);
    let position = transfer.pose.position;
    assert_eq!(
        [position.x, position.y, position.z],
        [0.5, f64::from(FLOOR + 2), 0.5]
    );
    // Removed for whoever watched them at the portal.
    assert!(fired.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityRemoved { entity: removed, chunk } if *removed == entity && *chunk == ChunkPos::new(0, 0)
    )));
    // The store keeps the place at the target, with the travel, let go.
    assert!(fired.stays.iter().any(|note| matches!(
        note,
        StayNote::Has { player, let_go: true, place, .. }
            if *player == id && place.travel == Some(travel) && place.dimension == Dimension::Nether
    )));
}

#[test]
fn a_cart_whose_portal_fires_is_filed_for_the_targets_chunk_and_reported_gone_by_a_portal() {
    let mut region = lit_portal();
    with_homes(&mut region);
    let key = cart(
        &mut region,
        entity_types::MINECART,
        [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5],
    );
    idle(&mut region);
    let fired = idle(&mut region);
    assert!(region.find_entity(key).is_none());
    assert_eq!(fired.travellers.len(), 1);
    let traveller = &fired.travellers[0];
    assert_eq!(
        traveller.chunk,
        LevelChunk::new(Dimension::Nether, ChunkPos::new(0, 0))
    );
    assert_eq!(traveller.travel.to, Dimension::Nether);
    assert_eq!(traveller.entities.len(), 1);
    assert_eq!(traveller.entities[0].key, key);
    assert_eq!(traveller.entities[0].hops, 1);
    assert!(fired.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityGone { entity, travelled: true, hops: 0, .. } if *entity == key
    )));
}

#[test]
fn without_the_destinations_home_nothing_goes() {
    let mut region = lit_portal();
    start_edge(&mut region);
    let feet = [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5];
    let (id, _) = add_player(&mut region, 1, feet, false);
    idle(&mut region);
    let fired = idle(&mut region);
    assert!(departed(&fired, id).is_none());
    assert!(region.players.contains_key(&id));
}

// The destination.

/// A Nether region with a floor at [`NETHER_FLOOR`], the homes and a known edge, and
/// the blocks `blocks` names.
fn nether(blocks: impl Fn(BlockPos) -> Option<clustine_data::BlockState>) -> Region {
    let mut region = world_in(Dimension::Nether, NETHER_FLOOR, blocks);
    with_homes(&mut region);
    start_edge(&mut region);
    region
}

/// A player on their way to the Nether, aimed at `target`, through a portal along x
/// whose middle bottom they stood at.
fn travelling(target: BlockPos) -> (PlayerId, PlayerTransfer) {
    let mut source = lit_portal();
    with_homes(&mut source);
    start_edge(&mut source);
    let feet = [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5];
    let (id, _) = add_player(&mut source, 1, feet, false);
    idle(&mut source);
    let (mut transfer, _) = departed(&idle(&mut source), id).expect("let go");
    let travel = transfer.travel.as_mut().expect("a travel");
    travel.target = target;
    transfer.pose.position.x = f64::from(target.x) + 0.5;
    transfer.pose.position.y = f64::from(target.y);
    transfer.pose.position.z = f64::from(target.z) + 0.5;
    (id, transfer)
}

fn arrive(region: &mut Region, id: PlayerId, transfer: PlayerTransfer) -> TickOutput {
    tick_with(
        region,
        TickInputs {
            player_changes: vec![PlayerChange::Arrive(EDGE, id, transfer)],
            ..TickInputs::default()
        },
    )
}

fn answer(region: &mut Region, output: &TickOutput, portals: Vec<BlockPos>) -> TickOutput {
    let answers = output
        .portal_queries
        .iter()
        .map(|query| PortalAnswer {
            id: query.id,
            portals: portals.clone(),
        })
        .collect();
    tick_with(
        region,
        TickInputs {
            portal_answers: answers,
            ..TickInputs::default()
        },
    )
}

fn placed(output: &TickOutput, id: PlayerId) -> Option<[f64; 3]> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Placed {
            player,
            pose,
            dimension: Dimension::Nether,
            ..
        } if *player == id => Some([pose.position.x, pose.position.y, pose.position.z]),
        _ => None,
    })
}

#[test]
fn a_player_with_a_travel_waits_apart_and_asks_the_index_once() {
    let mut region = nether(|_| None);
    let target = at(0, NETHER_FLOOR + 1, 0);
    let (id, transfer) = travelling(target);
    let taken = arrive(&mut region, id, transfer);
    assert!(!region.players.contains_key(&id));
    assert!(region.waiting_state(id).is_some());
    assert_eq!(taken.portal_queries.len(), 1);
    assert_eq!(taken.portal_queries[0].centre, target);
    assert_eq!(taken.portal_queries[0].radius, 16);
    // Noted where they wait, with the travel.
    assert!(taken.stays.iter().any(|note| matches!(
        note,
        StayNote::Has { player, let_go: false, place, .. } if *player == id && place.travel.is_some()
    )));
    // Without the answer nothing is placed, and nothing is asked again.
    for _ in 0..3 {
        let output = idle(&mut region);
        assert!(output.portal_queries.is_empty());
        assert!(placed(&output, id).is_none());
    }
    assert!(region.waiting_state(id).is_some());
}

#[test]
fn a_player_is_placed_in_the_portal_the_index_names_at_their_place_in_the_one_they_left() {
    let mut region = nether(frame_x(NETHER_FLOOR));
    assert!(use_on(
        &mut region,
        at(LEFT, NETHER_FLOOR + 1, Z),
        ClickFace::Top,
        items::FLINT_AND_STEEL
    ));
    let portals = inside(NETHER_FLOOR);
    let (id, transfer) = travelling(at(0, NETHER_FLOOR + 1, 0));
    let taken = arrive(&mut region, id, transfer);
    let output = answer(&mut region, &taken, portals.clone());
    // The middle of the portal along x, its bottom, its plane: (2 + 0.3 + 1.4 × 0.5,
    // the bottom, 5 + 0.5).
    assert_eq!(
        placed(&output, id),
        Some([3.0, f64::from(NETHER_FLOOR + 2), 5.5])
    );
    assert!(region.players.contains_key(&id));
    assert!(region.waiting_state(id).is_none());
    // No portal was made.
    for pos in portals {
        assert_eq!(region.block(pos), Some(portal("x")));
    }
}

#[test]
fn where_the_index_names_no_portal_one_is_made_and_the_player_is_placed_in_it() {
    let mut region = nether(|_| None);
    let (id, transfer) = travelling(at(0, NETHER_FLOOR + 1, 0));
    let taken = arrive(&mut region, id, transfer);
    let output = answer(&mut region, &taken, Vec::new());
    let at_feet = placed(&output, id).expect("placed");
    let feet = BlockPos::new(
        at_feet[0].floor() as i32,
        at_feet[1].floor() as i32,
        at_feet[2].floor() as i32,
    );
    // They stand in a portal block of the portal made, on obsidian.
    assert!(
        region
            .block(feet)
            .is_some_and(|state| clustine_blocks::data::is_block(state, blocks::NETHER_PORTAL)),
        "{feet:?}: {:?}",
        region.block(feet)
    );
    assert_eq!(
        region.block(BlockPos::new(feet.x, feet.y - 1, feet.z)),
        Some(blocks::OBSIDIAN)
    );
}

#[test]
fn two_travellers_placed_in_one_tick_share_the_portal_the_first_made() {
    let mut region = nether(|_| None);
    let target = at(0, NETHER_FLOOR + 1, 0);
    let (id, transfer) = travelling(target);
    let mut source = lit_portal();
    with_homes(&mut source);
    let key = cart(
        &mut source,
        entity_types::MINECART,
        [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5],
    );
    idle(&mut source);
    let mut traveller: Traveller = idle(&mut source).travellers.remove(0);
    traveller.travel.target = target;
    let number = EffectId {
        sequence: 1,
        index: 0,
    };
    let taken = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![PlayerChange::Arrive(EDGE, id, transfer)],
            travellers: vec![(number, traveller)],
            ..TickInputs::default()
        },
    );
    assert_eq!(taken.arrivals_taken, [number]);
    assert_eq!(taken.portal_queries.len(), 2);
    let output = answer(&mut region, &taken, Vec::new());
    assert!(placed(&output, id).is_some());
    assert!(region.find_entity(key).is_some());
    // One portal of six blocks: the second traveller found the first's.
    let mut made = 0;
    for x in -20..=20 {
        for z in -20..=20 {
            // Wherever the portal was made, on the floor or on the game's platform.
            for y in 0..128 {
                if region.block(at(x, y, z)).is_some_and(|state| {
                    clustine_blocks::data::is_block(state, blocks::NETHER_PORTAL)
                }) {
                    made += 1;
                }
            }
        }
    }
    assert_eq!(made, 6);
}

#[test]
fn a_leave_ends_a_waiting_stay_with_its_last_place_kept() {
    let mut region = nether(|_| None);
    let (id, transfer) = travelling(at(0, NETHER_FLOOR + 1, 0));
    let entity = transfer.entity_id;
    arrive(&mut region, id, transfer);
    let output = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![PlayerChange::Leave(EDGE, id, Some(entity), None)],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_none());
    assert!(output.stays.iter().any(|note| matches!(
        note,
        StayNote::Gone { player, entity: gone, .. } if *player == id && *gone == entity
    )));
}

#[test]
fn an_edge_gone_ends_its_waiting_stays() {
    let mut region = nether(|_| None);
    let (id, transfer) = travelling(at(0, NETHER_FLOOR + 1, 0));
    arrive(&mut region, id, transfer);
    let output = tick_with(
        &mut region,
        TickInputs {
            edges: vec![EdgeEvent::Gone { edge: EDGE }],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_none());
    assert!(output.stays.iter().any(|note| matches!(
        note,
        StayNote::Gone { player, .. } if *player == id
    )));
}
