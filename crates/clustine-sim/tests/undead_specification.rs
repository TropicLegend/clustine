//! Tests of the undead of step N7k, written from the specification and the public
//! interface alone, by someone who did not write them: the porting brief
//! `docs/groundwork/n7k-brief.md` (K: sections 0, 3.1, 3.3, 3.4, 5, 11.1, 11.2, 11.5,
//! 11.8 and 11.12) and the plan `docs/groundwork/n7k-plan.md` (sections 1.3, 2.2, 2.4,
//! 10 and 11, the last two holding where they differ from the rest), and the game's
//! bytecode where the brief leaves a rule open (`reference/n7-javap/`, `n7k-javap/`).
//!
//! A region is driven as a player drives it: spawn eggs, items used on mobs, hits and
//! ticks. Where a test wants a mob somewhere an egg cannot put it (under water, in powder
//! snow, at dusk), the mob is made by an egg at night on the open floor and its record
//! set into the chunk of a new region, as a region's restore reads it from its chunk.
//! What the game keeps of a mob that clients are not shown (its conversion's count) is
//! read from what it does and what it shows (its synched flags). A mob's own ticks are
//! counted by its record's `lived`, as a region ticks its mobs while a scene is set up.
//!
//! The world is the scene's: a stone floor at y = -61 with air above, feet at y = -60,
//! one region over the land near the origin, player 1 (creative) at the origin. The
//! world's difficulty is easy (plan 10, 11 item 5), so the special multiplier of every
//! `finalizeSpawn` is 0. Where light matters beyond the open sky (a roof's shade, a
//! lamp, glass), the chunks carry light as the game makes it (a flood of the sky's light
//! and the blocks', 1 less a step), as a generated or imported chunk carries it.

mod n6_scene;

use std::sync::OnceLock;

use clustine_data::{BlockState, attributes, blocks, entity_types, items, sounds, villagers};
use clustine_sim::TickOutput;
use clustine_sim::api::{Face, Inventory, Keys, PlayerInput, RegionEvent, Tracked, WorldClock};
use clustine_sim::shown::{DataShown, index, mob_shared_flags, mob_values};
use clustine_sim::time::{DAY, sky_darken};
use clustine_sim::{EdgeState, Region, RegionState, TickInputs, Ticket};
use clustine_world::{
    BlockPos, Chunk, Entity, EntityId, EntityIds, ItemStack, LIGHT_ARRAY_LENGTH, Light,
    SectionLight,
};
use n6_scene::*;
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The game's numbers.

/// Ticks of the overworld clock's day: noon, and the middle of the night.
const NOON: u64 = 6_000;
const NIGHT: u64 = 18_000;
/// `timeline/day.json`'s `gameplay/monsters_burn`: false from this tick of the day...
const BURN_ENDS: u64 = 12_542;
/// ... and true again from this one (K 3.3).
const BURN_BEGINS: u64 = 23_460;

/// `SoundSource.HOSTILE`, `SoundSource.BLOCKS` and `SoundSource.PLAYERS` (ordinals).
const HOSTILE: i32 = 5;
const BLOCKS_SOURCE: i32 = 4;
const PLAYERS_SOURCE: i32 = 7;

/// `igniteForSeconds(8)`: `Mth.floor(8 * 20)` ticks, times `burning_time` (1), rounded
/// up (K 3.3).
const SUN_FIRE: i32 = 160;

/// `Zombie.DATA_BABY_ID` and `DATA_DROWNED_CONVERSION_ID` (K 3.4.1).
const ZOMBIE_BABY: u8 = 16;
const DROWNED_CONVERSION: u8 = 18;
/// `ZombieVillager.DATA_CONVERTING_ID` and `DATA_VILLAGER_DATA` (K 3.4.3).
const VILLAGER_CONVERTING: u8 = 19;
const VILLAGER_DATA: u8 = 20;
/// `Skeleton.DATA_STRAY_CONVERSION_ID` and `Bogged.DATA_SHEARED` (K 11.2).
const STRAY_CONVERSION: u8 = 16;
const SHEARED: u8 = 16;
/// `TamableAnimal.DATA_FLAGS_ID` (after `AgeableMob`'s baby and age lock) and its tame
/// bit; `AbstractHorse.DATA_ID_FLAGS` and its tame bit.
const TAMABLE_FLAGS: u8 = 18;
const TAMABLE_TAME: u8 = 4;
const HORSE_FLAGS: u8 = 18;
const HORSE_TAME: u8 = 2;

/// The level events of a conversion (K 3.4.1, 11.2): a zombie's into a drowned, a
/// husk's into a zombie, a skeleton's into a stray.
const ZOMBIE_CONVERTED: i32 = 1040;
const HUSK_CONVERTED: i32 = 1041;
const SKELETON_CONVERTED: i32 = 1048;

/// `EquipmentSlot` ordinals.
const MAIN_HAND: u8 = 0;
const OFF_HAND: u8 = 1;
const ARMOUR: [u8; 4] = [2, 3, 4, 5];

/// The `Entity.FLAG_SWIMMING` bit of the shared flags.
const SWIMMING: u8 = 0x10;

// ------------------------------------------------------------------------------------
// Scenes.

/// The overworld clock at the day's tick `day`.
fn clock(day: u64) -> WorldClock {
    WorldClock {
        game_time: day,
        day,
    }
}

/// One region over the land near the origin with `world`, its clock at the day's tick
/// `day`, and player 1 at the origin holding `inventory`, slot 0 selected.
fn scene_at(world: &World, inventory: Inventory, day: u64) -> Scene {
    scene_on(Layout::alone(), world, inventory, day)
}

fn scene_on(layout: Layout, world: &World, inventory: Inventory, day: u64) -> Scene {
    let region = region_at(&layout, world, inventory.clone(), Some(clock(day)));
    with_player(region, layout, inventory)
}

fn with_player(region: Region, layout: Layout, inventory: Inventory) -> Scene {
    let mut scene = Scene {
        region,
        layout,
        inventory,
        actors: Vec::new(),
    };
    scene.join(1, SPAWN);
    scene
}

/// As [`scene_at`], nobody's inventory, every chunk carrying the light the game makes
/// for it ([`lit`]).
fn lit_scene_at(world: &World, day: u64) -> Scene {
    let layout = Layout::alone();
    let mut state = RegionState::new(EntityIds::block(layout.block).expect("a block"));
    state.edges.insert(
        EDGE,
        EdgeState {
            start: 1,
            ..EdgeState::default()
        },
    );
    let mut region = Region::restore(config(Inventory::default()), state, layout.holdings());
    region.tick(&TickInputs {
        tickets_added: layout
            .loaded
            .iter()
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        clock: Some(clock(day)),
        ..TickInputs::default()
    });
    region.tick(&TickInputs {
        chunks_loaded: layout
            .loaded
            .iter()
            .map(|position| (*position, lit(world.chunk(*position))))
            .collect(),
        ..TickInputs::default()
    });
    with_player(region, layout, Inventory::default())
}

/// Whether light goes through the block as through air (glass lets the sky's light
/// down whole, `propagatesSkylightDown`).
fn clear(state: BlockState) -> bool {
    state == blocks::AIR || state == blocks::GLASS
}

/// `chunk` with the light the game's rules give its blocks, for the two lowest sections
/// (y -64 to -33), which hold every block of these worlds: the sky's 15 down every
/// column to its first block that is not clear, and from there a flood, 1 less a step
/// through clear blocks; a glowstone's 15 flooded so. Above, the sky's full and no
/// blocks' light. (Each chunk is lit alone; the tests keep what light matters away
/// from a chunk's sides.)
fn lit(mut chunk: Chunk) -> Chunk {
    const LOW: i32 = -64;
    const HIGH: i32 = -32;
    let height = (HIGH - LOW) as usize;
    let at = |x: usize, y: usize, z: usize| (y * 16 + z) * 16 + x;
    let state = |chunk: &Chunk, x: usize, y: usize, z: usize| {
        chunk
            .get(x, LOW + y as i32, z)
            .expect("in the chunk's height")
    };
    let flood = |chunk: &Chunk, levels: &mut Vec<u8>| {
        let mut changed = true;
        while changed {
            changed = false;
            for y in 0..height {
                for z in 0..16 {
                    for x in 0..16 {
                        if !clear(state(chunk, x, y, z)) {
                            continue;
                        }
                        let mut best = levels[at(x, y, z)];
                        let mut take = |level: u8| best = best.max(level.saturating_sub(1));
                        if x > 0 {
                            take(levels[at(x - 1, y, z)]);
                        }
                        if x < 15 {
                            take(levels[at(x + 1, y, z)]);
                        }
                        if z > 0 {
                            take(levels[at(x, y, z - 1)]);
                        }
                        if z < 15 {
                            take(levels[at(x, y, z + 1)]);
                        }
                        if y > 0 {
                            take(levels[at(x, y - 1, z)]);
                        }
                        if y + 1 < height {
                            take(levels[at(x, y + 1, z)]);
                        }
                        if best > levels[at(x, y, z)] {
                            levels[at(x, y, z)] = best;
                            changed = true;
                        }
                    }
                }
            }
        }
    };
    let mut sky = vec![0u8; 16 * 16 * height];
    for z in 0..16 {
        for x in 0..16 {
            for y in (0..height).rev() {
                if !clear(state(&chunk, x, y, z)) {
                    break;
                }
                sky[at(x, y, z)] = 15;
            }
        }
    }
    flood(&chunk, &mut sky);
    let mut block = vec![0u8; 16 * 16 * height];
    for y in 0..height {
        for z in 0..16 {
            for x in 0..16 {
                if state(&chunk, x, y, z) == blocks::GLOWSTONE {
                    block[at(x, y, z)] = 15;
                }
            }
        }
    }
    flood(&chunk, &mut block);
    let sections = |levels: &[u8], rest: SectionLight| -> Vec<SectionLight> {
        let entries = chunk.sections().len() + 2;
        let mut out = vec![rest; entries];
        for section in 0..2 {
            let mut bytes = Box::new([0u8; LIGHT_ARRAY_LENGTH]);
            for dy in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let level = levels[at(x, section * 16 + dy, z)] & 0x0F;
                        let index = (dy << 8) | (z << 4) | x;
                        bytes[index >> 1] |= level << ((index & 1) * 4);
                    }
                }
            }
            out[section + 1] = SectionLight::of(bytes);
        }
        out
    };
    let light = Light {
        made_by: 0,
        beside: 0,
        sky: sections(&sky, SectionLight::Full),
        block: sections(&block, SectionLight::Dark),
    };
    chunk.set_light(light);
    chunk
}

/// The overworld's day tick of the clock a tick ended with, which its mobs saw (the
/// game moves its clock on before it ticks its entities, `ServerLevel.tick`).
fn day_of(output: &TickOutput) -> u64 {
    output.clock.day % DAY
}

/// Player 1 selects `slot` and uses what it holds on the top of the floor at (x, z):
/// the tick's output and the keys of the entities it made.
fn hatch(scene: &mut Scene, slot: u8, x: i32, z: i32) -> (TickOutput, Vec<EntityId>) {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    let selected = scene
        .region
        .player_state(scene.actors[0].id)
        .map(|state| state.selected_slot);
    if selected != Some(slot) {
        scene.select(slot);
    }
    let output = scene.click(bp(x, -61, z), Face::Top);
    let made = scene
        .all()
        .iter()
        .map(|entity| entity.key)
        .filter(|key| !before.contains(key))
        .collect();
    (output, made)
}

/// A use of the mob `key` with what player 1 holds (`interact`, main hand).
fn used(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    }
}

/// A mob of `kind` made by `egg` at night on the open floor and left to tick three
/// times there (so that it has been out of water and powder snow), as its region's
/// record has it.
fn made(egg: i32, kind: i32) -> Entity {
    let mut scene = scene_at(&World::default(), inventory(&[(0, stack(egg, 1))]), NIGHT);
    let (_, made) = hatch(&mut scene, 0, 2, 2);
    scene.idle_for(3);
    let found: Vec<Entity> = scene
        .of_kind(kind)
        .into_iter()
        .filter(|entity| made.contains(&entity.key))
        .collect();
    assert_eq!(found.len(), 1, "the egg makes one of kind {kind}");
    found[0].clone()
}

/// `record` as entity number `n` standing at `at`, at rest, riding nothing.
fn placed(record: &Entity, n: u32, at: [f64; 3]) -> Entity {
    let mut mob = record.clone();
    mob.key = key(n);
    mob.uuid = Uuid::from_u128(u128::from(n) + 7_000);
    mob.position = at;
    mob.motion = [0.0; 3];
    mob.on_ground = true;
    mob.fall_distance = 0.0;
    mob.riding = Default::default();
    mob
}

/// The feet of a mob standing on the floor in the middle of the column (x, z).
fn feet(x: i32, z: i32) -> [f64; 3] {
    [f64::from(x) + 0.5, -60.0, f64::from(z) + 0.5]
}

fn record(scene: &Scene, key: EntityId) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key)
}

fn the(scene: &Scene, key: EntityId) -> Entity {
    record(scene, key).expect("the mob is there")
}

fn value(mob: &Entity, at: u8) -> Option<DataShown> {
    mob_values(mob.kind, &mob.data)
        .into_iter()
        .find(|(index, _, _)| *index == at)
        .map(|(_, value, _)| value)
}

fn flag(mob: &Entity, at: u8) -> bool {
    match value(mob, at) {
        Some(DataShown::Boolean(set)) => set,
        other => panic!("kind {} shows a boolean at {at}: {other:?}", mob.kind),
    }
}

fn byte(mob: &Entity, at: u8) -> u8 {
    match value(mob, at) {
        Some(DataShown::Byte(set)) => set,
        other => panic!("kind {} shows a byte at {at}: {other:?}", mob.kind),
    }
}

fn health(mob: &Entity) -> f32 {
    match value(mob, index::HEALTH) {
        Some(DataShown::Float(health)) => health,
        other => panic!("a mob shows its health: {other:?}"),
    }
}

fn int_at(mob: &Entity, at: u8) -> i32 {
    match value(mob, at) {
        Some(DataShown::Int(n)) => n,
        other => panic!("kind {} shows an int at {at}: {other:?}", mob.kind),
    }
}

/// The block a position is in (`BlockPos.containing`).
fn block_of(at: [f64; 3]) -> BlockPos {
    bp(
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    )
}

/// The block column the mob's feet are in.
fn column(mob: &Entity) -> (i32, i32) {
    (
        mob.position[0].floor() as i32,
        mob.position[2].floor() as i32,
    )
}

/// Whether the tick set the mob alight by the sun: its fire is at the full eight
/// seconds after it (the tick's own burning counts a fire down before the sun's test, so
/// a fire carried on is below it).
fn ignited(mob: &Entity) -> bool {
    mob.fire_ticks == SUN_FIRE
}

/// Stone walls on the four sides of the column (x, z), from the floor up `high`
/// blocks: a well that keeps a mob in its column with the sky open above it.
fn well(x: i32, z: i32, high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut walls = Vec::new();
    for y in -60..-60 + high {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            walls.push((bp(x + dx, y, z + dz), blocks::STONE));
        }
    }
    walls
}

/// [`well`] with the wall to its east at the height of a mob's eyes (y = -59) of
/// glowstone, which lights the column to 14 whatever the sky's light is: enough for
/// the light's test of burning at any hour.
fn lit_well(x: i32, z: i32, high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut walls = well(x, z, high);
    for wall in &mut walls {
        if wall.0 == bp(x + 1, -59, z) {
            wall.1 = blocks::GLOWSTONE;
        }
    }
    walls
}

/// Stone walls `high` blocks high round the square from (-r, -r) to (r, r).
fn pen(r: i32, high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut walls = Vec::new();
    for y in -60..-60 + high {
        for along in -r..=r {
            for (x, z) in [(along, -r), (along, r), (-r, along), (r, along)] {
                walls.push((bp(x, y, z), blocks::STONE));
            }
        }
    }
    walls
}

/// The sounds of id `sound` the tick played, at a place or at an entity, each with its
/// source.
fn sounds_of(output: &TickOutput, sound: i32) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: played,
                source,
                ..
            } if *played == sound => Some(*source),
            RegionEvent::EntitySound {
                sound: played,
                category,
                ..
            } if *played == sound => Some(i32::from(*category)),
            _ => None,
        })
        .collect()
}

/// What the ticks showed of the mob `key`'s equipment, slot by slot, the last word on
/// each slot winning (`SetEquipment`): what its spawn's record wears, which an edge shows
/// with it (`shown::mob_equipment`), then each change.
fn equipment_shown(outputs: &[TickOutput], key: EntityId) -> Vec<(u8, ItemStack)> {
    let mut worn: Vec<(u8, Option<ItemStack>)> = Vec::new();
    for output in outputs {
        for event in &output.events {
            match event {
                RegionEvent::EntitySpawned(state) if state.entity == key => {
                    if let clustine_sim::api::EntityKind::Other { data, .. } = &state.kind {
                        worn = clustine_sim::shown::mob_equipment(data)
                            .into_iter()
                            .map(|(slot, stack)| (slot, Some(stack)))
                            .collect();
                    }
                }
                RegionEvent::EquipmentChanged { entity, slots, .. } if *entity == key => {
                    for (slot, stack) in slots {
                        worn.retain(|(at, _)| at != slot);
                        worn.push((*slot, stack.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    let mut worn: Vec<(u8, ItemStack)> = worn
        .into_iter()
        .filter_map(|(slot, stack)| stack.map(|stack| (slot, stack)))
        .collect();
    worn.sort_by_key(|(slot, _)| *slot);
    worn
}

/// The modifiers of `movement_speed` the ticks first showed of `key`, if they showed it:
/// with its spawn (`shown::mob_attributes`, which an edge sends with it) or in a change.
fn speed_modifiers(outputs: &[TickOutput], key: EntityId) -> Option<Vec<(String, f64, u8)>> {
    outputs.iter().find_map(|output| {
        output.events.iter().find_map(|event| match event {
            RegionEvent::EntitySpawned(state) if state.entity == key => {
                let clustine_sim::api::EntityKind::Other { kind, data, .. } = &state.kind else {
                    return None;
                };
                clustine_sim::shown::mob_attributes(*kind, data)
                    .into_iter()
                    .find(|shown| i32::from(shown.attribute) == attributes::MOVEMENT_SPEED)
                    .map(|shown| shown.modifiers)
            }
            RegionEvent::AttributesChanged {
                entity, attributes, ..
            } if *entity == key => attributes
                .iter()
                .find(|shown| i32::from(shown.attribute) == attributes::MOVEMENT_SPEED)
                .map(|shown| shown.modifiers.clone()),
            _ => None,
        })
    })
}

/// The tick's report of `key`, if it made one.
fn tracked(output: &TickOutput, key: EntityId) -> Option<Tracked> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::EntityTracked(tracked) if tracked.entity == key => Some(tracked.clone()),
        _ => None,
    })
}

// ------------------------------------------------------------------------------------
// K 3.3, plan 2.2 and 10: burning in daylight.

/// K 3.3: under the open sky at noon a zombie catches fire for eight seconds
/// (`igniteForSeconds(8)`, 160 ticks at `burning_time` 1), its chance 0.04 a tick at
/// full light; from then on its reports say it is on fire (the shared flags' bit 0,
/// which `Entity.baseTick` sets from the fire at the start of the next tick), and the
/// fire hurts it.
#[test]
fn a_zombie_under_the_open_sky_at_noon_catches_fire_for_eight_seconds() {
    let zombie = placed(
        &made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE),
        1,
        feet(3, 3),
    );
    let world = World::of(&well(3, 3, 3)).with_entities(&[zombie]);
    let mut scene = scene_at(&world, Inventory::default(), NOON);
    let mut caught = None;
    for tick in 1..=400 {
        scene.idle();
        let zombie = the(&scene, key(1));
        if zombie.fire_ticks > 0 {
            caught = Some((tick, zombie.fire_ticks));
            break;
        }
    }
    let (tick, fire) = caught.expect("a zombie burns at noon within 400 ticks");
    assert_eq!(fire, SUN_FIRE, "lit at tick {tick} for eight seconds");
    let next = scene.idle();
    let report = tracked(&next, key(1)).expect("the tick after reports it");
    assert_ne!(report.flags & Tracked::ON_FIRE, 0, "shown on fire");
    scene.idle_for(60);
    if let Some(zombie) = record(&scene, key(1)) {
        assert!(health(&zombie) < 20.0, "the fire hurts it");
    }
}

/// K 0.1, 3.3 and plan 1.3: the types of `#burn_in_daylight` catch fire at noon in the
/// open, the zombie horse and the zombie nautilus among them; the husk and the parched
/// are not in it and never do.
#[test]
fn the_types_of_the_burning_tag_catch_fire_at_noon_and_the_husk_and_the_parched_never() {
    let burning = [
        (items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE, true),
        (
            items::ZOMBIE_VILLAGER_SPAWN_EGG,
            entity_types::ZOMBIE_VILLAGER,
            true,
        ),
        (items::DROWNED_SPAWN_EGG, entity_types::DROWNED, true),
        (items::SKELETON_SPAWN_EGG, entity_types::SKELETON, true),
        (items::STRAY_SPAWN_EGG, entity_types::STRAY, true),
        (items::BOGGED_SPAWN_EGG, entity_types::BOGGED, true),
        (
            items::ZOMBIE_HORSE_SPAWN_EGG,
            entity_types::ZOMBIE_HORSE,
            true,
        ),
        (
            items::ZOMBIE_NAUTILUS_SPAWN_EGG,
            entity_types::ZOMBIE_NAUTILUS,
            true,
        ),
        (items::HUSK_SPAWN_EGG, entity_types::HUSK, false),
        (items::PARCHED_SPAWN_EGG, entity_types::PARCHED, false),
    ];
    let mut wrong = Vec::new();
    for (egg, kind, burns) in burning {
        let mob = placed(&made(egg, kind), 1, feet(3, 3));
        let world = World::of(&pen(6, 2)).with_entities(&[mob]);
        let mut scene = scene_at(&world, Inventory::default(), NOON);
        let mut caught = None;
        for tick in 1..=600 {
            scene.idle();
            match record(&scene, key(1)) {
                Some(mob) if mob.fire_ticks > 0 => {
                    caught = Some((tick, mob.fire_ticks));
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        match (burns, caught) {
            (true, Some((_, SUN_FIRE))) | (false, None) => {}
            (true, other) => wrong.push(format!("kind {kind} did not catch fire: {other:?}")),
            (false, Some(at)) => wrong.push(format!("kind {kind} caught fire: {at:?}")),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// K 3.3 and 0.2: the sun burns only in the day timeline's hours, from tick 23460 of
/// the day to 12542 of the next, whatever the light; and only where the light at the
/// eyes is above the magic value 0.5, which is a raw brightness of 12 or more
/// (`f / (4 - 3f)` in `float`: 12 gives 0.50000006, 11 gives 0.41). So a zombie lit by
/// glowstone burns from 23460 and up to 12541, while one under the open sky alone stops
/// before the hours end, when the sky's light falls below 12 (tick 12359), and starts
/// only after they begin, when it comes back to 12 (tick 23642). (K 0.2 and 3.3 say a
/// raw brightness of 7; the arithmetic gives 12.)
#[test]
fn the_sun_burns_only_in_the_day_timelines_hours_and_in_light_above_the_magic_half() {
    let open_sky = |day: u64| 15 - sky_darken(day) >= 12;
    let hours = |day: u64| !(BURN_ENDS..BURN_BEGINS).contains(&(day % DAY));
    let zombie = made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE);
    let mut blocks = lit_well(2, 2, 3);
    blocks.extend(well(9, 9, 3));
    let mobs = [
        placed(&zombie, 1, feet(2, 2)),
        placed(&zombie, 2, feet(9, 9)),
    ];
    let world = World::of(&blocks).with_entities(&mobs);
    // (start, ticks, the window in which the lit one must catch fire at least once)
    for (start, ticks, window) in [
        (12_242, 758, (12_400, BURN_ENDS)),
        (23_300, 1_000, (BURN_BEGINS, 23_642)),
    ] {
        let mut scene = lit_scene_at(&world, start);
        let mut lit_days = Vec::new();
        let mut open_days = Vec::new();
        for _ in 0..ticks {
            let output = scene.idle();
            let day = day_of(&output);
            if record(&scene, key(1)).is_some_and(|mob| ignited(&mob)) {
                lit_days.push(day);
            }
            if record(&scene, key(2)).is_some_and(|mob| ignited(&mob)) {
                open_days.push(day);
            }
        }
        for day in &lit_days {
            assert!(hours(*day), "the lit zombie caught fire at day tick {day}");
        }
        for day in &open_days {
            assert!(
                hours(*day) && open_sky(*day),
                "the zombie under the sky alone caught fire at day tick {day}, where the \
                 sky's light is {}",
                15 - sky_darken(*day)
            );
        }
        assert!(
            lit_days
                .iter()
                .any(|day| (window.0..window.1).contains(day)),
            "from {start} the lit zombie catches fire between {window:?}: {lit_days:?}"
        );
        if start == 23_300 {
            assert!(
                !open_days.is_empty(),
                "the zombie under the sky catches fire once the morning is light"
            );
        }
    }
}

/// K 3.3: the sky is seen where its light at the eyes is 15, which glass lets through
/// and stone does not: a zombie in a lit well under a stone roof never burns at noon,
/// and one under a glass roof does.
#[test]
fn a_stone_roof_keeps_the_sun_off_and_a_glass_one_does_not() {
    let zombie = made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE);
    let mut blocks = lit_well(2, 2, 3);
    blocks.push((bp(2, -57, 2), blocks::STONE));
    blocks.extend(lit_well(9, 9, 3));
    blocks.push((bp(9, -57, 9), blocks::GLASS));
    let mobs = [
        placed(&zombie, 1, feet(2, 2)),
        placed(&zombie, 2, feet(9, 9)),
    ];
    let world = World::of(&blocks).with_entities(&mobs);
    let mut scene = lit_scene_at(&world, NOON);
    let mut under_glass = 0;
    for tick in 1..=800 {
        scene.idle();
        let roofed = the(&scene, key(1));
        assert!(
            roofed.fire_ticks <= 0,
            "under stone it caught fire at {tick}"
        );
        if record(&scene, key(2)).is_some_and(|mob| ignited(&mob)) {
            under_glass += 1;
        }
    }
    assert!(under_glass > 0, "under glass the sun burns it");
}

/// K 3.3: a mob in water is wet (`isInWaterOrRain`) and does not burn: a zombie standing
/// in water to its knees at noon never catches fire.
#[test]
fn a_zombie_standing_in_water_does_not_burn() {
    let zombie = placed(
        &made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE),
        1,
        feet(2, 2),
    );
    let mut blocks = well(2, 2, 3);
    blocks.push((bp(2, -60, 2), blocks::WATER));
    let world = World::of(&blocks).with_entities(&[zombie]);
    let mut scene = scene_at(&world, Inventory::default(), NOON);
    for tick in 1..=800 {
        scene.idle();
        assert!(
            the(&scene, key(1)).fire_ticks <= 0,
            "wet, it caught fire at {tick}"
        );
    }
}

/// Plan 10 ("Burning"), K 3.3: the undead horse keeps the sun off with what its body
/// wears (`ZombieHorse.sunProtectionSlot` is `BODY`): horse armour, which a player puts
/// on a tamed zombie horse (it is in `#can_wear_horse_armor`; a wild one is made mad
/// by an item, `ZombieHorse.mobInteract`), keeps it from burning at noon, while the
/// same horse bare burns.
#[test]
fn horse_armour_keeps_the_sun_off_a_zombie_horse() {
    const EMPTY: u8 = 2;
    let mut scene = scene_at(
        &World::of(&pen(6, 2)),
        inventory(&[
            (0, stack(items::ZOMBIE_HORSE_SPAWN_EGG, 1)),
            (1, stack(items::IRON_HORSE_ARMOR, 1)),
        ]),
        NIGHT,
    );
    let (_, hatched) = hatch(&mut scene, 0, 2, 2);
    let horse = hatched[0];
    scene.select(EMPTY);
    let rider = scene.actors[0].id;
    let mut tamed = false;
    for _ in 0..40_000 {
        if byte(&the(&scene, horse), HORSE_FLAGS) & HORSE_TAME != 0 {
            tamed = true;
            break;
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
            scene.input(used(horse));
        }
    }
    assert!(tamed, "ridden, the zombie horse is tamed");
    for sneak in [true, false] {
        scene.input(PlayerInput::Keys(Keys {
            sneak,
            ..Keys::default()
        }));
    }
    scene.idle_for(2);
    let bare = the(&scene, horse);
    scene.select(1);
    scene.input(used(horse));
    scene.idle();
    let armoured = the(&scene, horse);
    assert_ne!(armoured.data, bare.data, "the armour went on");
    let mobs = [
        placed(&armoured, 1, feet(2, 2)),
        placed(&bare, 2, feet(-3, -3)),
    ];
    let world = World::of(&pen(6, 2)).with_entities(&mobs);
    let mut day = scene_at(&world, Inventory::default(), NOON);
    let mut bare_burnt = false;
    for tick in 1..=800 {
        day.idle();
        let armoured = the(&day, key(1));
        assert!(
            armoured.fire_ticks <= 0,
            "the armoured zombie horse caught fire at {tick}"
        );
        bare_burnt |= record(&day, key(2)).is_some_and(|mob| mob.fire_ticks > 0);
    }
    assert!(bare_burnt, "the bare one beside it burns");
}

// ------------------------------------------------------------------------------------
// K 3.4.1, 5 (Z) and 11.12: the zombie's egg at easy.

/// What a batch of zombie eggs made: each zombie with whether it is a baby, what it
/// rides (and whether that was a chicken there before it), and what the ticks of its
/// making showed of it.
struct Zombies {
    made: Vec<MadeZombie>,
}

struct MadeZombie {
    baby: bool,
    rides: Option<(EntityId, bool)>,
    equipment: Vec<(u8, ItemStack)>,
    speed_modifiers: Option<Vec<(String, f64, u8)>>,
}

/// The floor's columns within two of the origin but the player's.
fn cells() -> Vec<(i32, i32)> {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| (x, z)))
        .filter(|cell| *cell != (0, 0))
        .collect()
}

/// Two thousand zombies, made a hundred to a region (each region's player with keys
/// of its own, and so a stream of its own), at night, each region with four chickens
/// made first among the columns the eggs are used on.
fn zombies() -> &'static Zombies {
    static MADE: OnceLock<Zombies> = OnceLock::new();
    MADE.get_or_init(|| {
        let cells = cells();
        let mut made = Vec::new();
        for region in 0..20u32 {
            let mut layout = Layout::alone();
            layout.block = 20 + region;
            let mut scene = scene_on(
                layout,
                &World::default(),
                inventory(&[
                    (0, stack(items::ZOMBIE_SPAWN_EGG, 1)),
                    (1, stack(items::CHICKEN_SPAWN_EGG, 1)),
                ]),
                NIGHT,
            );
            let mut chickens = Vec::new();
            for (x, z) in [(1, 1), (-1, 1), (1, -1), (-1, -1)] {
                let (_, keys) = hatch(&mut scene, 1, x, z);
                chickens.extend(keys);
            }
            let mut outputs = Vec::new();
            let mut zombies = Vec::new();
            for n in 0..100 {
                let (x, z) = cells[n % cells.len()];
                let (output, keys) = hatch(&mut scene, 0, x, z);
                outputs.push(output);
                zombies.extend(keys.into_iter().filter(|key| {
                    record(&scene, *key)
                        .is_some_and(|mob| i32::from(mob.kind) == entity_types::ZOMBIE)
                }));
            }
            outputs.extend(scene.idle_for(2));
            for key in zombies {
                let zombie = the(&scene, key);
                made.push(MadeZombie {
                    baby: flag(&zombie, ZOMBIE_BABY),
                    rides: zombie
                        .riding
                        .vehicle
                        .map(|vehicle| (vehicle, chickens.contains(&vehicle))),
                    equipment: equipment_shown(&outputs, key),
                    speed_modifiers: speed_modifiers(&outputs, key),
                });
            }
        }
        Zombies { made }
    })
}

/// K 5 (Z, steps 3 and 4) and 3.4.1: one zombie egg in twenty makes a baby
/// (`getSpawnAsBabyOdds`, 0.05, whatever the difficulty), shown by its baby flag. A baby
/// rides the first chicken within 5, 3, 5 that carries nobody one time in twenty, else
/// a chicken made for it one time in twenty: a chicken jockey. Of 2000 zombies about
/// 100 are babies, and about 5 ride a chicken there before them and 5 a new one.
#[test]
fn one_zombie_egg_in_twenty_makes_a_baby_and_some_babies_ride_chickens() {
    let made = &zombies().made;
    assert_eq!(made.len(), 2_000, "every egg made a zombie");
    let babies = made.iter().filter(|zombie| zombie.baby).count();
    assert!(
        (70..=130).contains(&babies),
        "about one in twenty of 2000 is a baby: {babies}"
    );
    let riders: Vec<&MadeZombie> = made
        .iter()
        .filter(|zombie| zombie.rides.is_some())
        .collect();
    assert!(
        riders.iter().all(|zombie| zombie.baby),
        "only babies ride chickens"
    );
    let on_old = riders
        .iter()
        .filter(|zombie| zombie.rides.is_some_and(|(_, old)| old))
        .count();
    let on_new = riders.len() - on_old;
    assert!(
        on_old >= 1 && on_new >= 1 && riders.len() <= 25,
        "about 5 babies ride a chicken already there and 5 a new one: {on_old} and {on_new}"
    );
}

/// K 3.4.1 (`setBaby`): a baby zombie's speed has the `baby` modifier (+0.5,
/// `ADD_MULTIPLIED_BASE`), which no adult has, and clients are sent it with the mob's
/// pairing (`getSyncableAttributes`: every synced instance, `movement_speed` among
/// them): the ticks of its making show it.
#[test]
fn a_baby_zombie_is_shown_its_baby_speed_and_an_adult_is_not() {
    let baby_speed = (String::from("minecraft:baby"), 0.5, 1);
    let mut wrong = Vec::new();
    for zombie in &zombies().made {
        let shown = zombie
            .speed_modifiers
            .as_ref()
            .is_some_and(|modifiers| modifiers.contains(&baby_speed));
        if zombie.speed_modifiers.is_none() || shown != zombie.baby {
            wrong.push((zombie.baby, zombie.speed_modifiers.clone()));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} zombies (baby, speed shown) wrong, the first: {:?}",
        wrong.len(),
        wrong.first()
    );
}

/// K 5 (Z, step 6), 0.4 and plan 1.3: at easy no zombie wears armour (the 0.15 × 0 chance
/// never passes), and one in a hundred holds a weapon (0.01 at easy and normal): an iron
/// sword (one in six), an iron spear (one in six) or an iron shovel. What a mob holds and
/// wears is shown to clients (`SetEquipment` with its pairing).
#[test]
fn at_easy_no_zombie_wears_armour_and_one_in_a_hundred_holds_an_iron_weapon() {
    let made = &zombies().made;
    for zombie in made {
        for (slot, stack) in &zombie.equipment {
            assert!(
                !ARMOUR.contains(slot),
                "armour at easy: {stack:?} in {slot}"
            );
        }
    }
    let weapons: Vec<ItemStack> = made
        .iter()
        .filter_map(|zombie| {
            zombie
                .equipment
                .iter()
                .find(|(slot, _)| *slot == MAIN_HAND)
                .map(|(_, stack)| stack.clone())
        })
        .collect();
    assert!(
        (5..=40).contains(&weapons.len()),
        "about one in a hundred of 2000 holds a weapon: {weapons:?}"
    );
    for weapon in &weapons {
        assert!(
            [items::IRON_SWORD, items::IRON_SPEAR, items::IRON_SHOVEL].contains(&weapon.item)
                && weapon.count == 1,
            "a zombie's weapon: {weapon:?}"
        );
    }
}

/// K 11.2: a skeleton is given its bow at its spawn (`populateDefaultEquipmentSlots`),
/// and its clients see it hold it (`SetEquipment` with its pairing).
#[test]
fn a_skeleton_from_an_egg_is_shown_holding_its_bow() {
    let mut scene = scene_at(
        &World::default(),
        inventory(&[(0, stack(items::SKELETON_SPAWN_EGG, 1))]),
        NIGHT,
    );
    let (output, made) = hatch(&mut scene, 0, 2, 2);
    let mut outputs = vec![output];
    outputs.extend(scene.idle_for(3));
    assert_eq!(
        equipment_shown(&outputs, made[0]),
        vec![(MAIN_HAND, ItemStack::new(items::BOW, 1))],
        "the skeleton's bow is shown"
    );
}

/// K 11.1 and 11.12: a drowned's equipment ignores the difficulty: `nextFloat() > 0.9`
/// then `nextInt(16)`, a trident below 10 (0.0625) else a fishing rod (0.0375); then a
/// nautilus shell in the off hand 0.03; never armour. Of 800 drowned about 50 hold a
/// trident, 30 a rod and 24 carry a shell; clients are shown what each holds.
#[test]
fn a_drowned_holds_a_trident_one_time_in_sixteen_and_a_shell_one_in_thirty_three() {
    let cells = cells();
    let (mut tridents, mut rods, mut shells) = (0, 0, 0);
    for region in 0..8u32 {
        let mut layout = Layout::alone();
        layout.block = 60 + region;
        let mut scene = scene_on(
            layout,
            &World::default(),
            inventory(&[(0, stack(items::DROWNED_SPAWN_EGG, 1))]),
            NIGHT,
        );
        let mut outputs = Vec::new();
        let mut drowned = Vec::new();
        for n in 0..100 {
            let (x, z) = cells[n % cells.len()];
            let (output, keys) = hatch(&mut scene, 0, x, z);
            outputs.push(output);
            drowned.extend(keys);
        }
        outputs.extend(scene.idle_for(2));
        for key in drowned {
            for (slot, stack) in equipment_shown(&outputs, key) {
                match (slot, stack.item) {
                    (MAIN_HAND, items::TRIDENT) => tridents += 1,
                    (MAIN_HAND, items::FISHING_ROD) => rods += 1,
                    (OFF_HAND, items::NAUTILUS_SHELL) => shells += 1,
                    other => panic!("a drowned holds or wears {other:?}"),
                }
            }
        }
    }
    assert!(
        (30..=72).contains(&tridents),
        "about 50 tridents of 800: {tridents}"
    );
    assert!((14..=48).contains(&rods), "about 30 rods of 800: {rods}");
    assert!(
        (10..=40).contains(&shells),
        "about 24 shells of 800: {shells}"
    );
}

// ------------------------------------------------------------------------------------
// K 11.5, plan 2.4 and 10: the zombie's and the husk's drowning.

/// A well three blocks of water deep, walled four high, at (x, z): a zombie in it has
/// its eyes under water.
fn deep_well(x: i32, z: i32) -> Vec<(BlockPos, BlockState)> {
    let mut blocks = well(x, z, 4);
    for y in -60..=-58 {
        blocks.push((bp(x, y, z), blocks::WATER));
    }
    blocks
}

/// What the ticks of a scene showed of a mob that converts: counted in its own ticks
/// from `lived` (its record's when it was set in the scene), the tick after which its
/// flag at `flag_at` first showed, and the tick after which it had gone, with that
/// tick's output.
struct Converting {
    flagged: Option<u32>,
    gone: Option<(u32, TickOutput)>,
    last: Option<Entity>,
}

fn watch(scene: &mut Scene, old: EntityId, lived: u32, flag_at: u8, ticks: u32) -> Converting {
    let mut flagged = None;
    let mut own = 0;
    let mut last = None;
    // The ticks the scene's setting up gave it already.
    if let Some(mob) = record(scene, old) {
        own = mob.lived - lived;
        if flag(&mob, flag_at) {
            flagged = Some(own);
        }
        last = Some(mob);
    }
    for _ in 0..ticks {
        let output = scene.idle();
        match record(scene, old) {
            Some(mob) => {
                own = mob.lived - lived;
                if flagged.is_none() && flag(&mob, flag_at) {
                    flagged = Some(own);
                }
                last = Some(mob);
            }
            None => {
                return Converting {
                    flagged,
                    gone: Some((own + 1, output)),
                    last,
                };
            }
        }
    }
    Converting {
        flagged,
        gone: None,
        last,
    }
}

/// K 11.5, 3.4.1 and 3.4.2: a zombie with its eyes under water counts 600 ticks
/// (`InWaterTime`), then shows itself converting (data 18) and becomes a drowned on the
/// 301st tick after; a husk becomes a zombie so. A mob that has been out of water once
/// starts its count at -1, and so shows the flag after its 601st tick under water and
/// converts in its 902nd. The conversion plays level event 1040 (a husk's 1041) at the
/// new mob's block. Undead breathe under water (`#can_breathe_under_water` holds
/// `#undead`): its air stays full meanwhile.
#[test]
fn a_zombie_under_water_becomes_a_drowned_and_a_husk_a_zombie_after_601_and_301_ticks() {
    for (egg, kind, into, event) in [
        (
            items::ZOMBIE_SPAWN_EGG,
            entity_types::ZOMBIE,
            entity_types::DROWNED,
            ZOMBIE_CONVERTED,
        ),
        (
            items::HUSK_SPAWN_EGG,
            entity_types::HUSK,
            entity_types::ZOMBIE,
            HUSK_CONVERTED,
        ),
    ] {
        let mob = placed(&made(egg, kind), 1, feet(2, 2));
        let lived = mob.lived;
        let world = World::of(&deep_well(2, 2)).with_entities(&[mob]);
        let mut scene = scene_at(&world, Inventory::default(), NIGHT);
        let watched = watch(&mut scene, key(1), lived, DROWNED_CONVERSION, 1_000);
        let last = watched.last.expect("it was there");
        assert_eq!(int_at(&last, index::AIR_SUPPLY), 300, "it breathes");
        assert_eq!(
            watched.flagged,
            Some(601),
            "kind {kind} shows its conversion"
        );
        let (tick, output) = watched.gone.expect("it converts");
        assert_eq!(tick, 902, "kind {kind} converts in its 902nd tick");
        let new = scene.of_kind(into);
        assert_eq!(new.len(), 1, "kind {kind} became one of kind {into}");
        let at = block_of(new[0].position);
        assert!(
            level_events(&output).contains(&(event, at, 0)),
            "kind {kind}'s conversion plays {event} at {at:?}: {:?}",
            level_events(&output)
        );
    }
}

/// K 11.5 ("Saved", "Read"), plan 10 ("Counters"): the counts are kept with the mob and a
/// region that reads it from its chunk goes on with them: the flag comes 601 ticks under
/// water after the count's start, and the drowned 902, however the ticks are shared out
/// between regions.
#[test]
fn the_drowning_counts_go_on_through_a_restore_of_the_region() {
    let zombie = placed(
        &made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE),
        1,
        feet(2, 2),
    );
    let start = zombie.lived;
    let blocks = deep_well(2, 2);
    let mut first = scene_at(
        &World::of(&blocks).with_entities(&[zombie]),
        Inventory::default(),
        NIGHT,
    );
    first.idle_for(400);
    let before = the(&first, key(1));
    let gone_by = before.lived - start;
    let mut second = scene_at(
        &World::of(&blocks).with_entities(std::slice::from_ref(&before)),
        Inventory::default(),
        NIGHT,
    );
    let watched = watch(&mut second, key(1), before.lived, DROWNED_CONVERSION, 351);
    assert!(watched.gone.is_none(), "not converted yet");
    assert_eq!(
        watched.flagged,
        Some(601 - gone_by),
        "the count went on from {gone_by}"
    );
    let converting = the(&second, key(1));
    let gone_by = converting.lived - start;
    let mut third = scene_at(
        &World::of(&blocks).with_entities(std::slice::from_ref(&converting)),
        Inventory::default(),
        NIGHT,
    );
    let watched = watch(
        &mut third,
        key(1),
        converting.lived,
        DROWNED_CONVERSION,
        600,
    );
    let (tick, _) = watched.gone.expect("it converts");
    assert_eq!(tick, 902 - gone_by, "the conversion went on from {gone_by}");
    assert_eq!(third.of_kind(entity_types::DROWNED).len(), 1);
}

/// K 11.5: leaving the water at any moment, converting or not, sets the count back to -1
/// and clears the flag: a zombie converting that leaves stops converting at once, and
/// back in the water needs 601 ticks for the flag and 902 to convert again.
#[test]
fn leaving_the_water_starts_the_drowning_count_again() {
    let zombie = placed(
        &made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE),
        1,
        feet(2, 2),
    );
    let wet = deep_well(2, 2);
    let dry = well(2, 2, 4);
    let mut scene = scene_at(
        &World::of(&wet).with_entities(&[zombie]),
        Inventory::default(),
        NIGHT,
    );
    scene.idle_for(700);
    let converting = the(&scene, key(1));
    assert!(
        flag(&converting, DROWNED_CONVERSION),
        "converting after 700"
    );
    let mut out = scene_at(
        &World::of(&dry).with_entities(&[converting]),
        Inventory::default(),
        NIGHT,
    );
    out.idle_for(5);
    let dried = the(&out, key(1));
    assert!(!flag(&dried, DROWNED_CONVERSION), "out of water it stops");
    let lived = dried.lived;
    let mut back = scene_at(
        &World::of(&wet).with_entities(&[dried]),
        Inventory::default(),
        NIGHT,
    );
    let watched = watch(&mut back, key(1), lived, DROWNED_CONVERSION, 1_000);
    assert_eq!(watched.flagged, Some(601), "the count starts again");
    assert_eq!(watched.gone.map(|(tick, _)| tick), Some(902));
}

/// K 11.5 and plan 2.4: `convertTo` keeps the baby flag and gives the new mob a new key,
/// but not the health (the new mob starts at its maximum) and calls no `finalizeSpawn`.
/// A baby zombie (a zombie egg used on a zombie, `spawnOffspringFromSpawnEgg`) becomes a
/// baby drowned; a zombie hurt by a player becomes a drowned at full health.
#[test]
fn a_drowning_keeps_the_baby_flag_and_not_the_health() {
    let mut scene = scene_at(
        &World::default(),
        inventory(&[(0, stack(items::ZOMBIE_SPAWN_EGG, 1))]),
        NIGHT,
    );
    let (_, hatched) = hatch(&mut scene, 0, 2, 2);
    let parent = hatched[0];
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.input(used(parent));
    let baby = scene
        .of_kind(entity_types::ZOMBIE)
        .into_iter()
        .find(|zombie| !before.contains(&zombie.key))
        .expect("an egg used on a zombie makes a baby zombie");
    assert!(flag(&baby, ZOMBIE_BABY), "a baby");
    scene.input(PlayerInput::Attack {
        part: None,
        entity: parent,
    });
    scene.idle_for(3);
    let hurt = the(&scene, parent);
    assert!(health(&hurt) < 20.0, "the hit hurt it");
    let mut blocks = deep_well(2, 2);
    blocks.extend(deep_well(-3, 2));
    let world = World::of(&blocks)
        .with_entities(&[placed(&hurt, 1, feet(2, 2)), placed(&baby, 2, feet(-3, 2))]);
    let mut wet = scene_at(&world, Inventory::default(), NIGHT);
    wet.idle_for(902);
    assert!(record(&wet, key(1)).is_none() && record(&wet, key(2)).is_none());
    let drowned = wet.of_kind(entity_types::DROWNED);
    assert_eq!(drowned.len(), 2, "both drowned");
    for mob in &drowned {
        assert!(mob.key != key(1) && mob.key != key(2), "a new key");
        let was_baby = column(mob) == (-3, 2);
        assert_eq!(flag(mob, ZOMBIE_BABY), was_baby, "the baby flag is kept");
        assert_eq!(health(mob), 20.0, "at the drowned's own maximum");
    }
}

/// K 11.1 and 3.4.3: the drowned and the zombie villager do not convert in water
/// (`convertsInWater` false): after 1000 ticks with their eyes under they are as they
/// were, their flags clear.
#[test]
fn the_drowned_and_the_zombie_villager_never_convert_in_water() {
    let drowned = made(items::DROWNED_SPAWN_EGG, entity_types::DROWNED);
    let villager = made(
        items::ZOMBIE_VILLAGER_SPAWN_EGG,
        entity_types::ZOMBIE_VILLAGER,
    );
    let mut blocks = deep_well(2, 2);
    blocks.extend(deep_well(-3, 2));
    let world = World::of(&blocks).with_entities(&[
        placed(&drowned, 1, feet(2, 2)),
        placed(&villager, 2, feet(-3, 2)),
    ]);
    let mut scene = scene_at(&world, Inventory::default(), NIGHT);
    for tick in 1..=1_000 {
        scene.idle();
        let drowned = record(&scene, key(1)).unwrap_or_else(|| panic!("drowned gone at {tick}"));
        let villager = record(&scene, key(2)).unwrap_or_else(|| panic!("villager gone at {tick}"));
        assert!(!flag(&drowned, DROWNED_CONVERSION));
        assert!(!flag(&villager, DROWNED_CONVERSION));
        assert!(!flag(&villager, VILLAGER_CONVERTING));
    }
}

// ------------------------------------------------------------------------------------
// K 11.1: the drowned by day and by night.

/// K 11.1 (`DrownedGoToWaterGoal`): by day, a drowned on land walks to water within ten
/// blocks: a drowned four blocks from a pond at noon is in it within 600 ticks.
#[test]
fn by_day_a_drowned_on_land_walks_into_water_nearby() {
    let drowned = placed(
        &made(items::DROWNED_SPAWN_EGG, entity_types::DROWNED),
        1,
        feet(-3, 0),
    );
    let mut blocks = pen(9, 2);
    for x in 2..=6 {
        for z in -2..=2 {
            blocks.push((bp(x, -60, z), blocks::WATER));
        }
    }
    let world = World::of(&blocks).with_entities(&[drowned]);
    let mut scene = scene_at(&world, Inventory::default(), NOON);
    let mut seen = Vec::new();
    let reached = scene.idle_until(600, &mut seen, |scene, _| {
        record(scene, key(1)).is_some_and(|mob| {
            let (x, z) = column(&mob);
            scene.block(bp(x, -60, z)) == blocks::WATER
        })
    });
    assert!(reached, "the drowned walked into the pond");
}

/// A pool eight blocks of water deep (y = -60 to -53) over the floor from (x - 3, z - 3)
/// to (x + 3, z + 3), walled nine high.
fn deep_pool(x: i32, z: i32) -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for y in -60..=-52 {
        for along in -4..=4 {
            for (dx, dz) in [(along, -4), (along, 4), (-4, along), (4, along)] {
                blocks.push((bp(x + dx, y, z + dz), blocks::STONE));
            }
        }
    }
    for y in -60..=-53 {
        for dx in -3..=3 {
            for dz in -3..=3 {
                blocks.push((bp(x + dx, y, z + dz), blocks::WATER));
            }
        }
    }
    blocks
}

/// K 11.1 (`DrownedMoveControl`, `travelInWater`, `DrownedSwimUpGoal`): by day a drowned
/// in deep water with no target does not want to swim: it sinks to the bottom and is
/// never shown swimming. At night its swim-up goal has it search for land (the sea level
/// is far above), and wanting to swim under water it is shown swimming, is moved by its
/// input alone with no gravity, and gains 0.002 upwards a tick: it rises.
#[test]
fn a_drowned_sinks_in_deep_water_by_day_and_swims_up_at_night() {
    let drowned = made(items::DROWNED_SPAWN_EGG, entity_types::DROWNED);
    let start = [5.5, -57.0, 5.5];
    let world = World::of(&deep_pool(5, 5)).with_entities(&[placed(&drowned, 1, start)]);
    let mut day = scene_at(&world, Inventory::default(), NOON);
    let mut bottom = None;
    for tick in 1..=300 {
        day.idle();
        let mob = the(&day, key(1));
        assert_eq!(
            mob_shared_flags(&mob.data) & SWIMMING,
            0,
            "by day it does not swim (tick {tick})"
        );
        // It may hop off the bottom after a stroll's target in the water above (the
        // walking control's jump), about three and a half blocks, a little above where
        // it was let go, and sinks again; it never swims up the block and more that it
        // does at night.
        assert!(mob.position[1] <= start[1] + 1.0, "by day it does not rise");
        if bottom.is_none() && mob.position[1] == -60.0 {
            bottom = Some(tick);
        }
    }
    assert!(
        bottom.is_some_and(|tick| tick <= 100),
        "by day it sinks to the bottom: {bottom:?}"
    );
    let mut night = scene_at(&world, Inventory::default(), NIGHT);
    let mut swam = false;
    let mut highest = start[1];
    for _ in 0..300 {
        night.idle();
        let mob = the(&night, key(1));
        swam |= mob_shared_flags(&mob.data) & SWIMMING != 0;
        highest = highest.max(mob.position[1]);
    }
    assert!(swam, "at night it is shown swimming");
    assert!(highest > start[1] + 1.0, "at night it swims up: {highest}");
}

// ------------------------------------------------------------------------------------
// K 3.4.3: the zombie villager.

/// K 3.4.3, 0.6 and 0.16: a zombie villager shows its villager data: its type by the
/// biome at its feet (`finalizeVillagerType`; a plains floor here), its profession drawn
/// at its construction from all fifteen, its level 1; and it is not converting. Of
/// thirty, more than one profession shows.
#[test]
fn a_zombie_villager_shows_a_drawn_profession_and_the_type_of_its_biome() {
    let world = World {
        biome: biome_id("minecraft:plains"),
        ..World::default()
    };
    let mut scene = scene_at(
        &world,
        inventory(&[(0, stack(items::ZOMBIE_VILLAGER_SPAWN_EGG, 1))]),
        NIGHT,
    );
    let mut professions = Vec::new();
    for n in 0..30 {
        let (_, made) = hatch(&mut scene, 0, (n % 5) - 2, 2);
        let villager = the(&scene, made[0]);
        match value(&villager, VILLAGER_DATA) {
            Some(DataShown::VillagerData {
                kind,
                profession,
                level,
            }) => {
                assert_eq!(kind, villagers::types::PLAINS, "a plains zombie villager");
                assert_eq!(level, 1);
                assert!((0..15).contains(&profession));
                if !professions.contains(&profession) {
                    professions.push(profession);
                }
            }
            other => panic!("villager data at 20: {other:?}"),
        }
        assert!(!flag(&villager, VILLAGER_CONVERTING), "not converting");
    }
    assert!(professions.len() > 1, "drawn: {professions:?}");
}

/// K 3.4.3 (`mobInteract`): a golden apple cures only a zombie villager that is weak,
/// and none is (nothing a player has makes it weak): it does nothing, no entity event
/// 16 and no conversion.
#[test]
fn a_golden_apple_does_not_cure_a_zombie_villager_that_is_not_weak() {
    let mut scene = scene_at(
        &World::default(),
        inventory(&[
            (0, stack(items::ZOMBIE_VILLAGER_SPAWN_EGG, 1)),
            (1, stack(items::GOLDEN_APPLE, 1)),
        ]),
        NIGHT,
    );
    let (_, made) = hatch(&mut scene, 0, 2, 2);
    scene.select(1);
    let output = scene.input(used(made[0]));
    let cure = output.events.iter().any(|event| {
        matches!(event, RegionEvent::EntityEvent { entity, event: 16, .. } if *entity == made[0])
    });
    assert!(!cure, "no cure begins");
    scene.idle();
    assert!(!flag(&the(&scene, made[0]), VILLAGER_CONVERTING));
}

fn biome_id(name: &str) -> u16 {
    let biomes = clustine_data::SYNCED_REGISTRIES
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

// ------------------------------------------------------------------------------------
// K 11.8: turtle eggs.

/// K 11.8 and plan 1.3: every zombie, husk, zombie villager and drowned, with no target,
/// walks to a turtle egg within 24 and tramples it: at the egg it plays
/// `zombie_destroy_egg` (hostile) every few ticks and after 60 removes it with
/// `turtle_egg_break` (blocks).
#[test]
fn zombies_of_every_kind_walk_to_a_turtle_egg_and_trample_it() {
    let egg_at = bp(5, -60, 5);
    let mut wrong = Vec::new();
    for (egg, kind) in [
        (items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE),
        (items::HUSK_SPAWN_EGG, entity_types::HUSK),
        (
            items::ZOMBIE_VILLAGER_SPAWN_EGG,
            entity_types::ZOMBIE_VILLAGER,
        ),
        (items::DROWNED_SPAWN_EGG, entity_types::DROWNED),
    ] {
        // Off the exact diagonal to the egg: along it a mob's path ends on the egg's own
        // block just short of the goal's reach, where the game's goal stalls too (its
        // path is the start node alone, already reached).
        let mob = placed(&made(egg, kind), 1, feet(-4, -1));
        let mut blocks = pen(7, 2);
        blocks.push((egg_at, blocks::TURTLE_EGG));
        let world = World::of(&blocks).with_entities(&[mob]);
        let mut scene = scene_at(&world, Inventory::default(), NIGHT);
        let mut seen = Vec::new();
        let trampled = scene.idle_until(2_000, &mut seen, |scene, _| {
            scene.block(egg_at) == blocks::AIR
        });
        if !trampled {
            let mob = record(&scene, key(1)).map(|mob| mob.position);
            wrong.push(format!("kind {kind} left the egg; it stands at {mob:?}"));
            continue;
        }
        let destroying: Vec<i32> = seen
            .iter()
            .flat_map(|output| sounds_of(output, sounds::ENTITY_ZOMBIE_DESTROY_EGG))
            .collect();
        if destroying.is_empty() || destroying.iter().any(|source| *source != HOSTILE) {
            wrong.push(format!("kind {kind} at the egg sounded {destroying:?}"));
        }
        let broke: Vec<i32> = seen
            .iter()
            .flat_map(|output| sounds_of(output, sounds::ENTITY_TURTLE_EGG_BREAK))
            .collect();
        if broke != vec![BLOCKS_SOURCE] {
            wrong.push(format!("kind {kind}: the egg broke sounding {broke:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

// ------------------------------------------------------------------------------------
// K 11.2: the skeletons.

/// K 11.2 (`FleeSunGoal`, `RestrictSunGoal`) and plan 1.3: a skeleton burning at noon
/// walks into lit shade within ten blocks (a block that cannot see the sky and whose
/// light is above the magic half: under the edge of a roof), and once there keeps its
/// paths out of the sun (the navigation's `avoidSun` trim): it stays under the roof.
#[test]
fn a_burning_skeleton_flees_into_the_shade_and_keeps_out_of_the_sun() {
    let skeleton = placed(
        &made(items::SKELETON_SPAWN_EGG, entity_types::SKELETON),
        1,
        feet(2, 8),
    );
    let mut blocks = pen(15, 3);
    blocks.retain(|(at, _)| at.x >= 0 && at.z >= 0);
    for along in 0..=15 {
        for y in -60..=-58 {
            blocks.push((bp(along, y, 0), blocks::STONE));
            blocks.push((bp(0, y, along), blocks::STONE));
        }
    }
    for x in 8..=12 {
        for z in 5..=11 {
            blocks.push((bp(x, -58, z), blocks::STONE));
        }
    }
    let under_roof = |mob: &Entity| {
        let (x, z) = column(mob);
        (8..=12).contains(&x) && (5..=11).contains(&z)
    };
    let world = World::of(&blocks).with_entities(&[skeleton]);
    let mut scene = lit_scene_at(&world, NOON);
    let mut seen = Vec::new();
    let shaded = scene.idle_until(800, &mut seen, |scene, _| {
        record(scene, key(1)).is_some_and(|mob| under_roof(&mob))
    });
    assert!(shaded, "the skeleton found the shade");
    for tick in 1..=400 {
        scene.idle();
        let Some(skeleton) = record(&scene, key(1)) else {
            break;
        };
        assert!(
            under_roof(&skeleton),
            "it walked into the sun again {tick} ticks later: {:?}",
            skeleton.position
        );
    }
}

/// K 11.2 (`AvoidEntityGoal(Wolf, 6, 1.0, 1.2)`): a skeleton runs from a wolf within 6,
/// with or without a target: one three blocks from a tamed wolf that sits is soon more
/// than six from it.
#[test]
fn a_skeleton_runs_from_a_wolf() {
    let mut taming = scene_at(
        &World::of(&pen(6, 2)),
        inventory(&[
            (0, stack(items::WOLF_SPAWN_EGG, 1)),
            (1, stack(items::BONE, 1)),
        ]),
        NIGHT,
    );
    let (_, hatched) = hatch(&mut taming, 0, 2, 2);
    let wolf = hatched[0];
    taming.select(1);
    for _ in 0..60 {
        if byte(&the(&taming, wolf), TAMABLE_FLAGS) & TAMABLE_TAME != 0 {
            break;
        }
        taming.input(used(wolf));
    }
    assert_ne!(
        byte(&the(&taming, wolf), TAMABLE_FLAGS) & TAMABLE_TAME,
        0,
        "tamed by bones"
    );
    taming.idle_for(2);
    let wolf = the(&taming, wolf);
    let skeleton = made(items::SKELETON_SPAWN_EGG, entity_types::SKELETON);
    let world = World::of(&pen(12, 3)).with_entities(&[
        placed(&wolf, 1, feet(-3, -3)),
        placed(&skeleton, 2, feet(0, -3)),
    ]);
    let mut scene = scene_at(&world, Inventory::default(), NIGHT);
    let mut seen = Vec::new();
    let ran = scene.idle_until(200, &mut seen, |scene, _| {
        let (Some(wolf), Some(skeleton)) = (record(scene, key(1)), record(scene, key(2))) else {
            return false;
        };
        let dx = wolf.position[0] - skeleton.position[0];
        let dz = wolf.position[2] - skeleton.position[2];
        (dx * dx + dz * dz).sqrt() > 6.0
    });
    assert!(ran, "the skeleton ran from the wolf");
}

/// A column of powder snow two high at (x, z), walled three high.
fn snow_well(x: i32, z: i32) -> Vec<(BlockPos, BlockState)> {
    let mut blocks = well(x, z, 3);
    blocks.push((bp(x, -60, z), blocks::POWDER_SNOW));
    blocks.push((bp(x, -59, z), blocks::POWDER_SNOW));
    blocks
}

/// K 11.2 and 11.5: a skeleton in powder snow counts 140 ticks (`FreezingTime`), shows
/// itself shaking (data 16), and on the 301st tick after becomes a stray, with level
/// event 1048 at the stray's block. Its tracker runs before its tick and reads the
/// powder snow its last tick's move left: one set into a chunk, whose count is -1 and
/// whose first tick has no move behind it, shows the flag after its 142nd tick and
/// converts in its 443rd. It takes no freezing hurt (`Skeleton.canFreeze` false).
#[test]
fn a_skeleton_in_powder_snow_shakes_and_becomes_a_stray() {
    let skeleton = placed(
        &made(items::SKELETON_SPAWN_EGG, entity_types::SKELETON),
        1,
        feet(2, 2),
    );
    let lived = skeleton.lived;
    let world = World::of(&snow_well(2, 2)).with_entities(&[skeleton]);
    let mut scene = scene_at(&world, Inventory::default(), NIGHT);
    let watched = watch(&mut scene, key(1), lived, STRAY_CONVERSION, 600);
    let last = watched.last.expect("it was there");
    assert_eq!(health(&last), 20.0, "no freezing hurt");
    assert_eq!(watched.flagged, Some(142), "shaking after its 142nd tick");
    let (tick, output) = watched.gone.expect("it converts");
    assert_eq!(tick, 443, "a stray in its 443rd tick");
    let strays = scene.of_kind(entity_types::STRAY);
    assert_eq!(strays.len(), 1);
    let at = block_of(strays[0].position);
    assert!(
        level_events(&output).contains(&(SKELETON_CONVERTED, at, 0)),
        "{:?}",
        level_events(&output)
    );
}

/// K 11.2: the bogged has no freezing tracker and can freeze: in powder snow it stays a
/// bogged and is hurt by the cold.
#[test]
fn a_bogged_in_powder_snow_freezes_and_stays_a_bogged() {
    let bogged = placed(
        &made(items::BOGGED_SPAWN_EGG, entity_types::BOGGED),
        1,
        feet(2, 2),
    );
    let world = World::of(&snow_well(2, 2)).with_entities(&[bogged]);
    let mut scene = scene_at(&world, Inventory::default(), NIGHT);
    scene.idle_for(600);
    let bogged = the(&scene, key(1));
    assert!(scene.of_kind(entity_types::STRAY).is_empty(), "no stray");
    assert!(int_at(&bogged, index::TICKS_FROZEN) > 0, "it freezes");
    assert!(health(&bogged) < 16.0, "the cold hurts it");
}

/// K 11.2 (`Bogged.mobInteract`, `shear`): shears on a bogged that is not sheared play
/// `bogged_shear` from the players' source and drop two mushrooms, each brown or red, at
/// the top of its box (`getBbHeight()`, 1.99), and show it sheared; a second shearing does
/// nothing.
#[test]
fn shears_on_a_bogged_drop_two_mushrooms_from_its_top_once() {
    let mut scene = scene_at(
        &World::default(),
        inventory(&[
            (0, stack(items::BOGGED_SPAWN_EGG, 1)),
            (1, stack(items::SHEARS, 1)),
        ]),
        NIGHT,
    );
    let (_, made) = hatch(&mut scene, 0, 2, 2);
    let bogged = made[0];
    scene.idle();
    scene.select(1);
    let top = the(&scene, bogged).position[1] + 1.99;
    let output = scene.input(used(bogged));
    assert_eq!(
        sounds_of(&output, sounds::ENTITY_BOGGED_SHEAR),
        vec![PLAYERS_SOURCE]
    );
    let lying = items_lying(&scene.all());
    let mushrooms = lying_count(&scene.all(), items::BROWN_MUSHROOM)
        + lying_count(&scene.all(), items::RED_MUSHROOM);
    assert_eq!(mushrooms, 2, "two mushrooms: {lying:?}");
    for (stack, at) in &lying {
        assert!(
            [items::BROWN_MUSHROOM, items::RED_MUSHROOM].contains(&stack.item),
            "{stack:?}"
        );
        assert!(
            (at[1] - top).abs() < 0.25,
            "dropped from the top of its box, {top}: {at:?}"
        );
    }
    assert!(flag(&the(&scene, bogged), SHEARED), "shown sheared");
    let again = scene.input(used(bogged));
    assert!(sounds_of(&again, sounds::ENTITY_BOGGED_SHEAR).is_empty());
    let mushrooms = lying_count(&scene.all(), items::BROWN_MUSHROOM)
        + lying_count(&scene.all(), items::RED_MUSHROOM);
    assert_eq!(mushrooms, 2, "nothing more");
}

// ------------------------------------------------------------------------------------
// Plan 10 ("Despawning"): monsters far from players.

/// Plan 10 and `Mob.checkDespawn`: a monster from an egg is not persistent; one idle for
/// more than 600 ticks and more than 32 blocks from the nearest player goes one tick in
/// 800 (`nextInt(800) == 0`); one within 32 never does. A husk (which does not burn) in a
/// roofed well 39 blocks from the player (in a chunk that ticks: all its neighbours are
/// loaded) goes within 8000 ticks; one beside the player stays.
#[test]
fn a_monster_far_from_every_player_despawns_and_one_near_does_not() {
    let husk = made(items::HUSK_SPAWN_EGG, entity_types::HUSK);
    let mut blocks = well(28, 28, 3);
    blocks.push((bp(28, -57, 28), blocks::STONE));
    blocks.extend(well(3, 3, 3));
    blocks.push((bp(3, -57, 3), blocks::STONE));
    let world = World::of(&blocks)
        .with_entities(&[placed(&husk, 1, feet(28, 28)), placed(&husk, 2, feet(3, 3))]);
    let mut scene = scene_at(&world, Inventory::default(), NIGHT);
    // Nothing despawns until the region has heard of the world's players
    // (docs/groundwork/n9-plan.md, section 9.5): here, nobody but its own.
    scene.region.tick(&TickInputs {
        world_players: Some(Vec::new()),
        ..TickInputs::default()
    });
    let lived = the(&scene, key(1)).lived;
    let mut seen = Vec::new();
    let gone = scene.idle_until(8_000, &mut seen, |scene, _| record(scene, key(1)).is_none());
    let far = record(&scene, key(1)).map(|husk| husk.lived - lived);
    assert!(gone, "the far husk despawned (it ticked {far:?} times)");
    assert!(record(&scene, key(2)).is_some(), "the near husk stays");
    let removed = seen.iter().any(|output| {
        output.events.iter().any(|event| {
            matches!(event, RegionEvent::EntityGone { entity, .. } | RegionEvent::EntityRemoved { entity, .. } if *entity == key(1))
        })
    });
    assert!(removed, "the edges are told it is gone");
}

// ------------------------------------------------------------------------------------
// K 3.1: hostile sounds.

/// K 3.1 (`Monster`'s sound source `HOSTILE`): the undead's ambient sound, which an egg
/// plays as it makes the mob (`EntityType.spawn`'s `playAmbientSound`), and its hurt
/// sound are from the hostile source.
#[test]
fn the_undead_sound_from_the_hostile_source() {
    for (egg, ambient) in [
        (items::ZOMBIE_SPAWN_EGG, sounds::ENTITY_ZOMBIE_AMBIENT),
        (items::HUSK_SPAWN_EGG, sounds::ENTITY_HUSK_AMBIENT),
        (items::DROWNED_SPAWN_EGG, sounds::ENTITY_DROWNED_AMBIENT),
        (
            items::ZOMBIE_VILLAGER_SPAWN_EGG,
            sounds::ENTITY_ZOMBIE_VILLAGER_AMBIENT,
        ),
        (items::SKELETON_SPAWN_EGG, sounds::ENTITY_SKELETON_AMBIENT),
        (items::STRAY_SPAWN_EGG, sounds::ENTITY_STRAY_AMBIENT),
        (items::BOGGED_SPAWN_EGG, sounds::ENTITY_BOGGED_AMBIENT),
        (items::PARCHED_SPAWN_EGG, sounds::ENTITY_PARCHED_AMBIENT),
    ] {
        let mut scene = scene_at(&World::default(), inventory(&[(0, stack(egg, 1))]), NIGHT);
        let (output, _) = hatch(&mut scene, 0, 2, 2);
        assert_eq!(
            sounds_of(&output, ambient),
            vec![HOSTILE],
            "the egg of {egg} plays its ambient sound once, hostile"
        );
    }
    let mut scene = scene_at(
        &World::default(),
        inventory(&[(0, stack(items::ZOMBIE_SPAWN_EGG, 1))]),
        NIGHT,
    );
    let (_, made) = hatch(&mut scene, 0, 2, 2);
    scene.idle_for(2);
    let output = scene.input(PlayerInput::Attack {
        part: None,
        entity: made[0],
    });
    assert_eq!(
        sounds_of(&output, sounds::ENTITY_ZOMBIE_HURT),
        vec![HOSTILE],
        "hurt, hostile"
    );
}
