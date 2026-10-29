//! Tests of riding in one region and of the vehicles a creative player places (step N5),
//! written from `docs/groundwork/n5-plan.md` (sections 7 and 8 over the rest),
//! `docs/adr/0028-entities.md` (section 5 and its amendment) and the porting brief
//! `docs/groundwork/n5-brief.md` (sections 0 to 5), driving the region by its public
//! interface alone, by someone who did not write the code.
//!
//! What they ask of a region: a player who uses a cushion, a cart or a boat sits on it,
//! a player first where the first passenger is not one and anyone else last, as many as
//! the vehicle takes; their client is told with `Rides` and everyone with `Passengers`;
//! a rider's own moves turn only their head; sneaking (by the keys, or an interact's own
//! flag) gets them off at the vehicle's dismount place; leaving, dying or an edge's reset
//! frees the seat in that very tick, and the stay's note has the place they got off to.
//! The vehicles: a cushion laid on a top face at the click's height facing the player's
//! way, one to a place, broken by one hit and dropped by its survival check; a cart
//! placed on rails only, moved by `OldMinecartBehavior` (slowdown, a rider's push under a
//! tenth, powered rails' push and brake, slopes, carts pushing carts), discarded by a
//! creative hit; a boat placed where the look meets water or land, floating, driven by
//! its client's moves through the game's check (accepted at the client's place, refused
//! when too quick or wrong, the kick after 160 ticks in the air), paddles shown, bubble
//! columns' push and ejection.
//!
//! The numbers a test expects are worked out here from the brief's formulas, never by
//! calling the code under test. Where a model would have to guess what the brief leaves
//! open (the per-type attachment points the brief has from another project, the ray's
//! float arithmetic), the comparison allows a small tolerance and says so.
//!
//! Player 1 (and 2 and 3 where a test needs them) stands at (0.5, -60, 0.5) on a stone
//! floor at y = -61, with the 5×5 around chunk (0, 0) loaded so that it ticks; every
//! scene lies in chunk (0, 0).

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_sim::api::{
    EntityKind, Face, HOTBAR_SLOTS, Inventory, ItemStack, Keys, PlayerInput, RegionEvent,
    VehicleMove,
};
use clustine_sim::{
    EdgeEvent, EdgeState, Entered, Holdings, PlayerChange, PlayerEvent, PlayerJoin, Region,
    RegionConfig, RegionState, StayNote, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Attached, Biome, BlockPos, Chunk, ChunkArea, ChunkPos, Double, EdgeId, Entity, EntityChange,
    EntityData, EntityId, EntityIds, Float, Hurt, PlayerId, Riding, Seat, Section, Vec3,
};
use uuid::Uuid;

const EDGE: EdgeId = EdgeId(7);
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
const FEET: Vec3 = Vec3::new(0.5, -60.0, 0.5);

/// The top of the stone floor.
const FLOOR: f64 = -60.0;

/// What the hotbar holds: the slots the tests select.
const CART_SLOT: u8 = 0;
const BOAT_SLOT: u8 = 1;
const CUSHION_SLOT: u8 = 2;

/// The player's vehicle attachment (`Avatar.DEFAULT_VEHICLE_ATTACHMENT`, brief 1.5).
const PLAYER_ATTACHMENT: f64 = 0.6;

/// The cushion's passenger point and its height (brief 5.8).
const CUSHION_SEAT: f64 = 0.25;
const CUSHION_HEIGHT: f64 = 0.25;

/// A minecart's passenger point (brief 1.5, from the extraction the brief cites).
const CART_SEAT: f64 = 0.1875;

/// A boat's ride height (`height / 3.0f`, brief 4.6), its width and height.
const BOAT_SEAT: f64 = 0.1875;
const BOAT_WIDTH: f32 = 1.375;
const BOAT_HEIGHT: f64 = 0.5625;

/// What the attachment points' arithmetic and the attachments the brief has from another
/// project leave open: positions of riders are compared within this.
const SEATED: f64 = 1e-6;

// ------------------------------------------------------------------------------------
// The world.

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn config() -> RegionConfig {
    let mut hotbar = [const { None }; HOTBAR_SLOTS];
    hotbar[CART_SLOT as usize] = Some(ItemStack::new(items::MINECART, 1));
    hotbar[BOAT_SLOT as usize] = Some(ItemStack::new(items::OAK_BOAT, 1));
    hotbar[CUSHION_SLOT as usize] = Some(ItemStack::new(items::WHITE_CUSHION, 1));
    RegionConfig {
        spawn: FEET,
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

fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap_or_else(|| panic!("{name} is a block state"))
}

/// An overworld column with a stone floor at y = -61 and air above.
fn floor() -> Chunk {
    let sections = (0..24)
        .map(|_| Section::filled(blocks::AIR, Biome(0)))
        .collect();
    let mut chunk = Chunk::from_sections(-64, sections);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -61, z, blocks::STONE);
        }
    }
    chunk
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The chunk at the origin: the floor, then `blocks` (in the chunk's own coordinates,
/// which at the origin are the world's), then `entities`.
fn origin(blocks: &[(i32, i32, i32, BlockState)], entities: Vec<Entity>) -> Chunk {
    let mut chunk = floor();
    for (x, y, z, block) in blocks {
        chunk.set(*x as usize, *y, *z as usize, *block);
    }
    chunk.set_entities(entities);
    chunk
}

/// A region with players 1 to `players` in the world and the 5×5 around the origin
/// loaded, the origin's chunk being `with`.
struct Scene {
    region: Region,
    next_input: u64,
}

impl Scene {
    fn new(players: u128, with: Chunk) -> Self {
        let mut state = RegionState::new(EntityIds::block(1).expect("block 1"));
        state.edges.insert(
            EDGE,
            EdgeState {
                start: 1,
                ..EdgeState::default()
            },
        );
        let holdings = Holdings {
            held: around(3),
            pinned: vec![ChunkArea::EVERYWHERE],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let mut region = Region::restore(config(), state, holdings);
        region.tick(&TickInputs {
            player_changes: (1..=players)
                .map(|n| {
                    PlayerChange::Join(
                        EDGE,
                        PlayerJoin {
                            player: player(n),
                            name: format!("Rider{n}"),
                            attempt: 7000 + n as u64,
                        },
                    )
                })
                .collect(),
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
        for n in 1..=players {
            assert!(region.player(player(n)).is_some(), "player {n} entered");
        }
        let chunks = around(2);
        region.tick(&TickInputs {
            tickets_added: chunks
                .iter()
                .map(|chunk| (*chunk, Ticket::Viewer))
                .collect(),
            ..TickInputs::default()
        });
        let loaded = chunks
            .iter()
            .map(|position| {
                let chunk = if *position == ORIGIN {
                    with.clone()
                } else {
                    floor()
                };
                (*position, chunk)
            })
            .collect();
        region.tick(&TickInputs {
            chunks_loaded: loaded,
            ..TickInputs::default()
        });
        Self {
            region,
            next_input: 1,
        }
    }

    /// The key of player `n`.
    fn key(&self, n: u128) -> EntityId {
        self.region
            .player(player(n))
            .expect("the player is there")
            .0
    }

    fn feet(&self, n: u128) -> Vec3 {
        self.region
            .player(player(n))
            .expect("the player is there")
            .1
            .position
    }

    /// One tick with the inputs, each of the player it names.
    fn tick(&mut self, inputs: Vec<(u128, PlayerInput)>) -> TickOutput {
        let inputs = inputs
            .into_iter()
            .map(|(n, input)| {
                let number = self.next_input;
                self.next_input += 1;
                (EDGE, player(n), self.key(n), number, input)
            })
            .collect();
        self.region.tick(&TickInputs {
            inputs,
            ..TickInputs::default()
        })
    }

    fn idle(&mut self) -> TickOutput {
        self.region.tick(&TickInputs::default())
    }

    /// The entity as the region has it, if it has it.
    fn entity(&self, key: EntityId) -> Option<Entity> {
        self.region
            .chunk(ORIGIN)
            .expect("the origin is loaded")
            .entities()
            .iter()
            .find(|entity| entity.key == key)
            .cloned()
            .or_else(|| {
                around(2)
                    .into_iter()
                    .filter_map(|chunk| self.region.chunk(chunk))
                    .flat_map(|chunk| chunk.entities().iter())
                    .find(|entity| entity.key == key)
                    .cloned()
            })
    }

    fn passengers(&self, vehicle: EntityId) -> Vec<EntityId> {
        self.entity(vehicle)
            .expect("the vehicle is there")
            .riding
            .passengers
            .iter()
            .map(|seat| seat.key)
            .collect()
    }

    /// What player `n` rides, as the region keeps it.
    fn rides(&self, n: u128) -> Option<EntityId> {
        self.region
            .player_state(player(n))
            .expect("the player is there")
            .riding
            .map(|riding| riding.vehicle)
    }

    /// The entities of `kind` the region has, in the order of their keys.
    fn of_kind(&self, kind: i32) -> Vec<Entity> {
        let mut found: Vec<Entity> = around(2)
            .into_iter()
            .filter_map(|chunk| self.region.chunk(chunk))
            .flat_map(|chunk| chunk.entities().iter())
            .filter(|entity| i32::from(entity.kind) == kind)
            .cloned()
            .collect();
        found.sort_by_key(|entity| entity.key);
        found
    }
}

// ------------------------------------------------------------------------------------
// Entities.

fn key(n: u32) -> EntityId {
    EntityId((5 << 32) + u64::from(n))
}

fn entity(n: u32, kind: i32, position: [f64; 3], data: EntityData) -> Entity {
    Entity {
        key: key(n),
        kind: kind as u16,
        uuid: Uuid::from_u128(9_000 + u128::from(n)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: -1,
        riding: Riding::default(),
        data,
    }
}

fn cushion(n: u32, position: [f64; 3]) -> Entity {
    let block = BlockPos::new(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    );
    entity(
        n,
        entity_types::CUSHION,
        position,
        EntityData::Cushion {
            attached: Attached {
                block,
                direction: 0,
            },
            colour: 0,
        },
    )
}

fn cart_data() -> EntityData {
    EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents: Vec::new(),
        fuel: 0,
        push: [Double(0.0), Double(0.0)],
        fuse: -1,
        enabled: true,
    }
}

/// A plain cart on the rail at the block `rail`, where `MinecartItem` would put it.
fn cart(n: u32, rail: (i32, i32, i32), motion: [f64; 3]) -> Entity {
    Entity {
        motion,
        ..entity(
            n,
            entity_types::MINECART,
            [
                f64::from(rail.0) + 0.5,
                f64::from(rail.1) + 0.0625,
                f64::from(rail.2) + 0.5,
            ],
            cart_data(),
        )
    }
}

/// A boat's data, its status `status` (the ordinals of brief 4.3: 0 in water, 3 on land).
fn boat_data(status: u8) -> EntityData {
    EntityData::Boat {
        hurt: Hurt::default(),
        paddles: [false, false],
        bubble_time: 0,
        out_of_control: Float(0.0),
        turning: Float(0.0),
        status: [status, status],
        contents: Vec::new(),
    }
}

fn boat(n: u32, kind: i32, position: [f64; 3], status: u8) -> Entity {
    entity(n, kind, position, boat_data(status))
}

fn stand(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::ARMOR_STAND,
        position,
        EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new([[Float(0.0); 3]; 6]),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    )
}

// ------------------------------------------------------------------------------------
// Inputs and what comes out.

fn use_entity(vehicle: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: vehicle,
        location: [0.0, 0.1, 0.0],
        sneaking: false,
    }
}

fn keys(sneak: bool, forward: bool) -> PlayerInput {
    PlayerInput::Keys(Keys {
        sneak,
        forward,
        ..Keys::default()
    })
}

fn turn(yaw: f32, pitch: f32) -> PlayerInput {
    PlayerInput::Move {
        position: None,
        rotation: Some((yaw, pitch)),
        on_ground: true,
    }
}

fn drive(position: [f64; 3], yaw: f32) -> PlayerInput {
    PlayerInput::MoveVehicle(VehicleMove {
        position: Vec3::new(position[0], position[1], position[2]),
        yaw,
        pitch: 0.0,
        on_ground: true,
    })
}

/// What `output` told player `n` of riding: `Rides` events, in order.
fn rides_told(output: &TickOutput, n: u128) -> Vec<(Option<EntityId>, Vec3)> {
    output
        .player_events
        .iter()
        .filter_map(|(to, event)| match event {
            PlayerEvent::Rides { vehicle, pose } if *to == player(n) => {
                Some((*vehicle, pose.position))
            }
            _ => None,
        })
        .collect()
}

/// The lists of passengers `output` reported for `vehicle`, in order.
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

fn corrections(output: &TickOutput, n: u128) -> Vec<(Vec3, f32, f32)> {
    output
        .player_events
        .iter()
        .filter_map(|(to, event)| match event {
            PlayerEvent::VehicleCorrected {
                position,
                yaw,
                pitch,
            } if *to == player(n) => Some((*position, *yaw, *pitch)),
            _ => None,
        })
        .collect()
}

fn kicked(output: &TickOutput, n: u128) -> bool {
    output
        .player_events
        .iter()
        .any(|(to, event)| *to == player(n) && matches!(event, PlayerEvent::Kicked { .. }))
}

fn entity_events(output: &TickOutput, entity: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent {
                entity: of, event, ..
            } if *of == entity => Some(*event),
            _ => None,
        })
        .collect()
}

/// The entities a tick made, whole and new.
fn made(output: &TickOutput, kind: i32) -> Vec<Entity> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Whole(entity) if i32::from(entity.kind) == kind => Some(entity.clone()),
            _ => None,
        })
        .collect()
}

fn gone(output: &TickOutput, key: EntityId) -> bool {
    output.events.iter().any(|event| {
        matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key)
            || matches!(event, RegionEvent::EntityRemoved { entity, .. } if *entity == key)
    })
}

fn close(a: Vec3, b: [f64; 3], within: f64) -> bool {
    (a.x - b[0]).abs() <= within && (a.y - b[1]).abs() <= within && (a.z - b[2]).abs() <= within
}

fn assert_at(what: &str, a: Vec3, b: [f64; 3], within: f64) {
    assert!(close(a, b, within), "{what}: at {a:?}, expected {b:?}");
}

/// The game's sine table (`Mth.sin`), worked out here from how the game fills it.
fn table_sin(radians: f32) -> f32 {
    let steps = 10_430.378_350_470_453_f64;
    let index = (f64::from(radians) * steps) as i64 & 0xFFFF;
    (index as f64 * std::f64::consts::TAU / 65_536.0).sin() as f32
}

fn table_cos(radians: f32) -> f32 {
    let steps = 10_430.378_350_470_453_f64;
    let index = (f64::from(radians) * steps + 16_384.0) as i64 & 0xFFFF;
    (index as f64 * std::f64::consts::TAU / 65_536.0).sin() as f32
}

// ------------------------------------------------------------------------------------
// Getting on and off a cushion: the simplest vehicle (plan, N5b; brief 5.8, 1.2 to 1.7).

const CUSHION_AT: [f64; 3] = [2.5, FLOOR, 2.5];

fn on_a_cushion(players: u128) -> (Scene, EntityId) {
    let scene = Scene::new(players, origin(&[], vec![cushion(1, CUSHION_AT)]));
    (scene, key(1))
}

/// Brief 1.2 (`ServerPlayer.startRiding`) and 1.5: a player who uses a cushion is seated
/// at once, at the cushion's position plus its passenger point less their own vehicle
/// point, and their client is told so; the cushion's passengers are reported to all.
#[test]
fn a_player_who_uses_a_cushion_sits_on_it_and_everyone_is_told_who_rides_it() {
    let (mut scene, seat) = on_a_cushion(1);
    let rider = scene.key(1);
    let output = scene.tick(vec![(1, use_entity(seat))]);
    let seated = [
        CUSHION_AT[0],
        CUSHION_AT[1] + CUSHION_SEAT - PLAYER_ATTACHMENT,
        CUSHION_AT[2],
    ];
    let told = rides_told(&output, 1);
    let [(Some(vehicle), at)] = told[..] else {
        panic!("one Rides with the cushion: {told:?}");
    };
    assert_eq!(vehicle, seat);
    assert_at("the seat told", at, seated, SEATED);
    assert_eq!(passengers_reported(&output, seat), vec![vec![rider]]);
    assert_eq!(scene.passengers(seat), vec![rider]);
    assert_eq!(scene.rides(1), Some(seat));
    assert_at("the rider", scene.feet(1), seated, SEATED);
}

/// Brief 5.8 (`Cushion.interact`): a sneaking player's use passes, and seats nobody.
#[test]
fn a_sneaking_player_s_use_of_a_cushion_seats_nobody() {
    let (mut scene, seat) = on_a_cushion(1);
    let output = scene.tick(vec![(
        1,
        PlayerInput::Interact {
            entity: seat,
            location: [0.0, 0.1, 0.0],
            sneaking: true,
        },
    )]);
    assert!(rides_told(&output, 1).is_empty());
    assert!(scene.passengers(seat).is_empty());
    assert_eq!(scene.rides(1), None);
}

/// Brief 0.4 and 2.7: `handleInteract` sets the sneak flag from the packet before
/// anything else, so a player holding the sneak key whose client says the use was made
/// without it is seated, and stays seated: the shift dismount reads that flag.
#[test]
fn an_interact_s_own_flag_decides_and_not_the_sneak_key_held_before() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, keys(true, false))]);
    scene.tick(vec![(1, use_entity(seat))]);
    assert_eq!(scene.rides(1), Some(seat), "seated by an unsneaking use");
    for _ in 0..3 {
        scene.idle();
    }
    assert_eq!(scene.rides(1), Some(seat), "and still seated");
}

/// Brief 5.8: a cushion takes one rider; a second player's use passes.
#[test]
fn a_second_player_cannot_sit_on_a_cushion_someone_sits_on() {
    let (mut scene, seat) = on_a_cushion(2);
    scene.tick(vec![(1, use_entity(seat))]);
    let output = scene.tick(vec![(2, use_entity(seat))]);
    assert!(rides_told(&output, 2).is_empty());
    assert_eq!(scene.passengers(seat), vec![scene.key(1)]);
    assert_eq!(scene.rides(2), None);
}

/// Brief 0.15, 1.4 and 1.7: a rider who sneaks gets off in the next ride tick, placed by
/// `Entity.getDismountLocationForPassenger` (the cushion has no rule of its own): on top
/// of its box. Their client is told, and everyone the cushion's empty list.
#[test]
fn a_rider_who_sneaks_gets_off_on_top_of_the_cushion() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    let mut output = scene.tick(vec![(1, keys(true, false))]);
    if rides_told(&output, 1).is_empty() {
        // The keys may come after the tick's ride: the dismount is then the next one's.
        output = scene.idle();
    }
    let on_top = [CUSHION_AT[0], CUSHION_AT[1] + CUSHION_HEIGHT, CUSHION_AT[2]];
    let told = rides_told(&output, 1);
    let [(None, at)] = told[..] else {
        panic!("one Rides with nothing: {told:?}");
    };
    assert_at("the place told", at, on_top, 1e-9);
    assert_eq!(
        passengers_reported(&output, seat),
        vec![Vec::<EntityId>::new()]
    );
    assert!(scene.passengers(seat).is_empty());
    assert_eq!(scene.rides(1), None);
    assert_at("the player", scene.feet(1), on_top, 1e-9);
}

/// Brief 0.4: an interact's sneaking flag is the player's sneak flag, which the ride's
/// tick reads: a rider whose client says they sneak as they use something gets off.
#[test]
fn a_rider_whose_interact_says_they_sneak_gets_off() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    scene.tick(vec![(
        1,
        PlayerInput::Interact {
            entity: seat,
            location: [0.0, 0.1, 0.0],
            sneaking: true,
        },
    )]);
    scene.idle();
    assert_eq!(scene.rides(1), None);
    assert!(scene.passengers(seat).is_empty());
}

/// Brief 0.14 and 2.5: a riding player's own moves take only their rotation; where they
/// are is their vehicle's to say.
#[test]
fn a_riding_player_s_moves_turn_their_head_and_move_them_nowhere() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    let before = scene.feet(1);
    scene.tick(vec![(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(9.5, -60.0, 9.5)),
            rotation: Some((30.0, 10.0)),
            on_ground: true,
        },
    )]);
    scene.idle();
    let (_, pose) = scene.region.player(player(1)).expect("there");
    assert_eq!(pose.position, before, "the rider did not move");
    assert_eq!((pose.yaw, pose.pitch), (30.0, 10.0), "but turned");
    assert_eq!(scene.rides(1), Some(seat));
}

/// Plan, section 7 item 6 and section 8 item 6, as `docs/groundwork/n7h-logout-on-a-mount.md`
/// section 3 narrows them: a player who leaves alone on a vehicle takes it out of the
/// world in the commit that removes them, as the game's `PlayerList.remove` does: it is
/// gone from its chunk, and the stay's note carries it, its seat still theirs.
#[test]
fn a_rider_who_leaves_alone_takes_the_seat_out_of_the_world_in_that_tick() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    let rider = scene.key(1);
    let output = scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(EDGE, player(1), Some(rider), None)],
        ..TickInputs::default()
    });
    assert!(scene.region.player(player(1)).is_none(), "gone");
    assert!(
        output
            .entity_changes
            .iter()
            .any(|(_, change)| matches!(change, EntityChange::Gone(key) if *key == seat)),
        "the seat gone in the commit: {:?}",
        output.entity_changes
    );
    let unit = output.stays.iter().find_map(|note| match note {
        StayNote::Has {
            unit: Some(unit), ..
        } => Some(unit.clone()),
        _ => None,
    });
    let unit = unit.expect("the note carries the unit");
    assert_eq!(unit.root().key, seat);
    assert!(
        unit.root()
            .riding
            .passengers
            .iter()
            .any(|kept| kept.key == rider && kept.player),
        "the seat is theirs still"
    );
}

/// Plan, section 8 item 6: "the commit that removes a rider writes a `Has` note with the
/// place they are dismounted to", so that they come back beside the vehicle.
#[test]
fn a_leaving_rider_s_stay_note_has_the_place_they_are_dismounted_to() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    let rider = scene.key(1);
    let output = scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(EDGE, player(1), Some(rider), None)],
        ..TickInputs::default()
    });
    let on_top = [CUSHION_AT[0], CUSHION_AT[1] + CUSHION_HEIGHT, CUSHION_AT[2]];
    let notes: Vec<&StayNote> = output
        .stays
        .iter()
        .filter(|note| matches!(note, StayNote::Has { player: of, .. } if *of == player(1)))
        .collect();
    let [StayNote::Has { place, entity, .. }] = notes[..] else {
        panic!("one Has note for the leaving rider: {:?}", output.stays);
    };
    assert_eq!(*entity, rider);
    assert_at("the note's place", place.pose.position, on_top, 1e-9);
}

/// Plan, section 7 item 6: a stay the store says is dead leaves its seat in that tick.
#[test]
fn a_rider_whose_stay_is_dead_frees_the_seat_in_that_tick() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    let rider = scene.key(1);
    let hops = scene.region.player_state(player(1)).expect("there").hops;
    let output = scene.region.tick(&TickInputs {
        dead: vec![(player(1), rider, hops + 1)],
        ..TickInputs::default()
    });
    assert!(
        scene.region.player(player(1)).is_none(),
        "the stay is ended"
    );
    assert_eq!(
        passengers_reported(&output, seat),
        vec![Vec::<EntityId>::new()]
    );
    assert!(scene.passengers(seat).is_empty());
}

/// Plan, section 7 item 6, as the logout design's section 8 row 6 has it: an edge's
/// reset is a leave, and its lone rider takes their seat with them too.
#[test]
fn a_rider_whose_edge_is_reset_takes_the_seat_with_them() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    let output = scene.region.tick(&TickInputs {
        edges: vec![EdgeEvent::Started {
            edge: EDGE,
            start: 2,
        }],
        ..TickInputs::default()
    });
    assert!(scene.region.player(player(1)).is_none(), "reset");
    assert!(
        output
            .entity_changes
            .iter()
            .any(|(_, change)| matches!(change, EntityChange::Gone(key) if *key == seat))
    );
    assert!(output.stays.iter().any(|note| matches!(
        note,
        StayNote::Has { unit: Some(unit), .. } if unit.root().key == seat
    )));
}

/// Plan, section 7 item 6: a player who joins again while seated is a new stay; the old
/// stay's seat goes with it.
#[test]
fn a_rider_who_joins_again_leaves_no_seat_behind() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(1, use_entity(seat))]);
    let output = scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player: player(1),
                name: "Rider1".to_owned(),
                attempt: 7100,
            },
        )],
        ..TickInputs::default()
    });
    assert_eq!(
        passengers_reported(&output, seat),
        vec![Vec::<EntityId>::new()]
    );
    assert!(scene.passengers(seat).is_empty());
}

// ------------------------------------------------------------------------------------
// Seats: the order and how many (brief 0.9, 1.3, 4.6).

const BOAT_AT: [f64; 3] = [3.5, FLOOR, 3.5];

/// Brief 1.3 (`addPassenger`): on the server a player goes in at index 0 when the first
/// passenger is not a player.
#[test]
fn a_player_goes_first_in_a_boat_whose_first_passenger_is_not_a_player() {
    let mut vehicle = boat(1, entity_types::OAK_BOAT, BOAT_AT, 3);
    let mut sitter = stand(2, [BOAT_AT[0], BOAT_AT[1] + BOAT_SEAT, BOAT_AT[2]]);
    vehicle.riding.passengers = vec![Seat {
        key: key(2),
        player: false,
        pending: None,
    }];
    sitter.riding.vehicle = Some(key(1));
    let mut scene = Scene::new(1, origin(&[], vec![vehicle, sitter]));
    assert_eq!(
        scene.passengers(key(1)),
        vec![key(2)],
        "the stand is aboard"
    );
    let output = scene.tick(vec![(1, use_entity(key(1)))]);
    let rider = scene.key(1);
    assert_eq!(scene.passengers(key(1)), vec![rider, key(2)]);
    assert_eq!(
        passengers_reported(&output, key(1)),
        vec![vec![rider, key(2)]]
    );
}

/// Brief 1.3 and 4.6: a boat takes two, the second player appended behind the first,
/// and a third's use passes.
#[test]
fn a_boat_takes_a_second_player_behind_the_first_and_refuses_a_third() {
    let mut scene = Scene::new(
        3,
        origin(&[], vec![boat(1, entity_types::OAK_BOAT, BOAT_AT, 3)]),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    scene.tick(vec![(2, use_entity(key(1)))]);
    let output = scene.tick(vec![(3, use_entity(key(1)))]);
    assert_eq!(scene.passengers(key(1)), vec![scene.key(1), scene.key(2)]);
    assert!(rides_told(&output, 3).is_empty());
    assert_eq!(scene.rides(3), None);
}

/// Brief 3.12, 4.6 and 4.11: a cart takes one rider, and so does a chest boat.
#[test]
fn a_cart_and_a_chest_boat_each_take_one_rider() {
    let rail = (6, -60, 3);
    let mut scene = Scene::new(
        2,
        origin(
            &[(rail.0, rail.1, rail.2, blocks::RAIL)],
            vec![
                cart(1, rail, [0.0; 3]),
                boat(2, entity_types::OAK_CHEST_BOAT, BOAT_AT, 3),
            ],
        ),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    scene.tick(vec![(2, use_entity(key(1)))]);
    assert_eq!(
        scene.passengers(key(1)),
        vec![scene.key(1)],
        "one in the cart"
    );
    assert_eq!(scene.rides(2), None);
    scene.tick(vec![(2, use_entity(key(2)))]);
    let output = scene.tick(vec![(1, keys(true, false))]);
    let _ = output;
    scene.idle();
    scene.tick(vec![(1, keys(false, false))]);
    scene.tick(vec![(1, use_entity(key(2)))]);
    assert_eq!(
        scene.passengers(key(2)),
        vec![scene.key(2)],
        "one in the chest boat"
    );
    assert_eq!(scene.rides(1), None);
}

/// Brief 4.6: two riders of a boat sit 0.2 ahead of its centre and 0.6 behind, along its
/// length, at its ride height less their own vehicle point; one alone sits at the centre.
#[test]
fn two_riders_of_a_boat_sit_two_tenths_ahead_and_six_tenths_behind_and_one_at_the_centre() {
    let mut scene = Scene::new(
        2,
        origin(&[], vec![boat(1, entity_types::OAK_BOAT, BOAT_AT, 3)]),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    scene.idle();
    let y = BOAT_AT[1] + BOAT_SEAT - PLAYER_ATTACHMENT;
    assert_at("alone", scene.feet(1), [BOAT_AT[0], y, BOAT_AT[2]], SEATED);
    scene.tick(vec![(2, use_entity(key(1)))]);
    scene.idle();
    // The boat faces yaw 0, towards positive z: the turn of the seat's vector by
    // `-yaw` leaves it along z.
    let ahead = f64::from(0.2_f32);
    let behind = f64::from(-0.6_f32);
    assert_at(
        "in front",
        scene.feet(1),
        [BOAT_AT[0], y, BOAT_AT[2] + ahead],
        SEATED,
    );
    assert_at(
        "behind",
        scene.feet(2),
        [BOAT_AT[0], y, BOAT_AT[2] + behind],
        SEATED,
    );
}

/// Brief 4.11: a chest boat's single seat is 0.15 along its length.
#[test]
fn a_chest_boat_s_rider_sits_fifteen_hundredths_along_its_length() {
    let mut scene = Scene::new(
        1,
        origin(&[], vec![boat(1, entity_types::OAK_CHEST_BOAT, BOAT_AT, 3)]),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    scene.idle();
    let y = BOAT_AT[1] + BOAT_SEAT - PLAYER_ATTACHMENT;
    let along = f64::from(0.15_f32);
    assert_at(
        "seated",
        scene.feet(1),
        [BOAT_AT[0], y, BOAT_AT[2] + along],
        SEATED,
    );
}

// ------------------------------------------------------------------------------------
// Dismount places of carts and boats (brief 1.7, 3.11, 4.6).

/// Gets player 1 off whatever they ride by the sneak key, and gives the place their
/// client was told.
fn sneak_off(scene: &mut Scene) -> Vec3 {
    let mut output = scene.tick(vec![(1, keys(true, false))]);
    for _ in 0..2 {
        if !rides_told(&output, 1).is_empty() {
            break;
        }
        output = scene.idle();
    }
    let told = rides_told(&output, 1);
    let [(None, at)] = told[..] else {
        panic!("one Rides with nothing: {told:?}");
    };
    at
}

const RAIL_AT: (i32, i32, i32) = (3, -60, 3);

/// Brief 3.11: a cart at rest facing yaw 0 (`Direction.fromYRot(0)` south, its motion
/// direction west) tries `DismountHelper.offsetsForDirection(west)`, whose first is
/// north: the player stands on the floor in the middle of the block north of the cart's.
#[test]
fn a_rider_gets_off_a_cart_in_the_block_beside_it_square_to_its_direction() {
    let mut scene = Scene::new(
        1,
        origin(
            &[(RAIL_AT.0, RAIL_AT.1, RAIL_AT.2, blocks::RAIL)],
            vec![cart(1, RAIL_AT, [0.0; 3])],
        ),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    assert_eq!(scene.rides(1), Some(key(1)));
    let seated = scene.feet(1);
    let cart_y = FLOOR + 0.0625;
    assert!(
        (seated.y - (cart_y + CART_SEAT - PLAYER_ATTACHMENT)).abs() < 0.07,
        "seated in the cart at {seated:?}"
    );
    let at = sneak_off(&mut scene);
    let north = [
        f64::from(RAIL_AT.0) + 0.5,
        FLOOR,
        f64::from(RAIL_AT.2) - 0.5,
    ];
    assert_at("off the cart", at, north, 1e-9);
}

/// Brief 3.11: the heights are tried for each pose before the next offset is, but the
/// offsets for each height: with the block north walled at the feet and the head, the
/// player stands south of the cart.
#[test]
fn a_rider_whose_first_side_is_walled_gets_off_a_cart_on_its_other_side() {
    let mut scene = Scene::new(
        1,
        origin(
            &[
                (RAIL_AT.0, RAIL_AT.1, RAIL_AT.2, blocks::RAIL),
                (RAIL_AT.0, -60, RAIL_AT.2 - 1, blocks::STONE),
                (RAIL_AT.0, -59, RAIL_AT.2 - 1, blocks::STONE),
            ],
            vec![cart(1, RAIL_AT, [0.0; 3])],
        ),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    assert_eq!(scene.rides(1), Some(key(1)));
    let at = sneak_off(&mut scene);
    let south = [
        f64::from(RAIL_AT.0) + 0.5,
        FLOOR,
        f64::from(RAIL_AT.2) + 1.5,
    ];
    assert_at("off the cart", at, south, 1e-9);
}

/// Brief 4.6: a boat on land puts its rider at its collision escape vector along the
/// rider's yaw, `(w·√2 + 0.6 + 1e-5) / 2` away, on the floor there.
#[test]
fn a_rider_gets_off_a_boat_on_land_along_the_way_they_look_clear_of_its_box() {
    let mut scene = Scene::new(
        1,
        origin(&[], vec![boat(1, entity_types::OAK_BOAT, BOAT_AT, 3)]),
    );
    scene.tick(vec![(1, turn(0.0, 0.0))]);
    scene.tick(vec![(1, use_entity(key(1)))]);
    assert_eq!(scene.rides(1), Some(key(1)));
    let at = sneak_off(&mut scene);
    let square_of_two = 2.0_f64.sqrt() as f32;
    let distance =
        (f64::from(BOAT_WIDTH * square_of_two) + f64::from(0.6_f32) + 9.999_999_747_378_752e-6)
            / 2.0;
    // Yaw 0: `-Mth.sin(0)` is 0, `Mth.cos(0)` is 1, the largest of the two is 1.
    let (s, c) = (-table_sin(0.0), table_cos(0.0));
    let largest = f64::from(s.abs().max(c.abs()));
    let off = [
        BOAT_AT[0] + f64::from(s) * distance / largest,
        FLOOR,
        BOAT_AT[2] + f64::from(c) * distance / largest,
    ];
    assert_at("off the boat", at, off, 1e-9);
}

// ------------------------------------------------------------------------------------
// Cushions placed and broken (brief 5.8).

/// Brief 5.8 (`CushionItem.useOn`): on a block's top face the cushion is laid in the
/// middle of the block above, at the click's height, its yaw the player's facing snapped
/// to a cardinal direction (`Direction.fromYRot(...).toYRot()`), white as the item is;
/// a creative player keeps the item.
#[test]
fn a_cushion_used_on_a_top_face_is_laid_at_the_click_s_height_facing_the_player_s_way() {
    let mut scene = Scene::new(1, origin(&[], Vec::new()));
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: CUSHION_SLOT })]);
    scene.tick(vec![(1, turn(100.0, 30.0))]);
    let output = scene.tick(vec![(
        1,
        PlayerInput::UseItemOn {
            position: BlockPos::new(2, -61, 2),
            face: Face::Top,
            cursor: [0.3, 1.0, 0.7],
            sneaking: false,
            sequence: 1,
        },
    )]);
    let laid = scene.of_kind(entity_types::CUSHION);
    let [cushion] = &laid[..] else {
        panic!("one cushion: {:?}", output.entity_changes);
    };
    assert_eq!(
        cushion.position.map(f64::to_bits),
        [2.5, FLOOR, 2.5].map(f64::to_bits)
    );
    // Yaw 100 is nearest west, whose yaw is 90.
    assert_eq!(cushion.yaw, 90.0);
    let EntityData::Cushion { colour, .. } = cushion.data else {
        panic!("{cushion:?} is not a cushion");
    };
    assert_eq!(colour, 0, "white");
    let held = &scene.region.state().players[&player(1)].inventory.0[CUSHION_SLOT as usize];
    assert_eq!(
        *held,
        Some(ItemStack::new(items::WHITE_CUSHION, 1)),
        "a creative player keeps the item"
    );
}

/// Brief 5.8: only a top face takes a cushion.
#[test]
fn a_cushion_used_on_a_side_face_is_not_laid() {
    let mut scene = Scene::new(1, origin(&[(2, -60, 2, blocks::STONE)], Vec::new()));
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: CUSHION_SLOT })]);
    scene.tick(vec![(
        1,
        PlayerInput::UseItemOn {
            position: BlockPos::new(2, -60, 2),
            face: Face::North,
            cursor: [0.5, 0.5, 0.0],
            sneaking: false,
            sequence: 1,
        },
    )]);
    assert!(scene.of_kind(entity_types::CUSHION).is_empty());
}

/// Brief 5.8: one cushion to a place; another in the box refuses the next.
#[test]
fn a_cushion_is_not_laid_where_another_lies() {
    let mut scene = Scene::new(1, origin(&[], Vec::new()));
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: CUSHION_SLOT })]);
    for sequence in [1, 2] {
        scene.tick(vec![(
            1,
            PlayerInput::UseItemOn {
                position: BlockPos::new(2, -61, 2),
                face: Face::Top,
                cursor: [0.5, 1.0, 0.5],
                sneaking: false,
                sequence,
            },
        )]);
    }
    assert_eq!(scene.of_kind(entity_types::CUSHION).len(), 1);
}

/// Brief 5.8: one hit breaks a cushion, and a creative breaker gets no item.
#[test]
fn one_creative_hit_breaks_a_cushion_and_drops_nothing() {
    let (mut scene, seat) = on_a_cushion(1);
    let output = scene.tick(vec![(1, PlayerInput::Attack { entity: seat })]);
    assert!(scene.entity(seat).is_none(), "broken");
    assert!(gone(&output, seat), "reported gone");
    for _ in 0..3 {
        scene.idle();
    }
    assert!(scene.of_kind(entity_types::ITEM).is_empty(), "no drop");
}

/// Brief 5.8 and 0.33: the survival check runs on the 101st tick after the cushion is
/// made or loaded and every 101 after; one whose floor is gone is dropped then, as an
/// item, since the check names no breaker.
#[test]
fn a_cushion_whose_floor_is_gone_is_dropped_as_an_item_at_its_survival_check() {
    let (mut scene, seat) = on_a_cushion(1);
    scene.tick(vec![(
        1,
        PlayerInput::Dig {
            position: BlockPos::new(2, -61, 2),
            sequence: 1,
        },
    )]);
    assert!(
        scene.region.chunk(ORIGIN).expect("loaded").get(2, -61, 2) == Some(blocks::AIR),
        "the floor broke"
    );
    // Loaded three ticks before the dig's: the check is due at the cushion's 101st tick.
    for _ in 0..95 {
        scene.idle();
    }
    assert!(scene.entity(seat).is_some(), "still there before its check");
    let mut dropped = false;
    for _ in 0..10 {
        let output = scene.idle();
        dropped |= made(&output, entity_types::ITEM).iter().any(|item| {
            matches!(&item.data, EntityData::Item { stack, .. } if stack.item == items::WHITE_CUSHION)
        });
    }
    assert!(scene.entity(seat).is_none(), "gone after its check");
    assert!(dropped, "and dropped as its item");
}

// ------------------------------------------------------------------------------------
// Minecarts (brief 3).

/// Brief 3.1: the cart is made at the rail's centre, 1/16 above it (half a block more on
/// a slope), facing yaw 0 whichever way the player looks; a creative player keeps it.
#[test]
fn a_minecart_used_on_a_rail_is_placed_at_its_centre_facing_yaw_nought() {
    let mut scene = Scene::new(
        1,
        origin(
            &[
                (3, -60, 3, blocks::RAIL),
                (5, -60, 3, state("minecraft:rail[shape=ascending_east]")),
            ],
            Vec::new(),
        ),
    );
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: CART_SLOT })]);
    scene.tick(vec![(1, turn(90.0, 30.0))]);
    for (sequence, x) in [(1, 3), (2, 5)] {
        scene.tick(vec![(
            1,
            PlayerInput::UseItemOn {
                position: BlockPos::new(x, -60, 3),
                face: Face::Top,
                cursor: [0.5, 0.125, 0.5],
                sneaking: false,
                sequence,
            },
        )]);
    }
    let carts = scene.of_kind(entity_types::MINECART);
    let [flat, sloped] = &carts[..] else {
        panic!("two carts: {carts:?}");
    };
    // Compared as placed: their first tick may have moved them already, so the record
    // of the tick that made them is looked at through what moved since. The flat one
    // stands still on a level rail.
    assert_eq!(
        flat.position.map(f64::to_bits),
        [3.5, FLOOR + 0.0625, 3.5].map(f64::to_bits),
        "{flat:?}"
    );
    assert_eq!(flat.yaw, 0.0);
    assert!(
        (sloped.position[0] - 5.5).abs() < 0.2 && sloped.position[1] > FLOOR + 0.0625,
        "on the slope, higher: {sloped:?}"
    );
    let held = &scene.region.state().players[&player(1)].inventory.0[CART_SLOT as usize];
    assert_eq!(*held, Some(ItemStack::new(items::MINECART, 1)));
}

/// Brief 3.1: the cart is made exactly half a block higher on a sloped rail, in the
/// tick of the use.
#[test]
fn a_minecart_used_on_a_sloped_rail_is_made_half_a_block_higher() {
    let mut scene = Scene::new(
        1,
        origin(
            &[(5, -60, 3, state("minecraft:rail[shape=ascending_east]"))],
            Vec::new(),
        ),
    );
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: CART_SLOT })]);
    let output = scene.tick(vec![(
        1,
        PlayerInput::UseItemOn {
            position: BlockPos::new(5, -60, 3),
            face: Face::Top,
            cursor: [0.5, 0.125, 0.5],
            sneaking: false,
            sequence: 1,
        },
    )]);
    let spawned: Vec<Vec3> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state)
                if matches!(state.kind, EntityKind::Other { kind, .. }
                    if i32::from(kind) == entity_types::MINECART) =>
            {
                Some(state.pose.position)
            }
            _ => None,
        })
        .collect();
    let [at] = spawned[..] else {
        panic!("one cart shown: {:?}", output.events);
    };
    assert_at("made", at, [5.5, FLOOR + 0.0625 + 0.5, 3.5], 1e-12);
}

/// Brief 3.1: a cart used on anything but a rail is not placed.
#[test]
fn a_minecart_used_on_stone_is_not_placed() {
    let mut scene = Scene::new(1, origin(&[], Vec::new()));
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: CART_SLOT })]);
    scene.tick(vec![(
        1,
        PlayerInput::UseItemOn {
            position: BlockPos::new(3, -61, 3),
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence: 1,
        },
    )]);
    assert!(scene.of_kind(entity_types::MINECART).is_empty());
}

/// A line of rails of `rail` along z from 1 to 14 at x = 3, with what else `more` says.
fn rails(
    rail: BlockState,
    more: &[(i32, i32, i32, BlockState)],
) -> Vec<(i32, i32, i32, BlockState)> {
    let mut blocks: Vec<_> = (1..15).map(|z| (3, -60, z, rail)).collect();
    blocks.extend_from_slice(more);
    blocks
}

/// What one tick of `OldMinecartBehavior.moveAlongTrack` does to a cart on a level
/// north-south rail at `pos_z`, moving along z (brief 3.6 steps 9 to 18), with no rider's
/// push: the position's z and the motion's z after it. `brake` for an unpowered powered
/// rail, `power` for a powered one; `ridden` scales the move by 0.75 and the slowdown is
/// 0.997 rather than 0.96.
fn along_a_level_rail(
    z: f64,
    vz: f64,
    ridden: bool,
    push: f64,
    brake: bool,
    power: bool,
) -> (f64, f64) {
    let pos_z = z.floor();
    // Step 9: the whole horizontal speed onto the rail's line (`ex` 0, `ez` 2, `len` 2).
    let h = (vz * vz).sqrt().min(2.0);
    let sign = if vz < 0.0 { -1.0 } else { 1.0 };
    let mut v = h * (sign * 2.0) / 2.0;
    // Step 10: the rider's push, under a tenth a tick, which also lifts the brake.
    let mut braking = brake;
    if push != 0.0 && v * v < 0.01 {
        v += push * 0.001;
        braking = false;
    }
    // Step 11.
    if braking {
        if v.abs() < 0.03 {
            v = 0.0;
        } else {
            v *= 0.5;
        }
    }
    // Step 12: on a north-south rail `t` is the offset in the block.
    let snapped = pos_z + (z - pos_z);
    // Step 13.
    let k = if ridden { 0.75 } else { 1.0 };
    let moved = snapped + (k * v).clamp(-0.4, 0.4);
    // Step 15.
    v *= if ridden { 0.997 } else { 0.96 };
    // Step 16 multiplies by `(h + 0) / h`, 1; step 17 keeps the speed along z.
    // Step 18.
    if power {
        let h = (v * v).sqrt();
        if h > 0.01 {
            v += v / h * 0.06;
        }
    }
    (moved, v)
}

fn cart_now(scene: &Scene, cart: EntityId) -> (f64, f64) {
    let entity = scene.entity(cart).expect("the cart is there");
    (entity.position[2], entity.motion[2])
}

/// Steps the cart a tick at a time and checks each against the model, from where the
/// region had it after the tick before.
fn follow(
    scene: &mut Scene,
    cart: EntityId,
    ticks: usize,
    ridden: bool,
    push: f64,
    brake: bool,
    power: bool,
) {
    for tick in 0..ticks {
        let (z, vz) = cart_now(scene, cart);
        let expected = along_a_level_rail(z, vz, ridden, push, brake, power);
        scene.idle();
        let now = cart_now(scene, cart);
        assert_eq!(
            [now.0.to_bits(), now.1.to_bits()],
            [expected.0.to_bits(), expected.1.to_bits()],
            "tick {tick} from z {z}, motion {vz}: {now:?}, expected {expected:?}"
        );
    }
}

/// Brief 3.6 steps 9, 13 and 15: an empty cart on a level rail moves by its motion and
/// keeps 0.96 of it a tick.
#[test]
fn an_empty_cart_on_a_level_rail_moves_by_its_motion_and_keeps_ninety_six_hundredths_of_it() {
    let mut scene = Scene::new(
        1,
        origin(
            &rails(blocks::RAIL, &[]),
            vec![cart(1, (3, -60, 2), [0.0, 0.0, 0.3])],
        ),
    );
    scene.idle();
    follow(&mut scene, key(1), 20, false, 0.0, false, false);
}

/// Brief 0.3 and 3.6 step 10: a rider's forward key pushes a cart moving under a tenth a
/// tick by a thousandth a tick, along their look; the ridden cart moves three quarters
/// of its motion and keeps 0.997 of it.
#[test]
fn a_rider_s_forward_key_starts_a_cart_at_rest_by_a_thousandth_a_tick() {
    let mut scene = Scene::new(
        1,
        origin(
            &rails(blocks::RAIL, &[]),
            vec![cart(1, (3, -60, 2), [0.0; 3])],
        ),
    );
    scene.tick(vec![(1, turn(0.0, 0.0))]);
    scene.tick(vec![(1, use_entity(key(1)))]);
    assert_eq!(scene.rides(1), Some(key(1)));
    scene.tick(vec![(1, keys(false, true))]);
    assert!(cart_now(&scene, key(1)).1 > 0.0, "the push began");
    follow(&mut scene, key(1), 20, true, 1.0, false, false);
}

/// Brief 0.3: "the cart adds it × 0.001 only while it moves under 0.1 a tick".
#[test]
fn a_rider_s_forward_key_does_not_push_a_cart_moving_at_a_tenth_or_more() {
    let mut scene = Scene::new(
        1,
        origin(
            &rails(blocks::RAIL, &[]),
            vec![cart(1, (3, -60, 2), [0.0; 3])],
        ),
    );
    scene.tick(vec![(1, turn(0.0, 0.0))]);
    scene.tick(vec![(1, use_entity(key(1)))]);
    scene.tick(vec![(1, keys(false, true))]);
    // The push comes every tick while the cart is under a tenth and never above: the
    // model carries that rule, so following it past the tenth checks both. A slow cart
    // reaches a tenth in about a hundred ticks.
    let mut fastest: f64 = 0.0;
    for _ in 0..140 {
        follow(&mut scene, key(1), 1, true, 1.0, false, false);
        fastest = fastest.max(cart_now(&scene, key(1)).1);
    }
    assert!(fastest >= 0.1, "it reached a tenth: {fastest}");
    assert!(fastest < 0.1 + 0.002, "and went no faster: {fastest}");
}

/// Brief 3.6 step 11: an unpowered powered rail halves a cart's speed each tick and
/// stops it under 0.03.
#[test]
fn an_unpowered_powered_rail_halves_a_cart_s_speed_and_stops_it() {
    let brake = state("minecraft:powered_rail[powered=false,shape=north_south]");
    let mut scene = Scene::new(
        1,
        origin(
            &rails(brake, &[]),
            vec![cart(1, (3, -60, 2), [0.0, 0.0, 0.3])],
        ),
    );
    scene.idle();
    follow(&mut scene, key(1), 6, false, 0.0, true, false);
    assert_eq!(cart_now(&scene, key(1)).1, 0.0, "stopped");
}

/// Brief 3.6 step 18: a powered rail adds 0.06 a tick along a moving cart's way.
#[test]
fn a_powered_rail_adds_six_hundredths_a_tick_to_a_moving_cart() {
    let power = state("minecraft:powered_rail[powered=true,shape=north_south]");
    let under: Vec<_> = (1..15)
        .map(|z| (3, -61, z, blocks::REDSTONE_BLOCK))
        .collect();
    let mut scene = Scene::new(
        1,
        origin(
            &rails(power, &under),
            vec![cart(1, (3, -60, 2), [0.0, 0.0, 0.05])],
        ),
    );
    scene.idle();
    follow(&mut scene, key(1), 4, false, 0.0, false, true);
}

/// Brief 3.6 step 18: a cart at rest on a powered rail beside a conductor at its north
/// end is kicked south at 0.02.
#[test]
fn a_cart_at_rest_on_a_powered_rail_is_kicked_away_from_a_solid_block() {
    let power = state("minecraft:powered_rail[powered=true,shape=north_south]");
    let scene = Scene::new(
        1,
        origin(
            &[
                (3, -61, 3, blocks::REDSTONE_BLOCK),
                (3, -60, 3, power),
                (3, -60, 2, blocks::STONE),
                (3, -60, 4, blocks::RAIL),
                (3, -60, 5, blocks::RAIL),
            ],
            vec![cart(1, (3, -60, 3), [0.0; 3])],
        ),
    );
    // The cart's first tick is the load's: the kick is in it, before the push of a
    // moving cart could add to it.
    let motion = scene.entity(key(1)).expect("there").motion;
    assert_eq!(motion[0], 0.0);
    assert_eq!(motion[2], 0.02, "south, away from the stone: {motion:?}");
}

/// Brief 3.6 step 7: a slope pushes a cart downhill, `ASCENDING_SOUTH` towards the north.
#[test]
fn a_cart_on_a_slope_rolls_down_it() {
    let mut scene = Scene::new(
        1,
        origin(
            &[
                (3, -60, 1, blocks::RAIL),
                (3, -60, 2, blocks::RAIL),
                (3, -60, 3, blocks::RAIL),
                (3, -60, 4, state("minecraft:rail[shape=ascending_south]")),
                (3, -60, 5, blocks::STONE),
            ],
            vec![Entity {
                position: [3.5, FLOOR + 0.5625, 4.5],
                ..cart(1, (3, -60, 4), [0.0; 3])
            }],
        ),
    );
    for _ in 0..30 {
        scene.idle();
    }
    let rolled = scene.entity(key(1)).expect("there");
    assert!(rolled.position[2] < 4.0, "down the slope: {rolled:?}");
}

/// Brief 3.10: a moving cart that runs into one at rest pushes it on.
#[test]
fn a_moving_cart_pushes_a_cart_at_rest_ahead_of_it() {
    let mut scene = Scene::new(
        1,
        origin(
            &rails(blocks::RAIL, &[]),
            vec![
                cart(1, (3, -60, 2), [0.0, 0.0, 0.3]),
                cart(2, (3, -60, 5), [0.0; 3]),
            ],
        ),
    );
    for _ in 0..30 {
        scene.idle();
    }
    let pushed = scene.entity(key(2)).expect("there");
    assert!(pushed.position[2] > 5.6, "pushed on: {pushed:?}");
}

/// Brief 0.22 and 3.14: a creative player's hit discards a cart: no item.
#[test]
fn a_creative_hit_discards_a_cart_and_drops_nothing() {
    let mut scene = Scene::new(
        1,
        origin(
            &[(3, -60, 3, blocks::RAIL)],
            vec![cart(1, RAIL_AT, [0.0; 3])],
        ),
    );
    let output = scene.tick(vec![(1, PlayerInput::Attack { entity: key(1) })]);
    assert!(scene.entity(key(1)).is_none());
    assert!(gone(&output, key(1)));
    for _ in 0..3 {
        scene.idle();
    }
    assert!(scene.of_kind(entity_types::ITEM).is_empty());
}

/// Brief 1.4 (`setRemoved`): a vehicle broken under its rider lets them off; they are
/// told, and ride nothing.
#[test]
fn a_rider_whose_cart_is_broken_rides_nothing_and_is_told() {
    let mut scene = Scene::new(
        2,
        origin(
            &[(3, -60, 3, blocks::RAIL)],
            vec![cart(1, RAIL_AT, [0.0; 3])],
        ),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    let output = scene.tick(vec![(2, PlayerInput::Attack { entity: key(1) })]);
    assert!(scene.entity(key(1)).is_none());
    assert_eq!(scene.rides(1), None);
    let told = rides_told(&output, 1);
    assert!(matches!(told[..], [(None, _)]), "{told:?}");
}

// ------------------------------------------------------------------------------------
// Boats placed and floating (brief 4.1 to 4.4).

/// The pool: water sources at y = -61 over x and z from 1 to 4, on stone.
fn pool() -> Vec<(i32, i32, i32, BlockState)> {
    let mut blocks = Vec::new();
    for x in 1..5 {
        for z in 1..5 {
            blocks.push((x, -61, z, blocks::WATER));
            blocks.push((x, -62, z, blocks::STONE));
        }
    }
    blocks
}

/// The yaw and pitch that look from player 1's eyes at `target`, as the game's view
/// vector reads them (`-sin(yaw)·cos(pitch)`, `-sin(pitch)`, `cos(yaw)·cos(pitch)`).
fn look_at(feet: Vec3, target: [f64; 3]) -> (f32, f32) {
    let eyes = feet.y + f64::from(1.62_f32);
    let (dx, dy, dz) = (target[0] - feet.x, target[1] - eyes, target[2] - feet.z);
    let yaw = (-dx).atan2(dz).to_degrees();
    let pitch = (-dy).atan2((dx * dx + dz * dz).sqrt()).to_degrees();
    (yaw as f32, pitch as f32)
}

fn place_boat(scene: &mut Scene, target: [f64; 3]) -> TickOutput {
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: BOAT_SLOT })]);
    let rotation = look_at(scene.feet(1), target);
    scene.tick(vec![(
        1,
        PlayerInput::UseItem {
            rotation,
            sneaking: false,
            sequence: 1,
        },
    )])
}

/// Brief 4.1: a boat is placed where the look meets a block's outline or a fluid, with
/// the player's yaw exactly; on land it stands on the floor; a creative player keeps it.
#[test]
fn a_boat_used_looking_at_the_floor_is_placed_there_with_the_player_s_yaw() {
    let mut scene = Scene::new(1, origin(&[], Vec::new()));
    let target = [2.5, FLOOR, 2.5];
    place_boat(&mut scene, target);
    let boats = scene.of_kind(entity_types::OAK_BOAT);
    let [placed] = &boats[..] else {
        panic!("one boat: {boats:?}");
    };
    // Where the ray meets the floor depends on the table's sines: close to the target.
    assert!(
        (placed.position[0] - target[0]).abs() < 0.05
            && (placed.position[2] - target[2]).abs() < 0.05
            && (placed.position[1] - FLOOR).abs() < 1e-6,
        "{placed:?}"
    );
    let (yaw, _) = look_at(FEET, target);
    assert_eq!(placed.yaw, yaw, "the player's yaw, not snapped");
    let held = &scene.region.state().players[&player(1)].inventory.0[BOAT_SLOT as usize];
    assert_eq!(*held, Some(ItemStack::new(items::OAK_BOAT, 1)));
}

/// Brief 4.1: the boat's box may not meet an entity's: one aimed at the player's own
/// feet is refused.
#[test]
fn a_boat_used_looking_at_the_player_s_own_feet_is_not_placed() {
    let mut scene = Scene::new(1, origin(&[], Vec::new()));
    scene.tick(vec![(1, PlayerInput::SelectSlot { slot: BOAT_SLOT })]);
    scene.tick(vec![(
        1,
        PlayerInput::UseItem {
            rotation: (0.0, 90.0),
            sneaking: false,
            sequence: 1,
        },
    )]);
    assert!(scene.of_kind(entity_types::OAK_BOAT).is_empty());
}

/// Brief 4.1 and 4.4: a boat used at water is placed at its surface (a source's height
/// is 8/9 of the block) and floats: `floatBoat`'s buoyancy holds it where
/// `(level - y) / height · (0.04 / 0.65)` meets the gravity of 0.04, 0.65 of its height
/// under the surface.
#[test]
fn a_boat_used_at_water_is_placed_on_its_surface_and_floats_there() {
    let mut scene = Scene::new(1, origin(&pool(), Vec::new()));
    let surface = f64::from(-61.0_f32 + 8.0_f32 / 9.0_f32);
    place_boat(&mut scene, [2.5, surface, 2.5]);
    let boats = scene.of_kind(entity_types::OAK_BOAT);
    let [placed] = &boats[..] else {
        panic!("one boat: {boats:?}");
    };
    assert!(
        placed.position[1] > surface - 0.2 && placed.position[1] < surface + 0.01,
        "on the surface: {placed:?}"
    );
    for _ in 0..200 {
        scene.idle();
    }
    let floating = scene.entity(placed.key).expect("there");
    let rest = surface - 0.65 * BOAT_HEIGHT;
    assert!(
        (floating.position[1] - rest).abs() < 0.01,
        "floats at {} where {rest} is expected: {floating:?}",
        floating.position[1]
    );
}

// ------------------------------------------------------------------------------------
// Driving a boat (brief 0.12, 0.13, 2.2, 2.3, 2.4).

/// Player 1 in the front seat of a boat on land at `BOAT_AT`, with the blocks `more`.
fn driving(players: u128, more: &[(i32, i32, i32, BlockState)]) -> Scene {
    let mut scene = Scene::new(
        players,
        origin(more, vec![boat(1, entity_types::OAK_BOAT, BOAT_AT, 3)]),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    assert_eq!(scene.rides(1), Some(key(1)));
    scene.idle();
    scene
}

fn boat_pose(scene: &Scene) -> ([f64; 3], f32) {
    let boat = scene.entity(key(1)).expect("the boat is there");
    (boat.position, boat.yaw)
}

/// Brief 2.3 step 3.7: an accepted move puts the boat at the client's position and
/// rotation, and the rider follows it in the next tick by `positionRider`.
#[test]
fn a_driver_s_move_puts_the_boat_where_the_client_says_and_the_driver_follows() {
    let mut scene = driving(1, &[]);
    let to = [BOAT_AT[0], FLOOR, BOAT_AT[2] + 0.5];
    let output = scene.tick(vec![(1, drive(to, 12.0))]);
    assert!(
        corrections(&output, 1).is_empty(),
        "the first move is never corrected"
    );
    let (at, yaw) = boat_pose(&scene);
    assert_eq!(at.map(f64::to_bits), to.map(f64::to_bits));
    assert_eq!(yaw, 12.0);
    scene.idle();
    assert_at(
        "the driver",
        scene.feet(1),
        [to[0], to[1] + BOAT_SEAT - PLAYER_ATTACHMENT, to[2]],
        SEATED,
    );
}

/// Brief 2.3 step 3.2: a move more than ten blocks (100 squared beyond the motion) from
/// where the boat was at the end of the player's last tick is refused: the boat is not
/// touched, nor its rotation taken, and the client is sent the boat's own place.
#[test]
fn a_driver_s_move_too_far_is_refused_with_the_boat_s_own_place_and_rotation() {
    let mut scene = driving(1, &[]);
    let output = scene.tick(vec![(
        1,
        drive([BOAT_AT[0], FLOOR, BOAT_AT[2] + 10.5], 45.0),
    )]);
    let sent = corrections(&output, 1);
    let [(position, yaw, pitch)] = sent[..] else {
        panic!("one correction: {sent:?}");
    };
    assert_at("sent back to", position, BOAT_AT, 1e-12);
    assert_eq!((yaw, pitch), (0.0, 0.0), "the boat's own rotation");
    let (at, yaw) = boat_pose(&scene);
    assert_eq!(at.map(f64::to_bits), BOAT_AT.map(f64::to_bits));
    assert_eq!(yaw, 0.0);
}

/// Brief 2.3 steps 3.5 and 3.6: a move into a wall, whose collided end is more than a
/// quarter block short of the client's, from a box that was clear, is refused: the boat
/// goes back where it was with the client's rotation, which the correction carries.
#[test]
fn a_driver_s_move_into_a_wall_is_refused_and_the_boat_keeps_the_client_s_rotation() {
    let wall: Vec<_> = (1..7)
        .flat_map(|x| [(x, -60, 5, blocks::STONE), (x, -59, 5, blocks::STONE)])
        .collect();
    let mut scene = driving(1, &wall);
    let output = scene.tick(vec![(1, drive([BOAT_AT[0], FLOOR, 4.9], 20.0))]);
    let sent = corrections(&output, 1);
    let [(position, yaw, _)] = sent[..] else {
        panic!("one correction: {sent:?}");
    };
    assert_at("sent back to", position, BOAT_AT, 1e-12);
    assert_eq!(yaw, 20.0, "the client's rotation");
    let (at, yaw) = boat_pose(&scene);
    assert_eq!(at.map(f64::to_bits), BOAT_AT.map(f64::to_bits));
    assert_eq!(yaw, 20.0);
}

/// Brief 2.3 step 4: a passenger who does not drive and sends a move is sent the boat's
/// place, and the boat stays.
#[test]
fn a_back_seat_passenger_s_move_is_answered_with_the_boat_s_place() {
    let mut scene = driving(2, &[]);
    scene.tick(vec![(2, use_entity(key(1)))]);
    assert_eq!(scene.rides(2), Some(key(1)));
    for _ in 0..25 {
        scene.idle();
    }
    let output = scene.tick(vec![(2, drive([BOAT_AT[0], FLOOR, BOAT_AT[2] + 3.0], 0.0))]);
    let sent = corrections(&output, 2);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_at("sent", sent[0].0, BOAT_AT, 1e-12);
    assert_eq!(
        boat_pose(&scene).0.map(f64::to_bits),
        BOAT_AT.map(f64::to_bits)
    );
}

/// Brief 0.13 and 2.2 step 7: a driver whose client holds the boat with nothing but air
/// around and below it is put out after `ceil(80 · max(0.08 / 0.04, 1))` = 160 ticks.
/// The first move up starts from the ground (`verticalCollisionBelow` read before the
/// move), so it is the second that leaves the boat floating; each tick's `tickPlayer`
/// counts from then.
#[test]
fn a_driver_who_holds_a_boat_in_the_air_is_put_out_after_a_hundred_and_sixty_ticks() {
    let mut scene = driving(1, &[]);
    let up = [BOAT_AT[0], FLOOR + 2.0, BOAT_AT[2]];
    let first = scene.tick(vec![(1, drive(up, 0.0))]);
    assert!(corrections(&first, 1).is_empty());
    let second = scene.tick(vec![(1, drive(up, 0.0))]);
    assert!(corrections(&second, 1).is_empty());
    assert!(!kicked(&second, 1));
    // `++count > 160`: with the second move's tick counting one, the 161st kicks. One
    // tick either way is left to where in the tick the region counts.
    let mut kicked_after = None;
    for tick in 1..=170 {
        let output = scene.idle();
        if kicked(&output, 1) {
            kicked_after = Some(tick);
            break;
        }
    }
    let after = kicked_after.expect("put out");
    assert!(
        (159..=161).contains(&after),
        "put out {after} ticks after the second move"
    );
}

/// Brief 2.2 step 7: a move on the ground clears the count, so a driver who comes down
/// in time is not put out.
#[test]
fn a_driver_who_brings_the_boat_down_in_time_is_not_put_out() {
    let mut scene = driving(1, &[]);
    let up = [BOAT_AT[0], FLOOR + 2.0, BOAT_AT[2]];
    scene.tick(vec![(1, drive(up, 0.0))]);
    scene.tick(vec![(1, drive(up, 0.0))]);
    for _ in 0..100 {
        assert!(!kicked(&scene.idle(), 1));
    }
    scene.tick(vec![(1, drive(BOAT_AT, 0.0))]);
    scene.tick(vec![(1, drive(BOAT_AT, 0.0))]);
    for _ in 0..120 {
        assert!(!kicked(&scene.idle(), 1));
    }
}

/// Brief 2.4: `paddle_boat` sets the driven boat's paddles, synched data shown to
/// others; a passenger who does not drive moves no paddle.
#[test]
fn the_driver_s_paddles_are_shown_and_a_passenger_s_are_not() {
    let mut scene = driving(2, &[]);
    scene.tick(vec![(2, use_entity(key(1)))]);
    let output = scene.tick(vec![(
        2,
        PlayerInput::PaddleBoat {
            left: true,
            right: true,
        },
    )]);
    // Shown by any report that carries the boat's new data: a change of its data or
    // the tick's report of it with its data.
    let paddles = |output: &TickOutput| -> Vec<[bool; 2]> {
        output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::DataChanged {
                    entity,
                    data: EntityData::Boat { paddles, .. },
                    ..
                } if *entity == key(1) => Some(*paddles),
                RegionEvent::EntityTracked(tracked) if tracked.entity == key(1) => {
                    match &tracked.data {
                        Some(EntityData::Boat { paddles, .. }) => Some(*paddles),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    };
    assert!(paddles(&output).iter().all(|both| *both == [false, false]));
    let output = scene.tick(vec![(
        1,
        PlayerInput::PaddleBoat {
            left: true,
            right: false,
        },
    )]);
    assert_eq!(paddles(&output), vec![[true, false]]);
    let EntityData::Boat { paddles, .. } = scene.entity(key(1)).expect("there").data else {
        panic!("a boat");
    };
    assert_eq!(paddles, [true, false]);
}

// ------------------------------------------------------------------------------------
// Bubble columns (brief 4.7).

/// A 3×3 pool of bubble columns at y = -60 over `below` at y = -61, walled at -60, with
/// air above.
fn columns(drag: bool, below: BlockState) -> Vec<(i32, i32, i32, BlockState)> {
    let column = state(&format!("minecraft:bubble_column[drag={drag}]"));
    let mut blocks = Vec::new();
    for x in 1..6 {
        for z in 1..6 {
            let inside = (2..5).contains(&x) && (2..5).contains(&z);
            if inside {
                blocks.push((x, -61, z, below));
                blocks.push((x, -60, z, column));
            } else {
                blocks.push((x, -60, z, blocks::STONE));
            }
        }
    }
    blocks
}

/// Where a boat floats on a column's water: 0.65 of its height under the surface.
fn on_the_columns() -> [f64; 3] {
    let surface = f64::from(-60.0_f32 + 8.0_f32 / 9.0_f32);
    [3.5, surface - 0.65 * BOAT_HEIGHT, 3.5]
}

/// Brief 4.7: a boat nobody drives above an upward column is thrown up: its bubble time
/// is set to 60 and counted down, and at its end the server sets its motion's y to 0.6
/// and broadcasts event 72.
#[test]
fn a_boat_nobody_drives_above_an_upward_bubble_column_is_thrown_up_after_sixty_ticks() {
    let mut scene = Scene::new(
        1,
        origin(
            &columns(false, blocks::SOUL_SAND),
            vec![boat(1, entity_types::OAK_BOAT, on_the_columns(), 0)],
        ),
    );
    let mut thrown = None;
    for tick in 1..=80 {
        let output = scene.idle();
        if entity_events(&output, key(1)).contains(&72) {
            thrown = Some(tick);
            break;
        }
    }
    let tick = thrown.expect("event 72");
    assert!((58..=63).contains(&tick), "thrown at tick {tick}");
    let boat = scene.entity(key(1)).expect("there");
    assert!(
        (boat.motion[1] - 0.6).abs() < 0.1 || boat.position[1] > on_the_columns()[1] + 0.3,
        "going up: {boat:?}"
    );
    for _ in 0..3 {
        scene.idle();
    }
    let boat = scene.entity(key(1)).expect("there");
    assert!(boat.position[1] > on_the_columns()[1] + 0.5, "up: {boat:?}");
}

/// Brief 4.7: above a downward column, at the end of the bubble time the boat lets its
/// passengers off and broadcasts event 71, whoever drives it.
#[test]
fn a_boat_above_a_downward_bubble_column_lets_its_driver_off_after_sixty_ticks() {
    let mut scene = Scene::new(
        1,
        origin(
            &columns(true, blocks::MAGMA_BLOCK),
            vec![boat(1, entity_types::OAK_BOAT, on_the_columns(), 0)],
        ),
    );
    scene.tick(vec![(1, use_entity(key(1)))]);
    assert_eq!(scene.rides(1), Some(key(1)));
    let mut ejected = None;
    for tick in 1..=80 {
        let output = scene.idle();
        if entity_events(&output, key(1)).contains(&71) {
            assert!(
                matches!(rides_told(&output, 1)[..], [(None, _)]),
                "let off in that tick"
            );
            ejected = Some(tick);
            break;
        }
    }
    let tick = ejected.expect("event 71");
    assert!(tick <= 62, "ejected at tick {tick}");
    assert_eq!(scene.rides(1), None);
    assert!(scene.passengers(key(1)).is_empty());
}
