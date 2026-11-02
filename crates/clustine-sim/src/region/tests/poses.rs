//! A player's pose, swimming, gliding and appearance as the others are shown them
//! (`region/poses.rs`): `Player.updatePlayerPose` at the end of the player's tick,
//! `Entity.updateSwimming`, `Player.tryToStartFallFlying` and
//! `LivingEntity.updateFallFlying`, and `ServerPlayer.updateOptions`.

use super::*;
use crate::api::{Appearance, Keys};
use crate::shown::{DataShown, Serialiser};
use crate::state::{PlayerPose, Posture};

const SNEAKING: u8 = 1 << 1;
const SPRINTING: u8 = 1 << 3;
const SWIMMING: u8 = 1 << 4;
const FALL_FLYING: u8 = 1 << 7;

/// The chest's slot of the inventory.
const CHEST: u8 = 38;

/// What the others were shown of player 1 in the tick: each report's values.
fn shown(output: &TickOutput) -> Vec<Vec<(u8, DataShown)>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::PlayerData { values, .. } => Some(values.clone()),
            _ => None,
        })
        .collect()
}

fn flags(flags: u8) -> (u8, DataShown) {
    (0, DataShown::Byte(flags))
}

fn pose(pose: PlayerPose) -> (u8, DataShown) {
    (6, DataShown::Pose(pose.ordinal()))
}

fn keys(keys: Keys) -> TickInputs {
    moves(vec![numbered(1, PlayerInput::Keys(keys))])
}

fn sneak(sneak: bool) -> TickInputs {
    keys(Keys {
        sneak,
        ..Keys::default()
    })
}

fn sprint(sprint: bool) -> TickInputs {
    keys(Keys {
        sprint,
        ..Keys::default()
    })
}

/// Player 1 at `x`, `y`, `z`, on the ground or not.
fn to(x: f64, y: f64, z: f64, on_ground: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(x, y, z)),
            rotation: None,
            on_ground,
        },
    )])
}

fn input(input: PlayerInput) -> TickInputs {
    moves(vec![numbered(1, input)])
}

fn posture(region: &Region) -> Posture {
    region.players[&player(1)].posture
}

fn data_of(region: &Region) -> Vec<(u8, DataShown)> {
    match region.entity(EntityId(1)).map(|state| state.kind) {
        Some(EntityKind::Player { data, .. }) => data,
        other => panic!("player 1 is {other:?}"),
    }
}

#[test]
fn a_player_who_sneaks_is_shown_the_shift_key_s_flag_and_then_crouching() {
    let mut region = on_ticking_floor_with(&[]);
    let output = region.tick(&sneak(true));
    // The flag as the key is told, the pose at the end of the player's tick: two
    // reports, as the game sends them a tick apart.
    assert_eq!(
        shown(&output),
        [vec![flags(SNEAKING)], vec![pose(PlayerPose::Crouching)]]
    );
    // Nothing more while they keep sneaking.
    assert!(shown(&region.tick(&TickInputs::default())).is_empty());
    let output = region.tick(&sneak(false));
    assert_eq!(
        shown(&output),
        [vec![flags(0)], vec![pose(PlayerPose::Standing)]]
    );
}

#[test]
fn a_flying_player_who_sneaks_stays_standing() {
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&input(PlayerInput::SetFlying { flying: true }));
    let output = region.tick(&sneak(true));
    assert!(
        shown(&output)
            .iter()
            .all(|values| values.iter().all(|(index, _)| *index != 6)),
        "{:?}",
        shown(&output)
    );
    assert_eq!(posture(&region).pose, PlayerPose::Standing);
}

#[test]
fn a_player_under_a_ceiling_a_block_and_a_half_high_crouches() {
    let top_slab = blocks::STONE_SLAB.with_property("type", "top").unwrap();
    let mut region = on_ticking_floor_with(&[(BlockPos::new(5, -59, 5), top_slab)]);
    let output = region.tick(&to(5.5, -60.0, 5.5, true));
    assert_eq!(shown(&output), [vec![pose(PlayerPose::Crouching)]]);
    // Out from under it they stand.
    let output = region.tick(&to(8.5, -60.0, 8.5, true));
    assert_eq!(shown(&output), [vec![pose(PlayerPose::Standing)]]);
}

#[test]
fn a_player_in_a_gap_a_block_high_crawls() {
    let mut region = on_ticking_floor_with(&[(BlockPos::new(5, -59, 5), blocks::STONE)]);
    let output = region.tick(&to(5.5, -60.0, 5.5, true));
    // The swimming pose, without the swimming flag.
    assert_eq!(shown(&output), [vec![pose(PlayerPose::Swimming)]]);
    assert!(!posture(&region).swimming);
}

/// A pool two blocks deep at (5, 5), from the floor up.
fn pool() -> Region {
    on_ticking_floor_with(&[
        (BlockPos::new(5, -60, 5), blocks::WATER),
        (BlockPos::new(5, -59, 5), blocks::WATER),
    ])
}

#[test]
fn a_player_who_sprints_with_the_eyes_under_water_swims_until_they_stop_sprinting() {
    let mut region = pool();
    // In the pool, the eyes under water: nothing to show while they do not sprint.
    let output = region.tick(&to(5.5, -60.0, 5.5, true));
    assert!(shown(&output).is_empty());
    let output = region.tick(&sprint(true));
    assert_eq!(
        shown(&output),
        [
            vec![flags(SPRINTING)],
            vec![flags(SPRINTING | SWIMMING), pose(PlayerPose::Swimming)],
        ]
    );
    assert!(shown(&region.tick(&TickInputs::default())).is_empty());
    let output = region.tick(&sprint(false));
    assert_eq!(
        shown(&output),
        [
            vec![flags(SWIMMING)],
            vec![flags(0), pose(PlayerPose::Standing)],
        ]
    );
}

#[test]
fn a_swim_begins_with_the_eyes_of_the_tick_before_and_goes_on_in_shallow_water() {
    let mut region = pool();
    // Sprinting into the pool: the eyes are found under water at the end of the tick,
    // and the swim begins a tick later.
    region.tick(&sprint(true));
    let output = region.tick(&to(5.5, -60.0, 5.5, true));
    assert!(shown(&output).is_empty(), "{:?}", shown(&output));
    let output = region.tick(&TickInputs::default());
    assert_eq!(
        shown(&output),
        [vec![
            flags(SPRINTING | SWIMMING),
            pose(PlayerPose::Swimming)
        ]]
    );
    // Up at the top of the pool, the eyes out of the water and the box in it: a swim
    // goes on, where none would begin.
    let output = region.tick(&to(5.5, -59.3, 5.5, false));
    assert!(shown(&output).is_empty(), "{:?}", shown(&output));
    assert!(posture(&region).swimming);
}

#[test]
fn a_player_who_sprints_in_water_to_the_waist_does_not_swim() {
    let mut region = on_ticking_floor_with(&[(BlockPos::new(5, -60, 5), blocks::WATER)]);
    region.tick(&to(5.5, -60.0, 5.5, true));
    region.tick(&sprint(true));
    region.tick(&TickInputs::default());
    assert!(!posture(&region).swimming);
}

/// Player 1 with an elytra on, up in the air.
fn aloft() -> Region {
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&input(PlayerInput::SetInventorySlot {
        slot: CHEST,
        stack: Some(ItemStack::new(items::ELYTRA, 1)),
    }));
    region.tick(&to(5.5, -50.0, 5.5, false));
    region
}

#[test]
fn a_player_glides_with_an_elytra_in_the_air_until_they_land() {
    let mut region = aloft();
    let output = region.tick(&input(PlayerInput::StartFallFlying));
    assert_eq!(
        shown(&output),
        [vec![flags(FALL_FLYING)], vec![pose(PlayerPose::FallFlying)]]
    );
    // On through the air.
    let output = region.tick(&to(9.5, -52.0, 5.5, false));
    assert!(shown(&output).is_empty());
    // On the ground the glide ends in the player's tick, with its pose.
    let output = region.tick(&to(12.5, -60.0, 5.5, true));
    assert_eq!(shown(&output), [vec![flags(0), pose(PlayerPose::Standing)]]);
}

#[test]
fn a_glide_is_refused_on_the_ground_without_an_elytra_and_while_flying() {
    // On the ground: the flag is sent as it is, as `stopFallFlying` sends it.
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&input(PlayerInput::SetInventorySlot {
        slot: CHEST,
        stack: Some(ItemStack::new(items::ELYTRA, 1)),
    }));
    region.tick(&to(0.5, -60.0, 0.5, true));
    let output = region.tick(&input(PlayerInput::StartFallFlying));
    assert_eq!(shown(&output), [vec![flags(0)]]);
    assert!(!posture(&region).fall_flying);
    // In the air without one.
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&to(5.5, -50.0, 5.5, false));
    region.tick(&input(PlayerInput::StartFallFlying));
    assert!(!posture(&region).fall_flying);
    // Flying.
    let mut region = aloft();
    region.tick(&input(PlayerInput::SetFlying { flying: true }));
    region.tick(&input(PlayerInput::StartFallFlying));
    assert!(!posture(&region).fall_flying);
}

#[test]
fn a_second_start_of_gliding_ends_the_glide() {
    let mut region = aloft();
    region.tick(&input(PlayerInput::StartFallFlying));
    let output = region.tick(&input(PlayerInput::StartFallFlying));
    assert_eq!(
        shown(&output),
        [vec![flags(0)], vec![pose(PlayerPose::Standing)]]
    );
}

#[test]
fn a_glide_ends_when_the_elytra_comes_off() {
    let mut region = aloft();
    region.tick(&input(PlayerInput::StartFallFlying));
    let output = region.tick(&input(PlayerInput::SetInventorySlot {
        slot: CHEST,
        stack: None,
    }));
    assert_eq!(shown(&output), [vec![flags(0), pose(PlayerPose::Standing)]]);
}

#[test]
fn the_tenth_tick_of_a_glide_is_heard_by_a_sculk_sensor() {
    // A sensor on the floor eight blocks under the glide.
    let sensor = super::redstone::state(
        "minecraft:sculk_sensor[power=0,sculk_sensor_phase=inactive,waterlogged=false]",
    );
    let mut region = super::redstone::ticking_with(&[(BlockPos::new(5, -60, 5), sensor)]);
    region.tick(&input(PlayerInput::SetInventorySlot {
        slot: CHEST,
        stack: Some(ItemStack::new(items::ELYTRA, 1)),
    }));
    region.tick(&to(5.5, -52.0, 5.5, false));
    // Whatever going up made heard has gone by.
    for _ in 0..40 {
        region.tick(&TickInputs::default());
    }
    region.tick(&input(PlayerInput::StartFallFlying));
    // The start's tick is the glide's first; its tenth posts the vibration, which the
    // sensor takes up the tick after.
    let mut heard = None;
    for tick in 2..=12 {
        let output = region.tick(&TickInputs::default());
        let from: Vec<[f64; 3]> = output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::Vibration { from, .. } => Some(*from),
                _ => None,
            })
            .collect();
        if !from.is_empty() {
            heard = Some((tick, from));
            break;
        }
    }
    assert_eq!(heard, Some((11, vec![[5.5, -52.0, 5.5]])));
}

#[test]
fn the_skin_s_parts_and_the_main_hand_are_shown_where_they_change() {
    let mut region = on_ticking_floor_with(&[]);
    let appearance = Appearance {
        skin_parts: 0x55,
        left_handed: true,
    };
    let output = region.tick(&input(PlayerInput::Appearance(appearance)));
    assert_eq!(
        shown(&output),
        [vec![
            (15, DataShown::Serialised(Serialiser::HumanoidArm, 0)),
            (16, DataShown::Byte(0x55)),
        ]]
    );
    // The same again is nothing new.
    let output = region.tick(&input(PlayerInput::Appearance(appearance)));
    assert!(shown(&output).is_empty());
    let output = region.tick(&input(PlayerInput::Appearance(Appearance {
        skin_parts: 0x55,
        left_handed: false,
    })));
    assert_eq!(
        shown(&output),
        [vec![(
            15,
            DataShown::Serialised(Serialiser::HumanoidArm, 1)
        )]]
    );
}

#[test]
fn a_join_s_appearance_and_a_player_s_pose_go_with_every_report_of_the_player() {
    let mut region = region();
    region.tick(&changes(vec![PlayerChange::Join(
        EDGE,
        PlayerJoin {
            player: player(1),
            name: "Player1".to_owned(),
            attempt: attempt(1),
            appearance: Appearance {
                skin_parts: 0x7F,
                left_handed: true,
            },
        },
    )]));
    let output = enter(&mut region);
    let spawned = output
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::EntitySpawned(EntityState {
                kind: EntityKind::Player { data, .. },
                ..
            }) => Some(data.clone()),
            _ => None,
        })
        .expect("the player is shown entering");
    // Only what is not the default: the left hand and the skin's parts.
    assert_eq!(
        spawned,
        [
            (15, DataShown::Serialised(Serialiser::HumanoidArm, 0)),
            (16, DataShown::Byte(0x7F)),
        ]
    );
    region.tick(&sneak(true));
    assert_eq!(
        data_of(&region),
        [
            flags(SNEAKING),
            pose(PlayerPose::Crouching),
            (15, DataShown::Serialised(Serialiser::HumanoidArm, 0)),
            (16, DataShown::Byte(0x7F)),
        ]
    );
}

#[test]
fn a_glide_and_the_appearance_go_with_a_player_who_is_handed_over() {
    let mut arriving = transfer(2, 40, 5.5, 3);
    arriving.pose.position = Vec3::new(5.5, -50.0, 0.5);
    arriving.pose.on_ground = false;
    arriving.inventory.0[usize::from(CHEST)] = Some(ItemStack::new(items::ELYTRA, 1));
    arriving.appearance = Appearance {
        skin_parts: 0x01,
        left_handed: false,
    };
    arriving.posture = Posture {
        pose: PlayerPose::FallFlying,
        swimming: false,
        fall_flying: true,
    };
    let mut region = on_ticking_floor_with(&[]);
    let output = region.tick(&changes(vec![arrive(2, &arriving)]));
    let spawned = output
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::EntitySpawned(EntityState {
                kind: EntityKind::Player { data, .. },
                ..
            }) => Some(data.clone()),
            _ => None,
        })
        .expect("the arrival is shown");
    assert_eq!(
        spawned,
        [
            flags(FALL_FLYING),
            pose(PlayerPose::FallFlying),
            (16, DataShown::Byte(0x01)),
        ]
    );
    // Gliding on, nothing changes to be shown.
    assert!(shown(&output).is_empty(), "{:?}", shown(&output));
    assert_eq!(region.players[&player(2)].posture, arriving.posture);
    // And it is kept in the region's state as in a transfer.
    let state = region.state();
    let kept = (state.players.get(&player(2))).map(|stay| (stay.appearance, stay.posture));
    assert_eq!(kept, Some((arriving.appearance, arriving.posture)));
}

#[test]
fn a_sleeper_s_pose_is_shown_by_the_sleep_and_not_again() {
    // A red bed beside player 1, at night, as `sleep.rs`'s tests have it; the player
    // sneaks as they lie down.
    let (head, foot) = (BlockPos::new(2, -60, 0), BlockPos::new(2, -60, 1));
    let bed = |part: &str| {
        BlockState::parse(&format!("minecraft:red_bed[facing=north,part={part}]")).unwrap()
    };
    let mut region = on_ticking_floor_with(&[(head, bed("head")), (foot, bed("foot"))]);
    region.clock.day = 13_000;
    if let Some(player) = region.players.get_mut(&player(1)) {
        player.pose.position = Vec3::new(4.5, -60.0, 0.5);
        player.pose.yaw = 90.0;
    }
    region.tick(&sneak(true));
    let output = region.tick(&moves(vec![place(1, foot.x, foot.y, foot.z, Face::Top)]));
    assert!(region.players[&player(1)].sleeping.is_some());
    // Lying down says the pose with the bed (`sleep.rs`): the player's tick finds it so
    // and reports nothing of it.
    let posed = |output: &TickOutput| -> Vec<Vec<(u8, DataShown)>> {
        shown(output)
            .into_iter()
            .filter(|values| values.iter().any(|(index, _)| *index == 6))
            .collect()
    };
    assert!(posed(&output).is_empty(), "{:?}", shown(&output));
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::Sleeping { head: Some(_), .. }))
    );
    let data = data_of(&region);
    assert!(data.contains(&pose(PlayerPose::Sleeping)), "{data:?}");
    assert!(data.contains(&(14, DataShown::OptionalBlockPos(Some(head)))));
    // Getting up stands them up, which the waking says; still sneaking, their tick
    // crouches them.
    let output = region.tick(&input(PlayerInput::LeaveBed));
    assert_eq!(posed(&output), [vec![pose(PlayerPose::Crouching)]]);
}
