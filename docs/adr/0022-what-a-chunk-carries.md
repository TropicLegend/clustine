# ADR-0022: What a chunk carries, and where light is made

- Status: **Accepted**, after an independent review against the code and against
  real chunks, whose fifteen findings are worked in or handed to the record on
  light at the edge (see "Review"). The last record of step G4 of
  [the terrain plan](../groundwork/terrain-plan.md), which the owner agreed on
  2026-10-10. It decides what a chunk is in memory, on disk and between services,
  what the edge's chunk packet is made from, and who makes light and what it means
  for a chunk's light to be valid. **How an edge keeps light right once blocks have
  changed is not decided here**: section 7 says why and what the record that
  decides it has to answer. It changes `crates/clustine-world`,
  `crates/clustine-format` (format version 2), `docs/world-format.md`,
  `docs/architecture.md`, the world store's files, `crates/clustine-protocol` (one
  addition), `services/edge` (`encode.rs`, one comparison in `fanout.rs`),
  `services/worker` (what a snapshot carries), and adds a crate
  `crates/clustine-light`. It rests on
  [ADR-0021](0021-generation-is-a-function.md) for who makes a chunk and when it is
  stored, and on [ADR-0019](0019-data-made-from-mojangs-jar.md) for what is known of
  a block state.
- Date: 2026-10-10

Where a figure or a statement is a **guess** it says so. Statements about the code
are of the tree at `b67a1d0`, but for the numbers ADR-0020 has taken, which are of
`0bfc2e9`. "The plan" is the terrain plan. **Every size of real terrain in this
record is measured**, by the review, over 101 chunks that the official server
made, at four places of one seed (below); the draft's guesses are gone.

**This record departs from the plan in one thing, on purpose: a stored chunk is one
file, and sections are no longer stored one by one under their hashes.** The plan's
list for G5 keeps the sections content-addressed and adds one blob a chunk. Section
3 has the measurements that speak against it once every chunk a region was handed
is stored.

## Context

### What a chunk is today

| What | Where |
|---|---|
| A chunk is its lowest y and a list of sections, and two chunks are equal if those are | `crates/clustine-world/src/chunk.rs:9-15` |
| A section is one block state, or 4,096 state ids of two bytes each (8,192 bytes), and **one biome** | `crates/clustine-world/src/section.rs:14-33` |
| "Not air" is "not the one state `blocks::AIR`", wherever it is counted or scanned for; cave air and void air count as blocks | `section.rs:56`, `68`, `121-122`; `chunk.rs:75` |
| Its heights are found by scanning for the highest such block, when asked | `chunk.rs:62-87` |
| There are no block entities, no light, no ticks, no marks in it | `chunk.rs:9-15` |
| Light is not kept: sky light is reckoned from the heights for each packet, full above the highest block and none below; block light is none | `crates/clustine-world/src/light.rs:1-6`, `27-62` |
| Between services a chunk travels as it is in memory: a mixed section is a sequence of 4,096 ids, written by postcard | `section.rs:135-147`; `crates/clustine-rpc/src/wire.rs:24-31` |
| It travels in three messages: the store's answer to a load, a region's save, a region's snapshot for an edge | `crates/clustine-rpc/src/messages.rs:290-294`, `374-377`, `159-169` |
| A message may be 16 MiB | `wire.rs:14` |
| An edge sends a chunk again to everyone who views it when a snapshot is not equal to what its replica holds | `services/edge/src/fanout.rs:2176-2190` |
| On disk a chunk is a manifest that names, for each section, the hash of its canonical encoding; each distinct section is a file of its own | `crates/clustine-format/src/manifest.rs:23-39`, `44-70`; `services/worldstore/src/chunks.rs:165-175` |
| The section's encoding has one biome, a sorted palette and packed indices; it is compressed with zstd at level 3 if that is smaller | `crates/clustine-format/src/section.rs:37-64`, `29`, `118-130` |
| "Not stored yet: … block entities, light, and biomes finer than a section. There is no garbage collection of sections" | `docs/world-format.md:282-285` |
| A world of another format version is refused first of all, with a sentence that names the setting and both values | `services/worldstore/src/local.rs:44-58`; `services/worldstore/src/lib.rs:84-89`, `306` |
| File kinds 1 to 3 are the region file, the state file and the table; ADR-0020 has taken **4** for the players' file and log record kind **8**, both in format 1 | `crates/clustine-format/src/region.rs:57-59`; ADR-0020, "In the log" and its section 14 |

### What the edge's chunk packet is made from today

| Part of the packet | Today | Where |
|---|---|---|
| Three heightmaps (motion blocking, the same without leaves, world surface) | one map for all three: the highest block that is not `AIR` | `services/edge/src/encode.rs:21-33` |
| Block states of a section | the 4,096 ids are read out and a palette is built anew for every packet | `encode.rs:46-52`; `crates/clustine-protocol/src/chunk.rs:86-124` |
| Block count, fluid count | the section's count of what is not `AIR`; fluids counted anew, water and lava blocks only, waterlogged blocks not | `encode.rs:53-71` |
| Biomes | one value for the section | `encode.rs:61` |
| Block entities | none | `encode.rs:41` |
| Light | `sky_light` over the chunk; block light empty wherever sky light is listed | `encode.rs:73-94` |
| When | when a player first needs the chunk, on the fan-out task; kept until the chunk changes | `services/edge/src/fanout.rs:132-153`, `2218-2229`, `2271-2279` |

For the classic flat world all of that equals the official server's packet (the
ignored comparison, `encode.rs:171-231`). For terrain none of it would.

### What the official server keeps of a chunk, measured

**The sample**: a world that the official server of 26.3 made, kept beside the
reference clones; of its region files the 101 chunks at status `full` (2,907
unfinished ones were left out), **at four places of one seed**. It has no ocean
monument, no village centre and nothing anyone built; a built-up place will have
wider palettes. Read with the reader of `docs/groundwork/terrain-g1.py`, never run;
state ids from the data generator's `blocks.json`, the flags of a state from the
committed `block_states.bin`. Sizes in this record's forms are arithmetic on those
forms with the measured palettes and arrays; the stored file is the body of
section 3 built byte for byte and compressed with the real `zstd -3`. KB are 1,000
bytes.

A stored chunk of the official server has the keys `sections`, `Heightmaps` (four
maps of 37 words: 256 heights of 9 bits), `block_entities`, `block_ticks`,
`fluid_ticks`, `PostProcessing`, `structures`, `isLightOn`, `Status`,
`InhabitedTime`, `LastUpdate`, `DataVersion` and its position; each section has
`block_states`, `biomes` and, where there is light, `SkyLight` and `BlockLight` of
2,048 bytes.

**Per chunk:**

| | min | p10 | median | mean | p90 | max |
|---|---:|---:|---:|---:|---:|---:|
| Sections that are not uniform | 8 | 8 | 9 | **9.1** | 10 | 11 |
| Sky light arrays stored | 2 | 2 | 3 | **2.8** | 3 | 4 |
| Block light arrays stored | 0 | 1 | 3 | **3.1** | 5 | 7 |
| Block entities | 0 | 0 | 0 | 0.1 | 0 | 3 |
| Scheduled block ticks | 0 | 0 | 0 | 5.1 | 11 | 143 |
| Scheduled fluid ticks | 0 | 0 | 0 | 1.0 | 1 | 28 |
| Marks (`PostProcessing`) | 0 | 0 | 2 | **59** | 228 | 804 |

- **Palettes of blocks**, over 2,424 sections: 1,506 uniform, all of them air (no
  section is all stone or all deepslate); of the 918 that are not, **857 have at
  most 16 states, 57 have 17 to 32, 4 have 33 to 42, none more**. The median
  palette has 9 states.
- **Palettes of biomes**: 2,029 sections of one biome, 385 of two, 10 of three.
- **Light**: of 2,626 sky slots 2,345 are absent, 144 are mixed arrays and **137
  are 15 throughout**; of 2,626 block slots 318 are arrays and the rest absent. No
  stored array is all zero: on disk the official server does not tell "dark" from
  "not listed".
- **Block entities**: 7 chests (84 to 98 bytes of data: `LootTable`,
  `LootTableSeed`, `components`), 3 spawners (206 to 208), 1 bell (15).
- **Marks**: 5,954 in 58 chunks; 4,396 lie on water and 325 on lava. **72 of the
  101 chunks have a mark or a scheduled tick.**
- **Sharing**: the 918 sections that are not uniform are 918 distinct ones.
- **Cave air** is in 11 sections.
- **Heightmaps**: the four rules of section 2, applied to the blocks, give the
  official server's stored maps in **all 103,424 columns**.

## Decision

**A chunk carries its blocks, its biomes in 64 cells a section, its block entities,
its four heightmaps, the version of the generator it is exactly as made by, the
light that was made for it for as long as no change has touched what light sees,
and the ticks and marks generation left for later. Sections are held packed, in
the layout the client's packet has. On disk a chunk is one file. Light is made by a
pure crate, `clustine-light`, by whoever makes a chunk, and travels with it. What
an edge does about light once blocks have changed is a record of its own.**

### 1. What a chunk carries

| Part | Granularity | In memory | Between services | On disk | Changed by |
|---|---|---|---|---|---|
| Blocks | 4,096 a section | packed (section 2) | as in memory | packed (section 3) | `Chunk::set` |
| Count of what is not air; count of fluids | a section | kept | no: counted on arrival | no | follows the blocks |
| Biomes | 64 cells a section (4×4×4 blocks) | one value, or 64 | as in memory | palette and packed indices | never after generation |
| Block entities | a position: type and data | a map by position | as in memory | a list | `Chunk::set` keeps them in step with the blocks; nothing else yet |
| The four final heightmaps | 256 columns each | kept, 2 KB | no: made on arrival | no | follows the blocks |
| **As generated**: the generator's version, or 0; and whether something failed while it was made (ADR-0021, section 5) | the chunk | a number and a bit | yes | yes | both cleared by the first change |
| **Light**: sky and block, with the version of what made it | a light section: absent, dark, full, or 2,048 bytes | kept while it is valid (below) | as in memory | as in memory | dropped by the first change that light can see |
| Scheduled ticks and marks from generation | positions | two lists | to a worker and the store; **not to an edge** | yes | taken by phase B; never by a block change |

**"Not air" is the table's `is_air`** (`StateProps::is_air`,
`crates/clustine-data/src/block_states.rs:146-148`) everywhere: in the count, in
the world-surface heightmap, in "a section is empty". Air, cave air and void air
are air. The measured heightmaps come out right only so.

**A change** is a call of `Chunk::set` that puts another state where one was. It is
the only way a chunk's blocks change, in the simulation
(`crates/clustine-sim/src/region.rs:1018-1027`), in the store when a region is
brought back (`services/worldstore/src/chunks.rs:587-588`) and in the edge's replica
(`services/edge/src/fanout.rs:2227-2228`).

**"As generated" and "its light is valid" are two things.**

- **As generated** is about the blocks: nothing has changed the chunk since a
  generator of that version made it. The store rests on it (ADR-0021, S4 and S8).
  Any change ends it.
- **The light is valid** means: the light the chunk carries is what the light
  rules of the named version give for this chunk's blocks **beside the neighbours
  it was made beside**. A chunk can know the first half and keeps it: it drops its
  light at the first change in which the old and the new state differ in anything
  light sees (the light they give, the light they take, whether and with which
  faces they stop light, whether the sky passes down through them:
  `light_emission`, `light_dampening`, `occludes`, `occludes_by_shape`, the set of
  occlusion faces, `propagates_skylight_down`, `block_states.rs:131-195`, `298-307`).
  A change of grass to dirt, of one ore to stone, of a door's state where its
  faces stay, keeps the light; water that flows into air does not. The second
  half, whether the neighbours are still what they were, no chunk can know, and
  section 7 hands it on.
- So a chunk can be changed and still carry valid light, and the measured fact
  that 72 chunks in 100 will be changed by their own marks and ticks once phase B
  honours them does not by itself take the light from them; flowing water does.

**Three rules hold for every chunk, and `Chunk` keeps them itself:**

- **I1.** Light is there only if no change that light can see was made since it
  was made. (A chunk may lack light for other reasons: a generator that makes
  none, a step before light exists.) The failure bit is set only where the
  version is not 0.
- **I2.** A position has a block entity exactly if the block of its state has a
  block entity type (`BLOCK_INFO[..].block_entity_type`,
  `crates/clustine-data/src/lib.rs:208-210`), and the entity is of that type.
- **I3.** Counts and heightmaps are those of the blocks.

**Not carried, and named**: the official chunk's `structures` (which starts and
references a chunk has: the game asks it whether a position is inside a fortress or
a monument, for which mobs spawn, and to find structures), `InhabitedTime` (local
difficulty) and `LastUpdate`. Nothing in this milestone needs them.

### 2. In memory: `clustine-world`

```rust
pub struct Chunk {
    min_y: i32,
    sections: Vec<Section>,
    /// By position, ordered by y, then z, then x.
    block_entities: BTreeMap<InChunk, BlockEntity>,
    heights: Box<Heights>,          // four maps of 256 `u16`, as today's one
    /// The version of the generator this chunk is exactly as made by; 0 otherwise.
    generated: u16,
    /// Something failed while it was made (ADR-0021). Only with `generated != 0`.
    with_failure: bool,
    light: Option<Light>,
    ticks: Vec<ScheduledTick>,      // in the order generation scheduled them
    marks: Vec<InChunk>,            // ascending, each once
}
```

**A section's blocks** are one of three forms, which are the three forms of the
client's paletted container (`crates/clustine-protocol/src/chunk.rs:25-48`,
`86-124`):

| Form | Holds | Bytes of indices | Of the 918 measured sections |
|---|---|---:|---:|
| Uniform | one state | 0 | (1,506 more are this) |
| Indexed, 4 bits | a palette of at most 16 states in the order they were first put there, and 4,096 indices packed into 64-bit words, lowest bits first, none across two words | 2,048 | 857 |
| Indexed, 5, 6, 7, 8 bits | the same, to 256 states | 2,736; 3,280; 3,648; 4,096 | 57; 4; 0; 0 |
| Direct | 4,096 state ids of 16 bits (the width `PaletteKind::blocks` gives for 26.3's 35,723 states), four to a word | 8,192 | 0 |

- `get` reads an index and looks it up. `set` looks the state up in the palette;
  a state that is not there is appended; a palette that is full moves the section
  to the next width, and beyond 256 states to the direct form. A section never
  moves back by itself, and a palette may hold states that are no longer used.
  The game keeps its own sections so; both are allowed in the client's container
  (**guess**; the comparisons decode values and never compare bytes,
  `encode.rs:205-217`, and the first real client after C5 is the judge).
- `Section::filled`, `from_states`, `get`, `set`, `states`, `uniform_state` keep
  their signatures. `non_air_count` keeps its name and counts by `is_air`.
  `fluid_count()` is new: the blocks whose state has a fluid (`StateProps::fluid`,
  `block_states.rs:270`). (**Guess** that this is what the client's count is; the
  official server does not store it, and W2's comparison of packets decides.)
  `biome(x, y, z)` and `biomes()` are new; `biome()` stays until C3 and is the
  first cell's (section 9).

**Biomes** of a section: one value, or 64 values indexed by `y << 4 | z << 2 | x` in
cells of four blocks (the client's order, `clustine-protocol/src/chunk.rs:169`).

**A block entity** is its type (the index into `BLOCK_ENTITY_TYPES`) and its data:
the bytes of one NBT compound as the game writes it on the network, without the
keys `x`, `y`, `z` and `id`, which the position and the type give. `clustine-world`
does not look into the data. No data at all stands for an empty compound.

- **A bound where it comes in.** The data of one block entity is at most 16 KiB,
  and the data of all of a chunk's together at most 1 MiB. `Chunk` has one way to
  put a block entity's data in, and it refuses more with an error. Whoever brings
  data in handles the refusal: generation's view and the import of an official
  world keep the entity without data and count it. (The measured data is 15 to 208
  bytes. Nothing a player does brings data in yet; when inventories come, their
  record meets this bound.) So no chunk can be made in a worker or a generator
  that the store's reader or a link would refuse for its size.

**`Chunk::set`**, when the state changes:

1. the section's blocks and its two counts;
2. the four heights of that column: raised if the new block counts for a map and
   lies at or above its height; found again by going down the column if the old
   block was the one the height rested on;
3. I2: if the new block has no block entity type, or another one than the old, the
   entity at the position is removed; if it has one and none of that type is
   there, one without data is put there. So a chest a player places is shown, and
   a chest a player breaks leaves nothing behind;
4. `generated` becomes 0 and the failure bit is cleared;
5. the light is dropped **if the two states differ in what light sees** (section
   1). That is a comparison of a few bytes of two rows of the table.

Ticks and marks are not touched.

**The four heightmaps**, from the block table (`block_states.rs:147`, `217-223`,
`270`): world surface is "not air"; the ocean floor is the first heightmap tag;
motion blocking is that tag or a fluid; motion blocking without leaves is the
second tag or a fluid. ADR-0019 had these as a guess. **They are now checked**:
they give the official server's maps in all 103,424 columns of the sample.

**Light**: `Light { made_by: u16, sky: Vec<SectionLight>, block: Vec<SectionLight> }`.

- `made_by` is the version of the light rules that made it: `clustine_light::VERSION`,
  or the one value `Light::IMPORTED` for light the official server made (W4).
- `sky` and `block` each have one entry for the section below the chunk, one for
  each section and one for the section above, bottom first, as `sky_light` returns
  them today (`light.rs:25-27`).
- `SectionLight` has four cases: `Absent` (the packet lists it in neither mask),
  `Dark` (all 0), **`Full`** (all 15), `Levels` (2,048 bytes, four bits a block,
  indexed by `y << 8 | z << 4 | x`, the even index in the low half). An array that
  is all 0 is `Dark` and one that is all 15 is `Full`, always, so that equal light
  is equal. `Full` is written as an array only in the client's packet; 137 of the
  281 sky arrays of the sample are full, 2.8 KB of the 12.1 KB of light a chunk.

**A scheduled tick** is a position in the chunk, a block id or a fluid id, a delay
and a priority, as the official server keeps them in `block_ticks` and
`fluid_ticks`. **A mark** is a position whose block is to take its shape, or whose
fluid is to flow one step, when the chunk is first ticked (`PostProcessing`). They
are carried and stored from the start and used by phase B (the plan's B6, under
ADR-0023). They are not few: a mean of 59 marks and 6 ticks a chunk, 0.24 KB.

**When two chunks are equal, said field by field**, because the edge's resume turns
on a comparison of chunks:

- `==` on `Chunk` compares everything a chunk carries: the lowest y, the sections
  **by content** (two sections are equal if they hold the same blocks and biomes,
  however they came to, as today, `section.rs:35-45`), the block entities, the
  version and the failure bit, the light with its version, the ticks and the
  marks. Not the heights and counts, which follow.
- **`Chunk::shows_the_same(&other)`** compares what a client is shown: the lowest
  y, the sections by content, the block entities. Nothing else.
- **The edge uses `shows_the_same`**, where it uses `!=` today to decide whether
  everyone who views a chunk is sent it again (`fanout.rs:2177`). Otherwise the
  day phase B takes a chunk's marks in the region, every later snapshot of 58
  chunks in 100 would differ from the replica's and be sent again, whole, at every
  resume, hand-over, merge and split. Whether a difference in light alone is
  sent, and how, is section 7's record's.
- **A snapshot for an edge carries no ticks and no marks**: the worker hands the
  edge `chunk.for_viewers()`, a copy without them, where it clones the chunk today
  (`services/worker/src/lib.rs:2084-2090`).

**What it costs in memory** (the review's arithmetic on these forms over the
sample; the last two rows less the 137 full arrays):

| | min | median | mean | p90 | max |
|---|---:|---:|---:|---:|---:|
| Today: blocks alone, one biome | 65.5 | 73.7 | **74.5** | 81.9 | 90.1 |
| This record: blocks and biomes | 16.6 | 19.6 | **19.8** | 22.3 | 24.4 |
| This record: light, without `Full` | 4.1 | 12.3 | 12.1 | 16.4 | 20.5 |
| This record: a chunk, without `Full` | 25.6 | 35.0 | 34.8 | 39.3 | 44.1 |
| **This record: a chunk** | | | **32.0** | | |

A flat chunk is one section of four states and two sky arrays, one of them full:
about 6 KB, against 8.2 today.

### 3. On disk: format version 2

`FORMAT_VERSION` becomes 2 (`crates/clustine-format/src/lib.rs`). It is written
into everything stored, so every file and every record of the log changes its
first byte.

**The numbers, against ADR-0020**, which is being built at the same time:

| Number | Where | Taken | This record |
|---|---|---|---|
| File kind | the second byte of a file of `clustine-format` | 1 region, 2 state, 3 table; **4 the players' file** (ADR-0020, in the tree since `0bfc2e9`) | **5: a chunk** |
| Log record kind | `clustine-format`'s log | 1 to 7; **8 a commit with stays** (ADR-0020) | none |
| `FORMAT_VERSION` | `clustine-format` | 1 | **2** |
| `STATE_FORMAT` | `services/worker` | 4, to be 5 (ADR-0020, R1.5) | not touched: a region's state holds no chunk |
| The wire number | the replicas plan's R0.3; not in the tree yet | | raised once by C1 and once by C2, if it is there by then |

- **ADR-0020's file and record are of format 1 and go into format 2 as they are**,
  with the first byte 2. There is one format 1 and one format 2: ADR-0020 adds
  kinds within a format and raises no format number, and this record raises the
  format number and takes a kind.
- **Whichever lands second takes the next number.** If a later step of ADR-0020
  needs a new file kind or record kind after C3, it takes the lowest free one (6
  for a file, 9 for a record), in whatever format is current then. If ADR-0020
  ever needs to raise `FORMAT_VERSION` and C3 has landed, it is 3; if it lands
  first, C3 is the next one. The same for the wire number: each step that changes
  the shape of a message raises it by one from whatever it is, in the order the
  steps land on `main`; a step that lands before R0.3 has nothing to raise. The
  constants are `FORMAT_VERSION`, the kinds in `clustine-format/src/region.rs` and
  `log.rs`, `STATE_FORMAT`, and R0.3's wire number.
- A world that a build between `0bfc2e9` and C3 made is of format 1 and is
  refused by C3 like any other (section 9).

**A stored chunk is one file**, `chunks/<dimension>/<rx>.<rz>/<x>.<z>.chunk`, where
`<dimension>` is the world's dimension without its namespace (`overworld`,
`the_nether`, `the_end`; a world has one, ADR-0021 section 8) and `rx = x >> 5`,
`rz = z >> 5` as today. It is written under a temporary name, made durable and
renamed, as a manifest is today. `blobs/` and `manifests/` are gone.

All integers big-endian:

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

**The body**, before compression:

| Part | Encoding |
|---|---|
| Per section, bottom to top: blocks | palette length `n`, u16, 1 to 4,096; `n` × u16 state ids; if `n` > 1, 4,096 indices of `ceil(log2(n))` bits in u64 words, lowest bits first, none across two words (today's encoding without its version byte and its biome, `clustine-format/src/section.rs:46-62`) |
| … biomes | palette length `m`, u8, 1 to 64; `m` × u16 biome ids; if `m` > 1, 64 indices of `ceil(log2(m))` bits, packed the same way |
| Block entities | count u32; then, by y, z, x: `x << 4 \| z` u8, y i16, type u16, length of the data u32 (at most 16 KiB), the data |
| Light | u8: 0 for none, 1 for present. If present: `made_by` u16; then for sky and then for block, per light section bottom to top: u8 0 absent, 1 dark, 2 full, or 3 followed by 2,048 bytes |
| Scheduled ticks | count u32; then, in order: kind u8 (0 block, 1 fluid), `x << 4 \| z` u8, y i16, id u16, delay i32, priority i8 |
| Marks | count u32; then, ascending: `x << 4 \| z` u8, y i16 |

- **The writer writes one form**: palettes ascending with nothing unused, lists in
  order, arrays that are all 0 or all 15 as `dark` and `full`. The tests pin its
  bytes.
- **The reader refuses what it cannot make sense of, and nothing else**: a wrong
  checksum, version or kind; a length that does not fit; an index beyond its
  palette; a state id, biome id or type that does not exist; a length over a
  bound. A palette that is not sorted or has an unused entry is read. (Canonical
  bytes were what made a hash an address. With a tick and an epoch in every file
  no two files are equal anyway, and a strict reader would call a chunk
  unreadable that has all its blocks.)
- **I1 to I3 are made to hold on reading**, not demanded: heights and counts are
  made from the blocks; a position that I2 wants an entity for and that has none
  gets one without data, and an entity where I2 wants none is dropped, each with a
  warning that names the chunk.
- The body is compressed with zstd at level 3 if that makes it smaller, as a
  section is today (`section.rs:118-130`).
- A uniform section is seven bytes of the body, and an empty chunk is a file of
  under 200 bytes.

**Why one file, measured.**

| For a chunk of the sample | min | median | mean | p90 | max |
|---|---:|---:|---:|---:|---:|
| The body before compression | 21.5 | 29.8 | 29.7 | 34.1 | 39.7 |
| **This record's file, zstd 3** | 3.3 | 5.1 | **5.1** | 6.2 | 8.6 |
| the same without light | 3.2 | 4.6 | 4.6 | 5.4 | 7.0 |
| **This record's file on blocks of 4 KiB** | 4.1 | 8.2 | **7.9** | 8.2 | 12.3 |
| The plan's shape: files a chunk | 10 | 11 | **11.2** | 12 | 13 |
| The plan's shape: bytes | 3.8 | 5.4 | 5.6 | 6.6 | 9.2 |
| **The plan's shape on blocks of 4 KiB** | 41.0 | 45.1 | **45.9** | 49.2 | 53.2 |
| The official server's own entry (zlib) | 3.6 | 5.2 | 5.4 | 6.7 | 11.2 |

"The plan's shape" is today's section encoding with a biome palette, each distinct
section a file of its own, one more blob for block entities, light, ticks and
marks, and a manifest. A section file of it is 471 bytes at the median.

| For a world of a million chunks that regions were handed | One file a chunk | Sections under their hashes (the plan) | The official server |
|---|---:|---:|---:|
| Files | 1.0 million, in about 1,000 directories | 11.2 million | about 1,000 |
| Bytes of content | 5.1 GB | 5.6 GB | 5.4 GB |
| Bytes taken on blocks of 4 KiB | **7.9 GB** | **45.9 GB** | 5.4 GB |
| Inodes (ext4 makes one for every 16 KB of a volume by default) | fit a volume of 16 GB | **need a volume of 180 GB**, or run out with space left | |
| Files in a write of 128 pending chunks | 128, in one round | about 1,300, then 128 manifests: two rounds, the first in about twenty waves of 64 | |
| Shared by content addressing | | nothing: 918 of 918 sections distinct | |
| Left behind when a chunk is saved again | nothing: the rename replaces the file | the old sections, for good (`world-format.md:285`) | |
| Written when one block changes | the chunk: 5.1 KB, two blocks of the disk | a section and a manifest: two files of one block each | |

The owner was told that storing what was shown costs "disk like vanilla's (a guess:
5 to 15 KB a chunk)" (the plan's question 2). One file is 7.9 KB on an ordinary
disk. Sections as files would be **5.8 times** that, and would run a volume out of
inodes long before it is full. ADR-0018 syncs the files of a round at the same
time on up to 64 threads (`services/worldstore/src/disk.rs:82-97`), so what the
plan's shape costs in time is a second round and twenty waves where one file
needs two, not a factor on syncs. A block change costs the disk the same either
way. What one file gives up: a flat world no longer stores its one section once.

**What it changes in the store** (`services/worldstore/src/chunks.rs`):
`FileChunks::sync` writes one round, the chunk files, where it writes two today
(`chunks.rs:235-280`); the set of section files that may not be durable (`unsure`,
`chunks.rs:88-92`) and the rule that a manifest names only durable sections go,
because a chunk file names nothing. Everything ADR-0018 says of a round, of
`pending` and of a failed sync holds for the one round.
`StoreError::MissingSection` (`services/worldstore/src/lib.rs:81`),
`clustine_format::Hash` and `blake3` lose their use and go.

**`docs/world-format.md` then says:**

- Head: "This describes format version 2."
- Principles: ADR-0021's two (what a region was given is stored; a world takes a
  newer generator) in place of "Only changes are stored"; "**A chunk is one file**,
  checksummed, written whole or not at all" in place of "Sections are
  content-addressed"; the other two stand.
- Directory layout: `chunks/<dimension>/<rx>.<rz>/<x>.<z>.chunk` in place of
  `blobs` and `manifests`; `meta` has `generator-version` besides.
- "Sections" and "Chunk manifests" become one part, "Chunks", with the two tables
  above and what the reader refuses.
- The paragraphs on worlds from before the table and from before regions
  (`world-format.md:55-64`) and the rules for making such a world over go: those
  worlds are of format 1.
- "Reading a damaged world": "A chunk file with a wrong checksum is reported as an
  error and the chunk is not loaded", for the rest as it is.
- "Not stored yet: entities; players' inventories; of a chunk, which structures
  it belongs to, how long it has been inhabited and when it was last updated."
  The sentence on garbage goes.

`docs/architecture.md:118-125` ("where the format is going": sections by hash,
snapshots that copy manifests) and the head of `crates/clustine-format/src/lib.rs`
describe sections and manifests and are rewritten in the same commit.

### 4. Between services

A chunk travels as it is in memory, written by postcard as today: per section the
form, the palette and the words **as bytes** (postcard would write a `u64` as a
variable-length integer); the biomes; the block entities; the version and the
failure bit; the light; ticks and marks, but for a snapshot. **Not** the counts and
not the heightmaps, which the receiver makes (as it counts what is not air today,
`section.rs:161-180`). The receiver checks what it is given (every index within its
palette, every length and bound) and makes I1 to I3 hold as the file's reader
does; a chunk that cannot be made sense of is a message that cannot be read, as
any other is today (`wire.rs:34-36`).

The three messages keep their shape (`messages.rs:159-169`, `290-294`, `374-377`).

| For a chunk of the sample | min | median | mean | p90 | max |
|---|---:|---:|---:|---:|---:|
| Today (postcard, blocks alone) | 49.0 | 69.8 | **69.1** | 72.8 | 77.3 |
| This record, without `Full` | 23.2 | 32.6 | 32.3 | 36.8 | 41.6 |
| **This record** | | | **29.5** | | |

- A first view of 329 chunks is 22.7 MB a hop today and **9.7 MB** in this form
  (10.6 MB before `Full`).
- A player who flies at 21.6 blocks a second brings about 28 chunks a second:
  0.8 MB/s a hop. A hundred such players are 80 MB/s a hop, which is a gigabit
  link.
- The limit of a message, 16 MiB (`wire.rs:14`), is far away: the largest chunk
  this record allows is 8,192 bytes for each of 24 sections, the light and 1 MiB
  of block entity data, under 1.3 MiB.
- The links' queues are counted in messages, not bytes: 16,384 for a link
  (`bin/clustine/src/lib.rs:41`), 480 MB of chunks at worst, half of what today's
  form would allow; the edge's one queue for all regions holds 1,024
  (`services/edge/src/fanout.rs:55`), 30 MB.

Messages are not compressed (choice 3).

### 5. The edge's chunk packet

| Part | Needs | Made from |
|---|---|---|
| Three heightmaps | the real ones | the chunk's maps, packed as today (`encode.rs:24`) |
| Block states | a paletted container | **the section's palette and words as they are**: the forms of section 2 are the packet's. `clustine-protocol` gets a way to write a container from its palette and words; reading stays as it is |
| Block count, fluid count | | the section's two counts |
| Biomes | a container of 64 | the section's biomes, through `PalettedContainer` as today |
| Block entities | position, type, data | the chunk's positions and types, **and no data** (below) |
| Light | masks and arrays | the chunk's light where it has any, with `Full` written as an array of 15s; otherwise today's `sky_light`. From W3 on, what section 7's record says |

**Nothing of a block entity's data is sent to a client until its type is known to
need something.** The edge keeps a list of block entity types and, for each, the
keys of its data that a client is sent; **the list starts empty**, so every block
entity goes out with its position and type and without data. A chest shows as a
chest either way. Every generated chest carries `LootTable` and `LootTableSeed`
(measured); sent as stored, every client would be told the seed of every unopened
chest in view, and later every inventory. What the official server sends with a
chunk for each type is in Mojang's code and not in its data; W4's comparison of
packets for an imported official world shows it type by type (a sign's text, a
banner, a head, a spawner's entity are expected), and a type is put on the list
from that comparison and from nothing else.

The packet is still made on the fan-out task when first needed and kept until the
chunk changes (`fanout.rs:137-153`). It no longer builds palettes or scans columns;
it copies. It is **32.5 KB** at the mean before compression (23.8 to 42.3), against
a few kilobytes for a flat chunk today, and each connection compresses the same
bytes again (`services/edge/src/lib.rs:56-57`): 329 times for a join. Whether the
packet is made and compressed once, off the fan-out task, belongs with the threads
that section 7's record gives light, and is decided there by W3's measurement.

### 6. `clustine-light`

Pure, depending on `clustine-world` and `clustine-data` and nothing else:

```rust
/// The version of the rules in this crate. Raised whenever any chunk's light
/// would come out otherwise. Light says which version made it (`Light::made_by`).
pub const VERSION: u16 = 1;

/// The chunk whose light is wanted and the chunks around it.
pub struct Around<'a> {
    pub centre: &'a Chunk,
    /// North-west, north, north-east, west, east, south-west, south, south-east.
    /// One that is not there gives no light and takes none.
    pub neighbours: [Option<&'a Chunk>; 8],
    /// Whether the dimension has a sky.
    pub sky: bool,
}

/// The light of the centre chunk, as the game's rules give it for these blocks.
pub fn light(around: &Around<'_>) -> Light;
```

- It ports the **rules** of the engine (`S263:steel-core/src/chunk/light/`, SteelMC's
  26.3 branch): what a state gives and takes, when two faces between two blocks
  stop light, where the sky comes straight down; all from the fields section 1
  names. Not its queues: one chunk from nothing is a flood from its sources.
- **The light of a chunk is a function of the blocks of its three by three.** A
  level falls by at least one a step and starts at 15, so a path that ends in the
  centre chunk is at most 15 blocks long and never leaves the nine chunks; whether
  a block has full sky light from above depends on its own column alone. (The
  review went over the argument and found it sound; W1's comparison with the
  official server is what checks it.)
- Which sections come out `Absent`, `Dark`, `Full` or `Levels` is the official
  server's rule for its packet, which today's `sky_light` has for the flat world
  (`light.rs:27-62`) and W1's fixtures give for terrain.
- No clock, no thread, no hash map. The same chunks give the same light on every
  machine. A change to it that changes any generated chunk's light raises
  `VERSION` and the generator's version (ADR-0021, section 8).

**Who calls it:**

| Who | When | With |
|---|---|---|
| The generation service | once for each chunk it finishes (ADR-0021, the task `Lit`) | the nine finished chunks; that is why 409 chunks are finished for a view of 329 |
| The flat generator | once: every chunk has the same | its chunk and eight like it |
| The import of an official world (W4) | never: it takes the light the official server stored, as `Light::IMPORTED` | below |
| An edge | as section 7's record says | its replica |
| The store, a worker, the simulation | **never** | |

**What the import has to derive.** The official server's files have two cases for
a light section, an array or nothing, where a chunk here has four. An array that
is all 15 is `Full` and one that is all 0 is `Dark`. A slot without an array is
`Dark` or `Absent`, and which is derived: for sky light, `Absent` above the
highest array and `Dark` below the lowest (**guess**, from how today's `sky_light`
lays out the flat world); for block light, `Dark` wherever the sky's slot is not
`Absent`, as `encode.rs:86-87` does today. W4's comparison of packets with the
official server's decides, and W1's fixtures give the same rule to
`clustine-light`.

**The light travels with the chunk** for as long as it is valid in the chunk's own
half of the meaning (I1): in the store's file (0.5 KB of the 5.1), in the answer
to a load, in the region's memory, in the snapshot.

### 7. Light at the edge: a record of its own, before W3

The draft of this record decided it here, in seven rules (E1 to E7) and a step
C6: carried light is sent while a chunk and its neighbours in the replica are as
generated; otherwise the edge computes light from its replica on threads of its
own when a chunk is about to be sent, never at a block change; a late neighbour
makes a job and a light update. **The review's findings 1, 2, 3, 5 and 8 change
those rules themselves**, the first of them with a measurement nobody had. So
that part is taken out of this record, and **decided in a record of its own,
written and reviewed before step W3, by the main session.** W3 is not delegated
(the plan).

What this record fixes for it, and it builds on: light is carried with its version
and is a thing apart from "as generated" (sections 1, 2); `clustine-light` is pure
and has a version (section 6); the edge compares what is shown and not light
(section 2); until that record is built the edge sends a chunk's own light where
it has any and today's `sky_light` otherwise (step C5), and sends nothing about
light at a block change, as today.

What it keeps from the plan and from the draft, unless it finds reason against:
light is never computed on the fan-out task; nothing is computed for chunks whose
neighbourhood is untouched; no service but an edge computes light for changed
chunks; the edge does not ask for a wider ring of chunks.

**What that record has to answer, with the reviewer's proposals:**

1. **(Finding 1.) "As generated" does not outlast a chunk's first tick once phase
   B honours marks and ticks.** 72 of 101 chunks carry a mark or a tick, and 4,721
   of 5,954 marks sit on water or lava that is to flow one step. Flowing is a
   change that light sees. After B6 most chunks have lost their carried light
   before a player sees them, a rule like E1 passes for hardly any chunk, and the
   edge computes light for nearly every chunk at every join and every step: 329
   computations a join, on a quarter of the processors, one to two seconds by the
   draft's own guess of a few milliseconds a chunk. And it does not settle: while
   water flows near, every step makes up to nine jobs stale, and a chunk that is
   held back until its light is current is held back for as long.
   *Proposed:* (a) keep the light an edge computed **with the replica's chunk for
   as long as it is in the replica**, brought up to date after changes, instead
   of making it at every sending; (b) take a result whose basis is older only by
   block changes: build the packet from the job's own snapshot of the chunk, send
   it, and send the block updates since after it; (c) drop carried light only for
   a change that light sees (**done in this record**, sections 1 and 2); (d) do
   light jobs nearest a waiting player first; (e) measure W3's exit on a world in
   which seven chunks in ten are changed, with a figure that fails it ("more than
   *n* ms of light for a view of 329"). The record should also weigh what the
   review did not propose: whether the region, which makes the changes of B6,
   should make them before the chunk is first handed to anyone, so that what an
   edge first sees is settled.
2. **(Finding 2.) A player who has the neighbour of a changed chunk, and not the
   changed chunk, keeps wrong light with nothing to put it right.** Player A has
   N at the rim of their view; B places a torch in C, just beyond, within 15
   blocks of N; A's client has no C and gets no block update; A walks closer and
   is sent C, right; N was sent before, and no rule of the draft makes a job for
   it. A sees the torch's light stop at the chunk border, with two players for
   good. The owner's own try ("a second client comes from far away") can hit it.
   *Proposed:* a standing rule in place of triggers: **a chunk that some player
   has must either pass the test for carried light against the replica as it is
   now, or have been shown a current computed light.** Whenever a replica chunk's
   revision rises or a chunk enters, it and its eight neighbours that some player
   has are marked to be looked at; the marks are worked off gathered (once a chunk
   has been still for some ticks, at the latest after a second), one job each, and
   what differs is sent. That also ends the reliance on the client lighting a
   block change by itself, and bounds the work by chunks a second, not blocks.
3. **(Finding 3.) A view that arrives chunk by chunk makes up to nine
   computations and eight light updates for every changed chunk.** Snapshots come
   one a message and each is offered to its viewers at once
   (`fanout.rs:2165-2194`). A changed chunk that arrives before its neighbours is
   either computed again and again as they trickle in, or sent with light that
   lacks everything from the missing neighbours and corrected up to eight times,
   12 KB and a rebuild at the client each time: borders that flicker during a
   join. *Proposed:* a chunk is not ready while one of its eight neighbours is
   **wanted by a player who waits for the chunk and not yet in the replica**
   (`PlayerView::wanted`, `fanout.rs:194`); only neighbours outside that player's
   view count as missing; a neighbour that does not come releases the chunk after
   the edge's patience (`region_patience`, `fanout.rs:127`). One job a chunk. A
   test with that order of events and a count of jobs and updates.
4. **(Finding 5.) Carried light across two generator versions, and after a fix to
   the light rules, would be sent as it is for good.** A chunk of version 1
   beside one of version 2 was lit beside a neighbour that no longer exists; a
   chunk lit by rules that have since been fixed keeps its light; an imported
   chunk carries the official server's light, made beside whatever stood next to
   it then. *Proposed:* carried light is sent as it is only if it was made by
   **the version of the light rules the edge itself has**, and every neighbour in
   the replica is as generated by the same generator version; anything else is
   computed. (`Light::made_by`, `clustine_light::VERSION` and `Light::IMPORTED`
   are in this record for that.)
5. **(Finding 8.) Four things the draft left open.** Revisions must never come
   again: a replica entry is removed when its last viewer goes
   (`fanout.rs:2108-2111`) and made anew, so a counter of the entry can reach an
   old value; *one counter for the whole edge*. A job that fails (a panic in
   `clustine_light::light`, a thread that died) must not leave a chunk that is
   never sent; *say what is sent then and log it*. A batch with nothing in it must
   not be sent when every pending chunk waits for light (`fanout.rs:2253-2286`). A
   job for a chunk that has left the replica, whether the queue of results is
   bounded, and the copies: carried light, computed light, what was shown and the
   light inside the kept packet should be one `Arc`, not four times 12 KB.
6. **(From finding 11.) A light update cannot say `Absent`.** From memory of the
   client, a section named in neither mask is left as the client has it; where a
   section was an array and is now absent (the top of a tower was taken down),
   an update leaves the old array there. *Proposed:* send such a section as an
   array of 15s, or say that it is not sent.
7. **(From finding 14.) The packet is compressed once for every connection.**
   Making and compressing a chunk's packet once, off the fan-out task, is the
   larger gain at a join, and belongs with the threads light gets.
8. **Not checked by anyone, and that record's to settle with a real client**:
   that the client lights a block change by itself; what it does with a section a
   light update does not name; how long `clustine_light::light` takes for a chunk
   of real terrain (it is built in C4, before that record is written, so that it
   can be measured).

### 8. What the simulation knows of it

Nothing new. It sets blocks through `Chunk::set` and reads them through
`Chunk::get` (`crates/clustine-sim/src/region.rs:1012-1027`), and holds chunks as
the store gave them (`region.rs:307-309`; `crates/clustine-sim/src/api.rs:402`).

- It never reads light, biomes, block entities, ticks or marks. W6 reads one height
  for where a player stands; phase B takes the ticks and marks under a record of
  its own.
- "As generated" is cleared, light is dropped where it must be, heights, counts
  and block entities are kept, all inside `Chunk::set`, as a function of the chunk
  and the three arguments. No hash map, no clock, no I/O; the block entities are
  in a `BTreeMap`. The history of a section shows only in the bytes of its
  palette, never in `get`. So a tick stays a function of the region and its
  inputs, and `crates/clustine-sim/clippy.toml` needs no new line.
- What `set` costs: a palette of at most 16 to search in 93 of 100 sections; a
  repack of 4,096 indices when a 17th state comes; one column to walk when its top
  block goes; one comparison of two table rows for the light.
- A region's state and its commits hold no chunk (a change is a position and a
  state), so neither grows and `STATE_FORMAT` is not touched.
- A worker saves a chunk only when it has unsaved changes
  (`services/worker/src/lib.rs:2204-2217`), and such a chunk is no longer as
  generated. Chunks as generated are stored by the store alone (ADR-0021, S2).

### 9. Migration, and the steps

**There is no migration.** Worlds are thrown away until the first release
(`docs/world-format.md:9-10`). A store that finds a world whose `meta` says
`format=1` stops before it touches anything, with:

```text
This world was made by an earlier Clustine (world format 1; this server writes
format 2). Worlds are not carried over before the first release: move <directory>
away or name another, and a new world is made.
```

It is `local::prepare`'s check of `format` (`local.rs:44-58`), the first thing the
store does with a world (`services/worldstore/src/lib.rs:306`), with a sentence of
its own. Nothing in the directory is read further, changed or removed. With it,
the code that carries over worlds from before regions (`local.rs:76-133`) and
makes over a world with a `layout` file can never run on a world this server
opens; whether it is removed is not this record's.

**M0, the measurement, is done**: the review made it, and its numbers are in this
record. None of the three limits the draft set for it is reached (5.1 KB stored
against 20; 32 KB in memory against 100; no direct section against one in a
hundred). No server has to be run.

**The steps.** Every commit leaves the four commands of `CLAUDE.md` green and the
flat comparison with the official server equal.

| # | Step | In the plan | What holds afterwards, and what the step must do for that | Delegated? |
|---|---|---|---|---|
| C1a | **"Not air" is `is_air`**: `section.rs:56`, `68`, `121-122` (the count), `chunk.rs:75` (the scan), and through `non_air_count` the manifest's "no file" (`manifest.rs:52`) and `light.rs` | G5 | Everything: the flat world has no cave air. A section of cave air counts 0 | no: shared types |
| C1b | `Section` packed; biomes in 64 cells; its serialised form, words as bytes. **`Section::biome()` stays**, as the first cell's, for format 1 (`clustine-format/src/section.rs:44`, `manifest.rs:46`, `52`) and `encode.rs:61`. Nothing makes a section of two biomes before C3 | G5 | Everything; messages with chunks are smaller; the wire number is raised if it is there | no |
| C2 | `Chunk` carries block entities, heights, the version and the failure bit, light, ticks, marks; `Chunk::set` by section 2; equality field by field and `shows_the_same`; **the edge compares with `shows_the_same`**; **the worker's snapshot is `for_viewers()`**. The flat generator marks its chunks and gives them no light yet. **A chunk read from format 1 is given the empty entities I2 asks for**, since format 1 stores none: a chest placed between C2 and C3 is a chest again after a restart | G5 | Everything. Nothing else of what C2 adds can be lost by format 1: nothing generated is stored before ADR-0021's W5, and a stored chunk is a changed one, with version 0 and no light | **no**: it changes a comparison the edge's resume turns on |
| C3 | Format 2, kind 5, one file a chunk; the refusal; `Section::biome()` goes; `world-format.md`, `architecture.md`, the crate's head. Besides the store's own tests of rounds and files (`services/worldstore/src/tests.rs:204`, `240`; `scenarios.rs:4030`; ADR-0018's seventeen in `rounds.rs`), these look for `manifests/overworld` and change: `bin/clustine/tests/persistence.rs:153`, `164`; `bin/clustine/tests/handoff.rs:520`; `services/worldstore/src/tcp.rs:855`, `978`. `StoreError::MissingSection` and `Hash` go | G5 | Everything | no |
| C4 | `clustine-light`, with its version; the flat generator's chunks carry its light | W1 | Everything; the edge does not use it yet | **yes**: a crate of its own. Tests 5 and 6 by someone else |
| C5 | The packet from what the chunk carries: heights, counts, biomes, block entities without data, sections copied. Light: the chunk's if it has any, otherwise today's `sky_light` | W2 | The flat comparison (the flat chunk's light from `clustine-light` equals `sky_light`'s, test 5). A changed chunk is lit as today | no |
| — | Light at the edge | W3 | by the record of section 7 | no |

C1 to C3 are the shared types and are done before anything is delegated. The store
that saves what it handed out and the pool are ADR-0021's and come in W5.

## Tests

Written from this record by someone who does not write the change.

1. **Sections.** For random sequences of `set`: `get` and `states` equal a plain
   array's; the two counts equal a recount by `is_air` and by `fluid`, and **a
   section of cave air counts 0**; equality is of content (a section that went to
   eight bits and back to two states equals a fresh one); a 17th, a 257th state
   move the form; the serialised form comes back equal and a broken one (an index
   beyond its palette, too few words) is an error, not a panic.
2. **`Chunk::set`.** For random changes over blocks with and without fluids, leaves
   and block entities: the four heightmaps equal a scan from the top by the rules
   of section 2; I1 to I3 hold after every step; the first change sets `generated`
   to 0 and clears the failure bit, and a `set` of the state that is there does
   neither; **the light stays through a change of stone to an ore and of grass to
   dirt, and goes at a torch, at water into air and at glass into stone**; ticks
   and marks are untouched. Data of 16 KiB and one byte for one block entity, and
   of 1 MiB and one byte for a chunk, are refused.
3. **Equality.** Two chunks that differ only in marks, in ticks, in light or in
   `generated` are not `==` and do show the same; two that differ in a block, a
   biome or a block entity do not show the same. `for_viewers()` has no ticks and
   no marks and shows the same as its chunk. At the edge: a snapshot that differs
   from the replica's chunk only in what is not shown sends nothing again; one
   that differs in a block sends the chunk again, as today.
4. **Format 2.** Known bytes for an empty chunk, a flat chunk and a chunk with one
   of everything, the failure bit among it; the writer gives the same bytes for
   equal chunks whatever their history; **a file with an unsorted palette and one
   with an unused entry are read**, and equal the chunk; a wrong checksum, version
   or kind (4 among them), an index beyond its palette, an id that does not exist,
   a body over 8 MiB and data over 16 KiB are refused; a file that lacks an entity
   I2 wants, or has one it does not want, is read and put right; heightmaps and
   counts of a chunk read back equal those before.
5. **The store.** A world of format 1 is refused with the sentence and its
   directory is as it was, byte for byte; a saved chunk is one file after the next
   sync and none before; a save over a stored chunk leaves one file; what a crash
   leaves is the chunk as before or as saved, at every point of a sync that fails.
   Between C2 and C3: a chunk with a chest, saved and read back, has the chest's
   entity.
6. **`clustine-light`, in CI**, hand-built: open sky; a roof; a pillar; a torch in
   a closed room; a torch one block inside the neighbour; a slab and a stair
   between two rooms; water and leaves under the sky; a neighbour that is `None`;
   a dimension without a sky; a section that is all 15 is `Full` and one that is
   all 0 is `Dark`. The flat chunk gives exactly what `sky_light` gives today. The
   light of a chunk does not change when a chunk two away changes.
7. **`clustine-light`, on the owner's machine** (the plan's W1): over the official
   blocks of the inner 14 by 14 of a forced area, the official light.
8. **The packet** (W2): decoded again it has the chunk's blocks, 64 biomes a
   section, the block entities **each without data**, the heightmaps and the two
   counts; `Full` comes out as an array of 15s. On the owner's machine, equal to
   the official server's packet for official chunks, but for the data of block
   entities, which is listed type by type.
9. **The sample**, on the owner's machine: the four heightmap rules give the
   stored maps of every `full` chunk of the sample world (the review's check, kept
   as a test); the sizes of this record's tables come out of a built `Chunk`
   within a tenth.
10. **End to end**: the existing tests on the flat world pass at every step; the
    comparisons with the official server (`official_server`) pass after C2 and C5.

## Ruled out

| What | Why not |
|---|---|
| Sections as files under their hashes, one more blob a chunk, a manifest (the plan's list for G5) | Section 3, measured: 11.2 files and 45.9 KB of disk a chunk against one file and 7.9; inodes for a volume of 180 GB for a million chunks; a second round at every write; garbage with every save; and nothing shared, 918 distinct sections of 918 |
| One file for 32 by 32 chunks, as the official server's regions | Files would be changed in place or rewritten whole, against the format's third principle. A later record can pack chunk files if their number or their slack becomes a burden |
| 4,096 ids a section in memory, packed only on the way | 74.5 KB a chunk against 19.8, in every worker and edge, and the packet would still build a palette for every section each time |
| A section's own, simpler widths (1, 2, 4, 8, 16 bits) | The packet could not copy it, and it would cost 0.8 KB a chunk more over the sample |
| Heightmaps and counts on disk and between services | They follow from the blocks; kept in two places they can disagree |
| A reader that refuses whatever the writer would not have written (the draft) | Nothing needs equal bytes any more, and it would make a chunk unreadable that has all its blocks |
| Light dropped at every change (the draft) | Most changes are of no concern to light, and every chunk's own marks and ticks would take its light once phase B is built |
| The rules for light at the edge in this record (the draft's E1 to E7) | Section 7: five findings change the rules themselves |
| Light computed by the worker each tick; relighting on the fan-out task; the edge asking for one more ring of chunks | Each as the draft had it: a lighting engine in the simulation's process; one task for all players; a region's land is what its players see (ADR-0017, section 3.1). The record of section 7 starts from these |
| Block entity data sent to clients as stored, less a list of keys to leave out (the draft) | The wrong way round: it tells every client the loot seed of every chest, and stays wrong for every type nobody has compared |
| One type of chunk for a worker and an edge, ticks and marks and all (the draft) | The edge would see a difference whenever phase B takes them, and send the chunk again |
| Kind 4 for a chunk's file (the draft) | ADR-0020 has it |
| Compressing chunks between services | Choice 3 |
| Carrying a world of format 1 over | Worlds are thrown away until the first release |
| Parsing block entity data in `clustine-world` | It would bring NBT into the simulation's crate for something it never reads |
| C3 before C2 | Format 2 writes what C2 adds; and reading format 1 can make the empty entities again, which is all that is lost |

## Risks

- **The sample is 101 chunks at four places of one seed.** A built-up place has
  wider palettes and more block entities; an ocean has other light. The forms hold
  for all of them (the direct form is 8 KB a section at worst); the means move.
  Test 9 keeps the measurement repeatable, and W5 measures again on what the
  generator makes.
- **A million files of 5 KB.** They take 7.9 GB for 5.1 GB of content: 55 % more
  is slack in the last block of each. They are slow to copy and to back up, and
  each takes an inode (one for every 16 KB of an ext4 volume by default, so a
  volume fills its inodes when it is half full of chunks of this size). A later
  record can pack them; the official server's 1,000 files for the same world are
  the measure.
- **The plan is departed from** in the store's files, with the measurement above.
- **Light at the edge is not decided**, and W3 waits for its record. Until then a
  changed chunk is lit as today, by a rule that is wrong under any overhang. C1 to
  C5 do not wait.
- **The edge holds more**: 32 KB a chunk and a packet of 32.5 KB beside it, 21 MB
  for a player far from all others.
- **The packet is larger to compress**, 329 times for a join, each on its
  connection's task. Section 7, item 7.
- **The store's tests of rounds and of files** are rewritten in C3, and five tests
  outside the store's own change with them.
- **C2 changes what the edge compares**, in the part where ordering mistakes hide.
  It is not delegated, and test 3 has both directions.
- **`Chunk::set` does more.** Bounded and rare in a tick (section 8); the
  generation service writes blocks by the hundred thousand and uses a working form
  of its own until a chunk is finished.
- **A palette with unused states, or wider than needed, is not what the official
  server would send.** Allowed by the format as far as is known.
- **The fluid count's rule is a guess**; W2 decides before anything is built on it.
- **A change that light sees is told by the table's fields.** If light depends on
  something of a state that is not among them, a chunk keeps light it should have
  dropped. W1's comparison is over whole chunks and would not show it; the record
  of section 7 has a test for it, since only the edge acts on it.

## Not checked

- How long `clustine_light::light` takes for a chunk of real terrain: the crate
  does not exist yet.
- What the client does with a palette that holds unused states or is wider than
  needed; with a block entity it is sent without data; with a block entity for a
  block it then sees change.
- What exactly the packet's fluid count counts.
- What the official server sends as a block entity's data in a chunk packet, for
  any type.
- The official server's rule for which light sections are absent, dark or listed
  in its packet, beyond the flat world; the rule section 6 gives the import is a
  guess.
- How two face shapes stop light between two blocks (the plan: "from memory:
  together, when their union covers the face").
- How many marks lead to a changed block once phase B honours them: 79 of 100 lie
  on a fluid; whether each flows depends on what is beside it.
- The in-memory sizes are arithmetic on these forms over the sample, not a built
  `Chunk`; test 9 checks them when there is one.
- What a link does when one chunk in a message cannot be made sense of
  (`wire.rs:34-36`); read, not followed through the links' code.
- Whether the wire number exists when C1 lands.
- The Nether and the End: no sky light in one, other heights in both. Phase D.

## Choices the owner has not settled

Each is decided above so that the record can be built from; each can be turned.
Those the session decided after the review are not listed again (the record cut in
two, the numbers against ADR-0020, no block entity data to clients, `is_air`).

1. **A stored chunk is one file**, against the plan's sections under their hashes,
   with the cost named: a million small files, 55 % slack.
2. **Sections in memory in the client's own layout.** The plan said "chosen by
   measurement"; the measurement bears it out (93 of 100 sections at 4 bits, none
   direct).
3. **Chunks between services are not compressed.** 29.5 KB a chunk, 9.7 MB a first
   view a hop; zstd at a low level on the three messages is the change if W5's
   join on the test cluster wants it.
4. **Heightmaps and counts are never stored or sent**, always made from the blocks.
5. **Light is dropped only by a change that light sees**, told by the table's
   fields, and "as generated" is kept apart from it.
6. **A fourth case of a light section, `Full`**, in memory, between services and
   on disk.
7. **The reader repairs I2** (an entity missing or too many) with a warning, and
   refuses only what it cannot make sense of.
8. **The bounds**: 16 KiB of data for a block entity, 1 MiB for a chunk's, 8 MiB
   for a file's body.
9. **`Chunk::set` keeps block entities in step**: a block with a block entity type
   gets an entity without data when it is placed, and loses it when it goes.
10. **Ticks and marks stay with a chunk through every change** until phase B takes
    them, and are not sent to an edge.
11. **The path has the dimension**: `chunks/overworld/…`.
12. **A world of format 1 is refused and left alone**; the code for worlds older
    than that is left where it is.
13. **`structures`, `InhabitedTime` and `LastUpdate` are not carried.**
14. **C2 before C3**, with format 1's reader making the empty entities.

## Review

An independent reviewer went over the draft against the code at `b67a1d0` and
against the sample world, and measured the 101 chunks. Fifteen findings. Each was
checked against what it cites before it was taken.

0. **The measurements.** Taken whole: every table of sizes in this record is the
   review's, and every guess of the draft is gone. All of them were on the high
   side or right.
1. **"As generated" will not outlast a chunk's first tick once B6 is built, and
   the edge then relights nearly every chunk.** Accepted as a finding; by the
   session's decision its answer is **handed to the record on light at the edge**
   (section 7, item 1, with all four proposals). Of the proposals, the third is
   done here: light is dropped only by a change that light sees, and "light is
   valid" is kept apart from "as generated". **Not followed**: deciding where
   light lives for a changed chunk in this record, which the finding asked for.
2. **A player who has the neighbour of a changed chunk keeps wrong light.**
   Accepted; the order of events holds against the draft's rules. Handed on
   (section 7, item 2).
3. **A view that arrives chunk by chunk makes up to nine relights for every
   changed chunk.** Accepted; confirmed at `fanout.rs:2165-2194`. Handed on
   (item 3).
4. **What two chunks being equal means was not said.** Accepted; confirmed at
   `fanout.rs:2177` and `chunk.rs:9`. Equality is defined field by field, the edge
   compares what is shown, a snapshot carries no ticks and marks, and C2 is not
   delegated.
5. **Carried light across two generator versions, and after a fix to light.**
   Accepted. The light's version is in the chunk and the crate; the rule that
   uses it is handed on (item 4).
6. **Kind 4 is taken.** Accepted; confirmed, and ADR-0020's types have landed
   since (`0bfc2e9`). Kind 5, and section 3 says which number whoever lands
   second takes.
7. **Two steps did not leave everything working.** Accepted, all of it: `biome()`
   stays until C3; a chunk read from format 1 gets its empty entities; the files
   C3 touches outside the store's tests are listed (one more than the finding
   had, `handoff.rs:520`); M0 is done by the review; C2 is not delegated.
   **Chosen** of the finding's two ways: C2 before C3.
8. **Four things the rules for the edge left open.** Accepted. Handed on (item 5).
9. **Block entity data sent as stored is the wrong way round.** Accepted; by the
   session's decision nothing is sent until a type is known to need something,
   and there is a bound where data comes in.
10. **"Not air" has to be the table's `is_air`.** Accepted; confirmed at the four
    places, which are step C1a.
11. **The cases of a light section.** Accepted, all three parts: the import
    derives `Dark` from `Absent` (section 6, a guess until W4); a light update
    cannot say `Absent` (handed on, item 6); `Full` is a fourth case.
12. **Two sentences of the argument for one file were not what the measurements
    or ADR-0018 say.** Accepted: 5.8 times, not "four to five"; a second round and
    twenty waves, not "ten times the syncs"; a block change costs the same either
    way; inodes and slack are named under "Risks". The note on `PENDING_LIMIT` is
    ADR-0021's, which gave notes a limit of their own.
13. **"Equal chunks have equal bytes" has no one left who needs it.** Accepted:
    the writer keeps one form, the reader refuses only what it cannot make sense
    of.
14. **Uncompressed between services: the figures.** Accepted; they are in section
    4. Compressing the packet once is handed on (item 7).
15. **"Exactly the kinds of thing" was not so.** Accepted: `structures`,
    `InhabitedTime` and `LastUpdate` are named as not carried, and the sentence is
    gone.

Nothing of the review was rejected outright. One of its proposals was answered
otherwise than proposed, by the session: where light lives for a changed chunk is
not decided in this record (finding 1), because that decision and the four
findings beside it make a set of rules that should be written and reviewed as one.
