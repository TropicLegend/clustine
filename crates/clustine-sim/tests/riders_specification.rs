//! Tests of riders crossing a line with their vehicles (step N5h), written from
//! `docs/groundwork/n5-plan.md` (section 4, section 7 items 2, 4, 5 and 6, section 8
//! items 1 to 5 and 8 to 10), `docs/adr/0028-entities.md` (section 5, "Vehicles and
//! riders", and its amendment with step N5) and the porting brief's findings 12 and 13,
//! by someone who did not write the code, driving two or three regions by their public
//! interface alone and passing between them what a worker and the world store would.
//!
//! The scene: a line at block x = 16 (chunk x = 1). The west region is pinned west of
//! it and believes the east region to hold the land beyond; the east region is pinned
//! east of it. Both have a stone floor at y = -61. Player 1 spawns in the west at
//! (14.5, -60, 8.5), beside an oak boat at (15.5, -60, 8.5) standing on the floor.

use clustine_data::{entity_types, items};
use clustine_sim::api::{
    HOTBAR_SLOTS, Inventory, ItemStack, PlayerInput, RegionEvent, VehicleMove,
};
use clustine_sim::{
    Durable, EdgeEvent, EdgeState, Entered, Holdings, Misdirected, PlayerChange, PlayerEvent,
    PlayerJoin, PlayerTransfer, Region, RegionConfig, RegionState, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Arrival, Biome, Chunk, ChunkArea, ChunkPos, Due, EdgeId, EffectId, Entity, EntityChange,
    EntityData, EntityId, EntityIds, Float, Hurt, Pending, PlayerId, RegionId, Seat, Section, Vec3,
};
use uuid::Uuid;

const EDGE: EdgeId = EdgeId(7);
const WEST_REGION: RegionId = RegionId(1);
const EAST_REGION: RegionId = RegionId(2);
const FAR_REGION: RegionId = RegionId(3);

/// Where player 1 spawns in the west: a block and a half short of the line.
const SPAWN: Vec3 = Vec3::new(14.5, -60.0, 8.5);
/// Where the boat stands.
const BOAT_AT: [f64; 3] = [15.5, -60.0, 8.5];
/// Where a move of the boat's driver puts it: across the line, in chunk (1, 0).
const ACROSS: Vec3 = Vec3::new(16.3, -60.0, 8.5);
/// The chunk beyond the line.
const BEYOND: ChunkPos = ChunkPos::new(1, 0);

/// The seat kept for a player on their way, and the ticks a waiting player waits
/// (ADR-0028, section 5: "The 100 ticks on both sides").
const KEPT_FOR: u64 = 100;
/// The most moves of a vehicle kept for a waiting player (the plan, section 7 item 4).
const KEPT_MOVES: usize = 20;

/// Pinned west of chunk x = 1.
const WEST_OF_THE_LINE: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
/// Pinned from chunk x = 1 east.
const EAST_OF_THE_LINE: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};
/// Pinned to chunk x = 1 alone, for a scene with a third region beyond x = 2.
const ONLY_CHUNK_ONE: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: Some(2),
};

// ------------------------------------------------------------------------------------
// The world.

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn config() -> RegionConfig {
    let mut hotbar = [const { None }; HOTBAR_SLOTS];
    hotbar[0] = Some(ItemStack::new(items::STONE, 64));
    RegionConfig {
        homes: Vec::new(),
        dimension: Default::default(),
        spawn: Some(SPAWN),
        starting_inventory: Inventory::with_hotbar(hotbar),
        return_after: 0,
        lowest_y: -64,
        seed: 0,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: clustine_sim::RANDOM_TICK_SPEED,
    }
}

/// An overworld column with a stone floor at y = -61 and air above.
fn floor() -> Chunk {
    let sections = (0..24)
        .map(|_| Section::filled(clustine_data::blocks::AIR, Biome(0)))
        .collect();
    let mut chunk = Chunk::from_sections(-64, sections);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -61, z, clustine_data::blocks::STONE);
        }
    }
    chunk
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A region pinned to `pinned`, holding and loading the chunks near the origin in it
/// (with `entities` in theirs), copying those beyond it, and believing `beyond` to
/// hold them, which it wants as its viewers' tickets reach them.
struct Side {
    pinned: ChunkArea,
    block: u32,
    beyond: Vec<(ChunkArea, RegionId)>,
    /// Whether a straight rail runs east along z = 8 through chunks x = 0 to 2.
    rails: bool,
}

/// The floor of `position`, with the rail of [`Side::rails`] where it runs.
fn ground(position: ChunkPos, rails: bool) -> Chunk {
    let mut chunk = floor();
    if rails && (0..=2).contains(&position.x) && position.z == 0 {
        let rail =
            clustine_data::BlockState::parse("minecraft:rail[shape=east_west]").expect("a rail");
        for x in 0..16 {
            chunk.set(x, -60, 8, rail);
        }
    }
    chunk
}

fn side(side: &Side, entities: &[Entity]) -> Region {
    let mine = |chunk: &ChunkPos| side.pinned.contains(*chunk);
    let mut state = RegionState::new(EntityIds::block(side.block).expect("a block"));
    state.edges.insert(
        EDGE,
        EdgeState {
            start: 1,
            ..EdgeState::default()
        },
    );
    let holdings = Holdings {
        held: around(5).into_iter().filter(mine).collect(),
        pinned: vec![side.pinned],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let mut region = Region::restore(config(), state, holdings);
    let loaded: Vec<ChunkPos> = around(3).into_iter().filter(mine).collect();
    let others: Vec<ChunkPos> = around(3).into_iter().filter(|c| !mine(c)).collect();
    region.tick(&TickInputs {
        tickets_added: around(3)
            .into_iter()
            .map(|chunk| (chunk, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    });
    let chunks = loaded
        .iter()
        .map(|position| {
            let mut chunk = ground(*position, side.rails);
            chunk.set_entities(
                entities
                    .iter()
                    .filter(|entity| entity.chunk() == *position)
                    .cloned()
                    .collect(),
            );
            (*position, chunk)
        })
        .collect();
    let foreign = others
        .iter()
        .filter_map(|chunk| {
            side.beyond
                .iter()
                .find(|(area, _)| area.contains(*chunk))
                .map(|(_, region)| (*chunk, *region))
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        copy_bases: others
            .iter()
            .map(|chunk| (*chunk, ground(*chunk, side.rails)))
            .collect(),
        foreign,
        ..TickInputs::default()
    });
    for chunk in &loaded {
        assert!(region.chunk(*chunk).is_some(), "{chunk:?} is loaded");
    }
    region
}

fn west(entities: &[Entity]) -> Region {
    side(
        &Side {
            pinned: WEST_OF_THE_LINE,
            block: 1,
            beyond: vec![(EAST_OF_THE_LINE, EAST_REGION)],
            rails: false,
        },
        entities,
    )
}

fn east(entities: &[Entity]) -> Region {
    side(
        &Side {
            pinned: EAST_OF_THE_LINE,
            block: 2,
            beyond: vec![(WEST_OF_THE_LINE, WEST_REGION)],
            rails: false,
        },
        entities,
    )
}

/// Player 1 joins the west and enters the world at its spawn.
fn joined(region: &mut Region) -> EntityId {
    region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player: player(1),
                name: "Rider".to_owned(),
                attempt: 7001,
            },
        )],
        ..TickInputs::default()
    });
    let entered = region
        .state()
        .entering
        .iter()
        .map(|(player, entering)| Entered {
            unit: None,
            player: *player,
            entity: entering.entity_id,
            place: None,
            holder: None,
        })
        .collect();
    region.tick(&TickInputs {
        entered,
        ..TickInputs::default()
    });
    region.player(player(1)).expect("player 1 entered").0
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

/// The key of vehicle number `n`, from a block no region of these tests is given.
fn key(n: u32) -> EntityId {
    EntityId((9 << 32) + u64::from(n))
}

/// An oak boat standing on the floor at `position`, carrying nobody.
fn boat(n: u32, position: [f64; 3]) -> Entity {
    Entity {
        portal: Default::default(),
        key: key(n),
        kind: u16::try_from(entity_types::OAK_BOAT).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 7_000),
        position,
        motion: [0.0; 3],
        yaw: -90.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            // `ON_LAND` both, by `AbstractBoat.Status`'s ordinals.
            status: [3, 3],
            contents: Vec::new(),
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// Player 1's inputs, numbered on from what the region has applied of them.
struct Hands {
    next: u64,
}

impl Hands {
    fn new() -> Self {
        Self { next: 1 }
    }

    fn after(transfer: &PlayerTransfer) -> Self {
        Self {
            next: transfer.last_input + 1,
        }
    }

    fn inputs(
        &mut self,
        entity: EntityId,
        inputs: Vec<PlayerInput>,
    ) -> Vec<(EdgeId, PlayerId, EntityId, u64, PlayerInput)> {
        inputs
            .into_iter()
            .map(|input| {
                let number = self.next;
                self.next += 1;
                (EDGE, player(1), entity, number, input)
            })
            .collect()
    }
}

fn mount(boat: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: boat,
        location: [0.0, 0.3, 0.0],
        sneaking: false,
    }
}

fn drive(to: Vec3) -> PlayerInput {
    PlayerInput::MoveVehicle(VehicleMove {
        position: to,
        yaw: -90.0,
        pitch: 0.0,
        on_ground: true,
    })
}

/// The one record of `key` among the region's loaded chunks, with its chunk.
fn record(region: &Region, key: EntityId) -> Option<(ChunkPos, Entity)> {
    let mut found = Vec::new();
    for chunk in region.loaded_chunks() {
        for entity in region.chunk(chunk).expect("loaded").entities() {
            if entity.key == key {
                found.push((chunk, entity.clone()));
            }
        }
    }
    assert!(found.len() <= 1, "{key:?} is there twice: {found:?}");
    found.into_iter().next()
}

fn seats(region: &Region, vehicle: EntityId) -> Vec<Seat> {
    record(region, vehicle)
        .map(|(_, entity)| entity.riding.passengers)
        .unwrap_or_default()
}

fn departures(output: &TickOutput) -> Vec<(PlayerTransfer, RegionId)> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, durable)| match durable {
            Durable::Departed {
                player: who,
                transfer,
                to,
            } if *who == player(1) => Some((transfer.clone(), *to)),
            _ => None,
        })
        .collect()
}

fn misdirected(output: &TickOutput) -> Vec<(PlayerTransfer, RegionId)> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, durable)| match durable {
            Durable::NotMine {
                what:
                    Misdirected::Arrival {
                        player: who,
                        transfer,
                    },
                holder,
            } if *who == player(1) => Some((transfer.clone(), *holder)),
            _ => None,
        })
        .collect()
}

/// What player 1's own client is told of riding in `output`.
fn rides(output: &TickOutput) -> Vec<Option<EntityId>> {
    output
        .player_events
        .iter()
        .filter_map(|(who, event)| match event {
            PlayerEvent::Rides { vehicle, .. } if *who == player(1) => Some(*vehicle),
            _ => None,
        })
        .collect()
}

fn corrections(output: &TickOutput) -> usize {
    output
        .player_events
        .iter()
        .filter(|(who, event)| {
            *who == player(1) && matches!(event, PlayerEvent::VehicleCorrected { .. })
        })
        .count()
}

/// The passenger lists `output` reports for `vehicle`.
fn passengers_reported(output: &TickOutput, vehicle: EntityId) -> Vec<Vec<EntityId>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Passengers {
                vehicle: reported,
                passengers,
                ..
            } if *reported == vehicle => Some(passengers.clone()),
            _ => None,
        })
        .collect()
}

fn arrival(sequence: u64, chunk: ChunkPos, entity: &Entity) -> Arrival {
    Arrival {
        id: EffectId { sequence, index: 0 },
        chunk,
        entity: entity.clone(),
        due: Due::FromTaking,
    }
}

/// The west with player 1 seated in the boat and their client's teleport behind them.
struct Seated {
    west: Region,
    rider: EntityId,
    hands: Hands,
}

fn seated() -> Seated {
    let mut west = west(&[boat(1, BOAT_AT)]);
    let rider = joined(&mut west);
    let mut hands = Hands::new();
    let output = west.tick(&TickInputs {
        inputs: hands.inputs(rider, vec![mount(key(1))]),
        ..TickInputs::default()
    });
    assert_eq!(rides(&output), [Some(key(1))], "the click seats player 1");
    idle(&mut west);
    Seated { west, rider, hands }
}

/// What the west sent when the boat's driver moved it across the line: the unit's
/// arrival, filed by the root's chunk, and the rider's hand-over.
struct Crossing {
    seated: Seated,
    output: TickOutput,
    unit: (ChunkPos, Entity),
    transfer: PlayerTransfer,
}

fn crossing() -> Crossing {
    let mut seated = seated();
    let output = seated.west.tick(&TickInputs {
        inputs: seated.hands.inputs(seated.rider, vec![drive(ACROSS)]),
        ..TickInputs::default()
    });
    let unit = output
        .arrivals
        .iter()
        .find(|(_, entity)| entity.key == key(1))
        .cloned()
        .expect("the boat was sent");
    let mut sent = departures(&output);
    assert_eq!(sent.len(), 1, "player 1 was handed over once: {sent:?}");
    let (transfer, to) = sent.remove(0);
    assert_eq!(to, EAST_REGION);
    Crossing {
        seated,
        output,
        unit,
        transfer,
    }
}

/// Player `n`, walking, handed over at `at` for the first time.
fn walker(n: u128, at: Vec3) -> PlayerTransfer {
    PlayerTransfer {
        health: 20.0,
        dead: false,
        dimension: Default::default(),
        travel: None,
        seen_credits: false,
        portal: Default::default(),
        unit: None,
        sleeping: None,
        respawn: None,
        experience: Default::default(),
        attack_ticks: None,
        using: None,
        cooldowns: Vec::new(),
        effects: Vec::new(),
        time_since_rest: 0,
        entity_id: EntityId(5 << 32 | n as u64),
        name: format!("Walker{n}"),
        pose: clustine_sim::api::Pose::at(at),
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: Default::default(),
        last_input: 0,
        hops: 1,
        flying: false,
        attempt: None,
        riding: None,
        keys: Default::default(),
        kept: Vec::new(),
        raid_omen_position: None,
    }
}

fn arrive(transfer: &PlayerTransfer) -> PlayerChange {
    PlayerChange::Arrive(EDGE, player(1), transfer.clone())
}

// ------------------------------------------------------------------------------------
// The sending side.

/// ADR-0028, section 5, and the plan, section 4 items 1, 3 and 5: an accepted move of
/// the driver that puts the root into another region's chunk sends the unit in that
/// tick, filed by the root's chunk, `Gone` in its chunk's commit, and hands the rider to
/// the root's new holder in the same commit, naming the vehicle, its chunk and their
/// seat.
#[test]
fn a_driven_boat_moved_across_a_line_is_sent_with_its_rider_handed_over_in_the_same_tick() {
    let crossing = crossing();
    let (chunk, sent) = &crossing.unit;
    assert_eq!(*chunk, BEYOND, "filed by the root's chunk");
    assert_eq!(
        sent.position,
        [ACROSS.x, ACROSS.y, ACROSS.z],
        "at the client's place"
    );
    assert!(
        crossing
            .output
            .entity_changes
            .contains(&(ChunkPos::new(0, 0), EntityChange::Gone(key(1)))),
        "the boat is gone from its chunk in the same commit: {:?}",
        crossing.output.entity_changes
    );
    let riding = crossing
        .transfer
        .riding
        .expect("the transfer names the vehicle");
    assert_eq!(riding.vehicle, key(1));
    assert_eq!(riding.chunk, BEYOND);
    assert_eq!(
        sent.riding
            .passengers
            .iter()
            .map(|seat| seat.key)
            .collect::<Vec<_>>(),
        [crossing.seated.rider],
        "the unit keeps the rider's seat"
    );
    assert!(sent.riding.passengers[0].player);
    assert!(
        sent.riding.passengers[0].pending.is_some(),
        "the rider's seat is kept for them on their way: {:?}",
        sent.riding.passengers
    );
    assert!(
        record(&crossing.seated.west, key(1)).is_none(),
        "the boat left"
    );
    assert!(
        crossing.seated.west.player(player(1)).is_none(),
        "the rider left with it"
    );
}

/// The plan, section 7 item 2: a rider is never shown dismounted at a crossing. The
/// sending region tells the rider's client nothing of a dismount, and reports no
/// passenger list without them.
#[test]
fn the_sending_region_tells_nobody_the_rider_got_off() {
    let crossing = crossing();
    assert!(
        rides(&crossing.output).is_empty(),
        "no teleport off the boat"
    );
    for list in passengers_reported(&crossing.output, key(1)) {
        assert!(
            list.contains(&crossing.seated.rider),
            "a report of the boat without its rider: {list:?}"
        );
    }
    assert!(
        !crossing.output.events.iter().any(|event| matches!(
            event,
            RegionEvent::EntityRemoved { entity, .. } if *entity == crossing.seated.rider
        )),
        "the rider is not removed"
    );
}

/// The plan, section 7 item 8 and section 4 item 3: the hand-over carries the keys the
/// player holds and the move check's state, which the new region needs.
#[test]
fn a_riders_hand_over_carries_the_keys_they_hold() {
    let mut seated = seated();
    let held = clustine_sim::api::Keys {
        forward: true,
        ..Default::default()
    };
    seated.west.tick(&TickInputs {
        inputs: seated
            .hands
            .inputs(seated.rider, vec![PlayerInput::Keys(held)]),
        ..TickInputs::default()
    });
    let output = seated.west.tick(&TickInputs {
        inputs: seated.hands.inputs(seated.rider, vec![drive(ACROSS)]),
        ..TickInputs::default()
    });
    let sent = departures(&output);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.keys, held);
}

/// The plan, section 7 item 11 and section 8 item 8: a crossing within a packet stops
/// the old region applying that player's later inputs, and their last input is the
/// crossing packet, so that the new region applies the rest.
#[test]
fn inputs_after_the_crossing_packet_are_left_to_the_new_region() {
    let mut seated = seated();
    let crossing_number = seated.hands.next;
    let output = seated.west.tick(&TickInputs {
        inputs: seated.hands.inputs(
            seated.rider,
            vec![
                drive(ACROSS),
                drive(Vec3::new(16.8, -60.0, 8.5)),
                drive(Vec3::new(17.3, -60.0, 8.5)),
            ],
        ),
        ..TickInputs::default()
    });
    let sent = departures(&output);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.last_input, crossing_number);
    let (_, unit) = output
        .arrivals
        .iter()
        .find(|(_, entity)| entity.key == key(1))
        .expect("sent");
    assert_eq!(unit.position, [ACROSS.x, ACROSS.y, ACROSS.z]);
}

/// The record's amendment: a unit with a player in it is held back at a line only
/// where its region knows nothing of the land beyond. Into a chunk the region knows as
/// nobody's it is not sent, and the rider stays seated in it.
#[test]
fn a_boat_driven_into_land_its_region_knows_nothing_of_is_held_back_with_its_rider() {
    let mut west = side(
        &Side {
            pinned: WEST_OF_THE_LINE,
            block: 1,
            beyond: Vec::new(),
            rails: false,
        },
        &[boat(1, BOAT_AT)],
    );
    let rider = joined(&mut west);
    let mut hands = Hands::new();
    west.tick(&TickInputs {
        inputs: hands.inputs(rider, vec![mount(key(1))]),
        ..TickInputs::default()
    });
    idle(&mut west);
    let output = west.tick(&TickInputs {
        inputs: hands.inputs(rider, vec![drive(ACROSS)]),
        ..TickInputs::default()
    });
    assert!(
        departures(&output).is_empty(),
        "nobody to hand the rider to"
    );
    assert!(
        output
            .arrivals
            .iter()
            .all(|(_, entity)| entity.key != key(1)),
        "the unit is held back"
    );
    let state = west.player_state(player(1)).expect("still here");
    assert_eq!(state.riding.map(|riding| riding.vehicle), Some(key(1)));
    assert!(rides(&output).is_empty(), "still seated, never dismounted");
}

// ------------------------------------------------------------------------------------
// The receiving side.

/// ADR-0028, section 5, and the plan, section 7 items 2 and 5: the unit comes first and
/// is taken with the rider's seat kept, which its report names, so that their client
/// never sees the boat empty; when the rider comes they are seated with no teleport.
#[test]
fn the_unit_before_the_player_keeps_their_seat_and_seats_them_without_a_teleport() {
    let crossing = crossing();
    let rider = crossing.seated.rider;
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    let taking = east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    assert_eq!(taking.arrivals_taken.len(), 1);
    for list in passengers_reported(&taking, key(1)) {
        assert_eq!(list, [rider], "the kept seat is reported");
    }
    // The taking reports the boat whole, and whatever says who rides it names the
    // rider (the record's amendment: "the vehicle's report of its passengers names
    // them").
    let named = taking.events.iter().any(|event| match event {
        RegionEvent::Passengers {
            vehicle,
            passengers,
            ..
        } => *vehicle == key(1) && passengers.contains(&rider),
        RegionEvent::EntitySpawned(state) => {
            state.entity == key(1)
                && matches!(&state.kind, clustine_sim::api::EntityKind::Other { riding, .. }
                    if riding.passengers.iter().any(|seat| seat.key == rider))
        }
        _ => false,
    });
    assert!(
        named,
        "the boat's first report names its rider: {:?}",
        taking.events
    );
    let kept = seats(&east, key(1));
    assert_eq!(kept.len(), 1);
    assert!(kept[0].pending.is_some(), "kept for them: {kept:?}");

    let seating = east.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    assert!(
        rides(&seating).is_empty(),
        "no teleport on: {:?}",
        seating.player_events
    );
    let state = east.player_state(player(1)).expect("player 1 came");
    let riding = state.riding.expect("riding");
    assert_eq!(riding.vehicle, key(1));
    assert_eq!(riding.pending, None, "seated");
    let now = seats(&east, key(1));
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].key, rider);
    assert_eq!(now[0].pending, None);
    for list in passengers_reported(&seating, key(1)) {
        assert_eq!(list, [rider]);
    }
}

/// The plan, section 4 item 4 and section 7 item 4: a player who comes before their
/// vehicle waits seated, is sent nothing about it, and their client's moves meanwhile
/// are kept; when the unit comes they are seated and the kept moves are applied in
/// order, each as one client tick, from where the boat is, with no correction.
#[test]
fn the_player_before_the_unit_waits_and_their_kept_moves_take_the_boat_on_with_no_correction() {
    let crossing = crossing();
    let mut east = east(&[]);
    let mut hands = Hands::after(&crossing.transfer);
    let rider = crossing.seated.rider;
    let mut quiet = east.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    let moves = [16.8, 17.3, 17.8];
    for x in moves {
        let output = east.tick(&TickInputs {
            inputs: hands.inputs(rider, vec![drive(Vec3::new(x, -60.0, 8.5))]),
            ..TickInputs::default()
        });
        quiet.player_events.extend(output.player_events);
    }
    assert!(
        rides(&quiet).is_empty(),
        "nothing said of the vehicle while waiting"
    );
    assert_eq!(corrections(&quiet), 0, "no correction while waiting");
    let state = east.player_state(player(1)).expect("player 1 came");
    let riding = state.riding.expect("waits seated");
    assert_eq!(riding.vehicle, key(1));
    assert!(riding.pending.is_some(), "vehicle pending");
    assert_eq!(state.kept.len(), moves.len(), "the moves are kept");

    let (chunk, unit) = &crossing.unit;
    let mut seating = east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    seating.player_events.extend(idle(&mut east).player_events);
    assert_eq!(corrections(&seating), 0, "the kept moves are accepted");
    assert!(rides(&seating).is_empty(), "no teleport on");
    let state = east.player_state(player(1)).expect("here");
    assert_eq!(
        state.riding.and_then(|riding| riding.pending),
        None,
        "seated"
    );
    assert!(state.kept.is_empty(), "the kept moves are spent");
    let (_, boat) = record(&east, key(1)).expect("the boat is here");
    assert_eq!(boat.position, [17.8, -60.0, 8.5], "at the last kept move");
    assert_eq!(boat.riding.passengers.len(), 1);
    assert_eq!(boat.riding.passengers[0].pending, None);
}

/// The plan, section 7 item 4: at most twenty moves are kept, the older dropped.
#[test]
fn a_waiting_player_keeps_only_their_latest_twenty_moves() {
    let crossing = crossing();
    let mut east = east(&[]);
    let mut hands = Hands::after(&crossing.transfer);
    east.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    let moves: Vec<PlayerInput> = (1..=25)
        .map(|n| drive(Vec3::new(16.3 + 0.1 * f64::from(n), -60.0, 8.5)))
        .collect();
    east.tick(&TickInputs {
        inputs: hands.inputs(crossing.seated.rider, moves.clone()),
        ..TickInputs::default()
    });
    let state = east.player_state(player(1)).expect("here");
    assert_eq!(state.kept.len(), KEPT_MOVES);
    let latest: Vec<PlayerInput> = state
        .kept
        .iter()
        .map(|kept| PlayerInput::MoveVehicle(*kept))
        .collect();
    assert_eq!(latest, moves[moves.len() - KEPT_MOVES..]);
}

/// The plan, section 8 item 10: when moves were dropped, the first kept one is accepted
/// without the "too quickly" test, as the game never corrects a driver's first move.
/// Five moves of 2.1 blocks north are dropped; the first kept is then 10.5 blocks from
/// the boat, beyond the test's ten (finding 13), and is accepted.
#[test]
fn the_first_kept_move_after_moves_were_dropped_is_not_tested_for_going_too_quickly() {
    let crossing = crossing();
    let mut east = east(&[]);
    let mut hands = Hands::after(&crossing.transfer);
    east.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    let mut moves = Vec::new();
    for n in 1..=5 {
        moves.push(drive(Vec3::new(16.3, -60.0, 8.5 + 2.1 * f64::from(n))));
    }
    for n in 1..=20 {
        moves.push(drive(Vec3::new(
            16.3,
            -60.0,
            8.5 + 10.5 + 0.01 * f64::from(n),
        )));
    }
    east.tick(&TickInputs {
        inputs: hands.inputs(crossing.seated.rider, moves),
        ..TickInputs::default()
    });
    let (chunk, unit) = &crossing.unit;
    let mut seating = east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    seating.player_events.extend(idle(&mut east).player_events);
    assert_eq!(corrections(&seating), 0, "no move is put back");
    let (_, boat) = record(&east, key(1)).expect("here");
    assert!(
        (boat.position[2] - (8.5 + 10.5 + 0.2)).abs() < 1e-9,
        "at the last kept move: {:?}",
        boat.position
    );
}

/// The plan, section 7 item 4: while the controlling seat is pending the new region
/// keeps the boat still, as for a driven boat.
#[test]
fn a_boat_whose_driver_has_not_come_stands_still() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    let mut moving = unit.clone();
    moving.motion = [0.3, 0.0, 0.0];
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, &moving)],
        ..TickInputs::default()
    });
    for _ in 0..5 {
        idle(&mut east);
    }
    let (_, boat) = record(&east, key(1)).expect("here");
    assert_eq!(boat.position, unit.position, "kept still for its driver");
}

/// ADR-0028, section 5, and the plan, section 4 item 4: the seat kept for a player who
/// never comes is let go after 100 ticks, and the boat's report says so.
#[test]
fn a_seat_kept_for_a_player_who_never_comes_is_let_go_after_a_hundred_ticks() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    let mut emptied = None;
    for tick in 1..=KEPT_FOR + 5 {
        let output = idle(&mut east);
        if seats(&east, key(1)).is_empty() {
            assert!(
                passengers_reported(&output, key(1))
                    .iter()
                    .any(|list| list.is_empty()),
                "the boat's report of nobody: {:?}",
                output.events
            );
            emptied = Some(tick);
            break;
        }
    }
    let emptied = emptied.expect("the seat was let go");
    assert!(
        (KEPT_FOR - 2..=KEPT_FOR + 1).contains(&emptied),
        "let go after about {KEPT_FOR} ticks, not {emptied}"
    );
}

/// The same, and a player who comes after the seat was let go is not seated: the plan,
/// section 7 item 5, "a player who comes to a present unit that has dropped their seat
/// is dismounted at once".
#[test]
fn a_player_who_comes_after_their_seat_was_let_go_is_dismounted_at_once() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    for _ in 0..KEPT_FOR + 5 {
        idle(&mut east);
    }
    assert!(seats(&east, key(1)).is_empty());
    let output = east.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    assert_eq!(rides(&output), [None], "their client is told they got off");
    // Got off where they are, which may be on the west's side of the line (a rider is
    // placed a tick after their vehicle's move), and then handed back there standing.
    match east.player_state(player(1)) {
        Some(state) => assert_eq!(state.riding, None, "standing"),
        None => {
            let back = departures(&output);
            assert_eq!(back.len(), 1, "handed on: {:?}", output.durable);
            assert_eq!(back[0].0.riding, None, "standing");
        }
    }
    assert!(seats(&east, key(1)).is_empty(), "no phantom seat");
}

/// ADR-0028, section 5: a player whose vehicle has not come within 100 ticks is
/// dismounted where they are, and is never seated later.
#[test]
fn a_player_whose_unit_never_comes_gets_off_where_they_are_and_is_never_seated_later() {
    let crossing = crossing();
    let mut east = east(&[]);
    // A rider whom their old region had placed beyond the line, so that where they get
    // off is the east's.
    let mut transfer = crossing.transfer.clone();
    transfer.pose.position = Vec3::new(17.5, -60.0, 8.5);
    east.tick(&TickInputs {
        player_changes: vec![arrive(&transfer)],
        ..TickInputs::default()
    });
    let mut off = None;
    for tick in 1..=KEPT_FOR + 5 {
        let output = idle(&mut east);
        let state = east.player_state(player(1)).expect("here");
        if state.riding.is_none() {
            assert_eq!(rides(&output), [None], "told they got off");
            off = Some(tick);
            break;
        }
        assert!(rides(&output).is_empty(), "nothing said while waiting");
    }
    let off = off.expect("dismounted");
    assert!(
        (KEPT_FOR - 2..=KEPT_FOR + 1).contains(&off),
        "after about {KEPT_FOR} ticks, not {off}"
    );
    let standing = east.player(player(1)).expect("here").1;
    assert!(
        (standing.position.x - 17.5).abs() < 2.0 && (standing.position.z - ACROSS.z).abs() < 2.0,
        "where they are: {standing:?}"
    );

    // The unit comes late, its seat still kept by its own count: the player is not
    // seated by it, and the seat goes.
    let (chunk, unit) = &crossing.unit;
    let output = east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    idle(&mut east);
    assert!(rides(&output).is_empty(), "not seated later");
    assert_eq!(east.player_state(player(1)).expect("here").riding, None);
    assert!(
        seats(&east, key(1))
            .iter()
            .all(|seat| seat.key != crossing.seated.rider),
        "no phantom seat: {:?}",
        seats(&east, key(1))
    );
}

/// The plan, section 7 item 5: while pending, sneaking is refused.
#[test]
fn a_waiting_player_who_sneaks_stays_seated() {
    let crossing = crossing();
    let mut east = east(&[]);
    let mut hands = Hands::after(&crossing.transfer);
    east.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    let output = east.tick(&TickInputs {
        inputs: hands.inputs(
            crossing.seated.rider,
            vec![PlayerInput::Keys(clustine_sim::api::Keys {
                sneak: true,
                ..Default::default()
            })],
        ),
        ..TickInputs::default()
    });
    idle(&mut east);
    assert!(rides(&output).is_empty());
    let riding = east
        .player_state(player(1))
        .and_then(|state| state.riding)
        .expect("still riding");
    assert!(riding.pending.is_some(), "still waiting for the boat");
}

/// The plan, section 7 item 5: while pending, mounting elsewhere is refused.
#[test]
fn a_waiting_player_who_clicks_another_boat_stays_seated_in_theirs() {
    let crossing = crossing();
    let mut east = east(&[boat(2, [17.5, -60.0, 8.5])]);
    let mut hands = Hands::after(&crossing.transfer);
    east.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    let output = east.tick(&TickInputs {
        inputs: hands.inputs(crossing.seated.rider, vec![mount(key(2))]),
        ..TickInputs::default()
    });
    assert!(
        rides(&output).is_empty(),
        "refused: {:?}",
        output.player_events
    );
    let riding = east
        .player_state(player(1))
        .and_then(|state| state.riding)
        .expect("still riding");
    assert_eq!(riding.vehicle, key(1));
    assert!(
        seats(&east, key(2)).is_empty(),
        "nobody sits in the other boat"
    );
}

/// The plan, section 4 item 4 and section 8 item 9: the unit's region drops a pending
/// player passenger whose hand-over is discarded.
#[test]
fn a_seat_kept_for_a_player_whose_hand_over_is_discarded_goes() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    assert_eq!(seats(&east, key(1)).len(), 1);
    let output = east.tick(&TickInputs {
        player_changes: vec![PlayerChange::Discard {
            player: PlayerId(uuid::Uuid::from_u128(1)),
            entity: crossing.seated.rider,
            hops: 1,
            chunk: ChunkPos::new(0, 0),
        }],
        ..TickInputs::default()
    });
    let next = idle(&mut east);
    assert!(seats(&east, key(1)).is_empty(), "the seat went");
    assert!(
        passengers_reported(&output, key(1))
            .iter()
            .chain(passengers_reported(&next, key(1)).iter())
            .any(|list| list.is_empty()),
        "and the boat's report says so"
    );
}

/// The plan, section 8 item 9: the edge in each entry lets the unit's region drop the
/// entries of an edge that goes away.
#[test]
fn a_seat_kept_for_a_player_of_an_edge_that_goes_away_goes() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    let kept = seats(&east, key(1));
    assert_eq!(kept[0].pending.map(|pending| pending.edge), Some(EDGE));
    east.tick(&TickInputs {
        edges: vec![EdgeEvent::Gone { edge: EDGE }],
        ..TickInputs::default()
    });
    idle(&mut east);
    assert!(
        seats(&east, key(1)).is_empty(),
        "the seat went with the edge"
    );
}

/// The plan, section 8 item 5: a player who comes naming no vehicle drops any pending
/// entry of theirs rather than be seated by it.
#[test]
fn a_player_who_comes_riding_nothing_is_not_seated_by_a_seat_kept_for_them() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    let mut walking = crossing.transfer.clone();
    walking.riding = None;
    walking.hops += 1;
    walking.pose.position = Vec3::new(17.5, -60.0, 8.5);
    let output = east.tick(&TickInputs {
        player_changes: vec![arrive(&walking)],
        ..TickInputs::default()
    });
    idle(&mut east);
    idle(&mut east);
    assert!(rides(&output).is_empty());
    assert_eq!(east.player_state(player(1)).expect("here").riding, None);
    assert!(seats(&east, key(1)).is_empty(), "the kept seat went");
}

/// The plan, section 7 item 5, and the record's amendment: a unit sent on to a third
/// region while a seat is kept leaves a forwarding entry, so that the player follows:
/// they are handed on with the transfer's vehicle chunk rewritten (section 8 item 5).
/// A cart, which its region moves, carries the kept seat over the next line at x = 32.
#[test]
fn a_unit_sent_on_while_a_seat_is_kept_forwards_the_player_who_comes_after_it() {
    let rider = EntityId(1 << 32 | 9);
    let mut cart = boat(3, [29.5, -60.0, 8.5]);
    cart.kind = u16::try_from(entity_types::MINECART).expect("fits");
    cart.data = EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents: Vec::new(),
        fuel: 0,
        push: [clustine_world::Double(0.0); 2],
        fuse: -1,
        enabled: true,
    };
    cart.on_ground = false;
    cart.motion = [0.4, 0.0, 0.0];
    cart.riding.passengers = vec![Seat {
        key: rider,
        player: true,
        pending: Some(Pending {
            left: KEPT_FOR as u16,
            edge: EDGE,
            granted: false,
        }),
    }];
    let mut middle = side(
        &Side {
            pinned: ONLY_CHUNK_ONE,
            block: 2,
            beyond: vec![
                (WEST_OF_THE_LINE, WEST_REGION),
                (
                    ChunkArea {
                        min_x: Some(2),
                        max_x: None,
                    },
                    FAR_REGION,
                ),
            ],
            rails: true,
        },
        &[],
    );
    let mut sent = None;
    let first = middle.tick(&TickInputs {
        arrivals: vec![arrival(40, BEYOND, &cart)],
        ..TickInputs::default()
    });
    assert_eq!(first.arrivals_taken.len(), 1, "the cart is taken");
    if let Some(found) = first.arrivals.iter().find(|(_, e)| e.key == cart.key) {
        sent = Some(found.clone());
    }
    for _ in 0..20 {
        if sent.is_some() {
            break;
        }
        let output = idle(&mut middle);
        sent = output
            .arrivals
            .iter()
            .find(|(_, e)| e.key == cart.key)
            .cloned();
    }
    let (chunk, sent) = sent.expect("the cart rolled on over the line at x = 32");
    assert_eq!(chunk.x, 2);
    assert_eq!(sent.riding.passengers.len(), 1, "with the kept seat");
    assert!(sent.riding.passengers[0].pending.is_some());

    let transfer = PlayerTransfer {
        health: 20.0,
        dead: false,
        dimension: Default::default(),
        travel: None,
        seen_credits: false,
        portal: Default::default(),
        unit: None,
        sleeping: None,
        respawn: None,
        experience: Default::default(),
        attack_ticks: None,
        using: None,
        cooldowns: Vec::new(),
        effects: Vec::new(),
        time_since_rest: 0,
        entity_id: rider,
        name: "Rider".to_owned(),
        pose: clustine_sim::api::Pose::at(Vec3::new(31.0, -60.0, 8.5)),
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: Default::default(),
        last_input: 4,
        hops: 1,
        flying: false,
        attempt: None,
        riding: Some(clustine_sim::PlayerRiding {
            vehicle: cart.key,
            chunk: BEYOND,
            check: Default::default(),
            pending: Some(KEPT_FOR as u16),
        }),
        keys: Default::default(),
        kept: Vec::new(),
        raid_omen_position: None,
    };
    let output = middle.tick(&TickInputs {
        player_changes: vec![arrive(&transfer)],
        ..TickInputs::default()
    });
    let mut onward = misdirected(&output);
    onward.extend(departures(&output));
    assert_eq!(onward.len(), 1, "handed on: {:?}", output.durable);
    let (onward, to) = onward.remove(0);
    assert_eq!(to, FAR_REGION);
    assert_eq!(
        onward.riding.map(|riding| (riding.vehicle, riding.chunk)),
        Some((cart.key, chunk)),
        "the vehicle's chunk is rewritten"
    );
    assert!(middle.player(player(1)).is_none());
}

// ------------------------------------------------------------------------------------
// Take-ins (the plan, section 8 item 1).

/// The plan, section 8 item 1: at any take-in an entry with time left is kept, and
/// player passengers neither present nor pending are dropped. A restore of the east
/// from its saved state and chunks keeps the seat kept for the player on their way.
#[test]
fn a_restore_keeps_a_seat_kept_with_time_left_and_the_player_is_seated_when_they_come() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    for _ in 0..10 {
        idle(&mut east);
    }
    let state = east.state();
    let saved: Vec<(ChunkPos, Chunk)> = east
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| east.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    let holdings = Holdings {
        held: around(5)
            .into_iter()
            .filter(|chunk| EAST_OF_THE_LINE.contains(*chunk))
            .collect(),
        pinned: vec![EAST_OF_THE_LINE],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let tickets: Vec<(ChunkPos, Ticket)> = around(3)
        .into_iter()
        .map(|chunk| (chunk, Ticket::Viewer))
        .collect();
    let mut restored =
        Region::restore_loaded(config(), state, holdings, &tickets, saved, Vec::new());
    idle(&mut restored);
    let kept = seats(&restored, key(1));
    assert_eq!(
        kept.len(),
        1,
        "the seat is kept across the restore: {kept:?}"
    );
    assert!(kept[0].pending.is_some());
    restored.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    let riding = restored
        .player_state(player(1))
        .and_then(|state| state.riding)
        .expect("seated");
    assert_eq!(riding.vehicle, key(1));
    assert_eq!(riding.pending, None);
}

/// The same rule's other half: a take-in drops a player passenger neither present nor
/// pending. A chunk loaded with a boat whose record names a seated player the region
/// does not have leaves no phantom seat.
#[test]
fn a_boat_loaded_naming_a_seated_player_who_is_not_there_drops_the_seat() {
    let mut stale = boat(1, [20.5, -60.0, 8.5]);
    stale.riding.passengers = vec![Seat {
        key: EntityId(1 << 32 | 9),
        player: true,
        pending: None,
    }];
    let mut east = east(&[stale]);
    idle(&mut east);
    assert!(
        seats(&east, key(1)).is_empty(),
        "{:?}",
        seats(&east, key(1))
    );
}

/// The plan, section 8 item 1: a split's part takes its chunks in, and keeps the seat
/// kept for the player on their way.
#[test]
fn a_split_keeps_a_seat_kept_with_time_left_in_the_part_that_takes_the_boat() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    // A split needs someone to go and someone to stay: player 2 in the boat's chunk,
    // player 3 two chunks on.
    east.tick(&TickInputs {
        player_changes: vec![
            PlayerChange::Arrive(EDGE, player(2), walker(2, Vec3::new(20.5, -60.0, 3.5))),
            PlayerChange::Arrive(EDGE, player(3), walker(3, Vec3::new(52.5, -60.0, 8.5))),
        ],
        ..TickInputs::default()
    });
    assert!(east.player(player(2)).is_some() && east.player(player(3)).is_some());
    let named: Vec<ChunkPos> = east
        .loaded_chunks()
        .into_iter()
        .filter(|chunk| chunk.x == 1)
        .collect();
    let splitting = match east.split(&named, FAR_REGION, &[]) {
        Ok(splitting) => splitting,
        Err(refused) => panic!("the east splits: {refused:?}"),
    };
    let saved: Vec<(ChunkPos, Chunk)> = splitting
        .chunks
        .iter()
        .filter_map(|chunk| east.saved_chunk(*chunk).map(|saved| (*chunk, saved)))
        .collect();
    assert!(
        saved.iter().any(|(position, _)| *position == BEYOND),
        "the part takes the boat's chunk"
    );
    let holdings = Holdings {
        held: splitting.chunks.clone(),
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let tickets: Vec<(ChunkPos, Ticket)> = splitting
        .chunks
        .iter()
        .map(|chunk| (*chunk, Ticket::Viewer))
        .collect();
    let mut part = Region::restore_loaded(
        config(),
        splitting.part.clone(),
        holdings,
        &tickets,
        saved,
        Vec::new(),
    );
    idle(&mut part);
    let kept = seats(&part, key(1));
    assert_eq!(kept.len(), 1, "the seat goes with the boat: {kept:?}");
    assert!(kept[0].pending.is_some());
}

/// The plan, section 8 item 1: a merge takes the absorbed region's chunks in and keeps
/// the seat kept for the player on their way; the player then comes to the survivor
/// and is seated.
#[test]
fn a_merge_keeps_a_seat_kept_with_time_left_and_seats_the_player_in_the_survivor() {
    let crossing = crossing();
    let mut east = east(&[]);
    let (chunk, unit) = &crossing.unit;
    east.tick(&TickInputs {
        arrivals: vec![arrival(40, *chunk, unit)],
        ..TickInputs::default()
    });
    idle(&mut east);
    let west = crossing.seated.west;
    let merged = west.absorb(EAST_REGION, &east.state());
    let mut chunks: Vec<(ChunkPos, Chunk)> = Vec::new();
    for (region, area) in [(&west, WEST_OF_THE_LINE), (&east, EAST_OF_THE_LINE)] {
        for chunk in region.loaded_chunks() {
            if area.contains(chunk)
                && let Some(saved) = region.saved_chunk(chunk)
            {
                chunks.push((chunk, saved));
            }
        }
    }
    let holdings = Holdings {
        held: around(5),
        pinned: vec![ChunkArea::EVERYWHERE],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let tickets: Vec<(ChunkPos, Ticket)> = around(3)
        .into_iter()
        .map(|chunk| (chunk, Ticket::Viewer))
        .collect();
    let mut survivor =
        Region::restore_loaded(config(), merged, holdings, &tickets, chunks, Vec::new());
    idle(&mut survivor);
    let kept = seats(&survivor, key(1));
    assert_eq!(kept.len(), 1, "the seat survives the merge: {kept:?}");
    survivor.tick(&TickInputs {
        player_changes: vec![arrive(&crossing.transfer)],
        ..TickInputs::default()
    });
    let riding = survivor
        .player_state(player(1))
        .and_then(|state| state.riding)
        .expect("seated");
    assert_eq!(riding.vehicle, key(1));
    assert_eq!(riding.pending, None);
}

// ------------------------------------------------------------------------------------
// Leaving, and a hit across the line.

/// The plan, section 7 items 1 and 6, and section 8 item 6, as
/// `docs/groundwork/n7h-logout-on-a-mount.md` section 3 narrows them: a player who leaves
/// alone on a boat takes it out of the world with them in the commit that removes them,
/// and their stay note carries it with the place it would let them off at, not their
/// seat, where they stand if it is not put under them again.
#[test]
fn a_rider_who_leaves_alone_takes_the_boat_and_their_note_has_where_they_would_get_off() {
    let mut seated = seated();
    let (_, boat) = record(&seated.west, key(1)).expect("the boat");
    let output = seated.west.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(
            EDGE,
            player(1),
            Some(seated.rider),
            None,
        )],
        ..TickInputs::default()
    });
    assert!(seated.west.player(player(1)).is_none());
    assert!(record(&seated.west, key(1)).is_none(), "the boat goes too");
    let note = output.stays.iter().find_map(|note| match note {
        clustine_sim::StayNote::Has {
            player: who,
            place,
            unit,
            ..
        } if *who == player(1) => Some((place.clone(), unit.clone())),
        _ => None,
    });
    let (place, unit) = note.expect("a note of where they would get off");
    assert_eq!(unit.expect("the boat in the note").root().key, key(1));
    let sat_y = boat.position[1] + 0.1;
    assert!(
        (place.pose.position.y - sat_y).abs() > 0.05 || place.pose.position.x != boat.position[0],
        "the note's place is not the seat: {:?}",
        place.pose
    );
}

/// The plan, decision 10 and section 7 item 9: a hit on a vehicle across a line is an
/// effect on the entity, filed for its chunk, which its holder does.
#[test]
fn a_creative_hit_on_a_boat_across_the_line_is_an_effect_that_breaks_it_in_its_holder() {
    let beyond = boat(5, [16.5, -60.0, 8.5]);
    let mut west = west(&[]);
    // The west's copy of the east's chunk shows the boat.
    let mut copy = floor();
    copy.set_entities(vec![beyond.clone()]);
    west.tick(&TickInputs {
        copy_bases: vec![(BEYOND, copy)],
        ..TickInputs::default()
    });
    let rider = joined(&mut west);
    let mut hands = Hands::new();
    let output = west.tick(&TickInputs {
        inputs: hands.inputs(
            rider,
            vec![PlayerInput::Attack {
                part: None,
                entity: beyond.key,
            }],
        ),
        ..TickInputs::default()
    });
    let effect = output
        .effects
        .iter()
        .find(|effect| {
            matches!(effect.what, clustine_world::Effected::OnEntity { key, .. } if key == beyond.key)
        })
        .cloned()
        .expect("the hit goes as an effect on the boat");
    assert_eq!(effect.at.chunk(), BEYOND, "addressed to the boat's chunk");

    let mut east = east(std::slice::from_ref(&beyond));
    east.tick(&TickInputs {
        effects: vec![clustine_world::FiledEffect {
            id: EffectId {
                sequence: 50,
                index: 0,
            },
            effect,
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    });
    // Clustine's players are creative, and a creative hit discards a vehicle
    // (finding 22): the effect breaks the boat in its holder, and it drops nothing.
    assert!(record(&east, beyond.key).is_none(), "the boat is broken");
    for chunk in east.loaded_chunks() {
        let entities = east.chunk(chunk).expect("loaded").entities();
        assert!(
            entities
                .iter()
                .all(|entity| entity.kind != u16::try_from(entity_types::ITEM).expect("fits")),
            "nothing dropped: {entities:?}"
        );
    }
}

/// A minecart standing on the rail at `position`.
fn cart(n: u32, position: [f64; 3]) -> Entity {
    let mut cart = boat(n, position);
    cart.kind = u16::try_from(entity_types::MINECART).expect("fits");
    cart.on_ground = false;
    cart.data = EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents: Vec::new(),
        fuel: 0,
        push: [clustine_world::Double(0.0); 2],
        fuse: -1,
        enabled: true,
    };
    cart
}

/// ADR-0028, section 5, the plan, section 4 items 1 and 3 and section 7 item 8: a
/// player on a cart, which the server moves, pushes it east by holding forward (brief
/// finding 3); when the cart rolls across the line the unit is sent and the rider
/// handed over in the same tick, with the key they hold, and the new region seats them
/// and the cart rolls on there with them in it.
#[test]
fn a_cart_pushed_across_a_line_by_its_rider_takes_them_with_it_and_rolls_on_beyond() {
    let rails = |pinned, block, beyond| Side {
        pinned,
        block,
        beyond,
        rails: true,
    };
    let mut west = side(
        &rails(WEST_OF_THE_LINE, 1, vec![(EAST_OF_THE_LINE, EAST_REGION)]),
        &[cart(4, [14.5, -60.0, 8.5])],
    );
    let rider = joined(&mut west);
    let mut hands = Hands::new();
    let output = west.tick(&TickInputs {
        inputs: hands.inputs(rider, vec![mount(key(4))]),
        ..TickInputs::default()
    });
    assert_eq!(rides(&output), [Some(key(4))], "seated in the cart");
    let forward = clustine_sim::api::Keys {
        forward: true,
        ..Default::default()
    };
    west.tick(&TickInputs {
        inputs: hands.inputs(
            rider,
            vec![
                PlayerInput::Move {
                    position: None,
                    rotation: Some((-90.0, 0.0)),
                    on_ground: false,
                },
                PlayerInput::Keys(forward),
            ],
        ),
        ..TickInputs::default()
    });
    let mut sending = None;
    for _ in 0..600 {
        let output = idle(&mut west);
        assert!(
            rides(&output).is_empty(),
            "never got off: {:?}",
            output.player_events
        );
        if !departures(&output).is_empty() {
            sending = Some(output);
            break;
        }
    }
    let sending = sending.expect("the cart rolled over the line");
    let (transfer, to) = departures(&sending).remove(0);
    assert_eq!(to, EAST_REGION);
    assert_eq!(transfer.keys, forward, "the held key goes with them");
    let (chunk, unit) = sending
        .arrivals
        .iter()
        .find(|(_, entity)| entity.key == key(4))
        .cloned()
        .expect("the cart is sent in the same tick");
    assert_eq!(chunk, BEYOND);
    assert_eq!(
        transfer.riding.map(|riding| (riding.vehicle, riding.chunk)),
        Some((key(4), BEYOND))
    );

    let mut east = side(
        &rails(EAST_OF_THE_LINE, 2, vec![(WEST_OF_THE_LINE, WEST_REGION)]),
        &[],
    );
    let taking = east.tick(&TickInputs {
        arrivals: vec![arrival(40, chunk, &unit)],
        player_changes: vec![arrive(&transfer)],
        ..TickInputs::default()
    });
    assert!(
        rides(&taking).is_empty(),
        "no teleport: {:?}",
        taking.player_events
    );
    let riding = east
        .player_state(player(1))
        .and_then(|state| state.riding)
        .expect("seated beyond the line");
    assert_eq!((riding.vehicle, riding.pending), (key(4), None));
    let start = record(&east, key(4)).expect("the cart is here").1.position[0];
    for _ in 0..10 {
        idle(&mut east);
    }
    let (_, cart) = record(&east, key(4)).expect("the cart is here");
    assert!(
        cart.position[0] > start,
        "it rolls on east: {start} to {:?}",
        cart.position
    );
    assert_eq!(
        cart.riding
            .passengers
            .iter()
            .map(|seat| seat.key)
            .collect::<Vec<_>>(),
        [rider]
    );
}

/// `docs/groundwork/n7h-logout-on-a-mount.md`, section 5: a player who left alone on a
/// boat and joins again is put back on it. The home region told of the unit with the
/// stay that enters takes it out of the record (a `Took` in that tick's commit) and
/// sends it to its root's chunk as an arrival, the player's seat in it naming the new
/// stay and kept pending; the stay enters at the seat, riding it and waiting, its client
/// told what it rides; and once the arrival is taken, one hop higher, it is seated.
#[test]
fn a_player_who_left_alone_on_a_boat_is_put_back_on_it_when_they_join_again() {
    let mut seated = seated();
    let output = seated.west.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(
            EDGE,
            player(1),
            Some(seated.rider),
            None,
        )],
        ..TickInputs::default()
    });
    let (place, unit) = output
        .stays
        .iter()
        .find_map(|note| match note {
            clustine_sim::StayNote::Has {
                place,
                unit: Some(unit),
                ..
            } => Some((place.clone(), unit.clone())),
            _ => None,
        })
        .expect("the leave's note carries the boat");
    let left_hops = unit.root().hops;

    // The join, and the store's word: the place and the unit.
    seated.west.tick(&TickInputs {
        player_changes: vec![PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player: player(1),
                name: "Rider".to_owned(),
                attempt: 7002,
            },
        )],
        ..TickInputs::default()
    });
    let entering = seated.west.state().entering[&player(1)].entity_id;
    let output = seated.west.tick(&TickInputs {
        entered: vec![Entered {
            player: player(1),
            entity: entering,
            place: Some(place),
            holder: None,
            unit: Some(unit),
        }],
        ..TickInputs::default()
    });
    assert!(
        output.stays.iter().any(|note| matches!(
            note,
            clustine_sim::StayNote::Took { player: who, root } if *who == player(1) && *root == key(1)
        )),
        "the taking: {:?}",
        output.stays
    );
    let sent: Vec<&Entity> = output
        .arrivals
        .iter()
        .filter(|(chunk, entity)| entity.key == key(1) && *chunk == entity.chunk())
        .map(|(_, entity)| entity)
        .collect();
    let [boat_back] = sent[..] else {
        panic!("the boat sent back to its chunk: {:?}", output.arrivals);
    };
    assert_eq!(boat_back.hops, left_hops, "at the hops it left with");
    let seat = boat_back.riding.passengers.first().expect("a seat");
    assert!(seat.player && seat.key == entering && seat.pending.is_some());
    let spawned = output
        .player_events
        .iter()
        .find_map(|(who, event)| match event {
            PlayerEvent::Spawned { riding, .. } if *who == player(1) => Some(*riding),
            _ => None,
        });
    assert_eq!(
        spawned,
        Some(Some(key(1))),
        "the client is told what it rides"
    );
    let state = seated.west.player_state(player(1)).expect("entered");
    assert!(state.riding.is_some_and(|riding| riding.pending.is_some()));

    // The arrival is taken, and the player seated in it.
    let chunk = boat_back.chunk();
    let boat_back = boat_back.clone();
    seated.west.tick(&TickInputs {
        arrivals: vec![arrival(900, chunk, &boat_back)],
        ..TickInputs::default()
    });
    idle(&mut seated.west);
    let (_, boat) = record(&seated.west, key(1)).expect("the boat is back");
    assert_eq!(boat.hops, left_hops + 1);
    let state = seated.west.player_state(player(1)).expect("there");
    assert_eq!(
        state.riding.map(|riding| (riding.vehicle, riding.pending)),
        Some((key(1), None)),
        "seated"
    );
}

/// `docs/groundwork/n7h-logout-on-a-mount.md`, section 3: the rider's view goes with
/// their leave, in the same tick, and with nobody else watching the chunks would be let
/// go before the leave is applied; the boat's chunk is kept for the tick, so that the
/// boat goes with them. (It is let go at the end of the tick as an arrival's ticket's
/// chunk is, which the arrivals' tests judge; this boat stands in the home chunk, which
/// stays loaded.)
#[test]
fn a_lone_riders_leave_takes_the_boat_though_their_view_goes_in_the_same_tick() {
    let mut seated = seated();
    let output = seated.west.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(
            EDGE,
            player(1),
            Some(seated.rider),
            None,
        )],
        tickets_removed: around(3)
            .into_iter()
            .map(|chunk| (chunk, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    });
    assert!(
        output.stays.iter().any(|note| matches!(
            note,
            clustine_sim::StayNote::Has { unit: Some(unit), .. } if unit.root().key == key(1)
        )),
        "the boat goes with them: {:?}",
        output.stays
    );
}

// ------------------------------------------------------------------------------------
// Getting on across a line (`docs/groundwork/n7j-mount-across-a-line.md`): the lead's
// test of the whole path; the specification's own cases are written apart.

/// `effect`, filed for its holder.
fn filed(effect: clustine_world::Effect, sequence: u64) -> clustine_world::FiledEffect {
    clustine_world::FiledEffect {
        id: EffectId { sequence, index: 0 },
        effect,
        due: Due::FromTaking,
    }
}

/// Section 8 items 1, 2, 4 and 5: a click on a boat beyond the line grants a hidden
/// seat in its holder, the player's region hands them over to ride it, and the holder
/// seats them as a mount: `Rides` to their client and the seat shown at last.
#[test]
fn a_click_on_a_boat_beyond_the_line_seats_the_player_in_its_holder() {
    let beyond = boat(5, [16.5, -60.0, 8.5]);
    let mut west = west(&[]);
    let mut copy = floor();
    copy.set_entities(vec![beyond.clone()]);
    west.tick(&TickInputs {
        copy_bases: vec![(BEYOND, copy)],
        ..TickInputs::default()
    });
    let rider = joined(&mut west);
    let mut hands = Hands::new();
    let output = west.tick(&TickInputs {
        inputs: hands.inputs(rider, vec![mount(beyond.key)]),
        ..TickInputs::default()
    });
    let used = output
        .effects
        .iter()
        .find(|effect| {
            matches!(effect.what, clustine_world::Effected::OnEntity { key, .. } if key == beyond.key)
        })
        .cloned()
        .expect("the click goes as an effect on the boat");
    // An interaction across a line counts as an entity sent across one: the two regions
    // are reported working at it (step N7n).
    assert!(
        west.entities_at_line()
            .contains(&(BEYOND, Some(EAST_REGION))),
        "{:?}",
        west.entities_at_line()
    );

    let mut east = east(std::slice::from_ref(&beyond));
    let output = east.tick(&TickInputs {
        effects: vec![filed(used, 50)],
        ..TickInputs::default()
    });
    let granted = seats(&east, beyond.key);
    assert_eq!(granted.len(), 1, "one seat granted: {granted:?}");
    assert!(granted[0].pending.is_some_and(|pending| pending.granted));
    assert!(
        passengers_reported(&output, beyond.key)
            .iter()
            .all(|shown| !shown.contains(&rider)),
        "the granted seat is not shown"
    );
    let seated = output
        .effects
        .iter()
        .find(|effect| matches!(effect.what, clustine_world::Effected::Seated { .. }))
        .cloned()
        .expect("the player's region is told");
    assert_eq!(
        seated.at.chunk(),
        ChunkPos::new(0, 0),
        "at where they stood"
    );

    let output = west.tick(&TickInputs {
        effects: vec![filed(seated, 60)],
        ..TickInputs::default()
    });
    let mut sent = departures(&output);
    assert_eq!(sent.len(), 1, "handed over once: {sent:?}");
    let (transfer, to) = sent.remove(0);
    assert_eq!(to, EAST_REGION);
    assert_eq!(
        transfer.riding.map(|riding| riding.vehicle),
        Some(beyond.key)
    );

    let output = east.tick(&TickInputs {
        player_changes: vec![arrive(&transfer)],
        ..TickInputs::default()
    });
    assert_eq!(rides(&output), [Some(beyond.key)], "told they ride");
    assert_eq!(passengers_reported(&output, beyond.key), [vec![rider]]);
    let seats = seats(&east, beyond.key);
    assert_eq!(seats.len(), 1);
    assert_eq!(seats[0].pending, None, "the seat is taken");
}
