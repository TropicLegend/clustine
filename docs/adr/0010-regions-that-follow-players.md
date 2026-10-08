# ADR-0010: Regions that follow players

- Status: **Accepted** as the plan for milestone M3, phase C; revised after an independent
  review (see the end). Being built. How the world store does its part is
  [ADR-0011](0011-the-world-store-and-regions.md), which changes some of what is said
  here about the store and lists that under "Changes to ADR-0010": above all, the tick
  of a grant is the store's and a claim carries none, the merge names the epoch the
  absorbed region was opened with, the new region of a split is opened with an
  ordinary hello, and a pinned region claims the chunks of its areas like any other.
  How the tick and the region runner work on chunk sets is
  [ADR-0012](0012-the-tick-on-chunks.md), with its own list of changes to this record:
  above all, a guest's ticket keeps a chunk from being given back, a region gives a
  chunk back only after a while without use, what a player does to a chunk the region
  is not sure of is routed by the edge, and answers about chunks carry the number of
  the asking they answer.
- Date: 2026-10-08

## Context

Until now the world is cut into fixed stripes along the x axis
([ADR-0006](0006-static-regions-and-handoff.md)), one region each, set when the cluster
is started. That shows what M2 and the first two phases of M3 were to show, but it is
not how load is spread: a hundred players in one stripe are one worker's, however many
workers there are, and a boundary runs where it was drawn, also through a village.

Phases A and B made a region something that can be rebuilt from the world store by any
worker ([ADR-0008](0008-durable-regions-and-resuming.md)) and moved on purpose
([ADR-0009](0009-moving-a-region.md)). Phase C makes regions what the architecture
meant them to be: **sets of chunks around players who are near each other**, which come
into being, grow, merge, split and go as players move.

## Decision

### 1. What a region is

A region has an id that the world store gives it and that is never used again, the
**chunks it holds**, and a `RegionState` as before: the players standing in its chunks,
and what it keeps for each edge.

- **The world store says which region holds a chunk**, in a table of grants that is on
  disk. A region asks for chunks (`Claim`); each is granted if nobody holds it, and
  otherwise the store says who does. A claim is on disk before it is answered, with the
  commits that are written at that moment. Each grant notes the tick of the holder at
  which it was made.
- **A chunk leaves a region only saved.** Before a region gives a chunk back
  (`Return`), and before a split or a merge takes chunks from one region to another,
  every change the region made to the chunk is in the stored chunk and made durable.
  When a region is opened, the block changes of its log are applied only to chunks it
  holds now, and only those of ticks after the grant: what the region did to a chunk
  it has since given away is in that chunk already, and has perhaps been built over.
- **Only the holder loads and saves a chunk**; the store refuses others.
- **A region needs a chunk** while one of its players stands in it or a viewer whose
  player is the region's has it in view. A subscription of a **guest** (section 3) is
  no need.
- **There is always a home region**, which holds the chunk players enter the world in
  and is where they join. A world begins with it alone. It is never absorbed. The home
  region gives out all entity ids, as only joining needs one so far; other regions
  have none to give.
- A region that nobody runs keeps its chunks.
- **No region just disappears.** A region ends only by being absorbed by another
  (section 4), so that what it kept for edges, and what edges keep for it, has
  somewhere to go.
- `Layout`, the stripes and `--boundaries` go at the end of the phase (section 9).

**Pinned regions**, for tests and for whoever wants a boundary at a known place: the
store can be given regions that hold every chunk of an area (as wide as a stripe was)
that is not granted otherwise, and the coordinator can be told to decide nothing by
itself. Tests of hand-over, of actions across a boundary and of chaos pin two regions
side by side; without that, boundaries are where nobody is, and what crosses them is
hardly ever tried.

### 2. The tick, on chunks instead of an area

The region knows of each chunk it has had to do with whether it **holds** it, whether
another region does (and which), or that it has asked and not heard. What it holds the
store says when the region is opened; what it believes of others it does not keep. New
among a tick's inputs: `granted` and `foreign: Vec<(ChunkPos, RegionId)>`, the store's
answers, and `unbelieve`, chunks whose holder it is to ask about again. New among its
outputs: `claims` and `returns`.

- A player is the region's while they stand in a chunk it holds **or has asked for and
  not heard about**. What they do is applied. A block action on a chunk the region
  does not hold or has not heard about is acknowledged without effect, as one on a
  chunk that is not loaded is today.
- When the answer is that another region holds the chunk a player stands in, the
  player is let go to it: `Departed { player, transfer, to }`. A player who walks into
  a chunk nobody holds takes it for their region: the region grows where its players
  go. A region claims the chunks around its players when they come into view, so the
  answer is there long before a player is.
- A block action on a chunk the region knows another to hold is passed on, as before,
  and names that region: `Remote { action, to }`.
- An arrival or a remote action for a chunk the region does not hold is answered with
  an outbox entry `NotMine { what, holder }`, with the holder if the region knows one.
- The removal of an entity that departed and that nobody will pass on (ADR-0008,
  section 4) is told to every link, as it was for a chunk beyond the region's area.

### 3. The edge, without a layout

- **The edge keeps, for each chunk it shows, the region it is subscribed at.**
- **A viewer's chunks are asked of the viewer's region.** For each the region serves it
  if it holds it or is granted it, and otherwise answers `Elsewhere { chunk, region }`;
  the edge then subscribes there **as a guest**. A region asked as a guest for a chunk
  it does not hold answers `NotMine { chunk }`. When a region stops holding a chunk
  (it gives it back, or it goes to another region in a split or a merge), it tells
  every link subscribed to it `Elsewhere` or `NotMine`. On `NotMine` the edge asks the
  viewer's region again, after a short while if it has just asked, and that region
  asks the store again rather than answer from what it believed.
- `Departed`, `Remote` and `NotMine` name regions. A region that has been absorbed
  stands for the one it went into; if that makes an entry's destination the region it
  came from, it is handled there like any other.
- A `NotMine` for an arrival or a remote action goes to the holder it names, or else
  back to the region that sent the item, which is told to ask again who holds the chunk
  (`unbelieve`).
- **The routing table lists the regions**, with the owner of each that has one, names
  the home region, and for a while also the regions that were absorbed and what they
  went into. The edge links to the regions it has to do with.
- A resume's hold (ADR-0008, section 4) ends for a chunk also when the answer for it
  is `Elsewhere` or `NotMine`.

**Knowing an edge again.** A region's `EdgeState` gets a number that says since when the
region has known the edge without a break (`since`): set when the state is made,
whether by a hello or by absorbing. A hello says the `since` the edge last heard from
that region. The welcome is `Resumed` only if both the start and that number agree, and
otherwise `Unknown { since }`, on which the edge resets what it kept for the region as
in ADR-0008 and takes the new number. So an edge is never taken to know a numbering it
does not, whichever way the region came by its state for the edge. After `Unknown` the
region sends the outbox it has for the edge all the same, from its first entry.

**Sending what was kept.** The welcome says how many outbox entries follow it. The edge
sends what it kept for the region when it has handled those, not at the welcome: an
entry among them can change what is kept (section 4).

### 4. Merging

When players of two regions come near each other (section 7), one region absorbs the
other, so that people who play together are simulated together. `A` survives: the home
region if it is one of the two, else the one with more players.

1. **The coordinator reserves both regions** for the merge: neither is moved, released,
   split, merged with a third or assigned while it lasts, and both count as vouched
   for. It gives up after the lease, counted from the start; `B` is then a region like
   any other again.
2. It has `B` released as in ADR-0009, with nobody as target: `B`'s state is in its
   state file, its chunks are saved, and it stays without an owner.
3. It tells `A`'s owner `Absorb { region: A, epoch, absorbed: B, as_epoch }`, with a new
   epoch for `B`. `A`'s worker opens `B` at the store with that epoch, which fences
   whoever ran `B`, and checkpoints it if its log is not empty (its owner died instead
   of releasing it).
4. `A` brings its own store up to date as for a release: a checkpoint while ticking,
   then it stops ticking, waits for its commits, publishes them, and saves what changed
   since. It does **not** close its links yet.
5. It works out the merged state **aside**, in a tick of its own in which nothing else
   happens:
   - `B`'s players come in. If `A` has a player already, `A`'s stays, and `B`'s entity
     of that player is reported removed if it is another.
   - For each edge either region knows: if they know it with different starts, the side
     with the lower start is reset first, as a higher start resets a region (ADR-0008,
     section 2). Then one outbox entry `Absorbed { region: B, knew, applied, numbers,
     players }` is added to `A`'s outbox for the edge, and behind it `B`'s outbox
     entries for the edge under new numbers. `knew` says whether `B` knew the edge;
     `applied` how far `B` had applied its messages; `numbers` are the numbers the
     entries behind had in `B`'s outbox, in order; `players` are the edge's players
     that came from `B`, each with what a presence answer has. If `A` did not know the
     edge, its `EdgeState` is new, with a new `since`.
6. It hands the store the result: `AbsorbCommit { absorbed: B, tick, state }`. The
   store refuses unless the session is `A`'s current one, the same worker's session on
   `B` is `B`'s current one, and `B`'s log is empty. Otherwise it writes **one record
   to the log**, which is the merge: `A`'s whole state as of `tick`, every chunk `B`
   held being `A`'s from that tick, and `B` retired into `A`. When that record is on
   disk the merge has happened, and not before; a region is restored from the latest
   whole state the log or its state file has. State files and the table of grants are
   brought in line afterwards and again when the store starts.
7. If the commit is refused or fails, `A` drops what it worked out, ticks on as it was,
   and tells the coordinator. If it is confirmed, `A` takes the merged state, closes
   its links, ticks on, and tells the coordinator.
8. Edges link to `A` again and resume. `B`'s players stand still from step 2, `A`'s
   from step 4.

**What the edge does with `Absorbed { region: B, .. }`**, wherever it meets it:

- If the hello of this link did not name what the edge had under `B` (it had not heard
  that `B` went into `A`), it ends the link and says hello again, naming it.
- Players it had under `B` are `A`'s. One that is among the entry's `players` is
  treated as a presence answer that says present; one that is not, as one that says
  absent. A player who is not `B`'s any more (they left, or came back through another
  region) is passed over.
- What it kept for `B` above `applied`, it keeps for `A` under new numbers behind what
  it kept for `A`, except what concerns a player who is not `B`'s any more. If `knew`
  is false, it keeps none of it and treats `B` as having forgotten it.
- Of the entries behind `Absorbed`, it passes over those whose number in `B`'s outbox
  is not above what it had seen from `B`. It keeps that list for the region until it
  has seen past it, also across links.
- Chunks it was subscribed to at `B` are asked of `A`.

An edge that resumes with a region that absorbed `B`, and has been sent no `Absorbed`
for it by the time the welcome's entries are through, treats `B` as having forgotten it.

### 5. Splitting

When the players of a region form groups that are far from each other (section 7), a
group is split off as a region of its own, which can then be moved to another worker.
The group that stays is the one in or nearest to the home chunk if the region is the
home region, and the largest otherwise.

1. The coordinator reserves the region and tells its owner `SplitOff { region: A,
   epoch, chunks, as_epoch }`: the chunks in which the group's players stood, as it was
   told, and an epoch for the new region.
2. `A` brings its store up to date as in step 4 of a merge.
3. It works out the part, aside, in a tick of its own:
   - its **players**: those standing in the chunks named or nearer to one of them than
     to any chunk with a player who stays; if there is none any more, the split is off;
   - its **chunks**: those `A` holds that are nearer to a player of the part than to
     one who stays, by the distance of section 7, ties staying; the home chunk and what
     surrounds it within a view stay with the home region;
   - the new region's state: those players as they are, whole; each edge that has one
     of them known with nothing applied and nothing sent, and a new `since`;
   - in `A`'s outbox, for each edge that has a player or a subscription in the part, an
     entry `SplitOff { region: N, applied, players, chunks }`: `applied` is how far `A`
     has applied the edge's messages.
4. It hands the store both: `SplitCommit { tick, state, part: { chunks, state },
   as_epoch }`. The store writes **one record to the log**, which is the split: a new
   region `N` with its state, `N` holding the part's chunks from that tick, `A`'s whole
   state, and `N` opened by this session with `as_epoch`. It answers with `N` and the
   handle.
5. `A`'s worker **runs `N` at once, from what it has in memory**, tells the links
   subscribed to chunks of the part `Elsewhere { chunk, region: N }`, closes `A`'s
   links, ticks both on, and tells the coordinator, which lists `N` with this worker as
   its owner. If the worker dies before it has told anyone, `N` is a region nobody
   runs, which the coordinator finds in the store's list.

**What the edge does with `SplitOff { region: N, applied, players, chunks }`**: the
players named are in `N` from then on, and for each it sends `N` what they did after
the last input the region had applied, as after a hand-over. For a named player it no
longer has, it sends `N` that they left. What it kept for `A` above `applied` that
concerns a named player or a chunk named, it sends to `N` as well. The chunks named it
asks `N` for.

### 6. Workers and the coordinator, with several regions each

- **A worker runs several regions**, each on a thread of its own, started and stopped
  while the worker runs; it tells links apart by the region their hello names, vouches
  for each region by itself, and on being told to stop asks for all of them to be
  moved.
- ADR-0009's rules, restated: a **target** is a worker that is registered, connected
  and not leaving, and the one with the fewest regions among those is taken; a worker
  that has released a region is as before. A reserved target is one that is being
  given a region just now, and counts as having it.
- **The coordinator learns the regions from the store**: a list of the regions that
  exist, with those absorbed and what they went into, and what each holds roughly (a
  bounding box). It reads the list when it starts, whenever a worker registers or
  reports a split or a merge, and every few seconds. Workers report at registration
  what they run and what they are in the middle of. It keeps nothing on disk: after
  starting anew in the middle of a merge or a split it finds either the old regions or
  the new ones, and a region nobody runs is assigned after its grace period.
- A merge or a split **ends with a message** from the worker saying what came of it,
  or with the reservation running out.

### 7. When to merge and when to split

- **Workers say where their regions' players are**: with each heartbeat, per region,
  the chunks that have players in them and how many.
- **Distance** is counted in chunks along the longer of the two axes, which is the
  shape of what a player sees.
- **Merge** two regions when a player of one is within the merge distance of a player
  of the other. **Split** a region when its players fall into groups any two of which
  are further apart than the split distance.
- The merge distance is twice the reach of the largest view distance the edges grant,
  plus what a player covers at the fastest in the time a heartbeat is old (a second,
  two chunks and more when flying), plus a margin. The split distance is larger by a
  further margin, so that a group at the rim does not flap. A region that was merged
  or split is left alone for some seconds. The coordinator is told the largest view
  distance.
- **A region without players**, other than the home region, is absorbed by the region
  whose chunks are nearest to its own, or by the home region.
- A row of players each within the merge distance of the next is one region, however
  long. That is a limit of this record, not something it solves.
- **By hand**: `clustine merge --survivor A --absorbed B` and `clustine split --region
  A --chunks ...`, next to `clustine move`; and the coordinator can be started so that
  it decides nothing of this by itself.

### 8. The single process, and worlds from before

The single process runs the same: the coordinator's state machine is driven inside it,
and regions are started and stopped as it says.

A world that was last served in stripes is told by its layout file being there and the
list of regions not. It is opened with one home region: the stripes' commits are
applied to the chunks, as the store does when a layout changes, and their states are
dropped. Players do not outlive a server that stops, so nothing of theirs is lost.

### 9. The order of building

Each step leaves everything working, and stripes stay until the end.

| # | Scope | Verified by |
|---|---|---|
| C0 | The messages and types of this record, refused or ignored by everyone | All existing tests |
| C1 | Store: the list of regions with those absorbed, grants with their ticks, chunks leaving only saved, replay only into what is held, pinned regions, the merge and the split as one log record each | Store tests incl. kills at every point of a merge and a split; tests from this record by someone else |
| C2a | Several regions per worker; the coordinator without "a worker runs one region" | The move and chaos tests, on stripes, with fewer workers than regions |
| C2b | Sim, worker and edge on chunk sets: claims, guests, `Elsewhere`, `NotMine`, departures that name a region, `since` in hellos | Hand-over, block, takeover, chaos and move tests on two pinned regions |
| C3 | Absorb and split through sim, worker, edge and coordinator, asked for by hand | Differential tests against one region; kills at every step; an edge away during several merges and splits in a row |
| C4 | The coordinator decides by itself | State-machine tests with scripted and random movement; no flapping |
| C5 | Stripes, `Layout` and `--boundaries` go; the single process and the cluster on the new model by default | Bots meeting and parting; crowds; every chaos and move test again; kind |
| C6 | Docs; what to try with real clients | CI |

The edge's part of C2b and C3 is where ordering mistakes hide and is not delegated.

## What a player notices

At a merge the players of both regions stand still for about as long as at a move, which
phase B measures: the absorbed region's from its release, the survivor's from when it
stops ticking, until the edge has resumed. At a split the players of the region do. For
the survivor's own players that is more than is needed, as nothing about them changes:
a merge could be told to the edge on the links it has. That is left for after the
measurement, because the edge has to get `Absorbed` and `SplitOff` right inside a
resume in any case, a link being free to end at any moment. Until then twenty players
who gather from twenty directions are nineteen short pauses for those already there.

## Consequences

- Load follows players: a group that goes off by itself becomes a region that another
  worker can run, and groups that meet become one.
- Hand-overs and actions across a boundary become rare, as boundaries are where nobody
  is; pinned regions keep them tried.
- The world store decides more than before: who holds a chunk, which regions exist,
  and when a merge or a split has happened.
- Regions do not carry entity ids of their own yet; when things other than players
  need ids away from home, that has to be decided.

## Review

An independent review against the code found twenty-one defects in the first version of
this record. What it changed:

1. Replaying a region's log wrote its old block changes into chunks that had since gone
   to another region and been built over. Grants carry a tick, chunks leave only
   saved, and replay goes only into what is held.
2. The merge and the split were said to be one step of the store, which has no step
   that covers several files and a table. Each is one log record now.
3. Entity ids by halving ranges ran the home region dry after about thirty players had
   wandered off alone and logged out. Only the home region gives out ids.
4. Retiring an empty region lost a player on their way out of it. No region
   disappears.
5. The home region could be absorbed, and a player joining at a home whose crowd was
   elsewhere was split off and handed back for ever. Home survives and keeps the
   group at its chunk.
6. A player who left and came back during a merge was in the survivor twice, and the
   leave that was kept removed the one who was there. The edge passes over what is
   about a player who is not the absorbed region's any more, and the survivor keeps its
   own.
7. A split lost a leave that was kept for the region about a player who went with the
   part, and what the part's players had been acknowledged. `SplitOff` carries how far
   messages were applied, and players go whole.
8. After a merge, entries naming the absorbed region pointed at the region they came
   from, which the edge takes for an error today.
9. An edge whose region was absorbed while that region, or the survivor, had forgotten
   it was left with players in a region that is gone, or with a numbering the survivor
   did not share. `Absorbed` is written for every edge, and `since` tells an edge when
   what it kept is stale.
10. The edge's hello to the survivor could come before it knew of the merge.
11. The absorb was not fenced, named no epoch, and went against ADR-0009, in which a
    released region is assigned at once.
12. The coordinator learned of a new region from one message, and a list without the
    absorbed regions could not say what went into what.
13. A player in a chunk whose holder was not known yet could neither be held still nor
    have their actions passed on.
14. `NotMine` was not durable and could go round between two regions for ever, and a
    resume waited for snapshots that would never come.
15. What to pass over after `Absorbed` was forgotten when a link ended in the middle.
16. Every merge and split pauses the whole surviving region; see "What a player
    notices".
17. The second step could not be tried, there being no second region before splits
    exist, and nothing replaced a boundary at a known place. Hence pinned regions,
    merging and splitting by hand, and the order of building above.
18. Guests were left open: whether they keep a chunk with a region, and how they hear
    that it has gone.
19. A region that was split off waited to be restored from the store by whoever was
    told, though the worker that made it had it in memory.
20. The coordinator and the worker process assume one region per worker throughout.
21. The distances were not tied to how far players see and how old a heartbeat is, the
    split named players the coordinator does not know, and nothing said how a merge or
    a split ended.
