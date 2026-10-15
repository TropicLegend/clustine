# ADR-0024: Block behaviour in a region

- Status: **Proposed** on 2026-10-15, from the plan of phase B
  ([b-plan.md](../groundwork/b-plan.md), section 3), after two independent reviews of
  that plan whose findings are worked in (the plan's sections 8 and 9); for a third
  review before it is built. The owner's answers to the plan's questions are in it:
  effects wait at a region's border for now, falling blocks land at once, behaviour
  that needs light waits for light in the simulation, and redstone is a phase of its
  own after B.
- Date: 2026-10-15

## Context

A block changes today only when a player places or breaks it, and nothing changes in
turn: support, falling sand, flowing water, leaves and shapes that follow their
neighbours are missing, and the ticks and marks generation leaves in every chunk are
never read ([ADR-0022](0022-what-a-chunk-carries.md), finding 1). A region's durable
state has no chunk knowledge by design ([ADR-0012](0012-the-tick-on-chunks.md),
section 1.4); blocks live in chunks saved at checkpoints
([ADR-0018](0018-a-checkpoints-chunks-written-together.md)), and a tick's log
carries its block changes ([ADR-0008](0008-durable-regions-and-resuming.md)).
Effects cross regions only as a player's actions, through an edge's outbox.

This record decides how vanilla's updates, scheduled ticks, post-processing and random
ticks run in a region; which chunks tick; how ticks are kept durable through commits,
checkpoints, restores, merges and splits without any region's tick numbers in what is
stored; what happens at a region's border; and when a chunk generation left marks in
is first shown.

## Decision

### 1. What vanilla does, which is followed

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

### 2. Which chunks a region ticks, and the ring it loads

A region **ticks a chunk** when the chunk and its eight neighbours are loaded in the
region: scheduled ticks, fluid ticks and random ticks run only there, and a chunk with
marks is post-processed (section 6) as it first becomes so. Every read of a tick then stays
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

### 3. Time: delays left, which wait while a chunk does not tick

Nothing durable counts in a region's tick numbers, which differ from region to region
and do not survive merges and splits. A chunk's ticks are kept in its file, and in what
is committed of it, as **the delay left** from the moment they were written. A region
that starts ticking a chunk (after a load, a merge, a split, a restore) makes each due
at its own tick plus the delay. So a tick waits while no region ticks its chunk:
vanilla's waiting of unloaded ticks, carried a little further (named: in vanilla a
loaded chunk that does not tick still ages). After a crash a tick runs at most its own
delay later than it would have, since a chunk whose ticks change is committed within
that time (section 4).

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

### 4. What is durable, and how

Today a tick commits only its block changes, and a restore replays them on the stored
chunks; a chunk is unsaved only by a block change. Phase B commits, besides, for each
chunk where they changed in the tick:

- its **ticks**, whole, as delays left from this tick, in queue order;
- that its **marks** were taken (section 6);
- its **seam entries** (section 5), whole.

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
orders by load (section 3), as vanilla's does; one test shows that order.

### 5. At a region's border

A chunk beside another region's chunk does not tick (section 2), so water stops a chunk
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
  and settled when one region has the 3×3 (section 6).

The owner chose this for now (question 1); carrying updates and ticks between regions
is a record of its own after B.

### 6. Generation's marks and ticks, and when a chunk is first shown

A chunk that **has marks** is post-processed when it first ticks in a region, by
vanilla's rule of section 1: fluids' ticks run, liquid blocks tick, other marked blocks take
their shape without neighbour updates (flags as SteelMC-26.3's, with
`UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`); its marks are then cleared, committed and the
chunk is unsaved. The test is the marks themselves, not `Chunk::generated()`: `set`
clears that flag, and a chunk post-processed and then lost in a crash comes back
changed and still marked. Generation's ticks start with the chunk as ticks loaded
with it (section 3).

**The first snapshot.** Only a chunk that still has marks waits before its snapshot
goes to an edge, and only while a missing neighbour is one the region holds and is
loading, or has asked the store for; a neighbour that is another region's, unreadable
or being generated lets it go as it is, settled later. The runner's hold of an edge's
inputs (`link.hold`) treats a chunk waiting so as it treats one being generated, so a
resume or a hand-over never waits on a ring's generation. The world store's spawn area
(`SPAWN_AREA`, 8) grows to the view distance and two, so that a first join does not
wait on two rings of generation.

### 6a. The order within a region's tick

After inputs and remote actions, as now: the seam entries of chunks joined this tick;
the post-processing of chunks that start ticking this tick, in ascending position;
the due block ticks; the due fluid ticks; the random ticks of ticking chunks, in
ascending position. Then the chunks are settled and the delta taken, as now.

### 7. Rules shared with generation

`updateShape` and `canSurvive` exist for generation in `clustine-features`, and
survival again in the sim. They move into one crate, `clustine-blocks`
(deterministic, no I/O), which the sim, the features, W6's placement and phase S's
templates use; generation's results stay bit-equal (the F sweep judges that).

### 8. Light

Some behaviour depends on light: ice and snow melt, crops and bamboo grow, grass and
mycelium spread, mobs spawn. The simulation has none (it is made at the edge). Those
are **left out** and named; leaf decay, sugar cane, cactus, kelp, vines and cave vines,
dripstone growth need no light and are in. **Question 3**: light in the simulation is
a step of its own after B.


## Consequences

- Phase B's steps (the plan's section 4) build this: one author for the updates, the
  ticks' durability and post-processing; rule ports for classes of blocks may go to
  subagents with tests from this record by someone else.
- What a player meets that this does not settle is named: effects wait at borders;
  falling blocks land at once; behaviour that needs light waits; redstone waits for a
  phase of its own; a ring chunk is not post-processed before an update from the rim
  reaches it.
- The world's chunk format gains seam entries and the log's commits gain per-chunk
  records; both read their previous forms.
- Commits, saves and the pause of a merge change with flowing water and random ticks;
  phase B measures them on the real disk before it is called done.
