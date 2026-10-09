//! What the harnesses of `chaos.rs` and `moves.rs` share when their world is not
//! pinned: a cluster whose store is told nothing of how the world is divided and
//! whose coordinator does what it does when told nothing, with four bots that stand
//! so far apart that each is split off into a region of its own. See
//! `docs/adr/0017-the-end-of-the-stripes.md`, section 9.6, "Chaos and moves without
//! pins".
//!
//! Such a world moves by itself only when its players do. The bots stay where they
//! are once each has its region, so whatever the coordinator begins by itself from
//! then until a test's bots are told to end has moved the world under the test, and
//! the test fails for it: [`moved_under_the_test`] says what did.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clustine_rpc::RegionList;

use super::wandering::{Begun, Ended, bounds, has_chunk, living, time_of};

/// The view distance of the edge, and what the coordinator is told it is: the
/// distances that follow from it are 10 and 18 chunks.
pub const VIEW: i32 = 2;

/// How long the coordinator leaves a region alone after anything it did to it.
pub const REST: Duration = Duration::from_secs(5);

/// How many bots play, and how far apart their lanes are, in blocks: 19 chunks,
/// which is more than the split distance, so that each bot is a group of its own.
pub const BOTS: usize = 4;
pub const LANE_TO_LANE: i32 = 304;

/// One worker for each region there will be and one to spare, so that while all of
/// them live no worker runs two more than another and the coordinator moves nothing
/// by itself under the test.
pub const WORKERS: usize = BOTS + 1;

/// Blocks a tick on the way to the lanes, of which the farthest is 912 blocks from
/// where players enter, for the bots and for their auditor after them: sixty blocks
/// a second, which no player comes near. At eight blocks a tick, which the bots of
/// `moves.rs` run to their wide lanes at, `wanders.rs` twice met a player who was
/// left without a view after being split off; see the end of its W9.
pub const TO_THE_LANE: f64 = 3.0;

/// How long a world may take to come to four regions that have rested: three of the
/// four bots are split off one split and one rest at a time, and each part is moved
/// once, each move a rest after the split before it.
pub const SETTLES_WITHIN: Duration = Duration::from_secs(240);

/// What the coordinator of such a world is started with: the view distance and the
/// rest, and nothing of how it reshapes, which is what is tested.
pub fn coordinator_arguments() -> Vec<String> {
    [
        "--view-distance",
        &VIEW.to_string(),
        "--rest-seconds",
        &REST.as_secs().to_string(),
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The chunk the bot numbered `number` is in when it stands at the block x
/// coordinate `x` on its lane.
pub fn chunk_of(number: usize, x: f64) -> (i32, i32) {
    let lane = number as i32 * LANE_TO_LANE;
    ((x.floor() as i32).div_euclid(16), lane.div_euclid(16))
}

/// The region of `list` whose `bounds` have `chunk`, if exactly one has.
pub fn region_of(list: &RegionList, chunk: (i32, i32)) -> Option<u32> {
    let holders = living(list).into_iter();
    let mut holders = holders.filter(|region| has_chunk(bounds(list, *region), chunk.0, chunk.1));
    holders.next().filter(|_| holders.next().is_none())
}

/// The region each of the bots is in, by `list` and by where each stands, if every
/// bot's chunk is within the `bounds` of a region of its own.
pub fn regions_of_their_own(list: &RegionList, bots: &[f64]) -> Option<Vec<u32>> {
    let mut regions = Vec::new();
    for (number, x) in bots.iter().enumerate() {
        let region = region_of(list, chunk_of(number, *x))?;
        if regions.contains(&region) {
            return None;
        }
        regions.push(region);
    }
    (living(list).len() == bots.len()).then_some(regions)
}

/// The time of the logs now, in seconds since 1970.
fn now() -> f64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    now.as_secs_f64()
}

/// Whether the coordinator whose log is `log` has every region at rest and nothing
/// under way: everything it began by itself has ended, and its routing table has
/// not changed for longer than a rest, so that every region was given its owner,
/// split or merged at least a rest ago.
pub fn has_rested(log: &str) -> bool {
    let mut under_way: Vec<Begun> = Vec::new();
    for line in log.lines() {
        if let Some(begun) = Begun::read(line) {
            under_way.push(begun);
        } else if let Some(ended) = Ended::read(line) {
            under_way.retain(|begun| !begun.is_ended_by(&ended));
        }
    }
    let changed = log
        .lines()
        .rev()
        .find(|line| line.contains("the routing table changed"))
        .and_then(time_of);
    let rested = changed.is_some_and(|changed| now() - changed > REST.as_secs_f64() + 1.0);
    under_way.is_empty() && rested
}

/// What the coordinator did by itself between the end of a test's start and the
/// mark the test takes just before it tells its bots to end, if it did anything:
/// the bots stood still, so the world was to stay as it was.
///
/// `start` and `mark` are the store's list at those two moments and `log` what the
/// coordinator logged between them. A release to even regions out is as it should
/// be once the test has taken a worker away, by killing it, freezing it or telling
/// it to stop: `took_away` is how long `log` was when it first did, if it did.
pub fn moved_under_the_test(
    start: &RegionList,
    mark: &RegionList,
    log: &str,
    took_away: Option<usize>,
) -> Option<String> {
    if living(start) != living(mark) || start.next != mark.next || start.absorbed != mark.absorbed {
        return Some(format!(
            "the list was to have the regions it had when the test began, the same next id \
             and the same absorbed pairs: it had {start:?} then and has {mark:?} now"
        ));
    }
    let reshaped = log
        .lines()
        .find(|line| Begun::read(line).is_some_and(|begun| begun.reshapes()));
    if let Some(line) = reshaped {
        return Some(format!(
            "the coordinator was to begin no split, no merge and no absorption while the \
             bots stood: {line}"
        ));
    }
    let before = log.get(..took_away.unwrap_or(log.len())).unwrap_or(log);
    let evened = before
        .lines()
        .find(|line| line.contains("a region is moved to even regions out"));
    evened.map(|line| {
        format!(
            "the coordinator was to move no region by itself before the test took a worker \
             away: {line}"
        )
    })
}
