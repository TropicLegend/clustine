# ADR-0012: The tick on chunks

- Status: **Accepted**; the design of step C2b of milestone M3, phase C, for the
  simulation and the region runner. Revised after an independent review against the
  code (see the end). Not built yet. The edge's side of the step is designed
  elsewhere, against section 5 of this record.
- Date: 2026-10-08

## Context

[ADR-0010](0010-regions-that-follow-players.md) makes a region a set of chunks that the
world store grants, and says in a page (sections 2 and 3) what the tick and the links
do then. [ADR-0011](0011-the-world-store-and-regions.md) says how the store does its
part: `Claim { chunks }` answered `Claimed { granted, foreign }`, `Return { chunks }`
behind the saves, loads and saves by the holder only, `Restored::held` and
`Restored::pinned`. This record says how `crates/clustine-sim` and `services/worker`
work on that, while the world is still two or three pinned regions side by side.

What the code is today, as far as this record changes it:

- **The sim knows an area.** `RegionConfig::area` is a `ChunkArea`. `Region::tick`
  (`region.rs`) runs `update_chunks` first, which counts a ticket only for a chunk of
  the area; then `edges`, `applied`, `player_changes`, `remote_actions` and `inputs`;
  then it lets go every player whose chunk is outside the area, with
  `Durable::Departed { player, transfer }`. `apply_input` applies nothing more of a
  player once they stand outside the area, and passes a `Dig` or a `UseItemOn` about a
  block outside the area on as `Durable::Remote(action)`. `apply_remote` takes a step
  whatever chunk it is about (a block that is not loaded is not changed) and passes
  the rest of a placement on when its target is outside the area.
- **Nothing of chunks is in a `RegionState`**: tickets, loaded chunks and what was
  asked of storage (`Region::tickets`, `chunks`, `requested`) are rebuilt after a
  restore. A tick's inputs have `granted`, `foreign` and `unbelieve` and its output
  `claims` and `returns` since step C0; nothing fills or reads them.
- **`Region::drop_edge`**, when an edge is reset or gone, reports removed the entity of
  every `Durable::Departed` in the outbox it drops, and of nothing else in it; the
  runner works the same set out as `orphaned` in `RegionRunner::tick`.
- **The runner** (`RegionRunner` in `services/worker/src/lib.rs`) keeps, per link, the
  chunks it is subscribed to and those it awaits a snapshot of
  (`EdgeLink::subscriptions`, `awaiting_snapshot`). `RegionRunner::subscribe` passes
  over a chunk outside `Region::area()` without a word. Each subscription is one
  ticket. `RegionRunner::release` saves a chunk when the last link lets go of it,
  before the tick that drops it. After a tick the runner asks the store for the loads,
  sends the `Commit`, checkpoints if it is time, and makes ready what links are to
  hear, which `publish_committed` sends once the commit is confirmed; a tick that has
  no commit is published as soon as the ticks before it are. `take_replies` logs
  `Claimed` as an answer to what was not asked.
- **`EdgeToWorker::SubscribeAsGuest`** is said to be ignored by a worker. It is not: in
  `RegionRunner::accept` it falls into the arm for numbered messages, has no number,
  and ends the link as a numbered message before a hello does. No edge sends it.
- **A hello** names `edge`, `start`, `seen`, `players` and `chunks`.
  `RegionRunner::hello` turns it into `EdgeEvent::Started` and
  `EdgeEvent::Confirmed { number: seen }`, holds what the link sends until the
  snapshots of the hello's chunks are made (`EdgeLink::hold`; `RegionRunner::drain`
  puts every message of a held link aside), and answers `Welcome::Resumed` if the
  region knew the edge with that start (`KnownEdge::settled`), else
  `Welcome::Unknown`. A link whose edge was not known takes numbered messages only
  from the number after the last the region received (`EdgeLink::unknown`) and drops
  the others without a word.
- **An edge without a link for 600 ticks is forgotten** with its players
  (`EdgeEvent::Gone`, from `RegionRunner::tick`).
- **The edge** (`services/edge/src/fanout.rs`) finds the region of a chunk, of a player
  who was let go and of a remote action with `Layout::region_of`, keeps a link to every
  region of its layout, and logs `Elsewhere`, `NotMine` and the outbox entries of
  ADR-0010 as things it does not act on.
- **States are postcard bytes** (`restored_state`, and `postcard::to_stdvec` in
  `RegionRunner::tick` and `checkpoint`), with nothing in front that says which shape
  they have.

## Decision

Words in `code` are names in the code, or will be. "The store says" always means an
answer of the world store that is durable when it is given (ADR-0011, section 3.2).

### 1. What a region knows of chunks

#### 1.1 Four things it can know

Of every chunk the region is in exactly one of these, which `Region::knowledge(chunk)`
tells as a `Knowledge`:

| | Meaning |
|---|---|
| `Held` | The store has granted it the chunk, and it has not given it back |
| `Asked` | It has put the chunk into a tick's `claims` and has had no answer |
| `Foreign(region)` | The store has said that `region` holds it, and the region still wants to know |
| `Unknown` | Anything else: the region has had nothing to do with it, or has let go of what it knew |

A region never works out who holds a chunk. Its pinned areas (below) say only which
chunks it may claim for a guest and never gives back; a chunk of them is `Unknown`
like any other until the store has answered a claim of it (ADR-0011, section 2).

#### 1.2 Tickets, and what a region needs, wants, uses and keeps

A **ticket** is a link's subscription to a chunk, as today, and is now of one of two
kinds (`Ticket::Viewer`, `Ticket::Guest`): a viewer's, asked for with `Subscribe` by an
edge for a viewer whose player is this region's, and a guest's, asked for with
`SubscribeAsGuest`. The region counts both per chunk, on every chunk whatever it knows
of it: a ticket is no longer passed over because the chunk is elsewhere.

With "a player stands in a chunk" meaning a player of the region whose pose is in it:

- The region **needs** a chunk while a player stands in it or it has a viewer's
  ticket. A need is a reason to claim.
- It **wants** a chunk while it needs it, or the chunk is in one of its pinned areas
  and has a guest's ticket. What it wants and knows nothing of, it claims; what it
  believes of a chunk it keeps for as long as it wants the chunk.
- It **uses** a chunk while a player stands in it or it has a ticket of either kind.
  A chunk that is used is not given back. So a guest's ticket is no reason to claim a
  chunk outside the pinned areas, and a reason to keep one.
- It **keeps** a chunk of its pinned areas, and the chunk `RegionConfig::spawn` is in:
  such a chunk is never given back. The second is the home chunk, which only the home
  region can hold; the store would leave it out of a return with a warning (ADR-0011,
  section 3.3).

#### 1.3 Every transition

| From | When | To |
|---|---|---|
| any | the region is created or restored | `Held` if `Holdings::held` names the chunk, else `Unknown` |
| `Unknown` | the chunk is wanted at the end of a tick | `Asked`, and the chunk is in that tick's `claims` |
| `Asked`, `Unknown`, `Foreign` | `granted` names it | `Held` |
| `Asked`, `Unknown`, `Foreign` | `foreign` names it with `r`, and it is wanted at the end of that tick | `Foreign(r)` |
| `Asked`, `Unknown`, `Foreign` | `foreign` names it, and it is not wanted at the end of that tick | `Unknown` |
| `Foreign(r)` | `unbelieve` names it with `r` | `Unknown`; `Asked` at the end of that tick if wanted |
| `Foreign(r)` | it is not wanted at the end of a tick | `Unknown` |
| `Held` | its time has come (below): it is not kept and has not been used for `RegionConfig::return_after` ticks | `Unknown`, and the chunk is in that tick's `returns` |
| `Held` | `foreign` or `unbelieve` names it | `Held`: ignored |
| `Foreign(r)` | `unbelieve` names it with another region | `Foreign(r)`: ignored |

Between a `foreign` and the end of its tick the chunk is `Foreign(r)` in either case,
so that a player who stands in it is let go in that very tick.

At the end of every tick, therefore:

1. no chunk is in two of the three collections the region keeps (held, asked,
   foreign);
2. every wanted chunk is `Held`, `Asked` or `Foreign`;
3. every `Foreign` chunk is wanted;
4. a loaded chunk is `Held` and has a ticket, and so has a chunk asked of storage;
5. a chunk of that tick's `returns` has no ticket, no player in it, and is not loaded.

What triggers each, by cause:

- **A viewer's ticket** makes the chunk needed. If it is `Unknown` it is claimed; if it
  is `Foreign` the belief is kept for as long as the ticket is there.
- **A guest's ticket** makes a chunk wanted only in a pinned area: such a claim takes
  nothing from anyone. Elsewhere it changes nothing the region knows. On a chunk the
  region holds, it keeps the chunk from being given back.
- **A player standing in or walking into a chunk** makes it needed: an `Unknown` chunk
  is claimed at the end of that tick, so the region grows where its players go.
- **What a player does to a block** changes nothing the region knows: a region does
  not claim a chunk because someone clicked into it (section 2.3).
- **`granted` and `foreign`** are the store's answers and are believed whatever the
  region knew, except that a region does not unlearn that it holds a chunk: the store
  never says `foreign` of a chunk the region holds, as a region claims only what it
  does not hold and loses a chunk only by returning it.
- **`unbelieve`** carries the region that is no longer believed, `(ChunkPos,
  RegionId)`. A belief that has changed since is left alone: it is newer than the
  doubt.
- **Returning** forgets the chunk. If it is needed again a tick later it is claimed
  again, which calls the return off at the store if it is not through (ADR-0011,
  section 3.2).
- **Restore** forgets everything but what the store says the region holds.

**The time before a return.** ADR-0010 says what a region needs, and that it gives back
what it does not; it does not say when. Given back at once, a region that is restored,
which has no tickets until its edges have said hello, would return every chunk but
those its players stand in at the end of its first tick, and claim them again a moment
later; so would a region whose only edge lost its link for a second. Each such round
is a `Returned` and a `Granted` record with hundreds of chunks, and a neighbour whose
viewer sees one of them can take it in between. So:

> A chunk is returned at the end of a tick if it is held and not kept, nothing uses it
> at the end of that tick, and nothing used it at the end of any of the `return_after`
> ticks before. The ticks before the chunk was granted, and those before the region
> was created or restored, count as ticks in which it was used.

With `return_after` 0 a chunk goes at the end of the first tick in which nothing uses
it; with 5, five ticks after that one. Being used in between starts the count anew.
After a restore the count starts from the restore, as the time an edge is away does.
The processes use 600 ticks, which is the 30 seconds after which an edge is `Gone`;
tests set 0 unless they are about the time.

**A chunk that someone watches is never given back.** A ticket of either kind is a
use. So in this record a chunk leaves a region only when no link is subscribed to it
and nobody stands in it, and has not for `return_after` ticks; a chunk under
someone's eyes changes hands only by a merge or a split, which are step C3's. There
is no time in which a watched chunk is nobody's.

**A belief is kept only while it is wanted**, so what a region believes is bounded by
what its players see, and follows from what it holds, its tickets and the store's
answers alone: a restored region that is given the same tickets and the same answers
believes the same.

A chunk has at most one claim without an answer at any time: it leaves `Asked` only by
an answer. So an answer must never be lost while the region goes on (section 4.2).

#### 1.4 What is in `RegionState` and what is not

In it, new: `EdgeState::since` (section 2.6), and nothing else.

Not in it: what the region holds, which the store says when the region is opened
(`Restored::held`, as `Holdings::held`); the areas it is pinned to, which the store
says as well (`Restored::pinned`, as `Holdings::pinned`); what it has asked and not
heard; what it believes; tickets; loaded chunks; and since when a chunk has not been
used.

**Why a restore is still exact.** Nothing a restored region lacks was ever shown to
anyone as a fact about the region's state, and all of it comes back by asking:

- what it holds is the store's to say, and the store says it;
- a chunk it had asked for and has since been granted is in `Restored::held` if a grant
  was written, and otherwise (a chunk of a pinned area) is claimed again when it is
  wanted;
- a player who stands in a chunk the region does not hold after the restore stays the
  region's, the chunk is claimed in the first tick, and the answer lets the player go
  or not, as it would have;
- tickets come with the hellos of the edges.

What can differ from the run that was lost is when: a claim is made again, a player is
let go a tick or two later, a chunk is returned later, and an action about a chunk
whose holder the region has not heard again yet is passed on without a region named
(section 2.3) where the lost run named one. Two tests assert today that a restored
region gives the very outputs of the original, and both have to give the restored
region its tickets and the store's answers again, and to start the count of unused
ticks anew, before they compare:
`a_restored_region_carries_on_as_the_original_from_every_tick_of_a_scenario` in
`tests/specification.rs`, which already gives the tickets and the chunk and passes
over `chunk_requests`, and
`a_restored_region_carries_on_as_the_one_it_was_restored_from` in `tests/state.rs`,
which restores from the state alone, loads the chunks and then compares more than 400
random ticks in which players walk across the line.

### 2. The tick

#### 2.1 In order

1. The number of the tick goes up.
2. **Chunks** (`Region::update_chunks`), in this order:
   1. `granted`: each chunk becomes `Held`.
   2. `foreign`: each chunk that is not `Held` becomes `Foreign(region)`.
   3. `unbelieve`: each chunk that is `Foreign` of the region named becomes `Unknown`.
   4. `tickets_added`, then `tickets_removed`, each entry with its kind; additions
      first, as today. A loaded chunk whose last ticket of either kind goes is dropped,
      and no longer asked of storage, as today.
   5. `chunks_loaded`: a chunk is taken if it was asked of storage, is `Held` and has
      a ticket; otherwise it is dropped.
   6. Every `Held` chunk with a ticket that is neither loaded nor asked of storage is
      asked for: `chunk_requests`, in ascending order.
3. `edges`, then `applied`, as today, with `since` (section 2.6).
4. `player_changes`, as today but for arrivals (section 2.2).
5. `remote_actions` (section 2.4).
6. `inputs`, in their order (section 2.3).
7. Moves and acknowledgements are reported, as today.
8. **Players are let go**: every player who stands in a `Foreign(r)` chunk, in the
   order of the players, with `Durable::Departed { player, transfer, to: r }`.
9. **Chunks again**, in this order:
   1. Returns: every `Held` chunk whose time has come (section 1.3) becomes `Unknown`
      and is in `returns`, ascending.
   2. Every `Foreign` chunk that is not wanted becomes `Unknown`.
   3. Every wanted chunk that is `Unknown` becomes `Asked` and is in `claims`,
      ascending.
10. The delta is taken, as today.

A chunk cannot be in `returns` and `claims` of one tick: one is not used and the
other is wanted, and what is wanted is used.

The sim stays deterministic: every collection is ordered, the outputs are in ascending
order of the chunks or in the order of the inputs, and nothing is counted but ticks.

#### 2.2 Players

**A player is the region's** from the tick that takes them in until the tick that lets
them go, and they are let go only by step 8: when the chunk they stand in is `Foreign`.
In a chunk that is `Held`, `Asked` or `Unknown` they stay; an `Unknown` one is claimed
at the end of the tick. What a player does while standing in a `Foreign` chunk is not
applied (the check takes the place of the one for the area in `apply_input`), so a
player who steps into a chunk the region believes another's is let go in that tick
with the `last_input` of the step, as today, and one who stepped into a chunk that was
not answered yet is let go in the tick the answer `foreign` is in, with nothing of
that tick applied.

**A join** is as today: the player is placed at `RegionConfig::spawn` with the next
entity id, and refused with `Durable::Refused` if the region has none left. A region
made by a split has the empty block of entity ids (ADR-0011, section 2), for which
`EntityIds::contains` is false of every id: such a region answers every join with
`Refused`, by the code there is. Only the home region is joined; a join that reaches
another region with ids places the player at the spawn point and lets them go to
whoever holds that chunk, at once if the region knows and else when the store has
said, which is what a stripe without the spawn point does today.

**An arrival** `PlayerChange::Arrive(edge, player, transfer)`, in this order:

1. The region has the player, or does not know the edge: as today (nothing, or the
   entity that was on its way is reported removed).
2. The chunk of `transfer.pose` is `Foreign(r)`: the player is not taken in. The entry
   `Durable::NotMine { what: Misdirected::Arrival { player, transfer }, holder: r }`
   goes to the outbox of `edge`. Nothing else changes and no entity is reported.
3. Otherwise the player is taken in as today, whatever the region knows of the chunk:
   `Held`, `Asked` or `Unknown`. An `Unknown` chunk is claimed at the end of the tick
   because the player stands in it.

A region never makes a `NotMine` for an arrival without a holder. That is against
ADR-0010, which has every arrival for a chunk the region does not hold answered with
`NotMine`, and sent back to where it came from if the region knows no holder. It is
what makes hand-overs to pinned regions work at all: a pinned region does not know
that it holds a chunk of its area until it has claimed it, the store tells a neighbour
that it does all the same, and the neighbour's player arrives for a chunk that is
`Unknown` here. Sent back, the player would be let go to this region again by the
same answer of the store, for ever. Taken in, they make the region ask, and the
answer is `granted`.

**A `NotMine` for an arrival is a player on their way**, as a `Departed` is. When a
reset or `Gone` drops an outbox, `Region::drop_edge` reports the entity of every such
entry removed as it does for a `Departed`, with the chunk of `transfer.pose`, under
the same two conditions (no player of the region has that entity, and it has not been
reported in that tick); and the runner's `orphaned` has those entities as well
(section 4.6). Otherwise an entity that nobody will pass on stays on the screens of
those who saw it leave, which is ADR-0008's review item 11 again for the new entry.

**No player goes round in circles.** A region believes another to hold a chunk only
because the store said so, and stops believing it when it is granted the chunk.
Suppose some regions each believed the next of them to hold one chunk, and the last
the first. Take the one whose answer from the store is the latest. It names a region
that held the chunk at that time; and that region can believe something of the chunk
now only if it was told so after it stopped holding it, which is later than the
latest. So beliefs about a chunk form no ring, and a player passed along them ends,
after at most as many steps as there are regions, at one that takes them in. This
rests on a region that holds a chunk either knowing so or knowing nothing of it, which
holds as long as chunks change hands only by claims and returns; see the risks for
step C3.

#### 2.3 What a player does to blocks

A `Dig` or a `UseItemOn` is judged as today as far as only the player matters: one
that is out of reach, or a placement without a block in hand, is acknowledged and
nothing else. Then, by what the region knows of the chunk the block is in:

| The chunk is | `Dig { position }` | `UseItemOn`: the chunk of `against` | `UseItemOn`: the chunk of `target`, once `against` is found to be a block of a held chunk |
|---|---|---|---|
| `Held` | acknowledged; the block is broken if it is there, as today | if `against` is no block: acknowledged; else see the next column | placed if the spot is free, as today; acknowledged |
| `Foreign(r)` | `Durable::Remote { action, to: Some(r) }` with `RemoteStep::Break`; not acknowledged here | `Remote { action, to: Some(r) }` with `RemoteStep::PlaceAgainst` | `Remote { action, to: Some(r) }` with `RemoteStep::Place` |
| `Asked`, `Unknown` | `Remote { action, to: None }` with `Break`; not acknowledged here | `Remote { action, to: None }` with `PlaceAgainst` | `Remote { action, to: None }` with `Place` |

A block of a `Held` chunk that is not loaded is not there, as today. The input counts
as applied in every row, as one that is passed on does today.

**`to: None` means: to the region that serves this edge the chunk.** The edge knows
that region first-hand, because a client can only click a block of a chunk that some
region sent this edge. Where no region serves the edge the chunk, the edge has the
action acknowledged to its player without effect (section 5.6). The region does not
ask the store because of an action: by the time an answer came the action would be
two ticks old, and a click is no reason to take a chunk.

ADR-0010 has an action on a chunk the region does not hold or has not heard about
"acknowledged without effect, as one on a chunk that is not loaded is today". That is
a seam in ordinary play. A player who walks from region `A` into region `B` arrives in
a region that knows nothing of `A`'s chunks, because it never needed them; the chunks
are on the player's screen, sent by `A`; and the first thing a player who builds along
a boundary does after crossing is to click a block behind them. Passed to the edge
without a region, that click takes the two ticks of any action across a boundary.

The first version of this record had such an input wait inside the region, in a list
in the player's state, until the store had answered. The review found that this did
less than it claimed (after a restore the hold of section 4.5 already keeps inputs
back, and an input whose answer was `granted` was lost all the same, the chunk being
held and not loaded) at the price of a list in the durable state, a player who stands
still for everyone while they wait, and four ticks where two do.

#### 2.4 What players of other regions do to blocks

A remote action `(edge, action)` through an edge the region knows is answered with one
outbox entry for that edge, as today. With `c` the chunk of `action.step.concerns()`:

- **`c` is `Held`**: the step is taken as today. A `PlaceAgainst` whose `against` is a
  block goes on to its `target`, by the chunk of `target`:
  - `Held`: placed if the spot is free, `RemoteDone`;
  - `Foreign(r)`: `Remote { action, to: Some(r) }`, the action being the `Place` step;
  - `Asked` or `Unknown`: `Remote { action, to: None }`, the action being the `Place`
    step.
- **`c` is `Foreign(r)`**: `NotMine { what: Misdirected::Remote(action), holder: r }`,
  the action as it came.
- **`c` is `Asked` or `Unknown`**: `Remote { action, to: None }`, the action as it
  came. The region does not hold the chunk and does not know who does; the edge, which
  was sent the chunk by someone, does.

So a `NotMine` always names a holder: `Durable::NotMine::holder` is a `RegionId`, not
an `Option`. What ADR-0010 calls a `NotMine` without a holder is, for an arrival, never
made (section 2.2), and for a remote action a `Remote` without a region, which the
edge routes like any other.

**Every action ends.** An entry that names a region follows a belief, and beliefs form
no ring (section 2.2). An entry without a region goes to the region that serves the
edge the chunk, which holds it, as a region serves only what it holds and in this
record never stops holding what it serves (section 1.3); the edge sends nothing to
the region the entry came from, and where no other region serves it the chunk it ends
the action itself (section 5.6). `apply_remote` today ends an action for a block the
region does not have at once, so that regions which disagree about who has what
cannot pass it back and forth for ever; the rules here do the same with one more
party, the edge, which is the one that knows.

#### 2.5 What a tick's outbox entries are, and their order

`TickOutput::durable` has, in this order: the entries of step 4, `Refused` and
`NotMine` for an arrival, in the order of `player_changes`; the answers of step 5, one
for one; the `Remote` entries of step 6, in the order of `inputs`; the `Departed`
entries of step 8. Departures are numbered last, as today, so that numbers ascend on a
link in the order the runner publishes them.

`Departed`, `Refused`, a `Remote` of a player's own action and a `NotMine` for an
arrival go to the outbox of the edge the player belongs to or arrived through;
`RemoteDone`, and a `Remote` or `NotMine` that answers a remote action, to that of the
edge the action came through. That is today's rule with the new entries added.

**The removal of an entity that departed and that nobody will pass on** (ADR-0008,
section 4) is reported as today, with the chunk of the pose it left with, for the
entity of a `Departed` and of a `NotMine` for an arrival (section 2.2).

#### 2.6 Since when a region knows an edge

`EdgeState` gains `since: u64`: the number of the tick in which the state was made.
`EdgeEvent::Started` sets it when it notes an edge the region does not know and when it
resets one for a higher start; nothing else changes it. `EdgeDelta` carries it.

A tick's number is good for this. A state is made by a tick that changes the region,
which is committed before anything of it is published, so a `since` an edge has been
told is never given out again: the ticks whose numbers can be used twice are those
that committed nothing (ADR-0008, section 4). No state is made in tick 0, so 0 is what
an edge says that has heard none.

That rests on a region's ticks never going back. A world that is made over begins
every region at tick 1 again (ADR-0011, section 4.2); every service is started anew
with a division, so no edge outlives that with a `since` it was told.

### 3. Loading

- **A chunk is loaded only when held.** Step 2.6 of the tick asks storage only for a
  `Held` chunk with a ticket, of either kind. A ticket on a chunk that is `Asked`,
  `Foreign` or `Unknown` loads nothing; when the chunk is granted, it is asked of
  storage in the same tick.
- **A player does not load a chunk** by standing in it, as today: the viewer's ticket
  of their own edge does.
- **A chunk that arrives for a chunk no longer held**, or no longer ticketed, is
  dropped, as one that arrives after its last ticket went is today.
- **A chunk that is returned is not loaded.** It has no ticket, and a loaded chunk is
  dropped at the start of the tick that takes its last ticket, before anything of that
  tick can change it. Every change the region made to it is in the save that
  `RegionRunner::release` asked for when the last link let go of it, before that tick,
  as today; so the save is before the `Return` on the handle, which is what the store
  relies on (ADR-0011, section 3.3). Nothing new is needed to make a chunk leave
  saved.
- **Tickets on a chunk that turns out to be another's.** A viewer's ticket stays: it is
  the region's reason to go on knowing who holds the chunk. A guest's ticket is taken
  back by the runner, which tells the guest `NotMine` (section 4.4).
- **A block action on a held chunk that is still being loaded** is acknowledged without
  effect, as today.

### 4. The runner

#### 4.1 What a runner is made of

`RegionRunner::restore(config, store, restored)` keeps its shape, through every step
of section 8. It makes the region with `Region::restore(config, state, Holdings {
held, pinned })`, `held` being the chunks of `Restored::held` without their ticks,
which only the store needs, and `pinned` being `Restored::pinned`. `RegionConfig` has
no area any more and gains `return_after`.

**The bytes of a state.** `EdgeState::since`, and `Departed`, `Remote` and `NotMine`
changing their shapes, change what the postcard of a `RegionState` and of a
`StateDelta` is (an outbox is part of both), and old bytes read as the new shape are
not an error in every case: a state without edges reads the same, and one with edges
can read as something else. So, from this step on:

- What the worker hands the store as a state or a delta (`Commit::state`,
  `Checkpoint::state`) is the byte `0x00`, then `STATE_FORMAT` as one byte, then the
  postcard. `STATE_FORMAT` begins at 1 with the first step of this record that changes
  a shape, and is raised by every commit that changes the shape of anything a
  `RegionState` or a `StateDelta` contains, which a second step of this record does
  (section 8). A test with the bytes of one state and one delta written out fails when
  a shape changes, and says that the number has to go up.
- `restored_state` reads each item of a `Restored` so. A state of tick 0 is not read:
  it is that of a region that never ran, `RegionState::new(entity_ids)`. Bytes that
  begin `0x00, STATE_FORMAT` are the postcard behind them, and a failure to read it is
  a `RestoreError` as today. Bytes that begin `0x00` and a higher number are a later
  build's: `RestoreError::Format`. Anything else is **from before**: bytes that do not
  begin with `0x00` (the postcard of a state or a delta begins with its tick, whose
  first byte is `0x00` only for tick 0), or that begin with `0x00` and a lower number.
- If the state or a delta is from before, everything up to the last such item is
  dropped: the region is `RegionState::new(entity_ids)` with `tick` set to that item's
  tick, and the deltas behind it are applied to that. The same result comes out every
  time the region is opened until its next checkpoint replaces what was dropped.

**What that means for a world from before**: it is carried on. Its chunks have every
block, because the store applies the block changes of the commits itself. What is
dropped is who was in each region and what it kept for edges, of which nothing is
lost: every service of a cluster is of one build, so the edge that knew those players
is gone, and its next start would have reset the region for it in any case. Entity ids
are given out from the start of the block again, which nobody still shows.

#### 4.2 The store

**After a tick**, in this order on the region's handle:

1. `Load` for each of `chunk_requests`, as today.
2. The `Commit` of the tick, if it has one, as today.
3. `Return { chunks: returns }`, if there are any.
4. `Claim { chunks: claims }`, if there are any.
5. The checkpoint, if it is time, as today.

A returned chunk needs no save here and is in no later one: it is not loaded (section
3), and `unsaved` holds only loaded chunks. Its last save went out before the tick
that dropped it, so before this `Return`.

A claim does not wait for the commit of its tick, and neither does a return. A grant
made for a tick that is then lost is in `Restored::held` at the next opening, and the
region gives the chunk back if nothing comes to use it; a return made for such a tick
is of a chunk nothing used, and what the store does with it either way (it frees the
chunk, or drops the return with the session) is made good by `Restored::held`.

**Answers** (`RegionRunner::take_replies`):

- `Claimed { granted, foreign }`: both are added to the inputs of the coming tick. The
  store gives them in the order of the claims, so the region hears them in that order.
- **No answer is dropped by a runner that will tick again.** A chunk leaves `Asked`
  only by an answer, so a lost one leaves the chunk asked for good: a subscription to
  it waits for ever, a hello that names it holds its link for ever, a player who
  stands in it is never let go. A runner that has stopped ticking for good (it is
  releasing the region, or has ended) may drop what comes; one that pauses and goes
  on, as the survivor of a merge will in step C3, keeps every answer for its next
  tick.
- `NotHeld { position, holder }`: by this record a region loads and saves only what it
  holds, and holds only what the store has granted and it has not returned. So this
  answer means that the region and the store disagree, and the runner **gives up as
  for a lost store** (`RegionRunner::give_up`): the region is opened again and learns
  what it holds. Step C1.3 handles it as an unreadable chunk; this replaces that.
- The others as today.

**The store need not say that a return is through** (ADR-0011, open question 2). A
returned chunk has no subscriber, so nobody is told anything when it goes, and nobody
waits for it to be free.

**No region commits a change to a chunk it does not hold** (ADR-0011, open question
1): a block is changed only in a loaded chunk, and a loaded chunk is held. The store
may refuse such a commit if it comes to look.

#### 4.3 Subscriptions on a link

**Subscription messages are numbered.** `Subscribe`, `SubscribeAsGuest` and
`Unsubscribe` each carry `ask: u64`, a number the edge counts per link, from 1, each
higher than the one before on that link. The lists of a hello (section 4.5) are the
subscription message with the number 0. A subscription message whose number is not
above that of the one before on its link ends the link, as a numbered message out of
order does; so does a hello that names a chunk on a link that has sent a subscription
message already. These numbers have nothing to do with `EdgeMessage::number`, which
stays `None` for all three.

A link has at most one subscription per chunk. It has a kind, viewer's or guest's; a
**number**, that of the last subscription message of the link that named the chunk;
and one of three conditions: **waiting** for an answer, **served** (the snapshot has
been made), or told **elsewhere** with the region named (a viewer's only).
`EdgeLink::subscriptions` becomes a map from the chunk to all three, and
`awaiting_snapshot` the set of those that wait.

| The link says, with `ask` n | It has for the chunk | The runner |
|---|---|---|
| `Subscribe` | nothing | notes a viewer's subscription, waiting, n; adds a viewer's ticket |
| `Subscribe` | a viewer's, waiting | notes n |
| `Subscribe` | a viewer's, served | notes n; it stays served and nothing is sent |
| `Subscribe` | a viewer's, told elsewhere with `r` | makes it wait again, n; adds `(chunk, r)` to `unbelieve` of the coming tick. This is how an edge asks again |
| `Subscribe` | a guest's, waiting or served | makes it a viewer's, in the condition it is in, n; adds a viewer's ticket and takes a guest's back |
| `SubscribeAsGuest` | nothing | notes a guest's subscription, waiting, n; adds a guest's ticket |
| `SubscribeAsGuest` | a guest's | notes n |
| `SubscribeAsGuest` | a viewer's, waiting or served | makes it a guest's, in the condition it is in, n; adds a guest's ticket and takes a viewer's back |
| `SubscribeAsGuest` | a viewer's, told elsewhere | makes it a guest's, waiting, n; tickets as above |
| `Unsubscribe` | anything | forgets it and takes its ticket back, saving the chunk first if this was its last, as today |
| `Unsubscribe` | nothing | nothing |

A subscription that changes its kind while it is served stays served: there is no new
snapshot and no gap in what the link is told. That is what lets an edge turn a
player's whole view from a viewer's into a guest's, or back, when the player changes
regions, without a chunk being sent again.

A subscription needs no hello, as today. A link that ends gives back every ticket it
had (`RegionRunner::let_go`), as today; the time before a return (section 1.3) is what
keeps the region's chunks until the edge is back.

#### 4.4 What a link is told about its subscriptions

After `Region::tick`:

1. **The tick's events are sorted to the links** as today (`EdgeLink::visible`), by
   the subscriptions that wait or are served, as they were while the tick ran. A
   subscription that was told elsewhere does not count: a region tells a link nothing
   about a chunk between an `Elsewhere` and the link's asking again, except the
   removals of section 4.6.
2. **For each link, for each of its subscriptions that waits**, in ascending order of
   the chunks, by `Region::knowledge` of the chunk:

   | The region | A viewer's | A guest's |
   |---|---|---|
   | holds it, and it is loaded | `ChunkSnapshot`, as today; served | the same |
   | holds it, and it is not loaded | waits | waits |
   | has asked | waits | waits |
   | believes `r` to hold it | `Elsewhere { chunk, ask, region: r }`; told elsewhere with `r`, and the ticket stays | `NotMine { chunk, ask }`; forgotten, the ticket taken back with the coming tick |
   | knows nothing | waits (it cannot be: a viewer's ticket makes the region ask) | `NotMine { chunk, ask }`; forgotten, the ticket taken back with the coming tick |

   Each answer carries `ask`, the subscription's number **as it was when the tick
   ran**: `ChunkSnapshot`, `Elsewhere` and `NotMine` all have it.

So `Region::knowledge` after a tick is how the runner learns from the tick which
subscribed chunks the region does not hold and who does. Looking at what the region
knows, for the subscriptions that wait, covers a link that subscribed before the
answer came and one that subscribes to a chunk the region has long known not to hold
in the same way; a list of what became known in a tick would need a second path for
the latter. A tick's `returns` concern no link: a returned chunk has no subscriber.

**A subscription that is served stays served** until the link ends it or ends. The
region holds a chunk for as long as a ticket is on it, so in this record no served
subscription is ever answered a second time.

**What is left of `NotMine` for a guest** is races, none of which the pinned worlds of
C2b have:

- a guest asks for a chunk of open land that its region gave back, unwatched, while
  the viewer's region was telling the edge `Elsewhere`. Until the store has freed the
  chunk the viewer's region is told `foreign` again, and the edge asks again after a
  short while;
- a guest asks for a chunk that a split has taken from a pinned area, once step C3
  makes splits.

In a world of pinned regions every chunk a region is named for by the store is one it
is granted when it claims it, so a guest is always served.

A subscription that is told elsewhere is not looked at again until the link asks
again. If the region comes to hold the chunk through another link's asking, this link
hears nothing of the chunk until it asks, and then gets the snapshot.

A chunk the store cannot read is waited for without end and holds nothing up, as today
(`RegionRunner::unreadable`). It is the one subscription that is answered with
silence.

All of this is part of what the tick produced and is published with it, in the order
of section 5.2: when its commit is confirmed, or, for a tick that has none, when the
ticks before it are published.

#### 4.5 Hello and welcome

`EdgeToWorker::Hello` gains `since` and `guests`:

- `chunks` are the viewer's subscriptions the link begins with and `guests` the
  guest's, each noted as section 4.3 has it for a link that has nothing, with the
  number 0; a chunk in both is a viewer's. `chunks` is every chunk a viewer of the
  edge whose player is this region's has in view, **also those another region
  serves**: the region has forgotten what it believed if it was restored, and asks
  again for each.
- **The hold.** Every chunk of both lists holds the link, as the hello's chunks do
  today, and is let go of when its subscription is answered: with a snapshot, with
  `Elsewhere` or with `NotMine`, or when the store says the chunk cannot be read. So
  nothing an edge sends again after a restore reaches a tick before the region knows
  of every chunk its players can reach who holds it.
- `since` is the `EdgeState::since` the edge last had from this region in a welcome, 0
  if none.

With "the state" being the region's before the tick that takes the hello, as today,
and "received" being the number of the last numbered message of the edge that the
runner has passed on (`KnownEdge::received`):

- **The state knows the edge with the start of the hello and with its `since`**:
  `Resumed { entries }`. The hello's `seen` is turned into `EdgeEvent::Confirmed`, and
  the outbox entries above `seen` follow the welcome; `entries` is how many.
- **The state knows the edge with that start and another `since`, and nothing has
  been received**: `Unknown { since, entries }` with the state's `since`, and every
  entry of the state's outbox follows, from its first. `seen` is **not** turned into a
  confirmation: it is a number of a numbering the region does not share. This is an
  edge that never read the welcome that told it the `since`. In C2b such a state has
  an empty outbox, as an entry is made only for a numbered message of the edge; the
  case with entries is step C3's, in which a region comes by a state for an edge by
  absorbing another.
- **The state knows the edge with that start and another `since`, and something has
  been received**: the edge has lost its `since`, and nothing it kept can be trusted
  to fit what the region has. The region **resets the edge as for a higher start**
  (ADR-0008, section 2): the runner passes `EdgeEvent::Gone` and then
  `EdgeEvent::Started` into the coming tick, forgets what the edge had sent for it
  (`RegionRunner::forget_inputs_of`), and counts from nothing received. The welcome is
  as in the next case. Without this the link would drop the edge's messages numbered
  from 1 as ones it has had, without a word, up to the number received.
- **The state does not know the edge, or knows it with a lower start**: `Unknown {
  since, entries: 0 }`, `since` being the number of the tick that takes the hello,
  which is what the region gives the state it makes in that tick. `seen` confirms
  nothing.
- `Superseded` as today.

`EdgeEvent::Started` is passed in as today in every case. Presence answers follow the
entries, and are `Absent` after `Unknown`, as today. After `Unknown` the link takes
numbered messages from 1 (`EdgeLink::unknown`, with nothing received in all three
cases).

#### 4.6 Entities nobody will pass on

`orphaned` in `RegionRunner::tick` has the entities of the `Departed` entries and of
the `NotMine` entries for an arrival in an outbox that the tick drops.
`EdgeLink::visible` sends the removal of an entity among them to every link, whatever
the link is subscribed to, when the region does not hold the chunk the removal names
(`Region::knowledge` is not `Held`), in place of "when the chunk is outside the area".

#### 4.7 Checkpoints, release and stopping

A checkpoint is as today: every chunk of `unsaved`, which are loaded and so held, then
the state. A release is as today as well. A region that is released keeps its chunks
(ADR-0010, section 1); the next owner is told them in `Restored::held`, starts the
time before a return anew for each, and gives back what nobody comes to use. What the
released runner had asked and not heard, it drops; a grant among it is in
`Restored::held`. The flush that ends a release is answered only when the returns
before it are through (ADR-0011, section 3.3). A runner that is told to stop
(`RegionRunner::run`) checkpoints and flushes as today, and returns nothing on its way
out.

#### 4.8 Where the players are

`Region::crowds()` gives the chunks with players in them, each with how many, in
ascending order (`clustine_rpc::Crowds`). The runner puts it into `RegionStatus` after
a tick in which it changed, beside `players`, and counts the held chunks there as
well. Nothing sends it until step C4.

### 5. The contract with an edge

This section is what the edge is built against. It says what a region expects of an
edge, the order of what a region sends on a link, what each message means and what
the edge does with it, and what an edge must not assume. "The region" is one region
and "the link" the edge's link to it.

#### 5.1 What the region expects of an edge

1. **A link to every region the routing table lists**, kept or made again whether or
   not the edge has a player or a chunk there, as today. A region forgets an edge that
   has had no link for 600 ticks, with its players; an edge that then says hello is
   told `Unknown` and gives up what it kept for the region (ADR-0008, section 5), also
   an arrival it had queued a moment before. Linking only "to the regions it has to
   do with" (ADR-0010, section 3) is left for when there are many regions; it needs a
   rule for what an edge does with what it queued for a region that has forgotten it,
   and this record makes none.
2. **A hello first**, with the `since` that the last welcome of this region gave it (0
   if none), `chunks` and `guests` being its subscriptions by rules 5 and 6 below, and
   `seen` and `players` as today.
3. **Numbered messages** as ADR-0008 has them, sent when the outbox entries the
   welcome announced have been handled (ADR-0010, section 3).
4. **Subscription messages numbered per link** (section 4.3): `ask` from 1, rising;
   the hello's lists are number 0. The edge remembers, per chunk and link, the number
   of its last message that named the chunk.
5. **A viewer's subscription for every chunk a viewer sees whose player the edge
   believes to be this region's, whoever serves the chunk, and for no other chunk.** A
   viewer's subscription is a claim: the region takes a chunk nobody holds because of
   it and keeps it while it is there. When no such viewer sees a chunk any more
   (the player moved on, left, or was let go to another region), the edge ends the
   viewer's subscription in that same turn:
   - with `SubscribeAsGuest`, if the subscription is served and a viewer of another
     region's player still sees the chunk;
   - with `Unsubscribe` otherwise: if nobody sees the chunk, **or the subscription
     waits**, or it was told elsewhere. A chunk that was waited for and that a viewer
     still sees is asked of that viewer's region, like every chunk of its view.
6. **A guest's subscription in two ways only**: by turning a served viewer's
   subscription into one (rule 5), and at a region that an `Elsewhere` named for the
   chunk, where the edge has no subscription to the chunk yet. In particular the edge
   never says `SubscribeAsGuest` for a chunk at a region where a viewer of that
   region's own players still sees it: that would take the region's need away. The
   edge ends a guest's subscription with `Unsubscribe` when no viewer sees the chunk
   any more, or makes it a viewer's with `Subscribe` when a player who sees it becomes
   this region's.
7. **`Confirm`** for the outbox entries it has handled, as today.

#### 5.2 The order on a link

Ticks are published one by one, in order. Nothing of a tick is on a link before the
commit of that tick and of every tick before it is confirmed; a tick that changed
nothing of the region's state and no block has no commit and is published as soon as
the ticks before it are, which is also the case for a tick whose only product is an
`Elsewhere` or a `NotMine` for a chunk. What one tick produced for a link comes in
this order:

1. if the tick took the link's hello: `Welcome`, then exactly `entries` `Outbox`
   messages in ascending order of their numbers, then one `Presence` for each player
   of the hello;
2. `TickDelta`, if the tick has events for the link;
3. `ToPlayer` with `Spawned`, for those who entered the world;
4. `Outbox` for the tick's entries other than `Departed`, in ascending order of their
   numbers;
5. `ToPlayer` with `Acknowledged`;
6. `Outbox` for the tick's `Departed` entries, which have the tick's highest numbers;
7. the answers to subscriptions, `ChunkSnapshot`, `Elsewhere` and `NotMine`, in
   ascending order of the chunks;
8. `Progress`.

Outbox numbers ascend on a link, in the welcome's entries and after.

#### 5.3 The welcome

- `Resumed { entries }`: the region has the edge's numbering. `seen` was taken; what
  the edge kept goes on from where the region is.
- `Unknown { since, entries }`: it has not. Nothing of the hello's `seen` was taken;
  the entries that follow, if any, are numbered from the region's own first; the edge
  gives up what it kept as ADR-0008 has it, numbers its messages from 1 again, and
  says that `since` in every later hello to this region. In C2b `entries` is 0 after
  `Unknown`.
- An edge that says a `since` the region does not have for it, after the region has
  taken numbered messages from it, is reset: its players in the region are removed.
  An edge never does that as long as it keeps the `since` of the last welcome it read.
- `Superseded`, `Presence`: as ADR-0008 has them.

#### 5.4 Subscriptions

8. **An answer says which asking it answers.** `ChunkSnapshot`, `Elsewhere` and
   `NotMine` carry the number of the last subscription message of the link that named
   the chunk when the tick ran. **The edge passes over an answer whose `ask` is below
   the number of its own last message about that chunk on this link.** Such an answer
   is about a subscription the edge has changed or ended since. Any other answer is
   the region's word on the subscription as the edge last asked for it.
9. **Every asking is answered once or overtaken.** For each chunk that a `Subscribe`,
   a `SubscribeAsGuest` or a hello names, exactly one of these happens:
   - one answer with that message's number comes;
   - a later message of the edge names the chunk before the region has answered, and
     the answer, if any, carries the later number;
   - the subscription was served when the message came (it only changed its kind):
     nothing comes, and it is still served.

   So an edge does not count answers. A chunk the store cannot read is the one
   exception: it is answered with nothing, as today.
10. **A viewer's subscription is answered with a `ChunkSnapshot` or with `Elsewhere`,
    a guest's with a `ChunkSnapshot` or with `NotMine`**, by the kind the subscription
    had when the tick ran. With rule 8 an edge never takes an `Elsewhere` for a
    subscription it holds as a guest's, nor a `NotMine` for one it holds as a
    viewer's: such an answer has a lower number than the message that changed the
    kind.
11. **`ChunkSnapshot`** is the chunk after its tick with the entities in it; the
    subscription is served from then on: every later event of the chunk comes in a
    `TickDelta`. A snapshot of a chunk the edge already shows is reconciled, as
    ADR-0008 has it.
12. **A served subscription stays served** until the edge ends it or the link ends.
    In C2b a region never says `Elsewhere` or `NotMine` for a chunk it has sent a
    snapshot of on that link: it holds a chunk for as long as a link is subscribed to
    it, a guest's subscription included. Changing the kind of a served subscription
    costs no snapshot and loses no event. (Step C3 will end served subscriptions when
    a split or a merge takes a chunk; rule 8 is what makes that safe.)
13. **`Elsewhere { chunk, ask, region }`** is what the store said, at that moment or,
    if the region had it from an earlier answer for another need, then. The edge
    subscribes at `region` as a guest, unless it has a subscription to the chunk
    there already. The viewer's subscription **stays** with the region that said
    `Elsewhere`: it is why that region goes on knowing who holds the chunk, lets a
    player go to the holder in the tick they step into it, and names the holder when
    its players act there. Nothing about the chunk comes on that subscription until
    the edge asks again. The region does not learn by itself that the chunk has
    become free or gone to a third region.
14. **To ask again, the edge says `Subscribe` for the chunk once more.** If the region
    still believes what it last told this link, it asks the store; if it has heard
    otherwise since, it answers with that. One answer with the new number comes.
15. **`NotMine { chunk, ask }`** ends a guest's subscription that was never served:
    the region does not hold the chunk. The edge asks again, as in 14, at every region
    where it has a viewer's subscription to the chunk that was told elsewhere with the
    region that said `NotMine`, after a short while if it has just asked there
    (ADR-0010, section 3): the answer can be `Elsewhere` with the same region once
    more, while the store has not freed a chunk that this region is giving back. In a
    world of pinned regions no guest is told `NotMine` (section 4.4).
16. **A new link begins with nothing**: the hello names every subscription, with the
    number 0. All of them hold what the link sends until each is answered. What a
    region served the edge on a link that has ended, it still serves for the purpose
    of section 5.6, until an answer on the new link says otherwise.
17. **A guest's subscription keeps a chunk with its region.** A region gives back a
    chunk outside its pinned areas only when no link has been subscribed to it and no
    player has stood in it for 30 seconds.

#### 5.5 Players

18. **`Departed { player, transfer, to }`** names the region the store said holds the
    chunk the player stands in. The edge passes the player on to `to` with
    `PlayerArrive`, then every input above `transfer.last_input`, as today, and from
    then on asks `to` as a viewer for the player's view and ends the viewer's
    subscriptions at this region as rule 5 has it. If the edge no longer has the
    player with that entity, it sends `to` a `Discard`, as today.
19. **`to` takes the player in whether or not it has heard that it holds the chunk**,
    unless it believes a third region to hold it; then it answers with the outbox
    entry `NotMine { what: Arrival { player, transfer }, holder }`.
20. **`NotMine` for an arrival is handled as a `Departed` to `holder`**: the arrival
    as it came, then every input above `transfer.last_input`, the player's region
    becomes `holder`, the viewer's subscriptions move; or a `Discard` to `holder` if
    the player is gone. A `NotMine` always names a holder. A player passed on this way
    ends in a region after at most as many steps as there are regions. `to`, or a
    holder, can be the region the player came from two steps ago; it is handled there
    like any other. It is never the region that says it.
21. **A region takes in a player who arrives for a chunk it has had nothing to do
    with**, and claims the chunk. An edge that sends an arrival to the wrong region
    therefore does not lose the player: that region asks the store, and lets the
    player go to the holder.
22. **In the tick a player is let go because the store's answer has just come**, the
    `Departed` is before the `Elsewhere` for the chunk they stand in. By rule 5 the
    edge has ended that subscription, which waited, by the time it reads the
    `Elsewhere`, and by rule 8 passes it over. Usually the `Elsewhere` came long
    before, when the chunk came into view.
23. **`Progress { applied, inputs }`**: every numbered message of the edge up to
    `applied` has been taken into a tick whose commit is confirmed. That says nothing
    about whether an input among them was applied to its player: an input is passed
    over when its player is not the region's, was let go in that tick, stands in a
    chunk the region believes another's, or has a later one applied already. `inputs`
    has, for each of the edge's players whose last applied input changed, its number.
    The edge trims what it keeps for the region by `applied`, and a player's inputs
    only by a `last_input` (from `inputs`, a `Present`, or a transfer), never by
    `applied`. That is as today.
24. **`ToPlayer`** with `Spawned` and `Acknowledged`, `Refused` and `Presence`: as
    today.

#### 5.6 Blocks

A region **serves** an edge a chunk from the snapshot that answers the edge's
subscription to it until the edge ends the subscription or the region says `Elsewhere`
or `NotMine` for it; rule 16 says what holds across links.

25. **`Remote { action, to: Some(r) }`**: the edge passes the action to `r`, as it
    passes a remote action on today.
26. **`Remote { action, to: None }`**: the edge passes the action to the region that
    serves it the chunk of `action.step.concerns()`. If no region does, or the only
    one that does is the region the entry came from, the edge ends the action: it
    acknowledges it to its player as handled, as it does today for a remote action it
    gives up (`Fanout::arrived`), and the player sees the block as it is. If the edge
    no longer has the player, it drops the entry.
27. **`NotMine { what: Remote(action), holder }`**: the edge passes the action to
    `holder`.
28. **`RemoteDone { player, sequence }`**: as today.
29. **Every action that is passed on ends**: in a `RemoteDone` from some region after
    finitely many, or at the edge by rule 26. An entry is handled once, as today
    (`Fanout::outbox`), so what `to: None` means is decided once, when the edge
    handles it.
30. **Until an action that was passed on has ended, later ones of that player are not
    acknowledged**, as today.

#### 5.7 Events and entities

31. **`TickDelta`** has the events of the chunks the link has a subscription to that
    waits or is served, as the subscriptions were when the tick ran. So events come
    for a chunk the edge has no snapshot of yet, and can still come for a chunk it
    has just unsubscribed; an edge takes or passes over both as today. None come for a
    chunk that was told elsewhere.
32. **A player can be a region's while they stand in a chunk another region holds and
    serves the edge**, for the ticks the store takes to answer. Their moves come from
    their own region, and a snapshot of that chunk by its holder does not have them.
    An edge takes what is said of an entity only from the region that last introduced
    it (ADR-0008, section 5), and does not remove one of its own players' entities
    because a snapshot lacks it.
33. **The removal of an entity that departed and that nobody will pass on**, be it
    from a `Departed` or from a `NotMine` for an arrival, comes on every link of the
    region, whatever the link is subscribed to. Only if the region has come to hold
    the chunk the entity was last seen in does it come, like any other event, on the
    links subscribed to that chunk.

#### 5.8 What an edge must not assume

- That a region it has no link to remembers it (rule 1).
- That one `Subscribe` gets one answer: several askings can share one, and a served
  subscription gets none (rule 9).
- That an answer is about the subscription as the edge has it now (rule 8).
- That a region which said `Elsewhere` tells it when that stops being true (rule 13).
- That `applied` means an input took effect (rule 23).
- That the region a `Departed` names knows the player's chunk as its own already
  (rule 19), or that the player's own region is the one that holds the chunk they
  stand in (rule 32).
- That `RemoteDone` means the block changed: it means the action was dealt with, as
  today.

### 6. Changes to messages and types

Beyond step C0, and ADR-0011, section 8.

**`clustine-sim`**

```rust
pub struct RegionConfig {
    pub spawn: Vec3,
    pub starting_hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
    /// How many ticks a chunk the region holds outside its pinned areas may be without
    /// use before the region gives it back.
    pub return_after: u64,
}

/// What the world store says of a region's chunks when the region is opened.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Holdings {
    /// The chunks the region has been granted.
    pub held: Vec<ChunkPos>,
    /// The areas the region is pinned to.
    pub pinned: Vec<ChunkArea>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Knowledge { Held, Asked, Foreign(RegionId), Unknown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ticket { Viewer, Guest }

impl Region {
    pub fn new(config: RegionConfig, entity_ids: EntityIds, holdings: Holdings) -> Self;
    pub fn restore(config: RegionConfig, state: RegionState, holdings: Holdings) -> Self;
    pub fn knowledge(&self, position: ChunkPos) -> Knowledge;
    pub fn held_chunk_count(&self) -> usize;
    pub fn crowds(&self) -> Vec<(ChunkPos, u32)>;
    // `area` goes.
}

pub enum Durable {
    Departed { player: PlayerId, transfer: PlayerTransfer, to: RegionId },
    Refused { player: PlayerId },
    Remote { action: RemoteAction, to: Option<RegionId> },
    RemoteDone { player: PlayerId, sequence: i32 },
    NotMine { what: Misdirected, holder: RegionId },
    // `Absorbed` and `SplitOff` as they are.
}
```

| Type | Change |
|---|---|
| `TickInputs` | `tickets_added` and `tickets_removed` become `Vec<(ChunkPos, Ticket)>`; `unbelieve` becomes `Vec<(ChunkPos, RegionId)>`; `granted` and `foreign` as they are |
| `TickOutput` | `claims` and `returns` as they are, in ascending order; nothing new |
| `Durable::Departed` | gains `to: RegionId` |
| `Durable::Remote` | `Remote(RemoteAction)` becomes `Remote { action: RemoteAction, to: Option<RegionId> }`; `None` is "the region that serves the edge the chunk" |
| `Durable::NotMine` | `holder` becomes `RegionId`, from `Option<RegionId>`; now made |
| `Misdirected` | as it is |
| `EdgeState` | gains `since: u64`, behind `start` |
| `EdgeDelta` | gains `since: u64`, behind `start`; `RegionState::apply` sets it |
| `PlayerState`, `PlayerTransfer`, `PlayerChange`, `RemoteAction`, `RemoteStep`, `EdgeEvent` | as they are |

`PlayerEvent::Departed` and `WorkerToEdge::Remote`, which nothing makes since ADR-0008,
stay as they are for the cleanup the roadmap names.

**`clustine-rpc`**

```rust
EdgeToWorker::Hello {
    edge: EdgeId,
    start: u64,
    /// The `since` of the last welcome the edge had from this region; 0 if none.
    since: u64,
    seen: u64,
    players: Vec<PlayerId>,
    /// The chunks viewers of this region's players see, whoever serves them.
    chunks: Vec<ChunkPos>,
    /// The chunks of this region that viewers of other regions' players see.
    guests: Vec<ChunkPos>,
}
EdgeToWorker::Subscribe { ask: u64, chunks: Vec<ChunkPos> }
EdgeToWorker::SubscribeAsGuest { ask: u64, chunks: Vec<ChunkPos> }
EdgeToWorker::Unsubscribe { ask: u64, chunks: Vec<ChunkPos> }

WorkerToEdge::ChunkSnapshot {
    position: ChunkPos,
    ask: u64,
    tick: u64,
    chunk: Chunk,
    entities: Vec<EntityState>,
}
WorkerToEdge::Elsewhere { chunk: ChunkPos, ask: u64, region: RegionId }
WorkerToEdge::NotMine { chunk: ChunkPos, ask: u64 }

pub enum Welcome {
    Resumed { entries: u32 },
    Unknown { since: u64, entries: u32 },
    Superseded,
}
```

`PlayerArrive` and `EdgeToWorker::Remote` keep their shapes. No message is added: an
edge asks again with `Subscribe`, and the store's interface is ADR-0011's without a
change.

**`services/worker`**: `DEFAULT_RETURN_AFTER: u64 = 30 * 20`; `STATE_FORMAT: u8`, which
is 2 when all of this record is built (section 8); `RestoreError::Format { tick,
format }`; `RegionStatus` gains `held` and the crowds.

**What each breaks**

- `RegionConfig::area`, `Region::new`, `Region::restore`: every fixture of the sim's
  tests (`config`, `fresh`, `region_in`, `joined_in`, `on_floor_in`,
  `east_with_player`, `west_with_floor` in `region.rs`; `config_a`, `config_b`,
  `loaded`, `handing_over`, `three_entries` in `tests/specification.rs`; `AREA`,
  `config`, `generated_region`, `depart`, `entries` in `tests/state.rs`), `config` and
  `runner_of` in the worker's unit tests, `config` in
  `services/worker/tests/specification.rs`, `hold` in `bin/clustine/src/cluster.rs`,
  and `Regions::run`, `run_first` and `started` in `bin/clustine/src/lib.rs`.
- Tickets with a kind: `tickets` in `region.rs`, `load` in `tests/specification.rs`
  and in `tests/state.rs`, and `RegionRunner::subscribe` and `release`.
- `Durable::Departed`, `Durable::Remote` and `Durable::NotMine`: `told`, `requests` and
  `outcomes` in `region.rs`; some thirty places in the sim's and the worker's tests;
  `Region::drop_edge`; `RegionRunner::tick`; `Fanout::outbox` in the edge; the round
  trips in `clustine-rpc/src/link.rs` and `wire.rs`.
- `Welcome` and the hello: `RegionRunner::tick` and `hello`; `Fanout::take_link` and
  `welcomed`; `Link::hello`, `welcome` and some thirty uses in the worker's
  specification tests; the edge's unit tests; `link.rs`.
- `ask` in the subscription messages and their answers: `RegionRunner::accept` and
  `tick`; `Fanout::subscribe`, `unsubscribe` and `handle_region`; every `Subscribe` in
  the worker's unit tests (some thirty) and specification tests (six); `snapshot` in
  both; `link.rs` and `tcp.rs` in `clustine-rpc`.
- `EdgeState` and `EdgeDelta`: `Region::apply_edge_event`, `take_delta`, the sim's
  tests that write an `EdgeState` out.

**Existing tests that change their point**, beyond their fixtures. The step that
changes each is C2b.2 unless said:

| Test | What becomes of it |
|---|---|
| `chunks_outside_the_area_are_neither_requested_nor_loaded` (`region.rs`) | A ticket of either kind on a chunk that is not held loads nothing, and is counted |
| `a_player_is_let_go_exactly_where_the_area_ends` | Where the next chunk is believed another's |
| `a_player_who_arrives_outside_the_area_is_passed_on_unchanged` | Not taken in, and answered `NotMine` with the holder |
| `a_player_who_joins_outside_the_area_is_let_go_at_once` | As it is, with `to` |
| `a_remote_placement_against_a_block_of_this_region_into_another_is_passed_on` | With the region, or with none where the region knows nothing of the spot's chunk |
| `a_remote_placement_into_a_spot_of_another_region_is_dropped_and_not_passed_on` | Not dropped: `NotMine` with the holder, or `Remote` without a region |
| `a_remote_action_for_another_region_is_answered_for_the_edge_it_came_from` (`tests/specification.rs`) | Its one entry is a `NotMine` or a `Remote` without a region |
| `a_restored_region_has_the_state_it_was_restored_from_and_no_chunk` | And knows nothing but what it holds |
| `a_restored_region_carries_on_as_the_original_from_every_tick_of_a_scenario` | Section 1.4 |
| `a_restored_region_carries_on_as_the_one_it_was_restored_from` (`tests/state.rs`) | Section 1.4 |
| `subscriptions_outside_the_area_of_the_region_are_ignored` (worker, `lib.rs`), in C2b.3 | Such a subscription is answered `Elsewhere`, loads nothing, and is told no events |

**Existing tests that need a neighbour the region knows**, and are otherwise as they
are. Their fixtures give the region viewer's tickets on the neighbour's chunks near
the line, answered before the test begins, so that a player who steps across is let
go in the tick of the step and a block across the line is passed on with its region:

- in `region.rs`: `a_player_who_steps_out_of_the_area_is_let_go_with_all_they_carry`,
  `a_player_who_leaves_in_the_tick_they_step_out_in_is_simply_gone`,
  `an_arriving_player_carries_on_as_they_were_handed_over`,
  `blocks_outside_the_area_are_dug_and_placed_by_the_region_that_has_them`,
  `digging_beyond_the_area_within_reach_is_asked_of_the_region_that_has_the_block`,
  `placing_against_a_block_beyond_the_area_is_asked_of_the_region_that_has_it`,
  `placing_into_a_spot_beyond_the_area_is_asked_of_the_region_that_has_the_spot`,
  `placing_without_a_block_or_out_of_reach_is_acknowledged_whoever_has_the_blocks`,
  `what_is_asked_of_another_region_is_not_acknowledged_by_this_one`,
  `remote_requests_are_in_the_order_of_the_inputs_that_caused_them`, the two tests of
  `two_regions_match_one`,
  `inputs_waiting_where_a_player_returns_to_do_not_overtake_those_sent_again`, the
  tests on the fixture that runs two regions against one (from
  `two_regions_change_blocks_across_the_line_as_one_region_does` to
  `a_player_astride_the_line_is_built_into_by_the_region_they_are_not_in`),
  `a_recorded_run_of_a_bounded_region_replays_identically` and
  `a_recorded_run_with_remote_actions_replays_identically`;
- in `tests/specification.rs`: everything on `handing_over`, `three_entries` and
  `scenario`;
- in `tests/state.rs`: everything on `depart`, `entries` and `Scenario`, among them
  `the_delta_of_every_tick_turns_the_state_before_it_into_the_state_after_it`;
- in the worker's `lib.rs`, the ten other tests on `config(WEST)`:
  `an_outbox_entry_stays_until_the_edge_confirms_it`,
  `players_arrive_with_their_entity_and_depart_through_their_link`,
  `actions_on_blocks_of_other_regions_go_through_the_edges`,
  `an_edge_that_stays_away_is_gone_with_its_players_and_departures`,
  `the_status_follows_the_region`,
  `a_runner_restored_after_a_commit_that_was_never_published_tells_it_on_hello`,
  `a_restored_region_carries_on_where_the_store_has_it`,
  `an_edge_that_resumes_is_told_what_it_missed_before_anything_else`,
  `what_a_tick_tells_an_edge_is_in_a_fixed_order` and
  `a_released_region_is_restored_from_a_state_file_alone`. They open region 0 of an
  undivided store (`Store::memory`, `Layout::single()`) and took the western area from
  their config. From C2b.2 they presume it (section 8); in C2b.5 they are moved to a
  store divided at 1, as the specification tests have it since ADR-0011's C1.3, and
  ask;
- in `services/worker/tests/specification.rs`, which is on a divided store already:
  the tests that walk or dig beyond the line
  (`everything_an_edge_was_told_survives_the_owner` and the others on `busy_region`,
  `three_entries` and `with_an_unconfirmed_departure`). In C2b.3 they are put on the
  real path, and the dig at `BEYOND` is passed on with region 1 where a subscription
  has asked and with none where not.

**How the sim's fixtures play the store.** A fixture `Grants` keeps who holds which
chunk: pinned areas with their regions, and grants. `Grants::answer(region, claims)`
gives `granted` and `foreign` as the store would, `Grants::take_back(region, returns)`
frees. A fixture for a region with its store runs a tick, answers that tick's claims
into the inputs of the next, and takes its returns. Tests that are not about asking
make their region with what it needs already known: `Holdings::held` naming the chunks
the test uses, and, where a neighbour matters, as above. The tests that run two
regions against one give each player viewer's tickets on the chunks around the line at
the start, which is the edge's part, and one `Grants` for both; their router sends an
action without a region to the region that holds the chunk, which is the edge's part
as well.

### 7. Stripes until C5

The coordinator and the store keep the layout; `FromCoordinator::Assigned` still names
it, and a `RegionHello` still carries its fingerprint, to the store and from an edge to
a worker (`greet_edge`).

- **The worker process** (`bin/clustine/src/cluster.rs`): `hold` no longer looks the
  region's area up in `Orders::layout`. `Held::config` is the spawn point, the hotbar
  and `DEFAULT_RETURN_AFTER`. A region the layout does not have is no longer found
  there; the store refuses its hello with `UnknownRegion`, which ends the worker as a
  wrong layout does (ADR-0011, section 7). Nothing else changes: what the region holds
  and is pinned to comes with the `Restored`.
- **The single process** (`bin/clustine/src/lib.rs`): `Regions::run`, `run_first` and
  `started` lose `area`. `Regions::layout` stays for the regions to start, the
  fingerprint, and the division the store is made with (ADR-0011, step C1.3).
- **The edge**: its use of `Routing::layout` goes with its own step. It goes on
  linking to every region the routing table lists (section 5.1).

### 8. Building it

Each step leaves `cargo test --workspace` and the run with
`CLUSTINE_TEST_BOUNDARIES=0,4` green. A sim without an area cannot run the stripes
until the runner asks the store, and the edge cannot be rebuilt in the same commit as
both. So the second step puts a scaffold into the sim that the last one removes:

> `RegionConfig::presumed: Vec<(ChunkArea, Option<RegionId>)>`: areas whose holder the
> region takes as given, `None` for itself. A chunk in one of them is `Held` or
> `Foreign` from the start, is never claimed, returned or forgotten, and `granted`,
> `foreign` and `unbelieve` for it are ignored.

**How it gets in and out.** It is a field of the config, so `RegionRunner::restore`
keeps its shape. `hold` in the worker process and `Regions::started` in the single
process fill it with the stripes of the layout they have, each with its region and
`None` for the region itself; tests fill it or leave it empty. From C2b.3 on the
processes can leave it empty: `Config::presumed: bool` for the single process and
`clustine worker --ask-the-store` for a worker, which the end-to-end tests set when the
environment variable `CLUSTINE_TEST_ASK_THE_STORE` is there (`config` in
`bin/clustine/tests/common/mod.rs`, `start_worker` in `common/processes.rs`). **From
C2b.3 on CI and whoever verifies a commit run the divided world a second time with
that variable**, beside the run that has only `CLUSTINE_TEST_BOUNDARIES`; `CLAUDE.md`
gets the line with that step. So the runner's real path is tried end to end before
the edge changes, and the edge of C2b.4 is built against regions that presume and
regions that ask. C2b.5 removes the field, the switch and the variable, and the
second run is the only one.

| # | Scope | Needs | The edge in the same commit | Its tests |
|---|---|---|---|---|
| C2b.1 | `since`; the welcome with its entries; `seen` not confirmed after `Unknown`; the reset of an edge that has lost its `since`; the format byte and states from before (`STATE_FORMAT` 1) | nothing | says and keeps `since`; reads the new welcome | Below, S16; R11, R12, R14 |
| C2b.2 | The sim of sections 1 to 3, with `presumed`; the new shapes of `Durable` (`STATE_FORMAT` 2); `drop_edge` for a `NotMine` arrival. The runner as it is, with every ticket a viewer's and `Region::knowledge` in place of `Region::area` | C2b.1 | the new shapes; `Remote` goes to `to`, or by the layout where it names none | Below, S1 to S15, S17, S18; every existing test of the sim on the new fixtures; the worker's and all end-to-end tests as they are, presumed |
| C2b.3 | The runner of section 4: claims, returns and their answers, `NotHeld`, kinds, numbers and conditions of subscriptions, `Elsewhere` and `NotMine`, the hello's `guests` and the hold, `orphaned`, `Holdings` from `Restored`, crowds; the switch that leaves `presumed` out | C2b.2; ADR-0011's C1.3 and C1.4 | numbers its subscription messages; a hello with empty `guests`; takes `ask` in a snapshot | Below, R1 to R10, R13, R15 to R22, on stores with a division; the worker's specification tests on the real path; the end-to-end tests twice, presumed and asking |
| C2b.4 | The edge without a layout (designed elsewhere, against section 5) | C2b.3 | all of it | Its own; the end-to-end tests twice |
| C2b.5 | `presumed` goes, from the sim, the processes and the tests (section 7); the worker's unit tests on a western area move to a divided store | C2b.4 | nothing | Hand-over, block, takeover, chaos and move tests on two pinned regions; kind |

C2b.1 and C2b.2 touch no code of the store and can be built while C1 is. C2b.2 is the
sim alone but for mechanical changes in the worker, the edge and the binary. C2b.3 is
the runner alone but for the edge counting its subscription messages.

**The edge of today on regions that ask** (the second run of C2b.3). It subscribes to
a chunk at the region its layout names, with `Subscribe`: a pinned region claims the
chunk and serves it, and no subscription is answered `Elsewhere` or `NotMine`. A
region then believes something of a neighbour's chunk only while a player stands in
it, so players are let go two ticks after the step instead of in its tick, and what
they do across the line is passed on without a region, which this edge sends by its
layout since C2b.2, and never back to the region the entry came from: that case it
acknowledges, as `Fanout::outbox` does today. The region the layout names has the
chunk, as it sent this edge the snapshot the player clicked into. The edge never
changes a subscription's kind, so it need not look at `ask`. No `NotMine` entry is
made for it: an arrival goes where the layout says, to the region that holds the
chunk.

**For whoever writes tests from this record alone.**

*The sim* is driven as in `crates/clustine-sim/tests/specification.rs`: a `Region`, a
`TickInputs` per tick, the `TickOutput` and `Region::state`. The test plays the store
(it decides what `granted` and `foreign` a later tick is given for the `claims` of an
earlier one, and when) and the edges (tickets). `return_after` is 0 and `presumed`
empty unless a scenario says otherwise. The worlds: **stripes**, region 0 pinned to
the chunks with x below 1 and region 1 to the rest, as `config_a` and `config_b` have
it; and **open land**, a region pinned to nothing. After every tick of every scenario
the five statements at the end of section 1.3 hold, and the delta turns the state
before into the state after, as `checked_tick` checks today.

- S1. A new region pinned to an area holds nothing: a chunk of the area is `Unknown`.
  A viewer's ticket puts it into that tick's `claims`, once, and it is `Asked`; it is
  in no later `claims` while no answer has come; `granted` makes it `Held` and puts it
  into `chunk_requests` of the same tick; it is never in `returns`, whatever
  `return_after` is.
- S2. A guest's ticket on an unknown chunk of a pinned area claims it. On open land it
  claims nothing, and the chunk stays `Unknown`.
- S3. `foreign` for a chunk with a viewer's ticket: `Foreign(r)` for as long as the
  ticket is there, with no further claim; when the ticket goes, `Unknown` at the end
  of that tick; a new viewer's ticket claims it again.
- S4. `foreign` for a chunk nothing wants any more leaves it `Unknown`; `granted` for
  one makes it `Held`, and on open land it is in `returns` of that tick.
- S5. `unbelieve` naming the believed region makes the chunk `Asked` and puts it into
  `claims` of that tick if it has a viewer's ticket; naming another region, or a held
  chunk, changes nothing.
- S6. A player walks into a chunk the region knows nothing of: they stay, what they do
  next is applied, the chunk is in `claims` of the tick of the step. `granted`: they
  stay. `foreign` with `r` instead: `Departed { to: r }` in the tick of the answer,
  with the `last_input` of the last input applied before that tick, and nothing of
  that tick's inputs applied.
- S7. A player steps into a chunk that is `Foreign(r)`: `Departed { to: r }` in that
  tick, the inputs behind the step not applied, as today.
- S8. An arrival for a held chunk is taken in; for an unknown chunk it is taken in and
  the chunk claimed; for a chunk that is `Foreign(r)` the outbox of the edge gets
  `NotMine` with the arrival and `r`, the players are as before and no entity is
  reported. An arrival of a player who is there is as today in all three. With that
  `NotMine` in its outbox, a higher start of the edge, and in another run `Gone`,
  report the arrival's entity removed, with the chunk of its pose, once; not if a
  player of the region has that entity.
- S9. A dig within reach at a block of a chunk that is `Foreign(r)`: `Remote { to:
  Some(r) }` with the dig. At one of a chunk that is asked or unknown: `Remote { to:
  None }`, in the tick of the input. In both the dig is not acknowledged, the
  player's `last_input` is the dig's, and the chunk is in no `claims` because of it.
  A dig out of reach is acknowledged and nothing else, whatever the chunk.
- S10. A placement against a block of a held chunk into a spot of a chunk that is
  `Foreign(r)` is `Remote { to: Some(r) }` with `Place`, into one of an unknown chunk
  `Remote { to: None }` with `Place`; against a block of a chunk that is `Foreign(r)`
  or unknown it is `Remote` with `PlaceAgainst` and `Some(r)` or `None`. Nothing is
  placed in any of them.
- S11. A remote action about a held chunk is as today. About a chunk that is
  `Foreign(r)`: `NotMine` with the action as it came and `r`. About an asked or
  unknown chunk: `Remote { to: None }` with the action as it came, and nothing
  changes. A `PlaceAgainst` on a block of a held chunk: with the target's chunk held,
  placed and `RemoteDone`; with it `Foreign(r)`, `Remote { to: Some(r) }` with
  `Place`; with it unknown, `Remote { to: None }` with `Place`. Each is one entry, for
  the edge the action came through.
- S12. On open land, a held chunk that nothing uses is in `returns` once and is
  `Unknown` after. Not returned: a chunk with a player in it, with a viewer's ticket,
  **with a guest's ticket**, and the chunk of the spawn point. With `return_after` 5
  it is returned five ticks after the first tick in which nothing used it and not
  before, and a ticket of either kind in between starts the count anew.
- S13. A ticket of either kind on a chunk that is not held asks storage for nothing. A
  chunk that is loaded is in no `returns`. A chunk delivered after its last ticket
  went is not taken, as today.
- S14. `Region::restore` with a state and `Holdings`: `Held` is what `held` names and
  nothing else is known; a player of the state who stands in a chunk that is not held
  is still there after the first tick, and the chunk is in its `claims`.
- S15. A region with the empty block of entity ids answers a join with `Refused`.
- S16. `since` is the number of the tick that noted the edge; a higher start gives a
  new one; the same start, a confirmation and `applied` leave it; an edge that is gone
  and comes back, in a later tick or in the same, has the number of that tick.
- S17. The same inputs, with the same `granted` and `foreign`, give byte-identical
  states and identical `claims` and `returns`.
- S18. Two regions on stripes, with one `Grants`, viewer's tickets around the line and
  a router that sends an action without a region to the region that holds the chunk,
  treat players and blocks as one region does that holds everything, as the existing
  tests of that kind assert. The same without the viewer's tickets, in which every
  action across the line is without a region and every player is let go two ticks
  after the step: the blocks end the same.

*The runner* is driven as in `services/worker/tests/specification.rs`: a `Link` per
edge, the runner stepped by the test, every wait a loop until a message or a state is
there. The store is `Store::memory_divided` and `Store::local_divided` of ADR-0011,
with **stripes** at a boundary at 1, of which region 0 is under test and has the home
chunk and region 1 is the other; or with the division **with a gap** of ADR-0011,
section 9, of which the home region, region 2, is under test: it is pinned to nothing
and holds the home chunk alone, the chunks between the two pinned areas are free, and
"the neighbour" is region 1. The test plays the neighbour through a handle of its own
(`Store::open_region`, `StoreRequest::Claim`, `Return`, `Flush`, `Load`, `Save`). A
"crash" is the region opened again with a higher epoch. `return_after` is 0 unless
said, and nothing is presumed. Subscription messages are numbered by the test.

- R1. A viewer's subscription to a chunk of the region's own stripe is answered with a
  snapshot that has the message's `ask`; to one of the other stripe with `Elsewhere`
  naming region 1, with the `ask`, no snapshot, and no event of that chunk after it. A
  second `Subscribe` for the latter is answered `Elsewhere` again, with the second
  number.
- R2. A guest's subscription to a chunk of the own stripe is answered with a snapshot;
  to one of the other stripe with `NotMine`. With the gap: a guest's subscription to a
  free chunk is answered `NotMine`, and the neighbour's claim of that chunk afterwards
  is granted.
- R3. With the gap: a viewer's subscription to a free chunk is answered with a
  snapshot, and a crash later the chunk is in `Restored::held`.
- R4. With the gap, the test's neighbour holding a chunk: a viewer's subscription is
  answered `Elsewhere` with the neighbour. The neighbour returns the chunk and waits
  for the answer to a flush behind the return; a second `Subscribe` is then answered
  with a snapshot.
- R5. **A guest keeps a chunk.** With the gap, a chunk the region was granted for a
  viewer: the subscription is made a guest's. No second snapshot and no `NotMine`
  come, an event in the chunk still reaches the link, and the neighbour's claim of the
  chunk is `foreign` however many ticks the test steps. After `Unsubscribe` the
  neighbour's claim is granted once the return is through (the test claims until it
  is). With `return_after` 40 the claim is `foreign` for at least 40 ticks after the
  `Unsubscribe`.
- R6. **A chunk leaves saved.** With the gap: a player in the home chunk breaks a
  block of a neighbouring chunk that the region was granted for the link's viewer's
  subscription; the link unsubscribes; the neighbour claims the chunk until it is
  granted and loads it: the block is broken. The same after a crash of the region
  right behind the `Unsubscribe`, when the store may have dropped the return with the
  old session: whichever of the two regions then holds the chunk loads it with the
  block broken.
- R7. A player who walks from the home chunk into the other stripe is let go with
  `Departed { to: 1 }`, also when no subscription had asked about that chunk before.
- R8. An arrival for a chunk of the other stripe that a viewer's subscription has
  asked about is answered with the outbox entry `NotMine` naming region 1; one for a
  chunk of the own stripe that nothing has asked about is taken in.
- R9. A player digs a block of the other stripe right after joining, with no
  subscription: `Remote { to: None }` is among what the tick of the dig produced. With
  a viewer's subscription to that chunk answered `Elsewhere` before the dig: `Remote {
  to: Some(1) }`.
- R10. A hello with `chunks` in both stripes and `guests` in both: nothing the link
  sends behind it is applied before each of them is answered, with a snapshot,
  `Elsewhere` or `NotMine`, each with `ask` 0; then all of it is.
- R11. A first hello is welcomed `Unknown { since, entries: 0 }` with `since` above 0;
  a hello that says that `since` is welcomed `Resumed`. A second link that says
  another `since`, or 0, with the same start, **before the edge has sent a numbered
  message**, is welcomed `Unknown` with the same `since` again. One that does so
  **after a join of the edge was applied** is welcomed `Unknown` with a higher
  `since` and no entries, the player is reported removed, and numbered messages from 1
  are taken on that link.
- R12. `entries` is the number of `Outbox` messages between the welcome and the first
  presence answer, with none, some and all of an outbox seen. A `Resumed` hello's
  `seen` drops the entries up to it; after `Unknown` no entry is dropped for it.
- R13. `Elsewhere` and `NotMine` for a chunk are part of a tick: on a world in memory
  and on one on disk they come behind everything of the ticks before theirs and before
  anything of the ticks after it, at the place section 5.2 gives them. In a tick that
  also has a commit (a player joins in it), they are not on the link before that
  commit is confirmed, and a runner whose store was taken by another owner after the
  tick ran sends neither.
- R14. A region whose stored state and deltas were written without the two bytes in
  front (written by hand through a handle) is restored: it has no players and no
  edges, its tick is that of the last such item, and deltas written with the bytes
  behind them are applied. A state with a higher format number is a `RestoreError`.
- R15. After a crash a hello with the chunks of R1 is answered as in R1 again, and a
  player who stood in the home chunk is present.
- R16. A released region's granted chunks are in the next owner's `Restored::held`.
  With `return_after` 40 and no link, the next owner returns them with its 41st tick
  and not before; a hello with those chunks before that keeps them.
- R17. The removal of a departed entity whose edge started anew reaches a link that is
  subscribed to nothing. So does the removal of the entity of an arrival that was
  answered `NotMine`, when the edge whose outbox has the entry starts anew, and when
  it stays away until it is gone.
- R18. A link that ends takes its tickets with it: with the gap and `return_after` 0,
  the region's granted chunks without a player are returned; with `return_after` 40
  and a new hello within that time, none is.
- R19. **An answer carries the number of the last asking.** For one chunk, with no
  step of the runner in between: `Subscribe` 1, `Unsubscribe` 2, `Subscribe` 3 gets
  one answer, with `ask` 3. For a chunk of the other stripe: `Subscribe` 1 and then
  `SubscribeAsGuest` 2 before the answer gets one answer, `NotMine` with `ask` 2, and
  no `Elsewhere`. For a chunk of the own stripe: `SubscribeAsGuest` 1 and then
  `Subscribe` 2 before the answer gets one snapshot, with `ask` 2.
- R20. **A served subscription that changes its kind** gets no answer, to a guest's
  and back, and events of the chunk go on in every tick. A `Subscribe` for a chunk
  that was told elsewhere and has since been made a guest's and answered `NotMine`
  begins anew.
- R21. **Numbers out of order.** A subscription message whose `ask` is not above the
  one before on its link ends the link; so does a hello that names a chunk after a
  subscription message. A hello without chunks after one does not.
- R22. **The crossing at a hand-over.** With nothing asked about it before, the link
  sends together a player's step into a chunk of the other stripe and a `Subscribe`
  for that chunk. In the tick of the store's answer the link gets `Departed { to: 1 }`
  and behind it `Elsewhere` for the chunk with the number of that `Subscribe`. If the
  test says `Unsubscribe` with a higher number as soon as it reads the `Departed`,
  nothing more comes for the chunk.

In the worker's unit tests, which can put a `Gate` before the store: a `NotHeld` ends
the runner as a lost store does; the order of section 4.2 on the handle, with the
save of a chunk whose last ticket went before its `Return`; and a `Claimed` that
arrives while a release is still ticking is taken into the next tick.

## Ruled out

- **Inputs that wait inside the region** for the store's answer; see section 2.3.
- **A region that goes on serving a returned chunk until the store says the return is
  through**, which the review proposed for the time a returned chunk is nobody's. A
  chunk that is watched is not returned at all, so there is no such time, and the
  store need say nothing.
- **Answers that say which kind they answer**, in place of a number. A kind does not
  tell an answer to a subscription that was ended and made again from the answer to
  the new one.
- **Answering a viewer from what the region believed long ago**, by keeping beliefs
  until a restore. It saves the store a question when a player comes back, and makes
  what a region believes depend on everyone who ever passed by.
- **Ending a viewer's subscription with `Elsewhere`**, so that an edge has one
  subscription per chunk. A region then learns who holds the chunks around its players
  once and forgets it: every player who steps across would stand in another region's
  chunk for the two ticks the store takes, where today they are let go in the tick of
  the step.
- **A message of its own for asking again.** A second `Subscribe` says it.
- **A list of what became known in a tick** as the tick's output for the runner; see
  section 4.4.
- **Keeping an edge's viewer's tickets while it has no link.** The time before a
  return does the same with one number.
- **Sending the load with the claim**, which the store would allow (ADR-0011, section
  3.1). It saves a tick per chunk that is new to a region; see the consequences.
- **Building sim, runner and edge in one commit**, without `presumed`. Nothing in
  between could be verified.

## Consequences

- A region asks before it loads. A chunk of the viewer's own region that is new to it
  is on a client three ticks after it was asked for, where it is two today; a chunk
  that another region serves five or six (the viewer's region asks the store and says
  `Elsewhere`, then the holder claims, loads and sends), or three if the holder has it
  loaded. Everything a restored region sends again is a tick later as well, on top of
  the 0.75 seconds a resume was measured at in phase B. None of this is measured.
- A player who is handed over makes the new region ask about every chunk in their view
  that it does not hold: one claim of a few hundred chunks, and as many `Elsewhere`.
  Where every chunk is some region's, as in a world of pinned regions, that is
  answered from the table and writes nothing; on open land the free chunks in view are
  granted, with a `Granted` record. A player who stands on a boundary does that every
  tick they are handed back.
- What a player does to a block of a chunk their region has no answer for takes the
  two ticks of any action across a boundary (ADR-0006), by way of the edge.
- A chunk stays with its region for as long as anyone watches it. Regions whose
  players are near each other therefore keep chunks among each other's until a merge
  makes them one; C4 has to count on that. A region sheds what nobody watches 30
  seconds after the last look.
- With pinned regions that cover the world, which is all C2b runs, no chunk is ever
  returned, every claim is answered from the table, and no guest is told `NotMine`.
  Returns, free chunks and beliefs that go stale are tried by the tests with a gap,
  and by C3.
- The edge has more to keep than ADR-0010 said: per region and chunk whether a viewer
  of that region's players sees it, apart from where the chunk is served, and per link
  and chunk the number of its last message.

## Changes to ADR-0010

1. **Section 1, "A subscription of a guest is no need"**: it is no need to claim, and
   a reason to keep. A region does not give back a chunk while any link is subscribed
   to it (section 1.2 here). A chunk that someone watches leaves a region only by a
   merge or a split.
2. **Section 1 and section 3, a guest's subscription in a pinned area**: it makes the
   region claim the chunk, as that takes nothing from anyone. ADR-0010 has a region
   asked as a guest for a chunk it does not hold answer `NotMine`, and the comment on
   `EdgeToWorker::SubscribeAsGuest` says a region "does not claim a chunk because a
   guest asks for it"; both hold outside pinned areas only.
3. **Section 1, when a chunk is given back**: ADR-0010 does not say. Here it is after
   `return_after` ticks without use, and never for a pinned chunk or the home chunk.
4. **Section 2, "A block action on a chunk the region does not hold or has not heard
   about is acknowledged without effect"**: it is passed to the edge without a region
   named, and the edge sends it to the region that serves it the chunk (section 2.3
   here). `Durable::Remote` names a region or none.
5. **Section 2, "An arrival ... for a chunk the region does not hold is answered ...
   `NotMine`"**: only when the region believes another region to hold the chunk.
   Otherwise the player is taken in and the chunk claimed (section 2.2). A pinned
   region does not know its own chunks until it asks.
6. **Section 2, a remote action for a chunk the region does not hold**: `NotMine` with
   the holder if the region believes one, and otherwise `Remote` without a region
   (section 2.4). `NotMine` always names a holder.
7. **Section 2, the removal of an entity nobody will pass on**: also for the entity of
   a `NotMine` for an arrival.
8. **Section 3, "back to the region that sent the item, which is told to ask again"**:
   nothing is sent back for a region to ask again. A region is made to ask again by a
   second `Subscribe` of its edge, and `unbelieve` names the region that is no longer
   believed.
9. **Section 3, "When a region stops holding a chunk (it gives it back ...), it tells
   every link subscribed to it"**: a region never gives back a chunk a link is
   subscribed to. What is left of that sentence is the split and the merge.
10. **Section 3, subscriptions**: subscription messages are numbered per link and
    every answer carries the number of the asking it answers. A viewer's subscription
    outlives its `Elsewhere`. The hello names viewer's and guest's chunks apart, and
    the viewer's include those another region serves.
11. **Section 3, "The edge links to the regions it has to do with"**: in C2b to every
    region the routing table lists (section 5.1).
12. **Section 3, "Knowing an edge again"**: `since` is the number of the tick that
    made the state; after `Unknown` the hello's `seen` confirms nothing; an edge that
    says another `since` after the region has taken messages from it is reset as for
    a higher start.
13. **Section 2, what a region knows**: a belief is kept only while the chunk is
    wanted.
14. **Section 5, step 5**, tells "the links subscribed to chunks of the part
    `Elsewhere`". Here a guest's subscription is never answered `Elsewhere`, and a
    served subscription is never ended by the region. Step C3 has to say what a guest
    of a chunk that is split off is told; see the risks.

## Changes to ADR-0011

1. **Section 9, C1.3, `NotHeld` in the worker**: the runner gives up as for a lost
   store, instead of treating the chunk as unreadable.
2. **Section 9, C1.3, "The unit tests in the worker's `lib.rs` open region 0 of an
   undivided world and stay as they are"**: the eleven on a western area presume that
   area from C2b.2 and move to a store divided at 1 in C2b.5 (section 6 here).
3. **Open question 1** (commits to chunks not held): no region makes one.
4. **Open question 2** (hearing that a return is through): not needed, because a chunk
   that anyone is subscribed to is not returned.
5. **Section 2, "a pinned region claims the chunks of its areas like any other
   region"**: also when only a guest asks.

Nothing in the store's interface changes.

## Changes to ADR-0008

1. **Section 1**: `EdgeState::since`.
2. **Section 4**: the hello and the welcome of section 4.5 here; the hold ends for a
   chunk also with `Elsewhere` and `NotMine`; a state is stored with two bytes in
   front; subscribe and unsubscribe carry a number of their own.
3. **Section 4, "Departures nobody will pass on"**: also arrivals that were answered
   `NotMine`.

## Found by the tests of C2b.1

Written from this record by someone who had not seen the code. One fault in the code:

- A stored state of tick 0 was read like any other, against section 4.1. One that an
  earlier build left (a runner stopped before its first tick stores one) begins with
  the zero of its tick, which was taken for the zero in front of a format number, and
  the region was refused as written by a later build. It is not read now.

And what the record left to the reader, which the tests and the code read alike:

- **What "received" is after a restore** (section 4.5): a restored runner counts from
  the state's `applied`, so an edge that lost its `since` after a message of it was
  applied under an earlier owner is reset.
- **Two hellos of one edge between two ticks**: the link that stays is told the
  `since` the region has for the edge after the tick, with its entries; the cases of
  section 4.5 are not worked out for each hello by itself from the state before.
- **A numbered message right behind the hello of an edge that is reset** is taken:
  nothing counts as received from the hello on.
- **An item that cannot be read, or is a later build's, before an item from before**
  (section 4.1): items are gone through in order and the error comes first. It cannot
  arise without going back to an earlier build twice.

Left for step C3, which is the first to make such states: what presence says after
`Unknown` when the state has a player of the edge and nothing received.

## Open questions

1. **Whether a region may take in an arrival for a chunk it has nothing to do with.**
   It is what makes pinned regions work, and it means that after a region gave a chunk
   back, a neighbour's player who walks into it within the moment the neighbour still
   believes the old holder ends in the old holder's region, which grows by that chunk.
   A viewer sees a chunk long before its player is in it, so the moment is short. C4
   will see whether such a region is then merged or split for nothing.
2. **`return_after`** is a guess. It has to be longer than an edge takes to come back
   after a restore, and it decides how much a region holds behind its players.
3. **Whether a region should name a holder for a block action at all.** The edge knows
   who serves it the chunk; a region's belief is the same answer of the store, a little
   older. `Remote` could always be without a region, and a region's beliefs would then
   be for letting players go only. Kept because ADR-0010 has regions name where things
   go, and it costs nothing.
4. **Whether `NotHeld` should end the runner** once C3 exists: a split takes chunks
   from a region by its own doing while loads of them can be under way. C3 has to say
   what the runner does with those answers.
5. **Dropping states from before** was chosen over refusing them. If a world is ever
   served by builds of two formats in turn, the older one finds bytes it calls "a
   later build's" and refuses.
6. **The format byte's place.** It is the worker's, in front of bytes the store does
   not look into. The store's `meta` could carry it instead, at the price of the store
   knowing that states have formats.
7. **The lists of tests in section 6** were made by reading the sim's tests for what
   they do at the line and the worker's for what they construct. The end-to-end tests
   in `bin/clustine/tests` were not read one by one; they see the server through a
   client, and are expected to hold as they are.
8. **That a client puts a block back** when its action is acknowledged without the
   block having changed was not checked against the official server. Section 5.6,
   rule 26, relies on it as today's code does when it gives a remote action up.

## Risks

Three things that C2b's processes cannot show and step C3 has to settle, each with
the sequence that shows it:

- **A region learns its pinned areas only when it is opened.** `Holdings::pinned`
  comes with `Restored`, and ADR-0011's merge makes the absorbed region's areas the
  survivor's in the store without its reply saying which (its open question 10).
  Region `A` absorbs pinned `B`. An edge asks `A` as a guest for a chunk of `B`'s
  former area: it is `Unknown` and in no area `A` knows, so `NotMine`. The edge asks
  the viewer's region: claim, `foreign(A)`, `Elsewhere` with `A`, guest at `A`,
  `NotMine`, for ever; the chunk is a hole for those viewers until `A` is restored. C3
  has the reply carry the areas and gives the tick a way to learn an area without a
  restore.
- **The proof that beliefs form no ring** rests on a region that holds a chunk knowing
  so or knowing nothing of it. C3 moves chunks without a claim. A region that splits
  has to believe the new region. And: a part is split off pinned `P` and becomes `N`;
  `P` believes `N` to hold a chunk of its own area for as long as it wants it. `N`
  gives the chunk back; by the table it is `P`'s again, and nobody tells `P`. A
  neighbour claims it, is told `foreign(P)`, lets a player go to `P`; `P` answers
  `NotMine` with `N`; `N`, which knows nothing of it, takes the player in, claims, is
  told `foreign(P)`, lets them go to `P`; and round again, until `P`'s edge asks
  again.
- **A chunk that changes hands under someone's eyes.** In this record that does not
  happen. A split or a merge does it: a served subscription ends (ADR-0010, section 5,
  step 5 says with `Elsewhere`, also to guests, which section 5.4 here does not
  allow), an action on its way meets a region that no longer holds the chunk, and the
  new holder has to load before it can take one. The numbers of section 4.3 make the
  first safe: an edge that turns its guest's subscription into a viewer's while the
  region's last word on it is on its way passes that word over. The rest is C3's:
  what a guest is told, and that the chunk is served again before anyone can act on
  it.

And of this step:

- **Section 5 is long, and the edge is built from it by someone else.** A rule that is
  wrong there is an ordering mistake in the edge. The R tests check the region's half
  of it; the edge's half, rules 5 and 6 above all, only the edge's own tests and the
  end-to-end runs do.
- **The scaffold `presumed`** hides the real path from the kind test until C2b.5, and
  from the first run of the end-to-end tests. The second run is what covers it.
- **`to: None` trusts the edge to know who serves it a chunk.** An edge that keeps a
  chunk on a client's screen after the region that sent it said `NotMine` has nobody
  to send the action to and ends it, which is right, and which the player sees as a
  block that does not break.

## Review

An independent review against the code found sixteen defects in the first version of
this record, and judged it fit to build as corrected, with the store side and what a
region knows of chunks sound. What it changed:

1. Answers crossed a change of kind: an `Elsewhere` could arrive for what the edge by
   then held as a guest's subscription, a `NotMine` for a viewer's, and one such
   crossing left a viewer's ticket that nobody took back. Subscription messages are
   numbered, every answer carries the number of the asking it answers, and the edge
   passes over a lower one (sections 4.3, 4.4, 5.4).
2. Inputs that waited inside the region did less than claimed (after a restore the
   hold keeps inputs back already, and an input whose answer was `granted` was lost
   all the same) at the price of a list in the durable state. The list is gone; such
   an action is passed to the edge without a region, which sends it to the region
   that serves it the chunk (section 2.3).
3. A chunk given back under a guest's eyes was nobody's until the store's one thread
   for chunks had got to the return, with every action on it lost and the edge going
   round between `Elsewhere` and `NotMine` meanwhile. A region no longer gives back a
   chunk any link is subscribed to (section 1.2).
4. After `Unknown` with another `since` the record numbered messages two ways, and a
   test it asked for walked into the difference: a join dropped without a word and an
   input applied for nobody. The case with nothing received is stated as what it is,
   and the other resets the edge (section 4.5).
5. A `NotMine` for an arrival that was dropped with its outbox left the entity on
   screens. It is treated as a `Departed` there (sections 2.2, 4.6).
6. The contract for the edge left out or got wrong: that a region forgets an edge
   without a link; what goes with a `NotMine` for an arrival; that events came for
   subscriptions in every condition; that a player can be a region's in a chunk
   another serves; that `SubscribeAsGuest` takes a need away; that a region does not
   take a chunk "when it is free" by itself; that several askings share one answer;
   that an action can end without `RemoteDone`; what `Progress` covers; and the order
   on a link. Section 5 is written anew, and events no longer come for a subscription
   that was told elsewhere.
7. `Asked` is left only by an answer, and the record let a runner drop answers; a
   survivor of a merge that paused and ticked on would have had chunks asked for
   good. Only a runner that never ticks again may drop one (section 4.2).
8. A region that absorbs a pinned region does not learn the areas and serves no guest
   there. Named under the risks for C3, with its sequence.
9. Three groups of existing tests that break or change their point were not named: the
   restore test in `tests/state.rs`, the worker's test of subscriptions outside the
   area, and its unit tests on a western area of an undivided store. Section 6 names
   them and the others.
10. How `presumed` reached the runner was not said, and the sim's, the runner's and
    the edge's real paths met for the first time in the last step. It is a field of
    the config, and from C2b.3 on a second test run leaves it out (section 8).
11. Changes to ADR-0010 and ADR-0011 that were made and not listed: a guest's ticket
    making a pinned region claim, the split's `Elsewhere` to guests, the worker's unit
    tests, the store's reply to a return. All are listed now.
12. Three runner scenarios could not be written as they stood (a tick with only an
    `Elsewhere` has no commit to wait for; a return is not through when it is asked
    for; the test of `since` contradicted the text), and four were missing. Corrected
    and added (R4, R11, R13, R17, R19 to R22).
13. The record told the edge to move a view's subscriptions when a player is let go,
    and, under the consequences, that it could spare doing so. The second is gone.
14. What a waiting input did when its player's chunk turned `Foreign` was not said.
    There are no waiting inputs any more.
15. Smaller statements that were not so: that a handed-over player's claim writes
    nothing (only where every chunk is some region's); that an edge is never sent
    anything of a chunk it was told is elsewhere (now so by rule); that waiting helped
    after a restore; that `RegionRunner::restore` keeps its shape (now it does).
16. `since` can come again after a world is made over. Section 2.6 says why that
    meets no edge.

The review could not verify the timings under "Consequences", and counted five to six
ticks where the first version said "two or three ticks after the near half"; both
counts are there now, and neither is measured.
