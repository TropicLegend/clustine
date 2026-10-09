# ADR-0017: The end of the stripes

- Status: **Accepted.** The design of step C5 of milestone M3, phase C: stripes,
  `Layout` and `--boundaries` go, and the single process and the cluster run regions
  that follow their players unless told otherwise. It was reviewed twice against the
  code and revised after each: the first review found fifteen defects, the second,
  of what the first revision had added, ten, and all of them are worked in here
  ("Review" at the end says which, and what else each revision found). It is being
  built in the steps of section 9. Step C5.0 is done (`23a6c3b`, `b841c6f`): the
  processes of a cluster have a file each, and a coordinator can be reached in its
  own process without a socket.
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
`b841c6f`, in which the processes of a cluster have a file each under
`bin/clustine/src/cluster/` (`coordinator.rs`, `worldstore.rs`, `worker.rs`,
`edge.rs`, `commands.rs`) and a coordinator can be served and reached inside one
process (section 5.3). "Not checked" at the end says what was read only in
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
- **The service around it** (`service.rs`) knows nothing of TCP but at its door:
  `run(door, config, lists, first_epoch)` takes its clients from a `Door`, which is
  a TCP listener (`serve`) or the receiving end of a `LocalCoordinator`
  (`serve_local`), and hands each to `Service::attach` as an `End<FromCoordinator,
  ToCoordinator>`. `clustine_rpc::link::in_process` makes such a pair of ends for any
  two message types. `Service::new` makes the coordinator, with `Coordinator::new`.
- **The store's threads** (`services/worldstore/src/lib.rs`, `lanes.rs`,
  `chunks.rs`). A `Store` is the sending end of the commit thread's queue and
  nothing else; a handle is another such end, and dropping it queues a `Close`. The
  commit thread and the thread for chunks are started detached and end by
  themselves when every such end is gone, having done everything they were asked
  first. **Nothing can wait for them.** A handle's own `flush` waits until
  everything asked through that handle has been done; a runner that is stopped
  while it only runs calls it (`RegionRunner::run`), and one that is stopped in the
  middle of a release, a merge or a split does not: it lets go of its handle as it
  is (`Ended::Abandoned`), and the store does what it was asked afterwards.
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
8. **Nothing changes in the edge's fan-out, and one thing in the store.** What is
   new for them is that what they were built for now happens. The store's table,
   log, grants, merges and splits are as they are; it gets a way to wait until it is
   at rest (`Store::flush`, section 5.5), which the single process needs to stop
   cleanly (section 6.4).

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

**What a worker says it let go of before the first list is kept, and judged by that
list.** `Coordinator::released(now, name, region, epoch)` has three cases today: the
owner's release for a merge, the owner's release, and a region the coordinator
knows without an owner that a registered worker says it released with an epoch not
below the region's (`without_owner_since`). A word that fits none of them is
dropped. From this step it is kept instead if all of this holds: **the coordinator
still awaits its first list** (`awaits_the_list()`), **it does not know `region`**,
and **`name` is a registered worker**. It keeps `(name, region, epoch)` behind the
words it has kept before, each such word once, changes nothing else by that call,
and logs `a worker released a region this coordinator does not know yet; the first
list will say`.

**`listed` judges the words that were kept**, in the order they were said, at one
place: when it has added the living regions of the list and raised the epoch of
every region without an owner to the list's, and before it gives regions away. Each
word is judged **by the third case as it stands**. If the region is known by then,
has no owner, is no part of a merge, and has no epoch above the word's, which now
says that the list's epoch is not above it, and if `name` is still registered: the
region is let go with the word's epoch, no epoch issued from then on is at or below
it, the worker goes behind all the others, and the `assign` that ends `listed`
gives the region away **whatever the grace period says**, to any worker that can be
given it, the one that released it included. It logs what that case logs, `a worker
released a region before this coordinator knew of it`. **Any other word is
dropped**, with the line there is for that (`a worker released a region it does not
own with that epoch`):

- **the list does not have the region living** (absorbed, or of no table the store
  has: rolled back, or another store): the coordinator goes on knowing nothing of
  it, which is right;
- **the list has it with a higher epoch**: somebody has run it since the release,
  and the word is stale. The region waits out the grace period like any region of
  the list, or is reported by whoever runs it;
- **the region has an owner by then**: a worker registered holding it meanwhile, and
  owns it by its report (`report`), with whatever epoch the rules for reports allow;
- **`name` is no longer registered**.

The words that were kept are gone with the first list, whatever became of each, and
after it `released` is what it was: a region the coordinator does not know then is
one the store does not have. A reading that fails (`unlisted`) leaves them kept. A
region that was let go **for a merge** which a coordinator before this one began is
no special case: if the list has it absorbed the word is dropped; if it has it
living the region is given away, and its new owner's hello either fences whoever
has it open to absorb it, so that the store declines that merge, or comes behind the
merge and is told that the region was absorbed.

**What it costs is one reading.** The service has the list read at every
registration, and at every tick while the coordinator awaits its first list (above),
so the word waits for the reading that is under way or the next: milliseconds when
the store is there, and when it is not, no worker could open the region anyway.
Without any of this the word is dropped, as the worker says it once, and the region
is found by the first list and waits out the grace period: a lease of standing
still where a move takes a moment (the first review's fourth defect; on stripes the
layout named the region, so `without_owner_since` found it).

**Why this, and neither of the two ways the first revision weighed.** That
revision had the coordinator believe the word at once: the region noted as known,
without an owner, with the word's epoch, let go, and given away by that very call.
The second review showed what is wrong with it. *Nothing says that the store has
such a region*: the worker's release was over before it registered, so the list
that follows was begun after it, and if that list does not have the region no list
ever will; the rule that would have left it alone ("the reading is older than the
split that made it") is for a coordinator that ordered a split, which this one did
not. The region would stay known for good and be given to a worker whose loop ends
on the store's `UnknownRegion` every time it is started. *Nothing checks the epoch*:
the third case is safe against a stale word because the list supplies the store's
epoch to compare with (`listed` raises it, `without_owner_since` compares), and
before the list there is nothing. A word from a worker whose region the
coordinator before had long given to another would give the region to that worker
with a new epoch, and its hello would fence a healthy runner. *And it was keyed to
`home()` being `None`*, which is so for good in every coordinator made `knowing`:
`released_from_anyone_but_the_owner_with_its_epoch_changes_nothing` hands in no
list and fails by it. The other way the first revision named, keeping such words in
the service and saying them to the state machine behind the first list, was turned
down because a word said later is said out of its order against the registrations
and reports that came meanwhile. That reason stands, and does not touch this:
nothing is kept in the service and nothing is said later. The state machine has the
word from the call that said it, and applies it inside `listed`, where the list's
epoch is at hand and everything that was registered and reported meanwhile is
already in its state, by a case that is there and tested.

**Keyed to `awaits_the_list()`**, so a coordinator made `knowing` (below) keeps no
word, and every test of `released` that there is stands as it is.

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
- **it takes no region from a worker for having been slow**: `forget_silent`
  forgets no worker for silence (it still takes the regions of one that said it
  leaves and whose connection then ended, which is no silence), `take_unvouched`
  does nothing, and the service that serves it closes no worker's
  connection for silence. There is nobody else to give a region to, so taking it
  could only give it back to the same worker with another epoch, which stands its
  players still for a restore and, when the coordinator decides by itself, leaves
  the worker at fault and nothing merged or split for thirty seconds. That is what
  a process does that is held up for longer than the lease: a laptop that sleeps, a
  debugger, sixty tests on six processors. A region whose runner has lost the store
  is opened again by the worker's loop, which needs no coordinator for it;
- **it notes no failure of a worker**: `note_failure` does nothing. One thing
  still takes a region from the one worker, and has to: a merge whose first stage
  is overdue (`end_overdue_reshapes`, `lapse_merge`), which is how a release that
  was never answered ends. In a cluster the owner is then at fault for six leases,
  so that others are given regions before it. Here there are no others, and the
  only thing the fault would do is keep the coordinator from beginning anything
  with that worker's regions (`free_but_for_its_rest`), which are all the regions
  there are, for thirty seconds. So such a merge costs the one restore it has to
  cost, and is begun again when its regions have rested. (`take_unvouched` sets the
  same mark by itself and is off anyway.)

Section 6.6 goes through everything that hangs on the lease, and through what
becomes of a merge, a split and a release that are overdue in one process.

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
`serve` makes a coordinator that knows no region for good. So the loop that serves,
`run` (section 5.3), is given its coordinator where it is given a configuration
and a first epoch today: by `serve` and `serve_from` one made with
`Coordinator::new`, by `serve_local` one made `alone`, and by `serve_with(listener,
coordinator, lists)`, which is `#[doc(hidden)]`, whatever its caller made. The
tests' `Served::start` and `start_at` call that with `Coordinator::knowing(config,
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
   It is wider than tick `T` alone: a tick is published when the store has
   confirmed it, and a region runs up to eight ticks ahead of that
   (`MAX_TICKS_AHEAD`), so on a slow disk the `Subscribe`s of several ticks are
   lost with the link. And it needs the edge to link to `A` before it links to `N`,
   which is the rule: the link to `A` is tried again every 20 ms, the one to `N`
   needs a routing table that names `N`. An edge that reaches `N` first is told
   there that `p` is `N`'s, and its hello to `A` names `p`'s view as a guest's.

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
its own stands in it or sees it. At distances that follow from the view distance
that cannot be for anybody the split was worked out for (`D_s` is more than twice
the reach), and comes to be only when the two have come near enough to be merged
(N3). Two things are changed for it, one for each way, both in the simulation and
the runner; the edge, the store and the coordinator are as they are.

**What L does not cover**, and no part of this section changes:

- **a player who walked with those who went and was not caught by the split's
  margin** (ADR-0016, section 4.3, "What the margin does not catch"). A split names
  the chunks within the margin, three chunks, of where a group was at the report it
  goes by. Somebody of the group who is out of those chunks by tick `T` is no seed:
  they stay `A`'s, far out, on a chunk that stays with them, and `A` goes on
  claiming what they see, among it chunks on the part's side. They are split off
  when `A` has rested, and that part is merged with the first; if they walk into
  the part's land meanwhile they are handed over. That is ADR-0016's, costs a
  second stop at the spawn point, and takes a group that covers the margin in less
  than a report, a checkpoint and a flush take: twice a sprint in flight, on a
  machine that is busy. W11 says how it tells such a round from the fault here;
- **a region that is restored between the split and the edge's hearing of it**:
  section 3.6.5 ends with it.

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
- The line `a part of the region has been split off` gains `players`, how many
  went with it, `chunks`, how many the part holds, and `waited`, how many of them
  were grants that no tick had been told of, in that order behind `tick` and
  `part`. (`players` is what W11 tells a split that caught its whole group by.)

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
  `Parted` for, **and the edge has not said on this link that it has that entry**;
  and the region **knows nothing of the chunk** (`Knowledge::Unknown`);
  and the chunk is in `gone`, or is outside the region's pinned areas and
  `sides.goes` is true of it: then `(chunk, part)` is put into the coming tick's
  `foreign`, as if the store had said so. Of several such parts the lowest id.
  Nothing is put in for a guest's subscription, for one that is said again
  (`Subscribe` for a chunk that was told elsewhere, which has the region ask the
  store, as ever), or for a chunk the region holds, has asked for or believes
  another's.
- **An edge has said that it has the entry** if the hello this link began with was
  answered as a resume and its `seen` is at or above the entry's number, or a
  `Confirm` taken from this link names such a number. Such an edge has read the
  `SplitOff`, on this link or on one before it, and has moved its player's view
  already. What it says so with is put among the coming tick's inputs
  (`EdgeEvent::Confirmed`, in `RegionRunner::hello` for a hello) and takes the
  entry out of the outbox only when that tick has run, so the state as of the last
  tick still has the entry: the runner goes by the number as well, which it keeps
  for each link. What such an edge names as a viewer's is the view of somebody who
  stayed, and is claimed as ever. After anything but a resume a hello's `seen`
  says nothing, as today.
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
3. `A` takes the `SubscribeAsGuest`s and the `Confirm`, which wait behind the hold
   of the hello until every chunk of it is answered, `A`'s own among them: the
   tickets are a guest's, nothing wants the beliefs any more and they go
   (`settle_chunks`), each subscription is answered `NotMine` and ends, and the
   `Parted` is dropped after the tick that took the `Confirm`.
4. `N` is asked for `p`'s view as a viewer, by its hello or by a `Subscribe`. It
   serves what it holds and claims what it knows nothing of: **the row is `N`'s by
   the first claim there is for it.**

**What ends a belief** is the end of the viewer's ticket it was put in for, and
nothing else: the edge's `SubscribeAsGuest` or `Unsubscribe`, which it sends for
every chunk of `p`'s view when it reads the entry (`Fanout::move_stay`, `unwant`);
the end of the link, which gives its tickets back; the edge's being gone. **Not the
`Parted`**: the `Parted` decides only whether a belief is put in for a new viewer's
subscription. So nothing rests on the `SubscribeAsGuest` coming before the
`Confirm`, which the first revision said it did: with the two the other way round
the `Parted` goes a tick sooner, and the beliefs last by their tickets as before.
What the mechanism does rest on is the first order alone, that the edge reads the
entries of a welcome before the answers to its subscriptions ("Risks").

**When a belief is wrong.** A belief is put in for a chunk the region knows nothing
of, and after `take_split` it knows nothing of anything it did not hold
(`Region::restore`). So what it takes for the part's is one of three things:

- **a chunk that went with the split**: the part's, as the store would say a tick or
  two later;
- **a chunk nobody holds**: nobody's yet, and the part's by the first claim, which
  is the purpose;
- **a chunk a third region `B` holds**, which `A` believed `B`'s before the split
  and has forgotten with it. This is wrong, and ordinary where regions are pinned
  side by side: `p` is split off beside a boundary, and the neighbour's chunks in
  `p`'s view are on the part's side.

For a chunk that only `p` sees the third is as harmless as the second: the edge
passes the answer over (step 2), `N` claims the chunk, is told `B`'s and says so,
and the edge asks `B`. **Where somebody who stayed sees the chunk as well**, the
subscription stays a viewer's and the edge takes the answer `Elsewhere { N }`
(`Fanout::elsewhere`). It then asks `N` as a guest:

- `N` holds the chunk: `N` serves it, which is right;
- **nobody holds it**: `N` says `NotMine`, the edge asks `A` again, at once or
  within a second (`not_mine`, `ask_those_told`), and `A` asks the store. The chunk
  arrives a second late, nine chunks from whoever sees it;
- **`B` holds it**: the edge has been `B`'s guest for the chunk since before the
  split and goes on being served by `B`, so nothing is missing on a screen. The
  edge asks `A` again **when nothing is asked of `N` for the chunk any more**
  (`not_mine`, `unwant`, `ask_those_told`): at once, if `N` was asked as a guest
  and says `NotMine`; and when `p` no longer sees the chunk, if `N` is asked for it
  as `p`'s own region and has said `Elsewhere { B }`. `A` then asks the store and
  is told `B`'s. Until then, what the one who stayed does to a block of the chunk
  is sent by way of `N` to `B` (`N` says `NotMine` to it, naming `B` or nobody,
  and the edge sends it on), one hop more; and if they walk into the chunk they
  are let go to `N`, which sends them on to `B` or takes them in for a tick.
  Nothing goes round in a ring: `N` and `B` believe only the store.

All three need somebody who stayed within twice the reach of somebody who went
(distances set by hand, or a split asked by hand); the third needs a third region's
land in that view as well, which is pinned worlds, where `follows.rs` runs at such
distances. A belief that somebody who stayed keeps looking at lasts for as long as
they look, and is by then either true or has been doubted by the edge.

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
| another region `B`'s, asked of `A` by an edge that has not heard | | `B`'s | believed `N`'s while that edge asks for it as a viewer (section 3.6.2, "When a belief is wrong"); `B`'s on the store's word when the edge asks again | believed `B`'s on the store's word, if `N` is asked |
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
| the store, at any write or sync of the split | the split or not, as the record is durable or not | the runner has lost the store and ends with `Off::StoreLost`: `A` is opened again, by a runner that never made the split, and is the one or the other of the two rows above. **If the record was durable and the store died before it answered, `A` is the region after the split and has no `Parted`**: section 3.6.5's last paragraph |
| both | the same | the same |
| the worker, or `A`'s runner alone (the region is taken over, or loses the store), between the split and the edge's reading of the entry | the split | `A` is restored without a `Parted`: section 3.6.5's last paragraph |

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
   two of each other when one of them was split off the other by hand; or, where
   the chunk is a third region's, an arrival that `N` sends on to that region with
   `NotMine`, as it sends on any arrival into a chunk it believes another's.
3. **"A merge announces itself in the survivor's outbox before anything the survivor
   says of a stay that came with it."** Nothing here touches a merge.

**One thing this does not make sure of.** `Parted` is in the runner's memory, and
`A` runs without one **whenever it is restored between the split and the edge's
reading of the entry**. That is so in more cases than the first revision named:

- **the store dies between making the record of the split durable and answering
  it** (`Lanes::split`: `write_alone`, then the table, then the answer). The runner
  ends with `Off::StoreLost` and the region is opened again as the region after the
  split, by a runner that never held a `Splitting`. This is every time the store is
  killed there, and W7 kills it there on purpose;
- **the worker dies** before the edge has read the entry: both regions are restored
  from the record, on whichever workers;
- **`A` is taken over, or loses the store**, in that time.

In each of them the second way is open once, **if `p` crossed a chunk border in a
tick whose `Subscribe` no tick took**: the edge's hello names the row, `A` claims it
and is granted it. If the edge links to `N` first, nothing happens at all. It costs
one hand-over and one more split, and is not mended: mending it means keeping the
line of a split in the region's state, which is a change to what the store has on
disk (open question 9). Nothing asserts statement L across such a restore (W7, R3).

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

/// Serves `coordinator` to the clients that come in at `door`, for as long as the
/// future is not dropped. Private, as today; `Door` is `Tcp(TcpListener)` or
/// `Local(..)`, as built.
async fn run(door: Door, coordinator: Coordinator, lists: Lists);

/// `serve` for a coordinator the caller has made. For the tests of the service.
#[doc(hidden)]
pub async fn serve_with<L>(listener: TcpListener, coordinator: Coordinator, lists: L)
    -> io::Result<()>
where L: Fn() -> io::Result<RegionList> + Send + Sync + 'static;
```

`released` keeps a word while the coordinator awaits its first list, and `listed`
judges it (section 2.3). `note_failure` does nothing while `keeps_its_workers()`.
`Service::new` takes the coordinator where it takes a configuration and a first
epoch; `Service::tick` has the list read while `awaits_the_list()` and no reading is
under way, and leaves the connections of workers alone while `keeps_its_workers()`.

**The loop that serves is `run`, as step C5.0 built it** (`b841c6f`), with one
change in step C5.2: it takes the coordinator where it takes `config` and
`first_epoch` today, and reads the lease and whether the coordinator decides by
itself from `Coordinator::config()`. Who makes the coordinator, each on the clock
the ticks follow (`now()` in `service.rs`), when the future is first polled:

- `serve` and `serve_from`: `Coordinator::new(config, now(), first_epoch)`, at
  `Door::Tcp(listener)`;
- `serve_local`: `Coordinator::alone(config, now(), unix_milliseconds())`, at
  `Door::Local(..)`. **Until C5.2 it is made with `new`**, as built, since there is
  no `alone` yet;
- `serve_with`: its caller's, at `Door::Tcp(listener)`. It is `serve_from` without
  the making, and is the only one of the three that is hidden.

The line `the coordinator is serving`, which `run` writes with `lease` and
`first_epoch`, is written by those who know the first epoch, `serve_from` and
`serve_local`, and `serve_with` writes it without. (The first revision had
`serve_local` be `serve_with` "with those ends where it accepts from a listener":
`serve_with` takes a `TcpListener`, and what the three share is `run`.)

And how a client reaches a coordinator, for the single process, as built:

```rust
/// Where a coordinator is: at an address, or in this process.
#[derive(Debug, Clone)]
pub enum Reach { Tcp(String), Local(LocalCoordinator) }

/// The way to a coordinator in this process; clones lead to the same one.
#[derive(Debug, Clone)]
pub struct LocalCoordinator { /* a sender of the service's ends of new connections */ }

/// A coordinator in this process and the way to it. The future serves until it is
/// dropped. From C5.2 its coordinator is made `alone`; its first epoch is from the
/// wall clock, as `serve` takes it.
pub fn serve_local<L>(config: CoordinatorConfig, lists: L)
    -> (LocalCoordinator, impl Future<Output = ()>)
where L: Fn() -> io::Result<RegionList> + Send + Sync + 'static;
```

`WorkerClient::register`, `register_with_heartbeat`, `RoutingWatch::connect`,
`Asker::merge`, `Asker::split` and `Mover::ask` take an `impl Into<Reach>` where they
took an address, and `&str`, `&String`, `String`, a `LocalCoordinator` and a
`&Reach` are each a `Reach`, so that no caller changed. A client that is given a
`Reach::Local` makes a pair with `link::in_process` and hands the service its end
through the `LocalCoordinator`. `serve_local` is the loop `serve` is, `run`, at
another door: the same `Service`, the same order of calls, the same ticks, the same
thread for a reading of the list. A local connection ends when either side drops
its end, as a TCP connection does when it is closed; when every `LocalCoordinator`
is gone nobody can come any more, and those who are there are served on.

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

**And one thing is added: a way to wait until the store is at rest** (step C5.1b).

```rust
impl Store {
    /// Waits until the store is at rest with everything that was asked of it before
    /// this call, through this store, a clone of it or any handle: each such request
    /// has been done, or will never be (what a handle asks after it is lost is not
    /// done); what it wrote is as durable as the store makes it; every answer the
    /// store owes for it has been sent to its handle; and neither of the store's
    /// threads is in the middle of any of it.
    ///
    /// It closes nothing. Handles and clones that live are served on, and what they
    /// ask after this call is not waited for.
    ///
    /// The error is [`StoreError::Io`] for as long as what a failed write left in
    /// the log is not durably gone, as for [`Store::regions`]. Nothing is under way
    /// then either.
    pub fn flush(&self) -> Result<(), StoreError>;
}
```

It is **a barrier through the thread for chunks and back, answered when the commit
thread has ended the group it comes back in**, which is what `StoreRequest::Flush`
is for one handle (`Job::Flush`, `Message::Flushed`, `Group::flushes`), done for
the store as a whole. Three small things in `services/worldstore/src/lib.rs`,
`lanes.rs` and `chunks.rs`:

1. `Store::flush` sends the commit thread `Message::Barrier { reply_to, answer }`,
   with a clone of its own sender as `reply_to`, as `Store::open_region` sends
   `Message::Open`, and waits for `answer`.
2. The commit thread, when the message's turn comes, has handled everything that
   was sent before it. It **ends the group** (`end_group`): the log is synced, the
   commits and claims of the group are answered, and the jobs that waited for the
   group (`Owner::held`) are passed on to the thread for chunks. Then it passes on
   `Job::Barrier { reply_to, answer }`, behind them.
3. The thread for chunks, when the job's turn comes, has done every job that was
   passed on before it: the saves, the checkpoints, the returns, the flushes of
   handles, and a region that had to be put into the stored chunks before it was
   handed to whoever opened it. What those had to say to the commit thread
   (`Checkpointed`, `Returned`, `Flushed`, `Lost`, `Close`) is in its queue. The
   job sends `Message::Passed { answer }` through `reply_to`, behind them, and does
   nothing else. It does not make the saved chunks durable: that stays the business
   of a checkpoint and a return, with what a failure of it means.
4. The commit thread, when that message's turn comes, has put the state files of
   those checkpoints in place and written the records of those returns. It **ends
   the group** once more, as it does before it makes a list of regions or takes a
   hello: the log and the directory are synced, the segments and the table file
   are seen to (`collect`, `trim_for_the_table`), and the flushes of handles are
   answered. Then it answers the barrier: `Ok(())`, or `Err(StoreError::Io(..))`
   if `Log::settle` cannot cut back what a failed write left, exactly as
   `Message::Regions` is answered.

**One round is enough**, because nothing that the commit thread does with what
comes back from the thread for chunks gives that thread more work: `install`,
`returned` and a flush put something into the group and send no job, and the only
other senders of jobs are a request and a hello, which are behind the barrier and
were not asked before it. A merge and a split are done whole on the commit thread
when their request is handled (`Lanes::absorb`, `Lanes::split`), so they are done
by step 2. What a failed write or sync undid is not done after all and is not
waited for: every region has lost its owner then (`fail_log`), and the barrier
still goes round and is answered.

**Why a barrier and not a close that joins the two threads.**

- *A join waits for every sender to be gone*, which is every clone of the `Store`
  and every handle, wherever they are: the threads end only then. One that is
  forgotten (a clone inside a closure of a task that has not ended; the handle of a
  hello nobody took, in the result of a future that was dropped) makes a close hang
  for ever and without a word. The barrier does not care who lives.
- *A `Store` is a `Sender` and is cloned freely.* A close that joins has to keep two
  `JoinHandle`s where every clone finds them, and say which clone may close.
- *The barrier is the path a handle's flush goes*, which every checkpoint, release,
  merge and split of every test already runs.
- *It can be used on a store that runs*, which a test needs that wants to step a
  runner only when the store has answered (R1).

What it does not give, and a join would: that the threads are gone. They end by
themselves when the last sender is dropped, as today, and neither `Lanes` nor the
log writes anything when it is dropped, so a store that is at rest and then let go
of writes nothing more.

**If a handle or a clone still lives** when `flush` returns: it is served on.
What it asked before the call is covered by it. What it asks afterwards is done
afterwards, and the store's threads live for as long as it does. Dropping a handle
that asked for nothing since writes nothing: `Close` ends a group that is empty and
takes the owner out. So a caller that wants the store quiet for good calls `flush`
when it knows of no handle that will ask again, which is the caller's to see to,
and section 6.4 says how `Server::stop` does.

#### 5.6 `clustine-sim` and `services/worker`

Section 3.6: `Region::split` takes `granted`; `Splitting` has `sides`; `Sides` is
new; the runner hands `split` the grants that wait and keeps a `Parted` for each
split it made. Nothing of it is on the wire or on disk. Breaks: every call of
`Region::split` (the simulation's tests of ADR-0014, the runner) and every literal of
a `Splitting`.

**And the runner says how long its region stood still for a merge or a split**, for
the line of section 9.7:

```rust
/// How long a region did not tick for a merge or a split, and what it was then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standstill {
    /// The players the region had after its last tick before it stopped: those who
    /// stood still in it.
    pub players: u64,
    /// The chunks it held then by the world store's word, as `RegionStatus::held`.
    pub held: u64,
    /// From when it stopped ticking until its next tick had run.
    pub milliseconds: u64,
}

impl RegionRunner {
    /// Has `tell` called, on the runner's thread, each time the region has ticked on
    /// after it had stopped ticking for a merge or a split, whether that was made
    /// or came to nothing. `tell` must not wait.
    pub fn with_standstills(self, tell: Box<dyn Fn(Standstill) + Send>) -> Self;
}
```

**The runner measures, because only it knows the two moments.** It notes the time,
its players and its held chunks in the step that takes it from `Phase::Preparing`
to `Phase::Settling` with a merge or a split under way (`RegionRunner::step`, where
it logs that the region stops ticking; not for a release, after which the region
never ticks on), and calls `tell` in the step that runs its next tick, right behind
that tick: after `begin_anew` if the merge or the split was made, after `tick_on`
if the store declined it or nothing came of it. A runner that ends instead (the
store lost, stopped, abandoned) tells nothing. The clock is read in the runner,
which reads it for its ticks already, and not in the simulation.

**The worker's loop writes the line, because only it knows the region's number**,
and it writes the other lines of a merge and a split (`the merge has ended`, `the
split has ended; opening the new region`). A runner knows its ticks and not which
region it is: none of its own lines names one. Where the loop makes a runner
(`cluster/worker.rs`, the arm that takes an opened region, for
`RegionRunner::restore` and for `RegionRunner::of_part` alike, beside
`with_checkpoint_interval`) it hands it a `tell` that writes

```text
a region stood still for a merge or a split region=0 players=2 held=658 milliseconds=140
```

(the numbers are made up) at the level of the lines beside it, with exactly these
four fields in this order:
`region`, the loop's; `players`, `held` and `milliseconds`, the `Standstill`'s. The
call is made on the region's thread at the tick, so the line stands in the log
where the region went on, and the loop gets no arm, no queue and no look more.

**What the number is, and is not.** It is how long the region did not tick: the
commits of its last ticks confirmed, the second checkpoint and its flush, the
store's record of the merge or the split, and taking it. **A player waits a little
longer than that**: until their edge has a link to the region again (tried every
20 ms), has said hello, and the tick or two that answer it have run. For a merge it
is the survivor's standstill; those who were in the region that was absorbed stood
still from when *that* region stopped ticking to be released, which is longer, and
no line says how long: the runner that released it has ended and the region is no
more. For a split it is the standstill of everybody the region had, those who went
included, who wait for the part's first tick besides. X1 prints the line's number
beside what the bots waited, so that the difference is known once.

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
   of the table has to run**: `start` waits until the worker's watch shows each
   region the table has a route for with the epoch of that route, which is when it
   is restored and ticks. **It goes by the latest table it has been sent**, and not
   by the first: should a route change before its region runs, the epoch waited for
   is the new one. (Nothing in one process changes a route before its region runs,
   as far as was read; a wait that would last for ever if something did is not
   left in.) **And it waits for the worker's loop as well**: if the loop's task
   ends first, `start` returns with the loop's error.
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
another epoch than it was split off with. "Runs" is `Running` or `Releasing`: the
phases that have a runner. A region that is not running yet (it is being opened, or
waits as a part in memory) has no runner to keep aside and is dropped and opened as
today.

**The runner that is kept aside is stopped with whatever ends that opening**, and
never outlives it. While the region is being opened with its runner aside it is,
for everything else in the loop, a region that is being opened: in no `Serving`,
vouched for as waiting for the store, reported without players, not free for a
merge or a split. What ends the opening, and what becomes of the runner each time:

- **the store answers the hello**, whatever it answers: the runner is stopped in
  the background, as above. If the store took the hello the region runs with the
  new epoch; if it refused the epoch or has the region as absorbed, the loop does
  what it does for any opening that ends so;
- **orders name the region with yet another epoch**: the opening is dropped and the
  runner stopped in the background, both at once, and the region is opened with
  the newest epoch as one that is not running, with nothing aside;
- **orders no longer name the region**: the opening is dropped and the runner
  stopped in the background, as a running region is that orders no longer name;
- **a release is asked** of the region with the epoch it is being opened with: as
  for any region that is asked to be released while it is being opened (the word
  that it is released is said, the assignment is not taken up again), and the
  runner is stopped in the background;
- **the loop ends**, for whichever reason, an error of this very opening among
  them: the runner is stopped and **waited for with the runners of the loop's
  regions and those that were being stopped**, before the regions that were open to
  be absorbed are closed. The loop does not return while a thread of a runner it
  ever had still runs.

A merge or a split the runner was in the middle of goes with it in each of these:
taken out of the loop's merges and splits when the runner is put aside, and dropped
behind its stop.

**And the loop writes the line that says how long a region stood still** for a
merge or a split, through the call it hands each runner it makes (section 5.6): a
second change of step C5.5 to this file, which moves nothing either.

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
   the one it had; **or with the loop's error, if the loop's task ends first**
   (a region whose state cannot be read ends the loop, and nothing would ever show
   in the watch). It has no bound of its own beyond that: it waits for as long as
   the store takes to answer the hello, as the loop does.

It is a word the store did not say, said for a test; from there on it goes through
the paths a real refusal goes through, and the order of step 4 is the loop's own
for every region that is named with another epoch, in a cluster as here. Nothing in
it is for tests alone but the sender the server keeps. **That an old runner goes on
after it was fenced and harms nothing** is also what
`players_keep_playing_when_a_worker_wakes_up_after_its_region_went_to_another`
(`chaos.rs`) tests between processes.

**`stop` returns when nothing of the server holds the store any more and the store
is at rest**, so that a server started on the same directory right after it finds
the world as the store last made it durable and nobody else writing to it. The
server keeps its `Store` behind a lock that every use takes for as long as the call
lasts: the `store` of the worker's loop, and `lists`. `stop` does, in this order:

1. ends the edge's task and waits for it, which closes every client and every link;
2. says `Stop::AtOnce` to the worker's loop and **waits for the loop to return**.
   By then every thread of a runner has ended and every handle the loop had is let
   go of: of the runners of its regions, of those that were being stopped, of one
   that was kept aside (section 6.2), and of the regions that were open to be
   absorbed;
3. ends the coordinator's task and waits for it;
4. **takes the store out from behind the lock**, which waits for a hello or a
   reading of the list that is under way, on whichever thread, and after which
   either is answered `the server is stopping`;
5. **waits for the store to be at rest**: `Store::flush` (section 5.5), on a thread
   that may block. An error of it is logged (`the world store was not at rest when
   the server stopped`, with the error) and `stop` goes on: the next start then
   finds what a crash would have left, which the store is built for;
6. drops the store.

**Why step 5, which the first revision did not have.** It said that the store's
threads "end by themselves when the last handle is gone, with nothing left to
write". They do end by themselves, and nobody waits for them: they are detached,
and dropping a handle only queues a `Close`. For a runner that was only running
that made no difference, as it checkpoints and waits with its handle's own flush
before it lets go (`RegionRunner::run`). **A runner that is stopped in the middle of
a release, a merge or a split does not wait** (`Ended::Abandoned`): it lets go of
its handle while the checkpoint it asked for is a job of the thread for chunks,
or the record of its split is being appended and its part's file written on the
commit thread. Until this step the single process never met that, because it
merged and split nothing; from C5.7 every single process does. Without step 5 the
next `Server::start` in the same process reads the log and the region files while
the old store's threads still write them, and then both append to one log. A
process that exits takes the threads with it, which is a crash like any other; it
is a server started again in the same process that needs this, and that is every
test of `persistence.rs` and P5.

**What "stopped in the middle" means for a runner does not change.** The other
remedy, `flush` in the path that abandons, is two lines in `RegionRunner::run` and
would make a runner in a cluster wait for the store when it is told to let go of a
merge or a split, which is the one thing that path exists not to do (its comment:
"whoever stops a release, a merge or a split has waited long enough for the
store"). The runner lets go as it did; it is the server that waits, once, for the
store.

**What step 5 can find alive.** No handle of a runner, by step 2. One handle at
most that nobody ever held: a hello that was under way when the loop returned has
been answered by step 4, on a thread of its own, to a future that was dropped; the
handle in that answer is dropped a moment later, before or after step 5. It asked
for nothing, and its `Close` writes nothing (section 5.5). No clone of the `Store`
is made by the server: its parts reach the store through the lock.

#### 6.5 What it prints

What its parts print, in one log: the coordinator's lines (`a region was assigned`,
`the routing table changed`, `a split is begun by itself`, `a merge is begun by the
distances`, `an absorption is begun by itself`, `a merge has ended`, `a worker says
what came of a split`), the worker's (`given a region`, `running a region`, `the
merge has ended`, `the split has ended; opening the new region`, `a region stood
still for a merge or a split`) and the edge's
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
| a worker that failed a region is passed over for six leases, and nothing is begun by itself with its regions | others are given regions first | **no failure is noted** (`Coordinator::alone`) |
| a release, a merge or a split that has not ended within a lease | the region is taken, or the merge ends by what the list says | **as in a cluster, stage by stage: below** |
| no evening out within a lease of a merge or a split | | nothing to even out |

**What is overdue in one process**, by `end_overdue_releases`,
`end_overdue_reshapes`, `lapse_merge` and `lapse_split` as they are. "Overdue" is
more than a lease, five seconds, after it was asked; it takes a store that needs
that long for a checkpoint, or a process that was held up in the middle.

- **A release** (ADR-0009: for a move, for a worker that leaves, to even out):
  there is none. A move is refused with one worker (`MoveRefusal::NoTarget`), the
  worker never says that it leaves (`Stop::AtOnce`), and nothing is evened out. The
  only release there is in one process is the first stage of a merge.
- **A merge at its first stage**: the region to absorb was asked to be released and
  the worker has not said that it is. The merge ends (`a merge was not done within
  the lease`, `a merge has ended` with `NotReleased`), **the region is taken from
  the worker and, by the same tick, given back to it with another epoch**, as
  nobody else is there. The loop opens it with that epoch, which fences the runner
  that was releasing it, and it is restored from what the store had confirmed: one
  restore, for players who were standing still for the release already. The region
  that was to survive was only told to prepare and has ticked all along. **No
  failure is noted**, so nothing holds the next attempt back but the rest of the
  region that was just given out: the merge is begun again a rest later. This is
  the one way in which a single process still takes a region from its worker, and
  it has to be there: it is what ends a release that the store never answers.
- **A merge at its second stage**: the region to absorb is released and has no
  owner, and the worker was told to have the survivor absorb it and has not said
  what came of it. Nothing is taken from a runner. The list is read, and says: *it
  was absorbed*, and the merge has ended well; or *it lives*, and the merge ends as
  overdue and the region is given out, to the same worker, with another epoch.
  That hello and the record of the merge then meet at the store, which has them in
  one order: if the hello is first, it takes the region from the handle it was
  open with to be absorbed, the store declines the merge, the survivor ticks on
  and the region is restored as it was released; if the record is first, the hello
  is told that the region was absorbed, and the worker drops it and says so.
  Either way one region runs every chunk, and the merge was made whole or not at
  all.
- **A split**: the worker has not said within the lease what came of it. Whoever
  asked is told that it is overdue, the list is read, and **no region is taken**.
  The split goes on at the worker and is made or is not; a part that it makes is
  run by the worker, reported by it and found in the list.

So **the single process never takes a region from its worker for silence**: a
process that sleeps for a minute wakes with its worker registered and its regions
running. If it fell asleep in the tens of milliseconds of a merge's first stage,
one region is restored once when it wakes, and the merge is made a rest later; if
in the second stage, or in a split, the list says what became of it.

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
coordinator logs once, as a warning, at the first reading that shows a pinned region:
`the world
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
each other a rest later, by the same rule. **What this does not say**: that
everybody of a group is caught by its split. One who has left the chunks the split
names by the time it is worked out stays `H`'s, far out, and is split off a rest
later (statement L, and ADR-0016, "What the margin does not catch").

**N18. A coordinator starts anew after a region was let go, and has not read the
list.** The coordinator is killed in the middle of a move; the old owner finishes
releasing region `r`; a new coordinator starts; the worker registers, holding
nothing, which has the service begin its reading anew; and the worker's `Released
{ r }` is heard before that reading is back. The coordinator does not know `r` and
awaits its first list: **it keeps the word** (section 2.3). The reading comes back
with `r` living, without an owner, with the epoch the worker released it with or a
lower one: `r` is let go, and given to a worker by that very call of `listed`,
grace period or not, the worker that released it behind the others. Its players
stand still for a move and one reading of the list, and not for a lease. **If the
list has `r` with a higher epoch** (the coordinator before had given it to
somebody else, who ran it), **or does not have it**, the word is dropped: in the
first case `r` waits for its owner's report or for the grace period, in the second
there is no `r`. If the store is away, the word waits with everything else for the
first reading that succeeds.

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
| C5.0 | **Done.** This record reviewed twice, and revised after each. And two commits that change no behaviour: `bin/clustine/src/cluster.rs` cut into `cluster/coordinator.rs`, `worldstore.rs`, `worker.rs`, `edge.rs` and `commands.rs`, moved and not changed (`23a6c3b`); and the shared types of section 5.3 (`Reach`, `LocalCoordinator`, `serve_local` on `run` with a `Door`, the clients taking `impl Into<Reach>`, so that no caller changed) (`b841c6f`). `serve_local`'s coordinator is made with `new` until C5.2 gives it `alone` | `bin/clustine/src/cluster*`, `services/coordinator/src/{client,service,lib}.rs` | the lead | Q9; all four checks |
| C5.1 | The store: `Division::open`, `Division::side_by_side`; tests of what `Lanes::load` already does with them, a world with a `layout` file and no table among it, which is made over today whenever the store is told no fingerprint; the second line of the log when a world is made over (section 2.2). `Division::layout` and `stripes` stay until C5.9 | `services/worldstore` | delegated | T1 to T5, T7, T9 |
| C5.1b | The store can be waited for: `Store::flush` (section 5.5), a barrier through the thread for chunks and back. Nothing else of the store changes, and nothing calls it before C5.5 but tests. It adds a message, a job and a function to `lib.rs`, `lanes.rs` and `chunks.rs`, away from what C5.1 changes there (`Division`, and what `Lanes::load` logs): built beside C5.1 and merged behind it | `services/worldstore` | delegated | T10; every test of the crate |
| C5.2 | The coordinator: `new` knows no region; `knowing`; `alone`, which has no grace period, gives no worker up and notes no failure; `home()`, `awaits_the_list()`, `keeps_its_workers()`; a word of release kept while it awaits the list and judged in `listed`; `run` given its coordinator, and `serve_with`; the service reads until it has a list; N14's line. **Until C5.9 `serve` makes its coordinator `knowing` the stripes of a `CoordinatorConfig::layout` that has a boundary**, so that a coordinator started with `--boundaries` is as it was; one started without, and `serve_local`'s, knows no region | `services/coordinator` | delegated | Q1 to Q8, Q10, Q12 to Q14; every test of the crate, the state machine's helpers on `knowing` and the service's on `serve_with` |
| C5.3 | The worker's loop cut from its process (section 6.2): `run(setup, outside)`, and `clustine worker` gives it what it has today. **Nothing inside the loop moves**: its builder is told that the order of the arms, of what each does, of what goes into which watch and queue, and of what is stopped at the end, is not theirs to change, and that a change they think is needed is to be reported and not made | `cluster/worker.rs` | delegated, with tests written by someone else, and **read line by line against the file before it** when it comes back | the tests of `cluster/worker.rs` that drive the loop, unchanged in what they assert; P7 |
| C5.3a | **A split leaves no land ahead of those who go** (section 3.6), in two commits: the grants that wait go by nearness (`Region::split` with `granted`, `Sides`, `RegionRunner::commit` and `take_answer`, the fields of the runner's line); then what an edge asks for before it has heard of the split (`Parted`). **And a third, which has nothing to do with either and shares their file**: the runner says how long its region stood still (`Standstill`, `RegionRunner::with_standstills`, section 5.6). No edge code, no store code | `crates/clustine-sim/src/region/reshape.rs`, `services/worker/src/lib.rs` | the first commit and the third delegated; **the second by the lead, and not delegated**: it changes what a region answers an edge that resumes, which is the resume logic | S1 to S9, R1 to R12, written from sections 3.6, 5.6 and 9.5a by someone else; the 103 tests of ADR-0014's section 2.6 and the runner's tests of merges and splits; all four checks |
| C5.4 | The edge's process: `whole_world` waits for the home region; the home region from the table; a later table taken whatever its layout; links in this process (sections 5.4 and 6.3). The hello still carries the fingerprint | `cluster/edge.rs` | **the lead; not delegated** | every end-to-end test; `cluster.rs`; Q11 |
| C5.5 | The single process on the parts (section 6): `Config::pins` and `follow`; `Server::start` that returns when every region runs, `regions`, `take_over`, `stop`, which waits for the store with `Store::flush`; **the loop opens a region that is named with another epoch before it stops its runner**, and stops a runner it kept aside with whatever ends that opening (section 6.2), a commit of its own; **the loop writes `a region stood still for a merge or a split`** through the call it hands each runner (section 5.6), a commit of its own; the line that says how it reshapes; the flags `--pin`, `--reshape` and the distances for it and `--pin` for the store, **with `--reshape by-hand` still what both do unless told, and `--boundaries` still taken, as `--pin`**. `common::config()` turns `CLUSTINE_TEST_BOUNDARIES` into pins | `bin/clustine/src/{lib,main}.rs`, `cluster/worker.rs`, `tests/common/mod.rs` | **the lead; not delegated** | every test of the single process in both runs; P1 to P6, P9, P10; T6 but for the refusals; `chaos.rs` and `moves.rs`, for the loop |
| C5.6 | The tests leave `--boundaries` (section 9.2): `Cluster::new` passes `--pin` to the store where it has pins, and then `--reshape by-hand` to the coordinator; `CLUSTINE_TEST_PINS`; `tools/check.sh`, `ci.yml`, `CLAUDE.md`'s four lines; the one test of `reshapes.rs` that needs the layout rewritten; **the base manifests lose `--boundaries=4`, the overlay is added**, and the kind test runs on the overlay (section 8) | `bin/clustine/tests`, `tools/check.sh`, `.github/workflows/ci.yml`, `deploy/` | delegated | themselves; `deploy/kind/test.sh` on GitHub |
| C5.7 | **`by-itself` is what both do unless told.** `common::config()` follows the players when it has no pins. The comments of the manifests; section 3.5's line; the kind test's new part (G1) | `main.rs`, `cluster/coordinator.rs`, `tests/common/mod.rs`, `coordinator_flags.rs`, `deploy/` | the lead | the lines of P8 that need no refusal; G1; every check |
| C5.8 | End to end without stripes, under the bots, **written from this record by someone who built none of the above**: W1 to W11, the chaos and the moves without pins (Y1 to Y6), X1; what they measure goes into the roadmap | `bin/clustine/tests/wanders.rs`, `crowds.rs`, `chaos.rs`, `moves.rs`, `common/` | delegated | themselves, ten runs in a row |
| C5.9 | **The layout goes**: everything "Deleted" in section 4; `--boundaries` refused; `serve` makes its coordinator with `new`; **`README.md` (three places) and `CLAUDE.md` ("is what they run") name the commands there are**, `cargo run -p clustine` for what the owner runs and `--pin` where a boundary is meant, in this commit, as the flag they name exits with 2 from here on | every crate, `README.md`, `CLAUDE.md` | the lead | T8, Q8, the refusals of T6 and P8; every check; the comparisons with the official server; kind |
| C5.10 | Only if X1 says so (section 9.7): the survivor of a merge and the region of a split keep their links. A record of its own first | `services/edge`, `services/worker` | **the lead; not delegated** | its own |
| C5.11 | Roadmap: where M3 stands, what was measured, section 10 for the owner, and that the tests of chaos and of moves are pinned on purpose but for Y1 to Y6. Documentation proper is step C6 | `docs/roadmap.md` | the lead | CI |

C5.1, C5.2, C5.3 and C5.3a are built side by side once C5.0 is pushed: each owns
files no other of them touches. **C5.1 and C5.3 are being built from this record
as it was before the second review, and nothing of what they are has changed.**
C5.1b is the one step that shares a crate with another, C5.1, and lands behind it.
It has to be in before C5.5, whose `stop` calls it, and before the scenario R1 of
C5.3a is run, which waits with it. C5.4 and C5.5 follow in that order; C5.6 can be
prepared beside them and lands after C5.5. C5.3a has to be in before C5.8, whose
tests it is there for, and can be in at any time before: it changes nothing a
pinned world shows. Its third commit has to be in before the commit of C5.5 that
writes the line. The scenarios T, Q, P, S and R are written by somebody other
than the builder of each step, from this record, and can be written while the step
is built.

**What the builder of C5.3a is told.** The interface of section 3.6.1, as code; that
`Region::split` stays a function that changes nothing, and that every equality of
ADR-0014, section 2.6, has to hold as it does (a region after `take_split` and the
part each equal what `Region::restore` makes of the same state and holdings); that
`waiting_for_the_tick` is called before the region takes the split and why; that the
store, the edge and the coordinator are not theirs to touch, and that a `Decline`
from the store for a chunk of the part means the design is wrong and is to be
reported; the three conditions of section 3.6.5; and, for the third commit, section
5.6, with what the number is and is not. How to verify: the scenarios, then the
four checks. **The second commit is the lead's**: the rule of section 3.6.2 with
what an edge has said it has, its four steps with the functions named there, of
which the edge's are read and not changed, and "When a belief is wrong".

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
- **T10. The store at rest** (section 5.5; step C5.1b). A store on a disk in memory
  that tells what a crash would leave of it and counts what is done to it, as the
  store's tests of kills have it, and with a generator of chunks that the test can
  hold: it says when it is entered for a chunk and returns when it is let go.
  1. *The thread for chunks is waited for.* A region is opened and commits three
     ticks. Then, without waiting for any answer: a load of a chunk that was never
     stored, which enters the generator and is held there; a checkpoint of the
     third tick; and the handle is dropped, as a runner that is abandoned drops it.
     The barrier is asked for and not waited for (the crate's tests reach the two
     halves of `Store::flush`). The list of regions is read and answered, so the
     commit thread has had the barrier's turn: **the barrier is not answered**, and
     a store started on what a crash would leave now restores the region with
     three commits and no state of the third tick. The generator is let go. The
     barrier is answered `Ok`. **A store started on what a crash would leave at
     that moment, with nothing that was not durable, restores the region from the
     state of the third tick and no commit.**
  2. *The commit thread is waited for.* A split is handed to the store through a
     handle that is dropped at once, as the store's tests of splits hand one; then
     `flush`. What a crash would leave when it returns has the split: the list of a
     store started on it has the part, with the chunks it was split off with.
  3. *Nothing is written afterwards.* After 1 and after 2: the count of what was
     done to the disk is read when `flush` has returned; the store and every handle
     are dropped; when both threads have ended (the generator tells the test when
     it is dropped, which the thread for chunks does last) the count is the same.
  4. *Answers are out.* A handle asks for a claim and for its own flush and reads no
     answer; `Store::flush` returns; without anything more being waited for, the
     handle's queue has `Claimed` and `Flushed`, in that order.
  5. *It closes nothing.* With another region's handle open throughout: after
     `flush`, a commit through that handle is confirmed and a `flush` of the handle
     returns; and a second `Store::flush`, and one through a clone, return `Ok`.
  6. *After a write that failed*, on a disk that fails the sync of a group once:
     `flush` returns `Ok`, every handle is lost, and what the group wrote is not in
     what a crash would leave. On a disk that fails everything from that sync on:
     `flush` returns `StoreError::Io`, as `regions` does, and returns.

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
  begins to serve (a test that drives it by hand begins that reading itself, as
  `run` does) and once more at every tick at which no reading is under way; after the
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
  coordinator `alone`; the first review found it listed under C5.0, which has no such
  coordinator yet.)
- **Q11.** The edge's process, against a coordinator that is played: it does not
  listen for players while the table has no home region, or no route for it, or a
  region waiting; it does when all three hold; a later table with `home: None` is
  taken for its routes; a join goes to the home region of the first table.
- **Q12. A release before the first list** (section 2.3, N18). A coordinator made
  with `new`, within its grace period, handed no list; worker `a` registers holding
  nothing, worker `b` too. `a` says it released region 5 with epoch 40. **That call
  changes nothing**: it returns what a call returns that changed nothing, region 5
  is not known, the routing table has no route and `waiting: 0`. Then a list is
  handed in, with the home region 0 and:
  1. **region 5 living with epoch 40**: by that very call of `listed` region 5 has
     an owner, `b` (`a` is behind it), with an epoch above 40, though the grace
     period is not over; region 0 has none, and waits for it. With epoch 12 in the
     list: the same.
  2. The same with `a` alone registered: the owner is `a`, with an epoch above 40.
  3. **Region 5 living with epoch 41**: the word is dropped. Region 5 is known
     without an owner and is assigned by the first tick at or after the end of the
     grace period and by nothing before; the epoch it is assigned with is above 41.
  4. **No region 5** (the list has `(5, 2)` among the absorbed; or `next: 9` and no
     region 5; or `next: 4`): the word is dropped and region 5 is not known, then
     or after any later call.
  5. **`b` registers holding region 5 with epoch 41 before the list is handed in**:
     `b` owns it with 41 by its report, and the list (living, epoch 41) changes
     nothing; `a`'s word is dropped.
  6. **`a` is no longer registered when the list is handed in** (its lease ran
     out): the word is dropped and nothing fails; region 5 is known without an
     owner, is not let go, and is given out like any region of a list.
  7. With no worker that can be given it (`a` has lost its connection and `b` is
     not there; a worker that says it leaves while it owns nothing is forgotten by
     that call, and its word is then case 6's), after 1's list: region 5 is known without an owner, let go, and
     the table has `waiting: 2`; `b` registers, and is given region 5 by that call
     or the next tick, within the grace period.
  8. **Two words, in the order said**, with a third worker `c` registered: `a` says
     region 5 with 40 and then `c` says region 5 with 42; the list has it living
     with 40: its owner is `b`, with an epoch above 42, and both `a` and `c` are
     behind `b`. The same word said twice by
     `a` is one word.
  9. **A reading that fails keeps the word**: `unlisted`, then the list of 1: as 1.
  10. **After the first list nothing is kept**: a list without region 5 is handed
      in, `a` says the word, a second list has region 5 living with epoch 40:
      region 5 waits out the grace period like any region of a list.
  11. **A coordinator made `knowing`** regions 0 and 1 and handed no list: `a` says
      it released region 5 with 40, and a list that has region 5 living with 40 is
      handed in: region 5 is added without an owner and is not let go.
      (`released_from_anyone_but_the_owner_with_its_epoch_changes_nothing` is the
      test there is of the first half.)
  12. The word from a name that is not registered: nothing is kept. A region the
      coordinator knows without an owner (a list named it) and that is released
      so: as today, by the case there is.

  And through the service, with a `lists` whose answer the test holds back: a worker
  registers, says `Released`, is given nothing; the answer is let go, and the
  worker's orders name the region before any tick has run.
- **Q13. `alone` gives nobody up.** A coordinator made `alone` with one worker that
  owns a region. Nothing is heard of the worker for ten leases, and then a tick:
  the worker is registered, owns the region with the epoch it had and is not at
  fault. The same with heartbeats that name no region for ten leases. The same
  made with `new`: the worker is forgotten, and the region taken. The service that
  serves an `alone` coordinator closes the connection of no worker for silence, and
  closes one that never said what it is.
  **And it notes no failure.** Made `alone`, deciding by itself with a rest of one
  lease, one worker that owns regions 0 and 1 and reports players of the two
  within the merge distance, at every look, throughout. `a merge is begun by the
  distances`; the worker never says that it released region 1; more than a lease
  after the merge was asked, a tick: the merge has ended with `NotReleased`, and
  by that same tick region 1 is the worker's again with a higher epoch. The worker
  reports it with that epoch. **The merge is begun again when the two regions have
  been left alone as after any merge that came to nothing** (ADR-0016, section 5.5:
  three rests), which is well before six leases have passed since the first ended. The same if
  nothing at all is heard of the worker for ten leases and then one tick comes,
  followed by its reports. Made with `new` (and heartbeats, so that the worker is
  not forgotten): the same up to the higher epoch, and then nothing is begun with
  either region until six leases after the first merge ended.
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
  of the list and now and then a merge or a split under way: **in ten of the fifty
  lives `stop` is called when the second bot has been beyond the split distance for
  a second and `regions()` does not have its part yet**, which is when its split is
  begun, so that now and then a runner is stopped in the middle of one. Every start
  works whichever way that split went, and the list of the next life has the part
  or has not, and never half of it. (`persistence.rs` has one round. What makes
  sure of it is the store's barrier, T10; this is that `stop` calls it.)
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
  made so.) **The log is the process's standard error, written to a file**, as
  `common/processes.rs` keeps the logs of a cluster's processes: `spawn_server`
  sends it nowhere today and gets a way to say where. **Twelve seconds, and not
  more than fourteen**: more than two leases, and less than the edge's keep-alive
  interval of fifteen. The edge disconnects a client when a keep-alive is due and
  the one before it is unanswered (`services/edge/src/play.rs`). A keep-alive that
  was unanswered when the process was stopped has its answer in the socket when
  the process wakes, and the next is due no sooner than fifteen seconds after it
  was sent, so it is read first; and the edge gives up on a silent client only
  after thirty seconds. A stop of fifteen seconds or more could meet the
  keep-alive, and would be a test of something else.

#### 9.5a Scenarios: the split (section 3.6)

Written from section 3.6 by someone who built none of it. `S` are of the simulation
(`crates/clustine-sim`), with regions made as the tests of ADR-0014's section 2
make them. `R` are of the runner, stepped by hand, **on the store itself**, as
`services/worker/tests/reshape.rs` makes its worlds, so that what the store's list
says can be asserted; where a scenario says "killed", with whatever those tests
have to drop a runner and to start the store again from what a crash keeps. **None
of them needs a double of the store**, or answers that the test holds back: where
an order of answers matters (R1), the scenario says how the order of asking and
one wait for the store make it. (The runner's own tests, in
`services/worker/src/lib.rs`, have a gate on a real store's handle, `Gate`, which
can hold each kind of answer back by itself; R1 says what it would do with it.)
Chunks are `(x, z)`; the home chunk is `(0, 0)`.

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
  viewer's subscription to both views. **How the grant is made to wait**, with the
  store as it is and nothing held back:
  1. the link sends `Subscribe` for `(23, -3)` to `(23, 3)`, and the runner is
     stepped once: a tick, `T`, which takes it, and behind which the runner sends
     the claim;
  2. `SplitOff` naming `(19, 0)` is given at once, with no step in between. The
     first checkpoint and its flush are asked **behind the claim**, so the store
     answers the claim first: a handle's requests are answered in their order
     (`Lanes::request` holds a job back behind what is not durable yet, and
     `end_group` answers the claim and only then passes the flush on);
  3. **the test waits for the store, and not for the runner**: `Store::flush` (step
     C5.1b), which returns when both answers are in the handle's queue (T10.4);
  4. the runner is stepped: it takes both answers in that one step, puts the grant
     among the inputs of the coming tick, finds its flush answered and goes to
     `Stage::Settling` without a tick (`RegionRunner::step`, the arm of
     `Phase::Preparing`; `take_replies`). **The grant waits**, and no tick runs
     again before the split.

  A step between 2 and 3 would spoil it either way: if only the claim's answer is
  there, a tick runs and takes the grant. (With the gate of the runner's own tests
  the same is had without step C5.1b: the answers to flushes are held back from
  before 2 and let go in place of 3.) Then it is stepped to the outcome, which is
  `Split`; the part holds the seven; the region knows nothing of them; the store's
  list has them within the part's `bounds` and not within the region's; the log
  line has `players=1 chunks=56 waited=7`.
- **R2.** The same with the seven chunks at `(-4, -3)` to `(-4, 3)`: the region
  holds them, `waited=0`.
- **R3. Killed.** R1, and the runner dropped at the first step of each stage, and
  after the outcome before the part is opened; and the store killed at each write
  and sync it makes from the commit of the split on. Each time both regions are
  opened anew where the store has them: what each is restored holding is what the
  store's list grants it, no chunk is held by both, and the seven are the part's
  if the list has the part and the region's if it has not. **And the one case
  section 3.6.5 allows**: R2's split, with the store killed when the record of the
  split is durable and before it has answered. The runner's outcome is
  `Off::StoreLost`; the region is opened anew and is the region after the split; a
  link says R5's hello. **The region claims the seven at `(23, ..)`, which nobody
  holds, and is granted them**, and claims the 49 that went and is told the
  part's: it has no `Parted`. The test asserts that, so that whoever closes this
  hole later finds the test that says it was open.
- **R4.** A chunk the store had delivered for a request from before the region gave
  the chunk back, which is granted again by an answer no tick took and goes to the
  part: `Part::chunks` does not have it, and the part reads it from the store.
- **R5. The second way.** After R2's split (no grant waits), the link having been
  closed by it: a new link says hello naming `s` and `p` and, as a viewer's, the 49
  chunks around `(0, 0)`, the 49 around `(19, 0)` and the seven at `(23, ..)`, which
  nobody holds. In the tick that takes it the store is sent **no claim that names
  any of the 56**, and each of them is answered `Elsewhere` with the part; the
  welcome is followed by the `SplitOff` and by `Absent` for `p`; the log line has
  `chunks=56 free=7`. **The chunks around `(0, 0)` are served by the tick after**:
  they are warm from the split, and a warm chunk is handed to the next tick as the
  store's delivery would be (`RegionRunner::tick`).
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
- **R10. A third region's chunk.** A store with two regions pinned side by side,
  `A` to `x < 30` and `B` to `x >= 30`; in `A`, `s` at the home chunk and `p` in
  `(27, 0)`, a link with a viewer's subscription to both views, so that `A` believes
  `(30, -3)` to `(30, 3)` to be `B`'s. `p` is split off, no grant waiting. A new
  link says hello naming both views as a viewer's: the seven are answered
  `Elsewhere` **with the part**, and the store is sent no claim for them. The link
  sends `Subscribe` once more for `(30, 0)`, as an edge does that the part told
  `NotMine`: the region claims it, the store says `B`'s, and it is answered
  `Elsewhere` with `B`.
- **R11. An edge that has heard.** After R5, and without R6, the link is dropped
  before the runner is stepped again: the edge has read the `SplitOff`, and no
  tick has taken its confirmation. A new link of the same edge says hello with the
  `since` it was welcomed with and the number of the `SplitOff` as `seen`, and is
  answered as a resume; it names as a viewer's `(22, 0)`, which went, and
  `(24, 0)`, which nobody holds and is on the part's side. **Both are claimed** in
  the tick that takes the hello; the first is told the part's by the store, a tick
  or two later, the second is granted and served. No line `chunks asked for
  players who went …` is written for that hello. And on R5's own link, in place of
  R6: the link sends `Confirm` for the `SplitOff` and behind it `Subscribe` for
  `(24, 0)`: it is claimed and granted, whether or not one step takes both.
- **R12. The standstill** (section 5.6). A runner made `with_standstills`, the
  call noting what it is told. R2's split, stepped to the outcome: nothing is told
  yet. One step more, which runs a tick: **one `Standstill`, with `players` 2 and
  `held` the number of chunks the region held before the split** by what its ticks
  had been told (98 here);
  `milliseconds` is whatever it took and is not asserted. No second one, however
  many ticks follow. A split that comes to nothing after the region stopped (the
  chunks named have nobody in them: `Off::Nobody`): one, at the first tick after.
  An absorption: one, with the survivor's players and chunks as they were before
  it. A release, a runner that loses the store in the middle of a split, and one
  that is stopped there: none.

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
  as `follows.rs` checks it); no wait was longer than 5 s; **the workers' logs
  have `a region stood still for a merge or a split region=0` once for every split
  and every merge that ended well**, with `players=3` for a split and `players=2`
  for a merge (those region 0 had when it stopped). Then the end of
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
  all, and the round is gone through again until it has been. **What is not
  asserted after a kill at a split**, of the store or of the split region's worker:
  that no player is handed over and that region 0 is not split a second time.
  Region 0 is then restored without the line of its split (section 3.6.5), and if
  `B` crossed a chunk border in the tick the split stopped at, `B` is handed back
  to region 0 some seconds later and split off again a rest after that. Both
  outcomes pass; nobody is disconnected in either, the list comes to regions 0 and
  a part with `B`'s chunk in its `bounds`, and the audit holds.
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
  the wanderer was not disconnected; the log of the worker that ran region 0 has `a
  region stood still for a merge or a split region=0 players=3` once for the round.
  Three rounds,
  the wanderer of each leaving at its end and the next joining when the last one's
  region has been absorbed or 40 s have passed. In the single process as well,
  without the move (below).
- **W11. Straight on, at a sprint in flight, and at every second tick** (N17). This
  is the scenario that meets the two ticks of section 3.6 on purpose. `A`
  `within(0)`. **Eight wanderers** join and walk to the lanes z = `112 k` + 8.5 for
  `k` = -4 to 3: seven chunks from lane to lane, so that no two have a chunk in view
  together and each has subscriptions of its own, and near enough for all eight and
  the home chunk to be one group. Wanderer `i` (0 to 7) stands at x = 8.5 - `2 i`.
  When all stand, they are sent together, in the same turn of the test, to
  x = 16 * 49 + 8.5 on their lanes, **at one block a tick: twenty blocks a second,
  which is a sprint in creative flight and the pace the owner flies at**, a chunk
  every sixteen ticks. The first version of this scenario walked at two blocks a
  tick, at which a group leaves the three chunks of a split's margin in 1.2 to
  1.6 s: no more than a report, an order, a checkpoint and a flush take on a
  machine that runs several clusters at once, so that the split caught some of
  the eight and not the others, and the test took what followed for the fault it
  looks for. At one block a tick the margin is 2.4 to 3.2 s. With their starts two
  blocks apart **a chunk border is crossed by one of the eight in every second
  tick**, so a split that stops at a tick `T` finds, about every second time, one
  whose `Subscribe` was taken by `T` (the first way), and about every second time
  one who crossed in `T` (the second). They arrive and stay for 40 s.

  **Which rounds count.** A round counts if **the first split of region 0 that is
  begun in it** ended with `Ok(n)` and the worker's line `a part of the region has
  been split off` for it has `players=8`: the split caught the whole group. What
  comes after that split does not decide whether the round counts; it is what is
  asserted.
  **A round in which the margin caught only some of them, or none, is told by that
  line and by nothing else**: it has `players` below 8, or there is no such line
  and the coordinator's `a worker says what came of a split` has an outcome that is
  not `Ok` ("not yet", ADR-0016, section 5.5). The fault of section 3.6 cannot look
  like that: in it all eight go with the split, the line has `players=8`, and the
  hand-over and the second split come seconds later. In a round that does not
  count, those who were not caught are region 0's own players far out, and what
  follows is ADR-0016's and right: region 0 is split again when it has rested,
  that part is merged with the first, and somebody may be handed over on the way.
  **Such a round is not a failure and is not a round that counts.** Of it the test
  asserts only that nobody was disconnected and that no wait was longer than 5 s;
  it notes the round, by its kind, for its message at the end; and it goes on to
  the next round as after any other: the eight leave, and the next eight join when
  the regions of the round have been absorbed or 40 s have passed, as in W10. Its
  lines `waited` and `free` are not counted either.

  **Asserted of every round that counts**, from the wanderers' being sent to the
  end of their 40 s: no second split is begun, and no merge and no absorption
  that names region 0 or region `n` (what regions earlier rounds left empty do
  among themselves is not this round's);
  **no worker logs `player arrived from another region` or `player departed to
  another region`**; of the `bounds`, only what holds: **from the end of the split
  on, at every reading of the list (four a second), region 0's east end never
  moves east**, and at the end region `n`'s `bounds` are x = 46 to 52 and z = -31
  to 24. (Not that region 0's `bounds` end west of where region `n`'s begin, which
  W10 asserts of its one wanderer: right after this split, on a build that does
  everything right, region `n`'s begin at x = -4 and region 0's end at x = 11.
  The eight stood on lanes up to 28 chunks from the home chunk, the land around
  where they stood is nearer to them than to anybody who stayed and goes with
  them, and on the lane z = 0 the chunks up to x = 11 are nearer to the home
  chunk and stay, until both are given back thirty seconds later.) No bot of `A`
  waited longer than it waits when nothing happens, but once; no wanderer was
  disconnected.

  **And that the run met what it is for**: over the rounds that count, the
  worker's line `a part of the region has been split off` has been written at
  least once with `waited` above 0, and the line `chunks asked for players who
  went are taken for the part's` at least once with `free` above 0. **Rounds until
  three have counted and both lines were seen, twelve rounds at most**, counted or
  not. A run that ends without one of the two lines, or with fewer than three
  rounds that counted, **fails as not having tested**, with how many rounds
  counted, how many did not and why, and how often each line was seen. Why twelve
  is enough: a round that counts meets each of the two ticks with a chance near a
  half, reckoned and not measured (the wanderers' ticks are their own, on their
  own timers, and slip against the region's), so one of them is missed in twelve
  such rounds less than once in a thousand runs; the lines say what was met, and the
  test goes
  by them and not by the reckoning. A slow machine makes both lines more likely
  and rounds that do not count more likely as well, which is why those are told
  apart and not failed. Before section 3.6 is built this scenario fails in about
  every second round that counts (a hand-over is logged and region 0 is split a
  second time), which is how its writer checks it.

**In the single process**, the same file. **W1 without the move, W2 and W3** run on
a `Server` in the test's own process whose `follow` has those distances and that
rest, and `view_distance: 2`, observed through `Server::regions()` and the bots'
waits; and the end of W1 with the server stopped and started from its disk. Of
those three only what `regions()` and the bots show is asserted there: a `Server`
in the test's process has nothing else to look at, and its log is the test's own
output. **W10 and W11 run against a server process that the test starts**
(`spawn_server`, which gets two things for it: where the process's standard error
is written, as P10 needs it too, and the view distance, which it sets to the
tests' own today; here 2, with `--rest-seconds 5`), because what they are for is
in the log: the
lines `player arrived from another region` and `player departed to another
region`, the coordinator's lines of what was begun, and for W11 the two lines it
counts by and the one it tells its rounds by. The list is not to be had from
another process (nothing prints it, open question 3), so of the `bounds` these
two assert nothing in the single process; the cluster's run of them does.

**Chaos and moves without pins.** The tests of `chaos.rs` and `moves.rs` that there
are stay pinned (section 9.2). Six of them get a twin on the default world, and the
two harnesses take such a world with this much rewriting and no more:

- `Chaos::start` and `Moves::start` take **a world** where they take boundaries:
  `World::Pinned(&[i32])`, which is everything as it is, or `World::Following`.
- **`World::Following`**: `Cluster::new` with no pins; the coordinator with
  `--view-distance 2 --rest-seconds 5` and the edge with `--view-distance 2`; four
  bots of one ledger on lanes 19 chunks apart (`Ledger::first_lane` 0 and
  `lane_spacing` 304, as W9 spaces its lanes), with `lines` empty; **five
  workers**, one for each region there will be and one to spare, so that while
  all five live no worker runs two more than another and the coordinator moves
  nothing by itself under the test. `start` returns when the store's list has four
  regions, each bot's chunk within the `bounds` of a region of its own, every one
  of them runs and has rested, and no move, merge or split is under way. **How
  fast the bots go to their lanes and how long `start` waits for all that is the
  harness's to set by the distance**, as `Moves::start` sets `Ledger::to_the_lane`
  for its wide lanes: the farthest lane is 912 blocks from the spawn point, three
  of the four bots are split off one split and one rest at a time (N11), and each
  part is moved once. The sixty seconds of `PATIENCE`, which suit lanes side by
  side, are not that bound. From then on the bots stay where they are, so **the
  coordinator has nothing to merge or split until the ledger's auditor walks at
  the end**, and what happens in between is what the test did.
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
  assert `report.actions > 0` alone. **Weaker**: when the bots have been told to
  end, the ledger's auditor joins and walks from the spawn point to lanes 19, 38
  and 57 chunks away; it is merged into each region it comes within 10 chunks of
  and split off again, so regions are absorbed and made. **Nothing is asserted of
  the list or of what the coordinator began from that moment on**, but that every
  region runs and that the audits hold.

| | The test, and its twin of today | What it does on the default world |
|---|---|---|
| Y1 | `chaos.rs`: `players_of_regions_that_follow_them_keep_playing_when_a_worker_wakes_up_after_its_region_went_to_another` (twin of the same name without the first five words) | freezes and wakes the owner of a region with players, three times; the region it had is a part three times in four |
| Y2 | `chaos.rs`: `players_join_and_leave_while_the_home_region_of_a_world_without_pins_has_no_worker` (`players_join_and_leave_while_the_region_they_do_it_in_has_no_worker`) | a guest leaves and a visitor joins right after the home region's worker is killed; the visitor is let in when another worker has it, is in the home region, and builds beside the spawn point |
| Y3 | `chaos.rs`: `players_of_regions_that_follow_them_keep_playing_when_an_owner_and_its_heir_are_killed` (the rounds of `players_keep_playing_while_the_workers_that_run_their_regions_are_killed` that call `kill_owner_and_heir`) | kills the owner of a part and then the worker that is given it, once while that one restores it and once just after |
| Y4 | `moves.rs`: `a_worker_that_is_told_to_stop_hands_the_regions_that_follow_players_over_first` (`a_worker_that_is_told_to_stop_hands_its_region_over_first`) | tells the owner of a part to stop; it is gone within a few seconds, the spare runs the part, nobody waited for the lease |
| Y5 | `moves.rs`: `players_of_regions_that_follow_them_keep_playing_while_every_worker_is_replaced_in_turn` (`players_keep_playing_while_every_worker_is_replaced_in_turn`) | replaces all five workers in turn |
| Y6 | `moves.rs`: `a_part_is_moved_by_hand_and_its_players_stand_still_only_briefly` (`players_stand_still_only_briefly_while_their_regions_are_moved_back_and_forth`, at the small view) | `clustine move --region n` for a part, to the spare and back, each a rest after the last; the pause of its bot within the bound, the others' as when nothing happens |

**What each asserts besides what its twin asserts, and of which time.** The
harness takes **a mark just before it tells the bots to end**: in `Chaos::finish`
and its like in `Moves`, behind the last `tend` and before `Progress::finish`, it
reads the store's list and notes how long the coordinator's log is. Everything
here is about the time **from the end of `start` to that mark**, and nothing about
what comes after it, when the auditor walks.

- **The list at the mark** has the four regions it had at the end of `start`, the
  same `next`, and the same absorbed pairs: no region was made and none absorbed.
- **The coordinator's log up to the mark has none of** `a split is begun by
  itself`, `a merge is begun by the distances` and `an absorption is begun by
  itself`. Any of them fails the twin: the bots stood still, so the world moved
  under the test and what it measured is not what it says.
- **`a region is moved to even regions out` fails the twin unless the test has
  taken a worker away before that line**: killed it, frozen it or told it to stop.
  From the first time it has, the line is allowed, any number of times. Why: with
  four regions on five workers a region goes to a worker that has one already
  only when no worker without a region is registered at that moment, which takes
  a worker that is gone and one that has not come back; and when the one that was
  away registers again, some worker has two regions more than it and the
  coordinator, rightly, moves one. In Y3 that is ordinary: the owner is killed,
  then its heir, and the first is started again at that moment; if it has not
  registered when the heir's lease runs out, the part goes to a worker that has a
  region. **Nothing in these tests rests on a process registering within a lease**
  or within any other time: a fixed bound that was long enough locally is what
  failed on CI before. In Y6, which takes no worker away, the line always fails
  the twin; in the others it fails it only before the first kill, freeze or stop.

The other way to the same end, a sixth worker, so that two can be away and a
region still finds a worker without one, was not taken: it only moves the bound
to three.

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
  how long the coordinator took by its own lines, **and the `milliseconds`, `players`
  and `held` of the worker's line `a region stood still for a merge or a split
  region=0` for it**, which has to be there for every one that ended well; and
  what a bot of the crowd waits when nothing happens. Printed as the least, the
  middle and the worst of each, as `moves.rs` prints its pauses. **And how long the
  store takes to make its list** while the crowd stands and nothing else happens:
  twenty calls of
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
  `milliseconds`, so that an operator sees a number close to the one a player
  felt. Section 5.6 has who measures it (the runner, in step C5.3a), who writes
  it (the worker's loop, in step C5.5), what its fields are exactly, and what the
  number leaves out: the edge's way back to the region, and for a merge the longer
  stop of those who were in the region that was absorbed.
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
are then numbered from where that world's numbers had got to and not from 1: from
2 if no region was ever split off in it, and higher if any was, as in the owner's,
which went through the trials of steps C3 and C4. The lines `a split is begun by
itself region=0 part=…` say which numbers they are; everything else below is the
same.

Two clients, both in creative mode, where flying is a double tap on the jump key. F3
shows the block and the chunk. **Nobody can see what a region holds**: nothing prints
the store's list (open question 3). So every expectation below is something on a
client's screen or a line of the log, quoted as it is written; the lines carry more
fields than are quoted. A stop of a fifth of a second is not something a lone
player sees on their own screen, where they move without the server; the log says
when it happened and for how long the region did not tick (`a region stood still
for a merge or a split region=… players=… held=… milliseconds=…`, with those four
fields and no others; `players` and `held` are what the region had when it
stopped), and two players who look at each other see the other's figure stop for
that long and a little longer, as their client is told again only when the edge
has found its way back to the region.

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
   `a split is begun by itself region=0 part=1`, and then, within the same moment
   and in either order, `a worker says what came of a split` with
   `outcome=Ok(RegionId(1))` and `a region stood still for a merge or a split
   region=0 players=2`, with a number of `milliseconds` that nobody has measured
   yet: of the fifth of a second that step C3 measured as what a player waits, it
   is the part in which the region did not tick. Say what it was. On the screens:
   nothing you should notice.
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
   with `outcome=Ok`, and `a region stood still for a merge or a split region=0
   players=1`: it is the stop of the one at the spawn point; the one who flew
   stood still for longer, and no line says how long (section 5.6). You are 350
   blocks apart and cannot see each other.
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
  before the edge has read of its split still knows it. It is the one hole
  section 3.6.5 leaves, and closing it changes what a state file and a record of a
  split hold, for a restore that has to fall between a split and the edge's
  reading of it (a store or a worker killed at the split; a takeover) and a player
  who crossed a chunk border in its last tick besides. Open question 9.
- **The edge reading of the split before it says what its players see**: a hello
  without chunks, and the subscriptions sent when the entries of the welcome have
  been read. It would close the second way with no belief at all. It costs a round
  trip in every resume, at every merge, split, move and takeover, for everybody of
  the region, which is the very pause X1 is there to measure, and it changes the
  hold of a hello in the runner and how the edge takes a link.
- **Answering by the line of the split in the runner alone**, with no ticket
  counted and nothing told to the simulation. The same messages and no belief; but
  a subscription that counts no ticket has to be honoured in three places (a
  change of kind, an unsubscribe, the end of the link), and a mistake in one leaks
  a ticket or takes one that is another's.
- **A guest's ticket no longer keeping a chunk**, so that land a region holds only
  because another region's player looks at it goes to that region. It would undo
  the strip half a minute late, when the player has long walked into it, and it is
  a change to who holds what wherever two regions' lands touch.
- **Keeping what a worker says it released until the first list, in the service**,
  and saying it to the state machine then; and **believing it at once**, with the
  region noted as known on the worker's word, which the first revision of this
  record decided and the second review took apart. Section 2.3: the state machine
  keeps the word and `listed` judges it.
- **A close of the store that joins its two threads**, in place of a barrier, and
  **a flush in the path of a runner that is stopped in the middle of a merge or a
  split.** Sections 5.5 and 6.4.
- **Telling the runner which region it runs**, so that it writes the line of its
  standstill itself. The number would be in the runner for one line of the log and
  every maker of a runner would have to say it. The loop hands the runner a call
  instead (section 5.6).
- **The loop reading the standstill off the runner's status at its look**, four
  times a second. It works; the line would stand up to a quarter of a second
  behind the tick it is about, in a log whose order the owner is asked to read.
- **A sixth worker under Y1 to Y6**, so that nothing is evened out after a kill.
  Section 9.6: it moves the bound from two workers away to three.
- **W11 at two blocks a tick.** Section 9.6: a group that fast outruns the margin
  of its own split on a busy machine, and what follows looks like the fault the
  scenario is there to find.
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
- A coordinator can do nothing until it has read the store's list once, but
  believe what workers say they run. What a worker says it let go of before that,
  it keeps, and the first list decides.
- A coordinator whose workers are in its own process gives none of them up and
  notes no failure of one. It still takes a region for a merge whose release was
  never answered, and gives it back.
- The store can be waited for (`Store::flush`), and the single process waits for it
  when it stops. A store's threads are still detached and still end by themselves.
- A worker's log says how long each region stood still for a merge or a split,
  which is a little less than what its players waited.
- A split takes with the part what the region was granted and no tick had heard
  of, if it lies on the part's side; and a region that was split answers an edge
  that has not heard of it by the line of the split, without asking the store. The
  second is in the runner's memory and not in the store's: a region that is
  restored before the edge has heard answers as it did before this step. And the
  answer can name the part for a chunk that is a third region's, which the edge's
  asking again puts right.
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
- A split does not catch a player of a group who has outrun its margin; they are
  split off a rest later. That is ADR-0016's, and this step makes it the one way
  left in which a group that walks on stands the spawn point still twice.
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
   worker, takes no region for silence and notes no failure of a worker, so that
   **section 5.2's "is not at fault"** always holds there (sections 2.3 and 6.6
   here).
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
   not catch" is as it was, and is what statement L here does not cover.
10. **Section 2.3, what a coordinator takes on a worker's word**: nothing more.
   That a region it does not know was released, said before it has read a list, it
   keeps and has the first list decide (section 2.3 here).
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
   that has not said that it has the `SplitOff` (by a `Confirm`, or by the `seen`
   of a hello that resumes), `A`'s runner answers by the line of the split, in the
   tick that takes the hello, and without asking the store, for the chunks that
   went and for every chunk on the part's side that `A` knows nothing of: those
   nobody holds, and those a third region holds, of which the answer is wrong
   until the edge asks again. For any other edge, and after a restore, it is as
   written.
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
   the store has answered, or with whatever else ends that opening (section 6.2
   here).
12. **ADR-0011, section 2, the `Store`**: it has `flush`, which waits until the
   store is at rest with everything asked before (section 5.5 here). That a store
   stops by itself when it and every handle it has handed out are dropped stands.
13. **ADR-0014, section 3.1, what a runner tells of a merge or a split**: besides
   the outcome, how long the region stood still for it, when it ticks on (section
   5.6 here).
14. **ADR-0009, that a worker which did not answer a release is passed over for a
   while**: not by a coordinator made `alone` (section 2.3 here).

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
9. **Whether the line of a split should be in the region's state** (section 3.6.5),
   so that a region restored between its split and the edge's reading of it (a
   store or a worker killed at the split, a takeover) still answers by it: only if
   what that would cover is ever seen in play. W7 meets it on purpose and says what
   it does not assert for it.

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
  subscriptions. It was read in the code (`RegionRunner::tick`,
  `Fanout::handle_entry`) and is promised by no rule of the contract. If it did not
  hold, the edge would take `Elsewhere` for a viewer's subscription and ask the
  part as a guest; when it then reads the `SplitOff` it ends its subscription at
  the region, which was told elsewhere, and asks the part as a viewer, so it ends
  as it should, unless the part's `NotMine` is handled in between: then the edge
  asks the region again, which asks the store, and that is the strip of section
  3.6 again, but nothing worse. **It does not rest on `SubscribeAsGuest` coming
  before `Confirm`**, which the first revision listed here as well (section
  3.6.2, "What ends a belief"). R5 and R6 pin the runner's half; W11 says by its
  lines whether the whole worked.
- **A belief the store did not give.** For a few ticks after a split, and for as
  long as somebody who stayed looks at such a chunk, the region that was split
  believes chunks to be the part's on its runner's word. Right of what went; a
  guess that comes true of a chunk nobody holds; **wrong of a chunk a third region
  holds**, which is ordinary on pinned worlds and is put right by the edge asking
  again, with one hop more for a block action meanwhile (section 3.6.2, "When a
  belief is wrong"; R10). It is the first belief in this server that is not the
  store's, and section 3.6.5 is the whole of the reasoning that it moves no player
  who stands still.
- **A region restored between its split and the edge's reading of it answers as
  before this step** (section 3.6.5): a store or a worker killed at a split, a
  takeover. With a player who crossed a chunk border in the split's last tick,
  that is one hand-over and one more split. W7 and R3 meet it and say what they
  do not assert.
- **W11 may not meet its ticks, or its splits may not catch its group.** It says
  which, round by round, and fails as not having tested if twelve rounds were not
  enough; if that happens often, its bots' pace or their number is wrong, not the
  server.
- **Y1 to Y6 rest on nothing being merged or split by the coordinator under
  them**, which bots that stay put are to make sure of, and on nothing being moved
  by it before the test takes a worker away, which five workers for four regions
  are to make sure of. Each checks the list and the log for it, up to the moment
  the bots are told to end, and fails if the world moved.
- **`Store::flush` is new code in the store's two threads**, small and beside a
  path that is there, and the first thing in the store that waits for both. If it
  answered too soon, P5 would fail now and then with a log that two stores wrote;
  T10 holds the thread for chunks still to see that it waits.
- **The line of a standstill measures the region and not the player.** What it
  leaves out is said (section 5.6), and X1 prints both; nobody knows the
  difference yet.
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
  regions (section 6.6, P10), with one exception: if it fell asleep in the first
  stage of a merge, the region that was to be absorbed is taken and given back
  when it wakes, which is one restore, and the merge is made a rest later (Q13).
  A merge at its second stage and a split end by what the list says.
- **`stop` waits for the worker's loop to return**, and the loop waits for every
  runner's last checkpoint; **and then for the store to be at rest**. A store that
  does not answer keeps `stop` from returning, where today it keeps a runner's
  thread from ending, which `stop` also waits for.
- **A runner that is kept aside while its region is opened anew** is one more
  thing the worker's loop has to end in every way an opening can end (section
  6.2). One that were left would hold its links until it is fenced and its thread
  for good; the loop's end waits for it, so a test of the single process that
  stops would hang and show it.

## Not checked

- **Nothing was built and no test was run for this record**, in its first version,
  in its revision or in this one. `cargo fmt --all --check` was run at the end of
  each, to see that no code was touched.
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
- **Read for the second revision, by its writer and nobody else**, at `b841c6f`,
  for the parts the second review left to be designed. For `Store::flush`: the
  whole of `worldstore/src/lib.rs`; in `lanes.rs` `run`, `handle`, `close`,
  `request`, `end_group`, `fail_log`, `returned`, `install`, `open`, `admit`,
  `split`, `lose` and the log's `fail` and `settle`; in `chunks.rs` the jobs and
  `ChunkService::run` and `work`; the memory disk of the store's tests and the
  head of `kill.rs`. For the word of release: `released`, `listed`, `unlisted`,
  `without_owner_since`, `holds`, `assign`, `grant`, and the two tests of
  `released` named in section 2.3. For `alone`: `end_overdue_reshapes`,
  `lapse_merge`, `lapse_split`, `merge_listed`, `end_merge`, `absorb_released`,
  `end_overdue_releases`, `hand_over`, `note_failure`, `at_fault`, `lightest`,
  `take_unvouched`, `even_out`, and `free_but_for_its_rest` in `follow.rs`. For
  the standstill: in the runner `step`, `take_command`, `carry_on`, `commit`,
  `tick_on`, `take_answer`, `begin_anew`, `end`, `run`, `show_status`, the head of
  `tick` to where it asks the store, `hello` and `subscribe`; the whole loop of
  `cluster/worker.rs` from where it is set up to its end. For the rest:
  `service.rs` from `serve` to `tick_interval` and the helpers of its tests; the
  head of `client.rs`; the edge's `elsewhere`, `not_mine` and `ask_those_told`,
  and the keep-alive of `play.rs`; `settle_chunks`, `restore` and what a region
  does with an arrival into a chunk it believes another's; `Chaos::finish` and
  `kill_owner_and_heir`; `spawn_server`; `Server::take_over` and `stop` as they
  are; the gate of the runner's own tests; the test of a chunk asked for right
  before a split. The second review's own readings were taken as it gives them
  where this list does not name the place.
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
- **That a store can be put under a `Server` that holds back commits** (P9) was
  not looked up; the scenario says what stands in its place if not. R1 was looked
  up and needs nothing held back (section 9.5a).
- **That `Store::flush` needs one round and no more** follows from what was read of
  who sends the thread for chunks a job; it was not run. T10 is written to fail
  if the barrier is answered on the commit thread alone.
- **That a stop of twelve seconds cannot meet the edge's keep-alive** (P10) was
  reckoned from the interval and from how the edge's task waits, not tried; it
  goes against a doubt of the second review, which saw a window of a millisecond.
- **How much less than a player's wait the line of a standstill says** is not
  known; and the bots' twelve rounds in W11 are reckoned like its three were.
- **That C5.1b and C5.1 do not meet in a line of `lanes.rs` or `lib.rs`** was
  judged from what each adds, with C5.1 not yet in.
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

### The first review

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
to Y6). A release said to a coordinator that had read no list was dropped: the
revision had it believed at once, and the second review changed that to kept and
judged by the list (2.3, N18, Q12). W3's bound forgot that a move is a restore (9.6). The
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
store, and P5 tries it fifty times (6.4); the second review found what that
left out. A store started with other pins under
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

### The second review

A second reviewer went over what the revision had added, against the code of
`b841c6f`, again without building or running anything, and found **ten defects**,
the tenth being five small contradictions, **and five doubts**. On section 3.6,
which was the part its writer was least sure of, the verdict was: both causes are
real by the code, both remedies do what they say in every order of messages that
could be constructed, **build it, with changes** to what the record claimed about
the second remedy and to the scenario that is to show it. All ten were accepted
and are worked into the sections above.

**The worst were not in section 3.6.** `Server::stop` returned while the store's
two threads could still be writing for a runner that had been stopped in the
middle of a merge or a split, so that the next start in the same process read a
log another thread was appending to; and P5, which was there to show that `stop`
is clean, was the test that would have hit it. The store gets the one thing it
lacked, a way to be waited for (`Store::flush`, a barrier and not a join; 5.5,
6.4, T10, step C5.1b), and what "stopped in the middle" means for a runner is left
alone. **W11 asserted something a correct build does not do**, and walked so fast
that a split on a busy machine caught only part of its group, which looks exactly
like the fault it was written to find: it walks at a sprint, asserts of the
`bounds` only what holds, tells a split that caught everybody by a field of the
runner's line, and neither fails nor counts a round in which it did not; statement
L says what it does not cover (3.6, 9.6). **A coordinator made `alone` still put
its one worker at fault** for a merge whose release was overdue, and so did what
`alone` is there to prevent, thirty seconds without a merge or a split: it notes
no failure, and section 6.6 says what really happens at each stage (2.3, 6.6,
Q13). **A release said before the first list** was believed at once, which
contradicted a test there is, could leave the coordinator with a region the store
does not have, and checked no epoch: the word is kept by the state machine and
judged by the first list, by the case there is (2.3, N18, Q12).

**The others**, each where it is now. The line the owner is told to read at every
merge and split was built by no step: the runner measures, the worker's loop
writes, with four fields, in steps C5.3a and C5.5, and section 10 quotes it as it
will be written and no longer promises a number (5.6, 9.7, 10, R12). Y1 to Y6
asserted "at its end" what is false once the auditor has walked, and failed a
correct build that evened regions out after a kill: the list and the log are
marked just before the bots are told to end, and an evening out is allowed once
the test has taken a worker away (9.6). Section 3.6.2 said less than is true:
the belief can be wrong of a third region's chunk, which the edge's asking again
puts right; a region is restored without the line of its split whenever the store
dies at the commit or the region is taken over before the edge has read of it,
and not only when a worker dies within ticks; and nothing rests on the order of
`SubscribeAsGuest` and `Confirm`. An edge that has said it has the entry is
answered as ever (3.6.2, 3.6.4, 3.6.5, R3, R10, R11, W7). W10 and W11 could not
see in a `Server` of the test's own process what they assert: there they run
against a server process and read its log (9.6). The runner that is kept aside
while its region is opened anew had no end in four cases: it has one in each
(6.2), and `start` and `take_over` return the loop's error and go by the latest
table (6.1, 6.4). And five small things: step C5.0 was a commit behind, and section
5.3 now fits what that commit built, with the coordinator handed to `run`; R1 says
how its grant is made to wait without anything held back; the second commit of
C5.3a is the lead's; a world from before is numbered from where its numbers had
got to; and R5's own chunks are served a tick later than it said.

**Working it in found a little more.** The rule of section 3.6.2 has to go by what
an edge has said on its link, a `Confirm` as well as a hello's `seen`, as either
reaches the region's state only with the next tick. `spawn_server` throws its
process's log away and fixes the view distance, and gets a way to say both, for
P10 and for W10 and W11. And the first of the five doubts did not hold up: a
process stopped for twelve seconds cannot meet the edge's keep-alive, whose
interval is fifteen, and P10 says so and why it must not be stopped for longer.
Of the others, the harness's bound for bots that walk to lanes far apart is the
harness's to set by the distance (9.6), and three need nothing: the two tests of
the runner that are about the very ticks section 3.6.2 changes are among those
step C5.3a has to keep, the lock on the store is held for a whole hello to no
harm, and an edge that starts in the middle of a merge waits a moment longer.

**What neither reviewer could check stays unchecked**, as "Not checked" has it:
nothing has been run, and every rate and every duration is reckoned.

## Found while building

What the builders decided where the record could be read in two ways, and what the
tests written from it found. Where a sentence above was sharpened for it, that is
said.

**Step C5.1, the store.** Two things the store did not do as section 2.2 says, both
mended in `Lanes::load`: the line of a world made over was written only if a region
had something left to put into the chunks, and is now written with the second line
whenever a start makes a world over, once the new table is durable; and a `layout`
file that is no fingerprint ended the start, where a store that is told no
fingerprint now does not read the file at all. After a world is made over, the
region file of a region that is gone stays if it has entity ids, so that they are
not issued again, and a region made later under that id begins with the epoch the
file has: T3's list differs from T1's in region 0's epoch as well as in `next`, and
T4's region 1 has the epoch of the part that was region 1. `side_by_side(home,
&[])` is one region pinned to the whole world, and not `open`.

**Step C5.1b, `Store::flush`.** One round is enough, as section 5.5 reckoned. A
handle in another process counts only once what it asked has arrived. Besides the
unsettled log, `flush` answers `StoreError::Io` if a thread of the store has gone,
where `regions` panics. A hello that is handled after `flush` does write (the
region file and its `Opened` record), so "nothing is written afterwards" holds
because `stop` lets no hello be under way, which is its step 4.

**Step C5.2, the coordinator.** Sharpened above: what `forget_silent` still does
for a coordinator that is alone; N14's line once for each coordinator; Q12.7, Q13
and Q5. Besides: the words of release that are kept have no cap, as the regions
`report` takes on a worker's word have none; the first reading is begun by `run`
and not by `Service::new`; `serve` makes its coordinator `knowing` the stripes of a
layout with a boundary until step C5.9, which is the one call of `knowing` that is
no test's; a tick exactly a lease after a coordinator was made is outside its grace
period; and `note_failure` doing nothing for a coordinator that is alone covers a
release that was not answered as well as a merge.

**Step C5.3, the worker's loop.** Nothing inside the loop moved but what became a
parameter, and one line at its end: the loop lets go of the watch of what it serves
where it used to end the task that accepts edges, and the process stops accepting
when that watch is closed, so the listener closes before the regions are stopped,
as it did. A closed watch is a worker that serves nothing any more, which is what
`start` and `take_over` watch for beside the loop's end. An opening that was on a
blocking thread when its future was dropped runs to its end and closes its handle
only then, so the loop's return does not say that no handle is open: `stop`'s
`Store::flush` covers it only if that hello had reached the store, which the lock
of section 6.4 is for.

**Step C5.3a, the split.** `waited` counts the chunks of the part that were named
in the grants and that the region did not hold by its ticks. `take_split` does not
check that it is handed the grants `split` was. Section 3.6.4's "both regions are,
in memory, what `Region::restore` makes of the store's record" holds of the part
always and of the split region on land that is not pinned; a pinned region that is
restored holds none of its area and claims anew, as before this step, and the
store's `bounds` of a pinned region are `None`. In section 3.6.2 the number an edge
has said on its link is kept for each link (`heard`): set by a hello that is
answered as a resume, raised by a `Confirm`. A stop that is noted for a standstill
stays if the region stops again before it has ticked, and the runner reads no clock
when nobody is to be told.

