# Phase B: block behaviour, planned

Draft of 2026-10-15, revised after two independent reviews the same day (sections 8 and 9); to be reviewed a third time as ADR-0024 before it is built. Phase B of the
[terrain plan](terrain-plan.md) (B1 to B6, question 5: in this milestone, by the
project's rule). Its first step is a decision record, ADR-0024, whose substance is
section 3 below; the record is written from this plan once the plan is agreed.
Phase S ([its plan](s-plan.md)) goes beside it.

## 1. Why, and what a player meets

Today a block changes only when a player places or breaks it, and nothing changes
in turn. Within minutes a player meets:

- a torch, a flower or snow left standing in the air when the block under it is
  broken; the top half of a door or a tall flower left when the bottom goes;
- sand and gravel hanging over a dug-out space;
- water and lava that never flow: a bucket's water stands as one block, a spring is
  one block on a cliff, a cave cut through an aquifer shows the water as a wall;
- leaves that stay forever when the tree's log is cut;
- fences, walls and panes that do not reconnect when a neighbour goes (W6 shapes a
  block from its neighbours as it is placed; the neighbours do not change in turn).

Generation has left what settles these in every chunk: scheduled ticks (fluids, sand)
and marks (blocks to take their shape from their neighbours), stored and never read
(ADR-0022, finding 1: 72 of 101 sampled chunks carry one; most marks sit on water).

## 2. Where things stand

- `crates/clustine-sim`: `Region::tick` (`region.rs:397`) runs chunk updates, edge
  events, remote actions, inputs, settles chunks and takes a delta. Block changes all
  end in `set_block` (`:1514`), which sets the block in the chunk and reports
  `BlockChanged`. A click's changes are applied all or nothing (`change_all`), refused
  if any is in a chunk that is not loaded.
- A region's durable state (`RegionState`) has no chunk knowledge by design (ADR-0012,
  section 1.4); blocks live in chunks, saved at checkpoints (ADR-0018). Merges and
  splits carry chunks by the line between regions.
- `Chunk` has `ticks: Vec<ScheduledTick { at, of, delay, priority }>` and
  `marks: Vec<InChunk>`, in the on-disk format; `for_viewers()` strips both.
- A survival rule exists twice: `clustine-sim/src/placement/survive.rs` (to refuse a
  placement) and `clustine-features/src/survive.rs` (for features, with more blocks).
  `updateShape` exists only for generation (`clustine-features/.../trees/shape.rs`:
  leaves, liquids, plants, double plants, vines, falling blocks, snowy dirt).
- `clustine-data` has, per state: solidity, replaceability, sturdy faces by support
  type, the fluid (kind, amount, source, falling), the post-processing offset,
  collision shapes, push reaction; per block its class with ancestry (`FallingBlock`,
  `LiquidBlock`, `LeavesBlock`, `DoorBlock`, `DoublePlantBlock`...).
- Effects cross regions only as a player's actions (`RemoteAction`, through the edge's
  durable outbox, ending in `RemoteDone`). Nothing carries a block's effect on a
  neighbour in another region.
- Light is made at the edge (ADR-0023); the simulation has none.

## 3. The design (ADR-0024's substance)

### 3.1 What vanilla does, which is followed

Checked against the reference ports (SteelMC-26.3 above all) and, where they differ,
the jar's classes.

- **Setting a block** (`Level.setBlock`, default flags 3): inside the chunk's own set,
  the old state's `affectNeighborsAfterRemoval` and the new state's `onPlace` run
  first; then the client is told; then `updateNeighborsAt` (`neighborChanged` on the
  six neighbours in the order west, east, down, up, north, south) and
  `updateNeighbourShapes` (each neighbour's `updateShape`, in the order west, east,
  north, south, down, up, setting it if its shape changes, with flags that carry the
  update on, to depth 512). There is no `onRemove` in 26.3.
- **The updater** (`CollectingNeighborUpdater`) is a stack: an update made while one
  runs is run before that one's remaining siblings (**depth first**), up to
  `max-chained-neighbor-updates` (1,000,000).
- **Scheduled ticks** (`LevelTicks`): per-chunk containers; a tick has a trigger time,
  a priority and a sub-tick order. Due ticks run in (time, priority, order), at most
  65,536 a tick for blocks and as many for fluids, blocks first. A tick is not
  scheduled twice for the same position and block. Ticks a chunk saved are given
  negative orders as it loads, so they run before ticks scheduled since at the same
  time and priority. A chunk saves its ticks with the delay left; unloaded, they wait.
- **Which chunks tick**: scheduled and random ticks run only in chunks that tick with
  entities, whose eight neighbours are always loaded; so a tick never reads an unloaded
  chunk. The server loads one ring beyond what it ticks.
- **Post-processing** (`LevelChunk.postProcessGeneration`, when a chunk becomes
  ticking in `ChunkMap.prepareTickingChunk`, before it is sent): each marked position
  that holds a fluid has its **fluid tick run at once**; a `LiquidBlock` has its block
  tick run; any other block is set to `Block.updateFromNeighbourShapes` of its state
  with `UPDATE_INVISIBLE | UPDATE_KNOWN_SHAPE`, that is **without neighbour updates**.
  Generation's saved ticks enter the level's ticks with the chunk.
- **Random ticks**: three random positions per section per tick (`randomTickSpeed` 3).
- **Falling blocks** schedule a tick (delay 2) on placing and on a shape update; at
  the tick, with air, fire or a fluid or replaceable block below, they become an
  entity. Pointed dripstone falls and shatters on landing; concrete powder hardens
  where it touches water.

### 3.2 Which chunks a region ticks, and the ring it loads

A region **ticks a chunk** when the chunk and its eight neighbours are loaded in the
region: scheduled ticks, fluid ticks and random ticks run only there, and a chunk with
marks is post-processed (3.6) as it first becomes so. Every read of a tick then stays
in loaded chunks, as in vanilla, including a fluid's search for a slope four blocks
away.

A region loads, besides the chunks its viewers see, **one ring around them** where the
chunk is its own or nobody's, by a ticket of a kind of its own (a ring ticket), never
sent to an edge for it. From what a region knows of a chunk (ADR-0012): `Unknown` is
claimed, `Held` loaded, `Foreign` passed over, `Asked` waited for. A ring ticket counts
wherever a viewer's does for keeping a chunk: in accepting a load (`chunks_loaded`), in
what is used (`settle_chunks`, so that a ring chunk is not returned and claimed again
and again) and in what is wanted (so that a `Foreign` belief of a ring chunk is kept).
The view's rim then ticks and is post-processed before it is sent, as vanilla's is.
Vanilla also post-processes the ring itself; Clustine does not, so an update from the
rim can reach a ring chunk whose marks later run against it: named.

Two regions' rings meet before their views do. The coordinator's merge distance
(`D_m = 2(V+1)+4`, `services/coordinator/src/policy.rs:48`) leaves four chunks for an
approach; it grows by two, so that a rim in someone's view does not stop ticking during
an ordinary approach.

### 3.3 Time: delays left, which wait while a chunk does not tick

Nothing durable counts in a region's tick numbers, which differ from region to region
and do not survive merges and splits. A chunk's ticks are kept in its file, and in what
is committed of it, as **the delay left** from the moment they were written. A region
that starts ticking a chunk (after a load, a merge, a split, a restore) makes each due
at its own tick plus the delay. So a tick waits while no region ticks its chunk:
vanilla's waiting of unloaded ticks, carried a little further (named: in vanilla a
loaded chunk that does not tick still ages). After a crash a tick runs at most its own
delay later than it would have, since a chunk whose ticks change is committed within
that time (3.4).

There are **two queues**, block ticks and fluid ticks, as in vanilla: each tick all due
block ticks run (at most 65,536), then all due fluid ticks (as many). Within a queue,
ticks run in (due, priority, order). A tick scheduled by one of the tick's runs is put
in the queue for later ticks: the due ticks are collected before any runs (vanilla's
`collectTicks`), and a tick of delay 0 scheduled meanwhile runs in the next tick.
`order` is the region's counter at scheduling, in `RegionState`; a split's part
inherits it, a merge takes the larger. When a chunk starts ticking its stored ticks
are given orders below every order given at scheduling, in the order of the chunks
starting (ascending positions within a tick) and of each chunk's list, as vanilla
gives a loaded chunk's ticks negative orders: they run before ticks scheduled since at
the same due and priority.

**Random ticks** need no state: a section's positions are drawn from a stream seeded
by the world's seed, the chunk, the section and the region's tick.

### 3.4 What is durable, and how

Today a tick commits only its block changes, and a restore replays them on the stored
chunks; a chunk is unsaved only by a block change. Phase B commits, besides, for each
chunk where they changed in the tick:

- its **ticks**, whole, as delays left from this tick, in queue order;
- that its **marks** were taken (3.6);
- its **seam entries** (3.5), whole.

`TickOutput` names the chunks and what changed. A chunk with any of them is unsaved.
A chunk that only **stops ticking** (a merge, a split, a release) is unsaved only if it
has ticks pending, which are then written into it with their delays; a chunk with none
is as it was. Saving a chunk (checkpoint, release, return) writes its ticks into it
the same way. **The warm copies** a merge or split hands over, which stand for what the
store has, are such saved forms. `Region::saved_chunk(position)` gives that form, and is
what the restore tests load.

**The store** carries the records through: `LogRecord::Commit` gains per-chunk records,
written only when there are some, so that a commit without any is written as before
(as `stays` was added); the replay job (`Job::Restore`, `touched`, `apply`, `replayed`)
carries them with the block changes and applies them under the same rules of who held
the chunk; a chunk that only these records touched is touched. A chunk the store holds
only as generated is made and then has them applied, as with block changes (its
generation's ticks are part of the deterministic output). A commit of ticks, marks or
seams clears the chunk's `generated` flag, so that the remaking of a chunk whose
generation failed (`remakes`) does not lose them.

**The format** gains seam entries; ticks keep their present form (delays and
priority, in queue order). The previous format of chunks and of the log is read, so
phase B refuses no world; phase S's switch does, once.

**The restore tests.** A restored region's chunks are given to it from their saved
forms in its first tick, and the original is not ticked while the copy loads (the
existing tests let it idle two ticks, which with B changes things). Equality then holds
but for ties of due and priority between ticks of different chunks, which a reload
orders by load (3.3), as vanilla's does; one test shows that order.

### 3.5 At a region's border

A chunk beside another region's chunk does not tick (3.2), so water stops a chunk
short of the border and waits, as it does at the edge of vanilla's simulation
distance. What still reaches over a border is an **update**: a chain from a ticking
chunk that runs on beyond its 3×3, or a player's action in a chunk beside another
region's (breaking the block under a torch that stands in the other region). Such an
update is not carried over. It is kept as a **seam entry** in the chunk it came from:
the position it would update, the direction, the kind (`updateShape` or
`neighborChanged`), the source block (which `neighborChanged` is given), the update's
flags and the depth left; at most one per position, direction and kind (a later one
replaces it); none for a position outside the world. When one region has both chunks
loaded (a merge, a load, a claim), it runs the chunk's seam entries in position,
direction and kind order, then forgets them.

**Where a player meets this.** Regions that follow players keep a border out of
everyone's view as a rule, but not always:

- with pinned regions (`--pin`), which never merge, a border is where it is: water
  stops a chunk short of it on both sides, and a torch whose support was broken across
  it stays, for good;
- a player handed into land another region holds keeps seeing the region they left
  until it is absorbed, 30 seconds and more after it empties (ADR-0016's
  `EMPTY_FOR`); in that time water near the old border waits and updates across it
  wait;
- a chunk whose post-processing needs another region's neighbour is sent unsettled
  and settled when one region has the 3×3 (3.6).

The owner chose this for now (question 1); carrying updates and ticks between regions
is a record of its own after B.

### 3.6 Generation's marks and ticks, and when a chunk is first shown

A chunk that **has marks** is post-processed when it first ticks in a region, by
vanilla's rule of 3.1: fluids' ticks run, liquid blocks tick, other marked blocks take
their shape without neighbour updates (flags as SteelMC-26.3's, with
`UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`); its marks are then cleared, committed and the
chunk is unsaved. The test is the marks themselves, not `Chunk::generated()`: `set`
clears that flag, and a chunk post-processed and then lost in a crash comes back
changed and still marked. Generation's ticks start with the chunk as ticks loaded
with it (3.3).

**The first snapshot.** Only a chunk that still has marks waits before its snapshot
goes to an edge, and only while a missing neighbour is one the region holds and is
loading, or has asked the store for; a neighbour that is another region's, unreadable
or being generated lets it go as it is, settled later. The runner's hold of an edge's
inputs (`link.hold`) treats a chunk waiting so as it treats one being generated, so a
resume or a hand-over never waits on a ring's generation. The world store's spawn area
(`SPAWN_AREA`, 8) grows to the view distance and two, so that a first join does not
wait on two rings of generation.

### 3.6a The order within a region's tick

After inputs and remote actions, as now: the seam entries of chunks joined this tick;
the post-processing of chunks that start ticking this tick, in ascending position;
the due block ticks; the due fluid ticks; the random ticks of ticking chunks, in
ascending position. Then the chunks are settled and the delta taken, as now.

### 3.7 Rules shared with generation

`updateShape` and `canSurvive` exist for generation in `clustine-features`, and
survival again in the sim. They move into one crate, `clustine-blocks`
(deterministic, no I/O), which the sim, the features, W6's placement and phase S's
templates use; generation's results stay bit-equal (the F sweep judges that).

### 3.8 Light

Some behaviour depends on light: ice and snow melt, crops and bamboo grow, grass and
mycelium spread, mobs spawn. The simulation has none (it is made at the edge). Those
are **left out** and named; leaf decay, sugar cane, cactus, kelp, vines and cave vines,
dripstone growth need no light and are in. **Question 3**: light in the simulation is
a step of its own after B.

## 4. Steps

| # | Scope | Verified by |
|---|---|---|
| B1 | ADR-0024 from section 3, reviewed independently again | Review |
| B2 | `clustine-blocks`: `updateShape`, `canSurvive` and shape helpers moved out of features and the sim, one rule set (before S1) | The F sweep and the spawn judge unchanged; the sim's placement tests unchanged |
| B3 | Updates: `setBlock` with flags, the depth-first updater, `onPlace`, `affectNeighborsAfterRemoval`; support (torches, plants, snow, carpets, rails, doors and double plants, ladders, vines...), shapes of fences, walls, panes, stairs and wire on a neighbour's change; a block destroyed by an update shown to clients with its particles and sound (the level event 2001 vanilla sends) | Simulation tests; `placing_oracle.rs` extended with cases of breaking under supported blocks, judged against the official server |
| B4 | Ticking chunks and the ring, the ring ticket and the merge distance (3.2); two queues of ticks with delays left (3.3); durability: commits, the log's records, replay, saves, the previous format read, checkpoint, restore, merge, split (3.4); the order within a tick (3.6a); falling blocks at once (question 2) | The state tests extended (pending ticks across restore, merge and split, by the record's equality); tests from the record by someone else; oracle cases of undermined sand and gravel |
| B5 | Fluids by `FlowingFluid`'s rules: spread, drain, sources, lava and water making stone, cobblestone and obsidian, waterlogging, concrete powder hardening | Simulation tests against vanilla's numbers; oracle cases (a bucket on flat ground, a channel, a drop, lava meeting water) compared once settled |
| B6 | Post-processing at first tick, the first snapshot and the hold, the spawn area (3.6); seam entries (3.5) | Tests from the record by someone else (one region; two regions; a merge that joins a seam; a crash between post-processing and a save); F8's count |
| B7 | Random ticks that need no light (3.8), and fire burning out and spreading, farmland drying, coral dying out of water | Simulation tests; a felled tree's leaves gone within vanilla's time (statistically) |
| B8 | Measured on the real disk: commits per second and bytes with water flowing, random ticks over a full view, a flight over new land (each chunk now stored once settled), a merge's pause; parity matrix, architecture, roadmap, what to try | Measurements; CI |

One author for B3, B4 and B6 (me): that is where ordering mistakes hide. Rule ports
for classes of blocks (survival, fluids' arithmetic, random ticks) can go to
subagents on files of their own, with tests from the record by someone else.

## 5. What the oracle can and cannot judge

The placing oracle runs a bot script on the official server and on Clustine and
compares the blocks a second bot is sent; it suits a settled result (support, sand,
water once still). Timing (water's five ticks a block) is judged by simulation tests
against vanilla's numbers. Random ticks are off in the oracle and stay off; B7 is
judged by simulation tests. All oracle cases stand in one region.

## 6. Questions for the owner

1. **At a region's border, effects wait** (3.5): a chunk beside another region's does
   not tick, and updates across wait until one region has both sides; permanent with
   pinned regions, for 30 s and more after a hand-over, out of view otherwise.
   Recommended: yes for now, and carrying updates and ticks between regions as a
   record of its own after B, if the owner wants pinned borders seamless.
2. **Falling blocks land at once**, without a fall to watch, until there are entities?
   Recommended: yes, as the terrain plan says.
3. **Behaviour that needs light** (melting, crops, bamboo, grass spreading) waits for
   light in the simulation, a step after B? Recommended: yes.
4. **Redstone** (levers, buttons, wire, lamps, repeaters, pistons): players in creative
   will place it. In B it is a large set of rules of its own on the same updates and
   ticks. Recommended: a phase of its own right after B and the record between
   regions, since it needs nothing else.

**Answered on 2026-10-15, each as recommended**: (1) effects wait at borders for now,
and carrying updates and ticks between regions is a record of its own taken up right
after B; (2) falling blocks land at once; (3) behaviour that needs light waits for
light in the simulation, a step after B. Question 4, which came with the second review, was
answered the same day as recommended: redstone is a phase of its own right after B.

## 7. Risks

| Risk | Answer |
|---|---|
| Ordering in the queue and the updater differs from vanilla's where it shows | Ported from the jar's classes; oracle cases; the order after a merge named |
| Fluids are expensive in oceans when post-processing runs their ticks | vanilla does the same work; its budget per tick; measured at first view (T8's measurement) |
| Commits on most ticks (random ticks, flowing water), more unsaved chunks and longer merges, and every chunk seen stored once settled | B8 measures all on the real disk before B is called done |
| The restore equality with ticks pending | The record states the equality; tests from it by someone else |
| Seam entries that never join | Bounded (one per position and direction), kept in chunks, cost nothing until joined; a test that a merge joins them |

## 8. What the review changed

An independent review against the code and the reference ports, 2026-10-15, found
the first draft not ready:

| # | Finding | Answer |
|---|---|---|
| 1 | Ticks would not be durable between checkpoints: commits carry block changes only, replay applies only them, and only block changes make a chunk unsaved | Ticks, marks, seams and clocks committed and replayed (3.4) |
| 2 | Merges and splits restore with nothing loaded; there is no moment for two queues to interleave, and a region id per tick would be needed | Queues written into chunks at absorb and split; the key ends in chunk and place; counters inherited or the larger taken (3.3, 3.4) |
| 3 | No time base; region ticks differ; the restore tests idle the original | A clock per chunk; the equality the tests require stated (3.3, 3.4) |
| 4 | Which chunks tick was undefined; every view's rim would be a seam; a fluid reads four blocks away | Vanilla's rule: a chunk ticks with its eight neighbours loaded; a ring loaded beyond the view (3.2) |
| 5 | Vanilla misstated: post-processing has no neighbour updates and runs fluid ticks; `onPlace` and removal run inside the set; the updater is depth first; shape updates have their own order; loaded ticks run first; bamboo needs light; dripstone and concrete powder | Corrected (3.1, 3.8, B5) |
| 6 | `Chunk::generated()` is the wrong test | The marks themselves (3.6) |
| 7 | Snapshots would show unsettled chunks along chunk lines | Post-process, then snapshot; the ring (3.2, 3.6) |
| 8 | "A border is in no one's view" overstated: pins never merge; hand-overs keep a border in view 30 s and more | Said in 3.5 and question 1 |
| 9 | Seam entries underspecified | Deduplicated, flags and depth, an order, none outside the world (3.5) |
| 10 | The format change would refuse worlds | The previous format is read (3.4) |
| 11 | Disk writes and commits not weighed | B8 measures them |
| 12 | `RegionState` additions incomplete | The counter; random ticks need no state (3.3, 3.4) |

## 9. What the second review changed

| # | Finding | Answer |
|---|---|---|
| 1 | A clock pair in one region's tick numbers is misread by another region; a restored region's clock both runs and does not | No region tick in anything durable: delays left, which wait while a chunk does not tick (3.3) |
| 2 | Committing a clock on every stop makes every loaded chunk unsaved, lengthening merges and writing every chunk passed | A chunk that stops is unsaved only with ticks pending; no clock (3.3, 3.4) |
| 3 | The restore test loads the live chunk, without ticks, and idles the original | `Region::saved_chunk`; the original is not ticked while the copy loads; ties named (3.4) |
| 4 | The log, the replay job and failed generations need the records too; the format version is shared with the log | Said (3.4) |
| 5 | Holding first snapshots would freeze resumes and hand-overs behind a ring's generation | Only chunks with marks wait, only for a neighbour being loaded or asked; the hold treats them as generating; `SPAWN_AREA` grows (3.6) |
| 6 | Rings meet before views; the ring ticket must count where a viewer's does | `D_m` grows by two; where the ticket counts said (3.2) |
| 7 | The seam key merged the two kinds of update | Keyed by kind too, with the source block; the order within a tick (3.5, 3.6a) |
| 8 | One queue, an unclear class, collect-then-run unsaid | Two queues; orders on every load as vanilla; collect then run (3.3) |
| 9 | Vanilla's ring is itself post-processed; a flag missing | Named; the flag added (3.2, 3.6) |
| 10 | A block destroyed by an update vanishes without particles; redstone is neither in nor out | The level event in B3; redstone is question 4 |
