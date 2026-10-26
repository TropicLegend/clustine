# N8: light in the simulation

Draft of 2026-10-26, for the owner and an independent reviewer before anything is built.
The game's rules are those of `n8-brief.md` (cited as L8: L8 0.4 is finding 4 of its
section 0, L8 5.8 its section 5.8), read from the 26.3 jar. The classes L8 section 6 found
missing are dumped now in `/home/claude/reference/n8-javap/` (50 classes, `javap -p -c
-constants`, from the jar `cargo datagen` keeps; nothing downloaded), cited as n8 with the
class's short name and the lines of its dump ("n8 SaplingBlock 39–58"); the earlier dumps
are cited as L8 cites them ("n7 CropBlock 100–140"). A23 to A28 are ADR-0023 to ADR-0028;
NP is `n-plan.md`; SteelMC is `/home/claude/reference/SteelMC-26.3`, a port and not the jar.
Section 2 says where the dumps correct L8.

Where it sits: the roadmap's next item after N7 ("then light in the simulation (N8,
brought forward)"), NP section 4 and its question 4; A24 section 8 left everything that
reads light out of phase B for it. N9 (natural spawning, monsters' targeting) reads what
N8 makes.

Where things stand (`main` at `bcf253a`): N7 is built but for its comparisons and
measurements (N7n), which are under way. The simulation reads light only through
`MobWorld::light` (`crates/clustine-sim/src/region/mobs.rs` 166–192: the chunk's carried
light, or a fallback of sky over the motion-blocking heights and no block light at all),
every rule's view answers no light (`BlockView::raw_brightness`, `None` in
`region/updates.rs` 1101, `placement/context.rs` 66, `rails.rs` 52, `physics.rs` 77), and
the random ticks that need light do nothing (`region/random_ticks.rs` 13–14).
`clustine-sim` does not depend on `clustine-light` or `clustine-features`.

What N8 does not change: the edge and what clients are sent (A23 stands), the chunk
format, the log and what a commit carries, generation and its version. **No world is
refused.**

## 0. What is decided, and why

The lead's answers to L8's questions (L8 section 5), as decided, with the reasons, and
where reading the dumps refines them.

1. **Light is computed in the region, at one fixed point of its tick**: after the
   copies and loads are taken (`update_chunks`), the players' actions, the seams and
   effects (`run_seams`) and the arrivals (`take_arrivals`), and before the block ticks
   (`region.rs` 948–1058; section 4.1). Every rule of the tick then reads light that is
   current with the blocks as the tick began, which is what the game nearly always gives
   a rule in the tick after a change (L8 2.3; the dumps confirm the race, section 2
   rows 1 and 2), and a player's action is read in the tick it is applied. A tick stays
   a function of the region and its inputs: light is a function of blocks, taken at a
   point the tick fixes. *Refinement*: what the tick changes after the step (block and
   fluid ticks, post-processing, random ticks, entities) is lit at the next tick's step,
   so a chunk post-processed in its first ticking tick has that tick's random ticks read
   the light of its unsettled blocks. Named; no rule of section 6 can tell within one tick.
2. **An incremental engine, ported from the game's** (`LightEngine`, `BlockLightEngine`,
   `SkyLightEngine`, `ChunkSkyLightSources`: n8, dumped now) into `clustine-light`, with
   `clustine_light::light` as its oracle. Ported from the game's rather than adapted from
   SteelMC's because SteelMC's (`steel-core/src/chunk/light/`, 9,268 lines) is a port of
   a concurrent design: cache windows of 5×5 chunks admitted under locks
   (`workset.rs`, `work_gate.rs` with `parking_lot` and `tokio`), queues leased from
   thread-local pools (`queue.rs` 501–509). Most of it is what a deterministic,
   single-threaded region would strip, and what remains is the game's propagation, which
   the dumps now give directly (the game's `LightEngine` is 561 lines of `javap`,
   `BlockLightEngine` 356). The rules of light themselves (what a block gives and takes,
   faces, the sky's column) are already adapted from SteelMC in `clustine-light`
   (`flood.rs` 1–13, `NOTICE`); the engine shares that code, so the engine and the oracle
   cannot disagree about a block. No threads in `clustine-sim`; queues are plain vectors
   in a fixed order; an engine over its budget is measured and mended in N8, not
   deferred (section 8).
   *Added*: the lead's first commit puts a whole-chunk implementation (L8's E1) behind
   the engine's interface, so that the rules and the region can be built and tested
   against exact light at once, and the incremental engine replaces it behind the same
   interface with the same tests (section 3.9).
3. **Derived, not committed.** Light is no part of a commit, of the log or of a saved
   chunk; a region lights what it has. It reuses a chunk's carried light where a test
   stricter than A23's T says it is what the engine would compute (section 3.5:
   T as the edge has it can pass at the rim of what a region has, where the region's
   light must differ), and computes the rest. The pause this adds to a load, a merge, a
   split and a take-over is measured on the real disk (section 8).
4. **Nothing about light crosses a line.** Each region lights its own chunks, its ring
   and its copies from the blocks it has. For a copy or ring chunk whose neighbours it
   does not have, L8 5.8's **option a**: it is lit with what is there, an absent chunk
   being a wall, as the crate and the game's engine both take a chunk that is not there
   (n8 LightEngine 159–183 answers bedrock). The reason, and the bound on the difference
   (it lowers only levels of 8 or less where mobs read, and changes no threshold any rule
   or mob of N8 reads), are in section 5. Options b (the fallback) and c, d (more copies,
   or light on the stream) are not taken.
5. **The edge is unchanged.** A23 keeps lighting what clients are sent. Whether regions
   later feed the edge (L8 5.9's T2, T3) is a question for after N8; a test holds the two
   to the same values where they have the same blocks (section 10).
6. **Scope: every rule of L8 section 1 that reads light**, and every mob's read moved
   onto the region's light. Natural spawning stays N9's. **Saplings are a step of their
   own** (N8f, section 7), last: they need the simulation to place features, which it has
   never done, across a line as effects, and a dependency on `clustine-features`; none of
   that is light, and none of the other rules waits for it. Bone meal is not in the
   lead's list; this plan recommends it with the trees (question 1).

## 1. What a player sees when N8 is done

### 1.1 Crops and their kin

- **Wheat, carrots, potatoes** grow where the light at the crop (sky or block, the sky
  not darkened by night) is 9 or more, at the game's chance by the farmland around (n7
  CropBlock 100–140, 174–326): under the open sky they grow day and night; in a cellar
  only by a torch. **Beetroots and the torchflower crop** the same after a one-in-three
  that skips (n7 BeetrootBlock 31–45, TorchflowerCropBlock 68–81); a torchflower crop
  grown out becomes a torchflower. **Melon and pumpkin stems** grow by the same light and
  at age 7 set a fruit beside them on a block that allows it (n7 StemBlock 71–184).
  **The pitcher crop** draws its chance first and then grows with light 8 or more and
  room above, becoming two blocks tall from age 3 (n8 PitcherCropBlock 219–375).
  **Sweet berries** age by light 9 or more above the bush, one try in five (n7
  SweetBerryBushBlock 72–110).
- **Crops need light to stay**: seeds (wheat, carrots, potatoes, beetroot, torchflower)
  and a pitcher pod cannot be planted where the light is below 8, and a crop in light
  below 8 breaks at its next shape update (n7 CropBlock 327–356; n8 PitcherCropBlock
  127–144, the lower half). Light changing is no update (section 2, row 3): a crop whose
  room went dark stays until something beside it changes. In a world played before N8, a
  farm in a dark room breaks as its neighbours change: the game's behaviour.

### 1.2 Melting

- **Snow layers** melt where block light is 12 or more (n7 SnowLayerBlock 178–200): a
  torch melts the snow within one block of it, glowstone or a lantern within two or three.
- **Ice** melts to water where block light is above 11 less its own dampening (n8
  IceBlock 63–115), or vanishes where water evaporates (no such place in the overworld).
  Packed and blue ice are other blocks and never melt. The sun melts nothing: both read
  block light alone.

### 1.3 Grass and mycelium

- A grass block or mycelium **dies to dirt** where the block above stops light through
  their shared face or holds a full fluid, unless it is one layer of snow; it **spreads**
  to dirt one block across and up to three down or one up, four tries a random tick,
  where the light above it is 9 or more *with* the night's darkening: by day, or by a
  torch at night (section 6, `SpreadingSnowyBlock`, still to be dumped: section 2,
  row 8). Paths of grass come back to a cleared field in a few minutes of daylight.

### 1.4 Bamboo

- **Bamboo and its shoot** grow one block, one try in three, where the block above is
  air and its light (not darkened) is 9 or more, up to 16 tall, the leaves of the top
  three changing as the game's (n8 BambooStalkBlock 227–270, 454–572; BambooSaplingBlock
  21–44, 126–149).

### 1.5 Mushrooms

- A mushroom stays on a block of the tag `overrides_mushroom_light_requirement` in any
  light, or on a block of solid render where the light is below 13 (n8 MushroomBlock
  144–175): one can no longer be planted on dirt in daylight, and one in the open is not
  judged until a neighbour changes. Mushrooms **spread** one try in 25, where at most
  four of their kind stand in the 9×3×9 around, by a walk of up to five steps to air
  where they could stay (n8 MushroomBlock 26–137): a mushroom farm fills a dark room.

### 1.6 Saplings and trees (N8f)

- Every sapling grows where the light above it, **darkened by the night**, is 9 or more,
  one try in seven: a first try sets its stage, a second grows the tree (n8 SaplingBlock
  39–89). So saplings grow by day or by a torch, not in the open at night. The tree is
  the game's for its wood, from the same features generation places: oak or (one in ten)
  fancy oak, with a bee nest at times beside flowers; spruce, and with four saplings in a
  square mega spruce or mega pine; birch; jungle, and the mega jungle tree from four;
  acacia; cherry; dark oak and pale oak only from four; mangrove from a propagule on the
  ground (tall mangrove four times in five); and the poplars, red, orange or yellow
  (n8 TreeGrower 101–219, 514–743). A sapling with no room stays a sapling.
- A tree beside a line grows across it: its blocks in the other region's land appear a
  tick later (coupled regions) or a few ticks later, where nothing has taken their place
  meanwhile (section 7.4).
- With bone meal (question 1): a sapling grows at once at a chance of 0.45, crops and
  stems age, grass sprouts grass and flowers around, a mushroom becomes a huge one at
  0.4, bamboo, berries, a pitcher and a propagule grow; a dispenser does the same.

### 1.7 The daylight detector

- Every second, a daylight detector under the sky gives the sky's light at it less the
  night's darkening, lowered by the sun's angle towards dawn and dusk; inverted, 15 less
  the light (n8 DaylightDetectorBlock 42–120, 185–216). Clicking it inverts it and
  updates its output at once (121–161). It drives wire, lamps and the rest of phase R's
  circuits; across a pinned line, as ADR-0027 lends it.

### 1.8 Mobs

- Every read of light a mob makes (L8 1.5) reads the region's light, which sees every
  torch, lantern, lava and glowstone, sees through glass and leaves (that let the sky's
  light through), and follows every change. So an undead mob under glass burns at noon
  (the game's), one in a lit hut's shade does not; monsters' strolls prefer the dark and
  animals' the light as they should around a player's torches; a skeleton fleeing the
  sun finds a hiding place under a real roof; a fox's shelter and a villager's
  sky-seeing spot are where the game's are.

### 1.9 Left out, and named

- **Frosted ice** (n8 FrostedIceBlock 42–176) is made only by Frost Walker, which
  Clustine has no enchantment for: not built.
- **Snow and ice forming, lightning, phantoms**: weather and N9.
- **Natural spawning and spawners**: N9, reading the light N8 makes.
- **What a client is shown** is the edge's light (A23), as now.
- **The rim** (section 5): in a copy or ring chunk, light from beyond what the region
  has is missing; only a stroll's ranking of dark candidates there can differ.
- **A tree's bee nest** grown across a line comes without its bees (section 7.4); a
  **stem's fruit** across a line may be refused after the stem has turned to it
  (section 6.2).

## 2. What the dumps change in L8

| # | L8 said | The dump says | So |
|---|---|---|---|
| 1 | 2.2 (**Inferred**): queues of decreases and increases | n8 LightEngine: two `LongArrayFIFOQueue`s of pairs (position, entry); an entry packs the level it came from (4 bits), the directions still to visit (6), "from an empty shape" and "increase from emission" (n8 LightEngine$QueueEntry); `checkBlock` only adds the position to a hash set (290–299); `runLightUpdates` runs `checkNode` over the set, then all decreases, then all increases (345–390). A missing chunk reads as bedrock (159–183) | as L8 inferred. The order of `checkNode` is a hash set's, which confirms L8 2.5: the order of work is not seen in the values |
| 2 | 2.3 (**Inferred**): the light thread runs between ticks | n8 ServerChunkCache$MainThreadExecutor 55–70: `pollTask` runs the distance manager, then `tryScheduleUpdate`, then a task. n8 ThreadedLevelLightEngine: every call goes to a `ChunkTaskDispatcher` by the chunk's level (180–210); its task appends to `lightTasks` and, **once 1,000 are queued, runs an update at once** (542–559); `runUpdate` takes at most 1,000 tasks, runs `runLightUpdates`, then the tasks that wait (313–379) | as inferred, and racier: a tick with more than a thousand changes light can see is partly lit while it runs. Section 0's fixed point stands |
| 3 | 1.0, 1.2 (**Inferred**): `getBrightness`, `canSeeSky`, and that light changing is no update | n8 BlockAndLightGetter: `getBrightness` is the layer's `getLightValue`, `canSeeSky` is sky ≥ 15 (5–36). The engine's only call outwards is `onLightUpdate`, for the clients (n8 LayerLightSectionStorage `swapSectionMap`) | confirmed |
| 4 | 1.0: a sky section without data reads the first section above with some | n8 SkyLightSectionStorage 29–110: 15 at or above the column's top section; else the first section upwards with data, read at its **bottom layer** (`BlockPos.getFlatIndex`), and 15 if none | refined; section 3.6 |
| 5 | 2.1: `hasDifferentLightProperties` not dumped | n8 LightEngine 68–95: dampening, emission, or either state using its shape for light; emptiness is apart (`updateSectionStatus`) | Clustine's `light_tells_apart` (`clustine-world/src/chunk.rs` 212) is a superset (air, faces, sky down): it asks for more checks, never fewer |
| 6 | 1.1 saplings (**Inferred**) | n8 SaplingBlock 39–89: `getMaxLocalRawBrightness(above) >= 9`, then `nextInt(7) == 0`, then `advanceTree`: stage 0 to 1 with flags 260, else `TreeGrower.growTree` | confirmed |
| 7 | 0.11: `advanceTree` places "the tree's feature" | n8 TreeGrower 101–219: the mega list is drawn first, and tried where a 2×2 of the same sapling is found at offsets (0,0), (0,−1), (−1,0), (−1,−1) (470–506); else the flower list where a flower stands within ±2, ±1 (384–402), else the plain list; the saplings are removed (to their fluid's block, flags 818) before the feature and put back with flags 260 if it fails (293–354). The growers (514–743) include **poplar** (red, orange, yellow, equal), new in 26.3; dark and pale oak have only mega lists | section 7.1 |
| 8 | 1.1, 6: grass in `SpreadingSnowyDirtBlock` | n8 GrassBlock and MyceliumBlock extend **`SpreadingSnowyBlock`** (26.3's name), given the dirt block's key; the dumped classes hold only bone meal (GrassBlock 41–105) and particles. **The random tick is still not dumped** | rule from SteelMC (`behavior/blocks/vegetation/spreading_grass_block.rs`, as L8 inferred) until `SpreadingSnowyBlock` and `SnowyBlock` are dumped, before D2 builds it |
| 9 | 1.1 ice (**Inferred**: `11 - getLightBlock()`) | n8 IceBlock 63–115: block light `> 11 - getLightDampening()`; `melt`: where water evaporates `removeBlock`, else `setBlockAndUpdate(water)`, `neighborChanged(pos, water)`, game event `block_destroy` | confirmed, the method named |
| 10 | 1.1: frosted ice among random ticks | n8 FrostedIceBlock 42–176: a **scheduled** tick (60–120 after placing, 20–40 after); `nextInt(3) == 0` or fewer than four frosted neighbours, then `getMaxLocalRawBrightness > 11 - age - dampening` (block light in the End) | not built (1.9) |
| 11 | 1.1 bamboo (**Inferred**) | n8 BambooStalkBlock 227–270: stage 0 (no draw), `nextInt(3) == 0`, air above, `getRawBrightness(above, 0) >= 9`, height below plus one under 16. BambooSaplingBlock 21–44: `nextInt(3)`, air above, light ≥ 9, bamboo with small leaves above | confirmed |
| 12 | 1.1 mushrooms (**Inferred**): "a few tries at a spot one away" | n8 MushroomBlock 26–137: `nextInt(25) == 0`; at most four of the kind in −4..4, −1..1, −4..4 (`atMostMatched(4)`, the mushroom counted); a target at (`nextInt(3)−1`, `nextInt(2)−nextInt(2)`, `nextInt(3)−1`); four times: step onto the target if it is air and the mushroom survives there, draw the next target from where it stands; then set the last target with flags 2 if it is air and survivable | **corrected**: a walk of up to five steps, so a spread lands up to five blocks across, possibly in a copy |
| 13 | 1.1 pitcher (**Inferred**: `hasSufficientLight` in `canGrow`) | n8 PitcherCropBlock 219–375: the draw `nextInt((int)(25/f)+1) == 0` **first**, then `canGrow`: not grown out, `CropBlock.hasSufficientLight` (≥ 8), inside the height, the block above air or pitcher crop from age 3; survival of the lower half asks the same light (127–144) | confirmed, with the draw first and the threshold 8 |
| 14 | 1.3 detector (**SteelMC**): the sun's angle in radians | n8 DaylightDetectorBlock 42–120: `getEffectiveSkyBrightness`, `visual/sun_angle` **in degrees** times 0.017453292; inverted, `15 − i`; else for `i > 0`, `f += ((f < π ? 0 : 2π) − f) * 0.2`, `i = round(i * cos f)`; clamped 0..15; set by `setBlockAndUpdate` if it differs. The ticker exists only where the dimension has sky light; runs when `gameTime % 20 == 0` (185–216). Use: invert with flags 2, game event `block_change`, update at once (121–161) | corrected (degrees); section 6.4 |
| 15 | 1.1: the propagule reads no light; azalea grows by bone meal alone | n8 MangrovePropaguleBlock 216–245: not hanging, `nextInt(7) == 0`, `advanceTree`; hanging, its age (built). n8 AzaleaBlock: no random tick; bone meal at 0.45 | confirmed |
| 16 | 1.1 "read no light": cocoa, kelp, nether wart, sugar cane | no brightness or light layer anywhere in their dumps | confirmed |
| 17 | 6: `DynamicGraphMinFixedPoint`, `LeveledPriorityQueue`, `SpatialLongSet` asked for with the engine | in the lighting package, but no engine class refers to them | not needed |

**Still to be dumped** (by the lead, before the part that needs them): `SpreadingSnowyBlock`
and `SnowyBlock` (D2's grass); `TreeFeature` and `Feature.setBlock` (the flags of a
tree's sets, taken here as 19 from the features crate's port, **Inferred**), and
`WorldGenLevel.ensureCanWrite` (taken as true on a `ServerLevel`, which does not override
it: n5 ServerLevel) for N8f; `util.random.WeightedList` (whether a list of one draws) for
N8f; the timeline's cubic Bézier ease (`EasingType` and the keyframe sampler) for the
detector's sun angle (D3); `BoneMealItem`'s and `BonemealableBlock`'s callers are in n7.

## 3. The engine

### 3.1 What it is a function of

**The invariant**: after each run, the light the engine holds for every chunk the region
has (loaded, ring, lent or copied: its *present* chunks) is `clustine_light::light` of that
chunk and its eight neighbours as present, an absent neighbour being a wall. Light
reaching a chunk starts at most 14 steps from it, so every way it travels stays in the
3×3 (`clustine-light/src/lib.rs` 27–28); the greatest fixed point over all present chunks
is therefore, chunk by chunk, the crate's function of each 3×3. That is what the
property tests check, exactly, after any sequence of changes, arrivals and departures.
The order in which the engine works is not seen in the values (section 2, row 1).

### 3.2 The interface (the lead's, fixed first)

In `crates/clustine-light/src/engine.rs`, public, beside `light` and `reach`:

```rust
/// What the engine reads of the world: a present chunk, or `None` for a wall.
pub trait Chunks {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk>;
}

/// The two levels at a block, as stored: the sky's undarkened, and the blocks'.
pub struct LightAt { pub sky: u8, pub block: u8 }

/// What a run did, for the figures (section 8).
pub struct Work { pub chunks_lit_whole: u32, pub carried: u32, pub cells: u64 }

pub struct Lighting { /* the levels of present chunks, and what waits for a run */ }

impl Lighting {
    pub fn new(sky: bool) -> Self;
    /// A chunk became present, or its blocks were replaced whole (a copy's new base).
    pub fn arrived(&mut self, at: ChunkPos);
    /// A chunk stopped being present.
    pub fn left(&mut self, at: ChunkPos);
    /// A block changed in a way `light_tells_apart` sees.
    pub fn changed(&mut self, at: BlockPos);
    /// Brings every present chunk to the invariant of 3.1, reading `chunks` as they are.
    pub fn run(&mut self, chunks: &dyn Chunks) -> Work;
    /// The levels at a block as of the last run; `None` where no present chunk is.
    pub fn at(&self, at: BlockPos) -> Option<LightAt>;
    /// The chunk's light in the crate's form (sections listed by its rule), for the
    /// tests and for whoever later wants it whole (N9, the edge after N8).
    pub fn light(&self, at: ChunkPos, chunks: &dyn Chunks) -> Option<Light>;
    /// Handed on at a merge or a split with the warm chunks (section 4.5).
    pub fn keep_only(&mut self, present: &dyn Fn(ChunkPos) -> bool);
}
```

`arrived`, `left` and `changed` only note; nothing is lit before `run`. A position
noted in a chunk that is not present is passed over. `Lighting` holds a `BTreeMap` of
chunks and vectors; no hash map, no clock, no thread (the crate's `clippy.toml` is the
simulation's).

### 3.3 What it keeps

For each present chunk, for the sky and for blocks, one entry a section and one below
and one above, as `Light` has (`clustine-world/src/light.rs` 91–127), each in its one
form: dark, full or 2,048 bytes of levels. Unlike `Light` it keeps the levels of **every**
section, also those the crate lists as absent, because light spreads through them
(section 3.6). Beside them, for each column of each present chunk, the lowest block the
sky reaches at 15 (the game's `ChunkSkyLightSources`, n8, 481 lines), so that a check
knows whether a column's source moved.

### 3.4 How it works

A port of the game's propagation (n8 LightEngine, BlockLightEngine, SkyLightEngine and
ChunkSkyLightSources), cited file by file as the crate's other adaptations are, and
licensed as Clustine is:

- `changed` positions are kept in a sorted, deduplicated vector (the game's hash set,
  section 2 row 1). `run` takes the departures first, then the arrivals, then the
  checks, each in ascending order, then propagates all decreases and then all increases
  from two FIFO vectors, with the game's packed entries and directions.
- **A check** is the game's `checkNode` of each layer: the stored level is decreased in
  all directions, or the light is pulled in from the neighbours where the new state
  gives no less (`PULL_LIGHT_IN_ENTRY`); a block that gives light enqueues its increase;
  for the sky, the column's lowest source is updated and the sources below or above
  removed or added (`updateSourcesInColumn`, `removeSourcesBelow`, `addSourcesAbove`).
- **What is not ported**: the section storages' gating (`storingLightForSection`,
  `lightOnInSection`, `propagateFromEmptySections`, `countEmptySectionsBelowIfAtBorder`),
  the visible and updating maps and their swap, and the queued data of loading chunks.
  The crate floods every section alike and leaves the empty ones out afterwards; the
  engine does the same, so that it has one oracle. The game and the crate can differ
  only where the crate already says they may (`lib.rs` 37–45: sky light far under
  something that floats, with no block near), which no chunk of the sample showed
  (L8 2.5).
- **An arrival** is lit whole from nothing in the same pass: every block of the chunk
  that gives light enqueues its increase, every column's sky sources are added, and the
  cells of present neighbours along its faces pull light in. A wall that becomes blocks
  can only add ways for light, so an arrival makes no decreases. Arrivals of one run are
  lit together, which floods each cell once instead of nine times (the crate's 48×48
  grid lights a 16×16 chunk): the cost of a view's load (section 8).
- **A departure** is the reverse: blocks that become a wall can only take ways away. Each
  cell of a present neighbour along the departed chunk's faces is decreased in all
  directions from its level, which zeroes what could have come through it and re-enqueues
  as increases the neighbours lit otherwise, as the game does for a source removed.
- The rules of a cell (dampening, emission, faces, the sky's straight column) are
  `flood.rs`'s `Cell` and `faces.rs`, shared, not copied.

### 3.5 Carried light

A chunk may arrive carrying light (`Chunk::light`): the generation pool lit it from the
nine chunks as generated (`services/worldgen/src/scheduler/pool.rs` 341), and it is kept
until the first change light can see (`chunk.rs` 364). A23's test T lets an edge send it
while every neighbour **it has** is as generated by the version the light was made
beside; a neighbour it does not have passes. In a region that last clause breaks the
invariant: at the rim of what the region has, the region's light must be the light with a
wall, and carried light is the light beside a neighbour. So the region adopts carried
light for an arriving chunk only where:

1. `made_by` is this crate's `VERSION` (not `IMPORTED`, which waits for W4) and `beside`
   is a generator version *g* other than 0;
2. all eight neighbours are present and as generated by *g* (`Chunk::generated`); and
3. every section the light lists as absent lies above every section of the chunk that
   holds a block. Then its sky is 15 and its blocks' light 0 in both the crate's and the
   game's reading, so the engine fills the levels it keeps for absent sections exactly.

Else it is lit (3.4). The property tests include chunks adopted so. How much this saves
is measured (section 8): in the flat world every chunk passes; in a seeded world after B6
most chunks carry a mark or a tick and lose their light at post-processing (A23 section
9: 72 in 100), so little passes. If the measurement shows it saves nothing in seeded
worlds and little in flat ones, the step that measures it says so and it may be dropped
(question 3).

### 3.6 Reads, and sections without light

`Lighting::at` answers the stored levels wherever the chunk is present, also in a
section the crate lists as absent. There the game reads otherwise (section 2, row 4: a
sky section without data reads the bottom layer of the first section above that has
some, 15 at the top). The two agree wherever no block is within a section of the place,
which is what "absent" means, except under something floating with no block near, the
crate's named difference (`lib.rs` 37–45). Block light in an absent section is 0 in both:
an emitter is at least 17 blocks from any of its cells.

### 3.7 Determinism

`run` is a function of the engine's levels and of the chunks it reads, and by the
invariant its result is a function of the present chunks' blocks alone: two regions that
reach the same present chunks by different histories (incremental here, whole after a
restore there) hold the same light. Queues are vectors in insertion order, the checks
sorted, the chunks a `BTreeMap`.

### 3.8 Memory

About the 9.3 KB of carried light a chunk at the mean for real terrain (A23 section 1),
more for the absent sections the engine keeps (mostly full sky, one byte of form each),
and 1 KB of column sources. A region at a view distance of 8 has some 400 to 650 present
chunks with its ring and copies: about 4 to 8 MB (**guess**; measured, section 8). The
queues grow with the largest run and are kept between runs.

### 3.9 The first implementation, and the oracle

The lead's first commit implements `Lighting` by L8's E1: `run` lights whole, with
`clustine_light::light_with` (asking the levels of absent sections too, `Unlisted` all
set), every present chunk within `reach` of a change (with all eight where a section's
emptiness changed), every arrival and the present neighbours of every arrival and
departure. It is exact by construction and slow (13 ms a change at the mean, L8 5.4).
The rules (D2, D3), the region's step (L2) and every test are built on it. The
incremental engine (D1) then replaces the bodies; the E1 code stays in the tests as the
oracle, and its property tests (section 10, part A) are written against the interface,
not the implementation, so they pass on both.

## 4. Light in the region

### 4.1 The light step in the tick

`Region::tick` (`region.rs` 923–), with the step marked:

1. the clock, `update_chunks` (loads, lets go, copies' bases, updates and withdrawals);
2. edge events, players joining, leaving and arriving, the entering stays, faces, the
   players' actions (`apply_remote`, `apply_input`);
3. the sleep check, `update_ticking`, `run_seams` (seam entries and effects from other
   regions), `lay_blast_drops`, `take_arrivals`;
4. **the light step**: `self.light.run(&present)`, the darkening of the tick taken once
   from the clock (`time::sky_darken`);
5. block ticks, fluid ticks, post-processing, random ticks, block events, entities, block
   entities and the rest, as now (`region.rs` 1058–).

Everything that reads light in a tick reads the step's light: random ticks, block ticks
(none of section 6 is one), placing and using in later ticks, survival in shape updates,
mobs, the detector. What changes after the step is noted and lit at the next step.

### 4.2 What tells the engine

- **Every write to a loaded chunk** goes through the region's set
  (`region/updates.rs` 150–200, which also makes the edge's `BlockChanged`); there the
  region calls `light.changed(pos)` when `light_tells_apart(old, new)`. The lead audits
  every other `get_mut` of `self.chunks` (34 sites: post-processing, pistons, falling
  blocks, explosions, the tests' builders) and routes each through one function
  `Region::block_written(pos, old, new)`. The guard against a missed site is 4.7.
- **Copies**: each copy update (`region.rs` 3092–3103) reads the old state before
  `apply` and calls `changed` likewise; a new base is `arrived` (replacing whole); a
  withdrawal is `left`.
- **Loads and lets go**: a chunk inserted into `chunks` is `arrived`, one removed is
  `left` (unless a copy of it remains or arrives in the same tick: then `arrived`,
  since its blocks may be another sequence's); a copy dropped because its chunk is now
  loaded is an `arrived` of the loaded chunk.

### 4.3 Reads

One set of functions on the region, the game's names, in `region/light.rs`:

| Function | The game's | Read by |
|---|---|---|
| `raw_brightness(pos, darken)` | `getRawBrightness`: `max(sky − darken, block)` | crops, stems, berries, bamboo, survival (darken 0) |
| `max_local_raw_brightness(pos)` | `getMaxLocalRawBrightness`: the above with the tick's darkening; 15 outside ±30,000,000 (n7 LevelReader 297–330) | saplings, grass, the mobs' magic value |
| `block_brightness(pos)` | `getBrightness(BLOCK, pos)` | snow, ice |
| `effective_sky_brightness(pos)` | `getEffectiveSkyBrightness` (n7 LevelReader 332–341) | the detector |
| `can_see_sky(pos)` | sky ≥ 15 (n8 BlockAndLightGetter 24–36) | mobs, navigation |

Each answers `None` where the chunk is not present. Through them:
`RegionView::raw_brightness` (`updates.rs` 1101) and the fluids' view answer, so every
`BlockView` rule judges light (the mushroom's survival in `clustine-blocks/src/survive.rs`
186–193 among them); the placement view (`placement/context.rs` 66) answers, so seeds
are refused in the dark; `MobWorld::light` (`region/mobs.rs` 166–192) reads
`Lighting::at` and its fallback goes. The views that answer `None` still (the rails',
physics') read no light-dependent rule; each says so where it answers.

### 4.4 The darkening

`skyDarken` is set once a tick from the clock before the chunks tick (L8 2.4); Clustine
has it from the region's clock (`time.rs` 43–50). Nothing is relit at dusk: the stored
sky light is the sky's full light and the darkening is taken at each read.

### 4.5 Loads, merges, splits, restores, moves and take-overs

- **A load** (a join, a flight, chunks claimed): the chunks arriving in a tick are lit
  together at its step (3.4). A first view arrives over several ticks; each tick lights
  what arrived. Measured (section 8).
- **A restore** (a crash, a move, a take-over, a restart of the store): the restored
  region has no light; its first step lights every present chunk as arrivals. The pause
  this adds is that step's length, measured on the real disk with the rest of the
  restore (section 8).
- **A merge or a split** begins anew through `Region::restore` with warm chunks
  (A24 section 4; A25 section 3). The light is handed on with them: the absorber keeps
  its `Lighting` (`keep_only` what stays present), with its noted changes, and the
  absorbed region's chunks arrive; a split's part takes the parent's `Lighting` cut to
  its own present chunks, and what it no longer has leaves. A warm `Lighting` is valid
  for its region's present chunks with the changes noted since its last step, so what it
  becomes after the next step is the invariant's, as a fresh one's would be. **This is
  ordering-sensitive and the lead's** (the noted changes of the last tick must go with
  it, or a change made after the last step is lost).

### 4.6 Not state, not committed

The light is no part of `RegionState`, the commit, the log or a saved chunk. Saving a
chunk is unchanged: `Chunk::light` keeps its meaning (carried light, dropped at a change)
and the region never writes its own light into a chunk. The `Lighting` field is left out
of the region's equality (a wrapper whose `PartialEq` holds always), since between ticks
a running region's light is its last step's and a restored one's is none or whole; the
tests judge it by the oracle instead (4.7, question 2). Nothing is raised: the format,
`FEATURES_VERSION` and `clustine_light::VERSION` stay; the engine changes no value the
crate gives.

### 4.7 What the region checks of itself in tests

Under `cfg(test)` and in the end-to-end tests' builds (a feature `light-oracle`), after
every tick's step the region compares the light of each chunk the step touched (lit
whole, or reached by a change) with `clustine_light::light` of its present 3×3 and
panics on a difference. Every existing test of the simulation (thousands of ticks of
water, pistons, TNT, mobs, merges and splits) thereby tests the engine and the hooks of
4.2: a write that bypasses `block_written` shows as a difference at the next step.

## 5. What crosses a line

**Nothing about light.** Each region lights its present chunks from its blocks and its
copies' (decision 4). A chunk it ticks has its 3×3 present (A24 section 2, A25 section 3),
so its light is exact: the oracle's for its 3×3, which is the light a single region
would have with the same blocks. Two coupled regions see each other's changes in their
copies at the start of the next tick and light them at its step: the game's timing. An
uncoupled copy is one to a few ticks late (A25 section 1), and light at the line with it.

**Where the 3×3 is not there** (a copy, a ring chunk), option a. The bound, from 3.1:
every chunk in the 3×3 of a chunk the region ticks is present, so an absent chunk begins
at least 17 blocks from that chunk along x or z. A read within *d* blocks of it (along
both) misses only light whose source is at least 17 − *d* steps away, which arrives with
at most *d* − 2, and is exact wherever the true level is *d* − 1 or more. Mobs tick only
in chunks that tick (A28 section 2) and read up to 10 blocks away (a stroll,
`FleeSunGoal`): there a missing source gives at most 8, so

| Read in a copy or ring chunk | Within | Exact? |
|---|---|---|
| `canSeeSky` (sky ≥ 15) | anywhere | yes: 15 comes only from the column's own sky |
| `FleeSunGoal`'s hiding place (walk target value below 0, which needs raw light of 12 or 13, by the float) | 10 | yes |
| a mushroom's survival where its spread lands (< 13) | 5 | yes (a missing source gives at most 3) |
| a grass block's spread (no light read at the target) | 1 | — |
| a stroll's ranking of candidates by light | 10 | levels of 8 or less may read lower: a monster may judge one dark corner darker than another |
| a path's trim from the sun (`canSeeSky`) | the path | yes |

So no threshold any rule or mob of N8 reads differs; only the ranking of dark stroll
candidates near a line or the rim of the view can. Named. Option c (copies two chunks
across) would make even that exact, at a change of A25 and many more copy updates on the
stream; option d (the holder's light on the stream) would be a tick late and a new kind
of message. Neither is worth the ranking.

**What crosses as blocks** (A25 section 4, conditional sets, as now): a grass block
spreading into a copy (target one across, up to three down), a mushroom's spread (up to
five across), a stem's fruit (one across), a tree's blocks (section 7.4). Each is a
`SetIf` with the state the rule read there as `expected`, done by the holder by its
rules a tick (coupled) or a few later; a refused one does nothing. The rule's own
decision (light, chance) is the region's that ticks the plant.

## 6. The rules

### 6.1 Where they live

The random ticks of section 1 go into `region/random_ticks/` as a fourth file,
`growth.rs` (crops, stems, berries, pitcher, bamboo, mushrooms, grass and mycelium,
saplings and the propagule's ground half from N8f), and melting into `minerals.rs`'s
neighbour `melting.rs`; `random_tick`'s dispatch (`random_ticks.rs` 257–266) tries them
after plants. Each rule is the game's `randomTick` with the same draws in the same order
from the section's stream (B7: the game's chances, not its values), and sets with the
game's flags. Survival goes into `clustine-blocks/src/survive.rs` (shared with
generation and placing): `CropBlock` and the pitcher's lower half gain the light test,
asked of the view, `None` judging as now (generation answers 15 and passes).

### 6.2 The random ticks

| Rule | Order of reads and draws | What it sets, and flags | Across a line |
|---|---|---|---|
| wheat, carrots, potatoes (`CropBlock`) | `raw(pos, 0) ≥ 9`; age below max; growth speed of the farmland around; `nextInt((int)(25/f)+1) == 0` | age + 1, flags 2 | — |
| beetroots, torchflower crop | `nextInt(3) != 0`, then the crop's | the crop's; the torchflower crop at its last age becomes the torchflower (n7 TorchflowerCropBlock) | — |
| pitcher crop | growth speed; `nextInt((int)(25/f)+1) == 0`; then not grown out, `raw(pos, 0) ≥ 8`, inside the height, above air or pitcher from age 3 | lower age + 1 flags 2; from age 3 the upper half, `setBlockAndUpdate` | — |
| melon, pumpkin stems | `raw(pos, 0) ≥ 9`; `nextInt((int)(25/f)+1) == 0`; below age 7 age + 1; else a horizontal direction drawn, the fruit where air and the block below allows | age flags 2; fruit `setBlockAndUpdate`, then the attached stem | the fruit one across may be a `SetIf`; the stem turns at once, and stays turned if the holder refuses: named |
| sweet berry bush | age below 3; `nextInt(5) == 0`; `raw(above, 0) ≥ 9` | age + 1 flags 2, game event `block_change` | — |
| snow layer | `block(pos) > 11` | drops (none without a tool), `removeBlock`, game event `block_destroy` | — |
| ice | `block(pos) > 11 − dampening` | `melt` (section 2, row 9) | — |
| bamboo | stage 0; `nextInt(3) == 0`; air above; `raw(above, 0) ≥ 9`; height below + 1 < 16 | `growBamboo` (its draws and leaves, n8 454–572) | — (one column) |
| bamboo shoot | `nextInt(3) == 0`; air above; `raw(above, 0) ≥ 9` | bamboo with small leaves above, `setBlockAndUpdate` | — |
| mushrooms | `nextInt(25) == 0`; at most four of the kind around; the walk of section 2 row 12, reading survival (light < 13) at each target | the target, flags 2 | the target may be a copy's: `SetIf` (expected air) |
| grass block, mycelium (**SteelMC**, to be checked against `SpreadingSnowyBlock`) | if it cannot stay (snow of one layer above stays; a full fluid above, or the face above stopping light, kills): dirt, flags 3, no draw. Else if `max_local(above) ≥ 9`: four times a target (`nextInt(3)−1`, `nextInt(5)−3`, `nextInt(3)−1`), dirt there that could stay grass and has no water above | the grass or mycelium, snowy from the block above, flags 3 | the target may be a copy's: `SetIf` (expected dirt) |
| saplings, propagule on the ground | section 7 | | section 7.4 |

### 6.3 Survival

`can_survive` asks light only where the game does: `CropBlock` (and its kin by class:
beetroots, carrots, potatoes, torchflower crop) `raw(pos, 0) ≥ 8` and on
`supports_crops`; the pitcher's lower half the same; the mushroom as now
(`survive.rs` 186–193), now judged in a region. It is asked at placing and at each
shape update, never at a change of light (section 2, row 3).

### 6.4 The daylight detector (D3)

A block entity ticker in `run_block_entities` (ADR-0026 section 3): where the dimension
has sky light, when the clock's game time is a multiple of 20, `updateSignalStrength`
(section 2, row 14) with the sun's angle from `visual/sun_angle` (`timeline/day.json`:
a cubic Bézier ease of (0.362, 0.241, 0.638, 0.759) between 360° and 0° at tick 6,000,
added to `time.rs` beside `sky_light_level`, the ease to be dumped first), in `f32` as the
game computes it, with the game's `Mth.cos`. Set by `setBlockAndUpdate`, so
phase R's updates carry it. Its use (a click) inverts it with flags 2, makes the game
event and updates at once. ADR-0027 already lends it as a circuit's part
(`region/tests/lends_by_the_record.rs` 221–241).

### 6.5 Mobs

Nothing in the mobs' code changes but `MobWorld::light` (4.3): every reader of L8 1.5
(`burning.rs`, `monster_ai.rs`, `animal.rs`, `guardians.rs`, `random_pos.rs`,
`skeletons.rs`, `navigation.rs` 511–512, `villagers.rs` 33–39, `fox.rs`) goes through it.
`None` stays "true" for `can_see_sky` and 0 for the magic value where no chunk is
present, as the game's `Entity` reads an unloaded chunk (n5 Entity 4832–4852).

## 7. Saplings and trees (N8f)

### 7.1 `TreeGrower`

Ported whole from n8 TreeGrower: the twelve growers with their three weighted lists and
shortest tree (514–743); `growTree` (101–219) with the mega list drawn first, the 2×2
search, the flower test, the saplings removed (to the legacy block of their fluid, flags
818) before the feature and put back (flags 260) if it fails; `canGrow` and
`getMinimumHeight` for bone meal. `SaplingBlock.advanceTree` (stage 0 to 1, flags 260)
and the propagule's ground half call it. The features are the configured features
`clustine-features` reads already (`clustine-worldgen-data`: every name the growers use,
the poplars and `pale_oak_bonemeal` among them, is there).

### 7.2 A region's view for a feature

`clustine-features` places through `Level` over `&mut dyn Neighbourhood`
(`clustine-world/src/generation.rs` 94–). N8f gives the region one,
`region/feature_view.rs`, over its present chunks:

- **Reads** of blocks, block entities, heights and biomes come from loaded chunks and
  copies, and from the view's own writes kept below (so a feature reads what it wrote,
  as it does in the game).
- **Writes into the region's land** go straight through the region's set with the flags
  the feature uses (19 for a tree's logs, leaves and decorations: section 2, still to be
  dumped), with their updates, in the order made, as the game's `ServerLevel` does.
- **Writes elsewhere** (a copy, or a chunk the region holds but has not loaded) are kept
  in the view, read back by the feature, and become `SetIf` effects in the order made,
  `expected` being what the view read there first, when the feature returns.
- `ensure_can_write` is true (a `ServerLevel` writes anywhere, section 2); a position
  neither present nor held refuses the write, as the game's region of generation does at
  its edge: a tree is at most a few blocks from its sapling, so in ordinary play this
  never happens.
- Scheduled ticks and post-processing marks: a tick goes through the region's
  `schedule_tick` (an effect across a line); a mark is passed over (a `ServerLevel` has
  no post-processing, **Inferred**).
- Light, while a feature is placed by a region, is what features read in generation (sky
  15, blocks 0, `clustine-features/src/context.rs` 375–390): no tree, mushroom or bone
  meal feature reads light (only the freezing in `features/ground/shared.rs` does, which
  N8 does not place).
- `attempt` runs the closure without an undo: a feature that panics panics the tick, as
  any rule does.

### 7.3 The random

The game hands the level's random to the grower and the feature. Clustine's section
stream (`random_ticks.rs`'s `Random`) draws the sapling's own chances; the grower and
the feature then draw from a `WorldgenRandom` of the game's kind seeded with the
stream's next long, made at the moment the game would start drawing for the tree. Its
values differ from the game's, as every random tick's do (B7); its distribution is the
game's, and the same region state gives the same tree.

### 7.4 Across a line

A tree within a few blocks of a line sends the blocks across it as `SetIf`s (7.2); the
holder sets each where it still holds what the tree read (air, leaves, a plant, …), with
the flags and updates, a tick late when coupled. A block refused (a player built there
meanwhile) leaves a gap the game would not; named. **A bee nest's bees**: `SetIf` carries
a state and no block entity (`clustine-world/src/effects.rs` 50–54), so a nest placed
across a line is an empty nest: named (question 4). Decorators that place entities (none
of the growers' trees place a mob) need nothing more.

### 7.5 Bone meal (question 1)

If agreed: the player's use of bone meal (`BoneMealItem`, n7) on every
`BonemealableBlock` N8 builds (crops, stems, pitcher, berries, bamboo and shoot, saplings
and propagules, azalea, grass, mushrooms through `growMushroom` and the huge mushroom
features) with each block's own `isValidBonemealTarget`, `isBonemealSuccess` and
`performBonemeal` (n8 for those dumped now, n7 for the crops), the particles' level
event, and the dispenser's `Behaviour::BoneMeal` (`dispensers/behaviours.rs` 91), which
waits for this. Grass's bone meal places `grass_bonemeal` and the biome's bone meal
features through the view of 7.2.

### 7.6 What it needs

`clustine-sim` depends on `clustine-features` (no cycle: the features crate does not
depend on the simulation; worldgen reaches the simulation through N7l's maker trait).
The overworld's `FeatureSettings` is built once per process from the world's seed and
handed to regions in `RegionConfig` as an `Arc`, as the worker builds what regions share.
It adds the worldgen crates to the worker's build; measured as compile time in the
step's commit message.

## 8. Cost: what is measured, and when it is acted on

Each figure says whether it is in memory (`TMPDIR=/dev/shm`) or on the real disk with its
cache, in a release build, on a quiet machine. The light step is timed by the region and
counted (`Work`): chunks lit whole, carried, cells visited.

| Case | Measured | Acted on if (**guesses**, for the reviewer) |
|---|---|---|
| The engine alone (a bench in `clustine-light`) | a torch placed and broken; a glowstone ceiling of 64; a lamp toggled; 64 TNT's crater; a view's 400 chunks lit as arrivals in one run; each against the E1 oracle's time | an arrival dearer than 0.5 ms a chunk; a torch dearer than 0.2 ms |
| A builder (a bot placing and breaking torches, lanterns and glass twice a second) | the step's time a tick, worst and 99th centile | worst over 5 ms, or 99th over 1 ms |
| A river crossing a view, and a pinned line (E7's case) | the same, and the coupled pair's ticks a second | worst over 5 ms; ticks a second below E7's |
| 64 TNT (N7f's crater) | the step in the tick after | over 10 ms |
| A lamp clock of 16 lamps on a repeater clock (R8's), and redstone ore lit by walking mobs | the step's mean and worst | mean over 1 ms |
| A first join at view distance 8, seeded and flat | the step at each tick of the join, chunks carried and lit, the time to the view | any tick's step over 25 ms (half a tick) |
| A restore, a move, a take-over (the store or worker killed) | the pause as now (B8, E7) and the first step's share | the pause longer by more than half of B8's 0.04–0.24 s on land |
| A merge and a split with the warm light | the same | the same |
| Memory | the `Lighting`'s bytes a present chunk and a region's total | over three times the 9.3 KB mean |
| `tools/check.sh` | its length beside the last full check (4.7's oracle runs in the tests) | longer by more than a tenth |

"Acted on" means mended within N8 before it is called done: the engine made cheaper
(uniform sections filled at once as `flood.rs` does, arrivals lit by section, the
queues' entries smaller), or, if that is not enough, a question to the owner with the
figures. Light is not deferred to later ticks in N8 (decision 2).

## 9. Comparisons with the official server

All `#[ignore]`, named `official_server`, run by the lead alone on a quiet machine, the
jar from `cargo datagen`, `--no-fail-fast`.

1. **Light after changes** (`crates/clustine-light/tests/official.rs`'s way extended): a
   script of block changes made by commands on the official server (torches, glowstone,
   glass, a roof, a hole dug to the caves, water poured, a floating platform, blocks
   placed across a chunk's corner), saved and stopped; its stored light read section by
   section and compared with the engine's after the same changes on the same stored
   chunks, sections listed by both. A difference is a failure unless it is the crate's
   named case under something floating, which the test lists apart.
2. **The daylight detector** (`bin/clustine/tests/detector_oracle.rs`, R8's way): a
   detector and an inverted one in the open on both servers, `time set` through a day in
   steps; the power each sends (block updates seen by a bot) compared exactly at every
   20th tick. This also checks the sun's angle and its ease.
3. **Rates** (statistical, both servers with `tick sprint`): 256 wheat on hydrated
   farmland under a torch at midnight, under the open sky at midnight, in the dark;
   saplings of oak under the sky at midnight (none grow), by day, and under a torch at
   night; grass onto a field of dirt by day and by night; snow layers and ice beside a
   torch and glowstone (melted or not within so many random ticks). Ages and counts after
   N game ticks compared with the game's by a binomial bound at 1 in 1,000; the
   thresholds' sides (light 8 and 9, 11 and 12) placed exactly so.
4. **What a client is shown** with mobs (N7k's sixth comparison extended): a zombie at noon
   under glass and under leaves, burning on both.

## 10. Tests from the specification

Delegated to someone who wrote none of the code, from L8, the dumps' rows of section 2
and this plan, in two parts:

- **A. The engine** (`crates/clustine-light/tests/engine.rs`, by someone who did not
  write D1): property tests over small worlds (3×3 to 5×5 chunks of four to eight
  sections, so that the oracle is fast in a debug build): random sequences of block
  changes drawn from states light tells apart (emitters, dampeners, glass, slabs and
  stairs with faces, air, water), chunk arrivals and departures, copies' bases replaced,
  and runs at random points; after each run every present chunk equals the oracle, by
  `Lighting::light` and by `Lighting::at` at every block. Cases by hand: a torch placed
  and broken at a chunk's corner; light through a gap in a wall of slabs; the sky down a
  shaft and under an overhang; a section's first and last block; a chunk arriving beside
  a lit cave; a chunk leaving that carried the light of its neighbour's cave; carried
  light adopted and refused by each clause of 3.5; two histories to the same blocks
  giving the same light.
- **B. The region and the rules** (crate tests of the simulation and end-to-end): each
  rule's draws in order against a counting `Draws` (the table of 6.2 is the oracle); the
  thresholds at their edges (8 and 9 for crops and the pitcher, 9 with and without the
  night's darkening for saplings and grass, 11 and 12 for snow, ice's by its dampening,
  12 and 13 for mushrooms); seeds refused in the dark and accepted at 8; a dark crop
  breaking at a neighbour's change and not at a change of light; a mushroom's walk; a
  grass block dying under a slab and not under a layer of snow; bamboo stopping at 16;
  the detector's power at chosen times of day, inverted, and clicked; a zombie burning
  under glass and not under a roof; at a pinned line (`--pin` regions in the end-to-end
  tests) the light of the chunks each side ticks equal to the light of one region with
  the same blocks, a mushroom and grass spreading into the other side, a torch on one
  side lighting a crop on the other the tick after; a restore in the middle of a growth
  (the light after the restore equal to before, the crop growing on); a merge and a
  split with the warm light (equal to a fresh lighting); the edge's light for a chunk
  (a late joiner's chunk packets) equal to the region's `Lighting::light` for the same
  chunk once both have its 3×3; and N8f's trees: each grower's choice of list, the 2×2
  search at each corner, a failed tree putting its saplings back, a tree across a pinned
  line arriving on the other side a tick later, a refused block leaving a gap.

Twice before such tests found an ordering bug the author's own passed (CLAUDE.md).

## 11. The division of work

Steps, each one verified commit to `main`, pushed (the four commands of CLAUDE.md in a
clean checkout, the worlds in `/dev/shm`):

| Step | What | Whose |
|---|---|---|
| N8a | The contract: `clustine-light`'s `engine.rs` interface with E1's bodies (3.2, 3.9); the region's `Lighting` field, `block_written` and every hook of 4.2, the light step (4.1), the reads (4.3) with `MobWorld::light` and the views moved onto them, 4.7's check; the dumps of section 2's last paragraph | the lead |
| N8b | The incremental engine behind the interface (3.3–3.5), its benches | D1, delegated |
| N8c | The rules of 6.2 but saplings, survival (6.3), mushrooms' and grass's `SetIf`s | D2, delegated |
| N8d | The daylight detector (6.4) and the sun's angle | D3, delegated |
| N8e | Merges, splits and restores with the light (4.5), carried light (3.5) wired in the region, the measurements of section 8 but N8f's, the comparisons 1, 2 and 4 | the lead |
| N8f | Saplings and trees (section 7): the lead's view for features (7.2) and the dependency (7.6) first, then `TreeGrower`, the saplings and the propagule, and bone meal if agreed, delegated (D4); comparison 3 | the lead, then D4 |
| N8g | Tests from the specification (section 10), part A after N8b, part B after N8c to N8f | D5, delegated, someone who wrote none of it |

Order: N8a first and pushed before anything is delegated (a worktree starts from
`origin/main`). Then D1, D2 and D3 at once, each in a worktree of its own with crates or
files of its own: D1 `crates/clustine-light/src/engine*.rs` and its benches only; D2
`region/random_ticks/{growth.rs, melting.rs}` and `clustine-blocks/src/survive.rs`; D3
`region/block_entities.rs`'s detector part and `time.rs`'s sun angle. N8e after D1 is
merged. N8f after N8c (the propagule and the grass share the dispatch). D5's part A may
start against N8a's E1 at once, since it tests the interface.

Each brief states the interface, the tests expected and how to verify (`cargo test -p
clustine-light` or `-p clustine-sim`, fmt and clippy in its worktree), says that the agent
does not commit to `main`, push, download, or start Docker, kind or the official server,
and that it removes its worktree's `target` when done. The lead reads and tests what
comes back before merging, and runs the full check before each push.

**Not delegated**: the hooks of 4.2 and the step's place in the tick (N8a), the warm light
at merges, splits and restores (N8e), and the view for features with its effects (N8f's
first half): the parts where ordering mistakes hide.

## 12. Records and the roadmap

- **A record of its own, ADR-0029 "Light in the simulation"**, written from this plan
  once agreed and reviewed before N8a: the engine and its invariant, the step, derived
  light, option a with its bound, trees as effects. It amends A24 section 8 (light is now
  in the simulation), A23 (regions light their chunks too; the edge is unchanged and
  nothing passes between them), A25 section 4 (`SetIf` for growth and trees).
- **What to try**, written into the roadmap's first section when N8 is done:
  `cargo run --release -p clustine -- --seed 3 --pin 4 --reshape by-hand`. Plant wheat on
  watered farmland beside a torch in a dark room and in the open; at night the room's
  grows only near the torch and the open one grows on; seeds cannot be planted in the
  dark. Put a torch beside snow layers and ice: they melt. Clear grass from a field: it
  comes back from the edges by day. Plant saplings of every wood (four in a square for
  spruce, jungle, dark oak and pale oak): they grow into the game's trees by day, not at
  night without a torch; one planted at x = 62 grows across the line. Bamboo grows; a
  mushroom farm fills a dim room. A daylight detector beside a lamp lights it by day, the
  inverted one at night. A zombie under glass at noon burns; spawn monsters beside your
  torches and watch them walk off to the dark.

## 13. Open questions, each with my recommendation

1. **Bone meal.** Not in the lead's list, but a creative player with a sapling reaches
   for bone meal within minutes, the dispenser waits for it, and it shares N8f's whole
   machinery. *Recommended*: in N8f, for every bonemealable block N8 builds (7.5).
2. **The light out of the region's equality** (4.6), judged by the oracle instead of by
   comparing regions. *Recommended*; the alternative, lighting at `restore` so that a
   restored region equals its original between ticks, would still differ until the
   copies come back.
3. **Carried light** (3.5) adopted under the stricter test. *Recommended* as written, and
   measured; if it saves nothing measurable, dropped in N8e, which says so.
4. **A bee nest across a line comes without its bees** (7.4), rather than giving `SetIf` a
   block entity (a change of the effects' encoding). *Recommended*: named; it needs a
   bee tree, flowers near, and a line within a few blocks.
5. **A stem turned to a fruit the holder refused** (6.2) stays turned until a shape
   update. *Recommended*: named; making the two atomic across a line would need the
   holder to answer.
6. **Crops in dark rooms of worlds played before N8** break as their neighbours change.
   *Recommended*: as the game; said in what to try.
7. **The rim** (section 5, option a). *Recommended* as written: no threshold differs.
8. **The bounds of section 8** are guesses. *Recommended*: the reviewer looks at them;
   the measurement writes the figures into ADR-0029 whatever they are.
9. **The dumps still owed** (section 2's last paragraph), above all `SpreadingSnowyBlock`
   for grass and `TreeFeature` for the trees' flags. *Recommended*: the lead dumps them
   in N8a, before D2 and D4 start, and corrects this plan where they differ.
10. **Saplings last, as N8f.** *Recommended*: nothing else waits for them, they carry the
    one new dependency and the one new kind of cross-line work, and a player meets
    crops, grass and melting first.
11. **The light step before post-processing** (decision 1's refinement): a chunk's first
    ticking tick reads the light of its unsettled blocks. *Recommended*: named; moving the
    step after post-processing would put the tick's own block ticks before light.
12. **A record (ADR-0029) before building**, reviewed. *Recommended*: the region gains
    state of a new kind (derived, outside its equality) and a new kind of work across a
    line, both of which the earlier records' readers will look for there.
