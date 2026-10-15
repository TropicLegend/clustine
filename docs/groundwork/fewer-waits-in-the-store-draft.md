# Draft of a decision record: Fewer waits in the store

- Status: **Draft, set aside on 2026-10-13.** Its own count shows that it would take
  about 0.06 s off a merge; the half second it looked for was in the runner, which
  made two checkpoints too many for a crowd (ADR-0014, section 3.1, the note of
  2026-10-13: a merge's 1.02 s became 0.65 s). It is kept for its count of the
  store's waits in turn and is taken up again if those 0.06 s are wanted.
  To be numbered. Not reviewed, nothing of it built. Written
  2026-10-13 against `main` at `d1c11b3`, read-only: nothing was built, run or
  measured for it. It takes up the two steps that
  [the review](few-syncs-in-turn-review.md) of
  [the first draft](few-syncs-in-turn-draft.md) left to build (its steps 3 and 4) and
  writes them against the code as it is now, after the runner's changes (ADR-0014,
  section 3.1, the note of 2026-10-11) and the one file a chunk is (ADR-0022,
  section 3). It changes the world store's commit thread and nothing else: no message,
  no kind of record, no file's layout. Every guess is marked **Guess**; "counted"
  means that a test of `services/worldstore/src/syncs.rs` asserts it today, "read"
  that I read it in the code at the place named.
- Date: 2026-10-13

## Context

Clustine runs on network storage. A synced write takes about 22 ms there and the
sync of a directory about 14 ms; two or three files synced at the same time take
40 ms, sixty-four take 84 ms, and syncs in turn each pay
([the measurement](disk-syncs-measured.md), section 2). Those are the unit costs
used below: **22 ms for a file or the log, 14 ms for a directory, 40 ms and more for
a round of chunk files.**

Measured on 2026-10-11, optimised, on that disk, a crowd of a hundred and two
players who walk away and back (`docs/roadmap.md:1148-1167`):

| | the crowd waited | the two who walk waited | the coordinator took | the home region stood still |
|---|---|---|---|---|
| split | 0.36 / 0.38 / 0.41 s | 0.41 / 0.46 / 0.51 s | 0.38 / 0.42 / 0.44 s | 0.20 / 0.22 / 0.24 s |
| merge | 0.36 / 0.36 / 0.40 s | **0.86 / 1.02 / 1.15 s** | 0.99 / 1.03 / 1.13 s | 0.18 / 0.20 / 0.22 s |

ADR-0017, section 9.7, takes half a second in the middle and a second at worst to be
what a player bears, "optimised, for the crowd of a hundred"
(`docs/adr/0017-the-end-of-the-stripes.md:3011-3012`). The crowd is within that. The
same section says that the worker's number leaves out "for a merge the longer stop of
those who were in the region that was absorbed": that longer stop is the 1.02 s, and
it is what this record was asked about.

## 1. Count first

### 1.1 The store's waits in turn today

All **counted** (`syncs.rs`, the tests at `:427`, `:443`, `:461`, `:542`, `:573`,
`:632`), but for the players' file, which is read. A wait is a sync of a file, of a
directory, or a round. "C" is the commit thread, "K" the chunk thread. Every one of
these waits is the store's. What the runner decides is which checkpoints are asked
for and when; it makes no wait of its own, and since 2026-10-11 it asks for the last
checkpoint as it stops (`services/worker/src/lib.rs:1176-1179`).

| Asked of the store | Waits in order | Before the answer | At 22 / 14 / 40 ms |
|---|---|---|---|
| **A release, from the stop**: the last tick's commit | C the segment, C `log/` | 2 | 36 ms |
| … the last checkpoint, no chunk changed since the one before | K the state's temporary, C `regions/` | 2 | 36 ms |
| … the same with chunks changed | K {chunk files}, K {their directories}, K the state's temporary, C `regions/` | 4 | 90 ms and more |
| … if that checkpoint leaves the first segment kept for the table alone | then C the table's temporary, C `regions/` | 2 more | 36 ms more |
| … and for the stay notes alone (**read**: `lanes.rs:1130-1154`) | then C the players' temporary, C `regions/` | 2 more | 36 ms more |
| … the handle is dropped | none | 0 | |
| **The open by the next owner** (hello to `Opened`) | C the region file's temporary, C `regions/` → answer; then C the segment, C `log/` | 2, and 2 behind the answer | 36 ms, and 36 ms of the commit thread |
| **The record of a merge** | C the segment, C `log/`, C `regions/` | 3 | 50 ms |
| **The record of a split** | C the segment, C `log/`, C the part's region file's temporary, C `regions/` | 4 | 72 ms |
| … the hello for the part | → answer; then C the segment | 0, and 1 behind | 22 ms of the commit thread |
| **A move**: a release and the open | the rows above | 8 with chunks changed, 6 without, to the hello's answer | 162 or 108 ms, and 36 ms behind |

A region that is merged or split stands still for its last commit, its last
checkpoint and the record: 2 + 4 + 3 = **9 waits, 176 ms** for a merge and
2 + 4 + 4 = **10 waits, 198 ms** for a split, with a round of two or three chunk
files. That fits what was measured for the home region (0.20 s and 0.22 s in the
middle), whose rounds are of more files.

Two of these waits come from one line. Every group that puts a state file in place
closes the segment of the log (`lanes.rs:996-1000`), so whatever is appended next
begins a segment, whose name is made durable by a sync of `log/`
(`Log::sync`, `lanes.rs:2073-2076`). That is the `log/` in four rows above. The same
lines run `collect` and the two trims **before the group's answers**
(`lanes.rs:1001-1008`, answers from `:1011`), which is where the table file and the
players' file come into a checkpoint that somebody stands still for.

### 1.2 What the absorbed region's players wait for, and what the counts do not explain

The store's waits in turn on their way, by the table: the release 6 (126 ms), the
hello 2 (36 ms), the survivor's standstill 9 (176 ms, measured 0.20 s with the
crowd's rounds): **17 waits, about 0.34 to 0.36 s**. After the record their edge
finds the region again, which for the crowd is 0.36 − 0.20 = 0.16 s (measured), and
their chunks are loaded (section 7).

That is 0.5 to 0.55 s of 1.02 s. **About half a second is not in any wait counted
here.** The coordinator's own numbers say the same: a split takes it 0.42 s, a merge
1.03 s, and the release (a first checkpoint while ticking, then 0.13 s), the hello
(0.04 s) and the survivor's standstill (0.20 s) do not add up to more than 0.55 s.

**Guess**, from the code and not from a trace: it is the survivor checkpointing
while it still ticks, after the absorbed region has stopped. The crowd's bots place
and dig without a pause (ADR-0018, Context), so more than `FEW_UNSAVED` = 8 chunks
(`services/worker/src/lib.rs:119`) have unsaved changes within a few ticks of any
checkpoint. Then:

- the second `Prepare`, sent with the order to absorb
  (`bin/clustine/src/cluster/worker.rs:814`), asks for a new checkpoint if the first
  one's flush is answered (`lib.rs:1467-1474`);
- when the merge is taken and that flush is answered, `stop_if_prepared` finds more
  than eight chunks unsaved again and makes "one more while ticking"
  (`lib.rs:1157-1167`);
- each is a checkpoint of the crowd's chunks, about 0.2 s by the split's first
  checkpoint (0.42 − 0.22 s), and the absorbed region's two players stand through
  both.

That is the trade the review's finding 6 asked for (a short standstill of the
survivor), and with a crowd that digs it is paid by the absorbed region's players.
It is the runner's and not this record's. **What settles it** without a new run: the
worker's log of one merge of the run of 2026-10-11, where "releasing the region",
"the region is released", "much has changed since the region was told to prepare",
"the region stops ticking" and "handing the store a merge or a split" each have a
time (`lib.rs:1384`, `:1564`, `:1158`, `:1169`, `:1659`). If the logs are gone, one
merge of `crowds.rs` with the trace of the measurement's section 3.

### 1.3 The same table with each change

Changes A, B and C are sections 2 to 4. "D" is the hello of section 5, which this
record writes out and does not build. A is a rule of recovery and saves no wait.

| Asked of the store | Today | With B | With B and C | (With D) |
|---|---|---|---|---|
| A release: the last commit | 2, 36 ms | 1, 22 ms | | |
| … the last checkpoint with chunks | 4, 90 ms | 4, 90 ms | | |
| … the table file and the players' file in it | 0, 2 or 4 more, to 72 ms | none, but at one checkpoint in every `CLOSE_AT` bytes of log | | |
| The open by the next owner | 2 and 2 behind: 36 + 36 ms | 2 and 1 behind: 36 + 22 ms | | 1 and none behind: 22 ms |
| The record of a merge | 3, 50 ms | 2, 36 ms | **1, 22 ms** | |
| The record of a split | 4, 72 ms | 3, 58 ms | **1, 22 ms** | |
| … the hello for the part | 0 and 1 behind | 0 and 1 behind | 0 and 1 behind (section 4.2 keeps it so) | |
| A move, to the hello's answer | 8, 162 ms | 7, 148 ms | 7, 148 ms | 6, 134 ms |
| **A merge, the survivor's standstill** | 9, 176 ms | 7, 148 ms | **6, 134 ms** | |
| **A split, the region's standstill** | 10, 198 ms | 8, 170 ms | **6, 134 ms** | |
| **A merge, the store's waits on the absorbed region's players' way** | 17, 338 ms | 14, 296 ms | **13, 282 ms** | 12, 268 ms |

What that makes of the measured numbers, **by this arithmetic and not measured**:

| | Measured, middle | With A, B, C |
|---|---|---|
| The crowd at a merge | 0.36 s | about 0.32 s |
| The crowd at a split | 0.38 s | about 0.32 s |
| The two who walk at a split | 0.46 s | about 0.40 s |
| **The absorbed region's players at a merge** | **1.02 s** | **about 0.96 s** |

Of that, 28 ms of a merge and 50 ms of a split are certain: the record's `log/`,
the removal, the part's file. The other 14 ms is the `log/` behind the last tick's
commit, which is saved only where that commit is still on its way when the region
stops ("Guesses"). Where the table file or the players' file fell into a standing
checkpoint (36 or 72 ms each; how often it does was not counted), that goes as
well, so the worst cases gain more than the middle ones. **Guess**: the 1.15 s has
one in it.

So this record is worth about a fifth of the home region's standstill at a merge
and nearly a third at a split, for everybody, and **it does not bring the absorbed
region's players within the bound, and no change to the store's waits in turn
can**: all seventeen together are a third of their second. What would is in 1.2,
and is the runner's.

## Decision

**A segment of the log that is not the last is whole or the store does not start; a
checkpoint closes the segment only when it is full; and the record of a merge or a
split is answered when it is durable, with no file behind it.** Three changes to
`services/worldstore/src/lanes.rs`, each a rule of what is on disk before which
answer. The hello by its record is written out in section 5 and left unbuilt, for
the reason given there.

## 2. Change A: a segment that is not the last ends with a whole record

Answers the review's finding 2. It is first because B makes segments live longer.

**Now.** `Lanes::load` reads every segment up to its first record that is cut off
or fails its checksum and discards how many bytes were good, for every segment
(`lanes.rs:325-335`; `read_log_with_offsets`,
`crates/clustine-format/src/log.rs:578-605`). So one wrong bit in a segment hides
that record and everything behind it: other regions' commits, grants, returns, a
merge, a split. Nobody is told. A damaged state file or table file is refused by
name (`lanes.rs:262-268`, `:283-286`).

**Then, two rules.**

- **A1.** In every segment but the highest-numbered, the records that can be read
  end at the segment's last byte, or the start fails with an error that names the
  segment's path and the offset of the first byte that could not be read. Nothing is
  changed on disk. `FormatError::Corrupt` takes a fixed text
  (`crates/clustine-format/src/lib.rs:43`), so the offset needs a variant of
  `StoreError` of its own beside `Damaged` (`services/worldstore/src/lib.rs:86`).
- **A2.** In the highest-numbered segment, what follows the records that can be read
  is cut off and the cut is synced, before the start appends anything and before any
  hello is taken. If it cannot be, the start fails. This is `Log::settle`
  (`lanes.rs:2103-2116`) with `unsettled` set by `load`: the act a running store
  already performs on a group that failed.

**A2 is not in the review and A1 is wrong without it.** A store that starts appends
to a new segment (`lanes.rs:310-318`: `active: None`), so the segment a crash tore
is the last one only until the next append. Under A1 alone, a world that crashed in
an append would start once and be refused at the start after.

**Why only the last segment can be torn.** A segment is closed only when all of it
is durable: `Log::close` does nothing while something is unsynced
(`lanes.rs:2119-2123`), and the limit closes it behind a sync that ended well
(`:2077-2080`). After a failed write or sync nothing is appended anywhere until the
cut is durable (`Log::append`, `:2023-2029`). With A2 the same holds across a start.

**Durable when; a crash at each point.** No answer changes. A2 adds one truncation
and one sync to a start that follows a crash, and none to any other.

| The start is killed | What is left | The next start |
|---|---|---|
| before the truncation | the tail, in the last segment | cuts it |
| between the truncation and its sync | the tail or not (`Survival::Untruncated`, `services/worldstore/src/disk.rs:410-430`), in the last segment | cuts it or finds nothing to cut |
| after the sync | no tail | appends to a new segment; this one is whole |

What is cut off was never answered: a group is answered behind its sync
(`end_group`, `lanes.rs:962-967`, then `:1011-1039`).

**What A does not catch.** A wrong bit in the *last* segment still reads as a torn
tail, and A2 then cuts off good records behind it for good, where today they stay
on disk unread. The start says how many bytes it cut, as a warning. A torn write
and damage cannot be told apart by the bytes (a crash may keep a later page and
lose an earlier one), so this is left; "Risks" has what it means with B.

**A world of today's store** that crashed in an append and was started since has a
torn segment that is not the last, until a checkpoint lets it go. The store with A
refuses it and names the segment. Today's store closes at every checkpoint, so one
clean stop empties the log (`services/worldstore/src/stays.rs:962-1000` asserts
it). Worlds are not carried over before the first release
(`services/worldstore/src/lib.rs:89-91`), so nothing is built for this.

## 3. Change B: a checkpoint closes the segment when it is full

The first half of the first draft's section 2, with the review's findings 5 and 8.
The second half ("what follows a close comes behind the answers") is dropped; 3.3
says why.

**Now.** As 1.1: every group that put a state file in place closes the segment, and
then, before its answers, collects and writes the table file and the players' file
if the first segment is kept for them alone.

**Then.** The store has a setting, `close_at`, in bytes. In `end_group`, where a
state file was made durable (`lanes.rs:996`):

1. the segment is closed **if it holds at least `close_at` bytes** (`Active::written`,
   `lanes.rs:2010`);
2. `collect` runs, as today, **after every group that made a state durable**. It
   never removes the segment being appended to (`remove_while`, `:2191-2196`) and
   syncs nothing;
3. the table file and the players' file are written only if no segment is being
   appended to. Both trims already look for that and return otherwise (`:1141`,
   `:1170`), because each names `log.next` as the first segment its file does not
   stand for (`:1148`, `:1183`). So they run at a close and at no other time.

That is one condition around `self.log.close()` (`lanes.rs:1000`). The limit of
64 MiB stays (`SEGMENT_LIMIT`, `:67`).

### 3.1 The setting's values

| Value | Who uses it | What it does |
|---|---|---|
| `0` | the store's suites as they are: `store_on` and the other callers of `start` in `regions.rs`, `scenarios.rs`, `unpinned.rs`, `stays.rs`, `stripes_end.rs`, `tests.rs`, `rest.rs`, `rounds.rs` (`start`, `services/worldstore/src/lib.rs:586`) | every checkpoint closes, as today. Nothing these suites expect changes: not scenario 11's lists of segments (`regions.rs:1234`), not rule P's "no segment is left" (`stays.rs:962-1000`) |
| a few hundred bytes | the kill tests and the new tests of this section | a close falls in the middle of a scenario, after some checkpoints and not others |
| `CLOSE_AT` = 4 MiB (**guess**) | `Store::memory_divided` and `Store::local_divided` (`lib.rs:294`, `:325`), so every worker's test, every end-to-end test and the server | what this section describes |

`start` takes the value; the public constructors pass `CLOSE_AT`; one more public
constructor takes it from its caller. The binary passes what `CLUSTINE_LOG_CLOSE_AT`
says where that is set (`bin/clustine/src/cluster/worldstore.rs:26`,
`bin/clustine/src/lib.rs:199`), so that one build is measured with 0 and with 4 MiB
on the real disk; the processes of a test's cluster inherit it.

This answers finding 5: the suites that are the specification of recovery are not
rewritten by whoever builds the change. What they then check is the store at a
setting the server does not run with, which is why the kill tests run with two
values and `syncs.rs` with the server's (section 9).

One test outside the store sees the difference:
`checkpoints_save_loaded_chunks_and_empty_the_log`
(`services/worker/src/lib.rs:4839-4878`) asserts that the log's files are empty
after a checkpoint. It opens its store with `close_at` 0 and stays as it is.

### 3.2 Durable when; a crash; what a start reads

**No answer changes what it stands for.** A checkpoint counts when `regions/` is
synced, as today (`lanes.rs:972-987`). A segment is removed only when no lane needs
it and the table's and the players' files stand for it (`collect`, `:1096-1107`),
as today. Closing a segment was never a promise to anyone; the comment says it is
"so that this one can be removed" (`:997-998`).

**A crash at any point** leaves what a crash leaves today in a world whose segment
was not closed since its last checkpoint, which is every world between two
checkpoints. Recovery reads a commit by its tick against the state file's
(`:343-357`), a table record by its segment against the file's `from`
(`changed_by`, `:1940-1948`), a note by its segment against the players' `from`
(`:360-363`). None of that asks whether a checkpoint closed anything.

**Rule P of ADR-0020** ("no segment is ever numbered below the players' file's
`from`") holds as it is: the file is still written only when no segment is being
appended to, with `from` = the next one, and `Log::append` still holds to it
(`:2038-2040`). Its sentence "after a clean stop no segment may be left at all" is
no longer true at the server's setting: one is left, the one being appended to. The
start after reads it by the three rules above.

**What a start reads.** **Inferred**: right after a close, what is left on disk was
written since the oldest checkpoint of any region that has an owner, one interval
at most, as today. Until the next close up to `close_at` bytes more and what is
appended until the next checkpoint come on top. So a start reads at most about
4 MiB more than today. A quiet world never fills its segment: its table file and
players' file are not written again and every start reads that one segment, less
than 4 MiB. How long reading 4 MiB takes on the network disk was not measured
(**guess**: tens of milliseconds).

### 3.3 "Behind the answers" is dropped: it moves the wait

The review's finding 4, followed through in today's code. What follows a close
would run on the commit thread right behind the answers. The next request is:

- after a release's last checkpoint, the hello of the next owner, which the trace
  has 3 ms behind the flush's answer (the measurement, section 5, row 538 to 541);
- after a merge's or a split's last checkpoint, the record, which the runner hands
  in in the step that finds the flush answered (`lib.rs:1562-1572`).

Both would wait behind the table's and the players' files: 36 to 72 ms moved, about
3 ms saved. **So it is not built.** What does save the wait is B: the files are
written at one checkpoint in every `close_at` bytes of log and not at every one.
When a close does fall into a standing checkpoint, that one pays as today.
**Guess**: with a crowd the log grows by 4 MiB in twenty to forty seconds (a
hundred players' changes of state at a few kilobytes a tick; not measured), and of
the four or five checkpoints of a merge two are made standing, so one merge in
several has it. `ls -l` on `log/` during `crowds.rs` settles the first number.

## 4. Change C: the record of a merge or a split is answered with no file behind it

The first draft's section 4, changed in both halves.

### 4.1 A merge: the removal is not synced

**Now.** `Lanes::absorb` makes the record durable by itself (`write_alone`,
`lanes.rs:1536`), changes the table, removes the absorbed region's state file and
its region file if that has no entity ids, **syncs `regions/`** (`:1553-1567`), and
answers (`:1577`). A failure of the removal is logged and passed over (`:1568-1570`).

**Then.** The files are removed and `regions/` is not synced for it. The removal is
durable with the next sync of `regions/` that anything asks for, which is the next
checkpoint of any region.

**Durable when; a crash.** The answer rests on the record, which is durable. If a
crash brings a file back, the start finds a region file or a state file of a region
the table does not have and removes it, with a sync (`Lanes::align`,
`lanes.rs:1906-1926`): the row "Merge" of ADR-0011, section 4.3, "step 4 of 4.2
removes what is left". A hello for the absorbed region is refused by the table
before any file is looked at (`:1340-1345`), so the file cannot be read by mistake
meanwhile. Nothing is moved to a later request: no sync is made at all.

**Saved.** One wait, 14 ms, of every merge, for both regions' players.

### 4.2 A split: the part's region file is written with the table file

**Now.** `Lanes::split` makes the record durable, makes the part's lane with
`unwritten` set, **writes the part's region file and syncs `regions/`**
(`write_region_files`, `lanes.rs:1719-1721`, `:1792-1816`), and answers (`:1723`).
A failure is logged and passed over: the record has the part's epoch.

**Then.**

- `split` does not write the file. The lane stays `unwritten`.
- **A hello that changes neither the epoch nor the entity ids writes no file, also
  for a lane that is `unwritten`.** Today `admit` takes `unwritten` for a reason to
  write (`lanes.rs:1389-1390`). The worker says hello for the part right behind the
  split's answer (`bin/clustine/src/cluster/worker.rs:1218-1228`) and the part's
  players wait for it, so without this rule the two waits would move from the split
  into that hello: finding 4's first case. A hello that does change the epoch or
  the ids writes the file as today and clears the flag.
- The file is written where it is written today when it could not be at the split:
  before the table file (`trim_for_the_table`, `:1174-1179`, which writes nothing
  and lets no segment go if the region file fails) and at a start (`align`, `:1932`).
- An absorbed region whose file was never written has none removed (`:1557-1559`
  would otherwise ask the disk to remove a file that is not there).

**Durable when; a crash.** The part's epoch is on disk before the split is
answered: it is in the record (`part_epoch`). `load` takes it from there for a lane
whose file is missing or lower (`lanes.rs:446-456`) and `align` writes the file.
The record's segment cannot go before the file is durable: it is kept for the table
from the file's `from` up to `table_last` (`collect`, `:1098-1105`; `table_last` is
set at `:1691`), and only `trim_for_the_table` raises `from`, behind
`write_region_files`. This is the state that a store which could not write the file
is in today, and
`the_region_file_of_a_part_that_could_not_be_written_is_written_later`
(`services/worldstore/src/scenarios.rs:3503`) already holds the store to it
"after the table file has taken the place of the split's record".

**Is the wait saved or moved?** Saved, given B. The file is written at a close,
which B makes rare; with `close_at` 0 it would be written by the next checkpoint
that writes the table file, which is as likely a standing one. That is why C comes
behind B, as the review asked.

**Saved.** Two waits, 36 ms, of every split, for the region's players and the
part's.

## 5. The hello by its record: written out, not built

The first draft's section 3 and the review's finding 11. The rule is given so that
it can be judged; "Why not now" follows it.

**Now.** `Lanes::admit` writes the region file and syncs `regions/` when the epoch
or the entity ids change (`lanes.rs:1388-1402`), appends `Opened { region, epoch,
restored }` (`:1452-1460`) and is answered (`:1297`); the record is synced when the
group ends, behind the answer.

**The rule.** For a hello that raises the epoch and issues no block of entity ids:

1. the owner there was is lost and the region is read, as today (`:1410-1447`);
2. `Opened` is appended **and the log synced by itself, before the answer**. Only
   then does the lane have the epoch in memory, and it notes the segment that record
   is in as the only place its epoch is written;
3. no region file is written. `needed_by_lanes` (`:1085-1091`) counts that segment
   as needed, so `collect` cannot remove it, whoever calls it;
4. at a close, before the trims, the files of all such lanes are written and
   `regions/` synced once, and the notes are cleared; if that fails nothing is
   cleared and nothing goes;
5. `load` takes the epoch of an `Opened` that is above the lane's file's, as it
   takes a split's `part_epoch`, and `align` writes the file.

A hello that issues a block writes the file before the answer, as today. One that
changes nothing is as today and as 4.2.

**What is on disk before the answer:** the epoch, in a record that is whole and
synced. **A crash** before the sync has ended: the epoch is the old one or the new,
nobody was answered; with the new one the same owner says hello again and is taken,
and a lower one is refused, which is right, since the coordinator issued the higher.

**The failure is split brain.** The coordinator takes "no epoch at or below one the
list has" from the store's list (`services/coordinator/src/state.rs:1627-1637`),
the list has the lane's epoch (`lanes.rs:665-670`), and the store takes an equal
epoch for the same owner come back (`:1347-1358`). If a start ever comes up with an
epoch lower than one it answered, the coordinator can issue that epoch again, and
two workers hold one region under it.

**The two details the review found missing:**

- `load` acts on an `Opened` only for a lane that exists already
  (`lanes.rs:368`: `regions.get_mut`). To take an epoch it has to make the lane, as
  it does for a split's part (`:444`); `align` drops the lanes the table does not
  have (`:1906-1914`).
- `make_over` collects before `align` writes region files (`lanes.rs:1897`; `align`
  is called at `:561`). Rule 3 above answers this better than "write the files there
  too": the segment is needed for as long as the note stands, so no order of calls
  can let it go. The test is a make-over killed at every point, and a region file
  that fails to be written followed by a kill.

**Why not now.**

- It saves 14 ms before the answer (36 → 22) and, with B, 22 ms of the commit
  thread behind it. That is 14 to 36 ms of a second whose measured spread is
  0.86 to 1.15 s: `crowds.rs` cannot show it.
- A1 refuses damage in a closed segment. The record that is the only word of an
  epoch is, when it matters most, in the *last* segment, where a wrong bit in front
  of it reads as a torn tail and is cut off. Today the epoch is in a file with a
  checksum that fails the start by name. So the change moves the one number whose
  loss is split brain from where damage is refused to where it is passed over, for
  14 ms.

**What would bring it back:** a trace after A, B and C in which the hello's 36 ms
and its tail are still a share of the absorbed region's players' wait worth having,
together with a way to tell damage from a torn tail in the last segment.

## 6. What is deliberately not in it

| What | Why not (the review's reason) | What would bring it back |
|---|---|---|
| The first draft's section 1: the state as a record of the log | Finding 9: the only change of format, 14 ms a checkpoint; finding 2: a damaged state is refused by name today and would be passed over | A measurement after this record in which the state's two waits matter, and section 7 of that draft argued first |
| Its sections 5 and 6: one round for a sync's temporaries; the chunk thread taking what waits together | Section 5 is overtaken: a chunk is one file and a sync is two rounds, the files and their directories (`chunks.rs:238-258`, `Round::put`, `:135-155`), one fewer than section 5 aimed at. Section 6, findings 1 and 10: ADR-0021 redesigns the same loop and is not built (`ChunkService::run` still does one job at a time, `chunks.rs:348-352`) | ADR-0021's thread built; then written against it |
| Its section 7: a merge's or split's record covering the region's own commits | Finding 12: the store cannot know that the runner saved every chunk it changed; not argued for a split with a return under way, nor for stay notes | The same measurement as for section 1; it would be argued before section 1 |
| "What follows a close comes behind the answers" | Finding 4, and 3.3 here: it moves the wait to the hello or the record | Writing the two files on a thread of their own, which is a design of its own |
| The hello by its record | Section 5 here | As said there |
| Anything about the survivor's checkpoints while it ticks | It is the runner's (finding 6), and by 1.2 it may be the larger half of the second | The log lines of 1.2; then a record for the runner |

*Note of 2026-10-15*: ADR-0021's thread has since been built (step W5, 2026-10-14;
see that record's "As built"): the store's thread for chunks no longer makes chunks
itself, and goes on while the generator's pool makes them. What the row on sections
5 and 6 says of `chunks.rs` was true when this was written; whatever brings section
6 back is to be written against the code as it is now.

## 7. The chunks that are not at hand at the survivor

The review's finding 7, for a merge as for a move.

**What the code does.** When the store answers `Absorbed`, the survivor keeps warm
the chunks it had loaded itself (`take_answer`, `services/worker/src/lib.rs:1699-1704`;
`begin_anew`, `:1857-1879`). The absorbed region's chunks were in a runner that has
ended; the survivor has none of them. Its links are closed, the edges say hello
again, and each chunk a hello names holds the link: every message of that link,
moves among them, is put aside until each of those chunks is answered
(`:3013-3020`, `drain`, `:2608-2611`). The tick that takes the hello asks the store
for every chunk that is not warm, one `StoreRequest::Load` each (`:2125-2134`). The
commit thread looks up the holder and passes each on (`lanes.rs:824-832`), and the
chunk thread does them one at a time: read the file and decode it, or generate the
chunk if it was never stored (`chunks.rs:356-375`, `:186-211`). The answers go into
the next tick's inputs (`lib.rs:1197-1201`). A warm chunk is handed to that same
next tick (`:2126-2131`). A move has no warm chunk at all (`restore`, `:889`).

**The arithmetic**, with the numbers I was given for this record and found in no
file: 0.38 ms a chunk in an unoptimised build, and about 1,300 chunks within one
tick in an optimised one, so about 0.04 ms a chunk.

- A merge: the two who walk see (2 × 8 + 1)² = 289 chunks each at a view distance
  of 8, on lanes four blocks apart, so about 300 in all, none of them the crowd's.
  300 × 0.04 ms = **12 ms**, optimised.
- A tick is 50 ms (`lib.rs:70`). The loads are asked at the end of the tick that
  took the hello and their answers are wanted by the next. 12 ms is within that: the
  chunks reach the same tick that warm ones would. **They cost the players no tick**,
  unless the chunk thread is in a checkpoint when they arrive, and then one.
- A move of 1,300 chunks: about one tick, 50 ms, of a pause of 0.3 s and more.

**So it is not worth a change now**: nothing to one tick, of a second. What the
measurement of 2026-10-10 saw (0.6 s after a move) was 1,300 × 0.38 ms in an
unoptimised build.

**What would bring it back**, and the smallest change then. Generated terrain
(ADR-0021): a chunk that was never stored is generated by the load, and that will
not take 0.04 ms. ADR-0021 decides who generates and when, so it is that record's
to say. For stored chunks, if a load is ever seen to take several ticks: the worker
that runs both regions hands the absorbed region's loaded chunks to the survivor
with the order to absorb, as warm chunks. They are, block for block, what the store
has once the release's flush is answered (ADR-0014, section 3.2). It is nothing for
the store and it does not help a move or a merge across two workers. Letting moves
through a held link is the edge's contract and is not proposed.

## 8. Steps

Each is one commit to `main` that leaves the four checks of `CLAUDE.md` green, run
with the tests' worlds in memory (`TMPDIR=/dev/shm/...`). Steps 1 to 3 are in
`services/worldstore/src/lanes.rs` and `lib.rs`, with the two places named for
step 2; they follow one another in one file, so one subagent at a time. All three
are delegable. The tests of section 9 are written from this record by someone else,
before or beside each step.

| Step | What | What holds afterwards | Who |
|---|---|---|---|
| 1 | Change A: A1 and A2 in `Lanes::load`; the error with the offset | A segment that is not the last is whole, on every disk the store has started on | a subagent |
| 2 | Change B: `close_at` through `start`; the condition in `end_group`; `CLOSE_AT`; the constructor that takes the value; the binary reading `CLUSTINE_LOG_CLOSE_AT`; the worker's one test opened with 0 | A checkpoint closes a segment only at `close_at` bytes; the table's and the players' files are written at a close; the suites at 0 are untouched | a subagent, with `services/worker/src/lib.rs` (one test) and `bin/clustine` named in its brief |
| 3 | Change C: no sync behind a merge's removal; no file behind a split; `admit` leaves `unwritten`; no removal of a file never written | A merge and a split are answered one wait behind their checkpoints | a subagent |
| 4 | Measure on the network disk, optimised: `crowds.rs` with `CLUSTINE_LOG_CLOSE_AT=0` and without, and the log lines of 1.2. Into the roadmap, saying which disk | Known: what B and C gave, how often a close falls into a standing checkpoint, and where the other half second is | the lead; needs the real disk |
| (5) | The hello of section 5 | | **not built by this record**; if ever, not delegated without tests written first: it is where split brain hides |

After step 1 and before step 2 nothing relies on anything new. After step 2, with
the variable set to 0, the server is the server of today, which is how step 4
compares.

## 9. Tests

### Those that exist and change

`syncs.rs` runs at the server's setting; its world never fills a segment, so no
close falls into it. Counts are waits before the answer.

| Test (`syncs.rs`) | Today | After step 2 | After step 3 |
|---|---|---|---|
| `a_commit_waits_for_one_sync_and_the_first_after_a_checkpoint_for_two` (`:427`) | first commit after a checkpoint: segment, `log/`: 2 | 1; the name loses its second half | |
| `a_checkpoint_that_lets_a_segment_go_writes_the_table_file_first_with_two_more_syncs` (`:461`) | 4 | 2 at the server's setting; kept as it is at a setting it passes, 4 | |
| `a_release_and_the_open_behind_it_wait_for_the_syncs_of_two_checkpoints_and_a_region_file` (`:542`) | last commit 2; open 2 and 2 behind | last commit 1; open 2 and 1 behind | |
| `a_merge_waits_for_three_syncs_of_its_own_behind_those_of_its_checkpoints` (`:573`) | last commit 2; record 3; the next checkpoint 4 | 1; 2; 2 | record **1**; "three" becomes "one" |
| `a_split_waits_for_four_syncs_of_its_own_behind_those_of_its_checkpoints` (`:632`) | last commit 2; record 4; the part's hello 0 and 1 behind | 1; 3; 0 and 1 | record **1**; the hello still 0 and 1; "four" becomes "one" |
| `a_take_over_waits_for_a_region_file_and_for_the_rounds_of_the_chunks_it_replays` (`:674`) | 5 waits in all | 5 | 5 |

The first, third, fourth and fifth are also kept in a second form with `close_at`
0 and today's counts, so that the setting the suites run with is counted too.

Changed by step 3, whatever the setting, with their new expectations written by
the test's author and not by the builder:

- `the_file_of_a_region_a_split_made_is_written_at_the_next_start`
  (`regions.rs:2213`): the split has no steps of a file to stop at; it stops at the
  record's append and sync, and the file is there after the next start.
- `the_region_file_of_a_part_that_could_not_be_written_is_written_later`
  (`scenarios.rs:3503`): its case of a file that cannot be written at the
  split goes; what it asserts of every later moment stays.
- `a_part_absorbed_by_the_pinned_region_it_was_split_off_is_among_what_that_was_granted`
  (`scenarios.rs:3371`): the part's file is not there after the part's hello
  (`:3378`), and not after the merge.

Unchanged and relied on: `a_log_cut_off_in_the_middle_of_a_record_is_recovered_up_to_there`
and `a_record_cut_off_at_the_end_of_the_log_loses_only_itself` (`tests.rs:568`,
`:714`), which tear the last segment; and the kill tests that start a store a
second time on what the first start left (ADR-0011, section 4.3, asks that of the
scenario in `kill_regions.rs`; I did not check each of the others for it), which
fail on A1 without A2.

### New, for step 1

1. `a_closed_segment_with_a_wrong_bit_fails_the_start_and_names_the_segment_and_the_offset`:
   two segments; for a bit in each record of the first in turn (a commit, a grant, a
   return, an `Opened`, a merge, a split), the start fails, the error has the path
   and an offset at or before the bit, and no file has changed.
2. `a_closed_segment_that_ends_in_half_a_record_fails_the_start`.
3. `a_start_cuts_the_torn_end_off_the_last_segment_and_makes_that_durable`: after a
   crash with `Survival::Torn`, a start, and a crash with `Survival::Nothing`, the
   segment ends with its last whole record.
4. `a_world_torn_by_a_crash_starts_again_and_again_with_commits_between`: torn,
   start, commit, start, commit, start; everything confirmed is restored each time.
5. **Kill**: `a_start_that_is_killed_at_any_point_of_cutting_the_log_is_done_again_to_the_same_end`:
   `Fault::Stop(n)` for every `n` of a start on a torn world, each of the four
   `Survival`s, then two starts.
6. `a_start_that_cannot_cut_the_log_fails_and_changes_nothing` (`Fault::Fails`).

### New, for step 2

7. `a_checkpoint_closes_a_segment_that_holds_the_setting_and_no_smaller_one`.
8. `the_table_file_and_the_players_file_are_written_at_a_close_and_not_at_a_checkpoint_between`,
   with `from` the segment after the closed one.
9. `segments_no_lane_needs_go_at_the_close_after_every_region_has_checkpointed`:
   what the first draft's test 7 meant, said so that it holds (the review's finding
   8).
10. `a_world_stopped_with_its_segment_open_starts_with_every_grant_and_every_note`,
    and started twice, with a session between: rule P's test at the server's
    setting, without the assertion that no segment is left.
11. `a_store_at_setting_nought_closes_at_every_checkpoint` (scenario 11's lists, as
    a guard on the setting itself).
12. **Kill**: the scenarios of `kill.rs` (`:411`, `:420`), `kill_regions.rs`
    (`:805` to `:828`), `kill_unpinned.rs` (`:435`) and `stays_kill.rs` (`:302`,
    `:311`, `:324`) run with `close_at` 0 and with a value that the scenario passes
    half way, at every point of each. The new points a crash can fall on are those
    where a checkpoint is durable and its segment still open, with the table file
    and the players' file older than the log.

### New, for step 3

13. `a_merge_is_answered_when_its_record_is_durable_and_a_start_removes_the_files_a_crash_brought_back`:
    killed at every point from the record's append to the next checkpoint's sync
    of `regions/`, every `Survival`; the absorbed region is refused and has no file
    once a store has started.
14. `a_split_writes_no_file_and_the_parts_epoch_is_held_to_at_every_point_until_the_table_file_is_written`:
    **kill** at every point from the record to the close that writes the file; a
    hello for the part with a lower epoch is refused by the running store and by
    every store started on what was left; after the close the file is durable.
15. `the_hello_for_a_part_whose_file_is_not_written_writes_none_and_waits_for_no_sync`.
16. `a_hello_with_a_higher_epoch_for_such_a_part_writes_the_file_before_it_is_answered`.
17. `a_part_absorbed_before_its_file_was_written_leaves_no_file_and_no_error`.

### If section 5 is ever built

A hello with a higher epoch is not answered while the log's sync is held; killed
right behind the answer, a lower epoch is refused by the next store and the file is
written by its start; a make-over killed at every point keeps every epoch; a region
file that fails once and a kill behind it; a wrong bit in front of the record in a
closed segment fails the start.

## What a player notices

- **At a split**: everybody in the region waits about 0.06 s less (0.38 → about
  0.32 s for the crowd). **At a merge**: about 0.04 s less for the survivor's
  players, about 0.06 s for the absorbed region's. Nobody would name either.
- **Now and then less of a hitch.** Today every checkpoint of any region can hold
  the commit thread for 36 to 72 ms more while the table's and the players' files
  are written, and every region's tick of that moment is shown that much later.
  That happens once in 4 MiB of log then.
- **The absorbed region's players still stand for about a second.** This record
  does not change that; 1.2 says where to look.
- **A world that crashed and is damaged in the log** does not start, and says which
  file and where, in the cases A1 covers. Today it starts and has quietly lost what
  was behind the damage.
- With a real client there is nothing new to try. What to measure is step 4.

## Risks

- **A wrong bit in the last segment is cut off for good** (A2), and with B the last
  segment is up to 4 MiB and in a quiet world hours old, where today it is one
  checkpoint's worth. What is lost then is every record behind the bit: commits
  that a state file covers do not matter, grants, returns, a merge or a split do.
  This machine has flipped single bits (`CLAUDE.md`). Not solved here. What would
  narrow it: a smaller `CLOSE_AT`; closing also at every so-manyth checkpoint, which
  bounds the age and gives back some of the waits; or the store keeping the cut
  bytes beside the segment for a person to look at.
- **A1 refuses a world that today starts.** That is its purpose, and it is also a
  server that does not come up at night. The message has to say what to do: the
  file, the offset, and that the records behind it are lost if the file is cut
  there by hand.
- **A commit longer than a record may be** (64 MiB, `log.rs:95-100`) is written and
  then read as cut off. Over TCP a request is at most 16 MiB
  (`docs/adr/0014-merging-and-splitting.md:48`);
  in one process nothing bounds it (**guess**: I found no check for commits;
  `may_absorb` and `split` have one, `lanes.rs:1624`, `:1673`). Today such a commit
  hides itself and what follows; with A1 it fails the next start once its segment
  is closed. Rare, and better loud, but it should be declined when it is asked.
- **The suites run at a setting the server does not** (3.1). The kill tests at two
  values and `syncs.rs` at the server's are what stands against it. A test written
  later against `start` gets 0 unless its author thinks of it.
- **A split's part has no region file for a while**, in ordinary running and not
  only after a failure. Whoever looks at `regions/` by hand sees a living region
  without a file.
- **`CLOSE_AT` too small** gives back today's waits; **too large** makes a start
  read more and the last risk above worse. It is a guess until step 4.
- **A disk that is not this one.** No step makes a sync more; on a local disk
  nothing is felt either way.

## Guesses, and what was not checked

- **Nothing was run.** Not `syncs.rs` either: the working tree has another
  session's changes in `crates/clustine-world`, `clustine-format` and
  `clustine-light`, which a build would have taken in. "Counted" is what the
  assertions of `syncs.rs` at `d1c11b3` say.
- **The half second of 1.2 is a guess.** I read the runner's code and the
  coordinator's numbers; I saw no log of the run.
- **The numbers of section 7** (0.38 ms, 1,300 chunks, one tick) were given to me
  and are in no file I found. That a merge's 300 chunks are loaded within a tick
  follows from them and from nothing else.
- **How often the table's and the players' files fall into a standing checkpoint
  today**, and so how much more than the certain 28 ms of a merge and 50 ms of a
  split B and C give.
- **`CLOSE_AT` = 4 MiB**, how fast a crowd fills it, and how long a start takes to
  read it.
- **The times in 1.3** are counts times unit costs. The unit costs are the
  measurement's of 2026-10-10, taken as 22, 14 and 40 ms.
- **Whether the last tick's commit is still on its way when the region stops.**
  The trace of 2026-10-10 had it mostly over by then; since then the runner stops
  between ticks. If it is over, a release and a standstill have one or two waits
  fewer than 1.1 says, today and afterwards alike.
- **That a lane exists whenever an `Opened` of its region is read** (section 5): I
  argued it for a split's part and did not prove it for every order of segments.
- `tcp.rs`, `table.rs`, `players.rs`, `local.rs` and `region.rs` beyond what
  `lanes.rs` calls: none of the three changes touches them, by my reading.
- How often a commit carries stay notes. ADR-0020, section 8, says nearly every
  segment has some; `docs/world-format.md:197-198` still says "No region sends a
  note yet", which is out of date and not this record's.
- The first draft's "Ruled out" list was not gone through again.

## Changes to other records, when this is one

- **ADR-0009** (moving a region): nothing. The release and the hello are as they
  are. (With section 5 built: step 1's "closes the region at the store" is
  unchanged, and the sentence on the hello gains "the epoch is durable in the log
  before the answer, and in the region file later".)
- **ADR-0010**: nothing. "What a player notices" there still holds.
- **ADR-0011**, section 3.5, in place of "after `Log::close` and `Lanes::collect`":
  "A group that made a state file durable closes the segment if it holds `close_at`
  bytes (ADR-00NN, section 3), and collects in either case. The table file is
  written only at such a close." Section 3.6, step 5: "`regions/B.state` is
  removed, and `regions/B.region` if its block of entity ids is empty and it was
  ever written. The removal is not synced; a file that a crash brings back is
  removed when the store starts." Section 3.7, step 5: "`regions/N.region` is not
  written here. It is written before the table file is next written, and when the
  store starts (section 4.2); the record has `N`'s epoch until then. A hello for
  `N` that changes neither its epoch nor its entity ids writes no file." Section
  4.2, step 2, gains: "In every segment but the last, the records end at the
  segment's last byte, or the start fails and names the segment and the offset. In
  the last, what follows the records is cut off and the cut made durable before
  anything is appended." Section 4.3: the rows "Merge" and "Split" lose "remove
  `B.state`, `B.region`" and "write, sync, rename `N.region`; sync `regions/`" from
  what is synced, and a row "Start, the log's end" is added with the table of
  section 2 here.
- **ADR-0014**: nothing in the runner changes. Section 3.1's note of 2026-10-11
  gains one sentence: "The store's part of the same review is ADR-00NN."
- **ADR-0018**, "What stays in turn", gains: "ADR-00NN took two of them out: the
  directory of a new segment behind a checkpoint, and the removal or the region
  file before the store answers a merge or a split. The state file's two stay."
- **ADR-0020**, section 8, rule P: "After a clean stop no segment may be left at
  all" becomes "After a clean stop the segment being appended to is left, unless
  the last checkpoint closed it (ADR-00NN, section 3); the start reads it like any
  other. With `close_at` 0 none is left, which is what the rule's test asserts."
  (The review's finding 5 asked for this record to be among those changed.)
- **`docs/world-format.md`**, "The write-ahead log": after "Nothing is appended to
  a segment after that" add "A store that starts cuts what is left of such a record
  off the last segment and syncs the cut before it appends anything. A segment that
  is not the last and does not end with a whole record is damaged, and the store
  does not start." After the paragraph on removing segments add "A segment is
  closed, and the next begun, by the first checkpoint that finds at least 4 MiB in
  it, or when it reaches 64 MiB." "Region files": "a region that was made by a
  split has none, and its file has 0 for both ids" gains "; the file is written
  with the table file that lets the split's record go, or at the next start, and
  until then the record has its epoch." "The table of regions" and "The players":
  "whenever a checkpoint leaves the first segment …" becomes "whenever a checkpoint
  closes a segment and leaves the first segment …". "Reading a damaged world" gains
  the sentence on a damaged segment.

## Which of the review's findings is answered where

| Finding | Where |
|---|---|
| 1, 10 (ADR-0021 and the chunk thread) | Section 6: not in it |
| 2 (damage passed over) | Section 2, with A2 added; what is left of it under "Risks" |
| 3 (the bound, and milliseconds per change) | Context and section 1: per kind of player, in milliseconds |
| 4 (a wait moved is not saved) | 3.3 (dropped), 4.1 (no sync at all), 4.2 (the hello's rule, and C behind B) |
| 5 (the suites and the setting) | 3.1, section 9; ADR-0020 among the records changed |
| 6 (the survivor's checkpoints) | 1.2: built, and by a guess what the second is now made of |
| 7 (chunks after a move) | Section 7 |
| 8 (when `collect` runs; a quiet world) | Section 3, points 2 and 3; 3.2 |
| 9, 12 (the state as a record; a record covering commits) | Section 6: not in it |
| 11 (the hello) | Section 5: the failure, the two details, and why not now |
