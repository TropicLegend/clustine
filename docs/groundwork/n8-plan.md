# N8: light in the simulation

Draft of 2026-10-26, for the owner and an independent reviewer before anything is built;
reviewed the same day. Section 14 records the review and what the plan is now; the
sections above it were corrected in place where the review changed a number or a rule,
and where they and section 14 still differ, section 14 holds.
The game's rules are those of `n8-brief.md` (cited as L8: L8 0.4 is finding 4 of its
section 0, L8 5.8 its section 5.8), read from the 26.3 jar. The classes L8 section 6 found
missing are dumped now in `/home/claude/reference/n8-javap/` (67 classes, `javap -p -c
-constants`, from the jar `cargo datagen` keeps; nothing downloaded; `AttachedStemBlock`
and `StemBlock` added by the lead after the review), cited as n8 with the
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
   copies and loads are taken (`update_chunks`), the players' actions and the sleep
   check, and before `update_ticking`, which decides which chunks tick (`region.rs`
   1039–1058; section 4.1, moved there at the review, section 14). Every rule of the tick
   then reads light that is current with the blocks as the tick began, which is what the
   game nearly always gives a rule in the tick after a change (L8 2.3; the dumps confirm
   the race, section 2 rows 1 and 2), and a player's action is read in the tick it is
   applied. A tick stays a function of the region and its inputs: light is a function of
   blocks, taken at a point the tick fixes. *Refinement*: what the tick changes after the
   step (other regions' effects in `run_seams`, blast drops, block and fluid ticks,
   post-processing, random ticks, entities) is lit at the next tick's step, so a chunk
   post-processed in its first ticking tick has that tick's random ticks read the light
   of its unsettled blocks. Named; no rule of section 6 can tell within one tick.
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
   in a fixed order. Chunks that arrive are lit within a **deterministic budget** a
   tick, counted in chunks and never by time, and a chunk ticks only once its 3×3 is lit
   (section 4.5; the lead's decision at the review, which the owner may overrule); an
   engine over the cost bounds of section 8 is measured and mended in N8.
   *Added*: the lead's first commit puts a whole-chunk implementation (L8's E1) behind
   the engine's interface, so that the rules and the region can be built and tested
   against exact light at once, and the incremental engine replaces it behind the same
   interface with the same tests (section 3.9).
3. **Derived, not committed.** Light is no part of a commit, of the log or of a saved
   chunk; a region lights what it has. It reuses a chunk's carried light where a test
   stricter than A23's T says it is what the engine would compute (section 3.5:
   T as the edge has it can pass at the rim of what a region has, where the region's
   light must differ), adopts at a merge or a split the levels each chunk had in the
   region it came from (section 4.5), and computes the rest. The pause this adds to a
   load, a merge, a split and a take-over is measured on the real disk (section 8).
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
   that is light, and none of the other rules waits for it. Bone meal is built with the
   trees (question 1, agreed at the review).

## 1. What a player sees when N8 is done

### 1.1 Crops and their kin

- **Wheat, carrots, potatoes** grow where the light at the crop (sky or block, the sky
  not darkened by night) is 9 or more, at the game's chance by the farmland around (n7
  CropBlock 100–140, 174–326): under the open sky they grow day and night; in a cellar
  only by a torch. **Beetroots and the torchflower crop** the same after a one-in-three
  that skips (n7 BeetrootBlock 31–45, TorchflowerCropBlock 68–81); a torchflower crop
  grown out becomes a torchflower. **Melon and pumpkin stems** grow by the same light and
  at age 7 set a fruit beside them on a block that allows it (n8 StemBlock 71–256). A
  fruit harvested turns its attached stem back into a stem of age 7, which fruits again
  (n8 AttachedStemBlock 54–100: the shape update from the side it faces, where the fruit
  is gone).
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
  torch (14) melts the snow within two blocks of it (12 at two steps), glowstone or a
  lantern (15) within three.
- **Ice** melts to water where block light is above 11 less its own dampening (n8
  IceBlock 63–115), or vanishes where water evaporates (no such place in the overworld).
  Packed and blue ice are other blocks and never melt. The sun melts nothing: both read
  block light alone.

### 1.3 Grass and mycelium

- A grass block or mycelium **dies to dirt** where the block above is not one layer of
  snow and either holds a full fluid or takes 15 from light passing down into it (it
  dampens 15, or the faces the two show each other close the way:
  `LightEngine.getLightDampeningInto`); it **spreads** to dirt one block across and up to
  three down or one up, four tries a random tick, where the light above it is 9 or more
  *with* the night's darkening: by day, or by a torch at night. The dirt it spreads to
  must pass the same test and have no water above; no light is read there (n8
  SpreadingSnowyBlock 15–167, section 6.2). Paths of grass come back to a cleared field
  in a few minutes of daylight.

### 1.4 Bamboo

- **Bamboo and its shoot** grow one block, one try in three, where the block above is
  air and its light (not darkened) is 9 or more, up to 16 tall, the leaves of the top
  three changing as the game's (n8 BambooStalkBlock 227–270, 454–572; BambooSaplingBlock
  21–44, 126–149). A shoot with bamboo grown above it becomes bamboo (n8
  BambooSaplingBlock 55–85, its shape update), so a shoot grows into a stalk.

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
  39–89). So saplings grow by day or by a torch, not in the open at night. A mangrove
  propagule on the ground reads no light: it grows by the same one in seven in the dark
  too (n8 MangrovePropaguleBlock 216–245). The tree is
  the game's for its wood, from the same features generation places: oak or (one in ten)
  fancy oak, with a bee nest at times beside flowers; spruce, and with four saplings in a
  square mega spruce or mega pine; birch; jungle, and the mega jungle tree from four;
  acacia; cherry; dark oak and pale oak only from four; mangrove from a propagule on the
  ground (tall mangrove 85 times in 100, by the weights 15 : 85); and the poplars, red,
  orange or yellow
  (n8 TreeGrower 101–219, 514–743). A sapling with no room stays a sapling.
- A tree beside a line grows across it: its blocks in the other region's land appear a
  tick later (coupled regions) or a few ticks later, where nothing has taken their place
  meanwhile (section 7.4).
- With bone meal (question 1, agreed): a sapling advances at a chance of 0.45, as a
  random tick's success does, so a sapling of stage 0 takes stage 1 and the tree comes
  at the second success (n8 SaplingBlock 124–146, `performBonemeal` calls
  `advanceTree`); crops and stems age, grass sprouts grass and flowers up to about eight
  blocks around, a mushroom becomes a huge one at 0.4, bamboo, berries, a pitcher and a
  propagule grow; a dispenser does the same.

### 1.7 The daylight detector

- Every second, a daylight detector under the sky gives the sky's light at it less the
  night's darkening, lowered by the sun's angle towards dawn and dusk; inverted, 15 less
  the light (n8 DaylightDetectorBlock 42–120, 185–216). Clicking it inverts it and
  updates its output at once (121–161). It drives wire, lamps and the rest of phase R's
  circuits; across a pinned line, as ADR-0027 lends it.

### 1.8 Mobs

- Every read of light a mob makes (L8 1.5) reads the region's light, which sees every
  torch, lantern, lava and glowstone, sees through glass (which lets the sky's light
  through) and leaves (which pass it at one less a block), and follows every change. So
  an undead mob under glass burns at noon (the game's), one under a tree's leaves or in a
  lit hut's shade does not (sky 14 under leaves, and burning needs 15:
  `clustine-light/tests/cases.rs` 503); monsters' strolls prefer the dark and
  animals' the light as they should around a player's torches; a skeleton fleeing the
  sun finds a hiding place under a real roof; a fox's shelter and a villager's
  sky-seeing spot are where the game's are.

### 1.9 Left out, and named

- **Frosted ice** (n8 FrostedIceBlock 42–176) is made only by Frost Walker, which
  Clustine has no enchantment for: its scheduled tick (its ageing) is not built. Its
  random tick, which it inherits from `IceBlock`, is: frosted ice placed by a command
  melts by block light as ice does, as in the game.
- **Snow and ice forming, lightning, phantoms**: weather and N9.
- **Natural spawning and spawners**: N9, reading the light N8 makes.
- **What a client is shown** is the edge's light (A23), as now.
- **The rim** (section 5): in a copy or ring chunk, light from beyond what the region
  has is missing; only a stroll's ranking of dark candidates there can differ.
- **A tree's bee nest** grown across a line comes without its bees, and a tree's leaves
  across a line may keep a distance the tree would have rewritten where one is refused
  (section 7.4); a **stem's fruit** across a line may be refused after the stem has
  turned to it, and the stem turns back at the next change beside it (section 6.2).
- **Chunks waiting for light** (section 4.5): after a take-over or a restart, chunks at
  the edge of a view stand still for a few ticks until they are lit, rather than the
  whole region pausing.

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
| 8 | 1.1, 6: grass in `SpreadingSnowyDirtBlock` | n8 GrassBlock and MyceliumBlock extend **`SpreadingSnowyBlock`** (26.3's name), given the dirt block's key; GrassBlock holds bone meal (41–105: 128 walks of up to 7 steps) and particles. n8 SpreadingSnowyBlock 15–167: `canStayAlive` is one snow layer above, else not a full fluid above and `getLightDampeningInto(state, above, UP, …) < 15`; dying sets the dirt by `setBlockAndUpdate` with no draw; spreading needs `getMaxLocalRawBrightness(above) >= 9`, then four tries at (`nextInt(3)−1`, `nextInt(5)−3`, `nextInt(3)−1`) onto the dirt block that passes `canStayAlive` with no `#water` fluid above, set to the default state with `snowy` from the block above being in `#snow` (n8 SnowyBlock), flags 3 | **corrected** (the review, finding 11): the dump replaces SteelMC's row; section 6.2 |
| 9 | 1.1 ice (**Inferred**: `11 - getLightBlock()`) | n8 IceBlock 63–115: block light `> 11 - getLightDampening()`; `melt`: where water evaporates `removeBlock`, else `setBlockAndUpdate(water)`, `neighborChanged(pos, water)`, game event `block_destroy` | confirmed, the method named |
| 10 | 1.1: frosted ice among random ticks | n8 FrostedIceBlock 42–176: a **scheduled** tick (60–120 after placing, 20–40 after); `nextInt(3) == 0` or fewer than four frosted neighbours, then `getMaxLocalRawBrightness > 11 - age - dampening` (block light in the End) | not built (1.9) |
| 11 | 1.1 bamboo (**Inferred**) | n8 BambooStalkBlock 227–270: stage 0 (no draw), `nextInt(3) == 0`, air above, `getRawBrightness(above, 0) >= 9`, height below plus one under 16. BambooSaplingBlock 21–44: `nextInt(3)`, air above, light ≥ 9, bamboo with small leaves above | confirmed |
| 12 | 1.1 mushrooms (**Inferred**): "a few tries at a spot one away" | n8 MushroomBlock 26–137: `nextInt(25) == 0`; at most four of the kind in −4..4, −1..1, −4..4 (`atMostMatched(4)`, the mushroom counted); a target at (`nextInt(3)−1`, `nextInt(2)−nextInt(2)`, `nextInt(3)−1`); four times: step onto the target if `isEmptyBlock` and the mushroom's full `canSurvive` there, then draw the next target from where it stands, **whether or not the step was taken**; then set the mushroom's own state at the last target with flags 2 if it is empty and survivable | **corrected**: a walk of up to five steps, so a spread lands up to five blocks across, possibly in a copy; four draws of the next target always |
| 13 | 1.1 pitcher (**Inferred**: `hasSufficientLight` in `canGrow`) | n8 PitcherCropBlock 219–375: the draw `nextInt((int)(25/f)+1) == 0` **first**, then `canGrow`: not grown out, `CropBlock.hasSufficientLight` (≥ 8), inside the height, the block above air or pitcher crop from age 3; survival of the lower half asks the same light (127–144) | confirmed, with the draw first and the threshold 8 |
| 14 | 1.3 detector (**SteelMC**): the sun's angle in radians | n8 DaylightDetectorBlock 42–120: `getEffectiveSkyBrightness`, `visual/sun_angle` **in degrees** times 0.017453292; inverted, `15 − i`; else for `i > 0`, `f += ((f < π ? 0 : 2π) − f) * 0.2`, `i = round(i * cos f)`; clamped 0..15; set by `setBlockAndUpdate` if it differs. The ticker exists only where the dimension has sky light; runs when `gameTime % 20 == 0` (185–216). Use: invert with flags 2, game event `block_change`, update at once (121–161) | corrected (degrees); section 6.4 |
| 15 | 1.1: the propagule reads no light; azalea grows by bone meal alone | n8 MangrovePropaguleBlock 216–245: not hanging, `nextInt(7) == 0`, `advanceTree`; hanging, its age (built). n8 AzaleaBlock: no random tick; bone meal at 0.45 | confirmed |
| 16 | 1.1 "read no light": cocoa, kelp, nether wart, sugar cane | no brightness or light layer anywhere in their dumps | confirmed |
| 17 | 6: `DynamicGraphMinFixedPoint`, `LeveledPriorityQueue`, `SpatialLongSet` asked for with the engine | in the lighting package, but no engine class refers to them | not needed |
| 18 | (not in L8) a stem attached to its fruit | n8 AttachedStemBlock 54–106: `updateShape` from the side it faces, where the neighbour is no longer its fruit, gives the stem's default state at age 7; otherwise `VegetationBlock.updateShape` (survival); `mayPlaceOn` its `supportBlocks` tag | ported in D2 (section 6.3; finding 7) |
| 19 | (not in L8) the bamboo shoot's shape | n8 BambooSaplingBlock 55–85: cannot survive gives air; from above, bamboo there gives bamboo's default state | ported in D2 (finding 8) |
| 20 | 0.11: a tree's flags | n8 TreeFeature 23, 79–85, 288–293: `BLOCK_UPDATE_FLAGS = 19` for every block a tree sets (`setBlockKnownShape`) | confirmed, no longer inferred |
| 21 | (not in L8) the growers' draws | n8 WeightedList 149–166: `getRandom` draws `nextInt(totalWeight)` whenever the list is not empty, **also for a list of one**; mangrove's plain list is 15 mangrove : 85 tall mangrove (n8 TreeGrower 185–241) | spruce, jungle, dark oak and pale oak draw from their mega list even where it holds one tree (section 7.1) |
| 22 | 1.3 the detector's sun angle | `timeline/day.json`: two keyframes at tick 6,000, 360° then 0°, eased by `cubic_bezier` (0.362, 0.241, 0.638, 0.759); n8 EasingType$CubicBezier 64–160: four Newton–Raphson steps, tolerance 1e-5, a step clamped to ±0.25, then bisection, all in `f32` | section 6.4 |

**Still to be dumped**: `levelgen.feature.Feature` (its `setBlock`, for the flags of the
huge mushrooms' and the bone meal features' sets, which the features crate does not
carry since they do nothing in generation (`clustine-features/src/context.rs` 314),
taken meanwhile as 3, **Inferred**) and `WorldGenLevel` (its default `ensureCanWrite`, taken as true on a
`ServerLevel`, which does not override it: n5 ServerLevel), both for N8f, by the lead in
N8a. Everything else this plan cites is dumped: `SpreadingSnowyBlock`, `SnowyBlock`,
`TreeFeature`, `WeightedList`, the ease and the timeline classes (since the draft), and
`AttachedStemBlock` and `StemBlock` (after the review); `BoneMealItem`'s and
`BonemealableBlock`'s callers are in n7. The keyframe sampler is cited from n7d-brief
706–712, read from the jar.

## 3. The engine

### 3.1 What it is a function of

**The invariant**: after each run, the light the engine holds for every chunk it has been
told of (the region's chunks, loaded, ring, lent or copied, that it has handed to the
engine: its *present* chunks, which the region calls *lit*) is `clustine_light::light`
of that chunk and its eight neighbours as present, an absent neighbour being a wall. A
chunk the region has but still keeps waiting for its turn (section 4.5) is absent to the
engine, a wall like any other. Light
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

/// What a run did, for the figures (section 8). Counted, never timed.
pub struct Work { pub chunks_lit_whole: u32, pub adopted: u32, pub cells: u64 }

/// A chunk's levels as a run left them, handed out at a merge or a split and adopted
/// where the chunk comes back with the same blocks (section 4.5). `walls` names the
/// eight neighbours that were absent at that run, `taken` names the hand-over (the
/// region and its tick), so that two chunks kept from it know their levels agree.
pub struct Kept { /* the levels of every section, sky and blocks; the column sources */ pub walls: u8, pub taken: (u64, u64) }

pub struct Lighting { /* the levels of present chunks, and what waits for a run */ }

impl Lighting {
    pub fn new(sky: bool) -> Self;
    /// A chunk became present. Where it is present already, its blocks were replaced
    /// whole (a copy's new base, a loaded chunk become a copy or the reverse): the run
    /// takes it as a departure and then an arrival, since the old blocks may have lit
    /// its neighbours (finding 3).
    pub fn arrived(&mut self, at: ChunkPos);
    /// A chunk became present with levels kept from a run elsewhere, its blocks as
    /// they were then. Its faces are mended against its neighbours as they are now:
    /// towards a neighbour adopted from the same hand-over in this same call to `run`
    /// they stand; towards any other (one adopted earlier, whose own faces were
    /// mended when this chunk was absent, among them)
    /// they are taken as a departure (a wall, decreased) and then, where the neighbour
    /// is present, as an arrival (pulled in both ways). Section 4.5.
    pub fn adopt(&mut self, at: ChunkPos, kept: Kept);
    /// A chunk stopped being present.
    pub fn left(&mut self, at: ChunkPos);
    /// A block changed in a way `light_tells_apart` sees.
    pub fn changed(&mut self, at: BlockPos);
    /// Brings every present chunk to the invariant of 3.1, reading `chunks` as they are.
    pub fn run(&mut self, chunks: &dyn Chunks) -> Work;
    /// The levels at a block as of the last run; `None` where no present chunk is.
    /// Above and below the sections it keeps: sky 15, blocks 0.
    pub fn at(&self, at: BlockPos) -> Option<LightAt>;
    /// The chunk's light in the crate's form (sections listed by its rule), for the
    /// tests and for whoever later wants it whole (N9, the edge after N8).
    pub fn light(&self, at: ChunkPos, chunks: &dyn Chunks) -> Option<Light>;
    /// The chunk's levels to hand out with it, after a run (section 4.5).
    pub fn kept(&self, at: ChunkPos) -> Option<Kept>;
    /// Whether the chunk is present.
    pub fn has(&self, at: ChunkPos) -> bool;
}
```

`arrived`, `adopt`, `left` and `changed` only note; nothing is lit before `run`. A
position noted in a chunk that is not present is passed over. `Lighting` and `Kept`
have a short `Debug` (counts, not levels). `Lighting` holds a `BTreeMap` of chunks and
vectors; no hash map, no clock, no thread (the crate's `clippy.toml` is the
simulation's). The budget of section 4.5 is the region's, not the engine's: the engine
lights what it is told of, so that its property tests need no budget.

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
  section 2 row 1). `run` takes the departures first (replacements' departures among
  them), then the checks, propagating all decreases and then all increases from two
  FIFO vectors with the game's packed entries and directions; then the arrivals and
  adoptions **one chunk at a time** in the order noted, each propagated to its fixed
  point before the next (the values are the same fixed point whatever the order, and the
  queues then hold at most one chunk's flood and what spills from it).
- **Where it looks**: a step from cell to cell does not look the chunk up in the
  `BTreeMap`; the chunk being worked and its eight neighbours are resolved once into a
  window at hand, and a step leaving the window resolves the next. Sections of one
  state (air, stone, water) are filled at once as `flood.rs` fills them: a necessity for
  arrivals, not a later remedy.
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
- **An arrival** is lit whole from nothing: every block of the chunk that gives light
  enqueues its increase, every column's sky sources are added, and the cells of present
  neighbours along its faces pull light in. A wall that becomes blocks can only add ways
  for light, so an arrival makes no decreases; a chunk whose blocks are replaced whole is
  therefore a departure first and then an arrival, in the same run (`arrived` on a
  present chunk, 3.2).
- **A departure** is the reverse: blocks that become a wall can only take ways away. The
  departed chunk's cells on its faces become walls and are decreased through the ported
  `checkNode`, as any block that comes to stop light is; all light from the chunk into a
  neighbour passes those cells, so the game's own decrease takes away what came through
  them and re-enqueues what the neighbours have from elsewhere, their own sky columns
  and emitters at the face among them; then the chunk is dropped. (Decreasing the
  neighbours' face cells instead, as the draft had it, would zero their own sources
  there with nothing to add them back: finding 4.)
- **An adoption** (section 4.5) takes the kept levels as they are and mends only the
  faces, as `adopt` says (3.2).
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
2. all eight neighbours are present to the engine (lit before, or taken in the same run)
   and as generated by *g* (`Chunk::generated`); and
3. every section the light lists as absent lies above every section of the chunk that
   holds a block. Then its sky is 15 and its blocks' light 0 in both the crate's and the
   game's reading, so the engine fills the levels it keeps for absent sections exactly.

Else it is lit (3.4). The property tests include chunks adopted so. An adopted chunk
counts against the budget of section 4.5 as an adoption, not as a chunk lit whole. It
is built (the lead's decision at the review, section 14): in the flat world every chunk
but those at the rim of what the region has passes; in a seeded world after B6 most
chunks carry a mark or a tick and lose their light at post-processing (A23 section 9:
72 in 100), so little passes. How much it saves is measured and written into ADR-0029.
The review offered no simpler test that holds: without clause 3 the engine would have to
guess the levels of absent sections under an overhang, and without clause 2's "present"
the light would be the light beside a neighbour the region does not have.

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
queues are kept between runs; since arrivals are lit one at a time (3.4), an arrival
puts at most about one chunk's flood and its spill in them (some 100,000 entries of 16
bytes, under 2 MB, **guess**), and what changes put there is bounded by the tick's
writes (a crater's the most). Their peak is measured with the rest.

### 3.9 The first implementation, and the oracle

The lead's first commit implements `Lighting` by L8's E1: `run` lights whole, with
`clustine_light::light_with` (asking the levels of absent sections too, `Unlisted` all
set), every present chunk within `reach` of a change (with all eight where a section's
emptiness changed), every arrival, adoption and replacement and the present neighbours
of each and of every departure (E1 relights an adopted chunk whole and ignores its
kept levels, which is exact). It is exact by
construction and slow (13 ms a change at the mean, L8 5.4).
The rules (D2, D3), the region's step (L2) and every test are built on it. The
incremental engine (D1) then replaces the bodies; the E1 code stays in the tests as the
oracle, and its property tests (section 10, part A) are written against the interface,
not the implementation, so they pass on both.

## 4. Light in the region

### 4.1 The light step in the tick

`Region::tick` (`region.rs` 923–), with the step marked:

1. the clock, `update_chunks` (loads, lets go, copies' bases, updates and withdrawals);
2. edge events, players joining, leaving and arriving, the entering stays, faces, the
   players' actions (`apply_remote`, `apply_input`), which read the last step's light;
3. the sleep check;
4. **the light step**: the chunks' noted changes drained into the engine (4.2), the
   departures and replacements, the waiting chunks taken within the budget (4.5), then
   `self.light.run(&present)`; the darkening of the tick taken once from the clock
   (`time::sky_darken`);
5. `update_ticking`, in which a chunk ticks only where its 3×3 is lit (4.5), `run_seams`
   (seam entries and effects from other regions), `lay_blast_drops`, `take_arrivals`;
6. block ticks, fluid ticks, post-processing, random ticks, block events, entities, block
   entities and the rest, as now (`region.rs` 1058–).

The draft put the step after `take_arrivals`; it is before `update_ticking` now (the
review, section 14), so that a chunk lit in the step can tick in the same tick: a chunk
loaded in a tick and lit within the budget ticks in that tick, as now, and a region
made by `restore_loaded` goes on as its promise says (4.5). What `run_seams` and
`lay_blast_drops` set (other regions' effects, a tick late already, A25 section 1) is lit
at the next step, as block ticks' changes are.

Everything that reads light in a tick reads the step's light: random ticks, block ticks
(none of section 6 is one), placing and using in later ticks, survival in shape updates,
mobs, the detector. What changes after the step is noted and lit at the next step.

### 4.2 What tells the engine

- **Every write to a chunk's blocks** goes through `Chunk::set`
  (`clustine-world/src/chunk.rs` 343–368): the region's set (`region/updates.rs` 172), the
  direct `.set(` calls elsewhere (a test's in `random_ticks/fire.rs` 1024 among them), a copy's
  update through `Chunk::apply` (`clustine-world/src/change.rs` 52–58), the tests'
  builders. So the noting is put there, not at the sites: a chunk keeps a list of the
  positions whose change `light_tells_apart` (the test `set` already makes to drop
  carried light, chunk.rs 364), switched on by the region when the chunk comes to it and
  off in a chunk made anywhere else (generation, the store, the edge pay nothing). The
  list is not serialised and no part of the chunk's equality (`Chunk`'s `PartialEq` is
  written by hand, chunk.rs 266–278, and its serialised form is `ChunkRepr`); the light
  step drains it from every lit chunk into `Lighting::changed`. No write can go round
  it, and no audit of sites is needed (the draft's "34 sites" counted `get_mut`s that
  touch entities and ticks). **The lead's**, in N8a.
- **Copies**: a new base is a replacement (`arrived` on a present chunk); a withdrawal
  is `left`; their updates are noted by `Chunk::set` as above.
- **Loads and lets go**: a chunk inserted into `chunks` waits for its turn (4.5), one
  removed is `left`, unless a copy of it remains or arrives in the same tick: then it is
  a replacement, since its blocks may be another sequence's; a copy dropped because its
  chunk is now loaded is a replacement by the loaded chunk. Replacements are lit in the
  run they happen in, outside the budget (4.5), so that no ticking chunk beside one
  stops.

### 4.3 Reads

One set of functions on the region, the game's names, in `region/light.rs`:

| Function | The game's | Read by |
|---|---|---|
| `raw_brightness(pos, darken)` | `getRawBrightness`: `max(sky − darken, block)` | crops, stems, berries, bamboo, survival (darken 0) |
| `max_local_raw_brightness(pos)` | `getMaxLocalRawBrightness`: the above with the tick's darkening; 15 outside ±30,000,000 (n7 LevelReader 297–330) | saplings, grass, the mobs' magic value |
| `block_brightness(pos)` | `getBrightness(BLOCK, pos)` | snow, ice |
| `effective_sky_brightness(pos)` | `getEffectiveSkyBrightness` (n7 LevelReader 332–341) | the detector |
| `can_see_sky(pos)` | sky ≥ 15 (n8 BlockAndLightGetter 24–36) | mobs, navigation |

Each answers `None` where the chunk is not lit: absent, or present and still waiting for
its turn (4.5), such as a chunk loaded in this very tick that a player's action (step 2)
reads before the step. `None` is "not judged", as now: seeds go in. Above and below the
sections the engine keeps, sky 15 and blocks 0. Through them:
`RegionView::raw_brightness` (`updates.rs` 1101) and the fluids' view answer, so every
`BlockView` rule judges light (the mushroom's survival in `clustine-blocks/src/survive.rs`
186–193 among them); the placement view (`placement/context.rs` 66) answers, through a
light read the placement `Blocks` trait gains (`placement.rs` 31, stubbed by the lead in
N8a), so seeds are refused in the dark; `MobWorld::light` (`region/mobs.rs` 166–192) reads
`Lighting::at` and its fallback goes. The views that answer `None` still (the rails',
physics') read no light-dependent rule; each says so where it answers.

### 4.4 The darkening

`skyDarken` is set once a tick from the clock before the chunks tick (L8 2.4); Clustine
has it from the region's clock (`time.rs` 43–50). Nothing is relit at dusk: the stored
sky light is the sky's full light and the darkening is taken at each read.

### 4.5 Loads, merges, splits, restores, moves and take-overs

**The budget** (the lead's decision at the review, which the owner may overrule;
finding 2). A chunk the region comes to have (loaded, copied, in its ring) is not lit at
once: it joins the region's queue of *waiting* chunks, which is the region's, not the
engine's. Each light step takes from the queue, in ascending order of (the chunk's
distance in chunks from the nearest of the region's players as they stand after step 2,
its position; in a region without players, by position alone), as many as
fit in **256 units, a chunk lit whole counting 8 and a chunk adopted (kept levels, or
carried light by 3.5) counting 1**: 32 chunks lit whole, or 256 adopted, a step. The
count is of chunks and never of time or of cells, so it is a function of the region's
state and of the order, and the same whichever implementation of the engine is behind
the interface (a count of cells would differ between E1 and D1). A chunk is taken whole
or not at all; a chunk at the head that does not fit waits for the next step. The
figure of 32 is fixed in N8a and set in N8e from the measurement (section 8) to the most
that keeps a step within 16 ms on the owner's machine in a release build; it is a
constant, named in ADR-0029. Departures, changes and replacements are not budgeted.

**A chunk ticks only once its 3×3 is lit**: `update_ticking`, after the step (4.1), asks
of every chunk it would tick that it and its eight neighbours be lit, besides what it
asks now. Rules read light only in and around ticking chunks, at most 10 blocks from
them (section 5), so no rule reads a waiting chunk but a path's trim from the sun,
whose nodes can lie farther (6.5); a player's action may, and finds `None` (4.3). Waiting chunks are walls to the engine (3.1), so the light of a lit chunk
beside a waiting one is the light with a wall there until it is taken.

- **A load** (a join, a flight, chunks claimed): the chunks arriving in a tick wait and
  are taken nearest first. The worker already asks for chunks nearest first
  (`services/worker/src/lib.rs` 2601), so a joining player's own chunks are lit and tick
  first and the edge of the view last. Measured (section 8).
- **A restore** (a crash, a move, a take-over, a restart of the store): the restored
  region has no light; all its chunks wait, and are taken nearest first. **`restore_loaded`'s
  promise is amended** (`region.rs` 657–667: "so that it goes on as the region whose
  state and chunks were taken in the tick before"): it holds where the region's chunks
  fit in one step's budget, which is so for every test that relies on it (their worlds
  are a few to some tens of chunks; the lead counts them in N8a), and otherwise the
  region's chunks wait for their light as arrivals do. What a player sees after a
  take-over is the chunks at the edge of their view standing still for a few ticks (a
  view of 650 chunks lit whole in about 20 steps, a second), while the chunks around
  them tick from the first tick, rather than the whole region stalling for a fifth of a
  second or more while everything is lit. The doc comment says so and why.
- **A merge or a split** begins anew through `Region::restore` with warm chunks
  (A24 section 4; A25 section 3): `take_absorbed` and `take_split` replace the region
  whole (`*self = Self::restore(...)`, `region/reshape.rs` 251–260, 417–430), the loaded
  chunks go back to the runner, which keeps them in `warm` (`services/worker/src/lib.rs`
  2275–2281) and hands them back as `chunks_loaded` only once a ticket asks again, a
  tick after the request (2602–2606); copies are handed in afresh. No `Lighting`
  survives that, so the draft's "the absorber keeps its `Lighting`" could not be built
  (finding 1). Instead **each chunk's levels go with it**: before giving the chunks up,
  `take_absorbed` and `take_split` run the light step once more (so the noted changes of
  the last tick are lit, unbudgeted for changes; waiting chunks go without levels), then
  hand each lit chunk out with `Lighting::kept`. The runner keeps the `Kept` beside the
  chunk in `Warm` and hands it back with the chunk in `chunks_loaded` (an `Option`
  beside each chunk); the region queues such a chunk as an adoption (1 unit), which
  `adopt` mends at its faces (3.2). The blocks are the same by construction: a warm chunk
  is not written while it is warm, and a `Kept` is dropped with its `Warm` (a chunk the
  store delivers afresh is lit as any arrival). So a merge or a split relights only faces,
  a step or two after the chunks come back. **Ordering-critical and the lead's** (N8e).

### 4.6 Not state, not committed

The light is no part of `RegionState`, the commit, the log or a saved chunk. Saving a
chunk is unchanged: `Chunk::light` keeps its meaning (carried light, dropped at a change)
and the region never writes its own light into a chunk; the list of noted changes (4.2)
is not serialised. The `Lighting` field and the queue of waiting chunks are left out of
the region's equality (a wrapper whose `PartialEq` holds always and whose `Debug` gives
counts), since between ticks
a running region's light is its last step's and a restored one's is none or whole; the
tests judge it by the oracle instead (4.7, question 2). Nothing is raised: the format,
`FEATURES_VERSION` and `clustine_light::VERSION` stay; the engine changes no value the
crate gives.

### 4.7 What the region checks of itself in tests

After the step the region may compare the light of lit chunks with
`clustine_light::light` of their lit 3×3 and panic on a difference. How much, is a field
of the region's configuration (the light settings of `RegionConfig`, N8a), not a cargo
feature: a feature enabled through `bin/clustine`'s dev-dependencies would unify into
the whole workspace's test build (finding 16), and a runtime setting needs no second
build.

- In the simulation's own tests (`cfg(test)` sets the default): every chunk the step
  touched (lit whole, adopted, replaced, or reached by a change). Those worlds are small;
  the crate takes about 2.5 ms a chunk in the dev profile (`clustine-light` is built at
  `opt-level = 3` there, `Cargo.toml` 62–63).
- In the end-to-end tests: the worker reads `CLUSTINE_LIGHT_ORACLE` (a number of chunks
  a step, set by the tests' harness, 0 elsewhere) into the setting, and the region
  compares that many lit chunks a step, taken in turn in ascending order from where the
  last step stopped (not every chunk the step touched, which may be dozens). A difference
  left by
  a missed change lasts until something lights that place again, so a turn through the
  region's chunks finds it; the number is chosen so that `tools/check.sh` grows by less
  than a tenth (section 8), 4 to begin with.

Every existing test of the simulation (thousands of ticks of water, pistons, TNT, mobs,
merges and splits) thereby tests the engine, the adoptions and the noting of 4.2. Since
the noting is in `Chunk::set` (4.2), no write can bypass it; the oracle is there for the
engine and the region's handing of chunks to it.

## 5. What crosses a line

**Nothing about light.** Each region lights its present chunks from its blocks and its
copies' (decision 4). A chunk it ticks has its 3×3 present (A24 section 2, A25 section 3)
and lit (4.5), so its light is exact: the oracle's for its 3×3, which is the light a single region
would have with the same blocks. Two coupled regions see each other's changes in their
copies at the start of the next tick and light them at its step: the game's timing. An
uncoupled copy is one to a few ticks late (A25 section 1), and light at the line with it.

**Where the 3×3 is not there** (a copy, a ring chunk), option a. The bound, from 3.1:
every chunk in the 3×3 of a chunk the region ticks is present and lit, so an absent or
waiting chunk begins
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

The random ticks of section 1 go into `region/random_ticks/` beside `fire.rs`,
`minerals.rs` and `plants.rs`, as a fourth and a fifth file: `growth.rs` (crops, stems,
berries, pitcher, bamboo, mushrooms, grass and mycelium, saplings and the propagule's
ground half from N8f) and `melting.rs` (snow layers; ice and frosted ice, by `IceBlock`'s
random tick, which frosted ice inherits). `random_tick`'s dispatch (`random_ticks.rs`
257–266) tries them after plants. The lead stubs both files, their `mod` lines and the
dispatch in N8a, and moves the ground propagule out of `plants.rs` (103–115, where it
draws `nextInt(7)` and does nothing) into `growth.rs`, where it keeps that draw until
N8f, so that `plants` no longer claims the block before `growth` sees it. Each rule is
the game's `randomTick` with the same draws in the same order
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
| melon, pumpkin stems | `raw(pos, 0) ≥ 9`; `nextInt((int)(25/f)+1) == 0`; below age 7 age + 1; else a horizontal direction drawn, the fruit where air and the block below is in the stem's `fruitSupportBlocks` (n8 StemBlock 71–256) | age flags 2; fruit `setBlockAndUpdate`, then the attached stem `setBlockAndUpdate` | the fruit one across may be a `SetIf`; the stem turns at once, and if the holder refuses, stays turned until the next shape update from that side, which turns it back (n8 AttachedStemBlock, section 6.3): named |
| sweet berry bush | age below 3; `nextInt(5) == 0`; `raw(above, 0) ≥ 9` | age + 1 flags 2, game event `block_change` | — |
| snow layer | `block(pos) > 11` | drops (none without a tool), `removeBlock`, game event `block_destroy` | — |
| ice, frosted ice (`IceBlock.randomTick`) | `block(pos) > 11 − dampening` | `melt` (section 2, row 9) | — |
| bamboo | stage 0; `nextInt(3) == 0`; air above; `raw(above, 0) ≥ 9`; height below + 1 < 16 | `growBamboo` (n8 454–572): its leaves; `nextFloat() < 0.25` drawn **only where the new height is 11 or more**, stage 1 where it passes or the height is 15 | — (one column) |
| bamboo shoot | `nextInt(3) == 0`; air above; `raw(above, 0) ≥ 9` | bamboo with small leaves above, `setBlockAndUpdate`; the shoot's shape update then makes it bamboo (section 2, row 19) | — |
| mushrooms | `nextInt(25) == 0`; at most four of the kind around; the walk of section 2 row 12: at each step `isEmptyBlock` and the full `canSurvive` (the override tag below, or light below 13 *and* solid render below), the next target drawn whether or not the step was taken | the mushroom's own state at the target, flags 2 | the target may be a copy's: `SetIf` (expected air) |
| grass block, mycelium (n8 SpreadingSnowyBlock 15–167) | `canStayAlive`: one snow layer above lives; else a full fluid above dies; else `getLightDampeningInto(state, above, UP, above's dampening) ≥ 15` dies. Dying: dirt by `setBlockAndUpdate` (flags 3), no draw. Else if `max_local(above) ≥ 9`: four times a target at x `nextInt(3)−1`, y `nextInt(5)−3`, z `nextInt(3)−1` (drawn in that order); the target must be the dirt block, pass `canStayAlive` and have no `#water` fluid above. No light is read at the target | the default state with `snowy` = the block above is in `#snow`, flags 3 | the target may be a copy's: `SetIf` (expected dirt) |
| saplings | section 7 | | section 7.4 |
| mangrove propagule on the ground | not hanging; `nextInt(7) == 0`; `advanceTree`: **no light read** (section 2, row 15) | section 7 | section 7.4 |

### 6.3 Survival

`can_survive` asks light only where the game does: `CropBlock` (and its kin by class:
beetroots, carrots, potatoes, torchflower crop) `raw(pos, 0) ≥ 8` and on
`supports_crops`; the pitcher's lower half the same; the mushroom as now
(`survive.rs` 186–193), now judged in a region. It is asked at placing and at each
shape update, never at a change of light (section 2, row 3).

Two shape updates that are not light but that a player meets within minutes of growing
plants are ported with them in D2 (findings 7 and 8; section 2, rows 18 and 19):

- **The attached stem** (`AttachedStemBlock.updateShape`): from the side it faces,
  where the neighbour is no longer its fruit, it becomes its stem's default state at age
  7; otherwise it is judged as vegetation, on its `supportBlocks` tag (farmland), which
  the simulation's survival gains (`survive.rs` 130–132 has it only for placing, as not
  ported). Without it, the first fruit harvested would leave a stem attached to air for
  ever. The dump is the lead's, done after the review (n8 AttachedStemBlock).
- **The bamboo shoot** (`BambooSaplingBlock.updateShape`): air where it cannot stay; from
  above, bamboo there makes it bamboo's default state. `shape.rs` 283–297 handles only
  the stalk now.

Their arms in `clustine-blocks/src/shape.rs` are stubbed by the lead in N8a, calling
functions in a file of D2's own, `clustine-blocks/src/plants.rs`.

### 6.4 The daylight detector (D3)

A block entity ticker in `run_block_entities` (ADR-0026 section 3): where the dimension
has sky light, when `clock.game_time % 20 == 0`, `updateSignalStrength` (section 2,
row 14). The sun's angle is `visual/sun_angle`, added to `time.rs` beside
`sky_light_level` and sampled, as that is, at the clock's `day` (which jumps at a sleep
and at `time set`). `timeline/day.json` gives two keyframes at tick 6,000, 360° and
then 0°, with a period of 24,000, so the sampler's segments (n7d-brief 706–712) make the
angle `ease(α) · 360` with `α = floorMod(day − 6000, 24000) as f32 / 24000.0`: 0° at
noon, rising to 360° by the next noon; the lerp is `Mth.lerp` of floats with no wrap
(n7 AttributeTypes 112–122). The ease is the cubic Bézier (0.362, 0.241, 0.638, 0.759)
of n8 EasingType$CubicBezier 64–160, ported bit for bit in `f32`: four Newton–Raphson
steps with a tolerance of 1e-5 and each step clamped to ±0.25, then bisection. It has
to be exact, since `round(i · cos f)` turns on a half. `Mth.cos` is the table of
`clustine-noise/src/trig.rs` 18–35, on which `clustine-sim` already depends. Set by
`setBlockAndUpdate`, so phase R's updates carry it. Its use (a click) is refused where
the player may not build (`mayBuild`); otherwise it inverts with flags 2, makes the game
event `block_change` and updates at once (n8 DaylightDetectorBlock 121–161). ADR-0027
already lends it as a circuit's part (`region/tests/lends_by_the_record.rs` 221–241).
The `Typed` variant, its arm in `Typed::of` and the arm in `run_block_entities`
(`region/block_entities.rs` 40, 82, 588) are stubbed by the lead in N8a, calling D3's
own file `region/daylight_detector.rs`.

### 6.5 Mobs

Nothing in the mobs' code changes but `MobWorld::light` (4.3): every reader of L8 1.5
(`burning.rs`, `monster_ai.rs`, `animal.rs`, `guardians.rs`, `random_pos.rs`,
`skeletons.rs`, `navigation.rs` 511–512, `villagers.rs` 33–39, `fox.rs`) goes through it.
Where `MobWorld::light` answers `None` the readers differ today (`can_see_sky` true,
burning's sky test false, `burning.rs` 80–83; `light_cost` an undarkened 15,
`animal.rs` 423–426). They are left so, because almost no read of a mob that ticks can
land there: a mob ticks only in a ticking chunk (A28 section 2), whose 3×3 is present
and lit (4.5), and every read of L8 1.5 but one is within 10 blocks of the mob, inside
that 3×3. The one is a path's trim from the sun (`navigation.rs` 501, 511–512), whose
nodes can lie farther, in a chunk that is present and still waiting: there `None` reads
as seeing the sky and cuts the path at that node, which keeps a mob that flees the sun
out of a place whose light is not known yet, for the few ticks until it is lit.
Harmless, and named. Each reader's comment says which applies (finding 21).

## 7. Saplings and trees (N8f)

### 7.1 `TreeGrower`

Ported whole from n8 TreeGrower: the twelve growers with their three weighted lists and
shortest tree (514–743), mangrove's plain list 15 mangrove to 85 tall mangrove, each
list drawn by `WeightedList.getRandom`, which draws `nextInt(totalWeight)` whenever the
list is not empty, also for a list of one (n8 WeightedList 149–166); `growTree` (101–219) with the mega list drawn first, the 2×2
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
  the feature uses (19 for a tree's logs, leaves and decorations: n8 TreeFeature 23,
  79–85, section 2 row 20), with their updates, in the order made, as the game's
  `ServerLevel` does.
- **Writes into a copy** are kept in the view, read back by the feature, and become
  `SetIf` effects in the order made, `expected` being what the view read there first,
  when the feature returns.
- `ensure_can_write` is true (a `ServerLevel` writes anywhere, section 2). A position
  whose block the region cannot read (neither loaded nor copied: a chunk it holds and has
  not loaded among them) refuses the write, as the region's set drops one there
  (`updates.rs` 144–161: no block read, no change; and for a chunk it holds, `sends_to`,
  `region.rs` 2930–2946, makes no effect), and the feature reads such a
  place as the game's region of generation reads its edge. A tree is at most a few
  blocks from its sapling, whose 3×3 is present, so in ordinary play this never happens
  (finding 26).
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
meanwhile) leaves a gap the game would not; named. Among the `SetIf`s are the existing
leaves inside the tree's box whose distance the tree rewrites (`updateLeaves`, n8
TreeFeature), which may lie in a copy: one refused keeps its old distance until its next
update; named. **A bee nest's bees**: `SetIf` carries
a state and no block entity (`clustine-world/src/effects.rs` 50–54), so a nest placed
across a line is an empty nest: named (question 4). Decorators that place entities (none
of the growers' trees place a mob) need nothing more.

### 7.5 Bone meal (question 1)

Agreed at the review: the player's use of bone meal (`BoneMealItem`, n7) on every
`BonemealableBlock` N8 builds (crops, stems, pitcher, berries, bamboo and shoot, saplings
and propagules, azalea, grass, mushrooms through `growMushroom` and the huge mushroom
features) with each block's own `isValidBonemealTarget`, `isBonemealSuccess` and
`performBonemeal` (n8 for those dumped now, n7 for the crops), the particles' level
event, and the dispenser's `Behaviour::BoneMeal` (`dispensers/behaviours.rs` 91), which
waits for this. A sapling's `performBonemeal` is `advanceTree` (n8 SaplingBlock
124–146): a success at 0.45 moves a sapling of stage 0 to stage 1, and the tree comes
at the next success, not at once. Grass's bone meal makes 128 walks of up to 7 steps
(n8 GrassBlock 41–105) and places `grass_bonemeal` and the biome's bone meal features
through the view of 7.2, up to about eight blocks out, into copies too. `growMushroom`
removes the mushroom with flags 3 and, where the feature fails, puts it back by
`setBlockAndUpdate`.

### 7.6 What it needs

`clustine-sim` depends on `clustine-features` (no cycle: the features crate does not
depend on the simulation; worldgen reaches the simulation through N7l's maker trait).
The overworld's `FeatureSettings` is built once per process from the world's seed and
handed to regions in `RegionConfig` as an `Arc`, as the worker builds what regions share.
`RegionConfig` derives `Debug`, `Clone` and `PartialEq` and is built at some 50 sites,
the tests' among them, so the field is added in N8a by the lead, before anything is
delegated: a type of `clustine-sim`'s own (`Features`, `Default`, empty until N8f, with a
`PartialEq` that holds always and a short `Debug`), beside the light settings of 4.7.
It adds the worldgen crates to the worker's build; measured as compile time in the
step's commit message.

## 8. Cost: what is measured, and when it is acted on

Each figure says whether it is in memory (`TMPDIR=/dev/shm`) or on the real disk with its
cache, in a release build, on a quiet machine. The region never reads a clock
(`clustine-sim`'s `clippy.toml` forbids `Instant::now`): it counts each step's `Work`
(chunks lit whole, adopted, cells visited, the queues' peak) and returns it in its
`TickOutput`. Time is taken outside it: by the engine's benches, by the worker around
each whole tick (logged beside the step's `Work`), and, for the step alone, by a bench
that replays the calls a region made to `Lighting` (recorded in the measuring build of
a test) against the engine.

| Case | Measured | Acted on if (**guesses**, for the reviewer) |
|---|---|---|
| The engine alone (a bench in `clustine-light`) | a torch placed and broken; a glowstone ceiling of 64; a lamp toggled; 64 TNT's crater; a view's 400 chunks lit one at a time as the budget takes them; 400 adopted; each against the E1 oracle's time | a chunk lit whole dearer than 0.5 ms (so that a step's 32 stay within 16 ms); an adoption dearer than 0.06 ms; a torch dearer than 0.2 ms |
| A builder (a bot placing and breaking torches, lanterns and glass twice a second) | the step's time a tick, worst and 99th centile | worst over 5 ms, or 99th over 1 ms |
| A river crossing a view, and a pinned line (E7's case) | the same, and the coupled pair's ticks a second | worst over 5 ms; ticks a second below E7's |
| 64 TNT (N7f's crater) | the step in the tick after | over 10 ms |
| A lamp clock of 16 lamps on a repeater clock (R8's), and redstone ore lit by walking mobs | the step's mean and worst | mean over 1 ms |
| A first join at view distance 8, seeded and flat | the step at each tick of the join, chunks carried, adopted and lit, the ticks until the view is lit whole | any tick's step over 20 ms; the view lit whole later than 2 s after its last chunk arrives |
| A restore, a move, a take-over (the store or worker killed) | the pause as now (B8, E7), the first step's share, the ticks until every chunk is lit | the pause longer by more than 20 ms (half of B8's least, 0.04 s); every chunk lit later than 2 s |
| A merge and a split with the kept levels | the same, and the chunks relit whole (only those the store delivered afresh should be) | the same; any chunk handed back warm relit whole |
| Memory | the `Lighting`'s bytes a present chunk and a region's total | over three times the 9.3 KB mean |
| `tools/check.sh` | its length beside the last full check (4.7's oracle runs in the tests) | longer by more than a tenth (then fewer chunks a step in the end-to-end tests' oracle) |

The bounds agree with each other now (finding 2): a step lights at most 32 chunks whole
at 0.5 ms (16 ms) plus the tick's changes, under the 20 ms of a join's tick and the
20 ms a restore may add; a view's 400 to 650 chunks take 13 to 21 steps. "Acted on"
means mended within N8 before it is called done: the engine made cheaper (the queues'
entries smaller, the window of 3.4 wider), the budget's figure lowered, or, if that is
not enough, a question to the owner with the figures.

## 9. Comparisons with the official server

All `#[ignore]`, named `official_server`, run by the lead alone on a quiet machine, the
jar from `cargo datagen`, `--no-fail-fast`.

1. **Light after changes** (`crates/clustine-light/tests/official.rs`'s way extended): a
   script of block changes made by commands on the official server (torches, glowstone,
   glass, a roof, a hole dug to the caves, water poured, a floating platform, blocks
   placed across a chunk's corner), saved and stopped; its stored light read section by
   section and compared with the engine's after the same changes on the same stored
   chunks, sections listed by both. The official server lights on its own thread, and
   the flat comparison is already flaky for that race (CLAUDE.md), so the script waits
   some ticks after the last change for the light to settle before it saves. A
   difference is a failure unless it is the crate's named case under something floating,
   which the test lists apart.
2. **The daylight detector** (`bin/clustine/tests/detector_oracle.rs`, R8's way): a
   detector and an inverted one in the open on both servers, `time set` through a day in
   steps; the power each sends (block updates seen by a bot) compared exactly at every
   20th tick. This also checks the sun's angle and its ease.
3. **Rates** (statistical, both servers with `tick sprint`): 256 wheat on hydrated
   farmland under a torch at midnight, under the open sky at midnight, in the dark;
   saplings of oak under the sky at midnight (none grow), by day, and under a torch at
   night; grass onto a field of dirt by day and by night; snow layers two steps from a
   torch and three from glowstone (both melt) and one step farther from each (neither
   melts), and ice beside them (melted or not within so many random ticks); saplings
   given bone meal (the stage first, the tree at the second success). Ages and counts
   after N game ticks compared with the game's by a binomial bound at 1 in 1,000; the
   thresholds' sides (light 8 and 9, 11 and 12) placed exactly so.
4. **What a client is shown** with mobs (N7k's sixth comparison extended): a zombie at noon
   under glass, burning, and under a tree's leaves, not burning.

## 10. Tests from the specification

Delegated to someone who wrote none of the code, from L8, the dumps' rows of section 2
and this plan, in two parts:

- **A. The engine** (`crates/clustine-light/tests/engine.rs`, by someone who did not
  write D1): property tests over small worlds (3×3 to 5×5 chunks of four to eight
  sections, so that the oracle is fast in a debug build): random sequences of block
  changes drawn from states light tells apart (emitters, dampeners, glass, slabs and
  stairs with faces, air, water), chunk arrivals and departures, chunks replaced whole
  (`arrived` on a present chunk), chunks handed out with `kept` and adopted later in
  another `Lighting` in any order and with any of their neighbours present then and now,
  and runs at random points; after each run every present chunk equals the oracle, by
  `Lighting::light` and by `Lighting::at` at every block. Cases by hand: a torch placed
  and broken at a chunk's corner; light through a gap in a wall of slabs; the sky down a
  shaft and under an overhang; a section's first and last block; a chunk arriving beside
  a lit cave; a chunk leaving that carried the light of its neighbour's cave; a
  neighbour's sky column and an emitter on the face beside a chunk that leaves, kept; a
  chunk replaced whole whose old blocks held a torch by the face, its light gone from the
  neighbour after the run (finding 3); a chunk adopted before, with and after its
  neighbours from the same hand-over; carried
  light adopted and refused by each clause of 3.5; two histories to the same blocks
  giving the same light.
- **B. The region and the rules** (crate tests of the simulation and end-to-end): each
  rule's draws in order against a counting `Draws` (the table of 6.2 is the oracle); the
  thresholds at their edges (8 and 9 for crops and the pitcher, 9 with and without the
  night's darkening for saplings and grass, 11 and 12 for snow (two steps from a torch
  and three from glowstone melting, one more not), ice's by its dampening, 12 and 13 for
  mushrooms); seeds refused in the dark and accepted at 8; a dark crop breaking at a
  neighbour's change and not at a change of light; a mushroom's walk, its four draws of
  a target taken whether or not a step is; a grass block dying under a bottom slab and
  not under a top slab or a layer of snow, its spread's draws in the order x, y, z;
  bamboo stopping at 16, its `nextFloat` drawn only from height 11; a shoot becoming
  bamboo under grown bamboo; a melon harvested and its stem turning back and fruiting
  again; frosted ice melting by a torch; a sapling given bone meal taking its stage and
  then its tree; the detector's power at chosen times of day (noon, either side of
  dawn and dusk, and the ticks where `round(i · cos f)` turns), inverted, clicked, and a
  click refused to a player who may not build; a zombie burning under glass and not under
  a roof or a tree's leaves; the budget: a region given more chunks than a step takes
  lighting them nearest first, a chunk ticking only once its 3×3 is lit, a replacement
  beside a ticking chunk not stopping it, and a region made by `restore_loaded` with its
  chunks within the budget ticking as the original; at a pinned line (`--pin` regions in the end-to-end
  tests) the light of the chunks each side ticks equal to the light of one region with
  the same blocks, a mushroom and grass spreading into the other side, a torch on one
  side lighting a crop on the other the tick after; a restore in the middle of a growth
  (the light after the restore equal to before, the crop growing on); a merge and a
  split with the kept levels (equal to a fresh lighting, and no chunk handed back warm
  relit whole), with a change made in the very tick of the hand-over lit; the edge's light for a chunk
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
| N8a | The contract: `clustine-light`'s `engine.rs` interface with E1's bodies (3.2, 3.9); the noting in `Chunk::set` (4.2); the region's `Lighting` field, the queue of waiting chunks and the budget, `update_ticking`'s test of a lit 3×3, `restore_loaded`'s amended promise (4.5), the light step (4.1), the reads (4.3) with `MobWorld::light` and the views moved onto them, 4.7's check and its setting; the shared files stubbed (below); the dumps of section 2's last paragraph | the lead |
| N8b | The incremental engine behind the interface (3.3–3.5), its benches | D1, delegated |
| N8c | The rules of 6.2 but saplings, survival (6.3), the attached stem's and the bamboo shoot's shape updates (6.3), mushrooms' and grass's `SetIf`s | D2, delegated |
| N8d | The daylight detector (6.4) and the sun's angle | D3, delegated |
| N8e | Merges and splits with the kept levels (4.5: the last step at the hand-over, `Kept` in the runner's `Warm` and in `chunks_loaded`, adoption), carried light (3.5) wired in the region, the budget's figure set from the measurements of section 8 but N8f's, the comparisons 1, 2 and 4 | the lead |
| N8f | Saplings and trees (section 7): the lead's view for features (7.2) and the dependency (7.6) first, then `TreeGrower`, the saplings and the propagule, and bone meal, delegated (D4); comparison 3 | the lead, then D4 |
| N8g | Tests from the specification (section 10), part A after N8b, part B after N8c to N8f | D5, delegated, someone who wrote none of it |

**The shared files, stubbed by the lead in N8a** (finding 18), so that no two agents
touch one file and every agent's crate compiles with the others' stubs:

- `region/random_ticks.rs`: the `mod growth; mod melting;` lines, the two files with
  empty rule functions, and the dispatch trying them after plants (6.1);
- `region/random_ticks/plants.rs` 103–115: the ground propagule moved into
  `growth.rs`, keeping its draw of `nextInt(7)` (6.1);
- `clustine-blocks/src/shape.rs`: the arms of `AttachedStemBlock` and
  `BambooSaplingBlock`, calling empty functions in `clustine-blocks/src/plants.rs` (6.3);
- `region/block_entities.rs`: the detector's `Typed` variant, its arm in `Typed::of`
  and in `run_block_entities` (40, 82, 588), calling empty functions in
  `region/daylight_detector.rs` (6.4), and `time.rs`'s `sun_angle` with a body of 0;
- the placement `Blocks` trait (`placement.rs` 31): its light read, with every
  implementation (4.3);
- `RegionConfig`: the light settings (4.7) and the `Features` field (7.6), at every
  site that builds one.

Order: N8a first and pushed before anything is delegated (a worktree starts from
`origin/main`). Then D1, D2 and D3 at once, each in a worktree of its own with crates or
files of its own: D1 `crates/clustine-light/src/engine*.rs` and its benches only; D2
`region/random_ticks/{growth.rs, melting.rs}`, `clustine-blocks/src/survive.rs` and
`clustine-blocks/src/plants.rs`; D3 `region/daylight_detector.rs` and the body of
`time.rs`'s `sun_angle`. N8e after D1 is merged. N8f after N8c (the propagule and the
grass share `growth.rs`). D5's part A may start against N8a's E1 at once, since it tests
the interface.

Each brief states the interface, the tests expected and how to verify (`cargo test -p
clustine-light` or `-p clustine-sim`, fmt and clippy in its worktree), says that the agent
does not commit to `main`, push, download, or start Docker, kind or the official server,
and that it removes its worktree's `target` when done. The lead reads and tests what
comes back before merging, and runs the full check before each push.

**Not delegated** (the lead's decision at the review): the noting in `Chunk::set` (4.2,
finding 5), the step's place in the tick, the budget and `update_ticking`'s test (N8a),
the hand-over of kept levels at merges and splits and their adoption (N8e, finding 1),
and the view for features with its effects (N8f's first half): the parts where ordering
mistakes hide.

## 12. Records and the roadmap

- **A record of its own, ADR-0029 "Light in the simulation"**, written from this plan
  once agreed and reviewed before N8a: the engine and its invariant, the step, derived
  light, option a with its bound, the budget of arrivals and the rule that a chunk ticks
  only once its 3×3 is lit, the kept levels at merges and splits, the noting in
  `Chunk::set`, trees as effects, and the figures of section 8. It amends A24 section 8
  (light is now in the simulation), A24 section 4 (`restore_loaded` goes on as the
  region it was taken from only where its chunks fit in one step's budget), A23 (regions
  light their chunks too; the edge is unchanged and nothing passes between them), A25
  section 4 (`SetIf` for growth and trees).
- **What to try**, written into the roadmap's first section when N8 is done:
  `cargo run --release -p clustine -- --seed 3 --pin 4 --reshape by-hand`. Plant wheat on
  watered farmland beside a torch in a dark room and in the open; at night the room's
  grows only near the torch and the open one grows on; seeds cannot be planted in the
  dark. Grow melons and harvest one: the stem turns back and fruits again. Put a torch
  beside snow layers: those within two blocks melt, those three away stay; glowstone
  melts them within three. Ice beside a torch melts. Clear grass from a field: it comes
  back from the edges by day; a bottom slab on grass kills it, a top slab does not.
  Plant saplings of every wood (four in a square for spruce, jungle, dark oak and pale
  oak): they grow into the game's trees by day, not at night without a torch; a mangrove
  propagule on the ground grows in the dark too; one planted at x = 62 grows across the
  line. Bone meal on a sapling: the first success shows nothing, the second makes the
  tree. Bamboo grows from a shoot; a mushroom farm fills a dim room. A daylight detector
  beside a lamp lights it by day, the inverted one at night. A zombie under glass at
  noon burns, one under a tree does not; spawn monsters beside your torches and watch
  them walk off to the dark. Kill the worker (or restart the server) while flying: the
  chunks around you go on at once, those at the edge of the view stand still for about
  a second, then go on.

## 13. Open questions, each with my recommendation

Answered at the review (section 14, "The open questions"). The recommendations stand as
put, but for 3, 5, 8 and 9, which are corrected here.

1. **Bone meal.** Not in the lead's list, but a creative player with a sapling reaches
   for bone meal within minutes, the dispenser waits for it, and it shares N8f's whole
   machinery. *Recommended*: in N8f, for every bonemealable block N8 builds (7.5).
2. **The light out of the region's equality** (4.6), judged by the oracle instead of by
   comparing regions. *Recommended*; the alternative, lighting at `restore` so that a
   restored region equals its original between ticks, would still differ until the
   copies come back.
3. **Carried light** (3.5) adopted under the stricter test. *Decided* (the lead): built,
   adopted at the budget's lower cost, and measured; the kept levels of merges and splits
   are the larger saving (4.5).
4. **A bee nest across a line comes without its bees** (7.4), rather than giving `SetIf` a
   block entity (a change of the effects' encoding). *Recommended*: named; it needs a
   bee tree, flowers near, and a line within a few blocks.
5. **A stem turned to a fruit the holder refused** (6.2) stays turned until a shape
   update from that side, which turns it back now that `AttachedStemBlock.updateShape`
   is ported (6.3). *Recommended*: named; making the two atomic across a line would
   need the holder to answer.
6. **Crops in dark rooms of worlds played before N8** break as their neighbours change.
   *Recommended*: as the game; said in what to try.
7. **The rim** (section 5, option a). *Recommended* as written: no threshold differs.
8. **The bounds of section 8** contradicted each other (finding 2). *Decided* (the lead,
   which the owner may overrule): the budget of 4.5, with which they agree; the
   measurement writes the figures into ADR-0029 whatever they are.
9. **The dumps still owed**: all dumped but `Feature` and `WorldGenLevel` (section 2's
   last paragraph), which the lead dumps in N8a before D4 starts; this plan is
   corrected where the others differed (rows 8 and 18 to 22).
10. **Saplings last, as N8f.** *Recommended*: nothing else waits for them, they carry the
    one new dependency and the one new kind of cross-line work, and a player meets
    crops, grass and melting first.
11. **The light step before post-processing** (decision 1's refinement): a chunk's first
    ticking tick reads the light of its unsettled blocks. *Recommended*: named; moving the
    step after post-processing would put the tick's own block ticks before light.
12. **A record (ADR-0029) before building**, reviewed. *Recommended*: the region gains
    state of a new kind (derived, outside its equality) and a new kind of work across a
    line, both of which the earlier records' readers will look for there.

## 14. The review (2026-10-26), and what the plan is now

An independent review found eleven defects, nine risks and eight nits, and answered the
open questions. Each finding was checked against the code and the dumps before it was
taken. The sections above were corrected in place where a finding changed a number or a
rule; where this section and they still differ, this one holds. After the review the lead
dumped `AttachedStemBlock` and `StemBlock` into `n8-javap/` (finding 7), and decided the
questions the review left open (below, "The lead's decisions").

**The lead's decisions**

- **A deterministic budget for arrivals** (finding 2; section 4.5). The draft's bounds
  contradicted each other: an arrival allowed 0.5 ms a chunk, so a view's 400 to 650
  chunks could take 0.2 to 0.3 s in one step, against 25 ms a join's tick and half of
  B8's 0.04 s at a restore. Now a step takes waiting chunks nearest the region's players
  first, then by position, within 256 units (8 for a chunk lit whole, 1 for an adoption),
  counted in chunks and never by time or by cells, so that the count is a function of
  the region's state and the same for E1 and D1; departures, changes and replacements
  are not budgeted. A chunk ticks only once its 3×3 is lit (`update_ticking`). A carried
  light that still holds is adopted without relighting, by 3.5's test, which holds (the
  review offered no simpler one that does: without clause 3 the levels of absent
  sections under an overhang are unknown, and without clause 2 the light is the light
  beside a neighbour the region lacks); clause 2's "present" now reads "lit before, or
  taken in the same step". At a merge or a split each chunk's levels go with it
  (finding 1, below). **`restore_loaded`'s promise is amended** (`region.rs` 657–667): it
  goes on as the region it was taken from where its chunks fit in one step's budget,
  which every test that relies on it does (the lead counts them in N8a); otherwise its
  chunks wait for their light as arrivals do. What a player sees after a take-over: the
  chunks around them go on from the first tick and those at the edge of the view stand
  still for about a second, rather than the whole region stalling while everything is
  lit. **This is the lead's decision and can be overruled by the owner**; ADR-0029
  records it, and amends A24 section 4 for `restore_loaded`.
- **The attached stem and the bamboo shoot** (findings 7 and 8) are in N8, in D2
  (section 6.3; section 2, rows 18 and 19), with `AttachedStemBlock` dumped by the lead
  (done after the review, as part of N8a's dumps).
- **Finding 1's hand-over and finding 5's noting in `Chunk::set`** are the lead's, not
  delegated (section 11).
- **Finding 18's shared files** are stubbed by the lead in N8a: `random_ticks.rs` (the
  two modules and the dispatch), `plants.rs` 103–115 (the ground propagule moved to
  `growth.rs`), `clustine-blocks/src/shape.rs` (the arms of the attached stem and the
  shoot, calling D2's `clustine-blocks/src/plants.rs`), `region/block_entities.rs` (the
  detector's `Typed` variant, `Typed::of` and the arm in `run_block_entities`, calling
  D3's `region/daylight_detector.rs`) and `time.rs`'s `sun_angle`, the placement `Blocks`
  trait's light read, and `RegionConfig`'s light settings and `Features` field at every
  site that builds one (section 11).

**Made to fit those decisions, for the lead to confirm**

- **The light step moved** from after `take_arrivals` to before `update_ticking`, after
  the players' actions and the sleep check (decision 1, section 4.1). With the step after
  `update_ticking`, a chunk lit in a tick could tick only in the next, so every load
  would start a tick later than now and a region made by `restore_loaded` could never
  tick in its first tick, whatever the budget. What `run_seams` and `lay_blast_drops`
  set (other regions' effects, a tick late already) is now lit at the next step, as
  block ticks' changes are. The step's place is the lead's (section 11), so this is put
  to the lead rather than settled here.
- **Replacements are not budgeted** (section 4.2): a copy's new base, a loaded chunk
  become a copy or the reverse, are lit in the step they happen in, so that a ticking
  chunk beside one does not stop for a tick.
- **The waiting queue is the region's, not the engine's** (3.2): the engine lights what
  it is told of, so its interface and its property tests need no budget.

**The engine**

- **A replacement is a departure and then an arrival** (finding 3): `arrived` on a
  present chunk means its blocks were replaced whole, and the run takes it out before it
  lights it again (3.2, 3.4). Part A of the tests replaces a chunk whose old blocks held
  a torch by its face and checks the neighbour's light is gone.
- **A departure is the game's own decrease** (finding 4): the departed chunk's face cells
  become walls and are decreased through the ported `checkNode`, which keeps the
  neighbours' own sky columns and emitters at the face; the draft's decrease of the
  neighbours' face cells would have zeroed those with nothing to add them back (3.4).
- **The engine's own cost** (finding 15): arrivals and adoptions are lit one chunk at a
  time in the order given, each to its fixed point (the values are the same), so the
  queues hold about one chunk's flood (3.8, a peak measured); a step from cell to cell
  works in a window of the chunk and its neighbours resolved once, not a `BTreeMap`
  lookup a step; uniform sections are filled at once, a necessity (3.4).
- **`Debug` and equality** (finding 23): `Lighting` and `Kept` have a short `Debug`;
  `Lighting` and the waiting queue are out of the region's equality; `RegionConfig`'s
  `Features` has a `PartialEq` that holds always and a short `Debug` (4.6, 7.6).

**The region**

- **The hand-over at merges and splits** (finding 1, confirmed: `reshape.rs` 251–260 and
  417–430 replace the region whole; the worker keeps the chunks in `warm`, `lib.rs`
  2275–2281, and hands them back a tick after a ticket asks, 2602–2606). Before giving up
  its chunks, the region runs the step once more, so the last tick's noted changes are
  lit, and hands out each lit chunk's `Kept` (its levels, the neighbours that were walls,
  and which hand-over it is from). The runner keeps it in `Warm` and hands it back with
  the chunk in `chunks_loaded`; the region adopts it at one unit of the budget, and the
  engine mends only its faces: towards a neighbour adopted from the same hand-over in the
  same run they stand; towards any other they are a departure and then, where the
  neighbour is present, an arrival. That last rule is the review's "a neighbour arriving
  later pulling light in", made exact: a chunk adopted a step before its neighbour had
  its face towards it decreased while the neighbour was absent, so the neighbour's
  adoption must push its light back. A chunk the store delivers afresh is lit as any
  arrival (3.2, 4.5).
- **The noting is in `Chunk::set`** (finding 5, confirmed: `set` is the chunk's only
  writer of blocks, `chunk.rs` 343–368, and `Chunk::apply` calls it, `change.rs` 52–58).
  A list of positions light can tell apart, switched on by the region and off elsewhere,
  not serialised (`ChunkRepr`) and no part of the chunk's hand-written equality
  (`chunk.rs` 266–278), which the step drains. `block_written` and the audit of sites go
  (finding 28 with them: the count of sites no longer matters).
- **Nothing in the region reads a clock** (finding 14): it counts `Work` and returns it in
  `TickOutput`; benches and the worker time it (section 8).
- **The oracle in tests** (finding 16): a runtime setting in `RegionConfig` instead of a
  cargo feature, which would have unified into the workspace's whole test build: every
  touched chunk in the simulation's own tests; in the end-to-end tests a few lit chunks a
  step in turn (`CLUSTINE_LIGHT_ORACLE`, 4 to begin with), which finds a lasting
  difference within a turn of the region's chunks, kept so that `tools/check.sh` grows
  by less than a tenth (4.7).
- **Reads before the step** (finding 17): `None` (not judged: seeds go in) where the chunk
  is not lit, a chunk loaded in this very tick or still waiting among them; sky 15 and
  blocks 0 above and below the kept sections (3.2, 4.3).
- **`None` for mobs** (finding 21): taken as "say so", with a correction to the review's
  premise. The readers stay as they are; no read within 10 blocks of a ticking mob can
  land outside its lit 3×3, but a path's nodes can lie farther, in a chunk still
  waiting, where `None` reads as seeing the sky and cuts the path there. Harmless for
  the few ticks until it is lit, and named (6.5).
- **A feature's write where the region cannot read** (finding 26): refused, as the
  region's set drops it. The review read the set as making "an effect to itself" for a
  chunk held and not loaded; `sends_to` (`region.rs` 2930–2946) makes none for a held
  chunk, and with no block read the set changes nothing (`updates.rs` 144–161). Section
  7.2 says so; only writes into a copy become `SetIf`s.

**The rules**

- **Leaves** (finding 6): they pass the sky's light at one less, so the sky under them is
  14 (`clustine-light/tests/cases.rs` 503) and a zombie under a tree does not burn, as in
  the game. Sections 1.8, 9 (comparison 4) and 10 corrected.
- **Snow** (finding 10): a torch melts snow within two blocks, glowstone and a lantern
  within three (1.2, comparison 3, section 10, what to try).
- **Bone meal on a sapling** (finding 9): by stage, the tree at the second success (1.6,
  7.5, comparison 3, section 10).
- **Grass as dumped** (finding 11): n8 SpreadingSnowyBlock replaces SteelMC's row, in
  section 2 row 8, 1.3 and 6.2: survival by `getLightDampeningInto(…) ≥ 15` (a bottom
  slab kills, a top slab does not), dying by `setBlockAndUpdate` with no draw, the
  spread's four tries drawn x, y, z, the target the dirt block passing the same survival
  with no `#water` above, `snowy` from `#snow`, flags 3, no light read at the target.
- **The other rows** (finding 12), checked in the dumps and taken: the mushroom's walk
  asks `isEmptyBlock` and the full `canSurvive` and draws all four next targets, and sets
  its own state with flags 2 (n8 MushroomBlock 60–137); bamboo draws `nextFloat() < 0.25`
  only from height 11 and takes stage 1 at 15 (n8 BambooStalkBlock 530–560); the
  propagule on the ground reads no light and grows in the dark (1.6, 6.2, what to try);
  mangrove's weights are 15 : 85 (n8 TreeGrower 185–241, the first list being `trees`,
  40–57); grass's bone meal reaches about eight blocks, into copies; `growMushroom`
  removes with `removeBlock` (flags 3) and puts back by `setBlockAndUpdate`. **Frosted
  ice**: its random tick is `IceBlock`'s, which it inherits, and it is built (melting by
  block light, as the game's, for frosted ice placed by a command); its scheduled ageing
  stays unbuilt (1.9, 6.1, 6.2).
- **The detector's angle** (finding 13): stated whole in 6.4: `α = floorMod(day − 6000,
  24000) / 24000` in `f32`, `ease(α) · 360`, the cubic Bézier ported bit for bit (four
  Newton–Raphson steps, tolerance 1e-5, clamp ±0.25, then bisection: n8
  EasingType$CubicBezier 5–160), `Mth.cos` from `clustine-noise`'s table, the gate on
  `game_time % 20`, the click refused without `mayBuild`. Checked against
  `timeline/day.json` (two keyframes at 6,000, 360° then 0°).
- **Dumps and small corrections** (findings 19, 20, 24, 25): section 2's last paragraph
  lists only `Feature` and `WorldGenLevel` as still owed, and rows 18 to 22 record what
  the newer dumps say, `WeightedList` drawing for a list of one among them; a tree's
  `updateLeaves` rewrites in a copy are among its `SetIf`s (7.4, 1.9); `random_ticks/`
  gains a fourth and a fifth file (6.1); flags 19 are read from n8 TreeFeature, not
  inferred (7.2).

**Tests and comparisons** (findings 22 and 27): comparison 1 waits for the official
server's light to settle before it saves; the comparisons and the tests carry the
corrected numbers, and part B gains the budget, the kept levels with a change in the
hand-over's own tick, the stem and the shoot, and the detector's turning points
(sections 9 and 10).

**Refused**: none outright. Two were taken in another form than proposed: finding 16 as
a runtime setting rather than a sampled cargo feature, and finding 21 with its premise
corrected (paths can read a waiting chunk). Finding 26's reading of the code was wrong,
and its request (say which) is met.

**The open questions** (section 13), as answered: 1, bone meal in N8f with finding 9's
correction and grass's reach: agreed. 2, the light outside the region's equality:
agreed, the noting in `Chunk::set` closing the bypass the oracle could not see, and the
hand-over of finding 1 for merges. 3, carried light: built and measured (the lead's
decision; the review would have measured first). 4, a bee nest without its bees: agreed,
named with finding 20's leaves. 5, a stem left turned: mended by the next shape update
now that `AttachedStemBlock.updateShape` is ported. 6, crops in dark rooms of old worlds:
agreed. 7, the rim: agreed, with the leaves corrected and finding 21's reads named. 8,
the bounds: the budget. 9, the dumps owed: folded in, `AttachedStemBlock` added. 10,
saplings last: agreed. 11, the step before post-processing: agreed, and the step is now
also before `update_ticking`. 12, ADR-0029 before building: agreed, recording finding 1's
hand-over, finding 2's budget and `restore_loaded`'s amendment.

**For the lead to decide before N8a**: the step's new place before `update_ticking`
(above); the budget's figures (256 units, 8 and 1), which N8e sets from the measurement;
and whether the owner is asked about the budget now or shown it in ADR-0029.

### 14.x The lead's rulings on what the revision left (2026-10-26)

1. The light step's place before `update_ticking`, after the players' actions and the
   sleep check: agreed. A chunk lit in a tick ticks in it; other regions' effects are lit
   a tick later, which is the game's own "next tick" for light.
2. Replacements lit outside the budget, and the hand-over's rule for faces towards a
   neighbour adopted a step later: agreed.
3. The budget's figures (256 units, 8 a chunk lit whole, 1 adopted) stand until N8e's
   measurements set them.
4. The budget is recorded in ADR-0029 with why, as the lead's decision the owner can
   overrule, and named in the roadmap's "Where the work stands".
