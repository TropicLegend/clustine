# ADR-0008: Durable regions, output commit and resuming

- Status: **Accepted**; being implemented (milestone M3, phase A)
- Date: 2026-10-08

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
the code.

### 1. What a region's state is

Its chunks, and a `RegionState`:

| Part | Content |
|---|---|
| `tick` | The number of the last tick |
| `entity_ids`, `next_entity_id` | The region's block of entity ids, issued once by the world store and the region's for life, and the next free one |
| `players` | Per player a `PlayerState`: entity id, name, pose, hotbar, held slot, `last_input` (the number of the last input applied), `handled` (the highest sequence number of an action on blocks that was handled), `edge` (the edge the player belongs to) |
| `edges` | Per `EdgeId` an `EdgeState`: `start` (which start of that edge the region knows), `applied` (the number of the last message of that edge applied), `sent` (the number of the last outbox entry made), `outbox` (entries not yet confirmed) |

An `EdgeId` identifies an edge across its restarts; each start has a higher `start`
number than the one before.

### 2. The tick

A tick remains a function of the region and its inputs. New among the inputs:

- `edges: Vec<EdgeEvent>`, applied first, in order:
  - `Started { edge, start }`: an edge is there with this start. If the region knows the
    edge with a lower start, every player of that edge is removed (reported as
    `EntityRemoved`), its outbox is dropped, `applied` and `sent` become 0, and the new
    start is noted. An unknown edge is noted. An equal or lower start changes nothing.
  - `Confirmed { edge, number }`: outbox entries of that edge up to `number` are dropped.
  - `Gone { edge }`: the edge has been away too long. Its players are removed as above
    and the region forgets the edge.
- `applied: Vec<(EdgeId, u64)>`: the inputs of this tick contain the edge's messages up
  to this number; the region notes it as that edge's `applied`.
- Joining and arriving name the edge the player belongs to from then on. Leaving names
  the entity and only removes a player who has that entity.
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

- `Commit { tick, changes, state }` replaces `Log`: the block changes of a tick and its
  `StateDelta`, which to the store is bytes. It is answered with `Committed { tick }` once
  the record is on disk, and only then. Records of one region are committed in order.
- `Checkpoint { tick, state }`: every chunk changed up to `tick` has been saved before
  this request; `state` is the whole `RegionState` after `tick`, as bytes. The store
  writes it as the region's state file and drops the log records up to `tick`.
- Opening a region returns, with the handle, a `Restored`: the region's entity id block,
  the state file's content if there is one, and the state deltas of the log records after
  it, in order. The block changes of those records are applied to the stored chunks as
  before; the records stay in the log until a checkpoint drops them.
- The store issues an entity id block when a region is opened for the first time, and
  keeps it and the highest epoch of the region on disk.
- Commits have a lane of their own: saving chunks or opening one region does not delay
  another region's `Committed`.
- When the store starts it leaves the logs as they are.

### 4. The worker

- **Output commit.** After a tick the runner sends `Commit` and keeps everything the tick
  produced for edges until `Committed` for that tick has arrived: events, player events,
  chunk snapshots (taken at tick time), outbox entries, progress. Then it publishes them,
  tick by tick. The region goes on ticking meanwhile, but not more than eight ticks ahead
  of what is committed.
- **A link begins with `Hello { edge, start, seen }`**, where `seen` is the number of the
  last outbox entry the edge has got from this region. The runner turns it into
  `Started` and `Confirmed`, and answers `Welcome { applied }` with the edge's `applied`
  as of the last committed tick. It then sends the outbox entries above `seen` again, after
  what section 5 puts before them.
- **Numbered messages.** Join, leave, arrive, discard, input and remote action carry a
  number per (edge, region). The runner passes each on once, in order: a number not
  above `applied` is dropped, one that leaves a gap ends the link.
- **Subscriptions hold the line.** After a `Subscribe`, nothing further from that link is
  applied until the chunks are loaded.
- **Progress.** With each committed tick the runner tells each edge `Progress { applied,
  inputs }`: how far its messages are applied and durable, and for each of its players
  whose `last_input` changed, the new one.
- **Presence.** Asked `Presence { players }`, the runner answers for each whether the
  region has them, and if so with which entity, `last_input` and `handled`, and whether
  they have been told they entered the world.
- **Edges that stay away.** An edge without a link for 30 seconds is `Gone`.
- **Losing the store.** A region whose store handle is lost stops. Its worker closes the
  region's links, opens the region again when the store answers, and restores it. A
  region never carries on from memory after a commit went unanswered.

### 5. The edge

- It has a name, from which its `EdgeId` follows, and takes the time of its start as
  `start`.
- **Per region it keeps** the numbered messages not yet reported applied and durable
  (`Progress`), and the number of the last outbox entry it has seen. **Per player it
  keeps** the inputs not yet reported applied and durable by the region the player is in.
  A player whose kept inputs go back more than a minute is disconnected.
- **When a region's link ends** the edge keeps its players. What they do is numbered and
  kept as before, and sent when there is a link again. The routing table says where; a
  link to an owner with an older epoch than the table's is closed.
- **Resuming**, on a new link to a region, in this order:
  1. `Hello`, then `Subscribe` for every chunk of that region it shows, then `Presence`
     for every player it believes to be in that region, then every kept numbered message
     above `Welcome.applied`.
  2. From the region come, in this order: the snapshots; the presence answers; the outbox
     entries above `seen`; then ticks as usual.
  3. A snapshot of a chunk the edge already shows is reconciled: clients are sent the
     blocks that differ, entities in it are added, moved or removed. An entity of one of
     the edge's own players is never removed by this.
  4. A presence answer decides about a player: there, so the client is acknowledged up to
     `handled` and, if it was never told that it entered the world, told now; not there,
     and a join, an arrival or a `Departed` for them is still under way, so nothing;
     otherwise the player is disconnected.
- An outbox entry with a number the edge has seen is ignored. Others are handled as the
  messages they replace were, and confirmed.
- What a region sends is tagged with the epoch of the link it came over; what comes from
  a link that is no longer the region's is dropped.

### 6. The coordinator

- The lease is 5 seconds by default.
- A worker's heartbeat names the regions it has had a commit confirmed for within the
  lease. A region it holds but does not name loses its owner like one whose worker fell
  silent.
- The coordinator no longer issues entity ids.

## Why not replay

Logging each tick's inputs and replaying them would give the same without an outbox or
presence answers: outputs would simply be produced again. It needs every input of a tick
captured, including which stored version of a chunk arrived when, and a store that can
give back a chunk as it was at a checkpoint. Logging the changes of state needs neither.
The price is the list in section 5 of what is sent again and what is worked out again.

## Consequences

- A tick's results reach players one confirmed write later than before: a few
  milliseconds on a local disk.
- A replaced owner can show nobody anything: it gets nothing confirmed.
- The same steps serve a planned move (phase B) and merging and splitting (phase C).
- An edge is still a single point of failure for its players.
