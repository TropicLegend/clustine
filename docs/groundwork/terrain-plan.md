# Terrain generation: the plan, proposed

- Status: **Agreed by the owner on 2026-10-10**, with its questions 1 to 5 answered as it recommends (the roadmap, "What the owner answered on 2026-10-10"). As proposed: Drafted on 2026-10-10, gone over by an independent
  reviewer against the code of both projects, and revised; the review's fourteen
  findings are answered one by one in the last section. It waits for the owner's
  answers to the ten questions of section 7, each of which has a recommendation and
  what happens without an answer. The two spikes of phase G (G1, G2) decide whether
  its claims can be tested at all, and are done before any decision record is written.

**Steps G0 to G3 have been done** (2026-10-10), and both trials went well; what
they found is in "What the first trials found", before section 0, and the text below
is as it was proposed before them.

It rests on [terrain-generation.md](terrain-generation.md) ("groundwork §n"), on that
document's check against clones of both projects
([terrain-generation-check.md](terrain-generation-check.md), "check §n") and on the
data files of the released 26.3 server jar.

`S263:` is SteelMC's 26.3 branch at `885c4b3`, `X:` SteelExtractor at `e1e2695`, **jar**
the inner `server-26.3.jar` of the server jar that `cargo datagen` downloads
(`version.json`: 26.3, world version 5023, built 2026-09-15). The jar was read, never
run. `unzip` is not installed here, so its entries were read in memory with Python's
`zipfile`; the reach figures of section 1 were computed the same way, by a search over
all rankings. Both took seconds and wrote nothing. Where a figure is a guess it says so.

Steps are named by phase: **G** ground rules and spikes, **W** Clustine's side of a real
world, **T** terrain, **F** features, **S** structures, **B** block behaviour that real
terrain makes necessary, **D** the other dimensions.

## In short

- A generator of Clustine's own, on stable Rust, ported from SteelMC's 26.3 branch with
  notices, fed by tables and Rust that `cargo datagen` makes from the official jar.
  Neither project is a dependency.
- **Generation stays a pure function of seed and position**, decorated in a fixed order
  of nine classes (shape C). The review's objections to its premise are met by a wider
  view of *frozen* terrain data and one named exception; its cost is now computed: about
  1.6 times the feature runs and 1.9 times the terrain of the cheapest shape for a first
  view, 1.3 to 1.6 times while walking.
- **The first draft's terrain reference was wrong** (review, finding 1). Terrain is now
  judged against the unmodified server run with **data packs generated from its own
  files** that switch features off, which leaves the ring of terrain-status chunks
  clean; the same trick gives the fill and the surface gates of their own. Everything
  from features on is judged by **a Java program that runs the server inside itself**.
  Both are tried as **spikes in phase G, before the decision record on the shape is
  written**.
- **What a region was handed is saved** (recommended; the owner's question 2), so that a
  later fix to the generator neither rewrites nor locks out a world, and a region that
  is brought back never waits for generation.
- **Light is made with the chunk** and travels with it; the edge relights only where
  blocks have changed, and never on its fan-out task.
- Seams a player meets within minutes are in the milestone: placing against doors and
  chests, the spawn point on the ground as it is now, plants and sand that lose their
  support, and, as a phase of its own beside features and structures, flowing water.
- About 65 steps. SteelMC's generation and light are 91,580 lines (check §9).

---

## What the first trials found

Done on 2026-10-10 with a script kept beside this plan
([terrain-g1.py](terrain-g1.py)): the official server on seed 13579, a data pack made
from the jar's own 67 biome files with their `features` emptied, ticking frozen, one
chunk force-loaded, the server stopped and its region files read.

**G0, the data generator's reports.** `reports/biome_parameters/minecraft/overworld.json`
and `nether.json` hold the climate parameters of every biome, so they come from
`cargo datagen` and need no program of ours. `reports/blocks.json` has the properties
and the state ids of every block and nothing else: light, shapes and fluid states need
the Java program of G6, as assumed.

**G1, terrain from the unmodified server: it works.**

- The server takes the pack without a word but `Found new data pack file/spike,
  loading it automatically`.
- It generates force-loaded chunks while ticking is frozen.
- It saves unfinished chunks with their status. Around one force-loaded chunk: 5 by
  5 chunks at `full`, a ring at `initialize_light`, **a ring of 32 at `terrain`**, a
  ring at `biomes`, and many at `structure_starts` further out.
- **The blocks of the chunks left at `terrain` equal SteelMC's `terrain` hashes: 14
  of 14** that fall into the first cluster of their fixture, around chunk (−418462,
  366791), in two runs that loaded different neighbours. One of them, (−418462,
  366791) itself, was left at `terrain` in one run and finished in the other; so the
  ring is not written into by its neighbours when the biomes have no features.
- So for those chunks SteelMC's fixture of terrain is what the released 26.3 makes,
  and its 26.3 branch's two ignored fields of the noise settings (section 0) did not
  show there. Fourteen chunks at one place say nothing about the rest; the fixtures of
  G7 do.
- The finished chunks there differ from their terrain by a village and a mineshaft
  that happen to stand in that cluster (387, 284 and 353 blocks in three chunks:
  planks, logs, rails, cobwebs, paths, cave air), which is what the structures' step
  does and nothing else.
- 26.3 keeps regions in `world/dimensions/minecraft/overworld/region`, and writes a
  palette entry as the block's name alone for its default state and as `{id,
  properties}` otherwise, each wrapped in a compound where a palette has both kinds.
  The reader of G7 has to know both.

**G2, the server inside a plain Java program: it works, by the first route.**
[terrain-g2/G2.java](terrain-g2/G2.java) is a hundred lines, compiled with `javac`
against the server's own classes (the inner jar and the libraries that the bundler
unpacks beside it on a first run) and run with them on the class path.

- It calls `net.minecraft.server.Main.main` as it is, which starts the server on its
  own thread and returns. No mod loader, no agent, no change to any class.
- It finds the server through the thread that reads the console, which is an inner
  class of the server and holds it. That thread ends at once when standard input is
  closed, so the program is started with its input kept open.
- `ServerChunkCache.getChunk(x, z, status, true)` brings one chunk to a status and
  returns when it is there, from any thread. Asked for chunk after chunk, it runs the
  features of exactly those chunks in exactly that order.
- **For the first cluster of SteelMC's fixture, ten by ten chunks around (−418462,
  366791), brought to `terrain` and then to `features` in ascending x and z: 100 of
  100 terrain hashes and 100 of 100 features hashes equal the fixture's.** It took
  ten seconds.
- So everything from features on can be judged by the official server on this
  machine, in an order of Clustine's choosing; and for that cluster SteelMC's 26.3
  fixture is what the released 26.3 makes when it decorates in their order,
  trees, hash-set order and all.
- Left over: `MinecraftServer.halt(false)` followed by `System.exit` did not end the
  process, which had to be killed; F0 has to stop it properly. Set
  `pause-when-empty-seconds=-1`, or the server pauses after a minute without players.

**G3, the audit of what a feature run reads and writes: shape C holds for every
feature and structure piece of 26.3.** Read from SteelMC's 26.3 branch and the jar's
data, type by type.

- No read of live blocks lands beyond the three by three chunks around the run, but
  for one corner case: a geode whose origin is in a chunk's outermost column, with
  strongly positive noise, reads one block further and may schedule a fluid tick
  there. It is rare (well under one geode in a thousand, by estimate; a geode is in
  one chunk of 24). It is answered from terrain, the tick is dropped, and the
  fixtures count it.
- **The desert pyramid is not an exception**, as section 1 below has it: its box
  starts at its start chunk's corner and lies in that chunk and the next, so its scan
  of heights stays inside the three by three. No wider dependency and no order
  within a class is needed.
- No read of data frozen before features goes beyond the three by three either
  (the farthest are 13 to 16 blocks: an ore's probe, a lake's biome, a shipwreck's
  rectangle). The five by five frozen view is a margin and is kept until the fixtures
  have counted the geode case.
- Several pieces depend on the order inside the three by three (a pyramid's and a
  beached shipwreck's height, a mineshaft's spawner, a chest's facing at a chunk
  edge), which the fixed order covers on one condition for the decision record: a
  run's result carries its changes to structure pieces, and a run sees pieces as the
  runs of lower classes within two chunks left them.
- For the harness: a height asked of a finished neighbour, or of an unfinished chunk
  read back from disk, is of live blocks. Reference chunks stay below `full` and are
  not saved and read back in the middle of a run.
- The costs of section 1 reproduce: 812 to 822 feature runs and 1,301 to 1,337 chunks
  of terrain for a first view, against 497 and 697 without a fixed order; walking
  along an axis 26 to 32 runs a chunk, diagonally 51.

Not tried yet: the two other packs (without carvers; with an empty material rule),
which give the gates of T4 and T5; the Nether and the End; light; a second cluster.
None of them stands in the way of the decision records of G4.

---

## 0. What the jar's data settles

Read from the jar (sizes are uncompressed bytes in the jar):

| Question | Answer |
|---|---|
| Registries of world generation | `worldgen/biome` 67 files, `block_state_provider` 8, `carver` 4, `density_function` 55, `feature` 240, `material_condition` 8, `material_rule` 42, `multi_noise_biome_source_parameter_list` 2, `noise` 64, `noise_settings` 7 (overworld, amplified, large_biomes, caves, floating_islands, nether, end), `placed_feature` 273, `processor_list` 40, `structure` 52, `structure_set` 21, `template_pool` 245; besides `block_transformer` 3 and tags. About 2.0 MB of JSON in all |
| Are biome parameters in the data? | **No.** The two parameter lists are `{"preset": "minecraft:overworld"}` and `…nether`: the values are in Mojang's code. The data generator has a `BiomeParametersDumpReport` class (`net/minecraft/data/info/`), so a report exists; what it holds is seen when `cargo datagen` runs (G0) |
| Density function types really used | `add`, `mul`, `noise`, `cache`, `gradient`, `lerp`, `range_choice`, `clamp`, `min`, `sub`, `interpolated`, `abs`, `blend_alpha`, `spline`, `max`, `quarter_negative`, `old_blended_noise`, `squeeze`, `blend_density`, `square`, `beardifier`, `blend_offset`, `find_top_surface`, `div`, `half_negative`, `interval_select`, `cube`, `shift_a`, `shift_b`, `slice`, `distance_to_point`, `end_outer_islands`, `negate`. So the old names (`y_clamped_gradient`, `shifted_noise`, `flat_cache`, `cache_2d`, `cache_once`, `cache_all_in_cell`) are gone, as the groundwork had from the wiki |
| Noise settings | Keys: `aquifers` (overworld kinds only: `barrier`, **`exclusion`**, `fluid_level_floodedness`, `fluid_level_spread`, `lava`, `surface_level`), `material_rule` (a reference), `noise` (`min_y`, `height`), `noise_router` (**`chunk_surface_level`**, `continents`, `depth`, `erosion`, `final_density`, `ridges`, `temperature`, `vegetation`), `spawn_target`, `debug_functions`, `legacy_random_source`, `sea_level`. No vein entries and no `ore_veins` list: ore veins are two material rules |
| **SteelMC's branch against this** (new) | Its parser knows neither `aquifers.exclusion` nor `noise_router.chunk_surface_level` (`S263:steel-worldgen/build/density/functions.rs:241-277`; no mention of either in `steel-worldgen`). It takes the surface level from `aquifers.surface_level` (`:899-902`). Whether what it hard-codes instead is the same is not known; with the test's name `float_noise_matches_pre1_extractor` it suggests the branch was built against a pre-release. **Clustine's emitter follows the data, not SteelMC, here** |
| Structures | 52, of which **28 are built by jigsaw**: 18 abandoned camps, 5 villages, ancient city, bastion remnant, pillager outpost, trail ruins, trial chambers. The other 24 are of the fifteen hard-coded types. Placement is `random_spread` everywhere and `concentric_rings` for strongholds; **no `dimension_origin`** |
| Which structures bend the terrain | Every jigsaw structure but the bastion (`beard_thin`, `beard_box`, `bury`, `encapsulate`), the stronghold (`bury`) and the nether fossil (`beard_thin`). All others have none |
| Structure templates | 1,511 files, 4.24 MB (2.78 MB as compressed in the jar). Largest footprints: bastion 30×24×48, trial chambers 34×19×29, ancient city 18×31×41, end city 13×24×29, **shipwreck 9×9×28**, mansion 21×19×16, **underwater ruin 16×16×16**, ruined portal 11×17×16, igloo 7×6×9, fossil 3×3×13 |
| Features | 47 types at the top of the 240 files, more nested; the commonest: `tree` 45, `simple_block` 37, `ore` 30, `random_selector` 21, `overlay` 10. Kelp, seagrass, sea pickles and the like are no longer types of their own. SteelMC's branch knows every name I looked for. Fifteen placement modifiers |
| Biomes | Each lists `features` in eleven steps and `carvers`. So a data pack can take either away without touching terrain |
| What the server program offers | `net.minecraft.server.Main.main` and `MinecraftServer.spin` are public; `NoiseBasedChunkGenerator.buildTerrain` is public, its three parts `doFill`, `buildSurface`, `generateCarvers` are **private**; statuses are `empty … terrain, features, initialize_light, light, spawn, full` |

Still open after reading the jar: what the reports hold (they are made by running the
data generator), and everything about how the server behaves when run (section 4).

---

## 1. Where generation runs, and what the store promises

### What vanilla's order-dependence is, exactly

| Stage | Depends on | Evidence |
|---|---|---|
| Structure starts, as made | seed and position, through pure questions to the generator: biomes, base heights, column states, and the pools and templates with their sizes | `S263:steel-worldgen/src/structure/generation.rs:138-156` |
| Biomes | seed and position; a tie between two biomes is broken by what the thread found last | check §3 |
| Terrain | seed, position, structure starts within 8 chunks, biomes of the 8 neighbours | `S263:steel-core/src/chunk/chunk_pyramid.rs:385-388` |
| **Features** | see the next table | |
| Light | the blocks once the engine is idle. A chunk is lit from its own sources; a neighbour's light arrives when the neighbour is lit | `chunk_pyramid.rs:395-400`; `X:…/SteelExtractor.kt:139-145, 497-560` |
| After generation | fluids at marked positions flow one step and marked blocks take their shape as a chunk becomes full; later ticks settle leaf distances | check §8; `S263:steel-core/src/worldgen/stages/leaf_distance.rs:1-11` |

### What one feature run reads and writes

A run for chunk *c* is seeded by seed and *c* (`S263:…/feature/runner.rs:164`). The view
it works in is not 3×3: it is as wide as the step's dependencies, 8 chunks
(`region.rs:209-217`). Blocks may be **written** within 1 chunk
(`region.rs:327-333, 515-519`); blocks, block entities and heights may be **read**
anywhere in the view and return whatever that chunk holds at that moment
(`region.rs:445-451, 711-721`); biomes only within 1 (`:486-492`); ticks and marks may be
recorded anywhere in the view (`:643-660, 700-708`); structure starts are read and
**changed** in their source chunks up to 8 away (`runner.rs:300-343`).

Sorted by what it does to a fixed order. "Frozen" means the two worldgen heightmaps,
which no longer change once terrain is done (`S263:steel-core/src/chunk/status.rs:91-108`).

| Who | Reads and writes | Kind |
|---|---|---|
| All placed features, by the data: origins lie in *c* (`in_square`), the widest `offset` is ±10 (pointed dripstone, sulfur spike: one column), ±8 for twisting vines (which then spread 8), ±7 for patches | within 16 blocks of *c* | **in the 3×3** |
| Large dripstone: radius clamped to 16; geode: ±16; lake: 16 wide from its origin; vegetation patch 7; disk 3; delta 7+2; netherrack blobs 7; root system 8; random neighbour spread 7; sculk patch 12 (`S263:…/features/sculk_patch.rs:94`) | within 16 blocks | **in the 3×3** |
| Not read line by line: iceberg, the extremes of the tree placers, stepped column cluster, speleothem cluster, fossils and template features at their largest rotation | believed within 16 blocks | **to be audited in G3** |
| Jigsaw, mansion, end city, fossil and other template pieces: every block of the template goes through the processors **before** the clip (`S263:…/template/placement.rs:180, 220`); rule and protected-block processors read the world block there (`template/processors.rs:119-127`), the terrain-matching projection a worldgen height (`:653-668`). Pieces are up to 48 long | reads up to 3 chunks out, live and frozen; **the result is thrown away** outside the clip, and randomness is per position | **harmless** |
| Shipwreck: settles its height once, from the worldgen heightmap over its whole footprint (`…/piece_placer/template_piece.rs:134-150, 172-204`); footprint at most 28 long in the data | frozen heights up to 2 chunks out; state of the start changed once | **frozen, 2 out** |
| Igloo: one column of the worldgen heightmap near the piece (`template_piece.rs:208-236`); ruined portal, mineshaft, buried treasure: worldgen heights at their own blocks | frozen | **in the 3×3** |
| Ocean ruin: recomputes its height **at every chunk it is placed in** from the current blocks under its footprint, at most 16×16 (`template_piece.rs:238-298`) | live, within 1 chunk; the start changed each time | **in the 3×3**, order-dependent |
| Swamp hut, jungle temple: average of the final heightmap **inside the chunk being placed**, once (`…/piece_placer/scattered_feature.rs:44-73`) | live, own chunk; the start changed once | **in the 3×3**, order-dependent |
| **Desert pyramid**: lowest value of the final heightmap `MotionBlockingNoLeaves` over its whole 21×21 box, once (`…/piece_placer/desert_pyramid.rs:152-176`) | **live heights up to 2 chunks out**; the start changed once | **live, 2 out** |
| Monument, stronghold, fortress, mansion, end city, mineshaft pieces | blocks beside what they place | **in the 3×3** |
| Ticks and marks outside the write radius | possible by the rule above; no case found | **recorded** (below) |

### Is shape C still sound?

Shape C: colour chunks by (x mod 3, z mod 3); a run sees exactly the runs of lower
classes.

- With everything in the 3×3 it holds, as the review confirms.
- **Frozen reads two chunks out** do not disturb it, if they are answered from
  `terrain(position)` and if the harness that drives the official server has terrain
  there before any run (section 4). So the view a run is given offers frozen heights for
  5×5 chunks and blocks for 3×3, and the terrain that C needs grows by one ring. That is
  in the figures below.
- **The desert pyramid's first placement** is the one live read beyond the 3×3 that the
  code shows. C answers it from terrain and the runs of lower classes; for that the
  scheduler gives this one run a dependency one ring wider (lower classes within 3
  chunks, not 2), and which runs these are follows from the structure starts, which are
  pure. That is still an order vanilla can take, as long as the run that places a
  pyramid first comes before the runs of its own class three chunks away, and pyramids
  are at least 8 chunks apart (`separation: 8` in the jar), so no two such runs
  constrain each other. The harness has to keep that order within a class. It is a
  **named case**: the fixtures mark every chunk a pyramid touches, and they are compared
  on their own.
- The view **records** every live read, tick and mark beyond the 3×3 instead of
  panicking (a panic would fire on every large template piece). Fixture runs must count
  none but the named case; a tick or mark beyond the 3×3 is dropped and counted.
- The review's alternative, classes that isolate reads at distance 2, would need runs of
  a class 4 apart, sixteen classes (not twenty-five: writes reach 1 and reads 2, so two
  runs meet at distance 3). Its chains are twice as long. Not needed for one piece type.

So: **sound with a wider frozen view and one named exception, pending the audit of G3.**
If G3 finds a live read two chunks out that is not rare, C falls and the recommendation
becomes B (below), not sixteen classes.

### What C costs, computed

Method: every ranking of the nine classes (8! up to translation) was searched for the
reach of a run's dependencies; the 168 rankings of least total reach were then counted
exactly on Clustine's real view. The review's hand figures are confirmed: row-major
reaches 16 and 8 along z and 4 and 2 along x; no ranking has a total below 24.

The view at distance 8 is 19 chunks across, 329 chunks (`services/edge/src/fanout.rs`,
`view_area`). Light needs the finished neighbours, so 409 chunks are finished. "Terrain"
counts every chunk a run may read, frozen heights two out included.

| For a view 19 across | Feature runs | Chunks of terrain |
|---|---:|---:|
| Vanilla itself (lights before neighbours are finished) | 409 | 497 |
| Shape A or B, finishing 409 chunks | 497 | 697 |
| **Shape C, ranking with reach 6, 6, 5, 7** (the most even) | **822** | **1,337** |
| Shape C, ranking with reach 2, 4, 8, 10 (the least for a join) | 812 | 1,301 |

| Per chunk walked | Feature runs: mean (worst step) | Terrain: mean (worst step) |
|---|---:|---:|
| Shape A or B | 23 (23) | 27 (27) |
| **Shape C, even ranking**, any direction | **28 to 30 (66)** | **37 (87)** |
| Shape C, long ranking, across its long side | 32 (72) | 43 (105) |
| Shape C, long ranking, along it | 26 (54) | 31 (69) |

| Cold, nothing cached | Feature runs | Terrain |
|---|---:|---:|
| One chunk with its light, shape B | 25 | 81 |
| One chunk with its light, shape C | 147 to 157 | 364 to 400 |
| A 5×4 patch of chunks with light, shape B | 72 | 156 |
| A 5×4 patch, shape C | 228 | 493 |

The work comes in lumps under C (the lattice has period 3), so the generation service
keeps one ring ahead of the views. A cache must hold **results of runs** (what each run
changed, per chunk) and terrain, not chunk states: a chunk passes through nine states
and cannot be evicted alone. Sized in W5; a guess is 10 to 30 KB per run.

### The shapes

| | **A** staged, stateful | **B** pure, each run alone | **C** pure, fixed order |
|---|---|---|---|
| Equal to vanilla at features | for vanilla in the same order | no: wrong wherever two chunks' features meet, and pieces that settle once settle in every chunk | for vanilla in that order, the named case apart |
| Provable by hash | in a test with a forced order | no, only counts | yes, by the harness of section 4 |
| `generate(position)` a function | no | yes | yes |
| The world depends on who asks first | yes; Clustine's differential tests would no longer compare equal worlds | no | no |
| Durable proto chunks, crash order | needed | no | no |
| Replicas of the store, later | state to replicate | nothing | nothing |
| Cost | 1 | 1 | 1.6 runs, 1.9 terrain at a join; 1.3 to 1.6 walking; 5 times for a lone cold chunk |

**Recommendation: C with the even ranking, B as a switch in the same code**, decided in
ADR-0020 after the two spikes and the audit. The policy lives in one place (which
earlier runs a run sees), so phases W and T do not wait for it.

- If G2 (the server inside a Java program) fails by every route, C is still what I
  would build, because B is known to be wrong and A changes what the store is; but the
  parity matrix then says "implemented" and not "verified" for features, and the record
  says that C's exactness rests on SteelMC's fixtures for the runs and on argument for
  the order.
- If T8 measures C as too slow for the targets of section 6, B by the switch, with the
  difference counted.

### What the store promises

Today: an unmodified chunk is generated again when needed, and a world refuses another
generator (`docs/world-format.md:14-16`). Kept: generation is pure, so nothing that
generation does needs to be durable for correctness. Changed, **if the owner agrees
(question 2)**:

- **A chunk that a region was handed is saved.** The store writes it behind the answer,
  without waiting and without a sync of its own; it becomes durable with the next sync
  the store makes anyway. Until then purity covers it: a crash loses nothing that cannot
  be made again. The principle in `world-format.md` becomes "what a region was given is
  stored".
- A manifest says "as generated by version *n*, unchanged", so that a later tool can
  drop or remake such chunks on purpose.
- A world then **accepts** a newer generator: saved chunks stay as they were, as in
  vanilla; new ones follow the new generator. Where a fix changed anything, a seam of a
  few blocks can appear between the two, which is named as a limit.
- The path that brings a region back (`Job::Restore`, `Job::Fold`,
  `services/worldstore/src/chunks.rs:353-386, 418-436`) then generates only chunks
  handed out in the moments before a kill. W5 measures a restore after a kill with
  nothing cached, on the imported world, against M3's "5 to 7 seconds"
  (`docs/roadmap.md`, M3 plan).

Generation leaves the chunk thread for a pool (W5). That changes an agreed
specification and is written into ADR-0020 with new text for ADR-0017's test T10
(`docs/adr/0017-the-end-of-the-stripes.md:2262-2285`), which today holds a load inside
the generator and expects the barrier behind it to wait:

- **The barrier and a checkpoint wait for every load asked before them**, generated or
  not: the chunk thread keeps the order of answers and only the work moves.
- **A panic in generation** is caught at the pool, logged with seed, position and
  generator version, and answered as `StoreReply::Unreadable` (as an unreadable stored
  chunk is, `chunks.rs:264-270`). The port carries a `todo!` and many `panic!`s
  (check §2).
- There is no separate cache on disk any more: the store's own files hold finished
  chunks, written whole or not at all and checksummed as today. Intermediate results
  live in memory only.

**What the reviewer should attack now:** the audit list above and what G3 adds; the
pyramid argument; whether write-behind saving can let a region see a chunk that differs
from what a later load finds (it must not: same version, pure); the barrier's new text;
the size of the cache of run results.

---

## 2. Crates, interfaces and shared types

The shared types are fixed in **G5**, after the spikes, the audit and the three records,
and before anything is delegated. Nothing fixed there is to change in a later step; what
the first draft changed later is in them now.

**Fixed in G5:**

- `Chunk`: sections; per section 64 biome cells (uniform or mixed); block entities
  (position, type, NBT kept opaque); **light** (per light section: none, empty, or 2,048
  bytes; sky and block), present when the chunk is as generated; a flag **as generated**
  that the simulation clears at the first block change; **marks** for post-processing
  and **scheduled block and fluid ticks** from generation (carried and stored from the
  start, used in phase B).
- The representation of a section in memory and on the wire, **chosen by measurement**
  on chunks of the official server read in G1 (today 4,096 ids, 8 KB, in both:
  `crates/clustine-world/src/section.rs:15-19`). Candidates: palette with packed indices
  as on disk; the same compressed on the wire.
- Format version 2 of the store: biome container in the section encoding; "flag 0" stays
  "all air, every cell the air biome"; one content-addressed blob per chunk for block
  entities, light, marks and ticks; "as generated, version" in the manifest.
- `ChunkGenerator`: `generate(position) -> Chunk`, `settings()`, `spawn()`.
- The view of a run (`Neighbourhood`): blocks, block entities, final heights and biomes
  over 3×3, writable; **frozen worldgen heights over 5×5**; structure starts by source
  chunk, changeable; ticks and marks; a recorder for anything beyond.
- The traits the generated routers and rules implement; the shapes of the generated
  tables, from the inventory of section 0.
- The test profile (section 4).

| Crate | Holds | Ported from (`S263:`) |
|---|---|---|
| `clustine-data` (exists) | + per state: light emission, dampening, **face shapes for states that occlude by shape**, fluid and level, is air, blocks motion, leaves, replaceable, **opens or is used when clicked** | values from the jar (G6) |
| `clustine-noise` (new) | random numbers, noises, splines, `Mth` tables; scalar; maths through `libm` | `steel-utils/src/random/`, `steel-math/`, `steel-worldgen/src/noise/` (the six noise files), `src/density/spline_eval.rs` |
| `clustine-worldgen-data` (new, generated, committed) | transpiled routers and material rules for overworld, nether, end; noise parameters and settings; biome parameter lists; features, placed features, providers, carvers; structure sets, pools, processors; **template metadata** (sizes, jigsaw blocks). Template **blocks** by the owner's answer to question 3 | emitted by `tools/datagen`; emitter ported from `steel-worldgen/build/density/` (scalar half), `steel-registry/build/` |
| `clustine-terrain` (new) | biome source, noise chunk, aquifer, ore veins, beardifier, surface, carvers, worldgen heightmaps | `steel-worldgen/src/`, `steel-utils/src/climate/`, `steel-core/src/worldgen/{generator,surface,carver}` |
| `clustine-features` (new) | placement, providers, the order of features, Java's hash set, every feature type | `steel-core/src/worldgen/feature/`, `steel-utils/src/value_providers/` |
| `clustine-structures` (new) | starts, placement, references, jigsaw, pieces, templates | `steel-worldgen/src/structure/`, `steel-core/src/worldgen/{structure,template}/` |
| `clustine-light` (new) | light of one chunk from the blocks of its 3×3; pure | the rules of `steel-core/src/chunk/light/`; not its queues |
| `services/worldgen` (exists) | the generator: pool, cache of run results, the order of runs, light of finished chunks | Clustine's own |
| `tools/datagen` (exists) | + emitters; + the Java program's part that needs no server | |
| `tools/fixtures` (new) | official server with console and generated data packs; region-file reader; the Java program's part that runs a server; hash fixtures | Clustine's own; logic of SteelExtractor (CC0) |

**Light.** Whoever makes a chunk makes its light: the generation service for a finished
chunk (from the finished 3×3, which is why 409 chunks are finished for a view of 329),
the import for a chunk of an official world. The edge sends that light as it is while
the chunk and its neighbours in the replica are as generated, which costs nothing. Where
one of them has changed, the edge relights the chunk from its replica **on a blocking
pool, never on the fan-out task** (which is one task for all players and encodes packets
lazily, `fanout.rs:132-153, 463-472`), and sends the chunk when its light is there; a
changed chunk invalidates the packets of its eight neighbours. A neighbour that is not
in the replica counts as unchanged; if it arrives changed, the chunks beside it are
relit and a light update follows. So nothing is relit while walking through an untouched
world, and the first draft's relighting of the outer ring is gone. Cost: light adds to
every chunk message and stored chunk (a guess: 10 to 20 KB before compression), measured
in W5.

---

## 3. Phases and steps

Sizes are SteelMC's lines (check §9). Each step is one verified commit.

### Phase G: ground rules and spikes

| # | Scope | Verified by / yields |
|---|---|---|
| G0 | `cargo datagen` run here; the inventory of section 0 completed with the reports (biome parameters, what `blocks.json` has) and kept with ADR-0019 | The inventory |
| **G1** | **Spike: terrain from the unmodified server.** The oracle with a console (its standard input is closed and it has 1 GB: `tools/botswarm/src/oracle.rs:87-91`), a normal world on seed 13579, ticking frozen, single chunks force-loaded; a tool that writes three data packs from the jar's own files (never committed): biomes without features; the same without carvers; the same with an empty material rule; a reader for region files. Then: which statuses are saved; whether a terrain-status chunk is the same in two runs with different loaded chunks; **its MD5 against SteelMC's `terrain` hash for the same chunk** | Whether this is the terrain reference (section 4). The first evidence about SteelMC's fixture, and about what its branch does with `exclusion` and `chunk_surface_level` |
| **G2** | **Spike: the server inside a Java program**, compiled with `javac` against the jar's own classes. Routes, in order: call `Main.main` on a thread and find the server through the server thread by reflection; an agent written with the JDK's own class-file API that keeps the server when it is constructed; Fabric (needs the owner, question 10). Then: one chunk asked for at `terrain` and hashed (must equal G1's); one cluster of SteelMC's brought to terrain with two rings, decorated in ascending x, z, hashed (compared with their `features` hash) | Whether features, light and the rest can be judged at all, before ADR-0020 claims it |
| G3 | The audit of section 1 finished: the feature files not yet read, with the data's radii; C's reach and counts recomputed by a test | The list in ADR-0020 |
| G4 | **ADR-0019** data and licences (section 7's questions 3 and 4; a notice that tables made from Mojang's data are not under the AGPL; budgets for size and build time). **ADR-0020** shape, order, the store's promise, the pool, the barrier, panics, with the spikes' results and what follows if G2 failed. **ADR-0021** what a chunk carries, light, the representation. An independent reviewer goes over the three | Review; the owner's questions asked with them |
| G5 | The shared types of section 2; empty crates with `NOTICE`s; the test profile | All existing tests; the flat comparison with the official server |
| G6 | The Java program without a server: block properties, face shapes, fluid states, "used when clicked"; biome parameters if the report lacks them; noise and router values at points; hash-set orders | `datagen --check`; known values |
| G7 | `tools/fixtures` whole; the fixture format; the first fixtures: starts as made, biomes, terrain under each of the three packs, three dimensions | Regenerating gives the same bytes |

G0, G1 and G2 start at once and are independent. G3 can run beside them. G4 follows
them, G5 follows G4, and nothing of W or T starts before G5 except the two crates that
need no shared type (`clustine-noise`, `clustine-light`), which may start after G4.

**If G1 fails** (packs not accepted, or the terrain ring is not clean): terrain comes
from G2's program, which asks for the terrain status in a world where nothing is ever
decorated; the fill and surface gates of T4 and T5 are then lost, and the plan says
plainly that those 5,000 lines are debugged from dumps. **If G1 and G2 both fail:**
terrain and features are judged by SteelMC's fixtures alone, and the milestone's claim
drops from "equal to the official server" to "equal to SteelMC's reference".

### Phase W: Clustine's side of a real world

| # | Scope | From | Verified by |
|---|---|---|---|
| W1 | `clustine-light` | rules of 10,305 | Hand-built cases in CI. On the owner's machine: light over the official blocks equals the official light **for chunks whose eight neighbours are lit too** (the inner 14×14 of a 16×16 forced area; no chunk of a lattice). Written from the fixture format by someone who has not seen the engine |
| W2 | Real heightmaps, fluid counts, biome palettes, block entities in the packet | 793 | Unit tests; official chunks on the owner's machine |
| W3 | Light at the edge as in section 2 (**not delegated**) | own | Bots: no light is computed for an untouched world; a block placed by one bot gives a later arrival the right light; nothing is lit on the fan-out task |
| W4 | A generator that serves region files the official server wrote (`--world-from`); full chunks only, proto chunks count as absent; nothing of Mojang's committed. Can be written beside W1 to W3; **verified after them** | G1's reader | Chunk packets equal the official server's for the same world, through the protocol (ignored test) |
| W5 | Generation in a pool; saving what was handed out (if agreed); the barrier; panics; the cache of run results; sizes of chunks measured; **a restore after a kill, timed** | own | Store tests incl. kills during generation and the new T10; chaos and move tests on the imported world |
| W6 | `--seed`; version in the settings; spawn x, z from the generator, fixed for a world, **and a joining player put on the highest block of that column as it is now**; placing into grass, water, snow replaces; **a click on a block that is used when clicked places nothing**; doors, trapdoors and fence gates open and close | own | Simulation tests; oracle for placement and for a door |

**Then the owner tries:** join a world the official server made: colours, light under
trees and in caves, chests and signs, digging and building, doors, a second client.

### Phase T: overworld terrain

| # | Scope | From (lines) | Verified by |
|---|---|---|---|
| T1 | `clustine-noise` | 1,508 + 815 + 2,866 + 155 | SteelMC's noise samples; values from the jar's classes (G6); the `Mth.sin` table and the beardifier's kernel against the JVM's |
| T2 | The emitter in `tools/datagen`; routers, noise data, material rules for three dimensions, following the jar's data where SteelMC departs from it; **build time of the generated crate measured against ADR-0019's budget** | 3,873 + 2,404 + 541 + 426 | `datagen --check`; router values at points equal the jar's |
| T3 | Biome source and climate | 1,763 | Biomes of every fixture chunk; a differing cell passes only if Clustine finds the two biomes exactly tied there, and is counted. SteelMC's biome hashes |
| T4 | Noise chunk, aquifers | 499 + 1,085 + part of 3,038 | **Fixture under the pack without rules and carvers** |
| T5 | Surface rules and ore veins | 926 + 267 | **Fixture under the pack without carvers** |
| T6 | Carvers | 1,773 | Fixture under the pack without features, structures off |
| T7a | Structure data model; template metadata; pools | 3,286 + about 550 | Unit tests; `datagen --check` |
| T7b | Placement; starts; jigsaw layout; stronghold and nether fossil layout (the three kinds that bend terrain); references; beardifier | 718 + 1,477 + 483 + about 715 + 1,523 + 279 + 915 + 234 + 287: **about 6,600** | Starts of those types, as made, equal the official ones; then T6's fixture with structures on, and SteelMC's `terrain` hashes |
| T8 | The spawn area made when a world is made; speed measured (section 6) | own | Spawn x, z equal the official world's; the measurement in the roadmap |

**Then the owner tries:** `--seed 13579` beside the official server on the same seed:
the same mountains, coasts, rivers, caves, ore veins and biomes at the same coordinates.

T1, T2, T3 run in parallel with each other and with W. T4 to T6 are one author's, in
order. T7a needs T2's data shapes and question 3 answered; T7b needs T7a and T3, and
base heights from T4.

### Phase F: features

| # | Scope | From (lines) | Verified by |
|---|---|---|---|
| F0 | The Java program of G2 made whole: any area, Clustine's order, pyramids first within a class, terrain two rings beyond every run before the first run; the world saved and read back, so that blocks, block entities, **ticks and marks** come through the one reader; light read at the light status, nothing lit before the last run | about 900 of Kotlin as a model | SteelMC's `features` and `light` hashes under their order |
| F1 | Generated feature data; placement, providers, predicates, order of features, Java's hash set, the view with its recorder; the order of runs (C, B as a switch); every feature type as a stub that counts; **`ore` for real** | 5,124 + 6,257 + 1,163 + 1,793 | The order of runs against a plain model that runs toy features class by class over a large area (test from the specification, by someone else); the first harness hashes for chunks with ores only |
| F2…F7 | Feature types in batches, a subagent each on files of its own: (2) disks, springs, lakes, blobs, columns, simple blocks, selectors, freezing; (3) trees; (4) patches, bamboo, vines, mushrooms; (5) dripstone, speleothems, geodes, sculk, multiface growth, monster rooms, fossils; (6) icebergs, spikes, wells, the rest; (7) the Nether's and the End's. Where vanilla makes an entity, the port draws the same random numbers and drops the entity | 2: 1,100; 3: 5,153; 4: 1,100; 5: 4,000; 6: 1,100; 7: 1,500 | Per batch: fixture chunks whose features are all implemented equal the harness's hashes; the recorder counts no read beyond the 3×3 |
| F8 | Final chunks against the official server's full chunks, ticks frozen | | A count per chunk, expected only where a fluid flowed or a block took its shape as the chunk was finished |

SteelMC's leaf-distance solver (`stages/leaf_distance.rs`, 494 lines) is **not ported**:
by its own header it replaces what vanilla settles by ticks, and no hash of theirs
covers it. Clustine keeps the leaves as the features left them, which is what the
official server has with ticks frozen; phase B's ticks settle them as vanilla does.

**Then the owner tries:** forests, flowers, ores, lakes, geodes, dripstone caves, snow.

### Phase S: structures

| # | Scope | From (lines) | Verified by |
|---|---|---|---|
| S1 | Placing a template: palettes, processors, liquids, block entities, markers | 3,117 + 801 + 179 | Harness hashes of chunks with jigsaw structures |
| S2 | The other layouts and their placers, a subagent each: (a) mineshaft; (b) ocean monument; (c) mansion, fortress, end city; (d) pyramid, temple, hut, igloo, shipwreck, ocean ruin, ruined portal, buried treasure; the stronghold's placer | a: 2,001; b: 2,804; c: 4,100; d: 3,700; 920 | Starts as made; harness hashes; pyramid chunks as the named case |
| S3 | Structures in the feature step with their steps and seeds; where the template blocks come from, by question 3 | part of 409 | All fixtures, structures on |

**Then the owner tries:** villages, mineshafts, a stronghold, a monument. Chests show and
do not open; nobody lives there.

### Phase B: block behaviour that real terrain makes necessary (question 5)

In `clustine-sim`, one author, beside F and S. Sizes are guesses.

| # | Scope | Verified by |
|---|---|---|
| B1 | ADR-0022: neighbour updates and scheduled ticks in a region, across chunks and across regions (passed on as block actions are today), in the region's durable state; reviewed | Review |
| B2 | Support: the other half of a double plant or door goes with it; plants, torches and snow go when what holds them goes | Simulation tests; oracle |
| B3 | Sand, gravel and the like fall when undermined: at once, to where they would land in their column (no falling entity yet, named) | as B2 |
| B4 | Scheduled ticks in region state: export, restore, merge, split | The existing state tests extended; tests from the record by someone else |
| B5 | Water and lava spread and drain by vanilla's rules | Differential tests against one region; oracle for simple cases |
| B6 | Marks and ticks from generation honoured when a region first takes a chunk; leaves settle | F8's count falls to what is named |

### Phase D: the Nether and the End

| # | Scope | Verified by |
|---|---|---|
| D1 | Nether: biome source, legacy random, terrain, carvers; features from F7 | Fixtures of the Nether |
| D2 | End: islands, biome source, features | Fixtures of the End |
| D3 | A world of one dimension chosen when it is made. Where one stands is Clustine's own rule and named: in the End the highest block at the origin; in the Nether the first floor with two blocks of air above it near the origin | The oracle's join comparison per dimension type |
| D4 | Parity matrix, architecture, roadmap, world format, known limits | CI |

---

## 4. How exactness is tested

**Where each reference comes from, and where it can be compared:**

| Aspect | Reference | Reproducible? | Compared in CI? |
|---|---|---|---|
| Noise, random numbers, router values, hash-set order | Java program, no server (G6) | yes | yes (values committed) |
| Block properties, biome parameters | Java program or reports → generated tables | yes | the tables are what is built; `datagen --check` on the owner's machine |
| Structure starts as made | region files of the unmodified server: starts in chunks that never decorated and whose pieces touch no chunk that did | yes | yes (hashes committed) |
| Biomes | region files, any chunk at `biomes` or later | yes but for exact ties (rule in T3) | yes |
| Terrain; fill only; fill and surface | region files of the unmodified server under generated data packs: chunks at `minecraft:terrain` (G1) | yes, to be shown in G1 | yes |
| Blocks of the features stage, block entities, ticks, marks | G2's program in Clustine's order, the world read back by the same reader; only chunks whose whole cone of dependencies lay inside the decorated rectangle | yes | yes |
| Light and heightmaps of generated chunks | G2's program, at the light status, for chunks whose neighbours are lit | yes | yes |
| Light and heightmaps as functions of blocks (W1, W2) | official full chunks; the blocks are not committed | yes for the inner chunks | **no: owner's machine**, like the oracle tests |
| Chunk packets (W4) | the official server through the protocol | per run | no |
| Finished chunks of an unmodified world | region files | **no** (order of runs, post-processing) | no: a count, on the owner's machine |
| SteelMC's fixtures | the reference clone, by an environment variable | | no; a second opinion |

**Seeds:** 13579, 0, and one 64-bit seed with the top bit set, chosen in G7 and written
down. **Places** per seed and dimension: the spawn; SteelMC's first cluster at chunk
(−418462, 366791), far enough out for single precision to show; one near the origin at
negative x. **Sizes:** for terrain a lattice of single forced chunks 5 apart (each
leaves 16 terrain-status chunks); for features a tracked 10×10 inside a decorated
rectangle that is wider by the ranking's reach plus one, with terrain two rings beyond.

**What is committed:** hashes, about a megabyte; the Java program; the pack-writing
tool; seeds and places. **Never:** blocks, region files, data packs, the jar.

**The test profile** (review, finding 12). The workspace has no `[profile]` today and
tests run unoptimised. Added in G5: `opt-level = 3` for the generation crates in the dev
and test profile (`[profile.dev.package.<crate>]`), so that the four commands of
`CLAUDE.md` stay as they are. In CI's ordinary run: the value tests, terrain hashes for
about 300 chunks and **one** features cluster per dimension (a cluster costs about 800
chunks of terrain under C). The full set is `#[ignore]`d and run with `--release` by a
workflow of its own on pushes that touch generation, as the `Cluster` workflow is, and
by `tools/check.sh` on request. ADR-0019 sets a budget for the build time of the
generated crate beside its size (T2 measures; if a router takes minutes to compile, the
Nether's and the End's are interpreted from tables instead).

**Who writes the tests:** fixtures, formats and comparisons are written from this
section by someone who does not write the generator, before the stage they judge.

---

## 5. What a player meets within minutes, and where it is

By the rule in `CLAUDE.md` ("do not defer what a player would notice within minutes").

| What a player meets | Where |
|---|---|
| Light under trees and in caves; biome colours; chests and signs visible | W1 to W3, G5 |
| A block placed onto grass, into water or snow | W6 |
| **A click on a door, chest, lever, button, bed or crafting table with a block in hand places the block** (the simulation places against anything that is not air: `crates/clustine-sim/src/region.rs:830-866, 971-974`) | W6: nothing is placed; doors, trapdoors and gates work. Chests and tables do nothing yet (no inventories), named |
| **Joining inside a block or in the air** once someone has built or dug at the spawn point (it is one fixed point: `bin/clustine/src/lib.rs:69-71`) | W6: the column is fixed, the height is found as the world is now |
| The top of tall grass left when the bottom is broken; flowers on nothing; a door's upper half | B2 |
| Sand and gravel that hang when dug under | B3 |
| **Water that does not close over a broken block, and stands in walls where vanilla lets it run** | B4 to B6. This is the part that could be argued out of the milestone; the rule says it cannot wait, so it is in unless the owner says otherwise (question 5) |
| Walking through blocks | not met: the client stops its own player |
| No animals, villagers, monsters; chests that do not open; no loot | named limits (entities, inventories) |
| Grass spreading, crops, ice forming | named limits (random ticks) |
| No falling animation for sand | named |

---

## 6. Speed

Guesses until T8 measures. Generation is measured when the other timing work on this
machine is not running.

- **Players' speed.** Sprinting 0.35 chunks a second; flying in creative mode 0.7;
  **flying while sprinting about 1.35** (21.6 blocks a second, from memory). The target
  is set for the last.
- **What that asks.** Under C with the even ranking: on average 37 chunks of terrain and
  30 feature runs per chunk walked, so about **50 of terrain and 40 runs a second**,
  sustained, with a worst step of 87 and 66 that one ring of look-ahead has to smooth.
- **What there is.** SteelMC states 2,560 finished chunks a second on sixteen cores with
  SIMD. Scalar, on four of six processors, I expect 150 to 400 chunks of terrain a
  second. The margin is real but not wide.
- **A join pays nothing for the first area:** the spawn area (1,337 of terrain, 822
  runs, 409 lit) is made when the world is made, before the server listens. Target:
  under 15 seconds on the owner's machine.
- **Targets:** a player flying while sprinting in creative mode at view distance 8 never
  reaches the edge of what was sent; a region is brought back after a kill within the
  time M3 promised.
- **Measured by:** an ignored test that makes a view cold and reports chunks a second
  per stage; a bot that flies a straight line at 21.6 blocks a second and records for
  every chunk the time between entering view and arriving; W5's restore after a kill.
  They report; none asserts a time in CI.

---

## 7. Questions that are the owner's

| # | Question | Recommendation | If there is no answer |
|---|---|---|---|
| 1 | Features across chunk borders have no single right answer in vanilla. Shall "equal to the official server when it decorates in Clustine's fixed order" be what block for block means there, at 1.6 to 1.9 times the work of a first view? | Yes | C is built; B if T8 finds it too slow, with the difference counted |
| 2 | **Generator fixes after worlds exist** (groundwork question 7). (a) Save every chunk a region was handed; a world then keeps what it showed and accepts a newer generator, as vanilla does. Costs disk like vanilla's (a guess: 5 to 15 KB a chunk) and ends "only changes are stored". (b) Keep regenerating; every fix either refuses older worlds or silently remakes their untouched chunks beside the touched ones. (c) Regenerate and keep old generator versions in the binary | (a) | (a): it can be undone (saved unchanged chunks are marked and can be dropped), while (b) cannot be undone for chunks already shown |
| 3 | **Structure templates** are Mojang's buildings (1,511 files, 4.2 MB). (a) Commit their sizes and connection points only, which starts and terrain need; read the blocks from the operator's own jar when the server starts, and build no structure blocks without it. (b) Commit them whole as packed tables, as Pumpkin does; then images and the repository carry them | (a) | (a): it can be undone, and a build still needs no jar. Needed before T7a |
| 4 | **Block properties and biome parameters** come from the jar's code and registries, not from its world-generation data: light, shapes, fluid states per block state; which climate gives which biome. May tables made from them be committed, with a notice that such tables are Mojang's data and not under the AGPL? | Yes; both projects do, and without it a build needs the jar | Committed, because the owner's stated aim was that a build needs no jar; this cannot be taken out of the history again, so it is asked first |
| 5 | **Plants and sand that lose support, and flowing water** (phase B): in this milestone, as the project's rule says, at about a dozen steps in the simulation and a record of its own? | Yes: B2 and B3 in any case; B4 to B6 beside F and S | In, by the rule |
| 6 | The Nether and the End as worlds of one dimension for now; portals and several dimensions per world later? | Yes | Yes |
| 7 | May Mojang's own source be read (groundwork question 3)? The plan needs the jar's behaviour, its class and method names, and SteelExtractor's calls. G2's first two routes call `Main.main` as it is and need no more; rebuilding what `Main` does by hand would | No | Not read; asked again only if G2 comes down to that |
| 8 | Large biomes, amplified, caves and floating islands (the jar has settings for them) and custom data packs (which transpiling rules out) | Out of this milestone; the two overworld variants are one more emitted router each, later | Out |
| 9 | Speed: "the spawn area in 15 seconds; flying while sprinting never reaches the edge; view distance 8" | Yes | These |
| 10 | Only if G2's first two routes fail: Fabric and its downloads, to run SteelExtractor's way | Asked then | Features are judged by SteelMC's fixtures and by counts |

Settled and not asked again: the goal, committing generated Rust from world-generation
data, all dimensions, structures, no first look with Pumpkin, stable Rust. The first
draft's question about a JDK and disk space is gone: `javac` is installed and 21 GB are
free.

---

## 8. Risks

| Risk | How likely, how bad | Answer |
|---|---|---|
| The terrain ring is not clean under the packs, or packs are refused | Unknown; the gate of phase T | G1, first thing; fallback in phase G |
| A server cannot be run inside a plain Java program | Possible; everything from features on | G2, first thing; three routes; what follows is written in section 1 |
| SteelMC's branch is not the released 26.3: it ignores two fields of the released noise settings, its noise test names a pre-release, it carries a `todo!` | Likely in details | The gate is the official server; G1 and G2 tell early how good their fixture is; the emitter follows the jar |
| A live read beyond the 3×3 that the audit missed | Possible | The recorder counts in every fixture run; G3 |
| C too slow, or its cache too large | Measured cost 1.6 to 1.9 at a join | B by a switch |
| Single precision without SIMD not bit-equal; `exp`, `ln`, `pow`, `sin` differ between Java, Rust and platforms | Low; fatal for an equality | T1 and T2 compare bits with the JVM before terrain exists; `libm` everywhere; vanilla's tables compared once |
| Vanilla is not a function at biome ties | Certain, rare | T3's rule; named |
| The generated crate compiles slowly or grows | Possible | Budgets in ADR-0019; measured in T2; tables for anything large (the JSON is 2 MB, so megabytes, not Pumpkin's 86) |
| Real chunks with light are large in memory, on the wire and on disk | Certain | Measured in G5 on official chunks, again in W5 |
| The pool changes the store's order of answers | A specification changes | ADR-0020, T10's new text, kills during generation |
| Phase B reaches into the simulation's state, where ordering mistakes hide | Certain | A record and a review of its own; one author; tests from the record by someone else |
| The size: 69 feature kinds, 16 structure types, about 65 steps | Certain | Stubs that count; subagents on files of their own; shared types fixed in G5 |
| This machine's defective memory | Seen before | A hash that differs once and not again is looked into and judged on GitHub |

**If block for block cannot be reached** for a stage: its comparison becomes a count,
the count and its cause go into the parity matrix, and the next stage does not wait.

---

## 9. Still not known

- Whether the unmodified server accepts the three data packs, saves proto chunks with
  their status, keeps generating forced chunks with ticking frozen, and leaves the
  terrain ring clean (G1). All four are from memory of vanilla.
- Whether a server can be reached inside a plain Java program (G2).
- What the data generator's reports hold (G0).
- Whether SteelMC's branch matches the released 26.3, and why it ignores `exclusion` and
  `chunk_surface_level`.
- The feature files not yet read for far reads (G3); how often a pyramid's first
  placement really reads two chunks out.
- Every speed and size: milliseconds per chunk, bytes per chunk with light, the cache of
  run results, the build time of the emitted routers.
- What the client does with missing block entities; how two face shapes occlude light
  (from memory: together, when their union covers the face).

---

## What the review changed

| # | Finding | Verdict | What changed, or why not |
|---|---|---|---|
| 1 | The terrain-status ring has been written into by neighbours' features | **Accepted** | Confirmed: only features ask for terrain, at radius 1, and they write at radius 1 (`chunk_pyramid.rs:390-392`). The reference is now the unmodified server under generated data packs without features (the review's "cheaper thing", made the main route, since every biome lists `features` and `carvers` in the jar), with the Java program as cross-check and fallback; spike G1 |
| 2 | A run is not a function of its 3×3 | **Accepted; one detail rejected** | Confirmed at every cited line. Section 1 lists what reads and writes what; the view is 3×3 live and 5×5 frozen with a recorder, not a panic; the pyramid is a named case; the trait is fixed after the audit. Rejected: "twenty-five classes" to isolate reads at distance 2. Writes reach 1 and reads 2, so runs meet at distance 3 and sixteen classes would do. And a shipwreck is at most 28 long in the jar, not 32 (32 is the code's limit, `template_piece.rs:167-170`) |
| 3 | Nothing from features on can be judged by the unmodified server; the tool was scheduled last | **Accepted** | Spike G2 before ADR-0020; fallbacks written; section 4 says per aspect where it comes from and whether CI can compare it; "same bytes twice" kept only where the reference is reproducible |
| 4 | Generator fixes after worlds exist | **Accepted** | Question 2, with (a) saving what was handed out recommended and the default; the disk cache is gone |
| 5 | C's reach and cost | **Accepted** | Computed by search and exact count: the review's hand figures hold (row-major 16/8 and 4/2; least total 24; 329 chunks). Costs in section 1; sprint flight in section 6; run results as the cache; restore after a kill measured in W5 and mostly removed by question 2 (a) |
| 6 | Light: the test at the rim, the fan-out task, relighting | **Accepted** | W1 compares inner chunks only; light is made with the chunk, the edge relights only changed neighbourhoods on a pool; face shapes in G6 |
| 7 | Three seams a player meets within minutes | **Accepted** | Clicks on used blocks and the spawn height in W6; support, falling and water as phase B, in by default; rules for where one stands in the End and the Nether; proto chunks of an imported world count as absent |
| 8 | T7's size and order; no gate for T4 and T5 | **Accepted; one suggestion rejected** | T7 split into T7a and T7b with the template metadata first, sized at about 6,600 and limited to the kinds that bend terrain (jigsaw, stronghold, nether fossil, by the jar). T4 and T5 get gates from data packs. Rejected: hashing between the three calls of `buildTerrain`: in the jar `doFill`, `buildSurface` and `generateCarvers` are private |
| 9 | Shared types were not fixed first | **Accepted** | G5 follows the spikes, the audit and the records; marks, ticks, light and the as-generated flag are in `Chunk` and format 2 from the start; the representation is chosen by measurement on official chunks; W4 is verified after W1 to W3 |
| 10 | The pool changes a specification; panics; a torn cache | **Accepted** | Section 1: the barrier's rule and new text for T10; a panic becomes `Unreadable`; no separate cache |
| 11 | Licences settled by default or not asked | **Accepted** | Questions 3 and 4; templates default to "not committed" with the metadata/blocks split; a notice for data made from Mojang's; question 7 says what G2 may come down to |
| 12 | The comparisons do not fit the checks | **Accepted** | The test profile of section 4; a small set in CI, the full set in a workflow of its own; a build-time budget |
| 13 | Things checked against nothing; a knowingly non-vanilla part | **Accepted** | The leaf solver is not ported; nothing is lit before the last run in the harness; block entities, ticks and marks are compared through the saved world; entities' random draws are kept |
| 14 | Smaller points | **Accepted** | Question about JDK and disk removed; presets asked (question 8); the oracle's memory and console in G1; the rule for biome ties in T3; F1 has a real gate |
