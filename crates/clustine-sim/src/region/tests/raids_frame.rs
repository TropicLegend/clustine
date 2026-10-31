//! Tests of raids as world state in a region (`docs/groundwork/n10-plan.md`, sections 9.1
//! to 9.5): a raid made at an omen's end and growing with another, `getRaidAt`, the news
//! from other regions, and a player's bar.

use clustine_world::raid::{RaidBarName, RaidStatus};
use clustine_world::{EntityUse, Float, RaidNews, RaiderUse};

use super::*;
use crate::api::{BarChange, BarName, BarStyle, RaidSeen};
use crate::region::raids::RaidAt;

/// Where the raids of these tests are: in the home chunk, by the player who joined.
const CENTRE: BlockPos = BlockPos::new(8, -60, 8);

/// A region pinned to `area` that the player 1 joined, with the chunks x -2 to 3 and z -2
/// to 2 within it loaded.
fn loaded_in(area: ChunkArea) -> Region {
    let mut region = joined_in(area, &[1]);
    for x in -2..=3 {
        for z in -2..=2 {
            let position = ChunkPos::new(x, z);
            if area.contains(position) {
                region.chunks.insert(position, floor());
            }
        }
    }
    region
}

/// Chunks x below 1: the member of the join tests is beyond the line.
const WEST: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};

fn with_raid() -> (Region, EntityId) {
    let mut region = loaded_in(ChunkArea::EVERYWHERE);
    let mut output = TickOutput::default();
    region.create_or_extend_raid(CENTRE, 1, &mut output);
    assert_eq!(output.raids_changed, [CENTRE.chunk()]);
    let id = region.own_raids()[0].1.id;
    (region, id)
}

fn seen_far(id: u64, centre: BlockPos, active: bool) -> RaidSeen {
    RaidSeen {
        id: EntityId(id),
        chunk: centre.chunk(),
        centre,
        active,
        status: RaidStatus::Ongoing,
        groups_spawned: 0,
        between_waves: false,
        omen: 1,
        bar: uuid::Uuid::from_u128(u128::from(id)),
        shown: true,
        progress: 0.5,
        name: RaidBarName::Raid,
    }
}

#[test]
fn an_omen_s_end_makes_a_raid_at_its_centre_and_another_grows_it_to_at_most_five() {
    let (mut region, id) = with_raid();
    let (chunk, raid) = region.find_raid(id).expect("made");
    assert_eq!(chunk, CENTRE.chunk());
    assert_eq!(raid.centre, CENTRE);
    assert_eq!(raid.omen_level, 1);
    assert_eq!(raid.group_count, 3, "easy has three waves");
    assert_eq!(raid.status, RaidStatus::Ongoing);
    let bar = raid.bar;
    let mut output = TickOutput::default();
    region.create_or_extend_raid(CENTRE.offset(5, 0, 0), 3, &mut output);
    region.create_or_extend_raid(CENTRE, 4, &mut output);
    assert_eq!(region.own_raids().len(), 1, "one raid, grown");
    let (_, raid) = region.find_raid(id).unwrap();
    assert_eq!(raid.omen_level, 5);
    assert_eq!(raid.bar, bar, "its bar stays its own");
}

#[test]
fn get_raid_at_finds_the_nearest_active_raid_within_96_of_its_own_and_the_table() {
    let (mut region, id) = with_raid();
    assert!(
        matches!(region.get_raid_at(CENTRE.offset(95, 0, 0)), Some(RaidAt::Own { id: found, .. }) if found == id)
    );
    assert_eq!(region.get_raid_at(CENTRE.offset(96, 0, 0)), None);
    region.take_raid_table(&[
        seen_far(900, CENTRE.offset(150, 0, 0), true),
        seen_far(901, CENTRE.offset(100, 0, 0), false),
    ]);
    // Nearer the table's active raid than its own.
    assert!(
        matches!(region.get_raid_at(CENTRE.offset(120, 0, 0)), Some(RaidAt::Seen(raid)) if raid.id == EntityId(900))
    );
    let mut output = TickOutput::default();
    region.change_raid(id, &mut output, |raid| raid.active = false);
    assert_eq!(
        region.get_raid_at(CENTRE),
        None,
        "a paused raid answers nobody"
    );
}

#[test]
fn a_raider_joins_on_the_current_wave_and_is_refused_otherwise() {
    let mut region = loaded_in(WEST);
    let mut output = TickOutput::default();
    region.create_or_extend_raid(CENTRE, 1, &mut output);
    let id = region.own_raids()[0].1.id;
    region.change_raid(id, &mut output, |raid| raid.groups_spawned = 1);
    let member = EntityId(0x9_0000_0001);
    region.raid_news(
        RaidNews::Joined {
            raid: id,
            member,
            wave: 1,
            health: Float(24.0),
            from: ChunkPos::new(30, 0),
        },
        &mut output,
    );
    assert_eq!(region.find_raid(id).unwrap().1.members.len(), 1);
    let late = EntityId(0x9_0000_0002);
    region.raid_news(
        RaidNews::Joined {
            raid: id,
            member: late,
            wave: 0,
            health: Float(24.0),
            from: ChunkPos::new(30, 0),
        },
        &mut output,
    );
    assert_eq!(region.find_raid(id).unwrap().1.members.len(), 1);
    assert!(region.made.iter().any(|effect| matches!(
        &effect.what,
        clustine_world::Effected::OnEntity { key, by: EntityUse::Raider(RaiderUse::Rejected { raid }) }
            if *key == late && *raid == id
    )));
}

#[test]
fn a_member_s_death_by_a_player_makes_a_hero_and_one_that_leaves_is_gone() {
    let (mut region, id) = with_raid();
    let mut output = TickOutput::default();
    region.change_raid(id, &mut output, |raid| raid.groups_spawned = 1);
    for member in [1, 2] {
        region.raid_news(
            RaidNews::Joined {
                raid: id,
                member: EntityId(member),
                wave: 1,
                health: Float(24.0),
                from: ChunkPos::new(30, 0),
            },
            &mut output,
        );
    }
    // `addWaveMob(…, true)`: each joiner's health counted.
    assert_eq!(region.find_raid(id).unwrap().1.total_health, Float(48.0));
    let hero = uuid::Uuid::from_u128(42);
    region.raid_news(
        RaidNews::Died {
            raid: id,
            member: EntityId(1),
            killer: Some(hero),
        },
        &mut output,
    );
    region.raid_news(
        RaidNews::Left {
            raid: id,
            member: EntityId(2),
        },
        &mut output,
    );
    // A hurt of a member it no longer lists is passed over.
    region.raid_news(
        RaidNews::Hurt {
            raid: id,
            member: EntityId(2),
            health: Float(1.0),
        },
        &mut output,
    );
    let (_, raid) = region.find_raid(id).unwrap();
    assert!(raid.members.is_empty());
    assert_eq!(raid.heroes, [hero]);
    // A death keeps its health in the total (`removeFromRaid(…, false)`), a leave takes
    // it away (`removeFromRaid(…, true)`).
    assert_eq!(raid.total_health, Float(24.0));
}

#[test]
fn a_start_whose_centre_is_another_region_s_is_sent_there() {
    let mut region = loaded_in(WEST);
    let mut output = TickOutput::default();
    let across = BlockPos::new(40, -60, 8);
    region.create_or_extend_raid(across, 2, &mut output);
    assert!(region.own_raids().is_empty());
    assert!(region.made.iter().any(|effect| effect.what
        == clustine_world::Effected::Raid(RaidNews::Start {
            centre: across,
            omen: 2
        })));
}

#[test]
fn a_player_near_a_raid_is_shown_its_bar_and_its_changes_and_loses_it_when_it_stops() {
    let mut region = loaded_in(ChunkArea::EVERYWHERE);
    let player = player(1);
    // At the player's feet, wherever they entered.
    let feet = region.players[&player].pose.position;
    let centre = crate::physics::containing([feet.x, feet.y, feet.z]);
    // A village at the raid, which a raid's tick needs or stops it (part R1's rules).
    region
        .chunks
        .get_mut(&centre.chunk())
        .unwrap()
        .set_structures(vec![clustine_world::KeptStructure {
            kind: clustine_world::KeptKind::VillagePlains,
            source: centre.chunk(),
            bounds: clustine_world::KeptBox {
                min: centre,
                max: centre,
            },
            pieces: vec![clustine_world::KeptBox {
                min: centre,
                max: centre,
            }],
        }]);
    // Its bell, its meeting point taken: villages are where villagers claim.
    let chunk = region.chunks.get_mut(&centre.chunk()).unwrap();
    let (x, z) = centre.in_chunk();
    chunk.set(x, centre.y, z, clustine_data::blocks::BELL);
    assert!(chunk.acquire_ticket(clustine_world::InChunk::new(x, centre.y, z)));
    let mut output = TickOutput::default();
    region.create_or_extend_raid(centre, 1, &mut output);
    let id = region.own_raids()[0].1.id;
    region.clock.game_time = 20;
    region.tick_raids(&mut output);
    let bars: Vec<BarChange> = output
        .player_events
        .iter()
        .filter_map(|(to, event)| match event {
            PlayerEvent::BossBar { change, .. } if *to == player => Some(*change),
            _ => None,
        })
        .collect();
    // Added as the official server adds it: as it stood when its players were gathered,
    // before the countdown's first step (`raids_oracle.rs`).
    assert_eq!(
        bars,
        [
            BarChange::Add {
                name: BarName::Raid(RaidBarName::Raid),
                progress: 0.0,
                style: BarStyle::RAID,
            },
            BarChange::Progress(1.0 / 300.0)
        ],
        "its countdown's first tick"
    );
    let mut output = TickOutput::default();
    region.clock.game_time = 21;
    region.tick_raids(&mut output);
    assert!(output.player_events.iter().any(|(_, event)| matches!(
        event,
        PlayerEvent::BossBar { change: BarChange::Progress(progress), .. }
            if *progress == 2.0 / 300.0
    )));
    let mut output = TickOutput::default();
    region.change_raid(id, &mut output, |raid| raid.status = RaidStatus::Stopped);
    region.clock.game_time = 40;
    region.tick_raids(&mut output);
    assert!(
        region.own_raids().is_empty(),
        "a stopped raid is taken away"
    );
    assert!(output.player_events.iter().any(|(_, event)| matches!(
        event,
        PlayerEvent::BossBar {
            change: BarChange::Remove,
            ..
        }
    )));
}

#[test]
fn bad_omen_outside_a_village_stays_bad_omen() {
    use crate::mobs::body::effects::{Effect, Layer};
    let mut region = loaded_in(ChunkArea::EVERYWHERE);
    let id = player(1);
    let mut output = TickOutput::default();
    let now = region.tick;
    region.add_player_effect(
        id,
        Effect::BAD_OMEN,
        Layer::new(0, 120_000, now),
        &mut output,
    );
    region.tick_player_effects(&mut output);
    let player = &region.players[&id];
    assert!(player.effects.has(Effect::BAD_OMEN));
    assert!(!player.effects.has(Effect::RAID_OMEN));
    assert_eq!(player.raid_omen_position, None);
}

#[test]
fn raid_omen_s_last_tick_starts_a_raid_where_it_began_and_ends_it() {
    use crate::mobs::body::effects::{Effect, Layer};
    let mut region = loaded_in(ChunkArea::EVERYWHERE);
    let id = player(1);
    let mut output = TickOutput::default();
    let now = region.tick;
    // Its last tick is now: the level of amplifier 1 ends at this very tick.
    region.add_player_effect(id, Effect::RAID_OMEN, Layer::new(1, 0, now), &mut output);
    region.players.get_mut(&id).unwrap().raid_omen_position = Some(CENTRE);
    region.tick_player_effects(&mut output);
    let raids = region.own_raids();
    assert_eq!(raids.len(), 1);
    assert_eq!(raids[0].1.centre, CENTRE, "no village: the place itself");
    assert_eq!(raids[0].1.omen_level, 2, "amplifier 1 is an omen of 2");
    let player = &region.players[&id];
    assert!(!player.effects.has(Effect::RAID_OMEN));
    assert_eq!(player.raid_omen_position, None);
}
