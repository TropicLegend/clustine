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
| B1 | Release and assign; `clustine move` to ask for it | Move under bot load: nobody disconnected, pause measured and bounded | to do |
| B2 | A worker asked to terminate hands its region over first | Rolling restart of all workers under bots, as processes | to do |
| B3 | ADR-0009, docs | CI | to do |

Then stop for the owner's check: `clustine move` and restarting workers while playing.

### Phase C: regions that follow players

- A region is a set of chunks and the players standing in them. Which region has a chunk
  is decided by the world store, which grants a chunk to the first region that needs it
  and takes it back when that region no longer does. A dead region keeps its chunks
  until it is restored. The stripes, `Layout` and `--boundaries` go.
- An edge asks the viewer's region for a chunk; if another region has it, it is told
  which and asks there. Hand-over and passing on of block actions work as in M2, on these
  chunk sets; an action that reaches a region which no longer has the block comes back as
  "not mine" and is sent on.
- There is always a home region with the spawn chunk; players join there.
- The coordinator hears where each region's players are and lists regions from the store,
  so it finds regions nobody runs, also after its own restart. It orders a merge of
  regions whose players come within a merge distance and a split of a region whose
  players form groups further apart than a larger split distance.
- Merge and split are one operation of the store each: absorbing takes another region's
  stored state, chunks, outbox and message numbers into the survivor and retires the
  other for good; splitting off writes part of a region as a new region with a fresh id.
  Edges are told and move what they kept for the old region to the new one.
- Workers run several regions and start and stop them while running.
- The single process runs the same, with the coordinator's decisions made in-process.

| # | Scope | Verified by | Status |
|---|---|---|---|
| C1 | Store: registry of regions, chunk grants, absorb and split-off as single operations | Store tests incl. kills at every point | to do |
| C2 | Sim, worker, edge on chunk sets instead of stripes: grants, redirects, "not mine"; several regions per worker | Existing hand-over, block and chaos tests on the new model | to do |
| C3 | Absorb and split-off through sim, worker and edge | Differential tests against one region; kills during merge and split | to do |
| C4 | Coordinator: reports, regions from the store, merge and split decisions with hysteresis, placing new regions on the worker with the fewest | State machine tests with scripted and random movement | to do |
| C5 | Stripes removed; single process and cluster on the new model | Bots meeting and parting; crowds; all chaos and migration tests again; kind | to do |
| C6 | ADR-0010, architecture, roadmap | CI | to do |

Then stop for the owner's check: two clients walking towards and away from each other.

### Where M3 stands

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

**For the owner to try with real clients**, whenever there is time; phase B has begun
meanwhile, as asked:

1. A cluster as processes, each in a terminal of its own, with one worker to spare:
   ```bash
   cargo build -p clustine
   target/debug/clustine coordinator --boundaries 4
   target/debug/clustine worldstore --world world
   target/debug/clustine worker --name a --listen 127.0.0.1:25611
   target/debug/clustine worker --name b --listen 127.0.0.1:25612
   target/debug/clustine worker --name c --listen 127.0.0.1:25613
   target/debug/clustine edge
   ```
   Join with one or two clients at `localhost:25565`. The regions meet at block x = 64.
2. Stand in one region, build something, and stop the worker that runs it with Ctrl-C
   or `kill -9` (the coordinator's log says which worker has which region). Expected:
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

Phase B, live migration, has begun. [ADR-0009](adr/0009-moving-a-region.md) is its plan:
a move is a crash that the old owner prepares and announces. An independent reviewer
went over it against the code and found eleven defects, worked in and listed at its
end. Its messages are in `clustine-rpc`; nobody acts on them yet.

Next, in this order:

1. In parallel, each in a crate of its own: the coordinator (`services/coordinator`:
   releases with a reserved target, leaving workers, `Move` and its answers, sections 1
   to 4 of the record) and the runner's release (`services/worker`: the checkpoint while
   ticking, stopping, waiting for commits, publishing, the last checkpoint, closing).
2. The worker process and `clustine move` (`bin/clustine`): release in every phase,
   dropping a region that is taken without exiting, leaving on SIGTERM, listening only
   once registered; and the edge's link-keeper, which has to try each region by itself
   and at once. That is the edge's side and not delegated.
3. Tests by someone who wrote none of it: a move under the ledger bots with the pause
   measured at the bots (view distance 8, between processes, fails above 3 seconds),
   every failure of section 2 of the record, a rolling restart of all workers as
   processes and on kind.

### After M3, as the owner asked on 2026-10-08

To be planned as milestones of their own, each with a written plan and an independent
review first:

- **Every service with several replicas**, highly available and balanced by load. M3
  leaves one coordinator with nothing on disk and one edge whose death takes its players
  with it (see the limits below); regions that survive their worker and edges that
  survive a region are what this builds on.
- **Terrain generation**, reusing what SteelMC or Pumpkin have, which ADR-0002 chose
  the AGPL for; `docs/library-evaluation.md` has what was found about both.

The owner also asked for the work to go on without waiting for them: at each stop of M3
what to try with real clients is written down here, and the next phase begins.

### Known limits after M3

- One coordinator, nothing on disk; while it is down nothing is taken over, moved, merged
  or split. Regions and players carry on.
- One edge; an edge that dies takes its players with it.
- A takeover takes the lease plus a moment; players of that region stand still meanwhile.
- No load-based balancing; no authentication between services.
