//! Tests of raids, raiders and the village's spawners written from the specification by
//! someone who did not write the code (X1, part B2): `docs/groundwork/n10-dumps.md`, part
//! C (D10), the brief's sections 7 and 8 (V) and `docs/groundwork/n10-plan.md`, sections
//! 9 and 10, with the dumps they cite (`n7k Raid`, `n7k Raids`, `n7 Raider`, `n9
//! VillageSiege`, `n7 WanderingTraderSpawner`).
//!
//! A village is where a villager claimed something: here a bell whose meeting point is
//! taken before its chunk is loaded (`VILLAGES_CLAIMED`). The bell at (8, -60, 4) makes
//! the sections within one of its own a village: blocks x and z from -16 to 31.

use std::mem;

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::raid::{Raid, RaidBarName, RaidStatus};
use clustine_world::{
    Due, Effect as Made, EffectId, Effected, Entity, EntityChange, EntityData, EntityUse,
    FiledEffect, RaidNews,
};

use super::*;
use crate::api::{RaidBarChange, RaidSeen, WorldPlayer};
use crate::mobs::Mob;
use crate::mobs::body::effects::{Effect, Layer};
use crate::mobs::body::hurt::{self, Causer, DamageSource, DamageType};
use crate::mobs::types::illagers;
use crate::region::raids::RaidAt;

/// The bell of the tests' village, its meeting point taken.
const BELL: BlockPos = BlockPos::new(8, -60, 4);
/// Where most raids of these tests are made: in the bell's chunk.
const CENTRE: BlockPos = BlockPos::new(8, -60, 8);
/// A village in the land's north-western corner, and its raid's centre: the far corner
/// of the land is more than 112 from it, and more than 96.
const CORNER: BlockPos = BlockPos::new(-40, -60, -40);
/// The far corner's feet.
const FAR: [f64; 3] = [60.5, -60.0, 60.5];

/// The stone floor's top is y = -61, so the feet of what stands on it are at -60.
fn floor_of(state: BlockState) -> Chunk {
    let mut chunk = chunk();
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -61, z, state);
        }
    }
    chunk
}

/// A bell at `pos` in `chunk`, its meeting point taken by a villager.
fn claim_bell(chunk: &mut Chunk, pos: BlockPos) {
    let (x, z) = pos.in_chunk();
    chunk.set(x, pos.y, z, blocks::BELL);
    assert!(chunk.acquire_ticket(clustine_world::InChunk::new(x, pos.y, z)));
}

/// A region pinned to the whole world holding the chunks x and z -3 to 3 (those within
/// two of the middle tick), with a floor of `floor`, claimed bells at `bells`, and the
/// players `players` in it at the spawn point (0.5, -60, 0.5).
fn land_with(floor: BlockState, bells: &[BlockPos], players: &[u128]) -> Region {
    let reach = 3;
    let mut region = joined_in(ChunkArea::EVERYWHERE, players);
    let around: Vec<ChunkPos> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let loaded = around
        .iter()
        .map(|position| {
            let mut chunk = floor_of(floor);
            for bell in bells.iter().filter(|bell| bell.chunk() == *position) {
                claim_bell(&mut chunk, *bell);
            }
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: loaded,
        ..TickInputs::default()
    });
    let edge = ChunkPos::new(reach - 1, reach - 1);
    for _ in 0..100 {
        if region.ticks_chunk(edge) && region.ticks_chunk(ChunkPos::new(-2, -2)) {
            break;
        }
        idle(&mut region);
    }
    assert!(region.ticks_chunk(edge), "the chunks within two tick");
    region
}

/// The village of the bell at [`BELL`], with the player 1 in it.
fn village() -> Region {
    let region = land_with(blocks::STONE, &[BELL], &[1]);
    assert!(region.close_to_village(CENTRE, 1));
    region
}

/// A raid of the omen `omen` made at `centre` (`createOrExtendRaid`'s end).
fn raid_at(region: &mut Region, centre: BlockPos, omen: i32) -> EntityId {
    let mut output = TickOutput::default();
    region.create_or_extend_raid(centre, omen, &mut output);
    region
        .own_raids()
        .iter()
        .find(|(_, raid)| raid.centre == centre)
        .map(|(_, raid)| raid.id)
        .expect("a raid made at the centre")
}

fn raid(region: &Region, id: EntityId) -> Raid {
    region.find_raid(id).expect("the raid is kept").1.clone()
}

/// One tick of the region's raids alone (`Raids.tick`), with its bar's players, at the
/// next game time (the bar's players are gathered every twentieth).
fn tick_raids(region: &mut Region) -> TickOutput {
    region.clock.game_time += 1;
    let mut output = TickOutput::default();
    region.tick_raids(&mut output);
    output
}

/// The raids' ticks until the raid's next wave comes; what each of them put out, the
/// last the wave's.
fn until_next_wave(region: &mut Region, id: EntityId) -> Vec<TickOutput> {
    let groups = raid(region, id).groups_spawned;
    let mut outputs = Vec::new();
    for _ in 0..400 {
        outputs.push(tick_raids(region));
        if raid(region, id).groups_spawned > groups {
            return outputs;
        }
    }
    panic!("no wave came");
}

/// Every member of the raid dies, by the hand of `killer` where it names one, as their
/// regions would tell it.
fn kill_all(region: &mut Region, id: EntityId, killer: Option<uuid::Uuid>) {
    let mut output = TickOutput::default();
    for member in raid(region, id).members {
        region.raid_news(
            RaidNews::Died {
                raid: id,
                member: member.key,
                killer,
            },
            &mut output,
        );
    }
}

/// The record of the entity `key`, wherever the region keeps it.
fn record_of(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .expect("the entity is here")
        .1
        .clone()
}

fn raider_state(region: &Region, key: EntityId) -> illagers::RaiderState {
    illagers::state_of(&Mob::decode(&record_of(region, key).data).expect("a mob"))
}

/// A raider of the type `kind` keyed `key` at `at`, its raider's state changed by
/// `change`; its health.
fn lay_raider(
    region: &mut Region,
    key: EntityId,
    kind: i32,
    at: [f64; 3],
    change: impl FnOnce(&mut illagers::RaiderState),
) -> f32 {
    let kind = kind as u16;
    let mut own = Random::of_entity(region.config.seed, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    illagers::update(&mut mob, change);
    let health = mob.body.vitals.health;
    let mut record = crate::mobs::making::fresh_record(key, kind, uuid, at, 0.0, &mob);
    record.on_ground = true;
    let chunk = record.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the region holds the chunk")
        .apply_entity(&EntityChange::Whole(record));
    region.entity_chunks.insert(key, chunk);
    health
}

/// A raider of the region that joined the raid `id` on its wave `wave`, as its
/// `aiStep` tells the raid.
fn join_raider(
    region: &mut Region,
    id: EntityId,
    key: EntityId,
    kind: i32,
    at: [f64; 3],
    wave: i32,
) -> f32 {
    let health = lay_raider(region, key, kind, at, |raider| {
        raider.can_join_raid = true;
        raider.raid = Some(id);
        raider.wave = wave;
    });
    region.raider_news(
        key,
        None,
        crate::mobs::ai::raider_goals::RaiderNews::Joined {
            raid: id,
            wave,
            health,
        },
        &mut TickOutput::default(),
    );
    health
}

/// A hurt by the player `hero`, standing near.
fn by_player(hero: uuid::Uuid) -> DamageSource {
    DamageSource::by(
        DamageType::named("minecraft:player_attack").expect("a damage type"),
        Causer {
            key: EntityId(1),
            kind: crate::region::mob_bodies::player_kind(),
            position: [4.5, -60.0, 6.5],
            player: Some(hero.as_u128()),
            living: true,
        },
    )
}

/// What the region told its players of raids' bars.
fn bars(output: &TickOutput, to: PlayerId) -> Vec<RaidBarChange> {
    output
        .player_events
        .iter()
        .filter_map(|(player, event)| match event {
            PlayerEvent::RaidBar { change, .. } if *player == to => Some(*change),
            _ => None,
        })
        .collect()
}

fn horns(output: &TickOutput) -> Vec<clustine_world::Vec3> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound, position, ..
            } if *sound == sounds::EVENT_RAID_HORN => Some(*position),
            _ => None,
        })
        .collect()
}

/// How many numbers of the raid `id`'s stream its tick at `ticks_active` drew.
fn drawn(region: &Region, id: EntityId, ticks_active: u64) -> u64 {
    let start = Random::of_raid(region.config.seed, id, ticks_active);
    region.raid_stream(id, ticks_active).draws_since(&start)
}

/// How many numbers `findRandomSpawnPos(level, attempts)` draws when no attempt is taken:
/// its angle's `nextFloat`, then both `nextInt(3)` of every attempt (D10 C §4).
fn search_draws(region: &Region, id: EntityId, ticks_active: u64, attempts: u32) -> u64 {
    let start = Random::of_raid(region.config.seed, id, ticks_active);
    let mut stream = start.clone();
    stream.next_float();
    for _ in 0..attempts * 2 {
        stream.next_int(3);
    }
    stream.draws_since(&start)
}

fn move_player(region: &mut Region, number: u128, feet: [f64; 3]) {
    region
        .players
        .get_mut(&player(number))
        .expect("the player is here")
        .pose
        .position = Vec3::new(feet[0], feet[1], feet[2]);
}

fn raid_omen_left(region: &Region, number: u128) -> Option<i64> {
    let now = region.tick;
    region.players[&player(number)]
        .effects
        .active
        .iter()
        .find(|active| active.effect == Effect::RAID_OMEN)
        .map(|active| active.top().duration(now))
}

// ---------------------------------------------------------------------------------
// The omens (D10 C §2 and correction 1; plan 9.5).

#[test]
fn bad_omen_in_a_village_gives_raid_omen_at_its_level_and_goes_only_on_the_next_tick() {
    let mut region = village();
    let id = player(1);
    let now = region.tick;
    region.add_player_effect(
        id,
        Effect::BAD_OMEN,
        Layer::new(2, 120_000, now),
        &mut TickOutput::default(),
    );
    idle(&mut region);
    let effects = &region.players[&id].effects;
    assert_eq!(
        effects.amplifier(Effect::RAID_OMEN),
        Some(2),
        "at its level"
    );
    assert!(
        effects.has(Effect::BAD_OMEN),
        "the game's removal fails on the tick raid omen comes"
    );
    assert_eq!(
        region.players[&id].raid_omen_position,
        Some(BlockPos::new(0, -60, 0))
    );
    // A step aside, still in the village: on the next tick raid omen is made 600 again
    // with the place it is now, and bad omen goes.
    move_player(&mut region, 1, [3.5, -60.0, 1.5]);
    idle(&mut region);
    let effects = &region.players[&id].effects;
    assert!(!effects.has(Effect::BAD_OMEN));
    assert_eq!(effects.amplifier(Effect::RAID_OMEN), Some(2));
    assert!(
        raid_omen_left(&region, 1).is_some_and(|left| (599..=600).contains(&left)),
        "{:?}",
        raid_omen_left(&region, 1)
    );
    assert_eq!(
        region.players[&id].raid_omen_position,
        Some(BlockPos::new(3, -60, 1))
    );
}

#[test]
fn bad_omen_of_a_player_who_left_the_village_before_the_next_tick_stays_with_raid_omen_s_first_place()
 {
    let mut region = village();
    let id = player(1);
    let now = region.tick;
    region.add_player_effect(
        id,
        Effect::BAD_OMEN,
        Layer::new(0, 120_000, now),
        &mut TickOutput::default(),
    );
    idle(&mut region);
    assert!(region.players[&id].effects.has(Effect::RAID_OMEN));
    // Three sections west of the bell's: no village.
    move_player(&mut region, 1, [-40.5, -60.0, 0.5]);
    assert!(!region.close_to_village(BlockPos::new(-41, -60, 0), 1));
    for _ in 0..3 {
        idle(&mut region);
    }
    let player = &region.players[&id];
    assert!(player.effects.has(Effect::BAD_OMEN), "bad omen stays");
    assert!(player.effects.has(Effect::RAID_OMEN), "raid omen runs on");
    assert_eq!(player.raid_omen_position, Some(BlockPos::new(0, -60, 0)));
}

#[test]
fn bad_omen_near_a_raid_at_omen_five_gives_no_raid_omen_and_near_one_below_it_does() {
    let mut region = village();
    let raid_id = raid_at(&mut region, CENTRE, 5);
    let id = player(1);
    let now = region.tick;
    region.add_player_effect(
        id,
        Effect::BAD_OMEN,
        Layer::new(0, 120_000, now),
        &mut TickOutput::default(),
    );
    for _ in 0..3 {
        idle(&mut region);
    }
    let effects = &region.players[&id].effects;
    assert!(effects.has(Effect::BAD_OMEN));
    assert!(!effects.has(Effect::RAID_OMEN));
    region.change_raid(raid_id, &mut TickOutput::default(), |raid| {
        raid.omen_level = 4;
    });
    idle(&mut region);
    assert!(region.players[&id].effects.has(Effect::RAID_OMEN));
}

#[test]
fn a_raid_starts_six_hundred_ticks_after_bad_omen_turns_where_the_village_s_points_near_the_kept_place_average()
 {
    // Two bells: the mean of their places is (16, -60, 4).
    let second = BlockPos::new(24, -60, 4);
    let mut region = land_with(blocks::STONE, &[BELL, second], &[1]);
    let id = player(1);
    let now = region.tick;
    region.add_player_effect(
        id,
        Effect::BAD_OMEN,
        Layer::new(1, 120_000, now),
        &mut TickOutput::default(),
    );
    let turned = idle(&mut region).tick;
    assert!(region.players[&id].effects.has(Effect::RAID_OMEN));
    idle(&mut region);
    // Away from the second bell: within 64 of where the player now stands only the first
    // bell is, of the kept place both.
    move_player(&mut region, 1, [-47.5, -60.0, 0.5]);
    let mut started = None;
    for _ in 0..700 {
        let tick = idle(&mut region).tick;
        if !region.own_raids().is_empty() {
            started = Some(tick);
            break;
        }
    }
    let started = started.expect("a raid started");
    assert!(
        (600..=601).contains(&(started - turned)),
        "{} ticks after bad omen turned",
        started - turned
    );
    let raids = region.own_raids();
    assert_eq!(raids.len(), 1);
    assert_eq!(raids[0].1.centre, BlockPos::new(16, -60, 4));
    assert_eq!(raids[0].1.omen_level, 2, "amplifier 1 is omen 2");
    assert!(!region.players[&id].effects.has(Effect::RAID_OMEN));
    assert_eq!(region.players[&id].raid_omen_position, None);
}

#[test]
fn raid_omen_ending_where_no_village_is_leaves_no_raid_and_shows_no_bar() {
    let mut region = land_with(blocks::STONE, &[], &[1]);
    let id = player(1);
    let now = region.tick;
    region.add_player_effect(
        id,
        Effect::RAID_OMEN,
        Layer::new(0, 0, now),
        &mut TickOutput::default(),
    );
    region.players.get_mut(&id).unwrap().raid_omen_position = Some(CENTRE);
    let mut shown = Vec::new();
    for _ in 0..21 {
        let output = idle(&mut region);
        shown.extend(bars(&output, id));
    }
    assert!(!region.players[&id].effects.has(Effect::RAID_OMEN));
    assert!(
        region.own_raids().is_empty(),
        "made, then stopped on its first tick: no village and no wave"
    );
    assert!(shown.is_empty(), "{shown:?}");
}

#[test]
fn an_omen_absorbed_after_the_first_wave_gives_a_raid_begun_at_omen_one_its_bonus_wave() {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    until_next_wave(&mut region, id);
    // `createOrExtendRaid` absorbs while the raid is unstarted or below omen 5.
    let mut output = TickOutput::default();
    region.create_or_extend_raid(CENTRE.offset(3, 0, 0), 1, &mut output);
    assert_eq!(region.own_raids().len(), 1);
    assert_eq!(raid(&region, id).omen_level, 2);
    kill_all(&mut region, id, None);
    for wave in 2..=4 {
        until_next_wave(&mut region, id);
        assert_eq!(raid(&region, id).groups_spawned, wave);
        kill_all(&mut region, id, None);
    }
    for _ in 0..400 {
        tick_raids(&mut region);
        if raid(&region, id).status != RaidStatus::Ongoing {
            break;
        }
    }
    let kept = raid(&region, id);
    assert_eq!(kept.groups_spawned, 4, "three waves and the bonus wave");
    assert_eq!(kept.status, RaidStatus::Victory);
}

// ---------------------------------------------------------------------------------
// The raid's tick (V 7.2, D10 C §1 and §4).

#[test]
fn a_countdown_searches_every_fifth_tick_drawing_both_numbers_of_all_eight_attempts() {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    // While the countdown is at 240 or more, every place it tries is more than 66 from
    // the centre, where no square of ten about it is in the land: none is taken.
    for _ in 0..60 {
        let before = raid(&region, id).cooldown_ticks;
        tick_raids(&mut region);
        let kept = raid(&region, id);
        let expected = if before % 5 == 0 {
            search_draws(&region, id, kept.ticks_active, 8)
        } else {
            0
        };
        assert_eq!(
            drawn(&region, id, kept.ticks_active),
            expected,
            "the tick with the countdown at {before}"
        );
        assert_eq!(kept.spawn_pos, None);
        assert_eq!(kept.cooldown_ticks, before - 1);
    }
}

#[test]
fn a_raid_that_finds_no_place_for_its_wave_stops_after_six_failed_searches_in_its_last_countdown_tick()
 {
    // Nothing may spawn on barriers: no place is ever taken.
    let mut region = land_with(blocks::BARRIER, &[BELL], &[1]);
    let id = raid_at(&mut region, CENTRE, 1);
    for _ in 0..299 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    assert_eq!(kept.cooldown_ticks, 1);
    assert_eq!(kept.spawn_pos, None);
    let ticks = kept.ticks_active + 1;
    tick_raids(&mut region);
    assert!(
        region
            .find_raid(id)
            .is_none_or(|(_, raid)| raid.is_stopped()),
        "stopped"
    );
    assert_eq!(
        drawn(&region, id, ticks),
        6 * search_draws(&region, id, ticks, 20),
        "six searches of twenty attempts"
    );
    let raiders = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities())
        .filter(|entity| crate::mobs::ai::patrol::PATROLLING.contains(&entity.kind))
        .count();
    assert_eq!(raiders, 0);
}

#[test]
fn an_easy_raid_s_vindicators_carry_iron_axes_its_pillagers_crossbows_and_its_ravager_no_rider() {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    let mut vindicators = 0;
    let mut ravagers = 0;
    for _ in 1..=3 {
        until_next_wave(&mut region, id);
        for member in raid(&region, id).members {
            let record = record_of(&region, member.key);
            let mob = Mob::decode(&record.data).expect("a mob");
            let hand = mob.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)]
                .as_ref()
                .map(|stack| stack.item);
            match i32::from(record.kind) {
                entity_types::VINDICATOR => {
                    vindicators += 1;
                    assert_eq!(hand, Some(items::IRON_AXE), "`applyRaidBuffs`' axe");
                }
                entity_types::PILLAGER => assert_eq!(hand, Some(items::CROSSBOW)),
                entity_types::RAVAGER => ravagers += 1,
                other => panic!("an easy raid brought the type {other}"),
            }
            assert_eq!(record.riding.vehicle, None, "nothing rides on easy");
        }
        kill_all(&mut region, id, None);
    }
    assert!(vindicators >= 2, "the second wave's two at least");
    assert_eq!(ravagers, 1, "the third wave's");
}

#[test]
fn at_omen_five_axes_are_enchanted_in_any_wave_but_crossbows_only_after_easy_s_third() {
    // `getEnchantOdds()` is 0.75 at omen 5. The vindicator's axe takes `raid/vindicator`
    // at any wave; the pillager's crossbow is changed only beyond `getNumGroups(EASY)`.
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 5);
    let (mut axes, mut enchanted_axes) = (0, 0);
    let mut enchanted_crossbows = [0; 4];
    for wave in 1..=4 {
        until_next_wave(&mut region, id);
        for member in raid(&region, id).members {
            let record = record_of(&region, member.key);
            let mob = Mob::decode(&record.data).expect("a mob");
            let Some(hand) =
                mob.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)].clone()
            else {
                continue;
            };
            let enchanted = hand != ItemStack::new(hand.item, 1);
            match i32::from(record.kind) {
                entity_types::VINDICATOR => {
                    axes += 1;
                    enchanted_axes += usize::from(enchanted);
                }
                entity_types::PILLAGER => {
                    enchanted_crossbows[wave - 1] += usize::from(enchanted);
                }
                _ => {}
            }
        }
        kill_all(&mut region, id, None);
    }
    assert!(axes >= 2);
    assert!(
        enchanted_axes > 0,
        "{enchanted_axes} of {axes} axes enchanted"
    );
    assert_eq!(enchanted_crossbows[..3], [0, 0, 0], "none in easy's waves");
    assert!(
        enchanted_crossbows[3] > 0,
        "the bonus wave's: {enchanted_crossbows:?}"
    );
}

#[test]
fn the_bar_names_the_raiders_left_on_a_twentieth_active_tick_where_one_or_two_are() {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    until_next_wave(&mut region, id);
    let members = raid(&region, id).members;
    assert!(members.len() >= 4);
    let mut output = TickOutput::default();
    for member in &members[2..] {
        region.raid_news(
            RaidNews::Died {
                raid: id,
                member: member.key,
                killer: None,
            },
            &mut output,
        );
    }
    // Until the next twentieth active tick the name stays as it was.
    let mut told = Vec::new();
    loop {
        let output = tick_raids(&mut region);
        told.extend(bars(&output, player(1)));
        let kept = raid(&region, id);
        if kept.ticks_active.is_multiple_of(20) {
            assert_eq!(kept.bar_name, RaidBarName::RaidersRemaining(2));
            break;
        }
        assert_eq!(kept.bar_name, RaidBarName::Raid, "{}", kept.ticks_active);
    }
    assert!(
        told.contains(&RaidBarChange::Name(RaidBarName::RaidersRemaining(2))),
        "{told:?}"
    );
    region.raid_news(
        RaidNews::Died {
            raid: id,
            member: members[1].key,
            killer: None,
        },
        &mut output,
    );
    for _ in 0..19 {
        tick_raids(&mut region);
        assert_eq!(raid(&region, id).bar_name, RaidBarName::RaidersRemaining(2));
    }
    tick_raids(&mut region);
    assert_eq!(raid(&region, id).bar_name, RaidBarName::RaidersRemaining(1));
    // The last gone: the countdown starts again with the plain name.
    region.raid_news(
        RaidNews::Died {
            raid: id,
            member: members[0].key,
            killer: None,
        },
        &mut output,
    );
    tick_raids(&mut region);
    let kept = raid(&region, id);
    assert_eq!(kept.cooldown_ticks, 300);
    assert_eq!(kept.bar_name, RaidBarName::Raid);
}

#[test]
fn a_raider_killed_by_a_player_makes_a_hero_and_a_hit_shows_on_the_bar_at_the_raider_s_next_hit() {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    until_next_wave(&mut region, id);
    let wave = raid(&region, id);
    let total = wave.total_health.0;
    assert_eq!(wave.bar_progress, Float(1.0));
    let (killed, hit) = (wave.members[0].key, wave.members[1].key);
    let killed_health = wave.members[0].health.0;
    // `Raider.die` → `removeFromRaid(…, false)`: the health stays in the total, the bar
    // falls by it.
    let hero = player(1).0;
    let mut output = TickOutput::default();
    region.struck_between_ticks(killed, &mut output, |struck| {
        hurt::die(struck, &by_player(hero))
    });
    for _ in 0..2 {
        idle(&mut region);
    }
    let kept = raid(&region, id);
    assert!(kept.members.iter().all(|member| member.key != killed));
    assert_eq!(kept.heroes, [hero]);
    assert_eq!(kept.total_health, Float(total));
    assert_eq!(kept.bar_progress, Float((total - killed_health) / total));
    // `Raider.hurtServer` updates the bar before the hurt lands: the first hit shows only
    // with the second.
    let generic = DamageSource::of(DamageType("minecraft:generic"));
    region.struck_between_ticks(hit, &mut output, |struck| {
        hurt::hurt_server(struck, &generic, 5.0)
    });
    for _ in 0..20 {
        idle(&mut region);
    }
    assert_eq!(
        raid(&region, id).bar_progress,
        Float((total - killed_health) / total),
        "the first hit is not shown"
    );
    region.struck_between_ticks(hit, &mut output, |struck| {
        hurt::hurt_server(struck, &generic, 5.0)
    });
    for _ in 0..2 {
        idle(&mut region);
    }
    let kept = raid(&region, id);
    let living: f32 = kept.members.iter().map(|member| member.health.0).sum();
    assert_eq!(living, total - killed_health - 5.0);
    assert_eq!(kept.bar_progress, Float(living / total));
}

#[test]
fn a_loss_celebrates_with_the_defeat_bar_and_no_hero_and_stops_after_six_hundred_ticks() {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    until_next_wave(&mut region, id);
    let mut output = TickOutput::default();
    let first = raid(&region, id).members[0].key;
    region.raid_news(
        RaidNews::Died {
            raid: id,
            member: first,
            killer: Some(player(1).0),
        },
        &mut output,
    );
    tick_raids(&mut region);
    let progress = raid(&region, id).bar_progress;
    // The bell is broken: no village is left within two sections.
    region.set_block(BELL, blocks::AIR, &mut output);
    assert!(!region.close_to_village(CENTRE, 1));
    tick_raids(&mut region);
    assert_eq!(raid(&region, id).status, RaidStatus::Loss);
    for celebrated in 1..=599 {
        tick_raids(&mut region);
        let kept = raid(&region, id);
        assert_eq!(kept.celebration_ticks, celebrated);
        if celebrated >= 20 {
            assert_eq!(kept.bar_name, RaidBarName::Defeat);
            assert_eq!(
                kept.bar_progress, progress,
                "a loss keeps the bar's progress"
            );
            assert!(kept.bar_shown);
        }
    }
    tick_raids(&mut region);
    assert!(
        region
            .find_raid(id)
            .is_none_or(|(_, raid)| raid.is_stopped())
    );
    assert!(
        !region.players[&player(1)]
            .effects
            .has(Effect::HERO_OF_THE_VILLAGE),
        "a loss makes no hero"
    );
}

#[test]
fn victory_at_omen_three_gives_hero_of_the_village_two_to_a_hero_here_and_one_another_region_holds()
{
    let mut region = village();
    let elsewhere = PlayerId(uuid::Uuid::from_u128(0xE15E));
    region.faces.world = Some(vec![WorldPlayer {
        player: elsewhere,
        entity: EntityId(0x0E15E),
        position: [500.5, -60.0, 0.5],
        game_mode: 1,
        sleeping: None,
    }]);
    let id = raid_at(&mut region, CENTRE, 3);
    let mut waves = 0;
    while raid(&region, id).status == RaidStatus::Ongoing && waves < 6 {
        let kept = raid(&region, id);
        if kept.groups_spawned > kept.group_count {
            break;
        }
        until_next_wave(&mut region, id);
        waves += 1;
        let killer = if waves == 1 { player(1).0 } else { elsewhere.0 };
        kill_all(&mut region, id, Some(killer));
    }
    assert_eq!(waves, 4, "three waves and the bonus wave");
    for _ in 0..60 {
        tick_raids(&mut region);
        if raid(&region, id).status == RaidStatus::Victory {
            break;
        }
    }
    assert_eq!(raid(&region, id).status, RaidStatus::Victory);
    let effects = &region.players[&player(1)].effects;
    assert_eq!(effects.amplifier(Effect::HERO_OF_THE_VILLAGE), Some(2));
    let given = region.made.iter().find_map(|made| match &made.what {
        Effected::OnPlayer {
            player,
            by: EntityUse::Effects(given),
            ..
        } if *player == elsewhere => Some(given.instances.clone()),
        _ => None,
    });
    let given = given.expect("the hero elsewhere is sent the effect");
    assert_eq!(given.len(), 1);
    assert_eq!(given[0].effect, "minecraft:hero_of_the_village");
    assert_eq!(given[0].amplifier, 2);
    assert_eq!(given[0].duration, 48_000);
}

#[test]
fn a_member_112_or_more_from_the_centre_leaves_the_raid_with_its_health_on_a_twentieth_tick() {
    let mut region = land_with(blocks::STONE, &[CORNER], &[1]);
    let id = raid_at(&mut region, CORNER, 1);
    let key = EntityId(0x7001);
    let health = join_raider(&mut region, id, key, entity_types::PILLAGER, FAR, 0);
    assert_eq!(raid(&region, id).total_health, Float(health));
    let mut left = None;
    for _ in 0..20 {
        tick_raids(&mut region);
        if raid(&region, id).members.is_empty() {
            left = Some(raid(&region, id).ticks_active);
            break;
        }
    }
    assert_eq!(left.map(|ticks| ticks % 20), Some(0), "on a twentieth tick");
    let kept = raid(&region, id);
    assert_eq!(kept.total_health, Float(0.0), "`removeFromRaid(…, true)`");
    assert_eq!(raider_state(&region, key).raid, None);
}

/// A member of the raid `id` keyed `key` at `at` that has lived 6 000 ticks idle the
/// whole time.
fn idle_member(region: &mut Region, id: EntityId, key: EntityId, at: [f64; 3]) {
    join_raider(region, id, key, entity_types::PILLAGER, at, 0);
    let chunk = region.entity_chunks[&key];
    let mut record = record_of(region, key);
    let mut mob = Mob::decode(&record.data).expect("a mob");
    mob.body.idle.no_action_since = Some(0);
    record.lived = 6_000;
    record.data = mob.encode();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
}

#[test]
fn a_member_long_idle_outside_a_village_leaves_at_its_thirtieth_pass_and_one_inside_stays() {
    let mut region = land_with(blocks::STONE, &[CORNER], &[1]);
    let id = raid_at(&mut region, CORNER, 1);
    // 85 from the centre, three sections from the village's: outside it.
    let outside = EntityId(0x7501);
    idle_member(&mut region, id, outside, [20.5, -60.0, 20.5]);
    let inside = EntityId(0x7502);
    idle_member(&mut region, id, inside, [-36.5, -60.0, -36.5]);
    assert!(!region.close_to_village(BlockPos::new(20, -60, 20), 1));
    let mut passes = 0;
    while raid(&region, id)
        .members
        .iter()
        .any(|member| member.key == outside)
    {
        tick_raids(&mut region);
        if raid(&region, id).ticks_active.is_multiple_of(20) {
            passes += 1;
        }
        assert!(passes <= 30, "still a member after thirty passes");
    }
    assert_eq!(passes, 30, "it left at its thirtieth pass");
    assert_eq!(raider_state(&region, outside).raid, None);
    assert!(
        raid(&region, id)
            .members
            .iter()
            .any(|member| member.key == inside),
        "idle in the village, it stays"
    );
}

#[test]
fn a_countdown_takes_no_place_in_the_village_while_it_is_at_160_or_more() {
    // Bells every three sections: every place of the land is in the village.
    let sections = [-2, 1, 3];
    let bells: Vec<BlockPos> = sections
        .iter()
        .flat_map(|x| {
            sections
                .iter()
                .map(move |z| BlockPos::new(x * 16 + 8, -60, z * 16 + 8))
        })
        .collect();
    let mut region = land_with(blocks::STONE, &bells, &[1]);
    for x in [-48, 0, 47, 63] {
        for z in [-48, 0, 47, 63] {
            assert!(region.close_to_village(BlockPos::new(x, -60, z), 1));
        }
    }
    let id = raid_at(&mut region, CENTRE, 1);
    let mut taken = None;
    for _ in 0..300 {
        let before = raid(&region, id).cooldown_ticks;
        tick_raids(&mut region);
        if raid(&region, id).spawn_pos.is_some() {
            taken = Some(before);
            break;
        }
    }
    let taken = taken.expect("a place was taken");
    assert!(
        (140..160).contains(&taken),
        "taken at the countdown's {taken}: the first search below 160 finds one"
    );
}

#[test]
fn the_raid_moves_a_centre_off_the_village_to_the_middle_of_its_nearest_village_section_and_goes_on()
 {
    let mut region = village();
    // Three sections east of the bell's: the nearest village section is the bell's
    // eastern neighbour, at its own height.
    let off = BlockPos::new(56, -60, 8);
    assert!(!region.close_to_village(off, 1));
    let id = raid_at(&mut region, off, 1);
    tick_raids(&mut region);
    let (chunk, kept) = region.find_raid(id).expect("the raid goes on");
    assert_eq!(kept.centre, BlockPos::new(24, -56, 8));
    assert_eq!(kept.status, RaidStatus::Ongoing);
    assert_eq!(chunk, ChunkPos::new(1, 0), "its record with its centre");
}

// ---------------------------------------------------------------------------------
// The bar and the horn (D10 C §3; plan 9.4).

#[test]
fn players_coming_near_a_raid_are_shown_its_bar_at_the_next_gathering_and_those_leaving_lose_it() {
    let mut region = land_with(blocks::STONE, &[CORNER], &[1, 2]);
    raid_at(&mut region, CORNER, 1);
    move_player(&mut region, 2, FAR);
    let step = tick_raids;
    while region.clock.game_time % 20 != 19 {
        region.clock.game_time += 1;
    }
    let output = step(&mut region);
    assert!(matches!(
        bars(&output, player(1))[..],
        [RaidBarChange::Add {
            name: RaidBarName::Raid,
            ..
        }]
    ));
    assert!(bars(&output, player(2)).is_empty(), "141 from the centre");
    move_player(&mut region, 2, [0.5, -60.0, 4.5]);
    for _ in 0..19 {
        let output = step(&mut region);
        assert!(bars(&output, player(2)).is_empty(), "until the gathering");
    }
    let output = step(&mut region);
    assert!(matches!(
        bars(&output, player(2))[..],
        [RaidBarChange::Add { .. }]
    ));
    move_player(&mut region, 1, FAR);
    let mut told = Vec::new();
    for _ in 0..20 {
        let output = step(&mut region);
        told.extend(bars(&output, player(1)));
    }
    assert_eq!(told.last(), Some(&RaidBarChange::Remove));
    assert_eq!(
        told.iter()
            .filter(|change| **change == RaidBarChange::Remove)
            .count(),
        1
    );
}

#[test]
fn the_horn_sounds_once_a_wave_13_blocks_from_each_player_within_64_of_its_place_or_shown_the_bar()
{
    let mut region = land_with(blocks::STONE, &[CORNER], &[1, 2]);
    move_player(&mut region, 2, FAR);
    let id = raid_at(&mut region, CORNER, 1);
    for _ in 1..=3 {
        let outputs = until_next_wave(&mut region, id);
        let (wave, before) = outputs.split_last().expect("ticks");
        for output in before {
            assert!(horns(output).is_empty(), "no horn but a wave's");
        }
        // Where the wave came: its raiders are made a block above the place's middle.
        let member = raid(&region, id).members[0].key;
        let at = record_of(&region, member).position;
        let place = [at[0], at[1] - 0.5, at[2]];
        let mut expected = Vec::new();
        for number in [1, 2] {
            let feet = region.players[&player(number)].pose.position;
            let (dx, dz) = (place[0] - feet.x, place[2] - feet.z);
            let distance = (dx * dx + dz * dz).sqrt();
            let in_bar = matches!(
                region.get_raid_at(crate::physics::containing([feet.x, feet.y, feet.z])),
                Some(RaidAt::Own { id: found, .. }) if found == id
            );
            if distance <= 64.0 || in_bar {
                expected.push((
                    feet.x + 13.0 / distance * dx,
                    feet.y,
                    feet.z + 13.0 / distance * dz,
                ));
            }
        }
        let heard = horns(wave);
        assert_eq!(heard.len(), expected.len(), "{heard:?} {expected:?}");
        for (x, y, z) in expected {
            assert!(
                heard.iter().any(|horn| (horn.x - x).abs() < 1e-6
                    && horn.y == y
                    && (horn.z - z).abs() < 1e-6),
                "{heard:?} has none at {x} {y} {z}"
            );
        }
        kill_all(&mut region, id, None);
    }
}

// ---------------------------------------------------------------------------------
// A raid across a pinned line (plan 9.3), with a post office between two regions as in
// `effects.rs`: the western region keeps the raid and the village, the eastern one a
// pillager penned in at (24.5, -60, 8.5), within the village and 16 from the centre.

const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

/// Where the eastern pillager stands, in its pen.
const PENNED: [f64; 3] = [24.5, -60.0, 8.5];

struct Across {
    west: Region,
    east: Region,
    to_west: Vec<Made>,
    to_east: Vec<Made>,
    sequence: u64,
    /// The wave count the eastern region's table says, where the test makes it stale.
    stale_wave: Option<i32>,
}

fn across() -> Across {
    let mut land: BTreeMap<ChunkPos, Chunk> = (-2..=4)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .map(|position| (position, floor()))
        .collect();
    claim_bell(land.get_mut(&BELL.chunk()).unwrap(), BELL);
    // The pen: stone three high about the pillager's block.
    let pen = land.get_mut(&ChunkPos::new(1, 0)).unwrap();
    for (x, z) in [
        (7, 7),
        (8, 7),
        (9, 7),
        (7, 8),
        (9, 8),
        (7, 9),
        (8, 9),
        (9, 9),
    ] {
        for y in -60..=-58 {
            pen.set(x, y, z, blocks::STONE);
        }
    }
    let region_of = |area: ChunkArea, ids: u32, copied: i32| {
        let chunks: Vec<(ChunkPos, Chunk)> = land
            .iter()
            .filter(|(position, _)| area.contains(**position))
            .map(|(position, chunk)| (*position, chunk.clone()))
            .collect();
        let tickets: Vec<(ChunkPos, Ticket)> = chunks
            .iter()
            .map(|(position, _)| (*position, Ticket::Viewer))
            .collect();
        let holdings = Holdings {
            held: chunks.iter().map(|(position, _)| *position).collect(),
            pinned: vec![area],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let copies = land
            .iter()
            .filter(|(position, _)| position.x == copied)
            .map(|(position, chunk)| (*position, chunk.clone()))
            .collect();
        let state = RegionState::new(EntityIds::block(ids).expect("a block of ids"));
        Region::restore_loaded(config(), state, holdings, &tickets, chunks, copies)
    };
    let mut across = Across {
        west: region_of(WESTERN, 1, 1),
        east: region_of(EASTERN, 2, 0),
        to_west: Vec::new(),
        to_east: Vec::new(),
        sequence: 0,
        stale_wave: None,
    };
    for _ in 0..100 {
        if across.west.ticks_chunk(ChunkPos::new(0, 0))
            && across.east.ticks_chunk(ChunkPos::new(1, 0))
        {
            break;
        }
        across.step();
    }
    assert!(across.west.ticks_chunk(ChunkPos::new(0, 0)));
    assert!(across.east.ticks_chunk(ChunkPos::new(1, 0)));
    assert!(across.west.close_to_village(CENTRE, 1));
    across
}

impl Across {
    fn inputs(sequence: &mut u64, mail: Vec<Made>) -> TickInputs {
        *sequence += 1;
        let effects = mail
            .into_iter()
            .enumerate()
            .map(|(index, effect)| FiledEffect {
                id: EffectId {
                    sequence: *sequence,
                    index: u32::try_from(index).expect("few effects"),
                },
                effect,
                due: Due::FromTaking,
            })
            .collect();
        TickInputs {
            effects,
            ..TickInputs::default()
        }
    }

    /// One tick of both, each with what the other sent in its tick before and the
    /// other's raids as the store would publish them.
    fn step(&mut self) {
        self.step_told();
    }

    /// [`Across::step`], with what the eastern region put out.
    fn step_told(&mut self) -> TickOutput {
        let west_raids = self.west.report_raids(true).unwrap_or_default();
        let east_raids = self.east.report_raids(true).unwrap_or_default();
        let mut seen_east: Vec<RaidSeen> = west_raids;
        if let Some(wave) = self.stale_wave {
            for raid in &mut seen_east {
                raid.groups_spawned = wave;
            }
        }
        self.east.take_raid_table(&seen_east);
        self.west.take_raid_table(&east_raids);
        let west_inputs = Self::inputs(&mut self.sequence, mem::take(&mut self.to_west));
        let east_inputs = Self::inputs(&mut self.sequence, mem::take(&mut self.to_east));
        let west = self.west.tick(&west_inputs);
        let mut east = self.east.tick(&east_inputs);
        self.to_east = west.effects;
        self.to_west = mem::take(&mut east.effects);
        east
    }
}

const EAST_PILLAGER: EntityId = EntityId(0x9_0000_0001);

/// The two with the western raid's first wave come and the eastern pillager laid,
/// which may join a raid.
fn across_with_a_wave() -> (Across, EntityId, f32) {
    let mut across = across();
    let id = raid_at(&mut across.west, CENTRE, 1);
    for _ in 0..400 {
        across.step();
        if raid(&across.west, id).groups_spawned == 1 {
            break;
        }
    }
    assert_eq!(raid(&across.west, id).groups_spawned, 1);
    let health = lay_raider(
        &mut across.east,
        EAST_PILLAGER,
        entity_types::PILLAGER,
        PENNED,
        |raider| raider.can_join_raid = true,
    );
    (across, id, health)
}

#[test]
fn a_raider_across_a_pinned_line_joins_the_wave_and_its_hits_and_death_by_a_player_reach_the_raid()
{
    let (mut across, id, health) = across_with_a_wave();
    let wave_total = raid(&across.west, id).total_health.0;
    let mut joined = false;
    for _ in 0..60 {
        across.step();
        if raid(&across.west, id)
            .members
            .iter()
            .any(|member| member.key == EAST_PILLAGER)
        {
            joined = true;
            break;
        }
    }
    assert!(joined, "it joined across the line");
    let state = raider_state(&across.east, EAST_PILLAGER);
    assert_eq!((state.raid, state.wave), (Some(id), 1));
    let kept = raid(&across.west, id);
    assert_eq!(
        kept.total_health,
        Float(wave_total + health),
        "its health counted"
    );
    // Two hits a second apart: the raid hears its health as it was before each.
    let generic = DamageSource::of(DamageType("minecraft:generic"));
    for _ in 0..2 {
        across
            .east
            .struck_between_ticks(EAST_PILLAGER, &mut TickOutput::default(), |struck| {
                hurt::hurt_server(struck, &generic, 5.0)
            });
        for _ in 0..20 {
            across.step();
        }
    }
    let kept = raid(&across.west, id);
    let member = kept
        .members
        .iter()
        .find(|member| member.key == EAST_PILLAGER)
        .expect("still a member");
    assert_eq!(member.health, Float(health - 5.0));
    let living: f32 = kept.members.iter().map(|member| member.health.0).sum();
    assert_eq!(kept.bar_progress, Float(living / kept.total_health.0));
    // Its death by a player's hand: a hero, gone, its health kept in the total.
    let hero = uuid::Uuid::from_u128(0x4E70);
    across
        .east
        .struck_between_ticks(EAST_PILLAGER, &mut TickOutput::default(), |struck| {
            hurt::die(struck, &by_player(hero))
        });
    for _ in 0..3 {
        across.step();
    }
    let kept = raid(&across.west, id);
    assert!(
        kept.members
            .iter()
            .all(|member| member.key != EAST_PILLAGER)
    );
    assert_eq!(kept.heroes, [hero]);
    assert_eq!(kept.total_health, Float(wave_total + health));
}

#[test]
fn a_raider_across_a_line_joining_a_wave_gone_by_is_refused_and_forgets_the_raid() {
    let (mut across, id, _) = across_with_a_wave();
    // The eastern region's table still says no wave came.
    across.stale_wave = Some(0);
    let mut joined = false;
    let mut forgot = false;
    for _ in 0..80 {
        across.step();
        let state = raider_state(&across.east, EAST_PILLAGER);
        if state.raid == Some(id) {
            assert_eq!(state.wave, 0);
            joined = true;
        } else if joined {
            forgot = true;
            break;
        }
        assert!(
            raid(&across.west, id)
                .members
                .iter()
                .all(|member| member.key != EAST_PILLAGER),
            "refused"
        );
    }
    assert!(joined, "it joined by its stale table");
    assert!(forgot, "it forgot the raid that refused it");
}

#[test]
fn a_player_across_the_line_from_a_raid_is_shown_its_bar_from_the_table_and_its_changes() {
    let mut across = across();
    let (east_player, _) =
        super::super::tnt::tests::add_player(&mut across.east, 9, [24.5, -60.0, 0.5], false);
    let id = raid_at(&mut across.west, CENTRE, 1);
    let bar = raid(&across.west, id).bar;
    let mut told = Vec::new();
    for _ in 0..60 {
        let output = across.step_told();
        told.extend(
            output
                .player_events
                .iter()
                .filter_map(|(player, event)| match event {
                    PlayerEvent::RaidBar { id, change } if *player == east_player => {
                        Some((*id, *change))
                    }
                    _ => None,
                }),
        );
    }
    assert!(
        matches!(told.first(), Some((shown, RaidBarChange::Add { .. })) if *shown == bar),
        "{told:?}"
    );
    assert!(
        told.iter()
            .any(|(shown, change)| *shown == bar && matches!(change, RaidBarChange::Progress(_))),
        "the countdown's progress follows: {told:?}"
    );
}

// ---------------------------------------------------------------------------------
// Raiders and patrols (61d3c8e: `Raider.canJoinPatrol`).

/// Whether a patrol's leader, with only `companion` near it, keeps patrolling: the leader
/// and the companion may join no raid, but the companion where `in_raid` is a member of
/// an active raid. The companion's patrol target after.
fn patrol_with(in_raid: bool) -> (bool, Option<BlockPos>) {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    let leader = EntityId(0x7101);
    let companion = EntityId(0x7102);
    lay_raider(
        &mut region,
        leader,
        entity_types::PILLAGER,
        [0.5, -60.0, 12.5],
        |raider| {
            raider.patrol_leader = true;
            raider.patrolling = true;
            raider.patrol_target = Some(BlockPos::new(400, -60, 12));
        },
    );
    let at = [3.5, -60.0, 12.5];
    if in_raid {
        join_raider(&mut region, id, companion, entity_types::PILLAGER, at, 0);
    } else {
        lay_raider(&mut region, companion, entity_types::PILLAGER, at, |_| {});
    }
    for _ in 0..60 {
        idle(&mut region);
        let leading = raider_state(&region, leader);
        let followed = raider_state(&region, companion).patrol_target;
        if !leading.patrolling || followed.is_some() {
            return (leading.patrolling, followed);
        }
    }
    let leading = raider_state(&region, leader);
    (
        leading.patrolling,
        raider_state(&region, companion).patrol_target,
    )
}

#[test]
fn a_raider_of_an_active_raid_is_no_companion_of_a_patrol() {
    let (patrolling, followed) = patrol_with(false);
    assert!(patrolling, "a free pillager keeps the patrol going");
    assert!(followed.is_some(), "and is given its step");
    let (patrolling, followed) = patrol_with(true);
    assert!(!patrolling, "with only a raid's raider near, no companion");
    assert_eq!(followed, None);
}

#[test]
fn killing_a_patrol_s_captain_gives_its_killer_no_bad_omen() {
    // 26.3's `Raider.die` gives no bad omen: the captain's bottle is its loot.
    let mut region = land_with(blocks::STONE, &[], &[1]);
    let captain = EntityId(0x7201);
    lay_raider(
        &mut region,
        captain,
        entity_types::PILLAGER,
        [4.5, -60.0, 4.5],
        |raider| {
            raider.patrol_leader = true;
            raider.patrolling = true;
        },
    );
    region.struck_between_ticks(captain, &mut TickOutput::default(), |struck| {
        hurt::die(struck, &by_player(player(1).0))
    });
    for _ in 0..30 {
        idle(&mut region);
    }
    assert!(!region.players[&player(1)].effects.has(Effect::BAD_OMEN));
}

fn head_of(region: &Region, key: EntityId) -> Option<ItemStack> {
    let mob = Mob::decode(&record_of(region, key).data).expect("a mob");
    mob.body.equipment.slots[usize::from(crate::living::slots::HEAD)].clone()
}

#[test]
fn the_banner_a_wave_s_dying_leader_drops_is_taken_up_by_a_raider_of_the_wave_who_then_leads_it() {
    let mut region = village();
    let id = raid_at(&mut region, CENTRE, 1);
    until_next_wave(&mut region, id);
    let wave = raid(&region, id);
    let leader = wave
        .leaders
        .iter()
        .find(|(of, _)| *of == 1)
        .map(|(_, leader)| *leader)
        .expect("the wave's leader");
    let banner = crate::mobs::ai::patrol::ominous_banner();
    assert_eq!(head_of(&region, leader), Some(banner.clone()));
    region.struck_between_ticks(leader, &mut TickOutput::default(), |struck| {
        hurt::die(struck, &by_player(player(1).0))
    });
    // `setLeader` gives the banner a drop chance of 2: it always falls.
    let mut fell = false;
    let mut taken = None;
    for _ in 0..600 {
        idle(&mut region);
        fell |= region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities())
            .any(
                |entity| matches!(&entity.data, EntityData::Item { stack, .. } if *stack == banner),
            );
        taken = raid(&region, id)
            .members
            .iter()
            .map(|member| member.key)
            .find(|key| head_of(&region, *key).as_ref() == Some(&banner));
        if taken.is_some() {
            break;
        }
    }
    assert!(fell, "the dead leader's banner lay on the ground");
    let taken = taken.expect("a raider of the wave took the banner up");
    let kept = raid(&region, id);
    assert!(kept.leaders.contains(&(1, taken)), "{:?}", kept.leaders);
    assert!(raider_state(&region, taken).patrol_leader);
}

const RAVAGER: EntityId = EntityId(0x7301);
const VILLAGER: EntityId = EntityId(0x7302);

/// A ravager at (0.5, -60, 20.5) and a grown villager standing still (no AI) at `x` along
/// the same line; the villager's health.
fn ravager_and_villager(region: &mut Region, x: f64) -> f32 {
    lay_raider(
        region,
        RAVAGER,
        entity_types::RAVAGER,
        [0.5, -60.0, 20.5],
        |_| {},
    );
    let kind = entity_types::VILLAGER as u16;
    let mut own = Random::of_entity(region.config.seed, VILLAGER, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    mob.body.flags.no_ai = true;
    let full = mob.body.vitals.health;
    let mut record =
        crate::mobs::making::fresh_record(VILLAGER, kind, uuid, [x, -60.0, 20.5], 0.0, &mob);
    record.on_ground = true;
    let chunk = record.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    region.entity_chunks.insert(VILLAGER, chunk);
    full
}

fn health_of(region: &Region, key: EntityId) -> f32 {
    Mob::decode(&record_of(region, key).data)
        .expect("a mob")
        .body
        .vitals
        .health
}

#[test]
fn a_ravager_goes_for_a_grown_villager_it_sees_and_hits_it_for_twelve() {
    let mut region = land_with(blocks::STONE, &[], &[1]);
    let full = ravager_and_villager(&mut region, 7.5);
    let mut hit = None;
    for _ in 0..400 {
        idle(&mut region);
        let health = health_of(&region, VILLAGER);
        if health < full {
            hit = Some(health);
            break;
        }
    }
    assert_eq!(hit, Some(full - 12.0), "its first hit");
}

#[test]
fn a_ravager_stopped_by_leaves_on_its_way_to_its_target_breaks_through_them() {
    let mut region = land_with(blocks::STONE, &[], &[1]);
    let full = ravager_and_villager(&mut region, 12.5);
    let mut targeted = false;
    for _ in 0..100 {
        idle(&mut region);
        let mob = Mob::decode(&record_of(&region, RAVAGER).data).expect("a mob");
        if mob.body.combat.target == Some(VILLAGER) {
            targeted = true;
            break;
        }
    }
    assert!(targeted, "it took the villager as its target");
    // A wall of leaves three high across its way, set once it saw the villager.
    let ahead = record_of(&region, RAVAGER).position[0].floor() as i32 + 3;
    assert!(ahead < 11, "the wall is before the villager");
    let leaves = BlockState::parse("minecraft:oak_leaves[distance=1,persistent=true]")
        .expect("a block state");
    let mut wall = Vec::new();
    for z in 12..=29 {
        for y in -60..=-58 {
            let pos = BlockPos::new(ahead, y, z);
            region.set_block(pos, leaves, &mut TickOutput::default());
            wall.push(pos);
        }
    }
    let mut hit = false;
    for _ in 0..600 {
        idle(&mut region);
        if health_of(&region, VILLAGER) < full {
            hit = true;
            break;
        }
    }
    let broken = wall
        .iter()
        .filter(|pos| region.block(**pos) == Some(blocks::AIR))
        .count();
    assert!(broken > 0, "it broke leaves of the wall");
    assert!(hit, "and reached the villager");
}

// ---------------------------------------------------------------------------------
// The siege and the wandering trader (V 8.3, 8.4; D10 C §5).

/// The table of the world's players: the player 1, at the spawn point.
fn world_of_player_one(region: &mut Region) {
    let entity = region.players[&player(1)].entity_id;
    region.faces.world = Some(vec![WorldPlayer {
        player: player(1),
        entity,
        position: [0.5, -60.0, 0.5],
        game_mode: 1,
        sleeping: None,
    }]);
}

#[test]
fn every_zombie_of_a_siege_is_made_where_the_village_is() {
    use super::super::custom_spawners::SIEGE_NIGHTS;
    let second = BlockPos::new(-8, -60, -8);
    let mut region = land_with(blocks::STONE, &[BELL, second], &[1]);
    world_of_player_one(&mut region);
    let seed = region.config.seed;
    let night = (0..400)
        .find(|night| {
            Random::of_schedule(seed, SIEGE_NIGHTS, night * 24_000 + 18_000).next_int(10) == 0
        })
        .expect("a siege night");
    let roll = night * 24_000 + 18_000;
    region.clock = crate::api::WorldClock {
        game_time: roll - 1,
        day: roll - 1,
    };
    region.custom_spawners.last = None;
    let mut seen: Vec<EntityId> = Vec::new();
    for _ in 0..90 {
        idle(&mut region);
        let made: Vec<Entity> = region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities().iter().cloned())
            .filter(|entity| i32::from(entity.kind) == entity_types::ZOMBIE)
            .filter(|entity| !seen.contains(&entity.key))
            .collect();
        for zombie in made {
            let block = crate::physics::containing(zombie.position);
            assert!(
                region.close_to_village(block, 1),
                "a zombie made at {:?}",
                zombie.position
            );
            seen.push(zombie.key);
        }
    }
    assert!(!seen.is_empty(), "the siege made zombies");
}

#[test]
fn a_wandering_trader_comes_about_a_bell_within_48_of_its_player_and_not_about_the_player() {
    use super::super::custom_spawners::TRADERS;
    // The bell 24 east of the player: the trader's spot is within 48 of it, x from -24,
    // where one about the player would be from -48, a quarter of them west of -24.
    let bell = BlockPos::new(24, -60, 0);
    let mut region = land_with(blocks::STONE, &[bell], &[1]);
    world_of_player_one(&mut region);
    let seed = region.config.seed;
    for firing in 1..2_000 {
        let due = firing * 24_000;
        region.custom_spawners.trader_chance = Some((0, 75));
        region.trader_fired(
            due,
            Random::of_schedule(seed, TRADERS, due),
            &mut TickOutput::default(),
        );
        let traders = region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities())
            .filter(|entity| i32::from(entity.kind) == entity_types::WANDERING_TRADER)
            .count();
        if traders >= 30 {
            break;
        }
    }
    let spots: Vec<[f64; 3]> = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter().cloned())
        .filter(|entity| i32::from(entity.kind) == entity_types::WANDERING_TRADER)
        .map(|entity| entity.position)
        .collect();
    assert!(spots.len() >= 30, "{} traders", spots.len());
    for spot in &spots {
        assert!(
            (f64::from(bell.x - 48)..f64::from(bell.x + 48)).contains(&spot[0])
                && (f64::from(bell.z - 48)..f64::from(bell.z + 48)).contains(&spot[2]),
            "{spot:?}"
        );
    }
}

#[test]
fn a_trader_s_llama_turns_on_what_hurt_its_trader() {
    use super::super::custom_spawners::TRADERS;
    let mut region = land_with(blocks::STONE, &[], &[1]);
    world_of_player_one(&mut region);
    let seed = region.config.seed;
    let of_kind = |region: &Region, kind: i32| -> Vec<Entity> {
        region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities().iter().cloned())
            .filter(|entity| i32::from(entity.kind) == kind)
            .collect()
    };
    for firing in 1..2_000 {
        let due = firing * 24_000;
        region.custom_spawners.trader_chance = Some((0, 75));
        region.trader_fired(
            due,
            Random::of_schedule(seed, TRADERS, due),
            &mut TickOutput::default(),
        );
        if !of_kind(&region, entity_types::WANDERING_TRADER).is_empty() {
            break;
        }
    }
    let trader = of_kind(&region, entity_types::WANDERING_TRADER)
        .pop()
        .expect("a trader came");
    let llamas = of_kind(&region, entity_types::TRADER_LLAMA);
    assert_eq!(llamas.len(), 2);
    // A pig beside the trader hurts it: neither a zombie nor an illager, which the llama
    // would take as its target anyway.
    let pig = EntityId(0x7401);
    let kind = entity_types::PIG as u16;
    let mut own = Random::of_entity(seed, pig, 0);
    let (uuid, mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    let at = [
        trader.position[0] + 1.0,
        trader.position[1],
        trader.position[2],
    ];
    let mut record = crate::mobs::making::fresh_record(pig, kind, uuid, at, 0.0, &mob);
    record.on_ground = true;
    let chunk = record.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    region.entity_chunks.insert(pig, chunk);
    let source = DamageSource::by(
        DamageType::named("minecraft:mob_attack").expect("a damage type"),
        Causer {
            key: pig,
            kind,
            position: at,
            player: None,
            living: true,
        },
    );
    region.struck_between_ticks(trader.key, &mut TickOutput::default(), |struck| {
        hurt::hurt_server(struck, &source, 1.0)
    });
    let mut defended = false;
    for _ in 0..40 {
        idle(&mut region);
        defended = llamas.iter().any(|llama| {
            Mob::decode(&record_of(&region, llama.key).data)
                .expect("a mob")
                .body
                .combat
                .target
                == Some(pig)
        });
        if defended {
            break;
        }
    }
    assert!(defended, "a llama took the pig as its target");
}
