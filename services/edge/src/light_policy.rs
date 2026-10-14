//! When a chunk whose light is not right gets its job: the hold, the rest, the
//! overdue and the patience with a thread
//! (`docs/adr/0023-light-at-the-edge.md`, section 4 and the end of section 5), each
//! on a paused clock with the jobs held by the test.

use std::time::Duration;

use clustine_data::blocks;
use clustine_world::ChunkPos;

use crate::fanout::LightSettings;
use crate::light::LightJob;
use crate::script::{
    Client, End, FLOOR, HOME, Script, Setup, VIEW, WEST, at, ground, nine_around, unlit, view,
};

async fn started_with(light: LightSettings) -> Script {
    let mut setup = Setup::new(End::Held);
    setup.config.light = light;
    Script::start_with(setup).await
}

/// A player at [`HOME`] who has every chunk of their view, lit as generated.
async fn at_home(script: &mut Script) -> Client {
    let mut client = script.join(1).await;
    for place in view(HOME, VIEW) {
        script.snapshot(WEST, place, ground());
    }
    script.quiet().await;
    script.sync(&mut client).await;
    assert!(client.has(HOME));
    client
}

/// The chunks whose jobs are out, in the order of dispatch.
fn out(script: &mut Script) -> Vec<ChunkPos> {
    let jobs = script.held().out();
    jobs.iter().map(|job| job.position).collect()
}

/// The job that is out for [`HOME`], if one is, once the edge is quiet. A lamp in
/// the middle of a chunk shines into the four beside it, which get jobs as well:
/// those are answered here, so that they hold no slot.
async fn home_job(script: &mut Script) -> Option<LightJob> {
    // What a timer of the edge has it do is done within a turn or two of the
    // clock's moving; several, to be sure of it.
    for _ in 0..4 {
        script.quiet().await;
        for job in script.held().out() {
            if job.position != HOME {
                script.held().answer(job.basis);
            }
        }
    }
    script.quiet().await;
    script.held().job_for(HOME)
}

fn lamp(script: &mut Script, chunk: ChunkPos, x: i32) {
    script.change(WEST, &[(at(chunk, x, FLOOR, 8), blocks::GLOWSTONE)]);
}

#[tokio::test(start_paused = true)]
async fn a_chunk_is_held_for_the_wanted_chunks_beside_it_and_lit_once_when_they_are_in() {
    let hold = LightSettings {
        latest: Duration::from_secs(1),
        ..LightSettings::AT_ONCE
    };
    let mut script = started_with(hold).await;
    let _client = script.join(1).await;
    script.snapshot(WEST, HOME, unlit(ground()));
    script.quiet().await;
    assert_eq!(out(&mut script), []);
    // The eight around it come: it is held no longer, and each of them still is,
    // for the chunks at the rim.
    for place in nine_around(HOME) {
        if place != HOME {
            script.snapshot(WEST, place, unlit(ground()));
        }
    }
    script.quiet().await;
    assert_eq!(out(&mut script), [HOME]);
    let job = script.held().next().await;
    assert!(job.nine.iter().all(Option::is_some));
    script.end().await;
}

#[tokio::test(start_paused = true)]
async fn a_chunk_is_held_no_longer_than_the_hold_for_a_chunk_that_does_not_come() {
    let hold = LightSettings {
        latest: Duration::from_secs(1),
        ..LightSettings::AT_ONCE
    };
    let mut script = started_with(hold).await;
    let mut client = script.join(1).await;
    script.snapshot(WEST, HOME, unlit(ground()));
    script.quiet().await;
    tokio::time::advance(Duration::from_millis(900)).await;
    script.quiet().await;
    assert_eq!(out(&mut script), []);
    tokio::time::advance(Duration::from_millis(200)).await;
    // The edge wakes by itself for it: nothing else is said to it.
    let job = script.held().next().await;
    assert_eq!(job.position, HOME);
    assert_eq!(job.nine.iter().flatten().count(), 1);
    script.held().answer(job.basis);
    script.quiet().await;
    script.sync(&mut client).await;
    assert!(client.has(HOME));
    script.end().await;
}

#[tokio::test(start_paused = true)]
async fn a_chunk_that_had_a_job_rests_before_its_next_and_the_first_change_does_not_wait() {
    let rest = LightSettings {
        rest: Duration::from_millis(250),
        ..LightSettings::AT_ONCE
    };
    let mut script = started_with(rest).await;
    let _client = at_home(&mut script).await;
    lamp(&mut script, HOME, 8);
    // After a quiet time the first change has its job at once.
    let first = home_job(&mut script).await.expect("a job at once");
    script.held().answer(first.basis);
    lamp(&mut script, HOME, 7);
    assert!(home_job(&mut script).await.is_none());
    tokio::time::advance(Duration::from_millis(200)).await;
    assert!(home_job(&mut script).await.is_none());
    tokio::time::advance(Duration::from_millis(100)).await;
    assert!(home_job(&mut script).await.is_some());
    script.end().await;
}

#[tokio::test(start_paused = true)]
async fn the_job_after_a_result_that_was_withheld_does_not_rest() {
    let rest = LightSettings {
        rest: Duration::from_millis(250),
        ..LightSettings::AT_ONCE
    };
    let mut script = started_with(rest).await;
    let _client = at_home(&mut script).await;
    lamp(&mut script, HOME, 8);
    let first = home_job(&mut script).await.expect("a job at once");
    // A block changes under the job: its result is kept from those who have the
    // chunk, and the light they are to have is not kept waiting.
    lamp(&mut script, HOME, 7);
    script.quiet().await;
    script.held().answer(first.basis);
    let second = home_job(&mut script)
        .await
        .expect("a job that does not rest");
    // Once in a rest: a change under that job too waits like any other.
    lamp(&mut script, HOME, 6);
    script.quiet().await;
    script.held().answer(second.basis);
    assert!(home_job(&mut script).await.is_none());
    tokio::time::advance(Duration::from_millis(250)).await;
    assert!(home_job(&mut script).await.is_some());
    script.end().await;
}

#[tokio::test(start_paused = true)]
async fn of_chunks_nobody_waits_for_the_nearest_goes_first_unless_another_is_overdue() {
    let far = ChunkPos::new(2, 0);
    for overdue in [false, true] {
        let mut script = started_with(LightSettings::AT_ONCE).await;
        let _client = at_home(&mut script).await;
        script.held().set_in_flight(0);
        lamp(&mut script, far, 8);
        script.quiet().await;
        if overdue {
            tokio::time::advance(Duration::from_secs(6)).await;
        }
        lamp(&mut script, HOME, 8);
        script.quiet().await;
        script.held().set_in_flight(1);
        script.turn().await;
        // The lamp far off shines into the chunks beside it too; all of them are
        // as old as it, and none is in the row the player stands in but itself.
        let jobs = out(&mut script);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].x == far.x, overdue, "overdue: {overdue}, {jobs:?}");
        assert!(overdue || jobs[0] == HOME);
        script.end().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_job_a_thread_has_had_for_too_long_is_given_up_and_its_chunk_sent_as_it_is() {
    let patience = LightSettings {
        job_patience: Duration::from_secs(5),
        ..LightSettings::AT_ONCE
    };
    let mut script = started_with(patience).await;
    let mut client = script.join(1).await;
    script.snapshot(WEST, HOME, unlit(ground()));
    script.quiet().await;
    let job = script.held().next().await;
    // The time runs from when a thread says it has the job, not from the dispatch.
    tokio::time::advance(Duration::from_secs(60)).await;
    script.quiet().await;
    script.sync(&mut client).await;
    assert!(!client.has(HOME));
    script.held().started(job.basis);
    script.quiet().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    script.quiet().await;
    script.sync(&mut client).await;
    assert!(!client.has(HOME));
    tokio::time::advance(Duration::from_secs(2)).await;
    // Given up: whoever waits has the chunk, with the light of its heights, and
    // no other job is made for it.
    for _ in 0..10 {
        script.quiet().await;
        script.sync(&mut client).await;
        if client.has(HOME) {
            break;
        }
    }
    assert!(client.has(HOME));
    assert_eq!(script.held().fresh().len(), 0);
    // What the thread says after all is late, and dropped.
    script.held().answer(job.basis);
    script.quiet().await;
    script.sync(&mut client).await;
    assert_eq!(client.times_sent(HOME), 1);
    assert!(client.light(HOME).is_some());
    script.end().await;
}

#[test]
fn seven_tenths_taken_as_changed_are_about_seven_chunks_in_ten_and_ten_tenths_all() {
    let area = || (-50..50).flat_map(|x| (-50..50).map(move |z| ChunkPos::new(x, z)));
    let taken = |tenths: u8| {
        let light = LightSettings {
            as_changed: tenths,
            ..LightSettings::AT_ONCE
        };
        area()
            .filter(|&place| light.takes_as_changed(place))
            .count()
    };
    assert_eq!(taken(0), 0);
    assert!((6_800..=7_200).contains(&taken(7)), "{}", taken(7));
    assert_eq!(taken(10), 10_000);
}

#[tokio::test(start_paused = true)]
async fn a_chunk_taken_as_changed_gets_a_job_though_it_came_with_light() {
    let all = LightSettings {
        as_changed: 10,
        ..LightSettings::AT_ONCE
    };
    let mut script = started_with(all).await;
    let mut client = script.join(1).await;
    script.snapshot(WEST, HOME, ground());
    script.quiet().await;
    let job = script.held().next().await;
    assert_eq!(job.position, HOME);
    script.held().answer(job.basis);
    script.quiet().await;
    script.sync(&mut client).await;
    assert!(client.has(HOME));
    script.end().await;
}
