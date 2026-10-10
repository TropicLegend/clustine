# Working on Clustine

Clustine is a from-scratch Minecraft: Java Edition server (26.3, protocol 777) in Rust that
simulates one world on several worker processes. Read `README.md` and
`docs/architecture.md` first, then `docs/roadmap.md`, which has what is done, the agreed
plan for the current milestone and **where it stands** (the last sections of that file).
The decision records in `docs/adr/` say why things are the way they are.

## How the owner wants the work done

- **Plan carefully before a milestone**: a written plan, the open questions asked, the
  plan agreed, then built. An independent reviewer goes over a design before it is built;
  that has found real defects every time.
- **One verified commit per step, straight to `main`, pushed.** No pull requests.
- **Use subagents to go faster, without them getting in each other's way.** Fix the
  shared types and messages first; give each subagent crates of its own (and a worktree
  of its own) and a brief that states the interface, the tests expected and how to
  verify; they do not commit to `main`, push, download, or start Docker, kind or the
  official server. Read and test what comes back before trusting it. Parts where ordering
  mistakes hide (the edge's hand-over and resume logic) are not delegated.
- **Have tests written from the specification by someone who did not write the code.**
  Twice that found an ordering bug the author's own end-to-end tests passed.
- **Do not defer what a player would notice within minutes.** The owner judges by playing
  with real clients. A gap that shows only under rare timing or only in operation can
  wait; a seam in ordinary play cannot, whatever a later milestone will do about it.
- **After each of M3's three phases the owner tries it with real clients.** Since
  2026-10-08 the work does not wait for that: what to try is written into the roadmap
  ("Where M3 stands") and the next phase begins. The owner asked for as much as
  possible to be done without them.
- Downloads: since 2026-10-10 the owner allows downloading what the work needs on
  their machine ("You can generally download stuff you need"): a JDK, reference clones
  of other projects, crates, the Mojang server jar (the owner has agreed to the
  Minecraft EULA for the comparisons below), kind and container images. Say what was
  downloaded and from where. Subagents still download nothing. On any other machine,
  ask first.

## Checking work

Judge by exit codes. Piping `cargo` into `grep` or `tail` hides them and has let a red
commit through once.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
CLUSTINE_TEST_PINS=0,4 cargo test -p clustine --locked
```

The last line runs the end-to-end tests on a world divided into three regions. The tests
of chaos and of moves divide their worlds themselves and run only in the third line. CI
runs all four, and names failed tests on the run's summary page (readable without
signing in through the check run's annotations). `tools/check.sh` runs the four, the two
test runs at the same time, and says which failed; that takes about an hour and a
quarter on six processors, half of it the end-to-end tests of a world without pins
(`bin/clustine/tests/wanders.rs`), and CI takes an hour and three quarters.
The tests that start clusters of processes run several at a time
(`CLUSTINE_TEST_CLUSTERS`, by default one for every two processors).

- **Comparisons with the official server** (`#[ignore]` tests): need Java and the jar that
  `cargo datagen` downloads. `CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test --workspace
  --locked -- --ignored official_server`. Not in CI. Run them after anything that touches
  the protocol. (Without the name, `--ignored` also runs tests that are ignored for their
  length or as findings.)
- **Cluster test**: `deploy/kind/test.sh` needs Docker, kubectl and kind
  (`deploy/kind/get-kind.sh`). The `Cluster` workflow runs it on GitHub for every push
  that touches code or `deploy/`, so it is checked there even where Docker is missing.
- Verify a commit as it will be pushed: in a clean checkout (a detached worktree with its
  own `CARGO_TARGET_DIR`), not in a tree with other work in progress.
- **The disk Clustine runs on is network storage**: about 22 ms for a synced write on
  the owner's nodes (2026-10-10), where a local disk takes well under one. The server
  has to do well on that. Run the checks with the tests' worlds in memory
  (`TMPDIR=/dev/shm/...`), so that they judge the logic and not the disk; measure
  pauses (a merge, a split, a move, a take-over, a crowd) on the real disk, and say
  which of the two a number is from.
- A check with a real client needs the owner. `cargo run --release -p clustine` is what
  they run: one world whose regions follow the players (`--pin 4 --reshape by-hand` where
  a boundary at a known place is meant). Say exactly what to try.

## Conventions in the code

- Comments say why, in plain sentences; British spelling ("serialise"); tests are named
  as sentences in snake case; no `unwrap` outside tests except invariants stated with
  `expect`.
- `crates/clustine-sim` must stay deterministic: no hash maps, no clock, no I/O (its
  `clippy.toml` enforces part of that). A tick is a function of the region and its inputs.
- The edge owns everything about the Minecraft protocol; a worker never sees a packet
  (ADR-0005).
- Tests wait for a message or a state, never for time to pass. A fixed sleep that was
  long enough locally failed on CI.
- The bots (`tools/botswarm`) share the codec with the server, so new packets are only
  really checked by the official server (the comparisons above) or a real client.
- Commit messages: an imperative subject, then why and what in prose.

## Things that cost time before

- A subagent's worktree starts from `origin/main`, not from local commits: push the
  shared contracts first, or tell it to `git merge --ff-only main`.
- `cargo fmt` rewrites files, so scripted replacements have to match the formatted text.
- `git push` once failed repeatedly with "Internal Server Error" while everything else
  worked; `git push --no-thin` went through.
- A machine with defective memory. A store test failed with one bit wrong in 17 MB, and
  the Rust compiler had crashed twice that day. A program that fills memory with a
  pattern and reads it back found single physical pages with a bit stuck at zero; the
  kernel retires such a page when its address is written to
  `/sys/devices/system/memory/hard_offline_page`, until the next boot. A failure that
  does not come again after that is still looked into, but compiler crashes and
  single wrong bits are the machine's. GitHub's machines are the judge then.
- GitHub's job logs cannot be read without being signed in; the annotations of a check
  run can (`/repos/<owner>/<repo>/check-runs/<job id>/annotations`).
