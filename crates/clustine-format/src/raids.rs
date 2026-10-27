//! A chunk's raids as a chunk file and a commit write them (`docs/world-format.md`,
//! "Raids"; `docs/groundwork/n10-plan.md`, section 9.1).
//!
//! | Field | Type |
//! |---|---|
//! | id | u64, an entity key |
//! | centre | i32 x, i32 y, i32 z |
//! | flags | u8: bit 0 started, bit 1 active, bit 2 with a spawn position; the others 0 |
//! | ticks active | u64 |
//! | omen level, groups spawned, group count, cooldown, post-raid ticks, celebration ticks | six i32 |
//! | total health | f32 |
//! | status | u8: 0 ongoing, 1 victory, 2 loss, 3 stopped |
//! | spawn position, with bit 2 only | i32 x, i32 y, i32 z |
//! | the bar's UUID | u128 |
//! | the bar | u8 shown (0 or 1); f32 progress; u8 name: 0 the raid's, 1 raiders remaining followed by i32 the count, 2 victory, 3 defeat |
//! | heroes | count u32, then each u128, ascending |
//! | members | count u32, then each wave i32, key u64, health f32, ascending by wave and key |
//! | leaders | count u32, then each wave i32, key u64, ascending by wave |

use clustine_world::raid::{Raid, RaidBarName, RaidMember, RaidStatus};
use clustine_world::{BlockPos, EntityId, Float};
use uuid::Uuid;

use crate::FormatError;
use crate::bytes::Input;

const STARTED: u8 = 1;
const ACTIVE: u8 = 2;
const WITH_SPAWN: u8 = 4;

fn put_pos(out: &mut Vec<u8>, pos: BlockPos) {
    out.extend_from_slice(&pos.x.to_be_bytes());
    out.extend_from_slice(&pos.y.to_be_bytes());
    out.extend_from_slice(&pos.z.to_be_bytes());
}

fn take_pos(input: &mut Input) -> Result<BlockPos, FormatError> {
    Ok(BlockPos::new(input.i32()?, input.i32()?, input.i32()?))
}

/// Writes `raids`, a count u32 and each raid.
pub(crate) fn put_raids(out: &mut Vec<u8>, raids: &[Raid]) {
    out.extend_from_slice(&(raids.len() as u32).to_be_bytes());
    for raid in raids {
        out.extend_from_slice(&raid.id.0.to_be_bytes());
        put_pos(out, raid.centre);
        let mut flags = 0;
        if raid.started {
            flags |= STARTED;
        }
        if raid.active {
            flags |= ACTIVE;
        }
        if raid.spawn_pos.is_some() {
            flags |= WITH_SPAWN;
        }
        out.push(flags);
        out.extend_from_slice(&raid.ticks_active.to_be_bytes());
        for value in [
            raid.omen_level,
            raid.groups_spawned,
            raid.group_count,
            raid.cooldown_ticks,
            raid.post_raid_ticks,
            raid.celebration_ticks,
        ] {
            out.extend_from_slice(&value.to_be_bytes());
        }
        out.extend_from_slice(&raid.total_health.0.to_be_bytes());
        out.push(raid.status.id());
        if let Some(spawn) = raid.spawn_pos {
            put_pos(out, spawn);
        }
        out.extend_from_slice(&raid.bar.as_u128().to_be_bytes());
        out.push(u8::from(raid.bar_shown));
        out.extend_from_slice(&raid.bar_progress.0.to_be_bytes());
        match raid.bar_name {
            RaidBarName::Raid => out.push(0),
            RaidBarName::RaidersRemaining(count) => {
                out.push(1);
                out.extend_from_slice(&count.to_be_bytes());
            }
            RaidBarName::Victory => out.push(2),
            RaidBarName::Defeat => out.push(3),
        }
        out.extend_from_slice(&(raid.heroes.len() as u32).to_be_bytes());
        for hero in &raid.heroes {
            out.extend_from_slice(&hero.as_u128().to_be_bytes());
        }
        out.extend_from_slice(&(raid.members.len() as u32).to_be_bytes());
        for member in &raid.members {
            out.extend_from_slice(&member.wave.to_be_bytes());
            out.extend_from_slice(&member.key.0.to_be_bytes());
            out.extend_from_slice(&member.health.0.to_be_bytes());
        }
        out.extend_from_slice(&(raid.leaders.len() as u32).to_be_bytes());
        for (wave, key) in &raid.leaders {
            out.extend_from_slice(&wave.to_be_bytes());
            out.extend_from_slice(&key.0.to_be_bytes());
        }
    }
}

/// Reads what [`put_raids`] wrote.
pub(crate) fn take_raids(input: &mut Input) -> Result<Vec<Raid>, FormatError> {
    let count = input.u32()? as usize;
    // A raid takes 81 bytes at the least.
    let mut raids = Vec::with_capacity(count.min(input.0.len() / 81));
    for _ in 0..count {
        let id = EntityId(input.u64()?);
        let centre = take_pos(input)?;
        let flags = input.u8()?;
        if flags & !(STARTED | ACTIVE | WITH_SPAWN) != 0 {
            return Err(FormatError::Corrupt("a raid's flags"));
        }
        let ticks_active = input.u64()?;
        let mut ints = [0; 6];
        for value in &mut ints {
            *value = input.i32()?;
        }
        let total_health = Float(input.f32()?);
        let status =
            RaidStatus::from_id(input.u8()?).ok_or(FormatError::Corrupt("a raid's status"))?;
        let spawn_pos = if flags & WITH_SPAWN != 0 {
            Some(take_pos(input)?)
        } else {
            None
        };
        let bar = Uuid::from_u128(input.u128()?);
        let bar_shown = match input.u8()? {
            0 => false,
            1 => true,
            _ => return Err(FormatError::Corrupt("a raid's bar shown flag")),
        };
        let bar_progress = Float(input.f32()?);
        let bar_name = match input.u8()? {
            0 => RaidBarName::Raid,
            1 => RaidBarName::RaidersRemaining(input.i32()?),
            2 => RaidBarName::Victory,
            3 => RaidBarName::Defeat,
            _ => return Err(FormatError::Corrupt("a raid's bar name")),
        };
        let heroes_count = input.u32()? as usize;
        let mut heroes = Vec::with_capacity(heroes_count.min(input.0.len() / 16));
        for _ in 0..heroes_count {
            heroes.push(Uuid::from_u128(input.u128()?));
        }
        let members_count = input.u32()? as usize;
        let mut members = Vec::with_capacity(members_count.min(input.0.len() / 16));
        for _ in 0..members_count {
            members.push(RaidMember {
                wave: input.i32()?,
                key: EntityId(input.u64()?),
                health: Float(input.f32()?),
            });
        }
        let leaders_count = input.u32()? as usize;
        let mut leaders = Vec::with_capacity(leaders_count.min(input.0.len() / 12));
        for _ in 0..leaders_count {
            leaders.push((input.i32()?, EntityId(input.u64()?)));
        }
        let [
            omen_level,
            groups_spawned,
            group_count,
            cooldown_ticks,
            post_raid_ticks,
            celebration_ticks,
        ] = ints;
        raids.push(Raid {
            id,
            centre,
            started: flags & STARTED != 0,
            active: flags & ACTIVE != 0,
            ticks_active,
            omen_level,
            groups_spawned,
            group_count,
            cooldown_ticks,
            post_raid_ticks,
            celebration_ticks,
            total_health,
            status,
            spawn_pos,
            bar,
            bar_shown,
            bar_progress,
            bar_name,
            heroes,
            members,
            leaders,
        });
    }
    if !raids.windows(2).all(|pair| pair[0].id < pair[1].id) {
        return Err(FormatError::Corrupt(
            "a chunk's raids are not ascending, each once",
        ));
    }
    Ok(raids)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn raid(id: u64) -> Raid {
        Raid {
            id: EntityId(id),
            centre: BlockPos::new(-40, 64, 12),
            started: true,
            active: true,
            ticks_active: 1234,
            omen_level: 2,
            groups_spawned: 1,
            group_count: 3,
            cooldown_ticks: 0,
            post_raid_ticks: 0,
            celebration_ticks: 0,
            total_health: Float(48.0),
            status: RaidStatus::Ongoing,
            spawn_pos: Some(BlockPos::new(-10, 70, 30)),
            bar: Uuid::from_u128(0x1234),
            bar_shown: true,
            bar_progress: Float(0.75),
            bar_name: RaidBarName::RaidersRemaining(2),
            heroes: vec![Uuid::from_u128(1), Uuid::from_u128(9)],
            members: vec![
                RaidMember {
                    wave: 1,
                    key: EntityId(77),
                    health: Float(24.0),
                },
                RaidMember {
                    wave: 1,
                    key: EntityId(78),
                    health: Float(24.0),
                },
            ],
            leaders: vec![(1, EntityId(77))],
        }
    }

    #[test]
    fn raids_are_read_back_as_they_were_written() {
        let mut quiet = raid(5);
        quiet.spawn_pos = None;
        quiet.bar_name = RaidBarName::Victory;
        quiet.status = RaidStatus::Victory;
        let raids = vec![raid(3), quiet];
        let mut out = Vec::new();
        put_raids(&mut out, &raids);
        let mut input = Input(&out);
        assert_eq!(take_raids(&mut input), Ok(raids));
        assert!(input.0.is_empty());
    }

    #[test]
    fn raids_out_of_order_or_with_flags_that_mean_nothing_are_refused() {
        let mut out = Vec::new();
        put_raids(&mut out, &[raid(5), raid(3)]);
        assert_eq!(
            take_raids(&mut Input(&out)),
            Err(FormatError::Corrupt(
                "a chunk's raids are not ascending, each once"
            ))
        );
        let mut out = Vec::new();
        put_raids(&mut out, &[raid(3)]);
        // The flags come after the count, the id and the centre: 4 + 8 + 12 bytes.
        out[24] |= 8;
        assert_eq!(
            take_raids(&mut Input(&out)),
            Err(FormatError::Corrupt("a raid's flags"))
        );
    }
}
