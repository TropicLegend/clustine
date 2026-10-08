# ADR-0014: Merging and splitting

- Status: **Proposed**; the design of step C3 of milestone M3, phase C, for the
  simulation, the region runner, the worker process and the coordinator, and the
  contract the edge's part of the step is designed against. Not reviewed yet and not
  built.
- Date: 2026-10-08

## Context

[ADR-0010](0010-regions-that-follow-players.md) says in two pages (sections 4 and 5)
what a merge and a split are. [ADR-0011](0011-the-world-store-and-regions.md) built the
store's part: `AbsorbCommit` and `SplitCommit`, each one record of the log.
[ADR-0012](0012-the-tick-on-chunks.md) put the simulation and the runner on chunk sets
and left three risks and one open question to this step, and
[ADR-0013](0013-the-edge-without-a-layout.md) took the layout from the edge. This
record says how a merge and a split go through `crates/clustine-sim`,
`services/worker`, the worker process and `services/coordinator` when somebody asks for
one by hand. When to merge or split is step C4, the end of the stripes step C5, and
what the edge keeps and does is designed after this record, against its section 8.

What the code is today, as far as this record builds on it or changes it. Each of
these was read in the code at commit `8ef6159`:

- **The messages of C0 are there and nobody acts on them.** `RegionRunner::take_replies`
  logs `StoreReply::Absorbed`, `Split` and `Declined` as answers to what was not asked.
  `event_from` in `services/coordinator/src/client.rs` turns `FromCoordinator::Absorb`
  and `SplitOff` into an error that ends the worker's connection. `Service::heard`
  closes the connection of whoever says `Merge`, `Split`, `AbsorbEnded`, `SplitEnded`
  or `Players`. `Fanout::handle_entry` logs `Durable::Absorbed` and `SplitOff` and
  confirms them.
- **The store's merge** (`Lanes::absorb`) ends the group, declines or writes and syncs
  the record by itself, makes the absorbed region's grants the survivor's, appends its
  pinned areas to the survivor's (`Table::absorb`), loses the absorbed region's owner,
  and answers `Absorbed { absorbed, chunks }` with the grants that moved. The areas are
  not in the answer. **Its split** (`Lanes::split`) takes the next region id from the
  table itself, checks that the region holds every chunk named by grant or by being
  pinned (`Table::held_from`), writes the record, and answers `Split { region }`. The
  hello for the new region is answered by the commit thread when nothing is to be
  replayed (`Lanes::open`). A hello for an absorbed region is refused with
  `StoreError::Absorbed`, over TCP too, and `worker` in `bin/clustine/src/cluster.rs`
  ends the process on it, as on every refusal but `EpochRefused`.
- **A region's tick** is `Region::tick`; nothing else changes a `Region`. A join of a
  player the region has under the same edge is ignored; an arrival of a player the
  region has changes nothing and reports the arriving entity removed if it is another;
  a leave removes the player if they belong to the edge it came through, whatever
  their entity. What a region holds, has asked and believes (`Land::known`), its
  tickets, its loaded chunks and what it has asked of storage are not in its
  `RegionState`.
- **The runner** owns one `Region` and one store handle. It is told things from other
  threads in two ways only: links through `Links::attach`, and a release through an
  `AtomicBool` that `RegionRunner::run` looks at before every step. A release goes
  through `Phase::Preparing` (a checkpoint and a flush while it ticks), `Settling` (no
  tick, nothing taken from links, new links closed, the pending ticks published as
  their commits are confirmed), and `Closing` (a second checkpoint and a flush), and
  ends by dropping the handle and the links. The step that finds the first flush
  answered stops before it takes anything from a link, so the last tick that ran took
  everything the runner had received.
- **The hold** (`EdgeLink::hold`, `held`): after a hello, every message of the link,
  numbered or not, is put aside until each chunk the hello named is answered or cannot
  be read. A subscription outside a hello holds nothing.
- **Presence** is `Present` only after `Answer::Resumed`; after both kinds of `Unknown`
  it is `Absent` (`RegionRunner::tick`).
- **The worker process** keeps a map from region to `Phase` (`Opening`, `Running`,
  `Releasing`), opens regions with `open_region`, and drops every region that its
  orders no longer name, comparing whole `Assignment`s, `entity_ids` included.
- **The coordinator** knows the regions of its layout and no others
  (`Coordinator::new`); a holding for another region is turned away with "the layout
  has no such region". A region whose owner says `Released` is assigned at once
  (`Coordinator::hand_over`). It has no address of the world store, and nothing calls
  `clustine_worldstore::regions`. `Coordinator::routing_table` leaves `home` and
  `absorbed` empty, and `RoutingTable::is_complete` counts the routes against the
  layout.
- **The edge** takes a `Departed` whose `to` is the region it came from for an error
  and disconnects the player (`Fanout::hand_over`), and says `PlayerLeave { player }`
  to the region it believes the player to be in.

A measurement that this record leans on, made on `8ef6159` with an optimised build
(`cargo test --release`,
`players_stand_still_only_briefly_while_their_regions_are_moved_back_and_forth`, three
runs of eight moves): the pause at a move is 0.36 to 0.40 s in the middle and 0.41 to
0.47 s at worst; `clustine move` itself takes 0.25 to 0.31 s, most of it the first
checkpoint, during which the region ticks; undisturbed, a bot waits 0.06 to 0.08 s. A
debug build on the same machine: 0.90 to 0.95 s and 0.33 s.

## Decision

Words in `code` are names in the code, or will be. `A` is the region that survives a
merge or is split, `B` the region that is absorbed, `N` the region a split makes, `T`
the number of `A`'s last tick before the merge or the split, and `M = T + 1`.

### 1. The shape of it

1. **A merge and a split are each one tick of `A`, the tick `M`, in which nothing else
   happens**, worked out aside by a function that changes nothing, handed to the store
   as `AbsorbCommit` or `SplitCommit`, and taken by `A` only when the store has answered
   that the record is on disk. Until then `A` is as it was after tick `T`, and if the
   store declines, `A` ticks on from `T` as if nothing had been asked.
2. **Links are not closed**, neither by a merge nor by a split. What the edge has to
   hear is an outbox entry (`Absorbed`, `SplitOff`), published on the links there are
   as part of tick `M` and sent again in every welcome until it is confirmed, like any
   entry. ADR-0010 closed the survivor's links and had every edge resume; that is two
   resumes per merge for players about whom nothing changes. A link can still end at
   any moment, and then the same entries come in the welcome.
3. **The absorbed region is released first**, as ADR-0010 has it, and the survivor's
   worker opens it at the store with a new epoch. The new region of a split is run at
   once by the worker that split it, from memory.
4. **The hold of a resume stays as it is** (section 3.5): with an optimised build a
   move's pause is 0.4 s and most of it is the release, not the resume. One rule is
   added, because a merge puts chunks under players' hands on links that say no hello:
   **a block action waits while its link has a subscription that waits for a chunk the
   action is about.**
5. **A player's stays are ordered by their entity ids** (section 2.1). A merge and a
   split carry players from one region's state into another's without a message on a
   link, so the order of messages on one link no longer says which of two stays of a
   player is the later one. The entity id does.
6. **The store's list decides what happened.** A worker's word that a merge or a split
   is done or off only tells the coordinator to look.

### 2. The simulation

Nothing here reads a clock or keeps a hash map; every collection is ordered and every
output is in ascending order of what it names.

#### 2.1 A player's stays

A **stay** is a player's time in the world from one join to the leave that ends it; it
has one entity id for all of it, through every hand-over, merge and split. Entity ids
of players are given out by the one region that is joined, the home region, from its
one block and in ascending order (`Region::tick`: `next_entity_id` only goes up, is in
the state, and a tick that gives one out is committed before anyone is shown it). So
**of two stays of one player the one with the higher entity id is the later.**

- **A join begins a new stay whatever the region has.** If the region has the player,
  under this edge or another, the entity they had is reported removed and they enter
  the world anew. Until now a join of a player the region has under the same edge is
  ignored.
- **An arrival of a player the region has with a lower entity id replaces them**: the
  entity that was there is reported removed where it stood, and the arrival goes on as
  for a player the region does not have (ADR-0012, section 2.2, steps 2 and 3). If
  the entity id the region has for them is higher or the same, they stay as they are,
  as today, and an arriving entity that is another is reported removed.
- **A leave names the stay it ends.** `PlayerChange::Leave(EdgeId, PlayerId,
  Option<EntityId>)`: with an entity, the player is removed only if they are that
  edge's and have that entity; with none, as today, whatever their entity. An edge
  names the entity whenever it has been told one for the player (section 8, rule 37).
- **In a merge the later stay stays** (section 2.3).

Why this is needed, by the shortest case of each. *Join*: `P` leaves while `B` is
released and its leave waits for a region that takes nothing any more; `P` joins again
at home, which is absorbing `B`, and the join is taken in the first tick after the
merge, by a region that has `P` from `B` under the same edge. *Arrival*: `N` is split
off with `P`, who has left since; the leave went to `A`, which no longer has `P`; `P`
joins again and walks into `N` before the edge has read `A`'s `SplitOff`. *Leave*:
the edge then tells `N` that the stay with the old entity has ended, after the new
stay has arrived there.

#### 2.2 In its own pinned areas a region doubts what it believes

A chunk that was split off a pinned region and is given back by whoever held it is the
pinned region's again by the store's table, and nobody tells the pinned region
(ADR-0011, section 2). So:

> An arrival for, or a remote action about, a chunk of one of the region's own pinned
> areas that the region believes another region to hold is handled as if the region
> knew nothing of the chunk, and the belief is dropped in that tick.

The player is taken in, and the chunk is claimed at the end of the tick because they
stand in it; the action goes on as `Remote { action, to: None }`. Outside its pinned
areas a region answers `NotMine` as before. What the store then answers is the truth
of that moment: `granted`, and the player stays; or `foreign`, and they are let go
once more, to a region that holds the chunk and knows so.

#### 2.3 The merge: `Region::absorb` and `Region::take_absorbed`

```rust
impl Region {
    /// What this region would be after absorbing `absorbed`, whose state is `other`.
    /// Changes nothing.
    pub fn absorb(&self, absorbed: RegionId, other: &RegionState) -> Absorbing;
    /// Makes the region what `absorbing` says, with what the store answered. Only on
    /// the region `absorbing` was made from, with no tick in between.
    pub fn take_absorbed(
        &mut self,
        absorbing: Absorbing,
        chunks: &[ChunkPos],
        pinned: &[ChunkArea],
    ) -> TickOutput;
}

pub struct Absorbing {
    /// The region's whole state after the merge, as of tick `M`: what the store is
    /// handed.
    pub state: RegionState,
    /// The edges of this region that the merge reset, because the other knew them
    /// with a higher start.
    pub reset: Vec<EdgeId>,
    // and, not public: the absorbed region, the events and the entries of the tick
}
```

`absorb`, in this order:

1. The tick is `M`.
2. **Edges**, for each edge either state knows, in ascending order, with `a` this
   region's `EdgeState` and `b` the other's:
   - both know it and `b.start < a.start`: the other's side is reset as a higher start
     resets a region (ADR-0008, section 2). Its players of that edge do not come in and
     their entities are reported removed; so are the entities of the players on their
     way in `b.outbox` (`Departed`, and `NotMine` for an arrival); `b` counts as not
     there from here on;
   - both know it and `a.start < b.start`: this region's side is reset, exactly as
     `Region::drop_edge` does it, and `a` becomes `{ start: b.start, since: M, applied:
     0, sent: 0 }` with an empty outbox. The edge is in `Absorbing::reset`;
   - only the other knows it: `a` is made, `{ start: b.start, since: M, applied: 0,
     sent: 0 }`;
   - only this region knows it, or both with one start: `a` is as it is.
3. **Players** of the other state whose edge was not reset away in step 2, in ascending
   order. One this region does not have comes in whole: entity, name, pose, hotbar,
   held slot, `last_input`, `handled` and edge as they are. For one it has, the later
   stay stays (section 2.1): if the other's entity id is higher, the other's player
   takes the place of this region's, and counts as having come in; otherwise this
   region's stays. The entity that does not stay is reported removed where it stood.
   With the same entity id on both sides nothing is reported; section 2.5 says why
   that cannot be.
4. An entity is reported removed once, and not at all if a player of the merged state
   has it. Each player who came in is reported with `RegionEvent::EntitySpawned`, as an
   arrival is.
5. **Entries.** For each edge of step 2, in ascending order, one entry is added to
   `a.outbox` under the number `a.sent + 1`:

   ```rust
   Durable::Absorbed {
       region: absorbed,
       since: b.since,     // 0 if the other did not know the edge, or was reset
       applied: b.applied, // 0 likewise
       numbers,            // the numbers of `b.outbox`, ascending; empty likewise
       players,            // those of step 3 that came in and are this edge's
   }
   ```

   and behind it every entry of `b.outbox` as it is, in ascending order of its number
   there, under `a.sent + 2` and on. `a.sent` goes up by one more than there are such
   entries. `players` has each player with their whole `PlayerState`, in ascending
   order. An entry is written for an edge only one of the two knows as well: the edge
   may keep things for the other.
6. `entity_ids` and `next_entity_id` are this region's. `Absorbing::state` is the
   result.

Nothing of an entry is rewritten: a `Departed`, `Remote` or `NotMine` that names `B`,
in either outbox, stays as it is, and so does one of `B` that names `A`. An edge takes
an absorbed region for the one it went into (rule 38).

`take_absorbed`, when the store has answered `Absorbed { chunks, pinned }`:

1. Tick, players and edges become those of `Absorbing::state`. The journal stays
   empty: the whole state is in the store's record, and the tick has no delta.
2. Each of `pinned` is added to the areas the region is pinned to, if it is not among
   them. **This is how a region learns an area without being opened** (ADR-0012's first
   risk).
3. Each of `chunks` becomes `Held`, as `granted` makes it: the ticks before count as
   ticks in which it was used.
4. **Every chunk the region believes `absorbed` to hold becomes `Unknown`.** That
   region is no more. Its granted chunks became `Held` in step 3; a chunk of its
   pinned areas that it had claimed is the survivor's by the table and is claimed
   again in step 5 if it is wanted.
5. As at the end of any tick: every wanted chunk that is `Unknown` becomes `Asked` and
   is in `claims`; every `Held` chunk with a ticket that is neither loaded nor asked of
   storage is in `chunk_requests`. Nothing is returned in this tick, and no chunk's
   count of unused ticks moves.

The `TickOutput` has `tick: M`, the events of steps 2 to 4, the entries of step 5 in
`durable` with their edges and numbers, `claims`, `chunk_requests`, and a `delta` that
names the tick and nothing else and is not to be stored. Tickets, loaded chunks and
what was asked of storage are untouched: they are this region's links' and stay.

#### 2.4 The split: `Region::split` and `Region::take_split`

```rust
impl Region {
    /// What this region and a new region `part` would be if the players standing in
    /// `named` were split off, or why the split is off. Changes nothing.
    pub fn split(
        &self,
        named: &[ChunkPos],
        part: RegionId,
    ) -> Result<Splitting, NoSplit>;
    /// Makes the region what `splitting` says and hands out the part. Only on the
    /// region `splitting` was made from, with no tick in between.
    pub fn take_split(&mut self, splitting: Splitting) -> (Part, TickOutput);
}

/// Why a region is not split.
pub enum NoSplit {
    /// No player stands in a chunk named that the region holds and that is not the
    /// home chunk.
    Nobody,
    /// Nobody would stay, and the region would be left with nothing.
    NothingStays,
}

pub struct Splitting {
    /// This region's whole state after the split, as of tick `M`.
    pub state: RegionState,
    /// The new region's whole state, as of tick `M`.
    pub part: RegionState,
    /// The chunks the new region holds, ascending.
    pub chunks: Vec<ChunkPos>,
    // and, not public: `part`'s id and the entries of the tick
}

pub struct Part {
    /// The new region, as any worker would restore it from the store's record.
    pub region: Region,
    /// The chunks of the part that were loaded, as they were: what the store has of
    /// each, since nothing was unsaved.
    pub chunks: Vec<(ChunkPos, Chunk)>,
}
```

`split`, with distance counted in chunks along the longer of the two axes (ADR-0010,
section 7), worked out in 64 bits:

1. **Seeds**: the chunks of `named` that the region holds (`Knowledge::Held`), that
   are not the chunk `RegionConfig::spawn` is in, and in which a player stands. No
   seed: `NoSplit::Nobody`.
2. **Who goes**: every player standing in a seed. Everyone else stays. If nobody stays
   and the region neither holds the home chunk nor is pinned to any area:
   `NoSplit::NothingStays`; a region that would be left with nothing is not split, it
   would only get a new name.
3. **Where the stayers are**: the chunks stayers stand in, and the home chunk if the
   region holds it, whether anyone stands there or not.
4. **The chunks of the part**: every chunk the region holds that is nearer to a seed
   than to any chunk of step 3; ties stay; with nothing in step 3, every chunk it
   holds. A seed is among them, and the home chunk never is. They include chunks of the
   region's own pinned areas (ADR-0011, section 2); the region stays pinned to its
   areas.
5. **This region's state**: tick `M`; the players who go are gone; for each edge that
   has a player who goes, in ascending order, the entry

   ```rust
   Durable::SplitOff {
       region: part,
       players,  // those of the edge who go, each with their entity id, ascending
   }
   ```

   is added to its outbox under the next number. ADR-0010 has it carry how far the
   edge's messages were applied and the chunks of the part. Neither is needed: which
   of a player's stays went is said by the entity id, and what a link is told of
   chunks is said on the link (section 3.4), anew on every link.
6. **The part's state**: tick `M`; the empty block of entity ids, with `first`, `end`
   and `next_entity_id` all 0, which is what the store says of such a region (ADR-0011,
   section 2); the players who go, whole; and for each edge that has one of them an
   `EdgeState { start, since: M, applied: 0, sent: 0 }` with an empty outbox, `start`
   being this region's for the edge. No other edge is known to the part.

`take_split`:

1. Tick, players and edges become those of `Splitting::state`; the journal stays
   empty.
2. Each chunk of the part: the region believes `part` to hold it (`Foreign(part)`) if
   it wants the chunk as tick `M` ends (a viewer's ticket, or a guest's ticket in one
   of its pinned areas), and knows nothing of it otherwise. If it was loaded it is
   moved into `Part::chunks`; if it was asked of storage it no longer is. Tickets stay
   as they are.
3. `Part::region` is `Region::restore(config, splitting.part, Holdings { held:
   splitting.chunks, pinned: vec![] })` with this region's config: it holds the part's
   chunks, each counted as used up to tick `M`, knows nothing of any other chunk, and
   has no ticket and no loaded chunk.

The `TickOutput` has `tick: M` and the entries in `durable`; nothing else. No entity is
reported removed: those who go live on in the part.

So after a split `A` never thinks it holds what it does not, and believes `N` exactly
where the store has just said so; and `N` begins as a restored region does, by asking.

**What ADR-0010 said otherwise.** It has the part's players be "those standing in the
chunks named or nearer to one of them than to any chunk with a player who stays",
which defines who goes by who stays. Here those who stand in a named chunk go, and
whoever names the chunks (the command line now, the coordinator in C4) names a margin
around the group if it wants players caught who have moved a chunk since. It keeps
"the home chunk and what surrounds it within a view" with the home region; a region
does not know view distances, so here the home chunk counts as a place where somebody
stays, and what is nearer to it than to anyone who goes stays.

#### 2.5 Entity ids

The merged region keeps the survivor's block and `next_entity_id`; the absorbed
region's block is never used again, as the store never removes the file of a region
that had one and issues above the highest block it has seen. A region made by a split
has the empty block and refuses every join, by the code there is; only the home region
is joined, and it is never absorbed and never the part of a split.

**Two entities with one id in the two regions of a merge cannot be.** A block is
issued once, to one region for life; a region gives out each id of its block once (a
tick that gave one out and was lost never showed it). So an id names one stay of one
player. A stay is in at most one region's state at a time: the tick that lets a
player go takes them out of the state and puts the `Departed` in the outbox in one
commit, and the region that takes them in does so by a numbered message that is
applied once; a split takes its players out of one state and puts them into the other
in one record; a merge retires the state it reads. `absorb` therefore does not look
for one id under two players, and keeps both if it is handed such states.

#### 2.6 What is compared

`absorb` and `split` are functions of the region and their arguments; `take_absorbed`
and `take_split` of the region, the plan and the store's answer. Two things follow
that the tests of section 10 use:

- a region that plans a merge or a split and does not take it is, bit for bit, the
  region it was, and its later ticks are those of a region that never planned;
- `Part::region` equals the region made by `Region::restore` from the part's state as
  the store's record has it and the grants the store lists for it, so a part that is
  run from memory and a part that another worker restores do the same.

Against one region that holds everything and is given the same joins, inputs and
leaves, two regions that merge, and one region that splits, show the same players in
the same places with the same hotbars and the same blocks, a bounded number of ticks
after the last hand-over between them (what crosses a boundary takes its two ticks, as
today). What is not the same and is not compared: tick numbers, outbox entries and
their numbers, `since`, `applied`, and what each region knows of chunks.

### 3. The runner

#### 3.1 Commands and phases

A runner is told to reshape its region through a channel beside the flag for a
release:

```rust
pub enum Reshape {
    /// Checkpoint now and tick on: a merge is coming. Changes nothing else.
    Prepare,
    /// Absorb the region `absorbed`, which this worker has open with `absorbed_epoch`
    /// and whose state is `state`.
    Absorb { absorbed: RegionId, absorbed_epoch: u64, state: RegionState },
    /// Split the players standing in `chunks` off as the region `part`, to be opened
    /// with `as_epoch`.
    SplitOff { chunks: Vec<ChunkPos>, as_epoch: u64, part: RegionId },
}

pub enum Reshaped {
    Absorbed { absorbed: RegionId },
    Split { region: RegionId, as_epoch: u64, part: Part },
    /// Nothing came of it, and the region is as it was. `StoreLost` alone leaves open
    /// whether the store has the record.
    Off { why: Off },
}

impl Worker {
    /// Hands `reshape` to the region's thread and returns where its outcome will be;
    /// nothing comes for `Prepare`.
    pub fn reshape(&self, reshape: Reshape) -> std::sync::mpsc::Receiver<Reshaped>;
}
```

`RegionRunner::run` takes a command before a step, as it looks at the release flag;
`RegionRunner::reshape(&mut self, reshape) -> Receiver<Reshaped>` is the same for
whoever steps a runner by hand. A runner that is not `Phase::Running`, or has a
command under way, answers `Off { why: Off::Busy }` at once.

`Phase` gains what the release already has, said of a purpose:

| Phase | For a release (as today) | For a merge or a split |
|---|---|---|
| `Preparing` | a checkpoint and a flush; ticks and serves links until the flush is answered | the same |
| `Settling` | no tick; nothing taken from links; links attached now are closed; pending ticks published as confirmed | the same, but **links attached now are left where they are**, in the channel, and taken up when the region ticks again |
| `Closing` | a second checkpoint and a flush | the same |
| `Committing` | (none: the release ends here) | the plan is made and the commit sent; the runner waits for the store's answer |

`Prepare` asks for a checkpoint (`RegionRunner::checkpoint`) and nothing else, in
`Phase::Running` only.

#### 3.2 Bringing the store up to date, and what that makes true

The first flush is answered by a step that then stops **before it takes anything from
a link**, as a release does today. So when the runner stops, every numbered message it
has received is applied (`KnownEdge::received` equals the region's `applied` for every
edge), no hello waits for a tick, and the inputs of the coming tick hold nothing a
link sent. What links send from here on stays in their queues (a link holds 16 384
messages, and an edge waits when it is full).

When the flush behind the second checkpoint is answered, the store has answered
everything asked before it, in order (`Lanes::end_group` answers commits and claims
before it passes the flush on; the thread for chunks answers a load before it takes
the flush behind it; a flush behind a return is answered when the return is durable).
So at that moment:

- every commit is confirmed and published, and covered by the checkpoint: the region
  has no live commit, which the store demands (`Decline::Uncheckpointed`);
- every claim is answered, and its `granted` and `foreign` wait in the coming tick's
  inputs. **They stay there** and go into the first ordinary tick after `M` (ADR-0012,
  section 4.2: no answer is dropped by a runner that will tick again). A chunk granted
  so is not `Held` in the sim when a split is worked out, is therefore no chunk of the
  part, and is `A`'s, by the store as by the sim, one tick later;
- every return is through; no chunk the sim has given back can be in a part, as the
  sim no longer holds it;
- **no load is under way.** Every `Loaded` has arrived; those for chunks the sim had
  asked for wait in the coming tick's inputs. `RegionRunner::loads` is empty;
- no chunk is unsaved, so every loaded chunk is, block for block, what the store has.

**`NotHeld` goes on ending the runner** (ADR-0012, open question 4). A split takes
chunks from a region at a moment when the region has nothing outstanding about them: a
load asked before the split was let through by the commit thread when it took the
request, which is before the split, and answered before the flush; after the split
the sim does not hold the chunk and asks for nothing. A save of a chunk of the part is
never asked after the split either: nothing is unsaved, and the chunks have left the
sim. So `NotHeld` still means that region and store disagree.

#### 3.3 The merge in the runner

On `Reshape::Absorb`, from `Phase::Running`: `Preparing`, `Settling`, `Closing` as
above. Then:

1. `absorbing = region.absorb(absorbed, &state)`.
2. `StoreRequest::AbsorbCommit { absorbed, absorbed_epoch, tick: M, state:
   stored(&absorbing.state) }`; the phase is `Committing`.
3. **`StoreReply::Absorbed { absorbed, chunks, pinned }`**:
   1. `output = region.take_absorbed(absorbing, &chunks, &pinned)`.
   2. `committed` is `M`. For each edge the merge made known: a `KnownEdge { start,
      settled: true, received: 0, applied: 0, link: None, away_since: M }`. For each
      edge in `Absorbing::reset`: `start` is the new one, `received` and `applied` are
      0, and the link it had is closed (it is a link of a start that has been
      replaced). `last_inputs` gets every player who came in.
   3. **Every subscription of every link that was told elsewhere with `absorbed`
      waits again**, with the number it has. The region answers it like any waiting
      subscription in the ticks that follow: with a snapshot once the chunk is loaded.
      And for every answer `foreign` that names `absorbed` and waits in the coming
      tick's inputs (the store gave it before the merge), `(chunk, absorbed)` is added
      to that tick's `unbelieve`: the tick believes it and doubts it at once, by its
      order (ADR-0012, section 2.1), and asks again if the chunk is wanted. The answer
      is not dropped, as the chunk would stay asked for good.
   4. What tick `M` produced is made ready as one `HeldTick` that needs no commit, in
      the order of rule 46: for each link, the `Outbox` messages of the entries of its
      edge, in ascending order of their numbers; then a `TickDelta` with the removals
      of the tick, **to every link, whatever it is subscribed to**, and with the
      `EntitySpawned` events for the links that watch the chunk, by the subscriptions
      as step 3 left them.
   5. `Load` for each of `chunk_requests` and `Claim` for `claims`, as after any tick.
      No commit and no checkpoint: the record is the region's whole state as of `M`.
   6. The phase is `Running`; the outcome is `Reshaped::Absorbed`. The next step takes
      what the links have sent meanwhile and ticks.
4. **`StoreReply::Declined { reason }`**: the plan is dropped, the phase is `Running`,
   the outcome is `Off { why: Off::Declined(reason) }`. Nothing was said to anyone and
   no tick number was used: the next tick is `M`, with a commit like any other.
5. **The handle is lost** (`RegionRunner::give_up`): as for a lost store at any time.
   The outcome is `Off { why: Off::StoreLost }`, and whether the merge happened is for
   the store to say when the region is opened again.

#### 3.4 The split in the runner, and the part

On `Reshape::SplitOff`, likewise, and then:

1. `splitting = region.split(&chunks, part)`. If it is off, the phase is `Running`
   and the outcome `Off`, with `Off::Nobody` or `Off::NothingStays` as the sim says.
2. `StoreRequest::SplitCommit { tick: M, state: stored(&splitting.state), part:
   SplitPart { chunks: splitting.chunks, state: stored(&splitting.part) }, as_epoch,
   region: part }`.
3. **`StoreReply::Declined { reason: Decline::NotNext { next } }`**, the first time:
   the plan is made again with `next` as the part's id and sent again. The store gives
   out region ids, and another split can have taken the one the order named. A second
   `NotNext` is a decline like any other.
4. **`StoreReply::Split { region }`**:
   1. `(part, output) = region.take_split(splitting)`.
   2. `committed` is `M`; `last_inputs` loses the players who went.
   3. Of the chunks that storage has delivered and that wait for the coming tick,
      those of the part are moved into `part.chunks`. Chunks of the part that the
      store could not read are no longer noted as such here.
   4. **Each link's subscription to a chunk of the part that waits or is served is
      answered** (rule 44): a viewer's with `Elsewhere { chunk, ask, region }`, and it
      is told elsewhere from now on, its ticket staying; a guest's with `NotMine {
      chunk, ask }`, and it is forgotten, its ticket taken back with the coming tick. A
      subscription that was told elsewhere before is left as it is.
   5. Tick `M` is made ready as a `HeldTick` that needs no commit: for each link the
      `Outbox` message of its edge's `SplitOff`, if it has one, and then the answers of
      step 4 in ascending order of the chunks.
   6. The phase is `Running`; the outcome is `Reshaped::Split { region, as_epoch,
      part }`.
5. Another decline, or a lost handle: as for a merge.

**A runner for the part**: `RegionRunner::of_part(part: Part, store: StoreHandle) ->
RegionRunner` is a runner for `part.region` as `with_store` makes one, with
`part.chunks` kept as **warm chunks**. When a tick of this runner asks storage for a
chunk that is warm, the runner hands it to the next tick in `chunks_loaded` instead of
asking the store, and forgets it. A warm chunk is also forgotten when the region gives
the chunk back (`TickOutput::returns`), and goes with the chunk into `Part::chunks` if
a later split takes it. That is safe because only the holder saves a chunk: from the
split until this region loads the chunk, nothing can have changed what the store has
of it. The sim cannot tell a warm chunk from one the store delivered a tick later.

#### 3.5 The hold

**A resume holds what it holds today**: everything the link sends behind its hello,
until each chunk the hello named is answered or cannot be read. ADR-0009 left open
whether to hold only what acts on chunks that are not there yet, if the pause at a
move were above about a second. It is 0.4 s with an optimised build, and of that the
resume is the smaller part (see the measurement in the context). Holding less would
change what every test of the hold asserts and what section 4.5 of ADR-0012 promises
("nothing an edge sends again after a restore reaches a tick before the region knows
of every chunk its players can reach who holds it") for a tenth of a second. It is
left as it is, for moves, and for the first link to a region a split made, which
begins with a hello like any link.

**One hold is new**, and applies to every link at all times:

> A numbered message that acts on a block is not passed into a tick while its link has
> a subscription that waits for a chunk the message is about, unless the store has
> said that it cannot read that chunk. Everything the link sent behind it waits with
> it, numbered or not.

The chunks a message is about: for `Input` with `Dig`, the chunk of the block; with
`UseItemOn`, the chunk of the block and that of the spot beside the clicked face; for
`Remote`, the chunk of every position its step names (`position`; `against` and
`target`; `target`). No other message is about a chunk for this rule: not a move, an
arrival, a join or a leave.

It is the same queue as the hello's hold (`EdgeLink::held`): a link is held while a
chunk of its hello is unanswered, or while the first message of its queue is such an
action. `RegionRunner::drain` and `release_held` take messages in order until they
meet one that has to wait.

Why a merge needs it. The survivor's links are not closed, so no hello names the
chunks that came with the absorbed region, and those are on screens already: the edge
has them from the absorbed region. The edge asks the survivor for them
(`Subscribe`) and sends on what the absorbed region's players did meanwhile; without a
hold, a dig would reach a tick in which the chunk is held and not yet loaded, and be
acknowledged without effect. The same holds for a `Remote` that was on its way to the
absorbed region.

What it costs in ordinary play: a block action about a chunk that the link asked for
at most two ticks ago and that is on the client's screen through another region. That
is the click behind one's back right after a hand-over (ADR-0012, section 2.3): it now
waits the one or two ticks the store takes and goes on with the holder named, where
it went on at once without one; and what that edge sent this region behind it waits
as long. A subscription that waits is always answered (ADR-0012, rule 9), so nothing
waits for ever.

#### 3.6 Presence after a welcome that tells the edge again

A presence answer is `Present` if the state has the player under the edge and the
welcome is `Resumed` **or `Unknown` with the `since` the state already had**
(`Answer::ToldAgain`: the state knows the edge with the hello's start and another
`since`, and nothing has been received). It is `Absent` after an `Unknown` that makes
or resets the state, as today. This is what ADR-0012 left to this step: a part's
state, and a survivor's state for an edge it came by through the merge, know the edge
with players and with nothing received.

#### 3.7 Release and stop in the middle

- **A release that is asked for while a merge or a split is under way waits for it to
  end.** `begin_release` does nothing outside `Phase::Running`, and `run` asks again
  before every step, which is the code there is.
- **A runner that is stopped in any phase but `Running`** lets go of the region as it
  is, without waiting for the store (`Ended::Abandoned`), as one stopped in a release
  does today. Nothing was published that the store had not confirmed; if the commit of
  a merge or a split was on its way, the next owner finds the region before it or
  after it.
- **A checkpoint by the clock** (`checkpoint_interval`) is made only by ordinary
  ticks. The first one after `M` names a tick above `M`, which is what the store puts
  in place over the record (ADR-0011, section 3.5).
- **`RegionStatus`** is brought up to date after tick `M` as after any tick.

### 4. The worker process

`FromCoordinator::Prepare`, `Absorb` and `SplitOff` become `WorkerEvent`s; `Orders`
and `Release` are as they are. In `worker` (`bin/clustine/src/cluster.rs`):

**Assignments are told apart by region and epoch**, not by `entity_ids`, wherever
orders are compared with what the worker holds (`ordered`, `declined`, `let_go`).
Nothing reads `Assignment::entity_ids` (the roadmap has it down for removal), and a
region made by a split is run before any orders name it.

**`Prepare { region, epoch }`**: if the worker runs that region with that epoch,
`Reshape::Prepare` to its runner. No answer.

**`Absorb { region, epoch, absorbed, as_epoch }`**:

1. If the worker does not run `region` with `epoch` in `Phase::Running`, or that
   region is reshaping: if it is this very merge (the same `absorbed` and `as_epoch`),
   nothing; otherwise `AbsorbEnded` with `Err(Off::NotRunning)` or `Err(Off::Busy)`
   at once.
2. `Reshape::Prepare` to the runner, and at the same time the region `absorbed` is
   opened at the store with `RegionHello { region: absorbed, epoch: as_epoch, layout
   }`, by `open_region`, which tries again while the store cannot be reached.
3. The hello's answer:
   - `Ok((handle, restored))`: `clustine_worker::absorbable(&handle, restored) ->
     Result<RegionState, RestoreError>` reads the state as `restored_state` does; if
     `restored.deltas` is not empty (the absorbed region's owner lost the store on its
     way out instead of finishing its release), it asks `Checkpoint { tick:
     restored.tick(), state }` and a flush on that handle and waits for the flush. The
     store has put the block changes of those commits into the chunks before it
     answered the hello, so the checkpoint needs no save. Then `Reshape::Absorb {
     absorbed, absorbed_epoch: as_epoch, state }` to the runner. **The handle is kept
     open until the outcome is there**: the store declines a merge whose absorbed
     region has no owner with that epoch;
   - `Err(StoreError::Absorbed { into })` with `into == region`: the merge has
     happened already (the order came twice). `AbsorbEnded { outcome: Ok(()) }`;
   - `Err(EpochRefused { seen, .. })`: somebody else has been given the region since.
     `AbsorbEnded { outcome: Err(Off::Refused) }`, and `EpochRefused` as today;
   - any other error, a `RestoreError` included: `Err(Off::Unreadable)`. The worker
     does not end over a region it was only to absorb.
4. The runner's outcome: the handle of `absorbed` is dropped (the store has lost its
   owner if the merge happened; if not, dropping it leaves the region without an
   owner, which the coordinator is told next), and `AbsorbEnded { region, absorbed,
   outcome }` is said.

The absorbed region is never among the worker's regions: it is not vouched for, not
served to edges and not reported at a registration.

**`SplitOff { region, epoch, chunks, as_epoch, part }`**:

1. If the worker does not run `region` with `epoch` in `Phase::Running`, or it is
   reshaping: `SplitEnded { region, as_epoch, outcome: Err(..) }` at once.
2. `Reshape::SplitOff { chunks, as_epoch, part }` to the runner.
3. `Reshaped::Split { region: n, as_epoch, part }`:
   - `SplitEnded { region, as_epoch, outcome: Ok(n) }` is said at once;
   - `n` is added to the worker's regions in a new `Phase::Starting { held, part,
     opening }`, with `held` being an assignment of `n` with `as_epoch`, a hello for it
     with the layout's fingerprint, and the config of `region`. It is vouched for as
     `WaitingForStore` and reported at a registration like any region the worker
     holds;
   - when the hello is answered, `RegionRunner::of_part(part, handle)` is spawned and
     the phase is `Running`: edges are let in. What the hello's `Restored` says is not
     looked at. `EpochRefused` drops the part, as it drops any region;
   - **`n` is not dropped for being absent from the worker's orders until orders have
     named it once**; and while that is so, the worker says `SplitEnded { .. Ok(n) }`
     again after every registration. Orders that were on their way when the split
     happened do not know of `n`.
4. `Reshaped::Off { why }`: `SplitEnded { region, as_epoch, outcome: Err(why) }`.

**`StoreError::Absorbed { into }` at any opening** no longer ends the worker: it drops
the region, never takes that assignment up again, and says `AbsorbEnded { region:
into, absorbed: region, outcome: Ok(()) }`, which makes the coordinator read the list.

**Orders that take a region away while it reshapes**, and a worker that stops: the
runner is stopped as it is (section 3.7), and a handle held for a region to absorb is
dropped.

**The single process** (`bin/clustine/src/lib.rs`) has no coordinator and gives no
such orders. Nothing in it changes but what the messages of section 9 change in what
it constructs. Its regions run the runner of this record: the new hold applies there,
and a leave names its entity.

### 5. The coordinator

#### 5.1 Asking by hand

```text
clustine merge --survivor A --absorbed B [--coordinator host:port]
clustine split --region A --chunks X,Z [X,Z ...] [--coordinator host:port]
```

Each connects, says `ToCoordinator::Merge { survivor, absorbed }` or `Split { region,
chunks }`, and waits for one answer, `FromCoordinator::Asked`, after which the
coordinator closes the connection: `Ok(region)`, with the survivor or the new region,
or `Err(reason)` in words, at once if the coordinator refuses and otherwise when it
knows what came of it. The connection is not closed for being silent meanwhile, as a
mover's is not. The commands print the answer and the time from asking. `--chunks`
takes chunk coordinates (a block's coordinate divided by 16, rounded down), each pair
`x,z`.

`clustine coordinator` gains `--store host:port`, the world store, with the default
the worker has; `deploy/kubernetes/coordinator.yaml` and what the README and the
roadmap tell the owner to type get it.

#### 5.2 The list of regions

The service reads the store's list with a function it is handed (`serve(listener,
config, lists)`, where `lists: Fn() -> io::Result<RegionList>` is
`clustine_worldstore::regions` over the store's address in the binary and whatever a
test wants in a test), in a task of its own, never in the task that owns the
`Coordinator`. A reading comes back like something a client said, and is passed to
`Coordinator::listed(now, &list)`. Readings are asked for one at a time and applied in
the order they were asked.

**When it is read** in this step: when the service starts; when a worker registers;
before a merge or a split that was asked for is looked at (the request waits for that
reading, and is refused if it fails); when a worker says `AbsorbEnded` or
`SplitEnded`; and when a reservation ends without the worker's word (section 5.4).
Reading it every few seconds, as ADR-0010 has it, waits for C4, which also needs the
bounds and where the players are; nothing in C3 depends on hearing of a change that no
worker reported and no reservation covered.

**What `listed` does:**

- a living region of the list that the coordinator does not know is added, without an
  owner, and assigned like any such region (it is one nobody runs: the part of a split
  whose worker died before saying so);
- a region the coordinator knows that the list has among `absorbed`, or that is below
  `RegionList::next` and neither living nor absorbed, is removed: its owner, if it has
  one, loses it, and a reservation that names it ends. A region at or above `next` is
  left alone: the reading is older than the split that made it;
- the home region and the absorbed pairs are noted for the routing table.

The regions of the layout are known from the start, as today, so a coordinator that
cannot reach the store yet runs the stripes as it does now. **A region that a worker
reports at a registration and that the coordinator does not know is taken as living**,
with that worker as its owner, until a reading says otherwise: it is a part whose
`SplitEnded` an earlier coordinator, or nobody, heard. The store fences whoever is
wrong.

#### 5.3 A merge

`Coordinator::merge(now, survivor, absorbed, asker)` refuses, changing nothing, with
the first of these that holds:

| Refusal | When |
|---|---|
| `NoSuchRegion(r)` | `r` is not a region the coordinator knows |
| `Same` | survivor and absorbed are one region |
| `Home` | `absorbed` is the home region of the last reading |
| `Reserved(r)` | `r` is part of a merge or a split under way |
| `BeingReleased(r)` | `r` is being released, for a move, a leaver or to even out |
| `NoOwner(r)` | `r` has no owner |
| `Unfit { worker, why }` | the survivor's owner has no connection or is leaving; the absorbed region's owner has no connection |

Otherwise it notes the merge: both regions, the survivor's owner and epoch, the
absorbed region's owner and epoch, the time, who asked. **Both regions are reserved**
from now until the merge ends: neither is moved, released for a leaver or to even out,
split, merged with a third, or taken from its owner for want of vouching, and the
absorbed region is not assigned while it has no owner. It then tells, through
`Changes`:

1. the absorbed region's owner `Release { region: B, epoch }`, as for a move with
   nobody as target, and the survivor's owner `Prepare { region: A, epoch }`;
2. on `Released { region: B, epoch }` from that owner (or its registering without
   `B`, as ADR-0009 has it): `B` is taken from it and **not assigned**. The
   coordinator issues `as_epoch`, above every epoch it has issued or heard, notes it
   as `B`'s epoch, and tells the survivor's owner `Absorb { region: A, epoch,
   absorbed: B, as_epoch }`. If that owner registers again while the merge is at this
   stage, it is told again; the order can be taken twice (section 4);
3. on `AbsorbEnded { region: A, absorbed: B, .. }` from the survivor's owner: the list
   is read, and `listed` ends the merge by what it finds. `B` absorbed: `B` is gone,
   the asker is told `Ok(A)`. `B` living: the merge is off; `B` has no owner (the
   worker has let go of it) and is assigned at once, as a region its owner let go of
   is; the asker is told `Err` with the worker's reason. If the list cannot be read,
   it is read again at every tick until the merge's time is up.

**The merge's time is one lease from when it was asked**, for all of it. When it is
up, or when the survivor loses its owner or its epoch changes, or the absorbed region
loses its owner otherwise than by step 2, the reservation ends:

- at stage 1, the release is an overdue release of ADR-0009: `B` is taken from its
  owner, which is noted as having failed it, and assigned;
- at stage 2, the list is read first. `B` absorbed: the merge happened and the asker
  is told so. Otherwise `B` is assigned like a region its owner let go of, with an
  epoch above `as_epoch`, which fences a survivor's worker that is still at it: its
  `AbsorbCommit` is declined (`Decline::NotOpened`). If the list cannot be read
  either, `B` is assigned all the same; should it have been absorbed, the worker that
  is given it is refused by the store and says so (section 4), and that reads the
  list.

A worker may answer `AbsorbEnded` for a merge the coordinator has no note of (it
started anew, or gave up): it reads the list.

#### 5.4 A split

`Coordinator::split(now, region, chunks, asker)`, after a reading of the list, refuses
with `NoSuchRegion`, `Reserved`, `BeingReleased`, `NoOwner`, `Unfit` (the owner has no
connection or is leaving) or `NoChunks`. Otherwise it reserves the region as above,
issues `as_epoch`, and tells the owner `SplitOff { region: A, epoch, chunks, as_epoch,
part }` with `part` the `next` of that reading. The order is not sent again if it is
lost: a split that is done twice makes two regions.

- On `SplitEnded { region: A, as_epoch, outcome: Ok(n) }` from that owner: `n` is a
  region with that worker as its owner and `as_epoch` as its epoch, vouched for as a
  new assignment is; the reservation ends; the asker is told `Ok(n)`; the list is
  read.
- On `Err(why)`: the reservation ends, the asker is told `Err`; the list is read,
  because `Off::StoreLost` leaves open what happened.
- **When a lease has passed** since it was asked, or the region loses its owner or
  changes its epoch: the reservation ends and the list is read. A new region that it
  shows is one nobody runs, and is assigned; the asker is told `Ok` if `part` is among
  the living and `Err` otherwise.

A `SplitEnded` with an `as_epoch` the coordinator has no reservation for is taken as a
registration that reports the region is: `n` is the worker's unless the coordinator
knows another owner of it.

#### 5.5 Everything else it keeps straight

- **Evening out** begins no release while a merge or a split is under way, nor within
  one lease of one having ended: a part is on the worker that made it, which then has
  one region more, and moving it in the same breath would stand its players still
  twice.
- **Vouching.** A reserved region counts as vouched for. When the reservation ends,
  the survivor, the split region and the part count as vouched for at that moment.
- **A leaving worker's** reserved regions are released when the reservation has ended.
- **The routing table** names the home region and the absorbed pairs of the last
  reading (`home`, `absorbed`), lists a route for every region with an owner, the
  parts among them, and gains `waiting: u32`, how many regions the coordinator knows
  have no owner; `is_complete` is `waiting == 0`. Its version goes up whenever a
  route, the home region or the pairs change.
- **A coordinator that starts anew in the middle** knows nothing of the merge or the
  split. It reads the list, finds the regions as they are, before or after, and hears
  from the workers what they run. A released `B` is reported as let go and assigned
  at once (ADR-0009), which fences the absorb if the record is not written yet; a
  part is reported by the worker that runs it, or found in the list and assigned
  after the grace period. Nothing is kept on disk.
- **What C4 needs of this**: `merge` and `split` with nobody as asker; their outcome
  in `Changes`, with the reason as an `Off` and not in words, to leave a region alone
  for a while or to name other chunks; the list read on a timer; and the chunks of a
  split named with a margin (section 2.4).

### 6. The merge, step by step

| # | Who | What | Durable after it |
|---|---|---|---|
| 1 | coordinator | reads the list; reserves `A` and `B`; `Release` to `B`'s owner, `Prepare` to `A`'s | nothing new |
| 2 | `B`'s worker | releases `B` (ADR-0009): checkpoint while ticking, stop, last checkpoint, handle and links closed; `Released` | `B`'s state file as of its last tick; its chunks |
| 2a | `A`'s runner | a checkpoint while it ticks (`Prepare`) | most of `A`'s changed chunks |
| 3 | coordinator | takes `B` from its owner, assigns it to nobody; `Absorb` with `as_epoch` | |
| 4 | `A`'s worker | opens `B` with `as_epoch`; checkpoints it if its log is not empty; `Reshape::Absorb` | `B`'s highest epoch; perhaps its state file |
| 5 | `A`'s runner | checkpoint and flush while ticking; stops; publishes what is confirmed; second checkpoint and flush | `A`'s state file as of `T`; all its chunks |
| 6 | `A`'s runner | `absorb`; `AbsorbCommit { tick: M }` | |
| 7 | store | ends the group; declines, or appends and syncs `Absorbed`; then table, `B`'s owner lost, `B`'s files removed; answers | **the merge**, when the record is synced |
| 8 | `A`'s runner | `take_absorbed`; the entries and removals to the links; loads and claims; ticks on | |
| 9 | `A`'s worker | drops `B`'s handle; `AbsorbEnded` | |
| 10 | coordinator | reads the list; `B` is gone; routing table with the pair; answers the asker | |
| 11 | edge | handles `Absorbed` on the link it has, or in the next welcome | |

`B`'s players stand still from the end of step 2's first checkpoint until step 11;
`A`'s from the stop in step 5 until step 8. Who may run out of time: the coordinator
alone, one lease after step 1. A worker waits for the store as long as the store
neither answers nor closes, as it does for any commit; the coordinator's lease ends
that from outside.

**A kill, and what is found.** "As before" is: `A` and `B` both living, each as its
state file and log have it. "As after" is: `A` with the merged state as of `M`, `B`
absorbed.

| Killed | When | Found | Put right by |
|---|---|---|---|
| coordinator | any time | as before or as after, whichever the workers reach | the new coordinator, by the list and the registrations (section 5.5). If `B` is assigned before step 7, the absorb is declined |
| `B`'s worker | in step 2 | as before; `B` without an owner, its log perhaps not empty | the merge's time runs out, or the worker's lease: the merge is off, `B` is assigned and restored like any region whose owner died |
| `A`'s worker | steps 2a to 6, before the record is synced | as before; `B` without an owner (the store closes both handles) | the worker's lease runs out, which ends the reservation: the list is read, `B` and `A` are assigned. `B`'s state file may be newer by step 4, which changes nothing |
| `A`'s worker | after the record is synced, before or after the store's answer, before or after step 8 | as after | the lease runs out; the list is read before `B` is given to anyone and shows it absorbed; `A`'s next owner is restored with the merged state, and the edges get `Absorbed` in its welcome |
| store | before the record is synced | as before; every handle lost | the workers open their regions again; `A`'s runner says `Off::StoreLost`; `B` is assigned when its hello can be answered |
| store | after | as after; every handle lost | the same; the worker that is given `B`, if any, is refused with `Absorbed` and says so |
| store, in step 7 | at any write or sync | as before or as after, as a whole | ADR-0011, section 4.3 |
| `A`'s runner stopped (orders took `A`, or the process stops) | steps 5 to 8 | as before or as after | whoever runs `A` next |
| edge | any time | its players are gone, as today | |

In every row `A`'s players and `B`'s are in exactly one living region's state, with
what they did up to that region's last confirmed tick, and what they did since is
with the edge.

### 7. The split, step by step

| # | Who | What | Durable after it |
|---|---|---|---|
| 1 | coordinator | reads the list; reserves `A`; `SplitOff` with `as_epoch` and `part` | |
| 2 | `A`'s runner | checkpoint and flush while ticking; stops; publishes; second checkpoint and flush | `A`'s state file as of `T`; all its chunks |
| 3 | `A`'s runner | `split`, or off; `SplitCommit { tick: M, region: part }` | |
| 4 | store | ends the group; declines, or appends and syncs `Split`; then table, `N`'s lane and region file; answers | **the split**, when the record is synced |
| 5 | `A`'s runner | `take_split`; `SplitOff`, `Elsewhere` and `NotMine` to the links; ticks on | |
| 6 | `A`'s worker | `SplitEnded { Ok(N) }`; says hello for `N` with `as_epoch`; runs the part when the store has answered | `N`'s highest epoch, which it had by the record |
| 7 | coordinator | `N` is the worker's; routing table with a route for `N`; answers the asker; reads the list | |
| 8 | edge | handles `SplitOff`; links to `N` when the table names it; hello, welcome, the part's chunks from memory | |

Everyone in `A` stands still from the stop in step 2 until step 5; those who went
until step 8.

| Killed | When | Found | Put right by |
|---|---|---|---|
| coordinator | any time | before or after | the new one: `N` is reported by its worker, or found in the list |
| `A`'s worker | before the record is synced | as before: `A` as its state file has it, no `N` | the lease; `A` is assigned and restored |
| `A`'s worker | after it, before step 6 or in it | as after: `A` and `N` in the list, nobody runs either | the lease ends the reservation; the list shows `N`; both are assigned and restored from the record, `N` from its part. `A`'s welcome has `SplitOff` |
| `A`'s worker | after step 6 | as after | the lease, as for any worker with two regions |
| store | before the record is synced | as before; handles lost | `A` is opened again; `Off::StoreLost`; the list shows no `N` |
| store | after | as after; handles lost; the part in the worker's memory is dropped with the runner if the answer never came | `A` is opened again; `Off::StoreLost`; the list shows `N`, which is assigned and restored from the record |
| store, in step 4 | at any write or sync | before or after as a whole; `N`'s region file is written at the next start if it is missing | ADR-0011, section 4.3 |
| store | between the answer and the hello for `N` | as after | the worker says hello for `N` until it is answered; the part waits in memory, vouched for as waiting for the store. If the worker dies too, `N` is restored from the record |
| `A`'s runner stopped | steps 2 to 5 | before or after | whoever runs `A` next; an `N` nobody runs is found in the list |

A part is never published from before its hello is answered, so a part that is lost
with its worker's memory has shown nobody anything that the record does not have.

### 8. The contract with an edge

This continues section 5 of ADR-0012, as ADR-0013 changed it, with rules numbered on
from 33. It is what the edge's part of this step is designed against. "The region" is
the one that says a thing, "the link" the edge's link to it.

#### 8.1 The hold

34. **A block action is judged only when the link it came on has no subscription that
    waits for a chunk the action is about** (section 3.5). It, and everything the edge
    sent on that link behind it, waits until that subscription is answered: with a
    snapshot, `Elsewhere` or `NotMine`. So an edge that wants an action judged on a
    chunk the region is still loading **subscribes on that link before it sends the
    action**; an action about a chunk the link has no subscription to is judged at
    once, as today. The hold behind a hello is as it was (rule 16).

#### 8.2 Stays

35. **A player's stay is known by its entity id, and the higher id is the later
    stay** (section 2.1). A join begins a new stay and replaces whatever stay of that
    player the region has. An arrival replaces a stay with a lower id and is passed
    over, its entity reported removed, where the region has a higher one.
36. **A region can have a stay that the edge has given up, or does not know to be
    there**: it comes with a merge or a split. Rules 41 and 45 say how the edge hears
    of it.
37. **`PlayerLeave { player, entity }`** names the entity of the stay it ends, whenever
    the edge has been told one for that stay (by `Spawned`, a presence answer, or an
    entry); `None` only for a player who quit before that. A leave that names an
    entity changes nothing at a region that does not have that stay, so an edge that
    gives a stay up without being sure where it is may say so to more than one region.

#### 8.3 Regions that are no more

38. **An absorbed region stands for the region it went into**, wherever a region
    names it: in `Departed::to`, `Remote::to`, `NotMine::holder` and
    `Elsewhere::region`, and through several merges in a row. The edge learns what
    went into what from the `Absorbed` entries it handles and from the routing table.
    **If that makes the place an entry sends something the region the entry came
    from, the edge sends it there**: a `Departed { to: B }` from `A`, read after `A`
    absorbed `B`, is an arrival at `A`. (Until now the edge takes that for an error.)
    Regions go on naming an absorbed region for as long as they believe it; nothing
    tells them.
39. **When the edge learns that `B` went into `A`, it asks again** (rule 14) for every
    viewer's subscription it has, at any region but `A`, that was told elsewhere with
    `B`. That region then asks the store and names `A`.

#### 8.4 `Absorbed`

40. **`Absorbed { region: B, since, applied, numbers, players }`** is an entry of the
    outbox of the region `A` that absorbed `B`, made by the tick of the merge, with
    the next number after everything `A` had for the edge. The entries `B` had for the
    edge follow it at once, under the next numbers of `A`'s outbox; `numbers` are the
    numbers they had with `B`, in the same order. On a link that stands they are the
    first things of that tick; after a lost link they come in the welcome's entries,
    in the order of their numbers, before the edge sends anything it kept (rule 3);
    the welcome's `entries` counts them like any others. **No link is closed by a
    merge**, and an entry is written for every edge either region knew.
    - `since` is the `EdgeState::since` `B` had for the edge: what a welcome of `B`
      would have said. 0 if `B` did not know the edge with the start `A` knows it
      with.
    - `applied` is the number of the edge's last message that `B` applied.
    - `players` are the stays `A` has taken over from `B` for this edge, each as a
      presence answer has it, and more: the whole `PlayerState`.
41. **What the edge does with it**, in this order:
    1. `B` stands for `A` from now on (rule 38). Entities that `B` introduced count as
       introduced by `A`.
    2. **Whether `B` shared a numbering with the edge.** If `since` is the one the
       edge holds for `B`, and not 0, it did. If not, and the edge has seen an entry
       of `B` or had a message reported applied by `B`, then `B` had forgotten the
       edge: it gives up what it kept for `B` as after `Welcome::Unknown` (ADR-0008,
       section 5). If not, and the edge never had anything from `B`, then nothing it
       kept for `B` was applied, and all of it counts as above `applied`.
    3. **Players.** Each stay among `players` that the edge has with that entity,
       under whatever region, is `A`'s from now on, and the entry is its presence
       answer. For each one it does not have with that entity, it says `PlayerLeave {
       player, entity }` to `A`. A player it had under `B` who is not among `players`
       is judged as one for whom a presence answer says absent, but only when the
       entries of `numbers` have been handled, and with what step 5 moves counting as
       on its way: a `Departed` for them can be among the former and their arrival
       among the latter.
    4. **Subscriptions.** What it was subscribed to at `B` it asks of `A`, by the
       kind rules 5 and 6 give it there. Its viewer's subscriptions at `A` that were
       told elsewhere with `B` wait again, with the numbers they have, and need no
       message: `A` answers them (rule 44). All of this is on the link before step 5.
    5. **What it kept for `B`** above `applied` it keeps for `A`, under `A`'s next
       numbers and in the same order, and sends it. Left out are the inputs of a stay
       that is not among `players`, and a leave that names no entity.
    6. **The entries behind it**: it passes over those whose number in `numbers` is
       not above what it had seen of `B`, if the numbering was shared. It keeps
       `numbers`, with the number of the `Absorbed`, until it has seen past them: a
       link that ends in between brings the rest again without the `Absorbed` in
       front.
42. **A hello to the survivor names nothing of the absorbed region** unless the edge
    has handled the `Absorbed` and made those players and subscriptions `A`'s. ADR-0010
    has the edge end the link and say hello again, naming them; with rule 34, and
    `players` carrying what a presence answer has, the link it has will do.
43. **An `Absorbed` can stand behind an `Absorbed`**: `B` had absorbed `C` and the
    edge has not confirmed that. The inner entry is among `B`'s entries, and is
    handled as an entry of `A`: `C` stands for `A`. An edge that resumes with a region
    the routing table says `B` went into, and has been sent no `Absorbed` for `B` when
    the welcome's entries are through, treats `B` as having forgotten it (ADR-0010).

#### 8.5 Chunks that change hands, and `SplitOff`

44. **A served subscription stays served until the edge ends it, the link ends, or a
    split takes the chunk** (rule 12, changed). In the tick of a split, each
    subscription of the link to a chunk of the part that waits or is served is
    answered once more, with its number: a viewer's with `Elsewhere { region: N }`,
    after which it is as rule 13 has it; a guest's with `NotMine`, which ends it (rule
    15). **A merge ends no subscription.** A viewer's subscription at the survivor
    that was told elsewhere with the absorbed region is answered again without the
    edge asking, with its number, by a snapshot when the chunk is loaded, or by
    whatever the region then knows.
45. **`SplitOff { region: N, players }`** is an entry of the outbox of the region `A`
    that was split, made by the tick of the split, for each edge that has a player in
    the part. `players` are the stays that are in `N` from that tick on, each with its
    entity id. The edge:
    - for a stay it has with that entity under `A`: the player is `N`'s. There is no
      `PlayerArrive`; `N` has them whole. Their view's subscriptions move as at a
      hand-over (rule 18), and every input of theirs the edge still keeps is sent to
      `N`, which passes over what `A` had applied;
    - for a stay it has with that entity under another region: nothing; a later entry
      has said where they went;
    - for a stay it does not have with that entity: `PlayerLeave { player, entity }` to
      `N`.

    It sends `N` nothing else of what it kept for `A`. What it sent `A` and `A` had not
    applied, `A` still answers: an input of a player who went is passed over there,
    which is why it is sent to `N`; an arrival or a remote action for a chunk of the
    part goes on to `N` by `NotMine` or `Remote`, as for any chunk `A` does not hold.
46. **The order of the tick of a merge or a split on a link**: the `Outbox` messages
    of its entries, in ascending order of their numbers; then, for a merge, one
    `TickDelta` with the removals of the tick, which come on every link, and with the
    `EntitySpawned` of the players who came in for the links that watch their chunks;
    then, for a split, the `Elsewhere` and `NotMine` of rule 44 in ascending order of
    the chunks. It has no welcome, no `ToPlayer` and no `Progress`, and it is not on a
    link before the store has the record.
47. **The first link to a new region** begins like any: a hello, with the players the
    edge believes to be there and their views. The welcome is `Unknown { since,
    entries: 0 }`, which to an edge that never had anything from `N` is how everything
    begins (ADR-0008, section 5): what it kept for `N` stays, numbered from 1. **The
    presence answers after it are `Present`** for the stays `N` has (section 3.6). An
    edge that links to `N` before it has read `A`'s `SplitOff` names nobody, and does
    what rule 45 says on that link when it reads it.
48. **A stay of a player who left in the meantime** is in the region the merge or the
    split put it in until the edge has read the entry and said `PlayerLeave` with its
    entity. An edge that gives up the players of a region because the region forgot it
    (`Welcome::Unknown`) cannot know which of them a split has taken elsewhere, and
    says `PlayerLeave { player, entity }` for each to every region it has a port for.

#### 8.6 What is on its way

49. Each of these ends at a region that holds the chunk and has it loaded, or is
    answered:
    - **A message for `B` that `B` had not applied** is kept by the edge and goes to
      `A` by rule 41, behind the subscriptions of step 4, so that rule 34 holds it
      until `A` has loaded what it acts on.
    - **A message for `A` sent while `A` stood still** waits on the link and is taken
      by the first tick after the merge or the split.
    - **An entry of any region that names `B`** goes to `A` (rule 38). An arrival that
      `A` itself let go to `B` before the merge comes back to `A` and is taken in.
    - **A block action of a player who stays, on a chunk of the part**: `A` believes
      `N` while it wants the chunk and says `Remote { to: Some(N) }`; the edge has
      asked `N` for the chunk by then (rule 13, on the `Elsewhere` of rule 44, which is
      earlier in `A`'s stream), so rule 34 holds the action at `N` until `N` serves
      the chunk.
    - **A third region that still believes `A` to hold a chunk of the part** sends
      players and actions to `A`, which sends them on to `N` with `NotMine` if it
      still believes `N`, or takes the player in and asks.
50. **Beliefs still form no ring** (rule 20). A region that holds a chunk knows so,
    or knows nothing of it, or holds it by one of its pinned areas and believes
    another; and in that last case it takes in whoever is sent to it for the chunk
    and asks again (section 2.2). The count of ADR-0013, section 4, stays as the last
    resort.

#### 8.7 An edge that was away

51. An edge that had no link to any region during several merges and splits finds,
    with each region that still lives, its outbox in the order it was made: every
    `Absorbed` (with the absorbed region's entries behind it, an inner `Absorbed` and
    a `SplitOff` among them, if that is how it went) and every `SplitOff`, before the
    presence answers and before it sends anything. It finds no route for a region
    that was absorbed, and learns from the entries or from the routing table what it
    went into. The order in which it resumes with the regions does not matter: rule 41
    takes a stay from whatever region the edge had it under, rule 45 only from the
    region that says it, and both go by the entity. If it was away for more than 600
    ticks of a region, that region has forgotten it and its entries (rule 1), and
    rule 48 is what is left.

#### 8.8 What an edge must not assume, further

- That a served subscription is served until the edge ends it (rule 44).
- That a region it sends a player's inputs to still has the player (rule 45).
- That an entry never sends a thing back to the region it came from (rule 38).
- That `Welcome::Unknown` is followed by `Absent` (rule 47), or by no entries (rule
  40: a survivor that came by its state for the edge through the merge says `Unknown`
  with the `Absorbed` and what is behind it).
- That a region has only the stays the edge has (rule 36).
- That a block action is judged in the tick that takes it (rule 34).

#### 8.9 Where ADR-0010's sketch of the edge's side does not hold

ADR-0010 was written before subscriptions had numbers and kinds, before a hello named
`since`, `chunks` and `guests`, and before a welcome announced its entries. Against
the runner and the edge as they are:

- "It ends the link and says hello again, naming it": not needed, and with links that
  stay there is no hello to say again (rule 42).
- `knew`: an edge can hold a `since` for the absorbed region that is not the one that
  region had, which a yes or no cannot tell it (rules 40 and 41).
- "Treated as a presence answer that says absent", at the entry: only behind the
  absorbed region's entries, which the welcome counts among its own (rules 40 and 41).
- "A player who is not `B`'s any more is passed over": told by the entity, and such a
  stay is ended with a leave that names it (rules 37 and 41).
- "Chunks it was subscribed to at `B` are asked of `A`": by kind, with numbers of the
  link to `A`; and what `A`'s own viewers were told elsewhere is answered unasked
  (rules 41 and 44).
- "It sends `N` what they did after the last input the region had applied, as after a
  hand-over": without an arrival, and everything it keeps of theirs, as it may not
  have heard what was applied (rule 45).
- "What it kept for `A` above `applied` that concerns a named player or a chunk named,
  it sends to `N` as well": no. `A`'s link stands and `A` has been sent those; it
  answers them, and sending them to `N` too would have them done twice (rule 45).
- "The chunks named it asks `N` for": the entry names no chunks. The players' views
  move with them, and the viewers of those who stay are told `Elsewhere` (rules 44 and
  45).
- `Elsewhere` to guests when a split takes a chunk: `NotMine`, as a guest's
  subscription is never answered `Elsewhere` (rule 10).
- An entry whose destination is the region it came from "is handled there like any
  other": the edge of today disconnects the player (rule 38).

### 9. Changes to messages and types

Beyond step C0, ADR-0011 section 8 and ADR-0012 section 6.

**`clustine-sim`**

```rust
pub enum PlayerChange {
    Leave(EdgeId, PlayerId, Option<EntityId>),  // gains the entity
    // the others as they are
}

pub enum Durable {
    Absorbed {
        region: RegionId,
        since: u64,                             // in place of `knew: bool`
        applied: u64,
        numbers: Vec<u64>,
        players: Vec<(PlayerId, PlayerState)>,
    },
    SplitOff {
        region: RegionId,
        // Was `Vec<PlayerId>`; `applied` and `chunks` go.
        players: Vec<(PlayerId, EntityId)>,
    },
    // the others as they are
}
```

and `Region::absorb`, `take_absorbed`, `split`, `take_split`, `Absorbing`, `Splitting`,
`NoSplit` and `Part` of sections 2.3 and 2.4. `TickInputs` and `TickOutput` are as
they are.

**`clustine-rpc`**

```rust
EdgeToWorker::PlayerLeave { player: PlayerId, entity: Option<EntityId> }

StoreRequest::SplitCommit { tick, state, part, as_epoch, region: RegionId }
StoreReply::Absorbed { absorbed, chunks, pinned: Vec<ChunkArea> }
Decline::NotNext { next: RegionId }
RegionList { home, regions, absorbed, next: RegionId }

/// Why a merge or a split came to nothing.
pub enum Off {
    /// The worker does not run the region with the epoch named.
    NotRunning,
    /// The region is in the middle of a release, a merge or a split.
    Busy,
    /// No player stands in a chunk named that the region holds.
    Nobody,
    /// Nobody would stay, and the region would hold nothing.
    NothingStays,
    /// The store declined.
    Declined(Decline),
    /// The store was lost on the way; what happened, the store's list says.
    StoreLost,
    /// The region to absorb could not be opened or read.
    Unreadable,
    /// The store has seen a later owner of the region to absorb.
    Refused,
}

ToCoordinator::AbsorbEnded { region, absorbed, outcome: Result<(), Off> }
ToCoordinator::SplitEnded { region, as_epoch: u64, outcome: Result<RegionId, Off> }
FromCoordinator::SplitOff { region, epoch, chunks, as_epoch, part: RegionId }
FromCoordinator::Prepare { region: RegionId, epoch: u64 }
RoutingTable { .., waiting: u32 }   // in `clustine-region`
```

`Merge`, `Split`, `Absorb` and `Asked` are as C0 made them.

**The store** (`services/worldstore`), three changes and no other:

1. `Lanes::absorb` answers with the areas the absorbed region was pinned to, read from
   the table before `Table::absorb` moves them (ADR-0011, open question 10).
2. `Lanes::split` makes the region the request names, and declines with `NotNext {
   next }`, `next` being the table's next id, if that is not the one. It is looked at
   last, after every other reason (ADR-0011, "Found while building", C1.5, item 1), so
   that a worker told `NotNext` knows nothing else stands in the way. ADR-0011 had the
   store choose the id and say it in the answer; but the entry `SplitOff`, which names
   the new region, is part of the state that the very record holds, so whoever makes
   the state has to know the id before.
3. `Table::list` fills `next`.

**`services/worker`**: `STATE_FORMAT` becomes 3 with the commit that changes the two
entries, and `a_state_and_a_delta` in the worker's tests gets one of each written
out. No stored state has either entry, so nothing would be misread without it; the
number goes up because the rule of ADR-0012, section 4.1, is that it does.

**What each breaks**

- `PlayerLeave` and `PlayerChange::Leave`: `RegionRunner::accept_numbered`;
  `Region::tick` and `TickInputs::change`; `Fanout::remove_player`, and
  `Fanout::presence`, which looks for a kept `PlayerLeave { player }`; `leave` in the
  sim's test fixtures and every literal in `crates/clustine-sim/tests`, in the worker's
  tests and in the round trips of `clustine-rpc` (`link.rs`, `tcp.rs`).
- `Durable::Absorbed`, `Durable::SplitOff`: the literals in
  `crates/clustine-sim/tests/state.rs` and `clustine-rpc/src/wire.rs`;
  `Fanout::handle_entry` only names them.
- `SplitCommit`, `StoreReply::Absorbed`, `Decline`, `RegionList`: `Lanes::split`,
  `absorb`, `Table::list`; `wire.rs`; and every use in the store's tests
  (`regions.rs`, `scenarios.rs`, `kill_regions.rs`: the helpers `split` and `absorb`
  and what compares their answers). A split there has to name the next id, which the
  tests know or read from the list.
- `AbsorbEnded`, `SplitEnded`, `SplitOff`, `Prepare`: `wire.rs`; `Service::heard`;
  `event_from`, `RoutingWatch::next` and `Mover::next` in the coordinator's client,
  which match on `FromCoordinator`.
- `RoutingTable::waiting` and `is_complete`: `Coordinator::routing_table`; the
  literals in the coordinator's tests and client; `whole_world` in
  `bin/clustine/src/cluster.rs` reads `is_complete` and logs the layout's count.

**Existing tests that change their point**

| Test | What becomes of it |
|---|---|
| `a_second_join_of_the_same_player_is_ignored` (`region.rs`), `a_join_through_the_same_edge_is_ignored` (`tests/specification.rs`) | A join of a player the region has begins a new stay, under the same edge as under another |
| `an_arrival_of_a_player_the_region_has_leaves_them_as_they_are` (`tests/specification.rs`), `an_arrival_of_a_player_who_is_there_changes_nothing_whatever_the_chunk` (`tests/chunks.rs`) | Both arrive with an entity id far above the player's: that arrival now takes the place of the stay that was there. As written they hold for an arrival with a lower id |
| `an_ordinary_subscription_holds_nothing` (worker, `tests/specification.rs`) | It sends a move, which is still not held. Its name says more than is so now: a block action about the chunk would wait |
| The worker's tests that find `Presence::Absent` after an `Unknown` that tells the edge again | `Present`, where the state has the player. No existing test can have such a player: until this step a state has a player of an edge only by a numbered message of that edge, and then something has been received |

Every test of the hold behind a hello is as it was.

### 10. Building it

Each step leaves the four checks of `CLAUDE.md` green on `main`. The edge's part is
designed after this record; where a step needs the edge to change with it, the table
says so.

| # | Scope | Whose | Needs | The edge in the same commit | Its tests |
|---|---|---|---|---|---|
| C3.1 | The messages and types of section 9, and the store's three changes. Everyone else constructs and passes over them: the runner still logs the store's answers, the coordinator still closes on `Merge` and `Split` | shared; then the store alone | nothing | says `PlayerLeave` with the entity it has; reads the new shapes and still only confirms the two entries | The store's: T1 to T3 below; every existing test |
| C3.2 | The sim: stays (2.1), the doubt in pinned areas (2.2), `absorb`, `take_absorbed`, `split`, `take_split` | `crates/clustine-sim` alone | C3.1 | nothing | S19 to S36 |
| C3.3 | The runner: the new hold (3.5), presence after being told again (3.6) | `services/worker` | C3.1 | nothing: an edge that subscribes before it acts gains, and none relies on the opposite | R23 to R27; the end-to-end tests |
| C3.4 | The runner: `Reshape`, the phases, the merge and the split, the part and its warm chunks (3.1 to 3.4, 3.7) | `services/worker` | C3.2, C3.3 | nothing | R28 to R45; K1 to K14 |
| C3.5 | The coordinator's state machine and service: the list, reservations, orders, answers, the routing table (section 5) | `services/coordinator`, `clustine-region` | C3.1 | reads `waiting` | Q1 to Q16 |
| C3.6 | The worker process and the commands (section 4, 5.1); `--store` for the coordinator | `bin/clustine`, `deploy/` | C3.4, C3.5 | nothing | P1 to P4, without players |
| C3.7 | The edge (its own record, against section 8) | `services/edge` | C3.3 for rule 34; C3.1 | all of it | Its own; A1 to A8 against scripted regions |
| C3.8 | End to end, under the bots | `bin/clustine/tests` | C3.6, C3.7 | | E1 to E6; kind |

C3.2, C3.3 and C3.5 share no file and can be built side by side once C3.1 is pushed;
C3.4 follows C3.2 and C3.3 in the worker's one file; C3.7 can begin when C3.3 is in.
**Until C3.7 is built, `clustine merge` and `clustine split` work on the regions and
must not be used with players**: the edge confirms the entries and does nothing. The
tests of C3.6 therefore have no players in them, and the roadmap says nothing to the
owner before C3.8.

**For whoever writes tests from this record alone.** The fixtures are those of
ADR-0012, section 8, and ADR-0011, section 9: the sim driven tick by tick with a test
that plays the store and the edges; the runner stepped by the test, with a `Link` per
edge, on `Store::memory_divided` and `local_divided`. The worlds: **stripes** at a
boundary at 1 (region 0 west, with the home chunk; region 1 east), and **three
stripes** at 0 and 4 where a third region is needed. A "crash" of a region is the
region opened again with a higher epoch.

*The store*

- T1. `Absorbed` names the areas the absorbed region was pinned to, and none for one
  that was pinned to nothing.
- T2. A split that names the next id is done and answered with it; one that names
  another is declined `NotNext` with the next id and changes nothing; a split that is
  wrong in another way as well is declined for that other reason.
- T3. `next` of the list is above every living and every absorbed region, goes up by
  one with each split, and is the same after the store is started again.

*The sim*

- S19. A join of a player the region has under the same edge: the old entity is
  reported removed, a new one is spawned with the next id, `last_input` and `handled`
  begin anew.
- S20. An arrival with a higher entity id than the region's stay of that player
  replaces it (the old entity removed where it stood; the new one as the transfer
  says, and sent on with `NotMine` instead if its chunk is believed another's); with a
  lower id it is passed over and its entity reported removed; with the same id nothing
  happens.
- S21. A leave that names the player's entity removes them; one that names another
  entity changes nothing; one that names none removes them; none of them does through
  another edge.
- S22. A region pinned to an area that believes region `r` to hold a chunk of it: an
  arrival for that chunk is taken in, the chunk is `Asked` at the end of that tick and
  in its `claims`; a remote action about it is answered `Remote { to: None }` and the
  chunk is `Unknown`. The same chunk outside any pinned area: `NotMine` with `r`, as
  before.
- S23. `absorb` on two states with different players: all of them are in the merged
  state as they were, `players` of each edge's entry has those that came in, and each
  is reported spawned.
- S24. A player in both states: the higher entity id stays, whichever side has it,
  the other entity is reported removed once, and `players` has the player only if the
  other's stayed.
- S25. Edges: one start in both (the survivor's `since`, `applied` and numbers stay;
  the entry is next, the other's entries behind it under the next numbers, `numbers`
  their old ones, `sent` the last); known to the survivor only (an entry with `since`
  0 and nothing behind it); known to the other only (a state with `since` `M` and
  `applied` 0, the entry numbered 1); the other's start lower (its players of that
  edge do not come in and are reported removed, as are the entities on their way in
  its outbox; the entry is as for an edge it did not know); the survivor's start lower
  (its own players of that edge removed, its outbox dropped, `since` `M`, the edge in
  `reset`, and then as for an edge only the other knows).
- S26. `take_absorbed`: the chunks named are `Held` and none is in `returns` for
  `return_after` ticks; a chunk believed the absorbed region's that has a viewer's
  ticket is in `claims`, one without is `Unknown`; a guest's ticket on a chunk of an
  area named in `pinned` puts the chunk into `claims`, in this tick if the ticket is
  there and in the tick it comes otherwise; a held chunk with a ticket is in
  `chunk_requests`.
- S27. The merged state has the survivor's block and next entity id; a join after the
  merge gets the id it would have got.
- S28. `absorb` changes nothing: the region equals its copy from before, and ticks on
  from `T` as the copy does.
- S29. `split` is off with `Nobody` when no named chunk has a player, and when the
  only named chunk with a player is not held, or is the home chunk; and with
  `NothingStays` when nobody would stay in a region that holds no home chunk and is
  pinned to nothing. It is a split when nobody
  stays in a pinned region, and in one that holds the home chunk.
- S30. Who goes: exactly the players standing in named chunks that are held and not
  home; a player in a named chunk that is not held stays.
- S31. Which chunks: with one player going at chunk (10, 0) and one staying at (0, 0),
  of the held chunks in the row z = 0 those with x above 5 go and x = 5 stays, and
  the chunk (6, 7), which is as far from the one as from the other, stays; with the
  home chunk at (0, 0) held and nobody staying, the same; with nobody staying and no
  home chunk in a pinned region, every held chunk goes.
- S32. The states: the part has the players who go as they were, the empty block, and
  for each of their edges a state with the survivor's `start`, `since` `M`, nothing
  applied or sent; the region has lost them and has, for each such edge, one
  `SplitOff` with their entities under the next number.
- S33. After `take_split`: a chunk of the part with a viewer's ticket is
  `Foreign(part)`, one without is `Unknown`, none is loaded; `Part::chunks` has those
  that were loaded, block for block; a player who stays and steps into a chunk of the
  part with a viewer's ticket on it is let go to the part in that tick; a join at the
  part is refused.
- S34. `Part::region` equals `Region::restore` of the part's state with the part's
  chunks as its holdings.
- S35. **Against one region**, merging: two regions on stripes with one `Grants` and a
  router, and one region that holds everything, are given the same joins, walks, digs,
  placements and leaves; at some tick the eastern region's state is absorbed by the
  western one, the test giving `take_absorbed` the chunks and the area as the store
  would, and its links' tickets moving as an edge moves them; some ticks later the
  players and every loaded block are the same in both worlds, and stay so. With
  generated runs.
- S36. **Against one region**, splitting: the same with one region that is split and
  then run as two. And a region that is split and whose part is absorbed again has the
  players, blocks and held chunks of one that never was.

*The runner*

- R23. With a subscription to a chunk waiting (the store's answers held back): a dig
  into that chunk sent behind the `Subscribe` is not applied, and a move sent behind
  the dig is not either; when the snapshot is out, both are, in order, and the block
  is broken.
- R24. A move behind a `Subscribe` that waits is applied at once; so is a dig into a
  chunk the link has no subscription to.
- R25. A dig into a chunk whose subscription is answered `Elsewhere` waits until that
  answer and is then passed on with the region named.
- R26. A chunk the store cannot read does not hold a dig.
- R27. A state that knows an edge with a `since` the hello does not say, with a
  player of that edge and nothing received (made by writing the state through a
  handle, or by R35): the welcome is `Unknown` with the state's `since`, and the
  presence answer for the player is `Present`.
- R28. A merge on stripes, the test playing the absorbed region's worker (it runs
  region 1 with a player and an unconfirmed entry, releases it, opens it with a new
  epoch and hands the state over): on the survivor's link, with nothing in between
  that belongs to another tick, come `Absorbed` and behind it the other's entry under
  the next numbers, and nothing else of that tick but removals; the link is not
  closed; the store's list has region 1 absorbed; a crash of region 0 restores the
  merged state at `M` with no deltas.
- R29. What the link sent while the region stood still is applied by the first tick
  after, once and in order.
- R30. A viewer's subscription that was told elsewhere with region 1 gets a snapshot
  after the merge, with the number it had, without asking.
- R31. After the merge the link subscribes to a chunk that was region 1's and sends a
  dig into it behind that: the dig waits for the snapshot and breaks the block.
- R32. A guest's subscription to a chunk of region 1's former area, which the
  survivor had not held, is served after the merge (the area came with the answer).
- R33. A claim whose answer came while the region stood still is `Held` or believed
  after the first tick behind `M`.
- R34. A declined merge (the test opens region 1 with a still higher epoch before the
  commit): `Off`, the region ticks on with tick `M` as an ordinary tick, the link is
  open and has been told nothing.
- R35. An edge only region 1 knew says hello to the survivor after the merge:
  `Unknown { since: M, entries }`, the entries being the `Absorbed` and what is behind
  it from number 1, and `Present` for its players.
- R36. A release asked for during the merge ends as `Released` after it, and the next
  owner is restored with the merged state.
- R37. A split on stripes with two players of one edge, one in a named chunk: on the
  link, `SplitOff` with that player's entity and then, in ascending order of the
  chunks, `Elsewhere` naming the new region for the served viewer's subscriptions to
  chunks of the part, each with its number, and `NotMine` for a guest's; nothing for
  the chunks that stay; no event of a chunk of the part afterwards; the link stays.
- R38. The list has the new region with `as_epoch`; a crash of either region restores
  it with the state of the split at `M`.
- R39. A runner made of the part (`of_part`) answers a hello that names the player
  and the part's chunks with `Unknown { since: M, entries: 0 }`, `Present`, and a
  snapshot of every chunk that was loaded, **without one `Load` reaching the store**
  for those; a chunk it gives back and is granted again is asked of the store.
- R40. That runner and a runner restored from the store for the same region give the
  same answers to the same hello and inputs.
- R41. `Off::Nobody`, `Off::NothingStays`: nothing reaches the store, the region ticks
  on, the link is told nothing.
- R42. A split that names a wrong id succeeds with the right one, and the `SplitOff`
  entry names the region the list has.
- R43. After the split: a dig by the player who stayed into a chunk of the part is
  `Remote { to: Some(N) }`; an arrival for a chunk of the part is `NotMine` with `N`;
  an input of the player who went is passed over, and progress still covers its
  number.
- R44. No load is under way at the commit: with the store's answers to loads held
  back, the runner does not send `SplitCommit` until they are let go, and takes none
  of those chunks afterwards.
- R45. A runner stopped while it waits for the store's answer ends as `Abandoned`,
  and has published nothing of tick `M`.

*Kills*, each on the runner with the store in memory and on disk. The test stops (by
dropping the runner and every handle, which is what a dead worker leaves) at the point
named, opens every region the list then has with a higher epoch, and checks: the list
and the states are **as before** or **as after** as a whole (sections 6 and 7); every
player is in exactly one region's state; everything a link was sent before the stop is
in what the states have; a link that says hello to each region afterwards, and then
sends what it kept, ends with every player where the uninterrupted run has them.

- K1 to K7, a merge: after the absorbed region's release; after it is opened with
  `as_epoch`; after the survivor's first checkpoint; after it stopped ticking; after
  the second checkpoint; after `AbsorbCommit` is sent and before the answer is taken
  (both outcomes, by holding the request back or letting it through); after
  `take_absorbed` and before anything is published.
- K8 to K12, a split: after the first checkpoint; after the second; after
  `SplitCommit` is sent (both outcomes); after `take_split` and before anything is
  published or the part has its handle; after the part's hello.
- K13. The store killed at every write and sync of the record under a runner that is
  merging, and K14 under one that is splitting (`Fault::Stop` and `Fault::Fails` of
  ADR-0011): the runner ends as lost, and what is found is as before or as after.

*The coordinator*, on the state machine and on the service with a list it is handed:

- Q1. Each refusal of a merge and of a split, with nothing changed.
- Q2. A merge in order: `Release` and `Prepare`; on `Released` no assignment of the
  absorbed region, and `Absorb` with an epoch above every other; on `AbsorbEnded` and
  a list with the pair, the region is gone, the routing table has the pair and a new
  version, the asker is told `Ok`.
- Q3. While it lasts: a move of either region is refused; nothing is evened out; the
  survivor is not taken for want of vouching; ticks do not assign the absorbed
  region.
- Q4. `AbsorbEnded` with an error and a list that still has the region: it is
  assigned at once with a higher epoch; the asker is told the reason.
- Q5. The release unanswered for a lease: as an overdue release; the asker is told.
- Q6. `Absorb` unanswered for a lease, the list with the pair: done. Without the
  pair: the absorbed region is assigned. With no list: assigned; then `AbsorbEnded`
  from the worker that was refused removes it.
- Q7. The survivor's owner registers again at the second stage: `Absorb` is said
  again with the same epoch.
- Q8. The survivor's worker falls silent: the reservation ends with its lease, the
  list is read, both regions are assigned or the absorbed one is gone.
- Q9. A split in order: `SplitOff` with the list's `next`; on `SplitEnded { Ok(n) }`
  the new region is that worker's with `as_epoch`, has a route, and its heartbeat
  keeps it.
- Q10. `SplitEnded` with an error; and none within a lease, with a list that has a
  new region (assigned) and one that has not.
- Q11. A list with a region the coordinator does not know adds it without an owner;
  one that has a known region among the absorbed removes it and its owner's
  assignment; one that lacks a region at or above its `next` leaves that region.
- Q12. A registration that reports a region the coordinator does not know keeps it
  with that worker; a later list without it, and `next` above it, takes it away.
- Q13. Nothing is evened out within a lease of a split's end.
- Q14. `is_complete` is false while a known region has no owner.
- Q15. A coordinator made anew with a merge at each of its stages behind it, and a
  split, comes to the regions and owners the list and the registrations give.
- Q16. A leaving worker's reserved region is released when the reservation has
  ended.

*The processes, without players* (`clustine coordinator`, `worldstore`, two workers):

- P1. `clustine merge` of the two stripes: the command prints the survivor; the list
  has one region pinned to both areas; `clustine merge` of the home region into the
  other is refused.
- P2. `clustine split` with no player anywhere is told that nobody stands there.
- P3. `clustine move` of the survivor after a merge works; a merge asked during a
  move is refused, and a move during a merge.
- P4. A worker killed while it absorbs: within two leases every region the list has
  is run by someone.

*An edge that was away*, on the runner with a scripted link (for the regions' half),
and again for the edge with scripted regions when it is built:

- A1. The link ends before a merge; its next hello to the survivor names only what
  it had there: `Resumed`, the `Absorbed` and the other's unconfirmed entries among
  the welcome's entries, `Present` for the players named.
- A2. `B` absorbs `C`, then `A` absorbs `B`, with no link meanwhile: `A`'s welcome
  has `Absorbed { B }`, and among the entries behind it `Absorbed { C }` with `C`'s
  entries behind that; `numbers` of each are the numbers its entries had where they
  came from.
- A3. A split, then the part absorbed by a third region, with no link: `A`'s welcome
  has `SplitOff` with the stay; the third region's has `Absorbed` with the same stay
  among `players`.
- A4. A player leaves while there is no link and their region is absorbed: the
  survivor has the stay until a leave that names its entity comes, and a leave that
  names another entity does not remove it.
- A5. That player joins again and walks into the survivor before the leave: the
  arrival replaces the stay, and the leave with the old entity then changes nothing.
- A6. The same for a part: the stay is in the part; an arrival with a higher entity
  replaces it; the leave with the old entity changes nothing.
- A7. A link that ends between the `Absorbed` and the entries behind it, having
  confirmed the `Absorbed`: the next welcome's entries are the rest, without it.
- A8. Three merges and two splits in a row with no link, then a hello to each living
  region in each order: the stays the welcomes and entries name, taken together, are
  each in exactly one region, and that is the region whose state has them.

*End to end*, under the ledger bots at view distance 8:

- E1. Two regions merged while bots walk, build and cross the line: nobody is
  disconnected, the ledger equals the world, one entity per player throughout; the
  longest wait of a bot of each region is printed and bounded.
- E2. A region split under the same, with bots in the part and bots that stay; then
  the part moved; then merged back.
- E3. Split and merge twenty times in a row with bots walking between the two.
- E4. A worker killed at a seeded moment during a merge and during a split, again and
  again (`CLUSTINE_CHAOS_SEED`): the invariants of the chaos tests.
- E5. The edge's process stopped (`SIGSTOP`) across two merges and a split, for less
  than its patience, and continued: the invariants.
- E6. Bots that leave and join again while their region is merged or split: each has
  one entity and can act.

## What a player notices

Nothing here is measured. From the measurement of a move (0.36 to 0.40 s, optimised)
and what each step is made of:

**A merge, for the absorbed region's players**: its release without the first
checkpoint, which is made while it ticks (the stop, the wait for its commits, a small
checkpoint, closing: tens of milliseconds); the word to the coordinator and the order
to the survivor's worker; a hello for the absorbed region, which the store answers
without its thread for chunks unless the release failed; the survivor's first
checkpoint, which `Prepare` has made small; its stop, its second checkpoint, and the
record with its two syncs; then the entry on a link that is there. That is a move
without the restore, the linking and the resume, and with the survivor's stop and the
record instead: about what a move costs, or less. Without `Prepare` the survivor's
first checkpoint would be in it, which is most of the 0.25 to 0.31 s that `clustine
move` takes. After it these players walk at once, and **what they do to blocks waits
until the survivor has loaded the chunk** from the store, which the absorbed region
had in memory a moment before: some hundred chunks per player are asked in one tick,
and an action waits for its own. That is the part of this design a player is most
likely to feel, as a dig that takes a few ticks longer right after a merge; see the
open questions.

**For the survivor's players**: from its stop to the store's answer, the wait for the
commits of at most eight ticks, a checkpoint of what changed in a few ticks, and the
record. No link ends and nothing is sent again. That should be one to three ticks.

**A split, for everyone in the region**: the same stop, checkpoint and record, and
the store writes the new region's file before it answers. **For those who went**,
further: the worker's word to the coordinator, the routing table, the edge's link to
the new region, a hello for the region at the store, and a resume whose chunks come
from memory. That is the linking and the resume of a move without its restore.

A part is left where it was made for a lease before regions are evened out, so that
its players are not stood still twice running.

## Ruled out

- **Closing the survivor's links**, as ADR-0010 has it. Every merge would be a resume
  for players about whom nothing changes, twice over where the hello could not yet
  name what came with the merge. The edge has to handle the entries in a welcome all
  the same, and does so with the same code that handles them on a link that stands.
- **Holding less behind a hello** (ADR-0009's open point). See section 3.5.
- **A merge without a release, on one worker**, with the absorbed region's runner
  handing its state and its loaded chunks over in memory. It would spare the absorbed
  region's players the release and the loading. It is a second way for a region's
  state to get from one runner to another, and the merge across workers needs the
  first one anyway. If the loading shows, this is the way back.
- **The store choosing the new region's id**, as built. The id is in the state the
  record holds. Also ruled out: an entry that names the part by its epoch (an epoch
  changes with the next move), and an id fixed up when the state is read (two forms of
  one state).
- **Telling a pinned region that a chunk has come back**, by a reply of the store
  nobody asked for; and **keeping a part from giving back chunks of another's area**,
  which would change what ADR-0011 built and tests. The doubt of section 2.2 needs
  neither.
- **Rewriting entries that name the absorbed region.** The edge may have seen them
  under their numbers, and has to know what went into what in any case, for what
  third regions say.
- **A leave without an entity and an edge that sends things in the right order.**
  The edge reads an entry after it has sent other things, on other links, and nothing
  orders two links.
- **Presence answers for players the hello did not name**, so that an edge learns of
  a part's players from the part itself. It would let a player go on a little sooner
  when the edge reaches the part before it reaches the region that was split, and it
  is one more thing an edge has to handle at every hello.
- **Sending orders again for a split.** A merge taken twice is refused by the store
  the second time; a split taken twice is two splits.
- **Loading what came with a merge before the merge**, by the survivor's worker
  reading the absorbed region's chunks while it prepares. Only the holder loads a
  chunk.

## Consequences

- A merge and a split are, to the region, one tick with no inputs, and to the store
  one record. Whatever dies, a region is restored as before or as after.
- The survivor's players and those who stay at a split lose a tick or three and keep
  their links.
- After a merge the survivor loads from the store what the absorbed region had in
  memory. Until a chunk is there, what is done to its blocks waits.
- A region can be pinned to several areas and learns the new ones at the merge.
- The coordinator needs the world store's address, reads its list, and knows regions
  the layout does not have.
- Three things ADR-0008 said of joins, arrivals and leaves change, and a leave is a
  field longer on the wire.
- A click behind one's back in the two ticks after a hand-over is held for those
  ticks, with what that edge sent the region behind it.
- A part comes to be on the worker that split its region; evening out moves it later.
- The home region can absorb and be split, and never loses the home chunk; a player
  who joins while home's players are a part elsewhere joins a region that may have
  nobody else in it.

## Changes to ADR-0010

1. **Section 4, step 7, and "What a player notices"**: the survivor does not close its
   links; neither does a region that is split (section 5, step 5).
2. **Section 4, step 5, "If `A` has a player already, `A`'s stays"**: the later stay
   stays, by the entity id.
3. **Section 4, `Absorbed`**: `knew` becomes `since`, the number a welcome of the
   absorbed region would have said.
4. **Section 4, what the edge does**: it does not end the link to say hello again; it
   takes a stay among `players` from whatever region it had it under; it says a leave
   with the entity for a stay it no longer has; "what concerns a player who is not
   `B`'s any more" is, exactly, the inputs of a stay that is not among `players`.
5. **Section 4, step 3, "checkpoints it if its log is not empty"**: by a checkpoint of
   the state alone, as the store has replayed the blocks.
6. **Section 5, step 1**, the chunks "in which the group's players stood, as it was
   told": the players standing in the chunks named when the tick of the split is
   worked out go, and no others.
7. **Section 5, step 3**: who goes and which chunks, as section 2.4 here has them;
   "the home chunk and what surrounds it within a view" is what is nearer to the home
   chunk than to anyone who goes; the split is also off when nothing would be left of
   the region.
8. **Section 5, `SplitOff`**: it names the stays with their entities and nothing
   else; the edge sends the new region the inputs of those players and nothing else it
   kept for the old one.
9. **Section 5, step 5, "tells the links subscribed to chunks of the part
   `Elsewhere`"**: a viewer's subscription; a guest's is told `NotMine` (ADR-0012,
   change 14).
10. **Section 6**: the list is read on events in C3 and on a timer from C4; a worker
    does not report what it is in the middle of; a region a worker reports that the
    coordinator does not know is taken as living.
11. **Section 3, "Knowing an edge again"**: after `Unknown` that tells the edge the
    `since` the region already had, presence is answered from the state.

## Changes to ADR-0011

1. **Section 3.6, step 6, and open question 10**: the answer to a merge carries the
   areas.
2. **Section 3.7, steps 2 and 3**: the request names the new region's id, and the
   store declines one that is not the next (`Decline::NotNext`), looked at last.
3. **Section 5**: the list carries the next region id.
4. **Section 7, "`Absorbed` would end it too"**: the worker drops the region and tells
   the coordinator.

## Changes to ADR-0012

1. **Section 1.3, "it believes ... for as long as it wants the chunk"**: every belief
   in an absorbed region is dropped by the merge; and in its own pinned areas a region
   drops a belief when a player or an action is sent to it for the chunk (section 2.2
   here).
2. **Section 2.2, step 2**, an arrival for a chunk believed another's: not in the
   region's own pinned areas. **Section 2.4**, likewise for a remote action.
3. **Section 2.2, "No player goes round in circles"**: the proof's premise is as rule
   50 has it. The first two risks are closed by sections 2.2 to 2.4 here, the third by
   rules 34 and 44.
4. **Section 4.4, "A subscription that is served stays served"**, and rule 12: until a
   split takes the chunk. **"A subscription that is told elsewhere is not looked at
   again until the link asks again"**: or the region it named is absorbed by this one.
5. **Section 4.5 and rule 16**: the hold behind a hello stays; a second hold is added
   (rule 34). **"Presence answers ... are `Absent` after `Unknown`"**: not after one
   that tells the edge again.
6. **Open question 4**: `NotHeld` goes on ending the runner (section 3.2 here).
7. **Section 4.1**: `STATE_FORMAT` 3.

## Changes to ADR-0008, ADR-0009 and ADR-0013

1. **ADR-0008, section 2**: a join of a player the region has under the same edge is
   not ignored; an arrival of a player the region has can replace them; a leave can
   name an entity. **Section 4**: `PlayerLeave` carries it.
2. **ADR-0009, section 1, step 4**: a region released for a merge is not assigned.
   **Section 5**: the resume is not changed; the pause is 0.4 s with an optimised
   build. **Section 7**: nothing is evened out during a merge or a split or within a
   lease of one.
3. **ADR-0013, section 4, step 1**: a `Departed` to the region it came from is an
   arrival there when that is what an absorbed region stands for. **Section 3**: a
   told-elsewhere subscription can be answered with a snapshot unasked. The rest of
   what the edge does is its own record's.

## Open questions

1. **Whether the loading after a merge shows.** The absorbed region's players act on
   chunks the survivor has to read back from the store. If the bots of E1 wait
   noticeably longer for a dig than at a move, the first remedy is to ask the store
   for the chunks nearest the players first (a player reaches six blocks; the runner
   asks in ascending order today), and the second the merge on one worker that is
   ruled out above.
2. **One lease for a whole merge.** A release whose first checkpoint is minutes of
   changed chunks can take longer than a lease today, and then ends as a failed move
   does. The merge inherits that.
3. **Whether `Prepare` earns its message.** It is worth the survivor's first
   checkpoint to the absorbed region's players, by reasoning and not by measurement.
4. **How long an edge needs the absorbed pairs.** The routing table passes on all the
   store keeps (4096). An edge needs a pair for as long as some region may still
   believe the absorbed one, which rule 39 makes short for what its own viewers see.
5. **A part with one player and hundreds of chunks**: the record of a split holds two
   states, not the chunks; its size was not estimated against the 64 MiB a record may
   have. A state is players and outboxes; `Decline::TooLarge` is the answer if it ever
   is too large.
6. **What C4 names as a group's chunks.** Section 2.4 leaves the margin to it.
7. **Whether a split should refuse a part that would leave the home region without
   the chunks around the spawn point that a joining player sees.** Here they go to
   the part if somebody who goes is nearer; the joining player is shown them by the
   part, as a guest.

## Risks

- **The edge is designed after this.** Section 8 asks more of it than section 5 of
  ADR-0012 did: stays by entity, names that stand for other names, entries that carry
  players, an order of steps inside one entry (rule 41). A rule that is wrong there is
  found only by the edge's own tests, A1 to A8 and E1 to E6.
- **The later stay is told by the entity id.** That holds while one region with one
  block joins every player. A second joining region, or ids for anything but players
  given out away from home (ADR-0010 leaves that open), needs another order of stays.
- **A stale leave without an entity.** A player who quits before they are told their
  entity is removed whatever their entity. That leave goes to the home region, in the
  same stream as their join, so nothing can come between; if joins ever go elsewhere,
  it can.
- **The new hold stops a link behind one action.** A subscription that the store is
  slow to answer holds everything that edge sent the region behind a click into that
  chunk. The store being slow holds the region's commits as well, so this adds little;
  it is new all the same.
- **Warm chunks rest on "only the holder saves".** If anything ever writes a chunk
  that is not its holder's, a part serves what the store no longer has.
- **Two readings of the list in a row can be in either order of truth** only if they
  are applied out of the order they were asked in; the service asks one at a time for
  that reason. A reading older than a split it does not show is told by `next`.
- **Tests without players (C3.6) cannot show a split that does anything**, as a split
  needs a player. Between C3.6 and C3.8 the split is covered by the runner's tests
  alone.

## Not checked

What this record says of the code and its author did not verify, or verified only in
part:

- **The edge's code** was read where it handles links, welcomes, entries, presence and
  hand-overs (`take_link`, `welcomed`, `handle_entry`, `presence`, `hand_over`,
  `send_to_region`), not as a whole. That nothing in the edge relies on a block action
  being judged at once behind a `Subscribe` (step C3.3) is a reading of ADR-0013, not
  of every path.
- **How the edge numbers a player's inputs across two stays** was not read. Rule 41,
  step 5, leaves the inputs of an earlier stay out for that reason.
- **The tests of the worker and of the sim** were searched by name for the hold, for
  joins and arrivals of a player who is there, and for presence; they were not read
  one by one. A test that sends a dig behind a `Subscribe` in one step and expects it
  passed on at once would change its point with rule 34, and none was looked for
  beyond those section 9 names.
- **The end-to-end tests** in `bin/clustine/tests` were not read.
- **That a `Loaded` always reaches the handle before the `Flushed` asked behind it**
  was read in `ChunkService::work` and `Lanes::handle` (one channel of replies per
  handle; the load is answered by the thread for chunks before it takes the flush
  behind it). Over TCP one thread writes a connection's replies from that channel and
  one reads them (`converse`, `receive_replies`); those two were looked at, not read
  through. Section 3.2 and R44 rest on it.
- **The times under "What a player notices"** are reasoning from one measurement of a
  move.
- **`deploy/`** was listed, not read.
