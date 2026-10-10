# Review 1 of "Few syncs in turn" (draft of 2026-10-10)

Reviewed read-only against a worktree of `main` at `12e445f` plus the draft, the
measurement and `syncs.rs`. Nothing was built or run. Paths below are relative to the
repository. "Draft" is `docs/groundwork/few-syncs-in-turn-draft.md`,
"measured" is `docs/groundwork/disk-syncs-measured.md`.

Each finding is marked **defect** (the draft is wrong, or loses something the code or a
record has today), **gap** (something it has to say and does not) or **doubt**.

## The short version

- The runner's three changes (8.1, 8.2, 8.3) hold up, give most of the time, and touch
  no file on disk. Build them first, as the draft says. 8.2 needs another condition
  than a number of ticks (finding 6).
- Of the store's changes, `CLOSE_AT` and section 4 are cheap and sound, but the build
  order credits step 2 with waits it only moves (finding 4), and both break the
  store's scenario tests wholesale (finding 5).
- Section 1 (the state as a record) is the only change of format, touches recovery
  everywhere, and saves about 14 ms per checkpoint. It also gives up a tested
  property: a damaged state is refused by name today and would be passed over in
  silence (finding 2). I would not build it as drafted.
- Sections 5 and 6 redesign the chunk thread without a word about ADR-0021, accepted
  the same day, which redesigns the same loop and puts up to 512 generated chunks
  into every sync (finding 1). Their counts and times do not survive that.
- The comparison with ADR-0017 section 9.7 is of the wrong quantity (finding 3), and
  the largest thing a player feels at a move, 0.6 s, is left out (finding 7).

## Findings, most serious first

### 1. Gap: ADR-0021 rewrites the chunk thread and fills every sync with notes; the draft never mentions it

ADR-0021 is **Accepted** (`docs/adr/0021-generation-is-a-function.md:3`) and not built:
`ChunkService::run` still does one job at a time (`services/worldstore/src/chunks.rs:366`)
and a load of a chunk that is not stored generates and notes nothing (`:379-383`). The
draft cites ADR-0008, 0011, 0014, 0017, 0018 and 0020, and not 0021 or 0022.

What ADR-0021 decides that sections 5 and 6 collide with:

- **S2** (`0021:589-597`): a generated chunk that a region was handed is noted like a
  save and "written and made durable **by the next sync the thread makes**: a
  checkpoint, a return, a flush or a restore of any handle", with a limit of 512 notes
  of their own. So after ADR-0021 the rounds of a standing region's small checkpoint
  carry every pending note of every region, up to 511 files. Regions merge, split and
  move exactly when players walk into new land or join (a first view is 329 notes by
  that record). The measurement has a round of 64 files at 84 ms and ADR-0018 has
  200 chunks at 0.7 s. That is larger than everything the draft's "Then" columns count,
  and it lands in the standstill.
- **Section 7** (`0021:702-800`): the thread becomes a scheduler in which a save,
  checkpoint, flush or return of a handle waits for every earlier job of that handle,
  loads that are being generated among them, and "while the thread syncs, it does
  nothing else". Draft section 6 ("takes whatever else is in its queue already, in
  order ... then it syncs once, and then each of those jobs says what it has to say")
  is written against a plain queue. A checkpoint set "beside the first" whose handle
  has a load still generating may not speak.
- **S5** (`0021:604-613`): on a failed sync the saves are dropped and the notes are
  kept pending. Draft section 5 says "if the first round fails, all temporaries are
  removed" and section 6 "every handle that saved is lost, as now: what is pending is
  one set whoever saved it". Neither is true of notes.
- **S7** (`0021:621-630`): `Store::close` is a barrier whose job syncs. Section 6 does
  not say what its go does with a `Barrier` or a `Fold` in the queue.

**Change.** Decide the order of the two records before either touches `chunks.rs`. I
would build ADR-0021's thread first and write sections 5 and 6 against it, and add to
this record what a standing checkpoint does about notes: either a sync asked by a
region that stands still writes that handle's saves only and leaves notes to the next
sync nobody stands for, or the notes' cost is counted in "What it gives". Until then
sections 5 and 6 should not be built.

### 2. Defect: with the state in the log, a damaged checkpoint is passed over in silence instead of refused

Today a state file is sealed with a checksum (`crates/clustine-format/src/region.rs:130-142`,
`:386-399`), and a damaged one fails the start or the hello with `StoreError::Damaged`
naming the file (`services/worldstore/src/lanes.rs:262-268`, `:1414-1421`).
`bin/clustine/tests/single.rs:430-447` tests exactly that: it overwrites
`regions/0.state` and asserts the error names region 0 or the file.

A record of the log whose checksum fails is not an error. `read_log_with_offsets`
breaks at it (`crates/clustine-format/src/log.rs:596-598`) and `Lanes::load` discards
the count of valid bytes for **every** segment, not only the last
(`lanes.rs:328-335`: "what follows ... was being written when the process died").
So one wrong bit in a `State` record hides that record and everything behind it in its
segment: other regions' commits, their states, table records, and under section 3 the
only word of an epoch. The region is then restored from whatever older state is still
there, or from none, and nobody is told. `CLOSE_AT` makes the segment 4 MiB where it is
one checkpoint's worth today. CLAUDE.md records this machine flipping single bits in
17 MB of the store's data.

The weakness is there today for commits and table records. Section 1 moves the one
thing that is refused by name into it, and section 2 widens it.

**Change.** Before section 1 or `CLOSE_AT`: `load` treats bytes behind the valid
records as a torn tail only in the highest-numbered segment; in any earlier segment
they are `StoreError::Damaged` with the path and the offset. With section 1, a region
whose latest `State`, `Absorbed` or `Split` cannot be read is refused by name. A kill
test and a damage test for each. The draft's test list has none for damage.

### 3. Defect in the numbers: the bound of ADR-0017 section 9.7 is not what the draft compares with

The draft's Context sets "950 to 1,650 ms by the command" for four bots, unoptimised,
against "half a second in the middle and a second at worst". ADR-0017 says the bound
is "**optimised, for the crowd of a hundred**" (`docs/adr/0017-the-end-of-the-stripes.md:3011-3012`)
and that the number meant is the one the worker logs as "a region stood still"
(`:3014-3020`), not the command's duration.

By that measure the trace is already inside the bound for the survivor (160 to 205 ms)
and outside it only for the absorbed region's players (824 ms). For a crowd the draft
shows nothing: it says itself that the waits saved "are the same number for a crowd,
and a smaller share of its pause", and ADR-0018's 0.7 s for 200 chunks is half
describing, which no change here touches. Change 8.2 can make a crowd's standing
checkpoint larger (finding 6).

Recounted from the code, the survivor's standstill for one changed chunk: now
(20+14) + (20+14+20+14+20+14) + (20+14+14) = 184 ms, which the trace confirms; then
20 + (40+14+14+20) + 20 = 128 ms. That is 56 ms less, and it is made of: two new
segments' names 28 (`CLOSE_AT`), the removal's sync 14 (section 4), the state 14
(section 1), section 5 nothing. "11 waits become 6" reads as nearly half; it is 30 %.
The 538 ms the absorbed region's players gain are mostly 8.2 (about 290), 8.1 and the
two threads' queues.

**Change.** State the goal as the standstill, per kind of player, and say for which
crowd. Run `crowds.rs` optimised before and after step 1, since that is what 9.7
names. Give "Then" in milliseconds per change, not in waits, so that section 1's
14 ms is seen beside its cost.

### 4. Defect: step 2 moves waits to the next request instead of saving them

"Behind the answer" is still on the commit thread, and in three places the very next
request is the one somebody stands still for.

- **A split's region file.** After `Split` is answered the worker says hello for the
  part at once (`bin/clustine/src/cluster/worker.rs:1211-1222`), and the part's players
  wait for that answer. If `write_region_files` runs right behind the answer
  (`lanes.rs:1717`), the hello waits behind its two syncs. If it does not run, the
  hello writes the file itself: `admit` takes `unwritten` as a reason
  (`lanes.rs:1387-1400`), and the rule that it "leaves `unwritten` as it is" is in
  section 3, which is step 6. Either way the part's players pay the two waits until
  step 6. `syncs.rs` counts "0, and 1 after" for that hello today only because the
  file was written before the `Split` answer.
- **The table file behind a flush.** A standing region's flush is answered, and the
  runner hands in the record at once (`services/worker/src/lib.rs:1387-1397`). The
  record's `end_group` and `write_alone` then wait behind the table's temporary and
  `regions/` (and the players' file). Until `CLOSE_AT` (step 3) a segment is closed at
  every checkpoint, so this is the usual case: measured, "13 with the table".
- **The same behind a release's second checkpoint**: the hello of the next owner waits
  behind it.

So step 2's "3 waits of a merge, 2 of a split, 2 of a checkpoint that lets a segment
go" is one wait of a merge (the removal's `regions/`) until steps 3 and 6 are in.

**Change.** Put `CLOSE_AT` before or with "behind the answers". Say that what follows
a close is done when the thread finds its queue empty, or at the next close, not
"by the same call of `end_group`". Move "a hello that does not raise the epoch leaves
`unwritten` as it is" from section 3 into the step that builds section 4.

### 5. Gap: the store's scenario tests are written against a handle's flush and a close at every checkpoint

The draft's Risks: "Tests that look at files behind a handle's flush ... How many do
was not counted." Counted roughly:

- `services/worldstore/src/regions.rs:1229-1275` (scenario 11) asserts after each
  `handle.flush()` the exact list of segments (`[1]`, `[1, 2]`, `[2]`) and the table
  file's `from` (3). Both depend on the close at every checkpoint and on the table
  file being written before the flush's answer.
- Handle flushes: 64 in `regions.rs`, 126 in `scenarios.rs`, 18 in `unpinned.rs`, 42 in
  `tests.rs`. `store.flush()` is used 0 times in the first three.
- ADR-0020's rule P says "After a clean stop no segment may be left at all"
  (`docs/adr/0020-one-stay-per-player.md:760-762`) and names a test that asserts it,
  `services/worldstore/src/stays.rs:962-1000` (`assert_eq!(segments(..), Vec::<u64>::new())`).
  With `CLOSE_AT` the segment is not closed, and with section 1 the regions' states
  are in it: the log is never empty again.
- `services/worker/src/lib.rs:4486-4526` (`checkpoints_save_loaded_chunks_and_empty_the_log`)
  asserts `log_length() == 0`. `bin/clustine/tests/single.rs:430` and `:490` read
  `regions/0.state` after a stop.

These suites are the specification of recovery, several written by someone who did not
write the code. If the author of each step rewrites their expectations, that
independence is gone at the moment the order of recovery changes.

The draft's "Changes to other records" omits ADR-0020. Rule P itself still holds
(segments are numbered from the last one), but its text and its named test do not.

**Change.** Make `CLOSE_AT` a setting of the store. The existing suites run with 0 and
stay as they are for steps 2 to 4; the kill tests run with both values. For section 1,
list the tests that change, by name, in the record, and have their new expectations
written by the independent test author. Add ADR-0020 section 8 to the records changed.

### 6. Doubt: 8.2 bounds the standing checkpoint by ticks, and for a crowd that is the wrong measure

8.2 is correct. The first checkpoint is an optimisation only: what `commit` relies on
is the flush behind the checkpoint made at the stop (`services/worker/src/lib.rs:1387-1397`,
`:1411-1416`), which 8.2 keeps. Nothing is lost by stopping at once.

What changes is the size of the checkpoint the region stands still for. Today it holds
what changed while the first was written. With 8.2 it holds what changed since the
`Prepare`, up to `PREPARED_FOR` = 40 ticks. `Prepare` is sent with the order to release
the absorbed region (`services/coordinator/src/state.rs:1871-1878`), so the survivor
accumulates for as long as the absorbed region's release takes, which for a crowd is
its own large checkpoints. Both regions' players wait for the survivor's standing
checkpoint. For four bots this is five ticks and fine; for the crowd of 9.7 it may be
longer than what 8.2 removes.

Only block changes mark a chunk unsaved today (`lib.rs:1955-1958`), which keeps this
small. ADR-0022 ("what a chunk carries") may change that.

**Change.** Stop at once if the `Prepare`'s flush is answered **and** `unsaved` has no
more than a few chunks (the runner has the count, `lib.rs:1237`); otherwise checkpoint
first while ticking, as now. Measure with `crowds.rs`.

Also: the coordinator's own description says `Prepare` is sent "for the regions that a
split is wanted of" (`state.rs:819-820`), but the only place that issues one is the
merge (`state.rs:1878`). The draft's "a split has no order before it" matches the code
and not that comment. A split is wanted for more than a second before it begins
(`state.rs:809-810`), so a `Prepare` then would let 8.2 serve splits too. I found one
issuing site by search and did not read all of `state.rs`.

### 7. Gap: the 0.6 s after a move is left out, and it is larger than all a move gains here

From the code, not measured:

- A region that is opened by a new owner has no chunk in memory:
  `RegionRunner::restore` (`services/worker/src/lib.rs:835-847`). A merge and a split
  keep theirs warm (`begin_anew`, `:1679-1699`); a move cannot.
- A link's hello puts every chunk it names into `link.hold` (`:2768-2776`), and while
  that is not empty **every** message of the link is held, moves included
  (`drain`, `:2364-2367`). So no input reaches a tick, and no tick has anything to
  commit (`:1955`), until every one of those chunks has been loaded.
- Each is a `Job::Load` on the one chunk thread, done one at a time: read the
  manifest, read and unpack each section (`services/worldstore/src/chunks.rs:200-218`,
  `:374-393`), and its answer is taken into a tick's inputs.

So the thirteen ticks without a commit are the view of every player of the region
being read off the disk serially, in an unoptimised build. The bots waited 1.1 to
1.4 s at a move; this record takes a move's standstill from 220 to about 140 ms.
By CLAUDE.md ("do not defer what a player would notice within minutes") this is the
seam, not the syncs.

It also bears on section 6: those loads sit in the chunk thread's queue in front of
any other region's checkpoint, which waits for all of them. The trace's 48 ms and
139 ms "behind another checkpoint" were with four bots; behind a move's or a join's
loads the wait is not bounded by any sync.

**Change.** Look into it before the store's steps: measure it optimised; then either
hold only actions on blocks (as `waits_for_its_chunk` already does, `:2420-2445`) and
let moves through, or have the store read a hello's chunks ahead, or let loads not
keep a checkpoint waiting (ADR-0021 section 7 does that for generated chunks only).

### 8. Gap: when `collect` runs under `CLOSE_AT` is not said, and test 7 contradicts the text

Section 2: "What follows a close (`collect`, the files of regions ..., the table file,
the players' file) is done after the group's answers." Read literally, `collect` runs
only when a segment is closed. Then a segment that became unneeded by a later
checkpoint in a segment below `CLOSE_AT` stays until that one fills. The draft's test 7
("after every region has checkpointed twice, no segment from before the first is
left") fails under that reading.

`trim_for_the_table` and `trim_for_the_players` do need a closed segment
(`lanes.rs:1140`, `:1169`: they name `log.next` as the file's `from`). `collect` does
not: it never removes the active segment (`lanes.rs:2190-2193`) and syncs nothing.

**Change.** `collect` after every group that made a state durable; the two files only
at a close. Say what a quiet world does: its one segment may take hours to fill, so
the table and players' files are not written and every start replays it. That is
safe, and should be written down with a bound on what a start reads.

### 9. Doubt: section 1 costs the most and gives the least

- It saves the state's temporary (20 ms) and `regions/` (14 ms) for one sync of the
  log (20 ms): 14 ms per checkpoint on a quiet disk (finding 3).
- It is the only change of format. An older store refuses such a world with
  `FormatError::Corrupt("record kind")` (`crates/clustine-format/src/log.rs:388`),
  which is reported as damage: refused, but with the wrong sentence.
- It keeps the state file path alive for states above 64 MiB, so two ways to
  checkpoint are to be killed at every point for good.
- Every region's latest state pins its segment. Today only a region that died with
  uncovered commits does. The draft's remedy ("not built until it is seen") is seen
  when the disk fills.
- Findings 2 and 5.

Mechanically I found it sound: see "Found sound". But the same 14 ms and more is had
without it for a merge and a split, where the state written at the stop is replaced
one sync later by the record that carries a state anyway. That is section 7, which
the draft puts last.

**Change.** Do not build section 1 as a step of this record. Measure after steps 1 to
3; if a wait must still go, argue section 7 properly (it removes the state's wait for
merges and splits with no new record kind), and leave moves with the state file.

### 10. Gap: section 6 leaves cases open

Apart from finding 1:

- **The first job is slowed by those behind it.** A standing region's one-chunk
  checkpoint that is first in the queue is today done in rounds of one. Under section 6
  it waits for the describing of up to 128 saves queued behind it
  (`ChunkManifest::describe` in `FileChunks::save`, `chunks.rs:220-233`) and shares
  rounds of that many files. "No wait of an operation's own" is not right for it.
- **A failed sync loses more handles**: those whose saves came behind the first job
  and would have had a sync of their own. Safe, and should be said.
- **What a job of a handle that is not lost does after a failed go.** Today a
  checkpoint whose own sync fails loses its handle even if it saved nothing
  (`chunks.rs:425-428`). In a go, a handle with no save pending may speak (all its
  saves were durable before) and a `Restore` must fail. The draft says neither.
- **`Barrier` and `Fold`** must end a go; not said.

### 11. Doubt: section 3 is sound as drafted but its failure is split brain, and two details are missing

The coordinator takes epochs from the store's list (`services/coordinator/src/state.rs:1610-1636`),
and the store takes an equal epoch for the same owner come back (`lanes.rs:1345-1356`).
So if a start ever misses an epoch that only an `Opened` record had, two workers can
hold one region. The protection is a flag in memory plus a rule in `collect`.

- `load` handles `Opened` only for a lane that exists (`lanes.rs:368`:
  `regions.get_mut`). Taking the epoch needs the lane made, as for a `Split`'s part
  (`:444`); `align` then drops lanes the table does not have (`:1904-1921`).
- `make_over` calls `collect` before `align` (`lanes.rs:1894-1895`, `:561`), at which
  point the table is still the old one. The region files have to be written there too.
- Test 10 should include a make-over and a failed `write_region_files` followed by a
  kill.

### 12. Doubt: change 7 should stay unbuilt as drafted

The store cannot know that the runner saved every chunk it changed; it can only count
saves it was sent. The draft says itself it is not argued for a split whose part gets
chunks with a return under way, nor for stay notes. See finding 9 for when it would be
worth arguing.

## Found sound

- **Section 1, recovery order.** A `State` read "by its tick and wherever it is" works
  as `Absorbed` does: `Lane::whole` (`lanes.rs:154-159`) drops commits up to its tick,
  and later commits have higher ticks. A stale session's checkpoint is dropped by the
  `current` check (`lanes.rs:1252`). A failed group cuts the record with the group
  (`fail_log`, `:1047-1079`). `make_over` lets records go through `Opened { restored: 0 }`
  (`:1865-1876`, `:370`), and segments are removed from the start only, so the
  `Opened` outlives the record. `absorbable` (`services/worker/src/lib.rs:3102-3114`)
  and a take-over read the state through `lane.record` (`lanes.rs:1427-1432`).
- **Section 4** as such: the store copes with both today (`lanes.rs:1566`, `:1718`,
  `align` at `:1903-1931`).
- **Section 5's order.** The invariant needs the sections' directories synced before
  any manifest is renamed, and that is kept. A durable temporary of a manifest is
  never read (`.tmp`, `disk.rs:71-75`). Its own claim that it saves nothing for one
  chunk is right.
- **Section 3's order within one run**: record synced alone, then memory changed.
  Two hellos for one region are serial on the commit thread.
- **8.1.** Answers are already taken between ticks (`lib.rs:2298`), and a tick is not
  replayed from its inputs, so nothing rests on the tick boundary. Stopping between
  ticks is the state the next step would stop in, since `step` looks before it drains
  a link (`:1018-1033`).
- **8.3.** The store holds a checkpoint behind unsynced commits (`lanes.rs:948-952`)
  and drops it with the owner if they fail (`lose`, `:1977-1983`). The flush is behind
  the last tick's claims and returns in the lane's order, so "every claim answered"
  still follows from the flush.
- **Counts**, recounted from the code: the record of a merge is 3 (`write_alone`
  segment and `log/`, then `regions/`, `lanes.rs:1534`, `:1559-1565`); a hello that
  changes the owner is 2 before and up to 2 after (`:1388-1396`, `:1450`, `:571`); a
  checkpoint of changed chunks is 6. "Then" follows if steps 3, 4 and 6 are all in.

## Not checked

- Nothing was run: no test, no count, no time. The times are the draft's arithmetic
  and mine.
- `kill.rs`, `kill_regions.rs`, `kill_unpinned.rs`, `rounds.rs`, `stays_kill.rs`: whether
  they "walk every point of the new order by how they are written".
- `tcp.rs`, `table.rs`, `players.rs` beyond what `lanes.rs` calls.
- `syncs.rs` beyond its test names.
- How many chunks a hello names in the trace (finding 7), and whether the bots
  themselves wait for chunks before acting.
- The coordinator's leases and what it waits for at a merge, beyond where `Prepare` is
  issued.
- Whether ADR-0022 changes what marks a chunk unsaved.

## What I would build, and in what order

1. **8.1, 8.3, then 8.2 with a condition on `unsaved`** (finding 6). No file changes,
   most of the gain, each testable with a runner stepped by hand.
2. **Look into the 0.6 s of a move** (finding 7).
3. **Refuse damage in a segment that is not the last** (finding 2), then **`CLOSE_AT`
   as a setting** together with section 4 and "behind the answers" (findings 4, 5, 8).
4. **Section 3**, with finding 11's details.
5. Measure, optimised and with the crowd.

Not as drafted: **section 1** (finding 9), **sections 5 and 6** until they are written
against ADR-0021 (findings 1, 10), **section 7** (finding 12).
