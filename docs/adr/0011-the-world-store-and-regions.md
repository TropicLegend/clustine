# ADR-0011: The world store and regions

- Status: **Accepted**; the design of step C1 of milestone M3, phase C. Revised after an
  independent review against the code (see the end). Not built yet.
- Date: 2026-10-08

## Context

[ADR-0010](0010-regions-that-follow-players.md) makes a region a set of chunks that the
world store grants, and says what the store must guarantee: a list of regions whose ids
are never used again, a table of grants on disk, chunks that leave a region only saved,
replay only into what a region holds, pinned regions, and a merge and a split that are
one log record each. It does not say how. This record does, for the store as it is in
`services/worldstore` and `crates/clustine-format` after step C0.

What the store is today, as far as this record builds on it:

- One **commit thread** (`Lanes::run` in `lanes.rs`) takes every hello and every request
  from one queue, in **groups**. `Lanes::request` appends each `Commit` to the log;
  `Lanes::end_group` syncs the log once, puts state files in place, and only then answers
  `Committed` and passes on the saves, checkpoints and flushes that waited
  (`Owner::held`). A hello ends the group first (`Lanes::open`).
- One **thread for chunks** (`ChunkService::run` in `chunks.rs`) does `Job::Load`,
  `Job::Save`, `Job::Checkpoint`, `Job::Flush`, `Job::Restore` and `Job::Fold` in the
  order it is given them. A save is durable only at the next `ChunkService::sync`, which
  a checkpoint does before it writes the state under a temporary name and sends
  `Message::Checkpointed` back; the commit thread renames it (`Lanes::install`), unless
  its tick is not above the state file's.
- The **log** (`log/<n>.wal`) has records of kind 1 (`Changes`, only read), 2 (`Commit`)
  and 3 (`Opened`). A segment is read up to its first record that is cut off or fails
  its checksum, and a store that starts never appends to a segment it found. A record
  of an unknown kind is an error (`FormatError::Corrupt("record kind")`). A payload
  longer than `MAX_PAYLOAD_LENGTH` (64 MiB) is written without complaint and read as a
  record that was cut off.
- Per region there are `regions/<r>.region` (`RegionFile`: highest epoch, entity ids)
  and `regions/<r>.state` (`StateFile`: tick, state). `Lane::live` lists where the
  region's commits above its state file's tick are in the log; `Lanes::collect` removes
  the segments at the start of the log that no lane's `live` names.
- `Lanes::admit` refuses a lower epoch, writes the region file, replaces the owner,
  reads the state file and the live commits, appends `Opened { region, epoch, restored }`
  and has `Job::Restore` apply **all** block changes of those commits to the stored
  chunks before the hello is answered, by the thread for chunks, also when there are
  none.
- After a failed append or sync, `Lanes::fail_log` cuts the segment back to what was
  durable (`Log::fail`), appends nothing more to it, and loses the owner of every region
  that wrote in the group; a region that did not keeps its handle, and its flush is
  answered. The truncation is not synced, so whether the bytes are gone after a crash
  is not known; for commits the `Opened` record of the next opening makes that harmless.
- The store never sees a `Layout`, only its fingerprint, in `RegionHello::layout`. The
  first hello decides it (`Lanes::decide_layout`); another fingerprint than the one in
  the file `layout` makes the store put every region's commits into the chunks and drop
  the states (`Lanes::fold`). The store does not know which chunks a region covers, and
  lets every owner load and save any chunk.
- `Store::local` runs `local::prepare` before `start`: it checks `meta` and carries the
  logs of very old worlds over, through `std::fs`. `Store::memory` and the tests that
  call `start` on a `MemoryDisk` do not run it.
- Over TCP (`tcp.rs`) a connection is about one region: `RegionHello`, then
  `StoreWelcome`, then the `Restored` in `RestoredPart`s, then requests and replies. A
  message is at most `wire::MAX_MESSAGE_LENGTH` (16 MiB); what a handle in the store's
  own process asks goes through a channel and has no limit. A hello that fails for
  another reason than its epoch is answered `StoreWelcome::Refused`, which
  `StoreHandle::connect` returns as `StoreError::Refused`. The worker process tries
  again only after `StoreError::Io` (`open_region` in `bin/clustine/src/cluster.rs`),
  drops the region on `EpochRefused`, and ends on any other error.

## Decision

### 1. What is on disk

```text
<world>/
  meta                 as before
  log/<n>.wal          the log; four new kinds of record
  regions/table        the list of regions and the table of grants, as of a place in the log
  regions/<r>.region   as before
  regions/<r>.state    as before
  layout               only in a world from before this record; removed once the table is there
```

**The log is what decides.** A grant, a return, a merge and a split have happened when
their record is durable in the log, and not before. `regions/table` is to the list of
regions and the grants what a state file is to a region: a whole state as of a place in
the log, written so that segments can be removed. Region files and state files follow
the log, as they do today.

`FORMAT_VERSION` stays 1 and `meta` is unchanged: a store of this record reads every
world a store of today wrote. A store of today that is given a world of this record
fails on the first new record it reads, or, if there is none left in the log, takes the
missing `layout` file for a changed layout and folds the world. Going back to an older
build is not supported.

#### 1.1 New log records (`clustine-format`, `log.rs`)

Framed as the others; every payload begins with the version (u8) and the kind (u8).
`chunks` is a count (u32) followed, per chunk, by i32 x and i32 z; `bytes` is a length
(u32) followed by that many bytes. All integers big-endian.

| Kind | Record | Fields |
|---|---|---|
| 4 | `Granted` | region u32, tick u64, chunks |
| 5 | `Returned` | region u32, chunks |
| 6 | `Absorbed` | region u32, epoch u64, absorbed u32, tick u64, state bytes |
| 7 | `Split` | region u32, epoch u64, tick u64, state bytes, part u32, part epoch u64, chunks, part state bytes |

- `Granted`: `region` holds each of `chunks` from its tick `tick` on.
- `Returned`: `region` no longer holds `chunks`.
- `Absorbed`: the merge. `state` is the whole state of `region` as of `tick`; every
  chunk `absorbed` was granted is granted to `region` with tick `tick`; every area
  `absorbed` was pinned to is one `region` is pinned to; `absorbed` is retired into
  `region`. `epoch` is that of the owner that asked, as in a `Commit`.
- `Split`: the split. `part` is a new region; `part state` is its whole state as of
  `tick`; it holds `chunks` with tick `tick`, which `region` no longer holds; `part
  epoch` is the highest epoch `part` has been opened with; `state` is the whole state
  of `region` as of `tick`.

**Sizes.** A record longer than `MAX_PAYLOAD_LENGTH` would hide itself and what follows
it in its segment, and a handle in the store's process can ask for one. So the store
writes a `Granted` or a `Returned` with at most 65 536 chunks, and as many of them as a
claim or a return takes, one behind the other in the same group; and it declines a merge
or a split whose record would be longer than the limit (section 7). A commit that long
is as it is today (open question 9).

#### 1.2 The table file (`clustine-format`, next to `RegionFile` and `StateFile`)

Written whole with `disk::replace` and made durable by syncing `regions/`, like a region
file. Kind 3 of the files sealed with a CRC-32 (`seal`, `opened` in `region.rs`).

| Field | Type |
|---|---|
| Format version, kind (3) | u8, u8 |
| `from`: the first log segment whose records change this table | u64 |
| The next region id | u32 |
| The home chunk | i32 x, i32 z |
| The home region | u32 |
| The division the store was started with: count, then areas | u32, areas |
| Regions, in ascending order of their ids: count, then per region | u32 |
| … id | u32 |
| … the areas it is pinned to: count, areas | u32, areas |
| … its grants, ascending by x then z: count, then per grant x, z, tick | u32, (i32, i32, u64)… |
| Absorbed regions, oldest first: count, then per pair absorbed, into | u32, (u32, u32)… |
| CRC-32 of everything before | u32 |

An area is a u8 of flags (bit 0: it has a western end, bit 1: an eastern end) followed by
i32 `min_x` and i32 `max_x`, each 0 if absent: a `ChunkArea`.

The table lists the **living** regions only. A region that is in neither list has gone
for good: its id is below the next region id and is never used again.

#### 1.3 What the store keeps in memory

Beside each `Lane`, on the commit thread only:

- the table: per living region the areas it is pinned to and its grants (chunk → tick),
  an index chunk → region over all grants, the home chunk and region, the next region
  id, the absorbed pairs, and `from`;
- per lane `latest`: the highest tick of the region the store has in a commit or a
  whole state. It is set to the tick the region is restored up to when it is opened,
  and raised by every commit taken and by a merge or split of the region. It is the
  tick of grants (section 3.2);
- per lane `named`: the highest tick the owner's session has named, in a commit or in a
  checkpoint, whether that checkpoint is in place yet or not. Set to the tick restored
  up to at an opening, raised by a merge or split of the region. A merge or a split has
  to name a tick above it (sections 3.6, 3.7);
- per lane `returning`: for each chunk a return of which is on its way through the
  thread for chunks, the **number** of that return; and how many returns the session
  has asked for, from which the numbers come (section 3.3);
- per lane, in place of `state_tick` alone, **where its latest whole state is**: in its
  state file (tick), or in an `Absorbed` or `Split` record of the log (tick, and where
  the record is, as an `Entry`) if that record's tick is above the file's.

**Who holds a chunk**: the region it is granted to; else the region pinned to an area
that contains it; else nobody. Areas of different regions never overlap.

### 2. Stripes until C5: the division, pinned regions, home

**The store is told how the world is divided when it is started**, not by the first
hello. `Store::memory_divided(generator, division)` and `Store::local_divided(root,
generator, division)` take a

```rust
pub struct Division {
    /// The chunk players enter the world in.
    pub home: ChunkPos,
    /// The areas of the pinned regions, which must not overlap. Region `i` is pinned
    /// to area `i`.
    pub pinned: Vec<ChunkArea>,
    /// The fingerprint a hello has to name, if the division is that of a layout.
    pub layout: Option<u64>,
}
```

`Division::stripes(home, &layout)` is the division of a layout: its stripes as the
pinned areas, in their order, and its fingerprint. `Store::memory(generator)` and
`Store::local(root, generator)` stay and mean the stripes of `Layout::single()`, one
region pinned to the whole world, with the home chunk at the origin. The single process
passes the stripes of its `Config::boundaries` and the chunk of `spawn_point()`;
`clustine worldstore` gets `--boundaries`, as the coordinator has it. A division with
other areas, or with none, is for the store's tests until C5, which need chunks that
nobody holds; from C5 on it is what `clustine worldstore --pin` gives, `layout` goes,
and nothing else in this record changes.

Two divisions are the same if their home chunks and their areas are; that is what the
table keeps of the division it was made from. The home chunk is part of it: a store
started with another home chunk makes the world over (section 4.2), although the areas
are the same. The home chunk is fixed in the program today (`spawn_point()`).

Ruled out: carrying the layout in the hello (it would be built now and thrown away at
C5, when the store has to know home and the pinned regions before anyone says hello, for
the coordinator to learn them from); and having the coordinator tell the store (the
coordinator keeps nothing on disk to tell it again from, and the single process and the
store's own tests have none).

**A stripe is a pinned region.** When the table is made from a division, region `i` is
pinned to the area of stripe `i` (`Layout::regions`), so the ids are those every service
works out from the layout today. The **home region** is the pinned region whose area
contains the home chunk; if there is none, it is a region made after the pinned ones
and granted the home chunk with tick 0. The next region id is the number of
regions made, or the one the table had before if that is higher.

- **A pinned region holds every chunk of its areas that is not granted** to any region,
  without a grant of its own and so without a record. Such a chunk counts as held from
  tick 0.
- **What a pinned region knows of that.** In C1 and C2a nothing is split off, so a
  stripe holds its whole area, as its worker assumes today, and loads, saves and
  commits without ever claiming. **From C2b on a pinned region claims the chunks of its
  areas like any other region before it treats them as its own.** The store answers
  such a claim from the table, `granted` or `foreign`, without writing a record (section
  3.2). The store does not tell a pinned region at its opening which chunks of its
  areas are another region's; it learns each when it claims it.
- **`Restored::held`** is the region's grants, in ascending order of the chunks, each
  with its tick. For a pinned region that is only what it was granted beyond its areas
  or took over in a merge. `Restored` gains `pinned: Vec<ChunkArea>`, the areas the
  region is pinned to.
- **`RegionHello { region, epoch, layout }`** stays as it is. `layout` is compared with
  `Division::layout`, if that is not `None`; another one is refused with
  `StoreError::LayoutMismatch`, as today. A region the table does not have is refused
  (section 7); today any number can be opened.
- **A changed division** is found when the store starts (section 4.2) and handled as a
  changed layout is today: every region's commits go into the chunks, their states are
  dropped, and the table is made anew from the division. Regions that were split off
  are dropped with the rest. The ids of the stripes are used again by the stripes of the
  new layout, with the epochs their region files have, as today; that is the one
  exception to "never used again", and it ends with the stripes.
- **Entity ids.** A region that is pinned or home is issued a block when it is first
  opened (`Lanes::allocate`), kept in its region file, as today. A region made by a
  split has the empty block (`first` and `end` both 0) for good; its state has what it
  needs. So that no block is issued twice, the store notes when it starts the highest
  block any region file has, of regions living or not, and issues above it; and the
  region file of a region that had a block is never removed. A pinned or home region
  whose file has the empty block (its id was a split-off region's before the division
  changed) is issued one at its next opening.
- **Merging two pinned regions** (section 3.6): the survivor is pinned to the areas of
  both afterwards. A region can therefore be pinned to several areas, also to areas that
  do not touch, and a region that was not pinned becomes so by absorbing one that was.
  When the store starts again with the same division it finds the table as it left it:
  what is compared is the division the table was made from, not what the regions are
  pinned to now.
- **Splitting a part off a pinned region** (section 3.7): the part's chunks are granted
  to the new region, which is not pinned. They are granted "otherwise" from then on, so
  the pinned region does not hold them. When such a chunk is returned, it is the pinned
  region's again, from tick 0.

### 3. The operations

Everything below is done by the commit thread, for the session that asks, and only if
that session is the region's owner (`Lanes::request` checks it today and goes on doing
so). "Held" in this section means section 1.3's rule.

#### 3.1 Loads and saves: by the holder only

`Load { position }` and `Save { position, .. }` are looked at when the commit thread
takes them. If the region holds the chunk, they are done as today. If not, nothing is
done and the session is answered `StoreReply::NotHeld { position, holder }`, with the
region that holds the chunk if one does. The handle stays as it is.

The check is made on the commit thread because that is where the table is; what the
thread for chunks is given has passed it, and is done in the order it was passed.

The table it is made against can have a grant or a return of the open group in it,
which is not durable yet. If that group fails, every owner is lost (section 4.1), so
nobody goes on from what such a load answered; and a save that was passed on at once
came from a region with nothing unsynced, so it holds no change that is not durable.

A commit is not looked into: block changes to chunks the region does not hold are
logged with the rest, and never applied to a stored chunk (section 3.4).

#### 3.2 `Claim { chunks }`

For each chunk named, each once and in the order of the request:

- the region holds it, by a grant or by being pinned: it is among `granted`, and
  nothing about it changes, also not its tick. If a return of it is on its way, the
  return is called off: the chunk leaves `returning`;
- another region holds it: it is among `foreign`, with that region;
- nobody holds it: it is granted to the region with the tick `latest`, and is among
  `granted`.

If any chunk was granted anew, `Granted { region, tick: latest, chunks }` with those
chunks is appended to the log, in the group, behind the commits the region asked for
before. The table is changed at once, so that a later claim in the same group, of any
region, is answered by it. The answer `Claimed { granted, foreign }` is given when the
group ends, after the sync, in order with the region's `Committed` answers
(`Owner::unsynced` becomes a list of answers). Saves, checkpoints, flushes and returns
asked for behind a claim wait for the group, as they do behind a commit.

**The tick of a grant is the store's, not the worker's.** ADR-0010 has the claim carry
the region's tick. A region runs ahead of what it has committed, and a tick that sent no
commit "may be issued again after a restore" (ADR-0008, section 4). A grant noted at tick
13 of a region that is then restored up to tick 10 would keep the block changes of its
new ticks 11 to 13 out of the chunk when the region is opened the next time. `latest` is
never above what a later opening restores the region up to, because the commit it comes
from is in the log before the grant, and it is not below the tick of any change the
region made to the chunk while it held it before. `Claim` loses its `tick`.

#### 3.3 `Return { chunks }`

1. When the commit thread takes it: chunks the region has no grant for (it does not
   hold them, or holds them by being pinned), and the home chunk, are left out, with a
   warning. The return gets the session's next number `n`; each chunk left is noted in
   `returning` with `n`, in place of an earlier number if it had one; and a `Job::Return
   { number: n, chunks, peer }` is passed on like a save: behind the group if the
   region has anything unsynced or held.
2. The thread for chunks makes the saves so far durable (`ChunkService::sync`, as for a
   checkpoint; a failure loses the handle) and sends `Message::Returned { session,
   number, chunks }`. A lost handle's return is dropped.
3. The commit thread drops it unless the session is still the one that opened the region
   last (`Lane::current`). **A chunk is freed only if `returning` has it with this very
   number.** Those chunks leave `returning`; if there are any, `Returned { region,
   chunks }` is appended to the log in the current group and the grants are taken out
   of the table at once. The chunk is then nobody's, or the pinned region's whose area
   it is in.

The number is what keeps a return that was called off from freeing the chunk later. A
region returns `c`, claims it again, changes it, saves it and returns it once more; the
thread for chunks still has the first return before that save. Its sync does not cover
the save, and without the number its message would free `c` because the second return
has put `c` in `returning` again: a confirmed change would be in no stored chunk and,
the region no longer holding `c`, never replayed.

A chunk also leaves `returning` when a split takes it to another region (section 3.7),
and a lane's `returning` goes with the lane when the region is absorbed.

Not answered. A `Flush` asked for behind a return is answered only when the return is
durable: its `Message::Flushed` follows the `Message::Returned` and is answered at the
end of the group that wrote the record, and not at all if that group fails.

The store cannot see that the saves hold every change; that is the region's promise, as
it is for a checkpoint. What the store sees to is the order: the saves before the return
are durable before the record is written, and the chunk is the region's, for every other
region's claim, until it is.

#### 3.4 Opening a region

`Lanes::admit`, with these changes:

- A region the table does not have is refused (section 7), before anything is written.
- The state is the latest whole state: the state file, or the `Absorbed` or `Split`
  record if its tick is higher (for the part of a split, the record's `part state`). The
  deltas are those of the live commits above its tick.
- **Replay.** Of the block changes of the live commits, a change is applied only if the
  region holds the chunk it is in now and the commit's tick is above the tick the chunk
  is held from (0 for a chunk held by being pinned). The others are left out of what
  `Job::Restore` is given. The same holds for `Lanes::fold`.
- **If no change is left to apply, the commit thread answers the hello itself**, and
  no `Job::Restore` is made. The part of a split is opened by a worker that already
  runs it and can publish nothing of it before it has the handle; behind `Job::Restore`
  it would wait for whatever checkpoint of another region the thread for chunks is busy
  with. Nothing is lost by it: what the owner before had the thread for chunks do is
  still done before anything the new owner asks, as both go through that thread's one
  queue. If whoever said hello has gone, the region is given up as `Job::Restore` does
  it today.
- `Restored::held` and `Restored::pinned` are filled as in section 2; `entity_ids` as
  there.
- `latest` and `named` are the tick restored up to; `returning` is emptied and the
  session's returns are counted from 0.

Over TCP (`tcp.rs`): `StoreWelcome::Accepted` gains `pinned`. `held` follows the deltas
as one more item, `RestoredItem::Held`, whose bytes are the postcard of the
`Vec<(ChunkPos, u64)>` and whose tick is 0, cut into pieces like a state (`Parts`); it
is left out if `held` is empty. `Arriving::add` takes a second `Held`, or a state or a
delta behind one, for parts that do not fit together.

#### 3.5 Checkpoints, and removing segments

A checkpoint is done as today, with one rule widened: `Lanes::install` puts a state
file in place only if its tick is above that of the lane's **latest whole state**, be
that the file, as today, or a record. So a checkpoint that was under way when a merge
or a split was written cannot take the place of the merged state. When a state file is
in place and durable (`Lanes::end_group`), it is the lane's latest whole state, and the
live commits up to its tick are let go as today.

The store notes the last segment that has a record for the table (`Granted`,
`Returned`, `Absorbed`, `Split`), when it appends one and when it reads the log at a
start. `Lanes::collect` keeps a segment if a lane's live commit is in it, if a lane's
latest whole state is a record in it, or if its number is from `from` up to that last
segment: those hold what the table file does not have.

**The table file is written** in `Lanes::end_group` when a state file was put in place
and, after `Log::close` and `Lanes::collect`, the first segment of the log is one that
no lane needs, kept for the table alone. The file is written with `from` = the number
of the next segment (`Log::next`) and made durable; only then `from` is raised in
memory, and `collect` runs again. If writing fails, `from` stays and nothing more is
removed. It is not written while the region file of a split's part is not durable
(section 3.7).

A region is thus restored from the latest whole state the log or its state file has.
ADR-0010 has state files "brought in line afterwards"; here the survivor's and the
part's state files are written by their next checkpoints, like any other, and until
then the record's segment stays. Nothing is copied from the log into a state file.

#### 3.6 `AbsorbCommit { absorbed, absorbed_epoch, tick, state }`

From the session of the survivor `A`, about `B = absorbed`.

1. The group is ended (`Lanes::end_group`), so that everything either region asked for
   before is durable and answered.
2. The store declines (section 7), changing nothing, unless all of this holds:
   - `B` is a living region, is not `A`, and is not the home region;
   - `B` has an owner now, and that owner opened it with `absorbed_epoch`;
   - neither `A` nor `B` has a live commit: each one's every commit is covered by its
     checkpoint;
   - `tick` is above `A`'s `named`: above every tick `A`'s session has named in a
     commit or a checkpoint, and above the tick it was restored up to;
   - the record is not longer than `MAX_PAYLOAD_LENGTH`.
3. It appends `Absorbed { region: A, epoch, absorbed: B, tick, state }` and syncs the
   log, by itself. If that fails: `Lanes::fail_log` (section 4.1), nothing is answered.
4. Only now memory is changed: `B`'s grants are `A`'s with tick `tick`, also those a
   return of which was under way; `B`'s areas are `A`'s; `B` leaves the regions and
   joins the absorbed pairs (of which the store keeps the latest 4096); `B`'s owner is
   lost and its lane goes, `returning` with it; `A`'s latest whole state is the record,
   and its `latest` and `named` are `tick`.
5. `regions/B.state` is removed, and `regions/B.region` if its block of entity ids is
   empty. A failure is logged; the store does it again when it starts.
6. `A` is answered `Absorbed { absorbed: B, chunks }`, with the chunks that became its
   own by step 4, in ascending order.

ADR-0010 asks that "the same worker's session on `B` is `B`'s current one". The store
does not know workers, only sessions; `absorbed_epoch` is what shows that whoever asks
is the one that was told to absorb `B`: without it, a worker whose time for the merge
has run out could absorb a region that another worker has been given since.

ADR-0010 asks only for `B`'s log to be empty. `A`'s has to be as well, which its step 4
brings about: the areas and chunks that come to `A` count as held from `tick` or from
0, and that is only right if no commit of `A` from before is left to be replayed.

`tick` is measured against `named` and not against the stored state alone, because a
checkpoint that the session asked for and that is still with the thread for chunks
would otherwise be put in place afterwards with a tick at or above the merge's, and the
state from before the merge would stand for the region while the table says `B` is
absorbed. With `tick` above `named`, section 3.5's rule keeps every such checkpoint
out.

#### 3.7 `SplitCommit { tick, state, part: { chunks, state }, as_epoch }`

From the session of `A`.

1. The group is ended.
2. The store declines unless: `A` has no live commit; `tick` is above `A`'s `named`;
   `chunks` is not empty; `A` holds every one of them; the home chunk is not among
   them; `as_epoch` is at least 1; the record is not longer than `MAX_PAYLOAD_LENGTH`.
3. `N` is the next region id. It appends `Split { region: A, epoch, tick, state, part:
   N, part epoch: as_epoch, chunks, part state }`, with each chunk once, and syncs the
   log, by itself. If that fails: `fail_log`, nothing is answered.
4. Memory: the next region id is `N + 1`; `N` is a living region, not pinned, granted
   `chunks` with tick `tick`; `A`'s grants among them go, **and each of them leaves
   `A`'s `returning`**; both regions' latest whole state is the record; `A`'s `latest`
   and `named` are `tick`, and `N`'s `latest`; `N`'s highest epoch is `as_epoch`, with
   the empty block of entity ids.
5. `regions/N.region` is written and made durable. If that fails it is tried again
   before the table file is next written, and when the store starts (section 4.2).
6. `A` is answered `Split { region: N }`.

A chunk whose return is under way can be in the part: it is `A`'s until the return is
through. The split takes it out of `returning`, so the `Message::Returned` that comes
later finds no number for it and frees nothing; the chunk is `N`'s.

**`N` is not opened by the split.** ADR-0010 has `N` "opened by this session with
`as_epoch`" and the store answer "with `N` and the handle". A handle is a channel in
this process and a connection in another, and neither fits in a reply. `N` is made with
`as_epoch` as the highest epoch it was opened with; the worker says an ordinary hello
for `N` with `as_epoch`, which the store takes as it takes the same owner coming back,
and meanwhile runs `N` from what it has in memory. Nobody with a lower epoch can open
`N`, which is what the session was for. The worker gets `N`'s state back in the
`Restored`, and need not look at it. The hello has nothing to replay, so the commit
thread answers it without the thread for chunks (section 3.4): `N` can publish as soon
as a hello and its answer have crossed, which is what ADR-0010 meant the worker to be
spared by running `N` at once.

The part's state is as of the same `tick` as `A`'s: the new region's ticks go on from
`A`'s.

#### 3.8 Fencing

- Every request is done only for the session that owns the region (as today); what
  comes back from the thread for chunks (`Checkpointed`, `Returned`, `Flushed`) only
  for the session that opened the region last and is not lost.
- A merge needs `B`'s epoch (3.6). A split gives `N` its epoch in the record.
- An absorbed region has no owner and is never opened again.
- An opening with a higher epoch fences whoever ran the region, wherever that owner is
  in a claim, a return, a merge or a split: what it asked for before the hello is done
  first (the hello ends the group), what it asks afterwards is not.

### 4. Recovery

#### 4.1 A failed write or sync of the log

Today the bytes `Log::fail` cuts off may come back after a crash, because the
truncation is not made durable, and only the regions that wrote in the group lose their
owners. For a commit the first is settled by the next `Opened`. A grant or a merge that
comes back would be a grant the store has since given to another region, or a merge of
a region that has since gone on by itself. And a group now holds what a region that
wrote nothing in it relies on: a return has no answer, so its region would never learn
that it was undone, and a load can have been answered by a grant that is taken back.

So, when an append or a sync of the log fails (`Lanes::fail_log`):

1. The segment is cut back to what was durable and nothing more is appended to it, as
   today.
2. What the group did to the table in memory is undone, in reverse order: grants given
   are taken back, returns are given back. Merges and splits change memory only after
   their own sync, so there is nothing of theirs to undo.
3. **Every region loses its owner**, whether it wrote in the group or not. Nothing of
   the group is answered: no commit, no claim, no flush. What the thread for chunks
   sends later for a lost session (`Checkpointed`, `Returned`, `Flushed`) is dropped,
   as a `Checkpointed` of a lost session is today.
4. **The truncation is made durable**: the segment is synced. Until truncating and
   syncing have both succeeded the log is **unsettled**. It is tried at once, and
   again before every hello and every request for the list of regions, which is all
   that can arrive: there are no owners. A segment that is not there (its first append
   failed) needs none.
5. **While the log is unsettled the store serves nobody.** A hello gets
   `StoreError::Io`, with the error of the truncation or the sync; `Store::regions`
   the same. Over TCP the connection is closed **without a welcome** (section 7), so
   that `StoreHandle::connect` returns `StoreError::Io` and a worker tries again, as it
   does when the store cannot be reached.

Workers open their regions again by themselves, as they do today after losing a handle,
and are restored from what is durable.

What this gives: nothing is answered, and no file is changed, on the strength of a table
that a restart would not rebuild. Once the truncation is durable, what was cut off is
no part of the file, whatever a crash leaves of the disk blocks it was in. If the store
dies before that, the records may count; they were never answered, nothing was answered
after them, they are a beginning of their group in the order it was written, and each
is whole.

ADR-0008 says that a failed sync is not retried, as "what it was meant to make durable
may be gone even if a later sync succeeds". That stands: nothing is done to make the
group durable after all. The sync here is of the file's new length, for the opposite
purpose. On the local file system the length having shrunk is metadata, which
`File::sync_data` need not write; `OsDisk::sync` uses `File::sync_all` for a file that
was truncated since it was last synced.

Ruled out: a record `Cut { segment, length }` at the start of the next segment, which
the first version of this record had. It needs a kind of record and a second pass over
the log at every start, and buys only that the store goes on when one file cannot be
synced and a new one can. A store whose log cannot be truncated and synced stays shut
until it can, or until it is started again, which reads what is there as after any
crash.

**The simulated disk** (`MemoryDisk` in `disk.rs`) cannot tell a store that makes the
truncation durable from one that does not: `truncate` shortens what every kind of
crash leaves. It is extended:

- `Content` gains `untruncated`: what the file had before the first truncation since
  it was last synced. `truncate` notes it when it shortens the file and nothing is
  noted yet; `sync` of the file forgets it.
- `Survival::Untruncated`: as `Survival::Everything`, except that a file with something
  noted in `untruncated` has that. It is what a machine finds whose truncation never
  reached the disk.
- `Fault::Fails(n, k)`: the `k` changes or syncs from the `n`th on fail, each as
  `Fault::Fail` makes one fail. `Fail(n)` is `Fails(n, 1)`. It is what makes the
  truncation, or its sync, fail after the sync before it has.

#### 4.2 When the store starts

`Store::local` runs `local::prepare` first, as today; that is outside `start`, goes
through `std::fs`, and is not part of what the kill tests of this record cover. Then,
in `start` in `lib.rs`, in this order and before any hello is taken (the thread for
chunks runs from step 3 on):

1. `Lanes::load`, as today: temporary files in `regions/` are removed, region files and
   state files are read. It notes the highest block of entity ids any region file has.
2. `regions/table` is read if it is there; one that cannot be decoded is
   `StoreError::Damaged`. Then the log, record by record in the order of its segments:
   - what a record means for a **lane** counts wherever the record is: a `Commit` above
     the state file's tick is live; an `Opened` lets go of live commits, and of a whole
     state in a record, above its `restored`; an `Absorbed` or `Split` with a tick above
     the state file's is the region's latest whole state (a `Split` is that for both
     its regions), and lets go of live commits up to its tick; a `Split` makes the
     part's highest epoch at least `part epoch`;
   - what a record means for the **table** counts only in segments at or above `from`:
     `Granted`, `Returned`, `Absorbed` and `Split` are applied to it as in section 3.
     **A chunk of a `Returned` that the region has no grant of is passed over**, with a
     warning: the record says that the region does not hold the chunk, which is so. Any
     other record that does not fit the table (a `Granted` of a chunk that is granted
     already, a region that is not there, a part's chunk that the region does not hold)
     ends the start with `StoreError::Table`.

   The next segment's number is at least `from`, also when no segment is left: a
   record for the table in a segment below `from` would not be read at the next start.
3. **The division.** With `told` the division the store was started with:
   - there is a table, made from the same division: nothing changes;
   - there is a table, made from another (other areas, or another home chunk): the
     world is **made over** (below);
   - there is no table and no `layout` file: a new world; the table is written;
   - there is no table but a `layout` file (a world from before this record): if its
     fingerprint is `told.layout`, the table is written and nothing else changes, so
     the stripes keep their states, logs, epochs and entity ids; otherwise the world is
     made over. A log with records for the table and no table file is
     `StoreError::Table`.

   In every case a `layout` file that is still there once the table is durable is
   removed and the world's directory synced, so that one left by a store that died
   between the two goes at the next start.

   **Making a world over**, which is `Lanes::fold` with the table added, in the order
   that makes a second attempt after a crash harmless: (a) the block changes of every
   region's live commits, chosen as for an opening (section 3.4; all of them where
   there is no table), are applied and made durable (`Job::Fold`); (b) an `Opened {
   restored: 0 }` is appended for every region that has a live commit, a whole state in
   a record or a state file, and the log is synced; (c) the state files are removed,
   and the region files with an empty block of regions `told` does not have, and
   `regions/` is synced; (d) the table of `told` is written with `from` = the next
   segment and made durable. Until (d) is durable a restart finds the old table, or
   none, and does all of it again.
4. **Files in line with the table**: lanes of regions the table does not have are
   dropped; state files of regions that are not living are removed, and their region
   files if the block is empty; the region file of every living region whose highest
   epoch in memory is above its file's (the part of a split, if the store died before
   step 5 of 3.7) is written, and `regions/` synced. A failure here ends the start.

The table of grants in memory is the file's with the log's records applied; the file is
not written at a start unless step 3 says so.

#### 4.3 What a kill leaves

| Operation | Writes and syncs, in order | Killed before the step in bold is through | After it |
|---|---|---|---|
| Claim | append `Granted` (with the group's commits); **sync the segment, and `log/` if the segment is new** | granted or not; never answered | granted |
| Return | sync the directories of saved manifests; append `Returned`; **sync as above** | the region's still, or free; its saves may be durable | free |
| Merge | (group synced); append `Absorbed`; **sync as above**; remove `B.state`, `B.region` | merged or not, as a whole; never answered. Not merged: both regions as their state files have them | merged; step 4 of 4.2 removes what is left |
| Split | (group synced); append `Split`; **sync as above**; write, sync, rename `N.region`; sync `regions/` | split or not, as a whole; never answered. Not split: no `N`, `A` as its state file has it | split; step 4 of 4.2 writes `N.region` |
| Table file | write, sync, rename `table`; **sync `regions/`**; remove segments | the old file or the new; every segment from the old file's `from` on is there | new file; segments below its `from` may be there, and change nothing |
| A failed write or sync | truncate the segment; **sync it** | what was cut off counts or not, as a whole beginning of its group; nothing was answered since | it does not count |
| Opening | region file; append `Opened`; saves of the replay, if there is any | as today | as today |
| Start | as 4.2 | every step is done again | |

"Or not" is decided by what the disk kept: a record counts if it is whole. A kill during
a write leaves a record cut off, which does not count. In every row, what was answered
is in the column "After it".

**The kill points the tests must cover**: every change and sync of the simulated disk
(`Fault::Stop(n)` for every `n`, and `Fault::Fails(n, k)` for every `n` and `k` from 1
to 3, each with `Survival::Nothing`, `Torn`, `Everything` and `Untruncated`; `kill.rs`
does the first three with `Fail(n)` for commits today) of a scenario that contains at
least: a claim in a group with commits of two regions; a return behind a save; a claim
by another region of the chunk returned; a merge of two regions of which the absorbed
one holds granted chunks; a split; a checkpoint after each of the two, so that the
table file is written and segments are removed; an opening of the survivor and of the
part with a new epoch; and then a second start of the store (`start`, on the simulated
disk) on what the first start left, killed at every point of its own as well. A
scenario goes on after a fault that is over by opening its regions again, as the one in
`kill.rs` does with `opened`: every handle is lost by it.

**What is checked** on what each kill leaves, by starting a store on it, reading the
list and opening every living region with a higher epoch:

- the store starts, and starts again on what that start left with the same result;
- no chunk is in the `held` of two regions, and no region that is absorbed can be
  opened;
- every claim that was answered `granted` is in its region's `held`, unless that region
  returned the chunk or a merge or split that happened took it elsewhere; a claim that
  was not answered is there or not;
- a merge that was answered has happened; one that was not has happened or not, and in
  either case as a whole: the survivor has the merged state and the absorbed region's
  chunks and the other is refused, or both are restored as they were before it. The same
  for a split, with the part in the list or not;
- every commit that was confirmed is in what its region is restored with, as `kill.rs`
  checks today, and its block changes are in the chunk as whoever holds it now loads
  it; no block that a later holder set is as an earlier holder left it.

### 5. The list of regions

In this process: `Store::regions() -> Result<RegionList, StoreError>`, a message to the
commit thread, which ends the group first so that the list has nothing that is not
durable. The error is that of an unsettled log (section 4.1).

Over TCP the first message on a connection becomes

```rust
pub enum StoreHello {
    /// Open a region, as today.
    Region(RegionHello),
    /// Send the list of regions and close.
    Regions,
}
```

and a connection that says `Regions` is sent one `RegionList` and closed, or closed
without one while the log is unsettled. `StoreHandle::connect` says
`StoreHello::Region`; a function `regions(address)` beside it reads the list. A bare
`RegionHello` is no longer what the store reads first. The coordinator connects anew
each time it wants the list, which is every few seconds; a lasting connection is not
needed for that.

- `home`, `regions` (the living ones, ascending) and `absorbed` (oldest first, the
  latest 4096) are the table's.
- `RegionInfo::epoch` is the lane's highest epoch, 0 if it was never opened.
- `RegionInfo::bounds` is the box around the region's **grants**, worked out when the
  list is asked for by going through them; nothing is kept for it. The areas a region
  is pinned to are not in it; they are in `RegionInfo::pinned`, which becomes a
  `Vec<ChunkArea>`, so that `RegionInfo` is no longer `Copy`.

**Pinned regions are the store's to know and the coordinator's to read.** The store is
told on its command line (section 2); the coordinator learns which regions are pinned
from the list. Whether the coordinator decides anything by itself is its own setting.

### 6. Threads and order

- **The table is the commit thread's alone.** Who holds a chunk is decided there, in
  the order messages arrive. That is the order across regions: a claim that arrives
  before the `Message::Returned` of another region's return is answered `foreign`; one
  that arrives after it is granted, by a `Granted` that is behind the `Returned` in the
  log.
- **Commits do not wait for saves**, as today. A return's sync of the saved chunks is
  done by the thread for chunks. The commit thread additionally writes the table file
  (at a checkpoint that frees a segment) and a region file per split, and syncs once
  more per merge and per split.
- **Claims and returns are part of the group**, changed in memory when they are
  appended and undone if the group fails. **Merges and splits are not**: the group is
  ended before, they are synced by themselves and change memory afterwards, because
  what they change could not be undone simply and they are rare.
- **A return against the same region's later claim and return** is told apart by its
  number (section 3.3); against a split of the chunk by the split taking the chunk out
  of `returning`.
- **A merge against the absorbed region's last commits**: the group is ended first, so
  a commit of `B` that arrived before the `AbsorbCommit` is live, and the merge is
  declined. What `B` asked the thread for chunks to do was passed on before anything
  `A` asks after the merge, and is done before it.
- **A merge or split against the region's own checkpoint under way**: its tick is above
  every tick the session named, and a checkpoint not above the latest whole state is
  not put in place (sections 3.5, 3.6).
- **Saves against a change of holder**: a save is passed on only for the holder at that
  moment, and the thread for chunks keeps the order. A chunk changes its holder only by
  a record that the old holder's session asked for (or a merge that ends it), so no
  save of the old holder can come behind one of the new.
- **A hello with nothing to replay** is answered by the commit thread; one with
  something by the thread for chunks, as today.

### 7. What the store refuses

| Asked | When | Answer |
|---|---|---|
| Hello | lower epoch than the region's highest | `StoreError::EpochRefused`; `StoreWelcome::EpochRefused` (as today) |
| Hello | another layout than the division's | `StoreError::LayoutMismatch`; `StoreWelcome::Refused` (as today) |
| Hello | the region was absorbed, and is among the pairs kept | `StoreError::Absorbed { region, into }`; `StoreWelcome::Absorbed { into }` |
| Hello | the table has no such region | `StoreError::UnknownRegion { region }`; `StoreWelcome::Refused` |
| Hello, list | the log is unsettled, or the hello fails with any other `StoreError::Io` | `StoreError::Io`; over TCP the connection is closed without a welcome |
| `Load`, `Save` | the region does not hold the chunk | `StoreReply::NotHeld { position, holder }`; nothing done |
| `Claim` | a chunk is another region's | that chunk in `foreign`; the others are dealt with |
| `Return` | a chunk the region has no grant for; the home chunk | left out, logged; not answered |
| `AbsorbCommit`, `SplitCommit` | a condition of 3.6 or 3.7 fails | `StoreReply::Declined { reason }`; nothing changed, the handle as it was |
| Anything | the session is not the region's owner | not done, not answered (as today) |
| Start | pinned areas overlap | `StoreError::Division` |
| Start | the table cannot be decoded; the log does not fit it | `StoreError::Damaged`; `StoreError::Table` |

**What the worker process makes of a refused hello.** It tries again after
`StoreError::Io` and ends on every refusal but `EpochRefused`. That is why an I/O error
of a hello closes the connection instead of being sent as `StoreWelcome::Refused`, which
is what `converse` does with it today: the hellos that meet an unsettled log are those
of the workers that the failed sync has just lost. It holds for every `StoreError::Io`
of a hello, as `converse` cannot tell them apart and none of them is the worker's
fault. `UnknownRegion` ends the worker, as a wrong layout does today, which is right
for a store that was started with another division. `Absorbed` would end it too: no
region is absorbed in a cluster before C3, which has to teach the worker to drop the
region instead.

`Declined::reason` becomes a value instead of a text, so that a worker and a test can
tell the cases apart:

```rust
pub enum Decline {
    /// `region` has commits that no checkpoint of it covers.
    Uncheckpointed { region: RegionId },
    /// `tick` is not above `named`, the highest tick this session has named in a commit
    /// or a checkpoint or was restored up to.
    Tick { named: u64 },
    /// The region to absorb is not a living region other than the one that asks.
    NoSuchRegion,
    /// The home region is never absorbed, and the home chunk never leaves it.
    Home,
    /// The region to absorb has no owner, or one with another epoch than named.
    NotOpened { epoch: Option<u64> },
    /// The part has a chunk the region does not hold.
    NotHeld { chunk: ChunkPos },
    /// The part has no chunks, or `as_epoch` is 0.
    Malformed,
    /// The record of it would be longer than a record of the log may be.
    TooLarge,
}
```

### 8. Changes to the messages of C0

In `crates/clustine-rpc/src/messages.rs`:

1. `StoreRequest::Claim { tick, chunks }` loses `tick` (section 3.2).
2. `StoreRequest::AbsorbCommit` gains `absorbed_epoch: u64` (section 3.6).
3. `StoreReply::Declined { reason: String }` becomes `Declined { reason: Decline }`,
   with the `Decline` of section 7.
4. New `StoreReply::NotHeld { position: ChunkPos, holder: Option<RegionId> }`. The
   worker's match on `StoreReply` (`RegionRunner::take_replies`) is exhaustive and has
   to learn it in the same step (section 9, C1.3).
5. `Restored` gains `pinned: Vec<ChunkArea>`; `held` is the grants only; `entity_ids`
   can be the empty block.
6. `StoreWelcome::Accepted` gains `pinned`; new `StoreWelcome::Absorbed { into }`; new
   `RestoredItem::Held`.
7. New `StoreHello` (section 5), the first message to the store in place of a bare
   `RegionHello`.
8. `RegionInfo::pinned` becomes `Vec<ChunkArea>`, and `RegionInfo` loses `Copy`; the
   round trip in `wire.rs` that makes one changes with it.
9. Comments: `StoreReply::Claimed` no longer speaks of "the claim's tick";
   `StoreReply::Split` says that the worker says hello for the region with the epoch it
   named; `SplitPart::state` is as of the split's tick; `Return` says that a flush
   behind it is answered when it is durable; `AbsorbCommit` and `SplitCommit` say that
   `tick` has to be above every tick the session has named.

`SplitCommit`, `SplitPart`, `Claimed`, `Absorbed`, `Split`, `RegionList` and `ChunkBox`
fit as they are. There is no message for a failed log: a lost handle and a connection
that closes are what a worker sees of it.

In `services/worldstore`: `StoreError` gains `Absorbed`, `UnknownRegion`, `Division` and
`Table`; `Store` gains `memory_divided`, `local_divided` and `regions`; `Division` is
new.

### 9. Building it

Each step leaves `cargo test --workspace` and the run with `CLUSTINE_TEST_BOUNDARIES=0,4`
green. An existing test that a step makes untrue as it is written is corrected in that
step, and this section names every one that is known: nothing else about what the
existing tests assert of commits, checkpoints and owners changes.

| # | Scope | Its tests |
|---|---|---|
| C1.1 | `clustine-format`: the four records and the table file | Known answers, round trips, damage and cut-off at every byte, arbitrary bytes, as for the records and files there are |
| C1.2 | A failed write or sync (section 4.1): every owner lost, the truncation made durable, nobody served meanwhile, a hello's I/O error closing the connection; the simulated disk's `untruncated`, `Survival::Untruncated`, `Fault::Fails` | Below, F1 to F4; `kill.rs` as it is, with the fourth survival |
| C1.3 | The division at the start, the table file, pinned regions and home, unknown regions refused, entity ids as in section 2, `Store::regions` and `RegionInfo`, loads and saves by the holder only, `NotHeld`, `Restored::pinned`; everything that makes a store | A new world, the same division again, another division and another home chunk (made over, at every kill point of `start`), a world of today with the same and with another layout; below, 1 |
| C1.4 | Claims and returns, replay only into what is held and above the grant's tick, `Restored::held` and its way over TCP, the table file written and segments removed | Below, 2 to 11 |
| C1.5 | `AbsorbCommit`, `SplitCommit`, whole states in the log, the part's region file, absorbed regions refused, a hello with nothing to replay answered by the commit thread | Below, 12 to 22 |
| C1.6 | `StoreHello` and the list over TCP | The list from another process equals `Store::regions`; `StoreHandle::connect` opens a region as before; a connection that says `Regions` gets the list and is closed |
| C1.7 | The kill tests of section 4.3; `docs/world-format.md` | Section 4.3 |

**What C1.2 changes in existing tests** (`services/worldstore/src/tests.rs`). All three
assert what ADR-0008 says of a failed group, which section 4.1 changes on purpose:

- `a_commit_whose_sync_fails_is_not_answered_and_loses_the_handle`: the bystander, which
  wrote nothing in the group, is lost as well (`assert!(!bystander.is_lost())` turns
  round). Its last assertion, that the segment whose sync failed is never synced again,
  becomes: the segment is synced once more, after it was truncated, it is no longer
  than what was durable, and what follows goes to another segment.
- `a_new_owner_is_restored_only_with_what_is_durable`: the hello that is handled while
  syncs still fail gets `StoreError::Io` instead of the region; the test lets syncs
  succeed again and says hello once more, and that owner is restored with the first
  commit only, which is what the test is about.
- `a_commit_that_cannot_be_written_is_cut_off_and_loses_the_handle`: the other region
  does not "commit on"; it is lost too, and commits on after it has opened its region
  again.

The scenario of `kill.rs` already opens its regions again after a fault (`opened`) and
asserts nothing of the neighbour's handle, so it is expected to hold as it is; whoever
builds C1.2 finds out.

**What C1.3 touches**, beyond the store's own code:

- **The store's tests.** `hello` in `tests.rs` names the layout of a boundary at 0, so
  every store those hellos go to is made with `Division::stripes` of that layout and
  the home chunk at the origin: the helpers `stores`, `held_saves` and `switched`,
  `store_on` in `kill.rs`, and every direct call of `Store::local` or `Store::memory`
  that is followed by such a hello, of which `tests.rs` has some twenty and `tcp.rs`
  six. Tests that use `spawn`, `spawn_local` or `whole_world` stay as they are.
- **Three tests whose point is the layout or a chunk outside the stripe**:
  `a_hello_with_another_layout_than_the_first_is_refused` (the layout is the store's,
  not the first hello's);
  `a_world_opened_with_another_layout_has_what_the_regions_of_the_old_one_committed`
  (the division is given at each start, and the `layout` file it reads at the end is
  gone: it reads the list instead); `commits_are_answered_while_a_save_is_under_way`
  (the western region saves a chunk at x = 1000, which is the eastern region's). And,
  to be looked at, the tests in `tcp.rs` that say hello for region 2 or 9 of a world of
  two.
- **`services/worker/tests/specification.rs`**, all of it: its hellos name the layout
  of a boundary at 1, on `Store::memory` and `Store::local`, which would refuse every
  one of them as another layout. `World::memory` and `World::local` make their store
  with the stripes of that layout. The unit tests in the worker's `lib.rs` open region
  0 of an undivided world and stay as they are.
- **The worker**: `RegionRunner::take_replies` gets an arm for `NotHeld`. A region
  that keeps to its stripe never gets one; it is logged as an error and, for a chunk
  that was being loaded, handled as `Unreadable` is, so that nothing waits for it. C2b
  gives it its meaning.
- **`bin/clustine`**: `Server::start` in `lib.rs` (the division from
  `Config::boundaries` and `spawn_point()`); `cluster::worldstore` and
  `Service::Worldstore` in `main.rs` (`--boundaries`); `start_store` in
  `tests/common/processes.rs`, which starts `clustine worldstore` without it and has
  the boundaries at hand: without that change every chaos and move test fails.
- **Deployment and what the owner types**: `deploy/kubernetes/worldstore.yaml` gets the
  argument the coordinator's manifest has; `README.md` shows `clustine worldstore
  --world world` and gets it too.
- **The world of today** for the tests of step 3 of section 4.2 cannot be written by
  the store once the step is built. The test makes it by hand on a `MemoryDisk` with
  `clustine_format`: region files and state files in `regions/`, a log segment of
  `LogRecord::Commit` and `LogRecord::Opened`, and the file `layout` with the
  fingerprint in sixteen hexadecimal digits and a line end, as `Lanes::decide_layout`
  writes it. `start` does not read `meta`.

**For whoever writes tests from this record alone.** The store is driven as in
`tests.rs` (`Store::memory_divided`, `open_region`, `StoreHandle::request`,
`try_reply`, `flush`, `Store::regions`), its disk is made to fail as `Switched` there
does it or with a `Fault`, the thread for chunks is held as `HeldSaves` and `Held` there
do it, and it is killed as in `kill.rs` (`MemoryDisk::crashed`, a new store on what is
left). In what follows the world is either the stripes of a boundary at 0, of which the
eastern region is home, or, where chunks have to be free, a division with a gap: one
area west of x = 0, one from x = 16 on, and the home chunk at the origin, which makes
regions 0 and 1 pinned and region 2 home, holding the home chunk and nothing else.

Of a failed write or sync:

- F1. After a sync or an append of the log has failed, every handle is lost, also that
  of a region that asked for nothing, and no commit, claim or flush of that group is
  answered.
- F2. Once the store has welcomed a hello after it, what a crash leaves with
  `Survival::Untruncated` has the segment no longer than it was when it was last synced
  with success.
- F3. While the truncation or its sync fails (`Switched::failing_syncs`, or
  `Fault::Fails`), every hello is `StoreError::Io` and `Store::regions` is; over TCP
  the connection ends without a welcome and `StoreHandle::connect` returns
  `StoreError::Io`, not `Refused`. When it succeeds, the next hello is welcomed and
  restored with exactly what was confirmed.
- F4. From C1.4 on: a claim in a group that failed, then the same chunk claimed by
  another region and answered, then a crash with `Survival::Untruncated`: the store
  starts, and the chunk is the second region's.

Of regions and chunks:

1. A region loads and saves a chunk of its stripe without claiming; the other gets
   `NotHeld { holder: Some(..) }` for both, and the chunk is unchanged.
2. A claim of a chunk in the region's own stripe is granted, writes nothing to the log
   and adds nothing to `Restored::held`; one in the other's stripe is `foreign` with
   that region.
3. In the division with a gap: a claim of a chunk in the gap is granted once; the
   second region to ask gets `foreign`; two claims in one group of the same chunk by
   two regions are answered one each way; after the store is started again
   `Restored::held` has the chunk for the first and not the second.
4. A claim is answered only after the commits asked for before it are answered, and is
   in the log (after a crash that keeps nothing unsynced) whenever it was answered.
5. The tick of a grant is that of the last commit the region had sent, whatever the
   region's own tick; a region opened again with a higher epoch, restored up to `t`,
   that changes a block of a claimed chunk in tick `t + 1` and is opened once more finds
   the change in the chunk.
6. **Built over**: a region changes a block of `c`, saves `c`, returns it, without a
   checkpoint; another claims `c`, sets the same block otherwise, saves, checkpoints,
   returns; the first claims `c` again and is then opened anew: the block is as the
   second left it.
7. A chunk that is returned and not saved again loses nothing that was saved before; a
   claim that arrives before the return is through is `foreign`, one after it granted; a
   flush behind the return is answered only once a crash would keep the return.
8. A region that claims a chunk again while its return of it is under way keeps it, with
   the tick it had.
9. **A return that was called off frees nothing later.** With the thread for chunks
   held: a region returns `c`, claims it again, commits a change to it, saves it and
   returns it again. When the thread is let go as far as the first return and the store
   is killed there, `c` is the region's and the change is in what it loads after
   opening again. Let go to the end, `c` is free and the stored chunk has the change.
10. A `Returned` in the log for a chunk the region has no grant of (made by hand) does
    not keep the store from starting, and changes nothing.
11. After checkpoints of every region, the log has no segment below the table file's
    `from`, and a store started on that world has the same list and the same `held`.
12. A merge is declined, each with its reason and nothing changed, for: a survivor or an
    absorbed region with a commit behind its checkpoint; a tick not above one the
    survivor's session named in a commit or in a checkpoint, also in a checkpoint that
    is still with the thread for chunks; the home region as the one absorbed; the
    region itself; an absorbed region that is not open, or open with another epoch.
13. After `Absorbed`: the survivor is restored with the state and tick of the request
    and no deltas; its `held` has what the other was granted, with the merge's tick; it
    is pinned to both areas; the absorbed region's handle is lost and its hello is
    refused with `Absorbed { into }`; the list has it among `absorbed` and not among
    `regions`; the survivor loads and saves chunks of both stripes.
14. Commits of the survivor after a merge are restored as deltas on the merged state; a
    checkpoint after them leaves no record of the merge needed.
15. A checkpoint with the merge's or the split's own tick, asked for afterwards, is not
    put in place, and the region is restored with the state of the record all the same.
16. A split is declined for: a commit behind the checkpoint; a tick not above one the
    session named; no chunks; a chunk not held; the home chunk; epoch 0.
17. After `Split { region: N }`: `N` is higher than every id there was; the list has it
    with `as_epoch`, not pinned, its bounds around the part's chunks; a hello for `N`
    with `as_epoch` is accepted and restored with the part's state at the split's tick,
    `held` the part's chunks with that tick, the empty block of entity ids; a hello
    with a lower epoch is `EpochRefused { seen: as_epoch }`; the old region is restored
    with its state of the request, and gets `NotHeld { holder: Some(N) }` for the
    part's chunks.
18. The hello for `N` is answered while the thread for chunks is held; so is any hello
    of a region with no commit behind its checkpoint. One with a block change to replay
    is answered only when the thread is let go.
19. **A split of a chunk that is being returned.** With the thread for chunks held: a
    region returns `c` and then splits a part off that has `c`. The split is answered;
    when the thread is let go, `c` is the part's, another region's claim of it is
    `foreign` with the part, and the store starts again on what is left.
20. A part split off a pinned region and then returned by the part chunk by chunk is the
    pinned region's again; one absorbed by the pinned region is in its `held`.
21. Ids are not used again: after a merge and a restart of the store the next split
    gets a higher id than any before.
22. A checkpoint of the absorbed region that is still under way when the merge is
    written is not put in place.

## Consequences

- The store knows the regions and who holds what before anyone says hello, and after
  any crash it knows what it knew: the log decides, and every file follows from it.
- The stripes go on working through C1 and C2a with no change to workers, edges or the
  coordinator but the store's reply `NotHeld`, which a region that keeps to its stripe
  never gets, and the store's `--boundaries`.
- **A failed write or sync of the log stops every region of the world**, not only
  those that were committing: all open their regions again and are restored, and their
  players stand still for as long as that takes. A disk that fails does that to most
  regions within a tick today; a single failed sync did not. In the single process,
  which does not open regions again by itself, every region stops where one or a few
  did.
- A store whose log can be neither truncated nor synced serves nobody until it can or
  is started again. Workers keep trying.
- A region that is not opened keeps the segment of its merge or split in the log, as
  one with commits and no owner keeps its segments today.
- A merge or a split costs the commit thread two syncs and a small file; a checkpoint
  that frees a segment costs it the table file, which is about 16 bytes per grant.
- A hello of a region with nothing to replay no longer waits for the thread for chunks.
- The store has to be started with the layout the coordinator has. If it is not, it
  refuses every hello, and if the world had another division it has made it over by
  then, as the first hello of a wrong layout does today.
- The store of this record cannot be replaced by an older build on the same world.

## Changes to ADR-0010

1. **Section 1, "Each grant notes the tick of the holder at which it was made"**: the
   tick is that of the holder's last commit the store has, not the holder's own; `Claim`
   carries none (section 3.2 here). The holder's own tick can be issued again after a
   restore, and a grant noted with it would keep later changes out of the chunk.
2. **Section 1, pinned regions**: a pinned region can be pinned to several areas, and
   becomes so by absorbing another; the store is told the pinned regions when it is
   started; until C5 they are the stripes, with the stripes' ids.
3. **Section 2, "What it holds the store says when the region is opened"**, made
   precise for pinned regions: the store says what the region was granted and which
   areas it is pinned to, not which chunks of those areas are another region's. From
   C2b on a pinned region claims the chunks of its areas like any other before it
   treats them as its own, and the store answers from the table without a record. In
   C1 and C2a nothing is split off, and a stripe need not claim.
4. **Section 1, entity ids**: the store issues a block to pinned regions as well as to
   the home region, as it does to every stripe today; a region made by a split has
   none.
5. **Section 4, step 6**: the store cannot tell "the same worker's session";
   `AbsorbCommit` names the epoch the absorbed region was opened with. The survivor's
   log has to be empty as well as the absorbed region's, and `tick` above every tick
   its session has named, in a commit or a checkpoint.
6. **Section 4, step 6, "State files and the table of grants are brought in line
   afterwards and again when the store starts"**: the table in memory changes when the
   record is durable and is rebuilt from the table file and the log at a start; the
   absorbed region's files are removed afterwards and at a start; the state files of
   the survivor, and of both regions of a split, are written by their next checkpoints.
7. **Section 5, step 4**: the split does not open `N` and answers no handle; `N` is made
   with `as_epoch` and the worker says hello for it, which the store answers without
   its thread for chunks. The part's state is as of the split's tick, which has to be
   above every tick the session has named. The store also declines a part that has the
   home chunk or no chunks.
8. **Section 6, the list**: the store keeps the latest 4096 absorbed regions; a hello
   for one it has forgotten is refused as for a region that never was.
9. **Section 8**: a world from before is told by its `layout` file being there and the
   table not. With the layout it was last served with it is opened as it is, stripes
   and states kept, and only with another division made over. From C1 on a striped
   world has a table, so at C5 "last served in stripes" is a table made from another
   division than the one the store is started with, which is made over in the same way.
10. **New**: a region is refused loads and saves of what it does not hold with a reply
    of its own, `NotHeld`; returns are numbered, so that one that was called off frees
    nothing.

## Changes to ADR-0008

All in its section 3, "The world store":

1. **"A failed sync therefore loses the handle of every region that wrote in that
   group"**: of every region (section 4.1 here). A group now holds returns, which have
   no answer, and grants, which regions that wrote nothing can have relied on.
2. **"A failed sync is not retried"** stands for what the sync was to make durable. The
   segment is synced once more, after it was truncated, so that what was cut off does
   not come back; and until that has succeeded the store serves nobody.
3. **"Opening a region returns a `Restored`"**: the hello is answered by the commit
   thread when no block change is to be applied, and by the thread for chunks
   otherwise.
4. **"The block changes of those records are applied to the stored chunks as before"**:
   only those in chunks the region holds, of ticks above the grant's (ADR-0010).
5. **"When the layout changes"**: that is found when the store starts, not at the first
   hello, and "the layout" is the division the store is started with.
6. **"The store issues an entity id block when a region is opened for the first
   time"**: to a pinned region and to the home region; the part of a split has none.
7. A hello that fails with an I/O error closes the connection instead of being refused
   in words, so that a worker tries again.

## Open questions

1. **Commits to chunks a region does not hold** are logged and never replayed. The store
   could refuse them (the handle would be lost, a commit having no other answer), at
   the price of looking up every changed block's chunk on the commit thread. Left out;
   to be decided when C2b shows whether a region can do this without a defect.
2. **Whether a worker needs to hear that a return is through.** Here it can ask for a
   flush behind it. If the region is to tell links `NotMine` only once the chunk is
   free, C2b may want a reply.
3. **Whether a sync of a truncated file can be trusted right after a sync of that file
   has failed** was not tried on a real file system, by the author or by the reviewer.
   What section 4.1 relies on is only that a sync which reports success has made the
   file's length durable. If that turns out not to hold somewhere, the `Cut` record
   that section rules out is the way back.
4. **The table file on the commit thread.** With some hundred thousand grants it is a
   few megabytes written and synced at a checkpoint that frees a segment. If that shows
   in commit times it can be written by the thread for chunks and put in place like a
   state file.
5. **4096 absorbed regions** is a guess at what an edge that was away can still need.
   ADR-0010 says "for a while" of the routing table and nothing of the store.
6. **`N`'s state travels back** to the worker that made it, in the `Restored` of its
   hello. A hello that says "I have the state" would spare that; not worth a message
   before it is measured.
7. **Pinned areas stay stripes** (`ChunkArea` has no limit along z). Whether anyone
   wants a pinned box after C5 is not for this step.
8. **A stripe's id that was a split-off region's** before the division changed (three
   stripes where there were two and a part) takes over that region's highest epoch if
   the region was living, and starts from nothing if it was absorbed. All services are
   started anew with a new layout, so no old owner is left to fence; said here because
   it is the one place an id is used again.
9. **A commit or a checkpoint longer than a record or a state file may be**, from a
   handle in the store's own process, is written and then read as cut off, today as
   after this record. A region's state would have to pass 64 MiB for it. Not made
   worse here, and not mended.
10. **`Absorbed` does not say which areas came with the chunks.** The survivor learns
    them at its next opening or from the list. If C3 needs them at once, the reply gets
    a field.
11. **Not every existing test was read against "by the holder only" and the division.**
    Section 9 names those that are known to change, by test or by kind. A test that
    turns out to load, save or open outside its stripe is corrected as those are.

## Review

An independent review against the code found thirteen defects in the first version of
this record, and judged it fit to build as corrected. What it changed:

1. A return that a claim had called off could still free the chunk: its message came
   back after a second return of the same chunk had been asked for, before the save in
   between was durable, and a confirmed change was lost. Returns are numbered, and a
   chunk is freed only by the return it is noted with (section 3.3).
2. A split could take a chunk whose return was under way; the message that came later
   either took the chunk from the part or left a record that kept the store from
   starting. The split takes the chunk out of `returning`, and reading the log passes
   over a `Returned` for a chunk the region has no grant of.
3. The `Cut` record could not be tested: on the simulated disk a truncation was final in
   every kind of crash. The record is gone; the truncation is made durable instead, and
   the simulated disk can bring truncated bytes back (section 4.1).
4. After a failed group, a region whose return had been undone kept its handle and was
   never told; and what became of messages from the thread for chunks, flushes and the
   list while the log could not be written was not said. Every region loses its owner
   now, and the store serves nobody until the log is settled.
5. Loads and saves passed on the strength of grants and returns that were not durable
   yet, so a region outside the failed group could go on from them. With every owner
   lost nobody does.
6. A hello refused in words because of a failed log ended the worker process, which
   tries again only after an I/O error, and the hellos that met it were those of the
   workers the failure had just lost. The connection is closed without a welcome.
7. Two existing tests that assert what a failed sync does to a bystander and to a hello
   were not named. They are, with a third the same rule changes, and with the
   assertion that a segment is never synced again (section 9, C1.2).
8. The list of what step C1.3 breaks was far too short: the whole of the worker's
   specification tests, some twenty-five stores made directly in the store's tests,
   the store's command line in the tests that start processes, the worker's match on
   the store's replies, and a "world of today" that cannot be written by a store that
   no longer exists. Section 9 has them.
9. A pinned region was not told which chunks of its areas it does not hold, against
   ADR-0010. It claims them from C2b on, and that is listed as a precision of ADR-0010.
10. Not opening the part in the split brought back a wait behind the thread for chunks
    that ADR-0010 had removed. A hello with nothing to replay is answered by the commit
    thread.
11. A checkpoint that was still under way could be put in place over a merged or split
    state with a tick at or above its own. The tick of a merge or split has to be above
    every tick the session has named, and no checkpoint is put in place that is not
    above the latest whole state.
12. Three statements about the code were wrong: `local::prepare` is not part of `start`
    and never runs on the simulated disk; a request is bounded only over TCP, so a
    record could pass the log's limit; `RegionInfo` is `Copy` today.
13. Smaller gaps: a `layout` file left for good by a kill at the wrong moment; that
    another home chunk makes a world over; a test that spoke of "a hello as before"
    although the first message changes; and when the table file is written, which is
    when the first segment of the log is kept for the table alone.

It also found two things simpler: a checkpoint with a lower tick than a record's is not
put in place, rather than put in place to no effect; and truncating and syncing in
place of the `Cut` record, which it asked rather than proposed.

Working the corrections in found two things the review had not named: a third existing
test that the rule for a failed group changes
(`a_commit_that_cannot_be_written_is_cut_off_and_loses_the_handle`, in which another
region commits on), and that making the truncation durable syncs a segment whose sync
has failed once more, which the first of the tests in item 7 asserts never happens.

## Found while building

What turned out otherwise than this record says when it was built, step by step, and
what was decided. None of it changes what section 4.3 guarantees.

### C1.3

1. **Section 4.2, step 3 (c): the region files of regions the new division does not
   have** (those with an empty block) are not removed while the world is made over,
   before the new table is written, but by step 4 of the same start, after it. Step 4
   removes exactly these files at every start in any case, and to know which regions
   the new division "does not have" in (c) the new table would have to be there before
   (d) writes it. A store that dies in between finds the new table and does step 4;
   one that dies before (d) finds the old table and makes the world over again, as the
   record says.
2. **Section 2, entity ids: the store counts the blocks it has issued** and counts on
   before it writes the region file, instead of working the next block out from the
   region files it has read and written. An opening whose region file was put in place
   and whose sync of `regions/` then failed is refused, and the block in that file was
   never told to anyone; but the next opening of another region would have been issued
   the same block, and its sync would have made both files durable. A failed opening
   can now pass over a block, of which there are 2047.
3. **`Restored::held` is empty until C1.4**, also for the home region of a division
   whose home chunk is in no pinned area and is therefore granted from the start. The
   table of section 9 gives `Restored::held` and its way over TCP to C1.4; the grant is
   in the table and in the list from C1.3 on, and loads and saves go by it.
4. **Records of kinds 4 to 7 in the log** stay what they were to the store before
   C1.1, records of no kind it knows (`StoreError::Damaged`), until the step that
   writes them: C1.4 for `Granted` and `Returned`, C1.5 for `Absorbed` and `Split`. No
   store before those steps writes one.
5. **What the owner types**: `docs/roadmap.md` shows the command line of
   `clustine worldstore` as well as `README.md` does ("Where M3 stands", what to try
   with real clients), and gets `--boundaries 4` with it. Without it the store would
   take the world for one region and refuse both workers.
6. **Existing tests that load or open outside their stripe** (open question 11), beyond
   those section 9 names, corrected as those are:
   - `tcp.rs`, `what_a_remote_handle_commits_and_saves_is_found_by_a_local_one_and_after_a_restart`
     and `a_chunk_with_every_section_different_survives_the_trip`: the handle in the
     store's own process that looks at what the remote one saved was the western
     region's and loaded chunks of the eastern one. It is the eastern region's now,
     opened with the same epoch in the first and the next in the second.
   - `tcp.rs`, `a_connection_that_sends_garbage_is_dropped_and_the_others_are_served`:
     its connection that stops in the middle of a request said hello for region 2 of a
     world of two. Its stores are made with three areas, of which that is the third.
   - `tcp.rs` and the worker's `lib.rs`: literals of `Restored` and of
     `StoreWelcome::Accepted` in tests get the new field `pinned`, and
     `what_a_remote_handle_commits_…` expects the eastern stripe in it.
   - `tests.rs`, `a_hello_with_another_layout_than_the_first_is_refused` is named
     `…_than_the_stores_is_refused`, as there is no first hello that decides any more,
     and asserts the refusal before any hello as well as after.

### C1.4

1. **Section 3.4, the bytes of `RestoredItem::Held`**: the store's crate has neither
   serde nor postcard (it is handed `wire`'s functions for everything it sends), so
   "the postcard of the `Vec<(ChunkPos, u64)>`" is made and read by two functions next
   to the message, `clustine_rpc::held_bytes` and `held_from_bytes`. No message
   changes by it.
2. **Section 9, scenario 11, "the log has no segment below the table file's `from`"**
   holds for the store that runs. After a crash a segment that was removed can be
   there again, because removing a segment is not made durable, today as before; the
   row "Table file" of section 4.3 says so ("segments below its `from` may be there,
   and change nothing"), and the store's test of the scenario checks the crash for
   the same list and the same grants, not for the segments.
3. **Section 4.2, a `Returned` of a region the table does not have** is passed over
   with the warning, like one of a chunk the region has no grant of: it says that the
   region does not hold the chunk, which is so. "A region that is not there" ends the
   start for `Granted`, as the section says, and will for `Absorbed` and `Split`.
4. **Section 3.3, a return of which no chunk is left** after those the region was not
   granted and the home chunk are left out makes no `Job::Return`, and takes no
   number: there is nothing for the thread for chunks to make durable for it.
5. **A test of C1.2 depended on how the commit thread grouped what it was sent**:
   `hellos_fail_until_cutting_the_log_back_succeeds` counts the changes and syncs up
   to a commit, and an opening and the commit behind it are one sync or two. It
   passed every run of C1.2 and failed once here. It now waits for the opening to be
   durable before it commits, so that each commit is a group by itself.

### C1.5

1. **Sections 3.6 and 3.7 do not say which reason is given when several hold.** The
   store looks in this order and names the first. A merge: `NoSuchRegion`, `Home`,
   `NotOpened`, `Uncheckpointed` (the survivor before the region to absorb), `Tick`,
   `TooLarge`. A split: `Uncheckpointed`, `Tick`, `Malformed`, `NotHeld`, `Home`,
   `TooLarge`. So a part with the home chunk asked for by a region that does not hold
   that chunk is declined as `NotHeld`, and `Home` is what the home region itself is
   told.
2. **Section 3.7, step 3, "with each chunk once"**: the chunks of the part are written
   in ascending order, not in that of the request.
3. **Section 3.4, "if no change is left to apply"** is taken as written, not as "no
   commit behind the checkpoint": a region whose live commits changed no block, or
   only blocks of chunks it does not hold or holds from a later tick, is answered by
   the commit thread as well.
4. **Section 3.7, step 4 and the review's second item: two things keep a chunk that was
   split off while it was being returned with the part**, and either would do: the
   split takes it out of `returning`, and a return frees only what the region still
   has a grant of when its message arrives. Without the first, a `Returned` naming the
   chunk would be written for the old region, which changes nothing in memory and is
   passed over at a start; the store's test of scenario 19 looks at the log for it.
5. **Section 3.6, step 5**: after the absorbed region's files are removed, `regions/`
   is synced, so that they are durably gone before the survivor is answered if the
   disk allows; a failure of either is logged and left to the next start, as the
   section says.
6. **A test of C1.4 looked at two handles after waiting for one**:
   `a_grant_of_a_failed_group_does_not_come_back` waited for the first region's answers
   to end and then asserted that the second was lost as well; the handles are lost one
   after the other, and once it looked in between. It waits for the second now.

### C1.6

1. **Tests in `tcp.rs` that speak to the store on a connection of their own** said a
   bare `RegionHello` first, which the store no longer reads: the helpers `greeted` and
   `welcoming_store` and the test
   `a_region_whose_owner_went_away_while_it_was_restored_is_opened_again_with_everything`
   say and expect `StoreHello::Region` now. What they assert is as it was. Section 9
   does not name them; section 5 implies them.
