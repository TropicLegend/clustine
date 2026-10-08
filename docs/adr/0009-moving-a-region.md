# ADR-0009: Moving a region on purpose

- Status: **Accepted**; being implemented (milestone M3, phase B)
- Date: 2026-10-08; revised the same day after an independent review (see the end)

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
waits for a lease. A move that fails at any point is a crash that is already recovered
from; the rest of this record is about not turning it into one needlessly.

### 1. The steps

1. Someone asks the coordinator to move region `R`, to worker `T` or to any worker
   (section 4).
2. The coordinator refuses, saying why, unless `R` has an owner `S`, `R` is not being
   released already, and there is a **target**: a worker that is registered, has a
   connection right now, runs no region, is not leaving, is not the target of another
   release, and is not `S`. Asked for a certain `T`, that has to be it. Otherwise it
   notes a **release**: the region, the epoch `S` runs it with, `S`, the target and
   the time. The target is **reserved**: it is given no other region and is nobody
   else's target while the release lasts. The coordinator tells `S`:
   `Release { region, epoch }`.
3. `S` releases the region if it holds it with that epoch; a release for anything else
   it answers with `Released` at once, as it does not have it.
   - If it is still opening or restoring the region, it gives that up, closes the
     handle if it gets one, and says `Released`.
   - If it runs it: first an ordinary checkpoint **while it goes on ticking** (every
     changed chunk saved, the whole state, and a `Flush` to know the store has all of
     it), so that what is left to do while players stand still is small. Then it stops
     ticking and takes no more links; what edges send from now on they keep and send
     again to the new owner. It waits until the store has confirmed every commit and
     publishes what those ticks produced. It checkpoints once more, which is the few
     chunks and the state changed since a moment ago, waits for the store to have it,
     closes the region at the store, and closes the region's links.
   - If the store handle is lost on the way, it skips what needs the store: the store
     has what was confirmed, and the rest was never shown.
   - It tells the coordinator `Released { region, epoch }`, never takes up that
     assignment again, and is a waiting worker, at the back of those that wait.
4. On `Released { region, epoch }` from the worker that owns `region` with `epoch`, the
   coordinator takes the region from it and assigns it at once, with a new epoch: to
   the release's target if that is still a target, else to any. This holds whether or
   not a release was asked for (a coordinator that started anew has asked for none, and
   a worker that is asked to stop with nobody to hand over to says it by itself), and
   whatever the grace period of a new coordinator says: the owner itself says that the
   region is free. If there is no target, the region is without an owner and is given
   to the first worker that waits, as ever.
5. The target opens `R` at the store, restores it, runs it and takes links. Edges see
   the new route, link, say hello and resume.

### 2. What the coordinator keeps straight

- **A release is of one owner and one epoch.** Whenever the region's owner or epoch
  changes for another reason (the owner's lease runs out, it reports an epoch the store
  refused, it does not vouch), the release is dropped, and its target is free again.
- **A release that is not answered** within the lease, counted from when it was first
  asked, ends like the owner's death: the region is taken from the owner and assigned,
  to the target if it still is one. The store fences the old owner from then on.
- **Vouching.** A region that is being released does not lose its owner for want of
  vouching; its owner has stopped ticking on purpose.
- **Messages get lost with connections.** When the owner of a region that is being
  released registers again, the coordinator tells it `Release` again; if what it says
  it holds lacks the region, that is its `Released`. A worker whose orders still
  contain an assignment it has released says `Released` again.
- **A worker is not at fault for losing a region.** A worker whose region is no longer
  among its orders, for whichever reason, drops the region (it stops the runner as it
  is, without the steps above: another worker may have the region already, and the
  store fences this one), and is a waiting worker. It no longer exits with an error.
- A `Released` from a worker that does not own that region with that epoch is ignored
  but for a log line.

### 3. A worker that is told to stop

A worker that gets SIGTERM tells the coordinator `Leaving` and goes on running.

- **Leaving belongs to one registration.** The coordinator gives a leaving worker
  nothing new. A worker that registers is not leaving, whatever a worker of that name
  said before: a replaced pod comes back under its name. A worker that is still leaving
  after registering again says so again.
- For each region a leaving worker owns, the coordinator begins a release as in section
  1 **as soon as there is a target**, and looks again whenever a worker registers and at
  every tick: the spare may register a moment later.
- When a leaving worker owns nothing any more, the coordinator forgets it and closes
  its connection, which is how the worker knows that it may exit. If a leaving worker's
  connection ends by itself, the coordinator treats it as gone at once: its regions are
  without an owner and assigned as such. The store fences it if it lives.
- The worker exits when the coordinator has closed its connection, when it cannot
  reach the coordinator at all, on a second signal, or after 20 seconds, whichever
  comes first. With a region left it then stops as it always has: it saves what is not
  saved and closes the region. Kubernetes gives a worker 30 seconds; a store that
  neither answers nor closes can hold a stop until Kubernetes kills the process, which
  is a crash like any other.

A worker that is restarted in place and registers under its name within the lease is
given its region again with the epoch it had, as before this record; the store lets the
same owner open it again. That is quicker than a move when there is nobody to move to.

Replacing the workers one after the other, with one worker to spare, moves each region
once or twice (a region can move to a worker that is replaced next), and makes nobody
wait for a lease. So that "ready" means something to whoever replaces them, a worker
listens for edges only once it has registered with the coordinator.

### 4. Asking for a move

`clustine move --region R [--to T] [--coordinator host:port]` connects to the
coordinator and says `Move { region, to }`. The coordinator answers on that connection:
`MoveRefused { reason }`, or `MoveBegun { from, to }` and, when the release has ended,
`MoveDone { to, epoch, released }`: who has the region now and with which epoch, and
whether the old owner released it or was taken for dead. The coordinator does not hang
up on a connection that waits for this. The command prints the outcome and how long it
took from asking to the new assignment; it does not know how long players stood still,
which is for tests to measure where the players are.

It is for a cluster; the single process has no coordinator and nothing to move a region
to. Like everything else the services say to each other, it is not authenticated:
whoever reaches the coordinator's port can move regions.

### 5. What players notice

Players of the region stand still from when the old owner stops ticking until the new
one has restored the region, the edge has linked to it, and the resume is through:

- the last checkpoint, small because of the one before it;
- opening and restoring from the state file;
- the edge reaching the new owner: when a link ends or a route changes it tries at once
  and then every 100 milliseconds for two seconds, each region by itself and without
  waiting for another region's attempt, and it takes up a new route also while an
  attempt at the old one is under way;
- the resume, which holds what players did until the new owner has loaded and sent
  again every chunk of the region that the edge shows (ADR-0008, section 4). That is
  hundreds of chunks per player at a usual view distance, and probably most of the
  pause.

Nobody is disconnected, nothing shown is lost, and what players did meanwhile takes
effect afterwards. How long the pause is, is **measured, not promised**: the test of B1
measures it at the bots, between processes, with bots spread out at a view distance of
8, reports it, and fails above 3 seconds. If it is more than about a second there, the
resume is changed to hold only what acts on chunks that are not there yet, which
ADR-0008's review already named as the alternative.

### 6. Messages

- To a worker: `FromCoordinator::Release { region, epoch }`.
- From a worker: `ToCoordinator::Released { region, epoch }`, `ToCoordinator::Leaving`.
- From whoever operates: `ToCoordinator::Move { region, to: Option<String> }`, answered
  with `FromCoordinator::MoveRefused { reason }`, or `FromCoordinator::MoveBegun { from,
  to }` and later `FromCoordinator::MoveDone { to, epoch, released }`.

All services of a cluster are of one build; a worker of an older build would not
understand `Release`.

## Why not hand the state over directly

The old owner could send the region to the new one and spare the store a checkpoint.
That would be a second way to get a region from one worker to another, with its own
failures in the middle (the old owner dies while sending; the new one dies holding the
only copy of a few ticks). Going through the store there is one way, which the chaos
tests of phase A already try.

## Consequences

- A move is as safe as a crash, and a crash at any point of a move is an ordinary one.
- The pause does not include the lease or the log since the last checkpoint.
- A region can only be moved to a worker that runs none, because a worker runs one
  region until phase C.
- With no waiting worker a worker that leaves cannot hand anything over; it stops as
  before, and its region stands still until a worker is there.
- A worker no longer exits because a region was taken from it.

## Review

An independent review against the code found eleven defects in the first version of
this record, all worked in above:

1. "Leaving" stuck to a worker's name, so a replaced pod, which registers under the
   same name, was never given a region again, and the next worker to leave had nobody
   to hand over to.
2. A `Release` or `Released` lost with a connection made the old owner take the region
   up again and then exit with an error when the timeout gave it away.
3. The pause left out what dominates it: a last checkpoint of up to five minutes of
   changed chunks, and the resume's hold until every chunk the edge shows is loaded and
   sent again.
4. The target was not reserved, so two moves could pick one worker, and the note of a
   release outlived a change of owner and took the region from its new one.
5. A release to an owner that was still restoring the region was not defined.
6. After the coordinator started anew in the middle of a release, the region waited a
   lease.
7. `clustine move` could not tell whom the region went to from a routing table, would
   have been cut off as a silent connection, and measured the release, not the pause.
8. The edge tried routes one after the other, each for up to two seconds, and did not
   look at a new routing table meanwhile.
9. A rolling replacement moves a region once or twice, not once, and a pod was ready
   before its worker had registered.
10. Stopping a runner did not wait for outstanding commits or publish them, and nothing
    could interrupt a stop that waits for a store that hangs.
11. A second Ctrl-C did nothing, a worker that could not reach the coordinator waited
    20 seconds for nothing, and a worker without a connection could be picked as a
    target.
