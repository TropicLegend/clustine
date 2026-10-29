//! Tests of a raid's rules in a region (part R1; `docs/groundwork/n10-plan.md`, section
//! 9.7; V 7.2 with D10's part C): easy's waves and their members with the raid's bonus
//! draws replayed, the bonus wave only from an omen of 2, the countdown and its progress,
//! a cleared wave's countdown, victory and Hero of the Village, a member another region
//! holds, the 48 000-tick limit, and a raider near a raid joining it.

use clustine_data::entity_types;
use clustine_world::raid::{Raid, RaidBarName, RaidStatus};
use clustine_world::{
    Effected, EntityChange, EntityId, Float, KeptBox, KeptKind, KeptStructure, RaidNews,
};

use super::*;
use crate::api::RaidSeen;
use crate::mobs::Mob;
use crate::mobs::ai::raider_goals::RaiderNews;
use crate::mobs::body::effects::Effect;
use crate::mobs::body::hurt::{self, Causer, DamageSource, DamageType};
use crate::mobs::types::illagers;
use crate::physics::Vec3 as At;

/// Where the raids of these tests are: in the home chunk, on the floor, in the village.
const CENTRE: BlockPos = BlockPos::new(8, -60, 8);

/// The bell of the village, its meeting point taken by a villager: what makes it one.
const BELL: BlockPos = BlockPos::new(8, -60, 4);

/// A village's bell at `pos` in `chunk`, its meeting point taken.
fn claim_bell(chunk: &mut Chunk, pos: BlockPos) {
    let (x, z) = pos.in_chunk();
    chunk.set(x, pos.y, z, clustine_data::blocks::BELL);
    assert!(chunk.acquire_ticket(clustine_world::InChunk::new(x, pos.y, z)));
}

/// The village taken away: its pieces and its bell.
fn unvillage(region: &mut Region) {
    let chunk = region.chunks.get_mut(&ChunkPos::new(0, 0)).unwrap();
    chunk.set_structures(Vec::new());
    let (x, z) = BELL.in_chunk();
    chunk.set(x, BELL.y, z, clustine_data::blocks::AIR);
}

/// A region holding the chunks x and z −3 to 3 (those within 2 tick), a stone floor
/// under y = −60 and a village over the home chunk (its pieces and a claimed bell),
/// which the player 1 joined.
fn village() -> Region {
    let reach = 3;
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let around: Vec<ChunkPos> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let loaded = around
        .iter()
        .map(|position| {
            let mut chunk = floor();
            if *position == ChunkPos::new(0, 0) {
                let piece = KeptBox {
                    min: BlockPos::new(0, -61, 0),
                    max: BlockPos::new(15, -50, 15),
                };
                chunk.set_structures(vec![KeptStructure {
                    kind: KeptKind::VillagePlains,
                    source: ChunkPos::new(0, 0),
                    bounds: piece,
                    pieces: vec![piece],
                }]);
                claim_bell(&mut chunk, BELL);
            }
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: loaded,
        ..TickInputs::default()
    });
    // Until the light has reached every chunk that can tick: those within one of the
    // edge do not.
    let edge = ChunkPos::new(reach - 1, reach - 1);
    for _ in 0..100 {
        if region.ticks_chunk(edge) {
            break;
        }
        idle(&mut region);
    }
    assert!(region.ticks_chunk(edge), "the chunks but the edge's tick");
    assert!(region.close_to_village(CENTRE, 1));
    region
}

/// The village with a raid at its centre of the omen `omen`.
fn village_with_raid(omen: i32) -> (Region, EntityId) {
    let mut region = village();
    let mut output = TickOutput::default();
    region.create_or_extend_raid(CENTRE, omen, &mut output);
    let id = region.own_raids()[0].1.id;
    (region, id)
}

fn raid(region: &Region, id: EntityId) -> Raid {
    region.find_raid(id).expect("the raid is kept").1.clone()
}

/// One tick of the region's raids.
fn tick_raids(region: &mut Region) -> TickOutput {
    let mut output = TickOutput::default();
    region.tick_raids(&mut output);
    output
}

/// The raid `id`'s raids' ticks until its next wave comes: the raid as the tick that
/// brought it began.
fn until_next_wave(region: &mut Region, id: EntityId) -> Raid {
    let groups = raid(region, id).groups_spawned;
    for _ in 0..400 {
        let before = raid(region, id);
        tick_raids(region);
        if raid(region, id).groups_spawned > groups {
            return before;
        }
    }
    panic!("no wave came");
}

/// Easy's bonus of vindicators and pillagers (D10 C.3): `nextInt(2)`, then `nextInt(n +
/// 1)` where it is above 0.
fn easy_bonus(stream: &mut Random) -> i32 {
    let n = stream.next_int(2) as i32;
    if n > 0 {
        stream.next_int((n + 1) as u32) as i32
    } else {
        0
    }
}

/// How many of the raid's members of the wave `wave` are of the type `kind`.
fn of_kind(region: &Region, raid: &Raid, wave: i32, kind: i32) -> i32 {
    raid.members
        .iter()
        .filter(|member| member.wave == wave)
        .filter(|member| {
            region
                .find_entity(member.key)
                .is_some_and(|(_, entity)| i32::from(entity.kind) == kind)
        })
        .count() as i32
}

/// Every member of the raid dies, by the hand of `killer` where it names one.
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

/// The waves of an easy raid of the omen `omen` until it has no more, each checked
/// against easy's table with the raid's bonus draws replayed (D10 C §1.3), each wave's
/// raiders killed by `killer`; how many waves came.
fn easy_waves(region: &mut Region, id: EntityId, killer: Option<uuid::Uuid>) -> i32 {
    // Easy's defaults by wave, the bonus wave the last wave's (D10 C.4).
    let vindicators = [0, 2, 0, 0];
    let pillagers = [4, 3, 3, 3];
    let ravagers = [0, 0, 1, 1];
    let mut waves = 0;
    loop {
        let kept = raid(region, id);
        let more = if kept.omen_level > 1 {
            kept.groups_spawned <= kept.group_count
        } else {
            kept.groups_spawned < kept.group_count
        };
        if !more {
            return waves;
        }
        let before = until_next_wave(region, id);
        let wave = before.groups_spawned + 1;
        // The place was found in the countdown, so the group's draws are the first of
        // the tick's stream.
        assert!(
            before.spawn_pos.is_some(),
            "wave {wave}'s place was found ahead"
        );
        let mut stream = Random::of_raid(region.config.seed, id, before.ticks_active + 1);
        let vindicator_bonus = easy_bonus(&mut stream);
        let pillager_bonus = easy_bonus(&mut stream);
        let after = raid(region, id);
        let index = (wave - 1) as usize;
        assert_eq!(
            of_kind(region, &after, wave, entity_types::VINDICATOR),
            vindicators[index] + vindicator_bonus,
            "wave {wave}'s vindicators"
        );
        assert_eq!(
            of_kind(region, &after, wave, entity_types::PILLAGER),
            pillagers[index] + pillager_bonus,
            "wave {wave}'s pillagers"
        );
        assert_eq!(
            of_kind(region, &after, wave, entity_types::RAVAGER),
            ravagers[index],
            "wave {wave}'s ravager"
        );
        for kind in [entity_types::EVOKER, entity_types::WITCH] {
            assert_eq!(of_kind(region, &after, wave, kind), 0, "never on easy");
        }
        assert_eq!(after.members.len() as i32, {
            vindicators[index]
                + vindicator_bonus
                + pillagers[index]
                + pillager_bonus
                + ravagers[index]
        });
        // The wave's first that can lead wears the banner and leads it.
        let leader = after
            .leaders
            .iter()
            .find(|(of, _)| *of == wave)
            .map(|(_, leader)| *leader)
            .expect("the wave has a leader");
        let (_, record) = region.find_entity(leader).expect("the leader is here");
        let mob = Mob::decode(&record.data).unwrap();
        assert!(illagers::state_of(&mob).patrol_leader);
        assert_eq!(
            mob.body.equipment.slots[usize::from(crate::living::slots::HEAD)],
            Some(crate::mobs::ai::patrol::ominous_banner())
        );
        assert_ne!(
            i32::from(record.kind),
            entity_types::RAVAGER,
            "a ravager cannot lead"
        );
        // Every raider is of the raid and the wave, its health counted.
        let total: f32 = after.members.iter().map(|member| member.health.0).sum();
        assert_eq!(after.total_health, Float(total));
        for member in &after.members {
            let (_, record) = region.find_entity(member.key).expect("made here");
            let shared = illagers::state_of(&Mob::decode(&record.data).unwrap());
            assert_eq!(shared.raid, Some(id));
            assert_eq!(shared.wave, member.wave);
            assert!(shared.can_join_raid);
        }
        assert_eq!(after.spawn_pos, None, "the place is forgotten");
        assert!(after.started);
        waves += 1;
        kill_all(region, id, killer);
    }
}

#[test]
fn an_easy_raid_of_omen_1_brings_three_waves_of_the_table_and_no_bonus_wave() {
    let (mut region, id) = village_with_raid(1);
    assert_eq!(easy_waves(&mut region, id, None), 3);
    for _ in 0..60 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    assert_eq!(kept.groups_spawned, 3, "no bonus wave below an omen of 2");
    assert_eq!(kept.status, RaidStatus::Victory);
}

#[test]
fn an_omen_of_2_brings_the_bonus_wave_of_the_last_wave_s_defaults() {
    let (mut region, id) = village_with_raid(2);
    assert_eq!(easy_waves(&mut region, id, None), 4);
    assert_eq!(raid(&region, id).groups_spawned, 4);
}

#[test]
fn the_countdown_fills_the_bar_and_brings_the_first_wave_at_its_end() {
    let (mut region, id) = village_with_raid(1);
    tick_raids(&mut region);
    let kept = raid(&region, id);
    assert_eq!(kept.cooldown_ticks, 299);
    assert_eq!(kept.bar_progress, Float(1.0 / 300.0));
    assert!(kept.bar_shown);
    assert_eq!(kept.ticks_active, 1);
    for _ in 1..150 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    assert_eq!(kept.cooldown_ticks, 150);
    assert_eq!(kept.bar_progress, Float(0.5));
    assert_eq!(kept.groups_spawned, 0);
    for _ in 150..300 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    assert_eq!(kept.cooldown_ticks, 0);
    assert_eq!(
        kept.groups_spawned, 1,
        "the wave comes as the countdown ends"
    );
    assert_eq!(kept.bar_progress, Float(1.0), "the wave's health whole");
}

#[test]
fn a_cleared_wave_starts_the_countdown_again_and_the_tick_ends_there() {
    let (mut region, id) = village_with_raid(1);
    until_next_wave(&mut region, id);
    let wave = raid(&region, id);
    // The names of one and two left are shown every 20 active ticks.
    kill_all(&mut region, id, None);
    tick_raids(&mut region);
    let kept = raid(&region, id);
    assert_eq!(kept.cooldown_ticks, 300);
    assert_eq!(kept.bar_name, RaidBarName::Raid);
    assert_eq!(kept.bar_progress, Float(0.0), "nobody left of the wave");
    assert_eq!(kept.ticks_active, wave.ticks_active + 1);
    tick_raids(&mut region);
    let kept = raid(&region, id);
    assert_eq!(kept.cooldown_ticks, 299);
    assert_eq!(kept.bar_progress, Float(1.0 / 300.0));
}

#[test]
fn victory_comes_forty_ticks_after_the_last_wave_and_gives_the_heroes_hero_of_the_village() {
    let (mut region, id) = village_with_raid(1);
    let hero = player(1).0;
    easy_waves(&mut region, id, Some(hero));
    assert_eq!(raid(&region, id).heroes, [hero]);
    let mut ticks = 0;
    let mut given = Vec::new();
    while raid(&region, id).status == RaidStatus::Ongoing {
        let output = tick_raids(&mut region);
        given.extend(output.player_events);
        ticks += 1;
        assert!(ticks < 60, "no victory");
    }
    // The last wave's own tick counted one already: its count of the living was taken
    // before the wave came (the game's `tick` keeps it in a local).
    assert_eq!(
        ticks, 40,
        "forty counted with the wave's own, then the victory"
    );
    assert_eq!(raid(&region, id).status, RaidStatus::Victory);
    let effects = &region.players[&player(1)].effects;
    assert_eq!(effects.amplifier(Effect::HERO_OF_THE_VILLAGE), Some(0));
    // It celebrates: the bar says so every twenty ticks, then the raid stops.
    for _ in 0..20 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    assert_eq!(kept.bar_name, RaidBarName::Victory);
    assert_eq!(kept.bar_progress, Float(0.0));
    for _ in 20..600 {
        tick_raids(&mut region);
    }
    assert!(region.own_raids().is_empty(), "stopped after celebrating");
}

#[test]
fn a_member_another_region_holds_is_alive_until_its_death_is_told() {
    let (mut region, id) = village_with_raid(1);
    until_next_wave(&mut region, id);
    let mut output = TickOutput::default();
    // A raider of another region joins the wave across a pinned line.
    let away = EntityId(0x9_0000_0001);
    region.raid_news(
        RaidNews::Joined {
            raid: id,
            member: away,
            wave: 1,
            health: Float(24.0),
            from: ChunkPos::new(30, 0),
        },
        &mut output,
    );
    let ours: Vec<EntityId> = raid(&region, id)
        .members
        .iter()
        .map(|member| member.key)
        .filter(|key| *key != away)
        .collect();
    for member in ours {
        region.raid_news(
            RaidNews::Died {
                raid: id,
                member,
                killer: None,
            },
            &mut output,
        );
    }
    for _ in 0..700 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    assert_eq!(kept.groups_spawned, 1, "the next wave waits for it");
    assert!(kept.members.iter().any(|member| member.key == away));
    let hero = player(1).0;
    region.raid_news(
        RaidNews::Died {
            raid: id,
            member: away,
            killer: Some(hero),
        },
        &mut output,
    );
    tick_raids(&mut region);
    let kept = raid(&region, id);
    assert!(kept.members.is_empty());
    assert_eq!(kept.heroes, [hero]);
    assert_eq!(kept.cooldown_ticks, 300, "the countdown begins");
}

#[test]
fn a_raider_of_another_region_s_raid_tells_its_death_to_the_raid_s_chunk() {
    // A region pinned west of chunk x 5; the raid's record is kept far east of it.
    let mut region = joined_in(
        ChunkArea {
            min_x: None,
            max_x: Some(5),
        },
        &[1],
    );
    region.chunks.insert(ChunkPos::new(0, 0), floor());
    let far = BlockPos::new(500, -60, 8);
    let id = EntityId(0x8_0000_0002);
    region.take_raid_table(&[RaidSeen {
        id,
        chunk: far.chunk(),
        centre: far,
        active: true,
        status: RaidStatus::Ongoing,
        groups_spawned: 1,
        between_waves: false,
        omen: 1,
        bar: uuid::Uuid::from_u128(7),
        shown: true,
        progress: 1.0,
        name: RaidBarName::Raid,
    }]);
    let key = EntityId(0x7000);
    let kind = entity_types::PILLAGER as u16;
    let mut own = Random::of_entity(region.config.seed, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    illagers::update(&mut mob, |raider| {
        raider.raid = Some(id);
        raider.wave = 1;
        raider.can_join_raid = true;
    });
    let record = crate::mobs::making::fresh_record(key, kind, uuid, [4.5, -60.0, 4.5], 0.0, &mob);
    region
        .chunks
        .get_mut(&ChunkPos::new(0, 0))
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    region.entity_chunks.insert(key, ChunkPos::new(0, 0));
    let hero = player(1).0;
    let source = DamageSource::by(
        DamageType::named("minecraft:player_attack").unwrap(),
        Causer {
            key: EntityId(1),
            kind: crate::region::mob_bodies::player_kind(),
            position: [4.5, -60.0, 6.5],
            player: Some(hero.as_u128()),
            living: true,
        },
    );
    let mut output = TickOutput::default();
    region.struck_between_ticks(key, &mut output, |struck| hurt::die(struck, &source));
    assert!(
        region
            .made
            .iter()
            .any(|effect| effect.at.chunk() == far.chunk()
                && effect.what
                    == Effected::Raid(RaidNews::Died {
                        raid: id,
                        member: key,
                        killer: Some(hero),
                    })),
        "its death goes to the raid's record"
    );
}

#[test]
fn a_raid_is_stopped_and_taken_away_after_48000_active_ticks() {
    let (mut region, id) = village_with_raid(1);
    let mut output = TickOutput::default();
    region.change_raid(id, &mut output, |raid| raid.ticks_active = 47_998);
    tick_raids(&mut region);
    assert_eq!(raid(&region, id).ticks_active, 47_999);
    tick_raids(&mut region);
    assert!(region.own_raids().is_empty());
}

#[test]
fn a_raid_with_no_village_at_its_centre_stops_before_its_first_wave_and_is_lost_after() {
    let (mut region, id) = village_with_raid(1);
    unvillage(&mut region);
    tick_raids(&mut region);
    assert!(
        region.find_raid(id).is_none(),
        "stopped, no wave having come"
    );
    let (mut region, id_lost) = village_with_raid(1);
    until_next_wave(&mut region, id_lost);
    unvillage(&mut region);
    tick_raids(&mut region);
    assert_eq!(raid(&region, id_lost).status, RaidStatus::Loss);
}

#[test]
fn a_raider_near_a_raid_joins_its_current_wave_on_a_twentieth_tick() {
    let (mut region, id) = village_with_raid(1);
    let key = EntityId(0x7000);
    let kind = entity_types::PILLAGER as u16;
    let mut own = Random::of_entity(region.config.seed, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    illagers::update(&mut mob, |raider| raider.can_join_raid = true);
    let health = mob.body.vitals.health;
    let record = crate::mobs::making::fresh_record(key, kind, uuid, [6.5, -60.0, 6.5], 0.0, &mob);
    region
        .chunks
        .get_mut(&ChunkPos::new(0, 0))
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    region.entity_chunks.insert(key, ChunkPos::new(0, 0));
    let mut joined = false;
    for tick in 1..=40 {
        region.clock.game_time = tick;
        idle(&mut region);
        if raid(&region, id)
            .members
            .iter()
            .any(|member| member.key == key)
        {
            joined = true;
            break;
        }
    }
    assert!(joined, "it joined");
    let kept = raid(&region, id);
    assert_eq!(kept.members.len(), 1);
    assert_eq!(
        kept.members[0].wave, 0,
        "the current wave, none having come"
    );
    assert_eq!(kept.members[0].health, Float(health));
    assert_eq!(kept.total_health, Float(health));
    let (_, record) = region.find_entity(key).expect("still here");
    let shared = illagers::state_of(&Mob::decode(&record.data).unwrap());
    assert_eq!(shared.raid, Some(id));
    assert_eq!(shared.wave, 0);
}

#[test]
fn a_raider_recruited_by_another_joins_the_raid_with_its_health() {
    let (mut region, id) = village_with_raid(1);
    until_next_wave(&mut region, id);
    let key = EntityId(0x7000);
    let kind = entity_types::VINDICATOR as u16;
    let mut own = Random::of_entity(region.config.seed, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    illagers::update(&mut mob, |raider| raider.can_join_raid = true);
    let record = crate::mobs::making::fresh_record(key, kind, uuid, [6.5, -60.0, 6.5], 0.0, &mob);
    region
        .chunks
        .get_mut(&ChunkPos::new(0, 0))
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    region.entity_chunks.insert(key, ChunkPos::new(0, 0));
    let before = raid(&region, id);
    let recruiter = before.members[0].key;
    let mut output = TickOutput::default();
    region.raider_news(
        recruiter,
        Some(key),
        RaiderNews::Recruit { raid: id, wave: 1 },
        &mut output,
    );
    let kept = raid(&region, id);
    assert_eq!(kept.members.len(), before.members.len() + 1);
    assert_eq!(
        kept.total_health,
        Float(before.total_health.0 + mob.body.vitals.health)
    );
    let (_, record) = region.find_entity(key).unwrap();
    assert_eq!(
        illagers::state_of(&Mob::decode(&record.data).unwrap()).raid,
        Some(id)
    );
}

#[test]
fn the_horn_sounds_once_as_a_wave_comes_to_the_player_near_its_place() {
    let (mut region, id) = village_with_raid(1);
    let mut horns = 0;
    while raid(&region, id).groups_spawned == 0 {
        let output = tick_raids(&mut region);
        horns += output
            .events
            .iter()
            .filter(|event| {
                matches!(event, RegionEvent::Sound { sound, volume, .. }
                    if *sound == clustine_data::sounds::EVENT_RAID_HORN && *volume == 64.0)
            })
            .count();
    }
    assert_eq!(horns, 1);
}

/// A pillager of the region at `at`, which may join raids, of the raid `raid` where it
/// names one.
fn lay_pillager(region: &mut Region, key: EntityId, at: At, raid: Option<EntityId>) {
    let kind = entity_types::PILLAGER as u16;
    let mut own = Random::of_entity(region.config.seed, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    illagers::update(&mut mob, |raider| {
        raider.can_join_raid = true;
        raider.raid = raid;
    });
    let record = crate::mobs::making::fresh_record(key, kind, uuid, at, 0.0, &mob);
    let chunk = record.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    region.entity_chunks.insert(key, chunk);
}

#[test]
fn a_raider_of_a_raid_outside_the_village_walks_to_its_centre_and_celebrates_its_loss() {
    let mut region = village();
    // The village's piece in the chunk (−2, −2) instead: the raid at its edge, and the
    // land's far corner beyond the stand-in's two chunks, no village.
    let piece = KeptBox {
        min: BlockPos::new(-32, -61, -32),
        max: BlockPos::new(-32, -50, -32),
    };
    unvillage(&mut region);
    region
        .chunks
        .get_mut(&ChunkPos::new(-2, -2))
        .unwrap()
        .set_structures(vec![KeptStructure {
            kind: KeptKind::VillagePlains,
            source: ChunkPos::new(-2, -2),
            bounds: piece,
            pieces: vec![piece],
        }]);
    claim_bell(
        region.chunks.get_mut(&ChunkPos::new(-2, -2)).unwrap(),
        BlockPos::new(-32, -60, -32),
    );
    let centre = BlockPos::new(-8, -60, -8);
    assert!(region.close_to_village(centre, 1));
    let mut output = TickOutput::default();
    region.create_or_extend_raid(centre, 1, &mut output);
    let id = region.own_raids()[0].1.id;
    let key = EntityId(0x7000);
    let start = [40.5, -60.0, 40.5];
    assert!(!region.close_to_village(crate::physics::containing(start), 1));
    lay_pillager(&mut region, key, start, Some(id));
    region.raider_news(
        key,
        None,
        RaiderNews::Joined {
            raid: id,
            wave: 0,
            health: 24.0,
        },
        &mut output,
    );
    for _ in 0..60 {
        idle(&mut region);
    }
    let (_, record) = region.find_entity(key).expect("still here");
    assert!(
        record.position[0] + record.position[2] < start[0] + start[2] - 3.0,
        "it went towards the centre"
    );
    region.change_raid(id, &mut output, |raid| raid.status = RaidStatus::Loss);
    let mut celebrated = false;
    for _ in 0..10 {
        idle(&mut region);
        let (_, record) = region.find_entity(key).expect("still here");
        celebrated |= illagers::state_of(&Mob::decode(&record.data).unwrap()).celebrating;
    }
    assert!(celebrated, "it celebrates the raid's loss");
}

/// A pillager of the region at `at`, a member of the raid `id`'s wave `wave` as its
/// `aiStep` told the raid.
fn lay_member(region: &mut Region, key: EntityId, at: At, id: EntityId, wave: i32) {
    lay_pillager(region, key, at, Some(id));
    let chunk = region.entity_chunks[&key];
    let mut record = region.find_entity(key).expect("laid").1.clone();
    let mut mob = Mob::decode(&record.data).expect("a mob");
    illagers::update(&mut mob, |raider| raider.wave = wave);
    let health = mob.body.vitals.health;
    record.data = mob.encode();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    region.raider_news(
        key,
        None,
        RaiderNews::Joined {
            raid: id,
            wave,
            health,
        },
        &mut TickOutput::default(),
    );
}

/// Where the last member of the tests below stands: in the land's corner chunk, 68 from
/// the centre.
const CORNER: At = [-40.5, -60.0, -40.5];

/// The land's corner chunk, where [`CORNER`] is.
const CORNER_CHUNK: ChunkPos = ChunkPos::new(-3, -3);

/// The raid `id` after its first wave, of which only the member `key` at [`CORNER`] is
/// left, waiting for it.
fn first_wave_left_to(key: EntityId) -> (Region, EntityId) {
    let (mut region, id) = village_with_raid(1);
    until_next_wave(&mut region, id);
    lay_member(&mut region, key, CORNER, id, 1);
    let mut output = TickOutput::default();
    for member in raid(&region, id).members {
        if member.key != key {
            region.raid_news(
                RaidNews::Died {
                    raid: id,
                    member: member.key,
                    killer: None,
                },
                &mut output,
            );
        }
    }
    for _ in 0..40 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    let keys: Vec<EntityId> = kept.members.iter().map(|member| member.key).collect();
    assert_eq!(keys, [key]);
    assert_eq!(kept.cooldown_ticks, 0, "no countdown while it is alive");
    (region, id)
}

#[test]
fn a_member_let_go_with_its_chunk_leaves_the_raid_and_the_next_wave_s_countdown_begins() {
    // The game's `updateRaiders` removes a raider unloaded with its chunk (`isRemoved()`);
    // a raid that kept counting it waited for it for good, between its waves.
    let key = EntityId(0x7000);
    let (mut region, id) = first_wave_left_to(key);
    let kept = raid(&region, id);
    let total = kept.total_health.0;
    let health = kept.members[0].health.0;
    region.tick(&tickets(vec![], vec![CORNER_CHUNK]));
    assert!(
        !region.chunks.contains_key(&CORNER_CHUNK),
        "the chunk is let go"
    );
    let kept = raid(&region, id);
    assert!(kept.members.is_empty(), "it left the raid");
    assert_eq!(
        kept.total_health,
        Float(total - health),
        "`removeFromRaid(…, true)`"
    );
    // The countdown begins in the raids' next tick, which may be the region's own.
    if kept.cooldown_ticks == 0 {
        tick_raids(&mut region);
    }
    let before = raid(&region, id).cooldown_ticks;
    assert!(before > 0, "the countdown began");
    tick_raids(&mut region);
    assert_eq!(raid(&region, id).cooldown_ticks, before - 1);
}

#[test]
fn a_member_let_go_rejoins_its_wave_still_leading_when_its_chunk_is_loaded_again() {
    let key = EntityId(0x7000);
    let (mut region, id) = first_wave_left_to(key);
    // It leads its wave, as the banner's bearer.
    let mut record = region.find_entity(key).expect("here").1.clone();
    let mut mob = Mob::decode(&record.data).expect("a mob");
    illagers::update(&mut mob, |raider| raider.patrol_leader = true);
    record.data = mob.encode();
    region
        .chunks
        .get_mut(&CORNER_CHUNK)
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    let saved = region.chunks[&CORNER_CHUNK].clone();
    region.tick(&tickets(vec![], vec![CORNER_CHUNK]));
    let left = raid(&region, id);
    assert!(left.members.is_empty());
    region.tick(&tickets(vec![CORNER_CHUNK], vec![]));
    region.tick(&TickInputs {
        chunks_loaded: vec![(CORNER_CHUNK, saved)],
        ..TickInputs::default()
    });
    assert!(
        region.chunks.contains_key(&CORNER_CHUNK),
        "the chunk is back"
    );
    let kept = raid(&region, id);
    let members: Vec<(EntityId, i32)> = kept
        .members
        .iter()
        .map(|member| (member.key, member.wave))
        .collect();
    assert_eq!(
        members,
        [(key, 1)],
        "`addWaveMob(level, wave, raider, false)`"
    );
    assert_eq!(kept.total_health, left.total_health, "its total unchanged");
    assert_eq!(kept.leaders, [(1, key)], "`setLeader(wave, raider)`");
}

#[test]
fn a_raider_of_another_region_s_raid_rejoins_it_when_loaded_and_leaves_it_when_let_go() {
    // A region pinned west of chunk x 5; the raid's record is kept far east of it.
    let mut region = joined_in(
        ChunkArea {
            min_x: None,
            max_x: Some(5),
        },
        &[1],
    );
    let far = BlockPos::new(500, -60, 8);
    let id = EntityId(0x8_0000_0002);
    region.take_raid_table(&[RaidSeen {
        id,
        chunk: far.chunk(),
        centre: far,
        active: true,
        status: RaidStatus::Ongoing,
        groups_spawned: 1,
        between_waves: false,
        omen: 1,
        bar: uuid::Uuid::from_u128(7),
        shown: true,
        progress: 1.0,
        name: RaidBarName::Raid,
    }]);
    let key = EntityId(0x7000);
    let kind = entity_types::PILLAGER as u16;
    let mut own = Random::of_entity(region.config.seed, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    illagers::update(&mut mob, |raider| {
        raider.raid = Some(id);
        raider.wave = 1;
        raider.can_join_raid = true;
    });
    let health = mob.body.vitals.health;
    let record = crate::mobs::making::fresh_record(key, kind, uuid, [20.5, -60.0, 4.5], 0.0, &mob);
    let place = ChunkPos::new(1, 0);
    let mut chunk = floor();
    chunk.apply_entity(&EntityChange::Whole(record));
    region.tick(&tickets(vec![place], vec![]));
    let output = region.tick(&TickInputs {
        chunks_loaded: vec![(place, chunk)],
        ..TickInputs::default()
    });
    assert!(region.chunks.contains_key(&place), "the chunk is loaded");
    let told = |output: &TickOutput, news: RaidNews| {
        output
            .effects
            .iter()
            .any(|effect| effect.at.chunk() == far.chunk() && effect.what == Effected::Raid(news))
    };
    let joined = RaidNews::Joined {
        raid: id,
        member: key,
        wave: 1,
        health: Float(health),
        from: place,
    };
    assert!(told(&output, joined), "it rejoins the raid's record");
    let output = region.tick(&tickets(vec![], vec![place]));
    assert!(!region.chunks.contains_key(&place), "the chunk is let go");
    let left = RaidNews::Left {
        raid: id,
        member: key,
    };
    assert!(told(&output, left), "it leaves the raid's record");
}

// Tests written from the rules of a raider let go with its chunk and taken up again
// (`Raid.updateRaiders` removing a raider that `isRemoved()`,
// `Raider.readAdditionalSaveData` rejoining its wave), by someone who did not write them.

/// Where a second member of the tests below stands: in a chunk at the land's north edge,
/// 58 from the centre.
const OTHER_CORNER: At = [40.5, -60.0, -40.5];

/// The chunk of [`OTHER_CORNER`].
const OTHER_CORNER_CHUNK: ChunkPos = ChunkPos::new(2, -3);

/// The raid `id` after its first wave, of which only the members `standing` (each a key
/// and where it stands) are left, all of the wave 1, waiting for them.
fn first_wave_left_to_all(standing: &[(EntityId, At)]) -> (Region, EntityId) {
    let (mut region, id) = village_with_raid(1);
    until_next_wave(&mut region, id);
    for (key, at) in standing {
        lay_member(&mut region, *key, *at, id, 1);
    }
    let mut keys: Vec<EntityId> = standing.iter().map(|(key, _)| *key).collect();
    keys.sort();
    let mut output = TickOutput::default();
    for member in raid(&region, id).members {
        if !keys.contains(&member.key) {
            region.raid_news(
                RaidNews::Died {
                    raid: id,
                    member: member.key,
                    killer: None,
                },
                &mut output,
            );
        }
    }
    for _ in 0..40 {
        tick_raids(&mut region);
    }
    let kept = raid(&region, id);
    let mut left: Vec<EntityId> = kept.members.iter().map(|member| member.key).collect();
    left.sort();
    assert_eq!(left, keys);
    assert_eq!(kept.cooldown_ticks, 0, "no countdown while they are alive");
    (region, id)
}

/// The raid's members as keys and waves.
fn members_of(raid: &Raid) -> Vec<(EntityId, i32)> {
    raid.members
        .iter()
        .map(|member| (member.key, member.wave))
        .collect()
}

/// The health the raid counts for its member `key`.
fn health_of(raid: &Raid, key: EntityId) -> f32 {
    raid.members
        .iter()
        .find(|member| member.key == key)
        .expect("a member")
        .health
        .0
}

/// The chunk `position` asked for and loaded as `saved`.
fn load(region: &mut Region, position: ChunkPos, saved: &Chunk) {
    region.tick(&tickets(vec![position], vec![]));
    region.tick(&TickInputs {
        chunks_loaded: vec![(position, saved.clone())],
        ..TickInputs::default()
    });
    assert!(region.chunks.contains_key(&position), "the chunk is back");
}

#[test]
fn a_member_let_go_while_another_of_its_wave_lives_leaves_and_the_countdown_waits_for_the_other() {
    let gone = EntityId(0x7000);
    let staying = EntityId(0x7001);
    let (mut region, id) = first_wave_left_to_all(&[(gone, CORNER), (staying, OTHER_CORNER)]);
    let before = raid(&region, id);
    let health = health_of(&before, gone);
    region.tick(&tickets(vec![], vec![CORNER_CHUNK]));
    assert!(!region.chunks.contains_key(&CORNER_CHUNK));
    assert!(region.chunks.contains_key(&OTHER_CORNER_CHUNK));
    let kept = raid(&region, id);
    assert_eq!(
        members_of(&kept),
        [(staying, 1)],
        "only the one let go left"
    );
    assert_eq!(kept.total_health, Float(before.total_health.0 - health));
    for _ in 0..40 {
        tick_raids(&mut region);
        assert_eq!(
            raid(&region, id).cooldown_ticks,
            0,
            "no countdown while one of the wave lives"
        );
    }
    region.raid_news(
        RaidNews::Died {
            raid: id,
            member: staying,
            killer: None,
        },
        &mut TickOutput::default(),
    );
    tick_raids(&mut region);
    if raid(&region, id).cooldown_ticks == 0 {
        tick_raids(&mut region);
    }
    assert!(
        raid(&region, id).cooldown_ticks > 0,
        "the countdown begins once the last is dead"
    );
}

#[test]
fn a_member_whose_chunk_goes_in_the_same_tick_as_its_raid_s_record_leaves_the_raid_first() {
    let key = EntityId(0x7000);
    let (mut region, id) = first_wave_left_to(key);
    // The record kept in a chunk other than the home chunk, which never goes: as where a
    // centre moved (section 9.1). The centre stays in the home chunk, so the raid stays
    // active.
    let record = ChunkPos::new(1, 1);
    let (home, kept) = region.find_raid(id).expect("kept");
    let kept = kept.clone();
    region.chunks.get_mut(&home).unwrap().set_raids(Vec::new());
    region
        .chunks
        .get_mut(&record)
        .unwrap()
        .set_raids(vec![kept]);
    assert_eq!(region.find_raid(id).map(|(chunk, _)| chunk), Some(record));
    let output = region.tick(&tickets(vec![], vec![CORNER_CHUNK, record]));
    assert!(!region.chunks.contains_key(&CORNER_CHUNK));
    assert!(!region.chunks.contains_key(&record));
    assert!(
        output.raids_changed.contains(&record),
        "the record heard its member leave before it went"
    );
    let told_left = output.effects.iter().any(|effect| {
        effect.what
            == Effected::Raid(RaidNews::Left {
                raid: id,
                member: key,
            })
    });
    assert!(
        !told_left,
        "a raider of the region's own raid leaves at once, not by word"
    );
}

#[test]
fn a_member_whose_ticket_is_removed_and_taken_again_in_one_tick_stays_in_the_raid() {
    let key = EntityId(0x7000);
    let (mut region, id) = first_wave_left_to(key);
    let before = raid(&region, id);
    region.tick(&tickets(vec![CORNER_CHUNK], vec![CORNER_CHUNK]));
    assert!(region.chunks.contains_key(&CORNER_CHUNK), "never let go");
    let kept = raid(&region, id);
    assert_eq!(members_of(&kept), [(key, 1)]);
    assert_eq!(kept.total_health, before.total_health);
    for _ in 0..5 {
        tick_raids(&mut region);
    }
    assert_eq!(raid(&region, id).cooldown_ticks, 0, "no countdown");
}

#[test]
fn a_member_let_go_and_loaded_again_takes_off_its_health_once_and_does_not_give_it_back() {
    let key = EntityId(0x7000);
    let (mut region, id) = first_wave_left_to(key);
    let before = raid(&region, id);
    let health = health_of(&before, key);
    let saved = region.chunks[&CORNER_CHUNK].clone();
    region.tick(&tickets(vec![], vec![CORNER_CHUNK]));
    assert!(!region.chunks.contains_key(&CORNER_CHUNK), "let go");
    load(&mut region, CORNER_CHUNK, &saved);
    let kept = raid(&region, id);
    assert_eq!(members_of(&kept), [(key, 1)], "it rejoined its wave");
    assert_eq!(
        kept.total_health,
        Float(before.total_health.0 - health),
        "less the leaver's health, not grown back on its rejoin"
    );
}

#[test]
fn a_leading_member_let_go_and_loaded_twice_leaves_and_rejoins_once_each_time() {
    let key = EntityId(0x7000);
    let (mut region, id) = first_wave_left_to(key);
    let mut record = region.find_entity(key).expect("here").1.clone();
    let mut mob = Mob::decode(&record.data).expect("a mob");
    illagers::update(&mut mob, |raider| raider.patrol_leader = true);
    record.data = mob.encode();
    region
        .chunks
        .get_mut(&CORNER_CHUNK)
        .unwrap()
        .apply_entity(&EntityChange::Whole(record));
    let saved = region.chunks[&CORNER_CHUNK].clone();
    let before = raid(&region, id);
    let health = health_of(&before, key);
    for time in 1..=2 {
        region.tick(&tickets(vec![], vec![CORNER_CHUNK]));
        assert!(!region.chunks.contains_key(&CORNER_CHUNK), "let go");
        let left = raid(&region, id);
        assert!(left.members.is_empty(), "it left, time {time}");
        // Each leave is a `removeFromRaid(…, true)`, which takes its health off.
        assert_eq!(
            left.total_health,
            Float(before.total_health.0 - health * time as f32),
            "its health came off once more, time {time}"
        );
        load(&mut region, CORNER_CHUNK, &saved);
        let back = raid(&region, id);
        assert_eq!(
            members_of(&back),
            [(key, 1)],
            "it rejoined once, time {time}"
        );
        assert_eq!(back.total_health, left.total_health, "not counted again");
        assert_eq!(back.leaders, [(1, key)], "it leads once, time {time}");
    }
}

#[test]
fn a_member_s_chunk_loaded_and_let_go_in_one_tick_leaves_it_out_of_the_raid() {
    let key = EntityId(0x7000);
    let (mut region, id) = first_wave_left_to(key);
    let saved = region.chunks[&CORNER_CHUNK].clone();
    region.tick(&tickets(vec![], vec![CORNER_CHUNK]));
    assert!(raid(&region, id).members.is_empty());
    region.tick(&tickets(vec![CORNER_CHUNK], vec![]));
    region.tick(&TickInputs {
        chunks_loaded: vec![(CORNER_CHUNK, saved)],
        tickets_removed: viewers(vec![CORNER_CHUNK]),
        ..TickInputs::default()
    });
    assert!(!region.chunks.contains_key(&CORNER_CHUNK), "let go");
    let kept = raid(&region, id);
    assert!(
        kept.members.iter().all(|member| member.key != key),
        "it is not counted while its chunk is gone"
    );
    assert!(kept.leaders.iter().all(|(_, leader)| *leader != key));
}

#[test]
fn a_raider_of_another_region_s_raid_loaded_and_let_go_twice_tells_one_join_and_one_leave_each_time()
 {
    let mut region = joined_in(
        ChunkArea {
            min_x: None,
            max_x: Some(5),
        },
        &[1],
    );
    let far = BlockPos::new(500, -60, 8);
    let id = EntityId(0x8_0000_0002);
    region.take_raid_table(&[RaidSeen {
        id,
        chunk: far.chunk(),
        centre: far,
        active: true,
        status: RaidStatus::Ongoing,
        groups_spawned: 1,
        between_waves: false,
        omen: 1,
        bar: uuid::Uuid::from_u128(7),
        shown: true,
        progress: 1.0,
        name: RaidBarName::Raid,
    }]);
    let key = EntityId(0x7000);
    let kind = entity_types::PILLAGER as u16;
    let mut own = Random::of_entity(region.config.seed, key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    illagers::update(&mut mob, |raider| {
        raider.raid = Some(id);
        raider.wave = 1;
        raider.can_join_raid = true;
    });
    let record = crate::mobs::making::fresh_record(key, kind, uuid, [20.5, -60.0, 4.5], 0.0, &mob);
    let place = ChunkPos::new(1, 0);
    let mut chunk = floor();
    chunk.apply_entity(&EntityChange::Whole(record));
    let joins = |output: &TickOutput| {
        output
            .effects
            .iter()
            .filter(|effect| {
                effect.at.chunk() == far.chunk()
                    && matches!(
                        effect.what,
                        Effected::Raid(RaidNews::Joined { raid, member, .. })
                            if raid == id && member == key
                    )
            })
            .count()
    };
    let left = Effected::Raid(RaidNews::Left {
        raid: id,
        member: key,
    });
    let leaves = |output: &TickOutput| {
        output
            .effects
            .iter()
            .filter(|effect| effect.at.chunk() == far.chunk() && effect.what == left)
            .count()
    };
    for time in 1..=2 {
        let asked = region.tick(&tickets(vec![place], vec![]));
        let loaded = region.tick(&TickInputs {
            chunks_loaded: vec![(place, chunk.clone())],
            ..TickInputs::default()
        });
        assert!(region.chunks.contains_key(&place), "loaded, time {time}");
        assert_eq!(joins(&asked) + joins(&loaded), 1, "one join, time {time}");
        assert_eq!(leaves(&asked) + leaves(&loaded), 0, "no leave, time {time}");
        let let_go = region.tick(&tickets(vec![], vec![place]));
        assert!(!region.chunks.contains_key(&place), "let go, time {time}");
        assert_eq!(leaves(&let_go), 1, "one leave, time {time}");
        assert_eq!(joins(&let_go), 0, "no join, time {time}");
    }
}
