//! Tests of boats and rafts (step N5f, `docs/groundwork/n5-plan.md`;
//! `docs/groundwork/n5-brief.md`, section 4 and sections 2.2 to 2.4). The numbers each
//! test expects are worked out here from the game's formulas as its bytecode has them
//! (`BoatItem.use`, `AbstractBoat.floatBoat`, `tickBubbleColumn`,
//! `ServerGamePacketListenerImpl.handleMoveVehicle` and `tickPlayer`), written afresh
//! rather than taken from what the region computes.
//!
//! The regions are those of the tests of redstone (`redstone.rs`): player 1 at the
//! spawn, (0.5, -60, 0.5), on a stone floor whose top is y = -60, with the nine chunks
//! within one of chunk (0, 0) ticking.

use clustine_data::{EntityTypeInfo, entity_types, sounds};
use clustine_noise::trig;
use clustine_world::{Double, Entity, EntityChange, EntityData, EntityUse, Float, Hurt, Seat};

use super::redstone::{holding, state, ticking_with};
use super::*;
use crate::api::VehicleMove;
use crate::state::VehicleCheck;

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

fn water() -> BlockState {
    state("minecraft:water[level=0]")
}

fn stone() -> BlockState {
    state("minecraft:stone")
}

fn boat_data(contents: Vec<(u8, ItemStack)>) -> EntityData {
    EntityData::Boat {
        hurt: Hurt::default(),
        paddles: [false; 2],
        bubble_time: 0,
        out_of_control: Float(0.0),
        turning: Float(0.0),
        status: [3; 2],
        contents,
    }
}

/// A boat's data whose status, now and the tick before, is the ordinal `status`.
fn with_status(status: u8) -> EntityData {
    let mut data = boat_data(Vec::new());
    if let EntityData::Boat { status: both, .. } = &mut data {
        *both = [status; 2];
    }
    data
}

/// A boat of the type `kind` at `position`, at rest.
fn boat_of(n: u64, kind: i32, position: [f64; 3]) -> Entity {
    Entity {
        key: key(n),
        kind: kind as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: boat_data(Vec::new()),
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

fn boat(n: u64, position: [f64; 3]) -> Entity {
    boat_of(n, entity_types::OAK_BOAT, position)
}

fn stand(n: u64, position: [f64; 3]) -> Entity {
    Entity {
        kind: entity_types::ARMOR_STAND as u16,
        data: EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new([[Float(0.0); 3]; 6]),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
        ..boat(n, position)
    }
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// Lays `vehicle` with `passenger` riding it.
fn lay_with_passenger(region: &mut Region, vehicle: &Entity, passenger: &Entity) {
    let mut vehicle = vehicle.clone();
    let mut passenger = passenger.clone();
    passenger.riding.vehicle = Some(vehicle.key);
    vehicle.riding.passengers.push(Seat {
        key: passenger.key,
        player: false,
        pending: None,
    });
    let chunk = vehicle.chunk();
    for member in [&vehicle, &passenger] {
        region
            .chunks
            .get_mut(&chunk)
            .unwrap()
            .apply_entity(&EntityChange::Whole(member.clone()));
        region.entity_chunks.insert(member.key, chunk);
    }
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn bits(values: [f64; 3]) -> [u64; 3] {
    values.map(f64::to_bits)
}

fn boats_in(region: &Region) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| matches!(entity.data, EntityData::Boat { .. }))
        .cloned()
        .collect()
}

fn use_item(yaw: f32, pitch: f32) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::UseItem {
            rotation: (yaw, pitch),
            sneaking: false,
            sequence: 1,
        },
    )])
}

fn interact(target: &Entity, sneaking: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: target.key,
            location: [0.0, 0.3, 0.0],
            sneaking,
        },
    )])
}

fn move_vehicle(position: [f64; 3], yaw: f32, pitch: f32, on_ground: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::MoveVehicle(VehicleMove {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw,
            pitch,
            on_ground,
        }),
    )])
}

fn feet(region: &Region) -> [f64; 3] {
    let at = region.players[&player(1)].pose.position;
    [at.x, at.y, at.z]
}

fn check_of(region: &Region) -> VehicleCheck {
    region.players[&player(1)]
        .riding
        .expect("the player rides")
        .check
}

fn corrections(output: &TickOutput) -> Vec<(Vec3, f32, f32)> {
    output
        .player_events
        .iter()
        .filter_map(|(id, event)| match event {
            PlayerEvent::VehicleCorrected {
                position,
                yaw,
                pitch,
            } if *id == player(1) => Some((*position, *yaw, *pitch)),
            _ => None,
        })
        .collect()
}

fn kicks(output: &TickOutput) -> Vec<String> {
    output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::Kicked { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// `Entity.calculateViewVector(xRot, yRot)` with the game's table of sines.
fn view_vector(yaw: f32, pitch: f32) -> [f64; 3] {
    let radians = std::f64::consts::PI as f32 / 180.0;
    let pitch = pitch * radians;
    let yaw = -yaw * radians;
    let yaw_cos = trig::cos(f64::from(yaw));
    let yaw_sin = trig::sin(f64::from(yaw));
    let pitch_cos = trig::cos(f64::from(pitch));
    let pitch_sin = trig::sin(f64::from(pitch));
    [
        f64::from(yaw_sin * pitch_cos),
        f64::from(-pitch_sin),
        f64::from(yaw_cos * pitch_cos),
    ]
}

/// Where the player's ray at `yaw` and `pitch`, from the eyes of one standing at the
/// spawn, reaches the height `y` (`AABB.clip` of the face it enters by there): the eyes
/// 1.62 above the feet, a reach of 5.
fn ray_at_height(yaw: f32, pitch: f32, y: f64) -> [f64; 3] {
    let from = [0.5, -60.0 + 1.62, 0.5];
    let view = view_vector(yaw, pitch);
    let to: [f64; 3] = std::array::from_fn(|axis| from[axis] + view[axis] * 5.0);
    let t = (y - from[1]) / (to[1] - from[1]);
    std::array::from_fn(|axis| from[axis] + (to[axis] - from[axis]) * t)
}

/// The surface of a still water source with air above it at the block height `y`, as
/// the game adds the fluid's height to the block's y, in floats.
fn surface(y: i32) -> f64 {
    f64::from(y as f32 + 8.0_f32 / 9.0_f32)
}

/// A pool of still water one block deep at y = -60 from `x0` to `x1` and `z0` to `z1`.
fn pool(x0: i32, x1: i32, z0: i32, z1: i32) -> Vec<(BlockPos, BlockState)> {
    (x0..=x1)
        .flat_map(|x| (z0..=z1).map(move |z| (BlockPos::new(x, -60, z), water())))
        .collect()
}

// Placing.

/// A boat used at water goes where the ray meets the surface, turned as the player is;
/// it ticks in the same tick, falling its first 0.04 through the air above the water
/// (its status is in the air, its bottom on the surface); the creative player keeps
/// the item.
#[test]
fn a_boat_used_at_water_lands_where_the_ray_meets_the_surface_and_the_item_is_kept() {
    let mut region = ticking_with(&pool(1, 6, -2, 3));
    checked(&mut region, &holding(items::OAK_BOAT));
    let output = checked(&mut region, &use_item(-90.0, 30.0));
    // The ray meets the fluid's shape, as high as the fluid's height widened and added
    // to the block's y in doubles (`FluidState.getShape`).
    let hit = ray_at_height(-90.0, 30.0, -60.0 + f64::from(8.0_f32 / 9.0_f32));
    let placed = boats_in(&region);
    let [placed] = &placed[..] else {
        panic!("one boat: {placed:?}")
    };
    assert_eq!(i32::from(placed.kind), entity_types::OAK_BOAT);
    assert_eq!(placed.yaw, -90.0);
    // Its first tick: the status reads the surface in floats (`checkInWater`), which
    // lies above the double the ray met, so that the boat is in the water and lifted by
    // it, if barely.
    let level = surface(-60);
    assert!(hit[1] < level);
    let lift = (level - hit[1]) / f64::from(0.5625_f32) * (0.04 / 0.65);
    let fall = ((0.0 + -0.04) + lift) * 0.75;
    assert_eq!(bits(placed.position), bits([hit[0], hit[1] + fall, hit[2]]));
    assert_eq!(bits(placed.motion), bits([0.0, fall, 0.0]));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntitySpawned(spawned) if spawned.entity == placed.key
    )));
    let held = region.players[&player(1)].held();
    assert_eq!(held.map(|stack| stack.item), Some(items::OAK_BOAT));
}

/// On land the ray meets the floor's top; the raft placed there sits on it.
#[test]
fn a_raft_used_at_the_floor_stands_on_it() {
    let mut region = ticking_with(&[]);
    checked(&mut region, &holding(items::BAMBOO_RAFT));
    checked(&mut region, &use_item(-90.0, 40.0));
    let hit = ray_at_height(-90.0, 40.0, -60.0);
    let placed = boats_in(&region);
    let [placed] = &placed[..] else {
        panic!("one raft: {placed:?}")
    };
    assert_eq!(i32::from(placed.kind), entity_types::BAMBOO_RAFT);
    assert_eq!(placed.position[0].to_bits(), hit[0].to_bits());
    assert_eq!(placed.position[2].to_bits(), hit[2].to_bits());
    assert_eq!(placed.position[1], -60.0);
    assert!(placed.on_ground);
}

/// A boat is not placed into a block's shape (a wall the ray meets at its side), into
/// another boat, where an entity's box holds the player's eyes, or where the ray meets
/// nothing.
#[test]
fn a_boat_is_refused_against_a_wall_into_a_boat_with_an_entity_at_the_eyes_and_into_the_air() {
    let wall = [
        (BlockPos::new(2, -60, 0), stone()),
        (BlockPos::new(2, -59, 0), stone()),
    ];
    let mut region = ticking_with(&wall);
    checked(&mut region, &holding(items::OAK_BOAT));
    checked(&mut region, &use_item(-90.0, 0.0));
    assert!(boats_in(&region).is_empty());
    checked(&mut region, &use_item(0.0, -90.0));
    assert!(boats_in(&region).is_empty());

    let mut region = ticking_with(&pool(1, 6, -2, 3));
    checked(&mut region, &holding(items::OAK_BOAT));
    checked(&mut region, &use_item(-90.0, 30.0));
    assert_eq!(boats_in(&region).len(), 1);
    checked(&mut region, &use_item(-90.0, 30.0));
    assert_eq!(boats_in(&region).len(), 1);

    let mut region = ticking_with(&pool(1, 6, -2, 3));
    lay(&mut region, &stand(1, [0.5, -60.0, 0.5]));
    checked(&mut region, &holding(items::OAK_BOAT));
    checked(&mut region, &use_item(-90.0, 30.0));
    assert!(boats_in(&region).is_empty());
}

// Riding and driving.

/// A player who uses a boat without sneaking gets on, sits at its seat (a single
/// passenger at the boat's middle, a third of its height up, less the player's own
/// vehicle point) and drives it: the end of the tick has their moves checked from where
/// it is. One who sneaks does not get on.
#[test]
fn a_player_who_uses_a_boat_gets_on_and_drives_it_from_its_first_seat() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [3.5, -60.0, 0.5]);
    lay(&mut region, &target);
    checked(&mut region, &interact(&target, true));
    assert!(region.players[&player(1)].riding.is_none());
    let output = checked(&mut region, &interact(&target, false));
    let seats = record(&region, target.key).unwrap().riding.passengers;
    assert_eq!(seats.len(), 1);
    assert!(seats[0].player && seats[0].key == EntityId(1));
    let attach = EntityTypeInfo::of(entity_types::PLAYER).vehicle_points[0];
    let height = f64::from(0.5625_f32 / 3.0);
    assert_eq!(
        bits(feet(&region)),
        bits([3.5, (-60.0 + height) - attach[1], 0.5])
    );
    assert!(output.player_events.iter().any(|(id, event)| {
        *id == player(1)
            && matches!(event, PlayerEvent::Rides { vehicle: Some(vehicle), .. } if *vehicle == target.key)
    }));
    let at = record(&region, target.key).unwrap().position.map(Double);
    assert_eq!(check_of(&region).good, Some([at, at]));
}

/// A raft seats its rider at eight ninths of its height (half a block), a chest boat
/// its one rider 0.15 ahead of its middle, turned with it.
#[test]
fn a_raft_and_a_chest_boat_seat_their_riders_where_their_kinds_say() {
    let attach = EntityTypeInfo::of(entity_types::PLAYER).vehicle_points[0];
    let mut region = ticking_with(&[]);
    let raft = boat_of(1, entity_types::BAMBOO_RAFT, [3.5, -60.0, 0.5]);
    lay(&mut region, &raft);
    checked(&mut region, &interact(&raft, false));
    let height = f64::from(0.5625_f32 * 0.888_888_9);
    assert_eq!(
        bits(feet(&region)),
        bits([3.5, (-60.0 + height) - attach[1], 0.5])
    );

    let mut region = ticking_with(&[]);
    let chest = Entity {
        yaw: 90.0,
        ..boat_of(2, entity_types::OAK_CHEST_BOAT, [3.5, -60.0, 0.5])
    };
    lay(&mut region, &chest);
    checked(&mut region, &interact(&chest, false));
    // `Vec3(0, h, 0.15).yRot(-yaw * DEG_TO_RAD)`.
    let angle = -90.0_f32 * 0.017_453_292;
    let (c, s) = (
        f64::from(trig::cos(f64::from(angle))),
        f64::from(trig::sin(f64::from(angle))),
    );
    let along = f64::from(0.15_f32);
    let point = [
        0.0 * c + along * s,
        f64::from(0.5625_f32 / 3.0),
        along * c - 0.0 * s,
    ];
    assert_eq!(
        bits(feet(&region)),
        bits([
            (3.5 + point[0]) - attach[0],
            (-60.0 + point[1]) - attach[1],
            (0.5 + point[2]) - attach[2],
        ])
    );
}

/// A boat the player drives is put where their client says, turned as it says, and
/// their seat follows it in the same tick; the server neither moves nor floats it.
#[test]
fn a_driven_boat_is_put_where_the_drivers_client_says_and_the_rider_follows() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [3.5, -60.0, 0.5]);
    lay(&mut region, &target);
    checked(&mut region, &interact(&target, false));
    let to = [4.5, -60.0, 1.25];
    let output = checked(&mut region, &move_vehicle(to, 30.0, 5.0, true));
    assert!(corrections(&output).is_empty());
    let moved = record(&region, target.key).unwrap();
    assert_eq!(bits(moved.position), bits(to));
    assert_eq!((moved.yaw, moved.pitch), (30.0, 5.0));
    assert_eq!(bits(moved.motion), bits([0.0; 3]));
    assert!(moved.on_ground);
    let attach = EntityTypeInfo::of(entity_types::PLAYER).vehicle_points[0];
    let height = f64::from(0.5625_f32 / 3.0);
    let feet_now = feet(&region);
    assert_eq!(
        feet_now[1].to_bits(),
        ((-60.0 + height) - attach[1]).to_bits()
    );
    assert_eq!(feet_now[0].to_bits(), to[0].to_bits());
    // The next moves count from where it is now.
    let at = to.map(Double);
    assert_eq!(check_of(&region).good, Some([at, at]));
    // Idle, a driven boat stays where its driver put it.
    idle(&mut region);
    assert_eq!(
        bits(record(&region, target.key).unwrap().position),
        bits(to)
    );
}

/// A move further than ten blocks beyond the boat's motion from where it was is
/// refused, the boat untouched, and the client told where it is; so is a move through
/// a wall, the boat turned as the client turned it.
#[test]
fn a_move_too_quick_or_through_a_wall_is_refused_and_the_driver_is_corrected() {
    let mut region = ticking_with(&[(BlockPos::new(5, -60, 0), stone())]);
    let target = boat(1, [3.5, -60.0, 0.5]);
    lay(&mut region, &target);
    checked(&mut region, &interact(&target, false));
    let start = record(&region, target.key).unwrap();
    let output = checked(
        &mut region,
        &move_vehicle([3.5, -60.0, 11.6], 0.0, 0.0, true),
    );
    assert_eq!(
        corrections(&output),
        [(Vec3::new(3.5, -60.0, 0.5), start.yaw, start.pitch)]
    );
    assert_eq!(
        bits(record(&region, target.key).unwrap().position),
        bits(start.position)
    );

    // Through the stone at x = 5: the server's own move stops the box at its face, a
    // whole block short, and the old box was clear.
    let output = checked(
        &mut region,
        &move_vehicle([6.5, -60.0, 0.5], 370.0, 0.0, true),
    );
    // `wrapDegrees(370)`, then `% 360`.
    assert_eq!(
        corrections(&output),
        [(Vec3::new(3.5, -60.0, 0.5), 10.0, 0.0)]
    );
    let after = record(&region, target.key).unwrap();
    assert_eq!(bits(after.position), bits(start.position));
    assert_eq!(after.yaw, 10.0);
}

/// A boat its driver holds in the air puts them out once their ticks of holding it
/// pass 160 (`ceil(80 * max(0.08 / 0.04, 1))`): a move up from the ground does not
/// count, as the boat stood on something before it; from the next move on each tick
/// counts, with or without a move in it.
#[test]
fn a_driver_who_holds_a_boat_in_the_air_is_put_out_after_160_ticks() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [3.5, -60.0, 0.5]);
    lay(&mut region, &target);
    // A tick of its own on the floor, which it lands on.
    idle(&mut region);
    checked(&mut region, &interact(&target, false));
    let up = [3.5, -56.0, 0.5];
    let output = checked(&mut region, &move_vehicle(up, 0.0, 0.0, false));
    assert!(corrections(&output).is_empty());
    assert!(!check_of(&region).floating);
    let output = checked(&mut region, &move_vehicle(up, 0.0, 0.0, false));
    assert!(kicks(&output).is_empty());
    assert!(check_of(&region).floating);
    assert_eq!(check_of(&region).floating_ticks, 1);
    for count in 2..=160 {
        let output = idle(&mut region);
        assert!(kicks(&output).is_empty(), "put out at {count}");
    }
    let output = idle(&mut region);
    assert_eq!(kicks(&output), ["multiplayer.disconnect.flying"]);
}

/// A move whose numbers are not numbers puts the player out at once.
#[test]
fn a_vehicle_move_that_is_not_a_number_puts_the_player_out() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [3.5, -60.0, 0.5]);
    lay(&mut region, &target);
    checked(&mut region, &interact(&target, false));
    let output = checked(
        &mut region,
        &move_vehicle([f64::NAN, -60.0, 0.5], 0.0, 0.0, true),
    );
    assert_eq!(
        kicks(&output),
        ["multiplayer.disconnect.invalid_vehicle_movement"]
    );
}

/// The paddles a driver's client turns are the boat's, committed and shown to everyone;
/// the paddle sounds on land in the second and third ticks of a stroke, then not until
/// the stroke comes round again.
#[test]
fn the_paddles_a_driver_turns_are_shown_and_sound_at_the_start_of_each_stroke() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [3.5, -60.0, 0.5]);
    lay(&mut region, &target);
    checked(&mut region, &interact(&target, false));
    let paddling = moves(vec![numbered(
        1,
        PlayerInput::PaddleBoat {
            left: true,
            right: false,
        },
    )]);
    let paddle_sounds = |output: &TickOutput| {
        output
            .events
            .iter()
            .filter(|event| {
                matches!(event, RegionEvent::Sound { sound, .. }
                    if *sound == sounds::ENTITY_BOAT_PADDLE_LAND)
            })
            .count()
    };
    let output = checked(&mut region, &paddling);
    let EntityData::Boat { paddles, .. } = record(&region, target.key).unwrap().data else {
        unreachable!()
    };
    assert_eq!(paddles, [true, false]);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityTracked(report)
            if report.entity == target.key
                && matches!(report.data, Some(EntityData::Boat { paddles: [true, false], .. }))
    )));
    assert!(output.entity_changes.iter().any(|(_, change)| matches!(
        change,
        EntityChange::Set(changed, _) if *changed == target.key
    )));
    // The stroke as the game counts it, in floats: a sound where the paddle is within
    // the first eighth of a turn of its stroke and the next step reaches past it.
    // `0.3926991f`, which is the float of `π / 8`.
    let step = std::f32::consts::FRAC_PI_8;
    assert_eq!(
        step.to_bits(),
        "0.3926991".parse::<f32>().unwrap().to_bits()
    );
    let stroke = 6.283_185_5_f32;
    let window = 0.785_398_185_253_143_3_f64;
    let mut at = 0.0_f32;
    let mut expected = Vec::new();
    for _ in 0..40 {
        let sounds = f64::from(at % stroke) <= window && f64::from((at + step) % stroke) >= window;
        expected.push(usize::from(sounds));
        at += step;
    }
    // Counting the ticks of the stroke from 0: the second and third, then the
    // seventeenth and thirty-third (the brief's 4.2, step 9).
    let at_ticks: Vec<usize> = (0..40).filter(|tick| expected[*tick] == 1).collect();
    assert_eq!(at_ticks, [1, 2, 17, 33]);
    let mut heard = vec![paddle_sounds(&output)];
    for _ in 1..40 {
        heard.push(paddle_sounds(&idle(&mut region)));
    }
    assert_eq!(heard, expected);
}

// A boat nobody drives.

/// A boat whose bottom is in still water floats up towards where the water's lift and
/// its gravity meet: `(v.y - 0.04 + (level - y) / 0.5625 * (0.04 / 0.65)) * 0.75`
/// each tick, its sideways motion dragged by nine tenths, and no current.
#[test]
fn a_boat_nobody_drives_floats_up_in_still_water_by_the_games_lift() {
    let mut region = ticking_with(&pool(4, 12, -4, 4));
    let start = [8.5, -59.5, 0.5];
    let floating = Entity {
        data: with_status(0),
        ..boat(1, start)
    };
    lay(&mut region, &floating);
    let level = surface(-60);
    let height = f64::from(0.5625_f32);
    let mut y = start[1];
    let mut v = [0.0_f64; 3];
    for _ in 0..20 {
        idle(&mut region);
        let buoyancy = (level - y) / height;
        let falling = v[1] + -0.04;
        v = [
            v[0] * f64::from(0.9_f32),
            (falling + buoyancy * (0.04 / 0.65)) * 0.75,
            v[2] * f64::from(0.9_f32),
        ];
        y += v[1];
        let now = record(&region, floating.key).unwrap();
        assert_eq!(bits(now.position), bits([8.5, y, 0.5]), "{now:?}");
        assert_eq!(bits(now.motion), bits(v));
    }
    assert!(y > start[1]);
}

/// A boat on water flowing along x only is carried by it: the current, made one long
/// and scaled to 0.014, then the water's drag of nine tenths.
#[test]
fn a_boat_nobody_drives_drifts_with_a_current() {
    let mut blocks = Vec::new();
    for x in 4..=11 {
        for z in -1..=1 {
            let level = x - 4;
            blocks.push((
                BlockPos::new(x, -60, z),
                state(&format!("minecraft:water[level={level}]")),
            ));
        }
    }
    for x in 3..=12 {
        blocks.push((BlockPos::new(x, -60, -2), stone()));
        blocks.push((BlockPos::new(x, -60, 2), stone()));
    }
    let mut region = ticking_with(&blocks);
    let drifting = Entity {
        data: with_status(0),
        ..boat(1, [7.5, -59.6, 0.5])
    };
    lay(&mut region, &drifting);
    idle(&mut region);
    let now = record(&region, drifting.key).unwrap();
    let carried = 0.014 * f64::from(0.9_f32);
    assert_eq!(now.motion[0].to_bits(), carried.to_bits());
    assert_eq!(now.position[0].to_bits(), (7.5 + carried).to_bits());
    assert_eq!(now.motion[2], 0.0);
    for _ in 0..10 {
        idle(&mut region);
    }
    assert!(record(&region, drifting.key).unwrap().position[0] > 7.5 + carried);
}

/// A boat over an upward bubble column counts its bubble time down from 60, and in the
/// sixtieth tick above it is thrown up at 0.6 a tick.
#[test]
fn a_boat_over_a_bubble_column_is_thrown_up_in_its_sixtieth_tick_above_it() {
    let column = state("minecraft:bubble_column[drag=false]");
    let blocks: Vec<_> = (4..=12)
        .flat_map(|x| (-4..=4).map(move |z| (BlockPos::new(x, -60, z), column)))
        .collect();
    let mut region = ticking_with(&blocks);
    let floating = Entity {
        data: with_status(0),
        ..boat(1, [8.5, -59.5, 0.5])
    };
    lay(&mut region, &floating);
    for tick in 1..60 {
        idle(&mut region);
        let EntityData::Boat { bubble_time, .. } = record(&region, floating.key).unwrap().data
        else {
            unreachable!()
        };
        assert_eq!(bubble_time, 60 - tick);
    }
    idle(&mut region);
    let thrown = record(&region, floating.key).unwrap();
    let EntityData::Boat { bubble_time, .. } = thrown.data else {
        unreachable!()
    };
    assert_eq!(bubble_time, 0);
    assert_eq!(bits(thrown.motion), bits([0.0, 0.6, 0.0]));
    idle(&mut region);
    assert!(record(&region, floating.key).unwrap().position[1] > thrown.position[1]);
}

/// Over a downward column the sixtieth tick lets the passengers off and pulls the boat
/// down by 0.7.
#[test]
fn a_downward_bubble_column_lets_a_boats_passengers_off_and_pulls_it_down() {
    let column = state("minecraft:bubble_column[drag=true]");
    let blocks: Vec<_> = (4..=12)
        .flat_map(|x| (-4..=4).map(move |z| (BlockPos::new(x, -60, z), column)))
        .collect();
    let mut region = ticking_with(&blocks);
    let floating = Entity {
        data: with_status(0),
        ..boat(1, [8.5, -59.5, 0.5])
    };
    let rider = stand(2, [8.5, -59.3, 0.5]);
    lay_with_passenger(&mut region, &floating, &rider);
    for _ in 1..60 {
        idle(&mut region);
        assert_eq!(
            record(&region, rider.key).unwrap().riding.vehicle,
            Some(floating.key)
        );
    }
    let output = checked(&mut region, &TickInputs::default());
    assert_eq!(record(&region, rider.key).unwrap().riding.vehicle, None);
    assert!(record(&region, floating.key).unwrap().motion[1] < -0.6);
    // Its driver's client, which moves a driven boat, is told of the pull.
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityEvent { entity, event: 71, .. } if *entity == floating.key
    )));
}

/// A boat under water for sixty ticks lets its passengers off; in the fifty-ninth it
/// still carries them.
#[test]
fn a_boat_sixty_ticks_under_water_lets_its_passengers_off() {
    let mut blocks = pool(4, 12, -4, 4);
    blocks
        .extend((4..=12).flat_map(|x| (-4..=4).map(move |z| (BlockPos::new(x, -59, z), water()))));
    let mut region = ticking_with(&blocks);
    let sunk = boat(1, [8.5, -60.0, 0.5]);
    let rider = stand(2, [8.5, -59.8, 0.5]);
    lay_with_passenger(&mut region, &sunk, &rider);
    for tick in 1..60 {
        idle(&mut region);
        assert_eq!(
            record(&region, rider.key).unwrap().riding.vehicle,
            Some(sunk.key),
            "let off at {tick}"
        );
        let EntityData::Boat { out_of_control, .. } = record(&region, sunk.key).unwrap().data
        else {
            unreachable!()
        };
        assert_eq!(out_of_control, Float(tick as f32));
    }
    idle(&mut region);
    assert_eq!(record(&region, rider.key).unwrap().riding.vehicle, None);
    assert!(
        record(&region, sunk.key)
            .unwrap()
            .riding
            .passengers
            .is_empty()
    );
}

// Carts and boats.

/// A cart that meets a boat while it carries something pushes it as `AbstractBoat.push`
/// says: a twentieth of the way apart along the larger of x and z, made of length one
/// over the square root of it; the cart, carrying nobody, is pushed back the other way.
#[test]
fn a_cart_pushes_a_boat_away_and_is_pushed_back_by_the_games_push() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [5.0, -60.0, 0.5]);
    lay(&mut region, &target);
    let cart_at = [4.2, -60.0, 0.3];
    let cart_box = crate::physics::aabb::Aabb::of_entity(cart_at, 0.98, 0.7);
    let mut output = TickOutput::default();
    let record_before = record(&region, target.key).unwrap();
    let back = region.boat_pushed_by_cart(
        &record_before,
        ChunkPos::new(0, 0),
        key(9),
        cart_at,
        &cart_box,
        &mut output,
    );
    // `Entity.push`, from the boat's side: the cart is `other`.
    let (mut dx, mut dz) = (cart_at[0] - 5.0, cart_at[2] - 0.5);
    let mut d = dx.abs().max(dz.abs());
    d = d.sqrt();
    dx /= d;
    dz /= d;
    let k = (1.0 / d).min(1.0);
    dx *= k;
    dz *= k;
    dx *= 0.050_000_000_745_058_06;
    dz *= 0.050_000_000_745_058_06;
    assert_eq!(back.map(bits), Some(bits([dx, 0.0, dz])));
    assert_eq!(
        bits(record(&region, target.key).unwrap().motion),
        bits([-dx, 0.0, -dz])
    );
    // A cart higher than the boat's bottom does not push it.
    let above = crate::physics::aabb::Aabb::of_entity([4.2, -59.9, 0.3], 0.98, 0.7);
    let none = region.boat_pushed_by_cart(
        &record(&region, target.key).unwrap(),
        ChunkPos::new(0, 0),
        key(9),
        [4.2, -59.9, 0.3],
        &above,
        &mut output,
    );
    assert_eq!(none, None);
}

/// A cart rolling into a boat while it carries a stand pushes the boat along; the boat
/// rides nothing.
#[test]
fn a_cart_carrying_a_rider_pushes_a_boat_it_meets_along() {
    let rail = state("minecraft:rail[shape=east_west,waterlogged=false]");
    let slab = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let mut blocks: Vec<_> = (2..=14).map(|x| (BlockPos::new(x, -60, 8), rail)).collect();
    // The boat stands on slabs either side of the rails, its bottom between the cart's
    // and the cart's top, so that the cart's bottom is no higher than its own.
    for x in 4..=7 {
        blocks.push((BlockPos::new(x, -60, 7), slab));
        blocks.push((BlockPos::new(x, -60, 9), slab));
    }
    let mut region = ticking_with(&blocks);
    let cart = Entity {
        kind: entity_types::MINECART as u16,
        motion: [0.2, 0.0, 0.0],
        data: EntityData::Minecart {
            hurt: Hurt::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents: Vec::new(),
            fuel: 0,
            push: [Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
        ..boat(1, [3.5, -60.0 + 0.0625, 8.5])
    };
    lay_with_passenger(&mut region, &cart, &stand(2, [3.5, -59.8, 8.5]));
    let target = boat(3, [5.4, -59.5, 8.5]);
    lay(&mut region, &target);
    let mut pushed = false;
    for _ in 0..8 {
        idle(&mut region);
        let now = record(&region, target.key).unwrap();
        assert_eq!(now.riding.vehicle, None);
        pushed |= now.motion[0] > 0.0;
    }
    assert!(pushed);
}

// Breaking.

/// A hit of more than 40 damage breaks a boat, which drops its own item where it was; a
/// creative hit on a chest boat spills what it holds and drops no boat.
#[test]
fn a_broken_boat_drops_itself_and_a_chest_boat_broken_in_creative_spills_its_chest() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [3.5, -60.0, 0.5]);
    lay(&mut region, &target);
    let hit = |target: &Entity, damage: f32, creative: bool| TickInputs {
        effects: vec![clustine_world::FiledEffect {
            id: clustine_world::EffectId {
                sequence: 7,
                index: 0,
            },
            effect: clustine_world::Effect {
                at: crate::physics::containing(target.position),
                what: clustine_world::Effected::OnEntity {
                    key: target.key,
                    by: EntityUse::Attack {
                        attacker: EntityId(1),
                        creative,
                        sprinting: false,
                        damage: Float(damage),
                        from: [Double(0.5), Double(-60.0), Double(0.5)],
                    },
                },
            },
            due: clustine_world::Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    checked(&mut region, &hit(&target, 4.1, false));
    assert!(record(&region, target.key).is_none());
    let dropped: Vec<i32> = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some(stack.item),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, [items::OAK_BOAT]);

    let mut region = ticking_with(&[]);
    let chest = Entity {
        data: boat_data(vec![(3, ItemStack::new(items::DIRT, 5))]),
        ..boat_of(2, entity_types::OAK_CHEST_BOAT, [3.5, -60.0, 0.5])
    };
    lay(&mut region, &chest);
    checked(&mut region, &hit(&chest, 0.1, true));
    assert!(record(&region, chest.key).is_none());
    let dropped: Vec<ItemStack> = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some(stack.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, [ItemStack::new(items::DIRT, 5)]);
}

/// A boat that falls lands without breaking: no fall breaks a boat in 26.3.
#[test]
fn a_boat_falling_onto_the_floor_does_not_break() {
    let mut region = ticking_with(&[]);
    let falling = Entity {
        data: with_status(4),
        ..boat(1, [3.5, -40.0, 0.5])
    };
    lay(&mut region, &falling);
    for _ in 0..80 {
        idle(&mut region);
    }
    let landed = record(&region, falling.key).expect("the boat is whole");
    assert_eq!(landed.position[1], -60.0);
    assert!(landed.on_ground);
}
