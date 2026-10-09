# ADR-0017: The end of the stripes

- Status: **Proposed, and revised after its review.** The design of step C5 of
  milestone M3, phase C: stripes, `Layout` and `--boundaries` go, and the single
  process and the cluster run regions that follow their players unless told
  otherwise. An independent reviewer went over the first version against the code and
  found fifteen defects, all of which are worked in here ("Review" at the end says
  which, and what else the revision found). Of its building only the first half of
  step C5.0 is done: `bin/clustine/src/cluster.rs` is cut into a file for each
  process.
- Date: 2026-10-09

## Context

A world is still divided into stripes when it is started. Everything that lets regions
follow players is built and tested **on stripes**: the store's grants, merges and
splits ([ADR-0011](0011-the-world-store-and-regions.md)), the tick and the runner on
chunk sets ([ADR-0012](0012-the-tick-on-chunks.md)), the edge without a layout
([ADR-0013](0013-the-edge-without-a-layout.md)), merging and splitting
([ADR-0014](0014-merging-and-splitting.md),
[ADR-0015](0015-the-edge-through-merges-and-splits.md)) and the coordinator deciding by
itself ([ADR-0016](0016-when-to-merge-and-split.md)), which is off unless asked for.
[ADR-0010](0010-regions-that-follow-players.md), section 8, has this step in four
sentences. This record says how, exactly enough to build it and to write tests from.

What the code is today, as far as this record builds on it or changes it, at commit
`23a6c3b`, in which the processes of a cluster have a file each under
`bin/clustine/src/cluster/` (`coordinator.rs`, `worldstore.rs`, `worker.rs`,
`edge.rs`, `commands.rs`). "Not checked" at the end says what was read only in
excerpts, and by whom.

- **The layout** (`crates/clustine-region/src/lib.rs`). `Layout` is a list of chunk x
  coordinates; `regions()` gives region `i` the stripe `i`, `region_of` the stripe of a
  chunk, `fingerprint` a number for comparing. `Layout::new(vec![])` is
  `Layout::single()`: **one stripe, region 0, which covers the world**. So there is no
  world without stripes today, only one with a single stripe. `RoutingTable` carries
  `layout` and `spawn` beside `home`, `routes`, `absorbed` and `waiting`.
- **Who reads the layout.** Listed one by one in section 4. In short: the store is
  started with the stripes as its pinned regions and compares a fingerprint in every
  hello; the coordinator knows the regions of the layout from the start, refuses a
  worker with another fingerprint, and sends the layout to workers and edges; the
  worker process puts the fingerprint into its hellos; **the edge process works the
  home region out of the layout** (`table.layout.region_of(spawn_chunk)` in `edge`,
  `cluster/edge.rs`), puts the fingerprint into its hellos to workers, and
  takes no routing table whose layout is another; the single process starts one runner
  for each stripe. `RoutingTable::home`, which the coordinator fills from the store's
  list, is read by nobody but the coordinator's own log line. ADR-0016 says of
  `RoutingTable::layout` that nothing in that record reads it; the edge's process
  does.
- **The store without pinned regions works, and is tried only by its own tests.**
  `Table::made_from` (`services/worldstore/src/table.rs`) pins region `i` to area `i`
  of the `Division` it is given; the home region is the pinned region whose area has
  the home chunk and, **if there is none, a region made after the pinned ones that is
  granted the home chunk with tick 0**. With no pinned area at all that is region 0,
  pinned to nothing, holding one chunk, with the next region id 1. `Lanes::admit`
  issues a block of entity ids to a region that is pinned or home. A claim of a chunk
  nobody holds is granted with a `Granted` record; a return frees a chunk unless it is
  the home chunk (`Lanes::request`). The store's tests run such divisions (`gap()`,
  `gap_at_the_east()` in `regions.rs`); no process has ever been started on one.
- **A world that was divided otherwise is made over, as built.** `Lanes::load` compares
  the division it is started with against the one the table was made from
  (`Table::is_of`: the areas and the home chunk). If they differ, `make_over` puts the
  block changes of every region's live commits into the stored chunks (chosen as for
  an opening where there is a table), lets go of the commits and the state files, and
  the table is made anew from the division it was told, with a next region id no lower
  than the one it had. A world with a `layout` file and no table is kept as it is if
  the file's fingerprint is `Division::layout`, and made over otherwise. The order of
  it is harmless to do twice, and `regions.rs` kills it at every write
  (`started_at_every_kill_point`).
- **What a region that is not pinned holds** (`settle_chunks` and `update_chunks` in
  `crates/clustine-sim/src/region.rs`; section 3 has it in full). It claims every chunk
  a player of its own stands in or sees, holds what the store grants, believes what
  the store calls another's for as long as it wants the chunk, and gives a chunk back
  when nothing has used it for `return_after` ticks (`DEFAULT_RETURN_AFTER`, 600: thirty
  seconds). It never gives back the home chunk or a chunk of a pinned area
  (`Land::keeps`). **This has run under players only inside pinned stripes**, where a
  claim is answered from the table without a record and nothing is ever given back; and
  in the parts that steps C3 and C4 split off, which on stripes can be granted nothing
  they were not split off with (ADR-0016, K15).
- **What a split does with chunks that are on their way** (`RegionRunner::commit` and
  `take_answer` in `services/worker/src/lib.rs`, `Region::split` and `take_split` in
  `crates/clustine-sim/src/region/reshape.rs`). A region claims a chunk at the end of
  the tick that first wants it, the runner sends the claim after that tick, and the
  store's answer goes into the inputs of the next tick that runs. A split stops the
  region after some tick `T` and is worked out when the store has answered
  everything asked before, so every claim is answered by then and **the answers to
  the claims of the last tick or two wait in the inputs of a tick that never runs**.
  `Region::split` gives the part "every chunk the region holds" by what its ticks were
  told (`self.held()`), so a chunk whose grant waits is none of the part's, and
  `take_split(splitting, &granted)` makes it the region's that stays, however near it
  is to those who go; ADR-0014 says so in section 3.2. **And the links are closed
  with the split, with whatever they had sent that no tick took**: an edge's
  `Subscribe` for the chunks that came into a player's view by the region's last tick
  among it. The edge keeps its subscriptions, and its next hello to the region names
  them all again as a viewer's (`Fanout::take_link` in `services/edge/src/fanout.rs`),
  those of the players who went as well, because it reads that they went only in the
  answer to that hello. The region claims every chunk of such a hello that it knows
  nothing of, in the tick that takes it (`RegionRunner::hello`, `settle_chunks`).
  On stripes none of this showed: a part could not grow at all. Section 3.6 has what
  follows from it without stripes, and what is changed.
- **The coordinator** (`services/coordinator/src/state.rs`). `Coordinator::new` makes a
  `Region` for every stripe of `CoordinatorConfig::layout`; `register` refuses a
  fingerprint that is not its own (`Refusal::Layout`, the only refusal there is);
  `routing_table` copies the layout. For one lease from when it was made it assigns
  nothing that its owner did not let go of (`assign`), evens nothing out (`even_out`)
  and begins nothing by itself (`the_world_is_known`): three tests of `now - started <
  lease`. `listed` adds a living region it does not know, without an owner. A
  coordinator that decides nothing by itself has the list read on events only: when it
  starts, at every registration, and for merges and splits; one that decides by itself
  also asks whenever no reading has been answered for a lease, or none ever
  (`list_is_due`). **A coordinator that decides nothing by itself and whose first
  reading fails reads the list again only when a worker registers or somebody asks for
  a merge or a split.** On stripes that costs nothing, as it knows the stripes anyway.
  A worker that says it has released a region is heard only if the coordinator knows
  the region (`released`, `without_owner_since`); on stripes it always does. Three
  things take a region from a worker that lives: the worker has not been heard for a
  lease (`forget_silent`), the region has not been vouched for within a lease
  (`take_unvouched`, which also puts the worker at fault for thirty seconds), and
  the service closes a connection that has been silent for a lease.
- **The service around it** (`service.rs`) knows nothing of TCP but in `serve_from`,
  which accepts connections and hands each to `Service::attach` as an `End<FromCoordinator,
  ToCoordinator>`. `clustine_rpc::link::in_process` makes such a pair of ends for any
  two message types.
- **The worker process** (`worker` in `cluster/worker.rs`) is one loop of about 700
  lines over its regions' phases (`Opening`, `Starting`, `Running`, `Releasing`) and
  the merges and splits under way. It reaches the outside in these places:
  `register` and `stay_registered`, which make and hold the connection to the
  coordinator and speak to the loop through watches and queues, one of them the queue
  of what the store refused (`refusals`); `open_region` and `fetch`, which open a
  region at the store's address; `accept_edges`, which attaches links to the regions
  in a watch (`Serving`: the regions that are restored and tick, each with its hello
  and its `Links`); and `stop_signal`. A region the coordinator names with another
  epoch than the worker runs it with is taken out of the loop's regions, **its runner
  is told to stop in the background, and the region is opened with the new epoch at
  the same time** ("the coordinator has taken the region; letting go of it", then
  "given a region"): which of the two reaches the store first is not fixed. A region
  that has lost the store is opened again. A region whose state cannot be read, or
  that the store refuses for another reason than its epoch or its having been
  absorbed, ends the loop with the error.
- **The single process** (`bin/clustine/src/lib.rs`) has none of that. `Server::start`
  makes a store for the stripes of `Config::boundaries`, opens every stripe
  (`run_first`, trying epochs from 1 up), runs each on a `Worker`, and hands the edge
  the links with `home = layout.region_of(spawn chunk)`. It has no coordinator, merges
  and splits nothing, moves nothing, and does not open a region again that has lost
  the store. `Server::take_over(region)`, for tests, opens the region with the next
  epoch **while its runner still runs**, which fences that runner, stops it only
  then, and gives the edge the new link. `Server::start` returns after every region
  is restored, and with the error if one cannot be.
- **The edge** (`services/edge`). A join goes to `Fanout::spawn_region`, which is
  `Routing::home` as the edge was started with and never changes. A message for a
  region without a link, or with a link that is not welcomed yet, is kept and sent
  when the link is; a player who has not been placed for `region_patience` (20 s) is
  disconnected. A viewer's chunks are asked of the player's own region; `view_area`
  sends a client with view distance `V` the chunks up to `V + 1` away along an axis,
  329 of them at 8, 19 new ones for a step along an axis.
- **The tests.** `common::config()` reads `CLUSTINE_TEST_BOUNDARIES` into
  `Config::boundaries`; 43 tests of ordinary play (`blocks`, `join`, `movement`,
  `oracle`, `persistence`, `players`, `status`) run once on one stripe and once on
  three. `handoff`, `takeover`, `sending` and two tests of `chaos` set the boundaries
  of a single process themselves; `takeover` and those two call `Server::take_over`
  with `RegionId(0)` and `RegionId(1)`. Every cluster of processes is made by
  `Cluster::new(directory, workers, boundaries)`, which passes `--boundaries` to the
  coordinator and to the store; no test starts a cluster without. `follows.rs` runs
  `--reshape by-itself` on two stripes. `reports.rs` plays a coordinator and sends a
  `Layout` in `Assigned` and in a `RoutingTable`.
- **The bots** know no layout. `Ledger::lines` and `Crossing::line` are block x
  coordinates that a test says a boundary is at: the ledger counts steps and actions
  across them, and, though its comment says it does nothing else, builds half of what
  it builds near a line on the far side of it (`spot` in `tools/botswarm/src/ledger.rs`).
  A scenario's bots are steered along x only (`Progress::walk_between`), each on a lane
  of its own along z.

## Decision

Words in `code` are names in the code, or will be. `V` is the largest view distance
the edges grant, reach is `V + 1`, `D_m` and `D_s` are the merge and the split
distance in chunks, as in ADR-0016. "The home chunk" is the chunk players enter the
world in. A region's **land** is the chunks it holds.

### 1. The shape of it

1. **A world begins as one home region that is pinned to nothing.** The store makes it,
   as it already can. Nobody else is told how a world is divided, because it is not.
2. **Pinned regions stay, as the store's alone.** `clustine worldstore --pin 4` pins
   two regions side by side where `--boundaries 4` made two stripes. The coordinator
   learns of them from the list, as it does today, and is told nothing.
3. **The coordinator knows no region until it has read the store's list.** It reads
   until it has, whichever way it reshapes.
4. **`by-itself` is what the coordinator does unless told `--reshape by-hand`**, with
   the rules of ADR-0016 as they are. Two things are added to them: a line in the log
   when the distances are too short for the view distance, and one when a world of
   pinned regions is reshaped by itself.
5. **The single process is the cluster's parts in one process, joined by channels.**
   It runs the coordinator's service, the worker's loop and the edge's link-keeper as
   they are, with one worker, and its store, its coordinator and its links are reached
   through channels where the processes use TCP. Nothing of the policy, of the
   worker's loop or of the link-keeper is written twice.
6. **A world that was last served otherwise is made over**, as built: what was built
   in it stays, its regions begin anew.
7. **A split leaves the region that is split no land on the side of those who go.**
   The first version of this record said that nothing changes in the simulation and
   in the region runner. The review showed that a split as built keeps for the region
   that stays what it was granted in its last tick or two, and this revision found a
   second way to the same end; either leaves a strip of the old region ahead of a
   player who walks on. Section 3.6 changes `Region::split`, what the runner hands
   it, and what the runner answers an edge that has not heard of a split.
8. **Nothing changes in the edge's fan-out or in the store.** What is new for them
   is that what they were built for now happens.

### 2. How a world begins and is found again

#### 2.1 The store

`clustine worldstore --world DIR [--pin X[,X…]]`.

- **Without `--pin`**, the division is `Division { home, pinned: vec![] }`, with `home`
  the chunk of `spawn_point()`. A new world's table then has: **region 0, the home
  region, pinned to nothing, granted the home chunk with tick 0; the next region id 1;
  no absorbed pair.** The list shows `home: RegionId(0)`, one `RegionInfo` with
  `pinned` empty and `bounds` the home chunk alone, and `next: RegionId(1)`. Region 0
  is issued a block of entity ids when it is first opened, as the home region; no
  other region of such a world ever has one.
- **With `--pin X1,…,Xn`** (chunk x coordinates, ascending, no two alike, negative
  ones allowed), the division pins `n + 1` regions side by side: region 0 to every
  chunk with `x < X1`, region `i` to `Xi <= x < Xi+1`, region `n` to `x >= Xn`. That is
  what `Division::stripes` makes of a layout, so the ids, the home region (the one
  whose area has the home chunk) and the table are those a world of stripes has.
  **A world that was served with `--boundaries 4` and is started with `--pin 4` is
  found as it was**, regions and states and all: `Table::is_of` compares areas and the
  home chunk, and the fingerprint was never in the table.
- The pinned regions of `--pin` cover the world, so nothing in such a world is free
  and no region in it grows: pins are for boundaries at a known place and for nothing
  else. Areas that leave a gap stay what the store's tests make by hand.
- **A hello names a region and an epoch**, and no layout (section 5). A region the
  table does not have is refused, as today.

`Division` loses `layout`. It gets two constructors in place of `stripes`:

```rust
impl Division {
    /// A world that is one home region, pinned to nothing.
    pub fn open(home: ChunkPos) -> Self;
    /// Regions pinned side by side, cut at these chunk x coordinates. `Err` unless
    /// they ascend without repetition.
    pub fn side_by_side(home: ChunkPos, cuts: &[i32]) -> Result<Self, NotAscending>;
}
```

`Store::memory`, `Store::local`, `spawn` and `spawn_local` go on meaning what they
mean: one region, region 0, pinned to the whole world, with the home chunk at the
origin. Every test of the worker, of the store and of the simulation that uses them
stands as it is.

#### 2.2 Worlds from before

What `Lanes::load` does today is what ADR-0010, section 8, asks for, and nothing more
is built. Stated for the three worlds there can be:

| The world on disk | Started with | What happens |
|---|---|---|
| A table made from the same areas and home chunk | anything equal | found as it was |
| A table made from other areas or another home chunk (stripes opened without `--pin`; a following world opened with `--pin`; other pins) | | **made over** |
| A `layout` file and no table (older than step C1) | anything | **made over** |

**Made over** means: every block that a region had confirmed is in the stored chunks;
every region's state, log and grants are dropped, and the regions that were split off
with them; the table is that of the division the store was started with; ids below
the old table's next id are not given to regions made later. A world of stripes that
is opened without `--pin` is afterwards one home region, region 0, holding the home
chunk, with the epoch and the entity ids that stripe 0 had in its region file. The
store says so in its log once (`the world was divided otherwise before; what its
regions had is in the stored chunks now`). It is done before any hello is taken, and a
store that dies in the middle does it again.

**Whom it finds.** In the single process nobody: players do not outlive the process,
and the store is its first part to start. **In a cluster the workers and the edge can
live through a store that is stopped and started with other pins, and it is the
operator's to stop them first.** If they are not stopped (N16): every region has lost
the store and is opened again by its worker; a region the new table has under that
id is restored without a state, so without its players, whom the edge then finds
gone and disconnects; a region the new table does not have is refused, which ends
its worker with `opening region … at the world store`; the coordinator learns the
new regions from its next reading and an edge that sees another home region says
what section 5.4 has it say. Nothing that was built is lost, and nobody who was in
the world stays in it. The store therefore writes a second line whenever it makes a
world over, at the level of a warning: `the regions of this world begin anew:
whoever is in it has to join again. Stop the workers and the edges of a cluster
before its world store is started with other pins`.

One thing changes. Today a world with a `layout` file and no table is kept as it is if
the store is started with the same layout. With the fingerprint gone the store cannot
tell, and **such a world is always made over**. It is a world from before 2026-10-08;
nobody has one of value, and it loses nothing that was built.

**A world that cannot be opened says so and is not touched**, as today: a table or a
region file that cannot be read ends the start with `StoreError::Damaged` or
`StoreError::Table` before anything is written, and making a world over is the only
thing a start writes besides a new world's table. Going back to a build from before
this step works the same way round: a store of that build, started with
`--boundaries`, makes a following world over into stripes.

#### 2.3 The coordinator

`Coordinator::new` makes a coordinator that **knows no region**. Its routing table has
no route, `waiting: 0` and `home: None`. A worker that registers is told that it runs
nothing; one that reports what it runs is believed, as a part's worker is today.

**The list is read until it has been read once.** The service (`Service::tick`), not
the state machine, has the list read at every tick at which no reading is under way
and the coordinator still waits for its first list (`Coordinator::awaits_the_list()`:
it has been handed none, and was not made knowing its regions, see below). That is
every 250 ms for one that decides by itself and a quarter of the lease for one that
does not. After the first list everything is as ADR-0016, section 7, has it: on a
timer for the one, on events for the other.

**A worker that says what it let go of before the first list is believed**, as one
that says what it runs is. `Coordinator::released(now, name, region, epoch)` gets one
case more, behind the three it has: if the coordinator **does not know `region`, has
been handed no list yet** (`home()` is `None`) and `name` is a registered worker, the
region is noted as known, **without an owner, with `epoch`, and let go**; no epoch
issued from then on is at or below `epoch`; the worker goes behind all the others for
it; and the call ends like the third case, by giving away what has no owner. A region
that was let go is given away whatever the grace period says, so it has an owner by
that very call if any worker can be given it, the one that released it included. It
logs `a worker released a region before this coordinator had read the list`.
Without this the word is dropped, as the worker says it once, and the region is
found by the first list and waits out the grace period: a lease of standing still
where a move takes a moment (the review's fourth defect; on stripes the layout named
the region, so `without_owner_since` found it).

What then becomes of such a region is what becomes of one noted on a worker's
report, by rules that are there:

- **the first list has it living**: nothing changes; if nobody could be given it,
  its epoch is raised to the list's;
- **the first list has it absorbed, or has a next id above it and does not have it**:
  it is removed, and its owner loses it (`listed`). A worker that was given it
  meanwhile has been refused by the store, or is, and says so;
- **the first list has a next id at or below it**: left alone, as the reading is
  older than the split that made it;
- **another worker reports that it runs the region** before it is given away, with
  the epoch the release named or a higher one: that worker owns it (`report`). With
  a lower one, or once the region has an owner: it may not go on, as for any report
  that comes too late;
- **the region was let go for a merge** that a coordinator before this one began:
  this coordinator knows of no merge, gives the region away, and its new owner is
  told by the store that it was absorbed, or runs it.

After the first list `released` is what it was: a region the coordinator does not
know then is one the store does not have.

The other way, which the review named as well, is to keep such words in the service
and say them to the state machine behind the first list. It was not taken: the
region would wait for a store that may be away, where the state machine gives it out
at once and its new owner waits for the store itself; and a word kept and said later
is said out of its order against the registrations and reports that came meanwhile,
which is where this state machine has had its mistakes.

Why it is needed: a store that starts after the workers have registered; and a
coordinator that is started anew while the store is away and one worker is dead, whose
region nobody reports and only the list can name. On stripes the layout named it.
Why in the service: the state machine's answer to a tick stays what it is for the
hundreds of tests that compare it (`Changes::read`), and the service already reads by
itself at every registration.

The first list brings the home region and every other living region; each is added
without an owner and assigned as today: when the grace period is over, to the worker
with the fewest. **Nothing is merged or split by itself before that**, as ADR-0016,
section 5.1, has it; nothing is asked by hand before it either, as a merge or a split
is refused for a region the coordinator does not know.

**A coordinator whose workers are in its own process waits for nobody and gives
nobody up.** `Coordinator::alone(config, now, first_epoch)` is `new` with two
differences, and only the single process uses it (section 6):

- **no grace period**: nobody else can have been running its world, so it assigns
  at once, evens out at once and decides at once. The three tests of the grace
  period become one time, `grace_until`, which `new` sets a lease from `now` and
  `alone` sets to `now`;
- **it takes no region from a worker for having been slow**: `forget_silent` and
  `take_unvouched` do nothing, and the service that serves it closes no worker's
  connection for silence. There is nobody else to give a region to, so taking it
  could only give it back to the same worker with another epoch, which stands its
  players still for a restore and, when the coordinator decides by itself, leaves
  the worker at fault and nothing merged or split for thirty seconds. That is what
  a process does that is held up for longer than the lease: a laptop that sleeps, a
  debugger, sixty tests on six processors. A region whose runner has lost the store
  is opened again by the worker's loop, which needs no coordinator for it.

Section 6.6 goes through everything else that hangs on the lease.

**For the tests of the state machine**, `Coordinator::knowing(config, now,
first_epoch, regions)` makes a coordinator that knows these regions from the start,
without owners and without a home, as `new` did for the stripes, and that does not
wait for a first list (`awaits_the_list()` is false from the start). It is
`#[doc(hidden)]` and no process calls it. Why it exists: some seven hundred tests of
the coordinator (`state.rs`, `state/tests/follows.rs`, `tests/reshape.rs`,
`tests/decides.rs`, `tests/follows.rs`, `service.rs`) are about what a coordinator
does with regions it knows, and say `Layout::new(vec![0])` only to have two. Handing
each a list instead would change the version of its routing table, its home region
and what it may be asked, and would have to be gone through test by test. What a
coordinator does that learns its regions from the list has tests of its own (section
9, Q1 to Q8 and Q12).

**The tests of the service reach such a coordinator through a door of their own.**
They start a served coordinator with `serve` or `serve_from`, with stripes from
their `config(boundaries)` and a store that never answers, and after this step
`serve` makes a coordinator that knows no region for good. So the loop that serves
is given its coordinator: `serve_with(listener, coordinator, lists)`, `#[doc(hidden)]`,
which `serve` calls with `Coordinator::new`, `serve_local` with `Coordinator::alone`,
and the tests' `Served::start` and `start_at` with `Coordinator::knowing(config,
now, first_epoch, &regions)`, the regions being those their boundaries made
(`0..=boundaries.len()`). **Such a coordinator is read for on events only**, as the
tests' coordinators are today: the reading at every tick is for a coordinator that
waits for its first list, and one made `knowing` does not. A thread and an
`unlisted` every 150 ms at their lease of 600 ms would change what they count.

Of those tests five places assert on `RoutingTable::layout`, which goes: the helper
`Cluster::of` in `service.rs`, the helper that compares every table in `state.rs`
(`assert_eq!(now.table.layout, self.layout)`), two tests of `state.rs`, and
`tests/reshape.rs`, whose `table.layout.region_count() == 3` under the comment "a
table with fewer routes than the layout has stripes is complete when the regions are
fewer" becomes a statement about a coordinator made knowing three regions. The four
others lose the line. The two tests of `Refusal::Layout` (one in `state.rs`, one in
`service.rs`) go with the refusal.

#### 2.4 What is started in which order

**In a cluster: in any order.** The store makes or finds its table. The coordinator
reads the list until the store answers. Workers register as soon as there is a
coordinator and open what they are given as soon as there is a store (`open_region`
tries until the store can be reached, as today). **The edge waits for a routing table
that names the home region, has a route for it and has no region waiting for an
owner** (section 5.4), and lets players in then.

**What a restart of each finds**, none of it new but the first line:

- the coordinator: no region, until the list and the workers' registrations name
  them; a lease of grace; nothing begun by itself until every region it knows has been
  reported once (ADR-0016, K6);
- the store: its table and its log, as ADR-0011, section 4.2, has it;
- a worker: nothing; it registers and is given regions, its own again if it is back
  within the lease;
- the edge: a new start, which every region resets its players of when the edge says
  hello; the players have to join again;
- **all of them at once, or the single process**: the regions of the table, the home
  region and whatever parts there were. The parts have no players once the new edge
  has said hello to them, and are absorbed as section 3.4 has it.

#### 2.5 Where the spawn point and the home chunk come from

From where they come from today, which is not the layout. `spawn_point()` in
`bin/clustine/src/lib.rs` is fixed in the program. The store's process makes its
`Division::home` of it; the coordinator's process puts it into `CoordinatorConfig::spawn`,
from which the policy takes the chunk players enter in, a worker its
`RegionConfig::spawn` (`FromCoordinator::Assigned::spawn`) and an edge
`RoutingTable::spawn`. **Which region is home comes from the store's list alone**:
`RegionList::home`, which the coordinator puts into `RoutingTable::home`, which the
edge now reads (section 5.4).

The three processes agree on the spawn point because they are one program. A store
of a build with another spawn point makes the world over when it starts (the home
chunk is part of the division); a coordinator of another build would decide by a
chunk that is not the store's home chunk. Nothing checks that, as nothing checks that
the coordinator's view distance is the edges'. It is a question for the step that
makes the spawn point something other than a constant ("Open questions").

### 3. What a region that is not pinned holds

Sections 3.1 to 3.5 are as built, and nothing of them changes; a reader who writes
tests needs them exact, and this step is the first to rest on them. Section 3.6 is
what does change: which chunks a split gives the part, and what the region that was
split does about chunks an edge asks it for before the edge has heard of the split.

#### 3.1 What it asks for, and how far

At the end of every tick a region claims every chunk it knows nothing of in which **a
player of its own stands**, or which **an edge asks of it for one of its own players'
view** (a viewer's ticket). An edge asks a player's own region for every chunk the
player sees. So a region asks for the chunks within the reach of each of its players:
the area of `view_area`, `2V + 3` chunks across with the corners cut, 329 chunks at
`V` = 8, and 19 more for every chunk a player walks along an axis.

A claim goes to the store in the tick that made it and is answered into the next tick
or the one after; the region ticks on meanwhile, and its players with it. **Granted**:
the region holds the chunk, loads it, and serves it. **Another region's**: it believes
that for as long as it wants the chunk, answers the edge `Elsewhere`, and the edge asks
that region as a guest. A guest's asking makes no region claim anything (outside
pinned areas).

A chunk comes into a player's view nine chunks before the player comes to it, so the
answer is there long before they are. A player who does stand in a chunk that is
asked for and not answered stays the region's. The chunk is not on their screen yet,
no region having sent it, and what they did to a block of it would be acknowledged
without effect (ADR-0012, section 2.3).

#### 3.2 What it gives back, and when

A chunk is **used** in a tick at whose end a player of the region stands in it or any
edge has it asked of the region, as a viewer or **as a guest**. A chunk the region
holds that nothing has used for more than 600 ticks, thirty seconds, is given back,
unless it is the home chunk or in a pinned area. It was saved when its last ticket was
let go. After a restore, a merge or a split every chunk the region holds counts as
used at that tick, so nothing is given back sooner than thirty seconds after.

So **the land of a region is what its players see, what guests look at of it, and a
trail thirty seconds long behind whoever moves**: eight chunks for a player on foot,
twenty for one in creative flight.

#### 3.3 The edges of its land

- **A chunk nobody holds** is whoever's asks first. A region grows where its players
  go, and K15 of ADR-0016 is gone: a part is granted what its players come to see.
  That rests on the region it was split off not asking first for what lies ahead of
  them, which is section 3.6.
- **A chunk another region holds.** The player sees it as that region's guest. What
  they do to a block of it is passed on and takes effect a tick or two later. A player
  who walks into it is handed over, without standing still. All of that is as between
  stripes since M2; what is new is the line it happens at, which is wherever two
  regions' claims met and need not be straight.
- **A chunk another region held until a moment ago.** The region that believed it
  another's is told `NotMine` by that region's link, asks the store again, and is
  granted it or told who holds it now. A player handed to a region that has given the
  chunk back meanwhile is taken in, and the region claims the chunk for them
  (ADR-0012, section 2.2).
- **With the distances that follow from the view distance, none of the second and
  third happens in ordinary play.** Two regions whose players are 23 chunks apart want
  the chunks within 9 of each: their lands are four chunks apart, and neither's
  players see the other's land. At 22 they are merged. Section 3.5 has what that rests
  on.

#### 3.4 Regions without players

**The home region without players** holds the home chunk and gives everything else
back within thirty seconds, unless a guest looks at it. It goes on running. Whoever
joins is placed in the home chunk, the region claims what they see, and what another
region holds of that they see as its guest.

**Any other region without players** gives back all it holds within thirty seconds,
unless a guest looks at it. It is absorbed by the rule of ADR-0016, section 4.4,
which is kept as it is: when it has been without players for three rests (thirty
seconds), by a region that has no players either: the home region, else the region
without players that has the lowest id below its own. The rule's exception for pinned
regions does nothing in a world without pins.

**ADR-0016 asked this step to look again at the one region without players that is
never absorbed while the home region has players.** It is kept. Off stripes that
region holds nothing as a rule; it costs a thread that ticks an empty region, a lane
of the store and a line of the routing table. It is the region every later empty
region goes into, so nobody stands still for a departure anywhere, and it is absorbed
itself once the home region is without players. The other choice,
the home region as survivor also when it has players, stands everybody at the spawn
point still for a region nobody is in, every time somebody leaves the game far away.
What would show that the rule is wrong: more than one region without players staying
in the routing table for longer than a minute while no merge fails.

**An empty region that a guest looks at** keeps what the guest sees, for as long as
they look, and is absorbed all the same when its turn comes, by a region without
players. N5 in section 7 has what follows.

#### 3.5 Whether the default distances are right off stripes

`D_m` = `2 * (V + 1) + 4` = 22 and `D_s` = `D_m + 8` = 30. They are right, and for
more reasons than ADR-0016 gave on stripes. Off stripes two things rest on them that
no stripe needed:

- **`D_m` is more than twice the reach.** A region's land reaches as far as its
  players see. Two regions whose players are further apart than `D_m` have lands that
  do not touch, so no claim is answered "another's" and nobody is a guest. Players
  are merged while their lands are still four chunks apart, and those four chunks are
  the two seconds a merge takes at the speed of flight.
- **`D_s` is more than the reach and the trail.** A player who steps into another
  region's land is handed over to it. If they are then further than `D_s` from its
  other players, that region is surely apart and is split, which stands its players
  still. With `D_s` = 30 and land that reaches 9 chunks around players, that takes
  stepping into a trail, more than 30 chunks and less than 30 seconds behind somebody:
  a chunk a second, which is sprinting in the air (ADR-0016, K14).

**With distances set by hand below that, the rules still hold and play is worse**: a
player who walks up to another region is handed over at the rim of its land, nine
chunks out, before any merge is wanted, and if `D_s` is less than that they are split
off again, walk on and are handed over once more. Nobody is lost and nothing breaks;
those who stay stand still once more than they need. ADR-0016's end-to-end tests ran
at 3 and 5 chunks with a view distance of 8 and did not see this, because on stripes
a region's land was its stripe.

So two things, both new:

- **The coordinator says so.** When it decides by itself with `D_m < 2 * V + 3`, it
  logs once, at its start: `the merge distance is less than players see across:
  regions will hand players over where they would merge` with `merge_distance`,
  `view_distance` and `needs` (`2 * V + 3`). It is a warning and not a refusal: the
  state machine's tests and whoever wants to see hand-overs set such distances on
  purpose.
- **Tests of the default rule keep the rule and shrink the view.** With
  `--view-distance 2` everywhere the distances are 10 and 18 chunks, 160 and 288
  blocks, which a bot walks in under a minute; lands are 7 chunks across and never
  touch. Section 9 uses that. The owner's trial uses the real distances and creative
  flight, in which 480 blocks are three quarters of a minute (section 10).

**Who sees whom across a boundary**: at the default distances, nobody, but for the
seconds in which a merge that is wanted waits for a region's rest. Then they see each
other as guests do, and are handed over if they walk in.

#### 3.6 A split leaves no land ahead of those who go

`A` is the region that is split, `N` the part, `p` a player who goes, `s` one who
stays. `T` is `A`'s last tick before it stops for the split and `M` = `T + 1` the
tick of the split. A chunk is **on the part's side** if it is nearer to a chunk in
which somebody who goes stood at `T` than to every chunk in which somebody who stays
stood at `T`, and than to the home chunk if `A` holds it, counted as `Region::split`
counts; a chunk that is as near to the one as to the other is not.

**What is wrong as built: two ways to one end.** Both need `p` to cross a chunk
border in the last tick or two before `A` stops, which a player who walks on does
every few seconds.

1. **A grant that waits stays with `A`.** `p` enters a chunk; the tick that moved it
   is published; the edge moves `p`'s view and sends `A` a `Subscribe` for the row of
   chunks that came into view; tick `T` takes it and claims them; the store grants
   them and answers before the split is worked out; the answer waits in the inputs of
   a tick that never runs. `Region::split` gives `N` what `A` holds by its ticks, so
   the row is none of `N`'s, and `take_split` makes it `A`'s.
2. **A subscription that no tick took is said again, to `A`.** `p` enters a chunk in
   tick `T` itself. `T` is published while `A` stands still; the edge sends the
   `Subscribe`, which no tick takes and the split drops with the link. The edge still
   has `p` under `A`, links to `A` again and names every chunk of `p`'s view in its
   hello as a viewer's. The tick that takes the hello claims all of them that `A`
   knows nothing of. Those that went with the split are answered "`N`'s". **The row
   that nobody had claimed yet is granted to `A`.** A moment later the edge reads the
   `SplitOff` among the answers to that hello and moves `p`'s view to `N`, too late.

Either way `A` holds a row of chunks nine chunks ahead of `p` (three at a view
distance of 2). `N` claims it for `p`'s view and is told "`A`'s"; the edge asks `A`
as a guest, which keeps the row used and `A`'s for as long as `p` looks. Some
seconds later `p` stands in it, `N` lets `p` go to `A`, and `p` is `A`'s player 40
chunks from everybody else of `A`: when `A` has rested it is split again, **and
everybody at the spawn point stands still a second time for a player who only
walked on**; `N` is left without players. That is ADR-0016's K15, which this step is
there to end. For each of the two the tick that matters is one of the 15 to 29
between two chunk borders in creative flight, one of 8 for a bot at two blocks a
tick: reckoned, not measured.

**What holds after this section** (statement L): from tick `M` on, `A` is granted no
chunk on the part's side for a player who went. It holds one only if a player of
its own stands in it or sees it, which at distances that follow from the view
distance cannot be when it is split (`D_s` is more than twice the reach), and comes
to be only when the two have come near enough to be merged (N3). Two things are
changed for it, one for each way, both in the simulation and the runner; the edge,
the store and the coordinator are as they are. Section 3.6.5 ends with the one case
in which it does not hold.

##### 3.6.1 The grants that wait go by nearness

```rust
impl Region {
    /// What this region and a new region `part` would be if the players standing in
    /// `named` were split off, or why the split is off. Changes nothing. `granted`
    /// are the chunks the world store has granted the region in answer to claims
    /// that no tick has been told of; each counts as a chunk the region holds.
    pub fn split(
        &self,
        named: &[ChunkPos],
        part: RegionId,
        granted: &[ChunkPos],
    ) -> Result<Splitting, NoSplit>;
    /// As before; `granted` has to be what `split` was given.
    pub fn take_split(
        &mut self,
        splitting: Splitting,
        granted: &[ChunkPos],
    ) -> (Vec<(ChunkPos, Chunk)>, Part);
}

pub struct Splitting {
    pub state: RegionState,
    pub part: RegionState,
    pub chunks: Vec<ChunkPos>,
    /// Where the line of this split is.
    pub sides: Sides,
}

/// Where a split put its line: the chunks in which those stood who went, and the
/// chunks in which those stood who stayed, with the home chunk if the region held
/// it. Both ascending.
pub struct Sides {
    pub seeds: Vec<ChunkPos>,
    pub staying: Vec<ChunkPos>,
}

impl Sides {
    /// Whether the chunk at `position` is on the part's side: nearer to a seed than
    /// to every chunk of `staying`, and any chunk if nothing stays.
    pub fn goes(&self, position: ChunkPos) -> bool;
}
```

`split`, where ADR-0014, section 2.4, says "the region holds", means from now on
**held by what its ticks were told (`Knowledge::Held`) or named in `granted`**:

1. **Seeds**: the chunks of `named` that the region holds so, that are not the home
   chunk, and in which a player stands. A player who stands in a named chunk whose
   grant waits is a seed and goes. (As built they stayed, alone among chunks that
   went, on a chunk that stayed with them: the same strip, by a third way.)
2. Who goes, where the stayers are, `NoSplit::Nobody` and `NoSplit::NothingStays`:
   as they are, by that meaning of "holds".
3. **The chunks of the part**: every chunk the region holds so for which
   `sides.goes` is true, ascending, each once. That is the rule there was, applied
   to the grants that wait as well.
4. The two states: as they are.

It is still a function of the region and its arguments and changes nothing: a region
that plans and does not take is, bit for bit, the region it was (ADR-0014, section
2.6), which handing the grants to the region before the split is worked out, as the
review put it, would not have kept.

`take_split(splitting, granted)`: the region becomes what `Region::restore` makes of
`splitting.state` and of the chunks it held and those of `granted`, **without the
part's**, and of its pinned areas. `Part::region` is as it was, restored holding
`splitting.chunks`. The part of ADR-0014's sentence that goes: "none of those is a
chunk of the part, as the sim did not hold it when the part was worked out".

**The runner** (`RegionRunner::commit`, `take_answer`):

- `commit` calls `self.region.split(named, *part, &self.inputs.granted)`. The inputs
  stay where they are: if the split is off or declined, the next tick takes them,
  as today; if the store has another id for the part, the split is worked out again
  from the same inputs. No claim is outstanding while the runner waits for the
  store's answer (no tick runs, and every claim was answered before the commit), so
  the grants that wait when the answer is taken are those the split was worked out
  with.
- `take_answer`, on `StoreReply::Split`: `waiting_for_the_tick()` first, as today,
  **before the region takes the split**: a chunk the store had delivered is kept
  only if the region held it by what its ticks had been told. So nothing is kept in
  memory of a chunk that was given back and granted again by an answer no tick took,
  whichever of the two regions gets it: another region can have held and changed it
  in between, and whoever holds it now reads it from the store
  (`a_chunk_read_before_the_region_gave_it_back_is_not_kept_when_it_is_granted_again`
  goes on holding, and gets a twin for a chunk that goes to the part). Then
  `take_split(splitting, &granted)` with those same grants.
- The line `a part of the region has been split off` gains `chunks`, how many the
  part holds, and `waited`, how many of them were grants that no tick had been told
  of.

**The store takes it as it is.** A grant is in the table, with its record durable,
before the claim is answered (`Lanes::request`, `end_group`), so every chunk of
`granted` is one the region holds by the table: `may_split` and `Table::split` find
`held_from(region, chunk)` for it, as for a chunk of a pinned area that the region
claimed, and move it to the part. Why ADR-0014 kept such chunks out of the part: so
that `split` could go by what the region itself knows, with one argument less. No
check of the store rested on it.

##### 3.6.2 What an edge asks for before it has heard of the split

The second way cannot be closed when the split is worked out: the subscription is
not there yet, and nothing says when the edge will have sent it. It is closed where
it does its harm, at the hello. **The region that was split takes a chunk on the
part's side that an edge asks it for as a viewer, and that it knows nothing of, to
be the part's, for as long as that edge has not heard of the split.**

The runner keeps, for every split it has made and by the part's id:

```rust
/// A split this runner made, kept while an edge has not heard of it.
struct Parted {
    /// Where its line is.
    sides: Sides,
    /// The chunks that went with it.
    gone: BTreeSet<ChunkPos>,
}
```

- **Made** when the runner takes the tick of a split, from the `Splitting`.
- **Used** whenever the runner makes a **new viewer's subscription** for a link: for
  each of the `chunks` of a hello, and for each chunk of a `Subscribe` the link has
  no subscription to. If the region's state as of its last tick has, in the outbox
  of that link's edge, a `Durable::SplitOff` that names a part the runner has a
  `Parted` for; and the region **knows nothing of the chunk** (`Knowledge::Unknown`);
  and the chunk is in `gone`, or is outside the region's pinned areas and
  `sides.goes` is true of it: then `(chunk, part)` is put into the coming tick's
  `foreign`, as if the store had said so. Of several such parts the lowest id.
  Nothing is put in for a guest's subscription, for one that is said again
  (`Subscribe` for a chunk that was told elsewhere, which has the region ask the
  store, as ever), or for a chunk the region holds, has asked for or believes
  another's.
- **Dropped** after any tick after which no outbox of the region has a `SplitOff`
  naming that part: every edge that had a player go has confirmed the entry, or was
  forgotten, or has started anew. And all of them when the region takes a merge.
- It logs `chunks asked for players who went are taken for the part's` with `part`,
  `chunks` and `free`, how many of them did not go with the split, once for each
  message that had any.

**What follows, step by step** (the code that does it is there; nothing but the
runner's part above is new).

1. Tick `M + 1` of `A` takes the hello. The chunks of `p`'s view that `A` does not
   hold are believed `N`'s (`update_chunks` believes the store's answers whatever
   the region knew); they are wanted, by a viewer's ticket, so the belief is kept and
   **nothing is claimed for them**. The same tick answers each `Elsewhere { region:
   N }`, which ends the hold of the hello for it (ADR-0014, section 3.6). For the
   chunks that went this is what the store would have said a tick or two later; for
   the row nobody holds it is what will be so.
2. The edge reads that tick's messages in the order the runner makes them: the
   welcome, the outbox entries, the presence answers, and only then the answers to
   subscriptions. So it handles `SplitOff` first (`Fanout::split_off`, `move_stay`):
   `p` is `N`'s, `p`'s view is asked of `N`, and its subscriptions at `A` become a
   guest's where `p` still sees the chunk (`unwant`), with a `SubscribeAsGuest` that
   has a new number; then it confirms the entry. The `Elsewhere` answers that follow
   carry the number 0 and are about subscriptions that are no longer a viewer's:
   the edge passes them over (`Fanout::elsewhere`).
3. `A` takes the `SubscribeAsGuest` and, behind it, the `Confirm`: the tickets are a
   guest's, nothing wants the beliefs any more and they go (`settle_chunks`), each
   subscription is answered `NotMine` and ends, and the `Parted` is dropped.
4. `N` is asked for `p`'s view as a viewer, by its hello or by a `Subscribe`. It
   serves what it holds and claims what it knows nothing of: **the row is `N`'s by
   the first claim there is for it.**

**A chunk on the part's side that somebody of `A` does see** (possible only where
somebody who stayed is within twice the reach of somebody who went: distances set
by hand, or a split asked by hand): the edge asks `N` for it as a guest; if `N`
holds it, `N` serves it, which is right; if nobody does, `N` says `NotMine`, the
edge asks `A` again, at once or within a second, and `A` asks the store. The chunk
arrives a second late, nine chunks from whoever sees it.

**It also saves a round to the store**: of the 329 chunks around a player who went,
`A` used to claim every one after a split only to be told that they are `N`'s.

##### 3.6.3 Every way a chunk can be on its way when a split is worked out

The split is worked out when the store has answered the flush behind the second
checkpoint, and with it everything asked before (ADR-0014, section 3.2).

| The chunk is | By `A`'s ticks | Afterwards, by the store | By `A` | By `N` |
|---|---|---|---|---|
| claimed and not answered | asked | cannot be: every claim is answered before the flush; with the store lost, this runner makes no split | | |
| granted, the answer waiting | asked | `N`'s if on the part's side, else `A`'s: the grant was durable before it was answered, and the record of the split moves it | held, or nothing known | held, or nothing known |
| answered "another's", the answer waiting | asked | that region's | nothing known: the answer is dropped | nothing known |
| given back, the return not through | nothing known | cannot be: a flush behind a return is answered when the return is durable. Nobody's | nothing known | nothing known |
| given back and claimed again before the return was through | asked | the store called the return off and answered "granted": as the second row | | |
| claimed by `A` after the split, for a player who went | | never claimed by `A`: section 3.6.2. `N`'s when `N` claims it | believed `N`'s while asked for, then nothing known | held |
| claimed by `A` after the split, for a player of its own | | whoever's claim came first | held, or believed `N`'s on the store's word | |
| claimed by a runner that was fenced | | not done: the store does nothing that a handle asks after its region was taken over (`Lanes::request`) | | |

**No claim of the region that was split is answered across the split.** Its last
claims are answered before the commit is sent, and it makes none while it waits for
the store. What it claims afterwards it claims as the region it is afterwards, by
the tickets it has then.

##### 3.6.4 After every kill

Both regions are, in memory, what `Region::restore` makes of the store's record:
`A` of its state after the split and of the grants the table leaves it, `N` of its
state and of `chunks`. That is so for the grants that wait as for any chunk: what
`A` held by its ticks together with `granted` is exactly what the table grants `A`
outside its pinned areas when the commit is sent (every claim answered, every return
through), and the record takes `chunks` out of it.

| What dies, and when | What the store has | What `A` and `N` are when they run again |
|---|---|---|
| the worker, before the record of the split is durable | no split; the grants that waited are `A`'s | `A` restored holding them all, with every player; no `N`. What the coordinator does about a split whose worker died is ADR-0016's, unchanged |
| the worker, after | the split | both restored from the record: `N` holds `chunks`, `A` the rest |
| the store, at any write or sync of the split | the split or not, as the record is durable or not | the runner has lost the store: `A` is opened again and is the one or the other of the two rows above |
| both | the same | the same |
| the worker, between the split and the edge's hearing of it | the split | `A` is restored without a `Parted`: section 3.6.5's last paragraph |

In none of them does a simulation hold a chunk the store grants another region: a
region that is restored holds what the store says, and nothing else.

##### 3.6.5 What ADR-0015 asked this step to come back to

ADR-0015, section 8, names three things that no rule of the contract with the edge
promises and that "steps C4 and C5 must not undo without coming back here". This
section changes which chunks a part is made with and what a region believes, so:

1. **"A part holds the chunk each of its players stands in."** It does. A seed is a
   chunk the region holds, by its ticks or by a grant that waits, and in both cases
   by the store's table; it is `0` chunks from itself and at least one from every
   chunk of `staying`, which has no seed in it and of which the home chunk is no
   seed, so `sides.goes` is true of it and it is among `chunks`
   (`reshape.rs`, where the seeds are chosen and where the chunks are filtered: lines
   213 to 219 and 234 to 242 today). The store grants the part every chunk of
   `chunks`, and the part is restored holding them. What is new is only that a seed
   may be a chunk no tick of `A` had been told it holds, and that is why the players
   in such a chunk go and are not left behind.
2. **"A stay does not leave a region without an input of this edge."** A region lets
   a player go only where it believes the chunk they stand in another's
   (`region.rs`, the `departing` of `Region::tick`, lines 525 to 535), and the
   beliefs of section 3.6.2 are new beliefs. They cannot move a player who stands
   still: a belief is put in only for a chunk the region knows nothing of that is on
   the part's side, and the chunk somebody of `A` stood in at `T` is in `staying`,
   which is on no part's side. A player
   of `A` who walks, by an input of the edge, into a chunk `A` believes `N`'s is let
   go to `N`, which takes them in whatever it knows of the chunk, and claims it. If
   the belief was the runner's and not the store's, that is a hand-over to a region
   that did not hold the chunk yet, between two players who were within a chunk or
   two of each other when one of them was split off the other by hand.
3. **"A merge announces itself in the survivor's outbox before anything the survivor
   says of a stay that came with it."** Nothing here touches a merge.

**One thing this does not make sure of.** `Parted` is in the runner's memory. A
worker that dies in the two or three ticks between a split and the edge's
confirmation of it leaves `A` to be restored without one, and if `p` crossed a chunk
border in tick `T` as well, the second way is open once. It is the product of two
rare things, costs one more split, and is not mended: mending it means keeping the
line of a split in the region's state, which is a change to what the store has on
disk.

##### 3.6.6 The same fault elsewhere

- **After a merge.** Every chunk either region was granted is the survivor's: the
  absorbed region's by the table (`Table::absorb`, also for a grant whose answer its
  runner never took), the survivor's own grants that wait by `take_absorbed`, as
  built. There is one region afterwards and nothing can lie on a wrong side. A
  `Parted` of the survivor is dropped with the merge, as a belief in a region that
  may have been the one absorbed must not be made.
- **After a move, or any restore.** The next owner holds what the table says,
  grants that the owner before never took among them, and gives back what nothing
  uses thirty seconds later. It is the same region; nobody else's land is made.
- **A guest's asking** makes no region claim anything outside its pinned areas
  (`settle_chunks`), so `A` is granted nothing because `p` looks at a chunk through
  it.
- **A player of `A` who looks that way** sees no further than the reach, and
  whoever was split off was more than `D_s` from every one of them. `A` comes to
  hold land in `p`'s view only when a player of its own has come within twice the
  reach of `p`, and by then the two are to be merged (N3).
- **`N`'s land behind `p`.** The other side of the same rule: what was nearer to
  `p` than to anybody who stayed went with `p`, the far end of `p`'s trail
  included, and `A` kept what was nearer to its own. Between them the row in the
  middle stays `A`'s. All of it lies behind somebody who walks on.

### 4. Everything that reads the layout, and what it reads instead

| Who | Where | What it reads today | Instead |
|---|---|---|---|
| Store | `Division::stripes`, `Division::layout` | the stripes as pinned areas; the fingerprint | `Division::open` or `side_by_side`; nothing |
| Store | `Lanes::admit` | `RegionHello::layout` against `Division::layout`, `StoreError::LayoutMismatch` | nothing: the table says which regions there are |
| Store | `Lanes::load`, `parse_layout` | the `layout` file's fingerprint, to keep a world older than C1 | that the file is there (section 2.2) |
| Store | `lib.rs`: `undivided`, `whole_world` | `Layout::single()` | one area that is everywhere, written out |
| Coordinator | `Coordinator::new` | the regions of the layout | none; the list (section 2.3) |
| Coordinator | `Coordinator::register`, `Refusal::Layout` | the worker's fingerprint | nothing; nobody is refused |
| Coordinator | `routing_table`, `Service::assign` | the layout, for edges and workers | nothing |
| Worker process | `hold`, `holdings`, `stay_registered` | `Orders::layout` for its hellos and its registration | nothing |
| Worker process | `greet_edge` | the whole hello of an edge against its own | region and epoch |
| Edge process | `edge` | `layout.region_of(spawn chunk)` for the home region | `RoutingTable::home` (section 5.4) |
| Edge process | `keep_linked` | the fingerprint for its hellos; `next.layout == table.layout` | nothing; the home region of a later table (section 5.4) |
| Single process | `Server::start`, `Regions` | `Config::boundaries`: the store's division, a runner for each stripe, the home region | all of section 6 |
| Command line | `clustine`, `coordinator`, `worldstore` | `--boundaries` | `--pin` at the store and for the single process; nothing at the coordinator (section 8) |
| Bots | `Ledger::lines`, `Crossing::line`, `--line` | no layout: where a test says a boundary is | unchanged; the comments say what `spot` does with a line |
| Tests | `common::config`, `Cluster::new`, each file | `CLUSTINE_TEST_BOUNDARIES`, `--boundaries`, `Layout` | section 9.2 |
| Manifests | `coordinator.yaml`, `worldstore.yaml` | `--boundaries=4` | nothing; an overlay with `--pin=4` for the kind test (section 8) |

**Deleted**: `Layout`, `LayoutError` and their tests in `clustine-region`;
`RoutingTable::layout`; `RegionHello::layout`; `ToCoordinator::RegisterWorker::layout`;
`FromCoordinator::Assigned::layout`; `FromCoordinator::Refused`, `Refusal`,
`ClientError::Refused` and the worker's `Word::Refused`, which nothing says any more;
`Orders::layout`; `CoordinatorConfig::layout` and `Coordinator::fingerprint`;
`Division::layout` and `Division::stripes`; `StoreError::LayoutMismatch`;
`parse_layout`; `Config::boundaries`; `Regions`, `run`, `run_first` and `started` of
the single process; `--boundaries` in three places; `CLUSTINE_TEST_BOUNDARIES`.

### 5. Messages and types that change

All of it is taking a field away; nothing is added to the wire. A cluster's processes
have to be of one build, as they have to be today: messages carry no version, and one
whose shape has changed is not read.

#### 5.1 `clustine-region`

```rust
pub struct RoutingTable {
    pub version: u64,
    pub spawn: Vec3,
    pub routes: Vec<RegionRoute>,
    pub home: Option<RegionId>,
    pub absorbed: Vec<(RegionId, RegionId)>,
    pub waiting: u32,
}
```

`is_complete` and `route` stay. Breaks: every literal of a table (`client.rs`,
`reports.rs`, the crate's own test), `keep_linked` and `edge`.

#### 5.2 `clustine-rpc`

```rust
pub struct RegionHello { pub region: RegionId, pub epoch: u64 }

ToCoordinator::RegisterWorker { name: String, address: String, holding: Vec<Assignment> }
FromCoordinator::Assigned { spawn: Vec3, assignments: Vec<Assignment> }
// FromCoordinator::Refused goes.
```

`RegionHello` is what a service says first to a worker and, in `StoreHello::Region`,
to the store. Breaks: every literal of a hello (the store's, the worker's and the
runner's tests, `wire.rs`), `greet_edge`'s message of refusal, `WorkerClient::register`
and `register_with_heartbeat` (which lose their last parameter), `Orders`.

#### 5.3 `services/coordinator`

```rust
pub struct CoordinatorConfig { pub spawn: Vec3, pub lease: Duration, pub follow: Option<Policy> }

impl Coordinator {
    pub fn new(config: CoordinatorConfig, now: Instant, first_epoch: u64) -> Self;   // knows no region
    pub fn alone(config: CoordinatorConfig, now: Instant, first_epoch: u64) -> Self; // section 2.3
    #[doc(hidden)]
    pub fn knowing(config: CoordinatorConfig, now: Instant, first_epoch: u64,
                   regions: &[RegionId]) -> Self;                                    // tests only
    pub fn register(&mut self, now: Instant, name: &str, address: &str,
                    holding: &[Assignment]) -> Changes;                              // refuses nobody
    pub fn home(&self) -> Option<RegionId>;                                          // of the last list
    /// Whether it has been handed no list and was not made knowing its regions.
    pub fn awaits_the_list(&self) -> bool;
    /// Whether it gives no worker up for silence (it was made `alone`).
    pub fn keeps_its_workers(&self) -> bool;
}
pub struct Orders { pub spawn: Vec3, pub assignments: Vec<Assignment> }

/// `serve` for a coordinator the caller has made. Tests and `serve_local` only.
#[doc(hidden)]
pub async fn serve_with<L>(listener: TcpListener, coordinator: Coordinator, lists: L)
    -> io::Result<()>
where L: Fn() -> io::Result<RegionList> + Send + Sync + 'static;
```

`released` gains the case of section 2.3. `Service::new` takes the coordinator where
it takes a configuration and a first epoch; `Service::tick` has the list read while
`awaits_the_list()` and no reading is under way, and leaves the connections of
workers alone while `keeps_its_workers()`.

And how a client reaches a coordinator, for the single process:

```rust
/// Where a coordinator is: at an address, or in this process.
#[derive(Clone)]
pub enum Reach { Tcp(String), Local(LocalCoordinator) }

/// The way to a coordinator in this process; clones lead to the same one.
#[derive(Clone)]
pub struct LocalCoordinator { /* a sender of the service's ends of new connections */ }

/// A coordinator in this process and the way to it. The future serves until it is
/// dropped. Its coordinator is made `alone`, with a first epoch from the wall clock
/// as `serve` takes it.
pub fn serve_local<L>(config: CoordinatorConfig, lists: L)
    -> (LocalCoordinator, impl Future<Output = ()>)
where L: Fn() -> io::Result<RegionList> + Send + Sync + 'static;
```

`WorkerClient::register`, `register_with_heartbeat`, `RoutingWatch::connect`,
`Asker::merge`, `Asker::split` and `Mover::ask` take an `impl Into<Reach>` where they
take an address, and `&str`, `&String` and `String` are each a `Reach::Tcp`, so that
no caller of today changes. `Reach::Local` makes a pair with
`link::in_process(QUEUE)` and hands the service its end; `serve_local` is
`serve_with` with those ends where it accepts from a listener and with a
coordinator made `alone`, and is otherwise the same loop: the same `Service`, the
same order of
calls, the same ticks, the same thread for a reading of the list. A local connection
ends when either side drops its end, as a TCP connection does when it is closed.

Breaks: every `CoordinatorConfig` literal and every call of `register` (ten helpers
in the coordinator's tests, `follows.rs` and `reports.rs` under `bin/clustine/tests`),
the tests of `Refusal::Layout`, which go, and every caller of the clients above.

#### 5.4 The edge process

`edge`, `whole_world` and `keep_linked` in `cluster/edge.rs`. No change to
`services/edge`: `Routing::new(home, spawn, identity, links)` is as it is.

- **`whole_world` waits for a table with `home: Some(h)`, a route for `h` and
  `waiting == 0`.** Today it waits for `is_complete()` alone, which a coordinator that
  knows no region yet satisfies with an empty table.
- **The home region is `table.home`** of that table, and stays the edge's home region
  for as long as the edge runs. The home region of a world never changes: it is never
  absorbed, the home chunk never leaves it, and its id is made with the table.
- **A later table is taken whatever it says of home.** `home: None` is a coordinator
  that has started anew and not read the list yet; its routes are as good as any.
  `home: Some(other)` cannot be unless the store was started on a world made over
  while this edge ran; the edge logs `the world has another home region now; this edge
  has to be started anew` once, at the level of a warning, and goes on with the home
  region it has. Today a table with another layout is not taken at all.
- **A hello to a worker is region and epoch.**
- **Links are made through a `Reach` of their own** (section 6.3): over TCP to the
  route's address, as today, or in this process.

#### 5.5 `services/worldstore`

`Division` as in section 2.1; `StoreError::LayoutMismatch` goes; `Lanes::layout`
goes. The table file and the log are as they are: `FORMAT_VERSION` stays, and a
store of this step reads every world a store of today wrote. The line of section
2.2 about workers that live is new.

#### 5.6 `clustine-sim` and `services/worker`

Section 3.6: `Region::split` takes `granted`; `Splitting` has `sides`; `Sides` is
new; the runner hands `split` the grants that wait and keeps a `Parted` for each
split it made. Nothing of it is on the wire or on disk. Breaks: every call of
`Region::split` (the simulation's tests of ADR-0014, the runner) and every literal of
a `Splitting`.

### 6. The single process

#### 6.1 What it is made of

`Server::start` makes, in this order:

1. **The store**: `Store::local_divided` or `Store::memory_divided` with
   `Division::open(home)`, or `Division::side_by_side(home, &config.pins)` if there are
   pins.
2. **The coordinator**: `serve_local(CoordinatorConfig { spawn, lease:
   CoordinatorConfig::DEFAULT_LEASE, follow: config.follow }, lists)`, with `lists`
   the store's own `Store::regions`, on a task. Its coordinator is made `alone`: no
   grace period, and no worker given up (section 2.3). It writes the line that says
   how it reshapes (section 6.5).
3. **One worker**, named `local`, with the address `in this process`: the loop of
   `cluster::worker` (section 6.2), which registers through `Reach::Local`, opens its
   regions with `Store::open_region`, and shows the regions it runs in a watch.
4. **The first routing table that is whole**: `whole_world` through `Reach::Local`
   (section 5.4): a home region, a route for it, nothing waiting. **Then every region
   of that table has to run**: `start` waits until the worker's watch shows each of
   them with the epoch of its route, which is when it is restored and ticks.
5. **The edge**: `Routing::new` with the table's home region and `Edge::bind`, and
   `keep_linked`, which attaches a link in this process to each region the watch
   shows (section 6.3).

**It returns when the edge listens, and every region of the world runs by then**, or
with the error of whichever part ended first. The worker's loop ends with an error
if a region's state cannot be read or the store refuses a region outright, and
`start` then stops what it has started and returns that error (`restoring region 0`,
with the store's reason), **so a world that cannot be restored does not start and
says why, as today**; the first version of this record had it listen and keep
players on the loading screen. A table that is whole has no region without an
owner, and the coordinator of a single process gives out every region of its first
list at once, so "every region of that table" is every region the store has: the
home region, and whatever parts a world on disk had. The links to them are made
within milliseconds of the edge's start, and a join that comes before one waits for
it, as in a cluster.

**So the single process calls the state machine exactly as the coordinator's process
does**, because the same `Service` does the calling: outcomes with the readings they
ask for, then where the players are, then the tick, in the order point 11 of ADR-0016's
"Found while building" asks for; every `Changes::read` answered; heartbeats beside
the reports. ADR-0016 expected the single process to make those calls itself and
listed what it would have to keep to. It makes none.

#### 6.2 The worker's loop, and everything it reaches the outside through

`cluster::worker` is cut in two: `worker(args)`, which is the process, and
`run(setup, outside)`, which is the loop and is given everything it reaches the
outside through. Nothing in `run` names an address, a listener or a signal.

```rust
/// What a worker is, wherever it runs.
pub(crate) struct Setup {
    /// Its name at the coordinator.
    pub name: String,
    /// Where edges are told to reach it.
    pub advertise: String,
    /// Ticks between two checkpoints of each region.
    pub checkpoint_interval: u64,
}

/// Everything a worker's loop reaches the outside through.
pub(crate) struct Outside {
    /// The registration the worker begins with: its client and its first orders.
    pub registered: (WorkerClient, Orders),
    /// Where it registers again when that connection ends.
    pub coordinator: Reach,
    /// Opens a region at the world store, or one that another is to absorb. It
    /// blocks, and is called on a thread that may. `StoreError::Io` says that the
    /// store cannot be reached yet, and the loop tries again; any other error is
    /// the store's refusal.
    pub store: Arc<dyn Fn(RegionHello) -> Result<(StoreHandle, Restored), StoreError> + Send + Sync>,
    /// Where the loop shows the regions it serves: those that are restored and
    /// tick, each with its hello and where links to it are attached. Whoever lets
    /// edges in reads it.
    pub serving: watch::Sender<Serving>,
    /// What the store refused, each a region and the epoch the store has seen, on
    /// its way to the coordinator as `EpochRefused`. The loop puts its own in at
    /// `refusals.0`; whoever holds a clone of that end can say one too.
    pub refusals: (mpsc::UnboundedSender<(RegionId, u64)>, mpsc::UnboundedReceiver<(RegionId, u64)>),
    /// The word to stop: `Leave` has the worker say that it leaves and go on until
    /// it is relieved, for twenty seconds at most; `AtOnce` stops it.
    pub stop: mpsc::UnboundedReceiver<Stop>,
}

pub(crate) enum Stop { Leave, AtOnce }

/// Runs a worker until it is told to stop or cannot go on.
pub(crate) async fn run(setup: Setup, outside: Outside) -> Result<()>;
```

| | The process gives it | The single process gives it |
|---|---|---|
| `registered` | `WorkerClient::register` at the coordinator's address, tried until it answers; **then** it binds its listener, as today, so that a worker that listens is one the coordinator knows | the same through `Reach::Local` |
| `coordinator` | `Reach::Tcp(address)` | `Reach::Local(..)` |
| `store` | `StoreHandle::connect(address, hello)` | `Store::open_region(hello)`, behind the server's lock on its store (section 6.4) |
| `serving` | the sender of a watch whose receiver `accept_edges` has | the sender of a watch whose receivers the link-keeper and the `Server` have |
| `refusals` | a queue nobody else holds | a queue of which the `Server` keeps a sender, for `take_over` |
| `stop` | `Leave` at the first signal, `AtOnce` at the second | `AtOnce`, from `Server::stop` |

**The cut is the whole of step C5.3, and it moves nothing inside the loop.** The
order of the arms of its `select!`, of what each arm does, of what is put into which
watch and queue and when, and of what is stopped and dropped at its end, is where
the last three steps found their ordering mistakes and is not to change by one
line. What becomes a parameter: `args.store` in `open_region` and `fetch`, the
channel `refusals` made inside `worker`, the watch `serving` made inside `worker`,
`crate::stop_signal()` in three places, and `register`. `stay_registered` registers
again through the same `Reach` if its connection ends, which in one process it does
only when the process is being stopped.

**One thing in the loop does change, in step C5.5 and by the same hand as the
single process: a region that is named with another epoch is opened before its
runner is stopped.** When orders name a region this worker **runs** with another
epoch than it runs it with, the loop today takes the region out, stops its runner
in the background and opens the region with the new epoch at the same time. From
C5.5 it opens the region first and keeps the runner aside, running; when the store
has answered that hello, whatever it answered, the runner is stopped, in the
background as today, and a merge or a split it was in the middle of is dropped
behind it as today. If the hello was taken, the runner had been fenced by it: what
it had confirmed is what the next runner is restored with, what it had only
applied was shown to nobody, and stopping it only waits for its thread. That is
the order in which a region goes to **another** worker while its owner lives, so
the loop now has one order for both; and it is what `Server::take_over` did and its
tests are about (section 6.4). In a cluster it is met when a coordinator takes a
region from a worker and gives it to the same one, and when a part is named with
another epoch than it was split off with. A region that is not running yet (it is
being opened, or waits as a part in memory) has no runner to keep aside and is
dropped and opened as today.

**What `Changes` asks of runners is done by that loop, as in a worker process:**

- **A region is started** when the worker's orders name it: opened at the store,
  restored, run on a thread of its own (`Worker::spawn`). The home region so, at the
  start; every region of a world found on disk so.
- **A merge**: the absorbed region is released (its runner checkpoints, stops and
  lets go), the coordinator orders `Absorb`, the loop opens the absorbed region with
  the new epoch, reads its state and hands the survivor's runner `Reshape::Absorb`.
  That the one worker is both the one that releases and the one that absorbs is what
  happens in a cluster whenever both regions are on one worker.
- **A split**: `Reshape::SplitOff` to the runner; the part is run at once from
  memory, opened at the store, and reported.
- **A region is stopped** when it has been absorbed or released, or when the worker's
  orders no longer name it.
- **A move is nothing.** There is one worker: `even_out` finds nobody lighter, and
  nothing asks for a move.
- **A region that has lost the store is opened again**, which the single process does
  not do today. The store of a single process is lost to a region only when a write
  or a sync of the log fails.

**A tick of the world is what it was.** A runner ticks on its own thread at its own
pace, waits for nothing but the store's confirmation of its commits, and is looked
at by the loop four times a second; neither the coordinator nor the loop is in the
path of a tick. What the tests of ordinary play can see differently: the home region
is granted its chunks with a record where the one stripe was answered from the table,
which adds nothing in memory and one sync on a disk; and starting takes the few
milliseconds of a registration and a reading.

#### 6.3 Links in one process

`keep_linked` makes a link to a route in one of two ways: `tcp::connect(address,
hello, ..)` as today, or, in the single process, from the worker's `Serving`: if it
has the region with that very hello (region and epoch), a pair of ends is made
(`link::framed` if `Config::serialise_link`, else `link::in_process`), one is
attached to the region's `Links`, and the other is the link; if not, the attempt has
failed and is made again as a refused connection is, every 20 ms at first. Which link
ended, which owner is new, and when to try again are `keep_linked`'s as they are.

#### 6.4 `Config` and `Server`

```rust
pub struct Config {
    // .. as today, without `boundaries` ..
    /// The chunk x coordinates at which regions are pinned side by side; empty for a
    /// world that is one home region.
    pub pins: Vec<i32>,
    /// What the regions are merged and split by, or `None` for a server that does
    /// neither.
    pub follow: Option<Policy>,
}

impl Server {
    pub async fn start(config: Config) -> Result<Self>;
    pub fn address(&self) -> SocketAddr;
    pub async fn stopped(&mut self);      // one of its parts has ended
    pub async fn stop(self);
    /// The world store's list of regions.
    pub fn regions(&self) -> Result<RegionList>;
    pub async fn take_over(&mut self, region: RegionId) -> Result<()>;
}
```

**`take_over`, which three tests of `takeover.rs` and two of `chaos.rs` rest on**,
keeps its meaning: the region is opened with a higher epoch **while its runner still
runs**, which fences that runner; the runner is not asked and is stopped only when
the store has answered the new hello; the edge resumes with the new runner; nobody
is disconnected. Those tests are about ticks the old runner has applied and the
store has not confirmed, and there are such ticks only if the runner is not stopped
first. How:

1. If the store's list does not have `region`, `take_over` fails with `the world has
   no region …`. **If the region is being opened, it waits until the worker's watch
   shows it running**, and goes on from the epoch it runs with then. (A region that
   was absorbed or is being released meanwhile fails the same way, or after ten
   seconds without running: `region … does not run`.)
2. The server says, through the sender it kept of the queue the worker's loop says
   its refusals through, `(region, seen)` with `seen` one above the epoch the region
   is run with. The worker's client says it to the coordinator as `EpochRefused`.
3. `Coordinator::epoch_refused` takes the region from the worker and gives it away
   again in the same call, with an epoch above `seen`, to the only worker there is.
4. The loop finds its region named with another epoch and, as section 6.2 has it
   from step C5.5, opens it with the new epoch first. That hello fences the runner
   at the store; the runner is stopped when the hello is answered.
5. The edge is given the route with the new epoch, finds its link ended and links
   to the new runner.
6. `take_over` returns when the worker's watch shows the region with an epoch above
   the one it had.

It is a word the store did not say, said for a test; from there on it goes through
the paths a real refusal goes through, and the order of step 4 is the loop's own
for every region that is named with another epoch, in a cluster as here. Nothing in
it is for tests alone but the sender the server keeps. **That an old runner goes on
after it was fenced and harms nothing** is also what
`players_keep_playing_when_a_worker_wakes_up_after_its_region_went_to_another`
(`chaos.rs`) tests between processes.

**`stop` returns when nothing of the server holds the store any more**, so that a
server started on the same directory right after it finds the world as the last
confirmed tick left it and nobody else at it. The server keeps its `Store` behind a
lock that every use takes for as long as the call lasts: the `store` of the
worker's loop, and `lists`. `stop` does, in this order: ends the edge's task and
waits for it, which closes every client and every link; says `Stop::AtOnce` to the
worker's loop and **waits for the loop to return**, by which each runner has
checkpointed and flushed and let go of its handle, every runner that was being
stopped has ended, and every region that was open to be absorbed is closed; ends the
coordinator's task and waits for it; **takes the store out from behind the lock**,
which waits for a hello or a reading of the list that is under way, on whichever
thread, and after which either is answered `the server is stopping`; and drops the
store. The store's own threads end by themselves when the last handle is gone, with
nothing left to write, as today.

#### 6.5 What it prints

What its parts print, in one log: the coordinator's lines (`a region was assigned`,
`the routing table changed`, `a split is begun by itself`, `a merge is begun by the
distances`, `an absorption is begun by itself`, `a merge has ended`, `a worker says
what came of a split`), the worker's (`given a region`, `running a region`, `the
merge has ended`, `the split has ended; opening the new region`) and the edge's
(`linked to a region`, `listening`). Today it prints `listening` and little else. The
lines are the same text as in a cluster, so what the owner is told to look for is
one list for both.

**The line that says how it reshapes is written by the single process itself.** In a
cluster it is the coordinator's process that writes `reshaping by itself: regions
merge and split by where their players are` with `merge_distance`, `split_distance`,
`margin` and `rest_seconds`, or `reshaping by hand: regions merge and split when
somebody asks` (`cluster/coordinator.rs`), and neither `serve` nor `Service` does.
The two lines become one function there, which `Server::start` calls as well, with
the same text. The two lines this step adds, section 3.5's about distances that are
too short and N14's about pinned regions, are likewise written by code that both
run: the first by that same function, the second by the state machine when it is
handed a list.

#### 6.6 The lease in one process

The single process has the coordinator's lease, five seconds, because the service
has one. Everything that hangs on it, and what it does where the one worker is in
the coordinator's own process:

| What hangs on the lease | In a cluster | In the single process |
|---|---|---|
| a worker that has not been heard for a lease is forgotten | its regions go to others | **not done** (`Coordinator::alone`) |
| a region that has not been vouched for within a lease is taken | it goes to another, and its worker is at fault for 30 s | **not done** |
| a connection that is silent for a lease is closed | the worker registers again | **not done for a worker**; a client that never said what it is is still closed |
| the grace period, one lease | nothing that was not let go is given out | **none** |
| how often the state machine ticks | a quarter of the lease by hand, 250 ms by itself | the same: 1.25 s by hand, which is when regions without an owner are given out and what is overdue ends, and little else; 250 ms by itself |
| the list on a timer, every lease, by itself | from the store's address | from the store in the process, which answers at once |
| how old the last list may be before nothing is begun by itself: two leases | holds things back while the store is away | never old |
| a release, a merge or a split that has not ended within a lease | the region is taken, or the merge ends by what the list says | the same. The region can only go back to the same worker, with another epoch, and what became of a merge the list says. It takes a store that needs more than five seconds for a checkpoint |
| no evening out within a lease of a merge or a split | | nothing to even out |

So **the single process never takes a region from its worker because the process was
slow or stopped for a while**. A process that sleeps for a minute wakes with its
worker registered and its regions running; what was overdue in that minute (a merge
that had begun) ends by the list, and is tried again a rest later.

### 7. Every order of events that is new without stripes

`H` is the home region, `P` and `Q` are parts, `p` and `q` their players, `s` a player
who stays. The numbers are for `V` = 8, so reach 9, `D_m` 22, `D_s` 30, unless a
scenario says "at short distances", which is `D_m < 2 * V + 3`. ADR-0016's K1 to K26
hold as they are but for K15, which is gone.

**N1. A part grows.** `p` walks on. `P` claims the chunks that come into view, 19 for
each chunk walked; nobody holds them; each claim is one `Granted` record, answered a
tick or two later; thirty seconds behind, `P` gives them back, one `Returned` record
each time. Nobody stands still and the coordinator begins nothing: `p`'s chunk is the
one place of `P`. The list shows `P`'s `bounds` moving with `p`.

**N2. A part grows into what the region it left gave back, or still holds.** After a
split `H` keeps the chunks that were nearer to those who stayed, among them the far
end of `p`'s trail, and gives them back thirty seconds after the split if nobody looks
at them. **All of it lies behind `p`; ahead of `p` `H` has nothing and is granted
nothing (section 3.6, N17)**, so this is about a `p` who turns round. *Given
back*: `P`'s claim is granted like any other. When `H` is opened the next time, what its log has of such a chunk is not replayed into it, as `H` does not
hold it (ADR-0011, section 3.4). *Still held*: the claim is answered "`H`'s"; `p`
sees the chunk as a guest, which keeps it used and `H`'s for as long as `p` looks;
if `p` walks into it, `p` is handed over to `H` and `P` is left without players. `p`
is then `H`'s player where the trail was. If that is within `D_s` of `H`'s others or
of the home chunk, nothing follows. If not (K14: it takes flight), `H` is split when
it has rested, which stands its players still once more.

**N3. A player walks back.** At 22 chunks from `s`, or from the home chunk whether
anybody is there or not, a merge of `P` into `H` is wanted, stands a second and is
begun when both are free. `H`'s land ends 9 chunks from `s` and `P`'s 9 chunks from
`p`: four chunks lie between them when the merge is wanted, and nobody has seen
across. **If `P` rests** (ten seconds after the split, and ten more if it was moved
to another worker meanwhile) the merge waits; a `p` who turned round at once and flies
can come to see `H`'s land by then, and walk into it: guest, then hand-over, and
`P` is left empty. Either way `p` ends as `H`'s player.

**N4. Two parts grow towards each other.** At 22 chunks they are merged: the one with
more players survives, else the lower id. *At short distances* their lands meet
first, at twice the reach. Each claims what it sees; a chunk is whoever's claim came
first, and the line between them is where the claims met. `p` sees `Q`'s chunks as a
guest and walks into one: handed over to `Q`, far from `q`. If that is further than
`D_s`, `Q` is surely apart and is split when it has rested: the larger group stays,
so with one player each the one in the lower chunk stays and the other is split off
into a new part, whose land begins halfway between them; they walk on, and the next
hand-over is within `D_s`. **At short distances a meeting is a hand-over, a split and
a hand-over**, and one stop for whoever was there. That is what section 3.5's line in
the log says.

**N5. A region loses its last player while a guest looks at it.** `p` leaves the
game; `q`, of `Q`, sees some of `P`'s chunks. `P` keeps those and gives the others
back. Nothing merges `P` and `Q` by the distances: `P` has no place. After thirty
seconds `P` is absorbed, by a region without players: `H` if it has none, else the
lowest empty region below `P`, else it stays. The chunks `q` looks at are that
region's from then; the edge asks it for them as it asked `P`; `q` does not stand
still. If `q` walks into them, `q` is handed over to whoever holds them. If that is
`H`, far from the home chunk, `H` is split a second later and `q` is in a part again,
having stood still once. At the default distances this needs `p` and `q` to have been
within 18 chunks of each other in two regions, which is a merge that was late; at
short distances it is the rule.

**N6. The home region left by everybody.** A group walks more than 30 chunks from the
home chunk and is split off. `H` has no player and goes on holding the home chunk,
and for thirty seconds the near end of the group's trail. A group that comes back
within 22 chunks of the home chunk is merged into `H`. One that lingers at the rim is
split once, at 31 chunks, and merged again only 9 chunks further in.

**N7. A player joins while the home region is merged or split, or has no owner.**
The join is kept by the edge and sent when the link to `H` is welcomed again: after a
fifth of a second for a merge or a split, after the lease and a moment if `H`'s
worker died. The player is on the screen that says the world is loading for that
long. They are disconnected only after 20 seconds without `H`. They are placed in the
home chunk, which `H` always holds, and never in a part. If another region holds
chunks they see (its player came near a moment ago and the merge has not stood yet),
they see those as a guest.

**N8. A player leaves and joins again.** They leave `P`, which is left without
players, and enter `H` at the spawn point. `P` gives its land back within thirty
seconds and is absorbed after that, by `H` if `H` has nobody else, by the lowest
empty region otherwise, or stays as that region.

**N9. A worker dies with a part that has just grown.** Every grant was on disk before
it was answered. The lease runs out, another worker is given `P`, the store restores
it with `held` as the table has it, among it a grant whose answer the dead worker
never read; `P`'s players are where the last confirmed tick had them and have stood
still for five to seven seconds. What `P` held and nobody uses any more is given back
thirty seconds after the restore. A claim that was not written is made again by the
first tick that wants the chunk.

**N10. The store is away while a region asks for chunks.** The region loses its
handle, stops and closes its links; its worker opens it again when the store answers,
and its players stand still until then, as they do today when the store is away. What
it had asked for and was not granted, it asks for again. In the single process this
is a failed write of the log, after which every region is opened again.

**N11. A hundred lone players.** They leave the spawn point in a hundred directions.
The first split takes every group that has stood, into one part; that part is split
again when it has rested, the group with the most players staying, and so on, one
split in the world at a time: ADR-0016, K9, with its quarter of an hour until the
last is by itself, and open question 2 there for what would shorten it. Off stripes
each of those parts grows as its players walk, so its land is a hundred islands and
not a block. A hundred regions are a hundred threads on however many workers there
are, evened out one release at a time between the splits. When the hundred leave the
game, their regions are absorbed in pairs, about half of what is left each half
minute.

**N12. The first reading fails.** The coordinator knows no region; workers register
and are given nothing; the edge waits. The service reads again at its every tick
until the store answers, then the home region is assigned, when the grace period is
over. If the store is away for good, nothing starts, and the coordinator's log says
`the world store's list of regions cannot be read`.

**N13. A coordinator starts anew while the store is away.** Workers register and
report what they run, which it believes. A region whose worker died meanwhile is
reported by nobody and is in no list it has: its players stand still. When the store
answers, the list names the region, it is assigned after the grace period, and its
players go on, if that was within the edge's patience of 20 seconds. On stripes the
layout named it and it was assigned a lease after the coordinator started, store or
no store; but no worker could open it without the store.

**N14. A world of pinned regions under a coordinator that decides by itself.** As in
step C4: pinned regions merge by the distances, a part cannot grow, K15. The
coordinator logs once for every reading that first shows a pinned region: `the world
has regions that are pinned to an area: a region that is split off here cannot grow.
Start the coordinator with --reshape by-hand to keep pinned regions as they are`.
`follows.rs` goes on testing exactly this.

**N15. Somebody stands in a chunk that is asked for when a split is ordered.** When
the split is worked out, the claim has been answered. *Granted*: the chunk counts as
held, they are a seed and go, and the part holds the chunk (section 3.6.1; as built
they stayed behind, alone). *Another region's*: they are no seed, and the next tick
lets them go to that region; with nobody else in the chunks named the answer is "not
yet" (ADR-0016, section 5.5). Off stripes either needs a claim that is seconds late,
as a chunk is asked for nine chunks before anybody stands in it.

**N16. A world from before.** Section 2.2: made over before any hello, and then N12
and what follows. **In a cluster whose workers and edge were not stopped**: every
player is disconnected when their region is opened again without them; a worker
that ran a region the world no longer has ends with an error and, where something
starts it again, registers holding nothing; the coordinator's next reading drops the
regions that are gone and adds the new ones. The store's log says both things
(section 2.2).

**N17. A player walks on through their own split.** `p` flies east and does not
stop. One or two seconds after `p` has passed 30 chunks from `s`, `H` is split:
everything nearer to `p` than to `s` and to the home chunk goes to `P`, **the chunks
`H` was granted for `p`'s view in its last tick or two among them** (section 3.6.1).
The edge, which has not read of the split yet, asks `H` again for all that `p` sees;
`H` claims none of it, and answers that it is `P`'s (section 3.6.2). The edge reads
of the split, asks `P`, and `P` is granted the row that nobody held. `p` flies on
through chunks that are `P`'s or nobody's, and `P` is granted each row as it comes
into view (N1). **No hand-over, no second split, and nobody at the spawn point
stands still again**, however fast `p` is and whichever tick the split fell on. Two
players who leave together in two directions are split off together and parted from
each other a rest later, by the same rule.

**N18. A coordinator starts anew after a region was let go, and has not read the
list.** The coordinator is killed in the middle of a move; the old owner finishes
releasing region `r`; a new coordinator starts; the worker registers, holding
nothing, which has the service begin its reading anew; and the worker's `Released
{ r }` is heard before that reading is back. `r` is noted on the worker's word,
let go, and given to a worker by that call, grace period or not (section 2.3). Its
players stand still for a move and not for a lease. The list that comes then has
`r` living and changes nothing.

### 8. The flags of every process, and the manifests

```text
clustine [--bind A] [--description T] [--max-players N] [--compression-threshold N]
         [--view-distance V] [--world DIR] [--checkpoint-interval S]
         [--reshape by-itself|by-hand] [--merge-distance N] [--split-distance N]
         [--rest-seconds N] [--pin X[,X…]]

clustine coordinator [--listen A] [--lease-seconds N] [--store HOST:PORT]
                     [--reshape by-itself|by-hand] [--view-distance V]
                     [--merge-distance N] [--split-distance N] [--rest-seconds N]

clustine worldstore  [--listen A] [--world DIR] [--pin X[,X…]]

clustine worker, edge, move, merge, split: as they are.
```

- **`--reshape` is `by-itself` unless told otherwise**, for the coordinator and for
  the single process. `by-hand` is a world whose regions stay as they are unless
  somebody asks: with `--pin`, the regions of the tests; without, one home region
  that holds whatever its players see.
- **The single process takes the coordinator's reshaping flags**, with the same
  refusals (ADR-0016, section 8). It has one `--view-distance`, which is what its edge
  grants and what its distances follow from; they cannot disagree there.
- **`--pin`** is section 2.1's, for the store and for the single process. It is
  refused unless the coordinates ascend without repetition, with `--pin takes chunk x
  coordinates in ascending order without repetitions`.
- **`--pin` does not change `--reshape`.** Pinned regions under a coordinator that
  decides by itself are N14, and say so in the log. Whoever wants the regions of
  before says both: `--pin 4 --reshape by-hand`.
- **The distances are checked against the view distance**, by the command line that
  knows both: section 3.5's line in the log.
- **`clustine move --region`**: its help says "regions are numbered from 0, from west
  to east". Regions are numbered as the store makes them: a new world's home region
  is 0, and the routing table in the coordinator's log names the others.

**`--boundaries` is refused**, by the command line, with exit code 2 like any other
argument it refuses, and one of these sentences:

| Where | What it says |
|---|---|
| `clustine --boundaries …` | `--boundaries is no more: regions follow their players now, and a world begins as one. For regions pinned side by side as before, say --pin 4 --reshape by-hand (with your coordinates for 4).` |
| `clustine coordinator --boundaries …` | `--boundaries is no more: the coordinator learns which regions there are from the world store. Regions pinned side by side are the store's to be told (clustine worldstore --pin 4); say --reshape by-hand here if they are to stay as they are.` |
| `clustine worldstore --boundaries …` | `--boundaries is --pin now: --pin 4 pins two regions side by side at chunk x = 4. Without it the world is one home region, and regions follow their players.` |

The flag stays in the parser, hidden from the help, so that it is these sentences and
not "unexpected argument".

**The manifests.** `deploy/kubernetes/coordinator.yaml` and `worldstore.yaml` lose
`--boundaries=4` and the comments about it; the coordinator keeps `--store`. A
cluster deployed from them is a world of one home region whose regions follow their
players, on three workers. `worker.yaml`'s comment ("one for each region and one to
spare") says instead that the workers share whatever regions there are. For the
tests that want a boundary at block x = 64 there is an overlay,
`deploy/kubernetes/test/pinned/kustomization.yaml`, which is the base with `--pin=4`
added to the store's arguments and `--reshape=by-hand` to the coordinator's.

**The base loses `--boundaries=4` in step C5.6, in the commit that adds the
overlay**, and not a step later with the rest of this paragraph: an overlay that
adds `--pin=4` to a base that still says `--boundaries=4` would give the store both,
and the `Cluster` workflow runs on that commit. So between C5.6 and C5.7 the base
is a world of one home region that is reshaped by hand, as a coordinator told
nothing still does then, and **nothing deploys the base in that time but the kind
test, through its overlay**. From C5.7 the base is the world that follows its
players, and the kind test deploys it as well (G1).

### 9. Building it

`main` is green at every commit and every test there is passes at every commit; the
layout is taken away last, when nothing reads it. The steps that can be built side by
side own files that no other step touches. What reaches into the edge's process, and
the single process, is built by one person and is not delegated.

#### 9.1 The steps

| # | Scope | Files | By | Its tests |
|---|---|---|---|---|
| C5.0 | This record reviewed, and revised. **Then two commits that change no behaviour**: `bin/clustine/src/cluster.rs` cut into `cluster/coordinator.rs`, `worldstore.rs`, `worker.rs`, `edge.rs` and `commands.rs`, moved and not changed (**done**, `23a6c3b`); and the shared types of section 5.3 (`Reach`, `LocalCoordinator`, `serve_local`, the clients taking `impl Into<Reach>`, so that no caller changes). `serve_local`'s coordinator is made with `new` until C5.2 gives it `alone` | `bin/clustine/src/cluster*`, `services/coordinator/src/{client,service,lib}.rs` | the lead | Q9; all four checks |
| C5.1 | The store: `Division::open`, `Division::side_by_side`; tests of what `Lanes::load` already does with them, a world with a `layout` file and no table among it, which is made over today whenever the store is told no fingerprint; the second line of the log when a world is made over (section 2.2). `Division::layout` and `stripes` stay until C5.9 | `services/worldstore` | delegated | T1 to T5, T7, T9 |
| C5.2 | The coordinator: `new` knows no region; `knowing`; `alone`; `home()`, `awaits_the_list()`, `keeps_its_workers()`; `released` before a list; `serve_with`; the service reads until it has a list; N14's line. **Until C5.9 `serve` makes its coordinator `knowing` the stripes of a `CoordinatorConfig::layout` that has a boundary**, so that a coordinator started with `--boundaries` is as it was; one started without, and `serve_local`'s, knows no region | `services/coordinator` | delegated | Q1 to Q8, Q10, Q12 to Q14; every test of the crate, the state machine's helpers on `knowing` and the service's on `serve_with` |
| C5.3 | The worker's loop cut from its process (section 6.2): `run(setup, outside)`, and `clustine worker` gives it what it has today. **Nothing inside the loop moves**: its builder is told that the order of the arms, of what each does, of what goes into which watch and queue, and of what is stopped at the end, is not theirs to change, and that a change they think is needed is to be reported and not made | `cluster/worker.rs` | delegated, with tests written by someone else, and **read line by line against the file before it** when it comes back | the tests of `cluster/worker.rs` that drive the loop, unchanged in what they assert; P7 |
| C5.3a | **A split leaves no land ahead of those who go** (section 3.6), in two commits: the grants that wait go by nearness (`Region::split` with `granted`, `Sides`, `RegionRunner::commit` and `take_answer`); then what an edge asks for before it has heard of the split (`Parted`). No edge code, no store code | `crates/clustine-sim/src/region/reshape.rs`, `services/worker/src/lib.rs` | the first commit delegated; **the second by the lead, or delegated and read line by line**: it changes what a region answers an edge that resumes | S1 to S9, R1 to R9, written from sections 3.6 and 9.5a by someone else; the 103 tests of ADR-0014's section 2.6 and the runner's tests of merges and splits; all four checks |
| C5.4 | The edge's process: `whole_world` waits for the home region; the home region from the table; a later table taken whatever its layout; links in this process (sections 5.4 and 6.3). The hello still carries the fingerprint | `cluster/edge.rs` | **the lead; not delegated** | every end-to-end test; `cluster.rs`; Q11 |
| C5.5 | The single process on the parts (section 6): `Config::pins` and `follow`; `Server::start` that returns when every region runs, `regions`, `take_over`, `stop`; **the loop opens a region that is named with another epoch before it stops its runner** (section 6.2), a commit of its own; the line that says how it reshapes; the flags `--pin`, `--reshape` and the distances for it and `--pin` for the store, **with `--reshape by-hand` still what both do unless told, and `--boundaries` still taken, as `--pin`**. `common::config()` turns `CLUSTINE_TEST_BOUNDARIES` into pins | `bin/clustine/src/{lib,main}.rs`, `cluster/worker.rs`, `tests/common/mod.rs` | **the lead; not delegated** | every test of the single process in both runs; P1 to P6, P9, P10; T6 but for the refusals; `chaos.rs` and `moves.rs`, for the loop |
| C5.6 | The tests leave `--boundaries` (section 9.2): `Cluster::new` passes `--pin` to the store where it has pins, and then `--reshape by-hand` to the coordinator; `CLUSTINE_TEST_PINS`; `tools/check.sh`, `ci.yml`, `CLAUDE.md`'s four lines; the one test of `reshapes.rs` that needs the layout rewritten; **the base manifests lose `--boundaries=4`, the overlay is added**, and the kind test runs on the overlay (section 8) | `bin/clustine/tests`, `tools/check.sh`, `.github/workflows/ci.yml`, `deploy/` | delegated | themselves; `deploy/kind/test.sh` on GitHub |
| C5.7 | **`by-itself` is what both do unless told.** `common::config()` follows the players when it has no pins. The comments of the manifests; section 3.5's line; the kind test's new part (G1) | `main.rs`, `cluster/coordinator.rs`, `tests/common/mod.rs`, `coordinator_flags.rs`, `deploy/` | the lead | the lines of P8 that need no refusal; G1; every check |
| C5.8 | End to end without stripes, under the bots, **written from this record by someone who built none of the above**: W1 to W11, the chaos and the moves without pins (Y1 to Y6), X1; what they measure goes into the roadmap | `bin/clustine/tests/wanders.rs`, `crowds.rs`, `chaos.rs`, `moves.rs`, `common/` | delegated | themselves, ten runs in a row |
| C5.9 | **The layout goes**: everything "Deleted" in section 4; `--boundaries` refused; `serve` makes its coordinator with `new`; **`README.md` (three places) and `CLAUDE.md` ("is what they run") name the commands there are**, `cargo run -p clustine` for what the owner runs and `--pin` where a boundary is meant, in this commit, as the flag they name exits with 2 from here on | every crate, `README.md`, `CLAUDE.md` | the lead | T8, Q8, the refusals of T6 and P8; every check; the comparisons with the official server; kind |
| C5.10 | Only if X1 says so (section 9.7): the survivor of a merge and the region of a split keep their links. A record of its own first | `services/edge`, `services/worker` | **the lead; not delegated** | its own |
| C5.11 | Roadmap: where M3 stands, what was measured, section 10 for the owner, and that the tests of chaos and of moves are pinned on purpose but for Y1 to Y6. Documentation proper is step C6 | `docs/roadmap.md` | the lead | CI |

C5.1, C5.2, C5.3 and C5.3a are built side by side once C5.0 is pushed: each owns
files no other of them touches. C5.4 and C5.5 follow in that order; C5.6 can be
prepared beside them and lands after C5.5. C5.3a has to be in before C5.8, whose
tests it is there for, and can be in at any time before: it changes nothing a
pinned world shows. The scenarios T, Q, P, S and R are written by somebody other
than the builder of each step, from this record, and can be written while the step
is built.

**What the builder of C5.3a is told.** The interface of section 3.6.1, as code; that
`Region::split` stays a function that changes nothing, and that every equality of
ADR-0014, section 2.6, has to hold as it does (a region after `take_split` and the
part each equal what `Region::restore` makes of the same state and holdings); that
`waiting_for_the_tick` is called before the region takes the split and why; that the
store, the edge and the coordinator are not theirs to touch, and that a `Decline`
from the store for a chunk of the part means the design is wrong and is to be
reported; the three conditions of section 3.6.5; and, for the second commit, the
five steps of section 3.6.2 with the functions named there, of which the edge's are
to be read and not changed. How to verify: the scenarios, then the four checks.

**What is true between the steps.** From C5.2 to C5.9 a coordinator's process that is
given `--boundaries` still knows its stripes from them, and reads the list as well;
from C5.6 no test gives it any, so every cluster of the tests has a coordinator that
learns its regions from the list three steps before the layout goes. From C5.5 the
single process runs on the coordinator's state machine whether its world is pinned
or not; a single process that reshapes by hand with no pins is one home region.
From C5.6 no test names `--boundaries`, and the manifests are one home region
reshaped by hand. From C5.7 a process started without flags follows its players.
From C5.9 `--boundaries` is refused, and nothing in the repository names it but the
records and the sentences that refuse it.

#### 9.2 What each existing kind of test rests on, and keeps

| File | Rests on | After `--boundaries` |
|---|---|---|
| `blocks`, `join`, `movement`, `oracle`, `persistence`, `players`, `status` | nothing of regions; run twice, the second time on boundaries at 0 and 4 | first run: the default, one home region that follows its players (at the tests' view distance of 3 the distances are 12 and 20 chunks, and the farthest any of them walks is 18, so nothing is split); second run: `CLUSTINE_TEST_PINS=0,4`, three pinned regions, by hand |
| `handoff` | a boundary at block x = 16 or 48; a world on disk opened with other boundaries, or none | `pins: vec![1]` and `follow: None`; the restart with `[1]`, none and `[-2, 0, 5]`, each made over or found as it was; the killed server started with `--pin 1 --reshape by-hand` |
| `takeover` | two pinned regions, ids 0 and 1, `Server::take_over` | `pins: vec![1]`, `follow: None`; `take_over` as section 6.4 |
| `sending` | boundaries at chunk 3 or 4, or none; how many lines a bot crossed | `pins` of the same, `follow: None`; "none" is one home region by hand |
| `chaos` (clusters) | `--boundaries 3` or `2,3`; regions numbered from west to east | `--pin` of the same at the store, `--reshape by-hand`; the ids are the same |
| `chaos` (single process) | `boundaries: vec![3]`, `take_over` | `pins: vec![3]`, `follow: None` |
| `moves`, `merges`, `cluster` | `--boundaries 3` or `2,3`; two or three regions that stay unless asked | `--pin`, `--reshape by-hand` |
| `reshapes` | the same, and in one test **a coordinator that knows region 1 from its layout while the store is away** | `--pin`, `--reshape by-hand`; that one test rewritten (below) |
| `follows` | `--boundaries 4` with `--reshape by-itself --merge-distance 3 --split-distance 5 --rest-seconds 5`; what pinned regions do under it | `--pin 4` at the store and the same coordinator flags, said in full. It goes on checking deciding by itself on pinned regions (N14); the world without pins is `wanders.rs` |
| `follows` (in memory), `reports` | a `Layout` in a `CoordinatorConfig`, in `Assigned` and in a `RoutingTable`; `register(.., None)` | the fields and the parameter gone; `reports` gives its store `--pin 4` |
| `coordinator_flags` | `by-hand` as what a coordinator does unless told | the other way round: `by-itself` unless told, with the default distances in its line of the log |

`Cluster::new(directory, workers, pins)` keeps its three parameters, the third a
list of chunk x coordinates separated by commas as today. **With pins** it passes
`--pin <pins>` to the store, and `--reshape by-hand` to the coordinator unless
`coordinator_arguments` has a `--reshape` of its own. **With an empty `pins` it
passes neither**: the store is started without `--pin`, an empty value being no
list of coordinates, and the coordinator with nothing about reshaping, so that it
does what a coordinator does when told nothing. That is by hand until step C5.7 and
by itself from then on, and it is the world `wanders.rs`, `crowds.rs` and Y1 to Y6
run on: they test the default by not naming it. No test of today passes an empty
`pins`.

**The pinned tests stay pinned.** Every test of `chaos.rs`, `moves.rs`, `merges.rs`,
`reshapes.rs`, `follows.rs`, `handoff.rs`, `takeover.rs` and `sending.rs` that
there is today goes on running on pinned regions, by hand unless it says otherwise,
because a boundary at a known place is what each of them is about: a bot that
crosses it, a region with a number, a move of that region. That is said here and
goes into the roadmap (C5.11), as the agreed plan has C5 verified by "every chaos
and move test again" and a reader would take that for the default world. What
chaos and moves do to a world without pins is tried by W5 to W8 and by a selection
of those tests on such a world, Y1 to Y6 (section 9.6).

**The one test that is rewritten**:
`a_worker_that_is_given_a_region_that_was_absorbed_drops_it_and_says_so`
(`reshapes.rs`). It has a new coordinator give out region 1, which was absorbed,
because the store is away and the coordinator goes by its layout. No coordinator does
that any more. What the test checks is the worker's side: that a worker which is
given a region the store has as absorbed drops it and says `AbsorbEnded` with `Ok`
for the region it went into. It keeps that by playing the coordinator, as
`reports.rs` does: a worker with regions 0 and 1 of a store with `--pin 3` is told to
release region 1 and to have region 0 absorb it, and is then given region 1 again
with a higher epoch.

**The fourth line of the checks** becomes `CLUSTINE_TEST_PINS=0,4 cargo test -p
clustine --locked`, in `CLAUDE.md`, in `tools/check.sh` (its log `pinned.log`) and in
`ci.yml`. What it is for has not changed: every test of ordinary play once more with
a boundary through the spawn point and one where the longer walks cross. The first
run is now also the first to try ordinary play on a region that is not pinned. The
tests that divide their worlds themselves skip the second run by the new name.

#### 9.3 Scenarios: the store

Written from sections 2.1 and 2.2. "Opened" is a hello with an epoch above every one
used before.

- **T1.** A new world started with `Division::open(home)`: the list is `home: 0`, one
  region, id 0, `pinned` empty, `bounds` the home chunk alone, `next: 1`, no absorbed
  pair. The table file has no area. Region 0 is opened and has a block of entity ids;
  it is restored with `held` the home chunk at tick 0.
- **T2.** In that world region 0 claims twenty chunks: all are granted, each is in
  `held` after any kill, and a return of nineteen of them and of the home chunk frees
  the nineteen and not the home chunk.
- **T3.** A world of two stripes that was lived in (commits in the log, a state file,
  a saved chunk, and a part split off one stripe that holds granted chunks and has
  changed blocks in them) is started with `Division::open(home)`, killed at every
  write and sync of that start and with everything a crash can keep: every start on
  what is left gives T1's list but for `next`, which is the old table's or higher;
  every block that was confirmed is in the chunk as region 0 loads it once it has
  claimed it; region 0 is restored without a state; no `layout` file is left; a second
  start changes nothing.
- **T4.** The other way round: T1's world with a part, started with
  `Division::side_by_side(home, &[4])`: two pinned regions, ids 0 and 1, home the one
  with the home chunk, the part gone, its blocks in the chunks.
- **T5.** A world with a `layout` file and no table, whatever the file says, started
  with any division: made over as T3; the file is gone when the table is durable.
- **T6.** The command line: `--pin 4`, `--pin 0,4`, `--pin -2,0,5` start a store with
  two, three and four pinned regions; `--pin 4,4` and `--pin 5,4` are refused with
  the sentence of section 8; `--boundaries 4` is refused with its sentence on each of
  the three commands, exit code 2 (from C5.9).
- **T7.** A world made with the stripes of a layout cut at 4, lived in, started with
  `Division::side_by_side(home, &[4])`: found as it was, the table file not written.
- **T8.** A hello is a region and an epoch; one for a region the table does not have,
  for one that was absorbed, and with a lower epoch is refused as today.
- **T9.** Whenever a start makes a world over (T3, T4, T5), the log has both lines of
  section 2.2, once each; a start that finds the world as it was (T7) has neither.
  After T3's start, a hello for region 1 of the world of stripes is refused as for
  a region the table does not have, and one for region 0 with the epoch stripe 0
  was last opened with is taken and restores a region without a state.

#### 9.4 Scenarios: the coordinator and its service

Written from sections 2.3, 5.3 and 5.4. `tick` and the other calls are the state
machine's; "the service" is `Service` driven by hand, as its tests drive it.

- **Q1.** A coordinator made with `new`: its routing table has no route, `waiting:
  0`, `home: None`; a worker that registers holding nothing is assigned nothing, also
  after the grace period; `merge` and `split` are refused with `NoSuchRegion`.
- **Q2.** It is handed a list with the home region 0 and nothing else: region 0 is
  known, without an owner, `home()` is 0, the table has `waiting: 1`; it is assigned
  to a registered worker by the first tick at or after the end of the grace period
  and by nothing before.
- **Q3.** The same with `alone`: assigned by the call that hands in the list, if a
  worker is registered by then, and otherwise by the first tick or list after one
  has registered.
- **Q4.** A worker registers holding region 3 before any list: it owns region 3 on
  its word. A list that shows region 3 living leaves it so; one that shows it
  absorbed, or has `next` above 3 and no region 3, takes it away.
- **Q5.** The service, deciding by hand, whose `lists` fails: it reads once when it
  is made and once more at every tick at which no reading is under way; after the
  first reading that succeeds it reads at no tick. Deciding by itself: the same until
  the first list, and then as ADR-0016, section 7.
- **Q6.** N13: a coordinator made anew, one worker registers holding region 0, no
  list can be read for longer than the grace period: region 1 is not known and not
  assigned. Then a list with regions 0 and 1 is read: region 1 is added and assigned
  by that call, the grace period being over.
- **Q7.** Deciding by itself, nothing is begun before the first list, whatever
  workers report; and with a list that shows a pinned region the line of N14 is
  written once, and not again for a later list that shows the same regions pinned.
- **Q8.** `register` takes no fingerprint and refuses nobody; a registration says
  `Assigned` with the spawn point and the assignments (from C5.9).
- **Q9.** A worker that registers through `Reach::Local` is answered, heard and told
  its orders as one over TCP; a `RoutingWatch` through it is sent every table; the
  service closes a local connection as it closes any, and the client's next call
  fails as it does when a TCP connection has ended.
- **Q10.** `serve_local`'s coordinator assigns a region of the first list to a worker
  that is registered without waiting for a lease. (Of step C5.2, which makes that
  coordinator `alone`; the review found it listed under C5.0, which has no such
  coordinator yet.)
- **Q11.** The edge's process, against a coordinator that is played: it does not
  listen for players while the table has no home region, or no route for it, or a
  region waiting; it does when all three hold; a later table with `home: None` is
  taken for its routes; a join goes to the home region of the first table.
- **Q12. A release before the first list** (section 2.3, N18). A coordinator made
  with `new`, within its grace period, handed no list; worker `a` registers holding
  nothing, worker `b` too. `a` says it released region 5 with epoch 40.
  1. Region 5 is known, and by that very call has an owner, `b` (`a` is behind it),
     with an epoch above 40; the routing table has a route for it.
  2. With `a` alone registered, the owner is `a`, with an epoch above 40.
  3. With no worker that can be given it (`a` has said that it leaves and `b` is not
     there), region 5 is known without an owner and the table has `waiting: 1`. If
     `b` then registers holding region 5 with epoch 41, `b` owns it with 41; holding
     it with epoch 39, `b` may not go on with that, and is given the region anew,
     with an epoch above 40, by the next call that gives regions away (a tick at
     the latest; the region was let go, so the grace period does not hold it).
  4. After 1, a list that has region 5 living changes neither owner nor epoch. A
     list that has `(5, 2)` among the absorbed, or `next: 9` and no region 5, removes
     it: its owner's orders no longer name it. A list with `next: 4` leaves it.
  5. The same word after a list was handed in that does not have region 5: nothing
     changes, as today. The same word from a name that is not registered: nothing.
  6. A region the coordinator knows without an owner (the list named it) and that
     is released so: as today, by the case there is.
  And through the service, with a `lists` whose answer the test holds back: a worker
  registers, says `Released`, and is given the region before the list is handed in.
- **Q13. `alone` gives nobody up.** A coordinator made `alone` with one worker that
  owns a region. Nothing is heard of the worker for ten leases, and then a tick:
  the worker is registered, owns the region with the epoch it had and is not at
  fault. The same with heartbeats that name no region for ten leases. The same
  made with `new`: the worker is forgotten, and the region taken. The service that
  serves an `alone` coordinator closes the connection of no worker for silence, and
  closes one that never said what it is.
- **Q14. A coordinator made `knowing` is read for on events only.** Served through
  `serve_with` with a `lists` that counts its calls and fails: one call when it is
  served, one for each registration, none at any tick; `awaits_the_list()` is false.
  One made with `new` and served the same way: a call at every tick at which none
  is under way.

#### 9.5 Scenarios: the single process

Written from section 6. A `Server` in the test's own process, observed through its
address and `Server::regions()`.

- **P1.** Started on a new world without pins: **as soon as `start` has returned**,
  `regions()` is T1's list with an epoch above 0 for region 0, as the region runs by
  then; a bot joins and is sent `view_area` of the spawn chunk. Started on a
  directory whose state file of region 0 cannot be read: `start` returns an error
  that names the region, nothing listens, and a second start on a directory that is
  in order works.
- **P2.** With `pins: vec![1]` and `follow: None`: two regions, pinned; a bot walks
  across block x = 16 and back and is one entity to a watcher. (`handoff.rs` has it.)
- **P3.** `take_over(r)`: `regions()` shows a higher epoch for `r`; a bot in `r` is
  not disconnected and what it was acknowledged is there. **Called right after
  `start`, and twice in a row without waiting for anything**: both return, each with
  a higher epoch (`a_world_whose_region_changed_hands_is_served_by_the_next_server`
  does the first). `take_over` of a region the world does not have is an error.
  (`takeover.rs` has the first sentence.)
- **P4.** Started on the disk of a world that had three parts when it was stopped,
  with a `follow` whose rest is one second: `start` returns with all four regions
  running; within twenty seconds `regions()` has the home region alone, and the
  parts in `absorbed`.
- **P5.** Stopped and started again on the same directory, **fifty times in a row,
  each start right after the `stop` before it returned**, a bot placing a block and
  seeing it acknowledged in every life: every start works, and after the last every
  block that was acknowledged is there. With a `follow` whose rest is one second and
  a second bot that is split off in every life, so that `stop` meets parts, readings
  of the list and now and then a merge under way. (`persistence.rs` has one round.)
- **P6.** Stopped while a bot is connected: `stop` returns, and does not wait twenty
  seconds for another worker.
- **P7.** The worker's loop, given a store in its process and a coordinator through
  `Reach::Local`, runs a region it is assigned and shows it in its watch of the
  regions it serves; a link attached there is welcomed. Told `Stop::AtOnce`, it
  returns, and the store has what its regions had done.
- **P8.** `clustine --boundaries 4` exits with 2 and the sentence of section 8; so do
  the two subcommands (from C5.9). **`clustine` started with nothing logs `reshaping
  by itself: regions merge and split by where their players are` with
  `merge_distance=22 split_distance=30`, and with `--reshape by-hand` it logs
  `reshaping by hand: regions merge and split when somebody asks`**; the
  coordinator's process logs the same two (`coordinator_flags.rs` has those).
  `clustine --pin 4` logs N14's line when it has read its list; `clustine
  --merge-distance 3 --split-distance 6` logs section 3.5's.
- **P9. A region that is taken over is fenced while it runs.** A single process with
  `serialise_link` and a store whose answers to commits the test holds back (the
  store of `takeover.rs`, if it has the means, or one given to the worker's loop
  for the test): a bot digs a block; the tick that applied it is not confirmed and
  the bot is not acknowledged; `take_over`; the store's answers are let go. The bot
  is acknowledged once, by the new runner; the old runner has ended as having lost
  the store, not as stopped; and the block is gone. If no such store can be put
  under a `Server` without a door for tests, this stays a test of the worker's loop
  as P7 drives it, and the record says here what `takeover.rs` then shows: that a
  takeover loses and doubles nothing, whichever of the two orders it met.
- **P10. A process that was held up keeps its regions.** The single process as a
  process of its own (`spawn_server`), a bot connected and playing; the process is
  stopped where it is (`kill -STOP`) for twelve seconds, which is more than two
  leases, and woken. Its log has no `the lease of a worker ran out`, no `a region
  was taken from its owner` and no second `a region was assigned region=0`; the bot
  is not disconnected and has a new action acknowledged. (What the state machine
  does about silence when it is made `alone` is Q13; this is that the process is
  made so.)

#### 9.5a Scenarios: the split (section 3.6)

Written from section 3.6 by someone who built none of it. `S` are of the simulation
(`crates/clustine-sim`), with regions made as the tests of ADR-0014's section 2
make them. `R` are of the runner, stepped by hand, with a store whose answers to
claims and to loads the test can hold back, as the runner's tests of grants do; and
where a scenario says "killed", with whatever those tests have to drop a runner and
to start the store again from what a crash keeps. Chunks are `(x, z)`; the home
chunk is `(0, 0)`.

- **S1.** A region holds the 7 by 7 chunks around `(0, 0)` and around `(19, 0)`,
  with `s` in `(0, 0)` and `p` in `(19, 0)`. `split(&[(19, 0)], N, &granted)` with
  `granted` the seven chunks `(23, -3)` to `(23, 3)`, none of which the region
  holds: `chunks` is the 49 around `(19, 0)` and those seven, ascending; `sides` is
  `seeds: [(19, 0)]`, `staying: [(0, 0)]`. After `take_split` with the same grants
  the region knows nothing of any of the 56, the part holds all of them, and each of
  the two equals what `Region::restore` makes of its state and those holdings.
- **S2.** The same with `granted` being `(-4, 0)` and `(9, 0)`: neither is among
  `chunks`, and the region holds both afterwards. With `granted` `(10, 0)`: it goes.
  (`(9, 0)` is 9 from the home chunk and 10 from the seed; `(10, 0)` is 10 and 9.)
- **S3.** A tie stays: seeds `(20, 0)`, staying `(0, 0)`, `granted` `(10, 5)`: the
  region holds it afterwards.
- **S4.** `p` stands in `(23, 0)`, which the region has asked for and which is in
  `granted`; `named` is `(23, 0)` alone: `p` goes, `(23, 0)` is the only seed and is
  among `chunks`. With `granted` empty: `NoSplit::Nobody`.
- **S5.** A region that plans so and does not take is the region it was, bit for
  bit; its next tick with those grants among its inputs equals the next tick of a
  region that never planned.
- **S6.** A chunk of `granted` that the region holds already changes nothing: the
  result equals that of the same call without it.
- **S7.** A region pinned to an area, with a chunk of that area in `granted` that it
  does not hold yet and that is nearer to the seed: it goes, and the region stays
  pinned to the area.
- **S8.** `Sides::goes`: true of a seed; false of every chunk of `staying`; false of
  a tie; true of every chunk if `staying` is empty; and the same answers at
  coordinates near the ends of what a coordinate can be, in 64 bits.
- **S9.** The runs of ADR-0014, section 2.6, that compare a region that splits with
  one region that holds everything are run with a claim made in the tick before
  the split and its grant handed to `split`: the same players, places, hotbars and
  blocks.
- **R1. The first way.** `s` at the home chunk, `p` in `(19, 0)`, a link with a
  viewer's subscription to both views. `SplitOff` naming `(19, 0)` is given; while
  the runner is at `Stage::Preparing` the link sends `Subscribe` for `(23, -3)` to
  `(23, 3)`, and the store's answers to claims are held back until the runner is at
  `Stage::Settling`, then let go. The outcome is `Split`; the part holds the seven;
  the region knows nothing of them; the store's list has them within the part's
  `bounds` and not within the region's; the log line has `waited=7`.
- **R2.** The same with the seven chunks at `(-4, -3)` to `(-4, 3)`: the region
  holds them, `waited=0`.
- **R3. Killed.** R1, and the runner dropped at the first step of each stage, and
  after the outcome before the part is opened; and the store killed at each write
  and sync it makes from the commit of the split on. Each time both regions are
  opened anew where the store has them: what each is restored holding is what the
  store's list grants it, no chunk is held by both, and the seven are the part's
  if the list has the part and the region's if it has not.
- **R4.** A chunk the store had delivered for a request from before the region gave
  the chunk back, which is granted again by an answer no tick took and goes to the
  part: `Part::chunks` does not have it, and the part reads it from the store.
- **R5. The second way.** After R2's split (no grant waits), the link having been
  closed by it: a new link says hello naming `s` and `p` and, as a viewer's, the 49
  chunks around `(0, 0)`, the 49 around `(19, 0)` and the seven at `(23, ..)`, which
  nobody holds. In the tick that takes it the store is sent **no claim that names
  any of the 56**; each of them is answered `Elsewhere` with the part; the chunks
  around `(0, 0)` are served; the welcome is followed by the `SplitOff` and by
  `Absent` for `p`; the log line has `chunks=56 free=7`.
- **R6.** Then the link sends `SubscribeAsGuest` for the 56 and `Confirm` for the
  `SplitOff`: 56 times `NotMine`. After that tick a `Subscribe` for `(24, 0)` has
  the region claim it, and it is granted.
- **R7. Said again.** After R5, without R6: `Subscribe` once more for `(23, 0)`: the
  region claims it and is granted it. For `(19, 0)`: it claims, and is told the
  part's.
- **R8.** After R5's split on a region pinned to `x < 30`: the hello names `(25, 0)`,
  which is in the area, did not go and is on the part's side: it is claimed and
  served, not told elsewhere.
- **R9.** After a split, the region absorbs another before any edge has said hello:
  the hello's chunks on the former part's side that nobody holds are claimed as
  ever. And with two splits made, neither heard of: a chunk on the side of both is
  told to be the part's with the lower id.

#### 9.6 Scenarios: end to end without stripes, under the ledger bots

`bin/clustine/tests/wanders.rs`, written from this section and from sections 3 and 7
by someone who built none of it.

**The cluster**: two workers, a store without `--pin`, an edge with `--view-distance
2`, a coordinator with `--view-distance 2 --rest-seconds 5` and nothing else, so that
it reshapes by itself with **the distances the rule gives: 10 and 18 chunks**, a
margin of 3 and a rest of 5 s. `Cluster::new` is given no pins and adds nothing
about reshaping (section 9.2): the tests do not say `--reshape`, and would notice
if the default were another. With a view distance of 2 a client is sent the 7 by 7
chunks around its own, so a lone player's region holds exactly those once its trail
is given back. The lease is the default, or 3 s where processes are killed.

**The bots**, as in `follows.rs`: group `A` is a ledger of two bots on the lanes z = 0
and 4, group `B` a ledger of one on z = 8; all walk in the row of chunks z = 0, and
`within(c)` is the blocks x = `16c + 2.5` to `16c + 13.5`. A **wanderer** is a plain
bot (`Bot::walk_to`) that is in no ledger and has no auditor. "Sent" is
`Progress::walk_between`. **An auditor is a player**: when a ledger ends, its auditor
joins at the spawn point and walks to where the group walked, and is merged and
split like anybody; no scenario asserts that nothing happens while one walks.

**What is looked at**: the store's list (`Cluster::regions`), the routing table and
the lines of the coordinator's log that ADR-0016, section 10, names, read as
`follows.rs` reads them (`Begun`, `Ended`); the workers' lines `player arrived from
another region` and `player departed to another region`; and the bots' waits
(`Progress::longest_waits`). A new world's home region is 0 and its first part 1.

- **W1. Apart, on, and back.** `A` settles `within(0)`; `B` joins and settles
  `within(0)`. The list has region 0 alone. Then, `R` times (3 unless
  `CLUSTINE_WANDERS_ROUNDS` says more), with `n` the list's `next`:
  1. `B` is sent `within(19)`. **Exactly one split is begun**, of region 0, with one
     group, and not before `B` has been in chunk 19; it ends with `Ok(n)`. The list
     then has regions 0 and `n`; `n` is pinned to nothing; its `bounds` have chunk
     (19, 0), begin at x = 10 or east of it and end at x = 22 or west of it; region
     0's have chunk (0, 0) and end at x = 9 or west of it. Region `n` is moved to the
     other worker once, a rest or more after the split ended, and region 0 is not.
  2. `B` is sent `within(40)`. **Nothing is begun**, by the distances or for an empty
     region, from when `B` is sent until 35 s after it has arrived, or after region
     `n` last began to run on a worker if that was later (a restore begins the
     thirty seconds anew). Then region `n`'s `bounds` are x = 37 to 43 and z = -3 to
     3 exactly, and region 0's x = -3 to 3 and z = -3 to 3. **At every reading of
     the list in between, region 0's `bounds` end west of where region `n`'s begin,
     and their east end never moves east** (section 3.6: nothing of region 0 lies
     ahead of `B`).
  3. `B` is sent `within(8)`. **Exactly one merge is begun**, by the distances, with
     `survivor=0 absorbed=n` and a gap of 10 or less, and not before `B` has been in
     chunk 10; it ends well. The list has region 0 alone again, `(n, 0)` among the
     absorbed, and `next` is `n + 1`.

  Over all rounds, until the groups end: **no worker has logged a player arriving
  from or departing to another region**, as nobody ever saw another region's land;
  nobody was stood still
  more than once in a rest, but `B` by the one move after a split (ADR-0016's bound,
  as `follows.rs` checks it); no wait was longer than 5 s. Then the end of
  `follows.rs`: every group is audited against its ledger; everything but the store
  is killed and the list is as it was; the store is started again and the list is the
  same; the whole cluster is started again from disk and the blocks are audited.
- **W2. Along the rim.** `A` `within(0)`. `B` walks `between(9, 11)` for a minute:
  nothing is begun, as they are one region. `B` is sent `within(19)` and is split
  off. `B` walks `between(9, 11)` for a minute: one merge, the first time `B` has
  been within 10 chunks for a second, and nothing after it. `B` walks `between(17,
  19)` for a minute: one split, in chunk 19, and nothing after it. In all: a split, a
  merge, a split, in that order, and the moves of parts.
- **W3. Regions that are left.** `A` `within(0)`. A wanderer walks to chunk (19, 0),
  is split off into region 1 and disconnects. For 30 s **no absorption is begun**;
  the list keeps regions 0 and 1. **Region 1 has no `bounds` 35 s after it last
  began to run**: in the single process that is 35 s after the split; in the cluster
  region 1 is moved to the other worker when it has rested, the move is a restore,
  and a restore begins the thirty seconds anew, so it is 35 s after the worker it
  was moved to logged `running a region region=1`, and no later than 50 s after the
  split (5 s of rest, up to 10 s for the move to be begun and made, 35 s). A second
  wanderer walks to chunk (-19, 0), is split off into region 2 and disconnects.
  Fifteen seconds and no more than thirty later **region 1 absorbs region 2**
  (`an absorption is begun by itself survivor=1 absorbed=2`), and no bot of `A`
  waited longer for it than it waits when nothing happens. Then `A` ends and leaves:
  **region 0 absorbs region 1**, and the list has region 0 alone.
- **W4. Joining in the middle.** W1's rounds, and all the while a wanderer joins
  every three seconds, places a block beside the spawn point where no ledger builds,
  sees it acknowledged and shown, breaks it, sees that, and leaves, as the guests of
  `chaos.rs` do. Every one of them is placed, at the spawn point, within 5 s, and
  none is disconnected.
- **W5. A worker dies with a part that has just grown.** `B` is split off at chunk 19
  and its region moved. `B` is sent `within(40)`; when it has been in chunk 30, the
  worker that runs its region is killed and not started again. The region runs
  again, on the other worker, within twice the lease and 5 s; `B` arrives; nobody is
  disconnected; the region's `bounds` are W1's 35 s after `B` has arrived or the
  region last began to run, whichever is later; the audit finds every
  block `B` was acknowledged, in the chunks it was granted in the last seconds before
  the kill as in the others.
- **W6. The store is away while a part grows.** As W5, but it is the store that is
  killed, and started again three seconds later. Every region runs again; `B`
  arrives; the same audit, and once more after the whole cluster is started from
  disk.
- **W7. Killed in the middle of what the coordinator began.** As E6 and E7 of
  `follows.rs`, on W1's steps 1 and 3: the survivor's worker or the absorbed region's
  at logged moments of the merge, the split region's worker at the split, and the
  coordinator and the store in turn. After each, every region of the list runs again
  within twice the lease and 5 s, the merge or the split was made whole or not at
  all, and the round is gone through again until it has been.
- **W8. A coordinator anew, the store away, a worker dead** (N13). `B` is split off
  at chunk 19 and its region moved. The coordinator, the store and the worker of
  `B`'s region are killed. The coordinator is started; three seconds later the
  store. `B`'s region is in the routing table again, with a worker, within the lease
  and 5 s of the store's start, and `B` is not disconnected.
- **W9. Lone players.** One ledger of eleven bots on lanes 19 chunks apart, the
  middle one on z = 0, all `within(0)`: ten of them are more than 18 chunks from the
  home chunk and from each other. Within three minutes of the last one's arrival the
  list has eleven regions, each bot's chunk in the `bounds` of a region of its own;
  the two workers run six and five; nobody was stood still more than once in a rest
  but by a move. Then they end: within three minutes of the last leaving the list has
  region 0 alone.
- **Two parts meet** is part of W3's file as **W3b**: two wanderers; one walks to
  chunk (19, 0) and is region 1; the other to (0, 19) and is region 2, then to (19,
  19), then towards (19, 0). When it has been within 10 chunks of the first, the two
  regions are merged with `survivor=1 absorbed=2`, and region 0 is in no merge.

- **W10. Straight on, on foot** (N17). `A` `within(0)`. A wanderer joins and walks,
  in one `Bot::walk_to` at half a block a tick, which is ten blocks a second, from
  the spawn point to the middle of chunk (49, 0): through its own split, which comes
  when it has been in chunk 19, and thirty chunks beyond. It stays there for 40 s.
  From its join to the end: **exactly one split is begun**, of region 0, and it ends
  with `Ok(n)`; no merge and no absorption is begun; **no worker logs `player
  arrived from another region` or `player departed to another region`**; from the
  end of the split on, every reading of the list (four a second) has region 0's
  `bounds` ending west of where region `n`'s begin, and their east end never moves
  east; at the end region `n`'s `bounds` are x = 46 to 52 and z = -3 to 3; **no bot
  of `A` waited longer than it waits when nothing happens, but once**, at the split;
  the wanderer was not disconnected. Three rounds, the wanderer of each leaving at
  its end and the next joining when the last one's region has been absorbed or 40 s
  have passed. In the single process as well, without the move.
- **W11. Straight on, as fast as a bot goes, and at every tick** (N17). This is the
  scenario that meets the two ticks of section 3.6 on purpose. `A` `within(0)`.
  **Eight wanderers** join and walk, at two blocks a tick, to the lanes z = `112 k`
  + 8.5 for `k` = -4 to 3: seven chunks from lane to lane, so that no two have a
  chunk in view together and each has subscriptions of its own, and near enough
  for all eight and the home chunk to be one group. Wanderer `i` (0 to 7) stands at
  x = 8.5 - `2 i`. When all stand, they are sent together, in the same turn of the
  test, to x = 16 * 49 + 8.5 on their lanes, **at two blocks a tick, which is forty
  blocks a second and the pace `movement.rs` and `players.rs` walk at**: twice a
  sprint in creative flight, a chunk every eight ticks, and as fast as a group can
  go and still be found by a split, whose margin of three chunks they cover in a
  second and a fifth. With their starts two blocks apart **each of the eight ticks
  between two chunk borders has one wanderer crossing a border in it**, so whichever
  tick a split stops at, one of them had its `Subscribe` taken by that tick (the
  first way) and one crossed in it (the second). They arrive and stay for 40 s.
  **Asserted**: everything W10 asserts, with one part for the eight (they are one
  group), `bounds` of the part that have x = 46 to 52 at the end, and region 0's
  east end never moving east. **And that the run met what it is for**: over its
  rounds, the worker's line `a part of the region has been split off` has been
  written at least once with `waited` above 0, and the line `chunks asked for
  players who went are taken for the part's` at least once with `free` above 0.
  Three rounds; if either line has not been seen by then, further rounds until
  both have, twelve at most, and a run that ends without one of them **fails as not
  having tested**, with the counts. Why three is enough as a rule: the wanderers'
  ticks are their own and slip against the region's by up to a tick, so a round
  meets each of the two with a chance well above a half, reckoned and not measured;
  the lines say what was met, and the test goes by them and not by the reckoning.
  Before section 3.6 is built this scenario fails in most rounds (a hand-over is
  logged and region 0 is split a second time), which is how its writer checks it.

**In the single process**, the same file, with a `Server` whose `follow` has those
distances and that rest, and `view_distance: 2`: W1 without the move, W2, W3, W10
and W11, observed through `Server::regions()` and the bots' waits; and the end of
W1 with the server stopped and started from its disk.

**Chaos and moves without pins.** The tests of `chaos.rs` and `moves.rs` that there
are stay pinned (section 9.2). Six of them get a twin on the default world, and the
two harnesses take such a world with this much rewriting and no more:

- `Chaos::start` and `Moves::start` take **a world** where they take boundaries:
  `World::Pinned(&[i32])`, which is everything as it is, or `World::Following`.
- **`World::Following`**: `Cluster::new` with no pins; the coordinator with
  `--view-distance 2 --rest-seconds 5` and the edge with `--view-distance 2`; four
  bots of one ledger on lanes 19 chunks apart (`Ledger::first_lane` 0 and
  `lane_spacing` 304, as W9 spaces its lanes), with `lines` empty; **five
  workers**, one for each region there will be and one to spare, so that no worker ever runs two more
  than another and the coordinator moves nothing by itself under the test. `start`
  returns when the store's list has four regions, each bot's chunk within the
  `bounds` of a region of its own, every one of them runs and has rested, and no
  move, merge or split is under way. From then on the bots stay where they are, so
  **the coordinator has nothing to merge or split until the ledger's auditor walks
  at the end**, and what happens in between is what the test did.
- **A region is a number of the routing table, as it is today.** What changes is
  where a test gets the number from. `every_region_runs` goes through the regions
  of the store's list as last read (the routing table's, while the store is down)
  where it goes through `0..=lines.len()`; `region_at(x)`, which counts lines, is
  beside `region_of(bot)`, which is the region of the list whose `bounds` have the
  bot's chunk; `region_with_players` picks among those; "the region with the spawn
  point" is the list's `home`. A test that says `Chaos::start(.., &[3], ..)` and
  means regions 0 and 1 is not touched.
- **What the checks mean there.** Unchanged: nobody is disconnected; every region
  runs again, on a worker the edge is linked to, within the time the test allows;
  nobody waited as long as the lease where the test says so; no lease ran out
  where it says so; which worker runs a region afterwards; the longest pause; the
  ledger's audit, from memory and from disk. **Gone**: `report.crossings > 0`,
  which counts steps across lines a following world does not have; these tests
  assert `report.actions > 0` alone. **Weaker**: while the auditor walks at the end
  it is merged into the regions it comes to and split off them again, so nothing
  is asserted of the routing table after the bots have ended but that every region
  runs.

| | The test, and its twin of today | What it does on the default world |
|---|---|---|
| Y1 | `chaos.rs`: `players_of_regions_that_follow_them_keep_playing_when_a_worker_wakes_up_after_its_region_went_to_another` (twin of the same name without the first five words) | freezes and wakes the owner of a region with players, three times; the region it had is a part three times in four |
| Y2 | `chaos.rs`: `players_join_and_leave_while_the_home_region_of_a_world_without_pins_has_no_worker` (`players_join_and_leave_while_the_region_they_do_it_in_has_no_worker`) | a guest leaves and a visitor joins right after the home region's worker is killed; the visitor is let in when another worker has it, is in the home region, and builds beside the spawn point |
| Y3 | `chaos.rs`: `players_of_regions_that_follow_them_keep_playing_when_an_owner_and_its_heir_are_killed` (the rounds of `players_keep_playing_while_the_workers_that_run_their_regions_are_killed` that call `kill_owner_and_heir`) | kills the owner of a part and then the worker that is given it, once while that one restores it and once just after |
| Y4 | `moves.rs`: `a_worker_that_is_told_to_stop_hands_the_regions_that_follow_players_over_first` (`a_worker_that_is_told_to_stop_hands_its_region_over_first`) | tells the owner of a part to stop; it is gone within a few seconds, the spare runs the part, nobody waited for the lease |
| Y5 | `moves.rs`: `players_of_regions_that_follow_them_keep_playing_while_every_worker_is_replaced_in_turn` (`players_keep_playing_while_every_worker_is_replaced_in_turn`) | replaces all five workers in turn |
| Y6 | `moves.rs`: `a_part_is_moved_by_hand_and_its_players_stand_still_only_briefly` (`players_stand_still_only_briefly_while_their_regions_are_moved_back_and_forth`, at the small view) | `clustine move --region n` for a part, to the spare and back, each a rest after the last; the pause of its bot within the bound, the others' as when nothing happens |

Each asserts at its end, besides what its twin asserts: the list has the four
regions it began with and no other was made, and the coordinator's log has no `a
split is begun by itself`, `a merge is begun by the distances`, `an absorption is
begun by itself` or `a region is moved to even regions out` between the end of
`start` and the end of the bots. If one of those lines is there, the world moved
under the test and what it measured is not what it says; that fails it.

#### 9.7 The crowd

**X1**, `bin/clustine/tests/crowds.rs`. It answers two questions that nobody has
measured: how long a merge and a split stand a large home region still, and whether
that grows with its players or with its land.

- **The world**: a cluster of two workers without pins; the edge and the coordinator
  with the same `--view-distance` (`CLUSTINE_CROWD_VIEW`), and `--rest-seconds 5`. An
  ordinary run of the tests has a view distance of 2, so the distances 10 and 18; the
  measurement has 8, so 22 and 30.
- **The crowd**: `N` bots (`CLUSTINE_CROWD`, 20 in an ordinary run) in ledgers of
  twenty, all `within(0)`, **each ledger on rows of its own, as the ledger asks of
  scenarios that play on one server at once** (`Ledger::first_lane`: a scenario uses
  the rows from its first lane to two beyond its last, and one is left free): ledger
  `g` has `first_lane` = `84 g` and the lanes z = `84 g` to `84 g + 76`. A hundred
  players are a strip one chunk wide and 26 chunks long from the spawn point south,
  all of the home region. The first version had the ledgers on the same lanes, apart
  in x only, for a square of five chunks by five; a hundred bots on lanes of their
  own need four hundred rows, so there is no such square.
- **Those who go**: a ledger of two with `first_lane` = -12, so on the lanes z = -12
  and -8, beside the crowd's first row. It is sent `within(D_s + 2)`, is split off,
  and is sent back `within(D_m)`, and is merged: at a view distance of 8 that is
  chunk 32 and chunk 22. Twice in an ordinary run, five times in the measurement
  (`CLUSTINE_CROWD_ROUNDS`).
- **Recorded** for every split and every merge, between the coordinator's line that
  it began and the line that it ended and for two seconds after: the longest any bot
  of the crowd waited for an acknowledgement, the longest either of the two waited,
  and how long the coordinator took by its own lines; and what a bot of the crowd
  waits when nothing happens. Printed as the least, the middle and the worst of
  each, as `moves.rs` prints its pauses. **And how long the store takes to make its
  list** while the crowd stands and nothing else happens: twenty calls of
  `Cluster::regions`, timed by the test, the least, the middle and the worst, beside
  how many chunks the home region holds by its worker's line. The list walks every
  grant of the world on the store's commit thread (`Table::list`), a coordinator
  that decides by itself asks for it every lease and around every merge and split,
  and nobody has measured it with the land of two hundred players.
- **Asserted**, in every run: nobody is disconnected; as many splits and merges end
  well as there were rounds; the ledgers' audits; no wait above 5 s.
- **Measured for the roadmap**, by whoever builds step C5.8, with `cargo test
  --release -p clustine --test crowds`, for `N` = 4, 20, 50, 100 and 200 at a view
  distance of 8, and for `N` = 100 once more with a `lane_spacing` of 16 in place
  of 4 (and `first_lane` = `324 g`): the same hundred on four times the land, a
  chunk for each bot. In the first
  series the land grows with the players, as it must on lanes of their own; the
  last pair says which of the two the pause follows.

**What is done about it.** A player bears a stop that they do not take for a fault:
this record takes that to be **half a second in the middle and a second at worst,
optimised, for the crowd of a hundred**, once in ten seconds.

- **Built now, whatever X1 says**: the measurement; and a line in the worker's log
  for every merge and every split of a region it runs, when the region ticks on:
  `a region stood still for a merge or a split` with `region`, `players`, `held` and
  `milliseconds`, so that an operator sees the number that a player felt.
- **If X1 is within the bound**: nothing more in M3. The roadmap has the table, and
  the limit after M3 is one sentence with a number: how long everybody at the spawn
  point stands still when a group leaves it or comes back, for a hundred and for two
  hundred.
- **If X1 is above it**: step C5.10, before the owner is asked to judge the
  milestone. What stands the crowd still is that the links of the region that goes on
  are closed and every edge resumes with it; ADR-0010 and ADR-0014 name the remedy,
  that the survivor of a merge and the region a split leaves behind tell the edge on
  the links it has. It is a change to the contract with the edge, where ordering
  mistakes hide, so it has a record of its own, reviewed, and is built by whoever
  builds the edge.
- **Not a remedy**: other distances, or a longer rest. They change how often the
  crowd stands still and not for how long.

Load is no reason to split in M3. A hundred players in one place are one region on
one thread.

#### 9.8 The cluster test on Kubernetes

`deploy/kind/test.sh` runs what it runs today on the overlay of section 8 (`kubectl
apply --kustomize deploy/kubernetes/test/pinned`): two pinned regions, by hand, with
every step and every check as it is, **the rollout of new workers among them: that
part stays on pinned regions**, whose numbers and boundary its checks name. What a
rolling restart does to regions that follow their players is Y5, between processes
on one machine. G1 is the part without pins, from step C5.7:

- **G1.** The namespace is deleted and the manifests as they are deployed: three
  workers, no pins, a coordinator that decides by itself with the distances of a
  view distance of 8. A Job runs `clustine-botswarm ledger` with four bots walking
  `--west=498.5 --east=509.5`, which is chunk 31, for long enough to get there and
  play (`--seconds=180`). It passes if the Job does, and the coordinator's log has `a
  split is begun by itself` followed by `a worker says what came of a split` with
  `Ok`, then `a region is moved to even regions out`, then, when the ledger's auditor
  has walked out to the four, `a merge is begun by the distances` and `a merge has
  ended`; two of the three workers log `running a region`; and no lease ran out.

The `Cluster` workflow runs it on every push that touches code or `deploy/`, as
today.

### 10. What the owner tries with real clients

An optimised build, because the pauses of an unoptimised one are three to six times
as long and it is the pauses that are to be judged. **A new world directory**
(`--world trial`), so that the regions have the numbers below: a new world's home
region is 0 and its parts are 1, 2, 3 in the order they are made. The directory the
owner has served until now (`world`, with `--boundaries 4`) can be opened as well:
it is made over, what was built in it is there, the log says `the world was divided
otherwise before; what its regions had is in the stored chunks now`, and the parts
are then numbered from 2 on, as the world had regions 0 and 1 before.

Two clients, both in creative mode, where flying is a double tap on the jump key. F3
shows the block and the chunk. **Nobody can see what a region holds**: nothing prints
the store's list (open question 3). So every expectation below is something on a
client's screen or a line of the log, quoted as it is written; the lines carry more
fields than are quoted. A stop of a fifth of a second is not something a lone
player sees on their own screen, where they move without the server; the log says
when it happened and for how long (`a region stood still for a merge or a split
region=… players=… held=… milliseconds=…`), and two players who look at each other
see the other's figure stop for that long.

**The single process:**

```bash
cargo run --release -p clustine -- --world trial
```

1. Join with both clients. **Expected in the log**, before you join: `reshaping by
   itself: regions merge and split by where their players are merge_distance=22
   split_distance=30`, `a region was assigned region=0 worker=local` and `running a
   region region=0`.
2. One of you stays at the spawn point. The other flies east. **Expected**: no line
   in the log until the one who flies is past x = 496. Within two seconds of that:
   `a split is begun by itself region=0 part=1`, then `a worker says what came of a
   split` with `outcome=Ok(RegionId(1))`, then `a region stood still for a merge or
   a split region=0` with `milliseconds` around 200. On the screens: nothing you
   should notice.
3. The one who flies goes on east without stopping, for a minute or more, and
   sprints while flying (the sprint key in the air). **Expected**: no line in the log
   that has `split`, `merge` or `departed` in it, however far and however fast. This
   is what the step is for. Before it a group was split off again every ten chunks;
   and a fault that the review of this record found would show here, once in ten or
   twenty flights, as `player departed to another region` about ten seconds after
   the split and a second `a split is begun by itself region=0` after that.
4. The one who flew flies back. **Expected**: within two seconds of coming west of
   x = 368, if ten seconds have passed since the split, `a merge is begun by the
   distances survivor=0 absorbed=1`, then `a merge has ended survivor=0 absorbed=1`
   with `outcome=Ok`, and `a region stood still for a merge or a split region=0`.
   You are 350 blocks apart and cannot see each other.
5. Fly out again and back to between x = 368 and x = 496, and to and fro there.
   **Expected**: `a split is begun by itself region=0 part=2` once, when you have
   passed x = 496 again; then no line while you stay east of x = 368; a merge with
   `absorbed=2` when you come west of it, and not sooner than ten seconds after the
   split.
6. Both of you fly out past x = 496, within a few chunks of each other. **Expected**:
   one line `a split is begun by itself region=0 part=3` for the two of you, and no
   line after it whatever you do out there: fly around each other, build together.
   Each sees the other, and what the other builds, as at the spawn point.
7. One of you leaves the game out there and joins again. **Expected**: the one who
   joined is at the spawn point; on the other's screen their figure is gone and
   nothing else has happened; no line with `split` or `merge`. **Then the one at the
   spawn point flies out to the other.** **Expected**: when they come within 352
   blocks of the other, `a merge is begun by the distances survivor=0 absorbed=3`;
   and when both are east of x = 496 and ten seconds have passed since that merge,
   `a split is begun by itself region=0 part=4`. Two lines, and the one who stayed
   out there stood still twice for a fifth of a second: the region one joins in is
   the one that survives a merge, and the two are then split off it together. That
   is the rule as it is, not a fault.
8. Build and break out there and at the spawn point, stop the server with Ctrl-C,
   start it again with the same line. **Expected**: `running a region` for region 0
   and for a part that was there when you stopped; you join at the spawn point;
   every block is as you left it, out there as well when you have flown out.

**To see it sooner, on foot**: `cargo run --release -p clustine -- --world trial3
--view-distance 3`. The distances are then 12 and 20 chunks: the split comes past
x = 336 and the merge west of x = 208. You see three chunks far.

**A line between two regions that you can stand at** is not something the default
has: no player sees another region's land. To build at one and across one, as after
step C3:

```bash
cargo run --release -p clustine -- --world pinned --pin 4 --reshape by-hand
```

**Expected in the log**: `reshaping by hand: regions merge and split when somebody
asks`, and `a region was assigned` for `region=0` and for `region=1`. The regions
meet at block x = 64. Walk across, build on both sides and across: nothing may show
on either screen, and the log has `player departed to another region` and `player
arrived from another region` each time one of you crosses.

**The cluster of processes**, each in a terminal of its own, on a world directory
that no other server has open:

```bash
cargo build --release -p clustine
target/release/clustine worldstore --world trial-cluster
target/release/clustine coordinator
target/release/clustine worker --name a --listen 127.0.0.1:25611
target/release/clustine worker --name b --listen 127.0.0.1:25612
target/release/clustine edge
```

9. Steps 1 to 8. The lines about splits and merges are in the coordinator's
   terminal, `a region stood still for a merge or a split` and `running a region`
   in the workers', and in step 1 `worker=` is `a` or `b`. **One thing more is
   expected at step 2**: ten seconds after the split, `a region is moved to even
   regions out region=1` in the coordinator's log and `running a region region=1`
   in the other worker's. The one who flew stood still once more, for about a third
   of a second; the one at the spawn point did not.
10. After a split, find the worker that runs region 1: the coordinator's last line
    `the routing table changed` has `region 1 at 127.0.0.1:25611` or `…:25612`.
    `kill -9` that worker. **Expected**: for five to seven seconds the one out
    there is not answered: chunks ahead do not come and what they build is not yet
    seen by anybody else; then the coordinator logs `the lease of a worker ran out`
    and `a region was assigned region=1`, and they go on, with everything they
    built in those seconds. The one at the spawn point notices nothing. Fly on at
    once, and build: nothing built may be missing.
11. Both of you at the spawn point, in one region (fly back, and wait for the line
    of the merge). Stop the coordinator with Ctrl-C. **One of you flies out past
    x = 496 and stays there; the other stays at the spawn point.** **Expected**:
    both play on, and nothing is split, as nobody is there to decide it. Start the
    coordinator again. **Expected**: within about a quarter of a minute its log has
    `a split is begun by itself region=0`: it waits for its lease and lets every
    region rest once.
12. One of you at x = 100, z = 8, the other at the spawn point, in one region. In a
    further terminal: `target/release/clustine split --region 0 --chunks 6,0`.
    **Expected**: the command ends without an error; the coordinator logs `a worker
    says what came of a split` with `outcome=Ok`; and ten to twenty seconds later `a
    merge is begun by the distances survivor=0`, as the two of you are within 22
    chunks: what is asked by hand lasts only where the distances agree.

What to say if it is not so: which step, what was seen, and the logs of the
terminals. Anything else than expected at steps 2 to 7 is a defect of this step.

## What a player notices

Two players, `a` and `b`, at the default distances, with an optimised build and the
pauses measured in step C3 (unoptimised they are three and a half to six and a half
times as long: half a second to a second and a half).

1. **Both join.** They are in the home region, region 0, which holds what they see.
2. **`b` walks away, east.** Nothing, for 480 blocks. One or two seconds after `b`
   has passed 30 chunks from `a` and from the spawn point, both stand still once for
   a fifth of a second. `b` is in region 1 from then, which holds what `b` sees and
   the last stretch of `b`'s trail; region 0 holds what `a` sees and the home chunk.
   In a cluster of two workers or more, region 1 is moved to another worker ten
   seconds later: `b` stands still once more, for a third of a second, and `a` does
   not. In the single process nothing is moved.
3. **`b` walks on.** Nothing, however far and however fast, and whether or not `b`
   stopped for the split. Region 1 is granted what comes into view and gives back
   what `b` left half a minute ago; region 0 has nothing ahead of `b` (section 3.6).
   This is where a group on stripes was split off again every ten chunks.
4. **`b` comes back.** One or two seconds after `b` is within 22 chunks, 352 blocks,
   of `a` or of the spawn point, `a` stands still for a fifth of a second and `b` for
   two fifths, and both are in region 0. They cannot see each other yet.
5. **They stand side by side, or walk together.** Nothing, ever, but that they are
   split off the home region together, once, 30 chunks from the spawn point, and
   merged into it when they come back within 22.
6. **They walk along each other at the rim.** Around 22 chunks: merged the first time
   they are within it, and not split until they are 30 apart. Around 30: split once,
   and not merged until they are within 22. To be stood still twice they have to
   cross the 128 blocks between the two, and then not sooner than ten seconds apart.
7. **They build.** At the default distances there is no line between two regions in
   anybody's view, so there is nothing to build at or across: every block either of
   them can reach is their own region's. Where a line is in view after all (a merge
   that waits for a rest, for ten or twenty seconds; pinned regions; short
   distances), a block on the far side takes a tick or two longer, as since M2, and
   walking across is a hand-over that nobody notices.
8. **`b` leaves the game** where it stands, far away. `a` notices nothing. Region 1
   has no player; while `a` is in the home region it stays, holding nothing after
   half a minute.
9. **`b` joins again.** At the spawn point, in region 0, wherever it left. Nothing
   for `a`. If `b` walks off again it is split off into region 2; when region 2 is
   left empty one day, region 1 absorbs it, and nobody stands still.
10. **Both leave.** Half a minute later region 0 absorbs what regions there are.

**How often anybody stands still**: as ADR-0016 has it. A player who does not walk
into another region's land is stood still at most once in ten seconds, and in
ordinary play once on going 480 blocks from the others and once on coming within 352
of them; a group that leaves is stood still a second time when its new region is
moved to another worker.

**What costs more than it looks**, new with this step:

- **The first chunks of a new world come a tick or two later than from a stripe**,
  and each is a record in the log. Nobody will tell.
- **A part's player who is moved ten seconds after the split** has been stood still
  twice in ten seconds, for a fifth and a third of a second. ADR-0016 measured it; the
  owner meets it first here.
- **Short distances.** N4. Whoever sets `--merge-distance` below what the view
  distance asks is told so in the log.
- **Somebody who joins again and goes back out to a friend** who stayed far away is
  merged with them into the home region, which survives every merge it is in, and
  the two are split off it together ten seconds later if they are still far from
  the spawn point: the friend stood still twice (the owner's step 7). The rule is
  ADR-0016's; this step is where a player first meets it.
- **Everybody at the spawn point stands still for every group that leaves it and
  every group that comes back**, and how long grows with how many they are. Section
  9.7 measures it and says what is done about it.

## Ruled out

- **A driver of the state machine written for the single process**, which calls
  `Coordinator` and does to its runners what `Changes` says: what ADR-0016 expected.
  It is the worker's loop a second time: the phases of a region, a merge that opens
  another region first, a part that runs from memory until the store has answered,
  the order of outcomes and reports. That loop is where the last three steps found
  their ordering mistakes, and a second copy would be tried by the single process's
  tests alone.
- **The single process as a cluster over TCP on its own machine.** Least to write;
  but every chunk would be loaded through a socket, `serialise_link` would mean
  nothing, and a server that is one process would listen on four ports.
- **Two workers in the single process**, so that a takeover is a worker that stops
  and regions are evened out. A takeover would take a lease, and evening out between
  two workers of one process moves nothing anywhere.
- **Handing the coordinator's tests a list** in place of `knowing`. Section 2.3.
- **The state machine asking for the list until it has one.** Section 2.3: every
  tick's answer would change for every test that hands in no list.
- **Handing the grants that wait to the region before the split is worked out**, as
  the review put its remedy. The region would then have taken something that no
  tick gave it, and one that plans a split and does not take it would not be the
  region it was. They are an argument of `Region::split` instead (section 3.6.1).
- **The region that is split giving back, right after the split, what it was
  granted on the part's side.** It leaves the chunks free a moment later and takes
  a return and a claim where the record of the split does it in one; and it does
  nothing about the second way, in which the region is granted the chunks only
  after the split.
- **Closing the second way in the edge**, which is where the stale word comes from:
  the edge names a player's view in a hello before it can know that the player
  went. It cannot know sooner, as the split is told in the answer to that hello,
  and a hello that names no chunks until its answer is read is another contract
  with the edge.
- **Closing it in the simulation**, with a region that does not claim for a
  viewer's ticket while an edge has not confirmed a `SplitOff`. The hello holds
  back everything behind it until each of its chunks is answered, the confirmation
  among it, so the region would wait for a word that waits for the region.
- **Keeping the line of a split in the region's state**, so that a region restored
  in the two or three ticks after a split still knows it. It is the one hole
  section 3.6.5 leaves, and closing it changes what a state file and a record of a
  split hold, for a kill that has to fall within a tenth of a second of a split
  whose player crossed a chunk border in its last tick.
- **A guest's ticket no longer keeping a chunk**, so that land a region holds only
  because another region's player looks at it goes to that region. It would undo
  the strip half a minute late, when the player has long walked into it, and it is
  a change to who holds what wherever two regions' lands touch.
- **Keeping what a worker says it released until the first list**, in the service.
  Section 2.3.
- **A lease that never runs out in the single process.** It would also stop the
  state machine's ticks when it reshapes by hand, the list on a timer, and the end
  of a merge or a release that nobody answers. Two things are switched off instead
  (section 6.6).
- **Stopping a region's runner before it is opened anew**, in `take_over` or in
  the worker's loop. Section 6.2: the tests of a takeover are about a runner that
  is fenced while it runs.
- **Running every test of chaos and of moves on the default world as well.** Most
  of them are about a boundary where the test put it. Six are not, and have twins
  (section 9.6).
- **`--pin` making `--reshape` `by-hand`**, at the command line or by the coordinator
  when its list shows pinned regions. One flag would mean two things; `follows.rs`
  and the scenarios F of ADR-0016 test deciding by itself on pinned regions and are
  right to; and the line of N14 tells whoever did not mean it.
- **Refusing pinned regions under a coordinator that decides by itself**, for the same
  tests.
- **`--boundaries` kept as another name for `--pin`.** At the coordinator it would
  have to mean nothing, and for the single process it would silently turn a world
  that follows its players into one that does not.
- **Pins that leave land free** (boxes, or stripes with gaps) on the command line.
  Nothing needs them: a world with a boundary at a known place is pinned from end to
  end, and a world that follows its players has no pins. The store's tests go on
  making such divisions by hand.
- **Keeping a world older than step C1 as it is** when its layout is the pins it is
  started with. It needs the fingerprint for one more step, for worlds nobody has.
- **An empty region absorbed by the home region also when that has players**, or by
  the region nearest to it. Section 3.4, and ADR-0016, section 4.4.
- **A region that claims land beyond what its players see**, so that boundaries are
  straighter or further off. What players see is what an edge asks for, and anything
  else is a second rule for who holds a chunk.
- **Short distances at a view distance of 8 as the test of the default rule.** Section
  3.5: they test another regime, in which lands touch.
- **A fifth run of the tests of ordinary play, on a world that follows its players
  at short distances.** Those tests are over within seconds, and a region rests for
  longer than that after it is first assigned: nothing would be merged or split in
  them.
- **Refusing distances that are too short for the view distance.** Section 3.5.
- **The store's list naming the home chunk**, for the coordinator to check its spawn
  point against. Section 2.5; nothing is wrong with it, and nothing needs it while
  the spawn point is a constant.
- **Other distances, a longer rest, or splitting a crowd by load**, as answers to a
  crowd that stands still for too long. Section 9.7.

## Consequences

- A world is one region until somebody walks away, in the single process as in a
  cluster, and nobody says how to divide it.
- The single process is the coordinator's service, a worker's loop and an edge's
  link-keeper in one process. It gains what they have: a region that has lost the
  store is opened again, and its log says what was merged and split. It pays what
  they cost: a registration and a reading before it listens, heartbeats and a lease
  inside one process, and the tasks of three services. It does not start before
  every region of its world runs, and does not stop before nothing of it holds the
  store.
- A coordinator can do nothing until it has read the store's list once, but give
  out a region that a worker says it let go.
- A coordinator whose workers are in its own process gives none of them up.
- A split takes with the part what the region was granted and no tick had heard
  of, if it lies on the part's side; and a region that was split answers an edge
  that has not heard of it by the line of the split, without asking the store. The
  second is in the runner's memory and not in the store's.
- A worker opens a region that it is given again with another epoch before it
  stops the runner it has for it.
- Nothing compares how the services divide the world, because none of them does. A
  worker and a coordinator that speak to two different stores are told apart by
  nothing, as they were not before.
- Every chunk a player sees is a grant in the store's table and a record in its log,
  and is given back with another record half a minute after they have left: 329
  grants for each lone player at a view distance of 8, and 19 of each kind for every
  chunk walked.
- Lines between regions are where claims met, and are in nobody's view as a rule.
- One region without players stays for as long as the home region has players.
- A world is made over when it is started with other pins than it was made with, or
  with none: its regions begin anew, and with them its region ids.
- The tests have two kinds of world: pinned and reshaped by hand, where a boundary is
  where the test put it; and without pins, where regions follow the bots.
- `Coordinator::knowing` and `serve_with` exist for the tests alone.
- Six tests of chaos and of moves run twice, on pinned regions and on regions that
  follow their bots; the others are pinned on purpose.
- Deciding by itself on pinned regions stays possible and stays tested, and is of no
  use to anybody.

## Changes to ADR-0016

1. **Section 8, "`--reshape` is `by-hand` unless told otherwise"**: `by-itself`, for
   the coordinator and the single process.
2. **"What step C5 needs of this", "the single process drives the same
   `Coordinator`: `tick` every `LOOK`; `players` …; `listed` from its own store; and
   what `Changes` says done to its own runners"**: the same `Service` drives it, a
   worker's loop does what `Changes` says, and the single process itself makes none
   of those calls. Point 11 of "Found while building" is kept by construction.
3. **Section 3, "nothing but this quality rests on the speed or on the view distance
   being told right"**: off stripes, that lands do not touch and that a player who is
   handed over is not split off again rest on `D_m` being more than twice the reach
   and `D_s` more than the reach and the trail (section 3.5 here). The coordinator
   says in its log when `D_m` is not.
4. **Section 3, "`--merge-distance` and `--split-distance` … for tests, whose bots
   walk a chunk in under three seconds"; section 11's end-to-end tests at 3 and 5
   chunks**: they stand, on pinned regions, as tests of the state machine between
   processes. They are not tests of the default rule: with a view distance of 8 and
   no pins those distances hand players over where the rule merges them (N4).
5. **Section 4.4, "What C5 has to look at again"**: looked at, and kept (section 3.4
   here).
6. **Section 5.1, "the coordinator's grace period is over (one lease from when it
   was made)"**: or none, for a coordinator made `alone`, which also forgets no
   worker and takes no region for silence (sections 2.3 and 6.6 here).
7. **Section 7, "when the coordinator decides nothing by itself, the list is read on
   events only"**: and at every tick of the service until it has been read once,
   however the coordinator decides.
8. **K15** is gone where there are no pins, and stays where there are (N14 here).
   It is gone only with section 3.6 here: as built, a strip of the region that was
   split could lie ahead of a part's player without any pin.
9. **Section 4.3, "a player who stands in a chunk the region has asked for and not
   been granted is no seed"**, and **"`Region::split` … (see the context; nothing of
   it changes)"**: by the time a split is worked out the claim is answered; a player
   in a chunk that was granted goes, and the grants that no tick had heard of go by
   nearness like every other chunk (section 3.6.1, N15 here). "What the margin does
   not catch" is as it was.
10. **Section 2.3, what a coordinator takes on a worker's word**: also that a region
   was released, before it has read a list (section 2.3 here).
11. **K6 and section 2.4, "the coordinator knows the regions of the layout from the
   start"**: it knows none.
12. **"Risks", "the tests of C4 run on stripes"**: `wanders.rs` (section 9.6 here).

## Changes to ADR-0014 and ADR-0015

All of section 3.6 here.

1. **ADR-0014, section 2.4**: `Region::split` takes `granted`, and "the region
   holds" means held by what its ticks were told or named in `granted`, in step 1
   (the seeds) and in step 4 (the chunks of the part). `Splitting` has `sides`. In
   `take_split`, "none of those is a chunk of the part, as the sim did not hold it
   when the part was worked out" goes: the region holds what it held and what
   `granted` names, without the part's.
2. **ADR-0014, section 3.2, "A chunk granted so is not `Held` in the sim when a
   split is worked out, is therefore no chunk of the part, and is `A`'s by the store
   as by the sim"**: it is the part's if it is on the part's side, by the store's
   record of the split as by both simulations. For a merge the sentence stands.
3. **ADR-0014, section 3.3, step 5**, stands, and is why `waiting_for_the_tick` is
   called before the region takes the split: a chunk the store had delivered is kept
   warm, by either region, only if `A` held it by what its ticks had been told.
4. **ADR-0014, section 3.4, "A split answers no subscription … `A` knows nothing of
   the chunk then, so a viewer's ticket makes it ask the store, which says `foreign`
   with `N`: `Elsewhere { region: N }`, a tick or two after the hello"**: for an edge
   that has not confirmed the `SplitOff`, `A`'s runner answers by the line of the
   split, in the tick that takes the hello, and without asking the store, for the
   chunks that went and for those on the part's side that nobody holds. For any
   other edge, and after a restore, it is as written.
5. **ADR-0014, section 3.5**: the runner keeps one thing more in memory from tick
   `M`, beside the warm chunks: the line of each split it made (`Parted`).
6. **ADR-0014, section 2.6**: every equality stands. `split` has one argument more
   and is a function of the region and its arguments still.
7. **ADR-0015, section 8**, the three things "which steps C4 and C5 must not undo
   without coming back here": come back to in section 3.6.5 here. All three hold;
   the first now also of a player who stands in a chunk whose grant no tick of the
   region had heard of.
8. **ADR-0015, section 6**: nothing changes. What it says the edge does on
   `SplitOff` is what section 3.6.2 here rests on, with the lines of
   `Fanout::split_off`, `move_stay`, `unwant` and `elsewhere` as they are.

## Changes to ADR-0006, ADR-0008 to ADR-0013, and what ADR-0014 says of the coordinator

1. **ADR-0010, section 8, "the single process runs the same: the coordinator's state
   machine is driven inside it"**: with the service around it, a worker's loop and
   the edge's link-keeper (section 6 here).
2. **ADR-0010, section 8, "a world that was last served in stripes is told by its
   layout file being there and the list of regions not"**, and **ADR-0011, "Changes
   to ADR-0010", 9**: by its table having been made from other areas; and a world
   with a `layout` file and no table is made over whatever the file says (section
   2.2 here).
3. **ADR-0010, section 6, "it reads the list when it starts, whenever a worker
   registers …"**: and until it has read it once.
4. **ADR-0010, section 3, "the routing table … names the home region"**: which the
   edge now reads; until this step its process worked the home region out of the
   layout.
5. **ADR-0011, section 2**: `Division` has no `layout`; `Division::stripes` is
   `Division::side_by_side`, and `Division::open` is new; `RegionHello` has no
   `layout`, and a hello is refused for no layout; "from C5 on it is what `clustine
   worldstore --pin` gives" holds of pins side by side, and a division with gaps
   stays the tests'. `Store::memory` and `Store::local` are one region pinned to the
   whole world, said without a `Layout`.
6. **ADR-0011, section 4.2, step 3, "if its fingerprint is `told.layout`, the table
   is written and nothing else changes"**: gone. **Open question 7** (a pinned box):
   no. **Open question 8** (a stripe's id that was a split-off region's) holds of
   pins as it held of stripes.
7. **ADR-0013, section 1, "until step C5 that is the region of the layout that has
   the spawn chunk"**: it is `RoutingTable::home` of the first table that names one.
8. **ADR-0014, section 5.2, "the regions of the layout are known from the start"**,
   and **the coordinator's flag `--store`, "while the store cannot be reached, the
   coordinator goes by `--boundaries` and by what the workers report"**: it goes by
   what the workers report, and knows no other region.
9. **ADR-0012, section 7, "Stripes until C5"**: ended. **ADR-0006** describes what
   `--pin` with `--reshape by-hand` still gives, and no longer what a server is
   unless told.
10. **ADR-0012, section 1, "a region never works out who holds a chunk; it knows
   what the world store has told it"**: the simulation still does not. Its runner
   tells it, for two or three ticks after a split, what the split itself decided
   (section 3.6.2 here); a belief so made is doubted and asked of the store like
   any other when an edge asks again.
11. **ADR-0009 and ADR-0008, what a worker does with a region that is given to it
   again with another epoch**: it opens it first and stops the runner it has when
   the store has answered (section 6.2 here).

## Open questions

The first two are the owner's to answer, and were left open on purpose.

1. **Whether half a second in the middle and a second at worst is what the owner
   bears** for everybody at the spawn point, once in ten seconds (section 9.7). It
   decides whether step C5.10 is built.
2. **Whether the one who leaves should be moved ten seconds after the split.** Its
   new region starts on the worker that made it and is moved when it has rested, so
   a player who walks away stands still twice in ten seconds. ADR-0010 chose that
   over a region that is restored elsewhere at once. If the owner feels the second
   stop: leave a part where it is while its worker has only one region more than
   another has; that is a change to `even_out`.
3. **Whether an operator should be able to ask which regions there are.** Region ids
   are in the coordinator's log and nowhere else, and `clustine move --region` and
   `merge` want one. A `clustine regions` that prints the store's list is a day's
   work for step C6. The owner's trial would be easier to read with it.
4. **Whether `follows.rs` should move to a world without pins**, now that
   `wanders.rs` tests the default there. As it is, the two test deciding by itself
   in two worlds, one of which nobody runs.
5. **Whether the store's list should name the home chunk** (section 2.5), when the
   spawn point stops being a constant.
6. **Whether `Server` should merge and split by hand**, for tests. `Asker` and
   `Mover` reach a coordinator in the process, and nothing uses that yet.
7. **ADR-0016's open question 2** (a part of many groups parted in halves) is as it
   was. N11 is where it would be felt first.
8. **Whether somebody who joins again and goes back out to a friend should stand
   that friend still twice** (the owner's step 7). The home region survives every
   merge it is in, so the friend's region is absorbed into it and the two are split
   off it ten seconds later. Letting the region with more players survive, home or
   not, is not possible while only the home region can be joined; a rule that does
   not merge into a home region whose only players are on their way out is
   ADR-0016's to make, and nothing here needs it.
9. **Whether the line of a split should be in the region's state** (section 3.6.5):
   only if the kill it would cover is ever seen.

## Risks

- **Regions that are not pinned have never run under players.** Claims that are
  granted with a record, land that is given back behind a player, lines where two
  regions' claims met, a region told `NotMine` by one that gave a chunk back a
  moment ago: each has tests of the simulation, the runner and the edge that were
  written from the records, and none has run under the bots or a client. Step C5.5
  runs every test of ordinary play on such a region, and C5.8 the rest. What would
  show a fault: a chunk that never arrives on a client (nothing times that out; a
  bot waits for its chunks and fails), a player who is handed back and forth, a
  block in the audit that is not there.
- **The store's log and table grow with what players see.** A `Granted` and a
  `Returned` record for every row of chunks a player walks past, 329 grants a
  player, and a table file that is written whole when a checkpoint frees a segment
  (ADR-0011, open question 4). Not measured. What would show it: commit times in a
  world of many walking players; the size of `regions/table`.
- **The single process is rebuilt.** Sixty tests of ordinary play and the owner's
  own server go from four hundred lines to the cluster's parts in one step, C5.5.
  The parts are the tested ones; how they are joined is new. What would show a
  fault there: a server that does not come up (the edge waits for a table that
  never names the home region), or one that does not stop.
- **`take_over` rests on a word the store did not say** (section 6.4), and on the
  worker's loop opening a region that its orders name with another epoch before it
  stops the runner it has, which is a change to that loop (section 6.2). The path
  was read, not run, for the one worker being given its own region back; and in a
  cluster it is met rarely, so its tests are those of the single process.
- **Section 3.6.2 rests on the order in which a runner writes a tick's messages and
  an edge reads them**: the outbox entries of a welcome before the answers to its
  subscriptions, and `SubscribeAsGuest` before `Confirm`. Both were read in the code
  (`RegionRunner::tick`, `Fanout::handle_entry`), and neither is promised by a rule
  of the contract. If the first did not hold, the edge would take `Elsewhere` for a
  viewer's subscription, ask the part as a guest, be told `NotMine`, and ask the
  region again a second later, which then asks the store: the strip of section 3.6
  again, but nothing worse. R5 and R6 pin the runner's half; W11 says by its lines
  whether the whole worked.
- **A belief the store did not give.** For two or three ticks after a split the
  region that was split believes chunks to be the part's on its runner's word.
  Wrong only of a chunk nobody holds, and then put right by the first asking again;
  but it is the first belief in this server that is not the store's, and section
  3.6.5 is the whole of the reasoning that it moves no player who stands still.
- **W11 may not meet its ticks.** It says so when it does not, and fails as not
  having tested; if that happens often, its bots' pace or their number is wrong,
  not the server.
- **Y1 to Y6 rest on nothing being merged, split or moved by the coordinator under
  them**, which five workers for four regions and bots that stay put are to make
  sure of. Each checks the log for it and fails if the world moved.
- **Seven hundred tests begin from a coordinator that knows regions and has read no
  list**, a state no process is in after this step. They test what follows from
  knowing regions; what follows from learning them is Q1 to Q8, which are few.
- **The service's tests count readings** and hold them back. They are served a
  coordinator made `knowing`, which is read for on events only, as theirs are today
  (section 2.3, Q14); a test that serves one made with `new` sees a reading at
  every tick.
- **Between C5.2 and C5.9 a coordinator has two sources of regions**, its stripes
  and the list. That is what it has today; the steps in between are short on
  purpose.
- **A coordinator that reshapes by hand and has no list lets nobody in.** Before, an
  edge listened as soon as every stripe had a worker, though no worker could open a
  region without the store. Now it waits for the home region to be named. What
  would show it: an edge that logs `waiting for every region to have a worker`
  while the store is away, where it used to listen and let players wait.
- **Pins that are forgotten make a world over.** A pinned world started once without
  `--pin` is one home region afterwards, and starting it with `--pin` again makes it
  over once more: the blocks are there both times, the regions and their states are
  not. The store's log says it, and nothing asks first.
- **Deciding by itself on pinned regions is only warned about.**
- **Tests at a view distance of 2.** The bots judge what is surely in view by the
  view distance they were granted, so a ledger sees less of what it built and its
  auditor walks more; and the exact `bounds` that W1 and W5 assert rest on a view of
  7 chunks by 7 and on land being given back after exactly thirty seconds.
- **X1 may measure the bots.** Two hundred bots, an edge, two workers and a store on
  six processors: what a bot waits when nothing happens is recorded beside every
  pause for that reason, and a pause is to be read against it.
- **The line of section 3.5 is all that keeps somebody from short distances.** A
  server started with `--merge-distance 3` at a view distance of 8 works and plays
  worse.
- **A process that sleeps for longer than the lease** keeps its worker and its
  regions (section 6.6, P10). What was under way when it fell asleep and is overdue
  when it wakes, a merge or a release, ends by the list and is tried again.
- **`stop` waits for the worker's loop to return**, and the loop waits for every
  runner's last checkpoint. A store that does not answer keeps `stop` from
  returning, where today it keeps a runner's thread from ending, which `stop` also
  waits for.

## Not checked

- **Nothing was built and no test was run for this record**, in its first version
  or in this one. `cargo fmt --all --check` was run at the end of each, to see that
  no code was touched.
- **Read for the first version**, at `c43b21c`: `crates/clustine-region/src/lib.rs`;
  `services/worldstore/src/lib.rs`, `table.rs` without its tests, and in `lanes.rs`
  `load`, claims and returns in `request`, the head of `admit`, `make_over` and
  `align`; in `regions.rs` the divisions its tests use and the tests of starting on
  another division; in the coordinator's `state.rs` the configuration, the comment
  of `Coordinator`, `new`, `register`, `epoch_refused`, `disconnected`, `tick`,
  `listed`, `unlisted`, `even_out`, `settle`, `finish`, `assign`, `grant`,
  `lightest`, `report`, `objection`, `assignments` and `routing_table`; in
  `state/follow.rs` everything from `note_listed` to `say_prepare`; `service.rs`
  without its tests; `bin/clustine/src/lib.rs`; `main.rs` without its tests;
  `cluster.rs` from its beginning to the end of `keep_linked`; in the simulation's
  `region.rs` the types of what a region knows, `tick`, `update_chunks` and
  `settle_chunks`; `bin/clustine/tests/common/mod.rs`. And in excerpts by three
  assistants, whose reports were checked in a handful of places: the region runner,
  the edge's fan-out, `region/reshape.rs`, every file of `bin/clustine/tests` and
  `common/processes.rs`, `tools/botswarm`, `deploy/`, `tools/check.sh` and the
  workflows.
- **Read for this revision, by its writer and nobody else**, at `23a6c3b`. For
  section 3.6: `region/reshape.rs` without its tests; in `region.rs` what a region
  knows of a chunk, `restore`, `tick` and the two functions of its chunks; in the
  runner (`services/worker/src/lib.rs`) `step`, `take_replies`, `reshape`,
  `take_command`, `carry_on`, `commit`, `take_answer`, `waiting_for_the_tick`,
  `begin_anew`, `tick` from the answers to hellos to the answers to subscriptions,
  `drain`, `release_held`, `waits_for_its_chunk`, `accept`, `hello` and
  `subscribe`; in the store `Table::split` and its neighbours, the claims and
  returns of `Lanes::request`, `split` and `may_split`; in the edge's fan-out
  (`fanout.rs`) `lose_link`, `take_link`, `welcomed`, `entries_through`,
  `send_kept`, `outbox`, `handle_entry`, `split_off`, `move_stay`, `hand_over`,
  `handle_event`, `move_view`, `want`, `unwant`, `end`, `flush_asking`, `served`,
  `elsewhere`, `not_mine` and `ask_those_told`; the messages of a link
  (`clustine-rpc/src/messages.rs`); ADR-0014, sections 2.4 to 3.5, and ADR-0015,
  sections 1 to 8. For the rest: the coordinator's `released`, `epoch_refused`,
  `tick`, `listed`, `unlisted`, `settle`, `without_owner_since`, `hand_over`,
  `report`, `objection`, `forget_silent`, `take_unvouched`, `assign`, `grant`,
  `Owner`, and in `follow.rs` `list_is_due` and `the_world_is_known`; in
  `service.rs` `serve`, `serve_from`, `tick_interval`, `Service::new`, the readings,
  `tick`, the end of `register`, and the helpers of its tests; `cluster/worker.rs`
  from `Phase` to the end of `greet_edge`, but for the arms of merges and splits;
  `cluster/coordinator.rs`; `bin/clustine/src/lib.rs`; the `Store` and its handle
  in `worldstore/src/lib.rs`; `Chaos` and `Moves` with their helpers and the tests
  that Y1 to Y6 are twins of; the head of `common/processes.rs`; `Bot::walk_to`;
  the settings of a `Ledger`.
- **Not read**: how a link is closed over TCP and whether what was written to it is
  read before its end is seen (section 3.6.2 has the region answer and the edge
  read on one link, and closes none itself); the edge's `absorbed`, `bring`,
  `take_presence` and everything about presence but ADR-0015's text; `keep_linked`
  and `whole_world` in `cluster/edge.rs`, beyond what the first version and its
  review say of them; the coordinator's `register` between the reports and its end,
  so that a region noted without an owner is given away by a registration, and not
  only by the next tick, is not known (Q12 allows either); the tests of the
  coordinator but the five places that name a table's layout; the tests of the
  runner but the names of those about grants; `policy.rs`; `client.rs` but for its
  outline; the store's `tcp.rs`; the format of the table file; `deploy/kind/test.sh`.
- **The two ways of section 3.6 were read, not run.** No test was written that
  shows either, and the rates given are reckoned from the ticks between two chunk
  borders. W11, run before section 3.6 is built, is the first thing that would show
  them, and its writer is asked to see it fail.
- **That the store's test double can hold back the answers to claims and let a
  flush through** (R1), and that a store can be put under a `Server` that holds
  back commits (P9), were not looked up; each scenario says what stands in its
  place if not.
- **What a store started with other pins does to a cluster that lives** (section
  2.2, N16) was put together from what each part does when it loses the store and
  finds it changed. It was not tried, and the kind of failure a player sees may be
  another than a disconnection.
- **That `knowing` keeps the coordinator's tests** with no change but to their
  helpers, and `serve_with` the service's, was not tried.
- **That making a world of stripes over into `Division::open` is safe at every kill
  point** follows from the same code being killed at every point for a division
  whose home region is not pinned (`gap_at_the_east`); a division without any pinned
  area is not among those the store's tests start on. T1 and T3 are the first.
- **How long the single process takes to start** on the service was reckoned, not
  measured.
- **The pauses in "What a player notices"** are step C3's and C4's, measured on
  stripes with a handful of bots. Nothing has been measured without pins.
- **What the edge sends its clients again when it resumes with a region** was not
  looked at, and decides how X1's pause grows with players.
- **That a hidden flag can be refused with a sentence of its own** was read off how
  `main.rs` refuses distances that do not fit (`Command::error`), not tried for a
  flag.
- **How long the kind test takes** with a second deployment.
- **The speeds of players** are ADR-0016's, from memory of the game.
- **The lines of the log in section 10** were read where they are written; that a
  line reads on a terminal exactly as quoted, with its fields in that order, was
  not tried.

## Review

An independent reviewer went over the first version of this record against the code
of `c43b21c`, line by line where a line is quoted, without building or running
anything, and found **fifteen defects and eight doubts**. All fifteen were accepted
and are worked into the sections above; nothing of the first version stands beside
them as an amendment.

**The one that mattered most** was the first: the record said that K15 was gone and
that the simulation and the runner need not change, and the reviewer showed, from
`Region::split` and `take_split`, that a split keeps for the region that stays
every chunk it was granted in its last tick, so that one fly-away in ten or thirty
ends with the player handed back to the home region forty chunks out and everybody
at the spawn point stood still a second time. Section 3.6.1 is the reviewer's
remedy, made an argument of `split` so that planning changes nothing. **Working it
out found a second way to the same fault that the review had not**: the edge names
the view of a player who went in its next hello to the region they left, before it
can have read that they went, and the region claims whatever of that view nobody
holds yet. It is as frequent as the first and needs a remedy of its own (section
3.6.2), which is in the runner and is the part of this revision its writer is least
sure of. And a third, small: a player standing in a chunk whose grant waited was
left behind by their own group.

**The others**, each where it is now. `Server::start` returned before any region ran
and no longer failed on a world it could not restore: it waits for every region of
the first table and returns the worker's error (6.1, P1, P3). "Every chaos and move
test again" had been read as "they still pass on pins": the pinned tests are said to
be pinned on purpose, and six of them get twins on the default world (9.2, 9.6, Y1
to Y6). A release said to a coordinator that had read no list was dropped: it is
believed (2.3, N18, Q12). W3's bound forgot that a move is a restore (9.6). The
tests' clusters without pins were made to reshape by hand by their own helper
(9.2). The owner's steps expected a region to be empty that had a player, named
region numbers that a world from before does not have, quoted a line that is not
written, and asked the owner to see what nothing shows: section 10 is gone through
anew, on a new directory, with every expectation a screen or a quoted line. The
single process did not write the line that says how it reshapes (6.5, P8). Q10
stood under a step that had nothing to test it with (9.1). The base manifests kept
`--boundaries=4` for one commit under an overlay that added `--pin=4` (8, 9.1).
`knowing` did not reach the tests of the service: `serve_with` does, and such a
coordinator is read for on events only (2.3, Q14). The record had not come back to
the three conditions of ADR-0015, section 8: section 3.6.5 does, and had to, as the
remedy changes which chunks a part is made with. And four small contradictions: one
signature for the clients (5.3), the worker's loop given everything it reaches the
outside through as an interface (6.2), an empty `pins` that leaves the flag out
(9.2), and `README.md` and `CLAUDE.md` changed in the commit that refuses the flag
they name (9.1).

**The doubts.** What `takeover.rs` still tests when the old runner's stop and the
new hello race: the worker's loop opens a region that is named with another epoch
before it stops the runner it has, for every such region and not for tests (6.2,
6.4, P9). A lease inside sixty tests: a coordinator made `alone` gives no worker up
(2.3, 6.6, Q13, P10); the first version's open question about it is answered.
`Server::stop` and the next start: it returns when nothing of the server holds the
store, and P5 tries it fifty times (6.4). A store started with other pins under
workers that live: said, with a line in the store's log (2.2, N16, T9). The
worker's loop is still cut out by a delegate, who is told that nothing inside it
moves, and is read line by line (9.1). X1's ledgers have rows of their own, which
did away with the square of five chunks by five that the first version measured
(9.7). Who flies in the owner's step 11 is said. The cost of a merge or a split of
a large home region stays X1's to measure, with the time of the store's list added
to what it records.

**What the reviewer could not check stays unchecked**: nothing has been run, the
edge's side of a merge was not read again, and how long anything takes is still
reckoned.
