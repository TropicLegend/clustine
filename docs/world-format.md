# World format

How Clustine stores a world on disk. This describes format version 1 as implemented in
`crates/clustine-format` (encodings) and `services/worldstore` (files).

The format is not stable yet: it may change without a migration path until the first
release.

## Principles

- **Only changes are stored.** A chunk nobody has modified is not stored; it is
  generated again when needed. A world therefore records the generator settings it was
  made with and refuses to open with others.
- **Sections are content-addressed.** A chunk column is split into 16×16×16 sections,
  and each distinct section content is stored once, under the hash of its content.
  Identical sections, within a chunk or across the world, share storage.
- **Files are never modified in place.** Every file is written under a temporary name
  and renamed, so a reader sees it whole or not at all.

## Directory layout

```text
<world>/
  meta
  blobs/<first two hex digits>/<hash in hex>
  manifests/overworld/<rx>.<rz>/<x>.<z>.manifest
```

- `meta` is a text file of `key=value` lines: `format` (the format version),
  `data-version` (the Minecraft data version whose block state and biome ids the world
  uses) and `generator` (the generator settings).
- `blobs` holds the sections.
- `manifests` holds one file per stored chunk at chunk coordinates `x`, `z`, grouped
  into directories of 32×32 chunks (`rx = x >> 5`, `rz = z >> 5`).

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

## What is saved when

- When a chunk that has changed is no longer needed by anyone, it is saved and unloaded.
- When the server stops, every loaded chunk that has changed is saved.

Changes made since the last of these are lost if the server process is killed. A
write-ahead log that closes this gap is the next step of the roadmap.

## Reading a damaged world

A manifest with a wrong checksum, or a section whose content does not match its address,
is reported as an error and the chunk is not loaded. It is deliberately not replaced by
a freshly generated chunk, which would hide the damage and overwrite it at the next save.

## Not stored yet

Entities, players and their inventories, block entities, light, and biomes finer than a
section. There is no garbage collection of sections that no manifest refers to any more.
