//! Tests of riding a horse (step N7h), written from the specification by someone who did
//! not write the code: the plan `docs/groundwork/n7h-riding-plan.md` (sections 1, 3, 7 and
//! 10), the research note `docs/groundwork/n7h-riding-research.md` (R5 and R9 above all)
//! and the 26.3 bytecode (`reference/n7-javap/`, `reference/n5-javap/`), cited as the
//! class and its line in the dump: `AbstractHorse` is
//! `net.minecraft.world.entity.animal.equine.AbstractHorse.txt`, and so on. Regions are
//! driven by their public interface alone.
//!
//! The horses are the game's own: hatched by a spawn egg in a scratch region, tamed there
//! by being ridden (`RunAroundLikeCrazyGoal`: every throw makes the next taming likelier,
//! so it ends), saddled by a saddle's use and their foals hatched by the egg on them;
//! their records are then set into the world of each test where it wants them. A skeleton
//! horse cannot be tamed by a player at all (`SkeletonHorse.mobInteract` 297–308: an
//! untamed one passes every click), so its tamed record is the wild one's with the type
//! state of a tamed horse, which the family shares; that is the one record these tests
//! make up.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60), in creative mode as
//! every Clustine player is, and is right-handed as every Clustine player is.

mod n6_scene;

use std::sync::OnceLock;

use clustine_data::sounds as sound;
use clustine_data::{EntityTypeInfo, blocks, entity_types, items};
use clustine_sim::TickOutput;
use clustine_sim::api::RegionEvent;
use clustine_sim::api::{Keys, MenuKind, PlayerEvent, PlayerInput, VehicleMove};
use clustine_sim::shown::{DataShown, head_yaw, index, mob_values};
use clustine_world::{BlockPos, EntityData, EntityId, Vec3};
use n6_scene::*;
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The numbers of the specification.

/// The player's vehicle attachment (`Avatar.DEFAULT_VEHICLE_ATTACHMENT`, the generated
/// `vehicle_points` of `minecraft:player`).
const PLAYER_ATTACHMENT: f64 = 0.6;
/// A player's width (`EntityType.PLAYER`, 0.6 × 1.8), a float.
const PLAYER_WIDTH: f32 = 0.6;
/// `AbstractHorse.DATA_ID_FLAGS`, after `AgeableMob`'s baby at 16 and age lock at 17
/// (`AgeableMob.defineSynchedData` 187–204), and its bits
/// (`AbstractHorse` 29–39).
const FLAGS: u8 = 18;
const FLAG_TAME: u8 = 2;
const FLAG_EATING: u8 = 16;
const FLAG_STANDING: u8 = 32;
/// `createBaseHorseAttributes` (`AbstractHorse` 760–783): the safe fall distance and the
/// fall damage multiplier.
const SAFE_FALL: f64 = 6.0;
const FALL_MULTIPLIER: f64 = 0.5;
/// `playJumpSound` and `causeFallDamage`'s landing: `playSound(…, 0.4f, 1.0f)`.
const HORSE_SOUND_VOLUME: f32 = 0.4;
/// `AbstractHorse.getSoundVolume`, which `makeSound` uses (`AbstractHorse` 793–797).
const HORSE_VOICE_VOLUME: f32 = 0.8;
/// `playBlockFallSound` (`LivingEntity` 4634–): the stone's fall sound at its volume
/// (1.0) × 0.5 and its pitch (1.0) × 0.75.
const STONE_FALL_VOLUME: f32 = 0.5;
const STONE_FALL_PITCH: f32 = 0.75;
/// `setStanding(20)` from `standIfPossible` (`AbstractHorse` 1651–1666).
const STAND_TICKS: u32 = 20;

/// The top of the stone floor.
const FLOOR: f64 = -60.0;

/// What the hotbar holds: slot 0 is empty.
const EMPTY: u8 = 0;
const WHEAT: u8 = 1;
const GOLDEN_APPLE: u8 = 2;
const STICK: u8 = 3;
const SADDLE: u8 = 4;
const CARROT_ON_A_STICK: u8 = 5;

/// The key of the horse every test sets into its world.
const HORSE: u32 = 1;
/// Where it stands, within player 1's reach of the spawn: in block (2, -60, 1).
const HORSE_AT: [f64; 3] = [2.5, FLOOR, 1.5];

fn hotbar() -> clustine_sim::api::Inventory {
    inventory(&[
        (usize::from(WHEAT), stack(items::WHEAT, 1)),
        (usize::from(GOLDEN_APPLE), stack(items::GOLDEN_APPLE, 1)),
        (usize::from(STICK), stack(items::STICK, 1)),
        (usize::from(SADDLE), stack(items::SADDLE, 1)),
        (
            usize::from(CARROT_ON_A_STICK),
            stack(items::CARROT_ON_A_STICK, 1),
        ),
    ])
}

// ------------------------------------------------------------------------------------
// `Mth` and `Vec3` as the game computes them (`n5-javap/net.minecraft.util.Mth.txt`
// 61–87 and 2249–2268, `Vec3.yRot` 856–).

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

/// `Vec3.yRot(angle)`.
fn y_rot(v: [f64; 3], angle: f32) -> [f64; 3] {
    let c = f64::from(mth_cos(f64::from(angle)));
    let s = f64::from(mth_sin(f64::from(angle)));
    [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
}

const DEGREES: f32 = 0.017_453_292;

/// `Entity.getCollisionHorizontalEscapeVector(vehicleWidth, riderWidth, yaw)`
/// (`Entity` 10416–10446).
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

/// `LivingEntity.calculateFallDamage` (4600–4632) with `calculateFallPower`: the floor of
/// `(distance + 1e-6 − safe) · multiplier · attribute`, the block's multiplier 1.
fn fall_damage(distance: f64) -> f32 {
    ((distance + 1e-6 - SAFE_FALL) * FALL_MULTIPLIER).floor() as f32
}

// ------------------------------------------------------------------------------------
// The horses, as the game makes them.

/// One kind's horses as its region's records had them: a wild adult hatched by an egg,
/// the same tamed, then saddled (none for a llama, which takes no saddle), and a foal
/// hatched by the egg on the adult.
#[derive(Clone)]
struct Horses {
    wild: clustine_world::Entity,
    tamed: clustine_world::Entity,
    saddled: Option<clustine_world::Entity>,
    foal: clustine_world::Entity,
}

fn egg_of(kind: i32) -> i32 {
    match kind {
        entity_types::HORSE => items::HORSE_SPAWN_EGG,
        entity_types::DONKEY => items::DONKEY_SPAWN_EGG,
        entity_types::MULE => items::MULE_SPAWN_EGG,
        entity_types::SKELETON_HORSE => items::SKELETON_HORSE_SPAWN_EGG,
        entity_types::ZOMBIE_HORSE => items::ZOMBIE_HORSE_SPAWN_EGG,
        entity_types::LLAMA => items::LLAMA_SPAWN_EGG,
        _ => panic!("no egg of kind {kind} here"),
    }
}

/// A stone wall two high along the four sides of the square from (-4, -4) to (5, 5),
/// which keeps a horse left to itself within the land that ticks and within reach.
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

fn record_of(scene: &Scene, key: EntityId) -> clustine_world::Entity {
    scene
        .all()
        .into_iter()
        .find(|entity| entity.key == key)
        .expect("there")
}

/// Hatches one of `kind` by an egg on the floor at `at`, or on the mob `on`.
fn hatch(scene: &mut Scene, kind: i32, on: Option<EntityId>) -> EntityId {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(0);
    match on {
        Some(mob) => {
            scene.input(used(mob));
        }
        None => {
            scene.click(bp(2, -61, 2), clustine_sim::api::Face::Top);
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

/// Rides the wild horse `key` until it is tamed: each throw adds 5 to its temper, and a
/// try is tamed below it (`RunAroundLikeCrazyGoal.tick`, R9.2), so it ends.
fn tame_by_riding(scene: &mut Scene, key: EntityId) {
    let rider = scene.actors[0].id;
    for _ in 0..40_000 {
        if flags(&record_of(scene, key)) & FLAG_TAME != 0 {
            return;
        }
        let riding = scene
            .region
            .player_state(rider)
            .expect("there")
            .riding
            .is_some();
        if riding {
            scene.idle();
        } else {
            scene.input(used(key));
        }
    }
    panic!("never tamed");
}

/// The horses of `kind`, made once for every test.
fn horses(kind: i32) -> Horses {
    static MADE: [OnceLock<Horses>; 6] = [const { OnceLock::new() }; 6];
    let slot = match kind {
        entity_types::HORSE => 0,
        entity_types::DONKEY => 1,
        entity_types::MULE => 2,
        entity_types::SKELETON_HORSE => 3,
        entity_types::ZOMBIE_HORSE => 4,
        entity_types::LLAMA => 5,
        _ => panic!("not of the family: {kind}"),
    };
    MADE[slot].get_or_init(|| make_horses(kind)).clone()
}

fn make_horses(kind: i32) -> Horses {
    let mut scene = Scene::alone(
        &World::of(&pen()),
        inventory(&[
            (0, stack(egg_of(kind), 1)),
            (usize::from(SADDLE), stack(items::SADDLE, 1)),
        ]),
        SPAWN,
    );
    let adult = hatch(&mut scene, kind, None);
    let wild = record_of(&scene, adult);
    assert_eq!(flags(&wild) & FLAG_TAME, 0, "an egg's is wild");
    let tamed = if kind == entity_types::SKELETON_HORSE {
        let mut tamed = wild.clone();
        let EntityData::Mob { state, .. } = &mut tamed.data else {
            panic!("a mob");
        };
        let EntityData::Mob { state: horse, .. } = &horses(entity_types::HORSE).tamed.data else {
            panic!("a mob");
        };
        *state = horse.clone();
        assert_ne!(
            flags(&tamed) & FLAG_TAME,
            0,
            "the tamed state is the family's"
        );
        tamed
    } else {
        // An empty hand (slot 1 of this inventory) mounts a wild horse.
        scene.select(1);
        tame_by_riding(&mut scene, adult);
        // Off again, by sneaking.
        scene.input(sneak(true));
        scene.input(sneak(false));
        scene.idle();
        record_of(&scene, adult)
    };
    let foal = hatch(&mut scene, kind, Some(adult));
    let foal = record_of(&scene, foal);
    let saddled = (kind != entity_types::LLAMA).then(|| {
        let mut saddling = with_entities(&[], vec![set(&tamed, HORSE, HORSE_AT, 0.0)]);
        saddling.select(SADDLE);
        saddling.input(used(key(HORSE)));
        assert_eq!(rides(&saddling, 0), None, "the saddle's use mounts nobody");
        let saddled = the_horse(&saddling);
        assert_ne!(saddled.data, tamed.data, "the saddle went on");
        saddled
    });
    Horses {
        wild,
        tamed,
        saddled,
        foal,
    }
}

/// `horse` as entity number `n` standing at `at`, facing `yaw`, at rest.
fn set(horse: &clustine_world::Entity, n: u32, at: [f64; 3], yaw: f32) -> clustine_world::Entity {
    let mut horse = horse.clone();
    horse.key = key(n);
    horse.uuid = Uuid::from_u128(u128::from(n) + 7_000);
    horse.position = at;
    horse.motion = [0.0; 3];
    horse.yaw = yaw;
    horse.pitch = 0.0;
    horse.on_ground = true;
    horse.fall_distance = 0.0;
    horse.riding = Default::default();
    horse
}

fn saddled(kind: i32) -> clustine_world::Entity {
    horses(kind).saddled.expect("of a kind that takes a saddle")
}

fn with_entities(
    more: &[(BlockPos, clustine_data::BlockState)],
    entities: Vec<clustine_world::Entity>,
) -> Scene {
    let world = World::of(more).with_entities(&entities);
    Scene::alone(&world, hotbar(), SPAWN)
}

/// A scene of one region with player 1 at the spawn holding [`hotbar`], slot 0 (empty)
/// selected, the blocks `more` and `horse` as [`HORSE`] at [`HORSE_AT`], facing `yaw`.
fn with_horse(
    more: &[(BlockPos, clustine_data::BlockState)],
    horse: &clustine_world::Entity,
    yaw: f32,
) -> Scene {
    with_entities(more, vec![set(horse, HORSE, HORSE_AT, yaw)])
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

fn horse_of(scene: &Scene, n: u32) -> Option<clustine_world::Entity> {
    scene.all().into_iter().find(|entity| entity.key == key(n))
}

fn the_horse(scene: &Scene) -> clustine_world::Entity {
    horse_of(scene, HORSE).expect("the horse is there")
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

fn passengers(scene: &Scene) -> Vec<EntityId> {
    the_horse(scene)
        .riding
        .passengers
        .iter()
        .map(|seat| seat.key)
        .collect()
}

fn shown_at(horse: &clustine_world::Entity, at: u8) -> Option<(DataShown, bool)> {
    mob_values(horse.kind, &horse.data)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, default)| (value, default))
}

fn flags(horse: &clustine_world::Entity) -> u8 {
    match shown_at(horse, FLAGS) {
        Some((DataShown::Byte(flags), _)) => flags,
        other => panic!("a horse shows its flags as a byte at 18: {other:?}"),
    }
}

fn standing(horse: &clustine_world::Entity) -> bool {
    flags(horse) & FLAG_STANDING != 0
}

fn health(horse: &clustine_world::Entity) -> f32 {
    match shown_at(horse, index::HEALTH) {
        Some((DataShown::Float(health), _)) => health,
        other => panic!("a mob shows its health: {other:?}"),
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

fn sounds_in(outputs: &[TickOutput], id: i32) -> Vec<(Vec3, f32, f32)> {
    outputs
        .iter()
        .flat_map(|output| sounds_of(output, id))
        .collect()
}

fn bits(at: [f64; 3]) -> [u64; 3] {
    at.map(f64::to_bits)
}

fn near(a: Vec3, b: [f64; 3], within: f64) -> bool {
    (a.x - b[0]).abs() <= within && (a.y - b[1]).abs() <= within && (a.z - b[2]).abs() <= within
}

/// The passenger point of `kind` (`EntityAttachments`' `PASSENGER`, the generated data).
fn seat_of(kind: i32) -> [f64; 3] {
    EntityTypeInfo::of(kind).passenger_points[0]
}

/// Where a rider's feet are for a vehicle of `kind` at `at`, facing `yaw`, with
/// `extra` added to its point before the turn (the rearing's): `getPassengerRidingPosition`
/// less the rider's vehicle attachment (R6; `AbstractHorse.getPassengerAttachmentPoint`
/// 2646–2681).
fn seated_at(kind: i32, at: [f64; 3], yaw: f32, extra: [f64; 3]) -> [f64; 3] {
    let point = seat_of(kind);
    let base = y_rot(point, -yaw * DEGREES);
    let rearing = y_rot(extra, -yaw * DEGREES);
    [
        at[0] + base[0] + rearing[0],
        at[1] + base[1] + rearing[1] - PLAYER_ATTACHMENT,
        at[2] + base[2] + rearing[2],
    ]
}

/// Player 1 gets on the horse with an empty hand, then holds what slot `slot` holds; where
/// it stood when they took it.
fn mounted(scene: &mut Scene, slot: u8) -> [f64; 3] {
    scene.select(EMPTY);
    scene.input(used(key(HORSE)));
    assert_eq!(
        rides(scene, 0),
        Some(key(HORSE)),
        "player 1 rides the horse"
    );
    scene.select(slot);
    scene.idle();
    the_horse(scene).position
}

/// Player 1 on the saddled horse of `kind` at [`HORSE_AT`], among `more`.
fn driving(kind: i32, more: &[(BlockPos, clustine_data::BlockState)]) -> (Scene, [f64; 3]) {
    let mut scene = with_horse(more, &saddled(kind), 0.0);
    let at = mounted(&mut scene, EMPTY);
    (scene, at)
}

/// The kinds of the family a player steers, once saddled.
const STEERED: [i32; 5] = [
    entity_types::HORSE,
    entity_types::DONKEY,
    entity_types::MULE,
    entity_types::SKELETON_HORSE,
    entity_types::ZOMBIE_HORSE,
];

// ------------------------------------------------------------------------------------
// 1. Mounting (`AbstractHorse.mobInteract` 1535–1596, `Horse.mobInteract` 284–345,
// `doPlayerRide` 1066–1090; R9.1).

/// `AbstractHorse.mobInteract` 107–115: a tamed adult clicked with an empty hand, not
/// sneaking, takes the player on, and their client is told.
#[test]
fn a_tamed_horse_takes_a_player_who_clicks_it_with_an_empty_hand() {
    let mut scene = with_horse(&[], &horses(entity_types::HORSE).tamed, 0.0);
    let output = scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    assert_eq!(passengers(&scene), [scene.me()]);
    let told = rides_told(&output, &scene, 0);
    assert!(
        matches!(told[..], [(Some(vehicle), _)] if vehicle == key(HORSE)),
        "the rider's client is told: {told:?}"
    );
}

/// `AbstractHorse.mobInteract` 44–115: a wild adult clicked with an empty hand takes the
/// player on (the taming ride, R9.2), of every kind a player can tame.
#[test]
fn a_wild_horse_takes_a_player_who_clicks_it_with_an_empty_hand() {
    for kind in [
        entity_types::HORSE,
        entity_types::DONKEY,
        entity_types::MULE,
        entity_types::LLAMA,
    ] {
        let mut scene = with_horse(&[], &horses(kind).wild, 0.0);
        scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), Some(key(HORSE)), "kind {kind}");
    }
}

/// `Horse.mobInteract` 0–61 and `AbstractHorse.mobInteract` 21–44: the inventory is
/// opened only for a tamed horse (`isTamed() && isSecondaryUseActive()`), so a wild one
/// takes a player who sneaks as one who does not: `Horse.mobInteract`'s flag is false,
/// the empty hand goes on to `AbstractHorse.mobInteract`, whose `isTamed()` at 21 is false
/// and whose empty hand at 44–54 reaches `doPlayerRide` at 107. (`Entity.interact`'s own
/// sneaking branch, `Entity` 6475–6496, only ties the player's leashed mobs to it.)
#[test]
#[ignore = "finding: the game mounts a player who sneak-clicks a wild horse with an empty \
            hand (AbstractHorse.mobInteract opens the inventory only for a tamed one); \
            Clustine takes nobody on"]
fn a_wild_horse_takes_a_player_who_clicks_it_with_an_empty_hand_while_sneaking() {
    for kind in [entity_types::HORSE, entity_types::DONKEY] {
        let mut scene = with_horse(&[], &horses(kind).wild, 0.0);
        scene.input(used_sneaking(key(HORSE)));
        assert_eq!(rides(&scene, 0), Some(key(HORSE)), "kind {kind}");
    }
}

/// `AbstractHorse.mobInteract` 21–43: a tamed adult clicked by a player who sneaks opens
/// its inventory (`openCustomInventoryScreen`) and takes nobody on.
#[test]
fn a_tamed_horse_clicked_by_a_player_who_sneaks_opens_its_inventory_and_takes_nobody() {
    let mut scene = with_horse(&[], &saddled(entity_types::HORSE), 0.0);
    let output = scene.input(used_sneaking(key(HORSE)));
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&scene).is_empty());
    let opened = the_opened(&output, scene.actors[0].id);
    assert!(
        matches!(opened.kind, MenuKind::Mount { entity, .. } if entity == key(HORSE)),
        "the horse's inventory: {:?}",
        opened.kind
    );
}

/// `Horse.mobInteract` 62–108: the horse's food goes to `fedFood`, which a horse at full
/// health that is tamed refuses (`handleEating` false, `PASS`), and a wild one eats; in
/// neither case is the player taken on.
#[test]
fn a_player_who_holds_the_horses_food_is_not_taken_on() {
    let family = horses(entity_types::HORSE);
    for (horse, which) in [(&family.tamed, "tamed"), (&family.wild, "wild")] {
        for slot in [WHEAT, GOLDEN_APPLE] {
            let mut scene = with_horse(&[], horse, 0.0);
            scene.select(slot);
            scene.input(used(key(HORSE)));
            assert_eq!(rides(&scene, 0), None, "a {which} horse, slot {slot}");
            assert!(passengers(&scene).is_empty());
        }
    }
}

/// `Horse.mobInteract` 94–108: a wild horse clicked with anything but its food is made
/// mad (`makeMad` 1668–1683): it rears and sounds its anger at its voice's volume, and
/// takes nobody on.
#[test]
fn a_wild_horse_clicked_with_an_item_that_is_not_its_food_rears_angrily_and_takes_nobody() {
    for slot in [STICK, SADDLE] {
        let mut scene = with_horse(&[], &horses(entity_types::HORSE).wild, 0.0);
        scene.select(slot);
        let output = scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), None, "slot {slot}");
        assert!(standing(&the_horse(&scene)), "it rears");
        let angry = sounds_of(&output, sound::ENTITY_HORSE_ANGRY);
        assert_eq!(angry.len(), 1, "its anger: {angry:?}");
        assert_eq!(angry[0].1, HORSE_VOICE_VOLUME);
    }
}

/// `AbstractHorse.mobInteract` 44–115: a tamed horse clicked with an item that does
/// nothing to it (a stick) takes the player on.
#[test]
fn a_tamed_horse_takes_a_player_who_clicks_it_holding_an_item_that_does_nothing_to_it() {
    for slot in [STICK, CARROT_ON_A_STICK] {
        let mut scene = with_horse(&[], &saddled(entity_types::HORSE), 0.0);
        scene.select(slot);
        scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), Some(key(HORSE)), "slot {slot}");
    }
}

/// `AbstractHorse.mobInteract` 57–78 and `canUseSlot(SADDLE)` 357–379: a saddle's use on
/// a tamed adult without one puts it on, and the click takes nobody on; the next click
/// with an empty hand does, and the rider steers it.
#[test]
fn a_saddle_used_on_a_tamed_horse_is_put_on_and_takes_nobody_on() {
    let mut scene = with_horse(&[], &horses(entity_types::HORSE).tamed, 0.0);
    scene.select(SADDLE);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), None);
    scene.select(EMPTY);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    scene.idle();
    let at = the_horse(&scene).position;
    let to = [at[0] + 0.5, at[1], at[2]];
    let output = scene.input(drive(to, true));
    assert!(corrections(&output, &scene, 0).is_empty(), "steered");
    assert_eq!(bits(the_horse(&scene).position), bits(to));
}

/// `AbstractHorse.mobInteract` 0–20: a baby goes to `Animal.mobInteract`, which mounts
/// nobody, with an empty hand, sneaking or not.
#[test]
fn a_foal_takes_no_rider() {
    for kind in [
        entity_types::HORSE,
        entity_types::DONKEY,
        entity_types::LLAMA,
    ] {
        let mut scene = with_horse(&[], &horses(kind).foal, 0.0);
        scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), None, "a foal of kind {kind}");
        scene.input(used_sneaking(key(HORSE)));
        assert_eq!(rides(&scene, 0), None, "a foal of kind {kind}, sneaking");
    }
}

/// `AbstractHorse.mobInteract` 0–20 (`isVehicle()`) and `Entity.canAddPassenger`: one
/// rider.
#[test]
fn a_ridden_horse_takes_no_second_rider() {
    let mut scene = with_horse(&[], &saddled(entity_types::HORSE), 0.0);
    let second = scene.join(2, Vec3::new(3.5, FLOOR, 0.5));
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    scene.input_of(second, used(key(HORSE)));
    assert_eq!(rides(&scene, second), None);
    assert_eq!(passengers(&scene), [scene.me()]);
}

/// `SkeletonHorse.mobInteract` 297–308: an untamed skeleton horse passes every click; a
/// tamed one goes on to `AbstractHorse`'s and takes the player.
#[test]
fn a_wild_skeleton_horse_takes_nobody_and_a_tamed_one_takes_its_rider() {
    let family = horses(entity_types::SKELETON_HORSE);
    let mut scene = with_horse(&[], &family.wild, 0.0);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), None, "wild");
    scene.input(used_sneaking(key(HORSE)));
    assert_eq!(rides(&scene, 0), None, "wild, sneaking");
    let mut scene = with_horse(&[], &family.tamed, 0.0);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)), "tamed");
}

/// `ZombieHorse.mobInteract` 235–287 and `AbstractHorse.mobInteract`: a wild zombie horse
/// takes a player who clicks it with an empty hand, as a wild horse does.
#[test]
fn a_wild_zombie_horse_takes_a_player_who_clicks_it_with_an_empty_hand() {
    let mut scene = with_horse(&[], &horses(entity_types::ZOMBIE_HORSE).wild, 0.0);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
}

/// The difference of two yaws, in degrees, the short way round.
fn yaw_apart(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// `doPlayerRide` 19–37 sets the player's yaw and pitch to the horse's before
/// `startRiding`: a player who looked elsewhere, down, is turned to the horse's way and
/// its pitch (0), and the horse, steered from that tick, takes the rider's yaw back.
#[test]
fn a_player_taken_on_is_turned_from_their_own_way_to_the_horses() {
    let mut scene = with_horse(&[], &saddled(entity_types::HORSE), 0.0);
    scene.input(turn(-120.0, 35.0));
    let before = the_horse(&scene);
    assert!(yaw_apart(before.yaw, -120.0) > 30.0, "{}", before.yaw);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    let rider = pose(&scene, 0);
    assert_eq!(rider.pitch, 0.0, "the horse's pitch");
    assert!(
        yaw_apart(rider.yaw, -120.0) > 30.0,
        "turned from their own: {}",
        rider.yaw
    );
    assert_eq!(the_horse(&scene).yaw, rider.yaw, "the horse takes it back");
}

/// `AbstractHorse.addPassenger` 1782–1795: after `doPlayerRide`, the passenger is snapped
/// to `getViewYRot(0)` and `getViewXRot(0)`, and a living entity's view yaw is its head's
/// (`LivingEntity.getViewYRot` 9092–9107: at 0, `rotLerp(0, yHeadRotO, yHeadRot)`, the
/// head's yaw at the start of its last tick). So the rider faces where the horse looks,
/// within the 10° a head turns in a tick (`LookControl`'s), not where its body points.
/// This horse's head is 28° from its body when it is clicked.
#[test]
#[ignore = "finding: the game snaps a mounting rider to the horse's head yaw \
            (addPassenger's getViewYRot(0) is LivingEntity's, yHeadRotO); Clustine turns \
            them to its body's yaw, and the horse then keeps its body's way"]
fn a_player_taken_on_faces_where_the_horse_looks() {
    let horse = saddled(entity_types::HORSE);
    let head = head_yaw(&horse.data).expect("a mob's head");
    let mut scene = with_horse(&[], &horse, head + 90.0);
    let before = the_horse(&scene);
    let looks = head_yaw(&before.data).expect("a mob's head");
    assert!(
        yaw_apart(looks, before.yaw) > 20.0,
        "the head ({looks}) is turned from the body ({})",
        before.yaw
    );
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    let rider = pose(&scene, 0);
    assert!(
        yaw_apart(rider.yaw, looks) <= 10.0,
        "the rider faces {}, the horse looked to {looks}, its body to {}",
        rider.yaw,
        before.yaw
    );
}

/// R6 and `AbstractHorse.getPassengerAttachmentPoint` 2646–2681 with `standAnimO` 0: the
/// rider's feet are at each kind's passenger point less their own vehicle attachment, x
/// and z unchanged.
#[test]
fn the_rider_sits_at_each_kinds_passenger_point_less_their_attachment() {
    for kind in STEERED {
        let (mut scene, _) = driving(kind, &[]);
        scene.idle();
        let horse = the_horse(&scene).position;
        let seated = feet(&scene, 0);
        let expected = seated_at(kind, horse, 0.0, [0.0; 3]);
        assert!(
            near(seated, expected, 1e-9),
            "kind {kind}: seated at {seated:?}, expected {expected:?}"
        );
    }
}

/// The passenger points of the generated data are the game's: a horse's 1.44375, a
/// donkey's 1.1125, a mule's 1.2125, an undead horse's 1.31875, a llama's 1.37 up and 0.3
/// back (`Llama`'s static initialiser, 22–45: its height less 0.25, and -0.3).
#[test]
fn the_passenger_points_are_the_games() {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    for (kind, y, z) in [
        (entity_types::HORSE, 1.443_75, 0.0),
        (entity_types::DONKEY, 1.1125, 0.0),
        (entity_types::MULE, 1.2125, 0.0),
        (entity_types::SKELETON_HORSE, 1.318_75, 0.0),
        (entity_types::ZOMBIE_HORSE, 1.318_75, 0.0),
        (entity_types::LLAMA, 1.37, -0.3),
    ] {
        let seat = seat_of(kind);
        assert!(
            seat[0] == 0.0 && close(seat[1], y) && close(seat[2], z),
            "kind {kind}: {seat:?}"
        );
    }
}

/// `Llama.getPassengerAttachmentPoint` 957–963: the default point, turned by the llama's
/// yaw (`EntityAttachments.get`): its rider sits 0.3 behind its middle, whichever way it
/// faces. The llama is its own (section 2), so its yaw is read as it stands.
#[test]
fn a_llamas_rider_sits_behind_its_middle_whichever_way_it_faces() {
    for yaw in [0.0_f32, 90.0, -135.0] {
        let mut scene = with_horse(&[], &horses(entity_types::LLAMA).tamed, yaw);
        scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), Some(key(HORSE)), "yaw {yaw}");
        let llama = the_horse(&scene);
        let expected = seated_at(entity_types::LLAMA, llama.position, llama.yaw, [0.0; 3]);
        let seated = feet(&scene, 0);
        assert!(
            near(seated, expected, 1e-6),
            "yaw {yaw} (now {}): seated at {seated:?}, expected {expected:?}",
            llama.yaw
        );
    }
}

// ------------------------------------------------------------------------------------
// 2. Who steers it (`getControllingPassenger` 2420–2438, `tickRidden` 1711–1767,
// `getRiddenRotation` 1769–1780; R9.2, R9.3).

/// `getControllingPassenger`: saddled and a player first, whatever they hold (no stick,
/// unlike the pig's). Their vehicle moves put the horse where they say, with no
/// correction, and they follow it to the seat; every kind of the family that takes a
/// saddle.
#[test]
fn a_saddled_horse_of_every_kind_is_steered_by_its_rider_where_their_client_puts_it() {
    for kind in STEERED {
        let (mut scene, at) = driving(kind, &[]);
        let to = [at[0] + 0.5, at[1], at[2] + 0.25];
        let output = scene.input(drive(to, true));
        assert!(corrections(&output, &scene, 0).is_empty(), "kind {kind}");
        assert_eq!(bits(the_horse(&scene).position), bits(to), "kind {kind}");
        scene.idle();
        let expected = seated_at(kind, to, 0.0, [0.0; 3]);
        assert!(
            near(feet(&scene, 0), expected, 1e-9),
            "kind {kind}: the rider at {:?}, expected {expected:?}",
            feet(&scene, 0)
        );
    }
}

/// `getControllingPassenger` asks nothing of the hands: the horse's food, a golden apple,
/// a stick, a carrot on a stick, the saddle or nothing, the rider steers it.
#[test]
fn a_saddled_horse_is_steered_by_its_rider_whatever_they_hold() {
    let (mut scene, mut at) = driving(entity_types::HORSE, &[]);
    for slot in [WHEAT, GOLDEN_APPLE, STICK, CARROT_ON_A_STICK, SADDLE, EMPTY] {
        scene.select(slot);
        let to = [at[0], at[1], at[2] + 0.3];
        let output = scene.input(drive(to, true));
        assert!(corrections(&output, &scene, 0).is_empty(), "slot {slot}");
        assert_eq!(bits(the_horse(&scene).position), bits(to), "slot {slot}");
        at = to;
    }
}

/// `getControllingPassenger` 0–26 with no saddle: `Animal`'s, which a player is not, so
/// the rider of a tamed horse without a saddle does not steer it: their move is a
/// passenger's who does not control the vehicle, answered with the horse's place.
#[test]
fn the_rider_of_a_horse_without_a_saddle_does_not_steer_it() {
    for kind in [entity_types::HORSE, entity_types::DONKEY] {
        let mut scene = with_horse(&[], &horses(kind).tamed, 0.0);
        scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), Some(key(HORSE)), "kind {kind}");
        let at = the_horse(&scene).position;
        let output = scene.input(drive([at[0] + 0.5, at[1], at[2]], true));
        let sent = corrections(&output, &scene, 0);
        assert_eq!(sent.len(), 1, "kind {kind}: one correction: {sent:?}");
        assert!(
            near(sent[0], at, 1e-9),
            "kind {kind}: sent {:?}, the horse at {at:?}",
            sent[0]
        );
    }
}

/// A llama takes no saddle (`isSaddled()` is false for it), so `getControllingPassenger`
/// is `Animal`'s: its rider's moves are refused, and it does not take their rotation.
#[test]
fn a_llama_is_not_steered_by_its_rider() {
    let mut scene = with_horse(&pen(), &horses(entity_types::LLAMA).tamed, 0.0);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    scene.input(turn(123.0, 40.0));
    let at = the_horse(&scene).position;
    let output = scene.input(drive([at[0] + 0.5, at[1], at[2]], true));
    let sent = corrections(&output, &scene, 0);
    assert_eq!(sent.len(), 1, "one correction: {sent:?}");
    assert!(near(sent[0], at, 1e-9), "sent {:?}, at {at:?}", sent[0]);
    let llama = the_horse(&scene);
    assert_ne!(llama.yaw, 123.0, "the llama keeps its own yaw");
    assert_ne!(llama.pitch, 20.0, "and pitch");
}

/// `tickRidden` 0–39 with `getRiddenRotation`: `setRot(rider yRot, rider xRot · 0.5f)`
/// (each `% 360f`, `Entity.setRot`), then the head, the body and `yRotO` to its yaw; run
/// after the tick's packets, so the rider's turn shows in that tick. Every kind.
#[test]
fn a_steered_horse_turns_to_its_riders_yaw_and_half_their_pitch() {
    for kind in STEERED {
        let (mut scene, _) = driving(kind, &[]);
        for (yaw, pitch) in [(30.0_f32, 40.0_f32), (-150.0, -50.0), (179.0, 89.0)] {
            scene.input(turn(yaw, pitch));
            let horse = the_horse(&scene);
            assert_eq!(horse.yaw, yaw % 360.0, "kind {kind}: yaw");
            assert_eq!(horse.pitch, (pitch * 0.5) % 360.0, "kind {kind}: pitch");
            assert_eq!(head_yaw(&horse.data), Some(yaw), "kind {kind}: head yaw");
        }
    }
}

/// R9.3 and R3: a steered horse runs no AI (`isEffectiveAi()` false) and no travel of its
/// own, its motion zero after every tick. Left steered in a pen for 600 ticks, in which a
/// horse left to itself would have strolled, grazed or reared (`RandomStandGoal`), it
/// stays exactly where it was and never rears.
#[test]
fn a_steered_horse_has_no_motion_and_no_ai_of_its_own() {
    let (mut scene, at) = driving(entity_types::HORSE, &pen());
    for tick in 0..600 {
        scene.idle();
        let horse = the_horse(&scene);
        assert_eq!(horse.motion, [0.0; 3], "motion at tick {tick}");
        assert_eq!(bits(horse.position), bits(at), "moved at tick {tick}");
        assert!(!standing(&horse), "reared at tick {tick}");
    }
}

/// Without a saddle nobody controls the horse, so `travelRidden` and its `tickRidden`
/// never run (`LivingEntity.aiStep` 551–587 asks for a controlling player): its rider's
/// turns do not turn it.
#[test]
fn the_rider_of_a_horse_without_a_saddle_does_not_turn_it() {
    let mut scene = with_horse(&pen(), &horses(entity_types::HORSE).tamed, 0.0);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    for (yaw, pitch) in [(77.0_f32, 10.0_f32), (-33.0, 60.0), (150.0, -20.0)] {
        scene.input(turn(yaw, pitch));
        let horse = the_horse(&scene);
        assert_ne!(horse.yaw, yaw, "turned to {yaw}");
        assert_ne!(horse.pitch, pitch * 0.5, "pitched to half of {pitch}");
    }
}

/// `HasCustomInventoryScreen` by `player_command OPEN_INVENTORY` (SGPLI 5213–, 212–242:
/// `getVehicle()`, not the controlled one) and `openCustomInventoryScreen` 803–825: the
/// rider of a tamed horse opens its inventory, saddled or not, a llama's too; a wild
/// horse's rider opens nothing.
#[test]
fn the_rider_of_a_tamed_horse_opens_its_inventory_by_the_inventory_key_and_of_a_wild_one_not() {
    for (horse, opens) in [
        (saddled(entity_types::HORSE), true),
        (horses(entity_types::HORSE).tamed, true),
        (horses(entity_types::LLAMA).tamed, true),
        (horses(entity_types::HORSE).wild, false),
    ] {
        let mut scene = with_horse(&[], &horse, 0.0);
        scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), Some(key(HORSE)));
        let output = scene.input(PlayerInput::OpenVehicleInventory);
        let menus = opened(&output, scene.actors[0].id);
        if opens {
            assert_eq!(menus.len(), 1, "kind {}: {menus:?}", horse.kind);
            assert!(
                matches!(menus[0].kind, MenuKind::Mount { entity, .. } if entity == key(HORSE)),
                "{:?}",
                menus[0].kind
            );
        } else {
            assert!(menus.is_empty(), "a wild horse's: {menus:?}");
        }
    }
}

// ------------------------------------------------------------------------------------
// 3. The jump bar (`handlePlayerCommand(START_RIDING_JUMP)`, n5 SGPLI 5213–, offsets
// 135–180; `canJump` 2223–2227, `handleStartJump` 2229–2238, `standIfPossible`
// 1651–1666, `playJumpSound` 1908–1915 and each kind's; R9.4).

/// Each kind's jump sound out of water: a horse's own, a donkey's, a mule's, and a
/// horse's for the undead ones (`SkeletonHorse.playJumpSound` 165–178 out of water, no
/// override in `ZombieHorse`).
fn jump_sound_of(kind: i32) -> i32 {
    match kind {
        entity_types::DONKEY => sound::ENTITY_DONKEY_JUMP,
        entity_types::MULE => sound::ENTITY_MULE_JUMP,
        _ => sound::ENTITY_HORSE_JUMP,
    }
}

const JUMP_SOUNDS: [i32; 4] = [
    sound::ENTITY_HORSE_JUMP,
    sound::ENTITY_DONKEY_JUMP,
    sound::ENTITY_MULE_JUMP,
    sound::ENTITY_SKELETON_HORSE_JUMP_WATER,
];

fn jump_sounds(output: &TickOutput) -> Vec<(i32, Vec3, f32, f32)> {
    JUMP_SOUNDS
        .iter()
        .flat_map(|&id| {
            sounds_of(output, id)
                .into_iter()
                .map(move |(at, volume, pitch)| (id, at, volume, pitch))
        })
        .collect()
}

/// `handleStartJump`: `allowStandSliding`, `standIfPossible` (`canPerformRearing()` true
/// for all but the llama; on the server `setStanding(20)`, which ends eating), then the
/// kind's jump sound at the horse, `playSound(…, 0.4f, 1.0f)`. Every kind a player
/// steers.
#[test]
fn a_jump_of_any_power_rears_a_steered_horse_and_sounds_its_kinds_jump() {
    for kind in STEERED {
        for power in [1, 45, 100] {
            let (mut scene, at) = driving(kind, &[]);
            assert!(!standing(&the_horse(&scene)));
            let output = scene.input(jump(power));
            let horse = the_horse(&scene);
            assert!(standing(&horse), "kind {kind}, power {power}: it rears");
            assert_eq!(flags(&horse) & FLAG_EATING, 0, "and does not eat");
            let heard = jump_sounds(&output);
            assert_eq!(
                heard.len(),
                1,
                "kind {kind}, power {power}: one jump sound: {heard:?}"
            );
            let (id, where_, volume, pitch) = heard[0];
            assert_eq!(id, jump_sound_of(kind), "kind {kind}: its kind's");
            assert_eq!((volume, pitch), (HORSE_SOUND_VOLUME, 1.0), "kind {kind}");
            assert!(near(where_, at, 1e-9), "kind {kind}: at {where_:?}");
        }
    }
}

/// `handlePlayerCommand` 169–170: `data > 0` is asked before `handleStartJump`, so a jump
/// of no power, or less, does nothing.
#[test]
fn a_jump_of_no_power_does_nothing() {
    for power in [0, -1] {
        let (mut scene, _) = driving(entity_types::HORSE, &[]);
        let output = scene.input(jump(power));
        assert!(!standing(&the_horse(&scene)), "power {power}");
        assert!(jump_sounds(&output).is_empty(), "power {power}");
    }
}

/// `handlePlayerCommand` 135–160: the jump goes to `getControlledVehicle()`, and
/// `canJump()` is `isSaddled()`: the rider of a horse without a saddle, or of a llama,
/// jumps nothing.
#[test]
fn a_jump_does_nothing_to_a_horse_its_rider_does_not_steer() {
    for horse in [
        horses(entity_types::HORSE).tamed,
        horses(entity_types::LLAMA).tamed,
    ] {
        let mut scene = with_horse(&[], &horse, 0.0);
        scene.input(used(key(HORSE)));
        assert_eq!(rides(&scene, 0), Some(key(HORSE)));
        let output = scene.input(jump(80));
        assert!(!standing(&the_horse(&scene)), "kind {}", horse.kind);
        assert!(jump_sounds(&output).is_empty(), "kind {}", horse.kind);
    }
}

/// `SkeletonHorse.playJumpSound` 165–178: in water, its own water jump sound at 0.4
/// instead of the horse's. The horse stands in a source of water a block deep; its
/// rider's eyes are above it, so it keeps them.
#[test]
fn a_skeleton_horse_jumping_in_water_sounds_its_water_jump() {
    let water = state("minecraft:water[level=0]");
    let mut pool = vec![(bp(2, -60, 1), water)];
    for (x, z) in [(1, 1), (3, 1), (2, 0), (2, 2)] {
        pool.push((bp(x, -60, z), blocks::STONE));
    }
    let (mut scene, at) = driving(entity_types::SKELETON_HORSE, &pool);
    assert_eq!(scene.block(bp(2, -60, 1)), water);
    let output = scene.input(jump(60));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)), "still on");
    let heard = jump_sounds(&output);
    assert_eq!(heard.len(), 1, "one jump sound: {heard:?}");
    let (id, where_, volume, pitch) = heard[0];
    assert_eq!(id, sound::ENTITY_SKELETON_HORSE_JUMP_WATER);
    assert_eq!((volume, pitch), (HORSE_SOUND_VOLUME, 1.0));
    assert!(near(where_, at, 1e-9), "at {where_:?}");
}

/// `setStanding(20)` and `AbstractHorse.tick` 39–61: the count goes down once a tick and
/// at 0 the standing ends. The jump's own tick counts 20 → 19; it stands after 18 more,
/// and stands no longer after the 19th.
#[test]
fn a_horse_reared_by_a_jump_stands_for_twenty_ticks() {
    let (mut scene, _) = driving(entity_types::HORSE, &[]);
    scene.input(jump(100));
    for tick in 1..STAND_TICKS - 1 {
        scene.idle();
        assert!(
            standing(&the_horse(&scene)),
            "still standing {tick} ticks on"
        );
    }
    scene.idle();
    assert!(
        !standing(&the_horse(&scene)),
        "standing {} ticks on",
        STAND_TICKS - 1
    );
}

/// `standAnim` as `AbstractHorse.tick` 216–342 moves it, from 0: up by
/// `(1 − s) · 0.4 + 0.05` (at most 1) while standing, then down by
/// `(0.8 s³ − s) · 0.6 − 0.05` (at least 0); the values after each tick from the jump's.
fn stand_anims(ticks: u32, standing_for: u32) -> Vec<f32> {
    let mut s = 0.0_f32;
    let mut values = Vec::new();
    for tick in 0..ticks {
        if tick < standing_for {
            s += (1.0 - s) * 0.4 + 0.05;
            if s > 1.0 {
                s = 1.0;
            }
        } else {
            s += (0.8 * s * s * s - s) * 0.6 - 0.05;
            if s < 0.0 {
                s = 0.0;
            }
        }
        values.push(s);
    }
    values
}

/// `getPassengerAttachmentPoint` 2646–2681: the point moved by
/// `(0, 0.15 · standAnimO · scale, −0.7 · standAnimO · scale)` turned by the horse's yaw,
/// so a rearing horse lifts its rider and puts them back. `standAnimO` is the value
/// before the tick's `tick()`, which runs before its passengers are placed
/// (`tickPassenger` after the vehicle's tick): 0 in the jump's own tick, then each value
/// of [`stand_anims`] a tick late. The horse faces west (yaw 90), so back is east.
#[test]
fn a_rearing_horse_lifts_its_rider_and_moves_them_back_as_it_stands() {
    let yaw = 90.0;
    let mut scene = with_horse(&[], &saddled(entity_types::HORSE), 0.0);
    mounted(&mut scene, EMPTY);
    scene.input(turn(yaw, 0.0));
    scene.idle();
    let at = the_horse(&scene).position;
    assert_eq!(the_horse(&scene).yaw, yaw);
    let anims = stand_anims(60, STAND_TICKS - 1);
    scene.input(jump(100));
    let mut previous = 0.0_f32;
    for (tick, &anim) in anims.iter().enumerate() {
        if tick > 0 {
            scene.idle();
        }
        let o = f64::from(previous);
        let expected = seated_at(entity_types::HORSE, at, yaw, [0.0, 0.15 * o, -0.7 * o]);
        let seated = feet(&scene, 0);
        assert!(
            near(seated, expected, 1e-6),
            "{tick} ticks after the jump, standAnimO {previous}: seated at {seated:?}, \
             expected {expected:?}"
        );
        previous = anim;
    }
    assert!(
        near(
            feet(&scene, 0),
            seated_at(entity_types::HORSE, at, yaw, [0.0; 3]),
            1e-9
        ),
        "back in the plain seat"
    );
}

// ------------------------------------------------------------------------------------
// 4. Getting off (`getDismountLocationForPassenger` 2544–2602 with
// `getDismountLocationInDirection` 2440–2542; R5, R9.8).

/// Player 1 sneaks; the place their client is told they got off to.
fn sneak_off(scene: &mut Scene) -> Vec3 {
    let mut output = scene.input(sneak(true));
    for _ in 0..2 {
        if !rides_told(&output, scene, 0).is_empty() {
            break;
        }
        output = scene.idle();
    }
    let told = rides_told(&output, scene, 0);
    let [(None, at)] = told[..] else {
        panic!("one Rides with nothing: {told:?}");
    };
    at
}

/// Player 1 on the saddled horse at [`HORSE_AT`] facing `yaw` (their own, which it
/// takes), among `more`; the horse's position.
fn facing(yaw: f32, more: &[(BlockPos, clustine_data::BlockState)]) -> (Scene, [f64; 3]) {
    let (mut scene, _) = driving(entity_types::HORSE, more);
    scene.input(turn(yaw, 0.0));
    scene.idle();
    let horse = the_horse(&scene);
    assert_eq!(horse.yaw, yaw);
    assert_eq!(
        horse.position.map(f64::floor),
        [2.0, FLOOR, 1.0],
        "the horse is still in its block: {:?}",
        horse.position
    );
    (scene, horse.position)
}

/// Where the rider is put off on the side `turn` degrees from the horse's yaw (90 for the
/// right of a right-handed rider, -90 for the left), on a floor of height `floor` above
/// the block the search starts in: `getCollisionHorizontalEscapeVector(horse width,
/// rider width, yaw + turn)` from the horse, at `y + floor`.
fn off_at(horse: [f64; 3], yaw: f32, turn: f32, floor: f64) -> [f64; 3] {
    let width = EntityTypeInfo::of(entity_types::HORSE).width;
    let way = escape(width, yaw + turn);
    [
        horse[0] + way[0],
        horse[1].floor() + floor,
        horse[2] + way[2],
    ]
}

/// `getDismountLocationForPassenger` 0–50: a right-handed rider (every Clustine player)
/// first to the horse's right, `yaw + 90`: west of a horse facing south, on the floor.
#[test]
fn a_rider_who_sneaks_gets_off_on_the_horses_right() {
    let (mut scene, horse) = facing(0.0, &[]);
    let at = sneak_off(&mut scene);
    let expected = off_at(horse, 0.0, 90.0, 0.0);
    assert!(
        near(at, expected, 1e-9),
        "off at {at:?}, expected {expected:?}"
    );
    assert!(expected[0] < horse[0], "to the west");
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&scene).is_empty());
}

/// The same for a horse facing west (yaw 90): its right is north.
#[test]
fn a_rider_of_a_horse_facing_west_gets_off_to_the_north() {
    let (mut scene, horse) = facing(90.0, &[]);
    let at = sneak_off(&mut scene);
    let expected = off_at(horse, 90.0, 90.0, 0.0);
    assert!(
        near(at, expected, 1e-9),
        "off at {at:?}, expected {expected:?}"
    );
    assert!(expected[2] < horse[2], "to the north");
}

/// `getDismountLocationInDirection` 81–186: the search starts in the block at the horse's
/// feet and climbs while below the top of the horse's box plus 0.75; a floor is valid
/// below a full block's height (`isBlockFloorValid`). A slab beside the horse is stood on
/// at its top, a block on its top, and a wall two high on its top too, 2 above the
/// floor and still below the horse's top (1.6) and 0.75.
#[test]
fn a_rider_gets_off_on_top_of_what_stands_beside_the_horse_up_to_two_blocks() {
    let slab = state("minecraft:stone_slab[type=bottom]");
    for (blocks_beside, floor) in [
        (vec![(bp(1, -60, 1), slab)], 0.5),
        (vec![(bp(1, -60, 1), blocks::STONE)], 1.0),
        (
            vec![
                (bp(1, -60, 1), blocks::STONE),
                (bp(1, -59, 1), blocks::STONE),
            ],
            2.0,
        ),
    ] {
        let (mut scene, horse) = facing(0.0, &blocks_beside);
        let at = sneak_off(&mut scene);
        let expected = off_at(horse, 0.0, 90.0, floor);
        assert!(
            near(at, expected, 1e-9),
            "a floor {floor} up: off at {at:?}, expected {expected:?}"
        );
    }
}

/// A wall three high on the right reaches above the horse's top and 0.75 (at 117: the
/// block's top beyond it ends the search there), so the rider goes to the left.
#[test]
fn a_rider_whose_right_is_walled_gets_off_on_the_horses_left() {
    let wall: Vec<_> = (-60..=-58).map(|y| (bp(1, y, 1), blocks::STONE)).collect();
    let (mut scene, horse) = facing(0.0, &wall);
    let at = sneak_off(&mut scene);
    let expected = off_at(horse, 0.0, -90.0, 0.0);
    assert!(
        near(at, expected, 1e-9),
        "off at {at:?}, expected {expected:?}"
    );
    assert!(expected[0] > horse[0], "to the east");
}

/// No floor beside the horse (a hole in the floor there): the block's floor height is
/// `getBlockFloorHeight`'s for nothing below either, not valid, and the blocks above have
/// none, so the right is passed over and the rider goes to the left.
#[test]
fn a_rider_beside_a_hole_gets_off_on_the_other_side() {
    let (mut scene, horse) = facing(0.0, &[(bp(1, -61, 1), blocks::AIR)]);
    let at = sneak_off(&mut scene);
    let expected = off_at(horse, 0.0, -90.0, 0.0);
    assert!(
        near(at, expected, 1e-9),
        "off at {at:?}, expected {expected:?}"
    );
}

/// `getDismountLocationInDirection` 48–70 tries each of the rider's dismount poses in
/// turn (a player's: standing, crouching, swimming, `Player.getDismountPoses`), each with
/// its own box: under a ceiling a block above the floor on the right, with a wall above
/// it, neither standing (1.8) nor crouching (1.5) fits anywhere up to the top, but
/// swimming (0.6) does on the floor, so the rider is put off there.
#[test]
fn a_rider_with_room_beside_the_horse_only_to_lie_gets_off_there() {
    let (mut scene, horse) = facing(
        0.0,
        &[
            (bp(1, -59, 1), blocks::STONE),
            (bp(1, -58, 1), blocks::STONE),
            (bp(1, -57, 1), blocks::STONE),
        ],
    );
    let at = sneak_off(&mut scene);
    let expected = off_at(horse, 0.0, 90.0, 0.0);
    assert!(
        near(at, expected, 1e-9),
        "off at {at:?}, expected {expected:?}"
    );
}

/// Both sides walled: `getDismountLocationForPassenger` 107–111, the horse's own
/// position, its feet (not on top of it as a pig's rider would be, R5).
#[test]
fn a_rider_walled_in_on_both_sides_gets_off_where_the_horse_stands() {
    let walls: Vec<_> = (-60..=-58)
        .flat_map(|y| [(bp(1, y, 1), blocks::STONE), (bp(3, y, 1), blocks::STONE)])
        .collect();
    let (mut scene, horse) = facing(0.0, &walls);
    let at = sneak_off(&mut scene);
    assert!(
        near(at, horse, 1e-12),
        "off at {at:?}, the horse at {horse:?}"
    );
}

/// The rider of a horse they do not steer (no saddle) gets off by sneaking the same way:
/// the dismount asks nothing of control.
#[test]
fn the_rider_of_a_horse_without_a_saddle_gets_off_on_its_right_by_sneaking() {
    let mut scene = with_horse(&pen(), &horses(entity_types::HORSE).tamed, 0.0);
    scene.input(used(key(HORSE)));
    assert_eq!(rides(&scene, 0), Some(key(HORSE)));
    let mut output = scene.input(sneak(true));
    let horse = the_horse(&scene);
    for _ in 0..2 {
        if !rides_told(&output, &scene, 0).is_empty() {
            break;
        }
        output = scene.idle();
    }
    let told = rides_told(&output, &scene, 0);
    let [(None, at)] = told[..] else {
        panic!("one Rides with nothing: {told:?}");
    };
    // The horse is its own and may have moved or turned in the tick; read it as it was
    // when the rider got off, within a tick's stroll.
    let expected = off_at(horse.position, horse.yaw, 90.0, 0.0);
    assert!(
        near(at, expected, 0.3),
        "off at {at:?}, expected near {expected:?}"
    );
    assert_eq!(rides(&scene, 0), None);
}

// ------------------------------------------------------------------------------------
// 5. Falls (`AbstractHorse.causeFallDamage` 495–534, `Llama.causeFallDamage` 806–845,
// `LivingEntity.calculateFallDamage` 4600–4632, `playBlockFallSound` 4634–; R9.7).

/// The rider takes the horse straight up `height` blocks a block a move, then down again
/// a block a move, the last landing on the floor: the horse's records after the climb and
/// after the landing, and the landing move's output. `Entity.checkFallDamage` counts the
/// landing move's own drop, so the fall is `height` (as the pig's tests have it).
fn up_and_down(
    scene: &mut Scene,
    at: [f64; 3],
    height: u32,
) -> (clustine_world::Entity, clustine_world::Entity, TickOutput) {
    for step in 1..=height {
        let output = scene.input(drive([at[0], at[1] + f64::from(step), at[2]], false));
        assert!(corrections(&output, scene, 0).is_empty(), "up {step}");
    }
    let top = the_horse(scene);
    let mut landing = None;
    for step in (0..height).rev() {
        let output = scene.input(drive([at[0], at[1] + f64::from(step), at[2]], step == 0));
        assert!(corrections(&output, scene, 0).is_empty(), "down to {step}");
        landing = Some(output);
    }
    (top, the_horse(scene), landing.expect("a landing"))
}

/// The sounds a fall may make: the horse's landing and a foal's, a living body's own
/// small and big falls, and the stone's fall.
struct FallSounds {
    land: Vec<(Vec3, f32, f32)>,
    foal_land: Vec<(Vec3, f32, f32)>,
    living: usize,
    stone: Vec<(Vec3, f32, f32)>,
}

fn fall_sounds(outputs: &[TickOutput]) -> FallSounds {
    FallSounds {
        land: sounds_in(outputs, sound::ENTITY_HORSE_LAND),
        foal_land: sounds_in(outputs, sound::ENTITY_BABY_HORSE_LAND),
        living: sounds_in(outputs, sound::ENTITY_GENERIC_SMALL_FALL).len()
            + sounds_in(outputs, sound::ENTITY_GENERIC_BIG_FALL).len(),
        stone: sounds_in(outputs, sound::BLOCK_STONE_FALL),
    }
}

/// A steered horse brought down `height` blocks: its health lost, and the sounds of the
/// landing.
fn driven_fall(height: u32) -> (f32, FallSounds, Scene, clustine_world::Entity) {
    let (mut scene, at) = driving(entity_types::HORSE, &[]);
    let before = health(&the_horse(&scene));
    let (top, landed, output) = up_and_down(&mut scene, at, height);
    assert_eq!(top.fall_distance, 0.0, "nothing gathered while moving up");
    assert_eq!(health(&top), before, "nothing lost while moving up");
    assert_eq!(landed.fall_distance, 0.0, "reset by the landing");
    (
        before - health(&landed),
        fall_sounds(&[output]),
        scene,
        landed,
    )
}

/// `causeFallDamage` 0–27: above one block, the horse's landing sound at
/// `playSound(HORSE_LAND, 0.4f, 1.0f)`; `calculateFallDamage` with the safe fall of 6 and
/// the multiplier 0.5 gives nothing for 2 to 7 blocks, so no hurt and no block's fall
/// sound (38–44 return first), and no living body's fall sound ever (the horse's
/// `causeFallDamage` does not call `LivingEntity`'s).
#[test]
fn a_steered_horse_brought_down_two_to_seven_blocks_lands_with_its_sound_and_takes_nothing() {
    for height in [2, 4, 7] {
        let (lost, heard, scene, landed) = driven_fall(height);
        assert!(fall_damage(f64::from(height)) <= 0.0);
        assert_eq!(lost, 0.0, "a fall of {height}");
        assert_eq!(heard.land.len(), 1, "a fall of {height}: {:?}", heard.land);
        let (where_, volume, pitch) = heard.land[0];
        assert_eq!((volume, pitch), (HORSE_SOUND_VOLUME, 1.0));
        assert!(near(where_, landed.position, 1e-9), "at {where_:?}");
        assert!(heard.foal_land.is_empty());
        assert_eq!(heard.living, 0, "no living body's fall sound");
        assert!(heard.stone.is_empty(), "no block's fall sound");
        assert_eq!(rides(&scene, 0), Some(key(HORSE)), "its rider stays on");
    }
}

/// `causeFallDamage` 0–3: `distance > 1.0` strictly, so a drop of one block makes no
/// sound.
#[test]
fn a_steered_horse_brought_down_one_block_lands_silently() {
    let (lost, heard, _, _) = driven_fall(1);
    assert_eq!(lost, 0.0);
    assert!(heard.land.is_empty(), "{:?}", heard.land);
    assert!(heard.stone.is_empty());
    assert_eq!(heard.living, 0);
}

/// `causeFallDamage` 30–67 with `calculateFallDamage`: `floor((8 + 1e-6 − 6) · 0.5)` = 1
/// point for 8 blocks, 2 for 10, 4 for 14; the hurt, then the fall passed to the rider
/// (a creative player, who takes none), then the block's fall sound: the stone's
/// `block.stone.fall` at its volume × 0.5 and pitch × 0.75. The landing sound comes
/// first, and no living body's fall sound.
#[test]
fn a_steered_horse_brought_down_eight_blocks_or_more_takes_half_its_fall_past_six() {
    for (height, points) in [(8, 1.0), (10, 2.0), (14, 4.0)] {
        assert_eq!(fall_damage(f64::from(height)), points);
        let (lost, heard, scene, landed) = driven_fall(height);
        assert_eq!(lost, points, "a fall of {height}");
        assert_eq!(heard.land.len(), 1, "a fall of {height}: the landing sound");
        assert_eq!(
            heard.living, 0,
            "a fall of {height}: no living body's fall sound"
        );
        assert_eq!(
            heard.stone.len(),
            1,
            "a fall of {height}: {:?}",
            heard.stone
        );
        let (where_, volume, pitch) = heard.stone[0];
        assert_eq!((volume, pitch), (STONE_FALL_VOLUME, STONE_FALL_PITCH));
        assert!(near(where_, landed.position, 1e-9), "at {where_:?}");
        assert_eq!(
            rides(&scene, 0),
            Some(key(HORSE)),
            "its creative rider stays on"
        );
    }
}

/// A horse left to fall by itself, from `height` above the floor, until it lands: its
/// health lost and the sounds of all the ticks it took.
fn own_fall(horse: &clustine_world::Entity, height: f64) -> (f32, FallSounds) {
    let mut falling = set(
        horse,
        HORSE,
        [HORSE_AT[0], FLOOR + height, HORSE_AT[2]],
        0.0,
    );
    falling.on_ground = false;
    let mut scene = with_entities(&pen(), vec![falling.clone()]);
    let before = health(&the_horse(&scene));
    let mut seen = Vec::new();
    let landed = scene.idle_until(200, &mut seen, |scene, _| {
        let horse = the_horse(scene);
        horse.on_ground && horse.position[1] < FLOOR + 0.5
    });
    assert!(landed, "it never landed: {:?}", the_horse(&scene).position);
    // A tick more, for whatever the landing tick left.
    seen.push(scene.idle());
    (before - health(&the_horse(&scene)), fall_sounds(&seen))
}

/// The same rules for a horse that falls by itself (`Entity.checkFallDamage` from its own
/// `move`): from 5 blocks the landing sound and nothing more; from 11, `floor((11 − 6) ·
/// 0.5)` = 2 points and the stone's sound; never a living body's fall sound.
#[test]
fn a_horse_falling_by_itself_lands_with_its_sound_and_half_its_fall_past_six() {
    let horse = horses(entity_types::HORSE).tamed;
    let (lost, heard) = own_fall(&horse, 5.0);
    assert_eq!(lost, 0.0);
    assert_eq!(heard.land.len(), 1, "{:?}", heard.land);
    assert_eq!(
        (heard.land[0].1, heard.land[0].2),
        (HORSE_SOUND_VOLUME, 1.0)
    );
    assert!(heard.stone.is_empty());
    assert_eq!(heard.living, 0);

    let (lost, heard) = own_fall(&horse, 11.0);
    assert_eq!(lost, fall_damage(11.0));
    assert_eq!(lost, 2.0);
    assert_eq!(heard.land.len(), 1, "{:?}", heard.land);
    assert_eq!(heard.stone.len(), 1, "{:?}", heard.stone);
    assert_eq!(heard.living, 0);
}

/// `causeFallDamage` 6–20: a foal's landing is `HORSE_LAND_BABY`, not the horse's.
#[test]
fn a_foal_lands_with_a_foals_sound() {
    let (lost, heard) = own_fall(&horses(entity_types::HORSE).foal, 9.0);
    assert_eq!(lost, fall_damage(9.0));
    assert_eq!(heard.foal_land.len(), 1, "{:?}", heard.foal_land);
    assert_eq!(
        (heard.foal_land[0].1, heard.foal_land[0].2),
        (HORSE_SOUND_VOLUME, 1.0)
    );
    assert!(
        heard.land.is_empty(),
        "not the grown horse's: {:?}",
        heard.land
    );
    assert_eq!(heard.living, 0);
}

/// Every kind of the family that falls by `AbstractHorse.causeFallDamage` lands with the
/// horse's sound (no kind overrides it but the llama).
#[test]
fn every_kind_but_the_llama_lands_with_the_horses_sound() {
    for kind in [
        entity_types::DONKEY,
        entity_types::MULE,
        entity_types::SKELETON_HORSE,
        entity_types::ZOMBIE_HORSE,
    ] {
        let (lost, heard) = own_fall(&horses(kind).tamed, 9.0);
        assert_eq!(lost, fall_damage(9.0), "kind {kind}");
        assert_eq!(heard.land.len(), 1, "kind {kind}: {:?}", heard.land);
        assert_eq!(heard.living, 0, "kind {kind}");
    }
}

/// `Llama.causeFallDamage`: no landing sound at all; nothing below the damage; from 11
/// blocks the same 2 points (its attributes are the family's), the fall passed on and the
/// block's sound.
#[test]
fn a_llama_lands_without_a_sound_of_its_own() {
    let llama = horses(entity_types::LLAMA).tamed;
    let (lost, heard) = own_fall(&llama, 5.0);
    assert_eq!(lost, 0.0);
    assert!(heard.land.is_empty() && heard.foal_land.is_empty());
    assert!(heard.stone.is_empty());
    assert_eq!(heard.living, 0);

    let (lost, heard) = own_fall(&llama, 11.0);
    assert_eq!(lost, 2.0);
    assert!(heard.land.is_empty() && heard.foal_land.is_empty());
    assert_eq!(heard.stone.len(), 1, "{:?}", heard.stone);
    assert_eq!(heard.living, 0);
}

// ------------------------------------------------------------------------------------
// 6. What else a rider notices.

/// R3 for a horse: `LivingEntity.maxUpStep` is the step height attribute, a horse's 1.0
/// (`createBaseHorseAttributes` 30–34), at least 1.0 while a player controls it: a
/// steered horse is driven up a stair a block a move on the ground, every move accepted,
/// and gathers no fall.
#[test]
fn a_steered_horse_is_driven_up_a_stair_of_whole_blocks() {
    let stair: Vec<_> = (0..4)
        .flat_map(|step: i32| (0..=step).map(move |y| (bp(4 + step, -60 + y, 1), blocks::STONE)))
        .collect();
    let (mut scene, at) = driving(entity_types::HORSE, &stair);
    let before = health(&the_horse(&scene));
    for step in 0..4 {
        let to = [4.2 + f64::from(step), FLOOR + f64::from(step + 1), at[2]];
        let output = scene.input(drive(to, true));
        assert!(corrections(&output, &scene, 0).is_empty(), "step {step}");
        let horse = the_horse(&scene);
        assert_eq!(bits(horse.position), bits(to), "step {step}");
        assert_eq!(horse.fall_distance, 0.0);
        assert_eq!(health(&horse), before);
    }
}

/// n5-brief 2.2 step 7 with R3's `getMaximumFlyingTicks`: `ceil(80 · max(0.08 / g, 1))`,
/// 80 ticks for a horse's gravity of 0.08. A rider whose client holds the horse in the
/// air is put out after that; one tick either way is left to where in the tick the region
/// counts, as N5's test of the boat does.
#[test]
fn a_rider_who_holds_the_horse_in_the_air_is_put_out_after_eighty_ticks() {
    let (mut scene, at) = driving(entity_types::HORSE, &[]);
    let up = [at[0], at[1] + 2.0, at[2]];
    assert!(corrections(&scene.input(drive(up, false)), &scene, 0).is_empty());
    assert!(corrections(&scene.input(drive(up, false)), &scene, 0).is_empty());
    let mut put_out = None;
    for tick in 1..=200 {
        let output = scene.idle();
        let kicked = events_of(&output, scene.actors[0].id)
            .iter()
            .any(|event| matches!(event, PlayerEvent::Kicked { .. }));
        if kicked {
            put_out = Some(tick);
            break;
        }
    }
    let after = put_out.expect("put out");
    assert!((79..=81).contains(&after), "put out {after} ticks after");
}

/// A pool of still water three deep around the horse's block, walled so that it stays.
fn pool() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let water = state("minecraft:water[level=0]");
    let mut blocks_of = Vec::new();
    for y in -60..=-58 {
        for x in 0..=4 {
            for z in -1..=3 {
                let edge = x == 0 || x == 4 || z == -1 || z == 3;
                blocks_of.push((bp(x, y, z), if edge { blocks::STONE } else { water }));
            }
        }
    }
    blocks_of
}

/// R5 and R9.8: the rider's `baseTick` with their eyes in water and no bubble column
/// there puts them off a vehicle in `#dismounts_underwater`, which lists the horse,
/// donkey, mule, zombie horse and llama. A horse standing in water three deep has its
/// rider's eyes 2.46 above its feet, in the water: they are put off as soon as they are
/// on.
#[test]
fn a_rider_whose_eyes_are_under_water_is_put_off_a_horse() {
    for kind in [entity_types::HORSE, entity_types::DONKEY] {
        let mut scene = with_horse(&pool(), &saddled(kind), 0.0);
        scene.input(used(key(HORSE)));
        let mut seen = Vec::new();
        let off = scene.idle_until(3, &mut seen, |scene, _| rides(scene, 0).is_none());
        assert!(off, "kind {kind}: still on in the water");
    }
}

/// `#dismounts_underwater` does not list the skeleton horse, which walks under water: its
/// rider stays on with their eyes in it.
#[test]
fn a_rider_stays_on_a_skeleton_horse_under_water() {
    let mut scene = with_horse(&pool(), &saddled(entity_types::SKELETON_HORSE), 0.0);
    scene.input(used(key(HORSE)));
    for tick in 0..40 {
        scene.idle();
        assert_eq!(rides(&scene, 0), Some(key(HORSE)), "put off at tick {tick}");
    }
    let eyes = feet(&scene, 0).y + 1.62;
    let block = scene.block(bp(2, eyes.floor() as i32, 1));
    assert_eq!(
        block,
        state("minecraft:water[level=0]"),
        "the eyes are in water"
    );
}
