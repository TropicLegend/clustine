//! Tests of the End portal's travel into the End, tick by tick (`docs/groundwork/p-plan.md`,
//! sections 19.2 P10b, 19.5 and 19.6; `docs/groundwork/p-brief.md`, section 2.4): the
//! portal fires at once and lets a player go to the End's home with the travel to the
//! platform; the home places them there, without the portal index, the platform made in
//! the placing's commit.

use clustine_blocks::data::same_block;
use clustine_data::blocks;
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EdgeId, EntityIds, PlayerId, RegionId, Travel, Way,
};

use super::super::tnt::tests::{FLOOR, add_player, at, idle};
use super::super::travel::END_PLATFORM;
use super::super::{Holdings, Region, RegionConfig};
use crate::api::{
    Durable, EdgeEvent, Inventory, PlayerChange, PlayerTransfer, TickInputs, TickOutput, Ticket,
};
use crate::state::RegionState;

const EDGE: EdgeId = EdgeId(1);
const END_HOME: RegionId = RegionId(9);

/// A region of `dimension` with a stone floor at `floor` over the chunks within `reach`
/// of `centre`, the blocks `blocks` names above it, the homes and a known edge.
fn world(
    dimension: Dimension,
    centre: ChunkPos,
    reach: i32,
    floor: i32,
    blocks: impl Fn(BlockPos) -> Option<clustine_data::BlockState>,
) -> Region {
    let rules = dimension.rules();
    let held: Vec<ChunkPos> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(centre.x + x, centre.z + z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(rules, Biome(0));
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, floor, z, blocks::STONE);
                    for y in floor + 1..=floor + 12 {
                        let pos = at(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                        if let Some(state) = blocks(pos) {
                            chunk.set(x, y, z, state);
                        }
                    }
                }
            }
            (*position, chunk)
        })
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held,
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let config = RegionConfig {
        world_spawn: clustine_world::BlockPos::new(0, -60, 0),
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, RegionId(0)),
            (Dimension::Nether, RegionId(1)),
            (Dimension::End, END_HOME),
        ],
        dimension,
        spawn: None,
        starting_inventory: Inventory::default(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 9,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, Vec::new());
    region.tick(&TickInputs::default());
    region.tick(&TickInputs {
        edges: vec![EdgeEvent::Started {
            edge: EDGE,
            start: 1,
        }],
        ..TickInputs::default()
    });
    region
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

/// The overworld with an End portal's block at (2, FLOOR + 1, 2) and a player standing
/// in it, who is let go into the End.
fn let_go_into_the_end() -> (PlayerId, PlayerTransfer, RegionId) {
    let portal = at(2, FLOOR + 1, 2);
    let mut region = world(Dimension::Overworld, ChunkPos::new(0, 0), 1, FLOOR, |pos| {
        (pos == portal).then_some(blocks::END_PORTAL)
    });
    let feet = [2.5, f64::from(FLOOR + 1), 2.5];
    let (id, _) = add_player(&mut region, 1, feet, false);
    let mut fired = None;
    for _ in 0..3 {
        let output = idle(&mut region);
        if let Some(found) = departed(&output, id) {
            fired = Some(found);
            break;
        }
    }
    let (transfer, to) = fired.expect("the End's portal fires at once");
    (id, transfer, to)
}

/// The brief's 2.4: the End's portal, its transition time none, lets a player go to the
/// End's home with the travel to the platform under the End's spawn point.
#[test]
fn a_player_in_the_end_s_portal_is_let_go_to_the_end_s_home_bound_for_the_platform() {
    let (_, transfer, to) = let_go_into_the_end();
    assert_eq!(to, END_HOME);
    assert_eq!(
        transfer.travel,
        Some(Travel {
            to: Dimension::End,
            target: END_PLATFORM,
            way: Way::EndPortal,
        })
    );
    assert_eq!(transfer.dimension, Dimension::End);
}

/// The brief's 2.4 and the plan's 19.5 item 15: the End's home places the traveller on
/// the platform it makes, without asking the portal index: obsidian 5 × 5 one under the
/// spawn point's block, air for three above, the player standing on it at
/// (100.5, 49, 0.5) facing west.
#[test]
fn the_end_s_home_makes_the_platform_and_places_the_player_on_it_without_the_index() {
    let (id, transfer, _) = let_go_into_the_end();
    // A pillar of stone where the platform is to be cleared, and End stone under it.
    let mut end = world(Dimension::End, ChunkPos::new(6, 0), 1, 40, |pos| {
        (pos.x == 101 && pos.z == 1 && (49..=51).contains(&pos.y)).then_some(blocks::STONE)
    });
    let output = end.tick(&TickInputs {
        player_changes: vec![PlayerChange::Arrive(EDGE, id, transfer)],
        ..TickInputs::default()
    });
    let placed_in = |output: &TickOutput| {
        output.durable.iter().find_map(|(_, _, entry)| match entry {
            Durable::Placed { player, pose, .. } if *player == id => Some(*pose),
            _ => None,
        })
    };
    let mut placed = placed_in(&output);
    let mut asked = !output.portal_queries.is_empty();
    for _ in 0..5 {
        if placed.is_some() {
            break;
        }
        let output = idle(&mut end);
        asked |= !output.portal_queries.is_empty();
        placed = placed_in(&output);
    }
    assert!(!asked, "no question to the portal index");
    let pose = placed.expect("placed within a few ticks");
    assert_eq!(
        [
            pose.position.x,
            pose.position.y,
            pose.position.z,
            f64::from(pose.yaw)
        ],
        [100.5, 49.0, 0.5, 90.0]
    );
    for dz in -2..=2 {
        for dx in -2..=2 {
            let floor = end.block(at(100 + dx, 48, dz)).expect("loaded");
            assert!(same_block(floor, blocks::OBSIDIAN), "{dx}, {dz}: {floor:?}");
            for y in 49..=51 {
                let above = end.block(at(100 + dx, y, dz)).expect("loaded");
                assert!(above.props().is_air(), "{dx}, {y}, {dz}: {above:?}");
            }
        }
    }
}

/// The brief's 2.4 (`EndPortalBlock.entityInside`): in the End, a player who has not
/// seen the credits is not marked inside the portal (the credits are theirs instead,
/// P10c's), and is not sent anywhere.
#[test]
fn in_the_end_a_player_who_has_not_seen_the_credits_is_not_sent_through_the_portal() {
    let portal = at(2, 41, 2);
    let mut end = world(Dimension::End, ChunkPos::new(0, 0), 1, 40, |pos| {
        (pos == portal).then_some(blocks::END_PORTAL)
    });
    let (id, _) = add_player(&mut end, 1, [2.5, 41.0, 2.5], false);
    for _ in 0..3 {
        let output = idle(&mut end);
        assert!(departed(&output, id).is_none());
        assert!(
            end.players[&id].portal.is_idle(),
            "{:?}",
            end.players[&id].portal
        );
    }
}
