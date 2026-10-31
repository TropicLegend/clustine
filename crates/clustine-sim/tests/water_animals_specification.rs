//! Tests of the water creatures and the turtle of step N7 (part N7i), written from the
//! specification and the public API alone, by someone who did not write them: the
//! porting brief `docs/groundwork/n7-brief.md` (sections 0, 6.0 and 6.5 to 6.10, 9.6 and
//! 10.6), the plan `docs/groundwork/n7-plan.md` (section 11) and the game's bytecode
//! (`reference/n7-javap/`: `WaterAnimal`, `AgeableWaterCreature`, `AbstractFish`,
//! `Salmon`, `Pufferfish`, `TropicalFish`, `Squid`, `GlowSquid`, `Dolphin`, `Turtle` and
//! their goals, `Bucketable`, `MobBucketItem`, `TurtleEggBlock`). Where the brief and the
//! bytecode differ, the bytecode is followed and the test says so.
//!
//! A region is driven as a player drives it: spawn eggs used on the floor of a pool or of
//! dry land, buckets and items used on the creatures, hits, ticks; the world's clock is
//! published as the store would publish it. What the game keeps of a creature that no
//! client is shown is read from what it does.
//!
//! Every player is in creative mode, as Clustine's players are: fish do not flee one and
//! a pufferfish does not puff at one (brief W6), so the pufferfish is shown a cow.
//!
//! Player 1 stands on the stone floor at y = -61 (feet at y = -60) at the spawn.

mod n6_entities;
mod n6_scene;

use clustine_data::{BlockState, SYNCED_REGISTRIES, blocks, entity_types, items, sounds};
use clustine_sim::api::{Face, PlayerInput, RegionEvent, Tracked, WorldClock};
use clustine_sim::shown::{DataShown, index, mob_values};
use clustine_sim::{TickInputs, TickOutput};
use clustine_world::{BlockPos, Entity, EntityId, ItemStack};
use n6_scene::*;

// ------------------------------------------------------------------------------------
// The creatures as the game makes them.

/// A water creature as the bytecode has it: its entity type, its egg, its `max_health`
/// (`AbstractFish.createAttributes` 3, `Squid.createAttributes` 10, the dolphin's 10, the
/// turtle's 30), its bucket if it is `Bucketable`, and the synced values of its own
/// beyond `Mob`'s (index, default), as brief 9.6.3 has them.
struct Kind {
    name: &'static str,
    kind: i32,
    egg: i32,
    health: f32,
    bucket: Option<i32>,
    own: &'static [(u8, DataShown)],
}

const COD: Kind = Kind {
    name: "cod",
    kind: entity_types::COD,
    egg: items::COD_SPAWN_EGG,
    health: 3.0,
    bucket: Some(items::COD_BUCKET),
    own: &[(FROM_BUCKET, DataShown::Boolean(false))],
};

const SALMON: Kind = Kind {
    name: "salmon",
    kind: entity_types::SALMON,
    egg: items::SALMON_SPAWN_EGG,
    health: 3.0,
    bucket: Some(items::SALMON_BUCKET),
    own: &[
        (FROM_BUCKET, DataShown::Boolean(false)),
        // `Salmon.DATA_TYPE`, medium by default.
        (17, DataShown::Int(1)),
    ],
};

const PUFFERFISH: Kind = Kind {
    name: "pufferfish",
    kind: entity_types::PUFFERFISH,
    egg: items::PUFFERFISH_SPAWN_EGG,
    health: 3.0,
    bucket: Some(items::PUFFERFISH_BUCKET),
    own: &[
        (FROM_BUCKET, DataShown::Boolean(false)),
        (PUFF_STATE, DataShown::Int(0)),
    ],
};

const TROPICAL_FISH: Kind = Kind {
    name: "tropical fish",
    kind: entity_types::TROPICAL_FISH,
    egg: items::TROPICAL_FISH_SPAWN_EGG,
    health: 3.0,
    bucket: Some(items::TROPICAL_FISH_BUCKET),
    own: &[
        (FROM_BUCKET, DataShown::Boolean(false)),
        // `DATA_ID_TYPE_VARIANT`, the packed kob, white, white.
        (17, DataShown::Int(0)),
    ],
};

const SQUID: Kind = Kind {
    name: "squid",
    kind: entity_types::SQUID,
    egg: items::SQUID_SPAWN_EGG,
    health: 10.0,
    bucket: None,
    own: &[
        (index::BABY, DataShown::Boolean(false)),
        (AGE_LOCKED, DataShown::Boolean(false)),
    ],
};

const GLOW_SQUID: Kind = Kind {
    name: "glow squid",
    kind: entity_types::GLOW_SQUID,
    egg: items::GLOW_SQUID_SPAWN_EGG,
    health: 10.0,
    bucket: None,
    own: &[
        (index::BABY, DataShown::Boolean(false)),
        (AGE_LOCKED, DataShown::Boolean(false)),
        (DARK_TICKS, DataShown::Int(0)),
    ],
};

const DOLPHIN: Kind = Kind {
    name: "dolphin",
    kind: entity_types::DOLPHIN,
    egg: items::DOLPHIN_SPAWN_EGG,
    health: 10.0,
    bucket: None,
    own: &[
        (index::BABY, DataShown::Boolean(false)),
        (AGE_LOCKED, DataShown::Boolean(false)),
        (GOT_FISH, DataShown::Boolean(false)),
        (MOISTNESS, DataShown::Int(2400)),
    ],
};

const TURTLE: Kind = Kind {
    name: "turtle",
    kind: entity_types::TURTLE,
    egg: items::TURTLE_SPAWN_EGG,
    health: 30.0,
    bucket: None,
    own: &[
        (index::BABY, DataShown::Boolean(false)),
        (AGE_LOCKED, DataShown::Boolean(false)),
        (HAS_EGG, DataShown::Boolean(false)),
        (LAYING_EGG, DataShown::Boolean(false)),
    ],
};

const FISH: [&Kind; 4] = [&COD, &SALMON, &PUFFERFISH, &TROPICAL_FISH];
const SWIMMERS: [&Kind; 7] = [
    &COD,
    &SALMON,
    &PUFFERFISH,
    &TROPICAL_FISH,
    &SQUID,
    &GLOW_SQUID,
    &DOLPHIN,
];
const ALL: [&Kind; 8] = [
    &COD,
    &SALMON,
    &PUFFERFISH,
    &TROPICAL_FISH,
    &SQUID,
    &GLOW_SQUID,
    &DOLPHIN,
    &TURTLE,
];

/// `AbstractFish.FROM_BUCKET`.
const FROM_BUCKET: u8 = 16;
/// `AgeableMob.AGE_LOCKED`.
const AGE_LOCKED: u8 = 17;
/// `Pufferfish.PUFF_STATE`.
const PUFF_STATE: u8 = 17;
/// `GlowSquid.DATA_DARK_TICKS_REMAINING`.
const DARK_TICKS: u8 = 18;
/// `Dolphin.GOT_FISH` and `MOISTNESS_LEVEL`.
const GOT_FISH: u8 = 18;
const MOISTNESS: u8 = 19;
/// `Turtle.HAS_EGG` and `LAYING_EGG`.
const HAS_EGG: u8 = 18;
const LAYING_EGG: u8 = 19;

/// The squid's stroke (`Squid.aiStep`, `broadcastEntityEvent(this, 19)`).
const STROKE: u8 = 19;
/// The bubbles before a drowning hit of `LivingEntity.baseTick`.
const BUBBLES: u8 = 67;
/// The hearts of `Animal.setInLove`.
const HEARTS: u8 = 18;

/// The 22 `TropicalFish.COMMON_VARIANTS` (TropicalFish.txt static block, read again from
/// the bytecode for these tests), packed as `packVariant` packs them: the pattern's
/// `base | index << 8`, the base colour `<< 16`, the pattern colour `<< 24`.
const COMMON_VARIANTS: [(u32, u32, u32); 22] = [
    (STRIPEY, ORANGE, GRAY),
    (FLOPPER, GRAY, GRAY),
    (FLOPPER, GRAY, BLUE),
    (CLAYFISH, WHITE, GRAY),
    (SUNSTREAK, BLUE, GRAY),
    (KOB, ORANGE, WHITE),
    (SPOTTY, PINK, LIGHT_BLUE),
    (BLOCKFISH, PURPLE, YELLOW),
    (CLAYFISH, WHITE, RED),
    (SPOTTY, WHITE, YELLOW),
    (GLITTER, WHITE, GRAY),
    (CLAYFISH, WHITE, ORANGE),
    (DASHER, CYAN, PINK),
    (BRINELY, LIME, LIGHT_BLUE),
    (BETTY, RED, WHITE),
    (SNOOPER, GRAY, RED),
    (BLOCKFISH, RED, WHITE),
    (FLOPPER, WHITE, YELLOW),
    (KOB, RED, WHITE),
    (SUNSTREAK, GRAY, WHITE),
    (DASHER, CYAN, YELLOW),
    (FLOPPER, YELLOW, YELLOW),
];
const KOB: u32 = 0x000;
const SUNSTREAK: u32 = 0x100;
const SNOOPER: u32 = 0x200;
const DASHER: u32 = 0x300;
const BRINELY: u32 = 0x400;
const SPOTTY: u32 = 0x500;
const FLOPPER: u32 = 0x001;
const STRIPEY: u32 = 0x101;
const GLITTER: u32 = 0x201;
const BLOCKFISH: u32 = 0x301;
const BETTY: u32 = 0x401;
const CLAYFISH: u32 = 0x501;
const PATTERNS: [u32; 12] = [
    KOB, SUNSTREAK, SNOOPER, DASHER, BRINELY, SPOTTY, FLOPPER, STRIPEY, GLITTER, BLOCKFISH, BETTY,
    CLAYFISH,
];
const WHITE: u32 = 0;
const ORANGE: u32 = 1;
const LIGHT_BLUE: u32 = 3;
const YELLOW: u32 = 4;
const LIME: u32 = 5;
const PINK: u32 = 6;
const GRAY: u32 = 7;
const CYAN: u32 = 9;
const PURPLE: u32 = 10;
const BLUE: u32 = 11;
const RED: u32 = 14;

fn packed(pattern: u32, base: u32, colour: u32) -> i32 {
    ((pattern & 0xFFFF) | (base & 255) << 16 | (colour & 255) << 24) as i32
}

/// The id of the damage type `name` in the registry the server sends, which a damage
/// event names.
fn damage_type(name: &str) -> i32 {
    let types = SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == "minecraft:damage_type")
        .expect("the damage type registry");
    let at = types
        .entries
        .iter()
        .position(|entry| *entry == name)
        .unwrap_or_else(|| panic!("{name} is a damage type"));
    i32::try_from(at).expect("few")
}

// ------------------------------------------------------------------------------------
// The places.

/// The pool: water inside x 2..=9, z -4..=4, four deep (y -60 to -57, its surface the top
/// of y = -57), in a stone rim as high as the water, beside the player.
const POOL_X: (i32, i32) = (2, 9);
const POOL_Z: (i32, i32) = (-4, 4);
const POOL_Y: (i32, i32) = (-60, -57);

fn pool() -> Vec<(BlockPos, BlockState)> {
    pool_of(POOL_X, POOL_Z, POOL_Y, false)
}

/// [`pool`] with its rim raised three blocks above the water: a dolphin's jump
/// (`DolphinJumpGoal`, which asks only for seven blocks of water ahead, while the jump
/// and the speed it had carry it up to some nine) cannot take it out over the rim.
fn walled_pool() -> Vec<(BlockPos, BlockState)> {
    let mut placed = pool();
    for y in POOL_Y.1 + 1..=POOL_Y.1 + 3 {
        for x in POOL_X.0 - 1..=POOL_X.1 + 1 {
            for z in POOL_Z.0 - 1..=POOL_Z.1 + 1 {
                let inside =
                    (POOL_X.0..=POOL_X.1).contains(&x) && (POOL_Z.0..=POOL_Z.1).contains(&z);
                if !inside {
                    placed.push((bp(x, y, z), blocks::STONE));
                }
            }
        }
    }
    placed
}

/// Sand at `at`, on stone: sand with air below falls at its first block update (an egg
/// cracking above it is one), as the game's `FallingBlock` does.
fn sand_at(at: BlockPos) -> [(BlockPos, BlockState); 2] {
    [(at, blocks::SAND), (at.offset(0, -1, 0), blocks::STONE)]
}

/// Water inside `x` and `z` from `y.0` to `y.1`, in a stone rim as high, under a stone
/// roof if `roofed`.
fn pool_of(
    x: (i32, i32),
    z: (i32, i32),
    y: (i32, i32),
    roofed: bool,
) -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for at_y in y.0..=y.1 {
        for at_x in x.0 - 1..=x.1 + 1 {
            for at_z in z.0 - 1..=z.1 + 1 {
                let inside = (x.0..=x.1).contains(&at_x) && (z.0..=z.1).contains(&at_z);
                let state = if inside { blocks::WATER } else { blocks::STONE };
                placed.push((bp(at_x, at_y, at_z), state));
            }
        }
    }
    if roofed {
        for at_x in x.0 - 1..=x.1 + 1 {
            for at_z in z.0 - 1..=z.1 + 1 {
                placed.push((bp(at_x, y.1 + 1, at_z), blocks::STONE));
            }
        }
    }
    placed
}

/// A cell of one block of water at `CELL` under a stone roof, walled in stone: whatever
/// is made in it stays within the player's reach.
const CELL: (i32, i32) = (2, 0);

fn cell() -> Vec<(BlockPos, BlockState)> {
    pool_of((CELL.0, CELL.0), (CELL.1, CELL.1), (-60, -60), true)
}

/// A dry cell at `CELL`, walled in stone `high` blocks high, open above.
fn dry_cell(high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut walls = Vec::new();
    for y in -60..-60 + high {
        for (dx, dz) in [
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, 1),
            (-1, 1),
            (1, -1),
        ] {
            walls.push((bp(CELL.0 + dx, y, CELL.1 + dz), blocks::STONE));
        }
    }
    walls
}

/// A dry room of 5 by 5 around the cell's block, walled `high` up: room for a large
/// salmon (1.05 wide, its eyes' box 0.84) to flop about without its eyes in a wall.
fn dry_room(high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut walls = Vec::new();
    for y in -60..-60 + high {
        for dx in -3_i32..=3 {
            for dz in -3_i32..=3 {
                if dx.abs() == 3 || dz.abs() == 3 {
                    walls.push((bp(CELL.0 + dx, y, CELL.1 + dz), blocks::STONE));
                }
            }
        }
    }
    walls
}

/// The floor's block whose top an egg is used on to make a mob at (x + 0.5, -60, z + 0.5).
fn floor_at(x: i32, z: i32) -> BlockPos {
    bp(x, -61, z)
}

/// A scene of one region with `placed` in its world and player 1 at the spawn holding
/// `slots` in the hotbar, one of each.
fn scene_with(placed: &[(BlockPos, BlockState)], slots: &[i32]) -> Scene {
    let stacks: Vec<(usize, ItemStack)> = slots
        .iter()
        .enumerate()
        .map(|(slot, item)| (slot, stack(*item, 1)))
        .collect();
    Scene::alone(&World::of(placed), inventory(&stacks), SPAWN)
}

// ------------------------------------------------------------------------------------
// Driving a scene and reading it.

/// Player 1 selects hotbar `slot` and uses what it holds on the top of `at`: the one
/// entity it made, if any.
fn hatch(scene: &mut Scene, slot: u8, at: BlockPos) -> Option<EntityId> {
    hatch_on(scene, slot, at, Face::Top)
}

fn hatch_on(scene: &mut Scene, slot: u8, at: BlockPos, face: Face) -> Option<EntityId> {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(slot);
    scene.click(at, face);
    let made: Vec<EntityId> = scene
        .all()
        .iter()
        // Not the loot or orbs of a mob that died in the tick (fish crammed into a cell).
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
    scene.input(PlayerInput::Attack {
        part: None,
        entity: key,
    })
}

fn entity_of(scene: &Scene, key: EntityId) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key)
}

fn the_entity(scene: &Scene, key: EntityId) -> Entity {
    entity_of(scene, key).unwrap_or_else(|| panic!("{key:?} is in the scene"))
}

fn shown(scene: &Scene, key: EntityId) -> Vec<(u8, DataShown, bool)> {
    let entity = the_entity(scene, key);
    mob_values(entity.kind, &entity.data)
}

fn shown_at(scene: &Scene, key: EntityId, at: u8) -> Option<DataShown> {
    shown(scene, key)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, _)| value)
}

fn int_at(scene: &Scene, key: EntityId, at: u8) -> i32 {
    match shown_at(scene, key, at) {
        Some(DataShown::Int(value)) => value,
        other => panic!("index {at} is an int: {other:?}"),
    }
}

fn flag_at(scene: &Scene, key: EntityId, at: u8) -> bool {
    match shown_at(scene, key, at) {
        Some(DataShown::Boolean(value)) => value,
        other => panic!("index {at} is a boolean: {other:?}"),
    }
}

fn air(scene: &Scene, key: EntityId) -> i32 {
    int_at(scene, key, index::AIR_SUPPLY)
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

/// The damage events of `key` in the tick, as (damage type, cause).
fn damage(output: &TickOutput, key: EntityId) -> Vec<(i32, Option<EntityId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::DamageEvent {
                entity,
                damage_type,
                cause,
                ..
            } if *entity == key => Some((*damage_type, *cause)),
            _ => None,
        })
        .collect()
}

/// How many sounds of id `sound` the tick played, at a place or at an entity.
fn played(output: &TickOutput, sound: i32) -> usize {
    output
        .events
        .iter()
        .filter(|event| match event {
            RegionEvent::Sound { sound: id, .. } | RegionEvent::EntitySound { sound: id, .. } => {
                *id == sound
            }
            _ => false,
        })
        .count()
}

/// The pitches of the sounds of id `sound` the tick played.
fn pitches(output: &TickOutput, sound: i32) -> Vec<f32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: id, pitch, ..
            }
            | RegionEvent::EntitySound {
                sound: id, pitch, ..
            } if *id == sound => Some(*pitch),
            _ => None,
        })
        .collect()
}

/// What the tick reported of `key` as tracked.
fn tracked(output: &TickOutput, key: EntityId) -> Option<Tracked> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::EntityTracked(tracked) if tracked.entity == key => Some(tracked.clone()),
        _ => None,
    })
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

fn others_of(scene: &Scene, kind: i32, known: &[EntityId]) -> Vec<Entity> {
    scene
        .of_kind(kind)
        .into_iter()
        .filter(|entity| !known.contains(&entity.key))
        .collect()
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn block_of(position: [f64; 3]) -> BlockPos {
    bp(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )
}

/// One tick with the world's clock published at the overworld's `day` ticks (and the
/// game time a tick on from the region's), as the store publishes it.
fn publish_day(scene: &mut Scene, day: u64) -> TickOutput {
    let now = scene.region.state().clock;
    scene.region.tick(&TickInputs {
        clock: Some(WorldClock {
            game_time: now.game_time + 1,
            day,
        }),
        ..TickInputs::default()
    })
}

/// The next overworld day tick at or after the region's that is `of_day` into a day.
fn next_day_at(scene: &Scene, of_day: u64) -> u64 {
    let day = scene.region.state().clock.day;
    let start = day - day % 24_000 + of_day;
    if start >= day { start } else { start + 24_000 }
}

// ------------------------------------------------------------------------------------
// 1. Eggs (brief 1.11, 6.6.1, 6.8.1, 6.9.1, 6.10.1).

/// An egg used on the floor of the pool makes one creature of its type in the water, at
/// its `max_health`, not from a bucket, with the synced values of its type at their
/// defaults but for what its `finalizeSpawn` draws (brief 9.6.3); the ageable ones are
/// adults, the first of a group never being a baby (`AgeableMob.finalizeSpawn`); its air
/// is full, 300, or the dolphin's 4800 (`Dolphin.finalizeSpawn` sets it,
/// `getMaxAirSupply` 4800).
#[test]
fn an_egg_makes_each_water_creature_in_water_with_its_types_health_and_defaults() {
    for kind in ALL {
        let mut scene = scene_with(&pool(), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(3, 0))
            .unwrap_or_else(|| panic!("the {} egg makes a mob", kind.name));
        let made = the_entity(&scene, key);
        assert_eq!(i32::from(made.kind), kind.kind, "{}", kind.name);
        assert_eq!(health(&scene, key), kind.health, "{}", kind.name);
        assert!(
            horizontal(made.position, [3.5, -60.0, 0.5]) < 0.3,
            "{} at {:?}",
            kind.name,
            made.position
        );
        assert_eq!(
            scene.block(block_of(made.position)),
            blocks::WATER,
            "{} is made in the water",
            kind.name
        );
        // A dolphin breathes by the living rule, and has ticked once under water in the
        // tick it was made: a point less than its 4800.
        let most = if kind.kind == entity_types::DOLPHIN {
            4_799..=4_800
        } else {
            300..=300
        };
        assert!(
            most.contains(&air(&scene, key)),
            "{}: its air is full: {}",
            kind.name,
            air(&scene, key)
        );
        for (at, default) in kind.own {
            let value = shown_at(&scene, key, *at)
                .unwrap_or_else(|| panic!("{} shows index {at}", kind.name));
            let drawn = (kind.kind == entity_types::SALMON
                || kind.kind == entity_types::TROPICAL_FISH)
                && *at == 17;
            if !drawn {
                assert_eq!(value, *default, "{}: index {at}", kind.name);
            }
        }
        assert_eq!(
            carried(&scene, kind.egg),
            1,
            "a creative player keeps the egg"
        );
    }
}

/// Brief 6.6.6 (`Salmon.finalizeSpawn`, Salmon.txt 187–220): an egg's salmon takes its
/// size from a weighted list of small 30, medium 50 and large 15, one `nextInt(95)`; 190
/// eggs make all three in about those shares (60, 100 and 30 expected).
#[test]
fn an_eggs_salmon_is_small_medium_or_large_by_the_games_weights() {
    let mut scene = scene_with(&cell(), &[items::SALMON_SPAWN_EGG]);
    let mut sizes = [0_u32; 3];
    for n in 0..190 {
        let key = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1))
            .unwrap_or_else(|| panic!("salmon {n} is made"));
        let size = int_at(&scene, key, 17);
        assert!((0..3).contains(&size), "a size of 0 to 2: {size}");
        sizes[size as usize] += 1;
    }
    assert!((35..=85).contains(&sizes[0]), "small: {sizes:?}");
    assert!((70..=130).contains(&sizes[1]), "medium: {sizes:?}");
    assert!((12..=50).contains(&sizes[2]), "large: {sizes:?}");
}

/// Brief 6.6.8 (`TropicalFish.finalizeSpawn`, TropicalFish.txt 376–450): an egg passes no
/// group data, so each fish draws: with 0.9 one of the 22 common variants
/// (`nextInt(22)`), else a pattern of the 12 and two colours of the 16. Of 200 eggs about
/// 180 are common, of many different variants, and the rest are any pattern with any
/// colours, packed as `packVariant` packs them.
#[test]
fn an_eggs_tropical_fish_is_mostly_one_of_the_common_variants() {
    let common: Vec<i32> = COMMON_VARIANTS
        .iter()
        .map(|(pattern, base, colour)| packed(*pattern, *base, *colour))
        .collect();
    let mut scene = scene_with(&cell(), &[items::TROPICAL_FISH_SPAWN_EGG]);
    let mut seen_common = Vec::new();
    let mut uncommon = 0;
    for n in 0..200 {
        let key = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1))
            .unwrap_or_else(|| panic!("tropical fish {n} is made"));
        let variant = int_at(&scene, key, 17);
        let pattern = (variant as u32) & 0xFFFF;
        let base = (variant as u32) >> 16 & 255;
        let colour = (variant as u32) >> 24 & 255;
        assert!(
            PATTERNS.contains(&pattern),
            "a pattern of the 12: {variant:#x}"
        );
        assert!(base < 16 && colour < 16, "dye colours: {variant:#x}");
        if common.contains(&variant) {
            if !seen_common.contains(&variant) {
                seen_common.push(variant);
            }
        } else {
            uncommon += 1;
        }
    }
    assert!((5..=40).contains(&uncommon), "{uncommon} of 200 uncommon");
    assert!(
        seen_common.len() >= 15,
        "{} common variants seen",
        seen_common.len()
    );
}

// ------------------------------------------------------------------------------------
// 2. Swimming (brief 6.5, 6.6.2, 6.6.3, 6.8.3, 6.8.4, 6.9).

/// Each swimmer made in the pool keeps to it for 600 ticks and moves in it: fish
/// (`FishSwimGoal`, `FishMoveControl`, `WaterBoundPathNavigation`) stay in its water;
/// squids (their movement vector, applied on one phase of a stroke) and the dolphin
/// (`RandomSwimmingGoal`, `SmoothSwimmingMoveControl`; its jumps need seven clear blocks
/// of water ahead, `DolphinJumpGoal`) stay over it; each goes more than a block from
/// where it was made. The player stands within 32 blocks, so none is idle
/// (`noActionTime`).
#[test]
fn each_swimmer_keeps_to_the_water_and_moves_in_it() {
    for kind in SWIMMERS {
        let mut scene = scene_with(&walled_pool(), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(5, 0)).expect("made");
        let start = the_entity(&scene, key).position;
        let mut farthest: f64 = 0.0;
        for tick in 0..600 {
            scene.idle();
            let now = the_entity(&scene, key).position;
            farthest = farthest.max(horizontal(now, start));
            let inside_x = f64::from(POOL_X.0) <= now[0] && now[0] <= f64::from(POOL_X.1 + 1);
            let inside_z = f64::from(POOL_Z.0) <= now[2] && now[2] <= f64::from(POOL_Z.1 + 1);
            assert!(
                inside_x && inside_z,
                "{}: over the pool at tick {tick}: {now:?}",
                kind.name
            );
            if FISH.iter().any(|fish| fish.kind == kind.kind) {
                assert_eq!(
                    scene.block(block_of(now)),
                    blocks::WATER,
                    "{}: in the water at tick {tick}: {now:?}",
                    kind.name
                );
            } else {
                assert!(
                    now[1] < f64::from(POOL_Y.1) + 4.0,
                    "{}: not far above the water at tick {tick}: {now:?}",
                    kind.name
                );
            }
        }
        assert!(farthest > 1.0, "{}: moved {farthest}", kind.name);
        assert!(entity_of(&scene, key).is_some(), "{} lives", kind.name);
        assert_eq!(
            health(&scene, key),
            kind.health,
            "{} is not hurt",
            kind.name
        );
    }
}

/// In water a fish's air is kept full (`WaterAnimal.handleAirSupply`: 300 in water), and
/// so is a squid's (`AgeableWaterCreature`'s, the same). "In water" is the box touching
/// it: a squid that swims up until its feet are above the surface (a source's top is 8/9
/// of the block) loses a point like any water animal out of it, so only the ticks it
/// ends in the water are judged.
#[test]
fn in_water_a_fish_and_a_squid_keep_their_air() {
    let surface = f64::from(POOL_Y.1) + 8.0 / 9.0;
    for kind in [&COD, &SQUID] {
        let mut scene = scene_with(&pool(), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(5, 0)).expect("made");
        let mut judged = 0;
        for _ in 0..200 {
            scene.idle();
            let feet = entity_of(&scene, key).expect("there").position[1];
            if feet < surface - 0.2 {
                judged += 1;
                assert_eq!(air(&scene, key), 300, "{}", kind.name);
            }
        }
        assert!(judged > 100, "{}: in the water {judged} ticks", kind.name);
    }
}

// ------------------------------------------------------------------------------------
// 3. Out of water (brief 6.5.1, 6.6.2, 6.8.4, 6.9.2).

/// Brief 6.5.1 and 6.6.2: a fish on dry land loses a point of air a tick from 300
/// (`handleAirSupply` with the air read before the tick), and at -20 it is hurt by
/// `drown` for 2 with its air set to 0, with no bubbles (event 67; that is the living
/// rule's, which this one overwrites); 20 ticks later again: with 3 health it dies in
/// its second hit, 340 ticks after it was made. Meanwhile it flops: on the ground with a
/// vertical collision it is thrown up by 0.4 with its kind's flop sound, and its motion
/// is sent at once (`needsSync`).
#[test]
fn a_fish_on_land_flops_and_drowns_on_the_games_schedule() {
    let drown = damage_type("minecraft:drown");
    for (kind, flop) in [
        (&COD, sounds::ENTITY_COD_FLOP),
        (&SALMON, sounds::ENTITY_SALMON_FLOP),
        (&PUFFERFISH, sounds::ENTITY_PUFFER_FISH_FLOP),
        (&TROPICAL_FISH, sounds::ENTITY_TROPICAL_FISH_FLOP),
    ] {
        let mut scene = scene_with(&dry_room(2), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
        let made_in = scene.region.tick_number();
        let first_air = air(&scene, key);
        assert!(
            (299..=300).contains(&first_air),
            "{}: {first_air}",
            kind.name
        );
        let mut hits = Vec::new();
        let mut flops = 0;
        let mut synced_flops = 0;
        let mut bubbles = 0;
        let mut previous_air = first_air;
        for _ in 0..400 {
            let output = scene.idle();
            let tick = scene.region.tick_number() - made_in;
            for (damage_type, _) in damage(&output, key) {
                assert_eq!(damage_type, drown, "{}: drowned", kind.name);
                hits.push(tick);
            }
            bubbles += events_of(std::slice::from_ref(&output), key, BUBBLES);
            if played(&output, flop) > 0 {
                flops += 1;
                if tracked(&output, key)
                    .is_some_and(|tracked| tracked.flags & Tracked::NEEDS_SYNC != 0)
                {
                    synced_flops += 1;
                }
            }
            // A dead fish's air is set to 300 again (`handleAirSupply` asks `isAlive`).
            if entity_of(&scene, key).is_none() || health(&scene, key) <= 0.0 {
                break;
            }
            let now = air(&scene, key);
            if hits.last() != Some(&tick) && now > -20 && previous_air > -20 {
                assert_eq!(now, previous_air - 1, "{}: one air a tick", kind.name);
            }
            previous_air = now;
        }
        assert_eq!(hits.len(), 2, "{}: two drowning hits: {hits:?}", kind.name);
        assert!(
            (319..=321).contains(&hits[0]),
            "{}: first hit {} ticks after it was made",
            kind.name,
            hits[0]
        );
        assert_eq!(hits[1] - hits[0], 20, "{}: {hits:?}", kind.name);
        assert!(
            entity_of(&scene, key).is_none() || health(&scene, key) <= 0.0,
            "{}: dead",
            kind.name
        );
        assert_eq!(bubbles, 0, "{}: no bubbles", kind.name);
        assert!(flops >= 10, "{}: {flops} flops", kind.name);
        assert_eq!(
            synced_flops, flops,
            "{}: each flop is sent at once",
            kind.name
        );
    }
}

/// Brief 6.6.2: the flop adds 0.4 up (the float widened, 0.4000000059604645) to the
/// motion the fish lay with (a body on the ground keeps the last tick's gravity, -0.08 ·
/// 0.98), besides a sideways push of at most 0.05 each way; then the tick's travel in the
/// air: gravity 0.08 and drag 0.98 up, 0.91 sideways, as it is off the ground.
#[test]
fn a_flop_throws_a_fish_up_by_the_games_lift() {
    let mut scene = scene_with(&dry_cell(2), &[items::COD_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
    let mut seen = 0;
    let mut before = the_entity(&scene, key);
    for _ in 0..200 {
        let output = scene.idle();
        let cod = the_entity(&scene, key);
        if played(&output, sounds::ENTITY_COD_FLOP) > 0 {
            assert!(before.on_ground, "it flops from the ground");
            let up = (before.motion[1] + 0.4000000059604645 - 0.08) * 0.9800000190734863;
            assert!(
                (cod.motion[1] - up).abs() < 1e-9,
                "lifted by the flop: {:?} from {:?}",
                cod.motion,
                before.motion
            );
            for axis in [0, 2] {
                let push = cod.motion[axis] / 0.9100000262260437 - before.motion[axis];
                assert!(push.abs() <= 0.05 + 1e-6, "pushed {push} along {axis}");
            }
            seen += 1;
        }
        before = cod;
    }
    assert!(seen >= 5, "{seen} flops");
}

/// Brief 6.9.2: on land a dolphin's moistness falls by one a tick from 2400 (synced at
/// 19), and at 0 or below it is hurt by `dry_out` for 1, as often as its hurt time lets
/// a hit through; its air is kept at its most, 4800, out of water (`increaseAirSupply`,
/// the dolphin's own, fills it at once). It flops on the ground too.
#[test]
fn a_dolphin_on_land_dries_out_over_its_moistness() {
    let dry_out = damage_type("minecraft:dry_out");
    let mut scene = scene_with(&dry_cell(3), &[items::DOLPHIN_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
    let made_in = scene.region.tick_number();
    let mut previous = int_at(&scene, key, MOISTNESS);
    assert!((2_399..=2_400).contains(&previous), "{previous}");
    let mut first_hit = None;
    for _ in 0..2_500 {
        let output = scene.idle();
        let tick = scene.region.tick_number() - made_in;
        let hits = damage(&output, key);
        if first_hit.is_none() && hits.iter().any(|(kind, _)| *kind == dry_out) {
            first_hit = Some(tick);
            break;
        }
        assert!(hits.is_empty(), "no other hurt: {hits:?} at {tick}");
        let now = int_at(&scene, key, MOISTNESS);
        assert_eq!(now, previous - 1, "one a tick at {tick}");
        previous = now;
        assert_eq!(air(&scene, key), 4_800, "its air is full on land");
    }
    let first = first_hit.expect("it dries out");
    assert!(
        (2_399..=2_401).contains(&first),
        "first hurt {first} ticks on"
    );
    // As long as its moistness is spent, it is hurt again whenever its hurt time allows:
    // every ten ticks for a hurt of the same amount.
    let mut later = 0;
    for _ in 0..100 {
        let output = scene.idle();
        later += damage(&output, key)
            .iter()
            .filter(|(kind, _)| *kind == dry_out)
            .count();
    }
    assert!((9..=11).contains(&later), "{later} hurts in 100 ticks");
}

/// Brief 6.8.4 (Squid.txt 324–371): out of water a squid's motion is set every tick to
/// `(0, (y − 0.08) · 0.98, 0)`: made in the air beside a pillar it falls straight down,
/// never sliding, each tick's fall the last's less gravity, times the drag.
#[test]
fn a_squid_out_of_water_falls_straight_down() {
    let mut pillar = Vec::new();
    for y in -60..=-54 {
        pillar.push((bp(2, y, 0), blocks::STONE));
    }
    for kind in [&SQUID, &GLOW_SQUID] {
        let mut scene = scene_with(&pillar, &[kind.egg]);
        // The east face of the pillar's top: the squid is made beside it, in the air.
        let key = hatch_on(&mut scene, 0, bp(2, -54, 0), Face::East).expect("made");
        let start = the_entity(&scene, key);
        assert!(start.position[1] > -55.0, "{}: in the air", kind.name);
        let mut previous = start.clone();
        let mut fell = 0;
        for _ in 0..40 {
            scene.idle();
            let now = the_entity(&scene, key);
            assert_eq!(
                (now.position[0], now.position[2]),
                (start.position[0], start.position[2]),
                "{}: straight down",
                kind.name
            );
            assert_eq!((now.motion[0], now.motion[2]), (0.0, 0.0), "{}", kind.name);
            if !now.on_ground && !previous.on_ground {
                let expected = (previous.motion[1] - 0.08) * 0.9800000190734863;
                assert!(
                    (now.motion[1] - expected).abs() < 1e-9,
                    "{}: {} after {}",
                    kind.name,
                    now.motion[1],
                    previous.motion[1]
                );
                fell += 1;
            }
            previous = now;
        }
        assert!(previous.on_ground, "{}: landed", kind.name);
        assert!((previous.position[1] - -60.0).abs() < 1e-9, "{}", kind.name);
        assert!(fell >= 5, "{}: {fell} ticks of falling", kind.name);
    }
}

/// Brief 6.5.2 and 6.8.5: a squid on land runs out of air as a fish does (the ageable
/// water creature's rule) and is hurt by drowning at -20; with no living attacker its
/// hurt squirts no ink (`Squid.hurtServer` asks `getLastHurtByMob`).
#[test]
fn a_squid_on_land_drowns_without_ink() {
    let drown = damage_type("minecraft:drown");
    let mut scene = scene_with(&dry_cell(2), &[items::SQUID_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
    let made_in = scene.region.tick_number();
    let mut seen = Vec::new();
    let hurt = scene.idle_until(400, &mut seen, |_, outputs| {
        outputs
            .last()
            .is_some_and(|output| !damage(output, key).is_empty())
    });
    assert!(hurt, "it drowns");
    let output = seen.last().expect("a tick");
    assert_eq!(damage(output, key), [(drown, None)]);
    let after = scene.region.tick_number() - made_in;
    assert!((319..=321).contains(&after), "hurt {after} ticks on");
    assert_eq!(played(output, sounds::ENTITY_SQUID_SQUIRT), 0, "no ink");
    assert_eq!(events_of(&seen, key, BUBBLES), 0, "no bubbles");
}

// ------------------------------------------------------------------------------------
// 4. Buckets (brief 6.7, W8).

/// Brief 6.7 (`Bucketable.bucketMobPickup`, Bucketable.txt 92–161): a water bucket used
/// on a fish plays `bucket.fill_fish`, takes the fish away and gives a bucket of its
/// kind. The pick-up calls `ItemUtils.createFilledResult(stack, player, bucket, false)`
/// (Bucketable.txt offset 67 `iconst_0`): with `limitCreative` false a creative player
/// keeps the water bucket and is given the filled one by `consume` and `add`, whether or
/// not one is held already: a second fish gives a second bucket.
#[test]
fn a_water_bucket_takes_a_fish_into_a_bucket_of_its_kind() {
    for kind in FISH {
        let bucket = kind.bucket.expect("a fish has a bucket");
        let mut scene = scene_with(&cell(), &[kind.egg, items::WATER_BUCKET]);
        let fish = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
        let output = use_on(&mut scene, 1, &[fish]);
        assert!(entity_of(&scene, fish).is_none(), "{}: taken", kind.name);
        assert!(
            output.events.iter().any(|event| matches!(event,
                RegionEvent::EntityGone { entity, .. } | RegionEvent::EntityRemoved { entity, .. }
                    if *entity == fish)),
            "{}: its going is shown",
            kind.name
        );
        assert_eq!(
            played(&output, sounds::ITEM_BUCKET_FILL_FISH),
            1,
            "{}",
            kind.name
        );
        assert_eq!(carried(&scene, bucket), 1, "{}: in a bucket", kind.name);
        assert_eq!(
            carried(&scene, items::WATER_BUCKET),
            1,
            "{}: the water bucket kept",
            kind.name
        );
    }
}

/// The second half of the rule above: in creative, `createFilledResult` with
/// `limitCreative` false adds a filled bucket for every fish taken
/// (ItemUtils.txt 17–60; the three-argument form passes `true` and adds only when none is
/// held, which is the milk bucket's rule and not this one).
#[test]
fn a_creative_player_is_given_a_bucket_for_every_fish_taken() {
    for kind in FISH {
        let bucket = kind.bucket.expect("a fish has a bucket");
        let mut scene = scene_with(&cell(), &[kind.egg, items::WATER_BUCKET]);
        for n in 0..2 {
            let fish = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
            use_on(&mut scene, 1, &[fish]);
            assert!(
                entity_of(&scene, fish).is_none(),
                "{} {n}: taken",
                kind.name
            );
        }
        assert_eq!(carried(&scene, bucket), 2, "{}: two buckets", kind.name);
    }
}

/// A water bucket used on a squid, a glow squid, a dolphin or a turtle does nothing: none
/// of them is `Bucketable`.
#[test]
fn a_water_bucket_takes_no_squid_dolphin_or_turtle() {
    for kind in [&SQUID, &GLOW_SQUID, &DOLPHIN, &TURTLE] {
        let mut scene = scene_with(&pool(), &[kind.egg, items::WATER_BUCKET]);
        let key = hatch(&mut scene, 0, floor_at(3, 0)).expect("made");
        let output = use_on(&mut scene, 1, &[key]);
        assert!(
            entity_of(&scene, key).is_some(),
            "{}: still there",
            kind.name
        );
        assert_eq!(played(&output, sounds::ITEM_BUCKET_FILL_FISH), 0);
        for fish in FISH {
            assert_eq!(carried(&scene, fish.bucket.expect("a bucket")), 0);
        }
    }
}

/// Brief 6.7 (`MobBucketItem`, `BucketItem.use`, `MobBucketItem.spawn`): a fish bucket
/// emptied on the floor pours water into the block above it and makes a fish of its kind
/// there, at the block's centre, marked as from a bucket (synced 16 true), with
/// `bucket.empty_fish`; a creative player keeps the bucket. The poured fish was made by
/// `EntityType.create` with `finalizeSpawn` (reason `BUCKET`), so its variant is a valid
/// one.
#[test]
fn a_fish_bucket_emptied_pours_water_and_makes_its_fish() {
    for kind in FISH {
        let bucket = kind.bucket.expect("a fish has a bucket");
        let mut scene = scene_with(&[], &[bucket]);
        scene.actors[0].sequence += 1;
        let sequence = scene.actors[0].sequence;
        // Looking east and down, at the floor's top some 1.6 blocks ahead: block (2,
        // -61, 0).
        let output = scene.input(PlayerInput::UseItem {
            rotation: (-90.0, 45.0),
            sneaking: false,
            sequence,
        });
        assert_eq!(
            scene.block(bp(2, -60, 0)),
            blocks::WATER,
            "{}: water",
            kind.name
        );
        let made = scene.of_kind(kind.kind);
        assert_eq!(made.len(), 1, "{}: one fish", kind.name);
        let fish = &made[0];
        assert!(
            horizontal(fish.position, [2.5, -60.0, 0.5]) < 1e-6,
            "{}: at the block's centre: {:?}",
            kind.name,
            fish.position
        );
        assert!(
            flag_at(&scene, fish.key, FROM_BUCKET),
            "{}: from a bucket",
            kind.name
        );
        assert_eq!(health(&scene, fish.key), 3.0, "{}", kind.name);
        assert_eq!(
            played(&output, sounds::ITEM_BUCKET_EMPTY_FISH),
            1,
            "{}",
            kind.name
        );
        assert_eq!(carried(&scene, bucket), 1, "{}: the bucket kept", kind.name);
        if kind.kind == entity_types::SALMON {
            assert!((0..3).contains(&int_at(&scene, fish.key, 17)));
        }
        if kind.kind == entity_types::TROPICAL_FISH {
            let variant = int_at(&scene, fish.key, 17) as u32;
            assert!(PATTERNS.contains(&(variant & 0xFFFF)));
        }
    }
}

/// What a fish's bucket keeps in the game and Clustine's stacks cannot (brief 6.7, W8):
/// `bucket_entity_data`'s `Health` (and the flags `NoAI`, `Silent`, `NoGravity`,
/// `Glowing`, `Invulnerable`, `PersistenceRequired`), the `custom_name`, the salmon's
/// `salmon/size` and the tropical fish's `tropical_fish/pattern`, `base_color` and
/// `pattern_color`. A hurt cod taken up and poured out again comes back hurt in the game;
/// in Clustine it comes back at full health, and a salmon or a tropical fish with a size
/// or a variant drawn again.
#[test]
#[ignore = "named difference (plan 11.6): buckets of fish carry no fish, as stacks have no components"]
fn a_hurt_fish_poured_from_its_bucket_keeps_its_health() {
    let mut scene = scene_with(&cell(), &[items::COD_SPAWN_EGG, items::WATER_BUCKET]);
    let fish = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
    hit(&mut scene, fish);
    let hurt = health(&scene, fish);
    assert!(hurt < 3.0, "hurt: {hurt}");
    use_on(&mut scene, 1, &[fish]);
    // Pour the bucket in the second slot onto the floor west of the player.
    let slot = (0..9)
        .find(|slot| scene.holds(*slot) == Some(stack(items::COD_BUCKET, 1)))
        .expect("a cod bucket");
    scene.select(u8::try_from(slot).expect("a hotbar slot"));
    scene.actors[0].sequence += 1;
    let sequence = scene.actors[0].sequence;
    scene.input(PlayerInput::UseItem {
        rotation: (90.0, 45.0),
        sneaking: false,
        sequence,
    });
    let poured = scene.of_kind(entity_types::COD);
    assert_eq!(poured.len(), 1);
    assert_eq!(health(&scene, poured[0].key), hurt);
}

// ------------------------------------------------------------------------------------
// 5. The pufferfish (brief 6.6.7, W6, W14).

/// `Pufferfish.SCARY_MOB` (Pufferfish.txt 410–430): a creative player is not scary, so a
/// pufferfish beside one stays flat.
#[test]
fn a_pufferfish_does_not_puff_at_a_creative_player() {
    let mut scene = scene_with(&cell(), &[items::PUFFERFISH_SPAWN_EGG]);
    let fish = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
    for _ in 0..100 {
        let output = scene.idle();
        assert_eq!(int_at(&scene, fish, PUFF_STATE), 0);
        assert_eq!(played(&output, sounds::ENTITY_PUFFER_FISH_BLOW_UP), 0);
    }
}

/// Brief 6.6.7: with a cow (scary: not in `#not_scary_for_pufferfish`) within 2 of it, a
/// pufferfish puffs half at once (`PufferfishPuffGoal.start`, then state 1 in the next
/// `tick` with `puffer_fish.blow_up`), full 40 ticks later (`inflateCounter > 40`); while
/// puffed it stings the cow within 0.3 of its box (`Pufferfish.touch`: a `mob_attack` hurt
/// of 1 + its state, caused by the fish, with `puffer_fish.sting`). Once the cow is dead
/// the goal stops: the fish goes back to half 62 ticks later (`deflateTimer > 60`) and
/// flat 40 after that (`deflateTimer > 100`), with `puffer_fish.blow_out` each time.
#[test]
fn a_pufferfish_puffs_at_a_cow_stings_it_and_deflates_when_it_is_gone() {
    let mob_attack = damage_type("minecraft:mob_attack");
    // Two blocks of water under a roof: the cow, 1.4 high, floats up against the roof
    // and the fish is never more than a block below it.
    let water = pool_of((CELL.0, CELL.0), (CELL.1, CELL.1), (-60, -59), true);
    let mut scene = scene_with(&water, &[items::PUFFERFISH_SPAWN_EGG, items::COW_SPAWN_EGG]);
    let fish = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("a pufferfish");
    let cow = hatch(&mut scene, 1, floor_at(CELL.0, CELL.1)).expect("a cow");
    let start = scene.region.tick_number();
    let mut states = vec![(start, int_at(&scene, fish, PUFF_STATE))];
    let mut stings = Vec::new();
    let mut sting_sounds = 0;
    let mut blow_ups = 0;
    let mut blow_outs = 0;
    let mut cow_gone = None;
    for _ in 0..500 {
        let output = scene.idle();
        let tick = scene.region.tick_number();
        for (kind, cause) in damage(&output, cow) {
            if kind == mob_attack {
                stings.push((tick, cause));
            }
        }
        sting_sounds += played(&output, sounds::ENTITY_PUFFER_FISH_STING);
        blow_ups += played(&output, sounds::ENTITY_PUFFER_FISH_BLOW_UP);
        blow_outs += played(&output, sounds::ENTITY_PUFFER_FISH_BLOW_OUT);
        let state = int_at(&scene, fish, PUFF_STATE);
        if states.last().map(|(_, last)| *last) != Some(state) {
            states.push((tick, state));
        }
        let cow_here = entity_of(&scene, cow).is_some_and(|_| health(&scene, cow) > 0.0);
        if !cow_here && cow_gone.is_none() {
            cow_gone = Some(tick);
        }
        if cow_gone.is_some() && state == 0 {
            break;
        }
    }
    let gone = cow_gone.expect("the stings kill the cow");
    let order: Vec<i32> = states.iter().map(|(_, state)| *state).collect();
    assert_eq!(order, [0, 1, 2, 1, 0], "the puff's states: {states:?}");
    let [_, (half, _), (full, _), (back, _), (flat, _)] = states[..] else {
        panic!("{states:?}")
    };
    assert!(half - start <= 3, "half at once: {states:?}");
    assert!(
        (39..=41).contains(&(full - half)),
        "full 40 later: {states:?}"
    );
    assert!(
        (back - gone) >= 60 && (back - gone) <= 66,
        "half again some 62 ticks after the cow died at {gone}: {states:?}"
    );
    assert!(
        (39..=41).contains(&(flat - back)),
        "flat 40 later: {states:?}"
    );
    assert_eq!((blow_ups, blow_outs), (2, 2));
    assert!(!stings.is_empty(), "the cow is stung");
    assert!(
        stings.iter().all(|(_, cause)| *cause == Some(fish)),
        "by the fish: {stings:?}"
    );
    assert!(
        stings.iter().all(|(tick, _)| *tick >= half),
        "only once puffed"
    );
    assert_eq!(sting_sounds, stings.len(), "a sting sound for every sting");
}

// ------------------------------------------------------------------------------------
// 6. Squids (brief 6.8, W12, W13).

/// Brief 6.8.5: a squid hurt by a living attacker (the player) squirts its ink: the
/// squirt sound of its kind (`squid.squirt`, `glow_squid.squirt`) with its two pitch
/// draws, beside 30 ink particles, which Clustine does not send (brief 9.6.3: "Clustine
/// sends no particles yet"). A glow squid hurt goes dark for 100 ticks
/// (`setDarkTicks(100)`), its synced count at 18 falling by one every tick to 0.
#[test]
fn a_squid_hit_by_a_player_squirts_its_ink_and_a_glow_squid_goes_dark() {
    for (kind, squirt) in [
        (&SQUID, sounds::ENTITY_SQUID_SQUIRT),
        (&GLOW_SQUID, sounds::ENTITY_GLOW_SQUID_SQUIRT),
    ] {
        let mut scene = scene_with(&cell(), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(CELL.0, CELL.1)).expect("made");
        scene.idle_for(5);
        let output = hit(&mut scene, key);
        assert_eq!(damage(&output, key).len(), 1, "{}: hurt", kind.name);
        assert_eq!(played(&output, squirt), 1, "{}: ink", kind.name);
        let pitch = pitches(&output, squirt)[0];
        assert!((0.8..=1.2).contains(&pitch), "{}: pitch {pitch}", kind.name);
        if kind.kind == entity_types::GLOW_SQUID {
            let mut dark = int_at(&scene, key, DARK_TICKS);
            assert!((99..=100).contains(&dark), "dark for 100: {dark}");
            while dark > 0 {
                scene.idle();
                let now = int_at(&scene, key, DARK_TICKS);
                assert_eq!(now, dark - 1, "one a tick");
                dark = now;
            }
            scene.idle_for(5);
            assert_eq!(int_at(&scene, key, DARK_TICKS), 0, "stays at 0");
        }
    }
}

/// Brief 6.8.4 and W12: a squid's tentacles take 32 to 63 ticks a stroke
/// (`tentacleSpeed` from 0.1 to 0.2 a tick over 2π), and every stroke is shown by entity
/// event 19: 6 to 13 strokes in 400 ticks.
#[test]
fn a_squid_shows_every_stroke_of_its_tentacles() {
    for kind in [&SQUID, &GLOW_SQUID] {
        let mut scene = scene_with(&pool(), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(5, 0)).expect("made");
        let seen = scene.idle_for(400);
        let strokes = events_of(&seen, key, STROKE);
        assert!(
            (6..=13).contains(&strokes),
            "{}: {strokes} strokes",
            kind.name
        );
        // Between two strokes at least 32 ticks pass.
        let ticks: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter(|(_, output)| events_of(std::slice::from_ref(output), key, STROKE) > 0)
            .map(|(tick, _)| tick)
            .collect();
        for pair in ticks.windows(2) {
            assert!(
                (31..=64).contains(&(pair[1] - pair[0])),
                "{}: strokes at {ticks:?}",
                kind.name
            );
        }
    }
}

// ------------------------------------------------------------------------------------
// 7. The dolphin's air (brief 6.5.3, 6.5.7, 6.9.2).

/// Brief 6.5.3 and 6.9.2: under water a dolphin breathes by the living rule: a point of
/// air a tick from 4800 (no oxygen bonus, no draw). In a pool open to the air it comes up
/// to breathe (`BreathAirGoal` below 140, or as it swims), and at the surface its air is
/// filled to 4800 at once (the dolphin's `increaseAirSupply`); it never drowns.
#[test]
fn a_dolphin_comes_up_to_breathe_and_never_drowns_in_an_open_pool() {
    let mut scene = scene_with(&walled_pool(), &[items::DOLPHIN_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(5, 0)).expect("made");
    let mut lowest = air(&scene, key);
    let mut refilled_from_low = false;
    let mut previous = lowest;
    for tick in 0..6_000 {
        let output = scene.idle();
        assert!(damage(&output, key).is_empty(), "not hurt at {tick}");
        let now = air(&scene, key);
        assert!(now > 0, "air left at {tick}: {now}");
        if now > previous {
            assert_eq!(now, 4_800, "filled at once at {tick}, from {previous}");
            if previous < 4_700 {
                refilled_from_low = true;
            }
        } else {
            assert!(previous - now <= 1, "one a tick at {tick}");
        }
        lowest = lowest.min(now);
        previous = now;
    }
    assert!(lowest < 4_800, "it was under water");
    assert!(refilled_from_low, "it came up for air after a dive");
}

/// Brief 6.5.3: a dolphin kept under water, in a pool with a roof, loses its air a point a
/// tick from 4800 and at -20 drowns: the bubbles (event 67, the living rule's) and a
/// `drown` hurt of 2, 4820 ticks after it was made.
#[test]
fn a_dolphin_under_a_roof_drowns_with_bubbles() {
    let drown = damage_type("minecraft:drown");
    let water = pool_of((2, 4), (-1, 1), (-60, -59), true);
    let mut scene = scene_with(&water, &[items::DOLPHIN_SPAWN_EGG]);
    let key = hatch(&mut scene, 0, floor_at(3, 0)).expect("made");
    let made_in = scene.region.tick_number();
    let mut previous = air(&scene, key);
    for _ in 0..5_000 {
        let output = scene.idle();
        let hits = damage(&output, key);
        if !hits.is_empty() {
            assert_eq!(hits, [(drown, None)]);
            assert_eq!(events_of(std::slice::from_ref(&output), key, BUBBLES), 1);
            let after = scene.region.tick_number() - made_in;
            assert!((4_819..=4_821).contains(&after), "drowned {after} ticks on");
            assert_eq!(air(&scene, key), 0, "its air set to 0");
            return;
        }
        let now = air(&scene, key);
        assert_eq!(now, previous - 1, "a point a tick");
        previous = now;
    }
    panic!("it did not drown");
}

/// Brief 6.9.9: a fish (`#fishes`) fed to a dolphin plays `dolphin.eat` and gives it the
/// fish (synced 18 true); a creative player keeps the fish.
#[test]
fn a_dolphin_fed_a_fish_shows_it_got_one() {
    let mut scene = scene_with(&pool(), &[items::DOLPHIN_SPAWN_EGG, items::COD]);
    let key = hatch(&mut scene, 0, floor_at(3, 0)).expect("made");
    assert!(!flag_at(&scene, key, GOT_FISH));
    let output = use_on(&mut scene, 1, &[key]);
    assert_eq!(played(&output, sounds::ENTITY_DOLPHIN_EAT), 1);
    assert_eq!(carried(&scene, items::COD), 1);
    // The treasure goal, which clears it once it has searched, runs in the AI's ticks
    // after this one.
    assert!(flag_at(&scene, key, GOT_FISH), "it got a fish");
}

// ------------------------------------------------------------------------------------
// 8. The turtle (brief 6.10).

/// A beach of sand, x and z from -3 to 4, walled two high around, with the player at the
/// spawn on it.
fn beach() -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for x in -3..=4 {
        for z in -3..=4 {
            placed.extend(sand_at(bp(x, -61, z)));
        }
    }
    for y in [-60, -59] {
        for along in -4..=5 {
            for (x, z) in [(along, -4), (along, 5), (-4, along), (5, along)] {
                placed.push((bp(x, y, z), blocks::STONE));
            }
        }
    }
    placed
}

/// Positions of the turtle-egg blocks in `scene` among `around`.
fn eggs_in(scene: &Scene, around: &[BlockPos]) -> Vec<(BlockPos, BlockState)> {
    around
        .iter()
        .map(|at| (*at, scene.block(*at)))
        .filter(|(_, state)| state.block_id() == blocks::TURTLE_EGG.block_id())
        .collect()
}

fn beach_cells() -> Vec<BlockPos> {
    let mut cells = Vec::new();
    for x in -3..=4 {
        for z in -3..=4 {
            cells.push(bp(x, -60, z));
        }
    }
    cells
}

fn property(state: BlockState, name: &str) -> i32 {
    state
        .property(name)
        .unwrap_or_else(|| panic!("{name} of {state:?}"))
        .parse()
        .expect("a number")
}

/// The digging shown at the turtle (`Turtle.aiStep`, level event 2001 every fifth tick of
/// laying, of the block below it), as a level event or a block's destruction.
fn digs(output: &TickOutput) -> Vec<(BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind: 2001,
                position,
                data,
            } => Some((*position, *data)),
            RegionEvent::BlockDestroyed {
                position, state, ..
            } => Some((*position, i32::from(state.0))),
            _ => None,
        })
        .collect()
}

/// Brief 6.10.4 (`TurtleBreedGoal.breed`, Turtle$TurtleBreedGoal.txt 30–113): two turtles
/// fed seagrass (`#turtle_food`) fall in love and breed, but no baby is made: the turtle
/// whose goal ran carries an egg (synced 18), and one orb is dropped. A turtle with an
/// egg takes no food (`canFallInLove`).
///
/// Then brief 6.10.6 (`TurtleLayEggGoal`): at its home beach (within 9 of where its egg
/// made it, `finalizeSpawn`'s `setHomePos`) it goes to a sand block with air above, digs
/// (synced 19 true, the digging shown every fifth tick), and after 200 ticks of it lays a
/// clutch of 1 to 4 eggs (`nextInt(4) + 1`) above the sand with `turtle.lay_egg`; it then
/// carries no egg and digs no more.
#[test]
fn turtles_bred_with_seagrass_lay_eggs_on_the_sand_of_their_beach() {
    let mut scene = scene_with(&beach(), &[items::TURTLE_SPAWN_EGG, items::SEAGRASS]);
    let a = hatch(&mut scene, 0, floor_at(2, 2)).expect("a turtle");
    let b = hatch(&mut scene, 0, floor_at(2, -1)).expect("a turtle");
    scene.idle_for(2);
    let fed = use_on(&mut scene, 1, &[a, b]);
    assert_eq!(events_of(std::slice::from_ref(&fed), a, HEARTS), 1);
    assert_eq!(events_of(std::slice::from_ref(&fed), b, HEARTS), 1);
    assert_eq!(
        carried(&scene, items::SEAGRASS),
        1,
        "a creative player keeps it"
    );

    let mut seen = Vec::new();
    let bred = scene.idle_until(400, &mut seen, |scene, _| {
        flag_at(scene, a, HAS_EGG) || flag_at(scene, b, HAS_EGG)
    });
    assert!(bred, "they bred");
    let mother = if flag_at(&scene, a, HAS_EGG) { a } else { b };
    let father = if mother == a { b } else { a };
    assert!(!flag_at(&scene, father, HAS_EGG), "only one carries an egg");
    assert!(
        others_of(&scene, entity_types::TURTLE, &[a, b]).is_empty(),
        "no baby"
    );
    assert_eq!(
        scene.of_kind(entity_types::EXPERIENCE_ORB).len(),
        1,
        "one orb"
    );

    let mut digging_ticks = 0;
    let mut dug = Vec::new();
    let mut laid = None;
    for _ in 0..4_000 {
        let output = scene.idle();
        if flag_at(&scene, mother, LAYING_EGG) {
            digging_ticks += 1;
            dug.extend(digs(&output));
        }
        let eggs = eggs_in(&scene, &beach_cells());
        if !eggs.is_empty() {
            laid = Some((eggs, output));
            break;
        }
    }
    let (eggs, output) = laid.expect("an egg is laid");
    assert_eq!(eggs.len(), 1, "one clutch: {eggs:?}");
    let (at, state) = eggs[0];
    assert_eq!(
        scene.block(at.offset(0, -1, 0)),
        blocks::SAND,
        "laid on sand"
    );
    let count = property(state, "eggs");
    assert!((1..=4).contains(&count), "{count} eggs");
    assert_eq!(property(state, "hatch"), 0);
    assert_eq!(played(&output, sounds::ENTITY_TURTLE_LAY_EGG), 1);
    assert!(!flag_at(&scene, mother, HAS_EGG), "no egg carried after");
    assert!(!flag_at(&scene, mother, LAYING_EGG), "no more digging");
    // `layEggCounter` counts only the ticks the turtle is at its target out of water
    // (TurtleLayEggGoal.txt 12–26, 199–219, inside the test of offsets 19 and 26), while
    // `LAYING_EGG` stays true from the first of them to the laying: at least 200 ticks.
    assert!(digging_ticks >= 199, "dug for {digging_ticks} ticks");
    // `Turtle.aiStep` shows the digging at every tick the counter stands at a multiple
    // of 5, from 5 to 200.
    let sand = i32::from(blocks::SAND.0);
    assert!(
        dug.len() >= 40,
        "the digging shown every fifth counted tick: {dug:?}"
    );
    assert!(
        dug.iter().all(|(_, data)| *data == sand),
        "of sand: {dug:?}"
    );
}

/// 64 eggs on sand, x and z from 16 to 23, and where they are.
fn nests() -> (Vec<(BlockPos, BlockState)>, Vec<BlockPos>) {
    let mut placed = Vec::new();
    let mut nests = Vec::new();
    for x in 16..24 {
        for z in 16..24 {
            placed.extend(sand_at(bp(x, -61, z)));
            placed.push((bp(x, -60, z), blocks::TURTLE_EGG));
            nests.push(bp(x, -60, z));
        }
    }
    (placed, nests)
}

/// The eggs at which a crack or a hatch was heard in the tick: each is a random tick of
/// an egg whose chance came up (`TurtleEggBlock.randomTick`).
fn hatchings(output: &TickOutput) -> Vec<BlockPos> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound, position, ..
            } if *sound == sounds::ENTITY_TURTLE_EGG_CRACK
                || *sound == sounds::ENTITY_TURTLE_EGG_HATCH =>
            {
                Some(bp(
                    position.x.floor() as i32,
                    position.y.floor() as i32,
                    position.z.floor() as i32,
                ))
            }
            _ => None,
        })
        .collect()
}

/// Brief 6.10.7 and W11 (`TurtleEggBlock.randomTick`, `shouldUpdateHatchLevel`): a turtle
/// egg on sand cracks (or, cracked twice, hatches) at a random tick when the level's
/// `nextFloat` is below `turtle_egg_hatch_chance`, which the day's timeline holds at 1.0
/// from 21062 to 21905 ticks into the day (before dawn) and at 0.002 otherwise
/// (`timeline/day.json`). 64 eggs are given some 40 random ticks in that hour (three
/// positions of 4096 a tick in their section), and about as many cracks are heard; in as
/// long at noon hardly any.
#[test]
fn turtle_eggs_crack_before_dawn_and_hardly_ever_by_day() {
    let (placed, nests) = nests();
    let mut scene = scene_with(&placed, &[]);
    let noon = next_day_at(&scene, 6_000);
    publish_day(&mut scene, noon);
    let mut by_day = 0;
    for _ in 0..843 {
        by_day += hatchings(&scene.idle()).len();
    }
    assert!(by_day <= 2, "{by_day} cracked by day");
    let dawn = next_day_at(&scene, 21_062);
    publish_day(&mut scene, dawn);
    let mut before_dawn = 0;
    for _ in 0..842 {
        let heard = hatchings(&scene.idle());
        assert!(heard.iter().all(|at| nests.contains(at)), "{heard:?}");
        before_dawn += heard.len();
    }
    assert!(
        (20..=65).contains(&before_dawn),
        "{before_dawn} cracked before dawn"
    );
}

/// The game draws the positions of a section's random ticks from the level's one random,
/// three a tick (`ServerLevel.tickChunk`), so the egg a random tick reached in one tick is
/// reached in the next with a chance of 3 in 4096: of the some 40 cracks of 64 eggs
/// before dawn, hardly any comes the tick after another of the same egg, and a clutch
/// cracks and hatches over the hour, not in three ticks running.
#[test]
fn an_egg_cracks_at_random_ticks_independent_of_the_last() {
    let (placed, _) = nests();
    let mut scene = scene_with(&placed, &[]);
    let dawn = next_day_at(&scene, 21_062);
    publish_day(&mut scene, dawn);
    let mut last: Vec<BlockPos> = Vec::new();
    let mut again = 0;
    let mut all = 0;
    for _ in 0..842 {
        let heard = hatchings(&scene.idle());
        again += heard.iter().filter(|at| last.contains(at)).count();
        all += heard.len();
        last = heard;
    }
    assert!(all >= 20, "{all} cracks");
    assert!(
        again <= 2,
        "{again} of {all} cracks came the tick after one of the same egg"
    );
}

/// Brief 6.10.7: an egg cracked twice (`hatch` 2) hatches at its next random tick that
/// passes the chance: the block goes, `turtle_egg.hatch` plays, and each of its eggs
/// becomes a baby turtle (`create` with `BREEDING`, age -24000), the i-th at
/// (x + 0.3 + i · 0.2, y, z + 0.3), each with the block's destruction shown
/// (level event 2001).
#[test]
fn a_cracked_clutch_hatches_a_baby_for_every_egg() {
    let mut placed = Vec::new();
    let mut nests = Vec::new();
    for x in 16..20 {
        for z in 16..20 {
            let eggs = 1 + (x + z) % 4;
            placed.extend(sand_at(bp(x, -61, z)));
            let state = BlockState::parse(&format!("minecraft:turtle_egg[eggs={eggs},hatch=2]"))
                .expect("a turtle egg");
            placed.push((bp(x, -60, z), state));
            nests.push(bp(x, -60, z));
        }
    }
    let mut scene = scene_with(&placed, &[]);
    let dawn = next_day_at(&scene, 21_062);
    publish_day(&mut scene, dawn);
    for _ in 0..842 {
        let before: Vec<(BlockPos, BlockState)> =
            nests.iter().map(|at| (*at, scene.block(*at))).collect();
        let known: Vec<EntityId> = scene
            .of_kind(entity_types::TURTLE)
            .iter()
            .map(|t| t.key)
            .collect();
        let output = scene.idle();
        let hatched: Vec<(BlockPos, BlockState)> = before
            .into_iter()
            .filter(|(at, _)| scene.block(*at).is_air())
            .collect();
        if hatched.is_empty() {
            continue;
        }
        assert_eq!(hatched.len(), 1, "one clutch in a tick, as it happens here");
        let (at, state) = hatched[0];
        let eggs = property(state, "eggs");
        assert_eq!(played(&output, sounds::ENTITY_TURTLE_EGG_HATCH), 1);
        let babies = others_of(&scene, entity_types::TURTLE, &known);
        assert_eq!(babies.len(), eggs as usize, "a baby an egg");
        // The babies of a clutch overlap (0.36 wide, 0.2 apart) and push each other in
        // their first tick, which they have before this reads them: by a few hundredths.
        for (i, _) in (0..eggs).enumerate() {
            let x = f64::from(at.x) + 0.3 + i as f64 * 0.2;
            let z = f64::from(at.z) + 0.3;
            assert!(
                babies.iter().any(|baby| (baby.position[0] - x).abs() < 0.05
                    && (baby.position[2] - z).abs() < 0.05
                    && (baby.position[1] - f64::from(at.y)).abs() < 1e-6),
                "a baby at ({x}, {}, {z}): {:?}",
                at.y,
                babies.iter().map(|baby| baby.position).collect::<Vec<_>>()
            );
        }
        for baby in &babies {
            assert!(flag_at(&scene, baby.key, index::BABY), "a baby");
        }
        let shown = output
            .events
            .iter()
            .filter(|event| match event {
                RegionEvent::LevelEvent {
                    kind: 2001,
                    position,
                    ..
                }
                | RegionEvent::BlockDestroyed { position, .. } => *position == at,
                _ => false,
            })
            .count();
        assert_eq!(shown, eggs as usize, "the destruction shown for every egg");
        return;
    }
    panic!("nothing hatched before dawn");
}

/// Brief 6.10.1 (`Turtle.ageBoundaryReached`): a baby turtle (an egg used on an adult)
/// growing up drops a turtle scute (`gameplay/turtle_grow`, one `turtle_scute`) at
/// itself, in the tick it stops being a baby, 24000 ticks after it was made; an adult
/// drops none.
#[test]
fn a_baby_turtle_drops_a_scute_when_it_grows_up() {
    // A pen of sand east of the player, out of their reach for pick-ups.
    let mut placed = Vec::new();
    for x in 4..=8 {
        for z in -2..=2 {
            placed.extend(sand_at(bp(x, -61, z)));
        }
    }
    for y in [-60, -59] {
        for along in -3..=3 {
            placed.push((bp(3, y, along), blocks::STONE));
            placed.push((bp(9, y, along), blocks::STONE));
        }
        for x in 3..=9 {
            placed.push((bp(x, y, -3), blocks::STONE));
            placed.push((bp(x, y, 3), blocks::STONE));
        }
    }
    let mut scene = scene_with(&placed, &[items::TURTLE_SPAWN_EGG]);
    let adult = hatch(&mut scene, 0, floor_at(5, 0)).expect("a turtle");
    use_on(&mut scene, 0, &[adult]);
    let made_in = scene.region.tick_number();
    let babies = others_of(&scene, entity_types::TURTLE, &[adult]);
    assert_eq!(babies.len(), 1, "a baby");
    let baby = babies[0].key;
    assert!(flag_at(&scene, baby, index::BABY));
    assert_eq!(health(&scene, baby), 30.0, "a baby's health is a turtle's");
    let scutes = |scene: &Scene| {
        lying_count(&scene.all(), items::TURTLE_SCUTE) + carried(scene, items::TURTLE_SCUTE)
    };
    for _ in 0..24_100 {
        scene.idle();
        if !flag_at(&scene, baby, index::BABY) {
            break;
        }
        assert_eq!(scutes(&scene), 0, "no scute while a baby");
    }
    assert!(!flag_at(&scene, baby, index::BABY), "it grew up");
    let took = scene.region.tick_number() - made_in;
    assert!((23_999..=24_001).contains(&took), "grew up after {took}");
    assert_eq!(scutes(&scene), 1, "one scute");
    let lying = items_lying(&scene.all());
    let scute = lying
        .iter()
        .find(|(stack, _)| stack.item == items::TURTLE_SCUTE)
        .expect("the scute lies");
    let grown = the_entity(&scene, baby).position;
    assert!(
        horizontal(scute.1, grown) < 0.5,
        "at the turtle: {scute:?} {grown:?}"
    );
    scene.idle_for(100);
    assert_eq!(scutes(&scene), 1, "no second scute");
}

// ------------------------------------------------------------------------------------
// 9. The synced data (brief 9.6.3).

/// Brief 9.6.3: what each creature shows beyond `Mob`'s values (index 15 and below) is
/// its own, at the indices the game has: a fish its `FROM_BUCKET` at 16 (no baby flag: a
/// `WaterAnimal` is not ageable) and its kind's value at 17; a squid the ageable's 16 and
/// 17, a glow squid its dark ticks at 18; a dolphin its fish at 18 and moistness at 19;
/// a turtle its egg at 18 and digging at 19. Values at their defaults are marked so, and
/// the dolphin's air of 4800 is its default (its `getMaxAirSupply`).
#[test]
fn each_creature_shows_its_own_synced_values_at_the_games_indices() {
    for kind in ALL {
        let mut scene = scene_with(&pool(), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(3, 0)).expect("made");
        let values = shown(&scene, key);
        let own: Vec<u8> = values
            .iter()
            .map(|(at, _, _)| *at)
            .filter(|at| *at > index::MOB_FLAGS)
            .collect();
        let expected: Vec<u8> = kind.own.iter().map(|(at, _)| *at).collect();
        assert_eq!(own, expected, "{}: its own indices", kind.name);
        for (at, value, default) in &values {
            if let Some((_, game_default)) = kind.own.iter().find(|(own, _)| own == at) {
                assert_eq!(
                    *default,
                    value == game_default,
                    "{}: index {at} is marked as the default only when it is",
                    kind.name
                );
            }
        }
        let (_, air_value, air_default) = values
            .iter()
            .find(|(at, _, _)| *at == index::AIR_SUPPLY)
            .expect("air");
        let most = if kind.kind == entity_types::DOLPHIN {
            4_800
        } else {
            300
        };
        assert_eq!(
            *air_default,
            *air_value == DataShown::Int(most),
            "{}: full air, {most}, is the default: {air_value:?}",
            kind.name
        );
    }
}

/// Brief 9.6.3: a pufferfish's puff state is shown as it changes, and a poured fish shows
/// its `FROM_BUCKET` true, which is not the default.
#[test]
fn a_poured_fish_shows_it_came_from_a_bucket() {
    let mut scene = scene_with(&[], &[items::COD_BUCKET]);
    scene.actors[0].sequence += 1;
    let sequence = scene.actors[0].sequence;
    scene.input(PlayerInput::UseItem {
        rotation: (-90.0, 45.0),
        sneaking: false,
        sequence,
    });
    let cod = scene.of_kind(entity_types::COD);
    assert_eq!(cod.len(), 1);
    let value = shown(&scene, cod[0].key)
        .into_iter()
        .find(|(at, _, _)| *at == FROM_BUCKET)
        .expect("index 16");
    assert_eq!(value, (FROM_BUCKET, DataShown::Boolean(true), false));
}

// ------------------------------------------------------------------------------------
// 10. Determinism (ADR-0028; `clustine-sim` is a function of its inputs).

/// Two regions given the same inputs, with every swimmer hatched in a pool, a squid hit
/// and a cod taken in a bucket, report the same changes and events every tick.
#[test]
fn two_regions_given_the_same_inputs_commit_the_same_water_creatures() {
    let mut slots: Vec<i32> = SWIMMERS.iter().map(|kind| kind.egg).collect();
    slots.push(items::WATER_BUCKET);
    let mut scenes: Vec<Scene> = (0..2).map(|_| scene_with(&pool(), &slots)).collect();
    let mut outputs: [Vec<TickOutput>; 2] = [Vec::new(), Vec::new()];
    let mut keys = Vec::new();
    for (scene, seen) in scenes.iter_mut().zip(&mut outputs) {
        let mut made = Vec::new();
        for (slot, _) in SWIMMERS.iter().enumerate() {
            let slot = u8::try_from(slot).expect("a hotbar slot");
            made.push(hatch(scene, slot, floor_at(3, 0)).expect("made"));
        }
        seen.extend(scene.idle_for(100));
        seen.push(hit(scene, made[4]));
        seen.push(use_on(scene, 7, &[made[0]]));
        seen.extend(scene.idle_for(300));
        keys.push(made);
    }
    assert_eq!(keys[0], keys[1]);
    for (tick, (one, other)) in outputs[0].iter().zip(&outputs[1]).enumerate() {
        assert_eq!(one.entity_changes, other.entity_changes, "tick {tick}");
        assert_eq!(one.events, other.events, "tick {tick}");
    }
    assert_eq!(scenes[0].all(), scenes[1].all());
}
