# ADR-0011: The world store and regions

- Status: **Proposed**; the design of step C1 of milestone M3, phase C. Not reviewed and
  not built yet.
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
  order it is given them. A checkpoint syncs the saved chunks, writes the state under a
  temporary name and sends `Message::Checkpointed` back; the commit thread renames it
  (`Lanes::install`).
- The **log** (`log/<n>.wal`) has records of kind 1 (`Changes`, only read), 2 (`Commit`)
  and 3 (`Opened`). A segment is read up to its first record that is cut off or fails
  its checksum, and a store that starts never appends to a segment it found. A record
  of an unknown kind is an error (`FormatError::Corrupt("record kind")`).
- Per region there are `regions/<r>.region` (`RegionFile`: highest epoch, entity ids)
  and `regions/<r>.state` (`StateFile`: tick, state). `Lane::live` lists where the
  region's commits above its state file's tick are in the log; `Lanes::collect` removes
  the segments at the start of the log that no lane's `live` names.
- `Lanes::admit` refuses a lower epoch, writes the region file, replaces the owner,
  reads the state file and the live commits, appends `Opened { region, epoch, restored }`
  and has `Job::Restore` apply **all** block changes of those commits to the stored
  chunks before the hello is answered.
- After a failed append or sync, `Lanes::fail_log` cuts the segment back to what was
  durable (`Log::fail`), appends nothing more to it, and loses the owner of every region
  that wrote in the group. Whether the cut-off bytes are gone after a crash is not
  known; for commits the `Opened` record of the next opening makes that harmless.
- The store never sees a `Layout`, only its fingerprint, in `RegionHello::layout`. The
  first hello decides it (`Lanes::decide_layout`); another fingerprint than the one in
  the file `layout` makes the store put every region's commits into the chunks and drop
  the states (`Lanes::fold`). The store does not know which chunks a region covers, and
  lets every owner load and save any chunk.
- Over TCP (`tcp.rs`) a connection is about one region: `RegionHello`, then
  `StoreWelcome`, then the `Restored` in `RestoredPart`s, then requests and replies.

## Decision

### 1. What is on disk

```text
<world>/
  meta                 as before
  log/<n>.wal          the log; five new kinds of record
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
| 4 | `Cut` | segment u64, length u64 |
| 5 | `Granted` | region u32, tick u64, chunks |
| 6 | `Returned` | region u32, chunks |
| 7 | `Absorbed` | region u32, epoch u64, absorbed u32, tick u64, state bytes |
| 8 | `Split` | region u32, epoch u64, tick u64, state bytes, part u32, part epoch u64, chunks, part state bytes |

- `Cut`: the segment `segment` ends after `length` bytes; whatever it has beyond that is
  no part of the log (section 4.1).
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

`MAX_PAYLOAD_LENGTH` (64 MiB) stays. A request cannot be larger than
`wire::MAX_MESSAGE_LENGTH` (16 MiB), so a record made from one fits.

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
- per lane `latest`: the highest tick the store has of the region. It is set to the
  tick the region is restored up to when it is opened, and raised by every commit taken
  and by a merge or split of the region;
- per lane `returning`: the chunks a return of which is on its way through the thread
  for chunks (section 3.3);
- per lane, in place of `state_tick` alone, **where its latest whole state is**: in its
  state file (tick) or in an `Absorbed` or `Split` record of the log (tick, and where
  the record is, as an `Entry`). The one with the higher tick counts; with equal ticks
  the state file.

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
`clustine worldstore` gets `--boundaries`, as the coordinator has it, and the Kubernetes
manifest of the store the same argument as that of the coordinator. A division with
other areas, or with none, is for the store's tests until C5, which need chunks that
nobody holds; from C5 on it is what `clustine worldstore --pin` gives, `layout` goes,
and nothing else in this record changes.

Two divisions are the same if their home chunks and their areas are; that is what the
table keeps of the division it was made from.

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
  tick 0. A stripe therefore loads, saves and commits as today without ever claiming.
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
- **`Restored::held`** is the region's grants, in ascending order of the chunks, each
  with its tick. For a pinned region that is only what it was granted beyond its areas
  or took over in a merge; the chunks it holds by being pinned are not listed.
  `Restored` gains `pinned: Vec<ChunkArea>`, the areas the region is pinned to.
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

A commit is not looked into: block changes to chunks the region does not hold are
logged with the rest, and never applied to a stored chunk (section 3.4).

#### 3.2 `Claim { chunks }`

For each chunk named, each once and in the order of the request:

- the region holds it (by a grant or by being pinned, or a return of it is on its way,
  which is called off: the chunk leaves `returning`): it is among `granted`, and nothing
  about it changes, also not its tick;
- another region holds it: it is among `foreign`, with that region;
- nobody holds it: it is granted to the region with the tick `latest`, and is among
  `granted`.

If any chunk was granted anew, one `Granted { region, tick: latest, chunks }` with those
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
   warning. The rest goes into `returning`, and a `Job::Return { chunks, peer }` is
   passed on like a save: behind the group if the region has anything unsynced or held.
2. The thread for chunks makes the saves so far durable (`ChunkService::sync`, as for a
   checkpoint; a failure loses the handle) and sends `Message::Returned { session,
   chunks }`. A lost handle's return is dropped.
3. The commit thread drops it unless the session is still the one that opened the region
   last (`Lane::current`). Of the chunks, those still in `returning` leave it; if any
   are left, `Returned { region, chunks }` is appended to the log in the current group
   and the grants are taken out of the table at once. The chunk is then nobody's, or
   the pinned region's whose area it is in.

Not answered. A `Flush` asked for behind a return is answered only when the return is
durable, as its `Message::Flushed` follows the `Message::Returned` and is answered at
the end of the group that wrote the record.

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
- `Restored::held` and `Restored::pinned` are filled as in section 2; `entity_ids` as
  there.
- `latest` is the tick restored up to; `returning` is emptied.

Over TCP (`tcp.rs`): `StoreWelcome::Accepted` gains `pinned`. `held` follows the deltas
as one more item, `RestoredItem::Held`, whose bytes are the postcard of the
`Vec<(ChunkPos, u64)>` and whose tick is 0, cut into pieces like a state (`Parts`); it
is left out if `held` is empty. `Arriving::add` takes a second `Held`, or a state or a
delta behind one, for parts that do not fit together.

#### 3.5 Checkpoints, and removing segments

A checkpoint is done as today. When its state file is in place and durable
(`Lanes::end_group`), the lane's latest whole state is the file if the file's tick is
not below that of a record; the live commits up to the file's tick are let go as today.
A checkpoint with a lower tick than a record's is put in place and changes nothing.

The store notes the last segment that has a record for the table (`Granted`,
`Returned`, `Absorbed`, `Split`), when it appends one and when it reads the log at a
start. `Lanes::collect` keeps a segment if a lane's live commit is in it, if a lane's
latest whole state is a record in it, or if its number is from `from` up to that last
segment: those hold what the table file does not have. A `Cut` needs no keeping: it is
in a later segment than the one it cuts, and segments are removed from the start.

**The table file is written** in `Lanes::end_group`, after `Log::close`, when a state
file was put in place and a segment that is kept for the table alone is needed by no
lane: the file is written with `from` = the number of the next segment (`Log::next`),
made durable, and only then `from` is raised in memory and `collect` runs. If writing
fails, `from` stays and nothing is removed that was kept for the table. It is not
written while a cut is outstanding (section 4.1) or while the region file of a split's
part is not durable (section 3.7).

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
   - `tick` is above the tick of `A`'s latest whole state.
3. It appends `Absorbed { region: A, epoch, absorbed: B, tick, state }` and syncs the
   log, by itself. If that fails: `Lanes::fail_log`, both regions lose their owners,
   nothing is answered.
4. Only now memory is changed: `B`'s grants are `A`'s with tick `tick`; `B`'s areas are
   `A`'s; `B` leaves the regions and joins the absorbed pairs (of which the store keeps
   the latest 4096); `B`'s owner is lost and its lane goes; `A`'s latest whole state is
   the record, and its `latest` is `tick`.
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

#### 3.7 `SplitCommit { tick, state, part: { chunks, state }, as_epoch }`

From the session of `A`.

1. The group is ended.
2. The store declines unless: `A` has no live commit; `tick` is above the tick of `A`'s
   latest whole state; `chunks` is not empty; `A` holds every one of them; the home
   chunk is not among them; `as_epoch` is at least 1.
3. `N` is the next region id. It appends `Split { region: A, epoch, tick, state, part:
   N, part epoch: as_epoch, chunks, part state }`, with each chunk once, and syncs the
   log, by itself. If that fails: `fail_log`, `A` loses its owner, nothing is answered.
4. Memory: the next region id is `N + 1`; `N` is a living region, not pinned, granted
   `chunks` with tick `tick`; `A`'s grants among them go; both regions' latest whole
   state is the record; both regions' `latest` is `tick`; `N`'s highest epoch is
   `as_epoch`, with the empty block of entity ids.
5. `regions/N.region` is written and made durable. If that fails it is tried again
   before the table file is next written, and when the store starts (section 4.2).
6. `A` is answered `Split { region: N }`.

**`N` is not opened by the split.** ADR-0010 has `N` "opened by this session with
`as_epoch`" and the store answer "with `N` and the handle". A handle is a channel in
this process and a connection in another, and neither fits in a reply. `N` is made with
`as_epoch` as the highest epoch it was opened with; the worker says an ordinary hello
for `N` with `as_epoch`, which the store takes as it takes the same owner coming back,
and meanwhile runs `N` from what it has in memory. Nobody with a lower epoch can open
`N`, which is what the session was for. The worker gets `N`'s state back in the
`Restored`, and need not look at it.

The part's state is as of the same `tick` as `A`'s: the new region's ticks go on from
`A`'s.

#### 3.8 Fencing

- Every request is done only for the session that owns the region (as today); what
  comes back from the thread for chunks (`Checkpointed`, `Returned`) only for the
  session that opened the region last.
- A merge needs `B`'s epoch (3.6). A split gives `N` its epoch in the record.
- An absorbed region has no owner and is never opened again.
- An opening with a higher epoch fences whoever ran the region, wherever that owner is
  in a claim, a return, a merge or a split: what it asked for before the hello is done
  first (the hello ends the group), what it asks afterwards is not.

### 4. Recovery

#### 4.1 A failed write or sync: the cut

Today the bytes `Log::fail` cuts off may come back after a crash, if the cut did not
reach the disk. For a commit that is settled by the next `Opened`. A grant or a merge
that comes back would be a grant the store has since given to another region, or a
merge of a region that has since gone on by itself.

So `Lanes::fail_log` additionally notes the **cut**, the segment and its durable length,
and undoes in memory what the group did to the table: grants given are taken back and
returns are given back, in reverse order. (Merges and splits change memory only after
their own sync, so there is nothing of theirs to undo.) Every region with anything to be
answered at the end of the group loses its owner, not only those that appended.

**Before the commit thread does anything else for anyone**, it makes the outstanding
cuts durable: it appends a `Cut { segment, length }` for each to a new segment and syncs
it (`Log::sync`, which syncs `log/` for a new segment as well). If that fails, that segment is cut in turn, all cuts stay outstanding, and whatever
was to be done is not: a hello is refused with the I/O error, a request loses its
handle, a state file is not put in place. It is tried again with the next message.

When the store starts it reads every segment for its `Cut` records first, and then
passes over what a cut segment has beyond its length. A cut that is itself cut off was
written again behind the cut that covers it.

What this gives: nothing is answered, and no file is changed, on the strength of a table
that a restart would not rebuild. If the store dies before a cut is durable, the
records it was to cut off may count; they were never answered, they are a beginning of
their group in the order it was written, and each is whole.

#### 4.2 When the store starts

In this order, before any hello is taken (`start` in `lib.rs`; the thread for chunks
runs from step 4 on):

1. `local::prepare`, as today.
2. `Lanes::load`, as today: temporary files in `regions/` are removed, region files and
   state files are read. It notes the highest block of entity ids any region file has.
3. `regions/table` is read if it is there; one that cannot be decoded is
   `StoreError::Damaged`. Then the log, record by record in the order of its segments,
   cuts honoured:
   - what a record means for a **lane** counts wherever the record is: a `Commit` above
     the state file's tick is live; an `Opened` lets go of live commits, and of a whole
     state in a record, above its `restored`; an `Absorbed` or `Split` with a tick above
     the state file's is the region's latest whole state (a `Split` is that for both
     its regions), and lets go of live commits up to its tick; a `Split` makes the
     part's highest epoch at least `part epoch`;
   - what a record means for the **table** counts only in segments at or above `from`:
     `Granted`, `Returned`, `Absorbed` and `Split` are applied to it as in section 3. A
     record that does not fit the table (a grant of a chunk another region holds, a
     region that is not there) ends the start with `StoreError::Table`.

   The next segment's number is at least `from`, also when no segment is left: a
   record for the table in a segment below `from` would not be read at the next start.
4. **The division.** With `told` the division the store was started with:
   - there is a table, made from the same division: nothing;
   - there is a table, made from another: the world is **made over** (below);
   - there is no table and no `layout` file: a new world; the table is written;
   - there is no table but a `layout` file (a world from before this record): if its
     fingerprint is `told.layout`, the table is written and nothing else
     changes, so the stripes keep their states, logs, epochs and entity ids; otherwise
     the world is made over. Then `layout` is removed and the world's directory synced.
     A log with records for the table and no table file is `StoreError::Table`.

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
5. **Files in line with the table**: lanes of regions the table does not have are
   dropped; state files of regions that are not living are removed, and their region
   files if the block is empty; the region file of every living region whose highest
   epoch in memory is above its file's (the part of a split, if the store died before
   step 5 of 3.7) is written, and `regions/` synced. A failure here ends the start.

The table of grants in memory is the file's with the log's records applied; the file is
not written at a start unless step 4 says so.

#### 4.3 What a kill leaves

| Operation | Writes and syncs, in order | Killed before the step in bold is through | After it |
|---|---|---|---|
| Claim | append `Granted` (with the group's commits); **sync the segment, and `log/` if the segment is new** | granted or not; never answered | granted |
| Return | sync the directories of saved manifests; append `Returned`; **sync as above** | the region's still, or free; its saves may be durable | free |
| Merge | (group synced); append `Absorbed`; **sync as above**; remove `B.state`, `B.region` | merged or not, as a whole; never answered. Not merged: both regions as their state files have them | merged; step 5 of 4.2 removes what is left |
| Split | (group synced); append `Split`; **sync as above**; write, sync, rename `N.region`; sync `regions/` | split or not, as a whole; never answered. Not split: no `N`, `A` as its state file has it | split; step 5 of 4.2 writes `N.region` |
| Table file | write, sync, rename `table`; **sync `regions/`**; remove segments | the old file or the new; every segment from the old file's `from` on is there | new file; segments below its `from` may be there, and change nothing |
| Cut | truncate; append `Cut` to a new segment; **sync it and `log/`** | what was cut off counts or not, as a whole beginning of its group | it does not count |
| Opening | region file; append `Opened`; saves of the replay | as today | as today |
| Start | as 4.2 | every step is done again | |

"Or not" is decided by what the disk kept: a record counts if it is whole and no cut
covers it. A kill during a write leaves a record cut off, which does not count. In
every row, what was answered is in the column "After it".

**The kill points the tests must cover**: every change and sync of the simulated disk
(`MemoryDisk`, `Fault::Stop(n)` and `Fault::Fail(n)` for every `n`, each with
`Survival::Nothing`, `Torn` and `Everything`, as `kill.rs` does for commits today) of a
scenario that contains at least: a claim in a group with commits of two regions; a
return behind a save; a claim by another region of the chunk returned; a merge of two
regions of which the absorbed one holds granted chunks; a split; a checkpoint after
each of the two, so that the table file is written and segments are removed; an opening
of the survivor and of the part with a new epoch; and then a second start of the store
on what the first start left, killed at every point of its own start as well.

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

In this process: `Store::regions() -> RegionList`, a message to the commit thread, which
ends the group first so that the list has nothing that is not durable.

Over TCP the first message on a connection becomes

```rust
pub enum StoreHello {
    /// Open a region, as today.
    Region(RegionHello),
    /// Send the list of regions and close.
    Regions,
}
```

and a connection that says `Regions` is sent one `RegionList` and closed.
`StoreHandle::connect` says `StoreHello::Region`; a function `regions(address)` beside
it reads the list. The coordinator connects anew each time it wants the list, which is
every few seconds; a lasting connection is not needed for that.

- `home`, `regions` (the living ones, ascending) and `absorbed` (oldest first, the
  latest 4096) are the table's.
- `RegionInfo::epoch` is the lane's highest epoch, 0 if it was never opened.
- `RegionInfo::bounds` is the box around the region's **grants**, worked out when the
  list is asked for by going through them; nothing is kept for it. The areas a region
  is pinned to are not in it; they are in `RegionInfo::pinned`, which becomes a
  `Vec<ChunkArea>`.

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
- **A merge against the absorbed region's last commits**: the group is ended first, so
  a commit of `B` that arrived before the `AbsorbCommit` is live, and the merge is
  declined. What `B` asked the thread for chunks to do was passed on before anything
  `A` asks after the merge, and is done before it.
- **Saves against a change of holder**: a save is passed on only for the holder at that
  moment, and the thread for chunks keeps the order. A chunk changes its holder only by
  a record that the old holder's session asked for (or a merge that ends it), so no
  save of the old holder can come behind one of the new.

### 7. What the store refuses

| Asked | When | Answer |
|---|---|---|
| Hello | lower epoch than the region's highest | `StoreError::EpochRefused`; `StoreWelcome::EpochRefused` (as today) |
| Hello | another layout than the division's | `StoreError::LayoutMismatch`; `StoreWelcome::Refused` (as today) |
| Hello | the region was absorbed, and is among the pairs kept | `StoreError::Absorbed { region, into }`; `StoreWelcome::Absorbed { into }` |
| Hello | the table has no such region | `StoreError::UnknownRegion { region }`; `StoreWelcome::Refused` |
| Hello | a cut cannot be made durable | `StoreError::Io`; `StoreWelcome::Refused` |
| `Load`, `Save` | the region does not hold the chunk | `StoreReply::NotHeld { position, holder }`; nothing done |
| `Claim` | a chunk is another region's | that chunk in `foreign`; the others are dealt with |
| `Return` | a chunk the region has no grant for; the home chunk | left out, logged; not answered |
| `AbsorbCommit`, `SplitCommit` | a condition of 3.6 or 3.7 fails | `StoreReply::Declined { reason }`; nothing changed, the handle as it was |
| Anything | the session is not the region's owner | not done, not answered (as today) |
| Start | pinned areas overlap | `StoreError::Division` |
| Start | the table cannot be decoded; the log does not fit it | `StoreError::Damaged`; `StoreError::Table` |

`Declined::reason` becomes a value instead of a text, so that a worker and a test can
tell the cases apart:

```rust
pub enum Decline {
    /// `region` has commits that no checkpoint of it covers.
    Uncheckpointed { region: RegionId },
    /// `tick` is not above the tick of the state the store has, which is `stored`.
    Tick { stored: u64 },
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
}
```

### 8. Changes to the messages of C0

In `crates/clustine-rpc/src/messages.rs`:

1. `StoreRequest::Claim { tick, chunks }` loses `tick` (section 3.2).
2. `StoreRequest::AbsorbCommit` gains `absorbed_epoch: u64` (section 3.6).
3. `StoreReply::Declined { reason: String }` becomes `Declined { reason: Decline }`.
4. New `StoreReply::NotHeld { position: ChunkPos, holder: Option<RegionId> }`.
5. `Restored` gains `pinned: Vec<ChunkArea>`; `held` is the grants only; `entity_ids`
   can be the empty block.
6. `StoreWelcome::Accepted` gains `pinned`; new `StoreWelcome::Absorbed { into }`; new
   `RestoredItem::Held`.
7. New `StoreHello` (section 5), the first message to the store in place of a bare
   `RegionHello`.
8. `RegionInfo::pinned` becomes `Vec<ChunkArea>`.
9. Comments: `StoreReply::Claimed` no longer speaks of "the claim's tick";
   `StoreReply::Split` says that the worker says hello for the region with the epoch it
   named; `SplitPart::state` is as of the split's tick; `Return` says that a flush
   behind it is answered when it is durable.

`SplitCommit`, `SplitPart`, `Claimed`, `Absorbed`, `Split`, `RegionList` and `ChunkBox`
fit as they are.

### 9. Building it

Each step leaves `cargo test --workspace` and the run with `CLUSTINE_TEST_BOUNDARIES=0,4`
green. Existing tests of the store that this record makes untrue as they are written are
corrected in the step that does so, by changing how their store is made or which chunk
they use, never what they assert about commits, checkpoints and owners. Known so far:
`a_hello_with_another_layout_than_the_first_is_refused` and
`a_world_opened_with_another_layout_has_what_the_regions_of_the_old_one_committed` (the
division is given at the start; the second also reads the `layout` file, which is gone);
`commits_are_answered_while_a_save_is_under_way` (the western region saves a chunk at
x = 1000); and, to be looked at, the tests in `tcp.rs` that say hello for region 2 or 9
of a world of two. The helpers `stores`, `store_on`, `held_saves` and `switched` make
their store with the stripes of the layout `hello` names, a boundary at 0.

| # | Scope | Its tests |
|---|---|---|
| C1.1 | `clustine-format`: the five records and the table file | Known answers, round trips, damage and cut-off at every byte, arbitrary bytes, as for the records and files there are |
| C1.2 | The cut: `fail_log` notes it, it is written before anything else, the start honours it | A sync that fails, then a crash that keeps everything: the record cut off does not count once something later was answered; the cut itself failing; existing `kill.rs` green |
| C1.3 | The division at the start, the table file, pinned regions and home, unknown regions refused, entity ids as in section 2, `Store::regions`, loads and saves by the holder only, `Restored::pinned`; the single process, `clustine worldstore --boundaries`, the manifest | A new world, the same division again, another division (made over, at every kill point), a world of today with the same and with another layout (from a directory written by the store as it is before this step); a region outside its stripe gets `NotHeld` |
| C1.4 | Claims and returns, replay only into what is held and above the grant's tick, `Restored::held` and its way over TCP, the table file written and segments removed | Below, 1 to 9 |
| C1.5 | `AbsorbCommit`, `SplitCommit`, whole states in the log, the part's region file, absorbed regions refused | Below, 10 to 17 |
| C1.6 | `StoreHello` and the list over TCP | The list from another process equals `Store::regions`; a hello as before still opens a region |
| C1.7 | The kill tests of section 4.3; `docs/world-format.md` | Section 4.3 |

**For whoever writes tests from this record alone.** The store is driven as in
`tests.rs` (`Store::memory_divided`, `open_region`, `StoreHandle::request`,
`try_reply`, `flush`, `Store::regions`) and killed as in `kill.rs` (a `MemoryDisk` with a
`Fault`, `MemoryDisk::crashed`, a new store on what is left). In what follows the world
is either the stripes of a boundary at 0, of which the eastern region is home, or, where
chunks have to be free, a division with a gap: one area west of x = 0, one from x = 16
on, and the home chunk at the origin, which makes regions 0 and 1 pinned and region 2
home, holding the home chunk and nothing else.

1. A region loads and saves a chunk of its stripe without claiming; the other gets
   `NotHeld { holder: Some(..) }` for both, and the chunk is unchanged.
2. A claim of a chunk in the region's own stripe is granted and adds nothing to
   `Restored::held`; one in the other's stripe is `foreign` with that region.
3. In the division with a gap: a claim of a chunk in the gap is granted once; the second region to ask gets `foreign`; two claims in one
   group of the same chunk by two regions are answered one each way; after the store is
   started again `Restored::held` has the chunk for the first and not the second.
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
9. After checkpoints of every region, the log has no segment below the table file's
   `from`, and a store started on that world has the same list and the same `held`.
10. A merge is declined, each with its reason and nothing changed, for: a survivor or an
    absorbed region with a commit behind its checkpoint; a tick not above the stored
    state's; the home region as the one absorbed; the region itself; an absorbed region
    that is not open, or open with another epoch.
11. After `Absorbed`: the survivor is restored with the state and tick of the request
    and no deltas; its `held` has what the other was granted, with the merge's tick; it
    is pinned to both areas; the absorbed region's handle is lost and its hello is
    refused with `Absorbed { into }`; the list has it among `absorbed` and not among
    `regions`; the survivor loads and saves chunks of both stripes.
12. Commits of the survivor after a merge are restored as deltas on the merged state; a
    checkpoint after them leaves no record of the merge needed.
13. A split is declined for: a commit behind the checkpoint; a tick not above; no
    chunks; a chunk not held; the home chunk; epoch 0.
14. After `Split { region: N }`: `N` is higher than every id there was; the list has it
    with `as_epoch`, not pinned, its bounds around the part's chunks; a hello for `N`
    with `as_epoch` is accepted and restored with the part's state at the split's tick,
    `held` the part's chunks with that tick, the empty block of entity ids; a hello
    with a lower epoch is `EpochRefused { seen: as_epoch }`; the old region is restored
    with its state of the request, and gets `NotHeld { holder: Some(N) }` for the
    part's chunks.
15. A part split off a pinned region and then returned by the part chunk by chunk is the
    pinned region's again; one absorbed by the pinned region is in its `held`.
16. Ids are not used again: after a merge and a restart of the store the next split
    gets a higher id than any before.
17. A checkpoint of the absorbed region that is still under way when the merge is
    written is not put in place.

## Consequences

- The store knows the regions and who holds what before anyone says hello, and after
  any crash it knows what it knew: the log decides, and every file follows from it.
- The stripes go on working through C1 to C4 with no change to workers, edges or the
  coordinator but the store's reply `NotHeld`, which a region that keeps to its stripe
  never gets.
- A region that is not opened keeps the segment of its merge or split in the log, as
  one with commits and no owner keeps its segments today.
- A merge or a split costs the commit thread two syncs and a small file; a checkpoint
  that frees a segment costs it the table file, which is about 16 bytes per grant.
- A failed write or sync of the log now touches every later step once: a cut has to be
  written before the store does anything else.
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
3. **Section 1, entity ids**: the store issues a block to pinned regions as well as to
   the home region, as it does to every stripe today; a region made by a split has
   none.
4. **Section 4, step 6**: the store cannot tell "the same worker's session";
   `AbsorbCommit` names the epoch the absorbed region was opened with. The survivor's
   log has to be empty as well as the absorbed region's, and `tick` above its stored
   state's.
5. **Section 4, step 6, "State files and the table of grants are brought in line
   afterwards and again when the store starts"**: the table in memory changes when the
   record is durable and is rebuilt from the table file and the log at a start; the
   absorbed region's files are removed afterwards and at a start; the state files of
   the survivor, and of both regions of a split, are written by their next checkpoints.
6. **Section 5, step 4**: the split does not open `N` and answers no handle; `N` is made
   with `as_epoch` and the worker says hello for it. The part's state is as of the
   split's tick. The store also declines a part that has the home chunk or no chunks.
7. **Section 6, the list**: the store keeps the latest 4096 absorbed regions; a hello
   for one it has forgotten is refused as for a region that never was.
8. **Section 8**: a world from before is told by its `layout` file being there and the
   table not. With the layout it was last served with it is opened as it is, stripes
   and states kept, and only with another division made over. From C1 on a striped
   world has a table, so at C5 "last served in stripes" is a table made from another
   division than the one the store is started with, which is made over in the same way.
9. **New**: a region is refused loads and saves of what it does not hold with a reply
   of its own, `NotHeld`; a failed write of the log is followed by a `Cut` record.

## Open questions

1. **Commits to chunks a region does not hold** are logged and never replayed. The store
   could refuse them (the handle would be lost, a commit having no other answer), at
   the price of looking up every changed block's chunk on the commit thread. Left out;
   to be decided when C2b shows whether a region can do this without a defect.
2. **Whether a worker needs to hear that a return is through.** Here it can ask for a
   flush behind it. If the region is to tell links `NotMine` only once the chunk is
   free, C2b may want a reply.
3. **The existing tests were not all checked against "by the holder only".** Four that
   break are named in section 9; the store has some 140 hellos in its tests, and the
   worker's tests open region 0 of an undivided world "whatever part of the world the
   region takes itself to be", which holds everything and should be unaffected.
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
9. **`Absorbed` does not say which areas came with the chunks.** The survivor learns
   them at its next opening or from the list. If C3 needs them at once, the reply gets
   a field.
