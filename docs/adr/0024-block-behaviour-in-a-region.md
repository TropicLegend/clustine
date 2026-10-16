# ADR-0024: Block behaviour in a region

- Status: Proposed on 2026-10-15, from the plan of phase B
  ([b-plan.md](../groundwork/b-plan.md), section 3), after two independent reviews of
  that plan whose findings are worked in (the plan's sections 8 and 9). A third review
  of this record found it close, and a fourth, focused one accepted it after the
  changes it lists; both are worked in (see "Review"). **Accepted** on 2026-10-15; not
  built. Where the plan's section 3 and this record differ, this record holds. The owner's answers to the plan's questions are in it:
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

A region loads, besides the chunks it has tickets for, **one ring around them**, by a
ticket of a kind of its own (a ring ticket), never sent to an edge for it. As every
ticket in ADR-0012's model, a ring ticket is an **input** of the tick: the runner
works the ring out from its links' subscriptions (around the chunks of both viewers'
and guests' tickets that the region holds, and around nothing it does not hold) and
hands it in as a ticket of one of **two ring kinds**, `Ticket::ViewerRing` around a
viewer's chunk and `Ticket::GuestRing` around a guest's; a chunk that has a ticket of
its own is in no ring. The runner works the rings out again after any tick in which
its subscriptions or the region's holdings changed (`Region::knowledge` is public). A
chunk in both kinds of ring has the viewer's; when its kind changes the runner adds
the new ticket before it lets the old go, as `subscribe` does. It lets a ring ticket go
through `release`, which saves the chunk before the tick that drops it, as for any
ticket, and `release` saves only when no ticket of any kind remains. So a ring
chunk's changes and pending ticks are saved like any other's, and the restore tests
can hand rings in.

Each ring kind is **wanted and claims exactly as its base kind**: a viewer's ring as a
viewer's ticket, a guest's ring as a guest's ticket, only in a pinned area, so that no
region takes land for another region's players. Both count as a viewer's ticket does
in accepting a load (`chunks_loaded`) and in what is used (`settle_chunks`, so that a
ring chunk is not returned and claimed again and again). From what a region knows of a
chunk (ADR-0012): `Unknown` is claimed where the kind claims, `Held` loaded, `Foreign`
passed over, `Asked` waited for. Saves go through `Region::saved_chunk` (section 4),
never the live chunk.
The view's rim then ticks and is post-processed before it is sent, as vanilla's is.
Vanilla also post-processes the ring itself; Clustine does not, so an update from the
rim can reach a ring chunk whose marks later run against it: named.

Two regions' rings meet before their views do. The coordinator's merge distance
(`D_m = 2(V+1)+4`, `services/coordinator/src/policy.rs:48`) leaves four chunks for an
approach; it grows by two, so that a rim in someone's view does not stop ticking during
an ordinary approach. With it change: the doc of `Policy::for_view_distance`; what
`say_how_it_reshapes` needs (`2V+3` becomes `2V+5`,
`bin/clustine/src/cluster/coordinator.rs`); the tests that derive from the defaults
(`bin/clustine/src/main.rs`, `single.rs`, `crowds.rs`, `follows.rs`); the roadmap's
figures and ADR-0016's and ADR-0017's reasoning. The split distance stays the merge
distance and eight.

### 3. Time: delays left, which wait while a chunk does not tick

Nothing durable counts in a region's tick numbers, which differ from region to region
and do not survive merges and splits. A chunk's ticks are kept in its file, and in what
is committed of it, as **the delay left**. A tick's due is vanilla's trigger time: the
tick it was scheduled in plus its delay; due ticks are collected while their due is at
most the current tick. Written at the end of tick T, a tick due in D is kept with the
value D − T − 1, which is −1 for a tick of delay 0 scheduled in T (the field is an
`i32`). A region that starts ticking a chunk in tick T′ (after a load, a merge, a
split, a restore, or when a missing neighbour comes back) makes each due in
T′ + value, so a chunk that ticks on without a break keeps every due, and ticks of
delays 0 and 1 scheduled in one tick keep vanilla's order by due before priority. A
tick scheduled with delay d into a loaded chunk that does not tick is kept as d − 1,
as if written at the end of the tick it was scheduled in, until the chunk ticks; a chunk that stops ticking (its neighbour let go, a merge, a split, a
release) keeps its delays from then. So a tick waits while no region ticks its chunk:
vanilla's waiting of unloaded ticks, carried a little further (named: in vanilla a
loaded chunk that does not tick still ages). A tick never runs early. After a crash
it runs late by less than its own delay, since a chunk whose ticks change is committed
within that time (section 4), and by the time the restored region takes to load the
chunk and its ring.

There are **two queues**, block ticks and fluid ticks, as in vanilla: each tick all due
block ticks run (at most 65,536), then all due fluid ticks (as many). Within a queue,
ticks run in (due, priority, order). A tick scheduled by one of the tick's runs is put
in the queue for later ticks: the due ticks are collected before any runs (vanilla's
`collectTicks`), and a tick of delay 0 scheduled meanwhile runs in the next tick.
Within a queue the comparison is (due, priority, class, order): class puts ticks
**stored** (that came with a chunk as it started ticking) before ticks **scheduled**
since, as vanilla's negative orders for a loaded chunk's ticks do. Each class has its
own counter in `RegionState`: a scheduled tick takes the scheduling counter as it is
scheduled; a stored tick takes the loading counter as its chunk starts ticking, in the
order of the chunks starting (ascending positions within a tick) and of each chunk's
list. A split's part inherits both counters, a merge takes the larger of each.

**Random ticks** need no state: a section's positions are drawn from a stream seeded
by the world's seed, the chunk, the section and the region's tick.

### 4. What is durable, and how

Today a tick commits only its block changes, and a restore replays them on the stored
chunks; a chunk is unsaved only by a block change. Phase B commits, besides, for each
chunk where they changed in the tick:

- its **ticks**, whole, as delays left from this tick, in queue order (a list that
  became empty is committed as empty);
- that its **marks** were taken (section 6);
- its **seam entries** (section 5), whole.

`TickOutput` names the chunks and what changed. A chunk with any of them is unsaved.
A chunk that only **stops ticking** (a merge, a split, a release) is unsaved only if it
has ticks pending, which are then written into it with their delays; a chunk with none
is as it was. Saving a chunk (checkpoint, release) writes its ticks into it the same
way. The runner cannot see that by itself, since its unsaved chunks come from changes:
`TickOutput` names the chunks that **tick with ticks pending**, and the runner treats
them as unsaved wherever a stop is near, in `release` and in the test of a merge's few
unsaved chunks (`FEW_UNSAVED`, which then counts every chunk that ticks with ticks
pending and was not saved in this tick). The stop's own checkpoint saves them too:
each is a chunk file (a round of eight files took 77 ms on the 22 ms disk). B8
measures a merge's pause with about thirty such chunks on the real disk without its
cache, and says which disk each number is from. **The warm copies** a merge or split hands over, which stand for what the
store has, are such saved forms. `Region::saved_chunk(position)` gives that form, and is
what the restore tests load.

**The store** carries the records through: a log record of a new kind, a commit with
per-chunk records and with the stays a commit may carry, is written only when a tick
has records, so that a commit without any is written as before; a tick whose only
change is to ticks, marks or seams is committed (the runner's `needs_commit` counts
them); the replay jobs (`Job::Restore`, and the fold of a change of
division at start-up, `Fold` and `make_over`, through `touched`, `apply` and
`replayed`) carry them with the block changes and applies them under the same rules of who held
the chunk; a chunk that only these records touched is touched. A chunk the store holds
only as generated is made and then has them applied, as with block changes (its
generation's ticks are part of the deterministic output). A commit of ticks, marks or
seams clears the chunk's `generated` flag, so that the remaking of a chunk whose
generation failed (`remakes`) does not lose them.

**The format** keeps its version (2), since the decoders and the store's meta refuse
any other: a chunk's seam entries are behind a bit of its flags that older chunks do
not set, and the commit with records is a log kind of its own that older logs do not
have. Ticks keep their present form (delays and priority, in queue order). So phase B
refuses no world; phase S's switch does, once.

**The restore tests.** A load is accepted only a tick after it is asked for, so a
restored region cannot be given its chunks in its first tick by the ordinary way.
`Region::restore` takes, beside the state, the tickets and the chunks (in their saved
forms) the region starts with, loaded as it starts; the tests restore so, and the
warm copies of a merge or split may too. The copy and the original then tick in step,
and equality holds but for ties of due and priority between ticks of different
chunks, which a reload orders by load (section 3), as vanilla's does; one test shows
that order. Rings change the claims, requests and loads that today's tests of the sim
and the worker expect; those tests change with B4, and the change is named there.

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
loading, has asked the store for, or is being generated, as vanilla's waits; a
neighbour that is another region's or unreadable lets it go as it is, settled later.
So new land is shown settled, as the official server shows it. The runner's hold of an
edge's inputs (`link.hold`) treats a chunk waiting so as it treats one being generated,
so a resume or a hand-over never waits on a ring's generation. The world store's spawn
area (`SPAWN_AREA`, 8) becomes the server's view distance and two (about 150 chunks
more made at every start at a view distance of 8), so that a first join does not wait
on two rings of generation. The first view's time is measured again with B (T8's
measurement), since a ring more is made.

### 6a. The order within a region's tick

After inputs and remote actions, as now: the seam entries of chunks joined this tick;
the due block ticks; the due fluid ticks; the post-processing of chunks that start
ticking this tick, in ascending position (vanilla runs its scheduled ticks before the
chunk source's tick, in which chunks become ticking); the random ticks of ticking
chunks, in ascending position. Then the chunks are settled and the delta taken, as now.

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

## Review

The third independent review (2026-10-15) found the time model of section 3 sound,
and:

| # | Finding | Answer |
|---|---|---|
| 1 | A ring ticket made inside the region went around the worker's save before a drop: ring chunks' changes would be lost | Ring tickets are inputs from the runner, let go through `release`, which saves (section 2) |
| 2 | Rings around guests' tickets unsaid; a ring that claims everywhere takes land for others | Rings around both, only around held chunks; a guest's ring claims as guests do (section 2) |
| 3 | The restore tests' rule could not be followed: a load takes two ticks | `Region::restore` takes tickets and chunks; the tests' churn named (section 4) |
| 4 | The format version is shared and refused when different | Version 2 kept; a flag bit and a new log kind (section 4) |
| 5 | The start-up fold replays commits too | Named with the other replay jobs (section 4) |
| 6 | New land would be shown unsettled while its ring is generated | A chunk with marks waits for neighbours being generated too; `SPAWN_AREA` from the view distance; the first view measured again (section 6) |
| 7 | `FEW_UNSAVED` does not count chunks made unsaved by stopping | Counted (section 4) |
| 8 | The delay's off-by-one, the bound, non-ticking chunks, empty lists | Said exactly (sections 3, 4) |
| 9 | What depends on the merge distance | Listed (section 2) |
| 10 | Post-processing's place relative to scheduled ticks; fire, farmland and coral not named | Post-processing after the scheduled ticks (section 6a); fire, farmland drying and coral dying are light-independent and go into B5 and B7 |

A fourth, focused review of sections 2 to 4 (2026-10-15) accepted the record after
these changes, with no further review needed: two ring kinds that are wanted and claim
as their base kinds (section 2); the chunks that tick with ticks pending named in the
tick's output and treated as unsaved near a stop (section 4); a class and a counter
each for stored and scheduled ticks (section 3); due as vanilla's trigger time, with a
kept value of −1 possible (section 3); the stop's own saves named and to be measured;
the new log kind carrying stays, and ticks with only such changes committed; no save
at a return; `release` saving only when no ticket remains; rings worked out again when
holdings change.

## Found while building

- The order of scheduled ticks needs no counter in `RegionState`: a tick's order is its
  tick number shifted left by twenty bits with a count from zero, which starts again at
  the start of each tick, so orders never collide and nothing more is saved.
- Rings are a setting of the region runner (`RegionSettings.rings`), on in the server
  and off in the tests of ADR-0012's tickets, whose expected loads and saves predate
  rings; the tests of rings turn them on.
- A test written from this record found that the shared rules had no survival rule for
  farmland, which therefore turned to dirt at every tick; it now has
  `FarmlandBlock.canSurvive`'s. Placing farmland where something solid is above it is
  refused, where the game places dirt; only a player in creative can try it.
- A commit with ticks makes the chunk no longer as generated in the store's replays,
  as a commit with blocks does.
- Counting the chunks whose ticks age in `FEW_UNSAVED`, as section 4 has it, adds a
  checkpoint and a wait for its flush to a merge of a region with more than eight such
  chunks, and does not shorten the stop: they age again in every tick, and only the
  stop's checkpoint saves them. B8 measures a merge's pause with and without the count
  and keeps it only if it pays.
- The store's spawn area is the edges' default view distance and two, not the
  server's: the store is not told the view distance. A server with a longer view
  makes the rest of its first view as a first player asks for it.
- A chunk waiting for its post-processing is held back from an edge only while a
  neighbour is on its way that would let it tick: held and being loaded or made,
  being claimed, or not yet claimed and wanted as a viewer's. A guest's ring outside
  a pinned area never claims, and is not waited for.
