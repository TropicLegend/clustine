//! Tests of the End's gateways, tick by tick (`docs/groundwork/p-plan.md`, section 19.8
//! with its review; the brief's 2.6): a gateway's data as the game saves it; a gateway
//! without an exit asked about once, its answer written and the exit's making filed; its
//! cooldown and beam; a player taken in only by a gateway that knows its exit, the
//! gateway cooling as it takes them, and sent on the next tick; where one comes out; and
//! the island and the return gateway made once.

use clustine_data::{blocks, entity_types, items};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EdgeId, EffectId, Entity, EntityData, EntityId,
    EntityIds, ItemStack, LevelChunk, PlayerId, RegionId, Travel, Traveller, Vec3, Way,
};

use super::super::tnt::tests::{add_player, at, idle};
use super::super::{Holdings, Region, RegionConfig};
use super::{GatewayData, gateway_data, read_gateway};
use crate::api::{
    Durable, EdgeEvent, GatewayExit, GatewayExitAnswer, Inventory, QueryId, RegionEvent,
    TickInputs, TickOutput, Ticket, WorldPlayer,
};
use crate::state::RegionState;

const EDGE: EdgeId = EdgeId(1);
const OVERWORLD_HOME: RegionId = RegionId(0);
const WORLD_SPAWN: BlockPos = BlockPos::new(0, -60, 0);

/// A region of `dimension` with a stone floor at `floor` over the chunks within `reach`
/// of `centre`, the blocks `blocks` names above it, the homes and a known edge; the
/// overworld's home where `spawn` is given (its home chunk is the spawn's).
fn world(dimension: Dimension, centre: ChunkPos, reach: i32) -> Region {
    let spawn: Option<Vec3> = None;
    let floor = 40;
    let blocks = |_: BlockPos| -> Option<clustine_data::BlockState> { None };
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
        world_spawn: WORLD_SPAWN,
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, OVERWORLD_HOME),
            (Dimension::Nether, RegionId(1)),
            (Dimension::End, RegionId(2)),
        ],
        dimension,
        spawn,
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

/// A gateway as the fight places one, without an exit, at (40, 75, 0) in the End's home.
const GATEWAY: BlockPos = BlockPos::new(40, 75, 0);

/// Where the tests' gateway leads.
const EXIT: BlockPos = BlockPos::new(20, 60, 20);

fn end_with_a_gateway() -> Region {
    let mut end = world(Dimension::End, ChunkPos::new(0, 0), 3);
    let mut output = TickOutput::default();
    end.set_block(GATEWAY, blocks::END_GATEWAY, &mut output);
    end
}

/// The gateway's exit written as an answer would write it.
fn with_its_exit(end: &mut Region, exact: bool) {
    let data = GatewayData {
        age: 0,
        exit: Some(EXIT),
        exact,
    };
    let mut output = TickOutput::default();
    assert!(end.set_block_entity_data_as(GATEWAY, gateway_data(data), false, &mut output));
}

#[test]
fn a_gateway_s_data_reads_back_as_it_was_written() {
    for data in [
        GatewayData::default(),
        GatewayData {
            age: 2_401,
            exit: Some(EXIT),
            exact: true,
        },
        GatewayData {
            age: 7,
            exit: Some(BlockPos::new(-1_000, 3, 77)),
            exact: false,
        },
    ] {
        assert_eq!(read_gateway(&gateway_data(data)), data);
    }
}

/// The review's first point: the region asks about a gateway without an exit once in
/// its run; the answer is written into it and the exit's making filed for the exit's
/// chunk, with the island it wants.
#[test]
fn a_gateway_without_an_exit_is_asked_about_once_and_its_answer_written() {
    let mut end = end_with_a_gateway();
    let mut asked = Vec::new();
    for _ in 0..3 {
        asked.extend(idle(&mut end).gateway_queries);
    }
    let [question] = asked[..] else {
        panic!("one question: {asked:?}");
    };
    assert_eq!(question.from, GATEWAY);
    let island = BlockPos::new(20, 75, 20);
    let output = end.tick(&TickInputs {
        gateway_exits: vec![GatewayExitAnswer {
            id: question.id,
            from: GATEWAY,
            exit: Some(GatewayExit {
                exit: EXIT,
                island: Some(island),
            }),
        }],
        ..TickInputs::default()
    });
    assert_eq!(
        end.gateway_at(GATEWAY).and_then(|data| data.exit),
        Some(EXIT)
    );
    let [traveller] = &output.travellers[..] else {
        panic!("one traveller: {:?}", output.travellers);
    };
    assert!(traveller.entities.is_empty());
    assert_eq!(
        traveller.travel,
        Travel {
            to: Dimension::End,
            target: EXIT,
            way: Way::Gateway {
                from: GATEWAY,
                exact: false,
                island: Some(island),
            },
        }
    );
    // A second answer finds the exit known.
    let output = end.tick(&TickInputs {
        gateway_exits: vec![GatewayExitAnswer {
            id: QueryId { tick: 1, index: 0 },
            from: GATEWAY,
            exit: Some(GatewayExit {
                exit: BlockPos::new(1, 2, 3),
                island: None,
            }),
        }],
        ..TickInputs::default()
    });
    assert!(output.travellers.is_empty());
    assert_eq!(
        end.gateway_at(GATEWAY).and_then(|data| data.exit),
        Some(EXIT)
    );
}

/// `portalTick`: every 2400 ticks of its age the gateway flashes (block event 1) and
/// cools down 40 ticks.
#[test]
fn a_gateway_flashes_every_two_thousand_four_hundred_ticks_of_its_age() {
    let mut end = end_with_a_gateway();
    let mut flashes = Vec::new();
    for tick in 1..=2_500 {
        let output = idle(&mut end);
        if output.events.iter().any(|event| {
            matches!(event, RegionEvent::BlockEvent { position, kind: 1, .. } if *position == GATEWAY)
        }) {
            flashes.push(tick);
        }
    }
    assert_eq!(flashes.len(), 1, "{flashes:?}");
    let clock = end.gateway_clock(GATEWAY).expect("the gateway");
    assert_eq!(clock.age % 2_400, clock.age - 2_400);
}

/// The review's first point and `entityInside`: a gateway that knows no exit takes
/// nobody in; one that does takes a player in, cools down as it does (its block event),
/// and sends them on the next tick through the gateway's way to the exit.
#[test]
fn a_player_is_taken_in_only_by_a_gateway_that_knows_its_exit_and_goes_the_next_tick() {
    let feet = [
        f64::from(GATEWAY.x) + 0.5,
        f64::from(GATEWAY.y),
        f64::from(GATEWAY.z) + 0.5,
    ];
    let mut end = end_with_a_gateway();
    let (id, _) = add_player(&mut end, 1, feet, false);
    for _ in 0..3 {
        idle(&mut end);
        assert!(end.player(id).is_some(), "no exit: not taken");
    }
    with_its_exit(&mut end, true);
    let mut cooled = false;
    let mut gone = None;
    for _ in 0..4 {
        let output = idle(&mut end);
        cooled |= output.events.iter().any(|event| {
            matches!(event, RegionEvent::BlockEvent { position, kind: 1, .. } if *position == GATEWAY)
        });
        if end.player(id).is_none() {
            gone = Some(output);
            break;
        }
    }
    assert!(cooled, "the gateway cools as it takes them");
    gone.expect("sent on");
    // The End's home of one region places them itself, with no departure.
    let waiting = end.waiting_state(id).expect("waits to be placed here");
    assert_eq!(
        waiting.transfer.travel.map(|travel| travel.way),
        Some(Way::Gateway {
            from: GATEWAY,
            exact: true,
            island: None,
        })
    );
}

/// `getPortalPosition`: where the gateway is exact, the player comes out at the exit's
/// bottom centre; placed by the region, as a `Placed` with nothing a respawn owes.
#[test]
fn a_player_through_an_exact_gateway_comes_out_at_its_exit() {
    let feet = [
        f64::from(GATEWAY.x) + 0.5,
        f64::from(GATEWAY.y),
        f64::from(GATEWAY.z) + 0.5,
    ];
    let mut end = end_with_a_gateway();
    with_its_exit(&mut end, true);
    let (id, _) = add_player(&mut end, 1, feet, false);
    let mut placed = None;
    for _ in 0..10 {
        let output = idle(&mut end);
        placed = output.durable.iter().find_map(|(_, _, entry)| match entry {
            Durable::Placed {
                player,
                pose,
                respawn,
                gateway,
                ..
            } if *player == id => Some((*pose, *respawn, *gateway)),
            _ => None,
        });
        if placed.is_some() {
            break;
        }
    }
    let (pose, respawn, gateway) = placed.expect("placed");
    assert_eq!(respawn, None);
    assert!(
        gateway,
        "the placing says a gateway made it, for the client's look"
    );
    assert_eq!(
        [pose.position.x, pose.position.y, pose.position.z],
        [20.5, 60.0, 20.5]
    );
}

/// The review's second point: the exit's holder makes the island where one is wanted
/// and the return gateway, leading back, not exact; a second making finds the gateway
/// there and makes nothing.
#[test]
fn the_exit_s_holder_makes_the_island_and_the_return_gateway_once() {
    let mut end = world(Dimension::End, ChunkPos::new(0, 0), 3);
    let island = BlockPos::new(20, 75, 20);
    let travel = Travel {
        to: Dimension::End,
        target: BlockPos::new(20, 86, 20),
        way: Way::Gateway {
            from: GATEWAY,
            exact: false,
            island: Some(island),
        },
    };
    let mut output = TickOutput::default();
    end.make_gateway_exit(&travel, &mut output);
    assert_eq!(
        end.block(island),
        Some(blocks::END_STONE),
        "the island's middle"
    );
    assert_eq!(end.block(travel.target), Some(blocks::END_GATEWAY));
    assert_eq!(
        end.block(travel.target.offset(0, 1, 0)),
        Some(blocks::BEDROCK)
    );
    let data = end.gateway_at(travel.target).expect("the return gateway");
    assert_eq!((data.exit, data.exact), (Some(GATEWAY), false));
    let changes = output.events.len();
    let mut again = TickOutput::default();
    end.make_gateway_exit(&travel, &mut again);
    assert!(again.events.is_empty(), "{changes} then {:?}", again.events);
}

/// 19.8, "Where the traveller comes out": a player through a gateway is placed by the
/// holder of the exit's chunk, which reads the tallest block about it. The End's home
/// that took in a player through its own gateway, knowing no holder of the exit's chunk
/// then, and learns that another region holds it, lets the player go to that region with
/// their travel, rather than placing them over chunks it cannot read (where they would
/// stand two above the exit, on the return gateway's cage).
#[test]
fn a_gateway_s_player_waiting_for_an_exit_another_region_holds_goes_on_to_that_region() {
    let far = BlockPos::new(200, 60, 0);
    let holder = RegionId(7);
    let feet = [
        f64::from(GATEWAY.x) + 0.5,
        f64::from(GATEWAY.y),
        f64::from(GATEWAY.z) + 0.5,
    ];
    let mut end = end_with_a_gateway();
    let data = GatewayData {
        age: 0,
        exit: Some(far),
        exact: false,
    };
    let mut output = TickOutput::default();
    assert!(end.set_block_entity_data_as(GATEWAY, gateway_data(data), false, &mut output));
    let (id, _) = add_player(&mut end, 1, feet, false);
    let mut fired = false;
    for _ in 0..4 {
        idle(&mut end);
        if end.waiting_state(id).is_some() {
            fired = true;
            break;
        }
    }
    assert!(fired, "the player went into the gateway and waits here");
    // Nothing is known of the exit's chunks yet: the player waits for them.
    let output = idle(&mut end);
    assert_eq!(placed_or_departed(&output, id), None);
    assert!(end.waiting_state(id).is_some());
    let centre = far.chunk();
    let foreign: Vec<(ChunkPos, RegionId)> = (-1..=1)
        .flat_map(|dx| (-1..=1).map(move |dz| ChunkPos::new(centre.x + dx, centre.z + dz)))
        .map(|chunk| (chunk, holder))
        .collect();
    let output = end.tick(&TickInputs {
        foreign,
        ..TickInputs::default()
    });
    let Some(Outcome::Departed(transfer, to)) = placed_or_departed(&output, id) else {
        panic!(
            "let go to the exit's holder: {:?}",
            placed_or_departed(&output, id)
        );
    };
    assert_eq!(to, holder);
    assert_eq!(
        transfer.travel,
        Some(Travel {
            to: Dimension::End,
            target: far,
            way: Way::Gateway {
                from: GATEWAY,
                exact: false,
                island: None,
            },
        }),
        "with their travel, for the holder to place them"
    );
    assert_eq!(transfer.forwards, 1);
    assert!(end.waiting_state(id).is_none());
}

/// The first gateway of the End as the fight places it, at (96, 75, 0), which a return
/// gateway on the outer islands leads back to.
const FIRST_GATEWAY: BlockPos = BlockPos::new(96, 75, 0);

/// An End chunk as the store hands one in: end stone up to y 60.
fn end_island_chunk() -> Chunk {
    let mut chunk = Chunk::empty(Dimension::End.rules(), Biome(0));
    for z in 0..16 {
        for x in 0..16 {
            for y in 50..=60 {
                chunk.set(x, y, z, blocks::END_STONE);
            }
        }
    }
    chunk
}

/// An ender pearl as a gateway sends one on: owned by the player whose entity is
/// `owner`, its motion cleared.
fn a_pearl(owner: EntityId) -> Entity {
    Entity {
        portal: Default::default(),
        key: EntityId(9_000),
        kind: u16::try_from(entity_types::ENDER_PEARL).expect("an entity type"),
        uuid: uuid::Uuid::from_u128(9_000),
        position: [1100.5, 70.0, 0.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 40,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Thrown {
            stack: ItemStack::new(items::ENDER_PEARL, 1),
            owner: Some(owner),
            left_owner: true,
            has_been_shot: true,
        },
    }
}

/// `TeleportTransition.PLACE_PORTAL_TICKET` (d-javap EndGatewayBlock.txt 65 and 103,
/// `Entity.placePortalTicket`): what comes out of a gateway places the game's portal
/// ticket about where it comes out, which keeps the chunks there loaded and their
/// entities ticking for 300 ticks with nobody near. A pearl that comes out of the
/// return gateway's exit, by the first gateway where no player is, so falls, lands and
/// carries its owner, a player far away on the outer islands, back beside it.
#[test]
fn a_pearl_out_of_a_gateway_where_nobody_is_lands_and_carries_its_far_owner() {
    let mut end = world(Dimension::End, ChunkPos::new(0, 0), 1);
    let owner = EntityId(500);
    let player = PlayerId(uuid::Uuid::from_u128(77));
    end.tick(&TickInputs {
        world_players: Some(vec![WorldPlayer {
            player,
            entity: owner,
            dimension: Dimension::End,
            position: [1100.5, 61.0, 0.5],
            game_mode: 1,
            sleeping: None,
        }]),
        ..TickInputs::default()
    });
    let return_gateway = BlockPos::new(1100, 70, 0);
    let traveller = Traveller {
        travel: Travel {
            to: Dimension::End,
            target: FIRST_GATEWAY,
            way: Way::Gateway {
                from: return_gateway,
                exact: false,
                island: None,
            },
        },
        chunk: LevelChunk::new(Dimension::End, FIRST_GATEWAY.chunk()),
        entities: vec![a_pearl(owner)],
    };
    let mut inputs = TickInputs {
        travellers: vec![(
            EffectId {
                sequence: 1,
                index: 0,
            },
            traveller,
        )],
        ..TickInputs::default()
    };
    // The store: what the region claims nobody holds, and what it asks for is end stone.
    let mut landed = None;
    for _ in 0..300 {
        let output = end.tick(&inputs);
        landed = output.effects.iter().find_map(|effect| match &effect.what {
            clustine_world::Effected::OnPlayer {
                player: to,
                entity,
                by: clustine_world::EntityUse::PearlLanded { at, .. },
            } if *to == player && *entity == owner => Some(at.map(|value| value.0)),
            _ => None,
        });
        if landed.is_some() {
            break;
        }
        inputs = TickInputs {
            granted: output.claims.clone(),
            chunks_loaded: (output.chunk_requests.iter())
                .map(|position| (*position, end_island_chunk()))
                .collect(),
            ..TickInputs::default()
        };
    }
    let at = landed.expect("the pearl lands and its owner is carried to it");
    // On the island's top by the first gateway, where the pearl was as its last tick
    // began.
    assert!(
        (at[0] - 96.5).abs() < 6.0 && (at[2] - 0.5).abs() < 6.0,
        "{at:?}"
    );
    assert!(at[1] >= 61.0 && at[1] < 62.0, "{at:?}");
}

/// What became of a waiting player in a tick.
#[derive(Debug, PartialEq)]
enum Outcome {
    Placed([f64; 3]),
    Departed(Box<crate::api::PlayerTransfer>, RegionId),
}

fn placed_or_departed(output: &TickOutput, id: PlayerId) -> Option<Outcome> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Placed { player, pose, .. } if *player == id => Some(Outcome::Placed([
            pose.position.x,
            pose.position.y,
            pose.position.z,
        ])),
        Durable::Departed {
            player,
            transfer,
            to,
        } if *player == id => Some(Outcome::Departed(Box::new(transfer.clone()), *to)),
        _ => None,
    })
}

#[allow(unused)]
fn _unused(_: PlayerId, _: Vec3) -> BlockPos {
    at(0, 0, 0)
}
