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
        first_wave_spawned: true,
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
