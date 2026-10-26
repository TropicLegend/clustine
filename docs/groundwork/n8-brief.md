# N8 research brief: light in the simulation (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's), one file per
class, in `/home/claude/reference/n5-javap/`, `n6-javap/`, `n7-javap/`, `n7d-javap/` and
`n7k-javap/`; from the datagen's output in `target/datagen/26.3/generated/`
(`dimension_type/overworld.json`, `timeline/day.json`); and from Clustine's tree at
`5f9d025`. Every rule below is the bytecode's or the data's unless marked **Inferred**
(reasoning, or the game's code as remembered, to be checked against a dump) or **SteelMC**
(read in SteelMC's 26.3 branch, `/home/claude/reference/SteelMC-26.3`, which is a port and
not the jar). Line ranges are of the `javap` output of the class named, in the dump named
the first time ("n7 CropBlock 100–140" is
`n7-javap/net.minecraft.world.level.block.CropBlock.txt`); a class is in the first dump
that has it.

Cited as: A22, A23, A24, A25 for ADR-0022 to ADR-0025 (`docs/adr/`); B for
`docs/groundwork/b-plan.md`; P for `docs/groundwork/n7-plan.md`; K for
`docs/groundwork/n7k-brief.md`; D for `docs/groundwork/n7d-brief.md`; NP for
`docs/groundwork/n-plan.md`.

The manner is K's: what the step should know first (section 0), then what the game reads
light for (1), how it keeps light up to date (2), what Clustine has (3), what crosses a line
(4), the questions the plan must answer (5) and what is not dumped (6).

Where it sits: the roadmap's "Next, in the order agreed", item 4 ("light in the
simulation, which melting, crops and grass spreading need", B's question 3); NP section 4
names it N8, after N7 and before N9 (monsters, natural spawning) and N10 (villagers' brain),
"brought forward" by NP question 4 because animals made with the chunk, monsters' spawning
and burning need light. Today the simulation reads stored light or a fallback (P 9.7.2); the
random ticks that need light are left out (B 3.8).

---------------------------------------------------------------------------------------

## 0. What the step should know before planning (findings)

**What reads light**

1. **Few block rules read light, and each reads it at one or two positions next to
   itself.** Dumped: crops (n7 CropBlock 100–140: `getRawBrightness(pos, 0) >= 9` before
   the draw), stems (n7 StemBlock 71–76: the same, `>= 9`, before the draw), sweet
   berries (n7 SweetBerryBushBlock 72–110: `nextInt(5)` first, then
   `getRawBrightness(pos.above(), 0) >= 9`), snow layers (n7 SnowLayerBlock 178–200:
   **block** light `> 11` melts them), the torchflower and beetroot (a `nextInt(3)` before
   the crop's rule: n7 TorchflowerCropBlock 68–81, n7 BeetrootBlock 31–45). Not dumped and
   **Inferred**: saplings and grass and mycelium spreading read
   `getMaxLocalRawBrightness(pos.above()) >= 9`, which **is darkened by the time of day**
   (so they grow and spread by day or by a torch, not under a bare night sky); bamboo
   `getRawBrightness(pos.above(), 0) >= 9`; ice melts at block light above
   `11 - getLightBlock()`; mushrooms spread by their survival rule (light below 13); the
   daylight detector reads the sky's light less the darkening, every 20 ticks (section
   1.3). Section 1.1 has the table.
2. **Two thresholds, two kinds of read.** `getRawBrightness(pos, 0)` (crops, stems, sweet
   berries, bamboo, the survival of crops and mushrooms, animals spawning) is **not**
   darkened: a crop under the open sky grows all night. `getMaxLocalRawBrightness(pos)`
   (saplings, grass, the light's "magic value" that mobs read, monsters spawning) **is**:
   `max(sky - skyDarken, block)` (n7 LevelLightEngine 304–332; n7 LevelReader 297–330). In
   26.3 `getMaxLocalRawBrightness` no longer looks at a block's neighbours: it is the
   raw brightness, 15 outside ±30,000,000 (LevelReader 306–330). Snow and ice read block
   light alone (`getBrightness(BLOCK, pos)`).
3. **A crop needs light to stay, not only to grow**: `CropBlock.canSurvive` is
   `hasSufficientLight && VegetationBlock.canSurvive`, and `hasSufficientLight` is
   `getRawBrightness(pos, 0) >= 8` (n7 CropBlock 327–356). So seeds cannot be planted in
   the dark, and a crop in the dark breaks at its next shape update. Clustine's rule is
   only "on `#supports_crops`" (`crates/clustine-blocks/src/survive.rs:115`), and every
   view a region's rules read answers no light (`RegionView`'s, `updates.rs:1101`; placing's,
   `placement/context.rs:66`), so the mushroom's light rule (`survive.rs:186-193`) is not
   judged in a region either.
4. **The mobs already read light, and read it wrong wherever a block changed.** Burning in
   daylight, a monster's idle time, every walk target value (each animal's and monster's
   stroll, the skeleton's flight from the sun, the fox's shelter), every `canSeeSky` (the
   fox, villagers, the path's trim that avoids the sun) go through `MobWorld::light`
   (`crates/clustine-sim/src/region/mobs.rs:166-192`): the chunk's stored light, or where a
   change left the chunk none, sky 15 above the motion-blocking heightmap and 0 below,
   **block light 0 everywhere**. A chunk drops its light at the first change light can see
   (`clustine-world/src/chunk.rs:364`), and after B6 most chunks of real terrain come with
   a change (A23 section 9, "72 chunks in 100 carry a mark or a tick"). So in a seeded world
   a torch, lava or glowstone lights nothing a mob reads, glass reads as a roof, and a chunk
   whose neighbour changed keeps the light it was made with (it is read, never checked).
5. **Natural spawning (N9) is the heaviest reader**: `Monster.isDarkEnoughToSpawn` (n7
   Monster 81–133) reads the sky's light against `nextInt(32)`, the block light against the
   dimension's limit (0 in the overworld) and the magic brightness against
   `monster_spawn_light_level` (uniform 0 to 7); every `PathfinderMob.checkSpawnRules` reads
   the walk target value (n7 PathfinderMob 27–41). Spawners use the same rules with
   `SPAWNER`, which does **not** ignore light (only `TRIAL_SPAWNER` does: n6
   EntitySpawnReason 66–87). N8 is what makes N9's spawning possible.

**How the game keeps light**

6. **The game computes light after the tick, not in it.** `LevelChunk.setBlockState` only
   queues: `updateSectionStatus` where a section becomes empty or not, and `checkBlock` where
   `LightEngine.hasDifferentLightProperties(old, new)` (n6 LevelChunk 594–633). On a server
   the light engine is `ThreadedLevelLightEngine` (n5 ChunkMap 39, 258), which is not dumped;
   **Inferred**: it queues the check as a task and runs the queue on a thread of its own
   when the main thread polls tasks between ticks (`tryScheduleUpdate`, called from
   `ChunkMap`'s unload lambda, n5 ChunkMap 3535–3541, and, as remembered, from the main
   thread executor's `pollTask`). `ServerChunkCache.tick` itself calls no light (n5
   ServerChunkCache 685–729). So a random tick in the tick of a change reads the light from
   before it, and one in the next tick almost always reads the new light: a race, not a
   rule (section 2.3).
7. **Light is a function of the blocks; the order of the engine's work is not seen.** Block
   light at a position is the greatest of each source's emission less the cost of the
   cheapest way to it (each step at least 1, more through dampening blocks, none through
   faces that close); sky light the same from the sky's columns. Such a greatest fixed
   point is unique, so any correct order of work reaches the same values; the order only
   decides when a reader sees them. `clustine-light` computes it whole from the 3x3 of
   chunks and matched the official server's stored light (`tests/official.rs`). **The
   only things that can differ are timing and the sections that hold no light** (the
   crate's own caveat, `crates/clustine-light/src/lib.rs:37-45`).
8. **The darkening is already in Clustine**, from the clock: `time::sky_darken`
   (`crates/clustine-sim/src/time.rs:43-50`) is `Level.updateSkyBrightness`
   (`(int)(15 - sky_light_level)`, n5 Level 1185–1199) over the `day` timeline's keyframes;
   0 by day, 11 at the bottom of the night. `isBrightOutside`, `isDarkOutside` and a bed's
   `when_dark` read only the darkening and need no light at all (n5 Level 656–684; n7
   BedRule$Rule 41–65); Clustine has them (`monster_ai.rs:25-27`, `sleep.rs`).

**Clustine's pieces**

9. **`clustine-light` is a function of nine chunks and nothing else**: deterministic, no
   hash maps, no clock (its `clippy.toml` is the simulation's), 2.15 ms a chunk in a release
   build (A23 "Found while building", L0a), 4.2 MB of scratch a call, reusable
   (`light_with`). It has **no relight from a change**: a change reaches 5.9 chunks at the
   mean (`reach`), so 13 ms of one thread a change, once (A23 section 9). A region's tick has
   50 ms. A river, a builder or a crater of TNT changes many blocks a tick.
10. **Nothing about light crosses a line today, and nothing needs to if each region lights
    its own chunks from blocks.** A chunk's light depends on its 3x3 alone; a chunk that
    ticks has its 3x3 loaded or copied (A25 section 3), and copies are kept current by the
    stream at the start of each tick (`region.rs` 3083–3103). So a region can compute the
    light of every chunk it ticks from what it holds already. What it cannot compute
    exactly is the light **inside** a copy or a ring chunk, whose 3x3 reaches a chunk it does
    not have; mobs read there (a stroll's candidates, `FleeSunGoal`'s hiding place, up to 10
    blocks away: n7 FleeSunGoal 129–181).
11. **Saplings need trees.** A sapling's random tick (not dumped) ends in `advanceTree`,
    which places a tree feature: the region has no features today (`clustine-sim` does not
    depend on `clustine-features`), the mangrove propagule's growth is left out for that
    reason already (`random_ticks/plants.rs:14-17`), and a tree can reach across a line
    (effects, A25). It is the largest piece of N8 that is not light.
12. **The edge keeps its own light** (A23): one light per chunk of its replica, computed on
    its own threads from nine chunks of the replica, never stored, never fed back. Region
    lines mean nothing to it: it lights from whatever chunks its views brought, whoever
    serves them. Both sides use the same function, so where both have the same blocks they
    agree.

---------------------------------------------------------------------------------------

## 1. What the game reads light for

### 1.0 The reads themselves

| Read | What it is | Where |
|---|---|---|
| `LevelLightEngine.getRawBrightness(pos, darken)` | `max(sky(pos) - darken, block(pos))`, each layer 0 where its engine is absent | n7 LevelLightEngine 304–332 |
| `getRawBrightness(pos, darken)` on a level | the light engine's (**Inferred**: `BlockAndLightGetter`'s default, not dumped) | Animal's call, n7 Animal 201–215 |
| `getMaxLocalRawBrightness(pos)` | `getMaxLocalRawBrightness(pos, getSkyDarken())` | n7 LevelReader 297–305 |
| `getMaxLocalRawBrightness(pos, darken)` | 15 where `x` or `z` is outside `[-30,000,000, 30,000,000)`, else `getRawBrightness(pos, darken)` | n7 LevelReader 306–330 |
| `getBrightness(layer, pos)` | one layer's stored level (**Inferred**: `getLightEngine().getLayerListener(layer).getLightValue(pos)`; a sky section without data reads the first section above that has some, 15 above all) | not dumped (`BlockAndTintGetter`) |
| `getEffectiveSkyBrightness(pos)` | `max(getBrightness(SKY, pos) - getSkyDarken(), 0)` | n7 LevelReader 332–341 |
| `canSeeSky(pos)` | **Inferred** (K 3.3): `getBrightness(SKY, pos) >= 15` | not dumped (`BlockAndTintGetter`) |
| `canSeeSkyFromBelowWater(pos)` | at or above sea level `canSeeSky(pos)`; below, `canSeeSky` at sea level over the column, then no block between with dampening above 0 that is not liquid | n7 LevelReader 148–200 |
| `getLightLevelDependentMagicValue(pos)` | `f = getMaxLocalRawBrightness(pos) / 15`, `v = f / (4 - 3f)`, `Mth.lerp(ambientLight, v, 1)`; the overworld's `ambient_light` is 0, so `v` | n7 LevelReader 211–236; `dimension_type/overworld.json` |
| `Entity.getLightLevelDependentMagicValue()` | the above at `BlockPos.containing(x, eyeY, z)` if the chunk at `x, z` is loaded, else 0 | n5 Entity 4832–4852 |
| `getPathfindingCostFromLightLevels(pos)` | magic value − 0.5 (≥ 0 from raw brightness 12: `generation/spawning.rs:465-467`) | n7 LevelReader 202–209 |
| `getSkyDarken()` | the field `skyDarken`, set by `updateSkyBrightness()` to `(int)(15.0f - SKY_LIGHT_LEVEL)` once a tick, after the sleep check and before `tickTime` | n5 Level 39, 1185–1199, 1782–1786; n5 ServerLevel 514, 518; D 1.4 |
| `getSkyDarken()` while features are placed | 0 | n7 WorldGenRegion 380–383 |
| `isBrightOutside()` | `!hasFixedTime && skyDarken < 4`; `isDarkOutside()` its negation where time is not fixed. **No light read** | n5 Level 656–684 |

The overworld's `sky_light_level` keyframes are 1.0 at 133 and 11,867 and 0.2667 at 13,670 and
22,330, multiplied by 15 (`timeline/day.json` 103–123): `skyDarken` is 0 by day and 11 in the
middle of the night (D 1.4; `time.rs:10-46`). The block-light limit for monsters is 0 and the
spawn light test uniform 0 to 7 (`overworld.json`, `monster_spawn_block_light_limit`,
`monster_spawn_light_level`).

### 1.1 Blocks' random ticks

Order of draw and read matters to which draws are made (section 5.2): "first" says which
comes first.

| Block | Class | Read | Grows/acts when | First | Source |
|---|---|---|---|---|---|
| wheat, carrots, potatoes | `CropBlock` | `getRawBrightness(pos, 0)` | `>= 9`, age below max, then `nextInt((int)(25 / growthSpeed) + 1) == 0` | light | n7 CropBlock 100–140 (growth speed 174–326) |
| beetroots | `BeetrootBlock` | as the crop | `nextInt(3) != 0`, then the crop's rule | draw | n7 BeetrootBlock 31–45 |
| torchflower crop | `TorchflowerCropBlock` | as the crop | `nextInt(3) != 0`, then the crop's rule | draw | n7 TorchflowerCropBlock 68–81 |
| pitcher crop | `PitcherCropBlock` | **Inferred**: `CropBlock.hasSufficientLight` (`>= 8`) in its `canGrow`, and in its survival for the lower half | | | not dumped |
| melon and pumpkin stems | `StemBlock` | `getRawBrightness(pos, 0)` | `>= 9`, then the crop's chance; at age 7 a fruit beside it | light | n7 StemBlock 71–184 |
| sweet berry bush | `SweetBerryBushBlock` | `getRawBrightness(pos.above(), 0)` | age below 3, `nextInt(5) == 0`, then `>= 9` | draw | n7 SweetBerryBushBlock 72–110 |
| snow layer | `SnowLayerBlock` | `getBrightness(BLOCK, pos)` | `> 11`: `dropResources`, `removeBlock`, game event `BLOCK_DESTROY` | light, no draw | n7 SnowLayerBlock 178–200 |
| ice | `IceBlock` | **Inferred**: `getBrightness(BLOCK, pos) > 11 - state.getLightBlock()` then `melt` (water, or air where water evaporates) | | light, no draw | not dumped |
| saplings (all woods, azalea not) | `SaplingBlock` | **Inferred**: `getMaxLocalRawBrightness(pos.above()) >= 9` then `nextInt(7) == 0`, then `advanceTree` (stage 0 to 1, then the tree's feature) | | light | not dumped |
| grass block, mycelium | `SpreadingSnowyDirtBlock` | **Inferred**: if `!canBeGrass` (opacity only, `LightEngine.getLightBlockInto` of the block above) it becomes dirt; else if `getMaxLocalRawBrightness(pos.above()) >= 9`, four tries at `offset(nextInt(3) - 1, nextInt(5) - 3, nextInt(3) - 1)` onto dirt that `canPropagate` (can be grass and no water above) | | light | not dumped |
| bamboo, bamboo shoot | `BambooStalkBlock`, `BambooSaplingBlock` | **Inferred**: stage 0, `nextInt(3) == 0`, air above, `getRawBrightness(pos.above(), 0) >= 9`, height below 16 | | draw | not dumped |
| mushrooms | `MushroomBlock` | **Inferred**: `nextInt(25) == 0`, fewer than five mushrooms in the 9x3x9 around, a few tries at a spot one away that is air and where the mushroom `canSurvive` (light below 13 unless the block below overrides it) | | draw | not dumped |
| frosted ice | `FrostedIceBlock` | **Inferred**: `getMaxLocalRawBrightness(pos) > 11 - age - getLightBlock()` among its conditions; made only by Frost Walker | | | not dumped |

**Read no light** (so nothing of N8 is needed for them; most are built in B7): cocoa, nether
wart, kelp, sugar cane, cactus, vines and cave vines, chorus, farmland's moisture (n7 does not
dump `FarmBlock`; B7 built it without light), turtle eggs (time of day: n7 TurtleEggBlock),
eyeblossoms (environment attribute), the mangrove propagule (**Inferred**: its own
`randomTick` draws `nextInt(7)` without a light test; `plants.rs:106-114` draws it so), nylium
dying (opacity only), and **grass and mycelium dying** (opacity only, **Inferred**): the
dying half of `SpreadingSnowyDirtBlock` could be built without light, the spreading half
cannot.

### 1.2 Survival and placing

| Rule | Read | Source | Clustine |
|---|---|---|---|
| `CropBlock.canSurvive` (wheat, carrots, potatoes, beetroots, torchflower) | `getRawBrightness(pos, 0) >= 8` and on `#supports_crops` | n7 CropBlock 327–356 | the tag only (`survive.rs:115`) |
| `PitcherCropBlock.canSurvive` | **Inferred**: the lower half needs `>= 8` too | not dumped | the tag only (`survive.rs:127`) |
| `MushroomBlock.canSurvive` | the block below in `#overrides_mushroom_light_requirement`, or `getRawBrightness(pos, 0) < 13` on a block of solid render | not dumped; Clustine's rule says the game's | judged only where a view has light: features (sky 15, `clustine-features/src/context.rs:380-392`), not a region |
| `StemBlock`, `SweetBerryBushBlock` | no light in survival | n7 StemBlock (no `canSurvive` of its own) | — |

Survival is asked at placing (`canSurvive` in the placement's state) and at every shape update
(`VegetationBlock.updateShape`); **light changing is not an update**, so a crop whose light
falls below 8 stays until something beside it changes (**Inferred** from the absence of any
update in the light engine's dumped surface).

### 1.3 The daylight detector

Not dumped (`DaylightDetectorBlock`, `DaylightDetectorBlockEntity`). **SteelMC**
(`steel-core/src/behavior/blocks/redstone/daylight_detector_block.rs:42-71`,
`block_entity/entities/daylight_detector.rs:53-55`), matching the game as remembered: its
block entity's ticker, when `gameTime % 20 == 0`, computes `i = getEffectiveSkyBrightness(pos)`
(dumped: LevelReader 332–341); inverted: `15 - i`; else if `i > 0`, with the sun's angle `f`
(26.3: the environment attribute `visual/sun_angle`, keyframed in `timeline/day.json` 336–355;
the detector's read of it is **Inferred**), `f += ((f < π ? 0 : 2π) - f) * 0.2`,
`i = Math.round(i * cos(f))`; clamped to 0..15; if the `power` property differs, set with
flags 3. It is a redstone source (A26; ADR-0027 lends it: `region/tests/lends_by_the_record.rs:221-241`).
The roadmap counts it among what a player misses within minutes.

### 1.4 Spawners and spawning

| Reader | Rule | Source |
|---|---|---|
| `Monster.isDarkEnoughToSpawn` | false if `getBrightness(SKY, pos) > nextInt(32)`; false if the dimension's block-light limit is below 15 and `getBrightness(BLOCK, pos)` exceeds it (overworld: any block light); then `(thundering ? getMaxLocalRawBrightness(pos, 10) : getMaxLocalRawBrightness(pos)) <= monsterSpawnLightTest.sample(random)` | n7 Monster 81–133 |
| `Monster.checkMonsterSpawnRules` | not `ignoresLightRequirements` (trial spawner) → `isDarkEnoughToSpawn`, then `checkMobSpawnRules` | n7 Monster 134–165 |
| `checkSurfaceMonstersSpawnRules` | the above, and (not a spawner) `canSeeSky(pos)` | n7 Monster 166–190 |
| `Animal.checkAnimalSpawnRules` | on `#animals_spawnable_on` and (unless the trial spawner) `getRawBrightness(pos, 0) > 8` | n7 Animal 174–215 |
| `PathfinderMob.checkSpawnRules` | `getWalkTargetValue(blockPosition()) >= 0` | n7 PathfinderMob 27–41 |
| `Bat.checkBatSpawnRules` | below `WORLD_SURFACE`; a `nextBoolean` that refuses; `getMaxLocalRawBrightness(pos) <= nextInt(4)`; on `#bats_spawnable_on` (**Inferred**: the `nextBoolean` and the 4 are the case outside Halloween, which takes 7 and no `nextBoolean`) | n7 Bat 537–580 |
| `GlowSquid.checkGlowSquidSpawnRules` | at most sea level − 33, `getRawBrightness(pos, 0) == 0`, in water | n7 GlowSquid 154–170 |
| `Guardian.checkGuardianSpawnRules` | `nextInt(20) == 0 || !canSeeSkyFromBelowWater(pos)`, not peaceful, water here and below (not a spawner) | n7 Guardian 796–830 |
| `PatrollingMonster.checkPatrollingMonsterSpawnRules` | `getBrightness(BLOCK, pos) > 8` refuses | n7 PatrollingMonster 151–160 |
| `Stray.checkStraySpawnRules` | monster rules, and `canSeeSky` (not a spawner) | n7k Stray 11–50 |
| `Drowned.checkDrownedSpawnRules` | water below, `isDarkEnoughToSpawn` unless the trial spawner, … | n7k Drowned 296–340 |
| `BaseSpawner.serverTick` | with custom spawn rules, `SpawnData.CustomSpawnRules.isValidPosition` (block and sky light ranges, **Inferred**; not dumped); otherwise `SpawnPlacements.checkSpawnRules(…, SPAWNER, …)`; then `Mob.checkSpawnRules` | n7 BaseSpawner 219–365, 456–462 |
| Spawning with the chunk | `CHUNK_GENERATION`: animals need `> 8` at generation, on the pool, where light is computed (`generation/spawning.rs:373-382`) | NP 3.7; built (N7l) |

Natural spawning and spawners are N9's; what N8 owes them is a light that is right in the
chunks they try.

### 1.5 Mobs

| Reader | Read | Source | Clustine today |
|---|---|---|---|
| `Mob.isSunBurnTick` (undead burning) | magic value at the eyes `> 0.5`, the mob's draw, dry, `canSeeSky(eyes)` | n5 Mob 1021–1088; K 3.3 | `mobs/body/burning.rs:17-24`, `60-95` |
| `Monster.updateNoActionTime` | magic value at the eyes `> 0.5`: idle time +2 more (despawns faster in light) | n7 Monster 27–43 | `burning.rs:33-43` |
| `Monster.getWalkTargetValue` | `-getPathfindingCostFromLightLevels(pos)` | n7 Monster 73–80 | `monster_ai.rs:20-22` |
| `Animal.getWalkTargetValue` | 10 on a grass block, else the light cost | n7 Animal 123–138 | `animal.rs:408-432` |
| `Guardian.getWalkTargetValue` | in water `10 + light cost`, else the monster's | n7 Guardian 343–362 | `types/guardians.rs` |
| strolls (`RandomPos`, `DefaultRandomPos`, `LandRandomPos`) | the best walk target value of up to ten candidates, ±10 across, ±7 up | n7 RandomPos | `mobs/ai/random_pos.rs:102` |
| `FleeSunGoal` | `isBrightOutside`, on fire, `canSeeSky(blockPosition)`, nothing on the head; ten tries ±10, −3..+2, for `!canSeeSky && walkTargetValue < 0` | n7 FleeSunGoal 35–75, 129–181 | `types/skeletons.rs:168-230` |
| `RestrictSunGoal`, `MoveThroughVillageGoal`, `StrollThroughVillageGoal`, `OfferFlowerGoal`, the drowned's goals, the wandering trader's | `isBrightOutside` / `isDarkOutside` only | n7 RestrictSunGoal 14–25 and the others' `canUse` | built where the types are |
| `GroundPathNavigation.trimPath` (avoid the sun) | `canSeeSky` along the path | n7 GroundPathNavigation 265–320 | `mobs/ai/navigation.rs:487-512` |
| villagers' `MoveToSkySeeingSpot`, `InsideBrownianWalk` | `canSeeSky` | n7 MoveToSkySeeingSpot 60–110; InsideBrownianWalk 36–125 | `brain/behaviours/villagers.rs:33-39` |
| the fox's shelter, sleep, sound | `canSeeSky`, `isBrightOutside`, the walk target value | n7 Fox$SeekShelterGoal 25–85, Fox$SleepGoal 74–95, Fox$FoxBehaviorGoal 31–55, Fox 1677–1690 | `types/fox.rs:284-366` |
| the spider's goals | magic value `>= 0.5` ends its attack one time in a hundred; below it, it targets | n7k Spider$SpiderAttackGoal 26–40, Spider$SpiderTargetGoal 12–20 | N9 |
| `Vex.getLightLevelDependentMagicValue` | always 1 | n7 Vex 401–404 | N9 |
| `ArmorStand.getLightProbePosition` | for the client's drawing only | n5 ArmorStand 1587–1630 | nothing to do |

### 1.6 What reads only the time, or the weather

Sleeping (`BedRule` `when_dark` = `isDarkOutside`, n7 BedRule$Rule 41–65), the wandering
trader's potions, the iron golem's flower, the drowned's goals, the skeleton's
`RestrictSunGoal`: the darkening alone, built. `Level.precipitationAt` (n5 Level 1617–1630)
and `findLightningTargetAround` (n5 ServerLevel 4722–4735) read `canSeeSky`, and snow and ice
forming in the cold (`Biome.shouldSnow`, `shouldFreeze`, not dumped) read block light below
10 (**Inferred**): all weather, which Clustine does not have. The phantoms' insomnia
(`PhantomSpawner`, not dumped) reads `canSeeSky` and the darkening (**Inferred**): N9.

---------------------------------------------------------------------------------------

## 2. How the game keeps light up to date

### 2.1 What a block change does

`LevelChunk.setBlockState` (n6 LevelChunk 488–640), after the section's state is set and the
heightmaps updated (around 575–592):

1. if the section went from only air to not, or back: `getLightEngine().updateSectionStatus(pos,
   empty)` and `onSectionEmptinessChanged` (594–598);
2. if `LightEngine.hasDifferentLightProperties(old, new)` (610): `ChunkSkyLightSources.update`
   (the column's lowest sky-lit block), then `getLightEngine().checkBlock(pos)` (633).

`ProtoChunk.setBlockState` does the same during generation (n7 ProtoChunk 165–250), and the
statuses `initializeLight` and `light` light a chunk whole (n7 ChunkStatusTasks 242–280).
`hasDifferentLightProperties` is not dumped; Clustine's equivalent is `light_tells_apart`
(`clustine-world/src/chunk.rs:204-226`, the function at 212: emission, dampening, sky passing down, occluding
faces, air).

### 2.2 The engines

`LevelLightEngine` (n7, 392 lines) holds a block engine and a sky engine (13–41: a
`BlockLightEngine` if the dimension has block light, a `SkyLightEngine` if it has a sky).
`checkBlock` passes the position to both (60–77); `runLightUpdates` runs the block engine and
then the sky engine and returns the work done (101–125); `getRawBrightness` reads both (304–332).
`LightEngine`, `BlockLightEngine`, `SkyLightEngine`, their section storages, `DataLayer` and
`ChunkSkyLightSources` are **not dumped**. **Inferred** (as remembered of the engine Mojang
rewrote in 1.20): each engine keeps a queue of decreases and a queue of increases of packed
positions with the directions still to visit; `checkBlock` enqueues a decrease of the old
level and, where the new state emits or the position can be lit from a neighbour, an
increase; `runLightUpdates` empties the decreases first, which re-enqueue the neighbours that
were lit from elsewhere as increases, then the increases. Each section's levels are a
`DataLayer` of 2,048 bytes; the storage keeps an "updating" map and a "visible" copy that
readers on the main thread read, swapped after a batch. SteelMC has a port of this machinery
(`steel-core/src/chunk/light/`, 9,600 lines: `queue.rs`, `propagation.rs`, `workset.rs`,
`sky_propagation/`), AGPL-3.0 like `clustine-light`'s other adaptations
(`crates/clustine-light/NOTICE`).

### 2.3 When a change's light is computed

What is dumped:

- `ChunkMap` makes the `ThreadedLevelLightEngine` with the chunk map as its chunk getter
  (n5 ChunkMap 258–259); asks it for `hasLightWork` (878–883); waits for its pending tasks
  before sending a chunk (`waitForLightBeforeSending`, 3131–3150); and at an unload calls
  `updateChunkStatus` then `tryScheduleUpdate` (3535–3541).
- `ServerChunkCache.tick` (n5 685–729) purges tickets, runs the distance manager, ticks the
  chunks (random ticks, spawning) and the chunk map, unloads: **no light**. `pollTask`
  (600–605) is the main thread executor's (`ServerChunkCache$MainThreadExecutor`, not
  dumped). `onLightUpdate` (1052–1060) tells the chunk holder which sections changed, for the
  clients.
- `ServerLevel.tick` (n5 426–): sleep check, `updateSkyBrightness` (514), `tickTime` (518),
  the chunk source's tick (568), block events (575), entities (623), block entities (628).
  No light.

**Inferred** (needs `ThreadedLevelLightEngine` and `ServerChunkCache$MainThreadExecutor`):
`ThreadedLevelLightEngine.checkBlock` does not run the engine; it adds a task for the
position's chunk to a queue. `tryScheduleUpdate`, if there are tasks or work and none is
scheduled, hands `runUpdate` to the light's own executor, a thread apart from the main one;
`runUpdate` runs the queued tasks, then `runLightUpdates`, then the tasks that wait for the
result. The main thread calls `tryScheduleUpdate` from its executor's `pollTask`, that is,
while it waits between ticks (`MinecraftServer.waitUntilNextTick`, not dumped), and as
chunks unload. So:

- **during a tick**, a rule that reads light reads what the light thread last published:
  the light from before every change made since the last wait, this tick's included;
- **in the next tick**, nearly always the new light (a torch's update is microseconds of
  work, and the wait is tens of milliseconds), but nothing orders the two: under load the
  light thread can still be running when the tick begins, and a read may see a part of it;
- **a player's action** is handled between ticks too (as `region.rs:1015-1018` notes,
  `MinecraftServer.runAllTasks`), so its light may be ready before the next tick or not.

No player can tell one of these from another. What a deterministic simulation needs is a
rule of its own (section 5.3).

### 2.4 The darkening

`skyDarken` is set once a tick from the clock (2.1's table; D 1.4), before the chunks tick,
and stays for the tick. It changes no stored light: the stored sky light is the sky's full
light, and the darkening is subtracted at each read (`getRawBrightness(pos, darken)`,
`getMaxLocalRawBrightness`). Clustine's `sky_darken(day)` gives it from the region's clock
(`time.rs:43`). A night is therefore **no light work at all**: nothing is relit at dusk.

### 2.5 Whether the order of work matters

The values: the engine's increase and decrease passes are a way of finding the greatest
assignment of levels in which every lit position is either a source at its emission or one
less (at least the dampening less) than a neighbour that can pass light to it. That
assignment is unique for given blocks, so the order in which positions are visited cannot
change it; it changes only how much is visited. The sky's is the same with the columns open
to the sky as sources at 15 and straight down through air without loss (`clustine-light`'s
rule, `lib.rs:14-21`). `clustine-light` computes it by flooding the 3x3 whole, and matched the
official server's stored light, section by section, over the sample world
(`tests/official.rs:379`). **Exceptions** where order or storage, not the blocks, decide:

1. sections that hold no light of their own: the game reads a sky section without data from
   the one above; the crate floods them and leaves them out after (`lib.rs:37-45`); a read
   under a floating block far above anything may differ, and no chunk of the sample showed
   it;
2. the rim of what is loaded: the game does not propagate into a chunk whose light is not on,
   the crate takes an absent chunk as a wall. Neither matters where the 3x3 is present;
3. the timing of 2.3.

So the plan can take "the light is `clustine_light::light` of the 3x3 as the blocks stand at a
fixed point of the tick" as exactly the game's values, and choose that point.

---------------------------------------------------------------------------------------

## 3. What Clustine has

### 3.1 `crates/clustine-light`

| What | Where |
|---|---|
| `light(&Around) -> Light`: sky and block light of the centre chunk from it and its eight neighbours; an absent neighbour is a wall; a dimension without sky has no sky light | `src/lib.rs:64-91` |
| `light_with(around, also, scratch)`: the same in a reusable scratch, and the levels of sections the light leaves out where asked (the edge's "listed once, listed always") | `src/lib.rs:93-108`, `140-217` |
| `reach(x, z)`: which neighbours a change in a column can relight, by distance (14 steps); 4.9 a column at the mean; a change of a section's emptiness reaches all eight | `src/lib.rs:110-137` |
| `VERSION = 1`; `Light::made_by`, `Light::beside` | `src/lib.rs:62`; `clustine-world/src/light.rs:91-113` |
| The flood: a grid of 48 × 48 × the world's height for the nine chunks; uniform sections filled at once; 4.2 MB allocated a call, the peak the same | `src/flood.rs`; A23 "What `clustine-light` is" |
| **Time**: 2.15 ms a chunk in release, 2.10 ms with a scratch used again; 15.6 ms in the dev profile, 2.5 ms with the three crates optimised in it (the workspace's profile lines) | A23 "Found while building", L0a |
| **No relight**: nothing is kept between calls | `src/lib.rs:4-7` |
| Tests: `official.rs` (ignored; a world of the official server's, `CLUSTINE_SAMPLE_WORLD`), 30 cases (`cases.rs`) and 15 for the additions (`additions.rs`) | `tests/` |
| Determinism: its `clippy.toml` is the simulation's ("the simulation may use this crate") | `crates/clustine-light/clippy.toml` |

`clustine-sim` does not depend on it today (`crates/clustine-sim/Cargo.toml`).

### 3.2 Light in a chunk

`clustine_world::Light` (`light.rs:91-127`): `made_by`, `beside`, and for the sky and the
blocks one `SectionLight` a section with one below and one above (`Absent`, `Dark`, `Full`,
`Levels` of 2,048 bytes); `sky_at` and `block_at` read a level with the game's meaning of
`Absent` (15 for the sky, 0 for blocks). A `Chunk` has `light: Option<Light>`; **`set` drops it
at the first change `light_tells_apart`** (`chunk.rs:341-366`, the drop at 364); `set_light` and `take_light`
(`chunk.rs:522-560`). Light is about 9.3 KB of a chunk's 32.0 KB at the mean for real terrain
(A23 section 1, from A22).

Who gives a chunk light: the generation pool lights every chunk it makes, from the nine
(`services/worldgen/src/scheduler/pool.rs:341-346`), so a stored chunk carries light until
its first change; the store replays commits into chunks with `set`, so a chunk stored after
a change has none; a region's post-processing and settling (B6) change most chunks of
real terrain. In the flat world the edge needs no job at all (A23 section 9, first row).

### 3.3 Light at the edge (A23)

- An edge keeps one light per chunk of its replica, beside the chunk (`ReplicaChunk`):
  carried (the chunk's own, while test T on the chunk and its neighbours passes: same rule
  version and `beside` the generator version of unchanged neighbours, A23 section 7),
  computed, or fallback (a job failed: old light, or `sky_light` over heights).
- A block change that light sees marks the chunk and those `reach` names (A23 section 3);
  marked chunks are lit on the edge's own threads (two on six or eight processors, three on
  twelve), one job a chunk, from nine shared chunks of the replica, nearest the players who
  wait first; a result goes to those who wait as the chunk, to those who have it as a light
  update (A23 sections 4–6). A job is 2.5 ms (L7).
- Nothing is stored or sent back to a region (A23's preamble: "nothing an edge computes is
  stored"). A chunk at the rim of a view is held up to `LATEST` (1 s) for its neighbours,
  then lit against a wall (sections 4, 9).
- **At a line the edge does nothing special**: the replica has chunks from every region that
  serves its views (`served_by`), and a job reads whichever of the nine are in the replica.
  What is shown is light for the blocks the edge has, which are the regions' committed blocks.

### 3.4 What the simulation reads today

| Reader | Light it gets | Where |
|---|---|---|
| `MobWorld::light(pos)` (every mob read of section 1.5) | the chunk's `Light` if it has one, levels as stored; else sky 15 at and above the motion-blocking height, 0 below; block 0; `None` where the chunk is neither land nor copy | `region/mobs.rs:166-192`; `mobs/view.rs:64-69`, `138-142` |
| the magic value at the eyes | raw = `max(sky - sky_darken, block)`; 0 where `None` | `burning.rs:13-24`; `monster_ai.rs:29-40` |
| `light_cost` (walk target values) | the same raw; 15 where `None` | `animal.rs:422-432` |
| `can_see_sky` (navigation, villagers, fox, skeletons) | sky `>= 15`; **true** where `None` | `navigation.rs:511-512`; `villagers.rs:33-39`; `fox.rs:290-292` |
| `sky_darken`, `bright_outside`, `is_dark` | the clock | `time.rs`; `region/mobs.rs:105-107` |
| block rules' `BlockView::raw_brightness` | `None` in every region view ("a rule that needs light then does not judge it") | `clustine-blocks/src/lib.rs:37-40`; `region/updates.rs:1101`; `region/fluids.rs:332`; `placement/context.rs:66`; `rails.rs:52`; `physics.rs:77` |
| features at generation | sky 15, the game's `WorldGenRegion` | `clustine-features/src/context.rs:380-392` |
| spawning with the chunk | the pool's light (`SpawnView::sky_light`, `block_light`) | `generation.rs:55-58`; `generation/spawning.rs:373-382` |

So the fallback is the common case in a seeded world after B6, not the rare one. It gives
**no block light anywhere**: torches, lanterns, lava, glowstone, a lit furnace; it takes
glass and every other motion-blocking block that lets light through as a roof, and leaves
(motion-blocking, dampening 1) the same. A chunk that kept its light because nothing in it
changed is read as made, even where a neighbour's torch now lights it.

### 3.5 Random ticks left out for light

`crates/clustine-sim/src/region/random_ticks.rs:13-14`: "What needs light (crops, saplings,
grass, mushrooms, bamboo, ice and snow melting) does nothing yet". The dispatch
(`random_ticks.rs:257-266`) tries plants (`plants.rs`: leaves, sugar cane, cactus, kelp and
the growing plants, vines, cocoa, nether wart, chorus, the hanging propagule), minerals
(`minerals.rs`: copper, amethyst, dripstone and the sulfur spike, redstone ore, nylium,
farmland, dried ghasts) and fire; anything else does nothing and draws nothing. Positions are
drawn from a stream of each section, chunk and tick (`Random::of_section`, 51–58; the
loop, 215–251), so
Clustine's draws are the game's in distribution, not in value (B7). The random-tick order is
ascending chunk, sections from the bottom, three positions each.

Not built for reasons other than light, which N8 meets: bone meal (a dispenser's
`Behaviour::BoneMeal` waits "until growth comes", `dispensers/tests.rs:708`), trees grown in a
region (`advanceTree`, `plants.rs:14-17`), weather.

---------------------------------------------------------------------------------------

## 4. What crosses a line

**What light depends on.** A level falls by at least one a step from at most 15, so the light
of a block depends on blocks at most 14 steps away (`lib.rs:27-28`), and the light of a chunk
on its 3x3 and nothing further (`lib.rs:24-31`). A change relights the chunks `reach` names,
up to all eight neighbours where a section's emptiness changes.

**What a region has across its line.** A border copy of every foreign chunk in the 3x3 of a
chunk it holds with a viewer's or a guest's ticket; ring chunks (loaded around the views, not
ticking) take no copies beside them (A25 section 3). Copies are bases (the committed chunk as
of a sequence) and later block changes, applied at the start of the tick with the loads,
before the ticking set is worked out (`region.rs:3083-3103`). A chunk ticks where its 3x3 is
loaded or copied (A24 section 2; A25 section 3). Copies carry light only as their base does:
a stored chunk with its light, which the replay of any light-visible change drops.

So:

| Chunk | Its 3x3 in the region | Its light computable exactly in the region |
|---|---|---|
| a held chunk that ticks | yes (own, ring, copies) | yes |
| a ring chunk | not always (the ring's outer side) | only where its 3x3 happens to be loaded |
| a border copy | no: a chunk two across is not copied | no; a column near its far side depends on blocks the region has not got |

**What reads where.** Random ticks read at their own position or the one above, in a chunk
that ticks: always computable. Mushroom spreading and a grass block's spreading read and
write up to one block away, possibly in a copy: the decision is the region's, the set is an
effect (A25 section 4), and the grass's target needs no light (opacity only). Mobs tick
only in chunks that tick (ADR-0028 section 2), but read up to 10 blocks across (strolls,
`FleeSunGoal`'s hiding place), and their paths reach further: into ring chunks and copies.
Natural spawning (N9) tries chunks near players, all of which the holder ticks (NP 3.7).

**Coupled regions.** While two regions' border ticks they run in lockstep, and a change made
in B's tick `t` reaches A's copy before A's tick `t + 1` (A25 sections 1 and 8). If A lights
its chunks at the start of each tick, after the copies are updated, A's border chunks see B's
change in `t + 1`, which is when the game would show it to a rule in one level (2.3).
Uncoupled, a copy's update is one to a few ticks late, and light at the line follows it.

**What the edge does at a line**: nothing (3.3). Its light at a line is computed from both
sides' chunks as its views bring them, independently of the regions.

**The two sides agree without exchanging light**: each region computes its own chunks from
blocks, which already cross; neither needs the other's light for a chunk it ticks. The
question is only what a read inside a copy or a ring chunk returns (question 5.8).

---------------------------------------------------------------------------------------

## 5. The questions the plan must answer

### 5.1 Where light is computed

- **A. In the region, from its own blocks and copies, at a fixed point of each tick**
  (recommended): every chunk that ticks gets the light of its 3x3; rules read fresh light; a
  tick stays a function of the region and its inputs. Needs an engine fast enough (5.4).
- **B. In the region, on demand**: a chunk's light is computed the first time something reads
  it in a tick after a change within its reach, and kept until the next. Exactly A's values
  (the function of the same blocks at the same point), so as deterministic, but only where
  something reads: a river through a field with no crops and no mobs costs nothing. A
  variant of A rather than a rival; it moves cost, it does not remove it.
- **C. At the edge, fed back**: the edge's results would arrive at a region at times no input
  orders, so they would have to go through the store as inputs, late by a round trip, and be
  part of the log. Light at a line would be the edge's, which sees only what its players see.
  Not recommended.
- **D. Keep the fallback and widen it** (block light from emitting blocks within a radius by
  distance alone, glass let through): cheap, wrong in every enclosed space, and still a
  function to test. Only as a stopgap.

### 5.2 Exactness

Section 2.5: the values are a function of the blocks; the game's order of work is invisible.
What the plan must still say:

1. **Which point of the tick light is current at** (5.3), the only place Clustine and the game
   can differ, and the game itself is racy there.
2. **Draws gated by light.** Crops, stems and saplings test light before they draw; sweet
   berries, bamboo, beetroots, torchflowers and mushrooms draw first (1.1). A region's stream
   is its own (B7), so a draw made or not made shifts only what the stream gives later in
   that section's tick: distribution, not value, as for every random tick now. The plan
   should keep the game's order within each rule all the same (as `plants.rs` does).
3. **Version.** If the region computes light with the crate's function, `clustine_light::VERSION`
   stays the one definition; an incremental engine (5.4) is judged against it, not against
   itself.

### 5.3 When in the tick

- **At the start, after the inputs** (copies, seams, arrivals, players' actions) **and before
  the block ticks** (recommended): a change of tick `t`, the region's or a coupled neighbour's,
  is read from `t + 1`, as the game nearly always gives it; a player's action is read in the
  tick it is applied, where the game gives it in that tick or the next.
- **At each change**: fresher than the game (a crop beside a torch placed in the same tick
  would grow by it); costs a relight for each change.
- **At the end**: equal to the start for the region's own changes, but a neighbour's change
  reaches its copy only at the next tick's start, so light at a line would lag one tick more.

### 5.4 Cost, and which engine

The tick has 50 ms; measured ticks today stay under it with a hundred cows and a crater of
64 TNT (roadmap: N6, N7f), and a river crossing a line left a gap of 74 ms between ticks at
worst (E7). A full relight with the crate is 2.15 ms a chunk; one change
relights 5.9 chunks at the mean, 13 ms; two changes apart, 26 ms. A tick in which a crater,
a river at its front, a piston clock with redstone lamps, redstone ore lit by walking mobs,
or a builder's every block changes light in several places is over the budget with the crate
alone.

- **E1. The crate, whole chunks, once a tick for every chunk within reach of a change**
  (union, not a sum): exact by construction, nothing new to write, cost unbounded by design.
  With B (on demand) only chunks read are relit.
- **E2. An incremental engine** in `clustine-light`: light kept per chunk and mended from a
  change by decrease and increase queues, as the game's `LightEngine` does; cost proportional
  to what changes (a torch: thousands of positions, well under a millisecond). Deterministic
  if its queues are plain vectors in a fixed order (no hash maps; the crate's `clippy.toml`).
  Judged by a property test: after any sequence of changes, equal to `light()` of the 3x3.
  Needs the engine's classes dumped (section 6) or SteelMC's port read (`steel-core/src/chunk/light/`).
- **E3. Both**: E1 first (exact, simple, the reference), E2 behind the same function once a
  measurement says the tick needs it, with E1 as its test oracle.
- **What happens when the work is over a budget**: either the tick is longer (as today with
  any heavy tick), or the relight is deferred by a deterministic rule (so many chunks a tick,
  in ascending order, the rest read their old light): values then lag, still a function of
  the inputs.
- **Threads**: the chunks of one tick could be lit in parallel and their results taken in
  order, deterministic in value. Whether `clustine-sim` may use threads at all is the owner's
  (architecture "Determinism": a pure function, not one thread).

To measure (on the real disk and in memory, as CLAUDE.md asks): a builder placing and breaking
torches; a river flowing across a view; 64 TNT; a lamp clock; a region's load of a view (the
light of 289 to 330 chunks at 2.15 ms is 0.6 to 0.7 s of one thread); a merge, a split and a
take-over (5.6).

### 5.5 Determinism

- The light of a chunk that ticks is a function of its 3x3's blocks; computed at a fixed
  point of the tick it is a function of the region's state and inputs. An incremental engine
  must give the same (5.4, E2's property test), or a restore, which computes whole, reads
  different light from the region that died.
- A chunk whose 3x3 is not all there (ring, copies) must have a rule that depends only on
  which chunks are there, which is region state (5.8).
- The darkening is the clock's, already an input (`time.rs`).
- No hash maps, no clock (both crates' `clippy.toml`).

### 5.6 Durability

- **Derived, not committed** (recommended): light is a function of blocks, so a restore, a
  merge, a split, a move or a take-over computes it from the chunks. Nothing in the log; the
  commit is unchanged. Cost: the whole view at a load (0.6 to 0.7 s of one thread for a view,
  less on demand); against B8's pauses (a merge or a split stands still 0.04 to 0.24 s on land)
  that is long, so the plan should say whether light is computed before the first tick after a
  load, on demand, or on a pool, and whether chunks kept warm across a merge or a split keep
  their light (they keep their blocks today: A25 section 3).
- **Committed**: every relight's changed sections in the log: kilobytes a change (a section is
  2 KB of levels), more than the change itself. Not recommended.
- **Carried as a cache**: a chunk as generated carries light the pool made, and A23's test T
  says when it still holds (same rule version, neighbours as generated). A region could take
  carried light where T passes and compute the rest, as the edge does. Since stored chunks lose
  light at their first change, this saves work only for untouched land.

### 5.7 Lines and copies

- Each region lights the chunks it ticks from its own blocks and its copies; nothing about
  light crosses a line (section 4). Two coupled regions see each other's changes in the next
  tick (5.3).
- Uncoupled regions: a copy update one to a few ticks late makes light at the line as late.
  Named difference, or the same as every effect today.
- The effects of a rule decided by light at a line (a mushroom spreading into a copy, a grass
  block into dirt across the line) are sets of blocks in another's land, already carried (A25).

### 5.8 Reads where the 3x3 is not there

Mobs read light in copies and ring chunks (section 4). Options:

- **a.** Light the chunk with what is there, absent chunks as walls (the crate's rule):
  deterministic, wrong near the far side, right near the line, where a mob mostly reads.
- **b.** The fallback of today for such chunks (stored light, or the heightmap): simpler, wrong
  everywhere in a changed chunk.
- **c.** Copy two chunks across instead of one for chunks with a viewer's ticket: exact, more
  copies (the stream's bytes: N7f measured 11 MiB a minute of copy updates for 100 cows at a
  line) and an A25 change.
- **d.** Carry the holder's light with copies: the holder computes it anyway, but it is a new
  thing on the stream, timed by the holder's tick, and read a tick late.

### 5.9 The transition from ADR-0023

- **T1. Leave the edge as it is** (recommended for N8): two computations of one function, the
  client's light unchanged and already right after changes. The cost is doubled work where
  light changes; nothing a player sees moves.
- **T2. Regions send their light to the edge** with each tick's changes (changed sections),
  and the edge stops lighting chunks a region serves: removes the edge's jobs and A23's hold,
  rest and test T for served chunks; adds up to 2 KB a changed section to every delta and
  every client fan-out. A large change to A23, A22 and the protocol between worker and edge;
  a record of its own, if ever.
- **T3. Regions give chunks their light when a view first asks for them**, so the edge's test
  T or a new kind ("lit by the region") makes them sendable at once, and the edge relights only
  after changes. Smaller than T2; saves A23's join jobs (330 for a join, L7).

Whatever is chosen, the edge's light and the region's should be compared in a test: the same
blocks give the same light (both `clustine_light`).

### 5.10 Scope of N8

What needs light and a player meets within minutes, and what else each needs:

| Rule | Needs besides light |
|---|---|
| crops, beetroots, torchflower, stems and their fruit, sweet berries | the crop's survival rule with light (1.2); bone meal is not light's |
| pitcher crop | its two halves (dump `PitcherCropBlock`) |
| saplings | `advanceTree`: tree features placed by a region, across a line as effects (finding 11); the azalea and mangrove propagule too |
| grass and mycelium spreading and dying | dump `SpreadingSnowyDirtBlock`, `GrassBlock`, `MyceliumBlock`; the dying half needs no light and could come first |
| bamboo and its shoot | dump `BambooStalkBlock`, `BambooSaplingBlock` |
| mushrooms spreading; mushrooms and crops surviving by light | dump `MushroomBlock`; `BlockView::raw_brightness` answering in a region and at placing |
| ice and snow melting | dump `IceBlock`; `SnowLayerBlock` is dumped |
| the daylight detector | dump `DaylightDetectorBlock` and its block entity; a block entity ticking every 20 ticks |
| mobs' reads (1.5) | `MobWorld::light` reading the region's light; `can_see_sky`'s answer where nothing is loaded (true today; **Inferred** the game's sky read of an unloaded chunk) |
| spawners, natural spawning | N9 |
| frosted ice, snow and ice forming, phantoms | Frost Walker, weather, N9 |

### 5.11 Smaller points to settle

1. **A crop already planted in the dark** in an existing world breaks at its next shape update
   once the survival rule comes. The game does that too; it may surprise a player whose crops
   in a dark room vanish after N8.
2. **What the tests are judged by.** From the specification (CLAUDE.md): the thresholds of
   1.1, 1.2 and 1.4 at their edges (8 and 9, 11 and 12, the darkening at night), each rule's
   draw order, light equal to `clustine_light::light` after any changes (and at a line, equal
   to one region's), the edge's and the region's light equal. Against the official server:
   `tests/official.rs`'s way extended to changed worlds (place blocks by command in the
   official server, save, compare its stored light with the region's after the same changes);
   statistical growth rates under a torch and under the sky at night (crops grow, saplings do
   not).
3. **`clustine_light` in `clustine-sim`**: a new dependency, `clustine-light` → `clustine-world`
   and `clustine-data` only; no cycle.
4. **The rim between a region and nothing** (the world's edge of what is loaded at all): a chunk
   ticks only with its 3x3, so this is 5.8's case.
5. **Memory**: light for each loaded chunk, about 9.3 KB at the mean, up to 104 KB for a chunk of
   26 sections in both layers as levels; a region of 600 chunks about 6 MB at the mean, and the
   crate's 4.2 MB scratch.
6. **A chunk's `light` field** is dropped by `Chunk::set` at a change; a region that keeps light
   needs it to live beside the chunk (as the edge's does) or `set` to stop dropping it inside a
   region. The store's replay should keep dropping it.

---------------------------------------------------------------------------------------

## 6. Classes not dumped, which the plan needs

For the rules (section 1):

- `net.minecraft.world.level.block.SaplingBlock` (and `MangrovePropaguleBlock`, `AzaleaBlock`,
  `TreeGrower` for `advanceTree`)
- `net.minecraft.world.level.block.SpreadingSnowyDirtBlock`, `GrassBlock`, `MyceliumBlock`,
  `SnowyDirtBlock`
- `net.minecraft.world.level.block.IceBlock`, `FrostedIceBlock`
- `net.minecraft.world.level.block.BambooStalkBlock`, `BambooSaplingBlock`
- `net.minecraft.world.level.block.MushroomBlock`
- `net.minecraft.world.level.block.PitcherCropBlock`
- `net.minecraft.world.level.block.VegetationBlock` (its `updateShape` and `canSurvive`)
- `net.minecraft.world.level.block.DaylightDetectorBlock`,
  `net.minecraft.world.level.block.entity.DaylightDetectorBlockEntity`
- `net.minecraft.world.level.BlockAndTintGetter`, `BlockAndLightGetter` (`getBrightness`,
  `getRawBrightness`, `canSeeSky`)
- `net.minecraft.world.level.SpawnData$CustomSpawnRules` (N9)
- `net.minecraft.world.level.biome.Biome` (`shouldFreeze`, `shouldSnow`: weather, later)

For how the game keeps light (section 2):

- `net.minecraft.server.level.ThreadedLevelLightEngine` and its task type
- `net.minecraft.server.level.ServerChunkCache$MainThreadExecutor` (`pollTask`)
- `net.minecraft.server.MinecraftServer` (`waitUntilNextTick`, `runAllTasks`, the tick's order)
- `net.minecraft.world.level.lighting.LightEngine`, `BlockLightEngine`, `SkyLightEngine`,
  `LayerLightSectionStorage`, `BlockLightSectionStorage`, `SkyLightSectionStorage`,
  `ChunkSkyLightSources`, `LayerLightEventListener`, `LightEventListener`
- `net.minecraft.world.level.chunk.DataLayer`

Already dumped and used here: `LevelLightEngine`, `LevelReader`, `Level`, `ServerLevel`,
`ServerChunkCache`, `ChunkMap`, `LevelChunk`, `ProtoChunk`, `ChunkStatusTasks`,
`WorldGenRegion`, `CropBlock`, `StemBlock`, `SweetBerryBushBlock`, `SnowLayerBlock`,
`BeetrootBlock`, `TorchflowerCropBlock`, `TurtleEggBlock`, `Entity`, `Mob`, `Monster`, `Animal`,
`PathfinderMob`, `EntitySpawnReason`, `BaseSpawner`, `NaturalSpawner`, `SpawnPlacements`,
`DimensionType`, the goals and spawn rules of section 1.5 and 1.4.
