# ADR-0020: One stay per player, and a player's place kept by the world store

- Status: **Accepted**, after two independent reviews against the code, whose ten and
  twelve findings are worked in (see "Review"); the design of step R1 of the milestone
  "every service with several replicas"
  ([replicas-plan.md](../groundwork/replicas-plan.md)). Nothing of it is built or run
  yet. Section 4.1 of the plan is where it starts from.
- Date: 2026-10-10. The code it cites is as at `6bf1613`.
- It **departs from section 4.1 in six places**, gathered in section 15. The largest:
  the store never makes a login wait for another region, and a region keeps no floors.
- It changes what ADR-0012, ADR-0014 and ADR-0015 say; see "Changes to earlier
  records".
- Marks, as in the plan: a `file:line` is something I read there. **Inferred** is
  reasoning from what I read and is a claim to test. **Guess** is from memory of the
  client or the protocol and is checked against nothing here.

Short names: `fanout.rs`, `play.rs` are in `services/edge/src/`; `region.rs`,
`reshape.rs` (`region/reshape.rs`), `api.rs`, `sim/state.rs` in
`crates/clustine-sim/src/`; `messages.rs` in `crates/clustine-rpc/src/`; `lanes.rs`,
`table.rs`, `store/lib.rs` in `services/worldstore/src/`; "the runner" is
`services/worker/src/lib.rs`; `log.rs` is `crates/clustine-format/src/log.rs`.

## Context

### What a stay is today, and what orders two of them

A stay is a player's time in the world from one join to the leave that ends it, with one
entity for all of it (`api.rs:104-109`). The home region gives a joining player the next
entity id of its block (`region.rs:391-400`) and the edge sends every join to the home
region and to no other (`fanout.rs:432`, `909-914`). Of two stays the one with the higher
entity id is taken for the later: at an arrival (`region.rs:444-464`) and at a merge
(`reshape.rs:171-181`).

That order holds only while one region gives out ids from one block without ever going
back. The code breaks it in two ways:

| | Evidence |
|---|---|
| A state of another build is dropped, and the region starts again at the first id of its block | the runner:3116-3143 (`fresh` is `RegionState::new`, whose next id is `entity_ids.first`, `sim/state.rs:70-78`) |
| A world started with another division has another home region, which has a block of its own that can be lower. The region ids start again at 0 (`table.rs:140-178`), a region keeps the block its file names (`lanes.rs:1156-1160`), and blocks are issued in the order regions were first opened (`lanes.rs:1161-1167`) | The owner switches between a world without pins and `--pin 4` (`CLAUDE.md`, "Checking work") |

The plan knows the first (its rule 9) and not the second.

### What a join, a hand-over, a leave and a resume do today

| Step | What happens | Evidence |
|---|---|---|
| A second connection of an account on one edge | Refused: "You are already connected to this server." | `fanout.rs:875-878` |
| A join | The edge makes a view with no entity and the home region as its region, and sends `PlayerJoin`, numbered and kept until the region reports it applied | `fanout.rs:882-914`, `2688-2698` |
| The home region takes it | Removes whatever stay of the player it has, gives the next id, places the player at the spawn point with the starting hotbar, and says `PlayerEvent::Spawned` and `EntitySpawned` | `region.rs:380-427` |
| The loser of that | Its entity is reported removed. Its edge is told nothing | `region.rs:694-702` |
| The edge puts the player into the world | Login, abilities without the flying bit, position with yaw and pitch 0, inventory; then it asks the player's region for the view | `fanout.rs:52`, `1987-2076`, `2080-2150` |
| A player steps into a chunk the region believes another's | Removed from the region in that tick; `Durable::Departed` with the transfer goes into the outbox of the player's edge. The state's change for the player is "no longer here" | `region.rs:535-571`, `719-728` |
| The edge hands over | Only if its view has that very entity; otherwise it sends `Discard`. It asks the new region for the view first, then sends `PlayerArrive` and the inputs no region has reported applied | `fanout.rs:1789-1876` |
| The arrival | Takes the place of a stay with a lower id; is passed over if the region has the same or a later stay; goes on with `NotMine` if the region believes the chunk another's; otherwise the player is taken in | `region.rs:440-511` |
| A leave | Ends the stay it names, if it is that edge's. With no entity named it ends whatever stay of that edge there is | `region.rs:428-439`, `fanout.rs:2666-2673` |
| An edge away for 600 ticks, or back with a higher start | Its players are removed, and the entities on their way in its outbox are reported removed | the runner:71, 1739-1751; `region.rs:632-691` |
| A resume | The region answers a hello with the outbox entries the edge has not seen and then with one presence answer for every player the hello names and for every other stay of that edge | the runner:1757-1850 |
| Presence at the edge | `Absent` for a player the edge has under that region ends the connection with "The server lost track of where you are", unless a join or an arrival for them is still kept | `fanout.rs:1238-1256`, `1666-1683` |
| A stay that came with a merge or a split | Moved under the region that says it has it, with no arrival, **if the edge's view has that entity** | `fanout.rs:1263-1278`, `1585-1661` |

Nothing outlives a stay. A region's state has each player's pose, hotbar and slot
(`sim/state.rs:32-47`), handed to the store as bytes it does not look into
(`messages.rs:295-304`, `log.rs:80-90`).

**A view without an entity is found by one path only today**: a `Present` from the
region the view is under (`fanout.rs:1284-1297`). That is enough today because a join is
placed in the home chunk, which never leaves the home region (`lanes.rs:1532-1534`) and
from which nobody is let go. It is not enough once a join is placed elsewhere
(section 4.3).

### What can be built on

- **Output commit.** What a tick produced is published only when the store has
  confirmed the tick's commit (the runner:1720-1733, 1946-1955). A `Departed` is
  durable before any edge acts on it.
- **One thread of the store decides everything**, in the order messages arrive
  (`lanes.rs:1-28`, `488-496`). Commits of a group are appended, made durable with one
  sync and only then answered (`lanes.rs:645-677`, `825-890`).
- **The store can say something to a region's owner at any time**, and no such word is
  lost without a restore. An owner's replies are a queue the commit thread writes to
  (`store/lib.rs:161-167`), also over the socket, read by the runner before every tick
  (the runner:1069-1137). The runner drops inputs in two places only: a tick, and the
  taking of a merge or a split (the runner:1653, 1897).
- **The store knows who holds a chunk** (`table.rs:301-308`).
- **A tick that changes nothing sends no commit** (the runner:1946-1955).
- **A merge or a split drops what waited for the coming tick** (the runner:1652-1662)
  and makes the region what a restore makes of its new state (`reshape.rs:215-231`,
  `348-383`).

## Decision

### 1. In short

1. The world store keeps **one record per player**: the latest stay the home region
   gave them, how often that stay was handed on, and its place.
2. Regions tell the store about stays in a part of every commit that the store can
   read: **stay notes**. The region's own state stays opaque.
3. **A login** goes edge → home region → store → home region → edge. The home region
   gives the stay its number and holds it as *entering*. The store answers that commit
   with the place and with who holds the chunk of that place. **The store answers at
   once, always.** A login waits for the home region and for the store, as a join does
   today, and for nothing else.
4. **A stay that was replaced is dead** from the moment the store has the new stay's
   note on disk. Every region that runs is told at once; every region that takes a
   stay in, or begins to run, names its stays to the store and is told which are dead.
   No region is asked, and no region remembers a floor.
5. **Whoever removes a stay that did not leave by itself tells that stay's edge**, in
   the edge's outbox. The edge ends the connection with the official server's sentence.
6. **An edge shows of one player only the highest stay it has seen.**
7. **Stays are ordered by `(entity, hops)` everywhere**: in the store, at an arrival, at
   a merge, in a `Dead`. Entity ids never go back for the life of a world; the store
   sees to it when it opens the home region.
8. **A stay carries the attempt of its join until its first input is applied**, so that
   an edge whose view has no entity yet finds its stay by every path a stay can take.
9. **Flying** is read from the client, kept by the region, carried by the transfer and
   the record, and sent on entering.

### 2. The record

One per player, for the life of the world. It is never dropped.

| Field | Type | Meaning |
|---|---|---|
| `player` | `PlayerId` (u128) | Key |
| `stay` | `EntityId` | The highest stay the store was told the home region gave this player: **the floor**. Never lowered |
| `hops` | `u32` | The highest hand-over count the store was told of that stay. 0 when the stay is issued |
| `place` | `Option<Place>` | `None` until a region has written one |

`Place`:

| Field | Type | Note |
|---|---|---|
| `pose` | `Pose`: position (three `f64`), yaw, pitch (`f32`), on-ground | As `api.rs:13-22` |
| `flying` | `bool` | New |
| `hotbar` | nine `Option<ItemStack>` | As `sim/state.rs:36` |
| `selected_slot` | `u8` | |

Besides the records the store keeps one number, **`issued`**: the highest entity id any
`Entering` note ever named.

Not in the record, and why a rejoin does not need it: the name (it comes with the join,
`api.rs:79-83`); the number of the last input and the highest handled sequence (they
count within one connection, `messages.rs:92-95`); the edge and the attempt (they are of
one connection); where the stay is.

**Departure 1: the record does not say where the stay is.** The plan's record has "a
region, on its way, or ended". Nothing in this design reads it.

### 3. Stay notes: what a region tells the store

```rust
/// What a region says to the world store about a stay, in the commit of a tick.
pub enum StayNote {
    /// The home region has given `player` the stay `entity` and holds it as entering,
    /// or still holds it so and names it again.
    Entering { player: PlayerId, entity: EntityId },
    /// The stay is in this region after the tick, or was let go in it, with `hops`
    /// hand-overs behind it and this place.
    Has { player: PlayerId, entity: EntityId, hops: u32, place: Place },
}
```

**When a tick makes a note** (`TickOutput::stays`, in the order of the players, an
`Entering` before a `Has` of the same player):

| The tick | Note |
|---|---|
| Took a join | `Entering` for the stay it gave out, if that stay is still entering at the end of the tick |
| Placed an entering stay in this region | `Has`, hops 0 |
| Let an entering stay go to another region without placing it (section 4) | `Has`, hops 1, with the place the store gave |
| Took an arrival in | `Has`, with the transfer's hops |
| Changed anything of a player who is in the region after the tick: the player is among the tick's changed players (`region.rs:786-787`, `719-728`) | `Has` |
| Let a player go (`region.rs:560-571`) | `Has`, with the hops and the place of the transfer, which are those the next region begins with |
| Is the **first tick of a region after `Region::restore`**: after an opening, after a merge, after a split (for both regions) | A note for **every** stay the region has: `Has` for each player, `Entering` for each entering stay |
| Removed a player (a leave, a replacement, a dead stay, an edge that is gone) | None. The record has the last place written |

The first-tick rule is a flag of the `Region` that `restore` sets and the first tick
clears. It is not part of the state. A region run on from memory and one restored from
the store's record still do the same, because a merge and a split go through `restore`
on both paths (`reshape.rs:229`, `374-375`).

**A tick with notes is committed**, whatever else it changed. The runner's condition
(the runner:1946) becomes: block changes, or a state that changed, or notes.

**No note is made while the region's setting `place_by_store` is off** ("Building it"):
not by a join, an arrival, a change or a first tick.

**The message.** `StoreRequest::Commit` gains a field:

```rust
Commit {
    tick: u64,
    changes: Vec<(BlockPos, BlockState)>,
    state: Vec<u8>,
    /// What the region says of stays in this tick; see ADR-0020, section 3.
    stays: Vec<StayNote>,
}
```

`AbsorbCommit`, `SplitCommit` and `Checkpoint` carry none: the first tick after a merge
or a split names every stay, and a checkpoint is no news.

**A note is applied only for a commit that is taken**: after the owner's test
(`lanes.rs:634-642`) and after the record was appended (`lanes.rs:659-670`). A commit of
an owner that was replaced is dropped there today; its notes are dropped with it, or a
lost handle would raise a floor for a stay that is in no state.

**What the store does with a note**, on the commit thread, when it takes the commit and
before it answers. `record` is the player's record, made with stay 0, hops 0 and no
place if there is none (no block has id 0, `position.rs:139-149`).

| Note | Condition | The store |
|---|---|---|
| `Entering` | The commit is not the home region's | Drops it, logs an error, answers `Dead` (below) |
| `Entering` | `entity > record.stay` | **Raises the floor**: `record.stay = entity`, `record.hops = 0`, the place stays. `issued = max(issued, entity)`. Notes `(player, entity, 0)` among the group's **dead** (section 5). Answers `Enter` |
| `Entering` | `entity == record.stay` | Answers `Enter` again. Changes nothing |
| `Entering` | `entity < record.stay` | Answers `Dead` |
| `Has` | `entity == record.stay` and `hops >= record.hops` | `record.hops = hops`, `record.place = Some(place)` |
| `Has` | `entity < record.stay` | Drops the note and answers `Dead` |
| `Has` | `entity == record.stay` and `hops < record.hops` | Drops the note, answers `Dead`, **and logs an error**: this is a copy of the living stay, which should not exist, and the first one has to be seen |
| `Has` | `entity > record.stay` | Drops the note and logs an error. It cannot be: a stay leaves the home region only on the store's answer to its `Entering` note. It is not answered `Dead`, so that a mistake here costs a place and not a player |

So a later place is never overwritten by an earlier one, whatever order commits of
different regions arrive in.

**The answers**, both new variants of `StoreReply`, put behind the `Committed` of the
commit that carried the note and given when the group is durable, as a claim's answer
is (`lanes.rs:772-778`, `872-886`):

```rust
/// To the home region: the stay `entity` of `player`, which it named as entering, may
/// enter. `place` is where the player was last, if anywhere; `holder` is the region
/// that held the chunk of that place when the note was taken, if one did.
Enter { player: PlayerId, entity: EntityId, place: Option<Place>, holder: Option<RegionId> },
/// Of each player named, a stay below `(stay, hops)` is dead: one with a lower entity
/// id, or the stay `stay` with fewer hand-overs than `hops`.
Dead { stays: Vec<(PlayerId, EntityId, u32)> },
```

`holder` is `Table::holder` of the chunk of `place.pose.position` as the table is when
the note is taken (`table.rs:301-308`). **It is advice.** The answer goes out when the
group ends, and a claim later in the same group can take the chunk
(`lanes.rs:760-771`); a region can give a chunk back or be absorbed a moment later. The
sim asserts nothing of it: whatever it says, the ordinary rules bring the player to
whoever holds the chunk (the home region places, claims, is told "another's" and lets
go; a region that is sent a player for a chunk it believes another's sends them on,
`region.rs:465-484`). A `Dead` that answers a note carries
`(player, record.stay, record.hops)`.

**Why "below" and not "other than".** A `Dead` can reach a runner late. Read as "every
stay but this one", a `Dead` from before the latest login would remove the latest stay
if that had got there first. Read as "below", an old `Dead` never touches a later stay,
because a stay's entity never changes and its hops only rise. It rests on entity ids
never going back (section 7).

### 4. A login, step by step

| # | Who | Does | Waits for |
|---|---|---|---|
| 1 | Edge | A connection of player P has finished configuration. **If the edge has a view of P, it ends that one first**: the disconnect sentence of section 6, then `remove_player`, which sends that stay's leave as today (`fanout.rs:2609-2674`). Replaces the refusal of `fanout.rs:875-878` | Nothing |
| 2 | Edge | Makes the view (no entity, region = home) and sends `PlayerJoin { player, name, attempt }` to the home region, numbered and kept as today. `attempt` is the connection's `SessionId` (`fanout.rs:74-77`); sessions are numbered from 1 (from 0 today, `services/edge/src/lib.rs:265`) | – |
| 3 | Home region, tick *t* | For a join through an edge it knows: removes a present stay of P and an entering stay of P, **telling each loser's edge** (section 6). Gives the next entity id E, or refuses as today (`region.rs:392-398`). Holds `EnteringState { entity_id: E, name, edge, attempt }` under P. Says nothing to any edge. Note: `Entering { P, E }` | – |
| 4 | Runner | Commits tick *t* with the note | – |
| 5 | Store | Takes the note (section 3): floor raised, `Dead (P, E, 0)` for every region that runs, `Enter { P, E, place, holder }` for the home region. Syncs the group. Then answers | The disk |
| 6 | Runner of the home region | Reads `Enter` with its other replies and puts it into the coming tick's inputs (`TickInputs::entered`), as it does a claim's answer (the runner:1095-1098) | – |
| 7 | Home region, tick *u* > *t* | If it still holds E as entering for P, one of the three cases below. Otherwise the answer is passed over | – |
| 8 | Runner | Commits tick *u*; publishes when it is confirmed | The disk |
| 9 | Edge | Puts the player into the world: section 4.1 | – |

**A place below the world counts as no place.** If the feet of the kept pose are below
the world's lowest block, `y < -64` (`crates/clustine-data/src/lib.rs:187`; the region
is given the number as `RegionConfig::lowest_y`), step 7 goes by its first row whatever
`holder` says: the spawn point, looking ahead, not flying, **with the kept hotbar and
slot**. The sim takes any position down to 20 million blocks below
(`region.rs:156-157`, `1276-1287`), there is no death, no void and no command, and
until now a leave and a join was what brought a player who fell through the floor back
(`region.rs:404`). It is a function of the place and a constant, so the sim stays
deterministic. A player who flies about under the world on purpose and leaves there
comes back at the spawn point.

**Step 7.** First, whatever the case: **a present stay of P in the home region is
removed, its entity reported removed and its edge told with `Ended`**, as a join does.
It can be there: an earlier stay whose arrival landed between the join and the answer
and was taken in under the rule of today (section 6 now passes such an arrival over,
and this is the second guard). If the present stay is not below the entering one, which
cannot be while only the home region gives out ids, the entering stay is dropped
instead, with `Ended` to its edge. Then:

| `place` | `holder` | The home region | Says |
|---|---|---|---|
| `None`, or below the world | – | Places the player at the spawn point, looking ahead, not flying: today's join (`region.rs:401-427`). With the starting hotbar if there is no place, with the kept hotbar and slot if the place is below the world | `PlayerEvent::Spawned`, `EntitySpawned`; note `Has`, hops 0 |
| `Some` | `None`, or the home region itself | Places the player at the place, with its look, flying, hotbar and slot. The chunk they stand in is claimed at the end of the tick if the region knows nothing of it, and if the store says it is another's they are let go in the tick that hears it, as anyone is (`region.rs:535-542`, `1152-1161`). A region that takes an arrival into a chunk it knows nothing of is in the same condition today (`region.rs:486-491`) | The same |
| `Some` | `Some(X)`, another region | **Lets the stay go to X in this tick without placing it.** The entering stay is removed. Nothing is claimed, no entity is reported | `Durable::Departed { player, transfer, to: X }` in the outbox of the stay's edge, with `transfer.hops = 1` and `transfer.attempt = Some(attempt)`; note `Has`, hops 1 |

A placed player's state has `attempt: Some(attempt)` until their first input is applied
(section 4.3).

The home region never holds a joining player back for anything but the store's answer.
While it waits, every other message of that edge is applied as ever: the join was
applied in tick *t* and the answer is an input of a later tick, so the edge's `applied`
(`sim/state.rs:60-61`, `region.rs:369-376`) does not wait.

**What a login never waits for**: a region that is away, hung, being taken over,
merged, split or moved; the old stay's region, running or not; the old edge; a
hand-over that is under way. What the player then waits for is in "What a player
notices".

**Departure 2: the plan's rule 3 is not taken.** There the store holds its answer until
the old stay's region has committed its removal, if that region runs. I leave it out
because:

- The plan itself names the hole: a region that is hung and not yet taken over has the
  region open at the store, and a login would wait for a deadline that R0.4b has yet to
  build.
- The wait buys two things, and neither needs it. *No screen shows both*: that is the
  edge's rule 6, which has to hold without the wait anyway, since nothing orders what
  two regions say to an edge that only watches (`fanout.rs:417-419`). *The place is the
  old stay's last*: without the wait it is the place of the old stay's last commit the
  store had taken when the login's commit arrived (how stale that can be is in "What a
  player notices").
- Every wait between two regions is a place for an ordering mistake.

**Departure from the first draft: no `Durable::Entering`.** The first draft had an
outbox entry of its own for a stay let go without being placed, because a `Departed`
did not say which join it answers. The transfer now carries the attempt (section 4.3),
so a `Departed` does, and one path serves both a stay let go by case 3 and a stay
placed and let go before its edge heard of the placing.

#### 4.1 The edge puts the player into the world

**Entering**, in one turn of the fan-out task, in this order: the login packet; the
abilities packet with the flying bit if `flying`; the position **with the yaw and pitch
of the pose**; the game event; the inventory; the player list; then `move_view`, which
asks the view's region for the chunks (`fanout.rs:2074-2075`). It is `spawn_player`
(`fanout.rs:1987-2076`) with a pose and `flying` in place of a position.

The word that sets it off is ordinarily one of two:

| Word | From | The edge |
|---|---|---|
| `ToPlayer { Spawned { attempt, entity_id, pose, flying, hotbar, selected_slot } }` | The home region, which has the player | Only if the view has no entity **and** its session is `attempt`: enters the player with the view's region as it is. Otherwise passes it over. Today's test is "has no entity" alone (`fanout.rs:1994-2001`) |
| `Outbox { Durable::Departed { player, transfer, to } }` with `transfer.attempt == Some(a)` | The home region, which let the stay go | Section 4.3, path 3 |

A view that was entered and whose arrival waits for a region without a link is in the
world with no chunks. Its inputs are kept, and the edge gives it up after its patience
(`fanout.rs:840-863`), as it does anyone of that region.

**Why the join names its attempt.** Without it the edge cannot tell an answer to this
connection's join from an answer to an earlier one of the same player, because it does
not know the entity of a stay that is entering. Order of events: a join is taken, the
client goes and comes back before the edge has read the answer, and the answer to the
first join finds the second connection's view without an entity. Today the edge takes
it (`fanout.rs:1994-2002`) and then passes over the second answer as "told twice"; the
region meanwhile ended the first stay by the leave that names no entity
(`region.rs:429-438`). The view has an entity no region has, every input is ignored
(`region.rs:770-772`), and the player is put out after 20 seconds. **Inferred, not
run**; the reviewer read the same paths and could not settle it either. It needs a
reconnect within one commit's time, and this design adds a second commit to that
window.

The attempt is safe across an edge's restart only because a higher start resets the
edge at the region (`region.rs:595-604`): two starts of one edge number their sessions
alike.

#### 4.2 Presence for a stay that is entering

A hello names every player the edge has under the region, also one who is entering
(`fanout.rs:627-637`). `Presence` gains a variant:

```rust
/// The region holds a stay of the player as entering, for this edge, from the join
/// with this attempt.
Entering { attempt: u64 },
```

The runner answers it for a player **the hello names** whom the region holds as
entering for that edge, where it answers `Absent` today (the runner:1792-1812). The
edge: if its view has no entity, is under that region and has that session, it goes on
waiting; in every other case it does nothing. Without the variant the answer is
`Absent`, and a player whose join was applied and trimmed is put out with "lost track"
(`fanout.rs:1666-1683`) whenever the link to the home region is lost between their join
and their entering.

**An entering stay the hello does not name gets no answer.** The runner answers named
players and, beyond them, the stays among its players (the runner:1813-1819); entering
stays are not among those. That is safe: an edge that has no view of P under the home
region has ended it, and `remove_player` sent a leave without an entity
(`fanout.rs:2669-2673`), which names the attempt and ends the stay of that attempt
wherever and whenever it is applied (section 4.4). A view without an entity is always
under the region its join went to, or where section 4.3 put it.

**`Refused` names the attempt** (`Durable::Refused { player, attempt }`), and the edge
puts out only a view without an entity that has that session, where it puts out any
view without an entity today (`fanout.rs:1084-1094`). It matters only when the home
region's block is used up, when the current join is refused as well; it is there so
that every answer to a join is held to the join.

Through a merge of the home region this holds as it stands (review, S1): the home
region is never absorbed (`lanes.rs:1383-1385`), so an entering view is never among
those a welcome brought (`fanout.rs:1510-1533`) and is never judged when the presence
is through (`fanout.rs:1690-1704`).

#### 4.3 A view without an entity is found by every path

Case 2 of step 7 places a joining player far from home in the home region. That is
whom the next split names, and whom the region lets go when the store says the chunk is
another's. `Spawned` is not an outbox entry, so if the link to the home region is lost
between the placing and the edge's reading of it, the stay can reach the edge by three
other ways, and today each of them looks for a view **with that entity**:

| Path | Today | Evidence |
|---|---|---|
| 1. A `Present` from the region the view is under | Enters the player (unless the join is still kept) | `fanout.rs:1284-1297` |
| 1a. A `Present` from another region (a part the stay went to) | "A stay the edge does not have": ended with a leave that names it | `fanout.rs:1298-1319` |
| 2. `SplitOff` names the stay | `there` is false for a view without that entity: nothing. The home region then answers `Absent`, no join or arrival is kept, "lost track" | `fanout.rs:1596-1611`, `1666-1683` |
| 3. `Departed` | `current` is `None` for a view without that entity: `Discard`. Then `Absent` as above | `fanout.rs:1801-1808` |

**The rule.** A stay has `attempt: Option<u64>`: `Some` from its join, `None` from the
first input of it that a region applies (where `last_input` is set, `region.rs:786`).
Until then the edge has not been heard from as that entity, and may not know it: an
edge drops the inputs of a view without an entity (`fanout.rs:929-935`). The attempt
is in `EnteringState`, in `PlayerState`, in `PlayerTransfer`, in `Presence::Present`
and in `SplitOff`'s entries. **Wherever the edge meets a stay with `attempt == Some(a)`
of a player whose view has no entity and the session `a`, that stay is the view's.**

| Path | The edge, new | In the code |
|---|---|---|
| 1, 1a. `Present { entity, attempt: Some(a), pose, flying, .. }` from `R`, the view has no entity and session `a` | Whatever region the view is under: `view.region = R`; the player is entered. The test for a kept join goes: a stay with this attempt is this join's. **It is an arm of the match and falls through to what follows it**: the player is taken out of the link's `brought` (`fanout.rs:1321-1323`), as in today's cases 1 to 3. Returning early would leave a view that a merge brought to be judged absent when the presence is through (`fanout.rs:1690-1704`) and put out a moment after it entered | `take_presence`, a new first arm of the match at `fanout.rs:1261`; replaces the arm at `1284-1297` |
| 1. `Present` with another attempt or none, the view has no entity | Under `R` with a join kept for `R`: nothing, as today (`fanout.rs:1285-1291`). Otherwise the stay is one the edge does not have: a leave that names its entity, as today's last arm | `fanout.rs:1298-1319`, unchanged |
| 2. `SplitOff { region: N, players }` with an entry `(P, e, Some(a))`, the view has no entity, is under the region the entry comes from, and has session `a` | `view.region = N`, and nothing else: the view sees nothing yet, so no subscription moves and no input is kept. `N`'s hello names the player (`fanout.rs:632-637`), and `N`'s `Present` enters them by path 1 | `split_off`, beside `there` at `fanout.rs:1597-1600` |
| 3. `Departed { transfer, to }` from `R` with `transfer.attempt == Some(a)`, the view has no entity, is under `R`, and has session `a` | The four steps below, in that order, in one turn. `to` is read through `living` and may be `R` itself under the same condition as at any hand-over (`back`, `fanout.rs:1080-1082`, `1815-1823`): the region the stay was let go to has gone into `R` since | `hand_over`, in front of `current` at `fanout.rs:1801-1808` |
| 3. `Departed` for a view without an entity, with another attempt or none | `Discard`, as today | `fanout.rs:1805-1808` |
| `Spawned` | Section 4.1 | `fanout.rs:1994-2001` |

**Path 3 keeps the order of today's `hand_over`**: what the edge notes, what it asks of
`to`, the arrival, and only then anything to the client.

1. **Note**: `view.entity = Some(transfer.entity_id)`, `view.region = to`,
   `joining_since = None`, the entity's owner, the count of players online, rule 6's
   map (what `spawn_player` notes at `fanout.rs:2002-2005`).
2. **Ask `to` for the view**: the view area around the transfer's chunk is worked out,
   `want(to, ..)` for each chunk of it and the replica counted, and `flush_asking`
   (what `move_view` does at `fanout.rs:2113-2146`, without its packets).
3. **`PlayerArrive { player, transfer }` to `to`** (`fanout.rs:1862`). No input is
   kept, so none is sent again.
4. **The client's packets**: the login, the abilities, the position, the game event,
   the inventory, the player list, the centre of the view, and `send_chunks`.

So entering is cut in two here: what the edge notes and asks (steps 1 and 2) and what
the client is told (step 4), with the arrival between. It is **not** "enter, then send
the arrival" as one call after the other. A client whose queue is closed or full is
removed by the first packet that does not fit (`fanout.rs:2298-2305`), and
`remove_player` sends that stay's leave to the view's region (`fanout.rs:2669-2673`).
With the packets first, that leave would be numbered in front of the arrival: `to`
would apply the leave to nobody and then take the stay in, with nobody behind it, a
player entity standing where the player last stood until their next login. Today's
`hand_over` has arrival first and the client last (`fanout.rs:1862`, `1875`), and is
safe for that reason. The subscriptions are in front of the arrival as at any
hand-over (`fanout.rs:1843-1857`), so that one claim of `to` covers the chunk the
player arrives in and their view.

What makes each safe:

- A `Present` from another region for a stay with the view's attempt can only be a
  stay that a split or a merge moved (ADR-0014, rule 38, as changed below): the edge
  sent that stay nowhere.
- The `SplitOff` is among the welcome's entries of the split region, which come before
  its presence answers (the runner:1843-1849), so the view is under the part before the
  split region's `Absent` for it is read, and that is passed over
  (`fanout.rs:1248-1254`). If the part's link comes first, its `Present` moves the view
  by path 1a, and the `SplitOff` then finds the view with an entity under the part and
  does nothing.
- A `Departed` is likewise in front of the presence answers.
- **A stay whose attempt is `None` is never the stay of a view without an entity.** A
  region clears the attempt only on an input, and an edge sends inputs only for a view
  that has the entity.

- `move_stay` does nothing for a view without an entity (`fanout.rs:1622-1624`), and
  no region under `stands_for` says anything the edge reads (`fanout.rs:482-487`,
  `601-604`, `1361-1365`).
- A view that `SplitOff` put under a part which is absorbed again before its link
  comes is carried by today's rules (second review, S1): the pair is owed and not
  retired, because a view is under the part (`fanout.rs:1386`, `1420-1431`); the
  survivor's `Absorbed` brings the view (`fanout.rs:1456-1481`, `1510-1533`); and the
  survivor answers every stay of the edge with its attempt.

An arrival keeps the transfer's attempt; `NotMine` passes the transfer on unchanged
(`region.rs:478-482`). `Ended` carries the stay's attempt (section 6). A stay that
stands in a chunk believed another's has no input applied (`region.rs:783-785`) and so
can keep its attempt although its edge knows the entity; the edge's rules ask "has the
view no entity" first, so that is harmless.

#### 4.4 A leave without an entity names the attempt

`PlayerLeave` gains `attempt: Option<u64>`, which `remove_player` fills with the view's
session **for a view without an entity** and leaves `None` otherwise. A region takes a
leave that names no entity only for a stay of that edge, entering or present, **with
that attempt**. Today it takes it for whatever stay of that edge there is
(`region.rs:432-438`). A leave that names an entity is as today.

A stay keeps its attempt until its first input, and a view without an entity has sent
none, so the leave and its stay always meet.

Why: since path 2, a view without an entity can be under another region than the home
region. Its leave then goes there, and a merge puts what was kept for an absorbed
region **behind** what is kept for the survivor (`fanout.rs:1472-1481`). Order of
events, one edge: a first connection is placed far from home, the link is lost, the
home region is split, the view goes under the part, which has no link yet; the client
goes, and the leave is kept for the part; the player joins again; the part is absorbed;
the leave is moved behind the join, or behind the new stay's arrival; and a leave that
names nothing ends the new stay. With the attempt it ends nothing but its own.

### 5. A dead stay, wherever it is

A stay of P is **dead** when the store's record of P is of a later stay, or of the same
stay with more hand-overs. Two means find it. Neither depends on anybody noticing that
an edge or a worker has died.

**(A) Told at once.** When a group that raised a floor is durable, the store says
`Dead { (P, E, 0) }` to **every region that has an owner**, after the answers of that
group (`lanes.rs:872-889`). A hello ends the group first (`lanes.rs:1075-1077`), so an
owner is either told or opens the region after the floor is durable.

**(B) Told on naming.** A region names a stay whenever it takes it in, and every stay
it has in its first tick after a restore (section 3). The store answers a note that
names a dead stay with `Dead`.

**What a region does with `Dead`.** The runner puts the entries into the coming tick's
inputs (`TickInputs::dead`). The tick applies them after the edges' events and before
anything of players: for each `(P, stay, hops)`,

- a player P it has with `(entity_id, hops)` below `(stay, hops)` is removed, their
  entity reported removed, and their edge told (section 6);
- a stay of P it holds as entering with `entity_id < stay` is removed and its edge told.

It keeps nothing of the entry after the tick.

**For the time between a floor and the reading of its `Dead`, a dead stay and the
living one can each be in a region's state.** That is on purpose, and changes what
ADR-0014 says ("Changes to earlier records").

**Departure 3: a region keeps no floor, and leaves its outboxes alone.** The plan has a
region remember floors and take in no arrival below one, and drop what it has of a dead
stay from any outbox; it leaves "how long a region keeps a floor" open. Here:

- An arrival of a dead stay is refused at the door only where the region has a later
  stay of that player, present or entering (section 6). Otherwise it is taken in, named
  in that tick's commit and removed in the tick that reads the answer. Its entity is
  shown for those ticks to edges that have not seen the later stay, and to no other
  (rule 6).
- A `Departed` of a dead stay stays in its outbox. If its edge reads it, the stay
  arrives somewhere and goes by the line above. If its edge is gone, it goes with the
  edge (`region.rs:668-686`).
- Nothing durable grows with the number of players who ever joined, and no rule is
  needed for how a floor travels through a merge, a split or a restore.

**A stay in the middle of a hand-over** is in nobody's players. It is found when it
arrives. Its place is in the record already: the tick that let it go wrote it
(section 3), and that commit was durable before the edge heard of the departure.

### 6. Which stay stays, what each edge is told, and what it shows

**Rule 7: of two stays of a player, the one with the higher `(entity_id, hops)` stays.**
In every place that compares two:

| Where | Today | New |
|---|---|---|
| An arrival against a present stay | By entity id (`region.rs:443-447`) | By `(entity_id, hops)`. Equal in both: this very stay, nothing happens. The same entity with other hops: the lower copy goes without a word to anyone (its entity lives on in the other; no `Ended`, which would put out the real player), and an error is logged |
| An arrival against an **entering** stay | Not looked at: the test sees only `self.players` | Passed over if the region holds an entering stay of the player with a higher entity id; `Ended` to the arrival's edge |
| A join | Removes the present stay (`region.rs:390`) | Removes the present and the entering stay |
| The placing of an entering stay, and its letting go | – | Section 4, step 7 |
| A merge | By entity id (`reshape.rs:177-180`) | By `(entity_id, hops)`. Without it the rule by hops would turn one wrong copy into no player: the merge would keep the survivor's copy with fewer hops, and the store would answer its naming with `Dead` |
| `Dead` | – | Section 5 |
| The store | – | Section 3 |

**Rule 5: every removal that the stay's own edge did not ask for tells that edge.** A
new outbox entry:

```rust
/// The stay `entity` of `player`, which was this edge's, has been ended by the server:
/// a later stay of the player took its place. `attempt` is that of the stay's join, as
/// long as the stay had it.
Ended { player: PlayerId, entity: EntityId, attempt: Option<u64> },
```

| Where a region makes it | For | Today |
|---|---|---|
| A join over a stay the region has (`region.rs:385-390`) | The edge of the stay that was there | Nothing said |
| A join over a stay it holds as entering | The edge of that entering stay | – |
| An arrival over an earlier stay (`region.rs:459-464`) | The edge of the stay that was there | Nothing said |
| An arrival passed over because the region has a later stay, **present or entering**, if the arrival's edge is known (`region.rs:447-457`) | The edge the arrival came through. The arrival's entity is reported removed as today | Entity reported removed, nothing said to the edge; an entering stay is not looked at |
| An entering stay placed, or let go, over a present stay (step 7) | The edge of the present stay | – |
| A `Dead` that removes a player or an entering stay (section 5) | That stay's edge | – |

Not for: a leave; an edge that is gone or reset (`region.rs:632-691`), which has nobody
to tell; a lower copy of the same entity.

`Ended` is only ever made for a stay that is dead.

**The edge, on `Ended`.** It applies to the view of that player if the view's entity is
`entity`, or the view has no entity and its session is `attempt`. Then: the disconnect
packet with the sentence, and `remove_player`. Otherwise the entry is about a
connection the edge has ended itself, and is passed over. It is confirmed either way.

`remove_player` sends a leave for the stay to the view's region as ever
(`fanout.rs:2669-2673`). It finds nothing: a leave that names an entity ends only that
entity, and one that names none ends only a stay of this edge
(`region.rs:432-438`), which by then is another edge's or gone.

**Where `Ended` is on a link.** It is among "the tick's entries other than `Departed`"
(the runner:2036-2039) and has to have a lower number than the tick's `Departed`
entries, since an edge remembers only the highest number it has seen
(`fanout.rs:1051-1054`). It has: a tick makes `Ended` under "dead", the player changes
and "entered", and lets players go last (`region.rs:560-571`). **A stay let go by step 7 is decided in the middle of the tick
and numbered at its end, with the tick's other departures.** Numbered where it is
decided, and followed by a `Remote` entry that an input of the same edge makes later in
the tick (`region.rs:825`, `883`), it would be published after that entry (the
runner:2036-2054), the edge would pass it over as a number it has had, and the stay
would be nowhere. Its sim test, by name: *a stay let go unplaced in a tick that also
passes an action on is numbered after it*. This is a rule of the contract now
("Changes to earlier records").

**The sentence** is sent as a translatable text component and not as a string: the
disconnect packet's reason is the compound `{ translate:
"multiplayer.disconnect.duplicate_login" }`
(`Nbt::Compound`, `crates/clustine-protocol/src/nbt.rs:30-42`), which each client shows
in its own language; in English, "You logged in from another location". Today's
`refuse` sends `Nbt::String` (`fanout.rs:2870-2873`) and goes on doing so for the
edge's own sentences. **Guess**: that this is the key and the form the official server
uses. Step R1.0c compares the packet's NBT, not a string ("If a guess is wrong").

**Rule 6: an edge shows of one player only the highest stay it has seen.** The fan-out
task keeps, per player, the highest entity id it has been told of: by an introduction
(`fanout.rs:1933-1945`) or by entering one of its own players. An introduction of a
lower entity of that player is passed over. One of a higher entity first removes every
lower entity of the player from what the edge shows (`remove_entity`,
`fanout.rs:1970-1983`) and then is taken. The map is kept for the life of the edge
process; it is 20 bytes a player.

Why today's rule is not enough: a view already hides an older entity of a player when a
newer one comes into sight (`fanout.rs:2739-2752`), but the older one stays among the
edge's entities, and its next move shows it again and hides the newer
(`fanout.rs:1904-1913`, `2735-2759`). **Inferred.**

**So that no client ever shows two entities of one player**: a view shows one entity
per player (`fanout.rs:2739-2752`), never its own player's (`fanout.rs:2729-2733`), and
by rule 6 never an older one after a newer.

**The player list** is each edge's own, as today (`fanout.rs:2041-2072`, `2633-2648`),
with one change. `remove_player` takes the player out of the list of every client of
the edge (`fanout.rs:2634-2648`), and a client is given a list entry only when an
entity of that player comes into view and is not yet shown (`fanout.rs:2753-2757`).
What the winner's region and the loser's region say reaches the loser's edge in no
order: if the winner's entity was shown first, its clients would show it, then lose the
player from the list, and nothing would list them again until the entity left their
view and came back. So: **when a view is removed on `Ended`, a client that shows a
higher entity of that player keeps its list entry** (rule 6's map and the client's
`visible` say which). On one edge nothing changes: step 1 of section 4 removes the old
view before the join is sent.

### 7. Entity ids never go back

The store sees to it where it opens a region (`lanes.rs:1153-1169`), with `issued`:

1. **The home region gets a new block if its block has no id above `issued`**: if
   `file.entity_ids.end <= issued + 1`. (With `end <= issued` a block whose last id is
   `issued` would count as good, the runner would set the next id to `end`, and every
   join would be refused for the life of the world, `region.rs:392-398`.) The new block
   is the next one (`lanes.rs:1162-1166`), which is above every block a region file
   names (`lanes.rs:256-262`). The region file is written with it before anyone is
   told, as it is today for a new epoch (`lanes.rs:1172-1184`).
2. **What the opener is told gains `issued`**: `StoreWelcome::Accepted` and `Restored`
   get `issued: EntityId` (0 if no stay was ever issued).
3. **The runner, when it has made the state a region is restored with**, deltas
   applied (the runner:3116-3165): it takes the store's block; it **keeps the state's
   next id if that lies inside the block**, and takes the block's first otherwise;
   then, if `issued` is inside the block, the next id is at least `issued + 1`.

A region's block is in its whole state and in no delta (`sim/state.rs:138-149`), so
after the store has given the home region a new block the checkpoint on disk names the
old one until the next checkpoint, while the deltas since have next ids inside the new
one. Step 3 as written gives the same next id at every restore in between. A merge's
and a split's whole state carry the new block, as they are made from the region in
memory (`reshape.rs:148-150`, `298`), and a checkpoint ends the mismatch.

This covers a state dropped for another build, a home region that changed with the
division, and a group that failed (the id is given again, but its note was taken back
with it). As a side effect a home region whose block is used up gets the next one when
it is next opened; nothing here opens it for that.

**What it costs**: a world whose division is changed back and forth uses one block each
time the home region changes to one with a lower block. There are 2,047 blocks
(`position.rs:133-136`).

Stays and records need no care when a world is made over for another division
(`lanes.rs:451-477`, `1610-1680`), but for step 5 of section 8.

### 8. Where it is on disk, and how recovery reads it

**In the log.** A commit with notes is a record of a new kind, 8, "Commit with stays":
kind 2's fields, then the notes. `LogRecord::Commit` gains `stays: Vec<LoggedStay>`; it
is written as kind 2 when there are none (so today's bytes stay today's) and as kind 8
otherwise, and kind 2 is read as a commit without notes. One record, so that a commit
and its notes are on disk together or not at all.

`stays` is a count (u32) followed, per note, by:

| Field | Type |
|---|---|
| Kind | u8: 1 `Entering`, 2 `Has` |
| Player | u128 |
| Entity | i32 |
| *`Has` only:* hops | u32 |
| … position | f64 x, f64 y, f64 z |
| … yaw, pitch | f32, f32 |
| … flags | u8: bit 0 on-ground, bit 1 flying |
| … selected slot | u8 |
| … hotbar | nine times: u8 0 for an empty slot, or u8 1, i32 item, i32 count |

All big-endian, as the log is. `LoggedStay` is `clustine-format`'s own type of plain
numbers; the store turns a `StayNote` into it. The layout is the store's and does not
change with `STATE_FORMAT`.

**In a file.** `regions/players`, to the records what `regions/table` is to the list of
regions:

| Field | Type |
|---|---|
| Format version | u8 |
| Kind | u8, 4 |
| `from`: the first log segment whose notes are not in this file | u64 |
| `issued` | i32 |
| Records, ascending by player: count, then per record | u32 |
| … player, stay, hops | u128, i32, u32 |
| … has a place | u8 0 or 1 |
| … the place, as in a `Has` note from "position" on | |
| CRC-32 of everything before | u32 |

Written like the table: under a temporary name (`players.tmp`), synced, renamed, the
directory synced (`lanes.rs:591-597`). The scan of the directory at a start takes a
name without a dot for no region's file and passes it over (`lanes.rs:238-240`), and
removes a `.tmp` that was left (`lanes.rs:234-236`); `align` removes files by lane and
not by pattern (`lanes.rs:1686-1707`). So the file needs no exception there.

**When it is written.** Nearly every segment has notes, so the file is what lets a
segment go. Where the store has put a state file in place, closed the segment and looks
for segments to remove (`lanes.rs:863-870`), it also looks whether the first segment is
kept only for the players' notes, and if so, and no segment is being appended to,
writes the file with `from` = `log.next` and then collects, as `trim_for_the_table`
does for the table (`lanes.rs:958-987`). **It looks after `trim_for_the_table`, not
before**: a first segment kept for the table and for the players alike is freed of the
table there (`lanes.rs:978-982`), and a look taken before would find it "not only for
the players", do nothing, and not come again until the next state file; at a clean
stop there is none, and segments would stay. `collect` keeps every segment from the file's
`from` on (`lanes.rs:941-949`, with a third condition). If the file cannot be written
the old one counts, with every segment from its `from` (but see step 5).

**Rule P: no segment is ever numbered below the players' file's `from`.** The file is
written when the next segment does not exist yet. After a clean stop no segment may be
left at all: a runner that is stopped checkpoints and flushes (the runner:2303-2304),
the store puts the state in place, closes the segment and collects. A store that
starts then numbers its next segment from what is on disk: from 1 (`lanes.rs:282`),
from the last segment + 1 (`lanes.rs:416-417`), or from the *table* file's `from`
(`lanes.rs:419-423`, which is there for the same hazard, met once before). The table
file is written only when a record changed the table (`lanes.rs:958-961`), so its
`from` can be far below the players'. Without the rule a session's commits would go to
segments below the players' `from`, and the start after it would pass every note of
that session over: stop, start, play, stop, start, and everybody is where they were two
sessions ago, with the floors of that session forgotten. So:

- `Lanes::load` raises `log.next` to the players' file's `from`, beside the line for
  the table (`lanes.rs:421-423`).
- `Log::append` holds to it where it begins a segment (`lanes.rs:1811-1821`): a number
  below the players' `from` is an invariant broken, stated with `expect`. A note that is
  skipped in silence is how this would go unseen; a start cannot tell a segment that is
  wrongly below `from` from one that is rightly there and still kept for a lane, so the
  guard is where segments are numbered and not where they are read.
- **Its test** (store, by name): *a store that is stopped with no segment left and
  started twice has every note of the session between*: commits with notes, a
  checkpoint and a flush so that every segment goes, **the assertion that no segment
  is left**, a start, more commits with notes,
  a kill, a start; the records are those of the last notes, and `issued` and the floors
  are the session's. It fails without the line in `load`.

**Recovery** (`Lanes::load`, `lanes.rs:219-486`):

1. Read `regions/players` if it is there: the records, `issued`, `from`. A world
   without one has no records and `from` 0. Raise `log.next` (rule P).
2. Read the log as today. Of every commit in a segment at or above `from`, remember its
   notes with the region, the tick and its place in the order of the log.
3. When an `Opened` record is read, forget the remembered notes of that region with a
   tick above its `restored`: those commits are not part of the region's history
   (`lanes.rs:320-326`), and neither are their notes.
4. When the log is read, apply what is left in the order of the log, by the table of
   section 3, without answering anybody.
5. **If the world is to be made over for another division** (`lanes.rs:451-469`): the
   players' file is written, with `from` = `log.next`, **before** `make_over` is
   called, **and if it cannot be written the start fails with that error**. Here the
   old file must not count: `make_over` appends an `Opened` with `restored` 0 for every
   region that had anything (`lanes.rs:1646-1663`), and by step 3 the start after it
   would forget every note since the old file. No segment is being appended to while a
   store starts, so the file stands for the whole log.

A note counts exactly if its commit counts: the commits a region is restored with are
those with a tick at or below `restored` (`lanes.rs:320-326`, `1218-1238`). In a
running store an `Opened` passes over nothing (`open` ends the group and settles the
log first, `lanes.rs:1077-1085`); the cases are `make_over` and a cut that a crash
undid. No owner is told anything by recovery: all handles are lost with the store,
every region is opened again, and its first tick names its stays (means B).

**A group that fails** (`lanes.rs:898-926`). Its commits are cut off the log, so what
their notes did is taken back. The group keeps, for each player it touched, the record
as it was before the group first touched it, or that there was none; and `issued` as it
was. `fail_log` puts each record back, **removes** a record the group made (it is not
left with stay 0), and puts `issued` back. Nothing of the group was answered, and
every region loses its owner. `fail_log` is reached from eight places
(`lanes.rs:673`, `755`, `832`, `1019`, `1240`, `1565`, `1656`, `1661`); it takes the
group out as it does today (`lanes.rs:902`), so the paths that come after `end_group`
with an empty group (`1240`, `1565`, and the two of `make_over`) undo nothing, and
nothing is undone twice.

**What an earlier build makes of it**: it cannot read kind 8 and does not start
(`log.rs:230-262`). The plan says so to the owner (its section 8).

**What the file costs** is in "Risks", and is measured in R1.1.

### 9. Merges, splits, moves, restores

| | What holds, and why |
|---|---|
| **A region is opened** (first, after a move, after a takeover) | Its first tick names every stay; a dead one is removed in the tick that reads the answer. Until then it is in the state, and an edge that resumes is told it is present |
| **The home region is opened with a stay entering** | Its first tick names it; the store answers `Enter` again (`entity == record.stay`) or `Dead` |
| **A merge** | The merged state has the players of both and, of a player both have, the stay with the higher `(entity_id, hops)`; and the home region's entering stays (the home region is never absorbed, `lanes.rs:1383-1385`; an absorbed region's entering stays, of which there can be none, are dropped). The survivor's runner begins anew (the runner:1675-1697); its first tick names every stay |
| **A split** | The stays that go get **one hand-over more**, in the split's own record (for R2's order of introductions, plan 4.2; for the store it changes nothing). Their entries in `SplitOff` carry their attempts. Entering stays stay. Both regions name every stay in their first tick. The part ticks from memory only once the store has answered its hello (`bin/clustine/src/cluster/worker.rs:95-103`) |
| **A `Dead` or an `Enter` that waits for a tick when a merge or a split is taken** | Dropped with the other inputs (the runner:1652-1662), also one read in the same pass as the merge's answer. The first tick after it names every stay, and the store says both again |
| **A merge or a split that is declined** | The inputs wait and the next tick takes them (the runner:1489-1502) |
| **A region without players is merged into the home region during a login** | A home region with only an entering stay counts as without players. The login waits for the merge and is answered again after it, by the line above |
| **The store restarts** | Section 8 |
| **A state of another build** | The region begins without players (the runner:3134-3140). The records and `issued` are the store's, so everybody joins again in place with a higher id |

An edge reset (`Started` with a higher start, `Gone`) removes that edge's entering
stays with its players (`region.rs:632-656`), and a merge drops them where it drops
that edge's players (`reshape.rs:161-166`).

#### 9.1 A player placed where the home region holds nothing, against ADR-0015 and ADR-0017

Case 2 of step 7 puts a player of the home region into a chunk it does not hold: the
claim is under way, or answered "another's" and not yet ticked. ADR-0015, section 8,
names three things `hand_over` rests on that no rule of the contract promises, and
ADR-0017, section 3.6.5, went through them for the split. Case 2 by name against each:

1. **"A part holds the chunk each of its players stands in."** It still does. Those go
   who stand in a chunk named that the region **holds** (`reshape.rs:266-272`), by its
   ticks or by a grant that waits (`reshape.rs:126-128`). A player placed by case 2
   whose chunk is not held is no seed and stays; their chunk is among `staying`
   (`reshape.rs:279`). When the split is worked out every claim has been answered
   (ADR-0017, section 3.6.3, first row): the chunk is then granted and waits, and the
   player goes with it as ADR-0017's section 3.6.1 has it for anybody; or it was
   answered "another's" and the answer is dropped with the split (third row), the
   player stays, the region claims the chunk again in its next tick because they stand
   in it (`region.rs:1152-1161`), and lets them go when it hears. **A region that took
   an arrival into a chunk it knew nothing of is in exactly this condition today**
   (`region.rs:486-491`), and section 3.6.3's table has every row of it. Case 2 adds no
   row.
2. **"A stay does not leave a region without an input of this edge."** What the edge
   needs of it is that a `Departed` from `R` finds the stay under `R`. Case 2 can let a
   player go with no input of theirs: placed, the claim answered "another's", let go.
   The edge has the view under the home region by its own join. With `Spawned` read,
   the view has the entity and the `Departed` is today's (the runner publishes who
   entered before who was let go, the runner:2029-2054). With `Spawned` lost, it is
   path 3 of section 4.3. Case 3 of step 7 is a `Departed` for a view the edge has
   under the home region. So the premise holds in the form the edge needs, **by section
   4.3 and not without it**.
3. **"A merge announces itself in the survivor's outbox before anything the survivor
   says of a stay that came with it."** Nothing here touches the order of a merge's
   entries; `Ended` entries a survivor makes are made by ticks after the merge's.

ADR-0017's section 3.6.2 (what an edge asks for before it has heard of the split) meets
case 2 where the player is split off at once: the edge has their view under the home
region and names all they see in its hello as a viewer's. That is the case the section
was written for, and nothing in it depends on how the player came to stand there.

### 10. Every order of events

P's old stay is **D** (through edge A). The new login gives **N** (through edge B, which
may be A). H is the home region; X is where D is or is heading.

| # | Order of events | Store | Regions | Edge A (loser) | Edge B (winner) | Any edge that watches |
|---|---|---|---|---|---|---|
| 1 | D is in X, which runs. B is another edge | Floor to N; `Dead` to all; `Enter` to H at once | X removes D in its next tick and puts `Ended` into A's outbox. H places N or lets it go to the holder. If N arrives in X before the `Dead`, the arrival takes D's place (`region.rs:462-464`) and tells A the same | Reads `Ended`: sentence, connection closed | Enters P at the place | D removed by X's word; N introduced; by rule 6 never D after N. D can be seen to walk on for a tick or two and N to appear a step behind it |
| 2 | The same, B = A | The same | The same; A's leave for D may come first and X removes D by it | Ended its own view in step 1. `Ended` finds no such view and is passed over | The same edge | The same |
| 3 | D is in H | The same | The join itself removes D and tells A (`region.rs:390`). N enters | As 1 | As 1 | As 1 |
| 4 | D is in X; X has no worker, or is being taken over | The same. Nobody waits. `Enter` names X if the table says X holds the chunk | H lets N go to X. X, when it runs: the arrival of N, kept by B, takes D's place among the player changes, before any input of D that A kept is applied (`region.rs:462-464`, `529-531`); or, where N went elsewhere, X's first tick names D and it is removed on the answer | Its players of X stand still as all of X's do. Told when X runs | P is in the world without chunks until X runs, or is given up after the edge's patience with "The server fell too far behind." (`fanout.rs:859`). A new try raises the floor again and leaves one more arrival and leave kept for X, which cancel in order when X runs | As 1, when X runs |
| 5 | D is in X, which hangs | The same; the `Dead` waits in X's connection | As 4 when X wakes or is taken over | As 4 | As 4 | As 4 |
| 6 | D stood in a part that was split off a moment ago; B has no link to it | The same | The part names its stays in its first tick and removes D on the answer; or was told directly if it had been opened | Told by the part | Keeps the arrival for the part until the routing table brings a link (`fanout.rs:2688-2698`) | As 1 |
| 7 | X is absorbed, or split, between the floor being raised and X's tick reading the `Dead` | Says it again when the survivor, or both parts, name their stays | The `Dead` that waited is dropped with the merge or the split; the first tick after names D; removed | Told by whoever removes D | As 1 | As 1 |
| 8 | D is in the middle of a hand-over: let go by X, the `Departed` unread or the `PlayerArrive` kept | The same. The place is the one X's letting go wrote | D is in no region. When its arrival lands: passed over if N is there, present or entering (A told), else taken in, named, removed on the answer (A told) | Told then. If A is dead, nobody passes D on and it never lands | As 1 | D stands where it was let go until N is introduced there (it is: that is N's place), then rule 6 |
| 8a | **D's arrival lands in H while N is entering there** (two edges) | `Enter` for N was sent at N's note | **The arrival is passed over, because H holds a later entering stay; A is told.** Were it taken in (as today's test, which sees only players, would), step 7 removes it with `Ended` before it places N | Reads `Ended` | Enters P | D's entity reported removed |
| 9 | A kept `PlayerArrive` of D lands after N entered | The note of D is dropped; `Dead` answered. The record is not touched | As 8. The arrival drops only D's own waiting inputs (section 12) | As 8 | Nothing | D shown for a tick or two only where N was never seen |
| 10 | A is alive and its client does things in the ticks before D is removed | Notes of D after the floor was raised are dropped | What D did is applied, as before any kick, and published | – | – | – |
| 11 | A was frozen and wakes | – | Its link is new: the welcome's entries have `Ended` before the presence answers; inputs of D find no such stay (`region.rs:770-772`); a kept arrival goes by 9; a kept action on blocks is applied (section 11) | Reads `Ended` first. If it was away for 600 ticks the region has forgotten it and it puts everybody of that region out with "lost track" (`fanout.rs:800-815`) | – | – |
| 12 | A is dead for good | – | D is removed by `Dead` all the same, or goes with A after 600 ticks if it was on its way | – | – | D removed |
| 13 | Two logins within a tick, through B and then C | One `Entering` note, of the later | H takes the joins in order: the second removes the entering stay of the first and tells B | – | B: `Ended` with its attempt, sentence. C enters | – |
| 14 | Two logins a few ticks apart, the first not yet answered | Two floors, two `Enter` | The second join removes the first entering stay and tells B. The first `Enter` finds no such entering stay and is passed over | – | As 13 | – |
| 15 | The same, the first already entered | The second floor makes the first stay dead | As 1 | – | B is the loser of row 1 | As 1 |
| 16 | P leaves while entering | – | The leave names no entity and ends the entering stay of that edge. An `Enter`, a `Spawned` or a `Departed` that comes later finds no view with that attempt | – | Sends `Discard` for a `Departed`, as today | – |
| 17 | P leaves in the middle of a hand-over | The record has the place of the letting go. `Discard` and an edge that is gone write nothing | As today (`fanout.rs:1801-1808`) | – | – | As today |
| 18 | The home region is restored or taken over while N is entering | Answers the naming of N with `Enter` again | H's first tick names N | – | Resumes with H: `Presence::Entering` if the join was applied; else the kept join is sent again | – |
| 19 | The store restarts between any two of these | Records and floors are on disk with the commits that made them | Every handle is lost; every region is opened again and names its stays | Resumes | Resumes | – |
| 20 | A world is started with another division | Records kept. The new home region gets a block above `issued` if its own has no id above it | Begin anew, without stays | Whatever an edge that was not stopped sends of an old stay is below the floor, or is the floor's own stay coming back | – | – |
| 21 | **N is placed far from home (case 2), the link to H is lost before B reads `Spawned`, and H is split with N among those who go** | – | The part has N with its attempt | – | `SplitOff` puts the view under the part; the part's `Present` enters P. Or the part's `Present` first, and the `SplitOff` finds nothing to do (section 4.3) | – |
| 22 | **The same, and H lets N go because the store says the chunk is another's** | – | `Departed` with the attempt in B's outbox | – | Path 3 of section 4.3: enters P and hands them over | – |

**What "never twice" means here**, said plainly. No region ever has two stays of one
player, present and entering counted together, after a tick (`region.rs:390`,
`443-464`; `reshape.rs:171-181`; section 6). Two regions can each have one for as long
as it takes the region with the dead one to read its `Dead`: a tick or two for a region
that runs, and until it runs for one that does not. No screen shows both, by rule 6.
The dead stay can leave no trace in the record. Its edge is told when it is removed.

### 11. Actions on blocks name their stay

`RemoteAction` and `Durable::RemoteDone` name the player and the client's sequence
number (`api.rs:197-204`, `279`). A client numbers its actions afresh with every
connection (**guess**; the edge's own comment has it so, `fanout.rs:1751-1752`). So a
"done" of an earlier connection can end the wait for a later connection's action with
the same number (`fanout.rs:1747-1756`).

Both gain `entity: EntityId`. The sim fills the action's with the entity of the player
who acts, in the two places that make one (`region.rs:819-823`, `878-882`), and a
"done" with the entity of the action it answers (`region.rs:899-902`). The edge passes
an action on only for a view that has that entity, and takes a "done" only for one
(`fanout.rs:1184-1188`, `1747-1756`). The sim's half is built a step before the edge's
("Building it"): an edge that looked for an entity the sim does not fill would pass
nothing on, and every break and place across a boundary would hang.

**Departure 4**: a region does not drop an action of a stay it knows dead, because it
knows no floors. A dead stay's action that an old edge still sends is applied, and the
plan's ledger already calls such a block "in doubt until the old edge is gone" (its
section 7).

### 12. An arrival drops only its own stay's inputs

`TickInputs::change` drops, for an arrival, every input of that player that waits for
the tick (`api.rs:446-455`), whatever stay it names. ADR-0014 notes that with several
edges an arrival of an earlier stay so eats the later stay's inputs of that tick
(`docs/adr/0014-merging-and-splitting.md`, lines 2332-2340). Row 9 of section 10 makes
it reachable in R1. The rule becomes: an arrival drops the waiting inputs that name its
own entity. A join drops all of the player's, as today. `TickInputs` is no `Region` and
sees no setting (`api.rs:446-455`); the rule needs none, and holds from the step that
builds it.

### 13. Flying

| Where | Change |
|---|---|
| The codec (`crates/clustine-protocol`) | The serverbound packet `minecraft:player_abilities` (id 40, `generated/packet_ids.rs:483`): one byte of flags, of which `0x02` is flying (**guess**; the clientbound flags are in `docs/protocol-26.3.md:87`). `ServerboundPlay` gets it (`packets/play.rs:1397-1411`) |
| The edge's connection (`play.rs`) | Reads it and passes on `PlayerInput::SetFlying { flying }`. It is not held back for an unconfirmed teleport, which is about positions |
| The sim | `Player`, `PlayerState`, `PlayerTransfer` and `Place` get `flying`. The input sets it and counts as a change of the player |
| The edge, entering | The abilities packet has `0x02` if flying (`fanout.rs:52`, `2013-2017`) |
| Presence | `Present` gets `flying` |

Nobody else is shown that a player flies. The bots have no gravity, so only the
comparison with the official server and the owner can tell whether a client placed in
the air with the bit stays there (**guess**: it does).

### 14. Every change to messages and types

**`crates/clustine-sim`**

| Type | Change |
|---|---|
| `Place` (new, `api.rs`) | `pose`, `flying`, `hotbar`, `selected_slot` |
| `StayNote` (new, `api.rs`) | Section 3 |
| `PlayerJoin` | `+ attempt: u64` |
| `PlayerChange::Leave` | `+ attempt: Option<u64>`; section 4.4 |
| `PlayerTransfer` | `+ hops: u32`, `+ flying: bool`, `+ attempt: Option<u64>` |
| `PlayerInput` | `+ SetFlying { flying: bool }` |
| `PlayerEvent::Spawned` | `position` becomes `pose: Pose`; `+ attempt: u64`, `+ flying: bool` |
| `Durable` | `+ Ended { player, entity, attempt: Option<u64> }`; `Refused` `+ attempt: u64`; `RemoteDone` `+ entity`; `SplitOff.players` becomes `Vec<(PlayerId, EntityId, Option<u64>)>` |
| `RemoteAction` | `+ entity: EntityId` |
| `TickInputs` | `+ entered: Vec<Entered>` (`Entered { player, entity, place: Option<Place>, holder: Option<RegionId> }`), `+ dead: Vec<(PlayerId, EntityId, u32)>`. Order of a tick: edges, applied, **dead**, player changes, **entered**, remote actions, inputs |
| `TickInputs::change` | Section 12 |
| `TickOutput` | `+ stays: Vec<StayNote>` |
| `PlayerState` | `+ hops: u32`, `+ flying: bool`, `+ attempt: Option<u64>` |
| `EnteringState` (new, `state.rs`) | `entity_id`, `name`, `edge`, `attempt: u64` |
| `RegionState` | `+ entering: BTreeMap<PlayerId, EnteringState>` |
| `StateDelta` | `+ entering: Vec<(PlayerId, Option<EnteringState>)>`; `changes_only_the_tick` looks at it |
| `RegionConfig` | `+ place_by_store: bool`, for the steps of building only; `+ lowest_y: i32`, the height of the world's lowest block (section 4) |
| `Region` | The flag of the first tick after `restore`; `entering_state(player)` for the runner |
| `Region::tick`, arrival | Section 6: `(entity_id, hops)`; an entering stay is looked at |
| `Region::absorb`, `split` | Sections 6 and 9 |

**`crates/clustine-rpc`** (`messages.rs`)

| Type | Change |
|---|---|
| `StoreRequest::Commit` | `+ stays: Vec<StayNote>` |
| `StoreReply` | `+ Enter { .. }`, `+ Dead { .. }` |
| `StoreWelcome::Accepted`, `Restored` | `+ issued: EntityId` |
| `EdgeToWorker::PlayerLeave` | `+ attempt: Option<u64>` |
| `Presence` | `+ Entering { attempt }`; `Present` `+ flying`, `+ attempt: Option<u64>` |
| `WorkerToEdge::RemoteDone` | `+ entity` |
| The wire number (R0.3) | Raised |

**`crates/clustine-format`**: `LogRecord::Commit` `+ stays: Vec<LoggedStay>`, kind 8;
`PlayersFile` (kind 4) beside `TableFile`. `docs/world-format.md` gets both.

**`crates/clustine-protocol`**: the serverbound abilities packet.

**`services/worker`**: `STATE_FORMAT` 3 → 4, and → 5 when the setting goes
("Building it"), each with the bytes of its test written down anew (the runner:459,
6390-6394); the notes into the commit and the commit's condition; `Enter` and `Dead`
into the inputs; `Presence::Entering` and the attempt in `Present`; `issued` and the
block at a restore.

**`services/worldstore`**: sections 3, 5, 7 and 8.

**`services/edge`**: sections 4, 4.1 to 4.4, 6 and 11; the reason of a second login as a text component; the abilities packet; sessions
from 1.

No change: the coordinator, the routing table, `clustine-region`.

### 15. Where this departs from the plan's section 4.1

| # | The plan | Here | Why |
|---|---|---|---|
| 1 | The record says where the stay is | It does not | Nothing reads it (section 2) |
| 2 | Rule 3: the store answers when the old stay's region, if it runs, has removed the stay | The store answers at once, always | Section 4 |
| 3 | Rule 4: a region remembers floors, refuses arrivals below one, drops dead stays from outboxes | It remembers nothing and leaves outboxes alone | Section 5 |
| 4 | Rule 8: a region drops an action of a stay it knows dead | Only the edge's half is built | Section 11 |
| 5 | Rule 9: the store says the highest stay and the region goes on above it | Also a new block for a home region whose block has no id above it, and the runner takes the store's block | Section 7 |
| 6 | – | The join names its attempt and the stay carries it; `Presence::Entering`; an arrival drops only its own inputs (from R2.1) | Sections 4.1 to 4.3, 12 |

## Changes to earlier records

Each gets a note under its own "Changes" heading that points here, with this text.

**ADR-0012, section 2.5 ("What a tick's outbox entries are, and their order")**. Its
first paragraph ("`TickOutput::durable` has, in this order: the entries of step 4,
`Refused` and `NotMine` for an arrival, in the order of `player_changes`; the answers
of step 5, one for one; the `Remote` entries of step 6, in the order of `inputs`; the
`Departed` entries of step 8. Departures are numbered last…") becomes:

> `TickOutput::durable` has, in this order: the `Ended` entries of the store's `Dead`;
> the entries of step 4, `Refused`, `Ended` and `NotMine` for an arrival, in the order
> of `player_changes`; the `Ended` entries of the store's `Enter` answers, in their
> order; the answers of step 5, one for one; the `Remote` entries of step 6, in the
> order of `inputs`; the `Departed` entries, **those of stays the home region let go
> without placing them (ADR-0020, section 4, step 7) and those of step 8 alike**.
> Departures are numbered last, as today, so that numbers ascend on a link in the
> order the runner publishes them. A stay let go unplaced is decided where the
> `Enter` answers are applied and numbered here.

and its second paragraph gains: "`Ended` goes to the outbox of the edge of the stay it
ends, or, for an arrival that is passed over, of the edge the arrival came through."

**ADR-0012, rule 18**, last sentence ("If the edge no longer has the player with that
entity, it sends `to` a `Discard`, as today") becomes:

> If the edge has the player with no entity yet, under this region, on the connection
> whose attempt the transfer carries, the stay is that view's: the edge notes the
> entity, asks `to` for the view, passes the player on with `PlayerArrive`, and only
> then puts the client into the world (ADR-0020, section 4.3, path 3). In every other
> case in which it does not have the player with that entity, it sends `to` a
> `Discard`, as today.

**ADR-0012, rule 24** ("`ToPlayer` with `Spawned` and `Acknowledged`, `Refused` and
`Presence`: as today") becomes:

> `ToPlayer` with `Acknowledged`: as today. `Spawned` and `Refused` name the attempt of
> the join they answer, and the edge takes each only for a view without an entity on
> that connection (ADR-0020, sections 4.1 and 4.2). `Presence`: ADR-0015, section 2,
> as changed by ADR-0020.

**ADR-0012, section 5.2 ("The order on a link")**, items 3 to 6 become:

> 3. `ToPlayer` with `Spawned`, for those who entered the world. It names the attempt of
>    the join it answers (ADR-0020, section 4.1);
> 4. `Outbox` for the tick's entries other than `Departed`, in ascending order of their
>    numbers. **`Ended` is among these: a region numbers every `Ended` of a tick below
>    every `Departed` of that tick**, as an edge remembers only the highest number it
>    has seen;
> 5. `ToPlayer` with `Acknowledged`;
> 6. `Outbox` for the tick's `Departed` entries, which have the tick's highest numbers.
>    A stay that the home region lets go without having placed it is a `Departed` like
>    any other (ADR-0020, section 4).

and item 1 gains: "…one `Presence` for each player of the hello, which for a player the
region holds as entering is `Entering`".

**ADR-0012, section 5.2, first paragraph**: "a tick that changed nothing of the region's
state and no block has no commit" becomes "a tick that changed nothing of the region's
state and no block **and has no stay notes** has no commit".

**ADR-0014, rule 37** gains:

> A player the hello named whom the region holds as entering for the edge is answered
> `Entering { attempt }`. An entering stay the hello did not name is not answered
> (ADR-0020, section 4.2). A `Present` carries the stay's attempt for as long as the
> stay has one.

**ADR-0014, rule 38**, the first case's second sentence ("For a player who is entering
the world and has not been told an entity: the answer is their stay if their join is at
or below the welcome's `applied`, and an earlier stay, which the join will end, if it is
above") becomes:

> For a player who is entering the world and has not been told an entity: the answer
> is their stay **if it carries the attempt of their connection, from whichever region
> it comes**; the view goes under that region. A `Present` with another attempt or
> none is an earlier stay.

and its reasoning ("A stay is in at most one living region's state (section 2.5)")
gains:

> **Since ADR-0020 this holds for a stay that lives.** A stay that a later login made
> dead can be in one region's state while the living stay is in another's, until the
> region reads the store's `Dead`. Moving a stay on a region's word is still right: the
> edge has at most one view of a player, and a `Present` for an entity that is not the
> view's moves nothing. The third case (a leave that names the region's entity) then
> also ends a dead stay of the same edge, which is what is wanted.

**ADR-0014, section 2.1 and `Region::absorb`** ("Of a player both have, the later stay
stays, which is the one with the higher entity id") becomes "…the one with the higher
entity id, and of the same entity the one with more hand-overs".

**ADR-0014, the note of lines 2332-2340** ("An arrival still drops every input of its
player that waits for the tick … left for the milestone that has several edges") gets:
"Done by ADR-0020, section 12."

**ADR-0015, section 2.1** gains a fifth answer and a changed case 2:

> 2. **The edge has `P` with no entity yet**, under `R` or under another region, and
>    the answer's attempt is that of the view's connection: `view.region = R` and they
>    enter the world as `e`. With another attempt or none: if the view is under `R` and
>    a `PlayerJoin` of `P` is kept for `R`, nothing is done; otherwise case 4.
>
> `Presence::Entering { attempt }` for `P` from `R`: if the edge has `P` under `R` with
> no entity and that connection, it goes on waiting. Otherwise nothing is done. It is
> counted like any answer.

**ADR-0015, section 2.1, last line** ("In cases 1 to 3 `P` is taken out of the link's
`brought`") stays, and gains: "also in case 2 where the view was under another region:
the changed case is an arm like the others and does not return before this."

**ADR-0015, section 7**, second point ("`remove_player` sends `PlayerLeave { player,
entity: view.entity }` to `view.region`") becomes:

> `remove_player` sends `PlayerLeave { player, entity: view.entity, attempt }` to
> `view.region`, with the attempt of the view's connection if the view has no entity
> and none otherwise. A region takes a leave without an entity only for a stay of that
> edge with that attempt (ADR-0020, section 4.4).

**ADR-0015, section 6 (`SplitOff`)** gains:

> An entry is `(P, e, attempt)`. If the edge has `P` under `A` with no entity and the
> connection `attempt` names: `view.region = N` and nothing else; `N`'s presence
> answer enters them (section 2.1, case 2).

**ADR-0015, section 8, first point** gains:

> ADR-0020 adds one way for a `Departed` to be true of a view the edge could not match
> by entity: a stay placed, or let go without being placed, whose view has not been
> told its entity. It is matched by the attempt the transfer carries (ADR-0020,
> section 4.3). The three premises were gone through for a join placed away from home
> in ADR-0020, section 9.1.

## Building it

Each step is one verified commit and leaves everything working: every existing test
passes, nothing is logged as an error in ordinary play, and a real client plays as
before or better.

**What is behind the setting `place_by_store`, and what is not.** Behind it is the
store's part only: a join held as entering, notes, `Enter`, `Dead`, `Ended`, the first
tick's naming, the place. **Not** behind it, and real from the step that has the
field: a stay's attempt (set by the join, cleared by the first input, carried by the
transfer, `SplitOff`, `Present`, `Spawned`, `Refused` and the leave); the entity in
`RemoteAction` and `RemoteDone`; hops; flying; the comparison by `(entity_id, hops)`;
section 12.

| # | Scope | `attempt` and `entity` after it | Leaves working because | Verified by | Who |
|---|---|---|---|---|---|
| R1.0b | The types of section 14 in `clustine-sim`, `clustine-rpc`, `clustine-format`; the wire number raised, `STATE_FORMAT` 4; pushed. Filled with what today's behaviour means: `hops` 0, `flying` false, no notes, `issued` 0; `Ended`, `Enter`, `Dead` and `Presence::Entering` are made by nobody and passed over by everybody. **Filled for real, in this step**: sessions are numbered from 1; the edge puts the session into `PlayerJoin::attempt` and, for a view without an entity, into `PlayerLeave::attempt`; the sim's join puts `Some(join.attempt)` into the player it places and copies it into `Spawned`, clears it at the first input applied, and carries it in the transfer and in `SplitOff`; a refusal names the join's attempt; the runner puts the stay's attempt into `Present`; **the sim fills `RemoteAction::entity` with the acting player's entity and `RemoteDone::entity` from the action**. Nobody reads any of it yet: the region still takes a leave without an entity for whatever stay of the edge there is | Real everywhere they are written; read by nobody | Behaviour is unchanged | All existing tests; the bytes of a state written down anew | main session |
| R1.0c | The codec: the serverbound abilities packet; a disconnect reason as a text component. The bots send the packet, read the clientbound one, and can log in twice as one name. **The comparisons with the official server**: the packet's id and flag; the NBT of the reason a first connection is given at a second login, and that it is given one; what the server sends a client that enters with a look and flying. What follows from each result is under "If a guess is wrong" | The same | Nothing of the server reads the packet yet | `--ignored official_server`, **run by the main session**: subagents do not start the official server (`CLAUDE.md`) | **main session**. A subagent may write the codec, the bots' part and the `#[ignore]` tests in `crates/clustine-protocol` and `tools/botswarm`; the main session runs them against the official server, acts on what they show and makes the commit |
| R1.1 | Store: notes into the log, the records, `Enter` and `Dead`, the file, rule P, recovery, the undo of a failed group, `issued` and the block | The same | No region sends a note, so no record is made and no kind-8 record is written | "Tests"; the cost of the file with 10,000 records | subagent, `services/worldstore`, `clustine-format` |
| R1.2 | Sim. **Not behind the setting**: the leave without an entity by attempt (section 4.4); `(entity_id, hops)` at an arrival and a merge, hops raised by a letting go and a split; flying; section 12. **Behind the setting**: sections 3 to 5, `Ended`, the place below the world. With it off a join places at once as today and **no note is made by anything** | Read by the sim (the leave) | The setting is off everywhere but in the sim's own tests; a leave without an entity meets the attempt the edge has sent since R1.0b | Scenario and differential tests, with the setting on and off | subagent, `crates/clustine-sim` |
| R1.3 | Runner: notes into the commit, the store's two answers into inputs, `Presence::Entering`, the restore | The same | The sim makes no notes with the setting off, so nothing new reaches the store and nothing is logged | Runner tests with the setting on | subagent, `services/worker` |
| R1.4 | Edge: all of sections 4, 4.1 to 4.3, 6 and 11; the abilities packet read. **This is the first step that changes what a client is sent**: a second connection on one edge puts the first out with the sentence, where it was refused | Read by the edge | **Every stay the edge meets has had a real attempt since R1.0b, and every action a real entity**, with the setting off as with it on. So a `Present` for a join that was applied and whose `Spawned` was lost carries the view's attempt and enters the player, as today's arm does (`fanout.rs:1292-1296`, and the tests at `fanout.rs:3663` and `11936`); the three paths of section 4.3 are met by the existing end-to-end tests that lose links during joins, a step before the store is in play; and actions across a boundary go on as before | The edge against scripted regions; the existing end-to-end tests; **the comparison for the reason of a second login, against Clustine** | main session, **not delegated** |
| R1.5 | The setting on in the one place that makes a `RegionConfig` outside tests (`bin/clustine/src/cluster/worker.rs:1358-1362`, which the single process and the cluster share), and removed; **`STATE_FORMAT` 5**. This is the first step in which a client is sent a look, the flying bit and a place | – | R1.1 to R1.4 are in | `single.rs`, `persistence.rs`; **the comparisons for entering with a look and flying, against Clustine** | main session |
| R1.6 | Bots across a leave: place, look, hotbar, slot, flying; the ledger's new rules for a bot whose edge was lost | – | – | The ledger | subagent, `tools/botswarm` |
| R1.7 | End to end, single process and a cluster with two edges: every row of section 10 by name; *a friend on the first edge has the player in the list after a second login through another edge*; the cases of "What is measured" | – | – | Tests from this record by someone who saw none of R1.1 to R1.5 | subagent |
| R1.8 | Docs, the notes in ADR-0012, 0014, 0015; what to try | – | – | CI | main session |

**Why the setting.** Without it R1.2 alone would stop every join: a region would hold
a player as entering and wait for an answer no runner passes on.

**Why `STATE_FORMAT` is raised twice.** A state written by a build between R1.0b and
R1.5 has stays the store has no record of. After R1.5 their notes would be dropped as
"cannot be", with an error for each, and their next join would be at the spawn point.
With format 5 such a state is dropped at the start (the runner:3134-3140): everybody
joins again once, at the spawn point, which is what the owner's update between two
phases is anyway (plan, section 3).

**Never delegated**: everything in `fanout.rs`, and the order within R1.4. Before R1.4
is begun, whoever builds it reads again: section 4.3 with the four steps of path 3,
section 4.4, the paragraphs on the numbering of `Ended` and on the player list in
section 6, and the notes for ADR-0012 rule 18 and ADR-0015 sections 2.1 and 7 under
"Changes to earlier records". The second review's findings 1, 2 and 11 changed what
those say about `fanout.rs`.

### If a guess is wrong

R1.0c may come back with "not so" for each of these, and then:

| The guess | If the official server or the client does otherwise | What changes |
|---|---|---|
| The serverbound abilities packet is one byte of flags and flying is `0x02` | Another bit or another shape | The packet in the codec and the constant in `play.rs`. Nothing else: the sim has a `bool` |
| A second login puts the first connection out | The official server refuses the second | Step 1 of section 4 goes back to today's refusal on one edge. Across edges nothing changes: the home region cannot refuse a join for a stay it does not have, so the later stay still wins and the earlier one is told `Ended`. The owner is asked, since the plan's question 4 was answered on this guess |
| The reason is the text component with the key `multiplayer.disconnect.duplicate_login` | Another key, or a plain string | The constant, or `refuse` as it is. The comparison is of the packet's NBT either way |
| The clientbound abilities packet with `0x02` and a position in the air leave a client flying | The official server sends something more or in another order on entering | The edge sends what the official server sends, in its order; `spawn_player`'s list in section 4.1 and step 4 of path 3 change together. Whether a real client then stays in the air only the owner's trial shows; if it falls, flying is not kept by this record and the place's height is, which a player notices as falling once after every join far above ground |
| A client numbers its actions on blocks afresh with every connection | It does not | Nothing breaks: the entity in `RemoteAction` is then a guard that is never needed |

### Tests written from this record by someone who did not write the code

| Crate | From | What |
|---|---|---|
| Store | Sections 3, 7, 8 | The table of section 3, one test a row, with the error asserted where it says one is logged. Two regions' commits in every order for one stay: the place is that of the highest hops. **Rule P's test, by the name given there, with its assertion that no segment is left.** A restore between a new block and the next checkpoint gives the same next id each time. The store killed at every write between an `Entering` note and its answer, while the players' file is written, and in `make_over`: after it, records equal those of the commits that count, and nothing is answered that is not on disk. The file that cannot be written before a make-over fails the start. A group that fails, by each of the paths into `fail_log` that has a group, takes its notes, its new records and `issued` back. A commit of a replaced owner leaves no note. A commit passed over by an `Opened` leaves no note. A world started with another division, and a block whose last id is `issued`: the home region's next stay is above every floor |
| Sim | Sections 3 to 6, 9, 12 | Scenario tests, one for each row of the tables in sections 3 and 6 and for step 7, **among them: an arrival against a later entering stay; an entering stay placed, and let go, over a present one; an arrival of the same entity with fewer and with more hops; a merge of two copies with different hops; *a stay let go unplaced in a tick that also passes an action on is numbered after it*; a leave without an entity against a stay of another attempt, entering and present; a place below the world**. Differential: a region restored from its state after any tick does what the region run on does, but for the notes of its first tick. A generated run with `dead` and `entered` at random ticks **never has two stays of a player, an entering one and a present one counted together**, and never loses an `Ended`. With the setting off: no note, ever |
| Runner | Sections 3, 4.2, 9 | The store lost at each point of a login; `Enter` and `Dead` dropped by a merge and a split and said again; a tick with only notes is committed; an entering stay the hello does not name is not answered |
| Edge | Sections 4.1 to 4.3, 6 | Against scripted regions: `Spawned` with a matching and a stale attempt; **each path of section 4.3 with the link lost before `Spawned`: `Present` from the view's region, `Present` from a part before and after the `SplitOff`, `SplitOff` alone followed by the split region's `Absent`, `Departed`**, each with a matching and a stale attempt; `Ended` for the view's entity, for another, for an entering view; rule 6 with the older entity moving after the newer was shown. **By name**: *a client that is gone when its stay is let go to it unplaced leaves an arrival followed by a leave, in that order*; *a view without an entity that a merge brought is entered by the survivor's answer and stays*; *a friend keeps the player in the list when the first connection is ended after the second was shown*; a `Refused` of an earlier attempt leaves the view; path 3 with `to` gone into the region the word comes from |
| End to end | Section 10 | R1.7 |

### What is measured (R1.7), and what follows

| Case | Asserted or measured | If it is not so |
|---|---|---|
| Join inside a friend's land | Asserted: no merge, no split, no chunk granted to the home region | A defect of R1 |
| **A player alone leaves far from home and joins again after more than half a minute.** Their region has given everything back and been absorbed by then (ADR-0017, section 3.4), so nobody holds the place: case 2. The home region places them, claims what they see and is split at once. This is the first thing the owner will do | Measured: how long the joining player stands still for the split that follows, and how long anybody at the spawn point does. Asserted: nobody at the spawn point stands still longer than ADR-0017 allows for a split (half a second in the middle, a second at worst) | If only the joining player waits, and for about as long as a split takes today: it is what walking out costs today, and stays. If they wait noticeably longer than that, or the spawn point stands still beyond the bound: the store makes a region for a place nobody holds when it answers `Enter` and names it as the holder, so that the home region never has the player. That is a change to step 5 and to ADR-0011, and is not designed here |
| The same within half a minute | Measured: the region is still there or has just given its land back; whichever of cases 2 and 3 it is, the player is in place | – |
| Join 10 to 22 chunks from a friend | Measured, as the plan has it | The plan's remedy: the store names the nearest region within the merge distance |
| Twenty join far apart at once | Measured, as the plan has it | The plan's remedy: a split that names several parts |
| The players' file with 10,000 records (R1.1) | Measured: how long the commit thread takes to write it | Below |

## What a player notices

- Leave and join: back in the same place, looking the same way, flying if they flew,
  with the same hotbar and held slot. Also after the server was stopped and started,
  and after a world was started with other pins.
- **The spawn point is where a player is once in the life of a world**, and no longer
  where every join begins. The one way back the game gives: **whoever leaves below the
  world's lowest block (below y = -64) comes back at the spawn point**, with what they
  held. So a player who dug through the floor and fell can leave and join, as today.
  One who stands in a hole anywhere above that comes back where they were.
- **Alone, far out, back after half a minute**: in place, and then standing still once
  for the split that makes their region again, as when they first walked out. How long
  is measured.
- Joining takes one commit longer than today, and two where another region holds the
  place: a tenth of a second or two. **Inferred** from a tick of 50 ms; not measured.
- Join as the same name from a second client: the first is put out with "You logged in
  from another location", the second stands where the first stood. Today the second is
  refused on the same edge, and on another edge the first stands in a dead world for
  20 seconds (`fanout.rs:840-863`).
- **Where exactly the second stands.** At the place of the first one's last commit that
  the store had taken when the login arrived. For a region that runs, that is behind
  the first client's own screen by the ticks the region ran ahead (eight at most, the
  runner:82), by the inputs it had not ticked yet and by the commits on their way: a few
  steps. For a region that is slow, hung or without a worker it is behind by everything
  the first edge still kept for it (`fanout.rs:938-939`), which is thrown away with the
  view or applied to the dead stay with its notes dropped. Still the place, and not the
  spawn point as today.
- **A friend who watches a second login** sees the first entity walk on for a tick or
  two and the new one appear a step behind it: what the dead stay did after the floor
  was raised is published like any tick.
- **Joining while the region of one's place is away** (taken over, or its worker
  dead): the home region and the store answer at once, and the player is in the world
  with nothing around them until that region runs. If that takes longer than the edge
  waits, 20 seconds, they read "The server fell too far behind." and can try again;
  each try comes out the same until the region runs, and then they are in place.
- **A friend on the first connection's edge** keeps the player in the player list
  through a second login from elsewhere.
- A friend who watches a leave and a join sees them go and come back. In a friend's
  land, no merge and no split follows.

## Ruled out

- **The new edge asks every region to end the old stay.** The plan's first draft; its
  review's finding 1.
- **The store holds a login back for the old stay's region.** Section 4, departure 2.
  The review looked at what it costs (a staler place) and would not bring it back
  either.
- **Regions remember floors.** Section 5, departure 3.
- **An outbox entry of its own for a stay let go without being placed** (`Entering`, in
  the first draft). Section 4.
- **Finding a view without an entity by "no join is kept"** alone, as today. It holds
  for the region the join went to and says nothing of a part or of a `Departed`
  (section 4.3).
- **A record written when a stay ends.** A stay's place is written while it lives, so
  how it ends does not matter.
- **Notes as a log record of their own** beside the commit. Two records can be cut
  apart by a crash.
- **The store reads the region's state.** It would tie the store's format to
  `STATE_FORMAT`.
- **A stay's order by a number of its own**, apart from the entity id. The sim orders
  by entity id already; section 7 makes that order true.
- **Recovery refuses a segment numbered below the players' file's `from` that has
  notes** (the review's second suggestion under finding 1). A start cannot tell such a
  segment from one that is rightly below `from` and kept for a lane or the table: both
  have notes that are, or should be, in the file. The guard is where segments are
  numbered (rule P).

## Risks

- **Read, not run.** Every "inferred" above is a claim.
- **The commit thread does more for every commit.** It is a map lookup a note.
- **Every commit grows.** A `Has` note is about 140 bytes; a hundred players who walk
  are about 280 kB a second in the log, beside the same again in the opaque state. If
  R1.7 finds it too much: a note that leaves the hotbar out when it did not change.
- **The players' file is written whole, with three syncs, on the commit thread**, and
  holds everybody who ever joined, 150 bytes each. Every state file put in place closes
  the segment (`lanes.rs:863-870`), so the file is written at most that often, and
  while it is written no region's commit is taken. With the default checkpoint every
  five minutes (the runner:67) that is nothing; with a short interval, as in tests, or
  many regions, it is a stall of every region. R1.1 measures it with 10,000 records. If
  it shows: the file is written by the thread for chunks from a copy, as a state file
  is (`store/lib.rs:216-222`).
- **A dead stay lives for up to eight ticks in a region that was just opened**, where
  the new stay went elsewhere. Where the new stay's arrival is kept for that same
  region, which is the usual case since the new place is the old one, the arrival
  replaces it before any of its inputs is applied.
- **A wrong `Dead` puts a player out.** The rule by hops is new. The review pushed it
  through a `NotMine` chain, a split and a stay that comes back by way of a part and
  could not make it remove a real stay; what it does not catch is two copies with equal
  hops. The store logs every `Dead` it answers by hops alone as an error.
- **Blocks of entity ids** are used up by changing the division (section 7).
- **The first tick after a restore says more than a tick of a region run on.**
- **`fanout.rs` changes in the paths where ordering mistakes hide**, while the owner's
  trial of C5 is outstanding (plan, section 9).

## Not traced, or unsure

1. **Nothing was built or run.** Two reviewers and I read the same code.
2. **Today's gap of section 4.1** (an answer to an earlier join taken for the current
   connection): the second review reads it as reachable today, by a bot rather than a
   hand. Not shown by a test.
3. **Two copies of a stay with equal hops** are not caught by anything here.
4. **The remedy for a place nobody holds** (the store makes a region) is named and not
   designed.
5. **Whether the coordinator's policy counts a region without a worker as without
   players**, which the order of events of section 4.4 passes through, and whether
   anything else there should count an entering stay: not read.
6. **The store's sockets** (`services/worldstore/src/tcp.rs`) beyond that replies are a
   queue; the store's `absorb` and `split` beyond their conditions.
7. **Whether the end-to-end tests that lose a link during a join exist as R1.4's row
   assumes**: the edge's own tests of it do (`fanout.rs:3663`, `11936`);
   `bin/clustine/tests` was not read for it.
8. **Records**: ADR-0015 sections 2, 6, 7 and 8, ADR-0012 sections 2.5 and 5.2 and
   rules 18 and 24, ADR-0014 rules 37 and 38 and ADR-0017 sections 3.4, 3.6, 3.6.3 to
   3.6.5 were read. ADR-0017's section 3.6.2 in its first part only; ADR-0010 and
   ADR-0016 not.
9. **The client**: everything in "If a guess is wrong".

## Review

### The first review

An independent reviewer read the first draft against the code at `6bf1613`. It found
ten things, the forty or so citations it followed right, and eight parts sound (an
entering view through a merge of the home region; no reply lost without a restore; the
rule by hops as far as it could be pushed; recovery's rule for commits an `Opened`
passes over; entity ids; the store's hello and `Dead`; determinism; all rows of the
table of orders but two). I checked each finding against the code it cites. All ten
hold; two of their proposed changes are taken differently.

1. **After a restart with no segment left, new segments are numbered below the players'
   file's `from`.** Holds (`lanes.rs:282`, `416-423`, `958-961`; the runner:2303-2304).
   Rule P in section 8, with `log.next` raised in `load`, an `expect` where a segment
   is begun, and a test by name. **Not taken**: recovery refusing a segment below
   `from` that has notes; a start cannot tell it from one rightly kept ("Ruled out").
2. **An earlier stay that arrives in the home region while a later one is entering is
   taken in and then overwritten without a word.** Holds (`region.rs:443-447`, `425`).
   Both halves: section 6's two tables have the rows, step 7 removes a present stay
   first, row 8a of section 10, and the sim's tests count an entering and a present
   stay together.
3. **Three build steps did not leave everything working.** Holds
   (`services/edge/src/lib.rs:265`). Sessions from 1; the attempt real in the join and
   echoed by the sim from R1.0b; no note while the setting is off; the codec and the
   comparisons against the official server in a step of their own before the edge
   (R1.0c), and against Clustine in the steps that first send each thing (R1.4, R1.5);
   `STATE_FORMAT` raised again at R1.5.
4. **`Region::absorb` keeps the survivor's copy whatever its hops.** Holds
   (`reshape.rs:177-180`). Stays are compared by `(entity_id, hops)` everywhere (rule
   7, section 6); the store logs a `Dead` by hops alone as an error.
5. **Conditions of the store's part.** All six written in: the file before a make-over
   is written or the start fails (section 8, step 5); a note only for a commit that is
   taken (section 3); the undo covers `issued` and new records and is not done twice
   (section 8; the review's list of paths into `fail_log` lacked `lanes.rs:1019`);
   `holder` is advice (section 3); `end <= issued + 1` (section 7); the directory scan
   (section 8).
6. **A view without an entity is not found by `SplitOff`, by a part's `Present`, or by
   a `Departed`.** Holds (`fanout.rs:1596-1600`, `1298-1317`, `1801-1808`). The attempt
   is in the player's state, the transfer, `Present` and `SplitOff` until the first
   input; section 4.3 has each path with its lines. **Taken further**: with the attempt
   in the transfer, the first draft's `Durable::Entering` is not needed and is gone.
7. **The solo rejoin, and case 2 against ADR-0015's premises.** The rejoin is a named
   case of what is measured, with what follows from either result. Section 9.1 goes
   through the three premises and ADR-0017's sections 3.6.3 and 3.6.5 by name: case 2
   adds no condition an arrival into an unknown chunk does not have today; the second
   premise holds by section 4.3 and not without it.
8. **The place can be staler than "eight ticks", and "never waits" reads better than it
   plays.** Holds. "What a player notices" says where the second stands, what a watcher
   sees, and what a player reads when the region of their place is away; the risk of
   eight ticks is narrowed to where the new stay went elsewhere.
9. **Records changed without saying so.** The section "Changes to earlier records",
   with the text each gets. An entering stay the hello does not name gets no answer,
   and why (section 4.2). "A stay is in one region's state at a time" is now true of a
   stay that lives.
10. **The players' file is written whole on the commit thread.** Holds
    (`lanes.rs:863-870`). Measured in R1.1 with 10,000 records; the thread for chunks
    is the remedy if it shows ("Risks").

### The second review

A second independent reviewer read the revised record against the code at `cc9cdfd`.
It found twelve things, the sixty or so citations it followed right, and judged the
record fit to build from once they are worked in, with no further round. It traced
what the revision had named as least sure and found it sound: a view under a part that
is absorbed before its link comes; a region under `stands_for`; the record without
`Durable::Entering`; the rule by `(entity, hops)`; a new block where the state names
another; rule P and every crash point of recovery; determinism and the single process;
a world from before. I checked each finding against the code it cites. All twelve
hold and none is rejected.

1. **Path 3 entered the player before it sent the arrival**, so a client gone at that
   moment left a stay that arrives after its own leave. Holds (`fanout.rs:2002`,
   `2035-2039`, `2298-2305`, `2669-2673`; today's order at `1862`, `1875`). Path 3
   keeps today's order, in four steps (section 4.3), with `back` as at any hand-over,
   and a test by name.
2. **The leave of a view without an entity names nothing**, and behind a merge it can
   end a later stay. Holds (`region.rs:432-438`, `fanout.rs:1472-1481`). `PlayerLeave`
   carries the attempt and a leave without an entity ends only a stay with it
   (section 4.4).
3. **Between R1.4 and R1.5 a stay had no attempt, and the edge ended every stay it
   found for a view without an entity.** Holds (`fanout.rs:1284-1297`). A stay's
   attempt is real from R1.0b whatever the setting; only the store's part is behind
   the setting ("Building it").
4. **Nobody was given the sim's half of section 11.** Holds (`region.rs:819-823`,
   `878-882`). R1.0b fills both entities; section 12 is not behind the setting.
5. **R1.0c was a subagent's and its verification is not a subagent's to run.** Holds
   (`CLAUDE.md`). The step is the main session's; "If a guess is wrong" says what
   follows from each result.
6. **A place is kept wherever it is, and nothing but a login took a player back to the
   spawn point.** Holds (`region.rs:156-157`, `404`, `1276-1287`). A place below the
   world's lowest block, y = -64, counts as no place (section 4); said under "What a
   player notices".
7. **The loser's edge took the player out of its list after the winner's entity was
   shown.** Holds (`fanout.rs:2634-2648`, `2753-2757`). The entry is kept for a client
   that shows a higher entity (section 6); a case of R1.7 and an edge test by name.
8. **The sentence is a translatable component on the official server, and `refuse`
   sends a string.** Holds for `refuse` (`fanout.rs:2870-2873`); the official server's
   form is a guess of both of us. Section 6 says the component is sent; the comparison
   is of the packet's NBT.
9. **The runner started at the first id of a new block on every restore until the next
   checkpoint.** Holds (`sim/state.rs:138-149`). The state's next id is kept if it
   lies inside the store's block (section 7).
10. **The players' file was looked at before the table was trimmed, so a clean stop
    could leave segments.** Holds (`lanes.rs:867-869`, `978-982`). The look comes after
    `trim_for_the_table`; rule P's test asserts that no segment is left.
11. **Three rules of earlier records changed without being named.** ADR-0012 section
    2.5 and rules 18 and 24, and ADR-0015 section 2.1's last line and section 7, are
    under "Changes to earlier records" with their text; the sim test for the numbering
    of a step-7 `Departed` is named (section 6); the presence arm falls through to the
    removal from `brought` (section 4.3), with its test.
12. **`Refused` named no attempt.** Holds (`fanout.rs:1084-1094`). It names it
    (section 4.2).
