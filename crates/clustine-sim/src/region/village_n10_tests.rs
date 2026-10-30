//! Tests of the village's other mobs and spawners in a region
//! (`docs/groundwork/n10-plan.md`, section 10; the brief's section 8): the iron golem's
//! strolls back to and about a village, the cat spawner in a village and in a swamp hut,
//! the siege's roll and its zombies, the wandering trader's chance and its llamas, and
//! zombies walking through a village at night. The village is the kept villages' pieces
//! with occupied beds in it, so that it is one whether the region reads it by the
//! stand-in or by the claims (`region/pois.rs`).

use clustine_data::{BlockState, attributes, blocks, entity_types, items};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Entity, EntityChange, EntityId, EntityIds, ItemStack,
    KeptBox, KeptKind, KeptStructure, LeashHolder, PlayerId, Vec3,
};
use uuid::Uuid;

use super::super::custom_spawners::{
    CATS, MIN_TRADER_CHANCE, SIEGE_NIGHTS, SIEGE_ZOMBIES, TRADER_STAYS, TRADERS,
};
use super::super::tnt::tests::add_player;
use super::super::{Holdings, Random, Region, RegionConfig};
use crate::api::{TickInputs, TickOutput, Ticket, WorldClock, WorldPlayer};
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::brain::{self, MemoryKind, Value};
use crate::mobs::types::{villager, wandering_trader};
use crate::state::RegionState;

const SEED: u64 = 0x6_10;

/// The floor: stone from the bottom of the world to y = -41, so mobs and players stand
/// at -40, with land below them as the world has it.
const FLOOR: i32 = -41;
const FEET: f64 = -40.0;
const Y: i32 = -40;

/// Noon and midnight of the overworld clock's first day, kept off the cat spawner's
/// cadence of 1 200 so that building a region fires no cat.
const NOON: u64 = 6_100;
const MIDNIGHT: u64 = 18_100;

/// What the region's chunks keep of structures.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kept {
    Nothing,
    /// A village's piece, kept by every chunk, from -40 to 40 along x and z: every place
    /// within 72 of the origin is near it.
    Village,
    /// A village's piece kept by the chunk at the origin alone, from 0 to 15 along x and
    /// z: places 32 beyond it are near it.
    SmallVillage,
    /// A swamp hut's piece, kept by every chunk, over all the land.
    Hut,
}

/// A region holding the chunks from -`reach` to `reach` each way, a stone floor, beds'
/// heads at `beds` (each occupied), with `kept`; ticked once from the clock's `day`.
fn region_at(day: u64, reach: i32, kept: Kept, beds: &[BlockPos]) -> Region {
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("the game has the overworld");
    let held: Vec<ChunkPos> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let piece = |kind, min: (i32, i32, i32), max: (i32, i32, i32)| {
        let bounds = KeptBox {
            min: BlockPos::new(min.0, min.1, min.2),
            max: BlockPos::new(max.0, max.1, max.2),
        };
        KeptStructure {
            kind,
            source: ChunkPos::new(0, 0),
            bounds,
            pieces: vec![bounds],
        }
    };
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(overworld, Biome(0));
            for z in 0..16 {
                for x in 0..16 {
                    for y in -64..=FLOOR {
                        chunk.set(x, y, z, blocks::STONE);
                    }
                }
            }
            for bed in beds.iter().filter(|bed| bed.chunk() == *position) {
                let (x, z) = bed.in_chunk();
                chunk.set(x, bed.y, z, bed_head());
            }
            match kept {
                Kept::Nothing => {}
                Kept::Village => chunk.set_structures(vec![piece(
                    KeptKind::VillagePlains,
                    (-40, FLOOR, -40),
                    (40, FLOOR + 6, 40),
                )]),
                Kept::SmallVillage => {
                    if *position == ChunkPos::new(0, 0) {
                        chunk.set_structures(vec![piece(
                            KeptKind::VillagePlains,
                            (0, FLOOR, 0),
                            (15, FLOOR + 6, 15),
                        )]);
                    }
                }
                Kept::Hut => chunk.set_structures(vec![piece(
                    KeptKind::SwampHut,
                    (-200, -64, -200),
                    (200, 0, 200),
                )]),
            }
            (*position, chunk)
        })
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: held.clone(),
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let config = RegionConfig {
        dimension: Default::default(),
        spawn: Some(Vec3::new(0.5, FEET, 0.5)),
        starting_inventory: Default::default(),
        return_after: u64::MAX,
        lowest_y: -64,
        seed: SEED,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, Vec::new());
    region.clock = WorldClock {
        game_time: day,
        day,
    };
    region.tick(&TickInputs::default());
    for bed in beds {
        assert!(
            region.acquire_poi(*bed, &mut TickOutput::default()),
            "{bed:?}"
        );
    }
    region
}

fn bed_head() -> BlockState {
    blocks::RED_BED
        .with_property("part", "head")
        .expect("a bed has a part")
}

fn player_number(number: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(number))
}

/// The world's table of the players `numbers`, each at the origin.
fn table(numbers: &[u128]) -> Vec<WorldPlayer> {
    numbers
        .iter()
        .map(|number| WorldPlayer {
            dimension: Default::default(),
            player: player_number(*number),
            entity: EntityId(1_000_000 + *number as u64),
            position: [0.5, FEET, 0.5],
            game_mode: 1,
            sleeping: None,
        })
        .collect()
}

fn of_kind(region: &Region, id: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter().cloned())
        .filter(|entity| i32::from(entity.kind) == id)
        .collect()
}

fn mob_of(entity: &Entity) -> Mob {
    Mob::decode(&entity.data).expect("a mob")
}

/// A mob of the type `kind` keyed `key` at `position`, at its type's full health, with
/// what `prepare` does to it.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], prepare: impl FnOnce(&mut Mob)) -> Entity {
    let kind = kind as u16;
    let mut mob = Mob::fresh();
    mob.body.vitals.health =
        supplied(kind, attribute(attributes::MAX_HEALTH)).expect("a living type") as f32;
    mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    prepare(&mut mob);
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: Uuid::from_u128(u128::from(key.0)),
        position,
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
        data: mob.encode(),
    }
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the region holds the chunk")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn position_of(region: &Region, key: EntityId) -> [f64; 3] {
    region
        .find_entity(key)
        .map(|(_, entity)| entity.position)
        .expect("the mob is here")
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

/// Keys well clear of the region's own block of keys.
const GOLEM: EntityId = EntityId(5_000_001);
const VILLAGER: EntityId = EntityId(5_000_002);
const ZOMBIE: EntityId = EntityId(5_000_003);

// ---------------------------------------------------------------------------------
// The iron golem.

#[test]
fn a_golem_strayed_from_a_village_walks_back_into_it() {
    // The village's piece is at x 0 to 15, so the land from x -32 to 47 is near it; the
    // golem stands at x 72, two sections from the nearest section of the village.
    let beds: Vec<BlockPos> = (0..5).map(|x| BlockPos::new(4 + x * 2, Y, 8)).collect();
    let mut region = region_at(NOON, 7, Kept::SmallVillage, &beds);
    let golem = mob_at(GOLEM, entity_types::IRON_GOLEM, [72.5, FEET, 8.5], |_| {});
    lay(&mut region, &golem);
    assert!(!region.close_to_village(BlockPos::new(72, Y, 8), 1));
    let mut back = None;
    for tick in 0..4_000 {
        idle(&mut region);
        let at = crate::physics::containing(position_of(&region, GOLEM));
        if region.close_to_village(at, 1) {
            back = Some(tick);
            break;
        }
    }
    assert!(back.is_some(), "it came back into the village");
}

/// Of the strolls a golem in a village takes in 8 000 ticks with a villager 30 blocks
/// off that slept lately (seen a golem lately or not; standing still, with no AI, so
/// that what it shows stays), how many head nearer the villager than the golem stands,
/// and how many there were.
fn strolls_nearer_a_villager(golem_seen: bool) -> (usize, usize) {
    // The village is where its beds are claimed, a row of them south of the walk.
    let beds: Vec<BlockPos> = (-2..=2).map(|x| BlockPos::new(x * 2, Y, -12)).collect();
    let mut region = region_at(NOON, 4, Kept::Village, &beds);
    let slept = i64::try_from(region.clock.game_time).expect("a game time");
    lay(
        &mut region,
        &mob_at(GOLEM, entity_types::IRON_GOLEM, [-14.5, FEET, 0.5], |_| {}),
    );
    lay(
        &mut region,
        &mob_at(VILLAGER, entity_types::VILLAGER, [15.5, FEET, 0.5], |mob| {
            mob.body.flags.no_ai = true;
            // What it shows is made from its memories at its every tick.
            let mut memories = brain::memories_of(&villager::PROVIDER, mob, 0);
            memories.set(MemoryKind::LastSlept, Value::Long(slept));
            if golem_seen {
                memories.set(MemoryKind::GolemDetectedRecently, Value::Bool(true));
            }
            brain::write_memories(&villager::PROVIDER, mob, memories);
        }),
    );
    let villager = position_of(&region, VILLAGER);
    let mut before = position_of(&region, GOLEM);
    let mut heading = None;
    let (mut nearer, mut all) = (0, 0);
    for _ in 0..8_000 {
        idle(&mut region);
        let golem = region.find_entity(GOLEM).expect("the golem").1.clone();
        let target = mob_of(&golem).body.path.followed.map(|path| path.target);
        if let Some(new) = target
            && target != heading
        {
            let target = at_bottom_centre(new);
            all += 1;
            if horizontal(target, villager) < horizontal(before, villager) {
                nearer += 1;
            }
        }
        heading = target;
        before = golem.position;
    }
    (nearer, all)
}

#[test]
fn a_golem_in_a_village_strolls_towards_a_villager_who_wants_a_golem() {
    // Seven strolls in ten are towards a villager who wants a golem (a place within a
    // right angle of it), the rest anywhere; with none who wants one, all are anywhere.
    let (nearer, all) = strolls_nearer_a_villager(false);
    assert!(all >= 15, "{all} strolls");
    assert!(nearer * 10 >= all * 6, "{nearer} of {all} nearer");
    let (seen_nearer, seen_all) = strolls_nearer_a_villager(true);
    assert!(seen_all >= 15, "{seen_all} strolls");
    assert!(
        seen_nearer * all < nearer * seen_all,
        "{seen_nearer} of {seen_all} nearer with a golem seen, {nearer} of {all} without"
    );
}

// ---------------------------------------------------------------------------------
// Cats.

/// The cat firing due at `due`, whose delay draws nothing.
fn cat_stream(due: u64) -> Random {
    Random::of_schedule(SEED, CATS, due)
}

/// Where the cat firing due at `due` looks, from a player at the origin's block, of a
/// table of one player.
fn replay_cat(due: u64) -> BlockPos {
    let mut stream = cat_stream(due);
    assert_eq!(stream.next_int(1), 0);
    let mut offset = || {
        let distance = 8 + stream.next_int(24) as i32;
        if stream.next_bool() {
            -distance
        } else {
            distance
        }
    };
    let dx = offset();
    let dz = offset();
    BlockPos::new(dx, FEET as i32, dz)
}

fn at_bottom_centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// Beds' heads every 16 blocks from -48 to 48 each way: a village by its claims over
/// all that land, as the kept piece makes one by the stand-in.
fn village_beds() -> Vec<BlockPos> {
    (-3..=3)
        .flat_map(|x| (-3..=3).map(move |z| BlockPos::new(x * 16 + 3, Y, z * 16 + 3)))
        .collect()
}

/// Five beds' heads near the origin.
fn five_beds() -> Vec<BlockPos> {
    (0..5).map(|x| BlockPos::new(x * 2 - 4, Y, -2)).collect()
}

#[test]
fn a_village_with_more_than_four_occupied_homes_gets_a_stray_cat_until_five_are_near() {
    let mut region = region_at(NOON, 6, Kept::Village, &five_beds());
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    for due in 1..=5 {
        region.spawn_cats(cat_stream(due), &mut TickOutput::default());
        let cats = of_kind(&region, entity_types::CAT);
        assert_eq!(cats.len(), due as usize, "firing {due}");
        let made = cats
            .iter()
            .find(|cat| cat.position == at_bottom_centre(replay_cat(due)))
            .expect("the cat stands where the replayed draws say");
        assert_eq!(made.yaw, 0.0);
        assert!(!mob_of(made).body.flags.persistent, "a stray");
    }
    // Five cats within 48: no sixth.
    region.spawn_cats(cat_stream(6), &mut TickOutput::default());
    assert_eq!(of_kind(&region, entity_types::CAT).len(), 5);
}

#[test]
fn a_village_with_four_occupied_homes_gets_no_cat() {
    let beds = five_beds();
    let mut region = region_at(NOON, 6, Kept::Village, &beds[..4]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    for due in 1..=5 {
        region.spawn_cats(cat_stream(due), &mut TickOutput::default());
    }
    assert!(of_kind(&region, entity_types::CAT).is_empty());
}

#[test]
fn a_swamp_hut_gets_a_persistent_cat_and_none_within_sixteen_of_another() {
    let mut region = region_at(NOON, 6, Kept::Hut, &[]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    region.spawn_cats(cat_stream(1), &mut TickOutput::default());
    let cats = of_kind(&region, entity_types::CAT);
    assert_eq!(cats.len(), 1);
    let first = replay_cat(1);
    assert_eq!(cats[0].position, at_bottom_centre(first));
    assert!(mob_of(&cats[0]).body.flags.persistent, "a hut's cat stays");
    // A firing whose place is within 16 (and 8 up and down) of the first cat makes none;
    // one beyond makes another.
    let near = (2..400)
        .find(|due| {
            let pos = replay_cat(*due);
            (pos.x - first.x).abs() <= 15 && (pos.z - first.z).abs() <= 15
        })
        .expect("a firing near the first cat");
    region.spawn_cats(cat_stream(near), &mut TickOutput::default());
    assert_eq!(
        of_kind(&region, entity_types::CAT).len(),
        1,
        "firing {near}"
    );
    let far = (2..400)
        .find(|due| {
            let pos = replay_cat(*due);
            (pos.x - first.x).abs() > 18 || (pos.z - first.z).abs() > 18
        })
        .expect("a firing far from the first cat");
    region.spawn_cats(cat_stream(far), &mut TickOutput::default());
    assert_eq!(of_kind(&region, entity_types::CAT).len(), 2, "firing {far}");
}

#[test]
fn no_cat_comes_outside_a_village_or_a_hut_nor_for_another_region_s_player() {
    let mut region = region_at(NOON, 6, Kept::Nothing, &[]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    for due in 1..=5 {
        region.spawn_cats(cat_stream(due), &mut TickOutput::default());
    }
    assert!(of_kind(&region, entity_types::CAT).is_empty(), "no village");
    let mut region = region_at(NOON, 6, Kept::Village, &five_beds());
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[2]));
    for due in 1..=5 {
        region.spawn_cats(cat_stream(due), &mut TickOutput::default());
    }
    assert!(of_kind(&region, entity_types::CAT).is_empty(), "not ours");
}

// ---------------------------------------------------------------------------------
// The siege.

/// The `roll_village_siege` marker of the night `night`.
fn roll_of(night: u64) -> u64 {
    night * 24_000 + 18_000
}

/// Whether the replayed roll of `night` makes a siege.
fn rolls_a_siege(night: u64) -> bool {
    Random::of_schedule(SEED, SIEGE_NIGHTS, roll_of(night)).next_int(10) == 0
}

/// The region's clock set to just before `tick`, and its spawners' last tick forgotten,
/// so that its next tick is `tick` and fires only what is due then.
fn just_before(region: &mut Region, tick: u64) {
    region.clock = WorldClock {
        game_time: tick - 1,
        day: tick - 1,
    };
    region.custom_spawners.last = None;
}

#[test]
fn the_night_s_roll_for_a_siege_is_one_in_ten_of_the_world_s_stream_at_the_marker() {
    let mut region = region_at(NOON, 1, Kept::Nothing, &[]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    let mut sieges = 0;
    for night in 0..60 {
        just_before(&mut region, roll_of(night));
        idle(&mut region);
        let expected = rolls_a_siege(night).then(|| player_number(1));
        assert_eq!(
            region.custom_spawners.siege.player, expected,
            "night {night}"
        );
        sieges += usize::from(expected.is_some());
    }
    assert!((1..=15).contains(&sieges), "{sieges} of 60");
    // At any other tick of the night nothing is rolled.
    let night = (0..60)
        .find(|night| rolls_a_siege(*night))
        .expect("a siege");
    just_before(&mut region, roll_of(night) + 1);
    region.custom_spawners.siege = Default::default();
    idle(&mut region);
    assert_eq!(region.custom_spawners.siege.player, None);
}

#[test]
fn a_siege_makes_up_to_twenty_zombies_one_every_third_tick_about_its_player_in_the_village() {
    let night = (0..200)
        .find(|night| rolls_a_siege(*night))
        .expect("a siege");
    let mut region = region_at(NOON, 6, Kept::Village, &village_beds());
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    just_before(&mut region, roll_of(night));
    let mut counts = Vec::new();
    let mut seen: Vec<EntityId> = Vec::new();
    for _ in 0..90 {
        idle(&mut region);
        let zombies = of_kind(&region, entity_types::ZOMBIE);
        // Where each was made: 32 off the player and 8 more at most along x and z.
        let new: Vec<&Entity> = zombies
            .iter()
            .filter(|zombie| !seen.contains(&zombie.key))
            .collect();
        for zombie in new {
            assert!(
                horizontal(zombie.position, [0.5, FEET, 0.5]) <= 32.0 + 8.0 * 1.5 + 2.0,
                "{:?}",
                zombie.position
            );
            seen.push(zombie.key);
        }
        counts.push(zombies.len());
    }
    assert!(region.custom_spawners.siege.set_up);
    // Made at the first tick and every third after.
    for (tick, pair) in counts.windows(2).enumerate() {
        if pair[1] > pair[0] {
            assert_eq!((tick + 1) % 3, 0, "a zombie at tick {}", tick + 1);
            assert_eq!(pair[1], pair[0] + 1);
        }
    }
    let made = *counts.last().expect("ticks");
    assert!(
        (SIEGE_ZOMBIES as usize * 3 / 4..=SIEGE_ZOMBIES as usize).contains(&made),
        "{made} zombies"
    );
    assert_eq!(
        region.custom_spawners.siege.player, None,
        "the siege is over"
    );
    // By day a siege is gone.
    region.custom_spawners.siege.player = Some(player_number(1));
    just_before(&mut region, roll_of(night) + 7_000);
    idle(&mut region);
    assert_eq!(region.custom_spawners.siege, Default::default());
}

#[test]
fn no_siege_is_set_up_for_a_player_outside_a_village() {
    let night = (0..200)
        .find(|night| rolls_a_siege(*night))
        .expect("a siege");
    let mut region = region_at(NOON, 4, Kept::Nothing, &[]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    just_before(&mut region, roll_of(night));
    for _ in 0..30 {
        idle(&mut region);
    }
    assert_eq!(
        region.custom_spawners.siege.player,
        Some(player_number(1)),
        "a siege tonight"
    );
    assert!(!region.custom_spawners.siege.set_up);
    assert!(of_kind(&region, entity_types::ZOMBIE).is_empty());
}

// ---------------------------------------------------------------------------------
// The wandering trader.

fn trader_stream(due: u64) -> Random {
    Random::of_schedule(SEED, TRADERS, due)
}

#[test]
fn the_trader_s_chance_rises_by_twenty_five_to_seventy_five_while_no_trader_comes() {
    // The world's one player is another region's: this region makes no trader.
    let mut region = region_at(NOON, 1, Kept::Nothing, &[]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[2]));
    assert_eq!(region.trader_chance(), MIN_TRADER_CHANCE);
    let mut chances = Vec::new();
    for firing in 1..=4 {
        region.trader_fired(
            firing * 24_000,
            trader_stream(firing * 24_000),
            &mut TickOutput::default(),
        );
        chances.push(region.trader_chance());
    }
    assert_eq!(chances, [50, 75, 75, 75]);
    assert!(of_kind(&region, entity_types::WANDERING_TRADER).is_empty());
}

#[test]
fn with_no_player_in_the_world_a_firing_that_passes_its_chance_sets_it_back_to_twenty_five() {
    let mut region = region_at(NOON, 1, Kept::Nothing, &[]);
    region.faces.world = Some(Vec::new());
    for firing in 1..=12 {
        let due = firing * 24_000;
        let old = region.trader_chance();
        let passes = trader_stream(due).next_int(100) as i32 <= old;
        region.trader_fired(due, trader_stream(due), &mut TickOutput::default());
        let expected = if passes {
            MIN_TRADER_CHANCE
        } else {
            (old + 25).min(75)
        };
        assert_eq!(region.trader_chance(), expected, "firing {firing}");
    }
}

#[test]
fn a_trader_comes_with_two_llamas_leashed_to_it_and_stays_forty_eight_thousand_ticks() {
    let mut region = region_at(NOON, 6, Kept::Nothing, &[]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    let mut came = None;
    for firing in 1..600 {
        let due = firing * 24_000;
        region.custom_spawners.trader_chance = Some((0, 75));
        region.trader_fired(due, trader_stream(due), &mut TickOutput::default());
        if !of_kind(&region, entity_types::WANDERING_TRADER).is_empty() {
            came = Some(firing);
            break;
        }
        assert_eq!(region.trader_chance(), 75, "firing {firing}");
    }
    assert!(came.is_some(), "a trader came");
    assert_eq!(region.trader_chance(), MIN_TRADER_CHANCE, "back to 25");
    let traders = of_kind(&region, entity_types::WANDERING_TRADER);
    assert_eq!(traders.len(), 1);
    let trader = &traders[0];
    let spot = crate::physics::containing(trader.position);
    assert_eq!(trader.position, at_bottom_centre(spot));
    assert!(horizontal(trader.position, [0.5, FEET, 0.5]) <= 48.0 * 1.5);
    let mob = mob_of(trader);
    let (_, own) = wandering_trader::state_of(&mob);
    assert_eq!(own.despawn_delay, TRADER_STAYS);
    assert_eq!(own.wander_target, Some(spot));
    assert_eq!(mob.body.home.centre, Some(spot));
    assert_eq!(mob.body.home.radius, 16);
    let llamas = of_kind(&region, entity_types::TRADER_LLAMA);
    assert_eq!(llamas.len(), 2);
    for llama in &llamas {
        assert!(horizontal(llama.position, trader.position) <= 4.0 * 1.5);
        let leash = llama.riding.leash.as_ref().expect("leashed");
        assert_eq!(leash.holder, LeashHolder::Entity(trader.key));
    }
}

#[test]
fn a_trader_s_llama_leaves_with_its_trader() {
    let mut region = region_at(NOON, 4, Kept::Nothing, &[]);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    for firing in 1..600 {
        let due = firing * 24_000;
        region.custom_spawners.trader_chance = Some((0, 75));
        region.trader_fired(due, trader_stream(due), &mut TickOutput::default());
        if !of_kind(&region, entity_types::WANDERING_TRADER).is_empty() {
            break;
        }
    }
    let trader = of_kind(&region, entity_types::WANDERING_TRADER)[0].key;
    // The trader about to leave: its llamas follow its delay, less one.
    let mut record = region.find_entity(trader).expect("the trader").1.clone();
    let mut mob = mob_of(&record);
    wandering_trader::update(&mut mob, |own| own.despawn_delay = 3);
    record.data = mob.encode();
    lay(&mut region, &record);
    for _ in 0..6 {
        idle(&mut region);
    }
    assert!(of_kind(&region, entity_types::TRADER_LLAMA).is_empty());
}

// ---------------------------------------------------------------------------------
// Zombies.

/// The zombie's `MoveThroughVillageGoal`, by its place among its goals.
const MOVE_THROUGH_VILLAGE: u16 = 5;

fn zombie_at(position: [f64; 3]) -> Entity {
    mob_at(ZOMBIE, entity_types::ZOMBIE, position, |mob| {
        mob.body.equipment.slots[usize::from(crate::living::slots::HEAD)] =
            Some(ItemStack::new(items::CARVED_PUMPKIN, 1));
    })
}

/// A row of occupied beds' heads 20 blocks east of the origin.
fn beds_east() -> Vec<BlockPos> {
    (0..4).map(|z| BlockPos::new(20, Y, z * 2)).collect()
}

#[test]
fn a_zombie_walks_through_a_village_at_night_to_its_occupied_points() {
    let beds = beds_east();
    let mut region = region_at(MIDNIGHT, 4, Kept::Village, &beds);
    lay(&mut region, &zombie_at([0.5, FEET, 2.5]));
    let mut ran = false;
    let mut nearest = f64::MAX;
    for _ in 0..600 {
        idle(&mut region);
        let Some((_, record)) = region.find_entity(ZOMBIE) else {
            break;
        };
        ran |= mob_of(record)
            .mind
            .goals
            .0
            .running
            .contains(&MOVE_THROUGH_VILLAGE);
        for bed in &beds {
            nearest = nearest.min(horizontal(record.position, at_bottom_centre(*bed)));
        }
    }
    assert!(ran, "`MoveThroughVillageGoal` ran");
    assert!(nearest < 6.0, "it came to the beds: {nearest}");
}

#[test]
fn a_zombie_does_not_walk_through_a_village_by_day_nor_outside_one() {
    let beds = beds_east();
    for (day, kept, beds) in [
        (NOON, Kept::Village, beds.clone()),
        (MIDNIGHT, Kept::Nothing, Vec::new()),
    ] {
        let mut region = region_at(day, 4, kept, &beds);
        lay(&mut region, &zombie_at([0.5, FEET, 2.5]));
        for _ in 0..200 {
            idle(&mut region);
            let Some((_, record)) = region.find_entity(ZOMBIE) else {
                break;
            };
            assert!(
                !mob_of(record)
                    .mind
                    .goals
                    .0
                    .running
                    .contains(&MOVE_THROUGH_VILLAGE),
                "{day} {kept:?}"
            );
        }
    }
}

#[test]
fn the_trader_s_chance_is_told_the_store_and_its_word_taken_but_for_a_later_firing_s() {
    // `docs/groundwork/n10-plan.md`, section 9.6.
    let mut region = region_at(NOON, 4, Kept::Nothing, &[]);
    region.take_trader_chance(24_000, 50);
    assert_eq!(region.trader_chance(), 50);
    // A firing with nobody in the world: the chance raised, then back to 25 (the game
    // counts no player as a spawn); the store is told the last.
    let mut output = TickOutput::default();
    region.trader_fired(48_000, trader_stream(48_000), &mut output);
    let told = output.trader_chance.expect("the store is told");
    assert_eq!(told.0, 48_000);
    assert_eq!(region.trader_chance(), told.1);
    // The store's word of an earlier firing does not undo it; of the same firing, the
    // lower stands; of a later one, it is taken.
    region.take_trader_chance(24_000, 75);
    assert_eq!(region.trader_chance(), told.1);
    region.take_trader_chance(48_000, MIN_TRADER_CHANCE);
    assert_eq!(region.trader_chance(), MIN_TRADER_CHANCE.min(told.1));
    region.take_trader_chance(72_000, 75);
    assert_eq!(region.trader_chance(), 75);
}
