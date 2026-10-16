# Phase E: effects between regions, planned

Draft of 2026-10-16, revised the same day after two independent reviews (sections 8 and 9); to go to the owner with its questions, and to be reviewed again as ADR-0025. The record the owner asked for on
2026-10-15 (the answers to the [B plan](b-plan.md)'s first question): after phase B,
updates and ticks are to be carried between regions, so that a border is seamless.
Redstone (a phase of its own) comes after it and builds on it. Its decision record is
written from this plan once the plan is agreed.

## 1. Why, and what a player meets

Since phase B a chunk ticks only where it and its eight neighbours are loaded in its
region, and an update whose target lies in another region's land is kept as a seam
entry in the chunk it came from until one region holds both sides
([ADR-0024](../adr/0024-block-behaviour-in-a-region.md), section 5). A player meets that:

- **at a pinned border** (`--pin`), for good: water stops a chunk short of the border
  on both sides; sand over a hole across it stays up; a torch whose block was broken
  across it stays; leaves across it never decay; fire never spreads over it; the
  chunks along it take no random ticks, so crops and kelp there never grow. A bucket
  emptied across the border does nothing (roadmap, known limits);
- **after a hand-over** between regions that follow players: the region the player
  left keeps its land until it is absorbed, 30 seconds and more after it empties
  (ADR-0016's `EMPTY_FOR`), and in that time the same holds along the line between
  the two;
- rarely, where claims meet at a rim when the distances were set by hand below what
  the view needs (ADR-0017).

With regions that follow players a border is out of everyone's view as a rule, so the
first case is the one a player meets within minutes, and the owner uses pinned
borders on purpose to try one.

## 2. Where things stand

- **No worker sends anything to another.** A player's action across a border goes
  through the player's edge as a remote action (`Durable::Remote`, ADR-0012 sections
  2.3 and 2.4), a tick or two later, and may take effect in the other order than two
  actions in one region would (ADR-0006, consequences; ADR-0014, the dig and the place
  of one block).
- **Each region has its own tick clock**, paced on its own (architecture.md: "An
  overloaded region slows down alone"); tick numbers differ from region to region and
  do not survive merges and splits (ADR-0024, section 4). Synchronised ticks were
  rejected (ADR-0003, halo exchange).
- **A region sees nothing of another's land**: a block outside what it has loaded
  reads as air (`RegionView`); only the edge holds both sides, for display.
- **The store** orders what matters across regions today: the table of chunks (a claim
  of a chunk another region holds is answered `Foreign`), and the records of merges
  and splits. Commits are durable before anything is shown (ADR-0008); epochs fence a
  former owner at the store.
- ADR-0006 rejected regions keeping copies of their neighbours' border chunks as "far
  more than a boundary that is going to disappear is worth". With pinned regions a
  boundary does not disappear, and phase B gave a border far more to carry.

## 3. What "seamless" can mean here

In one region an update chain, a fluid's spread and a scheduled tick run in one order,
in one tick. Across two regions with clocks of their own, nothing can make the result
**equal** to that without synchronising the two ticks, which the project rejected. What
can be had:

1. **Nothing waits for good.** Every update, tick and spread that would cross the
   border happens, a short time later, in the region that holds its target.
2. **Each region stays deterministic**: a tick is a function of the region and its
   inputs; what comes from across is an input, numbered and ordered, like a remote
   action.
3. **Late, never early, never twice.** An effect from across runs one to a few ticks
   later than it would in one region (the time to commit and to deliver), never before
   its cause is durable, and exactly once, also over crashes, merges, splits and moves.
4. **Reads across see the other side as it was a moment ago, and never older than
   the effects that arrive with it.** A fluid deciding where to flow, a torch asking
   whether its support is there, reads a copy of the neighbour's border, one to a few
   ticks old, brought up to date before any effect of the same or a later commit is
   run (section 4.5).

What differs from one region, then, is timing along the line: water crosses it a tick
or three late, two chains that meet at the border may interleave otherwise, and a
chain that zigzags across the line (a row of fences along it, leaves decaying across
it) takes a round trip for each crossing where the game takes one tick for all. The
plan takes that to be seamless for a player (a flowing front is five ticks a block, so
a tick or two late is not seen); question 1 asks the owner.

The game itself lets nothing happen in a chunk outside every player's simulation
distance, and drops an update into a chunk that is not loaded. Here an effect whose
target nobody has loaded **waits** for the chunk to be loaded, by whoever holds it then;
it forces no load (section 4.4).

## 4. The design proposed

### 4.1 Words

- **Effect**: what a region's tick would do in a chunk it does not have loaded (4.3),
  addressed to that chunk.
- **Border copy**: a read-only copy, in one region, of a chunk of another's land (4.2).
- **Sequence**: a number the store gives every durable group of commits, increasing
  for the life of the world (the log's position will do). Effects, copy updates and
  copies' bases carry the sequence of the group that made them durable.

### 4.2 Border copies

A region keeps a border copy of each chunk of another region's land, or of land nobody
holds, that lies in the 3x3 of a chunk it holds **with a ticket** (a viewer's, a
guest's, a ring's). A ring chunk's outer side is not loaded, so copies beside it buy
nothing and are not taken.

- A border copy is never ticked, post-processed, saved or shown to an edge from this
  region, and holds no ticket.
- **Its base** is the chunk's committed form: the store's saved chunk with its holder's
  commits since replayed into it, or the generator's chunk where none was ever made. It
  is made on the store's thread for chunks, from the list of log entries the commit
  thread hands it, as a restore's chunks are; the commit thread only lists (second
  review, finding 7). The base is cut at the end of a group and carries that group's
  sequence.
- **Copy updates** are the block changes, ticks and marks of each later commit in that
  chunk, with their sequence. The worker keeps updates that arrive before their base
  and applies those above the base's sequence once it comes, so a base on a busy chunk
  is never overtaken into a gap (finding 2).
- The worker asks for a base again only on a gap in a chunk's updates or after its own
  restart. A merge, split, move or hello of the **holder** changes nothing for the
  copy, which is of committed forms and keeps its sequence (finding 10).
- A copy of land nobody holds is the generator's chunk, which no commit changes until
  someone claims the chunk; from then its updates come as from any holder.
- A merge drops the absorber's copies of the absorbed region's land, which it now
  holds; a split asks for copies along its new line. Copies go when no ticketed chunk's
  3x3 reaches them.

### 4.3 Effects out

Whatever a tick would do in a chunk the region **does not have loaded**, its own or
another's or nobody's, becomes an effect. The sim makes them at every place it writes
or schedules: `set_block_updated` (which today does nothing for such a chunk),
`schedule_tick`, the updates' `run_one` and `shape_update`, the fluids' spread. Reads go
through one path that knows loaded chunks and copies; each of the 23 direct lookups of
`self.chunks` the first review counted goes through it or is shown not to need it.

The effect carries the **call**, not its result, wherever the target's side decides:

| In one region | The effect | What the holder does |
|---|---|---|
| `neighborChanged`, `updateShape` | target, direction, kind, source block, flags, depth left | runs the update by its own rules |
| `scheduleTick` | position, block or fluid, delay, priority | schedules it, the delay counted from the tick it takes it in |
| a fluid spreading (`FlowingFluid.spreadTo`) | target, direction, the source's fluid state | runs the whole target side on its own blocks: `canSpreadTo` (`canBeReplacedWith`, `canPassThroughWall`, `canHoldFluid`) and `getNewLiquid` for the target, reading its copy of the source's side; then `beforeDestroyingBlock`, the set and its updates. A wall a player put in the water's way stands; a source the copy still shows but a player took is seen gone in the holder's own copy if newer, and otherwise the next update corrects it (second review, finding 4) |
| a rail turned towards a placed rail (`rail_connect_to`), a chorus flower or vine grown into a block | target, the expected block, the state | sets it only if the target is still the expected block. The placing side decided on its copy, as the game decides on what it sees; where the target changed meanwhile, the placed rail stays curved towards a rail that does not answer, which nothing in the game reshapes. Named in the record as a rare consequence of section 3 (finding 5) |
| any other set by a rule (fire spreading, a block removed) | target, expected block, state | the same conditional set, then that change's updates |

A falling block never crosses: its fall is one column.

**Deduplication**, only among effects not yet sent (finding 8): one update per
position, direction and kind (the latest), one scheduled tick per position and block
(the earlier), one spread per position and direction. What waits is bounded by the
positions along a chunk's edge.

### 4.4 The store keeps effects per chunk

- A commit carries its effects. When its group is durable the store files each under
  its target chunk, with a number per chunk and the group's sequence.
- The store sends a chunk's effects, **unsolicited**, to its holder while the holder
  **uses** it: from the answer to its load or claim (not from the request, and not an
  answer `Unreadable`), or from a new request **`InUse`** for a chunk the runner takes
  from its warm copies after a merge or split, until its return or a new request
  **`Dropped`** for a held chunk no longer loaded (second review, findings 3 and 11).
- A load's, claim's or `InUse`'s answer carries the chunk's pending effects as of a
  sequence; effects sent live with a sequence at or below it are duplicates and dropped
  by the worker.
- **Taken as a set** (finding 1): the holder's commit lists, per chunk, the numbers of
  the effects it took in that tick. Every effect handed to a tick is taken in it:
  either run, or, where it cannot run yet (its target's six neighbours are not all
  loaded or copied), kept by the holder as a **seam entry in the target chunk**, which
  the holder has loaded, so that the commit makes it durable as phase B's seam entries
  are. Nothing waits at the head of a line, and nothing comes back. A tick that takes
  effects commits even if nothing else changed.
- The worker ignores an effect for a chunk it no longer uses; the store keeps it, and
  hands it to whoever uses the chunk next. A chunk returned and claimed by another,
  or by the sender, goes with its effects; a chunk nobody holds keeps them until
  somebody uses it. Resends after a hello, a merge or a split are of the effects of
  chunks in use, and dropped by number where already taken.
- **Durable in the log with a retention reason of their own** (findings 6 of both
  reviews): an effect is a record of the commit that made it, a taking is a record of
  the commit that took it. Collection keeps a segment while it holds an untaken effect.
  To free such a segment the store first writes its untaken effects forward, as records
  of a group of their own, **synced**, and only then removes the segment; a failure of
  that group cuts it off like any other. Records keep their numbers; after a restart
  the per-chunk counters continue above every number still in the log. A make-over
  (another division) re-files the effects of the commits it passes over before writing
  its `Opened` records. A group cut off takes its effects and takings with it, undone
  in memory as `players.undo` undoes stays.

### 4.5 Effects and copy updates in

- The worker keeps what arrives, per chunk, until the tick can use it: effects for a
  chunk being loaded until the load's answer, copy updates until their base.
- **An effect runs only when every copy its target reads is at or past the effect's
  sequence** (finding 2): its target's chunk and the chunks of its six neighbours, and
  for a spread the source's side. A copy not yet based counts as older. Until then the
  effect waits in the worker, untaken. So a torch whose support was broken in the same
  commit reads it broken.
- In the tick, copy updates are applied first, then effects, **where seam entries run**
  in ADR-0024's order (after the inputs and `update_ticking`, with the seam entries,
  ordered by chunk and number). Section 6a of that record is amended so; a scheduled
  tick taken into a chunk that starts ticking in the same tick counts its delay from
  that tick (finding 9).

### 4.6 What changes where

- `clustine-sim`: `Knowledge` gains a border copy; one read path; effects out;
  copies, copy updates and effects in, with their sequences; the rule of 4.5; received
  effects that cannot run kept as seam entries in their target chunk; restore and its
  tests take copies. Deterministic as before.
- `clustine-rpc`: `Commit` gains effects and takings; answers to loads, claims and
  `InUse` gain pending effects with a sequence; unsolicited effects, copy bases and copy
  updates with sequences; requests for copies, `InUse` and `Dropped`.
- `services/worldstore`: pending effects per chunk, numbered, with sequences; routing
  on the commit thread by the table and by who uses a chunk; bases on the thread for
  chunks from listed entries; the retention reason, writing forward with a sync, the
  make-over's re-filing, undo of a cut group.
- `services/worker`: copies its tickets' 3x3 needs; keeping what arrives until it can
  be used; `InUse` and `Dropped`; commits for takings.
- ADR-0024: section 5 gains effects beside seam entries; section 6's first snapshot
  waits for a copy on its way as for a neighbour being loaded; section 6a as above;
  section 4's saving of ticks pending counts chunks that now tick beside copies. The
  edge: nothing.

## 5. Steps

| Step | What | Judged by |
|---|---|---|
| E1 | The record (ADR-0025) from this plan, reviewed | An independent review |
| E2 | Shared types: effects, sequences, copy bases and updates, the commit's, answers' and replies' new fields, `InUse`, `Dropped`, the log's new kinds | Format round-trip tests |
| E3 | The store: effects per chunk, routing by use, retention and writing forward, the make-over, undo, bases from listed entries | Store tests with crashes at every point (the simulated disk) |
| E4 | `clustine-sim`: one read path, effects out, copies and effects in by sequence | Two regions stepped side by side through a fake store, against one region over the same land for the settled result: water across a line both ways and walled off, a torch whose support is broken across, rails, fences, leaves, fire |
| E5 | The worker | Worker tests; a pinned border end to end, with a crash on either side, a merge across it and warm chunks |
| E6 | Tests from the record by someone who did not write the code | |
| E7 | Measured: the store's load along a busy pinned border (bases, routing, writing forward), a merge's pause, a crash beside a border | On the real disk |

E3 and E5 are where ordering mistakes hide; they are not delegated. E2's types are
fixed before anything else.

## 6. Questions for the owner

1. **Seamless as section 3 says it**: effects cross a border a tick or a few late, may
   interleave otherwise where two chains meet on the line, and a chain that zigzags
   across it takes a round trip per crossing. Rarely, a rail placed at the line just as
   its neighbour across changes stays half joined. Recommended: yes; exact equality
   needs ticks synchronised across regions, which ADR-0003 rejected.
2. **Everything of phase B crosses**: updates, scheduled ticks, fluids, rails, fire,
   and random ticks along the line. **Redstone** would cross the same way, with a limit
   to know now: wire strength settles across a line as water does, but a circuit that
   counts ticks (a clock, repeaters timed against each other, observers in a chain)
   and straddles a line runs late on the far side and may not work as in one region.
   Recommended: yes, and redstone keeps that limit.
3. **All borders, not only pinned ones.** Recommended: yes.
4. **Through the store**, not directly between workers. Recommended: yes.
5. **An effect into land nobody uses waits** in the store until somebody uses the
   chunk, and forces no load; the game drops such updates. It costs the store memory
   and the writing forward of what waits, a little for every chunk at the edge of where
   players have been. Recommended: wait, and measure the cost in E7.
6. **Copies of the neighbours' border chunks**, which ADR-0006 rejected as more than a
   boundary that goes away is worth. Pinned borders do not go away, and phase B gave a
   border more to carry. Recommended: take them, one chunk deep beside chunks with
   tickets, and only of committed forms.

## 7. Risks

| Risk | Answer |
|---|---|
| The store's load along a busy pinned border (a river across the line) | E7 measures routing, bases and writing forward; effects deduplicated per target |
| A busy holder's chunk makes bases costly | Listed on the commit thread, made on the thread for chunks; asked for again only on a gap or a restart |
| Effects for land nobody uses keep log segments alive | Written forward, bounded per chunk by deduplication |
| An effect lost, taken twice or run against an old copy | Numbered per chunk, taken as a set in the holder's commit, dropped by number; runs only when its copies are at its sequence |
| More chunks tick with ticks pending, so more are saved at a stop | Measured again in E7 (B8: 20 to 40 a stop) |
| A chunk that stops ticking when its copy goes writes its ticks into itself and takes them back ahead of newer ones | Named in the record; the same as any chunk that stops and starts |

## 8. The first review (2026-10-16), and what changed

1. Effects in the log were collectable once their sender checkpointed: a retention
   reason of their own, and writing forward (4.4).
2. Delivery with the answers to commits missed regions that do not commit and was
   dropped across reshapes: unsolicited, resent after hellos and reshapes (4.4).
3. Inboxes per receiving region lost effects whose chunk was not in use or changed
   holder: kept per target chunk, going with the chunk (4.4).
4. A copy update and an effect of one commit could run in the wrong order: sequences,
   and an effect waits for its copies (4.5).
5. The store had no current form to copy: bases as a restore's chunks (4.2).
6. A conditional set broke rails; falling blocks never cross (4.3).
7. Effect or seam entry depended on beliefs the sim drops: every write to a chunk not
   loaded is an effect (4.3).
8. Taking must commit; per-sender marks did not merge: per chunk, committed (4.4).
9. The sim's read and write paths named (4.3).
10. Effects would make a holder load land nobody needs: they wait (question 5).
11. A chunk shown unsettled until its copy comes: the first snapshot waits for a copy
    on its way (4.6).
12. Reshapes and copies: merges drop and splits take copies (4.2); ticks reordered
    after a stop are named (section 7).
13. Copies beside ring chunks bought nothing: only beside chunks with tickets (4.2).
14. Unbounded growth: deduplication per target (4.3); copies' updates kept per chunk
    in the worker only until their base.
15. Chains zigzagging across the line: named in section 3 and question 1.

## 9. The second review (2026-10-16), and what changed

1. Taking up to a number stalled on an effect that could not run, or came back every
   tick: taken as a set, kept as a seam entry in the target chunk where it cannot run
   (4.4).
2. There was no one stream in commit order across the store's two threads: sequences,
   the worker keeps what arrives early, an effect runs only when its copies are at its
   sequence (4.2, 4.5).
3. Warm chunks bypassed the store: `InUse` (4.4).
4. Only the target's `canBeReplacedWith` let late water eat a wall: the whole target
   side, `canSpreadTo` and `getNewLiquid` (4.3).
5. The rail's answer could not be built: decided on the copy as the game decides, the
   rare half-joined rail named (4.3, question 1).
6. Writing forward was not durable: a synced group of its own before a segment goes;
   numbers kept; the make-over re-files (4.4).
7. Bases replayed on the commit thread: listed there, made on the thread for chunks
   (4.2).
8. Deduplication against effects already sent lost ticks: only among unsent (4.3).
9. The order against ADR-0024 6a: effects run where seam entries run (4.5).
10. Rebasing on the holder's reshapes was needless: only on a gap or a restart (4.2).
11. Copies of land nobody holds, claims and unreadable loads, effects for chunks no
    longer used: 4.2, 4.4.
12. Question 5 reworded; question 6 added; the first review's findings numbered.
