# ADR-0016: When to merge and when to split

- Status: **Proposed**, revised after an independent review against the code; not
  built. The design of step C4 of milestone M3, phase C: the coordinator decides by
  itself. It changes no code of the simulation, the region runner, the world store or
  the edge. What the review found and what was changed for it is in "Review", at the
  end.
- Date: 2026-10-09

## Context

Regions merge and split when somebody asks ([ADR-0014](0014-merging-and-splitting.md),
[ADR-0015](0015-the-edge-through-merges-and-splits.md)): `clustine merge`, `clustine
split`. [ADR-0010](0010-regions-that-follow-players.md), section 7, says in half a page
when the coordinator is to ask by itself. This record says how, exactly enough to build
it and to write tests from.

What the code is today, as far as this record builds on it or changes it. Each of these
was read in the code at commit `11764c3`, and again for the revision at `ec08d1c`, which
has the same code but for one test; "Not checked" at the end says what was not.

- **A message for where players are exists and nobody uses it.**
  `ToCoordinator::Players { regions: Vec<(RegionId, Crowds)> }` is step C0's. No worker
  says it, `Service::heard` closes the connection of whoever does (it falls into the
  last arm of the match), and `wire.rs` has one in a round trip.
- **A runner shows where its players are.** `RegionStatus::crowds()` gives the chunks
  with players in them, each with how many, and `RegionStatus::tick` the last tick.
  `RegionRunner::show_status` fills both when a runner is made (`with_store`), after
  every tick, and in `begin_anew`, which a merge and a split run **before** they call
  the outcome. So from the moment a worker process is handed the outcome of a merge or
  a split, the status is that of the region after it. `show_status` stores the tick
  first and the crowds after it, so whoever reads the two one after the other can find
  a tick with the crowds of the tick before. `Region::crowds` counts every player by
  the chunk they stand in, whether the region holds that chunk or has only asked for
  it.
- **The worker process** (`worker` in `bin/clustine/src/cluster.rs`) looks at its
  regions every 250 ms (`LOOK`): at `status.tick` and `status.store_lost` of those in
  `Phase::Running`. It reads no crowds. Heartbeats are sent every second
  (`HEARTBEAT_INTERVAL`) by the task of `WorkerClient`, with what a watch holds at
  that moment. The outcome of a merge or a split goes another way: the worker's loop
  puts it into a queue (`endings`), `stay_registered` takes it out and calls
  `WorkerClient::absorb_ended` or `split_ended`, which queue it for the connection.
  **After it has registered again, `stay_registered` says `split_ended(.., Ok(part))`
  from a watch (`split`) for every part that the new orders do not name, before it
  takes anything from that queue.** The loop does three things inline, and looks at
  nothing meanwhile: it waits for the runner's thread when a release has ended and
  when a region has lost the store, and it restores a region that the store has
  opened for it (`RegionRunner::restore`, in the arm of `Settled::Opened`).
- **What `Prepare` does.** A worker that is told `Prepare { region, epoch }` hands
  the runner of that region, if it runs it with that epoch, `Reshape::Prepare`. A
  runner that only runs (`Phase::Running`) then makes an ordinary checkpoint
  (`RegionRunner::checkpoint`: every chunk with unsaved changes and the region's whole
  state go to the store) and ticks on with its links; one that is releasing or in the
  middle of a merge or a split does nothing. Nothing is answered, and nothing about
  it knows of a merge: it is the checkpoint a worker makes every 300 seconds unless
  told otherwise. `Coordinator::merge` has it said to the survivor's owner; a split has
  none, and the first checkpoint of a split, during which the region still ticks
  (`Preparing`), is as large as what has changed since the last one.
- **The coordinator's state machine** (`services/coordinator/src/state.rs`):
  `Coordinator::tick` is `settle`, then `even_out`, then asking for the list if a
  split is `owed` a reading or a merge has had its worker's word. `merge` and `split`
  refuse with a `ReshapeRefusal` or note the reservation and say what workers are to be
  told; each calls `seen` once it has not refused and `finish` last, as the calls that
  can change who owns what do. `heartbeat` calls neither. A merge and a split have one
  lease. `reshape_ended` is called for the regions of every merge and split that
  ends, however, and notes the time in `reshaped`. `even_out` begins nothing while a
  release, a merge or a split is under way, nor within a lease of `reshaped`, and
  moves the region with the highest id of the worker with the most. `listed` adds a
  living region it does not know **unless a split is reserved**. `split` names
  `self.next`, the next id of the list as `listed` was last handed it; nothing else
  changes it, so after a split whose reading fails it is an id that is taken.
  `release_for_leavers`, which `finish` runs, asks a leaving worker to release each of
  its regions that is neither reserved nor being released, as soon as there is a
  worker to take it.
  `Coordinator::report`, which a registration calls for every region the worker says
  it holds, also for one it owns already, sets the epoch it was told and counts the
  region as vouched for.
- **The service** (`service.rs`) calls `tick` every quarter of the lease, at least 50 ms
  apart (`tick_interval`), and reads the list on events only. A reading asked for while
  another is under way has that one thrown away and made again, so whatever asks at
  every tick must wait for its answer first; `Coordinator::reading` is how `tick` does.
  It says only whether the state machine itself has asked: the service also reads the
  list by itself at every registration and before it looks at what is asked by hand.
- **A split takes who stands in the chunks named.** `Region::split`
  (`crates/clustine-sim/src/region/reshape.rs`): the seeds are the chunks named that
  the region holds (`Knowledge::Held`), that are not the home chunk and in which a
  player stands; who stands in a seed goes and everybody else stays; a chunk the
  region holds goes if it is nearer to a seed than to every chunk a stayer stands in
  and than the home chunk, if the region holds that, **and stays if it is as near to
  the one as to the other**. No seed is `NoSplit::Nobody`. The runner works a split
  out in `commit`, **after** the region has stopped ticking, and on `Nobody` ticks on
  with its links (`tick_on`): a split that finds nobody costs the region's players a
  stop of a few ticks and no resume. It tries once more with the store's next id if
  the id it was told is taken (`Decline::NotNext`), and answers `Off::TooLarge` if the
  two states of the split together are longer than it may hand the store.
- **What a player sees.** `view_area` in `services/edge/src/fanout.rs` sends a client
  with view distance `V` every chunk up to `V + 1` away along an axis. The edge grants
  at most `--view-distance`, 8 unless told otherwise, 32 at most.
- **Who holds a chunk** (ADR-0011, section 1): the region it is granted to, else the
  region pinned to an area that contains it, else nobody. A pinned region never gives
  a chunk of its areas back (`Land::keeps`). The list says of every region which areas
  it is pinned to (`RegionInfo::pinned`); a part is pinned to none, and a survivor is
  pinned to what the region it absorbed was pinned to as well. Until step C5 every
  world is stripes, and the stripes cover it: **no chunk is nobody's**, and a region
  that was split off can be granted nothing it was not split off with.
- **What was measured** (roadmap, "Where M3 stands"; optimised, in the middle / at
  worst): a merge stands the survivor's players still for 0.19 / 0.22 s and the
  absorbed region's for 0.37 / 0.41 s; a split those who stay for 0.14 / 0.20 s and
  those who go for 0.16 / 0.17 s; a move 0.27 / 0.31 s. Unoptimised, in the middle: a
  merge 1.10 and 1.28 s, a split 0.57 and 1.04 s, a move 1.48 s, which is three and a
  half to six and a half times as long.

## Decision

Words in `code` are names in the code, or will be. `D_m` is the merge distance and
`D_s` the split distance, in chunks. "By itself" is what the coordinator does unasked;
"by hand" is `clustine merge`, `split` and `move`. An **absorption** is the merge by
which a region without players goes into another without players (section 4.4); every
other merge the coordinator begins by itself is "by the distances".

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
   newer one. A merge stands by its two regions, and of a split **every group that is
   to go stands by itself**.
4. **A split takes every group that is to go and has stood**, as one new region. That
   region is split further by the same rules when it has rested, without the players
   who stayed behind.
5. **Every region is left alone for a rest**, ten seconds, after a merge, a split or
   a change of owner, by merges, by splits and by evening out alike. That is the bound
   on how often anybody stands still.
6. **Ignorance holds back and begins nothing.** Only a region whose sighting is fresh
   is merged or split. A sighting that is no longer fresh still counts where it can
   only hold a split back, and while a region has never been sighted at all nothing is
   merged or split anywhere.
7. **An empty region is absorbed at nobody's expense**: by a region that has no
   players either, which is not held to a rest for it, and never if it is pinned to
   an area.
8. **The paths of ADR-0014 are called as they are**, with nobody as asker. The
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
    /// A tick of the region: `crowds` is of this tick, of the one before it, or of a
    /// later one.
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
reads `status.tick` and then `status.crowds()` of each running region and queues one
`Players`. The crowds can be of the tick before the one it read, when the look falls
between the runner's two stores (see the context), or of a later tick. Nothing rests
on which: if the region ticks on, the next report puts it right, and if it stands
still from that very tick, its sighting has crowds one tick old, is not fresh a second
later and can only hold back. Three rules:

- **It goes down the queue the outcomes go down** (`endings`, whose `Outcome` gains a
  case for it), and `stay_registered` passes it on with a new
  `WorkerClient::players(Vec<PlayersOf>)`, which queues it as `absorb_ended` and
  `split_ended` do. Not a channel of its own and not the heartbeat's watch. Then the
  order on the connection is the order in which the loop made them, and since the
  status is of after a merge or a split from before the loop hears the outcome:
  **within one registration, a report that follows the word `AbsorbEnded` or
  `SplitEnded` on the connection was read after that merge or split.** With two queues
  `stay_registered` would take from them in either order, and a report read before a
  split could follow the word of it.
- **A report is said under the registration it was made under, or not at all.**
  `stay_registered` counts its registrations and shows the loop, in a watch, the
  number of the one it holds a connection of, or that it holds none. The loop queues a
  report only while there is one, and with that number; `stay_registered` passes on a
  report only if its number is that of the connection it holds, and drops it
  otherwise. **Only reports are ever dropped, never an outcome.** Without this the
  rule above broke across a registration: a report read before a split and still in
  the queue when the connection ended was said on the next connection behind the
  `SplitEnded { Ok(part) }` that `stay_registered` says from its watch (see the
  context), and was taken for a report of after the split.
- **None is queued while the worker has no connection to the coordinator**, which is
  the same watch. The queue has no bound, and four reports a second for as long as a
  coordinator is away would fill it.

**What does not hold across a registration.** The first rule is about one connection.
A word that was on its way when a connection ended is lost with it, and the
reservation then ends by a reading of the list or by its lease, not by the worker's
word; that case is K11's, and the rest covers it. Nothing else is left: every report
on a new connection was read after the registration that made it.

A region that stands still (it waits for the store, or its runner has ended and the
loop has not yet seen it) is reported with the tick it stopped at. A worker that
restores a region, or waits for the thread of one, reports none of its regions
meanwhile (see the context): their sightings stop being fresh if that takes more than
a second, and nothing is begun with them until it is over.

#### 2.3 What is taken, and what is kept

`Coordinator::players(now, name, regions) -> bool` returns whether the worker is
registered; the service closes the connection of one that is not, as for a heartbeat.
To say it is to be heard from. Like `heartbeat` it changes no owner and calls neither
`seen` nor `finish`. **A coordinator that decides nothing by itself keeps nothing of
it.** Otherwise each entry is taken or passed over:

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

Beside the sighting, per region:

- `empty_since`, the time of the first of the unbroken run of taken reports without
  players;
- **`alone_until`, before which the coordinator begins nothing with the region by
  itself. This one field is what "rests" and "is left alone" mean wherever this record
  says them**: for a rest, for `LONG` and after a refusal (sections 5.2, 5.4 and 5.5),
  and evening out keeps away from it as well (section 6). One thing does not look at
  it: the choice of a survivor for an empty region (section 4.4);
- two counters of attempts that failed (section 5.5);
- whether it was split last (section 5.3);
- `prepared`, when its owner was last told to prepare it for a split (section 5.6);
- `absorbed_at`, when the last absorption it was the survivor of ended, well or not
  (section 5.5);
- whether the last reading of the list that succeeded has it pinned to an area
  (section 7); a region no reading has shown, which is a part, is not.

And per thing that is wanted, since when (section 5.3).

#### 2.4 What is forgotten, and when

| When | The sighting | The rest |
|---|---|---|
| A merge or a split of the region begins, whoever asked | stays, `taken` is nothing: not fresh until a report is taken after the end | `empty_since` and `prepared` forgotten |
| A reading of the list shows the region absorbed, by a merge that was noted or not | its crowds are added to the sighting of the living region it went into, by the pairs of that reading, if the coordinator knows that one; a sighting is made for that one if it has none, of nobody and not fresh | all of it goes; what waited for a merge with it waits for a merge with that region, since the earlier of the two times (section 5.3) |
| A split ends with the worker's word `Ok(N)` for the split that was noted | the crowds in the chunks that split named go from the region's sighting to a sighting of `N`, of `N`'s owner and epoch, tick 0, not fresh | `N` begins like any region that is given an owner |
| A merge or a split ends otherwise | stays as it is | section 5.5 |
| The region is given an owner or an epoch that the coordinator did not have for it, or loses its owner | stays; it is no longer of the region's owner and epoch, so not fresh | `empty_since` and `prepared` forgotten; `alone_until` at least a rest from now if it was given an owner or an epoch |
| The region's owner registers again with the epoch the coordinator has for it | stays, and is fresh if it is not too old | nothing |
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
| margin | `min(3, (D_s - 1) / 2)` = 3 | how far around a group's chunks a split names chunks, and how far a group may move in a tick and still be the same group |
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
`(D_s + 1 - 2 * margin) / 2` = 12 of anybody who stays stay with their region, if
nobody has moved more than the margin since the report (section 4.3), which is more
than the reach; and within `(D_s + 1 - margin) / 2` = `V + 6` of the home chunk they
stay with the home region. That answers open question 7 of ADR-0014 for these
distances: a player who joins is shown the chunks around the spawn point by the home
region.

`--merge-distance` and `--split-distance` set the two outright, for tests, whose bots
walk a chunk in under three seconds (section 11). They are refused unless `1 <= D_m`,
`D_m + 2 <= D_s` and `3 <= D_s`, so the margin is 1 at least and twice the margin is
less than `D_s`; with small distances nothing above about views holds, and nothing
breaks.

### 4. What is wanted

A pure function in a module of its own, `services/coordinator/src/policy.rs`: no clock,
no map that is not ordered, and no state. It is given the distances, the chunk players
enter in (`ChunkPos::containing` of `CoordinatorConfig::spawn`), the home region of the
last reading of the list, and for every region the coordinator knows that has a
sighting: its id, its crowds and whether the sighting is fresh. It knows nothing of
owners, reservations, pinned areas or time.

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

`decide` gives the merges in ascending order of gap, then of the lower id, then of the
higher. That is the order of its answer, for its tests; the order in which merges are
served is section 5.3's.

#### 4.3 Splits

**A split is wanted of a region that is surely apart.** Its places fall into **groups**
by the cluster by all it has heard that they are in. Any two groups are more than
`D_s` apart, as they would be linked otherwise.

- **The group that stays**: in the home region, the one with the chunk players enter
  in; in any other region, the one with the most players, and of several such the one
  that has the region's lowest chunk among them.
- **The groups that go**: all the others. `decide` gives each as the chunks its
  players are in, ascending, and the groups in the order of their lowest chunks. Which
  of them a split takes is not `decide`'s to say: each has to have stood by itself
  (section 5.3), and **one split takes every group that has**.
- **The chunks named** for some of the groups (`policy::named`): every chunk at most
  the margin away from a chunk of one of them, ascending, each once. That is 49 chunks
  around each occupied chunk, less where they overlap. A chunk whose coordinate would
  lie beyond what a coordinate can be does not exist and is left out; the coordinates
  are neither clamped, which would name a chunk twice, nor wrapped, which would name
  chunks at the other end of the world.

**What `Region::split` makes of several groups** (see the context; nothing of it
changes). Whoever stands in a chunk named that the region holds goes, so the players
of every group named go, **into one new region**. Everybody else stays: the group that
stays, and every group that was not named because it had not stood yet. A chunk the
region holds goes if somebody who goes is nearer to it than everybody who stays and
than the home chunk, where the region holds that; a chunk that is as near to the one
as to the other stays. So:

- **between a group that goes and anybody who stays**, the chunks nearer to the group
  go, and the row of chunks in the middle, where there is one, stays;
- **between two groups that go**, every chunk goes that is nearer to either of them
  than to anybody who stays: the part is one region that holds both groups and what
  lies between them, however far apart they are;
- **around a group that was not named**, the chunks stay as around anybody who stays.
  Its players are no seeds. It is split off by a split of its own when the region has
  rested, if it is still apart then.

**The part is a region like any other.** Groups that went together are more than
`D_s` apart, and in different clusters as they were, so the part is surely apart as
soon as its worker has reported it, and is split when it has rested: the group with
the most players stays in it and the others go into a further region, until every
group is a region. The players who stayed behind at the first split stand still once
for all of it; the groups stand still once for each split they are in. K9 has the
numbers for many groups.

**What the margin is for**: the time between the report the chunks are named from,
which is the sighting of the tick that begins the split, and the tick at which the
region stops. In it are the age of that report (a quarter of a second as a rule, a
second at most), the order on its way, and **the first checkpoint of the split with
its flush, during which the region ticks on** (ADR-0014, section 3.1, `Preparing`). The
second the group had to stand is not in it. So that the checkpoint is short, the
region's owner is told `Prepare` about a second before (section 5.6); what is left is
tens of milliseconds for what changed in that second, as before a merge. A player in
flight covers a chunk in a second and a half, and one who sprints in the air in three
quarters of a second, so a margin of three chunks is two seconds and more, and of the
least margin there is, one chunk, the better part of a second. `Region::split` takes
who stands in a chunk named **that the region holds**, so naming more than is held or
occupied costs nothing. The margin is less than half of `D_s`, so nobody who stays,
and nobody of a group that has not stood, can be standing in a chunk named unless
they have come most of the way over since the report. This answers open question 6 of
ADR-0014.

What the margin does not catch: a player of a group who was faster stays, in a
region whose other players are far away, standing on a chunk that stays with them
among chunks that went. If they walk on, they step into the new region's chunks and
are handed over, which costs nothing; if they stand, they are a group to split off
when the region has rested, and that region is merged with the part in turn. A
checkpoint that takes longer than the margin allows (a region in which a great deal
was built since its last one, told to prepare a second before) lets a whole group in
flight get away: the answer is `Off::Nobody`, "not yet" (section 5.5), and the next
attempt, a rest later, finds the checkpoint made. And a player who stands in a chunk
the region has asked for and not been granted is no seed: with nobody else in the
chunks named, the answer is `Off::Nobody` as well.

`decide` gives the splits in ascending order of the region's id; the order in which
they are served is section 5.3's.

#### 4.4 Empty regions

This part needs the time, the owners and the list, so it is the state machine's
(section 5.3). **A region is absorbed** if all of this holds of it:

- it is not the home region, and the list does not have it pinned to an area;
- its sighting is fresh and has no players, and it has had none for `EMPTY_FOR`
  (`empty_since`);
- it is free (section 5.2);
- it has a survivor.

**Its survivor** is the first of these that has a fresh sighting without players, has
had none for more than `FRESH` (`empty_since`), is free **but for its `alone_until`,
which is not looked at**, and is in nothing begun at this tick: the home region; then
the regions that are not the home region and have a lower id than the empty one, the
lowest first, pinned or not. If there is none, the region stays.

**Why the survivor's `alone_until` is not looked at, and why it does not rest
afterwards** (section 5.5): the rest bounds how often players stand still, and here
nobody does. In the first version of this record every absorption left its survivor
at rest for ten seconds, and all the empty regions of the world had one survivor
between them: one was absorbed in a rest however many there were, and they piled up
wherever players made them faster. Everything else that makes a region not free holds
of a survivor as of any region.

**Why the survivor has to have been without players for more than `FRESH`**: nothing
is begun on one look. A region whose only player is missing from one report (K10), or
that somebody joined a moment ago, would otherwise stand its players still for a
region nobody is in. `empty_since` is forgotten when a merge of a region begins
(section 2.4), so this also puts more than a second of reports between two
absorptions into one survivor: if anybody has come into it, one of them shows it.

**How many at a time.** One for each survivor, because a region in a merge is
reserved; and no more than `AT_ONCE` in all, the merges and splits by the distances
counted. While the first candidate is in an absorption, its sighting is not fresh and
the next candidate is the survivor of the next empty region, so several go on side by
side. A survivor absorbs again about a second and a half after the last time. A
region that has been a survivor begins its `EMPTY_FOR` anew, for the same reason: of
many empty regions that are due at one moment, about half are absorbed at once by the
other half, and of those half again half a minute later.

**Why a pinned region is never absorbed for being empty.** Every world of step C4 is
stripes, and a stripe without players that went into another half a minute after the
cluster came up, or after its last player left, would take with it the boundary that
the tests of hand-over, of chaos and of moves put there, and that the owner who
starts a cluster and opens the client a minute later expects to find. A pinned region
holds its areas whether anybody is there or not; there is nothing to give back.
**Merges by the distances are not affected**: pinned regions merge like any others
(section 4.2), the survivor is then pinned to the areas of both, and that is what the
end-to-end tests of section 11 begin with. A pinned region can be the survivor of an
absorption. Off stripes this rule does nothing: only the regions of a layout are
pinned, and step C5 takes the layout away. For the home region it changes nothing
either, as that is never absorbed.

So of the regions without players that are not pinned, one is left while the home
region has players, the lowest, and that one only if no pinned region without players
has a lower id still; and none is left once the home region has no players. **Nobody
stands still for an empty region**: the survivor has no players, and the absorbed
region has none unless one came at that very moment (K13). ADR-0010 has an empty
region absorbed by the region whose chunks are nearest, or by the home region; that
would stand the players of that region still, at every departure of somebody
elsewhere, for chunks that are given back half a minute later anyway.

#### 4.5 A crowd, and room for a later rule

A hundred players in one place are one cluster, one region and one thread, and so is a
row of players each within `D_m` of the next, however long: ADR-0010's limit, which
this record keeps. Nothing here looks at load. A merge of two crowds whose state
together is too large to hand to the store is off (`Off::TooLarge`), and is tried
again ever more rarely (section 5.5). A region that has grown past that size by joins
and hand-overs cannot be split either, as the two states of a split are handed over
together and are no smaller than the one: that limit is ADR-0014's, and by itself it
is the home region that would reach it.

A later rule has what it needs without anything new in a worker: `Region::split`
takes whoever stands in the chunks named, so a crowd can be cut along a line by naming
the chunks on one side of it. What it would have to add is here, in this function: a
further reason to want a split, and the memory that two regions were parted on
purpose, without which the merge rule would join them again at once. `decide` returns
what is wanted with why (`Near`, `Apart`), so that a further reason is a further case.

### 5. What is begun, and when

`Coordinator::tick`, when the coordinator decides by itself and the list has told it
which region is home, does between `settle` and `even_out`: works out what is wanted
(`decide`), notes since when each thing has been wanted without a break, in ordered
maps, and begins what may be begun. With `follow: None` it does none of this, and
nothing of sections 6 and 7. Whoever drives the coordinator calls `tick` at least
every `LOOK` then (`Coordinator::LOOK`); the service's `tick_interval` becomes the
shorter of that and a quarter of the lease. Nothing is decided in any other call.

#### 5.1 What has to hold of the world

No merge and no split is begun by itself, and no absorption, unless all of these hold:

- the coordinator's grace period is over (one lease from when it was made);
- **the list has been read, and the last reading that succeeded is no more than two
  `LIST_EVERY` old**: without the list the coordinator does not know which region is
  home, which regions are pinned, or what became of a merge. A reading that fails
  holds nothing back by itself; only the age of the last one that succeeded does;
- **every region the coordinator knows has a sighting**, fresh or not. A sighting
  that was made for a part, or for a region that took in another (section 2.4),
  counts. A region that was sighted once and has been silent since does not stop
  anything here; it holds back what section 4 says it holds back;
- fewer than `AT_ONCE` merges and splits are under way, whoever asked for them;
- an epoch is left to issue.

**What the third is for.** A region that nobody has reported yet has players the
coordinator knows nothing of, and nothing in section 4 can hold back for them, as
they are in no sighting. That is so after a coordinator is made anew, until the worker
of every region has registered and reported, and for a region that the list shows
and no worker runs yet. With it the rest need not be longer than the lease for K6 to
be safe. **In a cluster in which a region has no worker to run it, nothing is merged
or split by itself until it has one**: that region's players stand still meanwhile
anyway, and the first report of whoever restores it ends the wait. Evening out is not
held back by any of this (section 6).

#### 5.2 What has to hold of a region: free

A region is **free** if it has an owner that has a connection, is not leaving and is
not at fault (`Coordinator::at_fault`); it is not part of a merge or a split under
way; it is not being released; and `now` is not before its `alone_until`. It is
**nearly free** if all of that holds but that `now` may be up to `FRESH` before its
`alone_until` (section 5.6 uses this and nothing else does).

These are the reasons for which `merge` and `split` would refuse, and three more:
neither region's owner may be leaving (a leaver's regions are being moved away, and
the worker has twenty seconds), none may be at fault (a worker that just failed a
region is not given the work of a merge), and `alone_until`. So a refusal is not
expected. If `merge` or `split` refuses all the same, it is logged as a fault of this
code and the regions are left alone for a rest.

#### 5.3 Standing, and the order

**Standing.** What `decide` wants at a tick is compared with what it wanted at the
tick before, at every tick, whether or not section 5.1 lets anything begin. Each
thing wanted has `since`, the time of the first tick of the unbroken run of ticks at
which it has been wanted, and **has stood** when `since` is more than `FRESH` before
`now`.

- **A merge** is kept by its two regions, whichever of them would survive. If it was
  wanted at the tick before, it keeps its `since`; if not, its `since` is `now`; a
  merge that is no longer wanted has none.
- **A group that is to go** is kept by its region and its chunks. A group of this
  tick **continues** a group that was to go at the tick before, of the same region,
  if **every chunk of it is at most the margin from a chunk of that one**; it then has
  that one's `since`. Otherwise it is new and its `since` is `now`. A tick at which no
  split is wanted of a region forgets all its groups.

A group can continue only one group: two groups of a region are more than `D_s`
apart, and twice the margin is less than `D_s`. Two groups of this tick can continue
the same one, a group that has parted in two, and both then have its time. What the
rule is made for:

- **A group that is there for one look never goes on that look**, also not by being
  joined to a group that has stood. Players who are a group of their own only because
  the player between them and the others is missing from one report (K10, "in
  neither") are a new group at that tick. If they are counted to a group that has
  stood, because they are within `D_s` of it, that group now has chunks further than
  the margin from where it was, is new as a whole, and waits a second more. (Players
  who turn up within the margin of a group that has stood do continue it. The chunks
  named around that group cover theirs in any case, and whoever stands that near to a
  group is of it.)
- **A group in flight does stand.** Reports are a quarter of a second apart, in which
  a player who sprints in the air covers a third of a chunk, and the margin is one
  chunk at least, three with the distances of section 3. A group keyed by its lowest
  chunk would change its key more than once a second and never stand.
- **Nothing depends on an order**: whether a group continues one is a statement about
  two sets of chunks.

When in doubt the time begins anew, which costs a second: for two groups that have
come together, for a group that somebody has joined from further away than the
margin, and for a group that was the one to stay a tick ago.

Why anything has to stand: a report can be true and still mislead for as long as
another region's report is older than it. A player who is handed from one region to
another is, for one report, in both sightings or in neither (K10). Every fresh
sighting a look goes by was taken within `FRESH`, and one that is not fresh can only
hold back; so when a thing has been wanted for longer than that, every report the
first look began it on has been replaced, and it is still wanted. Whether a region
rests does not come into what is wanted, so a thing can stand while its regions rest,
and is begun when they have.

**Waiting.** A merge has a second time, `waiting`, which is for the order alone. Its
`since` is forgotten whenever the merge is not wanted for a tick, and a merge is not
wanted while one of its regions is in something else, as the sighting of a reserved
region is not fresh. By `since` alone, all the merges that wait for one region would
begin anew together each time that region has been in something, and the nearest
would be served first for ever. So: a merge that is wanted and has no `waiting` gets
`now`. At a tick at which it is not wanted, it **keeps its `waiting` if the sighting
of one of its two regions is not fresh**, and loses it if both are fresh. When one of
its regions is absorbed, its `waiting` goes to the merge of the other region with the
survivor, which takes the earlier of the two times if it has one (section 2.4); when
a region goes otherwise, it is forgotten.

**The order.** At every tick, in this order, the first three only while fewer than
`AT_ONCE` are under way:

1. **One split at most.** Of the regions of which a split is wanted that are free,
   have a group that has stood and are not passed over for their turn (below): the
   one that has the earliest `since` among its groups that have stood, and of several
   such the lowest. Only if **no split is under way**, whoever asked for it, and no
   reading of the list is asked for or owed (`reading`, `owed`). `split(now, region,
   &named, None)`, where `named` are the chunks of section 4.3 for **the groups that
   have stood, and no others**.
2. **The merges that have stood**, the one that has waited longest first (`waiting`);
   of several that have waited since the same tick the one with the smaller gap, then
   with the lower of the lower ids, then with the lower of the higher. Each if both
   its regions are free and neither is in something begun at this tick.
   `merge(now, survivor, absorbed, None)`.
3. **The absorptions** (section 4.4): the regions that are to be absorbed, highest id
   first, each with its survivor as of that moment. `merge(now, survivor, absorbed,
   None)`, noted as an absorption.
4. **`Prepare`**, for the regions that section 5.6 names.

Splits come first because they are the scarcer. Two regions that each want to merge
with a third are served in the order in which they came to want it, and the second
when the survivor has rested (K1).

**Turns.** A region is passed over in step 1 if the last merge or split of it that
ended well was a split of it, and a merge of it has stood whose two regions are free.
Per region that is one bit: whether it was split last. An absorption does not change
it. **This is still needed although a split takes every group**, for those who
arrive, not for those who leave. Without it: a region that a group leaves every ten
seconds, which is the home region of a busy world, is free at `t` = 0 with a split
and a merge both wanted; step 1 comes first and splits it; it rests until 10, by
which time another group has left and stood; it is split again, and so at 20 and 30.
The region that has waited beside it since `t` = 0 is never taken in. With it: split
at 0 (every group that has stood by then), merge at 10 (the region that has waited
longest), split at 20 (every group that has left in twenty seconds), merge at 30.
With the merges first instead, the groups would never go. Nothing queues on the side
of those who leave any more; those who arrive are taken in one in twenty seconds, in
the order in which they came (K21).

**One split at a time in the whole world**, because a split has to name the id of the
region it makes, and the id the coordinator names is not always the one the store
will give. It names the next id of the list as `listed` was last handed it. That is
stale after a split whose worker said `Ok(N)` and whose reading failed: `unlisted`
clears `reading`, nothing is owed, and the next split names `N` once more. (The first
version of this record said that waiting for `reading` and `owed` made the id right.
It does not, and `reading` does not even know of the readings the service makes by
itself.) **What the rule rests on is the runner's second try**: the store declines an
id that is not its next and says which is (`Decline::NotNext`), and the runner tries
once more with that. One try more is enough exactly when nothing else takes an id
between the answer and the second try, which is so when no other split is under way
in the world; with two at a time and a stale id, both are declined, both name the
same next id, and one of them is off. A split that somebody asks for by hand while
one by itself is under way can still do that to it, which is "not yet" (section 5.5).

**Why no split while a reading is asked for or owed**, then: not for the id. `listed`
leaves a living region it does not know out for as long as a split is reserved
(K19), and a reading is owed exactly when a split has ended without the worker's word
that it was made, and may have made a region that nobody runs. A split begun before
that reading is handed in would keep the region out until the reading after it, and
its players stand still all the while. The reading that follows a split is waited for likewise,
which as a rule also makes the id the right one and spares the second try.

#### 5.4 Rest

`alone_until` of a region is set to a rest from `now`, if that is later than what it
has,

- when the region is given an owner, or an epoch, **that the coordinator did not have
  for it**: assigned, handed over after a release, taken on a worker's word at a
  registration or after a split, or reported by its owner with another epoch than the
  coordinator had. **Not when its present owner registers again with the epoch it
  had**, which `Coordinator::report` is also called for: nothing has happened to the
  region then. A coordinator that was made anew has no owner for any region, so every
  region rests from when its worker reports it (K6);
- when a merge it survived or a split of it ends well, an absorption excepted
  (section 5.5).

Nothing the coordinator begins by itself touches a region before that: no merge by
the distances, no split, no release to even out. What somebody asks for by hand is not
held back by it, and what follows a death (a takeover) is not either; both end with
the region being given an owner or a merge or a split ending, and so with a rest. And
a leaving worker's regions are released whether they rest or not (section 6).

#### 5.5 What comes of it

The coordinator notes how each merge and split ends where it puts the `Reshaped` into
`Changes` (`end_merge`, `split_ended`, `lapse_split`), for those somebody asked for by
hand as for its own. What is said of a merge next holds of the merges by the
distances and of those asked for by hand; an absorption has rules of its own, below.

**A merge that ends well**: the survivor rests, its counters are 0, and it was not
split last.

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

**An absorption that ends well**: the survivor **does not rest**, and its counters,
and whether it was split last, are as they were.

**An absorption that comes to nothing**: the absorbed region, if it is still there, as
after any merge that comes to nothing. **The survivor is left as it was**, without a
failure counted and without being left alone. Nobody stood still on its side. An
absorption does not look at the survivor's `alone_until` anyway, so leaving it alone
would only hold back what the distances want of it, which for the home region is
everybody who arrives at the spawn point. And it is the survivor of other empty
regions as well: with it left alone for `LONG`, and then for twice that, one failure
would have every empty region of the world wait up to four minutes. As it is, each
empty region waits for its own failures only, and a survivor that cannot absorb costs
one attempt for each of them in `LONG`, then in twice that.

**If somebody came at that very moment.** When an absorption ends, well or not, the
time is noted in the survivor's `absorbed_at`. If a report that is taken of the
survivor within `FRESH` of that time has a player in it, the survivor rests from that
report: somebody came into one of the two regions as the absorption began (K13) and
has stood still for it, or for the few ticks of an attempt that failed, and the rest
is theirs.

**A split that ends well**: the region rests, its counters are 0, and it was split
last; the part rests as a region that is given an owner.

**A split that was "not yet"**: `Off(Nobody)`, `Off(NothingStays)`, `Off(Busy)`,
`Off(NotRunning)` and `Off(Declined(NotNext))`. The players had moved on, stood in a
chunk not granted yet, or had left; those who were to stay had left; the region was
in the middle of something; or another split took the id, also at the second try.
**The region rests**, as after a split that was made: such a split has stopped the
region for a few ticks, and nobody waits for a group to be split off. The next
attempt names chunks from the report of that moment, for the groups that have stood
then. **The third such answer in a row is a failure** like those below, and the
count of such answers begins anew. This is what ADR-0014's tests under bots asked of
this step (a split within a tick or two of a merge finds nobody). It cannot come of a
merge the coordinator made by the distances, as the survivor rests ten seconds; it can
of a player who stands at the rim of what the region was granted, and of a checkpoint
that was longer than the margin allows (section 4.3).

**A split that comes to nothing otherwise** (`TooLarge`, any other `Declined`,
`StoreLost`, `Overdue`, `Disowned`, `Gone`): as a merge that comes to nothing, for its
one region.

The counters go back to 0 only when a split of the region, or a merge it survived
that was not an absorption, ends well; a region that is given an owner keeps them, as
what failed need not have been the owner's doing.

#### 5.6 `Prepare` before a split

The owner of a region is told `Order::Prepare { region, epoch }`, in
`Changes::orders`, at a tick at which all of this holds, when steps 1 to 3 of that
tick are done:

- the first, second, third and fifth of section 5.1 hold (the number under way does
  not come into it);
- a split of the region is wanted at this tick, whether any group has stood or not;
- the region is nearly free (section 5.2);
- the region's `prepared` is nothing.

`prepared` is then `now`. It is forgotten when a merge or a split of the region
begins, whoever asked; when the region is given an owner or an epoch the coordinator
did not have for it, or loses its owner; and at a tick at which no split is wanted of
the region and `prepared` is more than a rest old.

**What that makes of it.** A group parts from a region that is free: `Prepare` at the
first look that shows it, and `SplitOff` when the group has stood, a second and a
look later. A region that wants a split while it rests: `Prepare` at the first tick
within a second of the end of its rest, and `SplitOff` at the end. It is not said at
every tick, and not again for a split that waits long (for the one split there is in
the world, or for a merge whose turn it is: that merge forgets it, and it is said
again before the split that follows). It is not said nine seconds early to a region
that rests nine seconds more. A group that parts and comes back, over and over, has
it said once in a rest at most. A region that is told and not split after all has
made a checkpoint it would have made at the next interval; that is all a `Prepare`
costs (see the context), and nobody notices one.

**What it does not promise.** The order can be lost with a connection. A region can
become free and have a group that has stood at one and the same tick, when a fault of
its worker is forgotten or the list is read again after the store was away: the
split is begun at that tick and `Prepare` is not said. And the checkpoint that
`Prepare` asks for can itself take longer than a second, in which case the split's
own waits behind it while the region ticks. In each of these the margin covers less
(section 4.3), a split finds fewer players than were named or nobody, and the second
attempt finds the checkpoint made. The coordinator is not told when a checkpoint is
done, and this step does not add that.

### 6. Evening out, moves by hand and workers that leave

**Evening out, when the coordinator decides by itself**, is as `even_out` has it but
for two things:

- **"Nor within a lease of a merge or a split having ended" goes.** Where regions
  merge and split all the time that would never let anything be evened out, and parts
  stay on the worker that made them. In its place: a region is not released to even
  out before its `alone_until`. That is what ADR-0014 wanted of the lease, that a part
  is not moved in the same breath, for each region by itself.
- **Which region**: of the regions of the worker with the most that are not at rest
  **and are in nothing that is wanted at that tick** (no merge of them and no split,
  whether it has stood or not), the one with the fewest players by its sighting
  (fresh or not; a region without a sighting counts as having more than any), and of
  several such the one with the highest id. If none is left, nothing is evened out at
  that tick. A move stands its region's players still, so it is the region with the
  fewest that moves; a region without players moves first and costs nobody, though it
  begins its `EMPTY_FOR` anew with its new owner. A region that a merge is about to
  stand for would otherwise be moved and merged a rest later, two stops where one
  does; and one that is about to be split would be moved with players who are about
  to leave it.

It still begins nothing while a release, a merge or a split is under way, and it runs
after what section 5 begins in the same tick: a region that is wanted for a merge and
free is reserved before it could be picked. Section 5.1 does not hold it back; it goes
by owners, not by where players are. **When the coordinator decides nothing by itself,
`even_out` is exactly as it is**, and the seven tests that say so stand.

A merge that is wanted while one of its regions is being moved waits for the move and
then for the rest of the region's new owner (K3).

**A move by hand** is refused for a region of a merge or a split under way, as today,
and not for one at rest. **A worker that leaves**: none of its regions takes part in
anything begun by itself from the moment it says so. **They are released as soon as
there is a worker to take them, whether they rest or not** (`release_for_leavers`,
which is left as it is): the worker has twenty seconds before it stops with what it
runs, and a region that stood still a moment ago stands still again sooner than the
rest allows. Those that are reserved are released when the reservation has ended, as
ADR-0014 has it.

### 7. The list, on a timer

When the coordinator decides by itself, `tick` asks for the list (`Changes::read`)
whenever no reading is asked for (`reading`) and the last answer, `listed` or
`unlisted`, is `LIST_EVERY` old or there has been none. It asks through the same
`ask_for_the_list` as everything else, so it waits for the answer before it asks
again; a timer that asked at every tick would have every reading thrown away for the
next (see the context).

**What a reading changes** is what `listed` does today and two things more: a living
region the coordinator does not know is added without an owner and assigned, unless a
split is reserved; a region that was absorbed or is no more is removed; the home
region, the absorbed pairs and the next id are noted. Besides, it notes when it last
succeeded (section 5.1), and of every living region **whether it is pinned to an
area** (`RegionInfo::pinned` is not empty), for section 4.4. The areas themselves are
not kept, and neither are the bounds of a region (`RegionInfo::bounds`): nothing here
goes by where a region's chunks are. A merge ends by a reading of the list, and that
reading has what the survivor is pinned to from then on.

**A region the list shows and no worker reports** is a region nobody runs. It is
assigned like any such region, and takes part in nothing until its owner has reported
its players and it has rested. **Until it has been reported once, nothing is merged or
split by itself anywhere** (section 5.1). If it was known before and has a sighting,
it holds nothing else back: what is wanted of other regions goes on, by what is known
of them, and its players still count where they were (section 2.4).

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
  to stay two or three. **So the owner's trial of `by-itself` with real clients comes
  with step C5**, and the roadmap is to tell the owner so where it says what to try
  after C4 (step C4.8): that `--reshape by-itself` exists, what it does on stripes,
  and that it is not what C4 asks them to judge.
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
    pub fn margin(&self) -> u32;                            // min(3, (D_s - 1) / 2)
}
```

`deploy/kubernetes/coordinator.yaml` is not changed in this step.

### 9. Every order of events that matters

`A`, `B`, `C` are regions; "the look" is a tick that decides.

**K1. Two regions each want to merge with a third.** `A` and `B` both have players
within `D_m` of `C`'s. Both merges stand. The one that was wanted first is begun (of
two wanted since the same tick the one with the smaller gap, then the one with the
lower ids); the other is passed over at that tick because `C` is in something begun.
While the merge lasts it is not wanted at all, as the sighting of a reserved region is
not fresh, and its `since` is forgotten; its `waiting` is kept, and goes with `C` to
the survivor if `C` was the one absorbed. When the merge has ended, the survivor
reports and rests ten seconds; the other merge is wanted again from that report, of
`B` and whichever region survived, has stood long before the rest is over, and is
begun then, before any merge that came to want the survivor later, however near.

**K2. A region that should be split and merged at once.** Two cases, told apart by
the clusters. (a) `A` has a group at home and a group far off, and `C`'s players are
near the far group only: the far group and `C` are one cluster, the home group
another, so `A` is surely apart: it is split, no merge with `C` is wanted meanwhile
(the places near `C` are of the group that goes), and the part and `C` merge when the
part has rested. Three sets of players stand still
once, once and twice. Merging first would have made it twice each. (b) `C`'s players
are within `D_m` of both of `A`'s groups: everything is one cluster, `A` is whole, and
`A` and `C` merge. One merge; splitting first would have been three.

**K3. A merge is wanted while one of its regions is being moved.** Evening out picked
the region before the merge was wanted; a region in something wanted is passed over
(section 6). The region is being released, so it is not free. The move ends, its new
owner is given it, it rests ten seconds and reports; then the merge is begun. If the
release is not answered in a lease, the region is taken and assigned as today, and
rests from then.

**K4. Players who go back and forth across a distance.** Around `D_m`, in two
regions: one merge, the first time a report has them within it and it stands a second;
after that they are one region until they are more than `D_s` apart. Around `D_s`, in
one region: one split, and no merge until they are within `D_m`. Across the whole
band of 8 chunks and back: a merge and a split each time, each at least a rest after
the one before. That is the worst anybody can do to a region: one stop every ten
seconds.

**K5. A group that dissolves between the decision and the order.** A split: the
players of every group named have moved out of the chunks named, or have left:
`Off(Nobody)`, "not yet". Those of one group have and those of another have not: the
split is made with whoever is still there. Some of a group have moved out: those
still there go, and the others are the stragglers of section 4.3. Those who were to
stay have all left, and the region has neither the home chunk nor a pinned area:
`Off(NothingStays)`, "not yet"; otherwise the split is made and the region is left
without players. A group has walked back towards the others: it is split off all the
same, and merged back only if it comes within `D_m`. A merge: the absorbed region's
players have walked away or left: it is merged all the same, and split again, after
the rest, if they are more than `D_s` off.

**K6. A coordinator that starts anew in the middle.** It knows no merge and no split,
has no sighting and no owner of any region, and begins nothing for a lease. It reads
the list; workers register and report what they run, and each such region rests from
then, as its owner is new to this coordinator; their players are reported within a
quarter of a second. **Until every region it knows has been reported once, it begins
nothing** (section 5.1): a region that no worker reported is assigned when the grace
period ends and is restored for some seconds, and its players are in no sighting
until then. So the rest need not outlast the lease and a restore for this to be
safe, and the end-to-end tests run with a rest shorter than that. What the workers
were in the middle of ends as ADR-0014, section 5.5, has it: a released region waits
out the grace period and is assigned, which fences an absorb not yet made; a part is
reported by its worker. If the new coordinator orders a split that the old one had
ordered and the worker is still at, the answer is `Off(Busy)`, "not yet"; if the
worker has done it, the report shows the group gone and nothing is wanted.

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
is not tried for `LONG`. Readings of the list fail; one that fails holds nothing back
by itself, and when the last one that succeeded is more than two `LIST_EVERY` old,
nothing is begun at all until one succeeds. If only the coordinator is cut off from
the store, workers go on as they are and it begins nothing.

**K9. A hundred regions, and a hundred groups.** One look compares the occupied chunks
of all of them once (section 2.5). At most four merges and splits are under way at a
time and one split. A hundred regions that all come near each other merge in pairs,
then the fifty in pairs, and are one after seven rounds of a rest each; a hundred
that each come near one and the same region, and not near each other, are taken one
every ten seconds, in the order in which they came. **A hundred groups that part from
one region within a rest go in one split**, into one region, and the region they left
stands still once. That part has a hundred groups: when it has rested it is split,
its largest group stays and ninety-nine go on into a further region, and so on, a
rest apart each: the smallest group is a region of its own about a quarter of an hour
later and has stood still once in ten seconds until then, with fewer others each time
and never with the players it left. (Open question 2 has what would shorten that.)
Each merge and split is a new routing table for every edge, with the absorbed pairs
the store keeps, 4096 at most. Regions are evened out one release at a time, as
today.

**K10. A report from before a hand-over beside one from after it.** A player walks
from `A` into a chunk `B` holds. For up to one report they are in both sightings
(`A`'s older, `B`'s newer) or in neither. In both: a place of `A` and a place of `B`
side by side, a merge wanted. It has not stood: `A`'s next report comes within the
second, or `A`'s sighting stops being fresh, and either ends the run. In neither: a
player who joined two groups of `A` is missing, and the far one is a group to go. It
is a new group at that look and has not stood (section 5.3), whatever else has: `B`
reports within the second, the player joins the groups again from there, within
`D_m` of both, and the group is no more. What remains: if `B` is silent, the group
stands and is split off, and a merge puts it right when `B` is heard again; "Risks"
has it. And a player who stands on a boundary and is handed back and forth every few
ticks can be in both reports again and again, and the two regions are then merged;
that is a merge of two regions whose boundary somebody stands on, and no harm.

**K11. A report from before a merge or a split that arrives after it.** By section
2.2 it cannot follow the worker's word of the outcome on one connection, a report
that was queued under an earlier registration is dropped, and before the word the
region is reserved and the report passed over. It can be taken where the reservation
ended without that word: when the list shows a merge done before the worker has said
so, when the word was lost with a connection and the list or the lease ended the
reservation, or when a lease has passed. The sighting is then of before: a survivor
without the players it took in, or a split region with those that went. Nothing is
begun on it, because the region rests ten seconds and a report of after comes a
quarter of a second later. What this rests on is that a report is not ten seconds on
its way; if one is, a split region and its part can be merged back and split again,
once. An absorption leaves its survivor without a rest, and there the same report
costs less still: the survivor's sighting is without players either way.

**K12. A region between two groups is reserved, silent or without an owner.** Its
sighting stays and its places go on joining the groups, so the region around it is not
"surely apart" and is not split. It is not "surely whole" either if the join was all
that held it together, so it is not merged. When the region in between reports again,
or is absorbed (its crowds go to the survivor's sighting), or is split (the part's
crowds go to the part's), the same places are there under their new region.

**K13. Players come back to an empty region as it is absorbed.** A player walks into
a chunk it holds and is handed over to it, in the tick the coordinator begins the
absorption or after. The region is released with the player in it, or with the
player's arrival kept by the edge; either way the survivor has them after the merge
(ADR-0014, sections 2.3 and 8.5). They stand still as the players of an absorbed
region do, once. The survivor's first report afterwards shows them, and it rests from
that report (section 5.5), so that nothing by the distances stands them still again
within ten seconds; they are then alone in a region that had nobody, of which the
rules make whatever the distances say. The same for a player who joins while the home
region absorbs an empty region: they wait for that merge, a fifth of a second, and
the home region rests from the report that has them. The survivor is no survivor of
anything while a player is in its reports.

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
left behind are empty and are absorbed as section 4.4 has it, half a minute after the
group has left each: by a stripe without players, or by the lowest of themselves.

**K16. A worker that leaves, and one at fault.** Their regions are not free: nothing
is begun with them by itself. A leaver's regions are released as soon as there is
somebody to take them, as today, whether they rest or not (section 6), and take part
again when their new owner has reported and they have rested.

**K17. Somebody asks by hand while the coordinator decides by itself.** `merge`,
`split` and `move_region` answer as they do: refused for a region that is reserved or
being released, done otherwise, at rest or not. What comes of it is noted like the
coordinator's own (section 5.5), and whatever the distances say of the result is
wanted afterwards: **what was asked by hand is undone when the distances say
otherwise**. A group split off by hand within `D_m` of the others is merged back when
both have rested, and two regions merged by hand whose players are more than `D_s`
apart are split again.

**K18. The owner changes between the look and the order.** The order is lost with the
connection or answered `Off(NotRunning)`; the reservation ends as `Disowned` or
`Overdue`, and section 5.5 applies. A split's order is not given again (ADR-0014).

**K19. A reading of the list between a split's record and the worker's word** shows
the part. `listed` leaves it out while a split is reserved, as it does today; that
rule was written for this timer. A region the list shows then is that split's part
as a rule; one that is not is added by the next reading after the reservation, as
ADR-0014 has it, and no split is begun by itself before that reading is in (section
5.3).

**K20. A merge that the list shows done before the worker says so.** `listed` ends
the merge, the survivor rests, and the worker's `AbsorbEnded` finds no merge noted and
has the list read once more, as today. K11 is about the reports in between.

**K21. Groups leave a region and others arrive at it, faster than one in ten
seconds.** The region is surely apart nearly all the time. Those who arrive near the
players that stay are still wanted for a merge (section 4.2), and by the turns of
section 5.3 the region is split, rests, merges, rests, and so on; its players stand
still once in ten seconds. **Nothing queues on the side of those who leave**: a split
takes every group that has stood by then, so in each twenty seconds everybody who
left goes, however many groups they are, and the part sorts itself out without the
region (K9). Until their split they are the region's players, further and further
away, and stand still with it. **Those who arrive are taken in one region in twenty
seconds, in the order in which they came**; regions that wait near each other merge
among themselves meanwhile, and wait as one with the time of whichever waited longer.
More than three arrivals a minute that are not near each other do queue, each a
region of its own beside the one it waits for, which works as between stripes. The
turns are what gives them their one in twenty seconds: section 5.3 has the sequence
without.

**K22. One group has stood, and another appears as the region becomes free.** The
home region is surely apart for a group `X` far east and is at rest; `X` has stood
for seconds. Players `q` stand far north, joined to the players at the spawn point by
a player `p` between them. At the tick the rest ends, `p` has just been handed over
to `B` and is in neither sighting (K10): at that look `q` is a group to go, larger
than `X`. It is a new group and has not stood. The split is begun for `X`, names the
chunks around `X` only, and `q` stays; for `Region::split` its players are players
who stay, and the chunks around them stay. One look later `p` is in `B`'s report and
joins `q` to the others again. The first version of this record kept one time for the
region, and would have split `q` off on that one look.

**K23. A region that nobody has reported.** After a coordinator is made anew (K6);
when the list shows a part whose worker died before it said so; when a worker dies
and nobody is there to take its regions. The coordinator knows the region and has no
sighting of it, or (the last case) a sighting that is not fresh. Without a sighting
at all, nothing is merged or split by itself anywhere until the region has been run
and reported once. With an old sighting, its players count where they were and hold
back what section 4 says; everything else goes on.

**K24. Empty regions faster than one in a rest.** K15 leaves one every ten chunks a
group walks, and off stripes every player who leaves the game away from others leaves
one. Each is due half a minute after its last player. The highest goes into the first
survivor there is; while that one is in the merge the next goes into the next
survivor; no more than `AT_ONCE` at a time. A survivor is free for the next about a
second and a half after the last (section 4.4). On stripes the survivors are the
stripes without players and, when there is none, the lowest empty part.

**K25. A worker loses its connection with a report in its queue.** A split is
ordered; the loop queues a report read before the region stopped; the connection
ends before `stay_registered` takes it; the split is made; the worker registers again
with the coordinator that still has the split noted. On the new connection:
`SplitEnded { Ok(N) }` from the watch, which ends the reservation; the report from
before the split is dropped, as it has the number of the registration before; then
`SplitEnded` from the queue, which finds no split noted and has the list read; then
reports read under the new registration. Without the number the old report was
taken, and the split region's sighting had the group again beside `N`'s for a quarter
of a second, at rest.

**K26. `Prepare`, and no split.** A group parts for a moment and comes back; or a
split waits for the one split of the world; or it is the turn of a merge. The region
has made a checkpoint it would have made at its next interval. Nothing else follows,
and `Prepare` is not said again as section 5.6 has it.

### 10. Changes to messages and types

**`clustine-rpc`**: `ToCoordinator::Players` and `PlayersOf` as in section 2.1, in
place of the pairs of step C0. It breaks the one literal in `wire.rs`. The comment of
`FromCoordinator::Prepare` says that the region is about to absorb another and that
the coordinator says it once, when it asks the other region's owner to release; it is
to say that a merge or a split is coming (section 5.6). The message is as it is.

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
    /// Before when the coordinator begins nothing with `region` by itself, if it has
    /// noted such a time. For the logs and for the tests (section 11, R5).
    pub fn alone_until(&self, region: RegionId) -> Option<Instant>;
}

impl WorkerClient { pub fn players(&self, regions: Vec<PlayersOf>); }

// policy.rs
pub struct Sighted<'a> { pub region: RegionId, pub fresh: bool, pub crowds: &'a Crowds }
pub enum Wanted {
    Merge { survivor: RegionId, absorbed: RegionId, gap: u32, why: Why },
    /// `groups`: the groups that go, each the chunks its players are in, ascending,
    /// and the groups in the order of their lowest chunks.
    Split { region: RegionId, groups: Vec<Vec<ChunkPos>>, why: Why },
}
pub enum Why { Near, Apart }
pub fn decide(
    policy: &Policy,
    enter: ChunkPos,
    home: RegionId,
    regions: &[Sighted<'_>],
) -> Vec<Wanted>;   // the splits, then the merges, each in its order
/// The chunks a split names for `groups` (section 4.3).
pub fn named(policy: &Policy, groups: &[&[ChunkPos]]) -> Vec<ChunkPos>;
```

- `CoordinatorConfig::follow` breaks the five literals of the struct: one each in
  `state.rs` and `service.rs`, two in `services/coordinator/tests/reshape.rs`, one in
  `bin/clustine/src/cluster.rs`. All five get `follow: None` in step C4.1; the last
  gets `args.follow` in step C4.5, which is where `CoordinatorArgs` gains it.
- `merge` and `split` are each cut in two: what checks and notes, which `tick` calls
  between its own `seen` and `finish`; and the public call around it, which does what
  it did. `Split` (the note of one under way) keeps the chunks that were named, and
  `Merge` whether it is an absorption. Nothing else of the two paths changes.
- `Order::Prepare` is said before a split as well (section 5.6). Its comment, which
  says that the region is about to absorb, is to say so. Nothing about it changes.
- `Service::heard` gains an arm for a worker's `Players`. `tick_interval` takes
  whether the coordinator decides by itself; the four calls of it in the service's
  test of it get that argument.
- `Changes`, `Reshaped`, `Undone`, `Order`, `FromCoordinator`, `Off`, `RegionList` and
  `RoutingTable` are as they are. What the coordinator begins by itself is in
  `Changes::releases` and `Changes::orders` like what is asked for, and how it ends in
  `Changes::reshaped` with nobody as asker, which the service logs. The coordinator's
  log also has a line for each merge, split and release to even out that is begun by
  itself, with its regions and, of a merge, whether it is an absorption: the
  end-to-end tests count from that (section 11, E8).

**`bin/clustine`**: `CoordinatorArgs` gains `follow: Option<Policy>`; the flags of
section 8; `Outcome` in `cluster.rs` gains the report, with the number of the
registration it was made under; `Reports` gains the watch in which `stay_registered`
shows the loop that number (section 2.2). The worker's log line for `Prepare`, which
says that the region is to absorb, is to say what the comments say.

Nothing changes in `clustine-sim`, `services/worldstore`, `services/edge` or
`clustine-region`, and nothing in `services/worker` but one sentence: the comment of
`Reshape::Prepare` says that a merge is coming. The three things ADR-0015, section 8,
asks steps C4 and C5 not to undo are untouched: a part holds the chunk each of its
players stands in, a stay does not leave a region without an input of its edge, and a
merge announces itself before anything the survivor says of a stay that came with it.

### 11. Building it

`main` is green at every commit. Steps 2 and 3 are the coordinator's alone
(`services/coordinator/src`), steps 4 and 5 the worker process's and the command
line's (`bin/clustine`), and the two pairs share no file once step 1 is pushed.

| # | Scope | Verified by |
|---|---|---|
| C4.1 | The contract: `PlayersOf` and `Players`; `Policy` and `follow`, `None` in all five places; `Coordinator::players`, which only counts as being heard from; the service's arm; `WorkerClient::players`; the comments of `Prepare` | Round trip; a worker that says it is not cut off, an unknown one is; every existing test |
| C4.2 | `policy.rs`: `decide` and `named` | Its own tests, and D1 to D15 below |
| C4.3 | The state machine: sightings, standing and waiting, free, rest, what comes of it, absorptions, `Prepare`, evening out, the timer, `under_way`, `alone_until`; `tick_interval` | F1 to F50 below, by somebody else; every existing test of the coordinator as it is with `follow: None`, but for the four calls of `tick_interval` in the service's test of it, which get the new argument |
| C4.4 | The worker process reports, each report under the number of its registration | Its build; C4.7 |
| C4.5 | The flags; the test cluster (`tests/common/processes.rs`) can pass a coordinator more arguments | `clustine coordinator --help`; a refusal for distances that do not fit |
| C4.6 | The generated runs R1 to R7, by somebody else | They catch faults put into C4.3 on purpose: no rest, no standing, the list never read, a survivor that is not home, a group that goes on its first look, a pinned region absorbed for being empty |
| C4.7 | End to end, E1 to E8, by somebody else, with bots that can be sent to a chunk | No bot disconnected, the ledgers, the bound counted |
| C4.8 | Roadmap, README, what to try with real clients: that `--reshape by-itself` exists and that trying it is for C5 (section 8); and that under it **what is asked by hand is undone when the distances say otherwise** (K17), so that nobody watches a split they asked for being merged back and takes it for a fault | CI |

**`decide` and `named`, from this record alone** (distances 2 and 5 unless said, so
the margin is 2; `H` is the home region, the chunk players enter in is the origin):

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
  with the lowest chunk; the other is the one group that goes, given as its chunks.
  `named` of it is the square of the margin around each of its chunks, ascending,
  each chunk once.
- D6. In the home region the group at the origin stays although the other has more
  players; and a home region whose only players are 6 from the origin is split.
- D7. Three groups: both that do not stay go, each given as its own chunks, in the
  order of their lowest chunks. `named` of both is what `named` of each has, each
  chunk once, and `named` of one alone has none of the chunks that only the other
  has.
- D8. K2 (a) and K2 (b), each with what comes out; and a region that is apart with
  another region near its group that stays: the split and the merge are both wanted.
- D9. Two groups of `A` joined by a player of `C` whose sighting is not fresh:
  nothing, neither a split of `A` nor a merge of `A` with a fresh region near it.
- D10. A region that is not fresh is in no merge and no split, whatever it holds.
- D11. Any permutation of the regions given, and of the crowds within each, gives the
  same answer (generated).
- D12. The order of the answer: splits by region id; merges by gap, lower id, higher
  id.
- D13. Players at coordinates near `i32::MIN` and `i32::MAX`: no overflow; and `named`
  for a group at the last chunk there is along an axis leaves out the chunks beyond
  it and has every other chunk once.
- D14. A region without players is in nothing.
- D15. Two sets of players of `A`, 6 apart, with a player of a fresh region `C`
  within 2 of each: one group, and a merge of `A` and `C`. Without `C`: two groups.

**The state machine, from this record alone** (a cluster as
`services/coordinator/tests/reshape.rs` builds one, with `follow`: the distances 2
and 5, so the margin is 2, and a rest of ten seconds, unless a test needs others;
ticks a `LOOK` apart; "after it has stood" is a tick more than `FRESH` after the
first that wanted it; "the list shows" is a reading handed in with `listed`):

- F1 to F6, hearing: a report from a worker that does not own the region, or with
  another epoch, changes nothing; one about a reserved region is passed over; one with
  the tick the sighting has does not make it fresh again, so a region that repeats a
  tick is in nothing after a second; an unknown worker gets `false`; with `follow:
  None` reports are heard and nothing ever follows; an owner that registers again
  with the epoch it had, a moment after its last report, keeps its region's sighting
  fresh and begins no rest, and one that reports an epoch the coordinator did not have
  begins one.
- F7 to F10, merging: nothing at the first look, `Release` and `Prepare` after it has
  stood; a tick at which it is not wanted begins the wait anew; three regions in a
  row (K1), the one in the middle at rest until both merges have stood: the merge
  that was wanted first is begun first although the other has the smaller gap, of two
  wanted since the same tick the nearer, and the other after the rest, with whichever
  region survived, before the merge of a further region that came near while the
  first merge lasted, however near; with ten pairs wanted four are begun and the
  others as those end.
- F11, each thing that holds a merge back, one test each: reserved; being released;
  no owner; an owner without a connection; either owner leaving; either owner at
  fault; at rest; a sighting more than `FRESH` old; the grace period; the list never
  read; the last good reading more than two `LIST_EVERY` old, which takes two readings
  that failed, and not when it is exactly two; a region the coordinator knows that
  has no sighting, wherever it is. One reading that failed holds nothing back.
- F12 to F15, splitting: `Prepare` at the first look that wants a split of a free
  region, and `SplitOff` after it has stood, with the chunks of D5 and the next id of
  the list; one split in the world at a time, also beside one asked for by hand; none
  while a reading is asked for or owed, and after an `Ok(N)` whose reading failed the
  next split is begun and names the id last read; a region joined by a sighting that
  is not fresh is not split (K12), and is when that region reports its players
  elsewhere.
- F16 to F20, groups: with three groups that have all stood, one `SplitOff` names the
  chunks of both that go, and after `Ok(N)` the crowds of both are `N`'s; `N`, when it
  has reported and rested, is split and the first region is not; a group that appears
  less than `FRESH` before the split is not named with one that has stood, stays in
  the region's sighting, and goes after the rest if it is still apart (K22); a group
  that moves a chunk at every tick keeps its time and is split off after it has
  stood; a group that is joined, at one tick, by players further than the margin from
  where it was begins anew and nothing of it is named at that tick; of two regions
  to split, the one whose group has gone longer is first, then the lower.
- F21 to F23, turns and `Prepare`: a region that was split last and has a merge that
  stood is merged before it is split again, and one that was merged last is split
  first (K21); no `Prepare` for a region whose rest ends in more than `FRESH`, one
  when it ends within `FRESH`, and no second one however long the split then waits;
  one again after a merge or a split of the region has begun; for a group that parts
  and comes back at every tick, one in a rest at most.
- F24 to F29, what comes of it: after a merge the survivor is in nothing for ten
  seconds and then is; after a split both are; after `Off(Nobody)` the region rests
  and is then split with the chunks of the newest report, and the third in a row
  leaves it alone for `LONG`; each `Undone` of a merge leaves both alone for `LONG`,
  the next for twice that, eight times at most, and a merge that ends well makes it
  `LONG` again; a region given an owner rests; after `Ok(N)` the crowds in the chunks
  named count as `N`'s, after a merge the absorbed region's as the survivor's.
- F30 to F38, empty regions: absorbed by the home region if that has had no players
  for more than `FRESH`, otherwise by the lowest region without players that has a
  lower id than it; not before
  `EMPTY_FOR`, not if a report had a player in between, and left if there is no
  survivor; the home region is never named to be absorbed; **a region the list shows
  pinned is never absorbed for being empty, however long, is merged by the distances
  like any other, and is a survivor**; a survivor that had a player in a report less
  than `FRESH` ago is not taken; with five due at one tick and no other region
  without players, the highest goes into the lowest and the next into the next, and
  the one in the middle waits for a survivor; with nine, four are begun; a second
  absorption into one survivor is begun when that survivor has been reported without
  players for more than `FRESH` after the first ended, not a rest after, and also
  while it rests for another reason; after one that ended well a merge by the
  distances with the survivor is begun without a rest, unless a report within `FRESH`
  of the end had a player, and then a rest after that report; after one that came to
  nothing the absorbed region is left alone for `LONG`, and the survivor is neither
  left alone nor has a failure counted.
- F39 to F43, evening out: with `follow`, a region at rest is not moved and another
  of that worker is, at once; the one with the fewest players goes first, one without
  a sighting last; a region of a merge or a split that is wanted at that tick is
  passed over, stood or not; nothing while a merge is under way; with `follow: None`
  as today.
- F44 to F47, the list: asked for every `LIST_EVERY` with `follow` and never by time
  without; not asked again while one is asked for; a region the list adds is assigned
  and is in nothing until it has reported and rested, and nothing else is begun
  until it has reported (K19 with a split reserved); whether a region is pinned
  follows the last reading that succeeded.
- F48 to F50: a new coordinator begins nothing for a lease, nothing before every
  region has been reported, and nothing with a region for ten seconds after its
  worker reported it (K6); a player in two sightings for one report merges nothing
  (K10); what is asked by hand is done at rest, and rests afterwards (K17), and a
  leaving worker's regions are released at rest (K16).

**Generated runs, from this record alone.** A model in the test: players are points
that walk, each by a script or at random towards changing goals, a step of the run
being 250 ms and a player moving at most one chunk in two seconds, some joining at
the origin and some leaving; the distances are 6 and 12, so the margin is 3; regions
are sets of players, and the model keeps of each whether it is pinned (in one
variant those it begins with other than the home region are; a part never is; a
survivor is if either of the two was); workers do what they are ordered after a
delay of zero to four steps (a merge puts the absorbed region's players into the
survivor; a split takes who stands, by the true positions of that moment, in the
chunks named, makes a region of them under the model's next id whatever id the order
named, or answers `Off(Nobody)`; `Prepare` does nothing), say what came of it, and
report every region's true crowds at every step, with a tick that goes up, behind
the word of an outcome; the store's list is what the model made; the lease is longer
than the longest delay; nothing is asked by hand. Variants add: reports that are a
step old when they are given; a player handed from one region to another with one
stale report (K10); workers that die and are replaced; a worker that leaves; readings
that fail; a coordinator made anew. Those are the **faults** below, the first
excepted. A merge that the coordinator begins is an **absorption** if the last
reports it took of both regions were without players, and **by the distances**
otherwise. Checked after every call of the coordinator:

- R1, rest: between the end of anything that involved a region (a merge, a split, an
  owner or an epoch new to the coordinator) and the next thing the coordinator begins
  with it by itself (a merge by the distances, a split, a release to even out, or its
  being absorbed), at least a rest has passed. Not held to this: a region can be made
  the survivor of an absorption at any time; after an absorption, however it ended,
  its survivor is in something again as soon as the rules have it, unless a report
  taken of it within `FRESH` of that end had a player, and then a rest after that
  report; and a leaving worker's regions are released whether they rest or not, like
  regions taken over after a death.
- R2, no player more often than the bound: take the merges, the splits and the
  releases to even out that the coordinator began by itself, each at the time it was
  begun and each for the players who were of one of its regions at that time. No
  player has more than `1 + W / rest` of them in any time `W`. Left out of the
  count: the releases of a leaving worker's regions and what follows a death, which
  keep to no rest; and for a rest after it, a player whom the model handed from one
  region to another.
- R3, nothing on what is not known: every region of a merge by the distances or of a
  split had a report taken within `FRESH` before it was begun, and so had both of an
  absorption; the home region is never the one absorbed, and no pinned region is
  absorbed in an absorption; never more than `AT_ONCE` under way; never a split begun
  while another is under way; and nothing is begun while there is a region that was
  living in a list handed in at a moment when no split was under way, is living
  still, has never had a report taken by this coordinator, was not made by a split
  whose `Ok` it was told, and has not taken in another by a list it was handed.
- R4, what is merged and what is split:
  - (a) when a merge by the distances is begun, there are two steps of the last two
    seconds, the same one it may be, a player who was truly of the one region at the
    one step and a player who was truly of the other at the other step, at most `D_m`
    apart by where each was at its step. The origin stands for a player of the home
    region at every step. (Two steps, because a player who is handed back and forth
    across a boundary is in both reports and at no single step in both regions; K10.)
  - (b) when an absorption is begun, the absorbed region is neither the home region
    nor pinned, the survivor is the home region or has a lower id, and there was no
    player in any report the model gave of the absorbed region in the last
    `EMPTY_FOR` or of the survivor in the last `FRESH`.
  - (c) when a split is begun, take the last report the model gave of the region, and
    of the players in it those who are still of that region. Those of them who now
    truly stand in a chunk named were, when the report was true, more than `D_s` from
    all those who do not; and in the home region more than `D_s` from the origin.
    (Those who have left since, or were handed on, are not looked at: a group whose
    players have all gone is still rightly asked for, and answered `Off(Nobody)`.)
  - (d) no flapping: in every run in which, from some step on, the players stand
    still, nobody joins or leaves and no fault happens, no split begun after that step
    parts two players whom a merge begun after that step had put into one region, **if
    the two were joined, when that merge was begun, by steps of at most `D_s` through
    players of the two regions it merged**, or through the origin if one of the two
    was the home region. Without the condition it does not hold of these rules: a
    region with a group that is to go can first take in a region that stands near the
    players who stay (section 4.2), and that region's players and the group are parted
    by the split that follows. The other way round does not hold either, and is not
    checked: a split can part two players whom two merges with a region between them
    bring together again. What bounds both is R5's count.
- R5, it ends. Of a run whose last fault and last hand-over are at least eight leases
  before a step `s` from which every player stands still and nobody joins or leaves.
  `D` is the longest delay of the model's workers, `X` is `rest + D + 2 s`.
  - Let `s'` be a lease, `D` and two steps after `s`: everything that was under way at
    `s` has ended. Let `Q` be the later of `s'` and the latest
    `Coordinator::alone_until` of any region at `s'`.
  - Count at `s'`, by the model's regions and the true positions, with places, links
    and clusters as in section 4.1 and every region taken as fresh: `g`, the groups (a
    region's places in one cluster; the home region has the one with the origin
    whether anybody is in it or not); `r`, the regions that have a group; `c`, the
    clusters. `n` is `(g - r) + (g - c)`: **one for every group of a region beyond its
    first**, which is one for the first split of such a region and one more for every
    time a part has to be split further, **and one for every pair of regions to be
    joined**, which in a cluster of `k` groups are `k - 1`. `e` is the number of
    regions without players that are neither the home region nor pinned.
  - From `s'` on the coordinator begins no more than `n` merges by the distances and
    splits and no more than `e` absorptions, and none of them comes to nothing.
  - Let `m` be the releases to even out that it begins from `s'` on, and `m_e` those
    of them that are of a region without players. By
    `Q + (n + m + 1) * X + (e + m_e) * (EMPTY_FOR + X)` the end is reached: any two
    players within `D_m` of each other are in one region, and so is every player
    within `D_m` of the origin with the home region; the players of each region are
    joined by steps of at most `D_s`, in the home region also to the origin; a region
    without players is left only if it is the home region, or pinned, or the home
    region has players and no other region without players has a lower id; nothing is
    reserved or being released; and no call of the coordinator says anything to
    anybody from then on, but to read the list.
  - What each term waits for: every merge and split for the rest of its regions after
    the one before, for a look, and for its worker; every absorption for its region's
    `EMPTY_FOR`, which begins anew when the region has been a survivor or was moved;
    every move for its worker and the rest that follows.
- R6, the same calls give the same answers; and crowds given in another order do.
- R7, with `follow: None` the same runs begin nothing.

**End to end, from this record alone** (`bin/clustine/tests/follows.rs`, on a cluster
as `merges.rs` starts one from `tests/common/processes.rs`: two workers, ledger bots,
view distance 8; the world divided at chunk x = 4, so two stripes that meet at block
x = 64; the coordinator started with `--reshape by-itself --merge-distance 3
--split-distance 5 --rest-seconds 5`, so the margin is 2, `EMPTY_FOR` and `LONG` are
15 s; the lease it has when it is not told any, 5 s, but 3 s in E6 and E7, as in the
tests of `merges.rs` that kill; every test audits the world against the ledgers and
fails if a bot is disconnected).

**What these numbers are chosen for.** With the merge distance 1 and the split
distance 3 that the first version had, a part and the region it was split off could
not come within the merge distance again but by one exact walk of both. A group is
split off when it is `d` chunks from whoever stays, with `d` more than the split
distance, and the nearest chunk that is not the part's is then `(d + 1) / 2` from the
group: 2 chunks at the least for the distances 1 and 3, and 3 for 2 and 4, each more
than the merge distance, so that whoever walks up to a group that stands where it was
split off steps into the part and is handed over before any merge is wanted. With 3
and 5 it is 3, and the group can stand where it is. The stripes meet at chunk 4
because a player in the east stripe at chunk 3 would be within the merge distance of
the chunk players enter in, and the stripes would be merged for that alone the moment
a bot crossed. The rest is 5 s, not 2, because the processes are unoptimised and
stand a merge's players still for more than a second (see the context); with 2 s the
bots of E3 stood still for a second or more of every three. It need not be longer
than the lease, which section 5.1 makes safe (K6).

**What the tests need to know of a split** (section 4.3 and the context): whoever
stands in a chunk named goes; a chunk goes with them if it is nearer to them than to
everybody who stays **and than to the chunk players enter in, which counts as
somebody who stays in the region that holds it, whether anybody is there or not**; a
chunk that is as near to the one as to the other stays. And of a merge: the chunk
players enter in counts as a player of the home region, so a group of another region
within 3 chunks of it is merged into the home region for that alone.

**What the tests need of the bots, which they do not have.** A ledger bot walks up and
down between two x coordinates on a lane of its own for as long as it plays, the
slowest at 0.3 blocks a tick (a chunk in under three seconds) and each further one a
sixth faster; it cannot be told to stand or to go elsewhere, and all the bots of one
scenario share the two coordinates. Step C4.7 gives `tools/botswarm` a way to change
the two coordinates of a scenario that is running. Below, **a group** is one such
scenario, with its lanes in the chunk row z = 0 (block z from 0 to 15) and apart from
the other groups' lanes; **"stands in chunk x"** is its bots walking up and down
within that chunk, between block `16 * x + 2.5` and `16 * x + 13.5`; **"walks to chunk
x"** is its coordinates being set to those, and the test waiting until every bot of
it is within them; **"walks up and down between the chunks x and y"** is the
coordinates `16 * x + 2.5` and `16 * y + 13.5`. `A` is two bots, `B` is one, `C` (in
E5) is one. `B` is one bot
because its bots would cross a boundary between two regions at different moments, and
a group that stands on both sides of a boundary for more than a second is rightly
merged across it (K10), which is not what E1 is after. All chunks below are in the
row z = 0 and are given by their x.

Where the groups stand in E1 to E3, and what is to follow. The origin is chunk 0, and
`B` stands in chunk 6 from the start to the end.

| Step | `A` | What is wanted, and the regions afterwards |
|---|---|---|
| start | stands in 0, while `B` joins and walks to 6 | nothing: the stripes, region 0 (home) with the chunks up to 3 and region 1 with those from 4; `A` and `B` are 6 apart, and `B` was never within 3 of `A` or of the origin while it was in region 1 |
| 1 | walks to 3 | a merge: 3 apart, `A` in a chunk of region 0 and `B` in one of region 1. Region 0 absorbs region 1 and holds every chunk |
| 2 | walks to 0 | a split, once every bot of `A` is in chunk 0: `B` is 6 from `A` and from the origin. It names the chunks from 4 to 8 of the rows from -2 to 2. The part, a new region, holds the chunks from 4 on; chunk 3 is 3 from `B` and 3 from the origin, and stays. Region 0 holds the chunks up to 3 |
| 3 | walks to 3 | a merge: 3 apart, `A` in chunk 3, which is region 0's and 3 from the origin. Region 0 absorbs the part and holds every chunk |

Step 3 leaves what step 1 left, so 2 and 3 are a round. On the way `A` is 4 and 5
from `B`, at which nothing is wanted in one region or in two; while one bot of `A` is
still in chunk 1, `B` is within 5 of it and is no group of its own. Beyond the row
z = 0 the part of step 2 has, of the chunks region 0 held, those with x at least 4
and z between `-(x - 1)` and `x - 1`. `A` walks only in the chunks 0 to 3, which are
region 0's throughout, and `B` stays in chunk 6, which after step 2 is the part's:
nobody is handed over after the start.

- E1. The start and step 1. Before step 1 the routing table has two regions; after it
  the list has region 1 absorbed by region 0. From the start to the end of step 1 the
  coordinator began one merge, of these two, and no split.
- E2. Step 2, and then `A` stands. One split, of region 0; the list has a new region,
  granted chunks from x = 4 on and none below. Then **the part is moved to the other
  worker** and region 0 is not: the worker that made the part has two regions and the
  other none, and of the two the part has fewer players (one against two; section 6).
  That release is begun no sooner than a rest after the split ended, by the times in
  the coordinator's log.
- E3. After E1 and E2, step 3 and then ten rounds, with the same bots. A step is done
  when the list shows what it was to bring: the part absorbed by region 0, or a new
  region. In all of E3 eleven merges and ten splits end well, and no more; what came
  to nothing is printed. Whether the part of a round
  is moved to even out before it is merged depends on how fast the bots walk, and is
  not counted.
- E4. On a cluster of its own. `A` stands in chunk 0 until `B` has walked to its
  place; then `A` walks up and down between the chunks 0 and 1 and `B` between 5 and
  6, in two regions, 4 to 6 apart and never 3, for a minute: no merge and no split.
  Then `A` walks up and down between 2 and 3: one merge, when a bot of `A` and `B`
  have been 3 apart or nearer for a second. Then `A` walks up and down between 1 and
  2 and `B` goes on as it was, in one region, 3 to 5 apart, for a minute: no split
  and no merge.
- E5. On a cluster of its own. `A` stands in chunk 0, `B` joins and walks west to
  chunk -6: one split, the part (region 2) with the chunks up to -4. `B` leaves the
  game, and `C` joins and stands in chunk 0. The list then shows region 2 absorbed,
  and **what is checked is which regions that merge was of**: region 2 into region 1,
  the east stripe, which is pinned, never had a player and has the lower id; region 0
  was in no merge. Neither of the two has a bot in it, by where the test knows its
  bots to be: `A` and `C` in chunk 0, which is region 0's.
- E6. A worker is killed at a logged moment of a merge and of a split that the
  coordinator began by itself (the worker of the survivor, of the absorbed region, of
  the split region, by the routing table of that moment), with `A` walking as in
  step 3 for the merge and as in step 2 for the split, and standing where it arrives:
  every region runs again within what `merges.rs` allows after a kill, the merge or
  the split is whole or not at all by the list, and afterwards the regions are again
  what the table above says after that step, which can take `LONG` and a rest.
- E7. The same with the coordinator killed and started again with the same
  arguments, and with the world store killed and started again.
- E8. The bound, over the ten rounds of E3 or the same again: from the coordinator's
  log, every merge, split and release to even out that it began by itself, with the
  time and the regions; from the list and where the bots were, the region each bot
  was in then. **No bot's region is in more than `1 + W / rest` of them in any time
  `W`.** Nobody is handed over and no worker leaves in these rounds, so nothing is
  excepted. And no bot waits for an acknowledgement longer than `merges.rs` allows
  for a merge or a split. No test compares a wait with "undisturbed".

On stripes a group that walks on falls back (K15); the bots of these tests stay
within the chunks of the table, where they do not.

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
- **Three leave a place in three directions at about the same time.** Those who stay
  stand still once, when the three are 30 chunks out. The three are one region then,
  though far from each other, and stand still again ten seconds later, when two of
  them go on into a region of their own, and those two once more ten seconds after
  that. Nobody who stayed notices either.
- **One leaves the game.** Nobody notices. If they were the last of their region, it
  is absorbed half a minute later by a region without players, or kept, if there is
  none or it is a stripe.
- **One joins.** Nobody else notices: the spawn point is home's whether anybody is
  there or not, and whoever was within reach of it was home's already. The player who
  joins and walks off is split off 30 chunks out, as above.

**The bound.** Between the end of anything that stood a region's players still and
the next merge, split or move that the coordinator begins with that region by itself,
ten seconds pass. A player changes region without standing still only by walking into
another region's chunks. So **a player who does not do that is stood still at most
once in ten seconds** by all of this together, for a fifth to two fifths of a second
(three and a half to six and a half times that unoptimised), and only while somebody
keeps crossing a distance. In ordinary play it is once on coming within 350 blocks of
others and once on going 480 away. Two things are outside it, as they were before
this record: **a worker that is told to stop has its regions moved away whether they
rest or not**, so their players can stand still for that move sooner than ten seconds
after the last time; and so can the players of a region whose worker died.

**What costs more than it looks:**

- **Everybody near the spawn point stands still for a fifth of a second whenever a
  group walks out of reach of it or into reach of it**, at most every ten seconds.
  That is the price ADR-0014 put on closing the links at a split, paid where the most
  players are, and it was measured with a handful of bots (see "Risks"). If the owner
  feels it, the remedy is ADR-0014's "keeping the links open", not another distance.
- **Twenty players who gather from twenty directions**: regions that are near each
  other merge in pairs, four at a time, each resting ten seconds in between. If all
  twenty are near each other, nobody is stood still more than about five times, over
  a minute; if they arrive one by one at a crowd, the crowd is stood still once in
  ten seconds for as long as they keep arriving, in the order in which they came,
  and those who wait their turn merge among themselves. Meanwhile they see each other
  across region boundaries, which works as between stripes.
- **Many groups that leave one place together** are one region at first and are
  parted one in ten seconds (K9). Each stands still for every split until it is by
  itself; the place they left stands still once.
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
  **The owner tries `by-itself` with real clients for the first time then**; the
  roadmap is to say so after C4, where it tells them what to try (section 8).
- **The single process drives the same `Coordinator`**: `tick` every
  `Coordinator::LOOK`; `players` with the status of each of its runners every `LOOK`;
  `listed` from its own store; and what `Changes` says done to its own runners, as one
  worker with one name. **Of section 2.2 it has to keep the order and nothing else**:
  after it has told the coordinator of the outcome of a merge or a split, it calls
  `players` only with statuses read after that outcome. It has no connection and no
  registration, so there is nothing to number and nothing to drop. With one worker
  nothing is ever evened out.
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
  - that there are pinned regions: the rule that excepts them (section 4.4) does
    nothing off stripes, and the survivor of an empty region is then the home region
    or the lowest of the empty ones;
  - that region ids below the number of stripes mean anything.
- **What C5 has to look at again**: `RoutingTable::layout`, which nothing here reads;
  whether one region without players that is never absorbed while home is busy is
  wanted (section 4.4), now that it holds no stripe; how the pause of a merge and of
  a split grows with the region, which its tests with crowds are to measure
  ("Risks"); and the first trial by the owner of all of the above, which on stripes is
  spoilt by K15.

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
- **A split that takes one group**, the largest, and the next when the region has
  rested: the first version of this record. Where the most players are it stood them
  still once for every group that left, and a single player who walked out of a busy
  place was passed over for a larger group at every split, for as long as larger
  groups kept leaving: still a player of that place wherever they went, stood still
  with it once in ten seconds. Taking every group stands those who stay still once
  and makes nobody wait; its price is a part that has to be split again, paid by
  those who left.
- **One time for the split of a region**, whichever group it would take: a group that
  was there for one look went if that look fell on the tick at which a region that
  had long wanted a split became free (K22). And **a group kept by its lowest chunk**:
  a group in flight would never stand.
- **Merges served nearest first, always.** A region that waits at a larger gap would
  wait for as long as others keep arriving nearer.
- **A survivor of empty regions that rests**, or that is left alone when an absorption
  fails. See sections 4.4 and 5.5.
- **Absorbing a stripe for being empty.** See section 4.4.
- **Waiting for the readings of the list so that a split names the right id.** It
  cannot be had that way; see section 5.3.
- **Telling a region to prepare whenever a split of it is wanted**, also nine seconds
  before the end of its rest, or at every tick. See section 5.6.
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
  there is a stop for all of them, as is every departure, or several departures
  together.
- Players who leave a place together in several directions are one region at first,
  far apart, and stand still once in ten seconds until each group is by itself.
- Workers say four times a second where their players are, and the coordinator looks
  at leases four times a second when it decides by itself, where it looked every
  quarter of a lease.
- A region is told to checkpoint about a second before it is split, as before it
  absorbs.
- After a coordinator has started, regions stay as they are until every one of them
  has been reported once.
- The coordinator has two ways of evening out and two ways of reading the list until
  C5 takes `by-hand` out of ordinary use.
- Regions without players that are pinned stay, and one more can stay for good.
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
   kept; and never absorbed for that if it is pinned.
6. **Section 5, "a group is split off as a region of its own"**: every group that is
   to go and has stood for a second goes, in one split, as one region, which is split
   further by the same rule. "The largest otherwise" stays for which group stays.
7. **Section 6, "reads the list ... every few seconds"**: every lease, and only when
   the coordinator decides by itself. The bounding box is not used; whether a region
   is pinned is.
8. **Section 4, "`A` survives: ... else the one with more players"**: and of two with
   as many, the lower id.

## Changes to ADR-0014, ADR-0012 and ADR-0009

1. **ADR-0014, section 5.5, evening out**: when the coordinator decides by itself, a
   region is not moved while it rests, in place of nothing being moved within a lease
   of any merge or split; the region with the fewest players is the one moved; and one
   that a merge or a split is wanted of is passed over. **ADR-0009, section 7**,
   likewise.
2. **ADR-0014, section 5.2, "reading it every few seconds ... waits for C4"**: section
   7 here.
3. **ADR-0014, section 5.5, "what C4 needs of this"**: the outcome is taken where it
   is put into `Changes`, not from `Changes`; the reason is the `Undone` and its `Off`.
4. **ADR-0014, sections 3.1 and 5.3, `Prepare` ("a merge is coming"; "the coordinator
   says it once, when it asks the other region's owner to release")**: it is said
   before a split that the coordinator begins by itself as well (section 5.6). A split
   asked for by hand has none, as before.
5. **ADR-0014, open questions 6 and 7**: sections 4.3 and 3 here.
6. **ADR-0014, "Found by the tests under bots", 2 and 3**: `Off(Nobody)` is "not yet"
   (section 5.5); a region that is being released is not free, and is waited for.
7. **ADR-0012, section 4.8, "the runner ... counts the held chunks there as well"**:
   nothing reads `RegionStatus::held` in this step either. **Open question 1** (a
   region that took in an arrival for a chunk it had given back): it is split if the
   player is far from its others, K14.
8. **ADR-0010, section 9, and ADR-0014**: step C0's `Players` changes its shape.

## Open questions

1. **Whether ten seconds is the rest the owner wants.** It is the one number a player
   feels. Longer costs nothing but regions that stay as they are for longer.
2. **Whether a part with many groups should be parted in halves.** As it is, the
   largest group stays and all the others go on, so the last of `k` groups has stood
   still `k - 1` times (K9). Naming half of the groups at each split would make it
   about the logarithm of that. It is a change to which groups a split of a region
   that is not home names, and to nothing else; two or three groups, which is what is
   expected, gain nothing by it.
3. **`AT_ONCE`** is a guess at what keeps a burst of merges within their leases; a
   merge that runs out of its lease marks a worker at fault.
4. **Whether the coordinator should refuse to start when its view distance is not the
   edges'.** Nothing tells it theirs.
5. **A hundred players in one place.** Section 4.5 leaves the rule to whoever measures
   what one thread carries.
6. **Whether a region at the far end of a long row should be split off although it is
   joined.** ADR-0010's limit.
7. **Whether a worker should say when the checkpoint that `Prepare` asked for is
   done**, so that a split waits for it and not for a second (section 5.6). Only if
   splits are seen to find nobody in regions that were built in a great deal.

Decided since the first version, and no longer open: a split takes every group that
has stood; and `by-hand` stays what the coordinator does unless told otherwise in C4,
with the owner's trial of `by-itself` in C5.

## Risks

- **The home region is where the stops are.** Every group that leaves the surroundings
  of the spawn point and every group that comes back stops everybody there. The bound
  is one in ten seconds; a busy server reaches it.
- **The pause of a merge and of a split grows with the region.** The resume after the
  links are closed makes the snapshots of every chunk the region's players see
  (ADR-0014, "What a player notices"). The pauses were measured with a handful of
  bots; the home region of a busy world is the largest region and the one that is
  merged and split most. "A fifth of a second" for everybody at the spawn point is
  not measured for that. What would show it: the waits of bots in a crowded region at
  a merge and a split, which the tests with crowds of step C5 are to measure.
- **A region that ticks less than once a second is never fresh**, so it is never
  merged and never split: the region that most needs splitting. What would show it:
  a region that stays large while its worker's ticks are long; the coordinator's log
  has no merge and no split of it.
- **Nothing is evened out while a merge or a split is under way**, and with `AT_ONCE`
  of them under way all the time (K9) that is until it is over: parts stay on the
  worker that made them meanwhile. What would show it: one worker with most of the
  regions in the routing table during a long burst.
- **A neighbour after K11.** Where a reading ends a merge before the worker's word, a
  report from before it replaces the survivor's sighting, and the absorbed region's
  players are in no sighting for one report. Nothing is begun with that region, which
  rests; but a neighbour that those players joined is surely apart for that report.
  It stands only if the worker then reports nothing for more than a second. What
  would show it: a split of a neighbour right after a merge, undone by a merge a rest
  later.
- **A report that is long on its way** is taken for new (K11). Everything else about
  age is exact; this rests on connections that deliver within the rest.
- **The order on the worker's connection** (section 2.2) is a property of how
  `cluster.rs` is written, and no test from outside can see it broken, nor that a
  report of an earlier registration is dropped. The rest hides a breach of either.
- **A checkpoint that takes longer than the second `Prepare` gives it** (section
  5.6): a split of a region in which a great deal was built finds fewer players than
  it named, or nobody, the first time. What would show it: `Off(Nobody)` in the
  coordinator's log for splits of regions with players in flight. Bots change few
  chunks and will not show it.
- **A region that no worker can run holds every merge and split back** once the
  coordinator has started anew, as it has no sighting (section 5.1). That is a world
  in need of its operator in any case; the routing table says which region waits.
- **A stale sighting that is wrong** holds a split back for as long as its region is
  silent, and a region without an owner is silent until somebody runs it. Nothing is
  begun for it; a region stays larger than it need be.
- **A split decided while a region next to it is silent** can part two groups that a
  player of the silent region, handed over a moment before, would have joined (K10's
  second half, if the other region does not report). It is put right by a merge.
- **Groups that left together stand still for each other's partings** (K9, open
  question 2). With many groups that is many times, though never with those they
  left.
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
  changing nothing, not from reading them. Whether F1 to F50 can all be driven
  through the cluster of `tests/reshape.rs` as it is was not tried.
- **The service's tests** were read for how they hand readings in and hold them back,
  not one by one.
- **The world store** was not read. That a claim of a chunk in another region's
  pinned area is answered `foreign`, on which K15 rests, is ADR-0011's sections 1 and
  3.2; `Land::keeps` was read in the simulation.
- **The edge** was read at `view_area` only, and ADR-0013 not at all. That a guest's
  subscription and a hand-over cost a player nothing they notice is what M2 and the
  roadmap say; that a region keeps a chunk while a guest watches it is ADR-0010's and
  ADR-0012's word. Of ADR-0014's contract with the edge, rule 42 was read for K13.
  What a guest sees when the links of a survivor without players are closed for an
  absorption was not looked at.
- **Whether the simulation limits how fast a player moves** was not looked for. The
  speeds in sections 3, 4.3 and 5.3 are the game's, from memory.
- **How long a checkpoint takes** in a region that was built in was not measured, and
  neither was how long the store takes over the one `Prepare` asks for.
- **The size of a report** is reckoned from postcard's encoding of the fields.
- **`bin/clustine/tests/merges.rs` and `tests/common/processes.rs`** were read at
  their heads and for the names of what they offer. **The ledger bots**
  (`tools/botswarm/src/ledger.rs`) were read for how they walk and what steers them,
  not for how the coordinates of a running scenario are best changed, nor for how
  long they take to join.
- **`deploy/`** was listed, not read.
- **That nothing but `stay_registered` takes from the queue of outcomes**, and that
  `WorkerClient`'s task sends what is queued in the order it was queued, were read in
  `cluster.rs` and `client.rs`; that `tcp::link` keeps the order was not read.
- **The single process** (`bin/clustine/src/lib.rs`), which C5 is to have drive the
  same state machine, was not read.
- **The properties R1 to R5** were gone through against the rules on paper, each
  with a correct build in mind that would fail it, and nothing was run. R4 (d) and
  the count and the bound of R5 are arguments, not measurements; whoever writes the
  runs is the first to try them.

## Review

An independent review against the code found eleven defects in the first version of
this record and nothing that endangers the world's data: the record changes nothing
in the simulation, the runner or the store. It found sound the account of the
coordinator and the service, that "free" covers every refusal, the table of `Undone`,
whole, apart and neither, that failed attempts are not retried at every tick, what
ADR-0014 relies on, the cost, the view and the numbers. What it found, and what was
decided:

1. **On stripes an empty home region absorbed every other stripe**, half a minute
   after the cluster came up, and the end-to-end scenarios began after it might have.
   A region that is pinned to an area is never absorbed for being empty (section
   4.4); the coordinator keeps from the list whether a region is pinned.
2. **Two properties of the generated runs did not hold of the rules.** A merge can be
   followed by a split that parts what it joined, where the region that merged had a
   group to go; an absorption is a merge with no player near anybody; and the bound
   on how soon it all ends counted what was wanted at one step, not what had to
   happen. R4 and R5 are written anew. Going through them once more found that the
   other half of "no flapping" does not hold either (a split, then two merges that
   bring the pair together again) and that a merge across a boundary on which a
   player is handed back and forth has, at no single step, a player on each side;
   both are in R4 as it is now.
3. **With the distances of the end-to-end tests a part and the region it left could
   not merge again** but by one exact walk. The tests use 3 and 5, with the stripes
   meeting at chunk 4, and are given in chunk coordinates with the tie rule and the
   home chunk's part said. Reading the bots for it found that they cannot stand or be
   sent anywhere, and that the first version's "a chunk in four seconds" was not
   their speed; section 11 says what step C4.7 has to add to them.
4. **Empty regions were absorbed one in a rest in the whole world.** A survivor
   without players does not rest after an absorption, its rest does not hold one
   back, and a failed one leaves it as it was; several survivors work side by side.
   Two things were added that the review did not ask for: a survivor has to have been
   without players for more than a second, so that an absorption is not begun on one
   look either, and it rests after all if a report right after the absorption shows
   that somebody came.
5. **Who was served first was decided by size and by gap, never by how long it had
   waited.** A split takes every group that is to go; merges are served in the order
   in which they came to be wanted, and that order is kept while the region they
   wait for is in something else, which the plain time of standing would not have
   done; splits are served by how long their groups have stood.
6. **A split had stood for its region, not for the group it took**, so a group could
   go on one look. Every group stands by itself, and section 5.3 says when a group of
   one tick is the group of the tick before.
7. **A test asked for what the rules did not say**: that a reading that failed holds
   a merge back. It does not; only the age of the last good one does, and the test
   says "more than two".
8. **`stay_registered` itself broke the order on the connection** on which the
   sightings rest, by saying a split's outcome from a watch ahead of the queue after
   it registers again. Reports carry the number of their registration and are dropped
   under another; the rule is claimed for one registration.
9. **The margin was said to cover the second a split has to stand, and has to cover
   the split's first checkpoint.** The owner of a region is told `Prepare` about a
   second before a split (section 5.6), and the paragraph on the margin says what it
   is for.
10. **Seven claims about the code that were not so**: the number of literals that
    `follow` breaks; a test that does change; "three times" for unoptimised builds;
    that waiting for readings makes a split's id right, which it does not, so that
    one split at a time now rests on the runner's second try (section 5.3); what the
    worker's loop does inline; which tick the crowds are of; and which calls end with
    `finish`. Each was read again in the code. In one the review was not right
    either: reading the crowds before the tick does not make them the tick's, as the
    runner stores the tick first; the record says what the report can be and why
    nothing rests on it.
11. **What the tests could not be written from**: whether the rest begins anew when
    an owner registers again (it does not, unless the owner or the epoch is new to the
    coordinator); which region is moved in E2 (the group sizes are given); "longer
    than undisturbed" (no test says it; E5 checks which regions merged and E8 the
    bound itself); chunks beyond the limits of the coordinates (left out); and that
    "left alone" is `alone_until` (said once, in section 2.3).

Its doubts were taken up as well. A rest shorter than the lease is made safe by
beginning nothing while a region has never been sighted (section 5.1). Evening out
passes over a region that something is wanted of (section 6). A leaving worker's
regions, which are released whether they rest or not, are named in the bound and in
R1 and R2. What is asked by hand under `by-itself` is undone, and the roadmap is to
say so (step C4.8). A region too large to hand to the store cannot be split either
(section 4.5). The rest are in "Risks", each with what would show it.

**The largest thing it changed** is what a split is: every group that has stood, in
one new region, and each group with a time of its own, where the first version took
one group a rest and kept one time for the region. That removed the queue of those
who leave a busy place, left the turns with one task, which is to let those who
arrive in, and made a part a region that can itself be far apart; and it is why the
properties of section 11 count groups and clusters and not what is wanted at one
step.
