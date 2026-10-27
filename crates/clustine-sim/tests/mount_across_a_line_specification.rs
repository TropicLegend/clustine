//! Tests of getting on a vehicle across a line, written from
//! `docs/groundwork/n7j-mount-across-a-line.md` (section 8 above the rest, where it
//! amends them), ADR-0028 section 5 and `docs/groundwork/n5-plan.md` sections 7 and 8,
//! by someone who did not write the code, driving two regions by their public interface
//! alone and passing between them what a worker and the world store would: the effects
//! each region makes, filed for the other, and the hand-overs.
//!
//! The scene: a line at block x = 16 (chunk x = 1). The west region holds the land west
//! of it and shows the land beyond in copies; the east region holds the land beyond. The
//! vehicles stand in the east, a block or two past the line; the players stand in the
//! west, a block and a half short of it, within reach of the copies.
//!
//! The camel and the horse are the game's own: hatched by spawn eggs in scratch regions,
//! the horse tamed by riding it, both saddled by the item's use, as
//! `mount_riding_specification.rs` makes its mounts.

mod n6_scene;

use std::sync::OnceLock;

use clustine_data::{entity_types, items};
use clustine_sim::api::{Keys, PlayerEvent, PlayerInput, Pose, RegionEvent};
use clustine_sim::shown::{DataShown, head_yaw, mob_values};
use clustine_sim::{
    Durable, EdgeEvent, PlayerChange, PlayerTransfer, Region, TickInputs, TickOutput, Ticket,
    WorldClock,
};
use clustine_world::{
    Arrival, BlockPos, ChunkPos, Due, Effect, EffectId, Effected, Entity, EntityData, EntityId,
    EntityUse, FiledEffect, Hurt, PlayerId, Seat, Vec3,
};
use n6_scene::*;
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The numbers of the specification and the scene.

/// How long a granted seat waits for its player (section 8 item 7: 40 ticks, not 100).
const GRANTED_FOR: u32 = 40;
/// How long a seat kept for a rider who crossed waits (ADR-0028, section 5), which a
/// granted seat must not be kept for.
const KEPT_FOR: u32 = 100;

const FLOOR: f64 = -60.0;
/// Where player 1 and player 2 stand in the west.
const FIRST_AT: Vec3 = Vec3::new(14.5, FLOOR, 8.5);
const SECOND_AT: Vec3 = Vec3::new(14.5, FLOOR, 6.5);
/// Where a vehicle stands beyond the line, and a second one beside it.
const VEHICLE_AT: [f64; 3] = [17.0, FLOOR, 8.5];
const OTHER_AT: [f64; 3] = [17.0, FLOOR, 11.5];
/// Where a boat of the west's own stands, beside player 1.
const OWN_AT: [f64; 3] = [13.0, FLOOR, 8.5];
/// The chunk beyond the line the vehicles stand in.
const BEYOND: ChunkPos = ChunkPos::new(1, 0);
/// A time of night (`day` modulo 24 000), when a bed can be slept in.
const NIGHT: u64 = 13_000;

/// `AbstractHorse`'s flags, and their tame bit.
const HORSE_FLAGS: u8 = 18;
const HORSE_TAME: u8 = 2;

/// The keys of the vehicles a test sets into the world.
const VEHICLE: u32 = 1;
const OTHER_VEHICLE: u32 = 2;
const OWN_VEHICLE: u32 = 3;

// ------------------------------------------------------------------------------------
// Vehicles.

/// An oak boat as entity `n`, standing on the floor at `position`, carrying nobody.
fn boat(n: u32, position: [f64; 3]) -> Entity {
    Entity {
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
            out_of_control: clustine_world::Float(0.0),
            turning: clustine_world::Float(0.0),
            // `ON_LAND` both, by `AbstractBoat.Status`'s ordinals.
            status: [3, 3],
            contents: Vec::new(),
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// A minecart, at rest on the floor (off rails), carrying nobody: one seat.
fn cart(n: u32, position: [f64; 3]) -> Entity {
    let mut cart = boat(n, position);
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
    cart
}

/// The game's own mounts, made once for every test.
struct Made {
    camel: Entity,
    horse: Entity,
    clock: WorldClock,
}

fn made() -> &'static Made {
    static MADE: OnceLock<Made> = OnceLock::new();
    MADE.get_or_init(make)
}

fn make() -> Made {
    // A camel from its egg, saddled.
    let mut scene = scratch(&[items::CAMEL_SPAWN_EGG, items::SADDLE]);
    let camel = hatch(&mut scene, entity_types::CAMEL);
    let bare = record_of(&scene, camel);
    scene.select(1);
    scene.input(used(camel));
    let camel_saddled = record_of(&scene, camel);
    assert_ne!(camel_saddled.data, bare.data, "the saddle went on");
    let camel_clock = scene.idle().clock;

    // A horse from its egg, tamed by riding it, got off by sneaking, and saddled.
    let mut scene = scratch(&[items::HORSE_SPAWN_EGG, items::SADDLE]);
    let horse = hatch(&mut scene, entity_types::HORSE);
    scene.select(2);
    for _ in 0..40_000 {
        if flags(&record_of(&scene, horse), HORSE_FLAGS) & HORSE_TAME != 0 {
            break;
        }
        let riding = scene
            .region
            .player_state(scene.actors[0].id)
            .expect("there")
            .riding
            .is_some();
        if riding {
            scene.idle();
        } else {
            scene.input(used(horse));
        }
    }
    assert_ne!(
        flags(&record_of(&scene, horse), HORSE_FLAGS) & HORSE_TAME,
        0,
        "the horse was tamed"
    );
    scene.input(sneak(true));
    scene.input(sneak(false));
    scene.idle();
    scene.select(1);
    scene.input(used(horse));
    let horse_saddled = record_of(&scene, horse);
    let horse_clock = scene.idle().clock;

    Made {
        camel: camel_saddled,
        horse: horse_saddled,
        clock: camel_clock.later(horse_clock),
    }
}

/// A scratch region with a pen, player 1 at the spawn holding `held` from slot 0 on.
fn scratch(held: &[i32]) -> Scene {
    let mut walls = Vec::new();
    for y in [-60, -59] {
        for along in -4..=5 {
            for (x, z) in [(along, -4), (along, 5), (-4, along), (5, along)] {
                walls.push((bp(x, y, z), clustine_data::blocks::STONE));
            }
        }
    }
    let slots: Vec<(usize, clustine_sim::api::ItemStack)> = held
        .iter()
        .enumerate()
        .map(|(slot, item)| (slot, stack(*item, 1)))
        .collect();
    Scene::alone(&World::of(&walls), inventory(&slots), SPAWN)
}

/// Hatches one of `kind` by the egg in slot 0 on the floor.
fn hatch(scene: &mut Scene, kind: i32) -> EntityId {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(0);
    scene.click(bp(2, -61, 2), clustine_sim::api::Face::Top);
    let made: Vec<EntityId> = scene
        .of_kind(kind)
        .iter()
        .map(|entity| entity.key)
        .filter(|key| !before.contains(key))
        .collect();
    assert_eq!(made.len(), 1, "the egg makes one of kind {kind}");
    made[0]
}

fn record_of(scene: &Scene, key: EntityId) -> Entity {
    scene
        .all()
        .into_iter()
        .find(|entity| entity.key == key)
        .expect("there")
}

fn flags(entity: &Entity, at: u8) -> u8 {
    match mob_values(entity.kind, &entity.data)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, _)| value)
    {
        Some(DataShown::Byte(flags)) => flags,
        other => panic!("flags at {at}: {other:?}"),
    }
}

/// `record` as entity `n` at `at`, facing `yaw`, at rest and carrying nobody.
fn set(record: &Entity, n: u32, at: [f64; 3], yaw: f32) -> Entity {
    let mut entity = record.clone();
    entity.key = key(n);
    entity.uuid = Uuid::from_u128(u128::from(n) + 7_000);
    entity.position = at;
    entity.motion = [0.0; 3];
    entity.yaw = yaw;
    entity.pitch = 0.0;
    entity.on_ground = true;
    entity.fall_distance = 0.0;
    entity.riding = Default::default();
    entity
}

// ------------------------------------------------------------------------------------
// The two regions.

/// The west, with its players, and the east, with any of its own.
struct Line {
    west: Scene,
    east: Scene,
    /// The number of the next batch of effects the store files.
    sequence: u64,
}

/// A side of the line with `world`, its own players at `players` (numbered from
/// `first`), and the world's clock `clock`.
fn side_of(west: bool, world: &World, first: u128, players: &[Vec3], clock: WorldClock) -> Scene {
    let layout = Layout::side(west);
    let inventory = inventory(&[]);
    let mut scene = Scene {
        region: region_of(&layout, world, inventory.clone()),
        layout,
        inventory,
        actors: Vec::new(),
    };
    scene.region.tick(&TickInputs {
        clock: Some(clock),
        ..TickInputs::default()
    });
    for (n, feet) in (first..).zip(players) {
        scene.join(n, *feet);
    }
    scene
}

/// The world's clock, later than every scratch region's.
fn world_clock() -> WorldClock {
    let clock = made().clock;
    WorldClock {
        game_time: clock.game_time + 1_000,
        day: clock.day + 1_000,
    }
}

/// The two regions over `world` (the blocks `blocks` and the entities `entities`), with
/// players 1 onwards at `players` in the west.
fn line_with(
    blocks: &[(BlockPos, clustine_data::BlockState)],
    entities: &[Entity],
    players: &[Vec3],
    clock: WorldClock,
) -> Line {
    let world = World::of(blocks).with_entities(entities);
    Line {
        west: side_of(true, &world, 1, players, clock),
        east: side_of(false, &world, 1_000, &[], clock),
        sequence: 100,
    }
}

fn line(entities: &[Entity], players: &[Vec3]) -> Line {
    line_with(&[], entities, players, world_clock())
}

impl Line {
    /// The store files `effects` for `region` and it takes them, in one tick.
    fn file(&mut self, region: Side, effects: &[Effect]) -> TickOutput {
        self.sequence += 1;
        let sequence = self.sequence;
        let filed = effects
            .iter()
            .enumerate()
            .map(|(index, effect)| FiledEffect {
                id: EffectId {
                    sequence,
                    index: u32::try_from(index).expect("few"),
                },
                effect: effect.clone(),
                due: Due::FromTaking,
            })
            .collect();
        self.region(region).tick(&TickInputs {
            effects: filed,
            ..TickInputs::default()
        })
    }

    fn region(&mut self, side: Side) -> &mut Region {
        match side {
            Side::West => &mut self.west.region,
            Side::East => &mut self.east.region,
        }
    }

    /// The west's players `who` each do their inputs, all in one tick.
    fn act(&mut self, acts: &[(usize, PlayerInput)]) -> TickOutput {
        let inputs = acts
            .iter()
            .map(|(who, input)| {
                let actor = &mut self.west.actors[*who];
                actor.input += 1;
                (EDGE, actor.id, actor.entity, actor.input, input.clone())
            })
            .collect();
        self.west.region.tick(&TickInputs {
            inputs,
            ..TickInputs::default()
        })
    }

    /// The east takes the hand-overs `transfers`, in one tick.
    fn arrive(&mut self, transfers: &[(PlayerId, PlayerTransfer)]) -> TickOutput {
        self.east.region.tick(&TickInputs {
            player_changes: transfers
                .iter()
                .map(|(who, transfer)| PlayerChange::Arrive(EDGE, *who, transfer.clone()))
                .collect(),
            ..TickInputs::default()
        })
    }

    fn entity(&self, who: usize) -> EntityId {
        self.west.actors[who].entity
    }

    fn id(&self, who: usize) -> PlayerId {
        self.west.actors[who].id
    }
}

#[derive(Clone, Copy)]
enum Side {
    West,
    East,
}

// ------------------------------------------------------------------------------------
// Inputs.

/// A use of `key` with an empty hand, not sneaking.
fn used(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    }
}

fn sneak(down: bool) -> PlayerInput {
    PlayerInput::Keys(Keys {
        sneak: down,
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

// ------------------------------------------------------------------------------------
// What the regions report.

/// The uses `output` sends to the holder of `vehicle`.
fn uses_of(output: &TickOutput, vehicle: EntityId) -> Vec<Effect> {
    output
        .effects
        .iter()
        .filter(|effect| {
            matches!(
                effect.what,
                Effected::OnEntity { key, by: EntityUse::Interact { .. } } if key == vehicle
            )
        })
        .cloned()
        .collect()
}

/// The grants `output` sends: `Effected::Seated`.
fn grants(output: &TickOutput) -> Vec<Effect> {
    output
        .effects
        .iter()
        .filter(|effect| matches!(effect.what, Effected::Seated { .. }))
        .cloned()
        .collect()
}

/// The declines `output` sends, as (the vehicle, the player).
fn declines(output: &TickOutput) -> Vec<(EntityId, EntityId)> {
    output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::OnEntity {
                key,
                by: EntityUse::SeatDeclined { player },
            } => Some((key, player)),
            _ => None,
        })
        .collect()
}

fn decline_effects(output: &TickOutput) -> Vec<Effect> {
    output
        .effects
        .iter()
        .filter(|effect| {
            matches!(
                effect.what,
                Effected::OnEntity {
                    by: EntityUse::SeatDeclined { .. },
                    ..
                }
            )
        })
        .cloned()
        .collect()
}

/// The hand-overs `output` makes.
fn departures(output: &TickOutput) -> Vec<(PlayerId, PlayerTransfer)> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, durable)| match durable {
            Durable::Departed {
                player, transfer, ..
            } => Some((*player, transfer.clone())),
            _ => None,
        })
        .collect()
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

/// Whether any `Passengers` report of `output`, for any vehicle, names `rider`.
fn names(output: &TickOutput, rider: EntityId) -> bool {
    output.events.iter().any(|event| {
        matches!(event, RegionEvent::Passengers { passengers, .. } if passengers.contains(&rider))
    })
}

/// What `who`'s client is told of riding in `output`.
fn rides(output: &TickOutput, who: PlayerId) -> Vec<(Option<EntityId>, Pose)> {
    output
        .player_events
        .iter()
        .filter_map(|(player, event)| match event {
            PlayerEvent::Rides { vehicle, pose } if *player == who => Some((*vehicle, *pose)),
            _ => None,
        })
        .collect()
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    let found: Vec<Entity> = all_of(region)
        .into_iter()
        .filter(|entity| entity.key == key)
        .collect();
    assert!(found.len() <= 1, "{key:?} is there twice: {found:?}");
    found.into_iter().next()
}

fn seats(region: &Region, vehicle: EntityId) -> Vec<Seat> {
    record(region, vehicle)
        .map(|entity| entity.riding.passengers)
        .unwrap_or_default()
}

fn granted(seat: &Seat) -> bool {
    seat.pending.is_some_and(|pending| pending.granted)
}

/// The vehicle `who` rides in `region`, and whether they wait for it.
fn riding(region: &Region, who: PlayerId) -> Option<(EntityId, bool)> {
    region
        .player_state(who)
        .and_then(|state| state.riding)
        .map(|riding| (riding.vehicle, riding.pending.is_some()))
}

// ------------------------------------------------------------------------------------
// The path.

/// The west's players `who` click `vehicle` in one tick, and the east takes the uses:
/// the grants it sends, and its output.
fn clicked(line: &mut Line, who: &[usize], vehicle: EntityId) -> (Vec<Effect>, TickOutput) {
    let acts: Vec<(usize, PlayerInput)> = who.iter().map(|n| (*n, used(vehicle))).collect();
    let output = line.act(&acts);
    let uses = uses_of(&output, vehicle);
    assert_eq!(uses.len(), who.len(), "each click is a use on the holder");
    let output = line.file(Side::East, &uses);
    (grants(&output), output)
}

/// A boat beyond the line clicked by player 1, granted: the line, the grant and the
/// outputs so far.
fn boat_granted() -> (Line, Effect, Vec<TickOutput>) {
    let mut line = line(&[boat(VEHICLE, VEHICLE_AT)], &[FIRST_AT]);
    let click = line.act(&[(0, used(key(VEHICLE)))]);
    let uses = uses_of(&click, key(VEHICLE));
    assert_eq!(uses.len(), 1, "the click is a use on the holder");
    let use_taken = line.file(Side::East, &uses);
    let mut sent = grants(&use_taken);
    assert_eq!(sent.len(), 1, "one grant: {:?}", use_taken.effects);
    (line, sent.remove(0), vec![click, use_taken])
}

// ------------------------------------------------------------------------------------
// The plain path.

/// Section 3 steps 1 to 4 with section 8 items 1, 2 and 5: the click is a use on the
/// holder, which grants player 1 a seat of their edge, granted and hidden, and tells
/// their region by `Seated` at the block they clicked from (the west's land); the west
/// hands them over riding the boat, on their way; the east seats them as a mount.
#[test]
fn a_click_across_the_line_on_a_boat_hands_the_player_over_to_ride_it_and_its_holder_mounts_them() {
    let mut line = line(&[boat(VEHICLE, VEHICLE_AT)], &[FIRST_AT]);
    let rider = line.entity(0);
    let output = line.act(&[(0, used(key(VEHICLE)))]);
    let uses = uses_of(&output, key(VEHICLE));
    assert_eq!(uses.len(), 1);
    let Effected::OnEntity {
        by: EntityUse::Interact { player, edge, .. },
        ..
    } = uses[0].what
    else {
        unreachable!("filtered so");
    };
    assert_eq!(
        (player, edge),
        (rider, Some(EDGE)),
        "the use names the edge"
    );
    assert!(
        departures(&output).is_empty() && riding(&line.west.region, line.id(0)).is_none(),
        "the west records nothing at the click"
    );

    let output = line.file(Side::East, &uses);
    let kept = seats(&line.east.region, key(VEHICLE));
    assert_eq!(kept.len(), 1, "one seat: {kept:?}");
    assert!(kept[0].player && kept[0].key == rider);
    assert!(granted(&kept[0]), "granted");
    assert_eq!(kept[0].pending.map(|pending| pending.edge), Some(EDGE));
    let sent = grants(&output);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].what,
        Effected::Seated {
            player: rider,
            vehicle: key(VEHICLE),
            chunk: BEYOND,
        }
    );
    assert!(
        sent[0].at.chunk().x <= 0,
        "addressed to the west's land, where the player stood: {:?}",
        sent[0].at
    );

    let output = line.file(Side::West, &sent);
    assert!(declines(&output).is_empty());
    let handed = departures(&output);
    assert_eq!(handed.len(), 1, "handed over once: {handed:?}");
    let riding_on = handed[0].1.riding.expect("handed over riding");
    assert_eq!(riding_on.vehicle, key(VEHICLE));
    assert_eq!(
        riding_on.chunk, BEYOND,
        "its holding chunk is the vehicle's"
    );
    assert!(riding_on.pending.is_some(), "on their way to it");

    let output = line.arrive(&handed);
    assert_eq!(
        rides(&output, line.id(0))
            .iter()
            .map(|(vehicle, _)| *vehicle)
            .collect::<Vec<_>>(),
        [Some(key(VEHICLE))],
        "a mount: their client is told they ride"
    );
    assert_eq!(passengers_reported(&output, key(VEHICLE)), [vec![rider]]);
    let taken = seats(&line.east.region, key(VEHICLE));
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].pending, None, "the seat is taken");
    assert_eq!(
        riding(&line.east.region, line.id(0)),
        Some((key(VEHICLE), false)),
        "seated"
    );
}

/// Section 8 items 1 and 2 (and item 11's edge case at the region's side): no
/// `Passengers` report names the clicker until they are seated, however many ticks the
/// grant and the hand-over take, and their client is told `Rides` once, at the seating.
#[test]
fn the_clicker_is_shown_seated_in_no_report_before_the_seating_and_told_they_ride_at_it() {
    let (mut line, grant, mut seen) = boat_granted();
    let rider = line.entity(0);
    for _ in 0..5 {
        seen.push(line.east.idle());
        seen.push(line.west.idle());
    }
    let output = line.file(Side::West, &[grant]);
    let handed = departures(&output);
    seen.push(output);
    for _ in 0..3 {
        seen.push(line.east.idle());
    }
    for (n, output) in seen.iter().enumerate() {
        assert!(
            !names(output, rider),
            "output {n} before the seating names the player: {:?}",
            output.events
        );
        assert!(
            rides(output, line.id(0)).is_empty(),
            "output {n} tells the client of riding before the seating"
        );
    }
    let output = line.arrive(&handed);
    assert_eq!(rides(&output, line.id(0)).len(), 1, "told at the seating");
    assert!(names(&output, rider), "and the seat shown at it");
    let after = line.east.idle();
    assert!(rides(&after, line.id(0)).is_empty(), "and told once");
}

/// Section 8 item 1 and section 3 step 2: a saddled horse across the line takes the
/// player as one within a region, and at the first seating it turns them to its head,
/// as `doPlayerRide` does (the turn is the holder's, made at the seating).
#[test]
fn a_saddled_horse_across_the_line_seats_the_player_turned_to_its_head() {
    let horse = set(&made().horse, VEHICLE, [17.5, FLOOR, 8.5], 90.0);
    let mut line = line(&[horse], &[FIRST_AT]);
    let rider = line.entity(0);
    line.act(&[(0, turn(45.0, 20.0))]);
    let (sent, _) = clicked(&mut line, &[0], key(VEHICLE));
    assert_eq!(sent.len(), 1, "a seat granted on the horse");
    assert!(granted(&seats(&line.east.region, key(VEHICLE))[0]));
    let output = line.file(Side::West, &sent);
    let handed = departures(&output);
    assert_eq!(handed.len(), 1);
    let before = record(&line.east.region, key(VEHICLE)).expect("the horse");
    let head = head_yaw(&before.data).expect("a mob's head");
    assert_ne!(head, 45.0, "the turn shows");

    let output = line.arrive(&handed);
    let told = rides(&output, line.id(0));
    assert_eq!(told.len(), 1, "told they ride: {told:?}");
    assert_eq!(told[0].0, Some(key(VEHICLE)));
    let pose = told[0].1;
    assert_eq!(
        (pose.yaw, pose.pitch),
        (head, before.pitch),
        "turned to the horse's head yaw and its pitch"
    );
    assert_eq!(passengers_reported(&output, key(VEHICLE)), [vec![rider]]);
    assert_eq!(
        riding(&line.east.region, line.id(0)),
        Some((key(VEHICLE), false))
    );
}

/// Section 3 and section 6: a camel across the line takes two players who click it in
/// one tick, each on a seat granted them, each handed over and seated.
#[test]
fn a_camel_across_the_line_takes_two_players_who_click_it_in_one_tick() {
    let camel = set(&made().camel, VEHICLE, [17.5, FLOOR, 8.5], 90.0);
    let mut line = line(&[camel], &[FIRST_AT, SECOND_AT]);
    let (first, second) = (line.entity(0), line.entity(1));
    let (sent, output) = clicked(&mut line, &[0, 1], key(VEHICLE));
    assert_eq!(sent.len(), 2, "a seat for each: {:?}", output.effects);
    let kept = seats(&line.east.region, key(VEHICLE));
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(granted));
    let mut keys: Vec<EntityId> = kept.iter().map(|seat| seat.key).collect();
    keys.sort();
    let mut both = vec![first, second];
    both.sort();
    assert_eq!(keys, both);

    let output = line.file(Side::West, &sent);
    assert!(declines(&output).is_empty());
    let handed = departures(&output);
    assert_eq!(handed.len(), 2, "both handed over: {handed:?}");

    let output = line.arrive(&handed);
    for who in 0..2 {
        assert_eq!(
            rides(&output, line.id(who))
                .iter()
                .map(|(vehicle, _)| *vehicle)
                .collect::<Vec<_>>(),
            [Some(key(VEHICLE))],
            "player {} is told they ride",
            who + 1
        );
        assert_eq!(
            riding(&line.east.region, line.id(who)),
            Some((key(VEHICLE), false))
        );
    }
    let taken = seats(&line.east.region, key(VEHICLE));
    assert_eq!(taken.len(), 2);
    assert!(taken.iter().all(|seat| seat.pending.is_none()));
    let shown = passengers_reported(&output, key(VEHICLE));
    let last = shown.last().expect("the camel's seats are reported");
    assert_eq!(last.len(), 2, "both shown at last: {shown:?}");
}

// ------------------------------------------------------------------------------------
// Idempotence (section 8 item 3).

/// Section 8 item 3: a double click on a two-seat camel grants one seat; a use taken
/// again later grants nothing more; the player gets on once.
#[test]
fn a_double_click_on_a_camel_across_the_line_grants_one_seat() {
    let camel = set(&made().camel, VEHICLE, [17.5, FLOOR, 8.5], 90.0);
    let mut line = line(&[camel], &[FIRST_AT]);
    let rider = line.entity(0);
    let output = line.act(&[(0, used(key(VEHICLE))), (0, used(key(VEHICLE)))]);
    let uses = uses_of(&output, key(VEHICLE));
    assert!(!uses.is_empty(), "the clicks are uses on the holder");
    // Both clicks, however the west sent them.
    let both = [uses[0].clone(), uses.last().expect("one").clone()];
    let output = line.file(Side::East, &both);
    let sent = grants(&output);
    assert_eq!(sent.len(), 1, "one grant: {:?}", output.effects);
    let kept = seats(&line.east.region, key(VEHICLE));
    assert_eq!(kept.len(), 1, "one seat: {kept:?}");
    assert_eq!(kept[0].key, rider);

    let again = line.file(Side::East, &both[..1]);
    assert!(grants(&again).is_empty(), "nothing granted again");
    assert_eq!(seats(&line.east.region, key(VEHICLE)).len(), 1);

    let output = line.file(Side::West, &sent);
    let handed = departures(&output);
    assert_eq!(handed.len(), 1);
    let output = line.arrive(&handed);
    assert_eq!(passengers_reported(&output, key(VEHICLE)), [vec![rider]]);
    assert_eq!(seats(&line.east.region, key(VEHICLE)).len(), 1);
}

/// Section 8 item 3: a grant reaching the player's region twice in one tick (a replayed
/// one, section 5 item 5) finds them on their way to that vehicle: the second is
/// ignored, not declined, and they are handed over once.
#[test]
fn a_grant_for_the_vehicle_a_player_waits_for_is_ignored_and_not_declined() {
    let (mut line, grant, _) = boat_granted();
    let output = line.file(Side::West, &[grant.clone(), grant]);
    assert_eq!(declines(&output), [], "nothing declined");
    assert_eq!(departures(&output).len(), 1, "handed over once");
    let kept = seats(&line.east.region, key(VEHICLE));
    assert_eq!(kept.len(), 1);
}

/// Section 8 item 3: a grant naming the vehicle the player already rides is ignored:
/// they stay on it, nothing is declined, nobody is handed over.
#[test]
fn a_grant_for_the_vehicle_a_player_rides_already_is_ignored_and_not_declined() {
    let mut line = line(&[boat(OWN_VEHICLE, OWN_AT)], &[FIRST_AT]);
    let rider = line.entity(0);
    let output = line.act(&[(0, used(key(OWN_VEHICLE)))]);
    assert_eq!(
        rides(&output, line.id(0))
            .iter()
            .map(|(vehicle, _)| *vehicle)
            .collect::<Vec<_>>(),
        [Some(key(OWN_VEHICLE))],
        "on the west's own boat"
    );
    line.west.idle();
    let stale = Effect {
        at: BlockPos::new(14, -60, 8),
        what: Effected::Seated {
            player: rider,
            vehicle: key(OWN_VEHICLE),
            chunk: ChunkPos::new(0, 0),
        },
    };
    let output = line.file(Side::West, &[stale]);
    assert_eq!(declines(&output), [], "nothing declined");
    assert!(departures(&output).is_empty(), "nobody handed over");
    assert!(rides(&output, line.id(0)).is_empty(), "nothing told");
    assert_eq!(
        riding(&line.west.region, line.id(0)),
        Some((key(OWN_VEHICLE), false)),
        "still on it"
    );
    let kept = seats(&line.west.region, key(OWN_VEHICLE));
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].key, kept[0].pending), (rider, None));
}

// ------------------------------------------------------------------------------------
// Declines (section 3 step 3, section 8 items 3 and 6).

/// Players 1 and 2 click a camel beyond the line in one tick and both are granted a
/// seat; `meanwhile` does what player 1 does before the grants reach the west. Player 1
/// is declined and not handed over; the decline drops their seat alone; player 2 is
/// handed over and seated.
fn player_one_declined(
    blocks: &[(BlockPos, clustine_data::BlockState)],
    clock: WorldClock,
    meanwhile: impl FnOnce(&mut Line),
) {
    let camel = set(&made().camel, VEHICLE, [17.5, FLOOR, 8.5], 90.0);
    let mut line = line_with(blocks, &[camel], &[FIRST_AT, SECOND_AT], clock);
    let (first, second) = (line.entity(0), line.entity(1));
    let (sent, _) = clicked(&mut line, &[0, 1], key(VEHICLE));
    assert_eq!(sent.len(), 2, "both granted");
    meanwhile(&mut line);

    let output = line.file(Side::West, &sent);
    assert_eq!(
        declines(&output),
        [(key(VEHICLE), first)],
        "player 1's seat is declined, and only theirs"
    );
    let handed = departures(&output);
    assert_eq!(
        handed.iter().map(|(who, _)| *who).collect::<Vec<_>>(),
        [line.id(1)],
        "player 2 alone is handed over"
    );
    if line.west.region.player(line.id(0)).is_some() {
        assert_eq!(
            riding(&line.west.region, line.id(0)),
            None,
            "player 1 rides nothing"
        );
    }

    let decline = decline_effects(&output);
    line.file(Side::East, &decline);
    let kept = seats(&line.east.region, key(VEHICLE));
    assert_eq!(
        kept.iter().map(|seat| seat.key).collect::<Vec<_>>(),
        [second],
        "player 2's seat stays"
    );
    assert!(granted(&kept[0]), "untaken still");

    let output = line.arrive(&handed);
    assert_eq!(passengers_reported(&output, key(VEHICLE)), [vec![second]]);
    assert_eq!(
        riding(&line.east.region, line.id(1)),
        Some((key(VEHICLE), false))
    );
}

/// Section 5 item 1 and section 6: a player who sneaks between the click and the grant
/// is not seated and the seat goes.
#[test]
fn a_grant_reaching_a_player_who_sneaks_now_is_declined() {
    player_one_declined(&[], world_clock(), |line| {
        line.act(&[(0, sneak(true))]);
        assert!(
            line.west
                .region
                .player_state(line.id(0))
                .expect("there")
                .keys
                .sneak
        );
    });
}

/// Section 8 item 6: not sleeping.
#[test]
fn a_grant_reaching_a_player_asleep_now_is_declined() {
    // A bed beside player 1, its head towards the line's side of them.
    let head = bp(14, -60, 9);
    let foot = bp(14, -60, 10);
    let bed = |part: &str| state(&format!("minecraft:red_bed[facing=north,part={part}]"));
    let clock = world_clock();
    let night = WorldClock {
        game_time: clock.game_time,
        day: (clock.day / 24_000 + 1) * 24_000 + NIGHT,
    };
    player_one_declined(&[(head, bed("head")), (foot, bed("foot"))], night, |line| {
        line.west.click_by(0, foot, clustine_sim::api::Face::Top);
        assert!(
            line.west
                .region
                .player_state(line.id(0))
                .expect("there")
                .sleeping
                .is_some(),
            "player 1 lies in the bed"
        );
    });
}

/// Section 8 item 6: no menu open.
#[test]
fn a_grant_reaching_a_player_with_a_menu_open_is_declined() {
    let chest = bp(13, -60, 9);
    player_one_declined(
        &[(chest, state("minecraft:chest[facing=east]"))],
        world_clock(),
        |line| {
            line.west.open_by(0, chest);
            assert!(
                line.west
                    .region
                    .player_state(line.id(0))
                    .expect("there")
                    .menu
                    .is_some(),
                "player 1 has the chest open"
            );
        },
    );
}

/// Section 5 item 1, section 6 and section 8 item 6: within reach of the west's copy of
/// the vehicle, with a block's slack.
#[test]
fn a_grant_reaching_a_player_who_walked_out_of_reach_is_declined() {
    player_one_declined(&[], world_clock(), |line| {
        // A creative player's use reaches 8 blocks (`CREATIVE_ENTITY_REACH`, 5 and the
        // server's 3 of slack), and the grant allows a block more: 14 blocks from the
        // camel's box is out of it.
        line.west.walk_to(Vec3::new(8.5, FLOOR, 8.5));
        let away = Vec3::new(2.5, FLOOR, 8.5);
        line.west.walk_to(away);
        assert_eq!(
            line.west
                .region
                .player(line.id(0))
                .expect("there")
                .1
                .position,
            away
        );
    });
}

/// Section 3 step 3 ("where the player is still A's ... otherwise A answers
/// `SeatDeclined`") and section 8 item 6 ("still A's"): player 1 walked over the line
/// and was handed over before the grant came.
#[test]
fn a_grant_reaching_the_region_a_player_has_left_is_declined() {
    player_one_declined(&[], world_clock(), |line| {
        let output = line.west.walk_to(Vec3::new(16.5, FLOOR, 4.5));
        assert!(
            departures(&output)
                .iter()
                .any(|(who, _)| *who == line.id(0)),
            "player 1 walked over the line"
        );
        assert!(line.west.region.player(line.id(0)).is_none());
    });
}

/// Section 8 item 6: one who rides another vehicle at the grant is let off it first,
/// then handed over riding the one granted.
#[test]
fn a_player_riding_another_vehicle_is_let_off_it_and_handed_over() {
    let mut line = line(
        &[boat(VEHICLE, VEHICLE_AT), boat(OWN_VEHICLE, OWN_AT)],
        &[FIRST_AT],
    );
    let rider = line.entity(0);
    line.act(&[(0, used(key(OWN_VEHICLE)))]);
    assert_eq!(
        riding(&line.west.region, line.id(0)),
        Some((key(OWN_VEHICLE), false))
    );
    line.west.idle();
    let (sent, _) = clicked(&mut line, &[0], key(VEHICLE));
    assert_eq!(sent.len(), 1, "a seat granted");

    let output = line.file(Side::West, &sent);
    assert_eq!(declines(&output), []);
    assert_eq!(
        passengers_reported(&output, key(OWN_VEHICLE)),
        [Vec::<EntityId>::new()],
        "let off their boat"
    );
    assert!(seats(&line.west.region, key(OWN_VEHICLE)).is_empty());
    let handed = departures(&output);
    assert_eq!(handed.len(), 1);
    assert_eq!(
        handed[0].1.riding.map(|riding| riding.vehicle),
        Some(key(VEHICLE)),
        "handed over riding the one granted"
    );

    let output = line.arrive(&handed);
    assert_eq!(passengers_reported(&output, key(VEHICLE)), [vec![rider]]);
    assert_eq!(
        riding(&line.east.region, line.id(0)),
        Some((key(VEHICLE), false))
    );
}

// ------------------------------------------------------------------------------------
// Two vehicles in one tick, and the decline's order against the hand-over.

/// Player 1 clicks two boats beyond the line in one tick; both are granted; the west
/// takes both grants in one tick. It hands the player over riding one of them and
/// declines the other (section 8 item 6: one on their way to a vehicle is declined).
struct TwoBoats {
    line: Line,
    rider: EntityId,
    handed: Vec<(PlayerId, PlayerTransfer)>,
    chosen: EntityId,
    other: EntityId,
    decline: Vec<Effect>,
}

fn two_boats() -> TwoBoats {
    let mut line = line(
        &[boat(VEHICLE, VEHICLE_AT), boat(OTHER_VEHICLE, OTHER_AT)],
        &[FIRST_AT],
    );
    let rider = line.entity(0);
    let output = line.act(&[(0, used(key(VEHICLE))), (0, used(key(OTHER_VEHICLE)))]);
    let mut uses = uses_of(&output, key(VEHICLE));
    uses.extend(uses_of(&output, key(OTHER_VEHICLE)));
    assert_eq!(uses.len(), 2, "a use on each: {:?}", output.effects);
    let output = line.file(Side::East, &uses);
    let sent = grants(&output);
    assert_eq!(sent.len(), 2, "a seat on each granted");
    for vehicle in [key(VEHICLE), key(OTHER_VEHICLE)] {
        let kept = seats(&line.east.region, vehicle);
        assert_eq!(kept.len(), 1);
        assert!(granted(&kept[0]) && kept[0].key == rider);
    }

    let output = line.file(Side::West, &sent);
    let handed = departures(&output);
    assert_eq!(handed.len(), 1, "handed over once: {handed:?}");
    let chosen = handed[0].1.riding.expect("riding").vehicle;
    let other = if chosen == key(VEHICLE) {
        key(OTHER_VEHICLE)
    } else {
        key(VEHICLE)
    };
    assert_eq!(declines(&output), [(other, rider)], "the other is declined");
    TwoBoats {
        line,
        rider,
        handed,
        chosen,
        other,
        decline: decline_effects(&output),
    }
}

fn assert_seated_on_the_chosen(boats: &TwoBoats) {
    let east = &boats.line.east.region;
    assert!(seats(east, boats.other).is_empty(), "the other seat went");
    let kept = seats(east, boats.chosen);
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].key, kept[0].pending), (boats.rider, None));
    assert_eq!(
        riding(east, boats.line.id(0)),
        Some((boats.chosen, false)),
        "seated on the chosen"
    );
}

/// Section 8 item 11: the decline reaches the holder before the hand-over.
#[test]
fn a_decline_reaching_the_holder_before_the_hand_over_drops_only_the_declined_seat() {
    let mut boats = two_boats();
    let decline = boats.decline.clone();
    let output = boats.line.file(Side::East, &decline);
    assert!(
        !names(&output, boats.rider),
        "a seat never shown goes unreported"
    );
    assert!(seats(&boats.line.east.region, boats.other).is_empty());
    let kept = seats(&boats.line.east.region, boats.chosen);
    assert!(kept.len() == 1 && granted(&kept[0]), "the chosen one kept");
    let handed = boats.handed.clone();
    let output = boats.line.arrive(&handed);
    assert_eq!(
        rides(&output, boats.line.id(0))
            .iter()
            .map(|(vehicle, _)| *vehicle)
            .collect::<Vec<_>>(),
        [Some(boats.chosen)]
    );
    assert_seated_on_the_chosen(&boats);
}

/// Section 8 items 3 and 11: the decline reaches the holder after the hand-over; and a
/// decline naming the vehicle they now ride (a replayed one) leaves them on it, since a
/// decline drops only a granted seat.
#[test]
fn a_decline_reaching_the_holder_after_the_hand_over_drops_only_the_declined_seat() {
    let mut boats = two_boats();
    let handed = boats.handed.clone();
    boats.line.arrive(&handed);
    let decline = boats.decline.clone();
    boats.line.file(Side::East, &decline);
    assert_seated_on_the_chosen(&boats);

    let replayed = Effect {
        at: decline[0].at,
        what: Effected::OnEntity {
            key: boats.chosen,
            by: EntityUse::SeatDeclined {
                player: boats.rider,
            },
        },
    };
    let output = boats.line.file(Side::East, &[replayed]);
    assert!(rides(&output, boats.line.id(0)).is_empty());
    assert_seated_on_the_chosen(&boats);
}

// ------------------------------------------------------------------------------------
// The vehicle comes to the player's region (section 8 item 4).

/// Section 8 item 4: the boat crossed into the west, with its granted seat, before the
/// grant came; the west holds it now and mounts the player itself: no hand-over, no
/// decline, `Rides`, and the seat replaced by a taken one.
#[test]
fn a_vehicle_that_came_into_the_players_region_before_the_grant_is_mounted_there() {
    let (mut line, grant, _) = boat_granted();
    let rider = line.entity(0);
    let mut moved = record(&line.east.region, key(VEHICLE)).expect("the boat");
    assert!(moved.riding.passengers.iter().any(granted));
    moved.position = [15.0, FLOOR, 8.5];
    moved.hops += 1;
    let chunk = ChunkPos::new(0, 0);
    assert_eq!(moved.chunk(), chunk);
    line.west.region.tick(&TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 90,
                index: 0,
            },
            chunk,
            entity: moved,
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    });
    assert!(
        record(&line.west.region, key(VEHICLE)).is_some(),
        "the west holds the boat"
    );

    let output = line.file(Side::West, &[grant]);
    assert_eq!(declines(&output), [], "nothing declined");
    assert!(departures(&output).is_empty(), "nobody handed over");
    assert_eq!(
        rides(&output, line.id(0))
            .iter()
            .map(|(vehicle, _)| *vehicle)
            .collect::<Vec<_>>(),
        [Some(key(VEHICLE))]
    );
    assert_eq!(
        riding(&line.west.region, line.id(0)),
        Some((key(VEHICLE), false))
    );
    let kept = seats(&line.west.region, key(VEHICLE));
    assert_eq!(kept.len(), 1, "one seat: {kept:?}");
    assert_eq!((kept[0].key, kept[0].pending), (rider, None));
    assert_eq!(passengers_reported(&output, key(VEHICLE)), [vec![rider]]);
}

// ------------------------------------------------------------------------------------
// A granted seat that is never taken.

/// Section 3 and section 8 items 2 and 7: a granted seat never taken lapses after 40
/// ticks (not the 100 of a crossing's), and since it was never shown, no `Passengers`
/// report tells of it. (That the vehicle's type is told nothing, a happy ghast's ten
/// ticks' stop, is not seen from a boat.)
#[test]
fn a_granted_seat_never_taken_lapses_after_forty_ticks_unreported() {
    let (mut line, _, _) = boat_granted();
    let mut reported = Vec::new();
    for _ in 0..GRANTED_FOR - 2 {
        let output = line.east.idle();
        reported.extend(passengers_reported(&output, key(VEHICLE)));
    }
    let kept = seats(&line.east.region, key(VEHICLE));
    assert!(
        kept.len() == 1 && granted(&kept[0]),
        "kept for 40 ticks: {kept:?}"
    );
    for _ in 0..3 {
        let output = line.east.idle();
        reported.extend(passengers_reported(&output, key(VEHICLE)));
    }
    assert!(
        seats(&line.east.region, key(VEHICLE)).is_empty(),
        "gone by its 41st tick, well before the {KEPT_FOR} of a crossing's"
    );
    assert_eq!(reported, Vec::<Vec<EntityId>>::new(), "never reported");
}

/// Section 3 step 2 and section 8 item 7: the seat is of the player's edge, and goes
/// with it.
#[test]
fn a_granted_seat_goes_with_the_players_edge() {
    let (mut line, _, _) = boat_granted();
    let output = line.east.region.tick(&TickInputs {
        edges: vec![EdgeEvent::Gone { edge: EDGE }],
        ..TickInputs::default()
    });
    let next = line.east.idle();
    assert!(
        seats(&line.east.region, key(VEHICLE)).is_empty(),
        "the seat went with the edge"
    );
    assert!(!names(&output, line.entity(0)) && !names(&next, line.entity(0)));
}

/// Section 3 step 2 and section 6: a vehicle with no room grants nothing. The cart's
/// one seat is taken by a player of the east's own.
#[test]
fn a_vehicle_with_no_room_grants_nothing() {
    let world = World::default().with_entities(&[cart(VEHICLE, VEHICLE_AT)]);
    let clock = world_clock();
    let mut line = Line {
        west: side_of(true, &world, 1, &[FIRST_AT], clock),
        east: side_of(false, &world, 2, &[Vec3::new(18.5, FLOOR, 8.5)], clock),
        sequence: 100,
    };
    let own = line.east.actors[0].entity;
    line.east.input(used(key(VEHICLE)));
    assert_eq!(
        line.east
            .region
            .player_state(line.east.actors[0].id)
            .and_then(|state| state.riding)
            .map(|riding| riding.vehicle),
        Some(key(VEHICLE)),
        "the east's player is in the cart"
    );
    line.east.idle();
    let (sent, output) = clicked(&mut line, &[0], key(VEHICLE));
    assert!(sent.is_empty(), "nothing granted: {:?}", output.effects);
    let kept = seats(&line.east.region, key(VEHICLE));
    assert_eq!(kept.iter().map(|seat| seat.key).collect::<Vec<_>>(), [own]);
}

// ------------------------------------------------------------------------------------
// Take-ins.

/// Section 8 items 1 and 2 with the plan of N5, section 8 item 1: at a restore a kept
/// seat with time left is kept; a granted one is kept granted, still hidden, and the
/// player handed over is seated by it as a mount.
#[test]
fn a_granted_seat_is_kept_through_a_restore_still_hidden_and_seats_the_player() {
    let (mut line, grant, _) = boat_granted();
    let rider = line.entity(0);
    line.east.idle();
    let east = &line.east.region;
    let state = east.state();
    let saved: Vec<(ChunkPos, clustine_world::Chunk)> = east
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| east.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    let layout = Layout::side(false);
    let tickets: Vec<(ChunkPos, Ticket)> = layout
        .loaded
        .iter()
        .chain(&layout.foreign)
        .map(|chunk| (*chunk, Ticket::Viewer))
        .collect();
    let restored = Region::restore_loaded(
        config(inventory(&[])),
        state,
        layout.holdings(),
        &tickets,
        saved,
        Vec::new(),
    );
    line.east.region = restored;
    let kept = seats(&line.east.region, key(VEHICLE));
    assert_eq!(kept.len(), 1, "kept: {kept:?}");
    assert!(granted(&kept[0]) && kept[0].key == rider, "still granted");
    for _ in 0..2 {
        let output = line.east.idle();
        assert!(!names(&output, rider), "still hidden after the restore");
    }

    let output = line.file(Side::West, &[grant]);
    let handed = departures(&output);
    assert_eq!(handed.len(), 1);
    let output = line.arrive(&handed);
    assert_eq!(
        rides(&output, line.id(0))
            .iter()
            .map(|(vehicle, _)| *vehicle)
            .collect::<Vec<_>>(),
        [Some(key(VEHICLE))],
        "a mount after the restore too"
    );
    assert_eq!(passengers_reported(&output, key(VEHICLE)), [vec![rider]]);
}
