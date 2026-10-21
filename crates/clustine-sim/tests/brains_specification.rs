//! Tests of the animals with brains of step N7 (part N7j), written from the specification
//! and the public API alone, by someone who wrote none of them: the porting brief
//! `docs/groundwork/n7-brief.md` (section 7, 7.1 to 7.13, with 9.7, 10.7 and 11.7), the
//! plan `docs/groundwork/n7j-plan.md` (sections 1, 2, 3 and 8) and the game's bytecode
//! (`reference/n7-javap/`: `Brain`, `Behavior`, `MemorySlot`, the shared behaviours of
//! `ai.behavior`, and each animal's class and `*Ai`). Where the brief and the bytecode
//! differ, the bytecode is followed and the test says so.
//!
//! A region is driven as a player drives it: spawn eggs used on the floor, items used on
//! the animals, hits, keys held, ticks; at a pinned line, two regions whose copies,
//! effects, arrivals and players' faces are handed from one to the other as the store
//! would. Nothing of a brain is shown to clients (brief 9.7.1), so what a brain keeps is
//! read from what it does: the poses, synced states, sounds and entity events of its
//! behaviours, where the animal goes, what it hurts.
//!
//! The machinery's order (brief 7.1.5, 7.1.6) is read through it too. A behaviour ticks
//! in the tick it starts: a croak shows its pose for its 60 ticks of `tick`, not 61. A
//! memory set outside the brain's tick counts from the mob's next forget pass, and one
//! set with expiry `n` is gone in the forget pass `n + 1` brain ticks after it was set
//! (`MemorySlot.tick`): an armadillo hit in tick T unrolls in tick T + 80. A countdown
//! is read at its reader's place: the armadillo's ball reads its danger memory's time to
//! expiry after the forget pass, and begins to unroll exactly 30 ticks before it is
//! gone. A behaviour outlives its activity: an armadillo tempted and then rolled up
//! keeps its temptation through the panic and walks on at once when it unrolls. The
//! draws themselves are not visible through the public API, nor is the order of two
//! activities' behaviours at one priority (none of these animals' providers lets it show
//! within a tick); what a start draws shows in the lengths it gives (a dig of 160 to 180
//! ticks, a happiness of 40 to 100, a croak of exactly 60 for its draw of `nextInt(1)`).
//!
//! Every player is in creative mode, as Clustine's players are: no goat rams one, no
//! axolotl or nautilus attacks one (brief 7.0.1 point 9).
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60) at the spawn, in the
//! badlands (biome 0) unless a test says otherwise.

mod n6_entities;
mod n6_scene;

use clustine_data::{BlockState, SYNCED_REGISTRIES, blocks, entity_types, items, sounds};
use clustine_sim::api::{ClickKind, Face, Keys, PlayerInput, RegionEvent, StayNote};
use clustine_sim::shown::{DataShown, Holder, Serialiser, index, mob_values};
use clustine_sim::{FaceChange, FaceSeq, TickInputs, TickOutput};
use clustine_world::{
    Arrival, BlockPos, ChunkPos, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData,
    EntityId, EntityUse, FiledEffect, ItemStack, Vec3,
};
use n6_scene::*;

// ------------------------------------------------------------------------------------
// What clients are shown (brief 9.7).

/// `Entity.DATA_POSE`.
const POSE: u8 = 6;
/// `Pose`'s ordinals (Pose.txt's static block).
const STANDING: i32 = 0;
const LONG_JUMPING: i32 = 6;
const CROAKING: i32 = 8;
const USING_TONGUE: i32 = 9;
const SITTING: i32 = 10;

/// The axolotl's `DATA_VARIANT` and `DATA_PLAYING_DEAD` (brief 9.7.2).
const AXOLOTL_VARIANT: u8 = 18;
const AXOLOTL_PLAYING_DEAD: u8 = 19;
/// The frog's `DATA_VARIANT_ID`.
const FROG_VARIANT: u8 = 18;
/// The goat's horns (brief 9.7.3).
const GOAT_SCREAMING: u8 = 18;
const GOAT_LEFT_HORN: u8 = 19;
const GOAT_RIGHT_HORN: u8 = 20;
/// The camel's `LAST_POSE_CHANGE_TICK`.
const CAMEL_POSE_TICK: u8 = 20;
/// The armadillo's `ARMADILLO_STATE` and its ids.
const ARMADILLO_STATE: u8 = 18;
const IDLE: i32 = 0;
const ROLLING: i32 = 1;
const SCARED: i32 = 2;
const UNROLLING: i32 = 3;
/// The sniffer's `DATA_STATE` and its ids (brief 7.9.2).
const SNIFFER_STATE: u8 = 18;
const FEELING_HAPPY: i32 = 1;
const SNIFFING: i32 = 3;
const SEARCHING: i32 = 4;
const DIGGING: i32 = 5;
const RISING: i32 = 6;
/// The copper golem's `COPPER_GOLEM_STATE` (brief 9.7.5).
const GOLEM_STATE: u8 = 17;
const GETTING_ITEM: i32 = 1;
const DROPPING_ITEM: i32 = 3;

/// Entity events: the goat's head lowered and raised, the sniffer's dig, the
/// armadillo's peek (brief 9.7.1, 9.7.3, 9.7.4), love's hearts.
const HEAD_DOWN: u8 = 58;
const HEAD_UP: u8 = 59;
const DIG: u8 = 63;
const PEEK: u8 = 64;
const HEARTS: u8 = 18;

/// The id of the biome `name` in the registry the server sends.
fn biome(name: &str) -> u16 {
    let biomes = SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == "minecraft:worldgen/biome")
        .expect("the biome registry");
    let at = biomes
        .entries
        .iter()
        .position(|entry| *entry == name)
        .unwrap_or_else(|| panic!("{name} is a biome"));
    u16::try_from(at).expect("few")
}

/// The id of `name` in the synced registry `registry`.
fn registry_id(registry: &str, name: &str) -> i32 {
    let found = SYNCED_REGISTRIES
        .iter()
        .find(|held| held.name == registry)
        .unwrap_or_else(|| panic!("{registry} is synced"));
    let at = found
        .entries
        .iter()
        .position(|entry| *entry == name)
        .unwrap_or_else(|| panic!("{name} is in {registry}"));
    i32::try_from(at).expect("few")
}

// ------------------------------------------------------------------------------------
// The places.

/// The floor's block whose top an egg is used on to make a mob at (x + 0.5, -60, z + 0.5).
fn floor_at(x: i32, z: i32) -> BlockPos {
    bp(x, -61, z)
}

/// Stone in every block of the box from `from` to `to`, both included.
fn stone_box(from: BlockPos, to: BlockPos) -> Vec<(BlockPos, BlockState)> {
    filled(from, to, blocks::STONE)
}

fn filled(from: BlockPos, to: BlockPos, state: BlockState) -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for x in from.x..=to.x {
        for y in from.y..=to.y {
            for z in from.z..=to.z {
                placed.push((bp(x, y, z), state));
            }
        }
    }
    placed
}

/// A ring of stone `high` blocks high on the floor around the blocks from (x0, z0) to
/// (x1, z1), which stay as they are.
fn ring(x0: i32, z0: i32, x1: i32, z1: i32, high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for y in -60..-60 + high {
        for x in x0 - 1..=x1 + 1 {
            for z in z0 - 1..=z1 + 1 {
                let inside = (x0..=x1).contains(&x) && (z0..=z1).contains(&z);
                if !inside {
                    placed.push((bp(x, y, z), blocks::STONE));
                }
            }
        }
    }
    placed
}

/// A pool of water inside x and z from the floor up to `top`, with stone under it at
/// y = -61 and a stone rim as high as the water.
fn pool(x: (i32, i32), z: (i32, i32), top: i32) -> Vec<(BlockPos, BlockState)> {
    let mut placed = ring(x.0, z.0, x.1, z.1, top + 61);
    placed.extend(filled(bp(x.0, -60, z.0), bp(x.1, top, z.1), blocks::WATER));
    placed
}

/// A scene of one region with `placed` in its world, in the biome `biome`, and player 1
/// at the spawn holding `slots` in the hotbar, one of each.
fn scene_in(placed: &[(BlockPos, BlockState)], biome: u16, slots: &[i32]) -> Scene {
    let stacks: Vec<(usize, ItemStack)> = slots
        .iter()
        .enumerate()
        .map(|(slot, item)| (slot, stack(*item, 1)))
        .collect();
    let mut world = World::of(placed);
    world.biome = biome;
    Scene::alone(&world, inventory(&stacks), SPAWN)
}

fn scene_with(placed: &[(BlockPos, BlockState)], slots: &[i32]) -> Scene {
    scene_in(placed, 0, slots)
}

// ------------------------------------------------------------------------------------
// Driving a scene.

/// Player 1 selects hotbar `slot` and uses what it holds on the top of `at`: the one
/// mob it made, if any.
fn hatch(scene: &mut Scene, slot: u8, at: BlockPos) -> Option<EntityId> {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(slot);
    scene.click(at, Face::Top);
    let made: Vec<EntityId> = scene
        .all()
        .iter()
        .filter(|entity| {
            !before.contains(&entity.key)
                && i32::from(entity.kind) != entity_types::ITEM
                && i32::from(entity.kind) != entity_types::EXPERIENCE_ORB
        })
        .map(|entity| entity.key)
        .collect();
    assert!(made.len() <= 1, "one egg makes one mob: {made:?}");
    made.first().copied()
}

fn used(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.2, 0.0],
        sneaking: false,
    }
}

/// Player 1 selects `slot` and, in the same tick, uses each of `keys` with it.
fn use_on(scene: &mut Scene, slot: u8, keys: &[EntityId]) -> TickOutput {
    let mut inputs = vec![PlayerInput::SelectSlot { slot }];
    inputs.extend(keys.iter().map(|key| used(*key)));
    scene.inputs_of(0, inputs)
}

fn hit(scene: &mut Scene, key: EntityId) -> TickOutput {
    scene.input(PlayerInput::Attack { entity: key })
}

fn keys(sprint: bool) -> PlayerInput {
    PlayerInput::Keys(Keys {
        sprint,
        ..Keys::default()
    })
}

fn moved_to(feet: Vec3) -> PlayerInput {
    PlayerInput::Move {
        position: Some(feet),
        rotation: None,
        on_ground: true,
    }
}

// ------------------------------------------------------------------------------------
// Reading a scene.

fn entity_of(scene: &Scene, key: EntityId) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key)
}

fn the_entity(scene: &Scene, key: EntityId) -> Entity {
    entity_of(scene, key).unwrap_or_else(|| panic!("{key:?} is in the scene"))
}

fn shown_of(entity: &Entity, at: u8) -> Option<DataShown> {
    mob_values(entity.kind, &entity.data)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, _)| value)
}

fn shown_at(scene: &Scene, key: EntityId, at: u8) -> Option<DataShown> {
    shown_of(&the_entity(scene, key), at)
}

fn flag_at(scene: &Scene, key: EntityId, at: u8) -> bool {
    match shown_at(scene, key, at) {
        Some(DataShown::Boolean(value)) => value,
        other => panic!("index {at} is a boolean: {other:?}"),
    }
}

/// The mob's pose, standing where its type shows none.
fn pose_of(entity: &Entity) -> i32 {
    match shown_of(entity, POSE) {
        Some(DataShown::Pose(pose)) => pose,
        None => STANDING,
        other => panic!("a pose: {other:?}"),
    }
}

fn pose(scene: &Scene, key: EntityId) -> i32 {
    pose_of(&the_entity(scene, key))
}

/// A value of one of the game's own enumerations the mob shows at `at`.
fn state_of(scene: &Scene, key: EntityId, at: u8, of: Serialiser) -> i32 {
    match shown_at(scene, key, at) {
        Some(DataShown::Serialised(serialiser, value)) if serialiser == of => value,
        other => panic!("index {at} is a {of:?}: {other:?}"),
    }
}

fn armadillo_state(scene: &Scene, key: EntityId) -> i32 {
    state_of(scene, key, ARMADILLO_STATE, Serialiser::ArmadilloState)
}

fn sniffer_state(scene: &Scene, key: EntityId) -> i32 {
    state_of(scene, key, SNIFFER_STATE, Serialiser::SnifferState)
}

fn health(scene: &Scene, key: EntityId) -> f32 {
    match shown_at(scene, key, index::HEALTH) {
        Some(DataShown::Float(health)) => health,
        other => panic!("a mob shows its health: {other:?}"),
    }
}

/// How many times `key` was shown the entity event `event` in `outputs`.
fn events_of(outputs: &[TickOutput], key: EntityId, event: u8) -> usize {
    outputs
        .iter()
        .flat_map(|output| &output.events)
        .filter(|seen| {
            matches!(seen, RegionEvent::EntityEvent { entity, event: shown, .. }
                if *entity == key && *shown == event)
        })
        .count()
}

fn event_in(output: &TickOutput, key: EntityId, event: u8) -> bool {
    events_of(std::slice::from_ref(output), key, event) > 0
}

/// The sounds of id `sound` the tick played, at a place or at an entity, each with its
/// volume.
fn sounds_in(output: &TickOutput, sound: i32) -> Vec<f32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: id, volume, ..
            }
            | RegionEvent::EntitySound {
                sound: id, volume, ..
            } if *id == sound => Some(*volume),
            _ => None,
        })
        .collect()
}

fn played(output: &TickOutput, sound: i32) -> usize {
    sounds_in(output, sound).len()
}

/// The causes of the damage events of `key` in the tick.
fn hurts(output: &TickOutput, key: EntityId) -> Vec<Option<EntityId>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::DamageEvent { entity, cause, .. } if *entity == key => Some(*cause),
            _ => None,
        })
        .collect()
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn feet(entity: &Entity) -> [f64; 3] {
    entity.position
}

fn block_of(position: [f64; 3]) -> BlockPos {
    bp(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )
}

fn others_of(scene: &Scene, kind: i32, known: &[EntityId]) -> Vec<Entity> {
    scene
        .of_kind(kind)
        .into_iter()
        .filter(|entity| !known.contains(&entity.key))
        .collect()
}

/// How many of `item` player 1's inventory holds, in all its slots.
fn carried(scene: &Scene, item: i32) -> i32 {
    scene
        .region
        .player_state(scene.actors[0].id)
        .expect("player 1 is there")
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .map(|stack| stack.count)
        .sum()
}

fn player_feet(scene: &Scene) -> [f64; 3] {
    let feet = scene
        .region
        .player(scene.actors[0].id)
        .expect("there")
        .1
        .position;
    [feet.x, feet.y, feet.z]
}

/// The runs of consecutive samples that hold, each as (first index, length).
fn runs(samples: &[bool]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, held) in samples.iter().enumerate() {
        match (start, held) {
            (None, true) => start = Some(at),
            (Some(from), false) => {
                found.push((from, at - from));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        found.push((from, samples.len() - from));
    }
    found
}

// ------------------------------------------------------------------------------------
// 1. The machinery's order, read through the armadillo and the frog (brief 7.1.2, 7.1.5,
// 7.1.6, 7.8.3, 7.8.4, 7.5.6; plan 2.2 and section 8 item 6).

/// Player 1 stands two blocks west of the mob `key`.
fn beside(scene: &mut Scene, key: EntityId) -> TickOutput {
    let at = feet(&the_entity(scene, key));
    scene.input(moved_to(Vec3::new(at[0] - 2.0, -60.0, at[2])))
}

/// Brief 7.8.5, 7.8.3 and 7.1.2, with the plan's section 8 item 6: a hit by a living
/// attacker rolls an idle armadillo up at once (`actuallyHurt`: `rollUp`, state 1,
/// `ARMADILLO_ROLL`) and sets `DANGER_DETECTED_RECENTLY` with expiry 80. The hit comes
/// before the armadillo's tick, so its forget pass that tick already counts the memory
/// down: its time to expiry is 79 − k in the k-th tick after the hit. The ball
/// (`ArmadilloBallUp`, started the tick after, when `PANIC` is active) turns it scared
/// once it has been rolling for more than 10 ticks (`inStateTicks`, counted after the
/// brain in `Armadillo.tick`): state 2 with `ARMADILLO_LAND` 11 ticks after the hit. It
/// reads the time to expiry after the forget pass: below 30, 50 ticks after the hit,
/// it begins to unroll (state 3, `ARMADILLO_UNROLL_START`). The memory is present for
/// the last time 79 ticks after the hit and gone in the forget pass 80 ticks after it,
/// when the core's rolling out unrolls it (state 0, `ARMADILLO_UNROLL_FINISH`). Danger
/// (time to expiry above 75) is gone by the time it is scared, so it never peeks. The
/// player steps 9 blocks away in the tick of the hit, out of the scare box (7, 2, 7), so
/// that the scare sensor never sees them as the one who last hurt it.
#[test]
fn an_armadillo_hit_unrolls_eighty_ticks_after_the_hit_by_the_games_countdown() {
    let mut scene = scene_with(&[], &[items::ARMADILLO_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(3, 0)).expect("an armadillo");
    scene.idle_for(5);
    beside(&mut scene, key);
    assert_eq!(armadillo_state(&scene, key), IDLE);
    let at = player_feet(&scene);
    let mut seen = vec![scene.inputs_of(
        0,
        vec![
            PlayerInput::Attack { entity: key },
            moved_to(Vec3::new(at[0] - 9.0, -60.0, at[2])),
        ],
    )];
    let mut states = vec![armadillo_state(&scene, key)];
    for _ in 0..90 {
        seen.push(scene.idle());
        states.push(armadillo_state(&scene, key));
    }
    let expected: Vec<i32> = (0..=90)
        .map(|k| match k {
            0..=10 => ROLLING,
            11..=49 => SCARED,
            50..=79 => UNROLLING,
            _ => IDLE,
        })
        .collect();
    assert_eq!(states, expected, "the states, tick by tick from the hit");
    let at_ticks = |sound| -> Vec<usize> {
        seen.iter()
            .enumerate()
            .filter(|(_, output)| played(output, sound) > 0)
            .map(|(tick, _)| tick)
            .collect()
    };
    assert_eq!(at_ticks(sounds::ENTITY_ARMADILLO_ROLL), vec![0]);
    assert_eq!(at_ticks(sounds::ENTITY_ARMADILLO_LAND), vec![11]);
    assert_eq!(at_ticks(sounds::ENTITY_ARMADILLO_UNROLL_START), vec![50]);
    assert_eq!(at_ticks(sounds::ENTITY_ARMADILLO_UNROLL_FINISH), vec![80]);
    assert_eq!(events_of(&seen, key, PEEK), 0, "no peek without danger");
}

/// Brief 7.8.4, 7.8.2 and 7.8.3: a player who walks about an armadillo does not scare it;
/// one who sprints within its box (7, 2, 7) does, as the scare sensor (every 5 ticks)
/// reads their sprinting: the danger memory with expiry 80, `PANIC` after that brain
/// tick, the ball's start the tick after (state 1, `ARMADILLO_ROLL`), state 2 with
/// `ARMADILLO_LAND` 11 ticks later. While they sprint there the memory is refreshed every
/// 5 ticks (its time to expiry stays above 75), so it stays scared and peeks (entity
/// event 64) every 150 to 450 ticks (`50 + nextIntBetweenInclusive(100, 400)`, set when
/// danger first comes and after each peek). Once they stop, the last detection was 1 to
/// 5 ticks before; 51 ticks after it the time to expiry falls below 30 (state 3,
/// `ARMADILLO_UNROLL_START`), and 81 ticks after it the memory is gone (state 0,
/// `ARMADILLO_UNROLL_FINISH`): exactly 30 ticks apart, 76 to 80 ticks after they stopped.
#[test]
fn a_sprinting_player_rolls_an_armadillo_up_and_a_walking_one_does_not() {
    let mut scene = scene_with(&[], &[items::ARMADILLO_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(3, 0)).expect("an armadillo");
    for tick in 0..100 {
        let at = feet(&the_entity(&scene, key));
        let side = if tick % 2 == 0 { 3.0 } else { 4.0 };
        scene.input(moved_to(Vec3::new(at[0] - side, -60.0, at[2])));
        assert_eq!(armadillo_state(&scene, key), IDLE, "walking scares nothing");
    }
    let at = feet(&the_entity(&scene, key));
    let stand = Vec3::new(at[0] - 4.0, -60.0, at[2]);
    scene.inputs_of(0, vec![moved_to(stand), keys(true)]);
    let mut rolled = armadillo_state(&scene, key) == ROLLING;
    for _ in 0..10 {
        if rolled {
            break;
        }
        scene.idle();
        rolled = armadillo_state(&scene, key) == ROLLING;
    }
    assert!(rolled, "rolled up within 10 ticks of the sprint");
    let mut seen = Vec::new();
    let mut states = Vec::new();
    for _ in 0..1000 {
        seen.push(scene.idle());
        states.push(armadillo_state(&scene, key));
    }
    // `states[k]` is k + 1 ticks after the roll.
    assert!(
        states[..10].iter().all(|state| *state == ROLLING),
        "{states:?}"
    );
    assert_eq!(states[10], SCARED, "scared 11 ticks after it rolled");
    assert_eq!(played(&seen[10], sounds::ENTITY_ARMADILLO_LAND), 1);
    assert!(
        states[10..].iter().all(|state| *state == SCARED),
        "scared while they sprint"
    );
    let peeks: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|(_, output)| event_in(output, key, PEEK))
        .map(|(tick, _)| tick + 1)
        .collect();
    assert!(peeks.len() >= 2, "it peeks: {peeks:?}");
    assert!(
        (150..=450).contains(&peeks[0]),
        "the first peek 150 to 450 ticks after the roll: {peeks:?}"
    );
    for pair in peeks.windows(2) {
        assert!(
            (150..=450).contains(&(pair[1] - pair[0])),
            "peeks 150 to 450 ticks apart: {peeks:?}"
        );
    }
    scene.input(keys(false));
    let mut unrolling = None;
    let mut idle = None;
    for tick in 1..=100 {
        let output = scene.idle();
        let state = armadillo_state(&scene, key);
        if state == UNROLLING && unrolling.is_none() {
            unrolling = Some(tick);
            assert_eq!(played(&output, sounds::ENTITY_ARMADILLO_UNROLL_START), 1);
        }
        if state == IDLE {
            idle = Some(tick);
            assert_eq!(played(&output, sounds::ENTITY_ARMADILLO_UNROLL_FINISH), 1);
            break;
        }
    }
    let (unrolling, idle) = (unrolling.expect("unrolling"), idle.expect("unrolled"));
    assert_eq!(idle - unrolling, 30, "unrolling for 30 ticks");
    assert!(
        (76..=80).contains(&idle),
        "unrolled {idle} ticks after the sprint stopped"
    );
}

/// Brief 7.0.3 point 11 and 7.1.1 finding 7: only starting is limited to the active
/// activities. An armadillo following a player who holds a spider eye (`FollowTemptation`
/// in `IDLE`'s `RunOne`) and then rolled up by them sprinting goes on running its
/// temptation through `PANIC`: it is not stopped, so it sets no temptation cooldown of
/// 100, and the armadillo's sink (which does not start while it is scared) walks it on
/// towards the player within a few ticks of it unrolling, 81 ticks after the scare. A
/// temptation stopped with its activity would set its cooldown and keep the armadillo
/// where it is for some 20 ticks more.
#[test]
fn an_armadillo_rolled_up_while_tempted_follows_on_at_once_when_it_unrolls() {
    let mut scene = scene_with(&[], &[items::SPIDER_EYE, items::ARMADILLO_SPAWN_EGG]);
    let key = hatch(&mut scene, 1, floor_at(3, 0)).expect("an armadillo");
    scene.select(0);
    scene.input(moved_to(Vec3::new(-2.5, -60.0, 0.5)));
    let mut tempted = false;
    for _ in 0..300 {
        scene.idle();
        if horizontal(feet(&the_entity(&scene, key)), player_feet(&scene)) < 3.0 {
            tempted = true;
            break;
        }
    }
    assert!(tempted, "the armadillo comes to the spider eye");
    let mut rolled = false;
    for _ in 0..12 {
        scene.input(keys(true));
        if armadillo_state(&scene, key) == ROLLING {
            rolled = true;
            break;
        }
    }
    assert!(rolled, "the sprint rolls it up");
    scene.input(keys(false));
    let at = feet(&the_entity(&scene, key));
    scene.input(moved_to(Vec3::new(at[0] - 6.0, -60.0, at[2])));
    let mut unrolled = false;
    for _ in 0..100 {
        scene.idle();
        if armadillo_state(&scene, key) == IDLE {
            unrolled = true;
            break;
        }
    }
    assert!(unrolled, "it unrolls");
    let before = horizontal(feet(&the_entity(&scene, key)), player_feet(&scene));
    assert!(before > 5.0, "the player is away: {before}");
    scene.idle_for(10);
    let after = horizontal(feet(&the_entity(&scene, key)), player_feet(&scene));
    assert!(
        after < before - 0.5,
        "within 10 ticks of unrolling it walks on: {before} to {after}"
    );
}

/// Brief 7.5.6 and 7.1.6: a croak (`Croak`, a `Behavior` with entry `WALK_TARGET` absent,
/// in `IDLE`'s `RunOne`) sets the pose `CROAKING` at its start and its counter to 0; it
/// ticks in the tick it starts and every tick after, the counter going up by one, while
/// the counter is below 60, and then stops, standing the frog up. So the pose shows for
/// exactly 60 ticks, the start's included; a port that let a behaviour first tick in the
/// tick after its start would show it for 61. The frog is in a roofed cell, where it can
/// neither stroll nor long-jump.
#[test]
fn a_frogs_croak_shows_its_pose_for_sixty_ticks_from_the_tick_it_starts() {
    let mut placed = ring(2, 0, 2, 0, 3);
    placed.extend(stone_box(bp(1, -58, -1), bp(3, -58, 1)));
    let mut scene = scene_with(&placed, &[items::FROG_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(2, 0)).expect("a frog");
    let mut samples = Vec::new();
    let croaking = |samples: &[i32]| -> Vec<(usize, usize)> {
        runs(
            &samples
                .iter()
                .map(|pose| *pose == CROAKING)
                .collect::<Vec<_>>(),
        )
    };
    for _ in 0..3000 {
        scene.idle();
        samples.push(pose(&scene, key));
        if croaking(&samples).len() >= 5 {
            break;
        }
    }
    // Whole croaks only: not one begun before the samples, nor one still going.
    let croaks: Vec<(usize, usize)> = croaking(&samples)
        .into_iter()
        .filter(|(from, length)| *from > 0 && from + length < samples.len())
        .collect();
    assert!(croaks.len() >= 3, "it croaks: {croaks:?}");
    for (from, length) in &croaks {
        assert_eq!(*length, 60, "a croak from tick {from}: {croaks:?}");
        assert_eq!(samples[from + length], STANDING);
    }
    assert!(
        samples
            .iter()
            .all(|pose| *pose == CROAKING || *pose == STANDING),
        "nothing but croaks in the cell"
    );
}

// ------------------------------------------------------------------------------------
// 2. The axolotl (brief 7.4).

/// One block of water at (2, -60, 0) in a stone ring under a stone roof: whatever is
/// made in it stays within the player's reach.
fn water_cell() -> Vec<(BlockPos, BlockState)> {
    let mut placed = pool((2, 2), (0, 0), -60);
    placed.extend(stone_box(bp(1, -59, -1), bp(3, -59, 1)));
    placed
}

/// Hits `key` once a tick until `done` holds after a hit, for at most `hits` hits: the
/// outputs of the hits.
fn hit_until(
    scene: &mut Scene,
    key: EntityId,
    hits: usize,
    done: impl Fn(&Scene) -> bool,
) -> Vec<TickOutput> {
    let mut seen = Vec::new();
    for _ in 0..hits {
        seen.push(hit(scene, key));
        if done(scene) {
            break;
        }
    }
    seen
}

/// Brief 7.4.7 and 7.0.2 point 7: a hit with an attacker on an axolotl in water may set
/// `PLAY_DEAD_TICKS` to 200 (`hurtServer`: one in three, and the second draw below the
/// damage or the axolotl under half its health), before its tick; the core's
/// `ValidatePlayDead` counts it down by one every brain tick, the hit's tick included,
/// and the synced flag (index 19) is set after the brain while it is above 0. So the
/// flag shows from the tick of the hit for 199 ticks (199 down to 1) and is off when
/// the count reaches 0, the tick before the memory is erased. While it shows, the
/// axolotl stays where it is (its controls do nothing). On land it never plays dead.
#[test]
fn an_axolotl_hit_in_water_plays_dead_for_199_ticks_and_never_on_land() {
    let mut scene = scene_with(&water_cell(), &[items::AXOLOTL_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(2, 0)).expect("an axolotl");
    let dead = |scene: &Scene| flag_at(scene, key, AXOLOTL_PLAYING_DEAD);
    assert!(!dead(&scene));
    let hits = hit_until(&mut scene, key, 200, dead);
    assert!(dead(&scene), "it plays dead within {} hits", hits.len());
    let still = feet(&the_entity(&scene, key));
    let mut shown = vec![true];
    for _ in 0..205 {
        scene.idle();
        shown.push(dead(&scene));
        if dead(&scene) {
            assert!(
                horizontal(feet(&the_entity(&scene, key)), still) < 0.05,
                "it lies still"
            );
        }
    }
    assert_eq!(
        runs(&shown),
        vec![(0, 199)],
        "the flag from the hit for 199 ticks"
    );

    let mut scene = scene_with(&[], &[items::AXOLOTL_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(2, 0)).expect("an axolotl");
    let hits = hit_until(&mut scene, key, 200, |scene| {
        flag_at(scene, key, AXOLOTL_PLAYING_DEAD)
    });
    assert_eq!(hits.len(), 200, "it never plays dead on land");
    assert!(entity_of(&scene, key).is_some());
}

/// Brief 7.4.6 and 7.4.8: an axolotl in water hunts the `#axolotl_hunt_targets` it sees
/// in water within 8 blocks: `StartAttacking` from `NEAREST_ATTACKABLE`, then `FIGHT`,
/// whose melee hits for its attack damage of 2 with a cooldown of 20 (a cod's 3 health
/// takes two bites). When the fight ends (its target dead), `updateActivity` sets
/// `HAS_HUNTING_COOLDOWN` for 2400 ticks, while which the attackables sensor (every 20
/// ticks) passes over hunt targets. A cod put into the pool 40 ticks after the first
/// died, once the sensor has looked again, is not bitten until the rest is over, 2400
/// ticks after the fight, and is hunted after it. (A second cod there all along may be
/// bitten at once: the attackable the sensor last named outlives the fight by up to 20
/// ticks, as the game's does.)
#[test]
fn an_axolotl_hunts_a_fish_and_rests_from_hunting_for_2400_ticks() {
    let mut scene = scene_with(
        &pool((2, 7), (-2, 2), -58),
        &[items::AXOLOTL_SPAWN_EGG, items::COD_SPAWN_EGG],
    );
    let axolotl = hatch(&mut scene, 0, floor_at(3, 0)).expect("an axolotl");
    let first = hatch(&mut scene, 1, floor_at(6, 0)).expect("a cod");
    let alive = |scene: &Scene, cod: EntityId| {
        entity_of(scene, cod).is_some_and(|_| health(scene, cod) > 0.0)
    };
    let mut killed = false;
    for _ in 0..2000 {
        let output = scene.idle();
        assert!(
            hurts(&output, first)
                .iter()
                .all(|cause| *cause == Some(axolotl)),
            "only the axolotl hurts the cod"
        );
        if !alive(&scene, first) {
            killed = true;
            break;
        }
    }
    assert!(killed, "the axolotl kills the cod within 2000 ticks");
    scene.idle_for(40);
    let at = feet(&the_entity(&scene, axolotl));
    let (x, z) = (
        (at[0].floor() as i32).clamp(3, 6),
        (at[2].floor() as i32).clamp(-1, 1),
    );
    scene.walk_to(Vec3::new(f64::from(x) + 0.5, -60.0, 4.5));
    let other = hatch(&mut scene, 1, floor_at(x + 1, z)).expect("another cod");
    for tick in 0..2340 {
        let output = scene.idle();
        assert!(
            hurts(&output, other).is_empty(),
            "the second cod is not bitten {} ticks after the first died",
            tick + 41
        );
    }
    let mut bitten = false;
    for _ in 0..800 {
        let output = scene.idle();
        if hurts(&output, other).contains(&Some(axolotl)) {
            bitten = true;
            break;
        }
    }
    assert!(bitten, "it hunts again once its rest is over");
}

/// Brief 7.4.4 and 7.4.5: an egg's axolotl is one adult of a common colour, drawn from
/// two the group draws (lucy, wild, gold, cyan: 0 to 3); blue (4) never comes of an
/// egg. Of 60 eggs every common colour comes, and no blue.
#[test]
fn an_eggs_axolotl_is_of_a_common_colour_and_never_blue() {
    let mut seen = [0; 5];
    for pen in 0..3 {
        let layout = Layout {
            block: pen + 1,
            ..Layout::alone()
        };
        let mut scene = Scene::new(
            layout,
            &World::of(&pool((-4, 4), (2, 6), -59)),
            inventory(&[(0, stack(items::AXOLOTL_SPAWN_EGG, 1))]),
            SPAWN,
        );
        for n in 0..20 {
            let at = floor_at(n % 5 - 2, 2 + n / 5);
            let axolotl = hatch(&mut scene, 0, at).expect("an axolotl");
            match shown_at(&scene, axolotl, AXOLOTL_VARIANT) {
                Some(DataShown::Int(variant)) => {
                    seen[usize::try_from(variant).expect("small")] += 1
                }
                other => panic!("a variant: {other:?}"),
            }
        }
    }
    assert!(
        seen[..4].iter().all(|n| *n > 0),
        "every common colour: {seen:?}"
    );
    assert_eq!(seen[4], 0, "no blue from an egg: {seen:?}");
}

/// Brief 7.3.15 and 7.4.6: an axolotl on land looks for water within 6 blocks
/// (`TryFindLiquid` in `IDLE`, a block of water with air above it, nearest by its
/// Manhattan order) and walks into it: here a pond sunk into the floor, three blocks
/// off.
#[test]
fn an_axolotl_on_land_walks_back_into_the_water() {
    let mut pond = filled(bp(5, -61, -2), bp(8, -61, 2), blocks::WATER);
    pond.extend(stone_box(bp(5, -62, -2), bp(8, -62, 2)));
    let mut scene = scene_with(&pond, &[items::AXOLOTL_SPAWN_EGG]);
    let axolotl = hatch(&mut scene, 0, floor_at(2, 0)).expect("an axolotl");
    let mut wet = false;
    for _ in 0..600 {
        scene.idle();
        let at = block_of(feet(&the_entity(&scene, axolotl)));
        if scene.block(at) == blocks::WATER {
            wet = true;
            break;
        }
    }
    assert!(wet, "it is in the water within 600 ticks");
}

// ------------------------------------------------------------------------------------
// 3. The frog and the tadpole (brief 7.5).

/// Brief 7.5.3: an egg's frog takes the variant of the biome it is made in, the one of
/// the highest priority whose biome tag holds it: warm in the badlands
/// (`#spawns_warm_variant_frogs`), cold in snowy plains, temperate (the fallback) in
/// plains.
#[test]
fn an_eggs_frog_is_of_its_biomes_variant() {
    for (name, variant) in [
        ("minecraft:badlands", "minecraft:warm"),
        ("minecraft:snowy_plains", "minecraft:cold"),
        ("minecraft:plains", "minecraft:temperate"),
    ] {
        let mut scene = scene_in(&[], biome(name), &[items::FROG_SPAWN_EGG]);
        let frog = hatch(&mut scene, 0, floor_at(2, 0)).expect("a frog");
        assert_eq!(
            shown_at(&scene, frog, FROG_VARIANT),
            Some(DataShown::Holder(
                Holder::FrogVariant,
                registry_id("minecraft:frog_variant", variant)
            )),
            "{name}"
        );
    }
}

/// A platform of stone two blocks high (its top at y = -58) inside x and z: a frog or a
/// goat on the floor cannot walk up it, and so long-jumps onto it.
fn platform(x: (i32, i32), z: (i32, i32), high: i32) -> Vec<(BlockPos, BlockState)> {
    stone_box(bp(x.0, -60, z.0), bp(x.1, -61 + high, z.1))
}

/// One long jump as the region showed it: the tick of the launch's sound, the tick of
/// the landing, and where the jumper stood and landed.
#[derive(Debug)]
struct Jump {
    launch: usize,
    landing: usize,
    from: [f64; 3],
    to: [f64; 3],
}

/// Idles `scene` for up to `ticks` ticks, noting each long jump of `key` (its launch
/// sound `launch_sound`, at volume 1, its landing sound `landing_sound`, at volume 2)
/// and checking the jump's poses and its 40 ticks of standing still before it.
fn watch_jumps(
    scene: &mut Scene,
    key: EntityId,
    ticks: usize,
    launch_sound: i32,
    landing_sound: i32,
    enough: usize,
) -> Vec<Jump> {
    let mut jumps: Vec<Jump> = Vec::new();
    let mut positions = Vec::new();
    let mut poses = Vec::new();
    let mut launched: Option<(usize, [f64; 3], bool)> = None;
    for tick in 0..ticks {
        let output = scene.idle();
        let entity = the_entity(scene, key);
        positions.push(feet(&entity));
        poses.push(pose_of(&entity));
        let launches = sounds_in(&output, launch_sound);
        if !launches.is_empty() {
            assert_eq!(launches, vec![1.0], "the launch's sound at tick {tick}");
            assert!(launched.is_none(), "a launch in the air at tick {tick}");
            assert!(tick >= 40, "watched long enough to see the preparation");
            let still = positions[tick - 40];
            for (back, at) in positions[tick - 40..tick].iter().enumerate() {
                assert!(
                    horizontal(*at, still) < 1e-9 && (at[1] - still[1]).abs() < 1e-9,
                    "standing still for the 40 ticks before the launch: {back} {at:?} {still:?}"
                );
            }
            // A frog may prepare and leap while it croaks, and a croak that stops in
            // the tick after the leap stands it up after `LongJumpMidJump` set its
            // pose, as the game's running order has it: such a jump's poses are not
            // judged.
            launched = Some((tick, still, poses[tick] == STANDING));
        }
        if let Some((launch, from, clean)) = launched {
            if clean && tick == launch + 1 {
                assert_eq!(
                    poses[tick],
                    LONG_JUMPING,
                    "in the air the tick after the launch: {:?} {:?}",
                    &positions[launch - 1..=tick],
                    &poses[launch - 1..=tick]
                );
            }
            let landings = sounds_in(&output, landing_sound)
                .into_iter()
                .filter(|volume| *volume == 2.0)
                .count();
            if tick > launch && landings > 0 {
                if clean {
                    assert_eq!(poses[tick], STANDING, "standing again when it lands");
                    assert!(
                        poses[launch + 1..tick]
                            .iter()
                            .all(|pose| *pose == LONG_JUMPING),
                        "long jumping all the way"
                    );
                }
                jumps.push(Jump {
                    launch,
                    landing: tick,
                    from,
                    to: positions[tick],
                });
                launched = None;
                if jumps.len() >= enough {
                    break;
                }
            }
        }
    }
    jumps
}

/// Brief 7.5.4, 7.3.16 and 7.3.17: a frog whose long-jump cooldown (100 to 140 from its
/// `finalizeSpawn`) has run out looks for a landing within 4 blocks across and 2 up or
/// down (a platform two blocks high is in reach), stands still for the 40
/// ticks of the preparation, and leaps with `FROG_LONG_JUMP` (volume 1); the tick after,
/// `LongJumpMidJump` shows the pose `LONG_JUMPING`, and on landing the frog stands with
/// `FROG_STEP` at volume 2. The landing sets the cooldown again (100 to 140, counted by
/// the core from the tick after), and only once it is gone does the frog start the next
/// jump's 40 ticks: the next launch comes at least 142 ticks after a landing.
#[test]
fn a_frog_long_jumps_onto_a_ledge_after_standing_still_and_rests_after_landing() {
    let mut placed = platform((5, 7), (-2, 2), 2);
    placed.extend(ring(-2, -4, 9, 4, 4));
    let mut scene = scene_with(&placed, &[items::FROG_SPAWN_EGG]);
    let frog = hatch(&mut scene, 0, floor_at(3, 0)).expect("a frog");
    let jumps = watch_jumps(
        &mut scene,
        frog,
        3000,
        sounds::ENTITY_FROG_LONG_JUMP,
        sounds::ENTITY_FROG_STEP,
        3,
    );
    assert!(jumps.len() >= 2, "the frog long-jumps: {jumps:?}");
    assert!(
        jumps[0].launch >= 100 + 40,
        "not before its first cooldown and preparation: {jumps:?}"
    );
    for jump in &jumps {
        assert!(
            horizontal(jump.from, jump.to) <= 4.0 * 2.0_f64.sqrt() + 1.0,
            "within 4 blocks across: {jump:?}"
        );
    }
    for pair in jumps.windows(2) {
        assert!(
            pair[1].launch - pair[0].landing >= 142,
            "the next launch waits for the cooldown and the preparation: {jumps:?}"
        );
    }
}

/// Brief 7.5.5 and plan 1.2: the frog's tongue takes only `#frog_food` entities (slimes
/// and magma cubes, N9's): a frog among fish and tadpoles never sets `USING_TONGUE`, nor
/// hurts one.
#[test]
fn a_frogs_tongue_takes_no_fish_nor_tadpole() {
    let mut placed = pool((2, 6), (-2, 2), -60);
    placed.extend(ring(-3, -4, 8, 4, 3));
    let mut scene = scene_with(
        &placed,
        &[
            items::FROG_SPAWN_EGG,
            items::TADPOLE_SPAWN_EGG,
            items::COD_SPAWN_EGG,
        ],
    );
    let frog = hatch(&mut scene, 0, floor_at(0, 0)).expect("a frog");
    let prey = [
        hatch(&mut scene, 1, floor_at(3, 0)).expect("a tadpole"),
        hatch(&mut scene, 2, floor_at(4, 1)).expect("a cod"),
    ];
    for _ in 0..1500 {
        let output = scene.idle();
        assert_ne!(pose(&scene, frog), USING_TONGUE);
        for key in prey {
            assert!(!hurts(&output, key).contains(&Some(frog)), "nothing eaten");
        }
    }
}

/// Every block of `placed` in the world whose state is `state`, as the region has it now.
fn blocks_now(scene: &Scene, around: (BlockPos, BlockPos), state: BlockState) -> Vec<BlockPos> {
    let mut found = Vec::new();
    for x in around.0.x..=around.1.x {
        for y in around.0.y..=around.1.y {
            for z in around.0.z..=around.1.z {
                if scene.block(bp(x, y, z)) == state {
                    found.push(bp(x, y, z));
                }
            }
        }
    }
    found
}

/// Brief 7.5.7: two frogs fed slime balls (`#frog_food`; a creative player keeps the
/// ball) breed (`AnimalMakeLove`), and the one whose `spawnChildFromBreeding` runs
/// becomes pregnant instead of having a child. In `LAY_SPAWN` it goes to land beside
/// water (`TryFindLandNearLiquid`) and on the way lays frogspawn beside itself, at its
/// own height, on water one block below (`TryLaySpawnOnFluidNearLand`), with
/// `FROG_LAY_SPAWN`; no frog is born. The frogspawn hatches 3600 to 11999 ticks after
/// it was placed (`onPlace`'s `nextInt(3600, 12000)`): the block goes, with
/// `FROGSPAWN_HATCH`, and 2 to 5 tadpoles appear in its block, between 0.2 and 0.8 of
/// it across, half a block below it, in the water.
#[test]
fn two_frogs_fed_breed_into_frogspawn_on_water_which_hatches_tadpoles() {
    let mut placed = filled(bp(4, -61, -2), bp(8, -61, 2), blocks::WATER);
    placed.extend(stone_box(bp(4, -62, -2), bp(8, -62, 2)));
    placed.extend(ring(-1, -4, 10, 4, 4));
    let mut scene = scene_with(&placed, &[items::FROG_SPAWN_EGG, items::SLIME_BALL]);
    let frogs = [
        hatch(&mut scene, 0, floor_at(2, 0)).expect("a frog"),
        hatch(&mut scene, 0, floor_at(2, 1)).expect("a frog"),
    ];
    scene.idle();
    let fed = use_on(&mut scene, 1, &frogs);
    assert_eq!(
        frogs
            .iter()
            .map(|frog| events_of(std::slice::from_ref(&fed), *frog, HEARTS))
            .collect::<Vec<_>>(),
        vec![1, 1],
        "both fall in love"
    );
    assert_eq!(
        carried(&scene, items::SLIME_BALL),
        1,
        "the player keeps the ball"
    );
    // A player holding a slime ball tempts the frogs, and a frog tempted from close by
    // stands still and lays nothing (its temptation outlives `IDLE`).
    scene.select(0);
    let area = (bp(-1, -61, -4), bp(10, -59, 4));
    let mut laid = None;
    for tick in 0..3000 {
        let output = scene.idle();
        let spawn = blocks_now(&scene, area, blocks::FROGSPAWN);
        if !spawn.is_empty() {
            assert_eq!(spawn.len(), 1, "one frogspawn: {spawn:?}");
            assert_eq!(played(&output, sounds::ENTITY_FROG_LAY_SPAWN), 1);
            laid = Some((tick, spawn[0]));
            break;
        }
    }
    let (_, at) = laid.expect("frogspawn within 3000 ticks");
    assert_eq!(at.y, -60, "at the frogs' height");
    assert_eq!(scene.block(at.offset(0, -1, 0)), blocks::WATER, "on water");
    assert_eq!(
        scene.of_kind(entity_types::FROG).len(),
        2,
        "no frog is born of it"
    );
    let mut hatched = None;
    for tick in 1..=12_000 {
        let output = scene.idle();
        if scene.block(at) != blocks::FROGSPAWN {
            assert_eq!(played(&output, sounds::BLOCK_FROGSPAWN_HATCH), 1);
            hatched = Some(tick);
            break;
        }
    }
    let hatched = hatched.expect("it hatches");
    assert!(
        (3600..=11_999).contains(&hatched),
        "hatched {hatched} ticks after it was laid"
    );
    let tadpoles = scene.of_kind(entity_types::TADPOLE);
    assert!(
        (2..=5).contains(&tadpoles.len()),
        "{} tadpoles",
        tadpoles.len()
    );
    for tadpole in &tadpoles {
        let [x, y, z] = tadpole.position;
        let (dx, dz) = (x - f64::from(at.x), z - f64::from(at.z));
        assert!(
            (0.19..=0.81).contains(&dx) && (0.19..=0.81).contains(&dz),
            "in the frogspawn's block: {:?}",
            tadpole.position
        );
        assert!(y < f64::from(at.y), "in the water below it: {y}");
    }
}

/// `AgeableMob.getSpeedUpSecondsWhenFeeding` and `ageUp` as the tadpole feeds
/// (brief 7.5.8): a tenth of the whole seconds left, as ticks.
fn fed_age(age: i32) -> i32 {
    let left = (24_000 - age).max(0);
    let seconds = ((left / 20) as f32 * 0.1_f32) as i32;
    age + seconds * 20
}

/// Brief 7.5.8: a tadpole counts its age up by one every tick and becomes a frog when it
/// reaches 24000; a slime ball fed to it takes a tenth of the whole seconds it has left
/// off (a creative player keeps the ball). Fed thirty times, one ball a tick, it turns
/// when the count says (the tick it was made in counted or not): the frog is of the
/// variant of the biome it stands in (a cold one in snowy plains), and the tadpole plays
/// `TADPOLE_GROW_UP` at volume 0.15.
#[test]
fn a_tadpole_fed_slime_balls_grows_into_a_frog_of_its_biome_when_its_count_says() {
    let mut scene = scene_in(
        &water_cell(),
        biome("minecraft:snowy_plains"),
        &[items::TADPOLE_SPAWN_EGG, items::SLIME_BALL],
    );
    let tadpole = hatch(&mut scene, 0, floor_at(2, 0)).expect("a tadpole");
    scene.select(1);
    // The tadpole's age after the tick it was made in is 1 or 0, as that tick ticked
    // it or not, and one more after the selection's.
    let mut ages = [2, 1];
    for _ in 0..30 {
        scene.input(used(tadpole));
        for age in &mut ages {
            *age = fed_age(*age) + 1;
        }
    }
    assert_eq!(
        carried(&scene, items::SLIME_BALL),
        1,
        "the player keeps the ball"
    );
    let left: Vec<i32> = ages.iter().map(|age| 24_000 - age).collect();
    assert!(left.iter().all(|ticks| *ticks < 2_000), "{left:?}");
    let mut grown = None;
    for tick in 1..=3_000 {
        let output = scene.idle();
        if !scene.of_kind(entity_types::FROG).is_empty() {
            assert_eq!(
                sounds_in(&output, sounds::ENTITY_TADPOLE_GROW_UP),
                vec![0.15]
            );
            grown = Some(tick);
            break;
        }
        assert!(entity_of(&scene, tadpole).is_some(), "the tadpole is there");
    }
    let grown = grown.expect("it grows up");
    assert!(
        left.contains(&grown),
        "grown {grown} ticks after feeding, {left:?} left"
    );
    assert!(entity_of(&scene, tadpole).is_none(), "the tadpole is gone");
    let frog = scene.of_kind(entity_types::FROG)[0].clone();
    assert_eq!(
        shown_of(&frog, FROG_VARIANT),
        Some(DataShown::Holder(
            Holder::FrogVariant,
            registry_id("minecraft:frog_variant", "minecraft:cold")
        )),
        "a cold frog in snowy plains"
    );
}

// ------------------------------------------------------------------------------------
// 4. The goat (brief 7.6).

fn is_screaming(scene: &Scene, goat: EntityId) -> bool {
    flag_at(scene, goat, GOAT_SCREAMING)
}

/// Brief 7.6.3, 7.6.7, 7.3.16 and 7.3.17: an egg's goat has a long-jump cooldown of 600
/// to 1200 (`initMemories` in `finalizeSpawn`), counted down by the core; once it is
/// gone `LONG_JUMP` is active and the goat leaps onto a ledge it cannot walk up (5
/// across, 5 up at most) after 40 ticks standing still, with `GOAT_LONG_JUMP` (or the
/// screaming goat's), shown `LONG_JUMPING` while in the air and landing with
/// `GOAT_STEP` at volume 2. The landing sets the full cooldown again (600 to 1200, not
/// halved), so the next leap comes at least 642 ticks after a landing.
#[test]
fn a_goat_long_jumps_onto_a_ledge_only_once_its_cooldown_of_600_to_1200_is_over() {
    let mut placed = platform((6, 8), (-2, 2), 3);
    placed.extend(ring(-2, -5, 10, 5, 8));
    let mut scene = scene_with(&placed, &[items::GOAT_SPAWN_EGG]);
    let goat = hatch(&mut scene, 0, floor_at(3, 0)).expect("a goat");
    let launch = if is_screaming(&scene, goat) {
        sounds::ENTITY_GOAT_SCREAMING_LONG_JUMP
    } else {
        sounds::ENTITY_GOAT_LONG_JUMP
    };
    let jumps = watch_jumps(&mut scene, goat, 4000, launch, sounds::ENTITY_GOAT_STEP, 2);
    assert!(!jumps.is_empty(), "the goat long-jumps");
    assert!(
        jumps[0].launch >= 600 + 40,
        "not before its first cooldown and preparation: {jumps:?}"
    );
    for jump in &jumps {
        assert!(
            horizontal(jump.from, jump.to) <= 5.0 * 2.0_f64.sqrt() + 1.0,
            "within 5 blocks across: {jump:?}"
        );
    }
    for pair in jumps.windows(2) {
        assert!(
            pair[1].launch - pair[0].landing >= 642,
            "the next leap waits for the cooldown: {jumps:?}"
        );
    }
}

/// Brief 7.6.3, 7.3.18, 7.3.19 and 7.0.3 point 15: once its ram cooldown (600 to 6000
/// from the egg) is over, a goat rams the nearest visible mob that is not a goat, never
/// a creative player. It walks to a start 4 to 7 blocks from the target on a line
/// through the target's block and lowers its head there (entity event 58 every tick it
/// stands on the start); 20 ticks after it first stood there it plays
/// `GOAT_PREPARE_RAM` and charges, hurts the first living entity its box meets (the cow,
/// by the goat) with `GOAT_RAM_IMPACT`, and raises its head (59). A ram sets the
/// cooldown again, 600 to 6000 (100 to 300 for a screaming goat): no head is lowered
/// for 600 ticks after (100 for a screamer).
#[test]
fn a_goat_rams_a_cow_after_lowering_its_head_and_never_the_creative_player() {
    let mut scene = scene_with(
        &ring(-6, -6, 9, 6, 8),
        &[items::GOAT_SPAWN_EGG, items::COW_SPAWN_EGG],
    );
    let goat = hatch(&mut scene, 0, floor_at(2, 0)).expect("a goat");
    let cow = hatch(&mut scene, 1, floor_at(5, 0)).expect("a cow");
    let screaming = is_screaming(&scene, goat);
    let (prepare, impact) = if screaming {
        (
            sounds::ENTITY_GOAT_SCREAMING_PREPARE_RAM,
            sounds::ENTITY_GOAT_SCREAMING_RAM_IMPACT,
        )
    } else {
        (
            sounds::ENTITY_GOAT_PREPARE_RAM,
            sounds::ENTITY_GOAT_RAM_IMPACT,
        )
    };
    let mut seen: Vec<TickOutput> = Vec::new();
    let mut rammed = None;
    for tick in 0..15_000 {
        let output = scene.idle();
        let me = scene.me();
        assert!(hurts(&output, me).is_empty(), "the player is never rammed");
        let hit = played(&output, impact) > 0;
        seen.push(output);
        if hit {
            rammed = Some(tick);
            break;
        }
    }
    let rammed = rammed.expect("the goat rams within 15000 ticks");
    assert!(
        hurts(&seen[rammed], cow).contains(&Some(goat)),
        "the cow is hurt by the goat"
    );
    assert!(event_in(&seen[rammed], goat, HEAD_UP), "the head comes up");
    let prepared = (0..rammed)
        .rev()
        .find(|tick| played(&seen[*tick], prepare) > 0)
        .expect("the ram was prepared");
    assert!(prepared >= 20, "{prepared}");
    for (tick, output) in seen
        .iter()
        .enumerate()
        .take(prepared + 1)
        .skip(prepared - 20)
    {
        assert!(
            event_in(output, goat, HEAD_DOWN),
            "the head down every tick of the 20 before the charge: {tick}"
        );
    }
    let rest = if screaming { 100 } else { 600 };
    for tick in 0..rest {
        let output = scene.idle();
        assert!(
            !event_in(&output, goat, HEAD_DOWN),
            "no ram {tick} ticks after a ram"
        );
    }
}

/// Brief 7.6.5 and 7.6.7: a goat from an egg has both horns but one in ten adults
/// (`nextFloat() < 0.1`, then `nextBoolean()` for which), and is a screaming goat one
/// time in fifty; of 300 eggs some 30 have one horn, never none, both sides among them.
/// Brief 7.0.3 point 4: a goat drops no horn when it dies (`entities/goat.json` has no
/// pools).
#[test]
fn one_goat_in_ten_has_one_horn_and_a_dead_goat_drops_none() {
    let mut one_horned = [0, 0];
    let mut screaming = 0;
    // Ten pens of thirty, so that each tick has few goats to tick.
    // Each pen's keys from a block of its own, so that each draws its own goats.
    for pen in 0..10 {
        let layout = Layout {
            block: pen + 1,
            ..Layout::alone()
        };
        let mut scene = Scene::new(
            layout,
            &World::default(),
            inventory(&[(0, stack(items::GOAT_SPAWN_EGG, 1))]),
            SPAWN,
        );
        for n in 0..30 {
            let across = if (n / 6) % 2 == 0 {
                n % 6 - 3
            } else {
                2 - n % 6
            };
            let at = floor_at(across * 2, (n / 6) * 2 - 5);
            scene.walk_to(Vec3::new(
                f64::from(at.x) - 1.5,
                -60.0,
                f64::from(at.z) + 0.5,
            ));
            let goat = hatch(&mut scene, 0, at).unwrap_or_else(|| panic!("goat {n} of pen {pen}"));
            let left = flag_at(&scene, goat, GOAT_LEFT_HORN);
            let right = flag_at(&scene, goat, GOAT_RIGHT_HORN);
            assert!(left || right, "goat {n} of pen {pen} has a horn");
            if !left {
                one_horned[0] += 1;
            }
            if !right {
                one_horned[1] += 1;
            }
            if is_screaming(&scene, goat) {
                screaming += 1;
            }
        }
    }
    let one = one_horned[0] + one_horned[1];
    assert!(
        (12..=50).contains(&one),
        "{one_horned:?} of 300 have one horn"
    );
    assert!(one_horned.iter().all(|n| *n > 0), "{one_horned:?}");
    assert!(screaming <= 20, "{screaming} of 300 scream");

    let mut scene = scene_with(&[], &[items::GOAT_SPAWN_EGG]);
    let goat = hatch(&mut scene, 0, floor_at(2, 0)).expect("a goat");
    for _ in 0..40 {
        beside(&mut scene, goat);
        hit(&mut scene, goat);
        scene.idle_for(10);
        if entity_of(&scene, goat).is_none_or(|_| health(&scene, goat) <= 0.0) {
            break;
        }
    }
    scene.idle_for(30);
    assert!(entity_of(&scene, goat).is_none(), "the goat died");
    assert_eq!(
        lying_count(&scene.all(), items::GOAT_HORN),
        0,
        "no horn on death"
    );
}

/// Brief 7.8.5: an armadillo rolled up cannot be fed (`mobInteract` fails while it is
/// scared; `canFallInLove` is false), and once it has unrolled a spider eye puts it in
/// love.
#[test]
fn a_rolled_up_armadillo_takes_no_spider_eye() {
    let mut scene = scene_with(&[], &[items::ARMADILLO_SPAWN_EGG, items::SPIDER_EYE]);
    let armadillo = hatch(&mut scene, 0, floor_at(3, 0)).expect("an armadillo");
    scene.idle_for(3);
    beside(&mut scene, armadillo);
    let at = player_feet(&scene);
    scene.inputs_of(
        0,
        vec![
            PlayerInput::Attack { entity: armadillo },
            moved_to(Vec3::new(at[0] - 9.0, -60.0, at[2])),
        ],
    );
    assert_ne!(armadillo_state(&scene, armadillo), IDLE);
    scene.idle_for(20);
    beside(&mut scene, armadillo);
    let fed = use_on(&mut scene, 1, &[armadillo]);
    assert_eq!(
        events_of(&[fed], armadillo, HEARTS),
        0,
        "not while rolled up"
    );
    scene.select(0);
    let at = player_feet(&scene);
    scene.input(moved_to(Vec3::new(at[0] - 9.0, -60.0, at[2])));
    assert!(
        scene.idle_until(200, &mut Vec::new(), |scene, _| armadillo_state(
            scene, armadillo
        ) == IDLE),
        "it unrolls"
    );
    scene.idle_for(10);
    beside(&mut scene, armadillo);
    let fed = use_on(&mut scene, 1, &[armadillo]);
    assert_eq!(
        events_of(&[fed], armadillo, HEARTS),
        1,
        "in love once unrolled"
    );
}

/// Brief 7.8.5: rolled up, an armadillo takes `(damage − 1) / 2` of a hit, worked out
/// before anything else, and sounds `ARMADILLO_HURT_REDUCED`: a creative player's full
/// blow of 1 takes a point off an idle armadillo and nothing off a rolled-up one. The
/// first blow sounds reduced too: `LivingEntity.hurtServer` plays the hurt sound after
/// `actuallyHurt`, which has rolled it up by then.
#[test]
fn a_rolled_up_armadillo_takes_half_a_hit_less_one() {
    let mut scene = scene_with(&[], &[items::ARMADILLO_SPAWN_EGG]);
    let armadillo = hatch(&mut scene, 0, floor_at(3, 0)).expect("an armadillo");
    scene.idle_for(20);
    beside(&mut scene, armadillo);
    let first = hit(&mut scene, armadillo);
    assert_eq!(health(&scene, armadillo), 11.0, "a point off");
    assert_eq!(armadillo_state(&scene, armadillo), ROLLING);
    assert_eq!(played(&first, sounds::ENTITY_ARMADILLO_HURT_REDUCED), 1);
    assert_eq!(played(&first, sounds::ENTITY_ARMADILLO_HURT), 0);
    scene.idle_for(20);
    beside(&mut scene, armadillo);
    let second = hit(&mut scene, armadillo);
    assert_ne!(armadillo_state(&scene, armadillo), IDLE);
    assert_eq!(health(&scene, armadillo), 11.0, "nothing off, rolled up");
    assert_eq!(played(&second, sounds::ENTITY_ARMADILLO_HURT_REDUCED), 1);
    assert_eq!(played(&second, sounds::ENTITY_ARMADILLO_HURT), 0);
}

// ------------------------------------------------------------------------------------
// 5. The camel (brief 7.7).

fn camel_pose_tick(scene: &Scene, camel: EntityId) -> i64 {
    match shown_at(scene, camel, CAMEL_POSE_TICK) {
        Some(DataShown::Long(tick)) => tick,
        other => panic!("a long: {other:?}"),
    }
}

/// Brief 7.7.3, 7.7.4 and 7.7.7: a camel from an egg stands with its transition over
/// (its last pose change 53 ticks back, `finalizeSpawn`) and sits down only once it
/// has stood for 400 ticks (`RandomSitting(20)`, one of the four choices of its
/// strolling `RunOne`): `CAMEL_SIT`, the pose `SITTING`, and its last pose change the
/// game time negated. It stays where it sat (`refuseToMove` while sitting) and stands
/// up only after 400 ticks more, with `CAMEL_STAND`, the pose `STANDING` and the game
/// time; for the 52 ticks of standing up it does not move either. The camel husk has
/// the camel's brain, with sounds of its own.
#[test]
fn a_camel_sits_down_and_stands_up_after_400_ticks_and_moves_only_once_it_is_up() {
    for (egg, sit, stand) in [
        (
            items::CAMEL_SPAWN_EGG,
            sounds::ENTITY_CAMEL_SIT,
            sounds::ENTITY_CAMEL_STAND,
        ),
        (
            items::CAMEL_HUSK_SPAWN_EGG,
            sounds::ENTITY_CAMEL_HUSK_SIT,
            sounds::ENTITY_CAMEL_HUSK_STAND,
        ),
    ] {
        let mut scene = scene_with(&ring(-6, -6, 10, 6, 2), &[egg]);
        let camel = hatch(&mut scene, 0, floor_at(3, 0)).expect("a camel");
        let mut sat = None;
        for tick in 1..=6000 {
            let output = scene.idle();
            if pose(&scene, camel) == SITTING {
                assert_eq!(played(&output, sit), 1);
                assert_eq!(
                    camel_pose_tick(&scene, camel),
                    -(output.clock.game_time as i64),
                    "the game time of the sitting, negated"
                );
                sat = Some(tick);
                break;
            }
        }
        let sat = sat.expect("the camel sits down");
        assert!(sat >= 400 - 53, "not before it stood 400 ticks: {sat}");
        let place = feet(&the_entity(&scene, camel));
        let mut stood = None;
        for tick in 1..=6000 {
            let output = scene.idle();
            assert!(
                horizontal(feet(&the_entity(&scene, camel)), place) < 1e-6,
                "it stays where it sat"
            );
            if pose(&scene, camel) == STANDING {
                assert_eq!(played(&output, stand), 1);
                assert_eq!(
                    camel_pose_tick(&scene, camel),
                    output.clock.game_time as i64,
                    "the game time of the standing"
                );
                stood = Some(tick);
                break;
            }
        }
        let stood = stood.expect("the camel stands up");
        assert!(stood >= 400, "it sat at least 400 ticks: {stood}");
        for tick in 1..52 {
            scene.idle();
            assert!(
                horizontal(feet(&the_entity(&scene, camel)), place) < 1e-6,
                "it does not move {tick} ticks into standing up"
            );
        }
    }
}

/// Brief 7.7.3 and 7.7.4: a camel hit stands up at once (`actuallyHurt`:
/// `standUpInstantly`, no sound, its last pose change set back 53 ticks so that the
/// transition is over) and panics (`CamelPanic(4.0)`, from `hurt_by` at the hurt
/// sensor's next scan): it runs off.
#[test]
fn a_sitting_camel_hit_stands_up_at_once_and_runs_off() {
    let mut scene = scene_with(&[], &[items::CAMEL_SPAWN_EGG]);
    let camel = hatch(&mut scene, 0, floor_at(3, 0)).expect("a camel");
    assert!(
        scene.idle_until(6000, &mut Vec::new(), |scene, _| pose(scene, camel)
            == SITTING),
        "the camel sits down"
    );
    scene.idle_for(5);
    beside(&mut scene, camel);
    let place = feet(&the_entity(&scene, camel));
    let output = hit(&mut scene, camel);
    assert_eq!(pose(&scene, camel), STANDING, "up at once");
    assert_eq!(
        played(&output, sounds::ENTITY_CAMEL_STAND),
        0,
        "without a sound"
    );
    let now = output.clock.game_time as i64;
    assert_eq!(camel_pose_tick(&scene, camel), (now - 53).max(0));
    let mut ran = false;
    for _ in 0..120 {
        scene.idle();
        if horizontal(feet(&the_entity(&scene, camel)), place) > 3.0 {
            ran = true;
            break;
        }
    }
    assert!(ran, "it runs off within 120 ticks of the hit");
}

/// Brief 7.3.13 and the plan's section 8 item 2: what panics an animal is its last
/// damage source in `#panic_causes`, which counts fire and lava as well as a hit. A
/// camel standing in a pit of lava panics and runs out of it, alive; an armadillo put
/// in fire panics too (`ArmadilloPanic`, for `#panic_environmental_causes` only) and
/// runs out of it without rolling up, as only a living attacker rolls it up.
#[test]
fn fire_and_lava_panic_an_animal_out_of_them() {
    let mut placed = filled(bp(2, -61, -1), bp(4, -61, 1), blocks::LAVA);
    placed.extend(stone_box(bp(2, -62, -1), bp(4, -62, 1)));
    let mut scene = scene_with(&placed, &[items::CAMEL_SPAWN_EGG]);
    let camel = hatch(&mut scene, 0, bp(3, -62, 0)).expect("a camel");
    let start = feet(&the_entity(&scene, camel));
    let mut out = false;
    for _ in 0..200 {
        scene.idle();
        let at = feet(&the_entity(&scene, camel));
        if horizontal(at, start) > 3.0 && scene.block(block_of(at).offset(0, -1, 0)) != blocks::LAVA
        {
            out = true;
            break;
        }
    }
    assert!(out, "the camel runs out of the lava");
    assert!(health(&scene, camel) > 0.0, "alive");

    let fire =
        state("minecraft:fire[age=0,east=false,north=false,south=false,up=false,west=false]");
    let mut scene = scene_with(&[(bp(3, -60, 0), fire)], &[items::ARMADILLO_SPAWN_EGG]);
    let armadillo = hatch(&mut scene, 0, floor_at(3, 0)).expect("an armadillo");
    let start = feet(&the_entity(&scene, armadillo));
    let mut out = false;
    for _ in 0..200 {
        let output = scene.idle();
        assert_eq!(
            armadillo_state(&scene, armadillo),
            IDLE,
            "it does not roll up"
        );
        assert_eq!(played(&output, sounds::ENTITY_ARMADILLO_ROLL), 0);
        if horizontal(feet(&the_entity(&scene, armadillo)), start) > 2.0 {
            out = true;
            break;
        }
    }
    assert!(out, "the armadillo runs out of the fire");
}

// ------------------------------------------------------------------------------------
// 6. The sniffer (brief 7.9).

/// The runs of `samples` as (value, first index, length), in order.
fn spans(samples: &[i32]) -> Vec<(i32, usize, usize)> {
    let mut found: Vec<(i32, usize, usize)> = Vec::new();
    for (at, value) in samples.iter().enumerate() {
        match found.last_mut() {
            Some((last, _, length)) if last == value => *length += 1,
            _ => found.push((*value, at, 1)),
        }
    }
    found
}

/// Brief 7.9.2 to 7.9.6: a sniffer on grass sniffs (state 3, `Sniffing`, its whole 40
/// to 80 ticks), walks to a diggable block it picked (state 4) and digs (state 5, entity
/// event 63) for its `Digging(160, 180)`: the start sets the seed's moment 120 ticks on
/// (`DATA_DROP_SEED_AT_TICK`, read in `Sniffer.tick` before the brain), when a
/// torchflower seed or a pitcher pod drops by its head with `SNIFFER_DROP_SEED`. The dig
/// runs its draw's length and stops the tick after (162 to 182 ticks shown digging,
/// its start's tick and its stop's); `FinishedDigging` starts the tick after it (state
/// 6, `SNIFFER_DIGGING_STOP`) and runs 40 ticks, shown 41; the sniffer is idle one tick
/// and is then happy (state 1, `SNIFFER_HAPPY`) for its `FeelingHappy(40, 100)`, shown
/// 41 to 101 ticks. The finished dig set `SNIFF_COOLDOWN` for 9600 ticks, which
/// `Sniffing` needs absent: it does not sniff again for the next 3000 ticks.
#[test]
fn a_sniffer_sniffs_digs_up_a_seed_at_its_moment_and_rests_from_sniffing() {
    let mut placed = filled(bp(-8, -61, -8), bp(8, -61, 8), blocks::GRASS_BLOCK);
    placed.extend(ring(-8, -8, 8, 8, 2));
    let mut scene = scene_with(&placed, &[items::SNIFFER_SPAWN_EGG]);
    let sniffer = hatch(&mut scene, 0, floor_at(3, 0)).expect("a sniffer");
    let mut states = Vec::new();
    let mut seen = Vec::new();
    let mut happy_done = None;
    for tick in 0..8000 {
        seen.push(scene.idle());
        states.push(sniffer_state(&scene, sniffer));
        let found = spans(&states);
        // A whole dig: digging, rising, one idle tick, and a happiness that has ended.
        let dug = found.windows(5).position(|five| {
            five[0].0 == DIGGING
                && five[1].0 == RISING
                && five[2].0 == 0
                && five[3].0 == FEELING_HAPPY
        });
        if dug.is_some() {
            happy_done = Some(tick);
            break;
        }
    }
    assert!(
        happy_done.is_some(),
        "a whole dig within 8000 ticks: {:?}",
        spans(&states)
    );
    let found = spans(&states);
    let at = found
        .windows(5)
        .position(|five| {
            five[0].0 == DIGGING
                && five[1].0 == RISING
                && five[2].0 == 0
                && five[3].0 == FEELING_HAPPY
        })
        .expect("the dig");
    let (digging, rising, idle, happy) = (found[at], found[at + 1], found[at + 2], found[at + 3]);
    assert!(
        found[..at].iter().any(|(state, _, _)| *state == SNIFFING)
            && found[..at].iter().any(|(state, _, _)| *state == SEARCHING),
        "it sniffed and searched first: {found:?}"
    );
    let dig = digging.1;
    assert!(
        event_in(&seen[dig], sniffer, DIG),
        "event 63 as it begins to dig"
    );
    assert!(
        (162..=182).contains(&digging.2),
        "digging shown {} ticks",
        digging.2
    );
    let seeds: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|(_, output)| played(output, sounds::ENTITY_SNIFFER_DROP_SEED) > 0)
        .map(|(tick, _)| tick)
        .collect();
    assert_eq!(seeds, vec![dig + 120], "the seed 120 ticks into the dig");
    assert_eq!(rising.2, 41, "rising shown 41 ticks");
    assert_eq!(
        played(&seen[rising.1], sounds::ENTITY_SNIFFER_DIGGING_STOP),
        1
    );
    assert_eq!(idle.2, 1);
    assert!(
        (41..=101).contains(&happy.2),
        "happy shown {} ticks",
        happy.2
    );
    assert_eq!(played(&seen[happy.1], sounds::ENTITY_SNIFFER_HAPPY), 1);
    let lying: Vec<ItemStack> = scene
        .all()
        .iter()
        .filter_map(|entity| match &entity.data {
            EntityData::Item { stack, .. } => Some(*stack),
            _ => None,
        })
        .collect();
    assert_eq!(lying.len(), 1, "one seed: {lying:?}");
    assert!(
        [items::TORCHFLOWER_SEEDS, items::PITCHER_POD].contains(&lying[0].item),
        "{lying:?}"
    );
    for tick in 0..3000 {
        scene.idle();
        assert!(
            ![SNIFFING, SEARCHING, DIGGING].contains(&sniffer_state(&scene, sniffer)),
            "no sniffing {tick} ticks after the dig"
        );
    }
}

/// Brief 7.9.6: two sniffers fed torchflower seeds (`#sniffer_food`; a creative player
/// keeps them) breed (`AnimalMakeLove`, which first stops their sniffing), and the one
/// whose `spawnChildFromBreeding` runs lays a sniffer egg as an item at its feet with
/// `SNIFFER_EGG_PLOP`, never a baby.
#[test]
fn two_sniffers_fed_lay_an_egg_and_have_no_baby() {
    let mut placed = filled(bp(-6, -61, -6), bp(8, -61, 6), blocks::GRASS_BLOCK);
    placed.extend(ring(-6, -6, 8, 6, 2));
    let mut scene = scene_with(
        &placed,
        &[items::SNIFFER_SPAWN_EGG, items::TORCHFLOWER_SEEDS],
    );
    let sniffers = [
        hatch(&mut scene, 0, floor_at(2, -1)).expect("a sniffer"),
        hatch(&mut scene, 0, floor_at(2, 2)).expect("a sniffer"),
    ];
    let fed = use_on(&mut scene, 1, &sniffers);
    scene.select(0);
    assert_eq!(
        sniffers
            .iter()
            .map(|key| events_of(std::slice::from_ref(&fed), *key, HEARTS))
            .collect::<Vec<_>>(),
        vec![1, 1],
        "both fall in love"
    );
    assert_eq!(carried(&scene, items::TORCHFLOWER_SEEDS), 1);
    let mut laid = None;
    for tick in 0..600 {
        let output = scene.idle();
        if lying_count(&scene.all(), items::SNIFFER_EGG) > 0 {
            assert_eq!(played(&output, sounds::BLOCK_SNIFFER_EGG_PLOP), 1);
            laid = Some(tick);
            break;
        }
    }
    assert!(laid.is_some(), "an egg is laid within 600 ticks");
    assert_eq!(lying_count(&scene.all(), items::SNIFFER_EGG), 1, "one egg");
    let egg = items_lying(&scene.all())
        .into_iter()
        .find(|(stack, _)| stack.item == items::SNIFFER_EGG)
        .expect("the egg")
        .1;
    assert!(
        sniffers
            .iter()
            .any(|key| horizontal(feet(&the_entity(&scene, *key)), egg) < 1.0),
        "at a parent's feet"
    );
    scene.idle_for(100);
    assert_eq!(
        scene.of_kind(entity_types::SNIFFER).len(),
        2,
        "no baby is born of it"
    );
}

/// Brief 7.9.3 and 7.9.4: `Digging` goes on only while `canDig()` holds, and a tempted
/// sniffer cannot dig: a player who comes up holding torchflower seeds while it digs
/// stops the dig at the temptation sensor's next scan (within 20 ticks). Cut short, the
/// dig resets the sniffing (`IDLING`, not rising), drops no seed if its moment had not
/// come, and sets no `SNIFF_COOLDOWN`: once the seeds are put away the sniffer sniffs
/// again within the next minutes, where a finished dig would keep it from it for 9600
/// ticks.
#[test]
fn a_sniffer_tempted_while_it_digs_stops_and_is_free_to_sniff_again() {
    let mut placed = filled(bp(-8, -61, -8), bp(8, -61, 8), blocks::GRASS_BLOCK);
    placed.extend(ring(-8, -8, 8, 8, 2));
    let mut scene = scene_with(
        &placed,
        &[items::SNIFFER_SPAWN_EGG, items::TORCHFLOWER_SEEDS],
    );
    let sniffer = hatch(&mut scene, 0, floor_at(3, 0)).expect("a sniffer");
    assert!(
        scene.idle_until(8000, &mut Vec::new(), |scene, _| sniffer_state(
            scene, sniffer
        ) == DIGGING),
        "it digs"
    );
    let at = feet(&the_entity(&scene, sniffer));
    scene.input(moved_to(Vec3::new(
        at[0].clamp(-6.0, 6.0) - 1.5,
        -60.0,
        at[2].clamp(-6.0, 6.0) + 1.5,
    )));
    let mut seen = vec![scene.select(1)];
    let mut stopped = None;
    for tick in 0..30 {
        let state = sniffer_state(&scene, sniffer);
        if state != DIGGING {
            stopped = Some((tick, state));
            break;
        }
        seen.push(scene.idle());
    }
    let (tick, state) = stopped.expect("the dig stops");
    assert!(tick <= 21, "within the sensor's 20 ticks: {tick}");
    assert_eq!(state, 0, "idle, not rising");
    for _ in 0..150 {
        seen.push(scene.idle());
    }
    assert!(
        seen.iter()
            .all(|output| played(output, sounds::ENTITY_SNIFFER_DROP_SEED) == 0),
        "no seed of a dig cut short at its start"
    );
    scene.select(0);
    assert!(
        scene.idle_until(4000, &mut Vec::new(), |scene, _| sniffer_state(
            scene, sniffer
        ) == SNIFFING),
        "it sniffs again"
    );
}

// ------------------------------------------------------------------------------------
// 7. The allay (brief 7.10).

/// Dirt lying at `position`, which may be picked up at once.
fn loose_dirt(n: u32, position: [f64; 3]) -> Entity {
    let mut item = n6_entities::dirt(n, position);
    if let EntityData::Item { pick_up_delay, .. } = &mut item.data {
        *pick_up_delay = 0;
    }
    item
}

/// What the tick showed of `key`'s main hand (`EquipmentSlot.MAINHAND`, ordinal 0).
fn main_hand_shown(output: &TickOutput, key: EntityId) -> Option<Option<ItemStack>> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::EquipmentChanged { entity, slots, .. } if *entity == key => slots
            .iter()
            .find(|(slot, _)| *slot == 0)
            .map(|(_, stack)| *stack),
        _ => None,
    })
}

/// The `Sound`s of id `sound` in the tick, with whom each leaves out.
fn sound_except(output: &TickOutput, sound: i32) -> Vec<Option<clustine_world::PlayerId>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: id, except, ..
            } if *id == sound => Some(*except),
            RegionEvent::EntitySound { sound: id, .. } if *id == sound => Some(None),
            _ => None,
        })
        .collect()
}

/// Brief 7.10.5, 7.10.3 and 7.10.4: a player who gives an allay an item (one of what
/// they hold; a creative player keeps it) makes it hold one of that kind, with
/// `ALLAY_ITEM_GIVEN` played to everyone but them, and is its liked player, whom it
/// will not let hurt it. It flies to items of that kind it sees within 32 blocks, takes
/// them up and brings them to that player, throwing them before them one at a time, so
/// the dirt lying far off comes to the player. An empty hand takes its item back (the
/// player is given it, `ALLAY_ITEM_TAKEN`), and then a hit hurts it.
#[test]
fn an_allay_given_dirt_fetches_the_dirt_lying_about_to_its_player() {
    let lying: Vec<Entity> = (0..3)
        .map(|n| loose_dirt(n, [12.5 + f64::from(n) * 0.3, -60.0, 8.5]))
        .collect();
    let world = World::default().with_entities(&lying);
    let mut scene = Scene::alone(
        &world,
        inventory(&[
            (0, stack(items::ALLAY_SPAWN_EGG, 1)),
            (1, stack(items::DIRT, 64)),
        ]),
        SPAWN,
    );
    let allay = hatch(&mut scene, 0, floor_at(2, 0)).expect("an allay");
    let given = use_on(&mut scene, 1, &[allay]);
    assert_eq!(
        main_hand_shown(&given, allay),
        Some(Some(stack(items::DIRT, 1))),
        "it holds one dirt"
    );
    assert_eq!(
        sound_except(&given, sounds::ENTITY_ALLAY_ITEM_GIVEN),
        vec![Some(player(1))],
        "the chime of the gift to all but the giver"
    );
    assert_eq!(
        carried(&scene, items::DIRT),
        64,
        "a creative player keeps it"
    );
    let before = health(&scene, allay);
    let struck = hit(&mut scene, allay);
    assert!(
        hurts(&struck, allay).is_empty(),
        "its liked player cannot hurt it"
    );
    assert_eq!(health(&scene, allay), before);
    let far = |scene: &Scene| {
        scene
            .all()
            .iter()
            .filter(|entity| {
                matches!(entity.data, EntityData::Item { .. })
                    && horizontal(entity.position, [12.5, -60.0, 8.5]) < 3.0
            })
            .count()
    };
    assert_eq!(far(&scene), 3);
    let mut brought = false;
    for _ in 0..1200 {
        scene.idle();
        if far(&scene) == 0 && carried(&scene, items::DIRT) > 64 {
            brought = true;
            break;
        }
    }
    assert!(
        brought,
        "the dirt is fetched and the player takes it up: {} far, {} carried",
        far(&scene),
        carried(&scene, items::DIRT)
    );
    let held = carried(&scene, items::DIRT);
    scene.walk_to(Vec3::new(
        the_entity(&scene, allay).position[0] - 1.5,
        -60.0,
        the_entity(&scene, allay).position[2],
    ));
    // The dirt taken up went into the first empty slot, 2.
    let taken = use_on(&mut scene, 8, &[allay]);
    assert_eq!(
        main_hand_shown(&taken, allay),
        Some(None),
        "its hand is empty: {:?} {:?} {:?}",
        taken.events,
        the_entity(&scene, allay).position,
        player_feet(&scene)
    );
    assert_eq!(
        sound_except(&taken, sounds::ENTITY_ALLAY_ITEM_TAKEN),
        vec![Some(player(1))]
    );
    assert!(
        carried(&scene, items::DIRT) > held,
        "the player is given the dirt"
    );
    scene.idle_for(10);
    let struck = hit(&mut scene, allay);
    assert!(
        hurts(&struck, allay).contains(&Some(scene.me())),
        "liked no more, it can be hurt"
    );
}

// ------------------------------------------------------------------------------------
// 8. The happy ghast's ghastling (brief 7.11).

fn is_baby(scene: &Scene, key: EntityId) -> bool {
    flag_at(scene, key, index::BABY)
}

/// `AgeableMob.ageUp` of an animal fed as a baby: a tenth of the whole seconds left.
fn baby_fed_age(age: i32) -> i32 {
    let left = -age;
    let seconds = ((left / 20) as f32 * 0.1_f32) as i32;
    (age + seconds * 20).min(0)
}

/// Brief 7.11.3 and 7.11.0: an egg used on an adult happy ghast makes a ghastling
/// (`spawnOffspringFromSpawnEgg`); its brain makes it follow the nearest visible player
/// (`BabyFollowAdult` over `NEAREST_VISIBLE_PLAYER`, 3 to 16 blocks): a player who
/// walks off 12 blocks is followed until it is within some 3 of them. Fed snowballs
/// (`#happy_ghast_food`), it grows by a tenth of what it has left each, and grows up
/// when its age reaches 0, counted as the game counts it (the tick it was made in
/// counted or not).
#[test]
fn a_ghastling_follows_its_player_and_grows_up_on_snowballs() {
    let mut scene = scene_with(&[], &[items::HAPPY_GHAST_SPAWN_EGG, items::SNOWBALL]);
    let adult = hatch(&mut scene, 0, floor_at(4, 0)).expect("a happy ghast");
    assert!(
        !is_baby(&scene, adult),
        "an egg on the floor makes an adult"
    );
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    let born = use_on(&mut scene, 0, &[adult]).clock.game_time;
    let babies = others_of(&scene, entity_types::HAPPY_GHAST, &before);
    assert_eq!(babies.len(), 1, "the egg on the adult makes one more");
    let baby = babies[0].key;
    assert!(is_baby(&scene, baby), "a ghastling");
    // Away from the adult, which it would follow too.
    for step in 1..=6 {
        scene.walk_to(Vec3::new(0.5 - f64::from(step) * 2.0, -60.0, 0.5));
    }
    let mut near = false;
    for _ in 0..400 {
        scene.idle();
        if horizontal(feet(&the_entity(&scene, baby)), player_feet(&scene)) < 5.0 {
            near = true;
            break;
        }
    }
    assert!(near, "the ghastling comes to the player");
    let at = feet(&the_entity(&scene, baby));
    scene.walk_to(Vec3::new(at[0] - 2.0, -60.0, at[2]));
    let lived = scene.select(1).clock.game_time - born;
    let lived = i32::try_from(lived).expect("short");
    // Its age at the start is -24000, after the tick it was made in one more or not.
    let mut ages = [-24_000 + lived + 1, -24_000 + lived];
    for _ in 0..30 {
        scene.input(used(baby));
        for age in &mut ages {
            *age = (baby_fed_age(*age) + 1).min(0);
        }
    }
    assert_eq!(
        carried(&scene, items::SNOWBALL),
        1,
        "the player keeps the snowball"
    );
    let left: Vec<i32> = ages.iter().map(|age| -age).collect();
    let mut grown = None;
    for tick in 1..=3000 {
        scene.idle();
        if !is_baby(&scene, baby) {
            grown = Some(tick);
            break;
        }
    }
    let grown = grown.expect("it grows up");
    assert!(
        left.contains(&grown),
        "grown {grown} ticks after feeding, {left:?} left"
    );
}

// ------------------------------------------------------------------------------------
// 9. The nautilus (brief 7.12).

/// Brief 7.12.3: a wild adult nautilus in water picks a fight with a pufferfish
/// (`#nautilus_hostiles`) at most once its attack cooldown (2400 to 3600 from the egg)
/// has run out, half the times it may, or at once with one that hurt it (`ANGRY_AT`):
/// `ChargeAttack` plays `NAUTILUS_DASH` at its start and drives it at the target, which
/// its box meets and hurts (attack damage 3, the cause the nautilus). The charge's
/// cooldown of 80 keeps it from charging again for 80 ticks.
#[test]
fn a_nautilus_charges_a_pufferfish() {
    let mut scene = scene_with(
        &pool((2, 9), (-3, 3), -57),
        &[items::NAUTILUS_SPAWN_EGG, items::PUFFERFISH_SPAWN_EGG],
    );
    let nautilus = hatch(&mut scene, 0, floor_at(3, 0)).expect("a nautilus");
    let pufferfish = hatch(&mut scene, 1, floor_at(6, 1)).expect("a pufferfish");
    let mut charged = None;
    let mut seen = Vec::new();
    for tick in 0..9000 {
        let output = scene.idle();
        let dash = played(&output, sounds::ENTITY_NAUTILUS_DASH) > 0;
        seen.push(output);
        if dash {
            charged = Some(tick);
            break;
        }
    }
    let charged = charged.expect("the nautilus charges within 9000 ticks");
    let mut struck = None;
    for tick in 1..=61 {
        let output = scene.idle();
        if hurts(&output, pufferfish).contains(&Some(nautilus)) {
            struck = Some(tick);
            break;
        }
    }
    let struck = struck.unwrap_or_else(|| panic!("the charge from tick {charged} hits"));
    for tick in 0..79 - struck.min(79) {
        let output = scene.idle();
        assert_eq!(
            played(&output, sounds::ENTITY_NAUTILUS_DASH),
            0,
            "no charge {tick} ticks after the hit"
        );
    }
}

/// Brief 7.12.3 ("Anger", **Inferred** there, read here from `canAttack` as the brief
/// reads it for the goat): a nautilus is angered only by an attacker it could attack,
/// and a creative player cannot be seen as an enemy: hit by one, it does not charge.
#[test]
fn a_nautilus_hit_by_a_creative_player_does_not_charge() {
    let mut scene = scene_with(&pool((2, 9), (-3, 3), -57), &[items::NAUTILUS_SPAWN_EGG]);
    let nautilus = hatch(&mut scene, 0, floor_at(3, 0)).expect("a nautilus");
    scene.idle_for(5);
    hit(&mut scene, nautilus);
    for tick in 0..400 {
        let output = scene.idle();
        assert_eq!(
            played(&output, sounds::ENTITY_NAUTILUS_DASH),
            0,
            "no charge {tick} ticks after the hit"
        );
    }
}

// ------------------------------------------------------------------------------------
// 10. The copper golem (brief 7.13).

fn chest_state() -> BlockState {
    state("minecraft:chest[facing=west,type=single]")
}

/// The menu slot of hotbar slot `n` in a chest's menu.
fn chest_hotbar(n: usize) -> i16 {
    i16::try_from(27 + 27 + n).expect("fits")
}

/// How many of `item` the container at `at` holds, as player 1 sees it opened (and
/// closes it again).
fn holds_in(scene: &mut Scene, at: BlockPos, item: i32) -> i32 {
    let (serial, output) = scene.open(at);
    let opened = the_opened(&output, player(1));
    scene.close(serial);
    opened.slots[..27]
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .map(|stack| stack.count)
        .sum()
}

fn golem_state(scene: &Scene, golem: EntityId) -> i32 {
    state_of(scene, golem, GOLEM_STATE, Serialiser::CopperGolemState)
}

/// The block events of the chest lid at `at` in the tick: their data, the openers.
fn lid_events(output: &TickOutput, at: BlockPos) -> Vec<i32> {
    block_events_at(output, at)
        .into_iter()
        .filter(|(_, kind, _)| *kind == 1)
        .map(|(_, _, data)| data)
        .collect()
}

/// Brief 7.13.7, 7.13.4 and 7.13.5: a carved pumpkin placed on a copper block makes a
/// copper golem where the pumpkin went (`copper_golem.spawn`), the copper block turning
/// into a copper chest under it. Once its first transport cooldown is over, the golem
/// goes to the copper chest that has items, stands at it 60 ticks: at the first
/// (`GETTING_ITEM`, state 1) it opens the lid (the copper chest's open sound, and its
/// block event 1 with one opener in the next tick's block events), at the ninth it
/// plays its item sound, at the sixtieth it closes the lid and takes up to 16 of the
/// first stack into its hand. It carries them to the nearest chest that is empty
/// or holds that item and, while a player has that chest open, waits by it
/// (`shouldQueueForTarget`); once it is closed, the golem stands at it (`DROPPING_ITEM`,
/// state 3) and puts the 16 in.
#[test]
fn a_copper_golem_built_on_a_copper_block_carries_items_to_a_chest_and_waits_for_a_player() {
    let destination = bp(7, -60, 0);
    let mut scene = Scene::alone(
        &World::of(&[
            (bp(2, -60, 0), blocks::COPPER_BLOCK),
            (destination, chest_state()),
        ]),
        inventory(&[
            (0, stack(items::CARVED_PUMPKIN, 1)),
            (1, stack(items::DIRT, 64)),
        ]),
        SPAWN,
    );
    let source = bp(2, -60, 0);
    scene.select(0);
    let built = scene.click(source, Face::Top);
    let golems = scene.of_kind(entity_types::COPPER_GOLEM);
    assert_eq!(golems.len(), 1, "a golem is built");
    let golem = golems[0].key;
    assert_eq!(played(&built, sounds::ENTITY_COPPER_GOLEM_SPAWN), 1);
    assert_eq!(
        scene.block(source.offset(0, 1, 0)),
        blocks::AIR,
        "the pumpkin went"
    );
    let chest = scene.block(source);
    assert_eq!(
        chest.block_id(),
        blocks::COPPER_CHEST.block_id(),
        "a copper chest"
    );
    // The dirt into the copper chest.
    let (serial, _) = scene.open(source);
    scene.menu_click(serial, chest_hotbar(1), 0, ClickKind::QuickMove);
    scene.close(serial);
    assert_eq!(holds_in(&mut scene, source, items::DIRT), 64);
    let mut seen: Vec<TickOutput> = Vec::new();
    let mut getting = None;
    for tick in 0..1000 {
        seen.push(scene.idle());
        if golem_state(&scene, golem) == GETTING_ITEM {
            getting = Some(tick);
            break;
        }
    }
    let getting = getting.expect("the golem goes to the copper chest");
    for _ in 0..80 {
        seen.push(scene.idle());
    }
    let lids: Vec<(usize, Vec<i32>)> = (getting..seen.len())
        .map(|tick| (tick - getting, lid_events(&seen[tick], source)))
        .filter(|(_, lid)| !lid.is_empty())
        .collect();
    let opened: Vec<usize> = (getting..seen.len())
        .filter(|tick| played(&seen[*tick], sounds::BLOCK_COPPER_CHEST_OPEN) > 0)
        .map(|tick| tick - getting)
        .collect();
    let closed: Vec<usize> = (getting..seen.len())
        .filter(|tick| played(&seen[*tick], sounds::BLOCK_COPPER_CHEST_CLOSE) > 0)
        .map(|tick| tick - getting)
        .collect();
    assert_eq!(
        opened,
        vec![0],
        "the lid opens with its sound at the first tick"
    );
    assert_eq!(closed, vec![59], "and closes at the sixtieth");
    // Block events an entity's tick queues run in the next tick's block events
    // (`ServerLevel.tick`: `runBlockEvents` before the entities).
    assert_eq!(
        lids.first(),
        Some(&(1, vec![1])),
        "the lid's block event of one opener the tick after the first: {lids:?}"
    );
    let item_sounds = [
        sounds::ENTITY_COPPER_GOLEM_NO_ITEM_GET,
        sounds::ENTITY_COPPER_GOLEM_NO_ITEM_NO_GET,
        sounds::ENTITY_COPPER_GOLEM_ITEM_DROP,
        sounds::ENTITY_COPPER_GOLEM_ITEM_NO_DROP,
    ];
    let sounded: Vec<usize> = (getting..seen.len())
        .filter(|tick| {
            item_sounds
                .iter()
                .any(|sound| played(&seen[*tick], *sound) > 0)
        })
        .map(|tick| tick - getting)
        .collect();
    assert_eq!(sounded, vec![8], "its item sound at the ninth tick");
    let taken = getting + 59;
    assert_eq!(
        main_hand_shown(&seen[taken], golem),
        Some(Some(stack(items::DIRT, 16))),
        "16 taken at the sixtieth tick"
    );
    assert!(
        lid_events(&seen[taken + 1], source).contains(&0),
        "the lid's block event of no opener the tick after: {lids:?}"
    );
    // The player opens the destination and keeps it open.
    scene.walk_to(Vec3::new(5.5, -60.0, 2.5));
    let (serial, _) = scene.open(destination);
    for tick in 0..400 {
        scene.idle();
        assert_ne!(
            golem_state(&scene, golem),
            DROPPING_ITEM,
            "it waits while the chest is open ({tick})"
        );
    }
    let near = horizontal(feet(&the_entity(&scene, golem)), centre_of(destination));
    assert!(near < 4.0, "it waits by the chest: {near}");
    scene.close(serial);
    let mut dropping = None;
    for tick in 0..400 {
        scene.idle();
        if golem_state(&scene, golem) == DROPPING_ITEM {
            dropping = Some(tick);
            break;
        }
    }
    assert!(dropping.is_some(), "it goes on once the chest is closed");
    scene.idle_for(70);
    assert_eq!(holds_in(&mut scene, destination, items::DIRT), 16);
    assert_eq!(holds_in(&mut scene, source, items::DIRT), 48);
}

/// Brief 7.13.4 and 7.3.21: the golem's core opens the wooden doors on its path
/// (`InteractWithDoor`: the door at the path's previous or next node, of
/// `#mob_interactable_doors`) and closes those it opened once they are no longer its
/// path's previous or next node, while it is within 3 blocks of them: a golem carrying
/// items from a copper chest to a chest behind a closed oak door opens the door, goes
/// through it, and shuts it behind it, with the door's sounds.
#[test]
fn a_copper_golem_opens_a_wooden_door_on_its_way_and_shuts_it_behind_it() {
    let door = |half: &str, open: bool| {
        state(&format!(
            "minecraft:oak_door[facing=east,half={half},hinge=left,open={open},powered=false]"
        ))
    };
    let mut placed = ring(-1, -4, 11, 4, 3);
    placed.extend(stone_box(bp(5, -60, -4), bp(5, -58, 4)));
    placed.retain(|(at, _)| !(at.x == 5 && at.z == 0 && at.y < -58));
    placed.push((bp(5, -60, 0), door("lower", false)));
    placed.push((bp(5, -59, 0), door("upper", false)));
    let destination = bp(8, -60, 0);
    placed.push((bp(2, -60, 0), blocks::COPPER_BLOCK));
    placed.push((destination, chest_state()));
    let mut scene = Scene::alone(
        &World::of(&placed),
        inventory(&[
            (0, stack(items::CARVED_PUMPKIN, 1)),
            (1, stack(items::DIRT, 64)),
        ]),
        SPAWN,
    );
    let source = bp(2, -60, 0);
    scene.select(0);
    scene.click(source, Face::Top);
    let golem = scene.of_kind(entity_types::COPPER_GOLEM)[0].key;
    let (serial, _) = scene.open(source);
    scene.menu_click(serial, chest_hotbar(1), 0, ClickKind::QuickMove);
    scene.close(serial);
    let lower = bp(5, -60, 0);
    let mut opened = None;
    let mut through = None;
    let mut shut = None;
    for tick in 0..2000 {
        let output = scene.idle();
        let now = scene.block(lower);
        if opened.is_none() && now == door("lower", true) {
            assert_eq!(played(&output, sounds::BLOCK_WOODEN_DOOR_OPEN), 1);
            opened = Some(tick);
        }
        if opened.is_some() && through.is_none() && the_entity(&scene, golem).position[0] > 6.0 {
            through = Some(tick);
        }
        if through.is_some() && now == door("lower", false) {
            assert_eq!(played(&output, sounds::BLOCK_WOODEN_DOOR_CLOSE), 1);
            shut = Some(tick);
            break;
        }
    }
    assert!(opened.is_some(), "the golem opens the door");
    assert!(through.is_some(), "and goes through it");
    assert!(shut.is_some(), "and shuts it behind it");
    scene.idle_for(200);
    scene.walk_to(Vec3::new(4.5, -60.0, 2.5));
    assert_eq!(holds_in(&mut scene, destination, items::DIRT), 16);
}

fn centre_of(at: BlockPos) -> [f64; 3] {
    [
        f64::from(at.x) + 0.5,
        f64::from(at.y),
        f64::from(at.z) + 0.5,
    ]
}

// ------------------------------------------------------------------------------------
// 11. Across a pinned line (plan section 3; brief 11.7).

/// Two regions on either side of the pinned line at x = 16, the west's player 1 and the
/// east's player 2, run a tick at a time as the store couples them: each takes the
/// other's previous tick's entity changes into its copies, its effects on its own land,
/// its arrivals, and the faces of its players (from their stays' notes).
struct Line {
    west: Scene,
    east: Scene,
    from_west: TickOutput,
    from_east: TickOutput,
    sequence: u64,
}

impl Line {
    /// The two sides, the west's keys from block `west_block` and the east's from the
    /// other, the west's player at `west_feet` holding `west_slots` and the east's at
    /// `east_feet` holding `east_slots`, one of each.
    fn new(
        west_block: u32,
        world: &World,
        (west_feet, west_slots): (Vec3, &[i32]),
        (east_feet, east_slots): (Vec3, &[i32]),
    ) -> Self {
        let holding = |slots: &[i32]| {
            let stacks: Vec<(usize, ItemStack)> = slots
                .iter()
                .enumerate()
                .map(|(slot, item)| (slot, stack(*item, 1)))
                .collect();
            inventory(&stacks)
        };
        let mut west_layout = Layout::side(true);
        let mut east_layout = Layout::side(false);
        west_layout.block = west_block;
        east_layout.block = 3 - west_block;
        let west = Scene::new(west_layout, world, holding(west_slots), west_feet);
        let east_inventory = holding(east_slots);
        let mut east = Scene {
            region: region_of(&east_layout, world, east_inventory.clone()),
            layout: east_layout,
            inventory: east_inventory,
            actors: Vec::new(),
        };
        east.join(2, east_feet);
        let mut line = Self {
            west,
            east,
            from_west: TickOutput::default(),
            from_east: TickOutput::default(),
            sequence: 1_000,
        };
        // The joins' notes went by before the line was coupled: a step of a hair makes
        // each player's face again for the other side.
        let hair = |feet: Vec3| vec![moved_to(Vec3::new(feet.x, feet.y, feet.z + 0.001))];
        line.step(hair(west_feet), hair(east_feet));
        line
    }

    /// What `into` takes of `from`'s tick.
    fn carried(&mut self, from: &TickOutput, into: &Layout) -> TickInputs {
        self.sequence += 1;
        let sequence = self.sequence;
        let mut updates: Vec<(ChunkPos, Vec<EntityChange>)> = Vec::new();
        for (chunk, change) in &from.entity_changes {
            if !into.copies.contains(chunk) {
                continue;
            }
            match updates.iter_mut().find(|(at, _)| at == chunk) {
                Some((_, changes)) => changes.push(change.clone()),
                None => updates.push((*chunk, vec![change.clone()])),
            }
        }
        let effects: Vec<FiledEffect> = from
            .effects
            .iter()
            .filter(|effect| into.held.contains(&effect.at.chunk()))
            .enumerate()
            .map(|(index, effect)| FiledEffect {
                id: EffectId {
                    sequence,
                    index: u32::try_from(index).expect("few"),
                },
                effect: *effect,
                due: Due::FromTaking,
            })
            .collect();
        let arrivals: Vec<Arrival> = from
            .arrivals
            .iter()
            .filter(|(chunk, _)| into.held.contains(chunk))
            .enumerate()
            .map(|(index, (chunk, entity))| Arrival {
                id: EffectId {
                    sequence: sequence + 500_000,
                    index: u32::try_from(index).expect("few"),
                },
                chunk: *chunk,
                entity: entity.clone(),
                due: Due::FromTaking,
            })
            .collect();
        let faces: Vec<FaceChange> = from
            .stays
            .iter()
            .enumerate()
            .filter_map(|(index, note)| match note {
                StayNote::Has {
                    player,
                    entity,
                    hops,
                    face,
                    ..
                } => Some(FaceChange::Face {
                    player: *player,
                    entity: *entity,
                    hops: *hops,
                    seq: FaceSeq(sequence, u32::try_from(index).expect("few")),
                    face: face.clone(),
                }),
                _ => None,
            })
            .collect();
        TickInputs {
            copy_entity_updates: updates,
            effects,
            arrivals,
            faces,
            ..TickInputs::default()
        }
    }

    /// One tick of both, with the players' `west` and `east` inputs.
    fn step(&mut self, west: Vec<PlayerInput>, east: Vec<PlayerInput>) -> (TickOutput, TickOutput) {
        let from_east = std::mem::take(&mut self.from_east);
        let from_west = std::mem::take(&mut self.from_west);
        let west_layout = self.west.layout.clone();
        let east_layout = self.east.layout.clone();
        let mut to_west = self.carried(&from_east, &west_layout);
        let mut to_east = self.carried(&from_west, &east_layout);
        to_west.inputs = numbered(&mut self.west, west);
        to_east.inputs = numbered(&mut self.east, east);
        let west_out = self.west.region.tick(&to_west);
        let east_out = self.east.region.tick(&to_east);
        self.from_west = west_out.clone();
        self.from_east = east_out.clone();
        (west_out, east_out)
    }

    fn idle(&mut self) -> (TickOutput, TickOutput) {
        self.step(Vec::new(), Vec::new())
    }

    /// The mob `key` on whichever side holds it, and whether that is the west.
    fn find(&self, key: EntityId) -> Option<(Entity, bool)> {
        entity_of(&self.west, key)
            .map(|entity| (entity, true))
            .or_else(|| entity_of(&self.east, key).map(|entity| (entity, false)))
    }
}

/// `inputs` of player 1 of `scene` (its only actor), numbered on from their last.
fn numbered(
    scene: &mut Scene,
    inputs: Vec<PlayerInput>,
) -> Vec<(
    clustine_world::EdgeId,
    clustine_world::PlayerId,
    EntityId,
    u64,
    PlayerInput,
)> {
    let actor = &mut scene.actors[0];
    inputs
        .into_iter()
        .map(|input| {
            actor.input += 1;
            (EDGE, actor.id, actor.entity, actor.input, input)
        })
        .collect()
}

/// The inputs of using the item in hotbar `slot` on the top of `at`.
fn egg_on(slot: u8, at: BlockPos) -> Vec<PlayerInput> {
    vec![
        PlayerInput::SelectSlot { slot },
        PlayerInput::UseItemOn {
            position: at,
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence: 1,
        },
    ]
}

/// The mob of `kind` a side has that is not among `known`.
fn new_of(scene: &Scene, kind: i32, known: &[EntityId]) -> EntityId {
    let found = others_of(scene, kind, known);
    assert_eq!(found.len(), 1, "one new {kind}");
    found[0].key
}

/// Plan 3.1 and 11.7.3: an armadillo in the west's land is scared by a player in the
/// east's who sprints within its box (7, 2, 7) beyond the line: the west sees them only
/// by their face, which says whether they sprint. Walking there they scare nothing.
#[test]
fn a_player_sprinting_beyond_the_line_rolls_up_an_armadillo() {
    let world = World::of(&ring(12, 2, 14, 5, 2));
    let mut line = Line::new(
        1,
        &world,
        (Vec3::new(13.5, -60.0, 4.5), &[items::ARMADILLO_SPAWN_EGG]),
        (Vec3::new(18.5, -60.0, 3.5), &[]),
    );
    line.step(egg_on(0, bp(13, -61, 3)), Vec::new());
    let armadillo = new_of(&line.west, entity_types::ARMADILLO, &[]);
    for tick in 0..60 {
        let step = if tick % 2 == 0 { 18.5 } else { 19.0 };
        line.step(Vec::new(), vec![moved_to(Vec3::new(step, -60.0, 3.5))]);
        assert_eq!(
            armadillo_state(&line.west, armadillo),
            IDLE,
            "walking scares nothing"
        );
    }
    line.step(Vec::new(), vec![keys(true)]);
    let mut rolled = false;
    for _ in 0..40 {
        line.idle();
        if armadillo_state(&line.west, armadillo) == ROLLING {
            rolled = true;
            break;
        }
    }
    assert!(rolled, "the sprint beyond the line rolls it up");
}

/// Plan 3.1 and 11.7.3, and the review's item 1: a goat in the west follows a player in
/// the east who holds wheat (seen by their face), crosses the line as an arrival in the
/// middle of its temptation, and in the east keeps following them as they step on: its
/// temptation goes on there, without the cooldown a stopped one would set.
#[test]
fn a_goat_tempted_across_the_line_follows_on_beyond_it() {
    let mut line = Line::new(
        1,
        &World::default(),
        (Vec3::new(9.5, -60.0, 3.5), &[items::GOAT_SPAWN_EGG]),
        (Vec3::new(20.5, -60.0, 3.5), &[items::WHEAT]),
    );
    line.step(egg_on(0, bp(11, -61, 3)), Vec::new());
    let goat = new_of(&line.west, entity_types::GOAT, &[]);
    let near = |line: &Line, reach: f64| -> bool {
        line.find(goat).is_some_and(|(entity, west)| {
            !west && horizontal(feet(&entity), player_feet(&line.east)) < reach
        })
    };
    let mut crossed = false;
    let mut came = false;
    for _ in 0..600 {
        let (west_out, _) = line.idle();
        if west_out
            .arrivals
            .iter()
            .any(|(_, entity)| entity.key == goat)
        {
            crossed = true;
        }
        if near(&line, 3.5) {
            came = true;
            break;
        }
    }
    assert!(crossed, "the goat crossed the line");
    assert!(came, "it came to the player beyond the line");
    for step in 1..=3 {
        line.step(
            Vec::new(),
            vec![moved_to(Vec3::new(
                20.5 + f64::from(step) * 2.0,
                -60.0,
                3.5,
            ))],
        );
    }
    let mut followed = false;
    for _ in 0..60 {
        line.idle();
        if near(&line, 3.5) {
            followed = true;
            break;
        }
    }
    assert!(followed, "it follows on in the east within 60 ticks");
}

/// The breeding effects of a tick: requests (asker, partner).
fn asks(output: &TickOutput) -> Vec<(EntityId, EntityId)> {
    output
        .effects
        .iter()
        .filter_map(|effect: &Effect| match effect.what {
            Effected::OnEntity {
                key,
                by: EntityUse::BreedAsk { asker, .. },
            } => Some((asker, key)),
            _ => None,
        })
        .collect()
}

/// Plan 3.4: two armadillos in love on either side of the line, each seen by the other's
/// region only in a copy, breed once by the regions' protocol: both run `AnimalMakeLove`
/// towards each other (a moat of lava keeps them on their sides, and lets them see each
/// other), the higher key asks across the line, and the lower key's region breeds, as
/// the game's `this`: one baby armadillo, in its region.
#[test]
fn two_armadillos_in_love_across_the_line_breed_once_by_the_protocol() {
    let mut placed = filled(bp(15, -61, -2), bp(15, -61, 9), blocks::LAVA);
    placed.extend(stone_box(bp(15, -62, -2), bp(15, -62, 9)));
    placed.extend(ring(11, -1, 20, 8, 1));
    let world = World::of(&placed);
    for west_block in [1, 2] {
        let mut line = Line::new(
            west_block,
            &world,
            (
                Vec3::new(12.5, -60.0, 1.5),
                &[items::ARMADILLO_SPAWN_EGG, items::SPIDER_EYE],
            ),
            (
                Vec3::new(18.5, -60.0, 1.5),
                &[items::ARMADILLO_SPAWN_EGG, items::SPIDER_EYE],
            ),
        );
        line.step(egg_on(0, bp(14, -61, 3)), egg_on(0, bp(16, -61, 3)));
        let west = new_of(&line.west, entity_types::ARMADILLO, &[]);
        let east = new_of(&line.east, entity_types::ARMADILLO, &[]);
        line.idle();
        line.idle();
        let feed = |key| {
            vec![
                PlayerInput::SelectSlot { slot: 1 },
                used(key),
                PlayerInput::SelectSlot { slot: 5 },
            ]
        };
        let (west_fed, east_fed) = line.step(feed(west), feed(east));
        assert_eq!(
            (
                events_of(std::slice::from_ref(&west_fed), west, HEARTS),
                events_of(std::slice::from_ref(&east_fed), east, HEARTS)
            ),
            (1, 1),
            "both in love"
        );
        let asker = west.max(east);
        let partner = west.min(east);
        let mut seen_asks = Vec::new();
        let mut babies = Vec::new();
        for _ in 0..600 {
            let (west_out, east_out) = line.idle();
            seen_asks.extend(asks(&west_out));
            seen_asks.extend(asks(&east_out));
            babies = others_of(&line.west, entity_types::ARMADILLO, &[west, east])
                .into_iter()
                .map(|baby| (true, baby))
                .chain(
                    others_of(&line.east, entity_types::ARMADILLO, &[west, east])
                        .into_iter()
                        .map(|baby| (false, baby)),
                )
                .collect();
            if !babies.is_empty() {
                break;
            }
        }
        assert_eq!(babies.len(), 1, "block {west_block}: one baby: {babies:?}");
        assert_eq!(
            babies[0].0,
            partner == west,
            "block {west_block}: the baby in the lower key's region"
        );
        assert!(
            !seen_asks.is_empty() && seen_asks.iter().all(|ask| *ask == (asker, partner)),
            "block {west_block}: the higher key asks: {seen_asks:?}"
        );
        for _ in 0..200 {
            line.idle();
        }
        let more = others_of(&line.west, entity_types::ARMADILLO, &[west, east]).len()
            + others_of(&line.east, entity_types::ARMADILLO, &[west, east]).len();
        assert_eq!(more, 1, "block {west_block}: no second baby");
    }
}

/// The mob hits of a tick's effects: (attacker, the entity hit).
fn mob_hits(output: &TickOutput) -> Vec<(EntityId, EntityId)> {
    output
        .effects
        .iter()
        .filter_map(|effect: &Effect| match effect.what {
            Effected::OnEntity {
                key,
                by: EntityUse::MobHit { attacker, .. },
            } => Some((attacker, key)),
            _ => None,
        })
        .collect()
}

/// Plan 3.5: an axolotl in the west's water bites a cod in the east's that it sees only
/// in a copy: the bite goes as a mob's hit (`EntityUse::MobHit`) to the cod's region,
/// which hurts the cod in its next tick, the cause the axolotl. Where the two meet is
/// the draws' (the axolotl may swim over first and bite at home), so a few beginnings
/// are tried until one bites across; every bite across reaches the cod.
#[test]
fn an_axolotls_bite_across_the_line_reaches_the_cod_through_its_region() {
    let mut placed = pool((13, 18), (2, 3), -60);
    placed.extend(stone_box(bp(12, -59, 1), bp(19, -59, 4)));
    let world = World::of(&placed);
    let mut bit_across = false;
    'beginnings: for west_block in [1, 2] {
        for (from, to) in [(15, 16), (14, 16), (15, 17), (14, 17)] {
            let mut line = Line::new(
                west_block,
                &world,
                (Vec3::new(13.5, -60.0, 5.5), &[items::AXOLOTL_SPAWN_EGG]),
                (Vec3::new(18.5, -60.0, 5.5), &[items::COD_SPAWN_EGG]),
            );
            line.step(egg_on(0, bp(from, -61, 2)), egg_on(0, bp(to, -61, 3)));
            let axolotl = new_of(&line.west, entity_types::AXOLOTL, &[]);
            let cod = new_of(&line.east, entity_types::COD, &[]);
            let mut sent = false;
            for _ in 0..600 {
                let (west_out, east_out) = line.idle();
                if sent {
                    assert!(
                        hurts(&east_out, cod).contains(&Some(axolotl)),
                        "the bite sent across hurts the cod in the east's next tick"
                    );
                    bit_across = true;
                    break 'beginnings;
                }
                sent = mob_hits(&west_out).contains(&(axolotl, cod));
                if line.find(cod).is_none() {
                    break;
                }
            }
        }
    }
    assert!(bit_across, "a bite went across the line");
}

/// Plan 3.10 and 11.7.2: a frog that leaps from the west's land onto a ledge in the
/// east's crosses in the air as an arrival with its record whole: `LongJumpMidJump`
/// goes on running in the east, which shows it standing again and plays its landing
/// (`FROG_STEP` at volume 2) when it comes down there, and sets its cooldown, so that
/// it does not leap again within 100 ticks.
#[test]
fn a_frog_leaping_across_the_line_lands_beyond_it_with_its_brain_whole() {
    let mut placed = ring(13, 2, 15, 4, 8);
    // The ring's east side is the ledge, two high, and beyond it open ledge.
    placed.retain(|(at, _)| at.x != 16);
    placed.extend(platform((16, 19), (1, 5), 2));
    let world = World::of(&placed);
    let mut line = Line::new(
        1,
        &world,
        (Vec3::new(11.5, -60.0, 3.5), &[items::FROG_SPAWN_EGG]),
        (Vec3::new(22.5, -60.0, 8.5), &[]),
    );
    line.step(egg_on(0, bp(14, -61, 3)), Vec::new());
    let frog = new_of(&line.west, entity_types::FROG, &[]);
    let mut launched = false;
    let mut arrived = false;
    let mut landed = None;
    for tick in 0..3000 {
        let (west_out, east_out) = line.idle();
        if played(&west_out, sounds::ENTITY_FROG_LONG_JUMP) > 0 {
            launched = true;
        }
        if west_out
            .arrivals
            .iter()
            .any(|(_, entity)| entity.key == frog)
        {
            assert!(launched, "it crosses in a leap");
            arrived = true;
        }
        if arrived && sounds_in(&east_out, sounds::ENTITY_FROG_STEP).contains(&2.0) {
            landed = Some(tick);
            break;
        }
        if launched && !arrived && line.find(frog).is_some_and(|(_, west)| west) {
            // A leap that came down in the west: wait for the next.
            let entity = the_entity(&line.west, frog);
            if entity.on_ground && pose_of(&entity) == STANDING {
                launched = false;
            }
        }
    }
    assert!(arrived, "the frog leaps across the line");
    assert!(
        landed.is_some(),
        "it lands in the east with its landing sound"
    );
    assert_eq!(pose(&line.east, frog), STANDING, "and stands");
    for tick in 0..100 {
        let (_, east_out) = line.idle();
        assert_eq!(
            played(&east_out, sounds::ENTITY_FROG_LONG_JUMP),
            0,
            "no leap {tick} ticks after landing"
        );
    }
}
