//! The crowd: how long a merge and a split stand a large home region still, and
//! whether that grows with its players or with its land. This is the scenario X1 of
//! `docs/adr/0017-the-end-of-the-stripes.md`, section 9.7, written from that record.
//!
//! **The world** is a cluster of two workers without pins, whose edge and coordinator
//! are told the same view distance and the coordinator a rest of 5 s: regions follow
//! their players by the distances that follow from the view distance. **The crowd**
//! is `N` bots in ledgers of twenty, all in the chunk players enter in, each ledger
//! on rows of its own: a strip one chunk wide from the spawn point south, all of the
//! home region. **Those who go** are a ledger of two beside the crowd's first row.
//! They are sent two chunks beyond the split distance and are split off, and are sent
//! back to the merge distance and are merged, round after round.
//!
//! For every split and every merge the test records, between the coordinator's line
//! that it began and the line that it ended and for two seconds after, the longest
//! any bot of the crowd waited for an acknowledgement, the longest either of the two
//! waited, how long the coordinator took by its own lines, and what the worker says
//! of how long region 0 stood still, with how many players and how many chunks. It
//! prints the least, the middle and the worst of each, beside what a bot of the crowd
//! waits when nothing happens and how long the store takes to make its list of
//! regions while the crowd stands.
//!
//! An ordinary run has twenty bots, a view distance of 2 and two rounds, and asserts
//! that nobody is disconnected, that as many splits and merges end well as there were
//! rounds, that no wait is above 5 s and that the ledgers' audits hold. The
//! measurement is the same test with other numbers and an optimised build:
//!
//! ```text
//! CLUSTINE_CROWD=100 CLUSTINE_CROWD_VIEW=8 CLUSTINE_CROWD_ROUNDS=5 \
//!     cargo test --release -p clustine --test crowds -- --nocapture
//! ```
//!
//! `CLUSTINE_CROWD_SPACING` sets how far apart the lanes of a ledger are, in blocks,
//! 4 unless said: with 16 the same crowd stands on four times the land.
//! `CLUSTINE_CROWD_CHECKPOINT` sets how often the workers write a checkpoint, in
//! seconds. Unless said that follows from the seed, as in the other tests here, and
//! is every second or two in two runs of three; a server that is told nothing writes
//! one every 300 s, and the measurement says so: with a hundred players and more a
//! store that is asked for a checkpoint every second or two does not keep up, and
//! what a split or a release waits for then is the store and not the region.
//! `CLUSTINE_CROWD_SEED` runs a seed again, and `CLUSTINE_CROWD_KEEP` keeps the world
//! and the logs of a run that passes.
//!
//! `CLUSTINE_CROWD_LAND` serves the land of that seed rather than the flat world
//! (ADR-0021's measurements, W5m), and `CLUSTINE_CROWD_FAR=1` sends one bot far from
//! the crowd before the rounds and then on east through land nobody has been in, at
//! a flying sprint, for as long as the rounds last: a third region whose view is made
//! from nothing while the crowd's region is split and merged. One bot, because the
//! bots of a ledger walk at paces of their own: two drift apart, and their region is
//! split, which takes the id the crowd's part was to have.

mod common;

use std::time::{Duration, Instant};

use clustine_botswarm::Ledger;
use clustine_rpc::RegionList;

use common::wandering::{
    Begun, PATIENCE, Region, Setup, TICK, WRITING, Wandering, Wanders, a_repetition, absorbed_by,
    bounds, chunk_of, has_chunk, living, number_from, seconds, spread, within,
};

/// How many bots a ledger of the crowd has.
const LEDGER: usize = 20;

/// How long the coordinator leaves a region alone after anything it did to it.
const REST: Duration = Duration::from_secs(5);

/// Blocks a tick of the two who go: ten blocks a second, so that a round is walked
/// in a minute or two.
const BRISK: f64 = 0.5;

/// Blocks a tick of those who go far: a flying sprint, so that their region asks for
/// land nobody has been in all the time.
const FAR_SPEED: f64 = 1.08;

/// The row the lane of those who go far is on: sixty chunks north of the crowd, well
/// beyond the split distance of any view distance a bot asks for.
const FAR_LANE: i32 = -60 * 16;

/// What was recorded of the splits, or of the merges, of a run.
#[derive(Default)]
struct Recorded {
    /// The longest any bot of the crowd waited, and either of the two who went.
    crowd: Vec<Duration>,
    two: Vec<Duration>,
    /// How long the coordinator took, by its own lines.
    took: Vec<Duration>,
    /// How long region 0 stood still by its worker's line, and with how many players
    /// and how many chunks.
    stood: Vec<Duration>,
    players: Vec<u32>,
    held: Vec<u32>,
}

impl Recorded {
    /// A line of the table: the least, the middle and the worst of each, in seconds.
    fn line(&self, what: &str) -> String {
        let range = |numbers: &[u32]| match (numbers.iter().min(), numbers.iter().max()) {
            (Some(least), Some(most)) if least == most => least.to_string(),
            (Some(least), Some(most)) => format!("{least} to {most}"),
            _ => "-".to_owned(),
        };
        format!(
            "{what} ({}) | {} | {} | {} | {} | {} | {}",
            self.took.len(),
            spread(&self.crowd),
            spread(&self.two),
            spread(&self.took),
            spread(&self.stood),
            range(&self.players),
            range(&self.held)
        )
    }
}

/// X1. A crowd stands in the chunk players enter in; two walk away from it, are
/// split off, come back and are merged, round after round. Nobody is disconnected,
/// as many splits and merges end well as there were rounds, region 0's worker says
/// how long it stood still for each of them, no bot waits longer than 5 s, and the
/// ledgers' audits hold. What each split and each merge cost the crowd is printed.
#[tokio::test(flavor = "multi_thread")]
async fn a_crowd_at_the_spawn_point_stands_still_briefly_when_two_leave_it_and_come_back() {
    if a_repetition() {
        return;
    }
    let crowd = number_from("CLUSTINE_CROWD").unwrap_or(20) as usize;
    let view = number_from("CLUSTINE_CROWD_VIEW").unwrap_or(2) as i32;
    let rounds = number_from("CLUSTINE_CROWD_ROUNDS").unwrap_or(2) as u32;
    let spacing = number_from("CLUSTINE_CROWD_SPACING").unwrap_or(4) as i32;
    // What the rule gives for the view distance.
    let merge_distance = 2 * view + 8;
    let split_distance = merge_distance + 8;
    let world = Setup {
        family: "crowd",
        workers: 2,
        view,
        lease: None,
        rest: REST,
        // It measures, so it runs with no other cluster beside it.
        alone: true,
        checkpoint: number_from("CLUSTINE_CROWD_CHECKPOINT"),
        land: number_from("CLUSTINE_CROWD_LAND"),
    };
    let far = number_from("CLUSTINE_CROWD_FAR").unwrap_or(0) > 0;
    let world_land = world.land;
    let mut wanders = Wanders::cluster("the crowd", world).await;

    // Each ledger on rows of its own, with one left free before the next: a ledger
    // uses the rows from its first lane to two beyond its last.
    let rows = LEDGER as i32 * spacing + 4;
    let mut names: Vec<&'static str> = Vec::new();
    for number in 0..crowd.div_ceil(LEDGER) {
        // A group is known by a name that lasts as long as the test's process.
        let name: &'static str = Box::leak(format!("C{number}").into_boxed_str());
        let bots = (crowd - number * LEDGER).min(LEDGER);
        let scenario = Ledger {
            lane_spacing: spacing,
            // The furthest row of two hundred is fifty chunks from where players
            // enter, a long way at a player's pace.
            to_the_lane: 3.0,
            ..wanders.scenario(name, bots, rows * number as i32, within(0))
        };
        wanders.joins_with(name, scenario);
        names.push(name);
    }
    let scenario = Ledger {
        speed: BRISK,
        ..wanders.scenario("Two", 2, -12, within(0))
    };
    wanders.joins_with("Two", scenario);
    for name in names.iter().chain(&["Two"]) {
        wanders.arrives(name).await;
    }
    let list = wanders.whole().await;
    if living(&list) != [0] {
        wanders.fail(&format!(
            "the crowd and the two were to be one region: {list:?}"
        ));
    }

    // Those who go far are split off by themselves before anything is counted, and
    // their region is left out of what the rounds look for.
    let mut far_region = None;
    if far {
        let scenario = Ledger {
            speed: FAR_SPEED,
            to_the_lane: 3.0,
            ..wanders.scenario("Far", 1, FAR_LANE, within(0))
        };
        wanders.joins_with("Far", scenario);
        wanders.arrives("Far").await;
        let list = wanders
            .until_the_list_within(PATIENCE, "those who go far are split off", |list| {
                living(list).len() == 2
            })
            .await;
        far_region = living(&list).into_iter().find(|region| *region != 0);
    }
    let ours = |list: &RegionList| -> Vec<Region> {
        let living = living(list);
        living
            .into_iter()
            .filter(|region| Some(*region) != far_region)
            .collect()
    };

    // What a bot of the crowd waits when nothing happens, and how long the store
    // takes to make its list while the crowd stands and nothing else happens.
    wanders.served().await;
    let quiet = Instant::now();
    for _ in 0..10 {
        wanders.served().await;
    }
    let of_the_crowd = |wanders: &Wanders, from: Instant, to: Instant| {
        let waits = names
            .iter()
            .filter_map(|name| wanders.longest_wait_of(name, from, to));
        waits.max().unwrap_or_default()
    };
    let calm = of_the_crowd(&wanders, quiet, Instant::now());
    let mut listing: Vec<Duration> = Vec::new();
    for _ in 0..20 {
        let asked = Instant::now();
        wanders.list().await;
        listing.push(asked.elapsed());
    }

    let from = wanders.now_at();
    let counting = Instant::now();
    // How long a walk of the two may take: three times their way at their own pace,
    // and the patience on top. It only ever runs out when something hangs.
    let way = f64::from(16 * (split_distance + 3));
    let walk = PATIENCE + 3 * TICK.mul_f64(way / BRISK);
    if far {
        // Further than the rounds can take them.
        wanders.walks_to("Far", 100_000);
    }
    for round in 1..=rounds {
        let part = wanders.list().await.next.0;
        wanders.walks_to("Two", split_distance + 2);
        wanders
            .until_the_list_within(walk, "the two are split off", |list| {
                ours(list) == [0, part]
            })
            .await;
        wanders.whole().await;
        wanders.arrives("Two").await;
        wanders.walks_to("Two", merge_distance);
        let merged = "the two are merged into region 0 again";
        wanders
            .until_the_list_within(walk, merged, |list| {
                absorbed_by(list, part) == Some(0) && ours(list) == [0]
            })
            .await;
        wanders.whole().await;
        wanders.arrives("Two").await;
        wanders.note(format!("round {round} of {rounds} is done"));
    }
    // The two seconds after the last of them, by the bots' own answers.
    wanders.served().await;
    wanders.walks_for("Two", Duration::from_secs(2)).await;

    let (mut splits, mut merges) = (Recorded::default(), Recorded::default());
    let stood = wanders.stood();
    let begun = wanders.reshapes_at();
    for (at, begun) in begun.iter().filter(|(at, _)| *at >= from) {
        let end = wanders.end_of(*at, begun);
        let Some((ended_at, _)) = end.filter(|(_, ended)| ended.well()) else {
            continue;
        };
        let of = match begun {
            Begun::Split { region: 0, .. } => &mut splits,
            Begun::Merge { survivor: 0, .. } => &mut merges,
            _ => continue,
        };
        let since = wanders.instant_of(*at);
        let until = wanders.instant_of(ended_at) + Duration::from_secs(2);
        of.crowd.push(of_the_crowd(&wanders, since, until));
        let two = wanders.longest_wait_of("Two", since, until);
        of.two.push(two.unwrap_or_default());
        of.took.push(Duration::from_secs_f64(ended_at - at));
        // The line is written when the region ticks again, a moment before or after
        // the coordinator has heard how the thing ended.
        let line = stood.iter().find(|stood| {
            stood.region == 0 && stood.at >= at - WRITING && stood.at <= ended_at + 2.0
        });
        let Some(line) = line else {
            wanders.fail(&format!(
                "no worker said how long region 0 stood still for {}",
                begun.told()
            ));
        };
        of.stood.push(Duration::from_millis(line.milliseconds));
        of.players.push(line.players);
        of.held.push(line.held);
    }

    println!(
        "X1: a crowd of {crowd} on lanes {spacing} blocks apart, a view distance of {view} \
         (merged at {merge_distance} chunks, split beyond {split_distance}), {rounds} rounds, \
         seed {}, {}{}; seconds as least / middle / worst",
        wanders.seed,
        match world_land {
            Some(land) => format!("the land of seed {land}"),
            None => "the flat world".to_owned(),
        },
        if far { ", one going far" } else { "" }
    );
    println!(
        "X1: what (how many) | the crowd waited | the two waited | the coordinator took | \
         region 0 stood still | players | chunks held"
    );
    println!("X1: {}", splits.line("split"));
    println!("X1: {}", merges.line("merge"));
    println!(
        "X1: when nothing happens a bot of the crowd waits {} at most; the store's list of \
         regions took {} in twenty readings",
        seconds(calm),
        spread(&listing)
    );

    // Not asserted, and said for whoever reads the numbers: one of the two who was
    // handed over on the way is counted with those it was handed to.
    let handed_over = wanders.handed_over();
    let handed_over = handed_over.iter().filter(|(at, _)| *at >= from);
    println!(
        "X1: the workers logged {} players arriving from or departing to another region",
        handed_over.count()
    );

    let ended = (splits.took.len() as u32, merges.took.len() as u32);
    if ended != (rounds, rounds) {
        wanders.fail(&format!(
            "{rounds} splits and {rounds} merges of region 0 were to end well; {} splits and \
             {} merges did",
            ended.0, ended.1
        ));
    }
    wanders.nobody_waited_too_long(counting);
    wanders.finish().await;
}

/// What is measured (ADR-0021, W5m): the merge of two regions whose players both walk
/// outward, into land nobody has been in, against the record's second at worst. Two
/// wanderers on the land of seed 13579 (`CLUSTINE_CROWD_LAND` for another), with a
/// view distance of 8, walk apart until their regions are split and then towards each
/// other until they are merged on the way, at a flying sprint, round after round
/// (`CLUSTINE_CROWD_ROUNDS`, 5 unless said). Every walk takes them 30 chunks further
/// north than the last, so that whatever comes into their view is made from nothing.
/// Printed for each merge and split: how long the coordinator took, and how long the
/// region stood still by its worker's line, with how many chunks.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement that takes minutes; run it by name with --ignored --nocapture"]
async fn measured_two_regions_whose_players_walk_outward_are_merged_on_the_way() {
    if a_repetition() {
        return;
    }
    const VIEW: i32 = 8;
    // Half the distance between the two when apart and when together, and how far
    // north each walk takes them, in chunks: the rule merges at 22 and splits
    // beyond 30 for a view of 8.
    const APART: i32 = 25;
    const TOGETHER: i32 = 5;
    const NORTH: i32 = 30;
    let rounds = number_from("CLUSTINE_CROWD_ROUNDS").unwrap_or(5) as i32;
    let land = number_from("CLUSTINE_CROWD_LAND").unwrap_or(13579);
    let world = Setup {
        family: "outward",
        workers: 3,
        view: VIEW,
        lease: None,
        rest: REST,
        alone: true,
        checkpoint: number_from("CLUSTINE_CROWD_CHECKPOINT"),
        land: Some(land),
    };
    let mut wanders = Wanders::cluster("outward", world).await;
    let west = wanders.wanders("West").await;
    let east = wanders.wanders("East").await;
    let start = wanders.wanderers[west].noted();
    let (x, z) = (start.x, start.z);
    let block = |chunks: i32| f64::from(16 * chunks);
    // The region with the chunk a wanderer is in, if the list has one.
    let region_of = |list: &RegionList, noted: &Wandering| {
        let (cx, cz) = (chunk_of(noted.x), chunk_of(noted.z));
        living(list)
            .into_iter()
            .find(|region| has_chunk(bounds(list, *region), cx, cz))
    };
    let from = wanders.now_at();
    // When each walk began on the test's clock, and how long it takes at its pace.
    let mut walks: Vec<(Instant, Duration, bool)> = Vec::new();
    let mut walked = 0;
    // First together far enough north that their region is split off from the one
    // with the chunk players enter in, which would otherwise hold both.
    let legs = std::iter::once((TOGETHER, true, 2))
        .chain((0..rounds).flat_map(|_| [(APART, false, 1), (TOGETHER, true, 1)]));
    for (leg, (half, together, steps)) in legs.enumerate() {
        walked += steps;
        let north = z - block(NORTH * walked);
        let (west_at, east_at) = (x - block(half), x + block(half));
        let here = wanders.wanderers[west].noted();
        let way = (west_at - here.x).hypot(north - here.z);
        walks.push((Instant::now(), TICK.mul_f64(way / FAR_SPEED), together));
        wanders.wanderers[west].walks_to(west_at, north, FAR_SPEED);
        wanders.wanderers[east].walks_to(east_at, north, FAR_SPEED);
        wanders.wanderer_arrives(west, way, FAR_SPEED).await;
        wanders.wanderer_arrives(east, way, FAR_SPEED).await;
        let (w, e) = (
            wanders.wanderers[west].noted(),
            wanders.wanderers[east].noted(),
        );
        let what = if together {
            "the two are in one region"
        } else {
            "the two are in regions of their own"
        };
        wanders
            .until_the_list_within(PATIENCE, what, |list| {
                let (w, e) = (region_of(list, &w), region_of(list, &e));
                let home = Some(list.home.0 as Region);
                w.is_some() && e.is_some() && (w == e) == together && w != home && e != home
            })
            .await;
        wanders.whole().await;
        if leg > 0 && !together {
            wanders.note(format!(
                "round {} of {rounds} has split them",
                leg.div_ceil(2)
            ));
        }
    }

    println!(
        "OUTWARD: two wanderers on the land of seed {land}, a view distance of {VIEW}, \
         {rounds} rounds, seed {}; each merge and split: when it was begun in its walk, \
         how long the coordinator took, how long the region stood still and with how \
         many chunks",
        wanders.seed
    );
    let stood = wanders.stood();
    // How many of each ended well.
    let (mut merges, mut splits) = (0, 0);
    for (at, begun) in wanders.reshapes_at().iter().filter(|(at, _)| *at >= from) {
        let (region, of) = match begun {
            Begun::Merge { survivor, .. } => (*survivor, &mut merges),
            Begun::Split { region, .. } => (*region, &mut splits),
            _ => continue,
        };
        let Some((ended_at, ended)) = wanders.end_of(*at, begun) else {
            continue;
        };
        let when = wanders.instant_of(*at);
        let walk = walks.iter().rev().find(|(began, _, _)| *began <= when);
        let into = walk.map_or("before the walks".to_owned(), |(began, takes, _)| {
            format!(
                "{:.1} s into a walk of {:.1} s",
                (when - *began).as_secs_f64(),
                takes.as_secs_f64()
            )
        });
        let line = stood.iter().find(|stood| {
            stood.region == region && stood.at >= at - WRITING && stood.at <= ended_at + 2.0
        });
        let still = line.map_or("no line".to_owned(), |line| {
            format!(
                "{:.3} s with {} chunks",
                line.milliseconds as f64 / 1000.0,
                line.held
            )
        });
        println!(
            "OUTWARD: {} of region {region}{}: {into}, took {:.3} s, stood still {still}",
            if matches!(begun, Begun::Merge { .. }) {
                "merge"
            } else {
                "split"
            },
            if ended.well() {
                ""
            } else {
                " (did not end well)"
            },
            ended_at - at
        );
        if ended.well() {
            *of += 1;
        }
    }
    if merges < rounds || splits < rounds {
        wanders.fail(&format!(
            "the two were to be merged and split {rounds} times; {merges} merges and {splits} splits ended well"
        ));
    }
    wanders.finish().await;
}
