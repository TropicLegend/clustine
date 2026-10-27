//! A mob's steps as it walks (`Entity.applyMovementEmissionAndPlaySound`): the walk
//! counted at 0.6 of each move's horizontal length, a step each time the count passes
//! its next whole number (1 at first), its sound the type's own or the block's, and its
//! `STEP` game event with the block stepped on.

use clustine_data::{BlockState, blocks, entity_types, sounds};

use super::{MOB_KEY, Plot, Ticked};
use crate::mobs::body::{self, Event};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::{self, MobType, animal};
use crate::physics::body::{StepCount, Steps};

/// A plot whose ground under x in -1..=1 is grass, the walker's path along z.
fn grass_path() -> Plot {
    let mut plot = Plot::flat();
    for x in -1..=1 {
        for z in -2..80 {
            plot.set(x, -1, z, blocks::GRASS_BLOCK);
        }
    }
    plot
}

/// A walker of `mob_type` standing at the start of the path, its steps counted from
/// none, as after a load.
fn walker(plot: &Plot, mob_type: &'static dyn MobType) -> Ticked {
    let mut ticked = Ticked::new(plot, mob_type, [0.5, 0.0, 0.5]);
    ticked.living.body.steps = Some(Steps::default());
    ticked
}

/// One tick's travel of `ticked` walking ahead (`zza` 1) at `speed`: what it showed
/// and wrote.
fn travel(ticked: &mut Ticked, plot: &Plot, speed: f32) -> MobOutput {
    let mut draws = ticked.stream();
    ticked.mob.body.inputs.zza = 1.0;
    ticked.mob.body.inputs.speed = speed;
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: MOB_KEY,
        kind: entity_types::COW as u16,
        lived: ticked.lived,
        tick_count: ticked.lived,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: false,
        mob: ticked.mob.clone(),
        living: ticked.living.clone(),
        yaw: ticked.yaw,
        pitch: ticked.pitch,
        mob_type: ticked.mob_type,
        world: plot,
        draws: &mut draws,
        out: MobOutput::default(),
        controlled: None,
        controlled_by_mob: false,
    };
    body::travel(&mut tick);
    ticked.mob = tick.mob;
    ticked.living = tick.living;
    ticked.lived += 1;
    ticked.living.body.old_position = ticked.living.body.position;
    tick.out
}

/// The sounds a tick played.
fn sounds_of(out: &MobOutput) -> Vec<(i32, f32, f32)> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound {
                sound,
                volume,
                pitch,
            } => Some((*sound, *volume, *pitch)),
            _ => None,
        })
        .collect()
}

/// The game events a tick posted at points: their names and the blocks they affect.
fn posted(out: &MobOutput) -> Vec<(&'static str, Option<BlockState>)> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::GameEventAt { name, affected, .. } => Some((*name, *affected)),
            _ => None,
        })
        .collect()
}

/// Walks `ticked` for `ticks` ticks, checking each tick's steps against the rule worked
/// out from its moves alone; the ticks it stepped in.
fn walk_and_check(
    ticked: &mut Ticked,
    plot: &Plot,
    ticks: u32,
    sound: (i32, f32, f32),
) -> Vec<u32> {
    let mut count = StepCount::default();
    let mut stepped = Vec::new();
    for tick in 0..ticks {
        let before = ticked.living.body.position;
        let out = travel(ticked, plot, 0.1);
        let after = ticked.living.body.position;
        let (dx, dz) = (after[0] - before[0], after[2] - before[2]);
        count.moved += ((dx * dx + dz * dz).sqrt() * 0.600_000_023_841_857_9) as f32;
        if count.moved > count.next {
            count.next = (count.moved as i32 + 1) as f32;
            stepped.push(tick);
            assert_eq!(sounds_of(&out), [sound], "tick {tick}");
            assert_eq!(
                posted(&out),
                [("minecraft:step", Some(blocks::GRASS_BLOCK))],
                "tick {tick}"
            );
        } else {
            assert_eq!(sounds_of(&out), [], "tick {tick}");
            assert_eq!(posted(&out), [], "tick {tick}");
        }
        assert_eq!(
            ticked.living.body.steps.as_ref().map(|steps| steps.count),
            Some(count),
            "tick {tick}"
        );
    }
    stepped
}

/// A cow walking on grass steps with its own sound at 0.15 and 1 (`AbstractCow`), the
/// first once its walk passes 1 (`nextStep` starts at 1) and then at each whole number:
/// a step every 1/0.6 blocks.
#[test]
fn a_cow_walking_on_grass_plays_its_own_step_sound_every_five_thirds_of_a_block() {
    let plot = grass_path();
    let mut cow = walker(&plot, &types::cow::Cow);
    let stepped = walk_and_check(&mut cow, &plot, 200, (sounds::ENTITY_COW_STEP, 0.15, 1.0));
    let walked = cow.living.body.position[2] - 0.5;
    assert!(walked > 20.0, "it walked {walked}");
    // One step for each whole number its count passed but the first, give or take the
    // one it is about to pass.
    let count = (walked * 0.6) as usize;
    assert!(
        stepped.len() == count || stepped.len() + 1 == count,
        "{} steps in {walked} blocks",
        stepped.len()
    );
}

/// A moody cow steps with its sound variant's step.
#[test]
fn a_moody_cow_steps_with_the_moody_step_sound() {
    let plot = grass_path();
    let mut cow = walker(&plot, &types::cow::Cow);
    let own = types::cow::CowState {
        sound: animal::variants::id("minecraft:cow_sound_variant", "minecraft:moody"),
        ..Default::default()
    };
    cow.mob.state = animal::write(&animal::Animal::default(), &own);
    let stepped = walk_and_check(
        &mut cow,
        &plot,
        60,
        (sounds::ENTITY_COW_MOODY_STEP, 0.15, 1.0),
    );
    assert!(!stepped.is_empty());
}

/// A mob of a type without a step of its own (`types::Plain`) plays the block's: grass's
/// step at 0.15 of grass's volume, at grass's pitch.
#[test]
fn a_mob_without_its_own_step_sound_plays_the_block_s_at_fifteen_hundredths_of_its_volume() {
    let plot = grass_path();
    let mut mob = walker(&plot, &types::Plain);
    let grass = blocks::GRASS_BLOCK.sound_type().expect("grass sounds");
    assert_eq!(grass.step, sounds::BLOCK_GRASS_STEP);
    let stepped = walk_and_check(
        &mut mob,
        &plot,
        60,
        (grass.step, grass.volume * 0.15, grass.pitch),
    );
    assert!(!stepped.is_empty());
}

/// A mob moving through the air counts its walk but steps on nothing.
#[test]
fn nothing_plays_in_the_air() {
    let plot = grass_path();
    let mut cow = walker(&plot, &types::cow::Cow);
    cow.living.body.position = [0.5, 40.0, 0.5];
    cow.living.body.on_ground = false;
    for tick in 0..30 {
        cow.living.body.motion[2] = 0.4;
        let out = travel(&mut cow, &plot, 0.1);
        assert!(!cow.living.body.on_ground, "tick {tick}");
        assert_eq!(sounds_of(&out), [], "tick {tick}");
        assert_eq!(posted(&out), [], "tick {tick}");
    }
    let count = cow.living.body.steps.as_ref().expect("steps").count;
    assert!(count.moved > 5.0, "{count:?}");
    assert_eq!(count.next, 1.0);
}

/// A body without a count (a mob riding something) makes no step.
#[test]
fn a_body_without_steps_makes_none() {
    let plot = grass_path();
    let mut cow = walker(&plot, &types::cow::Cow);
    cow.living.body.steps = None;
    for _ in 0..60 {
        let out = travel(&mut cow, &plot, 0.1);
        assert_eq!(sounds_of(&out), []);
        assert_eq!(posted(&out), []);
    }
    assert!(cow.living.body.position[2] > 5.0);
}
