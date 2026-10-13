# World format

How Clustine stores a world on disk. This describes format version 2 as implemented in
`crates/clustine-format` (encodings) and `services/worldstore` (files); see
`docs/adr/0008-durable-regions-and-resuming.md` for what the store promises of commits
and states, `docs/adr/0011-the-world-store-and-regions.md` for what it promises of
regions and the chunks they hold, and `docs/adr/0022-what-a-chunk-carries.md` for what
a chunk is and why it is one file.

The format is not stable yet: it may change without a migration path until the first
release. **A world of format 1 is not opened and not made over.** A store that finds
`format=1` in a world's `meta` stops before it reads, changes or removes anything else
in the directory, and says:

```text
This world was made by an earlier Clustine (world format 1; this server writes
format 2). Worlds are not carried over before the first release: move <directory>
away or name another, and a new world is made.
```

## Principles

- **What a region was given is stored.** A chunk is stored as the region that holds it
  has it, so that what a player was shown is what is there after a restart, whatever a
  later generator would make of the place
  (`docs/adr/0021-generation-is-a-function.md`). *Not built yet*: until that record's
  step W5 a region saves the chunks it changed, and a chunk that was never saved is
  generated again when it is needed.
- **A world takes a newer generator.** A world records the generator settings it was
  made with and refuses to open with others; the version of the generator is no part
  of them, and a chunk says which version it is exactly as made by. *Not built yet*
  either: there is one version of each generator so far.
- **A chunk is one file**, checksummed, written whole or not at all. Nothing is shared
  between chunks, and a chunk that is saved again leaves nothing behind.
- **Files are never modified in place.** Every file but the log is written under a
  temporary name, made durable and renamed, so a reader sees it whole or not at all.
  The log is only appended to.
- **Nothing is durable before it is synced,** and a file that was created, renamed or
  removed is not durably so before its directory is synced. The store syncs what it
  relies on, and nothing it has answered depends on what it has not synced.

## Directory layout

```text
<world>/
  meta
  log/<segment>.wal
  regions/table
  regions/players
  regions/<region>.region
  regions/<region>.state
  chunks/<dimension>/<rx>.<rz>/<x>.<z>.chunk
```

- `meta` is a text file of `key=value` lines: `format` (the format version),
  `data-version` (the Minecraft data version whose block state and biome ids the world
  uses) and `generator` (the generator settings). It will have `generator-version`
  besides, the highest version of the generator that made a chunk the world stores
  (`docs/adr/0021-generation-is-a-function.md`, section 8); no store writes or reads
  that line yet.
- `log` holds the write-ahead log that all regions share, in segments numbered in the
  order they were begun (twenty decimal digits).
- `regions/table` holds the regions there are and the chunks each holds, as of a place
  in the log.
- `regions/players` holds what the store keeps of every player who ever joined, as of a
  place in the log. A world in which the file was never needed has none.
- `regions` holds two files per region besides: `<region>.region`, with the highest
  epoch it was opened with and its entity ids, once it has been opened or was made by a
  split, and `<region>.state`, with its state as of its last checkpoint. `<region>` is
  the number of the region in decimal.
- `chunks` holds one file per stored chunk at chunk coordinates `x`, `z`, grouped into
  directories of 32×32 chunks (`rx = x >> 5`, `rz = z >> 5`). `<dimension>` is the
  world's dimension without its namespace (`overworld`, `the_nether`, `the_end`). A
  world has one dimension, and every world is the overworld so far.

Chunks are kept the same way whichever region they are in.

## Chunks

A chunk file, all integers big-endian:

| Field | Type |
|---|---|
| Format version | u8, 2 |
| Kind | u8, 5 |
| Chunk x, z | i32, i32 |
| Y of the lowest block | i32 |
| Tick at which the chunk was saved | u64 |
| Epoch of the region that saved it | u64 |
| **Generated**: the version of the generator the chunk is exactly as made by, or 0 | u16 |
| Flags | u8: bit 0 "with a failure"; the others 0 |
| Section count | u16 |
| Codec of the body | u8: 0 as it is, 1 zstd |
| Length of the body, as it is | u32, at most 8 MiB |
| Length of the body, as stored | u32 |
| The body, as stored | bytes |
| CRC-32 of everything before | u32 |

The body, before compression:

| Part | Encoding |
|---|---|
| Per section, bottom to top: blocks | palette length `n`, u16, 1 to 4,096; `n` × u16 state ids; if `n` > 1, 4,096 indices of `ceil(log2(n))` bits in u64 words, lowest bits first, none across two words, in the order `y << 8 \| z << 4 \| x` |
| … biomes | palette length `m`, u8, 1 to 64; `m` × u16 biome ids; if `m` > 1, 64 indices of `ceil(log2(m))` bits, packed the same way, in the order `y << 4 \| z << 2 \| x` of cells of 4×4×4 blocks |
| Block entities | count u32; then, by y, z, x: `x << 4 \| z` u8, y i16, type u16, length of the data u32 (at most 16 KiB), the data |
| Light | u8: 0 for none, 1 for present without `beside`, 2 for present with it (the writer writes 0 or 2; 1 is read from files written before `beside` was kept, as light that does not say). If present: `made_by` u16; for 2, `beside` u16, the version of the generator whose chunks the light was made beside, or 0; then for sky and then for block, per light section bottom to top: u8 0 absent, 1 dark, 2 full, or 3 followed by 2,048 bytes |
| Scheduled ticks | count u32; then, in order: kind u8 (0 block, 1 fluid), `x << 4 \| z` u8, y i16, id u16, delay i32, priority i8 |
| Marks | count u32; then, ascending: `x << 4 \| z` u8, y i16 |

A chunk has one light section more below its sections and one more above. The data of
a block entity is one NBT compound as the game writes it on the network, without the
keys `x`, `y`, `z` and `id`; no data stands for an empty compound.

- **The writer writes one form**: palettes ascending with nothing unused, lists in
  order, light that is all 0 or all 15 as `dark` and `full`. The body is compressed
  with zstd at level 3 if that makes it smaller.
- **The reader refuses what it cannot make sense of, and nothing else**: a wrong
  checksum, version or kind; a length that does not fit; an index beyond its palette; a
  state id, biome id or type that does not exist; a length over a bound; flags that
  mean nothing; a file whose position is not the one its name says. A palette that is
  not sorted or has an unused entry is read.
- **Block entities are made to fit the blocks on reading**: a position whose block has
  a block entity type and that has no entity gets one without data, and an entity
  where the blocks ask for none, or for another type, is dropped. The store logs a
  warning that names the chunk and how many there were. Counts and heightmaps are not
  stored; they are made from the blocks.
- The tick and the epoch are for whoever looks at what is stored; nothing reads them
  back. The epoch is the one the region was opened with by whoever saved the chunk:
  for a chunk a region saves, its owner's; for a chunk the store saves when a region
  is opened and its commits are put into the stored chunks, the epoch it is opened
  with; and 0 for a chunk the store saves when it makes a world over for another
  division, which no region does.

Saved chunks are written together when they are made durable, at a checkpoint, a
return, a flush, the opening of a region, or when 128 of them wait: every file under a
temporary name (`<x>.<z>.chunk.tmp`), all of those synced at the same time, each renamed
into place, and their directories synced at the same time
(`docs/adr/0018-a-checkpoints-chunks-written-together.md`). A file under a temporary
name that a crash left is never read, and is written over by the next save of its
chunk.

## The write-ahead log

All regions share one log, so that a single sync makes the commits of all of them
durable. It is a sequence of records, each framed as a u32 payload length, a u32 CRC-32
of the payload, and the payload. All integers are big-endian. Every payload begins with
the format version (u8) and the kind of record (u8):

| Kind | Record | Then |
|---|---|---|
| 1 | Block changes, of the worlds from before regions had a state, which were of format 1; never written | tick u64, epoch u64, changes |
| 2 | Commit | region u32, tick u64, epoch u64, changes, state length u32, state |
| 3 | Opened | region u32, epoch u64, restored u64 |
| 4 | Granted | region u32, tick u64, chunks |
| 5 | Returned | region u32, chunks |
| 6 | Absorbed | region u32, epoch u64, absorbed u32, tick u64, state |
| 7 | Split | region u32, epoch u64, tick u64, state, part u32, part epoch u64, chunks, part state |
| 8 | Commit with stay notes | the fields of kind 2, stays |

where `changes` is a count (u32) followed, per change, by i32 x, i32 y, i32 z and a u16
block state; `chunks` is a count (u32) followed, per chunk, by i32 x and i32 z; and a
`state` is a length (u32) followed by that many bytes. A payload is at most 64 MiB; a
longer one is read as a record that was cut off. A commit holds what one tick of a region changed: its block changes, in
order, and the region's own record of the change of its state, which the store does not
look into. The epoch is that of the owner that committed it. An opened record says that
an owner opened the region and was restored up to tick `restored`; a commit of that
region before it in the log with a later tick is not part of the region's history.

A commit that has stay notes is of kind 8, and one that has none is of kind 2, as it
was before there were notes: one record, so that a commit and its notes are on disk
together or not at all. A note is what a region says to the store about a stay of a
player (`docs/adr/0020-one-stay-per-player.md`, section 3). `stays` is a count (u32),
which is not 0, followed, per note, by:

| Field | Type |
|---|---|
| Kind | u8: 1 `Entering`, 2 `Has` |
| Player | u128 |
| Entity | i32 |
| *`Has` only:* hops | u32 |
| … the place | below |

A place is where a player is and what they hold:

| Field | Type |
|---|---|
| Position | f64 x, f64 y, f64 z |
| Yaw, pitch | f32, f32 |
| Flags | u8: bit 0 on-ground, bit 1 flying |
| Selected slot | u8 |
| Hotbar | nine times: u8 0 for an empty slot, or u8 1, i32 item, i32 count |

The floating-point numbers are big-endian like the integers. A build from before kind 8
cannot read such a record and does not start.

The store takes the notes of a commit when it takes the commit, and keeps of each
player what they make of the player's record ("The players"). No region sends a note
yet, so no record of kind 8 is written.

Kinds 4 to 7 are what decides which region holds which chunk and which regions there
are. `Granted`: the region holds each of the chunks from its tick `tick` on. `Returned`:
it no longer does. `Absorbed` is a merge: `state` is the whole state of `region` as of
`tick`, every chunk `absorbed` was granted is granted to `region` with that tick, every
area `absorbed` was pinned to is one `region` is pinned to, and `absorbed` is no region
any more. `Split` is a split: `part` is a new region with the whole state `part state`
as of `tick`, which holds `chunks` from that tick on and has `part epoch` as the highest
epoch it was opened with; `state` is the whole state of `region` as of `tick`. Each of
these has happened when its record is durable, and not before. A `Granted` or a
`Returned` has at most 65 536 chunks; a claim or a return of more is several records.

A process can die while appending, which leaves a partial record at the end of the
segment it was appending to. Reading a segment stops at the first record that is
incomplete or fails its checksum. Nothing is appended to a segment after that: a store
that starts appends to a new segment, and one that fails to write or sync cuts the
segment back to what was durable, syncs it so that what was cut off is durably gone,
and goes on in a new one.

A segment is removed once nothing in it is needed any more and every segment before it
is gone: no commit that a region is restored with, no record of a merge or a split that
is a region's latest whole state, nothing the table file does not have, and no stay note
the players' file does not have. Commits are
not removed one by one: one that a checkpoint covers is passed over because its tick is
not above that of the region's latest whole state. A segment that was removed can be
there again after a crash, as removing it is not made durable; it is read like any
other, and nothing in it counts.

## Region files

A region file is the format version (u8), the kind (u8, 1), the highest epoch the region
was opened with (u64), the first entity id of the region's block and the one beyond it
(i32, i32), and a CRC-32 of everything before (u32). A state file is the format version
(u8), the kind (u8, 2), the tick the state is as of (u64), the length of the state (u32),
the state, and a CRC-32 of everything before. Both are written under a temporary name,
made durable and renamed, and the directory is synced after the rename.

A region that is pinned or home has a block of entity ids, issued when it is first
opened; a region that was made by a split has none, and its file has 0 for both ids. A
region file that names a block is never removed, also when its region is gone, so that
no block is issued twice.

## The table of regions

`regions/table` is to the list of regions and their chunks what a state file is to a
region: all of it as of a place in the log, written so that the log before that place
can go. All integers big-endian:

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 3 |
| `from`: the first log segment whose records change this table | u64 |
| The next region id | u32 |
| The home chunk | i32 x, i32 z |
| The home region | u32 |
| The division the table was made from: count, then areas | u32, areas |
| Regions, in ascending order of their ids: count, then per region | u32 |
| … id | u32 |
| … the areas it is pinned to: count, areas | u32, areas |
| … its grants, ascending by x then z: count, then per grant x, z, tick | u32, (i32, i32, u64)… |
| Absorbed regions, oldest first: count, then per pair absorbed, into | u32, (u32, u32)… |
| CRC-32 of everything before | u32 |

An area is a u8 of flags (bit 0: it has a western end, bit 1: an eastern end) followed
by i32 `min_x` and i32 `max_x`, each 0 if absent: the chunks with `min_x <= x < max_x`,
whatever their z.

The table as the store has it is this file with the records of kinds 4 to 7 applied
that are in segments from `from` on, in the order of the log. Records in earlier
segments are in the file already. The file is written when the world is first started,
when it is made over for another division, and whenever a checkpoint leaves the first
segment of the log needed for nothing but the table: then `from` becomes the next
segment, and the segments before it go.

## The players

`regions/players` is to what the store keeps of each player what the table is to the
list of regions: all of it as of a place in the log
(`docs/adr/0020-one-stay-per-player.md`, sections 2 and 8). All integers big-endian:

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 4 |
| `from`: the first log segment whose stay notes are not in this file | u64 |
| `issued`: the highest entity id a stay was ever given, 0 if none was | i32 |
| Records, ascending by player: count, then per record | u32 |
| … player, stay, hops | u128, i32, u32 |
| … has a place | u8 0 or 1 |
| … the place, if it has one, as in a stay note of the log | |
| CRC-32 of everything before | u32 |

A record is of one player: the entity of the latest stay the store was told the
player was given, the highest number of hand-overs it was told of that stay, and where
the player was last, if a region has said.

The records as the store has them are this file with the stay notes applied that are in
commits in segments from `from` on, in the order of the log, but for a commit with a
tick above what a later `Opened` record of its region was restored up to: such a commit
is no part of the region's history, and neither are its notes. A world without the file
has no records, and every note of its log counts. The file is written whenever a
checkpoint leaves the first segment of the log needed for nothing but the notes in it,
after the table file has been looked at in the same way, and before a world that has a
record is made over for another division, which fails the start if the file cannot be
written. `from`
becomes the next segment each time, and **no segment is ever numbered below it**: a
store that starts with no segment left numbers its next one from the file's `from`.

## Regions

- **Who holds a chunk**: the region it is granted to; else the region that is pinned to
  an area which contains it; else nobody. Only the holder loads and saves a chunk.
- The store is started with a **division**: the areas of the pinned regions, and the
  chunk players enter the world in, the home chunk. Region `i` is pinned to area `i`.
  The home region is the pinned region whose area has the home chunk, or else a region
  made after the pinned ones and granted that chunk. A world is started with no
  pinned region unless its store is told `--pin`, which pins regions side by side.
- Every region has an id below "the next region id", and an id is never used again for
  a region that a split makes. A region that is in neither list of the table has gone
  for good. The store remembers the latest 4096 regions that were absorbed.
- When a store starts on a world whose table was made from **another division**, by
  other areas or with another home chunk, the world is made over: the block changes of every
  region's commits that count are applied to the chunks and made durable, an opened
  record with `restored` 0 is written for every region that had anything, the state
  files are removed, and then the table of the new division is written. Until that
  table is durable, a store that starts finds the old one and does all of it again.
- When a region is opened, a block change of its commits is applied to the stored chunk
  only if the region holds the chunk now and the commit's tick is above the tick it
  holds the chunk from (0 for a chunk it holds by being pinned): what it did to a chunk
  before is in the chunk as it gave it away, and has perhaps been built over.
- A region's **latest whole state** is its state file or, if its tick is higher, the
  record of the merge or the split it went through last. A state file is put in place
  only with a tick above it.

## What is saved when

- Every tick in which something changed, the region commits it: its block changes and
  the change of its state. The store appends what all regions committed meanwhile to the
  log, syncs it once, and only then answers each commit.
- A chunk is saved once the commits asked for before it are durable; a chunk on disk
  therefore never holds a change that is not committed.
- At a **checkpoint**, a region saves every loaded chunk of its own that has changed and
  hands the store its whole state as of a tick. Once the saves before it are durable,
  the store writes the state file, and the commits up to that tick are passed over from
  then on. Commits after it stay. Checkpoints happen every five minutes by default and
  when the server stops.
- When a region is opened, the store hands its owner the state file and the state of
  every commit with a later tick, in the order of their ticks, applies the block changes
  of those commits to the chunks they are in, and saves those chunks. The commits stay
  in the log until a checkpoint covers them. A chunk may have been saved with some or
  all of them in it already; applying all of them again in order gives the same result,
  because each change sets a block to a definite state.
- The first time a region is opened the store gives it a block of entity ids, which is
  the region's for good, with one exception: the home region is given the next block
  when it is opened with a block that has no id above the highest entity id a stay was
  ever given, so that the ids of stays never go back. The region file is written before
  the owner is answered.
- A chunk a region gives back is free once the saves the region asked for before are
  durable; only then is the `Returned` record written.
- When the store starts it leaves the log as it is, unless the world was divided
  otherwise (see "Regions").

Only the owner of a region gets anything done for it. An owner that opens the region
with the epoch of the current owner, or a higher one, replaces it; what the previous
owner sends from then on is dropped, and none of its commits is answered unless the new
owner was restored with it. An owner with a lower epoch than the highest the region has
been opened with is refused, also after the store was started again.

A group of commits, grants and returns that could not be written or synced is never
answered, and the owner of every region loses its handle, whether it wrote in the group
or not; each opens its region again and is restored from what is on disk. Until the
segment is durably cut back to what was good, the store opens no region and gives no
list of regions.

## Reading a damaged world

A chunk file with a wrong checksum is reported as an error and the chunk is not loaded;
so is one the reader refuses for another reason ("Chunks"). The region that asked for
it is told that it cannot be read, and leaves a hole in the world. It is deliberately
not replaced by a freshly generated chunk, which would hide the damage and overwrite it
at the next save. Block changes of commits that would be put into such a chunk when its
region is opened are not applied, and the file stays as it is.

## Not stored yet

Entities; players' inventories; of a chunk, which structures it belongs to, how long it
has been inhabited and when it was last updated.
