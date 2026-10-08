# World format

How Clustine stores a world on disk. This describes format version 1 as implemented in
`crates/clustine-format` (encodings) and `services/worldstore` (files); see
`docs/adr/0008-durable-regions-and-resuming.md` for what the store promises of commits
and states, and `docs/adr/0011-the-world-store-and-regions.md` for what it promises of
regions and the chunks they hold.

The format is not stable yet: it may change without a migration path until the first
release.

## Principles

- **Only changes are stored.** A chunk nobody has modified is not stored; it is
  generated again when needed. A world therefore records the generator settings it was
  made with and refuses to open with others.
- **Sections are content-addressed.** A chunk column is split into 16×16×16 sections,
  and each distinct section content is stored once, under the hash of its content.
  Identical sections, within a chunk or across the world, share storage.
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
  regions/<region>.region
  regions/<region>.state
  blobs/<first two hex digits>/<hash in hex>
  manifests/overworld/<rx>.<rz>/<x>.<z>.manifest
```

- `meta` is a text file of `key=value` lines: `format` (the format version),
  `data-version` (the Minecraft data version whose block state and biome ids the world
  uses) and `generator` (the generator settings).
- `log` holds the write-ahead log that all regions share, in segments numbered in the
  order they were begun (twenty decimal digits).
- `regions/table` holds the regions there are and the chunks each holds, as of a place
  in the log.
- `regions` holds two files per region besides: `<region>.region`, with the highest
  epoch it was opened with and its entity ids, once it has been opened or was made by a
  split, and `<region>.state`, with its state as of its last checkpoint. `<region>` is
  the number of the region in decimal.
- `blobs` holds the sections.
- `manifests` holds one file per stored chunk at chunk coordinates `x`, `z`, grouped
  into directories of 32×32 chunks (`rx = x >> 5`, `rz = z >> 5`).

A world from before the store kept a table of regions has a file `layout` instead of
`regions/table`, with the fingerprint of the layout its regions were last part of, in
hexadecimal. It is read once, when such a world is started (see "Regions" below), and
removed when the table is durable.

A world from before regions had a state has a log per region instead, `logs/<region>.wal`,
and one from before there were regions a single log, `wal` next to `meta`. Their records
hold block changes alone (kind 1 below). When such a world is opened, those changes are
applied to the chunks they are in, the chunks are made durable, and the old logs are
removed.

Chunks and sections are kept the same way whichever region they are in.

## Sections

The **canonical encoding** of a section, all integers big-endian:

| Field | Type |
|---|---|
| Format version | u8 |
| Biome | u16 |
| Palette length `n` | u16, 1 to 4096 |
| Palette | `n` × u16 block state ids, strictly ascending, each used at least once |
| Indices, only if `n` > 1 | u64 words with one palette index of `ceil(log2(n))` bits per block, lowest bits first, none spanning two words |

Blocks are in the order `y << 8 | z << 4 | x`. The rules on the palette make the
encoding unique: equal sections have equal bytes, whatever edits led to them.

The **address** of a section is the BLAKE3 hash of its canonical encoding.

The **stored file** is one codec byte followed by the payload: `0` for the canonical
encoding as is, `1` for the canonical encoding compressed with zstd. Because the address
is that of the uncompressed encoding, the compression can change without invalidating
any address.

A section has a single biome. Biomes varying within a section are not modelled yet.

## Chunk manifests

All integers big-endian:

| Field | Type |
|---|---|
| Format version | u8 |
| Chunk x, z | i32, i32 |
| Y of the lowest block | i32 |
| Tick at which the chunk was saved | u64 |
| Epoch of the region that saved it | u64 |
| Air biome | u16 |
| Section count | u16 |
| Per section, bottom to top | u8 flag: `0`, or `1` followed by the 32-byte address |
| CRC-32 of everything before | u32 |

A section with flag `0` is nothing but air in the air biome and has no file.

The epoch is always 1 for now. It will identify which owner of a region wrote the chunk
once regions can move between workers.

## The write-ahead log

All regions share one log, so that a single sync makes the commits of all of them
durable. It is a sequence of records, each framed as a u32 payload length, a u32 CRC-32
of the payload, and the payload. All integers are big-endian. Every payload begins with
the format version (u8) and the kind of record (u8):

| Kind | Record | Then |
|---|---|---|
| 1 | Block changes, from before regions had a state; only read | tick u64, epoch u64, changes |
| 2 | Commit | region u32, tick u64, epoch u64, changes, state length u32, state |
| 3 | Opened | region u32, epoch u64, restored u64 |
| 4 | Granted | region u32, tick u64, chunks |
| 5 | Returned | region u32, chunks |
| 6 | Absorbed | region u32, epoch u64, absorbed u32, tick u64, state |
| 7 | Split | region u32, epoch u64, tick u64, state, part u32, part epoch u64, chunks, part state |

where `changes` is a count (u32) followed, per change, by i32 x, i32 y, i32 z and a u16
block state; `chunks` is a count (u32) followed, per chunk, by i32 x and i32 z; and a
`state` is a length (u32) followed by that many bytes. A payload is at most 64 MiB; a
longer one is read as a record that was cut off. A commit holds what one tick of a region changed: its block changes, in
order, and the region's own record of the change of its state, which the store does not
look into. The epoch is that of the owner that committed it. An opened record says that
an owner opened the region and was restored up to tick `restored`; a commit of that
region before it in the log with a later tick is not part of the region's history.

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
is a region's latest whole state, and nothing the table file does not have. Commits are
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

## Regions

- **Who holds a chunk**: the region it is granted to; else the region that is pinned to
  an area which contains it; else nobody. Only the holder loads and saves a chunk.
- The store is started with a **division**: the areas of the pinned regions, and the
  chunk players enter the world in, the home chunk. Region `i` is pinned to area `i`.
  The home region is the pinned region whose area has the home chunk, or else a region
  made after the pinned ones and granted that chunk. Until regions follow players
  everywhere, the division is the stripes of the layout the cluster is started with.
- Every region has an id below "the next region id", and an id is never used again for
  a region that a split makes. A region that is in neither list of the table has gone
  for good. The store remembers the latest 4096 regions that were absorbed.
- When a store starts on a world whose table was made from **another division**, by
  other areas or with another home chunk, or on a world from before with another
  layout, the world is made over: the block changes of every region's commits that
  count are applied to the chunks and made durable, an opened record with `restored` 0
  is written for every region that had anything, the state files are removed, and then
  the table of the new division is written. Until that table is durable, a store that
  starts finds the old one and does all of it again. A world from before with the
  layout it is started with keeps its regions as they are, and only gets its table.
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
  the region's for good. The region file is written before the owner is answered.
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

A manifest with a wrong checksum, or a section whose content does not match its address,
is reported as an error and the chunk is not loaded. It is deliberately not replaced by
a freshly generated chunk, which would hide the damage and overwrite it at the next save.

## Not stored yet

Entities, players and their inventories, block entities, light, and biomes finer than a
section. There is no garbage collection of sections that no manifest refers to any more.
