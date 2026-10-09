# ADR-0017: The end of the stripes

- Status: **Proposed.** The design of step C5 of milestone M3, phase C: stripes,
  `Layout` and `--boundaries` go, and the single process and the cluster run regions
  that follow their players unless told otherwise. Not yet reviewed, and nothing of it
  is built.
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
`c43b21c`. "Not checked" at the end says what was read only in excerpts, and by whom.

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
  `bin/clustine/src/cluster.rs`), puts the fingerprint into its hellos to workers, and
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
- **The service around it** (`service.rs`) knows nothing of TCP but in `serve_from`,
  which accepts connections and hands each to `Service::attach` as an `End<FromCoordinator,
  ToCoordinator>`. `clustine_rpc::link::in_process` makes such a pair of ends for any
  two message types.
- **The worker process** (`worker` in `cluster.rs`) is one loop of about 700 lines over
  its regions' phases (`Opening`, `Starting`, `Running`, `Releasing`) and the merges
  and splits under way. It reaches the outside in three places: `stay_registered`,
  which holds the connection to the coordinator and speaks to the loop through watches
  and queues; `open_region` and `fetch`, which open a region at the store's address;
  and `accept_edges`, which attaches links to the regions in a watch (`Serving`). A
  region the coordinator names with another epoch than the worker runs it with is
  stopped and opened again with the new one ("the coordinator has taken the region;
  letting go of it", then "given a region"). A region that has lost the store is opened
  again.
- **The single process** (`bin/clustine/src/lib.rs`) has none of that. `Server::start`
  makes a store for the stripes of `Config::boundaries`, opens every stripe
  (`run_first`, trying epochs from 1 up), runs each on a `Worker`, and hands the edge
  the links with `home = layout.region_of(spawn chunk)`. It has no coordinator, merges
  and splits nothing, moves nothing, and does not open a region again that has lost
  the store. `Server::take_over(region)`, for tests, opens the region with the next
  epoch, which fences its runner, and gives the edge the new link.
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
7. **Nothing changes in the simulation, in the region runner or in the edge's
   fan-out.** What is new for them is that what they were built for now happens.

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
chunk, with the epoch and the entity ids that stripe 0 had in its region file. Players
do not outlive a store that stops with its regions, so nothing of theirs is lost. The
store says so in its log once (`the world was divided otherwise before; what its
regions had is in the stored chunks now`). It is done before any hello is taken, and a
store that dies in the middle does it again.

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
and the coordinator has not been handed a list yet (`Coordinator::home()` is `None`).
That is every 250 ms for one that decides by itself and a quarter of the lease for
one that does not. After the first list everything is as ADR-0016, section 7, has it:
on a timer for the one, on events for the other.

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

**The grace period can be left out.** `Coordinator::without_grace(config, now,
first_epoch)` is `new` for a coordinator whose world nobody else can have been
running: it assigns at once, evens out at once and decides at once. The three tests
of the grace period become one time, `grace_until`, which `new` sets a lease from
`now` and `without_grace` sets to `now`. Only the single process uses it (section 6).

**For the tests of the state machine**, `Coordinator::knowing(config, now,
first_epoch, regions)` makes a coordinator that knows these regions from the start,
without owners and without a home, as `new` did for the stripes. It is `#[doc(hidden)]`
and no process calls it. Why it exists: some seven hundred tests of the coordinator
(`state.rs`, `state/tests/follows.rs`, `tests/reshape.rs`, `tests/decides.rs`,
`tests/follows.rs`, `service.rs`) are about what a coordinator does with regions it
knows, and say `Layout::new(vec![0])` only to have two. Handing each a list instead
would change the version of its routing table, its home region and what it may be
asked, and would have to be gone through test by test. What a coordinator does that
learns its regions from the list has tests of its own (section 9, Q1 to Q8).

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

As built; nothing of it changes. A reader who writes tests needs it exact, and this
step is the first to rest on it.

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
    pub fn new(config: CoordinatorConfig, now: Instant, first_epoch: u64) -> Self;           // knows no region
    pub fn without_grace(config: CoordinatorConfig, now: Instant, first_epoch: u64) -> Self; // section 2.3
    #[doc(hidden)]
    pub fn knowing(config: CoordinatorConfig, now: Instant, first_epoch: u64,
                   regions: &[RegionId]) -> Self;                                            // tests only
    pub fn register(&mut self, now: Instant, name: &str, address: &str,
                    holding: &[Assignment]) -> Changes;                                      // refuses nobody
    pub fn home(&self) -> Option<RegionId>;                                                  // of the last list
}
pub struct Orders { pub spawn: Vec3, pub assignments: Vec<Assignment> }
```

And how a client reaches a coordinator, for the single process:

```rust
/// Where a coordinator is: at an address, or in this process.
#[derive(Clone)]
pub enum Reach { Tcp(String), Local(LocalCoordinator) }

/// The way to a coordinator in this process; clones lead to the same one.
#[derive(Clone)]
pub struct LocalCoordinator { /* a sender of the service's ends of new connections */ }

/// A coordinator in this process and the way to it. The future serves until it is
/// dropped. Its coordinator is made `without_grace`.
pub fn serve_local<L>(config: CoordinatorConfig, lists: L)
    -> (LocalCoordinator, impl Future<Output = ()>)
where L: Fn() -> io::Result<RegionList> + Send + Sync + 'static;
```

`WorkerClient::register`, `register_with_heartbeat`, `RoutingWatch::connect`,
`Asker::merge`, `Asker::split` and `Mover::ask` take a `&Reach` where they take an
address. `Reach::Local` makes a pair with `link::in_process(QUEUE)` and hands the
service its end; `serve_local` is `serve_from` with those ends where it accepts from
a listener, and is otherwise the same loop: the same `Service`, the same order of
calls, the same ticks, the same thread for a reading of the list. A local connection
ends when either side drops its end, as a TCP connection does when it is closed.

Breaks: every `CoordinatorConfig` literal and every call of `register` (ten helpers
in the coordinator's tests, `follows.rs` and `reports.rs` under `bin/clustine/tests`),
the tests of `Refusal::Layout`, which go, and every caller of the clients above.

#### 5.4 The edge process

`edge`, `whole_world` and `keep_linked` in `cluster.rs`. No change to
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
store of this step reads every world a store of today wrote.

### 6. The single process

#### 6.1 What it is made of

`Server::start` makes, in this order:

1. **The store**: `Store::local_divided` or `Store::memory_divided` with
   `Division::open(home)`, or `Division::side_by_side(home, &config.pins)` if there are
   pins.
2. **The coordinator**: `serve_local(CoordinatorConfig { spawn, lease:
   CoordinatorConfig::DEFAULT_LEASE, follow: config.follow }, lists)`, with `lists`
   the store's own `Store::regions`, on a task. No grace period.
3. **One worker**, named `local`, with the address `in this process`: the loop of
   `cluster::worker` (section 6.2), which registers through `Reach::Local`, opens its
   regions with `Store::open_region`, and shows the regions it runs in a watch.
4. **The edge**: `whole_world` through `Reach::Local`, then `Routing::new` with the
   table's home region and `Edge::bind`, and `keep_linked`, which attaches a link in
   this process to the region the watch shows (section 6.3).

It returns when the edge listens. By then the list has been read, the home region
assigned, opened and restored, and the routing table has named it; the link to it is
made within milliseconds, and a join that comes before it waits for it, as in a
cluster.

**So the single process calls the state machine exactly as the coordinator's process
does**, because the same `Service` does the calling: outcomes with the readings they
ask for, then where the players are, then the tick, in the order point 11 of ADR-0016's
"Found while building" asks for; every `Changes::read` answered; heartbeats beside
the reports. ADR-0016 expected the single process to make those calls itself and
listed what it would have to keep to. It makes none.

#### 6.2 The worker's loop, with three ways out

`cluster::worker` becomes a loop that is given its three ways out, and the process
gives it the ones it has today:

| | The process | The single process |
|---|---|---|
| The coordinator | `Reach::Tcp(address)` | `Reach::Local(..)` |
| The store | `StoreHandle::connect(address, hello)`, tried again while the store cannot be reached | `Store::open_region(hello)` |
| Edges | a listener, `accept_edges` | the watch of `Serving` itself, handed to the link-keeper |
| Told to stop | the signal: it says that it leaves and waits up to 20 s to be relieved; a second signal stops it at once | `Server::stop`: at once, as the second signal does |

Everything else is the loop as it is: phases, merges and splits under way, the queue
of outcomes and reports with its registrations, the regions that lost the store.
`stay_registered` registers again through the same `Reach` if its connection ends,
which in one process it does only if the service gives the worker up.

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
keeps its meaning: the region is given a new runner with a higher epoch, its runner
is not asked, the edge resumes with the new one, nobody is disconnected. How: the
server says to its coordinator, as its worker and through the queue the worker's loop
says its refusals through, `EpochRefused { region, seen }` with `seen` one above the
epoch the region is run with. `Coordinator::epoch_refused` takes
the region from the worker and gives it away again with an epoch above `seen`, to the
only worker there is; the loop finds its region named with another epoch, stops the
runner it has in the background and opens the region with the new epoch, which
fences that runner at the store; the edge is given the route and links.
`take_over` returns when the worker shows the region running with an epoch above the
one it had, and fails if the world has no such region. It is a word the store did not
say, said for a test, and it goes through the paths a real refusal goes through. The
order in which the old runner's last save and the new one's hello reach the store is
not fixed, as it is not when a coordinator takes a region from a worker that lives.

#### 6.5 What it prints

What its parts print, in one log: the coordinator's lines (`reshaping by itself: …`
with the distances, `a region was assigned`, `the routing table changed`, `a split is
begun by itself`, `a merge is begun by the distances`, `an absorption is begun by
itself`, `a merge has ended`, `a worker says what came of a split`), the worker's
(`given a region`, `running a region`, `the merge has ended`, `the split has ended;
opening the new region`) and the edge's (`linked to a region`, `listening`). Today it
prints `listening` and little else. The lines are the same text as in a cluster, so
what the owner is told to look for is one list for both.

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
at them. *Given back*: `P`'s claim is granted like any other. When `H` is opened the
next time, what its log has of such a chunk is not replayed into it, as `H` does not
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

**N15. Somebody stands in a chunk that is asked for and not granted when a split is
ordered.** They are no seed; with nobody else in the chunks named the answer is "not
yet" (ADR-0016, section 5.5). Off stripes this needs a claim that is seconds late.

**N16. A world from before.** Section 2.2: made over before any hello, and then N12
and what follows.

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

### 9. Building it

`main` is green at every commit and every test there is passes at every commit; the
layout is taken away last, when nothing reads it. The steps that can be built side by
side own files that no other step touches. What reaches into the edge's process, and
the single process, is built by one person and is not delegated.

#### 9.1 The steps

| # | Scope | Files | By | Its tests |
|---|---|---|---|---|
| C5.0 | This record reviewed. **Then two commits that change no behaviour**: `bin/clustine/src/cluster.rs` cut into `cluster/coordinator.rs`, `worldstore.rs`, `worker.rs`, `edge.rs` and `commands.rs`, moved and not changed; and the shared types of section 5.3 (`Reach`, `LocalCoordinator`, `serve_local`, the clients taking `impl Into<Reach>`, with `&str` and `String` being a `Reach::Tcp`, so that no caller changes) | `bin/clustine/src/cluster*`, `services/coordinator/src/{client,service,lib}.rs` | the lead | Q9, Q10; all four checks |
| C5.1 | The store: `Division::open`, `Division::side_by_side`; tests of what `Lanes::load` already does with them, a world with a `layout` file and no table among it, which is made over today whenever the store is told no fingerprint. `Division::layout` and `stripes` stay until C5.9 | `services/worldstore` | delegated | T1 to T5, T7 |
| C5.2 | The coordinator: `new` knows no region; `knowing`; `without_grace`; `home()`; the service reads until it has a list; N14's line. **Until C5.9 `serve` makes its coordinator `knowing` the stripes of a `CoordinatorConfig::layout` that has a boundary**, so that a coordinator started with `--boundaries` is as it was; one started without, and `serve_local`'s, knows no region | `services/coordinator` | delegated | Q1 to Q8; every test of the crate, their helpers on `knowing` |
| C5.3 | The worker's loop with its three ways out (section 6.2); `clustine worker` gives it the ones it has | `cluster/worker.rs` | delegated, read line by line when it comes back | the tests of `cluster.rs` that drive the loop; P7 |
| C5.4 | The edge's process: `whole_world` waits for the home region; the home region from the table; a later table taken whatever its layout; links in this process (sections 5.4 and 6.3). The hello still carries the fingerprint | `cluster/edge.rs` | **the lead; not delegated** | every end-to-end test; `cluster.rs`; Q11 |
| C5.5 | The single process on the parts (section 6): `Config::pins` and `follow`; `Server::regions`, `take_over`; the flags `--pin`, `--reshape` and the distances for it and `--pin` for the store, **with `--reshape by-hand` still what both do unless told, and `--boundaries` still taken, as `--pin`**. `common::config()` turns `CLUSTINE_TEST_BOUNDARIES` into pins | `bin/clustine/src/{lib,main}.rs`, `tests/common/mod.rs` | **the lead; not delegated** | every test of the single process in both runs; P1 to P6; T6 but for the refusals |
| C5.6 | The tests leave `--boundaries` (section 9.2): `Cluster::new` passes `--pin` to the store and `--reshape by-hand` to the coordinator; `CLUSTINE_TEST_PINS`; `tools/check.sh`, `ci.yml`, `CLAUDE.md`'s four lines; the one test of `reshapes.rs` that needs the layout rewritten; the kind test on the overlay | `bin/clustine/tests`, `tools/check.sh`, `.github/workflows/ci.yml`, `deploy/` | delegated | themselves; `deploy/kind/test.sh` on GitHub |
| C5.7 | **`by-itself` is what both do unless told.** `common::config()` follows the players when it has no pins. The manifests of section 8; section 3.5's line; the kind test's new part (G1) | `main.rs`, `cluster/coordinator.rs`, `tests/common/mod.rs`, `coordinator_flags.rs`, `deploy/` | the lead | the two lines of P8; G1; every check |
| C5.8 | End to end without stripes, under the bots, **written from this record by someone who built none of the above**: W1 to W9, X1; what they measure goes into the roadmap | `bin/clustine/tests/wanders.rs`, `crowds.rs` | delegated | themselves, ten runs in a row |
| C5.9 | **The layout goes**: everything "Deleted" in section 4; `--boundaries` refused; `serve` makes its coordinator with `new` | every crate | the lead | T8, Q8, the refusals of T6 and P8; every check; the comparisons with the official server; kind |
| C5.10 | Only if X1 says so (section 9.7): the survivor of a merge and the region of a split keep their links. A record of its own first | `services/edge`, `services/worker` | **the lead; not delegated** | its own |
| C5.11 | Roadmap: where M3 stands, what was measured, section 10 for the owner. `README.md` and `CLAUDE.md` where they name a command that is gone. Documentation proper is step C6 | `docs/roadmap.md`, `README.md`, `CLAUDE.md` | the lead | CI |

C5.1, C5.2 and C5.3 are built side by side once C5.0 is pushed; C5.4 and C5.5 follow
in that order; C5.6 can be prepared beside them and lands after C5.5. The scenarios
T, Q and P are written by somebody other than the builder of each step, from this
record, and can be written while the step is built.

**What is true between the steps.** From C5.2 to C5.9 a coordinator's process that is
given `--boundaries` still knows its stripes from them, and reads the list as well;
from C5.6 no test gives it any, so every cluster of the tests has a coordinator that
learns its regions from the list three steps before the layout goes. From C5.5 the
single process runs on the coordinator's state machine whether its world is pinned
or not; a single process that reshapes by hand with no pins is one home region.
From C5.6 no test names `--boundaries`. From C5.7 a process started without flags
follows its players.

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

`Cluster::new(directory, workers, pins)` keeps its three parameters; an empty `pins`
is a world without. It passes `--reshape by-hand` to the coordinator unless
`coordinator_arguments` has a `--reshape` of its own.

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
- **Q3.** The same with `without_grace`: assigned by the call that hands in the list,
  if a worker is registered by then, and otherwise by the first tick or list after
  one has registered.
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
  that is registered without waiting for a lease.
- **Q11.** The edge's process, against a coordinator that is played: it does not
  listen for players while the table has no home region, or no route for it, or a
  region waiting; it does when all three hold; a later table with `home: None` is
  taken for its routes; a join goes to the home region of the first table.

#### 9.5 Scenarios: the single process

Written from section 6. A `Server` in the test's own process, observed through its
address and `Server::regions()`.

- **P1.** Started on a new world without pins: `regions()` is T1's list, with an
  epoch for region 0; a bot joins and is sent `view_area` of the spawn chunk.
- **P2.** With `pins: vec![1]` and `follow: None`: two regions, pinned; a bot walks
  across block x = 16 and back and is one entity to a watcher. (`handoff.rs` has it.)
- **P3.** `take_over(r)`: `regions()` shows a higher epoch for `r`; a bot in `r` is
  not disconnected and what it was acknowledged is there. `take_over` of a region the
  world does not have is an error. (`takeover.rs` has the first.)
- **P4.** Started on the disk of a world that had three parts when it was stopped,
  with a `follow` whose rest is one second: within twenty seconds `regions()` has the
  home region alone, and the parts in `absorbed`.
- **P5.** Stopped and started again: every block that was acknowledged is there.
  (`persistence.rs` has it.)
- **P6.** Stopped while a bot is connected: `stop` returns, and does not wait twenty
  seconds for another worker.
- **P7.** The worker's loop, given a store in its process and a coordinator through
  `Reach::Local`, runs a region it is assigned and shows it in its watch of the
  regions it serves; a link attached there is welcomed.
- **P8.** `clustine --boundaries 4` exits with 2 and the sentence of section 8; so do
  the two subcommands (from C5.9). `clustine --pin 4` logs N14's line when it has
  read its list; `clustine --merge-distance 3 --split-distance 6` logs section 3.5's.

#### 9.6 Scenarios: end to end without stripes, under the ledger bots

`bin/clustine/tests/wanders.rs`, written from this section and from sections 3 and 7
by someone who built none of it.

**The cluster**: two workers, a store without `--pin`, an edge with `--view-distance
2`, a coordinator with `--view-distance 2 --rest-seconds 5` and nothing else, so that
it reshapes by itself with **the distances the rule gives: 10 and 18 chunks**, a
margin of 3 and a rest of 5 s. With a view distance of 2 a client is sent the 7 by 7
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
     region, from when `B` is sent until 35 s after it has arrived. Then region `n`'s
     `bounds` are x = 37 to 43 and z = -3 to 3 exactly, and region 0's x = -3 to 3
     and z = -3 to 3.
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
  the list keeps regions 0 and 1; within 35 s region 1 has no `bounds`. A second
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
  disconnected; the region's `bounds` are W1's 35 s after; the audit finds every
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

**In the single process**, the same file, with a `Server` whose `follow` has those
distances and that rest, and `view_distance: 2`: W1 without the move, W2 and W3,
observed through `Server::regions()` and the bots' waits; and the end of W1 with the
server stopped and started from its disk.

**The pinned tests again.** Every test of `chaos.rs`, `moves.rs`, `merges.rs`,
`reshapes.rs` and `follows.rs` passes from C5.6 on with `--pin`, and that is what
"every chaos and move test again" means here: their worlds are pinned on purpose.
What chaos does to a world without pins is W5 to W8.

#### 9.7 The crowd

**X1**, `bin/clustine/tests/crowds.rs`. It answers two questions that nobody has
measured: how long a merge and a split stand a large home region still, and whether
that grows with its players or with its land.

- **The world**: a cluster of two workers without pins; the edge and the coordinator
  with the same `--view-distance` (`CLUSTINE_CROWD_VIEW`), and `--rest-seconds 5`. An
  ordinary run of the tests has a view distance of 2, so the distances 10 and 18; the
  measurement has 8, so 22 and 30.
- **The crowd**: `N` bots (`CLUSTINE_CROWD`, 20 in an ordinary run) in ledgers of
  twenty, ledger `g` `within(g)` on the lanes z = 0 to 76: a hundred players are five
  chunks by five around the spawn point, all of the home region. `e` is the crowd's
  easternmost chunk, 0 for twenty and 4 for a hundred.
- **Those who go**: a ledger of two on the lanes z = 80 and 84. It is sent `within(e
  + D_s + 2)`, is split off, and is sent back `within(e + D_m)`, and is merged: with
  a hundred at a view distance of 8 that is chunk 36 and chunk 26. Twice in an
  ordinary run, five times in the measurement (`CLUSTINE_CROWD_ROUNDS`).
- **Recorded** for every split and every merge, between the coordinator's line that
  it began and the line that it ended and for two seconds after: the longest any bot
  of the crowd waited for an acknowledgement, the longest either of the two waited,
  and how long the coordinator took by its own lines; and what a bot of the crowd
  waits when nothing happens. Printed as the least, the middle and the worst of
  each, as `moves.rs` prints its pauses.
- **Asserted**, in every run: nobody is disconnected; as many splits and merges end
  well as there were rounds; the ledgers' audits; no wait above 5 s.
- **Measured for the roadmap**, by whoever builds step C5.8, with `cargo test
  --release -p clustine --test crowds`, for `N` = 4, 20, 50, 100 and 200 at a view
  distance of 8, and for `N` = 100 with the crowd spread along one row of chunks (a
  hundred lanes, 25 chunks from end to end; those who go on a lane of that row's
  middle) in place of the square. The first series
  says how the pause grows with players, the last with land.

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
every step and every check as it is. Then:

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
as long and it is the pauses that are to be judged. A world directory from before is
made over when it is opened: what was built in it is there, and the log says `the
world was divided otherwise before`. Two clients, both in creative mode, where flying
is a double tap on the jump key. F3 shows the block and the chunk.

**The single process:**

```bash
cargo run --release -p clustine
```

1. Join with both clients. The log has `reshaping by itself` with `merge_distance=22
   split_distance=30`, and `a region was assigned region=0`.
2. One stays at the spawn point. The other flies east. **Expected**: nothing until
   it is past x = 496; within two seconds of that, `a split is begun by itself
   region=0 part=1` in the log, and both stand still for about a fifth of a second:
   the other player's figure stops for a moment, and a block broken just then comes
   a moment late.
3. It flies on, as far as you like. **Expected**: nothing, ever, and no line in the
   log. (Before this step it was split off again every ten chunks.)
4. It flies back. **Expected**: within two seconds of coming west of x = 368, `a
   merge is begun by the distances survivor=0 absorbed=1`, and both stand still, the
   one who flew a little longer. They are 350 blocks apart and cannot see each
   other.
5. Fly out again and back to between x = 368 and x = 496, and to and fro there.
   **Expected**: nothing; only crossing all of it, and not sooner than ten seconds
   after the last time, is a split or a merge.
6. Fly out past x = 496 together. **Expected**: split off together, once; nothing
   between the two of you whatever you do out there.
7. One of you leaves the game out there and joins again. **Expected**: it is at the
   spawn point; the other noticed nothing. Half a minute later the region it left
   holds nothing; the log's routing table still lists it until the home region is
   empty.
8. Build, break, stop the server with Ctrl-C, start it again. **Expected**: every
   block is there; you join at the spawn point.

**To see it sooner, on foot**: `cargo run --release -p clustine -- --view-distance
3`. The distances are then 12 and 20 chunks: the split comes past x = 336 and the
merge west of x = 208. You see three chunks far.

**A line between two regions that you can stand at** is not something the default
has: no player sees another region's land. To build at one and across one, as after
step C3:

```bash
cargo run --release -p clustine -- --pin 4 --reshape by-hand
```

The regions meet at block x = 64. Walk across, build on both sides and across;
nothing may show. The log says once that the world has pinned regions.

**The cluster of processes**, each in a terminal of its own, on a world directory
that no other server has open:

```bash
cargo build --release -p clustine
target/release/clustine worldstore --world world
target/release/clustine coordinator
target/release/clustine worker --name a --listen 127.0.0.1:25611
target/release/clustine worker --name b --listen 127.0.0.1:25612
target/release/clustine edge
```

9. Steps 1 to 8, with the coordinator's log for the lines. **One thing more is
   expected at step 2**: ten seconds after the split, `a region is moved to even
   regions out region=1`, and the one who flew stands still once more, for about a
   third of a second; the one at the spawn point does not.
10. After a split, `kill -9` the worker that runs region 1 (the routing table in the
    coordinator's log says which). **Expected**: the one out there stands still for
    five to seven seconds and goes on; the one at the spawn point notices nothing.
    Fly on at once, and build: nothing built may be missing.
11. Kill the coordinator and fly out past x = 496. **Expected**: nothing is split;
    start it again, and within about a quarter of a minute it is: the new coordinator
    waits for its lease and lets every region rest once.
12. `target/release/clustine split --region 0 --chunks 6,0` with one of you at x =
    100, z = 8, the other at the spawn point. **Expected**: done, and undone ten to
    twenty seconds later by a merge, as the two of you are within 22 chunks: what is
    asked by hand lasts only where the distances agree.

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
3. **`b` walks on.** Nothing, however far. Region 1 is granted what comes into view
   and gives back what `b` left half a minute ago. This is where a group on stripes
   was split off again every ten chunks.
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
  inside one process, and the tasks of three services.
- A coordinator can do nothing until it has read the store's list once.
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
- `Coordinator::knowing` exists for the tests alone.
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
   was made)"**: or none, for `without_grace`.
7. **Section 7, "when the coordinator decides nothing by itself, the list is read on
   events only"**: and at every tick of the service until it has been read once,
   however the coordinator decides.
8. **K15** is gone where there are no pins, and stays where there are (N14 here).
9. **K6 and section 2.4, "the coordinator knows the regions of the layout from the
   start"**: it knows none.
10. **"Risks", "the tests of C4 run on stripes"**: `wanders.rs` (section 9.6 here).

## Changes to ADR-0010, ADR-0011, ADR-0013 and ADR-0014

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

## Open questions

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
   work for step C6.
4. **Whether `follows.rs` should move to a world without pins**, now that
   `wanders.rs` tests the default there. As it is, the two test deciding by itself
   in two worlds, one of which nobody runs.
5. **Whether the store's list should name the home chunk** (section 2.5), when the
   spawn point stops being a constant.
6. **Whether the single process needs its lease.** Its one worker cannot die without
   the coordinator. The lease is kept because the service has one; a process that is
   stopped for longer than five seconds and goes on finds its worker forgotten and
   registered again, with the regions it had.
7. **Whether `Server` should merge and split by hand**, for tests. `Asker` and
   `Mover` reach a coordinator in the process, and nothing uses that yet.
8. **ADR-0016's open question 2** (a part of many groups parted in halves) is as it
   was. N11 is where it would be felt first.

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
  worker's loop opening again a region that its orders name with another epoch. That
  path was read, not run, for the one worker being given its own region back.
- **Seven hundred tests begin from a coordinator that knows regions and has read no
  list**, a state no process is in after this step. They test what follows from
  knowing regions; what follows from learning them is Q1 to Q8, which are few.
- **The service's tests count readings** and hold them back. One whose `lists` fails
  at first now sees a reading at every tick. How many do was not looked at.
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
- **A process that sleeps for longer than the lease** finds its worker forgotten
  when it wakes. Reasoned to come back by itself (open question 6); not tried.

## Not checked

- **Nothing was built and no test was run for this record.** `cargo fmt --all
  --check` was run at the end, to see that no code was touched.
- **Read by the author**, at `c43b21c`: `crates/clustine-region/src/lib.rs`;
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
  `settle_chunks`; `bin/clustine/tests/common/mod.rs`.
- **Read in excerpts by three assistants, whose reports were checked in a handful of
  places**: the region runner (`services/worker/src/lib.rs`), the edge's fan-out
  (`services/edge/src/fanout.rs`), `region/reshape.rs`, every file of
  `bin/clustine/tests` and `common/processes.rs`, `tools/botswarm`, `deploy/`,
  `tools/check.sh` and the workflows. From them: what a join waits for, what a
  region does with a subscription to a chunk it does not hold, that a part is an
  ordinary region, what becomes of land after a merge, how the bots are steered,
  what each test file passes to which process and how far its bots walk, and the
  steps of the kind test.
- **Not read**: the tests of the coordinator and of its service, so how many of
  them compare a tick's `Changes`, count readings or name a routing table's layout
  is not known; `policy.rs` but for the distances; `state/follow.rs` before
  `note_listed`; `client.rs` but for its outline; the store's `tcp.rs`; the format
  of the table file, which is ADR-0011's word.
- **That `knowing` keeps the coordinator's tests** with no change but to their ten
  helpers was not tried.
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
