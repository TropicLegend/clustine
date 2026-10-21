//! Tests of riding the camel, the camel husk, the happy ghast, the nautilus and the
//! zombie nautilus (part R of step N7j), written from the specification by someone who
//! did not write the code: the research note `docs/groundwork/n7j-riding-research.md`
//! (cited as "R §n"), the plan `docs/groundwork/n7j-riding-plan.md` (cited as "P §n",
//! its section 9 over the rest), `docs/groundwork/n7-brief.md` sections 7.7.5, 7.11.4
//! and 7.12.5 (cited as "B 7.7.5" and so on) and the 26.3 bytecode
//! (`reference/n7-javap/`), cited as the class and its line in the dump: `Camel` is
//! `net.minecraft.world.entity.animal.camel.Camel.txt`, `HappyGhast` and
//! `AbstractNautilus` likewise. Where the brief and the dumps differ, the dumps are
//! followed and the test says so. Regions are driven by their public interface alone.
//!
//! The mounts are the game's own: hatched by spawn eggs in scratch regions, saddled or
//! harnessed by the item's use, a nautilus tamed by pufferfish, a calf and a ghastling
//! hatched by the egg on the grown one, a sitting camel left to its brain until it sat.
//! Their records are then set into the world of each test where it wants them, and the
//! test's region is given the world's clock as the scratch regions left it and more
//! (`TickInputs::clock`), as the store would publish it: a camel's pose is read by the
//! world's game time (B 7.7.4), the same in every region.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60), in creative mode and
//! right-handed as every Clustine player is.

mod n6_scene;

use std::sync::OnceLock;

use clustine_data::sounds as sound;
use clustine_data::{EntityTypeInfo, blocks, entity_types, items};
use clustine_sim::api::{
    ClickKind, Keys, MenuKind, PlayerEvent, PlayerInput, RegionEvent, Tracked, VehicleMove,
};
use clustine_sim::shown::{DataShown, head_yaw, mob_values};
use clustine_sim::{PlayerChange, StayNote, TickInputs, TickOutput, WorldClock};
use clustine_world::{Arrival, BlockPos, ChunkPos, Due, EffectId, Entity, EntityId, Vec3};
use n6_scene::*;
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The numbers of the specification.

/// The player's vehicle attachment (`Avatar.DEFAULT_VEHICLE_ATTACHMENT`).
const PLAYER_ATTACHMENT: f64 = 0.6;
/// A player's width (`EntityType.PLAYER`), a float.
const PLAYER_WIDTH: f32 = 0.6;
/// A player's eye height standing.
const PLAYER_EYES: f64 = 1.62;
/// `Camel.SITTING_HEIGHT_DIFFERENCE`: the sitting box is `scalable(width, height − 1.43f)`
/// (`Camel` static initialiser 1619–1628).
const SITTING_HEIGHT_DIFFERENCE: f32 = 1.43;
/// The camel's synced values (R §6): `DASH` and `LAST_POSE_CHANGE_TICK`.
const CAMEL_DASH: u8 = 19;
const CAMEL_POSE_TICK: u8 = 20;
/// The nautilus's `DASH` (R §6) and `TamableAnimal`'s flags with their tame bit.
const NAUTILUS_DASH: u8 = 20;
const TAME_FLAGS: u8 = 18;
const TAME_BIT: u8 = 4;
/// `AbstractHorse`'s flags, and their tame and standing bits (for the horse's jump).
const HORSE_FLAGS: u8 = 18;
const HORSE_TAME: u8 = 2;
const HORSE_STANDING: u8 = 32;
/// The camel's dash cooldown (`Camel.onSyncedDataUpdated` 1526–1548) and the nautilus's
/// (`AbstractNautilus.onSyncedDataUpdated` 837–858).
const CAMEL_COOLDOWN: u32 = 55;
const NAUTILUS_COOLDOWN: u32 = 40;
/// `AbstractHorse.getSoundVolume` (793–796), the camel's `makeSound` volume.
const CAMEL_VOICE_VOLUME: f32 = 0.8;
/// `AbstractHorse.playJumpSound`: `playSound(…, 0.4f, 1.0f)`.
const HORSE_JUMP_VOLUME: f32 = 0.4;
/// The happy ghast's still timeout after a rider gets off (`HappyGhast.removePassenger`
/// 570–590), and the grace after it is loaded in which it does not count down
/// (`tick()` 798–847: only while `tickCount > 60`).
const STILL_TICKS: u32 = 10;
const STILL_GRACE: u32 = 60;
/// `ServerGamePacketListenerImpl`'s floating kick for a living root: 80 ticks at a
/// gravity of 0.08 (R §2.5); the happy ghast's riders are held up longer than that.
const ALOFT: u32 = 120;

/// The top of the stone floor.
const FLOOR: f64 = -60.0;

/// What the hotbar holds: slot 0, selected, is empty.
const SADDLE: u8 = 1;
const HARNESS: u8 = 2;
const SHEARS: u8 = 3;
const CACTUS: u8 = 4;
const COD: u8 = 5;
const SNOWBALL: u8 = 6;

/// The key of the mount every test sets into its world, and of a second entity.
const MOUNT: u32 = 1;
const OTHER_ENTITY: u32 = 2;
/// Where a camel or a nautilus stands, in block (2, -60, 1), within reach of the players.
const MOUNT_AT: [f64; 3] = [2.5, FLOOR, 1.5];
/// Where a happy ghast floats, its 4-wide box over blocks 1..=5 by 0..=4, clear of the
/// players.
const GHAST_AT: [f64; 3] = [3.5, FLOOR, 2.5];
/// Where the players of a scene stand, player 1 first.
const PLAYERS_AT: [Vec3; 5] = [
    Vec3::new(0.5, FLOOR, 0.5),
    Vec3::new(0.5, FLOOR, 3.5),
    Vec3::new(4.5, FLOOR, -0.5),
    Vec3::new(-0.5, FLOOR, 2.0),
    Vec3::new(6.5, FLOOR, 3.5),
];

fn hotbar() -> clustine_sim::api::Inventory {
    inventory(&[
        (usize::from(SADDLE), stack(items::SADDLE, 1)),
        (usize::from(HARNESS), stack(items::WHITE_HARNESS, 1)),
        (usize::from(SHEARS), stack(items::SHEARS, 1)),
        (usize::from(CACTUS), stack(items::CACTUS, 1)),
        (usize::from(COD), stack(items::COD, 1)),
        (usize::from(SNOWBALL), stack(items::SNOWBALL, 1)),
    ])
}

// ------------------------------------------------------------------------------------
// `Mth` and `Vec3` as the game computes them (`n5-javap/net.minecraft.util.Mth.txt`,
// `Vec3.yRot`).

/// `Mth.SIN[i] = (float) Math.sin(i / 10430.378350470453)`.
fn table(index: i64) -> f32 {
    ((index & 65_535) as f64 / 10_430.378_350_470_453).sin() as f32
}

fn mth_sin(value: f64) -> f32 {
    table((value * 10_430.378_350_470_453) as i64)
}

fn mth_cos(value: f64) -> f32 {
    table((value * 10_430.378_350_470_453 + 16_384.0) as i64)
}

const DEGREES: f32 = 0.017_453_292;

/// `Vec3.yRot(angle)`.
fn y_rot(v: [f64; 3], angle: f32) -> [f64; 3] {
    let c = f64::from(mth_cos(f64::from(angle)));
    let s = f64::from(mth_sin(f64::from(angle)));
    [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
}

/// `Entity.getCollisionHorizontalEscapeVector(vehicleWidth, riderWidth, yaw)`.
fn escape(vehicle_width: f32, yaw: f32) -> [f64; 3] {
    let d = (f64::from(vehicle_width) + f64::from(PLAYER_WIDTH) + 9.999_999_747_378_752e-6) / 2.0;
    let sin = -mth_sin(f64::from(yaw * DEGREES));
    let cos = mth_cos(f64::from(yaw * DEGREES));
    let most = sin.abs().max(cos.abs());
    [
        f64::from(sin) * d / f64::from(most),
        0.0,
        f64::from(cos) * d / f64::from(most),
    ]
}

/// `Mth.lerp(delta, start, end)` in floats.
fn lerp(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

/// `Camel.getBodyAnchorAnimationYOffset(first, 0, dims, 1)` (`Camel` 1115–1272) for a
/// grown camel, sitting or not, `pose_time` ticks after its last change of pose.
fn camel_anchor(first: bool, sitting: bool, pose_time: i64) -> f64 {
    let standing = EntityTypeInfo::of(entity_types::CAMEL).height;
    let height = if sitting {
        standing - SITTING_HEIGHT_DIFFERENCE
    } else {
        standing
    };
    let scale = 1.0_f32;
    let mut y = f64::from(height) - 0.375;
    let sit = scale * 1.43_f32;
    let mid0 = sit - scale * 0.2_f32;
    let low = sit - mid0;
    let total: i64 = if sitting { 40 } else { 52 };
    let in_transition = pose_time < total;
    if in_transition {
        let (split, k) = if sitting {
            (28, if first { 0.5_f32 } else { 0.1 })
        } else {
            (
                if first { 24 } else { 32 },
                if first { 0.6_f32 } else { 0.35 },
            )
        };
        let t = (pose_time as f32).clamp(0.0, total as f32);
        let early = t < split as f32;
        let p = if early {
            t / split as f32
        } else {
            (t - split as f32) / (total - split) as f32
        };
        let mid = sit - k * mid0;
        let shift = if sitting {
            lerp(
                p,
                if early { sit } else { mid },
                if early { mid } else { low },
            )
        } else {
            lerp(
                p,
                if early { low - sit } else { low - mid },
                if early { low - mid } else { 0.0 },
            )
        };
        y += f64::from(shift);
    }
    if sitting && !in_transition {
        y += f64::from(low);
    }
    y
}

/// `Camel.getPassengerAttachmentPoint` (1041–1104) for the player in seat `index` of
/// `count`, turned by the camel's yaw.
fn camel_point(camel: &Entity, index: usize, count: usize, game_time: u64) -> [f64; 3] {
    let tick = pose_tick(camel);
    let sitting = tick < 0;
    let pose_time = i64::try_from(game_time).expect("small") - tick.abs();
    let y = camel_anchor(index == 0, sitting, pose_time) as f32;
    let z: f32 = if count > 1 && index != 0 { -0.7 } else { 0.5 };
    y_rot(
        [0.0, f64::from(y), f64::from(z * 1.0)],
        -camel.yaw * DEGREES,
    )
}

/// Where the feet of the rider in seat `index` of `count` are, on `vehicle` at the game
/// time `game_time`: the vehicle's position and its point, less the rider's attachment.
fn seated(vehicle: &Entity, index: usize, count: usize, game_time: u64) -> [f64; 3] {
    let point = match i32::from(vehicle.kind) {
        entity_types::CAMEL | entity_types::CAMEL_HUSK => {
            camel_point(vehicle, index, count, game_time)
        }
        kind => {
            let points = EntityTypeInfo::of(kind).passenger_points;
            y_rot(points[index.min(points.len() - 1)], -vehicle.yaw * DEGREES)
        }
    };
    [
        vehicle.position[0] + point[0],
        vehicle.position[1] + point[1] - PLAYER_ATTACHMENT,
        vehicle.position[2] + point[2],
    ]
}

// ------------------------------------------------------------------------------------
// The mounts, as the game makes them.

/// The records every test sets into its world, made once, and the latest clock of the
/// scratch regions that made them.
#[derive(Clone)]
struct Made {
    camel: Entity,
    camel_saddled: Entity,
    camel_sitting: Entity,
    camel_calf: Entity,
    husk: Entity,
    husk_saddled: Entity,
    ghast: Entity,
    ghast_harnessed: Entity,
    ghastling: Entity,
    nautilus_wild: Entity,
    nautilus_tame: Entity,
    nautilus_saddled: Entity,
    zombie_nautilus_tame: Entity,
    zombie_nautilus_saddled: Entity,
    horse_saddled: Entity,
    pig: Entity,
    clock: WorldClock,
}

fn made() -> &'static Made {
    static MADE: OnceLock<Made> = OnceLock::new();
    MADE.get_or_init(make)
}

/// The world's clock every test's region is given: later than every scratch region's, so
/// that each record's pose change is in the past and long over.
fn world_clock() -> WorldClock {
    let clock = made().clock;
    WorldClock {
        game_time: clock.game_time + 1_000,
        day: clock.day + 1_000,
    }
}

fn record_of(scene: &Scene, key: EntityId) -> Entity {
    scene
        .all()
        .into_iter()
        .find(|entity| entity.key == key)
        .expect("there")
}

/// The scratch region's clock, read from an idle tick.
fn clock_of(scene: &mut Scene) -> WorldClock {
    scene.idle().clock
}

/// Hatches one of `kind` by the egg in slot 0, on the floor at `at` or on the mob `on`.
fn hatch(scene: &mut Scene, kind: i32, at: BlockPos, on: Option<EntityId>) -> EntityId {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(0);
    match on {
        Some(mob) => {
            scene.input(used(mob));
        }
        None => {
            scene.click(at, clustine_sim::api::Face::Top);
        }
    }
    let made: Vec<EntityId> = scene
        .of_kind(kind)
        .iter()
        .map(|entity| entity.key)
        .filter(|key| !before.contains(key))
        .collect();
    assert_eq!(made.len(), 1, "the egg makes one of kind {kind}");
    made[0]
}

/// A stone wall two high along the four sides of the square from (-4, -4) to (5, 5).
fn pen() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut walls = Vec::new();
    for y in [-60, -59] {
        for along in -4..=5 {
            for (x, z) in [(along, -4), (along, 5), (-4, along), (5, along)] {
                walls.push((bp(x, y, z), blocks::STONE));
            }
        }
    }
    walls
}

/// A pool of still water `depth` deep over blocks -1..=7 by -2..=5, walled with stone.
fn pool(depth: i32) -> Vec<(BlockPos, clustine_data::BlockState)> {
    let water = state("minecraft:water[level=0]");
    let mut placed = Vec::new();
    for y in -60..-60 + depth {
        for x in -2..=8 {
            for z in -3..=6 {
                let edge = x == -2 || x == 8 || z == -3 || z == 6;
                placed.push((bp(x, y, z), if edge { blocks::STONE } else { water }));
            }
        }
    }
    placed
}

/// A scratch region with player 1 at the spawn holding `held` from slot 0 on.
fn scratch(more: &[(BlockPos, clustine_data::BlockState)], held: &[i32]) -> Scene {
    let slots: Vec<(usize, clustine_sim::api::ItemStack)> = held
        .iter()
        .enumerate()
        .map(|(slot, item)| (slot, stack(*item, 1)))
        .collect();
    Scene::alone(&World::of(more), inventory(&slots), SPAWN)
}

/// Uses what slot `slot` holds on `key` (not sneaking).
fn use_slot(scene: &mut Scene, slot: u8, key: EntityId) -> TickOutput {
    scene.select(slot);
    scene.input(used(key))
}

fn make() -> Made {
    let mut clocks = Vec::new();

    // Camels: the egg's, saddled, a calf, and a saddled one that its brain sat down.
    let mut scene = scratch(&pen(), &[items::CAMEL_SPAWN_EGG, items::SADDLE]);
    let camel = hatch(&mut scene, entity_types::CAMEL, bp(2, -61, 2), None);
    let bare = record_of(&scene, camel);
    let calf = hatch(&mut scene, entity_types::CAMEL, bp(0, 0, 0), Some(camel));
    let camel_calf = record_of(&scene, calf);
    use_slot(&mut scene, 1, camel);
    let camel_saddled = record_of(&scene, camel);
    assert_ne!(camel_saddled.data, bare.data, "the saddle went on");
    assert_eq!(pose_tick(&camel_saddled), 0, "a camel from an egg stands");
    let mut seen = Vec::new();
    let sat = scene.idle_until(100_000, &mut seen, |scene, _| {
        pose_tick(&record_of(scene, camel)) < 0
    });
    assert!(sat, "the camel never sat down");
    // Its sitting down over, as the test regions' later clock has it anyway.
    scene.idle_for(50);
    let camel_sitting = record_of(&scene, camel);
    assert!(pose_tick(&camel_sitting) < 0, "still sitting");
    clocks.push(clock_of(&mut scene));

    // Camel husks: the egg's and saddled.
    let mut scene = scratch(&pen(), &[items::CAMEL_HUSK_SPAWN_EGG, items::SADDLE]);
    let husk = hatch(&mut scene, entity_types::CAMEL_HUSK, bp(2, -61, 2), None);
    let husk_bare = record_of(&scene, husk);
    use_slot(&mut scene, 1, husk);
    let husk_saddled = record_of(&scene, husk);
    assert_ne!(husk_saddled.data, husk_bare.data, "the saddle went on");
    clocks.push(clock_of(&mut scene));

    // Happy ghasts: the egg's (grown), harnessed, and a ghastling.
    let mut scene = scratch(&[], &[items::HAPPY_GHAST_SPAWN_EGG, items::WHITE_HARNESS]);
    let ghast = hatch(&mut scene, entity_types::HAPPY_GHAST, bp(2, -61, 2), None);
    let ghast_bare = record_of(&scene, ghast);
    let little = hatch(
        &mut scene,
        entity_types::HAPPY_GHAST,
        bp(0, 0, 0),
        Some(ghast),
    );
    let ghastling = record_of(&scene, little);
    assert!(baby(&ghastling), "the egg on a grown one makes a ghastling");
    use_slot(&mut scene, 1, ghast);
    let ghast_harnessed = record_of(&scene, ghast);
    assert_ne!(ghast_harnessed.data, ghast_bare.data, "the harness went on");
    clocks.push(clock_of(&mut scene));

    // Nautiluses, in water: wild, tamed by pufferfish, saddled; and the zombie's.
    let (nautilus_wild, nautilus_tame, nautilus_saddled, clock) =
        nautiluses(items::NAUTILUS_SPAWN_EGG, entity_types::NAUTILUS);
    clocks.push(clock);
    let (_, zombie_nautilus_tame, zombie_nautilus_saddled, clock) = nautiluses(
        items::ZOMBIE_NAUTILUS_SPAWN_EGG,
        entity_types::ZOMBIE_NAUTILUS,
    );
    clocks.push(clock);

    // A horse, tamed by riding it and saddled.
    let mut scene = scratch(&pen(), &[items::HORSE_SPAWN_EGG, items::SADDLE]);
    let horse = hatch(&mut scene, entity_types::HORSE, bp(2, -61, 2), None);
    scene.select(2);
    for _ in 0..40_000 {
        if flags(&record_of(&scene, horse), HORSE_FLAGS) & HORSE_TAME != 0 {
            break;
        }
        if rides(&scene, 0).is_some() {
            scene.idle();
        } else {
            scene.input(used(horse));
        }
    }
    assert_ne!(
        flags(&record_of(&scene, horse), HORSE_FLAGS) & HORSE_TAME,
        0
    );
    scene.input(sneak(true));
    scene.input(sneak(false));
    scene.idle();
    use_slot(&mut scene, 1, horse);
    let horse_saddled = record_of(&scene, horse);
    clocks.push(clock_of(&mut scene));

    // A pig, to fall on a happy ghast.
    let mut scene = scratch(&[], &[items::PIG_SPAWN_EGG]);
    let pig = hatch(&mut scene, entity_types::PIG, bp(2, -61, 2), None);
    let pig = record_of(&scene, pig);
    clocks.push(clock_of(&mut scene));

    let clock = clocks
        .into_iter()
        .fold(WorldClock::default(), WorldClock::later);
    Made {
        camel: bare,
        camel_saddled,
        camel_sitting,
        camel_calf,
        husk: husk_bare,
        husk_saddled,
        ghast: ghast_bare,
        ghast_harnessed,
        ghastling,
        nautilus_wild,
        nautilus_tame,
        nautilus_saddled,
        zombie_nautilus_tame,
        zombie_nautilus_saddled,
        horse_saddled,
        pig,
        clock,
    }
}

/// A nautilus of `kind` hatched under water: wild, tamed (a pufferfish's use tames one
/// time in three, `AbstractNautilus.tryToTame` 989–1015) and saddled.
fn nautiluses(egg: i32, kind: i32) -> (Entity, Entity, Entity, WorldClock) {
    let mut scene = scratch(&pool(3), &[egg, items::PUFFERFISH, items::SADDLE]);
    let nautilus = hatch(&mut scene, kind, bp(2, -61, 2), None);
    let wild = record_of(&scene, nautilus);
    assert!(!tame(&wild), "an egg's is wild");
    scene.select(1);
    for _ in 0..1_000 {
        if tame(&record_of(&scene, nautilus)) {
            break;
        }
        scene.input(used(nautilus));
    }
    let tamed = record_of(&scene, nautilus);
    assert!(tame(&tamed), "kind {kind} never tamed");
    use_slot(&mut scene, 2, nautilus);
    let saddled = record_of(&scene, nautilus);
    assert_ne!(saddled.data, tamed.data, "the saddle went on");
    (wild, tamed, saddled, clock_of(&mut scene))
}

/// `record` as entity number `n` at `at`, facing `yaw`, at rest.
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
// Scenes.

/// A region alone with `entities` among the blocks `more`, `players` players at
/// [`PLAYERS_AT`] holding [`hotbar`], slot 0 selected, and the world's clock.
fn scene_of(
    more: &[(BlockPos, clustine_data::BlockState)],
    entities: Vec<Entity>,
    players: usize,
) -> Scene {
    let world = World::of(more).with_entities(&entities);
    let mut scene = Scene::alone(&world, hotbar(), PLAYERS_AT[0]);
    for (n, feet) in (2..).zip(&PLAYERS_AT[1..players]) {
        scene.join(n, *feet);
    }
    at_world_time(&mut scene);
    scene
}

/// The store publishes the world's clock to the scene's region.
fn at_world_time(scene: &mut Scene) -> TickOutput {
    scene.region.tick(&TickInputs {
        clock: Some(world_clock()),
        ..TickInputs::default()
    })
}

/// `mount` as [`MOUNT`] at `at` facing `yaw`, with `players` players.
fn with_mount(mount: &Entity, at: [f64; 3], yaw: f32, players: usize) -> Scene {
    scene_of(&[], vec![set(mount, MOUNT, at, yaw)], players)
}

/// Each of the players `who` clicks the mount with an empty hand, in turn; asserts that
/// each got on.
fn all_on(scene: &mut Scene, who: &[usize]) {
    for &n in who {
        scene.input_of(n, used(key(MOUNT)));
        assert_eq!(rides(scene, n), Some(key(MOUNT)), "player {} is on", n + 1);
    }
}

// ------------------------------------------------------------------------------------
// Inputs.

/// A use of `key` (`interact`, main hand, not sneaking).
fn used(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    }
}

fn used_sneaking(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: true,
    }
}

fn turn(yaw: f32, pitch: f32) -> PlayerInput {
    PlayerInput::Move {
        position: None,
        rotation: Some((yaw, pitch)),
        on_ground: true,
    }
}

fn drive(to: [f64; 3], on_ground: bool) -> PlayerInput {
    PlayerInput::MoveVehicle(VehicleMove {
        position: Vec3::new(to[0], to[1], to[2]),
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
    })
}

fn sneak(down: bool) -> PlayerInput {
    PlayerInput::Keys(Keys {
        sneak: down,
        ..Keys::default()
    })
}

fn jump(power: i32) -> PlayerInput {
    PlayerInput::RidingJump { power }
}

// ------------------------------------------------------------------------------------
// What the region says.

fn mount_of(scene: &Scene, n: u32) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key(n))
}

fn the_mount(scene: &Scene) -> Entity {
    mount_of(scene, MOUNT).expect("the mount is there")
}

fn rides(scene: &Scene, who: usize) -> Option<EntityId> {
    scene
        .region
        .player_state(scene.actors[who].id)
        .expect("there")
        .riding
        .map(|riding| riding.vehicle)
}

fn pose(scene: &Scene, who: usize) -> clustine_sim::api::Pose {
    scene.region.player(scene.actors[who].id).expect("there").1
}

fn feet(scene: &Scene, who: usize) -> Vec3 {
    pose(scene, who).position
}

fn passengers(entity: &Entity) -> Vec<EntityId> {
    entity
        .riding
        .passengers
        .iter()
        .map(|seat| seat.key)
        .collect()
}

fn entities_of(scene: &Scene, who: &[usize]) -> Vec<EntityId> {
    who.iter().map(|&n| scene.actors[n].entity).collect()
}

fn shown_at(entity: &Entity, at: u8) -> Option<DataShown> {
    mob_values(entity.kind, &entity.data)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, _)| value)
}

fn flags(entity: &Entity, at: u8) -> u8 {
    match shown_at(entity, at) {
        Some(DataShown::Byte(flags)) => flags,
        other => panic!("flags at {at}: {other:?}"),
    }
}

fn tame(nautilus: &Entity) -> bool {
    flags(nautilus, TAME_FLAGS) & TAME_BIT != 0
}

fn baby(entity: &Entity) -> bool {
    shown_at(entity, clustine_sim::shown::index::BABY) == Some(DataShown::Boolean(true))
}

/// A camel's `LAST_POSE_CHANGE_TICK`: negative while it sits (B 7.7.4).
fn pose_tick(camel: &Entity) -> i64 {
    match shown_at(camel, CAMEL_POSE_TICK) {
        Some(DataShown::Long(tick)) => tick,
        other => panic!("a camel shows its pose tick as a long at 20: {other:?}"),
    }
}

fn dashing(entity: &Entity) -> bool {
    let at = match i32::from(entity.kind) {
        entity_types::CAMEL | entity_types::CAMEL_HUSK => CAMEL_DASH,
        _ => NAUTILUS_DASH,
    };
    match shown_at(entity, at) {
        Some(DataShown::Boolean(dash)) => dash,
        other => panic!("DASH at {at}: {other:?}"),
    }
}

fn corrections(output: &TickOutput, scene: &Scene, who: usize) -> Vec<Vec3> {
    events_of(output, scene.actors[who].id)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::VehicleCorrected { position, .. } => Some(position),
            _ => None,
        })
        .collect()
}

fn rides_told(output: &TickOutput, scene: &Scene, who: usize) -> Vec<(Option<EntityId>, Vec3)> {
    events_of(output, scene.actors[who].id)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::Rides { vehicle, pose } => Some((vehicle, pose.position)),
            _ => None,
        })
        .collect()
}

fn kicked(output: &TickOutput, scene: &Scene, who: usize) -> bool {
    events_of(output, scene.actors[who].id)
        .iter()
        .any(|event| matches!(event, PlayerEvent::Kicked { .. }))
}

/// The sounds of the tick of the id `id`, with where each plays, how loud and how high.
fn sounds_of(output: &TickOutput, id: i32) -> Vec<(Vec3, f32, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                position,
                volume,
                pitch,
                ..
            } if *sound == id => Some((*position, *volume, *pitch)),
            _ => None,
        })
        .collect()
}

/// Whom the tick's sounds of the id `id` leave out: none for a sound every player near
/// hears (`Level.playSound(null, …)`).
fn sound_excepts(output: &TickOutput, id: i32) -> Vec<Option<clustine_world::PlayerId>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, except, .. } if *sound == id => Some(*except),
            _ => None,
        })
        .collect()
}

/// How many of the sound `id` the tick played, at a place or at an entity.
fn played(output: &TickOutput, id: i32) -> usize {
    output
        .events
        .iter()
        .filter(|event| match event {
            RegionEvent::Sound { sound, .. } | RegionEvent::EntitySound { sound, .. } => {
                *sound == id
            }
            _ => false,
        })
        .count()
}

/// The flags of the tick's reports of `key`.
fn tracked_flags(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityTracked(tracked) if tracked.entity == key => Some(tracked.flags),
            _ => None,
        })
        .collect()
}

fn bits(at: [f64; 3]) -> [u64; 3] {
    at.map(f64::to_bits)
}

fn near(a: Vec3, b: [f64; 3], within: f64) -> bool {
    (a.x - b[0]).abs() <= within && (a.y - b[1]).abs() <= within && (a.z - b[2]).abs() <= within
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The difference of two yaws, in degrees, the short way round.
fn yaw_apart(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Whether `who`'s move of the mount to `to` was taken: no correction, and the mount
/// where they put it.
fn steers(scene: &mut Scene, who: usize, to: [f64; 3], on_ground: bool) -> bool {
    let output = scene.input_of(who, drive(to, on_ground));
    corrections(&output, scene, who).is_empty() && bits(the_mount(scene).position) == bits(to)
}

/// A place a little east of where the mount is now, `step` tenths of a block.
fn beside(scene: &Scene, step: u32) -> [f64; 3] {
    let at = the_mount(scene).position;
    [at[0] + 0.1 * f64::from(step), at[1], at[2]]
}

/// `who` sneaks; the tick's output in which their client is told they got off, and the
/// place they got off to. A dismount by the shift key happens in the rider's own tick
/// (`Player.rideTick`), which may be the next.
fn sneak_off(scene: &mut Scene, who: usize) -> (TickOutput, Vec3) {
    let mut output = scene.input_of(who, sneak(true));
    for _ in 0..2 {
        if !rides_told(&output, scene, who).is_empty() {
            break;
        }
        output = scene.idle();
    }
    let told = rides_told(&output, scene, who);
    let [(None, at)] = told[..] else {
        panic!("one Rides with nothing: {told:?}");
    };
    scene.input_of(who, sneak(false));
    (output, at)
}

/// The riders' feet against their seats on the mount at the game time of `output`.
fn assert_seated(scene: &Scene, who: &[usize], output: &TickOutput, within: f64, when: &str) {
    let mount = the_mount(scene);
    assert_eq!(
        passengers(&mount),
        entities_of(scene, who),
        "{when}: the riders, in order"
    );
    for (index, &n) in who.iter().enumerate() {
        let expected = seated(&mount, index, who.len(), output.clock.game_time);
        let at = feet(scene, n);
        assert!(
            near(at, expected, within),
            "{when}: player {} in seat {index} of {} at {at:?}, expected {expected:?} \
             (the mount at {:?}, yaw {}, game time {})",
            n + 1,
            who.len(),
            mount.position,
            mount.yaw,
            output.clock.game_time
        );
    }
}

// ------------------------------------------------------------------------------------
// 1. Who gets on, and how many (R §1.1–1.3; B 7.7.5, 7.11.4, 7.12.4).

/// R §1.1, `Camel.mobInteract` 768–832: fewer than two passengers and not a baby →
/// `doPlayerRide`, saddled or not (only steering needs a saddle). A third player's click
/// finds two and is refused (`FAIL`), though `canAddPassenger` would take a third.
#[test]
fn a_camel_takes_two_players_by_a_click_with_or_without_a_saddle_and_no_third() {
    let made = made();
    for camel in [
        &made.camel,
        &made.camel_saddled,
        &made.husk,
        &made.husk_saddled,
    ] {
        let mut scene = with_mount(camel, MOUNT_AT, 0.0, 3);
        all_on(&mut scene, &[0, 1]);
        assert_eq!(
            passengers(&the_mount(&scene)),
            entities_of(&scene, &[0, 1]),
            "kind {}: the first to click sits first",
            camel.kind
        );
        scene.input_of(2, used(key(MOUNT)));
        assert_eq!(rides(&scene, 2), None, "kind {}: no third", camel.kind);
        assert_eq!(passengers(&the_mount(&scene)).len(), 2);
    }
}

/// `doPlayerRide` (`AbstractHorse` 1066–1090) sets the player's yaw and pitch to the
/// camel's, and `AbstractHorse.addPassenger` (1782–1795) to its view: each rider who
/// looked elsewhere faces where the camel looks (within the 10° a head turns in a tick),
/// at its pitch, 0.
#[test]
fn each_player_taken_on_a_camel_is_turned_to_it() {
    let made = made();
    for camel in [&made.camel, &made.husk_saddled] {
        let mut scene = with_mount(camel, MOUNT_AT, 70.0, 2);
        for n in [0, 1] {
            scene.input_of(n, turn(-120.0, 35.0));
            let looks = head_yaw(&the_mount(&scene).data).expect("a mob's head");
            scene.input_of(n, used(key(MOUNT)));
            assert_eq!(rides(&scene, n), Some(key(MOUNT)));
            let rider = pose(&scene, n);
            assert_eq!(rider.pitch, 0.0, "player {}: the camel's pitch", n + 1);
            assert!(
                yaw_apart(rider.yaw, looks) <= 10.0,
                "player {} faces {}, the camel looked to {looks}",
                n + 1,
                rider.yaw
            );
        }
    }
}

/// `Camel.mobInteract`: the ride is only for one that is not a baby.
#[test]
fn a_camel_calf_takes_nobody() {
    let mut scene = with_mount(&made().camel_calf, MOUNT_AT, 0.0, 1);
    scene.input(used(key(MOUNT)));
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&the_mount(&scene)).is_empty());
}

/// `Camel.mobInteract` 1: sneaking (and not a baby) opens its saddle menu instead; and
/// `Entity.canRide` refuses a rider whose shift key is down anyway.
#[test]
fn a_player_who_sneaks_is_not_taken_on_a_camel() {
    let mut scene = with_mount(&made().camel_saddled, MOUNT_AT, 0.0, 1);
    scene.input(used_sneaking(key(MOUNT)));
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&the_mount(&scene)).is_empty());
}

/// R §1.2, `HappyGhast.mobInteract` 482–525 and `canAddPassenger` 592–602: a harnessed
/// grown ghast takes a player who does not sneak, up to four (`size < 4`); a fifth's
/// click finds it full. `doPlayerRide` (527–537) is `startRiding` alone: nobody is
/// turned.
#[test]
fn a_harnessed_grown_happy_ghast_takes_four_players_without_turning_them_and_no_fifth() {
    let mut scene = with_mount(&made().ghast_harnessed, GHAST_AT, 0.0, 5);
    let looks = [
        (-120.0_f32, 35.0_f32),
        (60.0, -20.0),
        (10.0, 5.0),
        (170.0, 80.0),
    ];
    for (n, (yaw, pitch)) in looks.iter().enumerate() {
        scene.input_of(n, turn(*yaw, *pitch));
        scene.input_of(n, used(key(MOUNT)));
        assert_eq!(rides(&scene, n), Some(key(MOUNT)), "player {} is on", n + 1);
        let rider = pose(&scene, n);
        assert_eq!(
            (rider.yaw, rider.pitch),
            (*yaw, *pitch),
            "player {} is not turned",
            n + 1
        );
    }
    assert_eq!(
        passengers(&the_mount(&scene)),
        entities_of(&scene, &[0, 1, 2, 3])
    );
    scene.input_of(4, used(key(MOUNT)));
    assert_eq!(rides(&scene, 4), None, "no fifth");
    assert_eq!(passengers(&the_mount(&scene)).len(), 4);
}

/// `HappyGhast.mobInteract`: `!player.isSecondaryUseActive()`.
#[test]
fn a_happy_ghast_takes_nobody_who_sneaks() {
    let mut scene = with_mount(&made().ghast_harnessed, GHAST_AT, 0.0, 1);
    scene.input(used_sneaking(key(MOUNT)));
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&the_mount(&scene)).is_empty());
}

/// `HappyGhast.mobInteract`: the ride needs a harness (`isWearingBodyArmor`), and a
/// ghastling goes to `Animal.mobInteract`, which mounts nobody.
#[test]
fn a_happy_ghast_without_a_harness_and_a_ghastling_take_nobody() {
    let made = made();
    for ghast in [&made.ghast, &made.ghastling] {
        let mut scene = with_mount(ghast, GHAST_AT, 0.0, 1);
        scene.input(used(key(MOUNT)));
        assert_eq!(rides(&scene, 0), None, "baby: {}", baby(ghast));
        assert!(passengers(&the_mount(&scene)).is_empty());
    }
}

/// R §1.3, `AbstractNautilus.mobInteract` 889–987 and `canAddPassenger` 253–261 (`!isVehicle()`):
/// a tame nautilus takes a player who does not sneak and holds no food, saddled or not,
/// and only one; both kinds.
#[test]
fn a_tame_nautilus_takes_one_player_and_no_second() {
    let made = made();
    for nautilus in [
        &made.nautilus_tame,
        &made.nautilus_saddled,
        &made.zombie_nautilus_tame,
        &made.zombie_nautilus_saddled,
    ] {
        let mut scene = with_mount(nautilus, MOUNT_AT, 0.0, 2);
        all_on(&mut scene, &[0]);
        scene.input_of(1, used(key(MOUNT)));
        assert_eq!(rides(&scene, 1), None, "kind {}: no second", nautilus.kind);
        assert_eq!(passengers(&the_mount(&scene)), [scene.me()]);
    }
}

/// `AbstractNautilus.mobInteract`: the ride is only for a tame one; and a tame one clicked
/// by a player who sneaks opens its menu instead.
#[test]
fn a_wild_nautilus_and_a_sneaking_click_take_nobody() {
    let made = made();
    let mut scene = with_mount(&made.nautilus_wild, MOUNT_AT, 0.0, 1);
    scene.input(used(key(MOUNT)));
    assert_eq!(rides(&scene, 0), None, "wild");
    let mut scene = with_mount(&made.nautilus_saddled, MOUNT_AT, 0.0, 1);
    scene.input(used_sneaking(key(MOUNT)));
    assert_eq!(rides(&scene, 0), None, "sneaking");
    assert!(passengers(&the_mount(&scene)).is_empty());
}

/// What the clicker holds: the camel's food (cactus) goes to `fedFood` first, which an
/// adult at full health eats to fall in love (`handleEating`: tamed, always, and of age),
/// so nobody gets on; a tame nautilus's food (`#nautilus_food`, cod among `#fishes`) is
/// refused by the ride's `!isFood(stack)`; and a snowball, a ghastling's food, consumes
/// nothing on a grown ghast, so the ride goes on (R §1.2).
#[test]
fn food_stops_the_ride_on_a_camel_and_a_nautilus_and_a_snowball_does_not_on_a_ghast() {
    let made = made();
    for (mount, at, slot, rides_then) in [
        (&made.camel, MOUNT_AT, CACTUS, false),
        (&made.nautilus_saddled, MOUNT_AT, COD, false),
        (&made.zombie_nautilus_saddled, MOUNT_AT, COD, false),
        (&made.ghast_harnessed, GHAST_AT, SNOWBALL, true),
    ] {
        let mut scene = with_mount(mount, at, 0.0, 1);
        scene.select(slot);
        scene.input(used(key(MOUNT)));
        assert_eq!(
            rides(&scene, 0).is_some(),
            rides_then,
            "kind {}, holding slot {slot}",
            mount.kind
        );
    }
}

/// R §4: `Mob.canShearEquipment` is `!isVehicle()` (Mob 1222–1230): shears used on a
/// ridden camel or ghast take neither its saddle nor its harness, and its rider steers it
/// on.
#[test]
fn shears_take_no_saddle_or_harness_off_a_ridden_mount() {
    let made = made();
    for (mount, at) in [
        (&made.camel_saddled, MOUNT_AT),
        (&made.ghast_harnessed, GHAST_AT),
    ] {
        let mut scene = with_mount(mount, at, 0.0, 2);
        all_on(&mut scene, &[0]);
        scene.idle();
        use_slot(&mut scene, SHEARS, key(MOUNT));
        scene.input_of(1, PlayerInput::SelectSlot { slot: SHEARS });
        scene.input_of(1, used(key(MOUNT)));
        assert_eq!(
            lying_count(&scene.all(), items::SADDLE)
                + lying_count(&scene.all(), items::WHITE_HARNESS),
            0,
            "kind {}: nothing shorn off",
            mount.kind
        );
        scene.idle();
        let to = beside(&scene, 5);
        assert!(
            steers(&mut scene, 0, to, true),
            "kind {}: still steered",
            mount.kind
        );
    }
    // The same shears on one nobody rides take its saddle off (`attemptToShearEquipment`).
    let mut scene = with_mount(&made.camel_saddled, MOUNT_AT, 0.0, 1);
    use_slot(&mut scene, SHEARS, key(MOUNT));
    assert_eq!(
        lying_count(&scene.all(), items::SADDLE),
        1,
        "shorn off unridden"
    );
}

// ------------------------------------------------------------------------------------
// 2. Seats (R §1.4; B 7.7.5's formula).

/// R §1.4: a standing camel (its stand long over) seats its first rider 0.5 ahead and,
/// with two aboard, its second 0.7 behind, both at `2.375 − 0.375` = 2.0 above it, so
/// their feet are 1.4 above its own; turned by its yaw. A lone rider sits 0.5 ahead.
#[test]
fn a_standing_camels_riders_sit_half_a_block_ahead_and_seven_tenths_behind() {
    let made = made();
    for camel in [&made.camel_saddled, &made.husk_saddled] {
        let mut scene = with_mount(camel, MOUNT_AT, 0.0, 2);
        all_on(&mut scene, &[0]);
        let output = scene.idle();
        assert_seated(&scene, &[0], &output, 1e-6, "alone");
        let mount = the_mount(&scene);
        let ahead = y_rot([0.0, 0.0, 0.5], -mount.yaw * DEGREES);
        let alone = feet(&scene, 0);
        assert!(
            near(
                alone,
                [
                    mount.position[0] + ahead[0],
                    mount.position[1] + 1.4,
                    mount.position[2] + ahead[2]
                ],
                1e-6
            ),
            "the formula's numbers: {alone:?}"
        );
        all_on(&mut scene, &[1]);
        for yaw in [0.0_f32, 90.0, -135.0] {
            scene.input(turn(yaw, 0.0));
            let output = scene.idle();
            assert_eq!(the_mount(&scene).yaw, yaw, "steered to {yaw}");
            assert_seated(&scene, &[0, 1], &output, 1e-6, &format!("yaw {yaw}"));
        }
    }
}

/// R §1.4: a sitting camel (its sitting down long over) seats its riders at `0.945 −
/// 0.375 + low`, about 0.77 above it, ahead and behind as standing.
#[test]
fn a_sitting_camels_riders_sit_at_its_sitting_height() {
    let mut scene = with_mount(&made().camel_sitting, MOUNT_AT, 0.0, 2);
    all_on(&mut scene, &[0, 1]);
    let output = scene.idle();
    assert!(pose_tick(&the_mount(&scene)) < 0, "it sits");
    assert_seated(&scene, &[0, 1], &output, 1e-6, "sitting");
    let mount = the_mount(&scene);
    assert!(
        (feet(&scene, 0).y - (mount.position[1] + 0.77 - PLAYER_ATTACHMENT)).abs() < 0.01,
        "about 0.77 up: {:?}",
        feet(&scene, 0)
    );
}

/// R §1.4 and B 7.7.4: between the poses the riders' height follows the animation by
/// `getPoseTime()`, the world's game time less the pose's tick. The sitting camel's
/// region is given a clock five ticks after it sat down, so its riders go down with it
/// tick by tick, and are at the sitting height once its 40 ticks are over.
#[test]
fn a_camel_sitting_down_moves_its_riders_by_the_game_time() {
    let camel = &made().camel_sitting;
    let sat_at = pose_tick(camel).unsigned_abs();
    let world = World::default().with_entities(&[set(camel, MOUNT, MOUNT_AT, 0.0)]);
    let mut scene = Scene::alone(&world, hotbar(), PLAYERS_AT[0]);
    scene.join(2, PLAYERS_AT[1]);
    scene.region.tick(&TickInputs {
        clock: Some(WorldClock {
            game_time: sat_at + 2,
            day: sat_at + 2,
        }),
        ..TickInputs::default()
    });
    all_on(&mut scene, &[0, 1]);
    let mut checked_between = false;
    for tick in 0..50 {
        let output = scene.idle();
        let pose_time =
            i64::try_from(output.clock.game_time).expect("small") - pose_tick(camel).abs();
        checked_between |= pose_time < 40;
        assert_seated(&scene, &[0, 1], &output, 1e-6, &format!("tick {tick}"));
    }
    assert!(checked_between, "the test saw the camel between its poses");
}

/// The same for a camel standing up: a camel from an egg stood up at the game time 0
/// (`standUpInstantly` in `finalizeSpawn` puts its tick at `max(0, time − 53)`), and a
/// region whose clock is young (not given the world's) has it in its 52 ticks of
/// standing up, its riders rising with it.
#[test]
fn a_camel_standing_up_moves_its_riders_by_the_game_time() {
    let camel = &made().camel_saddled;
    assert_eq!(pose_tick(camel), 0);
    let world = World::default().with_entities(&[set(camel, MOUNT, MOUNT_AT, 0.0)]);
    let mut scene = Scene::alone(&world, hotbar(), PLAYERS_AT[0]);
    scene.join(2, PLAYERS_AT[1]);
    all_on(&mut scene, &[0, 1]);
    let mut checked_between = false;
    for tick in 0..60 {
        let output = scene.idle();
        checked_between |= output.clock.game_time < 52;
        assert_seated(&scene, &[0, 1], &output, 1e-6, &format!("tick {tick}"));
    }
    assert!(checked_between, "the test saw the camel standing up");
}

/// R §1.4: the happy ghast overrides no attachment point: its four riders sit at the
/// type's four points, (0, 4, 1.7), (−1.7, 4, 0), (0, 4, −1.7) and (1.7, 4, 0), in seat
/// order, turned by its yaw; their feet 3.4 above its own.
#[test]
fn a_happy_ghasts_four_riders_sit_at_its_four_points_turned_by_its_yaw() {
    let points = EntityTypeInfo::of(entity_types::HAPPY_GHAST).passenger_points;
    assert_eq!(
        points,
        [
            [0.0, 4.0, 1.7],
            [-1.7, 4.0, 0.0],
            [0.0, 4.0, -1.7],
            [1.7, 4.0, 0.0]
        ],
        "the extracted points"
    );
    let mut scene = with_mount(&made().ghast_harnessed, GHAST_AT, 0.0, 4);
    all_on(&mut scene, &[0, 1, 2, 3]);
    scene.input(turn(100.0, 0.0));
    let mut turned = false;
    for tick in 0..30 {
        let output = scene.idle();
        turned |= yaw_apart(the_mount(&scene).yaw, 0.0) > 10.0;
        assert_seated(
            &scene,
            &[0, 1, 2, 3],
            &output,
            1e-6,
            &format!("tick {tick}"),
        );
    }
    assert!(turned, "the ghast turned toward its driver");
    let mount = the_mount(&scene);
    assert!(close(feet(&scene, 0).y, mount.position[1] + 3.4));
}

/// R §1.4: a nautilus's rider sits at its type's point, (0, 1.1375, 0).
#[test]
fn a_nautiluses_rider_sits_at_its_passenger_point() {
    let made = made();
    for nautilus in [&made.nautilus_saddled, &made.zombie_nautilus_tame] {
        let point = EntityTypeInfo::of(i32::from(nautilus.kind)).passenger_points[0];
        assert!(close(point[1], 1.1375) && point[0] == 0.0 && point[2] == 0.0);
        let mut scene = with_mount(nautilus, MOUNT_AT, 0.0, 1);
        all_on(&mut scene, &[0]);
        let output = scene.idle();
        assert_seated(&scene, &[0], &output, 1e-6, "on the nautilus");
    }
}

// ------------------------------------------------------------------------------------
// 3. Who steers (R §1.5, §2.5).

/// R §1.5, `AbstractHorse.getControllingPassenger` 2420–2438: saddled and a player first:
/// that player's vehicle moves are taken; the second rider's are not.
#[test]
fn the_first_rider_of_a_saddled_camel_steers_it_and_the_second_does_not() {
    let made = made();
    for camel in [&made.camel_saddled, &made.husk_saddled] {
        let mut scene = with_mount(camel, MOUNT_AT, 0.0, 2);
        all_on(&mut scene, &[0, 1]);
        scene.idle();
        let to = beside(&scene, 5);
        assert!(
            steers(&mut scene, 0, to, true),
            "kind {}: the first",
            camel.kind
        );
        let to = beside(&scene, 3);
        assert!(
            !steers(&mut scene, 1, to, true),
            "kind {}: the second",
            camel.kind
        );
    }
}

/// `getControllingPassenger` with no saddle: `Mob`'s, which a player is not.
#[test]
fn nobody_steers_a_camel_without_a_saddle() {
    let mut scene = with_mount(&made().camel, MOUNT_AT, 0.0, 2);
    all_on(&mut scene, &[0, 1]);
    scene.idle();
    for n in [0, 1] {
        let to = beside(&scene, 5);
        assert!(!steers(&mut scene, n, to, true), "player {}", n + 1);
    }
}

/// R §1.5, `HappyGhast.getControllingPassenger` 604–625: the first rider of a harnessed
/// ghast not on its still timeout (mounting with nobody on top clears it, `addPassenger`
/// 539–568) steers it; the others do not.
#[test]
fn the_first_rider_of_a_harnessed_happy_ghast_steers_it_and_the_others_do_not() {
    let mut scene = with_mount(&made().ghast_harnessed, GHAST_AT, 0.0, 3);
    all_on(&mut scene, &[0, 1, 2]);
    scene.idle();
    let to = beside(&scene, 5);
    assert!(steers(&mut scene, 0, to, true), "the first");
    for n in [1, 2] {
        let to = beside(&scene, 3);
        assert!(!steers(&mut scene, n, to, true), "player {}", n + 1);
    }
}

/// R §1.5, `AbstractNautilus.getControllingPassenger` 263–281: saddled and a player.
#[test]
fn the_rider_of_a_saddled_nautilus_steers_it_and_of_a_bare_one_does_not() {
    let made = made();
    for (nautilus, steered) in [
        (&made.nautilus_saddled, true),
        (&made.zombie_nautilus_saddled, true),
        (&made.nautilus_tame, false),
        (&made.zombie_nautilus_tame, false),
    ] {
        let mut scene = with_mount(nautilus, MOUNT_AT, 0.0, 1);
        all_on(&mut scene, &[0]);
        scene.idle();
        let to = beside(&scene, 5);
        assert_eq!(
            steers(&mut scene, 0, to, true),
            steered,
            "kind {}, saddled {steered}",
            nautilus.kind
        );
    }
}

/// R §7: control is the first seat's, and changes hands when the first rider gets off;
/// the game's `lastVehicle` is set at the end of the tick (n5-brief 2.2), so the second
/// rider's moves are taken from the tick after.
#[test]
fn when_the_first_rider_of_a_camel_gets_off_the_second_steers_it() {
    let mut scene = with_mount(&made().camel_saddled, MOUNT_AT, 0.0, 2);
    all_on(&mut scene, &[0, 1]);
    scene.idle();
    sneak_off(&mut scene, 0);
    assert_eq!(passengers(&the_mount(&scene)), entities_of(&scene, &[1]));
    scene.idle();
    let to = beside(&scene, 5);
    assert!(steers(&mut scene, 1, to, true), "the second steers");
    let output = scene.idle();
    assert_seated(&scene, &[1], &output, 1e-6, "alone in front");
}

// ------------------------------------------------------------------------------------
// 4. Dashes (R §3).

/// A saddled camel (or husk) with player 1 on it, standing settled; the scene.
fn dashing_camel(camel: &Entity) -> Scene {
    let mut scene = with_mount(camel, MOUNT_AT, 0.0, 1);
    all_on(&mut scene, &[0]);
    scene.idle();
    scene
}

/// R §3: `handlePlayerCommand(START_RIDING_JUMP)` with `canJump()` (saddled, not
/// refusing) and power above 0 → `handleStartJump`: `makeSound(CAMEL_DASH)` at the
/// camel, at `getSoundVolume()` = 0.8 and the voice pitch `(r.nextFloat() −
/// r.nextFloat()) · 0.2 + 1` (two draws, so within 0.8 to 1.2 and, drawn, not 1), and
/// `DASH` set. The husk's is its own sound.
#[test]
fn a_camels_dash_sounds_at_eight_tenths_with_a_drawn_pitch_and_shows_it_dashing() {
    let made = made();
    for (camel, dash) in [
        (&made.camel_saddled, sound::ENTITY_CAMEL_DASH),
        (&made.husk_saddled, sound::ENTITY_CAMEL_HUSK_DASH),
    ] {
        let mut scene = dashing_camel(camel);
        assert!(!dashing(&the_mount(&scene)));
        let output = scene.input(jump(100));
        let mount = the_mount(&scene);
        let heard = sounds_of(&output, dash);
        assert_eq!(
            heard.len(),
            1,
            "kind {}: one dash sound: {heard:?}",
            camel.kind
        );
        let (at, volume, pitch) = heard[0];
        assert_eq!(volume, CAMEL_VOICE_VOLUME, "kind {}", camel.kind);
        assert!(
            (0.8..=1.2).contains(&pitch) && pitch != 1.0,
            "kind {}: a drawn pitch {pitch}",
            camel.kind
        );
        assert!(near(at, mount.position, 1e-9), "at the camel: {at:?}");
        assert_eq!(
            sound_excepts(&output, dash),
            [None],
            "heard by all, its rider too"
        );
        assert!(dashing(&mount), "kind {}: DASH", camel.kind);
    }
}

/// R §3: no cooldown is tested on the server, so each command sounds again, each pitch
/// drawn anew.
#[test]
fn each_dash_command_sounds_with_a_pitch_of_its_own() {
    let mut scene = dashing_camel(&made().camel_saddled);
    let mut pitches = Vec::new();
    for _ in 0..3 {
        let output = scene.input(jump(100));
        let heard = sounds_of(&output, sound::ENTITY_CAMEL_DASH);
        assert_eq!(heard.len(), 1, "{heard:?}");
        pitches.push(heard[0].2);
        scene.idle_for(10);
    }
    assert!(
        pitches[0] != pitches[1] || pitches[1] != pitches[2],
        "drawn each time: {pitches:?}"
    );
}

/// R §3 and P §9 item 7: `setDashing(true)` runs `onSyncedDataUpdated` on the server too,
/// which starts `dashCooldown` at 55 (B 7.7.5 says this of the client only; the dumps'
/// `SynchedEntityData.set` calls it on either side wherever the value changes, and
/// `Camel.onSyncedDataUpdated` 1526–1548 has no side test, so the dumps are followed);
/// `Camel.tick` (293–359) counts it down once a tick,
/// the command's own tick among them, and at 0 plays the ready sound at the camel's block
/// centre, volume 1 and pitch 1: on the 55th tick counting the command's (T + 54). The
/// dash itself ends in the first tick that finds the cooldown under 50 on the ground:
/// the sixth after the command's.
#[test]
fn a_camel_sounds_ready_on_the_fifty_fifth_tick_counting_its_dash_and_stops_dashing_on_the_sixth() {
    let made = made();
    for (camel, ready) in [
        (&made.camel_saddled, sound::ENTITY_CAMEL_DASH_READY),
        (&made.husk_saddled, sound::ENTITY_CAMEL_HUSK_DASH_READY),
    ] {
        let mut scene = dashing_camel(camel);
        let mut outputs = vec![scene.input(jump(80))];
        for tick in 1..=CAMEL_COOLDOWN + 5 {
            outputs.push(scene.idle());
            let mount = the_mount(&scene);
            assert_eq!(
                dashing(&mount),
                tick < 6,
                "kind {}: DASH {tick} ticks after the command",
                camel.kind
            );
        }
        let heard: Vec<(usize, (Vec3, f32, f32))> = outputs
            .iter()
            .enumerate()
            .flat_map(|(tick, output)| {
                sounds_of(output, ready)
                    .into_iter()
                    .map(move |sound| (tick, sound))
            })
            .collect();
        assert_eq!(
            heard.len(),
            1,
            "kind {}: one ready sound: {heard:?}",
            camel.kind
        );
        let (tick, (at, volume, pitch)) = heard[0];
        assert_eq!(tick, CAMEL_COOLDOWN as usize - 1, "kind {}", camel.kind);
        assert_eq!((volume, pitch), (1.0, 1.0));
        assert_eq!(
            sound_excepts(&outputs[tick], ready),
            [None],
            "R §3: to every player near, the rider among them"
        );
        let block = the_mount(&scene).position.map(f64::floor);
        assert!(
            near(at, [block[0] + 0.5, block[1] + 0.5, block[2] + 0.5], 1e-9),
            "at the block's centre: {at:?}"
        );
    }
}

/// P §9 item 7: a second command while the cooldown runs sounds the dash again and sets
/// `DASH` again (a change, so `onSyncedDataUpdated` runs), but the cooldown is not 0, so
/// it is not restarted: the ready sound still comes on the 55th tick counting the first.
/// That `DASH` ends within the same tick (the cooldown is under 50, on the ground).
#[test]
fn a_second_dash_command_sounds_again_but_does_not_put_off_the_ready_sound() {
    let mut scene = dashing_camel(&made().camel_saddled);
    let mut outputs = vec![scene.input(jump(100))];
    for tick in 1..=CAMEL_COOLDOWN + 5 {
        let output = if tick == 10 {
            let output = scene.input(jump(100));
            assert_eq!(sounds_of(&output, sound::ENTITY_CAMEL_DASH).len(), 1);
            assert!(!dashing(&the_mount(&scene)), "over within the tick");
            output
        } else {
            scene.idle()
        };
        outputs.push(output);
    }
    let ready: Vec<usize> = outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| played(output, sound::ENTITY_CAMEL_DASH_READY) > 0)
        .map(|(tick, _)| tick)
        .collect();
    assert_eq!(ready, [CAMEL_COOLDOWN as usize - 1]);
}

/// R §3, `Camel.canJump` (590–602): `!refuseToMove() && saddled`. A sitting camel and one
/// without a saddle take no dash: no sound, no `DASH`, no ready sound after.
#[test]
fn a_sitting_camel_and_one_without_a_saddle_do_not_dash() {
    let made = made();
    for camel in [&made.camel_sitting, &made.camel] {
        let mut scene = with_mount(camel, MOUNT_AT, 0.0, 1);
        all_on(&mut scene, &[0]);
        scene.idle();
        let mut outputs = vec![scene.input(jump(100))];
        assert!(
            !dashing(&the_mount(&scene)),
            "sitting {}",
            pose_tick(camel) < 0
        );
        outputs.extend(scene.idle_for(CAMEL_COOLDOWN + 2));
        for output in &outputs {
            assert_eq!(played(output, sound::ENTITY_CAMEL_DASH), 0);
            assert_eq!(played(output, sound::ENTITY_CAMEL_DASH_READY), 0);
        }
    }
}

/// R §3: `AbstractNautilus.canJump` = saddled; `handleStartJump` (817–829) sounds
/// `getDashSound()` by the medium (`Nautilus`: under water by its eyes, else on land),
/// volume 1 and a drawn pitch, and sets `DASH`; its cooldown of 40 starts with the
/// change, and `AbstractNautilus.tick` (682–725) sounds it ready by `makeSound` on the
/// 40th tick counting the command's. Both kinds, in water and on land.
#[test]
fn a_nautilus_dashes_and_sounds_ready_by_its_medium() {
    let made = made();
    for (nautilus, water_sounds, land_sounds) in [
        (
            &made.nautilus_saddled,
            (
                sound::ENTITY_NAUTILUS_DASH,
                sound::ENTITY_NAUTILUS_DASH_READY,
            ),
            (
                sound::ENTITY_NAUTILUS_DASH_LAND,
                sound::ENTITY_NAUTILUS_DASH_READY_LAND,
            ),
        ),
        (
            &made.zombie_nautilus_saddled,
            (
                sound::ENTITY_ZOMBIE_NAUTILUS_DASH,
                sound::ENTITY_ZOMBIE_NAUTILUS_DASH_READY,
            ),
            (
                sound::ENTITY_ZOMBIE_NAUTILUS_DASH_LAND,
                sound::ENTITY_ZOMBIE_NAUTILUS_DASH_READY_LAND,
            ),
        ),
    ] {
        for (more, (dash, ready), other) in [
            (pool(6), water_sounds, land_sounds),
            (Vec::new(), land_sounds, water_sounds),
        ] {
            let mut scene = scene_of(&more, vec![set(nautilus, MOUNT, MOUNT_AT, 0.0)], 1);
            all_on(&mut scene, &[0]);
            scene.idle();
            let output = scene.input(jump(100));
            let heard = sounds_of(&output, dash);
            assert_eq!(heard.len(), 1, "kind {}: {heard:?}", nautilus.kind);
            let (_, volume, pitch) = heard[0];
            assert_eq!(volume, 1.0);
            assert!((0.8..=1.2).contains(&pitch) && pitch != 1.0, "{pitch}");
            assert_eq!(played(&output, other.0), 0, "not the other medium's");
            assert!(dashing(&the_mount(&scene)), "DASH");
            let mut outputs = vec![output];
            outputs.extend(scene.idle_for(NAUTILUS_COOLDOWN + 3));
            let at: Vec<usize> = outputs
                .iter()
                .enumerate()
                .filter(|(_, output)| played(output, ready) > 0)
                .map(|(tick, _)| tick)
                .collect();
            assert_eq!(
                at,
                [NAUTILUS_COOLDOWN as usize - 1],
                "kind {}",
                nautilus.kind
            );
        }
    }
}

/// `AbstractNautilus.canJump` = saddled: the rider of a bare tame nautilus dashes nothing.
#[test]
fn a_nautilus_without_a_saddle_does_not_dash() {
    let mut scene = with_mount(&made().nautilus_tame, MOUNT_AT, 0.0, 1);
    all_on(&mut scene, &[0]);
    let output = scene.input(jump(100));
    assert_eq!(played(&output, sound::ENTITY_NAUTILUS_DASH_LAND), 0);
    assert_eq!(played(&output, sound::ENTITY_NAUTILUS_DASH), 0);
    assert!(!dashing(&the_mount(&scene)));
}

/// R §3 and P §9 item 9: the horse's jump is as N7h left it: it rears, and its jump sound
/// at 0.4 and pitch 1, undrawn.
#[test]
fn a_horses_jump_is_as_it_was() {
    let mut scene = with_mount(&made().horse_saddled, MOUNT_AT, 0.0, 1);
    all_on(&mut scene, &[0]);
    scene.idle();
    let output = scene.input(jump(100));
    let heard = sounds_of(&output, sound::ENTITY_HORSE_JUMP);
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!((heard[0].1, heard[0].2), (HORSE_JUMP_VOLUME, 1.0));
    assert_ne!(
        flags(&the_mount(&scene), HORSE_FLAGS) & HORSE_STANDING,
        0,
        "it rears"
    );
}

// ------------------------------------------------------------------------------------
// 5. The happy ghast (R §1.2, §2.3, §2.5, §4).

/// A harnessed ghast with `riders` of `players` players on it, driven, past the grace of
/// its still timeout (`tickCount > 60`, which the game counts from its loading).
fn ridden_ghast(riders: usize, players: usize) -> Scene {
    let mut scene = with_mount(&made().ghast_harnessed, GHAST_AT, 0.0, players);
    let who: Vec<usize> = (0..riders).collect();
    all_on(&mut scene, &who);
    scene.idle_for(STILL_GRACE + 5);
    scene
}

/// R §1.2 and B 7.11.4 (amended by R §9 item 4 from the dumps): `addPassenger` with no
/// riders before plays `HARNESS_GOGGLES_DOWN` by `Entity.playSound(SoundEvent)`, at
/// volume 1 and pitch 1 (the brief says the entity's volume and pitch; the dump's
/// `playSound(SoundEvent)` passes 1 and 1); `removePassenger` with nobody left plays
/// `HARNESS_GOGGLES_UP` the same way. Riders between do neither.
#[test]
fn the_goggles_come_down_at_the_first_rider_and_go_up_as_the_last_gets_off() {
    let mut scene = with_mount(&made().ghast_harnessed, GHAST_AT, 0.0, 2);
    let output = scene.input(used(key(MOUNT)));
    let down = sounds_of(&output, sound::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN);
    assert_eq!(down.len(), 1, "{down:?}");
    assert_eq!((down[0].1, down[0].2), (1.0, 1.0));
    assert_eq!(
        sound_excepts(&output, sound::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN),
        [None]
    );
    let output = scene.input_of(1, used(key(MOUNT)));
    assert_eq!(rides(&scene, 1), Some(key(MOUNT)));
    assert_eq!(
        played(&output, sound::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN),
        0
    );
    scene.idle();
    let (output, _) = sneak_off(&mut scene, 1);
    assert_eq!(
        played(&output, sound::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP),
        0
    );
    let (output, _) = sneak_off(&mut scene, 0);
    let up = sounds_of(&output, sound::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP);
    assert_eq!(up.len(), 1, "{up:?}");
    assert_eq!((up[0].1, up[0].2), (1.0, 1.0));
}

/// `steers` each tick, a little further each time, until a move is taken: the number of
/// ticks it took, counting the first try, in the tick after the one the test made the
/// change in, as 1.
fn ticks_until_steered(scene: &mut Scene, who: usize, most: u32) -> Option<u32> {
    (1..=most).find(|_| {
        let to = beside(scene, 2);
        steers(scene, who, to, true)
    })
}

/// R §4 and §1.2: `removePassenger` sets the still timeout to 10 when any rider gets off,
/// and while it runs `getControllingPassenger` gives nobody. The rider who got off is put
/// on its top, where `scanPlayerAboveGhast` keeps the timeout at 10 for as long as they
/// stand there; once they walk off, it runs out in ten ticks. So the first rider's moves
/// are not taken while the other stands on top, and are again on the tenth tick after
/// the walk's: the walk's move comes before the ticks of entities, so in its own tick the
/// timeout counts 10 → 9 and the scan finds nobody; nine ticks more bring it to 0 and
/// `STAYS_STILL` off; `lastVehicle` is set at that tick's end (SGPLI `tickPlayer`
/// 350–385, the root if the player controls it), and the move of the tick after is
/// taken (`handleMoveVehicle` @80–116).
#[test]
fn a_happy_ghast_stays_still_ten_ticks_after_a_rider_gets_off_and_nobody_steers_it_then() {
    let mut scene = ridden_ghast(2, 2);
    let to = beside(&scene, 2);
    assert!(steers(&mut scene, 0, to, true), "steered before");
    sneak_off(&mut scene, 1);
    assert_eq!(
        ticks_until_steered(&mut scene, 0, 3 * STILL_TICKS),
        None,
        "steered while a player stands on its top"
    );
    let ghast = the_mount(&scene).position;
    scene.input_of(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(ghast[0], FLOOR, ghast[2] + 8.0)),
            rotation: None,
            on_ground: true,
        },
    );
    let after = ticks_until_steered(&mut scene, 0, STILL_TICKS + 4).expect("steered again");
    assert_eq!(
        after, STILL_TICKS,
        "ticks after the walk off its top until the driver steers"
    );
}

/// R §1.5 and §7: when the first rider of a happy ghast gets off, the second is first and
/// steers it once its still timeout is over: the first, put on its top, holds it still
/// until they walk off, and the second's moves are taken on the tenth tick after the
/// walk's, as in the test above.
#[test]
fn when_the_first_rider_of_a_happy_ghast_gets_off_the_second_steers_it_once_it_moves_again() {
    let mut scene = ridden_ghast(2, 2);
    sneak_off(&mut scene, 0);
    assert_eq!(passengers(&the_mount(&scene)), entities_of(&scene, &[1]));
    let ghast = the_mount(&scene).position;
    scene.input(PlayerInput::Move {
        position: Some(Vec3::new(ghast[0], FLOOR, ghast[2] + 8.0)),
        rotation: None,
        on_ground: true,
    });
    let after = ticks_until_steered(&mut scene, 1, STILL_TICKS + 4).expect("the second steers");
    assert_eq!(
        after, STILL_TICKS,
        "ticks after the walk off its top until the second steers"
    );
    let output = scene.idle();
    assert_seated(&scene, &[1], &output, 1e-6, "in the front seat now");
}

/// R §5, `HappyGhast.getDismountLocationForPassenger` (1245–1257): `(x, box maxY, z)`, on
/// its top at the middle, for every rider.
#[test]
fn a_happy_ghasts_riders_are_let_off_on_its_top() {
    let mut scene = ridden_ghast(2, 2);
    let height = f64::from(EntityTypeInfo::of(entity_types::HAPPY_GHAST).height);
    for n in [1, 0] {
        let (_, at) = sneak_off(&mut scene, n);
        let ghast = the_mount(&scene).position;
        assert!(
            near(at, [ghast[0], ghast[1] + height, ghast[2]], 1e-9),
            "player {} off at {at:?}, the ghast at {ghast:?}",
            n + 1
        );
    }
}

/// R §2.5: `clientVehicleIsFloating` needs `!vehicle.isFlyingVehicle()` (SGPLI
/// @782–787), true for a grown happy ghast: its driver, holding it in the air well past
/// the 80 ticks of a pig or a horse, is not kicked.
#[test]
fn a_rider_who_holds_a_happy_ghast_aloft_is_not_kicked_for_floating() {
    let mut scene = ridden_ghast(1, 1);
    let at = the_mount(&scene).position;
    let up = [at[0], at[1] + 3.0, at[2]];
    assert!(steers(&mut scene, 0, [at[0], at[1] + 1.5, at[2]], false));
    assert!(steers(&mut scene, 0, up, false));
    for tick in 0..ALOFT {
        let output = scene.idle();
        assert!(!kicked(&output, &scene, 0), "kicked at tick {tick}");
    }
    assert_eq!(rides(&scene, 0), Some(key(MOUNT)));
    assert_eq!(bits(the_mount(&scene).position), bits(up), "held up");
}

/// R §4, `HappyGhast.canBeCollidedWith` (1196–1233): on the server a grown ghast is solid
/// to other entities' moves while on its still timeout, which a player standing on its
/// top keeps at 10 (`scanPlayerAboveGhast`): a pig let fall above it lands on its top and
/// stays. A ghast nobody stands on and nobody rides is not still, and the pig falls
/// through it to the floor.
#[test]
fn a_still_happy_ghast_is_solid_to_a_falling_pig_and_a_ghast_not_still_is_not() {
    let height = f64::from(EntityTypeInfo::of(entity_types::HAPPY_GHAST).height);
    let up = 4.0;
    for still in [true, false] {
        let ghast = set(
            &made().ghast,
            MOUNT,
            [GHAST_AT[0], FLOOR + up, GHAST_AT[2]],
            0.0,
        );
        let world = World::default().with_entities(&[ghast]);
        // Player 1 on the ghast's top at a corner, or on the floor beside it.
        let standing = if still {
            Vec3::new(GHAST_AT[0] + 1.5, FLOOR + up + height, GHAST_AT[2] + 1.5)
        } else {
            PLAYERS_AT[0]
        };
        let mut scene = Scene::alone(&world, hotbar(), standing);
        at_world_time(&mut scene);
        scene.idle_for(3);
        let top = the_mount(&scene).position[1] + height;
        let ghast_at = the_mount(&scene).position;
        let mut pig = set(
            &made().pig,
            OTHER_ENTITY,
            [ghast_at[0] - 1.2, top + 2.0, ghast_at[2] - 1.2],
            0.0,
        );
        pig.on_ground = false;
        scene.region.tick(&TickInputs {
            arrivals: vec![Arrival {
                id: EffectId {
                    sequence: 40,
                    index: 0,
                },
                chunk: ChunkPos::containing(pig.position[0], pig.position[2]),
                entity: pig,
                due: Due::FromTaking,
            }],
            ..TickInputs::default()
        });
        let mut seen = Vec::new();
        let landed = scene.idle_until(100, &mut seen, |scene, _| {
            mount_of(scene, OTHER_ENTITY).is_some_and(|pig| pig.on_ground)
        });
        assert!(landed, "still {still}: the pig never landed");
        scene.idle_for(20);
        let pig = mount_of(&scene, OTHER_ENTITY).expect("the pig");
        let top = the_mount(&scene).position[1] + height;
        if still {
            assert!(
                (pig.position[1] - top).abs() < 1e-6,
                "on the still ghast's top ({top}): {:?}",
                pig.position
            );
        } else {
            assert!(
                pig.position[1] < FLOOR + 0.5,
                "through the ghast to the floor: {:?}",
                pig.position
            );
        }
    }
}

/// R §4 and P §9 item 13: `setServerStillTimeout` from 0 to more sends the ghast's whole
/// position to its watchers (`ClientboundEntityPositionSyncPacket`), and `aiStep` asks
/// for precise positions while it is still: its report is flagged `PRECISE` and
/// `NEEDS_SYNC` as it begins to stay still, when a rider gets off; a ghast driven and not
/// still is reported without `PRECISE`.
#[test]
fn a_happy_ghast_sends_its_whole_position_as_it_begins_to_stay_still() {
    let mut scene = ridden_ghast(2, 2);
    let output = scene.input(drive(beside(&scene, 2), true));
    for flags in tracked_flags(&output, key(MOUNT)) {
        assert_eq!(flags & Tracked::PRECISE, 0, "driven, not still: {flags}");
    }
    let mut outputs = vec![scene.input_of(1, sneak(true))];
    outputs.extend(scene.idle_for(2));
    let begun: Vec<u8> = outputs
        .iter()
        .flat_map(|output| tracked_flags(output, key(MOUNT)))
        .collect();
    assert!(
        begun
            .iter()
            .any(|flags| flags & (Tracked::PRECISE | Tracked::NEEDS_SYNC)
                == Tracked::PRECISE | Tracked::NEEDS_SYNC),
        "a report flagged PRECISE and NEEDS_SYNC as it stops: {begun:?}"
    );
    assert_eq!(rides(&scene, 1), None, "the second rider got off");
}

/// R §6: the happy ghast is not `PlayerRideableJumping`: the jump command does nothing.
#[test]
fn a_happy_ghast_does_not_dash() {
    let mut scene = ridden_ghast(1, 1);
    let before = the_mount(&scene);
    let output = scene.input(jump(100));
    assert!(
        !output.events.iter().any(|event| matches!(
            event,
            RegionEvent::Sound { .. } | RegionEvent::EntitySound { .. }
        )),
        "no sound: {:?}",
        output.events
    );
    assert_eq!(the_mount(&scene).position, before.position);
}

// ------------------------------------------------------------------------------------
// 6. Getting off (R §5).

/// Where a rider is put off a camel on the side `turn` degrees from its yaw (90 for the
/// right of a right-handed rider): `getCollisionHorizontalEscapeVector(camel width, rider
/// width, yaw + turn)` from the camel, on the floor of its block.
fn off_the_camel(camel: [f64; 3], yaw: f32, turn: f32) -> [f64; 3] {
    let width = EntityTypeInfo::of(entity_types::CAMEL).width;
    let way = escape(width, yaw + turn);
    [camel[0] + way[0], camel[1].floor(), camel[2] + way[2]]
}

/// Player 1 on the saddled camel at [`MOUNT_AT`] facing south (yaw 0), among `more`; the
/// camel's position.
fn camel_facing_south(more: &[(BlockPos, clustine_data::BlockState)]) -> (Scene, [f64; 3]) {
    let mut scene = scene_of(
        more,
        vec![set(&made().camel_saddled, MOUNT, MOUNT_AT, 0.0)],
        2,
    );
    all_on(&mut scene, &[0, 1]);
    scene.input(turn(0.0, 0.0));
    scene.idle();
    let camel = the_mount(&scene);
    assert_eq!(camel.yaw, 0.0);
    (scene, camel.position)
}

/// R §5, `AbstractHorse.getDismountLocationForPassenger` (2544–2600) with the camel's
/// width: a right-handed rider is put off on the camel's right first (west of a camel
/// facing south), on the floor; the second rider the same way.
#[test]
fn a_camels_riders_get_off_on_its_right_by_its_width() {
    let (mut scene, camel) = camel_facing_south(&[]);
    let expected = off_the_camel(camel, 0.0, 90.0);
    assert!(expected[0] < camel[0], "to the west");
    for n in [1, 0] {
        let (_, at) = sneak_off(&mut scene, n);
        assert!(
            near(at, expected, 1e-9),
            "player {} off at {at:?}, expected {expected:?}",
            n + 1
        );
    }
    assert!(passengers(&the_mount(&scene)).is_empty());
}

/// The same rule's other branches: a wall three high on the right sends the rider to the
/// left; walls on both sides to the camel's own position (not its top).
#[test]
fn a_camels_rider_walled_on_the_right_gets_off_on_its_left_and_walled_on_both_where_it_stands() {
    let width = EntityTypeInfo::of(entity_types::CAMEL).width;
    let right = off_the_camel(MOUNT_AT, 0.0, 90.0);
    let left = off_the_camel(MOUNT_AT, 0.0, -90.0);
    let wall_at = |x: f64| -> Vec<(BlockPos, clustine_data::BlockState)> {
        let x = x.floor() as i32;
        (-60..=-57)
            .flat_map(|y| (0..=2).map(move |z| (bp(x, y, z), blocks::STONE)))
            .collect()
    };
    assert!(
        f64::from(width) / 2.0 < 1.0,
        "the walls stand clear of the camel"
    );
    let (mut scene, camel) = camel_facing_south(&wall_at(right[0]));
    let (_, at) = sneak_off(&mut scene, 0);
    let expected = off_the_camel(camel, 0.0, -90.0);
    assert!(
        near(at, expected, 1e-9),
        "off at {at:?}, expected {expected:?}"
    );

    let mut walls = wall_at(right[0]);
    walls.extend(wall_at(left[0]));
    let (mut scene, camel) = camel_facing_south(&walls);
    let (_, at) = sneak_off(&mut scene, 0);
    assert!(
        near(at, camel, 1e-9),
        "off at {at:?}, the camel at {camel:?}"
    );
}

/// R §5: the nautilus has no place of its own, so `Animal`'s, the pig's: the offsets of
/// its facing, its clockwise side first (south's is west), on the floor.
#[test]
fn a_nautiluses_rider_gets_off_on_its_clockwise_side() {
    let made = made();
    for nautilus in [&made.nautilus_saddled, &made.zombie_nautilus_saddled] {
        let mut scene = with_mount(nautilus, MOUNT_AT, 0.0, 1);
        all_on(&mut scene, &[0]);
        scene.input(turn(0.0, 0.0));
        scene.idle();
        assert_eq!(the_mount(&scene).yaw, 0.0);
        let (_, at) = sneak_off(&mut scene, 0);
        assert!(
            near(at, [1.5, FLOOR, 1.5], 1e-9),
            "kind {}: off at {at:?}",
            nautilus.kind
        );
    }
}

/// R §5: the rider's `baseTick`, their eyes under water and no bubble column there, puts
/// them off a vehicle in `#dismounts_underwater`, which holds the camel and the happy
/// ghast. Water six deep over the floor puts every rider's eyes under (a camel's are 3.02
/// above its feet, a ghast's 5.02).
#[test]
fn riders_whose_eyes_go_under_water_are_put_off_a_camel_and_a_happy_ghast() {
    let made = made();
    for (mount, at) in [
        (&made.camel_saddled, MOUNT_AT),
        (&made.camel, MOUNT_AT),
        (&made.ghast_harnessed, GHAST_AT),
    ] {
        let mut scene = scene_of(&pool(6), vec![set(mount, MOUNT, at, 0.0)], 1);
        scene.input(used(key(MOUNT)));
        let mut seen = Vec::new();
        let off = scene.idle_until(3, &mut seen, |scene, _| rides(scene, 0).is_none());
        assert!(off, "kind {}: still on in the water", mount.kind);
    }
}

/// R §5: `#dismounts_underwater` holds neither the camel husk nor the nautiluses: their
/// riders stay on with their eyes under water.
#[test]
fn riders_stay_on_a_camel_husk_and_the_nautiluses_under_water() {
    let made = made();
    for mount in [
        &made.husk_saddled,
        &made.nautilus_saddled,
        &made.zombie_nautilus_saddled,
    ] {
        let mut scene = scene_of(&pool(6), vec![set(mount, MOUNT, MOUNT_AT, 0.0)], 1);
        all_on(&mut scene, &[0]);
        for tick in 0..40 {
            scene.idle();
            assert_eq!(
                rides(&scene, 0),
                Some(key(MOUNT)),
                "kind {}: put off at tick {tick}",
                mount.kind
            );
        }
        let eyes = feet(&scene, 0).y + PLAYER_EYES;
        assert!(
            eyes < FLOOR + 6.0,
            "kind {}: the eyes are under",
            mount.kind
        );
    }
}

// ------------------------------------------------------------------------------------
// 7. Menus (R §1.1, §1.3; P §9 item 5).

/// Whether `output` opened `who` a mount menu of the mount, with its two slots and no
/// chest.
fn opened_mount_menu(output: &TickOutput, scene: &Scene, who: usize) -> Option<Opened> {
    let menus = opened(output, scene.actors[who].id);
    match menus.as_slice() {
        [] => None,
        [menu] => {
            assert!(
                matches!(menu.kind, MenuKind::Mount { entity, columns: 0 } if entity == key(MOUNT)),
                "the mount's menu without a chest: {:?}",
                menu.kind
            );
            assert_eq!(menu.kind.container_slots(), 2);
            Some(menu.clone())
        }
        more => panic!("more than one menu: {more:?}"),
    }
}

/// R §1.1: `Camel.mobInteract` opens its saddle menu to anyone who sneaks (not a baby),
/// and `openCustomInventoryScreen` (1554–1566) asks nothing of who rides it: player 2
/// opens it on a camel player 1 rides, and player 1 on an unridden one. Nobody gets on.
#[test]
fn a_camels_menu_opens_to_anyone_who_sneaks_and_clicks_it_ridden_or_not() {
    let made = made();
    for camel in [&made.camel, &made.husk_saddled] {
        let mut scene = with_mount(camel, MOUNT_AT, 0.0, 2);
        let output = scene.input(used_sneaking(key(MOUNT)));
        let menu = opened_mount_menu(&output, &scene, 0);
        assert!(menu.is_some(), "kind {}: unridden", camel.kind);
        assert_eq!(rides(&scene, 0), None);
        scene.close(menu.expect("opened").serial);
        all_on(&mut scene, &[0]);
        let output = scene.input_of(1, used_sneaking(key(MOUNT)));
        assert!(
            opened_mount_menu(&output, &scene, 1).is_some(),
            "kind {}: ridden by another",
            camel.kind
        );
        assert_eq!(rides(&scene, 1), None);
    }
}

/// R §1.1: the inventory key opens the vehicle's (`getVehicle()`, SGPLI 5213– @212–242),
/// so either of a camel's riders opens its menu, saddled or not.
#[test]
fn either_rider_of_a_camel_opens_its_menu_by_the_inventory_key() {
    let made = made();
    for camel in [&made.camel, &made.camel_saddled] {
        let mut scene = with_mount(camel, MOUNT_AT, 0.0, 2);
        all_on(&mut scene, &[0, 1]);
        for n in [0, 1] {
            let output = scene.input_of(n, PlayerInput::OpenVehicleInventory);
            assert!(
                opened_mount_menu(&output, &scene, n).is_some(),
                "player {}",
                n + 1
            );
        }
    }
}

/// R §1.3, `AbstractNautilus.openCustomInventoryScreen` (1149–1170): only tame, and only
/// unridden or ridden by that player. Player 1 opens a tame one by sneaking where nobody
/// rides it, and by the inventory key while riding it; player 2's sneaking click on it
/// then opens nothing; a wild one's opens nothing.
#[test]
fn a_nautiluses_menu_opens_to_its_rider_or_where_nobody_rides_it() {
    let made = made();
    let mut scene = with_mount(&made.nautilus_saddled, MOUNT_AT, 0.0, 2);
    let output = scene.input(used_sneaking(key(MOUNT)));
    let menu = opened_mount_menu(&output, &scene, 0).expect("opened where nobody rides it");
    scene.close(menu.serial);
    all_on(&mut scene, &[0]);
    let output = scene.input(PlayerInput::OpenVehicleInventory);
    assert!(
        opened_mount_menu(&output, &scene, 0).is_some(),
        "to its rider"
    );
    let output = scene.input_of(1, used_sneaking(key(MOUNT)));
    assert!(
        opened_mount_menu(&output, &scene, 1).is_none(),
        "not to another while it is ridden"
    );
    assert_eq!(rides(&scene, 1), None);

    let mut scene = with_mount(&made.nautilus_wild, MOUNT_AT, 0.0, 1);
    let output = scene.input(used_sneaking(key(MOUNT)));
    assert!(opened_mount_menu(&output, &scene, 0).is_none(), "wild");
}

/// Player 1 opens the mount's menu by a sneaking click and puts the saddle from their
/// hotbar into its first slot; the tick's output.
fn saddle_by_menu(scene: &mut Scene) -> TickOutput {
    let serial = scene.next_input(0);
    let output = scene.input(used_sneaking(key(MOUNT)));
    let menu = opened_mount_menu(&output, scene, 0).expect("the menu opened");
    let hotbar_slot = menu.kind.container_slots() + 27 + usize::from(SADDLE);
    let hotbar_slot = i16::try_from(hotbar_slot).expect("small");
    scene.menu_click(serial, hotbar_slot, 0, ClickKind::Pickup);
    scene.menu_click(serial, 0, 0, ClickKind::Pickup)
}

/// B 7.7.5 (`Camel` 1506–1525) and P §9 item 5: a saddle put in a grown camel's saddle
/// slot sounds as its own, `CAMEL_SADDLE` (the husk's `CAMEL_HUSK_SADDLE`), and its rider
/// then steers it. Twice, as the game plays it: the menu's saddle slot is an `ArmorSlot`
/// (`HorseInventoryMenu$1`), whose `setByPlayer` calls `onEquipItem` itself and then
/// `Slot.setByPlayer` → `set` → the mob's slot container (`Mob$1.setTheItem`) →
/// `LivingEntity.setItemSlot`, which calls `onEquipItem` again with the same old and new
/// stacks (`LivingEntity` 5852–5866); each plays the sound (1385–1460).
#[test]
fn a_saddle_put_in_a_camels_menu_sounds_as_its_own() {
    let made = made();
    for (camel, saddle) in [
        (&made.camel, sound::ENTITY_CAMEL_SADDLE),
        (&made.husk, sound::ENTITY_CAMEL_HUSK_SADDLE),
    ] {
        let mut scene = with_mount(camel, MOUNT_AT, 0.0, 1);
        let serial = scene.next_input(0);
        let output = saddle_by_menu(&mut scene);
        assert_eq!(played(&output, saddle), 2, "kind {}", camel.kind);
        assert_eq!(played(&output, sound::ENTITY_HORSE_SADDLE), 0);
        scene.close(serial);
        all_on(&mut scene, &[0]);
        scene.idle();
        let to = beside(&scene, 5);
        assert!(
            steers(&mut scene, 0, to, true),
            "kind {}: saddled",
            camel.kind
        );
    }
}

/// `AbstractNautilus.getEquipSound` (1077–1096): its saddle sounds
/// `NAUTILUS_SADDLE_UNDERWATER_EQUIP` under water and `NAUTILUS_SADDLE_EQUIP` out of it,
/// never the other. (How many times is the menu's: the nautilus's menu class is not
/// among the dumps, so the count is left open; a camel's is twice, above.)
#[test]
fn a_saddle_put_in_a_nautiluses_menu_sounds_by_its_medium() {
    for (more, saddle, other) in [
        (
            pool(6),
            sound::ITEM_NAUTILUS_SADDLE_UNDERWATER_EQUIP,
            sound::ITEM_NAUTILUS_SADDLE_EQUIP,
        ),
        (
            Vec::new(),
            sound::ITEM_NAUTILUS_SADDLE_EQUIP,
            sound::ITEM_NAUTILUS_SADDLE_UNDERWATER_EQUIP,
        ),
    ] {
        let mut scene = scene_of(
            &more,
            vec![set(&made().nautilus_tame, MOUNT, MOUNT_AT, 0.0)],
            1,
        );
        let output = saddle_by_menu(&mut scene);
        assert!(played(&output, saddle) > 0, "{saddle}");
        assert_eq!(played(&output, other), 0);
    }
}

// ------------------------------------------------------------------------------------
// 8. Across a pinned line (R §7, §8; P §5).

/// Where the mount stands in the west, short of the line at block x = 16, facing east.
const MOUNT_WEST: [f64; 3] = [15.5, FLOOR, 8.5];
/// A driven move that takes it over the line, into chunk (1, 0).
const ACROSS: [f64; 3] = [16.3, FLOOR, 8.5];
const BEYOND: ChunkPos = ChunkPos::new(1, 0);
/// Where the players stand in the west, beside the mount's place.
const WEST_PLAYERS: [Vec3; 3] = [
    Vec3::new(12.5, FLOOR, 8.5),
    Vec3::new(15.5, FLOOR, 11.5),
    Vec3::new(15.5, FLOOR, 5.5),
];

/// The west with `riders` players on `mount` at [`MOUNT_WEST`] facing east, its driver
/// first; and the east with nobody in it; both at the world's time.
fn line(mount: &Entity, riders: usize) -> (Scene, Scene) {
    let world = World::default().with_entities(&[set(mount, MOUNT, MOUNT_WEST, -90.0)]);
    let mut west = Scene::new(Layout::side(true), &world, hotbar(), WEST_PLAYERS[0]);
    for (n, feet) in (2..).zip(&WEST_PLAYERS[1..riders]) {
        west.join(n, *feet);
    }
    at_world_time(&mut west);
    let who: Vec<usize> = (0..riders).collect();
    all_on(&mut west, &who);
    west.idle();
    let east_layout = Layout::side(false);
    let mut east = Scene {
        region: region_of(&east_layout, &World::default(), hotbar()),
        layout: east_layout,
        inventory: hotbar(),
        actors: Vec::new(),
    };
    at_world_time(&mut east);
    (west, east)
}

/// The west's crossing: the unit, filed by its chunk, and each rider's hand-over, in the
/// order of the west's actors.
struct Sent {
    chunk: ChunkPos,
    unit: Entity,
    transfers: Vec<clustine_sim::PlayerTransfer>,
}

fn sent(west: &Scene, output: &TickOutput, riders: usize) -> Sent {
    let (chunk, unit) = output
        .arrivals
        .iter()
        .find(|(_, entity)| entity.key == key(MOUNT))
        .cloned()
        .expect("the mount was sent");
    let handed = departures(output);
    assert_eq!(
        handed.len(),
        riders,
        "every rider is handed over: {handed:?}"
    );
    let transfers = west.actors[..riders]
        .iter()
        .map(|actor| {
            let (_, transfer, to) = handed
                .iter()
                .find(|(who, _, _)| *who == actor.id)
                .cloned()
                .expect("handed over");
            assert_eq!(to, OTHER);
            transfer
        })
        .collect();
    Sent {
        chunk,
        unit,
        transfers,
    }
}

/// The east takes the unit, then the riders, the last first: each is its actor from then
/// on, in the west's order. The output of the riders' arrival.
fn taken(east: &mut Scene, west: &Scene, sent: &Sent) -> TickOutput {
    taken_in(east, west, sent, false)
}

/// [`taken`], the unit and its riders in one tick where `together`.
fn taken_in(east: &mut Scene, west: &Scene, sent: &Sent, together: bool) -> TickOutput {
    let arrivals = vec![Arrival {
        id: EffectId {
            sequence: 40,
            index: 0,
        },
        chunk: sent.chunk,
        entity: sent.unit.clone(),
        due: Due::FromTaking,
    }];
    let changes = sent
        .transfers
        .iter()
        .enumerate()
        .rev()
        .map(|(n, transfer)| PlayerChange::Arrive(EDGE, west.actors[n].id, transfer.clone()))
        .collect();
    let output = if together {
        east.region.tick(&TickInputs {
            arrivals,
            player_changes: changes,
            ..TickInputs::default()
        })
    } else {
        east.region.tick(&TickInputs {
            arrivals,
            ..TickInputs::default()
        });
        east.region.tick(&TickInputs {
            player_changes: changes,
            ..TickInputs::default()
        })
    };
    for (n, transfer) in sent.transfers.iter().enumerate() {
        east.actors.push(Actor {
            id: west.actors[n].id,
            entity: transfer.entity_id,
            input: transfer.last_input,
            sequence: west.actors[n].sequence,
        });
    }
    output
}

/// Every rider of the east's mount seated, none told to teleport, in the west's order.
fn assert_seated_beyond(east: &Scene, seating: &TickOutput, riders: usize) {
    for n in 0..riders {
        assert!(
            rides_told(seating, east, n).is_empty(),
            "player {}: no teleport on: {:?}",
            n + 1,
            seating.player_events
        );
        let riding = east
            .region
            .player_state(east.actors[n].id)
            .expect("came")
            .riding
            .expect("riding");
        assert_eq!(riding.vehicle, key(MOUNT), "player {}", n + 1);
        assert_eq!(riding.pending, None, "player {}: seated", n + 1);
    }
    let mount = the_mount(east);
    assert!(
        mount
            .riding
            .passengers
            .iter()
            .all(|seat| seat.pending.is_none()),
        "{:?}",
        mount.riding.passengers
    );
}

/// R §7: a camel with two players crosses as a boat does, the unit first with both seats
/// kept, each rider handed over by their own transfer; the east seats both with no
/// teleport, in their seats (the second's 0.7 behind, still in the west's land), and the
/// driver drives it on there.
#[test]
fn a_camel_and_its_two_riders_cross_a_line_and_both_are_seated_beyond() {
    let (mut west, mut east) = line(&made().camel_saddled, 2);
    let output = west.input(drive(ACROSS, true));
    let crossing = sent(&west, &output, 2);
    assert_eq!(crossing.chunk, BEYOND);
    let seats = &crossing.unit.riding.passengers;
    assert_eq!(seats.len(), 2);
    assert!(
        seats
            .iter()
            .all(|seat| seat.player && seat.pending.is_some())
    );
    assert!(mount_of(&west, MOUNT).is_none(), "the camel left the west");

    let seating = taken(&mut east, &west, &crossing);
    assert_seated_beyond(&east, &seating, 2);
    assert_eq!(
        passengers(&the_mount(&east)),
        [east.actors[0].entity, east.actors[1].entity],
        "in their order"
    );
    let output = east.idle();
    assert_seated(&east, &[0, 1], &output, 1e-6, "beyond");
    assert!(
        feet(&east, 1).x < 16.0,
        "the second's seat is in the west's land"
    );
    let on = [17.0, FLOOR, 8.5];
    assert!(steers(&mut east, 0, on, true), "driven beyond");
}

/// R §7: a happy ghast with three players crosses the same way.
#[test]
fn a_happy_ghast_and_its_three_riders_cross_a_line_and_all_are_seated_beyond() {
    let (mut west, mut east) = line(&made().ghast_harnessed, 3);
    let at = the_mount(&west).position;
    let output = west.input(drive([ACROSS[0], at[1], at[2]], true));
    let crossing = sent(&west, &output, 3);
    assert_eq!(crossing.unit.riding.passengers.len(), 3);
    let seating = taken(&mut east, &west, &crossing);
    assert_seated_beyond(&east, &seating, 3);
    let output = east.idle();
    assert_seated(&east, &[0, 1, 2], &output, 1e-6, "beyond");
    let to = beside(&east, 5);
    assert!(steers(&mut east, 0, to, true), "driven beyond");
}

/// R §7: a dashing nautilus crosses with its dash, which is the record's: `DASH` travels
/// with the unit, and its ready sound comes in the east, once. In the game it comes on
/// the 40th tick counting the command's; here on the 41st, counting the west's ticks and
/// the east's together, the unit taken with its rider in the east's first tick: a unit
/// is sent across in its crossing tick before the entities tick and is not ticked there,
/// so its countdown misses that tick, as every crossing unit's does (a crossing waits,
/// as the roadmap names for items in phase N). A tick of a sound, which no player
/// notices; pinned so that a change to it is seen.
#[test]
fn a_dashing_nautilus_crosses_a_line_and_sounds_ready_beyond() {
    let (mut west, mut east) = line(&made().nautilus_saddled, 1);
    west.input(jump(100));
    assert!(dashing(&the_mount(&west)));
    let output = west.input(drive(ACROSS, true));
    let crossing = sent(&west, &output, 1);
    assert!(dashing(&crossing.unit), "DASH travels with it");
    assert_eq!(played(&output, sound::ENTITY_NAUTILUS_DASH_READY_LAND), 0);
    taken_in(&mut east, &west, &crossing, true);
    assert_eq!(
        rides(&east, 0),
        Some(key(MOUNT)),
        "seated in the arrival's tick"
    );
    // The command's tick and the crossing's in the west, then the east's taking.
    let mut ticks = 3;
    let mut seen = Vec::new();
    let heard = east.idle_until(60, &mut seen, |_, seen| {
        seen.last()
            .is_some_and(|output| played(output, sound::ENTITY_NAUTILUS_DASH_READY_LAND) > 0)
    });
    assert!(heard, "the ready sound beyond");
    ticks += seen.len();
    assert_eq!(
        ticks,
        NAUTILUS_COOLDOWN as usize + 1,
        "the tick of the ready sound, counting the command's"
    );
    seen.extend(east.idle_for(NAUTILUS_COOLDOWN));
    let total: usize = seen
        .iter()
        .map(|output| played(output, sound::ENTITY_NAUTILUS_DASH_READY_LAND))
        .sum();
    assert_eq!(total, 1, "once");
}

/// R §7 and P §9 item 3: the seat's count includes pending seats. Of a camel's two riders
/// the second comes to the east first: they sit 0.7 behind, where they sat, not alone in
/// front, while the first's seat waits; and both sit where they did once the first comes.
#[test]
fn a_camels_second_rider_beyond_a_line_keeps_their_seat_behind_while_the_first_is_on_the_way() {
    let (mut west, mut east) = line(&made().camel_saddled, 2);
    let output = west.input(drive(ACROSS, true));
    let crossing = sent(&west, &output, 2);
    let arrive =
        |n: usize| PlayerChange::Arrive(EDGE, west.actors[n].id, crossing.transfers[n].clone());
    east.region.tick(&TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 40,
                index: 0,
            },
            chunk: crossing.chunk,
            entity: crossing.unit.clone(),
            due: Due::FromTaking,
        }],
        player_changes: vec![arrive(1)],
        ..TickInputs::default()
    });
    for n in [0, 1] {
        east.actors.push(Actor {
            id: west.actors[n].id,
            entity: crossing.transfers[n].entity_id,
            input: crossing.transfers[n].last_input,
            sequence: west.actors[n].sequence,
        });
    }
    let output = east.idle();
    let camel = the_mount(&east);
    assert_eq!(
        passengers(&camel),
        entities_of(&east, &[0, 1]),
        "the first's seat kept"
    );
    assert!(
        camel.riding.passengers[0].pending.is_some(),
        "the first is on the way"
    );
    let behind = seated(&camel, 1, 2, output.clock.game_time);
    assert!(
        near(feet(&east, 1), behind, 1e-6),
        "the second at {:?}, their seat behind at {behind:?}",
        feet(&east, 1)
    );
    let seating = east.region.tick(&TickInputs {
        player_changes: vec![arrive(0)],
        ..TickInputs::default()
    });
    assert_seated_beyond(&east, &seating, 2);
    let output = east.idle();
    assert_seated(&east, &[0, 1], &output, 1e-6, "both come");
}

/// R §7: control changes hands beyond the line as before it: the second rider of a camel
/// steers it in the east once the first has got off there.
#[test]
fn the_second_rider_of_a_camel_steers_it_beyond_a_line_once_the_first_gets_off() {
    let (mut west, mut east) = line(&made().camel_saddled, 2);
    let output = west.input(drive(ACROSS, true));
    let crossing = sent(&west, &output, 2);
    taken(&mut east, &west, &crossing);
    east.idle();
    sneak_off(&mut east, 0);
    assert_eq!(passengers(&the_mount(&east)), [east.actors[1].entity]);
    east.idle();
    let to = beside(&east, 5);
    assert!(steers(&mut east, 1, to, true), "the second steers beyond");
}

/// The `Has` notes of `p` in `output`: the place each puts them.
fn noted_places(output: &TickOutput, p: clustine_world::PlayerId) -> Vec<Vec3> {
    output
        .stays
        .iter()
        .filter_map(|note| match note {
            StayNote::Has { player, place, .. } if *player == p => Some(place.pose.position),
            _ => None,
        })
        .collect()
}

fn leaves(scene: &mut Scene, who: usize) -> TickOutput {
    let actor = &scene.actors[who];
    let change = PlayerChange::Leave(EDGE, actor.id, Some(actor.entity), None);
    scene.region.tick(&TickInputs {
        player_changes: vec![change],
        ..TickInputs::default()
    })
}

/// R §8: of two players on a camel, the one who logs out is put off where the camel lets
/// them off (its side), and the camel stays with the other, who steers it.
#[test]
fn of_two_riders_of_a_camel_the_one_who_leaves_is_put_off_and_the_camel_stays() {
    let mut scene = with_mount(&made().camel_saddled, MOUNT_AT, 0.0, 2);
    all_on(&mut scene, &[0, 1]);
    scene.idle();
    let camel = the_mount(&scene);
    let output = leaves(&mut scene, 1);
    let places = noted_places(&output, scene.actors[1].id);
    assert_eq!(places.len(), 1, "the leave's note: {places:?}");
    assert!(
        (0.5..1.5).contains(&horizontal(
            [places[0].x, places[0].y, places[0].z],
            camel.position
        )),
        "put off at the camel's side: {:?}, the camel at {:?}",
        places[0],
        camel.position
    );
    let camel = mount_of(&scene, MOUNT).expect("the camel stays");
    assert_eq!(passengers(&camel), [scene.me()]);
    assert_eq!(rides(&scene, 0), Some(key(MOUNT)));
    scene.idle();
    let to = beside(&scene, 5);
    assert!(steers(&mut scene, 0, to, true), "the other steers it");
}

/// R §8 and P §5: of two players on a happy ghast, the one who logs out is put off on
/// its top, the ghast stays, and its `removePassenger` holds it still for ten ticks, in
/// which its driver does not steer it. The game handles a disconnection in its
/// connection's tick, after the levels' (`Connection.tick`), so the removal's timeout of
/// 10 first counts down in the tick after; a region takes the leave at the start of its
/// tick, which is that tick: the driver's moves are taken again on the tenth tick after
/// the leave's, as after a walk off its top.
#[test]
fn a_rider_leaving_a_happy_ghast_holds_it_still_ten_ticks() {
    let mut scene = ridden_ghast(2, 2);
    let height = f64::from(EntityTypeInfo::of(entity_types::HAPPY_GHAST).height);
    let ghast = the_mount(&scene).position;
    let output = leaves(&mut scene, 1);
    let places = noted_places(&output, scene.actors[1].id);
    assert_eq!(places.len(), 1, "{places:?}");
    assert!(
        near(places[0], [ghast[0], ghast[1] + height, ghast[2]], 1e-6),
        "on its top: {:?}",
        places[0]
    );
    assert_eq!(passengers(&the_mount(&scene)), [scene.me()]);
    let after = ticks_until_steered(&mut scene, 0, STILL_TICKS + 4).expect("steered again");
    assert_eq!(
        after, STILL_TICKS,
        "ticks after the leave until the driver steers"
    );
}
