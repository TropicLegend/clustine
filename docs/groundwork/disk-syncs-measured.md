# The world store's syncs, counted and timed

Written 2026-10-10 against `main` at `12e445f`, as groundwork for a decision record
("Few syncs in turn", drafted beside this file as `few-syncs-in-turn-draft.md`). It says
which synced writes the world store makes for each thing it is asked, in which order, on
which thread, which of them wait one for another, and where the time of a merge goes on
a disk that takes 20 ms for a sync. Nothing of the store was changed for it.

## How to read this

- **Counted** means the tests in `services/worldstore/src/syncs.rs` observed it: they
  run the store on a disk that holds every sync back until the test lets it through,
  and they fail if the store waits for the disk otherwise than this file says.
- **Traced** means it was seen in one run of the real store on the real disk, with every
  sync written down with its time, its thread and how long it took (section 3 says how).
- **Read** means I read it in the code at the place named and did not observe it.
- A **wait** is one of: a sync of a file, a sync of a directory, or a round of syncs
  that the store starts at the same time (`Disk::sync_files`, `sync_directories`).
  Waits **in turn** are those where the next begins only when the one before has ended.
- The **commit thread** is `worldstore` (`Lanes::run`), the **chunk thread** is
  `worldstore-chunks` (`ChunkService::run`).

## The short version

1. **A sync costs 20 ms on this disk and a directory's sync 14 ms, and two syncs that
   wait together cost as much as two in turn.** A round is worth it from four files
   on; a round of two or three costs 40 ms. One append to one file costs 20 ms however
   much is appended. Two threads that each sync in turn slow each other to half. So
   what a player waits for is the number of waits on the way, and a wait is only
   saved by not making it, or by writing into a file that is synced anyway. (Section 2.)
2. **A merge makes 39 waits in turn between the command and its answer** in the
   trace, a split 14 to 20, a move 12 to 18. Eleven of a merge's are made while the survivor's
   players stand still, and about thirty while the absorbed region's do. (Sections 4
   and 5.)
3. **Of a merge that took 1,088 ms by the command, 853 ms were the store's own waits,
   103 ms were requests waiting behind another region's sync on one of the store's two
   threads, 117 ms were a runner waiting for its next tick to look at an answer or an
   order, and 15 ms were messages.** (Section 5.)
4. **The survivor of a merge checkpoints four times where two would do**, and the
   second and third of them, 292 ms from the one being asked to the survivor's stop,
   are made while the absorbed region's players already stand still. (Section 5.)
5. Things that are on the way and need not be: the name of a new segment of the log
   three times in a merge, because every checkpoint closes the segment; the table file,
   written before a checkpoint is answered; the new region's file of a split and the
   removal of an absorbed region's files, both before the answer although the store
   already copes with their never being done. (Section 4.)

## 1. What was run

The disk is the one the default temporary directory is on: ext4 on a logical volume,
which the owner says is network storage on NVMe. The machine has twelve processors.
Builds were unoptimised, as the owner's measurement was.

| What | Command | Gives |
|---|---|---|
| Counts | `cargo test -p clustine-worldstore --locked syncs:: -- --nocapture --test-threads 1` | every wait of each operation, in order, with its thread and whether it comes before the answer |
| Unit costs | `python3 docs/groundwork/disk-syncs-bench.py` | what a sync costs alone and together (section 2) |
| A trace | the test `players_stand_still_only_briefly_when_regions_are_merged_split_and_moved` of `bin/clustine/tests/merges.rs`, with the store's `OsDisk` writing down every sync (section 3) | five merges, five moves and five splits under four bots, each sync with its time |

The counting tests passed sixty times in a row, and the store's whole test suite, `cargo
fmt --all --check` and `cargo clippy -p clustine-worldstore --all-targets --locked -- -D
warnings` pass with them.

## 2. What a sync costs here

`docs/groundwork/disk-syncs-bench.py`, second of two runs (the first, while the machine
was busier, was slower by a quarter and had the same shape). Thirty of each in turn, ten
of each round; the middle one, the least and the most, in milliseconds.

| What is synced | middle | least | most |
|---|---|---|---|
| a new file of 3 kB, `fsync` (a temporary) | 20.1 | 19.8 | 20.8 |
| an append of 3 kB, `fdatasync` (a commit) | 20.2 | 19.7 | 38.9 |
| 64 appends of 3 kB to one file, one `fdatasync` | 23.2 | 23.0 | 29.5 |
| a rename, then `fsync` of the directory | 13.6 | 13.4 | 17.4 |
| a removal, then `fsync` of the directory | 13.8 | 13.4 | 14.3 |
| write, `fsync`, rename, `fsync` of the directory (`replace` and its directory) | 34.3 | 33.9 | 34.9 |
| **2 new files, `fsync` at the same time** | **40.0** | 39.3 | 44.0 |
| 3 new files at the same time | 39.7 | 39.3 | 40.7 |
| 4 | 56.9 | 39.8 | 78.9 |
| 8 | 76.8 | 68.5 | 88.2 |
| 16 | 61.8 | 40.4 | 74.3 |
| 32 | 54.5 | 52.9 | 54.9 |
| 64 | 83.9 | 82.1 | 87.1 |
| 1, 2, 4 directories with a rename each, at the same time | 13.7, 13.8, 14.1 | 13.5 | 18.0 |
| 8, 16 such directories | 22.4, 25.5 | 14.1 | 31.2 |
| **an append's `fdatasync` and a new file's `fsync` at the same time** | **40.0** | 25.8 | 41.0 |
| an append's `fdatasync` and a directory's `fsync` at the same time | 33.4 | 19.6 | 40.3 |
| appends synced in turn on one thread **beside** new files synced in turn on another: each append | 39.1 | 25.7 | 43.8 |
| the same: each new file | 39.0 | 33.4 | 45.3 |

What follows from it:

- **A sync is one trip to the disk of about 20 ms, and the disk makes one trip at a
  time.** Syncs that are asked while a trip is under way are served together by the
  next one. So two at the same time take two trips (40 ms), as two in turn do; sixty-
  four take four (84 ms) where in turn they take 1,300 ms. This is what
  [ADR-0018](../adr/0018-a-checkpoints-chunks-written-together.md) measured for a
  hundred files, and it does not hold for two: "syncs that wait at the same time are
  made durable together" is true of a crowd of them and false of a pair. **Inferred**:
  the reading of the numbers as trips is mine; the numbers are measured.
- **Directories do wait together**, up to four of them in one trip: a directory's sync
  writes the file system's journal and no data of a file.
- **A round of files costs two trips at least**, whatever is in it: 40 ms for two or
  three files, 40 to 85 ms for four to sixty-four.
- **The two threads of the store halve each other.** While the commit thread syncs the
  log for every tick of every region (766 syncs in the 45 s of the trace: it was in a
  sync for half of the time), every sync of the chunk thread takes two trips. In the
  trace a state file's temporary took 20.3 ms at the least and 38.9 ms in the middle;
  a round of one file 20.6 and 27.9; a round of two or three 27.4 and 46.0; a round of
  four to eight 26.3 and 84.6.
- **What does cost one trip for any amount is one file**: sixty-four records appended
  and synced once take 23 ms.
- A first version of the script synced a directory right behind the `fsync` of a new
  file in it, and that took no time at all: on this file system a new file's sync
  seems to carry its name. Nothing here relies on it, and the store's simulated disk
  rightly does not. The store's own new segments did pay for both (the table below).

The store's own syncs in the trace, for comparison (the middle one of each kind):

| Sync | how many | least | middle | most |
|---|---|---|---|---|
| the log's segment (commit thread) | 766 | 19.8 | 28.5 | 120.7 |
| `log/`, after a segment was begun | 53 | 13.3 | 13.9 | 50.4 |
| `regions/` | 88 | 13.4 | 14.2 | 94.2 |
| a state file's temporary (chunk thread) | 54 | 20.3 | 38.9 | 104.1 |
| a region file's temporary | 18 | 20.4 | 21.1 | 665.1 |
| the table file's temporary | 11 | 20.9 | 34.6 | 40.3 |

Fifty-three segments were begun in 45 s, one for every checkpoint: each cost whoever
committed next a second wait.

## 3. How the trace was made

Not committed, because it is lines in the store that nothing else needs. To do it again:

1. In `services/worldstore/src/disk.rs`, have `OsDisk::sync`, `sync_directory`,
   `sync_files` and `sync_directories` append a line to the file that an environment
   variable names: the time in microseconds, the name of the thread, the kind, how
   long it took, the path. Renames and removals a line each, without a duration.
2. In `Lanes::handle` (`lanes.rs:575`), a line for every message but a load, and one
   where a flush, a hello, a merge and a split are answered (`lanes.rs:1037`, `1296`,
   `1575`, `1721`).
3. In `bin/clustine/tests/common/processes.rs`, `Cluster::new`, have the logs go to a
   directory that outlives the test.
4. Run `cargo test -p clustine --test merges --locked -- players_stand_still_only_briefly
   --nocapture`. The children inherit the variable. Put the store's lines and the
   workers', the coordinator's and the edge's log lines in one list by their times.

The run that this file quotes: seed 189452, five rounds. Its merges took 1,104 to
1,655 ms by the command, its moves 433 to 1,001 ms, its splits 471 to 1,194 ms. That is
more than the owner measured the same day (950 to 1,100, 300 to 390, about 320).
**Inferred**: the machine was shared with other work that evening, and a sync took
20 to 40 ms from one minute to the next (compare the two splits in section 6); the
trace itself writes a line to a file without syncing it. The counts do not depend on it.

## 4. The counts

Each row is **counted** unless it says otherwise. "C" is the commit thread, "K" the
chunk thread. A round is written as `{…}`. What stands before `→ answer` is in turn
and is what whoever asked waits for; what stands after it is made after the answer,
and keeps the commit thread from whatever is asked next.

| Asked of the store | Waits, in order | In turn before the answer | At 22 ms each |
|---|---|---|---|
| **A commit** of a tick (`Lanes::request`, `lanes.rs:728`; `end_group`, `:961`) | C the log's segment → `Committed` | 1 | 22 ms |
| The **first commit after a checkpoint**: the checkpoint closed the segment (`:999`), so this one begins a segment and syncs its name (`Log::sync`, `:2063`) | C the segment, C `log/` → `Committed` | 2 | 44 ms |
| **A claim** (`:849`) | none of its own: the record is in the group of the tick's commit. C the segment → `Claimed` | 1, shared | 22 ms |
| **A return** of chunks nothing was saved in (`:913`, `returned`, `:1196`) | C the segment; no answer | 1 until the chunks are free | 22 ms |
| A return behind a save | K {section files}, K {their directories}, K {manifests}, K {their directories}, C the segment | 5 until free | 110 ms |
| **A checkpoint** with no changed chunk, and the flush behind it (`chunks.rs:421`, `lanes.rs:1241`, `:971`) | K the state file's temporary, C `regions/` → `Flushed` | 2 | 44 ms |
| A checkpoint of *n* changed chunks (`FileChunks::sync`, `chunks.rs:235`) | K {section files}, K {their directories}, K {*n* manifests}, K {their directories}, K the state's temporary, C `regions/` → `Flushed` | 6 | 132 ms |
| A checkpoint that finds the first segment kept for the table alone (`trim_for_the_table`, `lanes.rs:1162`): after any claim, return, merge or split | as above, then C the table file's temporary, C `regions/` → `Flushed` | 4 or 8 | 88 or 176 ms |
| **A release** (a move's first half; `RegionRunner::begin_release`, `services/worker/src/lib.rs:1227`): first checkpoint, the region ticking | as a checkpoint of *n* chunks | 6 | the region ticks |
| … the last tick's commit, the region standing | C the segment, C `log/` → `Committed` | 2 | 44 ms |
| … the second checkpoint | as a checkpoint: 2, or 6 if a chunk changed since the first, and 2 more with the table | 2 to 8 | 44 to 176 ms |
| … the handle is dropped | none | 0 | |
| **The open by the next owner** (`Lanes::admit`, `lanes.rs:1388`, `:1450`) | C the region file's temporary, C `regions/` → answer; then C the segment, C `log/` for the record that it was opened | 2, and 2 after | 44 ms, and 44 ms of the commit thread |
| **A merge**, the survivor: the last tick's commit | C the segment, C `log/` | 2 | 44 ms |
| … its second checkpoint | as a checkpoint | 2 to 8 | 44 to 176 ms |
| … **the record** (`Lanes::absorb`, `lanes.rs:1507`; `write_alone`, `:1768`; the removal, `:1559`) | C the segment, C `log/`, C `regions/` → `Absorbed` | 3 | 66 ms |
| … what follows: the survivor's next commit | C the segment | 1 | |
| … its next checkpoint, which writes the table file because of the record | K the state's temporary, C `regions/`, C the table's temporary, C `regions/` | 4 | |
| **A split**: the last tick's commit and the second checkpoint | as for a merge | 4 to 10 | |
| … **the record** (`Lanes::split`, `:1634`; the part's file, `:1717`) | C the segment, C `log/`, C the new region's file's temporary, C `regions/` → `Split` | 4 | 88 ms |
| … the hello for the part | → answer; then C the segment | 0, and 1 after | |
| **A take-over** of a region with commits no checkpoint covers (`Job::Restore`, `chunks.rs:490`) | C the region file's temporary, C `regions/`; then K the four rounds → answer, and C the segment beside them | 6 | 132 ms |

Where a count is "2 to 8": under bots that place and dig without a pause a chunk has
always changed, so it is 6, and 8 when a claim, a return, a merge or a split has been
written since the table file was (in a world whose regions follow players, as a rule).

**Read, not counted**: `trim_for_the_players` (`lanes.rs:1129`) writes the players' file
in the same place and way as the table file, two more waits before the answer; no tick
writes a stay note yet (`services/worker/src/lib.rs:1963`), so it does not happen.

### In turn while a region stands still

| | Waits in turn | At 22 ms | Traced |
|---|---|---|---|
| **A merge, the survivor's players**: last commit 2, second checkpoint 6, record 3 | 11 (13 with the table) | 242 ms | 160 to 205 ms by the worker's own count; the last commit's two waits were over before the runner saw that it was to stop (section 5) |
| **A merge, the absorbed region's players**: its last commit 2, its second checkpoint 6, the hello 2, then all the survivor makes before it stops (5 + 2 + 2 + 2 + 1 in the trace) and the survivor's 11 | 33 | 726 ms | 824 ms from its stop to the record's answer |
| **A split**: last commit 2, second checkpoint 6, record 4 | 12 (14) | 264 ms | 183 to 527 ms |
| **A move**: last commit 2, second checkpoint 6 (8), the hello 2 | 10 (12) | 220 ms | 220 ms from the stop to the hello's answer; the players then wait 0.6 s more, which is not the store's syncs (below) |
| **A take-over**, after the lease: region file 2, four rounds | 6 | 132 ms | not traced |

After a move the new owner has none of its players' chunks: in the trace the hello of
the edge was answered 57 ms after the store had opened the region, and the first
commit came 600 ms later (thirteen ticks without one). The store made no sync in that
time. **Inferred**: the runner loads what its players see, in an unoptimised build.
It is most of what the bots waited for at a move (1.1 to 1.4 s) and is not looked
into here.

## 5. Where a merge's time goes

The last merge of the trace: region 0 absorbs region 5, both run by `worker-0`, one bot
in region 0 and three in region 5. 1,088 ms from the coordinator's note that it began to
its answer. Times in milliseconds from the first.

| From | To | ms | What | Waits in turn |
|---|---|---|---|---|
| 0 | 10 | 10 | The order reaches the worker; the runner of region 5 takes it at its next tick and asks for a checkpoint and a flush | |
| 10 | 215 | 205 | **Region 5's first checkpoint**, three chunks, the region ticking: K {3 section files} 51, K {3 directories} 21, K {3 manifests} 51, K {2 directories} 34, K the state's temporary 32, C `regions/` 14. Each longer than its own cost, because the commit thread synced the log beside it seven times | 6 |
| 215 | 258 | 43 | The runner sees the answer **at its next tick** and stops | |
| 258 | 281 | 23 | **Region 5 stands still.** Its last tick's commit had begun a segment at 215: C the segment 39, C `log/` 21, ended at 281 | 2, mostly before the stop |
| 281 | 306 | 25 | The runner asks for the second checkpoint at once; the request **waits for the commit thread**, which is syncing two commits of region 0 | (1 of another region) |
| 306 | 355 | 48 | The checkpoint **waits for the chunk thread**, which is writing the checkpoint the survivor was asked to make beforehand (`Prepare`) | (2 of another region) |
| 355 | 538 | 183 | **Region 5's second checkpoint**, one chunk: K {1} 27, K {1} 23, K {1} 52, K {1} 21, K the state's temporary 39, C `regions/` 14; 7 ms between two rounds are writing | 6 |
| 538 | 541 | 3 | `Released`; the coordinator orders the merge; the worker says hello for region 5 | |
| 541 | 572 | 30 | The hello **waits for the commit thread**: a commit of region 0 has begun a segment, C the segment 20, C `log/` 13 | (2 of another region) |
| 572 | 606 | 34 | **The hello**: C the region file's temporary 20, C `regions/` 14 | 2 |
| 606 | 632 | 26 | The worker reads the state and hands the runner the merge; the runner takes it **at its next tick** (17 ms), and its requests reach the store 9 ms later | |
| 632 | 886 | 253 | **The survivor's first checkpoint for the merge, the survivor ticking, region 5's players standing.** It is the third: the first was asked with the order to merge (at 36), **a second with the order to absorb** (at 606; `bin/clustine/src/cluster/worker.rs:811`), and this one a tick later (`services/worker/src/lib.rs:1345`). It waits on the chunk thread behind the second, K four rounds and the state's temporary, until 771; K its own state's temporary 39; C `regions/` 20; **C the table file's temporary 32, C `regions/` 14**; C a new segment 21, C `log/` 14; C `regions/` 14 | 12 |
| 886 | 924 | 38 | The runner sees the answer **at its next tick** and stops. Meanwhile its last tick's commit: C a new segment 21, C `log/` 14, ended at 920 | 2, within the wait for the tick |
| 924 | 1,084 | 160 | **The survivor stands still.** Second checkpoint, one chunk: K {1} 21, K {1} 14, K {1} 21, K {1} 14, K the state's temporary 21, C `regions/` 14 (106 in all, on a quiet disk). The record: C the segment 21, C `log/` 14, C `regions/` 14 (49). 5 ms are messages | 9 |
| 1,084 | 1,088 | 5 | `AbsorbEnded`; the coordinator reads the list and answers | |

By what it is:

| | ms | Share |
|---|---|---|
| Waits the merge made itself: 39 in turn, of which 2 fell into a wait for a tick; the other 37 are rows 2, 4, 7, 10, 12 and 14 less its messages | 853 | 78 % |
| A request waiting behind another region's sync on the commit thread (25, 30) or behind another checkpoint on the chunk thread (48) | 103 | 9 % |
| A runner waiting for its next tick to see an answer (43, 38) or an order (10, 26) | 117 | 11 % |
| Messages between the processes | 15 | 1 % |

The 37 waits took 23 ms each on average: 20 where the disk was quiet, 30 to 50 where
the commit thread synced beside them. With the world in memory the merge takes 187 ms
(the owner's number): the ticks waited for and the messages are 132 ms of that here.

So the 900 ms that this merge lost on this disk are:

| ms | What | Waits |
|---|---|---|
| 494 | the three checkpoints the merge needs as it is built: the absorbed region's two and the survivor's last, each four rounds, a state file and `regions/` | 18 |
| 253 | the survivor's two further checkpoints behind the order to absorb, with the table file and a new segment that came with them | 12 |
| 106 | the absorbed region's last commit in a new segment (23), the hello's region file (34), the record with its segment's name and the removal behind it (49) | 7 |
| 117 | runners waiting for a tick; the survivor's last commit and its new segment (2 waits) fell into one of those | 2 |
| 103 | requests waiting behind other regions on the store's two threads | |
| 15 | messages | |

## 6. A move and a split, traced

**The last move** (region 0 from `worker-0` to `worker-1`, 505 ms by the command):

| ms | What |
|---|---|
| 22 | the runner takes the order at its next tick |
| 278 | the first checkpoint, seven chunks, the region ticking: rounds of 36, 27, 53 and 21, the state's temporary 95 (the log was synced beside it for 90), a commit and `regions/` 41 |
| 17 | the runner sees the answer at its next tick and stops |
| 20 | the last commit: a new segment and `log/`, begun 17 ms before the stop |
| 160 | the second checkpoint, two chunks: rounds of 21, 14, 40, 14, the state's temporary 21, `regions/` 14, **the table file's temporary 21, `regions/` 14** |
| 4 | `Released`, the region assigned, the hello sent |
| 35 | the hello: the region file's temporary 21, `regions/` 14 |
| then | the region runs 3 ms later; the edge's hello is answered at the next tick, 53 ms later; the first commit comes 600 ms after that |

**The last split** (region 6 off region 0, 940 ms by the command, at a moment when a
sync took 33 to 40 ms and a directory's 25 ms):

| ms | What |
|---|---|
| 14 | the order, the next tick, the requests |
| 452 | the first checkpoint, seven chunks, the region ticking: rounds of 97, 40, 100 and 35, the state's temporary 91, a commit 35, `regions/` 25 |
| 43 | the runner sees the answer at its next tick and stops |
| 27 | the last two commits: a new segment 40 and `log/` 25, begun 43 ms before the stop |
| 273 | the second checkpoint, four chunks: rounds of 85, 24, 72 and 28, the state's temporary 36, `regions/` 25 |
| 124 | **the record**: the segment 33, `log/` 25, the new region's file's temporary 39, `regions/` 25 |
| 4 | the worker takes the answer; the region stood still for 430 ms by its own count |

The other splits of the run stood still for 183, 304, 505 and 527 ms.

## 7. What waits and is not a sync

Seen in the trace, at the places named:

- **A runner looks at the answer to the flush behind its first checkpoint once a
  tick** (`RegionRunner::step`, `services/worker/src/lib.rs:1017`; the loop between two
  ticks, `:2282`, takes answers and publishes and does not look at the phase): 43, 38,
  17 and 43 ms above. The region ticks meanwhile, so its own players do not wait; the
  command does, and in a merge the absorbed region's players do while the survivor
  finds out.
- **A runner takes an order once a tick** (`:2264` to `:2269`): 8 to 22 ms.
- **The runner waits for its commits to be confirmed before it asks for the second
  checkpoint** (`carry_on`, `:1380`): the request then waits for whatever the commit
  thread is syncing, 25 ms above. The store would hold the request behind those
  commits by itself (`lanes.rs:948`).
- **A request waits for the sync the commit thread is in**, which is a sync for
  another region as often as not: 25 and 30 ms above; half a sync on average, two
  when a segment is being begun.
- **A checkpoint waits for the checkpoint before it on the chunk thread**, which
  does one job at a time (`ChunkService::run`, `chunks.rs:366`): 48 ms and 139 ms
  above. The order to prepare the survivor is sent with the order to release the
  absorbed region, so the two meet there every time.
- **The survivor checkpoints four times**: with the order to merge, with the order to
  absorb, when the runner takes the merge, and when it has stopped. The second and
  third are a tick apart.
- **The edge's hello is answered at the next tick** after a merge, a split or a move:
  40 to 53 ms above. It is the edge's part and not looked into here.

## 8. What I did not measure

- A take-over end to end: the count is from the tests, the time is the count times
  the unit costs.
- More than four bots. A crowd's checkpoint has rounds of a hundred files and more,
  which section 2 covers up to sixty-four and ADR-0018 measured at two hundred.
- An optimised build. The counts are the same; the shares of section 5 are not.
- Another disk. On one that syncs in a tenth of a millisecond none of this is felt.
