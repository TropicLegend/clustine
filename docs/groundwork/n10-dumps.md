# N10 brief against the n10 dumps: what the bytecode settles

Reads the **Inferred** claims of `docs/groundwork/n10-brief.md` against the classes its section
15 asked for, dumped from `server-26.3.jar` into `/home/claude/reference/n10-javap/` (`javap -p
-c -constants`, one file per class; files ending in `-v` are `javap -v`, which shows the
bootstrap arguments that say which lambda an `invokedynamic` is). Also dumped since the brief:
`TradeCost`, the boss event packet's inner classes and `ClientboundSoundPacket`. "n10 X lines
a–b" are lines of that class's file; other dumps are cited by their directory. Verdicts:
**CONFIRMED**, **CORRECTED**, **STILL UNKNOWN**.

Read by three readers in parallel (A: points of interest, the village tracker, the schedule,
saved data; B: the social life and trading; C: raids, omens, the boss bar, the spawners, maps),
each part below as its reader wrote it; what the lead settled afterwards ends parts B and C.

---------------------------------------------------------------------------------------

## Part A: points of interest, the village tracker, the schedule, saved data

Classes read: n10 `SectionTracker`, `PoiManager$DistanceTracker`, `PoiManager$Occupancy`,
`PoiSection$Packed`, `PoiRecord$Packed`, `SectionStorage`, `SerializableChunkData`,
`BlockableEventLoop`, `Cursor3D`, `Holder$Reference`, `GlobalPos`, `KeyframeTrack`,
`KeyframeTrackSampler`, `SavedData`, `SavedDataStorage`; with n8 `DynamicGraphMinFixedPoint`,
n7 `PoiManager`, `PoiSection`, `PoiRecord`, `PoiTypes`, `SectionPos`, `SectionPos$1`,
`ChunkPos`, `ChunkPos$2`, `AgeableMob`, `Villager`, `LerpFunction`, `AttributeType`,
`AttributeModifier$OverrideModifier`, n8 `AttributeTrack`, `AttributeTrackSampler`, n9
`MinecraftServer`, `LivingEntity`, n7d `ServerLevel`, n7k `Raids`. Line numbers are of the
`javap` file named.

### Summary of the corrections

1. **The village tracker reaches 5 sections, not 6** (finding 3, section 11). Levels are 0 to 6
   (`levelCount` 7), a lowered level is passed on only below 5, so a source gives levels 0..5 by
   3-D Chebyshev section distance and nothing at 6. A section never reached reads **7**, not 6.
   A section that was once within 5 and lost its source keeps a stored **6** for the rest of
   the run (it is never removed), and `isCloseToVillage(p, 6)` (zombies' `MoveThroughVillageGoal`,
   foxes' `StrollThroughVillageGoal`) is true there: a residue of former villages.
2. **Point-of-interest sections are never unloaded** (finding 3, section 2.2). `SectionStorage`
   only ever puts into its map; `get` is null only for a chunk whose POI data was never read
   since the server started. "Loaded" in `isVillageCenter` means "read at any time this run",
   whether by a chunk load's prefetch or by any query's `getOrLoad` reaching it.
3. **A rebuild at load does not free claims** (finding 4). `checkConsistencyWithBlocks` leaves a
   valid section untouched. An invalid one (`Valid` false or missing) is refreshed **keeping
   every old record, with its tickets and its type, at each position where a POI block now is**
   (`computeIfAbsent` by position); only new positions get full tickets. Only a section absent
   from the POI file is built from blocks with full tickets (and only when its palette may hold
   a POI state). The check runs only for chunks read from disk, for each saved section with
   block data.
4. **`server.execute` in `updatePOIOnBlockStateChange` runs at once** for changes made from the
   world tick **and** from packets: in 26.3 packets are processed by `PacketProcessor` directly
   from `processPacketsAndTick`, before `tickServer`, not as queued tasks (n9 MinecraftServer
   2225–2245). Only a change made inside a queued task is deferred. (Two classes behind this
   are not dumped; see section 4.)
5. **`free_tickets` is omitted when 0 and defaults to 0** (the save format, section 2.2): a
   record without the field loads as fully claimed, not free. `Valid` defaults to false.
6. **A villager load builds the brain three times for an adult and four times for a baby**
   (finding 10): constructor, `LivingEntity` reading `Brain`, `AgeableMob.setAge` crossing zero
   for a baby (`ageBoundaryReached` → `refreshBrain`), `Villager.readAdditionalSaveData`'s
   `refreshBrain`. So 27 own draws for an adult, 36 for a baby.
7. The registry order of POI types ends `lodestone, test_instance, lightning_rod` (ids 18, 19,
   20); the key of the test block's type is `test_instance` (section 2.1 says
   `test_instance_block`, the block).

Everything else checked is confirmed, including the schedule table, `Holder$Reference`'s
identity hash and the query orders.

---------------------------------------------------------------------------------------

### 1. Points of interest: record, section, manager

#### Finding 1 (0, lines 46–52): "A point of interest is a counter… one ticket but the bell (32, range 6)"
**CONFIRMED.** n7 PoiRecord 1–60: fields `pos`, `poiType`, `freeTickets`, `setDirty` only;
the public constructor takes `maxTickets` (21–33). `acquireTicket` (61–79): `free <= 0` → false,
else `free -= 1`, `setDirty`, true. `releaseTicket` (80–98): `free >= max` → false, else
`free += 1`, `setDirty`, true. `hasSpace` = `free > 0`; `isOccupied` = `free != max` (so a type
with 0 tickets is never occupied, and a loaded record with `free > max` counts as occupied).
`equals`/`hashCode` by `pos` only (142–171). n7 PoiTypes bootstrap: 13 jobs and `home` (1, 1),
`meeting` (32, 6), then six with (0, 1). `GlobalPos` (n10 1–125) is a record of `dimension`
(`ResourceKey<Level>`) and `pos`, record `equals`/`hashCode`, codec `{dimension: <id string>,
pos: <BlockPos codec, int array>}`; `isCloseEnough(dim, pos, r)` = same dimension and
`pos.distChessboard(p) <= r` (45–62).

#### Section 2.1: types and ids
**CONFIRMED with one name corrected.** Registration order (n7 PoiTypes, bootstrap) and protocol
ids (datagen `reports/registries.json`):

| id | type | tickets | valid range |
|---|---|---|---|
| 0–12 | armorer, butcher, cartographer, cleric, farmer, fisherman, fletcher, leatherworker, librarian, mason, shepherd, toolsmith, weaponsmith | 1 | 1 |
| 13 | home | 1 | 1 |
| 14 | meeting | 32 | 6 |
| 15 | beehive | 0 | 1 |
| 16 | bee_nest | 0 | 1 |
| 17 | nether_portal | 0 | 1 |
| 18 | lodestone | 0 | 1 |
| 19 | test_instance | 0 | 1 |
| 20 | lightning_rod | 0 | 1 |

`#village` = `#acquirable_job_site` (the 13 jobs), `home`, `meeting` (datagen tags).

#### Finding 2 (0, lines 53–60): "every query goes through getOrLoad (Inferred: from disk, also for chunks that are not loaded)"
**CONFIRMED**, with the mechanism. n10 SectionStorage 211–245: `getOrLoad(s)`: outside the
build height → empty; `storage.get(s)` non-null → it; else `unpackChunk(chunk)` (341–410): if
the chunk is not yet in `loadedChunks`, it is added, and the pending read (a prefetch started at
chunk scheduling, n7 ChunkMap 1106–1108, or a new `tryRead`) is **joined on the server thread**
(70: `CompletableFuture.join`), then unpacked into the map. A chunk's POI data is read from
disk at most once per run (`loadedChunks` is never cleared). Unpack (436–500): no data → every
section `Optional.empty()`; data → each section y from min to max unpacked, then for each present
one `onSectionLoad(s)` and, if the data version changed, `setDirty(s)` (708–718).

The query reach: n7 PoiManager 86–107 `getInSquare` = `ChunkPos.rangeClosed(chunkOf(c),
floorDiv(r,16)+1)` flat-mapped through `getInChunk` (127–147: every section y from min to max,
`getOrLoad`, present ones only, `section.getRecords(type, occupancy)`), then `|dx| <= r &&
|dz| <= r` (797–822). `getInRange` adds `pos.distSqr(c) <= r*r` (782–796; `distSqr` between the
integer positions, f64). The stream is lazy: `find`, `take` and `findFirst` stop loading chunks
once they have their answer; `findAllClosestFirstWithType`'s sort and `getRandom`'s shuffle
load all. r = 48 → 9 × 9 chunks, confirmed.

#### Finding 3 (0, lines 61–73) and section 2.2's tracker bullet: "isVillageCenter… sectionsToVillage… level 0 at a source, +1 to each of the 26 neighbours, capped at 7… reach 6 sections, 96 blocks"
**CORRECTED** (the levels and the reach); `isVillageCenter`'s test **CONFIRMED**.

`isVillageCenter(s)` (n7 PoiManager 362–380, lambdas 616–633): `storage.get(s)` (not
`getOrLoad`); null → false; else `section.getRecords(h -> h.is(#village), IS_OCCUPIED).findAny()
.isPresent()`, an empty Optional → false. Since sections are never removed from `storage`
(n10 SectionStorage: the map is only ever `put`, lines 290–294, 450–455, 487–490; there is no
unload), "loaded" means "read at some time this run". **The brief's "a section's unload lowers
nothing until a later update" is moot: there is no unload.**

The tracker, exactly (n10 PoiManager$DistanceTracker 1–78, SectionTracker 1–150, n8
DynamicGraphMinFixedPoint 15–494):

| Item | Rule |
|---|---|
| Construction | `SectionTracker(7, 16, 256)`: `levelCount = 7`; levels are clamped to 0..6 everywhere (`Mth.clamp(x, 0, levelCount-1)`) |
| Stored levels | `Long2ByteOpenHashMap`, `defaultReturnValue = 7`; `setLevel(s, l)`: `l > 6` → remove, else put (l is always ≤ 6, so it always puts) |
| Source level | `getLevelFromSource(s) = isVillageCenter(s) ? 0 : 7` |
| From a neighbour | `computeLevelFromNeighbor(from, to, l) = isSource(from) ? getLevelFromSource(to) : l + 1` |
| Neighbours | the 26 sections `SectionPos.offset(s, dx, dy, dz)`, dx, dy, dz in −1..1 without (0,0,0); loops dx outer, dy, dz inner (SectionTracker 16–93, 0–116) |
| Propagation | after a **lowered** level `l`, neighbours are checked only if `l < levelCount − 2 = 5` (SectionTracker 0–15): a level-5 section passes nothing on |
| After a raise | the section is set to 6 (`levelCount − 1`), re-queued with its recomputed level if that is not 6, and its neighbours rechecked (DynamicGraphMinFixedPoint 95–166) |
| Recompute | `getComputedLevel(s, excluded, start)` = min(start, min over the 26 neighbours ≠ excluded of their level + 1, and the source term `getLevelFromSource(s)` unless the source is excluded), stopping early at 0 (SectionTracker 0–116) |
| Triggers | `update(s, getLevelFromSource(s), false)` at every `setDirty(s)` (n7 PoiManager 392–406) and `onSectionLoad(s)` (408–419); queued updates run all at once in `tick` (382–390) and before every `sectionsToVillage` (350–360) |
| Read | `sectionsToVillage(s)` returns the raw stored level: 0..5, 6 or 7 |

So the values mean: **0..5** = Chebyshev section distance (3-D) to the nearest occupied
`#village` section, if it is at most 5; **7** = never within 5 of a centre this run; **6** = was
within 5 once, and no centre is within 5 now (the raise path stores 6, and nothing ever removes
it until the server stops; the map is not saved). Consequences:

- `isVillage(p)` = level ≤ 1; `isCloseToVillage(p, 2)` (cats, patrols) = ≤ 2; `isCloseToVillage
  (p, 6)` (n7 MoveThroughVillageGoal 59–61, StrollThroughVillageGoal 69–71) = ≤ 5 **or the
  residue 6**. `isCloseToVillage(p, r)` with r > 6 is false (n7d ServerLevel 3387–3427,
  confirmed).
- Rules comparing `sectionsToVillage` values (`GoToClosestVillage`'s `−sectionsToVillage` score,
  `findSectionClosestToVillage`'s "least below the own") see 6 and 7 as different: a former
  village's ring attracts more than untouched land.
- The tracker's reach is 5 sections: a level depends on centres within 5 sections, i.e. up to
  95 blocks from the record on an axis (5 × 16 + 15), not "6 sections, 96 blocks".

#### Finding 4 (0, lines 74–83): updates on block change, structure registration, the load check
- "does nothing when `forState(old) == forState(new)`… else removes the old type and adds the
  new, each in `server.execute`" — **CONFIRMED** (n7d ServerLevel 3351–3378, lambdas 4604–4651:
  `$0` executes `$1` = `PoiManager.remove(pos)` and drops the debug POI; `$2` executes `$3` =
  `PoiManager.add(pos, type)`; the remove is scheduled before the add).
- "(Inferred: at once within the world tick)" — **CORRECTED to: at once, from the world tick and
  from packet handling alike** (section 4 below), deferred only inside a queued task.
- "Structure generation registers them with full tickets (WorldGenRegion.txt 716)" — not re-read
  here; `PoiSection.add(pos, type)` (n7 PoiSection 78–105) always makes a record with
  `maxTickets`, so any add has full tickets.
- "At a chunk's load checkConsistencyWithBlocks rebuilds an invalid or missing section from the
  blocks with full tickets… a rebuild silently frees every claim" — **CORRECTED**:
  - n10 SerializableChunkData 441–560 (`read`): for each saved section in the chunk file that has
    block data (`chunkSection != null`), in file order, `poiManager.checkConsistencyWithBlocks(
    sectionPos, chunkSection)` (172–180). Chunks generated fresh are never checked.
  - n7 PoiManager 421–437 and lambdas 572–614: `getOrLoad(s)` (which reads the POI file);
    **present** → `section.refresh(...)`; **absent** → if `mayHavePoi(section)` (the palette
    may hold a POI state, 439–444) `getOrCreate(s)` (a new, valid section) and every POI block
    in it added through `section.add` (full tickets, `setDirty` each).
  - n7 PoiSection 281–305 `refresh`: **does nothing when `isValid`**. When invalid: copy the
    records map, clear, then for every block of the section (`SectionPos.blocksInside`, n7
    PoiManager 446–454, 542–570) whose state has a POI type: `computeIfAbsent(relativePos, new
    record with full tickets)` on the **old** map, and add that record. So an old record at a
    position that still has a POI block is kept **with its tickets and its old type, even if the
    block's type changed**; a record where no POI block is now is dropped; new positions get
    full tickets. Then `isValid = true`, `setDirty`.
  - A new section (`new PoiSection(Runnable)`, PoiSection 13–20) is valid. A loaded one's
    `Valid` comes from the file (default false).
  So tickets are lost only if the POI file lost the section while the chunk kept its blocks.
  The brief's port note stands in spirit (the tickets must travel with the chunk), but the game
  never discards tickets on an ordinary load.

#### Finding 5 (0, lines 84–90): "Removing a record tells nobody… Releases come from anywhere"
**CONFIRMED.** n7 PoiSection 154–197: remove from `records`; none → logs "POI data mismatch:
never registered" and returns; else remove from its type's set (the empty set stays in
`byType`), log, `setDirty`. `PoiManager.release(pos)` (292–306) throws `IllegalStateException`
when the section is absent, and `PoiSection.release` (213–240) throws when the record is absent;
`Villager.releasePoi` (n7 Villager 1205–1227, 2117–2161) guards this: `server.getLevel(gp.
dimension())` (null → nothing), `getType(pos)` (through `getOrLoad`, reading disk), and only if
present and accepted by the memory's predicate does it `release`. A release of a record already
at max is a no-op returning false.

#### Finding 6 (0, lines 91–97) and q15: "Query order is a hash order… (Inferred identity-hashed holders…)"
**CONFIRMED.** n10 Holder$Reference 1–290 overrides neither `hashCode` nor `equals`: identity
(`Object.hashCode`), so `PoiSection.byType` (a `HashMap<Holder, Set<PoiRecord>>`, n7
PoiSection 22–31) iterates types in an order that differs between JVM runs (stable within a
run). The order within a query, in full:

1. chunks: `ChunkPos.rangeClosed(min corner, max corner)` (n7 ChunkPos 458–530, ChunkPos$2
   33–92): **x fastest, then z**, both ascending;
2. within a chunk: section y ascending from the minimum (n7 PoiManager 127–147);
3. within a section: types in `HashMap` order of identity hashes (varies per run);
4. within a type: a `HashSet<PoiRecord>` (`Sets.newHashSet`, PoiSection 354–357) keyed by
   `BlockPos.hashCode` (PoiRecord 166–171): deterministic given the set's insertion history
   and capacity, not random.

The brief's fixed order (chunk x then z, section y, type id, position) matches 1–2 exactly and
replaces 3–4; parity there is by distribution, as recommended.

#### Section 2.2: `Occupancy`
**CONFIRMED in names and order**; the two tests **STILL UNKNOWN in the strict sense**. n10
PoiManager$Occupancy: `HAS_SPACE` (ordinal 0, indy #0), `IS_OCCUPIED` (1, indy #1), `ANY` (2,
indy #2); the only synthetic lambda is `lambda$static$0` = `true`, used by `ANY`. The other two
are method references, which `-c` does not show; that they are `PoiRecord::hasSpace` and
`PoiRecord::isOccupied` is all but certain (the names, and the absence of other lambdas). A
`javap -v` of the class would settle it.

#### Section 2.2: the save format ("`(Valid, Records)` (Inferred field names; `$Packed` not dumped)")
**CORRECTED in detail.** The POI file (region storage `POI_CHUNK`, n7 PoiManager 11–41), per
chunk (n10 SectionStorage 541–620):

```
{
  "Sections": {                 // keys: Integer.toString(section y); every section whose
    "<y>": {                    // storage entry is present (an empty section is written too)
      "Valid":   <bool>,        // lenientOptionalFieldOf, default false (written only when true)
      "Records": [              // fieldOf: required
        { "pos":  <BlockPos codec: int array [x, y, z]>,
          "type": "<point_of_interest_type id>",       // RegistryCodecs.holder
          "free_tickets": <int> }                      // optionalFieldOf, default 0: OMITTED when 0
      ]
    }
  },
  "DataVersion": <int>
}
```

(n10 PoiSection$Packed `lambda$static$0`: `Valid` `Codec.BOOL.lenientOptionalFieldOf("Valid",
false)`, `Records` `PoiRecord$Packed.CODEC.listOf().fieldOf("Records")`; n10 PoiRecord$Packed
`lambda$static$0`: `pos` `BlockPos.CODEC.fieldOf`, `type` `RegistryCodecs.holder(
POINT_OF_INTEREST_TYPE).fieldOf`, `free_tickets` `Codec.INT.optionalFieldOf(..., 0)`.) Records
are written in the section's `Short2ObjectOpenHashMap` value order (PoiSection 46–60). A record
with no `free_tickets` is **fully claimed** on load; a claimed bed or job site is saved without
the field.

Writing: dirty chunks (a `LongLinkedOpenHashSet`, insertion order) are written in
`SectionStorage.tick` while the time supplier allows (88–112), all at `flushAll`, and one chunk
at `ChunkMap.save` (`flush`, 664–675). `tick` also unpacks finished prefetches (114–172), so a
chunk's POI sections become sources at the next `PoiManager.tick` after the read completes, not
when the chunk finishes loading.

### 2. The schedule

#### Finding 7 (schedule part) and section 1.2: the table and "a keyframe's value holds until the next and switches on its tick"
**CONFIRMED**, with the mechanism.
- `timeline/villager_schedule.json`: clock `minecraft:overworld`, `period_ticks` 24000, no
  `ease` (so `LINEAR`, the default: n10 KeyframeTrack 180–197), keyframes as below.
- n8 AttributeTrack 48–63: the sampler's lerp is the modifier's `argumentKeyframeLerp`; with no
  modifier (override) that is `attribute.type().keyframeLerp()` (n7 AttributeModifier$
  OverrideModifier); `ofNotInterpolated` sets `keyframeLerp = LerpFunction.ofStep(1.0f)` (n7
  AttributeType 82–101), and `ofStep(t)(alpha, from, to) = alpha >= t ? to : from` (n7
  LerpFunction 116–125).
- n10 KeyframeTrackSampler 26–112 (`bakeSampler` with a period): segments `[last.ticks − period →
  first.ticks]`, then each consecutive pair, then `[last.ticks → first.ticks + period]`.
  `sample(t)` (153–208): `t' = floorMod(t, period)`; the first segment with `t' < toTicks`
  (else the last); `t' <= fromTicks` → from; `t' >= toTicks` → to; else
  `lerp(easing((t' − from)/(to − from)) as f32, from, to)`. With `ofStep(1.0)` and linear easing
  the middle case returns `from` (alpha < 1; the longest segment here is 12010 ticks, so f32
  rounding never reaches 1.0). So each keyframe's value holds from its tick until the next
  keyframe's tick, and the last's wraps to the first's tick of the next day.

| `floorMod(totalTicks, 24000)` | adult (`villager_activity`) | baby (`baby_villager_activity`) |
|---|---|---|
| 0–9 | rest (from 12000 the day before) | rest |
| 10–1999 | idle | idle |
| 2000–2999 | work | idle |
| 3000–5999 | work | play |
| 6000–8999 | work | idle |
| 9000–9999 | meet | idle |
| 10000–10999 | meet | play |
| 11000–11999 | idle | play |
| 12000–23999 | rest | rest |

Keyframes, as data: adult `10 idle, 2000 work, 9000 meet, 11000 idle, 12000 rest`; baby
`10 idle, 3000 play, 6000 idle, 10000 play, 12000 rest`. Clustine's `scheduled` (`clustine-sim/
src/mobs/types/villager.rs` 275–298) gives the same; its comment's **Inferred** can be dropped.
(`KeyframeTrackSampler$Segment` and `EasingType` are not dumped; that the segment takes the
track's easing and `LINEAR` is the identity are the only unread links, and neither can change
the result under `ofStep(1.0)` unless an easing reached 1.0 before the segment's end.)

### 3. The brain rebuild at load

#### Finding 10 (0, lines 115–120) and q14: "A load builds it three times (27 own draws)"
**CORRECTED for babies.** The builds on a load of a villager:
1. the constructor (`LivingEntity.<init>` calls `makeBrain`, n9 LivingEntity 350);
2. `LivingEntity.readAdditionalSaveData` reading `Brain` → `makeBrain(packed)` (n9 LivingEntity
   1792–1800, 11555–11562), when the tag is there (it always is in a saved villager);
3. **for a baby only**: `AgeableMob.readAdditionalSaveData` (n7 AgeableMob 357–380) calls
   `setAge(Age)` after `super`; `setAge` (287–315) calls `ageBoundaryReached` when the sign
   changes, and a new entity's age is 0 (≥ 0), so a negative saved age crosses → `Villager.
   ageBoundaryReached` (n7 Villager 167–181) → `refreshBrain`;
4. `Villager.readAdditionalSaveData` ends with `refreshBrain` (n7 Villager 832–920, at 195).

`refreshBrain` (121–142): `stopAll`, `pack` (the memories with codecs), `makeBrain`,
`registerBrainGoals` (schedule set, `updateActivityFromSchedule` once). With nine sensors
drawing own `nextInt(rate)` per build: **27 own draws for an adult, 36 for a baby**. The rest
of the finding (rebuilt at every age-sign change and profession change, only codec memories
kept) is confirmed as far as `refreshBrain` shows it.

### 4. `server.execute` from the server thread

**CORRECTED** (finding 4's Inferred). n10 BlockableEventLoop 137–153: `execute(r)`: wrap
(`MinecraftServer.wrapRunnable` → `TickTask(tickCount, r)`, n9 MinecraftServer 1936–1944); if
`scheduleExecutables()` → queue (`pendingRunnables.add`, unpark) else `doRunTask` **at once**.
The base `scheduleExecutables` is `!isSameThread()` (50–58); `MinecraftServer.scheduleExecutables`
= `ReentrantBlockableEventLoop.scheduleExecutables() && !isStopped()` (n9 3207–3219).
`ReentrantBlockableEventLoop` is **not dumped**: by memory it is `runningTask() ||
super.scheduleExecutables()`, with `runningTask` = a re-entrance count raised inside
`doRunTask` (`MinecraftServer.haveTime` calls `runningTask()`, n9 1846–1866, consistent with
that).

So on the server thread the POI change runs at once unless the caller is itself inside a queued
task. In 26.3 the world tick and packet handling both run outside tasks: `processPacketsAndTick`
calls `packetProcessor.processQueuedPackets()` then `tickServer` directly from `runServer` (n9
MinecraftServer 2225–2245, 1540–1566); queued tasks run only in `waitUntilNextTick`
(`runAllTasks`, then `managedBlock`, 1874–1890). `PacketProcessor` is **not dumped**; that it
runs handlers inline is inferred from its being called there. A block change made inside a
queued task (a scheduled callback) would have its POI update appended to the queue and run later
in the same wait, before the next tick.
**What would settle it**: dump `net.minecraft.util.thread.ReentrantBlockableEventLoop` and
`net.minecraft.network.PacketProcessor`. For Clustine the rule to port is "at once, at the block
change", which both readings give for every ordinary case.

### 5. `SectionPos.cube` order

**CONFIRMED** (section 1.5's "the first of equals in `SectionPos$1` order", section 7.2's
5 × 5 × 5 cube). n7 SectionPos 578–608: `cube(s, r)` = `betweenClosedStream(x−r, y−r, z−r,
x+r, y+r, z+r)`; SectionPos$1 iterates a `Cursor3D`; n10 Cursor3D 79–120: `x = index % width`,
`y = (index / width) % height`, `z = (index / width) / height`. **x fastest, then y, then z**,
each ascending from the minimum.

### 6. Saved data

`SavedData` (n10, whole class): only a `dirty` flag. `SavedDataStorage` (n10):
- `get(type)` (89–114): from the cache, else `readSavedData` once (cached as present or absent).
  `computeIfAbsent(type)` (67–88): `get`, else `type.constructor()` and `set`, which **marks the
  new value dirty** (169–181), so a type touched once is written at the next save even unchanged.
- The file (41–65): `Identifier.withSuffix(".dat").resolveAgainst(dataFolder)`, refused outside
  the folder (`resolveAgainst` is not dumped; by memory `<folder>/<namespace>/<path>`). Read
  (116–167): if the file exists, `readTagFromDisk` (gzip detected, data-fixed with the type's
  `DataFixTypes`), then `type.codec().parse(tag.get("data"))`, partial results logged; any
  exception → logged, absent.
- Write (396–445): `{ "data": codec.encodeStart(value), "DataVersion": <current> }`
  (`NbtUtils.addCurrentDataVersion`), `NbtIo.writeCompressed`, parent directories created;
  dirty values are collected and cleared (472–500) at `scheduleSave` (asynchronously, chained
  after the previous write) or `saveAndJoin`. A level saves its storage in `saveLevelData` (n7d
  ServerLevel 1897–1909).
- `Raids` (n7k Raids 400–465): id `minecraft:raids`, `DataFixTypes.SAVED_DATA_RAIDS`, fields
  `raids` (optional list), `next_id`, `tick`. It is **per level**: every `ServerLevel` does
  `getDataStorage().computeIfAbsent(Raids.TYPE)` in its constructor (n7d ServerLevel 354–368),
  and a level's storage is its chunk source's (3260–3266); the server-wide storage is
  `MinecraftServer.getDataStorage()` (3271–3295, used for other types). This confirms finding
  26's "per-level world state".
- `WanderingTraderData` (n10, for the spawners' reader): id `minecraft:wandering_trader`,
  `SAVED_DATA_WANDERING_TRADER`, `spawn_delay` and `spawn_chance` optional fields.

### 7. Section 11 and the open questions

- **Section 11, first bullet** ("the village tracker 96 in three dimensions… the village level of
  every section within six sections"): **CORRECTED** to five sections (up to 95 blocks from the
  record), plus the residue level 6 that only the tracker's history explains. "In the game a
  point of interest in a chunk that is not loaded is read from disk": **CONFIRMED** (section 1
  above), once per chunk per run, blocking the server thread, and kept in memory afterwards.
- **q1** (where points of interest live): facts the plan can use. (i) The game trusts a valid
  POI section and never re-derives it from blocks; Clustine deriving records from blocks at
  every load differs only where file and blocks disagree (external edits, a lost file), and in
  that case the game keeps old tickets at matching positions too. (ii) The tickets are the only
  state not derivable, and `free_tickets` saves as "absent = 0". (iii) Village sources in the
  game are every section with an occupied `#village` record read this run, in loaded chunks or
  not: a store-wide index of occupied sections is a faithful stand-in (it differs only for
  sections never read since start, which the game ignores). (iv) Levels need sources within 5
  sections, so a margin of five sections (80 blocks plus the section) suffices for the tracker;
  the 72-block query reach (finding 2) remains the larger need for records.
- **q14** (a load's brain): the counts are 3 builds / 27 draws (adult) and 4 / 36 (baby).
- **q15** (query order): confirmed as section 1, finding 6 above; the fixed order is exact for
  chunks and section y and a stand-in only for the type and within-type order.

---------------------------------------------------------------------------------------

## Part B: the social life and trading

Brief: `docs/groundwork/n10-brief.md` sections 0 (findings 16–25), 4, 5, 6, 10 (trading rows),
16 q5, q6, q8. Dumps: `/home/claude/reference/n10-javap/` ("n10 X lines a–b" are line numbers in
that file; `Villager-v` and `VillagerGoalPackages-v` are the `javap -v` files). Data:
`target/datagen/26.3/generated/`.

### Summary of the corrections

1. **The bell (finding 19, 4.5).** `onHit` touches no entity: it sets `clickDirection`, starts or
   restarts the shake, and queues block event `(1, direction)`. Everything else happens when that
   block event runs (`triggerEvent`):
   - **Who hears it.** Every `LivingEntity` in the block's box grown by 48 is listed. Those strictly
     closer than 32 to the block's centre (3-D Euclidean) get `heard_bell_time = gameTime`, but only
     brains with that memory slot keep it. In the dumps that is only the villager.
   - **The list is kept.** It is taken again only when `gameTime > lastSearch + 60`. A second ring
     within 60 ticks reuses the old list: it re-checks the old entities' current positions and adds
     no newcomers.
   - **Resonance.** It starts on the bell's 5th tick after the event, and only if a living raider
     from that list is **within 32**, not 48. It starts at most once per ring.
   - **The glow.** It comes 40 ticks later, on the 45th tick, to raiders from the same list that are
     within 48. It is `GLOWING` for 60 ticks.
   - **A new sound.** The resonance plays `bell_resonate` (blocks, 1, 1) from the server.
2. **Golem summoning (finding 17).** On success, `golem_detected_recently` (599 ticks) goes into
   **every villager in the box**, the summoner's bounding box grown by 10. That includes those that
   did not want a golem, not only the up to five that agreed. The "wants" test also requires the
   summoner itself to want one first.
3. **A trade's price at generation (5.2).** The `additional_trade_cost` that the enchant functions
   write is the only part that is doubled, not the wanted count. It is doubled only when the result
   has **`stored_enchantments`** (books; `enchantments` does not count) with any enchantment in the
   trade's `double_trade_price_enchantments`. All modifiers run before any cost is built. The base
   count and the addition meet in `TradeCost.toItemCost`, which is not dumped (clamp unknown).
   Draw order: predicate, modifiers, wanted count, additional wants, max uses, xp, discount. In the
   vanilla data every count, max_uses, xp and discount is a constant, so only the predicate and the
   modifiers draw.
4. **The wire (5.6).** In each offer, uses, max uses, xp, special price and demand are **fixed 4-byte
   ints** (`writeInt`), and the multiplier is a 4-byte float. The level and xp after the list are
   var-ints. An `ItemCost` is item id (var-int), count (var-int), then the component list. The
   result is a non-optional `ItemStack.STREAM_CODEC`.
5. **`MerchantContainer.updateSellItem` (5.6).**
   - **Which slot is A.** With slot 0 empty, slot 1 is taken as A and B is empty.
   - **Both slots empty.** The result is cleared and the method returns **without
     `notifyTradeUpdated`**.
   - **A leftover active offer.** When `(a,b)` finds an out-of-stock offer, `activeOffer` is set to
     it before `(b,a)` is tried. If `(b,a)` also fails, `activeOffer` keeps that out-of-stock offer
     while the result slot is empty.
6. **`MerchantMenu.tryMoveItems` (5.6).**
   - **Putting the payments back.** Each payment goes back with `moveItemStackTo(3, 39, reverse)`,
     hotbar end first.
   - **When it stops.** If either payment does not fit entirely, the method returns at once and
     fills nothing. Filling happens only when both payment slots are then empty.
   - **How much it fills.** Each payment slot is filled up to a **full stack**, not the price. It
     merges every matching stack in slots 3–38 in order.
7. **Hero gifts (4.5).** There is **no nitwit table**. Babies get `baby_gift`. A profession without a
   key or without an entry in the map (`none`, `nitwit`) gets `unemployed_gift`.
8. **`GOLEM_KILLED` (4.1).** Besides being raised nowhere, the villager has no branch for it, so it
   writes nothing.
9. **Explorer maps (finding 25, 5.7).** **10** of the cartographer's 31 trades carry
   `exploration_map`, not 12.
10. **`ItemCost.test` is a subset match (5.7).** The item must match, and each listed component
    must equal the stack's (`Objects.equals`). Components not listed are ignored.
11. **Price arithmetic types (finding 23).** The demand bonus is
    `max(0, Mth.floor((float)(base*demand) * priceMultiplier))`, in f32. Demand can be negative, and
    `updateDemand` never clamps it.

Everything else in the area is CONFIRMED, and the gossip constants are exactly the Inferred ones.

---------------------------------------------------------------------------------------

### 1. Gossip and reputation (findings 16–17, section 4.1, 4.4)

**4.1 "`GossipType`, not dumped; Inferred values … major_negative (−5, 100, 10, 10) …"**:
**CONFIRMED** exactly (n10 GossipType lines 103–166). Also constants `REPUTATION_CHANGE_PER_EVENT =
25`, `…_PER_EVERLASTING_MEMORY = 20`, `…_PER_TRADE = 2` (13–17). The ordinals and the codec are
`StringRepresentable.fromEnum` by `id`.

Table (constructor `(name, ordinal, id, weight, max, decayPerDay, decayPerTransfer)`):

| ordinal | id | weight | max | decayPerDay | decayPerTransfer |
|---|---|---|---|---|---|
| 0 | `major_negative` | −5 | 100 | 10 | 10 |
| 1 | `minor_negative` | −1 | 200 | 20 | 20 |
| 2 | `minor_positive` | 1 | 25 | 1 | 5 |
| 3 | `major_positive` | 5 | 20 | 0 | 20 |
| 4 | `trading` | 1 | 25 | 2 | 20 |

So major_positive never decays and never transfers (20 − 20 = 0 < 2). Trading transfers only from a
value of at least 22.

**Finding 16, "returns while either villager is within 1200 of its lastGossipTime"**: **CONFIRMED**
(n10 Villager-v lines 5063–5155). It returns when `last ≤ t < last + 1200` for `this` **or** for
`target`; a `t` before `last` counts as cool. Then `this.gossips.transferFrom(target.gossips,
this.random, 10)`: the caller is the receiver, using its own random. Both are stamped with `t`, then
`spawnGolemIfNeeded(level, t, 5)`. If `this` has a trading player and the count transferred is > 0,
`updateSpecialPrices`.

**4.1 `maybeDecayGossip`**: CONFIRMED (5156–5208). With `lastGossipDecayTime == 0` it only stamps;
otherwise it does nothing until `t ≥ last + 24000`, then `decay()` and stamps.

**Finding 17 / 4.4**: **CORRECTED** in one point (n10 Villager-v 5209–5302, 5303–5341, 5557–5587,
5731–5773; bootstrap #10 at 6256ff).

1. Return unless `this.wantsToSpawnGolem(t)`. That is `golemSpawnConditionsMet(level.getGameTime())`
   (`last_slept` present and `gameTime − lastSlept < 24000`) and no `golem_detected_recently`. Note
   the conditions read `level.getGameTime()`, not `t`; they are the same value.
2. `nearby = level.getEntitiesOfClass(Villager, this.boundingBox.inflate(10, 10, 10))`. This
   includes `this`, in the level's entity order.
3. `agree = nearby.stream().filter(v -> v.wantsToSpawnGolem(t)).limit(5).toList()`. Return if
   `agree.size() < n` (n = 5 from gossip, 3 from panic).
4. `SpawnUtil.trySpawnMob(IRON_GOLEM, MOB_SUMMONED, level, this.blockPosition(), 10, 8, 6,
   LEGACY_IRON_GOLEM, false)`. Return if empty.
5. **`nearby.forEach(GolemSensor::golemDetected)`**: every villager in the box, wanting or not, gets
   `golem_detected_recently = true` with expiry 599 (n7 GolemSensor `golemDetected`).

**Reputation events (4.1 list)**: CONFIRMED (n10 Villager-v 5342–5444). The event types are
registered in the order `zombie_villager_cured`, `golem_killed`, `villager_hurt`, `villager_killed`,
`trade` (n10 ReputationEventType 21–38). `ReputationEventHandler` has the single method
`onReputationEventFrom(type, entity)`; only `Villager` implements it in the dumps.

| Event | Writes into the villager's gossips about `source.getUUID()` | Raised by |
|---|---|---|
| `ZOMBIE_VILLAGER_CURED` | `major_positive` +20, then `minor_positive` +25 | the cure's end |
| `TRADE` | `trading` +2 | `customServerAiStep` after the brain, when `lastTradedPlayer` is set (Villager-v 2363–2410): event, then entity event 14, then `lastTradedPlayer = null` |
| `VILLAGER_HURT` | `minor_negative` +25 | `setLastHurtByMob` (Villager-v ~3915–3935) |
| `VILLAGER_KILLED` | `major_negative` +25 | witnesses |
| `GOLEM_KILLED` | **nothing** (no branch) | nowhere in the dumps |

After any of them: if the trading player is not null and its UUID equals the source's,
`updateSpecialPrices(tradingPlayer)`.

**Witnesses**: CONFIRMED (Villager-v 4044–4113, lambda 5913–5932). The dying villager's
`nearest_visible_living_entities` memory: if absent, nothing. Otherwise
`findAll(ReputationEventHandler.class::isInstance)`, so villagers only, each given
`level.onReputationEvent(VILLAGER_KILLED, murderer, witness)`.

### 2. Bells (finding 19, section 4.5)

**"`BellBlockEntity.onHit` (not dumped; Inferred: every living entity within 32 gets
`heard_bell_time`, raiders within 48 glow after the resonance)"**: **CORRECTED**. The rule, exactly
(n10 BellBlockEntity):

- **Constants (3–17).** `DURATION 50`, `GLOW_DURATION 60`, `MIN_TICKS_BETWEEN_SEARCHES 60`,
  `MAX_RESONATION_TICKS 40`, `TICKS_BEFORE_RESONATION 5`, `SEARCH_RADIUS 48`,
  `HEAR_BELL_RADIUS 32`, `HIGHLIGHT_RAIDERS_RADIUS 48`. None of the fields is saved.
- **`onHit(dir)` (159–187).** It sets `clickDirection`. If already shaking, `ticks = 0`, else
  `shaking = true`. Then `level.blockEvent(pos, block, 1, dir.get3DDataValue())`.
- **`triggerEvent(1, data)` (42–68)**, when the block event runs: `updateEntities()`,
  `resonationTicks = 0`, `clickDirection = from3DDataValue(data)`, `ticks = 0`, `shaking = true`.
- **`updateEntities` (189–262).**
  - If `gameTime > lastRingTimestamp + 60` or the list is null, it sets `lastRingTimestamp =
    gameTime` and `nearbyEntities = getEntitiesOfClass(LivingEntity, new AABB(pos).inflate(48))`.
  - On the server, for each listed entity that is alive and not removed with
    `pos.closerToCenterThan(entity.position(), 32.0)` (distance squared to the block centre `< 32²`,
    strict), it calls `entity.getBrain().setMemory(HEARD_BELL_TIME, gameTime)`.
  - `Brain.setMemory` ignores a type the brain has no slot for (n7 Brain 362–410). In the dumps
    `HEARD_BELL_TIME` is read only by `ReactToBell` and `SetHiddenState`, so in effect only
    villagers.
- **`tick` (70–137), server and client alike, each block-entity tick.**
  1. If shaking, `ticks++`. If `ticks ≥ 50`, `shaking = false` and `ticks = 0`.
  2. If `ticks ≥ 5`, `resonationTicks == 0` and `areRaidersNearby`, then `resonating = true` and
     `playSound(null, pos, BELL_RESONATE, BLOCKS, 1, 1)`. `areRaidersNearby` (264–297) asks for any
     listed entity that is alive, not removed, within **32** and in `#raiders`.
  3. If resonating: when `resonationTicks < 40`, `resonationTicks++`; otherwise the end action runs
     and `resonating = false`.
- **Timing.** Counting the bell's ticks after the event as k = 1, 2, …: resonance and the sound at
  k = 5, the end action at k = 45. `resonationTicks` stays 40 until the next `triggerEvent`, so
  there is one resonance per ring.
- **The end action on the server is `makeRaidersGlow` (299–308).** For each listed entity passing
  `isRaiderWithinRange` (338–359: alive, not removed, within **48**, in `#raiders`), `addEffect(new
  MobEffectInstance(GLOWING, 60))` (361–371).
  - `-c` cannot show which method reference is the server's and which the client's end action
    (bootstrap #0, #1). `showBellParticles` only adds client particles, so the server one is
    `makeRaidersGlow` by elimination.
  - Raiders who came within 48 after the list was taken do not glow.
- **Section 10 row "a bell"** gains `bell_resonate` (block sound) and the raiders' glowing as an
  effect (shared flag bit 6).

`RingBell` and `BellBlock` are not in this dump; nothing here changes them.

### 3. Food and gifts (4.3, 4.5)

**4.3 "`villager_food` nutrition: bread 4, carrot, potato and beetroot 1"**: **CONFIRMED**.
`VillagerFood` is a record `{nutrition}`, codec `POSITIVE_INT` field `nutrition`, stream a var-int
(n10 VillagerFood 1–68). The values come from item components: `reports/minecraft/components/item/
{bread,carrot,potato,beetroot}.json` give 4, 1, 1, 1, and no other item has the component.

**4.5 `GiveGiftToHero` tables, "(baby, unemployed and nitwit tables apart)"**: **CORRECTED** (n10
BuiltInLootTables 143–173, 518–565; n7 GiveGiftToHero 172–193, 305–347). The paths are below. There
is no nitwit table.
- **Baby:** `gameplay/hero_of_the_village/baby_gift`.
- **Otherwise, by profession key.** Armorer, butcher, cartographer, cleric, farmer, fisherman,
  fletcher, leatherworker, librarian, mason, shepherd, toolsmith and weaponsmith each map to
  `gameplay/hero_of_the_village/<profession>_gift`.
- **Fallback.** A profession without a key, or with a key outside that map (`none`, `nitwit`), gets
  `gameplay/hero_of_the_village/unemployed_gift`.

The cat's table is `gameplay/cat_morning_gift`. All 15 gift files and `cat_morning_gift.json` are in
`data/minecraft/loot_table/gameplay/`.

### 4. Merchant offers: fields, codec, price, demand (findings 22–23, 5.1, 5.4, 5.5)

**`MerchantOffer` fields (n10 MerchantOffer 7–25).** `baseCostA: ItemCost`, `costB:
Optional<ItemCost>`, `result: ItemStack`, `uses: int`, `maxUses: int`, `rewardExp: boolean`,
`specialPriceDiff: int`, `demand: int`, `priceMultiplier: float`, `xp: int`. Only `uses`,
`specialPriceDiff` and `demand` are mutable.

Constructors (27–143):
- **The main one:** `(a, b, result, uses, maxUses, rewardExp, special, demand, mult, xp)`.
- **Trades use** `(a, b, result, maxUses, xp, mult)`, which gives uses 0, rewardExp true, special 0,
  demand 0.
- **The network decode uses** `(a, b, result, uses, maxUses, xp, mult, demand)`, which gives
  rewardExp true and special 0, then sets special and, if the out-of-stock flag is set,
  `setToOutOfStock` (`uses = maxUses`).
- **`copy()`** copies every field and copies the result stack.

**The saved codec (5.1 "Inferred also buyB, specialPrice, demand, priceMultiplier, xp")**:
**CONFIRMED** with defaults (551–622). It is a `RecordCodecBuilder` in this order:

| key | codec | default |
|---|---|---|
| `buy` | `ItemCost.CODEC` | required |
| `buyB` | `ItemCost.CODEC`, `lenientOptionalFieldOf` | absent |
| `sell` | `ItemStack.CODEC` | required |
| `uses` | int, lenient optional | 0 |
| `maxUses` | int, lenient optional | 4 |
| `rewardExp` | bool, lenient optional | true |
| `specialPrice` | int, lenient optional | 0 |
| `demand` | int, lenient optional | 0 |
| `priceMultiplier` | float, lenient optional | 0.0 |
| `xp` | int, lenient optional | 1 |

"Lenient" means a malformed value reads as the default instead of failing. `MerchantOffers` is
`{Recipes: [offer…]}`, optional with default empty (n7k MerchantOffers 99–110).

**`ItemCost` (n10 ItemCost).**
- **Fields:** `item: Holder<Item>`, `count`, `components: DataComponentExactPredicate`, and a
  precomputed `itemStack = new ItemStack(item, count, components.asPatch())` (83–92).
- **Codec (154–179):** `id` (`Item.CODEC`, required), `count` (`POSITIVE_INT`,
  `optionalAlwaysPresentFieldOf`, default 1, always written), `components`
  (`DataComponentExactPredicate.CODEC`, optional, default `EMPTY`).
- **Stream (181–199):** `Item.STREAM_CODEC`, `VAR_INT` count, `DataComponentExactPredicate.
  STREAM_CODEC`; `OPTIONAL_STREAM_CODEC` applies a codec operation (bootstrap #6, `-c` does not name
  it; by its type, the usual boolean-prefixed optional).
- **`test(stack)` (94–109):** `stack.is(item) && components.test(stack)`.

**`DataComponentExactPredicate` (n10 lines 1–244).** A list of `TypedDataComponent`.
- **`test` (125–152)** is true when each expected component's value `Objects.equals` the stack's
  value for that type. Components it does not list are not checked.
- **Codec (224–230):** `DataComponentType.VALUE_MAP_CODEC` (a map from component id to value),
  dropping transient types when encoding (lambda 198–207).
- **Stream (231–237):** `TypedDataComponent.STREAM_CODEC` as a list (var-int count, then each
  type's id and value).

**The price formula (finding 23 "Inferred `clamp(base + max(0, floor(base · demand · multiplier)) +
special, 1, maxStack)`")**: **CONFIRMED**, with the types (n10 MerchantOffer 152–192):

```
getCostA() = baseCostA.itemStack().copyWithCount(modified(baseCostA))
modified(c) = Mth.clamp(c.count() + max(0, Mth.floor((float)(c.count() * demand) * priceMultiplier))
                        + specialPriceDiff,
                        1, c.itemStack().getMaxStackSize())
```

- **f32.** `count * demand` is an int product, made a float and multiplied by the f32 multiplier,
  then `Mth.floor(float)`.
- **Cost B is never modified.** `getCostB()` is `costB.map(ItemCost::itemStack)` with
  `ItemStack.EMPTY` otherwise; its count is the base count.
- **The worked example in 5.4 is right.** `floor(12f * 0.2f) = floor(2.4000001) = 2`, then
  `4 + 2 − 5 = 1`.

**Other methods**:

| Method | Rule | n10 MerchantOffer lines |
|---|---|---|
| `updateDemand()` | `demand = demand + uses − (maxUses − uses)`, no clamp (**CONFIRMED** 5.5, finding 22) | 223–238 |
| `resetUses()` | `uses = 0` | 253–258 |
| `increaseUses()` | `uses++` | 266–274 |
| `isOutOfStock()` | `uses ≥ maxUses` | 324–334 |
| `needsRestock()` | `uses > 0` | 344–352 |
| `addToSpecialPriceDiff(d)` | `special += d` | 282–290 |
| `resetSpecialPriceDiff()` | `special = 0` | 292–297 |
| `setSpecialPriceDiff(d)` | `special = d` | 305–310 |
| `assemble()` | `result.copy()` | 240–245 |
| `satisfiedBy(a, b)` | see below | 360–401 |
| `take(a, b)` | see below | 403–427 |

- **`satisfiedBy(a, b)`.** It needs `baseCostA.test(a)` and `a.count ≥ modified(A)`. Then, if
  `costB` is present, `costB.test(b)` and `b.count ≥ costB.count`; otherwise `b.isEmpty()`.
- **`take(a, b)`.** It returns false unless satisfied. Otherwise `a.shrink(getCostA().count)` and, if
  `getCostB()` is not empty, `b.shrink(costB.count)`; then true.

### 5. Drawing an offer: `VillagerTrade` and its context (finding 20, 5.2, finding 25)

**Fields (n10 VillagerTrade 5–21).** `wants: TradeCost`, `additionalWants: Optional<TradeCost>`,
`gives: ItemStackTemplate`, `merchantPredicate: Optional<Holder<LootItemCondition>>`,
`givenItemModifier: Optional<Holder<LootItemFunction>>`, `maxUses` and `xp`
(`Holder<ContextIntProvider>`), `reputationDiscount` (`Holder<ContextFloatProvider>`),
`doubleTradePriceEnchantments: Optional<HolderSet<Enchantment>>`.

**Codec (281–340, validated against `LootContextParamSets.VILLAGER_TRADE`, 396–404).**
- **Required:** `wants`, `gives`.
- **Optional:** `additional_wants`, `merchant_predicate`, `given_item_modifier` (a list in the data
  is the sequence form of `LootItemFunctions.CODEC`), `double_trade_price_enchantments` (a holder set:
  the 4 files use `#minecraft:double_trade_price`).
- **Optional with defaults:** `max_uses` (`ContextIntProviders.CODEC`, default `exactly(4)`), `xp`
  (default `exactly(1)`), `reputation_discount` (default `exactly(0.0)`).
- So 5.2's "Inferred default 4 / default 1" is **CONFIRMED**. 17 files omit `max_uses` and 125
  omit `xp`.

**`getOffer(ctx)` (95–239), exactly, in order:**

1. If `merchant_predicate` is present and `!predicate.test(ctx)`, return null.
2. `stack = gives.create()`, `extra = 0`.
3. If `given_item_modifier` is present, `stack = modifier.apply(stack, ctx)` (all its draws). If the
   stack is empty, return null (the `filtered`/`discard` case).
4. `Integer c = stack.remove(ADDITIONAL_TRADE_COST)`. If not null, `extra += c`. The component never
   reaches the offer.
5. If `double_trade_price_enchantments` is present and `stack.get(STORED_ENCHANTMENTS)` is not null,
   and any key of those stored enchantments is in the set, then `extra *= 2`. Only stored
   enchantments count; an enchanted tool's `enchantments` do not.
6. `costA = wants.toItemCost(ctx, extra)`. If `costA.count() < 1`, return null.
7. `costB = additionalWants.map(w -> w.toItemCost(ctx, 0))`. If present and `count < 1`, return null.
8. Return `new MerchantOffer(costA, costB, stack, max(1, maxUses.getInt(ctx)), max(0,
   xp.getInt(ctx)), max(0f, reputationDiscount.getFloat(ctx)))`, with uses 0, special 0 and demand 0.

`ContextIntProvider.getInt` returns 0 when the provider throws `ArithmeticException` (n10
ContextIntProvider 5–17), so a failing `max_uses` gives 1. The codec accepts an inline int as
`ConstantValue`, else a typed provider, else a registry reference (n10 ContextIntProviders 550–580).

**5.2 "the hidden `additional_trade_cost` … added to the wanted count, doubled for
`#double_trade_price`, clamped; … The order of the wanted count's draw against the modifiers needs
the class"**: **CORRECTED** as listed above.
- **What is doubled.** Only the addition is doubled, not the base count.
- **Order.** The wanted count is built after all modifiers.
- **STILL UNKNOWN:** how `TradeCost.toItemCost(ctx, extra)` combines its own count with `extra`, and
  whether it clamps (to 64 or the max stack). The 4 book trades say `wants: {id: emerald, count:
  0}`, so the count is probably `count + extra`. A dump of `net.minecraft.world.item.trading.
  TradeCost` settles it.
- **Draws.** In the vanilla data every `count`, `max_uses`, `xp` and `reputation_discount` is a
  plain constant (all 391 files). The 27 `merchant_predicate`s are `entity_properties` on the
  villager's variant and draw nothing. So only the modifiers draw.
- **The modifiers in the data:**
  - `enchant_with_levels` + `filtered`: 17
  - `exploration_map` + `filtered`: 10
  - `set_random_dyes` + `filtered`: 7
  - `enchant_randomly` + `filtered`: 4
  - `set_potion`: 1
  - `set_random_potion`: 1
  - `set_stew_effect`: 1
- **`include_additional_cost_component`.** All 21 enchant modifiers set it to true.

**`LootContextParamSets.VILLAGER_TRADE`**: `ORIGIN`, `THIS_ENTITY` and
`ADDITIONAL_COST_COMPONENT_ALLOWED` are all **required** (n10 LootContextParamSets 416–427, the
`register("villager_trade", …)` at 585–592). `-c` does not print which lambda the registration's
bootstrap #11 is. `lambda$static$9` is the tenth registration's lambda in source order, and the only
lambda other than `generic`'s with `ADDITIONAL_COST_COMPONENT_ALLOWED`. It also matches what
`addOffersFromTradeSet` supplies (n7 AbstractVillager: position, the villager, `Unit`).

**Finding 20 (world-wide sequences)**: **CONFIRMED** (n10 RandomSequences, RandomSequence; n9
MinecraftServer 3781–3792; n7 AbstractVillager `addOffersFromTradeSet`). Every one of the 68 trade
sets names `random_sequence: "minecraft:trade_set/<profession>/level_N"` (or the trader's). The
loot context's random is that sequence. A set draws `nextInt(size)` on its candidate list, and each
`getOffer` draws its modifiers' numbers from the **same** stream.

How a sequence is made (exact):
- **Lookup.** `MinecraftServer.getRandomSequence(id)` is `randomSequences.get(id, worldOptions.
  seed())`.
- **Creation.** `get` is `computeIfAbsent(id, createSequence(id, worldSeed))`, and each call wraps
  the source in a `DirtyMarkingRandomSource` (n10 RandomSequences 62–80).
- **`createSequence` (95–117).** `seed = (includeWorldSeed ? worldSeed : 0) ^ (long) salt`, then
  `new RandomSequence(seed, includeSequenceId ? Optional.of(id) : Optional.empty())`. The defaults
  are salt 0, `includeWorldSeed` true and `includeSequenceId` true (15–31).
- **`RandomSequence.createSequence` (n10 RandomSequence 35–56).** `s =
  RandomSupport.upgradeSeedTo128bitUnmixed(seed)`. If there is an id, `s = s.xor(RandomSupport.
  seedFromHashOf(id.toString()))`. The result is `new XoroshiroRandomSource(s.mixed())`.
- **Saved data `random_sequences` (206–262).** `{salt: int, include_world_seed: bool (default true),
  include_sequence_id: bool (default true), sequences: {<id>: {source: <Xoroshiro state>}}}`.

So the initial state of each set's stream is a pure function of the world seed and the set's id.
What a cluster cannot share is only its advancing.

### 6. A trade: menu, container, result slot (finding 24, 5.3, 5.6)

**`Merchant.openTradingScreen(player, name, level)` (n10 Merchant 28–60)**: **CONFIRMED**.
`player.openMenu(new SimpleMenuProvider((id, inv, p) -> new MerchantMenu(id, inv, this), name))`.
If that gave an id **and `getOffers()` is not empty**: `player.sendMerchantOffers(id, offers, level,
getVillagerXp(), showProgressBar(), canRestock())`. The default `canRestock()` is false (23–26).

**`MerchantMenu` (n10 MerchantMenu)**: **CONFIRMED** slots, with corrections to `tryMoveItems`.
- **Slots and type (48–104).** The type is `MenuType.MERCHANT`, protocol id 19 (`reports/
  registries.json`). Slot 0 is a payment at (136, 37), slot 1 a payment at (162, 37), slot 2 the
  `MerchantResultSlot` at (220, 37). Then `addStandardInventorySlots(inv, 108, 84)`: 3–29 the
  inventory, 30–38 the hotbar (constants 3–15).
- **`slotsChanged`** calls `updateSellItem` then the super's. `stillValid(p)` is
  `trader.stillValid(p)` (138–144). `canTakeItemForPickAll` is false (194–197).
- **`setSelectionHint(i)`** sets `tradeContainer.selectionHint = i`, then `updateSellItem()`
  (130–136 → MerchantContainer 275–282).
- **`quickMoveStack(p, i)` (199–304).**
  - **Slot 2:** `moveItemStackTo(3, 39, reverse = true)`; on failure EMPTY. Then
    `slot.onQuickCraft(stack, copy)`, then `playTradeSound()` (306–332: `playLocalSound` when the
    trader is not client side, a no-op on a server level, **Inferred**).
  - **Slots 0–1:** `moveItemStackTo(3, 39, false)`.
  - **Slots 3–29:** to 30–39.
  - **Slots 30–38:** to 3–30.
  - **Afterwards:** an empty source gets `setByPlayer(EMPTY)`, else `setChanged()`. An unchanged
    count returns EMPTY. Otherwise `slot.onTake(p, stack)`, and the copy is returned.
- **`removed(p)` (334–408).**
  1. The super's.
  2. `trader.setTradingPlayer(null)`.
  3. On the server: if the player is dead, or is a `ServerPlayer` that has disconnected, slot 0 then
     slot 1 are dropped (`p.drop(stack, false, SERVER_ONLY)`). Otherwise, for a `ServerPlayer`, both
     go through `inventory.placeItemBackInInventory`.
- **`tryMoveItems(i)` (410–490)**, **CORRECTED** (5.6 said "puts both payments back, then fills"):
  1. Return if `i < 0` or `i ≥ offers.size()`.
  2. If slot 0 is not empty: `moveItemStackTo(slot0, 3, 39, reverse = true)`. If that fails (not all
     moved), **return**. Otherwise `setItem(0, remainder)`.
  3. The same for slot 1.
  4. Only if both payment slots are now empty: `moveFromInventoryToPaymentSlot(0, offer.costA)`,
     then, if cost B is present, `(1, costB)`.
- **`moveFromInventoryToPaymentSlot(slot, cost)` (492–559).** For `j` in 3..38 in order, a non-empty
  stack with `cost.test(stack)` is taken. It must be `isSameItemSameComponents` with the payment
  slot's current stack, unless that slot is empty.
  - It moves `n = min(maxStack − payment.count, stack.count)`: the payment becomes
    `stack.copyWithCount(payment.count + n)`, and the stack shrinks by n.
  - It stops once the payment count reaches max stack. So it fills a whole stack, not the price.

**`MerchantContainer` (n10 MerchantContainer)**.
- **Basics.** Three stacks, plus `activeOffer`, `selectionHint` and `futureXp`.
- **`removeItem(slot, n)` (65–102).** On slot 2 it removes the whole stack (`count`). Otherwise
  `ContainerHelper.removeItem`, and if something was removed from a payment slot, `updateSellItem`.
- **`setItem` (124–143).** It sets the stack, then `limitSize(maxStack)`, then `updateSellItem` if
  the slot is a payment slot.
- **`setChanged`** is `updateSellItem`. `stillValid(p)` is `merchant.getTradingPlayer() == p`.

**`updateSellItem` (163–267)**, **CORRECTED** (5.6 had only the two lookups):
1. `activeOffer = null`.
2. If slot 0 is empty, `a = slot1` and `b = EMPTY`; else `a = slot0` and `b = slot1`.
3. If `a` is empty: `setItem(2, EMPTY)`, `futureXp = 0`, **return** (no `notifyTradeUpdated`).
4. If offers are not empty:
   - `r = offers.getRecipeFor(a, b, hint)`.
   - If `r == null || r.isOutOfStock()`, then `activeOffer = r` (possibly the out-of-stock offer) and
     `r = getRecipeFor(b, a, hint)`.
   - If `r != null && !r.isOutOfStock()`, then `activeOffer = r`, `setItem(2, r.assemble())` and
     `futureXp = r.xp`. Otherwise `setItem(2, EMPTY)` and `futureXp = 0`.
5. `merchant.notifyTradeUpdated(getItem(2))`. This also runs when offers are empty.

**`MerchantResultSlot` (n10 MerchantResultSlot)**.
- **What it allows.** `mayPlace` is false. `mayPickup` (81–118) needs `activeOffer != null`,
  `satisfiedBy(a, b) || satisfiedBy(b, a)`, and the super's.
- **Counting what was taken.** `remove(n)` adds `min(n, count)` to `removeCount`. `onQuickCraft`
  adds its count and calls `checkTakeAchievements`, which is `stack.onCraftedBy(player,
  removeCount)` with `removeCount = 0` afterwards.

**5.3 "`MerchantResultSlot.onTake` (Inferred): … `take(a, b)` or `take(b, a)` → `notifyTrade` …, the
statistic, the payment slots set (each `updateSellItem`)"**: **CONFIRMED**, in this exact order
(120–177):
1. `checkTakeAchievements(stack)`.
2. `offer = activeOffer`. If null, stop.
3. `offer.take(slot0, slot1) || offer.take(slot1, slot0)`. If either succeeds:
   1. `merchant.notifyTrade(offer)`: uses + 1, `ambientSoundTime = −interval`, `rewardTradeXp`, the
      `TRADE` criterion (n7 AbstractVillager).
   2. `player.awardStat(TRADED_WITH_VILLAGER)`.
   3. `setItem(0, a)`, then `setItem(1, b)`. Each runs `updateSellItem`, whose
      `notifyTradeUpdated` stays silent because the ambient timer was just reset.
4. Whether or not the take succeeded: `merchant.overrideXp(getVillagerXp() + offer.xp)`. That is a
   no-op on a server merchant (n7 AbstractVillager `overrideXp` is an empty `return`); it serves the
   client's merchant.

The `TRADE` reputation event follows later. `customServerAiStep` raises it after the brain tick
(Villager-v 2363–2410). It fires in the villager's next AI step, which is in the same server tick
when the click was handled before the level's entities ticked (**Inferred** from the tick order, not
dumped here). It adds trading +2, may call `updateSpecialPrices` (which sends the offers again,
Villager-v 3170–3275), and then sends entity event 14.

### 7. The packets (5.6, section 10)

**`merchant_offers` (clientbound 53)**: **CORRECTED** in the integer types (n10
ClientboundMerchantOffersPacket 42–106; MerchantOffer 437–549; n7k MerchantOffers 111–115). On the
wire:

| # | field | type |
|---|---|---|
| 1 | container id | `writeContainerId` |
| 2 | offers count | var-int (`ByteBufCodecs.collection`) |
| 2a | each: cost A | `ItemCost.STREAM_CODEC`: item (registry var-int), count (var-int), components (var-int n, then n × (type var-int, value)) |
| 2b | result | `ItemStack.STREAM_CODEC` (non-optional) |
| 2c | cost B | `ItemCost.OPTIONAL_STREAM_CODEC` (boolean, then an ItemCost) |
| 2d | out of stock | boolean (`uses ≥ maxUses`) |
| 2e | uses | **int, 4 bytes** |
| 2f | max uses | **int, 4 bytes** |
| 2g | xp | **int, 4 bytes** |
| 2h | special price | **int, 4 bytes** |
| 2i | price multiplier | float, 4 bytes |
| 2j | demand | **int, 4 bytes** |
| 3 | villager level | var-int |
| 4 | villager xp | var-int |
| 5 | show progress | boolean |
| 6 | can restock | boolean |

- **Decoding the out-of-stock flag.** It is not kept as a field: decoding sets `uses = maxUses` when
  it is true, so the client sees `uses` as written only for offers in stock.
- **The offers are copied.** The packet's constructor takes `offers.copy()`.

**`select_trade` (serverbound 52)**: **CONFIRMED**, one var-int `item` (n10
ServerboundSelectTradePacket 16–33).

**`open_screen`** is packet id 60 with menu type 19 (`reports/packets.json`, `registries.json`).

### 8. Components trades need (5.7, finding 25)

**`DyedItemColor` (n10 DyedItemColor 1–294)**, the rule `set_random_dyes` will call (5.7 listed it
as not dumped):
- **The component.** It is `{rgb: int}`. Codec `ExtraCodecs.RGB_COLOR_CODEC`, stream
  `ByteBufCodecs.INT` (4 bytes). `LEATHER_COLOR = −6265536`.
- **`getOrDefault(stack, d)`** returns `ARGB.opaque(rgb)` if the component is present, else `d`.
- **`applyDyes(stack, dyes)` (36–58)** puts the new colour on `stack.copyWithCount(1)`.
- **`applyDyes(old, dyes)` (59–210), in ints except where marked:**
  - `r`, `g`, `b`, `sumMax` and `n` start at 0.
  - If `old` is not null, it counts as one dye: add its red, green and blue, add `max(r, g, b)` of
    it to `sumMax`, and `n++`.
  - For each dye: `c = dye.getTextureDiffuseColor()`; add its red, green and blue, add their max to
    `sumMax`, and `n++`.
  - `ar = r / n`, `ag = g / n`, `ab = b / n` (int division).
  - `avgMax = (float) sumMax / n` (f32), and `m = (float) max(ar, ag, ab)`.
  - The new components are `(int)((float) ar * avgMax / m)`, and the same for green and blue (f32,
    truncated).
  - The result is `rgb = ARGB.color(0, r', g', b')`, with alpha 0.

`stored_enchantments` is what `getOffer` tests for doubling (section 5). `additional_trade_cost` is
a component the stack carries only between the modifier and `getOffer`'s removal.

**"for the cartographer `exploration_map` … (12 of its 31 trades)"**: **CORRECTED** to **10 of 31**.
`villager_trade/cartographer/2/` has 7 (`village_{plains,desert,savanna,snowy,taiga}`,
`explorer_{jungle,swamp}`), level 3 has 2 (`ocean_monument`, `buried_trial_chambers`) and level 5
has 1 (`woodland_mansion`). That makes 7 + 2 + 1 = 10, and no other profession has one.

**`WorldBorder`**: no class in this area reads it. Neither `VillagerTrade` nor the menu classes do.

### 9. The idle and raid predicates and `validateBedPoi` (`VillagerGoalPackages-v`, for 1.4 and 4.3)

From the bootstrap arguments (n10 VillagerGoalPackages-v 2103–2150) and their call sites:

- **Core (790–850).**
  - `AcquirePoi(acquirableJobSite, job_site, potential_job_site, true, empty, #0)`. #0 is
    `lambda$getCorePackage$0`, which returns true (2088–2102).
  - `AcquirePoi(#1 = holder is PoiTypes.HOME, home, false, Optional.of(14), #2 = validateBedPoi)`.
    Babies too.
  - `AcquirePoi(#3 = holder is MEETING, meeting_point, true, Optional.of(14))`. This is the overload
    without a BiPredicate.
- **`validateBedPoi(level, pos)` (901–942):** the block at `pos` is in
  `#villagers_can_sleep_on_bed` **and** `BedBlock.OCCUPIED` is false. **CONFIRMED**.
- **The rest and meet packages' `ValidateNearbyPoi`:** #4 is `HOME` (2043–2057) and #5 is `MEETING`
  (2028–2042).
- **Idle (1416–1460).** `InteractWith.of(VILLAGER, 8, #6, #7, BREED_TARGET, speed, 2)`, weight 1.
  - #6 is `Villager::canBreed`, typed `(Villager)Z`: the self predicate.
  - #7 is `AgeableMob::canBreed`, typed `(AgeableMob)Z`: the target predicate. It is a virtual call,
    so a villager target uses Villager's override.
  - The brief's **Inferred** "canBreed, canBreed" is **CONFIRMED**.
- **Raid (1698–1740).** **CONFIRMED** the Inferred reading in 1.4.
  - **#8 is `raidExistsAndNotVictory`** (1990–2027). It returns `raid != null && raid.isVictory()`
    and gates `MoveToSkySeeingSpot(0.5)` 5 / `VillageBoundRandomStroll(0.5 × 1.1)` 2.
  - **#9 is `raidExistsAndActive`** (1946–1989). It returns `raid != null && isActive && !isVictory
    && !isLoss` and gates `LocateHidingPlace(24, 0.5 × 1.4, 1)`.

### 10. Zombie villagers (section 6)

Section 6 has no **Inferred** claim. The dumps here bear on it in two places:
- `ZOMBIE_VILLAGER_CURED` writes major_positive +20 then minor_positive +25 about the starter
  (section 1 table). With weights 5 and 1, the saturated values are 20 and 25, so the curer's
  reputation is 20 × 5 + 25 = 125. 5.4's figure is **CONFIRMED**.
- Offers copied through a conversion keep every saved field of the codec in section 4.

### 11. Open questions 5, 6, 8 (section 16): what the bytecode adds

- **q5 (trade sets' sequences).**
  - **The seeding is exact and cheap to reproduce:** world seed ^ salt, then XOR with the MD5-based
    `seedFromHashOf(id)`, then mixed into Xoroshiro.
  - **One stream carries more than the choice.** It also carries every modifier's draws (enchant
    levels, book enchantments, dye picks, potion picks). A world stream per set kept by the store
    (option b) would need each generation's whole sequence of draws applied in one place, not only
    `nextInt(size)`.
  - **This favours (a).** Vanilla's own per-set streams are shared by every villager in the world,
    so a single villager's offers are no more "reproducible" in vanilla than with an own stream.
    Judged by distribution, (a) loses nothing a player can see.
- **q6 (trading across a line).**
  - The menu's validity is entirely `Merchant.stillValid` (the villager's range + 4 rule).
    `MerchantContainer.stillValid` is identity with `getTradingPlayer()`.
  - Closing always calls `setTradingPlayer(null)` before the payments go back.
  - The result slot needs the live `activeOffer` and the live offer list, so the menu cannot work
    from a copy of the villager.
  - Nothing here argues against (a).
- **q8 (enchanting).**
  - **What trades need from it:** the `additional_trade_cost` component the enchant functions write
    when `include_additional_cost_component` is true (all 21 enchant modifiers in the data), the
    `ADDITIONAL_COST_COMPONENT_ALLOWED` parameter (always supplied for trades), and
    `stored_enchantments` with the `#double_trade_price` set for the 4 book trades.
  - **What the price of a book needs:** the function's cost value, doubled when an enchantment is in
    the set, and then `TradeCost.toItemCost` (still undumped).
  - **Without enchanting (option b):** the 4 book trades and the 17 `enchant_with_levels` trades
    would hit their `filtered`/`discard` (no `stored_enchantments` or `enchantments` component), so
    they are discarded and redrawn. That changes which other trades a librarian gets. It is a visible
    difference, not only an unenchanted item.

### Still unknown (what settles each)

- **`TradeCost`** (`net.minecraft.world.item.trading.TradeCost`): its codec (the default count when
  `count` is absent; 172 of 391 files omit it) and `toItemCost(ctx, extra)` (sum, clamp).
- **Which end action is the server's.** `BellBlockEntity`'s bootstrap #0/#1 (client particles or
  server glow) is settled only by `javap -v BellBlockEntity`; by elimination it is `makeRaidersGlow`
  on the server.
- **`ItemCost.OPTIONAL_STREAM_CODEC`'s operation** (bootstrap #6), by `javap -v ItemCost`. By
  convention it is `ByteBufCodecs::optional`.

### Settled by the lead afterwards (n10 TradeCost.txt, BellBlockEntity-v.txt, ItemCost-v.txt)

- **`TradeCost`** `{id, count (ContextIntProvider, default exactly(1)), components (default EMPTY)}`.
  `toItemCost(ctx, extra)` = `ItemCost(item, Mth.clamp(count.getInt(ctx) + extra, 0, item.getDefaultMaxStackSize()), components)`
  (TradeCost.txt 85–108, codec 123–154). So a missing count is 1, the extra cost is added to the wanted count, and
  the sum is clamped to 0..max stack (not 1).
- **The bell's two end actions** are `showBellParticles` (client) and `makeRaidersGlow` (server), confirmed by
  the bootstrap methods of BellBlockEntity-v.txt 400–412.
- **`ItemCost.OPTIONAL_STREAM_CODEC`** is `STREAM_CODEC.apply(ByteBufCodecs::optional)` (ItemCost-v.txt 281, 732, 791): a boolean, then the cost when true.

---------------------------------------------------------------------------------------

## Part C: raids, omens, the boss bar, the spawners, maps

Cited as "n10 X lines a–b" for `/home/claude/reference/n10-javap/<full name>.txt`, "n7k Raid" for
`n7k-javap/net.minecraft.world.entity.raid.Raid.txt` (likewise n7, n9). Brief = `docs/groundwork/n10-brief.md`.

### Summary of the corrections

1. **Bad omen is not removed on the tick it turns to raid omen** (finding 28, 7.1). `BadOmenMobEffect`
   adds raid omen (a new key in the player's `activeEffects` `HashMap`) and returns false; the
   `iterator.remove()` that follows in `LivingEntity.tickEffects` then throws
   `ConcurrentModificationException`, which the method catches and abandons the loop (n7 LivingEntity
   lines 1867–1997, exception table `32 125 128 ConcurrentModificationException`; JDK `HashMap`'s
   iterator checks `modCount` in `remove`). So on tick T: raid omen (600, amplifier) added,
   `raid_omen_position` = player's block position, **bad omen kept**, and every effect after bad omen
   in the map's (identity-hash) order is not ticked that tick. On tick T+1, if the player still
   qualifies: raid omen is refreshed to 600 (`update`: same amplifier, longer duration), the position
   is stored again (the T+1 position), and bad omen is removed (a put on an existing key does not
   change `modCount`). If the player no longer qualifies on T+1 (left the village, peaceful, a raid at
   omen 5 appeared), bad omen stays and keeps counting down while raid omen runs on with the T
   position. The raid therefore starts about 600 ticks after T+1 (599 or 600, depending on whether
   raid omen comes before or after bad omen in the map's order).
2. **No bonus wave unless the raid's omen level is at least 2** (finding 27, 16 q10). `hasBonusWave`
   is `raidOmenLevel > 1` (n7k Raid 952–962), and the first absorption sets it to amplifier + 1. A level-I
   bottle (amplifier 0) gives omen 1: on easy, three waves and no bonus wave.
3. **The easy bonus for vindicators and pillagers is not `+nextInt(2)`**: it is `n = nextInt(2)`, then
   `nextInt(n + 1)` only if `n > 0`. So +1 with probability 1/4, one or two draws. It is drawn **for
   every wave, also where the default is 0**: wave 1 and wave 3 on easy can bring a vindicator.
   On normal it is one `nextInt(2)`, on hard one `nextInt(3)` (n7k Raid 1968–2045).
4. **The bonus wave uses the final wave's defaults** (index `numGroups`), not a column of its own:
   on easy the bonus wave is wave 3 again (3 pillagers, 1 ravager, the bonus draws), with wave
   number 4 passed to the bonus rule (n7k Raid 1952–1966, 1212–1330).
5. **Riders are fixed by wave number, not by difficulty**: a ravager in wave 5 (`getNumGroups(NORMAL)`)
   gets a pillager, in wave 7 or later (`>= getNumGroups(HARD)`) the first ravager of the wave gets an
   evoker and the others vindicators. On normal the bonus wave is wave 6: **no rider**. On hard wave 5
   gets a pillager rider too (n7k Raid 1212–1375, `spawnGroup` offsets 161–291).
6. **`findRandomSpawnPos` refuses a village position while `cooldown / 20 > 7`, i.e. while the
   countdown is at 160 or more** (the first 141 ticks of the 300-tick countdown), not "during the first 160
   ticks" (n7k Raid 1666–1825, offsets 201–216). It accepts a position when the ravager placement
   passes **or** (the block below is snow **and** the block is air). Both `nextInt(3)` draws are made
   every attempt before any refusal.
7. **The raid's centre averages the occupied `#village` points within 64 of the stored
   `raid_omen_position`**, not of the player's position at the time (n7k Raids 172–270; the `pos`
   argument comes from `RaidOmenMobEffect`).
8. **Explorer-map trades are 10 of the cartographer's 31, not 12** (finding 25, q9): 7 at level 2, 2 at 3, 1
   at 5; all with `search_radius` 100 (not the function's default 50) and a `filtered` modifier on
   `map_id` with `on_fail: discard`, so a failed search empties the stack and `VillagerTrade.getOffer`
   returns null (n10 VillagerTrade lines 95–140; datagen `villager_trade/cartographer/`).
9. **`InsideBrownianWalk`'s third filter is `level.noCollision(body)`**, the villager's own current box,
   not the candidate's: it is the same for all 27 candidates (n10 InsideBrownianWalk-v lines 442–458).
   The behaviour also does nothing (and draws nothing) when the villager's own block sees the sky, and
   returns true whether or not a target was found.

---------------------------------------------------------------------------------------

### 1. Raid composition (`Raid$RaiderType`, n10 Raid$RaiderType lines 1–288; n7k Raid 1212–1330, 1952–2079)

#### 1.1 The table, exactly

`spawnsPerWaveBeforeBonus`, indexed by wave number (index 0 never used). Enum order (= ordinals, =
the order `spawnGroup` walks `VALUES`): VINDICATOR 0, EVOKER 1, PILLAGER 2, WITCH 3, RAVAGER 4.

| index | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
|---|---|---|---|---|---|---|---|---|
| vindicator | 0 | 0 | 2 | 0 | 1 | 4 | 2 | 5 |
| evoker | 0 | 0 | 0 | 0 | 0 | 1 | 1 | 2 |
| pillager | 0 | 4 | 3 | 3 | 4 | 4 | 4 | 2 |
| witch | 0 | 0 | 0 | 0 | 3 | 0 | 0 | 1 |
| ravager | 0 | 0 | 0 | 1 | 0 | 1 | 0 | 2 |

`numGroups` = `getNumGroups(difficulty)` at creation, from the level difficulty (`Raids.getOrCreateRaid`
→ `new Raid(pos, level.getDifficulty())`): peaceful 0, easy 3, normal 5, hard 7 (n7k Raid 2053–2079 with
n10 Raid$1: PEACEFUL→1, EASY→2, NORMAL→3, HARD→4 in the switch map).

#### 1.2 The rule of one `spawnGroup(level, pos)`

- `wave = groupsSpawned + 1`; `totalHealth = 0`; `difficulty = level.getCurrentDifficultyAt(pos)
  .getDifficulty()` (the level difficulty **now**, not the raid's); `bonus = shouldSpawnBonusGroup()`
  (`groupsSpawned == numGroups && aliveCount == 0 && raidOmenLevel > 1`).
- For each type in enum order: `count = default + extra`, where
  `default = table[bonus ? numGroups : wave]` and `extra` is drawn from the **raid's random** now:
  - vindicator, pillager: easy `n = nextInt(2)`; normal `n = 1`; hard `n = 2`; then
    `extra = n > 0 ? nextInt(n + 1) : 0`.
  - evoker: 0, no draw.
  - witch: `n = 1` if not easy and `wave > 2` and `wave != 4`, else return 0 with no draw;
    then `nextInt(2)`.
  - ravager: `n = 1` if not easy and `bonus`, else 0; then `nextInt(2)` only if `n = 1`.
  So per group the raid's draws are, in order: vindicator's, pillager's, witch's (if any), ravager's
  (if any). Each type's mobs are created and joined **before** the next type's draws (only entity
  streams are used by the creation).
- Each raider: `EntityType.create(level, EVENT)` (null skips the rest of that type); the first that
  `canBeLeader()` in the group becomes patrol leader and the wave's leader; `joinRaid(level, wave,
  raider, pos, false)`.
- A ravager then gets a rider: `wave == 5` → a pillager; `wave >= 7` → the group's first ravager an
  evoker, later ones a vindicator (a counter incremented for every ravager); otherwise none. The rider
  is joined (`joinRaid`, so it is counted and finalised), `snapTo(pos, 0, 0)`, `startRiding(ravager,
  false, false)`.
- After all types: `waveSpawnPos = empty`, `groupsSpawned++`, `updateBossbar()`, dirty.

#### 1.3 Waves per difficulty

`b2e` = easy bonus (0 or 1, P(1) = 1/4, one or two `nextInt(2)`); `b2` = `nextInt(2)`; `b3` = `nextInt(3)`.

**Easy** (3 waves; bonus wave only at omen ≥ 2):

| wave | vindicators | pillagers | others |
|---|---|---|---|
| 1 | 0 + b2e | 4 + b2e | |
| 2 | 2 + b2e | 3 + b2e | |
| 3 | 0 + b2e | 3 + b2e | 1 ravager |
| bonus (4) | 0 + b2e | 3 + b2e | 1 ravager |

No witch or evoker ever; no rider; the ravager draws nothing.

**Normal** (5 waves):

| wave | vindicators | evokers | pillagers | witches | ravagers |
|---|---|---|---|---|---|
| 1 | 0 + b2 | 0 | 4 + b2 | 0 | 0 |
| 2 | 2 + b2 | 0 | 3 + b2 | 0 | 0 |
| 3 | 0 + b2 | 0 | 3 + b2 | 0 + b2 | 1 |
| 4 | 1 + b2 | 0 | 4 + b2 | 3 | 0 |
| 5 | 4 + b2 | 1 | 4 + b2 | 0 + b2 | 1 (rider: pillager) |
| bonus (6) | 4 + b2 | 1 | 4 + b2 | 0 + b2 | 1 + b2 (no rider) |

**Hard** (7 waves):

| wave | vindicators | evokers | pillagers | witches | ravagers |
|---|---|---|---|---|---|
| 1 | 0 + b3 | 0 | 4 + b3 | 0 | 0 |
| 2 | 2 + b3 | 0 | 3 + b3 | 0 | 0 |
| 3 | 0 + b3 | 0 | 3 + b3 | 0 + b2 | 1 |
| 4 | 1 + b3 | 0 | 4 + b3 | 3 | 0 |
| 5 | 4 + b3 | 1 | 4 + b3 | 0 + b2 | 1 (rider: pillager) |
| 6 | 2 + b3 | 1 | 4 + b3 | 0 + b2 | 0 |
| 7 | 5 + b3 | 2 | 2 + b3 | 1 + b2 | 2 (riders: evoker, vindicator) |
| bonus (8) | 5 + b3 | 2 | 2 + b3 | 1 + b2 | 2 + b2 (riders: evoker, then vindicators) |

#### 1.4 The bonus-wave rule, step by step (n7k Raid 920–989, 1493–1512, tick 151–338)

- `hasBonusWave = raidOmenLevel > 1`; `isFinalWave = groupsSpawned == numGroups`;
  `hasSpawnedBonusWave = groupsSpawned > numGroups`.
- `hasMoreWaves = hasBonusWave ? !hasSpawnedBonusWave : !isFinalWave`.
- `shouldSpawnGroup = cooldown == 0 && (groupsSpawned < numGroups || shouldSpawnBonusGroup) &&
  alive == 0`.
- Between waves (alive 0, more waves): countdown of 300 as the brief has it; when the cooldown is
  already 0 and a wave has spawned, the cooldown is set to 300, the name reset, and the tick
  **returns** (nothing else that tick).
- The omen level can rise while the raid runs (`createOrExtendRaid` absorbs whenever the raid is
  unstarted **or** its omen is below 5), so a bonus wave can appear for a raid begun at omen 1.
- Victory: started, no more waves, alive 0: `postRaidTicks` counts to 40, then VICTORY and Hero of the
  Village `(48000, omen − 1, ambient false, visible false, showIcon true)` to each hero UUID found by
  `level.getEntity` that is a living non-spectator (n7k Raid tick 671–843). At omen 1 the amplifier is 0.

**Verdict on finding 27 (n7k Raid 1952–2079)**: CORRECTED (corrections 2–5 above). "includes a ravager"
CONFIRMED (wave 3 and the bonus wave); "witches and evokers never come on easy" CONFIRMED.

---------------------------------------------------------------------------------------

### 2. The omens, step by step

#### 2.1 Bad omen (n10 BadOmenMobEffect lines 1–65)

`shouldApplyEffectTickThisTick` is always true. `applyEffectTick(level, entity, amplifier)`:
1. not a `ServerPlayer`, or a spectator → return true (keep);
2. `level.getDifficulty() == PEACEFUL` → true;
3. `!level.isVillage(player.blockPosition())` → true;
4. `raid = level.getRaidAt(player.blockPosition())` (nearest active within 96: n7 ServerLevel
   3435–3442, `getNearbyRaid(pos, 9216)`); if `raid != null && raid.getRaidOmenLevel() >=
   raid.getMaxRaidOmenLevel()` (5) → true;
5. else `player.addEffect(new MobEffectInstance(RAID_OMEN, 600, amplifier))`,
   `player.setRaidOmenPosition(player.blockPosition())`, return **false**.

`MobEffectInstance.tickServer` (n9 MobEffectInstance 456–502) returns false at once when
`applyEffectTick` is false (no count-down); `LivingEntity.tickEffects` then removes it, except for the
`ConcurrentModificationException` described in correction 1.

#### 2.2 Raid omen (n10 RaidOmenMobEffect lines 1–50)

`shouldApplyEffectTickThisTick(duration, amplifier)` is `duration == 1` (the remaining duration
before the decrement; `tickCount` for an infinite one). `applyEffectTick`:
1. a non-spectator `ServerPlayer` with a non-null `getRaidOmenPosition()` →
   `level.getRaids().createOrExtendRaid(player, pos)` (result ignored), `clearRaidOmenPosition()`,
   return false (removed at once);
2. otherwise true, and the effect ends by its duration that same tick (decrement to 0).

#### 2.3 `createOrExtendRaid(player, pos)` (n7k Raids 172–270) — CONFIRMED as the brief has it, with
the centre corrected (correction 7): spectator → null; rule `raids` false → null; attribute
`can_start_raid` at `pos` false → null; centre = `BlockPos.containing(sum / count)` of the occupied
`#village` points (`PoiTypeTags.VILLAGE`, n7k Raids 391–396) within 64 of `pos`, or `pos` if none
(sum as a `Vec3` of `double`s, scaled by `1.0 / count`); `getOrCreateRaid` (a new raid draws its
UUID: two `nextLong`); stored under `++nextId` if unstarted and not stored; then `absorbRaidOmen`
when unstarted or below 5: `omen = clamp(omen + amp + 1, 0, 5)` with amp the player's **current
raid omen** amplifier (none → nothing absorbed, returns false); the stat and trigger only before the
first wave.

**Verdicts**: finding 28 CORRECTED (correction 1; the rest CONFIRMED, including 600 ticks and the
position on the player, n9 ServerPlayer 117, 399–408, 5407–5424). Section 7.1's Inferred omen rules:
CORRECTED as above (correction 1).

---------------------------------------------------------------------------------------

### 3. The boss bar

#### 3.1 Enums (n10 BossEvent$BossBarColor lines 1–170, BossEvent$BossBarOverlay lines 1–126)

| colour | ordinal | name | formatting |
|---|---|---|---|
| PINK | 0 | pink | RED |
| BLUE | 1 | blue | BLUE |
| RED | 2 | red | DARK_RED |
| GREEN | 3 | green | GREEN |
| YELLOW | 4 | yellow | YELLOW |
| PURPLE | 5 | purple | DARK_BLUE |
| WHITE | 6 | white | WHITE |

| overlay | ordinal | name |
|---|---|---|
| PROGRESS | 0 | progress |
| NOTCHED_6 | 1 | notched_6 |
| NOTCHED_10 | 2 | notched_10 |
| NOTCHED_12 | 3 | notched_12 |
| NOTCHED_20 | 4 | notched_20 |

The raid's bar: RED (2), NOTCHED_10 (2), name `translatable("event.minecraft.raid")`, flags all false.

#### 3.2 `BossEvent` and `ServerBossEvent` (n10 BossEvent lines 1–139, ServerBossEvent lines 1–288)

- `BossEvent(id, name, color, overlay)` sets `progress = 1.0f`; `darkenScreen`, `playBossMusic`,
  `createWorldFog` false.
- `ServerBossEvent`: `players` a `HashSet<ServerPlayer>`, `visible = true` at construction. The raid's
  constructor then calls `setProgress(0.0f)` (no players yet: no packet). A raid **loaded** from disk
  builds a fresh bar with a new UUID from a new unseeded random, and its progress stays 1.0 until
  the next update (n7k Raid 140–209).
- Each setter sends only on a change: progress by `fcmpl` (exact float), colour/overlay by identity,
  the three flags by value, the name by `Objects.equal` (component equality). It sends to every
  player in `players` **only while visible**, through a factory chosen by a lambda (`-c` does not show
  which factory each `invokedynamic` #0–#3 names; by the shared call sites: #0 progress → update
  progress, #1 colour and overlay → update style, #2 the three flags → update properties, #3 name →
  update name; a `-v` dump of `ServerBossEvent` would show it).
- `addPlayer`: added to the set and, if visible, an **add** packet. `removePlayer`: removed and, if
  visible, a **remove** packet. `removeAllPlayers`: `removePlayer` for a copy of the set.
  `setVisible(v)` on a change: to every player an add (v true) or a remove (v false).
- The raid: `updatePlayers` (n7k Raid 330–381) adds every player of the level for which
  `validPlayer` holds (alive, and `level.getRaidAt(blockPosition)` is this raid; n7k Raid 2140–2157)
  and not yet in the set, then removes those no longer in that list. `stop()` removes all.

#### 3.3 The packet (`minecraft:boss_event`, play id 9 in Clustine's generated ids; n10
ClientboundBossEventPacket lines 1–220)

Wire: `uuid` (two longs), `operation type` (`writeEnum`: VarInt of the ordinal of
`ClientboundBossEventPacket$OperationType`), then the operation's own `write`. Flags byte from
`encodeProperties(darken, music, fog)` = `darken ? 1 : 0 | music ? 2 : 0 | fog ? 4 : 0` (CONFIRMED,
lines 3–7 constants and `encodeProperties`).

The operation classes are **not dumped**, so the type ordinals and field orders below are
**Unverified (Pumpkin)** (`Pumpkin/crates/pumpkin-protocol/src/java/client/play/boss_event.rs`);
the factories in the dumped class confirm which values each operation carries:

| op | ordinal (Pumpkin) | fields in order | dumped factory |
|---|---|---|---|
| add | 0 | name (component), progress f32, colour VarInt, overlay VarInt, flags u8 | `createAddPacket(event)` (`AddOperation(event)`) |
| remove | 1 | none | `createRemovePacket(uuid)` (a shared `REMOVE_OPERATION`, anonymous `$1`) |
| update progress | 2 | progress f32 | `createUpdateProgressPacket` (`getProgress()`) |
| update name | 3 | name (component) | `createUpdateNamePacket` (`getName()`) |
| update style | 4 | colour VarInt, overlay VarInt | `createUpdateStylePacket` (`getColor()`, `getOverlay()`) |
| update properties | 5 | flags u8 | `createUpdatePropertiesPacket(darken, music, fog)` |

A raid sends only add, remove, update progress and update name.

#### 3.4 What the raid sets, and when (n7k Raid 452–945, 1442–1456)

- Progress: during a countdown `clamp((300 − cooldown) / 300.0f, 0, 1)` after the decrement (int
  subtraction, `i2f`, `/ 300.0f`); after a group and on every raider hurt or removal
  `clamp(healthOfLivingRaiders / totalHealth, 0, 1)` (`f32` sum over the groups' sets).
- Name: every 20 active ticks (`ticksActive % 20 == 0`, after `updatePlayers` and `updateRaiders`):
  alive 1 or 2 → `event.minecraft.raid` copy + literal `" - "` + `translatable(
  "event.minecraft.raid.raiders_remaining", alive)`; otherwise the plain name. Reset to the plain name
  when a countdown restarts.
- Celebration (won or lost): every 20 celebration ticks `updatePlayers`, `setVisible(true)`, then
  victory: progress 0 and `translatable("event.minecraft.raid.victory.full")`; loss:
  `translatable("event.minecraft.raid.defeat.full")`. At 600 celebration ticks `stop()`.
- Centre chunk unloaded: `active` false → `setVisible(false)` (removes to all), the tick returns.

**Verdicts**: finding 29 CONFIRMED (the operation classes still missing, section 7). Section 7.2's
Inferred "add when visible, remove, progress and name on a change" CONFIRMED. Section 10's raid row
CONFIRMED.

#### 3.5 The horn (n7k Raid 1104–1210) — CONFIRMED, exactly: after the first successful `spawnGroup` of
a tick's spawn loop; one `nextLong` from the raid's random as the sound seed; for each player of the
level (`level.players()` order), `d = sqrt((cx − px)² + (cz − pz)²)` with `c = Vec3.atCenterOf(spawnPos)`;
sent if `d <= 64.0` **or** the player is in the bar's set: `ClientboundSoundPacket(RAID_HORN, NEUTRAL,
px + 13.0 / d · (cx − px), py, pz + 13.0 / d · (cz − pz), volume 64.0f, pitch 1.0f, seed)` (all `f64`).

---------------------------------------------------------------------------------------

### 4. Other raid claims re-read

- **Finding 26** (n7k Raid 87–166, Raids 105–465): own unseeded random CONFIRMED
  (`RandomSource.create()`); centre CORRECTED (correction 7); `getRaidAt` 96 CONFIRMED; "wave
  positions reach about 104 blocks" CONFIRMED (the search at cooldown 300 has `f = 0.22·15 − 0.24 =
  3.06f`, radius `floor(cos·32·f)` up to 97, plus `nextInt(3)·floor(f)` up to 6); members at
  `distSqr >= 12544` (112²) dropped CONFIRMED (n7k Raid 991–1102); horn 64 CONFIRMED (horizontal, or in
  the bar).
- **`findRandomSpawnPos(attempts)`** (n7k Raid 1666–1825): `c = cooldown / 20` (int);
  `f = 0.22f · c − 0.24f` (`f32`); one `nextFloat() · 6.2831855f`; per attempt `k`:
  `a = angle + 3.1415927f · k / 8.0f`; `x = cx + floor_f32(Mth.cos(a) · 32.0f · f) + nextInt(3) ·
  floor_f32(f)`; `z = cz + floor_f32(Mth.sin(a) · 32.0f · f) + nextInt(3) · floor_f32(f)` (x's draw,
  then z's); `y = WORLD_SURFACE` height at (x, z); refused if `|y − cy| > 96`, if `isVillage(pos) &&
  c > 7`, if not `hasChunksAt(x−10, z−10, x+10, z+10)`, if not `isPositionEntityTicking`; accepted if
  the ravager's placement is ok, or (block below is SNOW and block is air). In the spawn loop
  (cooldown 0) `f = −0.24f`, so `floor(f) = −1`: the position is within about 8 blocks of the centre,
  each `nextInt(3)` subtracting 0 to 2.
- **Spawn loop** (n7k Raid tick 585–670): while `shouldSpawnGroup`: position = `waveSpawnPos` or
  `findRandomSpawnPos(level, 20)`; found → `started = true`, `spawnGroup`, horn once; not found →
  failures + 1; failures > 5 → `stop()`. Raid draws per failed call: 1 + 2·20.
- **Draws** (brief section 9, the raid row): CORRECTED in detail — UUID two `nextLong` at creation (and
  again at every load); per search one `nextFloat` then two `nextInt(3)` per attempt (8 attempts during
  a countdown, 20 in the spawn loop); per group the type draws of section 1.2; the horn one `nextLong`.
- **`joinRaid`** (n7k Raid 1377–1440): `setCurrentRaid`, `setWave`, `setCanJoinRaid(true)`,
  `setTicksOutsideRaid(0)`; for a fresh (not recruited) raider: `setPos(x + 0.5, y + 1.0, z + 0.5)`,
  `finalizeSpawn(level, difficultyAt(pos), EVENT, null)`, `applyRaidBuffs(level, wave, false)`,
  `setOnGround(true)`, `addFreshEntityWithPassengers`. CONFIRMED.
- **`Raid$RaidStatus`** (n10 lines 1–107): ONGOING 0 `ongoing`, VICTORY 1 `victory`, LOSS 2 `loss`,
  STOPPED 3 `stopped`; codec by the lower-case names.
- **`Raids$RaidWithId`** (n10 lines 1–87): a record `(int id, Raid raid)`; codec `id` (int) plus
  `Raid.MAP_CODEC` flattened into the same compound. `Raid.MAP_CODEC` fields (n7k Raid 2159–2227), all
  required: `started` bool, `active` bool, `ticks_active` long, `raid_omen_level` int,
  `groups_spawned` int, `cooldown_ticks` int, `post_raid_ticks` int, `total_health` float,
  `group_count` int, `status` string, `center` block pos, `heroes_of_the_village` UUID set. The
  bar's UUID, the wave spawn position and the members are not saved. `Raids`: saved data
  `minecraft:raids` per level, `nextId` starting at 1 and pre-incremented (first id 2).
- **Question 16.7**: the facts it rests on are as the brief has them; one more for the record's hand-over: the
  bar's UUID is regenerated only when the game loads a raid, so a port that carries the record across
  regions must carry the UUID with it (or a client sees a second bar); the members are a
  `HashMap<Integer, Set<Raider>>` (`HashSet`s), the leaders a `HashMap<Integer, Raider>`.
- **Question 16.10** (the ravager): CONFIRMED that easy's wave 3 and bonus wave bring one; the bonus
  wave only at omen ≥ 2.

---------------------------------------------------------------------------------------

### 5. The wandering trader's spawner (section 8.4, question 16.12)

**`WanderingTraderData`** (n10 lines 1–123): `SavedData`, type id `minecraft:wandering_trader`, data
fixer `SAVED_DATA_WANDERING_TRADER`. Fields `spawnDelay` (int), `spawnChance` (int); the no-argument
constructor gives 24000 and 25. Codec: `spawn_delay` int, optional, default 24000; `spawn_chance` int,
optional, default 25. Setters mark dirty only on a change. It lives in the **server's** storage
(`MinecraftServer.savedDataStorage`, n9 MinecraftServer 600–630), one record for the whole server.
`SavedDataStorage` keeps the payload under a `data` tag (n10 SavedDataStorage 146, 406).

**`WanderingTraderSpawner.tick`** (n7 lines 1–200) — section 8.4 CONFIRMED, exactly:
1. rule `spawn_wandering_traders` false → return (the spawner's `tickDelay` does not count down);
2. `--tickDelay > 0` → return; `tickDelay = 1200` (field, starts at 1200, not saved);
3. `delay = data.spawnDelay − 1200`; `setSpawnDelay(delay)`; `delay > 0` → return;
4. `setSpawnDelay(24000)`; `old = spawnChance`; `setSpawnChance(clamp(old + 25, 25, 75))`;
5. own `nextInt(100) > old` → return;
6. `spawn(level)` true → `setSpawnChance(25)`. `spawn` returns true when there is **no player**
   (`level.getRandomPlayer()` null) as well as on success.
In `spawn`: own `nextInt(10) != 0` → false; a `meeting` point within 48 (`PoiManager.find`, occupancy
ANY, first found) or the player's block position; `findSpawnPositionNear(level, centre, 48)`: up to 10
tries of own `x = cx + nextInt(96) − 48`, `z = cz + nextInt(96) − 48`, the trader's heightmap and
placement; `hasEnoughSpace` (every block from the spot to spot + (1, 2, 1) with an empty collision
shape); biome not `#without_wandering_trader_spawns` (else false); the trader by
`EntityType.spawn(EVENT)`; two llamas each by `findSpawnPositionNear(trader, 4)` (own `nextInt(8) − 4`
twice per try, **the trader's** placement and heightmap), leashed; despawn delay 48000, wander target the
found spot, home there within 16; true.

Question 16.12 (a): the record is exactly two ints with those defaults; the spawner's 1200-tick
cadence and random are not part of it.

StructureStart: the trader's spawner uses none. The cat spawner's hut branch uses only
`StructureManager.getStructureWithPieceAt(pos, #cats_spawn_in).isValid()` (n7 CatSpawner 108–112), and
`StructureStart.isValid()` is "pieces not empty" (n10 StructureStart 262–272).

---------------------------------------------------------------------------------------

### 6. Explorer maps (question 16.9; finding 25's maps)

What one exploration-map trade does, as far as the dumps show:

1. **Data** (datagen `villager_trade/cartographer/`): 10 trades with `exploration_map`: level 2 the
   five village maps (plains, savanna, taiga, snowy, desert; each with a `merchant_predicate` on the
   villager's variant), jungle and swamp explorer maps; level 3 ocean monument and trial chambers;
   level 5 woodland mansion. All `search_radius: 100`; `skip_existing_chunks` and `zoom` absent (defaults
   true and 2, n7 ExplorationMapFunction lines 3–9); then `filtered` on `map_id` with `on_fail:
   discard`. `gives` is a map item (`minecraft:plains_village_map` and so on), `additional_wants` a
   compass.
2. **`ExplorationMapFunction.run`** (n7 lines 56–142): no `ORIGIN` → unchanged; with
   `skipKnownStructures`, `claimStructureAt(origin)`: the start of a destination structure whose
   bounding box contains the origin (`StructureManager.getStructureAt(pos, set)`, n9 228–268) gets a
   reference if `canBeReferenced` (so a cartographer standing in a plains village does not sell a map
   to it); then `level.findNearestMapStructure(set, origin, 100, true)`; null → unchanged (then the
   `filtered` step discards it).
3. **The search** (`ChunkGenerator.findNearestMapStructure`, n7 ChunkGenerator 153–610, not read in
   full): the chunks searched are loaded to `STRUCTURE_STARTS` (`level.getChunk(x, z, STRUCTURE_STARTS)`:
   world generation up to that status for chunks never generated); a start is taken if valid and
   `tryAddReference` succeeds; the target is `placement.getLocatePos(start chunk)`, not a piece's
   position.
4. **Claims** (n10 StructureStart 19–36, 209–310; n9 StructureManager 453–462): `references` is an int
   saved in the start's chunk NBT as `references`; `getMaxReferences()` is 1, so each structure
   start can be claimed once; `StructureManager.addReference` also bumps `StructureCheck`'s cache.
5. **The map** (n10 MapItem 32–96, MapItemSavedData 204–259, 637–660): `MapItem.applyNewSavedData(
   level, stack, target.x, target.z, zoom 2, tracking true, unlimited true)`:
   `MapItemSavedData.createFresh`: `size = 128 << scale` (512); `cx = floor((x + 64.0) / size) · size +
   size / 2 − 64` (z likewise), locked false, dimension the level's; colours a new `byte[16384]`; a new
   id from `getFreeMapId()` (`MapIndex` in the **server's** storage, n7 ServerLevel 3289–3300) and
   `setMapData`; the stack's `map_id` component set.
6. **`renderBiomePreviewMap`** (n10 MapItem 610–839): `s = 1 << scale`; 128 × 128 `getBiome` calls at
   `((cx / s − 64 + i) · s, seaLevel, (cz / s − 64 + j) · s)` testing `#water_on_map_outlines`; then
   for each interior pixel (1..126 both ways) a count of watery neighbours among the 8 and the colour:
   watery pixel → `COLOR_ORANGE`, and with more than 7 watery neighbours, on even z only,
   `((x + (int)(Mth.sin(z + 0.0f) · 7.0f)) / 8) % 5` → 0 or 4 LOW, 1 or 3 NORMAL, 2 HIGH, else LOWEST;
   with more than 7 but odd z, NONE; more than 5 NORMAL; more than 3 LOW; more than 1 LOW; else
   LOWEST. Not watery with any watery neighbour → `COLOR_BROWN`, NORMAL above 3 else LOWEST. NONE
   is not written. (x is the first index, z the second, `colors[x + z · 128]`.)
7. **Decoration** (n10 MapItemSavedData 637–660, 1340–1347): `MapDecorations.Entry(type, x, z, 180.0f)`
   added to the stack's `map_decorations` component under the key `"+"`.
8. **Saved data** (n10 MapItemSavedData 1403–1473): `map_<id>` (`MapId.key()`, not dumped) with
   `dimension`, `xCenter`, `zCenter`, `scale` (byte, default 0), `colors` (byte buffer, required),
   `trackingPosition` (default true), `unlimitedTracking` (default false), `locked` (default false),
   `banners`, `frames` (lists, default empty). Size: 16384 bytes of colours per map, plus the item's
   component.

**Verdict on question 16.9's premise**: CONFIRMED that a failed search discards the offer
(`VillagerTrade.getOffer` returns null on an empty stack, n10 VillagerTrade 95–137); whether the
trade set then draws another is the trade set's filling rule (section 5.2, another reader's). The
count 12 is CORRECTED to 10. What maps would need beyond the item and its packet: structure-start
generation for unvisited chunks within 100 chunks' search, a write to the start's chunk (the
reference), and server-wide map ids, all world state across regions.

---------------------------------------------------------------------------------------

### 7. The `-v` files

#### 7.1 `InsideBrownianWalk` (n10 InsideBrownianWalk-v lines 276–546)

`create(speed)`: a `OneShot` with `WALK_TARGET` **absent**. Trigger (lambda$create$2, lines 337–419):
1. `level.canSeeSky(body.blockPosition())` → return false (no draws);
2. the 27 positions `BlockPos.betweenClosedStream(pos − (1,1,1), pos + (1,1,1))`, mapped by
   `BlockPos::immutable` (bootstrap 3), collected by `Util.toMutableList()`;
3. `java.util.Collections.shuffle(list)`: the JDK's static unseeded `Random` shared by the whole JVM
   (26 `nextInt(i)` for i = 27 down to 2, swap `i − 1` with the draw; JDK behaviour, not in the
   dumps). The list starts in `betweenClosed` order: x fastest, then y, then z (`x = i % 3`,
   `y = (i / 3) % 3`, `z = i / 9`, from the minimum corner; n7 BlockPos 874–990 and BlockPos$4);
4. filters in order: `!level.canSeeSky(p)` (bootstrap 4 → lambda$3), `level.loadedAndEntityCanStandOn(p,
   body)` (bootstrap 5 → lambda$4), `level.noCollision(body)` (bootstrap 6 → lambda$5: **ignores p**);
5. `findFirst`, if present `WALK_TARGET = new WalkTarget(p, speed, 0)` (bootstrap 7 → lambda$6);
6. return true.
Brief 1.5 and finding 11: CORRECTED (correction 9); "indoors only" and the JDK global random
CONFIRMED.

#### 7.2 `NearestItemSensor` (n10 NearestItemSensor-v, `doTick`)

`items = level.getEntitiesOfClass(ItemEntity, body.getBoundingBox().inflate(32.0, 16.0, 32.0),
e -> true)` (bootstrap 0); `items.sort(Comparator.comparingDouble(body::distanceToSqr))` (bootstrap 1;
`List.sort`, a stable merge sort: ties keep the entity query's order); then filters in order:
`body.wantsToPickUp(level, item.getItem())` (bootstrap 2), `item.closerThan(body, 32.0)` (bootstrap 3),
`body.hasLineOfSight(item)` (bootstrap 4); `findFirst` → `NEAREST_VISIBLE_WANTED_ITEM` (set or
cleared). **No random** of any kind. Constants `XZ_RANGE` 32, `Y_RANGE` 16,
`MAX_DISTANCE_TO_WANTED_ITEM` 32. Brief 1.5: CONFIRMED.

---------------------------------------------------------------------------------------

### 8. Classes needed and not found in any dump directory

- `net.minecraft.network.protocol.game.ClientboundBossEventPacket$Operation`, `$OperationType`
  (the ordinals), `$AddOperation`, `$UpdateProgressOperation`, `$UpdateNameOperation`,
  `$UpdateStyleOperation`, `$UpdatePropertiesOperation`, `$1` (the remove operation): the wire
  order of each operation. Until then the table in 3.3 is Unverified (Pumpkin); a comparison with the
  official server settles it as well.
- `net.minecraft.server.level.ServerBossEvent` with `javap -v` (which factory each broadcast lambda
  names; a confirmation only).
- For maps: `net.minecraft.world.level.saveddata.maps.MapId`, `net.minecraft.world.level.saveddata.maps.MapIndex`,
  `net.minecraft.world.item.component.MapDecorations` and `MapDecorations$Entry`,
  `net.minecraft.world.level.saveddata.maps.MapDecorationType`, `net.minecraft.world.level.saveddata.maps.MapDecorationTypes`,
  `net.minecraft.world.level.saveddata.maps.MapItemSavedData$HoldingPlayer`,
  `net.minecraft.network.protocol.game.ClientboundMapItemDataPacket`,
  `net.minecraft.world.level.levelgen.structure.placement.StructurePlacement` (`getLocatePos`).
- `net.minecraft.network.protocol.game.ClientboundSoundPacket` (the horn; only if Clustine's sound
  packet is not already checked against the official server).

### Settled by the lead afterwards (the boss event's classes, dumped with -v into n10-javap)

- `ClientboundBossEventPacket.write`: UUID, then the operation's type as `writeEnum` (a var-int
  ordinal), then the operation's own fields.
- `$OperationType` ordinals: ADD 0, REMOVE 1, UPDATE_PROGRESS 2, UPDATE_NAME 3, then (as in the
  file) UPDATE_STYLE 4, UPDATE_PROPERTIES 5.
- `$AddOperation.write`: name (component stream codec), progress `writeFloat`, colour `writeEnum`,
  overlay `writeEnum`, then one byte `encodeProperties(darkenScreen, playMusic, createWorldFog)`.
  The Pumpkin-derived order in this file agrees; the classes are now dumped for E1 to cite.
