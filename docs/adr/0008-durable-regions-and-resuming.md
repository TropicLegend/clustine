# ADR-0008: Durable regions, output commit and resuming

- Status: **Accepted**; being implemented (milestone M3, phase A)
- Date: 2026-10-08; revised the same day after an independent review (see the end)

## Context

Until M2 a region lives in the memory of one worker. Its block changes are logged, but a
player is told of them without waiting for the log, and everything else it knows (who is
in it, where, holding what) is nowhere else. When the worker dies, the edge can do nothing
but disconnect everyone.

M3 needs a region that another worker can carry on with, for three reasons that turn out
to be one: a worker died, a region is being moved, regions are being merged or split.

## Decision

A region can be **rebuilt from the world store**, and an edge can **resume** with a
rebuilt region. The rest of this record says exactly how. Words in `code` are names in
the code, or will be.

### 1. What a region's state is

Its chunks, and a `RegionState`:

| Part | Content |
|---|---|
| `tick` | The number of the last tick |
| `entity_ids`, `next_entity_id` | The region's block of entity ids, issued once by the world store and the region's for life, and the next free one |
| `players` | Per player a `PlayerState`: entity id, name, pose, hotbar, held slot, `last_input` (the number of the last input applied), `handled` (the highest sequence number of the player's own actions on blocks of this region that was handled; actions passed on to another region are not covered), `edge` (the edge the player belongs to) |
| `edges` | Per `EdgeId` an `EdgeState`: `start` (which start of that edge the region knows), `applied` (the number of the last message of that edge applied), `sent` (the number of the last outbox entry made), `outbox` (entries not yet confirmed) |

An `EdgeId` identifies an edge across its restarts; each start has a higher `start`
number than the one before.

`RegionConfig` loses `entity_ids`: a new region takes its block from what the store
returns when the region is opened (section 3), and a restored one from its state.

### 2. The tick

A tick remains a function of the region and its inputs; `clustine-sim` stays free of
clocks, I/O and hash maps. New among the inputs:

- `edges: Vec<EdgeEvent>`, applied first, in order:
  - `Started { edge, start }`: an edge is there with this start. If the region knows the
    edge with a lower start, the edge is **reset**: every player of that edge is removed
    (reported as `EntityRemoved`), the entity of every `Departed` in its outbox is reported
    as removed too (a departing entity is otherwise never reported removed, and would stay
    on the screens of those who saw it leave), its outbox is dropped, `applied` and `sent`
    become 0, and the new start is noted. An unknown edge is noted with `applied` and
    `sent` at 0. An equal start changes nothing. A lower start never reaches the tick
    (section 4).
  - `Confirmed { edge, number }`: outbox entries of that edge up to `number` are dropped.
  - `Gone { edge }`: the edge has been away too long. Its players and the entities of its
    outbox's `Departed` are removed as for a reset, and the region forgets the edge.
- `applied: Vec<(EdgeId, u64)>`: the inputs of this tick contain the edge's messages up
  to this number; the region notes it as that edge's `applied`.
- Joining, arriving and leaving name the edge they came from. That is input to the tick,
  not something on the wire: the runner knows which edge a link belongs to.
  - A join or an arrival makes the player that edge's.
  - A join of a player the region has under **another** edge replaces them: the old entity
    is reported removed and the player enters the world anew. This is the rule the runner
    applies by link today, carried over to edges; it is what lets a player come back
    through another edge. A join of a player the region has under the same edge is
    ignored, as today.
  - A leave removes the player only if they belong to the edge it came from, whatever
    their entity. A leave of the edge's player who has not been spawned yet (they quit
    while loading the world) therefore works, and a leave meant for an earlier connection
    through another edge cannot end the current one.
- Remote actions come with the edge that passed them on.

New among the outputs:

- `durable: Vec<(EdgeId, u64, Durable)>`: the outbox entries made in this tick, each with
  its number. A `Durable` is one of: `Departed { player, transfer }`, `Refused { player }`,
  `Remote(action)` (an action of one of the region's players that concerns another
  region), `RemoteDone { player, sequence }`. These no longer appear as player events,
  remote requests or remote outcomes. `Departed`, `Refused` and `Remote` go to the
  outbox of the player's edge, `RemoteDone` and a `Remote` that continues a remote action
  to that of the edge the action came from.
- `delta: StateDelta`: everything that changed in the `RegionState` in this tick, such
  that `state_before.apply(&delta) == state_after`.

`Region::state()` gives the `RegionState`; `Region::restore(config, state)` makes a region
from one, with no chunk loaded. `PlayerEvent::Acknowledged` is still emitted, and can be
worked out again from `handled`.

### 3. The world store

The store does not need to understand a region's state: state deltas and whole states are
bytes to it (postcard of `StateDelta` and of `RegionState`). Block changes stay in the
store's own record format and are applied to chunks by the store.

- `Commit { tick, changes, state }` replaces `Log`: the block changes of a tick and its
  `StateDelta`. It is answered with `Committed { tick }` once the record is on disk, and
  only then. Records of one region are committed in order.
- **Each region has a lane** on which its commits, its checkpoints and the opening of it
  are done in order. Commits of all regions that are waiting are written and synced
  together, so that a busy store does one sync for many regions. Saving and loading chunks
  is done elsewhere and does not delay a `Committed`.
- **Opening is fenced by the lane.** An open is done on the region's lane, after the old
  owner's commits that reached the store before it; from then on the old owner's commits
  are refused and its handle is lost. No old owner can be told `Committed` for a record
  that the new owner's restore did not read.
- **A failed write or sync loses the handle.** The log is cut back to its last good
  length, the region's handle is lost, and nothing is answered. A failed sync is not
  retried: what it was meant to make durable may be gone even if a later sync succeeds.
  The worker restores the region from what is on disk (section 4), which holds everything
  that was ever confirmed.
- **Chunks are saved as of a committed tick.** `Save { position, tick, chunk }` comes
  after the `Commit` of `tick` on the same handle, and is written only once that commit is
  on disk; if the commit fails, the save is dropped. A chunk on disk therefore never holds
  a change that a restored region knows nothing of. Saves and loads of one chunk are done
  in the order they were asked for: a load that follows a save returns what was saved,
  never the older stored chunk.
- `Checkpoint { tick, state }`: `state` is the whole `RegionState` after `tick`. The store
  waits until every save asked for before the checkpoint is on disk, then writes `state`
  as the region's state file and drops the log records up to `tick`. Records after `tick`
  can be in the log already, as the region runs ahead of what is committed; they are kept
  (the log is kept in segments, or rewritten, but never just emptied).
- **Opening a region returns a `Restored`** with the handle: the region's entity id
  block, the state file's content if there is one, and the state deltas of the log records
  whose tick is above the state file's, in order. Records are chosen by tick, never by
  their place in the log or by the tick a stored chunk was saved at. The block changes of
  those records are applied to the stored chunks as before; the records stay in the log
  until a checkpoint drops them.
- The store issues an entity id block when a region is opened for the first time, and
  keeps it and the highest epoch of the region on disk.
- An open with the epoch of the current owner replaces that owner's session: it is the
  same owner coming back after losing its connection, before the store has noticed. An
  open with a lower epoch is refused with the highest epoch seen.
- A chunk that cannot be read is answered with `Unreadable { position }` rather than not
  at all; the region leaves it unloaded, as today, and nothing waits for it.
- When the store starts it leaves the logs as they are.

### 4. The worker

- **Output commit.** After a tick the runner sends `Commit` and keeps everything the tick
  produced for edges until `Committed` for that tick has arrived: events, player events,
  chunk snapshots (taken at tick time), outbox entries, progress. Then it publishes them,
  tick by tick. The region goes on ticking meanwhile, but not more than eight ticks ahead
  of what is committed; at that bound it waits. A tick whose delta changes nothing but the
  tick number and that has no block changes sends no `Commit`, and counts as committed as
  soon as the tick before it is. Its tick number may be issued again after a restore,
  which nothing relies on.
- **A region is restored before it has links.** A worker accepts links to a region only
  once it is open and restored.
- **A link begins with `Hello { edge, start, seen, players, chunks }`**: which edge and
  start, the number of the last outbox entry the edge has got from this region, the
  players the edge believes to be in this region, and every chunk of this region the edge
  shows or has asked for.
  - A start lower than the one the region knows is refused with `Welcome::Superseded`,
    and the link is closed: that edge has been replaced.
  - Every other link of the same `EdgeId` is closed; what the region has to tell that
    edge from now on goes to this link. If the start is higher than the one known, what
    the old start's links had sent that waits for the coming tick or is held is dropped,
    so that nothing of the old start is applied after the reset.
  - The runner turns the hello into `Started` and `Confirmed { number: seen }` of the
    coming tick `t`, and subscribes the link to `chunks`.
  - From the region's state before tick `t` (that is, after `t - 1`), it makes the
    **resume**: `Welcome::Resumed` if the region knew the edge with this start, else
    `Welcome::Unknown`; then the outbox entries above `seen`; then for each of `players`
    a presence answer: `Present { entity, pose, hotbar, selected_slot, last_input,
    handled }` or `Absent`. The resume is published with tick `t`, before anything else
    of it. As `t` is published only once it is committed, everything in the resume is
    durable by the time the edge sees it.
- **Numbered messages.** Join, leave, arrive, discard, input and remote action carry a
  number per (edge, region), in an envelope `EdgeMessage { number: Option<u64>, body }`;
  hello, subscribe, unsubscribe and confirm carry none. The runner passes each on once,
  in order. It counts as **received** what it has passed on to the region or holds for
  the coming tick or behind a resume; a number not above that is dropped, one that leaves
  a gap ends the link. A link that ends leaves what it had sent for the coming tick in
  place, as it counts as received; only a higher start drops it (above).
- **A resume holds the line.** After a `Hello`, nothing further from that link is passed
  on until the snapshots of the hello's chunks are out (or the chunk is unreadable), so
  that what the edge sends again acts on loaded chunks. Ordinary subscriptions do not
  hold anything: a client only acts on chunks it has been sent.
- **Confirming.** An edge sends `Confirm { number }` for the outbox entries it has
  handled, at least once a second while it has any; the runner turns it into `Confirmed`.
- **Progress.** With each committed tick the runner tells each edge `Progress { applied,
  inputs }`: how far its messages are applied and durable, and for each of its players
  whose `last_input` changed, the new one.
- **Edges that stay away.** An edge without a link for 30 seconds is `Gone`.
- **Losing the store.** A region whose store handle is lost stops. Its worker closes the
  region's links, opens the region again when the store answers, and restores it. A
  region never carries on from memory after a commit went unanswered. If the store
  refuses the epoch, the region has another owner: the worker stops trying, drops the
  region and tells the coordinator the epoch it was refused with.

### 5. The edge

- It has a name that stays the same across its restarts (the edge runs as a StatefulSet),
  from which its `EdgeId` follows, and takes the milliseconds since the Unix epoch at its
  start as `start`. An edge told `Welcome::Superseded` stops: another process has its
  name. One that restarted on a clock that is behind is refused until the clock has
  passed its previous start, and comes back then.
- **Per region it keeps** the numbered messages not yet reported applied and durable
  (`Progress`), and the number of the last outbox entry it has seen. It trims what it
  keeps only on `Progress`. **Per player it keeps** the inputs not yet reported applied
  and durable by the region the player is in. A player whose oldest kept input is more
  than 20 seconds old is disconnected; that is well within the 30 seconds after which a
  region forgets an edge.
- **When a region's link ends** the edge keeps its players. What they do is numbered and
  kept as before, and sent when there is a link again. The routing table says where; a
  link to an owner with an older epoch than the table's is closed.
- **Resuming**, on a new link to a region:
  1. `Hello` with the players it believes to be in that region and every chunk of that
     region in its replica, including those whose snapshot never came; then at once every
     kept numbered message for that region. The region drops what it has received already.
  2. From the region come, in this order: the welcome; the outbox entries above `seen`;
     the presence answers; then ticks as usual, with the snapshots of the hello's chunks
     among them.
  3. `Welcome::Unknown` to an edge that had seen entries of that region, or trimmed
     messages for it, means the region has forgotten it: the edge disconnects the
     players it believed to be there, gives up what it kept for that region (a remote
     action given up is acknowledged to its player, who sees the block as the region has
     it), and numbers from 1 again with `seen` at 0.
  4. An outbox entry with a number the edge has seen is ignored. Others are handled as the
     messages they replace were, and confirmed. Because they come before the presence
     answers, a `Departed` that was committed but never delivered is passed on before the
     edge decides about that player, and a `Remote` is noted as under way before anything
     acknowledges a later action.
  5. A presence answer decides about a player. `Present`: the client is acknowledged up
     to `handled`, through the same holding back as every acknowledgement (an action
     under way elsewhere holds back later ones); if the edge has never shown the player
     their own entity, they are told now that they entered the world, with the entity,
     pose, hotbar and held slot of the answer. `Absent`, and a join, an arrival or a
     `Departed` for them is under way: nothing. Otherwise the player is disconnected.
  6. A snapshot of a chunk the edge already shows is reconciled: clients are sent the
     blocks that differ, entities in it are added, moved or removed. An entity of one of
     the edge's own players is never removed by this.
- An `EntityRemoved` removes an entity only from what the edge shows of the region that
  sent it. An entity that departed and lives on in another region is not taken off the
  screens by its old region forgetting it.
- What a region sends is tagged with the epoch of the link it came over; what comes from
  a link that is no longer the region's is dropped.
- The limit of 128 kept inputs per player goes; the 20-second rule replaces it.

### 6. The coordinator

- The lease is 5 seconds by default.
- A worker's heartbeat names, for each region it holds, whether it has had a commit
  confirmed for it within the lease, or is waiting for the world store to answer.
- A region the worker holds but does not vouch for loses its owner like one whose worker
  fell silent. A region counts as vouched for during the first lease after it was
  assigned, so that a new owner has time to open and restore it. A region whose owner is
  waiting for the store counts as vouched for up to 30 seconds: moving it would not help
  while nobody can reach the store, and after that the store is likely fine and the
  worker cut off from it.
- A worker refused by the store with a higher epoch reports it, and the coordinator
  raises the epochs it issues above it. Epochs come from the clock when the coordinator
  starts, and the store keeps the highest one on disk; a clock that went back would
  otherwise make every region impossible to open.
- The coordinator no longer issues entity ids.

## Why not replay

Logging each tick's inputs and replaying them would give the same without an outbox or
presence answers: outputs would simply be produced again. It needs every input of a tick
captured, including which stored version of a chunk arrived when, and a store that can
give back a chunk as it was at a checkpoint. Logging the changes of state needs neither.
The price is the list in section 5 of what is sent again and what is worked out again.

## Consequences

- A tick's results reach players one confirmed write later than before: a few
  milliseconds on a local disk. Every tick in which something changes is one commit;
  commits of many regions share one sync.
- A replaced owner can show nobody anything: it gets nothing confirmed.
- The same steps serve a planned move (phase B) and merging and splitting (phase C).
- An edge is still a single point of failure for its players.
- Entity id blocks are never given back. Phase C, whose splits each take a new block,
  has to say how blocks are reused.

## Review

An independent review against the code found fourteen defects in the first version of
this record, all worked in above:

1. Presence answers came before the outbox, so a committed `Departed` that had not been
   delivered made the edge disconnect a player who was on their way to another region.
2. A leave had to name the entity, which a player who quits while loading does not have;
   and nothing carried over the runner's rule that a join through another link replaces
   the player, so a player could be left as a ghost who cannot join again.
3. An edge that came back after being forgotten (`Gone`) with the same start was taken
   for a new one but kept its old numbers, and looped on a gap for ever.
4. A region just assigned had had no commit, so its owner would not vouch for it and
   would lose it at once; and every region would be moved during a store outage.
5. Presence answers were taken from memory, which runs ahead of what is durable, and
   lacked what is needed to tell a player they entered the world.
6. The lanes of the store were left open, and a direct implementation would lose block
   changes: a checkpoint that does not wait for saves, a log emptied with records after
   the checkpoint in it, a load that overtakes a save, an open not fenced against the old
   owner's commits; and chunks saved from memory could hold changes of ticks never
   committed.
7. A failed write or sync was not defined; a part-written record would hide every later
   confirmed one on recovery.
8. Holding the line on every subscription would make players freeze whenever they cross
   a chunk border while the store is busy, and for good on a chunk that cannot be read.
9. Two links of one edge at a runner, which half-open connections cause, could lose
   outbox entries and apply an old start's messages after a reset.
10. A start taken from the clock could be equal to or lower than the previous one.
11. Dropping an outbox dropped its `Departed`, whose entities stayed on screens.
12. Which `applied` decides what is dropped was not said; messages waiting for the tick
    have to count, or a remote action is applied twice.
13. A worker could not open its region again with its own epoch while the store still
    had its old session, and did not tell being replaced from the store being away.
14. Epochs kept on disk and epochs from the coordinator's clock could disagree for good.
