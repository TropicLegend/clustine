# ADR-0016: When to merge and when to split

- Status: **Proposed**; the design of step C4 of milestone M3, phase C: the coordinator
  decides by itself. Not reviewed yet and not built. It changes no code of the
  simulation, the region runner, the world store or the edge.
- Date: 2026-10-09

## Context

Regions merge and split when somebody asks ([ADR-0014](0014-merging-and-splitting.md),
[ADR-0015](0015-the-edge-through-merges-and-splits.md)): `clustine merge`, `clustine
split`. [ADR-0010](0010-regions-that-follow-players.md), section 7, says in half a page
when the coordinator is to ask by itself. This record says how, exactly enough to build
it and to write tests from.

What the code is today, as far as this record builds on it or changes it. Each of these
was read in the code at commit `11764c3`; "Not checked" at the end says what was not.

- **A message for where players are exists and nobody uses it.**
  `ToCoordinator::Players { regions: Vec<(RegionId, Crowds)> }` is step C0's. No worker
  says it, `Service::heard` closes the connection of whoever does (it falls into the
  last arm of the match), and `wire.rs` has one in a round trip.
- **A runner shows where its players are.** `RegionStatus::crowds()` gives the chunks
  with players in them, each with how many, and `RegionStatus::tick` the last tick.
  `RegionRunner::show_status` fills both when a runner is made (`with_store`), after
  every tick, and in `begin_anew`, which a merge and a split run **before** they call
  the outcome. So from the moment a worker process is handed the outcome of a merge or
  a split, the status is that of the region after it. `Region::crowds` counts every
  player by the chunk they stand in, whether the region holds that chunk or has only
  asked for it.
- **The worker process** (`worker` in `bin/clustine/src/cluster.rs`) looks at its
  regions every 250 ms (`LOOK`): at `status.tick` and `status.store_lost` of those in
  `Phase::Running`. It reads no crowds. Heartbeats are sent every second
  (`HEARTBEAT_INTERVAL`) by the task of `WorkerClient`, with what a watch holds at
  that moment. The outcome of a merge or a split goes another way: the worker's loop
  puts it into a queue (`endings`), `stay_registered` takes it out and calls
  `WorkerClient::absorb_ended` or `split_ended`, which queue it for the connection.
  When a release has ended or a region has lost the store, that loop waits for the
  runner's thread inline, and looks at nothing meanwhile.
- **The coordinator's state machine** (`services/coordinator/src/state.rs`):
  `Coordinator::tick` is `settle`, then `even_out`, then asking for the list if a
  split is `owed` a reading or a merge has had its worker's word. `merge` and `split`
  refuse with a `ReshapeRefusal` or note the reservation and say what workers are to be
  told; each calls `seen` first and `finish` last, as every public call does. A merge
  and a split have one lease. `reshape_ended` is called for the regions of every merge
  and split that ends, however, and notes the time in `reshaped`. `even_out` begins
  nothing while a release, a merge or a split is under way, nor within a lease of
  `reshaped`, and moves the region with the highest id of the worker with the most.
  `listed` adds a living region it does not know **unless a split is reserved**.
  `split` names `self.next`, the next id of the list as it was last read.
- **The service** (`service.rs`) calls `tick` every quarter of the lease, at least 50 ms
  apart (`tick_interval`), and reads the list on events only. A reading asked for while
  another is under way has that one thrown away and made again, so whatever asks at
  every tick must wait for its answer first; `Coordinator::reading` is how `tick` does.
- **A split takes who stands in the chunks named.** `Region::split`
  (`crates/clustine-sim/src/region/reshape.rs`): the seeds are the chunks named that
  the region holds (`Knowledge::Held`), that are not the home chunk and in which a
  player stands; who stands in a seed goes and everybody else stays; a chunk goes if it
  is nearer to a seed than to every chunk a stayer stands in and than the home chunk,
  if the region holds that. No seed is `NoSplit::Nobody`. The runner works a split out
  in `commit`, **after** the region has stopped ticking, and on `Nobody` ticks on with
  its links (`tick_on`): a split that finds nobody costs the region's players a stop of
  a few ticks and no resume.
- **What a player sees.** `view_area` in `services/edge/src/fanout.rs` sends a client
  with view distance `V` every chunk up to `V + 1` away along an axis. The edge grants
  at most `--view-distance`, 8 unless told otherwise, 32 at most.
- **Who holds a chunk** (ADR-0011, section 1): the region it is granted to, else the
  region pinned to an area that contains it, else nobody. A pinned region never gives
  a chunk of its areas back (`Land::keeps`). Until step C5 every world is stripes, and
  the stripes cover it: **no chunk is nobody's**, and a region that was split off can
  be granted nothing it was not split off with.
- **What was measured** (roadmap, "Where M3 stands"; optimised, in the middle / at
  worst): a merge stands the survivor's players still for 0.19 / 0.22 s and the
  absorbed region's for 0.37 / 0.41 s; a split those who stay for 0.14 / 0.20 s and
  those who go for 0.16 / 0.17 s; a move 0.27 / 0.31 s. Unoptimised, about three times
  that.

## Decision

Words in `code` are names in the code, or will be. `D_m` is the merge distance and
`D_s` the split distance, in chunks. "By itself" is what the coordinator does unasked;
"by hand" is `clustine merge`, `split` and `move`.

### 1. The shape of it

1. **Workers say where their players are four times a second**, in a message of its
   own that travels behind the outcomes of merges and splits, not in the heartbeat.
   The coordinator keeps the last word of each region, its **sighting**, and knows how
   old it is.
2. **What is wanted is a pure function of the sightings**: the occupied chunks of all
   regions are joined into clusters, players of one region holding together up to
   `D_s` and players of different regions up to `D_m`. A region whose players are in
   several clusters is to be split; two regions with players within `D_m` of each
   other are to be merged, unless the players in question are about to be split off.
   The chunk players enter the world in counts as a place where the home region has a
   player, always.
3. **Nothing is begun on one look.** What is wanted has to stand for more than a
   second, by which time every report the first look went by has been replaced by a
   newer one.
4. **Every region is left alone for a rest**, ten seconds, after a merge, a split or
   a change of owner, by merges, by splits and by evening out alike. That is the bound
   on how often anybody stands still.
5. **Ignorance holds back and begins nothing.** Only a region whose sighting is fresh
   is merged or split. A sighting that is no longer fresh still counts where it can
   only hold a split back.
6. **An empty region is absorbed at nobody's expense**: by a region that has no
   players either.
7. **The paths of ADR-0014 are called as they are**, with nobody as asker. The
   coordinator can be started so that it does none of this, and in step C4 that is
   how it starts unless told otherwise.

### 2. What the coordinator hears and keeps

#### 2.1 The message

```rust
/// Where the players of one region are, as the worker that runs it says.
pub struct PlayersOf {
    pub region: RegionId,
    /// The epoch the worker runs the region with.
    pub epoch: u64,
    /// The region's last tick, which `crowds` is of.
    pub tick: u64,
    /// The chunks with players in them, each with how many, ascending; empty if the
    /// region has no player.
    pub crowds: Crowds,
}

ToCoordinator::Players { regions: Vec<PlayersOf> }
```

One message has **every region the worker runs** (`Phase::Running`), also those
without players, and nothing of a region it is opening, starting from a split or
releasing. It is the whole of what the worker knows each time, never a difference, so
a report that is lost or passed over costs a quarter of a second and nothing else.

#### 2.2 The worker process

At every `LOOK` (the 250 ms it has), when it has noted the ticks, the worker's loop
reads `status.tick` and `status.crowds()` of each running region and queues one
`Players`. Two rules:

- **It goes down the queue the outcomes go down** (`endings`, whose `Outcome` gains a
  case for it), and `stay_registered` passes it on with a new
  `WorkerClient::players(Vec<PlayersOf>)`, which queues it as `absorb_ended` and
  `split_ended` do. Not a channel of its own and not the heartbeat's watch. Then the
  order on the connection is the order in which the loop made them, and since the
  status is of after a merge or a split from before the loop hears the outcome:
  **a report that follows the word `AbsorbEnded` or `SplitEnded` on the connection was
  read after that merge or split.** With two queues `stay_registered` would take from
  them in either order, and a report read before a split could follow the word of it.
- **None is queued while the worker has no connection to the coordinator**
  (`stay_registered` says so in a flag the loop reads). The queue has no bound, and
  four reports a second for as long as a coordinator is away would fill it.

A region that stands still (it waits for the store, or its runner has ended and the
loop has not yet seen it) is reported with the tick it stopped at.

#### 2.3 What is taken, and what is kept

`Coordinator::players(now, name, regions) -> bool` returns whether the worker is
registered; the service closes the connection of one that is not, as for a heartbeat.
To say it is to be heard from. **A coordinator that decides nothing by itself keeps
nothing of it.** Otherwise each entry is taken or passed over:

- passed over unless `name` owns the region with that epoch (`holds`);
- passed over while the region is part of a merge or a split under way;
- passed over if the region's sighting is of this owner and epoch and has a tick at or
  above the entry's: a region that says nothing new is not heard anew.

A **sighting** is what was taken last: the owner and epoch it was of, the tick, the
crowds (kept by chunk, in order, whatever order they came in; a count of 0 is left
out), and `taken`, the time of the call, or nothing once something has happened to
the region since (below).

A sighting is **fresh** at `now` if it is of the region's present owner and epoch, its
`taken` is no more than `FRESH`, one second, before `now`, and the region is not part
of a merge or a split under way. So a sighting stops being fresh, without anything
being said, when its region stands still, when its worker is silent or cut off, while
the region is opened, restored or released, and when the region changes hands.

Beside the sighting, per region: `empty_since`, the time of the first of the unbroken
run of taken reports without players; `alone_until`, before which the coordinator
begins nothing with the region by itself; two counters of attempts that failed
(section 5.5); and whether it was split last (section 5.3).

#### 2.4 What is forgotten, and when

| When | The sighting | The rest |
|---|---|---|
| A merge or a split of the region begins | stays, `taken` is nothing: not fresh until a report is taken after the end | `empty_since` forgotten |
| A reading of the list shows the region absorbed, by a merge that was noted or not | its crowds are added to the sighting of the living region it went into, by the pairs of that reading, if the coordinator knows that one; a sighting is made for it if it has none, of nobody and not fresh | all of it goes |
| A split ends with the worker's word `Ok(N)` for the split that was noted | the crowds in the chunks that split named go from the region's sighting to a sighting of `N`, of `N`'s owner and epoch, tick 0, not fresh | `N` begins like any region that is given an owner |
| A merge or a split ends otherwise | stays as it is | section 5.5 |
| The region is given an owner, or loses it, or its epoch changes | stays; it is of another owner, so not fresh | `empty_since` forgotten; `alone_until` at least a rest from now if it was given one |
| A reading of the list takes the region away otherwise | goes | goes |

The rule behind the table: **the players a region last named are still somewhere
near there**, in that region or in the one that took them, until somebody says
otherwise. A sighting is only ever replaced by a newer word of the same region or
moved with its players; it is never dropped for being old.

#### 2.5 What it costs, and how old it is

A report is about a dozen bytes for each region and some five to eight for each chunk
with players in it: with a thousand players, each in a chunk of their own, about 8 kB
four times a second over all workers together (reckoned from the encoding, not
measured). The coordinator compares occupied chunks pair by pair at every look: half
a million comparisons for a thousand occupied chunks, four times a second. That is
nothing; beyond some thousands of occupied chunks a grid of cells `D_s` wide has to
take the place of the pairs, which changes no result.

The age of a sighting is known from `taken`, the coordinator's own clock at the call.
What it cannot know is how long the report was on its way; section 9, K11, says what
rests on that being short.

### 3. The distances, and every other number

**Distance** is counted in chunks along the longer of the two axes, in 64 bits, as
`Region::split` counts it.

| Name | Value | What it is |
|---|---|---|
| `V` | 8 | `clustine coordinator --view-distance`: the largest view distance the edges grant |
| reach | `V + 1` = 9 | how far along an axis a player is sent chunks |
| `D_m` | `2 * (V + 1) + 4` = 22 | regions with players this near or nearer are merged |
| `D_s` | `D_m + 8` = 30 | players of one region further apart than this are split |
| margin | `min(3, (D_s - 1) / 2)` = 3 | how far around a group's chunks a split names chunks |
| `LOOK` | 250 ms | how often a worker reports, and how often `tick` is to be called |
| `FRESH` | 1 s | how old a sighting may be; and what is wanted must stand for longer |
| rest | 10 s | `--rest-seconds`: how long a region is left alone |
| `EMPTY_FOR` | 3 rests = 30 s | how long a region is without players before it is absorbed |
| `LONG` | 3 rests = 30 s | how long a region is left alone after an attempt that failed, doubling to 8 times |
| `LIST_EVERY` | one lease | how often the list is read |
| `AT_ONCE` | 4 | merges and splits under way at one time |

**Why these distances.** Two players whose views do not touch are `2 * reach + 1`
chunks apart or more. Regions are merged before that, so that a boundary between
regions is, as a rule, in nobody's view: nobody is a guest at a neighbour's chunk,
nothing a player does to a block crosses a boundary, and a player is handed over only
where nobody watches. That is ADR-0010's "twice the reach". The 4 on top is what two
players cover towards each other between the tick in which they come within `D_m` and
the end of the merge: up to a quarter of a second until the report, a second and a
look until it has stood, and the merge; two seconds, in which two players in creative
flight (10.9 blocks a second each, which is assumed to be the fastest; sprinting while
flying is twice that, and a rocket is out of scope) close 2.7 chunks. Faster players
see across a boundary for a moment, which works as it has since M2. **Nothing but
this quality rests on the speed or on the view distance being told right.**

`D_s` is 8 chunks more, 128 blocks. For a split to be undone by a merge the players
have to come 8 chunks nearer, which is half a minute on foot for one of them and six
seconds at a sprint in the air; and the same ages that the 4 above allows for cut
into the band from both sides and leave it open. After a split the chunks within
`(D_s + 1 - 2 * margin) / 2` = 12 of anybody stay with their region, if nobody has
moved more than the margin since the report (section 4.3), which is more than the
reach; and within `(D_s + 1 - margin) / 2` = `V + 6` of the home chunk they stay with
the home region. That answers open question 7 of ADR-0014 for these distances: a
player who joins is shown the chunks around the spawn point by the home region.

`--merge-distance` and `--split-distance` set the two outright, for tests, whose bots
walk a chunk in four seconds. They are refused unless `1 <= D_m`, `D_m + 2 <= D_s` and
`3 <= D_s`; with small distances nothing above about views holds, and nothing breaks.

### 4. What is wanted

A pure function in a module of its own, `services/coordinator/src/policy.rs`: no clock,
no map that is not ordered, and no state. It is given the distances, the chunk players
enter in (`ChunkPos::containing` of `CoordinatorConfig::spawn`), the home region of the
last reading of the list, and for every region the coordinator knows that has a
sighting: its id, its crowds and whether the sighting is fresh. It knows nothing of
owners, reservations or time.

#### 4.1 Places, links and clusters

- A **place** is a region and a chunk in which that region's sighting has players. The
  home region has one place more, whatever its sighting says and whether or not it has
  one: **the chunk players enter in**, with no players.
- Two places are **linked** if they are of one region and at most `D_s` apart, or of
  two regions and at most `D_m` apart.
- **The clusters by all it has heard** are the connected sets of all places. **The
  clusters by what it knows** are the connected sets of the places of regions whose
  sighting is fresh, and of the home region's extra place.
- A region is **surely whole** if its sighting is fresh and its places are in one
  cluster by what it knows, or it has none.
- A region is **surely apart** if its sighting is fresh and its places are in two or
  more clusters by all it has heard.

Connected sets do not depend on the order in which regions, chunks or players are
listed, and neither does anything below: where a choice is made, it is by a count or
by the order of chunks (`ChunkPos`'s own, ascending) or of region ids.

No region is both: links by what it knows are links by all it has heard. A region can
be neither: fresh, apart by what is known for sure, and joined only through places of
a sighting that is not fresh. **Nothing is wanted of such a region**, and that is the
whole of what ignorance does here: what cannot be known holds a split back (the players
that may still be there join the groups) and a merge too (which of its players would
stay is not known).

#### 4.2 Merges

A place of a region **counts for a merge** if the region is surely whole, or if it is
surely apart and the place is of the group that stays (section 4.3). No place of a
region that is neither counts, and none of a region whose sighting is not fresh.

**A merge is wanted of two regions** that have a place of the one that counts at most
`D_m` from a place of the other that counts. Their **gap** is the least such
distance. The home region's extra place counts like any other: a region with a player
within `D_m` of the chunk players enter in is merged into the home region, although
nobody may be there.

So a region that is to be split still takes in whoever comes near the players that
stay, and nobody is merged with players that are about to go: their part is merged
when it is a region (K2).

**Which survives**: the home region if it is one of the two; else the one with more
players by the two sightings; else the one with the lower id. The home region is never
named as the one to absorb, and nothing is wanted at all before the list has said which
region is home (section 5.1). The choice is made when the merge is begun, from the
sightings of that moment.

Merges come out in ascending order of gap, then of the lower id, then of the higher.

#### 4.3 Splits

**A split is wanted of a region that is surely apart.** Its places fall into **groups**
by the cluster by all it has heard that they are in. Any two groups are more than
`D_s` apart, as they would be linked otherwise.

- **The group that stays**: in the home region, the one with the chunk players enter
  in; in any other region, the one with the most players, and of several such the one
  that has the region's lowest chunk among them.
- **The group that goes**: of the others, the one with the most players, and of
  several such the one with the lowest chunk. **One split takes one group.** The next
  is wanted when the region has rested.
- **The chunks named**: every chunk at most the margin away from a chunk of the group
  that goes, ascending, each once. That is 49 chunks around each, less where they
  overlap.

The margin is for the time between the report and the tick of the split: the look,
the second it has to stand, and the stop of the region, in which a player in flight
covers two chunks. `Region::split` takes who stands in a chunk named **that the region
holds**, so naming more than is held or occupied costs nothing. The margin is less
than half of `D_s`, so nobody who stays can be standing in a chunk named unless they
have come most of the way over since the report. This answers open question 6 of
ADR-0014.

What the margin does not catch: a player of the group who was faster stays, in a
region whose other players are far away, standing on a chunk that stays with them
among chunks that went. If they walk on, they step into the new region's chunks and
are handed over, which costs nothing; if they stand, they are a group to split off
when the region has rested, and that region is merged with the first part in turn.
And a player who stands in a chunk the region has asked for and not been granted is
no seed: with nobody else in the chunks named, the answer is `Off::Nobody`, "not yet"
(section 5.5).

Splits come out in ascending order of the region's id.

#### 4.4 Empty regions

This part needs the time and the owners, so it is the state machine's (section 5.3):
a region other than the home region whose sighting is fresh and has no players, and
has had none for `EMPTY_FOR`, **is absorbed by a region without players**: the home
region if its sighting is fresh and has none; else the region with the lowest id that
is not the home region, has a lower id than the empty one and a fresh sighting without
players. If there is no such region, it stays. The survivor need not have been
without players for any time.

So of the regions without players one is left, the lowest, while the home region has
players, and none once it has none. **Nobody stands still for an empty region**: the
survivor has no players, and the absorbed region has none unless one came at that
very moment (K13). ADR-0010 has an empty region absorbed by the region whose chunks
are nearest, or by the home region; that would stand the players of that region still,
at every departure of somebody elsewhere, for chunks that are given back half a minute
later anyway.

#### 4.5 A crowd, and room for a later rule

A hundred players in one place are one cluster, one region and one thread, and so is a
row of players each within `D_m` of the next, however long: ADR-0010's limit, which
this record keeps. Nothing here looks at load. A merge of two crowds whose state
together is too large to hand to the store is off (`Off::TooLarge`), and is tried
again ever more rarely (section 5.5).

A later rule has what it needs without anything new in a worker: `Region::split`
takes whoever stands in the chunks named, so a crowd can be cut along a line by naming
the chunks on one side of it. What it would have to add is here, in this function: a
further reason to want a split, and the memory that two regions were parted on
purpose, without which the merge rule would join them again at once. `decide` returns
what is wanted with why (`Near`, `Apart`), so that a further reason is a further case.

### 5. What is begun, and when

`Coordinator::tick`, when the coordinator decides by itself and the list has told it
which region is home, does between `settle` and `even_out`: works out what is wanted
(`decide`), notes since when each thing has been wanted without a break, in an ordered
map, and begins what may be begun. With `follow: None` it does none of this, and
nothing of sections 6 and 7. Whoever drives the coordinator calls `tick` at least
every `LOOK` then (`Coordinator::LOOK`); the service's `tick_interval` becomes the
shorter of that and a quarter of the lease. Nothing is decided in any other call.

#### 5.1 What has to hold of the world

Nothing is begun by itself unless all of these hold:

- the coordinator's grace period is over (one lease from when it was made);
- **the list has been read, and the last reading that succeeded is no more than two
  `LIST_EVERY` old**: without the list the coordinator does not know which region is
  home, what the next region id is, or what became of a merge;
- fewer than `AT_ONCE` merges and splits are under way, whoever asked for them;
- an epoch is left to issue.

#### 5.2 What has to hold of a region: free

A region is **free** if it has an owner that has a connection, is not leaving and is
not at fault (`Coordinator::at_fault`); it is not part of a merge or a split under
way; it is not being released; and `now` is not before its `alone_until`.

These are the reasons for which `merge` and `split` would refuse, and three more:
neither region's owner may be leaving (a leaver's regions are being moved away, and
the worker has twenty seconds), none may be at fault (a worker that just failed a
region is not given the work of a merge), and the rest. So a refusal is not expected.
If `merge` or `split` refuses all the same, it is logged as a fault of this code and
the regions are left alone for a rest.

#### 5.3 Standing, and the order

What is wanted is kept by what it is of: a merge by its two regions, a split by its
region. At every tick, each thing wanted that was also wanted at the tick before keeps
the time it was first wanted in this run; a new one gets `now`; one that is no longer
wanted is forgotten. A thing wanted **has stood** when that time is more than `FRESH`
before `now`.

Why: a report can be true and still mislead for as long as another region's report is
older than it. A player who is handed from one region to another is, for one report,
in both sightings or in neither (K10). Every fresh sighting a look goes by was taken
within `FRESH`, and one that is not fresh can only hold back; so when a thing has been
wanted for longer than that, every report the first look began it on has been
replaced, and it is still wanted. Whether a region rests does not come into what is
wanted, so a thing can stand while its regions rest, and is begun when they have.

Then, in this order, each only while fewer than `AT_ONCE` are under way:

1. **One split at most**: of the splits that have stood whose region is free and not
   passed over for its turn (below), the one of the lowest region. Only if **no split
   is under way** and no reading of the list is asked for or owed (`reading`,
   `owed`): the next region id is then the one the store will give. `split(now,
   region, &named, None)`.
2. **The merges that have stood**, in their order (section 4.2), each if both its
   regions are free and neither is in something begun at this tick.
   `merge(now, survivor, absorbed, None)`.
3. **The empty regions** (section 4.4), highest id first, each if it and its survivor
   are free and neither is in something begun at this tick.

Splits come first because they are the scarcer. Two regions that each want to merge
with a third are served nearest first, and the other when the survivor has rested.

**Turns.** A region is passed over in step 1 if the last merge or split of it that
ended well was a split of it, and a merge of it has stood whose two regions are free.
Without this a region that groups keep leaving, which is the home region of a busy
world, would be split every time it has rested and never take in those who arrive
(K21). Per region that is one bit: whether it was split last.

**One split at a time in the whole world**, because a split has to name the id of the
region it makes. The runner tries once more with the store's next id if the one named
is taken (`Decline::NotNext`), which is enough for one split that raced another and
not for three at once. The coordinator reads the list when a split ends, so the next
split names an id read after it.

#### 5.4 Rest

`alone_until` of a region is set to a rest from `now`, if that is later than what it
has,

- when the region is given an owner: assigned, handed over after a release, taken on
  a worker's word at a registration or after a split;
- when a merge it survived or a split of it ends well.

Nothing the coordinator begins by itself touches a region before that: no merge, no
split, no release to even out. What somebody asks for by hand is not held back by it,
and what follows a death (a takeover) is not either; both end with the region being
given an owner or a merge or a split ending, and so with a rest.

#### 5.5 What comes of it

The coordinator notes how each merge and split ends where it puts the `Reshaped` into
`Changes` (`end_merge`, `split_ended`, `lapse_split`), for those somebody asked for by
hand as for its own.

**A merge that ends well**: the survivor rests, and its counters are 0.

**A merge that comes to nothing, for whatever reason**: each of its two regions that
is still there is left alone for `LONG` times 2 to the power of the failures it has
had in a row before this one, 8 times `LONG` at most, and has one failure more. One
rule for all of `Undone`, because by the time a merge can fail the absorbed region has
been released as a rule: its players have stood still as for a move, for nothing, and
whatever went wrong is likely to go wrong again.

| `Undone` | What happened | Why not sooner again |
|---|---|---|
| `Off(NotRunning)`, `Off(Busy)` | the survivor's worker does not run it with that epoch, or it is in the middle of something | orders crossed; the absorbed region was moved for it |
| `Off(TooLarge)`, `Off(Declined(TooLarge))` | the two states together are too large | it will be so again; the doubling is for this |
| `Off(Declined(Uncheckpointed, Tick, NoSuchRegion, Home, NotOpened))` | the store declined | the region was given away meanwhile, or the two do not agree |
| `Off(StoreLost)`, `Unread` | the store was lost on the way | the list says what happened; everybody opens anew |
| `Off(Unreadable)`, `Off(Refused)` | the region to absorb could not be opened | somebody else has it, or it is damaged |
| `NotReleased` | its owner did not release it in a lease | that owner is at fault as well |
| `Overdue`, `Contradicted` | no word, or a word the list contradicts | nobody knows why |
| `Disowned(r)`, `Gone(r)` | a region changed hands or went | a worker died; a quiet half minute is welcome |
| `NoEpoch` | no epoch is left | nothing is begun ever again (section 5.1) |

`Off(Nobody)`, `Off(NothingStays)`, `Off(Declined(NotHeld, Malformed, NotNext))` are
answers to a split and do not come of a merge.

**A split that ends well**: the region rests and its counters are 0; the part rests as
a region that is given an owner.

**A split that was "not yet"**: `Off(Nobody)`, `Off(NothingStays)`, `Off(Busy)`,
`Off(NotRunning)` and `Off(Declined(NotNext))`. The players had moved on, stood in a
chunk not granted yet, or had left; those who were to stay had left; the region was
in the middle of something; or another split took the id. **The region rests**, as
after a split that was made: such a split has stopped the region for a few ticks,
and nobody waits for a group to be split off. The next attempt names chunks from the
report of that moment. **The third such answer in a row is a failure** like those
below, and the count of such answers begins anew. This is what ADR-0014's tests under
bots asked of this step (a split within a tick or two of a merge finds nobody). It
cannot come of a merge the coordinator made, as the survivor rests ten seconds; it can
of a player who stands at the rim of what the region was granted.

**A split that comes to nothing otherwise** (`TooLarge`, any other `Declined`,
`StoreLost`, `Overdue`, `Disowned`, `Gone`): as a merge that comes to nothing, for its
one region.

The counters go back to 0 only when a merge or a split of the region ends well; a
region that is given an owner keeps them, as what failed need not have been the
owner's doing.

### 6. Evening out, moves by hand and workers that leave

**Evening out, when the coordinator decides by itself**, is as `even_out` has it but
for two things:

- **"Nor within a lease of a merge or a split having ended" goes.** Where regions
  merge and split all the time that would never let anything be evened out, and parts
  stay on the worker that made them. In its place: a region is not released to even
  out before its `alone_until`. That is what ADR-0014 wanted of the lease, that a part
  is not moved in the same breath, for each region by itself.
- **Which region**: of the regions of the worker with the most that are not at rest,
  the one with the fewest players by its sighting (fresh or not; a region without a
  sighting counts as having more than any), and of several such the one with the
  highest id. If all its regions rest, nothing is evened out at that tick. A move
  stands its region's players still, so it is the region with the fewest that moves;
  the one empty region that is left over (section 4.4) moves first and costs nobody.

It still begins nothing while a release, a merge or a split is under way, and it runs
after what section 5 begins in the same tick: a region that is wanted for a merge and
free is reserved before it could be picked. **When the coordinator decides nothing by
itself, `even_out` is exactly as it is**, and the seven tests that say so stand.

A merge that is wanted while one of its regions is being moved waits for the move and
then for the rest of the region's new owner (K3).

**A move by hand** is refused for a region of a merge or a split under way, as today,
and not for one at rest. **A worker that leaves**: none of its regions takes part in
anything begun by itself from the moment it says so; those already reserved are
released when the reservation has ended, as ADR-0014 has it.

### 7. The list, on a timer

When the coordinator decides by itself, `tick` asks for the list (`Changes::read`)
whenever no reading is asked for (`reading`) and the last answer, `listed` or
`unlisted`, is `LIST_EVERY` old or there has been none. It asks through the same
`ask_for_the_list` as everything else, so it waits for the answer before it asks
again; a timer that asked at every tick would have every reading thrown away for the
next (see the context).

**What a reading changes** is what `listed` does today and nothing more: a living
region the coordinator does not know is added without an owner and assigned, unless a
split is reserved; a region that was absorbed or is no more is removed; the home
region, the absorbed pairs and the next id are noted. The bounds of a region
(`RegionInfo::bounds`) and its pinned areas are not kept: nothing here goes by where a
region's chunks are. Besides, it notes when it last succeeded (section 5.1).

**A region the list shows and no worker reports** is a region nobody runs. It is
assigned like any such region, and takes part in nothing until its owner has reported
its players and it has rested. It holds nothing else back: what is wanted of other
regions goes on, by what is known of them. If it was known before and has a sighting,
its players still count where they are (section 2.4).

**When the coordinator decides nothing by itself, the list is read on events only**,
as in step C3. The service's tests hold readings back and count them, and nothing
that is asked by hand needs a timer.

### 8. Starting it

```text
clustine coordinator ... [--reshape by-hand|by-itself] [--view-distance V]
                         [--merge-distance N] [--split-distance N] [--rest-seconds N]
```

- `--reshape` is `by-hand` unless told otherwise, **in step C4**: the coordinator
  decides nothing, keeps nothing of `Players`, reads the list on events, evens out as
  it did, and every test there is runs as it does. Step C5 makes `by-itself` the
  default, when the stripes go. Why not now: every world of C4 is stripes, and on
  stripes a group that walks on falls back into the region it was split off (section
  9, K15); and the tests of hand-over, chaos and moves want their two or three regions
  to stay two or three.
- `--view-distance` is what the edges are started with (2 to 32, 8 if not said).
  Nothing checks that the two agree. It is used for `D_m` and `D_s` and nothing else.
- `--merge-distance` and `--split-distance` take the place of what follows from the
  view distance; `--rest-seconds` (1 or more, 10 if not said) is the rest, and
  `EMPTY_FOR` and `LONG` go with it.

They become `CoordinatorConfig::follow: Option<Policy>`, with `None` for `by-hand`:

```rust
pub struct Policy {
    pub merge_distance: u32,
    pub split_distance: u32,
    pub rest: Duration,
}
impl Policy {
    pub fn for_view_distance(view_distance: u32) -> Self;   // 2V + 6, 2V + 14, 10 s
    pub fn checked(self) -> Result<Self, String>;           // section 3
}
```

`deploy/kubernetes/coordinator.yaml` is not changed in this step.

### 9. Every order of events that matters

`A`, `B`, `C` are regions; "the look" is a tick that decides.

**K1. Two regions each want to merge with a third.** `A` and `B` both have players
within `D_m` of `C`'s. Both merges stand. The one with the smaller gap is begun (of
equal gaps the one with the lower ids); the other is passed over at that tick because
`C` is in something begun, and at later ticks because `C` is reserved. When the merge
has ended the survivor rests ten seconds, reports, and the other merge, wanted all
the while of `B` and whichever region survived, is begun.

**K2. A region that should be split and merged at once.** Two cases, told apart by
the clusters. (a) `A` has a group at home and a group far off, and `C`'s players are
near the far group only: the far group and `C` are one cluster, the home group
another, so `A` is surely apart: it is split, no merge with `C` is wanted meanwhile
(the places near `C` are of the group that goes), and the part and `C` merge when the
part has rested. Three sets of players stand still
once, once and twice. Merging first would have made it twice each. (b) `C`'s players
are within `D_m` of both of `A`'s groups: everything is one cluster, `A` is whole, and
`A` and `C` merge. One merge; splitting first would have been three.

**K3. A merge is wanted while the part of a split is being evened out.** The part is
being released, so it is not free. The move ends, its new owner is given it, it rests
ten seconds and reports; then the merge is begun. If the release is not answered in a
lease, the region is taken and assigned as today, and rests from then.

**K4. Players who go back and forth across a distance.** Around `D_m`, in two
regions: one merge, the first time a report has them within it and it stands a second;
after that they are one region until they are more than `D_s` apart. Around `D_s`, in
one region: one split, and no merge until they are within `D_m`. Across the whole
band of 8 chunks and back: a merge and a split each time, each at least a rest after
the one before. That is the worst anybody can do to a region: one stop every ten
seconds.

**K5. A group that dissolves between the decision and the order.** A split: the
players have moved out of the chunks named, or have left: `Off(Nobody)`, "not yet".
Some have moved out: those still there go, and the others are the stragglers of
section 4.3. Those who were to stay have all left, and the region has neither the
home chunk nor a pinned area: `Off(NothingStays)`, "not yet"; otherwise the split is
made and the region is left without players. The group has walked back towards the
others: it is split all the same, and merged back only if it comes within `D_m`. A
merge: the absorbed region's players have walked away or left: it is merged all the
same, and split again, after the rest, if they are more than `D_s` off.

**K6. A coordinator that starts anew in the middle.** It knows no merge and no split,
has no sighting and begins nothing for a lease. It reads the list; workers register
and report what they run, and each such region rests ten seconds from then; their
players are reported within a quarter of a second. What the workers were in the
middle of ends as ADR-0014, section 5.5, has it: a released region waits out the grace
period and is assigned, which fences an absorb not yet made; a part is reported by
its worker. If the new coordinator orders a split that the old one had ordered and
the worker is still at, the answer is `Off(Busy)`, "not yet"; if the worker has done
it, the report shows the group gone and nothing is wanted.

**K7. A worker dies with the survivor.** ADR-0014, section 6, says what is found. For
this record: the merge ends as `Disowned` or by the list. If it was made, the survivor
is given an owner and rests. If not, both regions are given owners, rest, and are left
alone for `LONG`. The sightings stay meanwhile, so the players of both still hold back
splits of their neighbours, and nothing is begun with either before its new owner has
reported. The same for the absorbed region's worker dying while it releases, and for
the worker of a region that is being split.

**K8. The store away.** Regions wait for the store and stand still: their ticks do not
go up, their sightings stop being fresh within a second, and nothing is wanted of
them. A merge or a split under way ends `Off(StoreLost)` or with the reservation, and
is not tried for `LONG`. Readings of the list fail; after two `LIST_EVERY` nothing is
begun at all until one succeeds. If only the coordinator is cut off from the store,
workers go on as they are and it begins nothing.

**K9. A hundred regions.** One look compares the occupied chunks of all of them once
(section 2.5). At most four merges and splits are under way at a time and one split.
A hundred regions that all come near each other merge in pairs, then the fifty in
pairs, and are one after seven rounds of a rest each; a hundred that each come near
one and the same region, and not near each other, are taken one every ten seconds;
and a hundred groups that part from one region are split off one every ten seconds.
Each merge and split is a new routing table for every edge, with the absorbed pairs
the store keeps, 4096 at most. Regions are evened out one release at a time, as
today.

**K10. A report from before a hand-over beside one from after it.** A player walks
from `A` into a chunk `B` holds. For up to one report they are in both sightings
(`A`'s older, `B`'s newer) or in neither. In both: a place of `A` and a place of `B`
side by side, a merge wanted. It has not stood: `A`'s next report comes within the
second, or `A`'s sighting stops being fresh, and either ends the run. In neither: a
player who joined two groups of `A` is missing, a split wanted. It has not stood
either if `B` reports within the second and the player still joins the groups from
there, within `D_m` of both. What remains: if `B` is silent, the split stands and is
begun, and a merge puts it right when `B` is heard again; "Risks" has it. And a
player who stands on a boundary and is handed back and forth every few ticks can be
in both reports again and again, and the two regions are then merged; that is a merge
of two regions whose boundary somebody stands on, and no harm.

**K11. A report from before a merge or a split that arrives after it.** By section
2.2 it cannot follow the worker's word of the outcome, and before that word the region
is reserved and the report passed over. It can be taken where the reservation ended
without that word: when the list shows a merge done before the worker has said so, or
when a lease has passed. The sighting is then of before: a survivor without the
players it took in, or a split region with those that went. Nothing is begun on it,
because the region rests ten seconds and a report of after comes a quarter of a second
later. What this rests on is that a report is not ten seconds on its way; if one is,
a split region and its part can be merged back and split again, once.

**K12. A region between two groups is reserved, silent or without an owner.** Its
sighting stays and its places go on joining the groups, so the region around it is not
"surely apart" and is not split. It is not "surely whole" either if the join was all
that held it together, so it is not merged. When the region in between reports again,
or is absorbed (its crowds go to the survivor's sighting), or is split (the part's
crowds go to the part's), the same places are there under their new region.

**K13. Players come back to an empty region as it is absorbed.** A player walks into
a chunk it holds and is handed over to it, in the tick the coordinator begins the
merge or after. The region is released with the player in it, or with the player's
arrival kept by the edge; either way the survivor has them after the merge
(ADR-0014, sections 2.3 and 8.5). They stand still as the players of an absorbed
region do, once, and are then alone in a region that had nobody, of which the rules
make whatever the distances say. A player who joins while the home region absorbs an
empty region waits for that merge, a fifth of a second.

**K14. A stranger in another region's chunks.** A region keeps the chunks behind its
players for thirty seconds and those a guest watches for as long as they watch. A
player of another region who walks into one is handed over to it. If its own players
are within `D_s`, that is all. If they are further, the region is surely apart and is
split, which stands its players still once for somebody who only crossed their trail.
It takes a player who follows another's path at more than `D_s` and less than thirty
seconds behind, which on foot cannot be and in the air can.

**K15. A group that walks on, on stripes.** In a world of pinned regions a part holds
the chunks it was split off with and can be granted no other: all the rest is some
pinned region's (see the context). The group walks to the rim of its part, about ten
chunks, steps onto a chunk of the pinned region and is handed over to it, far from
that region's other players. It is split off again when the region has rested. **On
stripes, a group that keeps walking away is split off every ten chunks or every ten
seconds, whichever is longer, and everybody in the pinned region stands still each
time.** Nothing in this record can change that; it ends with the stripes. The parts
left behind are empty and are absorbed as section 4.4 has it.

**K16. A worker that leaves, and one at fault.** Their regions are not free: nothing
is begun with them. A leaver's regions are released as soon as there is somebody to
take them, as today, and take part again when their new owner has reported and they
have rested.

**K17. Somebody asks by hand while the coordinator decides by itself.** `merge`,
`split` and `move_region` answer as they do: refused for a region that is reserved or
being released, done otherwise, at rest or not. What comes of it is noted like the
coordinator's own (section 5.5), and whatever the distances say of the result is
wanted afterwards: a group split off by hand within `D_m` of the others is merged
back when both have rested.

**K18. The owner changes between the look and the order.** The order is lost with the
connection or answered `Off(NotRunning)`; the reservation ends as `Disowned` or
`Overdue`, and section 5.5 applies. A split's order is not given again (ADR-0014).

**K19. A reading of the list between a split's record and the worker's word** shows
the part. `listed` leaves it out while a split is reserved, as it does today; that
rule was written for this timer. A region the list shows then is that split's part
as a rule; one that is not is added by the next reading after the reservation, as
ADR-0014 has it.

**K20. A merge that the list shows done before the worker says so.** `listed` ends
the merge, the survivor rests, and the worker's `AbsorbEnded` finds no merge noted and
has the list read once more, as today. K11 is about the reports in between.

**K21. Groups leave a region and others arrive at it, faster than one in ten
seconds.** The region is surely apart nearly all the time. Those who arrive near the
players that stay are still wanted for a merge (section 4.2), and by the turns of
section 5.3 the region is split, rests, merges, rests, and so on: one group leaves
and one region arrives every twenty seconds, and its players stand still once in
ten. Regions that wait to be taken in merge among themselves meanwhile. Groups that
wait to be split off stay the region's players, further and further away, and stand
still with it; if more than three a minute leave, they queue (open question 2).

### 10. Changes to messages and types

**`clustine-rpc`**: `ToCoordinator::Players` and `PlayersOf` as in section 2.1, in
place of the pairs of step C0. It breaks the one literal in `wire.rs`.

**`services/coordinator`**

```rust
pub struct CoordinatorConfig { .., pub follow: Option<Policy> }   // None: by hand
pub struct Policy { .. }                                          // section 8

impl Coordinator {
    /// How often `tick` is to be called when the coordinator decides by itself.
    pub const LOOK: Duration = Duration::from_millis(250);
    pub fn players(&mut self, now: Instant, name: &str, regions: &[PlayersOf]) -> bool;
    /// The merges and splits under way, whoever asked for them.
    pub fn under_way(&self) -> Vec<Asked>;
}

impl WorkerClient { pub fn players(&self, regions: Vec<PlayersOf>); }

// policy.rs
pub struct Sighted<'a> { pub region: RegionId, pub fresh: bool, pub crowds: &'a Crowds }
pub enum Wanted {
    Merge { survivor: RegionId, absorbed: RegionId, gap: u32, why: Why },
    Split { region: RegionId, named: Vec<ChunkPos>, why: Why },
}
pub enum Why { Near, Apart }
pub fn decide(
    policy: &Policy,
    enter: ChunkPos,
    home: RegionId,
    regions: &[Sighted<'_>],
) -> Vec<Wanted>;   // the splits, then the merges, each in its order
```

- `CoordinatorConfig::follow` breaks the ten literals of the struct: three each in
  `state.rs`, `service.rs` and `services/coordinator/tests/reshape.rs`, one in
  `bin/clustine/src/cluster.rs`. All get `follow: None` but the last.
- `merge` and `split` are each cut in two: what checks and notes, without `seen` and
  `finish`, which `tick` calls; and the public call around it, which does what it
  did. `Split` (the note of one under way) keeps the chunks that were named. Nothing
  else of the two paths changes.
- `Service::heard` gains an arm for a worker's `Players`. `tick_interval` takes
  whether the coordinator decides by itself.
- `Changes`, `Reshaped`, `Undone`, `Order`, `FromCoordinator`, `Off`, `RegionList` and
  `RoutingTable` are as they are. What the coordinator begins by itself is in
  `Changes::releases` and `Changes::orders` like what is asked for, and how it ends in
  `Changes::reshaped` with nobody as asker, which the service logs.

**`bin/clustine`**: `CoordinatorArgs` gains `follow: Option<Policy>`; the flags of
section 8; `Outcome` in `cluster.rs` gains the report.

Nothing changes in `clustine-sim`, `services/worker`, `services/worldstore`,
`services/edge` or `clustine-region`. The three things ADR-0015, section 8, asks
steps C4 and C5 not to undo are untouched: a part holds the chunk each of its players
stands in, a stay does not leave a region without an input of its edge, and a merge
announces itself before anything the survivor says of a stay that came with it.

### 11. Building it

`main` is green at every commit. Steps 2 and 3 are the coordinator's alone
(`services/coordinator/src`), steps 4 and 5 the worker process's and the command
line's (`bin/clustine`), and the two pairs share no file once step 1 is pushed.

| # | Scope | Verified by |
|---|---|---|
| C4.1 | The contract: `PlayersOf` and `Players`; `Policy` and `follow`, `None` everywhere; `Coordinator::players`, which only counts as being heard from; the service's arm; `WorkerClient::players` | Round trip; a worker that says it is not cut off, an unknown one is; every existing test |
| C4.2 | `policy.rs`: `decide` | Its own tests, and D1 to D14 below |
| C4.3 | The state machine: sightings, standing, free, rest, what comes of it, empty regions, evening out, the timer, `under_way`; `tick_interval` | F1 to F40 below, by somebody else; every existing test of the coordinator unchanged with `follow: None` |
| C4.4 | The worker process reports | Its build; C4.7 |
| C4.5 | The flags; the test cluster (`tests/common/processes.rs`) can pass a coordinator more arguments | `clustine coordinator --help`; a refusal for distances that do not fit |
| C4.6 | The generated runs R1 to R7, by somebody else | They catch faults put into C4.3 on purpose: no rest, no standing, the list never read, a survivor that is not home |
| C4.7 | End to end, E1 to E8, by somebody else | No bot disconnected, the ledgers, the pauses counted |
| C4.8 | Roadmap, README, what to try with real clients | CI |

**`decide`, from this record alone** (distances 2 and 5 unless said; `H` is the home
region, the chunk players enter in is the origin):

- D1. Two regions, a player each, 2 apart along x and 0 along z: a merge. 3 apart:
  nothing. 2 along x and 2 along z: a merge. 3 along z and 0 along x: nothing.
- D2. Survivor: the home region with one player against a region with five; of two
  others the one with more players; of two with as many the lower id.
- D3. A region with a player 2 from the origin and the home region without a
  sighting: nothing (the home region is not fresh). The home region fresh and
  empty: a merge into it.
- D4. One region that is not home, far from the origin, players at x = 100 and
  x = 105: nothing. At x = 100 and x = 106: a split.
- D5. In the split of D4 the group with more players stays; with as many, the one
  with the lowest chunk; the chunks named are the square of the margin (2) around the
  other, ascending, each once.
- D6. In the home region the group at the origin stays although the other has more
  players; and a home region whose only players are 6 from the origin is split.
- D7. Three groups: the one that goes is the largest of the two that do not stay.
- D8. K2 (a) and K2 (b), each with what comes out; and a region that is apart with
  another region near its group that stays: the split and the merge are both wanted.
- D9. Two groups of `A` joined by a player of `C` whose sighting is not fresh:
  nothing, neither a split of `A` nor a merge of `A` with a fresh region near it.
- D10. A region that is not fresh is in no merge and no split, whatever it holds.
- D11. Any permutation of the regions given, and of the crowds within each, gives the
  same answer (generated).
- D12. The order: splits by region id; merges by gap, lower id, higher id.
- D13. Players at coordinates near `i32::MIN` and `i32::MAX`: no overflow.
- D14. A region without players is in nothing.

**The state machine, from this record alone** (a cluster as
`services/coordinator/tests/reshape.rs` builds one, with `follow`; "after it has
stood" is a tick more than `FRESH` after the first that wanted it):

- F1 to F5, hearing: a report from a worker that does not own the region, or with
  another epoch, changes nothing; one about a reserved region is passed over; one with
  the tick the sighting has does not make it fresh again, so a region that repeats a
  tick is in nothing after a second; an unknown worker gets `false`; with `follow:
  None` reports are heard and nothing ever follows.
- F6 to F9, merging: nothing at the first look, `Release` and `Prepare` after it has
  stood; a tick at which it is not wanted begins the wait anew; three regions in a
  row merge nearest first and the third after the rest (K1); with ten pairs wanted
  four are begun and the others as those end.
- F10, each thing that holds a merge back, one test each: reserved; being released;
  no owner; an owner without a connection; either owner leaving; either owner at
  fault; at rest; a sighting more than `FRESH` old; the grace period; the list never
  read; the last good reading two `LIST_EVERY` old; a reading that failed.
- F11 to F16, splitting: `SplitOff` after it has stood, with the chunks of D5 and the
  next id of the list; one split in the world at a time; none while a reading is
  asked for or owed; a region joined by a sighting that is not fresh is not split
  (K12), and is when that region reports its players elsewhere; a second group goes
  after the rest; a region that was split last and has a merge that stood is merged
  before it is split again, and one that was merged last is split first (K21).
- F17 to F24, what comes of it: after a merge the survivor is in nothing for ten
  seconds and then is; after a split both are; after `Off(Nobody)` the region rests
  and is then split with the chunks of the newest report, and the third in a row
  leaves it alone for `LONG`;
  each `Undone` of a merge leaves both alone for `LONG`, the next for twice that, and
  a merge that ends well makes it `LONG` again; a region given an owner rests; after
  `Ok(N)` the crowds in the chunks named count as `N`'s, after a merge the absorbed
  region's as the survivor's.
- F25 to F29, empty regions: absorbed by the home region if that has no players, by
  the lowest empty region otherwise, not before `EMPTY_FOR`, not if a report had a
  player in between, and left if it is the only one; the home region is never named
  to be absorbed.
- F30 to F33, evening out: with `follow`, a region at rest is not moved and another
  of that worker is, at once; the one with the fewest players goes first, one without
  a sighting last; nothing while a merge is under way; with `follow: None` as today.
- F34 to F37, the list: asked for every `LIST_EVERY` with `follow` and never by time
  without; not asked again while one is asked for; a region the list adds is assigned
  and is in nothing until it has reported and rested (K19 with a split reserved).
- F38 to F40: a new coordinator begins nothing for a lease and nothing with a region
  for ten seconds after its worker reported it (K6); a player in two sightings for
  one report merges nothing (K10); what is asked by hand is done at rest, and rests
  afterwards (K17).

**Generated runs, from this record alone.** A model in the test: players are points
that walk, each by a script or at random towards changing goals, a step of the run
being 250 ms and a player moving at most one chunk in two seconds, some joining at
the origin and some leaving; the distances are 6 and 12; regions are sets of
players; workers do what they are ordered after a delay of zero to four steps (a merge
puts the absorbed region's players into the survivor; a split takes who stands, by
the true positions of that moment, in the chunks named, or answers `Off(Nobody)`),
say what came of it, and report their regions' true crowds every step, behind the
word of an outcome; the store's list is what the model made. Variants add: reports
that lag a step; a player handed from one region to another with one stale report
(K10); workers that die and are replaced; readings that fail; a coordinator made
anew. Checked after every call of the coordinator:

- R1, rest: between the end of anything that involved a region (a merge, a split, a
  new owner) and the next merge, split or release to even out that the coordinator
  begins with it by itself, at least a rest has passed.
- R2, no player more often than the bound: without hand-overs and deaths, every
  player is in a region of a merge, a split or a release to even out that the
  coordinator began by itself at most `1 + W / rest` times in any time `W`.
- R3, nothing on what is not known: every region of anything begun had a sighting
  taken within `FRESH`; the home region is never the one to absorb; never more than
  `AT_ONCE` under way, never two splits.
- R4, no flapping: when a merge is begun, a player of the one region was truly within
  `D_m + 2` of a player of the other, or of the origin for the home region, at some
  step of the last two seconds; when a split is begun, the players truly in the
  chunks named were more than `D_s - 2` from everybody who stays (two players close
  two chunks in two seconds). And in every run in which players stand still from
  some step on, no split ever parts two players whom a merge put into one region
  after that step, and no merge puts into one region two players whom a split parted
  after it.
- R5, it ends: from the step at which every player stands still and no fault follows,
  if `n` merges and splits are wanted, then after `(n + 1) * (rest + D + 2 s)`, with
  `D` the longest delay of the model's workers, `EMPTY_FOR` more where a region is
  empty and a rest more for each region that is moved to even out: any two players
  within `D_m` of each other are in one region, and so is every player within `D_m`
  of the origin with the home region; the players of each region are joined by steps
  of at most `D_s`, through players of regions within `D_m` if need be; at most one
  region besides home has no players; nothing is reserved or being released; and no
  call of the coordinator says anything to anybody from then on, but to read the
  list.
- R6, the same calls give the same answers; and crowds given in another order do.
- R7, with `follow: None` the same runs begin nothing.

**End to end, from this record alone** (`bin/clustine/tests/follows.rs`, on the
cluster of `merges.rs`: two stripes, ledger bots, three seconds of lease, the
coordinator started with `--reshape by-itself --merge-distance 1 --split-distance 3
--rest-seconds 2`; every test audits the world against the ledgers and fails if a bot
is disconnected):

- E1. Bots in two groups, one in each stripe, four chunks apart, walk towards each
  other: the regions are merged without anybody asking, once.
- E2. One group walks away, to more than three chunks from the other and from the
  origin, and stands: one split; the part is moved to the other worker no sooner than
  two seconds later, if the workers' regions differ by two.
- E3. E1 and E2 ten times in a row with the same bots.
- E4. Two groups walk up and down beside each other for a minute, never more than
  three chunks from the origin: two chunks apart in one region, and then two chunks
  apart each in its own stripe, which were never within one: no merge and no split in
  either minute.
- E5. A group that was split off leaves the game, and a bot joins: the region the
  group left is absorbed six seconds later by a region without players that has a
  lower id, or stays if there is none, and no bot waits longer for an acknowledgement
  than undisturbed, the one that joined included.
- E6. A worker is killed at a logged moment of a merge and of a split that the
  coordinator began by itself (the survivor's, the absorbed region's, the split
  region's): every region runs again within the lease and a moment, the merge or the
  split is whole or not at all, and afterwards the regions are again what the
  distances say.
- E7. The same with the coordinator and with the world store killed.
- E8. The pauses: no bot waits for an acknowledgement longer than `merges.rs` allows,
  and no bot more often than once in two seconds for longer than undisturbed.

The chunk players enter in is the origin, three chunks west of where the stripes
meet, and counts as a player of the home region: a group of another region within one
chunk of it is merged into home for that alone. On stripes a group that walks on
falls back (K15); the bots of these tests stand where they were split off, or the
test counts on it.

## What a player notices

With the numbers of section 3, an optimised build, and the pauses that were measured.
"Home" is the home region, which has everybody within reach of the spawn point: within
22 chunks (352 blocks) of the chunk players enter in or of somebody who is.

- **Two players walk towards each other**, from far apart. When they are 22 chunks
  apart, a second or two later, both stand still once: the survivor's side for a
  fifth of a second, the other for two fifths. They are 350 blocks apart then and do
  not see each other. Nothing more happens as they meet, pass, build and stand
  together.
- **They walk away from each other.** When one is more than 30 chunks (480 blocks)
  from the other, from everybody else of the region and, in the home region, from the
  chunk players enter in, both stand still once for a fifth of a second. If that
  leaves one worker with two regions more than another, a region of that worker is
  moved, one that has rested and of those the one with the fewest players: a third
  of a second for them.
- **They stand side by side, or walk together**, away from others. Nothing, ever,
  with one exception: when they walk away from the spawn point together, they are
  split off home once, 30 chunks out, and everybody near the spawn point stands still
  for a fifth of a second with them.
- **They walk along each other at the rim.** Around 22 chunks: merged the first time
  they are within it, and not split until they are 30 apart. Around 30: split once,
  and not merged until they are within 22. To be stood still again they have to cross
  the 128 blocks between the two.
- **They fly.** The same, sooner. A player who flies back and forth across those 128
  blocks is merged and split each time, and with them everybody in the two regions,
  but never sooner than ten seconds after the last time. A player who sprints in
  the air at another sees across a region boundary for a second before the merge.
- **One leaves.** Nobody notices. If they were the last of their region, it is
  absorbed half a minute later by a region without players, or kept.
- **One joins.** Nobody else notices: the spawn point is home's whether anybody is
  there or not, and whoever was within reach of it was home's already. The player who
  joins and walks off is split off 30 chunks out, as above.

**The bound.** Between the end of anything that stood a region's players still and
the next merge, split or move that the coordinator begins with that region by itself,
ten seconds pass. A player changes region without standing still only by walking into
another region's chunks. So **a player who does not do that is stood still at most
once in ten seconds** by all of this together, for a fifth to two fifths of a second
(three times that unoptimised), and only while somebody keeps crossing a distance. In
ordinary play it is once on coming within 350 blocks of others and once on going 480
away.

**What costs more than it looks:**

- **Everybody near the spawn point stands still for a fifth of a second whenever a
  group walks out of reach of it or into reach of it**, at most every ten seconds.
  That is the price ADR-0014 put on closing the links at a split, paid where the most
  players are. If the owner feels it, the remedy is ADR-0014's "keeping the links
  open", not another distance.
- **Twenty players who gather from twenty directions**: regions that are near each
  other merge in pairs, four at a time, each resting ten seconds in between. If all
  twenty are near each other, nobody is stood still more than about five times, over
  a minute; if they arrive one by one at a crowd, the crowd is stood still once in
  ten seconds for as long as they keep arriving, and those who wait their turn merge
  among themselves. Meanwhile they see each other across region boundaries, which
  works as between stripes.
- **A merge that fails** stands the absorbed region's players still as a move does,
  for nothing. Not again for half a minute, then a minute, up to four.
- **A split that finds nobody** stops the region for a few ticks, and the region
  rests as after one that was made.
- **On stripes** (every world of step C4): K15. A player who walks away from another
  is split off again every ten chunks, and both stand still each time. This is why
  `by-hand` is the default in C4.

## What step C5 needs of this

- **`by-itself` becomes what `--reshape` is unless told otherwise**, and pinned
  regions with `by-hand` are for the tests that want boundaries where they put them.
- **The single process drives the same `Coordinator`**: `tick` every
  `Coordinator::LOOK`; `players` with the status of each of its runners every `LOOK`,
  and after it has told the coordinator of an outcome only statuses read after it;
  `listed` from its own store; and what `Changes` says done to its own runners, as one
  worker with one name. With one worker nothing is ever evened out.
- **A world that begins as one home region**: the first player who walks 30 chunks
  from the spawn point is split off. The rules need no second region to begin with.
- **What must not be assumed here that holds only on stripes:**
  - that the regions of the layout are known from the start: which region is home
    comes from the list alone, and section 5.1 waits for it;
  - that every chunk is some region's: off stripes a player can stand in a chunk the
    region has asked for and not been granted, and is no seed until it is (section
    5.5, "not yet");
  - that a part cannot grow: it does, and K15 goes;
  - that a region without players still holds chunks: off stripes it gives everything
    back in thirty seconds unless a guest watches, and the empty region that is left
    over holds nothing as a rule;
  - that region ids below the number of stripes mean anything.
- **What C5 has to look at again**: `RoutingTable::layout`, which nothing here reads;
  whether one region without players that is never absorbed while home is busy is
  wanted (section 4.4), now that it holds no stripe; and the first trial by the owner
  of all of the above, which on stripes is spoilt by K15.

## Ruled out

- **Grouping that depends on the order of players**: a group grown from the first
  player listed, or centres moved towards their players. Connected sets do not.
- **A survivor chosen by where it runs** (the lighter worker, the one that already
  has both): two coordinators, or one after a move, would choose the other way round
  and regions would chase each other. Home, players, id.
- **A timer that merges or splits on a schedule**, or looks only every few seconds:
  the rules are looked at four times a second and begin only what the distances say.
- **Positions in the heartbeat**, as ADR-0010 has it. Once a second is too slow for
  two players in flight, and a heartbeat is filled from a watch at the moment it is
  sent, so one sent after the word of a split could carry crowds from before it.
- **Acting on the first look.** See K10.
- **Dropping a sighting when its region is reserved or silent.** A region between two
  groups would vanish for the length of its own merge, and its neighbour would be
  split for that.
- **Splitting first, always, or merging first, always**, when a region wants both.
  Each is three stops where the other is one, in one of the two cases of K2.
- **The home region's players alone deciding what is home's.** Then whoever stands
  near the spawn point in another region is stood still by everybody who joins.
- **An empty region absorbed by its nearest neighbour or by home**, as ADR-0010 has
  it. See section 4.4.
- **One rest after a merge and another after a split**, or a longer one before a
  region is put back as it was. One number bounds everything, and the band between
  the distances is what keeps a region from being put back.
- **A split that takes every group but the one that stays.** It would stand those who
  stay still once where one group per split does it once per group. It makes parts
  that have to be split again, and the plan for this step has a split take one
  group. It is a change to which chunks are named and to nothing else; see the open
  questions.
- **Several splits at a time.** See section 5.3.
- **Naming the players who go.** The coordinator has no names, and a report with a
  thousand of them four times a second would be the largest thing workers say.
- **Evening out as it is, with a lease after every merge and split in the world.**
  See section 6.
- **The list on a timer also when the coordinator decides nothing.** See section 7.

## Consequences

- Regions follow players without anybody asking, and nobody who stays in the regions
  they are put in is stood still more often than once in ten seconds for it.
- The home region is everybody within reach of the spawn point, and every arrival
  there and departure from there is a stop for all of them.
- Workers say four times a second where their players are, and the coordinator looks
  at leases four times a second when it decides by itself, where it looked every
  quarter of a lease.
- The coordinator has two ways of evening out and two ways of reading the list until
  C5 takes `by-hand` out of ordinary use.
- One region without players can stay for good.
- The absorbed pairs of the list turn over quickly: 4096 merges are a few hours of a
  busy world. An edge that was away for longer than that finds regions it has
  something of gone without a pair (ADR-0014, open question 4).

## Changes to ADR-0010

1. **Section 7, "with each heartbeat"**: in a message of its own four times a second,
   behind the outcomes of merges and splits; with the epoch and the tick.
2. **Section 7, "merge two regions when a player of one is within the merge distance
   of a player of the other"**: unless that player is of a group that is to be split
   off first; and the chunk players enter in counts as a player of the home region.
3. **Section 7, "split a region when its players fall into groups any two of which
   are further apart than the split distance"**: unless players of another region
   within the merge distance of both join them (K2).
4. **Section 7, the merge distance**: `2 * (V + 1) + 4`, the split distance 8 more;
   "a region that was merged or split is left alone for some seconds" is ten, and
   holds of a region that changed hands and of evening out as well.
5. **Section 7, a region without players**: absorbed by a region without players, or
   kept.
6. **Section 5, "the largest otherwise"** stays; of the groups that go, one goes at a
   time, the largest.
7. **Section 6, "reads the list ... every few seconds"**: every lease, and only when
   the coordinator decides by itself. The bounding box is not used.
8. **Section 4, "`A` survives: ... else the one with more players"**: and of two with
   as many, the lower id.

## Changes to ADR-0014, ADR-0012 and ADR-0009

1. **ADR-0014, section 5.5, evening out**: when the coordinator decides by itself, a
   region is not moved while it rests, in place of nothing being moved within a lease
   of any merge or split; and the region with the fewest players is the one moved.
   **ADR-0009, section 7**, likewise.
2. **ADR-0014, section 5.2, "reading it every few seconds ... waits for C4"**: section
   7 here.
3. **ADR-0014, section 5.5, "what C4 needs of this"**: the outcome is taken where it
   is put into `Changes`, not from `Changes`; the reason is the `Undone` and its `Off`.
4. **ADR-0014, open questions 6 and 7**: sections 4.3 and 3 here.
5. **ADR-0014, "Found by the tests under bots", 2 and 3**: `Off(Nobody)` is "not yet"
   (section 5.5); a region that is being released is not free, and is waited for.
6. **ADR-0012, section 4.8, "the runner ... counts the held chunks there as well"**:
   nothing reads `RegionStatus::held` in this step either. **Open question 1** (a
   region that took in an arrival for a chunk it had given back): it is split if the
   player is far from its others, K14.
7. **ADR-0010, section 9, and ADR-0014**: step C0's `Players` changes its shape.

## Open questions

1. **Whether ten seconds is the rest the owner wants.** It is the one number a player
   feels. Longer costs nothing but regions that stay as they are for longer.
2. **Whether one group per split is right at a busy spawn point.** Taking every group
   that goes at once would stand home's players still once instead of once per group
   where several groups leave within ten seconds, and nothing would queue (K21). It
   is what this record would choose if the plan did not have one group; see "Ruled
   out".
3. **`AT_ONCE`** is a guess at what keeps a burst of merges within their leases; a
   merge that runs out of its lease marks a worker at fault.
4. **Whether the coordinator should refuse to start when its view distance is not the
   edges'.** Nothing tells it theirs.
5. **A hundred players in one place.** Section 4.5 leaves the rule to whoever measures
   what one thread carries.
6. **Whether a region at the far end of a long row should be split off although it is
   joined.** ADR-0010's limit.
7. **Whether the island of K15 wants an answer before C5**, for the owner's trial of
   this step: the only one this record sees is to try `by-itself` with C5.

## Risks

- **The home region is where the stops are.** Every group that leaves the surroundings
  of the spawn point and every group that comes back stops everybody there. The bound
  is one in ten seconds; a busy server reaches it.
- **A report that is long on its way** is taken for new (K11). Everything else about
  age is exact; this rests on connections that deliver within the rest.
- **The order on the worker's connection** (section 2.2) is a property of how
  `cluster.rs` is written, and no test from outside can see it broken. The rest hides
  a breach of it.
- **A stale sighting that is wrong** holds a split back for as long as its region is
  silent, and a region without an owner is silent until somebody runs it. Nothing is
  begun for it; a region stays larger than it need be.
- **A split decided while a region next to it is silent** can part two groups that a
  player of the silent region, handed over a moment before, would have joined (K10's
  second half, if the other region does not report). It is put right by a merge.
- **Merges that fail cost a move each time**, and `TooLarge` fails every time. The
  doubling bounds it at one in four minutes for a pair of crowds.
- **Four ticks a second** of a coordinator that used to tick every 1.25 s: `settle`
  and `finish` walk every region and worker each time. Not measured with a hundred
  regions.
- **The tests of C4 run on stripes**, where parts are islands and no chunk is free.
  What only shows off stripes (a chunk asked for and not granted, parts that grow,
  regions that hold nothing) is first tried in C5.
- **Two coordinators at once** would both decide. The store lets one merge and one
  split win; M3 has one coordinator.

## Not checked

- **The coordinator's tests** were read only where they are about evening out after a
  merge or a split (seven tests, in `state.rs` and `tests/reshape.rs`) and for how
  their clusters and their generated run are built. That no other test rests on
  `tick` doing nothing between `settle` and `even_out` follows from `follow: None`
  changing nothing, not from reading them.
- **The service's tests** were read for how they hand readings in and hold them back,
  not one by one.
- **The world store** was not read. That a claim of a chunk in another region's
  pinned area is answered `foreign`, on which K15 rests, is ADR-0011's sections 1 and
  3.2; `Land::keeps` was read in the simulation.
- **The edge** was read at `view_area` only, and ADR-0013 not at all. That a guest's
  subscription and a hand-over cost a player nothing they notice is what M2 and the
  roadmap say; that a region keeps a chunk while a guest watches it is ADR-0010's and
  ADR-0012's word. Of ADR-0014's contract with the edge, rule 42 was read for K13.
- **Whether the simulation limits how fast a player moves** was not looked for. The
  speeds in section 3 are the game's, from memory.
- **The size of a report** is reckoned from postcard's encoding of the fields.
- **`bin/clustine/tests/merges.rs` and `tests/common/processes.rs`** were read at
  their heads and for the names of what they offer.
- **`deploy/`** was listed, not read.
- **That nothing but `stay_registered` takes from the queue of outcomes**, and that
  `WorkerClient`'s task sends what is queued in the order it was queued, were read in
  `cluster.rs` and `client.rs`; that `tcp::link` keeps the order was not read.
