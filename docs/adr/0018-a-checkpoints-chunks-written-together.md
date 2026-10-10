# ADR-0018: A checkpoint's chunks are written together

- Status: **Built** (step C5.10), after an independent review against the code,
  whose ten findings are worked in (see "Review"); what building it settled and what
  it measured is at the end. The design of step C5.10 of milestone M3,
  phase C. It changes the world store's thread for chunks and its `Disk`; it changes
  no message and nothing of the format on disk. What it changes for whoever looks at
  the store's files is in section 3.
- Date: 2026-10-10

## Context

[ADR-0017](0017-the-end-of-the-stripes.md), section 9.7, set what a player bears when
a region is merged or split: half a second in the middle and a second at worst, for a
crowd of a hundred. It expected a longer stop to come from the edge finding its way
back to the region, and named keeping the links as the remedy (its step C5.10).

Step C5.8 measured it (`bin/clustine/tests/crowds.rs`, optimised, a view distance of
8, the bots of the crowd each placing and digging without a pause). With a hundred the
crowd waits 1.0 s in the middle, of which the region does not tick for 0.78 s and the
edge's way back is the rest. With two hundred, or a hundred on lanes four times as far
apart, the region does not tick for 1.4 to 3.3 s, and a merge takes longer than the
coordinator waits for it, so the two who left never come back into the crowd's region.

Where the time goes, by the worker's log of the hundred on wide lanes:

```text
01:02:43.46  bringing the store up to date for a merge or a split tick=2024
01:02:46.81  the store has the first checkpoint …; the region stops ticking tick=2091
01:02:49.59  handing the store a merge or a split tick=2092
```

The first checkpoint takes 3.35 s, during which the region ticks and every bot changes
its chunk again. The second, which the region stands still for, takes 2.78 s for about
a hundred chunks: 28 ms a chunk.

A checkpoint is, for the store, one `Job::Save` for every chunk that changed and one
`Job::Checkpoint` (`RegionRunner::checkpoint`). `FileChunks::save` makes each chunk
durable by itself: every section that is not stored yet is written under another name,
**synced**, renamed, and its directory **synced**; then the manifest is written under
another name, **synced** and renamed. `Job::Checkpoint` syncs the manifests'
directories and writes and syncs the state file. A chunk with one changed section is
three syncs, one after another, on one thread, and each waits for the disk.

What a sync costs on the machine that measured, while it was busy with tests: a hundred
files of 3 kB, written first and then synced,

| synced on | took |
|---|---|
| 1 thread | 1,436 ms |
| 8 threads | 546 ms |
| 16 threads | 145 ms |
| 32 threads | 82 ms |
| 100 threads | 29 ms |
| one `syncfs` of the file system | 1,354 ms (a build was writing to the same disk) |

Syncs that wait at the same time are made durable together by the file system; syncs
in turn each pay for the disk. So the store's order of work, not the amount of it, is
what stands a crowd still.

## Decision

**The store writes what a checkpoint saved in rounds: all section files, then all
manifests, each round's files written first and synced at the same time.** Nothing
else about a checkpoint changes.

### 1. `Chunks::save` only notes; `Chunks::sync` writes

`Chunks` keeps its three methods and what they promise: `save` "stores the chunk, so
that it is what is loaded from now on; it need not be durable before `sync`", and
`sync` "makes every chunk saved so far durable".

`FileChunks::save` describes the chunk (`ChunkManifest::describe`) and keeps the
manifest's bytes and the sections in `pending`, by position: a later save of the same
chunk takes the place of an earlier one. It touches no file. `Chunks::save` still
returns a `Result`, and `Job::Save` still loses the handle of a save that fails, for
whatever keeps chunks otherwise.

`FileChunks::load` answers from `pending` if the chunk is there (the chunk is kept
beside its manifest for that), and from the files otherwise.

`FileChunks::sync` takes all of `pending` and does, in this order:

1. **Sections.** Of all sections of all pending chunks, each hash once: those whose
   file is not there, or is in `unsure`. Their directories are made, each is written
   under its temporary name, all temporaries are synced together (section 2), each
   is renamed into place, and all their directories are synced together.
2. **Manifests.** Each pending manifest's directory is made, the manifest is written
   under its temporary name, all temporaries are synced together, each is renamed
   into place, and all their directories are synced together.

Only then is `pending` empty. What holds at every moment, as it does today:

- **A manifest in place names only sections that are durably in place.** No manifest
  is renamed before step 1 has ended well.
- **A chunk on disk is whole: as it was before, or as it was saved.** A manifest is
  put in place by a rename of a synced file, one chunk at a time. Between two chunks
  of one checkpoint nothing has to hold: the log has the commits of both until the
  state file of the checkpoint is in place, which is after `sync` has returned.
- **Nobody is told that something is durable before `sync` has ended well.**
  `Job::Checkpoint`, `Job::Return` and `Job::Fold` call it first, as today.

If anything in `sync` fails, it returns the error, and the caller loses every handle
that saved since the last sync, as today (`ChunkService::sync`); opening such a
region again applies its commits to the stored chunks again. Before it returns the
error, `sync`:

- removes the temporaries it wrote and did not put in place;
- **if step 1 failed**, removes every section file it put in place in this call, and
  remembers in `unsure` each one it could not remove, as `store_sections` does
  today: such a file may be there without being durably so, and must not be taken
  for stored. No manifest names them yet. **If step 2 failed, no section file is
  removed**: step 1 ended well, every one of them is durable, and manifests that
  were renamed before the failure name them;
- forgets the directories it was to sync, as today: after a failed sync, what it was
  to make durable may be lost although a later one succeeds, so whoever saved into
  them is told and saves again. A manifest that was renamed is whole and names
  durable sections, whether or not a crash keeps it;
- drops `pending`. The chunks in it are saved again by whoever opens their regions.

`MemoryChunks` is as it is.

**Why a round may stop anywhere.** Each file of a round is durable or not by itself,
and nothing of a round depends on another file of the same round. A section file is
named by the hash of what is in it, so one that is durable is right whoever wrote it,
and one that is not is not named by any manifest in place, because none is renamed
before every section of the call is durable with its directory. A manifest's
temporary that is not durable is not renamed. A manifest that was renamed and whose
directory was not synced is, after a crash, the old one or the new one. So every
subset of a round that a crash can leave is safe, not only the files before a given
one.

### 1a. When `pending` is written

The thread for chunks calls `sync`, and loses the handles that saved if it fails:

- for `Job::Checkpoint`, `Job::Return` and `Job::Fold`, as today;
- for **`Job::Flush`**, before it has the commit thread answer. A flush then means
  that every save before it is in the files, which is what callers take it to mean:
  a release, a merge and a split flush right after a checkpoint, when nothing is
  pending, and tests flush before they look at files;
- for **`Job::Restore`**, after the commits were applied and before the opened
  region is handed over; if it fails, whoever opens is answered with the error, as
  when a save failed there until now. So a world whose chunk files cannot be written
  is not opened, instead of being opened and lost at its first checkpoint; and what
  an open applied is not held in memory;
- after a **`Job::Save`** that leaves 128 chunks or more pending. A region saves a
  chunk also when the last ticket for it goes (`RegionRunner::release`), and nothing
  syncs after that until the next checkpoint, five minutes later by default, or the
  next return. Without a limit `pending` would grow with what an interval changed.

`Job::Barrier` makes nothing durable, as today; nothing that passes it looks at
files.

### 2. `Disk` gets two methods

```rust
/// Makes what was written to each of the files durable, as `sync` does for one.
fn sync_files(&self, files: &[PathBuf]) -> io::Result<()>;
/// Makes the files created, renamed or removed in each of the directories durably
/// so, as `sync_directory` does for one.
fn sync_directories(&self, directories: &[PathBuf]) -> io::Result<()>;
```

Both have a default that calls the single method for each path in turn and stops at
the first error.

`OsDisk` does them on threads: `std::thread::scope`, one thread for each path up to
64, which take the paths from a shared counter; with one path it calls the single
method. The threads are started with `Builder::spawn_scoped`, and the calling thread
takes paths itself, so that a system that refuses a thread makes the round slower
and does not panic the thread for chunks, which would leave every job after it
undone and the log never cut. Every thread goes on to the end; the first error by
the order of the paths is returned. Nothing is kept between calls: a checkpoint is
rare and a thread costs far less than a sync.

`MemoryDisk` keeps the default. So in the simulated disk every sync of a round is a
step of its own, in the order of the paths, and the tests that stop the store at
every change or sync walk every point of the new order. That alone shows only what
is left when the files **before** a point are synced; a real crash leaves any subset
of a round. Why any subset is safe is argued in section 1, and test 3 below stops a
round with a chosen subset of it synced.

### 3. What it changes for whoever looks at the files, and what it does not

- **A save is in the files at the next checkpoint, return, flush or open, or when
  128 are pending, and not before.** Nothing but `FileChunks::load` reads chunk
  files in the server, and it reads `pending` first. Tests that save a chunk and
  then read, damage or count files flush first; those outside the store
  (`services/worker/tests`, `bin/clustine/tests/persistence.rs`) do so already or
  look after an open. Tests inside the store that look at files after a bare save
  get a flush, and each says why.
- **A load of a pending chunk does not read its file**, so a file that is damaged
  between a save and its sync is not noticed until the chunk is loaded after the
  sync.
- **A failure to write a chunk shows at the sync**, and loses every handle that
  saved since the last one, instead of the one whose save failed. That is what a
  failed sync of a manifest's directory does today.
- Not changed: the format on disk and `docs/world-format.md`; the log, the commit
  thread, the state files and what a checkpoint means; `apply`, which loads, changes
  and saves chunks and finds its own saves through `pending`; temporaries left by a
  crash, which are never read and stay until a save of the same chunk or section
  replaces them (a crash can now leave two for each pending chunk).

## What it gives

A checkpoint of *n* changed chunks is four rounds of at most *n* syncs each at the
same time, in place of about 3 *n* syncs in turn. By the table above a hundred chunks
are then four rounds of about 30 to 80 ms on that disk, in place of 2.8 s. The first
checkpoint, which the region ticks through, gets shorter by the same factor, so fewer
chunks have changed again when the second begins. Opening a region after a crash
gains the same: it no longer pays three syncs for every chunk its commits touch.

**What stays in turn** while a region stands still, five to nine syncs by the
review's count: the state file's temporary, the directory of the region files when
the state is put in place, the record of the merge or the split and, where it opens
a new segment of the log, that segment's directory, and the removal or the region
file before the store answers. At 3 to 13 ms each on that disk they are 20 to 120 ms
beside the rounds. They are left as they are until the measurement says they matter;
the state file's temporary could join the manifests' round without touching any
invariant.

**Measured after it is built**, with `crowds.rs` as in step C5.8: the crowd of a
hundred, of two hundred, and of a hundred on wide lanes. What is asked of it: the
bound of ADR-0017 for a hundred, and merges that are made for the other two. If the
bound is still missed, the roadmap says by how much and where the rest goes; keeping
the links at a merge and a split stays the remedy for the edge's part, which is a
quarter of a second for a hundred.

## Tests

Written from this record by someone who does not write the change, in
`services/worldstore`:

1. A chunk that was saved and not synced is what `load` gives; after `sync` too; and a
   second save of it before the sync is what counts. No file of it is there before
   the sync, and after a flush, an open or the 128th pending save it is.
2. With the simulated disk stopped at every change and sync of a `sync` of several
   chunks that share a section and have sections of their own, saved by two regions
   (as `kill.rs` stops the store): what a crash leaves, in each of the ways
   `MemoryDisk` can leave it, has every chunk either as before or as saved, and no
   manifest that names a section that is not there. **Every chunk is loaded right
   after the crash, before anything is saved again.**
3. The same with a round stopped when a chosen subset of its files is synced, not
   only those before a point: a disk of the test's own around `MemoryDisk` that, in
   `sync_files` and `sync_directories`, syncs the paths a seed picks and then stops.
4. With every one of the steps of test 2 made to fail in turn: `sync` returns the
   error; **every chunk is loaded right after it and is as before or as saved**; the
   handles of both regions that saved are lost and a handle that did not save is not;
   a later save of the same chunks and a sync that works leave them as saved; and no
   section file is taken for stored that a crash could take away (the case `unsure`
   is for).
5. A store whose chunk files cannot be written does not open a region whose commits
   have to be applied.
6. `OsDisk::sync_files` and `sync_directories` on a temporary directory: two hundred
   files are synced without an error; a path that is not there gives an error and
   the call still returns for the others.
7. An ignored test that saves and syncs two hundred chunks on the real disk and
   prints how long it took, for the roadmap.

The store's existing tests keep what they assert. Those that count the simulated
disk's changes and syncs to place a fault (`kill.rs`, `kill_regions.rs`,
`kill_unpinned.rs`, `scenarios.rs`) walk every point of the new order by how they are
written; where one names a point by its number, the number is found again and the
test says what the point is.

## Ruled out

- **One `syncfs` for a round.** It makes everything on the file system durable, also
  what other programs wrote: 1.35 s in the measurement above, while a build was
  writing. It is Linux only and needs a dependency.
- **One file for a checkpoint's sections**, or a log of chunks. Faster still, and a
  new format with its own compaction. Not for a pause of a second.
- **Not syncing sections**, and trusting the log. The log is cut back at the
  checkpoint, which is the point of it.
- **Keeping the links at a merge and a split** (what ADR-0017 named). It is a change
  to the contract with the edge and addresses a quarter of the second.
- **Other distances or a longer rest.** As ADR-0017 says: they change how often a
  crowd stands still, not for how long.

## Risks

- **A file system that does not make concurrent syncs durable together.** Then
  nothing is gained and nothing lost. The measurement is from ext4 on a virtual disk.
- **More memory**: at most 128 chunks are held, each as it was saved, until the next
  sync.
- **A failure is found later**, at the sync and not at the save, and then loses every
  handle that saved since the last sync instead of the one whose save failed. That is
  the safe side, and what a failed directory sync does today. Between checkpoints
  most regions have saved something, so one failed write can lose all of them at
  once; each is opened again from what is durable.

## Not checked

- How long a sync takes on the disks Clustine will be run on. On a disk that syncs in
  a tenth of a millisecond none of this is felt either way.
- Whether the commit thread's sync of the log waits behind a round. Both are syncs
  that wait together, so it should not; `crowds.rs` shows it in what a bot waits.
- Three things the review found that are older than this record and that it leaves
  as they are: `unsure` does not outlive the process, so a section file that a
  killed store had written and not synced is taken for stored by the next one (a
  round has more such files at a time than a single save had); the parent of a newly
  made directory is never synced, which the simulated disk cannot see; and whether
  an error of writing a file that is closed before it is synced is always reported
  by the sync.

## Built, and measured

Built as decided, with these things settled on the way:

- `Chunks` says how many chunks wait through a fourth method, `pending`, which is
  none unless a store holds saves back.
- `FileChunks` no longer keeps the directories of manifests that are still to be
  synced: a manifest is put in place only inside `sync`, which syncs its directory in
  the same call or forgets it.
- The 64 threads of a round count the calling thread.
- A flush whose sync fails loses the handles that saved and still answers a handle
  that saved nothing.
- A checkpoint of more than 128 chunks is written in several sets of rounds: one for
  every 128 saves and one for the rest.
- `Job::Fold` syncs without losing handles, as before; it runs only while a world is
  made over, when none is open.

The tests of the record, written without sight of the change, are in
`services/worldstore/src/rounds.rs`: seventeen, of which eleven could not pass before
the change and each failed where the record says the store was to change. All pass.
They found nothing in the change and nothing in the store as it was.

Two hundred chunks are saved and made durable in 0.7 s on the disk that took 28 ms a
chunk (half of it describing the chunks, half the rounds). `crowds.rs`, as in step
C5.8 and on the same machine, least / middle / worst:

| Crowd | Split: the crowd waited, before | after | Merge: the crowd waited, before | after |
|---|---|---|---|---|
| 100 | 0.40 / 1.03 / 1.07 s | 0.31 / 0.36 / 0.41 s | 0.40 / 0.97 / 0.97 s | 0.35 / 0.36 / 0.36 s |
| 200 | one split, 1.44 s without a tick | 0.24 / 0.26 / 0.35 s | no merge was made | 0.26 / 0.26 / 0.55 s |
| 100 on lanes four times as far apart | one split, 0.73 s without a tick | 0.26 / 0.30 / 0.35 s | no merge was made | 0.26 / 0.26 / 0.35 s |

The bound of ADR-0017 is met for a hundred and for two hundred, and every merge is
made. The syncs that stay in turn were left as they are.

## Review

An independent reviewer went over the first version against the code and found ten
things; all are worked in above.

1. The failure path removed section files that manifests already in place name: a
   failure in the manifests' round would have left chunks that cannot be read, and
   `apply` passes over such chunks, so the next checkpoint would have cut their
   commits off the log. Sections are removed only when their own round failed, and
   the tests load every chunk right after a failed sync.
2. "Nothing outside the store changes" and "the existing tests keep what they
   assert" were false: tests in four crates look at chunk files after a save.
   Section 3 says what changes; a flush and an open write what is pending.
3. `pending` was not held "for a moment": a region saves a chunk whenever its last
   ticket goes, and nothing synced after that for up to an interval. The limit of
   128.
4. The estimate left out the syncs that stay in turn ("What stays in turn").
5. A thread that cannot be started would have panicked the thread for chunks.
6. The simulated disk leaves only the files before a point synced; the argument for
   any subset, and test 3.
7. `Job::Save` keeps its failure, and an open keeps failing on files that cannot be
   written.
8. Directories of a failed sync were kept for a later one, against what the store
   does today for a stated reason.
9. Tests the list missed: several chunks of two regions in one sync, who is lost by
   a failed sync and who is not.
10. The three older doubts in "Not checked".
