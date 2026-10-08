# World format

How Clustine stores a world on disk. This describes format version 1 as implemented in
`crates/clustine-format` (encodings) and `services/worldstore` (files); see
`docs/adr/0008-durable-regions-and-resuming.md` for what the store promises.

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
  layout
  log/<segment>.wal
  regions/<region>.region
  regions/<region>.state
  blobs/<first two hex digits>/<hash in hex>
  manifests/overworld/<rx>.<rz>/<x>.<z>.manifest
```

- `meta` is a text file of `key=value` lines: `format` (the format version),
  `data-version` (the Minecraft data version whose block state and biome ids the world
  uses) and `generator` (the generator settings).
- `layout` holds the fingerprint of the layout the regions were last part of, in
  hexadecimal.
- `log` holds the write-ahead log that all regions share, in segments numbered in the
  order they were begun (twenty decimal digits).
- `regions` holds two files per region that has been opened: `<region>.region`, with
  the highest epoch it was opened with and its entity ids, and `<region>.state`, with its
  state as of its last checkpoint. `<region>` is the number of the region in decimal.
- `blobs` holds the sections.
- `manifests` holds one file per stored chunk at chunk coordinates `x`, `z`, grouped
  into directories of 32×32 chunks (`rx = x >> 5`, `rz = z >> 5`).

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

where `changes` is a count (u32) followed, per change, by i32 x, i32 y, i32 z and a u16
block state. A commit holds what one tick of a region changed: its block changes, in
order, and the region's own record of the change of its state, which the store does not
look into. The epoch is that of the owner that committed it. An opened record says that
an owner opened the region and was restored up to tick `restored`; a commit of that
region before it in the log with a later tick is not part of the region's history.

A process can die while appending, which leaves a partial record at the end of the
segment it was appending to. Reading a segment stops at the first record that is
incomplete or fails its checksum. Nothing is appended to a segment after that: a store
that starts appends to a new segment, and one that fails to write or sync cuts the
segment back to what was durable and goes on in a new one.

A segment is removed once none of its commits is needed any more and every segment
before it is gone. Commits are not removed one by one: one that a checkpoint covers is
passed over because its tick is not above that of the region's state file.

## Region files

A region file is the format version (u8), the kind (u8, 1), the highest epoch the region
was opened with (u64), the first entity id of the region's block and the one beyond it
(i32, i32), and a CRC-32 of everything before (u32). A state file is the format version
(u8), the kind (u8, 2), the tick the state is as of (u64), the length of the state (u32),
the state, and a CRC-32 of everything before. Both are written under a temporary name,
made durable and renamed, and the directory is synced after the rename.

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
- When the store starts it leaves the log as it is. When the first region is opened with
  another layout than the one in `layout`, the block changes of every region's commits
  are applied to the chunks, the commits are passed over and the state files removed
  before the new layout is written: the regions of one layout know nothing of those of
  another.

Only the owner of a region gets anything done for it. An owner that opens the region
with the epoch of the current owner, or a higher one, replaces it; what the previous
owner sends from then on is dropped, and none of its commits is answered unless the new
owner was restored with it. An owner with a lower epoch than the highest the region has
been opened with is refused, also after the store was started again.

A commit that could not be written or synced is never answered, and the owner of every
region that wrote to the log in that sync loses its handle, opens the region again and
is restored from what is on disk.

## Reading a damaged world

A manifest with a wrong checksum, or a section whose content does not match its address,
is reported as an error and the chunk is not loaded. It is deliberately not replaced by
a freshly generated chunk, which would hide the damage and overwrite it at the next save.

## Not stored yet

Entities, players and their inventories, block entities, light, and biomes finer than a
section. There is no garbage collection of sections that no manifest refers to any more.
