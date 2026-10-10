//! A world without stripes, end to end: one home region, regions that are split off
//! with whoever walks away and grow with them, regions that merge when their players
//! meet, and regions nobody is in that are absorbed, with nobody asking for anything,
//! under players who keep playing and keep a ledger of what they were told. These are
//! the scenarios W1 to W11 of `docs/adr/0017-the-end-of-the-stripes.md`, section 9.6,
//! written from that record and from its sections 3 and 7 by someone who built none of
//! what they run: the first time regions that are not pinned run under players for
//! longer than a join.
//!
//! **The world** is a cluster of two workers, a store that is told nothing of how the
//! world is divided, an edge with a view distance of 2 and a coordinator that is told
//! that view distance, a rest of 5 s and nothing else: it reshapes by itself because
//! that is what a coordinator does when told nothing, with the distances the rule
//! gives, 10 and 18 chunks. With a view distance of 2 a client is sent the 7 by 7
//! chunks around its own, so a lone player's region holds exactly those once its
//! trail is given back. Some scenarios run on the single process as well: a `Server`
//! in the test's own process, of which the list and the bots are all there is to look
//! at, or a server process whose log is read.
//!
//! **The players** are groups, each a ledger scenario of the bots: `A`, two bots on
//! the lanes z = 0 and 4, and `B`, one on z = 8, all in the row of chunks z = 0; and
//! wanderers, plain bots that are in no ledger. A group stands in a chunk when its
//! bots walk up and down within it, and is sent to another by being given other
//! coordinates to walk between. Chunks are given by their x where the row is z = 0.
//!
//! What is expected is the record's, not the code's. Where the server does something
//! else, the test stays, marked `#[ignore = "finding: …"]`, with the sequence, what
//! the record says and what happened above it; and where a scenario could not be
//! written as the record has it, the test says what it does instead.
//!
//! Every test prints its seed; `CLUSTINE_WANDERS_SEED` runs one again.
//! `CLUSTINE_WANDERS_ROUNDS` sets how many rounds the tests of rounds do,
//! `CLUSTINE_WANDERS_KILLS` how many rounds the tests that kill do, and
//! `CLUSTINE_WANDERS_KEEP` keeps the world and the logs of a test that passes; those
//! of a test that fails are always kept, and the failure says where.
//!
//! The processes are those of an unoptimised build, as in every test here, so the
//! pauses are three to six times those of a server built for use.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clustine_botswarm::{Bot, Ledger};
use clustine_data::blocks;
use clustine_protocol::packets::play::face;
use clustine_rpc::RegionList;
use tokio::task::JoinHandle;

use common::processes::{Cluster, worker_name};
use common::wandering::{
    Begun, Ended, LONGEST_PAUSE, MOMENT, PATIENCE, Region, Setup, SplitOff, Stands, WRITING,
    Wanders, a_repetition, absorbed_by, between, bounds, has_chunk, info, is_box, living,
    number_from, seconds, within,
};

/// The view distance of the edge, and what the coordinator is told it is.
const VIEW: i32 = 2;

/// The distances that follow from it, in chunks: regions whose players are this near
/// are merged, and players of one region further apart than the second are split.
const MERGE_DISTANCE: i32 = 10;
const SPLIT_DISTANCE: i32 = 18;

/// How far a client with that view distance is sent chunks along an axis.
const REACH: i32 = VIEW + 1;

/// How long the coordinator leaves a region alone after anything it did to it, and
/// how long a region has to be without players before it is absorbed: three rests.
const REST: Duration = Duration::from_secs(5);
const EMPTY_FOR: Duration = Duration::from_secs(15);

/// How long a chunk that nothing uses stays a region's, and what the record waits for
/// it to be given back: thirty seconds, and five more.
const GIVEN_BACK: Duration = Duration::from_secs(35);

/// The chunk beyond the split distance from the chunk players enter in, in which a
/// group is split off; a chunk far beyond it; and one well within the merge distance.
const OUT: i32 = SPLIT_DISTANCE + 1;
const FAR: i32 = 40;
const BACK: i32 = 8;

/// Blocks a tick of the group that goes: ten blocks a second, the pace of the
/// wanderer of W10, so that a round is walked in minutes. And of a wanderer on foot.
const BRISK: f64 = 0.5;
const ON_FOOT: f64 = 0.5;

/// What the wait of a bot is held to "when nothing happens": three times the longest
/// it waited in a time in which nothing did, and a second at least. An unoptimised
/// region that stands still for a merge or a split keeps its players waiting for a
/// third of a second to a second. A machine that runs three clusters and whatever
/// else at once keeps a bot waiting for half a second and more now and then with
/// nothing happening to its region, for the disk that every region's commits go
/// through: with 0.3 s here, runs failed for that. So this catches a stop that is
/// longer than a split's, and not a second split; a second split is told by the
/// coordinator's and the workers' lines, which every scenario that uses this asserts
/// as well.
const NOTHING_HAPPENS: Duration = Duration::from_secs(1);

/// How long a merge or a split that a kill struck may take to be gone through again:
/// a region is left alone for three rests after an attempt that failed, and for twice
/// that after the next.
const AGAIN: Duration = Duration::from_secs(180);

/// The world of section 9.6: two workers, and the lease a coordinator has when it is
/// told none, or `lease` seconds where processes are killed.
fn world(lease: Option<u64>) -> Setup {
    Setup {
        family: "wanders",
        workers: 2,
        view: VIEW,
        lease,
        rest: REST,
        alone: false,
        checkpoint: None,
    }
}

/// How many rounds a test does: `CLUSTINE_WANDERS_ROUNDS` if set, else `default`.
fn rounds(default: u32) -> u32 {
    number_from("CLUSTINE_WANDERS_ROUNDS").map_or(default, |rounds| rounds as u32)
}

/// The middle of the chunk at `x` and `z`, as block coordinates.
fn middle_of(x: i32, z: i32) -> (f64, f64) {
    (f64::from(16 * x) + 8.0, f64::from(16 * z) + 8.0)
}

/// Where a wanderer walks to who is to be in the chunk at `x` of the row z = 0 and
/// comes from where players enter: along the block row they enter in, on which
/// nobody builds. A straight walk to the middle of chunk 19 leads through the plot
/// of the first bot of `A`, and a block that `A` places where the wanderer's body is
/// at that tick is acknowledged and not placed, as in the game: one run in thirty
/// or so failed for it.
fn east_along_the_row(x: i32) -> (f64, f64) {
    (f64::from(16 * x) + 8.0, 0.5)
}

/// What the scenarios do to a world, whichever way it is served.
impl Wanders {
    /// `A` joins and settles in the chunk players enter in.
    async fn a_settles(&mut self) {
        self.joins("A", 2, 0, within(0));
        self.arrives("A").await;
        self.plays("A").await;
    }

    /// `B` joins and settles in the chunk players enter in, on a lane of its own, as
    /// a group that will walk at a brisk pace.
    async fn b_settles(&mut self) {
        let scenario = Ledger {
            speed: BRISK,
            ..self.scenario("B", 1, 8, within(0))
        };
        self.joins_with("B", scenario);
        self.arrives("B").await;
        self.plays("B").await;
    }

    /// Fails unless the world is region 0 alone and runs. Returns the list.
    async fn region_0_is_alone(&mut self, what: &str) -> RegionList {
        let list = self.whole().await;
        if living(&list) != [0] || list.home.0 != 0 {
            self.fail(&format!(
                "{what} the list was to have region 0 alone: {list:?}"
            ));
        }
        list
    }

    /// Waits until the world is whole and, in a cluster, the workers share the
    /// regions evenly, so that the coordinator has no region to move from here on.
    async fn settled(&mut self) -> RegionList {
        let what = "every region runs and the workers share them evenly";
        let even = |wanders: &Self| wanders.a_cluster().is_none_or(Cluster::even);
        let list = self.runs_and(what, even).await;
        self.served().await;
        let loads = self.a_cluster().map(Cluster::loads);
        self.note(format!(
            "the world is whole, with the regions {:?} shared evenly as {loads:?}",
            living(&list)
        ));
        list
    }

    /// What a bot of the group `name` waits for an acknowledgement when nothing
    /// happens: the longest it waited over ten rounds of everybody being served, in
    /// which the test did nothing and the coordinator had nothing to do. Returns what
    /// a wait is held to by it.
    async fn calm(&mut self, name: &str) -> Duration {
        self.served().await;
        let quiet = Instant::now();
        for _ in 0..10 {
            self.served().await;
        }
        let longest = self.longest_wait_of(name, quiet, Instant::now());
        let calm = longest.unwrap_or_default();
        self.note(format!(
            "undisturbed, a bot of {name} waits {} at most for an acknowledgement",
            seconds(calm)
        ));
        (3 * calm).max(NOTHING_HAPPENS)
    }

    /// How often each bot of the group `name` was kept waiting for longer than
    /// `above` between `from` and `to`, bot by bot: when each such stop began, by the
    /// test's clock, and the longest wait of it. Waits that are less than half a rest
    /// apart are of one stop: a region stops twice in the fraction of a second a
    /// merge or a split takes, for its checkpoint and for the change itself, and
    /// nothing is begun with a region twice within a rest.
    fn stops_of(
        &self,
        name: &str,
        from: Instant,
        to: Instant,
        above: Duration,
    ) -> Vec<Vec<(Duration, Duration)>> {
        let now = Instant::now();
        let mut all = Vec::new();
        for waits in self.group(name).progress.waits() {
            let mut stops: Vec<(Duration, Duration)> = Vec::new();
            let mut until: Option<Instant> = None;
            for wait in waits {
                let over = wait.acknowledged.unwrap_or(now);
                if wait.sent > to || over < from || wait.lasted(now) <= above {
                    continue;
                }
                match stops.last_mut() {
                    Some(stop) if until.is_some_and(|until| wait.sent < until + REST / 2) => {
                        stop.1 = stop.1.max(wait.lasted(now));
                    }
                    _ => stops.push((
                        wait.sent.saturating_duration_since(self.started),
                        wait.lasted(now),
                    )),
                }
                until = Some(until.map_or(over, |until| until.max(over)));
            }
            all.push(stops);
        }
        all
    }

    /// Fails if a bot of `A` was kept waiting longer than `above` more than once
    /// since `since`, or longer than a player may wait at all: no bot of `A` waits
    /// longer than it waits when nothing happens, but once, at the split.
    ///
    /// One thing is left out that the record does not name: a stop of `A` for a move
    /// of region 0 itself. Where more go than stay, as the eight of W11 do, the part
    /// has more players than region 0, and the coordinator evens the workers out by
    /// moving the region with the fewest (ADR-0016, section 6): region 0, a rest
    /// after the split, with `A` in it.
    fn a_stood_still_once_at_most(&mut self, since: Instant, above: Duration) {
        let mut stops = self.stops_of("A", since, Instant::now(), above);
        let begun = self.begun();
        let moves = begun
            .iter()
            .filter(|(_, begun)| matches!(begun, Begun::Move { region: 0, .. }));
        let moved: Vec<(Duration, Duration)> = moves
            .map(|(at, begun)| {
                let end = self.end_of(*at, begun).map_or(*at, |(end, _)| end);
                let from = self.instant_of(*at - 0.5);
                let to = self.instant_of(end + 2.0);
                (
                    from.saturating_duration_since(self.started),
                    to.saturating_duration_since(self.started),
                )
            })
            .collect();
        let by_a_move = |stop: &(Duration, Duration)| {
            moved
                .iter()
                .any(|(from, to)| *from <= stop.0 && stop.0 <= *to)
        };
        let mut for_a_move = 0;
        for stops in &mut stops {
            for_a_move += stops.iter().filter(|stop| by_a_move(stop)).count();
            stops.retain(|stop| !by_a_move(stop));
        }
        self.note(format!(
            "the bots of A were kept waiting longer than {}, as when and for how long: \
             {stops:?}; and {for_a_move} times for a move of region 0",
            seconds(above)
        ));
        let too_long = |stop: &(Duration, Duration)| stop.1 > LONGEST_PAUSE;
        if stops
            .iter()
            .any(|stops| stops.len() > 1 || stops.iter().any(too_long))
        {
            self.fail(&format!(
                "no bot of A was to wait longer than it waits when nothing happens ({}), but \
                 once, at the split; they did, as when and for how long: {stops:?}",
                seconds(above)
            ));
        }
    }

    /// Section 3.6, by every reading of the list since `from`: nothing of region 0
    /// lies ahead of those who went east with `part`. Region 0's `bounds` never reach
    /// further east than at the reading before, and, with `ends_west`, they end west
    /// of where the part's begin.
    fn the_line_holds(&mut self, part: Region, from: Instant, ends_west: bool) {
        let mut east: Option<i32> = None;
        let mut fault = None;
        let mut read = 0;
        for (at, list) in self.readings.iter().filter(|(at, _)| *at >= from) {
            let (Some(home), Some(theirs)) = (bounds(list, 0), bounds(list, part)) else {
                continue;
            };
            read += 1;
            let since = at.saturating_duration_since(self.started);
            if ends_west && home.max.x >= theirs.min.x {
                fault = Some(format!(
                    "{}: region 0 reaches east to x = {} and region {part} begins at x = {}",
                    seconds(since),
                    home.max.x,
                    theirs.min.x
                ));
            }
            if east.is_some_and(|east| home.max.x > east) {
                fault = Some(format!(
                    "{}: the east end of region 0 moved east, from x = {east:?} to x = {}",
                    seconds(since),
                    home.max.x
                ));
            }
            east = Some(home.max.x);
            if fault.is_some() {
                break;
            }
        }
        if let Some(fault) = fault {
            self.fail(&format!(
                "nothing of region 0 was to lie ahead of those who went (section 3.6); {fault}"
            ));
        }
        self.note(format!(
            "in {read} readings of the list region 0 had nothing ahead of region {part}"
        ));
    }

    /// W1, step 1. `B` is sent to chunk 19, beyond the split distance from `A` and
    /// from the chunk players enter in. Exactly one split is begun, of region 0, with
    /// one group, and not before `B` has been in chunk 19; it ends with the id the
    /// list named as its next. The part is pinned to nothing and holds what `B` sees
    /// and the near end of its trail; region 0 holds nothing east of chunk 9. In a
    /// cluster the part is moved to the other worker once, a rest or more after the
    /// split ended, and region 0 is not. Returns the part.
    async fn b_is_split_off(&mut self) -> Region {
        let what = format!("to chunk {OUT}");
        self.b_is_split_off_between(within(OUT), &what).await
    }

    /// The same for a `B` that is sent to walk up and down `between` two x
    /// coordinates whose east end is in chunk 19. Of where the part's land ends in
    /// the east and begins in the west the record says what it says for a `B` that
    /// stands in chunk 19, and only that `B` is held to it.
    async fn b_is_split_off_between(&mut self, between: (f64, f64), what: &str) -> Region {
        let stands = between == within(OUT);
        let before = self.list().await;
        let part = before.next.0;
        let begun_before = self.reshapes().len();
        let moves_before = self.begun().len() - begun_before;
        let made_by = self.a_cluster().and_then(|cluster| cluster.owner(0));
        self.walks("B", between, what);
        let entered = self.passes("B", OUT, true).await;
        let entered = self.at_of(entered);
        let first = self
            .until_the_list("there is a new region", |list| list.next.0 != part)
            .await;
        let list = self.whole().await;
        self.arrives("B").await;
        for list in [&first, &list] {
            let (home, theirs) = (bounds(list, 0), bounds(list, part));
            // A `B` that walks up and down is in chunk 18 or 19 when it goes, and
            // sees chunk 18 from wherever it walks.
            let near = if stands { OUT } else { OUT - 1 };
            let as_told = living(list) == [0, part]
                && list.next.0 == part + 1
                && info(list, part).is_some_and(|info| info.pinned.is_empty())
                && has_chunk(theirs, near, 0)
                && theirs.is_some_and(|theirs| {
                    !stands || (theirs.min.x >= MERGE_DISTANCE && theirs.max.x <= OUT + REACH)
                })
                && has_chunk(home, 0, 0)
                && home.is_some_and(|home| home.max.x < MERGE_DISTANCE);
            if !as_told {
                self.fail(&format!(
                    "after the split the list was to have region 0, with chunk 0 and nothing \
                     east of chunk {}, and region {part}, pinned to nothing, with chunk {near} \
                     and, if B stands, nothing west of chunk {MERGE_DISTANCE} or east of chunk \
                     {}: {list:?}",
                    MERGE_DISTANCE - 1,
                    OUT + REACH
                ));
            }
        }
        if !self.has_logs() {
            return part;
        }

        let split = |begun: &Begun| {
            matches!(
                begun,
                Begun::Split {
                    region: 0,
                    groups: 1,
                    ..
                }
            )
        };
        let what = "one split, of region 0, with one group";
        self.has_begun(begun_before, what, &[&split]);
        let (split_at, begun) = self.reshapes_at().pop().expect("a split was begun");
        if split_at + WRITING < entered {
            self.fail(&format!(
                "the split was begun {:.3} s before B had been in chunk {OUT}",
                entered - split_at
            ));
        }
        let end = self.end_of(split_at, &begun);
        let Some((ended_at, _)) = end.filter(|(_, ended)| ended.part() == Some(part)) else {
            self.fail(&format!(
                "the split of region 0 was to end with region {part}"
            ));
        };

        // In a cluster the part is made by the worker of region 0, which then runs
        // two regions and the other none.
        let Some(made_by) = made_by else {
            return part;
        };
        self.until("the new region is run by the other worker", |wanders| {
            let cluster = wanders.a_cluster().expect("it is a cluster");
            let owner = cluster.owner(part);
            owner.is_some_and(|owner| owner != made_by) && cluster.runs(part)
        })
        .await;
        self.whole().await;
        self.is_between("B", between);
        let begun = self.begun();
        let moves: Vec<&(f64, Begun)> = begun
            .iter()
            .filter(|(_, begun)| !begun.reshapes())
            .collect();
        let new = moves.get(moves_before..).unwrap_or_default();
        let the_move = Begun::Move {
            region: part,
            from: worker_name(made_by),
            to: worker_name(1 - made_by),
        };
        let owner_of_0 = self.a_cluster().and_then(|cluster| cluster.owner(0));
        let [(moved_at, moved)] = new else {
            self.fail(&format!(
                "the coordinator was to move region {part} to the other worker once, and \
                 nothing else: it moved {new:?}"
            ));
        };
        if *moved != the_move || owner_of_0 != Some(made_by) {
            self.fail(&format!(
                "the coordinator was to move region {part} to the other worker, and not \
                 region 0: it began {moved:?}, and region 0 is run by {owner_of_0:?}"
            ));
        }
        let rested = moved_at - ended_at;
        self.note(format!(
            "the release of region {part} was begun {rested:.3} s after the split ended"
        ));
        if rested + WRITING < REST.as_secs_f64() {
            self.fail(&format!(
                "region {part} was released to even out {rested:.3} s after the split that made \
                 it ended; it was to be left alone for a rest of {REST:?}"
            ));
        }
        part
    }

    /// `B` has arrived in chunk 40, in the part. It walks up and down there for
    /// 35 s, and then the part holds exactly the 7 by 7 chunks around `B` and region
    /// 0 those around the chunk players enter in.
    ///
    /// The 35 s are bounded as `follows.rs` bounds such things, by the rounds `B`
    /// walks within its chunk: as many as are 35 s of its steps. A bot takes a step
    /// every client tick whatever the server does, and on a machine that is slow the
    /// bots are slow with it. Should the part have begun to run on a worker after
    /// `B` arrived, the 35 s are walked once more, as a restore begins the thirty
    /// seconds anew.
    async fn b_stands_far_out(&mut self, part: Region) {
        let mut arrived = self.now_at();
        for _ in 0..3 {
            self.walks_for("B", GIVEN_BACK).await;
            match self.last_ran(part).filter(|ran| *ran > arrived) {
                Some(ran) => arrived = ran,
                None => break,
            }
        }
        let list = self.whole().await;
        let around = |chunk: i32| (chunk - REACH, chunk + REACH);
        let theirs = is_box(bounds(&list, part), around(FAR), around(0));
        let home = is_box(bounds(&list, 0), around(0), around(0));
        if living(&list) != [0, part] || !theirs || !home {
            self.fail(&format!(
                "35 s after B arrived in chunk {FAR}, region {part} was to hold exactly the \
                 chunks x = {} to {} and z = -{REACH} to {REACH}, and region 0 those within \
                 {REACH} of chunk 0: {list:?}",
                FAR - REACH,
                FAR + REACH
            ));
        }
    }

    /// W1, step 2. `B` is sent to chunk 40. Nothing is begun, by the distances or for
    /// an empty region, from when `B` is sent until 35 s after it has arrived; then
    /// the part holds exactly the 7 by 7 chunks around `B` and region 0 those around
    /// the chunk players enter in. At every reading of the list in between, region
    /// 0's `bounds` end west of where the part's begin and their east end never moves
    /// east.
    async fn b_walks_on(&mut self, part: Region) {
        let begun_before = self.reshapes().len();
        let sent = self.walks_to("B", FAR);
        self.arrives("B").await;
        self.b_stands_far_out(part).await;
        self.has_begun(begun_before, "nothing while B walked on", &[]);
        self.the_line_holds(part, sent, true);
    }

    /// W1, step 3. `B` is sent to chunk 8. Exactly one merge is begun, by the
    /// distances, of the part into region 0 with a gap of 10 or less, and not before
    /// `B` has been in chunk 10; it ends well, and the list has region 0 alone.
    async fn b_comes_back(&mut self, part: Region) {
        let what = format!("to chunk {BACK}");
        self.b_comes_back_between(part, within(BACK), &what).await;
    }

    /// The same for a `B` that is sent to walk up and down `between` two x
    /// coordinates of which the west one is within the merge distance.
    async fn b_comes_back_between(&mut self, part: Region, between: (f64, f64), what: &str) {
        let begun_before = self.reshapes().len();
        self.walks("B", between, what);
        let entered = self.passes("B", MERGE_DISTANCE, false).await;
        let entered = self.at_of(entered);
        let absorbed = format!("region {part} is absorbed by region 0");
        self.until_the_list(&absorbed, |list| absorbed_by(list, part) == Some(0))
            .await;
        let list = self.whole().await;
        self.arrives("B").await;
        if living(&list) != [0] || list.next.0 != part + 1 {
            self.fail(&format!(
                "after the merge the list was to have region 0 alone and {} as its next \
                 region: {list:?}",
                part + 1
            ));
        }
        if !self.has_logs() {
            return;
        }
        let merge = |begun: &Begun| {
            matches!(
                begun,
                Begun::Merge { survivor: 0, absorbed, gap }
                    if *absorbed == part && *gap <= MERGE_DISTANCE as u32
            )
        };
        let what = format!("one merge, of region {part} into region 0");
        self.has_begun(begun_before, &what, &[&merge]);
        let (merge_at, begun) = self.reshapes_at().pop().expect("a merge was begun");
        if merge_at + WRITING < entered {
            self.fail(&format!(
                "the merge was begun {:.3} s before B had been in chunk {MERGE_DISTANCE}",
                entered - merge_at
            ));
        }
        self.note(format!(
            "the merge was begun {:.3} s after B was seen in chunk {MERGE_DISTANCE}",
            merge_at - entered
        ));
        let end = self.end_of(merge_at, &begun);
        if !end.is_some_and(|(_, ended)| ended.well()) {
            self.fail(&format!(
                "the merge of region {part} into region 0 was to end well"
            ));
        }
    }

    /// Fails unless the workers have said that region 0 stood still once for each of
    /// the splits and merges of it that ended well from the time `from` of the logs
    /// on, in their order; and, with `players`, with so many players at a split and
    /// so many at a merge: those region 0 had when it stopped.
    fn region_0_stood_still_once_for_each(&mut self, from: f64, players: Option<(u32, u32)>) {
        let begun = self.reshapes_at();
        let well: Vec<Begun> = begun
            .into_iter()
            .filter(|(at, begun)| {
                let end = self.end_of(*at, begun);
                *at >= from && begun.is_of(0) && end.is_some_and(|(_, ended)| ended.well())
            })
            .map(|(_, begun)| begun)
            .collect();
        let stood = self.stood().into_iter();
        let stood: Vec<_> = stood
            .filter(|stood| stood.at >= from && stood.region == 0)
            .collect();
        let as_told = stood.len() == well.len()
            && stood.iter().zip(&well).all(|(stood, begun)| {
                let split = matches!(begun, Begun::Split { .. });
                players.is_none_or(|(at_a_split, at_a_merge)| {
                    stood.players == if split { at_a_split } else { at_a_merge }
                })
            });
        if !as_told {
            self.fail(&format!(
                "the workers were to say `a region stood still for a merge or a split \
                 region=0` once for each of {well:?}, with the players {players:?} at a split \
                 and at a merge; they said {stood:?}"
            ));
        }
    }
}

/// Somebody who joined in the middle of W4: how long it took until they were placed
/// in the world, where, and what went wrong for them, if anything did.
struct Visit {
    name: String,
    placed_after: Duration,
    placed_at: (f64, f64),
    fault: Option<String>,
}

/// W4's wanderers: until `over` is said, somebody joins every three seconds, places a
/// block beside where players enter, where no ledger builds, sees it acknowledged
/// and shown, breaks it, sees that, and leaves, as the guests of `chaos.rs` do.
///
/// The three seconds are how often they come, and not a wait for anything to come
/// about.
fn visitors(address: String, over: Arc<AtomicBool>) -> JoinHandle<Vec<Visit>> {
    tokio::spawn(async move {
        let air = Some(i32::from(blocks::AIR.0));
        let mut visits = Vec::new();
        while !over.load(Ordering::Relaxed) {
            let name = format!("Visitor{}", visits.len());
            let joining = Instant::now();
            let mut visit = Visit {
                name: name.clone(),
                placed_after: Duration::ZERO,
                placed_at: (f64::NAN, f64::NAN),
                fault: None,
            };
            let played = async {
                // A bot has joined when the server has placed it.
                let mut visitor = Bot::join(&address, &name).await?;
                visit.placed_after = joining.elapsed();
                visit.placed_at = (visitor.location.0, visitor.location.2);
                visitor.wait_for_chunks(1, PATIENCE).await?;
                let placed = visitor.use_item_on(2, -61, 3, face::TOP).await?;
                visitor
                    .wait_until(PATIENCE, |bot| {
                        bot.acknowledged_sequence >= placed
                            && bot
                                .block_at(2, -60, 3)
                                .is_ok_and(|block| block.is_some() && block != air)
                    })
                    .await?;
                let broken = visitor.dig(2, -60, 3).await?;
                visitor
                    .wait_until(PATIENCE, |bot| {
                        bot.acknowledged_sequence >= broken
                            && bot.block_at(2, -60, 3).is_ok_and(|block| block == air)
                    })
                    .await
            };
            if let Err(error) = played.await {
                visit.fault = Some(format!("{error:#}"));
            }
            visits.push(visit);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        visits
    })
}

/// W1 on whatever serves the world: `A` and `B` settle in the chunk players enter
/// in, and then, in rounds, `B` is split off at chunk 19, walks on to chunk 40 and
/// comes back to chunk 8, where it is merged into region 0 again. With `visited`,
/// which is W4, somebody joins every three seconds all the while, builds beside
/// where players enter and leaves.
///
/// One thing is done here that the record does not say: at the end of a round `B`
/// walks up and down in chunk 8 for 35 s before it is sent out again. The merge
/// leaves region 0 with the trail `B` came back along, which reaches east of chunk
/// 22 and is region 0's for thirty seconds more (section 3.2); a `B` that is split
/// off before that takes what is left of the trail along, and its part does not "end
/// at x = 22 or west of it" as step 1 says. With the wait every round begins as the
/// first does.
async fn apart_on_and_back(mut wanders: Wanders, visited: bool) {
    wanders.a_settles().await;
    wanders.b_settles().await;
    wanders.region_0_is_alone("at the start").await;
    wanders.has_begun(0, "nothing", &[]);
    let from = wanders.now_at();
    let counting = Instant::now();
    let over = Arc::new(AtomicBool::new(false));
    let visiting = visited.then(|| visitors(wanders.address.clone(), over.clone()));
    let stand = [
        Stands {
            group: "A",
            first: 0,
            goes: false,
        },
        Stands {
            group: "B",
            first: 0,
            goes: true,
        },
    ];
    let rounds = rounds(3);
    for round in 1..=rounds {
        let part = wanders.b_is_split_off().await;
        wanders.b_walks_on(part).await;
        wanders.b_comes_back(part).await;
        let list = wanders.list().await;
        let pairs = &list.absorbed;
        if !pairs.iter().any(|pair| (pair.0.0, pair.1.0) == (part, 0)) {
            wanders.fail(&format!(
                "region {part} was to be among the absorbed, as region 0's: {list:?}"
            ));
        }
        wanders.walks_for("B", GIVEN_BACK).await;
        let list = wanders.region_0_is_alone("after a round").await;
        if !is_box(bounds(&list, 0), (-REACH, BACK + REACH), (-REACH, REACH)) {
            wanders.fail(&format!(
                "35 s after B came back to chunk {BACK}, region 0 was to hold exactly what A \
                 and B see: {list:?}"
            ));
        }
        wanders.note(format!("round {round} of {rounds} is done"));
    }
    wanders.is_in("A", 0);
    wanders.is_in("B", BACK);

    // W4: every one of those who joined in the middle was placed, where players
    // enter, within 5 s, and none was disconnected.
    over.store(true, Ordering::Relaxed);
    if let Some(visiting) = visiting {
        wanders
            .until("the last visitor has left", |_| visiting.is_finished())
            .await;
        let visits = visiting.await.expect("visiting does not panic");
        let slowest = visits.iter().map(|visit| visit.placed_after).max();
        wanders.note(format!(
            "{} visitors joined, built and left; the slowest was placed after {slowest:?}",
            visits.len()
        ));
        let amiss = visits.iter().find(|visit| {
            visit.fault.is_some()
                || visit.placed_after > LONGEST_PAUSE
                || visit.placed_at != (0.5, 0.5)
        });
        if let Some(visit) = amiss {
            wanders.fail(&format!(
                "{} was to be placed where players enter within {LONGEST_PAUSE:?} and to \
                 build and leave: it was placed at {:?} after {:?}, and {:?}",
                visit.name, visit.placed_at, visit.placed_after, visit.fault
            ));
        }
        if visits.is_empty() {
            wanders.fail("nobody joined in the middle");
        }
    }

    // Over all rounds: nobody ever saw another region's land, so nobody was handed
    // over; nobody was stood still more than once in a rest, but `B` by the one move
    // after a split; and no wait was longer than a player may wait.
    wanders.nobody_waited_too_long(counting);
    if wanders.has_logs() {
        wanders.nobody_was_handed_over(from, "in any round");
        wanders.nobody_is_stood_still_more_than_once_in_a_rest(from, &stand);
        // Those region 0 had when it stopped: `A` and `B` at a split, `A` at a merge,
        // and whoever was visiting.
        let exactly = (!visited).then_some((3, 2));
        wanders.region_0_stood_still_once_for_each(from, exactly);
        let measured = wanders.measured(from, &stand);
        wanders.prints(&measured);
    }
    wanders.finish().await;
}

/// W1. Apart, on, and back, in a cluster.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_is_split_off_walks_on_and_is_merged_when_it_comes_back() {
    if a_repetition() {
        return;
    }
    let wanders = Wanders::cluster("apart, on and back", world(None)).await;
    apart_on_and_back(wanders, false).await;
}

/// W1 in the single process, without the move: only what the list and the bots show
/// is asserted, and the end is the server stopped and started from its disk.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_is_split_off_walks_on_and_is_merged_when_it_comes_back_in_one_process() {
    if a_repetition() {
        return;
    }
    let wanders = Wanders::server("apart, on and back, in one process", world(None)).await;
    apart_on_and_back(wanders, false).await;
}

/// W4. Joining in the middle: W1's rounds, and all the while somebody joins every
/// three seconds, builds beside where players enter and leaves. Every one of them is
/// placed, where players enter, within 5 s, and none is disconnected.
#[tokio::test(flavor = "multi_thread")]
async fn players_join_at_the_spawn_point_while_a_group_is_split_off_and_merged() {
    if a_repetition() {
        return;
    }
    let wanders = Wanders::cluster("joining in the middle", world(None)).await;
    apart_on_and_back(wanders, true).await;
}

/// W2 on whatever serves the world. `A` stands in chunk 0. `B` walks between the
/// chunks 9 and 11 for a minute: nothing is begun, as they are one region. `B` is
/// sent to chunk 19 and is split off. `B` walks between the chunks 9 and 11 for a
/// minute: one merge, when `B` has been within 10 chunks, and nothing after it. `B`
/// walks between the chunks 17 and 19 for a minute: one split, in chunk 19, and
/// nothing after it. In all: a split, a merge, a split, in that order, and the moves
/// of parts.
///
/// "For a minute" is bounded by the rounds `B` walks, as many as are a minute of its
/// steps; each takes it through every distance its walk has.
async fn along_the_rim(mut wanders: Wanders) {
    let a_minute = Duration::from_secs(60);
    wanders.a_settles().await;
    wanders.b_settles().await;
    wanders.region_0_is_alone("at the start").await;

    wanders.walks("B", between(9, 11), "to walk between the chunks 9 and 11");
    wanders.arrives("B").await;
    wanders.walks_for("B", a_minute).await;
    let alone = "while B walked along the rim in it";
    let list = wanders.region_0_is_alone(alone).await;
    wanders.has_begun(0, "nothing", &[]);
    if list.next.0 != 1 {
        wanders.fail(&format!("no region was to be made yet: {list:?}"));
    }

    let part = wanders.b_is_split_off().await;
    // One thing more than the record says: `B` stands for 35 s before it walks back,
    // until region 0 has given back what it kept of the trail `B` came along. A `B`
    // that turned round at once would see that land from chunk 12 on and walk into
    // it in chunk 9, and whether the merge is made before that is a matter of
    // tenths of a second: if not, `B` is region 0's player by a hand-over, and there
    // is no merge (N3).
    wanders.walks_for("B", GIVEN_BACK).await;
    let back = "to walk between the chunks 9 and 11";
    wanders
        .b_comes_back_between(part, between(9, 11), back)
        .await;
    wanders.walks_for("B", a_minute).await;
    wanders.region_0_is_alone("after the merge").await;

    let again = "to walk between the chunks 17 and 19";
    let second = wanders.b_is_split_off_between(between(17, 19), again).await;
    wanders.walks_for("B", a_minute).await;
    let list = wanders.whole().await;
    if living(&list) != [0, second] || list.next.0 != second + 1 {
        wanders.fail(&format!(
            "the list was to have region 0 and region {second} and no region more: {list:?}"
        ));
    }
    let split = |begun: &Begun| matches!(begun, Begun::Split { region: 0, .. });
    let merge = |begun: &Begun| matches!(begun, Begun::Merge { survivor: 0, absorbed, .. } if *absorbed == part);
    let what = "a split, a merge and a split, in that order";
    wanders.has_begun(0, what, &[&split, &merge, &split]);
    wanders.is_in("A", 0);
    wanders.is_between("B", between(17, 19));
    wanders.finish().await;
}

/// W2. Along the rim, in a cluster.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_that_walks_along_the_rim_is_split_and_merged_once_each_time_it_crosses() {
    if a_repetition() {
        return;
    }
    along_the_rim(Wanders::cluster("along the rim", world(None)).await).await;
}

/// W2 in the single process, by what the list and the bots show.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_that_walks_along_the_rim_is_split_and_merged_once_each_time_in_one_process() {
    if a_repetition() {
        return;
    }
    along_the_rim(Wanders::server("along the rim, in one process", world(None)).await).await;
}

/// W3 on whatever serves the world. `A` stands in chunk 0. A wanderer walks to chunk
/// (19, 0), is split off into region 1 and leaves the game. No absorption is begun,
/// and the list keeps regions 0 and 1; region 1 holds nothing 35 s after it last
/// began to run, which in a cluster is when the worker it was moved to restored it,
/// and no later than 50 s after the split. A second wanderer walks to chunk (-19,
/// 0), is split off into region 2 and leaves. Fifteen seconds and no more than
/// thirty later region 1 absorbs region 2, and no bot of `A` waits longer for it
/// than it waits when nothing happens. Then `A` ends and leaves: region 0 absorbs
/// region 1, and the list has region 0 alone.
///
/// The times of this scenario are the record's and are held to the clock: what is
/// waited for is the state, and the clock says whether it came when the record says.
async fn regions_that_are_left(mut wanders: Wanders) {
    wanders.a_settles().await;
    let list = wanders.region_0_is_alone("at the start").await;
    if list.next.0 != 1 {
        wanders.fail(&format!(
            "a new world's first part was to be region 1: {list:?}"
        ));
    }
    let above = wanders.calm("A").await;
    let made_by = wanders.a_cluster().and_then(|cluster| cluster.owner(0));

    let first = wanders.wanders("Rover1").await;
    let (x, z) = east_along_the_row(OUT);
    wanders.wanderers[first].walks_to(x, z, ON_FOOT);
    wanders.wanderer_arrives(first, x, ON_FOOT).await;
    wanders
        .until_the_list("the wanderer is split off into region 1", |list| {
            living(list) == [0, 1]
        })
        .await;
    let split = Instant::now();
    wanders.wanderer_leaves(first).await;
    let begun_before = wanders.reshapes().len();

    // In a cluster region 1, which has nobody, is moved to the other worker when it
    // has rested, and that restore begins its thirty seconds anew.
    let mut ran = None;
    if let Some(made_by) = made_by {
        wanders
            .until("region 1 is run by the other worker", |wanders| {
                let cluster = wanders.a_cluster().expect("it is a cluster");
                cluster.owner(1).is_some_and(|owner| owner != made_by) && cluster.runs(1)
            })
            .await;
        ran = wanders.last_ran(1);
    }
    let fifty = Duration::from_secs(50);
    wanders
        .until_the_list_within(fifty, "region 1 holds nothing", |list| {
            living(list) == [0, 1] && bounds(list, 1).is_none()
        })
        .await;
    let emptied = wanders.now_at();
    let since_the_split = split.elapsed();
    let since = match ran {
        Some(ran) => emptied - ran,
        None => since_the_split.as_secs_f64(),
    };
    wanders.note(format!(
        "region 1 held nothing {} after it was split off, and {since:.3} s after it last \
         began to run",
        seconds(since_the_split)
    ));
    if since > GIVEN_BACK.as_secs_f64() + WRITING || since_the_split > fifty {
        wanders.fail(&format!(
            "region 1 was to hold nothing 35 s after it last began to run, and no later than \
             50 s after the split: it took {since:.3} s, and {} since the split",
            seconds(since_the_split)
        ));
    }
    // That took thirty seconds and more, in which the list kept regions 0 and 1 at
    // every reading and nothing was begun.
    let kept = wanders.readings.iter().filter(|(at, _)| *at >= split);
    let mut kept = kept.map(|(_, list)| living(list));
    if let Some(other) = kept.find(|living| living != &[0, 1]) {
        wanders.fail(&format!(
            "the list was to keep regions 0 and 1 while region 1 was empty; it had {other:?}"
        ));
    }
    wanders.has_begun(begun_before, "no absorption while region 1 was empty", &[]);

    let second = wanders.wanders("Rover2").await;
    let (x, z) = middle_of(-OUT, 0);
    wanders.wanderers[second].walks_to(x, z, ON_FOOT);
    wanders.wanderer_arrives(second, -x, ON_FOOT).await;
    wanders
        .until_the_list("the wanderer is split off into region 2", |list| {
            living(list) == [0, 1, 2]
        })
        .await;
    wanders.wanderer_leaves(second).await;
    let left = Instant::now();
    let begun_before = wanders.reshapes().len();
    let thirty = Duration::from_secs(30);
    wanders
        .until_the_list_within(thirty + MOMENT, "region 1 absorbs region 2", |list| {
            absorbed_by(list, 2) == Some(1)
        })
        .await;
    // By the coordinator's line where there is one, and else by when the list
    // showed it, which is a fraction of a second later.
    let (mut begun_at, mut ended_at) = (Instant::now(), Instant::now());
    if wanders.has_logs() {
        let absorption = |begun: &Begun| {
            *begun
                == Begun::Absorption {
                    survivor: 1,
                    absorbed: 2,
                }
        };
        let what = "one absorption, of region 2 by region 1";
        wanders.has_begun(begun_before, what, &[&absorption]);
        let (at, begun) = wanders.reshapes_at().pop().expect("it was begun");
        begun_at = wanders.instant_of(at);
        if let Some((ended, _)) = wanders.end_of(at, &begun) {
            ended_at = wanders.instant_of(ended);
        }
    }
    let after = begun_at.saturating_duration_since(left);
    wanders.note(format!(
        "region 1 absorbed region 2 {} after the second wanderer left",
        seconds(after)
    ));
    let early = after + Duration::from_secs_f64(WRITING) < EMPTY_FOR && wanders.has_logs();
    if early || after > thirty {
        wanders.fail(&format!(
            "region 1 was to absorb region 2 fifteen seconds and no more than thirty after \
             the second wanderer left; it was {}",
            seconds(after)
        ));
    }
    wanders.served().await;
    let from = begun_at.min(ended_at) - Duration::from_millis(500);
    let waited = wanders.longest_wait_of("A", from, Instant::now());
    wanders.note(format!(
        "for the absorption a bot of A waited {} at most",
        waited.map_or("-".to_owned(), seconds)
    ));
    if waited.is_some_and(|waited| waited > above) {
        wanders.fail(&format!(
            "no bot of A was to wait longer for the absorption than it waits when nothing \
             happens ({}); one waited {waited:?}",
            seconds(above)
        ));
    }
    let list = wanders.whole().await;
    if living(&list) != [0, 1] {
        wanders.fail(&format!(
            "the list was to have regions 0 and 1 after the absorption: {list:?}"
        ));
    }

    // `A` ends, and its auditor comes and goes; then nobody is in region 0, and it
    // takes region 1.
    wanders.leaves("A").await;
    let list = wanders
        .until_the_list_within(AGAIN, "region 0 absorbs region 1", |list| {
            absorbed_by(list, 1) == Some(0) && living(list) == [0]
        })
        .await;
    wanders.note(format!("the list has region 0 alone: {list:?}"));
    if wanders.has_logs() {
        let last = wanders.reshapes().pop();
        let home = Begun::Absorption {
            survivor: 0,
            absorbed: 1,
        };
        if last != Some(home) {
            wanders.fail(&format!(
                "region 0 was to absorb region 1 for being empty; the last thing begun was \
                 {last:?}"
            ));
        }
    }
    wanders.finish().await;
}

/// W3. Regions that are left, in a cluster.
#[tokio::test(flavor = "multi_thread")]
async fn regions_that_their_players_left_give_their_land_back_and_are_absorbed() {
    if a_repetition() {
        return;
    }
    regions_that_are_left(Wanders::cluster("regions that are left", world(None)).await).await;
}

/// W3 in the single process, by what the list and the bots show.
#[tokio::test(flavor = "multi_thread")]
async fn regions_that_their_players_left_give_their_land_back_and_are_absorbed_in_one_process() {
    if a_repetition() {
        return;
    }
    let wanders = Wanders::server("regions that are left, in one process", world(None)).await;
    regions_that_are_left(wanders).await;
}

/// W3b. Two parts meet. One wanderer walks to chunk (19, 0) and is region 1; another
/// walks to (0, 19) and is region 2, then to (19, 19), then towards (19, 0), as far
/// as (19, 8). When it has been within 10 chunks of the first, the two regions are
/// merged with region 1 as the survivor, which has as many players and the lower id,
/// and region 0 is in no merge.
#[tokio::test(flavor = "multi_thread")]
async fn two_parts_whose_players_meet_are_merged_and_the_home_region_is_in_no_merge() {
    if a_repetition() {
        return;
    }
    let mut wanders = Wanders::cluster("two parts meet", world(None)).await;
    wanders.region_0_is_alone("at the start").await;
    let from = wanders.now_at();

    let first = wanders.wanders("Rover1").await;
    let (x, z) = east_along_the_row(OUT);
    wanders.wanderers[first].walks_to(x, z, ON_FOOT);
    wanders.wanderer_arrives(first, x, ON_FOOT).await;
    wanders
        .until_the_list("the first wanderer is region 1", |list| {
            living(list) == [0, 1] && has_chunk(bounds(list, 1), OUT, 0)
        })
        .await;

    let second = wanders.wanders("Rover2").await;
    let (x, z) = middle_of(0, OUT);
    wanders.wanderers[second].walks_to(x, z, ON_FOOT);
    wanders.wanderer_arrives(second, z, ON_FOOT).await;
    wanders
        .until_the_list("the second wanderer is region 2", |list| {
            living(list) == [0, 1, 2] && has_chunk(bounds(list, 2), 0, OUT)
        })
        .await;
    wanders.everything_runs().await;

    let (x, z) = middle_of(OUT, OUT);
    wanders.wanderers[second].walks_to(x, z, ON_FOOT);
    wanders.wanderer_arrives(second, x, ON_FOOT).await;
    let split = |begun: &Begun| matches!(begun, Begun::Split { region: 0, .. });
    let what = "two splits of region 0, one for each wanderer";
    wanders.has_begun(0, what, &[&split, &split]);

    let (x, z) = middle_of(OUT, BACK);
    wanders.wanderers[second].walks_to(x, z, ON_FOOT);
    let within_ten = f64::from(16 * (MERGE_DISTANCE + 1));
    wanders
        .until("the second wanderer has been within 10 chunks", |wanders| {
            wanders.wanderers[second].noted().z < within_ten
        })
        .await;
    let entered = wanders.now_at();
    wanders
        .until_the_list("region 1 absorbs region 2", |list| {
            absorbed_by(list, 2) == Some(1)
        })
        .await;
    wanders.wanderer_arrives(second, x, ON_FOOT).await;
    let list = wanders.everything_runs().await;
    if living(&list) != [0, 1] {
        wanders.fail(&format!(
            "the list was to have regions 0 and 1 after the two parts met: {list:?}"
        ));
    }
    let merge = |begun: &Begun| {
        matches!(
            begun,
            Begun::Merge { survivor: 1, absorbed: 2, gap } if *gap <= MERGE_DISTANCE as u32
        )
    };
    let what = "two splits of region 0 and one merge, of region 2 into region 1";
    wanders.has_begun(0, what, &[&split, &split, &merge]);
    let (merge_at, _) = wanders.reshapes_at().pop().expect("a merge was begun");
    if merge_at + WRITING < entered {
        wanders.fail(&format!(
            "the merge was begun {:.3} s before the second wanderer had been within \
             {MERGE_DISTANCE} chunks of the first",
            entered - merge_at
        ));
    }
    let of_home = wanders.ended().into_iter().any(|(_, ended)| {
        matches!(ended, Ended::Merge { survivor, absorbed, .. } if survivor == 0 || absorbed == 0)
    });
    if of_home {
        wanders.fail("region 0 was to be in no merge");
    }
    // Neither of them ever saw the other's land: they were merged four chunks before.
    wanders.nobody_was_handed_over(from, "while two parts met");
    wanders.finish().await;
}

/// W5. A worker dies with a part that has just grown (N9). `B` is split off at chunk
/// 19 and its region moved. `B` is sent to chunk 40; when it has been in chunk 30,
/// the worker that runs its region is killed and not started again. The region runs
/// again, on the other worker, within twice the lease and 5 s; `B` arrives; nobody
/// is disconnected; the regions hold what W1 has them hold 35 s after `B` has
/// arrived; and the audit finds every block `B` was acknowledged.
#[tokio::test(flavor = "multi_thread")]
async fn a_part_that_has_just_grown_runs_on_another_worker_when_its_worker_dies() {
    if a_repetition() {
        return;
    }
    let mut wanders = Wanders::cluster("a worker dies", world(Some(3))).await;
    wanders.a_settles().await;
    wanders.b_settles().await;
    wanders.region_0_is_alone("at the start").await;
    let part = wanders.b_is_split_off().await;
    wanders.walks_to("B", FAR);
    wanders.passes("B", 30, true).await;
    let Some(owner) = wanders.processes().owner(part) else {
        wanders.fail(&format!("region {part} has no owner to kill"));
    };
    wanders.processes().kill_worker(owner).await;
    let killed = Instant::now();
    let whereabouts = wanders.whereabouts();
    wanders.note(format!(
        "killed {}, which ran region {part}; {whereabouts}",
        worker_name(owner)
    ));
    wanders.everything_runs().await;
    let took = killed.elapsed();
    let now = wanders.processes().owner(part);
    wanders.note(format!(
        "every region ran again {} after the kill, region {part} on {now:?}",
        seconds(took)
    ));
    let may = 2 * wanders.lease + MOMENT;
    if took > may || now == Some(owner) {
        wanders.fail(&format!(
            "region {part} was to run on the other worker within {may:?} of the kill: it took \
             {}, and its worker is {now:?}",
            seconds(took)
        ));
    }
    wanders.arrives("B").await;
    wanders.b_stands_far_out(part).await;
    wanders.finish().await;
}

/// W6. The store is away while a part grows (N10). As W5, but it is the store that
/// is killed, and started again three seconds later. Every region runs again; `B`
/// arrives; the same audit, and once more after the whole cluster is started from
/// disk.
#[tokio::test(flavor = "multi_thread")]
async fn a_part_grows_on_when_the_store_was_away_for_three_seconds() {
    if a_repetition() {
        return;
    }
    let mut wanders = Wanders::cluster("the store is away", world(Some(3))).await;
    wanders.a_settles().await;
    wanders.b_settles().await;
    wanders.region_0_is_alone("at the start").await;
    let part = wanders.b_is_split_off().await;
    wanders.walks_to("B", FAR);
    wanders.passes("B", 30, true).await;
    let cluster = wanders.processes();
    let mut store = cluster.store.1.take().expect("the store is running");
    store.kill().await.unwrap();
    // What the workers say about running a region has to be said anew from here.
    for worker in 0..cluster.workers.len() {
        let length = cluster.log(&worker_name(worker)).len();
        cluster.since.insert(worker, length);
    }
    let whereabouts = wanders.whereabouts();
    wanders.note(format!("killed the world store; {whereabouts}"));
    // How long the store is away is what is tried here, not a wait for anything to
    // come about.
    tokio::time::sleep(Duration::from_secs(3)).await;
    wanders.processes().start_store();
    wanders.note("started the world store".to_owned());
    wanders.whole().await;
    wanders.arrives("B").await;
    wanders.b_stands_far_out(part).await;
    wanders.finish().await;
}

/// W8. A coordinator anew, the store away, a worker dead (N13). `B` is split off at
/// chunk 19 and its region moved. The coordinator, the store and the worker of
/// `B`'s region are killed. The coordinator is started; three seconds later the
/// store. `B`'s region is in the routing table again, with a worker, within the
/// lease and 5 s of the store's start, and `B` is not disconnected.
#[tokio::test(flavor = "multi_thread")]
async fn a_part_whose_worker_died_is_given_out_by_a_new_coordinator_when_the_store_is_back() {
    if a_repetition() {
        return;
    }
    let mut wanders = Wanders::cluster("a coordinator anew", world(Some(3))).await;
    wanders.a_settles().await;
    wanders.b_settles().await;
    wanders.region_0_is_alone("at the start").await;
    let part = wanders.b_is_split_off().await;
    let Some(owner) = wanders.processes().owner(part) else {
        wanders.fail(&format!("region {part} has no owner to kill"));
    };
    let cluster = wanders.processes();
    let mut coordinator = cluster.coordinator.1.take().expect("it is running");
    coordinator.kill().await.unwrap();
    let mut store = cluster.store.1.take().expect("the store is running");
    store.kill().await.unwrap();
    cluster.kill_worker(owner).await;
    for worker in 0..cluster.workers.len() {
        let length = cluster.log(&worker_name(worker)).len();
        cluster.since.insert(worker, length);
    }
    cluster.coordinator_since = cluster.log("coordinator").len();
    cluster.start_coordinator();
    wanders.note(format!(
        "killed the coordinator, the world store and {}, which ran region {part}, and \
         started a coordinator",
        worker_name(owner)
    ));
    // How long the store is away is what is tried here.
    tokio::time::sleep(Duration::from_secs(3)).await;
    wanders.processes().start_store();
    let started = Instant::now();
    wanders.note("started the world store".to_owned());
    let may = wanders.lease + MOMENT;
    let what = "B's region is in the routing table again, with a worker";
    wanders
        .until(what, |wanders| {
            let cluster = wanders.a_cluster().expect("it is a cluster");
            cluster.owner(part).is_some_and(|now| now != owner)
        })
        .await;
    let took = started.elapsed();
    wanders.note(format!(
        "region {part} was in the routing table again, with a worker, {} after the store was \
         started",
        seconds(took)
    ));
    if took > may {
        wanders.fail(&format!(
            "region {part} was to be given a worker within {may:?} of the store's start; it \
             took {}",
            seconds(took)
        ));
    }
    wanders.whole().await;
    wanders.plays("B").await;
    wanders.is_in("B", OUT);
    wanders.finish().await;
}

/// What a worker logs at the moments of a merge at which a process is killed: the
/// worker of the absorbed region when it is told to release it, and the survivor's
/// when it is told to absorb, when it hands the merge to the world store and when the
/// merge is done.
const RELEASING: &str = "asked to release the region";
const ABSORBING: &str = "asked to have the region absorb another";
const HANDING: &str = "handing the store a merge or a split";
const MERGED: &str = "the merge has ended";

/// And of a split: when the worker is told, when it hands the split to the store, and
/// when the split is made and it opens the new region.
const SPLITTING: &str = "asked to split the region";
const OPENING: &str = "the split has ended; opening the new region";

/// A process that is killed in the middle of a merge or a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Victim {
    Worker(usize),
    /// Killed and started again at once, on the world as it left it.
    Store,
    /// Killed, and another started at once with the same arguments, which knows
    /// nothing of the one before.
    Coordinator,
}

/// Whom a test kills during a merge: the worker of the region that absorbs, the worker
/// of the region that is absorbed, the world store or the coordinator. During a split
/// the two workers are one, the worker of the region that is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Harm {
    Survivor,
    Absorbed,
    Store,
    Coordinator,
}

impl Wanders {
    /// Waits until a worker that was started has registered with the coordinator,
    /// whether or not it was given a region.
    async fn registered(&mut self, worker: usize) {
        self.until("a worker that was started has registered", |wanders| {
            let log = wanders.a_cluster().expect("a cluster").log_since(worker);
            log.contains("waiting to be given a region") || log.contains("given a region")
        })
        .await;
        self.note(format!("{} has registered", worker_name(worker)));
    }

    /// Waits until the worker `watched` has logged `moment` more than `before` times,
    /// and `then` likewise if there is one; then kills `victim` a moment later that
    /// the seed chooses. Returns when it was killed.
    async fn kills_at(
        &mut self,
        watched: usize,
        (moment, before): (&str, usize),
        then: Option<(&str, usize)>,
        victim: Victim,
    ) -> Instant {
        let mut last = moment;
        let what = format!("{} has logged `{moment}`", worker_name(watched));
        // A walk comes first, and may be a long one.
        self.until_within(AGAIN, &what, |wanders| {
            let cluster = wanders.a_cluster().expect("a cluster");
            cluster.said(watched, moment) > before
        })
        .await;
        if let Some((moment, before)) = then {
            let what = format!("{} has logged `{moment}`", worker_name(watched));
            self.until(&what, |wanders| {
                let cluster = wanders.a_cluster().expect("a cluster");
                cluster.said(watched, moment) > before
            })
            .await;
            last = moment;
        }
        // How long after the line the process dies is what the seed varies here, not
        // a wait for anything to come about.
        let delay = Duration::from_millis(self.random.below(30));
        tokio::time::sleep(delay).await;
        let why = format!("{delay:?} after {} logged `{last}`", worker_name(watched));
        let whereabouts = self.whereabouts();
        match victim {
            Victim::Worker(worker) => {
                self.processes().kill_worker(worker).await;
                let name = worker_name(worker);
                self.note(format!("killed {name}, {why}; {whereabouts}"));
            }
            Victim::Store => {
                self.processes().kill_and_start_the_store().await;
                self.note(format!(
                    "killed the world store, {why}, and started it again; {whereabouts}"
                ));
            }
            Victim::Coordinator => {
                self.processes().kill_and_start_the_coordinator().await;
                self.note(format!(
                    "killed the coordinator, {why}, and started another; {whereabouts}"
                ));
            }
        }
        Instant::now()
    }

    /// Waits, after `victim` was killed at `killed`, for every region the list has to
    /// be run again; a worker that was killed is started again at once or only then,
    /// as the seed has it. Fails unless every region runs within two leases and a
    /// moment of the kill. Returns the list as it was when everything ran.
    async fn gets_over(&mut self, victim: Victim, killed: Instant) -> RegionList {
        let at_once = self.random.one_in(2);
        let worker = match victim {
            Victim::Worker(worker) => Some(worker),
            _ => None,
        };
        if let Some(worker) = worker.filter(|_| at_once) {
            self.processes().start_worker_again(worker);
            self.note(format!("started {}", worker_name(worker)));
        }
        let list = self.everything_runs().await;
        let took = killed.elapsed();
        self.note(format!(
            "every region ran again {} after {victim:?} was killed: {:?}",
            seconds(took),
            living(&list)
        ));
        let may = 2 * self.lease + MOMENT;
        if took > may {
            self.fail(&format!(
                "every region was run again only {} after {victim:?} was killed; two leases \
                 and a moment are {may:?}",
                seconds(took)
            ));
        }
        if let Some(worker) = worker {
            if !at_once {
                self.processes().start_worker_again(worker);
                self.note(format!("started {}", worker_name(worker)));
            }
            self.registered(worker).await;
        }
        list
    }

    /// The process that `harm` is, when `survivor` and `absorbed` are the workers of
    /// the two regions of a merge, or both the worker of the region that is split.
    fn victim(harm: Harm, survivor: usize, absorbed: usize) -> Victim {
        match harm {
            Harm::Survivor => Victim::Worker(survivor),
            Harm::Absorbed => Victim::Worker(absorbed),
            Harm::Store => Victim::Store,
            Harm::Coordinator => Victim::Coordinator,
        }
    }

    /// The part `B` is in when it stands in chunk 19 by itself, by `list`: a region
    /// other than region 0 whose `bounds` have that chunk, while region 0 holds
    /// nothing east of chunk 9. Of several, as when a part that `B` was handed back
    /// out of still holds its land, the one that was made last.
    fn part_of_b(list: &RegionList) -> Option<Region> {
        let home = bounds(list, 0)?;
        let parts = living(list).into_iter().filter(|region| *region != 0);
        let theirs = parts.filter(|part| has_chunk(bounds(list, *part), OUT, 0));
        theirs.max().filter(|_| home.max.x < MERGE_DISTANCE)
    }

    /// W1's step 1 with a process killed in the middle: `B` is sent to chunk 19, the
    /// coordinator splits it off region 0 by itself, and `harm` is done at a logged
    /// moment of that split which the seed chooses. Fails unless every region runs
    /// again in time, the split is whole or not at all by the list, and afterwards,
    /// when the round has been gone through again if it has to be, the list comes to
    /// region 0 and a part with `B`'s chunk in its `bounds`. Returns that part.
    ///
    /// Not asserted, as the record has it: that no player is handed over and that
    /// region 0 is not split a second time. A region 0 that is restored after its
    /// split has no line of that split (section 3.6.5).
    async fn b_is_split_off_with_a_kill(&mut self, harm: Harm) -> Region {
        let before = self.settled().await;
        let Some(owner) = self.processes().owner(0) else {
            self.fail("region 0 has no owner though the cluster was whole");
        };
        let victim = Self::victim(harm, owner, owner);
        let part = before.next.0;
        let running = format!("running a region region={part} ");
        let (moment, then) = match self.random.below(4) {
            0 => (SPLITTING, None),
            1 => (HANDING, None),
            2 => (OPENING, None),
            // When the worker has just begun to run the part.
            _ => (OPENING, Some(running.as_str())),
        };
        let said = (moment, self.processes().said(owner, moment));
        let then = then.map(|moment| (moment, self.processes().said(owner, moment)));
        self.walks_to("B", OUT);
        let killed = self.kills_at(owner, said, then, victim).await;
        let now = self.gets_over(victim, killed).await;
        let made = info(&now, part).is_some() && now.next.0 == part + 1;
        let not_at_all = info(&now, part).is_none() && now.next == before.next;
        if !made && !not_at_all {
            self.fail(&format!(
                "the list is neither as before the split nor as after it: {now:?}"
            ));
        }
        self.note(format!(
            "after the kill the split was {}",
            if made { "made" } else { "not made at all" }
        ));

        let waiting = Instant::now();
        let what = "B is in a part of its own, and region 0 holds nothing east of chunk 9";
        self.until_the_list_within(AGAIN, what, |list| Self::part_of_b(list).is_some())
            .await;
        self.settled().await;
        self.arrives("B").await;
        self.note(format!(
            "B is in a part of its own {} after everything ran again",
            seconds(waiting.elapsed())
        ));
        // One thing more than the record says: `B` stands for 35 s, by its own steps,
        // and until region 0 has given back what it kept of the trail `B` came
        // along. A `B` that turned round at once would see that land and walk into
        // it, and be region 0's player by a hand-over before any merge is made (N2,
        // N3): the merge that the next kill is to strike might never be wanted, and
        // the part would stay behind, empty, for `B` to walk into the next time.
        self.walks_for("B", GIVEN_BACK).await;
        let what = "region 0 holds what A sees and no more, and B is in a part of its own";
        let list = self
            .until_the_list_within(AGAIN, what, |list| {
                let home = bounds(list, 0);
                Self::part_of_b(list).is_some() && home.is_some_and(|home| home.max.x <= REACH)
            })
            .await;
        self.settled().await;
        self.is_in("A", 0);
        self.is_in("B", OUT);
        Self::part_of_b(&list).expect("the list showed it")
    }

    /// W1's step 3 with a process killed in the middle: `B` is sent to chunk 8, the
    /// coordinator has region 0 absorb `part` by itself, and `harm` is done at a
    /// logged moment of that merge which the seed chooses. Fails unless every region
    /// runs again in time, the merge is whole or not at all by the list, and
    /// afterwards, when the round has been gone through again if it has to be,
    /// region 0 has absorbed the part.
    async fn b_comes_back_with_a_kill(&mut self, part: Region, harm: Harm) {
        // The part was made by region 0's worker; the two are run by different
        // workers once the workers share the regions evenly.
        self.settled().await;
        let owners = (self.processes().owner(0), self.processes().owner(part));
        let (Some(survivor), Some(releasing)) = owners else {
            self.fail("a region has no owner though the cluster was whole");
        };
        let victim = Self::victim(harm, survivor, releasing);
        let (watched, moment) = match self.random.below(4) {
            0 => (releasing, RELEASING),
            1 => (survivor, ABSORBING),
            2 => (survivor, HANDING),
            _ => (survivor, MERGED),
        };
        let said = (moment, self.processes().said(watched, moment));
        self.walks_to("B", BACK);
        let killed = self.kills_at(watched, said, None, victim).await;
        let now = self.gets_over(victim, killed).await;
        let made = absorbed_by(&now, part) == Some(0) && info(&now, part).is_none();
        let not_at_all = info(&now, part).is_some() && absorbed_by(&now, part).is_none();
        if !made && !not_at_all {
            self.fail(&format!(
                "the list is neither as before the merge nor as after it: {now:?}"
            ));
        }
        self.note(format!(
            "after the kill the merge was {}",
            if made { "made" } else { "not made at all" }
        ));

        let waiting = Instant::now();
        let absorbed = format!("region {part} is absorbed by region 0");
        self.until_the_list_within(AGAIN, &absorbed, |list| absorbed_by(list, part) == Some(0))
            .await;
        self.settled().await;
        self.note(format!(
            "region {part} is region 0's {} after everything ran again",
            seconds(waiting.elapsed())
        ));
        self.arrives("B").await;
        self.is_in("A", 0);
        self.is_in("B", BACK);
    }
}

/// W7: rounds of W1's steps 1 and 3, each with what `harm` says for its round done
/// in the middle: a process killed at a logged moment of a split and of a merge that
/// the coordinator began by itself. The lease is the shortest there is: it is what
/// everybody waits for after a kill.
async fn a_group_is_split_off_and_merged_with_kills(
    test: &str,
    harm: impl Fn(u32) -> (Harm, Harm),
) {
    let mut wanders = Wanders::cluster(test, world(Some(3))).await;
    wanders.a_settles().await;
    wanders.b_settles().await;
    wanders.region_0_is_alone("at the start").await;
    let kills = number_from("CLUSTINE_WANDERS_KILLS").map_or(2, |kills| kills as u32);
    for round in 0..kills {
        let (at_the_split, at_the_merge) = harm(round);
        let part = wanders.b_is_split_off_with_a_kill(at_the_split).await;
        wanders.b_comes_back_with_a_kill(part, at_the_merge).await;
    }
    wanders.plays("A").await;
    wanders.plays("B").await;
    wanders.finish().await;
}

/// W7, the workers. The worker of the region that is split is killed at a logged
/// moment of the split, and the survivor's worker or the absorbed region's at a
/// logged moment of the merge. After each, every region of the list runs again
/// within twice the lease and 5 s, the merge or the split was made whole or not at
/// all, and the round is gone through again until it has been; nobody is
/// disconnected, and the audit holds.
#[tokio::test(flavor = "multi_thread")]
async fn regions_follow_a_group_when_a_worker_is_killed_in_the_middle_of_a_merge_or_a_split() {
    if a_repetition() {
        return;
    }
    a_group_is_split_off_and_merged_with_kills("kills", |round| {
        // The worker that splits is the survivor's, as the part is not there yet.
        let at_the_merge = [Harm::Survivor, Harm::Absorbed][(round % 2) as usize];
        (Harm::Survivor, at_the_merge)
    })
    .await;
}

/// W7, the coordinator and the store in turn: the coordinator killed and another
/// started with the same arguments, which knows nothing of what the one before had
/// begun, and the world store killed and started again.
#[tokio::test(flavor = "multi_thread")]
async fn regions_follow_a_group_when_the_coordinator_or_the_store_is_killed_in_the_middle() {
    if a_repetition() {
        return;
    }
    a_group_is_split_off_and_merged_with_kills("other kills", |round| {
        if round % 2 == 0 {
            (Harm::Coordinator, Harm::Store)
        } else {
            (Harm::Store, Harm::Coordinator)
        }
    })
    .await;
}

/// How many lone players W9 has, and how far apart their lanes are: 19 chunks, more
/// than the split distance.
const LONE: i32 = 11;
const APART: i32 = 16 * OUT;

/// For each of the chunks `theirs`, in the column of chunks x = 0 and given by
/// their z, the one region of `list` whose `bounds` have it, if every chunk has
/// exactly one and no two have the same.
fn regions_of_their_own(list: &RegionList, theirs: &[i32]) -> Option<Vec<Region>> {
    let mut regions = Vec::new();
    for z in theirs {
        let holders = living(list).into_iter();
        let mut holders = holders.filter(|region| has_chunk(bounds(list, *region), 0, *z));
        let region = holders.next().filter(|_| holders.next().is_none())?;
        if regions.contains(&region) {
            return None;
        }
        regions.push(region);
    }
    Some(regions)
}

/// W9. Lone players (N11). One ledger of eleven bots on lanes 19 chunks apart, the
/// middle one on z = 0, all in the column of chunks x = 0: ten of them are more
/// than 18 chunks from the home chunk and from each other. Within three minutes of
/// the last one's arrival the list has eleven regions, each bot's chunk in the
/// `bounds` of a region of its own, and the two workers run six and five; nobody was
/// stood still more than once in a rest but by a move. Then they end: within three
/// minutes of the last leaving the list has region 0 alone.
///
/// Who was stood still by what is told by the regions: a bot that ends in a region
/// was in the region that one was split off, and so on back to region 0, and each of
/// those was the bot's region until the split that made the next. Every merge and
/// split of a bot's region in its time counts for the bot.
///
/// The farthest lane is 1520 blocks from where players enter, and the bots, and
/// their auditors after them, go there at `to_the_lane` blocks a tick.
async fn lone_players(test: &str, to_the_lane: f64) {
    let mut wanders = Wanders::cluster(test, world(None)).await;
    let scenario = Ledger {
        lane_spacing: APART,
        to_the_lane,
        ..wanders.scenario("L", LONE as usize, -(LONE / 2) * APART, within(0))
    };
    wanders.joins_with("L", scenario);
    let from = wanders.now_at();
    let progress = wanders.group("L").progress.clone();
    let what = "every lone player is on its lane and acknowledged";
    wanders
        .until_within(3 * PATIENCE, what, |_| {
            let bots = progress.bots();
            bots.iter()
                .all(|bot| bot.playing && bot.arrived && bot.acknowledged > 0)
        })
        .await;
    let arrived = Instant::now();
    let whereabouts = wanders.whereabouts();
    wanders.note(format!("the lone players have arrived; {whereabouts}"));

    let three_minutes = Duration::from_secs(180);
    let theirs: Vec<i32> = (-(LONE / 2)..=LONE / 2).map(|lane| lane * OUT).collect();
    let what = "there are eleven regions, each bot's chunk in a region of its own";
    let list = wanders
        .until_the_list_within(three_minutes, what, |list| {
            living(list).len() == LONE as usize && regions_of_their_own(list, &theirs).is_some()
        })
        .await;
    let regions = regions_of_their_own(&list, &theirs).expect("the list showed it");
    let what = "the two workers run six and five";
    wanders
        .until_within(three_minutes, what, |wanders| {
            let cluster = wanders.a_cluster().expect("it is a cluster");
            let mut loads: Vec<usize> = cluster.loads().iter().map(Vec::len).collect();
            loads.sort_unstable();
            loads == [5, 6] && regions.iter().all(|region| cluster.runs(*region))
        })
        .await;
    let took = arrived.elapsed();
    wanders.note(format!(
        "every lone player had a region of its own, {regions:?}, and the workers ran six and \
         five, {} after the last one arrived",
        seconds(took)
    ));
    if took > three_minutes {
        wanders.fail(&format!(
            "the lone players were to have a region each within {three_minutes:?} of the last \
             one's arrival; it took {}",
            seconds(took)
        ));
    }
    wanders.served().await;

    // Nobody was stood still more than once in a rest but by a move.
    let begun = wanders.reshapes_at();
    let begun: Vec<&(f64, Begun)> = begun.iter().filter(|(at, _)| *at >= from).collect();
    // Which region each part was split off, and when that split was begun.
    let mut made: Vec<(Region, Region, f64)> = Vec::new();
    for (at, split) in &begun {
        if let Begun::Split { region, .. } = split
            && let Some(part) = wanders.end_of(*at, split).and_then(|(_, end)| end.part())
        {
            made.push((part, *region, *at));
        }
    }
    for (bot, region) in regions.iter().enumerate() {
        // The regions the bot was in, the last first, each with when it left it.
        let mut was_in: Vec<(Region, f64)> = vec![(*region, f64::MAX)];
        loop {
            let last = was_in.last().map(|(region, _)| *region);
            match made.iter().find(|(part, ..)| Some(*part) == last) {
                Some((_, parent, at)) => was_in.push((*parent, *at)),
                None => break,
            }
        }
        let theirs: Vec<&(f64, Begun)> = begun
            .iter()
            .copied()
            .filter(|(at, begun)| {
                let mut entered = f64::MIN;
                was_in.iter().rev().any(|(region, left)| {
                    let then = begun.is_of(*region) && *at >= entered && *at <= *left;
                    entered = *left;
                    then
                })
            })
            .collect();
        for (first, (since, earlier)) in theirs.iter().enumerate() {
            for (last, (at, later)) in theirs.iter().enumerate().skip(first + 1) {
                let may = 1.0 + (at - since + WRITING) / REST.as_secs_f64();
                let were = last - first + 1;
                if were as f64 > may {
                    wanders.fail(&format!(
                        "the regions of lone player {bot} were in {were} merges and splits \
                         within {:.3} s, from {} to {}; with a rest of {REST:?} they may be \
                         in {may:.2}",
                        at - since,
                        earlier.told(),
                        later.told()
                    ));
                }
            }
        }
        wanders.note(format!(
            "lone player {bot} came to region {region} by way of {was_in:?} and was stood \
             still {} times",
            theirs.len()
        ));
    }
    wanders.nobody_waited_too_long(arrived);

    // A lone player's region holds exactly the 7 by 7 chunks around its player once
    // its trail is given back: eleven islands, 35 s later by the bots' own steps.
    wanders.walks_for("L", GIVEN_BACK).await;
    let list = wanders.whole().await;
    let islands = theirs.iter().zip(&regions).all(|(z, region)| {
        is_box(
            bounds(&list, *region),
            (-REACH, REACH),
            (z - REACH, z + REACH),
        )
    });
    if !islands || living(&list).len() != LONE as usize {
        let held: Vec<_> = living(&list)
            .into_iter()
            .map(|region| {
                let held = bounds(&list, region);
                let held = held.map(|held| (held.min.x, held.max.x, held.min.z, held.max.z));
                (region, held)
            })
            .collect();
        wanders.fail(&format!(
            "35 s after every lone player had a region of its own, each of the regions \
             {regions:?} was to hold exactly the 7 by 7 chunks around its player, who \
             stand in the chunks x = 0, z = {theirs:?}; they hold, as x and z from and \
             to: {held:?}"
        ));
    }

    wanders.leaves("L").await;
    let left = Instant::now();
    wanders
        .until_the_list_within(three_minutes, "the list has region 0 alone", |list| {
            living(list) == [0]
        })
        .await;
    wanders.note(format!(
        "the list had region 0 alone {} after the last lone player left",
        seconds(left.elapsed())
    ));
    wanders.finish().await;
}

/// W9, with the bots going to their lanes at three blocks a tick: sixty blocks a
/// second, which no player comes near, and what `moves.rs` has its bots walk to
/// lanes that are side by side.
#[tokio::test(flavor = "multi_thread")]
async fn lone_players_come_to_a_region_each_and_their_regions_are_absorbed_when_they_leave() {
    if a_repetition() {
        return;
    }
    lone_players("lone players", 3.0).await;
}

// What W9 found when its bots ran to their lanes at eight blocks a tick, as those of
// `moves.rs` run to their wide lanes.
//
// **A player whose region was split off while they went very fast was left with no
// view: their region held the chunk they stood in and nothing around it, and the
// client was not sent the chunks around them.** Seen three times in twenty-six runs
// at that pace, not in eleven at three blocks a tick, nor in four each at 1.7 and
// at 1.1 blocks a tick, the paces of an elytra with rockets and of a sprint in
// flight.
//
// The cause, found with the edge and the runner saying once a second what they
// asked and held: an edge asks for what a player sees by where the player's region
// says they are, and a region told an edge of a move only if the edge watched the
// chunk the player left or the one they came into. It takes a few ticks for an
// edge's wish for a chunk to come back to the region. A player who in those ticks
// got further than their view reaches was in chunks the edge did not watch yet; no
// move of theirs was told any more, so the edge went on asking for the chunks
// around the place they were last reported in, for good. At 160 blocks a second and
// a view of three chunks a third of a second is enough, which the moments after a
// split have. At a client's pace it takes a wait of seconds, as while a region
// whose worker died is taken over. A region now tells an edge where its own players
// moved to wherever that is (`EdgeLink::visible` in the runner), and the test below
// holds the server to it at the pace that showed it.

/// W9 with the bots running to their lanes at eight blocks a tick, as it was first
/// written: the pace at which a player outran what their edge had asked for.
#[tokio::test(flavor = "multi_thread")]
async fn lone_players_who_ran_to_their_lanes_come_to_a_region_each() {
    if a_repetition() {
        return;
    }
    lone_players("lone players, at a run", 8.0).await;
}

/// W9 at the paces a client can reach, which was run to say whether the fault above
/// showed there: 1.1 blocks a tick, 22 blocks a second, which is a sprint in
/// creative flight, and 1.7 blocks a tick, 34 blocks a second, which is an elytra
/// with rockets.
#[ignore = "takes a quarter of an hour: W9 at a client's pace"]
#[tokio::test(flavor = "multi_thread")]
async fn lone_players_who_flew_to_their_lanes_at_a_sprint_come_to_a_region_each() {
    if a_repetition() {
        return;
    }
    lone_players("lone players, at a sprint in flight", 1.1).await;
}

/// See above: 34 blocks a second.
#[ignore = "takes a quarter of an hour: W9 at a client's pace"]
#[tokio::test(flavor = "multi_thread")]
async fn lone_players_who_flew_to_their_lanes_with_rockets_come_to_a_region_each() {
    if a_repetition() {
        return;
    }
    lone_players("lone players, with rockets", 1.7).await;
}

/// The chunk (49, 0), thirty chunks beyond where a wanderer is split off.
const BEYOND: i32 = OUT + 30;

impl Wanders {
    /// Waits, when wanderers have left, until every region that was split off since
    /// the time `from` of the logs has been absorbed, or 40 s have passed by the
    /// steps of `A`: then the next wanderers find no land of theirs on their way.
    async fn the_parts_are_gone_or_forty_seconds_pass(&mut self, from: f64) {
        let slices = 16;
        for _ in 0..slices {
            let ended = self.ended();
            let parts = ended.iter().filter(|(at, _)| *at >= from);
            let parts: Vec<Region> = parts.filter_map(|(_, ended)| ended.part()).collect();
            let gone = |part: &Region| {
                ended.iter().any(|(_, ended)| {
                    matches!(ended, Ended::Merge { absorbed, .. } if absorbed == part)
                        && ended.well()
                })
            };
            if parts.iter().all(gone) {
                return;
            }
            self.walks_for("A", Duration::from_secs(40) / slices).await;
        }
    }
}

/// W10 on a cluster or on a server process (N17). `A` stands in chunk 0. In each of
/// three rounds a wanderer joins and walks straight from where players enter to the
/// middle of chunk (49, 0), at half a block a tick, through its own split, and stays
/// for 40 s. From its join to the end exactly one split is begun, of region 0, and
/// nothing else; nobody is handed over; nothing of region 0 lies ahead of the
/// wanderer at any reading of the list; the part ends as the 7 by 7 chunks around
/// the wanderer; no bot of `A` waited longer than it waits when nothing happens, but
/// once; and region 0 stood still once, with three players.
///
/// The 40 s are bounded by the rounds `A` walks in its chunk, as many as are 40 s of
/// its steps: a wanderer that stands takes no steps to count.
async fn straight_on_on_foot(mut wanders: Wanders) {
    wanders.a_settles().await;
    if wanders.has_a_list() {
        wanders.region_0_is_alone("at the start").await;
    }
    let above = wanders.calm("A").await;
    for round in 1..=rounds(3) {
        let from = wanders.now_at();
        let since = Instant::now();
        let begun_before = wanders.reshapes().len();
        let wanderer = wanders.wanders(&format!("Wanderer{round}")).await;
        let (x, z) = middle_of(BEYOND, 0);
        wanders.wanderers[wanderer].walks_to(x, z, ON_FOOT);
        wanders
            .until("the wanderer has been in chunk 19", |wanders| {
                wanders.wanderers[wanderer].noted().east >= OUT
            })
            .await;
        let entered = wanders.now_at();
        wanders
            .until("a split of region 0 has ended", |wanders| {
                let ended = wanders.ended();
                ended.iter().any(|(at, ended)| {
                    *at >= from && matches!(ended, Ended::Split { region: 0, .. })
                })
            })
            .await;
        let split_ended = Instant::now();
        wanders.wanderer_arrives(wanderer, x, ON_FOOT).await;
        wanders.walks_for("A", Duration::from_secs(40)).await;

        let split = |begun: &Begun| {
            matches!(
                begun,
                Begun::Split {
                    region: 0,
                    groups: 1,
                    ..
                }
            )
        };
        let what = "one split, of region 0, and no merge and no absorption";
        wanders.has_begun(begun_before, what, &[&split]);
        let (split_at, begun) = wanders.reshapes_at().pop().expect("a split was begun");
        let end = wanders.end_of(split_at, &begun);
        let Some(part) = end.and_then(|(_, ended)| ended.part()) else {
            wanders.fail("the split of region 0 was to end with a new region");
        };
        if split_at + WRITING < entered {
            wanders.fail(&format!(
                "the split was begun {:.3} s before the wanderer had been in chunk {OUT}",
                entered - split_at
            ));
        }
        wanders.nobody_was_handed_over(from, "while a wanderer walked straight on");
        if wanders.has_a_list() {
            wanders.the_line_holds(part, split_ended, true);
            let list = wanders.list().await;
            let theirs = bounds(&list, part);
            if !is_box(theirs, (BEYOND - REACH, BEYOND + REACH), (-REACH, REACH)) {
                wanders.fail(&format!(
                    "40 s after the wanderer arrived in chunk {BEYOND}, region {part} was to \
                     hold exactly the chunks within {REACH} of it: {list:?}"
                ));
            }
        }
        wanders.a_stood_still_once_at_most(since, above);
        wanders.region_0_stood_still_once_for_each(from, Some((3, 3)));

        // The wanderer leaves, and the next one joins when its region has been
        // absorbed or 40 s have passed.
        wanders.wanderer_leaves(wanderer).await;
        wanders.the_parts_are_gone_or_forty_seconds_pass(from).await;
        wanders.note(format!("round {round} is done, with region {part}"));
    }
    wanders.finish().await;
}

/// W10. Straight on, on foot, in a cluster.
#[tokio::test(flavor = "multi_thread")]
async fn a_wanderer_who_walks_straight_on_is_split_off_once_and_never_handed_over() {
    if a_repetition() {
        return;
    }
    straight_on_on_foot(Wanders::cluster("straight on", world(None)).await).await;
}

/// W10 against a server process, whose log has what the scenario is for; of the
/// `bounds` nothing is asserted there, as nothing prints the list.
#[tokio::test(flavor = "multi_thread")]
async fn a_wanderer_who_walks_straight_on_is_split_off_once_and_never_handed_over_in_one_process() {
    if a_repetition() {
        return;
    }
    straight_on_on_foot(Wanders::process("straight on, in one process", world(None)).await).await;
}

/// W11's wanderers: eight, on lanes seven chunks apart, at one block a tick, which
/// is twenty blocks a second, a sprint in creative flight.
const SPRINTERS: usize = 8;
const LANE_TO_LANE: f64 = 112.0;
const SPRINT: f64 = 1.0;

/// Where the sprinter `number` stands before it is sent: on its lane, two blocks
/// west of the one before, so that a chunk border is crossed by one of the eight in
/// every second tick of their sprint.
fn start_of(number: usize) -> (f64, f64) {
    let lane = number as f64 - 4.0;
    (8.5 - 2.0 * number as f64, LANE_TO_LANE * lane + 8.5)
}

/// What became of a round of W11: the part its eight went into, when the split
/// ended and the worker's line of it, if the split caught all eight; and else why
/// the round does not count.
type Caught = Result<(Region, f64, SplitOff), String>;

/// W11 on a cluster or on a server process (N17): the scenario that meets the two
/// ticks of section 3.6 on purpose. `A` stands in chunk 0. Eight wanderers stand on
/// lanes seven chunks apart, near enough for all eight and the home chunk to be one
/// group, and are sent together, in the same turn of the test, to chunk 49 on their
/// lanes at a sprint. They arrive and stay for 40 s.
///
/// **A round counts** if the first split of region 0 that is begun in it ended with
/// a new region and the worker's line for it has `players=8`: the split caught the
/// whole group. Of a round that counts, from the wanderers' being sent to the end of
/// their 40 s: no second split is begun, and no merge and no absorption that names
/// region 0 or the part; no worker logs a hand-over; from the end of the split on,
/// region 0's east end never moves east at any reading of the list, and at the end
/// the part holds x = 46 to 52 and z = -31 to 24; no bot of `A` waited longer than
/// it waits when nothing happens, but once. Of a round that does not count, in which
/// those who were not caught are region 0's own players far out and what follows is
/// ADR-0016's and right, only that nobody was disconnected and that no wait was
/// longer than 5 s.
///
/// **And the run has to have met what it is for**: over the rounds that count, the
/// worker's line of the split has been written at least once with `waited` above 0,
/// and the line `chunks asked for players who went are taken for the part's` at
/// least once with `free` above 0. Rounds until three have counted and both lines
/// were seen, twelve at most; a run that ends without fails as not having tested.
async fn straight_on_at_a_sprint(mut wanders: Wanders) {
    wanders.a_settles().await;
    if wanders.has_a_list() {
        wanders.region_0_is_alone("at the start").await;
    }
    let above = wanders.calm("A").await;
    let (mut counted, mut with_waited, mut with_free) = (0, 0, 0);
    let mut not_counted: Vec<String> = Vec::new();
    let mut round = 0;
    while round < 12 && !(counted >= 3 && with_waited > 0 && with_free > 0) {
        round += 1;
        let joined = wanders.now_at();
        let mut eight = Vec::new();
        for number in 0..SPRINTERS {
            let wanderer = wanders.wanders(&format!("Sprint{round}x{number}")).await;
            let (x, z) = start_of(number);
            wanders.wanderers[wanderer].walks_to(x, z, SPRINT);
            eight.push(wanderer);
        }
        for wanderer in &eight {
            wanders.wanderer_arrives(*wanderer, 512.0, SPRINT).await;
        }

        let from = wanders.now_at();
        let since = Instant::now();
        let begun_before = wanders.reshapes().len();
        let x = f64::from(16 * BEYOND) + 8.5;
        // Together, in the same turn of the test.
        for (number, wanderer) in eight.iter().enumerate() {
            wanders.wanderers[*wanderer].walks_to(x, start_of(number).1, SPRINT);
        }
        wanders.note(format!("the eight are sent to chunk {BEYOND}"));
        for wanderer in &eight {
            wanders.wanderer_arrives(*wanderer, x, SPRINT).await;
        }
        wanders.walks_for("A", Duration::from_secs(40)).await;

        // Which kind of round it was, by the first split of region 0 begun in it and
        // by the worker's line for that split, and by nothing else.
        let begun: Vec<(f64, Begun)> = wanders.reshapes_at().split_off(begun_before);
        let first = begun
            .iter()
            .position(|(_, begun)| matches!(begun, Begun::Split { region: 0, .. }));
        let caught: Caught = match first.map(|first| &begun[first]) {
            None => Err("no split of region 0 was begun".to_owned()),
            Some((at, split)) => match wanders.end_of(*at, split) {
                None => Err("the split of region 0 did not end".to_owned()),
                Some((ended_at, ended)) => match ended.part() {
                    None => Err(format!("the split of region 0 came to {ended:?}")),
                    Some(part) => {
                        let lines = wanders.split_off().into_iter();
                        let mut lines =
                            lines.filter(|line| line.part == part && line.at >= *at - WRITING);
                        match lines.next_back() {
                            Some(line) if line.players == SPRINTERS as u32 => {
                                Ok((part, ended_at, line))
                            }
                            Some(line) => {
                                Err(format!("the split caught {} of the eight", line.players))
                            }
                            None => Err(format!("no worker has a line for region {part}")),
                        }
                    }
                },
            },
        };
        wanders.nobody_waited_too_long(since);
        match caught {
            Err(kind) => {
                wanders.note(format!("round {round} does not count: {kind}"));
                not_counted.push(format!("round {round}: {kind}"));
            }
            Ok((part, ended_at, line)) => {
                counted += 1;
                let first = first.expect("a split was begun");
                let more = begun.iter().enumerate().any(|(index, (_, begun))| {
                    let theirs = begun.is_of(0) || begun.is_of(part);
                    index != first && (theirs || matches!(begun, Begun::Split { .. }))
                });
                if more {
                    wanders.fail(&format!(
                        "in a round whose split caught all eight, no second split was to be \
                         begun, and no merge and no absorption of region 0 or region {part}; \
                         begun were {begun:?}"
                    ));
                }
                wanders.nobody_was_handed_over(from, "in a round whose split caught all eight");
                if wanders.has_a_list() {
                    let split_ended = wanders.instant_of(ended_at);
                    wanders.the_line_holds(part, split_ended, false);
                    let list = wanders.list().await;
                    let theirs = bounds(&list, part);
                    let (north, south) = (-28 - REACH, 21 + REACH);
                    if !is_box(theirs, (BEYOND - REACH, BEYOND + REACH), (north, south)) {
                        wanders.fail(&format!(
                            "40 s after the eight arrived in chunk {BEYOND}, region {part} was \
                             to hold exactly x = {} to {} and z = {north} to {south}: {list:?}",
                            BEYOND - REACH,
                            BEYOND + REACH
                        ));
                    }
                }
                wanders.a_stood_still_once_at_most(since, above);
                let taken = wanders.taken().into_iter();
                let free: u32 = taken
                    .filter(|taken| taken.at >= from && taken.part == part)
                    .map(|taken| taken.free)
                    .sum();
                with_waited += u32::from(line.waited > 0);
                with_free += u32::from(free > 0);
                wanders.note(format!(
                    "round {round} counts: region {part} was split off with all eight, \
                     {} chunks, {} of them grants that waited; {free} chunks nobody held \
                     were taken for the part's",
                    line.chunks, line.waited
                ));
            }
        }

        // The eight leave, and the next eight join when the regions of the round
        // have been absorbed or 40 s have passed.
        for wanderer in &eight {
            wanders.wanderers[*wanderer].leaves();
        }
        for wanderer in eight {
            wanders.wanderer_leaves(wanderer).await;
        }
        wanders
            .the_parts_are_gone_or_forty_seconds_pass(joined)
            .await;
    }
    let met = format!(
        "{counted} of {round} rounds counted; in {with_waited} of them a grant waited when \
         the split was worked out, and in {with_free} chunks nobody held were taken for the \
         part's; the rounds that did not count: {not_counted:?}"
    );
    wanders.note(met.clone());
    if counted < 3 || with_waited == 0 || with_free == 0 {
        wanders.fail(&format!("the run did not test what it is for: {met}"));
    }
    wanders.finish().await;
}

/// W11. Straight on, at a sprint in flight, and at every second tick, in a cluster.
#[tokio::test(flavor = "multi_thread")]
async fn eight_who_sprint_straight_on_are_split_off_together_and_never_handed_over() {
    if a_repetition() {
        return;
    }
    straight_on_at_a_sprint(Wanders::cluster("at a sprint", world(None)).await).await;
}

/// W11 against a server process, by its log; of the `bounds` nothing is asserted
/// there.
#[tokio::test(flavor = "multi_thread")]
async fn eight_who_sprint_straight_on_are_split_off_together_and_never_handed_over_in_one_process()
{
    if a_repetition() {
        return;
    }
    let wanders = Wanders::process("at a sprint, in one process", world(None)).await;
    straight_on_at_a_sprint(wanders).await;
}

// What the runs of these tests found beside what they assert, and what the test below
// keeps. It is mended (`next_retry` in `bin/clustine/src/cluster/edge.rs`), and the
// test is no longer ignored.
//
// **An edge used a whole processor from the first time a region left the routing
// table while the edge waited to try its link again**, which was after about every
// merge and every absorption. The edge's link-keeper noted when to try a region's
// link again and slept until the earliest such time of every region it had ever
// known; a region that left the table while such a time was noted kept it for ever,
// and the loop went round without sleeping from then on. Seen as two edges of
// clusters whose test had ended, each at a whole processor for three quarters of an
// hour with no player and nothing in its log; measured by the test below as 0.02 of
// a processor before a merge and 1.02 after. The single process ran the same
// keeper, and every cluster of the tests whose coordinator merges had such an edge
// beside it.

/// How much of a processor the process `pid` has used so far, in seconds, by what
/// the system says of it. A hundredth of a second is what it counts in.
fn processor_seconds(pid: u32) -> Option<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Behind the name, which is in brackets and may have spaces: the state and the
    // numbers, of which the twelfth and thirteenth are the time in user mode and in
    // the kernel.
    let numbers: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let in_user_mode: f64 = numbers.get(11)?.parse().ok()?;
    let in_the_kernel: f64 = numbers.get(12)?.parse().ok()?;
    Some((in_user_mode + in_the_kernel) / 100.0)
}

/// After a region was absorbed the edge is as idle as before: with two players who
/// stand in one chunk it uses a small part of a processor. A wanderer is split off
/// at chunk 19 and walks back to chunk 8, where its region is merged into region 0,
/// and leaves; then the edge's use of the processor is measured over twenty seconds
/// of the steps of `A`.
#[tokio::test(flavor = "multi_thread")]
async fn an_edge_is_idle_again_after_a_region_was_absorbed() {
    if a_repetition() {
        return;
    }
    let mut wanders = Wanders::cluster("an idle edge", world(None)).await;
    wanders.a_settles().await;
    let edge = wanders.processes().edge.1.as_ref();
    let Some(edge) = edge.and_then(|edge| edge.id()) else {
        wanders.fail("the edge does not run");
    };
    let used_in = async |wanders: &mut Wanders| {
        let (before, since) = (processor_seconds(edge), Instant::now());
        wanders.walks_for("A", Duration::from_secs(20)).await;
        let used = processor_seconds(edge).zip(before);
        let used = used.map(|(now, before)| now - before);
        used.map(|used| used / since.elapsed().as_secs_f64())
    };
    let Some(before) = used_in(&mut wanders).await else {
        // Nowhere to read it from, on a system that is not Linux.
        return;
    };
    wanders.note(format!(
        "before any merge the edge uses {before:.2} of a processor"
    ));

    let wanderer = wanders.wanders("Rover").await;
    let (x, z) = east_along_the_row(OUT);
    wanders.wanderers[wanderer].walks_to(x, z, ON_FOOT);
    wanders.wanderer_arrives(wanderer, x, ON_FOOT).await;
    wanders
        .until_the_list("the wanderer is split off", |list| living(list) == [0, 1])
        .await;
    wanders.walks_for("A", GIVEN_BACK).await;
    let (x, z) = east_along_the_row(BACK);
    wanders.wanderers[wanderer].walks_to(x, z, ON_FOOT);
    let merged = "the wanderer's region is merged into region 0";
    wanders
        .until_the_list(merged, |list| absorbed_by(list, 1) == Some(0))
        .await;
    wanders.wanderer_arrives(wanderer, x, ON_FOOT).await;
    wanders.wanderer_leaves(wanderer).await;
    wanders.whole().await;

    let after = used_in(&mut wanders).await.expect("it was read before");
    wanders.note(format!(
        "after the merge the edge uses {after:.2} of a processor"
    ));
    if after > 0.5 {
        wanders.fail(&format!(
            "the edge was to be as idle after a merge as before it, when it used {before:.2} \
             of a processor: it uses {after:.2}"
        ));
    }
    wanders.finish().await;
}
