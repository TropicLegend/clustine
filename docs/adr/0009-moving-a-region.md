# ADR-0009: Moving a region on purpose

- Status: **Proposed**; milestone M3, phase B. To be gone over by an independent reviewer
  before it is built.
- Date: 2026-10-08

## Context

Since [ADR-0008](0008-durable-regions-and-resuming.md) a region survives its worker: the
world store has everything a tick showed anyone, another worker restores the region from
there, and the edge keeps the region's players and resumes. That happens when a worker
is believed dead, which takes the coordinator's lease of 5 seconds to find out, and the
new owner restores from the log of every tick since the last checkpoint.

Operating a cluster needs the same on purpose: to empty a worker before its machine is
drained or its program replaced, and later (phase C) to put regions where there is room.
On purpose it can be quicker and gentler than after a death, because the old owner is
there to help.

## Decision

Moving a region is **a crash that the old owner prepares and announces**. Nothing new is
needed to keep players and state: the new owner opens the region at the world store and
restores it, and the edge resumes with it, exactly as in ADR-0008. What is new is only
that the old owner brings the store up to date, lets go, and says so, so that nobody
waits for a lease.

### 1. The steps

1. Someone asks the coordinator to move region `R` to worker `T` (section 4).
2. The coordinator checks that `R` has an owner `S`, that `T` is registered, waits (runs
   no region) and is not `S`, and that `R` is not being moved already. If not, it
   refuses and says why. Otherwise it notes `R` as **being released to `T`** and tells
   `S`: `Release { region, epoch }`, with the epoch `S` runs `R` with.
3. `S` **releases** the region if it runs it with that epoch (a release for another
   epoch is ignored):
   - It stops ticking. What edges send from now on is not taken any more; they keep it
     and send it again to the new owner.
   - It waits until the store has confirmed every commit, and publishes what those
     ticks produced, so that no edge is left without something that is durable. (It
     would get it from the resume anyway; this only keeps the gap short.)
   - It saves every changed chunk and sends a `Checkpoint` with the whole state, and
     waits until the store has done all of it (`Flush`).
   - It closes the region at the store and closes the region's links.
   - It tells the coordinator `Released { region, epoch }` and is a waiting worker
     again.
   If the store handle is lost on the way, it does the same without the steps that need
   the store: the store has what was confirmed, and the rest was never shown.
4. On `Released` from `S` for the epoch it asked about, the coordinator assigns `R` to
   `T` with a new epoch, at once and whatever the grace period of a new coordinator says,
   and publishes the routing table. If `T` is no longer registered and waiting, `R` goes
   to any waiting worker as a region without an owner does.
5. `T` opens `R` at the store, restores it (from the checkpoint, with no deltas to
   apply), runs it and takes links. Edges see the new route, link, say hello and resume.

### 2. When the old owner does not answer

A region that has been **being released** for longer than the lease without a
`Released` is treated as one whose owner died: the coordinator assigns it to `T` (or to
any waiting worker) with a new epoch. The store fences `S` from then on, whatever it
was doing. If `S` falls silent altogether, its lease running out does the same sooner
or at the same time. If `S` registers again or vouches for `R` meanwhile, that changes
nothing: once a release is asked for it is not taken back.

While a region is being released its owner does not vouch for it, and need not: the
coordinator does not take a region for want of vouching while it is being released.

A `Released` for a region that is not being released, or with another epoch than the
one asked about, is ignored but for a log line. A worker says `Released` only in answer
to a `Release`; one that wants to give up a region by itself says so as in section 3.

### 3. A worker that is told to stop

A worker that gets SIGTERM tells the coordinator `Leaving` and goes on running. The
coordinator notes it as leaving: it is never given a region again, and for each region
it owns the coordinator begins a release as in section 1, to any waiting worker that is
not leaving; a region for which there is no such worker stays where it is. The worker
exits when it owns nothing any more, or after 20 seconds at the latest, in which case it
stops as it does today: it saves what is not saved and closes the region, and the region
waits for a worker as after any stop. Kubernetes gives a worker 30 seconds.

So replacing the workers one after the other, with one worker to spare, moves every
region once and makes nobody wait for a lease.

### 4. Asking for a move

`clustine move --region R --to T [--coordinator host:port]` connects to the coordinator
and says `Move { region, to }`, where `to` may be left out for "any waiting worker". It
is answered with `MoveBegun { from, to }` or `Refused { reason }`. The command then
watches the routing table and returns when `R` has another owner than `from`, printing
how long that took, or fails after 30 seconds.

Like everything else the services say to each other, this is not authenticated; whoever
reaches the coordinator's port can move regions.

### 5. What players notice

Players of the region stand still from when the old owner stops ticking until the new
one has restored the region and the edge has resumed: the time of a checkpoint (a few
changed chunks and the state), opening and restoring from a state file, and one round
between edge and worker. The aim is **well under a second** on one machine, and the test
of B1 measures it and fails above a bound. Nobody is disconnected, nothing shown is
lost, and what players did meanwhile takes effect afterwards, as in ADR-0008.

For that the edge may not wait a second before it tries the new owner: when a link has
ended or a route has changed, it tries at once, then every 100 milliseconds for the
first two seconds, and every second after that.

### 6. Messages

- To a worker: `FromCoordinator::Release { region, epoch }`.
- From a worker: `ToCoordinator::Released { region, epoch }`, `ToCoordinator::Leaving`.
- From whoever operates: `ToCoordinator::Move { region, to: Option<String> }`, answered
  with `FromCoordinator::MoveBegun { from, to }` or `FromCoordinator::Refused`.

## Why not hand the state over directly

The old owner could send the region to the new one and spare the store a checkpoint.
That would be a second way to get a region from one worker to another, with its own
failures in the middle (the old owner dies while sending; the new one dies holding the
only copy of a few ticks). Going through the store there is one way, which the chaos
tests of phase A already try, and a move that fails at any point is a crash that is
already recovered from.

## Consequences

- A move is as safe as a crash, and a crash at any point of a move is an ordinary one.
- The pause is bounded by a checkpoint and a restore, not by the lease.
- A region can only be moved to a worker that runs none, because a worker runs one
  region until phase C.
- With no waiting worker a worker that leaves cannot hand anything over; its regions
  stand still until a worker is there, as today.
