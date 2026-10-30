//! Tests of the golems, the villager and the wandering trader, written from
//! `docs/groundwork/n7k-brief.md` (sections 1 and 2, with 11.6, 11.7 and 11.10) and
//! `docs/groundwork/n7k-plan.md` (sections 1.1, 1.2 and 4, amended by sections 10 and 11,
//! which hold where they differ), by someone who did not write the code, driving regions
//! by their public interface alone: players place pumpkins, use eggs, items and their
//! fists, and the regions' reports are read; two regions at a pinned line are given what
//! a worker and the world store would pass between them.
//!
//! Mobs are the game's own: hatched by spawn eggs, and where a test wants one somewhere an
//! egg cannot put it (in deep water, a hair short of a line), hatched in a scratch region
//! and its record set into the test's world, as `mount_across_a_line_specification.rs`
//! sets its mounts.

mod n6_scene;

use clustine_data::{blocks, entity_types, items, sounds, villagers};
use clustine_sim::api::{EntityKind, Face, PlayerEvent, PlayerInput, RegionEvent, WorldClock};
use clustine_sim::shown::{DataShown, index, mob_shared_flags, mob_values};
use clustine_sim::{TickInputs, TickOutput};
use clustine_world::{
    BlockPos, Due, Effect, EffectId, Effected, Entity, EntityData, EntityId, EntityUse,
    FiledEffect, ItemStack, Vec3,
};
use n6_scene::*;
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The numbers of the specification.

/// The level event of a block broken (`clearPatternBlocks`, K 1.5).
const BROKEN: i32 = 2001;
/// The iron golem's data 16, its flags, and the bit of a player's golem (K 1.2).
const IRON_FLAGS: u8 = 16;
const PLAYER_CREATED: u8 = 1;
/// The snow golem's data 16 and its pumpkin's bit (K 1.4).
const SNOW_FLAGS: u8 = 16;
const PUMPKIN: u8 = 0x10;
/// A villager's data (K 2.2): baby 16, unhappy counter 18, its `VillagerData` 19 and
/// whether its type is final 20.
const UNHAPPY: u8 = 18;
const VILLAGER_DATA: u8 = 19;
const FINALIZED: u8 = 20;
/// `setUnhappy`'s counter (K 2.5).
const UNHAPPY_FOR: i32 = 40;
/// The iron golem's attack entity event (K 1.2, `doHurtTarget` step 1).
const GOLEM_ATTACK: u8 = 4;
/// The shared flags' invisible bit.
const INVISIBLE: u8 = 0x20;
/// `EquipmentSlot.MAINHAND`'s ordinal.
const MAIN_HAND: u8 = 0;

/// The clock at noon and at midnight of the first day, and at noon of the next.
const NOON: WorldClock = WorldClock {
    game_time: 6_000,
    day: 6_000,
};
const MIDNIGHT: WorldClock = WorldClock {
    game_time: 18_000,
    day: 18_000,
};
const NEXT_NOON: WorldClock = WorldClock {
    game_time: 30_000,
    day: 30_000,
};

const FLOOR: f64 = -60.0;

// ------------------------------------------------------------------------------------
// Scenes.

/// The id of the biome `name` in the registry the floor's sections are filled from.
fn biome(name: &str) -> u16 {
    let index = clustine_data::synced_registry("minecraft:worldgen/biome")
        .expect("the biomes")
        .entries
        .iter()
        .position(|entry| *entry == name)
        .unwrap_or_else(|| panic!("{name} is a biome"));
    u16::try_from(index).expect("fits")
}

/// A world of `blocks` and `entities` on the floor, in the biome `name`.
fn world_in(
    name: &str,
    set: &[(BlockPos, clustine_data::BlockState)],
    entities: &[Entity],
) -> World {
    let mut world = World::of(set).with_entities(entities);
    world.biome = biome(name);
    world
}

/// One region over `world` at `clock`, player 1 at `feet` holding `held` from slot 0 on.
fn scene_at(world: &World, held: &[ItemStack], feet: Vec3, clock: WorldClock) -> Scene {
    let slots: Vec<(usize, ItemStack)> = held.iter().cloned().enumerate().collect();
    let inventory = inventory(&slots);
    let layout = Layout::alone();
    let region = region_at(&layout, world, inventory.clone(), Some(clock));
    let mut scene = Scene {
        region,
        layout,
        inventory,
        actors: Vec::new(),
    };
    scene.join(1, feet);
    scene
}

fn one(item: i32) -> ItemStack {
    stack(item, 1)
}

/// Hatches one of `kind` by the egg player 1 holds in `slot`, used on the top of the
/// block `on`: its key.
fn hatch(scene: &mut Scene, slot: u8, on: BlockPos, kind: i32) -> EntityId {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(slot);
    scene.click(on, Face::Top);
    let made: Vec<EntityId> = scene
        .of_kind(kind)
        .iter()
        .map(|entity| entity.key)
        .filter(|key| !before.contains(key))
        .collect();
    assert_eq!(made.len(), 1, "the egg makes one of kind {kind}");
    made[0]
}

/// A record of a fresh mob of `kind` from its egg `egg`, hatched in a scratch region of
/// the biome `name` at `clock`.
fn fresh(kind: i32, egg: i32, name: &str, clock: WorldClock) -> Entity {
    let mut scene = scene_at(&world_in(name, &[], &[]), &[one(egg)], SPAWN, clock);
    let key = hatch(&mut scene, 0, bp(3, -61, 3), kind);
    record(&scene, key).expect("hatched")
}

/// `record` as entity `n` at `at`, facing south, at rest.
fn set(record: &Entity, n: u32, at: [f64; 3]) -> Entity {
    let mut entity = record.clone();
    entity.key = key(n);
    entity.uuid = Uuid::from_u128(u128::from(n) + 7_000);
    entity.position = at;
    entity.motion = [0.0; 3];
    entity.yaw = 0.0;
    entity.pitch = 0.0;
    entity.on_ground = true;
    entity.fall_distance = 0.0;
    entity.riding = Default::default();
    entity
}

fn record(scene: &Scene, key: EntityId) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key)
}

fn value(entity: &Entity, at: u8) -> Option<DataShown> {
    mob_values(entity.kind, &entity.data)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, _)| value)
}

fn health(entity: &Entity) -> f32 {
    match value(entity, index::HEALTH) {
        Some(DataShown::Float(health)) => health,
        other => panic!("health: {other:?}"),
    }
}

fn health_of(scene: &Scene, key: EntityId) -> f32 {
    health(&record(scene, key).expect("there"))
}

fn byte(entity: &Entity, at: u8) -> u8 {
    match value(entity, at) {
        Some(DataShown::Byte(byte)) => byte,
        other => panic!("byte at {at}: {other:?}"),
    }
}

fn int(entity: &Entity, at: u8) -> i32 {
    match value(entity, at) {
        Some(DataShown::Int(int)) => int,
        other => panic!("int at {at}: {other:?}"),
    }
}

fn villager_data(entity: &Entity) -> (i32, i32, i32) {
    match value(entity, VILLAGER_DATA) {
        Some(DataShown::VillagerData {
            kind,
            profession,
            level,
        }) => (kind, profession, level),
        other => panic!("villager data: {other:?}"),
    }
}

fn is_baby(entity: &Entity) -> bool {
    match value(entity, index::BABY) {
        Some(DataShown::Boolean(baby)) => baby,
        other => panic!("baby: {other:?}"),
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Player 1 uses the entity `key` with what they hold in the main hand.
fn interact(scene: &mut Scene, key: EntityId) -> TickOutput {
    scene.input(PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    })
}

/// Whether the sound `sound` is played in the tick, at a place or at an entity.
fn heard(output: &TickOutput, sound: i32) -> bool {
    output.events.iter().any(|event| match event {
        RegionEvent::Sound { sound: played, .. } => *played == sound,
        RegionEvent::EntitySound { sound: played, .. } => *played == sound,
        _ => false,
    })
}

/// The pitch the sound `sound` plays at in the tick, wherever it plays.
fn pitch_of(output: &TickOutput, sound: i32) -> Option<f32> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::Sound {
            sound: played,
            pitch,
            ..
        }
        | RegionEvent::EntitySound {
            sound: played,
            pitch,
            ..
        } if *played == sound => Some(*pitch),
        _ => None,
    })
}

fn entity_events(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
            _ => None,
        })
        .collect()
}

/// The blocks set to a layer of snow in the tick.
fn snow_set(output: &TickOutput) -> Vec<BlockPos> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } if *state == blocks::SNOW => Some(*position),
            _ => None,
        })
        .collect()
}

/// A pool of water five deep from x, z = 4 to 11 (y = -61 to -57), stone under it and
/// round it.
fn pool() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut placed = Vec::new();
    for x in 3..=12 {
        for z in 3..=12 {
            placed.push((bp(x, -62, z), blocks::STONE));
            for y in -61..=-57 {
                let wall = x == 3 || x == 12 || z == 3 || z == 12;
                placed.push((
                    bp(x, y, z),
                    if wall { blocks::STONE } else { blocks::WATER },
                ));
            }
        }
    }
    placed
}

/// Stone walls three high round x, z = 2 to 13, with a roof over them where `roofed`.
fn pen(roofed: bool) -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut placed = Vec::new();
    for x in 1..=14 {
        for z in 1..=14 {
            if x == 1 || x == 14 || z == 1 || z == 14 {
                for y in -60..=-58 {
                    placed.push((bp(x, y, z), blocks::STONE));
                }
            }
            if roofed {
                placed.push((bp(x, -57, z), blocks::STONE));
            }
        }
    }
    placed
}

// ------------------------------------------------------------------------------------
// Building golems (K 1.5 and 11.7; the plan's 2.6 and 10).

/// An iron golem's cells: the head, the four iron blocks, the four that must be air,
/// and the cell at the bottom of the middle column, where the golem stands.
struct Iron {
    head: BlockPos,
    iron: [BlockPos; 4],
    air: [BlockPos; 4],
    leg: BlockPos,
}

impl Iron {
    /// The iron golem whose head is at `head`, whose body runs from it along `down` and
    /// whose arms run along `across`.
    fn at(head: BlockPos, down: [i32; 3], across: [i32; 3]) -> Self {
        let step = |from: BlockPos, by: [i32; 3], times: i32| {
            from.offset(by[0] * times, by[1] * times, by[2] * times)
        };
        let body = step(head, down, 1);
        let leg = step(head, down, 2);
        Self {
            head,
            iron: [step(body, across, -1), body, step(body, across, 1), leg],
            air: [
                step(head, across, -1),
                step(head, across, 1),
                step(leg, across, -1),
                step(leg, across, 1),
            ],
            leg,
        }
    }

    fn blocks(&self) -> Vec<(BlockPos, clustine_data::BlockState)> {
        self.iron
            .iter()
            .map(|at| (*at, blocks::IRON_BLOCK))
            .collect()
    }
}

/// Where player 1 stands to place the pumpkins, clear of every shape the tests build.
const BUILDER: Vec3 = Vec3::new(4.5, FLOOR, 7.5);

/// Player 1 places what they hold in slot 0 at `head`, by a click on the block below
/// it, or on the floor's top where the head is on the floor.
fn place_head(scene: &mut Scene, head: BlockPos) -> TickOutput {
    scene.select(0);
    scene.click(head.offset(0, -1, 0), Face::Top)
}

/// The blocks broken in the tick, with the state id each event names.
fn broken(output: &TickOutput) -> Vec<(BlockPos, i32)> {
    let mut found: Vec<(BlockPos, i32)> = level_events(output)
        .into_iter()
        .filter(|(kind, _, _)| *kind == BROKEN)
        .map(|(_, at, data)| (at, data))
        .collect();
    found.sort_by_key(|(at, _)| (at.x, at.y, at.z));
    found
}

fn state_id(state: clustine_data::BlockState) -> i32 {
    i32::from(state.0)
}

/// Whether `data` is the state id of a state of the block `block`.
fn is_state_of(data: i32, block: clustine_data::BlockState) -> bool {
    let state = clustine_data::BlockState(u16::try_from(data).expect("a state id"));
    state.block_id() == block.block_id()
}

/// Where a golem made in the tick was put: the pose its spawn reports.
fn spawned_at(output: &TickOutput, kind: i32) -> Vec<(EntityId, Vec3, f32, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state)
                if matches!(state.kind, EntityKind::Other { kind: made, .. } if i32::from(made) == kind) =>
            {
                Some((
                state.entity,
                state.pose.position,
                state.pose.yaw,
                state.pose.pitch,
            ))
            }
            _ => None,
        })
        .collect()
}

fn standing_at(cell: BlockPos) -> Vec3 {
    Vec3::new(
        f64::from(cell.x) + 0.5,
        f64::from(cell.y) + 0.05,
        f64::from(cell.z) + 0.5,
    )
}

#[test]
fn an_iron_golem_is_built_upright_either_way_lying_either_way_and_upside_down() {
    let shapes = [
        (
            "upright, arms along x",
            Iron::at(bp(4, -58, 4), [0, -1, 0], [1, 0, 0]),
        ),
        (
            "upright, arms along z",
            Iron::at(bp(4, -58, 4), [0, -1, 0], [0, 0, 1]),
        ),
        (
            "lying, head north",
            Iron::at(bp(4, -60, 2), [0, 0, 1], [1, 0, 0]),
        ),
        (
            "lying, head west",
            Iron::at(bp(2, -60, 4), [1, 0, 0], [0, 0, 1]),
        ),
        ("upside down", Iron::at(bp(4, -60, 4), [0, 1, 0], [1, 0, 0])),
    ];
    for (named, shape) in shapes {
        for head in [blocks::CARVED_PUMPKIN, blocks::JACK_O_LANTERN] {
            let item = if head == blocks::CARVED_PUMPKIN {
                items::CARVED_PUMPKIN
            } else {
                items::JACK_O_LANTERN
            };
            let mut scene = scene_at(&World::of(&shape.blocks()), &[one(item)], BUILDER, NOON);
            let output = place_head(&mut scene, shape.head);

            // Every cell of the pattern is broken as it was, the air beside it too.
            let found = broken(&output);
            let mut wanted: Vec<BlockPos> = shape
                .iron
                .iter()
                .chain(&shape.air)
                .copied()
                .chain([shape.head])
                .collect();
            wanted.sort_by_key(|at| (at.x, at.y, at.z));
            let cells: Vec<BlockPos> = found.iter().map(|(at, _)| *at).collect();
            assert_eq!(cells, wanted, "{named}: the cells broken");
            for (at, data) in &found {
                if shape.iron.contains(at) {
                    assert_eq!(*data, state_id(blocks::IRON_BLOCK), "{named}: {at:?}");
                } else if shape.air.contains(at) {
                    assert_eq!(*data, state_id(blocks::AIR), "{named}: {at:?}");
                } else {
                    assert!(is_state_of(*data, head), "{named}: the head's event {data}");
                }
            }
            for at in shape.iron.iter().chain([&shape.head]) {
                assert_eq!(scene.block(*at), blocks::AIR, "{named}: {at:?} is cleared");
            }

            // One golem, at the bottom of the middle column, facing south, a player's.
            let made = spawned_at(&output, entity_types::IRON_GOLEM);
            assert_eq!(made.len(), 1, "{named}: one golem");
            let (golem, at, yaw, pitch) = made[0];
            assert_eq!(at, standing_at(shape.leg), "{named}: where it stands");
            assert_eq!((yaw, pitch), (0.0, 0.0), "{named}");
            let golem = record(&scene, golem).expect("there");
            assert_eq!(byte(&golem, IRON_FLAGS) & PLAYER_CREATED, PLAYER_CREATED);
            assert_eq!(health(&golem), 100.0);
        }
    }
}

#[test]
fn a_pumpkin_on_an_iron_pattern_with_a_shoulder_that_is_not_air_builds_nothing() {
    let shape = Iron::at(bp(4, -58, 4), [0, -1, 0], [1, 0, 0]);
    let mut set = shape.blocks();
    set.push((shape.air[0], blocks::STONE));
    let mut scene = scene_at(
        &World::of(&set),
        &[one(items::CARVED_PUMPKIN)],
        BUILDER,
        NOON,
    );
    let output = place_head(&mut scene, shape.head);
    assert!(scene.of_kind(entity_types::IRON_GOLEM).is_empty());
    assert!(broken(&output).is_empty());
    for at in shape.iron {
        assert_eq!(scene.block(at), blocks::IRON_BLOCK);
    }
    assert!(is_state_of(
        state_id(scene.block(shape.head)),
        blocks::CARVED_PUMPKIN
    ));
}

#[test]
fn a_snow_golem_is_built_upright_lying_and_upside_down_with_its_pumpkin() {
    // Head, the two snow blocks from it, and the cell the golem stands in.
    let shapes = [
        ("upright", bp(4, -58, 4), [bp(4, -59, 4), bp(4, -60, 4)]),
        (
            "lying, head north",
            bp(4, -60, 2),
            [bp(4, -60, 3), bp(4, -60, 4)],
        ),
        (
            "lying, head east",
            bp(6, -60, 4),
            [bp(5, -60, 4), bp(4, -60, 4)],
        ),
        ("upside down", bp(4, -60, 4), [bp(4, -59, 4), bp(4, -58, 4)]),
    ];
    for (named, head, snow) in shapes {
        let set: Vec<_> = snow.iter().map(|at| (*at, blocks::SNOW_BLOCK)).collect();
        let world = world_in("minecraft:plains", &set, &[]);
        let mut scene = scene_at(&world, &[one(items::CARVED_PUMPKIN)], BUILDER, NOON);
        let output = place_head(&mut scene, head);
        let found = broken(&output);
        let mut wanted = vec![head, snow[0], snow[1]];
        wanted.sort_by_key(|at| (at.x, at.y, at.z));
        assert_eq!(
            found.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
            wanted,
            "{named}: the cells broken"
        );
        for (at, data) in &found {
            if *at == head {
                assert!(is_state_of(*data, blocks::CARVED_PUMPKIN), "{named}");
            } else {
                assert_eq!(*data, state_id(blocks::SNOW_BLOCK), "{named}: {at:?}");
            }
        }
        let made = spawned_at(&output, entity_types::SNOW_GOLEM);
        assert_eq!(made.len(), 1, "{named}: one golem");
        let (golem, at, yaw, pitch) = made[0];
        assert_eq!(at, standing_at(snow[1]), "{named}: where it stands");
        assert_eq!((yaw, pitch), (0.0, 0.0), "{named}");
        let golem = record(&scene, golem).expect("there");
        assert_eq!(byte(&golem, SNOW_FLAGS) & PUMPKIN, PUMPKIN, "{named}");
        assert_eq!(health(&golem), 4.0, "{named}");
    }
}

#[test]
fn a_copper_golem_is_built_lying_down_and_stands_at_its_pumpkin() {
    // The plan's 11.3: at the pumpkin's cell, not at the bottom of the column.
    let head = bp(4, -60, 4);
    let copper = bp(5, -60, 4);
    let mut scene = scene_at(
        &World::of(&[(copper, blocks::COPPER_BLOCK)]),
        &[one(items::CARVED_PUMPKIN)],
        BUILDER,
        NOON,
    );
    let output = place_head(&mut scene, head);
    let made = spawned_at(&output, entity_types::COPPER_GOLEM);
    assert_eq!(made.len(), 1, "one copper golem: {:?}", output.events);
    assert_eq!(made[0].1, standing_at(head));
    assert_eq!(scene.block(head), blocks::AIR);
    let chest = scene.block(copper);
    assert!(
        chest
            .block()
            .is_some_and(|block| block.name == "minecraft:copper_chest"),
        "the copper block becomes its chest: {chest:?}"
    );
}

// ------------------------------------------------------------------------------------
// Two regions at a pinned line, at block x = 16.

/// Both sides of the line over `world` at `clock`: the west with player 1 at `feet`
/// holding `held`, the east with nobody.
struct Line {
    west: Scene,
    east: Scene,
    sequence: u64,
}

fn side(
    west: bool,
    world: &World,
    held: &[ItemStack],
    feet: Option<Vec3>,
    clock: WorldClock,
) -> Scene {
    let slots: Vec<(usize, ItemStack)> = held.iter().cloned().enumerate().collect();
    let inventory = inventory(&slots);
    let layout = Layout::side(west);
    let region = region_at(&layout, world, inventory.clone(), Some(clock));
    let mut scene = Scene {
        region,
        layout,
        inventory,
        actors: Vec::new(),
    };
    if let Some(feet) = feet {
        scene.join(1, feet);
    }
    scene
}

fn line(world: &World, held: &[ItemStack], feet: Vec3, clock: WorldClock) -> Line {
    let mut east = side(false, world, &[], None, clock);
    // The east's mobs settle on the ground with a few ticks of their own, as a holder's
    // have long done when a hit reaches them.
    east.idle_for(3);
    Line {
        west: side(true, world, held, Some(feet), clock),
        east,
        sequence: 100,
    }
}

impl Line {
    /// The store files `effects` for the east, which takes them in one tick.
    fn file_east(&mut self, effects: &[Effect]) -> TickOutput {
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
        self.east.region.tick(&TickInputs {
            effects: filed,
            ..TickInputs::default()
        })
    }
}

#[test]
fn a_pattern_whose_first_match_crosses_a_pinned_line_builds_nothing_and_no_later_match_is_tried() {
    // An iron golem lying west of the head, wholly in the west's land; two snow blocks
    // lying east of the same head, beyond the line. The snow golem is looked for first.
    let head = bp(15, -60, 4);
    let iron = Iron::at(head, [-1, 0, 0], [0, 0, 1]);
    let snow = [
        (bp(16, -60, 4), blocks::SNOW_BLOCK),
        (bp(17, -60, 4), blocks::SNOW_BLOCK),
    ];
    let feet = Vec3::new(12.5, FLOOR, 7.5);
    let mut set = iron.blocks();
    set.extend(snow);
    let mut sides = line(&World::of(&set), &[one(items::CARVED_PUMPKIN)], feet, NOON);
    let output = place_head(&mut sides.west, head);
    assert!(sides.west.of_kind(entity_types::SNOW_GOLEM).is_empty());
    assert!(
        sides.west.of_kind(entity_types::IRON_GOLEM).is_empty(),
        "no later match is tried"
    );
    assert!(broken(&output).is_empty());
    assert!(is_state_of(
        state_id(sides.west.block(head)),
        blocks::CARVED_PUMPKIN
    ));
    for at in iron.iron {
        assert_eq!(sides.west.block(at), blocks::IRON_BLOCK);
    }
    // Nothing beyond is cleared.
    assert!(
        output
            .effects
            .iter()
            .all(|effect| !matches!(effect.what, Effected::SetIf { .. }) || effect.at.x < 16),
        "{:?}",
        output.effects
    );

    // Without the snow beyond, the iron golem is the first match, and is built.
    let mut sides = line(
        &World::of(&iron.blocks()),
        &[one(items::CARVED_PUMPKIN)],
        feet,
        NOON,
    );
    place_head(&mut sides.west, head);
    assert_eq!(sides.west.of_kind(entity_types::IRON_GOLEM).len(), 1);
}

#[test]
fn an_iron_golem_whose_arm_is_across_a_pinned_line_is_not_built() {
    // Upright at the line: its east arm and shoulders in the east's land.
    let iron = Iron::at(bp(15, -58, 4), [0, -1, 0], [1, 0, 0]);
    let feet = Vec3::new(12.5, FLOOR, 7.5);
    let mut sides = line(
        &World::of(&iron.blocks()),
        &[one(items::CARVED_PUMPKIN)],
        feet,
        NOON,
    );
    let output = place_head(&mut sides.west, iron.head);
    assert!(sides.west.of_kind(entity_types::IRON_GOLEM).is_empty());
    assert!(broken(&output).is_empty());
    assert_eq!(sides.west.block(iron.iron[0]), blocks::IRON_BLOCK);
    assert_eq!(sides.west.block(iron.iron[1]), blocks::IRON_BLOCK);
}

// ------------------------------------------------------------------------------------
// The iron golem (K 1.2 and 1.3).

/// A pen of stone two high round x, z = 3 to 5, so that a golem built in it stays in
/// reach.
fn small_pen() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut placed = Vec::new();
    for along in 2..=6 {
        for (x, z) in [(along, 2), (along, 6), (2, along), (6, along)] {
            for y in [-60, -59] {
                placed.push((bp(x, y, z), blocks::STONE));
            }
        }
    }
    placed.sort_by_key(|(at, _)| (at.x, at.y, at.z));
    placed.dedup();
    placed
}

/// Player 1 hits `key` with what they hold, after time enough for a full swing.
fn hit(scene: &mut Scene, key: EntityId) -> TickOutput {
    scene.idle_for(20);
    scene.input(PlayerInput::Attack { entity: key })
}

#[test]
fn an_iron_ingot_repairs_a_hurt_iron_golem_by_25_and_a_whole_one_not_at_all() {
    let shape = Iron::at(bp(4, -58, 4), [0, -1, 0], [1, 0, 0]);
    let mut set = shape.blocks();
    set.extend(small_pen());
    let mut scene = scene_at(
        &World::of(&set),
        &[
            one(items::CARVED_PUMPKIN),
            one(items::IRON_SWORD),
            one(items::IRON_INGOT),
        ],
        BUILDER,
        NOON,
    );
    let output = place_head(&mut scene, shape.head);
    let golem = spawned_at(&output, entity_types::IRON_GOLEM)[0].0;
    scene.idle_for(5);

    // Hit with an iron sword (6) four times: to 76, no crack yet; a fifth takes it below
    // three quarters, and its damage sound plays (K 11.6).
    scene.select(1);
    let mut cracked = Vec::new();
    for _ in 0..5 {
        let output = hit(&mut scene, golem);
        assert!(heard(&output, sounds::ENTITY_IRON_GOLEM_HURT), "it is hurt");
        cracked.push(heard(&output, sounds::ENTITY_IRON_GOLEM_DAMAGE));
    }
    assert_eq!(health_of(&scene, golem), 70.0);
    assert_eq!(cracked, [false, false, false, false, true]);

    // An ingot heals 25, with the repair's sound at its drawn pitch.
    scene.select(2);
    let output = interact(&mut scene, golem);
    assert_eq!(health_of(&scene, golem), 95.0);
    let pitch = pitch_of(&output, sounds::ENTITY_IRON_GOLEM_REPAIR).expect("the repair sound");
    assert!((0.8..=1.2).contains(&pitch), "{pitch}");
    // A creative player keeps the ingot.
    assert_eq!(scene.holds(2), Some(one(items::IRON_INGOT)));
    // The next heals it to its most, and then an ingot does nothing.
    interact(&mut scene, golem);
    assert_eq!(health_of(&scene, golem), 100.0);
    let output = interact(&mut scene, golem);
    assert!(!heard(&output, sounds::ENTITY_IRON_GOLEM_REPAIR));
    assert_eq!(health_of(&scene, golem), 100.0);
}

#[test]
fn an_iron_golem_never_drowns() {
    let golem = fresh(
        entity_types::IRON_GOLEM,
        items::IRON_GOLEM_SPAWN_EGG,
        "minecraft:plains",
        NOON,
    );
    let golem = set(&golem, 1, [7.5, -61.0, 7.5]);
    let world = world_in("minecraft:plains", &pool(), &[golem]);
    let mut scene = scene_at(&world, &[], Vec3::new(7.5, -56.0, 1.5), NOON);
    scene.idle_for(600);
    let golem = record(&scene, key(1)).expect("there");
    assert!(golem.position[1] < -59.0, "it sinks: {:?}", golem.position);
    assert_eq!(int(&golem, index::AIR_SUPPLY), 300);
    assert_eq!(health(&golem), 100.0);
}

/// The zombie and the iron golem, both from eggs at midnight, the zombie `apart` east of
/// the golem.
fn golem_and(kind: i32, egg: i32, apart: i32) -> (Scene, EntityId, EntityId) {
    let mut scene = scene_at(
        &World::default(),
        &[one(items::IRON_GOLEM_SPAWN_EGG), one(egg)],
        Vec3::new(6.5, FLOOR, 7.5),
        MIDNIGHT,
    );
    let golem = hatch(&mut scene, 0, bp(3, -61, 4), entity_types::IRON_GOLEM);
    let foe = hatch(&mut scene, 1, bp(3 + apart, -61, 4), kind);
    (scene, golem, foe)
}

#[test]
fn an_iron_golem_fights_a_zombie_hitting_for_seven_and_a_half_and_a_draw_and_flinging_it() {
    let (mut scene, golem, zombie) = golem_and(entity_types::ZOMBIE, items::ZOMBIE_SPAWN_EGG, 6);
    let mut seen = Vec::new();
    let hit = scene.idle_until(600, &mut seen, |scene, _| {
        record(scene, zombie).is_none_or(|zombie| health(&zombie) < 20.0)
    });
    assert!(hit, "the golem hits the zombie within 600 ticks");
    let output = seen.last().expect("a tick");
    assert!(
        entity_events(output, golem).contains(&GOLEM_ATTACK),
        "its arms swing"
    );
    assert!(heard(output, sounds::ENTITY_IRON_GOLEM_ATTACK));
    let Some(after) = record(&scene, zombie) else {
        // Killed at a blow: only a draw of 13 or 14 does that.
        return;
    };
    // Half its attack of 15 and a draw below 15, less what the zombie's armour of 2
    // takes off a hit that hard (a fifth of it, of 25).
    let lost = 20.0 - health(&after);
    let blow = lost / (1.0 - 0.4 / 25.0);
    let drawn = blow - 7.5;
    assert!(
        (0.0..=14.0).contains(&drawn) && (drawn - drawn.round()).abs() < 1e-3,
        "{lost} lost"
    );
    // The hit's own knockback lifts it at most 0.4; the fling adds 0.4 more.
    assert!(after.motion[1] > 0.5, "flung: {:?}", after.motion);
}

#[test]
fn an_iron_golem_leaves_a_creeper_alone() {
    let (mut scene, golem, creeper) = golem_and(entity_types::CREEPER, items::CREEPER_SPAWN_EGG, 3);
    for _ in 0..600 {
        let output = scene.idle();
        assert!(!entity_events(&output, golem).contains(&GOLEM_ATTACK));
    }
    assert_eq!(health_of(&scene, creeper), 20.0);
}

#[test]
fn across_a_pinned_line_an_iron_golem_s_hit_carries_its_fling_which_lands_once_where_the_hurt_takes()
 {
    let zombie = fresh(
        entity_types::ZOMBIE,
        items::ZOMBIE_SPAWN_EGG,
        "minecraft:plains",
        MIDNIGHT,
    );
    let zombie = set(&zombie, 2, [16.9, FLOOR, 8.5]);
    let world = World::default().with_entities(&[zombie]);
    let mut sides = line(
        &world,
        &[one(items::IRON_GOLEM_SPAWN_EGG)],
        Vec3::new(10.5, FLOOR, 3.5),
        MIDNIGHT,
    );
    let golem = hatch(&mut sides.west, 0, bp(12, -61, 8), entity_types::IRON_GOLEM);
    let mut sent = None;
    for _ in 0..600 {
        let output = sides.west.idle();
        sent = output
            .effects
            .iter()
            .find(|&effect| {
                matches!(
                    effect.what,
                    Effected::OnEntity { key: hit, by: EntityUse::MobHit { .. } } if hit == key(2)
                )
            })
            .cloned();
        if sent.is_some() {
            assert!(
                heard(&output, sounds::ENTITY_IRON_GOLEM_ATTACK),
                "played at the golem"
            );
            assert!(entity_events(&output, golem).contains(&GOLEM_ATTACK));
            break;
        }
    }
    let sent = sent.expect("the golem hits the zombie across the line");
    let Effected::OnEntity {
        by:
            EntityUse::MobHit {
                attacker,
                kind,
                damage,
                fling,
                ..
            },
        ..
    } = sent.what
    else {
        unreachable!()
    };
    assert_eq!(attacker, golem);
    assert_eq!(i32::from(kind), entity_types::IRON_GOLEM);
    let damage = damage.0;
    assert!(
        (7.5..=21.5).contains(&damage) && (damage - 7.5).fract() == 0.0,
        "{damage}"
    );
    assert!(fling.is_some(), "the hit carries the fling");
    // The golem's region does not hurt the zombie's copy itself.
    let copy = sides
        .west
        .region
        .copy(clustine_world::ChunkPos::new(1, 0))
        .expect("the copy")
        .entities()
        .iter()
        .find(|entity| entity.key == key(2))
        .cloned()
        .expect("the zombie's copy");
    assert_eq!(health(&copy), 20.0);

    // The holder: one hit takes, and flings; the same hit twice in a tick takes once and
    // flings once.
    let mut once = line(&world, &[], Vec3::new(10.5, FLOOR, 3.5), MIDNIGHT);
    once.file_east(std::slice::from_ref(&sent));
    let mut twice = line(&world, &[], Vec3::new(10.5, FLOOR, 3.5), MIDNIGHT);
    twice.file_east(&[sent.clone(), sent]);
    let after_once = record(&once.east, key(2));
    let after_twice = record(&twice.east, key(2));
    match (after_once, after_twice) {
        (Some(once), Some(twice)) => {
            assert!(health(&once) < 20.0, "the hurt took");
            assert_eq!(health(&once), health(&twice), "the second did not take");
            assert!(once.motion[1] > 0.5, "flung: {:?}", once.motion);
            assert_eq!(once.motion, twice.motion, "flung once");
        }
        (None, None) => {
            // Killed at a blow, by a draw of 13 or 14: the fling cannot be seen.
            assert!(damage >= 20.5, "{damage}");
        }
        other => panic!("the same either way: {other:?}"),
    }
}

// ------------------------------------------------------------------------------------
// The snow golem (K 1.4).

#[test]
fn a_snow_golem_leaves_a_layer_of_snow_where_it_stands() {
    let mut scene = scene_at(
        &world_in("minecraft:plains", &[], &[]),
        &[one(items::SNOW_GOLEM_SPAWN_EGG)],
        BUILDER,
        NOON,
    );
    let golem = hatch(&mut scene, 0, bp(4, -61, 3), entity_types::SNOW_GOLEM);
    let mut seen = Vec::new();
    let laid = scene.idle_until(5, &mut seen, |scene, _| {
        scene.block(bp(4, -60, 3)) == blocks::SNOW
    });
    assert!(laid, "snow at its feet");
    // Over the next minute it strolls, and every cell it stood on keeps its layer.
    let mut stood = Vec::new();
    for _ in 0..1_200 {
        scene.idle();
        let at = record(&scene, golem).expect("there").position;
        stood.push(bp(at[0].floor() as i32, -60, at[2].floor() as i32));
    }
    stood.sort_by_key(|at| (at.x, at.z));
    stood.dedup();
    assert!(stood.len() > 1, "it strolls: {stood:?}");
    for at in stood {
        assert_eq!(scene.block(at), blocks::SNOW, "{at:?}");
    }
    assert_eq!(health_of(&scene, golem), 4.0, "the plains do not melt it");
}

/// The losses of health of the mob `key` over `ticks` ticks, or until it is gone: the
/// tick of each (counted from 1), what it lost, and whether its hurt sound `sound` played
/// in that tick, where it lived; and whether it was gone by the end.
fn losses(
    scene: &mut Scene,
    key: EntityId,
    ticks: u32,
    sound: i32,
) -> (Vec<(u32, f32, bool)>, bool) {
    let mut last = health_of(scene, key);
    let mut lost = Vec::new();
    for tick in 1..=ticks {
        let output = scene.idle();
        let Some(now) = record(scene, key) else {
            return (lost, true);
        };
        let now = health(&now);
        if now < last {
            // A hurt that kills plays the death's sound instead.
            lost.push((tick, last - now, now <= 0.0 || heard(&output, sound)));
        }
        last = now;
    }
    (lost, false)
}

/// That `lost` is a hurt of 1 every ten ticks, each with its sound, as a hurt of 1 a
/// tick is when its cooldown lets it take (K 1.4).
fn every_ten_ticks(lost: &[(u32, f32, bool)]) {
    assert!(lost.len() >= 2, "{lost:?}");
    for (_, amount, sound) in lost {
        assert_eq!((*amount, *sound), (1.0, true), "{lost:?}");
    }
    for pair in lost.windows(2) {
        assert_eq!(pair[1].0 - pair[0].0, 10, "{lost:?}");
    }
}

#[test]
fn a_snow_golem_melts_in_a_desert_and_lays_snow_there_still() {
    let mut scene = scene_at(
        &world_in("minecraft:desert", &[], &[]),
        &[one(items::SNOW_GOLEM_SPAWN_EGG)],
        BUILDER,
        NOON,
    );
    let golem = hatch(&mut scene, 0, bp(4, -61, 3), entity_types::SNOW_GOLEM);
    assert_eq!(
        scene.block(bp(4, -60, 3)),
        blocks::SNOW,
        "snow whether it melts or not"
    );
    let (lost, gone) = losses(&mut scene, golem, 60, sounds::ENTITY_SNOW_GOLEM_HURT);
    every_ten_ticks(&lost);
    assert!(gone, "it melts away: {lost:?}");
}

#[test]
fn a_snow_golem_in_water_is_hurt_by_it_every_ten_ticks_and_lays_no_snow() {
    let golem = fresh(
        entity_types::SNOW_GOLEM,
        items::SNOW_GOLEM_SPAWN_EGG,
        "minecraft:plains",
        NOON,
    );
    let golem = set(&golem, 1, [7.5, -61.0, 7.5]);
    let world = world_in("minecraft:plains", &pool(), &[golem]);
    let mut scene = scene_at(&world, &[], Vec3::new(7.5, -56.0, 1.5), NOON);
    assert!(
        health_of(&scene, key(1)) < 4.0,
        "hurt already in its first ticks"
    );
    let (lost, _) = losses(&mut scene, key(1), 25, sounds::ENTITY_SNOW_GOLEM_HURT);
    every_ten_ticks(&lost);
    assert!(snow_set(&scene.idle()).is_empty());
    for x in 4..=11 {
        for z in 4..=11 {
            assert_ne!(scene.block(bp(x, -61, z)), blocks::SNOW);
        }
    }
}

#[test]
fn shears_take_a_snow_golem_s_pumpkin_once_and_drop_it_as_a_carved_pumpkin() {
    let mut scene = scene_at(
        &world_in("minecraft:plains", &[], &[]),
        &[one(items::SNOW_GOLEM_SPAWN_EGG), one(items::SHEARS)],
        BUILDER,
        NOON,
    );
    let golem = hatch(&mut scene, 0, bp(4, -61, 5), entity_types::SNOW_GOLEM);
    scene.select(1);
    let output = interact(&mut scene, golem);
    assert!(heard(&output, sounds::ENTITY_SNOW_GOLEM_SHEAR));
    let shorn = record(&scene, golem).expect("there");
    assert_eq!(byte(&shorn, SNOW_FLAGS) & PUMPKIN, 0);
    let dropped = items_lying(&scene.all());
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert_eq!(dropped[0].0, one(items::CARVED_PUMPKIN));
    // Dropped at its eyes (1.7 above its feet), give or take a tick's fall.
    let eyes = shorn.position[1] + 1.7;
    assert!(
        (dropped[0].1[1] - eyes).abs() < 0.2,
        "{:?} by {eyes}",
        dropped[0].1
    );
    assert_eq!(
        scene.holds(1),
        Some(one(items::SHEARS)),
        "a creative player's shears"
    );
    // Shorn, it has nothing more to give.
    let output = interact(&mut scene, golem);
    assert!(!heard(&output, sounds::ENTITY_SNOW_GOLEM_SHEAR));
    assert_eq!(lying_count(&scene.all(), items::CARVED_PUMPKIN), 1);
}

#[test]
fn a_snow_golem_throws_snowballs_at_a_zombie_which_hurt_it_not() {
    let zombie = fresh(
        entity_types::ZOMBIE,
        items::ZOMBIE_SPAWN_EGG,
        "minecraft:plains",
        MIDNIGHT,
    );
    let zombie = set(&zombie, 2, [10.5, FLOOR, 4.5]);
    let golem = fresh(
        entity_types::SNOW_GOLEM,
        items::SNOW_GOLEM_SPAWN_EGG,
        "minecraft:plains",
        MIDNIGHT,
    );
    let golem = set(&golem, 1, [3.5, FLOOR, 4.5]);
    let world = world_in("minecraft:plains", &[], &[zombie, golem]);
    let mut scene = scene_at(&world, &[], Vec3::new(4.5, FLOOR, -6.5), MIDNIGHT);
    let mut seen = Vec::new();
    let thrown = scene.idle_until(600, &mut seen, |scene, _| {
        !scene.of_kind(entity_types::SNOWBALL).is_empty()
    });
    assert!(thrown, "a snowball flies");
    assert!(
        seen.iter()
            .any(|output| heard(output, sounds::ENTITY_SNOW_GOLEM_SHOOT)),
        "with its sound"
    );
    // The snowballs hurt for nothing.
    scene.idle_for(100);
    if let Some(zombie) = record(&scene, key(2)) {
        assert_eq!(health(&zombie), 20.0);
    }
}

#[test]
fn across_a_pinned_line_a_snow_golem_s_trail_is_an_effect_expecting_air_which_the_holder_places() {
    let golem = fresh(
        entity_types::SNOW_GOLEM,
        items::SNOW_GOLEM_SPAWN_EGG,
        "minecraft:plains",
        NOON,
    );
    let golem = set(&golem, 1, [15.9, FLOOR, 8.5]);
    let world = world_in("minecraft:plains", &[], &[golem]);
    let mut sides = line(&world, &[], Vec3::new(10.5, FLOOR, 3.5), NOON);
    // Its corners are at x 15.65 and 16.15: one cell on each side.
    let mut seen = Vec::new();
    let mut beyond = Vec::new();
    for _ in 0..3 {
        let output = sides.west.idle();
        beyond.extend(
            output
                .effects
                .iter()
                .filter(|&effect| {
                    matches!(effect.what, Effected::SetIf { .. }) && effect.at.x >= 16
                })
                .cloned(),
        );
        seen.push(output);
        if !beyond.is_empty() {
            break;
        }
    }
    assert_eq!(
        sides.west.block(bp(15, -60, 8)),
        blocks::SNOW,
        "its own side's"
    );
    let [ref sent] = beyond[..] else {
        panic!("one cell beyond: {beyond:?}");
    };
    assert_eq!(sent.at, bp(16, -60, 8));
    let Effected::SetIf {
        expected, state, ..
    } = sent.what
    else {
        unreachable!()
    };
    assert_eq!((expected, state), (blocks::AIR, blocks::SNOW));
    sides.file_east(std::slice::from_ref(sent));
    sides.east.idle();
    assert_eq!(
        sides.east.block(bp(16, -60, 8)),
        blocks::SNOW,
        "the holder places it"
    );
}

// ------------------------------------------------------------------------------------
// The villager (K 2; the plan's 1.2 and 10).

#[test]
fn a_villager_from_an_egg_is_an_unemployed_adult_of_the_type_of_the_biome_at_its_feet() {
    use villagers::types::*;
    let biomes = [
        ("minecraft:badlands", DESERT),
        ("minecraft:desert", DESERT),
        ("minecraft:eroded_badlands", DESERT),
        ("minecraft:wooded_badlands", DESERT),
        ("minecraft:bamboo_jungle", JUNGLE),
        ("minecraft:jungle", JUNGLE),
        ("minecraft:sparse_jungle", JUNGLE),
        ("minecraft:savanna_plateau", SAVANNA),
        ("minecraft:savanna", SAVANNA),
        ("minecraft:windswept_savanna", SAVANNA),
        ("minecraft:deep_frozen_ocean", SNOW),
        ("minecraft:frozen_ocean", SNOW),
        ("minecraft:frozen_river", SNOW),
        ("minecraft:ice_spikes", SNOW),
        ("minecraft:snowy_beach", SNOW),
        ("minecraft:snowy_taiga", SNOW),
        ("minecraft:snowy_plains", SNOW),
        ("minecraft:grove", SNOW),
        ("minecraft:snowy_slopes", SNOW),
        ("minecraft:frozen_peaks", SNOW),
        ("minecraft:jagged_peaks", SNOW),
        ("minecraft:swamp", SWAMP),
        ("minecraft:mangrove_swamp", SWAMP),
        ("minecraft:old_growth_spruce_taiga", TAIGA),
        ("minecraft:old_growth_pine_taiga", TAIGA),
        ("minecraft:windswept_gravelly_hills", TAIGA),
        ("minecraft:windswept_hills", TAIGA),
        ("minecraft:taiga", TAIGA),
        ("minecraft:windswept_forest", TAIGA),
        // Every other biome: the plains'.
        ("minecraft:plains", PLAINS),
        ("minecraft:forest", PLAINS),
        ("minecraft:cherry_grove", PLAINS),
        ("minecraft:mushroom_fields", PLAINS),
        ("minecraft:ocean", PLAINS),
        ("minecraft:river", PLAINS),
        ("minecraft:meadow", PLAINS),
    ];
    for (name, kind) in biomes {
        let villager = fresh(
            entity_types::VILLAGER,
            items::VILLAGER_SPAWN_EGG,
            name,
            NOON,
        );
        assert_eq!(
            villager_data(&villager),
            (kind, villagers::professions::NONE, 1),
            "{name}"
        );
        assert_eq!(
            value(&villager, FINALIZED),
            Some(DataShown::Boolean(true)),
            "{name}"
        );
        assert!(!is_baby(&villager), "{name}");
        assert_eq!(int(&villager, UNHAPPY), 0);
    }
}

#[test]
fn a_click_on_a_villager_makes_it_shake_its_head_and_say_no_for_forty_ticks() {
    let mut scene = scene_at(
        &world_in("minecraft:plains", &[], &[]),
        &[one(items::VILLAGER_SPAWN_EGG)],
        BUILDER,
        NOON,
    );
    let villager = hatch(&mut scene, 0, bp(4, -61, 5), entity_types::VILLAGER);
    // An empty hand.
    scene.select(5);
    let output = interact(&mut scene, villager);
    assert!(
        heard(&output, sounds::ENTITY_VILLAGER_NO),
        "{:?}",
        output.events
    );
    let unhappy = int(&record(&scene, villager).expect("there"), UNHAPPY);
    // Set to 40 by the click, and counted down in the villager's tick, which may come
    // after it.
    assert!(
        (UNHAPPY_FOR - 1..=UNHAPPY_FOR).contains(&unhappy),
        "{unhappy}"
    );
    assert!(opened(&output, scene.actors[0].id).is_empty(), "no screen");
    // It counts down to nothing.
    scene.idle_for(40);
    assert_eq!(int(&record(&scene, villager).expect("there"), UNHAPPY), 0);
    // A use with the off hand sets nothing and plays nothing.
    let output = scene.input(PlayerInput::WithOffHand(Box::new(PlayerInput::Interact {
        entity: villager,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    })));
    assert!(!heard(&output, sounds::ENTITY_VILLAGER_NO));
    assert_eq!(int(&record(&scene, villager).expect("there"), UNHAPPY), 0);
}

/// A villager of the desert's type, from an egg in the badlands, at `at` in the plains.
fn desert_villager(n: u32, at: [f64; 3]) -> Entity {
    let made = fresh(
        entity_types::VILLAGER,
        items::VILLAGER_SPAWN_EGG,
        "minecraft:badlands",
        NOON,
    );
    set(&made, n, at)
}

#[test]
fn an_egg_on_a_villager_makes_a_baby_at_its_feet_of_the_biome_s_type_or_its_own() {
    let mut types = Vec::new();
    for round in 0..8 {
        let parent = desert_villager(1, [4.5, FLOOR, 4.5]);
        let world = world_in("minecraft:plains", &[], &[parent]);
        let mut scene = scene_at(&world, &[one(items::VILLAGER_SPAWN_EGG)], BUILDER, NOON);
        // Some ticks first, so that each round draws at another place of the stream.
        scene.idle_for(round);
        let feet = record(&scene, key(1)).expect("there").position;
        let output = interact(&mut scene, key(1));
        let made = spawned_at(&output, entity_types::VILLAGER);
        assert_eq!(made.len(), 1, "one baby");
        let (baby, at, yaw, _) = made[0];
        assert_eq!([at.x, at.y, at.z], feet, "at its parent's feet");
        assert_eq!(yaw, 0.0, "facing south");
        let baby = record(&scene, baby).expect("there");
        assert!(is_baby(&baby));
        let (kind, profession, level) = villager_data(&baby);
        assert_eq!((profession, level), (villagers::professions::NONE, 1));
        assert!(
            [villagers::types::PLAINS, villagers::types::DESERT].contains(&kind),
            "the biome's type or its parent's: {kind}"
        );
        assert_eq!(value(&baby, FINALIZED), Some(DataShown::Boolean(true)));
        types.push(kind);

        // A baby says no too.
        scene.select(5);
        let output = interact(&mut scene, baby.key);
        assert!(heard(&output, sounds::ENTITY_VILLAGER_NO));
        let unhappy = int(&record(&scene, baby.key).expect("there"), UNHAPPY);
        assert!(
            (UNHAPPY_FOR - 1..=UNHAPPY_FOR).contains(&unhappy),
            "{unhappy}"
        );
    }
    assert!(
        types.contains(&villagers::types::PLAINS) && types.contains(&villagers::types::DESERT),
        "both are drawn in eight rounds: {types:?}"
    );
}

#[test]
fn a_villager_by_a_zombie_runs_from_it() {
    // By day under a roof, where the zombie does not burn.
    let mut scene = scene_at(
        &world_in("minecraft:plains", &pen(true), &[]),
        &[one(items::VILLAGER_SPAWN_EGG), one(items::ZOMBIE_SPAWN_EGG)],
        Vec3::new(8.5, FLOOR, 11.5),
        NOON,
    );
    let villager = hatch(&mut scene, 0, bp(7, -61, 7), entity_types::VILLAGER);
    let zombie = hatch(&mut scene, 1, bp(10, -61, 7), entity_types::ZOMBIE);
    let mut seen = Vec::new();
    let ran = scene.idle_until(200, &mut seen, |scene, _| {
        let villager = record(scene, villager).expect("there").position;
        let zombie = record(scene, zombie).expect("there").position;
        distance(villager, zombie) > 6.0
    });
    assert!(ran, "it runs from the zombie");
}

#[test]
fn a_villager_hit_by_a_player_runs_from_them() {
    let mut scene = scene_at(
        &world_in("minecraft:plains", &[], &[]),
        &[one(items::VILLAGER_SPAWN_EGG)],
        Vec3::new(7.5, FLOOR, 4.5),
        NOON,
    );
    let villager = hatch(&mut scene, 0, bp(7, -61, 7), entity_types::VILLAGER);
    scene.select(5);
    let output = scene.input(PlayerInput::Attack { entity: villager });
    assert!(heard(&output, sounds::ENTITY_VILLAGER_HURT));
    let from = [7.5, FLOOR, 4.5];
    let mut seen = Vec::new();
    let ran = scene.idle_until(200, &mut seen, |scene, _| {
        distance(record(scene, villager).expect("there").position, from) > 6.0
    });
    assert!(ran, "it runs from the player");
}

#[test]
fn a_villager_in_deep_water_floats_up_and_a_zombie_sinks() {
    let villager = fresh(
        entity_types::VILLAGER,
        items::VILLAGER_SPAWN_EGG,
        "minecraft:plains",
        NOON,
    );
    let villager = set(&villager, 1, [6.5, -61.0, 7.5]);
    let zombie = fresh(
        entity_types::ZOMBIE,
        items::ZOMBIE_SPAWN_EGG,
        "minecraft:plains",
        MIDNIGHT,
    );
    let zombie = set(&zombie, 2, [9.5, -58.0, 7.5]);
    let world = world_in("minecraft:plains", &pool(), &[villager, zombie]);
    let mut scene = scene_at(&world, &[], Vec3::new(7.5, -56.0, 1.5), MIDNIGHT);
    let mut seen = Vec::new();
    let rose = scene.idle_until(100, &mut seen, |scene, _| {
        record(scene, key(1)).expect("there").position[1] > -58.5
    });
    assert!(rose, "it rises to the top, its head out");
    scene.idle_for(100);
    assert!(
        record(&scene, key(1)).expect("there").position[1] > -58.5,
        "and stays there"
    );
    // The control: a zombie, which has no float, sinks to the bottom.
    assert!(record(&scene, key(2)).expect("there").position[1] < -60.5);
}

#[test]
fn a_villager_walks_to_bread_on_the_ground_and_takes_it() {
    let villager = desert_villager(1, [4.5, FLOOR, 4.5]);
    let bread = Entity {
        portal: Default::default(),
        key: key(2),
        kind: u16::try_from(entity_types::ITEM).expect("fits"),
        uuid: Uuid::from_u128(7_002),
        // Three blocks off: `GoToWantedItem`'s reach is 4, strictly (K 2.4; the core
        // package's).
        position: [7.5, FLOOR, 4.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Item {
            stack: stack(items::BREAD, 3),
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    };
    // Walled in within the bread's reach (walls at x 3 and 8, z 2 and 7), so that a
    // stroll its stream happens to start first cannot take it beyond: which way a
    // villager strolls is a draw (V 1.6), and the game's would sometimes stroll off too.
    let floor = FLOOR as i32;
    let mut walls = Vec::new();
    for x in 3..=8 {
        for z in 2..=7 {
            if x == 3 || x == 8 || z == 2 || z == 7 {
                for y in floor..=floor + 1 {
                    walls.push((BlockPos::new(x, y, z), blocks::STONE));
                }
            }
        }
    }
    let world = world_in("minecraft:plains", &walls, &[villager, bread]);
    let mut scene = scene_at(&world, &[], Vec3::new(4.5, FLOOR, -6.5), NOON);
    let mut seen = Vec::new();
    let taken = scene.idle_until(300, &mut seen, |scene, _| record(scene, key(2)).is_none());
    assert!(taken, "the bread is taken");
    let by: Vec<EntityId> = seen
        .iter()
        .flat_map(|output| output.events.iter())
        .filter_map(|event| match event {
            RegionEvent::ItemTaken {
                item, collector, ..
            } if *item == key(2) => Some(*collector),
            _ => None,
        })
        .collect();
    assert_eq!(by, [key(1)], "by the villager");
}

#[test]
fn a_villager_at_night_under_a_roof_walks_about_and_sleeps_nowhere() {
    let villager = desert_villager(1, [7.5, FLOOR, 7.5]);
    let world = world_in("minecraft:plains", &pen(true), &[villager]);
    let mut scene = scene_at(&world, &[], Vec3::new(12.5, FLOOR, 12.5), MIDNIGHT);
    let mut places = Vec::new();
    for _ in 0..1_200 {
        let output = scene.idle();
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::Sleeping { .. })),
            "no bed, no sleep"
        );
        let at = record(&scene, key(1)).expect("there").position;
        places.push(BlockPos::new(
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        ));
    }
    places.dedup();
    assert!(places.len() > 1, "it walks about: {places:?}");
}

// ------------------------------------------------------------------------------------
// Leads, the trader, and the sun.

fn leashed(output: &TickOutput, key: EntityId) -> bool {
    output.events.iter().any(|event| {
        matches!(event, RegionEvent::Leashed { entity, holder: Some(_), .. } if *entity == key)
    })
}

#[test]
fn a_lead_goes_on_no_villager_and_no_trader_but_on_a_cow() {
    let mut scene = scene_at(
        &world_in("minecraft:plains", &[], &[]),
        &[
            one(items::VILLAGER_SPAWN_EGG),
            one(items::WANDERING_TRADER_SPAWN_EGG),
            one(items::COW_SPAWN_EGG),
            one(items::LEAD),
        ],
        BUILDER,
        NOON,
    );
    let villager = hatch(&mut scene, 0, bp(3, -61, 5), entity_types::VILLAGER);
    let trader = hatch(&mut scene, 1, bp(5, -61, 5), entity_types::WANDERING_TRADER);
    let cow = hatch(&mut scene, 2, bp(4, -61, 9), entity_types::COW);
    scene.select(3);
    for mob in [villager, trader] {
        let output = interact(&mut scene, mob);
        assert!(!leashed(&output, mob), "{mob:?}");
        assert!(items_lying(&scene.all()).is_empty());
        assert!(scene.of_kind(entity_types::LEASH_KNOT).is_empty());
    }
    let output = interact(&mut scene, cow);
    assert!(leashed(&output, cow), "the control: a cow takes a lead");
}

#[test]
fn a_click_on_a_wandering_trader_opens_nothing_and_an_egg_on_it_makes_nothing() {
    let mut scene = scene_at(
        &world_in("minecraft:plains", &[], &[]),
        &[one(items::WANDERING_TRADER_SPAWN_EGG)],
        BUILDER,
        NOON,
    );
    let trader = hatch(&mut scene, 0, bp(4, -61, 5), entity_types::WANDERING_TRADER);
    // The egg on it: no offspring.
    let output = interact(&mut scene, trader);
    assert!(spawned_at(&output, entity_types::WANDERING_TRADER).is_empty());
    assert_eq!(scene.of_kind(entity_types::WANDERING_TRADER).len(), 1);
    // An empty hand: nothing opens.
    scene.select(5);
    let output = interact(&mut scene, trader);
    let events = events_of(&output, scene.actors[0].id);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PlayerEvent::MenuOpened { .. })),
        "{events:?}"
    );
}

fn main_hand(output: &TickOutput, key: EntityId) -> Option<Option<i32>> {
    output.events.iter().rev().find_map(|event| match event {
        RegionEvent::EquipmentChanged { entity, slots, .. } if *entity == key => slots
            .iter()
            .find(|(slot, _)| *slot == MAIN_HAND)
            .map(|(_, held)| held.as_ref().map(|stack| stack.item)),
        _ => None,
    })
}

#[test]
fn a_wandering_trader_drinks_invisibility_at_night_and_milk_by_day() {
    let trader = fresh(
        entity_types::WANDERING_TRADER,
        items::WANDERING_TRADER_SPAWN_EGG,
        "minecraft:plains",
        NOON,
    );
    let trader = set(&trader, 1, [6.5, FLOOR, 6.5]);
    let world = world_in("minecraft:plains", &pen(false), &[trader]);
    let mut scene = scene_at(&world, &[], Vec3::new(12.5, FLOOR, 12.5), NOON);
    let invisible = |scene: &Scene| {
        mob_shared_flags(&record(scene, key(1)).expect("there").data) & INVISIBLE != 0
    };
    scene.idle_for(100);
    assert!(!invisible(&scene), "by day it drinks nothing");
    let night = scene.region.tick(&TickInputs {
        clock: Some(MIDNIGHT),
        ..TickInputs::default()
    });
    let mut seen = vec![night];
    let drunk = scene.idle_until(200, &mut seen, |scene, _| invisible(scene));
    assert!(drunk, "it drinks invisibility at night");
    let held: Vec<_> = seen
        .iter()
        .filter_map(|output| main_hand(output, key(1)))
        .collect();
    assert!(
        held.contains(&Some(items::POTION)),
        "a potion in its hand: {held:?}"
    );

    // The next noon: milk, and it is seen again.
    let day = scene.region.tick(&TickInputs {
        clock: Some(NEXT_NOON),
        ..TickInputs::default()
    });
    let mut seen = vec![day];
    let drunk = scene.idle_until(200, &mut seen, |scene, _| !invisible(scene));
    assert!(drunk, "it drinks milk by day");
    let held: Vec<_> = seen
        .iter()
        .filter_map(|output| main_hand(output, key(1)))
        .collect();
    assert!(
        held.contains(&Some(items::MILK_BUCKET)),
        "milk in its hand: {held:?}"
    );
}

#[test]
fn neither_golem_nor_a_villager_nor_a_trader_burns_at_noon_while_a_zombie_does() {
    let made = [
        fresh(
            entity_types::IRON_GOLEM,
            items::IRON_GOLEM_SPAWN_EGG,
            "minecraft:plains",
            NOON,
        ),
        fresh(
            entity_types::SNOW_GOLEM,
            items::SNOW_GOLEM_SPAWN_EGG,
            "minecraft:plains",
            NOON,
        ),
        fresh(
            entity_types::VILLAGER,
            items::VILLAGER_SPAWN_EGG,
            "minecraft:plains",
            NOON,
        ),
        fresh(
            entity_types::WANDERING_TRADER,
            items::WANDERING_TRADER_SPAWN_EGG,
            "minecraft:plains",
            NOON,
        ),
        fresh(
            entity_types::ZOMBIE,
            items::ZOMBIE_SPAWN_EGG,
            "minecraft:plains",
            MIDNIGHT,
        ),
    ];
    let set_out: Vec<Entity> = made
        .iter()
        .zip(1..)
        .map(|(mob, n)| set(mob, n, [3.5 + 2.0 * f64::from(n), FLOOR, 7.5]))
        .collect();
    let world = world_in("minecraft:plains", &pen(false), &set_out);
    let mut scene = scene_at(&world, &[], Vec3::new(12.5, FLOOR, 12.5), NOON);
    let mut zombie_burnt = false;
    for _ in 0..400 {
        scene.idle();
        for n in 1..=4 {
            if let Some(mob) = record(&scene, key(n)) {
                assert_eq!(mob.fire_ticks, 0, "{} does not burn", mob.kind);
            }
        }
        zombie_burnt |= record(&scene, key(5)).is_none_or(|zombie| zombie.fire_ticks > 0);
    }
    assert!(zombie_burnt, "the control: the zombie burns");
}
