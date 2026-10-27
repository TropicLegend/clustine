# N10: villagers' brain, trading, points of interest, raids

Draft of 2026-10-27, for an independent reviewer before anything is built; revised the
same day after the review (section 17 says what changed and where). The game's rules
are those of `n10-brief.md` (cited as V: V 0.12 is finding 12 of its section 0, V 3.1 its
section 3.1), read from the 26.3 jar, as `n10-dumps.md` settles its inferred claims against
the classes dumped since (cited as D10: D10 A.3 is correction 3 of part A's summary, D10 B
§4 the fourth section of part B; where D10 corrects V, D10 holds, and this plan says so where
it matters). Dumps: `/home/claude/reference/n10-javap/` and the earlier `n5-` to `n9-javap/`.
A5, A22, A25, A28 and A29 are the ADRs of those numbers; P9 is `n9-plan.md`. Code is cited as
it is at `92471b0`, paths under `crates/clustine-sim/src/` unless another crate is named.

Where it sits: `n-plan.md` section 4 names N10 "villagers' brain and trading, golems, raids,
as a phase of their own"; the roadmap's next item after N9. What is built: V's opening and
section 13. Points of interest do not exist in Clustine; every rule that reads them has a
stand-in.

What N10 changes that a world keeps: the store's format (a chunk's claims of points of
interest, its raids, the villager's offers and gossips, a player's omen position, the
trader spawner's record), raised in each step that changes the shape of a record a world
has; **worlds made before are refused**, as at every raise before. The worker's
`STATE_FORMAT` (21, `services/worker/src/lib.rs` 524) rises likewise.

---------------------------------------------------------------------------------------

## 0. What is decided, and why

The lead's answers to V's nineteen questions (V 16), with what D10 and the code add.

1. **Where points of interest live: in each chunk, the records derived from the blocks and
   the claims kept.** A record exists where a block's state is of a point-of-interest type
   (V 2.1; derived at every arrival of a chunk in a region's land or copies, and kept up to
   date by every block change through the region's one write path, `Chunk::set`). Only the
   **claims** are state: for each record whose free tickets are below its maximum, the
   position, the type and the free tickets, kept in the chunk as a new part (flag bit 4 of a
   chunk file), changed by commits, carried by copies, merges, splits, lends and hand-overs
   with the chunk, restored by replay. A type that a block change replaces loses its claim
   (V 0.5: removing a record tells nobody). This is the game's own rule at a load (D10 A.3: a
   rebuild keeps each old record, with its tickets, wherever a point of interest still is, and
   only new positions get full tickets), applied always. The store keeps no index of its own;
   the regions report their records, as they report their counts (decision 2's margin).
   *Why*: tickets are attached to blocks in the game; a chunk is Clustine's unit of everything
   durable; a separate service would put a round trip inside a behaviour; and tickets derived
   from villagers' memories cannot see owners in chunks nobody holds.
2. **A claim across a line, and reads beyond the land.** A region **takes** a ticket only of a
   record in its own land; a record in a copy, or known only from the margin, counts as having
   no space (named). It **reads** records (to find a site, to path to it, to count a village,
   to validate a memory) in its land, its copies and a **published margin** (section 3.5): every
   region reports the records of each chunk of its land whose records changed (position, type,
   occupied) by a request of its own, and the store sends each region, by a reply of its own,
   the records of every chunk within five chunks of its land that it neither holds nor copies.
   Five chunks is the reach of the raid's centre search (64 blocks, `floorDiv(64,16)+1`) and of
   the tracker's five sections; the villager's 48-block queries need four. A **release** of a
   record in another region's land is an effect, `Effected::Release { kind }` at the record's
   block, applied by its holder as the game's guarded release (the type still there and the
   free tickets below the maximum: `releaseTicket`), and filed by the store for land nobody
   holds as every effect is (dropped at a store restart, as every effect for land nobody holds
   is, ADR-0025 E3a: a ticket so lost stays claimed; named). Where regions follow players,
   **they merge** for a villager whose site, bed, meeting point or partner is across their line
   (decision 4). **Every write across a line that carries a ticket** (a release, a yield) is
   checked by its holder against the game's conditions at the moment it lands, and where they
   fail the ticket is released, so that no ticket is left with nobody remembering it. *Why*: no double
   claim is possible; a village is in one region where regions follow players (a region is the
   chunks its players see, and a village is seen whole); the seam at a pinned line through a
   village is named.
3. **What copies of villagers carry: the fields others read, in the type's state.**
   `VillagerState` gains a `Shown` part, written at the end of each tick of the villager from
   its brain: `job_site`, `potential_job_site`, `last_slept`, `golem_detected_until`,
   `interaction_target`, `food_points`, `sleeping`, `xp`, and the gossips the golem's defence
   reads (only the summed reputation of each player with a reputation ≤ −100; empty in a
   creative world). The mind is not carried (A25 section 3). *Why*: small and explicit.
4. **Writes into another villager across a line: an entity effect, and the merge.** One
   variant, `EntityUse::Villager(VillagerUse)`, so that the eight exhaustive matches of
   `EntityUse` change once, with the kinds: `Gossiped { at }` (the partner's
   `lastGossipTime`), `Gaze { from, walk }` (`lockGazeAndWalkToEachOther`'s write into the
   partner), `Yield { site, walk }` (`YieldJobSite`), `LoseJobSite { site }`
   (`PoiCompetitorScan`), `Born { at }` (love's digest and age into the partner),
   `GolemDetected { until }`, `Witnessed { murderer }` (`major_negative` +25), `Heard {
   at }` (a bell's `heard_bell_time`), `Glow` (the bell's glowing raider). The writer acts as
   if the write took, except where a ticket rides on it: `Yield`'s holder re-checks the game's
   conditions (the partner alive, adult, without a `potential_job_site`, with no job site or this
   one, of a profession that holds the type) and, where they fail or the partner is gone,
   releases the site (by `Effected::Release` if it is not in its land). Where regions follow
   players, `region/arrivals.rs` adds to the reasons to merge (A28 section 7, its four blocks of
   the line): a villager within four blocks of the line whose `interaction_target`,
   `breed_target`, job site, potential job site, home or meeting point is in another region's
   land. *Why*: gossip and witnesses are ordinary, trading and love at a pinned line rare.
5. **The trade sets' random sequences: the villager's stream aside.** D10 B §11: each set's
   stream is a pure function of the world seed and the set's id, saved world-wide, and it also
   carries every modifier's draws. A cluster cannot share those streams without a round trip
   per offer. Offers are drawn from the villager's stream aside
   (`Random::of_entity_aside(world, key, lived, tick)` with the in-tick count, as every aside
   draw), named; judged by distribution.
6. **Trading across a line.** As N6's containers (the owner's choice of 2026-10-18): a click on
   a villager seen in a copy opens nothing at a pinned line (named); regions that follow
   players merge for the interaction (built, A28 section 7); the menu closes, the payments going
   back, when the villager or the player is handed over, as every N6 menu does.
7. **Raids: a record in a chunk, a published table, effects to the record's chunk.** A raid is
   a record kept as a part of a chunk (flag bit 5), at its making the chunk of its centre, ticked
   by the region that ticks that chunk, moving with the chunk through merges, splits, lends and
   hand-overs and restored by replay; its id is an entity key of the region that made it
   (unique in the world). Every region reports its raids' summaries, **with the record's
   chunk**; the store publishes the table of raids to every region; `getRaidAt` takes the
   nearest of the region's own raids and the table's (its own dropped from the table). Members
   held elsewhere report their hurts, deaths, killers and departures to the record's chunk by
   effects (`Effected::Raid`), and the raid's region is the judge of membership (section 9.3).
   A raid comes to a player, so where regions follow players its village and its members are
   in one region (the ordinary merge distance covers 112 blocks); at a pinned line the effects
   carry it. Not delegated: the record's hand-overs, the effects' ordering, the bar's players
   (N10d).
8. **Enchanting: ported.** `EnchantmentHelper.selectEnchantment` and `enchantItem` (dumped,
   n7), with the registry's weights, costs and exclusions, used by trades, `applyRaidBuffs` and
   mobs' equipment (`mobs/making.rs` 162–179, a stub). The librarian's book is the trade a
   player seeks first.
9. **Explorer maps: left out, named.** D10 C §6: 10 of the cartographer's 31 trades; each has
   `filtered` on `map_id` with `on_fail: discard`, so where no structure is found the game
   itself discards the offer and draws another. Clustine treats every search as failed: the
   cartographer trades, and never offers a map. Maps are an item system of their own (a
   player's own empty map, map data, its packet, structure location with claims over 100
   chunks) and a later phase. The roadmap asks the owner whether they want it sooner.
10. **The ravager: built in N10.** Easy's third wave and the bonus wave bring one (D10 C.4:
    the bonus wave repeats the last wave's defaults; D10 C.2: there is a bonus wave only from
    omen 2).
11. **The siege: built, late in the phase.** One siege a world: the night's roll and the
    player come from a per-world schedule stream over the world's players' table, as the
    patrols' (`region/custom_spawners.rs` 1–21), and the region holding that player runs it.
12. **The trader spawner's state: the delay a schedule, the chance a world record.**
    `WanderingTraderData` (D10 C §5: `spawn_delay` 24000, `spawn_chance` 25). The delay is a
    function of game time on the 1200-tick cadence: a schedule chain, as the phantoms'. Only the
    chance depends on outcomes: the store keeps `(firing, chance)` durably (a new request,
    `StoreRequest::TraderChance`, logged; the later firing wins, so a firing doubled at a
    pinned line converges, named), published with the clock, written by the region that holds
    the player the firing chose. The store's side is the lead's (N10e), with its format raise.
13. **The undumped classes**: dumped and read (D10). Nothing is built on an inferred table.
14. **What a load does to a brain: the live brain kept, named.** The game rebuilds a brain at
    every load (D10 A.6: three builds for an adult, four for a baby, nine own draws each, every
    transient memory and cache lost). Clustine has no fresh load: every take-in keeps the whole
    entity, mind included (`region/entities.rs` 129–143; `fresh_at` is never set; N7j's
    `fresh_load` hooks are unbuilt, `n7j-plan.md` 2.10). Building the store's fresh and
    continued marks (`n7-plan.md` 9.1 and 11) is ordering work on the commit thread with a
    design of its own, and nothing a player meets within minutes depends on it: the raid's
    default activity, gossip's 1200-tick cooldown and `AcquirePoi`'s retry caches survive an
    unload. Named; a later step may build the marks for every brained mob at once. Fields the
    game does not save (`trading_player`, `last_gossip_time`, `last_restock_check_day`) are kept
    in Clustine's record; `trading_player` is cleared as section 8.2 says.
15. **Order within a point-of-interest query: fixed.** Chunks x fastest then z (D10 A
    confirms the game's chunk order), sections y ascending, then type id ascending, then
    position (y, z, x ascending). The game's order within a section is a hash order (D10 A:
    `Holder$Reference` has identity hashes, records a `HashSet`); a query is so a function of
    the state, judged by distribution.
16. **The tick's length**: measured (section 15): a village of 30 villagers at dusk before
    they have beds, and a raid's three waves, in memory and on the real disk.
17. **The bell's ring at a line**: the bell's region lists the living entities in its box
    grown by 48 from its land and copies (D10 B.1); each in another region's land gets
    `Heard` (within 32 of the centre) and, at the 45th tick, `Glow` (raiders within 48) as an
    entity effect. Entities beyond the copies are not reached (named); where regions follow
    players a village is one region's.
18. **Comparisons with the official server**: V 16.18's list, by part (section 14).
19. **Order of work**: section 13.

---------------------------------------------------------------------------------------

## 1. What a player sees when N10 is done

- **A village that lives by day and night.** In the morning villagers walk to their job sites
  and work (a librarian at its lectern, a farmer harvesting and replanting, a composter
  filling), gather at the bell from 9000, mill about, trade food and gossip; babies chase each
  other and jump on beds; at dusk they go to their beds and sleep, and wake at dawn.
- **Professions**: an unemployed villager takes a lectern the player places (green particles,
  the robe), loses it when it is broken; nitwits never work.
- **Trading**: right-click a villager with a profession: the merchant screen with its offers;
  trading levels it up (the badge, more offers, Regeneration's particles); uses run out and come
  back at the job site twice a day; prices fall with good gossip and with Hero of the Village.
  Librarians sell enchanted books, armourers enchanted gear; cartographers sell no maps (named).
- **Breeding**: with food and free beds, babies appear; golems appear in villages where enough
  villagers slept recently.
- **A zombie at night**: villagers panic and flee; a golem appears when three panicking
  villagers have slept recently. A bell rung by the player sends them to hide.
- **A cure**: splash weakness and a golden apple on a zombie villager; it shakes and becomes a
  villager with low prices for the curer.
- **A raid**: a patrol captain's ominous bottle drunk in a village: raid omen for 30 seconds,
  then the raid bar, the horn, three waves (pillagers, vindicators, a ravager), villagers
  ringing the bell and hiding, the golem fighting, Hero of the Village and gifts thrown by
  villagers afterwards.
- **Cats** in villages; a **wandering trader** with two llamas every few days; a siege on one
  night in ten for a player in a village.

### 1.1 What they do not see, named

The golem defending against them (creative); explorer maps; the experimental trade rebalance;
a claim across a pinned line (section 0.2); a trade with a villager across a pinned line; the
village tracker's residue of former villages (section 3.4); offers from world-wide streams.

---------------------------------------------------------------------------------------

## 2. What D10 changes in V, in one place

| V | D10 | Consequence |
|---|---|---|
| 0.3, 11: the tracker reaches 6 sections | A.1: levels 0..5 by 3-D Chebyshev distance in sections, 7 where never reached; a stored 6 kept where a source was lost | `is_close_to_village(p, 6)` is true only within 5 of a source in Clustine; the residue of 6 is named (3.4) |
| 0.4: a rebuild frees claims | A.3: a rebuild keeps claims where a point of interest still is | decision 1's rule is the game's |
| 0.4: `server.execute` at once (inferred) | A.4: at once for the tick and for packets | records follow blocks in the same tick |
| 2.2: save names inferred | A.5: `{Valid, Records: [{pos, type, free_tickets}]}`, `free_tickets` omitted at 0 | only for the comparisons' templates; Clustine's own format is section 3.2 |
| 0.10: three builds a load | A.6: three for an adult, four for a baby | decision 14 |
| 2.1: `test_instance_block` | A.7: the type is `test_instance`, ids 18 lodestone, 19 test_instance, 20 lightning_rod | the type table (3.1) |
| 0.19, 4.5: the bell's reach | B.1: through block event 1; the list of the box grown by 48 kept 60 ticks; `heard_bell_time` within 32; resonance at the 5th tick with a raider within 32; glow at the 45th within 48, 60 ticks | `region/bells.rs` (V3) |
| 0.17: golem detected into the five | B.2: into every villager in the summoner's box grown by 10 | V3 |
| 5.2: the cost's order | B.3 and TradeCost: predicate, every modifier, then `clamp(count + extra, 0, maxStack)`; only `additional_trade_cost` doubled, for `stored_enchantments` in the trade's list; `max_uses ≥ 1`, `xp ≥ 0`, discount ≥ 0 | T1 |
| 5.6: the wire | B.4: uses, max uses, xp, special price, demand are fixed `int`s, the multiplier a `float`; level and xp var-ints; `ItemCost` item, var-int count, components; the second cost a boolean then the cost | the protocol's packet (N10c) |
| 5.6: `updateSellItem`, `tryMoveItems` | B.5, B.6 | T2 |
| 4.5: gift tables | B.7: `baby_gift`, `unemployed_gift` for none and nitwit; no nitwit table | the datagen (N10a) |
| 5.7: 12 map trades | B.9, C.8: 10, each discarded where no structure is found | decision 9 |
| 0.23: the price's types | B.11: `max(0, floor((float)(base·demand) · mult))`; demand unclamped | T2 |
| 0.28: the omens | C.1: bad omen lingers one tick after raid omen begins; the raid starts about 600 ticks after the tick after | N10e |
| 0.27: easy waves | C.2–C.5: no bonus wave below omen 2; easy's extra is 1 in 4, drawn every wave; the bonus wave repeats the last wave; riders by wave number | R1 |
| 7.2: refused in a village during 160 ticks | C.6: while `cooldown / 20 > 7`, the first 141 ticks; both `nextInt(3)` drawn every attempt | R1 |
| 7.1: the centre round the player | C.7: round the stored `raid_omen_position` | N10e |
| 1.5 `InsideBrownianWalk` | C.9: its third test is the villager's own box; nothing and no draw where its block sees sky | V1 |

---------------------------------------------------------------------------------------

## 3. Points of interest (N10b; the lead's)

### 3.1 Types (`clustine-world/src/poi.rs`, new)

`PoiType`, numbered by the registry's order (n7 PoiTypes bootstrap, D10 A §2.1): the 13 jobs,
`home`, `meeting`, `bee_nest`, `beehive`, `nether_portal`, `lodestone` (18), `test_instance`
(19), `lightning_rod` (20); with `max_tickets` and `valid_range` (V 2.1), and
`poi_type_of(BlockState) -> Option<PoiType>` from a table built by the datagen from the
blocks' states (`clustine-data`, generated: for a bed only `part=head` of the 16 coloured beds;
every lightning rod's weathering and waxing). The tags `acquirable_job_site`, `village`,
`bee_home` are generated already. `is_job(PoiType)` and each profession's held job site come
from `clustine-data/src/villagers.rs`.

### 3.2 Claims in the chunk (store format 17; the lead's)

`Chunk` gains `claims: Vec<Claim>`, ascending by position, `Claim { at: InChunk, kind:
PoiType, free: u8 }`, each with `free < kind.max_tickets()` (a full record is no claim) and
its block of that type. A claim whose block is not of its type is **dropped** by the reader
and logged, never refused: a refused chunk is a hole never generated again
(`clustine-rpc/src/messages.rs` 520–524), and a mismatch can arise without corruption (D10
A.3: the game itself keeps or drops quietly). Part of the chunk's equality and of its copies;
not of what an edge is handed.

- **Chunk file**: flag bit 4, "with claims", the part after the structures: a count (u16) and
  per claim the position packed as the block events pack it, the type u8, the free tickets u8.
- **Log**: commit kind 19, "Commit with claims", the fields of kind 16 and per chunk whose
  claims changed i32 x, i32 z and the chunk's claims whole (as seams and events are carried,
  `docs/world-format.md`, "Commits").
- **The block's change** (`Chunk::set` and the region's set path): where `poi_type_of(old) !=
  poi_type_of(new)`, the claim at that position goes (V 0.4, 0.5). Where the types are equal
  (a bed's `occupied`, a composter's level) nothing changes.
- **Generation**: a structure's points of interest are full; generated chunks have no claims.
- **The store's fold** of a commit into a chunk applies its block changes before its claims
  (and its raids), so that a bed broken and another placed and claimed at one position in one
  tick keeps the new claim. A change of claims clears `with_failure` as a block change does,
  and `remakes` (`services/worldstore/src/chunks.rs` 1203) keeps a remade chunk's claims only
  where they still match, as it keeps entities.
- **Copies**: `StoreReply::CopyUpdate` gains `claims: Option<Vec<Claim>>`, the chunk's claims
  whole where the commit changed them, and a commit that changes only claims (or only raids)
  counts as the chunk's change for `previous` and the gap check, as ADR-0028 made entity
  changes count. ADR-0025 section 3 is amended.

### 3.3 The region's index (`region/pois.rs`, new; the lead's)

`Pois` keeps, per chunk of land and copies, the records by section (type, position, free),
rebuilt from the blocks when a chunk enters the land or copies (a section is scanned only if
its palette holds a state of a point-of-interest type), updated by every set, and the claims
applied over it. Its margin part holds, per published chunk, the records `(position, type,
occupied)`; a margin record's `free` is unknown and it never has space. A chunk the region
copies is read from the copy, never the margin (the copy knows `free`); the store sends no
margin for copied chunks.

Queries, each in the fixed order of decision 15, over land, copies and margin, chunks
`rangeClosed(chunkOf(c), floorDiv(r, 16) + 1)`:

| Function | The game's | Rule |
|---|---|---|
| `in_square(pred, c, r, occ)` | `getInSquare` | `|dx|, |dz| ≤ r` |
| `in_range(pred, c, r, occ)` | `getInRange` | and `distSqr ≤ r²` |
| `find_all`, `find_all_with_type` | | filters of `in_range` |
| `find_all_closest_first_with_type` | | a stable sort by `distSqr`, every filter run on every match |
| `find` | | the first in order |
| `find_closest` | | the least `distSqr`, the first of equals |
| `take(type, pred, c, r)` | `take` | the first with space passing the predicate, **of the land only**; `acquire`, its result ignored |
| `random(pred, c, r, occ, random)` | `getRandom` | shuffle as `Util.toShuffledList` with the caller's stream |
| `release(pos)` | `release` | in land: `releaseTicket` (guarded); else `Effected::Release` |
| `exists(pos, pred)`, `type_at(pos)` | | land, copies, margin |
| `count_in_range` | `getCountInRange` | |

Occupancy: `HasSpace` (`free > 0`; margin records never), `IsOccupied` (`free != max`; a
margin record's `occupied`), `Any`.

### 3.4 The village (`region/pois.rs`)

`sections_to_village(section)`: the least 3-D Chebyshev distance in sections to a section
holding an occupied record of `#village`, from land, copies and margin, when at most 5; else 7
(D10 A.1). `is_close_to_village(pos, r) = r <= 6 && sections_to_village(section) <= r`,
`is_village(pos) = is_close_to_village(pos, 1)`. Computed per tick on demand, cached by section
until a set or a claim change in reach, a chunk entering or leaving the land or the copies, a
copy's claims changing, or a margin publication. **Named**: the game's residue of a stored 6 around
a village that lost its last claim this run is not kept (D10 A.1); "loaded" is land, copies
and margin (D10 A.2: the game reads any section ever read since start, from disk if need be).

Every stand-in moves onto it: `region/structures.rs`'s `village_near` (the patrol spawner,
V 8.6), the golem's two strolls (`iron_golem.rs` 255–288), foxes (`fox.rs` 861, 1042–1062),
zombies' `MoveThroughVillageGoal`, the bee's hive search (`bee.rs` 820–858 onto `#bee_home`
records, its order the fixed one), and `VillageBoundRandomStroll` and `GoToClosestVillage`
(`behaviours/villagers.rs`). **Not before V2**: until villagers claim (V2), no village has a
claimed record, so N10b gives `is_village` and keeps the stand-ins as they are, and the commit
that lands V2 moves them (the lead's, at V2's merge), so that `main` never has patrols spawning
inside generated villages.

### 3.5 The margin's publication (the lead's; the store, the runner and the region)

Not on the counts' messages: the counts are published only once every region has reported
(`worldstore/src/spawn_counts.rs` 81–96) and the runner keeps only the last
(`services/worker/src/lib.rs` 1715), both right for whole counts and wrong for a delta.

- **Report**, `StoreRequest::Village { session, pois: Vec<(ChunkPos, Vec<PoiSeen>)>,
  let_go: Vec<ChunkPos>, raids: Option<Vec<RaidSeen>> }`, `PoiSeen { at: BlockPos, kind:
  PoiType, occupied: bool }`, sent at the end of a tick that changed any of it: each chunk of
  the land whose records or occupancy changed, whole; the chunks the region stopped holding
  (`let_go`, by the region that held them, as the counts' `moved_away`); the raids (section
  9.2). After every opening of the region (a start, a take-over, a merge, a split, a move) the
  first report lists every chunk of its land whole, so that the store learns a pinned
  region's land (which its table does not list, ADR-0025 E3a) from the region itself.
- **The store** keeps, per chunk, the last records and the region and session that reported
  them, and takes a region's word only for chunks it reported last as its own in its current
  session. A chunk let go keeps its records until another region reports it (the chunk
  returned to nobody keeps its last records). Not durable: after the store's restart the
  records of a chunk are unknown until a region holding it reports.
- **Publication**, `StoreReply::Village { whole: bool, pois: Vec<(ChunkPos, Vec<PoiSeen>)>,
  dropped: Vec<ChunkPos>, raids: Vec<RaidSeen> }`, once a game second with the clock, not
  gated on other regions' reports: to each region the chunks within five chunks of its land
  that it neither holds nor copies, as a delta since the last publication to that region's
  current session; **whole** at the first publication to a session and after the region's
  every opening. The raids are always whole.
- **The runner** folds publications in order into one input (a whole one replaces, a delta is
  applied over it); it keeps the folded input across a merge's or a split's new beginning
  only if whole, and the store sends whole after either anyway.
- **The region** keeps its margin across ticks; `Region::restore` starts it empty, and the
  next whole publication fills it.
- **Named**: a margin read is up to a second and a half old; the records of chunks not held
  by any region since the store started are unknown (the game reads them from disk): at the
  edge of a region's land, `IsOccupied`, `Any` and the tracker see no village there until it
  is held.

### 3.6 Merges, splits, lends, restores

Claims are a chunk part: they travel with the chunk, as structures and events do, so a
merge, a split, a lend and a restore by replay are the chunk's own (B4, A25). The index is
derived and rebuilt from the chunk at each of those. A release effect to land nobody holds is
filed by the store and applied when the chunk is next held (the effects' existing filing),
and dropped at the store's restart (decision 2, named). Raids in copies are never read (the
table answers for other regions' raids); a lent chunk's raid ticks in the borrower, which
ticks the chunk.

### 3.7 Tests (the lead's own; X1 adds from the specification)

Records follow sets (a lectern placed and broken; a bed's foot and head; a composter's level
changes no claim); a claim survives a save, a merge, a split, a lend and a restore; a take in
land and a refused take in a copy; a release across a line by effect and into stored land;
the margin published and read, and read again after a merge, a split, a take-over and a
move (each followed by a whole publication); a copy's `IsOccupied` following a take by its
holder; a claim whose block changed type dropped at reading, the chunk kept; a commit
breaking a bed and claiming a new one at one position kept through the store's fold;
`sections_to_village` by distance 0 to 5 and 7; the query order.

---------------------------------------------------------------------------------------

## 4. The contracts (N10c; the lead's)

- **`VillagerState`** (`mobs/types/villager.rs`): `offers: Option<Offers>`, `gossips:
  Gossips`, `trading_player: Option<PlayerId>`, `last_gossip_time`, `last_restock_check_day`
  (the three the game does not save, kept in Clustine's record, decision 14),
  `restocks_today`, `last_restock`, `xp`, and `shown: Shown` (decision 3). The type state
  carries its own encoding's version (`clustine-world/src/entity.rs` 443): the villager's and
  the zombie villager's (which gains `offers` and `gossips`) records rise in their own
  versions, and a store format raise is made only where the entity record's frame changes.
- **`Offers`, `Offer`** (`clustine-world/src/trading.rs`, new): `Offer { cost_a: ItemCost,
  cost_b: Option<ItemCost>, result: ItemStack, uses, max_uses, xp, special_price, demand: i32,
  price_multiplier: f32, reward_exp: bool }` (D10 B §4's fields), `ItemCost { item, count,
  components: ExactPredicate }`.
- **`Gossips`** (`mobs/types/gossip.rs`, new, V3 fills): by target `Uuid` ascending, then type
  ascending (the fixed order of a port of two hash maps, named).
- **`EntityUse::Villager(VillagerUse)`** with the kinds of decision 4, and
  **`EntityUse::Raider(RaiderUse)`** (`Rejected { raid }`, section 9.3); the exhaustive matches
  (`mob_bodies.rs`, `cushions.rs`, `knots.rs`, `minecarts.rs`, `stands.rs`, `boats.rs`, and the
  log's codec `clustine-format/src/effects.rs`, encode and decode) take each once;
  `region/entity_effects.rs` and `region/hanging.rs` have wildcards. The holder sides for
  villagers and raiders (`region/entity_effects.rs`) are the lead's.
- **`Effected::Release { kind }`** (section 3.3, in N10b) and **`Effected::Raid(RaidNews)`**
  (section 9.3, in N10d), with their holder sides; each changes the log's effects codec, and
  `docs/world-format.md`'s "Effects" with it.
- **The merge for villagers** at a line (decision 4, `region/arrivals.rs`).
- **`SpawnReason::Event`**; `illagers::finalize_spawn` and every type's finalize read it as
  the game's `EVENT`.
- **Menus**: `MenuKind::Merchant { villager: EntityId }`, `PlayerInput::SelectTrade { index }`,
  `PlayerEvent::Offers { container, offers, level, xp, show_progress, can_restock }`.
- **The protocol** (`clustine-protocol`): `merchant_offers` (clientbound), `select_trade`
  (serverbound), `ItemCost`'s codec and its optional form, `boss_event` with its six
  operations (D10 C, settled from the dumped operation classes), the menu type 19. The edge's
  side of the merchant and the bar is E1's; the codecs are the lead's.
- **`PlayerFace`** gains the player's effects that villagers and raids read across a line
  (bad omen, raid omen, Hero of the Village, with amplifiers).
- **Stubs and hooks**: each part's new modules declared with their signatures and inert
  bodies, as N9's contract step did, so that parts compile apart; and every place where one
  part's rule is called from a file another part owns gets its call now, to an inert hook in
  the caller's part (section 13 names each): the profession change's brain rebuild, gossip, the
  reputation events and the witnesses on death, the opening of trading from `mobInteract`, the
  cure, the hopper's composter, enchanted mob equipment.
- `STATE_FORMAT` 22.

---------------------------------------------------------------------------------------

## 5. The villager's brain (V1, on N10b and N10c)

The provider of V 1.1 whole: the nine sensors made real (the bed's: `find_all_with_type(home,
48)` and a path to up to four, level `nextInt(20)` for a baby; the babies'; the secondary
points' 9 × 5 × 9 block scan every 40; the golem's), the nine activities with their conditions,
the packages of V 1.4 with `VillagerGoalPackages-v`'s predicates (D10 B §9: the raid's #8
`raidExistsAndNotVictory` returning `isVictory`, #9 `raidExistsAndActive`; idle love's
`Villager::canBreed` and `AgeableMob::canBreed`; `validateBedPoi`), switching (V 1.3: the panic
trigger, `ReactToBell`, `SetRaidStatus` with its level `nextInt(20)` every tick, their exits),
the movement and rest behaviours of V 1.5 (`InsideBrownianWalk` per D10 C.9; `SleepInBed` and
`WakeUp`, beds' `occupied` for villagers, `startSleeping` and `stopSleeping`, and a player's use
of a bed a villager sleeps in waking the villager (V 3.4, AbstractBedBlock 93–233;
`region/sleep.rs` 137 says "occupied" today), play
(`PlayTagWithOtherKids`, `JumpOnBed`), and the order of draws of V 1.6. The JDK's global
random of `InsideBrownianWalk` is drawn from the villager's stream (named, as B 7.1.1.5's
`ShufflingList`). The second level draw (`SetRaidStatus`) changes the villager streams' tests:
the lead changes them in N10c, before V1 begins.

---------------------------------------------------------------------------------------

## 6. Work and professions (V2)

`AcquirePoi` with `JitteredLinearRetry` (V 3.1, its three instances per villager, lost at a
rebuild), `ValidateNearbyPoi`, `YieldJobSite` (a path with the other villager's navigation; the
write by `VillagerUse::Yield` where the other is across), `PoiCompetitorScan` (by
`VillagerUse::LoseJobSite` across), `AssignProfessionFromJobSite` and `ResetProfession` (V 0.13,
event 14, the brain rebuilt by `remake`), `GoToPotentialJobSite`,
`SetWalkTargetFromBlockMemory`, the strolls to points, `WorkAtPoi` (restock's caller, the work
sound), the farmer's `WorkAtComposter`, `HarvestFarmland` and `UseBonemeal`, and the
composter's recipes (`region/composting.rs`; the hopper's composter reads them too).

---------------------------------------------------------------------------------------

## 7. The social life (V3)

Gossip and reputation (V 4.1 with `GossipType`'s constants confirmed, D10 B §1), the reputation
events (`ZOMBIE_VILLAGER_CURED`, `TRADE`, `VILLAGER_HURT`, `VILLAGER_KILLED`; `GOLEM_KILLED`
writes nothing, D10 B.8), `maybeDecayGossip`, witnesses, `TradeWithVillager`,
`SocializeAtBell`, `InteractWith`, `SetLookAndInteract`, `ShowTradesToPlayer`, food,
`VillagerMakeLove` with `takeVacantBed`, golem summoning with `SpawnUtil` (D10 B.2), bells'
effect on entities (D10 B.1, decision 17), `LocateHidingPlace`, `SetHiddenState`, `RingBell`,
`CelebrateVillagersSurvivedRaid`, `GiveGiftToHero` with the gift tables (D10 B.7), murder.

---------------------------------------------------------------------------------------

## 8. Trading

### 8.1 The data and its evaluation (N10a's compiler, T1)

The datagen compiles `villager_trade` (391), `trade_set` (68) and the trade tags (nested,
ordered) into `clustine-data`'s generated trading tables; `TradeCost`'s default count 1
(D10 B, settled); the gift tables and `cat_morning_gift`. T1 evaluates: `VillagerTrade.getOffer`
in D10 B §5's order; the loot functions trades need (`enchant_with_levels`, `enchant_randomly`,
`set_random_dyes`, `set_random_potion`, `set_stew_effect`, `filtered`, the `add` number
provider, `exploration_map` as a failure, decision 9); `EnchantmentHelper` (decision 8); the
components `stored_enchantments` and `dyed_color` (with their codecs, hashes and the store's
form, as N9a's).

### 8.2 The rules and the menu (T2)

Offers drawn lazily (V 5.1), the level-up in the trade (V 0.21), prices (D10 B.11), demand and
restock (V 5.5), `updateSpecialPrices`, the merchant menu's slots, clicks, shift-click's repeat,
`tryMoveItems` and `updateSellItem` (D10 B.5, B.6), the close's returns, the trader's trades and
`TradeWithPlayerGoal`, the zombie villager's offers and gossips kept and the cure (V 6).

`trading_player` freezes a villager (`isTrading`). It is cleared whenever the player's merchant
menu on that villager closes for any reason (a close, a leave, a hand-over of either, death),
and at every take-in of the villager (a hand-over, a merge, a split, a restore) where the region
does not hold that player with that menu open, so that no villager stays frozen by a trade that
ended elsewhere.

---------------------------------------------------------------------------------------

## 9. Raids (N10d, N10e: the lead's; R1, R2)

### 9.1 The record (`region/raids.rs`'s frame; the lead's)

`Raid` as the game's fields (D10 C §1.4 and its codec's names): id, centre, status, waves
spawned, groups, omen level, ticks active, cooldown, post-raid ticks, celebration, spawn
position, the bar's UUID (D10 C: not saved by the game; Clustine saves it, so that a raid that
moves between regions shows one bar), total health, heroes (by UUID, ascending), members by
wave (entity keys, ascending). Kept as a part of a chunk (flag bit 5 of a chunk file; log kind
20 "commit with raids", whole per changed chunk, folded after the block changes and claims;
`CopyUpdate` does not carry raids, since copies never read them). At its making the record's
chunk is its centre's. A centre that moves (V 7.2: to the nearest village section, within two
sections) to another chunk of the land takes the record with it in the same commit; to a chunk
outside the land, the record stays in its chunk with its centre moved (named): the table says
where the record is, and every effect goes to the record's chunk.

**Its stream**: `Random::of_raid(world, id, ticks_active)` with a count within the tick, as
`entity_draws` counts (streams keep no state, ADR-0028 section 4: keyed by the id alone every
use would draw the same), declared in N10d; R1 tests that two waves draw differently.

**`active`**: the game's `hasChunkAt(centre)`; in Clustine, the record's chunk is ticked by its
region and the centre's chunk is in that region's land or copies. A raid whose centre went
beyond the copies pauses until a merge or the centre's return (named; where regions follow
players the village is one region's).

### 9.2 The world's table (the lead's)

`StoreRequest::Village`'s `raids` (section 3.5): `Option<Vec<RaidSeen>>`, `None` when unchanged
since the last report, `Some` with every raid of the land otherwise (`Some(vec![])` says "no
raids now"): `RaidSeen { id, chunk, centre, active, over, victory, loss, stopped,
first_wave_spawned, between_waves, omen, bar: (progress, name, uuid) }`. The store keeps each
region's last list (taking a region's word only in its current session), and publishes the
union to every region in every `StoreReply::Village`, deduplicated by id (a split leaves one id
under the parent until the part reports: the one reported later wins). A region drops its own
raids from the table it reads and keeps the last table while none is published.

`get_raid_at(pos)`: the nearest raid within 96 that is not stopped, of the region's own and the
table's together (the game's nearest). A raid id is unique in the world (an entity key).

### 9.3 Members elsewhere: the raid's region judges (the lead's)

`Effected::Raid(RaidNews)` to the record's chunk (the holder finds the raid by id among its
raids; a raid it does not hold ignores the news):

- `Joined { member, wave, health }`: a raider of another region joining (V 7.3: raiders without
  a raid every 20 ticks). Accepted only if the raid is still active, not over, and the wave
  still the current one; refused otherwise, answered with `EntityUse::Raider(RaiderUse::
  Rejected { raid })` to the member's chunk, and the member clears its raid (it is no longer
  persistent for the raid).
- `Hurt { member, health }`: the bar's health. A hurt of a member the raid does not list (gone,
  dead, refused) is ignored.
- `Died { member, killer: Option<Uuid> }`: removed; a player killer becomes a hero.
- `Left { member }`: removed. The member's holder decides it by the game's rules it can judge:
  another dimension, 112 or more from the table's centre, despawned or discarded, idle outside a
  village for 30 passes (its own count).

A member the raid's region cannot see (not in its land or copies) is **alive until `Died` or
`Left`**: `updateRaiders`' "not found after 600 ticks" applies only to members the region could
see in its land; a member handed over across a line is not gone, and the next wave waits for
it as the game's would. X1 tests a member handed across a pinned line mid-wave, its death
there, a late hurt and a refused join.

### 9.4 The bar and the horn (the lead's per-player events; E1's packets)

The bar's players: those alive whose `get_raid_at` is the raid, each region for its own players
(the raid's region from its record, others from the table, so a player across a pinned line
sees the bar a second late, named). `PlayerEvent::BossBar { id, op }` with the six operations
(D10 C, settled from the operation classes); the edge keeps each player's bars and sends
`boss_event` (ADD when visible, the others on a change, REMOVE). The horn: a sound to each of
the region's players within 64 of the spawn point, or in the bar; the table's players across a
pinned line are not reached (named).

### 9.5 The omens (N10e; the lead's)

Player effects tick (`region/player_effects.rs` 232–257 only counts down today): bad omen and
raid omen as D10 C.1 (raid omen added and the position stored on the first tick; bad omen
removed on the next if the player still qualifies; the effects after it in the game's map order
not ticked that tick: Clustine ticks effects in id order and names the order),
`raid_omen_position` kept with the player (their record, carried through hand-overs and leaves:
the stay note's and the players' file's format rise in N10e), and at raid omen's end
`create_or_extend_raid` at the stored position (D10 C.7):

- The centre is the mean of the occupied `#village` records within 64 of the position, read
  from land, copies and margin; the work is sent to the **centre's chunk**: done by the
  player's region where it holds that chunk, else by `Effected::Raid(RaidNews::Start {
  player, omen, centre })` to its holder.
- The centre's holder does the game's `getRaidAt(centre)`: a raid it holds absorbs the omen; a
  raid of the table gets `RaidNews::Absorb { player, omen }` at its record's chunk; none makes a
  new raid there, after re-checking its own records (so a second start in the same region in
  the same second extends the first).
- Two regions starting raids at one village within the table's second at a pinned line make
  two raids (named; one region where regions follow players).

### 9.6 The trader's chance (N10e; the lead's)

The store's side of decision 12: `StoreRequest::TraderChance { firing, chance }`, logged (a new
record kind) and kept with the clock's state, the later firing winning, published with
`StoreReply::Clock`; its format raise with N10e's.

### 9.6 Raiders and the ravager (R1, R2)

R1: the raid's tick on N10d's frame (V 7.2 with D10 C.2–C.6), waves and the spawn search,
`spawnGroup`, `joinRaid` with `SpawnReason::Event`, `applyRaidBuffs`, `updateRaiders`, victory
and Hero of the Village (by `OnPlayer` to heroes anywhere), celebrations; the raiders' goals
(`ObtainRaidLeaderBannerGoal`, `PathfindToRaidGoal`, `RaiderMoveThroughVillageGoal`,
`RaiderCelebration`, `VindicatorBreakDoorGoal`, the witch's healer target, open doors).
R2: the ravager (n7k Ravager.txt), its goals, roar, stun and riders.

---------------------------------------------------------------------------------------

## 10. The golem, cats, the siege, the trader's spawner, other village goals (G1)

V 8 with D10: the golem's `DefendVillageTargetGoal` (reading the copies' `Shown` gossips),
`GolemRandomStrollInVillageGoal`, `MoveBackToVillageGoal`; the cat spawner; the siege (decision
11, one a world); the trader spawner (decision 12: its delay a schedule chain, its chance the
store's record through the request N10e builds; its two leashed llamas); zombies'
`MoveThroughVillageGoal`; foxes' village goals.

---------------------------------------------------------------------------------------

## 11. What clients are sent

V 10's table, whole. New at the edge: the merchant window (`open_screen` type 19 with the
profession's name, `merchant_offers` after it and after each change), the boss bar per player,
the horn. Everything else is built (entity events, data, sounds, level events, block changes).

---------------------------------------------------------------------------------------

## 12. Named differences

1. Offers drawn from the villager's stream aside, not the world's per-set streams (decision 5).
2. No take across a line; a margin record never has space (decision 2).
3. The margin up to 1.5 s old; the records of chunks not held since the store started unknown
   (3.5).
4. A release (or a raid's start) filed for land nobody holds is dropped at a store restart, and
   its ticket stays claimed (decision 2).
5. The village tracker's residue of 6 not kept (3.4).
6. Query order within a section fixed, not a hash order (decision 15).
7. The JDK's global random drawn from the villager's stream (5).
8. Gossip and raid sets iterated by key (7, 9).
9. Effects ticked in id order, not the effect map's order (9.5).
10. A raid's record may be off its centre's chunk after the centre moved across a line, and
    pauses while its centre is beyond its region's copies (9.1).
11. The bar across a pinned line a second late, the horn not at all (9.4).
12. Two raids at one village from two regions' omens within a second at a pinned line (9.5).
13. No explorer maps (decision 9).
14. A trade with a villager across a pinned line opens nothing (decision 6).
15. The brain kept whole at every load: transient memories, caches and the raid's default
    activity survive an unload (decision 14).
16. A bell's ring reaches no entity beyond its region's copies (decision 17).
17. A villager lying down in a bed whose head is across a line: the bed's `occupied` is set a
    tick later by effect (V 11).
18. A trader firing doubled at a pinned line (decision 12).

---------------------------------------------------------------------------------------

## 13. The division of work

Each step one verified commit to `main`, pushed. The lead's first, in order:

| Step | What |
|---|---|
| N10a | The data: `PoiType` and its block table; the trades' compiler (`villager_trade`, `trade_set`, tags, `TradeCost`'s default count 1); the gift tables and `cat_morning_gift`; the enchantment, potion and structure tags trades need |
| N10b | Points of interest (section 3): claims (store format 17, log kind 19, `CopyUpdate`'s claims), the index, queries, take and release, `Effected::Release`, the margin's request and reply, the village (the stand-ins kept until V2) |
| N10c | The contracts and every cross-part hook (section 4), `STATE_FORMAT` 22, and the villager streams' tests changed by `SetRaidStatus`'s level draw (which N10c adds to the villager's core as an inert one-shot) |
| N10d | Raids as world state (9.1–9.4): the record, its stream, log kind 20 and store format 18, the table, `Effected::Raid`, `EntityUse::Raider`, the bar's players |
| N10e | The omens' ticks and `raid_omen_position` (9.5), the trader's chance (9.6), their formats |

The fresh load of V 16.14 is not built (decision 14).

Then in parallel, each in a worktree of its own. Paths under `crates/clustine-sim/src/` unless
another crate is named; **no file is touched by two parts**, and where one part's rule is
called from another's file, N10c put the call there, to an inert function in the callee's
file:

| Part | What | Files |
|---|---|---|
| V1 | The brain (section 5), the bed's kick | `mobs/types/villager.rs`, `mobs/brain/behaviours/villagers.rs`, `mobs/brain/behaviours/rest.rs` (new), `mobs/brain/sensors_villager.rs` (new, the four sensors; `sensors.rs` keeps the shared ones), `region/sleep.rs`, `mobs/types/villager_n10_tests.rs` (new) |
| V2 | Work and professions (section 6) | `mobs/brain/behaviours/work.rs` (new), `region/composting.rs` (new; `region/hoppers.rs` calls it through N10c's hook), `mobs/brain/behaviours/work_n10_tests.rs` (new) |
| V3 | The social life (section 7) | `mobs/types/gossip.rs`, `mobs/brain/behaviours/social_villagers.rs` (new), `region/spawn_util.rs` (new), `region/bells.rs` (new), `mobs/types/social_n10_tests.rs` (new) |
| T1 | Trading's data and evaluation, enchanting (8.1) | `trading/offers.rs`, `trading/evaluate.rs`, `items/enchanting.rs` (new; `items/enchantments.rs` stays N9's), `mobs/making.rs`, the two new components in `clustine-world/src/components.rs`, `trading/trading_n10_tests.rs` |
| T2 | Trading's rules and menu, the cure (8.2) | `trading/merchant.rs`, `menus/merchant.rs` (new module beside `menus.rs`, declared by N10c), `mobs/types/wandering_trader.rs`, `mobs/types/zombie_villager.rs`, `trading/merchant_n10_tests.rs` |
| E1 | The edge: the merchant window, the boss bar per player, the horn | `services/edge/src/merchant.rs` (new), `services/edge/src/boss_bars.rs` (new) |
| R1 | Raids' rules on N10d's frame (9.6) | `region/raid_tick.rs` (new; `region/raids.rs` is N10d's), `mobs/ai/raider_goals.rs` (new), `mobs/types/pillager.rs`, `vindicator.rs`, `witch.rs`, `illagers.rs`, `region/raids_n10_tests.rs` |
| R2 | The ravager | `mobs/types/ravager.rs`, `mobs/types/ravager_n10_tests.rs` |
| G1 | The golem's village goals, cats, the siege, the trader's spawner, zombies' and foxes' village goals (10) | `mobs/types/iron_golem.rs`, `region/custom_spawners.rs`, `mobs/ai/village_goals.rs` (new), `mobs/types/fox.rs`, `mobs/types/zombie_family.rs`, `mobs/types/trader_llama.rs`, `region/village_n10_tests.rs` |
| X1 | Tests from the specification (the brief, D10, this plan) by someone who did not write the code: part A after N10b to N10e, part B after the parts | `region/tests/n10_*.rs`, `bin/clustine/tests/n10_*.rs` |
| X2 | The comparisons of section 14, for the lead to run | `bin/clustine/tests/merchant_oracle.rs`, `raids_oracle.rs`, `villagers_oracle.rs` |

Order: N10a to N10e in turn; T1 after N10a; E1, X2, V1, V2, V3, T2 and G1 after N10c; R1 and R2
after N10d and N10e. The stand-ins move onto the village in the commit that lands V2 (3.4).
**Not delegated**: the claims' life through merges, splits, lends, restores, copies and the
store's fold, the release effect and the margin (N10b); the villager and raider effects' holder
sides and the merge (N10c); the raid record's hand-overs, its effects and the bar's players
(N10d); the omens' hand-over and the trader's record (N10e); `fanout.rs` and the protocol's
packets.

---------------------------------------------------------------------------------------

## 14. Comparisons with the official server (X2 writes, the lead runs)

Exact where nothing draws: the merchant screen packet by packet (`/summon villager` with fixed
`Offers`, a trade, a shift-click repeat, a select, a close); restock's timing given
`LastRestock`; the boss bar's packets for a raid whose waves are summoned; the schedule's
switch ticks (`/time set`, the activity seen by pose and work sound); a profession taken at a
lectern (event 14 and the data); the cure's end with a fixed `ConversionTime`. By distribution:
offers per level over many villagers, gossip spread, the raid's waves.

---------------------------------------------------------------------------------------

## 15. Cost

Measured after V1 and V2 and again after R1: a village of 30 villagers at dusk before they have
beds (every bed claimed by a path search), and a raid's three waves, in memory and on the real
disk, as N7 measured mobs. Acted on if a tick exceeds 50 ms on the real disk.

---------------------------------------------------------------------------------------

## 16. Records and the roadmap

ADR-0022 amended for claims and raids in the chunk; ADR-0028 for the villager's `Shown` and its
effects; `docs/world-format.md` to formats 17–19; the roadmap's state and what to try after
each part.

---------------------------------------------------------------------------------------

## 17. The review (2026-10-27), and what the plan is now

An independent reviewer read the draft against the code at `92471b0`, the brief, D10 and the
ADRs (`n10-review.md` in the lead's scratchpad, not kept). Eleven defects, eleven risks, five
groups of nits. Each, and where the plan now answers it:

| # | Finding | Now |
|---|---|---|
| 1 | The fresh load rests on a mark that does not exist (`StoreReply::Loaded` has no `fresh`, `fresh_at` is never set) | decision 14: not built, the live brain kept, named; N10f dropped |
| 2 | The margin as a delta is lost by the runner's overwrite, by restores, merges and splits, and held back by the counts' gate | 3.5: a request and reply of their own, whole after every opening, folded by the runner |
| 3 | Copies do not carry claims | 3.2: `CopyUpdate`'s claims, a claims-only change counting for the gap check |
| 4 | Raid effects addressed by centre while the record may be off it; `active` undefined | 9.1, 9.2: the record's chunk in the table, effects to it, `active` defined |
| 5 | A yielded job site can leak its ticket | decisions 2 and 4: the holder re-checks and releases |
| 6 | Members held elsewhere: liveness, refusal, hand-overs | 9.3 |
| 7 | Starting or extending a raid across a line | 9.5: the work at the centre's chunk, `Absorb`, the double start named |
| 8 | The store cannot know a pinned region's land; old and new holders confused | 3.5: land from the region's whole report, `let_go` from the old holder, records kept for chunks let go |
| 9 | The raid's stream has no counter | 9.1: keyed by id and ticks active, with an in-tick count |
| 10 | The trader's record has no write path, format or owner | decision 12, 9.6: the delay a schedule, the chance a logged request, the lead's in N10e |
| 11 | Refusing a mismatched claim makes a hole in the world | 3.2: dropped, the fold's order, `remakes` |
| 12 | A player's use of a bed a villager sleeps in | section 5, V1 owns `region/sleep.rs` |
| 13 | Parts touching the same files; files that do not exist | section 13: the files named, the hooks in N10c |
| 14 | Releases for unheld land dropped at a store restart | decision 2, named 4 |
| 15 | The margin unknown for chunks not held since the store started | 3.5, named 3 |
| 16 | `trading_player` left set | 8.2 |
| 17 | The raid table's partial reports, duplicates, lookup order, the gate | 9.2 |
| 18 | The siege once a region | decision 11: one a world |
| 19 | Stand-ins moved before anyone claims | 3.4: moved with V2 |
| 20 | The village cache not cleared by the margin | 3.4 |
| 21 | Copies and lends carrying raids | 3.6, 9.1 |
| 22 | The 112-block merge has no mechanism and is nearly redundant | decision 7: dropped |
| 23 | The exhaustive matches listed wrongly; the log's codec | section 4 |
| 24 | Counts reported every changing tick, published once a second | 3.5 |
| 25 | `Random::aside` does not exist | decision 5 |
| 26 | N10e changes the player's record without a raise | 9.5, section 13 |
| 27 | The merge distance, copies against the margin, a bed across a line, the five chunks' reason, the villager record's version | decision 4, 3.3, named 17, decision 2, section 4 |

