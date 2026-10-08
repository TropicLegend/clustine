# ADR-0007: A single coordinator without storage, for now

- Status: **Accepted** for milestone M2; superseded when regions can move (M3). What
  [ADR-0008](0008-durable-regions-and-resuming.md) changed already is noted below
- Date: 2026-10-07

## Context

The [architecture](../architecture.md) describes the coordinator as replicated with Raft,
granting each region to a worker with a lease and an epoch that storage and peers
enforce. M2 needs much less: a fixed set of regions, each given to one worker once.
Building the replicated version first would delay the first cluster by a lot and decide
things that only M3's migrating regions can inform.

## Decision

- **One coordinator, state in memory.** Its decisions are a state machine without I/O
  and without a clock of its own, which the service around it drives; that is the part
  that will later sit on top of a replicated log.
- **Leases by being heard from.** A worker registers and sends heartbeats. One that is
  silent for longer than the lease loses its region, which goes to a worker that waits,
  with a higher epoch. A lost connection alone takes nothing away.
- **One region per worker**, given in the order the workers registered.
- **Epochs and blocks of entity ids are never issued twice.** Every assignment gets an
  epoch above all issued or reported before and entity ids no other assignment has.
- **A restarted coordinator asks rather than remembers.** It starts its epochs from the
  current time, so they are above its predecessor's, and for one lease it gives nothing
  away while workers that kept running report what they hold. Whatever they report, a
  region never goes back to an epoch it has left behind.
- **Only the world store acts on epochs so far.** It lets a higher epoch replace the
  owner of a region and refuses lower ones. Workers and edges compare epochs when they
  connect, which catches mistakes, not a worker that carries on after losing its lease.
- **Everyone compares the layout.** Workers, edges and the world store refuse to work
  with someone who divides the world differently.

## Consequences

- If the coordinator is down, nothing changes: regions stay where they are and players
  keep playing, but no worker can be replaced and no new edge can start.
- When a worker dies, its region is given to a waiting worker if there is one, which
  finds the world as the dead one had logged it. Players are disconnected meanwhile,
  because an edge without one of its regions starts over; keeping them is M3.
- Two workers can believe for up to a lease that they run the same region, if one of
  them is cut off rather than dead. The world store only listens to the newer one; an
  edge could still be connected to the older. Closing that gap needs leases that the
  holder itself gives up in time, which comes with M3.
- A coordinator whose clock was set back across a restart could issue an epoch that is
  not above every earlier one, unless a running worker reports a higher one.

## Since ADR-0008

- Being heard from no longer keeps a region: its owner has to vouch for it in its
  heartbeats, and the lease is 5 seconds.
- Entity ids are issued by the world store, once per region and for good. The
  coordinator still fills in `Assignment::entity_ids`, which nothing reads.
- When a worker dies, players are no longer disconnected: the edge keeps them and
  resumes with the worker that is given the region.
- A worker that is cut off rather than dead can show nobody anything: it gets nothing
  confirmed by the world store once the newer owner has opened the region, and what is
  not confirmed is not published.
- The world store keeps the highest epoch of each region on disk, refuses a lower one
  and says which it has seen; the coordinator issues above that from then on.
