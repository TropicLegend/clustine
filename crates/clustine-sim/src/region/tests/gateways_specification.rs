//! The End's gateways in a region, tick by tick, from their specification and by
//! someone who did not write them: `docs/groundwork/p-plan.md`, section 19.8 and its
//! review (what was built), the brief's section 2.6, and the game's bytecode (`d-javap`
//! TheEndGatewayBlockEntity.txt, EndGatewayBlock.txt, EndGatewayFeature.txt).
//!
//! The tests run in an End of their own: end stone at y = 40 over the chunks they hold,
//! a gateway standing on it at [`GATEWAY`], in chunk (2, 0), and the End's home holding
//! the home chunk (0, 0), so that a gateway's traveller whose exit is in these chunks
//! is placed here (an End of one region).

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_features::features::end::gateway_exit::island_writes;
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EdgeId, Effect, EffectId, Effected, EntityChange,
    EntityData, EntityId, EntityIds, InChunk, ItemStack, LevelChunk, PlayerId, RegionId, Travel,
    Traveller, Way,
};

use crate::api::{
    Durable, EdgeEvent, GatewayExit, GatewayExitAnswer, GatewayExitQuery, Inventory, QueryId,
    RegionEvent, TickInputs, TickOutput, Ticket,
};
use crate::region::gateways::{GatewayClock, GatewayData, gateway_data, read_gateway};
use crate::region::tnt::tests::{add, add_player, idle, record};
use crate::region::{Holdings, Region, RegionConfig};
use crate::state::RegionState;

const EDGE: EdgeId = EdgeId(1);

/// The floor of the tests' End.
const FLOOR: i32 = 40;

/// A gateway standing on the floor, in chunk (2, 0).
const GATEWAY: BlockPos = BlockPos::new(40, FLOOR + 1, 8);

/// Where the tests' gateway leads, over the floor, in chunk (-2, -2).
const EXIT: BlockPos = BlockPos::new(-20, 60, -20);

/// The region another region is, where a test needs one.
const OTHER: RegionId = RegionId(7);

/// The region's configuration in `dimension`, the End's home being region 2.
fn config(dimension: Dimension) -> RegionConfig {
    let rules = dimension.rules();
    RegionConfig {
        world_spawn: BlockPos::new(0, -60, 0),
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, RegionId(0)),
            (Dimension::Nether, RegionId(1)),
            (Dimension::End, RegionId(2)),
        ],
        dimension,
        spawn: None,
        starting_inventory: Inventory::default(),
        default_mode: clustine_world::GameMode::Creative,
        force_mode: false,
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 19,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    }
}

/// The chunks within `reach` of (0, 0).
fn about_the_middle(reach: i32) -> Vec<ChunkPos> {
    (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A chunk of `dimension` with end stone over it at `floor`, where there is one.
fn ground(dimension: Dimension, floor: Option<i32>) -> Chunk {
    let mut chunk = Chunk::empty(dimension.rules(), Biome(0));
    if let Some(floor) = floor {
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, floor, z, blocks::END_STONE);
            }
        }
    }
    chunk
}

/// What a test's region is made of: the chunks it holds and has loaded, copies of
/// others', and chunks it knows another region holds.
struct Land {
    dimension: Dimension,
    held: Vec<(ChunkPos, Chunk)>,
    copies: Vec<(ChunkPos, Chunk)>,
    others: Vec<(ChunkPos, RegionId)>,
}

impl Land {
    /// The chunks within 3 of the middle, held and loaded, with the floor: the gateway's chunk and the exit's have all their neighbours, and tick.
    fn end() -> Self {
        Self::of(Dimension::End, about_the_middle(3), Some(FLOOR))
    }

    fn of(dimension: Dimension, chunks: Vec<ChunkPos>, floor: Option<i32>) -> Self {
        Self {
            dimension,
            held: chunks
                .into_iter()
                .map(|at| (at, ground(dimension, floor)))
                .collect(),
            copies: Vec::new(),
            others: Vec::new(),
        }
    }

    fn holdings(&self) -> Holdings {
        Holdings {
            held: self.held.iter().map(|(at, _)| *at).collect(),
            pinned: Vec::new(),
            lent: Vec::new(),
            lent_away: self.others.clone(),
        }
    }

    fn tickets(&self) -> Vec<(ChunkPos, Ticket)> {
        self.held
            .iter()
            .map(|(at, _)| (*at, Ticket::Viewer))
            .collect()
    }

    /// The region, its edge started.
    fn region(self) -> Region {
        let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
        let mut region = Region::restore_loaded(
            config(self.dimension),
            state,
            self.holdings(),
            &self.tickets(),
            self.held,
            self.copies,
        );
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
}

/// `region` as it is restored from its state and its chunks as saved, holding
/// `held` of them: after a restart, or as one part of a split.
fn restored(region: &Region, dimension: Dimension, held: &[ChunkPos]) -> Region {
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .filter_map(|at| region.saved_chunk(*at).map(|chunk| (*at, chunk)))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held.iter().map(|at| (*at, Ticket::Viewer)).collect();
    let holdings = Holdings {
        held: held.to_vec(),
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    Region::restore_loaded(
        config(dimension),
        region.state(),
        holdings,
        &tickets,
        chunks,
        Vec::new(),
    )
}

/// A gateway put at `pos` as a placing puts one: age 0, no exit.
fn place_gateway(region: &mut Region, pos: BlockPos) {
    let mut output = TickOutput::default();
    region.set_block(pos, blocks::END_GATEWAY, &mut output);
    assert!(region.gateway_at(pos).is_some(), "a gateway at {pos:?}");
}

/// The gateway at `pos` given `data`, as an answer or a load would give it.
fn write_gateway(region: &mut Region, pos: BlockPos, data: GatewayData) {
    let mut output = TickOutput::default();
    assert!(region.set_block_entity_data_as(pos, gateway_data(data), false, &mut output));
}

/// The tests' End with its gateway, which leads to [`EXIT`], exact or not.
fn end_with_an_open_gateway(exact: bool) -> Region {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    write_gateway(
        &mut end,
        GATEWAY,
        GatewayData {
            age: 0,
            exit: Some(EXIT),
            exact,
        },
    );
    end
}

fn ask(question: GatewayExitQuery, exit: Option<GatewayExit>) -> TickInputs {
    TickInputs {
        gateway_exits: vec![GatewayExitAnswer {
            id: question.id,
            from: question.from,
            exit,
        }],
        ..TickInputs::default()
    }
}

/// The questions the region asks in `ticks` idle ticks.
fn questions_in(region: &mut Region, ticks: usize) -> Vec<GatewayExitQuery> {
    (0..ticks)
        .flat_map(|_| idle(region).gateway_queries)
        .collect()
}

/// The gateway's block events in `output`: (kind, data).
fn flashes(output: &TickOutput, pos: BlockPos) -> Vec<(i32, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEvent {
                position,
                block,
                kind,
                data,
            } if *position == pos => {
                assert_eq!(
                    Some(usize::from(*block)),
                    blocks::END_GATEWAY.block_id(),
                    "the gateway's block"
                );
                Some((*kind, *data))
            }
            _ => None,
        })
        .collect()
}

/// The gateway's data written in `output`, each with whether clients were told.
fn written(output: &TickOutput, pos: BlockPos) -> Vec<(GatewayData, bool)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEntityChanged {
                position,
                data,
                seen,
            } if *position == pos => Some((read_gateway(data), *seen)),
            _ => None,
        })
        .collect()
}

/// Feet standing in the gateway at `pos`.
fn feet_in(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// The player's placing in `output`, if it is there: its pose and whether it was a
/// respawn.
fn placing(output: &TickOutput, id: PlayerId) -> Option<(crate::api::Pose, bool)> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Placed {
            player,
            pose,
            respawn,
            ..
        } if *player == id => Some((*pose, respawn.is_some())),
        _ => None,
    })
}

fn departed(output: &TickOutput, id: PlayerId) -> Option<RegionId> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Departed { player, to, .. } if *player == id => Some(*to),
        _ => None,
    })
}

/// Ticks until the player is placed, at most 20 ticks: the outputs, and the placing.
fn until_placed(region: &mut Region, id: PlayerId) -> (Vec<TickOutput>, crate::api::Pose) {
    let mut outputs = Vec::new();
    for _ in 0..20 {
        let output = idle(region);
        let placed = placing(&output, id);
        outputs.push(output);
        if let Some((pose, respawn)) = placed {
            assert!(!respawn, "a gateway's placing is no respawn");
            return (outputs, pose);
        }
    }
    panic!("not placed in 20 ticks");
}

/// An item at `position` that no player picks up.
fn item(region: &mut Region, position: [f64; 3]) -> EntityId {
    let data = EntityData::Item {
        stack: ItemStack::new(items::DIRT, 1),
        pick_up_delay: 32_767,
        thrower: None,
        health: 5,
        age_offset: 0,
    };
    let entity = record(region, entity_types::ITEM, position, data);
    add(region, entity)
}

// The gateway's tick (`portalTick`, lines 116-163; `triggerCooldown`, 230-249).

/// `portalTick`: the age goes on by one each tick; where the gateway is not cooling
/// down and its age reaches a multiple of 2400, `triggerCooldown`: a cooldown of 40 and
/// the block event (1, 0) that has the clients draw the beam; the cooldown then counts
/// down a tick at a time.
#[test]
fn a_gateway_ages_a_tick_at_a_time_and_flashes_when_its_age_reaches_2400() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    end.set_gateway_clock(
        GATEWAY,
        GatewayClock {
            age: 2_395,
            cooldown: 0,
        },
    );
    let mut flashed = Vec::new();
    for tick in 1..=50 {
        let output = idle(&mut end);
        let clock = end.gateway_clock(GATEWAY).expect("the gateway");
        assert_eq!(clock.age, 2_395 + tick, "a tick of age a tick");
        for flash in flashes(&output, GATEWAY) {
            flashed.push((clock.age, flash));
        }
        let cooldown = if clock.age >= 2_400 {
            (40 - (clock.age - 2_400)).max(0)
        } else {
            0
        };
        assert_eq!(i64::from(clock.cooldown), cooldown, "at age {}", clock.age);
    }
    assert_eq!(flashed, [(2_400, (1, 0))]);
}

/// `portalTick`, offsets 22-57: a gateway that is cooling down when its age reaches a
/// multiple of 2400 counts its cooldown down and does not flash.
#[test]
fn a_gateway_cooling_down_when_its_age_reaches_2400_does_not_flash() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    end.set_gateway_clock(
        GATEWAY,
        GatewayClock {
            age: 2_390,
            cooldown: 20,
        },
    );
    for _ in 0..60 {
        let output = idle(&mut end);
        assert_eq!(
            flashes(&output, GATEWAY),
            [],
            "{:?}",
            end.gateway_clock(GATEWAY)
        );
    }
    let clock = end.gateway_clock(GATEWAY).expect("the gateway");
    assert_eq!((clock.age, clock.cooldown), (2_450, 0));
}

/// `portalTick`, offsets 60-81, `triggerCooldown` and `setExitPosition`: the gateway's
/// data is written, with its age as it is then, only where the game calls `setChanged`:
/// its spawning ends (age 200), it begins or ends cooling down, its exit is set. A
/// `setChanged` tells the clients nothing.
#[test]
fn a_gateway_s_age_is_written_only_where_the_game_calls_set_changed() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    let mut writes = Vec::new();
    for _ in 0..210 {
        let output = idle(&mut end);
        writes.extend(written(&output, GATEWAY));
    }
    assert_eq!(
        writes,
        [(
            GatewayData {
                age: 200,
                exit: None,
                exact: false
            },
            false
        )],
        "its spawning ends at 200"
    );
    end.set_gateway_clock(
        GATEWAY,
        GatewayClock {
            age: 2_398,
            cooldown: 0,
        },
    );
    let mut ages = Vec::new();
    for _ in 0..50 {
        let output = idle(&mut end);
        for (data, seen) in written(&output, GATEWAY) {
            assert!(!seen, "setChanged tells no client");
            ages.push(data.age);
        }
    }
    ages.dedup();
    assert_eq!(ages, [2_400, 2_440], "it begins and ends cooling down");
}

/// The review's fourth point: the age is committed "with the chunk when it is let go";
/// only a crash loses the ticks since the last `setChanged`.
#[test]
#[ignore = "finding: the review (19.8, item 4) has a gateway's age committed with its chunk when the chunk is let go (the game saves the age it has); the region writes the age only at setChanged, so a chunk unloaded at age 300 keeps the 200 written when its spawning ended"]
fn a_gateway_let_go_with_its_chunk_is_committed_with_the_age_it_has() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    let mut last = None;
    for _ in 0..300 {
        let output = idle(&mut end);
        last = written(&output, GATEWAY).last().copied().or(last);
    }
    let age = end.gateway_clock(GATEWAY).expect("the gateway").age;
    let output = end.tick(&TickInputs {
        tickets_removed: vec![(GATEWAY.chunk(), Ticket::Viewer)],
        ..TickInputs::default()
    });
    last = written(&output, GATEWAY).last().copied().or(last);
    for _ in 0..5 {
        if end.chunk(GATEWAY.chunk()).is_none() {
            break;
        }
        let output = idle(&mut end);
        last = written(&output, GATEWAY).last().copied().or(last);
    }
    assert!(end.chunk(GATEWAY.chunk()).is_none(), "the chunk is let go");
    let (data, _) = last.expect("the age was written");
    assert!(data.age >= age, "committed {} of {age}", data.age);
}

// The question asked ahead (the review's first point).

/// The review's first point: a region of the End asks about each gateway it holds that
/// has no exit, once in its run, and not about one that has an exit.
#[test]
fn a_region_of_the_end_asks_once_in_its_run_about_each_gateway_without_an_exit() {
    let mut end = Land::end().region();
    let known = BlockPos::new(-8, FLOOR + 1, 20);
    place_gateway(&mut end, known);
    write_gateway(
        &mut end,
        known,
        GatewayData {
            age: 0,
            exit: Some(EXIT),
            exact: false,
        },
    );
    place_gateway(&mut end, GATEWAY);
    let asked = questions_in(&mut end, 10);
    let [question] = asked[..] else {
        panic!("one question: {asked:?}");
    };
    assert_eq!(question.from, GATEWAY);
    // A gateway a placing makes later is asked about as soon as there is one.
    let later = BlockPos::new(20, FLOOR + 1, -20);
    place_gateway(&mut end, later);
    let asked = questions_in(&mut end, 10);
    assert_eq!(
        asked
            .iter()
            .map(|question| question.from)
            .collect::<Vec<_>>(),
        [later]
    );
}

/// The review's first point: the question is not kept, so a region restored from its
/// state and chunks asks again about a gateway that has no exit.
#[test]
fn a_restored_region_asks_again_about_a_gateway_without_an_exit() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    assert_eq!(questions_in(&mut end, 3).len(), 1);
    let mut again = restored(&end, Dimension::End, &about_the_middle(3));
    let asked = questions_in(&mut again, 5);
    assert_eq!(
        asked
            .iter()
            .map(|question| question.from)
            .collect::<Vec<_>>(),
        [GATEWAY]
    );
}

/// The review's first point: "one in a chunk it loads"; a gateway that comes with a
/// chunk loaded later is asked about.
#[test]
fn a_gateway_in_a_chunk_loaded_later_is_asked_about() {
    let later = ChunkPos::new(4, 0);
    let mut land = Land::end();
    land.held.push((later, ground(Dimension::End, Some(FLOOR))));
    // Held but without a ticket, so not loaded.
    let tickets: Vec<(ChunkPos, Ticket)> = about_the_middle(3)
        .into_iter()
        .map(|at| (at, Ticket::Viewer))
        .collect();
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let holdings = land.holdings();
    let mut end = Region::restore_loaded(
        config(Dimension::End),
        state,
        holdings,
        &tickets,
        land.held,
        Vec::new(),
    );
    assert!(end.chunk(later).is_none());
    assert_eq!(questions_in(&mut end, 3), []);
    let gateway = BlockPos::new(66, FLOOR + 1, 3);
    let mut chunk = ground(Dimension::End, Some(FLOOR));
    let (x, z) = gateway.in_chunk();
    chunk.set(x, gateway.y, z, blocks::END_GATEWAY);
    chunk
        .set_block_entity_data(
            InChunk::new(x, gateway.y, z),
            gateway_data(GatewayData::default()),
        )
        .expect("the gateway's data");
    let mut output = end.tick(&TickInputs {
        tickets_added: vec![(later, Ticket::Viewer)],
        ..TickInputs::default()
    });
    let mut asked = output.gateway_queries.clone();
    for _ in 0..10 {
        let delivered = if output.chunk_requests.contains(&later) || end.chunk(later).is_none() {
            vec![(later, chunk.clone())]
        } else {
            Vec::new()
        };
        output = end.tick(&TickInputs {
            chunks_loaded: delivered,
            ..TickInputs::default()
        });
        asked.extend(output.gateway_queries.iter().copied());
        if end.chunk(later).is_some() && !asked.is_empty() {
            break;
        }
    }
    assert!(end.chunk(later).is_some(), "the chunk is loaded");
    assert_eq!(
        asked
            .iter()
            .map(|question| question.from)
            .collect::<Vec<_>>(),
        [gateway]
    );
}

/// The review's ninth point and `getPortalPosition` (offsets 0-17): outside the End a
/// gateway is asked nothing about and takes nobody in.
#[test]
fn outside_the_end_a_gateway_is_asked_nothing_and_takes_nobody_in() {
    let mut overworld = Land::of(Dimension::Overworld, about_the_middle(3), Some(FLOOR)).region();
    place_gateway(&mut overworld, GATEWAY);
    let (id, _) = add_player(&mut overworld, 1, feet_in(GATEWAY), false);
    for _ in 0..10 {
        let output = idle(&mut overworld);
        assert_eq!(output.gateway_queries, []);
        assert_eq!(flashes(&output, GATEWAY), []);
    }
    assert!(overworld.player(id).is_some(), "not taken");
}

// The answer.

/// The review's third point and `setExitPosition`: the answer is written into the
/// gateway, its `ExactTeleport` kept, unseen; in the same output the traveller with no
/// entities is filed for the exit's chunk, carrying the island to make.
#[test]
fn an_answer_is_written_with_the_gateway_s_exactness_and_the_exit_s_making_filed_with_it() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    write_gateway(
        &mut end,
        GATEWAY,
        GatewayData {
            age: 0,
            exit: None,
            exact: true,
        },
    );
    let [question] = questions_in(&mut end, 3)[..] else {
        panic!("one question");
    };
    let island = BlockPos::new(-20, 75, -21);
    let found = GatewayExit {
        exit: EXIT,
        island: Some(island),
    };
    let output = end.tick(&ask(question, Some(found)));
    let data = end.gateway_at(GATEWAY).expect("the gateway");
    assert_eq!((data.exit, data.exact), (Some(EXIT), true));
    let writes = written(&output, GATEWAY);
    assert!(
        writes
            .iter()
            .any(|(data, seen)| data.exit == Some(EXIT) && data.exact && !seen),
        "{writes:?}"
    );
    assert_eq!(
        output.travellers,
        [Traveller {
            travel: Travel {
                to: Dimension::End,
                target: EXIT,
                way: Way::Gateway {
                    from: GATEWAY,
                    exact: true,
                    island: Some(island),
                },
            },
            chunk: LevelChunk::new(Dimension::End, EXIT.chunk()),
            entities: Vec::new(),
        }]
    );
    assert!(end.gateway_open(GATEWAY), "it takes travellers in now");
}

/// The review's third point: the region writes the exit only where it holds the
/// gateway still and the gateway has none: not into another region's gateway it reads,
/// nor one that has an exit, nor a block that is no gateway any more.
#[test]
fn an_answer_is_written_only_into_a_gateway_the_region_holds_that_has_no_exit() {
    let theirs = ChunkPos::new(4, 0);
    let theirs_gateway = BlockPos::new(68, FLOOR + 1, 4);
    let mut copy = ground(Dimension::End, Some(FLOOR));
    let (x, z) = theirs_gateway.in_chunk();
    copy.set(x, theirs_gateway.y, z, blocks::END_GATEWAY);
    let mut land = Land::end();
    land.copies.push((theirs, copy));
    let mut end = land.region();
    assert!(end.gateway_at(theirs_gateway).is_some(), "read in the copy");
    let known = BlockPos::new(-8, FLOOR + 1, 20);
    place_gateway(&mut end, known);
    write_gateway(
        &mut end,
        known,
        GatewayData {
            age: 0,
            exit: Some(EXIT),
            exact: false,
        },
    );
    place_gateway(&mut end, GATEWAY);
    questions_in(&mut end, 3);
    let mut output = TickOutput::default();
    end.set_block(GATEWAY, blocks::AIR, &mut output);
    let elsewhere = GatewayExit {
        exit: BlockPos::new(500, 70, 500),
        island: None,
    };
    for from in [theirs_gateway, known, GATEWAY] {
        let question = GatewayExitQuery {
            id: QueryId { tick: 1, index: 0 },
            from,
        };
        let output = end.tick(&ask(question, Some(elsewhere)));
        assert_eq!(output.travellers, [], "from {from:?}");
        assert_eq!(written(&output, from), [], "from {from:?}");
    }
    assert_eq!(end.gateway_at(known).and_then(|data| data.exit), Some(EXIT));
    assert_eq!(
        end.gateway_at(theirs_gateway).and_then(|data| data.exit),
        None
    );
}

/// The review's third point: an answer of none leaves the gateway without an exit,
/// taking nobody in, not asked about again in the run, and asked about again after a
/// restore.
#[test]
fn an_answer_of_none_leaves_the_gateway_closed_until_a_restore_asks_again() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    let [question] = questions_in(&mut end, 3)[..] else {
        panic!("one question");
    };
    let output = end.tick(&ask(question, None));
    assert_eq!(output.travellers, []);
    assert_eq!(end.gateway_at(GATEWAY).and_then(|data| data.exit), None);
    let (id, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    for _ in 0..10 {
        let output = idle(&mut end);
        assert_eq!(output.gateway_queries, []);
        assert_eq!(flashes(&output, GATEWAY), [], "it takes nobody in");
    }
    assert!(end.player(id).is_some());
    let mut again = restored(&end, Dimension::End, &about_the_middle(3));
    let asked = questions_in(&mut again, 5);
    assert_eq!(
        asked
            .iter()
            .map(|question| question.from)
            .collect::<Vec<_>>(),
        [GATEWAY]
    );
}

// Entering (`EndGatewayBlock.entityInside`, lines 171-203).

/// The review's first point: a gateway whose exit is not known takes nobody in: the
/// player is not marked, the gateway does not cool down or flash.
#[test]
fn a_gateway_without_an_exit_takes_nobody_in_and_does_not_cool_down() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    let (id, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    for _ in 0..5 {
        let output = idle(&mut end);
        assert_eq!(flashes(&output, GATEWAY), []);
    }
    assert!(end.players.get(&id).expect("there").portal.is_idle());
    assert_eq!(end.gateway_clock(GATEWAY).expect("there").cooldown, 0);
}

/// `entityInside` with `setAsInsidePortal` then `triggerCooldown`, and the gateway's
/// transition time of 0: a player in an open gateway is marked, the gateway flashes
/// (1, 0) and cools down, and the player is gone in the next tick. In an End of one
/// region they wait to be placed here, with no departure (19.5 item 3).
#[test]
fn a_player_in_an_open_gateway_cools_it_and_is_gone_the_next_tick_with_no_departure() {
    let mut end = end_with_an_open_gateway(true);
    let (id, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    let output = idle(&mut end);
    assert_eq!(flashes(&output, GATEWAY), [(1, 0)]);
    assert!(end.player(id).is_some(), "marked, not gone yet");
    assert!(end.gateway_clock(GATEWAY).expect("there").cooldown > 0);
    assert!(!end.gateway_open(GATEWAY));
    let output = idle(&mut end);
    assert!(end.player(id).is_none(), "gone the next tick");
    assert_eq!(departed(&output, id), None, "no departure to itself");
    let waiting = end.waiting_state(id).expect("waits to be placed here");
    assert_eq!(
        waiting.transfer.travel,
        Some(Travel {
            to: Dimension::End,
            target: EXIT,
            way: Way::Gateway {
                from: GATEWAY,
                exact: true,
                island: None,
            },
        })
    );
}

/// `isCoolingDown` in `entityInside` and the cooldown of 40: a player standing in the
/// gateway after another went through is taken in no sooner than 40 ticks after it.
#[test]
fn a_gateway_that_took_a_player_takes_the_next_forty_ticks_later() {
    let mut end = end_with_an_open_gateway(true);
    add_player(&mut end, 1, feet_in(GATEWAY), false);
    let mut first = None;
    let mut second = None;
    let mut waiting = None;
    for tick in 0..60 {
        let output = idle(&mut end);
        if !flashes(&output, GATEWAY).is_empty() {
            if first.is_none() {
                first = Some(tick);
                let (id, _) = add_player(&mut end, 2, feet_in(GATEWAY), false);
                waiting = Some(id);
            } else if second.is_none() {
                second = Some(tick);
            }
        }
    }
    let (first, second) = (first.expect("the first"), second.expect("the second"));
    assert_eq!(second - first, 40, "taken 40 ticks after the first");
    // And through it: placed at the exit by now.
    let (_, pose) = end.player(waiting.expect("the second")).expect("placed");
    assert_eq!(
        [pose.position.x, pose.position.y, pose.position.z],
        [-19.5, 60.0, -19.5]
    );
}

/// `entityInside`'s `isCoolingDown`: the first player taken in starts the cooldown, so
/// a second standing in the same gateway in the same tick is not taken in with them.
#[test]
fn of_two_players_in_a_gateway_in_one_tick_only_one_goes() {
    let mut end = end_with_an_open_gateway(true);
    let (one, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    let (two, _) = add_player(&mut end, 2, [40.7, f64::from(FLOOR + 1), 8.3], false);
    let output = idle(&mut end);
    assert_eq!(flashes(&output, GATEWAY).len(), 1, "one entry");
    idle(&mut end);
    let gone = [one, two]
        .into_iter()
        .filter(|id| end.player(*id).is_none())
        .count();
    assert_eq!(gone, 1, "one goes, the other waits for the cooldown");
}

/// `entityInside` for an entity other than a player: the item is marked and the
/// gateway cools down; in the next tick it goes, filed for the exit's chunk on the
/// gateway's way.
#[test]
fn an_item_in_an_open_gateway_cools_it_and_goes_the_next_tick_for_the_exit_s_chunk() {
    let mut end = end_with_an_open_gateway(false);
    let key = item(&mut end, feet_in(GATEWAY));
    let output = idle(&mut end);
    assert_eq!(flashes(&output, GATEWAY), [(1, 0)]);
    assert!(end.find_entity(key).is_some());
    let output = idle(&mut end);
    assert!(end.find_entity(key).is_none(), "gone the next tick");
    let [traveller] = &output.travellers[..] else {
        panic!("one traveller: {:?}", output.travellers);
    };
    assert_eq!(
        traveller.travel,
        Travel {
            to: Dimension::End,
            target: EXIT,
            way: Way::Gateway {
                from: GATEWAY,
                exact: false,
                island: None,
            },
        }
    );
    assert_eq!(
        traveller.chunk,
        LevelChunk::new(Dimension::End, EXIT.chunk())
    );
    assert_eq!(
        traveller
            .entities
            .iter()
            .map(|entity| entity.key)
            .collect::<Vec<_>>(),
        [key]
    );
}

// Where one comes out (`getPortalPosition`, lines 267-317; `findExitPosition`, 319-338).

/// `getPortalPosition`, offsets 67-78 and 90-94: through an exact gateway a player comes
/// out at the exit's bottom centre, with the look they had, placed with no respawn.
#[test]
fn through_an_exact_gateway_a_player_comes_out_at_the_exit_with_their_look() {
    let mut end = end_with_an_open_gateway(true);
    let (id, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    if let Some(player) = end.players.get_mut(&id) {
        player.pose.yaw = 123.0;
        player.pose.pitch = -17.5;
    }
    let (_, pose) = until_placed(&mut end, id);
    assert_eq!(
        [pose.position.x, pose.position.y, pose.position.z],
        [-19.5, 60.0, -19.5]
    );
    assert_eq!((pose.yaw, pose.pitch), (123.0, -17.5));
    let (_, now) = end.player(id).expect("in the world here");
    assert_eq!(now.position, pose.position);
}

/// `findExitPosition`: `findTallestBlock(exit + (0, 2, 0), 5, false).above()`: within 5
/// of the exit on x and z, the middle column passed over, bedrock looked through; the
/// tallest strictly wins, and the player stands on it.
#[test]
fn through_a_gateway_that_is_not_exact_a_player_stands_on_the_tallest_block_within_five() {
    let mut end = end_with_an_open_gateway(false);
    let mut output = TickOutput::default();
    let mut pillar = |end: &mut Region, dx: i32, dz: i32, top: i32, state: BlockState| {
        for y in FLOOR + 1..=top {
            end.set_block(
                EXIT.offset(dx, 0, dz).offset(0, y - EXIT.y, 0),
                state,
                &mut output,
            );
        }
    };
    // Within 5: end stone to 50 at dx 5; outside: to 55 at dx 6; in the middle column
    // to 58; bedrock at 57 over end stone to 52 at (-3, 2).
    pillar(&mut end, 5, 0, 50, blocks::END_STONE);
    pillar(&mut end, 6, 0, 55, blocks::END_STONE);
    pillar(&mut end, 0, 0, 58, blocks::END_STONE);
    pillar(&mut end, -3, 2, 52, blocks::END_STONE);
    end.set_block(
        EXIT.offset(-3, 57 - EXIT.y, 2),
        blocks::BEDROCK,
        &mut output,
    );
    let (id, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    let (_, pose) = until_placed(&mut end, id);
    assert_eq!(
        [pose.position.x, pose.position.y, pose.position.z],
        [-22.5, 53.0, -17.5]
    );
}

/// `findTallestBlock`, offsets 158-169: with nothing within 5 of the exit, the place is
/// two above the exit, and the player stands one above that.
#[test]
fn through_a_gateway_over_the_void_a_player_comes_out_three_above_the_exit() {
    let mut end = Land::of(Dimension::End, about_the_middle(3), None).region();
    let mut output = TickOutput::default();
    end.set_block(GATEWAY.offset(0, -1, 0), blocks::END_STONE, &mut output);
    place_gateway(&mut end, GATEWAY);
    write_gateway(
        &mut end,
        GATEWAY,
        GatewayData {
            age: 0,
            exit: Some(EXIT),
            exact: false,
        },
    );
    let (id, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    let (_, pose) = until_placed(&mut end, id);
    assert_eq!(
        [pose.position.x, pose.position.y, pose.position.z],
        [-19.5, 63.0, -19.5]
    );
}

/// `getPortalDestination` (EndGatewayBlock 72-109): anything but a pearl keeps its
/// velocity and look (`DELTA ∪ ROTATION`): the item filed through the gateway is taken
/// by the exit's holder at the exit's place with the motion and yaw it left with.
#[test]
fn an_item_through_a_gateway_comes_out_with_the_motion_and_look_it_had() {
    let mut end = end_with_an_open_gateway(true);
    let key = item(&mut end, feet_in(GATEWAY));
    let mut filed = None;
    for _ in 0..3 {
        let output = idle(&mut end);
        if let Some(traveller) = output.travellers.into_iter().next() {
            filed = Some(traveller);
            break;
        }
    }
    let mut traveller = filed.expect("filed");
    traveller.entities[0].motion = [0.25, 0.5, -0.125];
    traveller.entities[0].yaw = 33.0;
    let output = end.tick(&TickInputs {
        travellers: vec![(
            EffectId {
                sequence: 1,
                index: 0,
            },
            traveller,
        )],
        ..TickInputs::default()
    });
    let taken = |output: &TickOutput| {
        output
            .entity_changes
            .iter()
            .find_map(|(_, change)| match change {
                EntityChange::Whole(entity) if entity.key == key => Some(entity.clone()),
                _ => None,
            })
    };
    let mut placed = taken(&output);
    for _ in 0..10 {
        if placed.is_some() {
            break;
        }
        placed = taken(&idle(&mut end));
    }
    let placed = placed.expect("taken in at the exit");
    assert_eq!(placed.position, [-19.5, 60.0, -19.5]);
    assert_eq!(placed.motion, [0.25, 0.5, -0.125]);
    assert_eq!(placed.yaw, 33.0);
}

// The island and the return gateway (the review's second point).

/// `EndGatewayFeature.place` (EndGatewayFeature.txt 45-192) about `centre`: the
/// gateway, air about it on its layer, bedrock in a plus one above and below with air
/// in the corners, and bedrock alone in the middle two above and below.
fn cage(centre: BlockPos) -> Vec<(BlockPos, BlockState)> {
    let mut blocks_of = Vec::new();
    for dz in -1..=1 {
        for dy in -2..=2_i32 {
            for dx in -1..=1 {
                let (same_x, same_y, same_z) = (dx == 0, dy == 0, dz == 0);
                let two = dy.abs() == 2;
                let state = if same_x && same_y && same_z {
                    blocks::END_GATEWAY
                } else if same_y {
                    blocks::AIR
                } else if two && same_x && same_z {
                    blocks::BEDROCK
                } else if (!same_x && !same_z) || two {
                    blocks::AIR
                } else {
                    blocks::BEDROCK
                };
                blocks_of.push((centre.offset(dx, dy, dz), state));
            }
        }
    }
    blocks_of
}

/// The review's second point: the traveller with no entities is taken by the exit's
/// holder, which makes the island about its place (`end_island` with the place's
/// random) and `knownExit(from, false)` at the exit: the cage, and a gateway leading
/// back, not exact. A block of the island in a neighbour's chunk is set there by an
/// effect, `SetIf` the block it read. A second such traveller makes nothing.
#[test]
fn the_exit_s_holder_makes_the_island_and_the_return_gateway_once_and_a_neighbour_s_part_by_an_effect()
 {
    // Held: chunks -1 to 2 on x, -1 to 1 on z, void; copied: x = -2, of nobody's.
    let held: Vec<ChunkPos> = (-1..=2)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let mut land = Land::of(Dimension::End, held, None);
    for z in -1..=1 {
        land.copies
            .push((ChunkPos::new(-2, z), ground(Dimension::End, None)));
    }
    let mut end = land.region();
    let island = BlockPos::new(-14, 75, 8);
    let exit = BlockPos::new(-14, 85, 8);
    let from = BlockPos::new(1_000, 70, 5);
    let marker = |n| {
        (
            EffectId {
                sequence: n,
                index: 0,
            },
            Traveller {
                travel: Travel {
                    to: Dimension::End,
                    target: exit,
                    way: Way::Gateway {
                        from,
                        exact: false,
                        island: Some(island),
                    },
                },
                chunk: LevelChunk::new(Dimension::End, exit.chunk()),
                entities: Vec::new(),
            },
        )
    };
    let mut outputs = vec![end.tick(&TickInputs {
        travellers: vec![marker(1)],
        ..TickInputs::default()
    })];
    for _ in 0..5 {
        outputs.push(idle(&mut end));
    }
    let effects: Vec<Effect> = outputs
        .iter()
        .flat_map(|output| output.effects.iter().cloned())
        .collect();
    let island_blocks = island_writes(island);
    assert!(
        island_blocks.iter().any(|(pos, _)| pos.x < -16),
        "it reaches chunk -2"
    );
    for (pos, state) in &island_blocks {
        if pos.x < -16 {
            assert!(
                effects.iter().any(|effect| effect.at == *pos
                    && matches!(effect.what, Effected::SetIf { expected, state: set, .. }
                        if expected == blocks::AIR && set == *state)),
                "{pos:?} by an effect: {effects:?}"
            );
        } else {
            assert_eq!(end.block(*pos), Some(*state), "{pos:?} of the island");
        }
    }
    for (pos, state) in cage(exit) {
        assert_eq!(end.block(pos), Some(state), "{pos:?} of the cage");
    }
    let back = end.gateway_at(exit).expect("the return gateway");
    assert_eq!((back.exit, back.exact), (Some(from), false));

    // Once: a second traveller like it makes nothing.
    let mut again = vec![end.tick(&TickInputs {
        travellers: vec![marker(2)],
        ..TickInputs::default()
    })];
    for _ in 0..5 {
        again.push(idle(&mut end));
    }
    for output in &again {
        assert_eq!(output.effects, [], "nothing made again");
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::BlockChanged { .. })),
            "nothing made again: {:?}",
            output.events
        );
    }
}

// An End of more than one region.

/// 19.8, "Where the traveller comes out": the End's home sends the traveller on by the
/// land's knowledge to the holder of the exit's chunk; only where the exit is its own
/// does it place them itself.
#[test]
fn the_end_s_home_sends_a_gateway_s_traveller_on_to_the_holder_of_the_exit_s_chunk() {
    let mut land = Land::of(
        Dimension::End,
        vec![ChunkPos::new(0, 0), GATEWAY.chunk()],
        Some(FLOOR),
    );
    // The exit's chunk and those about it are another region's.
    for x in -1..=1 {
        for z in -1..=1 {
            let at = EXIT.chunk();
            land.others.push((ChunkPos::new(at.x + x, at.z + z), OTHER));
        }
    }
    let mut end = land.region();
    place_gateway(&mut end, GATEWAY);
    write_gateway(
        &mut end,
        GATEWAY,
        GatewayData {
            age: 0,
            exit: Some(EXIT),
            exact: false,
        },
    );
    let (id, _) = add_player(&mut end, 1, feet_in(GATEWAY), false);
    let mut sent_on = None;
    for _ in 0..20 {
        let output = idle(&mut end);
        assert_eq!(placing(&output, id), None, "not placed in another's chunk");
        if let Some(to) = departed(&output, id) {
            sent_on = Some(to);
            break;
        }
    }
    assert_eq!(sent_on, Some(OTHER));
}

// A take-over or a restore while a question is out.

/// The review's first and third points: a region restored while its question is out
/// asks again; the answer to the old question, coming to it, is written where it holds
/// the gateway and it has no exit; the answer to the new one then finds the exit known
/// and files nothing more.
#[test]
fn a_region_restored_while_a_question_is_out_asks_again_and_takes_the_first_answer_once() {
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    let [old] = questions_in(&mut end, 3)[..] else {
        panic!("one question");
    };
    let mut again = restored(&end, Dimension::End, &about_the_middle(3));
    let [new] = questions_in(&mut again, 3)[..] else {
        panic!("asked again");
    };
    assert_eq!(new.from, GATEWAY);
    let found = GatewayExit {
        exit: EXIT,
        island: None,
    };
    let output = again.tick(&ask(old, Some(found)));
    assert_eq!(output.travellers.len(), 1, "the exit's making");
    assert_eq!(
        again.gateway_at(GATEWAY).and_then(|data| data.exit),
        Some(EXIT)
    );
    let other = GatewayExit {
        exit: BlockPos::new(900, 80, 3),
        island: Some(BlockPos::new(900, 75, 3)),
    };
    let output = again.tick(&ask(new, Some(other)));
    assert_eq!(output.travellers, []);
    assert_eq!(
        again.gateway_at(GATEWAY).and_then(|data| data.exit),
        Some(EXIT)
    );
}

/// The review's first and third points at a take-over: when the gateway's chunk has
/// gone to another region (a split) while the question is out, the answer coming to
/// the region that asked is not written, and the gateway's new holder asks itself.
#[test]
fn at_a_split_while_a_question_is_out_the_gateway_s_new_holder_asks_and_the_old_answer_is_dropped()
{
    let mut end = Land::end().region();
    place_gateway(&mut end, GATEWAY);
    let [question] = questions_in(&mut end, 3)[..] else {
        panic!("one question");
    };
    let rest: Vec<ChunkPos> = about_the_middle(3)
        .into_iter()
        .filter(|at| *at != GATEWAY.chunk())
        .collect();
    let mut kept = restored(&end, Dimension::End, &rest);
    let mut part = restored(&end, Dimension::End, &[GATEWAY.chunk()]);
    let found = GatewayExit {
        exit: EXIT,
        island: None,
    };
    let output = kept.tick(&ask(question, Some(found)));
    assert_eq!(output.travellers, []);
    assert_eq!(written(&output, GATEWAY), []);
    assert_eq!(questions_in(&mut kept, 3), []);
    let asked = questions_in(&mut part, 3);
    assert_eq!(
        asked
            .iter()
            .map(|question| question.from)
            .collect::<Vec<_>>(),
        [GATEWAY]
    );
}
