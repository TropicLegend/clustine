//! How a coordinator process is told who decides when regions merge and split
//! (`docs/adr/0016-when-to-merge-and-split.md`, section 8): what its command line
//! refuses, and what it says of it when it starts.

mod common;

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

use common::processes::Cluster;

const PATIENCE: Duration = Duration::from_secs(30);

/// Runs `clustine coordinator` with `arguments`, which it is expected to refuse:
/// the status it ended with and what it complained of. A coordinator that takes them
/// would not end, so this does not wait for it for ever.
async fn refusal(arguments: &[&str]) -> (Option<i32>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_clustine"));
    command
        // Nowhere another test listens, should it start after all.
        .args(["coordinator", "--listen", "127.0.0.1:0"])
        .args(arguments)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = timeout(PATIENCE, command.output())
        .await
        .unwrap_or_else(|_| panic!("a coordinator started with {arguments:?}"))
        .expect("the server binary runs");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.trim().is_empty(), "it printed {printed}");
    let complained = String::from_utf8_lossy(&output.stderr).into_owned();
    (output.status.code(), complained)
}

/// The command line refuses distances that do not fit each other, in the words of the
/// check and whoever decides, and numbers out of their ranges, as it refuses anything
/// else that is wrong: with the status 2.
#[tokio::test]
async fn a_coordinator_refuses_distances_that_do_not_fit_and_numbers_out_of_range() {
    let near = "the split distance has to be at least 2 more than the merge distance, \
                and 6 is not 2 more than 5";
    let cases: [(&[&str], &str); 7] = [
        (&["--merge-distance", "5", "--split-distance", "6"], near),
        // Against what follows from the view distance as well, and with how a
        // coordinator is started below the reason.
        (
            &["--reshape", "by-itself", "--merge-distance", "29"],
            "and 30 is not 2 more than 29\n\nUsage: clustine coordinator",
        ),
        (
            &[
                "--reshape",
                "by-itself",
                "--merge-distance",
                "5",
                "--split-distance",
                "6",
            ],
            near,
        ),
        (
            &["--reshape", "by-itself", "--merge-distance", "0"],
            "the merge distance has to be 1 at least",
        ),
        (
            &["--reshape", "by-itself", "--view-distance", "33"],
            "--view-distance",
        ),
        (
            &["--reshape", "by-itself", "--rest-seconds", "0"],
            "--rest-seconds",
        ),
        (&["--reshape", "by-chance"], "by-hand, by-itself"),
    ];
    for (arguments, why) in cases {
        let (code, complained) = refusal(arguments).await;
        assert_eq!(code, Some(2), "{arguments:?}: {complained}");
        assert!(complained.contains(why), "{arguments:?}: {complained}");
        assert!(complained.starts_with("error: "), "{complained}");
    }
}

/// A coordinator says when it starts who decides when regions merge and split, and
/// one that decides by itself with which numbers: by hand unless it is told otherwise,
/// and by the view distance unless it is told its distances. A test's cluster passes
/// its coordinator what it is to be started with besides the usual.
#[tokio::test]
async fn a_coordinator_says_when_it_starts_how_it_reshapes() {
    let directory = tempfile::tempdir().unwrap();
    // Nothing but the coordinator is started: it waits for workers and for the store.
    // A cluster without pins tells its coordinator nothing of how it reshapes, which
    // is what the first start here is about: regions follow their players unless a
    // coordinator is told otherwise, by the distances of the usual view distance.
    let mut cluster = Cluster::new(directory.path(), 0, "").await;
    let by_itself = "reshaping by itself: regions merge and split by where their players are";
    let by_hand = "reshaping by hand: regions merge and split when somebody asks";
    // What a coordinator says when its merge distance is too short for what players
    // see (ADR-0017, section 3.5).
    let too_short = "the merge distance is less than players see across";

    cluster.start_coordinator();
    let said = numbers(&cluster, by_itself, 1).await;
    let usual = "merge_distance=22 split_distance=30 margin=3 rest_seconds=10";
    assert!(said.ends_with(usual), "{said}");
    assert!(!cluster.log("coordinator").contains(by_hand));
    assert!(!cluster.log("coordinator").contains(too_short));
    cluster.kill().await;

    cluster.coordinator_arguments = vec!["--reshape".to_owned(), "by-hand".to_owned()];
    cluster.start_coordinator();
    cluster.wait_for_log("coordinator", by_hand, 1).await;
    assert_eq!(cluster.log("coordinator").matches(by_itself).count(), 1);
    cluster.kill().await;

    // As the tests of a cluster that reshapes by itself start theirs.
    let small = "--reshape by-itself --merge-distance 3 --split-distance 5 --rest-seconds 5";
    cluster.coordinator_arguments = small.split(' ').map(str::to_owned).collect();
    cluster.start_coordinator();
    let said = numbers(&cluster, by_itself, 2).await;
    let told = "merge_distance=3 split_distance=5 margin=2 rest_seconds=5";
    assert!(said.ends_with(told), "{said}");
    // Three chunks are fewer than players see across at the usual view distance of
    // eight, which needs nineteen, and the coordinator says so, once.
    cluster.wait_for_log("coordinator", too_short, 1).await;
    let log = cluster.log("coordinator");
    let warned = log.lines().find(|line| line.contains(too_short)).unwrap();
    assert!(
        warned.ends_with("merge_distance=3 view_distance=8 needs=19"),
        "{warned}"
    );
    assert!(warned.contains("WARN"), "{warned}");
    // It was started once for each, and each time as it was told.
    assert_eq!(log.matches(by_hand).count(), 1);
    assert_eq!(log.matches(too_short).count(), 1);
    cluster.kill().await;
}

/// The line of the coordinator's log that says it reshapes by itself and with which
/// numbers, of the start that was the `times`th to say so.
async fn numbers(cluster: &Cluster, by_itself: &str, times: usize) -> String {
    cluster.wait_for_log("coordinator", by_itself, times).await;
    let log = cluster.log("coordinator");
    let mut lines = log.lines().filter(|line| line.contains(by_itself));
    let line = lines.nth(times - 1).expect("it was logged");
    line.trim_end().to_owned()
}
