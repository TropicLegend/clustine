# N10 porting brief: the villager's brain, trading, points of interest, raids and the village (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's), one file per
class, in `/home/claude/reference/` (`n5-javap/` to `n9-javap/`, `n7d-javap/`, `n7k-javap/`),
from the datagen's output in `target/datagen/26.3/generated/` (trades, trade sets, timelines,
tags, loot tables, registries) and from Clustine's code. A file named without its directory
is in `n7-javap/`; others say so (`n7k Raid.txt`, `n9 BellBlock.txt`). Line ranges are of the
`javap` output of the class named. Every rule is the bytecode's or the data's unless marked
**Inferred** (reasoning, or a class that is not dumped, from memory of the game's source: to
be confirmed by the dumps section 15 asks for), **Unverified (Pumpkin)** (read in the Rust
server under `/home/claude/reference/Pumpkin`, not authoritative) or **Client**. Cited as B:
`n7-brief.md` (B 7.1.5 is its section 7.1.5), K: `n7k-brief.md`, N9: `n9-brief.md`, P9:
`n9-plan.md`, NP: `n-plan.md`, A25, A27 and A28: ADR-0025, ADR-0027 and ADR-0028.

The manner is N9's: what the step should know first (section 0), then subject by subject
(1 to 8), the draws (9), what clients are sent (10), the line between regions (11), what a
player meets within minutes (12), what Clustine has to reuse (13), the sizes (14), what was
not dumped (15), the open questions (16) and a division of the work (17). B's conventions
hold: `double` is `f64`, `float` is `f32`, no fused multiply-add. "Own" is the entity's own
random (`getRandom()`); "level" is `level.getRandom()`; "list" is a `ShufflingList`'s own
clock-seeded random inside every `RunOne`, `GateBehavior` and `TriggerGate` (B 7.1.1.4). As
before (B 7.1.7, A28 section 4) a port draws all of these from the entity's stream, in the
order the game draws them, and parity is the distributions (the owner's answer to NP
question 5).

Where it sits: NP section 4 names N10 "Villagers' brain and trading, golems, raids, as a phase
of their own". Built before it: the villager as a body with a reduced brain (core without the
points of interest, idle, rest without a home, panic; K 2, `mobs/types/villager.rs`), the
brain machinery and its shared behaviours (N7j, B 7.1 and 7.3), the iron golem built from
blocks and fighting (K 1, N9), the wandering trader with its goals (K 2.7), N9's monsters,
patrols with their ominous banners, effects (bad omen and Hero of the Village among them),
natural spawning and the custom spawners for phantoms and patrols. Points of interest do not
exist in Clustine; every rule that reads them has a stand-in (section 13).

The research was done by seven readers in parallel (the villager's brain; points of interest
and work; the social behaviours; trading; raids; the golem, cats, the siege and the trader's
spawner; Clustine's code), each reading the dumps; where two disagreed the bytecode was read
again here and the finding says so.

---------------------------------------------------------------------------------------

## 0. What the step should know before planning (findings)

**Points of interest and the village**

1. **A point of interest is a counter, not a claim with an owner.** `PoiRecord` holds `pos`,
   `poiType` and `freeTickets` only (PoiRecord.txt 11–42); `acquireTicket` and
   `releaseTicket` count it down and up (62–104). Who holds the ticket is only in the
   villager's brain, as a `GlobalPos` memory (`home`, `job_site`, `potential_job_site`,
   `meeting_point`), saved with codecs. Every type a villager claims has one ticket but the
   bell (`meeting`, 32 tickets, valid range 6; PoiTypes.txt 121–287). Keeping tickets and
   memories consistent, across regions, is the whole problem of section 2.
2. **The index is per section, loaded on demand, and a query reaches 72 blocks.**
   `PoiManager` is a `SectionStorage` saved as `POI_CHUNK` (PoiManager.txt 11–41); every
   query goes through `getOrLoad` (**Inferred**: from disk, also for chunks that are not
   loaded). `getInSquare(r)` walks the chunks `rangeClosed(chunk, floorDiv(r,16)+1)` and every
   section Y (86–107, 127–147): for the villager's radius 48 that is 9 × 9 chunks, up to 72
   blocks on an axis, the results within 48 (`getInRange`, 109–125). A region's copies reach
   one chunk across its line (A25 section 3). **A villager near its region's edge reads points
   of interest far beyond anything a region has today.**
3. **"Village" means a claimed point of interest within one section.** `isVillageCenter(s)` is
   true for a *loaded* section holding a `#village` record (the 13 job sites, `home`,
   `meeting`) that is **occupied**, `freeTickets != maxTickets` (PoiManager.txt 362–381, its
   lambdas 616–633, read again here); `sectionsToVillage` is a distance tracker over those
   sources (350–360; **Inferred**, the tracker is not dumped: level 0 at a source, +1 to each
   of the 26 neighbouring sections, capped at 7, so the 3-D Chebyshev distance in sections).
   `ServerLevel.isCloseToVillage(pos, r)` is `r <= 6 && sectionsToVillage(section) <= r`,
   `isVillage(pos)` is `isCloseToVillage(pos, 1)` (n7d ServerLevel.txt 3387–3427). **A
   generated village is no village until its villagers have claimed beds, job sites or the
   bell**: raids, the siege, cats, the golem's strolls and the zombies' and foxes' village
   goals all wait on claims. The tracker's reach is 6 sections, 96 blocks, in three
   dimensions.
4. **Points of interest follow the blocks; only the tickets are not derivable.**
   `ServerLevel.updatePOIOnBlockStateChange` (n7d ServerLevel.txt ~3351–3378) does nothing
   when `PoiTypes.forState(old) == forState(new)` (a bed's `occupied`, a composter's level, a
   lectern's book never touch tickets), else removes the old type and adds the new, each in
   `server.execute` (**Inferred**: at once within the world tick). Structure generation
   registers them with full tickets (WorldGenRegion.txt 716). At a chunk's load
   `checkConsistencyWithBlocks` rebuilds an invalid or missing section from the blocks with
   **full tickets** (PoiManager.txt 421–437, PoiSection.txt 281–352): a rebuild silently
   frees every claim while the villagers' memories keep them. Clustine's chunks can derive
   the records from their blocks; the tickets must be kept and travel with the chunk.
5. **Removing a record tells nobody** (PoiSection.txt 154–197): a broken bed or lectern is
   found by its claimant's `ValidateNearbyPoi` (within 16 blocks, every tick), which erases
   the memory. **Releases come from anywhere**: `Villager.releasePoi` (Villager.txt
   1205–1227, 2117–2161) through `server.getLevel(gp.dimension())`, from `die`, from
   `SetWalkTargetFromBlockMemory` when a site is unreachable for 1200 ticks or farther than
   its limit, from `GoToPotentialJobSite`'s timeout. A release may cross any line.
6. **Query order is a hash order where the code takes the first.** `PoiSection.getRecords`
   streams a `HashMap<Holder, HashSet<PoiRecord>>` (PoiSection.txt 32, 62–76; **Inferred**
   identity-hashed holders, so the order differs between JVM runs); `take` keeps the first
   match (PoiManager.txt 255–269), `VillagerMakeLove` takes the first reachable bed, the bed
   sensor and `SetClosestHomeAsWalkTarget` keep the first four, `getRandom` shuffles a list in
   that order. A port fixes one order (chunk x fastest then z, section y, type id, position)
   and is judged by distribution.

**The brain**

7. **The provider and its schedule** (Villager.txt 2216–2355): nine sensors (nearest living,
   players, items, bed, hurt by, hostiles, babies, secondary points, golem) and the
   activities `PLAY` (babies) or `WORK` (adults, condition `job_site`), then `CORE`, `MEET`
   (condition `meeting_point`), `REST`, `IDLE`, `PANIC`, `PRE_RAID`, `RAID`, `HIDE`; no
   activity erases memories when it stops. The schedule is a step function of the
   **overworld clock's** `totalTicks` mod 24000 in every dimension (`timeline/
   villager_schedule.json`, `AttributeType.ofNotInterpolated`): adults idle from 10, work from
   2000, meet from 9000, idle from 11000, rest from 12000; babies idle 10, play 3000, idle
   6000, play 10000, rest 12000. It is read only by `UpdateActivityFromSchedule` (priority 99
   of work, play, rest, meet and idle), at most once in 21 ticks (Brain.txt 695–727); a
   scheduled activity whose condition fails gives the **default** activity, IDLE unless a raid
   changed it.
8. **Leaving the schedule is by three core behaviours at priority 0, in list order**:
   `VillagerPanicTrigger` (PANIC on `hurt_by` or `nearest_hostile`), `ReactToBell` (HIDE every
   tick while `heard_bell_time` is held and no raid is near), `SetRaidStatus` (one tick in 20,
   PRE_RAID or RAID, and the **default activity changed** to match). A later one overrides an
   earlier one in the same tick. Each comes back only through its own exit (`VillagerCalmDown`,
   `SetHiddenState`, `ResetRaidStatus`), each through the 21-tick gate.
9. **Two level draws every tick for every villager, standing still.** `VillagerPanicTrigger`
   is a `Behavior` 60/60 with no entry memories: calm, it starts and stops every tick, one
   `nextInt(1)` (VillagerPanicTrigger.txt 10–56); `SetRaidStatus` draws `nextInt(20)` every
   tick (SetRaidStatus.txt 22–61); after the brain, `customServerAiStep` draws own
   `nextInt(100)` (Villager.txt 190–261). Clustine already draws the first and the third
   (`villager.rs` 641–651); the second is new, and the streams' tests change with it.
10. **A brain is rebuilt whenever age crosses zero and at every profession change**
   (`ageBoundaryReached` 167–181, `refreshBrain` 121–142; `AssignProfessionFromJobSite`,
   `ResetProfession`, the cure): every running behaviour stopped, only memories with codecs
   kept, nine sensor delays drawn again. A load builds it three times (27 own draws). Clustine
   carries the whole live brain through crossings, merges and splits (B 7.1.1.1) and already
   has `remake` for `refreshBrain`.
11. **Corrections to K 2.4**: at night with no bed a villager does not stand still: the rest
   package's `RunOne` (entry `home` absent) holds `SetClosestHomeAsWalkTarget` 1,
   `InsideBrownianWalk` 4 (indoors only, shuffled by the JDK's global random), `GoToClosestVillage`
   2 and `DoNothing(20, 40)` 2, and `GoToClosestVillage` makes 15-block walks outside a
   village (Clustine has it). The villager does not use `BabyFollowAdult`: babies chase each
   other (`PlayTagWithOtherKids`, which reads other babies' `interaction_target`) and jump on
   beds.

**Work, beds and professions**

12. **Claiming a site is a path search to up to five candidates within 48 blocks.**
   `AcquirePoi` (AcquirePoi.txt 163–297) throttles with level draws (`nextInt(20)` first,
   then `20 + nextInt(20)` between scans), runs `findAllClosestFirstWithType(..., 48,
   HAS_SPACE)` with a retry cache whose test draws a level `nextInt(40)` for each due entry
   (on **every** match within 48, since the sort is a barrier), keeps five, and asks
   `createPath(targets, 1 or 6)` once with the villager's navigation (required path length 48,
   box ±56). Each failed target is put on a jittered back-off (+40 to 79 ticks, at most 400),
   a level `nextInt(40)` each.
13. **Profession comes from the job site and is lost at level 1.** Within 2 blocks of its
   potential site `AssignProfessionFromJobSite` makes it the job site, sends entity event 14
   and, if unemployed, takes the first profession in registry order whose `heldJobSite`
   accepts the type, then rebuilds the brain. `ResetProfession` makes a villager without a job
   site unemployed when its xp is 0 and its level at most 1. `YieldJobSite` hands an
   unemployed villager's reserved site to an employed neighbour that lost its own;
   `PoiCompetitorScan` erases the job site of the villager with less xp (the later on a tie).
   **These two write another villager's memories, 16 blocks away.**
14. **Beds**: only the head of the 16 coloured beds is a `home` point (the straw bed is not);
   a villager sleeps on an unoccupied bed in `#villagers_can_sleep_on_bed` within 2 blocks
   (`SleepInBed.txt` 24–201), `startSleeping` sets `occupied` on the head and the foot follows
   by its shape (n7d LivingEntity.txt 10398–10464, AbstractBedBlock.txt 234–281). A player
   sleeping in a villager's bed costs the villager its claim (`ValidateNearbyPoi`, 19–61).
   `LAST_SLEPT` is what golem summoning needs (finding 17).
15. **The farmer works blocks around it**: harvests ripe crops (`destroyBlock` with drops),
   plants seeds on air over farmland, bone-meals, composts seeds and makes bread at its
   composter; `SecondaryPoiSensor` reads 9 × 5 × 9 blocks every 40 ticks for every villager.
   **Restocking is only at the job site** (`WorkAtPoi.start`, finding 22), so without points of
   interest no villager ever restocks.

**The social life**

16. **Gossip flows one way per pair per 1200 ticks, and writes into the other villager**
   (`Villager.gossip`, 1709–1767): the receiver copies the partner's gossips by 10 own draws
   `nextInt(total)` weighted by `|value × weight|` (n7k GossipContainer.txt 83–183), then
   stamps `lastGossipTime` on **both**, then tries to summon a golem. Reputation is
   Σ value × weight (184–200). The constants are in `GossipType`, **not dumped** (section 4.1
   gives the **Inferred** values). A murder writes `major_negative` +25 into every
   villager the dying one sees within 16 (`tellWitnessesThatIWasMurdered`, 1168–1204).
17. **Golem summoning reads and writes every villager within 10 blocks.**
   `spawnGolemIfNeeded(level, t, n)` (1799–1850) runs from gossip (n = 5) and from panic every
   100 game ticks (n = 3): the villagers in its box grown by 10 that slept within 24000 ticks
   and have no `golem_detected_recently`, at most 5, at least n; `SpawnUtil.trySpawnMob(
   IRON_GOLEM, MOB_SUMMONED, ..., 10, 8, 6, LEGACY_IRON_GOLEM)` with level draws
   (SpawnUtil.txt 9–160); on success `golem_detected_recently` (599 ticks) into all of them.
   **No golem is ever summoned without beds** (`LAST_SLEPT`).
18. **Breeding writes into the partner and needs a free bed within 48 it can path to.**
   `VillagerMakeLove` (Behavior 350/350) locks both villagers' look and walk targets, sends
   entity events 18 and 12 to both, and at birth digests both parents' food, sets both ages to
   6000, `take`s a vacant `home` the parent can reach (a path search per candidate in stream
   order) and gives it to the child; with no such bed, angry particles and no baby.
19. **Bells**: `BellBlock.attemptToRing` (n9 BellBlock.txt 223–277) calls `BellBlockEntity.onHit`
   (**not dumped**; **Inferred**: every living entity within 32 gets `heard_bell_time`, raiders
   within 48 glow after the resonance). A player rings a bell and the villagers run to hide
   for 300 ticks (`ReactToBell`, `LocateHidingPlace`, `SetHiddenState`). In a raid's pre-raid
   activity villagers ring it themselves (`RingBell`, 5% a tick within 3 of the meeting point).

**Trading**

20. **Trades are data drawn from world-wide random sequences** (correcting B finding 19 and
   12.1, which said 26.3 has none): 391 `villager_trade` files, 68 `trade_set` files, nested
   tags. `addOffersFromTradeSet` (AbstractVillager.txt 387–534) draws from
   `MinecraftServer.getRandomSequence(set.randomSequence())` (n9 MinecraftServer.txt 157,
   396–399, 3781–3797: one stream per set for the whole server, saved): removes
   `list[nextInt(size)]` until `amount` offers succeed (2 for every set but librarian level 5
   (3) and the trader's common (5)); a failed offer (predicate, discarded item) is not
   counted. A cluster cannot share those streams; the stand-in is the entity's stream aside.
21. **The level-up is immediate in 26.3** (Villager.txt 1025–1091, read again here): own
   `3 + nextInt(4)` for the orb, the offer's xp added, `lastTradedPlayer` set, and if the
   threshold `{0, 10, 70, 150, 250}` is reached `increaseMerchantCareer` (the next level's set
   added), Regeneration 200 ticks and +5 to the orb, in the trade itself. There is no 40-tick
   delay. One level per trade.
22. **Restock is twice a day at the job site**: only `WorkAtPoi.start` calls `shouldRestock`
   (WorkAtPoi.txt 67–95, Villager.txt 430–625), after 300 ticks since its last check and a
   level `nextInt(2) == 0`, within 1.73 of the job site in WORK; `catchUpDemand` makes up
   missed restocks; demand raises prices (**Inferred** `MerchantOffer.updateDemand`).
23. **Prices**: `updateSpecialPrices` (645–749) lowers each offer by `floor(reputation ×
   priceMultiplier)` and, with Hero of the Village, by `max(1, floor((0.3 + 0.0625·amp) ×
   baseCostA))`; the cost is **Inferred** `clamp(base + max(0, floor(base · demand ·
   multiplier)) + special, 1, maxStack)`. After every trade the client is sent the offers again
   a tick later (the `TRADE` reputation event calls `updateSpecialPrices` for the trading
   player).
24. **The menu's classes are not dumped** (`MerchantMenu`, `MerchantContainer`,
   `MerchantResultSlot`, `MerchantOffer`, `ItemCost`, `VillagerTrade`, both packets); dumped are
   `handleSelectTrade` (n6 ServerGamePacketListenerImpl.txt 2568–2610),
   `sendMerchantOffers` (n6 ServerPlayer.txt 3250–3264), `MerchantOffers.getRecipeFor` (n7k
   27–70, with its quirk: a selection hint above 0 tests only that offer). Clustine's N6 menu
   machinery fits (a mob-backed menu exists: the horse's), but the protocol has neither packet
   nor the menu type `minecraft:merchant` (19), nor an `ItemCost` codec.
25. **Trades need what Clustine lacks**: `EnchantmentHelper.enchantItem` (librarian books,
   enchanted gear; `mobs/making.rs` 162–179 is a stub), the components `stored_enchantments`
   and `dyed_color`, the `add` number provider, the loot functions `enchant_with_levels`,
   `enchant_randomly`, `set_random_dyes`, `set_random_potion`, `set_stew_effect`, `filtered`,
   and for the cartographer `exploration_map` with map data and structure location (12 of its
   31 trades).

**Raids and the rest of the village**

26. **A raid is per-level world state reaching 115 blocks from its centre, with its own
   unseeded random** (n7k Raid.txt 87–166, Raids.txt 105–465): the centre is the mean of the
   occupied `#village` points within 64 of the player; `getRaidAt` is the nearest active raid
   within 96 (ServerLevel.txt 3435–3442); wave positions reach about 104 blocks with
   `hasChunksAt(±10)`; members beyond 112 are dropped; the horn reaches players within 64 of
   the spawn point. It is read by every villager (`SetRaidStatus`, one tick in 20), every
   raider without a raid (every 20 ticks), every vindicator (every tick) and the bad omen
   effect.
27. **On easy a raid is three waves plus a bonus wave and includes a ravager**: wave 1 four
   pillagers, wave 2 two vindicators and three pillagers, wave 3 three pillagers and a
   ravager, each vindicator and pillager slot `+nextInt(2)` (**Inferred** `Raid$RaiderType`
   counts; n7k Raid.txt 1952–2079). **The ravager is not built** and must be for raids.
   Witches and evokers never come on easy.
28. **A raid starts from bad omen, raid omen and a stored position**: an ominous bottle gives
   bad omen (built), which in a village becomes raid omen for 600 ticks and stores
   `raid_omen_position` on the player, and at its last tick `createOrExtendRaid` runs
   (**Inferred**: `BadOmenMobEffect`, `RaidOmenMobEffect` not dumped; n9 ServerPlayer.txt 117,
   399–408, 5407–5424 for the position). Clustine's player effects only count down
   (`region/player_effects.rs` 232–257): their ticks are the first thing to add.
29. **The boss bar and the horn are new packets** (`ClientboundBossEventPacket`, not dumped):
   red, notched 10, `event.minecraft.raid`, filling over the 300-tick countdown then showing the
   raiders' health; the horn a sound packet to each player within 64 of the spawn point.
30. **`DefendVillageTargetGoal` never fires against a creative player** (DefendVillageTargetGoal.txt
   30–120: a villager's reputation ≤ −100 and a player neither creative nor spectator), so the
   golem's village defence a player meets is only against monsters. The golem's two strolls,
   the cat spawner, the siege and the trader's spawner need the village (finding 3); the
   trader's spawner keeps a chance that rises day by day as **world state** (section 8.4).

---------------------------------------------------------------------------------------

## 1. The villager's brain

### 1.1 Provider, memories and construction (Villager.txt)

- **Sensors** (static 2313–2355), in tick order, with their scan rates (each draws own
  `nextInt(rate)` at every brain build, B 7.1.3): `NEAREST_LIVING_ENTITIES` 20,
  `NEAREST_PLAYERS` 20, `NEAREST_ITEMS` 20, `NEAREST_BED` 20, `HURT_BY` 20,
  `VILLAGER_HOSTILES` 20, `VILLAGER_BABIES` 20, `SECONDARY_POIS` 40, `GOLEM_DETECTED` 200.
- **The activity supplier** (`lambda$static$0`, 2216–2311), a new set per brain:

  | # | Activity | Package | Condition |
  |---|---|---|---|
  | 1 | `PLAY` (baby) or `WORK` (adult) | `getPlayPackage(0.5)` / `getWorkPackage(profession, 0.5)` | — / `job_site` present |
  | 2 | `CORE` | `getCorePackage(profession, 0.5)` | — |
  | 3 | `MEET` | `getMeetPackage(0.5)` | `meeting_point` present |
  | 4 | `REST` | `getRestPackage(0.5)` | — |
  | 5–9 | `IDLE`, `PANIC`, `PRE_RAID`, `RAID`, `HIDE` | their packages | — |

  None erases memories when it stops. The profession baked into CORE and WORK is the one at
  the build.
- `POI_MEMORIES` (static 87–122, 2183–2215): `home` → is HOME, `job_site` → the profession's
  `heldJobSite`, `potential_job_site` → any acquirable job, `meeting_point` → is MEETING;
  `releasePoi` tests it.
- `makeBrain` (108–120) then `registerBrainGoals` (143–166): `setSchedule(baby ?
  BABY_VILLAGER_ACTIVITY : VILLAGER_ACTIVITY)` and `updateActivityFromSchedule` once, which
  always acts (a new brain's `lastScheduleUpdate` is −9999). `refreshBrain` (121–142): stop
  all, pack, make, register. `ageBoundaryReached` (167–181), from `AgeableMob.setAge` (285–315)
  whenever the age's sign changes, calls `refreshBrain`: a baby growing up switches from PLAY
  to WORK in that call. `readAdditionalSaveData` (832–920) ends with `refreshBrain`.
- **`customServerAiStep`** (190–261), in order: `brain.tick`; if `lastTradedPlayer`, the
  `TRADE` reputation event, entity event 14, clear; unless `NoAI`, own `nextInt(100)` and at 0
  `getRaidAt` and entity event 42 if a raid is active and not over; stop trading with no
  offers; `Mob.customServerAiStep` (empty). The villager has no goals.
- **`tick`** (262–278): `AbstractVillager.tick` (`LivingEntity.tick` stops sleeping when the bed
  is gone, LivingEntity.txt 7495–7508), the unhappy counter, `maybeDecayGossip`.

### 1.2 The schedule (n7 and n7d timeline classes; the datagen)

`EnvironmentAttributes.VILLAGER_ACTIVITY` and `BABY_VILLAGER_ACTIVITY` (EnvironmentAttributes.txt
594–607) are of type `ACTIVITY`, default `IDLE`, `ofNotInterpolated` (AttributeTypes.txt 152–157,
AttributeType.txt 82–101, LerpFunction.txt 116–125: a keyframe's value holds until the next and
switches on its tick). The track's clock is `minecraft:overworld`; `villager_schedule` is in
`#universal`, which every dimension's timeline tag includes, so the Nether's and the End's
villagers follow the overworld's day. No biome sets these attributes. Clustine hard-codes the
same table (`villager.rs` 275–298) from `WorldClock`.

| `totalTicks mod 24000` | adult | baby |
|---|---|---|
| 0–9 | rest | rest |
| 10–1999 | idle | idle |
| 2000–2999 | work | idle |
| 3000–5999 | work | play |
| 6000–8999 | work | idle |
| 9000–9999 | meet | idle |
| 10000–10999 | meet | play |
| 11000–11999 | idle | play |
| 12000–23999 | rest | rest |

`Brain.updateActivityFromSchedule` (Brain.txt 695–727): only when `gameTime − last > 20`; then
`a` is the attribute's value; if not active, `setActiveActivityIfPossible(a)` (614–626): its
conditions hold → `setActiveActivity(a)` (628–652), else `useDefaultActivity()`.
`setDefaultActivity` (752–757) is used by the raid behaviours only.

### 1.3 Switching, in one tick

| Behaviour | Where | Does | Draws |
|---|---|---|---|
| `VillagerPanicTrigger` (Behavior 60/60, no entry) | core 0, 4th | start: if `hurt_by` or `nearest_hostile` and PANIC not active, erase `path`, `walk_target`, `look_target`, `breed_target`, `interaction_target`; `setActiveActivityIfPossible(PANIC)` (23–57). tick: at `gameTime % 100 == 0`, `spawnGolemIfNeeded(t, 3)` | level `nextInt(1)` at each start (every tick while calm) |
| `ReactToBell` (one-shot, P `heard_bell_time`) | core 0, 6th | if `getRaidAt(pos) == null`, HIDE (32–47) | — |
| `SetRaidStatus` (point one-shot) | core 0, 7th | level `nextInt(20) == 0`; a raid near: `hasFirstWaveSpawned && !isBetweenWaves` → RAID else PRE_RAID, both as default and active (22–62) | level `nextInt(20)` every tick |
| `VillagerCalmDown` (one-shot) | panic 0 | not scared (`hurt_by`, `nearest_hostile`, or `hurt_by_entity` within 6): erase `hurt_by`, `hurt_by_entity`, schedule (45–88) | — |
| `SetHiddenState(15, 3)` (one-shot, closure counter) | hide 0 | bell heard 300 ago or 300 ticks hidden: erase both memories, schedule, counter 0; else within 3 of the hiding place, counter + 1 (59–117) | — |
| `ResetRaidStatus` (one-shot) | pre-raid, raid 99 | level `nextInt(20) == 0`; raid gone, stopped or lost: default IDLE, schedule (22–59) | level `nextInt(20)` every tick |
| `UpdateActivityFromSchedule` (point one-shot) | 99 of work, play, rest, meet, idle | `brain.updateActivityFromSchedule(...)` (22–35) | — |

A switch made by a start is seen by the starts after it in the same tick (Brain's start pass
checks active activities as it goes, B 7.1.5). A victory does not reset the default: villagers
stay in RAID, celebrating, until the raid stops.

### 1.4 The packages (VillagerGoalPackages.txt, 987 lines)

Speed 0.5; at one priority the activities go in bucket order (B 7.1.1.6): PLAY (0) before CORE
(1); REST shares bucket 1, inserted later; HIDE 2, MEET 4, IDLE and PRE_RAID 5, WORK 6, PANIC
and RAID 15. So PLAY's own `MoveToTargetSink(80, 120)` at 0 paths a baby's walk before core's.

**CORE** (17–180): 0 `Swim(0.8)`, `InteractWithDoor`, `LookAtTargetSink(45, 90)`,
`VillagerPanicTrigger`, `WakeUp`, `ReactToBell`, `SetRaidStatus`, `ValidateNearbyPoi(heldJobSite,
job_site)`, `ValidateNearbyPoi(acquirableJobSite, potential_job_site)`; 1 `MoveToTargetSink()`;
2 `PoiCompetitorScan`; 3 `LookAndFollowTradingPlayerSink(0.5)`; 5 `GoToWantedItem(0.5, false,
4)`; 6 `AcquirePoi(acquirableJobSite, job_site, potential_job_site, adults only, no event)`; 7
`GoToPotentialJobSite(0.5)`; 8 `YieldJobSite(0.5)`; 10 `AcquirePoi(home, babies too, event 14,
validateBedPoi)`, 10 `AcquirePoi(meeting, adults, event 14)`, 10 `AssignProfessionFromJobSite`,
10 `ResetProfession`.

**WORK** (202–313; adults with `job_site`): 2 `SetWalkTargetFromBlockMemory(job_site, 0.5, 9,
100, 1200)`; 3 `GiveGiftToHero(100)`; 5 the minimal look; 5 `RunOne` of the work behaviour
(`WorkAtComposter` for a farmer, else `WorkAtPoi`) 7, `StrollAroundPoi(job_site, 0.4, 4)` 2,
`StrollToPoi(job_site, 0.4, 1, 10)` 5, `StrollToPoiList(secondary_job_site, 0.5, 1, 6,
job_site)` 5, `HarvestFarmland` (2 for a farmer, else 5), `UseBonemeal` (4 for a farmer, else
7); 10 `ShowTradesToPlayer(400, 1600)`; 10 `SetLookAndInteract(player, 4)`; 99 schedule.

**PLAY** (314–389; babies): 0 `MoveToTargetSink(80, 120)`; 5 the full look; 5
`PlayTagWithOtherKids`; 5 `RunOne` (entry `visible_villager_babies` absent) of
`InteractWith(villager, 8, interaction_target, 0.5, 2)` 2, `InteractWith(cat, ...)` 1,
`VillageBoundRandomStroll(0.5)` 1, `SetWalkTargetFromLookTarget(0.5, 2)` 1, `JumpOnBed(0.5)` 2,
`DoNothing(20, 40)` 2; 99 schedule.

**REST** (390–454): 2 `SetWalkTargetFromBlockMemory(home, 0.5, 1, 150, 1200)`; 3
`ValidateNearbyPoi(home)`; 3 `SleepInBed`; 5 `RunOne` (entry `home` absent) of
`SetClosestHomeAsWalkTarget` 1, `InsideBrownianWalk` 4, `GoToClosestVillage(0.5, 4)` 2,
`DoNothing(20, 40)` 2; 5 the minimal look; 99 schedule.

**MEET** (455–534; with `meeting_point`): 2 `TriggerGate.triggerOneShuffled` of
`StrollAroundPoi(meeting_point, 0.4, 40)` 2 and `SocializeAtBell` 2; 10 `ShowTradesToPlayer`;
10 `SetLookAndInteract(player, 4)`; 2 `SetWalkTargetFromBlockMemory(meeting_point, 0.5, 6, 100,
200)`; 3 `GiveGiftToHero(100)`; 3 `ValidateNearbyPoi(meeting)`; 3 a gate (erasing
`interaction_target`) of `TradeWithVillager`; 5 the full look; 99 schedule.

**IDLE** (535–663): 2 `RunOne` of `InteractWith(villager, 8, interaction_target, 0.5, 2)` 2,
`InteractWith(villager, 8, canBreed, canBreed, breed_target, 0.5, 2)` 1 (**Inferred**
predicates), `InteractWith(cat, 8, ...)` 1, `VillageBoundRandomStroll(0.5)` 1,
`SetWalkTargetFromLookTarget(0.5, 2)` 1, `JumpOnBed(0.5)` 1, `DoNothing(30, 60)` 1; 3
`GiveGiftToHero(100)`; 3 `SetLookAndInteract(player, 4)`; 3 `ShowTradesToPlayer(400, 1600)`;
3 the `TradeWithVillager` gate; 3 a gate (erasing `breed_target`) of `VillagerMakeLove`; 5 the
full look; 99 schedule.

**PANIC** (664–700; speed 0.75): 0 `VillagerCalmDown`; 1 `SetWalkTargetAwayFrom.entity(
nearest_hostile, 0.75, 6, false)`; 1 the same for `hurt_by_entity`; 3
`VillageBoundRandomStroll(0.75, 2, 2)`; 5 the minimal look. No schedule.

**PRE_RAID** (701–737): 0 `RingBell`; 0 `TriggerGate.triggerOneShuffled` of
`SetWalkTargetFromBlockMemory(meeting_point, 0.75, 2, 150, 200)` 6 and
`VillageBoundRandomStroll(0.75)` 2; 5 the minimal look; 99 `ResetRaidStatus`.

**RAID** (738–787): 0 `sequence(triggerIf(#8), triggerOneShuffled[MoveToSkySeeingSpot(0.5) 5,
VillageBoundRandomStroll(0.55) 2])`; 0 `CelebrateVillagersSurvivedRaid(600, 600)`; 2
`sequence(triggerIf(#9), LocateHidingPlace(24, 0.7, 1))`; 5 the minimal look; 99
`ResetRaidStatus`. The two predicates are bootstrap arguments `javap -c` does not print; the
readers disagreed. **Inferred**, from what each branch is for: #8 is `raidExistsAndNotVictory`,
which despite its name returns `raid != null && raid.isVictory()` (938–954), so villagers go to
open sky to celebrate after a victory (`CelebrateVillagersSurvivedRaid` needs no blocks above);
#9 is `raidExistsAndActive` (915–937: active, neither won nor lost), so they hide during the
raid. To be confirmed with `javap -v` (section 15).

**HIDE** (788–810): 0 `SetHiddenState(15, 3)`; 1 `LocateHidingPlace(32, 0.625, 2)`; 5 the
minimal look.

**The looks**: the full look (811–883), 5 `RunOne` of `SetEntityLookTarget` cat 8, villager 2,
player 2, by category creature, water creature, axolotls, underground water creature, water
ambient, monster 1 each, `DoNothing(30, 60)` 2 (10 children); the minimal look (884–914), 5
`RunOne` of villager 2, player 2, `DoNothing(30, 60)` 8.

**A `RunOne` alternates a decision with a wait** (derived): its one-shot children need
`walk_target` absent and stop in the tick they fire, so while a walk target is held only
`DoNothing` can start. Every gate start makes one list draw per child (idle 7, full look 10,
minimal 3, play 6, rest 4, work 6; a `TriggerGate` 2).

### 1.5 Movement, rest and play behaviours

- **`SetWalkTargetFromBlockMemory(mem, speed, close, tooFar, tooLong)`** (one-shot; 58–171): the
  site in another dimension, or `cant_reach_walk_target_since` older than `tooLong`: release
  the point of interest, erase the memory, `cant_reach = now`. Farther than `tooFar` (Manhattan):
  `DefaultRandomPos.getPosTowards(15, 7, site, π/2)` (own draws) until one is non-null, **up to
  1000 calls**, then release and erase; else walk to the site.
- **`StrollToPoi`**, **`StrollAroundPoi`**, **`StrollToPoiList`**: one-shots with a closure
  `nextOk` (+80, +180, +100); `StrollAroundPoi` takes `LandRandomPos.getPos(8, 6)` (own) and
  erases the walk target when it finds none; `StrollToPoiList` draws level `nextInt(size)`
  **before** its timer test.
- **`VillageBoundRandomStroll(speed, h = 10, v = 7)`** (54–104): in a village
  `LandRandomPos.getPos`; else towards the centre of `findSectionClosestToVillage(s, 2)`
  (BehaviorUtils.txt 215–238, 442–452: the least `sectionsToVillage` below the own section's in
  the 5 × 5 × 5 cube, the first of equals in `SectionPos$1` order) by
  `DefaultRandomPos.getPosTowards(π/2)`, else `LandRandomPos`.
- **`GoToClosestVillage(speed, 4)`** (39–115): not in a village; up to five
  `LandRandomPos.getPos(15, 7, p → −sectionsToVillage(p))`, taking one nearer a village at once
  or one as near and going on.
- **`SetClosestHomeAsWalkTarget`** (62–223): every 20 ticks or more, `findClosest(home, 48)`;
  level `nextInt(20)`; up to four uncached beds by `findAllWithType`, one path search to them
  (`AcquirePoi.findPathToPois`).
- **`InsideBrownianWalk`** (36–125): indoors only; shuffles the 27 neighbours with the JDK's
  global random; the first not seeing sky that a mob can stand on.
- **`MoveToSkySeeingSpot`** (16–122): up to 10 tries of own `nextInt(20) − 10`, `nextInt(6) −
  3`, `nextInt(20) − 10`; the first that sees sky with the heightmap at or below the villager.
- **`SleepInBed`** (Behavior 60/60, never times out; 7–220): not riding, home in this
  dimension, not within 100 ticks of `last_woken`, within 2 of the bed's centre, the bed free;
  start: close the doors it opened, `startSleeping`, `last_slept = now`, erase the walk target
  and `cant_reach`; stop: `stopSleeping` (which sets `last_woken`), `nextOk = now + 40`.
  **`WakeUp`** (core 0): REST not active and sleeping → `stopSleeping`.
- **`PlayTagWithOtherKids`** (199–278): level `nextInt(10) == 0`; if a visible baby's
  `interaction_target` is this one, flee (up to 10 × `LandRandomPos.getPos(20, 8)` into a
  village); else chase the least-chased baby (a `HashMap` keyed by entity, whose hash is the
  network id: the order differs between runs), else the first.
- **`JumpOnBed`** (babies; 21–349): level `3 + nextInt(4)` jumps at start, 100 ticks to reach the
  bed.
- **`NearestItemSensor`** (21–66): items in the box grown by 32, 16, 32 the villager wants (tag
  `villager_picks_up`, `villager_food`, the profession's requested items, and room) within 32 and
  seen; `GoToWantedItem` goes for those within 4 only.
- **`VillagerBabiesSensor`** (15–50): the visible villagers that are babies.

### 1.6 The draws of one villager tick, in the game's order

At a brain build: own `nextInt(20)` × 7, `nextInt(40)`, `nextInt(200)`.

Each tick, inside `Mob.serverAiStep` → `customServerAiStep`:

1. Sensors: only a baby's bed sensor draws (level `nextInt(20)`).
2. Starts by priority, then bucket, then insertion:
   - priority 0: PLAY's sink; CORE: `Swim` (level `nextInt(1)` in fluid), `LookAtTargetSink`
     (level `nextInt(46)` with a look target), **`VillagerPanicTrigger` (level `nextInt(1)`)**,
     `ReactToBell`, **`SetRaidStatus` (level `nextInt(20)`)**; HIDE, PRE_RAID (`RingBell` level
     `nextFloat`, the gate's 2 list draws), PANIC, RAID (2 list draws; `MoveToSkySeeingSpot` 3
     own a try or a stroll's 30 own; `Celebrate...` level at start);
   - priority 1: CORE's sink (own draws when it paths, level `nextInt(101)` at start); HIDE's
     `LocateHidingPlace` (own shuffle); PANIC's two `SetWalkTargetAwayFrom` (up to 10 ×
     `getPosAway`, own);
   - priority 2: REST's and WORK's walk-target memories; MEET's gate (2 list); IDLE's `RunOne`
     (7 list, then the first child that fires: a stroll's 30 own, `JumpOnBed`'s level draws,
     `DoNothing` level `nextInt(31)`); CORE's `PoiCompetitorScan`; RAID's hiding place;
   - priority 3 to 10: CORE's trading sink, `GoToWantedItem`, the three `AcquirePoi` (level,
     section 3.1), `GoToPotentialJobSite` (level `nextInt(1)`); REST's `SleepInBed` (level
     `nextInt(1)`); the gifts, trades and love of IDLE, MEET and WORK (section 4); the looks (10
     or 3 list, `DoNothing` level `nextInt(31)`); WORK's `RunOne` (6 list; section 3.3);
   - priority 99: the schedule (none) or **`ResetRaidStatus` (level `nextInt(20)`)**.
3. The running pass: `Swim` own `nextFloat`; `MoveToTargetSink.stop` level `nextInt(40)` when
   stuck; `VillagerMakeLove` own `nextInt(35)`; the celebration's own draws.
4. Own `nextInt(100)` unless `NoAI`.

**Steady state** (an adult in IDLE, calm, on land, nothing in view, gates free): level
`nextInt(1)`, level `nextInt(20)`, 7 list draws then a stroll's 30 own or `DoNothing`'s level
`nextInt(31)`, 10 list draws then perhaps `nextInt(31)`, the `AcquirePoi` throttles, own
`nextInt(100)`.

### 1.7 The tick budget

Per villager: about 0.38 sensor scans a tick (the nearest-living box ±16 sorted, all players,
items in 64 × 32 × 64, 405 block reads every 40 for secondary points); a baby's bed sensor
paths to up to four beds every 20 ticks; `AcquirePoi` and `SetClosestHomeAsWalkTarget` path
finding every 20 to 40 ticks while homeless or jobless; `SetWalkTargetFromBlockMemory` up to
10,000 random positions in its worst case; `GoToClosestVillage` 150 own draws and 50 village
lookups. A village of twenty villagers at night, before they have beds, is the heaviest case to
measure.

---------------------------------------------------------------------------------------

## 2. Points of interest and the village

### 2.1 Types (PoiTypes.txt 75–451, PoiType.txt)

| Type | Blocks | Tickets | Valid range |
|---|---|---|---|
| armorer | blast furnace | 1 | 1 |
| butcher | smoker | 1 | 1 |
| cartographer | cartography table | 1 | 1 |
| cleric | brewing stand | 1 | 1 |
| farmer | composter | 1 | 1 |
| fisherman | barrel | 1 | 1 |
| fletcher | fletching table | 1 | 1 |
| leatherworker | cauldron, water, lava and powder snow cauldrons | 1 | 1 |
| librarian | lectern | 1 | 1 |
| mason | stonecutter | 1 | 1 |
| shepherd | loom | 1 | 1 |
| toolsmith | smithing table | 1 | 1 |
| weaponsmith | grindstone | 1 | 1 |
| home | the 16 coloured beds, `part=head` only | 1 | 1 |
| meeting | bell | 32 | 6 |
| beehive, bee_nest, nether_portal, lodestone, test_instance_block, lightning rods (all weathering) | | 0 | 1 |

Tags (datagen; Clustine has them, `clustine-data` generated tags 4570–4592):
`acquirable_job_site` (the 13 jobs), `village` (`#acquirable_job_site`, home, meeting),
`bee_home`. `PoiTypes` is code: Clustine writes the table by hand from this dump, as
`villagers.rs` was.

### 2.2 Record, section, manager

- **`PoiRecord`**: born with full tickets (30–42); `acquireTicket` fails at 0; `releaseTicket` at
  max; `hasSpace` = free > 0; `isOccupied` = free ≠ max; equal and hashed by position.
- **`PoiSection`**: records by section-relative position and by type; `add` refuses the same
  type at a position and overwrites another with a logged mismatch (106–152); `remove` (154–197)
  tells nobody; `release` throws without a record (213–240); `refresh` (281–305) rebuilds from
  blocks keeping old records (and tickets) where a point of interest still is; saved as
  `(Valid, Records)` (**Inferred** field names; `$Packed` not dumped).
- **`PoiManager` queries** (`Occupancy` not dumped; **Inferred** `HAS_SPACE`, `IS_OCCUPIED`,
  `ANY`):

  | Method | Rule |
  |---|---|
  | `getInSquare(pred, c, r, occ)` 86–107 | chunks `rangeClosed(chunkOf(c), floorDiv(r,16)+1)` (x fastest, then z), every section y, `|dx|, |dz| ≤ r` |
  | `getInRange` 109–125 | and `distSqr ≤ r²` (3-D) |
  | `findAll`, `findAllWithType` 149–176 | filters of it |
  | `findAllClosestFirstWithType` 178–191 | stable sort by distance (a barrier: every filter runs on every match) |
  | `find` 193–203 | the first in stream order, not the nearest |
  | `findClosest` 205–253 | the minimum by distance |
  | `take(type, bipred, c, r)` 255–269 | the first with space passing the predicate; `acquireTicket`, its result ignored |
  | `getRandom(..., random)` 271–290 | `Util.toShuffledList` (one `nextInt(i)` for i = n down to 2), then the first passing |
  | `release`, `exists`, `getType` 292–334 | by position, loading on demand |
  | `getCountInRange` 66–75 | the count of `getInRange` |
  | `sectionsToVillage` 350–360 | run the tracker's updates, read its level |

- **The distance tracker** is updated at every `setDirty(section)` (add, remove, take,
  release; 392–406), at `onSectionLoad` (408–419) and in `tick` (382–390, from `ChunkMap.tick`).
  `isVillageCenter` reads only **loaded** sections (`get`, not `getOrLoad`): **Inferred**, a
  section's unload lowers nothing until a later update.
- **At a chunk's load** (ChunkMap.txt 1092–1110, 3426; `SerializableChunkData` not dumped):
  `checkConsistencyWithBlocks` per section (finding 4). At save, `ChunkMap.save` flushes the
  chunk's points of interest (1561–1567).

### 2.3 Who reads "village"

`isVillage`, `isCloseToVillage` and `sectionsToVillage` are read by: `VillageBoundRandomStroll`,
`GoToClosestVillage`, `PlayTagWithOtherKids`, `BehaviorUtils.findSectionClosestToVillage`;
`GolemRandomStrollInVillageGoal`, `MoveBackToVillageGoal`, `MoveThroughVillageGoal` (zombies),
`StrollThroughVillageGoal` (foxes), `Fox$SeekShelterGoal`, `PathfindToRaidGoal`; the siege, the
cat spawner, the patrol spawner, `Raid` and `Raids`, the bad omen effect. Points of interest by
type are read by the trader spawner (meeting within 48), the cat spawner (occupied homes within
48), raids (occupied `#village` within 64), the bee's hive search (`#bee_home` within 20), the
lightning rod (within 128, ServerLevel.txt 1116–1123).

---------------------------------------------------------------------------------------

## 3. Job sites, professions, work and beds

### 3.1 `AcquirePoi` (AcquirePoi.txt; `$JitteredLinearRetry`)

Each instance keeps `nextScheduledPoiSearch` and a `batchCache` of retries (33–46): per
villager, three instances (job, home, meeting), lost at a brain rebuild. Entry: the memory to
set absent (and for the job, `job_site` absent too). Trigger (163–297):

```
if onlyIfAdult and baby: return false
if next == 0: next = t + level.nextInt(20); return false          # level
if t < next: return false
next = t + 20 + level.nextInt(20)                                 # level
batchCache.removeIf(t − previousAttempt >= 400)
set = findAllClosestFirstWithType(type, cacheTest, pos, 48, HAS_SPACE)   # cacheTest on every match:
        .limit(5).filter(validPoi).collect(toSet())                     #   due entry → level nextInt(40)
path = findPathToPois(mob, set)      # one createPath(targets, max(1, validRange)): 1, or 6 for bells
if path and path.canReach():
    take(type, p == target, target, 1); memory = GlobalPos(target); [entity event 14]; batchCache.clear()
else:
    for p in set (HashSet order): batchCache.computeIfAbsent(p, new JLR(level random, t))   # level nextInt(40) each
return true
```

`JitteredLinearRetry`: `delay = min(delay + 40 + nextInt(40), 400)`, retry at `t + delay`.
`validateBedPoi` (Villager.txt 181–200): a bed of `#villagers_can_sleep_on_bed`, not occupied.

### 3.2 The other point-of-interest behaviours

- **`ValidateNearbyPoi(pred, mem)`** (62–146; core 0 every tick, rest and meet 3): the memory's
  site in this dimension and within 16; gone or of another type → erase; a bed occupied by
  someone not this villager → erase, and release unless a sleeping villager is in it.
- **`YieldJobSite`** (120–211): unemployed adult with a reserved site; the first nearby villager
  (alive, not this) with no reservation whose profession holds this type and that either has no
  job site and can reach the site (**a path search with the other's navigation**) or whose job
  site is this one: erase own walk, look and reservation; set the other's walk and look targets
  and `potential_job_site`. The ticket is not released: it passes.
- **`PoiCompetitorScan`** (9–169): every tick with a job site, reduce the nearby villagers
  holding the same `GlobalPos` and profession; `a` wins only with more xp; the loser's job site
  erased, no release.
- **`AssignProfessionFromJobSite`** (39–133) and **`ResetProfession`** (32–75): finding 13.
  Registry order: none, armorer, butcher, cartographer, cleric, farmer, fisherman, fletcher,
  leatherworker, librarian, mason, nitwit, shepherd, toolsmith, weaponsmith (VillagerProfession.txt
  165–690). Nitwits never claim; babies hold no job.
- **`GoToPotentialJobSite`** (Behavior 1200/1200, level `nextInt(1)`): only with no active
  activity but IDLE, WORK or PLAY; walks to the site every tick; at stop, if the reservation is
  still there (a timeout), releases it and erases.

### 3.3 Work

- **`WorkAtPoi`** (Behavior 60/60; 20–133): extra condition `t − lastCheck >= 300`, then **level
  `nextInt(2) == 0`** (drawn each tick the `RunOne` tries it, `lastCheck` unchanged on failure),
  then `lastCheck = t`, the job site within 1.73. Start: `last_worked_at_poi`, look at the site,
  the profession's work sound, `useWorkstation`, then restock if due (section 5.6).
- **`WorkAtComposter`** (11–181): at the composter, `makeBread` (wheat by threes to at most
  three bread, unless more than 36 bread), then `compostItems` (seeds beyond 10 of each, at most
  20, inserted one by one through `ComposterBlock.insertItem`, n6 ComposterBlock.txt 265–452;
  extracting bone meal first at level 8), level event 1500. Clustine has no composter recipes yet
  (`region/hoppers.rs` 29–30).
- **`HarvestFarmland`** (Behavior 60/60; 31–448): `mob_griefing` and farmer; the 3 × 3 × 3 around
  it, ripe crops and air over farmland; level `nextInt(size)` to pick; walks; breaks ripe crops
  with drops, plants the first seed (`#villager_plantable_seeds`) with `crop_planted`, picks again
  (level), works up to 200 ticks (the 60-tick timeout first).
- **`UseBonemeal`** (Behavior 60/60; 27–287): every 10th tick, 160 after its last session, bone
  meal held; reservoir sampling over the 3 × 3 × 3 crops (a level `nextInt(count)` per crop);
  shows bone meal in hand; `BoneMealItem.growCrop` (level draws), level event 1505.
- **`SecondaryPoiSensor`** (5–99): the 9 × 5 × 9 blocks round the villager (x outer, y, z) of the
  profession's secondary blocks (the farmer's farmland) into `secondary_job_site`.

### 3.4 Beds

`LivingEntity.startSleeping(pos)` (n7d 10398–10464): a bed with a sleep height; dismount;
position `(x + 0.5, y + h + 0.125, z + 0.5)`; `occupied = true` on the head; pose sleeping;
sleeping position. `stopSleeping` (10543–, lambdas 11255–11364): where the bed is loaded,
`occupied = false`, the bed rule, a stand-up position; pose standing. A player's use of an
occupied bed kicks the first sleeping villager out (AbstractBedBlock.txt 93–233). Clustine's
beds set `occupied` for players only (`region/sleep.rs` 137, 223, 283).

### 3.5 A profession, from start to loss

Reserve (`AcquirePoi`, a ticket taken) → walk (`GoToPotentialJobSite`, 1200 ticks, released at
the timeout; `YieldJobSite` may pass it on) → arrive within 2 (`AssignProfessionFromJobSite`:
event 14, profession, brain rebuilt) → lose it (block broken and found by `ValidateNearbyPoi`;
`PoiCompetitorScan`; unreachable for 1200 ticks) → at xp 0 and level 1, unemployed again
(`ResetProfession`). The player's classic test: place a lectern by an unemployed villager in the
morning and watch it become a librarian within a minute; break it and watch it lose the job.

---------------------------------------------------------------------------------------

## 4. The social life

### 4.1 Gossip (n7k GossipContainer.txt and its inner classes; Villager.txt)

- **The container**: a `HashMap<UUID, EntityGossips>`, each an `Object2IntOpenHashMap<GossipType>`
  of positive values. `add` (214–241): `s = old + v`, kept at `max(type.max, old)` above the
  type's max; values below 2 removed. `decay` (49–72): each value less its type's daily decay,
  removed below 2. `getReputation(uuid, pred)` (184–200): Σ value × weight.
- **Transfer** (83–183, 388–412): the sender's entries listed (order: two hash maps, one keyed
  by an enum with identity hash: differs between runs), cumulative `|value × weight|`; **10 ×
  `nextInt(total)`** of the receiver's own random, binary-searched, into an identity set (repeats
  lost); each picked arrives as `value − decayPerTransfer` if at least 2, merged by `max`.
- **`GossipType`, not dumped; Inferred values** (weight, max, decay a day, decay a transfer):
  major_negative (−5, 100, 10, 10); minor_negative (−1, 200, 20, 20); minor_positive (1, 25, 1,
  5); major_positive (5, 20, 0, 20); trading (1, 25, 2, 20). So major_positive never decays and
  is never passed on.
- **Saved** as `Gossips`: a list of `{Target (UUID ints), Type, Value}` (GossipEntry.txt 73–95).
- **`Villager.gossip(level, other, t)`** (1709–1767): returns while either villager is within
  1200 of its `lastGossipTime` (not saved: 0 after a load); `n = transferFrom(other, own random,
  10)`; both stamped; `spawnGolemIfNeeded(t, 5)`; if trading and n > 0, `updateSpecialPrices`.
- **`maybeDecayGossip`** (1768–1798): first stamps, then every 24000 game ticks decays.
- **Reputation events** (`onReputationEventFrom`, 1871–1938), about the actor's UUID:
  `ZOMBIE_VILLAGER_CURED` major_positive +20 and minor_positive +25; `TRADE` trading +2;
  `VILLAGER_HURT` minor_negative +25 (from `setLastHurtByMob`, 1093–1126, any living attacker;
  entity event 13 for a player); `VILLAGER_KILLED` major_negative +25 (witnesses). Then, for the
  trading player, `updateSpecialPrices`. `GOLEM_KILLED` is raised nowhere in the dumps.
- **Witnesses** (`die` 1127–1151 → `tellWitnessesThatIWasMurdered` 1168–1204): every villager in
  the dying one's `nearest_visible_living_entities` (16 on each axis, line of sight judged now
  from the dying villager), before `releaseAllPois` and the body's death.

### 4.2 `TradeWithVillager`, `SocializeAtBell`, `InteractWith`, `SetLookAndInteract`, `ShowTradesToPlayer`

- **`TradeWithVillager`** (Behavior 60/60; 5–228): with a visible villager as
  `interaction_target`; start: lock both villagers' gaze and walk (`BehaviorUtils.
  lockGazeAndWalkToEachOther`, 9–147: **the partner's look and walk targets written**), the
  partner's requested items not its own. Tick within √5: lock again, `gossip`, then throw half a
  stack of food (excess food, and the partner wants more or this is a farmer), half the wheat
  above 32 (a farmer), half a stack of a wanted item. `BehaviorUtils.throwItem` (148–214): an
  item from the eye − 0.3 towards the partner at 0.3, with the item's own construction draws.
- **`SocializeAtBell`** (one-shot; 53–99): level `nextInt(100) == 0`, within 4 of the meeting
  point, a visible villager within √32 → interaction, look and walk (0.3) targets.
- **`InteractWith`** (one-shot; 83–143), **`SetLookAndInteract`** (53–86): own memories only, no
  draws.
- **`ShowTradesToPlayer(400, 1600)`** (Behavior; level `nextInt(1201)`; 31–362): a player within
  √17 as interaction target; while the player's main-hand item matches an offer's cost, the
  villager holds that offer's result, cycling every 40 ticks (an equipment change sent to
  clients).

### 4.3 Food and breeding

- `foodLevel` (a byte, saved), the inventory's food points (`villager_food` nutrition: bread 4,
  carrot, potato and beetroot 1; Clustine has them); `hungry` < 12; `wantsMoreFood` points < 12;
  `hasExcessFood` ≥ 24; `eatAndDigestFood` eats until not hungry then `foodLevel −= 12`;
  `canBreed` = food + points ≥ 12, not sleeping, age 0 (1228–1665).
- **`VillagerMakeLove`** (Behavior 350/350; 25–384): both `canBreed` and visible; start: lock
  both, entity event 18 to partner then self, `birth = t + 275 + own nextInt(50)`; tick within
  √5: before birth own `nextInt(35) == 0` → event 12 to both; at `t == birth` both digest, then
  `takeVacantBed` (a `home` within 48 the parent can reach: a path per candidate) → none: event
  13 to both; else `breed` (both ages 6000, `getBreedOffspring`: own `nextDouble`, built in
  Clustine; child age −24000 at the parent; event 12) and the child's `home` is the bed, or the
  bed released if no child. **Inferred**: only the villager whose `InteractWith` chose the
  partner runs it.

### 4.4 Golem summoning

Finding 17. `golemSpawnConditionsMet` (1989–2002): `last_slept` within 24000. `GolemSensor`
(rate 200; 34–70): an iron golem in `nearest_living_entities` → `golem_detected_recently` for
599. `SpawnUtil.trySpawnMob` (SpawnUtil.txt 9–160), level draws: for each of 10 attempts,
`nextInt(17) − 8` twice (`Mth.randomBetweenInclusive`, n5 Mth.txt 1932–1944); from y + 6 down to
y − 6 the first spot `LEGACY_IRON_GOLEM` accepts (SpawnUtil$Strategy.txt 55–137: the block below
solid or powder snow and none of glass, panes, leaves, ice, cobweb, cactus, conduit, TNT,
glowstone, beacon, sea lantern, tinted glass; above air or liquid); `EntityType.create(
MOB_SUMMONED)` with the golem's finalize draws; `checkSpawnObstruction` (IronGolem.txt 670–733);
add and play its ambient sound. Not player-created, so it would defend against players (finding
30).

### 4.5 Panic, bells, hiding, raids, celebrating

- **`VillagerHostilesSensor`** (rate 20; 11–112): the nearest visible of drowned, husk, vex,
  zombie, zombie villager (8), vindicator, zoglin (10), evoker, illusioner, ravager (12),
  pillager (15). Built.
- **`RingBell`** (one-shot, pre-raid 0; 39–86): level `nextFloat() > 0.95`, within 3 of the
  meeting point, a bell there → `BellBlock.attemptToRing`.
- **`BellBlock`** (n9 BellBlock.txt): rung by redstone's rising edge (47–79), a projectile with
  an owner (80–104), a player's use on a proper face (105–166), a villager; `attemptToRing`
  (223–277): `be.onHit(direction)`, the bell sound (2.0, 1.0), game event `BLOCK_CHANGE`. What the
  ring does to entities is `BellBlockEntity`'s (finding 19). Clustine rings bells (R) and holds the
  raider glow as a place (`region/block_entities.rs` 498–520).
- **`LocateHidingPlace(r, speed, close)`** (68–141): a `home` within `close + 1` (`find`), else a
  random `home` within r (`getRandom` with the **own** random), else the `home` memory; set
  `hiding_place` and walk there; erases path, look, breed and interaction targets.
- **`CelebrateVillagersSurvivedRaid(600, 600)`** (14–154): a won raid at its position and open
  sky; each tick own `nextInt(100) == 0` → celebrate sound; own `nextInt(200) == 0` with no
  blocks above → a firework of one random colour (own `nextInt(16)`) and flight `nextInt(3)`,
  whose own draws follow (n9 FireworkRocketEntity.txt 35–118).
- **`GiveGiftToHero(100)`** (Behavior 100/100; 21–349; in work, meet and idle, each its own
  cooldown): counts its 600 down only on ticks a hero is the nearest visible player; walks within
  5, throws after 20 ticks a gift from `gameplay/hero_of_the_village/<profession>_gift` (baby,
  unemployed and nitwit tables apart) by the loot context's draws; then `600 + level
  nextInt(6001)`. Clustine does not compile these tables (`tools/datagen/src/loot.rs` 330–341).
- **`thunderHit`** (1522–1557): a witch by conversion (not on peaceful), releasing all points.

---------------------------------------------------------------------------------------

## 5. Trading

### 5.1 State and opening (AbstractVillager.txt, Villager.txt, WanderingTrader.txt)

- `tradingPlayer` (transient), `offers` (null until generated), `villagerXp`,
  `lastRestockGameTime`, `numberOfRestocksToday` (saved as `Xp`, `LastRestock`, `RestocksToday`),
  `lastRestockCheckDay` (not saved). `Offers` saved when not null as `{Recipes: [...]}` (n7k
  MerchantOffers.txt 99–110; the igloo's cleric confirms `buy`, `sell`, `uses`, `maxUses`,
  `rewardExp`; **Inferred** also `buyB`, `specialPrice`, `demand`, `priceMultiplier`, `xp`).
  A change of profession clears the offers (975–995).
- `getOffers()` generates lazily on its first call (113–144), from a click or from a zombie's
  conversion of the villager (Zombie.txt 1320–1369 copies `getOffers()`).
- `mobInteract` (K 2.5): with offers, `startTrading` (352–368): `updateSpecialPrices`,
  `setTradingPlayer`, `openTradingScreen(player, displayName, level)` (**Inferred**
  `Merchant.openTradingScreen`: open a `MerchantMenu`, then `sendMerchantOffers(id, offers,
  level, xp, showProgressBar, canRestock)`). The trader: `openTradingScreen(player, name, 1)`,
  no special prices, no progress bar, no restock.
- A trade ends at the menu's close (`setTradingPlayer(null)` → `stopTrading`), a teleport, death,
  or no offers. A villager's menu stays valid within the player's interaction range + 4 of its
  box (AbstractVillager.txt 575–592), and `LookAndFollowTradingPlayerSink` (core 3) walks it to
  within 2; the trader's `TradeWithPlayerGoal` ends the trade beyond 4 blocks, in water, hurt or
  off the ground (TradeWithPlayerGoal.txt 15–64).

### 5.2 Drawing offers

`updateTrades` (1666–1707) adds the current level's trade set (`profession.getTrades(level)`;
none for none and nitwit), then `updateSpecialPrices` if trading. The trader adds buying (2 of
6), uncommon (2 of 16), common (5 of 78). `addOffersFromTradeSet` (387–534): finding 20. The
data:

- `trade_set/<profession>/level_N.json`: `{amount, random_sequence, trades: "#<profession>/level_N"}`
  (codec n7k TradeSet.txt 84–114; `allow_duplicates` false everywhere).
- `tags/villager_trade/...`: ordered, `#common_smith/level_N` nested first for the armorer,
  toolsmith and weaponsmith. Sizes per level: armorer 5, 4, 5, 2, 2; butcher 4, 3, 2, 1, 1;
  cartographer 2, 8, 3, 16, 2; cleric 2, 2, 2, 3, 2; farmer 5, 3, 2, 2, 2; fisherman 4, 3, 2,
  1, 6; fletcher 3, 2, 2, 2, 3; leatherworker 3, 3, 2, 2, 2; librarian 3, 3, 3, 4, 2; mason 2,
  2, 7, 33, 2; shepherd 5, 37, 21, 22, 1; toolsmith 5, 2, 5, 3, 1; weaponsmith 3, 2, 1, 2, 1.
- `villager_trade/<profession>/<level>/*.json` (391): `wants {id, count, components?}`,
  `additional_wants`, `gives {id, count}`, `max_uses` (**Inferred** default 4), `xp` (**Inferred**
  default 1), `reputation_discount` (the price multiplier: 0.05 in 338, 0.2 in 53),
  `given_item_modifier`, `merchant_predicate` (27, `entity_properties` by villager type),
  `double_trade_price_enchantments` (4).
- **`VillagerTrade.getOffer`, not dumped; Inferred**: the predicate; the result with its
  modifiers in order (an empty result → null); the hidden `additional_trade_cost` the enchant
  functions wrote (`enchant_randomly`: `2 + nextInt(5 + 10·level) + 3·level`,
  EnchantRandomlyFunction.txt 45–156; `enchant_with_levels`: the levels, 55–98) added to the
  wanted count, doubled for `#double_trade_price`, clamped; the offer. **The order of the wanted
  count's draw against the modifiers needs the class.**
- The `trade_rebalance` pack (104 files) is experimental, behind a feature flag: out of scope.

### 5.3 A trade

`MerchantResultSlot.onTake` (**Inferred**): the active offer's `take(a, b)` or `take(b, a)` →
`notifyTrade` (AbstractVillager.txt 153–177: uses + 1, the ambient timer reset, `rewardTradeXp`
(finding 21), the advancement), the statistic, the payment slots set (each `updateSellItem`).
The next tick the `TRADE` event: trading +2 gossip, entity event 14, `updateSpecialPrices`,
which sends the offers again. `notifyTradeUpdated` (186–215) plays `villager_yes` or
`villager_no` (two own draws) when the ambient timer allows, so not straight after a trade.

### 5.4 Prices

Finding 23. Worked: base 4, demand 3, multiplier 0.2, trading reputation 25: `4 + 2 − 5`, clamped
to 1. Saturated trading gossip is reputation 25; a cure gives the curer 20 × 5 + 25 = 125.
**Villager does not reset special prices when a trade stops** in 26.3: an offer keeps the last
trader's price until the next `updateSpecialPrices` (saved so).

### 5.5 Restock and demand (Villager.txt 430–625)

`shouldRestock`: a gap of more than 12000 ticks since the last restock, or a new day of the
`OVERWORLD_DAY` timeline since the last check, resets the count (`catchUpDemand` first: for each
missed restock of the two a day, uses reset and demand updated); then `allowedToRestock`
(none yet today, or fewer than 2 and 2400 ticks since the last) and `needsToRestock` (an offer
used). `restock`: demand updated and uses reset on every offer, prices and offers sent to a
trading player, `lastRestock = t`, count + 1. **Inferred** `updateDemand`: `demand += uses −
(maxUses − uses)`.

### 5.6 The menu and the packets (all **Inferred** but the dumped methods named)

- **Packets**: clientbound `merchant_offers` (53): container id, the offers (var-int count; each
  `ItemCost` A (item, count, an exact component predicate), the result stack, an optional
  `ItemCost` B, **out of stock** as a boolean, uses, max uses, xp, special price, the multiplier
  (float), demand), level, xp, show progress, can restock. Serverbound `select_trade` (52): an
  index → `handleSelectTrade` (n6 2568–2610): `stillValid`, `setSelectionHint`, `tryMoveItems`.
  `open_screen` with menu type 19 and the display name (`entity.minecraft.villager.<profession>`).
  **Unverified (Pumpkin)** agrees on the order and misnames the out-of-stock flag.
- **`MerchantMenu`**: slots 0 and 1 payment, 2 the result, 3–29 the inventory, 30–38 the hotbar.
  Shift-click of the result moves it to 3–38 reversed and, by `AbstractContainerMenu`'s repeat
  (built, `menus.rs` 429–473), trades again until payment, stock or room runs out; the
  inventory's shift-click never fills payment. `tryMoveItems(i)` puts both payments back, then
  fills slot 0 for cost A and slot 1 for cost B from slots 3–38 in order. Close: payments back to
  the inventory (dropped if dead or gone), `setTradingPlayer(null)`.
- **`MerchantContainer.updateSellItem`**: the offer for (a, b), else for (b, a), not out of
  stock → the result `assemble()`d; `notifyTradeUpdated`. `getRecipeFor`'s quirk: finding 24.

### 5.7 Components and the rest trades need

Finding 25. Clustine's stack has 15 component kinds (`clustine-world/src/components.rs` 24–137)
with codecs, hashes and the store's form (N9a): `enchantments`, `potion_contents`,
`suspicious_stew_effects` among them. Missing: `stored_enchantments`, `dyed_color`, `map_id`
(and **Inferred** `map_decorations`), the exact predicate's codec (the water bottle wanted by the
trader), `EnchantmentHelper.enchantItem`/`selectEnchantment` (dumped, n7), `DyedItemColor.
applyDyes` (not dumped), the tags `#tradeable`, `#double_trade_price`, `#on_traded_equipment`
(enchantments), `#tradeable` (potions), `#on_*_maps` (structures). **Explorer maps** need
structure location with references claimed over a radius of 100 chunks, map ids and map saved
data, which nothing in Clustine keeps; without them the map offer is discarded and another drawn,
as the game does where no structure is found.

---------------------------------------------------------------------------------------

## 6. Zombie villagers: the cure and the conversion

- **Starting the cure** (ZombieVillager.txt 277–387): a golden apple on a zombie villager with
  weakness: consumed; `startConverting(uuid, 3600 + own nextInt(2401))`: weakness removed,
  strength for that time at `min(difficulty − 1, 0)`, entity event 16 (the client's cure sound).
  Without weakness: `CONSUME`, nothing. Weakness exists since N9 (splash potions), so the cure is
  reachable now (`zombie_villager.rs` 146–157 still refuses).
- **The countdown** (244–276, 461–554): each tick `time −= progress`: 1, and on own
  `nextFloat() < 0.01` a scan of the 8 × 8 × 8 cube (by truncation, not floor) for up to 14 blocks
  of `#speeds_up_zombie_villager_curing`, each own `nextFloat() < 0.3` adding 1.
- **The end** (439–453, lambda 737–852): `convertTo(VILLAGER)` keeping equipment; data
  finalised and set; gossips, offers and xp copied; `finalizeSpawn(CONVERSION)` (level draws);
  `refreshBrain` (nine own draws); **the starter looked up anywhere in the level by UUID**: the
  advancement and `ZOMBIE_VILLAGER_CURED` gossip about them; nausea 200; level event 1027.
- **A zombie's kill** (Zombie.txt 859–900, 422–436, 1320–1369): on normal (own `nextBoolean`) and
  hard, the villager becomes a zombie villager with its data, gossips, offers (generated now if
  none) and xp; level event 1026. **On easy, Clustine's difficulty, never.**
- Saved: `ConversionTime`, `ConversionPlayer`, `Gossips`, `Offers`, `Xp` (70–208); a spawned zombie
  villager draws a random profession (224–243). Clustine's record keeps `conversion` and `xp`, not
  offers or gossips (`zombie_family.rs` 107–134).

---------------------------------------------------------------------------------------

## 7. Raids, omens and raiders

### 7.1 `Raids` and the start (n7k Raids.txt, Raid.txt)

- `Raids` is the level's saved data `raids` (449–465): raids by id, `next_id` (the first raid's
  id is 2), `tick`. `Raids.tick` (105–153) runs between the fluid ticks and the chunk source
  (ServerLevel.txt 552–560): the rule `raids` false stops each raid; a stopped raid is removed.
- **`createOrExtendRaid(player, pos)`** (172–291): not a spectator, the rule, the attribute
  `gameplay/can_start_raid` (false in the Nether: n7d EnvironmentAttributes.txt 424–431); the
  centre is the mean of the occupied `#village` points within 64, or `pos`; `getRaidAt(centre)` or
  a new raid (its `numGroups` by difficulty: easy 3); unstarted or below omen 5: `absorbRaidOmen`
  (`raidOmenLevel += amp + 1`, at most 5; Raid.txt 400–437).
- **The omens** (finding 28; n7k MobEffects.txt 446–495): bad omen (from the ominous bottle,
  120000 ticks, amplifier 0 to 4; built), raid omen (600 ticks, particle `raid_omen`), Hero of the
  Village. `MobEffectInstance.tickServer` (n9 456–502) asks `shouldApplyEffectTickThisTick`, then
  `applyEffectTick`, and ends the effect on false. **Inferred**: bad omen every tick, for a
  non-spectator player in a village, not peaceful, with no raid there at omen 5: raid omen with the
  same amplifier, the position stored, bad omen removed; raid omen at duration 1 with a stored
  position: `createOrExtendRaid`, the position cleared.

### 7.2 The raid's tick (Raid.txt 452–877)

Stopped: nothing. Ongoing: `active = hasChunkAt(centre)` (an unloaded centre pauses all, the bar
hidden); peaceful stops; not a village → the centre moved to the nearest village section in the
5 × 5 × 5 cube (879–903), still none → LOSS if a wave spawned else stop, **and the tick goes on**;
`ticksActive++`, at 48000 stop (neither win nor loss); `alive` = members of every wave;
between waves with a countdown: search a spawn position (cooldown % 5, or the position no longer
entity-ticking: `findRandomSpawnPos(8)`), update players at 300 and every 20, count down, the
bar's progress `(300 − cooldown)/300`; a cleared wave restarts the countdown and returns; every
20 active ticks players and raiders updated, the name with "Raiders Remaining: N" at 1 or 2; the
spawn loop (`shouldSpawnGroup`: cooldown 0, waves left or the bonus, nobody alive), `findRandomSpawnPos
(20)` without a position, more than 5 failures stop; victory after 40 post-raid ticks: Hero of
the Village (48000, amplifier omen − 1) to each hero found by UUID anywhere. Won or lost:
celebrate 600 ticks, then stop.

- **`findRandomSpawnPos(attempts)`** (1666–1825), the raid's random: `f = 0.22·(cooldown/20) −
  0.24`; `angle = nextFloat()·2π`; per attempt `angle + π·k/8`, `x = cx + floor(cos·32·f) +
  nextInt(3)·floor(f)`, z likewise (`Mth` table trigonometry); the surface y; refused when 96 off
  in y, in a village during the first 160 ticks of the countdown, without `hasChunksAt(±10)`, not
  entity-ticking, or not ravager-placeable (nor snow under air).
- **`spawnGroup`** (1212–1375): per type in order vindicator, evoker, pillager, witch, ravager:
  `default + bonus` (the bonus by the raid's random: on easy `nextInt(2)` for vindicators and
  pillagers, then `nextInt(n + 1)`); each `EntityType.create(EVENT)`; the first that can lead
  gets the ominous banner (1906–1927); `joinRaid` (1377–1440: health added, raid, wave, can join;
  placed at the position + (0.5, 1, 0.5), **`finalizeSpawn(EVENT)` after the raid is set** (so
  no vindicator's axe, no 0.06 leader draw: N9's `SpawnReason` has no `Event`, `mobs/types.rs`
  1185–1200), `applyRaidBuffs`, added); a ravager gets a rider at waves 5 and 7 (not on easy).
- **`updateRaiders`** (991–1102): a member removed when gone, in another dimension, 112 or more
  from the centre, not found after 600 ticks, or idle (`noActionTime > 2400`, raised by 2 a tick)
  outside a village for 30 passes. Health subtracted, the bar updated.
- **The bar**: `ServerBossEvent` (not dumped; **Inferred** packets: add when visible, remove,
  progress and name on a change); its players are those alive whose `getRaidAt` is this raid
  (2140–2157). `Raider.hurtServer` updates it before the damage lands (Raider.txt 559–572).
- **The horn** (1104–1210): the raid's `nextLong` seed; to each player within 64 horizontally of
  the spawn point, or in the bar, a sound packet 13 blocks towards it at volume 64.
- **Ordering**: the raider sets are `HashSet`s, the heroes a `HashSet<UUID>`, the raids an
  open hash map: a port iterates by key and names it.

### 7.3 Raiders (Raider.txt and its goals; N9 built the types)

- **Goals**: 1 `ObtainRaidLeaderBannerGoal` (an ominous banner item within the follow range,
  path, pick up); 3 `PathfindToRaidGoal` (not in a village: walk to the centre, recruit raiders
  within 16 every 20 own ticks); 4 `RaiderMoveThroughVillageGoal(1.05, 1)` (`PoiManager.getRandom(
  HOME, not visited, 48, own random)`); 5 `RaiderCelebration` (on a loss: own `nextInt(50)`
  celebrate sound and `nextInt(25)` jump). Clustine holds these slots with `Idle`
  (`illagers.rs` 194–200).
- **`aiStep`** (103–170): with no raid and able to join, every 20 game ticks `getRaidAt` and
  join the current wave (patrols and outpost pillagers included; a natural witch not); a target
  that is a player or a golem resets idle time. **`die`**: a leader removed, a player killer
  made a hero, removed from the raid; **no bad omen** in 26.3 (the captain's bottle is loot,
  built). Not despawned and persistent while in a raid.
- **Types**: the pillager's `applyRaidBuffs` (Pillager.txt 440–498: own `nextFloat() <=
  enchantOdds`, a crossbow enchanted from `raid/pillager_post_wave_3` or `_5` by
  `EnchantmentHelper.enchantItemFromProvider`) and its want of a banner; the vindicator's axe
  (332–379), `VindicatorBreakDoorGoal` (raid; own `nextInt(5)`; doors only on normal and hard)
  and `setCanOpenDoors(isRaided)` every tick; the witch's healer target (`NearestHealableRaiderTargetGoal`:
  own `nextBoolean`, then the raid); `RaiderOpenDoorGoal`. The enchant odds are 0 below omen 2, so
  on a first raid nothing is enchanted.
- **The ravager** (Ravager.txt, 989 lines, dumped): not built; easy's third wave and bonus wave
  bring one.

### 7.4 What the villagers read of a raid

`getRaidAt`, `isActive`, `isOver`, `isVictory`, `isLoss`, `isStopped`, `hasFirstWaveSpawned`,
`isBetweenWaves` (section 1.3, 4.5): a raid's status must be answerable wherever a villager
within 96 of its centre is held.

---------------------------------------------------------------------------------------

## 8. The golem, cats, the siege, the trader's spawner and other village goals

### 8.1 The iron golem's village parts (IronGolem.txt; built but for these)

- **`DefendVillageTargetGoal`** (target slot 1; 9–131): no draws; the villagers and players in
  the golem's box grown by 10, 8, 10 by combat conditions at 64 with line of sight; the **last**
  pair with reputation ≤ −100 sets `potentialTarget`, which is **never cleared** (a faithful port
  keeps targeting that player while they are neither creative nor spectator); a player-built
  golem refuses players (`canAttack`, 262–283). Then `NeutralMob` anger (own `UniformInt[400,
  780]`), built in `types/neutral.rs`. Never fires in Clustine's creative world.
- **`GolemRandomStrollInVillageGoal`** (B 2.13): level `nextFloat` twice and up to two level
  `nextInt(size)` over the village sections and their occupied points within 8, or one over
  villagers within 32 that want a golem (`last_slept` and no golem detected). Clustine draws the
  floats and strolls anywhere (`iron_golem.rs` 255–276).
- **`MoveBackToVillageGoal`**: `isVillage` first (before the interval draw), then towards
  `findSectionClosestToVillage(2)`. Clustine gives no position (`iron_golem.rs` 262–266, 288).
- **`OfferFlowerGoal`**: built; reads no points of interest.

### 8.2 Cats (CatSpawner.txt 13–216)

Every 1200 ticks from the first tick (not saved): a random player (level `nextInt(players)`);
level `8 + nextInt(24)` and `nextBoolean` for x and z; at the player's y; chunks ±10 loaded; the
cat's placement; then **in a village** (`isCloseToVillage(pos, 2)`): more than 4 occupied homes
within 48 and fewer than 5 cats in a 48 × 8 × 48 box; **else in a swamp hut**
(`#cats_spawn_in`): no cat within 16, 8, 16, persistent. The cat is finalised at (0, 0, 0)
before it is placed. Village cats are strays: they never lie on beds or chests (those goals need
a tame cat). The hut branch needs no points of interest (Clustine keeps the huts' starts,
`region/natural_spawning.rs` 447; `structures_at` reads the position's chunk only, where the
game reads the chunk's references).

### 8.3 The siege (n9 VillageSiege.txt 28–330)

Not saved. At the overworld clock's `roll_village_siege` marker (18000) level `nextInt(10) == 0`
makes tonight a siege; it then tries every tick until dawn, at the first non-spectator player of
the level's list standing in a village outside `#without_zombie_sieges`: up to 10 angles (level
`nextFloat`), 32 blocks off; then 20 zombies (`EVENT`), one every third tick, each at a position
found by 10 tries of level `nextInt(16) − 8` twice on the surface, in a village, passing the
monster rules (level `nextInt(32)` and the light test's draw); finalised at (0, 0, 0) before
`snapTo` with a level `nextFloat` yaw. No difficulty test but peaceful's removal. 10% of nights,
only for a player in a village.

### 8.4 The wandering trader's spawner (WanderingTraderSpawner.txt 25–353)

Its own unseeded random; `spawnDelay` and `spawnChance` are **server-wide saved data**
(`WanderingTraderData`, not dumped; n9 MinecraftServer.txt 625–629): every 1200 ticks the delay
less 1200; at 0, delay 24000, chance raised by 25 (at most 75), own `nextInt(100) > old chance`
returns; a random player (level), own `nextInt(10) != 0` returns; a `meeting` point within 48
(the first found) or the player; 10 tries of own `nextInt(96) − 48` twice on the heightmap; every block
from the spot to the spot + (1, 2, 1) without collision (`hasEnoughSpace`); not in `#without_wandering_trader_spawns`; the trader
(`EVENT`), two llamas within 4 leashed to it, despawn delay 48000, wander target, home within 16;
success resets the chance to 25, no player resets it too. Clustine has the trader's goals and
despawn; no llama is ever leashed to a trader (`trader_llama.rs` 7, 66).

### 8.5 The custom spawners' order

`ServerChunkCache.tickChunks` (794–928), after the random ticks, while `spawn_mobs`:
phantoms, patrols, cats, the siege, the trader (n9 MinecraftServer.txt 613–630; overworld only).
Clustine runs phantoms and patrols from per-world schedule chains in every region, the holder of
the chosen player acting (`region/custom_spawners.rs` 1–27, 84–176): the template for cats and
the trader. The trader's chance is not a schedule: it depends on outcomes and needs one writer.

### 8.6 Other mobs' village goals

- **Zombies' `MoveThroughVillageGoal(1.0, true, 4, canBreakDoors)`** (MoveThroughVillageGoal.txt
  55–401; zombies, husks, zombie villagers; not the drowned, **Inferred**): at night within 6
  sections of a village, **no interval draw**: `LandRandomPos.getPos(15, 7, scorer)` where the
  scorer is a point-of-interest `find` per candidate, then up to two paths and a door scan; a
  visited list of 15. Expensive at night near a village. Clustine's is a placeholder that draws
  nothing (`zombie_family.rs` 350–369).
- **Foxes**: `FoxStrollThroughVillageGoal(32, 200)` (own interval draw, then the village test),
  `SeekShelterGoal` refused in a village (`fox.rs` 861, 1042–1062).
- **Bees**: `#bee_home` within 20 by points of interest; Clustine scans blocks with the store's
  intended order (`bee.rs` 820–858).
- **Patrols**: `PatrolSpawner` refuses within 2 sections of a village; Clustine's stand-in is the
  kept village structures (`region/structures.rs` 47–66), to be replaced.

---------------------------------------------------------------------------------------

## 9. The draws, in the game's order

| Moment | Stream (game) | Draws |
|---|---|---|
| a villager's brain built (each load three times; growing up; profession change; cure) | own | `nextInt(20)` × 7, `nextInt(40)`, `nextInt(200)` |
| each villager tick, calm | level | `VillagerPanicTrigger` `nextInt(1)`; `SetRaidStatus` `nextInt(20)` |
| each villager tick, end | own | `nextInt(100)` (unless `NoAI`) |
| pre-raid and raid, each tick | level | `ResetRaidStatus` `nextInt(20)`; `RingBell` `nextFloat` |
| a gate or `RunOne` start | list | one `nextFloat` per child |
| a `Behavior` start | level | `nextInt(max + 1 − min)` |
| `AcquirePoi` (×3) | level | `nextInt(20)` at the first call; `nextInt(20)` per scan; `nextInt(40)` per due retry (every match within 48); `nextInt(40)` per new failed target |
| `WorkAtPoi` | level | `nextInt(2)` per try after 300 ticks |
| `StrollToPoiList`, `HarvestFarmland`, `UseBonemeal` | level | `nextInt(size)`; `nextInt(size)` per pick; `nextInt(count)` per crop |
| random positions (strolls, panic, `GoToClosestVillage`, `SetWalkTargetFromBlockMemory`) | own | `LandRandomPos` 30; `DefaultRandomPos` 20–30; up to 1000 calls |
| `SetClosestHomeAsWalkTarget`, a baby's bed sensor | level | `nextInt(20)` |
| `PlayTagWithOtherKids` | level, own | `nextInt(10)`; flight 10 × 30 own |
| `JumpOnBed` | level | `nextInt(4)` |
| `InsideBrownianWalk` | JDK global | `Collections.shuffle` of 27 |
| `LocateHidingPlace`, `RaiderMoveThroughVillageGoal` | own | `getRandom`'s shuffle, n − 1 `nextInt` |
| `MoveToSkySeeingSpot` | own | 3 a try, up to 10 |
| gossip | receiver's own | 10 × `nextInt(total)` when the sender has gossip |
| `TradeWithVillager` throws | item's own | each thrown item's construction |
| `SocializeAtBell` | level | `nextInt(100)` |
| `VillagerMakeLove` | own | `nextInt(50)` at start; `nextInt(35)` a tick; `nextDouble` at birth |
| golem summoned | level | per attempt `nextInt(17)` × 2; the golem's finalize |
| `GiveGiftToHero` | level, loot | `nextInt(6001)` at stop; the gift table's draws |
| celebration | own | `nextInt(100)`, `nextInt(200)`, colour `nextInt(16)`, flight `nextInt(3)`, the rocket's |
| a trade | own | `nextInt(4)` (orb); the sound's two when it plays; a level-up's offers |
| offers drawn | the set's world-wide random sequence | `nextInt(size)` per pick; the modifiers' and the counts' |
| the cure | own | start `nextInt(2401)`; each tick `nextFloat`, at 1% a `nextFloat` per accelerator; the villager's finalize and nine sensor delays |
| a raid | raid's own | bar UUID (two `nextLong`); per search `nextFloat` then `nextInt(3)` × 2 per attempt; per type the bonus; the horn's `nextLong` |
| a raider joining at spawn | own, level | finalize (`EVENT`), `applyRaidBuffs` `nextFloat` |
| cat spawner | level | `nextInt(players)`, `nextInt(24)`, `nextBoolean`, `nextInt(24)`, `nextBoolean`; the cat's |
| siege | level | `nextInt(10)` at 18000; `nextFloat` per angle; per position `nextInt(16)` × 2, `nextInt(32)`, the light draw; yaw `nextFloat` |
| trader spawner | own, level | `nextInt(100)`; level `nextInt(players)`; `nextInt(10)`; `nextInt(96)` × 2 per try; llamas `nextInt(8)` × 2 per try |

In Clustine every stream but the raid's and the spawners' is the entity's (A28 section 4, its
N7 amendment), in this order; the raid's is a stream keyed by the raid; the spawners' are
schedule streams as the phantoms' are; the trade sets' world-wide sequences become the villager's
stream aside (question 16.4). The JDK's global random and HotSpot's `Math.pow` in the
`ShufflingList` weights (B 7.1.1.5) are named as before.

---------------------------------------------------------------------------------------

## 10. What clients are sent

| What | Data or packet |
|---|---|
| a villager's level and profession | synched 19 (`VILLAGER_DATA`, built) |
| unhappy | synched 18 (built) |
| sleeping villager | pose (sleeping), `sleeping_pos` (living 14, optional block position), the bed's `occupied` block change |
| a claim, a profession, a trade | entity event 14 (happy) |
| breeding | 18 (love), 12 (hearts), 13 (angry: no bed) |
| hurt by a player | 13 |
| a raid's splash | 42 |
| show trades | the villager's main-hand equipment |
| farming | block changes, `crop_planted`, level events 1500 (composter), 1505 (bone meal) |
| the work sound | the profession's work sound (`villager_work_<profession>`) |
| gifts, food throws | item entities |
| a celebration | firework rockets |
| the menu | `open_screen` (type 19), `merchant_offers` after it and after each trade, level-up and restock while open, container contents and slots as N6 |
| a trade's xp | an experience orb (built) |
| the cure | entity event 16, level event 1027; the conversion 1026 |
| a golem summoned | the golem, its ambient sound |
| a raid | `boss_event` (add, progress, name, remove) per player; the horn as a sound packet per player; raiders' celebrating (synched 16, built); raid omen's particles; Hero of the Village as an effect |
| a bell | the bell's block event and sound (built); raiders glowing (shared flags bit 6) |

---------------------------------------------------------------------------------------

## 11. What crosses a line

Copies reach one chunk across a line and carry entities without their minds (A25 section 3,
`clustine-world/src/entity.rs` 384–386: "nothing reads another mob's mind across a line"); a
mob in a copy is not written by `Local::SetMemories` (`mobs/tick.rs` 192–194); what one entity
does to another across a line is an entity effect (A28 section 7); regions that follow players
merge at once for work at their line (A28 section 7: within 4 blocks, arrivals in the last 600
ticks, interactions, targets; `region/arrivals.rs` 95–158), and pinned lines use effects. Against
that:

- **Points of interest far beyond the copies.** A villager's queries reach 72 blocks on an axis
  (finding 2), the village tracker 96 in three dimensions, raids 64 to 115, the trader 48, cats
  48. A region needs the points of interest (and their occupancy) of a margin of about five
  chunks around the chunks where its villagers tick, and the village level of every section
  within six sections of them. In the game a point of interest in a chunk that is not loaded is
  read from disk; in Clustine those chunks are held by another region or by nobody (stored).
- **A claim across a line.** A take decrements a ticket in the chunk that holds the point of
  interest. If that chunk is another region's, two villagers in two regions can both see space
  and both claim within the effect's delay. Nothing in the game undoes a double claim of a bed or
  a lectern but `PoiCompetitorScan` (job sites only, within 16, reading the other villager's
  memories, which copies do not carry).
- **A release from anywhere** (death, timeouts, the 100-block limit, another dimension) is a
  ticket increment in a chunk that may be another region's or nobody's.
- **Writes into another villager**: gossip's `lastGossipTime` and its golem summoning;
  `lockGazeAndWalkToEachOther` (trading, love) every tick; `YieldJobSite` and `PoiCompetitorScan`;
  the partner's food and age at birth; `golem_detected_recently` into up to five; murder gossip
  into witnesses within 16. The partners are within √5 (gossip, love), 8 (`InteractWith`), 10
  (golems) or 16 (yield, competitor, witnesses).
- **Reads of another villager**: `PlayTagWithOtherKids` (`interaction_target`), `YieldJobSite`
  and `PoiCompetitorScan` (`job_site`, `potential_job_site`, xp, profession), `wantsToSpawnGolem`
  (`last_slept`, `golem_detected_recently`), `canBreed` (food, inventory, age, sleeping), the
  golem's stroll and defence (villagers wanting a golem, villagers' gossip about players). In a
  copy these must be in the type's state (copied), not the mind (not copied).
- **A bed or a job site across the line**: `startSleeping` sets `occupied` on a bed whose foot
  may be across; the farmer's harvest and planting and the composter are block changes within 2
  of the villager (effects where across, A25).
- **A trade with a player of another region**: a click on a villager seen in a copy is
  `EntityUse::Interact` to its holder (`clustine-world/src/effects.rs` 144–152); a trade needs the
  player's inventory and the villager's offers in one click. N6's rule for containers across a
  line: opens nothing (the owner's choice of 2026-10-18); regions that follow players merge for
  the interaction. A villager that crosses a line mid-trade, or a player handed over mid-trade:
  the menu closes and the payments go back, as every N6 menu does.
- **A raid spanning regions**: its record (world state; the game's `raids` saved data), the
  lookups of `getRaidAt` within 96 from villagers, raiders, vindicators and players with bad omen
  in other regions, members in several regions (their hurts move the bar, their deaths make
  heroes, recruits add health), spawn positions up to 104 blocks out in another region's land
  (made there by `send_made`), the bar's players and the horn's (players of any region within
  range), Hero of the Village to heroes anywhere by UUID, a raid id unique in the world.
- **The custom spawners' state**: the cat's countdown and the siege's are not saved (schedule
  chains, as phantoms'); the trader's chance and delay are saved world state with outcomes; the
  siege's state lives where its player is; the trader may be spawned 48 blocks from a meeting
  point 48 blocks from a player of another region.
- **World-wide random sequences** for offers (finding 20) and, with maps, the map id counter
  and the structures' claims.
- **A golem summoned 8 blocks off**, a gift or food thrown, a firework, a child born at its
  parent: entities made in another region's land go by `send_made` (built).

---------------------------------------------------------------------------------------

## 12. What a player meets within minutes

- **A village that lives by day and night**: in the morning villagers walk to their job sites
  and work (the librarian at its lectern, the farmer harvesting and replanting, the composter
  filling), gather at the bell after noon, mill about, trade food and gossip, babies chase each
  other and jump on beds; at dusk they go to their beds and sleep, and wake at dawn. Today they
  wander all day and stroll at night.
- **Professions**: an unemployed villager takes a lectern the player places (green particles,
  the librarian's robe), loses it when it is broken; nitwits never work.
- **Trading**: right-click a villager with a profession and the merchant screen opens with its
  offers; trading levels it up (the badge, more offers, Regeneration particles), uses run out and
  restock at the job site, prices fall as the player trades (gossip) and with Hero of the Village.
  Today every villager shakes its head. Librarians' enchanted books and the armourer's enchanted
  gear need enchanting; the cartographer's maps (question 16.9).
- **Breeding**: with food and free beds, babies appear; golems appear in villages with enough
  sleeping villagers.
- **A zombie at night**: villagers panic and flee, a golem appears when three panicking
  villagers have slept recently; the zombie kills villagers (no conversion on easy). A bell rung
  by the player sends them to hide.
- **A cure**: splash weakness and a golden apple on a zombie villager; it shakes and becomes a
  villager with low prices for the curer.
- **A raid**: a patrol captain's ominous bottle, drunk in a village, gives the raid bar, the
  horn, three waves of pillagers, vindicators and a ravager, villagers ringing the bell and
  hiding, the golem fighting, Hero of the Village and gifts thrown by villagers afterwards.
- **Cats** appear in villages; a **wandering trader** with two llamas arrives every few days.
- **What they will not see, named or a question**: the golem defending against them (creative),
  a siege most nights (10% of nights, in a village), explorer maps (if left out), the
  experimental trade rebalance.

---------------------------------------------------------------------------------------

## 13. What Clustine has to reuse, and what is missing

Format numbers now: the store's `FORMAT_VERSION` 16 (`clustine-format/src/lib.rs` 58), the
worker's `STATE_FORMAT` 21 (`services/worker/src/lib.rs` 524).

**Reusable as is**

- **The brain engine** (`mobs/brain.rs`, 1131 lines): `Provider`, `ActivityData`, the start pass
  by priority, bucket and insertion (`brain.rs` 1087–1122), `Gate` and `RunOne` with the game's
  shuffle (387–428), `Timed`, `Shot`, `KeptShot`, `DoNothing`, `TriggerIf`, `set_active`,
  `set_active_if_possible`, `requested` (an activity asked during the tick, 950–963), `remake`
  (`refreshBrain`, 677), `stop_all`; behaviour state in the record numbered by construction.
- **All 115 memory kinds and all 26 activities** with the HashMap bucket order
  (`brain/memory.rs` 19–135, `brain/activity.rs` 12–89); expiry as an absolute `lived` tick
  (553–713); countdowns; the sensed/kept split; entity references by key resolved through the
  region's entities, copies and faces (`mobs/ai/sensing.rs` 419–429).
- **Sensors**: nearest living, players, hurt by, villager hostiles, nearest items; the inert
  sensors keep the bed's, babies', secondary points' and golem's rates and draws
  (`brain/sensors.rs` 602–680).
- **Behaviours**: walking and looking sinks, strolls, `SetWalkTargetFromLookTarget`, the looks,
  doors, `GoToWantedItem`, `SetWalkTargetAwayFrom`, `Swim`, and the villager's own
  `VillagerPanicTrigger`, `VillagerCalmDown`, `UpdateActivityFromSchedule`, `InsideBrownianWalk`,
  `GoToClosestVillage` (`behaviours/villagers.rs`, 294 lines).
- **The villager's body** (`villager.rs`, 811 lines): `VillagerState` (data, inventory, food,
  xp, last restock, last gossip decay, restocks today), the schedule table, refusal, food
  pick-up, breeding's offspring, the panic, rest and idle packages.
- **The golem, the trader, the cat, the illagers**: every goal slot, with placeholders where
  N10 fills (section 8); `RaiderState { wave, can_join_raid, celebrating }` (`illagers.rs`
  49–62); patrols and the ominous banner (`mobs/ai/patrol.rs` 45–80); `HoldGroundAttackGoal`.
- **Effects**: bad omen from the bottle (`region/eating.rs` 210–236), Hero of the Village and
  raid omen ids, weakness from splashes; the effect packets.
- **Menus** (N6): the region's rules and the edge's synchroniser (`menus.rs` 1747 lines,
  `region/menu_play.rs` 1063, edge `menus.rs` 473), with a mob-backed menu (`MenuKind::Mount`,
  `Did::OpenInventory`, `open_mount_menu`); `put_back`, quick move's repeat.
- **Components** with wire codecs and hashes (N9a), `enchantments`, `potion_contents`,
  `suspicious_stew_effects`.
- **Lines**: entity effects and their plumbing, `send_made`, arrivals, the merge for work and
  targets at a line; faces; the published clock and players' table (`StoreReply::Clock`, once a
  second); the published counts (`StoreReply::Counts`); stateless schedule chains
  (`custom_spawners.rs` 84–103).
- **Data**: the point-of-interest tags, bed and villager block and item tags, the village biome
  tags, villager types and professions (`clustine-data/src/villagers.rs`), the loot machinery,
  the `BOSS_EVENT`, `MERCHANT_OFFERS`, `SELECT_TRADE` packet ids.

**Needs extending**

- `VillagerState`: offers, gossips, the trading player, and what other villagers read across a
  line (job site, potential job site, last slept, golem detected, interaction target, food).
- The villager's provider: work, meet, play, pre-raid, raid and hide packages, the core's point
  of interest and trading behaviours; the four inert sensors made real; the panic trigger's golem
  call.
- `EntityUse` (13 variants, eight exhaustive matches: `region/entity_effects.rs`,
  `mob_bodies.rs`, `cushions.rs`, `knots.rs`, `hanging.rs`, `minecarts.rs`, `stands.rs`,
  `boats.rs`): villager-to-villager writes, raid joins and reports.
- `target_at_line` for a brain's `attack_target` (now only `body.combat.target`).
- `PlayerFace`: the player's effects (bad omen, Hero of the Village) for villagers and raids in
  other regions.
- `MenuKind::Merchant`, `PlayerInput::SelectTrade`, an offers event; the protocol's packets.
- `LootModifier` and the datagen's loot compiler: the trades' functions, the gift tables,
  `cat_morning_gift`; the `add` provider; the `villager_trade` and `trade_set` compiler.
- The chunk: a points-of-interest part (flag bit 4 is free, `docs/world-format.md` 140–219).
- `SpawnReason::Event`; `illagers::finalize_spawn` excluding it.
- Player effects ticking (`player_effects.rs` 232–257), `raid_omen_position` with the player.
- The zombie villager: offers and gossips kept, the cure.
- `custom_spawners.rs`: cats, the siege, the trader.
- Composter recipes for the farmer; beds' `occupied` for villagers.

**Missing entirely**

- The point-of-interest index, its tickets, the village tracker, and their life across regions.
- Gossip and reputation; trading's offers, prices, restock and level; the merchant menu.
- `EnchantmentHelper.enchantItem`; `stored_enchantments`, `dyed_color`; maps.
- Raids: the record, its tick, its world-wide publication, the bar, the horn, the omens' ticks,
  the raiders' raid goals, the ravager.
- `BellBlockEntity`'s effect on entities; `SpawnUtil`.
- Any world-level saved state but the clock and the players.

---------------------------------------------------------------------------------------

## 14. Sizes

Estimated in lines of Rust with the tests of each part, by the readers, against N7j (the brain
engine and four brained animals, about 26,000) and N9 (about 70,600 in 48 commits; a delegated
part 1,500 to 4,700 with its tests):

| Part | Lines |
|---|---|
| Points of interest: types, records by section from blocks, tickets in the chunk, queries in a fixed order, take and release, the village tracker, the store's part, the margin's reads, effects across a line, `isVillage` replacing the stand-ins | 3,000–4,500 |
| The villager's brain: the nine packages, switching, sensors, the movement and rest behaviours, sleeping, the schedule's extremes | 3,500–5,000 |
| Work and professions: `AcquirePoi`, validation, yield, competitor, assign and reset, the farmer's three behaviours, composting, restock's caller | 2,000–3,000 |
| Social: gossip, reputation, trade between villagers, love, food, golem summoning with `SpawnUtil`, bells (`BellBlockEntity`), hiding, celebrating, gifts with their tables, murder | 2,500–3,500 |
| Trading data: the compiler of `villager_trade` and `trade_set`, tags, `getOffer`, the modifiers, `EnchantmentHelper`, `stored_enchantments`, `dyed_color`, `add` | 2,500–3,500 |
| Trading: offers, prices, demand, restock, level-ups, the trader's trades and goal, the zombie villager's offers and cure | 1,500–2,000 |
| The merchant menu (region and edge) and its packets | 1,200–1,700 |
| Raids: the record and its tick, waves, joining, removal, bar state, save, draws | 1,500–2,000 |
| Raids across regions: ids, the published table, effects, hand-overs, the bar and horn per player | 1,500–2,500 |
| Raiders' raid goals, buffs, `SpawnReason::Event`, the omens' ticks | 1,300–2,000 |
| The ravager | 1,000–1,500 |
| The golem's village goals, cats, the siege, the trader's spawner with its world state, zombies' and foxes' village goals | 1,800–2,500 |
| Tests from the specification by someone who did not write the code | 5,000–7,000 |
| Comparisons with the official server | 2,000–3,000 |
| **All, without explorer maps** | **about 30,000–45,000** |
| Explorer maps (structure location, map data, map items, `map_item_data`) | 2,000–3,500 more |

---------------------------------------------------------------------------------------

## 15. Not dumped (the lead dumps them)

**The village and points of interest** (`javap -p -c -constants`):
`net.minecraft.world.entity.ai.village.poi.PoiManager$DistanceTracker`,
`net.minecraft.server.level.SectionTracker`, `net.minecraft.world.entity.ai.village.poi.PoiManager$Occupancy`,
`net.minecraft.world.entity.ai.village.poi.PoiSection$Packed`,
`net.minecraft.world.entity.ai.village.poi.PoiRecord$Packed`,
`net.minecraft.world.level.chunk.storage.SectionStorage`,
`net.minecraft.world.level.chunk.storage.SerializableChunkData` (the consistency check at load),
`net.minecraft.util.thread.BlockableEventLoop` (whether `server.execute` in the world tick runs
at once), `net.minecraft.core.Cursor3D` (the order of `SectionPos.cube`),
`net.minecraft.core.Holder$Reference` (its hash), `net.minecraft.core.GlobalPos`.

**The social life**: `net.minecraft.world.entity.ai.gossip.GossipType` (weights, maxima, decay),
`net.minecraft.world.entity.ai.village.ReputationEventType`,
`net.minecraft.world.entity.ReputationEventHandler`,
`net.minecraft.world.level.block.entity.BellBlockEntity` (the ring's reach, `heard_bell_time`,
the glow), `net.minecraft.world.food.VillagerFood`,
`net.minecraft.world.level.storage.loot.BuiltInLootTables` (the gift tables' paths),
`net.minecraft.world.level.border.WorldBorder`.

**Trading**: `net.minecraft.world.item.trading.MerchantOffer`,
`net.minecraft.world.item.trading.ItemCost`, `net.minecraft.world.item.trading.VillagerTrade`,
`net.minecraft.world.item.trading.Merchant`, `net.minecraft.world.inventory.MerchantMenu`,
`net.minecraft.world.inventory.MerchantContainer`, `net.minecraft.world.inventory.MerchantResultSlot`,
`net.minecraft.network.protocol.game.ClientboundMerchantOffersPacket`,
`net.minecraft.network.protocol.game.ServerboundSelectTradePacket`,
`net.minecraft.world.RandomSequences`, `net.minecraft.world.RandomSequence`,
`net.minecraft.core.component.DataComponentExactPredicate`,
`net.minecraft.world.item.component.DyedItemColor`,
`net.minecraft.world.level.storage.loot.providers.number.ints.ContextIntProvider` and
`ContextIntProviders`, `net.minecraft.world.level.storage.loot.parameters.LootContextParamSets`;
if maps are in: `net.minecraft.world.item.MapItem`,
`net.minecraft.world.level.saveddata.maps.MapItemSavedData`.

**Raids**: `net.minecraft.world.entity.raid.Raid$RaiderType`, `Raid$RaidStatus`, `Raid$1`,
`net.minecraft.world.entity.raid.Raids$RaidWithId`, `net.minecraft.world.effect.BadOmenMobEffect`,
`net.minecraft.world.effect.RaidOmenMobEffect`, `net.minecraft.server.level.ServerBossEvent`,
`net.minecraft.world.BossEvent` with `$BossBarColor` and `$BossBarOverlay`,
`net.minecraft.network.protocol.game.ClientboundBossEventPacket` (and its operation classes),
`net.minecraft.world.level.saveddata.SavedData`, `net.minecraft.world.level.storage.SavedDataStorage`.

**The spawners**: `net.minecraft.world.level.saveddata.WanderingTraderData`,
`net.minecraft.world.level.levelgen.structure.StructureStart`.

**The schedule**: `net.minecraft.util.KeyframeTrack`, `net.minecraft.util.KeyframeTrackSampler`
(read for n7d-brief from another dump; to confirm the wrap).

**With `javap -v`** (the bootstrap arguments `-c` does not print): `VillagerGoalPackages`
(the raid package's predicates #8 and #9, the idle `InteractWith` predicates, `validateBedPoi`),
`Villager` (the golem summon's `forEach`), `InsideBrownianWalk`, `NearestItemSensor`.

Dumped and not read for this brief: `Ravager.txt` (989 lines; its own reading with the step that
builds it), `EnchantmentHelper` (n7; read with trading's data part), `ExplorationMapFunction`.

---------------------------------------------------------------------------------------

## 16. Open questions for the plan, each with options and a recommendation

1. **Where points of interest live.** (a) In each chunk: the records derived from its blocks at
   every load and change, the **tickets** kept as the chunk's state (a part under flag bit 4),
   changed by commits, copied, merged, split and lent with the chunk, restored by replay; the
   store keeps, from the commits it already sees, a world index by section (points, types,
   occupancy, the village sources) and publishes it to each region for a margin of six sections
   round its land, as it publishes counts; (b) a separate world service owning all tickets, every
   take a request; (c) tickets not kept at all, derived from the villagers' memories at load.
   **Recommended: (a)**: tickets are block-attached in the game, the chunk is Clustine's unit of
   everything durable, and the publication is the counts' machinery (P9 section 9.2); (c) cannot
   see owners in chunks nobody holds; (b) puts a round trip inside a behaviour.
2. **A claim across a line.** (a) A region takes only points of interest in land it holds;
   others are read (to path to, to count a village) but count as having no space, named; releases
   across a line as effects, applied by the holder or by the store into land nobody holds (as
   A28's arrivals into stored chunks); (b) takes across a line as effects with the claimant's
   memory set at once (a double claim possible within the delay, named); (c) regions that follow
   players merge for a villager working at its line (A28 section 7), with (a) at pinned lines.
   **Recommended: (a) with (c)**: a village is usually in one region where regions follow
   players (a region is the chunks its players see), the merge covers its edge, and (a) can never
   double-claim; the seam at a pinned line through a village is named.
3. **What copies of villagers carry.** (a) The fields other villagers read (job site, potential
   job site, last slept, golem detected, interaction target, xp, profession, food points, age,
   sleeping, gossips for the golem's defence) moved into the type's state, which copies carry;
   (b) the mind in copies. **Recommended: (a)**: small and explicit; the mind stays the region's.
4. **Writes into another villager across a line.** (a) A villager entity effect
   (`EntityUse::Villager(VillagerUse)`, one variant with its kinds so that the eight exhaustive
   matches change once): gossip stamp, gaze and walk, yield, competitor's erase, love's digest and
   age, golem detected, witness gossip; the writer acts as if it took; (b) only within a region,
   named; (c) as (a) with the merge of question 2. **Recommended: (c)**; trading and love at a
   pinned line are rare, gossip and witnesses ordinary.
5. **The trade sets' world-wide random sequences.** (a) Each villager's offers from its stream
   aside (key, `lived`, a mark per set), named; (b) a world stream per set kept by the store,
   advanced by requests. **Recommended: (a)**, judged by distribution, as every other draw.
6. **Trading across a line.** (a) As N6: a click on a villager seen in a copy opens nothing
   (named; regions that follow players merge for it); the menu closes when either crosses;
   (b) the player handed to the villager's region for the trade (as getting on a vehicle across a
   line, n7j). **Recommended: (a)**; (b) only if the owner finds (a) a seam in play.
7. **Raids as world state.** (a) A raid is a record kept with the chunk of its centre (moving with
   its holder through merges, splits and moves; its id from the region's entity ids, unique in
   the world), ticked by that region; the store publishes a table of active raids (id, centre,
   status, waves, between waves, omen level) to every region, as the players' table, for
   `getRaidAt`; members elsewhere report hurts, deaths, heroes and recruits by entity effects to
   the raid's chunk; (b) as (a) and regions that follow players merge everything within 112 blocks
   of an active raid's centre into its region; (c) the raid kept by the store. **Recommended:
   (b)**: a raid comes to a player, so where regions follow players it is in one region and the
   bar, the horn and the counts are exact; (a) is needed anyway for lookups from outside and for
   pinned lines. Not delegated: the record's hand-overs and the effects' ordering.
8. **Enchanting.** (a) Port `EnchantmentHelper.enchantItem`/`selectEnchantment` (dumped) with the
   enchantment registry's weights, costs and exclusions, used by trades, `applyRaidBuffs` and mob
   equipment (`making.rs` 162–179); (b) unenchanted results, named. **Recommended: (a)**: the
   librarian's book is the trade a player seeks within minutes.
9. **Explorer maps.** (a) In N10, a part of their own: structure location with claims, map ids
   and saved data, map items and their packet; (b) left out: the cartographer's map offers are
   discarded and redrawn, as where no structure is found, named. **Recommended: (b)**: maps are an
   item system of their own (a player's own empty map too), best a later phase; the cartographer
   still trades. The owner's judgement is wanted: a player levelling a cartographer meets it.
10. **The ravager.** (a) Built in N10 with its goals, roar, stun and riders; (b) raids on easy
    without it, named. **Recommended: (a)**: wave 3 and the bonus wave bring one on easy.
11. **The siege.** (a) Built (small), each region considering its own players in id order with
    the roll from a per-world schedule stream; (b) named. **Recommended: (a)**, late in the
    phase; it is rare (section 8.3) and can slip.
12. **The trader's spawner state.** (a) A small world record (delay, chance) in the store's
    table, written by the region that held the chosen player at a firing (one writer per firing),
    published with the clock; (b) derived: the chance assumed as if every day failed, named.
    **Recommended: (a)**; the same record can hold raids' `next_id` if question 7 wants it.
13. **The gossip constants and other undumped classes.** Dump first (section 15), and build
    nothing on the **Inferred** tables until confirmed. **Recommended**: the lead's first act.
14. **What a load does to a brain.** The game rebuilds the brain at every load (27 draws, all
    transient state lost); Clustine carries the whole brain through crossings, merges and splits,
    and N7j left `fresh_load` unbuilt. (a) A fresh load (a chunk loaded from the store, not a
    take-in) keeps only the memories with codecs and draws the sensors again, as the game; (b)
    everything kept, named. **Recommended: (a)** for villagers, where the difference shows (the
    raid's default activity, gossip's cooldown, `AcquirePoi`'s caches), since the store already
    tells a fresh load apart (A28's N7 amendment, `fresh_at`).
15. **Order within a point-of-interest query.** Fix one (chunk x then z, section y, type id,
    position), or keep insertion order per section. **Recommended**: the fixed order, so that a
    query is a function of the state; named as a distribution.
16. **The tick's length.** Villages are many villagers with path searches (finding 12, section
    1.7). **Recommended**: measure a village of 30 villagers at dusk (beds claimed by path
    search) and a raid's three waves, in memory and on the real disk, as N7 measured mobs.
17. **The bell's ring at a line** (32 to 48 blocks): an effect per entity in another region
    (`heard_bell_time`, the glow) or the region's own only. **Recommended**: an effect to the
    region holding each entity in range through the copies it sees, and the merge where regions
    follow players.
18. **Comparisons with the official server.** Exact where nothing draws: the merchant screen
    packet by packet (`/summon villager` with fixed `Offers`, a trade, a shift-click repeat, a
    select, a close), restock's timing given `LastRestock`, the boss bar's packets for a raid
    whose waves are summoned (no command starts one: bad omen by `/effect` in a village with
    claimed beds), the schedule's switch ticks (`/time set` and the activity seen by the pose and
    work sound), a profession's acquisition by a lectern (event 14 and the data), the cure's end
    with a fixed `ConversionTime`; by distributions: offers per level over many villagers, gossip
    spread, the raid's waves. **Recommended**: these, the dumps first.
19. **Order of work.** Section 17.

---------------------------------------------------------------------------------------

## 17. A proposed division

Steps, each one verified commit to `main`, pushed, as P9 section 12 does; the formats rise step
by step. The lead's first, in this order:

| Step | What | Whose |
|---|---|---|
| N10a | **Dumps and data**: section 15's classes; `PoiTypes` by hand; the `villager_trade`, `trade_set` and trade tags compiler; the gift tables and `cat_morning_gift`; the enchantment, potion and structure tags; `GossipType` and the raid's tables once dumped | the lead |
| N10b | **Points of interest** (question 1 and 2): the chunk's part and its tickets (store format 17), records from blocks at load and change, queries in the fixed order, take and release in the region's land, releases across a line as effects and into stored land by the store, the store's index and its publication for the margin, the village tracker, `is_village` and `is_close_to_village` with every stand-in moved onto them (`structures.rs` 47–66, the golem, foxes, zombies, bees if wanted) | the lead |
| N10c | **The contracts**: `VillagerState` grown (offers, gossips, the fields of question 3), `EntityUse::Villager(VillagerUse)` with the eight matches, the merge for villagers working at a line, `SpawnReason::Event`, `MenuKind::Merchant`, `PlayerInput::SelectTrade` and the offers event, the protocol's `merchant_offers`, `select_trade`, `ItemCost` and `boss_event`, `PlayerFace` with effects, the new modules declared with their signatures (each part's stubs), `STATE_FORMAT` 22 | the lead |
| N10d | **Raids as world state** (question 7): the record in its centre's chunk, ids, the published table, the effects from members, hand-overs, merges for a raid, the bar's and the horn's per-player events | the lead |
| N10e | **The player effects' ticks**: bad omen and raid omen, `raid_omen_position` with the player and its hand-over | the lead |
| N10f | **The fresh load** (question 14) and the streams' tests changed by the second level draw (finding 9) | the lead |

Then, in parallel, each in a worktree of its own, no file touched by two parts (paths under
`crates/clustine-sim/src/`):

| Part | What | Files |
|---|---|---|
| V1 | The villager's packages, switching, sensors (bed, babies, secondary points, golem), movement, rest, sleep and play behaviours | `mobs/types/villager.rs`, `mobs/brain/behaviours/villagers.rs`, `mobs/brain/behaviours/rest.rs`, `mobs/brain/sensors_villager.rs`, `mobs/types/villager_n10_tests.rs` |
| V2 | Job sites, professions and work: `AcquirePoi`, validation, yield, competitor, assign, reset, `GoToPotentialJobSite`, `SetWalkTargetFromBlockMemory`, the strolls to points, the farmer, composter recipes | `mobs/brain/behaviours/work.rs`, `region/composting.rs`, `mobs/brain/behaviours/work_n10_tests.rs` |
| V3 | The social life: gossip, reputation, trade between villagers, love, golem summoning and `SpawnUtil`, bells' effect on entities, hiding, celebrating, gifts, witnesses | `mobs/types/gossip.rs`, `mobs/brain/behaviours/social_villagers.rs`, `region/spawn_util.rs`, `region/bells.rs`, `mobs/types/social_n10_tests.rs` |
| T1 | Trading's data and evaluation: offers, `getOffer`, the modifiers, `EnchantmentHelper`, `stored_enchantments`, `dyed_color`, the `add` provider | `trading/offers.rs`, `trading/evaluate.rs`, `items/enchanting.rs`, `trading/trading_n10_tests.rs`, the component files the lead names |
| T2 | Trading's rules and menu: starting and stopping, the merchant menu's slots and clicks, prices, demand, restock, level-ups, the trader's trades and goal, the zombie villager's offers and cure | `trading/merchant.rs`, `menus/merchant.rs`, `mobs/types/wandering_trader.rs`, `mobs/types/zombie_villager.rs`, `trading/merchant_n10_tests.rs` |
| E1 | The edge: the merchant window and offers, select trade, the boss bar per player, the horn | `services/edge/src/merchant.rs`, `services/edge/src/boss_bars.rs` |
| R1 | Raids' rules on N10d's frame: the tick, waves, spawn search, joining, removal, celebrations; the raiders' raid goals, buffs and doors | `region/raids.rs`, `mobs/ai/raider_goals.rs`, `mobs/types/pillager.rs`, `vindicator.rs`, `witch.rs`, `illagers.rs`, `region/raids_n10_tests.rs` |
| R2 | The ravager | `mobs/types/ravager.rs`, `mobs/types/ravager_n10_tests.rs` |
| G1 | The golem's village goals, cats, the siege, the trader's spawner, zombies' and foxes' village goals | `mobs/types/iron_golem.rs`, `region/custom_spawners.rs`, `mobs/ai/village_goals.rs`, `mobs/types/fox.rs`, `mobs/types/zombie_family.rs`, `region/village_n10_tests.rs` |
| X1 | Tests from the specification (this brief and the plan) by someone who did not write the code: part A after N10b to N10e, part B after the parts | `region/tests/n10_*.rs`, `bin/clustine/tests/n10_*.rs` |
| X2 | The comparisons of question 18, for the lead to run | `bin/clustine/tests/merchant_oracle.rs`, `raids_oracle.rs`, `villagers_oracle.rs` |

Order: N10a to N10f in turn; T1 may begin after N10a, E1 after N10c, V1, V2, V3, T2 and G1 after
N10c (V2, V3 and G1 need N10b), R1 and R2 after N10d and N10e, X2 after N10c. **Not delegated**,
where ordering mistakes hide: the tickets' life through merges, splits, lends, restores and
releases across a line (N10b); the villager effects' holder side and the merge (N10c); the raid
record's hand-overs, its effects and the bar's players (N10d); the omens' hand-over with the
player (N10e); the fresh load (N10f); `fanout.rs` and the protocol's packets.
