# World format

How Clustine stores a world on disk. This describes **format version 16**
(`FORMAT_VERSION` in `crates/clustine-format/src/lib.rs`) as implemented in
`crates/clustine-format` (encodings) and `services/worldstore` (files); see
`docs/adr/0008-durable-regions-and-resuming.md` for what the store promises of commits
and states, `docs/adr/0011-the-world-store-and-regions.md` for what it promises of
regions and the chunks they hold, `docs/adr/0022-what-a-chunk-carries.md` for what
a chunk is and why its blocks are one file, and `docs/adr/0028-entities.md` for the
entities beside them. Where this document and the code disagree, the code is right.

The format is not stable yet: it may change without a migration path until the first
release. **A world of an earlier format is not opened and not made over.** A store that
finds an earlier `format` in a world's `meta` stops before it reads, changes or removes
anything else in the directory, and says:

```text
This world was made by an earlier Clustine (world format 15; this server writes
format 16). Worlds are not carried over before the first release: move <directory>
away or name another, and a new world is made.
```

A world of a later format is refused too, as any other setting of `meta` that differs
("the world was made with format `17`, but this server uses `16`").

The version is raised whenever what is written changes meaning; a tag that no world
made before it can hold (a new kind of entity data, of effect or of a use of an entity)
needs no raise, as an older record never has it. The raises since 2:

| Format | What changed |
|---|---|
| 4 | A player's place holds the whole inventory, 41 slots, where it held the hotbar's nine |
| 8 | A place holds the ender chest's 27 slots after the inventory |
| 9 | A minecart holds a TNT cart's fuse and a hopper cart's switch |
| 10 | An entity keeps `lived` and `fresh_at`, an item its age apart; a change may set an entity's hops; a mob is kept by the parts of its body and mind and its type's state |
| 11 | A place holds the experience after the ender chest |
| 12 | A place holds where a bed set the respawn point after the experience |
| 13 | Entities made with a chunk have keys of their own above every region's; a chunk's first blocks file follows its entities; a seat may be granted across a line |
| 14 | Every stack carries its components after its count, kinds by name |
| 15 | A place holds the effects and the time since rest after the respawn point |
| 16 | A chunk keeps the structure starts that reach it which natural spawning and the patrol spawner ask |
| 17 | A chunk keeps the claims on its points of interest, and a commit may carry them |

## Principles

- **What a region was given is stored.** A chunk that is generated for a region is
  noted behind the answer and becomes durable with the next sync the store makes, at
  the latest when the store is stopped in order. Until then it is made again when
  needed, and is the same. A chunk that was never handed to a region is not stored.
  Every stored chunk says whether it is exactly as a generator made it, which version
  of the generator, and whether something failed while it was made
  (`docs/adr/0021-generation-is-a-function.md`, section 6).
- **A world is tied to the generator it was made with, and takes a newer version of
  it.** `meta` records the generator's settings and the highest version that made a
  chunk the world stores. A server with other settings, or with an older version, is
  refused. One with a newer version opens the world: what is stored stays as it is,
  what is made from then on is the newer generator's, and a chunk that was stored with
  a failure and never changed is made again.
- **A chunk is two files at most**, each checksummed, written whole or not at all: its
  blocks and everything kept with them, and beside it its entities other than players,
  so that an entity's step does not rewrite the blocks. Nothing is shared between
  chunks, and a chunk that is saved again leaves nothing behind.
- **Files are never modified in place.** Every file but the log is written under a
  temporary name, made durable and renamed, so a reader sees it whole or not at all.
  The log is only appended to.
- **Nothing is durable before it is synced,** and a file that was created, renamed or
  removed is not durably so before its directory is synced. The store syncs what it
  relies on, and nothing it has answered depends on what it has not synced.

All integers are big-endian everywhere, and so are the floating-point numbers (IEEE 754,
`f32` and `f64`). Every file and every record of the log begins with the format version
(u8), and every file ends with a CRC-32 of everything before it (u32). A reader refuses
another version than its own, and refuses bytes left over after what it read.

## Directory layout

```text
<world>/
  meta
  log/<segment>.wal
  regions/table
  regions/players
  regions/blocks
  regions/couplings
  regions/<region>.region
  regions/<region>.state
  chunks/<dimension>/<rx>.<rz>/<x>.<z>.chunk
  chunks/<dimension>/<rx>.<rz>/<x>.<z>.entities
```

- `meta` is a text file of `key=value` lines: `format` (the format version),
  `data-version` (the Minecraft data version whose block state and biome ids the world
  uses), `generator` (the generator's settings, but its version),
  `generator-version`, the highest version of the generator that made a chunk the world
  stores (`docs/adr/0021-generation-is-a-function.md`, section 8), and `spawn`, the
  world's spawn as `x,y,z`, where the generator put it when the world was made. The
  file is written before anything else when the world is made, with the version of the
  generator that made it. A store with a newer generator opens the world and changes
  nothing, until the sync that would first put in place a file that holds a chunk the
  newer version made, or a chunk changed from one: that sync first replaces `meta` with
  the newer version and syncs it and its directory, and only then puts the chunk files
  in place. A store with an older generator than the line names stops and says:

  ```text
  This world holds chunks made by generator version 7, and this server has version 5.
  Start it with a server that is at least as new.
  ```

  A `meta` without `spawn` is of a world made before the spawn was kept, whose players
  entered on the column at the origin. Where the generator's spawn is on that column
  the line is added, by the same durable replacement; otherwise the world is refused,
  as one of an earlier Clustine. A `generator` of the same seed made by another of
  Clustine's generators is refused with a message that says so.
- `log` holds the write-ahead log that all regions share, in segments numbered in the
  order they were begun (twenty decimal digits). A segment that has grown beyond 64 MiB
  is not appended to any more.
- `regions/table` holds the regions there are and the chunks each holds, as of a place
  in the log.
- `regions/players` holds what the store keeps of every player who ever joined, as of a
  place in the log. A world in which the file was never needed has none.
- `regions/blocks` says how far blocks of entity keys have been reserved.
- `regions/couplings` holds what the store keeps of the lockstep of regions whose border
  ticks (`docs/adr/0025-effects-between-regions.md`, section 8).
- `regions` holds two files per region besides: `<region>.region`, with the highest
  epoch it was opened with and its block of entity keys, once it has been opened or was
  made by a split, and `<region>.state`, with its state as of its last checkpoint.
  `<region>` is the number of the region in decimal. A file of `regions` whose name
  ends in `.tmp` is what a crash left of a replacement, and is removed when the store
  starts.
- `chunks` holds the files of every stored chunk at chunk coordinates `x`, `z`, grouped
  into directories of 32×32 chunks (`rx = x >> 5`, `rz = z >> 5`): its blocks file,
  `<x>.<z>.chunk`, and, where it has entities, its entities' file, `<x>.<z>.entities`.
  `<dimension>` is the world's dimension without its namespace (`overworld`,
  `the_nether`, `the_end`). A world has one dimension, and every world is the overworld
  so far.
- A world from before there was a table has a file `layout`; that it is there says so,
  and what it says is not read.

Chunks are kept the same way whichever region they are in.

## Chunks

A chunk's blocks file, of kind 5 (`crates/clustine-format/src/chunk.rs`):

| Field | Type |
|---|---|
| Format version | u8, 16 |
| Kind | u8, 5 |
| Chunk x, z | i32, i32 |
| Y of the lowest block | i32 |
| Tick at which the chunk was saved | u64 |
| Epoch of the region that saved it | u64 |
| **Generated**: the version of the generator the chunk is exactly as made by, or 0 | u16 |
| Flags | u8: bit 0 "with a failure", bit 1 "with seams", bit 2 "with block events", bit 3 "with structures", bit 4 "with claims"; the others 0 |
| Section count | u16 |
| Codec of the body | u8: 0 as it is, 1 zstd |
| Length of the body, as it is | u32, at most 8 MiB |
| Length of the body, as stored | u32 |
| The body, as stored | bytes |
| CRC-32 of everything before | u32 |

The body, before compression, is these parts in this order:

| Part | Encoding |
|---|---|
| Per section, bottom to top: blocks | palette length `n`, u16, 1 to 4,096; `n` × u16 state ids; if `n` > 1, 4,096 indices of `ceil(log2(n))` bits in u64 words, lowest bits first, none across two words, in the order `y << 8 \| z << 4 \| x` |
| … biomes | palette length `m`, u8, 1 to 64; `m` × u16 biome ids; if `m` > 1, 64 indices of `ceil(log2(m))` bits, packed the same way, in the order `y << 4 \| z << 2 \| x` of cells of 4×4×4 blocks |
| Block entities | count u32; then, by y, z, x: `x << 4 \| z` u8, y i16, type u16, length of the data u32 (at most 16 KiB), the data |
| Light | u8: 0 for none, 1 for present without `beside`, 2 for present with it (the writer writes 0 or 2; 1 is read from files written before `beside` was kept, as light that does not say). If present: `made_by` u16; for 2, `beside` u16, the version of the generator whose chunks the light was made beside, or 0; then for sky and then for block, per light section bottom to top: u8 0 absent, 1 dark, 2 full, or 3 followed by 2,048 bytes |
| Scheduled ticks | count u32; then, in order: kind u8 (0 block, 1 fluid), `x << 4 \| z` u8, y i16, id u16 (a fluid's below 256), delay i32, priority i8 |
| Marks | count u32; then, ascending: `x << 4 \| z` u8, y i16 |
| Seams, with bit 1 of the flags only | as below; not empty |
| Block events, with bit 2 of the flags only | as below; not empty |
| Structures, with bit 3 of the flags only | as below; not empty |
| Claims, with bit 4 of the flags only | as below; not empty |

A chunk has one light section more below its sections and one more above. The data of
a block entity is one NBT compound as the game writes it on the network, without the
keys `x`, `y`, `z` and `id`; no data stands for an empty compound. A chunk without
seams, block events, structures or claims has the flag clear and the part left out, so that
it is written as a chunk was before there were any.

**Seams** (`seams.rs`) are the updates a region could not make, because they reach a
block whose chunk, or a neighbour's, it had not loaded
(`docs/adr/0024-block-behaviour-in-a-region.md`, section 5): a count u32, then per
entry, ascending by the block it reaches (x, then y, then z), the direction and the
kind, each such key once:

| Field | Type |
|---|---|
| The block it reaches | i32 x, i32 y, i32 z |
| Direction | u8, 0 to 5: down, up, north, south, west, east |
| Kind | u8: 0 `updateShape`, 1 `neighborChanged` |
| The block that changed | u16, a state id |
| Flags | u32 |
| Depth | i32 |

**Block events** (`kept_events.rs`) are those that outlived the tick they were queued in
(`docs/adr/0026-redstone.md`, section 4): a count u32, then per event, in the order of
the region's queue:

| Field | Type |
|---|---|
| Where | `x << 4 \| z` u8, y i16 |
| The block's id, which the block there has to be for the event to run | u16 |
| The game's `b0`, the kind, and `b1`, the data | i32, i32 |

**Structures** are the structure starts whose box reaches the chunk, of the kinds that
natural spawning and the patrol spawner ask (`docs/groundwork/n9-plan.md`, section 9.6):
a count u32, then per start, in generation's order:

| Field | Type |
|---|---|
| The structure's name in the game's registry | length u8, then UTF-8 |
| The chunk it starts in | i32 x, i32 z |
| Its box | a box |
| Its pieces' boxes that reach the chunk, in the start's order | count u32, then the boxes |

A box is its lowest corner and its highest, each x, y, z as i32: six i32s. The names a
chunk keeps are `minecraft:swamp_hut`, `monument`, `pillager_outpost`,
`ancient_city`, `trial_chambers`, `fortress`, `village_plains`, `village_desert`,
`village_savanna`, `village_snowy` and `village_taiga`, all in the `minecraft`
namespace.

**Claims** are the points of interest of the chunk's blocks with fewer free tickets than
their type has at most (`docs/groundwork/n10-plan.md`, section 3.2): a point of interest
is wherever a block of its type is, so only how many of its tickets are taken is kept. A
count u16, then per claim, ascending by position (y, then z, then x):

| Field | Type |
|---|---|
| Where | `x << 4 \| z` u8, y i16 |
| The point of interest's type, by the game's registry id (0 to 12 the job sites, 13 `home`, 14 `meeting`, up to 20 `lightning_rod`) | u8 |
| Its free tickets | u8 |

A type that does not exist is refused. A claim whose block is not of its type, or with
as many free tickets as its type has, is dropped and counted with the block entities
that were not as the blocks ask, and the chunk is read: such a mismatch can come of a
change made without corruption, and the game drops such a record quietly too.

- **The writer writes one form**: palettes ascending with nothing unused, lists in
  order, light that is all 0 or all 15 as `dark` and `full`. The body is compressed
  with zstd at level 3 if that makes it smaller.
- **The reader refuses what it cannot make sense of, and nothing else**: a wrong
  checksum, version or kind; a length that does not fit, or bytes left over after the
  body or after the file; a palette length of 0 or beyond the positions it covers; an
  index beyond its palette; a state id, biome id or block entity type that does not
  exist; a block entity's data over its bound; a light byte above 2 or a light
  section's form above 3; a scheduled tick of another kind than 0 or 1, or a fluid id
  above 255; flags that mean nothing; a flag of a part whose part is empty; a seam's
  direction above 5, kind above 1 or block that does not exist, or seams out of order
  or twice; a structure's name that is not UTF-8 or not one a chunk keeps; a box whose
  lowest corner is above its highest on any axis. A palette that is not sorted or has
  an unused entry is read. The store refuses besides a file whose position is not the
  one its name says.
- **Block entities are made to fit the blocks on reading**: a position whose block has
  a block entity type and that has no entity gets one without data, and an entity
  where the blocks ask for none, or for another type, is dropped. The store logs a
  warning that names the chunk and how many there were. Counts and heightmaps are not
  stored; they are made from the blocks.
- The tick and the epoch are for whoever looks at what is stored; nothing reads them
  back. The epoch is the one the region was opened with by whoever saved the chunk:
  for a chunk a region saves, its owner's; for a chunk the store saves when a region
  is opened and its commits are put into the stored chunks, the epoch it is opened
  with; and 0 for a chunk the store notes when the generator made it, and for one it
  saves when it makes a world over for another division, which no region does.

### The entities' file

A chunk's entities other than players are kept in a file of kind 7 beside its blocks
(`entities.rs`; `docs/adr/0028-entities.md`, section 2). A chunk without entities has
no such file.

| Field | Type |
|---|---|
| Format version | u8, 16 |
| Kind | u8, 7 |
| Chunk x, z | i32, i32 |
| Tick as of which it was saved | u64 |
| Epoch of the region that saved it | u64 |
| Entities, ascending by key, each once: count, which is not 0, then each as below | u32 |
| CRC-32 of everything before | u32 |

The reader refuses a file without entities, with two of one key or out of order, or
with an entity that rides nothing and whose position is not in the file's chunk: a
passenger is written in its root's chunk, wherever its own position is. The store
refuses besides a file whose position is not the one its name says.

A blocks file and its entities' file are saved as parts: a save may write either or
both, and the part not saved stays as it is. A chunk stored for the first time is
written whole, and its entities' file is made durable, with its directory, before its
blocks file is put in place, so that a blocks file always has its entities beside it.
An entities' file without a blocks file is never read, and is written again with the
chunk's next note.

### An entity

An entity, here and wherever the store keeps one (a commit's changes, an arrival, a
unit kept with a player), is:

| Field | Type |
|---|---|
| Key | u64 |
| Type, an index into the game's entity types | u16 |
| UUID | u128 |
| Position | f64 x, f64 y, f64 z |
| Motion | f64 x, f64 y, f64 z |
| Yaw, pitch | f32, f32 |
| Flags | u8: bit 0 on the ground; the others 0 |
| Hops: changes of holder behind it | u32 |
| Lived: the ticks it has ticked, as of the record's tick | u32 |
| Fresh at: `lived` at its last fresh load | u32 |
| Fall distance | f64 |
| Fire: ticks left to burn, or below 0 safe from fire | i32 |
| Riding | as below |
| Data | a tag u8, then what the tag says |

Counters are kept relative to the record they are in: regions count their ticks each
from their own start (`docs/groundwork/n2-plan.md`, section 11).

**Riding** is, in order:

| Field | Type |
|---|---|
| Vehicle | u8 0, or u8 1 and its key u64 |
| Cooldown | u16 |
| Passengers: count, then each | u8 |
| … key | u64 |
| … flags | u8: bit 0 a player, bit 1 pending (only with bit 0), bit 2 a pending seat granted across a line (only with bit 1); the others 0 |
| … *pending only:* ticks left, edge | u16, u64 |
| Leash | u8: 0 none, 1 an entity's, then its key u64, 2 a fence's, then its x, y, z as i32 |
| … *1 and 2 only:* the angular momentum | f64 |

The **data** of each tag, in the order written. A flag is u8 0 or 1 and nothing else;
an option is u8 0, or u8 1 and the value; a stack is as "Stacks" says; an owner is an
option of the owner's key u64; a block-attached entity's attachment is its block (i32
x, y, z) and the direction (u8, 0 to 5); a container's contents are a count u8 and per
stack its slot u8 and the stack, ascending by slot, each slot once; a hurt is the time
i32, direction i32 and damage f32.

| Tag | Entity | Data |
|---|---|---|
| 1 | an item | stack; the pick-up delay left u16; the thrower, an option of a UUID u128; health i16; how far `lived` is ahead of its age u32 |
| 2 | a falling block | state u16; ticks it has fallen i32; flags u8 (bit 0 it drops as an item where it cannot land, bit 1 its drop is called off) |
| 3 | a minecart | hurt; displayed block, an option of u16; display offset, an option of i32; flipped flag; contents; fuel i32; push 2 × f64; a TNT cart's fuse i32; a hopper cart's switch, a flag |
| 4 | a boat | hurt; paddles u8 (bits 0 and 1, the two paddles as the game numbers them); bubble time i32; out of control f32; turning f32; status 2 × u8; contents |
| 5 | an armour stand | flags u8; disabled slots i32; the pose, six parts of three angles f32; equipment as contents; health f32; last hit, an option of u32; air i16; frozen u16 |
| 6 | an item frame | attachment; item, an option of a stack; rotation u8, 0 to 7; drop chance f32; flags u8 (bit 0 fixed, bit 1 invisible) |
| 7 | a painting | attachment; variant u16 |
| 8 | a cushion | attachment; colour u8 |
| 9 | a leash's knot | attachment |
| 10 | primed TNT | fuse i16; block u16; power f32; owner, an option of a UUID u128 |
| 11 | a mob | its body's parts, then its mind's, each a count u16 and the parts ascending by id, each id once; then its type's state |
| 12 | an orb of experience | value i32; count i32; health i16; how far `lived` is ahead of its age u32; the player it follows, an option of a key u64 |
| 13 | a thrown item | stack; owner; flags u8 (bit 0 it has left its owner, bit 1 it has been shot) |
| 14 | an arrow, a spectral arrow or a trident | as below |
| 15 | the llama's spit | owner; flags u8 as a thrown item's |
| 16 | a firework rocket | stack; owner; flags u8 as a thrown item's, and bit 2 shot at an angle; life i32; lifetime i32 |
| 17 | an area effect cloud | as below |
| 18 | the evoker's fangs | owner; warmup i32 |

A mob's part is its id u16, its version u8 and its bytes as a length u32 and the bytes;
its type's state is its version u8 and its bytes as a length u32 and the bytes. The
store does not look into either.

An **arrow** (tag 14) is: its pickup u8 (the game's ids: 0 disallowed, 1 allowed, 2
creative only); its pickup stack; the weapon, an option of a stack; its owner; flags
u8 (bit 0 it has left its owner, bit 1 it has been shot, bit 2 critical); the base
damage f64; the pierce level u8; the entities pierced, a count u32 and each key u64 in
the order pierced; the hit sound as a string (a length u32 and UTF-8); where it is
stuck, an option of the state u16 and the `lived` it landed at u32; the `lived` its
shake ends at u32; and its kind u8: 0 an arrow; 1 a spectral arrow, followed by the
glowing's duration i32; 2 a trident, followed by a flag whether it has dealt its damage
and its returning i32.

An **area effect cloud** (tag 17) is: its duration i32, wait time i32, reapplication
delay i32, radius f32, the `lived` its radius is as of u32, radius on use f32, radius
per tick f32, duration on use i32, its potion as a stack's `potion_contents` value is
written, its duration scale f32, its owner, and its victims: a count u32 and each one's
key u64 and the `lived` it may be affected again from u32, ascending by key, each once.

The reader refuses a tag, a flag or an enumerated value it does not know, flags that
mean nothing, a seat's flags that do not go together, a direction above 5, contents,
parts or victims out of order or twice, and a list's count beyond the bytes left.

## Stacks

A stack, wherever the store keeps one (an item on the ground, a thrown item, an arrow's
pickup stack and the weapon it was shot from, a firework rocket's stack, the contents
of a cart or a boat, a stand's equipment, an item frame's item, a player's inventory and
ender chest, a stack given to a player or held in a use of an entity across a line), is
written as `crates/clustine-format/src/stacks.rs` says:

| Field | Type |
|---|---|
| Item, an id in the item registry | i32 |
| Count | i32 |
| Components added or changed: count, then each | u16 |
| … the kind's name in `data_component_type` | u16 length, UTF-8 |
| … the length of the value in bytes | u32 |
| … the value, in Clustine's own encoding | |
| Kinds removed: count, then each kind's name | u16; u16 length, UTF-8 |

A stack keeps only what differs from its item's defaults (`docs/groundwork/n9-plan.md`,
section 3.1), so a plain stack is its item, its count and two counts of 0: twelve
bytes. Components are written one of a kind, in Clustine's order of the kinds, and so
are the kinds removed. A kind this server does not know is skipped by its length, and a
removed kind it does not know is passed over, so that a kind added later needs no new
format. The reader refuses a kind named twice, or both added and removed; a name that is
not UTF-8; a value shorter or longer than its length; and stacks nested in stacks more
than eight deep.

In a value, registry values (a potion, an effect, an enchantment, a pattern, a colour,
a variant) are kept by name, so that a game update that renumbers a registry needs no
new format. A string is a length u32 and UTF-8; a list is its count u32 and its
elements (a count beyond the bytes left is refused); an option is a flag u8 (0 none, 1
some) and, where some, the value; a flag is u8 0 or 1; a float is its `f32` bits. By
kind, each name in the `minecraft` namespace:

| Kind | Value |
|---|---|
| `potion_contents` | potion, an option of a string; custom colour, an option of i32; custom effects, a list of effect instances; custom name, an option of a string |
| an effect instance | effect, a string; amplifier i32; duration i32; ambient, visible and show icon, three flags |
| `potion_duration_scale` | f32 |
| `charged_projectiles` | a list of stacks, each written as above |
| `enchantments` | a list of (enchantment string, level i32) |
| `use_cooldown` | seconds f32; group, an option of a string |
| `fireworks` | flight duration i32; a list of explosions |
| an explosion of those | shape u8 (the game's ids, 0 small ball to 4 burst); colours, a list of i32; fade colours, a list of i32; trail and twinkle, two flags |
| `damage` | i32 |
| `intangible_projectile` | nothing |
| `banner_patterns` | a list of (pattern string, colour string) |
| `item_name` | a string, the text component as the game's JSON writes it |
| `rarity` | u8 (the game's ids, 0 common to 3 epic) |
| `tooltip_display` | hide tooltip, a flag; hidden components, a list of strings |
| `suspicious_stew_effects` | a list of (effect string, duration i32) |
| `ominous_bottle_amplifier` | i32 |
| `chicken/variant` | a string |
| `stored_enchantments` | a list of (enchantment string, level i32) |
| `dyed_color` | i32, the RGB colour |

The stacks in a container block's entity in a chunk file are the game's NBT
(`ItemStackWithSlot`), and keep no components yet.

## The write-ahead log

All regions share one log, so that a single sync makes the commits of all of them
durable (`crates/clustine-format/src/log.rs`). It is a sequence of records, each framed
as a u32 payload length, a u32 CRC-32 of the payload, and the payload. Every payload
begins with the format version (u8) and the kind of record (u8):

| Kind | Record | Then |
|---|---|---|
| 1 | Block changes, of the worlds from before regions had a state; read, never written | tick u64, epoch u64, changes |
| 2 | Commit | region u32, tick u64, epoch u64, changes, state |
| 3 | Opened | region u32, epoch u64, restored u64 |
| 4 | Granted | region u32, tick u64, chunks |
| 5 | Returned | region u32, chunks |
| 6 | Absorbed | region u32, epoch u64, absorbed u32, tick u64, state |
| 7 | Split | region u32, epoch u64, tick u64, state, part u32, part epoch u64, chunks, part state |
| 8 | Commit with stay notes | the fields of kind 2, stays |
| 9 | Commit with records of chunks | the fields of kind 2, stays (whose count may be 0), ticks, marks taken (chunks), seams |
| 10 | Commit with effects | the fields of kind 9, effects, taken |
| 11 | Effects dropped | numbers |
| 12 | Effects carried forward | filed effects |
| 13 | Commit with block entities | the fields of kind 10, block entities |
| 14 | Commit with block events | the fields of kind 13, events |
| 15 | Commit with entity changes | the fields of kind 14, entity changes |
| 16 | Commit with arrivals | the fields of kind 14, a flag u8 whether entity changes follow, the entity changes if they do, arrivals, arrivals taken (numbers) |
| 19 | Commit with claims | the fields of kind 16, whose arrivals may be none, claims |
| 17 | Arrivals dropped | numbers |
| 18 | Arrivals carried forward | filed arrivals |

where `changes` is a count (u32) followed, per change, by i32 x, i32 y, i32 z and a u16
block state; `chunks` is a count (u32) followed, per chunk, by i32 x and i32 z; and a
`state` is a length (u32) followed by that many bytes, which the store does not look
into. A payload is at most 64 MiB; a longer one is read as a record that was cut off.
A record that is whole and passes its checksum but cannot be read (another version, a
kind that does not exist, a field the reader refuses, bytes left over) is an error, not
the end of the log: that is not what dying in the middle of a write looks like.

### Commits

A commit holds what one tick of a region changed: its block changes, in order, and the
region's own record of the change of its state. The epoch is that of the owner that
committed it. A commit is one record whatever it carries, so that all of it is on disk
together or not at all, and it is written in one form: as the lowest kind that holds
what it carries. In order of the fields:

- **Stays** (kind 8 on): a count (u32) and the notes, as below. In kind 8 the count is
  not 0, and a commit without notes and without anything of the later kinds is of kind
  2; from kind 9 on it may be 0.
- **Ticks** (kind 9 on): the scheduled ticks of each chunk whose ticks changed in the
  tick, whole, as the delays left at its end, ascending by chunk: a count of chunks
  (u32), and per chunk i32 x, i32 z, a count of ticks (u32) and per tick u8 x, i16 y,
  u8 z within the chunk (x and z below 16), u8 1 for a block or 2 for a fluid, u16 its id
  (a fluid's below 256), i32 the delay left and i8 the priority. (The chunk file numbers
  the kinds 0 and 1 and packs the column; the log does not.)
- **Marks taken** (kind 9 on): the chunks whose marks for post-processing were taken in
  the tick, ascending, as `chunks`.
- **Seams** (kind 9 on): a count of chunks (u32), and per chunk whose entries changed
  i32 x, i32 z and the chunk's entries whole, as in a chunk file (here a chunk's list
  may be empty).
- **Effects** and **taken** (kind 10 on): the effects the store keeps of what the tick
  made, each with when it is due, as "Effects" says, numbered by the record's sequence
  and their place in it; and the numbers of the effects the tick took, ascending.
- **Block entities** (kind 13 on): where a change is to a block entity's data
  (`docs/adr/0026-redstone.md`, section 6), `changes` hold the blocks alone, and this
  is a count (u32) followed, per change, by the number of block changes before it
  (u32), i32 x, i32 y, i32 z and its data as a length u32 and the bytes, ascending by
  that number, so that the two together are the tick's changes in the order it made
  them.
- **Events** (kind 14 on): a count of chunks (u32), and per chunk whose kept block
  events changed i32 x, i32 z and the chunk's events whole, as in a chunk file.
- **Entity changes** (kind 15; in kind 16 after a flag): what the tick did to entities
  other than players, as below. Not none.
- **Arrivals** and **arrivals taken** (kind 16): the entities the tick sent across a
  line, a count (u32) and each an arrival; and the numbers of the arrivals it took,
  ascending. Not both none in kind 16.
- **Claims** (kind 19): a count of chunks (u32), not 0, and per chunk whose claims
  changed i32 x, i32 z and the chunk's claims whole, as in a chunk file (here a chunk's
  list may be empty, when its last ticket was given back). A replay puts them into the
  chunk after the commit's blocks, so that a claim on a block placed in the same tick
  holds, and a chunk with claims is no longer as generated.

Each kind is written only when it has something of its own: the reader refuses a kind 8
without notes, a kind 9 without ticks, marks taken or seams, a kind 10 without effects
or taken, a kind 13 without block entities, a kind 14 without events, a kind 15 without
entity changes, a kind 16 without arrivals or arrivals taken and a kind 19 without
claims. The counts of the
fields a later kind takes over from an earlier one may all be 0. It refuses besides a
block entity's place among the blocks that goes back or beyond them.

**Entity changes** (`entities.rs`) are a count of runs (u32), which is not 0, and per
run the chunk's x i32 and z i32, a count of changes (u32), which is not 0, and the
changes: the changes in the order the tick made them, cut where the chunk changes. A
change is a tag u8 and its body: 1 whole, the entity; 2 gone, its key u64; 3 set, its
key u64, a count of fields u32 and the fields. A whole entity that rides nothing and
whose position is not in the chunk it is written under is refused. A field is a tag u8
and its value:

| Tag | Field | Value |
|---|---|---|
| 1 | position | 3 × f64 |
| 2 | motion | 3 × f64 |
| 3 | rotation | yaw f32, pitch f32 |
| 4 | on the ground | a flag |
| 5 | data | a tag and its data, as in an entity |
| 6 | `lived` | u32 |
| 7 | fall distance | f64 |
| 8 | fire | i32 |
| 9 | riding | as in an entity |
| 10 | `fresh_at` | u32 |
| 11 | hops | u32 |
| 12 | a mob's body part | id u16, version u8, length u32 and the bytes |
| 13 | a mob's body part gone | id u16 |
| 14 | a mob's mind part | as 12 |
| 15 | a mob's mind part gone | id u16 |
| 16 | a mob's type's state | version u8, length u32 and the bytes |

An **arrival** is the chunk it is sent to, i32 x and i32 z, the entity, and when it is
due (as an effect's). An entity that rides nothing and is not in that chunk is refused;
a passenger goes to its root's. A filed arrival (kind 18) is a count u32 and per arrival
its number (sequence u64, index u32) followed by the arrival.

### Stay notes

A note is what a region says to the store about a stay of a player
(`docs/adr/0020-one-stay-per-player.md`, section 3). Per note:

| Field | Type |
|---|---|
| Kind | u8: 1 `Entering`, 2 `Has`, 3 `Has` with a unit, 4 a taking |
| Player | u128 |
| Entity | u64: the stay's key, or for a taking the root of the unit taken |
| *2 and 3 only:* hops | u32 |
| … the place | below |
| *3 only:* the unit | a count (u32), which is not 0, and each member as an entity, root first |

A unit is the vehicle the player left the world on, with all its passengers
(`docs/groundwork/n7h-logout-on-a-mount.md`), kept with the player's record until the
home region takes it back for the player's next stay; a taking says it did.

A place is where a player is and what they hold:

| Field | Type |
|---|---|
| Position | f64 x, f64 y, f64 z |
| Yaw, pitch | f32, f32 |
| Flags | u8: bit 0 on-ground, bit 1 flying; the others 0 |
| Selected slot | u8 |
| Inventory | 41 times, in the order of the game's `Inventory` (0 to 8 the hotbar, 9 to 35 the rest, 36 to 39 armour from the feet up, 40 the off hand): u8 0 for an empty slot, or u8 1 and a stack |
| Ender chest | 27 times, as the inventory's slots |
| Experience: level, progress to the next (0 to 1), points gathered in all | i32, f32, i32 |
| Respawn point set by a bed | u8 0, or u8 1, the bed's head i32 x, y, z, then yaw f32 and pitch f32 |
| Effects: count, then each as an effect instance of a stack's `potion_contents` | u32 |
| Ticks since the player last slept | u32 |

The store takes the notes of a commit when it takes the commit, and keeps of each
player what they make of the player's record ("The players").

### Effects

Effects are what a region asks of a block or an entity it does not hold
(`effects.rs`; `docs/adr/0025-effects-between-regions.md`, sections 4 and 5). In a
commit, `effects` is a count u32 and per effect the effect followed by when it is due:
u8 0, counted from its taking, or u8 1 followed by the holder's tick u64. An effect's
number is its sequence u64 and its index u32; `numbers` (kinds 11 and 17, and `taken`)
are a count u32 and the numbers, ascending, each once; `filed effects` (kind 12) are a
count u32 and per effect its number, the effect and when it is due, ascending by
number, each once.

An effect is the block it acts on as i32 x, i32 y and i32 z, then a kind u8 and what
that kind carries:

| Kind | What follows |
|---|---|
| 1, an update | direction u8 (0 to 5); kind u8 (0 `updateShape`, 1 `neighborChanged`); the block that changed u16; flags u32; depth i32 |
| 2, a scheduled tick | u8 1 and a block u16, or u8 2 and a fluid u16 (below 256); delay i32; priority i8 |
| 3, a spread | direction u8 (0 to 5); the source's block u16 |
| 4, a set if still | the block expected u16; the block to set u16; flags u32 |
| 5, on an entity | its key u64, then a use |
| 6, exploded | the block expected u16; the blast |
| 7, burnt | the block expected u16 |
| 8, a player inside | the box's lower corner 3 × f64 and its upper corner 3 × f64 |
| 9, given | the player's key u64; the stack; a flag whether no duplicates |
| 10, seated | the player's key u64; the vehicle's key u64; its root's chunk x i32 and z i32 |
| 11, on a player | the player's UUID u128; their entity's key u64; then a use |
| 12, a release | the point of interest's type u8, by its registry id |

The blocks of kinds 1, 3, 4, 6 and 7 are state ids, and one that does not exist is
refused; so is a direction above 5, a kind of update above 1, a tick of another kind
than 1 or 2, and flags that mean nothing. A use of an entity is
a kind u8 and what that kind carries. An option is u8 0, or u8 1 and the value; a flag
u8 0 or 1; a list a count u32 (refused beyond the bytes left) and its elements; a
string a length u32 and UTF-8; an effect instance as a stack's `potion_contents` has
one; a causer its key u64, type u16, feet 3 × f64, an option of the player's UUID u128,
and a flag whether it is living.

| Use | What follows |
|---|---|
| 1, an attack | the attacker's key u64; flags u8 (bit 0 creative, bit 1 sprinting); the damage f32; where they stand 3 × f64 |
| 2, a use | the player's key u64; flags u8 (bit 0 creative, bit 1 sneaking); the stack held, an option of a stack; where the click landed 3 × f64; where they stand 3 × f64 |
| 3, an explosion's hit | the damage f32; the push 3 × f64; the blast |
| 4, a request to breed | the asker's key u64; the request's number u32 |
| 5, its answer | the partner's key u64; the number u32; a flag whether accepted |
| 6, a mob's hit | the attacker's key u64; its type u16; where it stands 3 × f64; the damage f32; the knockback, an option of 3 × f64; the sound, an option of i32 |
| 7, a use from an edge | as 2, followed by the player's edge u64 |
| 8, a seat declined | the player's key u64 |
| 9, a flung mob's hit | as 6, followed by the iron golem's fling f32 |
| 10, a thrown item's hit | the projectile's key u64; its type u16; where it was 3 × f64; its motion 3 × f64; the damage f32; the thrower, an option of its key u64, type u16, feet 3 × f64 and an option of the player's UUID u128 |
| 11, a guardian's spikes | as 6: a mob's hit of `thorns`, which flings nothing |
| 12, a hurt | the damage type, a string; the cause and the direct entity, each an option of a causer; the position and the projectile's motion, each an option of 3 × f64; the amount f32; the knockback, an option of 3 × f64; the fire before i32 and after i32; the effects, a list of instances; a flag whether by an arrow; the extras, a list, each u8 1 a zombie's fire, 2 a husk's hunger, or 3 a fling followed by its f32; the sound, an option of i32, volume f32 and pitch f32; the hit, an option of the attacker's key u64 and its `lived` u32 |
| 13, an alert | the attacker's key u64; its type u16; the class u16 |
| 14, effects | the instances, a list; the instant factor f32; the source and the owner, each an option of a causer; a flag whether water |
| 15, a pearl landed | where 3 × f64; a flag whether an endermite comes |
| 16, a patrol's target | x i32, y i32, z i32 |

A blast is the exploding entity's key u64, the tick u64, the centre 3 × f64, the radius
f32, flags u8 (bit 0 its drops decay, bit 1 it has a direct source, bit 2 it has an
owner, bit 3 the owner is a creative player; the others 0) and, with an owner, the
owner's UUID u128.

### Regions in the log

An opened record says that an owner opened the region and was restored up to tick
`restored`; a commit of that region before it in the log with a later tick is not part
of the region's history.

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

Kinds 11 and 12, and 17 and 18, keep the effects and the arrivals that are filed and
not yet taken: 11 and 17 say that those of these numbers were dropped untaken (the
chunk an effect acts on stopped being used; arrivals a fold at a change of division
wrote into their chunks), and 12 and 18 write untaken ones again, with their numbers,
ahead of a segment that is let go.

### Segments

A process can die while appending, which leaves a partial record at the end of the
segment it was appending to. Reading a segment stops at the first record that is
incomplete or fails its checksum. Nothing is appended to a segment after that: a store
that starts appends to a new segment, and one that fails to write or sync cuts the
segment back to what was durable, syncs it so that what was cut off is durably gone,
and goes on in a new one.

A segment is removed once nothing in it is needed any more and every segment before it
is gone: no commit that a region is restored with, no record of a merge or a split that
is a region's latest whole state, nothing the table file does not have, no stay note
the players' file does not have, and no effect or arrival that is filed and untaken
unless it was carried forward. Commits are not removed one by one: one that a
checkpoint covers is passed over because its tick is not above that of the region's
latest whole state. A segment that was removed can be there again after a crash, as
removing it is not made durable; it is read like any other, and nothing in it counts.

## Region files

A region file (`crates/clustine-format/src/region.rs`, kind 1):

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 1 |
| The highest epoch the region was opened with | u64 |
| The first entity key of the region's block, and the one beyond it | u64, u64 |
| CRC-32 of everything before | u32 |

The reader refuses a block that reaches beyond bit 63, which only keys of entities made
with a chunk have. A state file (kind 2):

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 2 |
| The tick the state is as of | u64 |
| The length of the state | u32 |
| The state, which the store does not look into | bytes |
| CRC-32 of everything before | u32 |

Both are written under a temporary name, made durable and renamed, and the directory is
synced after the rename.

A block of entity keys is the 2^32 keys whose high half is its index; block 0 begins at
key 1, since 0 stands for none. Every time a region is opened it is given a fresh block,
so that keys a lost tick gave out are never given to another entity, and blocks only
rise. A region that was made by a split and not opened since has none, and its file has
0 for both keys. A region file that names a block is never removed, also when its
region is gone, so that no block is issued twice; one that names none is removed with
its region.

The blocks' file, `regions/blocks` (kind 6), says how far blocks have been reserved:
none at or above `end` was ever given (`docs/adr/0028-entities.md`, section 1). It is
raised sixteen blocks at a time, so that most openings sync only their region's file.
A store that starts gives the next block above both `end` and every block a region file
names.

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 6 |
| `end`: the first block not reserved | u32 |
| CRC-32 of everything before | u32 |

## The table of regions

`regions/table` (kind 3) is to the list of regions and their chunks what a state file
is to a region: all of it as of a place in the log, written so that the log before that
place can go.

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
| The world's clock: its game time and the ticks of the overworld's clock | u64, u64 |
| CRC-32 of everything before | u32 |

An area is a u8 of flags (bit 0: it has a western end, bit 1: an eastern end; the others
0) followed by i32 `min_x` and i32 `max_x`, each 0 if absent: the chunks with
`min_x <= x < max_x`, whatever their z. The reader refuses regions or grants out of
order or twice, and an end that is absent and not 0. The clock is the later of every
region's the store took (`docs/groundwork/n7-plan.md`, section 8.4).

The table as the store has it is this file with the records of kinds 4 to 7 applied
that are in segments from `from` on, in the order of the log. Records in earlier
segments are in the file already. The file is written when the world is first started,
when it is made over for another division, and whenever a checkpoint leaves the first
segment of the log needed for nothing but the table: then `from` becomes the next
segment, and the segments before it go.

## The players

`regions/players` (kind 4) is to what the store keeps of each player what the table is
to the list of regions: all of it as of a place in the log
(`docs/adr/0020-one-stay-per-player.md`, sections 2 and 8).

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 4 |
| `from`: the first log segment whose stay notes are not in this file | u64 |
| `issued`: the highest entity key a stay was ever given, 0 if none was | u64 |
| Records, ascending by player, each once: count, then per record | u32 |
| … player, stay, hops | u128, u64, u32 |
| … what follows | u8: 0 nothing, 1 a place, 2 a place and a unit |
| … the place, for 1 and 2, as in a stay note of the log | |
| … the unit, for 2, as in a stay note of the log | |
| CRC-32 of everything before | u32 |

A record is of one player: the entity of the latest stay the store was told the
player was given, the highest number of hand-overs it was told of that stay, where the
player was last, if a region has said, and the vehicle they left the world on, until
their next stay takes it.

The records as the store has them are this file with the stay notes applied that are in
commits in segments from `from` on, in the order of the log, but for a commit with a
tick above what a later `Opened` record of its region was restored up to: such a commit
is no part of the region's history, and neither are its notes. A world without the file
has no records, and every note of its log counts. The file is written whenever a
checkpoint leaves the first segment of the log needed for nothing but the notes in it,
after the table file has been looked at in the same way, and before a world that has a
record is made over for another division, which fails the start if the file cannot be
written. `from` becomes the next segment each time, and **no segment is ever numbered
below it**: a store that starts with no segment left numbers its next one from the
file's `from`.

## The couplings

`regions/couplings` (kind 5) is what the store keeps of the lockstep of regions whose
border ticks: each region's offset to border time, the couplings, the handshakes under
way and the regions given up, whole, rewritten in the group that changes any of it
(`docs/adr/0025-effects-between-regions.md`, section 8).

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 5 |
| Regions with an offset or given up, ascending by id: count, then per region | u32 |
| … id | u32 |
| … has an offset, then the offset (0 where it has none) | u8 0 or 1, i64 |
| … given up | u8 0 or 1 |
| Couplings, ascending, the lower id first: count, then per pair | u32, (u32, u32)… |
| Handshakes, ascending, the lower id first: count, then per handshake | u32 |
| … the two regions | u32, u32 |
| … the tick each joins at, if it has said: flag and tick (0 where it has not), for each | u8, u64, u8, u64 |
| CRC-32 of everything before | u32 |

The reader refuses lists out of order or twice, a pair whose first id is not below its
second, and a flag other than 0 or 1.

## Regions

- **Who holds a chunk**: the region it is granted to; else the region that is pinned to
  an area which contains it; else nobody. Only the holder loads and saves a chunk.
- The store is started with a **division**: the areas of the pinned regions, and the
  chunk players enter the world in, the home chunk, which is the chunk of the world's
  spawn. Region `i` is pinned to area `i`. The home region is the pinned region whose
  area has the home chunk, or else a region made after the pinned ones and granted that
  chunk. A world is started with no pinned region unless its store is told `--pin`,
  which pins regions side by side.
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
  the change of its state, and whatever else of the kinds above the tick made. The
  store appends what all regions committed meanwhile to the log, syncs it once, and
  only then answers each commit.
- A chunk is saved once the commits asked for before it are durable; a chunk on disk
  therefore never holds a change that is not committed.
- At a **checkpoint**, a region saves every loaded chunk of its own that has changed and
  hands the store its whole state as of a tick. Once the saves before it are durable,
  the store writes the state file, and the commits up to that tick are passed over from
  then on. Commits after it stay. Checkpoints happen every five minutes by default and
  when the server stops.
- When a region is opened, the store hands its owner the state file and the state of
  every commit with a later tick, in the order of their ticks, applies the changes of
  those commits to the chunks they are in, and saves those chunks. The commits stay in
  the log until a checkpoint covers them. A chunk may have been saved with some or all
  of them in it already; applying all of them again in order gives the same result,
  because each change sets a block, or a block entity's data, to a definite value.
- Every time a region is opened the store gives it a fresh block of entity keys, and
  writes the region file before the owner is answered.
- A chunk a region gives back is free once the saves the region asked for before are
  durable; only then is the `Returned` record written.
- Saved chunks are written together when they are made durable, at a checkpoint, a
  return, a flush, the opening of a region, or when 128 of them wait: every file under
  a temporary name (`<x>.<z>.chunk.tmp`, `<x>.<z>.entities.tmp`), all of those synced
  at the same time, each renamed into place, and their directories synced at the same
  time (`docs/adr/0018-a-checkpoints-chunks-written-together.md`). Where a chunk is
  stored for the first time, its entities' files go in such a round of their own
  first. An entities' file of a chunk whose entities are now none is removed in the
  same round. A file under a temporary name that a crash left is never read, and is
  written over by the next save of its chunk.
- A chunk that was generated for a region is noted for writing when the region is
  answered, and is written with the next checkpoint, return, flush or opening of any
  region, when 512 such chunks wait, or when the store is stopped. A chunk stored for
  the first time is noted whole, its entities with it; one made again over a stored
  one only its blocks, its stored entities kept.
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

A chunk's blocks file or entities' file with a wrong checksum is reported as an error
and the chunk is not loaded; so is one the reader refuses for another reason
("Chunks"). The region that asked for it is told that it cannot be read, and leaves a
hole in the world. It is deliberately not replaced by a freshly generated chunk, which
would hide the damage and overwrite it at the next save. Block changes of commits that
would be put into such a chunk when its region is opened are not applied, and the file
stays as it is. A region file, a state file, the table, the players' file, the blocks'
file or the couplings' file that cannot be read stops the store when it starts, naming
the file.

## Not stored yet

Of a chunk: the structures it belongs to other than the starts of the kinds it keeps,
how long it has been inhabited and when it was last updated. The components of the
stacks in a container block's entity.
