# Phase F: features, placed as the game places them

- Status: **Drafted on 2026-10-15 and revised after an independent review** the same
  day (its sixteen findings are answered in the last section), **and agreed**: the
  owner answered the two questions of section 6 as recommended. Phase F of the
  [terrain plan](terrain-plan.md), section 3, made concrete;
  [ADR-0021](../adr/0021-generation-is-a-function.md) decided the order of runs, the
  view a run works in and how runs are judged, and this plan does not reopen those.
  Where it departs from the terrain plan's F rows, which were written before ADR-0021
  and the trial G3, it says so (section 7).

## 1. What there is now

A seeded world is the game's bare overworld: biomes, the fill, the surface, caves and
ravines, and the ground bent where structures will stand (T1 to T7), with the game's
spawn (T8). Nothing grows on it. The scheduler of ADR-0021
(`services/worldgen/src/scheduler/`, W5) knows the order of runs and has run it only
with toy stages: `Order::Nine` with the rank table `[[0,1,2],[7,6,3],[8,5,4]]`
(`lattice.rs:10`), a `Run(c)` task that needs the 25 terrains within 2 and the runs
it sees, `Finished(e)` from a chunk's terrain and the nine runs within 1, `Lit(p)`
from nine finished chunks. A run is handed a `View` (`scheduler/view.rs`) that
implements `Neighbourhood` (`crates/clustine-world/src/generation.rs:85-118`): the
3×3 live, the 5×5 frozen heights, anything further fetched and counted; writes
outside the 3×3 refused, ticks and marks outside it dropped; `attempt` undoes a
placement that panics and marks the chunks it could have touched. The served
`--seed` world runs `PreviewStages` under `Order::Bare`
(`bin/clustine/src/lib.rs:92-106`), whose identity is
`terrain;preview;<dimension>;seed <n>` (`services/worldgen/src/terrain.rs:306`):
a finished chunk is its terrain and `decorate` does nothing.

What is in place for the work:

- **Random numbers**: `set_decoration_seed` and `set_feature_seed(decoration, index,
  step)` with vanilla traces (`crates/clustine-noise/src/random/worldgen_random.rs`).
- **Data**: `tools/datagen` reads every registry under `data/minecraft/worldgen/`
  from the jar already (`worldgen.rs:43-73`), `feature` (240 files) and
  `placed_feature` (273) among them; nothing emits them. 26.3 has no
  `configured_feature` directory and no `random_patch` type: a patch is a
  `simple_block` or a `block_column` whose placement spreads it. 47 types at the top
  of the files, and nested ones besides (`template` inside the desert well's
  `overlay`, `sculk_patch` inside a `sequence`). Fifteen placement modifiers are used.
  Each of the 67 biomes lists its placed features in eleven steps; 56 of them are
  the overworld's.
- **A chunk carries** blocks, block entities (in the form the game sends them on the
  network, 16 KiB each), scheduled ticks and marks for post-processing, and its
  heights (ADR-0022). Ticks and marks travel to the worker and the store and wait for
  phase B.
- **The port's source**: SteelMC's 26.3 branch at `885c4b3`
  (`/home/claude/reference/SteelMC-26.3`, AGPL-3.0, as Clustine), whose feature code
  is about 4,700 lines of runner, placement, providers, predicates and dispatch,
  10,900 of feature types and 5,200 of trees, with a port of Java's `HashMap`
  iteration (`vanilla_collections.rs`, 778) that trees and vegetation patches iterate
  in. One `todo!`: the poplar foliage's skip test (`features/tree/foliage.rs:1153`).
- **The judges** of T3 to T8 drive the game's own classes without a server
  (`crates/clustine-terrain/tests/java/`); `GameStructures.java` loads the game's own
  data with `WorldLoader.load`, so every tag and registry is bound. Nothing drives the
  features step. The trial G2 (`docs/groundwork/terrain-g2/G2.java`) started the real
  server inside a Java program, with no change to any class, and asked it for chunks
  at `FEATURES`: 100 of 100 chunks' blocks equal to SteelMC's fixture hashes. It found
  that `halt` and `System.exit` did not end the process.
- **Templates**: `clustine-structures` knows every template's size and connection
  points (`templates.bin`), not its blocks; by the terrain plan's question 3, the
  blocks are read from the operator's own jar when the server starts, and no
  template block is placed without it. Placing a template's blocks with its
  processors was step S1.

## 2. What the game does (26.3)

`ChunkGenerator.applyBiomeDecoration(WorldGenLevel, ChunkAccess, StructureManager)`,
one call per chunk at the `FEATURES` step, with the region the step gives: writable
within 1 chunk, its dependencies structure starts within 8 and `TERRAIN` within 1
(26.3 merged noise, surface and carvers into one status, `TERRAIN`; SteelMC
`chunk_pyramid.rs:389-392`), so the neighbours' terrain is there and their features
only if their own step ran first:

1. The decoration seed from the world seed and the chunk's lowest corner block.
2. The placed features that can occur: the union, over the biomes of the 3×3
   chunks, of each biome's list, as indices into the order the generator memoised
   (`featuresPerStep`), which `FeatureSorter` built once from the biome source's
   possible biomes in their parameter list's order. The indices are sorted.
3. For each of the eleven steps: first the structures of that step whose pieces
   reach the chunk (phase S), seeded by structure index and step; then every
   feature index of the step, seeded by `set_feature_seed(decoration, index, step)`,
   placed by its placement modifiers and then its configured feature.

A biome a feature asks for is the zoomed biome at a block, and freezing asks the
temperature at a block; both exist in `clustine-terrain` (`biome/zoom.rs`,
`surface.rs:289-330`). Freezing and snow ask the block light, which nothing has made
at `FEATURES`. Trees, fallen trees and vegetation patches collect positions in Java
hash sets and place in their iteration order. Beehives write their bees into the
block entity, monster rooms their spawner's mob and their chest's loot table and
seed. Entities: one feature makes one (the End's crystal); ADR-0021: the random
numbers are drawn and the entity is not made.

## 3. What F builds

### The crate: `clustine-features`

Named in ADR-0021. It depends on `clustine-data`, `clustine-noise`,
`clustine-terrain` (the zoomed biome and the temperature), `clustine-world` and
`clustine-worldgen-data`, and does no I/O; like `clustine-sim` it uses no hash map
and no clock (a `clippy.toml` as sim's), since a run is a function of its view and
its settings. What it offers the scheduler is one function:

```rust
/// Places the features of `centre` into `view`, as the game's features step does,
/// leaving out the structures' pieces (phase S).
pub fn decorate(settings: &FeatureSettings, view: &mut dyn Neighbourhood, centre: ChunkPos);
```

`FeatureSettings::new(seed, dimension, templates: Option<Arc<Templates>>)`: the
templates' blocks are read by the binary from the operator's jar (as the terrain
plan's question 3 says), not by the crate. A block light asked for is answered 0,
as the game answers at `FEATURES`; F0 checks that with a lava lake beside a frozen
one. Inside:

- `placement/`: the fifteen modifiers the data uses.
- `providers.rs`, `predicates.rs`: block-state providers and block predicates.
- `dispatch.rs`, generated from the list of types: one function per type, each in
  the file of the batch that builds it, a stub that places nothing until then. A
  batch edits its own files and never the dispatch.
- `combine.rs`: the combinators, `random_selector`, `weighted_random_selector`,
  `simple_random_selector`, `random_boolean_selector`, `sequence`, `overlay` and
  `no_op`, which every batch needs.
- `features/`: one file per type, `features/tree/` for trees.
- `java_set.rs`: Java's `HashMap`/`HashSet` iteration over block positions as JDK 25
  has it (bucket spread, resize at 0.75, tree bins at 8 and 6, capacity 64).
- `run.rs`: section 2, items 1 to 3, without structures.

### The data: emitted, and the order taken from the game

`tools/datagen` emits into `crates/clustine-worldgen-data/src/generated/`, as for
carvers and structures: every configured feature as a typed static, every placed
feature as its modifiers and a reference, the named block-state providers, and for
each biome its eleven lists. An unknown type, nested or not, an unknown modifier,
provider or predicate fails the run, as an unknown carver does.

**The order of features is not computed in Rust.** `Extract.java` loads the game's
own data with `WorldLoader.load`, as `GameStructures.java` does (Extract bootstraps
with `VanillaRegistries` today, `Extract.java:6,42`; this changes it), takes the
generator of each dimension from the normal world preset, and reads its memoised
`featuresPerStep` by reflection: per step, the placed features in the game's order.
The emitted table is that order. A wrong order would reseed every feature after the
first difference; taking it from the generator the game uses removes that risk and
the sorter's port. Java's set order inside trees and patches stays a port.

### The stages, the identity and the switch

`clustine-worldgen` gets `FeatureStages` beside `PreviewStages`: `starts` from
`clustine-structures`, `terrain` from `clustine-terrain` with its worldgen heights,
`decorate` from `clustine-features`, under `Order::Nine`. Until S the starts are made
(the ground bends) and their pieces are not placed.

Its identity is `terrain;features <n>;<dimension>;seed <s>`, where *n* counts the
batches built (1 after F1, up to 6). The store compares identities exactly and
refuses another (`local.rs:115`); a refusal between two identities that differ only
in what made the land gets a sentence of its own: "this world was made by an
earlier generator of Clustine (…), and worlds are not carried over before the first
release; move it away or name another with `--world`". So a world made at F2 is
refused at F4, not mixed.

**The served world switches once, at the end of F6**, from the preview to
`FeatureStages` under its final identity; until then `FeatureStages` runs behind a
hidden setting, `--generator features`, for the trials and the tests. Switching
batch by batch would leave every chunk a world stored without trees beside chunks
made later with them. S changes the identity again when it places structures'
blocks, under the same rule.

After the switch the spawn (T8) is found on finished chunks with features: on
leaves where the game finds it on leaves. T8's comparison with the official worlds,
which allowed another height where leaves lie (`t8-plan.md:108-111`), becomes exact.

### The judge: the server inside a Java program, in Clustine's order

ADR-0021's six rules (its section on testing): terrain within 2 first; rank by rank,
in two orders within a rank; nothing beyond `FEATURES`; no save and reload; only
chunks whose dependencies lay inside the area compared; ticks and marks compared,
and F0 shows they come out. F0 builds it as
`crates/clustine-features/tests/java/GameFeatures.java`, beside the other judges and
run by hand as they are:

1. Start the server on a fresh world of a seed with `generate-structures=false`
   (until S), no mobs spawning, `pause-when-empty-seconds=-1`, saving off, and a
   data pack (below). Find the server's thread as G2 did.
2. **Hold every chunk itself.** For the area and two chunks around it, add a ticket
   of the program's own, without a timeout, at the level that keeps a chunk at
   `TERRAIN`; and keep the `ProtoChunk` object each request returned. A chunk held
   only by the game's own ticket for a request is let go a tick later and would be
   saved and read back, which rule 4 forbids.
3. **Check that nothing ran before.** Creating a world searches up to 121 full
   chunks for the spawn (T8): those chunks are decorated in the server's own order.
   Areas lie at least 32 chunks from the spawn, and before the first run the program
   asserts that every chunk it holds has a persisted status no higher than
   `TERRAIN`. The spawn's own surroundings are judged by F8's counts only.
4. Then, rank by rank in Clustine's table, every chunk of the rank at `FEATURES`,
   one at a time on the server's thread: a request at `FEATURES` raises only its
   neighbours to `TERRAIN`, so the order of runs is the order of requests. Two
   runs of the program, ascending and descending within a rank, must agree.
5. At the end, assert that each chunk is the object kept in 2 and at `FEATURES`, not
   beyond, and read it in the process: block states, block entities as the game
   writes them for the network (the form a chunk carries), block and fluid ticks,
   marks, the four heightmaps. Write them as text in the judges' form
   (`game-features 1`).
6. End with `Runtime.halt`.

**The gate per batch: the features not built are turned off in the judge.** Every
overworld biome lists ores, geodes, monster rooms, glow lichen and freezing, so no
chunk has "every feature built" before the last batch. The program writes a data pack
that replaces the *configured* feature of every placed feature not yet built with
`minecraft:no_op`; the biomes' lists, so the order, the indices and the seeds, stay
as they are, and Clustine's stubs place nothing just as `no_op` does. Each batch is
then judged exactly, chunk for chunk, on what is built so far. The list of what is
built is one file, read by both sides.

An ignored Rust test, `crates/clustine-features/tests/game_features.rs`, reads the
output and makes each chunk with `FeatureStages` under `Order::Nine` in the
scheduler, starts off, and compares blocks, block entities, ticks, marks and
heights. To tell which feature made a block that differs, the view gets a test-only
log of the `Placing` that last wrote each block (it exists for `attempt`,
`generation.rs`), printed beside the difference.

**Areas**: every one of the 56 overworld biomes, each in at least two areas of 8×8
chunks on seed 13579 or 3 or 12345, found from Clustine's own biomes, far from the
spawn; with 9 more for the mangrove swamp, the cherry grove, the pale garden, the
deep dark, the lush and dripstone caves, the sulfur caves, the badlands and the
mushroom fields. Exact ties of biomes are named as the T-judges named them.

### Steps

Each step is a verified commit. Batches F2 to F5 can run side by side after F1b,
with the order between them stated in the table.

| # | What | Who | Verified by |
|---|---|---|---|
| F0 | The judge, its data pack of `no_op`s, its reader in Rust, the area chooser | me: it starts the official server | The two orders within a rank give the same output; the assertions of 2, 3 and 5 hold; ticks and marks come out (springs, lakes); with everything `no_op` the chunks equal Clustine's terrain |
| F1a | Emitted data: configured and placed features, providers, the biome lists, **the order read from the game**; `Extract.java` on the game's own data | me: datagen runs Java | Every file of the two registries parsed; each step's table the length of the game's |
| F1b | The crate's frame: placement, providers, predicates, the combinators, the dispatch with its stubs, `run.rs`, `java_set.rs`; for real: `simple_block`, `block_column`, `ore`, `scattered_ore` | a subagent, on the crate | Unit tests; Java's set order against tables I make with the JDK; the judge with all but those `no_op` |
| F1c | `FeatureStages`, its identity, the store's sentence, `--generator features`, the write log, run results measured | me | Scheduler tests with the real stages on a small area; the store's refusal tested; section 5's figures |
| F1t | Tests from section 2 by someone who did not write `run.rs` and `java_set.rs`: the decoration seed, the union over the 3×3's biomes, the sorted indices, the order of steps, Java's set order (ADR-0021's order of runs has such tests already, `services/worldgen/tests/scheduler_order.rs`) | a subagent | The tests themselves |
| F2 | The ground's features: `disk`, `block_blob`, `block_pile`, `spring_feature`, `lake`, `freeze_top_layer`, `iceberg`, `blue_ice`, `spike`, `underwater_magma`, `monster_room` | a subagent | The judge, all else `no_op` |
| F3 | Trees: `tree` with every trunk, foliage and root placer and decorator (beehives with bees), `fallen_tree`, `huge_brown_mushroom`, `huge_red_mushroom`, `root_system` | a subagent | as F2, forests of every kind |
| F4 | Plants: `vegetation_patch`, `waterlogged_vegetation_patch`, `bamboo`, `vines`, `multiface_growth`, `sculk_patch`, coral's `overlay` pieces, `sea_pickle` and kelp through their combinators | a subagent | as F2 |
| F5a | Templates' blocks: read from the operator's jar by the binary; placed with their processors (rules, block-entity modifiers such as `append_loot`, gravity, protected blocks) in `clustine-structures`, where S builds on it. Step S1 of the terrain plan, brought forward | a subagent | Templates placed against the game's own `StructureTemplate.placeInWorld` in a Java judge without a server (as `GameStructures.java`) |
| F5 | Caves and buried things: `geode`, `large_dripstone`, `speleothem_cluster` and pointed dripstone, `fossil`, `template` (the desert well, the sulfur springs), the sulfur caves' features; after F3 and F5a | a subagent | as F2 |
| F6 | The judge over every area with nothing `no_op`; the edge sends a spawner's `SpawnData` (ADR-0022's list of block-entity types, compared with the official server's packet); **the switch** | me | Every compared chunk equal but named ties; the official comparisons; the served world |
| F7 | The Nether's and the End's features (`huge_fungus`, `delta_feature`, `netherrack_replace_blobs`, `random_neighbor_spread`, `chorus_plant`, the End's islands, spikes, gateways, podium) | a subagent | as F2, in those dimensions; served with D |
| F8 | Finished chunks against the official server's full chunks of a world it made and saved, ticks frozen | me | A count per chunk, expected only where a fluid flowed or a block took its shape on becoming full |

Subagents work in `crates/clustine-features` (F5a in `clustine-structures`) on the
files the frame gives them, port from SteelMC with its line references, and do not
run the judge (it starts Java). I run it after each batch and send back the chunks
that differ with the placement that wrote each block. No step touches the edge's
hand-over or resume logic; F6's change at the edge is a line in its list of
block-entity types.

## 4. What a player meets, and when

Until F6 the served world is as now. After F6, on a new world: trees of every kind,
grass, flowers, ferns, bushes, mushrooms, sugar cane, pumpkins, cacti, kelp and
seagrass, coral, every ore, lakes of lava, springs, snow lying and ice on cold water,
geodes, dripstone, glow lichen, sculk, desert wells, fossils, the sulfur caves'
springs, monster rooms with a turning mob in their spawners (it spawns nothing: no
mobs) and chests that do not open (no inventories). What looks wrong until phase B:
water and lava from springs stand where they would run (their ticks wait), which is
question 2. Villages and other structures' buildings come with S.

## 5. Speed

On the owner's machine, nothing else running, the world on the network disk with its
cache, and the same in memory (`/dev/shm`), each figure saying which:

- Bytes a run result takes in the cache and time per run by step, with ores (F1c)
  and with everything (F6). The cache's bound (512 MiB) is checked against them.
- A new world's spawn area as the store starts (T8f), target under 15 seconds
  (terrain plan, question 9); a first view of 8 cold.
- A bot flying at a sprint (21.6 blocks a second) 64 chunks east at view distance 8:
  for every chunk, the time from entering the view to arriving. Target, from the
  terrain plan: it never reaches the edge of what was sent, that is, every chunk
  arrives before the bot has flown the seven chunks between it and the edge (5.2 s).
- The home region's ticks while a first view is made (T8's measurement), none over
  50 ms.

If the flight falls behind, the terrain plan's fallback, running each chunk's
features alone (`Order::Alone`), changes what "block for block" means: that is put to
the owner with the figures before anything changes.

## 6. Questions that are the owner's

| # | Question | Recommendation | If there is no answer |
|---|---|---|---|
| 1 | A seeded world made before the switch (F6) has chunks without trees. Refuse it with a sentence that says so, as worlds before T8 are refused, or accept it with a seam where old chunks meet new? | Refuse | Refuse: nothing is carried over before the first release |
| 2 | Springs place a source and leave its flow to ticks (phase B, B4 to B6), so from F6 a spring stands as a single block of water or lava on a cliff or in a cave until B. Switch at F6 with that, or hold the switch until B5? | Switch at F6: today whole aquifers stand as walls where caves cut them, and trees and the rest are what a player misses first; B4 to B6 go on beside S as the terrain plan has it | Switch at F6 |

Decided without a question: monster rooms keep the spawner's mob and the chest's
loot table and seed in their block entities, the official server's form (ADR-0022
measured `LootTable` and `LootTableSeed` on generated chests).

## 7. Where this departs from the terrain plan's F rows

- F0 there: "pyramids first within a class" and "the world saved and read back".
  ADR-0021 found the pyramid is no exception (G3) and forbids save and reload in the
  judge (rule 4); F0 here follows the record, and holds its chunks to keep it.
- F0 there was checked by SteelMC's hashes under their order; here by the judge's
  own assertions and two orders within a rank.
- F1 there ported the order of features; here the game gives it.
- The batches follow 26.3's types, not SteelMC's files; the combinators,
  `simple_block` and `block_column` are in the frame.
- Placing templates (S1) comes forward to F5a, since the desert well, the sulfur
  springs and fossils are features.
- Each batch is judged with the rest turned off in the game, which the rows did not
  have; the served world switches once.

## 8. Risks

| Risk | What is done |
|---|---|
| The server decorates a chunk in the area out of the requested order | The judge holds its chunks, asserts nothing beyond `TERRAIN` before the first run, the same objects at `FEATURES` and not beyond at the end, and two orders within a rank equal |
| Java's set order in trees differs between JDK versions | The judge runs on the JDK 25 the datagen uses; the port follows JDK 25's `HashMap` |
| The official server's lighting race (CLAUDE.md) | The judge reads no light; light is the light crate's, judged in W1 |
| Templates need the operator's jar | Without it no template is placed and the chunk is marked, as structures will be |
| Feature code panics on data it does not expect | `attempt` catches it (ADR-0021); the judge counts such chunks |
| A biome or feature type that only a rare biome uses goes unjudged | Every overworld biome is in at least two areas |

## 9. The review, and what changed

An independent review of the first draft (2026-10-15) made sixteen findings; all were
taken.

1. The judge relied on the game's own ticket for a request, which lets a chunk go a
   tick later: it would be saved and reloaded. It now holds every chunk with a ticket
   of its own and checks the objects at the end.
2. The spawn search at world creation decorates up to 121 full chunks in the server's
   own order. Areas lie 32 chunks off, and the judge asserts nothing ran before.
3. Every overworld biome lists ores, geodes, monster rooms, lichen and freezing, so
   "chunks whose every feature is built" was empty until the last batch. A data pack
   now turns the unbuilt features into `no_op` in the game, keeping the order and the
   seeds.
4. `CARVERS` is not a status in 26.3; it is `TERRAIN`.
5. A hook on `applyBiomeDecoration` could not be built without changing a class;
   replaced by the assertions of 1 and 2.
6. The desert well and the sulfur springs are templates, not only fossils: placing
   templates moves forward to F5a, and the binary gives the crate the blocks.
7. The batches followed SteelMC's files; they now follow 26.3's types, and the
   combinators are in the frame.
8. The store accepts an older generator version without a word, and the draft named
   no identity: `FeatureStages` now has one with the batch in it, a refusal of its
   own, and the spawn's comparison becomes exact.
9. A spawner showed as an empty cage without its `SpawnData` at the edge: added to
   F6. Standing springs are now the owner's question 2. The draft's question 2 was
   already decided by ADR-0022.
10. F1t repeated tests that exist; it now tests `run.rs` and `java_set.rs`. The
    dispatch is generated, so batches do not edit one file, and the order between
    batches is stated.
11. The recorder names no writer of a block: a test-only log is added in F1c.
12. The crate needs `clustine-terrain` for the zoomed biome and temperatures; block
    light is answered 0 as at `FEATURES`, checked in F0.
13. `possibleBiomes()` follows the parameter list, not a hash; the order is read from
    the generator's memo rather than rebuilt, and Extract's change to the game's own
    data is the plan's.
14. Forty biomes were covered of 56; now all.
15. Speed: the flight's target is stated with a number, the disk named, and the
    fallback put to the owner.
16. Small slips: the heightmap and filter of a patch, the rank table's line, what G2
    found about ending the process, the form of block entities, beehives' bees.
