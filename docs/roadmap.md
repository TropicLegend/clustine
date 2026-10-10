# Roadmap

Ordering principle: get a distributed world working with trivial gameplay first, then add
vanilla mechanics on top of it.

| Milestone | Scope | Done when |
|---|---|---|
| **M0 Foundations** | Workspace skeleton, CI, architecture and decision records, parity matrix template, evaluation of reusable libraries, `botswarm` skeleton | Workspace builds and tests pass locally and in CI |
| **M1 Single-node walking skeleton** | Login, flat world, movement, placing and breaking blocks, chunk streaming through edge, world persisted in the Clustine format, single-binary mode | `botswarm` sessions (login, walk, place, break) pass against the single binary; world survives a restart |
| **M2 Two-worker cluster** | Coordinator with leases, static region assignment, player and entity handoff between workers | Bots walk across a worker boundary on a local Kubernetes cluster with no disconnects and no duplicated entities |
| **M3 Dynamic regions** | Merge and split, live migration, crash recovery from the log, fencing | Chaos tests killing workers lose no acknowledged state |
| **M4 Kubernetes** | Operator, Helm chart, autoscaling on tick time, drain on termination, metrics and tracing | Rolling update of all workers under bot load with no disconnects |
| **M5 Dense crowds** | Independent fan-out scaling, per-region time dilation, dynamic view distance | Benchmark with 1,000+ bots in one place published |
| **M6+ Vanilla parity** | Tiers T1–T5 of the [parity matrix](parity-matrix.md), worked on in parallel | Nightly differential tests against the vanilla server |
| **M7 Plugins** | WebAssembly plugin API, later the Bukkit bridge | Example plugins run unchanged on one and on many workers |

M6 can start once M3 has stabilised the simulation API; it does not wait for M4 and M5.

## M1 steps

M1 was completed on 2026-10-07. Besides the automated tests, joining, walking, seeing
other players, breaking and placing blocks, and the world surviving a restart were
checked by hand with unmodified 26.3 clients.

M1 targets Minecraft: Java Edition 26.3 in offline mode, with the server bound to
localhost. Online mode and encryption were planned to follow directly and were then put
behind M2.

"Oracle" means the same bot scenario is also run against Mojang's official server, which
guards against a mistake shared by our server and our bot. "Real client" means a manual
check with an unmodified 26.3 client.

| # | Scope | Verified by | Status |
|---|---|---|---|
| 0 | Licence, decision records, protocol notes | CI | done |
| 1 | `tools/datagen` and the generated id tables | `datagen --check`, tests on known ids | done |
| 2 | Codec primitives and framing | Known-answer vectors, property tests | done |
| 3 | Listener, handshake, status | Bot `ping`; real client lists the server | done |
| 4 | Offline login, configuration, entering play, keep-alive | Bot idles 30 s; oracle | done |
| 5 | Sections, palettes, flat generator, chunk packet | Property tests; chunk compared with the oracle's | done |
| 6 | Region, edge/worker messages and links, tick runner | Simulation tests over both link kinds | done |
| 7 | Edge fan-out, chunk replica, chunk batches | Bot receives the expected chunks; real client sees the world | done |
| 8 | Movement, view updates, chunk loading and unloading | Bot walks 200 blocks; replay test | done |
| 9 | Other players: spawn, move, remove | Two bots see each other | done |
| 10 | Breaking blocks | Bot A breaks, bot B observes; real client | done |
| 11 | Placing blocks from a fixed hotbar | Bot `place`; real client | done |
| 12 | Section format, manifests, local storage, checkpoint on shutdown | Restart test | done |
| 13 | Write-ahead log, recovery, periodic checkpoint | Torn-write tests; `kill -9` test | done |
| 14 | Compression, slow-client handling, timeouts, parity matrix | Tests for compression settings, dawdling and non-reading clients | done |

The comparisons with the official server need Java and agreement to the Minecraft EULA,
so they are run by hand and not in CI.

Not in M1: survival mechanics, chat and commands, a lighting engine, neighbour updates,
server-side collision, entities other than players, more than one dimension, player data
persistence and real world generation.

## M2 steps

| # | Scope | Verified by | Status |
|---|---|---|---|
| 1 | Region layout, areas of regions, numbered inputs, letting players go and taking them in | Simulation tests, among them two regions with a router compared with one region | done |
| 2 | One world store for several regions, a log per region | Store tests for independent logs, recovery and ownership by epoch | done |
| 3 | A worker that serves several edge links and survives losing one | Runner tests over both kinds of link | done |
| 4 | The edge shows one world out of several regions and hands players over; `--boundaries` in the single binary | Bots cross both ways, watched; a crowd crossing; leaving in mid hand-over; restart and kill with two regions; the M1 end-to-end tests once more on a divided world | done |
| 4a | Breaking and placing blocks across a boundary | Simulation tests comparing two regions with one; a bot working across the line in every combination | done |
| 5 | Links over TCP; the world store as a service | Tests over real sockets | done |
| 6 | The coordinator: its decisions as a state machine, and the service around it | State machine tests; service tests over real sockets | done |
| 7 | `clustine coordinator`, `worldstore`, `worker`, `edge` | A test that starts the five processes, has bots cross between the workers, kills everything and finds the world again | done |
| 8 | Container image, Kubernetes manifests, the cluster test with kind | `deploy/kind/test.sh` | done |

The cluster test first passed on 2026-10-07, on a kind cluster with a coordinator, a
world store, two workers and an edge: four bots walked back and forth across the
boundary 24 times and built on both sides, a fifth saw each of them as one entity
throughout, and each worker took players in and let them go twelve times.

A check by hand with unmodified clients on 2026-10-08 found building to work and to be
seen by others, and blocks on the other side of a boundary to be out of reach, which was
how it had been planned. That was changed the same day: what a player does to blocks of
another region is now passed on to it, which the same clients then confirmed, breaking
and placing across the boundary from both sides.

The design decisions are in [ADR-0006](adr/0006-static-regions-and-handoff.md) and
[ADR-0007](adr/0007-coordinator-scope.md).

Not in M2, and known to be missing:

- Regions are fixed stripes. A player standing on a boundary is handed back and forth,
  and what a player does to blocks on the other side of one takes a tick or two longer,
  with the small differences from one region that ADR-0006 lists.
- Nothing survives the loss of a process without players being disconnected. The world
  itself does survive.
- One edge. With several there is no shared player list and nothing stops an account
  from being on two edges at once.
- The services trust whoever reaches their ports.

## M3 plan

Planned and agreed on 2026-10-08. M3 is about three times M2 and is built in three phases
on one mechanism: **a region can be rebuilt from the world store, and an edge can resume
with a rebuilt region.** Failover, migration, merging and splitting are that, started for
different reasons. [ADR-0008](adr/0008-durable-regions-and-resuming.md) specifies the
mechanism; read it before touching phase A.

Decided with the owner:

- All three phases, **with a stop after each** for a check with real clients.
- **Workers and the world store** are survived without disconnecting anyone, and both are
  what the chaos tests kill. A dead edge still takes its players with it; the single
  coordinator is merely missed while it is away.
- **Takeover after 5 seconds**: that is the lease, so players of a dead worker stand
  still for about 5 to 7 seconds.

"No acknowledged state lost" means: every block change a client was shown or told was
handled is on disk before it is shown; a player of a region whose worker dies stays
connected and keeps entity, position, hotbar and held slot as last made durable, with
everything done since applied again from what the edge kept; and nobody else sees that
player vanish or appear twice.

### Phase A: recovery

Regions stay the fixed stripes. A spare worker waits; when a worker's lease runs out, its
region goes to a waiting worker, edges keep their players, hold what they do, reconnect
and resume. A region that loses the world store is torn down by its worker and restored
from disk when the store is back.

| # | Scope | Verified by | Status |
|---|---|---|---|
| A0 | Numbers and ids on the wire at both ends, snapshots taken at tick time, epoch tags at the edge; no change in behaviour | All existing tests | done |
| A1 | Store and format: commit lane and `Committed`, state records and state file, restored state on opening, epochs and id blocks on disk, no folding at start | Store tests: kill at every point of a commit, a checkpoint and a recovery; latency of commits while chunks are saved | done; commits are shown not to wait for saves, not timed on a real disk |
| A2 | Sim: export and restore of state, per-tick state changes, outbox, inbox numbers | Unit tests; restore then the kept messages equals the uninterrupted run up to the commit followed by the rest in one tick | done, with tests from the ADR by someone who had not seen the code |
| A3 | Worker: publish after commit, resume, edge starts and expiry, restore after losing the store | Runner tests incl. a runner dropped between commit and publish | done, with tests from the ADR by someone who had not seen the code |
| A4 | Edge: name and start count, outbox per region, kept inputs per player, resume and reconciliation, living through the loss of a region | E2E in one process: a region is torn down without warning and rebuilt while bots walk, build, hand over and watch | done |
| A5 | Coordinator: lease 5 s, per-region vouching, table changes that edges live through | State machine and service tests | done (the worker reports real vouches in A3; edges living through a change of owner is A4) |
| A6 | Chaos tests: workers and the world store killed at random under bots that keep a ledger of everything acknowledged; on kind by deleting pods; in CI | No disconnect, ledger equals world, one entity per player throughout, repeatedly | done |
| A7 | Docs | CI | done |

Then the owner's check: kill a worker while playing. What to try is under "Where M3
stands".

### Phase B: live migration

The coordinator asks an owner to release a region; the owner finishes its tick, waits
until everything is committed and published, checkpoints, closes the region and says so;
the region goes to the target, which opens and restores it; edges resume. An owner that
does not answer within the lease is treated as dead, which is the same as a crash.

| # | Scope | Verified by | Status |
|---|---|---|---|
| B0 | ADR-0009 reviewed; its messages on the wire, refused or ignored by everyone | All existing tests | done |
| B1 | Release and assign; `clustine move` to ask for it | Move under bot load: nobody disconnected, pause measured and bounded | done |
| B2 | A worker asked to terminate hands its region over first | Rolling restart of all workers under bots, as processes and on kind | done |
| B3 | ADR-0009, docs | CI | done |

What the owner is to try is under "Where M3 stands": `clustine move` and restarting
workers while playing.

### Phase C: regions that follow players

[ADR-0010](adr/0010-regions-that-follow-players.md) is the plan, gone over by an
independent reviewer, whose twenty-one findings are worked into it. In short:

- A region is a set of chunks and the players standing in them. The world store says
  which region holds a chunk: it grants a chunk to the first region that asks and takes
  it back when that region no longer needs it. A region nobody runs keeps its chunks.
- An edge asks the viewer's region for a chunk; if another region holds it, it is told
  which and asks there as a guest. Hand-over and passing on of block actions work as
  before, on chunk sets, and name the region they go to; what reaches a region that no
  longer holds the chunk comes back as "not mine".
- There is always a home region with the chunk players enter the world in. It gives
  out all entity ids and is never absorbed.
- The coordinator hears where each region's players are and lists regions from the
  store. It has regions merged whose players come within a merge distance and a region
  split whose players form groups further apart than a larger split distance.
- A merge and a split are one log record of the store each. No region just disappears:
  a region ends by being absorbed. A region that is split off is run at once by the
  worker that made it, and can be moved from there.
- Workers run several regions. The single process runs the same, with the coordinator's
  decisions made in-process.
- For tests, regions can be pinned to an area, and merging and splitting can be asked
  for by hand.

Changed from what was first agreed, for the owner to know: regions do not carry entity
ids of their own (the home region issues all), an empty region is absorbed rather than
retired, a new region starts on the worker that made it, and the steps are cut
differently, so that each leaves everything working.

| # | Scope | Verified by | Status |
|---|---|---|---|
| C0 | The messages and types of ADR-0010, refused or ignored by everyone | All existing tests | done |
| C1 | Store: the list of regions with those absorbed, grants with their ticks, chunks leaving only saved, replay only into what is held, pinned regions, the merge and the split as one log record each. Designed in [ADR-0011](adr/0011-the-world-store-and-regions.md), in steps C1.1 to C1.7 | Store tests incl. kills at every point of a merge and a split; tests from the record by someone else | done |
| C2a | Several regions per worker; the coordinator without "a worker runs one region" | The move and chaos tests, on stripes, with fewer workers than regions | done |
| C2b | Sim, worker and edge on chunk sets: claims, guests, `Elsewhere`, `NotMine`, departures that name a region, `since` in hellos. Designed in [ADR-0012](adr/0012-the-tick-on-chunks.md), in steps C2b.1 to C2b.5 | Hand-over, block, takeover, chaos and move tests on two pinned regions | done |
| C3 | Absorb and split through sim, worker, edge and coordinator, asked for by hand | Differential tests against one region; kills at every step; an edge away during several merges and splits in a row | done ([ADR-0014](adr/0014-merging-and-splitting.md), [ADR-0015](adr/0015-the-edge-through-merges-and-splits.md)) |
| C4 | The coordinator decides by itself | State-machine tests with scripted and random movement; no flapping | done ([ADR-0016](adr/0016-when-to-merge-and-split.md)); off unless asked for until C5 |
| C5 | Stripes, `Layout` and `--boundaries` go; the single process and the cluster on the new model by default | Bots meeting and parting; crowds; every chaos and move test again; kind | done ([ADR-0017](adr/0017-the-end-of-the-stripes.md); the crowd's pause in [ADR-0018](adr/0018-a-checkpoints-chunks-written-together.md)) |
| C6 | Docs; what to try with real clients | CI | done: README, architecture, and the trial under "C5 is done" below |

Then the owner's check: two clients walking towards and away from each other.

### Where M3 stands

**In one paragraph, on 2026-10-10.** All three phases are built and pushed: a worker
or the store can die (A), a region is moved without anyone noticing more than a pause
(B), and regions follow their players, merging and splitting by themselves, with no
stripes left (C). Nothing that the tests found is open. What is left is the owner's
trial with two clients, which is written down under "C5 is done" below, steps 1 to
12. What follows is the
history of each phase with what was measured and what was left.

**Phase A is done.** A worker or the world store can die without anyone being
disconnected or losing anything they were shown.

How it works is in [ADR-0008](adr/0008-durable-regions-and-resuming.md), which was gone
over by an independent reviewer before anything was built and corrected as the building
and the tests found more; the list is at its end. In short:

- The world store answers a commit once it is on disk, keeps each region's state beside
  its chunks and hands both to whoever opens the region, and lets only the latest owner
  commit.
- A worker restores its region from the store, shows nothing of a tick before the tick
  is committed, and answers an edge's hello with what the edge missed. When it loses
  the store it stops the region and opens it again once the store is back.
- The edge keeps a region's players when its link ends, links to whoever runs the
  region then and resumes; a player whose region stays silent for 20 seconds is
  disconnected. In a cluster it follows the coordinator's routing table and never
  starts over.
- The coordinator gives a region to a waiting worker when its owner has not vouched for
  it for the lease of 5 seconds.

What checks it:

- Tests of the sim and of the worker written from the record alone by someone who had
  not seen the code (`crates/clustine-sim/tests/specification.rs`,
  `services/worker/tests/specification.rs`); the store killed at every write and sync
  (`services/worldstore/src/kill.rs`); the edge against scripted regions
  (`services/edge/src/fanout.rs`).
- `bin/clustine/tests/takeover.rs`: regions taken over in the single process under bots.
- `bin/clustine/tests/chaos.rs`: a cluster of processes with workers and the world store
  killed from a seeded sequence under bots that keep a ledger of everything
  acknowledged (`CLUSTINE_CHAOS_SEED`, printed with every run; `CLUSTINE_CHAOS_KILLS`
  for a soak).
- `deploy/kind/test.sh`: the same bots on Kubernetes while the pods of the workers and
  of the world store are deleted.

**For the owner to try with real clients**, whenever there is time:

1. A cluster as processes, each in a terminal of its own:
   ```bash
   cargo build -p clustine
   target/debug/clustine coordinator --reshape by-hand
   target/debug/clustine worldstore --world world --pin 4
   target/debug/clustine worker --name a --listen 127.0.0.1:25611
   target/debug/clustine worker --name b --listen 127.0.0.1:25612
   target/debug/clustine worker --name c --listen 127.0.0.1:25613
   target/debug/clustine edge
   ```
   Join with one or two clients at `localhost:25565`. The regions meet at block x = 64.
   The workers share the two regions: the first to be there may be given both, and
   one of them is moved to another worker a moment later, which the coordinator logs.
   Two workers are enough; with three one has nothing to do until another goes.
2. Stand in one region, build something, and kill the worker that runs it with
   `kill -9` (the coordinator's log says which worker has which region). Expected:
   everyone in that region stands still for five to seven seconds, the blocks of the
   last moments are all there, nobody is disconnected, and then everything goes on;
   what was done while standing still takes effect. A second client in the other region
   notices nothing but the first one standing still.
3. Start that worker again (it is the spare now) and kill the other one. Walk across
   x = 64 while a worker is being killed.
4. Kill the world store and start it again within twenty seconds. Expected: everyone
   stands still while it is away and nobody is disconnected; away for longer, players
   are disconnected with "The server fell too far behind", and can join again.
5. With one client, break and place blocks quickly while killing the worker: no block
   may come back or vanish afterwards, also not after leaving and joining again.

Anything else than expected is a defect of phase A; the seed-driven tests above are
where to reproduce it.

**One failure is unexplained.** On the machine phase A was finished on, the hand-over
test `a_watcher_sees_one_entity_cross_the_boundary` failed once: the watcher saw the
walker's entity removed and shown again. It did not fail again in 54 runs under load,
and no path in the code was found that hides an entity whose position stays in view.
That machine had defective memory, found the same day (see `CLAUDE.md`), which flips
single bits, and a flipped bit in a position does exactly this; but that is not proof.
If this test ever fails on GitHub's machines, it is a race in the hand-over as phase A
changed it, and has to be found.

Not covered by tests yet:

- The worker's path for an epoch the store refuses, between processes (the worker hears
  from the coordinator first in every scenario tried).
- `Superseded` over a link between processes; it needs a second edge with the same
  name.
- Failed writes and syncs between processes; the store's own tests inject them.
- Commit latency on a real disk during a large checkpoint; the store's tests show that
  commits do not wait for saves, not how long they take.

Left for a later cleanup: `Assignment::entity_ids`, which the coordinator fills in and
nothing reads; and the `WorkerToEdge` messages `Remote`, `RemoteDone` and the
`Departed` and `Refused` player events, which regions say through their outbox now.

**Phase B is done.** A region can be moved to another worker on purpose, and a worker
that is told to stop hands its region over first. Players of the region stand still for
most of a second and notice nothing else.

How it works is in [ADR-0009](adr/0009-moving-a-region.md): a move is a crash that the
old owner prepares and announces. It checkpoints while it still ticks, stops, waits
until the store has everything, lets go and says so; the coordinator gives the region
to a worker it had reserved, at once; that worker restores the region and the edge
resumes with it, as after a crash. An independent reviewer went over the record against
the code before it was built and found eleven defects; the tests found two more, listed
at its end.

What checks it, besides the coordinator's and the worker's own tests:

- `bin/clustine/tests/moves.rs`, written from the record by someone who wrote none of
  the code: a cluster of processes under the ledger bots at view distance 8, bots 320
  blocks apart. Regions are moved one after the other with the pause measured at the
  bots; moved while a bot crosses the boundary and while the new owner still opens the
  region; refused for every reason the record names; with the old owner killed or
  frozen, the new owner killed, the world store killed and the coordinator killed in
  the middle; a worker told to stop with and without a spare; every worker replaced one
  after the other. `CLUSTINE_MOVES_SOAK` adds the worker that gives up after 20 seconds.
- `deploy/kind/test.sh`: `kubectl rollout restart statefulset/clustine-worker` under the
  ledger bots; no lease may run out.

What it measured, with unoptimised builds on one machine:

- **The pause at a move**, as the longest any bot waited for an acknowledgement:
  between 0.74 and 1.27 seconds over 80 moves, 0.85 in the middle. Bots in other
  regions wait as long as when nothing happens, about a tenth of a second.
- Where it goes: the last checkpoint 4 to 126 ms, the assignment 1 ms, restoring 5 to
  46 ms, linking up to 100 ms (the edge retried every 100 ms then, and every 20 ms
  now), and **the resume about 0.75 seconds**: the new owner loads and sends again
  every chunk the region's players see before it takes what they did.
- `clustine move` itself takes about 90 ms, most of it the first checkpoint, during
  which the region still ticks.
- Replacing every worker takes 1.6 to 1.8 seconds for three, each exiting within half a
  second of being told; the longest wait of a bot was 1.3 to 1.5 seconds, as a region
  can move twice in a row.
- A move in which the old or the new owner dies ends like a crash, 5 seconds later.

The record says that the resume is to hold only what acts on chunks not there yet if
the pause is above about a second. It is at that mark with unoptimised builds, so this
is not done yet; phase C changes what a resume sends, and it is decided there (C3) with
a measurement of an optimised build.

Two faults the tests found, both fixed, neither particular to moves: the edge found a
new coordinator only by chance while it was trying to link to a region, so players
stood still for up to 18 seconds and were disconnected once; and a region released
while the coordinator was away waited out the new coordinator's grace period.

Seen and left as it is:

- For a few milliseconds after a release the old owner still welcomes links, and the
  edge links to it up to six times before it finds the new one. No harm was seen.
- When the target of a move is told to stop in the middle of it, the region is without
  an owner for about a second, until the old owner is given it again.
- Not tried: a release that goes unanswered while its owner still heartbeats (nothing
  outside the process can produce it), optimised builds, more than two regions.

**For the owner to try with real clients**, with the cluster of processes above (three
workers for two regions, so one is to spare):

1. Stand in one region and build. In another terminal:
   ```bash
   target/debug/clustine move --region 1
   ```
   (`--region 0` is west of x = 64; `--to c` names the worker.) Expected: the command
   says who has the region now and that it was released by its owner; everyone in the
   region stands still for about a second, with a client that sees far perhaps longer,
   and then everything goes on. Nobody is disconnected and nothing is lost. Do it
   again and again, while walking, building, and crossing x = 64.
2. Stop the worker that runs your region with Ctrl-C (once). Expected: the same short
   pause, the worker exits within a second, and the spare runs the region. Start it
   again and stop the next one. With no spare running, Ctrl-C makes the worker wait
   20 seconds for one before it stops; a second Ctrl-C stops it at once, and the region
   stands still until a worker is started.
3. Stop the coordinator and play: nothing changes, and `clustine move` cannot reach
   it. Start it again: within a few seconds moves work again.

What to say if it is not so: which step, what was seen, and the logs of the terminals.

**The owner's first try, 2026-10-08**: after a worker was killed, joining again did not
work; the client was told "The server fell too far behind", which the edge says when a
region has not placed a player, or not confirmed what they did, for 20 seconds. It
could not be reproduced with bots, also not with one that keeps moving, leaves and
joins again under its name at the default settings. Looking for it found that the edge
took what a region said of a player who had left and joined again since for the player
as they are now, and put them into the world as their old self; that is fixed
(`what_a_region_says_of_a_player_who_left_and_came_back_since_is_not_taken_for_them`).
The owner tried again and could join. The bots now fail when they are shown their own
player as another entity or when their own entity is removed, which is what a client
would have met here and what the bots used to pass over; with that the test of joining
again while the region has no worker fails without the fix.

Phase C has begun. [ADR-0010](adr/0010-regions-that-follow-players.md) is its plan,
accepted after an independent review that found 21 defects. C0 is done: what the record
adds is in `clustine-rpc` and `clustine-sim`, and everyone passes over it or refuses it.
`RegionId` is in `clustine-world` now, so that the sim can name regions.

C2a is done as well: a worker process runs several regions, each on a thread of its
own, and the coordinator gives a region to the worker with the fewest, evens regions
out one release at a time, and passes over a worker that just failed a region
(ADR-0009, section 7). A cluster no longer needs a worker to spare: the regions of a
worker that dies or leaves go to the others. `chaos.rs` has two workers running three
regions with one of them killed again and again; `moves.rs` has a single worker that
runs everything and hands it to one that arrives.

C2b has its design in [ADR-0012](adr/0012-the-tick-on-chunks.md), written from the
code of the sim and the runner and reviewed like ADR-0011 (sixteen defects, worked
in). Of its steps C2b.1 is built: a region's state for an edge says since when the
region knows the edge, the edge says that number in its hellos and is resumed only if
it is the region's, a welcome says how many outbox entries follow it, and what a worker
stores of a region begins with a number for its form, so that what an earlier build
stored is dropped instead of being read as something else.

C1 is built as [ADR-0011](adr/0011-the-world-store-and-regions.md) has it, in its seven
steps: the store is told how the world is divided when it starts (`clustine worldstore
--boundaries` then, the same as the coordinator; `--pin` since C5), keeps a table of
regions and of which region holds which chunk, lets only the holder load and save a
chunk, grants and takes back chunks, replays a region's log only into what it holds,
merges and splits regions as one record of the log each, and gives the coordinator the
list of regions. After a failed write of the log every region loses its owner and nobody
is served until the log is cut back durably. It is killed at every write and sync of
claims, returns, a merge and a split (`kill_regions.rs`), which found a fault in the
chunk store older than this work: a section file left behind by two faults in a row was
taken for stored. Nothing asks the store for any of the new things yet: the stripes are
pinned regions and work as before. What the builder decided where the record was silent
is at the record's end. The scenarios of its section 9 were then written by someone else
from the record alone (`services/worldstore/src/scenarios.rs`, 79 tests, the thread for
chunks held at chosen points and the store killed there): they found nothing, and catch
each of ten faults that were put into the store to see whether they can.

C2b.2 is built: the sim has no area any more. A region knows of each chunk whether it
holds it, has asked for it, believes another region to hold it, or knows nothing; it
claims what its viewers and players need, gives back what nobody has used for a while,
lets a player go to the region the store names, and names the region an action or a
player goes to in its outbox entries. Until the runner asks the store (C2b.3) the
processes tell each region the stripes as given (`presumed`), so nothing changes for a
cluster yet. Its scenarios are being written by someone else from the record.

C2b.3 is built: the region runner passes a tick's claims and returns to the store and
the answers back into ticks, keeps subscriptions of viewers and of guests with their
numbers, answers each with a snapshot, `Elsewhere` or `NotMine`, and holds a link that
has said hello until every chunk it named is answered. `clustine worker
--ask-the-store` leaves the given stripes out, so that regions really ask the store
which chunks they hold; every end-to-end test passes that way too, with the edge as it
is, and a fifth check runs them so (`CLUSTINE_TEST_ASK_THE_STORE`). The processes still
start with the stripes given until C2b.5.

C2b.4 is built as [ADR-0013](adr/0013-the-edge-without-a-layout.md) has it, which was
reviewed like the others (nineteen defects, two of them ordering mistakes of the kind
this part has had before): the edge no longer knows how the world is divided. It asks
a player's region for everything the player sees and other regions as a guest where a
region names them, takes answers by their number, sends a player or an action on
blocks where a region says, and is handed the home region and nothing else. Every
end-to-end test passes with regions that take their stripes as given and with regions
that ask the store; the pause at a move is 0.8 seconds in the middle, as before. The
scenarios of ADR-0013 were then written by someone who did not read the edge's code: 59
tests, among them runs generated against a second implementation of the record. They
found a contradiction in the record itself, which the edge had followed (a subscription
ended while another region's pointed at it, which only regions that merge and split can
bring about), and an entity that stayed on screens when its player left in the very
tick they were handed on; both are put right.

C2b.5 is built, and C2b with it: the given stripes are gone, and with them `clustine
worker --ask-the-store`, the fifth check and its variable. A region knows of a chunk
only what the store has told it, in a cluster and in the single process, which until now
always started with the stripes given: `cargo run -p clustine -- --boundaries 4` (as it
was then) runs regions that ask for the first time. The worker's unit tests that ran a
western area on a world of one region run on a world divided at 1. Measured in turn with
one seed, the pause at a move is 0.95 seconds in the middle with regions that ask and
0.90 with the stripes given (both within one hour; the 0.8 above was measured at another
time, on a machine less busy): asking costs about a tick there, and taking the given
stripes out changed nothing.

C3 has its design in [ADR-0014](adr/0014-merging-and-splitting.md), reviewed against
the code like the others (fourteen defects). What the review changed most: a merge and
a split close the region's links, as ADR-0010 had it, so that an edge meets what they
did in one place only, among the entries of a welcome; at every hello a region says
every player it has for the edge, and the edge ends those it does not know; and what
an edge says about a player names the stay it means, by its entity, so that what an
earlier stay sent can never be taken for a later one. The pause at a move, which the
record for phase B left to be judged here with an optimised build, is 0.36 to 0.40
seconds in the middle (0.9 unoptimised), most of it the move itself, so the resume is
left as it is. The edge's part is
[ADR-0015](adr/0015-the-edge-through-merges-and-splits.md), written by whoever builds
it and reviewed in turn: eleven defects, three of which would have disconnected a
player who had done nothing wrong, each a sequence in which what one region said was
read late against what another had said since. Their fixes changed nine rules of
ADR-0014's contract with the edge and added one number to a welcome.

C3 is built, in the eight steps of ADR-0014: regions merge and split, asked for by
hand with `clustine merge` and `clustine split`. A merge and a split are one tick of a
region and one record of the store's log; the region's links are closed at that tick
and every edge learns what happened from its next welcome. The absorbed region is
released first, like a region that is moved; the new region of a split is run at once,
from memory, by the worker that split it, and is evened out a lease later. The
coordinator reads the store's list of regions (`clustine coordinator --store`, which a
local cluster needs no flag for), reserves the regions of a merge or a split until the
list shows what came of it, and tells whoever asked.

What checks it: each part has tests written from the record by someone who did not
read its code (the simulation 103, the region runner 88, the coordinator 164, the edge
66 besides the 59 of C2b.4, among them generated runs against a second implementation
of the record in which regions merge and split while links are lost); the store has
the kills at every write of a merge and a split from C1;
`bin/clustine/tests/reshapes.rs` has the commands between processes; and
`bin/clustine/tests/merges.rs` has a cluster of processes under the ledger bots:
regions merged and split as bots cross the line, twenty times in a row, a part moved,
split again and merged back; a worker, the world store or the coordinator killed at
logged moments of a merge and a split (`CLUSTINE_CHAOS_SEED`, `CLUSTINE_CHAOS_KILLS`);
the edge stood still across several merges and splits; a player who leaves and joins
again in the middle, who has to be one player with one entity afterwards. Every test
audits the world against the bots' ledger, most of them again after every process was
killed and started from disk.

What the tests written by others found, all put right: the coordinator named the wrong
reason when a reading of the list took a region away during a merge; the edge let two
regions name each other for a chunk for ever, ignored the first steps of a player whose
stay had moved, and sent a moved player's old inputs on; and, under the bots, the part
of a split was run by nobody when the world store died between making the split and
saying so, because the coordinator read the list once, while the store was away, and
never again. That one disconnected the part's players after twenty seconds and came up
once in eleven runs that killed the store during splits.

**The pause at a merge and at a split**, as the longest a bot waited for an
acknowledgement, in the middle / at worst, bots far apart, one run each:

| | Players who stay (the survivor's; those not split off) | Players who go (the absorbed region's; the part's) | The command, by its own account |
|---|---|---|---|
| Merge, optimised | 0.19 / 0.22 s | 0.37 / 0.41 s | 0.20 / 0.22 s |
| Split, optimised | 0.14 / 0.20 s | 0.16 / 0.17 s | 0.09 / 0.13 s |
| Move, optimised, to compare | – | 0.27 / 0.31 s | 0.10 / 0.12 s |
| Merge, unoptimised (50) | 1.10 / 1.45 s | 1.28 / 1.65 s | 0.22 / 0.54 s |
| Split, unoptimised (50) | 0.57 / 1.19 s | 1.04 / 1.23 s | 0.16 / 0.50 s |
| Move, unoptimised (50) | – | 1.48 / 1.70 s | 0.13 / 0.47 s |

A split and at once the merge back is 0.43 s optimised (0.75 unoptimised). Undisturbed
a bot waits 0.05 s (0.06 to 0.12). After a worker was killed in the middle every region
ran again within 3.7 s with a lease of 3 s, and the merge or the split was either made
whole or not at all.

**For the owner to try with real clients**, with the cluster of processes above (two
workers are enough) and two clients:

1. One client stays where it entered; the other walks east past x = 64, into region 1.
   Then:
   ```bash
   target/debug/clustine merge --survivor 0 --absorbed 1
   ```
   Expected: `region 0 has absorbed region 1, N ms after asking`. Both stand still for
   about a second, the one who was in region 1 a little longer, and everything goes
   on; nobody is disconnected, each still sees the other, and what was built just
   before is there. Walking across x = 64 afterwards shows nothing. `--survivor 1
   --absorbed 0` is refused: the region players enter in is never absorbed.
2. The far client stands a few chunks from the origin, say at x = 100, z = 8, which is
   chunk 6,0 (block coordinates divided by 16, rounded down; F3 shows it). Then:
   ```bash
   target/debug/clustine split --region 0 --chunks 6,0
   ```
   Expected: `region 2 has been split off region 0, N ms after asking`; both stand
   still for about half a second. The far client is now in region 2, which begins
   about halfway between the two players, and the same worker runs it. About five
   seconds later the coordinator moves one of the two regions to the other worker
   ("a region is moved to even regions out" in its log), another short pause for that
   region's players. A command asked just then is refused with "region N is being
   released"; ask again. Walk towards each other and past, and build across the line.
   A chunk nobody stands in gives `Error: the coordinator reports no split of region
   0: no player stands in a chunk named that the region holds`, and nothing changes;
   the chunk at the origin never goes. (`--chunks` takes several chunks and comes
   last; `--coordinator`, if needed, before it.)
3. `target/debug/clustine move --region 2`, then `target/debug/clustine merge
   --survivor 0 --absorbed 2`. Expected: as in phase B and as in step 1.
4. Over and over while walking, building and crossing; leave and join again right
   after asking (you are back where players enter, once). `kill -9` the worker that
   runs region 0 right after asking for a merge or a split: everyone there stands
   still for five to seven seconds, then the command says it was done or that region
   0 "changed hands before the worker said what came of it", and the coordinator's
   routing table shows which.

What to say if it is not so: which step, what was seen, and the logs of the terminals.
In the single process nothing merged or split when this was written; since C5 it
does, by itself (below).

Seen and left as it is, or not tried:

- After a merge, the first block action of a player who came from the absorbed region
  waits for its chunk to be loaded, and everyone of the same edge in the survivor
  waits behind it for those ticks (ADR-0014, "What a player notices").
- A split asked within a tick or two of a merge finds nobody to split off; asked again
  a tenth of a second later it is made. C4, which asks by itself, takes that as "not
  yet".
- Not tried: a worker told to stop during a merge or a split; two edges; merges and
  splits on Kubernetes beyond the one of `deploy/kind/test.sh`.

**C4 is done: the coordinator can decide by itself when regions merge and when one is
split.** Until C5 it was off unless asked for (`clustine coordinator --reshape
by-itself`), for the reason given below; since C5 it is what a server does unless it
is told `--reshape by-hand`.
[ADR-0016](adr/0016-when-to-merge-and-split.md) is its design, reviewed twice before
anything was built (eleven defects, then six, all worked in).

How it decides, in short. Every worker says four times a second in which chunks its
regions have players. The coordinator joins players into groups by distance: two
regions whose players come within the **merge distance** of each other are merged, and
a group that has gone further than the **split distance** from everybody else of its
region is split off with the chunks around it. Both distances follow from the view
distance (22 and 30 chunks at a view distance of 8; `--merge-distance`,
`--split-distance`), and the gap between them is what keeps a player who stands at the
edge from being merged and split over and over. What else keeps it calm: nothing is
begun on one look (it has to have held for a second), a region that was just merged,
split or moved **rests** for ten seconds (`--rest-seconds`), something that failed is
left alone for half a minute and for longer each time, one split at a time in the
world, and nothing at all while the coordinator does not know where the players of some
region are. A region nobody has been in for half a minute is absorbed by a neighbour
without players, the home region first. After a split the part starts on the worker
that made it and is moved to another when it has rested, if that evens the workers out.

What checks it. The rule itself (`policy::decide`) is a pure function with scripted
cases and generated ones. The state machine has its builder's tests, which thirty
faults put in on purpose each failed, and two sets written from the record by others
who had not seen the code: fifty scripted scenarios (195 tests, every timed rule tried
just before, at and just after its moment) and generated runs of players walking at
random while workers die and readings of the list fail, held to seven properties
(nobody's region is disturbed more often than the rest allows, nothing is begun on
ignorance, whoever should be together is in the end, and so on) over more than six
thousand runs. Neither found a fault in the code. `bin/clustine/tests/follows.rs` then
runs a cluster of processes under the ledger bots: a group walks up to another and
away again ten times in a row, with the regions, the store's list and the
coordinator's log checked after every step; an empty part is absorbed by the right
neighbour; a worker, the coordinator or the store is killed at logged moments of
merges and splits the coordinator began by itself. Ten runs of the whole file passed.

What those tests found: the record said two things of a split that the store made and
whose worker died before it could say so. The coordinator leaves such a region alone
for half a minute, as after a split that failed, because nothing but the worker's word
says that a split was made; the record now says so too. For that half minute nothing is
merged into the region, and players on both sides see each other across the boundary
as they did before C4.

**What a group that walks to another and back goes through**, measured in those ten
runs (110 merges, 100 splits, 110 moves; unoptimised processes, distances of 3 and 5
chunks and a rest of 5 s so that a test can walk them; least / middle / worst):

| | Begun after the group set out | Those who stayed stood still | Those who went stood still |
|---|---|---|---|
| Merge, walking up | 9.3 / 10.0 / 10.4 s | 0.34 / 0.51 / 1.11 s | 0.49 / 0.73 / 2.11 s |
| Split, walking away | 7.4 / 8.0 / 9.0 s | 0.33 / 0.51 / 1.10 s | 0.24 / 0.41 / 0.71 s |
| The part moved to the other worker, a rest after the split | 5.0 / 5.2 / 5.2 s | 0.03 / 0.07 / 0.19 s | 0.24 / 0.38 / 0.66 s |

So a group that leaves another stands still twice within five seconds, once for the
split and once when its new region is moved, each time for about half a second
unoptimised (0.15 to 0.3 s optimised, by C3's table above); those it left notice the
first only. On the way back the merge waited for the rest after that move, during
which the two groups saw each other across the boundary for two to four seconds. After
a worker was killed and not started again, every region ran again within 3.9 s with a
lease of 3 s.

**Why it was not the default until C5.** On stripes a part can only be cut out of what its
region holds, so a group that walks on leaves its part, falls back into the region it
was split from, and is split off again where it stands: a stop every ten seconds for a
group that travels (K15 in the record). C5 took the stripes away, lets a part grow
with its players and made `by-itself` the default; the trial with two clients walking
towards and away from each other is written down under C5 below. Where regions follow
their players, a merge or a split asked for by hand is undone again after a rest
where the distances say otherwise.

Seen and left as it is, or not tried, in C4:

- A split whose worker died counts as failed whether or not the store made it (above).
- A lone player six chunks from where players enter is split off the home region when
  the last player near the spawn point leaves it. That is the rule as written.
- A player who comes into a region at the moment it is absorbed for being empty stands
  still for that absorption and can be disturbed again sooner than a rest after it.
- Not tried: the distances of a real view distance (22 and 30) under bots; more than
  two workers; two edges; a worker told to stop while the coordinator decides by
  itself; many empty regions at once; real clients.

What C0 left to the steps that use it, because it changes what exists instead of adding
to it: `Departed` and `Remote` naming the region they go to, `since` in an `EdgeState`
and in hellos, the welcome saying how many entries follow (all C2b); how `Restored::held`
and the list of regions travel between the store and others (C1).

**C5 is done: the stripes are gone, and regions follow their players unless a server
is told otherwise.**
[ADR-0017](adr/0017-the-end-of-the-stripes.md) is its design, reviewed twice before
anything was built and corrected as the building found more (its last section).

How it is now, in short:

- A new world is one region, the home region, which holds what its players see and
  nothing else. A chunk goes to the region whose player sees it first, and a region
  gives a chunk back half a minute after the last of its players stopped seeing it.
  Nothing is divided beforehand: `Layout`, the stripes and `--boundaries` are gone
  from every process, and `--boundaries` is refused with a sentence that says what to
  write instead.
- The coordinator knows no region until it has read the store's list. It decides by
  itself when regions merge and split (C4) unless it is started with `--reshape
  by-hand`. The single process does the same, with the same code.
- A group that is split off takes the land on its side of the split with it, and what
  lies ahead of it is its own to claim. So it walks on, however far, without being
  handed back and split off again, which is what the stripes could not do.
- `--pin 4` on the store, or on the single process, pins regions side by side: for
  tests, and for whoever wants a boundary at a known place.
- A world directory that was served with `--boundaries` is opened as it is and made
  over; what was built in it is there.
- A worker logs for every merge and split how long its region did not tick: `a region
  stood still for a merge or a split region=… players=… held=… milliseconds=…`.

What checks it. Every step came with tests of its own in the store, the coordinator,
the runner and the single process. Then, written from the record by someone who had
not seen the code (step C5.8), and run on a world without pins under the ledger bots,
who count every block they place and dig and every answer they wait for:

- `bin/clustine/tests/wanders.rs`, in a cluster of processes and in the single
  process. A group is split off, walks on and is merged when it comes back, round
  after round, with the store's list, the routing table and the logs checked after
  every step; the same while players keep joining at the spawn point; a group that
  walks along the rim between the two distances; regions that are left give their land
  back and are absorbed; two parts whose players meet are merged without the home
  region; a worker, the coordinator or the store killed at logged moments of a merge
  or a split; a part that grows on while the store is away; eleven lone players who
  come to a region each; one and eight who go straight on, on foot and at a sprint,
  split off once and never handed over.
- `chaos.rs` and `moves.rs` once more, on a world that follows its players, beside
  their runs on a pinned one.
- `crowds.rs`: a crowd at the spawn point while two leave it and come back (below).
- `deploy/kind/test.sh`: on Kubernetes a group of bots walks out, is split off by the
  coordinator, moved to the other worker and merged again; the bots waited 0.08 to
  0.31 s.

Ten runs of `wanders.rs` in a row: nine passed, and the tenth failed for a fault of
the test (a bot walked through the spot where another placed a block, which the server
rightly refuses), since mended. Three runs each of `chaos.rs`, `moves.rs` and
`crowds.rs` passed. These tests add about three quarters of an hour to
`cargo test --workspace` on six processors.

What those tests found:

- **The edge used a whole processor after about every merge**, in the cluster and in
  the single process, until it was stopped: the task that keeps its links kept waking
  for a region that had been absorbed. Nobody was disconnected and nothing was lost,
  which is why no earlier test saw it; it showed as two edges at 100 % long after
  their tests had ended. Mended, and a test now holds an edge to being idle after a
  merge.
- **A player who got ahead of their own view was left without one, for good.** Three
  times in twenty-six runs of eleven bots who ran to their lanes at 160 blocks a
  second, a region held only the chunk its player stood in, and once a bot did not
  get even that chunk; never at a slower pace. The cause: an edge asks for what a
  player sees by where the region says they are, and a region told an edge of a
  move only in chunks the edge watched. Whoever outran what their edge had asked
  for, in the few ticks an asking takes, was never reported again. No client goes
  that fast, but a client that flies on while its region waits for a new worker
  (five to seven seconds) gets further than a short view reaches, so it was a fault
  a player could meet. Mended: a region tells an edge where its own players moved
  to wherever that is. Found by having the runner and the edge say once a second
  what they held and asked for, and running the test until it failed; the test is
  no longer ignored.
- **A crowd of a hundred stood still for about a second**, twice the half second the
  record set as what a player bears, and a crowd of two hundred could not be merged
  into at all. The time went into the store's way of writing, not into the edge.
  Mended in step C5.10; see the tables below.
- **A player whom the edge drops for not keeping up left no reason in its log**: the
  line was written only for whoever asked for debugging. It is a warning now.
- Three things the record said that are not so, and now are said as they are: if
  more players leave than stay, it is those who stay whose region is moved to the
  other worker, so they stand still twice in five to ten seconds; a group that turns
  round at once after its split walks into land the home region still holds and is
  handed over into it, leaving an empty region behind; and the home region keeps the
  trail of a group that came back for half a minute, so a second split takes more land
  along than the first.

**What a group that walks out and back goes through**, in those ten runs (60 of each;
unoptimised processes, a view distance of 2, so distances of 10 and 18 chunks, and a
rest of 5 s; least / middle / worst):

| | Those who stayed waited | Those who went waited | The home region did not tick |
|---|---|---|---|
| Split, 18 chunks out | 0.20 / 0.32 / 0.65 s | 0.19 / 0.31 / 0.67 s | 0.05 / 0.14 / 0.36 s |
| The part moved to the other worker, a rest later | 0.03 / 0.08 / 0.18 s | 0.16 / 0.29 / 0.49 s | not at all |
| Merge, coming back within 10 chunks | 0.18 / 0.29 / 0.53 s | 0.46 / 0.65 / 1.01 s | 0.05 / 0.09 / 0.33 s |

A split or a merge was begun about two seconds after the group crossed the distance.
While the group walked on, nobody was handed over and nothing was split again, in
every one of 60 counted rounds. After a worker was killed, every region ran again
within 2.4 to 3.3 s (a lease of 3 s). Eleven lone players each had a region of their
own 4.5 to 11.3 s after the last had arrived; they are split off one at a time, a
rest apart, so the last of them stood still up to nine times.

**What a crowd at the spawn point goes through when two players leave it and come
back**, optimised, at the usual view distance of 8 (distances of 22 and 30 chunks),
five rounds, the bots of the crowd each placing and digging without a pause (least /
middle / worst). First as step C5.8 measured it, before step C5.10:

| Crowd | Chunks of the home region | Split: the crowd waited | Split: the region did not tick | Merge: the crowd waited | Merge: the region did not tick |
|---|---|---|---|---|---|
| 4 | 935 | 0.17 / 0.18 / 0.24 s | 0.05 / 0.05 / 0.08 s | 0.16 / 0.19 / 0.21 s | 0.05 / 0.05 / 0.05 s |
| 20 | 1007 | 0.21 / 0.41 / 0.41 s | 0.05 / 0.21 / 0.21 s | 0.25 / 0.36 / 0.40 s | 0.05 / 0.17 / 0.18 s |
| 50 | 1159 | 0.30 / 0.35 / 0.67 s | 0.09 / 0.12 / 0.46 s | 0.30 / 0.57 / 0.61 s | 0.08 / 0.35 / 0.38 s |
| 100 | 1404 | 0.40 / 1.03 / 1.07 s | 0.17 / 0.78 / 0.81 s | 0.40 / 0.97 / 0.97 s | 0.17 / 0.70 / 0.72 s |
| 200 | 1917 | one split: 1.44 s without a tick | | no merge was ever made | 1.5 to 1.6 s at each attempt |

So with a handful of players a merge or a split is a fifth of a second for everybody,
of which the region itself stands for a twentieth; that is what the owner's trial
below meets. With a hundred it is a second. With two hundred the two who come back
are never merged: the merge takes longer than the coordinator waits for it, and each
attempt stands the crowd still for a second and a half; attempts come 15, 30 and 60 s
apart. The same happens with a hundred spread over twice the land.

**What was done about the crowd's pause (step C5.10,
[ADR-0018](adr/0018-a-checkpoints-chunks-written-together.md)).** ADR-0017 expected
the time to go into the edge finding its way back to the region, and named keeping
the links as the remedy. The measurement said otherwise: of the second, the edge's
way back was a quarter; the rest was the region waiting for the world store to write
its last checkpoint. The store made every changed chunk durable by itself, with
three syncs to disk each, one after the other, and a hundred bots who all build
leave a hundred changed chunks at every checkpoint. A hundred small files synced in
turn took 1.4 s on this machine; with the syncs waiting at the same time, 0.03 s. So
the store now notes a saved chunk and writes all that were saved together, when they
are made durable: the sections, then the manifests, each round's files synced at the
same time. Nothing of the format changed. The design was reviewed against the code
before it was built (ten findings, one of which would have lost chunks after a
failed write), and its tests were written from the record by someone who did not
see the change: the store stopped at every step of such a write, and with any part
of a round synced, leaves every chunk as it was or as it was saved.

The same measurement after it:

| Crowd | Chunks of the home region | Split: the crowd waited | Split: the region did not tick | Merge: the crowd waited | Merge: the region did not tick |
|---|---|---|---|---|---|
| 4 | 935 | 0.17 / 0.19 / 0.30 s | 0.05 / 0.05 / 0.05 s | 0.19 / 0.21 / 0.21 s | 0.05 / 0.05 / 0.05 s |
| 20 | 1007 | 0.21 / 0.25 / 0.29 s | 0.05 / 0.05 / 0.05 s | 0.21 / 0.21 / 0.25 s | 0.05 / 0.05 / 0.05 s |
| 50 | 1159 | 0.25 / 0.26 / 0.32 s | 0.05 / 0.05 / 0.05 s | 0.21 / 0.26 / 0.33 s | 0.05 / 0.05 / 0.05 s |
| 100 | 1406 | 0.31 / 0.36 / 0.41 s | 0.09 / 0.14 / 0.15 s | 0.35 / 0.36 / 0.36 s | 0.10 / 0.13 / 0.14 s |
| 200 | 1919 | 0.24 / 0.26 / 0.35 s | 0.05 / 0.05 / 0.05 s | 0.26 / 0.26 / 0.55 s | 0.05 / 0.05 / 0.05 s |
| 100 on lanes four times as far apart | 2831 | 0.26 / 0.30 / 0.35 s | 0.13 / 0.13 / 0.14 s | 0.26 / 0.26 / 0.35 s | 0.11 / 0.12 / 0.13 s |

A hundred are within what ADR-0017 set (half a second in the middle, a second at
worst), and so are two hundred, for whom every merge is made now (two runs of five
rounds; the row has both). What is left of a pause is mostly the edge's way back,
about a fifth of a second whatever the crowd. In the first run with two hundred the
edge dropped one bot while all two hundred joined within a moment, before anything
merged or split; step C5.8 saw the same once. All bots and all servers share six
processors there, so it is taken for the test's load, and the edge now says in its
log when it drops somebody. Workers that checkpoint every second or two (the default
is every five minutes) did not keep up with a crowd of a hundred before this step;
that was not measured again.

**For the owner to try with real clients** (two clients, creative mode; flying is a
double tap on the jump key; F3 shows the block and the chunk). An optimised build,
because the pauses of an unoptimised one are three to six times as long, and a new
world directory, so that the regions have the numbers below. The directory served
until now (`world`, with `--boundaries 4`) can be opened as well: it is made over,
what was built in it is there, the log says `the world was divided otherwise before;
what its regions had is in the stored chunks now`, and its parts are numbered from
where that world's numbers had got to. Nobody can see on a screen what a region
holds, so every expectation is a line of the log or something both of you see. A
pause of a fifth of a second is not something a lone player sees on their own screen;
two who look at each other see the other's figure stop for that long.

```bash
cargo run --release -p clustine -- --world trial
```

1. Join with both. **Expected in the log** before you join: `reshaping by itself:
   regions merge and split by where their players are merge_distance=22
   split_distance=30`, `a region was assigned region=0 worker=local`, `running a
   region region=0`.
2. One stays at the spawn point, the other flies east. **Expected**: no line until
   the one who flies is past x = 496. Within two seconds of that: `a split is begun
   by itself region=0 part=1`, then `a worker says what came of a split` with
   `outcome=Ok(RegionId(1))` and `a region stood still for a merge or a split
   region=0 players=2` with `milliseconds` of about 50. On the screens: nothing you
   should notice.
3. The one who flies goes on east for a minute or more, sprinting in the air.
   **Expected**: no line with `split`, `merge` or `departed` in it, however far and
   however fast. This is what C5 is for; on stripes a group was split off again every
   ten chunks.
4. Fly back. **Expected**: within two seconds of coming west of x = 368, if ten
   seconds have passed since the split: `a merge is begun by the distances survivor=0
   absorbed=1`, `a merge has ended survivor=0 absorbed=1` with `outcome=Ok`, and `a
   region stood still for a merge or a split region=0 players=1`. You are 350 blocks
   apart and cannot see each other.
5. Fly out again and to and fro between x = 368 and x = 496. **Expected**: `a split
   is begun by itself region=0 part=2` once, past x = 496; then nothing while you stay
   east of x = 368; a merge with `absorbed=2` when you come west of it, not sooner
   than ten seconds after the split.
6. Both fly out past x = 496, within a few chunks of each other. **Expected**: one
   line `a split is begun by itself region=0 part=3` for the two of you, and none
   after it whatever you do out there. Each sees the other and what the other builds,
   as at the spawn point.
7. One of you leaves the game out there and joins again. **Expected**: they are at
   the spawn point; the other sees their figure go and nothing else; no line with
   `split` or `merge`. Then the one at the spawn point flies out to the other.
   **Expected**: within 352 blocks of the other, `a merge is begun by the distances
   survivor=0 absorbed=3`; and when both are east of x = 496 and ten seconds have
   passed, `a split is begun by itself region=0 part=4`. The one who stayed out there
   stood still twice for a fifth of a second. That is the rule as it is (whoever
   joins is in the home region, which survives every merge), not a fault.
8. Build and break out there and at the spawn point, stop the server with Ctrl-C,
   start it again with the same line. **Expected**: `running a region` for region 0
   and for a part that was there; you join at the spawn point; every block is as you
   left it, out there as well.

To see it sooner, on foot: `cargo run --release -p clustine -- --world trial3
--view-distance 3`. The split then comes past x = 336 and the merge west of x = 208.

A line between two regions that you can stand at is not something the default has:
no player sees another region's land. To build at one and across one, as after C3:
`cargo run --release -p clustine -- --world pinned --pin 4 --reshape by-hand`. The
log has `reshaping by hand: regions merge and split when somebody asks`; the regions
meet at block x = 64; walking across logs `player departed to another region` and
`player arrived from another region`, and nothing may show on either screen.

The cluster of processes, each in a terminal of its own, on a world directory no
other server has open:

```bash
cargo build --release -p clustine
target/release/clustine worldstore --world trial-cluster
target/release/clustine coordinator
target/release/clustine worker --name a --listen 127.0.0.1:25611
target/release/clustine worker --name b --listen 127.0.0.1:25612
target/release/clustine edge
```

9. Steps 1 to 8. The lines about splits and merges are in the coordinator's terminal,
   `a region stood still` and `running a region` in the workers'. **One thing more at
   step 2**: ten seconds after the split, `a region is moved to even regions out
   region=1` in the coordinator's log and `running a region region=1` in the other
   worker's. The one who flew stood still once more, for about a third of a second.
10. After a split, find the worker that runs region 1 (the coordinator's last line
    `the routing table changed` has `region 1 at 127.0.0.1:25611` or `…:25612`) and
    `kill -9` it. **Expected**: for five to seven seconds the one out there is not
    answered: chunks ahead do not come and nobody else sees what they build; then the
    coordinator logs `the lease of a worker ran out` and `a region was assigned
    region=1`, and they go on, with everything they built in those seconds. The one at
    the spawn point notices nothing.
11. Both at the spawn point, in one region. Stop the coordinator with Ctrl-C; one of
    you flies out past x = 496 and stays there. **Expected**: both play on and nothing
    is split. Start the coordinator again: within about a quarter of a minute its log
    has `a split is begun by itself region=0`.
12. One of you at x = 100, z = 8, the other at the spawn point. In a further terminal:
    `target/release/clustine split --region 0 --chunks 6,0`. **Expected**: no error;
    `a worker says what came of a split` with `outcome=Ok`; and ten to twenty seconds
    later `a merge is begun by the distances survivor=0`, as you are within 22 chunks:
    what is asked by hand lasts only where the distances agree.

What to say if it is not so: which step, what was seen, and the logs of the
terminals. The record has more on each step (section 10) and on what a player notices
in ordinary play.

Seen and left as it is, or not tried, in C5:

- A player who leaves the others stands still twice within ten seconds in a cluster:
  for the split, and when their new region is moved to the other worker. Whether a
  part should stay where it was made while the workers are nearly even is a question
  to the owner (the record's open question 2).
- Somebody who joins again and flies back out to a friend stands that friend still
  twice (step 7 above).
- Nothing prints which regions there are; their numbers are in the coordinator's log
  only. A `clustine regions` is a day's work and comes with C6 if the trial is hard to
  read without it.
- The comparisons with the official server were not run for C5.0 to C5.9 when those
  were pushed, as this machine had no Java then. Run on the commit that has the
  tests of C5.8, all six pass.
- Not tried: two edges; more than two workers; real clients.

### After M3, as the owner asked on 2026-10-08

To be planned as milestones of their own, each with a written plan and an independent
review first:

- **Every service with several replicas**, highly available and balanced by load. M3
  leaves one coordinator with nothing on disk and one edge whose death takes its
  players with it (see the limits below); regions that survive their worker and edges
  that survive a region are what this builds on. **A plan is proposed and waits for
  the owner:** [groundwork/replicas-plan.md](groundwork/replicas-plan.md). It rests on
  the groundwork of 2026-10-08
  ([groundwork/replicas-and-availability.md](groundwork/replicas-and-availability.md)),
  which was written before regions followed their players and is checked against the
  code as it is now. An independent reviewer went over the first draft against the
  code and found fourteen things, the gravest that its way of keeping a player from
  being in the world twice did not hold; the plan as it stands answers each. In
  short, five phases: a player's place, look, flying and hotbar are kept by the world
  store, so that whoever joins again is back where they were, and a second login
  puts the first out; several edges, with a planned stop that sends each player on
  to another edge; several coordinators, of which one decides, fenced by a term the
  store gives out; regions moved by load, drains, and numbers to scale by; and last
  the store surviving the loss of its node. Before any of it, every connection
  between services gets a beat, because today nothing notices a peer that has gone
  silent, only one that was killed.

  **Nine questions are the owner's** (the plan's section 8). Two are not acted on
  without an answer: whether a port of the test cluster may be mapped to this
  machine's loopback address so that real clients reach several edges on
  Kubernetes (without it, several edges are tried as processes, and on Kubernetes
  only by bots), and whether a client library for the Kubernetes API may be
  downloaded when the last phase needs one. The others have defaults that can be
  taken back: a standby store of Clustine's own or replicated storage that
  whoever runs the cluster provides (recommended: its own, built last); a shared
  secret between services; the test cluster growing to three nodes; and whether a
  group that walks away stays on the worker it was split off on while the workers
  are nearly even, which would spare it the second pause.
- **Terrain generation**, reusing what SteelMC or Pumpkin have, which ADR-0002 chose
  the AGPL for. **A plan is proposed and waits for the owner:**
  [groundwork/terrain-plan.md](groundwork/terrain-plan.md). It rests on the groundwork
  of 2026-10-08 ([groundwork/terrain-generation.md](groundwork/terrain-generation.md),
  written from web pages) and on its check against clones of both projects, which the
  owner allowed on 2026-10-10
  ([groundwork/terrain-generation-check.md](groundwork/terrain-generation-check.md)).
  An independent reviewer went over the first draft against the code and found
  fourteen things, among them that the draft's reference for terrain was not terrain;
  the plan as it stands answers each. In short: a generator of Clustine's own on
  stable Rust, ported from SteelMC's 26.3 branch with notices and fed by tables made
  from the official jar; generation stays a function of seed and position, with
  features placed in a fixed order that the official server could have taken; what
  is right is judged by the official server itself, run on this machine, and not by
  either project's word; and the owner walks a world the official server made
  before any terrain is generated. About 65 steps in seven phases. Its first steps
  are two trials that decide whether its claims can be tested at all, and nothing
  else is built before them. **The first trial is done and went well**: the
  unmodified official server, given a data pack made from its own biome files with
  their features taken out, leaves chunks at the terrain stage in its region files,
  and their blocks equal SteelMC's fixture in 14 of 14 chunks compared. So terrain
  can be judged by the official server alone. **The second went well too**: the
  server runs inside a Java program of a hundred lines, with no mod loader and no
  change to it, decorates the chunks it is asked for in the order it is asked, and
  for a hundred chunks gives the same blocks as SteelMC's fixture at both stages,
  100 of 100 each. So everything from trees and ores on can be judged by the
  official server as well, and SteelMC's fixture has been shown right where it was
  compared.

  **Ten questions are the owner's** (the plan's section 7 has each with what is
  recommended and what happens without an answer). Those that cannot be undone
  later, and so are not acted on without an answer: whether tables made from the
  jar's code (what light a block gives, which climate is which biome) may be
  committed as Rust generated from its world-generation data may; and whether
  Mojang's buildings (1,511 structure templates, 4 MB) are committed or read from
  the operator's own jar when a server starts (recommended: read, and only their
  sizes committed). The others: what "block for block" means where the official
  server itself depends on the order chunks were made in; whether a chunk that was
  shown is stored from then on, so that a later fix to the generator does not
  change a world that exists (recommended; it ends "only changes are stored");
  whether water that flows and sand that falls belong in the same milestone
  (recommended, as a player meets them within minutes); the Nether and the End as
  worlds of one dimension for now; and the speed to aim for.

**What the owner answered on 2026-10-10**, to the four questions of the two plans that
were not to be acted on without an answer, with which the work on both begins. The
other questions go by what the plans recommend unless the owner says otherwise.

- *Terrain.* Tables made from the jar's code (what light a block gives, its shapes,
  which climate is which biome) may be committed, as Rust generated from its
  world-generation data may, with a notice that they are Mojang's data and not under
  the AGPL. Structure templates are not committed: their sizes and connection points
  are, and the blocks are read from the operator's own jar when a server starts.
- *Replicas.* A port of the test cluster may be mapped to this machine, to its
  loopback address or to the node's own local address, so that real clients reach
  several edges on Kubernetes. A client library for the Kubernetes API may be
  downloaded and used.
- Downloads in general are allowed on the owner's machine where the work needs them
  (`CLAUDE.md`).

**What the owner answered on 2026-10-08**, to the questions that shape the two plans
most (the other questions of both documents are still open and are asked with the
plans):

- *Replicas.* It has to survive the loss of a process at least, and preferably of a
  node. Clustine may use the Kubernetes API (so a Kubernetes lease can elect a
  coordinator; whether replicated storage may be relied on for the store's disk was not
  asked in so many words and is asked with the plan, as surviving a node rests on it or
  on a standby store). A disconnect when an edge dies unplanned is acceptable if the
  player rejoins at once and is back in place. Keeping a player's place and hotbar
  across leaving is part of that milestone.
- *Terrain.* The goal is block for block equal to vanilla for a seed, or as near to it
  as can be. Rust generated from Mojang's world-generation data may be committed, so a
  build needs no jar (this goes beyond ADR-0004's "ids and names only" and gets a
  record of its own with the plan). All dimensions are wanted, and a sensible way to
  generate structures. A first look with Pumpkin as a dependency to be thrown away was
  not asked for, and the plan goes straight to a generator of Clustine's own.

The owner also asked for the work to go on without waiting for them: at each stop of M3
what to try with real clients is written down here, and the next phase begins.

### Known limits after M3

- One coordinator, nothing on disk; while it is down nothing is taken over, moved, merged
  or split. Regions and players carry on.
- One edge; an edge that dies takes its players with it.
- A takeover takes the lease plus a moment; players of that region stand still meanwhile.
- No load-based balancing; no authentication between services.
- A player who places a block across a region boundary and digs that same block within
  about a tenth of a second, having walked into its chunk meanwhile, can find the block
  there afterwards: the dig is applied at once and the placement, which travels between
  regions, arrives after it. Found by the simulation's tests for step C3.
