# ADR-0012: The tick on chunks

- Status: **Proposed**; the design of step C2b of milestone M3, phase C, for the
  simulation and the region runner. Not reviewed and not built yet. The edge's side of
  the step is designed elsewhere, against section 5 of this record.
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
- **The runner** (`RegionRunner` in `services/worker/src/lib.rs`) keeps, per link, the
  chunks it is subscribed to and those it awaits a snapshot of
  (`EdgeLink::subscriptions`, `awaiting_snapshot`). `RegionRunner::subscribe` passes
  over a chunk outside `Region::area()` without a word. Each subscription is one
  ticket. `RegionRunner::release` saves a chunk when the last link lets go of it,
  before the tick that drops it. After a tick the runner asks the store for the loads,
  sends the `Commit`, checkpoints if it is time, and makes ready what links are to
  hear, which `publish_committed` sends once the commit is confirmed.
  `take_replies` logs `Claimed` as an answer to what was not asked.
- **`EdgeToWorker::SubscribeAsGuest`** is said to be ignored by a worker. It is not: in
  `RegionRunner::accept` it falls into the arm for numbered messages, has no number,
  and ends the link as a numbered message before a hello does. No edge sends it.
- **A hello** names `edge`, `start`, `seen`, `players` and `chunks`.
  `RegionRunner::hello` turns it into `EdgeEvent::Started` and
  `EdgeEvent::Confirmed { number: seen }`, holds
  what the link sends until the snapshots of the hello's chunks are made
  (`EdgeLink::hold`), and answers `Welcome::Resumed` if the region knew the edge with
  that start (`KnownEdge::settled`), else `Welcome::Unknown`.
- **The edge** (`services/edge/src/fanout.rs`) finds the region of a chunk, of a player
  who was let go and of a remote action with `Layout::region_of`, and logs
  `Elsewhere`, `NotMine` and the outbox entries of ADR-0010 as things it does not act
  on.
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

#### 1.2 Tickets, and what a region needs, wants and keeps

A **ticket** is a link's subscription to a chunk, as today, and is now of one of two
kinds (`Ticket::Viewer`, `Ticket::Guest`): a viewer's, asked for with `Subscribe` by an
edge for a viewer whose player is this region's, and a guest's, asked for with
`SubscribeAsGuest`. The region counts both per chunk, on every chunk whatever it knows
of it: a ticket is no longer passed over because the chunk is elsewhere.

With "a player stands in a chunk" meaning a player of the region whose pose is in it:

- The region **needs** a chunk while a player stands in it or it has a viewer's ticket.
- It **wants** a chunk while it needs it, or the chunk is in one of its pinned areas
  and has a guest's ticket, or an input of one of its players waits for it (section
  2.3).
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
| `Held` | it is not kept and has not been needed for `RegionConfig::return_after` ticks | `Unknown`, and the chunk is in that tick's `returns` |
| `Held` | `foreign` or `unbelieve` names it | `Held`: ignored |
| `Foreign(r)` | `unbelieve` names it with another region | `Foreign(r)`: ignored |

Between a `foreign` and the end of its tick the chunk is `Foreign(r)` in either case,
so that a player who stands in it is let go and an input that waited for it is passed
on in that very tick.

At the end of every tick, therefore:

1. no chunk is in two of the three collections the region keeps (held, asked,
   foreign);
2. every wanted chunk is `Held`, `Asked` or `Foreign`;
3. every `Foreign` chunk is wanted;
4. a loaded chunk is `Held` and has a ticket, and so has a chunk asked of storage.

What triggers each, by cause:

- **A viewer's ticket** makes the chunk needed. If it is `Unknown` it is claimed; if it
  is `Foreign` the belief is kept for as long as the ticket is there.
- **A guest's ticket** makes a chunk wanted only in a pinned area: such a claim takes
  nothing from anyone. Elsewhere it changes nothing the region knows.
- **A player standing in or walking into a chunk** makes it needed: an `Unknown` chunk
  is claimed at the end of that tick, so the region grows where its players go.
- **A waiting input** (section 2.3) makes the chunk it is about wanted, so that it is
  asked for, and the answer kept until the input has been dealt with.
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

> A chunk is returned at the end of a tick if it is held and not kept, nothing needs
> it at the end of that tick, and nothing needed it at the end of any of the
> `return_after` ticks before. The ticks before the chunk was granted, and those
> before the region was created or restored, count as ticks in which it was needed.

With `return_after` 0 a chunk goes at the end of the first tick in which nothing needs
it; with 5, five ticks after that one. Being needed in between starts the count anew.
After a restore the count starts from the restore, as the time an edge is away does.
The processes use 600 ticks, which is the 30 seconds after which an edge is `Gone`;
tests set 0 unless they are about the time.

**A belief is kept only while it is wanted**, so what a region believes is bounded by
what its players see, and follows from what it holds, its tickets and the store's
answers alone: a restored region that is given the same tickets and the same answers
believes the same.

#### 1.4 What is in `RegionState` and what is not

In it, new: `EdgeState::since` (section 2.6) and `PlayerState::waiting` (section 2.3).

Not in it: what the region holds, which the store says when the region is opened
(`Restored::held`, as `Holdings::held`); the areas it is pinned to, which the store
says as well (`Restored::pinned`, as `Holdings::pinned`); what it has asked and not
heard; what it believes; tickets; loaded chunks; and since when a chunk has not been
needed.

**Why a restore is still exact.** Nothing a restored region lacks was ever shown to
anyone as a fact about the region's state, and all of it comes back by asking:

- what it holds is the store's to say, and the store says it;
- a chunk it had asked for and has since been granted is in `Restored::held` if a grant
  was written, and otherwise (a chunk of a pinned area) is claimed again when it is
  wanted;
- a player who stands in a chunk the region does not hold after the restore stays the
  region's, the chunk is claimed in the first tick, and the answer lets the player go
  or not, as it would have;
- an input that waited is in the state and waits again, and asks again;
- tickets come with the hellos of the edges.

What can differ from the run that was lost is when: a claim is made again, a player is
let go a tick or two later, a chunk is returned later. The test
`a_restored_region_carries_on_as_the_original_from_every_tick_of_a_scenario` in
`tests/specification.rs` already gives the restored region its tickets and its chunk
while the original idles, and compares from there; it gives it the store's answers as
well from now on, and passes over `claims` as it passes over `chunk_requests`.

A chunk has at most one claim without an answer at any time: it leaves `Asked` only by
an answer.

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
6. **Inputs** (section 2.3): the inputs that wait, player by player in the order of the
   players, then `inputs` in their order.
7. Moves and acknowledgements are reported, as today.
8. **Players are let go**: every player who stands in a `Foreign(r)` chunk, in the
   order of the players, with `Durable::Departed { player, transfer, to: r }`. What
   waited of their inputs is dropped.
9. **Chunks again**, in this order:
   1. Returns: every `Held` chunk whose time has come (section 1.3, "The time before
      a return") becomes `Unknown` and is in `returns`, ascending. If it is
      loaded it is taken out of the region and put into `dropped` as it is now, with
      every change this tick made to it; it is no longer asked of storage. Its tickets
      stay counted: the runner takes them back (section 4.4).
   2. Every `Foreign` chunk that is not wanted becomes `Unknown`.
   3. Every wanted chunk that is `Unknown` becomes `Asked` and is in `claims`,
      ascending.
10. The delta is taken, as today.

A chunk cannot be in `returns` and `claims` of one tick: one is not needed and the
other is wanted, and a chunk that is only wanted (a guest's, or a waiting input's) and
`Held` is in neither.

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
   `Durable::NotMine { what: Misdirected::Arrival { player, transfer }, holder:
   Some(r) }` goes to the outbox of `edge`. Nothing else changes and no entity is
   reported.
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

#### 2.3 What a player does to blocks, and inputs that wait

A `Dig` or a `UseItemOn` is judged as today as far as only the player matters: one
that is out of reach, or a placement without a block in hand, is acknowledged and
nothing else. Then, by what the region knows of the chunk the block is in:

| The chunk is | `Dig { position }` | `UseItemOn`: the chunk of `against` | `UseItemOn`: the chunk of `target`, once `against` is found to be a block of a held chunk |
|---|---|---|---|
| `Held` | acknowledged; the block is broken if it is there, as today | if `against` is no block: acknowledged; else see the next column | placed if the spot is free, as today; acknowledged |
| `Foreign(r)` | `Durable::Remote { action, to: r }` with `RemoteStep::Break`; not acknowledged here | `Remote { action, to: r }` with `RemoteStep::PlaceAgainst` | `Remote { action, to: r }` with `RemoteStep::Place` |
| `Asked`, `Unknown` | **the input waits** | **the input waits** | **the input waits**; nothing has been changed |

A block of a `Held` chunk that is not loaded is not there, as today.

**An input that waits** is not applied, `last_input` stays where it was, and the input
is put at the end of `PlayerState::waiting`, a list of `(number, input)` in ascending
order of the numbers. The chunk it was found to wait for is wanted for as long as the
input is the first of its player's list, so it is claimed at the end of the tick if
nobody has asked, and what the store answers is kept until the input has been tried
again. While a player has inputs waiting, every further input of theirs that passes
the checks an input passes today (the player is there, the edge is theirs, the number
is above `last_input`) and whose number is above that of the last one waiting is put
behind them without being looked at; a number that is not above is dropped as one
that came twice. In step 6 of every tick a player's waiting inputs are tried from the
front, each as if it had just come, until one has to wait again or the list is empty.

So a player whose action is about a chunk the region has no answer for stands still
for the tick or two the answer takes, and then everything they did happens in order.
Other players are not held up.

The list is dropped when the player leaves, is removed with their edge, is let go, or
joins or arrives anew. It is not part of a `PlayerTransfer`: the edge keeps a player's
inputs until a region reports them applied, and sends those above `last_input` to the
next region.

The list is in the state because the runner counts a message as received when it has
passed it into a tick, and the region reports the edge's messages applied up to it:
the edge then stops keeping the message. An input that was only remembered outside the
state would be lost with the owner, between two that are applied.

ADR-0010 has such an action "acknowledged without effect, as one on a chunk that is
not loaded is today". That is a seam in ordinary play. A player who walks from region
`A` into region `B` arrives in a region that knows nothing of `A`'s chunks, because it
never needed them; the chunks are on the player's screen, sent by `A`; and the first
thing a player who builds along a boundary does after crossing is to click a block
behind them. So is every action across a boundary in the two ticks after its region
was restored. Waiting costs a list in the state and closes both.

#### 2.4 What players of other regions do to blocks

A remote action `(edge, action)` through an edge the region knows is answered with one
outbox entry for that edge, as today. With `c` the chunk of `action.step.concerns()`:

- **`c` is `Held`**: the step is taken as today. A `PlaceAgainst` whose `against` is a
  block goes on to its `target`:
  - the chunk of `target` is `Held`: placed if the spot is free, `RemoteDone`;
  - it is `Foreign(r)`: `Remote { action, to: r }`, the action being the `Place` step;
  - it is `Asked` or `Unknown`: `NotMine { what: Misdirected::Remote(action), holder:
    None }`, the action being the `Place` step. The region has done its part and does
    not know who does the rest; the region the player is in does, as the spot is on
    that player's screen.
- **`c` is `Foreign(r)`**: `NotMine { what: Misdirected::Remote(action), holder:
  Some(r) }`, the action as it came.
- **`c` is `Asked` or `Unknown`**: `RemoteDone`. The action is dealt with, without
  effect. This is what `apply_remote` does today for a block the region does not have,
  for the reason it gives: regions that disagree about who has what must not pass an
  action back and forth for ever.

So `NotMine` without a holder is made in exactly one case, the second step of a
placement, at most once per action; `NotMine` with a holder follows a belief, and
beliefs form no ring (section 2.2); everything else ends the action. Every remote
action is answered `RemoteDone` after finitely many regions.

ADR-0010 has an action for a chunk whose holder the region does not know sent "back to
the region that sent the item, which is told to ask again who holds the chunk". Sent
back, it
meets a region that believes what it believed, or has just been made to forget it and
cannot say where the action goes: either way it comes back again. What makes a region
ask again is its edge finding that the chunk is not where the region said (section
4.3), which the edge finds whether or not anybody acts on the chunk.

#### 2.5 What a tick's outbox entries are, and their order

`TickOutput::durable` has, in this order: the entries of step 4, `Refused` and
`NotMine` for an arrival, in the order of `player_changes`; the answers of step 5, one
for one; the `Remote` entries of step 6, in the order the inputs were applied; the
`Departed` entries of step 8. Departures are numbered last, as today, so that numbers
ascend on a link in the order the runner publishes them.

`Departed`, `Refused`, a `Remote` of a player's own action and a `NotMine` for an
arrival go to the outbox of the edge the player belongs to or arrived through;
`RemoteDone`, and a `Remote` or `NotMine` that answers a remote action, to that of the
edge the action came through. That is today's rule with the new entries added.

**The removal of an entity that departed and that nobody will pass on** (ADR-0008,
section 4) is reported as today, with the chunk of the departure's pose: a chunk the
region believed another's when it let the player go.

#### 2.6 Since when a region knows an edge

`EdgeState` gains `since: u64`: the number of the tick in which the state was made.
`EdgeEvent::Started` sets it when it notes an edge the region does not know and when it
resets one for a higher start; nothing else changes it. `EdgeDelta` carries it.

A tick's number is good for this. A state is made by a tick that changes the region,
which is committed before anything of it is published, so a `since` an edge has been
told is never given out again: the ticks whose numbers can be used twice are those
that committed nothing (ADR-0008, section 4). No state is made in tick 0, so 0 is what
an edge says that has heard none.

### 3. Loading

- **A chunk is loaded only when held.** Step 2.6 of the tick asks storage only for a
  `Held` chunk with a ticket, of either kind. A ticket on a chunk that is `Asked`,
  `Foreign` or `Unknown` loads nothing; when the chunk is granted, it is asked of
  storage in the same tick.
- **A player does not load a chunk** by standing in it, as today: the viewer's ticket
  of their own edge does.
- **A chunk that arrives for a chunk no longer held**, or no longer ticketed, is
  dropped, as one that arrives after its last ticket went is today.
- **A loaded chunk that is returned** leaves the region in `TickOutput::dropped`, with
  its content after that tick. The runner saves it if it has changes the store has not
  got, behind the commit of that tick and before the `Return` (section 4.2). The sim
  hands the chunk out, instead of keeping it readable for one more tick, so that
  "loaded" goes on meaning "held", and so that a test of the sim alone can see that
  nothing is lost: a change made to the chunk in the very tick that returns it, by a
  remote action for a guest, is in the chunk that comes out.
- **A chunk that is unloaded because its last ticket went** is saved by
  `RegionRunner::release` before the tick that drops it, as today. That stays as it
  is: it is the same for a held chunk whatever kind the ticket was.
- **Tickets on a chunk that turns out to be another's.** A viewer's ticket stays: it is
  the region's reason to go on knowing who holds the chunk, and to claim it when that
  region gives it back. A guest's ticket is taken back by the runner, which tells the
  guest `NotMine` (section 4.4).
- **A block action on a held chunk that is still being loaded** is acknowledged without
  effect, as today. It does not wait: a chunk that cannot be read never arrives (open
  question 3).

### 4. The runner

#### 4.1 What a runner is made of

`RegionRunner::restore(config, store, restored)` keeps its shape. It makes the region
with `Region::restore(config, state, Holdings { held, pinned })`, `held` being the
chunks of `Restored::held` without their ticks, which only the store needs, and
`pinned` being `Restored::pinned`. `RegionConfig` has no area any more and gains
`return_after`.

**The bytes of a state.** `EdgeState::since`, `PlayerState::waiting`, and `Departed`
and `Remote` naming a region change what the postcard of a `RegionState` and of a
`StateDelta` is, and old bytes read as the new shape are not an error in every case: a
state without edges reads the same, and one with edges can read as something else.
So, from this step on:

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
3. For each `(chunk, content)` of `dropped` that is among `unsaved`: `Save { position,
   tick, chunk: content }`, and the chunk leaves `unsaved`.
4. `Return { chunks: returns }`, if there are any.
5. `Claim { chunks: claims }`, if there are any.
6. The checkpoint, if it is time, as today.

A returned chunk is in no later save: `unsaved` holds only loaded chunks, and a
checkpoint saves from `unsaved`. So every save of a chunk is before its return on the
handle, which is what the store relies on (ADR-0011, section 3.3), and it is the
runner that makes sure. A chunk that was returned without being loaded was saved when
it was unloaded, or never changed.

A claim does not wait for the commit of its tick, and neither does a return. A grant
made for a tick that is then lost is in `Restored::held` at the next opening, and the
region gives the chunk back if it has no need of it; a return made for such a tick was
of a chunk whose saves the store drops if their commit fails, with the handle
(ADR-0011, section 3.3, step 2).

**Answers** (`RegionRunner::take_replies`):

- `Claimed { granted, foreign }`: both are added to the inputs of the coming tick. The
  store gives them in the order of the claims, so the region hears them in that order.
  A runner that ticks no more (it is releasing the region) drops them.
- `NotHeld { position, holder }`: by this record a region loads and saves only what it
  holds, and holds only what the store has granted and it has not returned. So this
  answer means that the region and the store disagree, and the runner **gives up as
  for a lost store** (`RegionRunner::give_up`): the region is opened again and learns
  what it holds. Step C1.3 handles it as an unreadable chunk; this replaces that.
- The others as today.

**The store need not say that a return is through** (ADR-0011, open question 2). The
region forgets the chunk in the tick that returns it and tells its guests then; an
edge that asks another region for the chunk before the store has freed it is told
that this region still holds it, and asks again (section 5, guarantee 9).

**No region commits a change to a chunk it does not hold** (ADR-0011, open question
1): a block is changed only in a loaded chunk, and the tick that returns a chunk
changes it, if at all, before it hands it out. The store may refuse such a commit if
it comes to look.

#### 4.3 Subscriptions on a link

A link has at most one subscription per chunk. It has a kind, viewer's or guest's, and
is in one of three conditions: **waiting** for an answer, **served** (the snapshot has
been made), or told **elsewhere** with the region named (a viewer's only).
`EdgeLink::subscriptions` becomes a map from the chunk to both, and
`awaiting_snapshot` the set of those that wait.

| The link says | It has for the chunk | The runner |
|---|---|---|
| `Subscribe` | nothing | notes a viewer's subscription, waiting; adds a viewer's ticket |
| `Subscribe` | a viewer's, waiting or served | nothing |
| `Subscribe` | a viewer's, told elsewhere with `r` | makes it wait again; adds `(chunk, r)` to `unbelieve` of the coming tick. This is how an edge asks again |
| `Subscribe` | a guest's | makes it a viewer's, in the condition it is in; adds a viewer's ticket and takes a guest's back |
| `SubscribeAsGuest` | nothing | notes a guest's subscription, waiting; adds a guest's ticket |
| `SubscribeAsGuest` | a guest's | nothing |
| `SubscribeAsGuest` | a viewer's, waiting or served | makes it a guest's, in the condition it is in; adds a guest's ticket and takes a viewer's back |
| `SubscribeAsGuest` | a viewer's, told elsewhere | makes it a guest's, waiting; tickets as above |
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

After `Region::tick`, and after the tick's events have been sorted to the links by the
subscriptions as they were while the tick ran:

1. **For each chunk of the tick's `returns`**, every link subscribed to it is told
   `NotMine { chunk }`, the subscription is forgotten and its ticket taken back with
   the coming tick. Those are guests: a chunk with a viewer's ticket is needed and is
   not returned.
2. **For each link, for each of its subscriptions that waits**, by
   `Region::knowledge` of the chunk:

   | The region | A viewer's | A guest's |
   |---|---|---|
   | holds it, and it is loaded | `ChunkSnapshot`, as today; served | the same |
   | holds it, and it is not loaded | waits | waits |
   | has asked | waits | waits |
   | believes `r` to hold it | `Elsewhere { chunk, region: r }`; told elsewhere with `r`, and the ticket stays | `NotMine { chunk }`; forgotten, the ticket taken back |
   | knows nothing | waits (it cannot be: a viewer's ticket makes the region ask) | `NotMine { chunk }`; forgotten, the ticket taken back |

So `Region::knowledge` after a tick, together with that tick's `returns`, is how the
runner learns from the tick which subscribed chunks the region does not hold and who
does. Looking at what the region knows, for the subscriptions that wait, covers a link
that subscribed before the answer came and one that subscribes to a chunk the region
has long known not to hold in the same way; a list of what became known in a tick
would need a second path for the latter.

A subscription that is told elsewhere is not looked at again until the link asks
again. If the region comes to hold the chunk through another link's asking, this link
hears nothing of it until it asks, and then gets the snapshot: an edge is never sent
a chunk by a region it believes not to serve it.

A chunk the store cannot read is waited for without end and holds nothing up, as today
(`RegionRunner::unreadable`). It is the one subscription that is answered with
silence.

All of this is part of what the tick produced and is published with it, when its
commit is confirmed.

#### 4.5 Hello and welcome

`EdgeToWorker::Hello` gains `since` and `guests`:

- `chunks` are the viewer's subscriptions the link begins with and `guests` the
  guest's, each noted as section 4.3 has it for a link that has nothing; a chunk in
  both is a viewer's. `chunks` is every chunk a viewer of the edge whose player is this
  region's has in view, **also those another region serves**: the region has forgotten
  what it believed if it was restored, and asks again for each.
- **The hold.** Every chunk of both lists holds the link, as the hello's chunks do
  today, and is let go of when its subscription is answered: with a snapshot, with
  `Elsewhere` or with `NotMine`, or when the store says the chunk cannot be read.
- `since` is the `EdgeState::since` the edge last had from this region in a welcome, 0
  if none.

With "the state" being the region's before the tick that takes the hello, as today:

- The welcome is **`Resumed { entries }`** if the state knows the edge with the start
  of the hello and with its `since`. Then the hello's `seen` is turned into
  `EdgeEvent::Confirmed`, and the outbox entries above `seen` follow the welcome;
  `entries` is how many.
- Otherwise it is **`Unknown { since, entries }`**, and `seen` is **not** turned into a
  confirmation: it is a number of a numbering the region does not share.
  - If the state knows the edge with that start and another `since`: `since` is the
    state's, and every entry of the state's outbox follows, from its first.
  - If the state does not know the edge, or knows it with a lower start: `since` is
    the number of the tick that takes the hello, which is what the region gives the
    state it makes in that tick, and no entry follows.
- `Superseded` as today.

`EdgeEvent::Started` is passed in as today in every case. Presence answers follow the
entries, and are `Absent` after `Unknown`, as today. The link takes numbered messages
after `Unknown` as it does today (`EdgeLink::unknown`): from the number after the last
the region received.

After `Unknown` with another `since`, the region has received nothing from the edge
under the state it has: an edge sends numbered messages only when it has had the
welcome, and then it has the `since`. In C2b such a state has an empty outbox as well.
The rule is there for step C3, in which a region comes by a state for an edge by
absorbing another.

#### 4.6 The order on a link

What a tick produced for a link is published in this order: the welcome, its entries
and the presence answers, if the tick took a hello; the tick's events; who entered the
world; outbox entries other than departures; acknowledgements; departures; snapshots;
`Elsewhere` and `NotMine` for chunks, those of returns first and then the answers in
the order of the chunks; progress.

The events of a tick go to a link by its subscriptions as they were when the tick ran,
in whatever condition, as subscriptions that await a snapshot count today. A guest of
a chunk that the tick returned therefore has that tick's events of the chunk before
its `NotMine`, and nothing of the chunk after it.

#### 4.7 Entities nobody will pass on

`EdgeLink::visible` sends the removal of an entity among `orphaned` to every link when
the region does not hold the chunk it names (`Region::knowledge` is not `Held`), in
place of "when the chunk is outside the area".

#### 4.8 Checkpoints, release and stopping

A checkpoint is as today: every chunk of `unsaved`, which are loaded and so held, then
the state. A release is as today as well. A region that is released keeps its chunks
(ADR-0010, section 1); the next owner is told them in `Restored::held`, starts the
time before a return anew for each, and gives back what nobody comes to need. What the
released runner had asked and not heard, it drops; a grant among it is in
`Restored::held`. The flush that ends a release is answered only when the returns
before it are through (ADR-0011, section 3.3). A runner that is told to stop
(`RegionRunner::run`) checkpoints and flushes as today, and returns nothing on its way
out.

#### 4.9 Where the players are

`Region::crowds()` gives the chunks with players in them, each with how many, in
ascending order (`clustine_rpc::Crowds`). The runner puts it into `RegionStatus` after
a tick in which it changed, beside `players`, and counts the held chunks there as
well. Nothing sends it until step C4.

### 5. What an edge can rely on

Per link, and what the region expects of the edge in turn.

**Welcome**

1. The welcome is first. Exactly `entries` `Outbox` messages follow it, in ascending
   order of their numbers, then one `Presence` for each player of the hello, then the
   tick's ordinary output.
2. `Resumed` means the region has the edge's numbering: `seen` was taken, and what the
   edge kept goes on from where the region is. `Unknown { since }` means it has not:
   nothing of the hello's `seen` was taken, the entries that follow are numbered from
   the region's own first, and messages are numbered from 1 again. The edge says that
   `since` in every later hello to this region.

**Subscriptions**

3. Every subscription is answered once, and again each time the edge asks again: a
   viewer's with a `ChunkSnapshot` or with `Elsewhere`, never with `NotMine`; a
   guest's with a `ChunkSnapshot` or with `NotMine`, never with `Elsewhere`. The one
   exception is a chunk the store cannot read, which is answered with nothing, as
   today.
4. `Elsewhere { chunk, region }` is what the store said, at that moment or, if the
   region had it from an earlier answer for another need, then. The viewer's
   subscription **stays** with the region: it is why the region goes on knowing who
   holds the chunk, passes on what its players do there, and takes the chunk when it
   is free. The edge ends it with `Unsubscribe` when no viewer of this region's
   players sees the chunk any more.
5. To ask again, the edge says `Subscribe` for the chunk once more. If the region
   still believes what it last told this link, it asks the store; if it has heard
   otherwise since, it answers with that. Either way an answer comes.
6. `NotMine { chunk }` ends a guest's subscription. It comes when the guest asks for a
   chunk the region does not hold, and when the region gives the chunk back. Every
   event of the chunk up to the tick that gave it back has come before it; nothing of
   the chunk comes after.
7. **A guest keeps nothing.** A region gives back a chunk outside its pinned areas 30
   seconds after it last needed it, whoever is a guest of it.
8. **A viewer's subscription is a claim.** The region takes a chunk nobody holds
   because a viewer asked, and keeps it while a viewer's subscription is there. So an
   edge asks as a viewer only for a player it believes to be this region's, and when
   the player is let go it turns their chunks here into guest's subscriptions (those
   that are served) or ends them (those told elsewhere), unless another of this
   region's players sees them. Changing the kind of a served subscription costs no
   snapshot and loses no event.
9. A region that has just said `NotMine` for a chunk can be named in an `Elsewhere`
   for it a moment later by the viewer's region: the store frees a returned chunk
   when its saves are durable. The edge asks the viewer's region again after a short
   while, as ADR-0010 has it. Until some region serves the chunk again nothing in it
   changes, as nobody simulates it.
10. A new link begins with nothing: the hello names every subscription. All of them
    hold what the link sends until each is answered.

**Players**

11. `Departed { player, transfer, to }` names the region the store said holds the
    chunk the player stands in. That region takes the player in **whether or not it
    has heard that it holds the chunk**, unless it believes a third region to hold it;
    then it answers `NotMine { what: Arrival { player, transfer }, holder: Some(third)
    }`, and the arrival goes there. An arrival is never answered `NotMine` without a
    holder, and a player passed on this way ends in a region after finitely many.
    `to`, or a holder, can be the region the player came from two steps ago; it is
    handled there like any other.
12. A `Departed` can come in the same tick as the `Elsewhere` for the chunk the player
    stepped into, when the player was there before the answer; the `Departed` is
    first. Usually the `Elsewhere` came long before.
13. A region takes in a player who arrives for a chunk it has had nothing to do with,
    and claims the chunk. An edge that sends an arrival to the wrong region therefore
    does not lose the player: that region asks the store, and lets the player go to
    the holder.

**Blocks**

14. `Remote { action, to }` and `NotMine { what: Remote(action), holder: Some(to) }`
    both mean: pass the action to `to`. `NotMine { what: Remote(action), holder: None
    }` is made only for the second step of a placement and means: pass it to the
    region the player is in now; if the edge no longer has the player, it is dropped.
    Every action passed on is answered `RemoteDone` by some region after finitely
    many.
15. `RemoteDone` can mean "without effect, because this region does not hold the chunk
    and does not know who does". That happens when a region gave the chunk back while
    the action was on its way. The player sees the block as it is; nothing is passed
    on again.
16. An action of a player about a chunk their region has no answer for is not lost and
    not acknowledged: the player's inputs wait in the region, in order, until the
    store has answered. The edge need not hold anything back after a hand-over, and
    need not send the player's subscriptions before their arrival, though sending
    them first lets one claim ask about everything.
17. A region knows who holds a chunk that its player acts on only by a viewer's
    subscription for it, or by asking when the action comes. Without the subscription
    every such action waits a tick or two for the store. That is the second reason
    for guarantee 4.

**Entities**

18. The removal of an entity that departed and that nobody will pass on comes on every
    link of the region, whatever the link is subscribed to. Only if the region has
    come to hold the chunk the entity was last seen in does it come, like any other
    event, on the links subscribed to that chunk.

### 6. Changes to messages and types

Beyond step C0, and ADR-0011, section 8.

**`clustine-sim`**

```rust
pub struct RegionConfig {
    pub spawn: Vec3,
    pub starting_hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
    /// How many ticks a chunk the region holds outside its pinned areas may be without
    /// need before the region gives it back.
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
```

| Type | Change |
|---|---|
| `TickInputs` | `tickets_added` and `tickets_removed` become `Vec<(ChunkPos, Ticket)>`; `unbelieve` becomes `Vec<(ChunkPos, RegionId)>`; `granted` and `foreign` as they are |
| `TickOutput` | new `dropped: Vec<(ChunkPos, Chunk)>`; `claims` and `returns` as they are, in ascending order |
| `Durable::Departed` | gains `to: RegionId` |
| `Durable::Remote` | `Remote(RemoteAction)` becomes `Remote { action: RemoteAction, to: RegionId }` |
| `Durable::NotMine`, `Misdirected` | as they are; now made |
| `EdgeState` | gains `since: u64`, behind `start` |
| `EdgeDelta` | gains `since: u64`, behind `start`; `RegionState::apply` sets it |
| `PlayerState` | gains `waiting: Vec<(u64, PlayerInput)>`, last |
| `PlayerTransfer`, `PlayerChange`, `RemoteAction`, `RemoteStep`, `EdgeEvent` | as they are |

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

pub enum Welcome {
    Resumed { entries: u32 },
    Unknown { since: u64, entries: u32 },
    Superseded,
}
```

`Subscribe`, `SubscribeAsGuest`, `Unsubscribe`, `PlayerArrive`, `Remote`,
`WorkerToEdge::Elsewhere` and `WorkerToEdge::NotMine` keep their shapes and get the
meaning of sections 4.3 and 4.4. No message is added: an edge asks again with
`Subscribe`, and the store's interface is ADR-0011's without a change.

**`services/worker`**: `DEFAULT_RETURN_AFTER: u64 = 30 * 20`; `STATE_FORMAT: u8`, which
is 2 when all of this record is built (section 8); `RestoreError::Format { tick,
format }`; `RegionStatus` gains `held` and the crowds.

**What each breaks**

- `RegionConfig::area`, `Region::new`, `Region::restore`: every fixture of the sim's
  tests (`config`, `fresh`, `region_in`, `joined_in`, `on_floor_in`,
  `east_with_player`, `west_with_floor` in `region.rs`; `config_a`, `config_b` in
  `tests/specification.rs`; `AREA` in `tests/state.rs`), `config` and `runner_of` in
  the worker's unit tests, `config` in `services/worker/tests/specification.rs`, `hold`
  in `bin/clustine/src/cluster.rs`, and `Regions::run`, `run_first` and `started` in
  `bin/clustine/src/lib.rs`.
- Tickets with a kind: `tickets` in `region.rs`, `load` in `tests/specification.rs`,
  one use in `tests/state.rs`, and `RegionRunner::subscribe` and `release`.
- `Durable::Departed` and `Durable::Remote`: `told`, `requests` and `outcomes` in
  `region.rs`; some thirty places in the sim's and the worker's tests;
  `RegionRunner::tick`; `Fanout::outbox` in the edge; the round trip in
  `clustine-rpc/src/link.rs`.
- `PlayerState::waiting`: `Player::to_state` and `Player::from_state` in `region.rs`;
  nothing else writes a `PlayerState` out.
- `Welcome` and the hello: `RegionRunner::tick` and `hello`; `Fanout::take_link` and
  `welcomed`; `Link::hello`, `welcome` and some thirty uses in the worker's
  specification tests; the edge's unit tests; `link.rs`.
- `EdgeState` and `EdgeDelta`: `Region::apply_edge_event`, `take_delta`, the sim's
  tests that write an `EdgeState` out.
- The tests whose point was the area change their point, not only their fixtures; the
  step that changes them (section 8) says what becomes of each:
  `chunks_outside_the_area_are_neither_requested_nor_loaded` (tickets on what is not
  held load nothing), `a_player_is_let_go_exactly_where_the_area_ends` (where the next
  chunk is another's), `a_player_who_arrives_outside_the_area_is_passed_on_unchanged`
  (answered `NotMine` with the holder, and not taken in),
  `a_player_who_joins_outside_the_area_is_let_go_at_once` (as it is, with `to`),
  `a_remote_placement_into_a_spot_of_another_region_is_dropped_and_not_passed_on` and
  `a_remote_action_for_another_region_is_answered_for_the_edge_it_came_from` (by what
  the region knows of the chunk).

**How the sim's fixtures play the store.** A fixture `Grants` keeps who holds which
chunk: pinned areas with their regions, and grants. `Grants::answer(region, claims)`
gives `granted` and `foreign` as the store would, `Grants::take_back(region, returns)`
frees. A fixture for a region with its store runs a tick, answers that tick's claims
into the inputs of the next, and takes its returns. Tests that are not about asking
make their region with what it needs already known: `Holdings::held` naming the chunks
the test uses, and, where a neighbour matters, viewer's tickets on the neighbour's
chunks near the line, answered before the test begins. With that a player who steps
across is let go in the tick of the step, and a block across the line is passed on at
once, as the existing tests expect. The tests that run two regions against one
(`two_regions_match_one` and the block tests behind it) give each player viewer's
tickets on the chunks around the line at the start, which is the edge's part, and one
`Grants` for both.

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
- **The edge**: its use of `Routing::layout` goes with its own step.

### 8. Building it

Each step leaves `cargo test --workspace` and the run with
`CLUSTINE_TEST_BOUNDARIES=0,4` green. A sim without an area cannot run the stripes
until the runner asks the store and the edge subscribes at the viewer's region, and
those are three steps. So the second step puts a scaffold into the sim that the last
one removes: `Holdings::presumed: Vec<(ChunkArea, Option<RegionId>)>`, areas whose
holder the region takes as given, `None` for itself. A chunk in one of them is `Held`
or `Foreign` from the start, is never claimed, returned or forgotten, and `granted`,
`foreign` and `unbelieve` for it are ignored. The processes give every region the
stripes of the layout that way until the last step, so that what players see changes
in one step that changes nothing else.

| # | Scope | Needs | The edge in the same commit | Its tests |
|---|---|---|---|---|
| C2b.1 | `since`, the welcome with its entries, `seen` not confirmed after `Unknown`, the format byte and states from before | nothing | says and keeps `since`; reads the new welcome | Below, S16; R11, R12, R14 |
| C2b.2 | The sim of sections 1 to 3, with `presumed`. The runner as it is, with every ticket a viewer's, `Region::knowledge` in place of `Region::area`, and `presumed` made from the layout it is handed | C2b.1, for one change of format less | the new shapes of `Departed` and `Remote`, routed as before | Below, S1 to S19; every existing test of the sim on the new fixtures; the worker's and all end-to-end tests as they are |
| C2b.3 | The runner of section 4: claims, returns and their answers, `NotHeld`, kinds and conditions of subscriptions, `Elsewhere` and `NotMine`, the hello's `guests` and the hold, `Holdings` from `Restored`, crowds | C2b.2; ADR-0011's C1.3 and C1.4 | a hello with empty `guests` | Below, R1 to R10, R13, R15 to R18, on stores with a division; the end-to-end tests as they are, still presumed |
| C2b.4 | The edge without a layout (designed elsewhere) | C2b.3 | all of it | Its own; the end-to-end tests, still presumed |
| C2b.5 | `presumed` goes, from the sim, the runner and the processes (section 7) | C2b.4 | nothing | Hand-over, block, takeover, chaos and move tests on two pinned regions; kind |

C2b.1 and C2b.2 touch no code of the store and can be built while C1 is. C2b.2 is the
sim alone but for mechanical changes in the worker, the edge and the binary. C2b.3 is
the runner alone but for one field. C2b.1 makes `STATE_FORMAT` 1 and C2b.2 makes it 2.

Until C2b.4 the edge finds regions by the layout, and every region presumes the same
layout. So nothing waits, no arrival and no remote action reaches a region for a chunk
it believes another's, and no placement's second step meets a chunk its region knows
nothing of: the edge of C2b.2 and C2b.3 is never sent a `NotMine` entry, `Elsewhere`
or `NotMine` for a chunk, and goes on logging them as today.

**For whoever writes tests from this record alone.**

*The sim* is driven as in `crates/clustine-sim/tests/specification.rs`: a `Region`, a
`TickInputs` per tick, the `TickOutput` and `Region::state`. The test plays the store
(it decides what `granted` and `foreign` a later tick is given for the `claims` of an
earlier one, and when) and the edges (tickets). `return_after` is 0 unless a scenario
says otherwise. The worlds: **stripes**, region 0 pinned to the chunks with x below 1
and region 1 to the rest, as `config_a` and `config_b` have it; and **open land**, a
region pinned to nothing. After every tick of every scenario the four statements at
the end of section 1.3 hold, and the delta turns the state before into the state
after, as `checked_tick` checks today.

- S1. A new region pinned to an area holds nothing: a chunk of the area is `Unknown`.
  A viewer's ticket puts it into that tick's `claims`, once, and it is `Asked`;
  `granted` makes it `Held` and puts it into `chunk_requests` of the same tick; it is
  never in `returns`, whatever `return_after` is.
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
  `NotMine` with the arrival and `Some(r)`, the players are as before and no entity is
  reported. An arrival of a player who is there is as today in all three.
- S9. A dig within reach at a block of a chunk that is `Foreign(r)`: `Remote { to: r
  }`, not acknowledged. At one of an unknown chunk: nothing is acknowledged or
  changed, `last_input` is as before, the chunk is in `claims`, the input is in
  `PlayerState::waiting` and in the delta. A move of the same player in a later tick
  waits behind it; another player's inputs are applied. Then `foreign` with `r`: in
  that tick the dig is passed on to `r` and the move applied, `waiting` is empty and
  `last_input` is the move's. With `granted` instead and the chunk not loaded: the dig
  is acknowledged without effect.
- S10. A placement against a block of a held chunk into a spot of an unknown chunk
  waits and places nothing; against a block of a chunk that is `Foreign(r)` it is
  `Remote { to: r }` with `PlaceAgainst`.
- S11. A remote action about a held chunk is as today. About a chunk that is
  `Foreign(r)`: `NotMine` with the action and `Some(r)`. About an asked or unknown
  chunk: `RemoteDone`, and nothing changes. A `PlaceAgainst` on a block of a held
  chunk: with the target's chunk held, placed and `RemoteDone`; with it `Foreign(r)`,
  `Remote { to: r }` with `Place`; with it unknown, `NotMine` with `Place` and no
  holder.
- S12. On open land, a held chunk that nothing needs is in `returns` once and is
  `Unknown` after; if it was loaded it is in `dropped` with a block that a remote
  action broke in that very tick. Not returned: a chunk with a player in it, with a
  viewer's ticket, the chunk of the spawn point. A guest's ticket does not keep it.
  With `return_after` 5 it is returned five ticks after the first tick in which
  nothing needed it and not before, and needing it in between starts the count anew.
- S13. A ticket of either kind on a chunk that is not held asks storage for nothing. A
  chunk delivered for a chunk that has been returned is not taken.
- S14. `Region::restore` with a state and `Holdings`: `Held` is what `held` names and
  nothing else is known; a player of the state who stands in a chunk that is not held
  is still there after the first tick, and the chunk is in its `claims`; an input in
  `PlayerState::waiting` is tried in the first tick and asks again.
- S15. A region with the empty block of entity ids answers a join with `Refused`.
- S16. `since` is the number of the tick that noted the edge; a higher start gives a
  new one; the same start, a confirmation and `applied` leave it; an edge that is gone
  and comes back in a later tick has a higher one.
- S17. What waits is dropped when the player leaves, when their edge is reset or gone,
  when they are let go and when they join anew. An input whose number is not above the
  last one waiting is dropped.
- S18. The same inputs, with the same `granted` and `foreign`, give byte-identical
  states and identical `claims`, `returns` and `dropped`.
- S19. Two regions on stripes, with one `Grants` and viewer's tickets around the line,
  treat players and blocks as one region does that holds everything, as the existing
  tests of that kind assert.

*The runner* is driven as in `services/worker/tests/specification.rs`: a `Link` per
edge, the runner stepped by the test, every wait a loop until a message or a state is
there. The store is `Store::memory_divided` and `Store::local_divided` of ADR-0011,
with **stripes** at a boundary at 1, of which region 0 is under test and has the home
chunk and region 1 is the other; or with the division **with a gap** of ADR-0011,
section 9, of which the home region, region 2, is under test: it is pinned to nothing
and holds the home chunk alone, the chunks between the two pinned areas are free, and
"the neighbour" is region 1. The test plays the neighbour through a handle of its own
(`Store::open_region`, `StoreRequest::Claim`, `Return`, `Load`, `Save`). A "crash" is
the region opened again with a higher epoch. `return_after` is 0 unless said.

- R1. A viewer's subscription to a chunk of the region's own stripe is answered with a
  snapshot; to one of the other stripe with `Elsewhere` naming region 1 and no
  snapshot. A second `Subscribe` for the latter is answered `Elsewhere` again.
- R2. A guest's subscription to a chunk of the own stripe is answered with a snapshot;
  to one of the other stripe with `NotMine`. With the gap: a guest's subscription to a
  free chunk is answered `NotMine`, and the neighbour's claim of that chunk afterwards
  is granted.
- R3. With the gap: a viewer's subscription to a free chunk is answered with a
  snapshot, and a crash later the chunk is in `Restored::held`.
- R4. With the gap, the test's neighbour holding a chunk: a viewer's subscription is
  answered `Elsewhere` with the neighbour. The neighbour returns the chunk; a second
  `Subscribe` is answered with a snapshot.
- R5. A served viewer's subscription that is made a guest's gets no second snapshot.
  With the gap, the chunk being one the region was granted: `NotMine` follows, and
  the neighbour's claim is granted once the return is through (the test claims until
  it is). With `return_after` 40, `NotMine` comes no earlier than 40 ticks after.
- R6. **A chunk leaves saved.** With the gap: a player breaks a block of a granted
  chunk; the viewer's subscription becomes a guest's; after the `NotMine` the
  neighbour claims the chunk and loads it: the block is broken. The same after a crash
  of the region right behind the `NotMine`, when the store may have dropped the return
  with the old session: whichever of the two regions then holds the chunk loads it
  with the block broken.
- R7. A player who walks from the home chunk into the other stripe is let go with
  `Departed { to: 1 }`, also when no subscription had asked about that chunk before.
- R8. An arrival for a chunk of the other stripe that a viewer's subscription has
  asked about is answered with the outbox entry `NotMine` naming region 1; one for a
  chunk of the own stripe that nothing has asked about is taken in.
- R9. A player digs a block of the other stripe right after joining, with no
  subscription: `Remote { to: 1 }` comes, after the tick or two the store takes, and
  what the player sent behind the dig is reported applied after it.
- R10. A hello with `chunks` in both stripes and `guests` in both: nothing the link
  sends behind it is applied before each of them is answered, with a snapshot,
  `Elsewhere` or `NotMine`; then all of it is.
- R11. A first hello is welcomed `Unknown { since, entries: 0 }` with `since` above 0;
  a hello that says that `since` is welcomed `Resumed`; one that says another, or 0,
  with the same start is welcomed `Unknown` with the same `since` again, the region's
  outbox for the edge is as it was, and no entry was dropped for the hello's `seen`.
- R12. `entries` is the number of `Outbox` messages between the welcome and the first
  presence answer, with none, some and all of an outbox seen.
- R13. `Elsewhere` and `NotMine` for a chunk are part of a tick: on a world in memory
  and on one on disk they come behind everything of the ticks before theirs and before
  anything of the ticks after it; and a runner whose store was taken by another owner
  after the tick ran sends neither.
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
  subscribed to nothing.
- R18. A link that ends takes its tickets with it: with the gap and `return_after` 0,
  the region's granted chunks without a player are returned; with `return_after` 40
  and a new hello within that time, none is.

In the worker's unit tests, which can put a `Gate` before the store: a `NotHeld` ends
the runner as a lost store does; and the order of section 4.2 on the handle, with the
save of a dropped chunk before its `Return`.

## Ruled out

- **Answering a viewer from what the region believed long ago**, by keeping beliefs
  until a restore. It saves the store a question when a player comes back, and makes
  what a region believes depend on everyone who ever passed by.
- **Ending a viewer's subscription with `Elsewhere`**, so that an edge has one
  subscription per chunk. A region then learns who holds the chunks around its players
  once, and has nothing that tells it to ask again after it was restored, or when the
  holder changed; what its players do across a boundary would wait for the store
  every time.
- **A message of its own for asking again.** A second `Subscribe` says it.
- **Passing a block action on without naming a region**, for the edge to send to
  whoever serves it the chunk. The edge always knows that, and it would spare the
  region its beliefs; but ADR-0010 has regions name where things go, and an entry that
  is read again after a resume would mean something else each time.
- **A list of what became known in a tick** as the tick's output for the runner; see
  section 4.4.
- **Telling guests `NotMine` only when the return is through.** It spares the edge
  asking twice, and needs an answer from the store and a runner that holds messages
  outside a tick.
- **Keeping an edge's viewer's tickets while it has no link.** The time before a
  return does the same with one number.
- **Sending the load with the claim**, which the store would allow (ADR-0011, section
  3.1). It saves a tick per chunk that is new to a region; see the consequences.
- **Building sim, runner and edge in one commit**, without `presumed`. Nothing in
  between could be verified.

## Consequences

- A region asks before it loads: a chunk that is new to it is on a client about a tick
  later than today, and so is everything a restored region sends again, on top of the
  0.75 seconds a resume was measured at in phase B.
- At a boundary, the half of a view that another region serves comes two or three
  ticks after the near half the first time: the viewer's region asks the store, says
  `Elsewhere`, and the edge subscribes there.
- A player who is handed over makes the new region ask about every chunk in their view
  that it does not hold: one claim of a few hundred chunks, answered from the table,
  and as many `Elsewhere`. A player who stands on a boundary does that every tick
  they are handed back. It writes nothing; the edge can spare it by not moving a
  view's subscriptions at once.
- The first thing a player does to a block of the region they just left, or of any
  chunk their new region has no answer for yet, takes a tick or two longer than what
  they do across a boundary otherwise, which is a tick or two already (ADR-0006).
- Chunks follow players with a lag of `return_after`: a region sheds what is behind
  its players 30 seconds after the last of them looked.
- With pinned regions that cover the world, which is all C2b runs, no chunk is ever
  returned and every claim is answered from the table. Returns, free chunks and
  beliefs that go stale are tried by the tests with a gap, and by C3.
- `RegionState` grows by a list per player that is empty but for a tick or two.

## Changes to ADR-0010

1. **Section 2, "A block action on a chunk the region does not hold or has not heard
   about is acknowledged without effect"**: it waits, in the region's state, until
   the store has answered (section 2.3 here). Acknowledged without effect is a block
   of a held chunk that is not loaded, as today.
2. **Section 2, "An arrival ... for a chunk the region does not hold is answered ...
   `NotMine`"**: only when the region believes another region to hold the chunk.
   Otherwise the player is taken in and the chunk claimed (section 2.2). A pinned
   region does not know its own chunks until it asks.
3. **Section 2, a remote action for a chunk the region does not hold**: `NotMine` with
   the holder if the region believes one; `RemoteDone` without effect if not; and
   `NotMine` without a holder only for the second step of a placement, which goes to
   the region the player is in (section 2.4).
4. **Section 3, "back to the region that sent the item, which is told to ask again"**:
   nothing is sent back for a region to ask again. A region is made to ask again by a
   second `Subscribe` of its edge, and `unbelieve` names the region that is no longer
   believed.
5. **Section 1, "A region needs a chunk while ..."**: ADR-0010 does not say when a
   chunk that is not needed is given back. Here it is after `return_after` ticks
   without need (section 1.3), and never for a pinned chunk or the home chunk.
6. **Section 3, the hello**: it names viewer's and guest's chunks apart, and the
   viewer's include those another region serves. A viewer's subscription outlives its
   `Elsewhere`.
7. **Section 3, "Knowing an edge again"**: `since` is the number of the tick that made
   the state; after `Unknown` the hello's `seen` confirms nothing.
8. **Section 2, what a region knows**: a belief is kept only while the chunk is wanted.

## Changes to ADR-0011

1. **Section 9, C1.3, `NotHeld` in the worker**: the runner gives up as for a lost
   store, instead of treating the chunk as unreadable.
2. **Open question 1** (commits to chunks not held): no region makes one. **Open
   question 2** (hearing that a return is through): not needed.

Nothing in the store's interface changes.

## Changes to ADR-0008

1. **Section 1**: `EdgeState::since`, `PlayerState::waiting`.
2. **Section 2**: an input can wait; a player's `last_input` is that of the last input
   applied, and what waits is behind it.
3. **Section 4**: the hello and the welcome of section 4.5 here; the hold ends for a
   chunk also with `Elsewhere` and `NotMine`; a state is stored with two bytes in
   front.

## Open questions

1. **Whether a region may take in an arrival for a chunk it has nothing to do with.**
   It is what makes pinned regions work, and it means that after a region gave a chunk
   back, a neighbour's player who walks into it within the moment the neighbour still
   believes the old holder ends in the old holder's region, which grows by that chunk.
   A viewer sees a chunk long before its player is in it, so the moment is short. C4
   will see whether such a region is then merged or split for nothing.
2. **`return_after`** is a guess. It has to be longer than an edge takes to come back
   after a restore, and it decides how much a region holds behind its players.
3. **A block action on a held chunk that is being loaded** is acknowledged without
   effect. When a chunk goes from one region to another under a player's eyes, 30
   seconds after the first one's players left, an action in the tenth of a second the
   new holder takes to load it is lost. It could wait like one on an unanswered chunk
   if the sim were told which chunks cannot be read. With pinned regions it cannot
   happen.
4. **A remote action that arrives while its region is asking** for the chunk (it gave
   it back and wants it again) is done without effect. The same moment as question 3,
   from the other side.
5. **Whether `NotHeld` should end the runner** once C3 exists: a split takes chunks
   from a region by its own doing while loads of them can be under way. C3 has to say
   what the runner does with those answers.
6. **Dropping states from before** was chosen over refusing them. If a world is ever
   served by builds of two formats in turn, the older one finds bytes it calls "a
   later build's" and refuses.
7. **The format byte's place.** It is the worker's, in front of bytes the store does
   not look into. The store's `meta` could carry it instead, at the price of the store
   knowing that states have formats.
8. **Which existing tests change their point** is listed in section 6 as far as they
   were found by reading the sim's tests for `area`. The worker's and the edge's were
   read for what they construct, not one by one.

## Risks

- **`PlayerState::waiting`** is new state with rules for five ways a player goes. A
  mistake there loses or doubles an input. S9 and S17 are for it, and the tests of two
  regions against one will not see it, as their fixtures make nothing wait.
- **The proof that beliefs form no ring** rests on a region that holds a chunk knowing
  so or knowing nothing of it. C3 moves chunks without a claim. A region that splits
  has to believe the new region, and one case is left for C3 to close: a pinned region
  that believes a region split off it to hold a chunk of its area still believes so
  when that region has given the chunk back, although the chunk is the pinned region's
  again by the table. Until its edge asks again, a player sent to it for that chunk is
  sent on, and comes back.
- **The scaffold `presumed`** keeps the real path out of the processes until the last
  step. What C2b.5 changes for players is the timing of the first answers; if a
  hand-over test fails only there, that is where to look.
- **The edge has more to keep than ADR-0010 said**: per region and chunk whether a
  viewer of that region sees it, apart from where the chunk is served.
