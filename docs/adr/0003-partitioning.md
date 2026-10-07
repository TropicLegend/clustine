# ADR-0003: World partitioning model

- Status: **Accepted**
- Date: 2026-10-07

## Context

One world has to be simulated by several workers. The model has to cover three workloads:
many players spread over a large world, dense crowds in one place, and few players with
very expensive farms and redstone. Vanilla behaviour must be preserved.

## Options

| Model | Summary | Main drawback |
|---|---|---|
| Static grid | Fixed shard borders, one worker per cell | Borders cut through bases and farms; mechanics crossing them deviate from vanilla |
| Dynamic regions | Nearby active chunks form a region; regions merge when close | One region is bounded by one worker |
| Halo exchange | Neighbouring workers swap boundary state every tick, as in domain decomposition | Hardest to keep vanilla-exact; needs synchronised ticks |
| Actor per chunk | Every chunk is an actor placed anywhere in the cluster | Redstone, fluids and entities become message-heavy across chunks |

## Decision

Use **dynamic regions** as the unit of ownership, ticking and migration, combined with a
**stateless fan-out tier** that performs interest management and packet encoding outside
the worker.

Halo exchange stays a later research goal for splitting a single dense region across workers.

## Consequences

- Region boundaries only run through inactive gaps, so no mechanic ever crosses a worker
  boundary within a tick and vanilla behaviour is preserved.
- Spread-out players and separate farms scale across workers directly.
- Dense crowds are helped by moving fan-out off the worker, but simulation of one crowded
  region remains limited by a single machine until halo exchange exists.
- The region graph (merge, split, ownership epochs) is core infrastructure and must exist
  before most game mechanics; see the [roadmap](../roadmap.md).
- Players converging from different workers force a region migration before the merge,
  so migration has to be fast and routine, not an exceptional path.
