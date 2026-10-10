# ADR-0019: Data made from Mojang's jar, and the notices for what is ported

- Status: **Accepted**, after an independent review against the jar, the code and
  SteelMC's branch, whose twelve findings are worked in (see "Review"). The owner
  decided what may be committed (the roadmap, "What the owner answered on
  2026-10-10"); the choices this record makes beyond that are listed at its end and
  can each be turned. The record that step G4 of
  [the terrain plan](../groundwork/terrain-plan.md) calls "data and licences". It
  amends [ADR-0004](0004-game-data.md) ("ids and names only") and adds to
  [ADR-0002](0002-licence.md) what the licence statement says about data that is not
  Clustine's. It changes `tools/datagen`, `crates/clustine-data`, a new crate
  `crates/clustine-worldgen-data`, the world store's start, the command line and the
  `generator` line of a world's `meta`.
- Date: 2026-10-10

Where a figure or a statement is a guess it says **guess**. Nothing here is legal
advice; the parts on licences are a reading and are marked so.

## Context

ADR-0004 lets `cargo datagen` commit Rust tables of ids and names made from the
official server jar, so that a build needs only cargo. A generator that is block for
block equal to the official server needs far more of the jar than names: its
world-generation data (2.0 MB of JSON), values that are in its code and not in its
data (what light a block gives, its shapes, its fluid, which class of block it is,
which climate is which biome), and its buildings (1,511 structure templates).

The owner has decided three things (`docs/roadmap.md`, "What the owner answered" on
2026-10-08 and 2026-10-10):

1. Rust generated from Mojang's world-generation data may be committed, so that a
   build needs no jar.
2. Tables made from the jar's code and registries may be committed too, with a notice
   that they are Mojang's data and not under the AGPL.
3. Structure templates are **not** committed. Their sizes and connection points are;
   the blocks are read from the operator's own jar when a server starts.

The generator is ported from SteelMC's 26.3 branch (AGPL-3.0-or-later), which
ADR-0002 chose the licence for. Neither SteelMC nor Pumpkin shows a way to follow:
SteelMC downloads the jar at every fresh build, embeds Mojang's template files in
its binary and commits extracts of the game's code (`blocks.json` 22.8 MB,
`steel-core/build/classes.json` 381,818 bytes); Pumpkin commits 85.9 MB of generated
Rust, 22.0 MB of it the templates, and its build still needs the jar (check §5).

### What datagen does today

Read from `tools/datagen/src` at `6bf1613`:

- `jar.rs` pins one jar (`SERVER_JAR`: version 26.3, Mojang's URL, SHA-1
  `33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c`), downloads it into
  `target/datagen/26.3/server.jar` unless a copy with that SHA-1 is there, and runs
  the jar's data generator (`-DbundlerMainClass=net.minecraft.data.Main --all`) into
  `target/datagen/26.3/generated`. A stamp file marks a complete run, which is then
  reused.
- `model.rs` reads the reports (`registries.json`, `blocks.json`, `packets.json`), the
  tags of the generated data pack and `version.json` of the jar.
- `emit.rs` renders ten Rust files as strings, laid out by itself with one entry a
  line, into two directories that hold nothing else and are under
  `#[rustfmt::skip]`: `crates/clustine-data/src/generated` and
  `crates/clustine-protocol/src/generated`.
- `cargo datagen --check` builds the same strings in memory, compares each with the
  file on disk, names every file that differs, is missing, or is in one of the two
  directories without being an output, and exits with failure if there is any. It
  does not look into sub-directories and it reads files as text.
- The `Datagen` workflow on GitHub runs `--check` on pushes that touch
  `tools/datagen/**` or the two directories, with a toolchain that is whatever
  `stable` is that day and Temurin 25. `Cargo.lock` is not among its paths. The four
  commands of `CLAUDE.md` and `tools/check.sh` do not run it.

### What was measured

From the pinned jar, read in memory with Python's `zipfile`, its class files parsed
for their constants and member tables, never run. The SHA-1 of the file read is the
pinned one. Sizes are bytes. "jar" is the inner `server-26.3.jar` unless it says
otherwise.

| What | Measured |
|---|---|
| The outer jar | 62,294,556; the inner jar 26,652,008; 39 library jars (`META-INF/libraries.list`), 42,901,556 |
| `data/minecraft/` | 9,883 files, 9,822,164 |
| `worldgen/` | 1,144 files, 1,999,195 |
| `worldgen/density_function` | 55 files, 328,221 (111,743 without white space). The largest: `overworld/offset.json` 53,460 and `overworld/factor.json` 29,062, both splines, each three times (overworld, amplified, large biomes) |
| `worldgen/noise_settings` | 7 files, 26,207; `overworld.json` 3,881, `nether.json` 3,990, `end.json` 3,441 |
| `worldgen/material_rule`, `material_condition` | 42 files, 58,834; 8 files, 1,051 |
| `worldgen/noise` | 64 files, 6,392 |
| `worldgen/biome` | 67 files, 288,500. Keys: `attributes`, `carvers`, `downfall`, `effects`, `features`, `has_precipitation`, `temperature` in all 67, and **`temperature_modifier` in two** (`frozen_ocean`, `deep_frozen_ocean`: `"frozen"`) |
| `worldgen/feature`, `placed_feature`, `block_state_provider`, `carver` | 240 files, 321,555; 273 files, 179,255; 8 files, 3,306; 4 files, 3,336 |
| `worldgen/structure`, `structure_set`, `processor_list` | 52 files, 24,104; 21 files, 7,882; 40 files, 68,898 |
| `worldgen/template_pool` | 245 files, 669,927 (471,277 without white space). 2,281 weighted elements: 1,665 legacy single, 531 single, 57 feature, 31 empty, 3 lists. They name 1,286 templates, **one of which the jar does not have** (`ancient_city/walls/intact_horizontal_wall_stairs_5`, named by `ancient_city/walls/no_corners`) |
| **How a block state is written in `worldgen/**`** | never as `{"Name", "Properties"}`. 488 objects `{id, properties}`, 81 objects `{id}`, and bare strings (329 under the keys `state`, `block`, `to_place` alone; `"state": "minecraft:coal_ore"` in `feature/ore_coal.json`) |
| `structure/` (templates) | 1,511 files, 4,236,847 as gzip files; 2,782,235 as stored in the jar; **45,290,384 when unpacked** |
| What the templates hold | 1,170,664 listed blocks in a volume of 1,850,264; 471 distinct blocks; palettes written as `{id}` and `{id, properties}`; 5,597 block entities, of which 4,272 jigsaws, 473 chests, 226 barrels, 201 structure blocks (data markers); 288 entities; 20 templates with several palettes (1,651 palettes in all), **none of which has a jigsaw**; every `DataVersion` is 5023; no side above 48 |
| Jigsaws | 4,272 in 1,291 templates, at most 47 in one. Fields: `name`, `target`, `pool`, `joint` (two values), `final_state` (always there, a string such as `minecraft:oak_stairs[facing=east,…]`), `selection_priority` (0 to 2) and `placement_priority` (0 to 3) in 1,347 and absent in 2,925, an empty `components` in 513; twelve orientations; no coordinate above 40. **In all 1,291 templates they stand in the file in the order y, then x, then z** |
| Templates no pool names | 226: woodland mansion 73, underwater ruin 48, shipwreck 20, end city 20, fossil 16, nether fossils 14, ruined portal 13, spring 10, village 5, igloo 3, desert well 2, ancient city 1, empty 1. They are used by structure code and by features (the G3 audit: two `fossil` features and seven nested `template` features, the desert well and the sulfur springs) |
| Biome parameter lists in the jar's data | two files of 71 bytes together: `{"preset": "minecraft:overworld"}` and `…nether` |
| `Climate` | holds the float constant 10000.0, `quantizeCoord`, `unquantizeCoord`; so a parameter is an integer of ten thousandths (the review read `long min, max` in `Climate$Parameter`) |
| `BlockBehaviour$BlockStateBase` | 94 public methods. Among them `getLightEmission`, `getLightDampening`, `canOcclude`, `useShapeForLightOcclusion`, `getFaceOcclusionShape(Direction)`, `propagatesSkylightDown`, `isLightPermeable`, `isAir`, `isSolid`, `isSolidRender`, `liquid`, `canBeReplaced()`, `getFluidState`, `hasBlockEntity`, `getPistonPushReaction`, `getCollisionShape(BlockGetter, BlockPos)`, `isCollisionShapeFullBlock(BlockGetter, BlockPos)`, `isFaceSturdy(BlockGetter, BlockPos, Direction, SupportType)`, `getPostProcessPos(BlockGetter, BlockPos) -> BlockPos`, `useWithoutItem`. **Nothing named for motion** |
| `Heightmap$Types` | names `BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP` and `BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES`, a test of a tag, and `isEmpty`. The jar has `tags/block/blocks_motion_in_heightmap.json` (= `#blocks_motion`), `…_no_leaves.json` (= `#blocks_motion_no_leaves`), `blocks_motion.json` (= `#blocks_motion_no_leaves` + `#leaves`) and `blocks_motion_no_leaves.json` (340 entries, stone among them) |
| `WorldGenRegion` | names `getPostProcessPos` and `markPosForPostProcessing` |
| `BlockSetType` | has `canOpenByHand` |
| Block states | 35,723 in 1,286 blocks (`BLOCK_STATE_COUNT`, `BLOCKS` in the committed tables) |

From the reference clones and the repository:

| What | Measured |
|---|---|
| Committed generated output today | 754,040 (730,721 in `clustine-data`, 23,319 in `clustine-protocol`) |
| All tracked files today | 9,854,351; the pack is 802 KiB |
| The block tags named above | committed already: `tags.rs` has `blocks_motion_in_heightmap` and `…_no_leaves` |
| SteelMC's committed list of biome parameters | 7,594 entries for the overworld, 5 for the Nether; 3.78 MB of JSON |
| What SteelMC's layout reads of a template | size and jigsaws only (`S263:steel-worldgen/src/structure/jigsaw.rs:270-590`, by the review); `final_state` and the NBT only when placing. It sorts jigsaws by y, x, z "to match vanilla's `buildInfoList`" (`S263:steel-registry/build/structure/template_pools.rs:219-228`) |
| How SteelMC orders registries it emits | structures, sets, pools and templates sorted by name (`S263:steel-registry/build/structure/sets.rs:1068, 1156`, `template_pools.rs:351, 386`); a structure's index within its step seeds it (`steel-core/src/worldgen/feature/runner.rs:297-302`) |
| How SteelMC lays out emitted Rust | token streams, then the toolchain's `rustfmt` if a switch is on (`S263:steel-worldgen/build/build.rs:36-38, 62-64`) |
| SteelMC's extract of block classes | `classes.json`: a class for every block, and per class such things as `type_can_open_by_hand` (76 entries), `support_blocks`, `growth_direction`, `tree_grower_name` |
| Pumpkin's generated Rust, for scale | `noise_router.rs` 1,816,455; `configured_features_generated.rs` 641,960; `placed_features_generated.rs` 395,984; `template_pool.rs` 946,240; `structure_metadata.rs` 3,049,415; `biome.rs` 14,298,616; `block.rs` 23,953,854; `structure_template.rs` 22,024,276 |
| Rounding of parameters | of the 50,001 integers from −25,000 to 25,000, 3,182 do not come back from `(long)((float) n / 10000f * 10000f)` (the review's figure, computed again) |

**The data generator's output is not on this machine** (a reboot cleared it). What it
holds is taken from the plan's "What the first trials found" (G0):
`reports/biome_parameters/minecraft/overworld.json` and `nether.json` hold the
climate parameters of every biome, and `reports/blocks.json` has the properties and
the state ids of every block and nothing else. The constants of the two report
classes in the jar agree with that.

## Decision

**The repository holds what the pinned jar's data and code say, turned into Rust and
packed tables by `cargo datagen`, and nothing that is one of Mojang's files or one
of its buildings. A server gets the buildings from a jar its operator names. A world
is made with them or, if its maker says so in so many words, without them, and
records which.**

### 1. What becomes committed output, where, and in which form

Three forms:

- **Code**: Rust functions, emitted. For what is a computation in the data.
- **Statics**: Rust `static` and `const` values of types that are written by hand.
  For records of many shapes, a few hundred of each.
- **Packed**: a binary table, little-endian, in a file that the crate takes in with
  `include_bytes!`. For thousands of rows of one shape, which nobody reviews line by
  line and which cost nothing to compile.

| # | Input | From | Committed as | Form | Size |
|---|---|---|---|---|---|
| 1 | Ids and names of packets, blocks, items, entity types, registries, tags, dimension types (ADR-0004) | reports, generated tags | as today | statics | 754,040, measured |
| 2 | The properties of every block with their values in order, so that a block's name and properties give its state id | `reports/blocks.json` | `clustine-data/src/generated/block_properties.rs` | statics | **guess** 80 KB |
| 3 | Per block state: the row of section 5 | the Java program; two bits from the block tags | `clustine-data/src/generated/block_states.bin` | packed | 571,568 for the rows (16 bytes a state), and **guess** under 60 KB of shapes |
| 4 | Per block: its class, its block entity type, whether a click uses it, and the arguments of its class that a ported behaviour reads | the Java program | `clustine-data/src/generated/block_classes.rs` | statics | **guess** 100 to 150 KB (SteelMC's extract is 381,818 for blocks and items together, as JSON) |
| 5 | Noise routers of the overworld, the Nether and the End, with every density function they reach | `worldgen/noise_settings/{overworld,nether,end}.json`, `worldgen/density_function/**` | `clustine-worldgen-data/src/generated/{overworld,nether,end}/router.rs` | code; splines as statics beside it | **guess** 300 to 800 KB together; measured before G5 (section 6) |
| 6 | Material rules and conditions of the three dimensions | `worldgen/material_rule/**`, `material_condition/**` | `…/{overworld,nether,end}/material_rule.rs` | code | in the guess of row 5 |
| 7 | Noise parameters; the rest of the three noise settings (aquifers, sea level, heights, spawn target, legacy random source) | `worldgen/noise/**`, `noise_settings` | `…/noises.rs`, `…/noise_settings.rs` | statics | **guess** 15 KB |
| 8 | Which climate is which biome: the two parameter lists | the Java program; the report as a cross-check | `…/tables/biome_parameters.bin` | packed | 205,173 for 7,599 rows of 27 bytes, if SteelMC's count is the jar's |
| 9 | Per biome: features by step, carvers, temperature, **temperature modifier**, downfall, precipitation | `worldgen/biome/**` | `…/biomes.rs` | statics | **guess** 50 KB |
| 10 | Features, placed features, block state providers, carvers | the four directories | `…/features.rs`, `placed_features.rs`, `providers.rs`, `carvers.rs` | statics | **guess** 0.5 to 1.0 MB (Pumpkin's two files are 1.04 MB) |
| 11 | Structures, structure sets, processor lists | the three directories | `…/structures.rs`, `structure_sets.rs`, `processors.rs` | statics | **guess** 150 KB |
| 12 | Template pools; a template is named by its index in table 13 | `worldgen/template_pool/**` | `…/template_pools.rs` | statics | **guess** 200 to 300 KB for 2,281 elements |
| 13 | Of every template: its name, its size, and of every jigsaw in it the position, orientation, `name`, `target`, `pool`, `joint` and the two priorities | `structure/**/*.nbt` in the jar | `…/tables/templates.bin` | packed | about 152 KB, computed from the counts above |
| 14 | The jar's pin for the server: version, SHA-1, URL | `jar.rs` | `clustine-data/src/generated/version.rs` | statics | under 1 KB |
| 15 | Values to test against (section 5) | the Java program | `crates/clustine-worldgen-data/reference/` | text, one value a line | **guess** under 200 KB |
| 16 | The sums of section 3 | datagen | `tools/datagen/generated.sums` | text | a few KB |

In all: 2.6 to 3.7 MB beside today's 0.75 MB, most of it guessed.

Why each form:

- **Routers and rules are code** because that is the owner's decision 1 and SteelMC's
  way, and the speed of section 6 of the plan rests on it. Splines stay data that a
  hand-written evaluator reads, as in SteelMC (`spline_eval.rs`); the two overworld
  splines are 82 KB of JSON and would be the longest functions otherwise.
- **Features, structures, pools, biomes, block classes are statics** because their
  records have dozens of shapes (47 feature types at the top level alone). A packed
  form would need a decoder for each; a static needs none, and a new version is a
  readable diff.
- **Per-state block values, biome parameters and template metadata are packed**
  because they are 35,723, 7,599 and 5,783 rows of one shape. As literals they are
  what made Pumpkin's `block.rs` 24 MB; the template metadata alone is 1.2 MB as
  literals.

**How emitted Rust is laid out.** By the emitter itself, as `emit.rs` does today:
text written line by line, one statement or one entry a line, no line over 100
columns where a literal allows it. It is never passed through `rustfmt` or any
formatting library, and no token streams are used, so the bytes do not depend on a
toolchain. This departs from SteelMC's emitter on purpose. The generated module of
the new crate is under `#[rustfmt::skip]`, as the two today are, and under
`#[allow(clippy::all)]` with the reason beside it (the code is emitted; a lint would
be answered in the emitter or not at all).

**A packed file** starts with a header: the four bytes `CLT1`, the kind of table
(`u16`), the version of its layout (`u16`), the SHA-1 of the jar (20 bytes), the
number of sections (`u16`), and for each section its number of rows and the bytes of
a row (`u32`, `u32`). The sections follow in order with nothing between them, so the
header gives the file's length. The crate that includes a table checks that length
and the SHA-1 against `version.rs` in a test, and in a constant assertion where the
compiler can. `cargo datagen --dump <table>` prints a packed table as text, one row
a line with the block's or the template's name, so that a version update can be read
(`git diff` can be given it as a `textconv`). A `.gitattributes` marks `*.bin` as
binary and everything under the generated directories, `reference/` and
`generated.sums` as `-text`, so that no checkout changes line ends under the sums.

The layouts, to build and to test from:

- `block_states.bin`: section 1 has one row of 16 bytes for each state id in order,
  as section 5 defines it. Sections 2 to 5: the sets of six face shapes (six `u16`);
  the distinct face shapes (rectangles of four `f64`, written as their bits, with a
  section of offsets into them); the distinct collision shapes (boxes of six `f64`,
  likewise); the post-process offsets (three `i8`).
- `biome_parameters.bin`: one section for each list, rows in the list's own order
  (the order decides ties): twelve `i16` for the lower and upper bound of
  temperature, humidity, continentalness, erosion, depth and weirdness, one `i16`
  for the offset, one `u8` for the biome's id in Clustine's sorted biome registry.
  The numbers are the game's integers of ten thousandths. The emitter fails if one
  does not fit an `i16` (every value in SteelMC's extract does).
- `templates.bin`: a section of strings; a section of templates, 9 bytes each (name
  as `u16`, three `u8` of size, the index of its first jigsaw as `u16`, their number
  as `u8`, flags as `u8`: bit 0 "the jar does not have it"); a section of jigsaws, 15
  bytes each (three `u8` of position, orientation, joint, three strings as `u16`, two
  `i16` of priority). Templates are sorted by name.
  - **Jigsaws are in the order y, then x, then z**, which is the order of the file in
    every template of this jar and the order SteelMC sorts into. The emitter sorts
    and **fails if the file's order was another**, so that the day the two rules
    part is noticed.
  - A priority that the file does not have is 0.
  - The emitter **fails if a template with several palettes has a jigsaw**.
  - The template the jar does not have is a row of size zero without jigsaws, with
    bit 0 set. Its pool element **stays in its pool with its weight**, so that the
    shuffle of the pool is what the server's is.

The types of the statics and the traits the emitted code implements are written by
hand in `clustine-worldgen-data/src` (ported from SteelMC's data model, so under its
notice, section 7) and are fixed in G5. The crate depends on `clustine-data` and
`clustine-noise` and on nothing that uses it.

**What the emitter must do with what it reads:**

- A block state is written in three ways in the data, and a fourth in a jigsaw's
  `final_state`: a bare name (the default state), `{id}`, `{id, properties}`, and
  `name[key=value,…]`. Each becomes a state id at datagen time, resolved against the
  model datagen has **in memory** from this run's `blocks.json`, never against the
  committed crate (which is the last version's when the pin moves). A name or a value
  the model does not have fails the run. The store's reader of templates (section 4)
  resolves the first three and the fourth through the committed table 2.
- Every reference from one entry to another is resolved at datagen time. One that
  names nothing fails the run, with the one known exception above. (**Guess:** the
  official server logs it and treats the element as empty; the fixture of the
  ancient city decides.)
- It reads the three presets the plan builds and no other.
- A number that the game takes as a `float` is parsed from its decimal text to
  single precision, and the emitter **fails if reading it through a `double` gives
  another value**. For this jar the two agree for every literal the review could
  pull out (4,986, 290 distinct), and SteelMC reads through a `double` and matched
  the server in G2; the assertion keeps a later jar from differing in silence.

**The order of every registry that is emitted**, since order decides things (a
structure's index within its step seeds it; a pool is shuffled from its own order):

| Registry or list | Order committed | Why it is the server's |
|---|---|---|
| Block states, blocks, items, fluids, block entity types | the ids of the reports | they are the game's own ids |
| A biome parameter list | the list's own | dumped from the game by the Java program |
| Features of a biome, by step | the file's | it is the data |
| Elements of a template pool; processors of a list; rules of a material rule | the file's | it is the data |
| Members of a tag | the generated tag file's, as today | as the data generator wrote them. **Guess** that a running server holds them so |
| Jigsaws of a template | y, x, z (above) | SteelMC's reading of the game, equal to the files |
| Structures, structure sets | sorted by name | **not argued, only evidence:** SteelMC sorts so, and its fixture equalled the server in G2 for a cluster with a village and a mineshaft. The fixtures program (`tools/fixtures`, G7) dumps the running server's order of both registries, and an ignored test compares it with the committed order |
| Biomes, features, placed features, providers, carvers, pools, processor lists, noises, density functions | sorted by name | they are reached by name only. If the order of features across biomes turns out to rest on a registry's order, that is F1's to find; the same dump covers them |
| The End's biomes | not from any list: they are in the game's code | the Java program dumps them if it can without a server; otherwise the fixtures program does. **Not checked which** |

### 2. What stays out of the repository for good

| Never committed | Where it is instead |
|---|---|
| The jar, its inner jar and its libraries | `target/datagen/26.3/`, ignored |
| The data generator's output: reports and the generated data pack | `target/datagen/26.3/generated/`, ignored |
| Any JSON file of the data pack, as it is or reformatted | read from the jar by datagen |
| The Java program's output (the dump of block values, classes and biome parameters) | `target/datagen/26.3/extract/`, ignored |
| A template's blocks, palettes, block entities and entities, and each jigsaw's `final_state` (the block the jigsaw turns into is part of the building) | read from the operator's jar when a server starts, section 4 |
| Loot tables (they enter only as names), advancements, recipes, the 30 context providers and 3 block transformers (nothing in `worldgen/**` refers to them), and the rest of `data/minecraft/` | not read |
| Data packs made for fixtures, region files, block dumps of chunks (plan, section 4) | made where the comparisons run |
| SteelMC's and Pumpkin's committed extracts and SteelMC's fixtures | read in the reference clones; Clustine makes its own from the pinned jar, so that `--check` can prove them |

This replaces ADR-0004's third bullet. Its second bullet ("ids and names only")
becomes: "the outputs of ADR-0019, section 1". Its last consequence stands and now
covers more: hand-written code must not repeat what a table gives, which is why
table 4 exists.

### 3. How `cargo datagen` makes each, and what `--check` proves

`cargo datagen` keeps its two options and gets `--fresh` and `--dump`. In order:

1. **The jar.** As today: the pinned jar, by SHA-1, downloaded or given with `--jar`.
2. **The data generator.** As today, reused if its stamp is there.
3. **The Java program** (section 5). `tools/datagen/java/Extract.java` is compiled
   with `javac` against the inner jar and the 39 libraries and run with them on the
   class path, with no server. Datagen unpacks the libraries itself from the entries
   that `META-INF/libraries.list` names, with their checksums, and does not rely on
   what an earlier run left beside the stamp. The program's output is reused only if
   a stamp beside it holds the SHA-256 of the program's source and the jar's SHA-1.
   This needs a JDK 25, where datagen needed a Java runtime until now; the README
   says so.
4. **Reading.** Reports and tags from step 2; `worldgen/**` and `structure/**` from
   the inner jar's own entries, which is what a server loads; the dump of step 3.
   Templates are read by datagen's own NBT reader; one whose `DataVersion` is not
   the jar's world version fails the run.
5. **Emitting.** Every output of section 1 is made in memory as bytes, rows 15 and
   16 included.
6. **Budgets** (section 6) are checked on what was made; over budget fails the run
   before anything is written.
7. **Writing**, or with `--check` **comparing**: every output with the file on disk
   byte for byte, and every file under a generated directory, sub-directories
   included, that is not an output. The generated directories become four: the two
   of today, `crates/clustine-worldgen-data/src/generated` and
   `crates/clustine-worldgen-data/reference`.

`--fresh` ignores the stamps of steps 2 and 3.

**What the output is a function of, and how each is held still:**

| Source of difference | Closed by |
|---|---|
| The jar | its SHA-1 |
| The emitter and the Java program | they are in the repository; their hashes are in the sums (below) |
| Datagen's dependencies (`serde_json`, `zip`, `flate2`, the NBT reader) | `--locked`; `Cargo.lock` is in the workflow's paths and its hash in the sums |
| The Rust toolchain | emitted code is never formatted by it, and nothing emitted depends on a hash map's order, the machine's word size or a float printed as a decimal (floats are written as bits where a decimal would not give them back) |
| The JVM | the program writes integers with `Integer.toString` and `Long.toString` and floats as their bits with `Long.toHexString`, never through `format`; it writes files, in UTF-8 named explicitly, not standard output (the game's start wraps the streams); it takes rows in id order from the registries |
| The machine | nothing emitted has a path, a time or a host in it |
| A cache | stamps keyed as above; `--fresh` in the workflow |
| Line ends | `.gitattributes` |

**`tools/datagen/generated.sums`** is an output like the others. It holds the jar's
SHA-1, then one line with the BLAKE3 hash of each **input in the repository**
(every file under `tools/datagen/src` and `tools/datagen/java`, `tools/datagen/Cargo.toml`,
`Cargo.lock`), then one line for each **output**. A test of `clustine-datagen`, in
the ordinary tests, computes all of them again and compares. So without a jar it
fails when a generated file was edited, and **when the emitter, the Java program or
the lock file changed and datagen was not run again**, which is the common mistake
and would otherwise pass the four commands and turn the workflow red after the push.
The price is named: every change to `Cargo.lock`, also one that has nothing to do
with datagen, needs `cargo datagen` run again where a jar is, which then changes one
line.

The `Datagen` workflow runs `--check --fresh`, with `--locked`, and is triggered by
`tools/datagen/**`, the four directories and `Cargo.lock`. `tools/check.sh` runs
`cargo datagen --check` as a fifth check where the pinned jar is in the cache and
`javac` is found, and says that it skipped it otherwise.

What changes in the code: `Output.content` becomes bytes; `stale_paths` reads bytes
and walks sub-directories; `DIRECTORIES` gets two entries; single outputs outside a
directory are allowed.

**What a green `--check` proves:** that the committed output is, byte for byte, what
the datagen of this commit and the Java program of this commit make from the file
whose SHA-1 is pinned, with the dependencies of this lock file.

**What it does not prove:** that the emitter is right (T2 compares router values at
points with the jar's; the fixtures compare chunks); that a registry's committed
order is the server's (the table of section 1); that a table has every value a later
step needs.

### 4. Structure template blocks at run time

**Who reads them.** The world store, because only it generates
(`bin/clustine/src/cluster/worldstore.rs:26`, `bin/clustine/src/lib.rs:199-201`; a
worker and the edge name a generator only in tests). `clustine` without a subcommand
holds a store and behaves the same.

**The setting.** A world with terrain is made with structures built (`blocks`) or
not (`none`). The generator takes the setting as an argument that has no default in
the code; only the command line fills it in, by these rules. The flat world has no
structures, takes no such setting and records none.

Two options, on `clustine` and on `clustine worldstore`:

- `--minecraft-jar <path>`, or the environment variable `CLUSTINE_MINECRAFT_JAR`: the
  jar as Mojang publishes it (the bundler jar of 62 MB, not the inner one).
- `--structures <blocks|none>`.

**The store looks nowhere else for a jar.** Not in the working directory, not in a
checkout's `target`. What a world holds does not depend on files that happen to lie
somewhere, and `cargo run`, `cargo test` and a container make the same world from
the same options.

| Starting | With | The store |
|---|---|---|
| A **new** world | a jar named, `--structures` absent or `blocks` | checks the jar; the world is `blocks` |
| | no jar named, `--structures none` | **does not look for a jar**; the world is `none`; one line at the start says what that means (below) |
| | a jar named and `--structures none` | does not open the jar; the world is `none`; says so |
| | **no jar named, `--structures` absent or `blocks`** | **stops**, with: "A world needs the Minecraft 26.3 server jar to build villages and other structures. Give it with --minecraft-jar <path> (it is `<URL>`, SHA-1 `<sha1>`; `cargo datagen` leaves it in target/datagen/26.3/server.jar), or say --structures none to make this world without structures for good." |
| An **existing** world recorded `blocks` | a jar named | checks the jar; opens |
| | no jar named | stops, naming the option and the jar |
| An existing world recorded `none` | anything | opens; never looks for a jar, never fails on one |
| An existing world | `--structures` that is not what the world records | stops, as for any other setting of the generator that differs |

A jar that is named and is not the pinned one (by SHA-1, against `version.rs`) stops
the start with the version, SHA-1 and URL that are wanted, in every row where the
jar is checked. The store never downloads a jar.

**What it takes from the jar.** The 1,511 entries under `data/minecraft/structure/`
of the inner jar, 4.2 MB, kept in memory as they are; the file is then closed. A
template is unpacked and parsed when a piece first needs it, and kept: palette
entries (a bare name, `{id}`, or `{id, properties}`) become state ids through the
committed table 2, block entities stay opaque NBT, entities are dropped (Clustine
has none). Nothing is ever evicted: all 1,511 parsed are 1,170,664 blocks, a
**guess** of 10 to 15 MB, so the cache is bounded by the jar. A template whose size
or jigsaws differ from `templates.bin` is a bug in datagen and a panic with the
template's name; an ignored test walks all 1,511.

**What a world with `--structures none` looks like**, said plainly, in the record,
in the option's help and in the line at the start:

- **Nothing of any structure is built**: no village, no mineshaft, no stronghold, no
  pyramid, no temple, no monument, no shipwreck. That includes the structures the
  generator makes from code and needs no template for. Features that place a
  template place nothing either (fossils, desert wells, sulfur springs), and
  features that a village's pieces would have placed (its trees and flowers, the
  sculk of an ancient city) are not placed.
- **The terrain is still shaped for them.** Structure starts are made from the
  committed sizes and connection points, as with a jar, so that the terrain, the
  biomes and all other features are block for block the same in both kinds of world.
  So where the official world on the same seed has a village there is levelled
  ground with nothing on it; an ancient city and a trial chamber leave their carved
  and encased hollows; a stronghold leaves its burial.
- It is for good: the world records it, and a jar given later does not change it.

**How a world records it.** The `generator` line of `meta` carries
`structures=blocks` or `structures=none` beside the seed; the generator's version has
a line of its own (ADR-0021).
It is part of the generator's settings, so the store's rule for those applies.
Changing it for an existing world is the same question as a fix to the generator
after worlds exist, which [ADR-0021](0021-generation-is-a-function.md) decides;
nothing is offered here.

**Tests.** Every test that makes a world with terrain names the setting: `none`, or
`blocks` with a jar the test gives. Since `none` never looks for a jar, a test's
world is the same with `CLUSTINE_MINECRAFT_JAR` set or not. The four commands and CI
make `none` worlds; the comparisons with the official server, which have the jar,
make `blocks` worlds.

**What the owner runs**, written into the roadmap's "what to try" from phase T on:
`cargo datagen` once, then `cargo run --release -p clustine -- --seed 13579
--minecraft-jar target/datagen/26.3/server.jar`.

**Container images.** The image is built from the repository with cargo alone and
holds no jar. What it holds of Mojang's is what the repository holds: the tables of
section 1, compiled in. In a container the operator puts the jar where the store's
pod can read it (the store's volume is `/data`) and names it with
`CLUSTINE_MINECRAFT_JAR` or the option. The manifests in `deploy/kubernetes` name
`--structures=none` from the day they name a seed, so the cluster test depends on
nothing of Mojang's; `deploy/README.md` says in a paragraph how to give a jar
instead. Only the store's pod needs it. No downloader is added to the server's
binary (see "Ruled out").

### 5. What the Java program has to extract

One program, `Extract.java`, of Clustine's own, which calls the jar's public classes
after `Bootstrap.bootStrap()` and starts no server. It is written from the names in
the jar's class files and from SteelExtractor's calls (CC0); Mojang's source is not
read (plan, question 7). The reports lack all of what follows (G0).

**Where a call wants a level and a position** (`getCollisionShape`,
`isCollisionShapeFullBlock`, `isFaceSturdy`, `getPostProcessPos`), the program hands
it a `BlockGetter` of its own that answers "air, no fluid" and **notes that it was
asked**. A state whose answer consulted the level is named in the dump, and datagen
fails unless the block is in a short list in datagen that says what is done about
it. (**Guess** that few or none do.)

**The per-state row**, 16 bytes, defined from what 26.3's state offers (measured
above) and from what SteelMC's generation and light read (the review's count in
`S263:steel-core/src/worldgen` and `steel-worldgen/src`: `is_face_sturdy` 17 uses,
`is_solid` 13, `is_solid_render` 10, collision shape 5, `has_collision` 4, piston
reaction 4):

| Bytes | Field | From | For |
|---|---|---|---|
| 0 | light emission | `getLightEmission` | light |
| 1 | light dampening | `getLightDampening` | light |
| 2–3 | flags, one bit each, in this order: is air; is solid; is solid render; is a liquid; can be replaced; occludes; occludes by its shape; sky light passes down; is light permeable; has a block entity; collision shape is empty; collision shape is a full block; **is in `#blocks_motion_in_heightmap`**; **is in `#blocks_motion_in_heightmap_no_leaves`**; two spare | the methods of those names; the collision shape; **the two tag bits from the tag files, by datagen, not by the program** (tags are not bound without a server, by the review) | light, heightmaps, features, placing |
| 4, 5, 6 | which of the six faces are sturdy, for the support types full, centre and rigid: six bits each | `isFaceSturdy` | features (trees ask with a support type, `tree/mod.rs:304-312`) |
| 7 | piston push reaction | `getPistonPushReaction` | features |
| 8 | fluid: 0 for none, otherwise the fluid's id in `registries.json` plus one | `getFluidState` | aquifers, heightmaps, features |
| 9 | fluid level: the amount in the low bits, bit 6 for a source, bit 7 for falling | the fluid state | the same |
| 10 | post-processing: 0 for none, 1 for the position itself, otherwise 2 plus an index into the offsets (soul sand and magma mark the block above, by the G3 audit) | `getPostProcessPos`, asked at two positions; the program fails if the two answers are not the same offset | marks (phase B) |
| 11 | spare | | |
| 12–13 | the set of six occlusion face shapes; set 0 is "no face covers anything", set 1 "every face is full" | `getFaceOcclusionShape` for states that occlude by shape | light between two blocks |
| 14–15 | collision shape; 0 is empty, 1 the full block | `getCollisionShape` | features |

It is 571,568 bytes of rows. The two heightmap bits replace the first draft's
"blocks motion" and "is leaves by its class", which 26.3 does not have. How the four
heightmaps are put together from them is code in Clustine and a **guess** from the
class's constants: motion blocking is the first tag or a fluid; motion blocking
without leaves the second tag or a fluid; the ocean floor the first tag alone; the
world surface "not air". W2 compares heightmaps with the official server's and
decides. What `isLightPermeable` is, is not known; it costs a bit.

The row is fixed with the other shared shapes in G5. A value that is found missing
after that goes into a spare bit or byte; the row does not grow.

**Per block** (table 4):

- **Its class**, as the simple name of the block's Java class. The ported
  behaviours (`can_survive` 11 uses in SteelMC's generation, `update_shape` 7,
  `rotate` 15) are chosen by it, as SteelMC chooses by its `classes.json`.
- **Its block entity type**, if it has one, as the id in `registries.json`. (The
  first draft put this into the state's flags, where a type does not fit.)
- **Whether a click uses it** (W6): its class has a handler of its own for a click
  without an item, **and**, where the block has a block set type, that type's
  `canOpenByHand` holds. So an iron door and an iron trapdoor are not used by a
  click, and a block in hand is placed against them, as in the game. Named limit:
  a handler that answers by state or by what is held (a cake, a candle) is counted
  as used.
- **The arguments of its class that a ported behaviour reads** (such as which blocks
  support a plant, or which way it grows). Which they are is settled as each
  behaviour is ported; each is a named field of the class's entry, read by the
  program from the block. They are added in the step that needs them, and the
  budget of section 6 holds them.

**Besides:**

- The two biome parameter lists as the integers the game holds, in the game's order
  (`MultiNoiseBiomeSourceParameterList.knownPresets()` is public and static). The
  report has them as decimals. Datagen compares **forwards**: each integer of the
  program, divided by 10,000 as a `float`, must equal the report's number read as a
  `float`, or the run fails. (Quantising the report's number and comparing integers
  would fail on right values: 3,182 of 50,001 do not come back.)
- Values to test against, committed as row 15: noise and router values at points,
  the `Mth.sin` table and the beardifier's kernel as bits, hash-set orders for given
  insertions (plan G6, T1).

**Not from the Java program:** template metadata (datagen reads the NBT itself);
block properties and their values (the report has them); tags (the generated files);
anything that needs a running server, among it the order a server holds its
registries in (that is `tools/fixtures`' program).

### 6. Budgets, and how they are checked

| Budget | Limit | Checked by |
|---|---|---|
| All committed generated output (the four directories and the sums) | **6,000,000 bytes** | `cargo datagen` before it writes; a test of `clustine-datagen` that adds up the files, in the ordinary tests |
| Emitted code (form "code") | **1,000,000 bytes, provisional** | the same two |
| The longest emitted function | **300 lines** (the emitter splits a longer one; a line is at most 100 columns but for a literal) | the same test, by counting |
| Committed test data made by running the server (`tools/fixtures`: hashes, starts; plan section 4) | **1,500,000 bytes** | the same test |
| Building `clustine-worldgen-data` alone, from nothing, as the four commands of `CLAUDE.md` build it (the plan's test profile gives it `opt-level = 3`), and `cargo clippy` over it | **60 seconds each on the owner's six processors** | measured with `cargo build --timings`, written into this record. No test asserts a time |

The limits are this record's choice. The byte limits can be asserted; the time
cannot, by the project's rule that tests do not wait on a clock, so the two limits
on emitted code are its stand-in.

**The limit on emitted code rests on no measurement.** Nobody has seen SteelMC's
emitted size (its tree has no generated files and was never built here). So a step
is put **before G5**: the emitter's first version is run over the overworld's router
alone, its bytes, its longest function, its build time and clippy's time are
written into this record, and the limit and the fallback are settled then. It has to
be before G5 because the fallback (the Nether and the End interpreted from tables
instead of emitted) changes the traits that G5 fixes. The budget is not raised after
that without the owner.

### 7. Notices for code ported from SteelMC

A reading of sections 4 and 5 of the AGPL (check §1), not legal advice. SteelMC's
files carry no notice of their own and it has no `NOTICE`; the one notice to keep is
the end of its `LICENSE`.

- **A `NOTICE` file in every crate that holds ported code**: `clustine-noise`,
  `clustine-terrain`, `clustine-features`, `clustine-structures`, `clustine-light`,
  `clustine-worldgen-data` (the hand-written types) and `tools/datagen` (the
  emitter). It says:

  > Parts of this crate are adapted from SteelMC
  > (https://github.com/Steel-Foundation/SteelMC), branch `26.3` at commit
  > `885c4b3e60ed79862c37311780774f76806cb714`. Steel: A high-performance Minecraft
  > server implementation written in Rust. Copyright (C) 2026 Alve Jeansson and
  > contributors. Licensed under the GNU Affero General Public License, version 3 or
  > (at your option) any later version; see `LICENSE` at the root of this
  > repository. The files that were adapted say so at their head.

- **At the head of every ported file**, one comment: the SteelMC file it was adapted
  from, the commit, and that it was changed for Clustine and when. For example:

  ```rust
  // Adapted from SteelMC, steel-worldgen/src/noise/aquifer.rs at 885c4b3
  // (AGPL-3.0-or-later, Copyright (C) 2026 Alve Jeansson and contributors; see
  // NOTICE). Changed for Clustine in October 2026: scalar, stable Rust, Clustine's
  // chunk types.
  ```

  A file that takes from several names each. A fix taken from a later SteelMC commit
  adds that commit. The step that ports a file adds the header in the same commit,
  and whoever reads the step back checks it; no tool can tell a ported file from one
  of Clustine's own.
- **Emitted code** is made by an emitter that is itself adapted from SteelMC's, so
  its head says both (section 8).
- **SteelExtractor** is CC0 and asks for nothing. `tools/fixtures/NOTICE` and the
  head of `Extract.java` credit it all the same, at commit
  `e1e269595b4e059abaa16d1b440b4369c55dddff`. Whether it descends from Pumpkin's
  extractor (MIT) is not known; if it turns out to, both are credited.
- **Pumpkin** (GPL-3.0, nothing says "or later") is read and not copied, as before,
  and so needs no notice.
- G5 makes the empty crates with their `NOTICE`s. A test checks that each crate in
  the list above has one.

### 8. The notice for data made from Mojang's

Also a reading and not legal advice. Clustine cannot license what is not its own.
The notice therefore does two things and no third: it says which data is Mojang's,
and it says that Clustine grants no rights to it. It does not say what anyone may
do with that data, under which terms of Mojang's, or for what purpose it is here;
those would be promises the record cannot keep.

**`NOTICE.md` at the root** is the one place that says it in full:

> **Data from Minecraft.** The files under `crates/clustine-data/src/generated/`,
> `crates/clustine-protocol/src/generated/`,
> `crates/clustine-worldgen-data/src/generated/` and
> `crates/clustine-worldgen-data/reference/`, and the test data under
> `tools/fixtures/data/`, are made by `tools/datagen` and `tools/fixtures` from the
> server of Minecraft: Java Edition 26.3 as Mojang publishes it. They hold Mojang's
> data in another form: ids and names, properties and classes of blocks,
> world-generation settings turned into Rust, the sizes and connection points of
> structures, and values and hashes computed by the game. That data is Mojang's and
> not Clustine's. **It is not under the GNU Affero General Public License, and
> Clustine grants no rights to it.** The programs that make these files, and the
> Rust around the data in them, are Clustine's and are under that licence. Clustine
> is not affiliated with or endorsed by Mojang or Microsoft.

followed by the list of ported code (section 7) and the two reference projects.

Where else it shows:

| Place | What it says |
|---|---|
| The first line of every generated Rust file | `// Generated by cargo datagen from Minecraft 26.3. Do not edit. The data in this file is Mojang's and not under the AGPL; the code around it is: see NOTICE.md.` Emitted code adds: `// The emitter is adapted from SteelMC; see tools/datagen/NOTICE.` |
| Packed files and the reference values, which carry no comment | a `NOTICE` in each generated directory with the same sentence and the list of files; `cargo datagen` writes it, so `--check` covers it |
| `README.md`, "Licence" | after the present paragraph: "Game data that is generated from Mojang's server and committed is Mojang's; it is not under this licence and Clustine grants no rights to it. See [NOTICE.md](NOTICE.md)." |
| `LICENSE` | unchanged: it is the licence's text |
| The `license` of every crate's manifest, and the image's `licenses` label | **unchanged**, `AGPL-3.0-or-later`. The notice and the file heads carry the rest |
| The container image | `NOTICE.md` and `LICENSE` copied to `/usr/share/doc/clustine/`. This needs two lines in `.dockerignore` (`!NOTICE.md`, `!LICENSE`), which lets through only `.cargo`, the two cargo files, `bin`, `crates`, `services` and `tools` today, and a `COPY` in the `Dockerfile` |
| ADR-0002 | a line under "Consequences" that points here |

The tables ADR-0004 has committed since the first milestone are Mojang's data in the
same sense and get the same line; they have had none until now.

A test checks that every generated Rust file begins with the line and that each
generated directory has its `NOTICE`.

## Tests

Written from this record by someone who does not write the change.

1. `--check` passes on the committed output; fails and names the file when one byte
   of a Rust output, of a packed table, of a reference file or of the sums is
   changed, when an output is removed, and when a file that is no output is added in
   a sub-directory of a generated directory.
2. Two runs of the emitter over the same inputs give the same bytes (in datagen's
   own tests, on small inputs made by hand, without a jar).
3. The emitter fails on: a block state it cannot resolve, in each of the four ways
   of writing one; a reference that names nothing, other than the known one; a float
   that reads differently through a double; a biome parameter that does not fit, or
   that the report does not confirm; a template of another data version; jigsaws not
   in y, x, z order in the file; a jigsaw in a template with several palettes; a
   state whose answer consulted the level and is not listed; output over a budget.
4. Packed tables: each header has the magic, the kind, the jar's SHA-1 of
   `version.rs`, and sections whose lengths add up to the file's; `block_states.bin`
   has `BLOCK_STATE_COUNT` rows of 16 bytes. Known values hold: air is air and gives
   no light; stone is solid, solid render, in both heightmap tags, sturdy on six
   faces, and dampens fully; oak leaves are in the first heightmap tag and not in
   the second; glowstone gives 15; a water source is water, a source, at its full
   amount and not falling; a slab occludes by shape and its lower half covers the
   bottom face only; magma marks the position above it. From table 4: a chest has a
   block entity and is used by a click; an oak door is used by a click and an iron
   door is not.
5. `templates.bin`: 1,512 rows of which one is marked absent, 4,272 jigsaws, none
   of them in the absent row; no size above 48; every pool element's template index
   is in range; the pool `ancient_city/walls/no_corners` has as many elements as its
   file.
6. `generated.sums` equals the inputs and the outputs; changing a byte of a file
   under `tools/datagen/src`, of `Extract.java` or of `Cargo.lock` fails the test;
   every generated Rust file has the header line; every generated directory and
   every crate of section 7 has its `NOTICE`; the budgets of section 6 hold.
7. The store's start, with a small jar made by the test (a zip with an inner zip and
   two templates) and the pin given to the function under test, one case for each
   row of section 4's table. Among them: **a new world with neither option stops**
   and the message names both ways out; the default never yields `none`; `none`
   opens no file, with a jar named by option, by environment, or lying in the
   working directory as `server.jar`; a wrong jar stops a `blocks` start and does
   not stop a `none` one; a world made with `blocks` does not open without a jar; a
   world made with `none` opens with one and stays `none`; a jar in the working
   directory that is not named is never used.
8. Ignored, where the real jar is: all 1,511 templates parse and agree with
   `templates.bin`; every palette entry and every `final_state` resolves to a state
   id; the registry orders of section 1 that the fixtures program dumps equal the
   committed ones.
9. Two worlds on one seed, `blocks` and `none`: the starts, the biomes and the
   terrain-stage blocks of the same chunks are equal (once phase T exists; the test
   is named here because the claim is made here).

## Ruled out

| What | Why not |
|---|---|
| Committing the templates, as files (4.2 MB) or as Rust (Pumpkin: 22 MB) | The owner's decision 3. They are Mojang's buildings, not settings |
| Embedding the templates in the binary at build time (SteelMC) | The build would need the jar, and every binary and image would carry the buildings |
| Downloading the jar in a build script (SteelMC, Pumpkin) | ADR-0004: no download and no Java in a build |
| A new world that is made without structures because no jar was found (the first draft) | The owner's own first run would have been one, for good, with one line in a log to say so; and the same options would have made different worlds by what lay in a directory |
| Looking for a jar in the working directory or in `target/` | The same. `server.jar` is also the name every Minecraft server directory has, of whatever build |
| A server that downloads the jar, by itself or by a subcommand (`clustine minecraft-jar --fetch`, the first draft) | It would put an HTTP and TLS client (`ureq`, today only in `tools/datagen`) into the server's binary and image, and a pod would need a way out of the cluster. The stop message gives the URL and the SHA-1; `cargo datagen` fetches it for whoever has a checkout. Left to the owner (choice 5) |
| Building the code-made structures without a jar and leaving out only templates | Three kinds of world in place of two, and strongholds without villages read as a defect. Left to the owner with what `none` looks like (choice 1) |
| Not bending the terrain in a `none` world | Its terrain would then differ from the official world's, and from a `blocks` world's, around every structure; the one thing the two kinds share would be gone |
| Committing the data pack's JSON and interpreting it at run time | ADR-0004 keeps the JSON out; it is 2.0 MB of Mojang's files as they are; and an interpreter is slower |
| Everything as Rust literals; everything packed | Pumpkin's 86 MB; a decoder for every shape and no readable update |
| Emitted code formatted by the toolchain's `rustfmt` (SteelMC) | `--check` would depend on the formatter of the day |
| `license = "AGPL-3.0-or-later AND LicenseRef-Minecraft-Data"` in three manifests and the image's label (the first draft) | `AND` says both apply, and the second is defined as granting nothing; the crates that link the three would have kept the plain one; licence checkers refuse an unknown reference until configured |
| Committing the Java program's dump, or SteelMC's or Pumpkin's extracts | Many times the size, and someone else's extract cannot be proved against the pinned jar |
| The report alone for biome parameters | It has them as decimals of numbers the game holds as integers |
| One bit for "marks itself for post-processing"; a row of 8 bytes that grows a bit at a time (the first draft) | The game returns a position, which can be another block's; and the shared shapes are fixed in G5 |
| Generated output in a repository or submodule of its own | It moves the question and adds a step to every checkout |
| Asserting the build time in a test | Tests do not wait on a clock |

## Risks

- **The reading of the licences is wrong.** The owner has decided what is committed;
  this record only words the notice, and the notice promises nothing. Whether Mojang
  tolerates committed data made from its jar is not something a notice settles. Both
  reference projects commit more. What is committed cannot be taken out of the
  history again, which is why the buildings stay out.
- **A tension the notice does not remove, as a reading and not a legal conclusion.**
  The code ported from SteelMC is conveyed under the AGPL, whose section 5(c) asks
  that the whole of a work based on it be licensed under it. The notice says of data
  compiled into the same program that it is not under the AGPL. Clustine can say
  that for its own part; for SteelMC's part it cannot make exceptions. SteelMC is on
  the same footing (its binary embeds the templates), and Pumpkin under the GPL
  likewise. Nobody here can say how a court or either project would see it.
- **SteelMC's code is itself a translation of Mojang's source** (check §1). A port
  of it with every notice in place is on the same footing as SteelMC, no better.
  ADR-0002 accepted that when it chose the licence for this reuse.
- **The emitted code is larger or slower to build than guessed.** Measured before
  G5 (section 6); the fallback is written down.
- **A value the row lacks is found after G5.** Two spare bits, one spare byte and
  the per-block table are the room there is. The row was sized from a count of what
  SteelMC's generation reads, not from memory.
- **A registry's order is not the server's** where it is only "sorted by name". The
  dump of the fixtures program and test 8 find it; until G7 it rests on SteelMC's
  hashes.
- **Someone makes a world with `--structures none` to get past the stop** and is
  surprised later. The message, the help and the line at the start say that it is
  for good and what it looks like.
- **Every change of `Cargo.lock` needs datagen run again**, by someone with the jar.
  That is the price of the sums catching a forgotten run; subagents, who download
  nothing, cannot pay it and do not commit to `main` anyway.
- **An operator's jar is another build of 26.3.** The SHA-1 stops it.
- **The jar on a later standby store** (the replicas plan) has to be there too. Named
  for that plan.

## Not checked

- The data generator's output itself: it is gone from the machine, and G0's note is
  relied on. Whether `worldgen/**` in it is the same as in the jar.
- Every size marked as a guess, the build time and clippy's time. No Rust was
  emitted.
- That `javac` builds the program against the jar and that `Bootstrap.bootStrap()`
  needs nothing before it (G2 did the like for a program of its own); that every
  value of section 5 comes out without a level; that tags are not bound without a
  server. No class was run.
- How the four heightmaps are put together from the two tags and the fluid.
- What `isLightPermeable` is and whether light needs it.
- Which arguments of block classes the ported behaviours read; the size of table 4.
- That a click handler of a block's own class with `canOpenByHand` is a sound test
  for "used by a click" beyond doors and trapdoors.
- What the official server does with the pool element whose template is missing; the
  order a running server holds its registries and its tags in; where the End's
  biomes can be dumped from.
- Whether the review's search found every decimal literal of `worldgen/**`. The
  emitter's assertion covers all of them when it runs.
- How long the store takes to check the jar's SHA-1 at start (62 MB to hash; a guess
  is well under a second), and how much memory parsed templates take. Measured in
  S1.
- Whether an empty `components` on 513 jigsaws means anything. It is not committed.
- Any legal conclusion.

## Choices the owner has not settled

Each is decided above so that the record can be built from; each can be turned.

1. **What a world without structures is.** With `--structures none` nothing of any
   structure is built, also not the ones that need no template; the terrain is
   still shaped for them, so there is levelled ground where villages would stand
   and there are empty hollows where ancient cities would be. The other ways: build
   the code-made structures all the same; or do not shape the terrain either.
2. **A new world without a jar stops** unless `--structures none` is said (decided
   by the session after the review; the first draft had a warning).
3. **The store looks for a jar only where it is told**, by option or environment.
4. **A world keeps the choice it was made with.**
5. **No downloader in the server.** An operator fetches the jar; `cargo datagen`
   does it in a checkout.
6. **The budgets**: 6 MB, 1 MB of code (provisional), 300 lines, 1.5 MB of fixture
   data, 60 seconds to build and to lint.
7. **Packed binary files in the repository** for three tables, read through
   `cargo datagen --dump`.
8. **The wording of the notice**, and that the tables committed since ADR-0004 get
   it too.
9. **`generated.sums`** with the inputs in it, and so a datagen run after every
   change of `Cargo.lock`.
10. **Each jigsaw's `final_state` counts as a block of the building** and is not
    committed, while its `name`, `target` and `pool` count as connection points.
11. **Which class a block is, and arguments of that class, are committed** (table
    4). It falls under the owner's decision 2 by this record's reading; the owner's
    examples were light, shapes, fluid and climate.
12. **Committed test data**: values the game computes at given points (row 15) and,
    from `tools/fixtures`, hashes and structure starts of the official server's
    chunks. It is in the plan the owner read, and is "made from the jar" in another
    sense than a table is.

## Review

An independent reviewer went over the first draft against the jar, the code and
SteelMC's branch and found twelve things. Each was checked again against what it
cites before it was taken.

1. **Without a jar a world silently got no structures for good, and the owner's own
   run was such a case.** Accepted; the checkout the owner runs from had no jar. A new world without a jar now stops unless `--structures none` is
   said; `none` never looks for a jar; the store looks only where told; tests name
   the setting; test 7 has a case for each row (section 4).
2. **The per-state table was designed from memory of an older game.** Accepted;
   confirmed in the class files: no method for motion, the heightmaps go by two
   block tags, post-processing returns a position. The row is 16 bytes and defined
   from the state's methods and from what SteelMC's generation reads; the heightmap
   bits come from the tag files; the block entity type moved to a per-block table
   (section 5).
3. **Inputs wrong or left out.** Accepted, all four, each confirmed:
   `temperature_modifier` in two biomes (row 9); block states written as a bare
   name, `{id}` and `{id, properties}`, never with `Name` (section 1, section 4);
   the class of a block and its arguments (table 4, choice 11); 39 libraries, not
   40 (the first count had the inner jar among them).
4. **What makes the output the same bytes everywhere was not all named.** Accepted.
   Emitted Rust is laid out by the emitter and never formatted; a table says which
   order every registry is committed in and what vouches for it; `Cargo.lock` is in
   the workflow's paths; the sums and the reference values are outputs; block
   states resolve against the model in memory; the claim about two JDK builds is
   dropped; the Java program's way of writing is fixed (sections 1, 3).
5. **The check without a jar did not catch a forgotten run.** Accepted. The sums
   hold the inputs too; `tools/check.sh` runs `--check` where a jar is cached. The
   price for `Cargo.lock` is named under Risks.
6. **Licences: things stated as settled that are not, and one that would not
   build.** Accepted. The sentence that the image may be passed on is gone; the
   notice says whose the data is and that no rights are granted, and nothing else;
   the tension with section 5(c) is under Risks as a reading; the file head tells
   data from code; the manifests and the label stay as they are; `.dockerignore`
   and the `Dockerfile` are part of the change; committed test data is choice 12.
7. **The template table was under-specified and two numbers did not add up.**
   Accepted; measured again: all 1,291 templates have their jigsaws in y, x, z
   order, none with several palettes has a jigsaw, at most 47 in one, 2,925 without
   priorities. The order is named and asserted; the absent template keeps its place
   and weight; a jigsaw is 15 bytes; every section's length is in the header.
8. **A world without the jar was described too kindly.** Accepted. Section 4 says
   what is seen, and choice 1 puts it to the owner in those words.
9. **Budgets and forms.** Accepted, with one part that this record can only
   schedule: the size of emitted code cannot be measured in a record written
   without building anything, so the measurement is a step before G5 and the limit
   is marked provisional. The generated module's `rustfmt::skip` and its clippy
   `allow` are stated and clippy's time is budgeted; `--dump` and `.gitattributes`
   are in; printing how many rows differ is dropped for `--dump`.
10. **"Used when clicked" by the class's handler alone.** Accepted; `BlockSetType`
    has `canOpenByHand`. The rule takes it in, and what is left is a named limit.
11. **The report's cross-check could fail on a right value.** Accepted; the count of
    3,182 was computed again. The comparison goes forwards.
12. **Smaller points.** The downloader: accepted by taking it out rather than by
    naming its cost; it is choice 5. The stamp: accepted; datagen unpacks the
    libraries itself. A bound for parsed templates: not given, because the whole set
    is the bound; the figure (a guess of 10 to 15 MB) is.

Nothing of the review was rejected outright. Two of its proposals were answered
otherwise than proposed: the search for a jar beside the build's target directory
(finding 1) was not added, since the session decided that nothing depends on a
directory silently, and the owner's run names the jar instead; and no bound was
put on the template cache (finding 12).
