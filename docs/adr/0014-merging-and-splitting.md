# ADR-0014: Merging and splitting

- Status: **Accepted**; the design of step C3 of milestone M3, phase C, for the
  simulation, the region runner, the worker process and the coordinator, and the
  contract the edge's part of the step is designed against. Revised after an
  independent review against the code (see "Review"). Not built. The edge's part is
  ADR-0015, designed against section 8 of this one; its review changed rules 37 to
  40, 44, 45, 47, 48 and 50 and the welcomes, as ADR-0015 lists.
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
these was read in the code at commit `8ef6159`, and the review found each as stated:

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
  not in the answer, and a region that holds its chunks by being pinned has no grants.
  **Its split** (`Lanes::split`) takes the next region id from the table itself, checks
  that the region holds every chunk named by grant or by being pinned
  (`Table::held_from`), writes the record, and answers `Split { region }`. The hello for
  the new region is answered by the commit thread when nothing is to be replayed
  (`Lanes::open`). A hello for an absorbed region is refused with
  `StoreError::Absorbed`, over TCP too, and `worker` in `bin/clustine/src/cluster.rs`
  ends the process on it, as on every refusal but `EpochRefused`. A request over TCP
  is at most `wire::MAX_MESSAGE_LENGTH`, 16 MiB.
- **A region's tick** is `Region::tick`; nothing else changes a `Region`. A join of a
  player the region has under the same edge is ignored; an arrival of a player the
  region has changes nothing and reports the arriving entity removed if it is another;
  a leave removes the player if they belong to the edge it came through, whatever
  their entity; an input is applied if the player is that edge's and its number is
  above their last. `TickInputs::change` drops, for a leave, what that player did
  through that edge and waits for the coming tick. What a region holds, has asked and
  believes (`Land::known`), its tickets, its loaded chunks and what it has asked of
  storage are not in its `RegionState`.
- **The runner** owns one `Region` and one store handle. It is told things from other
  threads in two ways only: links through `Links::attach`, and a release through an
  `AtomicBool` that `RegionRunner::run` looks at before every step. A release goes
  through `Phase::Preparing` (a checkpoint and a flush while it ticks), `Settling` (no
  tick, nothing taken from links, new links closed, the pending ticks published as
  their commits are confirmed), and `Closing` (a second checkpoint and a flush), and
  ends by dropping the handle and the links. The step that finds the first flush
  answered stops before it takes anything from a link; what an earlier step took and
  no tick has taken yet can still wait then (a hold that ended in the last tick has
  its queue passed on at that tick's end, and a step at the bound of eight ticks ahead
  takes from links and does not tick).
- **The hold** (`EdgeLink::hold`, `held`): after a hello, every message of the link,
  numbered or not, is put aside until each chunk the hello named is answered or cannot
  be read. A subscription outside a hello holds nothing.
- **Presence** is answered for the players a hello names and for no others, and is
  `Present` only after `Answer::Resumed` (`RegionRunner::tick`).
- **The worker process** keeps a map from region to `Phase` (`Opening`, `Running`,
  `Releasing`), opens regions with `open_region`, drops every region that its orders
  no longer name, comparing whole `Assignment`s, `entity_ids` included, looks at a
  release every 5 ms (`RELEASE_LOOK`) and at everything else every 250 ms (`LOOK`),
  and says `Released` again only while its orders still name what it released.
- **The coordinator** knows the regions of its layout and no others
  (`Coordinator::new`); a holding for another region is turned away with "the layout
  has no such region". A region whose owner says `Released` is assigned at once
  (`Coordinator::hand_over`). It has no address of the world store, and nothing calls
  `clustine_worldstore::regions`. `Coordinator::routing_table` leaves `home` and
  `absorbed` empty, and `RoutingTable::is_complete` counts the routes against the
  layout.
- **The edge** takes a `Departed` whose `to` is the region it came from for an error
  and disconnects the player (`Fanout::hand_over`); says `PlayerLeave { player }` to
  the region it believes the player to be in; numbers a player's inputs from 1 with
  every connection (`PlayerView::inputs_sent`); passes over a presence answer for a
  player it does not have or has under another region (`Fanout::presence`); and its
  task is handed links, not routing tables.

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
   store declines, `A` ticks on from `T` as if nothing had been asked, with its links.
2. **At tick `M` the region and its runner begin anew, as after a restore, without
   leaving the worker.** The region is what `Region::restore` makes of its new state
   and of what it holds; the runner is what it would be for a region just restored;
   **every link is closed**. The edge links again and resumes, and meets `Absorbed`
   and `SplitOff` in one place only: among the entries of a welcome, before the
   presence answers and before it sends anything. That place is needed in any case, as
   a link can end at any moment. The price is a resume for the survivor's players and
   for those who stay at a split; "What a player notices" counts it.
3. **The absorbed region is released first**, as ADR-0010 has it, and the survivor's
   worker opens it at the store with a new epoch. The new region of a split is run at
   once by the worker that split it, from memory.
4. **The hold of a resume stays as it is** (section 3.6): with an optimised build a
   move's pause is 0.4 s and most of it is the release. One narrow hold is added,
   which is what makes one resume enough after a merge: **a block action waits while
   its link still waits for a chunk that the region is itself about to serve**: one
   it holds and has not loaded, or one of its own pinned areas that it has yet to
   hear about.
5. **A player's stays are ordered by their entity ids, and what is said about a player
   names the stay** (section 2.1). A merge and a split carry players from one region's
   state into another's without a message on a link, so the order of messages on one
   link no longer says which of two stays of a player is meant, or which is the later.
6. **At every hello the region says whom it has for the edge**, named in the hello or
   not (section 3.7). That is where region and edge come to agree on who is there,
   whatever was lost on the way.
7. **The store's list decides what happened.** A worker's word that a merge or a split
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
  edge's and have that entity; with none, as today, whatever their entity.
- **An input names the stay it is of.** `TickInputs::inputs` becomes `Vec<(EdgeId,
  PlayerId, EntityId, u64, PlayerInput)>`, and an input is applied only if the player
  is that edge's, has that entity, and the number is above their last. An edge numbers
  a player's inputs from 1 with every connection, so without the entity an input of an
  earlier stay that reaches a region late is taken for one of the later stay with a
  high number: it moves the player to where their old self walked, and their real
  inputs up to that number are passed over.
- **`TickInputs::change` drops nothing for a leave.** It drops, today, what the player
  did through that edge and waits for the coming tick, before the tick can know
  whether the leave applies; a leave for a stay the region does not have is an
  ordinary thing from now on, and would eat the inputs of the stay that is there.
  Nothing needs the dropping: a tick applies changes before inputs, so an input behind
  a leave that took effect finds no player, and a join or an arrival behind the leave
  drops what waits, as today.
- **In a merge the later stay stays** (section 2.3).

Why the first three are needed, by the shortest case of each. *Join*: `P` leaves while
`B` is released, and the leave waits for a region that takes nothing any more; `P`
joins again at home, which is absorbing `B`, and the join is taken after the merge by
a region that has `P` from `B` under the same edge. *Arrival*: `N` is split off with
`P`, who has left since; the leave went to `A`, which no longer has `P`; `P` joins
again and walks into `N` before the edge has said hello to `N`. *Leave*: the edge then
tells `N` that the stay with the old entity has ended, after the new stay has arrived
there.

#### 2.2 In its own pinned areas a region doubts what it believes

A chunk that was split off a pinned region and is given back by whoever held it is the
pinned region's again by the store's table, and nobody tells the pinned region
(ADR-0011, section 2). So:

> An arrival for, or a remote action about, a chunk of one of the region's own pinned
> areas that the region believes another region to hold is handled as if the region
> knew nothing of the chunk, and the belief is dropped in that tick.

The player is taken in, and the chunk is claimed at the end of the tick because they
stand in it; the action goes on as `Remote { action, to: None }`. Outside its pinned
areas a region answers `NotMine` as before. **The belief is dropped exactly where a
`NotMine` would have been made**, and nowhere else: by an arrival that comes through
an edge the region knows and is not passed over as an earlier or the same stay, and
by a remote action that comes through such an edge and whose step concerns the chunk.
What is passed over for another reason leaves the belief alone, and so does a
player's own action on the chunk, which names the believed region as before. What the store then answers is the truth
of that moment: `granted`, and the player stays; or `foreign`, and they are let go
once more, to a region that holds the chunk and knows so.

#### 2.3 The merge: `Region::absorb` and `Region::take_absorbed`

```rust
impl Region {
    /// The state this region would have after absorbing `absorbed`, whose state is
    /// `other`: its whole state as of tick `M`. Changes nothing.
    pub fn absorb(&self, absorbed: RegionId, other: &RegionState) -> RegionState;
    /// Makes the region what [`Region::restore`] makes of `state`, of the chunks it
    /// holds and `chunks`, and of its pinned areas and `pinned`, and returns the
    /// chunks that were loaded. `state` is what `absorb` gave for this very region,
    /// with no tick in between.
    pub fn take_absorbed(
        &mut self,
        state: RegionState,
        chunks: &[ChunkPos],
        pinned: &[ChunkArea],
    ) -> Vec<(ChunkPos, Chunk)>;
    /// Whether `position` is in an area the region is pinned to.
    pub fn pins(&self, position: ChunkPos) -> bool;
}
```

`absorb`, in this order:

1. The tick is `M`.
2. **Edges**, for each edge either state knows, in ascending order, with `a` this
   region's `EdgeState` and `b` the other's:
   - both know it and `b.start < a.start`: the other's side is reset as a higher start
     resets a region (ADR-0008, section 2): its players of that edge do not come in,
     its outbox for the edge is dropped, and `b` counts as not there from here on;
   - both know it and `a.start < b.start`: this region's side is reset: its players
     of that edge are gone, and `a` becomes `{ start: b.start, since: M, applied: 0,
     sent: 0 }` with an empty outbox;
   - only the other knows it: `a` is made, `{ start: b.start, since: M, applied: 0,
     sent: 0 }`;
   - only this region knows it, or both with one start: `a` is as it is.
3. **Players** of the other state whose edge was not reset away in step 2, in ascending
   order. One this region does not have comes in whole: entity, name, pose, hotbar,
   held slot, `last_input`, `handled` and edge as they are. For one it has, the later
   stay stays (section 2.1): if the other's entity id is higher, the other's player
   takes the place of this region's; otherwise this region's stays. Section 2.5 says
   why the two cannot have one entity id.
4. **Entries.** For each edge of step 2, in ascending order, one entry is added to
   `a.outbox` under the number `a.sent + 1`:

   ```rust
   Durable::Absorbed {
       region: absorbed,
       since: b.since,     // 0 if the other did not know the edge, or was reset
       applied: b.applied, // 0 likewise
       numbers,            // the numbers of `b.outbox`, ascending; empty likewise
   }
   ```

   and behind it every entry of `b.outbox` as it is, in ascending order of its number
   there, under `a.sent + 2` and on. `a.sent` goes up by one more than there are such
   entries. An entry is written for an edge only one of the two knows as well: the
   edge may keep things for the other.
5. `entity_ids` and `next_entity_id` are this region's.

Nothing of an entry is rewritten: a `Departed`, `Remote` or `NotMine` that names `B`,
in either outbox, stays as it is, and so does one of `B` that names `A` (rule 39).
The entry names no players. ADR-0010 has it carry those of the edge that came from
`B`, each as a presence answer; the presence answers that follow every welcome say
which stays `A` has (section 3.7), and an entry that is read again after a crash
would say who was there at the merge, not who is.

**No event is reported for tick `M`.** No link is there to hear one (section 3.3). An
entity that does not stay, and a player of an edge that is reset, are put right on
screens by the snapshots that answer the next hello, as after a restore. What is not
put right by them is the entity of a player on their way in an outbox the merge
drops, which only an edge can still show that the merge did not reset; a crash
between a commit and its publication leaves the same today.

`take_absorbed(state, chunks, pinned)` makes the region **exactly what
`Region::restore(config, state, Holdings { held, pinned })` makes**, with `held` being
every chunk the region held (`Knowledge::Held`: granted, or of a pinned area and
claimed) and every chunk of `chunks`, and `pinned` its areas and those named. So:

- **it learns the areas that came with the merge** without being opened (ADR-0012's
  first risk);
- **it forgets every belief about other regions, and everything it had asked**, as a
  restored region has. Not only what it believed of the absorbed region: a belief in a
  region that had gone into the absorbed one earlier, or in a third region about a
  chunk that came by one of the new areas, would stand against the store's table with
  nothing to doubt it. What is wanted afterwards is asked again;
- it has no ticket, no loaded chunk and nothing asked of storage: the links are
  closed with this tick, and the chunks that were loaded are handed to the runner,
  which keeps them warm (section 3.5);
- each chunk it holds counts as used up to tick `M`.

The runner passes as `chunks` what the store's answer names and what the store had
granted the region in answer to claims that no tick has been told of (section 3.2).

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
    /// Makes the region what [`Region::restore`] makes of `splitting.state`, of the
    /// chunks it holds without the part's and with `granted`, and of its pinned
    /// areas; returns the chunks that were loaded and stay, and the part.
    pub fn take_split(
        &mut self,
        splitting: Splitting,
        granted: &[ChunkPos],
    ) -> (Vec<(ChunkPos, Chunk)>, Part);
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
   chunks is said on the link, anew on every link.
6. **The part's state**: tick `M`; the empty block of entity ids, with `first`, `end`
   and `next_entity_id` all 0, which is what the store says of such a region (ADR-0011,
   section 2); the players who go, whole; and for each edge that has one of them an
   `EdgeState { start, since: M, applied: 0, sent: 0 }` with an empty outbox, `start`
   being this region's for the edge. No other edge is known to the part.

`take_split(splitting, granted)`:

- the region becomes **exactly what `Region::restore(config, splitting.state, Holdings
  { held, pinned })` makes**, with `held` being every chunk it held that is no chunk
  of the part, and `granted` (what the store had granted it in answer to claims that
  no tick has been told of; none of those is a chunk of the part, as the sim did not
  hold it when the part was worked out), and `pinned` its areas. So it knows nothing
  of the part's chunks, and nothing of any other region's: it asks again;
- `Part::region` is `Region::restore(config, splitting.part, Holdings { held:
  splitting.chunks, pinned: vec![] })` with this region's config;
- of the chunks that were loaded, those of the part are in `Part::chunks` and the
  others are returned.

So after a split `A` never thinks it holds what it does not, and `N` and `A` both
begin as a restored region does, by asking.

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
player. A stay is in at most one living region's state at a time: the tick that lets
a player go takes them out of the state and puts the `Departed` in the outbox in one
commit, and the region that takes them in does so by a numbered message that is
applied once; a split takes its players out of one state and puts them into the other
in one record; a merge retires the state it reads. `absorb` therefore does not look
for one id under two players, and keeps both if it is handed such states.

#### 2.6 What is compared

`absorb` and `split` are functions of the region and their arguments. Three things
follow that the tests of section 10 use:

- a region that plans a merge or a split and does not take it is, bit for bit, the
  region it was, and its later ticks are those of a region that never planned;
- the region after `take_absorbed`, the region after `take_split`, and `Part::region`
  each equal the region `Region::restore` makes of the same state and holdings. For
  the part those are what the store's record and list have, so a part that is run
  from memory and a part that another worker restores do the same;
- so everything ADR-0012 says of a restored region (its section 1.4) holds of a region
  after a merge or a split.

Against one region that holds everything and is given the same joins, inputs and
leaves, two regions that merge, and one region that splits, show the same players in
the same places with the same hotbars and the same blocks, a bounded number of ticks
after the last hand-over between them (what crosses a boundary takes its two ticks, as
today). What is not the same and is not compared: tick numbers, outbox entries and
their numbers, `since`, `applied`, and what each region knows of chunks.

**One condition, found by the tests written from this section** (103, with runs
generated against one region; nothing else was found): the blocks are the same only
if no player acts twice on one block while the first action is still under way
between regions. A placement against a block across a boundary travels region to
region, two ticks or more; a dig of that same block by the same player, who has
walked into its chunk meanwhile, is applied at once and overtakes it. One region
places and then digs; two regions dig and then place. It needs no merge or split and
is ADR-0012's two ticks; for a player it is about a tenth of a second around a
crossing. The generated runs keep off it, and it is listed with the known limits.

### 3. The runner

#### 3.1 Commands, phases, and what can be seen of them

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

/// How far a runner is with a release, a merge or a split.
pub enum Stage { Preparing, Settling, Closing, Committing }

impl Worker {
    /// Hands `reshape` to the region's thread. `done` is called on that thread with
    /// the outcome, once; never for `Prepare`.
    pub fn reshape(&self, reshape: Reshape, done: Box<dyn FnOnce(Reshaped) + Send>);
}

impl RegionRunner {
    /// The same, for whoever steps a runner by hand.
    pub fn reshape(&mut self, reshape: Reshape, done: Box<dyn FnOnce(Reshaped) + Send>);
    /// Where the runner is with a release, a merge or a split; `None` while it only
    /// runs, and once it has ended.
    pub fn stage(&self) -> Option<Stage>;
}
```

`RegionRunner::run` takes a command before a step, as it looks at the release flag. A
runner that is not `Phase::Running`, or has a command under way, calls `done` with
`Off { why: Off::Busy }` at once. **The outcome comes by a call, not by something to
look at**: the worker process waits for it in a `select!` that otherwise looks at its
regions every quarter of a second, and passes a closure that sends into a channel of
its own (section 4). `done` is called exactly once whatever becomes of the runner: one
that is stopped or dropped in the middle calls it with `Off::StoreLost`, as what the
store has is then for the store to say; a command the region's thread never took,
because the thread had ended, is answered `Off::Busy`. It runs on the region's
thread and must not block.

The phases are those of a release, with one more:

| Phase | What happens |
|---|---|
| `Preparing` | a checkpoint and a flush; the region ticks and serves its links until the flush is answered |
| `Settling` | no tick; nothing is taken from links; links attached now are closed; the pending ticks are published as their commits are confirmed |
| `Closing` | a second checkpoint and a flush |
| `Committing` | only for a merge or a split: the plan is made and the commit sent; the runner waits for the store's answer |

`Prepare` asks for a checkpoint (`RegionRunner::checkpoint`) and nothing else, in
`Phase::Running` only. `stage` is public because the kill scenarios of section 10 are
written by someone who has the crate's interface and nothing else: a test that steps
a runner by hand can stop at the first step of each stage. **A step advances at most
one stage**, for a release as well, which until now could pass from `Preparing`
through `Settling` to `Closing` in one step when nothing was pending; `stage` is
`None` again as soon as the outcome is there.

#### 3.2 What is true when the commit is sent

When the flush behind the second checkpoint is answered, the store has answered
everything asked before it, in order (`Lanes::end_group` answers commits and claims
before it passes the flush on; the thread for chunks answers a load before it takes
the flush behind it, over TCP as within one process; a flush behind a return is
answered when the return is durable). So at that moment:

- every commit is confirmed and published, and covered by the checkpoint: the region
  has no live commit, which the store demands (`Decline::Uncheckpointed`);
- **every claim is answered.** The answers wait in the coming tick's inputs, as no
  tick has run. When tick `M` is taken, the `granted` among them go into it (sections
  2.3 and 2.4) and the `foreign` are dropped: the region forgets what it believed and
  what it had asked in that tick, so nothing waits for them. A chunk granted so is not
  `Held` in the sim when a split is worked out, is therefore no chunk of the part, and
  is `A`'s by the store as by the sim. If the merge or the split is off, both stay
  where they are and the next tick takes them (ADR-0012, section 4.2);
- every return is through; no chunk the sim has given back can be in a part, as the
  sim no longer holds it;
- **no load is under way.** Every `Loaded` has arrived, and `RegionRunner::loads` is
  empty;
- no chunk is unsaved, so every loaded chunk is, block for block, what the store has.

**What a link sent and no tick has taken.** The first version of this record said
that there is none when the runner stops, and the review showed two ways there is
(see the context). Nothing rests on it now. Such messages are not in the region's
state: they are not counted in its `applied`, and no `Progress` has told the edge
otherwise. If tick `M` is taken, they are dropped with the links they came on
(section 3.3), a hello that no tick answered and a `Confirm` among them, and the edge
sends again what was numbered, having kept it. If the merge or the split is off, they
are where they were and the next tick takes them.

**`NotHeld` goes on ending the runner** (ADR-0012, open question 4). A split takes
chunks from a region at a moment when the region has nothing outstanding about them: a
load asked before the split was let through by the commit thread when it took the
request, which is before the split, and answered before the flush; after the split
the sim does not hold the chunk and asks for nothing. A save of a chunk of the part is
never asked after the split either: nothing is unsaved, and the chunks have left the
sim. So `NotHeld` still means that region and store disagree.

**A request that is too large.** A request over TCP is at most 16 MiB, and one that is
longer loses the handle, which is no answer to give a large region. So the runner
sends no `AbsorbCommit` whose state, and no `SplitCommit` whose two states and chunks
(counted at 16 bytes each), are together more than `MAX_RESHAPE_BYTES`, 8 MiB: the
outcome is `Off { why: Off::TooLarge }` and the region ticks on. A player is some 150
bytes of a state and an unconfirmed entry about as much, so that is a region of some
fifty thousand players. A checkpoint of such a state would meet the same bound a
little later, merge or no merge; that is not made worse here, and not mended.

#### 3.3 Taking tick `M`: the runner begins anew

Whether it was a merge or a split, when the store has answered that the record is on
disk the runner does this, and only this:

1. The region takes the tick (`take_absorbed` or `take_split`).
2. **Every link is dropped, which closes it**, and so are the links that were attached
   and not taken up. No ticket is given back for them: the region has none.
3. What the runner keeps for edges is made anew from the region's state, as
   `RegionRunner::with_store` makes it for a restored region: for each edge the state
   knows, its start, `received` and `applied` both the state's `applied`, no link, and
   away since `M`; `last_inputs` from the state's players. **The inputs of the coming
   tick are emptied**: everything a link sent and no tick took, every ticket change,
   and the store's answers, which step 1 took or dropped.
4. `committed` is `M`. What the store could not read stays noted. Nothing is pending,
   unsaved or being loaded (section 3.2).
5. The chunks that were loaded, and those the store had delivered for the coming
   tick **if the region held them already by what its ticks had been told**, are kept
   warm (section 3.5). A chunk that was given back and granted again by an answer no
   tick has taken can have been another region's in between, and is read from the
   store like any other.
6. `RegionStatus` is brought up to date; the phase is `Running`; `done` is called.

Nothing is published for tick `M`: there is no link. The next step takes up the links
that edges make, and its tick, `M + 1`, is an ordinary one: a player who stands in a
chunk the region knows nothing of makes it claim, and a hello is answered from the
state as of `M`, with the entries the merge or the split made among the welcome's.

This is the path a restore takes, entered without opening the region: nothing the
runner does after tick `M` differs from what it does after `RegionRunner::restore`,
but that it has the handle already and has warm chunks.

#### 3.4 The merge and the split in the runner

On `Reshape::Absorb`, from `Phase::Running`: `Preparing`, `Settling`, `Closing`. Then:

1. `state = region.absorb(absorbed, &other)`; if it is too large (section 3.2), off.
2. `StoreRequest::AbsorbCommit { absorbed, absorbed_epoch, tick: M, state:
   stored(&state) }`; the phase is `Committing`.
3. **`StoreReply::Absorbed { absorbed, chunks, pinned }`**: tick `M` is taken (section
   3.3), with `chunks` and the waiting `granted` as the chunks and `pinned` as the
   areas. The outcome is `Reshaped::Absorbed`.
4. **`StoreReply::Declined { reason }`**: the plan is dropped, the phase is `Running`,
   the outcome is `Off { why: Off::Declined(reason) }`. Nothing was said to anyone,
   no link was closed and no tick number was used: the next tick is `M`, with a commit
   like any other.
5. **The handle is lost** (`RegionRunner::give_up`): as for a lost store at any time.
   The outcome is `Off { why: Off::StoreLost }`, and whether the merge happened is for
   the store to say when the region is opened again.

On `Reshape::SplitOff`, likewise, and then:

1. `splitting = region.split(&chunks, part)`. If it is off, the phase is `Running`
   and the outcome `Off`, with `Off::Nobody` or `Off::NothingStays` as the sim says;
   if it is too large, `Off::TooLarge`.
2. `StoreRequest::SplitCommit { tick: M, state: stored(&splitting.state), part:
   SplitPart { chunks: splitting.chunks, state: stored(&splitting.part) }, as_epoch,
   region: part }`.
3. **`StoreReply::Declined { reason: Decline::NotNext { next } }`**, the first time:
   the plan is made again with `next` as the part's id and sent again. The store gives
   out region ids, and another split can have taken the one the order named. A second
   `NotNext` is a decline like any other. (The store changes nothing when it declines,
   so the same tick can be named again.)
4. **`StoreReply::Split { region }`**: tick `M` is taken (section 3.3), with the
   waiting `granted`. Of the chunks the store had delivered for the coming tick, those
   of the part go into `part.chunks`. The outcome is `Reshaped::Split { region,
   as_epoch, part }`.
5. Another decline, or a lost handle: as for a merge.

**A split answers no subscription.** The links are closed with tick `M`; a
subscription to a chunk of the part begins anew with the next hello, like every
other, and is answered by the ordinary rules of ADR-0012, section 4.4. `A` knows
nothing of the chunk then (it does not keep a belief that nothing wants, and nothing
wanted this one while it had no link), so a viewer's ticket makes it ask the store,
which says `foreign` with `N`: `Elsewhere { region: N }`, a tick or two after the
hello. A guest's is answered `NotMine`: at once outside `A`'s pinned areas, and after
the store's answer inside them, where a guest's ticket makes `A` ask. Each such answer
ends the hello's hold for its chunk, as today.

#### 3.5 Warm chunks, and a runner for the part

A runner keeps **warm chunks**: chunks it has in memory that the region holds and has
not loaded, each known to be what the store has. They come from tick `M`: every chunk
the region had loaded then, and for a part every chunk of it that the split region
had loaded. When a tick asks storage for a chunk that is warm, the runner hands it to
the next tick in `chunks_loaded` instead of asking the store, and forgets it. A warm
chunk is also forgotten when the region gives the chunk back (`TickOutput::returns`),
when 600 ticks have passed since `M` (`DEFAULT_GONE_AFTER`; a chunk nobody has asked
for by then is not coming back to a screen soon), and it goes with the chunk into
`Part::chunks` if a later split takes it.

That is safe because only the holder saves a chunk, and nothing was unsaved at `M`:
from then until this region loads the chunk, nothing can have changed what the store
has of it. The sim cannot tell a warm chunk from one the store delivered a tick later.
It is what keeps the resume after a merge or a split from reading back from the store
what the region had in memory a moment before.

`RegionRunner::of_part(part: Part, store: StoreHandle) -> RegionRunner` is a runner
for `part.region` as `with_store` makes one, with `part.chunks` warm.

#### 3.6 The hold

**A resume holds what it holds today**: everything the link sends behind its hello,
until each chunk the hello named is answered or cannot be read. ADR-0009 left open
whether to hold only what acts on chunks that are not there yet, if the pause at a
move were above about a second. It is 0.4 s with an optimised build, and of that the
resume is the smaller part (see the measurement in the context). Holding less would
change what every test of the hold asserts and what section 4.5 of ADR-0012 promises
("nothing an edge sends again after a restore reaches a tick before the region knows
of every chunk its players can reach who holds it") for a tenth of a second. It is
left as it is, for moves, merges and splits alike.

**One hold is new**, and applies to every link at all times:

> A numbered message that acts on a block is not passed into a tick while its link has
> a subscription that waits for a chunk the message is about, and that chunk is one
> the region holds and has not loaded, or one of an area the region is pinned to of
> which the store has told it nothing yet (it knows nothing of the chunk, or has
> asked). Everything the link sent behind it waits with it, numbered or not. A chunk
> the store has said it cannot read holds nothing.

The chunks a message is about: for `Input` with `Dig`, the chunk of the block; with
`UseItemOn`, the chunk of the block and that of the spot beside the clicked face; for
`Remote`, the chunk of every position its step names (`position`; `against` and
`target`; `target`). No other message is about a chunk for this rule: not a move, an
arrival, a join or a leave.

It is the same queue as the hello's hold (`EdgeLink::held`): a link is held while a
chunk of its hello is unanswered, or while the first message of its queue is such an
action. `RegionRunner::drain` and `release_held` take messages in order until they
meet one that has to wait. The runner tells by `Region::knowledge`, `Region::chunk`
and `Region::pins`.

**Why a merge needs it.** The hello of the edge's new link to the survivor names what
the edge had at the survivor. It cannot name what it had at the absorbed region: it
learns of the merge from the welcome's entries. It then asks the survivor for those
chunks (`Subscribe`, on that link) and sends on what the absorbed region's players did
meanwhile. Those chunks are on screens already, from the absorbed region; without a
hold, a dig would reach a tick in which the chunk is held and not yet loaded, and be
acknowledged without effect. With it the link the edge has will do, and the edge need
not end it to say hello again, as ADR-0010 has it.

**Why it is narrow.** The two cases are those in which the region itself is about to
serve the chunk. A chunk it has asked for outside its pinned areas is, if a client can
click it, another region's, and the answer will be `foreign`: the click behind one's
back right after a hand-over (ADR-0012, section 2.3) is judged at once, as today, and
goes on without a region named. The first version of this record held that too, and
with it everything that edge sent the region behind it, each time somebody crossed and
clicked.

**What it costs.** Behind one such action, everything the edge sent the region on
that link waits, the moves of other players among it. After a merge that is: while the
first dig of an absorbed player waits for its chunk to be claimed and read, nobody of
that edge moves in the survivor. See "What a player notices".

#### 3.7 Presence: the region says whom it has

After a welcome and its entries, a region answers presence **for every stay the state
has for the edge**, and for every player the hello named:

- for each player of the hello, in the hello's order: `Present`, with the stay's
  entity and the rest as today, if the state has the player under this edge; else
  `Absent`;
- then, for every other player the state has under this edge, in ascending order:
  `Present`.

The welcome says how many answers follow, as it says how many entries, and how far the
region had applied the edge's messages in the state they are made of:
`Welcome::Resumed { entries, presences, applied }`, `Welcome::Unknown { since,
entries, presences, applied }`. "The state" is the region's before the tick that
takes the hello, as for the entries, and `applied` is its `EdgeState::applied` for
the edge: 0 after an `Unknown` that makes or resets the state. It is what the tick's
`Progress` says afterwards; the welcome says it so that the edge knows, before it
reads a `Present` for a player who is entering the world, whether the region had
applied their join (ADR-0015, section 2.1). After an `Unknown` that makes or resets the state for the edge, the
state has nobody of it: the answers are those for the hello's players, all `Absent`,
as today. After an `Unknown` that tells the edge the `since` the state already had
(`Answer::ToldAgain`), the answers are from the state like after `Resumed`; until now
they are `Absent`. That is the case ADR-0012 left to this step: a part's state, and a
survivor's state for an edge it came by through the merge, know the edge with players
and with nothing received.

Why every stay, and why the count. A merge and a split put stays into a region that
the edge did not send there. The edge's hello cannot name them: it names whom it
believed to be there when the link was made. With an answer for each, the edge learns
at every hello what the region has, moves a stay it has elsewhere, and ends one it
does not have (section 8, rule 38); and what was lost on the way (an entry in an
outbox the region dropped when it forgot the edge, a leave the edge gave up) is put
right at the next hello, whenever that is. The count tells the edge when it has heard
all of them, so that it can take a player it believes to be there, and of whom no
`Present` came, for absent.

#### 3.8 Release and stop in the middle

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

### 4. The worker process

`FromCoordinator::Prepare`, `Absorb` and `SplitOff` become `WorkerEvent`s; `Orders`
and `Release` are as they are. In `worker` (`bin/clustine/src/cluster.rs`):

**Outcomes arrive on a channel of the process.** It makes an unbounded channel of
`(RegionId, Reshaped)`, hands each `Worker::reshape` a closure that sends into it, and
has a branch of its `select!` for the receiving end. Nothing a player waits for hangs
on `LOOK` or on `RELEASE_LOOK`.

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
served to edges and not reported at a registration. The survivor is served to edges
throughout, under the hello it had: an edge whose link the merge closed is let in
again at once.

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
     happened do not know of `n`;
   - **orders that name `n` with another epoch than `as_epoch`** are a new assignment
     of it (the coordinator found it in the list and gave it to this very worker): the
     part in memory is dropped with its runner, and the region is opened with the
     epoch named and restored from the store, like any region the worker is given.
4. `Reshaped::Off { why }`: `SplitEnded { region, as_epoch, outcome: Err(why) }`.

**`StoreError::Absorbed { into }` at any opening** no longer ends the worker: it drops
the region, never takes that assignment up again, and says `AbsorbEnded { region:
into, absorbed: region, outcome: Ok(()) }`, which makes the coordinator read the list.

**Orders that take a region away while it reshapes**, and a worker that stops: the
runner is stopped as it is (section 3.8), and a handle held for a region to absorb is
dropped.

**The single process** (`bin/clustine/src/lib.rs`) has no coordinator and gives no
such orders. Nothing in it changes but what the messages of section 9 change in what
it constructs. Its regions run the runner of this record: the new hold and the
presence answers apply there, and a leave and an input name their entity.

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
`SplitEnded`; and when a reservation ends without the worker's word (sections 5.3 and
5.4). Reading it every few seconds, as ADR-0010 has it, waits for C4, which also needs
the bounds and where the players are; nothing in C3 depends on hearing of a change
that no worker reported and no reservation covered.

**What `listed` does:**

- a living region of the list that the coordinator does not know is added, without an
  owner, and assigned like any such region (it is one nobody runs: the part of a split
  whose worker died before saying so). **Not while a split is reserved**: then it is
  left out of what the coordinator knows. It is the part of that split, run from
  memory by the worker that made it, and a reading that lands between the store's
  record and the worker's word would give it to another worker, with an epoch that
  fences the part and stands its players still for a restore. The reservation ends
  with the worker's word, which names the part, or with a reading of its own (section
  5.4), and that one adds what is still unknown;
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
3. on `AbsorbEnded { region: A, absorbed: B, .. }`, from whichever worker: the list is
   read, and `listed` ends the merge by what it finds. `B` absorbed: `B` is gone, the
   asker is told `Ok(A)`. `B` living: the merge is off; `B` has no owner (the worker
   has let go of it) and is assigned at once, as a region its owner let go of is; the
   asker is told `Err` with the worker's reason. If the list cannot be read, it is
   read again at every tick until the merge's time is up.

**The merge's time is one lease from when it was asked**, for all of it. When it is
up, or when the survivor loses its owner or its epoch changes, or the absorbed region
loses its owner otherwise than by step 2, the reservation ends:

- at stage 1, the release is an overdue release of ADR-0009: `B` is taken from its
  owner and assigned. That owner is noted as having failed the region **only if the
  merge's time is up**: where the reservation ends because the survivor lost its
  owner, or a reading of the list took the survivor away, `B`'s owner did nothing
  wrong, and is not passed over for six leases for it;
- at stage 2, the list is read first. `B` absorbed: the merge happened and the asker
  is told so. Otherwise `B` is assigned like a region its owner let go of, with an
  epoch above `as_epoch`, which fences a survivor's worker that is still at it: its
  `AbsorbCommit` is declined (`Decline::NotOpened`). If the list cannot be read
  either, `B` is assigned all the same; should it have been absorbed, the worker that
  is given it is refused by the store and says so (section 4), and that reads the
  list.

A worker may say `AbsorbEnded` for a merge the coordinator has no note of (it started
anew, or gave up, or the worker was refused a region that had been absorbed): it reads
the list.

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
  shows is one nobody is known to run, and is assigned; **the asker is told `Err`
  (`Overdue`) whatever the list shows.** A region with the id that was ordered says
  nothing of this split: the store can have declined it with `NotNext`, another split
  having taken that id, and the runner have tried again with the next. The regions
  are put right by the list either way; only the worker's word says which region a
  split made.

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
  route, the home region or the pairs change. The field, the coordinator that fills
  it and the new `is_complete` come in one commit (step C3.5): with the field alone,
  an edge would take a table of a coordinator that does not fill it for complete.
- **A coordinator that starts anew in the middle** knows nothing of the merge or the
  split. It reads the list, finds the regions as they are, before or after, and hears
  from the workers what they run. A released `B` is a region without an owner to it,
  and waits out the grace period like any such region: its old owner says `Released`
  again only to a coordinator whose orders still name `B`, which a new one's never
  did (it is assigned at once only if that word was still on its way). Whenever `B`
  is assigned, that fences the absorb if the record is not written yet. A part is
  reported by the worker that runs it, or found in the list and assigned after the
  grace period. Nothing is kept on disk.
- **What C4 needs of this**: `merge` and `split` with nobody as asker; their outcome
  in `Changes`, with the reason as an `Off` and not in words, to leave a region alone
  for a while or to name other chunks; the list read on a timer, which the rule for a
  reserved split above is written for; and the chunks of a split named with a margin
  (section 2.4).

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
| 8 | `A`'s runner | takes tick `M`; **closes its links**; ticks on | |
| 9 | `A`'s worker | drops `B`'s handle; `AbsorbEnded` | |
| 10 | coordinator | reads the list; `B` is gone; routing table with the pair; answers the asker | |
| 11 | edge | links to `A` again; hello; `Absorbed` among the welcome's entries; the presence answers; what it kept | |

`B`'s players stand still from the end of step 2's first checkpoint, `A`'s from the
stop in step 5, both until the resume of step 11 is through. Who may run out of time:
the coordinator alone, one lease after step 1. A worker waits for the store as long
as the store neither answers nor closes, as it does for any commit; the coordinator's
lease ends that from outside.

**A kill, and what is found.** "As before" is: `A` and `B` both living, each as its
state file and log have it. "As after" is: `A` with the merged state as of `M`, `B`
absorbed.

| Killed | When | Found | Put right by |
|---|---|---|---|
| coordinator | any time | as before or as after, whichever the workers reach | the new coordinator, by the list and the registrations (section 5.5). If `B` is assigned before step 7, the absorb is declined |
| `B`'s worker | in step 2 | as before; `B` without an owner, its log perhaps not empty | the merge's time runs out, or the worker's lease: the merge is off, `B` is assigned and restored like any region whose owner died |
| `A`'s worker | steps 2a to 6, before the record is synced | as before; `B` without an owner (the store closes both handles) | the worker's lease runs out, which ends the reservation: the list is read, `B` and `A` are assigned. `B`'s state file may be newer by step 4, which changes nothing |
| `A`'s worker | after the record is synced, before or after the store's answer, before or after step 8 | as after | the lease runs out; the list is read before `B` is given to anyone and shows it absorbed; `A`'s next owner is restored with the merged state, and the edges get `Absorbed` in its welcome, as they would have from the worker that died |
| store | before the record is synced | as before; every handle lost | the workers open their regions again; `A`'s runner says `Off::StoreLost`; `B` is assigned when its hello can be answered |
| store | after | as after; every handle lost | the same; the worker that is given `B`, if any, is refused with `Absorbed` and says so |
| store, in step 7 | at any write or sync | as before or as after, as a whole | ADR-0011, section 4.3 |
| `A`'s runner stopped (orders took `A`, or the process stops) | steps 5 to 8 | as before or as after | whoever runs `A` next |
| edge | any time | its players are gone, as today | |

In every row `A`'s players and `B`'s are in exactly one living region's state, with
what they did up to that region's last confirmed tick, and what they did since is
with the edge. Because the survivor begins anew at `M` as a restored region does,
"`A`'s worker died after the record" and "`A`'s worker lived" look the same to an
edge, but for the time it takes.

### 7. The split, step by step

| # | Who | What | Durable after it |
|---|---|---|---|
| 1 | coordinator | reads the list; reserves `A`; `SplitOff` with `as_epoch` and `part` | |
| 2 | `A`'s runner | checkpoint and flush while ticking; stops; publishes; second checkpoint and flush | `A`'s state file as of `T`; all its chunks |
| 3 | `A`'s runner | `split`, or off; `SplitCommit { tick: M, region: part }` | |
| 4 | store | ends the group; declines, or appends and syncs `Split`; then table, `N`'s lane and region file; answers | **the split**, when the record is synced |
| 5 | `A`'s runner | takes tick `M`; **closes its links**; ticks on | |
| 6 | `A`'s worker | `SplitEnded { Ok(N) }`; says hello for `N` with `as_epoch`; runs the part when the store has answered | `N`'s highest epoch, which it had by the record |
| 7 | coordinator | `N` is the worker's; routing table with a route for `N`; answers the asker; reads the list | |
| 8 | edge | links to `A` again: `SplitOff` among the welcome's entries; links to `N` when the table names it: hello, welcome, presence, the part's chunks from memory | |

Everyone in `A` stands still from the stop in step 2 until the edge has resumed with
`A`; those who went until it has resumed with `N`.

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

**The one setting.** A region that absorbs or is split closes every link it has, in
the tick that does it. So an edge meets `Absorbed` and `SplitOff` among the entries of
a welcome and nowhere else: after its hello, with every subscription of that link
begun anew, before the presence answers, and before it has sent the region anything
it kept (rule 3). A link that ends while the entries are being read brings the rest
in the next welcome. Nothing in this section has to hold on a link that was standing
when the merge or the split happened: there is none.

#### 8.1 The hold

34. **Behind a hello everything waits, as before** (rule 16). Beyond that, **a block
    action waits while the link it came on has a subscription that waits for a chunk
    the action is about, if the region holds that chunk and has not loaded it, or the
    chunk is in one of the region's own pinned areas and the store has not told the
    region about it yet** (section 3.6). Everything the edge sent on that link behind
    the action waits with it. What an edge may rely on:
    - if it says `Subscribe` or `SubscribeAsGuest` for a chunk and, behind that on
      the same link, sends a block action about the chunk, a region that holds the
      chunk, or comes to hold it inside its pinned areas, judges the action only when
      it has loaded the chunk and made the snapshot;
    - a region that neither holds the chunk nor is pinned to where it lies judges the
      action in the tick that takes it, as today, and passes it on;
    - an action about a chunk the link has no subscription to is judged in the tick
      that takes it, as today.

    So: **subscribe first, on that link, then send.** It is what lets an edge hand a
    region work for chunks the region has only just come by, on the link it has.

#### 8.2 Stays

35. **A player's stay is known by its entity id, and the higher id is the later
    stay** (section 2.1). A join begins a new stay and replaces whatever stay of that
    player the region has. An arrival replaces a stay with a lower id and is passed
    over, its entity reported removed, where the region has a higher one.
36. **What an edge says about a player names the stay.** `PlayerArrive` does by its
    transfer. `Input { player, entity, number, input }` names the entity the edge has
    told the player; a region applies an input only to that stay, and passes over,
    without a word and counted as applied, one for a stay it does not have.
    `PlayerLeave { player, entity }` names the entity whenever the edge has been told
    one for that stay (by `Spawned`, a presence answer, or a transfer), and `None`
    only for a player who quit before that; a region ends only that stay.
    **A leave that follows from what a region said names the entity the region
    said**, not one the edge had: where a presence answer shows a stay the edge does
    not have, that is the stay to end.

#### 8.3 Presence

37. **After a welcome's entries the region says whom it has for the edge**: one
    presence answer for each player the hello named, in the hello's order, `Present`
    or `Absent`; then one `Present` for every other stay it has for the edge, in
    ascending order of the players. The welcome's `presences` is how many follow, and
    its `applied` the number of the edge's last message the region had applied. They
    are of the same state as the entries: a stay that an entry among them sent
    elsewhere is not among them, and a join numbered above `applied` has not made
    the stay a `Present` speaks of.
38. **What the edge does with them:**
    - `Present`, and the edge has that stay (the player, with that entity) under this
      region: as today. For a player who is entering the world and has not been told
      an entity: the answer is their stay if their join is at or below the welcome's
      `applied`, and an earlier stay, which the join will end, if it is above.
    - `Present`, and the edge has that stay under another region: **the stay is this
      region's.** Its view's subscriptions move here as at a hand-over (rule 18),
      without an arrival, and every input of it the edge keeps above the answer's
      `last_input` is sent here.
    - `Present`, and the edge does not have that stay (it has given the player up, or
      has them with another entity): `PlayerLeave { player, entity }` to this region,
      with the answer's entity.
    - `Absent`: as today (ADR-0008, section 5).
    - **When `presences` answers have been handled**: a player whom an `Absorbed`
      among this welcome's entries made this region's (rule 42), and for whom no
      `Present` with their entity came, is absent, and is judged as after `Absent`
      today. Nobody else is judged then: a player the hello named has had an answer
      of their own, and one whom another region's entry put under this region
      meanwhile may have been sent on since, which their region's own answers will
      say.

    Why moving a stay on a region's word is always right. A stay is in at most one
    living region's state (section 2.5). A region comes by a stay in three ways: a
    join or an arrival the edge sent it, and then the edge has the stay under that
    region unless an entry of that very region has sent it on, which comes before the
    presence answers and takes the stay out of the state they are made of; or a merge
    or a split, which no message of the edge brought about. So a region that says
    `Present` for a stay the edge has elsewhere has it by a merge or a split the edge
    has not caught up with, and the region the edge had it under no longer has it, or
    is no more.

#### 8.4 Regions that are no more

39. **An absorbed region stands for the region it went into from the moment the edge
    has handled that `Absorbed`, and not before**; or from the moment it has concluded
    by rule 44 that none comes. Until then what is meant for `B` (what the edge kept
    for it, and what an entry of any region sends there in `Departed::to`,
    `Remote::to`, `NotMine::holder`, `Elsewhere::region` or `SplitOff::region`) is
    kept for `B`, as for any region without a link. From then on a name of `B` means `A`, through several
    merges in a row, and **if that makes the place an entry sends something the
    region the entry came from, the edge sends it there**: a `Departed { to: B }` of
    `A`, read after `A` absorbed `B`, is an arrival at `A`. (Until now the edge takes
    that for an error.) The routing table's pairs tell the edge which region to
    expect the entry from, and serve rule 44; it acts on nothing else of them, but
    that a region of which it has nothing at all (no player, subscription or kept
    message) may stand for its survivor on the table's word, as no entry could move
    anything.
    Regions other than the survivor go on naming an absorbed region for as long as
    they believe it; nothing tells them.
40. **A subscription that was told elsewhere with `B` is told elsewhere with `A`**
    from the moment the edge has handled `A`'s `Absorbed { B }`. It need not be asked
    again: the edge was a guest at `B` for the chunk, and by rule 42 is one at `A`,
    which serves it or says `NotMine` (rule 15).

#### 8.5 `Absorbed`

41. **`Absorbed { region: B, since, applied, numbers }`** is an entry of the outbox of
    the region `A` that absorbed `B`, made by the tick of the merge, with the next
    number after everything `A` had for the edge. The entries `B` had for the edge
    follow it at once, under the next numbers of `A`'s outbox, as they were; `numbers`
    are the numbers they had with `B`, in the same order. The welcome's `entries`
    counts them like any others. An entry is written for every edge either region
    knew.
    - `since` is the `EdgeState::since` `B` had for the edge: what a welcome of `B`
      would have said. 0 if `B` did not know the edge with the start `A` knows it
      with.
    - `applied` is the number of the edge's last message that `B` applied.
42. **What the edge does with it**, in this order:
    1. `B` stands for `A` from now on (rule 39). Entities that `B` introduced count as
       introduced by `A`, so that `A`'s snapshots put right what `B` showed.
    2. **Whether `B` shared a numbering with the edge.** If `since` is the one the
       edge holds for `B`, and not 0, it did. If not, and the edge has seen an entry
       of `B` or had a message reported applied by `B`, then `B` had forgotten the
       edge: it gives up the messages it kept for `B`, as after `Welcome::Unknown`
       (ADR-0008, section 5); the players it believed to be there are judged by step 3
       like any others, and `A` has none of them, as `B` had removed them when it
       forgot the edge. If not, and the edge never had anything from `B`, then
       nothing it kept for `B` was applied, and all of it counts as above `applied`.
    3. **The players it had under `B` it has under `A`.** It says nothing for that;
       the presence answers behind the entries say which of them `A` has (rule 38).
    4. **Subscriptions.** What it was subscribed to at `B` it asks of `A`, by the kind
       rules 5 and 6 give it there, with `Subscribe` and `SubscribeAsGuest` on this
       link.
    5. **What it kept for `B`** above `applied` it keeps for `A`, under `A`'s next
       numbers and in the same order, and sends it with the rest when the welcome's
       entries are through: behind step 4 on the link, so that rule 34 holds each
       block action among it until `A` has the chunk loaded.
    6. **The entries behind it**: it passes over those whose number in `numbers` is
       not above what it had seen of `B`, if the numbering was shared. It keeps
       `numbers`, with the number of the `Absorbed`, until it has seen past them: a
       link that ends in between brings the rest again without the `Absorbed` in
       front.
43. **A hello to the survivor names nothing of the absorbed region** unless the edge
    has handled the `Absorbed` and made those players and subscriptions `A`'s. One
    resume does: ADR-0010 has the edge end the link and say hello again, naming them;
    with rules 34, 37 and 42 the link it has will do.
44. **An `Absorbed` can stand behind an `Absorbed`**: `B` had absorbed `C` and the
    edge has not confirmed that. The inner entry is among `B`'s entries, and is
    handled as an entry of `A`: `C` stands for `A`. An edge that resumes with a region
    the routing table says `B` went into, and has been sent no `Absorbed` for `B` when
    the welcome's entries are through, treats `B` as having forgotten it (ADR-0010):
    it gives up what it kept for `B`, and `B` stands for that region. This rests
    on a region answering no hello between handing the store a merge or a split and
    taking it (section 3.3: taking it drops every link, also those attached and not
    taken up), so that a hello said after the table has the pair is answered from
    after the merge.

#### 8.6 `SplitOff`, and the new region

45. **`SplitOff { region: N, players }`** is an entry of the outbox of the region `A`
    that was split, made by the tick of the split, for each edge that has a player in
    the part. `players` are the stays that are in `N` from that tick on, each with its
    entity id. For a stay the edge has with that entity under `A`, **unless an
    arrival of theirs is among what the edge keeps for `A`** (the stay then came back
    to `A` after the split, by way of `N`, whose presence answers the edge read
    first): **the player is `N`'s.** There is no `PlayerArrive`; `N` has them whole. Their view's subscriptions
    move as at a hand-over (rule 18), and every input of theirs the edge still keeps
    is sent to `N`, which passes over what `A` had applied. For any other stay named,
    the edge does nothing: one it has under another region has been moved on by a
    presence answer or a later entry, and one it does not have is ended when `N`'s
    presence answers show it (rule 38).

    It sends `N` nothing else of what it kept for `A`. What it sends `A` again that
    `A` had not applied, `A` answers: an input of a stay that went is passed over
    there; an arrival or a remote action for a chunk of the part goes on by `NotMine`
    or `Remote`, or is taken in and let go, as for any chunk `A` does not hold.
46. **Chunks of the part need no word of their own.** `A`'s links are closed by the
    split, and on the next link each subscription is answered by the rules there are:
    a viewer's to a chunk of the part with `Elsewhere { region: N }`, a tick or two
    after the hello, as `A` asks the store; a guest's with `NotMine`. Rule 12 stands
    as it is: a served subscription stays served until the edge ends it or the link
    ends.
47. **The first link to a new region** begins like any: a hello, with the players the
    edge believes to be there and their views. The welcome is `Unknown { since,
    entries, presences, applied: 0 }`, with no entries unless `N` has itself been
    split or has absorbed since, which to an edge that never had anything from `N` is
    how everything begins (ADR-0008, section 5): what it kept for `N` stays, numbered from
    1. The presence answers after it are `Present` for every stay `N` has for the
    edge. An edge that links to `N` before it has read `A`'s `SplitOff` names nobody,
    and learns from those answers who is there (rule 38); the `SplitOff` then names
    stays it already has under `N`.

#### 8.7 What is on its way

48. Each of these is applied once by a region that has the player, or judged by a
    region that holds the chunk and has it loaded, or answered:
    - **A message for `B` that `B` had not applied** is kept by the edge and goes to
      `A` by rule 42, behind the subscriptions of its step 4.
    - **A message for `A` that no tick of `A` had taken when it stopped**, whether it
      was still on the link or not, was not applied and is not counted: `A`'s
      `Progress` on the next link says how far it got. The edge sends it again after
      the welcome's entries, having kept it.
    - **An entry of any region that names `B`** waits for `B` until the edge has
      handled the `Absorbed` (rule 39), and then goes to `A`. An arrival that `A`
      itself let go to `B` before the merge comes back to `A` and is taken in.
    - **A block action of a player who stays, on a chunk of the part.** It is behind
      the hello on the new link to `A`, and held until the chunk is answered, with
      `Elsewhere { N }`; `A` then says `Remote { to: Some(N) }`; the edge asks
      `N` for the chunk before it sends an action on to it, if it is not asking
      already (ADR-0015, section 4), so rule 34 holds the action at `N` until `N`
      serves it.
    - **A third region that still believes `A` to hold a chunk of the part** sends
      players and actions to `A`, which sends them on to `N` with `NotMine` if it
      has heard that `N` holds the chunk, and otherwise takes the player in and asks,
      or passes the action on without a region.
49. **Beliefs still form no ring** (rule 20). The proof of ADR-0012, section 2.2,
    takes, of the regions in a supposed ring, the one whose answer from the store is
    the latest; that answer names a region `H` that held the chunk then, and `H` can
    believe something of the chunk now only if it was told after it stopped holding
    it, which is later than the latest. With this step `H` can stop holding a chunk
    in two more ways, and hold one without knowing in one:
    - by a split: the split region knows nothing of the part's chunks afterwards, and
      what it comes to believe it is told later;
    - by being absorbed: the name `H` then means the survivor, and **the survivor
      forgets every belief at the merge** (section 2.3), so whatever it believes of
      the chunk it was told after the merge, which is after the answer that named
      `H`;
    - a region that holds a chunk by one of its pinned areas can believe another to
      hold it; it takes in whoever is sent to it for the chunk and asks again
      (section 2.2), which ends the ring there.

    The count of ADR-0013, section 4, stays as the last resort.

#### 8.8 An edge that was away

50. An edge that had no link to any region during several merges and splits finds,
    with each region that still lives, its outbox in the order it was made: every
    `Absorbed` (with the absorbed region's entries behind it, an inner `Absorbed` and
    a `SplitOff` among them, if that is how it went) and every `SplitOff`; and then
    the presence answers, which say who is there now. It finds no route for a region
    that was absorbed. The order in which it resumes with the regions does not
    matter, nor how the messages of their links fall between each other: a presence
    answer takes a stay from whatever region the edge had it under, a `SplitOff` only
    from the region that says it and only a stay that has not come back, both go by
    the entity, and nobody is judged absent on an entry of another region (rules 38
    and 45).
    **This is every edge after every merge and split**, as all of them meet the
    entries in a welcome. If the edge was away for more than 600 ticks of a region,
    that region has forgotten it and its entries (rule 1); the presence answers of
    the other regions still say what they have, and a stay that nobody claims is
    ended by the leave of rule 38.

#### 8.9 What an edge must not assume, further

- That a region has only the stays the edge sent it, or only those its hello named
  (rules 37 and 38).
- That a region it sends a player's inputs to still has the player (rule 45), or that
  an input is applied because it was counted (rule 36).
- That an entry never sends a thing back to the region it came from (rule 39).
- That `Welcome::Unknown` is followed by `Absent` (rule 47), or by no entries (rule
  41: a survivor that came by its state for the edge through the merge says `Unknown`
  with the `Absorbed` and what is behind it).
- That a block action is judged in the tick that takes it (rule 34).
- That a name it reads in the routing table's pairs may be acted on (rule 39).

What the edge's own record has to settle and this one does not: how the routing
table's pairs reach the task that handles links (it is handed links today, not
tables); and that the edge passes on no input of a player it has not told their
entity.

#### 8.10 Where ADR-0010's sketch of the edge's side does not hold

ADR-0010 was written before subscriptions had numbers and kinds, before a hello named
`since`, `chunks` and `guests`, and before a welcome announced its entries. Against
the runner and the edge as they are, and this record:

- "It ends the link and says hello again, naming it": not needed. The hello to the
  survivor comes before the edge can know of the merge; the subscriptions it then
  sends on that link are protected by rule 34, and the players by the presence
  answers (rule 43).
- `knew`: an edge can hold a `since` for the absorbed region that is not the one that
  region had, which a yes or no cannot tell it (rules 41 and 42).
- `players` in `Absorbed`, "treated as a presence answer": there are real presence
  answers behind every welcome's entries, for every stay, and they say who is there
  when the hello is answered, not who was at the merge (rules 37 and 42).
- "A player who is not `B`'s any more is passed over": every message about a player
  names the stay, and the region passes over what is for a stay it does not have
  (rule 36).
- "Chunks it was subscribed to at `B` are asked of `A`": by kind, with numbers of the
  link to `A` (rule 42).
- "It sends `N` what they did after the last input the region had applied, as after a
  hand-over": without an arrival, and everything it keeps of theirs, as it may not
  have heard what was applied (rule 45).
- "What it kept for `A` above `applied` that concerns a named player or a chunk named,
  it sends to `N` as well": no. `A` is sent it again and answers it; sending it to `N`
  too would have it done twice (rule 45).
- "The chunks named it asks `N` for": the entry names no chunks. The players' views
  move with them, and the viewers of those who stay are told `Elsewhere` when they
  ask `A` (rules 45 and 46).
- `Elsewhere` to links subscribed to chunks of the part, in the tick of the split: no
  link outlives that tick (rule 46).
- An entry whose destination is the region it came from "is handled there like any
  other": the edge of today disconnects the player (rule 39).
- "The routing table lists ... the regions that were absorbed and what they went
  into": it does, and the edge acts on an `Absorbed`, not on the table (rule 39).

### 9. Changes to messages and types

Beyond step C0, ADR-0011 section 8 and ADR-0012 section 6.

**`clustine-sim`**

```rust
pub enum PlayerChange {
    Leave(EdgeId, PlayerId, Option<EntityId>),  // gains the entity
    // the others as they are
}

pub struct TickInputs {
    // Gains the entity.
    pub inputs: Vec<(EdgeId, PlayerId, EntityId, u64, PlayerInput)>,
    // the others as they are
}

impl TickInputs {
    pub fn input(
        &mut self,
        edge: EdgeId,
        player: PlayerId,
        entity: EntityId,
        number: u64,
        input: PlayerInput,
    );
    // `change` drops nothing for a leave
}

pub enum Durable {
    Absorbed {
        region: RegionId,
        since: u64,          // in place of `knew: bool`
        applied: u64,
        numbers: Vec<u64>,
        // `players` goes
    },
    SplitOff {
        region: RegionId,
        // Was `Vec<PlayerId>`; `applied` and `chunks` go.
        players: Vec<(PlayerId, EntityId)>,
    },
    // the others as they are
}
```

and `Region::absorb`, `take_absorbed`, `pins`, `split`, `take_split`, `Splitting`,
`NoSplit` and `Part` of sections 2.3 and 2.4. `TickOutput` is as it is.

**`clustine-rpc`**

```rust
EdgeToWorker::PlayerLeave { player: PlayerId, entity: Option<EntityId> }
EdgeToWorker::Input {
    player: PlayerId,
    entity: EntityId,
    number: u64,
    input: PlayerInput,
}

pub enum Welcome {
    Resumed { entries: u32, presences: u32, applied: u64 },
    Unknown { since: u64, entries: u32, presences: u32, applied: u64 },
    Superseded,
}

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
    /// What the store would have to be handed is longer than a request may be.
    TooLarge,
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
RoutingTable { .., waiting: u32 }   // in `clustine-region`; with step C3.5
```

`Merge`, `Split`, `Absorb` and `Asked` are as C0 made them.

**The store** (`services/worldstore`), three changes and no other:

1. `Lanes::absorb` answers with the areas the absorbed region was pinned to, read from
   the table before `Table::absorb` moves them (ADR-0011, open question 10).
   **`chunks` stays what it is, the grants that moved.** In a world of pinned regions
   it is empty, as a stripe holds its chunks by being pinned; the survivor then claims
   each chunk of the new areas when it is wanted, and the store answers from the
   table. The answer cannot carry "what the absorbed region held" beyond that: the
   store keeps no note of which chunks of an area a pinned region has asked about,
   and which of them a part holds instead it says to a claim. The claim is a tick or
   two, the same every restored stripe pays for every chunk today.
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
`MAX_RESHAPE_BYTES: usize = 8 * 1024 * 1024`, half of `wire::MAX_MESSAGE_LENGTH`.

**What each breaks**

- `PlayerLeave` and `PlayerChange::Leave`: `RegionRunner::accept_numbered`;
  `Region::tick` and `TickInputs::change`; `Fanout::remove_player`, and
  `Fanout::presence`, which looks for a kept `PlayerLeave { player }`; `leave` in the
  sim's test fixtures and every literal in `crates/clustine-sim/tests`, in the worker's
  tests and in the round trips of `clustine-rpc` (`link.rs`, `tcp.rs`).
- `Input` and `TickInputs::inputs`: `Region::apply_input`, `TickInputs::input`,
  `RegionRunner::accept_numbered` and `forget_inputs_of`; `Command::Input` and
  `hand_over` in the edge, which make the message; and every fixture that makes an
  input: `numbered`, `with_number`, `walk`, `dig`, `place`, `select`, `set_slot` in
  `region.rs`, their like in the three files of `crates/clustine-sim/tests`, `walk_as`,
  `dig_by` and `input` in the worker's tests. Each has to be given the entity of the
  player it acts for, which a test knows from the join or the transfer. This is the
  largest mechanical change of the step.
- `Welcome`: `RegionRunner::tick`; `Fanout::welcomed`; `resume_of` in
  `services/worker/tests/specification.rs` and the book of `Link` in
  `services/worker/tests/chunks.rs`, which count the presence answers by the names of
  the hello; and some eighty literals of the two welcomes in the worker's tests, the
  edge's and `clustine-rpc`.
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

**Existing tests that change their point**, found by reading the tests of joins,
arrivals and leaves in the sim's four test files and the helpers of the worker's
tests that read a resume; "Not checked" says what was not read.

| Test | What becomes of it |
|---|---|
| `a_second_join_of_the_same_player_is_ignored` (`region.rs`); `a_join_through_the_same_edge_is_ignored` (`tests/specification.rs`); the last part of `a_join_through_another_edge_replaces_the_player` (`tests/state.rs`) | A join of a player the region has begins a new stay, under the same edge as under another |
| `a_player_who_is_already_there_does_not_arrive_again` (`region.rs`, its second half, which arrives with entity 50 where the player has 1); `an_arrival_of_a_player_the_region_has_leaves_them_as_they_are` (`tests/specification.rs`); `an_arrival_of_a_player_who_is_there_changes_nothing_whatever_the_chunk` (`tests/chunks.rs`) | Each arrives with an entity id above the player's: that arrival now takes the place of the stay that was there. As written they hold for an arrival with a lower id |
| `a_leave_through_another_edge_keeps_what_the_players_edge_passed_on` (`tests/state.rs`) | It asserts that `change` drops, for a leave, what the player did through that edge. A leave drops nothing; its last part, that a join drops everything, stands |
| `an_ordinary_subscription_holds_nothing` (worker, `tests/specification.rs`) | It sends a move, which is still not held. Its name says more than is so now |
| Tests of the worker whose hello names fewer players than the region has for the edge, and those that expect `Absent` after an `Unknown` that tells the edge again | More presence answers come, and the welcome counts them. No existing test can have a player of an edge in a state that has received nothing: until this step a state has one only by a numbered message of that edge |

Found while building C3.2, and not in the table: in the sim,
`a_player_who_arrives_is_taken_in_unless_the_chunk_is_believed_anothers` (its last
part arrived with a higher entity than the player had), and in `tests/chunks.rs`
`an_arrival_in_the_tick_its_chunk_is_called_anothers_goes_on_although_nothing_wants_the_chunk`
and `a_remote_action_in_the_tick_its_chunk_is_called_anothers_goes_on_to_that_region`
(both used a chunk of the region's own stripe, which is section 2.2's case now); in
the worker, `an_arrival_does_not_take_a_player_from_the_link_they_belong_to` (it
arrived with a higher entity). All 25 tests of the worker in which a player joins more
than once or arrives were read; only that one rested on what changed.

Every test of the hold behind a hello is as it was, and so is every test that sends
a block action about a chunk the region does not hold.

### 10. Building it

Each step leaves the four checks of `CLAUDE.md` green on `main`. The edge's part is
designed after this record; where a step needs the edge to change with it, the table
says so.

| # | Scope | Whose | Needs | The edge in the same commit | Its tests |
|---|---|---|---|---|---|
| C3.1 | The messages and types of section 9 but `RoutingTable::waiting`, and the store's three changes. Everyone else constructs and passes over them: the sim ignores the entity of a leave and of an input, the runner announces `presences` as the number of names in the hello and still logs the store's answers, the coordinator still closes on `Merge` and `Split` | shared; then the store alone | nothing | names the entity in `PlayerLeave` and `Input`; reads the new shapes and still only confirms the two entries | The store's: T1 to T3 below; every existing test |
| C3.2 | The sim: stays (2.1), the doubt in pinned areas (2.2), `absorb`, `take_absorbed`, `pins`, `split`, `take_split` | `crates/clustine-sim` alone | C3.1 | nothing | S19 to S37 |
| C3.3 | The runner: the new hold (3.6), presence for every stay (3.7) | `services/worker` | C3.1; C3.2 for `pins` | nothing: the edge of today passes over a presence answer for a player it does not have or has elsewhere | R23 to R30; the end-to-end tests |
| C3.4 | The runner: `Reshape`, the phases and `stage`, the merge and the split, beginning anew, warm chunks, the part (3.1 to 3.5, 3.8) | `services/worker` | C3.2, C3.3 | nothing | R31 to R47; K1 to K12; the builder's own, B1 to B6 |
| C3.5 | The coordinator's state machine and service: the list, reservations, orders, answers, the routing table with `waiting` (section 5) | `services/coordinator`, `clustine-region` | C3.1 | reads `is_complete` as it is then | Q1 to Q17 |
| C3.6 | The worker process and the commands (section 4, 5.1); `--store` for the coordinator | `bin/clustine`, `deploy/` | C3.4, C3.5 | nothing | P1 to P4, without players |
| C3.7 | The edge (its own record, against section 8) | `services/edge` | C3.3; C3.1 | all of it | Its own; A1 to A9 against scripted regions |
| C3.8 | End to end, under the bots | `bin/clustine/tests` | C3.6, C3.7 | | E1 to E6; kind |

C3.2 and C3.5 share no file and can be built side by side once C3.1 is pushed. C3.3
and C3.4 are both in the worker's one file and follow C3.2. **Before C3.2 and anything
in the worker are built side by side, whoever builds C3.2 reads the worker's tests for
any that rest on a second join being ignored or on an arrival of a player who is
there changing nothing**; none was found by name
(`a_player_who_joins_through_another_link_is_taken_over_by_it` is of another link and
another edge event), and they were not read one by one. C3.7 can begin when C3.3 is
in. **Until C3.7 is built, `clustine merge` and `clustine split` work on the regions
and must not be used with players**: the edge confirms the entries and does nothing.
The tests of C3.6 therefore have no players in them, and the roadmap says nothing to
the owner before C3.8.

**For whoever writes tests from this record alone.** Everything in the lists T, S, R,
K, Q, P, A and E goes through public interfaces. The fixtures are those of ADR-0012,
section 8, and ADR-0011, section 9: the sim driven tick by tick with a test that plays
the store and the edges; the runner stepped by the test (`RegionRunner::step`), with a
`Link` per edge, on `Store::memory_divided` and `local_divided`, told to reshape with
`RegionRunner::reshape` and watched through `RegionRunner::stage`, `region` and
`ended`, and through `Store::regions`. The worlds: **stripes** at a boundary at 1
(region 0 west, with the home chunk; region 1 east), **three stripes** at 0 and 4
where a third region is needed, and **the gap** of ADR-0011. A "crash" of a region is
the region opened again with a higher epoch. The test plays the absorbed region's
worker itself: it runs that region with a runner of its own, releases it, opens it
with a new epoch, reads its state with `clustine_worker::absorbable`, and keeps the
handle. Two things about the fixtures that building the runner showed: **entity-id
blocks are issued in the order regions are first opened**, so a test that opens
region 1 before region 0 gets another first entity than it may expect, and inputs
that name the wrong entity are passed over without a word, though counted as applied;
and "with no step in between" (R23 to R26) can only be had on a direct link, as over
one that serialises the chunk can be there before the dig. Waits are counted in what
the store has answered, not in ticks: a link that is held makes ticks idle.

What cannot be seen or held from outside (what the runner asks of the store and when
the store answers) is not in those lists. It is in **the builder's own tests, B1 to
B6**, in the worker's unit tests, which can put a `Gate` before the store; they are
listed so that they are written, and they are no substitute for the others. The store
killed at a write or a sync of the record is ADR-0011's `kill_regions.rs`, inside the
store's crate, which alone can make a disk fail; a runner under it is not tried, as
the runner sees a lost handle either way, and that is K5 and K10.

*The store*

- T1. `Absorbed` names the areas the absorbed region was pinned to, and none for one
  that was pinned to nothing; `chunks` is empty for a region that held everything by
  being pinned.
- T2. A split that names the next id is done and answered with it; one that names
  another is declined `NotNext` with the next id and changes nothing, and the same
  split with that id and the same tick is then done; a split that is wrong in another
  way as well is declined for that other reason.
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
  another edge. Through `TickInputs::input` and `change`: an input and then a leave
  that names another entity, in one tick: the input is applied. An input and then a
  leave that applies: the player is gone and the input moved nobody. A leave, then a
  join, with an input before the leave: the input is not applied to the new stay.
- S22. An input that names the player's entity is applied; one that names another is
  not, whatever its number, and `last_input` stays; with a stay of entity 2 at
  `last_input` 0, an input numbered 41 that names entity 1 is passed over and an input
  numbered 1 that names entity 2 is applied.
- S23. A region pinned to an area that believes region `r` to hold a chunk of it: an
  arrival for that chunk is taken in, the chunk is `Asked` at the end of that tick and
  in its `claims`; a remote action about it is answered `Remote { to: None }`, and the
  chunk is `Asked` and in the `claims` if something still wants it at the end of that
  tick (the ticket that kept the belief alive), and `Unknown` only if nothing does.
  The same chunk outside any pinned area: `NotMine` with `r`, as before. An arrival of
  an earlier stay, and one through an edge the region does not know, leave the belief
  as it is.
- S24. `absorb` on two states with different players: all of them are in the state it
  gives, as they were.
- S25. A player in both states: the higher entity id stays, whichever side has it.
- S26. Edges: one start in both (the survivor's `since`, `applied` and numbers stay;
  the entry is next, the other's entries behind it under the next numbers, `numbers`
  their old ones, `sent` the last); known to the survivor only (an entry with `since`
  0 and nothing behind it); known to the other only (a state with `since` `M` and
  `applied` 0, the entry numbered 1); the other's start lower (its players of that
  edge do not come in, its entries are not carried over, and the entry is as for an
  edge it did not know); the survivor's start lower (its own players of that edge
  gone, its outbox dropped, `since` `M`, and then as for an edge only the other
  knows).
- S27. After `take_absorbed` the region equals `Region::restore` of the state with,
  as holdings, what it held (chunks of a pinned area that it had claimed among them),
  the chunks named, and its areas with those named. In particular: no ticket and no
  loaded chunk; every chunk it believed any region to hold, the absorbed one or
  another, is `Unknown`; a guest's ticket on a chunk of a newly named area puts the
  chunk into `claims`; none of the chunks named is in `returns` for `return_after`
  ticks. What it returns is every chunk that was loaded, block for block.
- S28. The state `absorb` gives has the survivor's block and next entity id; a join
  after the merge gets the id it would have got.
- S29. `absorb` and `split` change nothing: the region equals its copy from before,
  and ticks on from `T` as the copy does.
- S30. `split` is off with `Nobody` when no named chunk has a player, and when the
  only named chunk with a player is not held, or is the home chunk; and with
  `NothingStays` when nobody would stay in a region that holds no home chunk and is
  pinned to nothing. It is a split when nobody stays in a pinned region, and in one
  that holds the home chunk.
- S31. Who goes: exactly the players standing in named chunks that are held and not
  home; a player in a named chunk that is not held stays.
- S32. Which chunks: with one player going at chunk (10, 0) and one staying at (0, 0),
  of the held chunks in the row z = 0 those with x above 5 go and x = 5 stays, and
  the chunk (6, 7), which is as far from the one as from the other, stays; with the
  home chunk at (0, 0) held and nobody staying, the same; with nobody staying and no
  home chunk in a pinned region, every held chunk goes.
- S33. The states: the part has the players who go as they were, the empty block, and
  for each of their edges a state with the survivor's `start`, `since` `M`, nothing
  applied or sent; the region has lost them and has, for each such edge, one
  `SplitOff` with their entities under the next number.
- S34. After `take_split` the region equals `Region::restore` of its state with what
  it held less the part's chunks, and with the chunks handed in as granted;
  `Part::region` equals `Region::restore` of the part's state with the part's chunks;
  the loaded chunks are divided between what is returned and `Part::chunks`, block
  for block, and none is lost. A player who stays and steps into a chunk of the part
  stays, the chunk is in `claims`, and with `foreign` naming the part they are let go
  to it; a join at the part is refused.
- S35. **Against one region**, merging: two regions on stripes with one `Grants` and a
  router, and one region that holds everything, are given the same joins, walks, digs,
  placements and leaves; at some tick the eastern region's state is absorbed by the
  western one, the test giving `take_absorbed` the chunks and the area as the store
  would, and giving the merged region its tickets again as an edge's hellos do; some
  ticks later the players and every loaded block are the same in both worlds, and
  stay so. With generated runs.
- S36. **Against one region**, splitting: the same with one region that is split and
  then run as two. And a region that is split and whose part is absorbed again has the
  players, blocks and held chunks of one that never was.
- S37. The same region and arguments give byte-identical states from `absorb` and
  from `split`, and identical chunk lists.

*The runner*

- R23. With the gap: a chunk the region was granted for a viewer, whose link then
  unsubscribes (with `return_after` 40, so that it stays held and is no longer
  loaded). With no step in between, a link says `Subscribe` for it, sends a dig into
  it and behind that a move: neither is applied before the snapshot is out; then
  both are, in order, and the block is broken.
- R24. On stripes: a link says `Subscribe` for a chunk of the region's own stripe that
  nothing has asked about, and sends a dig into it behind that, with no step in
  between: the dig waits for the snapshot and breaks the block. (Today it is passed on
  without a region.)
- R25. A dig into a chunk of the other stripe, sent behind a `Subscribe` for it with
  no step in between, is judged in the tick that takes it and passed on without a
  region, as today; and a move behind it is applied in that tick.
- R26. A move behind a `Subscribe` that waits is applied at once; so is a dig into a
  chunk the link has no subscription to.
- R27. A chunk the store cannot read does not hold a dig.
- R28. A region with two players of an edge: a hello that names neither is welcomed
  with `presences: 2` and two `Present`, in ascending order of the players, behind the
  entries; a hello that names one of them and a player who is not there gets `Present`
  and `Absent` for those two, in the hello's order, then `Present` for the other, and
  `presences: 3`.
- R29. A first hello of an edge the region does not know: `presences` is the number
  of names, all `Absent`, and `applied` is 0. A resume after the region applied the
  edge's messages up to `k` and before it took `k + 1`, which waits on the link's
  hold or came too late: the welcome says `applied: k`, and the `Progress` of that
  tick says the same or more.
- R30. A leave that names the entity of a stay a presence answer showed removes it; a
  leave that names another does not.
- R31. A merge on stripes (region 1, with a player and an unconfirmed entry, absorbed
  by region 0): when the outcome `Absorbed` has come, the survivor's link is closed;
  the store's list has region 1 absorbed; a new link whose hello names only what the
  edge had at region 0 is welcomed `Resumed`, with the `Absorbed` and behind it the
  other's entry under the next numbers among the entries, and `Present` for the
  absorbed region's player among the presence answers, though no hello named them; a
  crash of region 0 restores the merged state at `M` with no deltas.
- R32. Numbered messages the survivor's link sent between the stop and the merge are
  not applied: the `Progress` on the new link names the number from before them, and
  when the link sends them again they are applied, once and in order.
- R33. On the new link, behind the welcome's entries: `Subscribe` for a chunk that was
  region 1's and a dig into it. The dig waits for the snapshot and breaks the block,
  in a world of stripes (the chunk is of an area that came with the merge) and with
  the gap, the absorbed region having been granted the chunk.
- R34. A guest's subscription to a chunk of region 1's former area, which the
  survivor had not held, is served after the merge.
- R35. With three stripes: a viewer's subscription at the survivor that was told
  elsewhere with the third region before the merge is, named in the hello after it,
  answered `Elsewhere` with the third region again, and `Region::knowledge` of that
  chunk is `Unknown` right after the merge.
- R36. A declined merge (the test opens region 1 with a still higher epoch before the
  commit): `Off`, the link is open and has been told nothing, what it sent while the
  region stood still is applied by the next tick, and that tick is `M`.
- R37. An edge only region 1 knew says hello to the survivor after the merge:
  `Unknown { since: M, .. }`, the entries being the `Absorbed` and what is behind it
  from number 1, and `Present` for its players.
- R38. A release asked for during the merge ends as `Released` after it, and the next
  owner is restored with the merged state.
- R39. A split on stripes with two players of one edge, one in a named chunk: when the
  outcome has come the link is closed; a new link whose hello names both players and
  chunks on both sides is welcomed `Resumed` with `SplitOff`, naming that player's
  entity, among the entries; `Absent` for that player and `Present` for the other;
  snapshots of the chunks that stay; `Elsewhere` naming the new region for a viewer's
  chunk of the part and `NotMine` for a guest's, each with the number 0; and what the
  link sent behind the hello is applied when all of them are answered.
- R40. The list has the new region with `as_epoch`; a crash of either region restores
  it with the state of the split at `M`.
- R41. A runner made of the part (`of_part`) answers a hello that names nobody with
  `Unknown { since: M, entries: 0, presences: 1 }` and `Present` for the player who
  went, and one that names the player and the part's chunks with a snapshot of each.
- R42. That runner and a runner restored from the store for the same region give the
  same answers to the same hello and inputs.
- R43. `Off::Nobody`, `Off::NothingStays`: the list is as it was, the region ticks on,
  the link stays and is told nothing.
- R44. A split that names a wrong id succeeds with the right one, and the `SplitOff`
  entry names the region the list has.
- R45. After the split, on the new link: a dig by the player who stayed into a chunk
  of the part, sent behind the hello, is passed on with the new region named; an
  arrival for a chunk of the part is let go or sent on to the new region; an input
  that names the entity of the player who went is passed over, and progress still
  covers its number.
- R46. A split of a region that was itself split off (a runner made by `of_part`):
  the second part's runner serves its chunks.
- R47. A region that merges and then splits, and one that splits and then absorbs the
  part again, each with a link that says hello after each: the stays the presence
  answers name are those of the state.

*Kills*, each on the runner with the store in memory and on disk. The test steps the
runner by hand and stops at the point named, by dropping the runner and every handle
it holds (which is what a dead worker leaves) or, for K5 and K10, by opening the
region with a higher epoch (which is what a store that is lost and back leaves the
runner). It then opens every region the list has with a higher epoch, and checks: the
list and the states are **as before** or **as after** as a whole (sections 6 and 7);
every player is in exactly one region's state; everything a link was sent before the
stop is in what the states have; and a link that says hello to each region
afterwards, handles the entries and presence answers as section 8 says, and sends
what it kept, ends with every player where the uninterrupted run has them.

- K1 to K7, a merge. The runner dropped: after the absorbed region's release (K1);
  after that region is opened with `as_epoch` (K2); at the first step with
  `Stage::Settling` (K3); at the first with `Stage::Closing` (K4): all as before.
  The survivor opened by another owner at the first step with `Stage::Closing`, so
  that the runner's handle is lost before it can send the commit (K5): `Off` with
  `StoreLost`, and as before. The runner dropped at the first step with
  `Stage::Committing` (K6): as after, as the commit was sent and the store does what
  a handle asked before it closes it. The runner dropped at the first step after the
  outcome, before any link is taken up (K7): as after.
- K8 to K12, a split, likewise: dropped at the first step with `Stage::Settling`
  (K8) and with `Stage::Closing` (K9), as before; opened by another owner at
  `Stage::Closing` (K10), as before; dropped at the first step with
  `Stage::Committing` (K11), as after, with the part restored from the record by
  whoever opens it; dropped after the outcome with the part never run (K12), as
  after.

  A handle that is lost after the record is on disk and before the answer is taken
  cannot be made from outside, as the answer comes at once; K6 and K11 find what it
  would leave.

*The builder's own*, in the worker's unit tests, with a `Gate` before the store:

- B1. After a merge and after a split, the hello that names the chunks the region had
  loaded is answered with their snapshots without one `Load` reaching the store; so
  is the first hello of a part. A chunk that was given back and granted again is
  asked of the store.
- B2. With the store's answers to loads held back, the runner does not send the
  commit until they are let go.
- B3. A runner stopped while it waits for the store's answer ends as `Abandoned` and
  has published nothing.
- B4. A message taken from a link by a step that did not tick (at the bound of eight
  ticks ahead), and the queue of a hold that ended in the last tick: after tick `M`
  neither is counted as received, and after a merge that is off both are applied.
- B5. With `MAX_RESHAPE_BYTES` lowered: `Off::TooLarge`, nothing sent, the region
  ticks on.
- B6. Warm chunks are forgotten 600 ticks after `M`.

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
- Q5. The release unanswered for a lease: as an overdue release, with the owner noted
  as having failed the region; the asker is told. The survivor losing its owner at
  that stage instead: the absorbed region is taken from its owner and assigned, and
  that owner is not passed over when regions are next given out.
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
  new region (assigned) and one that has not: the asker is told that it is overdue
  either way, also when the new region has the id that was ordered.
- Q11. A list with a region the coordinator does not know adds it without an owner;
  one that has a known region among the absorbed removes it and its owner's
  assignment; one that lacks a region at or above its `next` leaves that region.
- Q12. A registration that reports a region the coordinator does not know keeps it
  with that worker; a later list without it, and `next` above it, takes it away.
- Q13. Nothing is evened out within a lease of a split's end.
- Q14. `is_complete` is false while a known region has no owner, and true for a
  coordinator whose every region has one.
- Q15. A coordinator made anew with a merge at each of its stages behind it, and a
  split, comes to the regions and owners the list and the registrations give. The
  released region of a merge that was at its second stage is assigned when the grace
  period is over, not before.
- Q16. A leaving worker's reserved region is released when the reservation has
  ended.
- Q17. A reading that shows a region the coordinator does not know, applied while a
  split is reserved, adds nothing and assigns nothing; `SplitEnded` then makes it the
  splitting worker's with `as_epoch`. The same reading applied when the reservation
  has run out adds it without an owner.

*The processes, without players* (`clustine coordinator`, `worldstore`, two workers):

- P1. `clustine merge` of the two stripes: the command prints the survivor; the list
  has one region pinned to both areas; `clustine merge` of the home region into the
  other is refused.
- P2. `clustine split` with no player anywhere is told that nobody stands there.
- P3. `clustine move` of the survivor after a merge works; a merge asked during a
  move is refused, and a move during a merge.
- P4. A worker killed while it absorbs: within two leases every region the list has
  is run by someone.

*An edge and the entries*, on the runner with a scripted link for the regions' half,
and again for the edge with scripted regions when it is built. Every one is in the
one setting there is: a hello, the welcome's entries, the presence answers.

- A1. A merge; the next hello to the survivor names only what the edge had there:
  the `Absorbed` and the other's unconfirmed entries are among the welcome's entries,
  and the presence answers have the absorbed region's stays.
- A2. `B` absorbs `C`, then `A` absorbs `B`, with no link meanwhile: `A`'s welcome
  has `Absorbed { B }`, and among the entries behind it `Absorbed { C }` with `C`'s
  entries behind that; `numbers` of each are the numbers its entries had where they
  came from; the presence answers have the stays of all three.
- A3. A split, then the part absorbed by a third region, with no link: `A`'s welcome
  has `SplitOff` with the stay and `Absent` for it if named; the third region's
  welcome has `Absorbed` and `Present` for the stay. Whichever of the two is read
  first, the stay ends at the third region, with the inputs sent since the split
  applied there once.
- A4. A player leaves while there is no link and their region is absorbed: the
  survivor's presence answers have the stay, unnamed; a leave that names its entity
  removes it, and one that names another entity does not.
- A5. That player had walked before leaving (inputs kept for the absorbed region),
  joins again, and the join is taken by the survivor after the merge: the join
  replaces the stay; the earlier stay's inputs, sent to the survivor afterwards with
  the entity they name, move nobody, and the new stay's input numbered 1 is applied;
  the leave with the old entity changes nothing.
- A6. The same for a part: the stay is in the part; an arrival with a higher entity
  replaces it; inputs and a leave that name the old entity change nothing.
- A7. A link that ends between the `Absorbed` and the entries behind it, having said
  in its next hello that it has seen the `Absorbed`: the welcome's entries are the
  rest, without it.
- A8. Three merges and two splits in a row with no link, then a hello to each living
  region, in each order: every stay is `Present` at exactly one region, and that is
  the region whose state has it.
- A9. A split; the split region then forgets the edge (no link for 600 ticks), while
  the part has had a link of that edge that named nobody. The split region's welcome
  is `Unknown` with no entries; a hello to the part still gets `Present` for the stay
  that went, and a leave that names its entity removes it.

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
  one entity and can act, and stands where it joined.

## What a player notices

Nothing here is measured but the move it is compared with (0.36 to 0.40 s, optimised).
A region ticks every 50 ms and takes from its links once a tick, so what follows the
record is counted in ticks, not in what the disk takes.

**A merge, for the survivor's players**:

1. The stop: the wait for the commits of at most eight ticks, a checkpoint of what
   changed in a few ticks, and the record with its two syncs. Tens of milliseconds;
   the runner looks at the store every millisecond while it waits.
2. The links are closed. The edge links again at once (it tries when a link ends and
   then every 20 ms) and says hello; the survivor takes the hello with its next step,
   up to one tick later.
3. The resume: the tick that takes the hello counts the tickets and asks storage for
   the chunks the region holds, which are warm; the next tick has them and makes the
   snapshots, which ends the hold; the tick after applies what the players did. Two to
   three ticks.

Together some four ticks and the stop: around a quarter of a second, against the one
to three ticks the first version of this record claimed for links that stayed. It is
a resume without a restore and without a chunk read from the store, which is the
smaller part of a move's pause. **This is the price of closing the links**, paid at
every merge by the players already there, and by those who stay at every split.

**For the absorbed region's players**: its release without the first checkpoint,
which is made while it ticks (tens of milliseconds); the word to the coordinator and
the order to the survivor's worker; a hello for the absorbed region, which the store
answers without its thread for chunks unless the release failed; the survivor's first
checkpoint, which `Prepare` has made small (without `Prepare` it would be most of the
0.25 to 0.31 s that `clustine move` takes, and it can be minutes of chunks); then
everything the survivor's players wait for. So they walk again when the survivor's
players do, a move's pause or a little less after their region stopped.

**What they do to blocks waits longer.** The survivor has to load their chunks, which
the absorbed region had in memory a moment before. The edge asks for them on the new
link, behind the hello, so the asking is taken when the hello's hold ends; in a world
of pinned regions the answer to the merge names no chunks, only areas, so each chunk
is claimed first (a tick or two, answered from the store's table) and then read from
the store, some hundred chunks per player asked in one tick; where the absorbed region
was granted its chunks, the claim is spared. An action waits for its own chunk (rule
34): several ticks more than a move.

**And behind such an action waits everyone on that link.** While the first dig of an
absorbed player waits for its chunk, nothing the edge sent the survivor behind it is
taken, the moves of the survivor's own players among it. So whenever somebody of the
absorbed region was digging, everyone of that edge in the survivor stands still for
those ticks more. This is the part of the design a player is most likely to feel;
open question 1 has the remedies.

**A split, for those who stay**: the stop, with the store writing the new region's
file before it answers, and the resume, as for the survivor of a merge. **For those
who went**: the stop; the worker's word to the coordinator, the routing table, the
edge's link to the new region; a hello for the region at the store; and a resume
whose chunks are warm. That is the linking and the resume of a move without its
restore. The worker process learns of the split by a call from the region's thread,
so none of this waits for the quarter of a second at which it otherwise looks at its
regions.

A part is left where it was made for a lease before regions are evened out, so that
its players are not stood still twice running.

## Ruled out

- **Keeping the links open** through a merge and a split, with the entries published
  on the links that stand. The first version of this record had it. It spares the
  survivor's players, and those who stay at a split, one resume: by the count above
  about four ticks, where links that stay would cost one to three. Its price is that
  `Absorbed` and `SplitOff` have to be right in two settings, of which the welcome is
  needed anyway; the second, a link with live numbers, served chunks and messages on
  their way, is where the review found three of its defects (what a merge does to
  subscriptions told elsewhere with a region that went into the absorbed one earlier;
  a name that stands for another before the edge has moved anything; what the runner
  has taken from a link and not ticked when it stops), and the edge is where ordering
  mistakes have hidden in this project every time. It can be a step of its own, with
  a measurement of E1 in hand, as ADR-0010 said.
- **Holding less behind a hello** (ADR-0009's open point). See section 3.6.
- **Holding every block action whose chunk the link still waits for**, as the first
  version had it. It stopped everybody on a link for the click behind one's back
  after an ordinary hand-over. See section 3.6.
- **A merge without a release, on one worker**, with the absorbed region's runner
  handing its state and its loaded chunks over in memory. It would spare the absorbed
  region's players the release and the loading. It is a second way for a region's
  state to get from one runner to another, and the merge across workers needs the
  first one anyway. If the loading shows, this is the way back.
- **The store choosing the new region's id**, as built. The id is in the state the
  record holds. Also ruled out: an entry that names the part by its epoch (an epoch
  changes with the next move), and an id fixed up when the state is read (two forms of
  one state).
- **An answer to a merge that names what the absorbed region held by its areas.** The
  store does not know it (section 9).
- **Telling a pinned region that a chunk has come back**, by a reply of the store
  nobody asked for; and **keeping a part from giving back chunks of another's area**,
  which would change what ADR-0011 built and tests. The doubt of section 2.2 needs
  neither.
- **Forgetting only the beliefs that name the absorbed region.** A belief can name a
  region that went into it earlier, and the proof that beliefs form no ring wants the
  survivor to believe nothing older than the merge.
- **Rewriting entries that name the absorbed region.** The edge may have seen them
  under their numbers, and has to know what went into what in any case, for what
  third regions say.
- **Leaves sent wherever a stay might be**, by the edge, when it reads an entry or
  gives a player up. The region saying whom it has, at every hello, does all of it
  and is right whatever was lost.
- **An edge that keeps, with each input, whose stay it was, and leaves out those of a
  stay it no longer has.** It can leave them out of what it moves from one region's
  numbers to another's. It cannot leave them out of what it kept for a region under
  that region's own numbers: a number that is missing ends the link (ADR-0008,
  section 4). So the input names its stay and the region passes it over.
- **Players in the `Absorbed` entry.** See section 2.3.
- **Sending orders again for a split.** A merge taken twice is refused by the store
  the second time; a split taken twice is two splits.
- **Loading what came with a merge before the merge**, by the survivor's worker
  reading the absorbed region's chunks while it prepares. Only the holder loads a
  chunk.
- **A public test support for holding the store's answers back.** What needs it is in
  the builder's own tests; `stage` is the one thing made public for the others.

## Consequences

- A merge and a split are, to the region and its runner, a restore without leaving
  the worker, and to the store one record. Whatever dies, a region is restored as
  before or as after, and an edge cannot tell a survivor whose worker died from one
  whose worker lived but by the time it takes.
- Every merge is a resume for the survivor's players and every split one for those
  who stay: about four ticks.
- After a merge the survivor loads from the store what the absorbed region had in
  memory. Until a chunk is there, what is done to its blocks waits, and with it
  everything that edge sent the survivor behind it.
- A region can be pinned to several areas and learns the new ones at the merge.
- The coordinator needs the world store's address, reads its list, and knows regions
  the layout does not have.
- What ADR-0008 said of joins, arrivals and leaves changes; a leave and an input are
  a field longer on the wire, and a welcome counts its presence answers.
- At every hello a region says every stay it has for the edge.
- A part comes to be on the worker that split its region; evening out moves it later.
- The home region can absorb and be split, and never loses the home chunk; a player
  who joins while home's players are a part elsewhere joins a region that may have
  nobody else in it.

## Changes to ADR-0010

1. **Section 4, step 5, "If `A` has a player already, `A`'s stays"**: the later stay
   stays, by the entity id.
2. **Section 4, `Absorbed`**: `knew` becomes `since`, the number a welcome of the
   absorbed region would have said; `players` goes, and the presence answers of the
   welcome take its place.
3. **Section 4, what the edge does**: it does not end the link to say hello again;
   "what concerns a player who is not `B`'s any more" is told by the stay each
   message names.
4. **Section 4, step 3, "checkpoints it if its log is not empty"**: by a checkpoint of
   the state alone, as the store has replayed the blocks.
5. **Section 4, step 7**: the survivor forgets what it believed of other regions and
   has no loaded chunk and no ticket afterwards: it begins as a restored region does.
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
   `Elsewhere`"**: it tells them nothing and closes them; the next link is answered by
   the ordinary rules.
10. **Section 3, "A region that has been absorbed stands for the one it went into"**:
    to an edge, from when it has handled the `Absorbed`.
11. **Section 6**: the list is read on events in C3 and on a timer from C4; a worker
    does not report what it is in the middle of; a region a worker reports that the
    coordinator does not know is taken as living; a region the list shows while a
    split is reserved is left to that reservation.
12. **Section 3, "Knowing an edge again"**: presence is answered for every stay, and
    from the state also after an `Unknown` that tells the edge the `since` the region
    already had.

## Changes to ADR-0011

1. **Section 3.6, step 6, and open question 10**: the answer to a merge carries the
   areas.
2. **Section 3.7, steps 2 and 3**: the request names the new region's id, and the
   store declines one that is not the next (`Decline::NotNext`), looked at last.
3. **Section 5**: the list carries the next region id.
4. **Section 7, "`Absorbed` would end it too"**: the worker drops the region and tells
   the coordinator.

## Changes to ADR-0012

1. **Section 1.3**: a merge and a split make a region what a restore makes it: every
   belief and everything asked is forgotten. And in its own pinned areas a region
   drops a belief when a player or an action is sent to it for the chunk (section 2.2
   here).
2. **Section 2.2, step 2**, an arrival for a chunk believed another's: not in the
   region's own pinned areas. **Section 2.4**, likewise for a remote action.
3. **Section 2.2, "No player goes round in circles"**: the proof is carried on in rule
   49. The first two risks are closed by sections 2.2 to 2.4 here; the third by the
   links being closed, so that no chunk changes hands under a subscription, and by
   rule 34.
4. **Section 4.5**: the hold behind a hello stays; a second, narrow hold is added
   (rule 34). **A hello's presence answers**: for every stay, counted by the welcome,
   and from the state after an `Unknown` that tells the edge again.
5. **Open question 4**: `NotHeld` goes on ending the runner (section 3.2 here).
6. **Section 4.1**: `STATE_FORMAT` 3.
7. **Section 5.5, rule 23**: an input is also passed over when it names a stay the
   region does not have.

## Changes to ADR-0008, ADR-0009 and ADR-0013

1. **ADR-0008, section 2**: a join of a player the region has under the same edge is
   not ignored; an arrival of a player the region has can replace them; a leave can
   name an entity and an input does; a leave drops nothing of what waits for the tick.
   **Section 4**: `PlayerLeave` and `Input` carry the entity; the welcome counts its
   presence answers, and there is one for every stay.
2. **ADR-0009, section 1, step 4**: a region released for a merge is not assigned.
   **Section 5**: the resume is not changed; the pause is 0.4 s with an optimised
   build. **Section 7**: nothing is evened out during a merge or a split or within a
   lease of one.
3. **ADR-0013, section 4, step 1**: a `Departed` to the region it came from is an
   arrival there when that is what an absorbed region stands for. **Section 6**: a
   presence answer can be for a player the hello did not name, and can move a stay.
   The rest of what the edge does is its own record's.

## Open questions

1. **Whether the loading after a merge shows**, and the wait of everyone on the link
   behind an absorbed player's dig. If the bots of E1 wait noticeably longer than at
   a move, the remedies, in the order of their cost: ask the store for the chunks
   nearest the players first (a player reaches six blocks; the runner asks in
   ascending order today); have the edge send what it kept for the absorbed region
   last, behind what the survivor's own players did; the merge on one worker that is
   ruled out above; the links kept open, likewise.
2. **One lease for a whole merge.** A release whose first checkpoint is minutes of
   changed chunks can take longer than a lease today, and then ends as a failed move
   does. The merge inherits that.
3. **Whether `Prepare` earns its message.** It is worth the survivor's first
   checkpoint to the absorbed region's players, by reasoning and not by measurement.
4. **How long an edge needs the absorbed pairs.** The routing table passes on all the
   store keeps (4096). The edge needs a pair only to know where an `Absorbed` will
   come from and to conclude that none does.
5. **How large a state gets** was reckoned, not measured (section 3.2).
6. **What C4 names as a group's chunks.** Section 2.4 leaves the margin to it.
7. **Whether a split should refuse a part that would leave the home region without
   the chunks around the spawn point that a joining player sees.** Here they go to
   the part if somebody who goes is nearer; the joining player is shown them by the
   part, as a guest.
8. **600 ticks for a warm chunk** is the time after which an edge is gone, taken for
   want of a better number.
- **An arrival still drops every input of its player that waits for the tick**
  (`TickInputs::change`, as before this step), whatever stay those inputs name. An
  arrival of an earlier stay, which the tick then passes over, would so eat that
  tick's inputs of the later stay that is there, as a leave did before review item 7.
  With one edge it cannot be reached: the edge sends no arrival for a stay it no
  longer has, and what it sends a region about a later stay is behind what it sent
  about an earlier one. With several edges it can, and the rule should then be that
  an arrival drops only the waiting inputs that name its own entity. Found while
  building C3.2; left for the milestone that has several edges.

## Risks

- **The edge is designed after this.** Section 8 asks more of it than section 5 of
  ADR-0012 did: stays by entity, names that stand for other names from a certain
  moment on, presence answers that move a stay, an order of steps inside one entry
  (rule 42). It asks it in one setting, the welcome. A rule that is wrong there is
  found only by the edge's own tests, A1 to A9 and E1 to E6.
- **The later stay is told by the entity id.** That holds while one region with one
  block joins every player. A second joining region, or ids for anything but players
  given out away from home (ADR-0010 leaves that open), needs another order of stays.
- **A stale leave without an entity.** A player who quits before they are told their
  entity is removed whatever their entity. That leave goes to the home region, in the
  same stream as their join, so nothing can come between; if joins ever go elsewhere,
  it can.
- **The new hold stops a link behind one action**, for as long as a chunk takes to be
  claimed and read. After a merge that is certain to happen whenever an absorbed
  player was acting on blocks.
- **Warm chunks rest on "only the holder saves" and on nothing being unsaved at tick
  `M`.** If either ever fails, a region serves what the store no longer has.
- **An entity on its way in an outbox that a merge drops is reported to nobody**
  (section 2.3). With one edge, the edge whose outbox it was has started anew and
  shows nothing from before. With several it is a ghost on the others.
- **Two readings of the list in a row can be in either order of truth** only if they
  are applied out of the order they were asked in; the service asks one at a time for
  that reason. A reading older than a split it does not show is told by `next`.
- **Tests without players (C3.6) cannot show a split that does anything**, as a split
  needs a player. Between C3.6 and C3.8 the split is covered by the runner's tests
  alone.
- **The largest mechanical change is the entity in every input** of every fixture of
  the sim and the worker. It is the kind of change in which a test is made to pass by
  naming whatever entity the region has, and stops testing.

## Not checked

What this record says of the code and its author did not verify, or verified only in
part. The review read the edge whole but for its tests, and confirmed the context,
that a `Loaded` reaches the handle before the `Flushed` asked behind it (over TCP
too), and that nothing in the edge relies on a block action being judged at once
behind a `Subscribe`.

- **The tests of the sim** were read where they join, arrive or leave a player who is
  there, and where they call `TickInputs::change`; section 9 names what was found.
  The rest of those files (some ten thousand lines of tests) was searched by name and
  by those calls, not read.
- **The tests of the worker** were read at the helpers that take a resume apart
  (`resume_of`, the book of `Link`, `the_resume_comes_first_and_in_order`) and at the
  tests of the hold behind a hello; they were searched for a dig sent behind a
  `Subscribe` in one step into a chunk the region holds and has not loaded, or of its
  own stripe and not yet asked about, and none was found by its name or its comments.
  They were not read one by one, and the generated runs of `tests/chunks.rs` were not
  read for what their scripts can produce.
- **The edge's tests** were not read.
- **That the edge takes no input from a player before it has told them their
  entity** was not read in `play.rs`. `Command::Input` in `fanout.rs` does not look.
  Section 8.9 leaves it to the edge's record.
- **The end-to-end tests** in `bin/clustine/tests` were not read.
- **The times under "What a player notices"** are counted in ticks from the code of
  `RegionRunner::step` and `run`, and reasoned from one measurement of a move.
- **That the store does what a handle asked before it closes it**, on which K6 and
  K11 rest, was read in `Lanes::close` (it ends the group first) for a handle in the
  store's process; that a connection's requests are all taken before its end is acted
  on was not read in `tcp.rs`.
- **`deploy/`** was listed, not read.

## Review

An independent review against the code found fourteen defects in the first version of
this record, and found sound: its account of the code, the store's three changes, the
sim's `absorb` and `split`, `NotHeld`, the first two risks of ADR-0012, the
coordinator's stages, the two tables of deaths, and the order of stays by entity id.
What it found, and what was decided:

1. An earlier stay's inputs, kept for the absorbed region, were sent to the survivor
   and applied to the player's later stay. An input names its stay, and the region
   passes over one for a stay it does not have. (The review proposed that the edge
   leave such inputs out; it cannot where they are kept under the region's own
   numbers.)
2. A stay could be left for good in a region the edge never told: a part's player
   whose edge was cut off from the split region for half a minute. The region says
   every stay it has at every hello, and the edge ends those it does not have.
3. Only beliefs and subscriptions naming the region absorbed last were doubted; one
   naming a region that had gone into it earlier was left, and the edge waited for an
   answer that never came. The survivor forgets every belief, and with the links
   closed every subscription begins anew.
4. The record said that when a runner stops, nothing a link sent waits untaken. Two
   paths leave something. Nothing rests on it now: at tick `M` the runner begins
   anew, and what no tick took is sent again.
5. An absorbed region stood for its survivor as soon as the routing table said so,
   before the edge had moved its subscriptions, so an action could be judged on a
   chunk not loaded. It stands for it once the edge has handled the `Absorbed`.
6. A stay taken from another region than the absorbed one had its view and its inputs
   left behind. A presence answer that moves a stay moves both (rule 38).
7. A leave for a stay the region does not have ate that tick's inputs of the stay
   that is there. A leave drops nothing in `TickInputs::change`.
8. A split answered waiting subscriptions outside a tick and left the hello's hold
   on. A split answers no subscription any more.
9. A third of the runner's scenarios needed sight of the runner's phase and a gate
   before the store, which only its unit tests have. `stage` is public; what needs
   the gate is listed as the builder's own.
10. "What a player notices" left out the wait of everyone on a link behind one held
    action, that a pinned world claims before it loads, and that the worker process
    would have learned of a split by looking every quarter of a second. All three are
    in it now, and the outcome comes by a call.
11. Three claims that were not so: that a new coordinator hears `Released` for a
    region released before it started; a test whose point changes that was not named;
    and the price put on closing the links (one resume, not two).
12. A reading of the list between a split's record and the worker's word gave the
    part to another worker. Such a region is left to the reservation.
13. The new hold stopped everybody on a link for the click behind one's back after
    an ordinary hand-over. It holds only for a chunk the region is itself about to
    serve.
14. Smaller things: which step changes `is_complete`; orders that name a part with
    another epoch; what whoever builds the sim's step has to look at in the worker's
    tests first.

Its largest finding was not a defect: that keeping the links open cost more than it
bought. The links are closed at tick `M`, as ADR-0010 had it, which removed from this
record an order of a tick on a standing link, a second answer to a served
subscription, and a second setting for everything in section 8; and the region and
the runner after a merge or a split became, exactly, a region and a runner after a
restore. Its doubts were taken up as well: the ring that rule 39 could close is ruled
out by the survivor forgetting what it believed (rule 49); a leave that follows from
what a region said names the region's entity (rule 36); and a merge or a split too
large for a message is off before it is sent (section 3.2).
