# ADR-0005: The edge forwards semantic inputs, not packets

- Status: **Accepted**
- Date: 2026-10-07

## Context

The edge terminates client connections and the worker simulates regions. They run in one
process in milestone M1 and in separate processes from M2. The interface between them
decides whether the worker has to know the Minecraft protocol, and whether one player's
view can span regions owned by different workers.

## Decision

- The edge owns everything protocol-specific: the connection state machine, compression,
  keep-alives, teleport confirmation, chunk batch flow control, each player's view, and a
  replica of the chunks it has subscribed to.
- The worker owns region state and never sees a Minecraft packet, a view or a protocol version.
- Edge to worker: player join and leave, **semantic inputs** (move, dig, use item on, set
  held slot), and chunk subscriptions. Subscriptions are per chunk and per link, reference
  counted by the edge across its players; a subscription also keeps the chunk loaded.
- Worker to edge: one chunk snapshot per subscription, a per-tick delta of region events
  filtered to the link's subscribed chunks, and events addressed to a single player.
- Both sides only hold link endpoints. In M1 the link is an in-process channel; a framed,
  serialised variant of the same link is exercised by the tests from the start.

## Consequences

- Supporting another protocol version, or Bedrock, is confined to the edge.
- The worker fans out per edge and the edge fans out per player, so a second viewer of a
  chunk costs the worker nothing.
- Because subscriptions are per chunk, a player near a region boundary can be served by
  two workers at once.
- The edge holds a copy of every chunk its players can see, which costs memory and must
  not drift from the worker's state; a debug check compares hashes.
- Input validation that needs world state (reach, collisions) happens on the worker, on
  semantic inputs.
