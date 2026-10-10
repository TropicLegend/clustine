# Draft of a decision record: Few syncs in turn

- Status: **Draft**, to be numbered. Not reviewed, nothing of it built. Written
  2026-10-10 against `main` at `12e445f`, from the measurement in
  [`disk-syncs-measured.md`](disk-syncs-measured.md), whose section numbers are meant
  wherever this says "measured". It changes the world store's two threads, one kind of
  record in the log, and three places in the runner; it changes no message between
  processes but in its last step (section 7), and no promise of
  [ADR-0008](../adr/0008-durable-regions-and-resuming.md),
  [ADR-0011](../adr/0011-the-world-store-and-regions.md) or
  [ADR-0018](../adr/0018-a-checkpoints-chunks-written-together.md). Where it changes
  how a promise is kept, the section says so. Every guess is marked **Guess**.
- Date: 2026-10-10

## Context

Clustine will run on nodes whose disk is network storage, where a sync takes 20 ms and
the sync of a directory 14 ms (measured, section 2). The world store answers nothing
before it is on disk, so what a player waits for when a region is moved, merged or
split is the number of syncs the store makes one after another on the way.

Measured on such a disk, four bots, unoptimised:

| | Waits in turn, command to answer | While players stand still | By the command | With the world in memory |
|---|---|---|---|---|
| A merge | 39 | 11 for the survivor's players, 33 for the absorbed region's | 950 to 1,650 ms | 187 ms |
| A split | 14 to 20 | 12 to 14 | 320 to 1,190 ms | |
| A move | 12 to 18 | 10 to 12 | 300 to 1,000 ms | 63 ms |
| A take-over | 6, after the lease | 6 | not traced | |

[ADR-0017](../adr/0017-the-end-of-the-stripes.md), section 9.7, allows half a second in
the middle and a second at worst.

ADR-0018 had the store write a checkpoint's chunks in rounds whose syncs wait together,
and left "what stays in turn" for a measurement to judge. This is that measurement's
answer. Two things it found decide what follows.

**Two syncs that wait together cost as much as two in turn.** The disk makes one trip
of 20 ms at a time, and what is asked during a trip is served by the next. A round of
two or three files costs 40 ms; of sixty-four, 84 ms. Directories, up to four, share one
trip of 14 ms. An append to a file that is synced anyway costs nothing more: sixty-four
records in one file are one trip of 23 ms. And the commit thread, which syncs the log
for every tick of every region, was in a sync for half of the traced time: beside it
every sync of the chunk thread takes two trips.

So making two syncs wait together saves nothing here. **A wait is saved by not making
it: by writing what it was for into the log, which is synced anyway; by doing it behind
the answer when nothing answered rests on it; or by not asking for it.** ADR-0018
remarked that "the state file's temporary could join the manifests' round": measured,
that saves a trip only where the round has two files already and none where it has one.

**A fifth of a merge is not syncs at all** (measured, section 7): runners that look at
an answer or an order once a tick, requests that wait behind another region on one of
the store's two threads, and two checkpoints of the survivor that nothing needs.

## Decision

**The store writes into the log what it now writes into files of their own, does behind
the answer what no answer rests on, and the runner asks for one checkpoint where it
asks for several.** Seven changes to the store and three to the runner, each of which
stands by itself. The first six keep every rule of ADR-0011 as it is written; the
seventh changes one condition of it and is built last, and only if it is still needed.

For each: what is done now, what is done then, what is durable when, why a crash at any
point is safe, and how many waits in turn it saves.

### 1. The state of a checkpoint is a record of the log

**Now.** `Job::Checkpoint` (`services/worldstore/src/chunks.rs:421`): once the saves
before it are durable, the chunk thread writes the state to `regions/<r>.state.<k>.tmp`
and syncs it (`:436` to `:439`). The commit thread renames it (`Lanes::install`,
`lanes.rs:1256`) and, when the group ends, syncs `regions/` (`:971`). Two waits in
turn, on two threads, behind the rounds of the chunks. The checkpoint counts from the
sync of `regions/`: only then are the commits up to its tick let go (`:983` to `:985`).

**Then.** A new record, `State { region, epoch, tick, state }`, which is to a region
what `Absorbed` is to the survivor of a merge: its latest whole state, in the log. The
store has kept such a record for a lane since ADR-0011 (`Lane::record`, `lanes.rs:113`;
`Lane::whole`, `:154`; `Log::whole_state`, `:2153`), and restores a region from it until
a later state takes its place (ADR-0011, section 3.5).

- `Job::Checkpoint` makes the saves durable as now, and then hands the state itself to
  the commit thread: `Message::Checkpointed { session, tick, state }`. It writes no file.
- The commit thread looks at it as `install` does now (the session is the one that
  opened the region last; the tick is above the latest whole state and above one that
  is pending), and appends the record to the log, in the group it is in. When the
  group's one sync has ended well, the record is the lane's latest whole state, the
  live commits up to its tick are let go, and what waited for the group is answered.
  Nothing is changed in memory before that, as for a state file now (`installing`).
- `Lanes::load` reads a `State` as it reads an `Absorbed` for the survivor
  (`lanes.rs:411` to `:428`), by its tick and wherever it is in the log; it says nothing
  of the table. `Lanes::admit` chooses between the state file and the record as now
  (`:1427`). `Opened { restored }` lets go of a state in a record above `restored`, as
  now (`:370`).
- A state file is still read, and a world written by today's store opens as it is.
  **No state file is written any more**, except for a state that is longer than a
  record may be (64 MiB, `clustine-format/src/log.rs:95`): that one goes the way of
  today. A state file below a record's tick is passed over, as now, and removed with
  the next sync of `regions/` that something else asks for.

**Durable when.** As now, a checkpoint counts when its last sync has ended well; that
sync is the group's sync of the log in place of the sync of `regions/`. The saves are
durable before the record is appended, as they are before the temporary is written now;
the commits up to the tick were durable before the store passed the checkpoint to the
chunk thread at all (`Owner::held`, `lanes.rs:948`).

**A crash at any point.** Before the append: nothing has changed. During or after it,
before the sync has ended: the record is not there, or is cut off, which does not count
(`read_log_with_offsets`), or is whole. If it is whole it is a true checkpoint, for the
reason just given, and nobody was told; the commits it covers are still in the log
either way, because a segment is removed only by a group that ended well. That is the
row "Merge" of ADR-0011, section 4.3, for a record that changes nothing in the table. A
sync that fails with the store alive is `Lanes::fail_log` as for any group: the record
is cut off with the group and the cut made durable before anyone is served
(section 4.1 there).

**What it costs.** The log grows by a state per checkpoint where a file was replaced. A
region's record keeps its segment until the region's next checkpoint, which is an
interval away at most while the region has an owner; a merge's and a split's records do
the same today. A region that has no owner for long keeps its segment for as long (see
"Risks").

**Saved.** Two waits of every checkpoint, less the group's sync of the log where the
group has no commit to sync anyway: a checkpoint with no changed chunk goes from two
waits to one, one with chunks from six to five.

### 2. A segment is closed when it is full, and what follows a close comes behind the answers

**Now.** Every group that put a state file in place closes the segment (`lanes.rs:999`),
"so that this one can be removed once a later checkpoint covers what is left in it". So
the next append begins a segment, and its sync is followed by a sync of `log/`
(`Log::sync`, `:2069` to `:2073`). In a merge that second wait is on the way three
times: for each region's last commit before it stands still, and for the record
(measured: 53 segments in 45 s). Behind the close, and **before the group's answers**
(`:1010`), come `collect`, the table file if the first segment is kept for the table
alone (`trim_for_the_table`, `:1162`: its temporary and `regions/`), and the players'
file in the same way (`:1129`). In a world whose regions claim and return chunks the
first is the rule: measured in a move's second checkpoint, and in the survivor's before
a merge.

**Then.**

- A group that made a whole state durable closes the segment only if the segment holds
  at least `CLOSE_AT` bytes. **Guess**: 4 MiB, a hundredth of an interval's commits of a
  busy world and sixteen segments to the limit of one.
- What follows a close (`collect`, the files of regions that are still to be written,
  the table file, the players' file) is done **after** the group's answers have been
  sent, by the same call of `end_group`.

**Durable when.** No answer changes what it stands for. The table file and the players'
file hold nothing that the log does not: they are written so that a segment can go
(ADR-0011, section 3.5; ADR-0020, section 8), and the segment goes only when its file
is durable, as now. What a flush promises ("everything asked before is done") is kept
for the files of chunks and the state; that the table file is written before a
handle's flush is answered is said by a comment (`lanes.rs:996`) and by no record.
`Store::flush`, the barrier, still returns behind all of it, because its answer is sent
when `end_group` has returned (`:641` to `:647`).

**A crash at any point.** Between the answers and the table file: what is on disk is
what a kill before the table's temporary leaves today, a point the kill tests stop at;
that somebody was answered a flush changes nothing of it. With a segment that lives
longer, a start reads up to `CLOSE_AT` more of the log, and the table file is written
once per segment and not once per checkpoint.

**Saved.** One wait of every first commit behind a checkpoint, which is one for each
region that stands still and one for the record of a merge or a split; one behind the
answer of every hello; and two (four with the players' file) of every checkpoint that
found a segment to let go.

### 3. A hello raises the epoch by its record

**Now.** `Lanes::admit` writes the region file when the epoch or the block of entity
ids is not what the file has: its temporary is synced, renamed, and `regions/` synced
(`lanes.rs:1388` to `:1400`), "so that an owner with a lower epoch is refused after a
restart too". Then `Opened { region, epoch, restored }` is appended (`:1450`), the hello
is answered (`:1296`), and the record is synced when the group ends, behind the answer
and in a segment of its own. Two waits before the answer and two after it.

**Then.** The record has the epoch already. For a hello that raises the epoch and
issues no block of entity ids:

- the record is appended and the log synced by itself, as the record of a merge is
  (`write_alone`, `:1768`), **before** the answer. Only then the lane has the epoch, and
  its file is marked as not on disk (`Lane::unwritten`, `:107`, which a split's part has
  today for the same reason: "the record of the split has its epoch meanwhile");
- no region file is written. `write_region_files` (`:1790`) writes the files that are
  still to be written before a segment that holds such a record is removed: `collect`
  calls it first and removes nothing if it fails, as `trim_for_the_table` does today
  (`:1175`). They are written together: one round of temporaries, one sync of
  `regions/`;
- `Lanes::load` takes the epoch of an `Opened` that is above the lane's file's, as it
  takes a `Split`'s `part_epoch` (`:446` to `:456`), and `Lanes::align` writes the file
  at the start, as it does for a part (`:1930`);
- a hello that neither raises the epoch nor issues a block (the worker's hello for the
  part of a split; the same owner coming back) writes nothing but its record, which is
  made durable with its group, behind the answer, as now. It leaves `unwritten` as it
  is, where `admit` now takes it for a reason to write the file (`:1388`);
- a hello that issues a block of entity ids (the first of a pinned or home region, or
  the home region's when its block is used up) writes the region file as now. It is
  once in a region's life.

**Durable when.** ADR-0008, section 3: the store "keeps the highest epoch of the region
on disk", and a hello with a lower one is refused "also after the store was started
again". The highest epoch on disk is then the greater of the region file's and the
records' of the segments there are, and it is durable **before the hello is answered**,
as now. The record that the region was opened, which now becomes durable after the
answer, becomes so before it.

**A crash at any point.** Before the record is whole on disk: the epoch is the old one
and nobody was answered. Whole on disk and not answered: the epoch is raised and the
commits above `restored` are let go; the same owner says hello again with the same
epoch and is taken, a lower one is refused, which is right, because the coordinator has
issued the higher. That is what a kill after the sync of `regions/` leaves today. A
sync that fails with the store alive is `fail_log`; memory has not been changed.

**Saved.** One wait before the answer of every hello that changes the owner (20 ms in
place of 34), and what came after the answer: the commit thread is free 34 ms sooner
for whoever asks next, which in a merge is the survivor.

### 4. A split's region file and a merge's removals come behind the answer

**Now.** `Lanes::split` writes the new region's file and syncs `regions/` before it
answers (`lanes.rs:1717`, `:1721`): two waits. `Lanes::absorb` removes the absorbed
region's files and syncs `regions/` before it answers (`:1559` to `:1565`, `:1575`): one.

**Then.** Both are answered when the record is durable. The new region's file is one
that is still to be written (section 3). The absorbed region's files are removed behind
the answer, and the removal is made durable by the next sync of `regions/`.

**Durable when, and a crash.** Neither answer rests on these files: the record has the
new region's epoch, and the table has the absorbed region as absorbed. The store copes
with their never being done today: a failure of either is logged and passed over
(`:1718`, `:1566`), `Lanes::align` does both at the next start, and ADR-0011,
section 4.3, has "split; step 4 of 4.2 writes `N.region`" and "merged; step 4 of 4.2
removes what is left" as what a kill behind the record leaves. That state is the one
this makes the usual one for a moment.

**Saved.** Two waits of a split and one of a merge, while the region stands still.

### 5. The temporaries of a sync are one round

**Now.** `FileChunks::sync` (`chunks.rs:235`): the section files' temporaries are
written and synced together; renamed; their directories synced together; then the same
for the manifests. Four waits.

**Then.** The temporaries of the sections **and of the manifests** are written, and
synced in one round. The sections are renamed and their directories synced together.
Then the manifests are renamed and their directories synced together. Three waits.

**Durable when, and a crash.** ADR-0018, section 1, word for word: "A manifest in place
names only sections that are durably in place. No manifest is renamed before step 1 has
ended well", where step 1 ends with the sync of the sections' directories. A manifest's
temporary that is durable sooner is a temporary, which is "never read". Every subset of
the first round that a crash can leave has no file in place. If the first round fails,
all temporaries are removed and no section is in place, so none is unsure; if the
sections' renames or directories fail, it is today's failure of step 1, and the
manifests' temporaries are removed as well; a failure behind that is today's failure of
step 2.

**Saved.** One wait where two chunks or more are written (40 + 14 + 40 + 14 ms become
40 to 57 + 14 + 14). **None where one chunk is written**: one section and one manifest
are two trips in turn or together (measured, section 2).

### 6. The chunk thread takes what waits together

**Now.** `ChunkService::run` does one job at a time (`chunks.rs:366`). A checkpoint
waits for the whole of the checkpoint before it, then makes its own rounds. Measured:
the absorbed region's second checkpoint, which its players stand still for, waited 48 ms
behind the checkpoint the survivor was asked to make beforehand, and the survivor's
checkpoint for the merge 139 ms behind its own from a tick earlier. The order to
prepare the survivor is sent together with the order to release the other region, so
the two meet on this thread every time.

**Then.** When the thread takes a job that makes saves durable (`Checkpoint`, `Flush`,
`Return`, `Restore`), it first takes whatever else is in its queue already, in order:
saves are noted, loads answered, a restore's changes applied, and further jobs of that
kind are set beside the first. Then it syncs once, and then each of those jobs says
what it has to say, in the order they came. It takes no more than `PENDING_LIMIT`
saves or 64 such jobs (**guess**) into one go, so that it still syncs when it is never
idle.

**Durable when, and a crash.** "Nobody is told that something is durable before `sync`
has ended well" (ADR-0018, section 1) holds: each job speaks behind a sync that began
after every save before it was noted. A load behind a save is answered from what is
held, as now. If the sync fails, every handle that saved is lost, as now: what is
pending is one set whoever saved it. Nothing new is on disk at any point; a round has
more files in it.

**Saved.** No wait of an operation's own. A checkpoint waits for at most the rounds
that are under way when it arrives, and no longer for those of every checkpoint queued
before it.

### 7. Last, and only if it is still needed: the record of a merge or a split covers the region's own commits

**Now.** The store declines a merge or a split while the region has a commit that no
checkpoint covers (`may_absorb`, `lanes.rs:1613`; `may_split`, `:1734`), so the runner,
standing still, checkpoints and then hands the record in: with section 1 that is a
record of the state, a sync, an answer, and then the record that takes its place.

**Then.** The runner sends its saves and a flush and no checkpoint, and the commit
behind the flush's answer. The store takes it if every save the session has asked for
is durable (it counts those it passed on and those the chunk thread has reported with
a `Flushed`, `Checkpointed` or `Returned`), and declines otherwise (a new reason,
`Unsaved`). The record is the region's whole state at its tick, and `Lane::whole`
(`:154`) lets go of the region's commits up to it, in memory when the record is durable
and at a start when it is read. For a merge the absorbed region is as now: no commit of
it may be uncovered.

**Why ADR-0011 asked for more, and why this is enough.** Section 3.6 there: the chunks
and areas that come to the survivor "count as held from the tick of the merge or from
0, which is only right if no commit of either region from before is left to be
replayed". None is left: the record covers the survivor's, and their block changes are
in files that were durable before the store took the record. A crash before the record
is durable leaves the region with its last state and its commits, which are replayed
into chunks that have them already (`apply`, `chunks.rs:553`: "applying all of them
again in order ends in the same state").

**This changes a condition of ADR-0011, sections 3.6 and 3.7, and a reason of
`Decline`**, which is a message. It rests on the runner having saved every chunk it
changed, as a checkpoint does today. It saves one wait and one trip to the worker and
back. It is here so that it is known; **I would build it only if a measurement after
the other six says the 25 ms matter.**

### 8. The runner

Three places where a region waits and the store is not at fault (measured, section 7).

**8.1 A runner looks at once.** `RegionRunner::run`
(`services/worker/src/lib.rs:2255`) takes orders (`:2264` to `:2269`) and looks whether
the flush behind its first checkpoint is answered (`step`, `:1017`) once a tick; between
two ticks it takes answers and publishes (`:2282`), and sleeps the whole tick if nothing
is pending (`:2293`). Measured: 17 to 43 ms for an answer, 8 to 22 ms for an order, four
times in a merge. Then: between two ticks the runner waits for an order as long as it
would sleep (`recv_timeout` on the commands, with the release made one of them), and
while it prepares it looks at the flush every `COMMIT_POLL`, as it does for commits
already. A release and a reshape stop the region at the answer and not at the tick
behind it. No message changes, and no tick runs sooner or later than it does.

**8.2 One checkpoint before the stop, not three.** For a merge the survivor
checkpoints with the order to merge (`Prepare`,
`bin/clustine/src/cluster/worker.rs:777`), again with the order to absorb (`:811`, "if
the order to prepare was lost"), again when the runner takes the merge
(`take_command`, `lib.rs:1345`), and a fourth time when it has stopped. Measured: 292 ms
from the third being asked to the stop, with the absorbed region's players standing.
Then: `Reshape::Prepare` asks for a checkpoint **and a flush**, and the runner notes its
tick and whether the flush is answered. A `Prepare` within `PREPARED_FOR` ticks of the
last one asks for nothing. A merge that is taken while that flush is outstanding waits
for it and asks for nothing more; one that is taken within `PREPARED_FOR` ticks of its
answer **stops at once**; otherwise as now. **Guess**: 40 ticks, two seconds, which is
some chunks of a crowd and a few of anybody else. ADR-0014, section 3.1, step 1 ("the
region goes on ticking until the store has a checkpoint of it") is kept: it is the
checkpoint of the order to prepare.

A split has no order before it and keeps its first checkpoint.

**8.3 The second checkpoint is asked at the stop.** `carry_on` (`lib.rs:1380`) asks
for it only when every commit is confirmed, "so those commits have to be confirmed
first". The store sees to that itself: what a handle asks behind commits that are not
durable is held until they are (`lanes.rs:948`), and is dropped with the handle if
they never are. Then: the runner asks when it stops, goes on publishing as commits are
confirmed, and takes the next step when both the flush is answered and nothing is
pending. It saves a trip to the worker and back and the wait for the commit thread
behind it (measured: 25 ms).

## What it gives

Waits in turn before the answer, by the tests of the measurement
(`services/worldstore/src/syncs.rs`) for "now" and by this record for "then":

| Asked of the store | Now | Then | By |
|---|---|---|---|
| A commit | 1 | 1 | |
| The first commit behind a checkpoint | 2 | 1 | 2 |
| A claim, with its tick's commit | 1 | 1 | |
| A checkpoint, no chunk changed | 2 | 1 | 1 |
| A checkpoint of changed chunks | 6 | 4 | 1, 5 |
| … that lets a segment go | 8 | 4, and the table file behind the answer | 2 |
| A hello that changes the owner | 2, and 2 behind the answer | 1 | 3, 2 |
| The record of a merge | 3 | 1 | 2, 4 |
| The record of a split | 4 | 1 | 2, 4 |
| A take-over with commits to replay | 6 | 4 | 3, 5 |

A wait is not always one trip to the disk: the first of a checkpoint's four is a round
of files, which takes two. For one changed chunk that makes five trips where there
were six; for several, five where there were eight.

While a region stands still, under bots that change a chunk every tick:

| | Now | Then | With section 7 |
|---|---|---|---|
| A merge, the survivor's players: last commit, checkpoint, record | 2 + 6 + 3 = 11 | 1 + 4 + 1 = 6 | 5 |
| A merge, the absorbed region's players: its last commit and checkpoint, the hello, what the survivor does before it stops, and the survivor's | 2 + 6 + 2 + 12 + 11 = 33 | 1 + 4 + 1 + 0 + 6 = 12 | 11 |
| A split | 2 + 6 + 4 = 12 | 1 + 4 + 1 = 6 | 5 |
| A move: last commit, checkpoint, hello | 2 + 6 + 2 = 10 | 1 + 4 + 1 = 6 | |
| A take-over | 6 | 4 | |

**What that should take.** **Inferred**, by adding the unit costs of the measurement
(a sync of a file or of the log 20 ms, a round of two or more files 40 ms, a round of
directories 14 ms, half a sync for a request that finds the commit thread in one) along
the order of the trace, for four bots on a quiet disk. "Now" is the trace.

| | Now | Then | Of which the region stands still |
|---|---|---|---|
| A checkpoint of a few chunks, by itself | 106 to 205 ms | 88 ms | |
| **A merge, by the command** | 1,088 ms (950 to 1,650) | **about 380 ms** | the survivor about 140 ms (now 160 to 205); the absorbed region about 280 ms (now 824) |
| **A split, by the command** | 320 to 1,190 ms | **about 235 ms** | about 140 ms (now 183 to 527) |
| **A move, by the command** | 300 to 1,000 ms | **about 235 ms** | about 140 ms to the hello's answer (now 220) |
| **A take-over**, the store's part behind the lease | 100 to 140 ms (not traced: the count times the unit costs) | **about 90 ms** | all of it |

How the merge adds up: the order 1; the absorbed region's first checkpoint 88, with
the survivor's in the same rounds; its stop 1, its last commit 20, its second checkpoint
88; `Released`, the order to absorb and the hello's wait 13; the hello 20; the state and
the order to the runner 2; the survivor's last commit 20, its checkpoint 88, the record
20, two waits for the commit thread 10; the end 5. With section 7, 25 less.

On a disk as busy as the traced one, where the log is being synced half of the time,
each wait of the chunk thread takes up to twice as long: **a merge then takes up to
about 580 ms**, a split and a move up to about 370 ms.

A player waits longer than the region stands still by what the edge needs to find the
region again and say hello, 40 to 100 ms in the trace, and after a move by what the new
owner needs to load what its players see, 600 ms in the trace. Neither is the store's
syncs, and neither is changed here.

## What cannot go below what

- **A commit is one trip.** ADR-0008: it "is answered once the record is on disk, and
  only then". A tick is published 20 ms behind its end at the soonest, and all regions
  share that trip.
- **A chunk in its files is three trips**: the files, the sections' directories, the
  manifests' directory. A section must be durably in place before a manifest that names
  it is in place, and each is put in place by a rename whose directory is synced. Two
  files are two trips however they are synced, so a single changed chunk costs 20 + 14
  + 20 + 14 ms, or 40 + 14 + 14 ms, which is the same. Below that is only a file that is
  synced anyway: chunks in the log, which ADR-0018 ruled out for its format and its
  compaction, and which I do not take up again for a pause of 70 ms.
- **A region that is merged or split stands still for five trips at least: its last
  commit, three for the chunks that changed since the checkpoint before, and the
  record.** The last commit cannot share a trip with the chunks, because a save is
  "written only once that commit is on disk" (ADR-0008). The chunks cannot wait until
  after the record, because ADR-0011 takes a merge or a split only of regions none of
  whose commits is uncovered. To let the survivor's commits stand across a merge, for
  the store to replay, would leave two trips, about 50 ms. **Inferred**: it would be
  right for the survivor's own chunks, whose ticks stay as they are in the table, and
  wrong for the part of a split, whose chunks nobody replays. It is another record's
  question and not asked here.
- **A hello is one trip**: the epoch is on disk before the answer.
- **The absorbed region's players wait for both regions**: for their own region's last
  checkpoint, the hello, and all of the survivor's standstill, because the survivor
  cannot work the merge out before it has the other's state. Twelve waits as this
  record leaves it.
- **A take-over is one trip and the three of the chunks**: ADR-0018, section 1a, has
  what an open applied in the files before the region is handed over.
- **A request waits for the sync the commit thread is in**, half a sync on average. One
  thread writes the log, and that is what makes one sync serve every region.
- And none of it is below the 187 ms and 63 ms that a merge and a move take with the
  world in memory, of which most is ticks and messages.

## Tests

Written from this record by someone who does not write the change, in
`services/worldstore` unless it says otherwise. The store's kill tests (`kill.rs`,
`kill_regions.rs`, `kill_unpinned.rs`, `scenarios.rs`, `stays_kill.rs`, `rounds.rs`)
stop at every change and sync of the simulated disk and walk every point of the new
order by how they are written; where one names a point by its number, the number is
found again.

For all of it:

1. `syncs.rs` expects what the column "Then" has, operation by operation, and fails if
   the store waits for the disk once more than that. It is the measure of this record.

For section 1:

2. A checkpoint that was answered is what the region is restored with after a crash
   that leaves nothing that was not synced; one that was not answered is there or
   not, and in both cases every commit that was confirmed is in what the region is
   restored with. At every change and sync of a scenario with two regions, a
   checkpoint of each and commits behind them.
3. A world with state files and no record of a state opens as before. A state file
   below a record's tick is passed over; a record below a state file's tick is.
4. A group with the record of a state whose sync fails: nothing is answered, every
   handle is lost, the record does not count after a crash once the cut is durable,
   and the region opens with the state before and its commits.
5. A state longer than a record may be is written as a file and counts.
6. A checkpoint of a session that has been replaced leaves no record.

For section 2:

7. A segment below `CLOSE_AT` is not closed by a checkpoint, and one above it is. The
   log is still emptied: after every region has checkpointed twice, no segment from
   before the first is left.
8. A handle's flush is answered before the table file is written, and `Store::flush`
   returns after. A kill between the two leaves a world that starts with the table as
   the log has it.

For section 3:

9. A hello with a higher epoch is answered only when its record is durable: with the
   disk stopped at the sync of the log, there is no answer. After a crash right behind
   the answer, a hello with the epoch before is refused, the file not having been
   written; after the start that follows, the file has the epoch.
10. No segment that holds the only word of a region's epoch is removed before the
    region's file is written, also if writing the file fails once.
11. A hello with the epoch the lane has writes no file and is answered before any
    sync, for the part of a split whose file is not written yet.
12. The first hello of a pinned region writes its file with its block of entity ids
    before it is answered, as now.

For section 4:

13. A split and a merge are answered with one wait behind them (`syncs.rs`). A kill at
    every point behind the record leaves the split or the merge as a whole, the part's
    file written and the absorbed region's files gone after the next start.

For section 5:

14. The tests of ADR-0018 numbered 2, 3 and 4 there, on the new order: three rounds;
    stopped at every point and with every part of a round synced, every chunk is as
    before or as saved and no manifest names a section that is not there; with every
    step failing in turn, the same, and no section taken for stored that a crash
    could take away.

For section 6:

15. Two regions that checkpoint while the chunk thread is held are written in one set
    of rounds and both answered; a sync that fails loses both and no third; a load
    behind a save in the same go gets what was saved; a job is answered only behind a
    sync that began after its saves.

For section 7, if it is built:

16. A merge and a split with commits that no checkpoint covers are taken when every
    save is durable and declined as `Unsaved` when one is not. Killed at every point,
    the region is restored as before with its commits, or as the record has it with
    none of them, and every block is as it was built.

For section 8, in `services/worker`:

17. A runner that is stepped by hand stops in the step that finds the flush answered,
    and one that runs takes an order without a tick passing (a test of its own clock:
    it waits for the stage, not for time).
18. A merge taken while the flush of a `Prepare` is outstanding asks the store for no
    checkpoint before it stops; one taken within `PREPARED_FOR` ticks of its answer
    stops in its first step; one taken later checkpoints first.
19. The second checkpoint is asked in the step that stops the region, and the merge is
    handed in only when every tick is published.

And once, on the disk it is for: the test of the trace
(`players_stand_still_only_briefly_when_regions_are_merged_split_and_moved`), before and
after each step, with what the commands took and what the bots waited written into the
roadmap.

## The order to build it in

By what each gives for what it risks. Each is one commit with its tests.

| Step | What | Gives, in the traced merge |
|---|---|---|
| 1 | Section 8.2 and 8.1: one checkpoint, and looking at once | about 290 ms and 120 ms, with no change to the store |
| 2 | Section 4, and the second half of section 2 (behind the answers) | 3 waits of a merge, 2 of a split, 2 of a checkpoint that lets a segment go |
| 3 | The first half of section 2 (`CLOSE_AT`) | 4 waits of a merge, 2 of a split, 1 of a move |
| 4 | Section 6 | 48 ms of waiting on the chunk thread, and 139 ms more where step 1 is not built |
| 5 | Section 1 | 1 wait of each checkpoint, 2 where its group has a commit to sync anyway |
| 6 | Section 3 | 1 wait of a hello and 2 behind it |
| 7 | Sections 5 and 8.3 | 1 wait of a checkpoint of several chunks; a trip to the worker |
| 8 | Section 7, if a measurement asks for it | 1 wait |

Steps 1 to 4 change no format and no rule. Step 5 adds a kind of record: a world it
wrote is not read by a store from before it. Step 6 changes when a region file is
written and not what is in it.

## Ruled out

- **Making the syncs that stay wait together** (the state file's temporary with the
  manifests' round; the record with its segment's directory; the two threads' syncs).
  Measured: two together cost what two in turn cost.
- **One `fsync` in place of `fdatasync` and the directory's sync for a new segment.**
  On this file system a new file's `fsync` seemed to make its name durable too (in the
  measurement a directory's sync behind one took no time). POSIX does not say so, the
  simulated disk rightly does not, and section 2 makes new segments rare instead.
- **Making the next segment beforehand**, behind an answer. It moves the wait to
  whoever asks next.
- **A release without its second checkpoint**, leaving the commits for the hello to
  replay. The chunks are then written by the hello, in as many trips, and a merge
  needs the absorbed region's state covered anyway.
- **Sections under their final names, without a rename**, told from a file that a crash
  cut off by their hash. It needs every section read and hashed before it is taken for
  stored, which ADR-0018 already lists as a doubt it left ("`unsure` does not outlive
  the process").
- **Chunks in the log.** As ADR-0018.
- **A second commit thread, or a log per region.** It gives up the one sync for all
  regions' commits, which is what keeps a busy store at one trip a tick.

## Risks

- **A region without an owner keeps its segment** (section 1), and with it every
  segment behind it. Today that is so only of a region that was merged or split and
  never checkpointed since. If the log is seen to grow for it, the remedy is the one
  the table has: before the first segment is let go, the states that keep it are
  written as state files. Not built until it is seen.
- **The log is larger**: a state per region and interval. **Guess**: a state is some
  hundred bytes a player and less than a megabyte for a crowd; not measured.
- **A hello's epoch is in two places** (section 3), and a rule about which segment may
  go rests on a flag in memory. The same is true of a split's part today, and
  `Lanes::align` puts the file right at every start.
- **A disk that is not this one.** On a disk where syncs do wait together, or cost a
  tenth of a millisecond, nothing here is slower: every step makes fewer syncs, none
  makes more.
- **Tests that look at files behind a handle's flush** (section 2) have to ask the
  store's flush for the table file and the segments. How many do was not counted.

## Guesses, and what was not checked

- `CLOSE_AT` (4 MiB), `PREPARED_FOR` (40 ticks) and the limit of jobs in one go of the
  chunk thread (64) are guesses.
- The times under "What that should take" are sums of measured unit costs, not
  measurements. The order of the sums is the trace's.
- That the disk makes "one trip at a time" is my reading of the measurement, on one
  machine, one file system and one evening.
- A crowd. A hundred players' checkpoint has rounds of a hundred files and more;
  ADR-0018 measured two hundred chunks at 0.7 s, half of it describing them. The waits
  saved here are the same number for a crowd, and a smaller share of its pause.
- Whether a state of a crowd fits a record comfortably (the limit is 64 MiB).
- The 600 ms after a move in which the new owner makes no commit.
- Section 7 has not been argued against a split whose part is given chunks that a
  return was under way for, nor against stay notes (ADR-0020), which no tick writes
  yet.

## Changes to other records, when this is one

- ADR-0008, section 3: "writes `state` as the region's state file" becomes "keeps
  `state` as the region's latest whole state"; the store "keeps the highest epoch on
  disk", in the region file or in the log.
- ADR-0011: section 1 gains the record; section 3.4 and the row "Opening" of 4.3 say
  that the record of an opening is synced before the answer and the region file
  written later; sections 3.6 and 3.7, steps 5, come behind the answer; section 3.5
  says when a segment is closed. With section 7 here, the conditions of 3.6 and 3.7.
- ADR-0018: section 1's two steps become three rounds; "What stays in turn" is
  answered by this record.
- `docs/world-format.md`: the record of a state; that a state file and a region file
  may be older than the log.
