//! Tests of riding a pig (step N7h), written from the specification by someone who did
//! not write the code: the research note `docs/groundwork/n7h-riding-research.md` (cited
//! as R1 to R9, its sections), the plan `docs/groundwork/n7h-riding-plan.md` (section 10
//! over the rest), N5's `n5-brief.md` and `n5-plan.md` for what of a vehicle is reused,
//! and the 26.3 bytecode (`reference/n7-javap/`, `reference/n5-javap/`) where a rule
//! needed checking. Regions are driven by their public interface alone.
//!
//! The pigs are the game's own: hatched by a spawn egg in a scratch region and saddled
//! there by a saddle's use, their records then set into the world of each test where it
//! wants them. What the game keeps of a pig and no client is shown (whether it boosts,
//! for how long yet) is read from what it does: two regions given the same inputs but
//! one, whose pigs are compared after it.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60), in creative mode as
//! every Clustine player is.

mod n6_scene;

use clustine_data::{blocks, entity_types, items};
use clustine_sim::api::{Keys, PlayerEvent, PlayerInput, VehicleMove};
use clustine_sim::shown::{DataShown, head_yaw, index, mob_values};
use clustine_sim::{TickInputs, TickOutput};
use clustine_world::{
    Arrival, BlockPos, ChunkPos, Due, EffectId, Entity, EntityId, OFF_HAND, Vec3,
};
use n6_scene::*;
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The numbers of the specification.

/// A pig's passenger point (R6: `0x3febccccc0000000`).
const PIG_SEAT: f64 = 0.868_749_976_158_142_1;
/// The player's vehicle attachment (`Avatar.DEFAULT_VEHICLE_ATTACHMENT`).
const PLAYER_ATTACHMENT: f64 = 0.6;
/// A pig's height (`EntityType.PIG`, 0.9 × 0.9), a float the box widens.
const PIG_HEIGHT: f32 = 0.9;
/// `DATA_BOOST_TIME`'s index (R4).
const BOOST: u8 = 18;
/// `ItemBasedSteering.boost`: `nextInt(841) + 140` (R4).
const SHORTEST_BOOST: i32 = 140;
const LONGEST_BOOST: i32 = 980;
/// A pig's health (`Pig.createAttributes`).
const PIG_HEALTH: f32 = 10.0;
/// `getMaximumFlyingTicks` for a pig: `ceil(80 · max(0.08 / 0.08, 1))` (R3).
const PIG_FLOATING: u32 = 80;

/// The top of the stone floor.
const FLOOR: f64 = -60.0;

/// What the hotbar holds: slot 0 is empty.
const EMPTY: u8 = 0;
const STICK: u8 = 1;
const CARROT: u8 = 2;
const POTATO: u8 = 3;
const BEETROOT: u8 = 4;
const SADDLE: u8 = 5;

/// The key of the pig every test sets into its world, and of a second one.
const PIG: u32 = 1;
const OTHER_PIG: u32 = 2;
/// Where the pig stands, within player 1's reach of the spawn.
const PIG_AT: [f64; 3] = [2.5, FLOOR, 1.5];

fn hotbar() -> clustine_sim::api::Inventory {
    inventory(&[
        (usize::from(STICK), stack(items::CARROT_ON_A_STICK, 1)),
        (usize::from(CARROT), stack(items::CARROT, 1)),
        (usize::from(POTATO), stack(items::POTATO, 1)),
        (usize::from(BEETROOT), stack(items::BEETROOT, 1)),
        (usize::from(SADDLE), stack(items::SADDLE, 1)),
    ])
}

// ------------------------------------------------------------------------------------
// The pigs, as the game makes them.

/// A saddled adult pig, an adult with no saddle and a piglet, each as its region's
/// record had it: hatched by an egg on the floor, the piglet by the egg on the adult, the
/// saddle put on by its use.
struct Pigs {
    saddled: Entity,
    bare: Entity,
    piglet: Entity,
}

fn pigs() -> Pigs {
    let mut scene = Scene::alone(
        &World::default(),
        inventory(&[
            (0, stack(items::PIG_SPAWN_EGG, 1)),
            (1, stack(items::SADDLE, 1)),
        ]),
        SPAWN,
    );
    let hatch = |scene: &mut Scene, at: BlockPos| -> EntityId {
        let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
        scene.select(0);
        scene.click(at, clustine_sim::api::Face::Top);
        let made: Vec<EntityId> = scene
            .of_kind(entity_types::PIG)
            .iter()
            .map(|entity| entity.key)
            .filter(|key| !before.contains(key))
            .collect();
        assert_eq!(made.len(), 1, "the egg makes one pig");
        made[0]
    };
    let adult = hatch(&mut scene, bp(2, -61, 2));
    let bare = hatch(&mut scene, bp(-2, -61, 2));
    scene.input(used(adult));
    let piglet = scene
        .of_kind(entity_types::PIG)
        .into_iter()
        .find(|pig| pig.key != adult && pig.key != bare)
        .expect("the egg on the adult makes a piglet")
        .key;
    scene.select(1);
    scene.input(used(adult));
    let record = |key: EntityId| {
        scene
            .all()
            .into_iter()
            .find(|entity| entity.key == key)
            .expect("there")
    };
    Pigs {
        saddled: record(adult),
        bare: record(bare),
        piglet: record(piglet),
    }
}

/// `pig` as entity number `n` standing at `at`, facing `yaw`, at rest.
fn set(pig: &Entity, n: u32, at: [f64; 3], yaw: f32) -> Entity {
    let mut pig = pig.clone();
    pig.key = key(n);
    pig.uuid = Uuid::from_u128(u128::from(n) + 7_000);
    pig.position = at;
    pig.motion = [0.0; 3];
    pig.yaw = yaw;
    pig.pitch = 0.0;
    pig.on_ground = true;
    pig.fall_distance = 0.0;
    pig.riding = Default::default();
    pig
}

fn saddled_at(at: [f64; 3]) -> Entity {
    set(&pigs().saddled, PIG, at, 0.0)
}

/// A stone wall two high along the four sides of the square from (-4, -4) to (5, 5),
/// which keeps a pig left to itself within the land that ticks.
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

/// A scene of one region with player 1 at the spawn holding [`hotbar`], slot 0 (empty)
/// selected, the blocks `more` and the saddled pig at [`PIG_AT`].
fn with_pig(more: &[(BlockPos, clustine_data::BlockState)]) -> Scene {
    with_entities(more, vec![saddled_at(PIG_AT)])
}

fn with_entities(more: &[(BlockPos, clustine_data::BlockState)], entities: Vec<Entity>) -> Scene {
    let world = World::of(more).with_entities(&entities);
    Scene::alone(&world, hotbar(), SPAWN)
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

/// A use of what the main hand holds (`use_item`), looking at `rotation`.
fn use_stick(scene: &mut Scene, who: usize) -> TickOutput {
    scene.actors[who].sequence += 1;
    let sequence = scene.actors[who].sequence;
    scene.input_of(
        who,
        PlayerInput::UseItem {
            rotation: (0.0, 0.0),
            sneaking: false,
            sequence,
        },
    )
}

// ------------------------------------------------------------------------------------
// What the region says.

fn pig_of(scene: &Scene, n: u32) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key(n))
}

fn the_pig(scene: &Scene) -> Entity {
    pig_of(scene, PIG).expect("the pig is there")
}

fn rides(scene: &Scene, who: usize) -> Option<EntityId> {
    scene
        .region
        .player_state(scene.actors[who].id)
        .expect("there")
        .riding
        .map(|riding| riding.vehicle)
}

fn feet(scene: &Scene, who: usize) -> Vec3 {
    scene
        .region
        .player(scene.actors[who].id)
        .expect("there")
        .1
        .position
}

fn passengers(scene: &Scene) -> Vec<EntityId> {
    the_pig(scene)
        .riding
        .passengers
        .iter()
        .map(|seat| seat.key)
        .collect()
}

fn shown_at(pig: &Entity, at: u8) -> Option<(DataShown, bool)> {
    mob_values(pig.kind, &pig.data)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, default)| (value, default))
}

fn boost_shown(pig: &Entity) -> i32 {
    match shown_at(pig, BOOST) {
        Some((DataShown::Int(total), _)) => total,
        other => panic!("a pig shows its boost as an int at 18: {other:?}"),
    }
}

fn health(pig: &Entity) -> f32 {
    match shown_at(pig, index::HEALTH) {
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

fn kicked(output: &TickOutput, scene: &Scene) -> bool {
    events_of(output, scene.actors[0].id)
        .iter()
        .any(|event| matches!(event, PlayerEvent::Kicked { .. }))
}

fn bits(at: [f64; 3]) -> [u64; 3] {
    at.map(f64::to_bits)
}

fn near(a: Vec3, b: [f64; 3], within: f64) -> bool {
    (a.x - b[0]).abs() <= within && (a.y - b[1]).abs() <= within && (a.z - b[2]).abs() <= within
}

/// Player 1 gets on the pig with what slot `slot` holds, then holds the stick: the pig
/// is theirs to drive, and where it stood when they took it.
fn mounted(scene: &mut Scene, slot: u8) -> [f64; 3] {
    scene.select(slot);
    scene.input(used(key(PIG)));
    assert_eq!(rides(scene, 0), Some(key(PIG)), "player 1 rides the pig");
    scene.select(STICK);
    scene.idle();
    the_pig(scene).position
}

/// Player 1 on the pig at [`PIG_AT`] holding the stick, among `more`.
fn driving(more: &[(BlockPos, clustine_data::BlockState)]) -> (Scene, [f64; 3]) {
    let mut scene = with_pig(more);
    let at = mounted(&mut scene, EMPTY);
    (scene, at)
}

// ------------------------------------------------------------------------------------
// 1. Mounting (R1).

/// R1, `Pig.mobInteract`: a saddled, unridden pig clicked by a player who does not
/// sneak and holds nothing takes them on, and everyone is told who rides it.
#[test]
fn a_saddled_pig_takes_a_player_who_clicks_it_with_an_empty_hand() {
    let mut scene = with_pig(&[]);
    let output = scene.input(used(key(PIG)));
    assert_eq!(rides(&scene, 0), Some(key(PIG)));
    assert_eq!(passengers(&scene), [scene.me()]);
    let told = rides_told(&output, &scene, 0);
    assert!(
        matches!(told[..], [(Some(vehicle), _)] if vehicle == key(PIG)),
        "the rider's client is told: {told:?}"
    );
}

/// R1: a carrot on a stick is not the pig's food, so it mounts as an empty hand does.
#[test]
fn a_saddled_pig_takes_a_player_who_clicks_it_holding_a_carrot_on_a_stick() {
    let mut scene = with_pig(&[]);
    scene.select(STICK);
    scene.input(used(key(PIG)));
    assert_eq!(rides(&scene, 0), Some(key(PIG)));
}

/// R1: `!player.isSecondaryUseActive()`, the interact's own sneaking flag.
#[test]
fn a_player_who_sneaks_is_not_taken_on() {
    let mut scene = with_pig(&[]);
    scene.input(used_sneaking(key(PIG)));
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&scene).is_empty());
}

/// R1: `isFood` (`#pig_food`: carrot, potato, beetroot) goes to `Animal.mobInteract`
/// instead, which feeds the pig; no rider.
#[test]
fn a_player_who_holds_the_pigs_food_is_not_taken_on() {
    for slot in [CARROT, POTATO, BEETROOT] {
        let mut scene = with_pig(&[]);
        scene.select(slot);
        scene.input(used(key(PIG)));
        assert_eq!(rides(&scene, 0), None, "holding slot {slot}");
        assert!(passengers(&scene).is_empty());
    }
}

/// R1: `isSaddled()` first.
#[test]
fn a_pig_with_no_saddle_takes_nobody() {
    let bare = set(&pigs().bare, PIG, PIG_AT, 0.0);
    let mut scene = with_entities(&[], vec![bare]);
    scene.input(used(key(PIG)));
    assert_eq!(rides(&scene, 0), None);
    scene.select(STICK);
    scene.input(used(key(PIG)));
    assert_eq!(rides(&scene, 0), None);
}

/// R1, `Pig.canUseSlot(SADDLE)` = alive and not a baby: a piglet takes no saddle, so it
/// takes no rider either.
#[test]
fn a_piglet_takes_no_saddle_and_no_rider() {
    let piglet = set(&pigs().piglet, PIG, PIG_AT, 0.0);
    let mut scene = with_entities(&[], vec![piglet]);
    scene.select(SADDLE);
    scene.input(used(key(PIG)));
    scene.select(EMPTY);
    scene.input(used(key(PIG)));
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&scene).is_empty());
}

/// R1, `!isVehicle()` and `Entity.canAddPassenger` = no passenger yet: one rider.
#[test]
fn a_ridden_pig_takes_no_second_rider() {
    let mut scene = with_pig(&[]);
    let second = scene.join(2, Vec3::new(3.5, FLOOR, 0.5));
    scene.input(used(key(PIG)));
    assert_eq!(rides(&scene, 0), Some(key(PIG)));
    scene.input_of(second, used(key(PIG)));
    assert_eq!(rides(&scene, second), None);
    assert_eq!(passengers(&scene), [scene.me()]);
}

/// R6: the rider's feet are at the pig's position plus its passenger point
/// (0, 0.86875, 0) less their own vehicle attachment (0, 0.6, 0), x and z unchanged.
#[test]
fn the_rider_sits_at_the_pigs_passenger_point_less_their_attachment() {
    let (mut scene, _) = driving(&[]);
    scene.idle();
    let pig = the_pig(&scene).position;
    let seated = feet(&scene, 0);
    let expected = [pig[0], pig[1] + PIG_SEAT - PLAYER_ATTACHMENT, pig[2]];
    assert!(
        near(seated, expected, 1e-9),
        "seated at {seated:?}, expected {expected:?}"
    );
}

// ------------------------------------------------------------------------------------
// 2. Who controls it (R2, R3).

/// R2, `Pig.getControllingPassenger`, and R3: a rider holding the stick controls the
/// pig, which is then the client's: their vehicle move puts it where they say, with no
/// correction, and they follow it to the seat.
#[test]
fn a_rider_holding_the_stick_drives_the_pig_where_their_client_puts_it() {
    let (mut scene, at) = driving(&[]);
    let to = [at[0] + 0.5, at[1], at[2] + 0.25];
    let output = scene.input(drive(to, true));
    assert!(corrections(&output, &scene, 0).is_empty());
    assert_eq!(bits(the_pig(&scene).position), bits(to));
    scene.idle();
    assert!(near(
        feet(&scene, 0),
        [to[0], to[1] + PIG_SEAT - PLAYER_ATTACHMENT, to[2]],
        1e-9
    ));
}

/// R2: `isHolding` is the main hand or the off hand.
#[test]
fn a_rider_holding_the_stick_in_the_off_hand_drives_the_pig() {
    let world = World::default().with_entities(&[saddled_at(PIG_AT)]);
    let mut scene = Scene::alone(
        &world,
        inventory(&[(OFF_HAND, stack(items::CARROT_ON_A_STICK, 1))]),
        SPAWN,
    );
    scene.input(used(key(PIG)));
    assert_eq!(rides(&scene, 0), Some(key(PIG)));
    scene.idle();
    let at = the_pig(&scene).position;
    let to = [at[0] + 0.5, at[1], at[2]];
    let output = scene.input(drive(to, true));
    assert!(corrections(&output, &scene, 0).is_empty());
    assert_eq!(bits(the_pig(&scene).position), bits(to));
}

/// R2 and R3: control is asked every time, so with the stick put away the rider's
/// moves are a passenger's who does not control the vehicle: answered with the pig's
/// place, which stays.
#[test]
fn with_the_stick_put_away_the_riders_moves_are_refused_with_the_pigs_place() {
    let (mut scene, _) = driving(&[]);
    scene.select(EMPTY);
    let at = the_pig(&scene).position;
    let output = scene.input(drive([at[0] + 0.5, at[1], at[2]], true));
    let sent = corrections(&output, &scene, 0);
    assert_eq!(sent.len(), 1, "one correction: {sent:?}");
    assert!(
        near(sent[0], at, 1e-9),
        "sent {:?}, the pig at {at:?}",
        sent[0]
    );
    assert_eq!(bits(the_pig(&scene).position), bits(at));
}

/// R3: a pig ridden without the stick is the server's: its AI on, it wanders with its
/// rider on it. In a pen, so that it stays where the land ticks.
#[test]
fn with_the_stick_put_away_the_pig_wanders_with_its_rider() {
    let (mut scene, _) = driving(&pen());
    scene.select(EMPTY);
    let at = the_pig(&scene).position;
    let mut seen = Vec::new();
    let wandered = scene.idle_until(3_000, &mut seen, |scene, _| {
        horizontal(the_pig(scene).position, at) > 0.5
    });
    assert!(wandered, "the pig never moved from {at:?}");
    assert_eq!(rides(&scene, 0), Some(key(PIG)), "with its rider on it");
}

/// R2: taking the stick back hands the pig to the rider again.
#[test]
fn taking_the_stick_back_hands_the_pig_over_again() {
    let (mut scene, _) = driving(&[]);
    scene.select(EMPTY);
    scene.idle();
    scene.select(STICK);
    let at = the_pig(&scene).position;
    let to = [at[0] - 0.5, at[1], at[2] + 0.5];
    let output = scene.input(drive(to, true));
    assert!(corrections(&output, &scene, 0).is_empty());
    assert_eq!(bits(the_pig(&scene).position), bits(to));
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// ------------------------------------------------------------------------------------
// 3. A driven pig's tick (R3).

/// R3, `Pig.tickRidden`: `setRot(rider yRot, rider xRot · 0.5f)`, and the head and body
/// yaw to it, run after the packets, so the rider's turn shows in that tick.
#[test]
fn a_driven_pig_turns_to_its_riders_yaw_and_half_their_pitch() {
    let (mut scene, _) = driving(&[]);
    for (yaw, pitch) in [(30.0_f32, 40.0_f32), (-150.0, -50.0), (179.0, 89.0)] {
        scene.input(turn(yaw, pitch));
        let pig = the_pig(&scene);
        assert_eq!(pig.yaw, yaw % 360.0, "yaw");
        assert_eq!(pig.pitch, (pitch * 0.5) % 360.0, "pitch");
        assert_eq!(head_yaw(&pig.data), Some(yaw), "head yaw");
    }
}

/// R3, `LivingEntity.aiStep` with `isEffectiveAi()` false and `travelRidden`: no goals,
/// no travel of its own, its motion zero after every tick. Left driven for 1200 ticks,
/// in which a pig left to itself would have strolled almost surely, it stays exactly
/// where it was.
#[test]
fn a_driven_pig_has_no_motion_and_no_ai_of_its_own() {
    let (mut scene, at) = driving(&pen());
    for tick in 0..1_200 {
        scene.idle();
        let pig = the_pig(&scene);
        assert_eq!(pig.motion, [0.0; 3], "motion at tick {tick}");
        assert_eq!(bits(pig.position), bits(at), "moved at tick {tick}");
    }
}

// ------------------------------------------------------------------------------------
// 4. The boost (R4).

/// Two scenes made alike, given the same inputs but where a test says otherwise: what
/// the game keeps of a boost and shows nobody is read from whether one input changed
/// the pig.
struct Twins {
    a: Scene,
    b: Scene,
}

impl Twins {
    fn new(make: impl Fn() -> Scene) -> Self {
        Self {
            a: make(),
            b: make(),
        }
    }

    /// Player 1 on the pig holding the stick in both.
    fn driving() -> Self {
        let twins = Self::new(|| driving(&[]).0);
        twins.assert_alike("made alike");
        twins
    }

    fn both(&mut self, act: impl Fn(&mut Scene)) {
        act(&mut self.a);
        act(&mut self.b);
    }

    fn idle(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.a.idle();
            self.b.idle();
        }
    }

    /// A uses the stick and B idles in the same tick: whether the use changed the pig.
    fn use_in_a(&mut self) -> bool {
        use_stick(&mut self.a, 0);
        self.b.idle();
        the_pig(&self.a) != the_pig(&self.b)
    }

    fn assert_alike(&self, when: &str) {
        assert!(
            the_pig(&self.a) == the_pig(&self.b),
            "{when}: the pigs differ:\n{:?}\n{:?}",
            the_pig(&self.a),
            the_pig(&self.b)
        );
    }
}

/// R4, `FoodOnAStickItem.use` and `ItemBasedSteering.boost`: a use of the stick by the
/// player who controls the pig starts a boost whose total, `nextInt(841) + 140` from the
/// pig's own random, is shown at index 18 (an int, no longer the default 0).
#[test]
fn a_use_of_the_stick_while_driving_boosts_the_pig_for_140_to_980_ticks_shown_at_index_18() {
    let (mut scene, _) = driving(&[]);
    assert_eq!(
        shown_at(&the_pig(&scene), BOOST),
        Some((DataShown::Int(0), true))
    );
    use_stick(&mut scene, 0);
    let pig = the_pig(&scene);
    let total = boost_shown(&pig);
    assert!(
        (SHORTEST_BOOST..=LONGEST_BOOST).contains(&total),
        "a total of {total}"
    );
    assert_eq!(shown_at(&pig, BOOST), Some((DataShown::Int(total), false)));
}

/// R4: `if (boosting) return false`: a use while boosting draws nothing and changes
/// nothing of the pig.
#[test]
fn a_use_of_the_stick_while_boosting_changes_nothing() {
    let mut twins = Twins::driving();
    twins.both(|scene| {
        use_stick(scene, 0);
    });
    twins.assert_alike("both boosted");
    twins.idle(5);
    assert!(!twins.use_in_a(), "a second use changed the boosting pig");
    assert!(!twins.use_in_a(), "a third use changed the boosting pig");
}

/// R4: the use asks `player.getControlledVehicle()`; a player who rides nothing boosts
/// nothing, and the pig is as if they had not used the stick.
#[test]
fn a_use_of_the_stick_by_a_player_who_does_not_drive_does_nothing_to_the_pig() {
    let mut twins = Twins::new(|| with_pig(&[]));
    twins.both(|scene| {
        scene.select(STICK);
    });
    assert!(!twins.use_in_a(), "a use on foot changed the pig");
    assert_eq!(boost_shown(&the_pig(&twins.a)), 0);
}

/// R2 and R4: the stick in the off hand both drives and boosts (a client tries the off
/// hand when the main hand's use did nothing).
#[test]
fn a_use_of_the_stick_in_the_off_hand_boosts_the_pig() {
    let world = World::default().with_entities(&[saddled_at(PIG_AT)]);
    let mut scene = Scene::alone(
        &world,
        inventory(&[(OFF_HAND, stack(items::CARROT_ON_A_STICK, 1))]),
        SPAWN,
    );
    scene.input(used(key(PIG)));
    scene.idle();
    scene.actors[0].sequence += 1;
    let sequence = scene.actors[0].sequence;
    let use_item = PlayerInput::UseItem {
        rotation: (0.0, 0.0),
        sneaking: false,
        sequence,
    };
    scene.inputs_of(
        0,
        vec![
            use_item.clone(),
            PlayerInput::WithOffHand(Box::new(use_item)),
        ],
    );
    let total = boost_shown(&the_pig(&scene));
    assert!((SHORTEST_BOOST..=LONGEST_BOOST).contains(&total), "{total}");
}

/// R4, `tickBoost`: `if (boosting && boostTime++ > total) boosting = false`, run from
/// `tickRidden` after the tick's packets. The use's own tick counts 0 → 1; the boost is
/// still on at the input of the tick after `total` more, and over at the input of the
/// one after that, where a use boosts again.
#[test]
fn a_boost_ends_after_its_total_and_a_use_then_boosts_again() {
    let mut twins = Twins::driving();
    twins.both(|scene| {
        use_stick(scene, 0);
    });
    let total = boost_shown(&the_pig(&twins.a));
    twins.idle(u32::try_from(total).expect("positive"));
    assert!(
        !twins.use_in_a(),
        "the boost of {total} was over after {total} ticks more"
    );
    assert!(
        twins.use_in_a(),
        "the boost of {total} was still on after {} ticks more",
        total + 1
    );
}

/// R4: `tickBoost` runs only from `tickRidden`, while a player controls the pig, so a
/// boost pauses while the stick is put away and goes on when it is taken back.
#[test]
fn a_boost_counts_only_while_the_pig_is_driven() {
    let mut twins = Twins::driving();
    twins.both(|scene| {
        use_stick(scene, 0);
    });
    let total = u32::try_from(boost_shown(&the_pig(&twins.a))).expect("positive");
    // The use's tick was driven; ten more are, then the select's own tick and 59 more
    // are not, and the select back's own tick is again.
    twins.idle(10);
    twins.both(|scene| {
        scene.select(EMPTY);
    });
    twins.idle(59);
    twins.both(|scene| {
        scene.select(STICK);
    });
    let driven = 1 + 10 + 1;
    twins.idle(total - driven + 1);
    assert!(
        !twins.use_in_a(),
        "the boost of {total} ran on while the stick was put away"
    );
    assert!(twins.use_in_a(), "the boost of {total} never ended");
}

/// R4, `processDurabilityChange`: a creative player pays nothing for a boost; the stick
/// stays in the hand as it was, and nothing is sent of it.
#[test]
fn a_creative_players_stick_is_kept_whole_by_a_boost() {
    let (mut scene, _) = driving(&[]);
    let output = use_stick(&mut scene, 0);
    assert!(boost_shown(&the_pig(&scene)) > 0, "boosted");
    assert_eq!(
        scene.holds(usize::from(STICK)),
        Some(stack(items::CARROT_ON_A_STICK, 1))
    );
    assert!(
        inventory_slots(&output, scene.actors[0].id).is_empty(),
        "nothing sent of the stick"
    );
}

// ------------------------------------------------------------------------------------
// 5. The vehicle move's checks for a pig as for a boat (R3; n5-brief 2.3).

/// n5-brief 2.3 step 3.2: a move more than ten blocks from where the pig was is refused,
/// the client sent the pig's own place.
#[test]
fn a_move_too_quick_is_refused_with_the_pigs_own_place() {
    let (mut scene, at) = driving(&[]);
    let output = scene.input(drive([at[0], at[1], at[2] + 10.5], true));
    let sent = corrections(&output, &scene, 0);
    assert_eq!(sent.len(), 1, "one correction: {sent:?}");
    assert!(near(sent[0], at, 1e-12), "sent {:?}", sent[0]);
    assert_eq!(bits(the_pig(&scene).position), bits(at));
}

/// n5-brief 2.3 steps 3.5 and 3.6: a move into a wall, whose collided end is more than
/// a quarter block short of the client's, is refused and the pig put back.
#[test]
fn a_move_into_a_wall_is_refused() {
    let wall: Vec<_> = (0..6)
        .flat_map(|x| {
            [
                (bp(x, -60, 3), blocks::STONE),
                (bp(x, -59, 3), blocks::STONE),
            ]
        })
        .collect();
    let (mut scene, at) = driving(&wall);
    let output = scene.input(drive([at[0], at[1], 3.4], true));
    let sent = corrections(&output, &scene, 0);
    assert_eq!(sent.len(), 1, "one correction: {sent:?}");
    assert!(near(sent[0], at, 1e-12), "sent {:?}", sent[0]);
    assert_eq!(bits(the_pig(&scene).position), bits(at));
}

/// R3 and n5-brief 2.2 step 7: a driver whose client holds the pig in the air is put out
/// after `ceil(80 · max(0.08 / 0.08, 1))` = 80 ticks, the boat's 160 halved by the pig's
/// gravity of 0.08. The first move up starts from the ground, so the second leaves it
/// floating; one tick either way is left to where in the tick the region counts, as
/// N5's test of the boat does.
#[test]
fn a_rider_who_holds_the_pig_in_the_air_is_put_out_after_eighty_ticks() {
    let (mut scene, at) = driving(&[]);
    let up = [at[0], at[1] + 2.0, at[2]];
    assert!(corrections(&scene.input(drive(up, false)), &scene, 0).is_empty());
    assert!(corrections(&scene.input(drive(up, false)), &scene, 0).is_empty());
    let mut put_out = None;
    for tick in 1..=200 {
        if kicked(&scene.idle(), &scene) {
            put_out = Some(tick);
            break;
        }
    }
    let after = put_out.expect("put out");
    assert!(
        (PIG_FLOATING - 1..=PIG_FLOATING + 1).contains(&after),
        "put out {after} ticks after the second move"
    );
}

/// n5-brief 2.2 step 7: a move on the ground clears the count.
#[test]
fn a_rider_who_brings_the_pig_down_in_time_is_not_put_out() {
    let (mut scene, at) = driving(&[]);
    let up = [at[0], at[1] + 2.0, at[2]];
    scene.input(drive(up, false));
    scene.input(drive(up, false));
    for _ in 0..60 {
        assert!(!kicked(&scene.idle(), &scene));
    }
    scene.input(drive(at, true));
    scene.input(drive(at, true));
    for _ in 0..100 {
        assert!(!kicked(&scene.idle(), &scene));
    }
}

// ------------------------------------------------------------------------------------
// 6. Falls (R3: `doCheckFallDamage`, `Entity.checkFallDamage`,
// `LivingEntity.causeFallDamage`).

/// The rider takes the pig straight up `height` blocks a block a move, then down again a
/// block a move, the last landing on the floor: the pig's records after the climb and
/// after the landing.
fn up_and_down(scene: &mut Scene, at: [f64; 3], height: u32) -> (Entity, Entity) {
    for step in 1..=height {
        let output = scene.input(drive([at[0], at[1] + f64::from(step), at[2]], false));
        assert!(corrections(&output, scene, 0).is_empty(), "up {step}");
    }
    let top = the_pig(scene);
    for step in (0..height).rev() {
        let output = scene.input(drive([at[0], at[1] + f64::from(step), at[2]], step == 0));
        assert!(corrections(&output, scene, 0).is_empty(), "down to {step}");
    }
    (top, the_pig(scene))
}

/// `Entity.checkFallDamage` (n7 Entity.txt 3990–4050): `if (!isInWater() && y < 0)
/// fallDistance -= (float) y`, *then* on the ground `fallOn` → `causeFallDamage`, then
/// the reset; so the landing move's own drop counts. `LivingEntity.calculateFallDamage`
/// (4600–4635): `floor((distance + 1e-6 − safe fall 3) · 1 · multiplier 1)`. A fall of 4
/// is one point; moving up gathers no fall. (The move's `dy` is the packet's position
/// less the pig's before it, SGPLI 661–732; `Entity.move` skips its own fall check for a
/// pig the server does not simulate, Entity.txt 1491–, offset 466.)
#[test]
#[ignore = "finding: the landing move's own drop is not counted, so a fall of 4 does no damage where Entity.checkFallDamage adds it before landing"]
fn a_driven_pig_brought_down_four_blocks_takes_one_point() {
    let (mut scene, at) = driving(&[]);
    let (top, landed) = up_and_down(&mut scene, at, 4);
    assert_eq!(top.fall_distance, 0.0, "nothing gathered while moving up");
    assert_eq!(health(&top), PIG_HEALTH);
    assert_eq!(health(&landed), PIG_HEALTH - 1.0, "floor(4 + 1e-6 - 3) = 1");
    assert_eq!(landed.fall_distance, 0.0, "reset by the landing");
    assert_eq!(
        rides(&scene, 0),
        Some(key(PIG)),
        "its creative rider stays on"
    );
}

/// The safe fall of 3: `floor(3 + 1e-6 − 3)` = 0.
#[test]
fn a_driven_pig_brought_down_three_blocks_takes_nothing() {
    let (mut scene, at) = driving(&[]);
    let (_, landed) = up_and_down(&mut scene, at, 3);
    assert_eq!(health(&landed), PIG_HEALTH);
    assert_eq!(landed.fall_distance, 0.0);
}

/// A fall of 8 is five points, the landing move's block among the eight.
#[test]
#[ignore = "finding: the landing move's own drop is not counted, so a fall of 8 does 4 points where Entity.checkFallDamage makes it 5"]
fn a_driven_pig_brought_down_eight_blocks_takes_five_points() {
    let (mut scene, at) = driving(&[]);
    let (_, landed) = up_and_down(&mut scene, at, 8);
    assert_eq!(health(&landed), PIG_HEALTH - 5.0, "floor(8 + 1e-6 - 3) = 5");
}

/// Moving up gathers no fall: a pig driven up a stair of blocks, a block a move on the
/// ground (its step raised to 1 while driven, R3), takes nothing.
#[test]
fn a_driven_pig_driven_up_a_stair_takes_nothing() {
    let stair: Vec<_> = (0..4)
        .flat_map(|step: i32| (0..=step).map(move |y| (bp(3 + step, -60 + y, 1), blocks::STONE)))
        .collect();
    let (mut scene, at) = driving(&stair);
    for step in 0..4 {
        let to = [3.5 + f64::from(step), FLOOR + f64::from(step + 1), at[2]];
        let output = scene.input(drive(to, true));
        assert!(corrections(&output, &scene, 0).is_empty(), "step {step}");
        let pig = the_pig(&scene);
        assert_eq!(pig.fall_distance, 0.0);
        assert_eq!(health(&pig), PIG_HEALTH);
    }
}

// ------------------------------------------------------------------------------------
// 7. Getting off (R5).

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

/// Player 1 on the pig, holding the stick and facing yaw 0, so that the pig faces south
/// (`Direction.fromYRot(0)`); the block the pig stands in.
fn facing_south(more: &[(BlockPos, clustine_data::BlockState)]) -> (Scene, [f64; 3]) {
    let (mut scene, _) = driving(more);
    scene.input(turn(0.0, 0.0));
    scene.idle();
    let pig = the_pig(&scene);
    assert_eq!(pig.yaw, 0.0);
    let block = pig.position.map(f64::floor);
    assert_eq!(
        block,
        [2.0, FLOOR, 1.0],
        "the pig is still in its block: {:?}",
        pig.position
    );
    (scene, pig.position)
}

/// R5, `Animal.getDismountLocationForPassenger`: the offsets of the pig's facing, the
/// first its clockwise side (south's is west), at the pig's own block height, on the
/// floor there.
#[test]
fn a_rider_who_sneaks_gets_off_on_the_pigs_clockwise_side() {
    let (mut scene, _) = facing_south(&[]);
    let at = sneak_off(&mut scene);
    assert!(near(at, [1.5, FLOOR, 1.5], 1e-9), "off at {at:?}");
    assert_eq!(rides(&scene, 0), None);
    assert!(passengers(&scene).is_empty());
}

/// R5: a place whose floor is not valid (a full block there: floor height 1) is passed
/// over; the next offset is the other side.
#[test]
fn a_rider_whose_pigs_clockwise_side_is_walled_gets_off_on_its_other_side() {
    let (mut scene, _) = facing_south(&[(bp(1, -60, 1), blocks::STONE)]);
    let at = sneak_off(&mut scene);
    assert!(near(at, [3.5, FLOOR, 1.5], 1e-9), "off at {at:?}");
}

/// R5: the third offset is back and clockwise (north-west for a pig facing south).
#[test]
fn a_rider_whose_pigs_sides_are_walled_gets_off_behind_it_on_the_clockwise_side() {
    let (mut scene, _) = facing_south(&[
        (bp(1, -60, 1), blocks::STONE),
        (bp(3, -60, 1), blocks::STONE),
    ]);
    let at = sneak_off(&mut scene);
    assert!(near(at, [1.5, FLOOR, 0.5], 1e-9), "off at {at:?}");
}

/// R5: where none of the eight fits, `Entity`'s: on top of the pig, `(x, box.maxY, z)`.
#[test]
fn a_rider_walled_in_on_every_side_gets_off_on_top_of_the_pig() {
    let mut walls = Vec::new();
    for x in 1..=3 {
        for z in 0..=2 {
            if (x, z) != (2, 1) {
                walls.push((bp(x, -60, z), blocks::STONE));
            }
        }
    }
    let (mut scene, pig) = facing_south(&walls);
    let at = sneak_off(&mut scene);
    assert!(
        near(at, [pig[0], pig[1] + f64::from(PIG_HEIGHT), pig[2]], 1e-12),
        "off at {at:?}, the pig at {pig:?}"
    );
}

/// R5: the rider's `baseTick`, their eyes in water and no bubble column there, the pig
/// in `#dismounts_underwater`: `stopRiding()`. Water three deep, which nothing makes
/// flow; the rider's eyes are 1.889 above the pig's feet. The dismount (LivingEntity.txt
/// `baseTick` offsets 364–389) is reached whether or not the air branch ran, so a
/// creative rider's invulnerability does not keep them on.
#[test]
#[ignore = "finding: a rider whose eyes are under water stays on the pig, where LivingEntity.baseTick calls stopRiding for a vehicle in #dismounts_underwater"]
fn a_rider_whose_eyes_go_under_water_is_put_off() {
    let water = state("minecraft:water[level=0]");
    let mut pool = Vec::new();
    for x in 5..=8 {
        for z in 0..=3 {
            for y in -60..=-58 {
                pool.push((bp(x, y, z), water));
            }
        }
    }
    let (mut scene, at) = driving(&pool);
    for x in [3.4, 4.3] {
        let output = scene.input(drive([x, at[1], at[2]], true));
        assert!(corrections(&output, &scene, 0).is_empty());
    }
    scene.idle();
    assert_eq!(rides(&scene, 0), Some(key(PIG)), "still on, on land");
    for x in [5.2, 6.1, 7.0] {
        let output = scene.input(drive([x, at[1], at[2]], false));
        assert!(corrections(&output, &scene, 0).is_empty(), "into the water");
    }
    assert_eq!(
        scene.block(bp(7, -59, 1)),
        water,
        "the rider's eyes are in water"
    );
    let mut seen = Vec::new();
    let off = scene.idle_until(3, &mut seen, |scene, _| rides(scene, 0).is_none());
    assert!(off, "the rider is still on the pig in the water");
}

/// R5 and plan 10.6: nothing in `hurtServer` or `die` touches passengers; the rider stays
/// on the dying pig until `tickDeath` removes it at a death time of 20, which puts them
/// off. A fall of 14 is 11 points, more than a pig's 10.
///
/// The plan (10.6) and R5 say the dying pig's moves are still its rider's. The bytecode
/// says not: `Pig.getControllingPassenger` asks `isSaddled()`, which is
/// `hasValidEquippableItemForSlot(SADDLE)` (Mob.txt 2080–2110) →
/// `isEquippableInSlot` → `canUseSlot(SADDLE)` (LivingEntity.txt 10755–10792), and
/// `Pig.canUseSlot(SADDLE)` is `isAlive() && !isBaby()` (Pig 360–378, R1). A dead pig
/// is not saddled, so nobody controls it, and the rider's move is a passenger's: the pig
/// stays where it is. The bytecode is tested here.
#[test]
fn a_rider_stays_on_a_pig_that_dies_until_it_is_removed() {
    let (mut scene, at) = driving(&[]);
    let (_, landed) = up_and_down(&mut scene, at, 14);
    assert!(health(&landed) <= 0.0, "dead: {}", health(&landed));
    scene.input(drive([at[0] + 0.1, at[1], at[2]], true));
    assert_eq!(
        bits(the_pig(&scene).position),
        bits(at),
        "a dead pig is not saddled, so its rider does not control it"
    );
    for tick in 2..=18 {
        scene.idle();
        assert_eq!(rides(&scene, 0), Some(key(PIG)), "still on at tick {tick}");
    }
    let mut seen = Vec::new();
    let off = scene.idle_until(5, &mut seen, |scene, _| {
        rides(scene, 0).is_none() && pig_of(scene, PIG).is_none()
    });
    assert!(off, "the pig is removed and the rider put off");
}

// ------------------------------------------------------------------------------------
// 8. Across a pinned line (plan 5 and 10.2; N5's crossing of a unit).

/// Where player 1 stands in the west, and the pig beside them, short of the line at
/// block x = 16.
const WEST_FEET: Vec3 = Vec3::new(14.5, FLOOR, 8.5);
const PIG_WEST: [f64; 3] = [15.5, FLOOR, 8.5];
/// A driven move that takes the pig over the line, into chunk (1, 0).
const ACROSS: [f64; 3] = [16.3, FLOOR, 8.5];
const BEYOND: ChunkPos = ChunkPos::new(1, 0);

/// The west with player 1 on the pig, holding the stick, among `more`; and the east with
/// nobody in it, whose copies of the west have no pig.
fn line(more: &[(BlockPos, clustine_data::BlockState)]) -> (Scene, Scene) {
    let pig = set(&pigs().saddled, PIG, PIG_WEST, -90.0);
    let world = World::of(more).with_entities(&[pig]);
    let mut west = Scene::new(Layout::side(true), &world, hotbar(), WEST_FEET);
    west.input(used(key(PIG)));
    assert_eq!(rides(&west, 0), Some(key(PIG)));
    west.select(STICK);
    west.idle();
    let east_layout = Layout::side(false);
    let east = Scene {
        region: region_of(&east_layout, &World::of(more), hotbar()),
        layout: east_layout,
        inventory: hotbar(),
        actors: Vec::new(),
    };
    (west, east)
}

/// What the west sent when the pig crossed: the unit, filed by its chunk, and the
/// rider's hand-over.
struct Sent {
    chunk: ChunkPos,
    unit: Entity,
    transfer: clustine_sim::PlayerTransfer,
}

fn sent(west: &Scene, output: &TickOutput) -> Option<Sent> {
    let (chunk, unit) = output
        .arrivals
        .iter()
        .find(|(_, entity)| entity.key == key(PIG))
        .cloned()?;
    let mut handed = departures(output);
    assert_eq!(handed.len(), 1, "the rider is handed over with the pig");
    let (who, transfer, to) = handed.remove(0);
    assert_eq!(who, west.actors[0].id);
    assert_eq!(to, OTHER);
    Some(Sent {
        chunk,
        unit,
        transfer,
    })
}

fn arrival(chunk: ChunkPos, unit: &Entity) -> Arrival {
    Arrival {
        id: EffectId {
            sequence: 40,
            index: 0,
        },
        chunk,
        entity: unit.clone(),
        due: Due::FromTaking,
    }
}

/// The east takes the unit, then the rider: player 1 is its actor from then on.
fn taken(east: &mut Scene, west: &Scene, sent: &Sent) -> TickOutput {
    east.region.tick(&TickInputs {
        arrivals: vec![arrival(sent.chunk, &sent.unit)],
        ..TickInputs::default()
    });
    let output = east.region.tick(&TickInputs {
        player_changes: vec![clustine_sim::PlayerChange::Arrive(
            EDGE,
            west.actors[0].id,
            sent.transfer.clone(),
        )],
        ..TickInputs::default()
    });
    east.actors.push(Actor {
        id: west.actors[0].id,
        entity: sent.transfer.entity_id,
        input: sent.transfer.last_input,
        sequence: west.actors[0].sequence,
    });
    output
}

/// Plan 5 and N5's crossing: the driven pig's accepted move into the east's land sends
/// the unit, the rider's seat kept for them, and hands the rider over in the same tick;
/// the east seats them on the pig with no teleport, and they drive it on there.
#[test]
fn a_driven_pig_and_its_rider_cross_a_line_together_and_the_rider_drives_it_beyond() {
    let (mut west, mut east) = line(&[]);
    let output = west.input(drive(ACROSS, true));
    let crossing = sent(&west, &output).expect("the pig was sent");
    assert_eq!(crossing.chunk, BEYOND);
    assert_eq!(bits(crossing.unit.position), bits(ACROSS));
    let seats = &crossing.unit.riding.passengers;
    assert_eq!(seats.len(), 1);
    assert!(seats[0].player && seats[0].pending.is_some(), "{seats:?}");
    assert!(pig_of(&west, PIG).is_none(), "the pig left the west");
    assert!(west.region.player(west.actors[0].id).is_none());

    let seating = taken(&mut east, &west, &crossing);
    assert!(
        rides_told(&seating, &east, 0).is_empty(),
        "no teleport on: {:?}",
        seating.player_events
    );
    let riding = east
        .region
        .player_state(east.actors[0].id)
        .expect("player 1 came")
        .riding
        .expect("riding");
    assert_eq!(riding.vehicle, key(PIG));
    assert_eq!(riding.pending, None, "seated");
    let pig = the_pig(&east);
    assert_eq!(pig.riding.passengers.len(), 1);
    assert_eq!(pig.riding.passengers[0].pending, None);

    east.idle();
    assert!(near(
        feet(&east, 0),
        [
            ACROSS[0],
            ACROSS[1] + PIG_SEAT - PLAYER_ATTACHMENT,
            ACROSS[2]
        ],
        1e-9
    ));
    let on = [17.0, FLOOR, 8.5];
    let output = east.input(drive(on, true));
    assert!(corrections(&output, &east, 0).is_empty(), "driven beyond");
    assert_eq!(bits(the_pig(&east).position), bits(on));
}

/// Plan 10.2: a pig whose first seat is a pending player's is kept still, no AI step and
/// no travel, so that the rider's kept moves are judged from where it stood. Sent with
/// motion, it does not move by it.
#[test]
fn a_pig_whose_riders_seat_is_pending_stands_still() {
    let (mut west, mut east) = line(&[]);
    let output = west.input(drive(ACROSS, true));
    let crossing = sent(&west, &output).expect("the pig was sent");
    let mut moving = crossing.unit.clone();
    moving.motion = [0.3, 0.0, 0.1];
    // Beside it, the same pig with nobody's seat and the same motion, which moves by it.
    let mut free = set(&moving, OTHER_PIG, [ACROSS[0] + 2.0, FLOOR, ACROSS[2]], 0.0);
    free.motion = moving.motion;
    let mut second = arrival(crossing.chunk, &free);
    second.id.index = 1;
    east.region.tick(&TickInputs {
        arrivals: vec![arrival(crossing.chunk, &moving), second],
        ..TickInputs::default()
    });
    for tick in 0..40 {
        east.idle();
        let pig = the_pig(&east);
        assert_eq!(bits(pig.position), bits(ACROSS), "moved at tick {tick}");
    }
    let other = pig_of(&east, OTHER_PIG).expect("the free pig is there");
    assert!(
        horizontal(other.position, free.position) > 0.1,
        "the free pig moved by its motion: {:?}",
        other.position
    );
}

/// Plan 5: a pig ridden without the stick is its own, and wanders over the line with its
/// rider on it; beyond, the rider without the stick does not drive it, and drives it
/// again once they take the stick back. A corridor along z = 8, walled behind the pig,
/// keeps its strolls on the line's way.
#[test]
fn a_pig_with_the_stick_put_away_wanders_over_a_line_with_its_rider() {
    let mut corridor = Vec::new();
    for y in [-60, -59] {
        for x in 13..=24 {
            corridor.push((bp(x, y, 7), blocks::STONE));
            corridor.push((bp(x, y, 9), blocks::STONE));
        }
        corridor.push((bp(13, y, 8), blocks::STONE));
    }
    let (mut west, mut east) = line(&corridor);
    let output = west.input(drive([15.9, FLOOR, 8.5], true));
    assert!(corrections(&output, &west, 0).is_empty());
    west.select(EMPTY);
    let mut seen = Vec::new();
    let crossed = west.idle_until(6_000, &mut seen, |_, seen| {
        seen.last()
            .is_some_and(|output| output.arrivals.iter().any(|(_, e)| e.key == key(PIG)))
    });
    assert!(crossed, "the pig never wandered over the line");
    let output = seen.last().expect("the crossing tick");
    let crossing = sent(&west, output).expect("sent with its rider");
    taken(&mut east, &west, &crossing);
    assert_eq!(rides(&east, 0), Some(key(PIG)));

    let at = the_pig(&east).position;
    let output = east.input(drive([at[0] + 0.5, at[1], at[2]], true));
    assert_eq!(
        corrections(&output, &east, 0).len(),
        1,
        "without the stick the rider does not drive it"
    );
    east.select(STICK);
    let at = the_pig(&east).position;
    let to = [at[0] + 0.5, at[1], at[2]];
    let output = east.input(drive(to, true));
    assert!(corrections(&output, &east, 0).is_empty(), "driven again");
    assert_eq!(bits(the_pig(&east).position), bits(to));
}

// ------------------------------------------------------------------------------------
// 9. A tick is a function of its inputs.

/// Two regions made alike and given the same inputs (a driven pig turned, boosted,
/// driven, left to its AI with the stick put away, driven again) report the same in
/// every tick, the pig's boost draw and its AI's draws among it.
#[test]
fn two_regions_given_the_same_inputs_commit_the_same_with_a_driven_boosted_pig() {
    let mut twins = Twins::driving();
    let mut script: Vec<Vec<PlayerInput>> = vec![vec![turn(45.0, 20.0)]];
    let rotation = (45.0, 20.0);
    script.push(vec![PlayerInput::UseItem {
        rotation,
        sneaking: false,
        sequence: 1,
    }]);
    for step in 1..=10 {
        script.push(vec![drive(
            [PIG_AT[0] + 0.3 * f64::from(step), FLOOR, PIG_AT[2]],
            true,
        )]);
    }
    script.push(vec![PlayerInput::SelectSlot { slot: EMPTY }]);
    script.extend((0..200).map(|_| Vec::new()));
    script.push(vec![PlayerInput::SelectSlot { slot: STICK }]);
    script.extend((0..50).map(|_| Vec::new()));
    for (tick, inputs) in script.into_iter().enumerate() {
        let a = twins.a.inputs_of(0, inputs.clone());
        let b = twins.b.inputs_of(0, inputs);
        assert!(a == b, "tick {tick} differs:\n{a:?}\n{b:?}");
    }
    twins.assert_alike("at the end");
    assert!(boost_shown(&the_pig(&twins.a)) > 0, "boosted");
}
