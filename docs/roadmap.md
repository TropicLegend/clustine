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
another region is now passed on to it.

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
