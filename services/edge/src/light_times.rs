//! When a chunk gets its light job: the hold, the rest, the order and the patience of
//! `docs/adr/0023-light-at-the-edge.md` ("the record" below), tested from the record
//! (sections 4 to 6 and, where it differs, "L4's second part as built") and the page at
//! the head of `script.rs`, by someone who has not seen how the fan-out queues light.
//!
//! **How a test here goes.** A [`Run`] is a script whose jobs are held, on a paused
//! clock, and beside it every job the edge has dispatched with the time of the clock at
//! which it did. The clock moves only where a test lets time pass ([`Run::pass`]), in
//! steps much shorter than any setting, and the edge has its turns after every step; no
//! wait of the script moves it. The jobs are looked at immediately before every step,
//! so the time a job is noted with is the time of the clock in the turn that dispatched
//! it, exactly.
//!
//! **What the times are held to.** The record as built wakes the task for light when
//! the first hold or rest ends or a job is to be given up, and never sooner than 50 ms
//! after it last did. So something that is due at a time is asked for within a tenth of
//! a second after it, and never before it.
//!
//! **Findings** are the tests marked `#[ignore = "finding: ..."]`: each is kept as the
//! record has it and fails today.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use clustine_data::blocks;
use clustine_world::{Chunk, ChunkPos, Light, SectionLight, sky_light};
use tokio::time::Instant;

use crate::fanout::LightSettings;
use crate::light::{FromThread, LightJob, LightOutcome};
use crate::script::{
    Client, End, FLOOR, HOME, Script, Setup, Told, VIEW, WEST, as_held, at, ground, lamp,
    nine_around, player, roof, unlit, view,
};

// ---------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------

/// Longer than any test lets pass.
const FOREVER: Duration = Duration::from_secs(1_000_000);

/// Nothing is held, nothing rests and no job is given up: what is asked is the order.
const NO_WAITS: LightSettings = LightSettings {
    latest: Duration::ZERO,
    rest: Duration::ZERO,
    overdue: Duration::from_secs(5),
    job_patience: FOREVER,
};

/// Nothing is held and nothing rests, and a thread has five seconds for a job.
const FIVE_SECONDS_A_JOB: LightSettings = LightSettings {
    job_patience: Duration::from_secs(5),
    ..NO_WAITS
};

/// The clock moves by this much at a time, and the edge has its turns in between.
const STEP: Duration = Duration::from_millis(25);

const fn ms(milliseconds: u64) -> Duration {
    Duration::from_millis(milliseconds)
}

const fn p(x: i32, z: i32) -> ChunkPos {
    ChunkPos::new(x, z)
}

// ---------------------------------------------------------------------------------
// A script on a clock
// ---------------------------------------------------------------------------------

/// A script with held jobs, and every job the edge dispatched with the time at which
/// it did; see the head of the file.
struct Run {
    script: Script,
    began: Instant,
    /// Every job dispatched, in order, with the time since the start.
    jobs: Vec<(Duration, LightJob)>,
}

impl Run {
    async fn start(light: LightSettings) -> Self {
        let mut setup = Setup::new(End::Held);
        setup.config.light = light;
        Self {
            script: Script::start_with(setup).await,
            began: Instant::now(),
            jobs: Vec::new(),
        }
    }

    /// The time of the clock since the start.
    fn now(&self) -> Duration {
        Instant::now() - self.began
    }

    /// Notes what was dispatched since this was last asked, as dispatched now: the
    /// clock has not moved since this was last asked.
    fn see(&mut self) {
        let now = self.now();
        for job in self.script.held().fresh() {
            self.jobs.push((now, job));
        }
    }

    /// The edge has handled everything said to it, and what it dispatched is noted.
    async fn quiet(&mut self) {
        self.script.quiet().await;
        self.script.quiet().await;
        self.see();
    }

    /// Lets `time` pass, a step at a time, the edge having its turns after each.
    async fn pass(&mut self, time: Duration) {
        let mut left = time;
        while !left.is_zero() {
            let step = left.min(STEP);
            // Before the clock moves, so that a job is noted with the time of the
            // turn that dispatched it.
            self.see();
            tokio::time::advance(step).await;
            left -= step;
            self.script.quiet().await;
        }
        self.quiet().await;
    }

    /// The times at which a job for `place` was dispatched.
    fn times(&self, place: ChunkPos) -> Vec<Duration> {
        let of_it = |(_, job): &&(Duration, LightJob)| job.position == place;
        let jobs = self.jobs.iter().filter(of_it);
        jobs.map(|(at, _)| *at).collect()
    }

    /// How many jobs were dispatched for `place`.
    fn count(&self, place: ChunkPos) -> usize {
        self.times(place).len()
    }

    /// The jobs for `place`, in order.
    fn jobs_for(&self, place: ChunkPos) -> Vec<LightJob> {
        let of_it = |(_, job): &&(Duration, LightJob)| job.position == place;
        let jobs = self.jobs.iter().filter(of_it);
        jobs.map(|(_, job)| job.clone()).collect()
    }

    /// The jobs that are out, in the order of their dispatch.
    fn out(&mut self) -> Vec<LightJob> {
        self.see();
        self.script.held().out()
    }

    /// The one job that is out for `place`.
    fn out_for(&mut self, place: ChunkPos) -> LightJob {
        let out = self.out();
        let mut jobs = out.iter().filter(|job| job.position == place);
        let (Some(job), None) = (jobs.next(), jobs.next()) else {
            panic!("not one job is out for {place:?}: {:?}", places(&out));
        };
        job.clone()
    }

    /// Answers the job with what the rules give, and the edge has taken it.
    async fn answer(&mut self, basis: u64) -> LightOutcome {
        let outcome = self.script.held().answer(basis);
        self.script.turn().await;
        self.quiet().await;
        outcome
    }

    /// Answers every job that is out with what the rules give, and those dispatched
    /// for that, until none is out. The clock stands still.
    async fn answer_all(&mut self) {
        loop {
            self.quiet().await;
            let out = self.out();
            if out.is_empty() {
                return;
            }
            for job in out {
                self.script.held().answer(job.basis);
            }
        }
    }

    /// With one job out at a time: answers the job that is out, and the one that is
    /// dispatched for that, until none is. Returns the chunks in that order. The clock
    /// stands still.
    async fn one_by_one(&mut self) -> Vec<ChunkPos> {
        let mut order = Vec::new();
        loop {
            self.quiet().await;
            let out = self.out();
            match out.as_slice() {
                [] => return order,
                [job] => {
                    order.push(job.position);
                    self.script.held().answer(job.basis);
                    self.script.turn().await;
                }
                more => panic!("several jobs are out at a time: {:?}", places(more)),
            }
        }
    }

    /// Whether `client` holds, for `place`, the light of the rules for the chunks the
    /// replica has now.
    fn holds_the_rules(&self, client: &Client, place: ChunkPos) -> bool {
        let due = self.script.model().light(place);
        let due = due.expect("the model has the chunk");
        client.light(place) == Some(&as_held(&due))
    }
}

fn places(jobs: &[LightJob]) -> Vec<ChunkPos> {
    jobs.iter().map(|job| job.position).collect()
}

/// How many of a job's nine chunks are there.
fn there(job: &LightJob) -> usize {
    job.nine.iter().filter(|chunk| chunk.is_some()).count()
}

/// The eight places around `place`.
fn around(place: ChunkPos) -> Vec<ChunkPos> {
    let nine = nine_around(place);
    nine.into_iter().filter(|other| *other != place).collect()
}

/// The squared distance between two chunks: the record's rank.
fn far(one: ChunkPos, other: ChunkPos) -> i64 {
    let (dx, dz) = (i64::from(one.x - other.x), i64::from(one.z - other.z));
    dx * dx + dz * dz
}

/// The chunks someone at [`HOME`] is shown, in ascending order.
fn home_view() -> Vec<ChunkPos> {
    view(HOME, VIEW).into_iter().collect()
}

/// How many light updates `client` was sent for `place`.
fn lights(client: &Client, place: ChunkPos) -> usize {
    let told = client.told_of(place);
    let light = |told: &&Told| matches!(told, Told::Light(..));
    told.iter().filter(light).count()
}

/// What a failed job leaves an entry that had no light (section 5): the sky's light
/// by the chunk's heights, and block light dark wherever sky light is listed.
fn by_heights(chunk: &Chunk) -> Light {
    let sky = sky_light(chunk);
    let dark = |section: &SectionLight| match section {
        SectionLight::Absent => SectionLight::Absent,
        _ => SectionLight::Dark,
    };
    let block = sky.iter().map(dark).collect();
    Light {
        made_by: 0,
        beside: 0,
        sky,
        block,
    }
}

/// Terrain that was changed since it was generated, different from place to place and
/// the same at every call: a lamp, a roof, or bare ground, none of it with light.
fn terrain(place: ChunkPos) -> Chunk {
    match (place.x + 2 * place.z).rem_euclid(4) {
        0 => unlit(lamp(ground(), 8, FLOOR, 8)),
        1 => roof(unlit(ground()), FLOOR + 3, (0, 0), (15, 7)),
        _ => unlit(ground()),
    }
}

/// A small generator of numbers that gives the same ones for the same seed.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `places` in an order that turns on the seed alone.
    fn shuffled(&mut self, mut places: Vec<ChunkPos>) -> Vec<ChunkPos> {
        for last in (1..places.len()).rev() {
            let other = (self.next() % (last as u64 + 1)) as usize;
            places.swap(last, other);
        }
        places
    }
}

// ---------------------------------------------------------------------------------
// 1. The hold
// ---------------------------------------------------------------------------------

/// Section 4: an unsendable entry is held while some wanted neighbour has not come and
/// `unsendable_since` is less than `LATEST` ago; `LATEST` is the patience for a
/// neighbour that does not come. The chunk then gets its job beside walls and is sent.
#[tokio::test(start_paused = true)]
async fn a_chunk_without_light_waits_latest_for_neighbours_that_do_not_come_and_no_longer() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, unlit(ground()));
    run.quiet().await;
    assert!(run.jobs.is_empty(), "dispatched before its neighbours");
    run.pass(ms(900)).await;
    assert!(run.jobs.is_empty(), "dispatched before the hold ended");
    run.script.sync(&mut client).await;
    assert!(!client.has(HOME));
    run.pass(ms(200)).await;
    let times = run.times(HOME);
    assert_eq!(times.len(), 1, "{times:?}");
    assert!(ms(1000) <= times[0] && times[0] <= ms(1100), "{times:?}");
    let job = run.out_for(HOME);
    assert_eq!(there(&job), 1, "no neighbour has come");
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert!(run.holds_the_rules(&client, HOME));
    // And that was all: the chunk is current.
    run.pass(ms(3000)).await;
    assert_eq!(run.jobs.len(), 1);
    run.script.end().await;
}

/// Section 4, "where entries are moved": a neighbour's coming can end a hold. The
/// chunk is dispatched in the turn its last wanted neighbour comes, whichever of the
/// eight that is, beside all eight, and no time has to pass for it.
#[tokio::test(start_paused = true)]
async fn a_held_chunk_is_dispatched_in_the_turn_its_last_wanted_neighbour_comes() {
    for last in around(HOME) {
        let mut run = Run::start(LightSettings::SERVER).await;
        let mut client = run.script.join(1).await;
        run.script.snapshot(WEST, HOME, unlit(ground()));
        run.quiet().await;
        run.pass(ms(300)).await;
        for place in around(HOME) {
            if place != last {
                run.script.snapshot(WEST, place, ground());
                run.quiet().await;
                assert!(run.jobs.is_empty(), "dispatched before {last:?} came");
            }
        }
        run.pass(ms(300)).await;
        assert!(run.jobs.is_empty(), "dispatched before {last:?} came");
        run.script.snapshot(WEST, last, ground());
        run.quiet().await;
        assert_eq!(run.times(HOME), [ms(600)], "the last to come was {last:?}");
        let job = run.out_for(HOME);
        assert_eq!(there(&job), 9);
        run.answer(job.basis).await;
        run.script.sync(&mut client).await;
        assert!(run.holds_the_rules(&client, HOME));
        assert_eq!(run.jobs.len(), 1);
        run.script.end().await;
    }
}

/// Section 4: `unsendable_since` is not set anew while an entry stays unsendable, so a
/// hold ends: neighbours that keep coming do not prolong it.
#[tokio::test(start_paused = true)]
async fn a_hold_is_not_prolonged_by_the_neighbours_that_come_during_it() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let _client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, unlit(ground()));
    run.quiet().await;
    for place in [p(-1, 0), p(1, 0), p(0, 1)] {
        run.pass(ms(300)).await;
        run.script.snapshot(WEST, place, ground());
        run.quiet().await;
        assert!(run.jobs.is_empty());
    }
    run.pass(ms(200)).await;
    let times = run.times(HOME);
    assert_eq!(times.len(), 1, "{times:?}");
    assert!(ms(1000) <= times[0] && times[0] <= ms(1100), "{times:?}");
    assert_eq!(there(&run.out_for(HOME)), 4);
    run.script.end().await;
}

/// Section 4: neighbours nobody wants have no entry and are not waited for. A chunk in
/// the corner of the only view has three wanted neighbours, and one at its side five.
#[tokio::test(start_paused = true)]
async fn a_neighbour_nobody_wants_is_not_waited_for() {
    for (place, wanted) in [(p(3, 3), 3), (p(3, 0), 5), (p(-3, -3), 3), (p(0, -3), 5)] {
        let inside: Vec<_> = around(place)
            .into_iter()
            .filter(|other| view(HOME, VIEW).contains(other))
            .collect();
        assert_eq!(inside.len(), wanted);

        // The chunk comes last: it is dispatched as it comes.
        let mut run = Run::start(LightSettings::SERVER).await;
        let mut client = run.script.join(1).await;
        for other in &inside {
            run.script.snapshot(WEST, *other, ground());
        }
        run.quiet().await;
        assert!(run.jobs.is_empty());
        run.script.snapshot(WEST, place, unlit(ground()));
        run.quiet().await;
        assert_eq!(run.times(place), [Duration::ZERO], "{place:?}");
        let job = run.out_for(place);
        assert_eq!(there(&job), wanted + 1);
        run.answer(job.basis).await;
        run.script.sync(&mut client).await;
        assert!(run.holds_the_rules(&client, place));
        run.script.end().await;

        // The chunk comes first: it is held for those that are wanted, and for no
        // other.
        let mut run = Run::start(LightSettings::SERVER).await;
        let _client = run.script.join(1).await;
        run.script.snapshot(WEST, place, unlit(ground()));
        run.quiet().await;
        for other in &inside {
            assert!(run.jobs.is_empty(), "{place:?} before {other:?}");
            run.script.snapshot(WEST, *other, ground());
            run.quiet().await;
        }
        assert_eq!(run.times(place), [Duration::ZERO], "{place:?}");
        assert_eq!(there(&run.out_for(place)), wanted + 1);
        run.script.end().await;
    }
}

/// Section 4: a chunk with carried light that is current is sent at once, neighbours
/// or none. No job is made for it, then or later.
#[tokio::test(start_paused = true)]
async fn a_chunk_whose_carried_light_is_right_is_sent_at_once_whatever_its_neighbours() {
    // Alone.
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, ground());
    run.quiet().await;
    run.script.sync(&mut client).await;
    assert!(client.has(HOME));
    assert_eq!(client.light(HOME), ground().light().map(as_held).as_ref());
    run.pass(ms(2500)).await;
    assert!(run.jobs.is_empty());
    run.script.end().await;

    // Beside a chunk that is held itself.
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, p(1, 0), unlit(ground()));
    run.quiet().await;
    run.script.snapshot(WEST, HOME, ground());
    run.quiet().await;
    run.script.sync(&mut client).await;
    assert!(client.has(HOME));
    assert!(!client.has(p(1, 0)));
    assert!(run.jobs.is_empty());
    assert_eq!(run.now(), Duration::ZERO);
    run.script.end().await;
}

/// Section 4: a stale chunk is never held. A block changes in a chunk that was sent
/// with its carried light and none of whose wanted neighbours has come: its job is
/// dispatched in that turn.
#[tokio::test(start_paused = true)]
async fn a_stale_chunk_is_never_held() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, ground());
    run.quiet().await;
    run.script.sync(&mut client).await;
    assert!(client.has(HOME));
    run.pass(ms(100)).await;
    run.script
        .change(WEST, &[(at(HOME, 8, FLOOR, 8), blocks::GLOWSTONE)]);
    run.quiet().await;
    assert_eq!(run.times(HOME), [ms(100)]);
    let job = run.out_for(HOME);
    assert_eq!(there(&job), 1);
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    // They have none of the chunks beside it, so they are at the rim and are sent it.
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(client.times_sent(HOME), 1);
    run.script.end().await;
}

/// Section 4, the last point: a chunk with carried light is sent at once; if a
/// neighbour then comes changed, the chunk is unsendable, held until the rest of its
/// neighbours are in, lit once and corrected by one light update.
#[tokio::test(start_paused = true)]
async fn a_changed_neighbour_beside_carried_light_costs_one_job_and_one_update_once_all_are_in() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, ground());
    run.quiet().await;
    run.script.sync(&mut client).await;
    assert!(client.has(HOME));
    // A lamp at the western border of the chunk east of it: its light crosses.
    let east = p(1, 0);
    run.script
        .snapshot(WEST, east, unlit(lamp(ground(), 0, FLOOR, 8)));
    run.quiet().await;
    assert_eq!(run.count(HOME), 0, "held for the seven that have not come");
    let rest: Vec<_> = around(HOME)
        .into_iter()
        .filter(|place| *place != east)
        .collect();
    for place in &rest {
        assert_eq!(run.count(HOME), 0, "dispatched before {place:?} came");
        run.script.snapshot(WEST, *place, ground());
        run.quiet().await;
    }
    assert_eq!(run.times(HOME), [Duration::ZERO]);
    let job = run.out_for(HOME);
    assert_eq!(there(&job), 9);
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert_eq!(lights(&client, HOME), 1);
    assert!(run.holds_the_rules(&client, HOME));
    // The others around the lamp are lit when their holds end; nothing of that
    // costs this chunk another job.
    run.pass(ms(1200)).await;
    run.answer_all().await;
    run.script.sync(&mut client).await;
    assert_eq!(run.count(HOME), 1);
    assert_eq!(client.times_sent(HOME), 1);
    assert_eq!(lights(&client, HOME), 1);
    assert!(run.holds_the_rules(&client, HOME));
    run.script.end().await;
}

/// Section 4: a neighbour that comes after `LATEST` costs one more job and one light
/// update. Section 5, the table's third row: the chunk is then unsendable anew and held
/// only while its new `unsendable_since` allows.
#[tokio::test(start_paused = true)]
async fn a_neighbour_that_comes_after_latest_costs_one_more_job_and_one_light_update() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, unlit(ground()));
    run.quiet().await;
    run.pass(ms(1100)).await;
    let job = run.out_for(HOME);
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert!(client.has(HOME));
    run.pass(ms(400)).await;

    let came = run.now();
    run.script
        .snapshot(WEST, p(1, 0), unlit(lamp(ground(), 0, FLOOR, 8)));
    run.quiet().await;
    assert_eq!(run.count(HOME), 1, "held for the seven that have not come");
    run.pass(ms(900)).await;
    assert_eq!(run.count(HOME), 1, "dispatched before the hold ended");
    run.pass(ms(200)).await;
    let times = run.times(HOME);
    assert_eq!(times.len(), 2, "{times:?}");
    let waited = times[1] - came;
    assert!(ms(1000) <= waited && waited <= ms(1100), "{waited:?}");
    run.answer_all().await;
    run.script.sync(&mut client).await;
    assert_eq!(run.count(HOME), 2);
    assert_eq!(client.times_sent(HOME), 1);
    assert_eq!(lights(&client, HOME), 1);
    assert!(run.holds_the_rules(&client, HOME));
    assert!(run.holds_the_rules(&client, p(1, 0)));
    run.script.end().await;
}

/// "L4's second part as built": a wanted chunk beside a held one that stops being
/// wanted (its last viewer walked off) does not end the hold; the time does, at most
/// `LATEST` later. What is asked here is the bound: the chunk is dispatched by then,
/// and sent to both who wait.
#[tokio::test(start_paused = true)]
async fn a_chunk_held_for_a_neighbour_nobody_wants_any_more_is_dispatched_within_latest() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let (mut stays, mut walks) = (run.script.join(1).await, run.script.join(2).await);
    run.script.walk(player(2), p(1, 0));
    run.quiet().await;
    for place in [p(2, -1), p(2, 0), p(2, 1), p(3, -1), p(3, 1)] {
        run.script.snapshot(WEST, place, ground());
    }
    // The three east of it are wanted by the one who walked, and have not come.
    run.script.snapshot(WEST, p(3, 0), unlit(ground()));
    run.quiet().await;
    assert!(run.jobs.is_empty());
    run.pass(ms(200)).await;
    assert!(run.jobs.is_empty());
    run.script.walk(player(2), HOME);
    run.quiet().await;
    run.pass(ms(900)).await;
    let times = run.times(p(3, 0));
    assert_eq!(times.len(), 1, "{times:?}");
    assert!(times[0] <= ms(1100), "{times:?}");
    run.answer_all().await;
    for client in [&mut stays, &mut walks] {
        run.script.sync(client).await;
        assert!(run.holds_the_rules(client, p(3, 0)));
    }
    run.script.end().await;
}

// ---------------------------------------------------------------------------------
// 2. A join into changed terrain
// ---------------------------------------------------------------------------------

/// When the jobs of a join are answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answered {
    /// Each as soon as it is dispatched, before the next chunk comes.
    AsTheyCome,
    /// None before the last chunk has come.
    AtTheEnd,
}

/// Section 4: held is what makes a join cost one job a chunk. A player joins and the
/// whole view comes without light, in `order`, all of it at one time of the clock
/// (within the hold, then). Every chunk gets one job, beside every neighbour that is
/// wanted; the client is sent every chunk once, with the light of the rules, and no
/// light update; and no time has to pass for any of it.
async fn a_join_costs_one_job_a_chunk(order: &[ChunkPos], slots: usize, answered: Answered) {
    let said = format!("{slots} jobs at a time, answered {answered:?}, in the order {order:?}");
    let mut run = Run::start(LightSettings::SERVER).await;
    run.script.held().set_in_flight(slots);
    let mut client = run.script.join(1).await;
    let all: BTreeSet<_> = order.iter().copied().collect();
    assert_eq!(all, view(HOME, VIEW));
    assert_eq!(all.len(), order.len());
    for place in order {
        run.script.snapshot(WEST, *place, terrain(*place));
        run.quiet().await;
        if answered == Answered::AsTheyCome {
            run.answer_all().await;
        }
    }
    run.answer_all().await;
    run.script.sync(&mut client).await;
    assert_eq!(run.now(), Duration::ZERO);
    for place in order {
        let jobs = run.jobs_for(*place);
        assert_eq!(jobs.len(), 1, "{place:?} cost {} jobs; {said}", jobs.len());
        assert!(
            run.script.model().same_nine(&jobs[0]),
            "{place:?} was lit beside {} chunks; {said}",
            there(&jobs[0])
        );
        assert_eq!(client.times_sent(*place), 1, "{place:?}; {said}");
        assert!(run.holds_the_rules(&client, *place), "{place:?}; {said}");
    }
    let updates = client.told().iter();
    let updates = updates.filter(|told| matches!(told, Told::Light(..)));
    assert_eq!(updates.count(), 0, "{said}");
    run.script.end().await;
}

#[tokio::test(start_paused = true)]
async fn a_join_into_changed_terrain_costs_one_job_a_chunk_in_ascending_and_descending_order() {
    let ascending = home_view();
    let descending: Vec<_> = ascending.iter().rev().copied().collect();
    for order in [&ascending, &descending] {
        for answered in [Answered::AsTheyCome, Answered::AtTheEnd] {
            a_join_costs_one_job_a_chunk(order, 4, answered).await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_join_into_changed_terrain_costs_one_job_a_chunk_nearest_first_and_farthest_first() {
    let mut nearest = home_view();
    nearest.sort_by_key(|place| far(*place, HOME));
    let farthest: Vec<_> = nearest.iter().rev().copied().collect();
    for order in [&nearest, &farthest] {
        for answered in [Answered::AsTheyCome, Answered::AtTheEnd] {
            for slots in [1, 4] {
                a_join_costs_one_job_a_chunk(order, slots, answered).await;
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_join_into_changed_terrain_costs_one_job_a_chunk_in_random_orders_from_fixed_seeds() {
    for seed in 1..=6 {
        let order = Dice(seed).shuffled(home_view());
        let slots = [1, 2, 4, 64][seed as usize % 4];
        for answered in [Answered::AsTheyCome, Answered::AtTheEnd] {
            a_join_costs_one_job_a_chunk(&order, slots, answered).await;
        }
    }
}

// ---------------------------------------------------------------------------------
// 3. The rest
// ---------------------------------------------------------------------------------

/// A player at [`HOME`] who has the whole view, of ground with its carried light, and
/// room for as many jobs as anything here marks: what is asked is the rest alone.
async fn a_view_of_ground(light: LightSettings) -> (Run, Client) {
    let mut run = Run::start(light).await;
    let mut client = run.script.join(1).await;
    for place in home_view() {
        run.script.snapshot(WEST, place, ground());
    }
    run.quiet().await;
    run.script.sync(&mut client).await;
    assert_eq!(client.chunks(), view(HOME, VIEW));
    assert!(run.jobs.is_empty());
    run.script.held().set_in_flight(64);
    (run, client)
}

/// A block of [`HOME`] becomes a lamp, or air again: light sees every one of these.
fn flicker(run: &mut Run, count: &mut u32) {
    *count += 1;
    let state = if *count % 2 == 1 {
        blocks::GLOWSTONE
    } else {
        blocks::AIR
    };
    run.script.change(WEST, &[(at(HOME, 8, FLOOR, 8), state)]);
}

/// Section 4: a stale entry rests until `REST` after its last dispatch; rest lets the
/// first change after a quiet time through at once.
#[tokio::test(start_paused = true)]
async fn the_first_change_after_a_quiet_time_gets_its_job_at_once_and_the_next_rests() {
    let (mut run, _client) = a_view_of_ground(LightSettings::SERVER).await;
    let mut count = 0;
    run.pass(ms(1000)).await;
    flicker(&mut run, &mut count);
    run.quiet().await;
    assert_eq!(run.times(HOME), [ms(1000)]);
    run.answer_all().await;

    // The second, in the same moment: it waits for the rest, and no longer.
    flicker(&mut run, &mut count);
    run.quiet().await;
    assert_eq!(run.count(HOME), 1);
    run.pass(ms(200)).await;
    assert_eq!(run.count(HOME), 1, "{:?}", run.times(HOME));
    run.pass(ms(150)).await;
    let times = run.times(HOME);
    assert_eq!(times.len(), 2, "{times:?}");
    assert!(ms(1250) <= times[1] && times[1] <= ms(1350), "{times:?}");
    run.answer_all().await;

    // A quiet time, and a change after it: at once again.
    run.pass(ms(1000)).await;
    let now = run.now();
    flicker(&mut run, &mut count);
    run.quiet().await;
    assert_eq!(run.times(HOME)[2..], [now]);
    run.answer_all().await;
    run.script.end().await;
}

/// Section 4: rest bounds the jobs of one chunk to four a second. Section 9: a block
/// change in a chunk that is changing all the time is shown after `REST` and one job.
/// A block changes every 50 ms for three seconds and every job is answered as it is
/// dispatched: no chunk gets two jobs within a rest, and the changed chunk gets one
/// about every rest.
#[tokio::test(start_paused = true)]
async fn a_chunk_whose_blocks_keep_changing_gets_one_job_a_rest_and_no_more() {
    let (mut run, _client) = a_view_of_ground(LightSettings::SERVER).await;
    let mut count = 0;
    for _ in 0..60 {
        flicker(&mut run, &mut count);
        run.answer_all().await;
        run.pass(ms(50)).await;
        run.answer_all().await;
    }
    let lit: BTreeSet<_> = run.jobs.iter().map(|(_, job)| job.position).collect();
    for place in lit {
        let times = run.times(place);
        for pair in times.windows(2) {
            assert!(pair[1] - pair[0] >= ms(250), "{place:?}: {times:?}");
        }
    }
    // The first at once, and one a rest from then on; a rest that ends may be found
    // a tenth of a second late.
    let times = run.times(HOME);
    assert_eq!(times[0], Duration::ZERO, "{times:?}");
    assert!(times.len() <= 13, "{times:?}");
    assert!(times.len() >= 9, "{times:?}");
    run.script.end().await;
}

/// Section 5, the table's second row: after a result that was withheld the next job
/// is exempt from the rest, once in a `REST` ("as built": counted from when the
/// exemption was last granted). Those who have the chunk are sent nothing of a withheld
/// result.
#[tokio::test(start_paused = true)]
async fn the_job_after_a_withheld_result_does_not_rest_once_in_a_rest() {
    let (mut run, mut client) = a_view_of_ground(LightSettings::SERVER).await;
    let mut count = 0;
    flicker(&mut run, &mut count);
    run.quiet().await;
    let first = run.out_for(HOME);
    // A change under the job: its result is withheld, and the next job is exempt.
    flicker(&mut run, &mut count);
    run.quiet().await;
    assert_eq!(run.count(HOME), 1, "a chunk has one job at a time");
    run.answer(first.basis).await;
    assert_eq!(run.times(HOME), [Duration::ZERO, Duration::ZERO]);
    let second = run.out_for(HOME);

    // And again under that one: withheld too, but exempt it is only once in a rest.
    flicker(&mut run, &mut count);
    run.quiet().await;
    run.answer(second.basis).await;
    assert_eq!(run.count(HOME), 2, "{:?}", run.times(HOME));
    run.pass(ms(200)).await;
    assert_eq!(run.count(HOME), 2, "{:?}", run.times(HOME));
    run.pass(ms(150)).await;
    let times = run.times(HOME);
    assert_eq!(times.len(), 3, "{times:?}");
    assert!(ms(250) <= times[2] && times[2] <= ms(350), "{times:?}");
    let third = run.out_for(HOME);
    run.script.sync(&mut client).await;
    assert_eq!(lights(&client, HOME), 0, "a withheld result was sent");

    // More than a rest after the exemption was granted, a third result is withheld:
    // the job after it is exempt again.
    run.pass(ms(100)).await;
    flicker(&mut run, &mut count);
    run.quiet().await;
    let now = run.now();
    run.answer(third.basis).await;
    let times = run.times(HOME);
    assert_eq!(times[3..], [now], "{times:?}");
    run.answer_all().await;
    run.script.sync(&mut client).await;
    assert_eq!(run.count(HOME), 4);
    assert_eq!(client.times_sent(HOME), 1);
    run.script.end().await;
}

/// Section 4: it is a stale entry that rests. A chunk that cannot be sent and that
/// someone waits for gets its job at once, however lately it had one: here a chunk
/// that is replaced the moment it was lit. The change after that does rest.
#[tokio::test(start_paused = true)]
async fn a_chunk_that_cannot_be_sent_does_not_rest() {
    let slow = LightSettings {
        rest: Duration::from_secs(10),
        ..LightSettings::SERVER
    };
    let mut run = Run::start(slow).await;
    let mut client = run.script.join(1).await;
    let corner = p(3, 3);
    for place in [p(2, 2), p(2, 3), p(3, 2)] {
        run.script.snapshot(WEST, place, ground());
    }
    run.script.snapshot(WEST, corner, unlit(ground()));
    run.quiet().await;
    let job = run.out_for(corner);
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert!(client.has(corner));

    // It comes again and shows otherwise (E3).
    run.script
        .snapshot(WEST, corner, unlit(lamp(ground(), 8, FLOOR, 8)));
    run.quiet().await;
    assert_eq!(run.times(corner), [Duration::ZERO, Duration::ZERO]);
    let job = run.out_for(corner);
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    let told = client.told_of(corner);
    assert_eq!(
        told,
        [
            Told::Sent(corner),
            Told::Unloaded(corner),
            Told::Sent(corner)
        ]
    );
    assert!(run.holds_the_rules(&client, corner));

    // A block changes in it: stale, and it rests.
    run.script
        .change(WEST, &[(at(corner, 12, FLOOR, 12), blocks::GLOWSTONE)]);
    run.quiet().await;
    run.pass(ms(9_800)).await;
    assert_eq!(run.count(corner), 2, "{:?}", run.times(corner));
    run.pass(ms(300)).await;
    let times = run.times(corner);
    assert_eq!(times.len(), 3, "{times:?}");
    assert!(ms(10_000) <= times[2], "{times:?}");
    run.script.end().await;
}

// ---------------------------------------------------------------------------------
// 4. The order
// ---------------------------------------------------------------------------------

/// Section 4, dispatch: the first of `waiting`, always, while there is one. `OVERDUE`
/// orders `unawaited` alone. Fourteen chunks two players have are stale for six
/// seconds, more than `OVERDUE`, while no slot is free; then one of the players walks
/// on and waits for a column of chunks without light. With one slot, every one of the
/// column is dispatched before any of the fourteen: nearest to the one who waits
/// first, and of two equally near the one that came first.
#[tokio::test(start_paused = true)]
async fn a_waited_for_chunk_goes_before_every_chunk_nobody_waits_for_however_long_overdue() {
    let mut run = Run::start(NO_WAITS).await;
    let (mut stays, mut walks) = (run.script.join(1).await, run.script.join(2).await);
    for place in home_view() {
        run.script.snapshot(WEST, place, ground());
    }
    run.quiet().await;
    run.script.sync(&mut stays).await;
    run.script.sync(&mut walks).await;
    run.script.held().set_in_flight(0);
    // In their second column from the west: these reach the chunks west, north and
    // south of each, and none to the east.
    let lamps: Vec<_> = (-3..=3)
        .map(|z| (at(p(-2, z), 1, FLOOR, 8), blocks::GLOWSTONE))
        .collect();
    run.script.change(WEST, &lamps);
    run.quiet().await;
    run.pass(ms(6000)).await;
    assert!(run.jobs.is_empty());

    run.script.walk(player(2), p(1, 0));
    run.quiet().await;
    let column: Vec<_> = (-3..=3).map(|z| p(4, z)).collect();
    for place in &column {
        run.script.snapshot(WEST, *place, unlit(ground()));
    }
    run.quiet().await;
    assert!(run.jobs.is_empty());
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    let order = run.one_by_one().await;

    // Stable, so of two equally near the one that came first stays first.
    let mut due = column.clone();
    due.sort_by_key(|place| far(*place, p(1, 0)));
    assert_eq!(order[..7], due, "{order:?}");
    let others: BTreeSet<_> = order[7..].iter().copied().collect();
    let stale: BTreeSet<_> = (-3..=3).flat_map(|z| [p(-3, z), p(-2, z)]).collect();
    assert_eq!(others, stale, "{order:?}");
    assert_eq!(order.len(), 21, "{order:?}");
    run.script.sync(&mut walks).await;
    for place in &column {
        assert!(run.holds_the_rules(&walks, *place), "{place:?}");
    }
    run.script.end().await;
}

/// The chunks a lamp in the middle of [`HOME`] marks: it and the four beside it.
fn near() -> BTreeSet<ChunkPos> {
    BTreeSet::from([HOME, p(-1, 0), p(1, 0), p(0, -1), p(0, 1)])
}

/// The chunks a lamp in the second column of the westernmost chunk of the row marks:
/// it and those north and south of it. Nothing east of it is reached, and nothing is
/// west of it.
fn distant() -> BTreeSet<ChunkPos> {
    BTreeSet::from([p(-3, -1), p(-3, 0), p(-3, 1)])
}

fn mark_near(run: &mut Run) {
    run.script
        .change(WEST, &[(at(HOME, 8, FLOOR, 8), blocks::GLOWSTONE)]);
}

fn mark_distant(run: &mut Run) {
    run.script
        .change(WEST, &[(at(p(-3, 0), 1, FLOOR, 8), blocks::GLOWSTONE)]);
}

/// Section 4, dispatch, the third rule: among the chunks nobody waits for, the first
/// by rank, which is the distance to the nearest viewer. The distant ones were marked
/// first and still go last.
#[tokio::test(start_paused = true)]
async fn among_chunks_nobody_waits_for_the_nearest_to_a_viewer_goes_first() {
    let (mut run, _client) = a_view_of_ground(NO_WAITS).await;
    run.script.held().set_in_flight(0);
    mark_distant(&mut run);
    run.quiet().await;
    run.pass(ms(1000)).await;
    mark_near(&mut run);
    run.quiet().await;
    run.pass(ms(1000)).await;
    assert!(run.jobs.is_empty());
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    let order = run.one_by_one().await;
    let all: BTreeSet<_> = order.iter().copied().collect();
    assert_eq!(all, near().union(&distant()).copied().collect());
    assert_eq!(order.len(), 8, "{order:?}");
    for pair in order.windows(2) {
        assert!(far(pair[0], HOME) <= far(pair[1], HOME), "{order:?}");
    }
    run.script.end().await;
}

/// Section 4, dispatch, the second rule: the oldest of `unawaited` goes first if its
/// `marked_since` is more than `OVERDUE` ago.
#[tokio::test(start_paused = true)]
async fn a_chunk_nobody_waits_for_goes_before_nearer_ones_once_it_is_overdue() {
    for overdue in [ms(5000), ms(1000)] {
        a_chunk_goes_before_nearer_ones_when_it_is_overdue_by(overdue).await;
    }
}

async fn a_chunk_goes_before_nearer_ones_when_it_is_overdue_by(overdue: Duration) {
    let light = LightSettings {
        overdue,
        ..NO_WAITS
    };
    let (mut run, _client) = a_view_of_ground(light).await;
    run.script.held().set_in_flight(0);
    mark_distant(&mut run);
    run.quiet().await;
    run.pass(overdue + ms(500)).await;
    mark_near(&mut run);
    run.quiet().await;
    assert!(run.jobs.is_empty());
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    let order = run.one_by_one().await;
    assert_eq!(order.len(), 8, "{order:?}");
    let first: BTreeSet<_> = order[..3].iter().copied().collect();
    assert_eq!(first, distant(), "{order:?}");
    assert_eq!(order[3], HOME, "{order:?}");
    for pair in order[3..].windows(2) {
        assert!(far(pair[0], HOME) <= far(pair[1], HOME), "{order:?}");
    }
    run.script.end().await;
}

/// The same, where the distant chunks become overdue while the nearer ones are worked
/// off: the nearest goes first, and after `OVERDUE` the distant ones go before the
/// rest of the near.
#[tokio::test(start_paused = true)]
async fn chunks_that_become_overdue_while_nearer_ones_are_lit_go_before_the_rest_of_them() {
    let (mut run, _client) = a_view_of_ground(NO_WAITS).await;
    run.script.held().set_in_flight(0);
    mark_distant(&mut run);
    run.quiet().await;
    run.pass(ms(4000)).await;
    mark_near(&mut run);
    run.quiet().await;
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    run.quiet().await;
    assert_eq!(places(&run.out()), [HOME]);
    run.pass(ms(1500)).await;
    let order = run.one_by_one().await;
    assert_eq!(order.len(), 8, "{order:?}");
    assert_eq!(order[0], HOME);
    let next: BTreeSet<_> = order[1..4].iter().copied().collect();
    assert_eq!(next, distant(), "{order:?}");
    let last: BTreeSet<_> = order[4..].iter().copied().collect();
    let mut rest = near();
    rest.remove(&HOME);
    assert_eq!(last, rest, "{order:?}");
    run.script.end().await;
}

/// Several players and the chunks they wait for: what the order among the waited-for
/// is held against.
struct Crowd {
    run: Run,
    clients: BTreeMap<u128, Client>,
    /// The chunk each player is in.
    centres: BTreeMap<u128, ChunkPos>,
    /// For each chunk of the replica, the how-manieth it was to come.
    age: BTreeMap<ChunkPos, usize>,
    served: usize,
    dice: Dice,
}

impl Crowd {
    /// No job is dispatched until a test opens a slot.
    async fn start(light: LightSettings, seed: u64) -> Self {
        let mut run = Run::start(light).await;
        run.script.held().set_in_flight(0);
        Self {
            run,
            clients: BTreeMap::new(),
            centres: BTreeMap::new(),
            age: BTreeMap::new(),
            served: 0,
            dice: Dice(seed),
        }
    }

    async fn join(&mut self, number: u128) {
        let client = self.run.script.join(number).await;
        self.clients.insert(number, client);
        self.centres.insert(number, HOME);
        self.run.quiet().await;
    }

    async fn walk(&mut self, number: u128, to: ChunkPos) {
        self.run.script.walk(player(number), to);
        self.centres.insert(number, to);
        self.run.quiet().await;
        self.forget();
    }

    /// The player's connection ends from the client's side.
    async fn leave(&mut self, number: u128) {
        let mut client = self.clients.remove(&number).expect("the player joined");
        self.run.script.leave(&mut client).await;
        self.centres.remove(&number);
        self.run.quiet().await;
        self.forget();
    }

    /// A chunk nobody wants is gone, and is new when it comes again.
    fn forget(&mut self) {
        let wanted = self.run.script.model().wanted();
        self.age.retain(|place, _| wanted.contains(place));
    }

    /// The west sends `chunk` for `place`.
    fn serve(&mut self, place: ChunkPos, chunk: Chunk) {
        self.run.script.snapshot(WEST, place, chunk);
        self.age.insert(place, self.served);
        self.served += 1;
    }

    /// The west sends every chunk it is asked for and has not sent, without light, in
    /// an order that turns on the seed.
    async fn serve_the_missing(&mut self) {
        let model = self.run.script.model();
        let asked = self.run.script.asked(WEST).into_iter();
        let missing: Vec<_> = asked.filter(|place| !model.has(*place)).collect();
        for place in self.dice.shuffled(missing) {
            self.serve(place, unlit(ground()));
        }
        self.run.quiet().await;
    }

    /// The record's rank of a chunk someone waits for: the squared distance to the
    /// centre of the nearest player whose view has it. Nothing here was sent to
    /// anyone, so whoever views a chunk waits for it.
    fn rank(&self, place: ChunkPos) -> i64 {
        let waiters = self.centres.values();
        let waiters = waiters.filter(|centre| view(**centre, VIEW).contains(&place));
        let rank = waiters.map(|centre| far(place, *centre)).min();
        rank.unwrap_or_else(|| panic!("nobody waits for {place:?}"))
    }

    /// One job may be out at a time from now on; the jobs are answered one by one
    /// until none is out. Returns the chunks in the order of their dispatch.
    async fn open(&mut self) -> Vec<ChunkPos> {
        self.run.script.held().set_in_flight(1);
        self.run.script.turn().await;
        self.run.one_by_one().await
    }

    /// Section 4: among themselves the waited-for go nearest first as the views stand
    /// now, and among the equally near, first come, first served.
    fn holds_the_order(&self, order: &[ChunkPos]) {
        let key = |place: ChunkPos| (self.rank(place), self.age[&place]);
        for pair in order.windows(2) {
            assert!(
                key(pair[0]) < key(pair[1]),
                "{:?} ({:?}) was dispatched before {:?} ({:?}); the players are in {:?}; \
                 the order: {order:?}",
                pair[0],
                key(pair[0]),
                pair[1],
                key(pair[1]),
                self.centres
            );
        }
    }

    /// Every chunk of the replica was dispatched once, and every player has their
    /// view with the light of the rules.
    async fn all_were_lit_once_and_sent(mut self, order: &[ChunkPos]) {
        let once: BTreeSet<_> = order.iter().copied().collect();
        assert_eq!(once.len(), order.len(), "{order:?}");
        assert_eq!(once, self.run.script.model().positions());
        for (number, client) in &mut self.clients {
            self.run.script.sync(client).await;
            let due = self.run.script.model().view(player(*number));
            assert_eq!(client.chunks(), due, "player {number}");
            for place in due {
                assert!(self.run.holds_the_rules(client, place), "{place:?}");
            }
        }
        self.run.script.end().await;
    }
}

/// Section 4, rank: the squared distance from the chunk to the centre of its nearest
/// waiter, kept up when a view's centre moves; among equal ranks, first come, first
/// served. Two players wait for a view that came in a random order; one walks two
/// chunks east and the other one south, new chunks coming for each; then one slot is
/// opened.
#[tokio::test(start_paused = true)]
async fn waited_for_chunks_go_nearest_to_their_nearest_waiter_as_the_views_stand_and_then_oldest() {
    for seed in 1..=3 {
        let mut crowd = Crowd::start(NO_WAITS, seed).await;
        crowd.join(1).await;
        crowd.join(2).await;
        crowd.serve_the_missing().await;
        for to in [p(1, 0), p(2, 0)] {
            crowd.walk(2, to).await;
            crowd.serve_the_missing().await;
        }
        crowd.walk(1, p(0, 1)).await;
        crowd.serve_the_missing().await;
        assert!(crowd.run.jobs.is_empty());
        let order = crowd.open().await;
        crowd.holds_the_order(&order);
        crowd.all_were_lit_once_and_sent(&order).await;
    }
}

/// Section 4, "where entries are moved": when a waiter is added or leaves. A third
/// player joins where chunks already wait and is the nearest waiter of some; a player
/// walks back; and where the third then leaves again, the order is that of the two who
/// are left.
#[tokio::test(start_paused = true)]
async fn a_waiter_who_joins_late_or_leaves_is_counted_in_the_order_as_the_views_then_stand() {
    for leaves in [false, true] {
        let mut crowd = Crowd::start(NO_WAITS, 7).await;
        crowd.join(1).await;
        crowd.join(2).await;
        for to in [p(1, 0), p(2, 0), p(3, 0)] {
            crowd.walk(2, to).await;
        }
        crowd.walk(1, p(-1, 0)).await;
        crowd.walk(1, p(-2, 0)).await;
        crowd.serve_the_missing().await;
        // In the middle, between the two.
        crowd.join(3).await;
        crowd.serve_the_missing().await;
        crowd.walk(2, p(2, 0)).await;
        if leaves {
            crowd.leave(3).await;
        }
        assert!(crowd.run.jobs.is_empty());
        let order = crowd.open().await;
        crowd.holds_the_order(&order);
        crowd.all_were_lit_once_and_sent(&order).await;
    }
}

// ---------------------------------------------------------------------------------
// 5. The patience with a thread
// ---------------------------------------------------------------------------------

/// A chunk under a roof, without light: what the heights give is not what a flood
/// gives beside walls.
fn roofed() -> Chunk {
    roof(unlit(ground()), FLOOR + 3, (0, 0), (15, 15))
}

/// A player waits for [`HOME`], whose job was dispatched at the start, said to be
/// started twenty seconds later, and given up for not being answered.
async fn a_job_given_up() -> (Run, Client, LightJob) {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, roofed());
    run.quiet().await;
    let job = run.out_for(HOME);

    // In the queue behind others, as far as the edge knows: never given up.
    run.pass(ms(20_000)).await;
    run.script.sync(&mut client).await;
    assert!(!client.has(HOME), "given up without a thread having it");
    assert_eq!(run.jobs.len(), 1);

    run.script.held().started(job.basis);
    run.script.turn().await;
    run.pass(ms(4800)).await;
    run.script.sync(&mut client).await;
    assert!(!client.has(HOME), "given up before the patience ended");
    run.pass(ms(400)).await;
    run.script.sync(&mut client).await;
    assert!(client.has(HOME), "not given up after the patience");
    (run, client, job)
}

/// Section 5: `JOB_PATIENCE` runs from the word `Started`, not from the dispatch, so
/// a job that waits in the queue behind others is never given up. A job that is given
/// up is a failed one: those who wait are sent the chunk with the light of its
/// heights, and no job is made for it again until something marks it.
#[tokio::test(start_paused = true)]
async fn a_job_is_given_up_a_patience_after_it_was_started_and_the_chunk_sent_lit_by_heights() {
    let (mut run, mut client, _job) = a_job_given_up().await;
    assert_eq!(client.light(HOME), Some(&as_held(&by_heights(&roofed()))));
    run.pass(ms(12_000)).await;
    run.script.sync(&mut client).await;
    assert_eq!(run.jobs.len(), 1, "a job for a chunk that nothing marked");
    assert_eq!(client.told_of(HOME), [Told::Sent(HOME)]);
    run.script.end().await;
}

/// Section 5: a result whose basis was given up is late, and dropped. Not when it
/// comes while nothing else is under way, and not when it comes under the chunk's next
/// job, whose result is the one that is taken, with every section named.
#[tokio::test(start_paused = true)]
async fn a_late_answer_to_a_job_that_was_given_up_changes_nothing() {
    let (mut run, mut client, job) = a_job_given_up().await;
    let outcome = run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.told_of(HOME), [Told::Sent(HOME)]);
    assert_eq!(client.light(HOME), Some(&as_held(&by_heights(&roofed()))));
    assert_eq!(run.jobs.len(), 1);

    // Something marks the chunk: now it gets a job, which names every section.
    let torch = at(HOME, 3, FLOOR, 3);
    run.script.change(WEST, &[(torch, blocks::TORCH)]);
    run.quiet().await;
    let next = run.out_for(HOME);
    assert!(next.whole, "the job after a failed one names every section");
    // The old job's word once more, under the new job.
    run.script.held().say(FromThread::Done {
        basis: job.basis,
        position: HOME,
        outcome,
    });
    run.script.turn().await;
    run.quiet().await;
    run.script.sync(&mut client).await;
    assert_eq!(lights(&client, HOME), 0, "a late result was taken");
    assert_eq!(places(&run.out()), [HOME]);
    assert_eq!(run.jobs.len(), 2);

    run.answer(next.basis).await;
    run.script.sync(&mut client).await;
    assert_eq!(
        client.told_of(HOME).len(),
        3,
        "the chunk, the block and one light update: {:?}",
        client.told_of(HOME)
    );
    assert_eq!(lights(&client, HOME), 1);
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(client.times_sent(HOME), 1);
    run.script.end().await;
}

/// Section 5: a `Fallback` entry is never kept by a neighbour's coming: a new
/// neighbour is a new input, and it gets a job, whose result goes to those who have
/// the chunk with every section named.
#[tokio::test(start_paused = true)]
async fn a_chunk_whose_job_was_given_up_gets_its_next_job_when_a_neighbour_comes() {
    let (mut run, mut client, job) = a_job_given_up().await;
    // Its thread never answers.
    run.script.held().never(job.basis);
    run.script.snapshot(WEST, p(1, 0), ground());
    run.quiet().await;
    let next = run.out_for(HOME);
    assert!(next.whole, "the job after a failed one names every section");
    assert_eq!(there(&next), 2);
    run.answer_all().await;
    run.script.sync(&mut client).await;
    assert_eq!(lights(&client, HOME), 1);
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(client.times_sent(HOME), 1);
    assert_eq!(run.count(HOME), 2);
    run.script.end().await;
}

/// Section 5, a failed job: those who have the chunk are sent nothing, and the result
/// of the next job, which something has to mark the chunk for, names every section.
#[tokio::test(start_paused = true)]
async fn those_who_have_a_chunk_whose_job_is_given_up_are_sent_nothing_until_the_next_job() {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, ground());
    run.quiet().await;
    run.script.sync(&mut client).await;
    let lamp = at(HOME, 3, FLOOR, 3);
    run.script.change(WEST, &[(lamp, blocks::GLOWSTONE)]);
    run.quiet().await;
    let job = run.out_for(HOME);
    run.script.held().started(job.basis);
    run.script.turn().await;
    run.pass(ms(5200)).await;
    run.pass(ms(10_000)).await;
    run.script.sync(&mut client).await;
    assert_eq!(
        client.told_of(HOME),
        [Told::Sent(HOME), Told::Block(lamp, blocks::GLOWSTONE)]
    );
    assert_eq!(run.jobs.len(), 1, "a job for a chunk that nothing marked");

    let torch = at(HOME, 12, FLOOR, 12);
    run.script.change(WEST, &[(torch, blocks::TORCH)]);
    run.quiet().await;
    assert_eq!(run.jobs.len(), 2, "the job that was out was not given up");
    let next = run
        .out()
        .into_iter()
        .find(|out| out.basis != job.basis)
        .expect("a job for the chunk that was marked");
    assert_eq!(next.position, HOME);
    assert!(next.whole, "the job after a failed one names every section");
    run.answer(next.basis).await;
    run.script.sync(&mut client).await;
    assert_eq!(lights(&client, HOME), 1);
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(client.times_sent(HOME), 1);
    run.script.end().await;
}

/// Section 5: a forgotten job counts against `IN_FLIGHT` until it answers or is given
/// up. With one slot, a chunk whose job a thread has and does not answer keeps the
/// next chunk's job back for the patience, and no longer; a job no thread has taken
/// keeps it back for as long as it is out.
#[tokio::test(start_paused = true)]
async fn a_job_that_is_given_up_frees_its_slot_for_the_next_chunk() {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    run.script.held().set_in_flight(1);
    let mut client = run.script.join(1).await;
    let apart = p(2, 0);
    run.script.snapshot(WEST, HOME, roofed());
    run.script.snapshot(WEST, apart, unlit(ground()));
    run.quiet().await;
    let job = run.out_for(HOME);
    assert_eq!(run.jobs.len(), 1);
    run.pass(ms(20_000)).await;
    assert_eq!(run.jobs.len(), 1, "a second job with one slot");

    run.script.held().started(job.basis);
    run.script.turn().await;
    run.pass(ms(4800)).await;
    assert_eq!(run.jobs.len(), 1, "given up before the patience ended");
    run.pass(ms(400)).await;
    let times = run.times(apart);
    assert_eq!(times.len(), 1, "the slot of a job given up was not freed");
    assert!(
        ms(25_000) <= times[0] && times[0] <= ms(25_100),
        "{times:?}"
    );
    let next = run.out_for(apart);
    run.answer(next.basis).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.light(HOME), Some(&as_held(&by_heights(&roofed()))));
    assert!(run.holds_the_rules(&client, apart));
    assert_eq!(run.jobs.len(), 2);
    run.script.end().await;
}

/// Section 5: a job that is answered within the patience is not given up, and nothing
/// comes of the patience's end afterwards.
#[tokio::test(start_paused = true)]
async fn a_job_answered_within_the_patience_is_taken_and_nothing_is_given_up_later() {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, roofed());
    run.quiet().await;
    let job = run.out_for(HOME);
    run.script.held().started(job.basis);
    run.script.turn().await;
    run.pass(ms(4000)).await;
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert!(run.holds_the_rules(&client, HOME));
    run.pass(ms(8000)).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.told_of(HOME), [Told::Sent(HOME)]);
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(run.jobs.len(), 1);
    run.script.end().await;
}

// ---------------------------------------------------------------------------------
// 6. No thread left
// ---------------------------------------------------------------------------------

/// Section 5: with no thread left, a job fails at its dispatch, in the dispatching
/// turn: every chunk gets the fallback at once and nothing waits `JOB_PATIENCE`. Here
/// nothing is held, so every chunk of a join is sent in the turn it comes.
#[tokio::test(start_paused = true)]
async fn with_no_thread_left_every_chunk_is_sent_as_it_comes_lit_by_its_heights() {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    run.script.held().shut(true);
    let mut client = run.script.join(1).await;
    for place in Dice(11).shuffled(home_view()) {
        run.script.snapshot(WEST, place, terrain(place));
        run.quiet().await;
        run.script.sync(&mut client).await;
        assert!(client.has(place), "{place:?} waits for something");
    }
    assert_eq!(run.now(), Duration::ZERO);
    assert!(run.out().is_empty());
    assert!(run.jobs.is_empty());
    for place in home_view() {
        assert_eq!(client.times_sent(place), 1, "{place:?}");
        let due = as_held(&by_heights(&terrain(place)));
        assert_eq!(client.light(place), Some(&due), "{place:?}");
    }
    run.script.end().await;
}

/// The same with the server's settings: a chunk is held for its wanted neighbours as
/// ever, and sent, lit by its heights, in the turn the last of them comes. No time has
/// to pass for a join.
#[tokio::test(start_paused = true)]
async fn with_no_thread_left_a_join_is_sent_as_its_holds_end_and_waits_for_no_patience() {
    for seed in [3, 4] {
        let mut run = Run::start(LightSettings::SERVER).await;
        run.script.held().shut(true);
        let mut client = run.script.join(1).await;
        let wanted = view(HOME, VIEW);
        let mut came = BTreeSet::new();
        for place in Dice(seed).shuffled(home_view()) {
            run.script.snapshot(WEST, place, terrain(place));
            came.insert(place);
            run.quiet().await;
            run.script.sync(&mut client).await;
            let whole = |place: &&ChunkPos| {
                let mut beside = around(**place).into_iter();
                beside.all(|other| !wanted.contains(&other) || came.contains(&other))
            };
            let due: BTreeSet<_> = came.iter().filter(whole).copied().collect();
            assert_eq!(client.chunks(), due, "after {place:?} came");
        }
        assert_eq!(client.chunks(), wanted);
        assert_eq!(run.now(), Duration::ZERO);
        assert!(run.jobs.is_empty());
        for place in home_view() {
            assert_eq!(client.times_sent(place), 1, "{place:?}");
            let due = as_held(&by_heights(&terrain(place)));
            assert_eq!(client.light(place), Some(&due), "{place:?}");
        }
        run.script.end().await;
    }
}

// ---------------------------------------------------------------------------------
// 7. A player removed in the middle
// ---------------------------------------------------------------------------------

/// How a player goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Goes {
    /// Their queue is full when the edge has a packet for them.
    Full,
    /// Their connection ends from the client's side.
    Leaves,
}

/// Section 4, R1 to R3. Two players wait: one at [`HOME`] and one two chunks east,
/// who is the nearest waiter of everything from there eastwards. All of the first
/// one's view has come and nothing of the east beyond it, so the easternmost column is
/// held for chunks only the second player wants, and the rest is queued. With one
/// slot, the first job is for the second player's own chunk; they are removed when its
/// result is fanned out, or leave while it is under way. The one who stays is sent
/// that chunk; what was queued is dispatched nearest to them first and then in the
/// order in which the holds ended; what was held is dispatched when its hold ends; and they have their whole
/// view, every chunk lit once.
async fn a_player_who_goes_in_the_middle_leaves_the_order_to_the_other(goes: Goes) {
    let waits = LightSettings {
        rest: Duration::ZERO,
        job_patience: FOREVER,
        ..LightSettings::SERVER
    };
    let mut crowd = Crowd::start(waits, 0).await;
    crowd.join(1).await;
    crowd.join(2).await;
    crowd.walk(2, p(1, 0)).await;
    crowd.walk(2, p(2, 0)).await;
    // The second player's own chunk and the eight around it come first, so that its
    // hold is the first to end.
    crowd.serve(p(2, 0), unlit(ground()));
    for place in around(p(2, 0)) {
        crowd.serve(place, terrain(place));
    }
    for place in home_view() {
        if !crowd.age.contains_key(&place) {
            crowd.serve(place, terrain(place));
        }
    }
    crowd.run.quiet().await;
    assert!(crowd.run.jobs.is_empty());
    // A chunk is put among those that are due when its hold ends, which is when the
    // last of its wanted neighbours has come: that is its age from here on. Several
    // holds that end with one chunk's coming are of one age.
    let wanted = crowd.run.script.model().wanted();
    let came = crowd.age.clone();
    for (place, age) in &mut crowd.age {
        let beside = around(*place).into_iter();
        let beside = beside.filter(|other| wanted.contains(other));
        let last = beside.filter_map(|other| came.get(&other)).max();
        *age = (*age).max(*last.expect("a chunk beside it has come"));
    }
    let mut gone = crowd.clients.remove(&2).expect("the second player joined");
    if goes == Goes::Full {
        assert!(gone.fill() > 0);
    }
    crowd.run.script.held().set_in_flight(1);
    crowd.run.script.turn().await;
    crowd.run.quiet().await;
    let first = crowd.run.out();
    assert_eq!(places(&first), [p(2, 0)]);
    match goes {
        Goes::Full => {
            crowd.run.script.held().answer(first[0].basis);
            crowd.run.script.turn().await;
            crowd.run.script.removed(&mut gone).await;
        }
        Goes::Leaves => {
            crowd.run.script.leave(&mut gone).await;
            crowd.run.script.held().answer(first[0].basis);
            crowd.run.script.turn().await;
        }
    }
    crowd.centres.remove(&2);

    // What was queued, with the clock standing still.
    let order = crowd.run.one_by_one().await;
    assert_eq!(crowd.run.now(), Duration::ZERO);
    for pair in order.windows(2) {
        assert!(
            crowd.rank(pair[0]) <= crowd.rank(pair[1]),
            "{goes:?}: {order:?}"
        );
    }
    let queued: Vec<_> = order.iter().copied().filter(|place| place.x <= 2).collect();
    for pair in queued.windows(2) {
        let key = |place: ChunkPos| (crowd.rank(place), crowd.age[&place]);
        assert!(
            key(pair[0]) <= key(pair[1]),
            "{goes:?}: {pair:?} of {order:?}"
        );
    }
    let due: BTreeSet<_> = view(HOME, VIEW)
        .into_iter()
        .filter(|place| place.x <= 2 && *place != p(2, 0))
        .collect();
    assert_eq!(queued.iter().copied().collect::<BTreeSet<_>>(), due);

    // What was held for chunks that nobody wants any more: `LATEST` at most.
    crowd.run.pass(ms(1100)).await;
    let late = crowd.run.one_by_one().await;
    for pair in late.windows(2) {
        assert!(
            crowd.rank(pair[0]) <= crowd.rank(pair[1]),
            "{goes:?}: {late:?}"
        );
    }
    let mut all = vec![p(2, 0)];
    all.extend(order);
    all.extend(late);
    crowd.all_were_lit_once_and_sent(&all).await;
}

#[tokio::test(start_paused = true)]
async fn a_player_removed_for_a_full_queue_at_a_result_leaves_the_order_to_the_other() {
    a_player_who_goes_in_the_middle_leaves_the_order_to_the_other(Goes::Full).await;
}

#[tokio::test(start_paused = true)]
async fn a_player_who_leaves_under_the_job_they_were_nearest_to_leaves_the_order_to_the_other() {
    a_player_who_goes_in_the_middle_leaves_the_order_to_the_other(Goes::Leaves).await;
}

// ---------------------------------------------------------------------------------
// What else the record says of times and order
// ---------------------------------------------------------------------------------

/// Section 4: the hold is a setting. With another `latest` a chunk waits that long.
#[tokio::test(start_paused = true)]
async fn the_hold_lasts_as_long_as_the_setting_says() {
    for latest in [ms(300), ms(2500)] {
        let light = LightSettings {
            latest,
            ..LightSettings::SERVER
        };
        let mut run = Run::start(light).await;
        let _client = run.script.join(1).await;
        run.script.snapshot(WEST, HOME, unlit(ground()));
        run.quiet().await;
        run.pass(latest - ms(50)).await;
        assert!(run.jobs.is_empty(), "{latest:?}");
        run.pass(ms(150)).await;
        let times = run.times(HOME);
        assert_eq!(times.len(), 1, "{latest:?}: {times:?}");
        assert!(latest <= times[0], "{latest:?}: {times:?}");
        run.script.end().await;
    }
}

/// "As built": the task sleeps until the first hold ends, and never wakes for light
/// sooner than 50 ms after it last did. Chunks apart from each other come at different
/// times, none with a neighbour: each is dispatched when its own hold ends, also one
/// whose hold ends a moment after another's and one whose hold began just before the
/// first ended.
#[tokio::test(start_paused = true)]
async fn holds_that_begin_at_different_times_end_each_at_its_own() {
    let mut run = Run::start(LightSettings::SERVER).await;
    // None of the jobs is answered, and none is to wait for a slot.
    run.script.held().set_in_flight(64);
    let _client = run.script.join(1).await;
    let comes = [
        (HOME, ms(0)),
        (p(2, 0), ms(25)),
        (p(-2, 0), ms(300)),
        (p(0, 2), ms(975)),
        (p(0, -2), ms(1500)),
    ];
    for (place, when) in comes {
        let wait = when - run.now();
        run.pass(wait).await;
        run.script.snapshot(WEST, place, unlit(ground()));
        run.quiet().await;
    }
    run.pass(ms(1200)).await;
    for (place, when) in comes {
        let times = run.times(place);
        assert_eq!(times.len(), 1, "{place:?}: {times:?}");
        let waited = times[0] - when;
        assert!(
            ms(1000) <= waited && waited <= ms(1100),
            "{place:?} came at {when:?} and was dispatched at {times:?}"
        );
    }
    run.script.end().await;
}

/// L4, and section 5's table, the third row: a chunk needs one job for each time a
/// chunk came in its three by three while a job for it was under way, and one more.
/// The result of the first is not sendable and those who wait go on waiting; the next
/// job is held only while `unsendable_since` allows, which was not set anew: it is
/// dispatched in the turn the first is answered.
#[tokio::test(start_paused = true)]
async fn a_neighbour_that_comes_under_a_job_brings_the_next_job_at_once_and_no_new_hold() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, unlit(ground()));
    run.quiet().await;
    run.pass(ms(1100)).await;
    let first = run.out_for(HOME);
    run.pass(ms(100)).await;
    run.script.snapshot(WEST, p(1, 0), ground());
    run.quiet().await;
    assert_eq!(run.count(HOME), 1, "a chunk has one job at a time");
    run.pass(ms(100)).await;
    let now = run.now();
    run.answer(first.basis).await;
    let times = run.times(HOME);
    assert_eq!(times[1..], [now], "{times:?}");
    run.script.sync(&mut client).await;
    assert!(!client.has(HOME), "sent with light made beside a wall");
    let second = run.out_for(HOME);
    assert_eq!(there(&second), 2);
    run.answer(second.basis).await;
    run.script.sync(&mut client).await;
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(client.told_of(HOME), [Told::Sent(HOME)]);
    assert_eq!(run.count(HOME), 2);
    run.script.end().await;
}

/// Section 4: it is a stale entry that rests. One that rests and beside which chunks
/// then come cannot be sent any more: it is held for the neighbours that are wanted,
/// and dispatched when the last of them is in, long before its rest would have ended.
#[tokio::test(start_paused = true)]
async fn a_resting_chunk_beside_which_chunks_come_is_dispatched_when_they_are_all_in() {
    let slow = LightSettings {
        rest: Duration::from_secs(10),
        ..LightSettings::SERVER
    };
    let mut run = Run::start(slow).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, ground());
    run.quiet().await;
    let mut count = 0;
    flicker(&mut run, &mut count);
    run.quiet().await;
    let job = run.out_for(HOME);
    run.answer(job.basis).await;
    flicker(&mut run, &mut count);
    run.quiet().await;
    run.pass(ms(500)).await;
    assert_eq!(run.count(HOME), 1, "it rests");
    for place in around(HOME) {
        assert_eq!(run.count(HOME), 1, "dispatched before {place:?} came");
        run.script.snapshot(WEST, place, ground());
        run.quiet().await;
    }
    assert_eq!(run.times(HOME), [Duration::ZERO, ms(500)]);
    run.answer_all().await;
    run.script.sync(&mut client).await;
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(client.times_sent(HOME), 1);
    run.script.end().await;
}

/// Section 4: `unawaited` has the stale and the unsendable that nobody waits for (a
/// neighbour came beside a chunk everyone has), and nothing of it comes before a chunk
/// someone waits for. A player has their view with computed light and walks east: the
/// new column is waited for, and its coming marks the column beside it, which the
/// player has and stands nearer to. With one slot the new column goes first.
#[tokio::test(start_paused = true)]
async fn a_chunk_everyone_has_beside_which_one_came_goes_after_those_someone_waits_for() {
    let mut run = Run::start(NO_WAITS).await;
    let mut client = run.script.join(1).await;
    for place in home_view() {
        run.script.snapshot(WEST, place, unlit(ground()));
    }
    run.answer_all().await;
    run.script.sync(&mut client).await;
    assert_eq!(client.chunks(), view(HOME, VIEW));
    let before = run.jobs.len();
    run.script.held().set_in_flight(0);
    run.script.walk(player(1), p(1, 0));
    run.quiet().await;
    let column: Vec<_> = (-3..=3).map(|z| p(4, z)).collect();
    for place in &column {
        run.script.snapshot(WEST, *place, unlit(ground()));
    }
    run.quiet().await;
    assert_eq!(run.jobs.len(), before);
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    let order = run.one_by_one().await;
    let mut due = column.clone();
    due.sort_by_key(|place| far(*place, p(1, 0)));
    assert_eq!(order[..7], due, "{order:?}");
    let others: BTreeSet<_> = order[7..].iter().copied().collect();
    let beside: BTreeSet<_> = (-3..=3).map(|z| p(3, z)).collect();
    assert_eq!(others, beside, "{order:?}");
    assert_eq!(order.len(), 14, "{order:?}");
    run.script.sync(&mut client).await;
    assert_eq!(client.chunks(), view(p(1, 0), VIEW));
    for place in column.iter().chain(&beside) {
        assert!(run.holds_the_rules(&client, *place), "{place:?}");
        assert_eq!(client.times_sent(*place), 1, "{place:?}");
    }
    run.script.end().await;
}

/// Section 5: each job has the patience from the word of its own thread.
#[tokio::test(start_paused = true)]
async fn two_jobs_started_at_different_times_are_given_up_each_at_its_own() {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    let mut client = run.script.join(1).await;
    let apart = p(2, 0);
    run.script.snapshot(WEST, HOME, roofed());
    run.script.snapshot(WEST, apart, roofed());
    run.quiet().await;
    let (one, other) = (run.out_for(HOME), run.out_for(apart));
    run.script.held().started(one.basis);
    run.script.turn().await;
    run.pass(ms(2000)).await;
    run.script.held().started(other.basis);
    run.script.turn().await;
    run.pass(ms(2800)).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.chunks(), BTreeSet::new());
    run.pass(ms(400)).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.chunks(), BTreeSet::from([HOME]));
    run.pass(ms(1600)).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.chunks(), BTreeSet::from([HOME]));
    run.pass(ms(400)).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.chunks(), BTreeSet::from([HOME, apart]));
    assert_eq!(client.light(apart), Some(&as_held(&by_heights(&roofed()))));
    assert_eq!(run.jobs.len(), 2);
    run.script.end().await;
}

/// Section 5: arrive forgets the job under way, and a forgotten job still counts
/// against `IN_FLIGHT` until it answers or is given up. A chunk is replaced while a
/// thread has its job and does not answer. Giving that job up is nothing to the chunk
/// that is there now: nobody is sent it with the light of its heights, and its own job
/// is answered and taken. With one slot that job waits for the old one to be given up.
#[tokio::test(start_paused = true)]
async fn giving_up_the_job_of_a_chunk_that_was_replaced_frees_a_slot_and_sends_nothing() {
    for slots in [1, 4] {
        let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
        run.script.held().set_in_flight(slots);
        let mut client = run.script.join(1).await;
        run.script.snapshot(WEST, HOME, roofed());
        run.quiet().await;
        let old = run.out_for(HOME);
        run.script.held().started(old.basis);
        run.script.turn().await;
        run.pass(ms(2000)).await;
        let lamp = unlit(lamp(roofed(), 8, FLOOR, 8));
        run.script.snapshot(WEST, HOME, lamp);
        run.quiet().await;
        // The old job's chunks are let go; the edge goes on counting it.
        run.script.held().never(old.basis);
        if slots == 1 {
            assert_eq!(run.jobs.len(), 1, "a second job with one slot");
        } else {
            assert_eq!(run.times(HOME), [ms(0), ms(2000)]);
        }
        run.pass(ms(2800)).await;
        run.script.sync(&mut client).await;
        assert!(!client.has(HOME));
        run.pass(ms(400)).await;
        run.script.sync(&mut client).await;
        assert!(
            !client.has(HOME),
            "{slots} slots: sent for a job that was forgotten"
        );
        let times = run.times(HOME);
        assert_eq!(times.len(), 2, "{slots} slots: {times:?}");
        if slots == 1 {
            assert!(ms(5000) <= times[1] && times[1] <= ms(5100), "{times:?}");
        }
        let new = run.out_for(HOME);
        run.answer(new.basis).await;
        run.script.sync(&mut client).await;
        assert_eq!(client.told_of(HOME), [Told::Sent(HOME)]);
        assert!(run.holds_the_rules(&client, HOME));
        run.pass(ms(6000)).await;
        assert_eq!(run.jobs.len(), 2);
        run.script.end().await;
    }
}

/// Section 5, the results that are not taken: a chunk that left and came again has no
/// job of the old moment. A thread has the job of a chunk whose only viewer walks away
/// and comes back, so that the chunk comes anew and gets a job of its own; the old job
/// is then given up, which is nothing to the chunk that is there now.
#[tokio::test(start_paused = true)]
async fn giving_up_the_job_of_a_chunk_that_left_and_came_again_sends_nothing() {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, roofed());
    run.quiet().await;
    let old = run.out_for(HOME);
    run.script.held().started(old.basis);
    run.script.turn().await;
    run.pass(ms(1000)).await;
    for x in [1, 2, 3, 4, 3, 2, 1, 0] {
        run.script.walk(player(1), p(x, 0));
        run.quiet().await;
    }
    assert!(!run.script.model().has(HOME));
    run.script.held().never(old.basis);
    run.script.snapshot(WEST, HOME, roofed());
    run.quiet().await;
    assert_eq!(run.times(HOME), [ms(0), ms(1000)]);
    let new = run.out_for(HOME);
    run.pass(ms(4500)).await;
    run.script.sync(&mut client).await;
    assert!(!client.has(HOME), "sent for a job of the chunk that left");
    run.answer(new.basis).await;
    run.script.sync(&mut client).await;
    assert_eq!(client.told_of(HOME), [Told::Sent(HOME)]);
    assert!(run.holds_the_rules(&client, HOME));
    assert_eq!(run.jobs.len(), 2);
    run.script.end().await;
}

/// Section 5, with no thread left, for those who have the chunk: they are sent
/// nothing, no job is made again until something marks the chunk, and with a thread
/// again the next result names every section.
#[tokio::test(start_paused = true)]
async fn with_no_thread_left_those_who_have_a_chunk_are_sent_nothing_and_no_job_is_tried_again() {
    let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, ground());
    run.quiet().await;
    run.script.sync(&mut client).await;
    run.script.held().shut(true);
    let lamp = at(HOME, 3, FLOOR, 3);
    run.script.change(WEST, &[(lamp, blocks::GLOWSTONE)]);
    run.quiet().await;
    run.pass(ms(8000)).await;
    run.script.held().shut(false);
    run.pass(ms(8000)).await;
    run.script.sync(&mut client).await;
    assert!(run.jobs.is_empty(), "a job for a chunk that nothing marked");
    assert_eq!(
        client.told_of(HOME),
        [Told::Sent(HOME), Told::Block(lamp, blocks::GLOWSTONE)]
    );
    let torch = at(HOME, 12, FLOOR, 12);
    run.script.change(WEST, &[(torch, blocks::TORCH)]);
    run.quiet().await;
    let job = run.out_for(HOME);
    assert!(job.whole, "the job after a failed one names every section");
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert_eq!(lights(&client, HOME), 1);
    assert!(run.holds_the_rules(&client, HOME));
    run.script.end().await;
}

/// Section 4, R2 and R3, where what is fanned out is a job given up: one of two who
/// wait has a full queue when the patience ends. They are removed and the other is
/// sent the chunk. Played with each of the two as the one whose queue is full, since
/// the record does not say in which order waiters are sent to.
#[tokio::test(start_paused = true)]
async fn a_waiter_whose_queue_is_full_when_a_job_is_given_up_is_removed_and_the_other_is_sent() {
    for full in [0, 1] {
        let mut run = Run::start(FIVE_SECONDS_A_JOB).await;
        let mut both = [run.script.join(1).await, run.script.join(2).await];
        run.script.snapshot(WEST, HOME, roofed());
        run.quiet().await;
        let job = run.out_for(HOME);
        run.script.held().started(job.basis);
        run.script.turn().await;
        run.pass(ms(4000)).await;
        assert!(both[full].fill() > 0);
        run.pass(ms(1200)).await;
        run.script.removed(&mut both[full]).await;
        assert!(!both[full].has(HOME));
        let other = &mut both[1 - full];
        run.script.sync(other).await;
        assert_eq!(other.light(HOME), Some(&as_held(&by_heights(&roofed()))));
        assert_eq!(run.jobs.len(), 1);
        run.script.end().await;
    }
}

/// The same where the job fails at its dispatch, in the turn the chunk comes.
#[tokio::test(start_paused = true)]
async fn a_waiter_whose_queue_is_full_when_a_job_fails_at_dispatch_is_removed_and_the_other_sent() {
    for full in [0, 1] {
        let mut run = Run::start(LightSettings::SERVER).await;
        let mut both = [run.script.join(1).await, run.script.join(2).await];
        run.script.held().shut(true);
        for place in [p(2, 2), p(2, 3), p(3, 2)] {
            run.script.snapshot(WEST, place, ground());
        }
        run.quiet().await;
        for client in &mut both {
            run.script.sync(client).await;
        }
        assert!(both[full].fill() > 0);
        // The corner's hold ends as it comes: all it waits for is there.
        run.script.snapshot(WEST, p(3, 3), roofed());
        run.quiet().await;
        run.script.removed(&mut both[full]).await;
        let other = &mut both[1 - full];
        run.script.sync(other).await;
        let due = as_held(&by_heights(&roofed()));
        assert_eq!(other.light(p(3, 3)), Some(&due));
        assert_eq!(run.now(), Duration::ZERO);
        run.script.end().await;
    }
}

/// The chunks in the corners of the view from [`HOME`], each with the one block in it
/// whose change reaches no other chunk of the replica: every chunk it would reach is
/// beyond the view.
fn corners() -> [(ChunkPos, (usize, usize)); 4] {
    [
        (p(-3, -3), (0, 0)),
        (p(3, 3), (15, 15)),
        (p(-3, 3), (0, 15)),
        (p(3, -3), (15, 0)),
    ]
}

fn mark_corner(run: &mut Run, corner: usize, state: clustine_data::BlockState) {
    let (place, (x, z)) = corners()[corner];
    let block = at(place, x as i32, FLOOR, z as i32);
    run.script.change(WEST, &[(block, state)]);
}

/// Section 4: the oldest of `unawaited` is the one whose `marked_since` is earliest;
/// `marked_since` is set when an entry stops being current, and not set anew by a
/// block change in an entry that is already marked. Corners equally far from the
/// player are marked a second apart, the first of them once more later, and the
/// chunk the player stands in last of all: those overdue go first, oldest first, and
/// then the rest nearest first.
#[tokio::test(start_paused = true)]
async fn the_overdue_go_oldest_first_by_when_their_light_first_became_wrong() {
    let (mut run, _client) = a_view_of_ground(NO_WAITS).await;
    run.script.held().set_in_flight(0);
    for corner in 0..3 {
        mark_corner(&mut run, corner, blocks::GLOWSTONE);
        run.quiet().await;
        run.pass(ms(1000)).await;
    }
    // The first corner again, while it is marked.
    mark_corner(&mut run, 0, blocks::TORCH);
    run.quiet().await;
    run.pass(ms(500)).await;
    run.script
        .change(WEST, &[(at(HOME, 0, FLOOR, 0), blocks::GLOWSTONE)]);
    run.quiet().await;
    run.pass(ms(3000)).await;
    // The first corner was marked 6.5 s ago, the second 5.5 s and the third 4.5 s.
    assert!(run.jobs.is_empty());
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    let order = run.one_by_one().await;
    let [first, second, third, _] = corners().map(|(place, _)| place);
    assert_eq!(order[..2], [first, second], "{order:?}");
    // The change at HOME reaches the three chunks west and north of it.
    let near = BTreeSet::from([HOME, p(-1, 0), p(0, -1), p(-1, -1)]);
    let next: BTreeSet<_> = order[2..6].iter().copied().collect();
    assert_eq!(next, near, "{order:?}");
    assert_eq!(order[6..], [third], "{order:?}");
    run.script.end().await;
}

/// Section 3, E2: a snapshot that shows the same as the chunk that is there does
/// nothing to light, and so nothing to a hold.
#[tokio::test(start_paused = true)]
async fn the_same_chunk_again_does_not_prolong_its_hold() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let _client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, unlit(ground()));
    run.quiet().await;
    run.pass(ms(600)).await;
    run.script.snapshot(WEST, HOME, unlit(ground()));
    run.quiet().await;
    run.pass(ms(500)).await;
    let times = run.times(HOME);
    assert_eq!(times.len(), 1, "{times:?}");
    assert!(ms(1000) <= times[0] && times[0] <= ms(1100), "{times:?}");
    run.script.end().await;
}

/// Section 3, arrive: `unsendable_since` is not set anew while an entry stays
/// unsendable. A chunk that is replaced (E3) while it is held for its neighbours stays
/// unsendable through it, so its hold ends when it would have.
///
/// **Finding.** The record: section 2, L4, says a chunk someone waits for is
/// dispatched "as soon as it is not held (at most `LATEST` after it became
/// unsendable)", and section 3, arrive, step 4, that `unsendable_since` is "not set
/// anew while an entry stays unsendable ..., so a hold ends". Seen: the hold begins
/// again at the replacement. The smallest sequence: `latest` 1 s; a player joins; the
/// chunk at [`HOME`] comes without light at 0 ms (held: none of its eight neighbours,
/// all wanted, has come); at 600 ms the region sends it again with a block that shows
/// otherwise (E3); nothing else comes. Its job is dispatched at 1,600 ms, not by
/// 1,100. A chunk replaced more often than once a `LATEST` while a neighbour is missing
/// is never dispatched. If arrive's step 3 ("`p` is unsendable from now") is meant to
/// start the hold anew for the replaced chunk itself, L4's bound is what is wrong; the
/// record does not say so.
#[tokio::test(start_paused = true)]
#[ignore = "finding: a chunk replaced (E3) while it is held starts its hold anew, beyond L4's bound of LATEST"]
async fn a_held_chunk_replaced_while_it_is_held_keeps_the_end_of_its_hold() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut client = run.script.join(1).await;
    run.script.snapshot(WEST, HOME, unlit(ground()));
    run.quiet().await;
    run.pass(ms(600)).await;
    run.script
        .snapshot(WEST, HOME, unlit(lamp(ground(), 8, FLOOR, 8)));
    run.quiet().await;
    run.pass(ms(500)).await;
    let times = run.times(HOME);
    assert_eq!(times.len(), 1, "{times:?}");
    assert!(ms(1000) <= times[0] && times[0] <= ms(1100), "{times:?}");
    let job = run.out_for(HOME);
    assert!(run.script.model().same_nine(&job));
    run.answer(job.basis).await;
    run.script.sync(&mut client).await;
    assert!(run.holds_the_rules(&client, HOME));
    run.script.end().await;
}

/// "As built": held is judged when an entry is put into a set, and again when a chunk
/// comes beside it or its time ends; a chunk that begins to be wanted beside an entry
/// already in `waiting` does not hold it.
#[tokio::test(start_paused = true)]
async fn a_neighbour_that_begins_to_be_wanted_does_not_hold_a_chunk_that_waits_for_a_slot() {
    let mut run = Run::start(LightSettings::SERVER).await;
    let mut stays = run.script.join(1).await;
    run.script.held().set_in_flight(0);
    for place in [p(2, -1), p(2, 0), p(2, 1), p(3, -1), p(3, 1)] {
        run.script.snapshot(WEST, place, ground());
    }
    run.script.snapshot(WEST, p(3, 0), unlit(ground()));
    run.quiet().await;
    let _walks = run.script.join(2).await;
    run.script.walk(player(2), p(1, 0));
    run.quiet().await;
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    run.quiet().await;
    assert_eq!(run.times(p(3, 0)), [Duration::ZERO]);
    run.answer_all().await;
    run.script.sync(&mut stays).await;
    assert!(stays.has(p(3, 0)));
    run.script.end().await;
}

/// Section 4, R1: an entry in `waiting` that has no waiter left goes to `unawaited`.
/// A chunk one player has and another waits for, after a chunk came beside it, loses
/// its waiter: it is lit all the same, after the chunk someone still waits for, and
/// the player who has it is sent its light.
#[tokio::test(start_paused = true)]
async fn a_chunk_whose_last_waiter_leaves_while_another_has_it_is_still_lit() {
    let mut run = Run::start(NO_WAITS).await;
    let mut holds = run.script.join(1).await;
    let mut waits = run.script.join(2).await;
    run.script.walk(player(2), p(-1, 0));
    run.quiet().await;
    for place in view(HOME, VIEW).union(&view(p(-1, 0), VIEW)) {
        run.script.snapshot(WEST, *place, unlit(ground()));
    }
    run.answer_all().await;
    run.script.sync(&mut holds).await;
    run.script.sync(&mut waits).await;
    assert!(holds.has(p(3, 0)) && !waits.has(p(3, 0)));

    // A chunk comes beside one the first player has, and the second turns to it.
    run.script.held().set_in_flight(0);
    run.script.walk(player(1), p(1, 0));
    run.quiet().await;
    run.script
        .snapshot(WEST, p(4, 0), unlit(lamp(ground(), 0, FLOOR, 8)));
    run.quiet().await;
    run.script.walk(player(2), HOME);
    run.quiet().await;
    run.script.leave(&mut waits).await;
    run.quiet().await;
    run.script.held().set_in_flight(1);
    run.script.turn().await;
    let order = run.one_by_one().await;
    assert_eq!(order[0], p(4, 0), "{order:?}");
    assert!(order.contains(&p(3, 0)), "{order:?}");
    run.script.sync(&mut holds).await;
    assert!(run.holds_the_rules(&holds, p(3, 0)));
    assert!(run.holds_the_rules(&holds, p(4, 0)));
    run.script.end().await;
}

/// Section 4, rank, with holds: chunks that are held while their waiters move are
/// ranked by the views as they stand when they are dispatched.
#[tokio::test(start_paused = true)]
async fn chunks_held_while_their_waiters_move_go_nearest_first_as_the_views_stand_then() {
    for seed in [5, 6] {
        let waits = LightSettings {
            rest: Duration::ZERO,
            job_patience: FOREVER,
            ..LightSettings::SERVER
        };
        let mut crowd = Crowd::start(waits, seed).await;
        crowd.join(1).await;
        crowd.join(2).await;
        // A third of what is asked does not come, so that much of the rest is held.
        let comes = |place: &ChunkPos| (place.x + place.z).rem_euclid(3) != 0;
        for (number, to) in [(2, p(1, 0)), (2, p(2, 0)), (1, p(0, -1)), (1, p(-1, -1))] {
            let asked = crowd.run.script.asked(WEST).into_iter();
            let model = crowd.run.script.model();
            let missing: Vec<_> = asked
                .filter(|place| !model.has(*place) && comes(place))
                .collect();
            for place in crowd.dice.shuffled(missing) {
                crowd.serve(place, terrain(place));
            }
            crowd.run.quiet().await;
            crowd.walk(number, to).await;
        }
        crowd.run.pass(ms(1100)).await;
        assert!(crowd.run.jobs.is_empty());
        let order = crowd.open().await;
        for pair in order.windows(2) {
            assert!(
                crowd.rank(pair[0]) <= crowd.rank(pair[1]),
                "{:?} before {:?}: {order:?}",
                pair[0],
                pair[1]
            );
        }
        let once: BTreeSet<_> = order.iter().copied().collect();
        assert_eq!(once.len(), order.len());
        assert_eq!(once, crowd.run.script.model().positions());
        for (number, client) in &mut crowd.clients {
            crowd.run.script.sync(client).await;
            let due = crowd.run.script.model().view(player(*number));
            let due: BTreeSet<_> = due.intersection(&once).copied().collect();
            assert_eq!(client.chunks(), due, "player {number}");
        }
        crowd.run.script.end().await;
    }
}
