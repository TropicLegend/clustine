//! Tests of the custom spawners and patrols from the game's rules
//! (`docs/groundwork/n9-plan.md`, sections 8.6 and 9.7; n9 PhantomSpawner, n9
//! PatrolSpawner, n9 PatrollingMonster and its `LongDistancePatrolGoal`): each draw is
//! replayed from the stream the rule says it is drawn from.

use clustine_data::{blocks, entity_types};
use clustine_world::InChunk;
use clustine_world::poi::{Claim, PoiType};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, EntityIds, KeptBox, KeptKind, KeptStructure, PlayerId, Vec3,
};
use uuid::Uuid;

use super::super::tnt::tests::add_player;
use super::super::{Holdings, Region, RegionConfig};
use super::*;
use crate::api::{TickInputs, Ticket, WorldClock, WorldPlayer};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::patrol;
use crate::mobs::types::{illagers, phantom};
use crate::state::RegionState;

const SEED: u64 = 0x5_11;

/// The floor's top: stone at y = -61, so mobs and players stand at -60.
const FLOOR: i32 = -61;
const FEET: f64 = -60.0;

/// Midnight of the first day, and noon of the sixth (past the `early_game` timeline).
const MIDNIGHT: u64 = 18_000;
const NOON: u64 = 126_000;

/// The chunks from -`REACH` to `REACH` each way, all held and loaded.
const REACH: i32 = 6;

/// A chunk of the overworld with a stone floor and what `blocks` names, from y -64 up to
/// 120.
fn chunk_of(position: ChunkPos, blocks: &dyn Fn(BlockPos) -> Option<BlockState>) -> Chunk {
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("the game has the overworld");
    let mut chunk = Chunk::empty(overworld, Biome(0));
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, FLOOR, z, blocks::STONE);
            for y in -64..=120 {
                let pos = BlockPos::new(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                if let Some(state) = blocks(pos) {
                    chunk.set(x, y, z, state);
                }
            }
        }
    }
    chunk
}

/// A region holding the chunks from -6 to 6 each way, with `blocks` over the floor and,
/// where `village`, the chunk at the origin keeping a village's piece and a claimed bell;
/// ticked once, at the clock's day time `day`.
fn region_with(day: u64, blocks: impl Fn(BlockPos) -> Option<BlockState>, village: bool) -> Region {
    let held: Vec<ChunkPos> = (-REACH..=REACH)
        .flat_map(|x| (-REACH..=REACH).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = chunk_of(*position, &blocks);
            if village && *position == ChunkPos::new(0, 0) {
                let piece = KeptBox {
                    min: BlockPos::new(0, FLOOR, 0),
                    max: BlockPos::new(8, FLOOR + 6, 8),
                };
                chunk.set_structures(vec![KeptStructure {
                    kind: KeptKind::VillagePlains,
                    source: ChunkPos::new(0, 0),
                    bounds: piece,
                    pieces: vec![piece],
                }]);
                // A village is where villagers claim: a bell with its meeting point taken.
                chunk.set(4, FLOOR + 1, 4, blocks::BELL);
                chunk.set_claims(vec![Claim {
                    at: InChunk::new(4, FLOOR + 1, 4),
                    kind: PoiType::Meeting,
                    free: 31,
                }]);
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
        homes: Vec::new(),
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
    region
}

fn of_kind(region: &Region, id: i32) -> Vec<clustine_world::Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter().cloned())
        .filter(|entity| i32::from(entity.kind) == id)
        .collect()
}

fn mob_of(entity: &clustine_world::Entity) -> Mob {
    Mob::decode(&entity.data).expect("a mob")
}

// ---------------------------------------------------------------------------------
// The schedule.

#[test]
fn a_spawner_s_chain_starts_at_game_time_zero_and_each_delay_is_drawn_first_from_its_firing_s_stream()
 {
    let mut next = None;
    let fired = firings(SEED, PHANTOMS, phantom_delay, &mut next, 0, 20_000);
    let mut due = 0;
    let mut expected = Vec::new();
    while due <= 20_000 {
        let mut stream = Random::of_schedule(SEED, PHANTOMS, due);
        let delay = u64::from(60 + stream.next_int(60)) * 20;
        assert!((1_200..=2_380).contains(&delay));
        if due > 0 {
            expected.push((due, stream));
        }
        due += delay;
    }
    assert_eq!(
        fired, expected,
        "every firing after 0, with its stream on from the delay"
    );
    assert_eq!(
        next,
        Some(due),
        "the chain's end, the first due time after 20 000"
    );
}

#[test]
fn the_patrols_chain_steps_twelve_thousand_to_thirteen_thousand_two_hundred_ticks() {
    let mut next = None;
    let fired = firings(SEED, PATROLS, patrol_delay, &mut next, 0, 200_000);
    let dues: Vec<u64> = fired.iter().map(|(due, _)| *due).collect();
    assert!(!dues.is_empty());
    let mut before = 0;
    for due in dues {
        assert!(
            (12_000..13_200).contains(&(due - before)),
            "{before} to {due}"
        );
        before = due;
    }
}

#[test]
fn a_jump_of_the_clock_fires_every_due_time_it_passes_once_and_the_kept_end_goes_on_from_there() {
    // Ticks one at a time up to 50 000 and then a jump to 90 000, against one look at
    // the whole span: the same firings.
    let mut whole_next = None;
    let whole = firings(SEED, PHANTOMS, phantom_delay, &mut whole_next, 10, 90_000);
    let mut next = None;
    let mut fired = Vec::new();
    let mut last = 10;
    for now in (11..=50_000).step_by(997).chain([50_000, 90_000]) {
        fired.extend(firings(SEED, PHANTOMS, phantom_delay, &mut next, last, now));
        last = now;
    }
    assert_eq!(fired, whole);
    assert_eq!(next, whole_next);
    assert!(whole.iter().filter(|(due, _)| *due > 50_000).count() >= 10);
    // Each region computes the same: another world's chain differs.
    let mut other = None;
    let elsewhere = firings(SEED + 1, PHANTOMS, phantom_delay, &mut other, 10, 90_000);
    assert_ne!(elsewhere, whole);
}

#[test]
fn a_region_keeps_its_previous_tick_s_game_time_and_its_chains_ends_across_a_jump() {
    let mut region = region_with(1_000, |_| None, false);
    assert_eq!(region.custom_spawners.last, Some(1_001));
    let jumped = WorldClock {
        game_time: 400_000,
        day: 400_000,
    };
    region.tick(&TickInputs {
        clock: Some(jumped),
        ..TickInputs::default()
    });
    assert_eq!(region.custom_spawners.last, Some(400_001));
    let phantoms = region.custom_spawners.phantoms.expect("computed");
    let patrols = region.custom_spawners.patrols.expect("computed");
    assert!(phantoms > 400_001 && phantoms <= 400_001 + 2_380);
    assert!(patrols > 400_001 && patrols <= 400_001 + 13_200);
}

// ---------------------------------------------------------------------------------
// Phantoms.

/// A player at the block `block`, awake for `awake` ticks.
fn sleeper(block: BlockPos, awake: u32, game_mode: u8) -> Sleeper {
    Sleeper {
        entity: EntityId(1_000_001),
        block,
        game_mode,
        time_since_rest: awake,
    }
}

/// What the replayed draws of the player's stream aside say of their phantoms: the
/// block and how many, or none, and how many draws the roll took.
fn replay_phantoms(region: &Region, sleeper: &Sleeper) -> (Option<(BlockPos, u32)>, u64) {
    let mut level = Random::of_entity_aside(SEED, sleeper.entity, 0, region.tick);
    let start = level.clone();
    let effective = region.difficulty_at(sleeper.block);
    let outcome = (|| {
        if effective <= level.next_float() * 3.0 {
            return None;
        }
        if level.next_int(sleeper.time_since_rest.clamp(1, i32::MAX as u32)) < 72_000 {
            return None;
        }
        let up = 20 + level.next_int(15) as i32;
        let east = -10 + level.next_int(21) as i32;
        let south = -10 + level.next_int(21) as i32;
        let block = sleeper.block;
        let pos = BlockPos::new(block.x + east, block.y + up, block.z + south);
        let count = 1 + level.next_int(2);
        Some((pos, count))
    })();
    (outcome, level.draws_since(&start))
}

/// Rolls for `sleeper` in `tries` regions of successive ticks, asserting each against
/// the replay; how many rolls made phantoms.
fn rolls_match_the_replay(day: u64, sleeper: &Sleeper, tries: u64) -> usize {
    let mut made = 0;
    for extra in 0..tries {
        let mut region = region_with(day, |_| None, false);
        for _ in 0..extra {
            region.tick(&TickInputs::default());
        }
        // The region's own phase has run in those ticks, for none of its players.
        assert!(of_kind(&region, entity_types::PHANTOM).is_empty());
        let (expected, draws) = replay_phantoms(&region, sleeper);
        let mut output = TickOutput::default();
        region.spawn_phantoms(&[*sleeper], &mut output);
        let phantoms = of_kind(&region, entity_types::PHANTOM);
        let drawn = region
            .aside_draws
            .get(&sleeper.entity)
            .copied()
            .unwrap_or(0);
        match expected {
            None => assert!(phantoms.is_empty(), "no phantom where the draws refuse"),
            Some((pos, count)) => {
                made += 1;
                assert_eq!(phantoms.len() as u32, count, "1 + nextInt(2) at easy");
                for made in &phantoms {
                    assert_eq!(
                        made.position,
                        [
                            f64::from(pos.x) + 0.5,
                            f64::from(pos.y),
                            f64::from(pos.z) + 0.5
                        ],
                        "snapped to the block's bottom middle"
                    );
                    let mob = mob_of(made);
                    assert_eq!(phantom::state_of(&mob).size, 0, "size 0");
                    assert_eq!(
                        phantom::state_of(&mob).anchor,
                        Some(BlockPos::new(pos.x, pos.y + 5, pos.z))
                    );
                }
                // Each phantom's `finalizeSpawn` draws Mob's three from the same stream.
                assert_eq!(drawn, draws + 3 * u64::from(count));
                continue;
            }
        }
        assert_eq!(drawn, draws, "the roll's draws, no more");
    }
    made
}

#[test]
fn a_player_awake_for_long_under_the_night_sky_gets_the_phantoms_the_replayed_draws_give() {
    let made = rolls_match_the_replay(MIDNIGHT, &sleeper(BlockPos::new(0, 70, 0), 1 << 30, 1), 24);
    assert!(made > 0, "about one roll in four makes phantoms");
}

#[test]
fn a_player_awake_for_less_than_seventy_two_thousand_ticks_never_gets_a_phantom() {
    let made = rolls_match_the_replay(MIDNIGHT, &sleeper(BlockPos::new(0, 70, 0), 71_999, 1), 24);
    assert_eq!(made, 0);
}

#[test]
fn no_phantom_comes_by_day_and_nothing_is_drawn() {
    let mut region = region_with(6_000, |_| None, false);
    let who = sleeper(BlockPos::new(0, 70, 0), 1 << 30, 1);
    region.spawn_phantoms(&[who], &mut TickOutput::default());
    assert!(of_kind(&region, entity_types::PHANTOM).is_empty());
    assert!(!region.aside_draws.contains_key(&who.entity));
}

#[test]
fn no_phantom_comes_below_sea_level_under_a_roof_or_to_a_spectator() {
    let roof = |pos: BlockPos| {
        (pos.y == 90 && pos.x.abs() < 3 && pos.z.abs() < 3).then_some(blocks::STONE)
    };
    let mut region = region_with(MIDNIGHT, roof, false);
    let low = sleeper(BlockPos::new(20, FEET as i32, 20), 1 << 30, 1);
    let roofed = sleeper(BlockPos::new(0, 70, 0), 1 << 30, 1);
    let spectator = sleeper(BlockPos::new(30, 70, 30), 1 << 30, SPECTATOR);
    for who in [low, roofed, spectator] {
        region.spawn_phantoms(&[who], &mut TickOutput::default());
        assert!(of_kind(&region, entity_types::PHANTOM).is_empty());
        assert!(
            !region.aside_draws.contains_key(&who.entity),
            "nothing drawn"
        );
    }
}

#[test]
fn a_region_s_phantoms_are_rolled_for_its_own_players_by_their_time_since_rest() {
    let mut region = region_with(MIDNIGHT, |_| None, false);
    let (id, key) = add_player(&mut region, 1, [0.5, 70.0, 0.5], true);
    let sleepers = region.sleepers();
    assert_eq!(sleepers.len(), 1);
    assert_eq!(sleepers[0].entity, key);
    assert_eq!(sleepers[0].block, BlockPos::new(0, 70, 0));
    assert_eq!(sleepers[0].game_mode, 1, "creative");
    let rested = region.players[&id].rested;
    assert_eq!(u64::from(sleepers[0].time_since_rest), region.tick - rested);
}

// ---------------------------------------------------------------------------------
// Patrols.

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

/// The patrol firing due at `due`: its stream on from the delay's draw.
fn patrol_stream(due: u64) -> Random {
    let mut stream = Random::of_schedule(SEED, PATROLS, due);
    patrol_delay(&mut stream);
    stream
}

/// What the replayed draws of a firing say: whether it goes on past the one in five,
/// which of `players` players it chose, and the leader's block from a player at the
/// origin's block.
fn replay_patrol(due: u64, players: u32) -> (bool, u32, BlockPos) {
    let mut stream = patrol_stream(due);
    if stream.next_int(5) != 0 {
        return (false, 0, BlockPos::new(0, 0, 0));
    }
    let chosen = stream.next_int(players);
    let mut offset = || {
        let distance = 24 + stream.next_int(24) as i32;
        if stream.next_bool() {
            -distance
        } else {
            distance
        }
    };
    let dx = offset();
    let dz = offset();
    (true, chosen, BlockPos::new(dx, FEET as i32, dz))
}

/// The first `count` due times (by the test's numbering) whose firings pass the one
/// in five.
fn passing(count: usize) -> Vec<u64> {
    (1..)
        .filter(|due| replay_patrol(*due, 1).0)
        .take(count)
        .collect()
}

#[test]
fn a_patrol_comes_one_firing_in_five_and_never_by_night() {
    let mut came = 0;
    for due in 1..40 {
        let mut region = region_with(NOON, |_| None, false);
        add_player(&mut region, 1, [0.5, FEET, 0.5], false);
        region.faces.world = Some(table(&[1]));
        region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
        let pillagers = of_kind(&region, entity_types::PILLAGER);
        let (passes, _, _) = replay_patrol(due, 1);
        assert_eq!(!pillagers.is_empty(), passes, "due {due}");
        came += usize::from(passes);
    }
    assert!((2..=16).contains(&came), "{came} of 39");
    let due = passing(1)[0];
    let mut region = region_with(MIDNIGHT + 120_000, |_| None, false);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
    assert!(of_kind(&region, entity_types::PILLAGER).is_empty());
}

#[test]
fn no_patrol_comes_before_the_early_game_ends_nor_before_the_world_s_players_are_known() {
    let due = passing(1)[0];
    let mut region = region_with(6_000, |_| None, false);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
    assert!(of_kind(&region, entity_types::PILLAGER).is_empty(), "day 0");
    let mut region = region_with(NOON, |_| None, false);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = None;
    region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
    assert!(
        of_kind(&region, entity_types::PILLAGER).is_empty(),
        "no table"
    );
}

#[test]
fn the_chosen_player_of_the_world_s_table_is_drawn_and_only_their_region_makes_the_patrol() {
    // The region holds player 2 of three; another region holds 1 and 3.
    let mut held = 0;
    let mut others = 0;
    for due in passing(12) {
        let mut region = region_with(NOON, |_| None, false);
        add_player(&mut region, 2, [0.5, FEET, 0.5], false);
        region.faces.world = Some(table(&[3, 1, 2]));
        region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
        let (_, chosen, _) = replay_patrol(due, 3);
        // In the order of ids: 1, 2, 3.
        let made = !of_kind(&region, entity_types::PILLAGER).is_empty();
        assert_eq!(made, chosen == 1, "due {due}: chose {chosen}");
        if chosen == 1 {
            held += 1;
        } else {
            others += 1;
        }
    }
    assert!(held > 0 && others > 0);
}

#[test]
fn a_patrol_s_leader_wears_the_ominous_banner_and_heads_for_a_target_found_from_the_origin() {
    let due = passing(1)[0];
    let mut region = region_with(NOON, |_| None, false);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
    let (_, _, leader_at) = replay_patrol(due, 1);
    let pillagers = of_kind(&region, entity_types::PILLAGER);
    // At easy the difficulty is below 1.125: two members.
    assert_eq!(pillagers.len(), 2);
    let (leaders, members): (Vec<_>, Vec<_>) = pillagers
        .iter()
        .partition(|entity| illagers::state_of(&mob_of(entity)).patrol_leader);
    assert_eq!(leaders.len(), 1);
    let leader = leaders[0];
    assert_eq!(
        leader.position,
        [f64::from(leader_at.x), FEET, f64::from(leader_at.z)],
        "at the block's corner, on the surface"
    );
    let mob = mob_of(leader);
    let raider = illagers::state_of(&mob);
    assert!(raider.patrolling);
    let target = raider.patrol_target.expect("a target");
    assert!((-500..500).contains(&target.x) && (-500..500).contains(&target.z));
    assert_eq!(target.y, 0, "found before the leader is placed");
    let head = mob.body.equipment.slots[usize::from(slots::HEAD)]
        .clone()
        .expect("a banner");
    assert_eq!(head, patrol::ominous_banner());
    assert_eq!(
        mob.body.equipment.drop_chance(usize::from(slots::HEAD)),
        2.0
    );
    let member = mob_of(members[0]);
    let raider = illagers::state_of(&member);
    assert!(raider.patrolling && !raider.patrol_leader);
    assert_eq!(raider.patrol_target, None);
    assert_eq!(member.body.equipment.slots[usize::from(slots::HEAD)], None);
    let [x, y, z] = members[0].position;
    assert_eq!(y, FEET);
    assert!((x - leader.position[0]).abs() <= 4.0 && (z - leader.position[2]).abs() <= 4.0);
}

#[test]
fn a_leader_that_cannot_stand_where_the_patrol_starts_ends_it() {
    let due = passing(1)[0];
    let (_, _, leader_at) = replay_patrol(due, 1);
    let rail = move |pos: BlockPos| (pos == leader_at).then_some(blocks::RAIL);
    let mut region = region_with(NOON, rail, false);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
    assert!(of_kind(&region, entity_types::PILLAGER).is_empty());
}

#[test]
fn no_patrol_comes_near_a_village() {
    let due = passing(1)[0];
    let mut region = region_with(NOON, |_| None, true);
    add_player(&mut region, 1, [0.5, FEET, 0.5], false);
    region.faces.world = Some(table(&[1]));
    region.spawn_patrol(patrol_stream(due), &mut TickOutput::default());
    assert!(of_kind(&region, entity_types::PILLAGER).is_empty());
}

// ---------------------------------------------------------------------------------
// The patrol goal.

/// A pillager at `position` made as an egg would make it, then given the patrol's
/// state `change` asks; laid in the region.
fn pillager(
    region: &mut Region,
    position: [f64; 3],
    change: impl FnOnce(&mut illagers::RaiderState),
) -> EntityId {
    raider_of(region, entity_types::PILLAGER, position, change)
}

/// A persistent raider of the type `kind` at `position`, its raider's state changed by
/// `change`.
fn raider_of(
    region: &mut Region,
    kind: i32,
    position: [f64; 3],
    change: impl FnOnce(&mut illagers::RaiderState),
) -> EntityId {
    let kind = kind as u16;
    let chunk = ChunkPos::containing(position[0], position[2]);
    let key = region.take_entity_key(chunk).expect("a key");
    let mut own = Random::of_entity(SEED, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    mob.body.flags.persistent = true;
    illagers::update(&mut mob, change);
    let mut record = crate::mobs::making::fresh_record(key, kind, uuid, position, 0.0, &mob);
    record.on_ground = true;
    region.add_fresh_entity(record, &mut TickOutput::default());
    key
}

fn raider(region: &Region, key: EntityId) -> illagers::RaiderState {
    let (_, entity) = region.find_entity(key).expect("the mob");
    illagers::state_of(&mob_of(entity))
}

#[test]
fn the_step_towards_a_target_is_ten_blocks_along_the_line_turned_aside_by_ninety_radians() {
    let target = BlockPos::new(100, FLOOR + 1, 0);
    let position = [0.5, FEET, 0.5];
    let step = patrol::step_towards(target, position);
    // By hand: the offset (-100, 0, 0.5) from the target's bottom middle turned by the
    // table's cos 90 and sin 90, scaled by 0.4, back about the target; ten along the
    // line to it.
    let cos = f64::from(clustine_noise::trig::cos(90.0));
    let sin = f64::from(clustine_noise::trig::sin(90.0));
    let (ox, oz) = (0.5 - 100.5, 0.5 - 0.5);
    let aside = [
        (ox * cos + oz * sin) * 0.4 + 100.5,
        (oz * cos - ox * sin) * 0.4 + 0.5,
    ];
    let (dx, dz) = (aside[0] - 0.5, aside[1] - 0.5);
    let dy = f64::from(FLOOR + 1) - FEET;
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    let expected = BlockPos::new(
        (0.5 + dx / length * 10.0).floor() as i32,
        (FEET + dy / length * 10.0).floor() as i32,
        (0.5 + dz / length * 10.0).floor() as i32,
    );
    assert_eq!(step, expected);
    let walked = f64::from(step.x.pow(2) + step.z.pow(2)).sqrt();
    assert!((8.0..=11.0).contains(&walked), "about ten blocks: {walked}");
}

#[test]
fn a_leader_walks_towards_its_target_and_gives_each_step_to_its_companions_within_sixteen() {
    let mut region = region_with(NOON, |_| None, false);
    let target = BlockPos::new(60, FLOOR + 1, 0);
    let leader = pillager(&mut region, [0.5, FEET, 0.5], |raider| {
        raider.patrol_leader = true;
        raider.patrolling = true;
        raider.patrol_target = Some(target);
    });
    let near = pillager(&mut region, [4.5, FEET, 4.5], |raider| {
        raider.patrolling = true
    });
    let far = pillager(&mut region, [0.5, FEET, 30.5], |raider| {
        raider.patrolling = true
    });
    let step = patrol::step_towards(target, [0.5, FEET, 0.5]);
    let step = BlockPos::new(step.x, FLOOR + 1, step.z);
    let mut given = None;
    for _ in 0..6 {
        region.tick(&TickInputs::default());
        given = given.or(raider(&region, near).patrol_target);
    }
    assert_eq!(given, Some(step), "the leader's first step");
    assert!(raider(&region, near).patrolling);
    assert_eq!(raider(&region, far).patrol_target, None, "beyond 16 blocks");
    let (_, entity) = region.find_entity(leader).expect("the leader");
    let mob = mob_of(entity);
    assert!(mob.body.path.followed.is_some(), "a path to its step");
    assert_eq!(mob.body.progress.speed, 0.595, "the leader's speed");
    for _ in 0..60 {
        region.tick(&TickInputs::default());
    }
    let (_, entity) = region.find_entity(leader).expect("the leader");
    assert!(
        entity.position[0] > 3.0,
        "it walked east: {:?}",
        entity.position
    );
    let (_, follower) = region.find_entity(near).expect("the companion");
    let follower_mob = mob_of(follower);
    assert_eq!(follower_mob.body.progress.speed, 0.7, "a companion's speed");
}

#[test]
fn a_leader_within_ten_blocks_of_its_target_finds_a_new_one_from_where_it_stands() {
    let mut region = region_with(NOON, |_| None, false);
    let target = BlockPos::new(3, FLOOR + 1, 0);
    let leader = pillager(&mut region, [0.5, FEET, 0.5], |raider| {
        raider.patrol_leader = true;
        raider.patrolling = true;
        raider.patrol_target = Some(target);
    });
    pillager(&mut region, [4.5, FEET, 4.5], |raider| {
        raider.patrolling = true
    });
    let mut found = None;
    for _ in 0..6 {
        region.tick(&TickInputs::default());
        let now = raider(&region, leader).patrol_target;
        if now != Some(target) {
            found = now;
            break;
        }
    }
    let found = found.expect("a new target");
    assert!((-500..500).contains(&found.x) && (-500..500).contains(&found.z));
    assert_eq!(found.y, FLOOR + 1, "at its own height");
}

#[test]
fn a_patrolling_mob_with_no_companion_left_stops_patrolling() {
    let mut region = region_with(NOON, |_| None, false);
    let alone = pillager(&mut region, [0.5, FEET, 0.5], |raider| {
        raider.patrol_leader = true;
        raider.patrolling = true;
        raider.patrol_target = Some(BlockPos::new(60, FLOOR + 1, 0));
    });
    for _ in 0..6 {
        region.tick(&TickInputs::default());
    }
    assert!(!raider(&region, alone).patrolling);
}

#[test]
fn a_patrol_target_given_across_a_line_is_taken_by_a_raider_and_by_nothing_else() {
    let mut region = region_with(NOON, |_| None, false);
    let raider_key = pillager(&mut region, [0.5, FEET, 0.5], |_| {});
    let pos = BlockPos::new(9, FLOOR + 1, 9);
    region.patrol_target_given(raider_key, pos, &mut TickOutput::default());
    let state = raider(&region, raider_key);
    assert_eq!(state.patrol_target, Some(pos));
    assert!(state.patrolling);
    let cow = {
        let kind = entity_types::COW as u16;
        let key = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
        let mut own = Random::of_entity(SEED, key, 0);
        let (uuid, mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
        let record =
            crate::mobs::making::fresh_record(key, kind, uuid, [2.5, FEET, 2.5], 0.0, &mob);
        region.add_fresh_entity(record, &mut TickOutput::default());
        key
    };
    let before = region.find_entity(cow).expect("the cow").1.clone();
    region.patrol_target_given(cow, pos, &mut TickOutput::default());
    assert_eq!(region.find_entity(cow).expect("the cow").1, &before);
}

#[test]
fn a_patrol_target_is_taken_by_a_ravager_and_by_a_raider_only_while_its_raid_is_not_active() {
    // `Raider.canJoinPatrol`: `!hasActiveRaid()`, the ravager a raider like the rest.
    let mut region = region_with(NOON, |_| None, false);
    let mut output = TickOutput::default();
    region.create_or_extend_raid(BlockPos::new(0, FLOOR + 1, 0), 1, &mut output);
    let id = region.own_raids()[0].1.id;
    region.change_raid(id, &mut output, |raid| raid.active = true);
    let member = pillager(&mut region, [0.5, FEET, 0.5], |state| state.raid = Some(id));
    let pos = BlockPos::new(9, FLOOR + 1, 9);
    region.patrol_target_given(member, pos, &mut output);
    assert_eq!(raider(&region, member).patrol_target, None);
    assert!(!raider(&region, member).patrolling);
    region.change_raid(id, &mut output, |raid| raid.active = false);
    region.patrol_target_given(member, pos, &mut output);
    assert_eq!(raider(&region, member).patrol_target, Some(pos));

    let ravager = raider_of(&mut region, entity_types::RAVAGER, [2.5, FEET, 2.5], |_| {});
    region.patrol_target_given(ravager, pos, &mut output);
    assert_eq!(raider(&region, ravager).patrol_target, Some(pos));
    assert!(raider(&region, ravager).patrolling);
}
