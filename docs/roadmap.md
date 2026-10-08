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
| C2b | Sim, worker and edge on chunk sets: claims, guests, `Elsewhere`, `NotMine`, departures that name a region, `since` in hellos. Designed in [ADR-0012](adr/0012-the-tick-on-chunks.md), in steps C2b.1 to C2b.5 | Hand-over, block, takeover, chaos and move tests on two pinned regions | designed and reviewed; to build |
| C3 | Absorb and split through sim, worker, edge and coordinator, asked for by hand | Differential tests against one region; kills at every step; an edge away during several merges and splits in a row | to do |
| C4 | The coordinator decides by itself | State-machine tests with scripted and random movement; no flapping | to do |
| C5 | Stripes, `Layout` and `--boundaries` go; the single process and the cluster on the new model by default | Bots meeting and parting; crowds; every chaos and move test again; kind | to do |
| C6 | Docs; what to try with real clients | CI | to do |

Then the owner's check: two clients walking towards and away from each other.

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

**For the owner to try with real clients**, whenever there is time:

1. A cluster as processes, each in a terminal of its own:
   ```bash
   cargo build -p clustine
   target/debug/clustine coordinator --boundaries 4
   target/debug/clustine worldstore --world world --boundaries 4
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
--boundaries`, the same as the coordinator), keeps a table of regions and of which
region holds which chunk, lets only the holder load and save a chunk, grants and takes
back chunks, replays a region's log only into what it holds, merges and splits regions
as one record of the log each, and gives the coordinator the list of regions. After a
failed write of the log every region loses its owner and nobody is served until the
log is cut back durably. It is killed at every write and sync of claims, returns, a
merge and a split (`kill_regions.rs`), which found a fault in the chunk store older
than this work: a section file left behind by two faults in a row was taken for stored.
Nothing asks the store for any of the new things yet: the stripes are pinned regions
and work as before. What the builder decided where the record was silent is at the
record's end. The scenarios of its section 9 were then written by someone else from
the record alone (`services/worldstore/src/scenarios.rs`, 79 tests, the thread for
chunks held at chosen points and the store killed there): they found nothing, and
catch each of ten faults that were put into the store to see whether they can.

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
that ask the store; the pause at a move is 0.8 seconds in the middle, as before. Left
of C2b: the scenarios of ADR-0013 by someone else, and C2b.5, which takes the given
stripes away so that asking is the only way.

What C0 left to the steps that use it, because it changes what exists instead of adding
to it: `Departed` and `Remote` naming the region they go to, `since` in an `EdgeState`
and in hellos, the welcome saying how many entries follow (all C2b); how `Restored::held`
and the list of regions travel between the store and others (C1).

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
