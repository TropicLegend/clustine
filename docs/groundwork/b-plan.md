# Phase B: block behaviour, planned

Draft of 2026-10-15, revised after an independent review the same day (section 8); to be reviewed again as ADR-0024 before it is built. Phase B of the
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
region: scheduled ticks, fluid ticks and random ticks run only there, and a chunk is
post-processed (3.6) as it first becomes so. Every read of a tick then stays in
loaded chunks, as in vanilla, including a fluid's search for a slope four blocks
away.

A region loads, besides the chunks its viewers see, **one ring around them** where the
chunk is its own or nobody's (a ticket of a kind of its own, not a viewer's, never
sent to an edge for it). The view's rim then ticks and is post-processed before it is
sent, as vanilla's is. A ring chunk that is another region's is not claimed; the
chunks beside it do not tick (section 3.5).

### 3.3 Time: a clock per chunk

Region tick numbers differ from region to region and survive neither merges nor
splits, so a tick's time is counted in its **chunk's clock**: the number of ticks
the chunk has been ticked. A scheduled tick is due when its chunk's clock reaches
`due = clock + delay`. A chunk's clock runs only while the region ticks it, which is
vanilla's waiting of unloaded ticks, carried a little further (named: in vanilla a
loaded chunk that does not tick still ages).

While a region ticks a chunk its clock is `base + (region tick − since)`, where `base`
and `since` are set as it starts ticking; the pair changes only when the chunk starts
or stops ticking, and is committed then (3.4), so the clock is durable without a
commit per tick.

Within a tick the queue runs in (region tick at which due, priority, class, order,
chunk, place in the chunk's list), where class puts ticks loaded with a chunk before
those scheduled since, as vanilla does, and `order` is the region's counter (in
`RegionState`, durable; a split's part inherits it, a merge takes the larger). Chunk
and place make any tie of two regions' orders after a merge deterministic, without a
region's id in each tick.

### 3.4 What is durable, and how

Today a tick commits only its block changes, and a restore replays them on the stored
chunks; a chunk is unsaved only by a block change. Phase B commits, besides:

- a chunk's **ticks**, whole, whenever they changed in the tick (scheduled, run,
  dropped), with each tick's order and class;
- a chunk's **marks** when they are taken (3.6), and its **seam entries** (3.5) when
  they change;
- a chunk's **clock pair** when it starts or stops ticking.

The store applies them on replay as it applies block changes, and a chunk with any of
them is unsaved. `TickOutput` names the chunks and what changed. When a chunk is saved
(checkpoint, release, return) its ticks are written into it with the delay left from
its clock, in queue order with their orders.

**Merges and splits** end in `Region::restore` with nothing loaded: `take_absorbed`,
`take_split` and the saving of a chunk write each chunk's queue and clock into the
chunk itself, and the chunks start ticking again as they load in the region they went
to. **A restored region** loads its chunks over its first ticks; a chunk's clock goes
on from the committed pair as if it had ticked throughout, so ticks that fell due
while it loaded run at once in queue order. The restore tests' equality holds for
ticks due after the copy has loaded; the record says so and the tests are written to
it (ticks scheduled to fall due after loading, and one test that overdue ticks run
at once in order).

**The format** gains each tick's order and class, the seam entries and the clock. The
store reads the previous format (ticks in list order as loaded ones, no seams, clock
0), so phase B does not refuse a world; phase S's switch does, once.

**Random ticks** need no state: the positions of a section are drawn from a stream
seeded by the world's seed, the chunk, the section and the chunk's clock.

### 3.5 At a region's border

A chunk beside another region's chunk does not tick (3.2), so water stops a chunk
short of the border and waits, as it does at the edge of vanilla's simulation
distance. What still reaches over a border is an **update**: a chain from a ticking
chunk that runs on beyond its 3×3, or a player's action in a chunk beside another
region's (breaking the block under a torch that stands in the other region). Such an
update is not carried over. It is kept as a **seam entry** in the chunk it came
from: the position it would update, the direction, the update's flags and the depth
left; at most one per position and direction (a later one replaces it); none for a
position outside the world. When one region has both chunks loaded (a merge, a load,
a claim), it runs the chunk's seam entries in position and direction order, before
the chunk's ticks of that tick, and forgets them.

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

The other way is to carry updates and ticks between regions: a durable message from
region to region (no edge need be there), with the holder applying it in order with
its own, and ticking chunks beside a border by reading the neighbour's blocks. That
is a design and a review of its own. **Question 1** for the owner.

### 3.6 Generation's marks and ticks

A chunk that **has marks** is post-processed when it first ticks in a region, by
vanilla's rule of 3.1: fluids' ticks run, liquid blocks tick, other marked blocks
take their shape without neighbour updates; its marks are then cleared, committed and
the chunk is unsaved. The test is the marks themselves, not `Chunk::generated()`:
`set` clears that flag, and a chunk post-processed and then lost in a crash comes back
changed and still marked. Generation's ticks are loaded with the chunk as ticks loaded
with it (3.3).

A chunk's first snapshot goes to an edge **after** it is post-processed, when it
ticks; a chunk in view that cannot tick (beside another region) is sent as it is and
settled later. Because of the ring (3.2) that is only at a border.

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
| B3 | Updates: `setBlock` with flags, the depth-first updater, `onPlace`, `affectNeighborsAfterRemoval`; support (torches, plants, snow, carpets, rails, doors and double plants, ladders, vines...), shapes of fences, walls, panes, stairs and wire on a neighbour's change | Simulation tests; `placing_oracle.rs` extended with cases of breaking under supported blocks, judged against the official server |
| B4 | Ticking chunks and the ring (3.2); scheduled ticks with chunk clocks (3.3); durability: commits, replay, saves, the format read back from the previous one, checkpoint, restore, merge, split (3.4); falling blocks at once (question 2) | The state tests extended (pending ticks across restore, merge and split, by the record's equality); tests from the record by someone else; oracle cases of undermined sand and gravel |
| B5 | Fluids by `FlowingFluid`'s rules: spread, drain, sources, lava and water making stone, cobblestone and obsidian, waterlogging, concrete powder hardening | Simulation tests against vanilla's numbers; oracle cases (a bucket on flat ground, a channel, a drop, lava meeting water) compared once settled |
| B6 | Post-processing at first tick (3.6); seam entries (3.5) | Tests from the record by someone else (one region; two regions; a merge that joins a seam; a crash between post-processing and a save); F8's count |
| B7 | Random ticks that need no light (3.8) | Simulation tests; a felled tree's leaves gone within vanilla's time (statistically) |
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

**Answered on 2026-10-15, each as recommended**: (1) effects wait at borders for now,
and carrying updates and ticks between regions is a record of its own taken up right
after B; (2) falling blocks land at once; (3) behaviour that needs light waits for
light in the simulation, a step after B.


1. **At a region's border, effects wait** (3.5): a chunk beside another region's does
   not tick, and updates across wait until one region has both sides; permanent with
   pinned regions, for 30 s and more after a hand-over, out of view otherwise.
   Recommended: yes for now, and carrying updates and ticks between regions as a
   record of its own after B, if the owner wants pinned borders seamless.
2. **Falling blocks land at once**, without a fall to watch, until there are entities?
   Recommended: yes, as the terrain plan says.
3. **Behaviour that needs light** (melting, crops, bamboo, grass spreading) waits for
   light in the simulation, a step after B? Recommended: yes.

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
