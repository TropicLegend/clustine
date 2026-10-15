# T8: a world's spawn, chosen as the game chooses it

- Status: **Drafted on 2026-10-15, revised after an independent review** the same day
  (its fifteen findings are answered in the last section), **and agreed**: the owner
  answered the four questions of section 6 as recommended. **Built on 2026-10-15**
  (T8a to T8g); what was judged and measured is in the roadmap. Step T8 of the [terrain plan](terrain-plan.md), with fixing the
  spawn's x, z for a world, which W6 moved here ([W6 plan](w6-plan.md), section 2).

## 1. What there is now

A player who enters for the first time stands on the column of block 0, 0, on its
highest block that blocks motion, or on the water there (since W6 the region reads
that column from its home chunk when they enter, `Region::spawn_pose`). On seed 13579
that is the sea at a coast: a player sinks unless they fly.

The spawn is decided by whoever builds a `Land` (`bin/clustine/src/lib.rs:111`,
`TerrainGenerator::spawn_point`). The world store's division takes its home chunk
from it before `meta` is read (`bin/clustine/src/cluster/worldstore.rs:24-30`,
`lib.rs:264-270`), and a table made with another home chunk is made over
(`services/worldstore/src/lanes.rs:480-486`). The coordinator builds a `Land` of its
own from `--seed` (`bin/clustine/src/main.rs:465`) and hands its spawn to workers
(`Assigned { spawn }`) and edges (`RoutingTable.spawn`). Each edge builds a whole
`Land`, scheduler included, from `--seed` only for what it tells a client at login
(`main.rs:554`). The client is never sent `set_default_spawn_position`.

## 2. What the game does (26.3, read from the pinned jar)

**When a world is made**, `MinecraftServer.setInitialSpawn`:

1. **A block to search from**, `ChunkGenerator.getOrigin`. The flat generator and any
   noise generator without spawn targets: chunk 0, 0. The overworld's noise
   generator, whose settings name targets (`spawn_target`, already emitted into
   `clustine-worldgen-data`): `NoiseSpawnFinder.findSpawnPosition`, with the samplers'
   caches enabled, and then the chunk containing the block it returns.
   - The fitness of block x, z: for each target, the sum over its five named
     functions (`overworld/continents`, `erosion`, `ridges`, `temperature`,
     `vegetation`) of the squared distance of the quantised sample
     (`(long)(value * 10000f)`, as `quantise` in `clustine-terrain` does) to the
     target's quantised range; the smallest over targets, **times 2048²**, plus
     **x² + z²** of the block itself, unscaled. The sample is taken at the quart's
     corner (`QuartPos.toBlock(QuartPos.fromBlock(x))`), y = 0. There is no depth term.
   - A pass begins from 0, 0 and then searches rings around the best so far, whose
     centre is fixed for the pass: the radius starts at the increment and runs to the
     maximum inclusive; at each step `x = cx + (int)(Math.sin((double) angle) *
     radius)`, `z = cz + (int)(Math.cos((double) angle) * radius)`, truncated; a
     candidate replaces the best only if strictly fitter; `angle += increment /
     radius` in float, and when `(double) angle > 6.283185307179586` the angle
     becomes 0 and the radius grows by the increment. First pass: increment 512 to
     2,048 (65 samples); second: 32 to 512 around the first's best (862 samples).
     The offsets are the same for every seed.
2. **A place to stand**, in chunks taken in a square spiral from the origin chunk:
   offsets begin (0,0), (1,0), (1,1), (0,1), (-1,1), (-1,0), (-1,-1), (0,-1), (1,-1),
   (2,-1), …, 121 of them (turning when `x == z`, or `x < 0 && x == -z`, or
   `x > 0 && x == 1 - z`, then `(dx, dz) = (-dz, dx)`, starting `(0, -1)`). The first
   chunk in which `PlayerSpawnFinder.getSpawnPosInChunk` finds a column gives the
   spawn. Its columns are tried x outer, z inner, from the chunk's north-west
   corner. A column (`getLevelRespawnPos`):
   - its height is the top of `MOTION_BLOCKING` (the generator's spawn height where
     the dimension has a ceiling); below the world's bottom, refused;
   - refused when `WORLD_SURFACE <= height && WORLD_SURFACE > OCEAN_FLOOR`, that is,
     when its top is a fluid, water or lava;
   - otherwise scanned down from height + 1: the first block holding any fluid
     refuses the column; the first block whose collision shape has a full top face
     gives the spawn, the block above it.
   If no chunk of the 121 has one: the origin chunk's block 8, 64, 8 (64 is
   `getSpawnHeight`, which the noise generator does not override). The spawn's yaw
   and pitch are 0.
3. A bonus chest if asked for (not here).

So the spawn is nearly always the north-west corner column of the first chunk with a
dry column. Step 2 reads full chunks, with features from neighbours. **A feature
changes the result's y where it stands on the column (a tree: on top of its canopy),
and its x, z only where it changes whether one of the columns tried before is wet or
dry** (ice from freezing, a lake, leaves over water, which `OCEAN_FLOOR` counts).

**When a player joins for the first time** (and when one respawns without a bed),
`PlayerSpawnFinder.findSpawn`: in a world whose default game mode is adventure, on
the spawn, raised until the player's box is free of blocks and liquid and lowered
onto the ground. Otherwise within the gamerule `respawn_radius` (default 10, clamped
by the world border): of the 441 columns of the square around the spawn, tried with
a stride of 17 from a random offset (`createThreadLocalInstance`, so not
repeatable), the first whose column passes the rule above and where the player's box
has no collision and no liquid; if none does, as in adventure mode. These read full
chunks too, and with the spawn at a chunk's corner three quarters of the square lie
in the three chunks beside it.

## 3. What T8 builds

**T8a: the search from the climate** (`clustine-terrain`, `spawn_search_origin(seed)
-> BlockPos`). Ported from the jar; SteelMC's `climate_sampler.rs:108` is a guide
only, as it multiplies in `f32` and wraps at an `f32` 2π. The five functions are
looked up in the emitted router by name, and an unknown name is refused when the data
is emitted. The sine and cosine are taken in doubles; HotSpot's `Math.sin` is an
intrinsic that can differ from Rust's `f64::sin` and from fdlibm in the last bit, so
the offsets are not computed at run time at all: they are the same for every seed,
and **the 927 offsets are emitted from the jar as a table** (`tools/datagen`, by
running the game's own loop), which T8a reads. Judges, without a server: the table
compared with the game's loop bit for bit (it is the game's loop, so `datagen
--check` is the judge); and `tests/java/GameSpawn.java` calling
`NoiseSpawnFinder.findSpawnPosition` for 1,000 seeds, every block equal.

**T8b: the place to stand** (`services/worldgen`). The spiral and the column rule of
step 2 over the chunks the generator makes, run once when a world is made, on the
scheduler's pool. Judges:
- the column rule and the spiral over **the official server's own chunks** (read
  from its region files with the Anvil reader the terrain tests have): must give the
  official `level.dat` spawn exactly, for ten seeds. This judges the logic apart from
  features;
- the same over Clustine's chunks, against the official spawn: x, z equal, y equal
  where the official column carries no feature. A differing x, z passes only where
  the official chunk shows one of the features named in section 2 on a column tried
  before, and is counted.
- `level.dat` is gzip with a named root; the test unzips it and skips the root's
  name, then reads `Data.spawn.pos` (an int array; 26.3 has no `SpawnX`).

**T8c: the spawn kept by the world store**, which owns the home chunk:
- **For a new world**, the store runs T8a and T8b *before* it writes `meta`, and
  writes `meta` once with a line `spawn=x,y,z`. A crash before that leaves no `meta`,
  so the world is new again, and the search, being a function of the seed, gives the
  same spawn. Raising `generator-version` keeps the line (`Meta::raise` keeps lines it
  does not know).
- **The store derives the home chunk itself** from `meta`'s spawn, where today its
  callers pass it in; callers pass only the pins. With `--pin`, the home region is
  whichever pinned area holds the spawn's chunk.
- **The store tells each region the spawn when it opens it** (in `StoreWelcome`), as
  a region cannot run without that welcome. `Assigned.spawn` and `RoutingTable.spawn`
  go, and with them the coordinator's `Land`. A coordinator that cannot read the
  store's list (it then runs on what workers report) needs no spawn.
- **`Land` is split**: what an edge tells a client at login (cheap, from the seed),
  which edges keep; the generator and the spawn, which only the store has. Neither the
  coordinator nor an edge makes a generator any more; the coordinator's `--seed` goes
  (it is refused with a sentence saying so, for one release of the README's commands).
- **The flat world** keeps 0.5, -60, 0.5, which is what the game's rule gives there.
- **Seeded worlds made before T8** have no `spawn` line: question 3.

**T8d: where a joining player stands.** W6's rule on the world's spawn column instead
of 0, 0: on the column as it is when they enter. Question 2 is whether they are also
spread.

**T8e: the client is told the spawn** (`set_default_spawn_position`, with yaw and
pitch, after login as the official server sends it), so that a compass points there.
Compared with the official server's packet in `oracle.rs`.

**No chunks kept loaded at the spawn**, as in 26.3: the game has had no spawn chunks
since 1.21.9 (no ticket keeps an area loaded for good; its tickets at the spawn,
`player_spawn` and `spawn_search`, hold chunks only while a player is placed or the
spawn is searched for, and no gamerule sets a radius). Since W6 the home region keeps
the spawn's one chunk in memory so that it can read the column when a player enters;
that is not loading in the game's sense, and when phase B brings ticks, that chunk
ticks only as any chunk does, where a player is near it.

**T8f: the spawn area made when the store starts.** The scheduler makes the chunks
of a view of 8 around the spawn as soon as the store has a spawn, before a region
asks; a region that asks meanwhile waits as it does for any chunk being made.
Chunks never handed to a region are not stored (ADR-0021, S6), so this is done at
every start, about a second of the pool. In a cluster the edge listens by itself; a
first player who joins during that second waits for their chunks as they would now.

**T8g: tests, pins and figures that assume the spawn at 0, 0.** With a seed the
spawn moves, possibly thousands of blocks out. Pins stay absolute chunk x (question
4). The tests on land (`bin/clustine/tests/terrain.rs`, `chaos.rs`'s `start_on`,
`crowds.rs`, `lights.rs`, `common/wandering.rs`, the store's `generation_tests.rs`)
take the spawn from the store and place their pins and their walks relative to it.
The roadmap's trials on land say where the boundary is relative to the spawn. The
W5m figures stay as measured and say where.

**Speed.** ADR-0021 section 5 gives T8 one measurement that is possible now: the
regions' tick times while a first view is made from nothing, a tick over 50 ms
failing it. T8 makes it (an ignored test, reporting, as W5m's are). The feature runs,
for which the terrain plan's other speed targets were set, do not exist until F, and
their measurement moves to F1.

## 4. Order

T8a and the emitted table first; then T8b; then T8c and T8d together, which change
the messages between the store, the coordinator and the regions and are **not
delegated** (they touch what a region is told when it is opened, and the edge's
login); then T8e, T8f and T8g. Each a verified commit.

## 5. Tests

- T8a: the table by `datagen --check`; `GameSpawn` over 1,000 seeds; unit tests of the
  fitness against hand-worked values, and that an unknown target name is refused.
- T8b: the two judges above; the spiral's order against a model written from section
  2 by someone else; a world in which no chunk of the 121 has a dry column gives the
  origin chunk's 8, 64, 8.
- T8c: a new world killed at every step of its making (as
  `no_crash_leaves_a_chunk_of_a_version_above_the_one_meta_names` does) has either no
  `meta` or one with the spawn; the spawn survives a newer generator version; the
  home chunk is the spawn's, also with pins, also when the spawn lies east of every
  cut; a region is told the spawn when opened; the coordinator and edges run without
  a generator; the flat world is unchanged; a cluster of processes with `--seed`.
- T8d: W6's spawn tests at another column. T8e: the official server's packet.
- Tests written from this plan by someone who has not seen the code, as before.

## 6. Questions that are the owner's

Answered on 2026-10-15, each as recommended: accept the spawn before F; players on
the spawn column; seeded worlds made before T8 refused; pins absolute. The owner also
said not to change vanilla's behaviour about spawn chunks, which in 26.3 is that there
are none (section 3).


| # | Question | Recommendation | If there is no answer |
|---|---|---|---|
| 1 | Until phase F the search sees no trees, ice or lakes. A world made before F keeps the spawn it got: the same column as the official server's in nearly every world, at another height where a tree stands on it (the player then stands on the ground, not the canopy), and rarely another column. Accept that, or wait with T8 until F? | Accept | Accept |
| 2 | Vanilla spreads first-time players over 10 blocks around the spawn, at random, which needs the four chunks around the spawn's corner held and loaded by the home region. Spread them (repeatably, by name), or keep them on the spawn column? | Keep them on the column: nobody notices ten blocks, and a spread costs chunks and region work | On the column |
| 3 | Seeded worlds made before T8 have no spawn recorded. Refuse them with a sentence (worlds are not carried before the first release), or open them with the spawn at 0, 0? | Refuse: keeping 0, 0 keeps the sinking spawn T8 is for | Refuse |
| 4 | With a seed the spawn moves far from 0, 0, and `--pin 4` then puts its boundary far from where players enter. Make pins relative to the spawn's chunk, or keep them absolute? | Keep them absolute: a pin names a place in the world; the trials say where to go | Absolute |

## 7. The review, and what changed

1. The home chunk was not mentioned; moving the spawn moves it, and a mismatch makes
   the world over. Now the store derives it (T8c).
2. Writing `spawn` into `meta` after it exists was not crash-safe; now `meta` is
   written once with it.
3. The spawn already travels from the coordinator, which builds a `Land`, and edges
   build one only for login. Now the store tells regions; `Land` is split.
4. The column rule was misdescribed (it scans from above the top, a fluid refuses,
   columns x outer, z inner); a tree changes y, not x, z. Section 2, T8b's judges and
   question 1 are corrected.
5. The fitness formula was backwards (the climate term is scaled by 2048²) and named
   a depth term; corrected.
6. The ring details were missing and `libm` is not correctly rounded; the offsets are
   now emitted from the jar.
7. The spiral, the fallback and the angle were missing; given in section 2.
8. The first join reads four chunks and uses the world's default mode; question 2
   says what spreading costs.
9. "Kept like any other" contradicted ADR-0021 S6; T8f makes the area at every start.
10. ADR-0021 gives T8 a tick-time measurement; kept.
11. `level.dat` needs gunzip and a named root, and the spawn is `Data.spawn.pos`; a
    second judge over the official chunks is added.
12. Tests and figures assume 0, 0; T8g and question 4.
13. The client was never told the spawn; T8e.
14. Decisions that are the owner's are questions 3 and 4.
15. Missing tests added to section 5; `getOrigin` also places structures of the
    origin kind (none in 26.3), so its exactness matters beyond the spawn.
