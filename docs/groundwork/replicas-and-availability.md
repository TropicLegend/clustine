# Several replicas of every service: groundwork

Written 2026-10-08 against `main` at `6f06cab`, by reading the code and the records, as
groundwork for the milestone after M3 (see the roadmap, "After M3"). Nothing was built
and no test was run for it. It is not a plan: the plan is written when the owner has
answered the questions in section 9, and is reviewed independently before anything is
built. Three of the stale texts section 10 names (the comment of `worker.yaml`, the help
of `clustine move --to`, the README's status) were put right with the commit that added
this file.

## How to read this

- **Verified** means I read it in the code, at the place named.
- **Record** means a decision record or the roadmap says it and I did not check the code.
- **Inferred** means reasoning from what I read. Treat it as a claim to test.
- ADR-0014 is proposed and under review. What rests on it is marked so.

What I read in full: `CLAUDE.md`, `README.md`, `docs/architecture.md`, `docs/roadmap.md`,
ADR-0003, 0005 to 0010 and 0013, `bin/clustine/src/cluster.rs` and `main.rs`,
`crates/clustine-rpc/src/messages.rs`, `services/edge/src/lib.rs`, `login.rs`,
`session.rs`, `status.rs`, everything in `deploy/kubernetes/`, `deploy/README.md`.
Read in the parts that matter here: ADR-0011, 0012, 0014, `services/coordinator/src/state.rs`
and `service.rs`, `services/worldstore/src/lib.rs`, `lanes.rs`, `disk.rs`, `chunks.rs`,
`tcp.rs`, `kill.rs`, `services/edge/src/fanout.rs`, `services/worker/src/lib.rs`,
`crates/clustine-sim/src/region.rs`, `bin/clustine/tests/chaos.rs`, `moves.rs`,
`common/processes.rs`, `deploy/kind/test.sh`.

## The short version

1. **The edge is the only part whose death a player sees as a disconnect, and nothing
   about several edges is far away.** Regions, the runner and the coordinator are built
   and unit-tested for several edges. The edge itself is not: four concrete gaps are
   listed in section 1.3. No end-to-end test starts two edges.
2. **A second coordinator today is safe for the world and bad for players.** The store
   lets only the highest epoch commit, so nothing is corrupted. But two coordinators
   would take regions from each other for ever, each time with a pause. A standby needs
   a lease and nothing more durable than that.
3. **The world store is the one place where high availability is expensive.** A
   replicated network volume gives survival of a node and its disk with no code. A
   synchronous standby of our own is weeks. A Raft log is months. I recommend the
   volume first and a decision by the owner on the rest.
4. **A player who logs in again starts at the spawn point with the starting hotbar.**
   That is so today with one edge, whatever the reason for logging in again
   (`Region::tick`, `PlayerChange::Join`). "Back where they were" after an edge dies
   needs that changed first.

---

## 1. Edge

### 1.1 What happens today when it dies, and when it is restarted

Verified:

- A player's connection is a TCP connection to the edge process. Each is a task in the
  `JoinSet` of `Edge::run` (`services/edge/src/lib.rs`). When the process goes, every
  connection goes.
- On SIGTERM the edge stops at once: `cluster::edge` returns when `stop_signal`
  fires, which drops everything. Nobody is told anything first. There is no draining.
- Regions notice through their links ending. They keep the edge's players, standing
  still, for `DEFAULT_GONE_AFTER` = 600 ticks = 30 seconds
  (`services/worker/src/lib.rs`), then remove them (`EdgeEvent::Gone`).
- The edge takes a new `start` at every start (`EdgeIdentity::starting_now`, wall clock
  in milliseconds). When it comes back under the same name, each region sees a higher
  start and resets the edge: its players are removed at once (ADR-0008, section 2;
  `drop_edge` in `crates/clustine-sim/src/region.rs`).
- A new edge does not listen until the coordinator's table has a worker for every
  region (`whole_world` in `cluster.rs`). So an edge cannot start while the
  coordinator is away.
- A join always makes a new entity at the spawn point with the starting hotbar
  (`PlayerChange::Join` in `region.rs`: `Pose::at(self.config.spawn)`,
  `self.config.starting_hotbar`). There is no player data (`services/playerdata` is one
  line).
- The chaos tests leave the edge alone, and say so (`chaos.rs`, the header: "an edge
  that dies takes its players with it"). The kind test deletes worker pods and the
  store pod, never the edge or the coordinator (`deploy/kind/test.sh`, the loop over
  `clustine-worker-0..2` and `clustine-worldstore-0`).

So, for a player today:

| | Who notices | After | Player sees | Lost |
|---|---|---|---|---|
| Edge process dies | Client, regions | At once | "Connection lost". Has to click to join again. Waits until the pod is back and the edge listens | Position, hotbar, held slot: they enter at spawn. Nothing of the world |
| Edge's machine vanishes | Client | Up to 30 s (the client's own read timeout; inferred from `DEFAULT_KEEP_ALIVE_INTERVAL`'s comment) | As above, later | As above |
| Edge restarted on purpose | As a death | At once | As a death | As a death |

Other players see the dead edge's players stand still for up to 30 seconds, or until
the edge is back, and then vanish. With one edge there are no other players.

### 1.2 What state it holds

Verified in `fanout.rs`:

- **In memory, per player** (`PlayerView`): the session, the entity, the region the
  player is believed to be in, the numbered inputs not yet reported applied and durable
  (`kept_inputs`), sequence numbers under way and acknowledged, the view (`wanted`,
  `sent`, `pending`), which entities and list entries the client was shown, the chunk
  batch state, the teleport the client still has to confirm.
- **In memory, per region** (`RegionPort`): the link, the numbered messages not yet
  reported applied (`kept`), `applied`, `seen`, `since`, and the subscriptions.
- **In memory, shared**: the replica of every chunk some player sees, and the entities
  shown with the region that introduced each (`Shown::from`).
- **On disk**: nothing. The manifest says so (`readOnlyRootFilesystem: true`).
- **In other services**: each region keeps an `EdgeState` per `EdgeId` in its durable
  `RegionState` (start, `since`, `applied`, `sent`, the outbox), and each player there
  names the edge they belong to (ADR-0008, section 1).

What is built to survive: a region outliving a lost link (the edge keeps and sends
again), and an edge outliving a region's worker. What is **not** built to survive is
the edge process: a new start is by design a reset. All of the edge's kept state is
about connections that died with it, so there is nothing a replica could take over
unless it also had the TCP connections.

### 1.3 Several edges at once: what works and what does not

What the records intended and the code has (verified):

- The sim keeps state per edge and refuses what comes through another edge than the
  player's (`region.rs`, the join, leave and input paths). The sim's and the runner's
  specification tests use two edges throughout
  (`crates/clustine-sim/tests/specification.rs`, `services/worker/tests/specification.rs`,
  and the worker's own tests around "Two edges watch different parts of the region").
- A worker fans out per link. An edge gets the events of the chunks it subscribed to.
  So a player of edge A who walks through a chunk that a player of edge B sees is sent
  to B as an entity.
- `EntityKind::Player` carries the player's id and name, and an edge adds a player to a
  client's list the first time it shows their entity (`PlayerView::update_visibility`).
  So **players of two edges do see each other**, with the right name.
- Block actions that go through another region are per edge: `Remote` goes to the
  outbox of the player's edge, `RemoteDone` to the edge the action came from. The
  worker's tests cover a second edge watching.
- The coordinator sends the routing table to any number of watchers (`Role::Watcher` in
  `service.rs`).
- The StatefulSet gives each edge pod a stable name, from which the `EdgeId` follows
  (`edge.yaml`, `EdgeId::from_name`). `replicas: 2` would give two distinct edges.

What does not work with two edges (each verified in the code named):

1. **Order between regions, for an edge that only watches.** The edge takes moves and
   removals of an entity only from the region that last introduced it
   (`Fanout::is_of`, `upsert_entity`). With one edge that is safe, because the new
   region speaks of a player only after this edge passed the player on. A watching edge
   passed nothing on, so what two regions say reaches it in no order. The comment on
   `Shown` says so. The mechanism is verified; the cases are worked out by reading and
   were not run:
   - A player of A crosses a boundary and back within a tick or two, which pinned
     stripes allow (ADR-0006: "handed back and forth every tick"). B is sent two
     `EntitySpawned`, one from each region, over two links. If the earlier one is read
     last, `from` names the region the player has left. Moves from the region they
     are in are then passed over. The entity stands frozen on B's screens until the
     next hand-over. This is the likeliest case.
   - The same from a late snapshot of the old region that still has the entity.
   - A player of A leaves in the middle of a hand-over. A sends `Discard` to the
     destination, which reports the entity removed. B never saw that region introduce
     the entity, so it passes the removal over. A ghost stays. ADR-0013 found this
     case for one edge and had the discarding edge hide the entity itself; another
     edge is not helped by that.
   - ADR-0006 already says what is needed: "versions on entity states". ADR-0013 lists
     it as open question 2.
2. **The player list is each edge's own.** `spawn_player` lists the players of this
   edge. A player of another edge appears in the list only once seen, and is then
   never taken off it (`remove_player` removes only this edge's own players). The
   server list ping counts this edge's players only (`status_json`, `Shared::online`).
   `max_players` is shown and not enforced anywhere (verified by search).
3. **One account can be on two edges at once.** The edge refuses a second connection
   of a player it has (`handle_command`, "You are already connected"). Through another
   edge the join goes to the home region, which replaces the player only if it has
   them itself (`PlayerChange::Join`). If the first self has walked into another
   region, the world has two entities of one player, and the old connection lives on.
   ADR-0014 (proposed) orders two stays of a player by entity id when they meet in one
   region. It does not find the other stay.
4. **Nobody knows which edges are alive.** Regions find out that an edge is gone only
   by 30 seconds without a link. The coordinator does not know edges at all beyond an
   open watcher connection.

Also true, and smaller: there is no chat and there are no system messages yet (search of
`play.rs` and `fanout.rs`), so nothing to relay; when they come they need a path
between edges. An edge links to every region (`keep_linked`), so N edges are N links
per region. `Welcome::Superseded` between processes is not covered by any test
(roadmap, "Not covered by tests yet").

**What it needs**, without designing it:

- A version on an entity's stay, raised at each hand-over and carried in the transfer
  and in `EntityState`, so that an edge takes the latest word whichever link it came
  over. This is in the sim, the runner and the edge, in the part "where ordering
  mistakes hide", so it is not for delegation.
- A directory of who is in the world, shared by the edges: for the player list, the
  count, the second login, and later chat. The architecture gives "global world
  state ... player list" to the coordinator. It can be in memory: edges report on
  connecting to the coordinator, as workers report what they hold. The same connection
  can be the edge's lease, so that regions are told that an edge is gone in seconds
  and not in 30.
- An end-to-end harness with two edges (`common/processes.rs` has one `edge` field).

### 1.4 What "an edge dies" can become

Honestly, in order of cost:

1. **The player joins again by hand, through another edge, at once.** The vanilla
   client does not reconnect by itself (my knowledge of the client; not checked
   against 26.3). With several edges behind a load balancer the next attempt lands on a
   living edge. A death then costs a fraction of the players one click. This is what
   several edges give with no further work.
2. **The transfer packet, for stops that are planned.** The protocol has it: the
   generated tables list `minecraft:transfer` and the cookie packets for configuration
   and play (`crates/clustine-protocol/src/generated/packet_ids.rs`), and the edge
   already accepts handshake intent 3 as a login (`session.rs`, `Intent::Transfer`).
   The packet itself is not implemented. An edge told to stop could send every player
   to the public address, and the client reconnects by itself: login and configuration
   again, a loading screen for about a second. It works for a rolling update, a drain
   and scaling down. **It cannot work for a crash**: a dead process sends nothing.
   A new packet has to be checked with the official server or a real client
   (`CLAUDE.md`).
3. **A proxy in front that keeps the TCP connection.** The connection then belongs to
   the proxy, and the proxy can attach the player to another edge by sending the
   client back to configuration and joining again. Two forms:
   - A stock proxy. Velocity has `failover-on-unexpected-server-disconnect` and a
     `try` list. It is a Java program outside this project, has to support protocol
     777, and wants its own forwarding of player identity. Not evaluated further.
   - Our own: the architecture allows splitting the edge into a gateway and a fan-out
     ("They may be split into two services"). Weeks, and it should come after
     encryption exists, since the gateway owns the cipher.
   Either way the single point moves; it does not go. A gateway that does little dies
   less often, and that is all it buys. I do not recommend it for this milestone.
4. **A connection that survives its process**: not possible. Nothing in the client or
   in TCP lets another process take a connection over.

### 1.5 A fast re-login that puts the player back where they were

What it needs, from the code:

- **The place has to survive.** Today a join is a new entity at spawn. Two ways:
  - *Adopt*: the region that still has the player, under the dead edge, gives them to
    the new edge. The edge side nearly exists: a `Presence::Present` for a player who
    was never shown their own entity makes the edge put them into the world with that
    entity, pose, hotbar and slot (`Fanout::presence`, ADR-0008 section 5, step 5).
    Missing: a message that asks for it, finding the region, and the player who was
    in mid hand-over, whose only trace is a `Departed` in the dead edge's outbox.
    That last case is an ordering problem of the usual kind.
  - *Persist*: when a region removes a player (a leave, a reset, `Gone`), it stores
    where they were and what they held, and a join starts from that. Inferred: a join
    at a position in another region's chunk needs nothing new in the tick, because a
    region already lets a player go who stands in a chunk another region holds
    (ADR-0010, section 2), so the home region would hand them on at once. This is
    also what ordinary logging out and in needs, which a player notices within
    minutes today.
- **The old self has to go at once**, not after 30 seconds: either the directory of
  section 1.3 tells the regions, or the new join finds it.
- **Time**: login and configuration are local to the edge. Placing and the chunks are
  what a resume costs today, about 0.4 s optimised and 0.75 s unoptimised (roadmap and
  ADR-0014's measurement). So one to two seconds after the client connects is
  realistic. Inferred, not measured.

Recommendation: persist. It is the simpler of the two, it is needed anyway, and it
keeps the dead edge's state out of the picture. Adopt can follow if the owner wants
the entity id kept.

---

## 2. Worker

### 2.1 Today

Verified: `cluster::worker`, `Coordinator` in `state.rs`, `chaos.rs`, `moves.rs`.

| | Who notices | After | Player sees | Lost |
|---|---|---|---|---|
| Worker dies | Coordinator, by silence or no vouch | Lease of 5 s, looked at every lease/4 | Players of its regions stand still 5 to 7 s. Nobody disconnected | Nothing shown |
| Worker told to stop | Itself, tells `Leaving` | At once | A pause per region: 0.36 to 0.47 s optimised, 0.74 to 1.27 s unoptimised (measured, ADR-0014 and roadmap) | Nothing |
| Worker cut off from the store | Itself (`store_lost`), then the coordinator after 30 s of `WaitingForStore` | | Stand still; disconnected after the edge's 20 s | Nothing shown |

Workers already are several. A worker runs several regions, each on a thread
(`Phase`, `Regions`). No spare is needed since C2a. `chaos.rs` kills workers at random
and one of two workers that run three regions; `moves.rs` replaces every worker in
turn; the kind test does both on Kubernetes.

### 2.2 State

In memory: the regions it runs. Everything a player was shown is in the store before it
is shown (output commit). On disk: nothing. A replica of a worker is simply another
worker. Nothing is missing here for availability.

### 2.3 What is missing: balance by load

Verified:

- A heartbeat is `Heartbeat { regions: Vec<(RegionId, Vouch)> }`, once a second. It
  carries no load.
- The coordinator counts regions: `loads()` is regions owned plus regions reserved,
  `lightest()` picks the fewest, `even_out()` moves one region when the difference is
  two or more, one release at a time, never during the grace period, never to or from
  a worker at fault.
- The runner publishes `RegionStatus`: tick number, players, loaded chunks, held
  chunks, arrivals, departures, and `crowds` (chunks with players and how many).
  Nothing sends `crowds` yet; `ToCoordinator::Players` exists and is ignored until C4.
- **Nobody measures how long a tick takes.** There is `MAX_CATCH_UP_TICKS` for a
  runner that falls behind, and no counter of it. There are no metrics at all (no
  such dependency in any `Cargo.toml`).

Options:

1. **Weigh by players** (after C4, which brings where players are and how many). Small:
   `loads()` returns a weight instead of a count. Crude, since a region with ten idle
   players costs less than one with two builders, but it needs no new measurement.
2. **Weigh by tick time.** The runner measures each tick and reports, per region, the
   time used of the 50 ms (a mean and a high percentile over some seconds) and ticks
   skipped. The worker reports its number of processors. A worker's load is the sum of
   its regions' tick time over 50 ms times its processors. Small to medium: one
   measurement in the runner, two fields in the heartbeat, the weight in `state.rs`.
   The state machine is free of I/O and well tested, so this part suits a subagent
   with tests written from a specification.
3. **Scaling on Kubernetes.** Scaling down already works: SIGTERM, `Leaving`, hand
   over, exit (`LEAVE_WITHIN` 20 s under a grace period of 30 s). Scaling up works:
   a new worker registers and is evened out towards. Automatic scaling needs a
   metric to scale on, which needs a metrics endpoint. The roadmap puts the operator
   and autoscaling in M4; I would export the numbers here and leave the autoscaler
   there.

Two cautions. A move costs the region's players a pause, so balancing must be slow to
act and must not undo itself; `even_out`'s "difference of two" rule is the model. And
a region is one thread: a region that needs more than 50 ms per tick is not helped by
any move unless its machine is oversubscribed.

Spare capacity: with no spare, the regions of a dead worker go to the others. Whether
the others have processors for them nobody checks. The coordinator could say so in its
log once it knows processors and load.

---

## 3. Coordinator

### 3.1 Today

Verified: `service.rs`, `state.rs`, `cluster.rs`, `chaos.rs`
(`players_keep_playing_while_the_coordinator_is_killed_and_comes_back`), `moves.rs`
(the coordinator killed in the middle of a move).

- It keeps nothing on disk. Its epochs start from the wall clock in milliseconds
  (`serve` passes `unix_milliseconds()`), so a new one issues above the old one's.
- While it is away: regions tick, players play, hand-overs work. Nothing is taken
  over, moved or evened out. `clustine move` fails. A new edge cannot start.
- Workers notice at once (the connection ends) and try to register every second,
  saying what they hold (`stay_registered`). Edges look for it every second
  (`find_coordinator`) and keep their routes.
- A new coordinator assigns nothing for one lease (5 s), so that workers can report.
  A region its owner released does not wait (`Region::let_go`).
- On Kubernetes it is a Deployment with one replica and `strategy: Recreate`. The
  comment there says why: two would "each give the regions away on its own".

A player notices a dead coordinator only if something else fails in the same window.
Then the takeover waits for the coordinator to be back, plus the grace period.

### 3.2 State a second one would need

None that is durable. Everything is rebuilt: workers and their holdings from
registrations, the regions from the layout (and, with ADR-0014, from the store's list,
proposed). What is lost at a restart is only in-flight intent: a release under way, the
"at fault" memory of a worker, who asked for a move. The tests show the first is
recovered from.

### 3.3 What two coordinators would do today

Verified by reading; nobody has run it:

- **The world stays safe.** The store refuses a hello with a lower epoch than the
  region's highest, keeps that on disk, and an open with a higher epoch loses the old
  owner its handle (`Lanes::admit`, `Lanes::open`). A region shows nothing that is not
  confirmed. So two owners can never both get anything of one region confirmed.
- **Players would suffer.** Each coordinator, after its grace period, gives every
  region it believes unowned to its own workers (`assign`). The store lets the higher
  epoch in. The loser reports `EpochRefused`, its coordinator raises its epochs above
  what the store saw and assigns again at once (`epoch_refused` ends like a tick). The
  region goes back and forth, each time with a restore and a resume. Nothing ends it.
- Each worker and edge is connected to one coordinator. An edge links only to the
  owner and epoch its own table names (`keep_linked`). While that coordinator's worker
  does not have the region, the edge's players stand still, and they are dropped if
  that lasts 20 seconds.
- A registration through a load-balancing Service would land on either coordinator at
  random. And a standby that answers `FromCoordinator::Refused` would end the worker
  process (`register` in `cluster.rs` bails on a refusal). A standby needs its own
  answer.

So: epochs protect the data fully and the players not at all. Exactly one coordinator
may give orders.

### 3.4 Options for standbys

| Election through | Needs | For | Against |
|---|---|---|---|
| **A lease in the world store** | A new first message to the store; a term number kept on disk (one small file); renewals in memory | Works without Kubernetes. The store is already what fences. The term can be the high part of every epoch, which removes the wall clock from epochs (ADR-0007 names the clock as a hazard). The store can refuse an opening issued under an old term, so a deposed leader is fenced where it matters. ADR-0014 gives the coordinator the store's address anyway | No leader while the store is down. That costs little: nothing can be assigned usefully then |
| **A Kubernetes Lease** | The `kube` client, a service account and a role; crates exist (`kube-lease-manager`, `kube-coordinate`; I read their descriptions only) | Standard on Kubernetes; nothing new in the store | Nothing for clusters of plain processes. The manifests give no service an API token today. Fencing stays by clock unless a term is also carried into epochs |
| **Raft among coordinators** | A Raft library and three replicas | What `architecture.md` first planned | There is nothing to replicate. It would be an election and nothing else, at the price of a consensus library |
| **External etcd** | An etcd cluster | Well known | A system to run for one small lease |

Recommendation: the lease in the store, with the term in the epochs. It is the one
that makes "a second coordinator must not act" something the store enforces and not
something clocks promise.

What else it needs, whichever is chosen:

- **Finding the leader.** Clients take one address today. Either a list of addresses
  tried in turn, with a standby answering "not the leader, try there"; or, on
  Kubernetes, a Service that selects the leader only. Readiness as "is the leader" is a
  trap with a Deployment: a rollout waits for pods that are standbys by design.
- **A leader that cannot renew stops giving orders** before its lease is over.
- **Orders carry the term**, and a worker passes over orders of a lower term than it
  has seen.
- The grace period stays for a first version. Later the standby can listen to
  registrations without acting, and take over without waiting a lease.

Size: one to two weeks. The state machine barely changes; the work is in `service.rs`,
`client.rs`, the store's lease and `cluster.rs`.

---

## 4. World store

### 4.1 Today

Verified: `lanes.rs`, `tcp.rs`, `cluster.rs`, `chaos.rs`
(`players_keep_playing_while_the_world_store_is_killed_and_comes_back`), `cluster.rs`
test `workers_restore_their_regions_when_the_world_store_is_back`, `kind/test.sh`.

- It is one process with one directory. A commit is answered once a group of commits
  is synced (`Lanes::end_group`). One thread decides everything about regions, grants
  and the log.
- When it dies, each region's handle is lost. The runner stops, closes its links, and
  the worker opens the region again every second until the store answers
  (`open_region`, `RETRY`). Workers vouch `WaitingForStore`, which the coordinator
  accepts for 30 s.
- Players stand still. The edge drops a player whose oldest unconfirmed input is 20 s
  old (`region_patience`, "The server fell too far behind").
- On Kubernetes it is a StatefulSet with one replica and a `ReadWriteOnce` claim of
  1 Gi from the default storage class. A deleted pod comes back under its name with
  the same volume; the kind test does that under bots with nobody disconnected.

| | Player sees | Lost |
|---|---|---|
| Process dies, restarted within about 15 s | Everyone stands still, then goes on | Nothing shown |
| Away for more than 20 s | Everyone is disconnected, can join again later | Nothing shown; players' places |
| Its node dies | Depends on the volume, see 4.3 | |
| Its disk is lost | The world is gone | Everything |

### 4.2 State

Everything durable: the log (`log/<n>.wal`), the table of regions and grants
(`regions/table`), per region the epoch and entity ids (`.region`) and the last whole
state (`.state`), chunk manifests and content-addressed section files. In memory only:
the sessions of owners, and what follows from the files.

One fact matters for every option below. **All of the store's file access goes through
one trait**, `Disk` in `disk.rs` (read, write, append, truncate, sync, sync of a
directory, rename, remove), with the local file system and a simulated disk behind it.
The log and the chunk files both use it (`Lanes`, `FileChunks::new(disk, root)`). The
one exception is `local::prepare`, which uses `std::fs` before the store starts. The
kill tests work by failing or stopping the simulated disk at the n-th operation, for
every n, and opening a new store on what a crash would leave (`kill.rs`,
`kill_regions.rs`).

### 4.3 Options

Latency matters because of output commit: nothing of a tick is shown before its commit
is confirmed. The budget is not hard, as a region runs up to eight ticks ahead
(`MAX_TICKS_AHEAD`), but every millisecond is added to what a player waits for an
acknowledgement.

**A. One store on a replicated network volume, restarted elsewhere by Kubernetes.**

- This is today's manifest with a storage class that is network-attached and
  replicated (a cloud provider's block volumes; Ceph or Longhorn on one's own
  machines). kind's default storage is, as far as I know, a directory on the node, and
  proves nothing here.
- Protects against: process death (already), node death, and disk loss as far as the
  volume is replicated.
- Pause: for a process, seconds. For a node, **minutes unless something acts**:
  Kubernetes does not replace a StatefulSet pod on a node that went silent until the
  node object is removed or tainted out of service, because it cannot know that the
  old pod is dead (Kubernetes' documentation of non-graceful node shutdown). Everyone
  is disconnected at 20 s. With automation that fences the node it is tens of seconds.
- Commit latency: whatever the volume's sync costs. Not ours to shape.
- Fencing: by the volume being attached to one node at a time. That holds for block
  volumes. It does not hold for shared file systems such as NFS, where two stores
  could write one world; the store has no lock of its own against that.
- Reuse: all of the store. Code: none, or a start-up lock file as a guard.
- Testing: a kind cluster with several nodes and a node stopped under the ledger bots
  shows the rescheduling; it does not show a real storage system.

**B. A synchronous standby of our own, by mirroring the disk.**

- A second store process on another node holds a copy of the directory. A `Disk` that
  wraps the local one sends every write, append, truncate, rename and remove to the
  standby in order, and a sync returns when both have synced. The store above it does
  not change. After a failover the standby opens what it has as after a crash, which
  is the path the kill tests already cover.
- Protects against: process, node and disk loss, without anything external.
- Pause: detection plus promotion plus every region being opened again. Has to stay
  well under the edge's 20 s, or that patience has to rise (it must stay below the
  30 s after which a region forgets an edge).
- Commit latency: the slower of the two syncs plus a round trip, done side by side.
  On one network with good disks under a millisecond more. Inferred.
- What is hard: who may promote. Two processes cannot decide that between themselves.
  It needs a third party: a Kubernetes Lease, or an operator's hand. And a primary that
  has lost its standby may go on alone only while it holds that lease. Then bringing a
  new or returning standby up to date while the primary runs: section files never
  change and are easy; the log, manifests and small files need an order.
- Reuse: nearly all of the store. New: the mirroring disk, the standby process, the
  resynchronisation, promotion.
- Testing: the equivalent of the kill tests is direct. Two simulated disks and a
  simulated connection; a fault or a stop at every operation of either disk and at
  every message between them; then a store opened on **the standby's** crash image has
  to restore every region with everything that was confirmed. The existing scenarios
  and their assertions carry over. Then the chaos tests with the primary killed.
- Size: three to six weeks. Large.

Shipping only log records to a standby that applies them was also considered. The
store can apply block changes to chunks by itself (`Job::Restore`, `Job::Fold`), but
saves and checkpoints are not log records and a region's state is opaque bytes to the
store, so the standby would need those streams too. Mirroring files is simpler and
keeps one recovery path.

**C. A replicated log with Raft among three stores.**

- Protects against the same as B, with election built in and no third party.
- Libraries, honestly: `openraft` is before 1.0, says its interface is unstable, and
  says of itself that its chaos testing is not complete; Databend runs it. `raft` from
  TiKV is the mature core (last release 0.7.0 in 2023 as far as I found) and leaves the
  log, the transport and the state machine to us. Either way the store's own log, with
  its torn-write handling and segment collection, has to become the Raft log's storage
  or live beside it, and saves and checkpoints have to become entries or be shipped
  beside the log. A snapshot is the world directory.
- Commit latency: a round trip to a majority plus their syncs. Like B.
- Pause: an election, one to a few seconds, plus regions opening again.
- Reuse: the table, the lanes' rules and the chunk store; the log and recovery are
  rewritten.
- Testing: a deterministic simulation of three nodes with faults in disks and
  network, plus everything the store has today. That is a project.
- Size: months. I do not recommend it at this project's size.

**D. Sharding the store by region.**

- Lanes are per region, but three things are shared and on one thread on purpose: the
  log (one sync makes a group of all regions durable), the table of grants (a chunk
  has one holder, decided in arrival order), and the merge and the split, which are
  one log record each exactly so that no step spans two places (ADR-0010 review item
  2, ADR-0011 section 6). Section files are shared between regions by content.
- Separate store processes would need a shared authority for grants and a
  transaction for a merge across stores. That undoes ADR-0011's central decision.
- It would buy throughput, not availability: each shard is still the only copy of its
  regions.
- Not recommended. If commit throughput ever limits, measure first; the roadmap notes
  that commit latency on a real disk was never timed.

**E. An external durable system under the store** (object storage, a database with
synchronous replication). `architecture.md` plans a backend trait and object storage
later. It moves the problem to something already solved, at the price of a dependency
and, for object storage, tens of milliseconds per commit. A question for the owner
rather than a recommendation.

**Reads from replicas**: nothing to gain. Only the holder loads a chunk, and a load
has to see the saves before it.

Recommendation: A now, with the manifest and the README saying which storage classes
are safe, a guard against two stores on one directory, and a test on a kind cluster of
several nodes. B only if the owner wants to survive the loss of a node and its disk
without depending on replicated storage. Whatever is chosen, a **backup** is the cheap
answer to disk loss and to mistakes, and replication is not a backup; there is none
today.

---

## 5. Load balancing, place by place

| Where | Today | What "balanced" should be measured by | What it needs |
|---|---|---|---|
| Players across edges | One edge | Connections per edge first. Later bytes sent per second and how far the fan-out task lags, since `Fanout` is one task and an edge's fan-out is therefore one processor | A TCP load balancer in front. Connections are long-lived, so it balances only new ones. To move players off an edge on purpose, the transfer packet |
| Regions across workers | By count of regions | Tick time used per worker against its processors; players as a first stand-in | Section 2.3 |
| Regions themselves | Fixed | Players near each other | C3 to C5 of M3, not this milestone |
| Requests across stores | One store | Nothing | Nothing; see 4.3 |
| Coordinator | One | Nothing: it does no work in proportion to players | Nothing |

One thing follows for edges: the reason to have several is not only survival. Fan-out
per edge is single-threaded, so edges are also how fan-out scales (M5).

---

## 6. Security between services

Verified: no service authenticates another. A link begins with a plain `RegionHello`
(`clustine-rpc/src/tcp.rs`), the coordinator takes whatever is said first as the
connection's role (`service.rs`), and the store opens a region for whoever names it
with a high enough epoch (`tcp.rs`). Whoever reaches the coordinator's port can move
regions; whoever reaches the store's can read and overwrite the world. Players are not
authenticated either (offline mode).

The least that several replicas on a real cluster need:

1. **NetworkPolicies** in `deploy/kubernetes`: the store from workers and coordinators
   only, the coordinator from workers and edges, workers from edges, the edge from
   anywhere. A day. It needs a network plugin that enforces them, which kind's default
   may not; say so.
2. **A shared secret proved at the start of every connection**, from a Kubernetes
   Secret: a challenge and a keyed hash, so that it cannot be replayed. Connections
   begin in three places (`clustine-rpc/src/tcp.rs`, the coordinator's first message,
   the store's `StoreHello`). A few days. It authenticates; it does not encrypt.
3. **Mutual TLS** is the full answer. `rustls` is already in the lock file for another
   tool. The store's sockets are blocking, the others asynchronous, so it is two
   integrations, plus certificates to issue and renew. More than this milestone needs;
   a service mesh can also do it without code.

I recommend 1 and 2. And one thing outside "between services": an edge behind a public
load balancer needs online mode first. `edge.yaml` says in capitals not to expose it.

---

## 7. Kubernetes

### 7.1 What the manifests do today

Verified in `deploy/kubernetes/`:

| Service | Kind | Replicas | Service | Probes | Notes |
|---|---|---|---|---|---|
| coordinator | Deployment, `Recreate` | 1 | ClusterIP | TCP readiness and liveness | No API token |
| worldstore | StatefulSet with a 1 Gi `ReadWriteOnce` claim | 1 | ClusterIP | TCP | 30 s to stop |
| worker | StatefulSet, parallel, rolling update | 3 | Headless, publishes addresses that are not ready | TCP; listens only once registered, so ready means "known to the coordinator" | 30 s to stop; advertises its pod name |
| edge | StatefulSet | 1 | ClusterIP | TCP; listens only once every region has had a worker | Name from the pod name |

There is no PodDisruptionBudget, no anti-affinity, no NetworkPolicy, no autoscaler and
no metrics. The kind cluster is one node (`deploy/kind/cluster.yaml`). The comment at
the top of `worker.yaml` still says a worker runs one region.

### 7.2 What each option needs

- **Edges**: `replicas` above one, anti-affinity across nodes, a budget of one
  unavailable at a time, a Service a load balancer can front. A load balancer keeps an
  established TCP connection as long as its pod lives, also when the pod stops being
  ready, so "not ready" can mean "takes no new players" while it drains (my
  understanding of kube-proxy; to be tried). A grace period long enough to transfer
  everyone. The public address as a setting, for the transfer packet.
- **Coordinator**: two or three replicas, rolling updates allowed once a lease exists,
  a headless Service or a list of addresses so that clients can find the leader. A
  role for Leases only if the Kubernetes Lease is chosen.
- **Store, option A**: a named storage class, `ReadWriteOncePod` where the cluster has
  it, and a note on what replaces a pod on a dead node. **Option B**: two replicas
  with a claim each, required anti-affinity, a headless Service between them, a Lease
  and its role, a budget.
- **Workers**: a budget of one unavailable, so that a node drain moves regions one
  worker at a time; soft anti-affinity. Readiness is already right.
- **The kind test**: worker nodes in `cluster.yaml`, so that a node can be stopped.
  That uses the node image the test already uses.

---

## 8. An order of work

Sizes are for this code base and this way of working (a record, a review, tests from
the record by someone else). S is days, M is one to two weeks, L is three weeks or more.

| # | What | Size | Helps a player | Depends on | Who |
|---|---|---|---|---|---|
| 1 | **Two edges that are right**: stay versions on entities; a directory of players and edges at the coordinator (list, count, second login, an edge known dead in seconds); two edges in the test harness, in the manifests and in the kind test | L | An edge's death hits a part of the players, and they are back with one click. First thing a player meets | M3's C3 settling the edge's contract (ADR-0014) | Not delegated: it is the edge's ordering |
| 2 | **A player's place and hotbar survive leaving** | M | Logging in again, for any reason, puts them where they were. Noticed within minutes today | Where it is kept is a decision (the store, through the lanes) | Store part delegable |
| 3 | **Edges drain with the transfer packet** | S to M | Rolling updates and scaling of edges without a click | 1; a real client or the official server to check the packet | Edge; bots need to follow a transfer |
| 4 | **Coordinator standby with a lease in the store, terms in epochs** | M | Little by itself; shortens the worst case when two things fail | ADR-0014 giving the coordinator the store's address | Side by side with 1: other crates |
| 5 | **Balance by load**: tick time measured and reported, weights in the coordinator, numbers exported | M | Lag where one worker is full and another idle | C4 of M3, so that it does not fight merging and splitting | State machine delegable, tests from the record |
| 6 | **Authentication between services**: NetworkPolicies and a shared secret | S | Nothing visible; needed before any real cluster | None | Side by side: `clustine-rpc` and the manifests |
| 7 | **Store on a replicated volume**: manifests, guard, several-node kind test, a backup command | S to M | Survives a node and its disk, with a pause | A decision on dependencies | Deploy and store; side by side |
| 8 | **Store standby by mirroring the disk** | L | The same without external storage, and a shorter pause | 4 (the same lease), 7's tests | Store; the promotion logic not delegated |
| 9 | Raft, sharding the store, a gateway in front of edges | L to months | | | Not recommended now |

Cheapest for the most: 7 in its small form, 6, then 3. The largest gain for a player is
1 with 2. What can be built side by side by people who share no files: 4 (coordinator,
a little of the store), 5 (worker measurement and the coordinator's weights, after 4
lands in `state.rs` or before it), 6 (`clustine-rpc`), 7 (`deploy/` and the store's
start), while 1 and 3 stay with whoever owns the edge. The shared messages
(`ToCoordinator`, `FromCoordinator`, the heartbeat, the hello to the store) have to be
fixed and pushed first, as always.

### Tests to add

- **Two edges, end to end**: every hand-over, block and takeover test with the watcher
  on the other edge. `a_watcher_sees_one_entity_cross_the_boundary` with the watcher on
  edge B is the end-to-end test for the ordering gap. It is a race, so it may fail
  only now and then; the test that fails every time is one in `fanout.rs` on scripted
  regions, which hands the edge the two regions' words in the bad order. The same
  account joining through both edges. The list and the count as seen from each.
- **Chaos with edges**: kill one edge of two under the ledger bots. Bots of the other
  edge must notice nothing but the dead edge's players standing still and then going.
  Bots of the dead edge join again through the other and, once 2 is built, are where
  they were with what they held, and their ledger still holds.
- **Draining**: every edge replaced in turn under bots that follow a transfer; nobody
  ends disconnected.
- **Coordinator**: two started at once, and no region changes hands that had an owner.
  The leader killed while a worker dies. A deposed leader's orders passed over. The
  leader killed in the middle of a move, a merge and a split.
- **Balance**: state-machine tests with scripted loads, and one that shows nothing
  moves back and forth.
- **Store on a volume**: a node stopped on a kind cluster of several nodes.
- **Store standby**: as in 4.3 B, the kill tests on two disks and a connection, then
  chaos with the primary killed.
- **Security**: a connection without the secret is turned away by each of the three.

What only the owner can check: the transfer packet with a real client, and how a
death of one edge of two feels.

---

## 9. Open questions for the owner

1. **What is to be survived: a process, a node, or a node with its disk?**
   - A process: workers and the store already are; edges need item 1; the coordinator
     is only missed. Cheapest.
   - A node: adds standbys that do not share a machine, and for the store either a
     replicated volume (days) or our own standby (weeks).
   - A node and its disk: the same, and the volume or the standby is no longer
     optional. A backup is needed in every case and does not exist.
2. **May Clustine depend on something outside itself?** On the Kubernetes API for a
   lease (cheap, but nothing for clusters of plain processes). On replicated storage
   for the store (no code, the pause is the platform's). On an external proxy in
   front of edges. On a database or object storage under the store. Or must it carry
   everything itself (then the lease lives in the store, and the store's standby is
   ours to build)?
3. **When an edge dies without warning, is a disconnect acceptable** if the player can
   join again at once and is back where they were? That is items 1 and 2. If
   connections must survive, it is a gateway in front of the edges: weeks more, after
   encryption, and the gateway is then what must not die.
4. **Is a player's place and hotbar surviving a log-out part of this milestone?**
   Without it "back where they were" is not possible, and it is missing today with one
   edge as well. If yes: kept in the store, or the start of the planned `playerdata`
   service?
5. **A second login of one account: turn the new one away, as the edge does now, or
   put the old one out, as the official server does?**
6. **How many players, regions, workers and edges is this meant for?** With tens of
   players, balance by count or by players is enough and several edges are only for
   survival. With hundreds in one place the edge's single fan-out task decides, and
   tick time is the right measure.
7. **Must high availability also work without Kubernetes**, for a cluster of plain
   processes? If yes, the lease in the store and a list of addresses. If no, the
   Kubernetes Lease and Services are less code.
8. **Will edges be reachable from the internet in this milestone?** Then online mode
   and encryption come first, which is a milestone part of its own. If not, several
   edges are tried through port forwarding, one at a time.
9. **How much authentication between services?** NetworkPolicies and a shared secret
   (days), or mutual TLS (more, with certificates to look after), or left to a mesh.
10. **How long may everyone stand still when the store fails over?** The edge gives
    up at 20 s. A replicated volume on a dead node takes longer than that unless
    something fences the node. Raise the patience, or accept the disconnect, or build
    the standby.
11. **Should the lease for a dead worker stay at 5 s?** The setting goes down to 3.
    Lower means shorter freezes and more false alarms on a busy machine.
12. **How will the owner try it?** Two edges need two addresses for a client, or a
    load balancer on the owner's machine. A dead node needs a kind cluster of several
    nodes, which the owner's machine has to carry.

---

## 10. Where the records and the code disagree, and what I did not check

Disagreements found:

- `docs/architecture.md` says of several edges "the workers can serve several, but
  there is no shared player list yet". The code has three more gaps (section 1.3).
- `architecture.md`'s table gives the coordinator "Replicated with embedded Raft,
  three replicas". It has nothing to replicate; a lease is enough.
- ADR-0007 says a restarted coordinator "asks rather than remembers" and that
  holdings are honoured "whatever the epochs" for whoever reports first. With two
  coordinators alive that rule is what lets each believe its own workers.
- The top comment of `deploy/kubernetes/worker.yaml` says each worker runs one of the
  regions; since C2a a worker runs several.
- `README.md`'s status says "The parts of the world each worker has are still fixed";
  regions ask the store since C2b.5, though the stripes are still pinned.
- `--to` of `clustine move` is documented in `main.rs` as "has to be one that runs no
  region"; ADR-0009 section 7 and `state.rs` allow any target.

Not checked:

- Nothing was built or run. Every statement about two coordinators or two edges is
  from reading.
- `fanout.rs` was read in the parts named, not as a whole. ADR-0011, 0012 and 0014
  were read by section, not line by line.
- How the vanilla 26.3 client behaves on a lost connection and on a transfer is from
  my knowledge of earlier versions. The packet ids are in the generated tables; the
  behaviour needs a real client.
- What kube-proxy and load balancers do with established connections to a pod that is
  no longer ready.
- The Rust crates named were read about, not evaluated. Whether Velocity supports
  protocol 777 was not looked into.
- Commit latency on a real disk has never been timed (the roadmap says so), so every
  latency above is an estimate.

Sources read on the web: the openraft documentation on docs.rs, the TiKV `raft`
releases, PaperMC's Velocity configuration page, the Kubernetes blog on non-graceful
node shutdown, and the docs.rs pages of `kube-lease-manager`, `kube-coordinate` and
`kube-leader-election`.
